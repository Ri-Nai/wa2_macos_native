//! Transitions: fades, flashes and the rewind cruise.

use crate::*;

/// What the player did while fast-rewind was stepping.
pub(crate) enum Cruise {
    Keep,
    Stop,
    Quit,
}

pub(crate) fn present_rewind_frame(
    win: &mut sdl2::Sdl,
    cur: &render::Surface,
    bar: &cmdbar::CmdBar,
    hover: Option<cmdbar::Cmd>,
    active: cmdbar::Active,
    idle: u8,
) {
    let mut frame = clone_surf(cur);
    let (fw, fh) = (frame.width as i32, frame.height as i32);
    bar.draw(&mut frame, hover, active, idle, fw, fh);
    win.present(&frame);
}

/// Click, Enter, Page Up, or the rewind button stops the cruise. Moving the
/// pointer only updates the hover, so the buttons stay usable. The stop is
/// taken on release: the press and the release are one click, and leaving
/// the release for the line-wait would advance the script.
pub(crate) fn rewind_cruise_stop(win: &mut sdl2::Sdl, bar: &cmdbar::CmdBar, hover: &mut Option<cmdbar::Cmd>) -> Cruise {
    match win.poll_input() {
        sdl2::Input::Quit => Cruise::Quit,
        sdl2::Input::Advance | sdl2::Input::Right | sdl2::Input::Rewind | sdl2::Input::RightClick => Cruise::Stop,
        sdl2::Input::Wheel { y } if y < 0 => Cruise::Stop,
        sdl2::Input::Release { x, y } => {
            let (sx, sy) = win.surface_pos(x, y, 1280, 720);
            *hover = bar.hit(sx, sy);
            Cruise::Stop
        }
        sdl2::Input::Pointer { x, y, .. } => {
            let (sx, sy) = win.surface_pos(x, y, 1280, 720);
            *hover = bar.hit(sx, sy);
            Cruise::Keep
        }
        sdl2::Input::Drag { x, y } => {
            let (sx, sy) = win.surface_pos(x, y, 1280, 720);
            *hover = bar.hit(sx, sy);
            Cruise::Keep
        }
        _ => Cruise::Keep,
    }
}

pub(crate) fn fade_len(settings: &menu::Settings, skipping: bool, frames: u32) -> u32 {
    if skipping || settings.effect == 0 {
        0
    } else {
        frames.min(240)
    }
}

/// Crossfade `from` into `to`. A click or a key lands on the destination.
/// Returns true when the player closed the window.
/// Snap the screen to `rgb` and fade back to `base` over `frames` at 60 fps.
/// A click or a key lands on the picture. Returns true when the window closed.
pub(crate) fn present_flash(win: &mut sdl2::Sdl, base: &render::Surface, frames: u32, r: u8, g: u8, b: u8) -> bool {
    let mut buf = vec![0u8; base.pixels.len()];
    for remain in (0..=frames).rev() {
        render::apply_flash(&mut buf, &base.pixels, r, g, b, remain, frames);
        win.present_raw(&buf, base.width, base.height);
        if remain == 0 {
            break;
        }
        match win.poll_input() {
            sdl2::Input::Quit => return true,
            sdl2::Input::None | sdl2::Input::Pointer { pressed: false, .. } | sdl2::Input::Drag { .. } => {}
            _ => {
                win.present(base);
                return false;
            }
        }
        win.delay(16);
    }
    false
}

pub(crate) fn present_fade(win: &mut sdl2::Sdl, from: &render::Surface, to: &render::Surface, frames: u32) -> bool {
    if frames == 0 || from.pixels.len() != to.pixels.len() || from.width != to.width {
        win.present(to);
        return false;
    }
    let mut buf = vec![0u8; to.pixels.len()];
    for step in 1..=frames {
        render::lerp_pixels(&mut buf, &from.pixels, &to.pixels, step, frames);
        win.present_raw(&buf, to.width, to.height);
        match win.poll_input() {
            sdl2::Input::Quit => return true,
            sdl2::Input::None
            | sdl2::Input::Pointer { pressed: false, .. }
            | sdl2::Input::Drag { .. } => {}
            _ => {
                win.present(to);
                return false;
            }
        }
        win.delay(16);
    }
    win.present(to);
    false
}
