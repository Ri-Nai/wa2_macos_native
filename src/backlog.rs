//! The history screen.

use crate::*;

/// One visual row of the text history. An empty text is the gap between two
/// logged lines; a name row is drawn with the speaker style so the history
/// reads like the dialog it came from.
pub(crate) struct BacklogRow {
    pub(crate) text: String,
    pub(crate) name: bool,
}

/// Wrap the history into rows. With a vector font the wrap follows the font's
/// own advances instead of the atlas cell pitch, so a Chinese line stops using
/// the Japanese layout's 28px-per-character rhythm.
pub(crate) fn backlog_rows(lines: &[String], vf: Option<&render::text::VectorFont>) -> Vec<BacklogRow> {
    let max_w = 1080;
    let mut rows = Vec::new();
    for line in lines {
        let (name, body) = match line.split_once('：') {
            Some((n, b)) if !n.is_empty() => (Some(n), b),
            _ => (None, line.as_str()),
        };
        if let Some(n) = name {
            push_wrapped(&mut rows, n, max_w, vf, true);
        }
        if body.is_empty() {
            rows.push(BacklogRow { text: String::new(), name: false });
        } else {
            for part in body.split('\n') {
                push_wrapped(&mut rows, part, max_w, vf, false);
            }
        }
        rows.push(BacklogRow { text: String::new(), name: false });
    }
    rows
}

pub(crate) fn push_wrapped(
    rows: &mut Vec<BacklogRow>,
    text: &str,
    max_w: i32,
    vf: Option<&render::text::VectorFont>,
    name: bool,
) {
    let mut buf = String::new();
    let mut x = 0i32;
    for ch in text.chars() {
        if ch == '\r' {
            continue;
        }
        let adv = match vf {
            Some(f) => f.measure_char(ch),
            None if (ch as u32) < 128 => 14,
            None => 28,
        };
        if x + adv > max_w && !buf.is_empty() {
            rows.push(BacklogRow { text: std::mem::take(&mut buf), name });
            x = 0;
        }
        buf.push(ch);
        x += adv;
    }
    if !buf.is_empty() {
        rows.push(BacklogRow { text: buf, name });
    }
}

pub(crate) fn paint_backlog(
    scene: &render::Surface,
    font: &render::Font,
    // Chinese mode draws the history with the vector font; Japanese keeps the
    // bitmap atlas, whose cell pitch the row layout below is built around.
    mut vf: Option<&mut render::text::VectorFont>,
    rows: &[BacklogRow],
    top: usize,
    per_page: usize,
    header: Option<&codec::tga::TgaImage>,
    scroll: Option<&codec::tga::TgaImage>,
    hover_bar: bool,
    dragging: bool,
) -> render::Surface {
    const LINE_H: i32 = 32;
    const TEXT_X: i32 = 72;
    const TEXT_Y: i32 = 52;
    const TEXT_BOTTOM: i32 = 688;
    const BAR_X: i32 = 1232;
    const BAR_Y: i32 = 40;
    const BAR_H: i32 = 640;
    const COL_W: i32 = 32;
    let mut surf = render::Surface {
        width: scene.width,
        height: scene.height,
        pixels: scene.pixels.clone(),
    };
    surf.dim(120);
    if let Some(h) = header {
        surf.blit(h, 0, 0);
    }
    let mut y = TEXT_Y;
    // The history screen is authored at 1280x720 like the rest of its panels,
    // so the vector text stays at the layout size rather than scaling with the
    // window.
    if let Some(f) = vf.as_mut() {
        let _ = f.set_size(render::DIALOG_FONT_720);
    }
    let body_style = render::text::TextStyle::for_role(render::text::TextRole::Body);
    let name_style = render::text::TextStyle::for_role(render::text::TextRole::Name);
    for row in rows.iter().skip(top).take(per_page) {
        if !row.text.is_empty() {
            match vf.as_mut() {
                Some(f) => {
                    let f = &mut **f;
                    let style = if row.name { name_style } else { body_style };
                    let baseline = f.baseline_in_box(y, LINE_H);
                    f.draw_styled(&mut surf, &row.text, TEXT_X, baseline, &style);
                }
                None => font.draw_pitch(&mut surf, &row.text, TEXT_X, y, render::DIALOG_PITCH),
            }
        }
        y += LINE_H;
        if y >= TEXT_BOTTOM {
            break;
        }
    }
    let total = rows.len();
    let max_top = total.saturating_sub(per_page);
    if let Some(sb) = scroll {
        let col = if dragging {
            2
        } else if hover_bar {
            1
        } else {
            0
        };
        surf.blit_region(sb, col * COL_W, 0, COL_W as u32, BAR_H as u32, BAR_X, BAR_Y);
        if max_top > 0 {
            let track_y = BAR_Y + COL_W;
            let track_h = BAR_H - COL_W * 2;
            let thumb_h = ((track_h as usize) * per_page / total.max(1)).max(28) as i32;
            let thumb_h = thumb_h.min(track_h);
            let thumb_y = track_y + (track_h - thumb_h) * top as i32 / max_top as i32;
            let src_h = thumb_h.min(BAR_H - 80).max(8) as u32;
            surf.blit_region(sb, 2 * COL_W, 80, COL_W as u32, src_h, BAR_X, thumb_y);
        }
    }
    surf
}

/// 文本历史. The banner is the game's own `sys_00032` ("Backlog"), the bar on
/// the right is `sys_00030`, and the lines sit on the dimmed scene in the
/// same face as the message window.
pub(crate) fn show_backlog(
    win: &mut sdl2::Sdl,
    font: &render::Font,
    // Owned by the screen: the History role's font for this visit.
    mut vf: Option<render::text::VectorFont>,
    lines: &[String],
    scene: &render::Surface,
    dirs: &[&std::path::Path],
) {
    const W: u32 = 1280;
    const LINE_H: i32 = 32;
    const TEXT_X: i32 = 72;
    const TEXT_Y: i32 = 52;
    const TEXT_BOTTOM: i32 = 688;
    const BAR_X: i32 = 1232;
    const BAR_Y: i32 = 40;
    const BAR_H: i32 = 640;
    const COL_W: i32 = 32;
    let rows = backlog_rows(lines, vf.as_ref());
    let per_page = ((TEXT_BOTTOM - TEXT_Y) / LINE_H) as usize;
    let total = rows.len();
    let max_top = total.saturating_sub(per_page);
    let mut top = max_top;
    let header = load_background(dirs, "sys_00032.tga");
    let scroll = load_background(dirs, "sys_00030.tga");
    let mut pointer: Option<(i32, i32)> = None;
    let mut drag_top: Option<i32> = None;
    loop {
        let hover_bar = pointer
            .map(|(x, yy)| x >= BAR_X && x < BAR_X + COL_W && yy >= BAR_Y && yy < BAR_Y + BAR_H)
            .unwrap_or(false);
        let surf = paint_backlog(
            scene,
            font,
            vf.as_mut(),
            &rows,
            top,
            per_page,
            header.as_ref(),
            scroll.as_ref(),
            hover_bar,
            drag_top.is_some(),
        );
        win.present(&surf);
        match win.poll_input() {
            sdl2::Input::Quit | sdl2::Input::Back | sdl2::Input::Left | sdl2::Input::RightClick | sdl2::Input::Advance | sdl2::Input::Right => {
                return;
            }
            sdl2::Input::Up => top = top.saturating_sub(1),
            sdl2::Input::Down => top = (top + 1).min(max_top),
            sdl2::Input::Wheel { y } => {
                let step = (y.abs() as usize).max(1) * 3;
                if y > 0 {
                    top = top.saturating_sub(step);
                } else {
                    top = (top + step).min(max_top);
                }
            }
            sdl2::Input::Pointer { x, y, pressed } => {
                let (sx, sy) = win.surface_pos(x, y, W, 720);
                pointer = Some((sx, sy));
                let on_bar = sx >= BAR_X && sx < BAR_X + COL_W && sy >= BAR_Y && sy < BAR_Y + BAR_H;
                if pressed && on_bar {
                    let track_y = BAR_Y + COL_W;
                    let track_h = BAR_H - COL_W * 2;
                    if sy < track_y {
                        top = top.saturating_sub(1);
                    } else if sy >= track_y + track_h {
                        top = (top + 1).min(max_top);
                    } else if max_top > 0 {
                        let rel = (sy - track_y).clamp(0, track_h);
                        top = (rel as usize * max_top / track_h as usize).min(max_top);
                        drag_top = Some(sy);
                    }
                } else if pressed {
                    return;
                } else {
                    drag_top = None;
                }
            }
            sdl2::Input::Drag { x, y } => {
                let (sx, sy) = win.surface_pos(x, y, W, 720);
                pointer = Some((sx, sy));
                if drag_top.is_some() && max_top > 0 {
                    let track_y = BAR_Y + COL_W;
                    let track_h = BAR_H - COL_W * 2;
                    let rel = (sy - track_y).clamp(0, track_h);
                    top = (rel as usize * max_top / track_h as usize).min(max_top);
                }
            }
            _ => win.delay(16),
        }
    }
}
