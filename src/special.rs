//! The special mode: the extra scenes, the music room and their visit counters.

use crate::*;

/// 「特别模式」. Labels are the system font. The highlight bar and the logo
/// stay the original art. 「时间线」 replaces the atlas row 场景回放.
pub(crate) enum SpecialPick {
    Novel,
    Replay,
    Cg,
    Music,
    Voice,
    /// An entry the original lists but this build does not open yet.
    Notice,
    Back,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct SpecialStats {
    pub(crate) entered: [u32; 5],
    pub(crate) visited: u8,
}

impl SpecialStats {
    pub(crate) fn index(pick: &SpecialPick) -> Option<usize> {
        match pick {
            SpecialPick::Novel => Some(0),
            SpecialPick::Cg => Some(1),
            SpecialPick::Replay => Some(2),
            SpecialPick::Music => Some(3),
            SpecialPick::Voice => Some(4),
            SpecialPick::Notice | SpecialPick::Back => None,
        }
    }

    pub(crate) fn label(&self, cn: bool) -> String {
        let names = if cn { ["小说", "CG", "时间线", "音乐", "语音"] }
            else { ["小説", "CG", "年表", "音楽", "ボイス"] };
        let visited = self.visited.count_ones();
        format!(
            "{} {}/5　{} {}　{} {}　{} {}　{} {}　{} {}",
            if cn { "已浏览" } else { "閲覧" },
            visited,
            names[0], self.entered[0],
            names[1], self.entered[1],
            names[2], self.entered[2],
            names[3], self.entered[3],
            names[4], self.entered[4],
        )
    }
}

pub(crate) fn special_stats_path(dirs: &[&std::path::Path]) -> Option<std::path::PathBuf> {
    dirs.iter().find_map(|d| d.parent().map(|root| root.join("save").join("special_stats.tsv")))
}

pub(crate) fn load_special_stats(dirs: &[&std::path::Path]) -> SpecialStats {
    let Some(path) = special_stats_path(dirs) else { return SpecialStats::default() };
    let Ok(text) = std::fs::read_to_string(path) else { return SpecialStats::default() };
    let mut stats = SpecialStats::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else { continue };
        let value = value.trim();
        match key {
            "visited" => stats.visited = value.parse::<u8>().unwrap_or(0) & 0x1f,
            k if k.starts_with("entered_") => {
                if let Ok(i) = k[8..].parse::<usize>() {
                    if let Some(slot) = stats.entered.get_mut(i) {
                        *slot = value.parse().unwrap_or(0);
                    }
                }
            }
            _ => {}
        }
    }
    stats
}

pub(crate) fn save_special_stats(dirs: &[&std::path::Path], stats: SpecialStats) {
    let Some(path) = special_stats_path(dirs) else { return };
    let Some(parent) = path.parent() else { return };
    if std::fs::create_dir_all(parent).is_err() { return; }
    let mut text = format!("version=1\nvisited={}\n", stats.visited);
    for (i, count) in stats.entered.iter().enumerate() {
        text.push_str(&format!("entered_{i}={count}\n"));
    }
    let tmp = path.with_extension("tsv.tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(tmp, path);
    }
}

pub(crate) fn record_special_visit(dirs: &[&std::path::Path], pick: &SpecialPick) {
    let Some(i) = SpecialStats::index(pick) else { return };
    let mut stats = load_special_stats(dirs);
    stats.visited |= 1 << i;
    stats.entered[i] = stats.entered[i].saturating_add(1);
    save_special_stats(dirs, stats);
}

pub(crate) fn run_special(
    win: &mut sdl2::Sdl,
    bg_dirs: &[&std::path::Path],
    cn: bool,
    mixer: Option<&audio::Mixer>,
    ui: Option<&UiSound>,
) -> SpecialPick {
    const W: u32 = 1280;
    const H: u32 = 720;
    const ITEM_W: i32 = 320;
    const ITEM_H: i32 = 40;
    const PITCH: i32 = 40;
    // Same horizontal placement as the title items. The six rows and the
    // first item's y come from WA2.exe: the special page lists T0100 rows
    // 9,10,11,12,13,8 and lays them out from y=380 at a pitch of 40.
    const X0: i32 = (W as i32 - ITEM_W) / 2;
    const Y0: i32 = 380;
    const ROWS: [i32; 6] = [9, 10, 11, 12, 13, 8];
    const N: usize = ROWS.len();
    const BACKDROP: [u8; 4] = [1, 29, 89, 255];

    let bar = load_background(bg_dirs, "T0101.tga");
    let logo = load_background(bg_dirs, "T0011.tga");
    // The original special page draws the title street plate (T0000) behind
    // the menu with its source scroll pinned to the bottom clamp (136 of the
    // 856px tall plate), plus the same falling snow as the title. WA2.exe sets
    // [0xa72f00] to the 136.0 constant and draws image 1 full-screen before
    // the menu rows (see the special-page draw function at 0x427df0).
    let street = load_background(bg_dirs, "T0000.tga");
    let weather = load_background(bg_dirs, "weather.tga");
    let mut snow = TitleSnow::new();
    let mut street_scaled: Option<(i32, i32, render::Surface)> = None;
    let mut snow_tick = std::time::Instant::now();
    let items: [&str; 6] = if cn {
        ["电子小说", "CG模式", "时间线", "音乐模式", "声优访谈", "返回"]
    } else {
        ["デジタルノベル", "CGモード", "タイムライン", "サウンドモード", "ボイス", "戻る"]
    };
    let stats = load_special_stats(bg_dirs);
    let head_text = if cn { "特别内容" } else { "スペシャル" };
    let mut label_px = 0u32;
    // Font generation the menu/heading bitmaps were rasterized with; the page
    // stays open across a settings visit, so a UI font change has to reach it
    // on the next frame (F-03).
    let mut label_gen = 0u64;
    let mut labels = Vec::new();
    let mut head_px = 0u32;
    let mut head_gen = 0u64;
    let mut head: Option<(u32, u32, Vec<u8>)> = None;
    let mut fitted: Option<FittedMenu> = None;

    let pick = |sel: usize| match ROWS[sel] {
        9 => SpecialPick::Novel,
        10 => SpecialPick::Cg,
        11 => SpecialPick::Replay,
        12 => SpecialPick::Music,
        13 => SpecialPick::Voice,
        8 => SpecialPick::Back,
        _ => SpecialPick::Notice,
    };
    let mut sel = 0usize;
    let mut bar_y = Y0 as f32;
    // WA2_SPECIAL_AUTO=<n> pre-selects item n and confirms it (verification
    // runs: no input synthesis needed to reach the timeline headlessly).
    let mut auto_pick: Option<usize> = std::env::var_os("WA2_SPECIAL_AUTO")
        .and_then(|v| v.to_string_lossy().parse::<usize>().ok())
        .map(|p| p.min(N - 1));
    if let Some(p) = auto_pick {
        sel = p;
        bar_y = (Y0 + p as i32 * PITCH) as f32;
    }
    loop {
        if auto_pick.take().is_some() {
            win.delay(400);
            return pick(sel);
        }
        match win.poll_input() {
            sdl2::Input::Quit | sdl2::Input::RightClick | sdl2::Input::Back | sdl2::Input::Left => {
                play_ui(mixer, ui, UiSe::Cancel);
                return SpecialPick::Back;
            }
            sdl2::Input::Up => {
                sel = (sel + N - 1) % N;
                play_ui(mixer, ui, UiSe::Move);
            }
            sdl2::Input::Down => {
                sel = (sel + 1) % N;
                play_ui(mixer, ui, UiSe::Move);
            }
            sdl2::Input::Advance | sdl2::Input::Right | sdl2::Input::Choice(_) => {
                play_ui(mixer, ui, UiSe::Confirm);
                return pick(sel);
            }
            sdl2::Input::Pointer { x, y, pressed } => {
                let (sx, sy) = win.surface_pos(x, y, W, H);
                if let Some(i) = (0..N).find(|i| {
                    let iy = Y0 + *i as i32 * PITCH;
                    sx >= X0 && sx < X0 + ITEM_W && sy >= iy && sy < iy + ITEM_H
                }) {
                    if i != sel {
                        sel = i;
                        play_ui(mixer, ui, UiSe::Move);
                    }
                    if pressed {
                        play_ui(mixer, ui, UiSe::Confirm);
                        return pick(sel);
                    }
                }
            }
            _ => {}
        }
        let (ww, wh) = win.window_size();
        let ww = ww.max(1);
        let wh = wh.max(1);
        let target = (Y0 + sel as i32 * PITCH) as f32;
        slide_bar(&mut bar_y, target, wh);
        let now = std::time::Instant::now();
        let dt = now.saturating_duration_since(snow_tick).as_secs_f32().min(0.05);
        snow_tick = now;
        if weather.is_some() {
            snow.step(dt);
        }

        // T0100 draws each label about 29px tall inside the 40px row.
        let ipx = layout_font_px(32, wh);
        let generation = render::text::role_generation();
        if ipx != label_px || generation != label_gen {
            label_px = ipx;
            label_gen = generation;
            labels = menu_labels(&items, ipx);
        }
        let hpx = layout_font_px(16, wh);
        if hpx != head_px || generation != head_gen {
            head_px = hpx;
            head_gen = generation;
            head = render::text::with_ui_font(hpx, |f| f.line_rgba(head_text, hpx)).flatten();
        }

        ensure_fitted(
            &mut fitted,
            ww,
            wh,
            0,
            || {
                // The street plate is composited under this surface; with no
                // street the plate itself is the navy field the logo was
                // authored on.
                let plate_bg = if street.is_some() { [0, 0, 0, 0] } else { BACKDROP };
                let mut s = render::Surface::new(W, H, plate_bg);
                if let Some(ref lg) = logo {
                    s.blit(lg, (W as i32 - lg.width as i32) / 2, 24);
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
            // 136 layout pixels is the clamped bottom of T0000 (856 - 720).
            let sy = ((136 * wh) / 720).clamp(0, max_y);
            let mut frame = render::Surface::new(ww as u32, wh as u32, BACKDROP);
            frame.copy_rows_from(scaled, sy);
            frame.blit_surface(&fitted.plate, 0, 0);
            frame
        } else {
            clone_surf(&fitted.plate)
        };
        paint_bar(&mut frame, fitted, 0, bar_y, X0);
        if let Some((w, h, rgba)) = head.as_ref() {
            let shadow = ((2 * wh) / 720).max(1);
            blit_tinted_label(
                &mut frame,
                rgba,
                *w,
                *h,
                (ww - *w as i32) / 2,
                (Y0 - 44) * wh / 720,
                [255, 255, 255],
                shadow,
            );
        }
        draw_menu_labels(&mut frame, &labels, sel, X0, Y0, ITEM_W, ITEM_H, PITCH, ww, wh);
        let stats_label = stats.label(cn);
        draw_label(&mut frame, &stats_label, 15, 0, H as i32 - 36, true, ww as u32, wh as u32);
        if let Some(ref sheet) = weather {
            snow.ensure(ww, wh, sheet);
            snow.paint(&mut frame, wh);
        }
        win.present(&frame);
        win.delay(16);
    }
}

/// Music mode.  BGM files are indexed by track number.  Where a track ships
/// both arrangements the full `_B` one is listed, since `_A` is often a short
/// excerpt (`BGM_041_A` is 5 s of an 83 s piece) and a listening room should not
/// play a clip.  The player keeps one looping stream alive and never decodes the
/// whole library up front.
pub(crate) fn run_music(
    win: &mut sdl2::Sdl,
    bg_dirs: &[&std::path::Path],
    bgm_dir: &std::path::Path,
    cn: bool,
    mixer: Option<&audio::Mixer>,
    ui: Option<&UiSound>,
) {
    const W: u32 = 1280;
    const H: u32 = 720;
    let mut choices: std::collections::BTreeMap<i32, (u8, std::path::PathBuf)> =
        std::collections::BTreeMap::new();
    if let Ok(rd) = std::fs::read_dir(bgm_dir) {
        for ent in rd.flatten() {
            let path = ent.path();
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
            let low = stem.to_ascii_lowercase();
            let Some(rest) = low.strip_prefix("bgm_") else { continue };
            let number = rest.split('_').next().and_then(|s| s.parse::<i32>().ok());
            let Some(number) = number else { continue };
            let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
            if !ext.eq_ignore_ascii_case("ogg") { continue; }
            // Same preference as `bgm_file`: plain name, then `_B`, then `_A`.
            let rank = if low.ends_with("_a") { 2 } else if low.ends_with("_b") { 1 } else { 0 };
            if choices.get(&number).map(|(old, _)| rank < *old).unwrap_or(true) {
                choices.insert(number, (rank, path));
            }
        }
    }
    let tracks: Vec<(i32, std::path::PathBuf)> = choices.into_iter().map(|(id, (_, p))| (id, p)).collect();
    let header = load_ui_asset_hi(bg_dirs, "sys_04000.tga", 1280, 40);
    let logo = load_ui_asset_hi(bg_dirs, "sys_04010.tga", 304, 168);
    let footer = load_ui_asset_hi(bg_dirs, "sys_04021.tga", 288, 40);
    let info_bar = load_ui_asset_hi(bg_dirs, "sys_04030.tga", 1024, 56);
    let select_sheet = load_ui_asset_hi(bg_dirs, "sys_04100.tga", 400, 128);
    let name_sheet = load_ui_asset_hi(bg_dirs, "sys_04101.tga", 344, 1512);
    let backdrop = load_background(bg_dirs, "T0000.tga").map(|img| surface_from_tga(&img));
    let mut selected = 0usize;
    let mut playing: Option<(i32, usize)> = None;
    let mut notice = String::new();
    let mut backdrop_fit: Option<(u32, u32, render::Surface)> = None;
    let labels: Vec<Option<codec::tga::TgaImage>> = tracks.iter().enumerate().map(|(i, _)| {
        name_sheet.as_ref().and_then(|sheet| {
            let sc = sheet.scale;
            let y = (i as u32 * 24 * sc).min(sheet.img.height.saturating_sub(24 * sc));
            crop_image(&sheet.img, 0, y, 344 * sc, 24 * sc)
        })
    }).collect();
    eprintln!("music: indexed {} tracks from {}", tracks.len(), bgm_dir.display());

    loop {
        match win.poll_input() {
            sdl2::Input::Quit | sdl2::Input::RightClick | sdl2::Input::Back | sdl2::Input::Left => {
                play_ui(mixer, ui, UiSe::Cancel);
                break;
            }
            sdl2::Input::Up => {
                if !tracks.is_empty() {
                    selected = (selected + tracks.len() - 1) % tracks.len();
                    play_ui(mixer, ui, UiSe::Move);
                }
            }
            sdl2::Input::Down => {
                if !tracks.is_empty() {
                    selected = (selected + 1) % tracks.len();
                    play_ui(mixer, ui, UiSe::Move);
                }
            }
            sdl2::Input::Advance | sdl2::Input::Right | sdl2::Input::Choice(_) => {
                if let Some((id, path)) = tracks.get(selected) {
                    play_ui(mixer, ui, UiSe::Confirm);
                    if let Some(mx) = mixer {
                        if let Some((_, slot)) = playing.take() { mx.stop(slot); }
                        let slot = mx.play_cat(&path.to_string_lossy(), 1.0, true, audio::mixer::CAT_BGM);
                        if slot != usize::MAX {
                            playing = Some((*id, slot));
                            notice = format!("{}  {}", if cn { "正在播放" } else { "Playing" }, id);
                            eprintln!("music: playing {} -> {}", id, path.display());
                        } else {
                            notice = if cn { "无法播放此曲目".into() } else { "Unable to play track".into() };
                        }
                    } else {
                        notice = if cn { "音频设备不可用".into() } else { "Audio unavailable".into() };
                    }
                }
            }
            sdl2::Input::Pointer { x, y, pressed } => {
                let (sx, sy) = win.surface_pos(x, y, W, H);
                if (94..510).contains(&sy) {
                    let col = ((sx - 42) / 400) as usize;
                    let row = ((sy - 94) / 32) as usize;
                    if col < 3 && row < 13 {
                        let index = (selected / 39) * 39 + col * 13 + row;
                        if index < tracks.len() {
                            if selected != index {
                                selected = index;
                                play_ui(mixer, ui, UiSe::Move);
                            }
                            if pressed {
                                play_ui(mixer, ui, UiSe::Confirm);
                                if let Some(mx) = mixer {
                                if let Some((_, slot)) = playing.take() { mx.stop(slot); }
                                let (id, path) = &tracks[selected];
                                let slot = mx.play_cat(&path.to_string_lossy(), 1.0, true, audio::mixer::CAT_BGM);
                                if slot != usize::MAX {
                                    playing = Some((*id, slot));
                                    notice = format!("{}  {}", if cn { "正在播放" } else { "Playing" }, id);
                                    eprintln!("music: playing {} -> {}", id, path.display());
                                }
                            }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        let page_size = 39usize;
        let top = (selected / page_size) * page_size;
        let (ww, wh) = win.window_size();
        let ww = ww.max(1) as u32;
        let wh = wh.max(1) as u32;
        let sx = |x: i32| (x as u32 * ww / W).max(0);
        let sy = |y: i32| (y as u32 * wh / H).max(0);
        let sw = |w: u32| (w * ww / W).max(1);
        let sh = |h: u32| (h * wh / H).max(1);
        if let Some(b) = backdrop.as_ref() {
            let rebuild = backdrop_fit.as_ref().map(|(bw, bh, _)| *bw != ww || *bh != wh).unwrap_or(true);
            if rebuild {
                backdrop_fit = Some((ww, wh, b.scaled_to(ww, wh)));
            }
        }
        let mut frame = backdrop_fit.as_ref().map(|(_, _, b)| clone_surf(b))
            .unwrap_or_else(|| render::Surface::new(ww, wh, [9, 31, 43, 255]));
        let draw = |dst: &mut render::Surface, a: &HiUiAsset, x: i32, y: i32, w: u32, h: u32| {
            dst.blit_region_scaled(&a.img, 0, 0, a.img.width, a.img.height,
                sx(x) as i32, sy(y) as i32, sw(w), sh(h), 255);
        };
        if let Some(h) = header.as_ref() { draw(&mut frame, h, 0, 0, 1280, 40); }
        if let Some(l) = logo.as_ref() { draw(&mut frame, l, 42, 48, 304, 168); }
        for (row, i) in (top..(top + page_size).min(tracks.len())).enumerate() {
            let col = row / 13;
            let line = row % 13;
            let x = 42 + col as i32 * 400;
            let y = 94 + line as i32 * 32;
            if let Some(sheet) = select_sheet.as_ref() {
                let mark_y = if i == selected { 64 } else { 32 };
                frame.blit_region_scaled(&sheet.img, 0, (mark_y * sheet.scale) as i32, 400 * sheet.scale, 32 * sheet.scale,
                    sx(x) as i32, sy(y) as i32, sw(400), sh(32), 255);
            }
            if let Some(name) = labels[i].as_ref() {
                frame.blit_region_scaled(name, 0, 0, name.width, name.height,
                    sx(x + 46) as i32, sy(y + 4) as i32, sw(344), sh(24), 255);
            } else {
                let fallback = format!("BGM_{:03}", tracks[i].0);
                let px = ((22 * wh) / H).max(8);
                if let Some((tw, th, rgba)) = render::text::with_ui_font(px, |f| f.line_rgba(&fallback, px)).flatten() {
                    frame.blit_rgba(&rgba, tw, th, sx(x + 46) as i32, sy(y + 4) as i32);
                }
            }
        }
        if tracks.is_empty() {
            notice = if cn { "未找到 BGM 文件".into() } else { "No BGM files".into() };
        }
        if !notice.is_empty() {
            let px = ((20 * wh) / H).max(8);
            if let Some((w, h, rgba)) = render::text::with_ui_font(px, |f| f.line_rgba(&notice, px)).flatten() {
                frame.blit_rgba(&rgba, w, h, sx(440) as i32, sy(610) as i32);
            }
        }
        if let Some(bar) = info_bar.as_ref() { draw(&mut frame, bar, 128, 532, 1024, 56); }
        if let Some(name) = labels.get(selected).and_then(|x| x.as_ref()) {
            frame.blit_region_scaled(name, 0, 0, name.width, name.height,
                sx(165) as i32, sy(560) as i32, sw(344), sh(24), 255);
        }
        if let Some(f) = footer.as_ref() { draw(&mut frame, f, 496, 620, 288, 40); }
        win.present(&frame);
        win.delay(16);
    }
    if let Some((_, slot)) = playing {
        if let Some(mx) = mixer { mx.stop(slot); }
    }
}
