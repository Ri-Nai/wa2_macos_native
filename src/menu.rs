//! The game's own option screen (「系统设置」/「オプション」).
//!
//! Everything on screen comes from the shipped layers, which are pixel
//! aligned with each other:
//!
//! ```text
//!   sys_03100/03101/03102   page base: grid, labels and silver widgets
//!   sys_03200/03210/03220   the same widgets in gold (a setting's current value)
//!   sys_03201/03211/03221   the same widgets in cyan (the widget under the cursor)
//!   sys_03000               top banner (1280x36)
//!   sys_03011               page tabs, 3 tabs x 3 states (192x32 each)
//!   sys_03040/03041/03042   one help line per setting row, 24 px each
//!   sys_03020               bottom bar: 3 arrow states plus the label strip
//!   sys_03030               "restore defaults" button, 3 states of 208x24
//! ```
//!
//! Widget rectangles are *not* taken from a table: they were recovered from
//! the images themselves (connected components of the widget border colour),
//! so no exe coordinates are needed. The Chinese patch ships its own copies of
//! 0310X / 0320X / 0321X / 0304X / 03020 / 03030, and `bg_dirs()` looks in the
//! CN directory first, so Chinese mode needs no extra work.
//!
//! Which overlay a widget gets is inferred: every widget is drawn from the
//! base layer, the option a setting currently holds comes from the gold layer,
//! and the widget under the cursor comes from the cyan layer.

use crate::codec::tga::TgaImage;
use crate::render::Surface;
use crate::sdl2::{self, Sdl};
use std::path::{Path, PathBuf};

const W: u32 = 1280;
const H: u32 = 720;

const PAGE_W: i32 = 1024;
const PAGE_H: i32 = 464;
const PAGE_X: i32 = (W as i32 - PAGE_W) / 2;
const PAGE_Y: i32 = 84;

const HEADER_H: i32 = 36;
const TAB_W: i32 = 192;
const TAB_H: i32 = 32;
const TABS_Y: i32 = 42;
const TABS_X: i32 = (W as i32 - TAB_W * 3) / 2;

const HELP_W: i32 = 944;
const HELP_H: i32 = 24;
const HELP_X: i32 = (W as i32 - HELP_W) / 2;
/// The guide-message bar is baked into the bottom of the page art (page rows
/// 438..458) and the strip text is pre-centred for it (text centre at strip
/// x 471 = bar centre), so the line belongs inside the bar, not below the page.
const HELP_Y: i32 = PAGE_Y + 435;
/// The 「向导信息 GUIDE MESSAGE」 label baked into the bar is covered while a
/// help line is showing, so long lines never run over it.
const GUIDE_X: i32 = PAGE_X + 36;
const GUIDE_Y: i32 = PAGE_Y + 440;
const GUIDE_W: i32 = 956;
const GUIDE_H: i32 = 19;

/// `sys_03020` is 432×128. The top 64 rows are three left-arrow states (48 px
/// apart) and then the label strip from x = 144. The bottom 64 rows are the
/// three right-arrow states. Drawing the whole top row piles those three states
/// on top of each other; the screen wants one arrow on each side of the strip.
const DEF_W: i32 = 208;
const DEF_H: i32 = 24;
/// 恢复默认 sits on its own. The page navigation that shared the bar with it
/// is gone, and the page grew into the space that freed up - see
/// [`FONT_BLOCK_H`].
const DEF_X: i32 = W as i32 - 128 - DEF_W;
const DEF_Y: i32 = 664;

/// Height of the font block inside page 1's 「文本设置」 group. The page
/// artwork is a black plate with no visible frame, so the rows the artwork laid
/// out below the split are blitted this much lower and the gap is filled with
/// the three font rows.
const FONT_BLOCK_H: i32 = 108;
/// Where page 1's artwork is cut: everything above stays put, everything below
/// moves down by [`FONT_BLOCK_H`]. The three font rows are the last entries of
/// the 文本设置 group, so the cut is after 「自动模式速度」 (its row ends at
/// page y = 201) and just before the 视觉设置 group header (y = 204); the band
/// therefore opens between the two groups instead of on a strip above the page.
const FONT_SPLIT: i32 = 204;

/// The three font rows, on the grid the rest of the page uses: the label
/// column ends at 448 and the two control columns are the artwork's own
/// 458..710 and 714..966 button slots. The row height and the plate are the
/// 252x36 button the page uses for its two-column options, so the new controls
/// read as part of the same UI instead of as custom flat plates.
const FROW_H: i32 = 34;
const FROW_PITCH: i32 = 36;
const FROW_Y0: i32 = 1;
const FROW_LABEL_X: i32 = 48;
const FROW_LABEL_W: i32 = 400;
const FROW_BOX_X: i32 = 458;
const FROW_BOX_W: i32 = 252;
const FROW_STYLE_X: i32 = 714;
const FROW_STYLE_W: i32 = 252;
/// The marker that says the box opens a list rather than stepping a value.
const FROW_DOWN_W: i32 = 22;

/// The list that opens under a font row.
const LST_X: i32 = 458;
const LST_W: i32 = 470;
/// Eleven rows keeps the panel inside the page even when it opens under the
/// last one.
const LST_ROW_H: i32 = 30;
const LST_ROWS: usize = 11;
const LST_HEAD: i32 = 26;
/// Footer strip: the font-only「恢复默认」 lives there, so the list owns the
/// operation that resets exactly the three fonts and nothing else.
const LST_FOOT: i32 = 22;
const LST_PAD: i32 = 8;
const LST_BAR_W: i32 = 10;
/// Name column, in list-local pixels from the left edge. The marker that says
/// which row is current sits at [`LST_TAG_X`], so the name has to be measured
/// and clipped to end before it.
const LST_NAME_W: i32 = 200;
const LST_TAG_X: i32 = 214;
/// Where the per-font sample starts, and how much room the scrollbar leaves it:
/// `LST_W - LST_BAR_W - LST_PAD - LST_PREVIEW_X` = 184 px, about 12 px of CJK
/// text. The sample is previewed at the largest size that fits that column, so
/// Chinese, kana and Latin are all on screen even for the widest face.
const LST_PREVIEW_X: i32 = 268;
const LST_PREVIEW_W: i32 = LST_W - LST_BAR_W - LST_PAD - LST_PREVIEW_X;
/// Hover/hit index of the footer entry; catalogue indices are smaller than
/// this and the scrollbar uses [`usize::MAX`].
const LST_RESET: usize = usize::MAX - 1;

/// Height of the drop list itself.
fn lst_h() -> i32 {
    LST_HEAD + LST_ROWS as i32 * LST_ROW_H + LST_FOOT + LST_PAD
}

/// Page y of font row i's row box. The rows are laid out in the band's own
/// coordinates (`font_row_y`); this maps them onto the page.
fn font_band_row_y(i: usize) -> i32 {
    FONT_SPLIT + font_row_y(i)
}

/// Where the list sits on the page, below the row that opened it. It is modal,
/// so when there is not enough room under the row it is clamped to the page
/// rather than covering the tab strip above it.
fn lst_y(row: usize) -> i32 {
    let want = font_band_row_y(row) + FROW_H + 2;
    want.min((PAGE_H - 4 - lst_h()).max(0)).max(0)
}

/// How much lower a page-1 row sits than the artwork does. Rows above the
/// split keep the artwork's own position; everything below it moves down by
/// the font block. Pages 2 and 3 never move.
fn insert_shift(page: usize, y: i32) -> i32 {
    if page == 0 && FONT_BLOCK_H != 0 && y >= FONT_SPLIT {
        FONT_BLOCK_H
    } else {
        0
    }
}

/// Screen y of a widget that the artwork placed at page y.
fn page_y(page: usize, y: i32) -> i32 {
    PAGE_Y + y + insert_shift(page, y)
}

/// The help line follows the page down, or it would sit in the middle of it.
fn help_y(page: usize) -> i32 {
    HELP_Y + page_shift(page)
}

fn guide_y(page: usize) -> i32 {
    GUIDE_Y + page_shift(page)
}

/// Geometry of a segmented slider bar, identical on every row: 21 segments of
/// 14 px on a 16 px pitch, the first one starting at x = 545 of the page image.
/// The sprite itself (dark in `sys_0310X`, lit in `sys_0320X`) is 334 px wide
/// and 24 px tall, so both copies are pixel aligned.
const BAR_X0: i32 = 545;
const BAR_X1: i32 = 879;
const BAR_H: i32 = 24;
const BAR_SEGMENTS: i32 = 21;
const BAR_PITCH: i32 = 16;

// ---------------------------------------------------------------------------
// settings model
// ---------------------------------------------------------------------------

/// Slider levels. The bars in the artwork are drawn with 20-ish segments; the
/// scripts' own settings use 0..10, which is what the original config stores.
pub const LEVELS: i32 = 10;

/// How long auto mode holds one line, in milliseconds.
///
/// `speed` is the 「自动模式速度」 slider: 0 is the fast end, [`LEVELS`] the
/// slow end. A longer line is held longer. Voiced lines are held at least
/// this long, and longer still while the voice is playing — that wait lives
/// in the game loop, which also checks the mixer.
pub fn auto_hold_ms(speed: i32, chars: usize) -> u32 {
    let speed = speed.clamp(0, LEVELS) as u32;
    let per_char = 30 + speed * 9;
    let floor = 200 + speed * 60;
    floor + (chars as u32).saturating_mul(per_char)
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Settings {
    /// 0 = windowed, 1 = fullscreen.
    pub win_disp: i32,
    /// 0 instant, 1 fast, 2 normal, 3 smooth.
    pub text_speed: i32,
    /// 0 = skip everything, 1 = skip only already-read text.
    pub text_skip: i32,
    /// 0..=LEVELS, 0 = FAST, LEVELS = SLOW. See [`auto_hold_ms`].
    pub auto_speed: i32,
    /// 0 instant, 1 normal.
    pub effect: i32,
    /// 0..=LEVELS opacity of the message window (normal scene).
    pub alpha_novel: i32,
    /// 0..=LEVELS opacity of the message window (CG scene).
    pub alpha_cg: i32,
    /// 0..=LEVELS opacity of the black backdrop in novel scenes.
    pub alpha_back: i32,
    /// 0..=LEVELS opacity of the command icons.
    pub alpha_command: i32,
    /// 0 = no yes/no confirmation, 1 = confirm.
    pub yes_no: i32,
    /// 0 hide window, 1 open the settings menu, 2 save.
    pub r_click: i32,
    /// all, bgm, se, voice gains (0..=LEVELS).
    pub vol: [i32; 4],
    /// 0 = let the voice finish, 1 = cut it when the page turns.
    pub page_voice: i32,
    /// 0 = play nothing, 1 = everything, 2 = female characters only.
    pub ero_voice: i32,
    /// Per character voice switch, one per row of page 3.
    pub char_voice: [i32; 10],
    /// Font catalogue ids for the three roles. Ids, never paths: the same
    /// value has to name the same font on another machine.
    pub font_home: i32,
    pub font_history: i32,
    pub font_game: i32,
    /// Dialogue body: size in 720p pixels, outline radius (0 follows the
    /// size), colour index, and whether lines carry a rule under them.
    pub dlg_size: i32,
    pub dlg_edge: i32,
    pub dlg_color: i32,
    pub dlg_under: i32,
    /// Outline coverage 0..=255 and synthetic stroke weight 0..=4: the rest of
    /// design §11's knobs, which used to be environment variables only.
    pub dlg_edge_alpha: i32,
    pub dlg_weight: i32,
    /// Letter tracking (-2..=6 px) and extra line pitch (0..=12 px).
    pub dlg_track: i32,
    pub dlg_line: i32,
    /// Font pack version the saved ids were written against. Not shown in the
    /// panel; recorded so a replaced pack can explain why an id moved.
    pub font_pack_version: i32,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            win_disp: 0,
            text_speed: 2,
            text_skip: 1,
            auto_speed: 5,
            effect: 1,
            alpha_novel: LEVELS,
            alpha_cg: LEVELS,
            // 「バックグラウンド不透明度（ノベルシーン時）」 defaults to 0 in
            // the original exe (WA2.exe reads win_color with default 0), i.e.
            // the background is not dimmed unless the player raises it.
            alpha_back: 0,
            alpha_command: LEVELS,
            yes_no: 1,
            r_click: 1,
            vol: [LEVELS, LEVELS, LEVELS, LEVELS],
            page_voice: 1,
            ero_voice: 2,
            char_voice: [1; 10],
            font_home: 0,
            font_history: 0,
            font_game: 0,
            dlg_size: crate::render::text::DialogStyle::default_style().size as i32,
            dlg_edge: 0,
            dlg_color: 0,
            dlg_under: 0,
            dlg_edge_alpha: crate::render::text::DEFAULT_EDGE_ALPHA as i32,
            dlg_weight: 0,
            dlg_track: 0,
            dlg_line: 0,
            font_pack_version: crate::render::text::FONT_PACK_VERSION,
        }
    }
}

/// The field a font key writes to, so `set` treats an id as an id rather than
/// as a slider level. The catalogue clamp lives in the font layer (`load`
/// normalizes what it reads back, `set_role_ids` clamps what it applies).
fn font_slot<'a>(st: &'a mut Settings, key: &str) -> Option<&'a mut i32> {
    match key {
        "font_home" => Some(&mut st.font_home),
        "font_history" => Some(&mut st.font_history),
        "font_game" => Some(&mut st.font_game),
        _ => None,
    }
}

/// The field a dialogue style key writes to.
fn dialog_slot<'a>(st: &'a mut Settings, key: &str) -> Option<&'a mut i32> {
    match key {
        "dlg_size" => Some(&mut st.dlg_size),
        "dlg_edge" => Some(&mut st.dlg_edge),
        "dlg_color" => Some(&mut st.dlg_color),
        "dlg_under" => Some(&mut st.dlg_under),
        "dlg_edge_alpha" => Some(&mut st.dlg_edge_alpha),
        "dlg_weight" => Some(&mut st.dlg_weight),
        "dlg_track" => Some(&mut st.dlg_track),
        "dlg_line" => Some(&mut st.dlg_line),
        _ => None,
    }
}

/// The dialogue style these settings describe, already ranged: the values
/// are stored as written and clamped here, so a hand-edited config still gives
/// a drawable size instead of a 1-pixel one.
pub fn dialog_style(st: &Settings) -> crate::render::text::DialogStyle {
    crate::render::text::DialogStyle {
        size: st.dlg_size.max(0) as u32,
        edge: st.dlg_edge.max(0) as u32,
        edge_alpha: st.dlg_edge_alpha.clamp(0, 255) as u32,
        color: st.dlg_color.max(0) as u32,
        underline: st.dlg_under > 0,
        weight: st.dlg_weight.max(0) as u32,
        tracking: st.dlg_track,
        line_gap: st.dlg_line.max(0),
    }
    .clamp()
}

/// `(ini key, getter-setter name)` pairs; keys are the ones the original exe
/// uses where they are known (`win_disp`, `wait`, `auto_max`, `win_alpha_*`,
/// `all_vol`, `page_voice`, `char_voice0..9`, ...).
impl Settings {
    fn get(&self, key: &str) -> Option<i32> {
        Some(match key {
            "win_disp" => self.win_disp,
            "wait" => self.text_speed,
            "msg_cut_optin" => self.text_skip,
            "auto_max" => self.auto_speed,
            "effect" => self.effect,
            "win_alpha_novel" => self.alpha_novel,
            "win_alpha_vis" => self.alpha_cg,
            "win_color" => self.alpha_back,
            "win_alpha_command" => self.alpha_command,
            "yes_no" => self.yes_no,
            "R_click" => self.r_click,
            "all_vol" => self.vol[0],
            "bgm_vol" => self.vol[1],
            "se_vol" => self.vol[2],
            "voice_vol" => self.vol[3],
            "page_voice" => self.page_voice,
            "ero_voice" => self.ero_voice,
            k if k.starts_with("char_voice") => {
                let i: usize = k["char_voice".len()..].parse().ok()?;
                *self.char_voice.get(i)?
            }
            "font_home" => self.font_home,
            "font_history" => self.font_history,
            "font_game" => self.font_game,
            "dlg_size" => self.dlg_size,
            "dlg_edge" => self.dlg_edge,
            "dlg_color" => self.dlg_color,
            "dlg_under" => self.dlg_under,
            "dlg_edge_alpha" => self.dlg_edge_alpha,
            "dlg_weight" => self.dlg_weight,
            "dlg_track" => self.dlg_track,
            "dlg_line" => self.dlg_line,
            "font_pack_version" => self.font_pack_version,
            _ => return None,
        })
    }

    fn set(&mut self, key: &str, v: i32) {
        // Font ids are hashes and the dialogue sizes are pixels: neither is a
        // slider level, so they are ranged by the layer that reads them
        // instead of by the generic clamp below.
        if let Some(slot) = font_slot(self, key) {
            *slot = v.max(0);
            return;
        }
        if let Some(slot) = dialog_slot(self, key) {
            // Stored as written and ranged where it is read (`dialog_style`),
            // the same rule the rest of the hand-editable keys follow.
            *slot = v;
            return;
        }
        if key == "font_pack_version" {
            self.font_pack_version = v.max(0);
            return;
        }
        let v = v.clamp(0, LEVELS);
        match key {
            "win_disp" => self.win_disp = v.min(1),
            "wait" => self.text_speed = v.min(3),
            "msg_cut_optin" => self.text_skip = v.min(1),
            "auto_max" => self.auto_speed = v,
            "effect" => self.effect = v.min(1),
            "win_alpha_novel" => self.alpha_novel = v,
            "win_alpha_vis" => self.alpha_cg = v,
            "win_color" => self.alpha_back = v,
            "win_alpha_command" => self.alpha_command = v,
            "yes_no" => self.yes_no = v.min(1),
            "R_click" => self.r_click = v.min(2),
            "all_vol" => self.vol[0] = v,
            "bgm_vol" => self.vol[1] = v,
            "se_vol" => self.vol[2] = v,
            "voice_vol" => self.vol[3] = v,
            "page_voice" => self.page_voice = v.min(1),
            "ero_voice" => self.ero_voice = v.min(2),
            k if k.starts_with("char_voice") => {
                if let Ok(i) = k["char_voice".len()..].parse::<usize>() {
                    if i < self.char_voice.len() {
                        self.char_voice[i] = v.min(1);
                    }
                }
            }
            _ => {}
        }
    }

    fn all_keys() -> Vec<String> {
        let mut v = [
            "win_disp",
            "wait",
            "msg_cut_optin",
            "auto_max",
            "effect",
            "win_alpha_novel",
            "win_alpha_vis",
            "win_color",
            "win_alpha_command",
            "yes_no",
            "R_click",
            "all_vol",
            "bgm_vol",
            "se_vol",
            "voice_vol",
            "page_voice",
            "ero_voice",
            "font_home",
            "font_history",
            "font_game",
            "dlg_size",
            "dlg_edge",
            "dlg_color",
            "dlg_under",
            "dlg_edge_alpha",
            "dlg_weight",
            "dlg_track",
            "dlg_line",
            "font_pack_version",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
        for i in 0..10 {
            v.push(format!("char_voice{}", i));
        }
        v
    }

    /// `WA2_INI`, else `wa2.ini` next to the data directory.
    pub fn path() -> PathBuf {
        if let Some(p) = std::env::var_os("WA2_INI") {
            return PathBuf::from(p);
        }
        match crate::data::Data::resolve() {
            Some(d) => d.root.join("wa2.ini"),
            None => PathBuf::from("wa2.ini"),
        }
    }

    pub fn load() -> Settings {
        let mut st = Settings::default();
        let path = Settings::path();
        let text = match std::fs::read(&path) {
            Ok(d) => d,
            Err(_) => {
                eprintln!("settings: {} not found, using defaults", path.display());
                // The defaults are not necessarily the font layer's initial
                // values (the default profile and style are resolved there),
                // so they still have to be handed over before the first draw.
                st.apply_text();
                return st;
            }
        };
        let text = String::from_utf8_lossy(&text).into_owned();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') || line.starts_with('[') {
                continue;
            }
            let (k, v) = match line.split_once('=') {
                Some(kv) => kv,
                None => continue,
            };
            if let Ok(n) = v.trim().parse::<i32>() {
                st.set(k.trim(), n);
            }
        }
        eprintln!("settings: loaded {}", path.display());
        // An id that names no font on this machine is normalized here, so the
        // file cannot keep (and re-save) an id that will never resolve. The
        // renderer clamps too, but the settings file is the source of truth.
        st.font_home = crate::render::text::clamp_profile_id(st.font_home);
        st.font_history = crate::render::text::clamp_profile_id(st.font_history);
        st.font_game = crate::render::text::clamp_profile_id(st.font_game);
        if st.font_pack_version != crate::render::text::FONT_PACK_VERSION {
            eprintln!(
                "font: font pack version {} -> {}; re-checking the saved ids against the current catalogue",
                st.font_pack_version,
                crate::render::text::FONT_PACK_VERSION
            );
            st.font_pack_version = crate::render::text::FONT_PACK_VERSION;
        }
        // Fonts have to be in place before the first screen draws, not when
        // the option screen is opened.
        st.apply_text();
        st
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = Settings::path();
        let mut out = String::from("; WHITE ALBUM 2 native port\n; keys follow the original wa2.ini\n");
        for k in Settings::all_keys() {
            if let Some(v) = self.get(&k) {
                out.push_str(&format!("{}={}\n", k, v));
            }
        }
        // Write a sibling and rename it over the target: a process that dies
        // mid-write leaves the old file behind rather than a half-written one.
        // The temporary name carries the pid so two instances cannot collide,
        // and the file is flushed before the rename so a hard power loss cannot
        // leave an empty but renamed config.
        let tmp = path.with_extension(format!("{}.ini.tmp", std::process::id()));
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(out.as_bytes())?;
            let _ = f.sync_all();
        }
        match std::fs::rename(&tmp, &path) {
            Ok(()) => Ok(()),
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                Err(e)
            }
        }
    }

    /// Hand the three font ids and the dialogue style to the font layer.
    /// Every cache whose role changed is dropped there, so no glyph of the
    /// previous font survives into the new one.
    pub fn apply_text(&self) {
        let changed = crate::render::text::set_role_ids([self.font_home, self.font_history, self.font_game]);
        crate::render::text::set_dialog_style(dialog_style(self));
        if changed {
            eprintln!(
                "settings: fonts home={} history={} game={}",
                self.font_home, self.font_history, self.font_game
            );
        }
        // A saved id whose font is gone is a silent fallback otherwise: the
        // player would think the choice took effect.
        for role in crate::render::text::FONT_ROLES {
            if let Some(reason) = crate::render::text::role_fallback_reason(role) {
                eprintln!(
                    "settings: {} font profile {} fell back: {}",
                    role.tag(),
                    crate::render::text::role_requested(role),
                    reason
                );
            }
        }
    }

    /// Linear gain for a mixer category, from the 0..=LEVELS slider value.
    pub fn gain(&self, cat: usize) -> f32 {
        let master = self.vol[0] as f32 / LEVELS as f32;
        let own = self.vol.get(cat).copied().unwrap_or(LEVELS) as f32 / LEVELS as f32;
        master * own
    }
}

// ---------------------------------------------------------------------------
// widget table
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Act {
    /// Set setting `key` to `val` (a radio button).
    Opt(&'static str, i32),
    /// Step a slider.
    Step(&'static str, i32),
    /// Click on the slider track: the value follows the pointer.
    Track,
    /// Jump to the slider's OFF position (0) - the 「窗口OFF」/「音量OFF」 button.
    Off(&'static str),
    /// Play a sample so the volume can be judged.
    Test,
    /// Restore every setting to its default.
    Reset,
    /// Switch to a page.
    Tab(usize),
    /// Open the dialogue style panel, from the font row.
    FontStyle,
    /// Leave the option screen.
    Close,
}

#[derive(Clone, Copy)]
pub struct Hot {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub act: Act,
    /// Which help line of `sys_0304X` to show while hovering this widget.
    pub row: usize,
    /// Slider this widget belongs to, for drawing the value indicator.
    pub slider: Option<&'static Slider>,
}

#[derive(Clone, Copy)]
pub struct Slider {
    /// Left edge of the first segment, in page coordinates.
    pub x0: i32,
    /// Right edge of the last segment.
    pub x1: i32,
    /// Top row of the bar.
    pub y: i32,
    pub key: &'static str,
}

const fn hot(x: i32, y: i32, w: i32, h: i32, act: Act, row: usize) -> Hot {
    Hot { x, y, w, h, act, row, slider: None }
}

const fn hot_sl(x: i32, y: i32, w: i32, h: i32, act: Act, row: usize, s: &'static Slider) -> Hot {
    Hot { x, y, w, h, act, row, slider: Some(s) }
}

// Page 1 - 画面设置. One help line per row (sys_03040 has 9).
const SL_AUTO: Slider = Slider { x0: BAR_X0, x1: BAR_X1, y: 158, key: "auto_max" };
const SL_ANOVEL: Slider = Slider { x0: BAR_X0, x1: BAR_X1, y: 262, key: "win_alpha_novel" };
const SL_ACG: Slider = Slider { x0: BAR_X0, x1: BAR_X1, y: 302, key: "win_alpha_vis" };
const SL_ABACK: Slider = Slider { x0: BAR_X0, x1: BAR_X1, y: 342, key: "win_color" };
const SL_ACMD: Slider = Slider { x0: BAR_X0, x1: BAR_X1, y: 382, key: "win_alpha_command" };

/// Widget index that asks for no help line. The page's help sheet has one
/// row per original setting and no row for the font panel, so hovering it
/// must not light up someone else's explanation.
const NO_ROW: usize = usize::MAX;

const PAGE1: &[Hot] = &[
    // 画面模式
    hot(458, 14, 252, 36, Act::Opt("win_disp", 0), 0),
    hot(714, 14, 252, 36, Act::Opt("win_disp", 1), 0),
    // 文本显示速度
    // The rects must cover the button frame: the gold "selected" version in
    // sys_0320X differs from the base only in the 2 px border, so a rect that
    // is inset by 4 px hides the highlight entirely.
    hot(458, 72, 124, 45, Act::Opt("wait", 0), 1),
    hot(586, 72, 124, 45, Act::Opt("wait", 1), 1),
    hot(714, 72, 124, 45, Act::Opt("wait", 2), 1),
    hot(842, 72, 124, 45, Act::Opt("wait", 3), 1),
    // 文本快进模式
    hot(458, 118, 252, 36, Act::Opt("msg_cut_optin", 0), 2),
    hot(714, 118, 252, 36, Act::Opt("msg_cut_optin", 1), 2),
    // 自动模式速度
    hot_sl(486, 162, 44, 28, Act::Step("auto_max", -1), 3, &SL_AUTO),
    hot_sl(894, 162, 44, 28, Act::Step("auto_max", 1), 3, &SL_AUTO),
    hot_sl(531, 156, 361, 36, Act::Track, 3, &SL_AUTO),
    // 画面特效
    hot(458, 222, 252, 36, Act::Opt("effect", 0), 4),
    hot(714, 222, 252, 36, Act::Opt("effect", 1), 4),
    // 窗口透明度(普通场景)
    hot(388, 272, 80, 26, Act::Off("win_alpha_novel"), 5),
    hot_sl(486, 266, 44, 28, Act::Step("win_alpha_novel", -1), 5, &SL_ANOVEL),
    hot_sl(894, 266, 44, 28, Act::Step("win_alpha_novel", 1), 5, &SL_ANOVEL),
    hot_sl(531, 261, 361, 36, Act::Track, 5, &SL_ANOVEL),
    // 窗口透明度(CG场景)
    hot(388, 302, 80, 26, Act::Off("win_alpha_vis"), 6),
    hot_sl(486, 306, 44, 28, Act::Step("win_alpha_vis", -1), 6, &SL_ACG),
    hot_sl(894, 306, 44, 28, Act::Step("win_alpha_vis", 1), 6, &SL_ACG),
    hot_sl(531, 300, 361, 36, Act::Track, 6, &SL_ACG),
    // 背景透明度(小说场景)
    hot_sl(486, 346, 44, 28, Act::Step("win_color", -1), 7, &SL_ABACK),
    hot_sl(894, 346, 44, 28, Act::Step("win_color", 1), 7, &SL_ABACK),
    hot_sl(531, 340, 361, 36, Act::Track, 7, &SL_ABACK),
    // 指令图标透明度
    hot_sl(486, 386, 44, 28, Act::Step("win_alpha_command", -1), 8, &SL_ACMD),
    hot_sl(894, 386, 44, 28, Act::Step("win_alpha_command", 1), 8, &SL_ACMD),
    hot_sl(531, 381, 361, 36, Act::Track, 8, &SL_ACMD),
];

// Page 2 - 系统设置 / 声音设置 / 暂停设置. sys_03041 has 5 help lines:
// 0 yes/no, 1 right click, 2 volume, 3 page voice, 4 H scene voice.
const SL_VALL: Slider = Slider { x0: BAR_X0, x1: BAR_X1, y: 118, key: "all_vol" };
const SL_VBGM: Slider = Slider { x0: BAR_X0, x1: BAR_X1, y: 158, key: "bgm_vol" };
const SL_VSE: Slider = Slider { x0: BAR_X0, x1: BAR_X1, y: 198, key: "se_vol" };
const SL_VVOICE: Slider = Slider { x0: BAR_X0, x1: BAR_X1, y: 238, key: "voice_vol" };

const PAGE2: &[Hot] = &[
    // 是/否确认: the left button is 「确认」, i.e. the feature is ON = 1.
    hot(458, 14, 252, 36, Act::Opt("yes_no", 1), 0),
    hot(714, 14, 252, 36, Act::Opt("yes_no", 0), 0),
    // 滑鼠右键功能
    hot(458, 48, 164, 47, Act::Opt("R_click", 0), 1),
    hot(630, 48, 164, 47, Act::Opt("R_click", 1), 1),
    hot(802, 48, 164, 47, Act::Opt("R_click", 2), 1),
    // 全体音量
    hot(390, 118, 80, 28, Act::Off("all_vol"), 2),
    hot_sl(486, 122, 44, 28, Act::Step("all_vol", -1), 2, &SL_VALL),
    hot_sl(894, 122, 44, 28, Act::Step("all_vol", 1), 2, &SL_VALL),
    hot_sl(531, 117, 361, 36, Act::Track, 2, &SL_VALL),
    // BGM 音量
    hot(390, 158, 80, 28, Act::Off("bgm_vol"), 2),
    hot_sl(486, 162, 44, 28, Act::Step("bgm_vol", -1), 2, &SL_VBGM),
    hot_sl(894, 162, 44, 28, Act::Step("bgm_vol", 1), 2, &SL_VBGM),
    hot_sl(531, 157, 361, 36, Act::Track, 2, &SL_VBGM),
    // SE 音量
    hot(390, 198, 80, 28, Act::Off("se_vol"), 2),
    hot_sl(486, 202, 44, 28, Act::Step("se_vol", -1), 2, &SL_VSE),
    hot_sl(894, 202, 44, 28, Act::Step("se_vol", 1), 2, &SL_VSE),
    hot_sl(531, 197, 361, 36, Act::Track, 2, &SL_VSE),
    hot(950, 202, 44, 28, Act::Test, 2),
    // VOICE 音量
    hot(390, 238, 80, 28, Act::Off("voice_vol"), 2),
    hot_sl(486, 242, 44, 28, Act::Step("voice_vol", -1), 2, &SL_VVOICE),
    hot_sl(894, 242, 44, 28, Act::Step("voice_vol", 1), 2, &SL_VVOICE),
    hot_sl(531, 237, 361, 36, Act::Track, 2, &SL_VVOICE),
    hot(950, 242, 44, 28, Act::Test, 2),
    // 翻页时中断语音: the left button is 「中断」= 1.
    hot(458, 302, 252, 36, Act::Opt("page_voice", 1), 3),
    hot(714, 302, 252, 36, Act::Opt("page_voice", 0), 3),
    // H 场景时播放语音: 「全部」= 1, 「仅限女性角色」= 2.
    hot(458, 342, 252, 36, Act::Opt("ero_voice", 1), 4),
    hot(714, 342, 252, 36, Act::Opt("ero_voice", 2), 4),
];

/// Page 3 has ten identical rows; built at run time because each one names a
/// different `char_voiceN` key.
fn page3() -> Vec<Hot> {
    let keys: [&'static str; 10] = [
        "char_voice0",
        "char_voice1",
        "char_voice2",
        "char_voice3",
        "char_voice4",
        "char_voice5",
        "char_voice6",
        "char_voice7",
        "char_voice8",
        "char_voice9",
    ];
    let mut v = Vec::with_capacity(20);
    for (i, k) in keys.iter().enumerate() {
        let y = 14 + i as i32 * 40;
        // Left button is 「语音开」 = 1, right is 「语音关」 = 0.
        v.push(hot(458, y, 252, 36, Act::Opt(k, 1), 0));
        v.push(hot(714, y, 252, 36, Act::Opt(k, 0), 0));
    }
    v
}

fn page_hots(page: usize) -> Vec<Hot> {
    match page {
        0 => PAGE1.to_vec(),
        1 => PAGE2.to_vec(),
        _ => page3(),
    }
}

/// Menu index of page 3 -> engine character id. `-1` is "other male voices"
/// and `-2` "other female voices"; those have no standing character here.
pub const CHAR_ROW_ID: [i32; 10] = [0, 2, 1, 3, 4, 5, 10, 11, -1, -2];

pub fn char_row_of(char_id: u32) -> Option<usize> {
    CHAR_ROW_ID.iter().position(|c| *c == char_id as i32)
}

// ---------------------------------------------------------------------------
// assets and drawing
// ---------------------------------------------------------------------------

/// Last dumped state, so `WA2_MENU_DUMP` only writes on changes.
static DUMP_SIG: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

pub struct Assets {
    base: [Option<TgaImage>; 3],
    /// Caption-free page art baked by `menu-clean`. When a page has one, the
    /// runtime blits it and skips the per-frame wipe entirely.
    clean: [Option<TgaImage>; 3],
    /// Caption-free copy of the hover/selected sheet (`sys_0321X`). It only
    /// contributes the state rim now: its plate was rebuilt from the clean
    /// page, so the baked captions of the original sheet cannot show through
    /// the vector text.
    hover: [Option<TgaImage>; 3],
    gold: [Option<TgaImage>; 3],
    cyan: [Option<TgaImage>; 3],
    header: Option<TgaImage>,
    tabs: Option<TgaImage>,
    nav: Option<TgaImage>,
    defbtn: Option<TgaImage>,
    help: [Option<TgaImage>; 3],
}

/// `Some("4x")` for a sibling art tree like `bg_4x`, `None` for an original.
fn tree_scale_suffix(dir: &Path) -> Option<&'static str> {
    let name = dir.file_name()?.to_str()?;
    for s in ["1x", "2x", "4x", "8x"] {
        if name.ends_with(&format!("_{}", s)) {
            return Some(match s {
                "1x" => "1x",
                "2x" => "2x",
                "4x" => "4x",
                _ => "8x",
            });
        }
    }
    None
}

fn load(dirs: &[&Path], name: &str) -> Option<TgaImage> {
    for dir in dirs {
        for cand in crate::codec::image::candidate_names(name) {
            let p = dir.join(&cand);
            if let Ok(img) = crate::codec::image::decode_ui_file(&p) {
                    return Some(img);
            }
        }
        // case-insensitive scan as a last resort
        if let Ok(rd) = std::fs::read_dir(dir) {
            let wants: Vec<String> = crate::codec::image::candidate_names(name)
                .into_iter().map(|n| n.to_ascii_lowercase()).collect();
            for e in rd.flatten() {
                if let Ok(n) = e.file_name().into_string() {
                    if wants.iter().any(|w| n.to_ascii_lowercase() == *w) {
                        if let Ok(img) = crate::codec::image::decode_ui_file(&e.path()) {
                            return Some(img);
                        }
                    }
                }
            }
        }
    }
    eprintln!("menu: {} NOT FOUND", name);
    None
}

impl Assets {
    pub fn load(dirs: &[&Path]) -> Assets {
        // The menu sheets are drawn at their logical 1280x720 size, and the
        // patch's Chinese artwork lives in the 1x trees. The 4x tree is a
        // runtime art tree produced by the upscaler: on the sound page its
        // AI-upgraded sheets lost the 音量ON/OFF glyphs, so the originals come
        // first here and 4x is only a last resort.
        let mut ordered: Vec<&Path> = Vec::new();
        ordered.extend(dirs.iter().copied().filter(|d| tree_scale_suffix(d).is_none()));
        ordered.extend(
            dirs.iter()
                .copied()
                .filter(|d| tree_scale_suffix(d).is_some_and(|s| s != "4x")),
        );
        ordered.extend(
            dirs.iter()
                .copied()
                .filter(|d| tree_scale_suffix(d) == Some("4x")),
        );
        let dirs: &[&Path] = &ordered;
        let cache = clean_dir();
        let clean = [
            read_clean(&cache, 0),
            read_clean(&cache, 1),
            read_clean(&cache, 2),
        ];
        let hover = [
            read_hover(&cache, 0),
            read_hover(&cache, 1),
            read_hover(&cache, 2),
        ];
        Assets {
            base: [
                load(dirs, "sys_03100.tga"),
                load(dirs, "sys_03101.tga"),
                load(dirs, "sys_03102.tga"),
            ],
            clean,
            hover,
            gold: [
                load(dirs, "sys_03200.tga"),
                load(dirs, "sys_03210.tga"),
                load(dirs, "sys_03220.tga"),
            ],
            cyan: [
                load(dirs, "sys_03201.tga"),
                load(dirs, "sys_03211.tga"),
                load(dirs, "sys_03221.tga"),
            ],
            header: load(dirs, "sys_03000.tga"),
            tabs: load(dirs, "sys_03011.tga"),
            nav: load(dirs, "sys_03020.tga"),
            defbtn: load(dirs, "sys_03030.tga"),
            help: [
                load(dirs, "sys_03040.tga"),
                load(dirs, "sys_03041.tga"),
                load(dirs, "sys_03042.tga"),
            ],
        }
    }

    pub fn available(&self) -> bool {
        self.base.iter().any(|b| b.is_some())
    }
}

/// Which overlay a widget should be drawn from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    Base,
    Gold,
    Cyan,
}

pub struct Screen {
    pub page: usize,
    /// Index into `page_hots(page)` of the widget under the cursor.
    pub hover: Option<usize>,
    /// Row whose help line is shown.
    pub help_row: Option<usize>,
    /// Hovering the navigation bar / default button.
    pub nav_hover: Option<usize>,
    /// Part of the font block under the cursor: a row or the style button.
    pub font_hover: Option<usize>,
    /// The font row the keyboard moves.
    pub font_sel: usize,
}

fn draw_page(
    s: &mut Surface,
    a: &Assets,
    page: usize,
    hots: &[Hot],
    st: &Settings,
    styles: &[Style],
) {
    // A baked clean page replaces the art and the wipe with one blit.
    let shift = page_shift(page);
    let src = a.clean[page].as_ref().or_else(|| a.base[page].as_ref());
    if let Some(b) = src {
        if shift > 0 {
            // Page 1 carries the font rows, so the artwork is cut at the split:
            // the strip above stays, everything below moves down, and the band
            // that opens up is theirs. The plate is black and so is the screen
            // behind it, so the seam does not show.
            s.blit_region(b, 0, 0, PAGE_W as u32, FONT_SPLIT as u32, PAGE_X, PAGE_Y);
            s.blit_region(
                b,
                0,
                FONT_SPLIT,
                PAGE_W as u32,
                (PAGE_H - FONT_SPLIT) as u32,
                PAGE_X,
                PAGE_Y + FONT_SPLIT + shift,
            );
        } else {
            s.blit(b, PAGE_X, PAGE_Y);
        }
    }
    // The baked state sheet carries the original captions inside every plate.
    // The cached copy has the plates rebuilt from the clean page, so only its
    // rims are new.
    let state = a.hover[page].as_ref().or_else(|| a.cyan[page].as_ref());
    // Slider values: the gold layer is a "lit" copy of the whole bar, so the
    // current level is the part of it that gets blitted back over the dark bar.
    let mut done: Vec<&str> = Vec::new();
    for h in hots {
        let sl = match h.slider {
            Some(sl) => sl,
            None => continue,
        };
        if done.contains(&sl.key) {
            continue;
        }
        done.push(sl.key);
        if let (Some(src), Some(w)) = (state, fill_width(st, sl)) {
            s.blit_region(src, sl.x0, sl.y, w as u32, BAR_H as u32, PAGE_X + sl.x0, page_y(page, sl.y));
        }
    }
    for (i, h) in hots.iter().enumerate() {
        let style = styles[i];
        // The font entry is drawn flat in the panel's own style; the gold and
        // cyan sheets are button frames for rows the artwork laid out.
        if style == Style::Base {
            continue;
        }
        let src = match style {
            // The value sheet is gold. Its rim and the body wash both read as
            // yellow, which fights the cyan page. Selected and hovered options
            // use the cyan sheet; the body wash tells them apart.
            Style::Gold | Style::Cyan => state,
            Style::Base => None,
        };
        if let Some(src) = src {
            // A hovered bar is drawn from the bright layer instead of the gold
            // one, but still only as far as the value goes, so dragging it keeps
            // showing the level instead of lighting up the whole bar.
            if style == Style::Cyan && matches!(h.act, Act::Track) {
                if let Some(sl) = h.slider {
                    if let Some(w) = fill_width(st, sl) {
                        s.blit_region(src, sl.x0, sl.y, w as u32, BAR_H as u32, PAGE_X + sl.x0, page_y(page, sl.y));
                    }
                    continue;
                }
            }
            s.blit_region(src, h.x.max(0), h.y.max(0), h.w as u32, h.h as u32, PAGE_X + h.x, page_y(page, h.y));
            // The gold and cyan sheets only recolour the rim. The interior is
            // the same dark plate as the base, so a selected option would
            // otherwise read as a thin outline. Wash the body, and leave the
            // label (the bright glyphs) alone.
            if matches!(h.act, Act::Opt(_, _)) {
                // Selected is the stronger cyan. Hover is the same hue, lighter,
                // so the current value stays obvious under the cursor too.
                // Three distinct states: idle is the untouched plate,
                // selected is a mid cyan, hover is the same hue a little
                // lighter. The old 175 washed the button out under a white
                // caption and a bright rim.
                let (tint, alpha) = if style == Style::Gold {
                    ([48u8, 196, 208], 132u8)
                } else {
                    ([36, 176, 196], 80)
                };
                fill_option_body(s, PAGE_X + h.x, page_y(page, h.y), h.w, h.h, tint, alpha);
            }
        }
    }
}

/// Tint the inside of an option button. Pixels bright enough to be the label
/// are left as they are, so the caption stays white on the wash.
fn fill_option_body(s: &mut Surface, x: i32, y: i32, w: i32, h: i32, tint: [u8; 3], alpha: u8) {
    let inset = 5;
    let a = alpha as u32;
    let ia = 255 - a;
    for yy in (y + inset)..(y + h - inset) {
        for xx in (x + inset)..(x + w - inset) {
            if xx < 0 || yy < 0 || xx >= s.width as i32 || yy >= s.height as i32 {
                continue;
            }
            let i = ((yy as u32) * s.width + xx as u32) as usize * 4;
            // No brightness test here on purpose. The label is drawn after this
            // wash and the baked caption underneath is wiped, so guessing
            // "is this pixel text" only left the caption's own halo behind.
            for c in 0..3 {
                s.pixels[i + c] = ((tint[c] as u32 * a + s.pixels[i + c] as u32 * ia) / 255) as u8;
            }
        }
    }
}

/// How many pixels of a slider bar are lit at the setting's current level.
///
/// The fill always stops on a segment boundary, so a level never leaves a
/// half-lit segment behind. Level 0 leaves the bar dark, which is what the
/// 「音量OFF」 button produces.
fn fill_width(st: &Settings, sl: &Slider) -> Option<i32> {
    let level = st.get(sl.key).unwrap_or(0).clamp(0, LEVELS);
    let lit = (level * BAR_SEGMENTS + LEVELS / 2) / LEVELS;
    if lit <= 0 {
        None
    } else {
        // The last lit segment is followed by a 2 px gap that stays dark.
        Some((lit * BAR_PITCH - 2).min(sl.x1 - sl.x0))
    }
}

/// Compose the whole option screen.
pub fn compose(s: &mut Surface, a: &Assets, st: &Settings, scr: &Screen) {
    let hots = page_hots(scr.page);
    if let Some(h) = a.header.as_ref() {
        s.blit(h, 0, 0);
    }
    // Tabs: column = page, row = state (0 current, 1 hover, 2 idle).
    if let Some(t) = a.tabs.as_ref() {
        for i in 0..3 {
            let state = if i == scr.page {
                0
            } else if scr.nav_hover == Some(NAV_TAB + i) {
                1
            } else {
                2
            };
            s.blit_region(
                t,
                i as i32 * TAB_W,
                state * TAB_H,
                TAB_W as u32,
                TAB_H as u32,
                TABS_X + i as i32 * TAB_W,
                TABS_Y,
            );
        }
    }
    let page = scr.page;
    let hover = scr.hover;
    let styles: Vec<Style> = hots
        .iter()
        .enumerate()
        .map(|(i, h)| {
            if hover == Some(i) {
                return Style::Cyan;
            }
            match h.act {
                Act::Opt(k, v) if st.get(k) == Some(v) => Style::Gold,
                _ => Style::Base,
            }
        })
        .collect();
    draw_page(s, a, page, &hots, st, &styles);

    // Help line for the row under the cursor. It follows the page down, or it
    // would sit in the middle of the rows on page 1.
    let hr = scr.help_row
        .or_else(|| hover.and_then(|i| (hots[i].row != NO_ROW).then_some(hots[i].row)));
    if let (Some(img), Some(row)) = (a.help[page].as_ref(), hr) {
        let y = (row as i32 * HELP_H).min(img.height as i32 - HELP_H).max(0);
        s.fill_rect(GUIDE_X, guide_y(page), GUIDE_W as u32, GUIDE_H as u32, [0, 0, 0, 255]);
        s.blit_region(img, 0, y, HELP_W as u32, HELP_H as u32, HELP_X, help_y(page));
    }

    // 恢复默认, on its own under the page.
    if let Some(d) = a.defbtn.as_ref() {
        let state = if scr.nav_hover == Some(NAV_DEF) { 2 } else { 0 };
        s.blit_region(d, 0, state * DEF_H, DEF_W as u32, DEF_H as u32, DEF_X, DEF_Y);
    }
}

// ---------------------------------------------------------------------------
// interaction
// ---------------------------------------------------------------------------

/// Hit test in screen coordinates. Returns (index, is_nav) - nav widgets use
/// Chrome widgets live above `NAV_BASE` so they can never collide with a page
/// widget index (an earlier version used 10..20, which clashed with the
/// eleventh page widget and made the navigation bar think it was hovered).
const NAV_BASE: usize = 100;
const NAV_TAB: usize = NAV_BASE;
const NAV_PREV: usize = NAV_BASE + 3;
const NAV_MID: usize = NAV_BASE + 4;
const NAV_NEXT: usize = NAV_BASE + 5;
const NAV_DEF: usize = NAV_BASE + 20;

fn is_nav(i: usize) -> bool {
    i >= NAV_BASE
}

fn hit(scr_page: usize, hots: &[Hot], sx: i32, sy: i32) -> Option<usize> {
    for (i, h) in hots.iter().enumerate() {
        let (x, y) = (PAGE_X + h.x, page_y(scr_page, h.y));
        if sx >= x && sx < x + h.w && sy >= y && sy < y + h.h {
            return Some(i);
        }
    }
    // tabs
    if sy >= TABS_Y && sy < TABS_Y + TAB_H {
        for i in 0..3 {
            let x = TABS_X + i as i32 * TAB_W;
            if sx >= x && sx < x + TAB_W {
                // Clicking the tab of the page we are already on does nothing.
                return if i == scr_page { None } else { Some(NAV_TAB + i) };
            }
        }
    }
    if sy >= DEF_Y && sy < DEF_Y + DEF_H && sx >= DEF_X && sx < DEF_X + DEF_W {
        return Some(NAV_DEF);
    }
    None
}

/// The slider track under a screen position, if any.
///
/// Page 1 carries the font block, so its artwork (and every widget on it) is
/// drawn [`page_shift`] lower: the drag has to use the same screen position
/// the click and the drawing use, or dragging a page 1 slider stops following
/// the pointer (F-15).
fn track_at(page: usize, sx: i32, sy: i32) -> Option<&'static Slider> {
    page_hots(page).into_iter().find_map(|h| {
        if !matches!(h.act, Act::Track) {
            return None;
        }
        let (x, y) = (PAGE_X + h.x, page_y(page, h.y));
        if sx >= x && sx < x + h.w && sy >= y && sy < y + h.h {
            h.slider
        } else {
            None
        }
    })
}

fn nav_act(idx: usize) -> Option<Act> {
    match idx {
        i if (NAV_TAB..NAV_TAB + 3).contains(&i) => Some(Act::Tab(i - NAV_TAB)),
        NAV_DEF => Some(Act::Reset),
        _ => None,
    }
}

/// One caption drawn with the system font, in 1280×720 coordinates.
/// Menu ink, kept apart from the dialogue's: these captions sit on rules,
/// buttons and dark plates, so they are lighter and flatter than text that has
/// to survive on top of a CG.
mod ink {
    /// Group heading.
    pub const HEAD: [u8; 3] = [186, 230, 240];
    /// Left-hand row label.
    pub const LABEL: [u8; 3] = [222, 236, 242];
    /// Button main text.
    pub const BUTTON: [u8; 3] = [255, 255, 255];
    /// Button second line, and the parenthetical notes.
    pub const SUB: [u8; 3] = [134, 178, 190];
    /// Slider scale (0%, 100%, MIN, MAX, FAST, SLOW).
    pub const SCALE: [u8; 3] = [152, 198, 210];
    /// Bottom navigation and the tab strip.
    pub const NAV: [u8; 3] = [236, 246, 250];
}

struct Lab {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    text: &'static str,
    sub: &'static str,
    size: u32,
    /// Size of the second line, when the sub text is itself Chinese.
    sub_size: u32,
    color: [u8; 3],
    sub_color: [u8; 3],
    /// Extra pixels between glyphs, in 1280x720 layout units. The original
    /// captions were letter-spaced; this is that spacing, done as tracking
    /// rather than as real spaces in the string (which would also change the
    /// measured width, the centring and the wrap).
    tracking: i32,
    center: bool,
    /// Pixels eaten off each edge before the baked glyphs are covered, so a
    /// button frame is not painted over.
    inset: i32,
    /// Fill the whole box. Used where the artwork has no dark pixel to sample
    /// (the tab face is brighter than its caption).
    fill: bool,
}

fn lab(
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    text: &'static str,
    sub: &'static str,
    size: u32,
    center: bool,
    inset: i32,
    fill: bool,
) -> Lab {
    Lab {
        x,
        y,
        w,
        h,
        text,
        sub,
        size,
        sub_size: (size * 2 / 3).max(11),
        color: ink::BUTTON,
        sub_color: ink::SUB,
        tracking: 0,
        center,
        inset,
        fill,
    }
}

/// The page whose labels are being built, so the helpers can add the same
/// shift the artwork got. Page 1 moves down for the font block; the others do
/// not move at all.
fn page_shift(page: usize) -> i32 {
    if page == 0 {
        FONT_BLOCK_H
    } else {
        0
    }
}

thread_local! {
    /// Set while `page_labs` runs, which is the one caller that knows the page.
    static LABEL_PAGE: std::cell::Cell<usize> = const { std::cell::Cell::new(1) };
}

fn label_page() -> usize {
    LABEL_PAGE.with(|c| c.get())
}

/// Where the label of a page-relative y lands.
fn label_y(y: i32) -> i32 {
    PAGE_Y + y + insert_shift(label_page(), y)
}

/// Group heading: 18px, a touch of tracking, the pale teal the section rules
/// are drawn in.
fn head(x: i32, y: i32, w: i32, text: &'static str) -> Lab {
    let mut l = lab(PAGE_X + x, label_y(y), w, 20, text, "", 18, false, 0, false);
    l.color = ink::HEAD;
    l.tracking = 1;
    l
}

/// Left-hand label of a row. 20px, no tracking: at 22 with a space between
/// every glyph it read looser than the button text it labels.
fn row(x: i32, y: i32, w: i32, h: i32, text: &'static str, sub: &'static str) -> Lab {
    let mut l = lab(PAGE_X + x, label_y(y), w, h, text, sub, 20, false, 0, false);
    l.color = ink::LABEL;
    l
}

/// Parenthetical note under a label.
fn note(x: i32, y: i32, w: i32, text: &'static str) -> Lab {
    let mut l = lab(PAGE_X + x, label_y(y), w, 16, text, "", 13, false, 0, false);
    l.color = ink::SUB;
    l
}

fn btn(x: i32, y: i32, w: i32, h: i32, text: &'static str, sub: &'static str) -> Lab {
    let mut l = lab(PAGE_X + x, label_y(y), w, h, text, sub, 17, true, 7, false);
    l.color = ink::BUTTON;
    l.sub_size = 11;
    l
}

/// Slider scale.
fn tiny(x: i32, y: i32, w: i32, h: i32, text: &'static str) -> Lab {
    let mut l = lab(PAGE_X + x, label_y(y), w, h, text, "", 10, true, 3, false);
    l.color = ink::SCALE;
    l
}

fn menu_px(layout_px: u32, window_h: i32) -> u32 {
    ((layout_px as i32 * window_h.max(1)) / 720).clamp(8, 64) as u32
}

fn page_labs(page: usize) -> Vec<Lab> {
    let mut v = Vec::new();
    LABEL_PAGE.with(|c| c.set(page));
    match page {
        0 => {
            v.push(head(36, 0, 220, "画面设置"));
            v.push(row(48, 20, 240, 28, "画面模式", ""));
            v.push(btn(458, 14, 252, 36, "瞬间显示", "Window"));
            v.push(btn(714, 14, 252, 36, "全屏模式", "Full Screen"));
            v.push(head(36, 54, 200, "文本设置"));
            v.push(row(40, 80, 250, 30, "文本显示速度", ""));
            v.push(btn(458, 72, 124, 45, "瞬间显示", "Moment"));
            v.push(btn(586, 72, 124, 45, "高速", "Fast"));
            v.push(btn(714, 72, 124, 45, "普通", "Normal"));
            v.push(btn(842, 72, 124, 45, "顺滑", "Smooth"));
            v.push(row(40, 122, 250, 30, "文本快进模式", ""));
            v.push(btn(458, 118, 252, 36, "全文", "All"));
            v.push(btn(714, 118, 252, 36, "仅限已读", "Already Read"));
            v.push(row(40, 162, 250, 28, "自动模式速度", ""));
            // Scales sit on their own row under the track, aligned to its
            // ends. They used to sit on the track's bottom edge.
            v.push(tiny(BAR_X0 - 16, SL_AUTO.y + BAR_H + 4, 44, 11, "FAST"));
            v.push(tiny(BAR_X1 - 44, SL_AUTO.y + BAR_H + 4, 48, 11, "SLOW"));
            v.push(head(36, 204, 200, "视觉设置"));
            v.push(row(48, 226, 240, 28, "画面特效", ""));
            v.push(btn(458, 222, 252, 36, "瞬间显示", "Fast"));
            v.push(btn(714, 222, 252, 36, "普通显示", "Normal"));
            v.push(row(40, 254, 220, 22, "窗口不透明度", ""));
            v.push(note(48, 278, 180, "（普通场景）"));
            v.push(row(40, 296, 220, 22, "窗口不透明度", ""));
            v.push(note(56, 320, 160, "（CG场景）"));
            v.push(row(40, 336, 220, 22, "背景不透明度", ""));
            v.push(note(48, 360, 180, "（小说场景）"));
            v.push(row(40, 386, 260, 26, "指令图标不透明度", ""));
            for sl in [SL_ANOVEL, SL_ACG, SL_ABACK, SL_ACMD] {
                let row_y = sl.y + BAR_H + 4;
                v.push(tiny(BAR_X0 - 16, row_y, 44, 11, "0%"));
                v.push(tiny(BAR_X1 - 44, row_y, 48, 11, "100%"));
            }
        }
        1 => {
            v.push(head(36, 0, 220, "系统设置"));
            v.push(row(40, 20, 240, 28, "是 / 否确认", ""));
            v.push(btn(458, 14, 252, 36, "确认", "Confirmation ON"));
            v.push(btn(714, 14, 252, 36, "不确认", "Confirmation OFF"));
            v.push(row(40, 56, 260, 30, "滑鼠右键功能", ""));
            v.push(btn(458, 48, 164, 47, "隐藏窗口", "Window OFF"));
            v.push(btn(630, 48, 164, 47, "设置菜单", "Option"));
            v.push(btn(802, 48, 164, 47, "存档", "Save"));
            v.push(head(36, 96, 200, "声音设置"));
            for (y, name) in [(118, "全体音量"), (158, "BGM音量"), (198, "SE音量"), (238, "VOICE音量")] {
                v.push(row(40, y + 2, 220, 28, name, ""));
                v.push(tiny(BAR_X0 - 16, y + BAR_H + 4, 44, 11, "MIN"));
                v.push(tiny(BAR_X1 - 44, y + BAR_H + 4, 48, 11, "MAX"));
            }
            v.push(head(36, 280, 200, "暂停设置"));
            v.push(row(40, 308, 270, 30, "翻页时中断语音", ""));
            v.push(btn(458, 302, 252, 36, "中断", "Stop"));
            v.push(btn(714, 302, 252, 36, "不中断", "Keep"));
            v.push(row(40, 348, 270, 30, "H场景时播放语音", ""));
            v.push(btn(458, 342, 252, 36, "全部", "Normal"));
            v.push(btn(714, 342, 252, 36, "仅限女性角色", "Female Voice Only"));
        }
        _ => {
            v.push(head(36, 0, 280, "角色语音设置"));
            let names = [
                "北原春希",
                "小木曾雪菜",
                "冬马和纱",
                "杉浦小春",
                "和泉千晶",
                "风冈麻理",
                "饭塚武也",
                "水泽依绪",
                "其他男性角色",
                "其他女性角色",
            ];
            for (i, name) in names.iter().enumerate() {
                let y = 14 + i as i32 * 40;
                v.push(row(70, y + 2, 260, 32, name, ""));
                v.push(btn(458, y, 252, 36, "语音开", "ON"));
                v.push(btn(714, y, 252, 36, "语音关", "OFF"));
            }
        }
    }
    v
}

fn chrome_labs() -> Vec<Lab> {
    // The page navigation bar is gone; 恢复默认 carries its own caption in the
    // artwork, so the chrome has nothing left to stamp.
    Vec::new()
}

fn help_lines(page: usize) -> &'static [&'static str] {
    match page {
        0 => &[
            "设置游戏时的画面模式。全屏模式时画面比例将固定为16:9",
            "设置文本窗口中文本的显示速度。设置为「瞬间显示」时将无特效。",
            "设置文本快进的条件。设置为「全文」时，未阅读的文本也能快进。",
            "设置开启自动模式时自动播放的速度。",
            "设置画面特效的表现方式。想要更快运行游戏请设置为「瞬间显示」。",
            "设置文本窗口的透明度。「窗口OFF」时文本窗口将完全消失。",
            "设置文本窗口的透明度。「窗口OFF」时文本窗口将完全消失。",
            "设置阅读小说时背景（黑色）的透明度。",
            "设置指令图标在通常情况下的透明度。",
        ],
        1 => &[
            "设置存档和读档时是否弹出确认窗口。",
            "设置滑鼠右键功能。",
            "设置各种音量。按下「音量OFF」键时，将维持当前音量设置并将音量降为0。可按「音符键」试听效果。",
            "设置语音播放的规则。设置为「中断」时，文本翻页时将强制中断语音。",
            "设置H场景中的语音播放状况。设置为「仅限女性角色」时，H场景中将不播放男性角色的语音。",
        ],
        _ => &["设置角色的语音是否开启。设置为OFF的情形下，和VOICE音量设置有关的角色的语音将一概不会播放。"],
    }
}

fn using_cn_art(dirs: &[&Path]) -> bool {
    dirs.iter().any(|d| d.file_name().and_then(|s| s.to_str()) == Some("bg_cn"))
}

fn px_of(img: &TgaImage, x: i32, y: i32) -> [u8; 3] {
    let i = ((y as u32 * img.width + x as u32) * 4) as usize;
    [img.rgba[i], img.rgba[i + 1], img.rgba[i + 2]]
}

/// Teal rules and the section ticks. Same colour family as the caption glow,
/// so they have to be recognised before any luminance test.
fn structural_teal(p: [u8; 3]) -> bool {
    p[0] < 60 && p[1] >= 75 && p[2] >= 75 && (p[1] as i32 - p[2] as i32).abs() < 30
}

fn bright_caption(p: [u8; 3]) -> bool {
    if structural_teal(p) {
        return false;
    }
    let m = p[0].max(p[1]).max(p[2]);
    if m >= 140 {
        return true;
    }
    // Dim Latin subtitles (SCREEN SETTING and the like). The rule is greener.
    p[2] > p[0] + 20 && p[2] + 5 > p[1] && p[1] < 75 && m >= 75
}

/// Button captions sit on the teal fill. Chinese is around (49,119,124) and
/// the English line under it is a little darker, around (34,107,111).
/// Snowflakes stay under r < 32.
fn button_caption(p: [u8; 3]) -> bool {
    p[0] >= 32 && p[0] < 180 && p[1] >= 96 && p[1] < 230 && p[2] >= 90
}

/// The English line under a button caption, around (26,103,105). Dimmer than
/// the Chinese stroke, still clearly above the teal plate.
fn dim_caption(p: [u8; 3]) -> bool {
    p[1] >= 98 && p[1] < 175 && p[2] >= 90 && p[0] < 90 && p[0] as i32 + 12 < p[1] as i32
}

/// Erase baked page captions, including the one-pixel halo around each stroke,
/// and put the colour underneath back. Rules, dotted leaders, snowflakes and
/// widget frames stay. The mask comes from the base art, so a selected
/// button's wash cannot hide the letters or get punched out.
/// Bake the caption-free page art once, instead of repairing the composed
/// frame on every state change.
///
/// `wipe_baked_captions` classifies pixels of the page art, grows the mask and
/// repaints each masked pixel from a neighbour. That is the right algorithm
/// but the wrong place for it: on every hover it walked the whole 1024x464
/// page and could still leave a halo where the classifier disagreed with the
/// artwork. Running it here, once, writes a clean page that the runtime only
/// blits; the wipe stays as the fallback for a data tree with no cache.
///
/// Returns the pages it wrote, so a caller can report what happened.
/// Where the baked caption-free pages live.
fn clean_dir() -> std::path::PathBuf {
    let root = std::env::var("WA2_DATA").unwrap_or_else(|_| "data".to_string());
    std::path::Path::new(&root).join("cache/ui/menu")
}

fn read_clean(dir: &Path, page: usize) -> Option<TgaImage> {
    let p = clean_page_path(dir, page);
    match crate::codec::image::decode_ui_file(&p) {
        Ok(img) => Some(img),
        Err(_) => None,
    }
}

fn read_hover(dir: &Path, page: usize) -> Option<TgaImage> {
    let p = hover_page_path(dir, page);
    match crate::codec::image::decode_ui_file(&p) {
        Ok(img) => Some(img),
        Err(_) => None,
    }
}

fn hover_page_path(out_dir: &Path, page: usize) -> std::path::PathBuf {
    out_dir.join(format!("page{}_hover.tga", page + 1))
}

/// The clean page for `page`: the art with its captions wiped, cropped to
/// the page rectangle. Split out from the file writing so a test can compare
/// it against the runtime path pixel for pixel.
pub fn bake_clean_page(base: &TgaImage, page: usize) -> Surface {
    // The wipe is written against the full screen with the page at
    // (PAGE_X, PAGE_Y), so bake on a full-size surface and crop the page
    // rectangle out of it. Doing it any other way would shift the art.
    let mut full = Surface::new(W, H, [0, 0, 0, 255]);
    full.blit(base, PAGE_X, PAGE_Y);
    wipe_baked_captions_flat(&mut full, base, page);
    let mut out = Surface::new(PAGE_W as u32, PAGE_H as u32, [0, 0, 0, 255]);
    for y in 0..PAGE_H as u32 {
        let src = ((PAGE_Y as u32 + y) * W + PAGE_X as u32) as usize * 4;
        let dst = (y * PAGE_W as u32) as usize * 4;
        let n = PAGE_W as usize * 4;
        out.pixels[dst..dst + n].copy_from_slice(&full.pixels[src..src + n]);
    }
    // The wipe is a classifier; it is bound to miss pixels where the artwork
    // disagrees. The caption regions have a known background (black plates for
    // the label column and the scale rows, a smooth gradient inside a button),
    // so rebuild those from the model instead of guessing (plan §1).
    rebuild_caption_regions(&mut out, page);
    out
}

/// Replace the baked captions of one page with the page's own background
/// model. Runs once, in the bake; the runtime only blits the result.
fn rebuild_caption_regions(s: &mut Surface, page: usize) {
    // 1) Caption column: the art behind the labels and their dotted leaders is
    //    black. Keep the dim teal heading bars and section rules. The leaders
    //    run all the way to the buttons on every page; the ON/OFF pills sit in
    //    that strip on pages 1 and 2 and are drawn as-is, so their rectangles
    //    (plus their glow) are skipped.
    let right = 480;
    let pills: Vec<(i32, i32, i32, i32)> = page_hots(page)
        .iter()
        .filter(|h| matches!(h.act, Act::Off(_)))
        .map(|h| (h.x - 5, h.y - 5, h.x + h.w + 5, h.y + h.h + 5))
        .collect();
    for y in 0..s.height as i32 {
        for x in 8..right.min(s.width as i32) {
            if pills.iter().any(|&(x0, y0, x1, y1)| x >= x0 && x < x1 && y >= y0 && y < y1) {
                continue;
            }
            if structural_run(s, x, y) {
                continue;
            }
            let i = ((y as u32 * s.width + x as u32) * 4) as usize;
            s.pixels[i] = 0;
            s.pixels[i + 1] = 0;
            s.pixels[i + 2] = 0;
            s.pixels[i + 3] = 255;
        }
    }
    // 2) Scale labels under each slider track (FAST/SLOW/0%/100%/MIN/MAX).
    //    The band runs from the track's bottom edge to just above the next
    //    row's track; the arrow buttons own the band's ends (their rims are
    //    white too), so their rectangles are skipped.
    let hots = page_hots(page);
    let mut seen: Vec<&'static str> = Vec::new();
    for h in &hots {
        let Some(sl) = h.slider else { continue };
        if seen.contains(&sl.key) {
            continue;
        }
        seen.push(sl.key);
        let arrows: Vec<(i32, i32, i32, i32)> = hots
            .iter()
            .filter(|a| {
                a.slider.map(|s| s.key) == Some(sl.key) && matches!(a.act, Act::Step(_, _))
            })
            .map(|a| (a.x, a.y, a.x + a.w, a.y + a.h))
            .collect();
        let y0 = sl.y + BAR_H;
        // Run right up to the next row's track. Its top edge is a horizontal
        // run, so the structure test keeps it; every faint label/noise pixel
        // between the tracks is wiped.
        let y1 = (sl.y + 40).min(s.height as i32);
        for y in y0..y1 {
            for x in 500..940.min(s.width as i32) {
                if arrows.iter().any(|&(ax0, ay0, ax1, ay1)| {
                    x >= ax0 - 6 && x < ax1 + 6 && y >= ay0 - 6 && y < ay1 + 6
                }) {
                    continue;
                }
                // No structure lives inside the band: the track edges are
                // above and at the band's end, so everything here is a baked
                // label, its faint leader line or the old value bar.
                let i = ((y as u32 * s.width + x as u32) * 4) as usize;
                s.pixels[i] = 0;
                s.pixels[i + 1] = 0;
                s.pixels[i + 2] = 0;
                s.pixels[i + 3] = 255;
            }
        }
    }
    // 3) Button plates: their captions sit on a smooth gradient, so the
    //    per-column median is the plate colour and glyphs stand out from it.
    for h in page_hots(page) {
        // Only option buttons. The ON/OFF pills are `Act::Off` and their baked
        // captions are already Chinese and always visible; scrubbing them left
        // empty plates.
        if !matches!(h.act, Act::Opt(_, _)) {
            continue;
        }
        scrub_button_caption(s, &h);
    }
}

/// Dim teal that is structure, not ink. The heading bars and rules are darker
/// than both the white captions and their brighter teal anti-aliasing.
fn dim_teal_structure(p: [u8; 3]) -> bool {
    let r = p[0] as i32;
    let g = p[1] as i32;
    let b = p[2] as i32;
    r < 90 && g > r + 25 && b > r + 25 && g < 135 && b < 135
}

fn clear_to_black(s: &mut Surface, x: i32, y: i32) {
    if x < 0 || y < 0 || x >= s.width as i32 || y >= s.height as i32 {
        return;
    }
    let i = ((y as u32 * s.width + x as u32) * 4) as usize;
    let p = [s.pixels[i], s.pixels[i + 1], s.pixels[i + 2]];
    if dim_teal_structure(p) {
        return;
    }
    s.pixels[i] = 0;
    s.pixels[i + 1] = 0;
    s.pixels[i + 2] = 0;
    s.pixels[i + 3] = 255;
}

/// A dim teal pixel only counts as structure when it belongs to a bar or a
/// rule. The dotted leaders are dim teal too, but they are 2-3 px clusters, so
/// a short run test tells them apart from the 20 px heading bars and the long
/// section rules.
fn structural_run(s: &Surface, x: i32, y: i32) -> bool {
    let w = s.width as i32;
    let h = s.height as i32;
    if x < 0 || y < 0 || x >= w || y >= h {
        return false;
    }
    let teal = |xx: i32, yy: i32| -> bool {
        if xx < 0 || yy < 0 || xx >= w || yy >= h {
            return false;
        }
        let i = ((yy as u32 * s.width + xx as u32) * 4) as usize;
        dim_teal_structure([s.pixels[i], s.pixels[i + 1], s.pixels[i + 2]])
    };
    if !teal(x, y) {
        return false;
    }
    let mut run = 1;
    for d in 1..=8 {
        if teal(x + d, y) || teal(x - d, y) {
            run += 1;
        } else {
            break;
        }
    }
    if run >= 8 {
        return true;
    }
    let mut run = 1;
    for d in 1..=8 {
        if teal(x, y + d) || teal(x, y - d) {
            run += 1;
        } else {
            break;
        }
    }
    run >= 8
}

/// Erase one baked caption from a button plate.
fn scrub_button_caption(s: &mut Surface, h: &Hot) {
    let inset = 5;
    let x0 = (h.x + inset).max(0);
    let x1 = (h.x + h.w - inset).min(s.width as i32);
    let y0 = (h.y + inset).max(0);
    let y1 = (h.y + h.h - inset).min(s.height as i32);
    if x1 - x0 < 16 || y1 - y0 < 10 {
        return;
    }
    // The snow decorations sit in the outer eighth of the plate; only the
    // centre carries the caption.
    let cut = ((x1 - x0) / 8).max(4);
    for x in (x0 + cut)..(x1 - cut) {
        let n = (y1 - y0) as usize;
        // Median of the column: the plate gradient is vertical, a glyph only
        // covers a few rows of it.
        let mut col = [0u8; 3];
        for c in 0..3 {
            let mut vals: Vec<u8> = (y0..y1)
                .map(|y| s.pixels[((y as u32 * s.width + x as u32) * 4) as usize + c])
                .collect();
            vals.sort_unstable();
            col[c] = vals[n / 2];
        }
        for y in y0..y1 {
            let i = ((y as u32 * s.width + x as u32) * 4) as usize;
            let p = [s.pixels[i], s.pixels[i + 1], s.pixels[i + 2]];
            let d = (0..3)
                .map(|c| (p[c] as i32 - col[c] as i32).abs())
                .max()
                .unwrap_or(0);
            if d > 16 {
                s.pixels[i] = col[0];
                s.pixels[i + 1] = col[1];
                s.pixels[i + 2] = col[2];
                s.pixels[i + 3] = 255;
            }
        }
    }
}

/// Clean copy of the hover/selected sheet: keep its rims, take the plates from
/// the clean page so no baked caption survives.
fn bake_clean_state(state: &TgaImage, clean: &Surface, page: usize) -> Surface {
    let mut s = Surface::new(state.width, state.height, [0, 0, 0, 255]);
    s.pixels.copy_from_slice(&state.rgba);
    for h in page_hots(page) {
        // Only option buttons: the ON/OFF pills keep this sheet's baked
        // caption (already Chinese).
        if !matches!(h.act, Act::Opt(_, _)) {
            continue;
        }
        let inset = 3;
        let x0 = (h.x + inset).max(0);
        let x1 = (h.x + h.w - inset).min(s.width as i32);
        let y0 = (h.y + inset).max(0);
        let y1 = (h.y + h.h - inset).min(s.height as i32);
        for y in y0..y1 {
            for x in x0..x1 {
                let si = ((y as u32 * clean.width + x as u32) * 4) as usize;
                let di = ((y as u32 * s.width + x as u32) * 4) as usize;
                s.pixels[di..di + 4].copy_from_slice(&clean.pixels[si..si + 4]);
            }
        }
    }
    s
}

pub fn bake_clean_pages(dirs: &[&Path], out_dir: &Path) -> Vec<usize> {
    let assets = Assets::load(dirs);
    let mut written = Vec::new();
    if std::fs::create_dir_all(out_dir).is_err() {
        eprintln!("menu: cannot create {}", out_dir.display());
        return written;
    }
    for page in 0..3 {
        let Some(base) = assets.base[page].as_ref() else { continue };
        let page_surf = bake_clean_page(base, page);
        let path = clean_page_path(out_dir, page);
        let mut ok = true;
        if let Err(e) = page_surf.save_tga(&path.to_string_lossy()) {
            eprintln!("menu: {}: {}", path.display(), e);
            ok = false;
        }
        if let Some(state) = assets.cyan[page].as_ref() {
            let hover = bake_clean_state(state, &page_surf, page);
            let path = hover_page_path(out_dir, page);
            if let Err(e) = hover.save_tga(&path.to_string_lossy()) {
                eprintln!("menu: {}: {}", path.display(), e);
                ok = false;
            }
        }
        if ok {
            written.push(page);
        }
    }
    written
}

/// The baked page for `page`, or None when the cache has not been built.
fn clean_page_path(out_dir: &Path, page: usize) -> std::path::PathBuf {
    out_dir.join(format!("page{}_clean.tga", page + 1))
}

thread_local! {
    /// How much lower the page being wiped sits than its artwork does, and the
    /// artwork y where that shift starts. Only page 1 moves, and only below the
    /// font block's split; the helpers below read page-image coordinates, so
    /// they learn the mapping from here.
    static WIPE_SHIFT: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
    static WIPE_SPLIT: std::cell::Cell<i32> = const { std::cell::Cell::new(i32::MAX) };
}

/// Screen y of row y of the page being wiped.
fn wipe_y(y: i32) -> i32 {
    let shift = WIPE_SHIFT.with(|c| c.get());
    let split = WIPE_SPLIT.with(|c| c.get());
    PAGE_Y + y + if shift > 0 && y >= split { shift } else { 0 }
}

/// Wipe the baked captions from the composited screen, where page 1's lower
/// artwork sits [`FONT_BLOCK_H`] lower than its source image.
fn wipe_baked_captions(s: &mut Surface, base: &TgaImage, page: usize) {
    WIPE_SHIFT.with(|c| c.set(page_shift(page)));
    WIPE_SPLIT.with(|c| c.set(FONT_SPLIT));
    wipe_baked_captions_impl(s, base, page);
}

/// Wipe the baked captions from an *unshifted* copy of the page artwork. The
/// clean-page bake works on the source image itself, so nothing may move.
fn wipe_baked_captions_flat(s: &mut Surface, base: &TgaImage, page: usize) {
    WIPE_SHIFT.with(|c| c.set(0));
    WIPE_SPLIT.with(|c| c.set(i32::MAX));
    wipe_baked_captions_impl(s, base, page);
}

fn wipe_baked_captions_impl(s: &mut Surface, base: &TgaImage, page: usize) {
    let pw = base.width as i32;
    let ph = base.height as i32;
    if pw <= 0 || ph <= 0 {
        return;
    }
    let n = (pw * ph) as usize;
    let mut mask = vec![false; n];
    let mut protect = vec![false; n];
    for y in 0..ph {
        for x in 0..pw {
            let p = px_of(base, x, y);
            let i = (y * pw + x) as usize;
            let m = p[0].max(p[1]).max(p[2]);
            // Near-white on the left is the caption itself. On the right it is
            // the widget frame, which has to survive.
            if structural_teal(p) || (x >= 430 && m >= 245) {
                protect[i] = true;
            }
        }
    }
    // The ON/OFF pills ship with Chinese captions and are drawn as-is; the
    // caption classifier below (which scans the left column) would punch holes
    // in them, so their rectangles are protected first.
    for h in page_hots(page) {
        if !matches!(h.act, Act::Off(_)) {
            continue;
        }
        let x0 = (h.x - 6).max(0);
        let y0 = (h.y - 6).max(0);
        let x1 = (h.x + h.w + 6).min(pw);
        let y1 = (h.y + h.h + 6).min(ph);
        for y in y0..y1 {
            for x in x0..x1 {
                protect[(y * pw + x) as usize] = true;
            }
        }
    }
    for y in 0..ph {
        for x in 0..(430.min(pw)) {
            let i = (y * pw + x) as usize;
            if !protect[i] && bright_caption(px_of(base, x, y)) {
                mask[i] = true;
            }
        }
    }
    for h in page_hots(page) {
        // Only option buttons. The ON/OFF pills (`Act::Off`) keep the baked
        // Chinese captions they already ship with.
        if !matches!(h.act, Act::Opt(_, _)) {
            continue;
        }
        let x0 = (h.x + 6).max(0);
        let y0 = (h.y + 6).max(0);
        let x1 = (h.x + h.w - 6).min(pw);
        let y1 = (h.y + h.h - 6).min(ph);
        for y in y0..y1 {
            for x in x0..x1 {
                let i = (y * pw + x) as usize;
                // The frame is outside this inset. A white pixel in here is a
                // caption, including on the gold and cyan sheets.
                protect[i] = false;
                let p = px_of(base, x, y);
                // Snowflakes sit on the left and right of the caption. The dim
                // English line is only in the middle, and its red channel drops
                // under the Chinese stroke, so the centre uses a wider test.
                let mid = x >= h.x + h.w / 6 && x < h.x + h.w - h.w / 6;
                if button_caption(p) || (mid && dim_caption(p)) {
                    mask[i] = true;
                }
            }
        }
    }
    // FAST / SLOW / 0% / 100% / MIN / MAX sit outside the option buttons.
    for lab in page_labs(page) {
        if !lab.sub.is_empty() || lab.w > 80 {
            continue;
        }
        let x0 = (lab.x - PAGE_X).max(0);
        let y0 = (lab.y - PAGE_Y).max(0);
        let x1 = (x0 + lab.w).min(pw);
        let y1 = (y0 + lab.h).min(ph);
        for y in y0..y1 {
            for x in x0..x1 {
                let i = (y * pw + x) as usize;
                let p = px_of(base, x, y);
                if !protect[i] && p[0] > 40 && p[1] > 80 && p[0].max(p[1]).max(p[2]) < 220 {
                    mask[i] = true;
                }
            }
        }
    }
    // Two pixels of halo, but not onto a rule or a frame.
    for _ in 0..3 {
        let prev = mask.clone();
        for y in 1..ph - 1 {
            for x in 1..pw - 1 {
                let i = (y * pw + x) as usize;
                if protect[i] || prev[i] {
                    continue;
                }
                let near = prev[i - 1] || prev[i + 1] || prev[(i as i32 - pw) as usize] || prev[(i as i32 + pw) as usize];
                if near {
                    mask[i] = true;
                }
            }
        }
    }
    for y in 0..ph {
        for x in 0..pw {
            if !mask[(y * pw + x) as usize] {
                continue;
            }
            let sx = PAGE_X + x;
            let sy = wipe_y(y);
            if let Some(c) = nearby_fill(s, &mask, pw, x, y) {
                if sx >= 0 && sy >= 0 && (sx as u32) < s.width && (sy as u32) < s.height {
                    let di = ((sy as u32 * s.width + sx as u32) * 4) as usize;
                    s.pixels[di] = c[0];
                    s.pixels[di + 1] = c[1];
                    s.pixels[di + 2] = c[2];
                    s.pixels[di + 3] = 255;
                }
            }
        }
    }
}

fn nearby_fill(s: &Surface, mask: &[bool], pw: i32, x: i32, y: i32) -> Option<[u8; 3]> {
    // Left of the widgets the page is black. Inside a button, take a pixel
    // from the same row so the vertical gradient stays and the caption is not
    // replaced by one flat dark rectangle.
    let want_black = x < 430;
    if !want_black {
        if let Some(c) = body_on_row(s, mask, pw, x, y) {
            return Some(c);
        }
    }
    let mut best: Option<(u32, [u8; 3])> = None;
    for rad in 1i32..24 {
        for dy in -rad..=rad {
            for dx in -rad..=rad {
                if dx.abs() != rad && dy.abs() != rad {
                    continue;
                }
                let nx = x + dx;
                let ny = y + dy;
                if nx < 0 || ny < 0 || nx >= pw || ny >= (mask.len() as i32 / pw) {
                    continue;
                }
                if mask[(ny * pw + nx) as usize] {
                    continue;
                }
                let sx = PAGE_X + nx;
                let sy = wipe_y(ny);
                if sx < 0 || sy < 0 || sx as u32 >= s.width || sy as u32 >= s.height {
                    continue;
                }
                let i = ((sy as u32 * s.width + sx as u32) * 4) as usize;
                let c = [s.pixels[i], s.pixels[i + 1], s.pixels[i + 2]];
                let sum = c[0] as u32 + c[1] as u32 + c[2] as u32;
                let good = if want_black { sum < 40 } else { (40..190).contains(&sum) };
                if good {
                    return Some(c);
                }
                if best.map(|(s0, _)| sum < s0).unwrap_or(true) {
                    best = Some((sum, c));
                }
            }
        }
    }
    best.map(|(_, c)| c)
}

/// Teal button fill on this row: darker than a caption, not the white frame.
fn body_on_row(s: &Surface, mask: &[bool], pw: i32, x: i32, y: i32) -> Option<[u8; 3]> {
    let rows = mask.len() as i32 / pw;
    let mut idle_hit = None;
    for dist in 1..160 {
        for nx in [x - dist, x + dist] {
            if nx < 0 || nx >= pw || y < 0 || y >= rows {
                continue;
            }
            if mask[(y * pw + nx) as usize] {
                continue;
            }
            let sx = PAGE_X + nx;
            let sy = wipe_y(y);
            if sx < 0 || sy < 0 || sx as u32 >= s.width || sy as u32 >= s.height {
                continue;
            }
            let i = ((sy as u32 * s.width + sx as u32) * 4) as usize;
            let c = [s.pixels[i], s.pixels[i + 1], s.pixels[i + 2]];
            let gb = (c[1] as i32 - c[2] as i32).abs();
            // A selected button's plate is the cyan wash. Prefer that over a
            // darker unwashed fringe, or the old English line is repainted as
            // a shadow.
            let washed = c[1] > 160 && c[0] < 90 && (c[0] as i32 + 40) < c[1] as i32 && gb < 30 && c[0].max(c[1]).max(c[2]) < 240;
            if washed {
                return Some(c);
            }
            let idle = c[0] < 30 && (45..95).contains(&c[1]) && gb < 25;
            if idle && idle_hit.is_none() {
                idle_hit = Some(c);
            }
        }
    }
    idle_hit
}

/// The chrome has nothing left to wipe: the tab strip ships the captions
/// the engine wants and the page navigation bar behind them is gone.
fn wipe_chrome_captions(_s: &mut Surface) {}

fn repaint_from_column(s: &mut Surface, src_x: i32, x: i32, y: i32, w: i32, h: i32) {
    if w <= 0 || h <= 0 {
        return;
    }
    for yy in y..y + h {
        let Some(bg) = surf_px(s, src_x, yy) else { continue };
        for xx in x..x + w {
            put_px(s, xx, yy, bg);
        }
    }
}

fn flatten_pill(s: &mut Surface, x: i32, y: i32, w: i32, h: i32) {
    if w <= 0 || h <= 0 {
        return;
    }
    let mut body = None;
    for yy in y..y + h {
        for xx in x..x + w {
            let Some(p) = surf_px(s, xx, yy) else { continue };
            if p[0] < 35 && p[1] > 50 && p[1] < 140 {
                body = Some(p);
                break;
            }
        }
        if body.is_some() {
            break;
        }
    }
    let body = body.unwrap_or([12, 70, 68]);
    s.fill_rect(x, y, w as u32, h as u32, [body[0], body[1], body[2], 255]);
}

fn surf_px(s: &Surface, x: i32, y: i32) -> Option<[u8; 3]> {
    if x < 0 || y < 0 || x >= s.width as i32 || y >= s.height as i32 {
        return None;
    }
    let i = ((y as u32 * s.width + x as u32) * 4) as usize;
    Some([s.pixels[i], s.pixels[i + 1], s.pixels[i + 2]])
}

fn put_px(s: &mut Surface, x: i32, y: i32, c: [u8; 3]) {
    if x < 0 || y < 0 || x >= s.width as i32 || y >= s.height as i32 {
        return;
    }
    let i = ((y as u32 * s.width + x as u32) * 4) as usize;
    s.pixels[i] = c[0];
    s.pixels[i + 1] = c[1];
    s.pixels[i + 2] = c[2];
}

struct MenuText {
    ww: i32,
    wh: i32,
    page: usize,
    /// Window-sized label layer, then one full-width row per help line, then
    /// the idle guide caption.
    buf: Vec<u8>,
    help_h: u32,
    help_n: usize,
}

fn build_menu_text(page: usize, ww: i32, wh: i32) -> Option<MenuText> {
    let help_h = menu_px(18, wh);
    let helps = help_lines(page);
    let extra = (helps.len() as u32 + 1) * help_h;
    let mut stamps = Vec::new();
    let mapx = |x: i32| x * ww / 1280;
    let mapy = |y: i32| y * wh / 720;
    let mapw = |x: i32, w: i32| (x + w) * ww / 1280 - x * ww / 1280;
    let maph = |y: i32, h: i32| (y + h) * wh / 720 - y * wh / 720;
    for lab in page_labs(page).iter().chain(chrome_labs().iter()) {
        let size = menu_px(lab.size, wh);
        let (bx, by, bw, bh) = (mapx(lab.x), mapy(lab.y), mapw(lab.x, lab.w), maph(lab.y, lab.h));
        // Tracking is authored in layout units and scaled with the window,
        // like every other measurement on this screen.
        let track = lab.tracking * wh / 720;
        // English under the Chinese (Window, Full Screen, …) is not drawn.
        // A second line is kept only when it is itself Chinese, such as 音量OFF.
        let sub_is_cjk = lab.sub.chars().any(|c| ('\u{3400}'..='\u{9fff}').contains(&c));
        if lab.sub.is_empty() || !sub_is_cjk {
            stamps.push(crate::render::text::Label {
                text: lab.text.to_string(),
                x: bx,
                y: by,
                w: bw,
                h: bh,
                size,
                center: lab.center,
                rgb: lab.color,
                tracking: track,
            });
        } else {
            // Main line on five eighths, sub line on the rest with a fixed gap
            // between them, so a two-line button does not drift with its height.
            let gap = (bh / 8).max(4);
            let top = ((bh - gap) * 5 / 8).max(1);
            let sub_h = (bh - top - gap).max(1);
            stamps.push(crate::render::text::Label {
                text: lab.text.to_string(),
                x: bx,
                y: by,
                w: bw,
                h: top,
                size,
                center: true,
                rgb: lab.color,
                tracking: track,
            });
            stamps.push(crate::render::text::Label {
                text: lab.sub.to_string(),
                x: bx,
                y: by + top + gap,
                w: bw,
                h: sub_h,
                size: menu_px(lab.sub_size, wh),
                center: true,
                rgb: lab.sub_color,
                tracking: 0,
            });
        }
    }
    // The help sheet (sys_0304X) already carries this text in Chinese, so
    // nothing is stamped over it. Drawing vector text on top produced two
    // different typefaces stacked on the same line.
    stamps.push(crate::render::text::Label {
        text: "向导信息".to_string(),
        x: mapx(GUIDE_X),
        y: wh + helps.len() as i32 * help_h as i32,
        w: 220,
        h: help_h as i32,
        size: menu_px(14, wh),
        center: false,
        rgb: ink::SUB,
        tracking: 0,
    });
    // One canvas for the whole page: the labels are re-tinted per frame, so
    // they have to come back as a buffer with the glyph coverage in alpha.
    let first = stamps.first().map(|l| l.size).unwrap_or(14);
    let buf = crate::render::text::with_ui_font(first, |f| {
        f.labels_rgba(ww as u32, wh as u32 + extra, &stamps)
    })??;
    Some(MenuText { ww, wh, page, buf, help_h, help_n: helps.len() })
}

/// Blit a label canvas with the colours it was built with.
///
/// The canvas is rendered once per page and window size, so the per-element
/// inks - pale teal for a heading, muted cyan-grey for a scale, white for a
/// button - travel in its own RGB channels. A single tint on the way out would
/// flatten all of them back to one colour, which is what made the page look
/// like it was assembled from two different screens.
/// Stamp one small RGBA buffer into a bigger one. The list uses it to put a
/// font's own sample on a canvas drawn with another.
/// `max_w` clips the source to a column: the font previews are stamped into
/// the list canvas and must never reach the scrollbar track (F-07).
fn blit_rgba_into(
    dst: &mut [u8],
    dw: u32,
    dh: u32,
    src: &[u8],
    sw: u32,
    sh: u32,
    x: i32,
    y: i32,
    max_w: i32,
) {
    for row in 0..sh as i32 {
        for col in 0..sw as i32 {
            if col >= max_w {
                break;
            }
            let (dx, dy) = (x + col, y + row);
            if dx < 0 || dy < 0 || dx >= dw as i32 || dy >= dh as i32 {
                continue;
            }
            let si = ((row as u32 * sw + col as u32) * 4) as usize;
            if src[si + 3] == 0 {
                continue;
            }
            let di = ((dy as u32 * dw + dx as u32) * 4) as usize;
            dst[di..di + 4].copy_from_slice(&src[si..si + 4]);
        }
    }
}

fn blit_ink(surf: &mut Surface, rgba: &[u8], w: u32, h: u32, x: i32, y: i32) {
    for row in 0..h as i32 {
        for col in 0..w as i32 {
            let i = ((row as u32 * w + col as u32) * 4) as usize;
            if i + 3 >= rgba.len() {
                return;
            }
            let px = [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]];
            if px[3] == 0 {
                continue;
            }
            surf.blend_pixel(x + col, y + row, px);
        }
    }
}

fn paint_menu_text(frame: &mut Surface, text: &MenuText, help_row: Option<usize>) {
    let ww = text.ww as u32;
    let wh = text.wh.max(0) as u32;
    let main = (wh as usize) * (ww as usize) * 4;
    if text.buf.len() >= main {
        blit_ink(frame, &text.buf[..main], ww, wh, 0, 0);
    }
    let row_bytes = (text.help_h as usize) * (ww as usize) * 4;
    let idx = match help_row {
        Some(r) if r < text.help_n => r,
        _ => text.help_n,
    };
    let start = main + idx * row_bytes;
    let end = start + row_bytes;
    if end <= text.buf.len() {
        let y = if help_row.is_some() {
            HELP_Y * text.wh / 720
        } else {
            GUIDE_Y * text.wh / 720
        };
        blit_ink(frame, &text.buf[start..end], ww, text.help_h, 0, y);
    }
}

// ---------------------------------------------------------------------------
// font rows on page 1
// ---------------------------------------------------------------------------

/// Page y of font row i.
fn font_row_y(i: usize) -> i32 {
    FROW_Y0 + i as i32 * FROW_PITCH
}

/// The rasterized text of the font block: one canvas for the three rows, so a
/// redraw that only moved the cursor does not re-stamp six labels.
struct FontRows {
    sig: String,
    w: u32,
    h: u32,
    buf: Vec<u8>,
}

/// Everything the band's captions depend on: the ids the settings file asks
/// for, what the font layer actually opened, whether it had to fall back, and
/// the window size (the canvas is rasterized at that size).
fn font_rows_sig(st: &Settings, scr: &Screen, ww: i32, wh: i32) -> String {
    let mut roles = String::new();
    for role in crate::render::text::FONT_ROLES {
        roles.push_str(&format!(
            "{}:{}:{};",
            crate::render::text::role_requested(role),
            crate::render::text::role_id(role),
            crate::render::text::role_fallback_reason(role).is_some(),
        ));
    }
    format!(
        "{:?}|{:?}|{}|{:?}|{:?}|{}|{}|{}x{}|{}",
        st.font_game,
        st.font_history,
        st.font_home,
        scr.font_hover,
        scr.font_sel,
        st.dlg_size,
        crate::render::text::role_generation(),
        ww,
        wh,
        roles
    )
}

/// Name of a catalogue entry, or the fallback note when the font it named is
/// no longer installed. An unknown id is not the default being missing: the
/// default is what took over, so say the requested face is gone (F-06).
fn font_name(id: i32) -> String {
    match crate::render::text::font_profile(id) {
        Some(p) if p.path.is_some() => p.name.clone(),
        Some(p) => format!("{}（未安装）", p.name),
        None => "默认（原字体未安装）".to_string(),
    }
}

/// What a font row's box says. A role that had to fall back says so and names
/// the id the file asked for, so the player cannot mistake the default for the
/// choice that took effect (F-06). The box is only ~300 px wide, so the
/// effective font's name is left out here and shown in the list header (see
/// [`font_fallback_note`]).
fn font_row_text(requested: i32, effective: i32, fallback: Option<&str>) -> String {
    if fallback.is_some() {
        format!("已使用备用字体（原 id {}）", requested)
    } else if requested != effective {
        // The file and the renderer can disagree when a role is overridden
        // from the environment: show what is drawing, not what was asked.
        format!("{}（请求 id {}）", font_name(effective), requested)
    } else {
        font_name(requested)
    }
}

/// The full fallback sentence for the list header: the font that is really
/// drawing and the id that was asked for. `None` when the role is drawing the
/// font its id names.
fn font_fallback_note(
    requested: i32,
    effective: i32,
    fallback: Option<&str>,
) -> Option<String> {
    fallback.map(|_| {
        if requested == 0 {
            // The usual cause is an unusable WA2_FONT* override; the settings
            // file itself says 0, so printing "original id 0" would be noise.
            format!("已使用备用字体：{}（环境变量字体不可用）", font_name(effective))
        } else {
            format!("已使用备用字体：{}（原 id {}）", font_name(effective), requested)
        }
    })
}

/// Clip one caption to `w` pixels with an ellipsis. `labels_rgba` does not
/// clip horizontally, so a long family name would otherwise run into the
/// current-value marker (F-19 m3).
fn fit_label(text: &str, w: i32, size: u32) -> String {
    if w <= 0 {
        return String::new();
    }
    crate::render::text::with_role_font(crate::render::text::FontRole::Home, size, |f| {
        if f.measure(text) <= w {
            return text.to_string();
        }
        let mut out = String::new();
        for ch in text.chars() {
            let mut cand = out.clone();
            cand.push(ch);
            cand.push('…');
            if f.measure(&cand) > w {
                break;
            }
            out.push(ch);
        }
        if out.is_empty() {
            // Not even one glyph and the ellipsis fit: leave the column blank
            // rather than paint over it.
            String::new()
        } else {
            out.push('…');
            out
        }
    })
    .unwrap_or_else(|| text.to_string())
}

/// Largest `size` in `min..=want` at which `text` fits `avail` pixels.
/// Returns `min` when even that overflows; the caller then clips the sample.
fn fit_sample_size(
    vf: &crate::render::text::VectorFont,
    text: &str,
    want: u32,
    min: u32,
    avail: i32,
) -> u32 {
    let mut size = want.max(min);
    loop {
        if vf.set_size(size) && vf.measure(text) <= avail {
            return size;
        }
        if size <= min {
            return min;
        }
        size -= 1;
    }
}

/// Clip a sample to `avail` pixels at the font's current size: the last resort
/// when even the smallest preview size overflows the column.
fn fit_text_with(vf: &crate::render::text::VectorFont, text: &str, avail: i32) -> String {
    if vf.measure(text) <= avail {
        return text.to_string();
    }
    let mut out = String::new();
    for ch in text.chars() {
        let mut cand = out.clone();
        cand.push(ch);
        cand.push('…');
        if vf.measure(&cand) > avail {
            break;
        }
        out.push(ch);
    }
    if !out.is_empty() {
        out.push('…');
    }
    out
}

fn build_font_rows(st: &Settings, scr: &Screen, ww: i32, wh: i32) -> Option<FontRows> {
    let px = |v: i32| (v * ww) / 1280;
    let py = |v: i32| (v * wh) / 720;
    let w = px(PAGE_W).max(1) as u32;
    let h = py(FONT_BLOCK_H).max(1) as u32;
    let mut labels: Vec<crate::render::text::Label> = Vec::new();
    let ids = [st.font_game, st.font_history, st.font_home];
    for (i, role) in crate::render::text::FONT_ROLES.iter().enumerate() {
        let y = font_row_y(i);
        let on = scr.font_sel == i || scr.font_hover == Some(i);
        labels.push(crate::render::text::Label {
            text: role.label().to_string(),
            x: px(FROW_LABEL_X),
            y: py(y),
            w: px(FROW_LABEL_W),
            h: py(FROW_H),
            size: menu_px(19, wh),
            center: false,
            rgb: if on { ink::HEAD } else { ink::LABEL },
            tracking: 0,
        });
        // The box names what the role is really drawing with. A fallback says
        // so, with the id the settings file still holds; the renderer's longer
        // explanation goes to the log, not into a 300 px row.
        let effective = crate::render::text::role_id(*role);
        let fallback = crate::render::text::role_fallback_reason(*role);
        let name = font_row_text(ids[i], effective, fallback.as_deref());
        let name_size = if fallback.is_some() { menu_px(14, wh) } else { menu_px(17, wh) };
        labels.push(crate::render::text::Label {
            text: fit_label(&name, px(FROW_BOX_W - FROW_DOWN_W) - px(6), name_size),
            x: px(FROW_BOX_X),
            y: py(y),
            w: px(FROW_BOX_W - FROW_DOWN_W),
            h: py(FROW_H),
            size: name_size,
            center: true,
            rgb: if fallback.is_some() { ink::SUB } else { ink::BUTTON },
            tracking: 0,
        });
        labels.push(crate::render::text::Label {
            text: "▼".to_string(),
            x: px(FROW_BOX_X + FROW_BOX_W - FROW_DOWN_W),
            y: py(y),
            w: px(FROW_DOWN_W),
            h: py(FROW_H),
            size: menu_px(14, wh),
            center: true,
            rgb: ink::SCALE,
            tracking: 0,
        });
        if i == 0 {
            labels.push(crate::render::text::Label {
                text: format!("样式 {}px", st.dlg_size),
                x: px(FROW_STYLE_X),
                y: py(y),
                w: px(FROW_STYLE_W),
                h: py(FROW_H),
                // Same bright button ink as the page's own option labels; the
                // style entry is a button, not a secondary note.
                size: menu_px(17, wh),
                center: true,
                rgb: ink::BUTTON,
                tracking: 0,
            });
        }
    }
    let size = menu_px(19, wh);
    let buf = crate::render::text::with_role_font(crate::render::text::FontRole::Home, size, |f| {
        f.labels_rgba(w, h, &labels)
    })??;
    Some(FontRows { sig: font_rows_sig(st, scr, ww, wh), w, h, buf })
}

/// Hand-drawn widget kit for the custom font UI.
///
/// The font rows, the font list and the dialogue-style panel are new controls
/// the original artwork has no plates for, so they are drawn by hand - but in
/// one deliberate flat style rather than by sampling patches of the page art:
/// a chamfered rectangle, a one-pixel cyan edge and three clearly separated
/// states (rest / hover / selected). Flat colours and crisp edges keep the
/// custom UI readable next to the vector text and avoid the dirty look copied
/// button textures got once they were stretched.
const UI_PANEL_FILL: [u8; 4] = [9, 14, 20, 248];
const UI_PANEL_EDGE: [u8; 4] = [92, 186, 204, 255];

#[derive(Clone, Copy, PartialEq, Eq)]
enum UiState {
    Idle,
    Hover,
    Selected,
}

fn ui_fill(state: UiState) -> [u8; 4] {
    match state {
        UiState::Idle => [20, 29, 38, 255],
        UiState::Hover => [30, 68, 84, 255],
        UiState::Selected => [36, 112, 132, 255],
    }
}

fn ui_edge(state: UiState) -> [u8; 4] {
    match state {
        UiState::Idle => [80, 132, 148, 255],
        UiState::Hover => [122, 214, 230, 255],
        UiState::Selected => [168, 236, 246, 255],
    }
}

/// Brighten (`pct` > 0) or darken (`pct` < 0) a colour by whole percent.
fn ui_shade(c: [u8; 4], pct: i32) -> [u8; 4] {
    let f = |v: u8| {
        let scaled = v as i32 + (v as i32 * pct) / 100;
        scaled.clamp(0, 255) as u8
    };
    [f(c[0]), f(c[1]), f(c[2]), c[3]]
}

/// One chamfered hand-drawn rectangle: a subtle top-to-bottom gradient body
/// plus a 1 px edge. The edge is drawn per row so the four corners are real
/// diagonal steps, not a blurred rounded rectangle. An edge with alpha 0 draws
/// the body alone (shadows).
fn ui_frame(
    frame: &mut Surface,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    fill: [u8; 4],
    edge: [u8; 4],
    cut: i32,
) {
    if w <= 1 || h <= 1 {
        return;
    }
    let cut = cut.clamp(0, (w - 1) / 2).clamp(0, (h - 1) / 2);
    let has_edge = edge[3] > 0;
    for row in 0..h {
        let inset = if cut <= 0 {
            0
        } else if row < cut {
            cut - row
        } else if row >= h - cut {
            row - h + cut + 1
        } else {
            0
        };
        let iw = w - inset * 2;
        if iw <= 0 {
            continue;
        }
        let ry = y + row;
        // A gentle vertical gradient: light on top, darker at the bottom. It
        // is drawn per row so it stays smooth at any window scale.
        let t = if h > 2 { (row * 255) / (h - 2) } else { 0 };
        let body = ui_shade(fill, 14 - (26 * t) / 255);
        if row == 0 || row == h - 1 {
            frame.fill_rect(x + inset, ry, iw as u32, 1, if has_edge { edge } else { body });
        } else {
            frame.fill_rect(x + inset, ry, iw as u32, 1, body);
            if has_edge {
                frame.fill_rect(x + inset, ry, 1, 1, edge);
                frame.fill_rect(x + w - 1 - inset, ry, 1, 1, edge);
            }
        }
    }
    if has_edge && h > 4 && w > 10 {
        // A single faint inner highlight along the top edge: enough to read as
        // a drawn bevel, not a texture.
        frame.fill_rect(x + cut + 2, y + 1, (w - (cut + 2) * 2).max(1) as u32, 1, [176, 236, 246, 48]);
    }
}

/// A hand-drawn panel: a soft drop shadow plus the chamfered plate.
fn ui_panel(frame: &mut Surface, x: i32, y: i32, w: i32, h: i32) {
    ui_frame(frame, x + 2, y + 3, w, h, [0, 0, 0, 120], [0, 0, 0, 0], 8);
    ui_frame(frame, x, y, w, h, UI_PANEL_FILL, UI_PANEL_EDGE, 8);
}

/// A hand-drawn button / value box in one of the three states.
fn ui_button(frame: &mut Surface, x: i32, y: i32, w: i32, h: i32, state: UiState) {
    ui_frame(frame, x, y, w, h, ui_fill(state), ui_edge(state), 5);
}





/// The row itself is hover index i; the style button lives past the rows in the
/// same field.
const FONT_STYLE_ROW: usize = 9;

/// Rects of the parts of the font block that can be clicked, in page
/// coordinates. The box and the row it labels are the same hit: both open the
/// list.
fn font_row_hits() -> Vec<(i32, i32, i32, i32, usize)> {
    let mut v = Vec::new();
    for i in 0..3 {
        let y = font_band_row_y(i);
        if i == 0 {
            v.push((FROW_STYLE_X, y, FROW_STYLE_W, FROW_H, FONT_STYLE_ROW));
        }
        v.push((FROW_LABEL_X, y, FROW_BOX_X + FROW_BOX_W - FROW_LABEL_X, FROW_H, i));
    }
    v
}

/// What a part of the font block does. A pick applies right away, the way
/// every other control on this page does.
enum FontHit {
    /// Open the font list for row i.
    List(usize),
    /// Open the dialogue style panel.
    Style,
}

fn font_hit_kind(h: usize) -> Option<FontHit> {
    match h {
        FONT_STYLE_ROW => Some(FontHit::Style),
        i if i < 3 => Some(FontHit::List(i)),
        _ => None,
    }
}

/// The slot a row writes to.
fn role_slot(st: &mut Settings, row: usize) -> Option<&mut i32> {
    let role = *crate::render::text::FONT_ROLES.get(row)?;
    Some(match role {
        crate::render::text::FontRole::Game => &mut st.font_game,
        crate::render::text::FontRole::History => &mut st.font_history,
        crate::render::text::FontRole::Home => &mut st.font_home,
    })
}

/// Give one role the picked font. Returns true when it changed.
fn pick_role_font(st: &mut Settings, row: usize, id: i32) -> bool {
    let Some(slot) = role_slot(st, row) else { return false };
    let next = crate::render::text::clamp_profile_id(id);
    if next == *slot {
        return false;
    }
    *slot = next;
    true
}
/// Draw the three hand-drawn font rows into the band above the artwork's own
/// rows.
///
/// `open` is the row whose list is currently on screen (F-19 m1): it was taken
/// out of `Screen` because the request to open a list lives for exactly one
/// frame, while the open state lives in the [`FontList`] itself.
fn paint_font_rows(
    frame: &mut Surface,
    ww: i32,
    wh: i32,
    scr: &Screen,
    open: Option<usize>,
    rows: Option<&FontRows>,
) {
    let px = |v: i32| (v * ww) / 1280;
    let py = |v: i32| (v * wh) / 720;
    let ox = px(PAGE_X);
    let oy = py(PAGE_Y);
    // The band starts at the split; the rows inside it keep their own grid.
    let band = oy + py(FONT_SPLIT);
    for i in 0..3 {
        let y = font_row_y(i);
        let is_open = open == Some(i);
        let state = if is_open || scr.font_hover == Some(i) {
            UiState::Hover
        } else if scr.font_sel == i {
            UiState::Selected
        } else {
            UiState::Idle
        };
        ui_button(
            frame,
            ox + px(FROW_BOX_X),
            band + py(y),
            px(FROW_BOX_W).max(2),
            py(FROW_H).max(2),
            state,
        );
        if i == 0 {
            let style_hot = scr.font_hover == Some(FONT_STYLE_ROW);
            ui_button(
                frame,
                ox + px(FROW_STYLE_X),
                band + py(y),
                px(FROW_STYLE_W).max(2),
                py(FROW_H).max(2),
                if style_hot { UiState::Hover } else { UiState::Idle },
            );
        }
    }
    if let Some(r) = rows {
        blit_ink(frame, &r.buf, r.w, r.h, ox, band);
    }
}


// ---------------------------------------------------------------------------
// font list
// ---------------------------------------------------------------------------

/// The open font list: which row it belongs to, the scroll offset, and the row
/// under the cursor. The catalogue itself lives in render::text.
struct FontList {
    /// Row whose list this is.
    row: usize,
    /// Index into the catalogue of the topmost visible row.
    top: usize,
    /// Cursor, as an index into the catalogue.
    cur: usize,
    /// The role's actually selected profile, as an index into the catalogue.
    /// The cursor follows the pointer, but「当前」has to keep naming the font
    /// that is really drawing, not the row the mouse happens to be over.
    current: usize,
    /// Row under the pointer, as an index into the catalogue.
    hover: Option<usize>,
    /// True while the scrollbar thumb is held.
    drag: bool,
    /// True while the pointer is over the font-only「恢复默认」footer.
    reset_hover: bool,
}

impl FontList {
    fn new(row: usize, current: i32) -> FontList {
        let cur = crate::render::text::font_profile_index(current);
        FontList {
            row,
            top: top_for(cur),
            cur,
            current: cur,
            hover: Some(cur),
            drag: false,
            reset_hover: false,
        }
    }

    /// Everything that changes the frame: the cursor and the pointer.
    fn sig(&self) -> String {
        format!(
            "{}|{}|{}|{:?}|{}|{}",
            self.row, self.top, self.cur, self.hover, self.drag as u8, self.reset_hover as u8
        )
    }

    /// The part that changes the text canvas. Hover and drag only re-tint the
    /// plates, so they must not rebuild the rasterized names and previews
    /// (F-20).
    fn key(&self) -> String {
        format!("{}|{}|{}|{}", self.row, self.top, self.cur, self.current)
    }

    fn len(&self) -> usize {
        crate::render::text::font_catalog().len()
    }

    /// Keep the cursor inside the visible window.
    fn clamp(&mut self) {
        if self.cur >= self.len() {
            self.cur = 0;
        }
        if self.cur < self.top {
            self.top = self.cur;
        }
        if self.cur >= self.top + LST_ROWS {
            self.top = self.cur + 1 - LST_ROWS;
        }
        let last = self.len().saturating_sub(LST_ROWS);
        if self.top > last {
            self.top = last;
        }
    }

    /// Move the cursor, scrolling the window when it leaves it.
    fn step(&mut self, d: i32) {
        let n = self.len() as i32;
        if n == 0 {
            return;
        }
        self.cur = ((self.cur as i32 + d + n) % n) as usize;
        self.hover = Some(self.cur);
        self.clamp();
    }

    /// Jump to an offset on the scrollbar track.
    fn scroll_to(&mut self, last: usize, want: usize) {
        self.top = want.min(last);
        // Keep the cursor on screen, or the keyboard would be moving a row
        // that is no longer visible.
        if self.cur < self.top {
            self.cur = self.top;
        }
        if self.cur >= self.top + LST_ROWS {
            self.cur = self.top + LST_ROWS - 1;
        }
        self.clamp();
    }
}

/// Open a list with the cursor on screen rather than at the very top.
fn top_for(cur: usize) -> usize {
    cur.saturating_sub(LST_ROWS / 2)
}

/// Rects of the list in page coordinates. `which` is an index into the
/// catalogue; [`usize::MAX`] marks the scrollbar and [`LST_RESET`] the
/// font-only reset in the footer.
fn list_hits(l: &FontList) -> Vec<(i32, i32, i32, i32, usize)> {
    let mut v = Vec::new();
    let top = lst_y(l.row) + LST_HEAD;
    for k in 0..LST_ROWS {
        v.push((LST_X, top + k as i32 * LST_ROW_H, LST_W - LST_BAR_W, LST_ROW_H, l.top + k));
    }
    v.push((LST_X + LST_W - LST_BAR_W, top, LST_BAR_W, LST_ROWS as i32 * LST_ROW_H, usize::MAX));
    v.push((
        LST_X + LST_PAD,
        top + LST_ROWS as i32 * LST_ROW_H,
        LST_W - LST_BAR_W - LST_PAD * 2,
        LST_FOOT,
        LST_RESET,
    ));
    v
}

/// Hit test the list, in page coordinates.
fn list_at(l: &FontList, x: i32, y: i32) -> Option<usize> {
    let ly = lst_y(l.row);
    if x < LST_X || x >= LST_X + LST_W || y < ly || y >= ly + lst_h() {
        return None;
    }
    list_hits(l)
        .iter()
        .find(|(hx, hy, hw, hh, _)| x >= *hx && x < *hx + *hw && y >= *hy && y < *hy + *hh)
        .map(|r| r.4)
}

/// Where a pointer y on the scrollbar track scrolls the list to. Shared by the
/// click that grabs the thumb and the drag that follows it.
fn scroll_from_bar(l: &mut FontList, last: usize, sy: i32) {
    if last == 0 {
        return;
    }
    let track = LST_ROWS as i32 * LST_ROW_H;
    let rel = (sy - PAGE_Y - (lst_y(l.row) + LST_HEAD)).clamp(0, track);
    l.scroll_to(last, (rel * last as i32 / (track - 1).max(1)) as usize);
}

/// What the list decided this frame.
enum ListAct {
    None,
    /// Close without picking.
    Close,
    /// Pick this catalogue id and close.
    Pick(i32),
    /// Put the three fonts back to the default profile and close.
    ResetFonts,
}

fn list_input(l: &mut FontList, input: sdl2::Input, win: &Sdl) -> ListAct {
    let n = l.len();
    let last = n.saturating_sub(LST_ROWS);
    match input {
        // Up/Down walk the catalogue; the page underneath never sees them
        // while the list is open (see `run`).
        sdl2::Input::Up => l.step(-1),
        sdl2::Input::Down => l.step(1),
        sdl2::Input::Wheel { y } => {
            let step = (y.abs() as i32).max(1) * 3;
            l.step(if y > 0 { -step } else { step });
        }
        // The number keys are page tabs on the option screen; the list gives
        // them no meaning rather than mixing two kinds of jump (F-19 m10).
        sdl2::Input::Advance => {
            let id = crate::render::text::font_catalog().get(l.cur).map(|p| p.id).unwrap_or(0);
            return ListAct::Pick(id);
        }
        sdl2::Input::Back | sdl2::Input::Left | sdl2::Input::RightClick | sdl2::Input::Quit => {
            return ListAct::Close;
        }
        // The thumb follows the pointer after the click that grabbed it; the
        // release drops it (F-19 m2).
        sdl2::Input::Drag { x, y } => {
            if l.drag {
                let (_, sy) = win.surface_pos(x, y, W, H);
                scroll_from_bar(l, last, sy);
            }
        }
        sdl2::Input::Release { .. } => l.drag = false,
        sdl2::Input::Pointer { x, y, pressed } => {
            let (sx, sy) = win.surface_pos(x, y, W, H);
            let px = sx - PAGE_X;
            let py = sy - PAGE_Y;
            match list_at(l, px, py) {
                // The scrollbar: jump to where it was clicked, and drag.
                Some(usize::MAX) => {
                    if pressed {
                        scroll_from_bar(l, last, sy);
                    }
                    l.drag = pressed;
                    l.hover = None;
                    l.reset_hover = false;
                }
                // Footer: the font-only reset. It closes the list, because the
                // page behind it has to be rebuilt with the new ids.
                Some(LST_RESET) => {
                    l.hover = None;
                    l.reset_hover = true;
                    if pressed {
                        return ListAct::ResetFonts;
                    }
                }
                Some(which) if which < n => {
                    l.hover = Some(which);
                    l.reset_hover = false;
                    l.cur = which;
                    l.clamp();
                    if pressed {
                        let id = crate::render::text::font_catalog()
                            .get(which)
                            .map(|p| p.id)
                            .unwrap_or(0);
                        return ListAct::Pick(id);
                    }
                }
                // Anywhere else on the page drops the list.
                _ => {
                    l.hover = None;
                    l.reset_hover = false;
                    if pressed {
                        return ListAct::Close;
                    }
                }
            }
        }
        _ => {}
    }
    ListAct::None
}

/// The sample every row is previewed with: Chinese, kana and Latin, so a font
/// that only covers part of the script shows it at once.
const LST_SAMPLE: &str = "春雪、かずさ、WHITE ALBUM 2";

/// Preview bitmaps, keyed by `(font id, requested pixel size, column width)`.
/// The key is known before any font is opened, so a scroll or a hover does not
/// reopen the eleven visible FreeType faces at all - relying on the glyph cache
/// alone would still pay the open cost every rebuild (F-20).
type PreviewCache = std::collections::HashMap<(i32, u32, i32), (u32, u32, Vec<u8>)>;

struct ListText {
    key: String,
    w: u32,
    h: u32,
    buf: Vec<u8>,
}

/// Build the list's text canvas. The names come from the UI font and each
/// preview from its own row's font, which is the only way to show what the
/// player is actually picking. Hover and drag are left to `paint_font_list`,
/// so moving the mouse does not rebuild this canvas (F-20).
fn build_list_text(
    l: &FontList,
    ww: i32,
    wh: i32,
    previews: &mut PreviewCache,
) -> Option<ListText> {
    let px = |v: i32| (v * ww) / 1280;
    let py = |v: i32| (v * wh) / 720;
    let w = px(LST_W).max(1) as u32;
    let h = py(lst_h()).max(1) as u32;
    let cat = crate::render::text::font_catalog();
    // A long session that resizes the window would otherwise keep one bitmap
    // set per column width alive; dropping them just costs a rebuild.
    if previews.len() > 256 {
        previews.clear();
    }
    let mut labels: Vec<crate::render::text::Label> = Vec::new();
    let header_w = px(LST_W - LST_BAR_W - LST_PAD * 2);
    // The row whose list this is may be drawing with a substitute font; the
    // inventory says which font that substitute is and what id it replaced
    // (F-06). It wins the header line because the count can be inferred from
    // the scrollbar.
    let header_note = crate::render::text::FONT_ROLES
        .get(l.row)
        .and_then(|r| {
            font_fallback_note(
                crate::render::text::role_requested(*r),
                crate::render::text::role_id(*r),
                crate::render::text::role_fallback_reason(*r).as_deref(),
            )
        })
        .map(|note| fit_label(&note, header_w, menu_px(14, wh)));
    let header_is_note = header_note.is_some();
    labels.push(crate::render::text::Label {
        text: header_note.unwrap_or_else(|| format!("字体 {} / {}", l.top + 1, cat.len())),
        x: px(LST_PAD),
        y: py(4),
        w: header_w,
        h: py(LST_HEAD - 6),
        size: menu_px(if header_is_note { 14 } else { 15 }, wh),
        center: false,
        rgb: ink::SUB,
        tracking: 0,
    });
    let name_size = menu_px(16, wh);
    let name_w = px(LST_NAME_W) - px(6);
    for k in 0..LST_ROWS {
        let i = l.top + k;
        let Some(p) = cat.get(i) else { continue };
        let y = LST_HEAD + k as i32 * LST_ROW_H;
        let current = l.current == i;
        let missing = p.path.is_none();
        labels.push(crate::render::text::Label {
            text: fit_label(&p.name, name_w, name_size),
            x: px(LST_PAD),
            y: py(y),
            w: px(LST_NAME_W),
            h: py(LST_ROW_H),
            size: name_size,
            center: false,
            rgb: if current { ink::HEAD } else { ink::LABEL },
            tracking: 0,
        });
        // The picked row is marked, and a font that cannot be loaded is called
        // out rather than silently offered.
        let tag = match (current, missing) {
            (true, _) => "当前",
            (false, true) => "未安装",
            _ => "",
        };
        if !tag.is_empty() {
            labels.push(crate::render::text::Label {
                text: tag.to_string(),
                x: px(LST_TAG_X),
                y: py(y),
                w: px(48),
                h: py(LST_ROW_H),
                size: menu_px(12, wh),
                center: false,
                rgb: ink::SCALE,
                tracking: 0,
            });
        }
    }
    // The footer entry: the only operation that resets exactly the three
    // fonts and leaves every other setting alone (F-08).
    labels.push(crate::render::text::Label {
        text: "恢复默认（三类字体）".to_string(),
        // Inside the native button plate drawn by `paint_font_list`, so the
        // caption starts after the plate's glowing border.
        x: px(LST_PAD + 14),
        y: py(LST_HEAD + LST_ROWS as i32 * LST_ROW_H),
        w: px(LST_W - LST_BAR_W - LST_PAD * 2 - 20),
        h: py(LST_FOOT),
        size: menu_px(14, wh),
        center: false,
        rgb: ink::BUTTON,
        tracking: 0,
    });
    let mut buf =
        crate::render::text::with_role_font(crate::render::text::FontRole::Home, name_size, |f| {
            f.labels_rgba(w, h, &labels)
        })??;
    // Previews, each in its own font: they are stamped onto the same canvas
    // after the labels, so a font's sample cannot be overwritten by another.
    // The size is picked per face so the whole sample fits the column, and the
    // width is clipped before the scrollbar so it can never cover the track.
    let want = menu_px(15, wh);
    let min = menu_px(9, wh);
    let avail = px(LST_PREVIEW_W) - px(4);
    let clip = px(LST_PREVIEW_W);
    for k in 0..LST_ROWS {
        let i = l.top + k;
        let Some(p) = cat.get(i) else { continue };
        if p.path.is_none() {
            continue;
        }
        let key = (p.id, want, avail);
        if let Some((pw, ph, rgba)) = previews.get(&key) {
            let y = LST_HEAD + k as i32 * LST_ROW_H + (LST_ROW_H - *ph as i32) / 2;
            blit_rgba_into(&mut buf, w, h, rgba, *pw, *ph, px(LST_PREVIEW_X), py(y), clip);
            continue;
        }
        let Some(vf) = crate::render::text::open_profile_font(p.id, want) else { continue };
        let size = fit_sample_size(&vf, LST_SAMPLE, want, min, avail);
        if !vf.set_size(size) {
            continue;
        }
        let sample = fit_text_with(&vf, LST_SAMPLE, avail);
        let Some((pw, ph, rgba)) = vf.line_rgba(&sample, size) else { continue };
        let y = LST_HEAD + k as i32 * LST_ROW_H + (LST_ROW_H - ph as i32) / 2;
        blit_rgba_into(&mut buf, w, h, &rgba, pw, ph, px(LST_PREVIEW_X), py(y), clip);
        previews.insert(key, (pw, ph, rgba));
    }
    Some(ListText { key: l.key(), w, h, buf })
}

/// Draw the hand-drawn font list over the page.
fn paint_font_list(frame: &mut Surface, ww: i32, wh: i32, l: &FontList, text: Option<&ListText>) {
    let px = |v: i32| (v * ww) / 1280;
    let py = |v: i32| (v * wh) / 720;
    let ox = px(PAGE_X);
    let oy = py(PAGE_Y);
    let (x, y, w, h) = (px(LST_X) + ox, py(lst_y(l.row)) + oy, px(LST_W), py(lst_h()));
    // The list is a modal like the dialogue-style panel: dim the page behind it.
    frame.fill_rect(0, 0, ww.max(1) as u32, wh.max(1) as u32, [0, 0, 0, 130]);
    ui_panel(frame, x, y, w, h);
    // A faintly lit header band, so the title line reads as part of the plate.
    frame.fill_rect(
        x + px(14),
        y + py(8),
        (w - px(28)).max(1) as u32,
        py(LST_HEAD).max(2) as u32,
        [140, 214, 228, 22],
    );
    // Rule between the count line and the first row.
    let head_y = y + py(LST_HEAD);
    frame.fill_rect(x + px(10), head_y - 1, (w - px(20)).max(1) as u32, 1, [70, 120, 134, 255]);
    let n = crate::render::text::font_catalog().len();
    let row_x = x + px(3);
    let row_w = (w - px(LST_BAR_W) - px(6)).max(2);
    let row_h = py(LST_ROW_H).max(2);
    for k in 0..LST_ROWS {
        let i = l.top + k;
        if i >= n {
            break;
        }
        let ry = head_y + py(k as i32 * LST_ROW_H);
        let state = if i == l.current {
            UiState::Selected
        } else if l.hover == Some(i) {
            UiState::Hover
        } else {
            UiState::Idle
        };
        if state != UiState::Idle {
            ui_button(frame, row_x, ry + py(1), row_w, py(LST_ROW_H - 2).max(2), state);
        } else if k > 0 {
            frame.fill_rect(
                x + px(8),
                ry,
                (w - px(LST_BAR_W + 16)).max(1) as u32,
                1,
                [38, 56, 66, 255],
            );
        }
    }
    // Footer: a hand-drawn button so it reads as an action rather than a row.
    let fy = head_y + py(LST_ROWS as i32 * LST_ROW_H);
    ui_button(
        frame,
        row_x,
        fy,
        row_w,
        py(LST_FOOT).max(2),
        if l.reset_hover { UiState::Hover } else { UiState::Idle },
    );
    // Scrollbar: a dark track and a thumb that shows where the window sits.
    let bx = x + w - px(LST_BAR_W).max(1);
    let by = y + py(LST_HEAD);
    let bh = py(LST_ROWS as i32 * LST_ROW_H).max(1);
    let bw = px(LST_BAR_W).max(2);
    frame.fill_rect(bx, by, bw as u32, bh as u32, [8, 12, 18, 255]);
    if n > LST_ROWS {
        let last = n - LST_ROWS;
        let track = (bh as f32) - 14.0;
        let thumb = (track * LST_ROWS as f32 / n as f32).max(14.0);
        let ty = by as f32 + (track - thumb) * l.top as f32 / last as f32;
        ui_frame(
            frame,
            bx + 1,
            ty as i32,
            (bw - 2).max(2),
            thumb.max(2.0) as i32,
            if l.drag { [72, 156, 176, 255] } else { [52, 112, 130, 255] },
            if l.drag { [150, 226, 238, 255] } else { [86, 158, 176, 255] },
            2,
        );
    }
    if let Some(t) = text {
        blit_ink(frame, &t.buf, t.w, t.h, x, y);
    }
}


// ---------------------------------------------------------------------------
// dialogue style panel
// ---------------------------------------------------------------------------

const SP_W: i32 = 600;
/// Eight rows: size, outline weight, outline alpha, colour, weight, tracking,
/// line pitch, underline. 472 px leaves room for the title, the rows on their
/// 44 px pitch and the button row inside a 720 px screen.
const SP_H: i32 = 472;
const SP_X: i32 = (W as i32 - SP_W) / 2;
const SP_Y: i32 = (H as i32 - SP_H) / 2;
const SP_ROW_Y: i32 = SP_Y + 66;
const SP_ROW_PITCH: i32 = 44;
const SP_ROW_H: i32 = 32;
const SP_LABEL_X: i32 = SP_X + 36;
const SP_LABEL_W: i32 = 150;
const SP_BOX_X: i32 = SP_X + 200;
const SP_BOX_W: i32 = 150;
const SP_ARROW_L: i32 = SP_BOX_X + SP_BOX_W + 8;
const SP_ARROW_R: i32 = SP_ARROW_L + 34;
const SP_ARROW_W: i32 = 30;
const SP_BTN_Y: i32 = SP_Y + SP_H - 52;
const SP_BTN_H: i32 = 32;
const SP_RESET_X: i32 = SP_X + 36;
const SP_CLOSE_X: i32 = SP_X + SP_W - 36 - 110;

/// The style knobs, in the panel's order. They are all of design §11's dials
/// and all of them only touch the dialogue body.
const SP_ROWS: usize = 8;
const SP_NAMES: [&str; SP_ROWS] = ["字号", "描边强度", "描边透明度", "颜色", "字重", "字距", "行距", "下划线"];

/// Widget indices: [`SP_ROWS`] rows each with a left and a right arrow, then
/// the two buttons.
const SP_PREV0: usize = 0;
const SP_NEXT0: usize = SP_ROWS;
const SP_RESET: usize = SP_ROWS * 2;
const SP_CLOSE: usize = SP_ROWS * 2 + 1;

/// The dialogue style being edited.
struct StylePanel {
    sel: usize,
    hover: Option<usize>,
    style: crate::render::text::DialogStyle,
}

impl StylePanel {
    fn new(st: &Settings) -> StylePanel {
        StylePanel { sel: 0, hover: None, style: dialog_style(st) }
    }

    fn sig(&self) -> String {
        format!("{:?}|{:?}|{}", self.style, self.hover, self.sel)
    }

    /// Step the value of row i, wrapping. Row 7 is a switch, so both
    /// directions toggle it.
    fn step(&mut self, i: usize, dir: i32) {
        use crate::render::text::{
            DIALOG_COLORS, DIALOG_EDGE_MAX, DIALOG_LINE_GAP_MAX, DIALOG_SIZE_MAX,
            DIALOG_SIZE_MIN, DIALOG_TRACK_MAX, DIALOG_TRACK_MIN, DIALOG_WEIGHT_MAX,
        };
        match i {
            0 => {
                let n = (DIALOG_SIZE_MAX - DIALOG_SIZE_MIN + 1) as i32;
                let cur = self.style.size as i32 - DIALOG_SIZE_MIN as i32;
                let next = (cur + dir + n) % n;
                self.style.size = DIALOG_SIZE_MIN + next as u32;
            }
            1 => {
                let n = DIALOG_EDGE_MAX as i32 + 1;
                self.style.edge = ((self.style.edge as i32 + dir + n) % n) as u32;
            }
            2 => {
                // Outline coverage in 16-px steps, clamped rather than wrapped:
                // no outline and a fully opaque one are both useful resting
                // points, so the arrows stop at them.
                let v = self.style.edge_alpha as i32 + dir * 16;
                self.style.edge_alpha = v.clamp(0, 255) as u32;
            }
            3 => {
                let n = DIALOG_COLORS.len() as i32;
                self.style.color = ((self.style.color as i32 + dir + n) % n) as u32;
            }
            4 => {
                let n = DIALOG_WEIGHT_MAX as i32 + 1;
                self.style.weight = ((self.style.weight as i32 + dir + n) % n) as u32;
            }
            5 => {
                let n = DIALOG_TRACK_MAX - DIALOG_TRACK_MIN + 1;
                let cur = self.style.tracking - DIALOG_TRACK_MIN;
                self.style.tracking = DIALOG_TRACK_MIN + (cur + dir + n) % n;
            }
            6 => {
                let n = DIALOG_LINE_GAP_MAX + 1;
                self.style.line_gap = (self.style.line_gap + dir + n) % n;
            }
            _ => self.style.underline = !self.style.underline,
        }
    }
}

/// The value column's text for row i. Kept apart from the canvas builder so the
/// labels can be checked without a window.
fn style_value_text(style: &crate::render::text::DialogStyle, i: usize) -> String {
    use crate::render::text::{DIALOG_COLOR_NAMES, DIALOG_COLORS};
    match i {
        0 => format!("{} px", style.size),
        1 => {
            if style.edge == 0 {
                "自动".to_string()
            } else {
                format!("{} px", style.edge)
            }
        }
        2 => format!("{} / 255", style.edge_alpha),
        3 => DIALOG_COLOR_NAMES[(style.color as usize).min(DIALOG_COLORS.len() - 1)].to_string(),
        4 => {
            if style.weight == 0 {
                "字体自带".to_string()
            } else {
                format!("+{} px", style.weight)
            }
        }
        5 => {
            if style.tracking == 0 {
                "0 px".to_string()
            } else {
                format!("{:+} px", style.tracking)
            }
        }
        6 => format!("{} px", style.line_gap),
        _ => {
            if style.underline {
                "开".to_string()
            } else {
                "关".to_string()
            }
        }
    }
}

fn style_panel_hits() -> Vec<(i32, i32, i32, i32, usize)> {
    let mut v = Vec::new();
    for i in 0..SP_ROWS {
        let y = SP_ROW_Y + i as i32 * SP_ROW_PITCH;
        v.push((SP_ARROW_L, y, SP_ARROW_W, SP_ROW_H, SP_PREV0 + i));
        v.push((SP_ARROW_R, y, SP_ARROW_W, SP_ROW_H, SP_NEXT0 + i));
        v.push((SP_LABEL_X, y, SP_BOX_X + SP_BOX_W - SP_LABEL_X, SP_ROW_H, i));
    }
    v.push((SP_RESET_X, SP_BTN_Y, 130, SP_BTN_H, SP_RESET));
    v.push((SP_CLOSE_X, SP_BTN_Y, 110, SP_BTN_H, SP_CLOSE));
    v
}

/// What the panel decided this frame.
enum StyleAct {
    None,
    Close,
    /// Write the style back to the settings.
    Apply(crate::render::text::DialogStyle),
}

fn style_input(p: &mut StylePanel, input: sdl2::Input, win: &Sdl) -> StyleAct {
    match input {
        sdl2::Input::Up => p.sel = (p.sel + SP_ROWS - 1) % SP_ROWS,
        sdl2::Input::Down => p.sel = (p.sel + 1) % SP_ROWS,
        // Every value change is applied and saved at once: the style panel is
        // a live preview, not a form with an OK button, so Esc/关闭 must not
        // be able to throw a change away.
        sdl2::Input::Right => {
            p.step(p.sel, 1);
            return StyleAct::Apply(p.style);
        }
        sdl2::Input::Left => {
            p.step(p.sel, -1);
            return StyleAct::Apply(p.style);
        }
        sdl2::Input::Advance => return StyleAct::Apply(p.style),
        sdl2::Input::Back | sdl2::Input::RightClick | sdl2::Input::Quit => return StyleAct::Close,
        sdl2::Input::Pointer { x, y, pressed } => {
            let (sx, sy) = win.surface_pos(x, y, W, H);
            let w = style_panel_hits()
                .iter()
                .find(|(hx, hy, hw, hh, _)| sx >= *hx && sx < *hx + *hw && sy >= *hy && sy < *hy + *hh)
                .map(|r| r.4);
            p.hover = w;
            if let (Some(w), true) = (w, pressed) {
                match w {
                    i if (SP_PREV0..SP_PREV0 + SP_ROWS).contains(&i) => {
                        p.sel = i - SP_PREV0;
                        p.step(p.sel, -1);
                        return StyleAct::Apply(p.style);
                    }
                    i if (SP_NEXT0..SP_NEXT0 + SP_ROWS).contains(&i) => {
                        p.sel = i - SP_NEXT0;
                        p.step(p.sel, 1);
                        return StyleAct::Apply(p.style);
                    }
                    i if i < SP_ROWS => {
                        p.sel = i;
                        p.step(i, 1);
                        return StyleAct::Apply(p.style);
                    }
                    SP_RESET => {
                        p.style = crate::render::text::DialogStyle::default_style();
                        return StyleAct::Apply(p.style);
                    }
                    SP_CLOSE => return StyleAct::Close,
                    _ => {}
                }
            }
        }
        _ => {}
    }
    StyleAct::None
}

struct StyleText {
    sig: String,
    w: u32,
    h: u32,
    buf: Vec<u8>,
}

fn build_style_text(p: &StylePanel, ww: i32, wh: i32) -> Option<StyleText> {
    let px = |v: i32| (v * ww) / 1280;
    let py = |v: i32| (v * wh) / 720;
    let w = px(SP_W).max(1) as u32;
    let h = py(SP_H).max(1) as u32;
    let mut labels = Vec::new();
    labels.push(crate::render::text::Label {
        text: "对话文本样式".to_string(),
        x: 0,
        y: py(20),
        w: w as i32,
        h: py(28),
        size: menu_px(22, wh),
        center: true,
        rgb: ink::HEAD,
        tracking: 0,
    });
    labels.push(crate::render::text::Label {
        text: "改动即时生效并保存".to_string(),
        x: 0,
        y: py(50),
        w: w as i32,
        h: py(16),
        size: menu_px(13, wh),
        center: true,
        rgb: ink::SUB,
        tracking: 0,
    });
    for i in 0..SP_ROWS {
        let y = SP_ROW_Y + i as i32 * SP_ROW_PITCH;
        let on = p.sel == i;
        labels.push(crate::render::text::Label {
            text: SP_NAMES[i].to_string(),
            x: px(SP_LABEL_X - SP_X),
            y: py(y - SP_Y),
            w: px(SP_LABEL_W),
            h: py(SP_ROW_H),
            size: menu_px(18, wh),
            center: false,
            rgb: if on { ink::HEAD } else { ink::LABEL },
            tracking: 0,
        });
        labels.push(crate::render::text::Label {
            text: style_value_text(&p.style, i),
            x: px(SP_BOX_X - SP_X),
            y: py(y - SP_Y),
            w: px(SP_BOX_W),
            h: py(SP_ROW_H),
            size: menu_px(17, wh),
            center: true,
            rgb: ink::BUTTON,
            tracking: 0,
        });
        for (ax, ch) in [(SP_ARROW_L, "<"), (SP_ARROW_R, ">")] {
            labels.push(crate::render::text::Label {
                text: ch.to_string(),
                x: px(ax - SP_X),
                y: py(y - SP_Y),
                w: px(SP_ARROW_W),
                h: py(SP_ROW_H),
                size: menu_px(17, wh),
                center: true,
                rgb: ink::SCALE,
                tracking: 0,
            });
        }
    }
    for (text, bx, bw) in [("恢复默认", SP_RESET_X, 130), ("关闭", SP_CLOSE_X, 110)] {
        labels.push(crate::render::text::Label {
            text: text.to_string(),
            x: px(bx - SP_X),
            y: py(SP_BTN_Y - SP_Y),
            w: px(bw),
            h: py(SP_BTN_H),
            size: menu_px(17, wh),
            center: true,
            rgb: ink::LABEL,
            tracking: 0,
        });
    }
    let size = menu_px(22, wh);
    let buf = crate::render::text::with_role_font(crate::render::text::FontRole::Home, size, |f| {
        f.labels_rgba(w, h, &labels)
    })??;
    Some(StyleText { sig: p.sig(), w, h, buf })
}

fn paint_style_panel(frame: &mut Surface, ww: i32, wh: i32, p: &StylePanel, text: Option<&StyleText>) {
    use crate::render::text::DIALOG_COLORS;
    let px = |v: i32| (v * ww) / 1280;
    let py = |v: i32| (v * wh) / 720;
    frame.fill_rect(0, 0, ww.max(1) as u32, wh.max(1) as u32, [0, 0, 0, 170]);
    let (x, y, w, h) = (px(SP_X), py(SP_Y), px(SP_W), py(SP_H));
    ui_panel(frame, x, y, w, h);
    // A faintly lit title band, then the rule under the hint.
    frame.fill_rect(
        x + px(22),
        y + py(10),
        (w - px(44)).max(1) as u32,
        py(54).max(2) as u32,
        [140, 214, 228, 22],
    );
    // Rule under the title and the auto-save hint.
    frame.fill_rect(x + px(24), y + py(64), (w - px(48)).max(1) as u32, 1, [70, 120, 134, 255]);
    let box_w = px(SP_BOX_W).max(2);
    let box_h = py(SP_ROW_H).max(2);
    for i in 0..SP_ROWS {
        let ry = py(SP_ROW_Y + i as i32 * SP_ROW_PITCH);
        let state = if p.sel == i {
            UiState::Selected
        } else if p.hover == Some(i) {
            UiState::Hover
        } else {
            UiState::Idle
        };
        ui_button(frame, px(SP_BOX_X), ry, box_w, box_h, state);
        let left_hot = p.hover == Some(SP_PREV0 + i);
        ui_button(
            frame,
            px(SP_ARROW_L),
            ry,
            px(SP_ARROW_W).max(2),
            box_h,
            if left_hot { UiState::Hover } else { UiState::Idle },
        );
        let right_hot = p.hover == Some(SP_NEXT0 + i);
        ui_button(
            frame,
            px(SP_ARROW_R),
            ry,
            px(SP_ARROW_W).max(2),
            box_h,
            if right_hot { UiState::Hover } else { UiState::Idle },
        );
    }
    ui_button(
        frame,
        px(SP_RESET_X),
        py(SP_BTN_Y),
        px(130).max(2),
        py(SP_BTN_H).max(2),
        if p.hover == Some(SP_RESET) { UiState::Hover } else { UiState::Idle },
    );
    ui_button(
        frame,
        px(SP_CLOSE_X),
        py(SP_BTN_Y),
        px(110).max(2),
        py(SP_BTN_H).max(2),
        if p.hover == Some(SP_CLOSE) { UiState::Hover } else { UiState::Idle },
    );
    // A swatch of the colour next to its name, so the rows read as one thing
    // being edited rather than eight numbers.
    let sw = DIALOG_COLORS[(p.style.color as usize).min(DIALOG_COLORS.len() - 1)];
    let sy = SP_ROW_Y + 3 * SP_ROW_PITCH;
    frame.fill_rect(
        px(SP_BOX_X) - px(20),
        py(sy) + py(8),
        px(14).max(1) as u32,
        py(SP_ROW_H - 16).max(1) as u32,
        [sw[0], sw[1], sw[2], 255],
    );
    if let Some(t) = text {
        blit_ink(frame, &t.buf, t.w, t.h, x, y);
    }
}


/// What the page is allowed to see this frame.
///
/// No modal layer is ever open while page input is handled: the page's turn
/// comes after the list and the style panel processed the same event. Both
/// flags describe the *start* of the frame, so the event that dismissed a
/// modal cannot also hit the page underneath it (F-02). `opened` covers the
/// frame a list appeared: the click that asked for it is gone by then, but a
/// second click arriving on the opening frame must not pick from it.
fn page_input(modal: bool, opened: bool, input: sdl2::Input) -> sdl2::Input {
    if modal || opened {
        sdl2::Input::None
    } else {
        input
    }
}

/// Font-only「恢复默认」: the three role ids go back to the default profile.
/// Every other setting - speed, volumes, window mode, the dialogue style - is
/// left as the player had it, which is what design §1.2 asks for (F-08).
fn reset_fonts(st: &mut Settings) {
    st.font_home = 0;
    st.font_history = 0;
    st.font_game = 0;
}

/// Run the option screen until the player leaves it.
///
/// `mx` is used for the little sample sound behind the volume rows; when no
/// mixer is available those buttons just log.
pub fn run(
    win: &mut Sdl,
    dirs: &[&Path],
    st: &mut Settings,
    mx: Option<&crate::audio::mixer::Mixer>,
    ui: Option<&crate::UiSound>,
    // The picture the option screen opens over. Every page sheet is ~60%
    // opaque dark glass (`sys_0310X` averages alpha 154), so the game's own
    // compositor put it on top of whatever was on screen: the standing scene,
    // or the title page. Painting it over a black surface instead turned the
    // whole screen into a black plate with a few teal rims on it.
    backdrop: Option<&Surface>,
) {
    let assets = Assets::load(dirs);
    // The sheets above are laid out in the 1280x720 design space, so the
    // backdrop is brought to that size once and blitted under them every
    // frame; the finished frame is scaled back up to the window at the end.
    let underlay: Option<Surface> = backdrop.map(|b| b.scaled_to(W, H));
    if !assets.available() {
        eprintln!("menu: option screen artwork missing, cannot open");
        return;
    }
    let mut scr = Screen {
        page: 0,
        hover: None,
        help_row: None,
        nav_hover: None,
        font_hover: None,
        font_sel: 0,
    };
    let mut dirty = false;
    let cn = using_cn_art(dirs);
    let mut menu_text: Option<MenuText> = None;
    let mut shown: Option<(String, Surface)> = None;
    let mut font_rows: Option<FontRows> = None;
    let mut font_list: Option<FontList> = None;
    let mut list_text: Option<ListText> = None;
    let mut previews: PreviewCache = PreviewCache::new();
    let mut style_panel: Option<StylePanel> = None;
    let mut style_text: Option<StyleText> = None;
    // The page's one-frame request to open a font list. It is not part of
    // `Screen`: once the list exists it owns the input, so nothing outside
    // this loop ever needs to know about the request.
    let mut pending_list: Option<usize> = None;
    // Verification runs: open the style panel without a mouse.
    if std::env::var_os("WA2_FONT_PANEL").is_some() {
        style_panel = Some(StylePanel::new(st));
    }

    loop {
        let mut activate: Option<Act> = None;
        // Where the click happened, for the slider tracks.
        let mut click_at: (i32, i32) = (0, 0);
        let input = win.poll_input();
        // Snapshot the modal state before any layer can close itself: on the
        // frame a list or panel consumes an event *and* goes away, that event
        // must not reach the page (F-02).
        let modal = font_list.is_some() || style_panel.is_some();
        let mut opened_list = false;
        if modal {
            match input {
                sdl2::Input::Up | sdl2::Input::Down | sdl2::Input::Left
                | sdl2::Input::Right | sdl2::Input::Wheel { .. } => {
                    crate::play_ui(mx, ui, crate::UiSe::Move);
                }
                sdl2::Input::Advance => crate::play_ui(mx, ui, crate::UiSe::Confirm),
                sdl2::Input::Back | sdl2::Input::RightClick | sdl2::Input::Quit => {
                    crate::play_ui(mx, ui, crate::UiSe::Cancel);
                }
                _ => {}
            }
        }
        // The style panel is modal: while it is open it owns the keyboard and
        // the page underneath stops seeing clicks. Esc/right-click close the
        // panel only.
        let mut applied: Option<crate::render::text::DialogStyle> = None;
        if let Some(p) = style_panel.as_mut() {
            match style_input(p, input, win) {
                StyleAct::None => {}
                StyleAct::Apply(s) => applied = Some(s),
                StyleAct::Close => style_panel = None,
            }
        }
        if let Some(s) = applied {
            st.dlg_size = s.size as i32;
            st.dlg_edge = s.edge as i32;
            st.dlg_edge_alpha = s.edge_alpha as i32;
            st.dlg_color = s.color as i32;
            st.dlg_under = if s.underline { 1 } else { 0 };
            st.dlg_weight = s.weight as i32;
            st.dlg_track = s.tracking;
            st.dlg_line = s.line_gap;
            if let Err(e) = st.save() {
                eprintln!("settings: could not write {}: {}", Settings::path().display(), e);
            }
            st.apply_text();
            apply(st, mx);
            menu_text = None;
            font_rows = None;
        }
        // A box that was clicked (or Enter on a font row) opens its list on
        // the next frame. That frame consumes its input too: the list just
        // appeared under the cursor and a second click must not pick from it.
        if style_panel.is_none() {
            if let Some(row) = pending_list.take() {
                let current = crate::render::text::FONT_ROLES
                    .get(row)
                    .map(|r| crate::render::text::role_id(*r))
                    .unwrap_or(0);
                font_list = Some(FontList::new(row, current));
                list_text = None;
                opened_list = true;
            }
        }
        // The font list is modal too. The cursor is opened on the role's
        // *effective* font, so the「当前」marker names what is really drawing.
        if let Some(l) = font_list.as_mut() {
            if !opened_list {
                match list_input(l, input, win) {
                    ListAct::None => {}
                    ListAct::Close => font_list = None,
                    ListAct::Pick(id) => {
                        let row = l.row;
                        if pick_role_font(st, row, id) {
                            dirty = true;
                            // The page captions were rasterised with the old
                            // menu font, so they have to be built again.
                            menu_text = None;
                            font_rows = None;
                        }
                        font_list = None;
                    }
                    // Font-only「恢复默认」. The explicit reset drops the font
                    // layer's caches even when the settings already said 0
                    // (an environment override can have been in effect).
                    ListAct::ResetFonts => {
                        reset_fonts(st);
                        crate::render::text::reset_role_ids();
                        dirty = true;
                        menu_text = None;
                        font_rows = None;
                        list_text = None;
                        font_list = None;
                    }
                }
            }
        }
        // While the list or the panel is open (or opened this frame) the page
        // sees nothing at all.
        let page_event = page_input(modal, opened_list, input);
        if !modal && !opened_list {
            match page_event {
                sdl2::Input::Up | sdl2::Input::Down => {
                    crate::play_ui(mx, ui, crate::UiSe::Move);
                }
                sdl2::Input::Choice(_) => crate::play_ui(mx, ui, crate::UiSe::Page),
                sdl2::Input::Quit => crate::play_ui(mx, ui, crate::UiSe::Cancel),
                sdl2::Input::Advance | sdl2::Input::Right => {
                    crate::play_ui(mx, ui, crate::UiSe::Confirm);
                }
                _ => {}
            }
        }
        match page_event {
            sdl2::Input::Quit => {
                win.set_fullscreen(st.win_disp == 1);
                break;
            }
            sdl2::Input::Up => {
                // Page 1: Up/Down move the selected font row, and Enter on a
                // row opens its list. That is the keyboard path to the history
                // and UI rows; the help line is driven by the pointer (F-19
                // m4). Other pages keep the old help-row stepping.
                if scr.page == 0 {
                    scr.font_sel = (scr.font_sel + 2) % 3;
                } else {
                    scr.help_row = Some(match scr.help_row {
                        Some(r) if r > 0 => r - 1,
                        _ => 0,
                    });
                }
            }
            sdl2::Input::Down => {
                if scr.page == 0 {
                    scr.font_sel = (scr.font_sel + 1) % 3;
                } else {
                    scr.help_row = Some(scr.help_row.map(|r| r + 1).unwrap_or(0));
                }
            }
            sdl2::Input::Choice(k) => {
                if let Some(p) = (1..=3).contains(&k).then(|| k as usize - 1) {
                    activate = Some(Act::Tab(p));
                }
            }
            sdl2::Input::Drag { x, y } => {
                // Dragging a slider updates it continuously.
                let (sx, sy) = win.surface_pos(x, y, W, H);
                if let Some(sl) = track_at(scr.page, sx, sy) {
                    let span = (sl.x1 - sl.x0).max(1);
                    let rel = (sx - (PAGE_X + sl.x0)).clamp(0, span);
                    st.set(sl.key, (rel * LEVELS + span / 2) / span);
                    dirty = true;
                }
            }
            sdl2::Input::Back | sdl2::Input::Left => activate = Some(Act::Close),
            sdl2::Input::Advance | sdl2::Input::Right => {
                // Enter on a font row opens its list, like the click does.
                if scr.page == 0 {
                    pending_list = Some(scr.font_sel);
                }
            }
            sdl2::Input::Pointer { x, y, pressed } => {
                let (sx, sy) = win.surface_pos(x, y, W, H);
                let hots = page_hots(scr.page);
                let h = hit(scr.page, &hots, sx, sy);
                let old_hover = scr.hover;
                scr.hover = h.filter(|i| !is_nav(*i));
                scr.nav_hover = h.filter(|i| is_nav(*i));
                if old_hover != scr.hover {
                    crate::play_ui(mx, ui, crate::UiSe::Move);
                }
                if let Some(i) = h {
                    if pressed {
                        let action = if is_nav(i) { nav_act(i) } else { Some(hots[i].act) };
                        activate = action;
                        click_at = (sx, sy);
                        if !matches!(action, Some(Act::Close)) {
                            crate::play_ui(mx, ui, crate::UiSe::Confirm);
                        }
                    }
                }
                // The font block owns the band inside page 1's 文本设置
                // group, so it is checked on its own.
                scr.font_hover = None;
                if scr.page == 0 {
                    if let Some(which) = font_row_hits()
                        .iter()
                        .find(|(hx, hy, hw, hh, _)| {
                            let (x, y) = (PAGE_X + hx, PAGE_Y + hy);
                            sx >= x && sx < x + hw && sy >= y && sy < y + hh
                        })
                        .map(|(_, _, _, _, w)| *w)
                    {
                        scr.font_hover = Some(which);
                        if pressed {
                            match font_hit_kind(which) {
                                // Clicking the box of the list that is already
                                // open can never happen: the list owns the
                                // pointer while it is up, and a click outside
                                // it closes it (the old "click again to
                                // collapse" branch was dead code).
                                Some(FontHit::List(i)) => {
                                    scr.font_sel = i;
                                    pending_list = Some(i);
                                }
                                Some(FontHit::Style) => activate = Some(Act::FontStyle),
                                None => {}
                            }
                        }
                    }
                }
                // Clicking empty space does nothing: leaving the screen is the
                // job of the left arrow key or Escape.
            }
            _ => {}
        }

        if let Some(a) = activate {
            if a == Act::Close {
                crate::play_ui(mx, ui, crate::UiSe::Cancel);
                win.set_fullscreen(st.win_disp == 1);
                break;
            }
            match a {
                Act::Opt(k, v) => {
                    st.set(k, v);
                    dirty = true;
                    // Apply while the page is still open, so the gold frame and
                    // the window change together.
                    if k == "win_disp" {
                        win.set_fullscreen(st.win_disp == 1);
                    }
                }
                Act::Step(k, d) => {
                    let cur = st.get(k).unwrap_or(0);
                    st.set(k, cur + d);
                    dirty = true;
                }
                Act::Track => {
                    // The value follows where the player clicked on the bar.
                    let sl = page_hots(scr.page).iter().find_map(|h| {
                        if !matches!(h.act, Act::Track) {
                            return None;
                        }
                        let (x, y) = (PAGE_X + h.x, page_y(scr.page, h.y));
                        if click_at.0 >= x && click_at.0 < x + h.w && click_at.1 >= y && click_at.1 < y + h.h {
                            h.slider
                        } else {
                            None
                        }
                    });
                    if let Some(sl) = sl {
                        let span = (sl.x1 - sl.x0).max(1);
                        let rel = (click_at.0 - (PAGE_X + sl.x0)).clamp(0, span);
                        let lv = ((rel * LEVELS) + span / 2) / span;
                        st.set(sl.key, lv);
                        dirty = true;
                    }
                }
                Act::Off(k) => {
                    st.set(k, 0);
                    dirty = true;
                }
                Act::Test => {
                    let g_se = st.gain(crate::audio::mixer::CAT_SE);
                    if let Some(mx) = mx {
                        let path = dirs
                            .iter()
                            .map(|d| d.join("SE_9702.OGG"))
                            .find(|p| p.is_file());
                        if let Some(p) = path {
                            mx.play_cat(
                                &p.to_string_lossy(),
                                g_se,
                                false,
                                crate::audio::mixer::CAT_SE,
                            );
                        }
                    }
                }
                Act::FontStyle => {
                    style_panel = Some(StylePanel::new(st));
                    style_text = None;
                }
                Act::Reset => {
                    *st = Settings::default();
                    dirty = true;
                    // The page captions and the font band were rasterised with
                    // the old Home font and the old ids: drop both so the
                    // restored font shows without changing page (F-24).
                    menu_text = None;
                    font_rows = None;
                    win.set_fullscreen(false);
                }
                Act::Tab(p) => {
                    if std::env::var_os("WA2_DEBUG_INPUT").is_some() {
                        eprintln!("menu: tab -> page {}", p);
                    }
                    scr.page = p.min(2);
                    scr.hover = None;
                    scr.help_row = None;
                }
                Act::Close => break,
            }
        }
        // Flush whenever anything changed - dragging a slider sets `dirty`
        // without producing an `activate`.
        if dirty {
            if let Err(e) = st.save() {
                eprintln!("settings: could not write {}: {}", Settings::path().display(), e);
            }
            apply(st, mx);
            dirty = false;
        }

        let (ww, wh) = win.window_size();
        let ww = ww.max(1);
        let wh = wh.max(1);
        // What the font band and the list should contain, computed before the
        // frame key so a font-layer change (fallback, environment override)
        // repaints even when no setting moved.
        let rows_want = if scr.page == 0 {
            font_rows_sig(st, &scr, ww, wh)
        } else {
            String::new()
        };
        let list_key = font_list.as_ref().map(|l| l.key());
        let open_row = font_list.as_ref().map(|l| l.row);
        let sig = format!(
            "{:?}|{:?}|{:?}|{:?}|{}|{}|{}{}{}{:?}",
            scr.page,
            scr.hover,
            scr.help_row,
            st,
            ww,
            wh,
            style_panel.as_ref().map(|p| p.sig()).unwrap_or_default(),
            rows_want,
            font_list.as_ref().map(|l| l.sig()).unwrap_or_default(),
            open_row,
        );
        let reuse = shown.as_ref().map(|(k, _)| k == &sig).unwrap_or(false);
        if !reuse {
            if cn {
                let stale = menu_text
                    .as_ref()
                    .map(|t| t.ww != ww || t.wh != wh || t.page != scr.page)
                    .unwrap_or(true);
                if stale {
                    menu_text = build_menu_text(scr.page, ww, wh);
                }
            }
            let mut s = match underlay.as_ref() {
                // The page art is dark glass; what shows through it is the
                // picture the option screen was opened over.
                Some(u) => Surface { width: u.width, height: u.height, pixels: u.pixels.clone() },
                None => Surface::new(W, H, [0, 0, 0, 255]),
            };
            compose(&mut s, &assets, st, &scr);
            if cn && menu_text.is_some() {
                s.fill_rect(GUIDE_X, guide_y(scr.page), GUIDE_W as u32, GUIDE_H as u32, [0, 0, 0, 255]);
                // Only the page art needs the wipe, and only when it has not
                // been baked already: with a clean page in the cache this is
                // the one branch that does no pixel work at all.
                if assets.clean[scr.page].is_none() {
                    if let Some(base) = assets.base[scr.page].as_ref() {
                        wipe_baked_captions(&mut s, base, scr.page);
                    }
                }
                wipe_chrome_captions(&mut s);
            }
            let mut frame = if ww as u32 == W && wh as u32 == H {
                s
            } else {
                s.scaled_to(ww as u32, wh as u32)
            };
            if let Some(text) = menu_text.as_ref() {
                paint_menu_text(&mut frame, text, scr.help_row);
            }
            // The font rows and the style panel are the two things on this
            // screen the page artwork does not provide, so they are drawn
            // last, over everything the artwork owns.
            if scr.page == 0 {
                let stale = font_rows.as_ref().map(|r| r.sig != rows_want).unwrap_or(true);
                if stale {
                    font_rows = build_font_rows(st, &scr, ww, wh);
                }
                paint_font_rows(&mut frame, ww, wh, &scr, open_row, font_rows.as_ref());
            }
            if let Some(l) = font_list.as_ref() {
                let pw = (LST_W * ww) / 1280;
                let ph = (lst_h() * wh) / 720;
                let stale = list_text
                    .as_ref()
                    .map(|t| Some(&t.key) != list_key.as_ref() || t.w as i32 != pw || t.h as i32 != ph)
                    .unwrap_or(true);
                if stale {
                    list_text = build_list_text(l, ww, wh, &mut previews);
                }
                paint_font_list(&mut frame, ww, wh, l, list_text.as_ref());
            }
            if let Some(p) = style_panel.as_ref() {
                let pw = (SP_W * ww) / 1280;
                let ph = (SP_H * wh) / 720;
                let stale = style_text
                    .as_ref()
                    .map(|t| t.sig != p.sig() || t.w as i32 != pw || t.h as i32 != ph)
                    .unwrap_or(true);
                if stale {
                    style_text = build_style_text(p, ww, wh);
                }
                paint_style_panel(&mut frame, ww, wh, p, style_text.as_ref());
            }
            // `WA2_MENU_DUMP` writes the frame the player sees, labels included.
            // The write goes to a temporary file and is renamed, so a reader
            // never sees a half-written image.
            if let Ok(path) = std::env::var("WA2_MENU_DUMP") {
                let changed = DUMP_SIG.lock().map(|g| *g != sig).unwrap_or(true);
                if changed {
                    let tmp = format!("{}.tmp", path);
                    if frame.save_tga(&tmp).is_ok() {
                        let _ = std::fs::rename(&tmp, &path);
                    }
                    if let Ok(mut g) = DUMP_SIG.lock() {
                        *g = sig.clone();
                    }
                }
            }
            shown = Some((sig, frame));
        }
        if let Some((_, frame)) = shown.as_ref() {
            win.present(frame);
        }
        win.delay(16);
    }
}

/// Push the settings the rest of the engine can honour right now.
///
/// The mixer applies `gain[ALL] * gain[category]` to every stream, so the four
/// sliders map straight onto it.
pub fn apply(st: &Settings, mx: Option<&crate::audio::mixer::Mixer>) {
    use crate::audio::mixer::{CAT_ALL, CAT_BGM, CAT_SE, CAT_VOICE};
    let g = |v: i32| v.clamp(0, LEVELS) as f32 / LEVELS as f32;
    st.apply_text();
    if let Some(mx) = mx {
        mx.set_gain(CAT_ALL, g(st.vol[0]));
        mx.set_gain(CAT_BGM, g(st.vol[1]));
        mx.set_gain(CAT_SE, g(st.vol[2]));
        mx.set_gain(CAT_VOICE, g(st.vol[3]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h_scene_voice_buttons_match_the_engine_rule_values() {
        let hots: Vec<_> = PAGE2
            .iter()
            .filter_map(|h| match h.act {
                Act::Opt("ero_voice", value) => Some(value),
                _ => None,
            })
            .collect();
        assert_eq!(hots, vec![1, 2], "全部 must map to all voices and female-only to the filtered mode");
    }

    /// The three font keys have to survive a save/load round trip, and an id
    /// that is not in the catalogue must fall back to the default rather than
    /// name a font file that does not exist.
    #[test]
    fn font_keys_round_trip_and_reject_unknown_ids() {
        let mut st = Settings::default();
        assert_eq!((st.get("font_home"), st.get("font_history"), st.get("font_game")), (Some(0), Some(0), Some(0)));
        // A real catalogue id has to stick, and one this machine does not
        // have has to fall back to the default rather than to a file that
        // is not there.
        let Some(real) = crate::render::text::font_catalog()
            .iter()
            .find(|p| p.id != 0 && p.path.is_some())
        else {
            return
        };
        st.set("font_game", real.id);
        assert_eq!(st.font_game, real.id);
        st.set("font_home", 99);
        crate::render::text::set_role_ids([st.font_home, 0, 0]);
        assert_eq!(crate::render::text::role_ids()[0], 0, "an unknown id must not stick");
        // Every role's key is one the settings file writes, or a choice made
        // in the panel would be gone after a restart.
        let keys = Settings::all_keys();
        for role in crate::render::text::FONT_ROLES {
            assert!(keys.iter().any(|k| k == role.ini_key()), "{} is not saved", role.ini_key());
        }
    }

    /// With a baked clean page there is nothing left to wipe at run time: one
    /// blit replaces the whole caption-scrubbing pass.
    #[test]
    fn clean_page_needs_no_wipe() {
        let dirs = [
            Path::new("/home/lrxin/wa2-native/data/bg_cn"),
            Path::new("/home/lrxin/wa2-native/data/bg"),
        ];
        let mut assets = Assets::load(&dirs);
        let Some(base) = assets.base[0].as_ref() else { return };
        // Build the clean page from the same art, so the test does not depend
        // on the derived cache being generated in this checkout.
        let baked = bake_clean_page(base, 0);
        assets.clean = [
            Some(TgaImage {
                width: baked.width,
                height: baked.height,
                rgba: baked.pixels.clone(),
            }),
            None,
            None,
        ];
        let st = Settings::default();
        let scr = Screen { page: 0, hover: None, help_row: None, nav_hover: None, font_hover: None, font_sel: 0 };
        let mut s = Surface::new(W, H, [0, 0, 0, 255]);
        compose(&mut s, &assets, &st, &scr);
        let at = |s: &Surface, x: u32, y: u32| {
            let i = ((y * s.width + x) * 4) as usize;
            s.pixels[i] as u32 + s.pixels[i + 1] as u32 + s.pixels[i + 2] as u32
        };
        // The same caption area the fallback test inspects: after a clean
        // compose there must be no bright baked glyph left on the left plate.
        let mut bright = 0u32;
        for y in (PAGE_Y as u32 + 20)..(PAGE_Y as u32 + 50) {
            for x in (PAGE_X as u32 + 40)..(PAGE_X as u32 + 280) {
                let sum = at(&s, x, y);
                if sum > 400 {
                    bright += 1;
                }
            }
        }
        assert_eq!(bright, 0, "clean page still shows the baked caption");
    }

    #[test]
    fn every_label_stays_inside_its_box() {
        let Some(f) = crate::render::text::VectorFont::open(
            crate::render::text::DEFAULT_FONT,
            crate::render::DIALOG_FONT_720,
        ) else {
            return;
        };
        let mut checked = 0usize;
        for page in 0..3 {
            for lab in page_labs(page).iter().chain(chrome_labs().iter()) {
                if lab.text.is_empty() {
                    continue;
                }
                assert!(f.set_size(lab.size), "size {} refused", lab.size);
                let m = f.metrics();
                let line = m.ascender - m.descender;
                let (above, below) = f.ink_bounds(lab.text);
                // Same rule labels_rgba uses.
                let baseline = if line <= lab.h {
                    lab.y + (lab.h - line) / 2 + m.ascender
                } else {
                    lab.y + (lab.h - (above + below)) / 2 + above
                };
                let top = baseline - above;
                let bottom = baseline + below;
                assert!(
                    top >= lab.y - 2,
                    "page {page} {:?}: ink top {top} above box {} (baseline {baseline})",
                    lab.text,
                    lab.y
                );
                assert!(
                    bottom <= lab.y + lab.h + 2,
                    "page {page} {:?}: ink bottom {bottom} below box {} (h {})",
                    lab.text,
                    lab.y + lab.h,
                    lab.h
                );
                let w = f.measure_tracked(lab.text, lab.tracking);
                assert!(
                    w <= lab.w,
                    "page {page} {:?}: width {w} over box {}",
                    lab.text,
                    lab.w
                );
                checked += 1;
            }
        }
        assert!(checked > 60, "only {checked} labels checked");
    }

    /// Tracking is real letter spacing, not spaces in the string: the label
    /// text itself must stay free of padding characters, and the rendered
    /// width must grow by exactly tracking * (chars - 1).
    /// The bake is the clean base: no caption ink may survive in the label
    /// column, while the heading bars and section rules must.
    #[test]
    fn baked_page_drops_captions_keeps_structure() {
        let dirs = [
            Path::new("/home/lrxin/wa2-native/data/bg_cn"),
            Path::new("/home/lrxin/wa2-native/data/bg"),
        ];
        let assets = Assets::load(&dirs);
        let Some(base) = assets.base[0].as_ref() else { return };
        let baked = bake_clean_page(base, 0);
        // 1) Caption column: white-ish ink is gone (white captions would be
        //    ~700 summed; the structures are dim teal, under 300).
        // The ON/OFF pills in that strip keep their baked captions on purpose;
        // their white rims would otherwise count as ink.
        let pills: Vec<(i32, i32, i32, i32)> = page_hots(0)
            .iter()
            .filter(|h| matches!(h.act, Act::Off(_)))
            .map(|h| (h.x - 6, h.y - 6, h.x + h.w + 6, h.y + h.h + 6))
            .collect();
        let mut ink = 0u32;
        for y in 0..PAGE_H as u32 {
            for x in 8..478u32 {
                if pills
                    .iter()
                    .any(|&(x0, y0, x1, y1)| {
                        let (px, py) = (x as i32, y as i32);
                        px >= x0 && px < x1 && py >= y0 && py < y1
                    })
                {
                    continue;
                }
                let i = ((y * baked.width + x) * 4) as usize;
                let p = [baked.pixels[i], baked.pixels[i + 1], baked.pixels[i + 2]];
                let sum = p[0] as u32 + p[1] as u32 + p[2] as u32;
                if sum > 400 && !dim_teal_structure(p) {
                    ink += 1;
                }
            }
        }
        assert_eq!(ink, 0, "baked label column still has caption ink ({ink} px)");
        // 2) The heading bar next to 画面设置 survives.
        let bar = |x: u32, y: u32| {
            let i = ((y * baked.width + x) * 4) as usize;
            dim_teal_structure([baked.pixels[i], baked.pixels[i + 1], baked.pixels[i + 2]])
        };
        let bars = (0..140u32).filter(|&y| bar(28, y)).count();
        assert!(bars > 10, "heading bar was eaten ({bars} px)");
        // 3) A section rule survives: the one right of 画面设置.
        let rules = (300..1000u32).filter(|&x| bar(x, 28)).count();
        assert!(rules > 100, "section rule was eaten ({rules} px)");
    }

    #[test]
    fn tracking_widens_without_polluting_the_string() {
        let Some(f) = crate::render::text::VectorFont::open(
            crate::render::text::DEFAULT_FONT,
            18,
        ) else {
            return;
        };
        let labs = page_labs(0);
        let head = labs.iter().find(|l| l.text == "画面设置").expect("group head");
        assert_eq!(head.tracking, 1, "headings get a little air");
        assert!(!head.text.contains(' '), "no spaces in the label text");
        let plain = f.measure("画面设置");
        let tracked = f.measure_tracked("画面设置", head.tracking);
        assert_eq!(tracked, plain + head.tracking * ("画面设置".chars().count() as i32 - 1));
        // A left label is not letter-spaced: at 22px with a space between every
        // glyph it read looser than the button text it labels.
        let row = labs.iter().find(|l| l.text == "画面模式").expect("row label");
        assert_eq!(row.tracking, 0, "row labels stay tight");
    }

    /// The fallback path: with no baked clean page (a data tree without the
    /// cache), the runtime still has to remove the original captions from the
    /// composed page. The normal path - blitting a clean page - is covered by
    /// `clean_page_needs_no_wipe`.
    #[test]
    fn baked_page_captions_are_replaced_by_the_background() {
        let dirs = [
            Path::new("/home/lrxin/wa2-native/data/bg_cn"),
            Path::new("/home/lrxin/wa2-native/data/bg"),
        ];
        let mut assets = Assets::load(&dirs);
        // Force the old path even if the cache exists in this checkout.
        assets.clean = [None, None, None];
        let base = assets.base[0].as_ref().expect("page art");
        let st = Settings::default();
        let scr = Screen { page: 0, hover: None, help_row: None, nav_hover: None, font_hover: None, font_sel: 0 };
        let mut s = Surface::new(W, H, [0, 0, 0, 255]);
        compose(&mut s, &assets, &st, &scr);
        // A stroke of 「画面模式」, page (140, 32).
        let at = |s: &Surface, x: u32, y: u32| {
            let i = ((y * s.width + x) * 4) as usize;
            s.pixels[i] as u32 + s.pixels[i + 1] as u32 + s.pixels[i + 2] as u32
        };
        // The wipe works on the artwork's own coordinates; the rows above the
        // font block (画面设置 / 画面模式) are not shifted, so the caption to
        // hunt for is at the top of the page image.
        let top = PAGE_Y as u32;
        let mut found = None;
        for y in top..(top + 50) {
            for x in (PAGE_X as u32 + 40)..(PAGE_X as u32 + 280) {
                let sum = at(&s, x, y);
                if sum > 400 {
                    found = Some((x, y, sum));
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }
        let (gx, gy, before) = found.expect("no bright caption pixel on the left");
        assert!(before > 400, "expected a baked glyph, sum {before}");
        wipe_baked_captions(&mut s, base, 0);
        let after = at(&s, gx, gy);
        assert!(after < 80, "glyph should be gone, sum {after}");
        let mut leftover = 0u32;
        let mut worst = (0u32, 0u32, 0u32, [0u8; 3]);
        for y in (PAGE_Y as u32 + 20)..(PAGE_Y as u32 + 50) {
            for x in (PAGE_X as u32 + 40)..(PAGE_X as u32 + 250) {
                let i = ((y * s.width + x) * 4) as usize;
                let p = [s.pixels[i], s.pixels[i + 1], s.pixels[i + 2]];
                if structural_teal(p) {
                    continue;
                }
                let sum = p[0] as u32 + p[1] as u32 + p[2] as u32;
                if sum > 80 {
                    leftover += 1;
                    if sum > worst.0 {
                        worst = (sum, x, y, p);
                    }
                }
            }
        }
        assert!(
            leftover < 40,
            "caption halo still visible: {leftover} px, worst {worst:?}"
        );
        // The button frame stays white, and the caption inside it does not.
        let frame = at(&s, (PAGE_X as u32) + 465, (page_y(0, 0) + 16) as u32);
        assert!(frame > 600, "frame was eaten, sum {frame}");
        let mut btn_left = 0u32;
        for y in (PAGE_Y as u32 + 22)..(PAGE_Y as u32 + 40) {
            for x in (PAGE_X as u32 + 560)..(PAGE_X as u32 + 640) {
                if at(&s, x, y) > 500 {
                    btn_left += 1;
                }
            }
        }
        assert!(btn_left < 24, "button caption still bright: {btn_left} px");
        wipe_chrome_captions(&mut s);
        // The tab strip is blitted untouched: its baked English captions match
        // the labels the engine wants, so the idle tab must still show them.
        let margin = at(&s, (TABS_X + TAB_W + 16) as u32, (TABS_Y + 16) as u32);
        let mut tab_ink = 0u32;
        for y in (TABS_Y as u32 + 12)..(TABS_Y as u32 + 26) {
            for x in (TABS_X as u32 + TAB_W as u32 + 20)..(TABS_X as u32 + TAB_W as u32 * 2 - 20) {
                if at(&s, x, y) > margin + 50 {
                    tab_ink += 1;
                }
            }
        }
        assert!(tab_ink > 20, "idle tab lost its baked caption: {tab_ink} px");
    }

    #[test]
    fn auto_hold_follows_the_slider_and_the_line() {
        let fast = auto_hold_ms(0, 20);
        let slow = auto_hold_ms(LEVELS, 20);
        assert!(slow > fast * 2, "slow {slow} should be well above fast {fast}");
        assert!(auto_hold_ms(5, 40) > auto_hold_ms(5, 8));
        assert!(fast >= 200);
    }

    #[test]
    fn selected_option_is_filled_not_just_an_edge() {
        let dirs = [
            Path::new("/home/lrxin/wa2-native/data/bg_cn"),
            Path::new("/home/lrxin/wa2-native/data/bg"),
        ];
        let assets = Assets::load(&dirs);
        assert!(assets.available());
        let mut st = Settings::default();
        st.text_speed = 1;
        let scr = Screen { page: 0, hover: None, help_row: None, nav_hover: None, font_hover: None, font_sel: 0 };
        let mut surf = Surface::new(W, H, [0, 0, 0, 255]);
        compose(&mut surf, &assets, &st, &scr);
        let sample = |x: i32, y: i32| {
            let i = ((y as u32 * W + x as u32) * 4) as usize;
            (surf.pixels[i], surf.pixels[i + 1], surf.pixels[i + 2])
        };
        // Centres of 「瞬间显示」 and the selected 「高速」.
        // Centres of the idle and the selected text-speed button. Page 1 sits
        // FONT_BLOCK_H lower now, and the artwork coordinates are page ones.
        let y = page_y(0, 72) + 22;
        let idle = sample(PAGE_X + 458 + 62, y);
        let picked = sample(PAGE_X + 586 + 62, y);
        assert!(
            picked.2 > idle.2 + 15 && picked.1 > picked.0,
            "selected body should be cyan, idle {:?} picked {:?}",
            idle,
            picked
        );
        let _ = std::fs::create_dir_all("/tmp/wa2btn");
        let _ = surf.save_tga("/tmp/wa2btn/menu_selected.tga");
    }

    /// The font block opens *inside* the 文本设置 group: page 1 is cut right
    /// after the 文本显示速度 row, rows above the cut keep the artwork's
    /// position, and rows below it move down by the block.
    #[test]
    fn font_block_sits_inside_the_text_settings_group() {
        for page in 0..3 {
            for hot in page_hots(page) {
                let want = if page == 0 && hot.y >= FONT_SPLIT {
                    FONT_BLOCK_H
                } else {
                    0
                };
                assert_eq!(page_y(page, hot.y), PAGE_Y + hot.y + want);
            }
        }
        // The cut is after 自动模式速度 (its track ends at y = 192 and its
        // scale labels at 201) and at the 视觉设置 header (y = 204), so the
        // three font rows are the last entries of the 文本设置 group.
        assert!(FONT_SPLIT >= 192, "split cuts into the auto-speed row");
        assert!(FONT_SPLIT <= 204, "split leaves the 视觉设置 header behind");
        assert!(FONT_BLOCK_H >= 3 * FROW_PITCH, "the font block needs room for three rows");
        // The rows above the split keep the artwork's position; the group
        // header below it moves down by the band.
        let labs = page_labs(0);
        let auto = labs.iter().find(|l| l.text == "自动模式速度").expect("auto-speed label");
        assert_eq!(auto.y, PAGE_Y + 162, "the auto-speed row must not move");
        let visual = labs.iter().find(|l| l.text == "视觉设置").expect("visual-settings head");
        assert!(
            visual.y >= PAGE_Y + FONT_SPLIT + FONT_BLOCK_H,
            "视觉设置 has to move below the font band, got {}",
            visual.y
        );
        assert_eq!(page_shift(1), 0);
        assert_eq!(page_shift(2), 0);
        // The help line follows the lower artwork.
        assert!(help_y(0) > PAGE_Y + 400, "page 1 grew, so the help line has to go with it");
        assert_eq!(help_y(1), HELP_Y);
    }

    /// The three font rows have to fit in the band the artwork gave up, and the
    /// dialogue row has to come first, because it is the one that carries the
    /// style entry.
    #[test]
    fn font_rows_fit_and_are_ordered() {
        for (i, role) in crate::render::text::FONT_ROLES.iter().enumerate() {
            let y = font_row_y(i);
            assert!(y >= 0 && y + FROW_H <= FONT_BLOCK_H, "row {} overflows the band", i);
            assert!(!role.label().is_empty(), "row {} has no label", i);
        }
        assert_eq!(font_row_y(2) + FROW_H <= FONT_BLOCK_H, true);
        // The dialogue row is first, and it is the one with the style entry.
        let hits = font_row_hits();
        assert!(hits
            .iter()
            .any(|(_, _, _, _, w)| matches!(font_hit_kind(*w), Some(FontHit::Style))));
        // Every row opens a list.
        for i in 0..3 {
            assert!(
                hits.iter()
                    .any(|(_, _, _, _, w)| matches!(font_hit_kind(*w), Some(FontHit::List(r)) if r == i)),
                "row {} has no list trigger",
                i
            );
        }
    }

    /// Picking a font moves only that role, and the list keeps the cursor on
    /// screen while it scrolls.
    #[test]
    fn picking_a_font_moves_only_that_role() {
        let mut st = Settings::default();
        st.font_game = 0;
        st.font_history = 0;
        st.font_home = 0;
        let Some(real) = crate::render::text::font_catalog()
            .iter()
            .find(|p| p.id != 0 && p.path.is_some())
        else {
            return
        };
        assert!(pick_role_font(&mut st, 0, real.id));
        assert_eq!(st.font_game, real.id, "the game font should have moved");
        assert_eq!(st.font_history, 0, "the history font must not follow");
        assert_eq!(st.font_home, 0, "the UI font must not follow");
        // An id this machine does not have falls back to the default rather
        // than to a file that is not there.
        assert!(pick_role_font(&mut st, 0, 99));
        assert_eq!(st.font_game, 0);
        // Out-of-range rows are ignored.
        assert!(!pick_role_font(&mut st, 9, real.id));
    }

    /// The list scrolls to show the cursor wherever it is in the catalogue.
    #[test]
    fn list_keeps_the_cursor_on_screen() {
        let n = crate::render::text::font_catalog().len();
        if n == 0 {
            return;
        }
        for i in [0, 1, LST_ROWS - 1, LST_ROWS, n - 1, n] {
            let mut l = FontList::new(0, 0);
            l.cur = i;
            l.clamp();
            assert!(l.cur < l.top + LST_ROWS.max(1), "cursor {} below the window at {}", i, l.top);
            assert!(l.cur >= l.top, "cursor {} above the window at {}", i, l.top);
            assert!(l.top <= n.saturating_sub(LST_ROWS), "scrolled past the end");
        }
        // Stepping off the end wraps to the front.
        let mut l = FontList::new(0, 0);
        l.cur = n - 1;
        l.step(1);
        assert_eq!(l.cur, 0);
    }
    /// The dialogue style has to survive a round trip through the settings file
    /// and has to be clamped, because it is pixels and an index rather than a
    /// slider level.
    #[test]
    fn dialogue_style_round_trips_and_clamps() {
        let mut st = Settings::default();
        st.set("dlg_size", 30);
        st.set("dlg_edge", 2);
        st.set("dlg_color", 5);
        st.set("dlg_under", 1);
        assert_eq!((st.dlg_size, st.dlg_edge, st.dlg_color, st.dlg_under), (30, 2, 5, 1));
        let style = dialog_style(&st);
        let want = crate::render::text::DialogStyle {
            size: 30,
            edge: 2,
            color: 5,
            underline: true,
            ..crate::render::text::DialogStyle::default_style()
        };
        assert_eq!(style, want);
        // A hand-edited value is stored as it is and ranged where it is read,
        // the same way the slider keys clamp on the way out.
        st.set("dlg_size", 999);
        assert_eq!(st.dlg_size, 999);
        assert_eq!(dialog_style(&st).size, crate::render::text::DIALOG_SIZE_MAX, "must clamp on read");
        st.set("dlg_edge", 9);
        assert_eq!(dialog_style(&st).edge, crate::render::text::DIALOG_EDGE_MAX, "must clamp on read");
        for k in ["dlg_size", "dlg_edge", "dlg_color", "dlg_under"] {
            assert!(Settings::all_keys().iter().any(|s| s == k), "{} is not saved", k);
        }
    }

    /// The catalogue has to list the machine's fonts rather than a fixed
    /// handful, and every entry has to be able to draw the script.
    #[test]
    fn catalogue_lists_the_machines_fonts() {
        let cat = crate::render::text::font_catalog();
        assert_eq!(cat[0].id, 0, "the default entry must be id 0, so old configs work");
        assert!(cat.iter().all(|p| p.path.as_ref().map_or(false, |x| std::path::Path::new(x).is_file())));
        let mut checked = 0;
        for p in cat.iter() {
            let Some(path) = p.path.as_ref() else { continue };
            let mut ft = match crate::ft2::Ft::open(path, 24, p.face) {
                Some(f) => f,
                None => continue,
            };
            // A font the settings page offers but that cannot draw Chinese
            // would show tofu in the middle of every line.
            assert!(ft.glyph('汉').is_some(), "{} cannot draw Chinese", p.name);
            assert!(ft.glyph('あ').is_some(), "{} cannot draw kana", p.name);
            checked += 1;
        }
        assert!(checked > 0, "no font could be opened");
        // Ids are unique, or one config value would name two fonts.
        let mut ids: Vec<i32> = cat.iter().map(|p| p.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), cat.len());
    }

    /// On the frame a modal layer consumes an event and dismisses itself, the
    /// page underneath must not see that event: that is the F-02 input
    /// passthrough. The page's turn always comes after the modal's, so the
    /// decision has to be based on the state the frame started with.
    #[test]
    fn modal_frames_never_feed_the_page() {
        let events = [
            sdl2::Input::Quit,
            sdl2::Input::Advance,
            sdl2::Input::Up,
            sdl2::Input::Pointer { x: 900, y: 220, pressed: true },
        ];
        for input in events {
            assert_eq!(page_input(false, false, input), input, "an idle frame forwards its event");
            assert_eq!(page_input(true, false, input), sdl2::Input::None, "modal frame must swallow it");
            assert_eq!(page_input(false, true, input), sdl2::Input::None, "opening frame must swallow it");
        }
    }

    /// The font-only「恢复默认」has to leave every other setting exactly as the
    /// player had it (design §1.2): text speed, volumes, window mode and the
    /// dialogue style all stay.
    #[test]
    fn font_reset_touches_only_the_three_roles() {
        let mut st = Settings::default();
        st.win_disp = 1;
        st.text_speed = 0;
        st.auto_speed = 2;
        st.vol = [3, 4, 5, 6];
        st.alpha_novel = 4;
        st.dlg_size = 31;
        st.dlg_edge = 2;
        st.dlg_edge_alpha = 200;
        st.dlg_color = 5;
        st.dlg_under = 1;
        st.dlg_weight = 2;
        st.dlg_track = 3;
        st.dlg_line = 4;
        st.font_home = 7;
        st.font_history = 8;
        st.font_game = 9;
        let before = st.clone();
        reset_fonts(&mut st);
        assert_eq!((st.font_home, st.font_history, st.font_game), (0, 0, 0));
        let mut want = before;
        want.font_home = 0;
        want.font_history = 0;
        want.font_game = 0;
        assert_eq!(st, want, "only the three font ids may change");
    }

    /// A role that fell back says so, naming the font it really draws with and
    /// the id the file still asks for (F-06). An id that names no font is not
    /// the default being missing: the default is what took over.
    #[test]
    fn font_row_names_the_requested_and_effective_font() {
        let default_name = font_name(0);
        assert!(!default_name.is_empty());
        // An id that names no catalogue entry must not claim the default font
        // is missing; the default is what replaced it (F-06).
        let unknown = font_name(999_999_999);
        assert!(unknown.contains("原字体未安装"), "{unknown}");
        assert!(unknown.contains("默认"), "{unknown}");
        let fallback = font_row_text(4242, 0, Some("could not open the file"));
        assert!(fallback.contains("已使用备用字体"), "{fallback}");
        assert!(fallback.contains("4242"), "{fallback}");
        // The box is ~300 px wide, so the full sentence (with the effective
        // font's name) is left to the list header.
        let note = font_fallback_note(4242, 0, Some("could not open the file")).unwrap();
        assert!(note.contains("已使用备用字体"), "{note}");
        assert!(note.contains("4242"), "{note}");
        assert!(note.contains(&default_name), "{note}");
        assert!(font_fallback_note(0, 0, None).is_none());
        // A role overridden from the environment draws with the effective font
        // and reports the id the file asked for.
        let overridden = font_row_text(4242, 0, None);
        assert!(overridden.contains("请求 id 4242"), "{overridden}");
        // With nothing in the way the row simply names the requested font.
        assert_eq!(font_row_text(0, 0, None), default_name);
    }

    /// Long family names are clipped to their column with an ellipsis: the
    /// label canvas does not clip horizontally by itself (F-19 m3).
    #[test]
    fn long_captions_are_clipped_within_the_column() {
        // Clipping needs a font to measure with; every other test in this
        // module assumes the same, and the default profile is bundled.
        if crate::render::text::with_ui_font(17, |_| ()).is_none() {
            return;
        }
        let long = "Noto Sans CJK SC DemiLight Extra Long Family Name";
        let clipped = fit_label(long, 200, 17);
        assert!(clipped.chars().count() < long.chars().count(), "{clipped}");
        let w = crate::render::text::with_ui_font(17, |f| f.measure(&clipped)).unwrap_or(0);
        assert!(w <= 200, "clipped caption is {w} px wide: {clipped}");
        // A caption that already fits is not touched.
        assert_eq!(fit_label("Short", 200, 17), "Short");
    }

    /// Every preview has to fit the column between the names/tag and the
    /// scrollbar, or it would be clipped mid-sample (F-07).
    #[test]
    fn font_previews_fit_the_preview_column() {
        let avail = LST_PREVIEW_W - 4;
        assert!(avail > 0);
        assert!(
            LST_PREVIEW_X + LST_PREVIEW_W <= LST_W - LST_BAR_W,
            "the preview column must end before the scrollbar"
        );
        let want = 15u32;
        let min = 9u32;
        let mut checked = 0;
        for p in crate::render::text::font_catalog() {
            let Some(vf) = crate::render::text::open_profile_font(p.id, want) else { continue };
            let size = fit_sample_size(&vf, LST_SAMPLE, want, min, avail);
            assert!((min..=want).contains(&size), "{}: preview size {size}", p.name);
            if !vf.set_size(size) {
                continue;
            }
            let sample = fit_text_with(&vf, LST_SAMPLE, avail);
            let w = vf.measure(&sample);
            assert!(w <= avail, "{}: preview {w} px overflows {avail}", p.name);
            checked += 1;
        }
        assert!(checked > 0, "no font could be opened");
    }

    /// The §11 knobs live in wa2.ini like every other setting, and a
    /// hand-edited value is ranged where it is read (F-14).
    #[test]
    fn style_knobs_round_trip_and_clamp() {
        let mut st = Settings::default();
        for (k, v) in [("dlg_edge_alpha", 200), ("dlg_weight", 2), ("dlg_track", 3), ("dlg_line", 5)] {
            st.set(k, v);
            assert_eq!(st.get(k), Some(v), "{k} did not round trip");
        }
        let s = dialog_style(&st);
        assert_eq!((s.edge_alpha, s.weight, s.tracking, s.line_gap), (200, 2, 3, 5));
        st.set("dlg_edge_alpha", 900);
        st.set("dlg_weight", 99);
        st.set("dlg_track", -9);
        st.set("dlg_line", 99);
        let s = dialog_style(&st);
        assert_eq!(
            (s.edge_alpha, s.weight, s.tracking, s.line_gap),
            (255, 4, -2, 12),
            "hand-edited values must clamp on read"
        );
        for k in ["dlg_edge_alpha", "dlg_weight", "dlg_track", "dlg_line"] {
            assert!(Settings::all_keys().iter().any(|s| s == k), "{} is not saved", k);
        }
    }

    /// Page 1's lower artwork is drawn 108 px lower; the drag hit test has to
    /// use the same position as the drawing and the click (F-15). The auto
    /// speed row sits above the font band, so the shifted tracks are the
    /// opacity ones below it.
    #[test]
    fn sliders_on_page_one_hit_where_they_are_drawn() {
        let x = PAGE_X + 531 + 10;
        assert_eq!(
            track_at(0, x, page_y(0, 156) + 10).map(|s| s.key),
            Some("auto_max"),
            "the auto-speed track is above the split and must not move"
        );
        assert_eq!(
            track_at(0, x, page_y(0, 261) + 10).map(|s| s.key),
            Some("win_alpha_novel"),
            "the shifted opacity track must be found"
        );
        assert!(
            track_at(0, x, PAGE_Y + 261 + 10).is_none(),
            "the unshifted position is above the shifted track"
        );
        // Pages 2 and 3 do not move.
        assert_eq!(
            track_at(1, x, page_y(1, 117) + 10).map(|s| s.key),
            Some("all_vol"),
            "page 2 tracks must not move"
        );
    }

    /// The style panel has to offer all eight §11 knobs, and every row needs
    /// its box and both arrows inside the panel.
    #[test]
    fn style_panel_has_all_eight_rows() {
        assert_eq!(SP_NAMES.len(), SP_ROWS);
        assert!(SP_NAMES.iter().all(|n| !n.is_empty()));
        let hits = style_panel_hits();
        for i in 0..SP_ROWS {
            for which in [i, SP_PREV0 + i, SP_NEXT0 + i] {
                assert!(hits.iter().any(|r| r.4 == which), "widget {which} is missing");
            }
        }
        assert_eq!(SP_NEXT0, SP_ROWS);
        assert_eq!(SP_RESET, SP_ROWS * 2);
        assert_eq!(SP_CLOSE, SP_ROWS * 2 + 1);
        let last_row = SP_ROW_Y + (SP_ROWS as i32 - 1) * SP_ROW_PITCH + SP_ROW_H;
        assert!(last_row < SP_BTN_Y, "last row {last_row} overlaps the buttons at {SP_BTN_Y}");
        assert!(SP_BTN_Y + SP_BTN_H <= SP_Y + SP_H, "buttons leave the panel");
        // Up/Down walk the rows, so the selection has to stay inside them.
        let mut p = StylePanel::new(&Settings::default());
        for _ in 0..(SP_ROWS * 2 + 1) {
            p.sel = (p.sel + 1) % SP_ROWS;
            assert!(p.sel < SP_ROWS);
        }
    }

    /// Stepping the new knobs must stay inside their ranges: size/edge/colour/
    /// weight/tracking/gap wrap, outline alpha clamps at 0 and 255.
    #[test]
    fn style_steps_stay_in_range() {
        use crate::render::text::{
            DIALOG_EDGE_ALPHA_MAX, DIALOG_LINE_GAP_MAX, DIALOG_TRACK_MAX, DIALOG_TRACK_MIN,
            DIALOG_WEIGHT_MAX,
        };
        let mut p = StylePanel::new(&Settings::default());
        // Outline coverage stops at the ends instead of wrapping.
        p.style.edge_alpha = 10;
        p.step(2, -1);
        assert_eq!(p.style.edge_alpha, 0);
        p.step(2, -1);
        assert_eq!(p.style.edge_alpha, 0);
        p.style.edge_alpha = DIALOG_EDGE_ALPHA_MAX - 5;
        p.step(2, 1);
        assert_eq!(p.style.edge_alpha, DIALOG_EDGE_ALPHA_MAX);
        p.step(2, 1);
        assert_eq!(p.style.edge_alpha, DIALOG_EDGE_ALPHA_MAX);
        // Tracking wraps through the negative end.
        p.style.tracking = DIALOG_TRACK_MIN;
        p.step(5, -1);
        assert_eq!(p.style.tracking, DIALOG_TRACK_MAX);
        // Weight and line gap wrap.
        p.style.weight = DIALOG_WEIGHT_MAX;
        p.step(4, 1);
        assert_eq!(p.style.weight, 0);
        p.style.line_gap = DIALOG_LINE_GAP_MAX;
        p.step(6, 1);
        assert_eq!(p.style.line_gap, 0);
        // Underline is a switch: both arrows toggle it.
        let under = p.style.underline;
        p.step(7, 1);
        assert_ne!(p.style.underline, under);
        p.step(7, -1);
        assert_eq!(p.style.underline, under);
        // The value column names every knob without panicking.
        for i in 0..SP_ROWS {
            assert!(!style_value_text(&p.style, i).is_empty(), "row {i} has no value text");
        }
    }

    /// The list footer is the font-only reset, and it has to be hittable as
    /// part of the list rather than as a row of the catalogue (F-08).
    #[test]
    fn list_footer_offers_the_font_only_reset() {
        let l = FontList::new(0, 0);
        let foot = list_hits(&l)
            .into_iter()
            .find(|r| r.4 == LST_RESET)
            .expect("the list has no reset footer");
        assert!(foot.1 >= lst_y(0) + LST_HEAD);
        assert!(foot.1 + foot.3 <= lst_y(0) + lst_h(), "footer overflows the list");
        assert_eq!(list_at(&l, foot.0 + 4, foot.1 + 4), Some(LST_RESET));
        // Footer and scrollbar are distinct hits.
        let bar = list_hits(&l)
            .into_iter()
            .find(|r| r.4 == usize::MAX)
            .expect("the list has no scrollbar");
        assert_ne!(bar.4, foot.4);
    }

    /// A role whose font fell back has to invalidate the band and draw a row
    /// that says so (F-06); the fallback reason is a runtime value, not part of
    /// the settings, so the cache key has to include it.
    #[test]
    fn fallback_state_invalidates_and_draws_the_band() {
        let st = Settings::default();
        let scr = Screen {
            page: 0,
            hover: None,
            help_row: None,
            nav_hover: None,
            font_hover: None,
            font_sel: 0,
        };
        let before = font_rows_sig(&st, &scr, 1280, 720);
        let Some(role) = crate::render::text::FONT_ROLES.first().copied() else { return };
        crate::render::text::set_role_fallback_reason(role, Some("test: cannot open".to_string()));
        let after = font_rows_sig(&st, &scr, 1280, 720);
        let rows = build_font_rows(&st, &scr, 1280, 720);
        crate::render::text::set_role_fallback_reason(role, None);
        assert_ne!(before, after, "a fallback has to invalidate the font band");
        if let Some(rows) = rows {
            let mut frame = Surface::new(W, H, [0, 0, 0, 255]);
            paint_font_rows(&mut frame, 1280, 720, &scr, None, Some(&rows));
        }
    }

    /// The three font-area layers must build and paint without a window: both
    /// are pure geometry over `Surface`, and a panic here would only show up
    /// as a crash on the option screen.
    #[test]
    fn font_area_layers_paint_without_a_window() {
        let st = Settings::default();
        let scr = Screen {
            page: 0,
            hover: None,
            help_row: None,
            nav_hover: None,
            font_hover: Some(0),
            font_sel: 1,
        };
        let mut frame = Surface::new(W, H, [0, 0, 0, 255]);
        let rows = build_font_rows(&st, &scr, 1280, 720);
        paint_font_rows(&mut frame, 1280, 720, &scr, Some(0), rows.as_ref());
        let l = FontList::new(0, 0);
        let mut previews = PreviewCache::new();
        let text = build_list_text(&l, 1280, 720, &mut previews);
        paint_font_list(&mut frame, 1280, 720, &l, text.as_ref());
        let panel = StylePanel::new(&st);
        let style = build_style_text(&panel, 1280, 720);
        paint_style_panel(&mut frame, 1280, 720, &panel, style.as_ref());
        // A smaller window has to scale the canvases with it.
        let small = build_list_text(&l, 960, 540, &mut previews);
        if let Some(t) = small {
            assert_eq!(t.w as i32, (LST_W * 960) / 1280);
            assert_eq!(t.h as i32, (lst_h() * 540) / 720);
            let mut small_frame = Surface::new(960, 540, [0, 0, 0, 255]);
            paint_font_list(&mut small_frame, 960, 540, &l, Some(&t));
        }
        if let Some(rows) = build_font_rows(&st, &scr, 960, 540) {
            assert_eq!(rows.w as i32, (PAGE_W * 960) / 1280);
            let mut small_frame = Surface::new(960, 540, [0, 0, 0, 255]);
            paint_font_rows(&mut small_frame, 960, 540, &scr, None, Some(&rows));
        }
    }

    /// Dragging the scrollbar maps the pointer position on the track to a
    /// scroll offset, and both ends of the track reach the ends of the list
    /// (F-19 m2).
    #[test]
    fn scrollbar_drag_maps_the_pointer_to_an_offset() {
        let n = crate::render::text::font_catalog().len();
        if n <= LST_ROWS {
            return;
        }
        let last = n - LST_ROWS;
        let top_y = PAGE_Y + lst_y(0) + LST_HEAD;
        let track = LST_ROWS as i32 * LST_ROW_H;
        let mut l = FontList::new(0, 0);
        scroll_from_bar(&mut l, last, top_y);
        assert_eq!(l.top, 0, "the top of the track is the top of the list");
        scroll_from_bar(&mut l, last, top_y + track);
        assert_eq!(l.top, last, "the bottom of the track is the end of the list");
        scroll_from_bar(&mut l, last, top_y + track / 2);
        assert!(l.top > 0 && l.top < last, "the middle of the track is in between");
    }

    /// The preview bitmaps are cached by (id, size, column width), so a scroll
    /// or a rebuild does not reopen another eleven FreeType faces.
    #[test]
    fn preview_cache_keeps_bitmaps_across_rebuilds() {
        let mut previews: PreviewCache = PreviewCache::new();
        let l = FontList::new(0, 0);
        // The font catalogue is never empty (id 0 exists even with no font),
        // but the UI font itself can be unavailable and then there is nothing
        // to cache.
        if build_list_text(&l, 1280, 720, &mut previews).is_none() {
            return;
        }
        let first = previews.len();
        assert!(first > 0, "no preview was cached");
        let _ = build_list_text(&l, 1280, 720, &mut previews);
        assert_eq!(previews.len(), first, "rebuilding reopened and recached previews");
    }
}
