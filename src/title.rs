//! The title screen.

use crate::*;

/// What the title screen resolved to.
pub(crate) enum TitleAction {
    Start,
    /// A scene picked on the timeline.
    Jump(String),
    /// Resume from a save slot picked on the load screen.
    Load(save::SaveState),
    Quit,
}

/// One falling flake. Layout numbers are 1280×720 pixels; the draw scales them.
pub(crate) struct TitleFlake {
    pub(crate) x: f32,
    pub(crate) y: f32,
    /// Downward speed, layout pixels per second.
    pub(crate) speed: f32,
    pub(crate) phase: f32,
    /// Which of the eight sizes. 0 is the largest.
    pub(crate) kind: usize,
    pub(crate) alpha: u8,
}

/// Title snowfall. The original loads `weather.tga` and draws the white flake
/// cell over the street. Kind 248..255 is a size of 36..8 px. The exe's own
/// alpha (about 96..40) vanishes on this bright plate, so the flakes are
/// drawn brighter and there are more of them.
pub(crate) struct TitleSnow {
    pub(crate) ww: i32,
    pub(crate) wh: i32,
    pub(crate) flakes: Vec<TitleFlake>,
    pub(crate) sprites: Vec<render::Surface>,
    pub(crate) rng: u32,
}

pub(crate) fn snow_rng(state: &mut u32) -> u32 {
    *state = state.wrapping_mul(1664525).wrapping_add(1013904223);
    *state
}

impl TitleSnow {
    const COUNT: usize = 400;
    const KINDS: usize = 8;

    pub(crate) fn new() -> TitleSnow {
        let mut rng = 0x00a2_5e1du32;
        let mut flakes = Vec::with_capacity(Self::COUNT);
        for i in 0..Self::COUNT {
            flakes.push(Self::make(&mut rng, i % Self::KINDS, true));
        }
        TitleSnow { ww: 0, wh: 0, flakes, sprites: Vec::new(), rng }
    }

    /// `kind` 0..8 maps to the exe's flake id 248..255.
    pub(crate) fn make(rng: &mut u32, kind: usize, anywhere: bool) -> TitleFlake {
        let id = 248 + kind;
        let span = 100 * (256 - id);
        let r = snow_rng(rng) % span as u32;
        let v = (r as f32 / 100.0 + 1.0) * 0.5;
        let x_span = (0x240 - id) * 4;
        let y_span = (0x1b4 - id) * 4;
        let inset = (256 - id) * 2;
        let x = (snow_rng(rng) % x_span as u32) as f32 - inset as f32;
        let y = if anywhere {
            (snow_rng(rng) % y_span as u32) as f32 - inset as f32
        } else {
            -((snow_rng(rng) % 48) as f32) - 8.0
        };
        TitleFlake {
            x,
            y,
            // vy * 5/40 per frame at 60fps. Larger ids are the small, slower flakes.
            speed: v * 7.5,
            phase: (snow_rng(rng) % 256) as f32 * std::f32::consts::TAU / 256.0,
            kind,
            // Exe's 96..40, lifted so a white flake still reads on the street.
            alpha: (((256 - id) * 8 + 32) * 2).min(220) as u8,
        }
    }

    pub(crate) fn ensure(&mut self, ww: i32, wh: i32, sheet: &codec::tga::TgaImage) {
        if self.ww == ww && self.wh == wh && self.sprites.len() == Self::KINDS {
            return;
        }
        self.sprites.clear();
        for kind in 0..Self::KINDS {
            let id = 248 + kind;
            let size = ((257 - id) as i32 * wh / 720).max(2) as u32;
            // weather.tga cell (0, 32): the white flake the title's source rect selects.
            self.sprites.push(scale_sprite(sheet, 0, 32, 32, 32, size, size));
        }
        self.ww = ww;
        self.wh = wh;
    }

    pub(crate) fn step(&mut self, dt: f32) {
        for flake in &mut self.flakes {
            flake.y += flake.speed * dt;
            flake.phase += dt * 1.8;
            flake.x += flake.phase.sin() * 18.0 * dt;
            if flake.y > 760.0 || flake.x < -40.0 || flake.x > 1320.0 {
                *flake = Self::make(&mut self.rng, flake.kind, false);
            }
        }
    }

    pub(crate) fn paint(&self, frame: &mut render::Surface, wh: i32) {
        for flake in &self.flakes {
            let Some(sp) = self.sprites.get(flake.kind) else { continue };
            let x = (flake.x * wh as f32 / 720.0).round() as i32 - sp.width as i32 / 2;
            let y = (flake.y * wh as f32 / 720.0).round() as i32 - sp.height as i32 / 2;
            frame.blit_surface_alpha(sp, x, y, flake.alpha);
        }
    }
}

/// The title page music. WA2.exe arms BGM(31, 0, 1, 255, 0, 0) once when the
/// title screen comes up (call at 0x426701) and the special page re-arms the
/// same track on entering (0x427611), so one looping stream covers both pages
/// and the title-origin screens in between.
pub(crate) const TITLE_BGM_ID: i32 = 31;

pub(crate) fn title_bgm_path(bg_dirs: &[&std::path::Path]) -> Option<std::path::PathBuf> {
    let dir = bg_dirs.iter().find_map(|d| d.parent())?.join("bgm");
    // Both arrangements of track 31 are the same 38 s cue, so the title keeps
    // the one it has always used; `bgm_file` would pick the other `_B` mix and
    // change a piece that is not part of the looping problem.
    let stem = format!("BGM_{:03}", TITLE_BGM_ID);
    [format!("{}.OGG", stem), format!("{}_A.OGG", stem)]
        .into_iter()
        .map(|name| dir.join(name))
        .find(|p| p.is_file())
}

/// Play the title/special track on the title screen's own mixer. That mixer
/// lives exactly as long as the title loop, so leaving the title (starting or
/// loading the game, quitting) drops every stream with it.
pub(crate) fn start_title_bgm(mixer: Option<&audio::Mixer>, bg_dirs: &[&std::path::Path]) -> Option<usize> {
    let (mixer, path) = (mixer?, title_bgm_path(bg_dirs)?);
    let slot = mixer.play_cat(&path.to_string_lossy(), 1.0, true, audio::mixer::CAT_BGM);
    if slot == usize::MAX {
        None
    } else {
        eprintln!("BGM: title {} -> {}", TITLE_BGM_ID, path.display());
        Some(slot)
    }
}

/// Original title screen. The logo, highlight bar and copyright stay the
/// game's art. The five item labels are the system font, white at rest and
/// gold when selected, so a player font can replace them later.
///
///   T0000  1280x856  street plate. A 720-tall window pans down it.
///   weather.tga      snowfall, drawn over the street and the logo
///   T0011  752x448   logo (transparent; the backdrop is its edge colour #011D59)
///   T0101  320x80    highlight bar: two 320x40 frames (dim / bright)
///   T0020  264x24    copyright line
///
/// The original title adds 1.2px per frame to the plate's source Y and clamps
/// at 136 (856 − 720), so the street settles after about two seconds. The bar
/// slides between items and flashes its bright frame while it moves. Arrow
/// keys move, Space/Enter confirms, Esc quits; items whose subsystems do not
/// exist yet answer with a notice.
///
/// The original starts BGM track 31 when this page comes up (WA2.exe arms it
/// once at 0x426701) and the special page re-arms the same track, so a single
/// looping stream carries the title and the special page. It is parked while
/// the music player owns the BGM category. Leaving the title drops the title
/// mixer, and with it the stream.
///
/// `WA2_TITLE_AUTO=<n>` pre-selects item `n` and confirms it (verification runs).
/// Title screen. `mixer` is the Runtime's: opening a second device inside one
/// process fails (SDL allows one), which used to leave the title - and every
/// screen it opens - silent.
pub(crate) fn run_title(
    win: &mut sdl2::Sdl,
    font: &render::Font,
    bg_dirs: &[&std::path::Path],
    script_dir: &std::path::Path,
    cn: bool,
    mixer: Option<&audio::Mixer>,
) -> TitleAction {
    const W: u32 = 1280;
    const H: u32 = 720;
    const ITEM_W: i32 = 320;
    const ITEM_H: i32 = 40;
    // The highlight oval is 40px. The labels are larger than the original
    // bitmap row, so the rows sit further apart than the oval height.
    const PITCH: i32 = 52;
    const N: usize = 5;
    const X0: i32 = (W as i32 - ITEM_W) / 2;
    const Y0: i32 = 412;
    const BACKDROP: [u8; 4] = [1, 29, 89, 255]; // T0011's edge colour

    let items: [&str; 5] = if cn {
        ["开始游戏", "继续游戏", "系统设置", "特别模式", "结束游戏"]
    } else {
        ["はじめから", "つづきから", "オプション", "スペシャル", "ゲーム終了"]
    };
    let notice_text = if cn { "此功能尚未实现" } else { "ただいま準備中です" };
    let mut label_px = 0u32;
    // Font generation the label/notice bitmaps were rasterized with. A UI font
    // change on the option screen must reach this page at once, not after a
    // restart or a window resize (F-03).
    let mut label_gen = 0u64;
    let mut labels = Vec::new();
    // The labels pre-rendered with their outline, in both menu colours.
    // Rebuilding these is the expensive part, so they only follow the font and
    // the window size - not the frame.
    let mut label_white: Vec<Option<render::Surface>> = Vec::new();
    let mut label_gold: Vec<Option<render::Surface>> = Vec::new();
    let mut label_stroke = 0i32;
    let mut note_px = 0u32;
    let mut note_gen = 0u64;
    let mut note_img: Option<(u32, u32, Vec<u8>)> = None;
    let mut fitted: Option<FittedMenu> = None;

    let street = load_background(bg_dirs, "T0000.tga");
    let weather = load_background(bg_dirs, "weather.tga");
    let mut snow = TitleSnow::new();
    let logo = load_background(bg_dirs, "T0011.tga");
    let copyright = load_background(bg_dirs, "T0020.tga");
    let bar = load_background(bg_dirs, "T0101.tga");
    // Source Y of the 720-tall window into T0000. The exe adds 1.2 per frame
    // and stops at image_height − 720, which is 136 for the shipped plate.
    let scroll_max = street
        .as_ref()
        .map(|img| (img.height as i32 - H as i32).max(0) as f32)
        .unwrap_or(0.0);
    let mut scroll = 0.0f32;
    let mut scroll_tick = std::time::Instant::now();
    let mut street_scaled: Option<(i32, i32, render::Surface)> = None;

    if std::env::var_os("WA2_DEBUG_INPUT").is_some() {
        let (ww, wh) = win.window_size();
        eprintln!("title: window_size = {}x{}", ww, wh);
    }
    // 「画面模式」 applies as soon as the option screen is left.
    let startup = menu::Settings::load();
    win.set_fullscreen(startup.win_disp == 1);
    set_render_art_scale(win);
    if let Some(mx) = mixer {
        menu::apply(&startup, Some(mx));
    }
    let mut bgm_slot = start_title_bgm(mixer, bg_dirs);
    let ui = UiSound::resolve(bg_dirs);
    // Warm the first two CG pages while the title screen is visible. The
    // worker stores decoded previews in cg's process-local memory cache, so
    // entering CG can reuse completed work without a disk cache or a blocking
    // decode on the UI thread.
    let cg_preview_scale = if win.is_fullscreen() || win.is_maximized() { 4 } else { 2 };
    let cg_preview_dirs = cg::preview_dirs(bg_dirs, cg_preview_scale);
    let cg_stems = cg::cg_stems(&cg_preview_dirs);
    let (cg_ww, cg_wh) = win.window_size();
    let (cg_pw, cg_ph) = cg::preview_box(cg_ww.max(1) as u32, cg_wh.max(1) as u32);
    let cg_warmup_stems: Vec<String> = cg_stems
        .into_iter()
        .take(cg::PER_PAGE * 2)
        .collect();
    let cg_warmup = cg::start_page_loader(
        &cg_preview_dirs,
        &cg_warmup_stems,
        cg_pw,
        cg_ph,
    );
    if std::env::var_os("WA2_DEBUG_CG").is_some() && cg_warmup.is_some() {
        eprintln!(
            "cg: startup warmup queued {} previews from {}x source",
            cg_warmup_stems.len(),
            cg_preview_scale,
        );
    }

    let item_y = |i: usize| (Y0 + i as i32 * PITCH) as f32;
    // What the player picked. Collected instead of returned so the title BGM
    // can be stopped on the way out of every path.
    let picked: TitleAction;
    let mut sel = 0usize;
    let mut pointer_hover = false;
    let mut notice: Option<&str> = None;
    let mut bar_y = item_y(0);
    let mut flash_until = std::time::Instant::now();
    let mut auto_pick: Option<usize> = std::env::var_os("WA2_TITLE_AUTO")
        .and_then(|v| v.to_string_lossy().parse::<usize>().ok())
        .map(|p| p.min(N - 1));
    if let Some(p) = auto_pick {
        sel = p;
        bar_y = item_y(p);
    }

    loop {
        let t_pace = std::time::Instant::now();
        // ---- input (non-blocking so the bar can animate) --------------------
        let mut confirm = false;
        if auto_pick.is_some() {
            auto_pick = None;
            win.delay(600);
            confirm = true;
        } else {
            match win.poll_input() {
                sdl2::Input::Up => {
                    pointer_hover = false;
                    sel = (sel + N - 1) % N;
                    flash_until = std::time::Instant::now() + std::time::Duration::from_millis(160);
                    play_ui(mixer, Some(&ui), UiSe::Move);
                }
                sdl2::Input::Down => {
                    pointer_hover = false;
                    sel = (sel + 1) % N;
                    flash_until = std::time::Instant::now() + std::time::Duration::from_millis(160);
                    play_ui(mixer, Some(&ui), UiSe::Move);
                }
                sdl2::Input::Advance | sdl2::Input::Right => confirm = true,
                sdl2::Input::Choice(k) => {
                    pointer_hover = false;
                    sel = ((k as usize).saturating_sub(1)).min(N - 1);
                    flash_until = std::time::Instant::now() + std::time::Duration::from_millis(160);
                    confirm = true;
                }
                sdl2::Input::Quit => {
                    picked = TitleAction::Quit;
                    break;
                }
                // Pointer: hovering highlights the item under the cursor, a
                // click on an item confirms it. The original is mouse-driven,
                // so this is the primary way to pick a menu entry.
                sdl2::Input::Pointer { x, y, pressed } => {
                    let (sx, sy) = win.surface_pos(x, y, W, H);
                    if let Some(i) = (0..N).find(|i| {
                        let iy = Y0 + *i as i32 * PITCH;
                        sx >= X0 && sx < X0 + ITEM_W && sy >= iy && sy < iy + ITEM_H
                    }) {
                        pointer_hover = true;
                        if i != sel {
                            sel = i;
                            flash_until =
                                std::time::Instant::now() + std::time::Duration::from_millis(160);
                            play_ui(mixer, Some(&ui), UiSe::Move);
                        }
                        if pressed {
                            confirm = true;
                        }
                    }
                }
                _ => {}
            }
        }

        // ---- animate the highlight bar and the street ----------------------
        let (ww, wh) = win.window_size();
        let ww = ww.max(1);
        let wh = wh.max(1);
        slide_bar(&mut bar_y, item_y(sel), wh);
        let flashing = std::time::Instant::now() < flash_until;
        let now = std::time::Instant::now();
        let dt = now.saturating_duration_since(scroll_tick).as_secs_f32().min(0.05);
        scroll_tick = now;
        if scroll < scroll_max {
            // 1.2 layout pixels per frame at 60fps.
            scroll = (scroll + dt * 72.0).min(scroll_max);
        }
        if weather.is_some() {
            snow.step(dt);
        }

        // ---- compose --------------------------------------------------------
        // T0100 draws each label about 29px tall inside the 40px row.
        let ipx = layout_font_px(32, wh);
        let generation = render::text::role_generation();
        let stroke = ((2 * wh) / 720).max(1);
        if ipx != label_px || generation != label_gen || stroke != label_stroke {
            label_px = ipx;
            label_gen = generation;
            label_stroke = stroke;
            labels = menu_labels(&items, ipx);
            let render_all = |rgb: [u8; 3]| -> Vec<Option<render::Surface>> {
                labels
                    .iter()
                    .map(|l| {
                        l.as_ref()
                            .map(|(w, h, rgba)| label_surface(rgba, *w, *h, rgb, stroke))
                    })
                    .collect()
            };
            label_white = render_all([255, 255, 255]);
            label_gold = render_all([254, 232, 136]);
        }
        if let Some(n) = notice {
            if cn {
                let px = layout_font_px(26, wh);
                if px != note_px || generation != note_gen {
                    note_px = px;
                    note_gen = generation;
                    note_img =
                        render::text::with_ui_font(px, |f| f.line_rgba(n, px)).flatten();
                }
            }
        } else {
            note_px = 0;
        }
        let baked_notice = notice.is_some() && (!cn || note_img.is_none());

        ensure_fitted(
            &mut fitted,
            ww,
            wh,
            if baked_notice { 1 } else { 0 },
            || {
                // The street is composited under this plate. With no street
                // the plate itself is the navy field the logo was authored on.
                let plate_bg = if street.is_some() { [0, 0, 0, 0] } else { BACKDROP };
                let mut s = render::Surface::new(W, H, plate_bg);
                if let Some(ref lg) = logo {
                    s.blit(lg, (W as i32 - lg.width as i32) / 2, 24);
                }
                if baked_notice {
                    if let Some(n) = notice {
                        let cell = font.cell() as i32;
                        let wpx = n.chars().count() as i32 * cell;
                        let nx = W as i32 / 2 - wpx / 2;
                        s.fill_rect(
                            nx - 16,
                            372,
                            (wpx + 32) as u32,
                            (cell + 12) as u32,
                            [1, 29, 89, 235],
                        );
                        font.draw(&mut s, n, nx, 378);
                    }
                }
                if let Some(ref cp) = copyright {
                    s.blit(cp, W as i32 - cp.width as i32 - 24, H as i32 - cp.height as i32 - 14);
                }
                s
            },
            bar.as_ref(),
            ITEM_W as u32,
            ITEM_H as u32,
        );
        let fitted = fitted.as_ref().unwrap();
        let mut frame = if let Some(ref img) = street {
            let rebuild = match street_scaled.as_ref() {
                Some((sw, sh, _)) => *sw != ww || *sh != wh,
                None => true,
            };
            if rebuild {
                let dh = ((img.height as i64 * wh as i64) / H as i64).max(wh as i64) as u32;
                street_scaled = Some((ww, wh, surface_from_tga(img).scaled_to(ww as u32, dh)));
            }
            let scaled = &street_scaled.as_ref().unwrap().2;
            let max_y = (scaled.height as i32 - wh).max(0);
            let sy = ((scroll * wh as f32 / H as f32).round() as i32).clamp(0, max_y);
            let mut frame = render::Surface::new(ww as u32, wh as u32, BACKDROP);
            frame.copy_rows_from(scaled, sy);
            frame.blit_surface(&fitted.plate, 0, 0);
            frame
        } else {
            clone_surf(&fitted.plate)
        };
        // Bar under the labels: the glyphs are drawn on top of the oval.
        // The original selection bar remains for keyboard navigation. Mouse
        // hover keeps the label highlight but omits the heavy background bar,
        // which avoids a laggy full-screen hover plate over the animated snow.
        let hide_hover_bar = pointer_hover && (win.is_fullscreen() || win.is_maximized());
        if !hide_hover_bar {
            paint_bar(&mut frame, fitted, if flashing { 1 } else { 0 }, bar_y, X0);
        }
        draw_menu_labels_pre(
            &mut frame,
            &label_white,
            &label_gold,
            sel,
            X0,
            Y0,
            ITEM_W,
            ITEM_H,
            PITCH,
            ww,
            wh,
            label_stroke,
        );
        if notice.is_some() {
            if let Some((rw, rh, rgba)) = note_img.as_ref() {
                let nx = ww / 2 - *rw as i32 / 2;
                let ny = 370 * wh / 720;
                let pad_x = (16 * ww / 1280).max(8);
                let pad_y = (4 * wh / 720).max(2);
                frame.fill_rect(
                    nx - pad_x,
                    ny - pad_y,
                    *rw + pad_x as u32 * 2,
                    *rh + pad_y as u32 * 2,
                    [1, 29, 89, 235],
                );
                frame.blit_rgba(rgba, *rw, *rh, nx, ny);
            }
        }
        if let Some(ref sheet) = weather {
            snow.ensure(ww, wh, sheet);
            snow.paint(&mut frame, wh);
        }
        win.present(&frame);

        // ---- act on the confirmation ---------------------------------------
        if confirm {
            if notice.is_some() {
                notice = None;
                play_ui(mixer, Some(&ui), UiSe::Cancel);
                continue;
            }
            play_ui(mixer, Some(&ui), UiSe::Confirm);
            match sel {
                0 => {
                    picked = TitleAction::Start;
                    break;
                }
                // 「继续游戏」: the load screen. A picked slot resumes the game.
                1 => {
                    if let Some(snap) = saveload::run(
                        win,
                        bg_dirs,
                        saveload::Mode::Load,
                        None,
                        // Dim the title screen behind the slots instead of a
                        // flat black backdrop, matching how the in-game load
                        // screen dims the scene it was opened from.
                        Some(&frame),
                        mixer,
                        Some(&ui),
                    ) {
                        picked = TitleAction::Load(snap);
                        break;
                    }
                    notice = None;
                }
                2 => {
                    // 「系统设置」: the game's own option screen.
                    let mut st = menu::Settings::load();
                    menu::apply(&st, None);
                    menu::run(win, bg_dirs, &mut st, mixer, Some(&ui), Some(&frame));
                    win.set_fullscreen(st.win_disp == 1);
                    set_render_art_scale(win);
                    notice = None;
                }
                3 => {
                    let special_pick = run_special(
                        win,
                        bg_dirs,
                        cn,
                        mixer,
                        Some(&ui),
                    );
                    record_special_visit(bg_dirs, &special_pick);
                    match special_pick {
                        SpecialPick::Novel => {
                            if let Some(id) = browse_novels(win, bg_dirs, cn, mixer, Some(&ui)) {
                                picked = TitleAction::Jump(id);
                                break;
                            }
                            notice = None;
                        }
                        SpecialPick::Replay => {
                            if let Some(id) = timeline::run(
                                win,
                                font,
                                bg_dirs,
                                script_dir,
                                cn,
                                mixer,
                                Some(&ui),
                                None,
                            ) {
                                picked = TitleAction::Jump(id);
                                break;
                            }
                            notice = None;
                        }
                        SpecialPick::Cg => {
                            if cg::cg_stems(bg_dirs).is_empty() {
                                notice = Some(notice_text);
                            } else {
                                browse_cg(win, bg_dirs, cn, mixer, Some(&ui));
                                notice = None;
                            }
                        }
                        SpecialPick::Music => {
                            // The music player drives the BGM category itself;
                            // park the title/special track while it runs.
                            if let (Some(mx), Some(slot)) = (mixer.as_ref(), bgm_slot.take()) {
                                mx.stop(slot);
                            }
                            let bgm_dir = script_dir.parent().unwrap_or(std::path::Path::new(".")).join("bgm");
                            run_music(win, bg_dirs, &bgm_dir, cn, mixer, Some(&ui));
                            bgm_slot = start_title_bgm(mixer, bg_dirs);
                            notice = None;
                        }
                        SpecialPick::Voice => {
                            browse_voice_messages(win, bg_dirs, cn, mixer, Some(&ui));
                            notice = None;
                        }
                        SpecialPick::Back => notice = None,
                        SpecialPick::Notice => notice = Some(notice_text),
                    }
                }
                4 => {
                    picked = TitleAction::Quit;
                    break;
                }
                _ => notice = Some(notice_text),
            }
        }
        // Pace to a 60 Hz target instead of sleeping a flat 16 ms. The compose
        // work is not constant - the street scroll and the snow keep changing
        // how much has to be redrawn - and stacking a fixed delay on top of it
        // made the frame rate depend on the frame's own cost.
        let budget = std::time::Duration::from_micros(16_667);
        if let Some(rest) = budget.checked_sub(t_pace.elapsed()) {
            win.delay(rest.as_millis().min(16) as u32);
        }
    }
    if let (Some(mx), Some(slot)) = (mixer, bgm_slot.take()) {
        mx.stop(slot);
    }
    picked
}
