//! The CG, novel and voice message browsers.

use crate::*;

/// CG stills from `bg/v*.tga`. Left and right (or the two halves of the
/// screen) page through them; Esc or the right button returns to the title.
/// CG mode: a grid of thumbnails, then a zoomable full view of one picture.
///
/// The grid reads the source tree selected for the current window. `cg::load_thumb`
/// resizes 2x/4x masters during decode and keeps only the current page in
/// memory, so the fullscreen grid never enlarges the original 256x144 `tv*`
/// copies or leaves a disk cache behind.
pub(crate) fn browse_cg(
    win: &mut sdl2::Sdl,
    dirs: &[&std::path::Path],
    cn: bool,
    mixer: Option<&audio::Mixer>,
    ui: Option<&UiSound>,
) {
    const W: u32 = 1280;
    const H: u32 = 720;
    // Five 240px cells plus the 8px gaps span the layout exactly:
    // 5*240 + 4*8 = 1232, leaving a 24px margin on each side.
    const CELL_W: u32 = 240;
    const CELL_H: u32 = 144;
    const GAP: u32 = 8;
    const GRID_X: i32 = 24;
    const GRID_Y: i32 = 64;

    let stems = cg::cg_stems(dirs);
    if stems.is_empty() {
        return;
    }
    let pages = stems.len().div_ceil(cg::PER_PAGE);
    eprintln!("cg: {} images, {} pages", stems.len(), pages);
    let mut sel = 0usize;
    let mut debug_page = usize::MAX;
    let mut thumbs: std::collections::HashMap<String, Option<std::sync::Arc<codec::tga::TgaImage>>> =
        std::collections::HashMap::new();
    let mut page_loader: Option<cg::PageLoader> = None;
    let mut thumb_page = usize::MAX;
    let mut thumb_box = (0u32, 0u32);
    let mut thumb_source_scale = 0u32;
    loop {
        let page = sel / cg::PER_PAGE;
        let (ww, wh) = win.window_size();
        let ww = ww.max(1) as u32;
        let wh = wh.max(1) as u32;
        let (preview_w, preview_h) = cg::preview_box(ww, wh);
        let preview_scale = if win.is_fullscreen() || win.is_maximized() { 4 } else { 2 };
        let preview_dirs = cg::preview_dirs(dirs, preview_scale);
        if page != thumb_page
            || thumb_box != (preview_w, preview_h)
            || thumb_source_scale != preview_scale
        {
            thumbs.clear();
            drop(page_loader.take());
            thumb_page = page;
            thumb_box = (preview_w, preview_h);
            thumb_source_scale = preview_scale;
            let first = page * cg::PER_PAGE;
            let last = (first + cg::PER_PAGE).min(stems.len());
            let page_stems: Vec<String> = stems[first..last]
                .iter()
                .filter_map(|stem| {
                    if let Some(img) = cg::peek_thumb(&preview_dirs, stem, preview_w, preview_h) {
                        thumbs.insert(stem.clone(), Some(img));
                        None
                    } else {
                        Some(stem.clone())
                    }
                })
                .collect();
            page_loader = cg::start_page_loader(
                &preview_dirs,
                &page_stems,
                preview_w,
                preview_h,
            );
            if page_loader.is_none() {
                // Thread creation is expected to succeed, but the page must
                // remain usable even under a very restricted process limit.
                for stem in &page_stems {
                    thumbs.insert(
                        stem.clone(),
                        cg::load_thumb(&preview_dirs, stem, preview_w, preview_h),
                    );
                }
            }
        }
        if let Some(loader) = page_loader.as_ref() {
            for (stem, img) in loader.poll() {
                thumbs.insert(stem, img);
            }
        }
        let mut frame = render::Surface::new(ww, wh, [6, 14, 26, 255]);
        if std::env::var_os("WA2_DEBUG_CG").is_some() && page != debug_page {
            eprintln!("cg: grid page={} selection={} window={}x{}", page + 1, stems[sel], ww, wh);
            debug_page = page;
        }
        let info = format!("{}   {} / {}   第 {} / {} 页", stems[sel], sel + 1, stems.len(), page + 1, pages);
        let head = if cn { "CG 鉴赏" } else { "CG 鑑賞" };
        draw_label(&mut frame, head, 26, 24, 18, false, ww, wh);
        draw_label(&mut frame, &info, 18, 1000, 24, false, ww, wh);
        for row in 0..cg::ROWS {
            for col in 0..cg::COLS {
                let idx = page * cg::PER_PAGE + row * cg::COLS + col;
                let Some(stem) = stems.get(idx) else { continue };
                let (lx, ly) = cell_origin(col, row, GRID_X, GRID_Y, CELL_W, CELL_H, GAP);
                let cx = lx * ww as i32 / W as i32;
                let cy = ly * wh as i32 / H as i32;
                let cw = (CELL_W * ww / W).max(1);
                let ch = (CELL_H * wh / H).max(1);
                match thumbs.get(stem) {
                    Some(Some(img)) => {
                        // CGs are not all 16:9. Fit the source into the cell
                        // in both axes so portrait CGs keep their real shape.
                        let (dw, dh) = cg::fit_size(img.width, img.height, cw, ch);
                        let dx = cx + ((cw - dw) / 2) as i32;
                        let dy = cy + ((ch - dh) / 2) as i32;
                        frame.blit_region_scaled(img, 0, 0, img.width, img.height,
                            dx, dy, dw, dh, 255);
                    }
                    Some(None) => {
                        frame.fill_rect(cx, cy, cw, ch, [16, 32, 48, 255]);
                        draw_label(&mut frame, stem, 16, lx + 8, ly + 60, false, ww, wh);
                    }
                    None => {
                        frame.fill_rect(cx, cy, cw, ch, [10, 24, 38, 255]);
                        let loading = if cn { "读取中…" } else { "読み込み中…" };
                        draw_label(&mut frame, loading, 16, lx, ly + 60, true, ww, wh);
                    }
                }
                if idx == sel {
                    highlight_cell(&mut frame, cx, cy, cw, ch);
                }
            }
        }
        let hint = if cn { "方向键选择　滚轮翻页　回车/点击放大　Esc 返回" }
            else { "方向キー選択　ホイールでページ　Enter/クリックで拡大　Esc 戻る" };
        draw_label(&mut frame, hint, 18, 0, H as i32 - 34, true, ww, wh);
        win.present(&frame);
        match win.poll_input() {
            sdl2::Input::Quit | sdl2::Input::RightClick | sdl2::Input::Back => {
                play_ui(mixer, ui, UiSe::Cancel);
                return;
            }
            sdl2::Input::Advance => {
                play_ui(mixer, ui, UiSe::Confirm);
                view_cg(win, dirs, &stems, &mut sel, cn, mixer, ui);
            }
            sdl2::Input::Left => {
                let next = sel.saturating_sub(1);
                if next != sel {
                    sel = next;
                    play_ui(mixer, ui, UiSe::Move);
                }
            }
            sdl2::Input::Right => {
                let next = (sel + 1).min(stems.len() - 1);
                if next != sel {
                    sel = next;
                    play_ui(mixer, ui, UiSe::Move);
                }
            }
            sdl2::Input::Up => {
                let next = sel.saturating_sub(cg::COLS);
                if next != sel {
                    sel = next;
                    play_ui(mixer, ui, UiSe::Move);
                }
            }
            sdl2::Input::Down => {
                let next = (sel + cg::COLS).min(stems.len() - 1);
                if next != sel {
                    sel = next;
                    play_ui(mixer, ui, UiSe::Move);
                }
            }
            sdl2::Input::Wheel { y } => {
                let next = if y > 0 { page.saturating_sub(1) } else { (page + 1).min(pages - 1) };
                let moved = (next * cg::PER_PAGE).min(stems.len() - 1);
                if moved != sel {
                    sel = moved;
                    play_ui(mixer, ui, UiSe::Page);
                }
            }
            sdl2::Input::Pointer { x, y, pressed } => {
                let (sx, sy) = win.surface_pos(x, y, W, H);
                if let Some(idx) = cell_at(sx, sy, page, GRID_X, GRID_Y, CELL_W, CELL_H, GAP, &stems) {
                    if idx != sel {
                        sel = idx;
                        play_ui(mixer, ui, UiSe::Move);
                    } else if pressed {
                        play_ui(mixer, ui, UiSe::Confirm);
                        view_cg(win, dirs, &stems, &mut sel, cn, mixer, ui);
                    }
                }
            }
            _ => win.delay(16),
        }
    }
}

/// Cell origin inside the grid, in layout pixels.
pub(crate) fn cell_origin(col: usize, row: usize, gx: i32, gy: i32, cw: u32, ch: u32, gap: u32) -> (i32, i32) {
    (gx + col as i32 * (cw + gap) as i32, gy + row as i32 * (ch + gap) as i32)
}

/// Which cell a layout point lands on, if the page really has a CG there.
pub(crate) fn cell_at(
    x: i32,
    y: i32,
    page: usize,
    gx: i32,
    gy: i32,
    cw: u32,
    ch: u32,
    gap: u32,
    stems: &[String],
) -> Option<usize> {
    if x < gx || y < gy {
        return None;
    }
    let stride_x = (cw + gap) as i32;
    let stride_y = (ch + gap) as i32;
    let col = (x - gx) / stride_x;
    let row = (y - gy) / stride_y;
    if col < 0 || row < 0 || col >= cg::COLS as i32 || row >= cg::ROWS as i32 {
        return None;
    }
    // Inside a cell, not in the gap between two of them.
    if x - gx - col * stride_x >= cw as i32 || y - gy - row * stride_y >= ch as i32 {
        return None;
    }
    let idx = page * cg::PER_PAGE + row as usize * cg::COLS + col as usize;
    (idx < stems.len()).then_some(idx)
}

pub(crate) fn highlight_cell(frame: &mut render::Surface, cx: i32, cy: i32, cw: u32, ch: u32) {
    let gold = [226, 196, 120, 255];
    frame.fill_rect(cx - 2, cy - 2, cw + 4, 2, gold);
    frame.fill_rect(cx - 2, cy + ch as i32, cw + 4, 2, gold);
    frame.fill_rect(cx - 2, cy - 2, 2, ch + 4, gold);
    frame.fill_rect(cx + cw as i32, cy - 2, 2, ch + 4, gold);
}

/// Zoom levels the full view steps through. 1.0 shows the whole picture.
pub(crate) const CG_ZOOMS: [f32; 4] = [1.0, 1.5, 2.0, 3.0];

/// How much of the visible area one arrow press moves.
pub(crate) const PAN_STEP: f32 = 0.125;

/// Full view of one CG: zoom with the wheel, pan by dragging or the arrows.
///
/// `sel` stays on the last picture looked at, so the grid comes back where the
/// player left it.
pub(crate) fn view_cg(
    win: &mut sdl2::Sdl,
    dirs: &[&std::path::Path],
    stems: &[String],
    sel: &mut usize,
    cn: bool,
    mixer: Option<&audio::Mixer>,
    ui: Option<&UiSound>,
) {
    const W: u32 = 1280;
    const H: u32 = 720;
    let mut zoom = 0usize;
    let mut centre: Option<(f32, f32)> = None;
    let mut img: Option<codec::tga::TgaImage> = None;
    let mut loaded: Option<String> = None;
    let mut last: Option<(i32, i32)> = None;
    loop {
        let stem = stems[*sel].clone();
        if loaded.as_deref() != Some(stem.as_str()) {
            img = load_background(dirs, &format!("{stem}.tga"));
            loaded = Some(stem.clone());
            centre = None;
            if std::env::var_os("WA2_DEBUG_CG").is_some() {
                if let Some(ref im) = img {
                    eprintln!("cg: view {} decoded={}x{} window={:?}", stem, im.width, im.height, win.window_size());
                } else {
                    eprintln!("cg: view {} FAILED to decode", stem);
                }
            }
        }
        let z = CG_ZOOMS[zoom];
        let (ww, wh) = win.window_size();
        let ww = ww.max(1) as u32;
        let wh = wh.max(1) as u32;
        let mut frame = render::Surface::new(ww, wh, [0, 0, 0, 255]);
        if let Some(ref im) = img {
            if z == 1.0 {
                // The overview zoom shows the entire picture. Portrait CGs
                // are letterboxed instead of being squeezed into 16:9.
                let (dw, dh) = cg::fit_size(im.width, im.height, ww, wh);
                frame.blit_region_scaled(
                    im,
                    0,
                    0,
                    im.width,
                    im.height,
                    ((ww - dw) / 2) as i32,
                    ((wh - dh) / 2) as i32,
                    dw,
                    dh,
                    255,
                );
            } else {
                let (vw, vh) = cg_view_crop(im, ww, wh, z);
                let (cx, cy) = centre.unwrap_or((im.width as f32 / 2.0, im.height as f32 / 2.0));
                let sx = (cx - vw / 2.0).clamp(0.0, (im.width as f32 - vw).max(0.0));
                let sy = (cy - vh / 2.0).clamp(0.0, (im.height as f32 - vh).max(0.0));
                // The crop and the window have the same aspect ratio, so this
                // scale fills the screen without distorting the source.
                frame.blit_region_scaled(
                    im,
                    sx as i32,
                    sy as i32,
                    vw.max(1.0) as u32,
                    vh.max(1.0) as u32,
                    0,
                    0,
                    ww,
                    wh,
                    255,
                );
            }
            let scale = if z == 1.0 {
                String::from(if cn { "全图" } else { "全体" })
            } else {
                format!("x{:.1}", z)
            };
            let label = format!("{}  {}  {} / {}", stem, scale, *sel + 1, stems.len());
            draw_label(&mut frame, &label, 18, 24, 20, false, ww, wh);
        }
        let hint = if cn { "←→切换 CG　滚轮/回车缩放　拖动移动　Esc 返回一览" }
            else { "←→CG切替　ホイール/Enter拡大　ドラッグ移動　Esc 一覧へ" };
        draw_label(&mut frame, hint, 18, 0, H as i32 - 34, true, ww, wh);
        win.present(&frame);
        match win.poll_input() {
            sdl2::Input::Quit | sdl2::Input::RightClick | sdl2::Input::Back => {
                play_ui(mixer, ui, UiSe::Cancel);
                return;
            }
            sdl2::Input::Left => {
                if *sel > 0 {
                    *sel -= 1;
                    play_ui(mixer, ui, UiSe::Page);
                }
            }
            sdl2::Input::Right => {
                if *sel + 1 < stems.len() {
                    *sel += 1;
                    play_ui(mixer, ui, UiSe::Page);
                }
            }
            sdl2::Input::Wheel { y } => {
                let next = step_zoom(zoom, y > 0);
                if next != zoom {
                    zoom = next;
                    play_ui(mixer, ui, UiSe::Move);
                }
            }
            sdl2::Input::Advance => {
                let next = step_zoom(zoom, true);
                zoom = next;
                play_ui(mixer, ui, UiSe::Move);
            }
            sdl2::Input::Up => pan_by(&mut centre, img.as_ref(), 0.0, -PAN_STEP, z, ww, wh),
            sdl2::Input::Down => pan_by(&mut centre, img.as_ref(), 0.0, PAN_STEP, z, ww, wh),
            // Dragging pulls the picture along with the cursor.
            sdl2::Input::Drag { x, y } => {
                let (sx, sy) = win.surface_pos(x, y, W, H);
                if let Some((px, py)) = last {
                    pan_by(&mut centre, img.as_ref(),
                        -(sx - px) as f32 / W as f32,
                        -(sy - py) as f32 / H as f32, z, ww, wh);
                }
                last = Some((sx, sy));
            }
            sdl2::Input::Pointer { x, y, .. } => {
                last = Some(win.surface_pos(x, y, W, H));
            }
            sdl2::Input::Release { .. } => last = None,
            _ => win.delay(16),
        }
        if CG_ZOOMS[zoom] == 1.0 {
            centre = None;
        }
    }
}

pub(crate) fn step_zoom(zoom: usize, up: bool) -> usize {
    if up {
        (zoom + 1).min(CG_ZOOMS.len() - 1)
    } else {
        zoom.saturating_sub(1)
    }
}

/// Source rectangle shown by a zoomed CG. At zoom 1 the caller fits the whole
/// image separately; higher zoom levels use a crop matching the window ratio,
/// which is essential for portrait CGs.
pub(crate) fn cg_view_crop(im: &codec::tga::TgaImage, ww: u32, wh: u32, zoom: f32) -> (f32, f32) {
    let aspect = ww.max(1) as f32 / wh.max(1) as f32;
    let mut vw = (im.width as f32 / zoom.max(1.0)).clamp(1.0, im.width as f32);
    let mut vh = vw / aspect;
    if vh > im.height as f32 {
        vh = im.height as f32;
        vw = (vh * aspect).min(im.width as f32);
    }
    if vw > im.width as f32 {
        vw = im.width as f32;
        vh = (vw / aspect).min(im.height as f32);
    }
    (vw.max(1.0), vh.max(1.0))
}

/// Move the view centre by a fraction of the visible area, keeping it inside
/// the picture so no empty band shows. Fractions keep every caller independent
/// of how large the decoded source behind the window happens to be.
pub(crate) fn pan_by(
    centre: &mut Option<(f32, f32)>,
    img: Option<&codec::tga::TgaImage>,
    fx: f32,
    fy: f32,
    zoom: f32,
    ww: u32,
    wh: u32,
) {
    let Some(im) = img else { return };
    let (vw, vh) = cg_view_crop(im, ww, wh, zoom);
    let (cx, cy) = centre.unwrap_or((im.width as f32 / 2.0, im.height as f32 / 2.0));
    let nx = (cx + fx * vw).clamp(vw / 2.0, (im.width as f32 - vw / 2.0).max(vw / 2.0));
    let ny = (cy + fy * vh).clamp(vh / 2.0, (im.height as f32 - vh / 2.0).max(vh / 2.0));
    *centre = Some((nx, ny));
}

/// Pick one of the two electronic-novel chains.  They are real script roots
/// (5000 and 5100), so entering one through the normal game runner preserves
/// its backgrounds, text, voice and save/rewind behaviour.
pub(crate) fn browse_novels(
    win: &mut sdl2::Sdl,
    dirs: &[&std::path::Path],
    cn: bool,
    mixer: Option<&audio::Mixer>,
    ui: Option<&UiSound>,
) -> Option<String> {
    const W: u32 = 1280;
    const H: u32 = 720;
    let ids = ["5000", "5100"];
    // These are the actual first-page titles in the two shipped script roots,
    // rather than invented "序章/终章" categories.
    let labels = if cn {
        ["特别篇　二年前", "序章　一月后，那女孩"]
    } else {
        ["特別編　二年前", "序章　一月後の、あの娘"]
    };
    // The original subpage keeps the same street/snow composition and menu
    // bar as the special-mode page. The previous implementation replaced it
    // with a dark custom panel, which is why it visibly diverged from WA2.
    let bar = load_background(dirs, "T0101.tga");
    let logo = load_background(dirs, "T0011.tga");
    let street = load_background(dirs, "T0000.tga").map(|img| surface_from_tga(&img));
    let weather = load_background(dirs, "weather.tga");
    let mut snow = TitleSnow::new();
    let mut snow_tick = std::time::Instant::now();
    let mut street_scaled: Option<(i32, i32, render::Surface)> = None;
    const ITEM_W: i32 = 320;
    const ITEM_H: i32 = 40;
    const PITCH: i32 = 40;
    const X0: i32 = (W as i32 - ITEM_W) / 2;
    const Y0: i32 = 380;
    let mut labels_img = Vec::new();
    let mut labels_px = 0u32;
    let mut head: Option<(u32, u32, Vec<u8>)> = None;
    let mut head_px = 0u32;
    let mut sel = 0usize;
    loop {
        let (ww, wh) = win.window_size();
        let ww = ww.max(1);
        let wh = wh.max(1);
        if let Some(ref img) = street {
            let rebuild = street_scaled.as_ref().map(|(sw, sh, _)| *sw != ww || *sh != wh).unwrap_or(true);
            if rebuild {
                let dh = ((img.height as i64 * wh as i64) / H as i64).max(wh as i64) as u32;
                street_scaled = Some((ww, wh, img.scaled_to(ww as u32, dh)));
            }
        }
        let mut frame = render::Surface::new(ww as u32, wh as u32, [1, 29, 89, 255]);
        if let Some((_, _, ref img)) = street_scaled {
            let sy = (136 * wh / H as i32).clamp(0, (img.height as i32 - wh).max(0));
            frame.copy_rows_from(img, sy);
        }
        if let Some(ref img) = logo {
            frame.blit_region_scaled(img, 0, 0, img.width, img.height,
                (W as i32 - img.width as i32) * ww / (2 * W as i32),
                24 * wh / H as i32,
                (img.width as i64 * ww as i64 / W as i64) as u32,
                (img.height as i64 * wh as i64 / H as i64) as u32,
                255);
        }
        if let Some(ref sheet) = bar {
            frame.blit_region_scaled(sheet, 0, if sel == 0 { 40 } else { 0 }, sheet.width, ITEM_H as u32,
                X0 * ww / W as i32, (Y0 + sel as i32 * PITCH) * wh / H as i32,
                (ITEM_W * ww / W as i32) as u32, (ITEM_H * wh / H as i32) as u32, 255);
        }
        let generation = render::text::role_generation();
        let want_px = layout_font_px(32, wh);
        if labels_px != want_px {
            labels_px = want_px;
            labels_img = menu_labels(&labels, want_px);
        }
        let want_head_px = layout_font_px(16, wh);
        if head_px != want_head_px {
            head_px = want_head_px;
            head = render::text::with_ui_font(want_head_px, |f| f.line_rgba(if cn { "电子小说" } else { "電子小説" }, want_head_px)).flatten();
        }
        let _ = generation;
        if let Some((w, h, rgba)) = head.as_ref() {
            blit_tinted_label(&mut frame, rgba, *w, *h, (ww - *w as i32) / 2, (Y0 - 44) * wh / H as i32, [255, 255, 255], (2 * wh / H as i32).max(1));
        }
        draw_menu_labels(&mut frame, &labels_img, sel, X0, Y0, ITEM_W, ITEM_H, PITCH, ww, wh);
        if let Some(ref sheet) = weather {
            snow.ensure(ww, wh, sheet);
            let now = std::time::Instant::now();
            snow.step(now.saturating_duration_since(snow_tick).as_secs_f32().min(0.05));
            snow_tick = now;
            snow.paint(&mut frame, wh);
        }
        win.present(&frame);
        match win.poll_input() {
            sdl2::Input::Quit | sdl2::Input::RightClick | sdl2::Input::Back | sdl2::Input::Left => {
                crate::play_ui(mixer, ui, UiSe::Cancel);
                return None;
            }
            sdl2::Input::Up => {
                sel = (sel + 1) % 2;
                crate::play_ui(mixer, ui, UiSe::Move);
            }
            sdl2::Input::Down => {
                sel = (sel + 1) % 2;
                crate::play_ui(mixer, ui, UiSe::Move);
            }
            sdl2::Input::Advance | sdl2::Input::Right | sdl2::Input::Choice(_) => {
                crate::play_ui(mixer, ui, UiSe::Confirm);
                return Some(ids[sel].to_string());
            }
            sdl2::Input::Pointer { x, y, pressed } => {
                let (sx, sy) = win.surface_pos(x, y, W, H);
                for i in 0..2 {
                    let y0 = Y0 + i as i32 * PITCH;
                    if sx >= X0 && sx < X0 + ITEM_W && sy >= y0 && sy < y0 + ITEM_H {
                        if sel != i {
                            sel = i;
                            crate::play_ui(mixer, ui, UiSe::Move);
                        }
                        if pressed {
                            crate::play_ui(mixer, ui, UiSe::Confirm);
                            return Some(ids[sel].to_string());
                        }
                        break;
                    }
                }
            }
            _ => win.delay(16),
        }
    }
}

/// The original 「ボイスメッセージ」 page is a small character-card player.
/// The shipped 9001..9005 banks contain the five heroine message groups and
/// sys_07200..07204 are their corresponding profile cards.
/// Voice message page layout, kept next to its helpers so the hit tests and
/// the drawing cannot drift apart.
pub(crate) const VOICE_PANEL: (i32, i32, u32, u32) = (700, 88, 556, 532);

pub(crate) const VOICE_ROW_X: i32 = 716;

pub(crate) const VOICE_ROW_W: u32 = 524;

pub(crate) const VOICE_ROW_H: u32 = 52;

pub(crate) const VOICE_ROW_PITCH: i32 = 60;

pub(crate) const VOICE_ROW_Y0: i32 = 176;

/// Only this many rows fit between the title and the stop button.
pub(crate) const VOICE_MAX_ROWS: usize = 5;

pub(crate) const VOICE_BTN: (i32, i32, u32, u32) = (716, 520, 220, 56);

/// The original 「ボイスメッセージ」 page: pick a character card, pick one of
/// her messages, play it, stop it.
///
/// Every card and every clip that exists is selectable from the first visit -
/// there is no "heard it before" gate. What the page tracks is what the
/// original shows: the open card, the selected message, whether something is
/// playing, and an explicit notice when the audio is missing (no clips shipped
/// for that character, or no audio device at all).
pub(crate) fn browse_voice_messages(
    win: &mut sdl2::Sdl,
    dirs: &[&std::path::Path],
    cn: bool,
    mixer: Option<&audio::Mixer>,
    ui: Option<&UiSound>,
) {
    const W: u32 = 1280;
    const H: u32 = 720;

    let names = if cn { ["冬马和纱", "小木曾雪菜", "小春", "千晶", "麻理"] }
        else { ["冬馬かずさ", "小木曾雪菜", "小春", "千晶", "麻理"] };
    let voice_dir = dirs.iter().find_map(|d| d.parent()).map(|p| p.join("voice"));
    // One directory scan for all five banks.
    let mut clips: Vec<Vec<std::path::PathBuf>> = vec![Vec::new(); names.len()];
    if let Some(dir) = voice_dir.as_ref() {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for ent in rd.flatten() {
                let p = ent.path();
                let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                // Files are `900{character}_{index:04}_{char}.OGG`.
                let Some(rest) = stem.strip_prefix("900") else { continue };
                let Some(bank) = rest.get(0..1).and_then(|v| v.parse::<usize>().ok()) else { continue };
                if rest.get(1..2) != Some("_") || bank == 0 || bank > names.len() {
                    continue;
                }
                clips[bank - 1].push(p);
            }
        }
    }
    for rows in clips.iter_mut() {
        rows.sort();
    }
    eprintln!(
        "voice messages: {}",
        clips.iter()
            .enumerate()
            .map(|(i, v)| format!("{}={}", names[i], v.len()))
            .collect::<Vec<_>>()
            .join(" ")
    );
    if clips.iter().all(|v| v.is_empty()) {
        eprintln!("voice messages: no 900x_* clips under {:?}; page not shown", voice_dir);
        return;
    }
    let cards: Vec<Option<codec::tga::TgaImage>> = (0..names.len())
        .map(|i| load_background(dirs, &format!("sys_0720{i}.tga")))
        .collect();

    let n_chars = names.len();
    let mut character = 0usize;
    let mut line = 0usize;
    // Stream slot plus what it belongs to, so its row can be marked playing.
    let mut playing: Option<(usize, usize, usize)> = None;
    loop {
        let (ww, wh) = win.window_size();
        let ww = ww.max(1) as u32;
        let wh = wh.max(1) as u32;
        let sx = |x: i32| x * ww as i32 / W as i32;
        let sy = |y: i32| y * wh as i32 / H as i32;
        let sw = |w: u32| (w * ww / W).max(1);
        let sh = |h: u32| (h * wh / H).max(1);
        let count = clips[character].len();
        // A clip that ended on its own clears the marker.
        if let (Some(mx), Some((slot, _, _))) = (mixer, playing) {
            if mx.finished(slot) {
                playing = None;
            }
        }
        let mut frame = render::Surface::new(ww, wh, [8, 24, 38, 255]);
        if let Some(card) = cards[character].as_ref() {
            frame.blit_region_scaled(card, 0, 0, card.width, card.height, 0, 0, ww, wh, 255);
        }
        frame.fill_rect(sx(VOICE_PANEL.0), sy(VOICE_PANEL.1), sw(VOICE_PANEL.2), sh(VOICE_PANEL.3), [4, 14, 28, 205]);
        frame.fill_rect(sx(VOICE_PANEL.0), sy(VOICE_PANEL.1), sw(VOICE_PANEL.2), sh(2), [140, 200, 225, 190]);
        let head = format!("{}  900{}", names[character], character + 1);
        draw_label(&mut frame, &head, 26, VOICE_ROW_X + 4, VOICE_PANEL.1 + 20, false, ww, wh);
        if count == 0 {
            let msg = if cn { "暂无语音消息" } else { "ボイスメッセージなし" };
            let why = match voice_dir.as_ref() {
                Some(d) if cn => format!("{} 下没有 900{}_*.OGG", d.display(), character + 1),
                Some(d) => format!("900{}_*.OGG not found in {}", character + 1, d.display()),
                None => String::from(if cn { "找不到 voice 目录" } else { "voice ディレクトリなし" }),
            };
            draw_label(&mut frame, msg, 22, VOICE_ROW_X + 4, VOICE_ROW_Y0 + 4, false, ww, wh);
            draw_label(&mut frame, &why, 16, VOICE_ROW_X + 4, VOICE_ROW_Y0 + 46, false, ww, wh);
        } else {
            for i in 0..count.min(VOICE_MAX_ROWS) {
                let y = VOICE_ROW_Y0 + i as i32 * VOICE_ROW_PITCH;
                let selected = i == line;
                let sounds = playing.map(|(_, c, l)| c == character && l == i).unwrap_or(false);
                let (fill, bar) = match (selected, sounds) {
                    (true, true) => ([52, 128, 150, 235], [240, 210, 130, 255]),
                    (true, false) => ([30, 74, 104, 230], [180, 220, 235, 235]),
                    (false, true) => ([36, 86, 108, 180], [140, 190, 210, 200]),
                    (false, false) => ([16, 40, 62, 200], [90, 140, 170, 120]),
                };
                frame.fill_rect(sx(VOICE_ROW_X), sy(y), sw(VOICE_ROW_W), sh(VOICE_ROW_H), fill);
                frame.fill_rect(sx(VOICE_ROW_X), sy(y), sw(4), sh(VOICE_ROW_H), bar);
                let label = if cn { format!("语音消息 {}", i + 1) }
                    else { format!("ボイスメッセージ {}", i + 1) };
                draw_label(&mut frame, &label, 20, VOICE_ROW_X + 18, y + 14, false, ww, wh);
                let mark = match (sounds, selected) {
                    (true, _) => Some(if cn { "播放中" } else { "再生中" }),
                    (false, true) => Some(if cn { "回车播放" } else { "Enter" }),
                    _ => None,
                };
                if let Some(m) = mark {
                    draw_label(&mut frame, m, 18, VOICE_ROW_X + VOICE_ROW_W as i32 - 130, y + 15, false, ww, wh);
                }
            }
            if count > VOICE_MAX_ROWS {
                let more = format!("... {} / {}", line + 1, count);
                draw_label(&mut frame, &more, 16, VOICE_ROW_X + 4,
                    VOICE_ROW_Y0 + VOICE_MAX_ROWS as i32 * VOICE_ROW_PITCH, false, ww, wh);
            }
        }
        // The button stays drawn either way so the layout never jumps.
        let armed = playing.is_some();
        frame.fill_rect(sx(VOICE_BTN.0), sy(VOICE_BTN.1), sw(VOICE_BTN.2), sh(VOICE_BTN.3),
            if armed { [60, 96, 120, 235] } else { [24, 44, 62, 200] });
        frame.fill_rect(sx(VOICE_BTN.0), sy(VOICE_BTN.1), sw(VOICE_BTN.2), sh(2),
            if armed { [200, 170, 110, 220] } else { [90, 130, 155, 150] });
        let btn_text = if cn {
            if armed { "停止 (空格)" } else { "停止" }
        } else if armed {
            "停止 (Space)"
        } else {
            "停止"
        };
        draw_label(&mut frame, btn_text, 20, VOICE_BTN.0 + 18, VOICE_BTN.1 + 16, false, ww, wh);
        let hint = match (mixer.is_some(), count > 0) {
            (false, _) if cn => "音频设备不可用，无法试听　Esc 返回",
            (false, _) => "オーディオ機器なし　Esc 戻る",
            (true, true) if cn => "↑↓ 切换角色　←→ 选择语音　空格 播放/停止　Esc 返回",
            (true, true) => "↑↓ キャラ切替　←→ 音声選択　Space 再生/停止　Esc 戻る",
            (true, false) if cn => "↑↓ 切换角色　Esc 返回",
            (true, false) => "↑↓ キャラ切替　Esc 戻る",
        };
        draw_label(&mut frame, hint, 18, 0, H as i32 - 34, true, ww, wh);
        win.present(&frame);
        match win.poll_input() {
            sdl2::Input::Quit | sdl2::Input::RightClick | sdl2::Input::Back => {
                voice_stop(mixer, &mut playing);
                play_ui(mixer, ui, UiSe::Cancel);
                return;
            }
            // Changing character or message always stops the current clip.
            sdl2::Input::Up => {
                character = (character + n_chars - 1) % n_chars;
                voice_stop(mixer, &mut playing);
                voice_clamp(&mut line, clips[character].len());
                play_ui(mixer, ui, UiSe::Move);
            }
            sdl2::Input::Down => {
                character = (character + 1) % n_chars;
                voice_stop(mixer, &mut playing);
                voice_clamp(&mut line, clips[character].len());
                play_ui(mixer, ui, UiSe::Move);
            }
            sdl2::Input::Left if count > 0 => {
                line = (line + count - 1) % count;
                play_ui(mixer, ui, UiSe::Move);
                voice_stop(mixer, &mut playing);
            }
            sdl2::Input::Right if count > 0 => {
                line = (line + 1) % count;
                play_ui(mixer, ui, UiSe::Move);
                voice_stop(mixer, &mut playing);
            }
            sdl2::Input::Advance | sdl2::Input::Choice(_) => {
                if playing.is_some() {
                    voice_stop(mixer, &mut playing);
                    play_ui(mixer, ui, UiSe::Cancel);
                } else if let Some(path) = clips[character].get(line) {
                    let ok = voice_play(mixer, &mut playing, path, character, line);
                    if ok {
                        play_ui(mixer, ui, UiSe::Confirm);
                    }
                    eprintln!(
                        "voice message: {} {} -> {}",
                        names[character],
                        if ok { "playing" } else { "FAILED" },
                        path.display(),
                    );
                }
            }
            sdl2::Input::Pointer { x, y, pressed: true } => {
                let (sx, sy) = win.surface_pos(x, y, W, H);
                if point_in(sx, sy, VOICE_BTN) {
                    if playing.is_some() {
                        voice_stop(mixer, &mut playing);
                        play_ui(mixer, ui, UiSe::Cancel);
                    }
                } else if let Some(i) = voice_row_at(sx, sy, count) {
                    if i != line {
                        line = i;
                        voice_stop(mixer, &mut playing);
                        play_ui(mixer, ui, UiSe::Move);
                    } else if playing.is_some() {
                        voice_stop(mixer, &mut playing);
                        play_ui(mixer, ui, UiSe::Cancel);
                    } else if let Some(path) = clips[character].get(i) {
                        if voice_play(mixer, &mut playing, path, character, i) {
                            play_ui(mixer, ui, UiSe::Confirm);
                        }
                    }
                } else if sy < 360 {
                    // Card art above the panel: left half back, right half on.
                    character = if sx < 640 {
                        (character + n_chars - 1) % n_chars
                    } else {
                        (character + 1) % n_chars
                    };
                    voice_stop(mixer, &mut playing);
                    voice_clamp(&mut line, clips[character].len());
                    play_ui(mixer, ui, UiSe::Move);
                }
            }
            _ => win.delay(16),
        }
    }
}

/// Stop whatever the page is playing and forget the marker.
pub(crate) fn voice_stop(mixer: Option<&audio::Mixer>, playing: &mut Option<(usize, usize, usize)>) {
    if let (Some(mx), Some((slot, _, _))) = (mixer, playing.take()) {
        mx.stop(slot);
    }
}

/// Start one message, replacing anything already playing. False when there is
/// no audio device or the decoder could not be started.
pub(crate) fn voice_play(
    mixer: Option<&audio::Mixer>,
    playing: &mut Option<(usize, usize, usize)>,
    path: &std::path::Path,
    character: usize,
    line: usize,
) -> bool {
    let Some(mx) = mixer else { return false };
    voice_stop(mixer, playing);
    let slot = mx.play_cat(&path.to_string_lossy(), 1.0, false, audio::mixer::CAT_VOICE);
    if slot == usize::MAX {
        return false;
    }
    *playing = Some((slot, character, line));
    true
}

/// Keep the selected row inside a bank that may be shorter than the last one.
pub(crate) fn voice_clamp(line: &mut usize, count: usize) {
    if *line >= count {
        *line = count.saturating_sub(1);
    }
}

pub(crate) fn point_in(x: i32, y: i32, r: (i32, i32, u32, u32)) -> bool {
    x >= r.0 && x < r.0 + r.2 as i32 && y >= r.1 && y < r.1 + r.3 as i32
}

/// Which message row a layout point lands on, if the bank has it.
pub(crate) fn voice_row_at(x: i32, y: i32, count: usize) -> Option<usize> {
    let i = (y - VOICE_ROW_Y0) / VOICE_ROW_PITCH;
    if i < 0 || i as usize >= count.min(VOICE_MAX_ROWS) {
        return None;
    }
    if !point_in(x, y, (VOICE_ROW_X, VOICE_ROW_Y0 + i * VOICE_ROW_PITCH, VOICE_ROW_W, VOICE_ROW_H)) {
        return None;
    }
    Some(i as usize)
}
