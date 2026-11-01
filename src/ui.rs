//! Shared interface widgets: prompts, toasts, menu bars and label drawing.

use crate::*;

/// How much bigger the canvas is than the 1280x720 design space the UI sheets
/// were authored in. A Retina canvas (2560x1440) is exactly 2; a 1x window is
/// 1, which keeps every 1280x720 frame pixel-identical to before.
pub(crate) fn ui_scale(w: u32, h: u32) -> f32 {
    (w as f32 / render::LAYOUT_W as f32)
        .min(h as f32 / render::LAYOUT_H as f32)
        .clamp(0.5, 4.0)
}

pub(crate) fn centered_plate(w: i32, h: i32) -> (i32, i32) {
    centered_plate_in(render::LAYOUT_W, render::LAYOUT_H, w, h)
}

pub(crate) fn centered_plate_in(cw: u32, ch: u32, w: i32, h: i32) -> (i32, i32) {
    (((cw as i32) - w) / 2, ((ch as i32) - h) / 2)
}

pub(crate) fn in_box(sx: i32, sy: i32, b: (i32, i32, i32, i32)) -> bool {
    sx >= b.0 && sx < b.0 + b.2 && sy >= b.1 && sy < b.1 + b.3
}

/// Yes on the left, no on the right, along the bottom of the confirm plate.
/// `k` keeps the row's proportions on a canvas that is not 1280x720.
pub(crate) fn ask_buttons(px: i32, py: i32, pw: i32, ph: i32, k: f32) -> [(i32, i32, i32, i32); 2] {
    let bw = (160.0 * k).round() as i32;
    let bh = (40.0 * k).round() as i32;
    let pad = (28.0 * k).round() as i32;
    let x = px + (pw - bw * 2) / 2;
    let y = py + ph - bh - pad;
    [(x, y, bw, bh), (x + bw, y, bw, bh)]
}

pub(crate) fn blit_line(
    surf: &mut render::Surface,
    text: Option<&(u32, u32, Vec<u8>)>,
    px: i32,
    py: i32,
    pw: i32,
    ph: i32,
    k: f32,
) {
    if let Some((tw, th, rgba)) = text {
        let x = px + (pw - *tw as i32) / 2;
        let y = py + (ph - *th as i32) / 2 - (8.0 * k).round() as i32;
        surf.blit_rgba(rgba, *tw, *th, x.max(px + 8), y.max(py + 8));
    }
}

pub(crate) fn paint_toast(
    frame: &render::Surface,
    plate: Option<&codec::tga::TgaImage>,
    text: Option<&(u32, u32, Vec<u8>)>,
) -> render::Surface {
    let k = ui_scale(frame.width, frame.height);
    let mut s = clone_surf(frame);
    s.dim(110);
    let (pw, ph) = plate
        .map(|p| (p.width as i32, p.height as i32))
        .unwrap_or((480, 112));
    let (pw, ph) = ((pw as f32 * k).round() as i32, (ph as f32 * k).round() as i32);
    let (px, py) = centered_plate_in(s.width, s.height, pw, ph);
    if let Some(p) = plate {
        s.blit_region_scaled(p, 0, 0, p.width, p.height, px, py, pw as u32, ph as u32, 255);
    } else {
        s.fill_rect(px, py, pw as u32, ph as u32, [16, 90, 104, 255]);
    }
    blit_line(&mut s, text, px, py, pw, ph, k);
    s
}

/// The dimmed still a modal is composited on. Built once per prompt: dimming a
/// 2560x1440 frame every 16 ms to follow the mouse is not affordable.
pub(crate) fn modal_backdrop(frame: &render::Surface, amount: u8) -> render::Surface {
    let mut s = clone_surf(frame);
    s.dim(amount);
    s
}

pub(crate) fn paint_confirm(
    backdrop: &render::Surface,
    plate: Option<&codec::tga::TgaImage>,
    buttons: Option<&codec::tga::TgaImage>,
    text: Option<&(u32, u32, Vec<u8>)>,
    hover: Option<bool>,
) -> render::Surface {
    let k = ui_scale(backdrop.width, backdrop.height);
    let mut s = clone_surf(backdrop);
    let (pw, ph) = plate
        .map(|p| (p.width as i32, p.height as i32))
        .unwrap_or((544, 216));
    let (pw, ph) = ((pw as f32 * k).round() as i32, (ph as f32 * k).round() as i32);
    let (px, py) = centered_plate_in(s.width, s.height, pw, ph);
    if let Some(p) = plate {
        s.blit_region_scaled(p, 0, 0, p.width, p.height, px, py, pw as u32, ph as u32, 255);
    } else {
        s.fill_rect(px, py, pw as u32, ph as u32, [16, 90, 104, 255]);
    }
    // The prompt sits in the upper half; the 是/否 row owns the bottom.
    if let Some((tw, th, rgba)) = text {
        let x = px + (pw - *tw as i32) / 2;
        s.blit_rgba(rgba, *tw, *th, x.max(px + 16), py + (48.0 * k).round() as i32);
    }
    if let Some(img) = buttons {
        let boxes = ask_buttons(px, py, pw, ph, k);
        for (i, rect) in boxes.iter().enumerate() {
            let hot = hover == Some(i == 0);
            let row = if hot { 1 } else { 0 };
            s.blit_region_scaled(
                img,
                (i as i32) * 160,
                row * 40,
                160,
                40,
                rect.0,
                rect.1,
                rect.2 as u32,
                rect.3 as u32,
                255,
            );
        }
    }
    s
}

/// Short confirmation that a quick save landed. A click dismisses it early.
/// `false` means the player closed the window.
pub(crate) fn show_toast(
    win: &mut sdl2::Sdl,
    frame: &render::Surface,
    dirs: &[&std::path::Path],
    message: &str,
) -> bool {
    let plate = load_background(dirs, "sys_00110.tga");
    let size = (22.0 * ui_scale(frame.width, frame.height)).round().max(12.0) as u32;
    let text = render::text::with_ui_font(size, |f| f.line_rgba(message, size)).flatten();
    let surf = paint_toast(frame, plate.as_ref(), text.as_ref());
    win.present(&surf);
    let t0 = std::time::Instant::now();
    while t0.elapsed() < std::time::Duration::from_millis(900) {
        match win.poll_input() {
            sdl2::Input::Quit => return false,
            sdl2::Input::Pointer { pressed: true, .. }
            | sdl2::Input::Advance
            | sdl2::Input::Right
            | sdl2::Input::Back | sdl2::Input::Left => return true,
            _ => win.delay(16),
        }
    }
    true
}

/// Yes / no over `sys_00100` + `sys_00101`. `None` is the window being closed.
pub(crate) fn ask_yes_no(
    win: &mut sdl2::Sdl,
    frame: &render::Surface,
    dirs: &[&std::path::Path],
    message: &str,
    mixer: Option<&audio::Mixer>,
    ui: Option<&UiSound>,
) -> Option<bool> {
    let plate = load_background(dirs, "sys_00100.tga");
    let buttons = load_background(dirs, "sys_00101.tga");
    // The prompt follows the canvas, not the 1280x720 design space: on a
    // Retina frame both the plate and the glyphs are drawn at twice the size
    // instead of being upscaled afterwards.
    let k = ui_scale(frame.width, frame.height);
    let size = (22.0 * k).round().max(12.0) as u32;
    let text = render::text::with_ui_font(size, |f| f.line_rgba(message, size)).flatten();
    let (pw, ph) = plate
        .as_ref()
        .map(|p| (p.width as i32, p.height as i32))
        .unwrap_or((544, 216));
    let (pw, ph) = ((pw as f32 * k).round() as i32, (ph as f32 * k).round() as i32);
    let (px, py) = centered_plate_in(frame.width, frame.height, pw, ph);
    let boxes = ask_buttons(px, py, pw, ph, k);
    let backdrop = modal_backdrop(frame, 140);
    let mut mx = -1i32;
    let mut my = -1i32;
    loop {
        let hover = if in_box(mx, my, boxes[0]) {
            Some(true)
        } else if in_box(mx, my, boxes[1]) {
            Some(false)
        } else {
            None
        };
        let surf = paint_confirm(&backdrop, plate.as_ref(), buttons.as_ref(), text.as_ref(), hover);
        win.present(&surf);
        match win.poll_input() {
            sdl2::Input::Quit => {
                play_ui(mixer, ui, UiSe::Cancel);
                return None;
            }
            sdl2::Input::Back | sdl2::Input::Left | sdl2::Input::RightClick => {
                play_ui(mixer, ui, UiSe::Cancel);
                return Some(false);
            }
            sdl2::Input::Advance | sdl2::Input::Right => {
                play_ui(mixer, ui, UiSe::Confirm);
                return Some(true);
            }
            sdl2::Input::Pointer { x, y, pressed } => {
                let (sx, sy) = win.surface_pos(x, y, frame.width, frame.height);
                mx = sx;
                my = sy;
                if pressed {
                    if in_box(sx, sy, boxes[0]) {
                        play_ui(mixer, ui, UiSe::Confirm);
                        return Some(true);
                    }
                    if in_box(sx, sy, boxes[1]) {
                        play_ui(mixer, ui, UiSe::Cancel);
                        return Some(false);
                    }
                }
            }
            _ => {}
        }
        win.delay(16);
    }
}

/// One line of UI text on a dark plate. `centre` lays it out across the 1280px
/// layout width instead of at `x`, which is what the hint lines want.
pub(crate) fn draw_label(
    frame: &mut render::Surface,
    text: &str,
    size: u32,
    x: i32,
    y: i32,
    centre: bool,
    ww: u32,
    wh: u32,
) {
    let px = ((size as u64 * wh as u64 + 360) / 720).max(8) as u32;
    if let Some((w, h, rgba)) = render::text::with_ui_font(px, |f| f.line_rgba(text, px)).flatten() {
        let left = if centre {
            (ww as i32 - w as i32) / 2
        } else {
            x * ww as i32 / 1280
        };
        let top = y * wh as i32 / 720;
        let pad_x = (6 * ww as i32 / 1280).max(3);
        let pad_y = (4 * wh as i32 / 720).max(2);
        frame.fill_rect(left - pad_x, top - pad_y, w + pad_x as u32 * 2, h + pad_y as u32 * 2, [6, 16, 28, 200]);
        frame.blit_rgba(&rgba, w, h, left, top);
    }
}

/// UI sheet retained at its source resolution. The scene compositor uses
/// logical-size UI sheets; standalone pages such as Music need the 4x/8x
/// pixels so their baked labels stay sharp at fullscreen size.
pub(crate) struct HiUiAsset {
    pub(crate) img: codec::tga::TgaImage,
    pub(crate) scale: u32,
}

pub(crate) fn load_ui_asset_hi(
    dirs: &[&std::path::Path],
    name: &str,
    expected_w: u32,
    expected_h: u32,
) -> Option<HiUiAsset> {
    let mut order: Vec<(&std::path::Path, u32)> = dirs.iter()
        .map(|d| (*d, codec::image::dir_art_scale(d)))
        .collect();
    order.sort_by(|a, b| b.1.cmp(&a.1));
    for (dir, dir_scale) in order {
        for cand in codec::image::candidate_names(name) {
            let path = dir.join(cand);
            let decoded = if dir_scale > 1 {
                codec::image::decode_file(&path)
            } else {
                codec::image::decode_ui_file(&path)
            };
            let Ok(img) = decoded else { continue };
            let scale = if expected_w > 0 && expected_h > 0 {
                (img.width / expected_w).min(img.height / expected_h).max(1)
            } else { 1 };
            if img.width >= expected_w.saturating_mul(scale)
                && img.height >= expected_h.saturating_mul(scale) {
                return Some(HiUiAsset { img, scale });
            }
        }
    }
    None
}

/// Rasterize menu labels once. Each line is the system CJK font, white on
/// transparent, ready to be tinted white or gold per frame.
pub(crate) fn menu_labels(items: &[&str], size: u32) -> Vec<Option<(u32, u32, Vec<u8>)>> {
    items
        .iter()
        .map(|s| render::text::with_ui_font(size, |f| f.line_rgba(s, size)).flatten())
        .collect()
}

/// Font size that has the same height, in window pixels, as `layout_px` on
/// the 720-tall layout. Labels are drawn after the scene is scaled, so this
/// is the size they must be rasterized at.
pub(crate) fn layout_font_px(layout_px: u32, window_h: i32) -> u32 {
    ((layout_px as i32 * window_h.max(1)) / 720).clamp(8, 160) as u32
}

/// Where the highlight sits in window pixels. `layout_y` stays fractional so
/// a maximized window can move the bar one screen pixel at a time.
pub(crate) fn bar_window_y(layout_y: f32, wh: i32) -> i32 {
    (layout_y * wh.max(1) as f32 / 720.0).round() as i32
}

/// Slide the highlight toward `target`. At 720px tall this is the original
/// per-frame ease. A taller window caps each step at about six screen pixels,
/// otherwise one layout pixel becomes several screen pixels and the bar hops.
pub(crate) fn slide_bar(bar_y: &mut f32, target: f32, wh: i32) {
    let left = target - *bar_y;
    let snap = if wh > 720 { 720.0 / wh as f32 } else { 0.5 };
    if left.abs() <= snap {
        *bar_y = target;
        return;
    }
    let mut step = left * 0.30;
    if wh > 720 {
        let max_layout = 6.0 * 720.0 / wh as f32;
        if step.abs() > max_layout {
            step = max_layout.copysign(step);
        }
    }
    *bar_y += step;
}

/// Logo and copyright, already stretched to the window, plus the highlight
/// oval at that same pixel size. Rebuilt only when the window size changes,
/// so the bar can animate without resampling the whole frame.
pub(crate) struct FittedMenu {
    pub(crate) ww: i32,
    pub(crate) wh: i32,
    pub(crate) stamp: u64,
    pub(crate) plate: render::Surface,
    pub(crate) bar: [Option<render::Surface>; 2],
}

pub(crate) fn scale_sprite(
    img: &codec::tga::TgaImage,
    sx: i32,
    sy: i32,
    sw: u32,
    sh: u32,
    dw: u32,
    dh: u32,
) -> render::Surface {
    let mut s = render::Surface::new(sw.max(1), sh.max(1), [0, 0, 0, 0]);
    for row in 0..sh as i32 {
        for col in 0..sw as i32 {
            let sxp = sx + col;
            let syp = sy + row;
            if sxp < 0 || syp < 0 || sxp >= img.width as i32 || syp >= img.height as i32 {
                continue;
            }
            let si = ((syp as u32 * img.width + sxp as u32) * 4) as usize;
            let di = ((row as u32 * s.width + col as u32) * 4) as usize;
            s.pixels[di..di + 4].copy_from_slice(&img.rgba[si..si + 4]);
        }
    }
    if dw == s.width && dh == s.height {
        return s;
    }
    for px in s.pixels.chunks_exact_mut(4) {
        let a = px[3] as u32;
        px[0] = (px[0] as u32 * a / 255) as u8;
        px[1] = (px[1] as u32 * a / 255) as u8;
        px[2] = (px[2] as u32 * a / 255) as u8;
    }
    let mut up = s.scaled_to(dw, dh);
    for px in up.pixels.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a == 0 {
            continue;
        }
        px[0] = (px[0] as u32 * 255 / a).min(255) as u8;
        px[1] = (px[1] as u32 * 255 / a).min(255) as u8;
        px[2] = (px[2] as u32 * 255 / a).min(255) as u8;
    }
    up
}

pub(crate) fn ensure_fitted(
    fitted: &mut Option<FittedMenu>,
    ww: i32,
    wh: i32,
    stamp: u64,
    layout: impl FnOnce() -> render::Surface,
    bar: Option<&codec::tga::TgaImage>,
    bar_w: u32,
    bar_h: u32,
) {
    if let Some(f) = fitted.as_ref() {
        if f.ww == ww && f.wh == wh && f.stamp == stamp {
            return;
        }
    }
    let layout = layout();
    let plate = if ww as u32 == layout.width && wh as u32 == layout.height {
        layout
    } else {
        layout.scaled_to(ww as u32, wh as u32)
    };
    let dw = (bar_w as i32 * ww / 1280).max(1) as u32;
    let dh = (bar_h as i32 * wh / 720).max(1) as u32;
    let mut frames = [None, None];
    if let Some(img) = bar {
        frames[0] = Some(scale_sprite(img, 0, 0, bar_w, bar_h, dw, dh));
        if img.height >= bar_h * 2 {
            frames[1] = Some(scale_sprite(img, 0, bar_h as i32, bar_w, bar_h, dw, dh));
        }
    }
    *fitted = Some(FittedMenu { ww, wh, stamp, plate, bar: frames });
}

pub(crate) fn paint_bar(frame: &mut render::Surface, fitted: &FittedMenu, which: usize, layout_y: f32, x0: i32) {
    let Some(img) = fitted.bar[which].as_ref() else { return };
    let dx = x0 * fitted.ww / 1280;
    let dy = bar_window_y(layout_y, fitted.wh);
    frame.blit_rgba(&img.pixels, img.width, img.height, dx, dy);
}

pub(crate) fn draw_menu_labels(
    surf: &mut render::Surface,
    labels: &[Option<(u32, u32, Vec<u8>)>],
    sel: usize,
    x0: i32,
    y0: i32,
    item_w: i32,
    item_h: i32,
    pitch: i32,
    ww: i32,
    wh: i32,
) {
    let shadow = ((2 * wh) / 720).max(1);
    for (i, label) in labels.iter().enumerate() {
        let Some((w, h, rgba)) = label else { continue };
        let rgb = if i == sel { [254, 232, 136] } else { [255, 255, 255] };
        let ay = y0 + i as i32 * pitch;
        let x0w = x0 * ww / 1280;
        let y0w = ay * wh / 720;
        let x1w = (x0 + item_w) * ww / 1280;
        let y1w = (ay + item_h) * wh / 720;
        blit_tinted_label(
            surf,
            rgba,
            *w,
            *h,
            x0w + (x1w - x0w - *w as i32) / 2,
            y0w + (y1w - y0w - *h as i32) / 2,
            rgb,
            shadow,
        );
    }
}

/// Draw the title menu from the pre-rendered label surfaces.
///
/// Same geometry as [`draw_menu_labels`], but the outline work already happened
/// once in [`label_surface`]; here each label is a single blit.
pub(crate) fn draw_menu_labels_pre(
    surf: &mut render::Surface,
    white: &[Option<render::Surface>],
    gold: &[Option<render::Surface>],
    sel: usize,
    x0: i32,
    y0: i32,
    item_w: i32,
    item_h: i32,
    pitch: i32,
    ww: i32,
    wh: i32,
    stroke: i32,
) {
    for i in 0..white.len().max(gold.len()) {
        let art = if i == sel {
            gold.get(i).and_then(|a| a.as_ref())
        } else {
            white.get(i).and_then(|a| a.as_ref())
        };
        let Some(art) = art else { continue };
        // The surface carries `stroke` pixels of outline on every side, so the
        // glyph box is that much smaller than the image it was painted into.
        let glyph_w = art.width as i32 - 2 * stroke;
        let glyph_h = art.height as i32 - 2 * stroke;
        let ay = y0 + i as i32 * pitch;
        let x0w = x0 * ww / 1280;
        let y0w = ay * wh / 720;
        let x1w = (x0 + item_w) * ww / 1280;
        let y1w = (ay + item_h) * wh / 720;
        let cx = x0w + (x1w - x0w - glyph_w) / 2;
        let cy = y0w + (y1w - y0w - glyph_h) / 2;
        surf.blit_surface(art, cx - stroke, cy - stroke);
    }
}

/// System-font stand-in for one menu cell. The atlas glyphs are white with a
/// navy outline `(35, 45, 66)` about two pixels thick. The outline is drawn
/// first so it surrounds the fill instead of sitting as a drop shadow.
pub(crate) fn blit_tinted_label(
    surf: &mut render::Surface,
    rgba: &[u8],
    w: u32,
    h: u32,
    x: i32,
    y: i32,
    rgb: [u8; 3],
    stroke_px: i32,
) {
    let outline = [35, 45, 66];
    let r = stroke_px.max(1);
    for row in 0..h as i32 {
        for col in 0..w as i32 {
            let i = ((row as u32 * w + col as u32) * 4) as usize;
            let a = rgba[i + 3];
            if a == 0 {
                continue;
            }
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    surf.blend_pixel(
                        x + col + dx,
                        y + row + dy,
                        [outline[0], outline[1], outline[2], a],
                    );
                }
            }
        }
    }
    for row in 0..h as i32 {
        for col in 0..w as i32 {
            let i = ((row as u32 * w + col as u32) * 4) as usize;
            let a = rgba[i + 3];
            if a == 0 {
                continue;
            }
            surf.blend_pixel(x + col, y + row, [rgb[0], rgb[1], rgb[2], a]);
        }
    }
}

/// Pre-render one menu label, outline included, into its own surface.
///
/// `blit_tinted_label` above needs `(2r+1)^2 - 1` blend calls *per glyph pixel*
/// to grow the navy outline. At a 2560x1440 canvas that is tens of millions of
/// blends for five labels, and the title screen paid it on every single frame
/// even though the text only changes when the font size or face does. Doing it
/// once per label and blitting the result costs about a tenth of a millisecond
/// per frame instead of eight.
///
/// The outline is drawn in the same two passes as the direct path (grow, then
/// fill on top), so the result is pixel-identical.
pub(crate) fn label_surface(rgba: &[u8], w: u32, h: u32, rgb: [u8; 3], stroke: i32) -> render::Surface {
    let r = stroke.max(1);
    let out_w = w + 2 * r as u32;
    let out_h = h + 2 * r as u32;
    let mut surf = render::Surface::new(out_w, out_h, [0, 0, 0, 0]);
    let outline = [35u8, 45, 66];
    // Grow: a destination pixel takes the strongest alpha anywhere in reach.
    for y in 0..out_h as i32 {
        for x in 0..out_w as i32 {
            let mut best = 0u8;
            for dy in -r..=r {
                let sy = y + dy - r;
                if sy < 0 || sy >= h as i32 {
                    continue;
                }
                for dx in -r..=r {
                    let sx = x + dx - r;
                    if sx < 0 || sx >= w as i32 {
                        continue;
                    }
                    let a = rgba[((sy as u32 * w + sx as u32) * 4) as usize + 3];
                    if a > best {
                        best = a;
                    }
                }
            }
            if best > 0 {
                surf.blend_pixel(x, y, [outline[0], outline[1], outline[2], best]);
            }
        }
    }
    // Fill: the glyph itself, inset by the stroke radius.
    for row in 0..h as i32 {
        for col in 0..w as i32 {
            let i = ((row as u32 * w + col as u32) * 4) as usize;
            let a = rgba[i + 3];
            if a == 0 {
                continue;
            }
            surf.blend_pixel(col + r, row + r, [rgb[0], rgb[1], rgb[2], a]);
        }
    }
    surf
}
