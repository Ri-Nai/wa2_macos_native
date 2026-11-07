//! The save / load screen (「セーブ」/「ロード」).
//!
//! Like the option screen everything is drawn from the shipped layers, which
//! are pixel aligned with each other:
//!
//! ```text
//!   sys_05200        top banner, "DATA PAGE" (1000x24)
//!   sys_05900        one slot panel, 272x160, normal
//!   sys_05901        the same panel lit (the slot under the cursor)
//!   sys_05903        the empty slot (fully transparent)
//!   sys_05100        gold frame drawn over the slot under the cursor
//!   sys_05110        drop shadow, drawn under a panel
//! ```
//!
//! The screen is a paged grid of twelve 272x160 cells, which is what fits on
//! 1280x720; the original addresses up to 100 slots (`save_%02d.sav`), so
//! there are nine pages.

use crate::codec::tga::TgaImage;
use crate::render::Surface;
use crate::save::{self, SaveState, PER_PAGE, SLOT_COUNT};
use crate::sdl2::{self, Sdl};
use std::path::Path;

const W: u32 = 1280;
const H: u32 = 720;

const CELL_W: i32 = 272;
const CELL_H: i32 = 160;
const GAP: i32 = 16;
const COLS: i32 = 4;
const ROWS: i32 = 3;
const GRID_W: i32 = COLS * CELL_W + (COLS - 1) * GAP;
const GRID_X: i32 = (W as i32 - GRID_W) / 2;
const GRID_Y: i32 = 60;

/// Usable area inside the gold frame (sys_05100's border is ~8 px).
const FRAME_X: i32 = 8;
const FRAME_Y: i32 = 8;
const FRAME_W: i32 = 259;
const FRAME_H: i32 = 136;
/// The screen is 16:9, the frame is not: a full frame is letterboxed instead
/// of stretched, so 242x136 with a small pillar on each side.
const THUMB_W: i32 = 242;
const THUMB_H: i32 = FRAME_H;

/// Caption strip over the bottom of the thumbnail.
const CAP_H: i32 = 22;
const HINT_W: i32 = 420;
const HINT_Y: i32 = 584;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Save,
    Load,
}

struct Assets {
    header: Option<TgaImage>,
    panel: Option<TgaImage>,
    panel_hi: Option<TgaImage>,
    frame: Option<TgaImage>,
    shadow: Option<TgaImage>,
}

fn load(dirs: &[&Path], name: &str) -> Option<TgaImage> {
    let mut scaled: Vec<(&Path, u32)> = dirs
        .iter()
        .map(|dir| (*dir, crate::codec::image::dir_art_scale(dir)))
        .filter(|(_, scale)| *scale > 1)
        .collect();
    scaled.sort_by(|a, b| b.1.cmp(&a.1));
    for (dir, _) in scaled.into_iter().chain(dirs.iter().map(|dir| (*dir, 1))) {
        for cand in crate::codec::image::candidate_names(name) {
            let path = dir.join(cand);
            let decoded = if crate::codec::image::dir_art_scale(dir) > 1 {
                crate::codec::image::decode_file(&path)
            } else {
                crate::codec::image::decode_ui_file(&path)
            };
            if let Ok(img) = decoded {
                return Some(img);
            }
        }
    }
    eprintln!("saveload: {} NOT FOUND", name);
    None
}

impl Assets {
    fn load(dirs: &[&Path]) -> Assets {
        Assets {
            // The save/load banner is "DATA PAGE" (`sys_05200`). `sys_05000`
            // is the CG-mode banner ("CG Mode / CGモード") and `sys_05021`
            // is the CG gallery's caption footer ("鉴赏事件CG / CG MODE"), so
            // neither belongs on this screen.
            header: load(dirs, "sys_05200.tga"),
            panel: load(dirs, "sys_05900.tga"),
            panel_hi: load(dirs, "sys_05901.tga"),
            frame: load(dirs, "sys_05100.tga"),
            shadow: load(dirs, "sys_05110.tga"),
        }
    }
}

struct Screen {
    page: usize,
    cursor: usize,
}

fn cell_pos(i: usize) -> (i32, i32) {
    let c = (i as i32) % COLS;
    let r = (i as i32) / COLS;
    (GRID_X + c * (CELL_W + GAP), GRID_Y + r * (CELL_H + GAP))
}

/// Which cell, if any, a screen position falls on.
fn cell_at(sx: i32, sy: i32, page: usize) -> Option<usize> {
    for i in 0..save::cells_on_page(page) {
        let (x, y) = cell_pos(i);
        if sx >= x && sx < x + CELL_W && sy >= y && sy < y + CELL_H {
            return Some(i);
        }
    }
    None
}

fn caption_of(i: usize, st: Option<&SaveState>) -> String {
    match st {
        Some(s) => format!("{:02} {} {}", i, s.time, s.caption()),
        None => format!("{:02} -", i),
    }
}

/// Write one slot: the state file plus the thumbnail shown on the grid.
fn write_slot(i: usize, st: &SaveState, frame: &Surface) -> std::io::Result<()> {
    st.save_to(&save::slot_path(i))?;
    let t = frame.thumbnail(THUMB_W as u32, THUMB_H as u32);
    let path = save::thumb_path(i);
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let mut out = Surface::new(t.width, t.height, [0, 0, 0, 255]);
    out.blit(&t, 0, 0);
    out.save_tga(&path.to_string_lossy())
}

/// Run the screen. Returns the state to resume, i.e. `Some` only when the
/// player loaded a slot.
///
/// `frame` is the screen as it looked when the screen was opened: it is
/// dimmed behind the grid and used as the thumbnail when saving.
pub fn run(
    win: &mut Sdl,
    dirs: &[&Path],
    mode: Mode,
    snap: Option<SaveState>,
    frame: Option<&Surface>,
    mixer: Option<&crate::audio::mixer::Mixer>,
    ui: Option<&crate::UiSound>,
) -> Option<SaveState> {
    let assets = Assets::load(dirs);
    let mut mode = mode;
    if mode == Mode::Save && (snap.is_none() || frame.is_none()) {
        mode = Mode::Load;
    }
    let pages = save::page_count();
    let mut slots: Vec<Option<SaveState>> = save::list();
    let mut thumbs: Vec<Option<TgaImage>> = (0..SLOT_COUNT)
        .map(|i| {
            std::fs::read(save::thumb_path(i)).ok().and_then(|d| crate::codec::tga::decode(&d).ok())
        })
        .collect();
    let mut scr = Screen { page: 0, cursor: 0 };
    // Captions go through the system font, so they are rasterized once per
    // page instead of once per frame.
    let mut caps: Option<(u32, u32, Vec<u8>)> = None;
    let mut hint: Option<(u32, u32, Vec<u8>)> = None;
    let mut caps_key = (usize::MAX, Mode::Save, 0i32, 0i32);

    loop {
        let n = save::cells_on_page(scr.page);
        let page_slots: Vec<Option<&SaveState>> = (0..n)
            .map(|i| slots[scr.page * PER_PAGE + i].as_ref())
            .collect();
        let mut s = match frame {
            Some(f) => Surface { width: f.width, height: f.height, pixels: f.pixels.clone() },
            None => Surface::new(W, H, [0, 0, 0, 255]),
        };
        let key = (scr.page, mode, s.width as i32, s.height as i32);
        if key != caps_key {
            let (c, h) = render_captions(&scr, &slots, mode, pages, s.width, s.height);
            caps = c;
            hint = h;
            caps_key = key;
        }
        compose(&mut s, &assets, &scr, &page_slots, &thumbs[scr.page * PER_PAGE..], caps.as_ref(), hint.as_ref());
        win.present(&s);

        let mut activate: Option<usize> = None;
        match win.poll_input() {
            sdl2::Input::Quit | sdl2::Input::Back | sdl2::Input::RightClick => {
                crate::play_ui(mixer, ui, crate::UiSe::Cancel);
                return None;
            }
            // 1 / 2 switch between saving and loading; the shipped banner only
            // carries the Japanese word for whichever mode was opened.
            sdl2::Input::Choice(1) if snap.is_some() && frame.is_some() => {
                mode = Mode::Save;
                crate::play_ui(mixer, ui, crate::UiSe::Page);
            }
            sdl2::Input::Choice(2) => {
                mode = Mode::Load;
                crate::play_ui(mixer, ui, crate::UiSe::Page);
            }
            sdl2::Input::Up => {
                if scr.cursor == 0 {
                    scr.page = (scr.page + pages - 1) % pages;
                    scr.cursor = save::cells_on_page(scr.page).saturating_sub(1);
                } else {
                    scr.cursor -= 1;
                }
                crate::play_ui(mixer, ui, crate::UiSe::Move);
            }
            sdl2::Input::Down => {
                if scr.cursor + 1 >= save::cells_on_page(scr.page) {
                    scr.page = (scr.page + 1) % pages;
                    scr.cursor = 0;
                } else {
                    scr.cursor += 1;
                }
                crate::play_ui(mixer, ui, crate::UiSe::Move);
            }
            sdl2::Input::Advance | sdl2::Input::Right => {
                activate = Some(scr.cursor);
                crate::play_ui(mixer, ui, crate::UiSe::Confirm);
            }
            sdl2::Input::Pointer { x, y, pressed } => {
                let (sx, sy) = win.surface_pos(x, y, W, H);
                if let Some(i) = cell_at(sx, sy, scr.page) {
                    let moved = scr.cursor != i;
                    scr.cursor = i;
                    if moved {
                        crate::play_ui(mixer, ui, crate::UiSe::Move);
                    }
                    if pressed {
                        activate = Some(i);
                        crate::play_ui(mixer, ui, crate::UiSe::Confirm);
                    }
                }
            }
            _ => {}
        }

        if let Some(i) = activate {
            let Some(idx) = save::slot_index(scr.page, i) else {
                continue;
            };
            match mode {
                Mode::Save => {
                    if let (Some(st), Some(f)) = (snap.as_ref(), frame) {
                        match write_slot(idx, st, f) {
                            Ok(()) => {
                                eprintln!("save: slot {} <- scene {} pc {}", idx, st.scene, st.vm.pc);
                                slots[idx] = SaveState::load_from(&save::slot_path(idx));
                                thumbs[idx] = std::fs::read(save::thumb_path(idx))
                                    .ok()
                                    .and_then(|d| crate::codec::tga::decode(&d).ok());
                                caps_key = (usize::MAX, Mode::Save, 0, 0);
                            }
                            Err(e) => eprintln!("save: slot {}: {}", idx, e),
                        }
                    }
                }
                Mode::Load => {
                    if let Some(st) = SaveState::load_from(&save::slot_path(idx)) {
                        return Some(st);
                    }
                }
            }
        }
        win.delay(16);
    }
}

/// One system-font line per slot, plus a key hint under the grid.
fn render_captions(
    scr: &Screen,
    slots: &[Option<SaveState>],
    mode: Mode,
    pages: usize,
    out_w: u32,
    out_h: u32,
) -> (
    Option<(u32, u32, Vec<u8>)>,
    Option<(u32, u32, Vec<u8>)>,
) {
    let n = save::cells_on_page(scr.page);
    let lines: Vec<String> = (0..n)
        .map(|i| caption_of(scr.page * PER_PAGE + i, slots[scr.page * PER_PAGE + i].as_ref()))
        .collect();
    let scale_x = out_w as f32 / W as f32;
    let scale_y = out_h as f32 / H as f32;
    let font_px = (14.0 * scale_y).round().max(8.0) as u32;
    let cap_w = (FRAME_W as f32 * scale_x).round().max(1.0) as u32;
    let cap_h = (CAP_H as f32 * scale_y).round().max(1.0) as u32;
    let caps = crate::render::text::with_ui_font(font_px, |f| {
        f.block_rgba(&lines, cap_w, (n as u32) * cap_h, font_px)
    })
    .flatten();
    let hint_text = format!(
        "[1]保存 [2]读档   现在:[{}]   [Enter]确定  [Esc]返回   {}/{} 页",
        if mode == Mode::Save { "保存" } else { "读档" },
        scr.page + 1,
        pages
    );
    let hint_w = (HINT_W as f32 * scale_x).round().max(1.0) as u32;
    let hint = crate::render::text::with_ui_font(font_px, |f| {
        f.block_rgba(&[hint_text], hint_w, cap_h, font_px)
    })
    .flatten();
    (caps, hint)
}
fn compose(
    s: &mut Surface,
    a: &Assets,
    scr: &Screen,
    slots: &[Option<&SaveState>],
    thumbs: &[Option<TgaImage>],
    caps: Option<&(u32, u32, Vec<u8>)>,
    hint: Option<&(u32, u32, Vec<u8>)>,
) {
    let sx = s.width as f32 / W as f32;
    let sy = s.height as f32 / H as f32;
    let px = |v: i32| (v as f32 * sx).round() as i32;
    let py = |v: i32| (v as f32 * sy).round() as i32;
    let pw = |v: i32| (v as f32 * sx).round().max(1.0) as u32;
    let ph = |v: i32| (v as f32 * sy).round().max(1.0) as u32;
    s.fill_rect(0, 0, s.width, s.height, [0, 0, 0, 190]);
    if let Some(h) = a.header.as_ref() {
        // "DATA PAGE" banner, 1000x24 in layout space; keep its aspect instead
        // of stretching it to the full width (the old full-width banner was the
        // CG-mode strip, which is a different, taller art).
        let hw = (1000.0 * sx).round().max(1.0) as u32;
        let hh = (24.0 * sy).round().max(1.0) as u32;
        s.blit_region_scaled(h, 0, 0, h.width, h.height, 0, 0, hw, hh, 255);
    }
    for i in 0..slots.len() {
        let (cx, cy) = cell_pos(i);
        if let Some(sh) = a.shadow.as_ref() {
            s.blit_region_scaled(sh, 0, 0, sh.width, sh.height, px(cx), py(cy), pw(CELL_W), ph(CELL_H), 255);
        }
        // Every slot keeps its panel even when empty: `sys_05903` is fully
        // transparent, so an empty cell would otherwise be invisible and there
        // would be nothing to aim at.
        let panel = if scr.cursor == i { a.panel_hi.as_ref() } else { a.panel.as_ref() };
        if let Some(p) = panel {
            s.blit_region_scaled(p, 0, 0, p.width, p.height, px(cx), py(cy), pw(CELL_W), ph(CELL_H), 255);
        }
        // Thumbnail, letterboxed inside the frame.
        if let Some(t) = thumbs.get(i).and_then(|t| t.as_ref()) {
            let tx = cx + FRAME_X + (FRAME_W - t.width as i32) / 2;
            let ty = cy + FRAME_Y + (FRAME_H - t.height as i32) / 2;
            s.blit_region_scaled(t, 0, 0, t.width, t.height, px(tx), py(ty), pw(t.width as i32), ph(t.height as i32), 255);
        }
        // Caption over the bottom of the thumbnail.
        if let Some((cw, _, rgba)) = caps {
            let slot_h = (*rgba).len() as u32 / (*cw * 4).max(1) / slots.len().max(1) as u32;
            let sy = (i as u32) * slot_h;
            let mut strip = Vec::with_capacity((*cw as usize) * slot_h as usize * 4);
            for row in sy..sy + slot_h {
                let off = (row * *cw * 4) as usize;
                strip.extend_from_slice(&rgba[off..off + (*cw as usize) * 4]);
            }
            s.fill_rect(px(cx + FRAME_X), py(cy + FRAME_Y + FRAME_H - CAP_H), pw(FRAME_W), ph(CAP_H), [0, 0, 0, 170]);
            s.blit_rgba(&strip, *cw, slot_h, px(cx + FRAME_X), py(cy + FRAME_Y + FRAME_H - CAP_H));
        }
        if scr.cursor == i {
            if let Some(f) = a.frame.as_ref() {
                s.blit_region_scaled(f, 0, 0, f.width, f.height, px(cx), py(cy), pw(CELL_W), ph(CELL_H), 255);
            }
        }
    }
    if let Some((hw, _, rgba)) = hint {
        let x0 = (W as i32 - HINT_W) / 2;
        s.fill_rect(px(x0 - 10), py(HINT_Y - 2), pw(HINT_W + 20), ph(CAP_H + 4), [0, 0, 0, 170]);
        let hh = rgba.len() as u32 / (*hw * 4).max(1);
        s.blit_rgba(rgba, *hw, hh, px(x0), py(HINT_Y));
    }
}
