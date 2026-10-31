//! Text drawn from a real font file.
//!
//! Glyphs are rasterized by FreeType (see ft2) once per character and size
//! into a tile holding two coverage masks - the ink and the outline around it -
//! so a glyph can be stamped in any colour without touching the font again.
//! Tiles are drawn at the window's own pixel size, which is the whole point:
//! the 1280x720 layer is stretched to the window, but the letters on top of it
//! are not.
//!
//! How heavy a glyph reads is split into independent knobs, because stacking
//! them all on top of each other is what made the first vector build look like
//! stickers instead of type:
//!
//!   * the face itself (and which face of a .ttc) - shape and base weight;
//!   * `bold` - morphological thickening of the coverage mask;
//!   * `ink` - the coverage curve (gamma by default, legacy linear gain);
//!   * the outline radius, its alpha, and an optional soft shadow - how the
//!     glyph separates from whatever is behind it.
//!
//! `with_ui_font` hands out one font for the whole program, so screens that
//! only need a caption do not have to thread one through their callers.

use crate::ft2;
use crate::render::Surface;
use std::cell::{Cell, Ref, RefCell};
use std::collections::{HashMap, HashSet};
use std::os::raw::c_int;

/// Default font for engine-side text.
///
/// Only a last resort: the font profiles found on this machine (see
/// `medium_paths` / `font_search_dirs`) are preferred, and `WA2_FONT` overrides
/// everything. It still has to name a file that exists on the platform, so that
/// a machine with no usable profile fails with a believable path in the error.
pub const DEFAULT_FONT: &str = if cfg!(target_os = "macos") {
    "/System/Library/Fonts/PingFang.ttc"
} else if cfg!(target_os = "windows") {
    "C:/Windows/Fonts/msyh.ttc"
} else {
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Medium.ttc"
};
/// Default face index inside the .ttc.
///
/// A .ttc is a collection: NotoSansCJK-Medium.ttc holds JP(0), KR(1), SC(2),
/// TC(3), HK(4). Face 0 draws Japanese glyph shapes - different shoulders on
/// 骨/直/着, different component proportions - so the Chinese text has to ask
/// for the SC face. `WA2_FONT` accepts `path#index` and `WA2_FONT_FACE`
/// overrides this.
///
/// PingFang.ttc (macOS) starts at the Simplified Chinese face, so the index
/// above does not apply there.
pub const DEFAULT_FACE: i64 = if cfg!(target_os = "macos") { 0 } else { 2 };
/// Default ink gamma in permille. 1000 leaves the coverage untouched; below
/// 1000 lifts the anti-aliased edge without the hard saturation a linear
/// percentage gain causes. See `VectorFont::set_ink_gamma`.
pub const DEFAULT_GAMMA: u32 = 900;
/// Legacy linear coverage gain, in percent. Only used when `WA2_FONT_GAIN`
/// is set explicitly, so the two curves never mix.
pub const DEFAULT_GAIN: u32 = 130;
/// Default outline coverage for dialog text, 0..=255. The original bitmap
/// font carried a full-black edge; a full-alpha ring around every glyph reads
/// as a sticker, so the default keeps it translucent.
pub const DEFAULT_EDGE_ALPHA: u8 = 160;

/// Version of the font pack the shipped `fonts/` directory is cut from. The
/// settings file records it so a replaced pack can tell the old ids apart from
/// ones that no longer exist; bump it whenever the bundled font files change.
pub const FONT_PACK_VERSION: i32 = 2;

/// Every character any font in this process had no glyph for, with how often
/// it was asked for. Reported once on exit (design §1.3 wants the data to
/// decide about font subsets) instead of only a per-character stderr line.
static MISSING_COUNTS: std::sync::Mutex<Option<HashMap<char, u32>>> = std::sync::Mutex::new(None);

fn note_missing(ch: char) {
    let mut slot = MISSING_COUNTS.lock().unwrap_or_else(|e| e.into_inner());
    *slot.get_or_insert_with(HashMap::new).entry(ch).or_insert(0) += 1;
}

/// The aggregated missing-glyph counts, most frequent first.
pub fn missing_glyph_stats() -> Vec<(char, u32)> {
    let slot = MISSING_COUNTS.lock().unwrap_or_else(|e| e.into_inner());
    let mut out: Vec<(char, u32)> = slot
        .as_ref()
        .map(|m| m.iter().map(|(c, n)| (*c, *n)).collect())
        .unwrap_or_default();
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

/// Print the aggregated missing-glyph report if anything was missing.
pub fn log_missing_glyph_stats() {
    let stats = missing_glyph_stats();
    if stats.is_empty() {
        return;
    }
    let unique = stats.len();
    let total: u32 = stats.iter().map(|(_, n)| *n).sum();
    let sample: String = stats
        .iter()
        .take(20)
        .map(|(c, n)| {
            let shown = if c.is_control() || c.is_whitespace() {
                format!("U+{:04X}", *c as u32)
            } else {
                format!("'{}'", c)
            };
            format!("{shown}x{n}")
        })
        .collect::<Vec<_>>()
        .join(" ");
    eprintln!("font: missing glyphs: {unique} unique / {total} requests: {sample}");
}

/// One rasterized glyph, ready to be stamped.
struct Tile {
    w: u32,
    h: u32,
    /// w * h * 4 bytes: R is the ink coverage, G the outline coverage.
    rgba: Vec<u8>,
    /// Offset from the pen (the baseline origin) to the tile's top-left.
    dx: i32,
    dy: i32,
    /// How far the pen moves right after this glyph, in 26.6 fixed point.
    /// Layout accumulates in these units and rounds where a glyph is stamped.
    advance_266: i32,
}

/// How the rasterized coverage is remapped into the ink channel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum InkMode {
    /// Legacy linear lift, in percent (100 = untouched).
    Gain(u32),
    /// Gamma curve, in permille (1000 = untouched).
    Gamma(u32),
}

/// FreeType hinting / render-target selection for FT_Load_Glyph.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HintMode {
    /// FT_LOAD_TARGET_LIGHT: the default for CJK dialog text.
    Light,
    /// No explicit target: FreeType's default hinting.
    Normal,
    /// Force the auto-hinter (matters for Latin faces without hinting data).
    Auto,
    /// No hinting at all.
    None,
}

impl HintMode {
    /// FT_Load_Glyph flags. FT_LOAD_RENDER is always set: everything
    /// downstream reads the slot's bitmap. FT_LOAD_NO_BITMAP keeps fonts with
    /// embedded monochrome strikes (the arphic CJK faces at 16px, for one) on
    /// the outline rasterizer - a 1-bit strike would otherwise come back as a
    /// pixel mode this pipeline cannot use and the whole size band would go
    /// missing.
    fn load_flags(self) -> c_int {
        const FT_LOAD_RENDER: c_int = 0x0000_0004;
        const FT_LOAD_NO_BITMAP: c_int = 0x0000_0008;
        const FT_LOAD_NO_HINTING: c_int = 0x0000_0002;
        const FT_LOAD_FORCE_AUTOHINT: c_int = 0x0000_0020;
        const FT_LOAD_TARGET_LIGHT: c_int = 0x0001_0000;
        let base = FT_LOAD_RENDER | FT_LOAD_NO_BITMAP;
        match self {
            HintMode::Light => base | FT_LOAD_TARGET_LIGHT,
            HintMode::Normal => base,
            HintMode::Auto => base | FT_LOAD_FORCE_AUTOHINT | FT_LOAD_TARGET_LIGHT,
            HintMode::None => base | FT_LOAD_NO_HINTING,
        }
    }

    fn parse(s: &str) -> Option<HintMode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "light" => Some(HintMode::Light),
            "normal" => Some(HintMode::Normal),
            "auto" | "autohint" => Some(HintMode::Auto),
            "none" | "off" => Some(HintMode::None),
            _ => None,
        }
    }

    /// Name as it appears in the startup font line.
    pub fn name(self) -> &'static str {
        match self {
            HintMode::Light => "light",
            HintMode::Normal => "normal",
            HintMode::Auto => "auto",
            HintMode::None => "none",
        }
    }
}

/// Where a piece of text sits, which decides how it is separated from the
/// background. The dialog body sits on CGs and needs the most help; the name
/// sits on the dark rim of the window; UI labels sit on their own opaque
/// plates and want no edge at all.
#[derive(Clone, Copy, Debug)]
pub enum TextRole {
    /// Dialogue body: white ink, thin translucent edge, soft shadow.
    Body,
    /// Speaker name on the window rim: white ink, thin edge, no shadow.
    Name,
    /// Menu / save / timeline labels: flat ink, no edge, no shadow.
    Ui,
}

/// One resolved text style: colour plus how it separates from the background.
#[derive(Clone, Copy, Debug)]
pub struct TextStyle {
    pub color: [u8; 3],
    /// Outline coverage, 0..=255. 0 draws no outline.
    pub edge_alpha: u8,
    /// Soft shadow: (dx, dy, alpha). Alpha 0 disables it.
    pub shadow: (i32, i32, u8),
    /// Shadow colour.
    pub shadow_rgb: [u8; 3],
    /// Rule under the line. The dialogue body only - a menu label sits on its
    /// own plate and has nothing to separate from.
    pub underline: bool,
}

impl TextStyle {
    /// Dialogue body on a CG: white ink, a one-pixel translucent edge and a
    /// slight offset shadow, so thin strokes stay readable over bright art.
    pub fn body() -> TextStyle {
        TextStyle {
            color: [255, 255, 255],
            edge_alpha: DEFAULT_EDGE_ALPHA,
            shadow: (1, 1, 110),
            shadow_rgb: [0, 0, 0],
            underline: false,
        }
    }

    /// Speaker name on the window rim. The body is plain white with a soft
    /// shadow; the name is a touch warmer and carries a slightly firmer edge,
    /// so the two read as different levels of the same interface rather than
    /// one style stamped twice. The original drew the name in the same white,
    /// which left it competing with the line underneath it.
    pub fn name() -> TextStyle {
        TextStyle {
            color: [255, 246, 226],
            edge_alpha: 170,
            shadow: (0, 0, 0),
            shadow_rgb: [0, 0, 0],
            underline: false,
        }
    }

    /// UI labels on their own plates: flat ink, nothing around it.
    pub fn ui() -> TextStyle {
        TextStyle {
            color: [255, 255, 255],
            edge_alpha: 0,
            shadow: (0, 0, 0),
            shadow_rgb: [0, 0, 0],
            underline: false,
        }
    }

    /// The style a role resolves to.
    pub fn for_role(role: TextRole) -> TextStyle {
        match role {
            TextRole::Body => TextStyle::body(),
            TextRole::Name => TextStyle::name(),
            TextRole::Ui => TextStyle::ui(),
        }
    }
}

/// A font file rasterized at one pixel size, with a glyph cache.
///
/// Everything is reachable through a shared reference and the cache is keyed
/// by (character, size): one font serves the whole program, so the settings
/// page asking for 18px between two dialog draws at 26px does not throw the
/// 26px glyphs away.
pub struct VectorFont {
    ft: RefCell<ft2::Ft>,
    size: Cell<u32>,
    /// Outline radius in pixels. The original bitmap font carried a one-pixel
    /// black edge, and dialog text sits on CGs, so it still needs one.
    outline: Cell<u32>,
    /// True once WA2_FONT_EDGE pinned the radius, so a resize keeps it.
    edge_override: Cell<bool>,
    /// Stroke thickening in pixels, applied by growing the coverage mask.
    bold: Cell<u32>,
    /// Coverage curve applied to the ink channel.
    ink: Cell<InkMode>,
    /// Outline coverage of dialog text, 0..=255.
    edge_alpha: Cell<u8>,
    /// Hinting / render target FT_Load_Glyph is called with.
    hint: Cell<HintMode>,
    /// Vertical metrics in pixels at the current size.
    metrics: Cell<ft2::Metrics>,
    /// Characters the face has no glyph for, already reported. The warning is
    /// once per character per font, so a line redrawn every typewriter tick
    /// does not flood the log.
    missing: RefCell<HashSet<char>>,
    cache: RefCell<HashMap<(char, u32), Tile>>,
    /// Backup face used only for characters the primary face has none for.
    /// Kept separate from the primary so a single absent symbol cannot change
    /// the style of the rest of the line.
    fallback: RefCell<Option<Box<ft2::Ft>>>,
    /// Family/face of the backup, for the startup log.
    fallback_desc: RefCell<Option<(String, i64)>>,
}

impl VectorFont {
    /// Open path at size pixels, on the default face. None when FreeType or
    /// the file is unavailable, in which case callers keep the bitmap font.
    pub fn open(path: &str, size: u32) -> Option<VectorFont> {
        VectorFont::open_face(path, size, DEFAULT_FACE)
    }

    /// Open a specific face of the file (see DEFAULT_FACE for the .ttc order).
    pub fn open_face(path: &str, size: u32, face: i64) -> Option<VectorFont> {
        let ft = ft2::Ft::open(path, size, face)?;
        let metrics = ft.metrics()?;
        Some(VectorFont::from_ft(ft, size, metrics))
    }

    fn from_ft(ft: ft2::Ft, size: u32, metrics: ft2::Metrics) -> VectorFont {
        VectorFont {
            size: Cell::new(size),
            outline: Cell::new(outline_for(size)),
            edge_override: Cell::new(false),
            bold: Cell::new(0),
            ink: Cell::new(InkMode::Gamma(DEFAULT_GAMMA)),
            edge_alpha: Cell::new(DEFAULT_EDGE_ALPHA),
            hint: Cell::new(HintMode::Light),
            metrics: Cell::new(metrics),
            missing: RefCell::new(HashSet::new()),
            ft: RefCell::new(ft),
            cache: RefCell::new(HashMap::new()),
            fallback: RefCell::new(None),
            fallback_desc: RefCell::new(None),
        }
    }

    pub fn size(&self) -> u32 {
        self.size.get()
    }

    /// Remap the ink coverage through a gamma curve, in permille. 1000 leaves
    /// it untouched; 900 lifts the anti-aliased edge towards solid without the
    /// hard saturation a linear percentage gain produces. Glyphs already
    /// cached at the old curve are dropped.
    pub fn set_ink_gamma(&self, permille: u32) {
        let permille = permille.clamp(400, 2000);
        if self.ink.get() == InkMode::Gamma(permille) {
            return;
        }
        self.ink.set(InkMode::Gamma(permille));
        self.cache.borrow_mut().clear();
    }

    pub fn ink_gamma(&self) -> Option<u32> {
        match self.ink.get() {
            InkMode::Gamma(g) => Some(g),
            InkMode::Gain(_) => None,
        }
    }

    /// Legacy linear lift of the ink coverage, in percent; 100 leaves it
    /// alone. Superseded by the gamma curve - setting this switches the ink
    /// channel back to the linear mode.
    pub fn set_ink_gain(&self, pct: u32) {
        let pct = pct.clamp(25, 400);
        if self.ink.get() == InkMode::Gain(pct) {
            return;
        }
        self.ink.set(InkMode::Gain(pct));
        self.cache.borrow_mut().clear();
    }

    pub fn ink_gain(&self) -> Option<u32> {
        match self.ink.get() {
            InkMode::Gain(g) => Some(g),
            InkMode::Gamma(_) => None,
        }
    }

    /// How much the strokes are thickened, in pixels.
    pub fn bold(&self) -> u32 {
        self.bold.get()
    }

    /// Thicken the strokes by growing the coverage mask. Glyphs already cached
    /// at the old weight are dropped.
    pub fn set_bold(&self, px: u32) {
        if px == self.bold.get() {
            return;
        }
        self.bold.set(px);
        self.cache.borrow_mut().clear();
    }

    /// Outline coverage of dialog text, 0..=255.
    pub fn set_edge_alpha(&self, alpha: u8) {
        self.edge_alpha.set(alpha);
    }

    pub fn edge_alpha(&self) -> u8 {
        self.edge_alpha.get()
    }

    /// Override the outline radius in pixels. None goes back to the
    /// size-derived radius. Changing it invalidates the glyph cache.
    pub fn set_edge_px(&self, px: Option<u32>) {
        let want = px.unwrap_or_else(|| outline_for(self.size.get()));
        if want == self.outline.get() && self.edge_override.get() == px.is_some() {
            return;
        }
        self.edge_override.set(px.is_some());
        self.outline.set(want);
        self.cache.borrow_mut().clear();
    }

    pub fn edge_px(&self) -> u32 {
        self.outline.get()
    }

    /// True when the primary face itself has a glyph for `ch`.
    pub fn has_glyph(&self, ch: char) -> bool {
        self.ft.borrow_mut().glyph(ch).is_some()
    }

    /// Install a backup face for characters the primary face is missing. It is
    /// only consulted per glyph, so the body of a line never switches font.
    pub fn set_fallback(&self, path: &str, face: i64) -> bool {
        let size = self.size.get();
        match ft2::Ft::open(path, size, face) {
            Some(mut fb) => {
                // The backup has to be rendered exactly like the primary, or a
                // single borrowed glyph would have a different hinting weight.
                fb.set_load_flags(self.hint.get().load_flags());
                *self.fallback.borrow_mut() = Some(Box::new(fb));
                *self.fallback_desc.borrow_mut() = Some((path.to_string(), face));
                // Glyph-cache entries for missing characters were gaps; drop
                // them so the next draw picks the backup face up.
                self.cache.borrow_mut().clear();
                true
            }
            None => false,
        }
    }

    pub fn clear_fallback(&self) {
        if self.fallback.borrow().is_none() {
            return;
        }
        *self.fallback.borrow_mut() = None;
        *self.fallback_desc.borrow_mut() = None;
        self.cache.borrow_mut().clear();
    }

    /// Family of the backup face, if one is installed.
    pub fn fallback_family(&self) -> Option<String> {
        self.fallback.borrow().as_ref().and_then(|f| f.family())
    }

    /// `(path, face)` of the backup face, if one is installed.
    pub fn fallback_spec(&self) -> Option<(String, i64)> {
        self.fallback_desc.borrow().clone()
    }

    /// Select the FreeType hinting / render target. Changing it invalidates
    /// the glyph cache, and the backup face follows the primary so borrowed
    /// glyphs keep the same weight.
    pub fn set_hint(&self, hint: HintMode) {
        if hint == self.hint.get() {
            return;
        }
        if self.ft.borrow_mut().set_load_flags(hint.load_flags()) {
            self.hint.set(hint);
            if let Some(fb) = self.fallback.borrow_mut().as_mut() {
                fb.set_load_flags(hint.load_flags());
            }
            self.cache.borrow_mut().clear();
        }
    }

    pub fn hint(&self) -> HintMode {
        self.hint.get()
    }

    /// Re-rasterize at a new pixel size. Already-cached sizes survive.
    pub fn set_size(&self, size: u32) -> bool {
        if size == 0 {
            return false;
        }
        if size == self.size.get() {
            return true;
        }
        if self.ft.borrow_mut().set_size(size).is_none() {
            return false;
        }
        let metrics = match self.ft.borrow().metrics() {
            Some(m) => m,
            None => return false,
        };
        self.size.set(size);
        self.metrics.set(metrics);
        if let Some(fb) = self.fallback.borrow_mut().as_mut() {
            let _ = fb.set_size(size);
        }
        // The outline is derived from the size unless it was pinned.
        if !self.edge_override.get() {
            self.outline.set(outline_for(size));
        }
        true
    }

    /// Vertical metrics in pixels at the current size.
    pub fn metrics(&self) -> ft2::Metrics {
        self.metrics.get()
    }

    /// Pixels above the baseline: where to put the baseline of a line whose
    /// top edge is known.
    pub fn ascent(&self) -> i32 {
        self.metrics.get().ascender
    }

    /// Pixels below the baseline (<= 0).
    pub fn descent(&self) -> i32 {
        self.metrics.get().descender
    }

    /// Suggested line height in pixels.
    pub fn line_height(&self) -> i32 {
        self.metrics.get().height
    }

    /// Baseline for a line box whose top edge and height are known: the
    /// font's own vertical extent is centred in the box, so the same code
    /// lands correctly on any face, size or window scale.
    pub fn baseline_in_box(&self, top: i32, box_h: i32) -> i32 {
        let m = self.metrics.get();
        top + (box_h - m.ascender - m.descender) / 2 + m.ascender
    }

    /// Family name of the open face, e.g. "Noto Sans CJK SC".
    pub fn family(&self) -> String {
        self.ft.borrow().family().unwrap_or_else(|| "?".to_string())
    }

    /// One line describing the open font: family, face index and the knobs.
    /// Printed at startup so the log says which face of the .ttc is in use,
    /// not just which file was opened.
    pub fn describe(&self) -> String {
        let ft = self.ft.borrow();
        let family = ft.family().unwrap_or_else(|| "?".to_string());
        let style = ft.style().unwrap_or_default();
        let ink = match self.ink.get() {
            InkMode::Gamma(g) => format!("gamma={:.2}", g as f32 / 1000.0),
            InkMode::Gain(g) => format!("gain={}%", g),
        };
        format!(
            "{} {} (face {} of {}) {}px {} bold={} edge={}px@{} hint={}",
            family,
            style,
            ft.face_index(),
            ft.num_faces(),
            self.size.get(),
            ink,
            self.bold.get(),
            self.outline.get(),
            self.edge_alpha.get(),
            self.hint.get().name(),
        )
    }

    fn ink_extents(&self, text: &str) -> (i32, i32, i32) {
        let (mut top, mut bottom, mut width) = (0i32, 0i32, 0i32);
        let mut prev: Option<char> = None;
        for ch in text.chars() {
            let t = self.tile(ch);
            top = top.max(-t.dy);
            bottom = bottom.max(t.dy + t.h as i32);
            width += self.advance_266(prev, ch);
            prev = Some(ch);
        }
        (top, bottom, round_266(width))
    }

    /// How far this text's ink reaches above and below the baseline, in
    /// pixels. Layout uses it to place a caption inside the box the artwork
    /// gave it, and to check afterwards that nothing crossed a rule.
    pub fn ink_bounds(&self, text: &str) -> (i32, i32) {
        let (top, bottom, _) = self.ink_extents(text);
        (top, bottom)
    }

    /// Width of text at the current size, kerning included.
    pub fn measure(&self, text: &str) -> i32 {
        self.measure_tracked(text, 0)
    }

    /// Width of text with `tracking` pixels added between glyphs. Tracking is
    /// a layout property, so it counts towards the width used for centring -
    /// that is the whole point of doing it here instead of typing spaces into
    /// the string.
    pub fn measure_tracked(&self, text: &str, tracking: i32) -> i32 {
        let mut x = 0i32;
        let mut prev: Option<char> = None;
        let mut n = 0usize;
        for ch in text.chars() {
            x += self.advance_after(prev, ch);
            if n > 0 {
                x += tracking;
            }
            prev = Some(ch);
            n += 1;
        }
        x
    }

    /// Advance width of one character. Layout uses it to decide where the next
    /// pen position is before committing to drawing.
    pub fn measure_char(&self, ch: char) -> i32 {
        round_266(self.advance_266(None, ch))
    }

    /// How far the pen moves after drawing `ch` when it follows `prev`:
    /// the glyph's own advance plus any kerning between the two, in pixels.
    /// The CJK width policy is applied, so this agrees with what drawing does.
    pub fn advance_after(&self, prev: Option<char>, ch: char) -> i32 {
        round_266(self.advance_266(prev, ch))
    }

    /// The same advance in 26.6 fixed point. The dialog layout keeps its pen
    /// in these units and rounds only when a glyph is stamped, so a long mixed
    /// CJK/Latin line does not accumulate a pixel of drift per character.
    pub fn advance_266(&self, prev: Option<char>, ch: char) -> i32 {
        let t = self.tile(ch);
        let base = if full_width_punct(ch) {
            // The original drew every CJK punctuation cell at the same pitch;
            // see the width policy below.
            (self.size.get() as i32) << 6
        } else {
            t.advance_266
        };
        base + self.kerning_266(prev, ch)
    }

    /// Where the glyph's ink sits relative to the pen: (left offset, width).
    /// Used for the last-character wrap check, which has to look at ink
    /// rather than at the advance the next glyph would start from.
    pub fn char_ink(&self, ch: char) -> (i32, i32) {
        let t = self.tile(ch);
        (t.dx, t.w as i32)
    }

    /// Draw one character at the pen position with a style.
    pub fn draw_char_styled(
        &self,
        surf: &mut Surface,
        ch: char,
        pen: i32,
        baseline: i32,
        style: &TextStyle,
    ) {
        let t = self.tile(ch);
        stamp_styled(surf, &t, pen, baseline, style);
    }

    /// Draw one character at the pen position, white with a black outline.
    pub fn draw_char(&self, surf: &mut Surface, ch: char, pen: i32, baseline: i32) {
        self.draw_char_styled(surf, ch, pen, baseline, &TextStyle::body());
    }

    /// Draw text in white with a black outline, starting at (x, baseline).
    /// Returns the pen position after the last glyph.
    pub fn draw(&self, surf: &mut Surface, text: &str, x: i32, baseline: i32) -> i32 {
        self.draw_styled(surf, text, x, baseline, &TextStyle::body())
    }

    /// Draw text with a style, starting at (x, baseline). Returns the pen
    /// position after the last glyph.
    pub fn draw_styled(
        &self,
        surf: &mut Surface,
        text: &str,
        x: i32,
        baseline: i32,
        style: &TextStyle,
    ) -> i32 {
        // The pen lives in 26.6 and is rounded only where a glyph is
        // stamped, so Latin advances do not lose their sub-pixel part one
        // character at a time.
        let mut pen = (x as i32) << 6;
        let mut prev: Option<char> = None;
        for ch in text.chars() {
            let t = self.tile(ch);
            stamp_styled(surf, &t, round_266(pen), baseline, style);
            pen += self.advance_266(prev, ch);
            prev = Some(ch);
        }
        round_266(pen)
    }

    /// Draw at most limit characters - the typewriter reveals a line by
    /// redrawing a longer prefix of it.
    pub fn draw_limited(
        &self,
        surf: &mut Surface,
        text: &str,
        x: i32,
        baseline: i32,
        ink: [u8; 3],
        limit: usize,
    ) -> i32 {
        let style = TextStyle { color: ink, ..TextStyle::body() };
        let mut pen = (x as i32) << 6;
        let mut prev: Option<char> = None;
        for (n, ch) in text.chars().enumerate() {
            if n >= limit {
                break;
            }
            let t = self.tile(ch);
            stamp_styled(surf, &t, round_266(pen), baseline, &style);
            pen += self.advance_266(prev, ch);
            prev = Some(ch);
        }
        round_266(pen)
    }

    /// Draw text in one flat colour with no outline, onto a surface that is
    /// already opaque. Menu and page labels sit on their own plate, where the
    /// original drew them solid.
    pub fn draw_flat(&self, surf: &mut Surface, text: &str, x: i32, baseline: i32, ink: [u8; 3]) -> i32 {
        let mut pen = (x as i32) << 6;
        let mut prev: Option<char> = None;
        for ch in text.chars() {
            let t = self.tile(ch);
            stamp_ink(surf, &t, round_266(pen), baseline, ink);
            pen += self.advance_266(prev, ch);
            prev = Some(ch);
        }
        round_266(pen)
    }

    /// One line rasterized into its own transparent RGBA buffer, sized to the
    /// text. The (w, h, rgba) shape is what the UI code already positions and
    /// tints by.
    pub fn line_rgba(&self, text: &str, size: u32) -> Option<(u32, u32, Vec<u8>)> {
        if text.is_empty() || !self.set_size(size) {
            return None;
        }
        let pad = 4i32;
        let (top, bottom, width) = self.ink_extents(text);
        let w = (width + pad * 2).max(1) as u32;
        let h = (top + bottom + pad * 2).max(1) as u32;
        let mut buf = vec![0u8; (w * h * 4) as usize];
        self.stamp_buf(&mut buf, w, h, text, pad, pad + top, &TextStyle::for_role(TextRole::Ui), 0);
        Some((w, h, buf))
    }

    /// Several lines stacked into one w x h buffer, the way the save/load
    /// captions and the backlog want them.
    pub fn block_rgba(
        &self,
        lines: &[String],
        w: u32,
        h: u32,
        size: u32,
    ) -> Option<(u32, u32, Vec<u8>)> {
        if w == 0 || h == 0 || !self.set_size(size) {
            return None;
        }
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let line_h = size as i32 + 8;
        let pad = 4i32;
        for (i, line) in lines.iter().enumerate() {
            let y = i as i32 * line_h;
            if y + line_h > h as i32 {
                break;
            }
            let (top, bottom, _) = self.ink_extents(line);
            let base = y + (line_h - (top + bottom)) / 2 + top;
            self.stamp_buf(&mut buf, w, h, line, pad, base, &TextStyle::for_role(TextRole::Ui), 0);
        }
        Some((w, h, buf))
    }

    /// Every label painted onto one transparent canvas. The settings page
    /// caches a canvas per state, so the labels have to come back as a buffer.
    pub fn labels_rgba(&self, w: u32, h: u32, labels: &[Label]) -> Option<Vec<u8>> {
        if w == 0 || h == 0 {
            return None;
        }
        let mut buf = vec![0u8; (w * h * 4) as usize];
        for l in labels {
            if l.text.is_empty() || !self.set_size(l.size) {
                continue;
            }
            let x = if l.center {
                l.x + (l.w - self.measure_tracked(&l.text, l.tracking)) / 2
            } else {
                l.x
            };
            let (ink_top, ink_bottom, _) = self.ink_extents(&l.text);
            let m = self.metrics.get();
            let line = m.ascender - m.descender;
            // Explicit baseline rather than centring the ink: the box is what
            // the artwork laid out, so the font's own line sits centred in it
            // and every caption on the page shares one rhythm. A box shorter
            // than the line would push the baseline past its own bottom, so
            // that case falls back to centring the ink.
            let baseline = if line <= l.h {
                l.y + (l.h - line) / 2 + m.ascender
            } else {
                l.y + (l.h - (ink_top + ink_bottom)) / 2 + ink_top
            };
            // UI labels carry their own colour and sit on an opaque plate, so
            // they take the flat style: no edge, no shadow.
            let style = TextStyle { color: l.rgb, ..TextStyle::for_role(TextRole::Ui) };
            self.stamp_buf(&mut buf, w, h, &l.text, x, baseline, &style, l.tracking);
        }
        Some(buf)
    }

    /// Stamp into a raw RGBA buffer. Unlike Surface::put, which forces the
    /// alpha to 255, this keeps the glyph's coverage so the buffer can be
    /// blitted - and re-tinted - later without losing its antialiasing.
    fn stamp_buf(
        &self,
        buf: &mut [u8],
        bw: u32,
        bh: u32,
        text: &str,
        x: i32,
        baseline: i32,
        style: &TextStyle,
        tracking: i32,
    ) {
        let mut pen = (x as i32) << 6;
        let track_266 = (tracking as i32) << 6;
        let mut prev: Option<char> = None;
        let mut first = true;
        for ch in text.chars() {
            // The tracking goes into the gap before each glyph after the
            // first one. Adding it after the advance instead leaves the first
            // gap out, while `measure_tracked` still counts it - so the line
            // renders narrower than it measures and every later gap is one
            // step too far right.
            if !first {
                pen += track_266;
            }
            let t = self.tile(ch);
            stamp_raw(buf, bw, bh, &t, round_266(pen), baseline, style);
            pen += self.advance_266(prev, ch);
            first = false;
            prev = Some(ch);
        }
    }

    fn tile(&self, ch: char) -> Ref<'_, Tile> {
        let key = (ch, self.size.get());
        if !self.cache.borrow().contains_key(&key) {
            let t = self.build_tile(ch);
            self.cache.borrow_mut().insert(key, t);
        }
        Ref::map(self.cache.borrow(), |m| match m.get(&key) {
            Some(t) => t,
            None => unreachable!(),
        })
    }

    fn build_tile(&self, ch: char) -> Tile {
        let size = self.size.get();
        // The face borrow has to end before missing_tile, which asks the face
        // its family name for the log line.
        let g = { self.ft.borrow_mut().glyph(ch) };
        let g = match g {
            Some(g) => g,
            None => {
                let fb = self
                    .fallback
                    .borrow_mut()
                    .as_mut()
                    .and_then(|fb| fb.glyph(ch));
                match fb {
                    Some(g) => return self.tile_from_glyph(g),
                    None => return self.missing_tile(ch, size),
                }
            }
        };
        self.tile_from_glyph(g)
    }

    fn tile_from_glyph(&self, g: ft2::Glyph) -> Tile {
        let pad = self.outline.get() as usize;
        let grow = self.bold.get() as usize;
        // The margin has to hold both the thickened stroke and the outline
        // drawn around it, or the tile clips what it just grew.
        let m = pad + grow;
        let (gw, gh) = (g.w as usize, g.h as usize);
        let (tw, th) = (gw + m * 2, gh + m * 2);
        let mut ink = vec![0u8; tw * th];
        for y in 0..gh {
            for x in 0..gw {
                ink[(y + m) * tw + (x + m)] = g.mask[y * gw + x];
            }
        }
        // Thickening first means the outline follows the heavier stroke
        // instead of the thin original.
        if grow > 0 {
            ink = dilate(&ink, tw, th, grow);
        }
        // The coverage curve is applied before the outline is grown, so the
        // ring follows the stroke as it will actually be seen.
        let lut = self.ink_lut();
        for v in ink.iter_mut() {
            *v = lut[*v as usize];
        }
        let mut rgba = vec![0u8; tw * th * 4];
        if pad > 0 {
            let edge = dilate(&ink, tw, th, pad);
            for i in 0..(tw * th) {
                if ink[i] == 0 {
                    rgba[i * 4 + 1] = edge[i];
                }
            }
        }
        for i in 0..(tw * th) {
            rgba[i * 4] = ink[i];
        }
        Tile {
            w: tw as u32,
            h: th as u32,
            rgba,
            dx: g.left - m as i32,
            dy: -(g.top + m as i32),
            advance_266: g.advance_266,
        }
    }

    /// The face has no glyph for `ch`. The text keeps a gap of the width the
    /// character would have had - never a ragged hole and never a placeholder
    /// box, which read worse than the gap - and the first time each character
    /// goes missing it is named in the log, so a silent hole is a bug report
    /// instead of a mystery.
    fn missing_tile(&self, ch: char, size: u32) -> Tile {
        note_missing(ch);
        if self.missing.borrow_mut().insert(ch) {
            let shown = if ch.is_control() || ch.is_whitespace() {
                format!("U+{:04X}", ch as u32)
            } else {
                format!("U+{:04X} '{}'", ch as u32, ch.escape_default())
            };
            eprintln!(
                "font: {} has no glyph for {} - leaving a gap",
                self.family(),
                shown
            );
        }
        let advance = if ch.is_ascii() {
            (size / 2).max(1) as i32
        } else {
            size.max(1) as i32
        };
        Tile {
            w: 0,
            h: 0,
            rgba: Vec::new(),
            dx: 0,
            dy: 0,
            advance_266: (advance as i32) << 6,
        }
    }

    /// 256-entry lookup table for the ink coverage curve.
    fn ink_lut(&self) -> [u8; 256] {
        let mut lut = [0u8; 256];
        match self.ink.get() {
            InkMode::Gain(pct) => {
                for (i, v) in lut.iter_mut().enumerate() {
                    *v = ((i as u32 * pct) / 100).min(255) as u8;
                }
            }
            InkMode::Gamma(permille) => {
                let g = permille as f32 / 1000.0;
                for (i, v) in lut.iter_mut().enumerate() {
                    let a = i as f32 / 255.0;
                    *v = (255.0 * a.powf(g)).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        lut
    }

    /// Kerning between two characters, in 26.6 fixed point. Only the scripts
    /// that carry a kern table get it: CJK advances are already full-width, and
    /// asking FreeType for every pair would slow the dialog loop for nothing.
    fn kerning_266(&self, prev: Option<char>, ch: char) -> i32 {
        let Some(p) = prev else { return 0 };
        if !(p.is_ascii() && ch.is_ascii()) {
            return 0;
        }
        self.ft.borrow_mut().kerning_266(p, ch)
    }
}

/// Round a 26.6 fixed-point value to whole pixels.
fn round_266(v: i32) -> i32 {
    (v + 32) >> 6
}

/// Characters that must advance a full em in this engine's text, whatever the
/// font file says about them.
///
/// The original bitmap font stepped every one of these at the same 28px pitch,
/// so 「」，。！？…――""''※ all occupied a full-width cell and a line of dialogue
/// had an even rhythm. Noto Sans CJK agrees for almost all of them, but it
/// gives U+2014 (em dash) 23px instead of 26, which shows up as a visible gap
/// inside a ―― run, and a player-supplied Latin face may disagree about more of
/// them. The override keeps the treatment uniform and independent of the face.
///
/// The set is the CP932 double-byte punctuation block plus the fullwidth forms
/// block, minus the characters that also belong to Latin text (acute,
/// diaeresis, plus-minus, times, divide, degree, section, pilcrow, permille):
/// those keep their proportional advance so "10±5" still typesets as Latin.
///
/// U+FF01..U+FF60 are the fullwidth ASCII/bracket forms and U+FFE0..U+FFE6 the
/// fullwidth currency and sign forms; they are one em in a CJK face already, so
/// the override is dead weight for Noto but keeps a player-supplied face from
/// narrowing them. The *halfwidth* forms (U+FF61..U+FF9F, halfwidth katakana
/// and CJK punctuation) are deliberately not in the set - they are narrow by
/// definition and are the one place a Latin-ish face can be right.
fn full_width_punct(ch: char) -> bool {
    matches!(ch,
        // CJK brackets, punctuation, iteratio, space, prolonged sound mark
        '\u{3000}'..='\u{3003}' | '\u{3005}'..='\u{3007}' | '\u{3008}'..='\u{3015}'
        | '\u{30FB}' | '\u{30FC}' | '\u{309B}'..='\u{309E}'
        // fullwidth ASCII variants, brackets, currency and sign forms
        | '\u{FF01}'..='\u{FF60}' | '\u{FFE0}'..='\u{FFE6}'
        // dashes, quotes, dagger, bullet, ellipsis, reference mark
        | '\u{2010}'..='\u{2015}' | '\u{2018}'..='\u{201D}' | '\u{2020}'..='\u{2026}'
        | '\u{203B}'
        // arrows, geometric shapes, stars, music, temperature
        | '\u{2190}'..='\u{2193}' | '\u{2225}' | '\u{25A0}'..='\u{25FF}'
        | '\u{2605}' | '\u{2606}' | '\u{266A}' | '\u{266D}' | '\u{266F}' | '\u{2103}')
}

/// Maximum filter over a round structuring element: every pixel takes the
/// strongest coverage within radius r, which is how the strokes get thicker and
/// how the outline is grown around them.
fn dilate(mask: &[u8], w: usize, h: usize, r: usize) -> Vec<u8> {
    if r == 0 || w == 0 || h == 0 {
        return mask.to_vec();
    }
    let r2 = (r as i32) * (r as i32);
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut best = 0u8;
            for oy in -(r as i32)..=r as i32 {
                for ox in -(r as i32)..=r as i32 {
                    if ox * ox + oy * oy > r2 {
                        continue;
                    }
                    let (sx, sy) = (x as i32 + ox, y as i32 + oy);
                    if sx >= 0 && sy >= 0 && sx < w as i32 && sy < h as i32 {
                        best = best.max(mask[sy as usize * w + sx as usize]);
                    }
                }
            }
            out[y * w + x] = best;
        }
    }
    out
}

/// One label placed on a shared canvas.
pub struct Label {
    pub text: String,
    /// Box the label sits in, in the canvas's own pixels.
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub size: u32,
    /// Centre the text in the box instead of left-aligning it.
    pub center: bool,
    pub rgb: [u8; 3],
    /// Extra pixels between glyphs, in this canvas's own pixels (the caller
    /// scales it from the layout). This is letter tracking done properly:
    /// putting real spaces into the string would change its width, its
    /// centring and its wrap.
    pub tracking: i32,
}

impl Label {
    /// A label with no tracking, for callers that do not letter-space.
    pub fn plain(text: impl Into<String>, x: i32, y: i32, w: i32, h: i32, size: u32, rgb: [u8; 3]) -> Label {
        Label {
            text: text.into(),
            x,
            y,
            w,
            h,
            size,
            center: true,
            rgb,
            tracking: 0,
        }
    }
}

/// One cached font instance. `gen` is the generation of the role ids it was
/// opened under: bumping an id drops the instance, and with it the glyph
/// cache, so not one glyph of the old font can survive into the new one.
struct RoleFont {
    gen: u64,
    vf: VectorFont,
}

thread_local! {
    /// One font per role, each with its own FreeType face and its own glyph
    /// cache. Sharing one instance was what locked the menu, the history and
    /// the dialogue to the same file.
    static ROLE_FONTS: RefCell<[Option<RoleFont>; 3]> = const { RefCell::new([None, None, None]) };
    /// The generation whose open failed. A missing or broken font must not be
    /// re-opened on every frame (design §5.4); the failure is cached until the
    /// role ids change.
    static ROLE_FAILED_GEN: RefCell<[Option<u64>; 3]> = const { RefCell::new([None, None, None]) };
}

/// What the player picked for each role, and how often it changed. The ids
/// live outside the thread-local instances so the settings screen can drop
/// every cached font from wherever it is applied.
static ROLE_IDS: std::sync::Mutex<[i32; 3]> = std::sync::Mutex::new([0, 0, 0]);
static ROLE_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
/// The ids as written by the caller (wa2.ini / a direct call), before they are
/// clamped to something this machine has. Kept so the panel and the logs can
/// say what was asked for instead of only what happened to open.
static ROLE_REQUESTED: std::sync::Mutex<[i32; 3]> = std::sync::Mutex::new([0, 0, 0]);
/// Why a role is not using the font its id names, if it is not.
static ROLE_FALLBACK: std::sync::Mutex<[Option<String>; 3]> = std::sync::Mutex::new([None, None, None]);

/// The font file and the knobs that change how heavy it reads.
pub struct FontChoice {
    pub path: String,
    /// Face index inside the file (which language of a .ttc).
    pub face: i64,
    /// Stroke thickening in pixels.
    pub bold: u32,
    /// Ink gamma in permille; None means the legacy linear gain is in use.
    pub gamma: Option<u32>,
    /// Legacy linear coverage gain in percent.
    pub gain: u32,
    /// Outline radius override in pixels; None derives it from the size.
    pub edge_px: Option<u32>,
    /// Outline coverage, 0..=255.
    pub edge_alpha: u8,
    /// FreeType hinting / render target.
    pub hint: HintMode,
}

/// Split a `path#index` font spec. A bare path keeps face index None.
fn split_font_spec(spec: &str) -> (String, Option<i64>) {
    match spec.rsplit_once('#') {
        Some((path, idx)) => match idx.trim().parse::<i64>() {
            Ok(n) => (path.to_string(), Some(n)),
            Err(_) => (spec.to_string(), None),
        },
        None => (spec.to_string(), None),
    }
}

// ---------------------------------------------------------------------------
// dialogue text style
// ---------------------------------------------------------------------------

/// How the dialogue body is drawn once the player has moved it off the
/// defaults. It lives with the font roles because a change has to reach the
/// same place: the next line drawn, not the one already on screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DialogStyle {
    /// Body size in 720p pixels; the window scales it from there.
    pub size: u32,
    /// Outline radius in pixels. 0 keeps the one the size implies.
    pub edge: u32,
    /// Outline coverage, 0..=255. 0 draws no outline.
    pub edge_alpha: u32,
    /// Index into DIALOG_COLORS.
    pub color: u32,
    pub underline: bool,
    /// Synthetic stroke weight in pixels (0 = the face's own weight).
    pub weight: u32,
    /// Letter tracking in pixels, added between glyphs.
    pub tracking: i32,
    /// Extra pixels between body lines on top of the layout's own pitch.
    pub line_gap: i32,
}

pub const DIALOG_SIZE_MIN: u32 = 20;
pub const DIALOG_SIZE_MAX: u32 = 40;
pub const DIALOG_EDGE_MAX: u32 = 3;
pub const DIALOG_EDGE_ALPHA_MAX: u32 = 255;
pub const DIALOG_WEIGHT_MAX: u32 = 4;
pub const DIALOG_TRACK_MIN: i32 = -2;
pub const DIALOG_TRACK_MAX: i32 = 6;
pub const DIALOG_LINE_GAP_MAX: i32 = 12;

/// The body colours the player can pick: white, then warm and cool tints that
/// still read on a CG.
pub const DIALOG_COLORS: [[u8; 3]; 8] = [
    [255, 255, 255],
    [255, 246, 226],
    [255, 231, 205],
    [226, 255, 235],
    [205, 233, 255],
    [233, 214, 255],
    [255, 214, 231],
    [206, 206, 206],
];

pub const DIALOG_COLOR_NAMES: [&str; 8] = ["白", "暖白", "米色", "淡绿", "淡蓝", "淡紫", "淡粉", "灰"];

impl DialogStyle {
    /// What the game ships with: 26px, the edge the size implies, white, no
    /// rule under the line, the face's own weight, no tracking or extra gap.
    pub const fn default_style() -> DialogStyle {
        DialogStyle {
            size: 26,
            edge: 0,
            edge_alpha: DEFAULT_EDGE_ALPHA as u32,
            color: 0,
            underline: false,
            weight: 0,
            tracking: 0,
            line_gap: 0,
        }
    }

    /// Clamp a value that came out of the config file.
    pub fn clamp(self) -> DialogStyle {
        DialogStyle {
            size: self.size.clamp(DIALOG_SIZE_MIN, DIALOG_SIZE_MAX),
            edge: self.edge.min(DIALOG_EDGE_MAX),
            edge_alpha: self.edge_alpha.min(DIALOG_EDGE_ALPHA_MAX),
            color: self.color.min(DIALOG_COLORS.len() as u32 - 1),
            underline: self.underline,
            weight: self.weight.min(DIALOG_WEIGHT_MAX),
            tracking: self.tracking.clamp(DIALOG_TRACK_MIN, DIALOG_TRACK_MAX),
            line_gap: self.line_gap.clamp(0, DIALOG_LINE_GAP_MAX),
        }
    }
}

static DIALOG: std::sync::Mutex<DialogStyle> = std::sync::Mutex::new(DialogStyle::default_style());

pub fn set_dialog_style(s: DialogStyle) -> bool {
    let s = s.clamp();
    let mut cur = DIALOG.lock().unwrap_or_else(|e| e.into_inner());
    if *cur == s {
        return false;
    }
    *cur = s;
    true
}

pub fn dialog_style() -> DialogStyle {
    *DIALOG.lock().unwrap_or_else(|e| e.into_inner())
}

/// Body size in 720p pixels, for the caller that scales it to the window.
pub fn dialog_font_size() -> u32 {
    dialog_style().size
}

/// The body style the dialogue is drawn with: the player's colour and rule on
/// top of the shadow the artwork was tuned for.
pub fn dialog_body_style() -> TextStyle {
    let s = dialog_style();
    TextStyle {
        color: DIALOG_COLORS[(s.color as usize).min(DIALOG_COLORS.len() - 1)],
        edge_alpha: s.edge_alpha.min(255) as u8,
        underline: s.underline,
        ..TextStyle::body()
    }
}

/// Outline coverage the player picked (0 draws none).
pub fn dialog_edge_alpha() -> u32 {
    dialog_style().edge_alpha
}

/// Synthetic stroke weight in pixels for the dialogue body.
pub fn dialog_weight_px() -> u32 {
    dialog_style().weight
}

/// Letter tracking in pixels for the dialogue body.
pub fn dialog_tracking_px() -> i32 {
    dialog_style().tracking
}

/// Extra line pitch in pixels for the dialogue body.
pub fn dialog_line_gap() -> i32 {
    dialog_style().line_gap
}

/// `WA2_FONT_BOLD`, which stays a global override for every role. The panel's
/// weight is added on top of it so both keep working.
pub fn env_bold_px() -> u32 {
    std::env::var("WA2_FONT_BOLD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// The outline radius the player asked for, or None to keep the one the size
/// implies - which is what the game shipped with.
pub fn dialog_edge_px(size: u32) -> Option<u32> {
    let e = dialog_style().edge;
    if e == 0 {
        None
    } else {
        Some(e.min(size / 4).max(1))
    }
}

// ---------------------------------------------------------------------------
// font roles
// ---------------------------------------------------------------------------

/// The three fonts the player sets independently.
///
/// Everything used to borrow one font, so the title screen, the text history
/// and the dialogue had to share a file and a glyph cache. A role owns its own
/// instance now: changing one role drops only that role's cache.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FontRole {
    /// Title screen, option screens, save/load, timeline, music mode.
    Home,
    /// The text history / backlog screen.
    History,
    /// Dialogue, speaker names and narration.
    Game,
}

/// Display order in the settings page: the text the player reads most comes
/// first. The slot each role owns is `FontRole::index`, not this order.
pub const FONT_ROLES: [FontRole; 3] = [FontRole::Game, FontRole::History, FontRole::Home];

impl FontRole {
    pub fn index(self) -> usize {
        match self {
            FontRole::Home => 0,
            FontRole::History => 1,
            FontRole::Game => 2,
        }
    }

    /// Short tag for the log line.
    pub fn tag(self) -> &'static str {
        match self {
            FontRole::Home => "home",
            FontRole::History => "history",
            FontRole::Game => "game",
        }
    }

    /// Key it is stored under in `wa2.ini`.
    pub fn ini_key(self) -> &'static str {
        match self {
            FontRole::Home => "font_home",
            FontRole::History => "font_history",
            FontRole::Game => "font_game",
        }
    }

    /// Row label on the option page.
    pub fn label(self) -> &'static str {
        match self {
            FontRole::Home => "UI字体",
            FontRole::History => "已读文本字体",
            FontRole::Game => "对话文本字体",
        }
    }

    /// Environment variable that overrides this role alone.
    pub fn env_var(self) -> &'static str {
        match self {
            FontRole::Home => "WA2_FONT_HOME",
            FontRole::History => "WA2_FONT_HISTORY",
            FontRole::Game => "WA2_FONT_GAME",
        }
    }
}

// ---------------------------------------------------------------------------
// font catalogue
// ---------------------------------------------------------------------------

/// One entry of the font list the settings page offers.
#[derive(Clone, Debug)]
pub struct FontProfile {
    /// Stable id, and the only thing wa2.ini stores: `pack:<file name>#<face>`
    /// hashed for a bundled font, `sys:<file name>#<face>` with the high bit set
    /// for a platform one. The same value names the same font on another
    /// machine and never depends on where it sorted here. 0 is the default
    /// entry.
    pub id: i32,
    /// Name shown in the settings page: the family, plus the style when the
    /// face carries an interesting one.
    pub name: String,
    /// Resolved file. None only for the default entry when this machine has
    /// nothing that can draw the script.
    pub path: Option<String>,
    pub face: i64,
    /// Family FreeType reports for the resolved face.
    pub family: String,
    /// True when the file ships with the program (the `fonts/` pack) rather
    /// than being scanned from the platform. Pack entries use their own id
    /// namespace so a config value keeps meaning the same font across machines.
    pub bundled: bool,
}

/// Face index of the Simplified Chinese face inside a Noto CJK collection
/// (JP=0, KR=1, SC=2, TC=3, HK=4). Noto ships one file per script region, so
/// the index is fixed for every weight.
const SC_FACE: i64 = 2;

/// High bit of the id space platform fonts live in. A bundled pack id never
/// carries it, so the two kinds of candidate can never share an id, and the log
/// tells a config value naming a shipped font from one naming a system font
/// (design §1.2).
const SYSTEM_ID_BIT: i32 = 0x4000_0000;

/// Characters a face has to draw before the Chinese catalogue offers it.
///
/// The old probe was `汉 あ A`, which a Japanese or Traditional face passes just
/// as easily as a Simplified one and which said nothing about the characters
/// the scripts actually use - 8 of the 22 candidates on the audit machine
/// passed it and then dropped 2-48 characters of real text. These are common
/// simplified characters from the CK-GAL scripts, plus kana and Latin because
/// names and UI previews mix them in.
const SCRIPT_PROBE: &str = "汉直骨着国门广发台里这说时あA";

/// A fonts/ folder shipped with the program, then one next to the working
/// directory. A packaged build drops its CJK font here so the default never
/// depends on what the distro happened to install.
fn bundled_dirs() -> Vec<String> {
    let mut v = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            v.push(dir.join("fonts").to_string_lossy().into_owned());
        }
    }
    v.push("fonts".to_string());
    v
}

/// One font that ships in the `fonts/` pack.
#[derive(Clone, Debug)]
struct BundledFont {
    path: String,
    /// The manifest's path column: the file name inside the `fonts/` dir. The
    /// bundled id is cut from this, so it stays the same on another machine.
    file_name: String,
    family: String,
    style: String,
    /// Localised name from the manifest's optional display column: the
    /// settings list shows 得意黑 for Smiley Sans instead of the Latin
    /// "Smiley Sans Oblique". Empty when the pack does not name one.
    display: String,
    face: i64,
    /// Share of the script corpus the pack generator measured. Informational:
    /// the runtime only requires the probe characters.
    coverage: f32,
    /// True when the manifest's size and SHA-256 were checked against the file.
    verified: bool,
}

impl BundledFont {
    /// Name in the settings list: the pack's own localised display name when it
    /// has one (得意黑), otherwise the family plus the style when it is not the
    /// plain one (the pack ships Medium and Regular of one family).
    fn display_name(&self) -> String {
        if !self.display.is_empty() {
            return self.display.clone();
        }
        if self.style.is_empty() || self.style == "Regular" {
            self.family.clone()
        } else {
            format!("{} {}", self.family, self.style)
        }
    }

    /// Id of this pack entry: `pack:<file name>#<face>`, so it names the same
    /// font on every machine that ships the same pack.
    fn id(&self) -> i32 {
        pack_id(&self.file_name, self.face)
    }

    fn profile(&self) -> FontProfile {
        FontProfile {
            id: self.id(),
            name: self.display_name(),
            path: Some(self.path.clone()),
            face: self.face,
            family: self.family.clone(),
            bundled: true,
        }
    }
}

/// FNV-1a over a stable key, kept to 31 bits and never 0 (0 is the default
/// profile).
fn fnv_id(key: &str) -> i32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in key.as_bytes() {
        h ^= *b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    match (h & 0x7fff_ffff) as i32 {
        0 => 1,
        n => n,
    }
}

/// Id of a bundled font. Namespaced with `pack:` so a system font of the same
/// file name is a different candidate, and a config written on a machine with
/// the pack keeps naming the same font anywhere. The system bit is cleared so
/// a bundled id can never look like a platform one.
fn pack_id(file_name: &str, face: i64) -> i32 {
    let id = fnv_id(&format!("pack:{}#{}", file_name, face)) & !SYSTEM_ID_BIT;
    if id == 0 {
        1
    } else {
        id
    }
}

/// Id of a platform font: a dynamic candidate, namespaced with `sys:` and with
/// the high bit set so it can never collide with a packed one.
fn system_id(file_name: &str, face: i64) -> i32 {
    fnv_id(&format!("sys:{}#{}", file_name, face)) | SYSTEM_ID_BIT
}

fn file_name_of(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// Parse and validate the pack manifest in the first bundled dir that has one.
/// Returns the dir, the recorded pack version and the entries that verified.
///
/// None means there is no `manifest.tsv` anywhere: an older source tree, or a
/// package stripped of it. That is tolerated - the well-known file names are
/// still tried, with only their existence checked, and the log says so. A
/// manifest that exists but lists a broken entry keeps going with the rest: one
/// corrupt bundled font must not stop the game from starting.
fn load_font_pack(dirs: &[String]) -> Option<(String, i32, Vec<BundledFont>)> {
    for dir in dirs {
        let manifest = std::path::Path::new(dir).join("manifest.tsv");
        let Ok(text) = std::fs::read_to_string(&manifest) else { continue };
        let mut version = FONT_PACK_VERSION;
        let mut entries = Vec::new();
        let mut rows = 0usize;
        for line in text.lines() {
            let line = line.trim_end();
            if line.is_empty() {
                continue;
            }
            if let Some(rest) = line.strip_prefix('#') {
                if let Some(v) = rest.trim().strip_prefix("wa2 font pack version ") {
                    version = v.trim().parse().unwrap_or(FONT_PACK_VERSION);
                }
                continue;
            }
            rows += 1;
            // path / family / face / coverage / size / sha256
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 6 {
                eprintln!("font: WARN manifest line has {} columns, want 6: {}", f.len(), line);
                continue;
            }
            let (rel, family, sha) = (f[0].trim(), f[1].trim(), f[5].trim());
            // Optional seventh column: the localised name for the settings list.
            let display = f.get(6).map(|d| d.trim().to_string()).unwrap_or_default();
            let Ok(face) = f[2].trim().parse::<i64>() else {
                eprintln!("font: WARN manifest face is not a number: {}", line);
                continue;
            };
            let coverage: f32 = f[3].trim().parse().unwrap_or(0.0);
            let Ok(size) = f[4].trim().parse::<u64>() else {
                eprintln!("font: WARN manifest size is not a number: {}", line);
                continue;
            };
            let path = std::path::Path::new(dir).join(rel).to_string_lossy().into_owned();
            match verify_bundled_font(&path, family, face, size, sha) {
                Ok((family, style)) => {
                    let shown = if display.is_empty() { family.clone() } else { display.clone() };
                    eprintln!(
                        "font: bundled {} face {} '{}'({}) coverage {:.1}%",
                        rel, face, shown, family, coverage
                    );
                    entries.push(BundledFont {
                        path,
                        file_name: rel.to_string(),
                        family,
                        style,
                        display,
                        face,
                        coverage,
                        verified: true,
                    })
                }
                Err(reason) => eprintln!("font: WARN bundled font {} rejected: {}", rel, reason),
            }
        }
        if version != FONT_PACK_VERSION {
            eprintln!(
                "font: WARN font pack version {} does not match the program's {}; saved font ids may have moved",
                version, FONT_PACK_VERSION
            );
        }
        eprintln!(
            "font: font pack version {} in {} ({} of {} font(s) verified)",
            version,
            dir,
            entries.len(),
            rows
        );
        return Some((dir.clone(), version, entries));
    }
    None
}

/// Check one manifest row against the file on disk: size, SHA-256, face and the
/// family FreeType reports. Returns the verified family and style.
fn verify_bundled_font(
    path: &str,
    family: &str,
    face: i64,
    size: u64,
    sha: &str,
) -> Result<(String, String), String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("missing file ({})", e))?;
    if meta.len() != size {
        return Err(format!("size {} does not match the manifest's {}", meta.len(), size));
    }
    if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("manifest sha256 is not 64 hex digits".to_string());
    }
    let digest = crate::sha256::hex_digest_file(std::path::Path::new(path))
        .map_err(|e| format!("cannot hash ({})", e))?;
    if !digest.eq_ignore_ascii_case(sha) {
        return Err(format!(
            "sha256 {} does not match the manifest's {}",
            &digest[..16],
            &sha[..16]
        ));
    }
    let mut ft = ft2::Ft::open(path, 24, face).ok_or_else(|| format!("cannot open face {}", face))?;
    // FreeType reports the face it actually opened, which catches a manifest
    // that names a face order the file no longer has.
    if ft.face_index() != face {
        return Err(format!("file opened on face {} instead of {}", ft.face_index(), face));
    }
    let actual_family = ft.family().unwrap_or_default();
    if !actual_family.eq_ignore_ascii_case(family) {
        return Err(format!(
            "family '{}' does not match the manifest's '{}'",
            actual_family, family
        ));
    }
    // Even a manifest that matches the file is not enough if it lists another
    // region's face: the glyph shapes would be wrong for the Chinese text.
    if is_other_region(&actual_family) {
        return Err(format!("'{}' names another CJK region", actual_family));
    }
    if !script_ok(&mut ft) {
        return Err(format!("{} cannot draw the script probe", actual_family));
    }
    Ok((actual_family, ft.style().unwrap_or_default()))
}

/// A fonts/ dir without a manifest still works, with only the file's presence
/// checked: the pack is a local file next to the program, not a download. The
/// known file names are enumerated (Medium, Regular, then the Smiley Sans
/// display face).
fn enumerate_bundled_fonts(dirs: &[String]) -> Vec<BundledFont> {
    let mut out: Vec<BundledFont> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for dir in dirs {
        for (file, face) in [
            ("NotoSansCJK-Medium.ttc", SC_FACE),
            ("NotoSansCJK-Regular.ttc", SC_FACE),
            ("SmileySans-Oblique.ttf", 0),
        ] {
            if !seen.insert(file.to_string()) {
                continue;
            }
            let path = std::path::Path::new(dir).join(file).to_string_lossy().into_owned();
            let Some(mut ft) = ft2::Ft::open(&path, 24, face) else { continue };
            if !script_ok(&mut ft) {
                continue;
            }
            out.push(BundledFont {
                path,
                file_name: file.to_string(),
                family: ft.family().unwrap_or_default(),
                style: ft.style().unwrap_or_default(),
                display: String::new(),
                face,
                coverage: 0.0,
                verified: false,
            });
        }
    }
    if !out.is_empty() {
        eprintln!(
            "font: no fonts/manifest.tsv; using {} bundled font(s) without hash verification",
            out.len()
        );
    }
    out
}

/// Families whose region suffix says they are Simplified Chinese.
fn is_sc_family(family: &str) -> bool {
    let f = family.trim();
    f.ends_with(" SC") || f.ends_with(" Hans") || f.ends_with(" CN")
}

/// Families that name another CJK region. Their Unicode coverage looks complete
/// - that is why the old probe let them through - but the glyph shapes are the
/// other region's, so they must not be offered as Chinese fonts.
fn is_other_region(family: &str) -> bool {
    let f = family.trim();
    [" JP", " KR", " TC", " TW", " HK"].iter().any(|s| f.ends_with(s))
}

/// Noto Sans CJK Medium, the weight the dialog layout was tuned on.
fn medium_paths() -> &'static [(&'static str, i64)] {
    if cfg!(target_os = "windows") {
        &[("C:/Windows/Fonts/msyhbd.ttc", 0), ("C:/Windows/Fonts/msyh.ttc", 0)]
    } else if cfg!(target_os = "macos") {
        // PingFang is what the design language assumes, but macOS 15+ no
        // longer ships it as a file in every install - it can arrive later as
        // a downloadable asset. Hiragino Sans GB is the CJK face that has been
        // in /System/Library/Fonts for every release, so it is the safety net.
        &[
            ("/System/Library/Fonts/PingFang.ttc", 0),
            ("/System/Library/Fonts/Hiragino Sans GB.ttc", 0),
            ("/System/Library/Fonts/Supplemental/Songti.ttc", 0),
            ("/Library/Fonts/NotoSansCJK-Medium.ttc", SC_FACE),
        ]
    } else {
        &[
            ("/usr/share/fonts/opentype/noto/NotoSansCJK-Medium.ttc", SC_FACE),
            ("/usr/share/fonts/truetype/noto/NotoSansCJK-Medium.ttc", SC_FACE),
            ("/usr/local/share/fonts/noto/NotoSansCJK-Medium.ttc", SC_FACE),
        ]
    }
}

/// A face that cannot draw the script is not offered: it would put tofu, or
/// Japanese glyph shapes, in the middle of the Chinese lines.
fn script_ok(ft: &mut ft2::Ft) -> bool {
    SCRIPT_PROBE.chars().all(|ch| ft.glyph(ch).is_some())
}

/// How many faces of a file to try. A ttc holds one per script region and the
/// Chinese one is rarely face 0.
fn faces_in(path: &str) -> i64 {
    let low = path.to_ascii_lowercase();
    if !low.ends_with(".ttc") && !low.ends_with(".otc") {
        return 1;
    }
    match ft2::Ft::open(path, 24, 0) {
        Some(ft) => ft.num_faces().clamp(1, 8),
        None => 1,
    }
}

/// Where the platform keeps its fonts.
fn font_search_dirs() -> Vec<String> {
    let mut v = Vec::new();
    if cfg!(target_os = "windows") {
        v.push("C:/Windows/Fonts".to_string());
    } else if cfg!(target_os = "macos") {
        v.push("/System/Library/Fonts".to_string());
        v.push("/System/Library/Fonts/Supplemental".to_string());
        v.push("/Library/Fonts".to_string());
    } else {
        v.push("/usr/share/fonts".to_string());
        v.push("/usr/local/share/fonts".to_string());
        if let Ok(home) = std::env::var("HOME") {
            v.push(format!("{}/.local/share/fonts", home));
            v.push(format!("{}/.fonts", home));
        }
    }
    v
}

fn collect_font_files(out: &mut Vec<String>, dir: &str, depth: usize) {
    if depth > 6 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_font_files(out, &p.to_string_lossy(), depth + 1);
        } else {
            let low = p.to_string_lossy().to_ascii_lowercase();
            if low.ends_with(".ttf") || low.ends_with(".otf") || low.ends_with(".ttc") || low.ends_with(".otc") {
                out.push(p.to_string_lossy().into_owned());
            }
        }
    }
}

fn font_files() -> Vec<String> {
    let mut files = Vec::new();
    for d in font_search_dirs() {
        collect_font_files(&mut files, &d, 0);
    }
    files.sort();
    files.dedup();
    files
}
/// Every platform font that can actually draw the script, one entry per family
/// and style. Opening a thousand-odd files is slow, so the result is kept in
/// data/cache/fonts.tsv between runs.
///
/// A collection is where this gets interesting: NotoSansCJK-Regular.ttc holds
/// five script regions and face 0 is Japanese, which has plenty of kanji and
/// would happily pass a coverage probe - and then draw Japanese glyph shapes
/// for hanzi like 骨, 直 and 着. So the Simplified Chinese face is looked for by
/// name, Japanese/Korean/Traditional faces are kept out of the Chinese list
/// entirely, and only a file whose family does not name another region may fall
/// back to its first covered face.
fn scan_system_fonts() -> Vec<FontProfile> {
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for path in font_files() {
        // LastResort and CJKSymbolsFallback are the "I have no glyph for this"
        // faces: they answer every probe with something, so either of them
        // would win the default slot on a machine whose real CJK faces were
        // filtered out - and then draw hollow boxes for the whole script.
        let lower = path.to_ascii_lowercase();
        if lower.contains("lastresort") || lower.contains("cjksymbols") {
            continue;
        }
        let mut fallback: Option<FontProfile> = None;
        for face in 0..faces_in(&path) {
            let Some(mut ft) = ft2::Ft::open(&path, 24, face) else { break };
            if !script_ok(&mut ft) {
                continue;
            }
            let family = ft.family().unwrap_or_default();
            // A Japanese/Traditional face draws every character of the probe
            // too - same code points, different shapes. The family is the only
            // thing that tells them apart, so it decides what is offered.
            if is_other_region(&family) {
                continue;
            }
            let style = ft.style().unwrap_or_default();
            let name = if style.is_empty() || style == "Regular" {
                family.clone()
            } else {
                format!("{} {}", family, style)
            };
            let prof = FontProfile {
                id: system_id(&file_name_of(&path), face),
                name,
                path: Some(path.clone()),
                face,
                family: family.clone(),
                bundled: false,
            };
            // The Simplified Chinese face of a collection: Noto Sans CJK SC,
            // SourceHanSansSC, PingFang SC - the families name their region.
            if is_sc_family(&family) {
                if seen.insert(format!("{}|{}", family, style)) {
                    out.push(prof);
                }
                fallback = None;
                break;
            }
            if fallback.is_none() {
                fallback = Some(prof);
            }
        }
        if let Some(p) = fallback {
            if seen.insert(format!("{}|{}", p.family, p.name)) {
                out.push(p);
            }
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

fn font_cache_path() -> Option<std::path::PathBuf> {
    Some(crate::data::Data::resolve()?.root.join("cache").join("fonts.tsv"))
}

/// `(size, mtime in whole seconds)` of a file, for the cache freshness check.
fn file_stamp(path: &std::path::Path) -> Option<(u64, u64)> {
    let md = std::fs::metadata(path).ok()?;
    let mtime = md.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    Some((md.len(), mtime))
}

/// The cached scan, if it was cut from the same files. The file count and each
/// entry's size+mtime are checked, so replacing a font in place invalidates the
/// cache even when no file was added or removed (the old cache only counted
/// files). A different format - an older cache without those columns - simply
/// misses, and the scan runs again.
fn load_font_cache_at(path: &std::path::Path, count: usize) -> Option<Vec<FontProfile>> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    let recorded: usize = lines.next()?.trim().strip_prefix("# files=")?.parse().ok()?;
    if recorded != count {
        return None;
    }
    let mut out = Vec::new();
    for line in lines {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        // id / name / path / face / family / size / mtime
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 7 {
            return None;
        }
        let (size, mtime) = file_stamp(std::path::Path::new(f[2]))?;
        if size != f[5].parse::<u64>().ok()? || mtime != f[6].parse::<u64>().ok()? {
            return None;
        }
        out.push(FontProfile {
            id: f[0].parse().ok()?,
            name: f[1].to_string(),
            path: Some(f[2].to_string()),
            face: f[3].parse().ok()?,
            family: f[4].to_string(),
            bundled: false,
        });
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn load_font_cache(count: usize) -> Option<Vec<FontProfile>> {
    load_font_cache_at(&font_cache_path()?, count)
}

fn save_font_cache(count: usize, list: &[FontProfile]) {
    let Some(path) = font_cache_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut out = format!("# files={}\n", count);
    for p in list {
        let Some(font_path) = p.path.as_ref() else { continue };
        let Some((size, mtime)) = file_stamp(std::path::Path::new(font_path)) else { continue };
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            p.id, p.name, font_path, p.face, p.family, size, mtime
        ));
    }
    let tmp = path.with_extension("tsv.tmp");
    if std::fs::write(&tmp, out).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// The whole catalogue: the default entry (id 0) first, then the fonts shipped
/// in `fonts/`, then the platform's own. The pack is checked against its
/// manifest before it may be used; a machine without the pack (or with a
/// damaged one) falls through to the platform and finally to the scan.
fn resolve_catalog() -> Vec<FontProfile> {
    let dirs = bundled_dirs();
    let pack = load_font_pack(&dirs);
    let bundled = match pack.as_ref() {
        Some((_, _, list)) => list.clone(),
        None => {
            let list = enumerate_bundled_fonts(&dirs);
            if list.is_empty() {
                eprintln!(
                    "font: font pack version {}; no fonts/ pack on this machine, using platform fonts",
                    FONT_PACK_VERSION
                );
            }
            list
        }
    };

    let files = font_files();
    let discovered = match load_font_cache(files.len()) {
        Some(v) => v,
        None => {
            let v = scan_system_fonts();
            save_font_cache(files.len(), &v);
            v
        }
    };

    // The default entry tries the pack first, then the platform's own CJK
    // paths, and only then the scan - a machine whose only CJK font sits in a
    // non-standard directory used to end up with no default at all (design
    // §1.1). The log says which of the three it ended up with.
    let mut def = FontProfile {
        id: 0,
        name: "默认".to_string(),
        path: None,
        face: SC_FACE,
        family: String::new(),
        bundled: false,
    };
    let mut origin = "";
    if let Some(b) = bundled.first() {
        def.path = Some(b.path.clone());
        def.face = b.face;
        def.family = b.family.clone();
        def.bundled = true;
        origin = "bundled";
    }
    if def.path.is_none() {
        for &(path, face) in medium_paths() {
            if let Some(mut ft) = ft2::Ft::open(path, 24, face) {
                if script_ok(&mut ft) {
                    def.path = Some(path.to_string());
                    def.face = face;
                    def.family = ft.family().unwrap_or_default();
                    origin = "platform";
                    break;
                }
            }
        }
    }
    if def.path.is_none() {
        for p in &discovered {
            let Some(path) = p.path.as_ref() else { continue };
            let Some(mut ft) = ft2::Ft::open(path, 24, p.face) else { continue };
            if !script_ok(&mut ft) {
                continue;
            }
            def.path = Some(path.clone());
            def.face = p.face;
            def.family = ft.family().unwrap_or_default();
            origin = "scanned";
            eprintln!(
                "font: WARN no bundled or platform default font opened; falling back to the scanned font '{}'",
                def.family
            );
            break;
        }
    }
    match def.path.as_ref() {
        Some(path) => {
            eprintln!("font: default -> {} (face {}) {} [{}]", path, def.face, def.family, origin)
        }
        None => eprintln!(
            "font: ERROR - nothing here can draw Chinese; install a CJK font or drop one in {}",
            dirs.first().cloned().unwrap_or_else(|| "fonts".to_string())
        ),
    }

    // Pack entries come before the system ones (design §6). A bundle entry and
    // a system entry for the same family/style are the same font: keep the
    // packaged one and drop the copy the scan found.
    let mut out = Vec::with_capacity(discovered.len() + bundled.len() + 1);
    out.push(def);
    let mut ids: HashSet<i32> = HashSet::new();
    ids.insert(0);
    let mut names: HashSet<String> = HashSet::new();
    let mut bundled_count = 0;
    for b in &bundled {
        if !ids.insert(b.id()) {
            continue;
        }
        names.insert(b.display_name().to_lowercase());
        out.push(b.profile());
        bundled_count += 1;
    }
    let mut system_count = 0;
    for p in discovered {
        if names.contains(&p.name.to_lowercase()) {
            continue;
        }
        if !ids.insert(p.id) {
            continue;
        }
        out.push(p);
        system_count += 1;
    }
    eprintln!(
        "font: {} fonts in the catalogue ({} bundled, {} platform); id 0 is the default",
        out.len() - 1,
        bundled_count,
        system_count
    );
    out
}

static CATALOG: std::sync::OnceLock<Vec<FontProfile>> = std::sync::OnceLock::new();

/// The catalogue: the default entry first, then everything the machine has
/// that can draw the script.
pub fn font_catalog() -> &'static [FontProfile] {
    CATALOG.get_or_init(resolve_catalog).as_slice()
}

pub fn font_profile(id: i32) -> Option<&'static FontProfile> {
    font_catalog().iter().find(|p| p.id == id)
}

/// Position of an id in the catalogue, for the arrow keys.
pub fn font_profile_index(id: i32) -> usize {
    font_catalog().iter().position(|p| p.id == id).unwrap_or(0)
}

/// An id that names a font this machine still has; anything else (a stale
/// config, a hand-edited value, an uninstalled font) falls back to the
/// default rather than to a file that is not there.
pub fn clamp_profile_id(id: i32) -> i32 {
    match font_profile(id) {
        Some(p) if p.path.is_some() => id,
        _ => match migrate_legacy_id(id) {
            Some(new) => {
                eprintln!("font: profile id {id} is from the pre-pack scheme; mapping it to {new}");
                new
            }
            None => 0,
        },
    }
}

/// The id scheme this program used before the font pack: FNV-1a of the bare
/// file name and face, with no namespace. Configs written by an older build
/// still carry those, so an unknown id is looked up in the old namespace before
/// the setting is dropped to the default - the settings file then saves the
/// migrated id on the next write.
fn migrate_legacy_id(id: i32) -> Option<i32> {
    if id <= 0 {
        return None;
    }
    for p in font_catalog() {
        if p.id == 0 {
            continue;
        }
        let Some(path) = p.path.as_ref() else { continue };
        if fnv_id(&format!("{}#{}", file_name_of(path), p.face)) == id {
            return Some(p.id);
        }
    }
    None
}

/// Point the three roles at the ids the player saved. Returns true when
/// anything changed, which is when the cached instances have to go.
pub fn set_role_ids(ids: [i32; 3]) -> bool {
    *ROLE_REQUESTED.lock().unwrap_or_else(|e| e.into_inner()) = ids;
    let next = [clamp_profile_id(ids[0]), clamp_profile_id(ids[1]), clamp_profile_id(ids[2])];
    let mut cur = ROLE_IDS.lock().unwrap_or_else(|e| e.into_inner());
    if *cur == next {
        return false;
    }
    *cur = next;
    drop(cur);
    ROLE_GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // The recorded reasons described the font that was in use before this
    // change. Keeping them would let the panel and the log report a fallback
    // that no longer applies; the next open records whatever is true then.
    let mut reasons = ROLE_FALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    for reason in reasons.iter_mut() {
        *reason = None;
    }
    true
}

pub fn role_ids() -> [i32; 3] {
    *ROLE_IDS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Changes every time the three ids change. Screens that rasterize text into
/// a long-lived cache include it in their key, so a font change reaches them
/// on the next frame instead of after a restart.
pub fn role_generation() -> u64 {
    ROLE_GEN.load(std::sync::atomic::Ordering::Relaxed)
}

/// The id the caller asked for, before clamping to a font this machine has.
pub fn role_requested(role: FontRole) -> i32 {
    ROLE_REQUESTED.lock().unwrap_or_else(|e| e.into_inner())[role.index()]
}

/// Why the role is not drawing with the requested font (missing file, no
/// Chinese coverage, failed to open), for the log and the settings panel.
pub fn role_fallback_reason(role: FontRole) -> Option<String> {
    ROLE_FALLBACK.lock().unwrap_or_else(|e| e.into_inner())[role.index()].clone()
}

/// Record (or clear) the fallback reason for a role. The font-opening code and
/// the panel both read this.
pub fn set_role_fallback_reason(role: FontRole, reason: Option<String>) {
    let mut all = ROLE_FALLBACK.lock().unwrap_or_else(|e| e.into_inner());
    all[role.index()] = reason;
}

/// Font-only「恢复默认」: the three roles go back to profile 0 without touching
/// any other setting. Bumps the generation so every cached screen rebuilds.
pub fn reset_role_ids() -> bool {
    set_role_ids([0, 0, 0])
}

/// True when this machine can actually draw Chinese with some usable font.
/// The GUI refuses to start in Chinese mode when this is false instead of
/// painting a screen full of empty text.
pub fn chinese_font_available() -> bool {
    let cat = font_catalog();
    cat.iter().any(|p| {
        p.path
            .as_ref()
            .and_then(|path| VectorFont::open_face(path, 24, p.face))
            .map(|vf| script_covers(&vf))
            .unwrap_or(false)
    })
}

/// True when the open font has a face glyph for every probe character. A role
/// must not use a font that fails this: the Chinese lines would have gaps in
/// them, or - worse - the shapes of another CJK region.
fn script_covers(vf: &VectorFont) -> bool {
    SCRIPT_PROBE.chars().all(|ch| vf.has_glyph(ch))
}

/// The id a role is using, already clamped to something loadable.
pub fn role_id(role: FontRole) -> i32 {
    role_ids()[role.index()]
}

/// True when the role asked for a font this machine cannot provide and the
/// default is standing in for it. The settings panel says so out loud instead
/// of letting the player believe the choice took effect.
pub fn role_fallback(role: FontRole) -> bool {
    // An override that is in force is what the player asked for, even when the
    // id in the file differs; an empty variable counts as unset (F-21).
    if env_font_spec(role).is_some() {
        return false;
    }
    // The saved id is normalized on load, so a requested value that differs
    // from the effective one means this machine does not have it.
    if role_requested(role) != role_id(role) {
        return true;
    }
    role_fallback_reason(role).is_some()
        || match font_profile(role_id(role)) {
            Some(p) => p.path.is_none(),
            None => true,
        }
}

/// The font file and face of the default profile (id 0). When this machine has
/// nothing usable at all, the design's historical path is returned so opening
/// fails with a clear message instead of an empty path.
fn default_path_face() -> (String, i64) {
    match font_profile(0) {
        Some(p) if p.path.is_some() => (p.path.clone().unwrap_or_default(), p.face),
        _ => (DEFAULT_FONT.to_string(), DEFAULT_FACE),
    }
}

/// The path override in force for a role: the role's own variable first, then
/// the global WA2_FONT. An empty value counts as "not set" - `WA2_FONT_GAME=`
/// is how a shell clears a variable for one run, and it must not shadow the
/// global override the way `std::env::var` alone would.
fn env_font_spec(role: FontRole) -> Option<String> {
    for key in [role.env_var(), "WA2_FONT"] {
        if let Ok(value) = std::env::var(key) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// WA2_FONT_FACE, the face companion of a path override. Only consulted when a
/// path spec is in force - an id from wa2.ini already names its own face, and
/// letting this variable move it would make the panel lie about what the
/// selected profile means (F-21).
fn env_face_override() -> Option<i64> {
    std::env::var("WA2_FONT_FACE")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
}

/// The face a bare path override opens on, decided once per file.
///
/// A spec without `#index` used to fall through to `DEFAULT_FACE = 2`, which is
/// wrong for every single-face .ttf/.otf and for collections with fewer faces.
/// That is exactly the form the design's §4.2 examples use, and it made
/// `WA2_FONT_HOME=/path/to/font.ttf` fail with no log at all. The faces are
/// probed instead: the Simplified Chinese one wins, then the first face that
/// can draw the script probe, then the first face that at least opens (the
/// caller reports the missing coverage, which is a better message than a face
/// index that does not exist).
fn probe_face(path: &str) -> i64 {
    static PROBED: std::sync::OnceLock<std::sync::Mutex<HashMap<String, i64>>> =
        std::sync::OnceLock::new();
    let cache = PROBED.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    {
        let known = cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(face) = known.get(path) {
            return *face;
        }
    }
    let faces = faces_in(path).clamp(1, 8);
    let mut first_open: Option<i64> = None;
    let mut first_covered: Option<i64> = None;
    let mut picked = None;
    for face in 0..faces {
        let Some(mut ft) = ft2::Ft::open(path, 24, face) else { continue };
        if first_open.is_none() {
            first_open = Some(face);
        }
        if !script_ok(&mut ft) {
            continue;
        }
        if is_sc_family(&ft.family().unwrap_or_default()) {
            picked = Some(face);
            break;
        }
        if first_covered.is_none() {
            first_covered = Some(face);
        }
    }
    let face = picked.or(first_covered).or(first_open).unwrap_or(0);
    cache.lock().unwrap_or_else(|e| e.into_inner()).insert(path.to_string(), face);
    face
}

/// What the player asked for. WA2_FONT / WA2_FONT_FACE / WA2_FONT_BOLD /
/// WA2_FONT_GAMMA (or the legacy WA2_FONT_GAIN) / WA2_FONT_EDGE /
/// WA2_FONT_EDGE_ALPHA / WA2_FONT_HINT are the stand-in for the font settings
/// page, which writes the same values per role.
///
/// A role's own WA2_FONT_* wins over the global WA2_FONT; both beat the profile
/// id from `wa2.ini`, which beats the default profile. For a path spec, an
/// explicit `#face` beats WA2_FONT_FACE, which beats probing the file for its
/// best face. WA2_FONT_FACE is ignored without a path spec (F-21).
pub fn font_choice_for(role: FontRole) -> FontChoice {
    match env_font_spec(role) {
        Some(spec) => {
            let (path, spec_face) = split_font_spec(&spec);
            let face = spec_face
                .or_else(env_face_override)
                .unwrap_or_else(|| probe_face(&path));
            env_choice(path, face)
        }
        None => {
            let id = role_id(role);
            match font_profile(id).and_then(|p| p.path.clone()) {
                Some(path) => {
                    let face = font_profile(id).map(|p| p.face).unwrap_or(DEFAULT_FACE);
                    env_choice(path, face)
                }
                None => {
                    let (path, face) = default_path_face();
                    env_choice(path, face)
                }
            }
        }
    }
}

/// The knobs that shape the ink, shared by every role. The settings page owns
/// the dialogue style; WA2_FONT_BOLD and the ink/edge/hint variables stay a
/// debug override for every role.
fn env_choice(path: String, face: i64) -> FontChoice {
    let bold = std::env::var("WA2_FONT_BOLD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    // The gamma curve is the default; the legacy gain only comes back when it
    // is asked for explicitly, so the two never mix in one run.
    let gamma = std::env::var("WA2_FONT_GAMMA")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .or(Some(DEFAULT_GAMMA));
    let gain = std::env::var("WA2_FONT_GAIN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_GAIN);
    let gamma = if std::env::var("WA2_FONT_GAIN").is_ok() { None } else { gamma };
    let edge_px = std::env::var("WA2_FONT_EDGE")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok());
    let edge_alpha = std::env::var("WA2_FONT_EDGE_ALPHA")
        .ok()
        .and_then(|v| v.trim().parse::<u8>().ok())
        .unwrap_or(DEFAULT_EDGE_ALPHA);
    let hint = std::env::var("WA2_FONT_HINT")
        .ok()
        .and_then(|v| HintMode::parse(&v))
        .unwrap_or(HintMode::Light);
    FontChoice { path, face, bold, gamma, gain, edge_px, edge_alpha, hint }
}

/// Point a font at a choice. Both knobs drop the glyph cache, so this only does
/// work when something actually changed.
pub fn apply_choice(vf: &VectorFont, choice: &FontChoice) {
    vf.set_bold(choice.bold);
    match choice.gamma {
        Some(g) => vf.set_ink_gamma(g),
        None => vf.set_ink_gain(choice.gain),
    }
    vf.set_edge_px(choice.edge_px);
    vf.set_edge_alpha(choice.edge_alpha);
    vf.set_hint(choice.hint);
}

/// The pack's other weight, used as the per-glyph backup for the default
/// profile: if the shipped Medium is missing a character, the Regular may still
/// have it, and both are the same family so a line never changes style. A
/// machine without the pack simply has no backup for its default font.
fn pack_sibling_fallback(exclude_path: &str) -> Option<(String, i64)> {
    font_catalog()
        .iter()
        .find(|p| p.bundled && p.id != 0 && p.path.as_deref() != Some(exclude_path))
        .and_then(|p| p.path.clone().map(|path| (path, p.face)))
}

/// Give a font its per-glyph backup. The backup is only asked for characters
/// the primary face has no glyph for, so the body of a line never switches
/// font: the default profile (the shipped pack) backs every player-chosen font,
/// and the default profile itself is backed by the pack's other weight.
fn install_fallback(vf: &VectorFont, chosen_path: &str) {
    let default = font_profile(0).and_then(|p| p.path.as_ref().map(|path| (path.clone(), p.face)));
    let target = match default {
        Some((path, face)) if path != chosen_path => Some((path, face)),
        _ => pack_sibling_fallback(chosen_path),
    };
    if let Some((path, face)) = target {
        if !vf.set_fallback(&path, face) {
            eprintln!("font: WARN cannot open the fallback font {} (face {})", path, face);
        }
    }
}

/// Open a font for one role, owned by the caller.
///
/// The runtime's dialogue font and the history screen hold theirs outright;
/// the shared UI path goes through [`with_role_font`].
///
/// Unlike a bare [`VectorFont::open_face`], this is where the font settings'
/// rules live: the requested file has to open *and* carry the script probe, a
/// failure is recorded with its reason and falls back to the default profile
/// (design §6), every font gets a per-glyph backup, and the log says which
/// file, family, face, pack version and backup the role actually ended up with.
pub fn open_role_font(role: FontRole, size: u32) -> Option<VectorFont> {
    let size = size.max(8);
    let choice = font_choice_for(role);
    let requested = role_requested(role);
    let mut used_path = choice.path.clone();
    let mut used_face = choice.face;
    let mut failure = None;
    let mut vf = match VectorFont::open_face(&choice.path, size, choice.face) {
        Some(vf) => {
            apply_choice(&vf, &choice);
            if script_covers(&vf) {
                Some(vf)
            } else {
                failure = Some(format!(
                    "{} (face {}) has no glyphs for the Chinese script",
                    choice.path, choice.face
                ));
                None
            }
        }
        None => {
            // Say whether the file is missing or merely unopenable: a wrong
            // path and a bad face index are different mistakes for the player
            // reading the log, and the default profile is usually in its place
            // by the time they look.
            failure = Some(if !std::path::Path::new(&choice.path).is_file() {
                format!("{} does not exist", choice.path)
            } else {
                format!("cannot open {} (face {})", choice.path, choice.face)
            });
            None
        }
    };
    let mut reason = None;
    if vf.is_none() {
        let why = failure.unwrap_or_else(|| "the requested font is unavailable".to_string());
        eprintln!(
            "font: role={} profile={} requested={} path={} face={} FAILED: {}",
            role.tag(),
            role_id(role),
            requested,
            choice.path,
            choice.face,
            why
        );
        reason = Some(why);
        // The player still has to get readable text: the default profile is
        // the documented fallback and the settings panel shows the reason.
        let (dpath, dface) = default_path_face();
        if dpath != choice.path || dface != choice.face {
            if let Some(fallback) = VectorFont::open_face(&dpath, size, dface) {
                apply_choice(&fallback, &env_choice(dpath.clone(), dface));
                if script_covers(&fallback) {
                    eprintln!("font: role={} falls back to the default font", role.tag());
                    used_path = dpath;
                    used_face = dface;
                    vf = Some(fallback);
                }
            }
        }
    }
    set_role_fallback_reason(role, reason);
    let vf = vf?;
    install_fallback(&vf, &used_path);
    // Which file and which face of it actually opened: a .ttc defaults to a
    // face chosen by the probe, and the file name alone never says which one
    // the role ended up with.
    eprintln!(
        "font role={} profile={} requested={} path={} family={} face={} pack={}{}",
        role.tag(),
        role_id(role),
        requested,
        used_path,
        vf.family(),
        used_face,
        FONT_PACK_VERSION,
        match vf.fallback_family() {
            Some(family) => format!(" fallback={}", family),
            None => " fallback=none".to_string(),
        }
    );
    Some(vf)
}

/// Open one catalogue entry by id, for the panel's preview line.
///
/// The preview shows the entry's own face, so it does not fall back to another
/// font for missing characters - but it does get the per-glyph backup, for the
/// same reason every other drawn line does.
pub fn open_profile_font(id: i32, size: u32) -> Option<VectorFont> {
    let p = font_profile(id)?;
    let path = p.path.as_deref()?.to_string();
    let vf = VectorFont::open_face(&path, size.max(8), p.face)?;
    apply_choice(&vf, &env_choice(path.clone(), p.face));
    install_fallback(&vf, &path);
    Some(vf)
}

/// Run f with one role's font at size pixels.
///
/// Screens that only need a caption or two borrow the font here instead of
/// threading one through every UI function. The instance is opened on first
/// use - or re-opened after the player changed that role's font - and is then
/// shared by every call for that role.
/// None means FreeType or the font file is unavailable, and the caller has to
/// do without the text. A failed open is remembered for the current generation,
/// so a font that is not there does not cost a FreeType init on every frame.
pub fn with_role_font<R>(role: FontRole, size: u32, f: impl FnOnce(&VectorFont) -> R) -> Option<R> {
    ROLE_FONTS.with(|cell| {
        let gen = ROLE_GEN.load(std::sync::atomic::Ordering::Relaxed);
        let mut slots = cell.borrow_mut();
        let slot = &mut slots[role.index()];
        if slot.as_ref().map(|s| s.gen) != Some(gen) {
            let failed = ROLE_FAILED_GEN.with(|g| g.borrow()[role.index()] == Some(gen));
            if failed {
                return None;
            }
            match open_role_font(role, size.max(8)) {
                Some(vf) => {
                    *slot = Some(RoleFont { gen, vf });
                    ROLE_FAILED_GEN.with(|g| g.borrow_mut()[role.index()] = None);
                }
                None => {
                    *slot = None;
                    ROLE_FAILED_GEN.with(|g| g.borrow_mut()[role.index()] = Some(gen));
                    return None;
                }
            }
        }
        let vf = &slot.as_ref()?.vf;
        if !vf.set_size(size) {
            return None;
        }
        apply_choice(vf, &font_choice_for(role));
        Some(f(vf))
    })
}

/// Run f with the menu font at size pixels.
///
/// Every screen that is not the dialogue or the history draws from the Home
/// role, so the player's menu choice covers the title screen, the option
/// screens, save/load, the timeline and the music mode in one go.
pub fn with_ui_font<R>(size: u32, f: impl FnOnce(&VectorFont) -> R) -> Option<R> {
    with_role_font(FontRole::Home, size, f)
}

/// The bitmap font carried a one-pixel edge at its ~26px dialog size; keep that
/// weight as the text gets bigger, capped so a huge window does not go chunky.
fn outline_for(size: u32) -> u32 {
    (size / 16).clamp(1, 3)
}

/// Stamp one tile, ink only, onto an opaque surface.
fn stamp_ink(surf: &mut Surface, t: &Tile, pen: i32, baseline: i32, ink: [u8; 3]) {
    let ox = pen + t.dx;
    let oy = baseline + t.dy;
    for row in 0..t.h as i32 {
        for col in 0..t.w as i32 {
            let i = ((row as u32 * t.w + col as u32) * 4) as usize;
            let a = t.rgba[i];
            if a > 0 {
                surf.blend_pixel(ox + col, oy + row, [ink[0], ink[1], ink[2], a]);
            }
        }
    }
}

/// Stamp one tile into a raw RGBA buffer, keeping the coverage as alpha.
fn stamp_raw(buf: &mut [u8], bw: u32, bh: u32, t: &Tile, pen: i32, baseline: i32, style: &TextStyle) {
    let ox = pen + t.dx;
    let oy = baseline + t.dy;
    for row in 0..t.h as i32 {
        for col in 0..t.w as i32 {
            let i = ((row as u32 * t.w + col as u32) * 4) as usize;
            let a = t.rgba[i];
            let e = t.rgba[i + 1];
            let (rgb, cov) = if a > 0 {
                (style.color, a)
            } else if style.edge_alpha > 0 && e > 0 {
                ([0, 0, 0], ((e as u32 * style.edge_alpha as u32) / 255) as u8)
            } else {
                continue;
            };
            let (x, y) = (ox + col, oy + row);
            if x < 0 || y < 0 || x >= bw as i32 || y >= bh as i32 {
                continue;
            }
            let o = ((y as u32 * bw + x as u32) * 4) as usize;
            if cov <= buf[o + 3] {
                continue;
            }
            buf[o] = rgb[0];
            buf[o + 1] = rgb[1];
            buf[o + 2] = rgb[2];
            buf[o + 3] = cov;
        }
    }
}

/// Stamp one tile with a style, in one pass per pixel so the layers keep their
/// order: the ink wins where it exists, the translucent edge takes over around
/// it, and the soft shadow - the glyph's own coverage shifted by the style's
/// offset - only shows where neither reached.
fn stamp_styled(surf: &mut Surface, t: &Tile, pen: i32, baseline: i32, style: &TextStyle) {
    if t.w == 0 || t.h == 0 {
        return;
    }
    let ox = pen + t.dx;
    let oy = baseline + t.dy;
    let (sdx, sdy, shadow_alpha) = style.shadow;
    let edge_alpha = style.edge_alpha;
    let shadow_on = shadow_alpha > 0 && (sdx != 0 || sdy != 0);
    for row in 0..t.h as i32 {
        for col in 0..t.w as i32 {
            let i = ((row as u32 * t.w + col as u32) * 4) as usize;
            let a = t.rgba[i];
            let e = t.rgba[i + 1];
            let (x, y) = (ox + col, oy + row);
            if a > 0 {
                surf.blend_pixel(x, y, [style.color[0], style.color[1], style.color[2], a]);
            } else if e > 0 && edge_alpha > 0 {
                let cov = ((e as u32 * edge_alpha as u32) / 255) as u8;
                if cov > 0 {
                    surf.blend_pixel(x, y, [0, 0, 0, cov]);
                }
            } else if shadow_on {
                let (sc, sr) = (col - sdx, row - sdy);
                if sc >= 0 && sr >= 0 && sc < t.w as i32 && sr < t.h as i32 {
                    let ink_cov = t.rgba[((sr as u32 * t.w + sc as u32) * 4) as usize];
                    if ink_cov > 0 {
                        let cov = ((ink_cov as u32 * shadow_alpha as u32) / 255) as u8;
                        surf.blend_pixel(
                            x,
                            y,
                            [style.shadow_rgb[0], style.shadow_rgb[1], style.shadow_rgb[2], cov],
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        font_catalog, font_profile, role_generation, role_ids, role_requested, round_266,
        set_role_ids, with_role_font, FontRole, Label, TextStyle, VectorFont, DEFAULT_FONT,
        DEFAULT_GAMMA,
    };
    use crate::render::Surface;

    /// Tests that touch the process-global role ids or the environment take
    /// this lock, so two parallel test threads do not fight over them.
    static GLOBAL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn global_lock() -> std::sync::MutexGuard<'static, ()> {
        GLOBAL_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Run `f` with environment variables set (an empty value means unset),
    /// restoring every previous value afterwards, even when the test fails.
    fn with_env<R>(vars: &[(&str, &str)], f: impl FnOnce() -> R) -> R {
        struct Restore(Vec<(String, Option<std::ffi::OsString>)>);
        impl Drop for Restore {
            fn drop(&mut self) {
                for (key, value) in &self.0 {
                    match value {
                        Some(v) => std::env::set_var(key, v),
                        None => std::env::remove_var(key),
                    }
                }
            }
        }
        let mut saved = Vec::new();
        for (key, _) in vars {
            saved.push((key.to_string(), std::env::var_os(key)));
        }
        for (key, value) in vars {
            if value.is_empty() {
                std::env::remove_var(key);
            } else {
                std::env::set_var(key, value);
            }
        }
        let restore = Restore(saved);
        let out = f();
        drop(restore);
        out
    }

    /// DejaVu has Latin but no CJK: the file the face and fallback tests use
    /// for "a font that opens but cannot draw the script".
    const DEJAVU: &str = "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf";

    /// The default entry is id 0, because 0 is what every old config file and
    /// every out-of-range value resolves to. Ids are otherwise hashes of the
    /// font file, so they are not dense - they just have to be unique.
    #[test]
    fn catalogue_ids_start_at_zero_and_are_unique() {
        let cat = font_catalog();
        assert_eq!(cat[0].id, 0);
        let mut ids: Vec<i32> = cat.iter().map(|p| p.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), cat.len(), "an id must name one font, not two");
    }

    /// An id this machine does not have - a hand-edited config, an
    /// uninstalled font - falls back to the default instead of to a file that
    /// is not there.
    #[test]
    fn unknown_profile_ids_fall_back_to_the_default() {
        let _guard = global_lock();
        let Some(real) = font_catalog().iter().find(|p| p.id != 0 && p.path.is_some()) else {
            return
        };
        set_role_ids([0, 0, 0]);
        set_role_ids([real.id, 0, 0]);
        assert_eq!(role_ids()[0], real.id, "a real id has to stick");
        set_role_ids([i32::MAX, -3, 0]);
        assert_eq!(role_ids(), [0, 0, 0], "out-of-range ids must not stick");
        set_role_ids([0, 0, 0]);
    }
    /// The whole point of the role layer: pointing one role at another profile
    /// leaves the others on the font they were opened with.
    #[test]
    fn roles_keep_their_own_font() {
        let _guard = global_lock();
        set_role_ids([0, 0, 0]);
        let def_path = font_catalog()[0].path.clone();
        // A different *file*, not just a different catalogue entry: the pack's
        // Medium is both the default and its own candidate row.
        let Some(other) = font_catalog()
            .iter()
            .find(|p| p.id != 0 && p.path.is_some() && p.path != def_path)
        else {
            return
        };
        let game_before = with_role_font(FontRole::Game, 20, |f| f.describe());
        set_role_ids([other.id, 0, 0]);
        let home = with_role_font(FontRole::Home, 20, |f| f.describe());
        let game_after = with_role_font(FontRole::Game, 20, |f| f.describe());
        set_role_ids([0, 0, 0]);
        let (Some(home), Some(before), Some(after)) = (home, game_before, game_after) else {
            return
        };
        assert_eq!(before, after, "the game font must not follow the home one");
        assert_ne!(home, after, "the home font should follow its own id");
    }

    /// Tracking has to move the glyphs, not just the measured width: every gap
    /// between two glyphs gets the extra pixels. A loop that adds the tracking
    /// after the second glyph makes the first gap disappear while the measured
    /// width still counts it, so the line comes out left-shifted and the last
    /// glyphs crowd one gap.
    #[test]
    fn tracking_moves_glyphs_apart() {
        let Some(f) = VectorFont::open(DEFAULT_FONT, 24) else { return };
        let ink_span = |tracking: i32| {
            let label = Label {
                text: "画面设置".to_string(),
                x: 0,
                y: 0,
                w: 320,
                h: 40,
                size: 24,
                center: false,
                rgb: [255, 255, 255],
                tracking,
            };
            let buf = f.labels_rgba(320, 40, &[label]).expect("label buffer");
            let mut min = 320i32;
            let mut max = -1i32;
            for y in 0..40u32 {
                for x in 0..320u32 {
                    let i = ((y * 320 + x) * 4) as usize;
                    if buf[i + 3] > 0 {
                        min = min.min(x as i32);
                        max = max.max(x as i32);
                    }
                }
            }
            (min, max)
        };
        let (a0, a1) = ink_span(0);
        let (b0, b1) = ink_span(6);
        assert_eq!(b0, a0, "tracking must not move the first glyph");
        assert_eq!(b1 - a1, 6 * 3, "three gaps each get six pixels");
    }

    /// Draw one line on a `bg` plate and report where the ink and the outline
    /// ended up. A white plate makes the outline countable, a black one the ink.
    fn probe(text: &str, size: u32, bg: [u8; 4]) -> Option<(Surface, i32, usize, usize)> {
        let f = VectorFont::open(DEFAULT_FONT, size)?;
        let mut s = Surface::new(240, 80, bg);
        let baseline = 50;
        let end = f.draw(&mut s, text, 4, baseline);
        let mut ink = 0;
        let mut edge = 0;
        for y in 0..s.height as i32 {
            for x in 0..s.width as i32 {
                let i = ((y as u32 * s.width + x as u32) * 4) as usize;
                if s.pixels[i] > 200 {
                    ink += 1;
                } else if s.pixels[i] < 200 {
                    edge += 1;
                }
            }
        }
        Some((s, end, ink, edge))
    }

    #[test]
    fn a_line_advances_one_em_per_ideograph() {
        // Skip on a machine without the font: the engine falls back to the
        // bitmap atlas there, it is not a failure of this code.
        let Some((_s, end, ink, edge)) = probe("一丁七", 26, [255, 255, 255, 255]) else { return };
        assert!(ink > 0, "nothing was drawn");
        assert!(edge > 0, "no outline around the ink");
        // Three full-width glyphs from x=4, within a pixel of rounding.
        assert!((end - (4 + 26 * 3)).abs() <= 3, "pen ended at {end}");
    }

    /// The UI screens cache a label as a (w, h, rgba) buffer and tint it later,
    /// so the coverage has to survive as alpha instead of being flattened onto
    /// the destination.
    #[test]
    fn a_label_buffer_keeps_its_coverage_as_alpha() {
        let Some(f) = VectorFont::open(DEFAULT_FONT, 22) else { return };
        let Some((w, h, buf)) = f.line_rgba("文本显示速度", 22) else { return };
        assert_eq!(buf.len(), w as usize * h as usize * 4);
        let mut solid = 0;
        let mut partial = 0;
        for px in buf.chunks_exact(4) {
            if px[3] == 0 {
                continue;
            }
            // Anything with coverage is white ink; the tint is applied later.
            assert!(px[0] > 200 && px[1] > 200 && px[2] > 200, "not white: ");
            if px[3] == 255 {
                solid += 1;
            } else {
                partial += 1;
            }
        }
        assert!(solid > 20, "no solid ink");
        assert!(partial > 20, "no antialiased edge");
    }

    /// The dialog is 26px CJK: the default face and coverage curve have to
    /// leave the strokes mostly solid, with a continuous anti-aliased edge
    /// rather than a binary one.
    #[test]
    fn the_default_face_draws_solid_strokes_at_dialog_size() {
        let Some(f) = VectorFont::open(DEFAULT_FONT, 26) else { return };
        let mut surf = Surface::new(900, 60, [0, 0, 0, 255]);
        f.draw(&mut surf, "路邊的樹木葉子差不多已經落盡", 4, 40);
        let mut solid = 0usize;
        let mut partial = 0usize;
        let mut any = 0usize;
        for px in surf.pixels.chunks_exact(4) {
            if px[0] > 128 {
                any += 1;
                if px[0] > 245 {
                    solid += 1;
                } else {
                    partial += 1;
                }
            }
        }
        assert!(any > 500, "nothing drawn, {any}");
        let frac = solid as f32 / any as f32;
        // Mostly solid strokes, but not pushed to a binary edge: the old
        // 130% linear gain reached ~0.79 by saturating every mid-coverage
        // pixel, which is what closed the counters of dense glyphs. The gamma
        // curve keeps a real anti-aliased band instead.
        assert!(frac > 0.60, "strokes only {:.2} solid ({solid}/{any})", frac);
        assert!(partial as f32 / any as f32 > 0.15, "no anti-aliased edge ({partial}/{any})");
    }

    /// The gamma curve must not saturate the way the old linear gain did:
    /// pushing 130% linearly turned every mid-coverage pixel into solid ink
    /// and closed the counter of dense glyphs.
    #[test]
    fn the_gamma_curve_keeps_a_partial_edge() {
        let Some(f) = VectorFont::open(DEFAULT_FONT, 26) else { return };
        let Some((_w, _h, buf)) = f.line_rgba("鬱邊體樂觀發", 26) else { return };
        let mut solid = 0usize;
        let mut partial = 0usize;
        for px in buf.chunks_exact(4) {
            if px[3] == 0 {
                continue;
            }
            if px[3] == 255 {
                solid += 1;
            } else {
                partial += 1;
            }
        }
        assert!(solid > 100, "no solid ink in dense glyphs ({solid})");
        assert!(partial > 100, "edge saturated to solid ({partial})");
    }

    #[test]
    fn ink_stays_above_the_descent() {
        let Some((s, _end, _ink, _edge)) = probe("一丁七", 26, [0, 0, 0, 255]) else { return };
        let mut lowest = -1;
        for y in 0..s.height as i32 {
            for x in 0..s.width as i32 {
                let i = ((y as u32 * s.width + x as u32) * 4) as usize;
                if s.pixels[i] > 200 {
                    lowest = lowest.max(y);
                }
            }
        }
        // 一丁七 has no descender, so the ink must sit on the baseline (50)
        // rather than hanging below it.
        assert!(lowest > 0 && lowest <= 53, "lowest ink row {lowest}");
    }

    /// The face index is what puts Simplified Chinese glyphs on screen: face 0
    /// of the same file is Japanese and draws 骨/直/着 with different shapes.
    #[test]
    fn the_sc_face_is_the_default() {
        let Some(f) = VectorFont::open(DEFAULT_FONT, 26) else { return };
        let family = f.family();
        assert!(family.contains("SC"), "default face is {family}");
    }

    /// Layout reads the metrics off the size object, so a line box centres the
    /// font's own vertical extent: the baseline sits between the ascender and
    /// the descender, not on top of the ascender alone.
    #[test]
    fn the_baseline_centres_the_line_box() {
        let Some(f) = VectorFont::open(DEFAULT_FONT, 26) else { return };
        let m = f.metrics();
        assert!(m.ascender > 0 && m.descender < 0, "metrics {m:?}");
        let base = f.baseline_in_box(100, 36);
        let above = base - 100;
        // The ink band the metrics describe sits centred in the box: the
        // slack above the ascender equals the slack below the descender.
        let slack_above = above - m.ascender;
        let slack_below = 36 - (above + m.descender);
        // Integer division can leave the slack a pixel uneven.
        assert!((slack_above - slack_below).abs() <= 1, "baseline {base} is not centred");
        assert!(slack_above >= 0, "ink taller than the line box: {slack_above}");
        // The old layout put the baseline one ascender below the box top,
        // which pushed every line down by the whole slack.
        assert!(above < m.ascender + 12, "baseline {base} leaves {above} above");
    }

    /// The default ink curve is the gamma one, and the legacy gain is opt-in.
    #[test]
    fn gamma_is_the_default_ink_curve() {
        let Some(f) = VectorFont::open(DEFAULT_FONT, 26) else { return };
        assert_eq!(f.ink_gamma(), Some(DEFAULT_GAMMA));
        assert_eq!(f.ink_gain(), None);
        f.set_ink_gain(130);
        assert_eq!(f.ink_gain(), Some(130));
        assert_eq!(f.ink_gamma(), None);
    }

    /// The face index is load-bearing, not cosmetic: 直 and 令 are drawn
    /// with visibly different shapes by the Japanese and the Simplified
    /// Chinese face of the same file. Rendering both and comparing proves the
    /// engine is really on the SC face it claims to be on.
    #[test]
    fn the_face_index_changes_the_glyphs() {
        let (Some(jp), Some(sc)) = (
            VectorFont::open_face(DEFAULT_FONT, 26, 0),
            VectorFont::open_face(DEFAULT_FONT, 26, 2),
        ) else {
            return
        };
        for ch in ['直', '令', '门'] {
            let mut a = Surface::new(80, 80, [0, 0, 0, 255]);
            let mut b = Surface::new(80, 80, [0, 0, 0, 255]);
            jp.draw_flat(&mut a, &ch.to_string(), 20, 50, [255, 255, 255]);
            sc.draw_flat(&mut b, &ch.to_string(), 20, 50, [255, 255, 255]);
            let diff = a.pixels.iter().zip(&b.pixels).filter(|(x, y)| x != y).count();
            assert!(diff > 100, "{ch}: JP and SC faces drew the same glyph ({diff} px differ)");
        }
        // The default must be the SC face, i.e. the same one as index 2.
        let Some(def) = VectorFont::open(DEFAULT_FONT, 26) else { return };
        let mut a = Surface::new(80, 80, [0, 0, 0, 255]);
        let mut b = Surface::new(80, 80, [0, 0, 0, 255]);
        def.draw_flat(&mut a, "直", 20, 50, [255, 255, 255]);
        sc.draw_flat(&mut b, "直", 20, 50, [255, 255, 255]);
        let diff = a.pixels.iter().zip(&b.pixels).filter(|(x, y)| x != y).count();
        assert_eq!(diff, 0, "default face differs from the SC face by {diff} px");
    }

    /// Kerning only applies between two Latin characters; CJK pairs must keep
    /// their full-width advance.
    #[test]
    fn kerning_applies_to_latin_only() {
        let Some(f) = VectorFont::open(DEFAULT_FONT, 26) else { return };
        let plain = f.measure_char('A') + f.measure_char('V');
        let kerned = f.measure("AV");
        assert!(kerned <= plain, "AV {kerned} wider than {plain}");
        let cjk = f.measure("一丁");
        assert_eq!(cjk, 26 * 2, "CJK pair measured {cjk}");
    }

    /// The body style stacks three separable layers, and on a mid-grey plate
    /// they land in different bands: white ink, a translucent dark edge around
    /// it, and a soft shadow. Measuring the bands is how the "not a black
    /// sticker" requirement stays true after later tuning.
    #[test]
    fn the_body_style_layers_ink_edge_and_shadow() {
        let Some(f) = VectorFont::open(DEFAULT_FONT, 26) else { return };
        let mut with = Surface::new(140, 90, [128, 128, 128, 255]);
        let mut without = Surface::new(140, 90, [128, 128, 128, 255]);
        let plain = TextStyle { shadow: (0, 0, 0), ..TextStyle::body() };
        f.draw_styled(&mut with, "國", 50, 60, &TextStyle::body());
        f.draw_styled(&mut without, "國", 50, 60, &plain);
        let at = |s: &Surface, x: i32, y: i32| -> u8 {
            if x < 0 || y < 0 || x >= s.width as i32 || y >= s.height as i32 {
                return 0;
            }
            s.pixels[((y as u32 * s.width + x as u32) * 4) as usize]
        };
        let (mut ink, mut ring, mut shadow) = (0usize, 0usize, 0usize);
        let mut misplaced = 0usize;
        for y in 0..with.height as i32 {
            for x in 0..with.width as i32 {
                let v = at(&with, x, y);
                let v0 = at(&without, x, y);
                if v > 220 {
                    ink += 1;
                } else if v < 60 {
                    ring += 1;
                } else if v < 105 {
                    shadow += 1;
                    // Everything the shadow adds must be the glyph's own
                    // coverage shifted one pixel down-right: the pixel up-left
                    // of it is touched by the glyph in the same render (ink
                    // brightens the grey plate, edge and shadow darken it), so
                    // an untouched plate pixel up-left means stray darkness.
                    if v != v0 && at(&with, x - 1, y - 1) == 128 {
                        misplaced += 1;
                    }
                }
            }
        }
        assert!(ink > 150, "no solid ink: {ink}");
        assert!(ring > 30, "no translucent edge ring: {ring}");
        assert!(shadow > 15, "no soft shadow: {shadow}");
        assert_eq!(misplaced, 0, "{misplaced} shadow pixels are not the shifted ink");
    }

    /// The CJK width policy: the punctuation the original drew in full-width
    /// cells keeps a full em whatever the font file says, so a line of dialogue
    /// has an even rhythm and the punctuation does not change width when the
    /// player swaps the font.
    #[test]
    fn cjk_punctuation_keeps_a_full_width_advance() {
        let Some(f) = VectorFont::open(DEFAULT_FONT, 26) else { return };
        let full = [
            '\u{3000}', '\u{3001}', '\u{3002}', '\u{300C}', '\u{300D}', '\u{300E}', '\u{300F}',
            '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2026}', '\u{203B}', '\u{2014}',
            // The fullwidth forms block is what the scripts actually use most:
            // '，' alone is 65k occurrences, '？' 15k, '！' 4.8k, '～' 3.7k.
            '\u{FF0C}', '\u{FF1F}', '\u{FF01}', '\u{FF5E}', '\u{FF08}', '\u{FF09}', '\u{FF1A}',
        ];
        for ch in full {
            assert_eq!(
                f.measure_char(ch),
                26,
                "U+{:04X} should advance a full em",
                ch as u32
            );
        }
        // A half-width space stays the font's own space, and Latin keeps its
        // proportional advance: this policy is about CJK punctuation, not
        // about squeezing everything to one width.
        let space = f.measure_char(' ');
        assert!(space > 0 && space < 13, "half-width space advanced {space}");
        assert!(f.measure_char('A') < 26, "Latin must stay proportional");
        // The halfwidth forms block is narrow by definition and must not have
        // been swept up by the U+FF01..U+FF60 range above.
        let halfwidth_stop = f.measure_char('\u{FF61}');
        assert!(halfwidth_stop > 0 && halfwidth_stop < 26, "halfwidth form widened to {halfwidth_stop}");
    }

    /// Two ellipsis dots are two cells wide - the shape the original had - and
    /// an em-dash run keeps the same rhythm, which is the one place Noto's own
    /// advance (23px for U+2014) would have made the line ragged.
    #[test]
    fn ellipsis_and_dash_runs_keep_the_dialogue_rhythm() {
        let Some(f) = VectorFont::open(DEFAULT_FONT, 26) else { return };
        let em = f.measure_char('\u{4E00}');
        assert_eq!(f.measure("\u{2026}\u{2026}"), 2 * em, "…… is two cells");
        assert_eq!(f.measure("\u{2014}\u{2014}"), 2 * em, "—— is two cells");
        // Mixed punctuation between ideographs must not change the count.
        assert_eq!(
            f.measure("\u{4E00}\u{3002}\u{4E00}"),
            3 * em,
            "ideograph, 。 , ideograph"
        );
    }

    /// Layout works in 26.6 and rounds only where a glyph is stamped, so a
    /// long Latin line does not lose its fractional advances one character at
    /// a time.
    #[test]
    fn a_long_latin_line_does_not_drift() {
        let Some(f) = VectorFont::open(DEFAULT_FONT, 26) else { return };
        // "AVAV" is the pair Noto kerns; repeating it 20 times is 40 Latin
        // glyphs, every one of them with a fractional advance.
        let line = "AVAVAVAVAVAVAVAVAVAVAVAVAVAVAVAVAVAV".to_string();
        // Per-glyph rounding would drift: each advance is 16px + a fraction,
        // so rounding 40 of them must not land 40x the error of one.
        let mut naive = 0i32;
        for ch in line.chars() {
            naive += f.measure_char(ch);
        }
        let exact = round_266(
            line.chars()
                .enumerate()
                .fold(0i32, |acc, (i, ch)| acc + f.advance_266(if i == 0 { None } else { line.chars().nth(i - 1) }, ch)),
        );
        assert_eq!(f.measure(&line), exact, "measure and the fixed-point pen disagree");
        // The kerning is what keeps this from being a wash; assert the two
        // differ by less than a pixel, i.e. no accumulation.
        assert!((naive - exact).abs() <= 1, "rounding drifted: {naive} vs {exact}");
    }

    /// A character the face has no glyph for leaves a gap of the width it
    /// would have had - no ragged hole, no placeholder box - and the font
    /// reports it once instead of swallowing it.
    #[test]
    fn a_missing_glyph_leaves_a_sized_gap() {
        let Some(f) = VectorFont::open(DEFAULT_FONT, 26) else { return };
        // A colour emoji is absent from a CJK text face.
        let missing = '\u{1F600}';
        assert_eq!(f.measure_char(missing), 26, "gap should be one em");
        // Drawn twice: the second pass must hit the cache, not warn again or
        // fall over.
        let mut s = Surface::new(200, 60, [0, 0, 0, 255]);
        f.draw_styled(&mut s, &missing.to_string(), 4, 40, &TextStyle::body());
        f.draw_styled(&mut s, &missing.to_string(), 4, 40, &TextStyle::body());
        // Text around it still lines up: gap + two ideographs.
        assert_eq!(f.measure(&format!("一{missing}一")), 26 * 3);
    }

    /// The name has to read as its own level, not as a second copy of the
    /// body line: a warmer ink and a firmer edge, no shadow.
    #[test]
    fn the_name_style_is_distinct_from_the_body() {
        let body = TextStyle::body();
        let name = TextStyle::name();
        assert_ne!(body.color, name.color, "name and body are the same colour");
        assert!(
            name.color[0] > name.color[2] && name.color[2] >= 200,
            "name should be a warm near-white, got {:?}",
            name.color
        );
        assert!(name.edge_alpha > body.edge_alpha, "name edge is not firmer");
        assert_eq!(name.shadow.2, 0, "the rim is dark; the name needs no shadow");
    }

    /// A style with no edge and no shadow draws exactly the ink, and a style
    /// with a shadow puts dark pixels below-right of the ink.
    #[test]
    fn styles_control_edge_and_shadow() {
        let Some(f) = VectorFont::open(DEFAULT_FONT, 26) else { return };
        let flat = TextStyle::ui();
        let mut a = Surface::new(120, 60, [255, 255, 255, 255]);
        f.draw_styled(&mut a, "字", 10, 40, &flat);
        let mut dark = 0;
        for px in a.pixels.chunks_exact(4) {
            if px[0] < 200 {
                dark += 1;
            }
        }
        assert_eq!(dark, 0, "flat style still darkens the plate");

        let body = TextStyle::body();
        let mut b = Surface::new(120, 60, [255, 255, 255, 255]);
        f.draw_styled(&mut b, "字", 10, 40, &body);
        let mut dark = 0;
        for px in b.pixels.chunks_exact(4) {
            if px[0] < 200 {
                dark += 1;
            }
        }
        assert!(dark > 10, "body style left no separation: {dark}");
    }

    // ------------------------------------------------------------------
    // font pack, ids and the per-glyph fallback
    // ------------------------------------------------------------------

    /// The shipped pack has to be found, verified against its manifest, and
    /// offered as candidates - the bundled dir comes before the system ones
    /// (design §6), so the pack is visible in the panel and not only hidden
    /// behind id 0.
    #[test]
    fn the_bundled_pack_is_verified_and_listed() {
        let dirs = super::bundled_dirs();
        let Some((dir, version, entries)) = super::load_font_pack(&dirs) else { return };
        assert_eq!(version, super::FONT_PACK_VERSION, "manifest in {}", dir);
        assert!(!entries.is_empty(), "the manifest in {} verified no fonts", dir);
        for e in &entries {
            assert!(e.verified, "{} listed as verified without the manifest", e.file_name);
            assert!(std::path::Path::new(&e.path).is_file(), "{} is not on disk", e.path);
            assert!(!e.family.is_empty(), "{} has no family", e.path);
            if e.file_name.ends_with(".ttc") {
                assert_eq!(e.family, "Noto Sans CJK SC", "{}", e.path);
                assert_eq!(e.face, super::SC_FACE, "{}", e.path);
                assert!(e.coverage > 99.0, "{} coverage {}", e.path, e.coverage);
            } else {
                // The third-party display face is a single-face .ttf.
                assert_eq!(e.face, 0, "{}", e.path);
                assert!(e.coverage > 60.0, "{} coverage {}", e.path, e.coverage);
            }
        }
        let smiley = entries
            .iter()
            .find(|e| e.file_name.starts_with("SmileySans"))
            .expect("the bundled display face is missing from the pack");
        assert_eq!(smiley.display, "得意黑", "manifest display column not used");
        assert_eq!(smiley.display_name(), "得意黑", "the list should show the Chinese name");
        let cat = font_catalog();
        assert!(cat[0].bundled, "the default should be the bundled font");
        for e in &entries {
            assert!(
                cat.iter().any(|p| p.id == e.id() && p.path.as_deref() == Some(e.path.as_str())),
                "{} is missing from the catalogue",
                e.file_name
            );
        }
    }

    /// The default entry is a resolved file, not a list of hard-coded paths
    /// that may not exist: with the pack present it is the pack's Medium, and
    /// if not, the platform fallback still has to be a real openable file
    /// (F-11).
    #[test]
    fn the_default_profile_resolves_to_a_real_file() {
        let cat = font_catalog();
        let def = &cat[0];
        let path = def.path.as_ref().expect("a default font has to resolve on this machine");
        assert!(std::path::Path::new(path).is_file(), "{} is not a file", path);
        assert!(!def.family.is_empty(), "the default has no family name");
        let Some((_, _, entries)) = super::load_font_pack(&super::bundled_dirs()) else { return };
        if let Some(medium) = entries.iter().find(|e| e.file_name.to_lowercase().contains("medium")) {
            assert_eq!(path, &medium.path, "the default has to prefer the bundled Medium");
            assert_eq!(def.face, medium.face);
        }
    }

    /// Every manifest column is load-bearing: the size, the SHA-256, the face
    /// and the family all have to match the file on disk, or the candidate is
    /// dropped and the log says why (F-13).
    #[test]
    fn a_manifest_row_is_checked_against_the_file() {
        // Use the smallest catalogue font so hashing it here is cheap.
        let Some((src, face)) = font_catalog()
            .iter()
            .filter_map(|p| p.path.as_ref().map(|path| (path.clone(), p.face)))
            .min_by_key(|(path, _)| std::fs::metadata(path).map(|m| m.len()).unwrap_or(u64::MAX))
        else {
            return
        };
        let dir = std::env::temp_dir().join(format!("wa2-font-row-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let file = dir.join("ProbeFace.ttf");
        if std::fs::copy(&src, &file).is_err() {
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        let path = file.to_string_lossy().into_owned();
        let size = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
        let sha = crate::sha256::hex_digest_file(&file).unwrap_or_default();
        let family = match crate::ft2::Ft::open(&path, 24, face) {
            Some(ft) => ft.family().unwrap_or_default(),
            None => {
                let _ = std::fs::remove_dir_all(&dir);
                return;
            }
        };
        assert!(super::verify_bundled_font(&path, &family, face, size, &sha).is_ok());
        let err = super::verify_bundled_font(&path, &family, face, size + 1, &sha).unwrap_err();
        assert!(err.contains("size"), "{}", err);
        let mut bad = sha.clone();
        bad.replace_range(0..1, if sha.starts_with('0') { "1" } else { "0" });
        let err = super::verify_bundled_font(&path, &family, face, size, &bad).unwrap_err();
        assert!(err.contains("sha256"), "{}", err);
        let err =
            super::verify_bundled_font(&path, "Definitely Not This Family", face, size, &sha).unwrap_err();
        assert!(err.contains("family"), "{}", err);
        let err = super::verify_bundled_font(&path, &family, 99, size, &sha).unwrap_err();
        assert!(err.contains("face"), "{}", err);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A package without a manifest is tolerated (unverified bundled files),
    /// and a manifest listing a file that is not there keeps going instead of
    /// panicking (F-13: no pack must not stop the game from starting).
    #[test]
    fn a_missing_or_broken_manifest_is_tolerated() {
        let dir = std::env::temp_dir().join(format!("wa2-font-manifest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let dirs = vec![dir.to_string_lossy().into_owned()];
        assert!(super::load_font_pack(&dirs).is_none(), "an empty dir has no manifest");
        let manifest = dir.join("manifest.tsv");
        let text = format!(
            "# wa2 font pack version {}\n# path\tfamily\tface\tcoverage\tsize\tsha256\nMissing.ttc\tNoto Sans CJK SC\t2\t100.0\t123\t{}\n",
            super::FONT_PACK_VERSION,
            "0".repeat(64)
        );
        if std::fs::write(&manifest, text).is_ok() {
            let Some((_, version, entries)) = super::load_font_pack(&dirs) else {
                panic!("a manifest with one broken row must still load");
            };
            assert_eq!(version, super::FONT_PACK_VERSION);
            assert!(entries.is_empty(), "a missing file must not verify");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Bundled and platform candidates live in two id spaces, so a config value
    /// can never silently name the other kind (F-12); the bundled id is cut
    /// from file name+face, not from the path it happened to sort into.
    #[test]
    fn bundled_and_system_ids_live_in_their_own_spaces() {
        assert_eq!(super::pack_id("NotoSansCJK-Medium.ttc", 2), super::pack_id("NotoSansCJK-Medium.ttc", 2));
        assert_ne!(super::pack_id("a.ttf", 0), super::pack_id("b.ttf", 0));
        assert_ne!(super::pack_id("a.ttf", 0), super::pack_id("a.ttf", 1));
        assert_eq!(super::system_id("a.ttf", 0) & super::SYSTEM_ID_BIT, super::SYSTEM_ID_BIT);
        assert_eq!(super::pack_id("a.ttf", 0) & super::SYSTEM_ID_BIT, 0);
        assert_ne!(super::pack_id("a.ttf", 0), super::system_id("a.ttf", 0));
        // The same file name and face is a different id in each space, and a
        // path in another directory does not move the bundled one.
        assert_eq!(super::pack_id("a.ttf", 0), super::pack_id("a.ttf", 0));
        for p in font_catalog().iter().skip(1) {
            if p.bundled {
                assert_eq!(p.id & super::SYSTEM_ID_BIT, 0, "bundled id of {}", p.name);
            } else {
                assert_eq!(p.id & super::SYSTEM_ID_BIT, super::SYSTEM_ID_BIT, "system id of {}", p.name);
            }
        }
    }

    /// Region suffixes decide what the Chinese candidate list may offer: a JP
    /// or TC face passes a coverage probe but draws the wrong shapes, so the
    /// scan keeps it out (F-13).
    #[test]
    fn region_families_are_classified() {
        assert!(super::is_sc_family("Noto Sans CJK SC"));
        assert!(super::is_sc_family("Source Han Sans CN"));
        assert!(super::is_sc_family("Source Han Sans Hans"));
        for family in [
            "Noto Sans CJK JP",
            "Noto Sans CJK KR",
            "Noto Sans CJK TC",
            "Noto Sans CJK TW",
            "Noto Sans CJK HK",
        ] {
            assert!(super::is_other_region(family), "{} should be another region", family);
            assert!(!super::is_sc_family(family), "{} must not count as SC", family);
        }
        // A family with no region suffix is judged by its coverage probe, not
        // by its name.
        assert!(!super::is_other_region("FZFangSong-Z02"));
        for p in font_catalog() {
            assert!(
                !super::is_other_region(&p.family),
                "{} ({}) leaked into the Chinese list",
                p.name,
                p.family
            );
        }
    }

    /// A path override without `#face` is probed instead of being sent to
    /// DEFAULT_FACE=2 unconditionally, which fails for every single-face font
    /// (F-09). For a Noto collection the probe lands on the SC face.
    #[test]
    fn a_bare_override_probes_for_its_face() {
        let Some((_, _, entries)) = super::load_font_pack(&super::bundled_dirs()) else {
            if !std::path::Path::new(DEFAULT_FONT).is_file() {
                return;
            }
            assert_eq!(super::probe_face(DEFAULT_FONT), super::SC_FACE, "the SC face is the default choice");
            return;
        };
        let Some(medium) = entries.first() else { return };
        assert_eq!(super::probe_face(&medium.path), medium.face, "the probe must land on the SC face");
        // A single-face Latin font can only be face 0; asking for face 2 would
        // fail before the coverage check could explain why.
        if std::path::Path::new(DEJAVU).is_file() {
            assert_eq!(super::probe_face(DEJAVU), 0, "a single-face font probes to face 0");
        }
    }

    /// The documented priority chain, including F-21's edge cases: an empty
    /// role variable counts as unset, an explicit `#face` beats
    /// WA2_FONT_FACE, and WA2_FONT_FACE only applies to a path override.
    #[test]
    fn env_font_face_follows_the_documented_priority() {
        let _guard = global_lock();
        if !std::path::Path::new(DEJAVU).is_file() {
            return;
        }
        // A bare path is probed: the old code sent it to face 2 and failed.
        with_env(&[("WA2_FONT_GAME", DEJAVU)], || {
            let c = super::font_choice_for(FontRole::Game);
            assert_eq!(c.path, DEJAVU);
            assert_eq!(c.face, 0, "a bare .ttf must probe to its only face");
        });
        // An explicit #face wins over WA2_FONT_FACE.
        with_env(
            &[("WA2_FONT_GAME", &format!("{}#0", DEJAVU)), ("WA2_FONT_FACE", "3")],
            || {
                assert_eq!(
                    super::font_choice_for(FontRole::Game).face,
                    0,
                    "an explicit #face has to win over WA2_FONT_FACE"
                );
            },
        );
        // WA2_FONT_FACE applies when the path has no #face.
        with_env(&[("WA2_FONT_GAME", DEJAVU), ("WA2_FONT_FACE", "1")], || {
            assert_eq!(super::font_choice_for(FontRole::Game).face, 1);
        });
        // An empty role variable must not shadow the global WA2_FONT.
        with_env(&[("WA2_FONT_GAME", ""), ("WA2_FONT", DEJAVU)], || {
            assert_eq!(
                super::font_choice_for(FontRole::Game).path,
                DEJAVU,
                "an empty WA2_FONT_GAME has to count as unset"
            );
        });
        // Without a path spec the id from wa2.ini names its own face, and
        // WA2_FONT_FACE has nothing to attach to.
        set_role_ids([0, 0, 0]);
        with_env(
            &[("WA2_FONT_GAME", ""), ("WA2_FONT", ""), ("WA2_FONT_FACE", "0")],
            || {
                let c = super::font_choice_for(FontRole::Game);
                let want = font_profile(super::role_id(FontRole::Game))
                    .map(|p| p.face)
                    .unwrap_or(super::DEFAULT_FACE);
                assert_eq!(c.face, want, "WA2_FONT_FACE must not move a profile's face");
            },
        );
    }

    /// A role font that cannot be opened, or cannot draw the script, has to
    /// fall back to the default profile and leave a reason the log and the
    /// settings panel can show (F-09). A good font clears the reason again.
    #[test]
    fn a_bad_role_font_falls_back_to_the_default_with_a_reason() {
        let _guard = global_lock();
        set_role_ids([0, 0, 0]);
        let (default_path, _) = super::default_path_face();
        if !std::path::Path::new(&default_path).is_file() {
            return;
        }
        let want_family = VectorFont::open(&default_path, 20).map(|f| f.family()).unwrap_or_default();
        with_env(&[("WA2_FONT_HOME", "/nonexistent/wa2-font.ttf#0")], || {
            let Some(vf) = super::open_role_font(FontRole::Home, 20) else { return };
            assert_eq!(vf.family(), want_family, "a missing override has to fall back to the default");
            let reason = super::role_fallback_reason(FontRole::Home).unwrap_or_default();
            assert!(reason.contains("nonexistent"), "the reason was {:?}", reason);
        });
        if std::path::Path::new(DEJAVU).is_file() {
            with_env(&[("WA2_FONT_HOME", &format!("{}#0", DEJAVU))], || {
                let Some(vf) = super::open_role_font(FontRole::Home, 20) else { return };
                assert_eq!(vf.family(), want_family, "a font with no coverage has to fall back");
                let reason = super::role_fallback_reason(FontRole::Home).unwrap_or_default();
                assert!(reason.contains("glyph"), "the reason was {:?}", reason);
            });
        }
        let _ = super::open_role_font(FontRole::Home, 20);
        assert_eq!(
            super::role_fallback_reason(FontRole::Home),
            None,
            "a good font has to clear the reason"
        );
        super::set_role_fallback_reason(FontRole::Home, None);
        set_role_ids([0, 0, 0]);
    }

    /// The design's documented form `WA2_FONT_HOME=/path/to/font.ttc` has to
    /// work: a collection with no `#face` is probed to the Simplified Chinese
    /// face instead of relying on DEFAULT_FACE happening to be right (F-09).
    #[test]
    fn a_bare_collection_override_opts_into_the_sc_face() {
        let _guard = global_lock();
        let Some((_, _, entries)) = super::load_font_pack(&super::bundled_dirs()) else { return };
        let Some(medium) = entries.first() else { return };
        set_role_ids([0, 0, 0]);
        with_env(&[("WA2_FONT_GAME", &medium.path)], || {
            let choice = super::font_choice_for(FontRole::Game);
            assert_eq!(choice.face, medium.face, "a bare .ttc path must probe to the SC face");
            let Some(vf) = super::open_role_font(FontRole::Game, 20) else { return };
            assert_eq!(vf.family(), medium.family);
            assert_eq!(
                super::role_fallback_reason(FontRole::Game),
                None,
                "a good override must not leave a fallback reason"
            );
        });
        set_role_ids([0, 0, 0]);
        super::set_role_fallback_reason(FontRole::Game, None);
    }

    /// A player-chosen font that passes the probe but misses a few script
    /// characters gets only those characters from the default profile (F-10).
    /// FZFangSong-Z02 is the candidate the audit measured as passing the old
    /// probe while having no glyph for the note symbol.
    #[test]
    fn a_sparse_chosen_font_borrows_the_missing_symbols() {
        let _guard = global_lock();
        let Some(sparse) = font_catalog().iter().find(|p| {
            p.path.is_some() && p.family.contains("FZFangSong")
        }) else {
            return
        };
        let Some(sparse_path) = sparse.path.clone() else { return };
        let Some(probe) = VectorFont::open_face(&sparse_path, 26, sparse.face) else { return };
        let note = '\u{266A}';
        if probe.has_glyph(note) {
            return; // a different build of the font; nothing to prove here
        }
        let sparse_id = sparse.id;
        set_role_ids([sparse_id, 0, 0]);
        let Some(vf) = super::open_role_font(FontRole::Home, 26) else {
            set_role_ids([0, 0, 0]);
            return
        };
        assert_eq!(vf.family(), probe.family(), "the chosen font has to be the one opened");
        assert!(vf.fallback_family().is_some(), "the chosen font has no backup installed");
        let mut with = Surface::new(80, 80, [0, 0, 0, 255]);
        vf.draw_flat(&mut with, &note.to_string(), 10, 50, [255, 255, 255]);
        let ink = with.pixels.chunks_exact(4).filter(|px| px[0] > 200).count();
        assert!(ink > 20, "the backup did not supply the note symbol ({} px)", ink);
        set_role_ids([0, 0, 0]);
        super::set_role_fallback_reason(FontRole::Home, None);
    }

    /// A character the primary face is missing is drawn by the backup face for
    /// that glyph only (F-10): DejaVu cannot draw any CJK, so a Chinese line
    /// has to come out as ink once the default profile is installed as its
    /// backup - and the advance has to come from the backup glyph so the line
    /// still lays out on the grid.
    #[test]
    fn a_per_glyph_fallback_fills_the_gap() {
        let (default_path, default_face) = super::default_path_face();
        if !std::path::Path::new(&default_path).is_file() || !std::path::Path::new(DEJAVU).is_file() {
            return;
        }
        let Some(primary) = VectorFont::open_face(DEJAVU, 26, 0) else { return };
        let ink = |f: &VectorFont| {
            let mut surf = Surface::new(360, 80, [0, 0, 0, 255]);
            f.draw_flat(&mut surf, "骨直着国门", 4, 50, [255, 255, 255]);
            surf.pixels.chunks_exact(4).filter(|px| px[0] > 200).count()
        };
        assert_eq!(ink(&primary), 0, "DejaVu is supposed to have no CJK glyphs");
        assert!(primary.set_fallback(&default_path, default_face), "the backup face must open");
        assert!(primary.fallback_family().is_some(), "the backup is not reported");
        let drawn = ink(&primary);
        assert!(drawn > 500, "the backup draw only {} ink pixels", drawn);
        assert_eq!(primary.measure_char('骨'), 26, "the backup glyph has to carry its advance");
        // The backup follows the primary when it changes size.
        assert!(primary.set_size(40));
        let mut surf = Surface::new(200, 80, [0, 0, 0, 255]);
        primary.draw_flat(&mut surf, "骨", 4, 60, [255, 255, 255]);
        assert!(surf.pixels.chunks_exact(4).any(|px| px[0] > 200), "the backup did not rescale");
        assert_eq!(primary.measure_char('骨'), 40);
    }

    /// The per-glyph backup chain: the default profile is backed by the other
    /// weight in the pack, and a font the player chose is backed by the default
    /// profile. A backup is never the font itself.
    #[test]
    fn the_default_and_a_chosen_font_get_their_backups() {
        let _guard = global_lock();
        let Some(def_path) = font_catalog()[0].path.clone() else { return };
        set_role_ids([0, 0, 0]);
        let Some(vf) = super::open_role_font(FontRole::Game, 26) else { return };
        let chosen = super::font_choice_for(FontRole::Game).path;
        if let Some((backup, _)) = vf.fallback_spec() {
            assert_ne!(backup, chosen, "a font must not be its own backup");
            if chosen == def_path {
                assert!(backup.contains("NotoSansCJK"), "the default's backup was {}", backup);
            } else {
                assert_eq!(backup, def_path, "a chosen font is backed by the default profile");
            }
        }
        // A catalogue entry that is not the default gets the default as backup.
        if let Some(other) = font_catalog()
            .iter()
            .find(|p| p.id != 0 && p.path.as_deref() != Some(def_path.as_str()))
        {
            if let Some(preview) = super::open_profile_font(other.id, 20) {
                if let Some((backup, _)) = preview.fallback_spec() {
                    assert_eq!(backup, def_path, "preview rows fall back to the default");
                }
            }
        }
        set_role_ids([0, 0, 0]);
    }

    /// Every row the settings panel offers has to open at the preview size. A
    /// row that silently opened the default font instead would lie about what
    /// the player is picking, and a missing backup would leave glyph holes in
    /// the preview.
    #[test]
    fn every_catalogue_entry_opens_for_preview() {
        let cat = font_catalog();
        let def_path = cat[0].path.clone();
        let mut opened = 0;
        for p in cat {
            if p.path.is_none() {
                continue;
            }
            let Some(vf) = super::open_profile_font(p.id, 16) else {
                panic!("catalogue entry {} ({}) does not open", p.name, p.id);
            };
            if p.path != def_path {
                assert!(
                    vf.fallback_spec().is_some(),
                    "{} has no per-glyph backup",
                    p.name
                );
            }
            opened += 1;
        }
        assert!(opened > 0, "no catalogue entry could be opened");
    }

    /// The scan cache is only reused while the files it recorded are the same:
    /// every entry's size and mtime are checked, and an old-format cache
    /// (fewer columns) is a miss rather than a mis-parse (F-23).
    #[test]
    fn the_font_cache_is_checked_against_the_files() {
        let Some(src) = font_catalog()[0].path.clone() else { return };
        let Ok(meta) = std::fs::metadata(&src) else { return };
        let Ok(modified) = meta.modified() else { return };
        let Ok(mtime) = modified.duration_since(std::time::UNIX_EPOCH) else { return };
        let mtime = mtime.as_secs();
        let dir = std::env::temp_dir().join(format!("wa2-font-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let cache = dir.join("fonts.tsv");
        let row = |size: u64, mtime: u64| {
            format!("# files=1\n777\tTest Face\t{}\t2\tTest Family\t{}\t{}\n", src, size, mtime)
        };
        assert!(std::fs::write(&cache, row(meta.len(), mtime)).is_ok());
        let hit = super::load_font_cache_at(&cache, 1);
        assert!(hit.is_some(), "a fresh cache has to hit");
        assert_eq!(hit.map(|v| v[0].id), Some(777));
        assert!(super::load_font_cache_at(&cache, 2).is_none(), "a changed file count is a miss");
        let _ = std::fs::write(&cache, row(meta.len() + 1, mtime));
        assert!(super::load_font_cache_at(&cache, 1).is_none(), "a changed size is a miss");
        let _ = std::fs::write(&cache, row(meta.len(), mtime + 1));
        assert!(super::load_font_cache_at(&cache, 1).is_none(), "a changed mtime is a miss");
        let _ = std::fs::write(&cache, format!("# files=1\n777\tTest Face\t{}\t2\tTest Family\n", src));
        assert!(super::load_font_cache_at(&cache, 1).is_none(), "the old format has to rescan");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Missing glyphs are counted per character across every font, so the
    /// report can name the ones worth subsetting (design §1.3 / F-22).
    #[test]
    fn missing_glyphs_are_aggregated() {
        let Some(f) = VectorFont::open_face(DEJAVU, 26, 0) else { return };
        // An Ext-B ideograph no ordinary face covers; unique to this test, so
        // the parallel suite cannot have counted it.
        let ch = '\u{2A6D6}';
        let count = |ch: char| {
            super::missing_glyph_stats()
                .iter()
                .find(|(c, _)| *c == ch)
                .map(|(_, n)| *n)
                .unwrap_or(0)
        };
        let before = count(ch);
        let mut surf = Surface::new(80, 60, [0, 0, 0, 255]);
        f.draw(&mut surf, &ch.to_string(), 4, 40);
        assert_eq!(count(ch), before + 1, "the missing glyph was not counted");
        super::log_missing_glyph_stats();
    }

    /// Changing a role id bumps the generation every raster cache keys on, and
    /// moving it back is another change; the raw requested id is kept next to
    /// the clamped effective one (F-06/F-12).
    #[test]
    fn role_ids_bump_the_generation_and_keep_the_requested_value() {
        let _guard = global_lock();
        set_role_ids([0, 0, 0]);
        let Some(real) = font_catalog().iter().find(|p| p.id != 0 && p.path.is_some()) else {
            return;
        };
        let g0 = role_generation();
        super::set_role_fallback_reason(FontRole::History, Some("stale".to_string()));
        assert!(set_role_ids([real.id, 0, 0]), "a real id is a change");
        assert_eq!(role_generation(), g0 + 1);
        assert_eq!(role_requested(FontRole::Home), real.id);
        assert_eq!(
            super::role_fallback_reason(FontRole::History),
            None,
            "a font change has to clear the reason recorded for the old one"
        );
        assert!(set_role_ids([i32::MAX, 0, 0]), "a bogus id clamps back to 0, which is a change");
        assert_eq!(role_ids()[0], 0, "the effective id is clamped");
        assert_eq!(role_requested(FontRole::Home), i32::MAX, "the requested id is kept for the log");
        assert!(!set_role_ids([0, 0, 0]), "already on the default");
        assert_eq!(role_generation(), g0 + 2);
        set_role_ids([0, 0, 0]);
    }

    /// The dialogue style clamps every knob: a hand-edited ini cannot push the
    /// outline, weight, tracking or line gap past what the renderer handles
    /// (F-14).
    #[test]
    fn the_dialogue_style_clamps_its_knobs() {
        let wild = super::DialogStyle {
            size: 999,
            edge: 99,
            edge_alpha: 999,
            color: 99,
            underline: true,
            weight: 99,
            tracking: 99,
            line_gap: 99,
        };
        let c = wild.clamp();
        assert_eq!(c.size, super::DIALOG_SIZE_MAX);
        assert_eq!(c.edge, super::DIALOG_EDGE_MAX);
        assert_eq!(c.edge_alpha, super::DIALOG_EDGE_ALPHA_MAX);
        assert_eq!(c.color, (super::DIALOG_COLORS.len() - 1) as u32);
        assert_eq!(c.weight, super::DIALOG_WEIGHT_MAX);
        assert_eq!(c.tracking, super::DIALOG_TRACK_MAX);
        assert_eq!(c.line_gap, super::DIALOG_LINE_GAP_MAX);
        let low = super::DialogStyle {
            size: 1,
            edge: 0,
            edge_alpha: 0,
            color: 0,
            underline: false,
            weight: 0,
            tracking: -99,
            line_gap: -99,
        };
        let c = low.clamp();
        assert_eq!(c.size, super::DIALOG_SIZE_MIN);
        assert_eq!(c.tracking, super::DIALOG_TRACK_MIN);
        assert_eq!(c.line_gap, 0);
        let d = super::DialogStyle::default_style();
        assert_eq!(d.clamp(), d, "the shipped default has to be inside every range");
        assert_eq!(d.edge_alpha, super::DEFAULT_EDGE_ALPHA as u32);
    }

    /// A config written before the pack namespace still names a font this
    /// machine has: the old bare-name hash is mapped to the current id instead
    /// of dropping the player's choice to the default.
    #[test]
    fn legacy_font_ids_are_migrated() {
        let Some(entry) = super::font_catalog()
            .iter()
            .find(|p| p.id != 0 && p.path.is_some())
        else {
            return
        };
        let path = entry.path.as_ref().unwrap();
        let old = super::fnv_id(&format!("{}#{}", super::file_name_of(path), entry.face));
        assert_eq!(super::clamp_profile_id(old), entry.id);
        // An id that is in neither namespace still falls back to the default.
        assert_eq!(super::clamp_profile_id(i32::MAX - 1), 0);
    }
}
