//! Movie playback.

use crate::*;

/// Play a movie (`mvNN` token) from the movie directory.
///
/// The movies are raw ASF/WMV files (the original `mv*.pak` is not an archive:
/// its first bytes are the ASF GUID), so the data tree stores them as plain
/// `*.wmv`. Scripts name them "mv01".."mv24" while the files ship as version
/// pairs (`mv010`, `mv011`), so we try the plain name first, then 0 and 1.
pub(crate) fn play_movie(
    win: &mut sdl2::Sdl,
    movie_dir: &std::path::Path,
    name: &str,
    mixer: Option<&audio::Mixer>,
    bg_dirs: &[&std::path::Path],
    ui: Option<&UiSound>,
) -> Option<()> {
    let cands = [
        name.to_ascii_lowercase(),
        format!("{}0", name.to_ascii_lowercase()),
        format!("{}1", name.to_ascii_lowercase()),
    ];
    let mut path = None;
    'outer: for cand in &cands {
        for ext in [".wmv", ".asf", ".WMV", ".ASF", ".pak", ".PAK"] {
            let p = movie_dir.join(format!("{}{}", cand, ext));
            if p.is_file() {
                path = Some(p);
                break 'outer;
            }
        }
    }
    let path = match path {
        Some(p) => p,
        None => {
            eprintln!("  movie {} not found in {} (skipped)", name, movie_dir.display());
            return None;
        }
    };
    let mut movie = video::Movie::open(path.to_str()?)?;
    // Movie audio is a normal mixer stream (WMA decoded by ffmpeg), scaled by
    // the BGM slider like the original's movie playback. Silent movies simply
    // have no slot here and stay video-only.
    let audio_slot = mixer.and_then(|m| movie.play_audio(m, audio::mixer::CAT_BGM));
    eprintln!("  playing movie {} ({}x{} @ {:.2}fps, audio={})", name,
        movie.width, movie.height, movie.fps,
        if audio_slot.is_some() { "on" } else if movie.has_audio { "unavailable" } else { "none" });
    let mut frame = vec![0u8; (movie.width * movie.height * 4) as usize];
    // Wall-clock pacing anchored on frame N's ideal presentation time instead of
    // a fixed `sleep(1/fps)` per frame: the old loop accumulated the cost of
    // decode + present into every frame and drifted behind the audio track.
    let fps = movie.fps.max(1.0);
    let mut t0 = std::time::Instant::now();
    let mut n = 0u64;
    'play: while movie.read_frame(&mut frame) {
        win.present_raw(&frame, movie.width, movie.height);
        n += 1;
        let deadline = t0 + std::time::Duration::from_secs_f64(n as f64 / fps);
        loop {
            match win.poll_input() {
                sdl2::Input::None | sdl2::Input::Pointer { pressed: false, .. } => {}
                sdl2::Input::Advance
                | sdl2::Input::Back
                | sdl2::Input::Right
                | sdl2::Input::RightClick
                | sdl2::Input::Choice(_)
                | sdl2::Input::Pointer { pressed: true, .. } => {
                    // The still the prompt is drawn on keeps the movie's own
                    // pixels. Downscaling it to 1280x720 first made the whole
                    // picture - and the prompt's text - drop to half
                    // resolution for as long as the dialog was up.
                    let snapshot = render::Surface {
                        width: movie.width,
                        height: movie.height,
                        pixels: frame.clone(),
                    };
                    let message = if std::env::var_os("WA2_LANG").as_deref() == Some(std::ffi::OsStr::new("jp")) {
                        "ムービーをスキップしますか？"
                    } else {
                        "要跳过影片吗？"
                    };
                    if ask_yes_no(win, &snapshot, bg_dirs, message, mixer, ui).unwrap_or(true) {
                        break 'play;
                    }
                }
                sdl2::Input::Quit => break 'play,
                _ => {}
            }
            let now = std::time::Instant::now();
            if now >= deadline { break; }
            let rest = (deadline - now).as_millis() as u32;
            // More than half a second behind means the decoder cannot keep up;
            // re-anchor instead of fast-forwarding through the rest of the movie.
            if rest > 500 { t0 = now - std::time::Duration::from_secs_f64(n as f64 / fps); break; }
            win.delay(rest.clamp(1, 8));
        }
    }
    if let (Some(m), Some(slot)) = (mixer, audio_slot) { m.stop(slot); }
    Some(())
}

/// Block until a non-None input event arrives.
pub(crate) fn wait_input(win: &mut sdl2::Sdl) -> sdl2::Input {
    loop {
        let ev = win.poll_input();
        match ev {
            sdl2::Input::None | sdl2::Input::Pointer { pressed: false, .. } => {}
            other => return other,
        }
        win.delay(16);
    }
}
