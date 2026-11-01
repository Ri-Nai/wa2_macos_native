//! Command line entry points and the sub-commands used to inspect the game data.

use std::path::Path;

use crate::*;

/// Chinese mode is only usable when some bundled or installed font can draw
/// Chinese. The design requires the game to refuse to start instead of
/// painting a screen of empty text; the headless CLIs stay available because
/// they are the tools used to diagnose a missing font.
pub(crate) fn require_chinese_font() {
    if !render::text::chinese_font_available() {
        eprintln!(
            "缺少中文字体：未找到可显示中文的字体，请把 NotoSansCJK-Medium.ttc 放入 fonts/ 或安装系统 CJK 字体"
        );
        std::process::exit(1);
    }
}

pub(crate) fn dispatch() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        print_usage();
        return;
    }
    match args[1].as_str() {
        "kcap" => {
            if args.len() < 3 { eprintln!("usage: wa2-native kcap <pakfile> [outdir]"); return; }
            let out = out_dir(&args[2], &args, 3, "_extracted");
            if let Err(e) = archive::kcap::unpack(Path::new(&args[2]), Path::new(&out)) {
                eprintln!("error: {}", e); std::process::exit(1);
            }
        }
        "lac" => {
            if args.len() < 3 { eprintln!("usage: wa2-native lac <pakfile> [outdir]"); return; }
            let out = out_dir(&args[2], &args, 3, "_audio");
            if let Err(e) = archive::lac::unpack(Path::new(&args[2]), Path::new(&out)) {
                eprintln!("error: {}", e); std::process::exit(1);
            }
        }
        "extract" => {
            if args.len() < 3 { eprintln!("usage: wa2-native extract <pakfile> [outdir]"); return; }
            let path = Path::new(&args[2]);
            let mut hdr = [0u8; 4];
            if std::fs::File::open(path).and_then(|mut f| std::io::Read::read_exact(&mut f, &mut hdr)).is_err() {
                eprintln!("error: cannot read {}", path.display()); std::process::exit(1);
            }
            let out = out_dir(&args[2], &args, 3, "_extracted");
            let res = if &hdr[0..4] == b"KCAP" {
                archive::kcap::unpack(path, Path::new(&out))
            } else if &hdr[0..4] == b"LAC\0" {
                archive::lac::unpack(path, Path::new(&out))
            } else {
                eprintln!("error: unknown archive magic {:?}", &hdr[0..4]); std::process::exit(1);
            };
            if let Err(e) = res { eprintln!("error: {}", e); std::process::exit(1); }
        }
        "timeline-check" => {
            // Offline chart consistency report: node/branch/ending counts,
            // roots, dangling jump targets. Costs half a second, no window.
            let dir = args.get(2).map(String::as_str).unwrap_or("data/script");
            let lines = timeline::check(Path::new(dir));
            for line in lines {
                println!("{}", line);
            }
        }
        "tga" => {
            if args.len() < 3 { eprintln!("usage: wa2-native tga <file.tga>"); return; }
            let data = match std::fs::read(&args[2]) { Ok(d) => d, Err(e) => { eprintln!("error: {}", e); std::process::exit(1); } };
            match codec::tga::decode(&data) {
                Ok(img) => println!("{}: {} x {} RGBA ({} bytes)", args[2], img.width, img.height, img.rgba.len()),
                Err(e) => { eprintln!("tga decode error: {}", e); std::process::exit(1); }
            }
        }
        "render-test" => {
            if args.len() < 4 { eprintln!("usage: wa2-native render-test <font.tga> <out.tga> [sprite.tga]"); return; }
            let font = match codec::tga::decode(&match std::fs::read(&args[2]) { Ok(d) => d, Err(e) => { eprintln!("error: {}", e); return; } }) {
                Ok(i) => i, Err(e) => { eprintln!("tga decode error: {}", e); return; }
            };
            let sprite = if args.len() >= 5 {
                match std::fs::read(&args[4]).ok().and_then(|d| codec::tga::decode(&d).ok()) {
                    Some(s) => Some(s), None => { eprintln!("warning: could not load sprite {}", args[4]); None }
                }
            } else { None };
            let surf = render::demo_scene(&font, sprite.as_ref(), 1280, 720);
            if let Err(e) = surf.save_tga(&args[3]) { eprintln!("error writing {}: {}", args[3], e); std::process::exit(1); }
            println!("wrote scene {} ({}x{}), font atlas {}x{}", args[3], surf.width, surf.height, font.width, font.height);
        }
        "font-test" => {
            if args.len() < 5 { eprintln!("usage: wa2-native font-test <font.tga> <charset.txt> <out.tga> [text]"); return; }
            let font = match load_font_charset(&args[2], &args[3]) { Some(f) => f, None => return };
            let charset = match std::fs::read_to_string(&args[3]) { Ok(s) => s, Err(e) => { eprintln!("error: {}", e); return; } };
            let sample: String = if args.len() >= 6 {
                args[5..].join(" ")
            } else {
                charset.chars().filter(|c| !c.is_whitespace()).take(40).collect()
            };
            let mut surf = render::Surface::new(1280, 720, [20, 20, 40, 255]);
            surf.fill_rect(40, 500, 1200, 160, [0, 0, 0, 180]);
            font.draw(&mut surf, &sample, 60, 520);
            if let Err(e) = surf.save_tga(&args[4]) { eprintln!("error writing {}: {}", args[4], e); std::process::exit(1); }
            println!("rendered {} chars of sample text -> {}", sample.chars().count(), args[4]);
        }
        "font-sample" => {
            // font-sample <out.tga> [w] [h]  : fixed vector-font specimen sheet
            // (body, dense glyphs, punctuation, Latin mix, and the real dialog
            // path with a name and three lines), drawn with the WA2_FONT*
            // settings in force so two configurations can be compared.
            if args.len() < 3 { eprintln!("usage: wa2-native font-sample <out.tga> [w] [h]"); return; }
            let out = args[2].clone();
            let w: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1280);
            let h: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(720);
            // The font ids and the dialogue style live in wa2.ini. Loading the
            // settings here makes the sheet match what the game would draw,
            // instead of only the WA2_FONT* environment.
            let _ = menu::Settings::load();
            let choice = render::text::font_choice_for(render::text::FontRole::Game);
            let mut vf = match render::text::VectorFont::open_face(&choice.path, render::DIALOG_FONT_720, choice.face) {
                Some(f) => f,
                None => {
                    eprintln!("font-sample: cannot open {} (face {})", choice.path, choice.face);
                    std::process::exit(1);
                }
            };
            render::text::apply_choice(&vf, &choice);
            eprintln!("font: {}", vf.describe());
            let mut s = render::Surface::new(w, h, [20, 20, 28, 255]);
            let px = |v: i32| (v * h as i32 + 360) / 720;
            let size = px(render::DIALOG_FONT_720 as i32).max(8) as u32;
            let _ = vf.set_size(size);
            let body = render::text::TextStyle::body();
            let label = render::text::TextStyle::name();
            let mut y = px(40);
            let step = px(56);
            vf.draw_styled(&mut s, &vf.describe(), px(40), y, &label);
            y += step;
            for (tag, text) in [
                ("正文", "春希、雪菜、冬马和纱，路边的树木叶子差不多已经落尽"),
                ("复杂", "鬱邊體樂觀發臺裏骨直着国门广发"),
                ("标点", "，。！？「」『』\u{201c}\u{201d}\u{2018}\u{2019}……——"),
                ("混排", "WHITE ALBUM 2 100% 2026"),
            ] {
                vf.draw_styled(&mut s, tag, px(40), y, &label);
                vf.draw_styled(&mut s, text, px(140), y, &body);
                y += step;
            }
            // The dialog itself, through the same path the game uses: a name on
            // the rim and three body lines in the oval.
            let dialog = "已经，什么都无所谓了。\n无论是白色季节里的雪色，\n还是渐渐残碎的约定。";
            render::draw_dialog_vector(&mut s, &mut vf, dialog, Some("冬马和纱"), w as i32, h as i32, usize::MAX);
            if let Err(e) = s.save_tga(&out) { eprintln!("error writing {}: {}", out, e); std::process::exit(1); }
            println!("font-sample: {}x{} -> {}", w, h, out);
        }
        "menu-clean" => {
            // menu-clean: bake the caption-free option pages once, so the
            // settings screen stops repairing pixels on every state change.
            let Some(d) = data::Data::resolve() else { return };
            // Same search order the settings screen uses: the Chinese overlay
            // sheets first, then the original. The 1x trees come first on
            // purpose - the menu draws at logical size and the 4x upscaler
            // mangled some of its sheets.
            let mut dirs_buf: Vec<std::path::PathBuf> = Vec::new();
            for name in ["bg_cn", "bg", "bg_cn_8x", "bg_8x"] {
                let p = d.root.join(name);
                if p.is_dir() {
                    dirs_buf.push(p);
                }
            }
            let dirs: Vec<&std::path::Path> = dirs_buf.iter().map(|p| p.as_path()).collect();
            let out = std::path::Path::new(&std::env::var("WA2_DATA").unwrap_or_else(|_| "data".into()))
                .join("cache/ui/menu");
            let pages = menu::bake_clean_pages(&dirs, &out);
            eprintln!("menu-clean: {} page(s) -> {}", pages.len(), out.display());
            println!("wrote {} clean page(s) to {}", pages.len(), out.display());
        }
        "scene" => {
            if args.len() < 7 {
                eprintln!("usage: wa2-native scene <scenario.txt> <font.tga> <charset.txt> <bg.tga> <out.tga>");
                return;
            }
            let scen = match std::fs::read(&args[2]) { Ok(d) => d, Err(e) => { eprintln!("error: {}", e); return; } };
            let page = vm::parse_scenario_text(&scen);
            let font = match load_font_charset(&args[3], &args[4]) { Some(f) => f, None => return };
            let bg = match std::fs::read(&args[5]).ok().and_then(|d| codec::tga::decode(&d).ok()) {
                Some(i) => i, None => { eprintln!("error: could not decode background {}", args[5]); return; }
            };
            // Optional 6th argument: an `.AMP` colour curve, for previewing the
            // sepia/nega flashback looks without running the game.
            let lut = args.get(7).and_then(|p| {
                let d = std::fs::read(p).ok()?;
                eprintln!("tint: {} ({} bytes)", p, d.len());
                Some(d)
            });
            let surf =
                render::render_scene_ex(&bg, &page.text(), &font, 1280, 720, &[], lut.as_deref(), true, None, None, 255, 0, &[]);
            if let Err(e) = surf.save_tga(&args[6]) { eprintln!("error writing {}: {}", args[6], e); std::process::exit(1); }
            println!("scene {} (voice={:?} bg={:?} next={:?}): {} lines, {} chars -> {}",
                args[2], page.voice, page.background, page.next, page.lines.len(), page.text().chars().count(), args[6]);
        }
        "scene-map" => {
            // scene-map <scenario.txt> <font.tga> <map.txt> <bg.tga> <out.tga>
            if args.len() < 7 {
                eprintln!("usage: wa2-native scene-map <scenario.txt> <font.tga> <map.txt> <bg.tga> <out.tga>");
                return;
            }
            let scen = match std::fs::read(&args[2]) { Ok(d) => d, Err(e) => { eprintln!("error: {}", e); return; } };
            let page = vm::parse_scenario_text(&scen);
            let atlas = match std::fs::read(&args[3]).ok().and_then(|d| codec::tga::decode(&d).ok()) {
                Some(i) => i, None => { eprintln!("font decode fail"); return; }
            };
            let font = match render::Font::from_map_file(atlas, 24, &args[4]) {
                Ok(f) => f, Err(e) => { eprintln!("map: {}", e); return; }
            };
            let bg = match std::fs::read(&args[5]).ok().and_then(|d| codec::tga::decode(&d).ok()) {
                Some(i) => i, None => { eprintln!("bg fail"); return; }
            };
            let surf = render::render_scene(&bg, &page.text(), &font, 1280, 720);
            if let Err(e) = surf.save_tga(&args[6]) { eprintln!("{}", e); return; }
            println!("scene-map: {} chars -> {}", page.text().chars().count(), args[6]);
        }
        "audio-test" => {
            // audio-test <file.ogg> [seconds]
            if args.len() < 3 {
                eprintln!("usage: wa2-native audio-test <file> [seconds]");
                return;
            }
            let secs: f64 = if args.len() >= 4 { args[3].parse().unwrap_or(5.0) } else { 5.0 };
            if !std::path::Path::new(&args[2]).is_file() {
                eprintln!("audio-test: input file not found: {}", args[2]);
                return;
            }
            let win = match sdl2::Sdl::new("wa2-audio", 320, 240) { Some(w) => w, None => { eprintln!("no SDL/display"); return; } };
            let mixer = match audio::Mixer::new(&win) { Some(m) => m, None => { eprintln!("audio device failed"); return; } };
            eprintln!("playing {} for {}s", args[2], secs);
            let slot = mixer.play(&args[2], 1.0, false);
            if slot == usize::MAX {
                eprintln!("audio-test: unable to decode or open {}", args[2]);
                return;
            }
            let pulled_before = audio::mixer::pulled_samples();
            let t0 = std::time::Instant::now();
            while t0.elapsed().as_secs_f64() < secs {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            let pulled = audio::mixer::pulled_samples();
            if pulled == pulled_before {
                eprintln!("audio-test: decoder produced no PCM for {}", args[2]);
            }
            println!("audio-test done: driver={} pulled {} samples, queued {} bytes", sdl2::sdl_audio_driver(), pulled, audio::mixer::queued_bytes());
        }
        "movie-test" => {
            // movie-test <mv.pak> [--audio <ogg>]
            if args.len() < 3 {
                eprintln!("usage: wa2-native movie-test <mv.pak> [bgm.ogg]");
                return;
            }
            let mut movie = match video::Movie::open(&args[2]) { Some(m) => m, None => { eprintln!("ffmpeg/ffprobe failed or no video"); return; } };
            eprintln!("movie {}x{} @ {:.3}fps has_audio={}", movie.width, movie.height, movie.fps, movie.has_audio);
            let mut win = match sdl2::Sdl::new("wa2-movie", movie.width as i32 as u32, movie.height) { Some(w) => w, None => { eprintln!("no SDL/display"); return; } };
            // args[3] is an optional external track (old debug usage); with no
            // argument the movie's own audio stream is mixed instead.
            let mixer = if args.len() >= 4 || movie.has_audio { audio::Mixer::new(&win) } else { None };
            let slot = match mixer.as_ref() {
                Some(mx) if args.len() >= 4 => Some(mx.play(&args[3], 1.0, false)).filter(|s| *s != usize::MAX),
                Some(mx) => movie.play_audio(mx, audio::mixer::CAT_BGM),
                None => None,
            };
            eprintln!("movie audio: {}", if slot.is_some() { "playing" } else { "silent" });
            let mut frame = vec![0u8; (movie.width * movie.height * 4) as usize];
            let fps = movie.fps.max(1.0);
            let mut t0 = std::time::Instant::now();
            let mut count = 0u64;
            'play: loop {
                if !movie.read_frame(&mut frame) { eprintln!("EOF"); break; }
                win.present_raw(&frame, movie.width, movie.height);
                count += 1;
                let deadline = t0 + std::time::Duration::from_secs_f64(count as f64 / fps);
                loop {
                    match win.poll_input() {
                        sdl2::Input::Quit => { eprintln!("quit"); break 'play; }
                        _ => {}
                    }
                    let now = std::time::Instant::now();
                    if now >= deadline { break; }
                    let rest = (deadline - now).as_millis() as u32;
                    if rest > 500 { t0 = now - std::time::Duration::from_secs_f64(count as f64 / fps); break; }
                    win.delay(rest.clamp(1, 8));
                }
            }
            if let (Some(mx), Some(s)) = (mixer.as_ref(), slot) { mx.stop(s); }
            eprintln!("played {} frames", count);
        }
        "movie-frame" => {
            // movie-frame <mv.pak> <out.tga>  : decode frame 0 and save as TGA
            if args.len() < 4 {
                eprintln!("usage: wa2-native movie-frame <mv.pak> <out.tga>");
                return;
            }
            let mut movie = match video::Movie::open(&args[2]) { Some(m) => m, None => { eprintln!("open fail"); return; } };
            let want: usize = if args.len() >= 5 { args[4].parse().unwrap_or(0) } else { 0 };
            let mut frame = vec![0u8; (movie.width * movie.height * 4) as usize];
            for i in 0..=want {
                if !movie.read_frame(&mut frame) { eprintln!("no frame {}", i); return; }
            }
            let mut s = render::Surface::new(movie.width, movie.height, [0, 0, 0, 255]);
            for y in 0..movie.height {
                for x in 0..movie.width {
                    let si = ((y * movie.width + x) * 4) as usize;
                    let c = [frame[si], frame[si + 1], frame[si + 2], frame[si + 3]];
                    s.blend_pixel(x as i32, y as i32, c);
                }
            }
            if let Err(e) = s.save_tga(&args[3]) { eprintln!("{}", e); return; }
            println!("saved frame0 {}x{} -> {}", movie.width, movie.height, args[3]);
        }
        "scene-noto" => {
            // scene-noto <scenario.txt> <bg.tga> <out.tga>
            if args.len() < 5 {
                eprintln!("usage: wa2-native scene-noto <scenario.txt> <bg.tga> <out.tga>");
                return;
            }
            // Draw with the same Home role font the game uses: the ids and the
            // style come from wa2.ini, not only from WA2_FONT*.
            let _ = menu::Settings::load();
            let scen = match std::fs::read(&args[2]) { Ok(d) => d, Err(e) => { eprintln!("error: {}", e); return; } };
            let page = vm::parse_scenario_text(&scen);
            let bg = match std::fs::read(&args[3]).ok().and_then(|d| codec::tga::decode(&d).ok()) {
                Some(i) => i, None => { eprintln!("bg fail"); return; }
            };
            let surf = render::render_scene_sysfont(&bg, &page.text(), 1280, 720);
            if let Err(e) = surf.save_tga(&args[4]) { eprintln!("{}", e); return; }
            println!("scene-noto: {} chars -> {}", page.text().chars().count(), args[4]);
        }
        "play-noto" => {
            // play-noto <scenario.txt> <bg.tga> [bgm.ogg]
            if args.len() < 4 {
                eprintln!("usage: wa2-native play-noto <scenario.txt> <bg.tga> [bgm.ogg]");
                return;
            }
            // Same settings the game loads: the Home role font and the style
            // come from wa2.ini as well as from WA2_FONT*.
            let _ = menu::Settings::load();
            let scen = match std::fs::read(&args[2]) { Ok(d) => d, Err(e) => { eprintln!("error: {}", e); return; } };
            let page = vm::parse_scenario_text(&scen);
            let bg = match std::fs::read(&args[3]).ok().and_then(|d| codec::tga::decode(&d).ok()) {
                Some(i) => i, None => { eprintln!("bg fail"); return; }
            };
            let mut win = match sdl2::Sdl::new("wa2-native", 1280, 720) { Some(w) => w, None => { eprintln!("no SDL/display"); return; } };
            let _mixer = if args.len() >= 5 { audio::Mixer::new(&win).map(|m| { m.play(&args[4], 0.8, true); m }) } else { None };
            eprintln!("play-noto: {} lines (system font). Space/Right/Click=next, Left=back, Esc/q=quit", page.lines.len());
            let mut idx: i32 = 0;
            let n = page.lines.len() as i32;
            loop {
                let surf = render::render_scene_sysfont(&bg, &page.lines[idx.max(0) as usize], 1280, 720);
                win.present(&surf);
                loop {
                    match win.poll_input() {
                        sdl2::Input::Advance | sdl2::Input::Right => { if idx < n - 1 { idx += 1; } break; }
                        sdl2::Input::Back | sdl2::Input::Left => { if idx > 0 { idx -= 1; } break; }
                        sdl2::Input::Up => { if idx > 0 { idx -= 1; } break; }
                        sdl2::Input::Down => { if idx < n - 1 { idx += 1; } break; }
                        sdl2::Input::Quit => return,
                        sdl2::Input::Fullscreen => break,
                        sdl2::Input::RightClick => break,
                        sdl2::Input::Drag { .. } => {}
                        sdl2::Input::None => win.delay(16),
                        sdl2::Input::Choice(_) => win.delay(16),
                        sdl2::Input::Pointer { .. } | sdl2::Input::Release { .. } => win.delay(16),
                        sdl2::Input::Wheel { y } if y < 0 => { if idx < n - 1 { idx += 1; } break; }
                        sdl2::Input::Wheel { .. } => win.delay(16),
                        sdl2::Input::Rewind => win.delay(16),
                        sdl2::Input::Nav(_) => win.delay(16),
                    }
                }
            }
        }
        "tga-dump" => {
            if args.len() < 4 {
                eprintln!("usage: wa2-native tga-dump <in.tga> <out.tga>");
                return;
            }
            let img = match std::fs::read(&args[2]).ok().and_then(|d| codec::tga::decode(&d).ok()) {
                Some(i) => i, None => { eprintln!("decode fail"); return; }
            };
            let mut s = render::Surface::new(img.width, img.height, [0, 0, 0, 255]);
            for y in 0..img.height {
                for x in 0..img.width {
                    let si = ((y * img.width + x) * 4) as usize;
                    let c = [img.rgba[si], img.rgba[si + 1], img.rgba[si + 2], img.rgba[si + 3]];
                    s.blend_pixel(x as i32, y as i32, c);
                }
            }
            if let Err(e) = s.save_tga(&args[3]) { eprintln!("{}", e); return; }
            println!("dumped {} -> {} ({}x{})", args[2], args[3], img.width, img.height);
        }
        "fmtinfo" => {
            let mut win = match sdl2::Sdl::new("fmt", 64, 64) { Some(w) => w, None => { eprintln!("no SDL/display"); return; } };
            for c in [0x16362004u32, 0x16762004, 0x16462004, 0x16262004] {
                println!("0x{:08X} = {}", c, win.format_name(c));
            }
        }
        "verify-window" => {
            if args.len() < 5 {
                eprintln!("usage: wa2-native verify-window <font.tga> <charset.txt> <bg.tga>");
                return;
            }
            let font = match load_font_charset(&args[2], &args[3]) { Some(f) => f, None => return };
            let bg = match std::fs::read(&args[4]).ok().and_then(|d| codec::tga::decode(&d).ok()) {
                Some(i) => i, None => { eprintln!("error bg"); return; }
            };
            let surf = render::render_scene(&bg, "\u{4E00}\u{4E8C}\u{4E09}\u{56DB}\u{4E94} test", &font, 1280, 720);
            let mut win = match sdl2::Sdl::new("wa2-verify", 1280, 720) { Some(w) => w, None => { eprintln!("no SDL/display"); return; } };
            win.present(&surf);
            let rb = win.read_back(1280, 720);
            // compare a sample of pixels
            let mut diff = 0u32; let mut nonblack_rb = 0u32; let mut nonblack_surf = 0u32;
            for i in 0..(1280*720) {
                let si = i*4;
                let sr=surf.pixels[si] as i32; let sg=surf.pixels[si+1] as i32; let sb=surf.pixels[si+2] as i32;
                let rr=rb[si] as i32; let rg=rb[si+1] as i32; let rbl=rb[si+2] as i32;
                if sr+sg+sb > 30 { nonblack_surf+=1; }
                if rr+rg+rbl > 30 { nonblack_rb+=1; }
                if (sr-rr).abs()+(sg-rg).abs()+(sb-rbl).abs() > 30 { diff+=1; }
            }
            println!("surface nonblack px: {}, readback nonblack px: {}, differing px: {}", nonblack_surf, nonblack_rb, diff);
            if nonblack_rb > 1000 { println!("SDL render path OK (readback has content)"); } else { println!("SDL readback EMPTY -> render path problem"); }
        }
        "play" => {
            // wa2-native play <scenario.txt> <font.tga> <charset.txt> <bg.tga>
            if args.len() < 6 {
                eprintln!("usage: wa2-native play <scenario.txt> <font.tga> <charset.txt> <bg.tga>");
                return;
            }
            let scen = match std::fs::read(&args[2]) { Ok(d) => d, Err(e) => { eprintln!("error: {}", e); return; } };
            let page = vm::parse_scenario_text(&scen);
            let font = match load_font_charset(&args[3], &args[4]) { Some(f) => f, None => return };
            let bg = match std::fs::read(&args[5]).ok().and_then(|d| codec::tga::decode(&d).ok()) {
                Some(i) => i,
                None => { eprintln!("error: could not decode background {}", args[5]); return; }
            };
            let mut win = match sdl2::Sdl::new("wa2-native", 1280, 720) {
                Some(w) => w,
                None => { eprintln!("error: SDL2 unavailable / no display. Try DISPLAY=:0"); return; }
            };
            eprintln!("wa2-native play: {} lines. Space/Right/Click=next, Left=back, F11=fullscreen, Esc/q=quit", page.lines.len());
            // optional BGM (6th arg)
            let _mixer = if args.len() >= 7 {
                match audio::Mixer::new(&win) {
                    Some(mx) => { let _ = mx.play(&args[6], 0.8, true); eprintln!("BGM: {}", args[6]); Some(mx) }
                    None => None,
                }
            } else { None };
            let mut idx: i32 = 0;
            let n = page.lines.len() as i32;
            loop {
                let seg = &page.lines[idx.max(0) as usize];
                let text = if idx == 0 {
                    format!("{}{}", seg, "\n\n[Space/Right/Click=next  Esc=quit]")
                } else {
                    seg.clone()
                };
                let surf = render::render_scene(&bg, &text, &font, 1280, 720);
                win.present(&surf);
                // wait for input
                loop {
                    match win.poll_input() {
                        sdl2::Input::Advance | sdl2::Input::Right => { if idx < n - 1 { idx += 1; } break; }
                        sdl2::Input::Back | sdl2::Input::Left => { if idx > 0 { idx -= 1; } break; }
                        sdl2::Input::Up => { if idx > 0 { idx -= 1; } break; }
                        sdl2::Input::Down => { if idx < n - 1 { idx += 1; } break; }
                        sdl2::Input::Quit => return,
                        sdl2::Input::Fullscreen => break,
                        sdl2::Input::RightClick => break,
                        sdl2::Input::Drag { .. } => {}
                        sdl2::Input::Choice(_) => { if idx < n - 1 { idx += 1; } break; }
                        sdl2::Input::Pointer { pressed, .. } => { if pressed && idx < n - 1 { idx += 1; break; } win.delay(16); }
                        sdl2::Input::Release { .. } => win.delay(16),
                        sdl2::Input::None => { win.delay(16); }
                        sdl2::Input::Wheel { y } if y < 0 => { if idx < n - 1 { idx += 1; } break; }
                        sdl2::Input::Wheel { .. } => win.delay(16),
                        sdl2::Input::Rewind => win.delay(16),
                        sdl2::Input::Nav(_) => win.delay(16),
                    }
                }
            }
        }
        "run" => {
            // wa2-native run [start_id]                                  (data tree)
            // wa2-native run <script_dir> <font.tga> <charset.txt> <bg_dir> <start_id> [bgm] [voice_dir]
            let mut win = match sdl2::Sdl::new("wa2-native", 1280, 720) {
                Some(w) => w,
                None => { eprintln!("error: SDL2 unavailable / no display. Try DISPLAY=:0"); return; }
            };
            if args.len() < 7 {
                // Data-tree mode: everything comes from `$WA2_DATA` or ./data.
                let d = match data::Data::resolve() { Some(d) => d, None => return };
                let start_id = args.get(2).cloned().unwrap_or_else(|| "1001".to_string());
                let font = match load_font(&d) { Some(f) => f, None => return };
                eprintln!(
                    "wa2-native run: data={} start={} (Space/Click=next, Up/Wheel=rewind, PageUp=fast rewind, 1-9=choice, Esc/q=quit)",
                    d.describe(), start_id
                );
                let session = Session::from_data(&d, None);
                if session.cn {
                    require_chinese_font();
                }
                let mut rt = Runtime::new(&win, &session);
                if let Err(e) = run_game(&mut win, &font, &session, &start_id, &mut rt) {
                    eprintln!("run error: {}", e);
                }
                render::text::log_missing_glyph_stats();
                return;
            }
            // Legacy explicit-path mode (kept for debugging single scenes).
            let script_dir = std::path::PathBuf::from(&args[2]);
            let bg_dir = std::path::PathBuf::from(&args[5]);
            let start_id = args[6].clone();
            let font = match load_font_paths(std::path::Path::new(&args[3]), None, Some(std::path::Path::new(&args[4]))) { Some(f) => f, None => return };
            let bgm_path = args.get(7).cloned();
            let voice_dir = args.get(8).map(std::path::PathBuf::from);
            let session = Session {
                script_dir: script_dir.clone(),
                cn: is_cn_script(&script_dir),
                bg_trees: vec![(1, bg_dir.clone())],
                bg_cn_trees: Vec::new(),
                bg_cn_original: None,
                bg_original: None,
                sprite_trees: find_sprite_dir(&bg_dir, &script_dir)
                    .map(|d| vec![(1, d)])
                    .unwrap_or_default(),
                movie_dir: bg_dir.join("..").join("mv"),
                voice_dir,
                bgm: bgm_path,
                bg_index: BgIndex::build(&[&bg_dir]),
            };
            eprintln!("wa2-native run: start={} (Space/Click=next, Left=back, 1-9=choice, Esc/q=quit)", start_id);
            let mut rt = Runtime::new(&win, &session);
            if let Err(e) = run_game(&mut win, &font, &session, &start_id, &mut rt) {
                eprintln!("run error: {}", e);
            }
            render::text::log_missing_glyph_stats();
        }
        "title" => {
            // wa2-native title [start_id]                                (data tree)
            // wa2-native title <script_dir> <font.tga> <charset.txt> <bg_dir> [start_id]
            let mut win = match sdl2::Sdl::new("wa2-native", 1280, 720) {
                Some(w) => w,
                None => { eprintln!("error: SDL2 unavailable / no display. Try DISPLAY=:0"); return; }
            };
            let (font, session, start_id, bg_dir) = if args.len() >= 6 {
                let script_dir = std::path::PathBuf::from(&args[2]);
                let bg_dir = std::path::PathBuf::from(&args[5]);
                let font = match load_font_paths(std::path::Path::new(&args[3]), None, Some(std::path::Path::new(&args[4]))) { Some(f) => f, None => return };
                let start_id = args.get(6).cloned().unwrap_or_else(|| "1001".to_string());
                let session = Session {
                    script_dir: script_dir.clone(),
                    cn: is_cn_script(&script_dir),
                    bg_trees: vec![(1, bg_dir.clone())],
                    bg_cn_trees: Vec::new(),
                    bg_cn_original: None,
                    bg_original: None,
                    sprite_trees: find_sprite_dir(&bg_dir, &script_dir)
                        .map(|d| vec![(1, d)])
                        .unwrap_or_default(),
                    movie_dir: bg_dir.join("..").join("mv"),
                    voice_dir: None,
                    bgm: None,
                    bg_index: BgIndex::build(&[&bg_dir]),
                };
                (font, session, start_id, bg_dir)
            } else {
                let d = match data::Data::resolve() { Some(d) => d, None => return };
                let font = match load_font(&d) { Some(f) => f, None => return };
                let start_id = args.get(2).cloned().unwrap_or_else(|| "1001".to_string());
                let bg_dir = d.bg();
                let session = Session::from_data(&d, None);
                eprintln!("wa2-native title: data={}", d.describe());
                (font, session, start_id, bg_dir)
            };
            if session.cn {
                require_chinese_font();
            }
            let mut rt = Runtime::new(&win, &session);
            'home: loop {
                match run_title(
                    &mut win,
                    &font,
                    &session.bg_dirs(),
                    &session.script_dir,
                    !session.bg_cn_trees.is_empty(),
                    rt.mixer.as_ref(),
                ) {
                TitleAction::Start => {
                    eprintln!("title: starting scene {}", start_id);
                    if let Err(e) = run_game(&mut win, &font, &session, &start_id, &mut rt) {
                        eprintln!("run error: {}", e);
                    }
                }
                TitleAction::Jump(id) => {
                    eprintln!("title: timeline jump {}", id);
                    if let Err(e) = run_game(&mut win, &font, &session, &id, &mut rt) {
                        eprintln!("run error: {}", e);
                    }
                }
                TitleAction::Load(snap) => {
                    eprintln!("title: loading scene {}", snap.scene);
                    if let Err(e) = run_game_from(&mut win, &font, &session, snap, &mut rt) {
                        eprintln!("run error: {}", e);
                    }
                }
                    TitleAction::Quit => {
                        eprintln!("title: quit");
                        break 'home;
                    }
                }
            }
            // Aggregate of characters no font could draw, if any: one line for
            // the whole session instead of the per-character warning. Only the
            // GUI entries do this - a headless CLI is often a diagnostic tool.
            render::text::log_missing_glyph_stats();
        }
        "lscr-trace" => {
            // wa2-native lscr-trace <script_dir> <start_id> [choice_seq] [max_events]
            // Headless interpreter trace for verification; choice_seq like "1,2".
            if args.len() < 4 {
                eprintln!("usage: wa2-native lscr-trace <script_dir> <start_id> [choice_seq] [max_events]");
                return;
            }
            let dir = std::path::Path::new(&args[2]);
            let mut id = args[3].clone();
            let seq: Vec<u32> = args
                .get(4)
                .map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).collect())
                .unwrap_or_default();
            let max_ev: usize = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(400);
            let mut seq_i = 0usize;
            let mut scenes = 0;
            let mut global = vm::global::GlobalState::load(dir);
            loop {
                scenes += 1;
                if scenes > 40 {
                    eprintln!("scene chain limit reached");
                    return;
                }
                let prog = match load_program(dir, &id, is_cn_script(dir)) {
                    Some(p) => p,
                    None => {
                        eprintln!("scene {} not found", id);
                        return;
                    }
                };
                eprintln!(
                    "=== scene {}: {} ops, {} tokens, entry {:#x} ===",
                    id,
                    prog.ops.len(),
                    prog.tokens.len(),
                    prog.entry
                );
                let mut it = vm::lscr::Interp::new(&prog, &mut global);
                let mut n = 0usize;
                let mut jumped = false;
                while n < max_ev {
                    n += 1;
                    match it.next_event() {
                        vm::lscr::Event::Text { text, speaker, voice } => {
                            let body = clean_text(&text).replace('\n', " / ");
                            let sp = speaker.unwrap_or_default();
                            let vt = voice.map(|v| format!("{}_{:04}_{:02}", v.scenario, v.line, v.char_id)).unwrap_or_default();
                            println!("  [text] speaker={:?} voice={} | {}", sp, vt, body);
                        }
                        vm::lscr::Event::BgScene { id, variant, frames, mode } =>
                            println!("  [bg  ] id={} var={} mode={} frames={}", id, variant, mode, frames),
                        vm::lscr::Event::EventCg { id, variant, frames } =>
                            println!("  [cg  ] id={} var={} frames={}", id, variant, frames),
                        vm::lscr::Event::ScrollCg { id, variant, frames, pages } =>
                            println!("  [cg-scroll] id={} var={} frames={} pages={}", id, variant, frames, pages),
                        vm::lscr::Event::Flash { frames, r, g, b } =>
                            println!("  [flash] {},{},{} {}f", r, g, b, frames),
                        vm::lscr::Event::Overlay { name, slot } => println!("  [ovl ] slot={} {}", slot, name),
                        vm::lscr::Event::WaitFrames(n) => println!("  [wait] {} frames", n),
                        vm::lscr::Event::Sound { opcode, args } =>
                            println!("  [sound] ACT{} {:?}", opcode, args),
                        vm::lscr::Event::Tint { name } => println!("  [tint] {}", if name.is_empty() { "<none>" } else { &name }),
                        vm::lscr::Event::WindowAlpha { value, channel } =>
                            println!("  [win-alpha] value={} channel={}", value, channel),
                        vm::lscr::Event::Movie { name } => println!("  [mv  ] {}", name),
                        vm::lscr::Event::Sprite { char_id, series, variant, slot, frames, kind } =>
                            println!("  [spr ] {:?} char={} series={} variant={} slot={} frames={}", kind, char_id, series, variant, slot, frames),
                        vm::lscr::Event::SpriteExit { char_id, frames } =>
                            println!("  [spr-] char={} frames={}", char_id, frames),
                        vm::lscr::Event::SpriteFocus { char_id } => println!("  [fo-c] {}", char_id),
                        vm::lscr::Event::EnvTint { preset } => println!("  [amb ] preset={}", preset),
                        vm::lscr::Event::Choice { options } => {
                            let pick = seq.get(seq_i).copied().unwrap_or(1);
                            seq_i += 1;
                            println!("  [choice] {} options -> picking {}", options.len(), pick);
                            for (i, o) in options.iter().enumerate() {
                                let c = clean_text(o);
                                let c = vm::strip_choice_prefix(&c).unwrap_or(c);
                                println!("           {}. {}", i + 1, c);
                            }
                            it.set_choice_result(pick);
                        }
                        vm::lscr::Event::Wait => {}
                        vm::lscr::Event::Jump { target, kind } => {
                            println!("  [jump] {} -> {}", kind, target);
                            id = target;
                            jumped = true;
                            break;
                        }
                        vm::lscr::Event::End => {
                            println!("  [end ] scene {} ended", id);
                            return;
                        }
                    }
                }
                if !jumped {
                    println!("  [stop] event limit reached in scene {}", id);
                    return;
                }
            }
        }
        "sjis" => {
            if args.len() < 3 { eprintln!("usage: wa2-native sjis <file>"); return; }
            let d = match std::fs::read(&args[2]) { Ok(d) => d, Err(e) => { eprintln!("error: {}", e); return; } };
            println!("{}", codec::sjis::decode(&d));
        }
        "text-info" => {
            if args.len() < 3 { eprintln!("usage: wa2-native text-info <scenario.txt>"); return; }
            let scen = match std::fs::read(&args[2]) { Ok(d) => d, Err(e) => { eprintln!("error: {}", e); return; } };
            let page = vm::parse_scenario_text(&scen);
            println!("voice     : {:?}", page.voice);
            println!("speaker   : {:?}", page.speaker);
            println!("background: {:?}", page.background);
            println!("next      : {:?}", page.next);
            println!("lines     : {}", page.lines.len());
            for (i, l) in page.lines.iter().take(12).enumerate() { println!("  [{}] {}", i, l); }
        }
        "steps-info" => {
            if args.len() < 3 { eprintln!("usage: wa2-native steps-info <scenario.txt>"); return; }
            let scen = match std::fs::read(&args[2]) { Ok(d) => d, Err(e) => { eprintln!("error: {}", e); return; } };
            let steps = vm::parse_steps(&scen, is_cn_script(std::path::Path::new(&args[2]).parent().unwrap_or(std::path::Path::new("."))));
            println!("steps: {}", steps.len());
            for (i, s) in steps.iter().enumerate() {
                match s {
                    vm::Step::Bg(n) => println!("  [{:>3}] bg    {}", i, n),
                    vm::Step::Movie(n) => println!("  [{:>3}] mv    {}", i, n),
                    vm::Step::Say { speaker, text, voice_line } => println!("  [{:>3}] say   {:?} (v={:?}): {}", i, speaker, voice_line, text),
                    vm::Step::Mono(t) => println!("  [{:>3}] mono  {}", i, t),
                    vm::Step::Choice(opts) => {
                        println!("  [{:>3}] choice {} options:", i, opts.len());
                        for (j, o) in opts.iter().enumerate() {
                            println!("         {}. {}", j + 1, o);
                        }
                    }
                    vm::Step::Jump { target, kind } => println!("  [{:>3}] jump  {} -> {}", i, kind, target),
                }
            }
        }
        _ => print_usage(),
    }
}

/// Resolve the output directory: explicit arg if present, else `<stem><suffix>`.
pub(crate) fn out_dir(pak: &str, args: &[String], explicit_idx: usize, suffix: &str) -> String {
    if args.len() > explicit_idx { return args[explicit_idx].clone(); }
    let p = Path::new(pak);
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "out".into());
    format!("{}{}", stem, suffix)
}

pub(crate) fn print_usage() {
    println!("wa2-native - native engine for White Album 2 (AVG32)");
    println!("usage:");
    println!("  wa2-native kcap <pak> [out]     unpack a KCAP data archive");
    println!("  wa2-native lac  <pak> [out]     unpack a LAC audio archive");
    println!("  wa2-native extract <pak> [out]  auto-detect and unpack KCAP/LAC");
    println!("  wa2-native tga <file.tga>       verify a TGA decodes");
    println!("  wa2-native sjis <file>          decode CP932 bytes to UTF-8");
    println!("  wa2-native text-info <txt>      parse a scenario text page");
    println!("  wa2-native scene <txt> <font> <charset> <bg.tga> <out.tga>  render a scene");
    println!("  wa2-native font-test <font.tga> <charset.txt> <out.tga> [text]  atlas glyph sheet");
    println!("  wa2-native font-sample <out.tga> [w] [h]  vector-font specimen sheet (WA2_FONT*)");
}
