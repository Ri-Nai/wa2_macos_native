//! Loading the art tree: background, effect, tint and interface sheets.

use crate::*;

/// Load a scenario by id from `script_dir` (looks for `<id>.txt`).
pub(crate) fn load_scenario(script_dir: &std::path::Path, id: &str) -> Option<Vec<u8>> {
    let p = script_dir.join(format!("{}.txt", id));
    std::fs::read(&p).ok()
}

/// Try to load a background / UI TGA by name, searching `dirs` in order (the
/// Chinese override directory first, then the shared background directory).
/// Returns None if the file is missing or fails to decode (the caller keeps the
/// previous background).
///
/// WA2 scripts sometimes reference a suffixed variant (e.g. `v100200_bg.tga`)
/// while the archive contains the base name (`v100200.tga`). We try the exact
/// name first, then strip common suffixes.
/// ACT148 / ACT201 file. The exe's mode-1 sprintf is `v%06d.tga` of
/// `id * 10 + variant`: 10170 variant 0 is `v101700.tga`.
pub(crate) fn event_cg_name(id: i64, variant: u32) -> String {
    format!("v{:06}.tga", id * 10 + variant as i64)
}

/// Scene-switch profiling. `WA2_TIMING=1` prints the cost of each decode,
/// composite and upload so a stall can be attributed to one of them.
pub(crate) fn timing_enabled() -> bool {
    std::env::var_os("WA2_TIMING").is_some()
}

pub(crate) fn tlog(label: &str, t0: std::time::Instant) {
    if timing_enabled() {
        eprintln!("  [timing] {label}: {:.1} ms", t0.elapsed().as_secs_f64() * 1000.0);
    }
}

pub(crate) fn source_art_scale() -> u32 {
    std::env::var("WA2_ART_SCALE")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|&v| matches!(v, 1 | 2 | 4 | 8))
        .unwrap_or(1)
}

/// How the decode scale is chosen. Decided once, before any call writes a
/// number into `WA2_RENDER_SCALE`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScaleMode {
    /// `WA2_RENDER_SCALE=auto`: decode only as much as the window can show.
    Window,
    /// Numeric `WA2_RENDER_SCALE`: exactly that scale.
    Forced(u32),
    /// No override: never below 4x, more only when the window needs it.
    Default,
}

pub(crate) fn scale_mode() -> ScaleMode {
    static MODE: std::sync::OnceLock<ScaleMode> = std::sync::OnceLock::new();
    *MODE.get_or_init(|| match std::env::var("WA2_RENDER_SCALE") {
        Ok(v) if v.eq_ignore_ascii_case("auto") => ScaleMode::Window,
        Ok(v) => match v.trim().parse::<u32>() {
            Ok(n) if matches!(n, 1 | 2 | 4 | 8) => ScaleMode::Forced(n),
            _ => ScaleMode::Default,
        },
        Err(_) => ScaleMode::Default,
    })
}

/// Decode scale wanted under the current mode.
///
/// Default: windowed decodes at 4x (the 4x tree is read directly), fullscreen
/// decodes at 8x (the master tree, even on a 1080p panel where the extra
/// pixels are supersampling). `WA2_RENDER_SCALE=auto` keeps the old
/// window-sized decoding, a number forces one scale.
pub(crate) fn decode_scale_for_window(win: &sdl2::Sdl) -> u32 {
    let (w, h) = win.window_size();
    match scale_mode() {
        ScaleMode::Forced(n) => n,
        ScaleMode::Window => render_scale_for(w, h),
        ScaleMode::Default => {
            if win.is_fullscreen() || win.is_maximized() {
                8
            } else {
                render_scale_for(w, h).max(4)
            }
        }
    }
}

/// How many pixels one decoded art asset stores per 1280x720 layout pixel.
pub(crate) fn render_art_scale() -> u32 {
    if let ScaleMode::Forced(n) = scale_mode() {
        return n;
    }
    std::env::var("WA2_RENDER_SCALE")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|&v| matches!(v, 1 | 2 | 4 | 8))
        .unwrap_or_else(source_art_scale)
}

/// Report and apply the decode scale for the current window.
pub(crate) fn set_render_art_scale(win: &sdl2::Sdl) {
    let (w, h) = win.window_size();
    let want = decode_scale_for_window(win);
    static REPORTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    REPORTED.get_or_init(|| {
        let why = match scale_mode() {
            ScaleMode::Forced(n) => format!("forced {}x by WA2_RENDER_SCALE", n),
            ScaleMode::Window => "auto: window-sized decoding".to_string(),
            ScaleMode::Default => {
                "default: windowed 4x / fullscreen or maximized 8x; WA2_RENDER_SCALE=auto for window-sized decoding"
                    .to_string()
            }
        };
        eprintln!(
            "art: source {}x, decode {}x for window {}x{} ({})",
            source_art_scale(),
            want,
            w,
            h,
            why
        );
    });
    let current = std::env::var("WA2_RENDER_SCALE")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok());
    if current != Some(want) {
        std::env::set_var("WA2_RENDER_SCALE", want.to_string());
        // Pictures decoded for the old scale are the wrong pixel size now;
        // dropping them keeps the compositor from mixing scales.
        artcache::drop_other_scales(want);
        eprintln!("art: render scale={} for window {}x{}", want, w, h);
    }
}

/// Largest useful art scale for a window: 1280x720 needs 1x, 1920x1080
/// needs 2x, 2560x1440/4K need 4x, anything larger 8x.
pub(crate) fn render_scale_for(w: i32, h: i32) -> u32 {
    let w = w.max(1280) as u32;
    let h = h.max(720) as u32;
    let need = ((w + 1279) / 1280).max((h + 719) / 720);
    if need <= 1 { 1 } else if need <= 2 { 2 } else if need <= 4 { 4 } else { 8 }
}

pub(crate) fn load_background(dirs: &[&std::path::Path], name: &str) -> Option<codec::tga::TgaImage> {
    load_background_asset(dirs, name).map(|asset| asset.img)
}

/// load_background_asset_inner with a WA2_TIMING profile line.
pub(crate) fn load_background_asset(dirs: &[&std::path::Path], name: &str) -> Option<LoadedAsset> {
    let t0 = std::time::Instant::now();
    let out = load_background_asset_inner(dirs, name);
    tlog(&format!("load {name}"), t0);
    out
}

/// Resolve the full-screen effect plate selected by an ACT146/147 mode.
/// The exe masks the high bit and formats the remaining value as `f0NNN.bmp`;
/// the scaled WebP siblings are picked by the normal art loader.
pub(crate) fn effect_name(mode: i32) -> Option<String> {
    (mode >= 0x80).then(|| format!("f0{:03}.bmp", mode & 0x7f))
}

/// What an ACT146/147 background id asks the scene to do.
///
/// The scripts use three kinds of id and they look alike in the bytecode:
///
/// * `0` is a scripted cut to black - the shot is over and everybody in it
///   leaves.
/// * a negative id (only `-2` occurs, 736 times in the corpus) is the
///   "re-composite the shot you are already showing" sentinel. It is
///   emitted right in front of a `spr Enter`, between a picture and the
///   same picture again with one more person standing in it. Treating it as
///   a black screen is what used to paint the cast on solid black.
/// * anything else is a picture id to look up.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BgRequest {
    Black,
    Keep,
    Picture,
}

pub(crate) fn bg_request(id: i64) -> BgRequest {
    match id {
        0 => BgRequest::Black,
        i if i < 0 => BgRequest::Keep,
        _ => BgRequest::Picture,
    }
}

pub(crate) fn background_override_name(id: i64, variant: u32, preset: i32) -> String {
    format!("b{:04}{}{}.amp", id, variant, preset.clamp(0, 9))
}

pub(crate) fn load_effect(dirs: &[&std::path::Path], mode: i32) -> Option<codec::tga::TgaImage> {
    let name = effect_name(mode)?;
    match load_background_asset(dirs, &name) {
        Some(asset) => {
            eprintln!("  effect: ACT146/147 mode {} -> {} ({}x)", mode, name, asset.src_scale);
            Some(asset.img)
        }
        None => {
            eprintln!("  effect: mode {} -> {} NOT FOUND", mode, name);
            None
        }
    }
}

/// Load a scene asset and retain the scale represented by the decoded pixels.
/// UI sheets are normalized by decode_ui_file, while art keeps the largest
/// useful scale allowed by the current render target.
pub(crate) fn load_background_asset_inner(dirs: &[&std::path::Path], name: &str) -> Option<LoadedAsset> {
    let t0 = std::time::Instant::now();
    let is_ui =
        name.starts_with("sys_") || name.starts_with("SYS_") || name.starts_with('t') || name.starts_with('T')
            || name.eq_ignore_ascii_case("weather.tga");
    let found = resolve_asset_path(dirs, name);
    tlog("resolve", t0);
    let t1 = std::time::Instant::now();
    // UI sheets are consumed at their 1280x720 logical size and the tool
    // pre-renders them under data/cache/ui, so they only need the decoder as a
    // fallback. Scene art goes through the derived cache: an 8x WebP is decoded
    // once and every later load of it is a file read or a memory hit.
    for (path, cp932) in found {
        let decoded = if is_ui {
            codec::image::decode_ui_file(&path).ok()
        } else {
            artcache::load(&path).map(|img| (*img).clone())
        };
        if let Some(img) = decoded {
            let (art_scale, src_scale) = if is_ui {
                (1, 1)
            } else {
                let (src, dec) = codec::image::art_scales(&path);
                (dec.max(1), src.max(1))
            };
            tlog(if cp932 { "decode (cp932 name)" } else { "decode" }, t1);
            return Some(LoadedAsset { img, art_scale, src_scale });
        }
    }
    None
}

/// Find the file backing one logical asset name without decoding anything.
///
/// Candidate names are tried against every tree with a plain existence check,
/// which is what the extracted data tree answers directly. Only when nothing
/// matches does it fall back to the CP932 name scan needed by trees extracted
/// before the extractor normalized filenames - and that scan is indexed once
/// per directory, because reading 1200 directory entries per candidate was
/// measurably slower than decoding the image afterwards.
///
/// Returns every existing file the name could mean, best first, each flagged
/// with whether it came from the CP932 fallback. The caller decodes them in
/// order, so a file that fails to decode still falls through to the next
/// candidate - which is what the old decode-as-you-search loop did.
pub(crate) fn resolve_asset_path(dirs: &[&std::path::Path], name: &str) -> Vec<(std::path::PathBuf, bool)> {
    let candidates = asset_candidates(name);
    let mut out = Vec::new();
    for dir in dirs {
        for cand in &candidates {
            let p = dir.join(cand);
            if p.is_file() {
                out.push((p, false));
            }
        }
    }
    #[cfg(unix)]
    {
        for dir in dirs {
            for (decoded, path) in cp932_entries(dir).iter() {
                for cand in &candidates {
                    if decoded.eq_ignore_ascii_case(cand) {
                        out.push((path.clone(), true));
                    }
                }
            }
        }
    }
    #[cfg(not(unix))]
    let _ = dirs;
    out
}

/// The names one logical asset may be stored under.
pub(crate) fn asset_candidates(name: &str) -> Vec<String> {
    let mut candidates = vec![name.to_string()];
    let lower = name.to_ascii_lowercase();
    for sfx in ["_bg", "_chr", "_chreff", "_ev", "_fg", "_eff"] {
        if lower.ends_with(sfx) {
            candidates.push(name[..name.len() - sfx.len()].to_string());
        }
    }
    // Each sibling tree keeps the same stem but may use WebP after upscale.
    // .bmp sources (backgrounds extracted from the archive) keep the same
    // stem in the WebP trees as well.
    let base = name
        .trim_end_matches(".tga")
        .trim_end_matches(".TGA")
        .trim_end_matches(".webp")
        .trim_end_matches(".WEBP")
        .trim_end_matches(".bmp")
        .trim_end_matches(".BMP");
    candidates.push(format!("{}.webp", base));
    candidates.push(format!("{}.WEBP", base));
    candidates.push(format!("{}.tga", base));
    candidates.push(format!("{}.TGA", base));
    candidates
}

/// Directory entries with their names CP932-decoded, indexed once per directory.
#[cfg(unix)]
pub(crate) fn cp932_entries(dir: &std::path::Path) -> std::sync::Arc<Vec<(String, std::path::PathBuf)>> {
    use std::collections::HashMap;
    use std::os::unix::ffi::OsStrExt;
    use std::sync::{Arc, Mutex, OnceLock};
    static IDX: OnceLock<Mutex<HashMap<std::path::PathBuf, Arc<Vec<(String, std::path::PathBuf)>>>>> =
        OnceLock::new();
    let mut m = match IDX.get_or_init(Default::default).lock() {
        Ok(m) => m,
        Err(e) => e.into_inner(),
    };
    if let Some(v) = m.get(dir) {
        return Arc::clone(v);
    }
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let raw = entry.file_name();
            v.push((codec::sjis::decode(raw.as_bytes()), entry.path()));
        }
    }
    let v = Arc::new(v);
    m.insert(dir.to_path_buf(), Arc::clone(&v));
    v
}

/// Load one of the game's `.AMP` colour curves by name (case-insensitive).
pub(crate) fn load_tint(dirs: &[&std::path::Path], name: &str) -> Option<Vec<u8>> {
    let want = name.to_ascii_lowercase();
    for dir in dirs {
        // exact first, then a case-insensitive scan (sepia.AMP vs sepia.amp)
        if let Ok(d) = std::fs::read(dir.join(name)) {
            return Some(d);
        }
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                if let Ok(n) = e.file_name().into_string() {
                    if n.to_ascii_lowercase() == want {
                        if let Ok(d) = std::fs::read(e.path()) {
                            return Some(d);
                        }
                    }
                }
            }
        }
    }
    None
}

/// The colour curve actually in force: an explicit ACT172 filter wins over
/// the per-background `.amp` override, which wins over the ACT170 preset.
pub(crate) fn effective_tint_name(
    filter: &Option<String>,
    ambient_override: &Option<String>,
    ambient_preset: &Option<String>,
) -> Option<String> {
    filter
        .clone()
        .or_else(|| ambient_override.clone())
        .or_else(|| ambient_preset.clone())
}

pub(crate) fn ambient_preset_name(preset: i32) -> Option<String> {
    match preset.clamp(0, 3) {
        1 => Some("evening.amp".to_string()),
        2 => Some("night.amp".to_string()),
        3 => Some("indoor.amp".to_string()),
        _ => None,
    }
}

/// Decode the effective curve, logging what happened (the old inline logs).
pub(crate) fn load_tint_logged(dirs: &[&std::path::Path], name: &Option<String>) -> Option<Vec<u8>> {
    match name.as_deref() {
        None => {
            eprintln!("  tint: <none>");
            None
        }
        Some(n) => match load_tint(dirs, n) {
            Some(lut) => {
                eprintln!("  tint: {} ({} bytes)", n, lut.len());
                Some(lut)
            }
            None => {
                eprintln!("  tint: {} NOT FOUND", n);
                None
            }
        },
    }
}

/// Build a solid-color fallback background image.
pub(crate) fn solid_bg(w: u32, h: u32, color: [u8; 3]) -> codec::tga::TgaImage {
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..(w * h) {
        rgba.extend_from_slice(&[color[0], color[1], color[2], 255]);
    }
    codec::tga::TgaImage { width: w, height: h, rgba }
}

/// A character currently on screen.
pub(crate) struct LoadedAsset {
    pub(crate) img: codec::tga::TgaImage,
    /// Effective scale of the decoded pixels relative to 1280x720 layout
    /// coordinates. UI assets have already been normalized and use 1.
    pub(crate) art_scale: u32,
    /// Scale of the source tree the file was read from (4 for `bg_4x`, 8 for
    /// `bg_8x`, 1 for the original TGA tree). Printed next to the asset name so
    /// the log says where the pixels came from.
    pub(crate) src_scale: u32,
}

pub(crate) fn crop_image(
    image: &codec::tga::TgaImage,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
) -> Option<codec::tga::TgaImage> {
    if x >= image.width || y >= image.height {
        return None;
    }
    let w = w.min(image.width - x);
    let h = h.min(image.height - y);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for row in 0..h as usize {
        let src = ((y as usize + row) * image.width as usize + x as usize) * 4;
        let dst = row * w as usize * 4;
        rgba[dst..dst + w as usize * 4]
            .copy_from_slice(&image.rgba[src..src + w as usize * 4]);
    }
    Some(codec::tga::TgaImage { width: w, height: h, rgba })
}

/// The `bgm/` file for a track number, in the order a scene should use it.
///
/// Tracks come in two arrangements. `BGM_001_B` is the whole piece (86 s) and
/// `BGM_001_A` is a short excerpt of it (9 s); the scripts never say which one
/// they want - every ACT158 call in the corpus carries exactly four ints
/// (id, fade frames, loop flag, volume) and nothing else - so the choice is the
/// engine's. Taking `_A` by default played a 5-11 s clip on repeat for the
/// most-used tracks (1, 2, 7, 41, 42), which is what "the BGM loops the first
/// few seconds" was; the full arrangement is the one a scene wants.
///
/// Some ids ship a single file with no `_A`/`_B` suffix at all, hence the plain
/// name first, and `_A` stays as the last resort for tracks that only have it.
pub(crate) fn bgm_file(dir: &std::path::Path, id: i32) -> Option<std::path::PathBuf> {
    let stem = format!("BGM_{:03}", id);
    [format!("{}.OGG", stem), format!("{}_B.OGG", stem), format!("{}_A.OGG", stem)]
        .into_iter()
        .map(|name| dir.join(name))
        .find(|p| p.is_file())
}
