//! The choice rows the script can put on screen.

use crate::*;

// ---------------------------------------------------------------------------
// branch choices
// ---------------------------------------------------------------------------

/// One choice row while a branch is presented, in output pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ChoiceRow {
    pub(crate) top: i32,
    pub(crate) height: i32,
}

/// A choice row is one line of the message window. The numbers come from the
/// original choice screen at 1280x720: a row is about a third of the dialogue
/// window tall, two rows sit 138px apart and straddle y=262 (so they land on
/// 193 and 331), and the block stays above the dialogue box.
pub(crate) const CHOICE_ROW_H: i32 = 72;

pub(crate) const CHOICE_PITCH: i32 = 138;

pub(crate) const CHOICE_CENTER_Y: i32 = 262;

pub(crate) const CHOICE_MIN_TOP: i32 = 24;

pub(crate) const CHOICE_MAX_BOTTOM: i32 = 476;

/// Vertical centres of every row, in logical 720p coordinates. Up to three
/// options keep the measured pitch; a rare four-option branch is tightened so
/// the block still fits between the top of the screen and the dialogue box.
pub(crate) fn choice_row_centers(count: usize) -> Vec<i32> {
    let n = count.max(1) as i32;
    let (pitch, center) = if n <= 3 {
        (CHOICE_PITCH, CHOICE_CENTER_Y)
    } else {
        let usable = CHOICE_MAX_BOTTOM - CHOICE_MIN_TOP - CHOICE_ROW_H;
        (
            (usable / (n - 1)).max(48),
            (CHOICE_MIN_TOP + CHOICE_MAX_BOTTOM) / 2,
        )
    };
    (0..n)
        .map(|i| center + (i * 2 - (n - 1)) * pitch / 2)
        .collect()
}

/// Same rows scaled to an output surface of surface_h height.
pub(crate) fn choice_row_layout(count: usize, surface_h: u32) -> Vec<ChoiceRow> {
    let sy = |v: i32| (v * surface_h as i32 + 360) / 720;
    let row_h = sy(CHOICE_ROW_H).max(20);
    // Scale the logical row centres before clamping: the block checks below
    // compare them against scaled bounds.
    let mut centers: Vec<i32> = choice_row_centers(count).into_iter().map(sy).collect();
    let block_top = |c: &[i32]| c.iter().min().copied().unwrap_or(0) - row_h / 2;
    let block_bottom = |c: &[i32]| c.iter().max().copied().unwrap_or(0) + row_h / 2;
    // Belt and braces for odd counts: never overlap the dialogue box or run
    // off the top of the screen.
    if block_bottom(&centers) > sy(CHOICE_MAX_BOTTOM) {
        let shift = sy(CHOICE_MAX_BOTTOM) - block_bottom(&centers);
        for c in &mut centers {
            *c += shift;
        }
    }
    if block_top(&centers) < sy(CHOICE_MIN_TOP) {
        let shift = sy(CHOICE_MIN_TOP) - block_top(&centers);
        for c in &mut centers {
            *c += shift;
        }
    }
    centers
        .into_iter()
        .map(|c| ChoiceRow {
            top: c - row_h / 2,
            height: row_h,
        })
        .collect()
}

/// Paint the choice rows: one message-window plate per option, squeezed to a
/// single line, with the option text centred inside. The hovered row gets a
/// soft inner highlight under the glyphs.
pub(crate) fn paint_choice_rows(
    frame: &mut render::Surface,
    vfont: Option<&mut render::text::VectorFont>,
    font: &render::Font,
    opts: &[String],
    rows: &[ChoiceRow],
    hover: Option<usize>,
    plate: Option<&codec::tga::TgaImage>,
    frame_img: Option<&codec::tga::TgaImage>,
    alpha: u8,
) {
    let w = frame.width;
    let x = render::WINDOW_X.max(0) as u32 * w / render::LAYOUT_W;
    let plate_w = plate
        .map(|p| p.width.max(1) * w / render::LAYOUT_W)
        .unwrap_or_else(|| w.saturating_sub(2 * x).max(1));
    for (i, row) in rows.iter().enumerate() {
        let height = row.height.max(1) as u32;
        if let Some(pl) = plate {
            frame.blit_region_scaled(
                pl, 0, 0, pl.width, pl.height, x as i32, row.top, plate_w, height, alpha,
            );
        } else {
            frame.fill_rect(x as i32, row.top, plate_w, height, [16, 62, 84, 220]);
        }
        if let Some(fr) = frame_img {
            frame.blit_region_scaled(
                fr, 0, 0, fr.width, fr.height, x as i32, row.top, plate_w, height, alpha,
            );
        }
        if hover == Some(i) {
            // Brighten the row by compositing its own plate again: the oval
            // keeps its feathered edges instead of getting a rectangle laid
            // over it.
            if let Some(pl) = plate {
                frame.blit_region_scaled(
                    pl, 0, 0, pl.width, pl.height, x as i32, row.top, plate_w, height, 96,
                );
            } else {
                frame.fill_rect(x as i32, row.top, plate_w, height, [120, 190, 220, 40]);
            }
            if let Some(fr) = frame_img {
                frame.blit_region_scaled(
                    fr, 0, 0, fr.width, fr.height, x as i32, row.top, plate_w, height, 96,
                );
            }
        }
    }
    match vfont {
        Some(vf) => {
            let size =
                render::layout_px(render::text::dialog_font_size() as i32, frame.height as i32)
                    .clamp(8, 160) as u32;
            if vf.size() != size {
                let _ = vf.set_size(size);
            }
            vf.set_edge_px(render::text::dialog_edge_px(size));
            let body_style = render::text::dialog_body_style();
            let hover_style = render::text::TextStyle {
                color: [255, 246, 226],
                ..body_style
            };
            for (i, (opt, row)) in opts.iter().zip(rows).enumerate() {
                let width = vf.measure(opt);
                let tx = ((w as i32 - width) / 2).max(x as i32 + 8);
                let baseline = vf.baseline_in_box(row.top, row.height);
                let style = if hover == Some(i) { &hover_style } else { &body_style };
                vf.draw_styled(frame, opt, tx, baseline, style);
            }
        }
        None => {
            // Japanese mode keeps the bitmap atlas; its cell size is fixed, so
            // centre by counting cells.
            let pitch = render::DIALOG_PITCH;
            for (opt, row) in opts.iter().zip(rows) {
                let mut adv = 0;
                for ch in opt.chars() {
                    adv += if (ch as u32) < 128 { pitch / 2 } else { pitch };
                }
                let tx = ((w as i32 - adv) / 2).max(x as i32 + 8);
                let ty = row.top + (row.height - font.cell() as i32) / 2;
                font.draw_pitch(frame, opt, tx, ty, pitch);
            }
        }
    }
}
