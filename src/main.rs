mod archive;
mod artcache;
mod audio;
mod codec;
mod cg;
mod cmdbar;
mod data;
mod ft2;
mod menu;
mod render;
mod save;
mod saveload;
mod sdl2;
mod sha256;
mod timeline;
mod video;
mod vm;

use std::path::Path;

/// Chinese mode is only usable when some bundled or installed font can draw
/// Chinese. The design requires the game to refuse to start instead of
/// painting a screen of empty text; the headless CLIs stay available because
/// they are the tools used to diagnose a missing font.
fn require_chinese_font() {
    if !render::text::chinese_font_available() {
        eprintln!(
            "缺少中文字体：未找到可显示中文的字体，请把 NotoSansCJK-Medium.ttc 放入 fonts/ 或安装系统 CJK 字体"
        );
        std::process::exit(1);
    }
}

fn main() {
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

/// Load a scenario by id from `script_dir` (looks for `<id>.txt`).
fn load_scenario(script_dir: &std::path::Path, id: &str) -> Option<Vec<u8>> {
    let p = script_dir.join(format!("{}.txt", id));
    std::fs::read(&p).ok()
}

/// Try to load a background / UI TGA by name, searching `dirs` in order (the
/// Chinese override directory first, then the shared background directory).
/// Returns None if the file is missing or fails to decode (the caller keeps the
/// previous background).
///
/// WA2 scripts sometimes reference a suffixed variant (e.g. `v100200_bg.tga`)
/// while the archive contains the base name (`v100200.tga`). We try the exact
/// name first, then strip common suffixes.
/// ACT148 / ACT201 file. The exe's mode-1 sprintf is `v%06d.tga` of
/// `id * 10 + variant`: 10170 variant 0 is `v101700.tga`.
fn event_cg_name(id: i64, variant: u32) -> String {
    format!("v{:06}.tga", id * 10 + variant as i64)
}

/// Scene-switch profiling. `WA2_TIMING=1` prints the cost of each decode,
/// composite and upload so a stall can be attributed to one of them.
fn timing_enabled() -> bool {
    std::env::var_os("WA2_TIMING").is_some()
}

fn tlog(label: &str, t0: std::time::Instant) {
    if timing_enabled() {
        eprintln!("  [timing] {label}: {:.1} ms", t0.elapsed().as_secs_f64() * 1000.0);
    }
}

fn source_art_scale() -> u32 {
    std::env::var("WA2_ART_SCALE")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|&v| matches!(v, 1 | 2 | 4 | 8))
        .unwrap_or(1)
}

/// How the decode scale is chosen. Decided once, before any call writes a
/// number into `WA2_RENDER_SCALE`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ScaleMode {
    /// `WA2_RENDER_SCALE=auto`: decode only as much as the window can show.
    Window,
    /// Numeric `WA2_RENDER_SCALE`: exactly that scale.
    Forced(u32),
    /// No override: never below 4x, more only when the window needs it.
    Default,
}

fn scale_mode() -> ScaleMode {
    static MODE: std::sync::OnceLock<ScaleMode> = std::sync::OnceLock::new();
    *MODE.get_or_init(|| match std::env::var("WA2_RENDER_SCALE") {
        Ok(v) if v.eq_ignore_ascii_case("auto") => ScaleMode::Window,
        Ok(v) => match v.trim().parse::<u32>() {
            Ok(n) if matches!(n, 1 | 2 | 4 | 8) => ScaleMode::Forced(n),
            _ => ScaleMode::Default,
        },
        Err(_) => ScaleMode::Default,
    })
}

/// Decode scale wanted under the current mode.
///
/// Default: windowed decodes at 4x (the 4x tree is read directly), fullscreen
/// decodes at 8x (the master tree, even on a 1080p panel where the extra
/// pixels are supersampling). `WA2_RENDER_SCALE=auto` keeps the old
/// window-sized decoding, a number forces one scale.
fn decode_scale_for_window(win: &sdl2::Sdl) -> u32 {
    let (w, h) = win.window_size();
    match scale_mode() {
        ScaleMode::Forced(n) => n,
        ScaleMode::Window => render_scale_for(w, h),
        ScaleMode::Default => {
            if win.is_fullscreen() || win.is_maximized() {
                8
            } else {
                render_scale_for(w, h).max(4)
            }
        }
    }
}

/// How many pixels one decoded art asset stores per 1280x720 layout pixel.
fn render_art_scale() -> u32 {
    if let ScaleMode::Forced(n) = scale_mode() {
        return n;
    }
    std::env::var("WA2_RENDER_SCALE")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|&v| matches!(v, 1 | 2 | 4 | 8))
        .unwrap_or_else(source_art_scale)
}

/// Report and apply the decode scale for the current window.
fn set_render_art_scale(win: &sdl2::Sdl) {
    let (w, h) = win.window_size();
    let want = decode_scale_for_window(win);
    static REPORTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    REPORTED.get_or_init(|| {
        let why = match scale_mode() {
            ScaleMode::Forced(n) => format!("forced {}x by WA2_RENDER_SCALE", n),
            ScaleMode::Window => "auto: window-sized decoding".to_string(),
            ScaleMode::Default => {
                "default: windowed 4x / fullscreen or maximized 8x; WA2_RENDER_SCALE=auto for window-sized decoding"
                    .to_string()
            }
        };
        eprintln!(
            "art: source {}x, decode {}x for window {}x{} ({})",
            source_art_scale(),
            want,
            w,
            h,
            why
        );
    });
    let current = std::env::var("WA2_RENDER_SCALE")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok());
    if current != Some(want) {
        std::env::set_var("WA2_RENDER_SCALE", want.to_string());
        // Pictures decoded for the old scale are the wrong pixel size now;
        // dropping them keeps the compositor from mixing scales.
        artcache::drop_other_scales(want);
        eprintln!("art: render scale={} for window {}x{}", want, w, h);
    }
}

/// Largest useful art scale for a window: 1280x720 needs 1x, 1920x1080
/// needs 2x, 2560x1440/4K need 4x, anything larger 8x.
fn render_scale_for(w: i32, h: i32) -> u32 {
    let w = w.max(1280) as u32;
    let h = h.max(720) as u32;
    let need = ((w + 1279) / 1280).max((h + 719) / 720);
    if need <= 1 { 1 } else if need <= 2 { 2 } else if need <= 4 { 4 } else { 8 }
}

fn load_background(dirs: &[&std::path::Path], name: &str) -> Option<codec::tga::TgaImage> {
    load_background_asset(dirs, name).map(|asset| asset.img)
}

/// load_background_asset_inner with a WA2_TIMING profile line.
fn load_background_asset(dirs: &[&std::path::Path], name: &str) -> Option<LoadedAsset> {
    let t0 = std::time::Instant::now();
    let out = load_background_asset_inner(dirs, name);
    tlog(&format!("load {name}"), t0);
    out
}

/// Resolve the full-screen effect plate selected by an ACT146/147 mode.
/// The exe masks the high bit and formats the remaining value as `f0NNN.bmp`;
/// the scaled WebP siblings are picked by the normal art loader.
fn effect_name(mode: i32) -> Option<String> {
    (mode >= 0x80).then(|| format!("f0{:03}.bmp", mode & 0x7f))
}

fn background_override_name(id: i64, variant: u32, preset: i32) -> String {
    format!("b{:04}{}{}.amp", id, variant, preset.clamp(0, 9))
}

fn load_effect(dirs: &[&std::path::Path], mode: i32) -> Option<codec::tga::TgaImage> {
    let name = effect_name(mode)?;
    match load_background_asset(dirs, &name) {
        Some(asset) => {
            eprintln!("  effect: ACT146/147 mode {} -> {} ({}x)", mode, name, asset.src_scale);
            Some(asset.img)
        }
        None => {
            eprintln!("  effect: mode {} -> {} NOT FOUND", mode, name);
            None
        }
    }
}

/// Load a scene asset and retain the scale represented by the decoded pixels.
/// UI sheets are normalized by decode_ui_file, while art keeps the largest
/// useful scale allowed by the current render target.
fn load_background_asset_inner(dirs: &[&std::path::Path], name: &str) -> Option<LoadedAsset> {
    let t0 = std::time::Instant::now();
    let is_ui =
        name.starts_with("sys_") || name.starts_with("SYS_") || name.starts_with('t') || name.starts_with('T')
            || name.eq_ignore_ascii_case("weather.tga");
    let found = resolve_asset_path(dirs, name);
    tlog("resolve", t0);
    let t1 = std::time::Instant::now();
    // UI sheets are consumed at their 1280x720 logical size and the tool
    // pre-renders them under data/cache/ui, so they only need the decoder as a
    // fallback. Scene art goes through the derived cache: an 8x WebP is decoded
    // once and every later load of it is a file read or a memory hit.
    for (path, cp932) in found {
        let decoded = if is_ui {
            codec::image::decode_ui_file(&path).ok()
        } else {
            artcache::load(&path).map(|img| (*img).clone())
        };
        if let Some(img) = decoded {
            let (art_scale, src_scale) = if is_ui {
                (1, 1)
            } else {
                let (src, dec) = codec::image::art_scales(&path);
                (dec.max(1), src.max(1))
            };
            tlog(if cp932 { "decode (cp932 name)" } else { "decode" }, t1);
            return Some(LoadedAsset { img, art_scale, src_scale });
        }
    }
    None
}

/// Find the file backing one logical asset name without decoding anything.
///
/// Candidate names are tried against every tree with a plain existence check,
/// which is what the extracted data tree answers directly. Only when nothing
/// matches does it fall back to the CP932 name scan needed by trees extracted
/// before the extractor normalized filenames - and that scan is indexed once
/// per directory, because reading 1200 directory entries per candidate was
/// measurably slower than decoding the image afterwards.
///
/// Returns every existing file the name could mean, best first, each flagged
/// with whether it came from the CP932 fallback. The caller decodes them in
/// order, so a file that fails to decode still falls through to the next
/// candidate - which is what the old decode-as-you-search loop did.
fn resolve_asset_path(dirs: &[&std::path::Path], name: &str) -> Vec<(std::path::PathBuf, bool)> {
    let candidates = asset_candidates(name);
    let mut out = Vec::new();
    for dir in dirs {
        for cand in &candidates {
            let p = dir.join(cand);
            if p.is_file() {
                out.push((p, false));
            }
        }
    }
    #[cfg(unix)]
    {
        for dir in dirs {
            for (decoded, path) in cp932_entries(dir).iter() {
                for cand in &candidates {
                    if decoded.eq_ignore_ascii_case(cand) {
                        out.push((path.clone(), true));
                    }
                }
            }
        }
    }
    #[cfg(not(unix))]
    let _ = dirs;
    out
}

/// The names one logical asset may be stored under.
fn asset_candidates(name: &str) -> Vec<String> {
    let mut candidates = vec![name.to_string()];
    let lower = name.to_ascii_lowercase();
    for sfx in ["_bg", "_chr", "_chreff", "_ev", "_fg", "_eff"] {
        if lower.ends_with(sfx) {
            candidates.push(name[..name.len() - sfx.len()].to_string());
        }
    }
    // Each sibling tree keeps the same stem but may use WebP after upscale.
    // .bmp sources (backgrounds extracted from the archive) keep the same
    // stem in the WebP trees as well.
    let base = name
        .trim_end_matches(".tga")
        .trim_end_matches(".TGA")
        .trim_end_matches(".webp")
        .trim_end_matches(".WEBP")
        .trim_end_matches(".bmp")
        .trim_end_matches(".BMP");
    candidates.push(format!("{}.webp", base));
    candidates.push(format!("{}.WEBP", base));
    candidates.push(format!("{}.tga", base));
    candidates.push(format!("{}.TGA", base));
    candidates
}

/// Directory entries with their names CP932-decoded, indexed once per directory.
#[cfg(unix)]
fn cp932_entries(dir: &std::path::Path) -> std::sync::Arc<Vec<(String, std::path::PathBuf)>> {
    use std::collections::HashMap;
    use std::os::unix::ffi::OsStrExt;
    use std::sync::{Arc, Mutex, OnceLock};
    static IDX: OnceLock<Mutex<HashMap<std::path::PathBuf, Arc<Vec<(String, std::path::PathBuf)>>>>> =
        OnceLock::new();
    let mut m = match IDX.get_or_init(Default::default).lock() {
        Ok(m) => m,
        Err(e) => e.into_inner(),
    };
    if let Some(v) = m.get(dir) {
        return Arc::clone(v);
    }
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let raw = entry.file_name();
            v.push((codec::sjis::decode(raw.as_bytes()), entry.path()));
        }
    }
    let v = Arc::new(v);
    m.insert(dir.to_path_buf(), Arc::clone(&v));
    v
}
/// Exact voice lookup by the identifiers from ACT137 and ACT138.
struct VoiceIndex {
    map: std::collections::HashMap<(u32, u32, u32), std::path::PathBuf>,
}

impl VoiceIndex {
    fn build(dir: &std::path::Path) -> VoiceIndex {
        let t0 = std::time::Instant::now();
        let mut map = std::collections::HashMap::new();
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let Some((stem, ext)) = name.rsplit_once('.') else { continue };
                if !ext.eq_ignore_ascii_case("ogg") { continue; }
                let parts: Vec<_> = stem.split('_').collect();
                if parts.len() != 3 { continue; }
                if let (Ok(scene), Ok(line), Ok(character)) = (parts[0].parse(), parts[1].parse(), parts[2].parse()) {
                    map.insert((scene, line, character), e.path());
                }
            }
        }
        tlog("voice index", t0);
        eprintln!("voice: indexed {} clips from {}", map.len(), dir.display());
        VoiceIndex { map }
    }

    fn find(&self, cue: vm::lscr::VoiceCue) -> Option<&std::path::Path> {
        self.map.get(&(cue.scenario, cue.line, cue.char_id)).map(|p| p.as_path())
    }
}

fn stop_voice(mx: &audio::Mixer, voice_stream: &mut Option<usize>) {
    if let Some(slot) = voice_stream.take() {
        mx.stop(slot);
    }
}

/// Stop all scene-owned audio and clear any modal-screen streams that may not
/// have a scene slot. The latter matters when returning Home and immediately
/// starting another game: the title/music screen and the game share one SDL
/// mixer, so leaving an untracked BGM slot alive makes the next scene sound
/// like another interface.
fn stop_game_audio(
    mixer: Option<&audio::Mixer>,
    bgm_track: &mut Option<(i32, usize)>,
    fading_bgm: &mut Option<usize>,
    voice_stream: &mut Option<usize>,
    se_channels: &mut std::collections::HashMap<i32, usize>,
    fading_se: &mut std::collections::HashMap<i32, usize>,
) {
    let Some(mx) = mixer else { return };
    stop_voice(mx, voice_stream);
    if let Some((_, slot)) = bgm_track.take() {
        mx.stop(slot);
    }
    if let Some(slot) = fading_bgm.take() {
        mx.stop(slot);
    }
    for slot in se_channels.drain().map(|(_, slot)| slot) {
        mx.stop(slot);
    }
    for slot in fading_se.drain().map(|(_, slot)| slot) {
        mx.stop(slot);
    }
    // Catch title/music streams whose owner was already dropped or whose slot
    // was never copied into the scene's bookkeeping.
    mx.stop_category(audio::mixer::CAT_BGM);
    mx.stop_category(audio::mixer::CAT_SE);
    mx.stop_category(audio::mixer::CAT_VOICE);
}

/// Scenario ids listed by the original sample-play table (`9999.bnr`).
/// The table is deliberately explicit: ordinary scenes must never inherit the
/// H-scene voice rule just because they happen to show an event CG.
const H_SCENE_IDS: [&str; 18] = [
    "1012_030_2", "2031", "2312", "2313", "2401", "2402", "2504", "2507",
    "2511", "2513", "3001", "3015", "3210", "3901", "3902", "3904", "3906", "3907",
];

fn is_h_scene_id(scene: &str) -> bool {
    H_SCENE_IDS.iter().any(|candidate| *candidate == scene)
}

/// Character ids that the settings page classifies as female. Unknown ids are
/// treated as male for the female-only H-scene mode, which avoids accidentally
/// playing an unclassified male voice.
fn is_female_voice_char(char_id: u32) -> bool {
    matches!(char_id, 1 | 2 | 3 | 4 | 5 | 16 | 19 | 23 | 24 | 25)
}

fn h_scene_voice_allowed(settings: &menu::Settings, scene: &str, char_id: u32) -> bool {
    if !is_h_scene_id(scene) {
        return true;
    }
    match settings.ero_voice {
        0 => false,
        1 => true,
        2 => is_female_voice_char(char_id),
        _ => true,
    }
}

/// Start one line's voice. A new playable clip always replaces the previous
/// clip; the page-voice setting only controls pages that have no new clip.
fn play_voice(
    mx: &audio::Mixer,
    voices: &VoiceIndex,
    settings: &menu::Settings,
    voice_stream: &mut Option<usize>,
    cue: vm::lscr::VoiceCue,
    scene: &str,
) {
    let muted = menu::char_row_of(cue.char_id)
        .map(|row| settings.char_voice[row] == 0)
        .unwrap_or(false);
    if muted {
        return;
    }
    if !h_scene_voice_allowed(settings, scene, cue.char_id) {
        eprintln!("  voice: H-scene rule filtered {}_{:04}_{:02}", cue.scenario, cue.line, cue.char_id);
        return;
    }
    let Some(vf) = voices.find(cue) else {
        eprintln!(
            "  voice: MISSING {}_{:04}_{:02}.OGG",
            cue.scenario, cue.line, cue.char_id
        );
        return;
    };
    // Stop only after all filters and the file lookup succeed. A muted,
    // filtered, or missing new cue is not a replacement, so an older clip is
    // allowed to finish when the setting says not to interrupt empty pages.
    stop_voice(mx, voice_stream);
    eprintln!("  voice: {}", vf.display());
    let slot = mx.play_cat(&vf.to_string_lossy(), 1.0, false, crate::audio::mixer::CAT_VOICE);
    if slot != usize::MAX {
        *voice_stream = Some(slot);
    }
}

/// Load one of the game's `.AMP` colour curves by name (case-insensitive).
fn load_tint(dirs: &[&std::path::Path], name: &str) -> Option<Vec<u8>> {
    let want = name.to_ascii_lowercase();
    for dir in dirs {
        // exact first, then a case-insensitive scan (sepia.AMP vs sepia.amp)
        if let Ok(d) = std::fs::read(dir.join(name)) {
            return Some(d);
        }
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                if let Ok(n) = e.file_name().into_string() {
                    if n.to_ascii_lowercase() == want {
                        if let Ok(d) = std::fs::read(e.path()) {
                            return Some(d);
                        }
                    }
                }
            }
        }
    }
    None
}

/// The colour curve actually in force: an explicit ACT172 filter wins over
/// the per-background `.amp` override, which wins over the ACT170 preset.
fn effective_tint_name(
    filter: &Option<String>,
    ambient_override: &Option<String>,
    ambient_preset: &Option<String>,
) -> Option<String> {
    filter
        .clone()
        .or_else(|| ambient_override.clone())
        .or_else(|| ambient_preset.clone())
}

fn ambient_preset_name(preset: i32) -> Option<String> {
    match preset.clamp(0, 3) {
        1 => Some("evening.amp".to_string()),
        2 => Some("night.amp".to_string()),
        3 => Some("indoor.amp".to_string()),
        _ => None,
    }
}

/// Decode the effective curve, logging what happened (the old inline logs).
fn load_tint_logged(dirs: &[&std::path::Path], name: &Option<String>) -> Option<Vec<u8>> {
    match name.as_deref() {
        None => {
            eprintln!("  tint: <none>");
            None
        }
        Some(n) => match load_tint(dirs, n) {
            Some(lut) => {
                eprintln!("  tint: {} ({} bytes)", n, lut.len());
                Some(lut)
            }
            None => {
                eprintln!("  tint: {} NOT FOUND", n);
                None
            }
        },
    }
}

/// Build a solid-color fallback background image.
fn solid_bg(w: u32, h: u32, color: [u8; 3]) -> codec::tga::TgaImage {
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..(w * h) {
        rgba.extend_from_slice(&[color[0], color[1], color[2], 255]);
    }
    codec::tga::TgaImage { width: w, height: h, rgba }
}

/// Standing-character stage slots. K16's first value is the slot: 0/1/2 are
/// the usual left/centre/right, 3/4 stand further out, and 7–10 are the back
/// row used in group shots. Clamping everything above 2 onto the right slot
/// stacked people who should have been standing apart.
const SPRITE_SLOTS: usize = 11;
/// Horizontal centre of each slot, in 1280-wide screen coordinates.
const SPRITE_ANCHORS: [i32; SPRITE_SLOTS] = [
    320, 640, 960, // 0 left, 1 centre, 2 right
    140, 1140, // 3 far left, 4 far right
    640, 640, // 5, 6 (unused by the scripts)
    220, 500, 780, 1060, // 7–10 back row
];
/// Paint the back row and the outer pair before the front three, so a group
/// shot doesn't hide the people standing behind.
const SPRITE_DRAW_ORDER: [usize; SPRITE_SLOTS] = [7, 8, 9, 10, 3, 4, 5, 6, 0, 1, 2];

/// Background file index for the ACT146/147 ids.
///
/// Files are named `b{id:04}{variant:02}.{tga,webp}` (id 1004 variant 0 ->
/// b100400, id 1014 variant 10 -> b101410). Not every variant was shipped:
/// for id 2067 only b206702 / b206712 exist, while the scripts ask for
/// variant 0 or 1. The fallback must be another *background* variant of the
/// same id.
///
/// A `v{id:04}{variant:02}` file is an event CG whose six digits happen to
/// equal a background id (v206700 is an event CG, b206702 is the concert
/// hall). It may stand in for a background only when no `b*` was shipped for
/// that id at all. The previous index let the readdir order decide between
/// the two, so some backgrounds showed an unrelated event CG.
struct BgIndex {
    by_name: std::collections::HashMap<String, String>,
    /// Background id -> its `b*` files, sorted lowest variant first.
    bgs: std::collections::HashMap<u32, Vec<(u32, String)>>,
    /// Background id -> `v*` event-CG stand-ins, used only when `bgs` has no
    /// entry for the id.
    cgs: std::collections::HashMap<u32, Vec<(u32, String)>>,
}

impl BgIndex {
    fn build(dirs: &[&std::path::Path]) -> BgIndex {
        let mut by_name: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        let mut bgs: std::collections::HashMap<u32, Vec<(u32, String)>> = std::collections::HashMap::new();
        let mut cgs: std::collections::HashMap<u32, Vec<(u32, String)>> = std::collections::HashMap::new();
        // The first directory that has a file wins, like the search order the
        // loaders use. Within one variant, keep the first file seen; the
        // per-id list is sorted afterwards so the fallback never depends on
        // the order of readdir (that used to make the chart differ between
        // filesystems).
        let mut seen_b: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
        let mut seen_v: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
        for dir in dirs {
            let rd = match std::fs::read_dir(dir) {
                Ok(r) => r,
                Err(_) => continue,
            };
            for e in rd.flatten() {
                let name = match e.file_name().into_string() {
                    Ok(n) => n,
                    Err(_) => continue,
                };
                let lower = name.to_ascii_lowercase();
                let picture = (lower.starts_with('b') || lower.starts_with('v'))
                    && (lower.ends_with(".tga") || lower.ends_with(".webp"));
                if !picture {
                    continue;
                }
                let ext_len = if lower.ends_with(".webp") { 5 } else { 4 };
                let digits = &lower[1..lower.len() - ext_len];
                if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }
                let Ok(id) = digits[..4].parse::<u32>() else { continue };
                let Ok(variant) = digits[4..].parse::<u32>() else { continue };
                let is_background = lower.starts_with('b');
                by_name.entry(lower).or_insert_with(|| name.clone());
                if is_background {
                    if seen_b.insert((id, variant)) {
                        bgs.entry(id).or_default().push((variant, name));
                    }
                } else if seen_v.insert((id, variant)) {
                    cgs.entry(id).or_default().push((variant, name));
                }
            }
        }
        for list in bgs.values_mut().chain(cgs.values_mut()) {
            list.sort_by(|a, b| a.0.cmp(&b.0));
        }
        BgIndex { by_name, bgs, cgs }
    }

    /// Resolve one ACT146/147 background request.
    ///
    /// 1. exact `b{id}{variant}`;
    /// 2. another `b` variant of the same id (lowest variant, deterministic);
    /// 3. exact `v{id}{variant}` - an event CG standing in for a background
    ///    that was never shipped as `b*`;
    /// 4. another `v` variant of the id.
    ///
    /// Event CGs (ACT148) do not use this. Their file is `v{id*10+variant:06}`.
    fn resolve(&self, id: i64, variant: u32) -> Option<String> {
        if id <= 0 {
            return None;
        }
        let id = id as u32;
        let exact_b = |v: u32| -> Option<String> {
            for ext in [".webp", ".tga"] {
                let name = format!("b{id:04}{v:02}{ext}");
                if let Some(n) = self.by_name.get(&name) {
                    return Some(n.clone());
                }
            }
            None
        };
        if let Some(name) = exact_b(variant) {
            return Some(name);
        }
        if let Some(list) = self.bgs.get(&id) {
            // A variant that was not shipped keeps its background family: the
            // lowest variant stands in, so b206702 wins over the v206700 CG.
            return list.first().map(|(_, n)| n.clone());
        }
        for ext in [".webp", ".tga"] {
            let name = format!("v{id:04}{variant:02}{ext}");
            if let Some(n) = self.by_name.get(&name) {
                return Some(n.clone());
            }
        }
        self.cgs.get(&id).and_then(|l| l.first()).map(|(_, n)| n.clone())
    }
}

/// Pictures a scenario will ask for later, in program order.
///
/// Scanning the bytecode is cheap and lets the prefetch thread decode the next
/// backgrounds and sprites while the player is still reading the line on
/// screen, which is what removes the wait at a scene change. The scan mirrors
/// the interpreter record handling: K30 with three ints followed by a K16
/// ACT154/155 is a sprite, ACT146/147 a background, ACT148/201 a CG.
fn upcoming_art(
    p: &vm::lscr::Program,
    bg: &BgIndex,
    bank: Option<&SpriteBank>,
) -> (Vec<String>, Vec<String>) {
    use vm::lscr::Op;
    let mut bgs: Vec<String> = Vec::new();
    let mut sprites: Vec<String> = Vec::new();
    let mut ints: Vec<i32> = Vec::new();
    let (mut tok_hit, mut act_hit) = (false, false);
    let mut pending: Option<(u32, u32, u32)> = None;
    let mut pending_tok: Option<u32> = None;
    for (i, op) in p.ops.iter().enumerate() {
        match op {
            Op::Rec { kind: k, .. } => {
                if *k == 30 && !tok_hit && !act_hit && ints.len() == 3 {
                    let cid = ints[0];
                    let variant = ints[1];
                    let flag = ints[2];
                    if (0..40).contains(&cid)
                        && (0..1000).contains(&variant)
                        && flag % 1000 == 0
                        && (1000..=30000).contains(&flag)
                        && p.ops[i..]
                            .iter()
                            .take(10)
                            .any(|o| matches!(o, Op::Act { id, .. } if *id == 154 || *id == 155))
                    {
                        pending = Some((cid as u32, (flag / 1000) as u32, variant as u32));
                    }
                }
                ints.clear();
                tok_hit = false;
                act_hit = false;
                pending_tok = None;
            }
            Op::Tok { n, .. } => {
                tok_hit = true;
                // ACT176 stores [image name, "", directory]. The interpreter
                // resolves the first token, so the prefetch scanner must not
                // overwrite it with the directory token that follows.
                if pending_tok.is_none() {
                    pending_tok = Some(*n);
                }
            }
            Op::Int { v, .. } => ints.push(*v),
            Op::Act { id, .. } => {
                act_hit = true;
                match *id {
                    146 | 147 => {
                        if let Some(n) = ints.first().and_then(|mode| effect_name(*mode)) {
                            if !bgs.contains(&n) {
                                bgs.push(n);
                            }
                        }
                        if ints.len() >= 4 && ints[1] > 0 {
                            if let Some(n) = bg.resolve(ints[1] as i64, ints[2].max(0) as u32) {
                                if !bgs.contains(&n) {
                                    bgs.push(n);
                                }
                            }
                        }
                    }
                    176 => {
                        if let Some(tok) = pending_tok.and_then(|n| p.token(n)) {
                            if !bgs.contains(&tok.to_string()) {
                                bgs.push(tok.to_string());
                            }
                        }
                    }
                    148 => {
                        if ints.len() >= 4 && ints[1] > 0 {
                            let n = event_cg_name(ints[1] as i64, ints[2].max(0) as u32);
                            if !bgs.contains(&n) {
                                bgs.push(n);
                            }
                        }
                    }
                    201 => {
                        if ints.len() >= 3 && ints[0] > 0 && ints[2] != 0 {
                            let n = event_cg_name(ints[0] as i64, ints[1].max(0) as u32);
                            if !bgs.contains(&n) {
                                bgs.push(n);
                            }
                        }
                    }
                    154 | 155 => {
                        if let (Some((cid, series, variant)), Some(bank)) = (pending.take(), bank) {
                            if let Some(n) = bank.resolve(cid, series, variant) {
                                if !sprites.contains(&n) {
                                    sprites.push(n);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    (bgs, sprites)
}

/// How many pictures of each kind to queue ahead of the script.
const PREFETCH_BG: usize = 6;
const PREFETCH_SPRITE: usize = 8;

/// Turn the names the script will ask for into file paths for the prefetch
/// thread. Missing files are dropped here rather than in the worker.
fn prefetch_paths(
    dirs: &[&std::path::Path],
    bgs: &[String],
    sprite_dir: Option<&std::path::Path>,
    sprites: &[String],
) -> Vec<std::path::PathBuf> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for name in bgs.iter().take(PREFETCH_BG) {
        if let Some((p, _)) = resolve_asset_path(dirs, name).into_iter().next() {
            if seen.insert(p.clone()) {
                out.push(p);
            }
        }
    }
    if let Some(dir) = sprite_dir {
        for name in sprites.iter().take(PREFETCH_SPRITE) {
            let p = dir.join(name);
            if p.is_file() && seen.insert(p.clone()) {
                out.push(p);
            }
        }
    }
    out
}

/// Where every kind of media comes from for one play session.
///
/// Built either from the loose-file data tree (`Session::from_data`) or from
/// explicit CLI paths (legacy debugging form). Everything the engine loads at
/// runtime goes through here - no `.pak` archive is opened.
struct Session {
    script_dir: std::path::PathBuf,
    /// True when `script_dir` is the CK-GAL Chinese patch: its text needs the
    /// glyph-table reading (see `codec::sjis::decode_cn`).
    cn: bool,
    /// Scaled art trees, `(scale, dir)` ascending. Both the 4x tree and the
    /// 8x master are kept when present: the decode scale decides which one is
    /// read first, so entering fullscreen switches to the 8x master without
    /// rebuilding the session.
    bg_trees: Vec<(u32, std::path::PathBuf)>,
    /// Chinese-patch UI overrides - the 31 images shipped loose in the game's
    /// `grp/` directory, which win over the same-named files inside `grp.pak`.
    /// Consulted before `bg_trees`; empty in Japanese mode.
    bg_cn_trees: Vec<(u32, std::path::PathBuf)>,
    /// Original Chinese UI tree used as a per-file fallback while the
    /// optional high-resolution sibling is still being generated.
    bg_cn_original: Option<std::path::PathBuf>,
    /// Original background tree used when the selected high-resolution file
    /// is not present yet.
    bg_original: Option<std::path::PathBuf>,
    /// Scaled sprite trees, ascending; empty disables sprites.
    sprite_trees: Vec<(u32, std::path::PathBuf)>,
    /// Movies as plain `*.wmv` / `*.asf` files.
    movie_dir: std::path::PathBuf,
    voice_dir: Option<std::path::PathBuf>,
    bgm: Option<String>,
    /// ACT146/147 background id -> file index.
    bg_index: BgIndex,
}

impl Session {
    fn from_data(d: &data::Data, bgm: Option<String>) -> Session {
        // Keep the scaled sibling trees, not just the one Data picked: the
        // decode scale changes at runtime (fullscreen asks for the 8x master)
        // and the loader then just reorders these directories.
        let mut bg_trees = vec![(d.art_scale, d.bg())];
        let mut sprite_trees = vec![(d.art_scale, d.sprite())];
        let mut bg_cn_trees = if d.cn {
            vec![(d.art_scale, d.bg_cn())]
        } else {
            Vec::new()
        };
        // The default source is the 4x tree; keep the 2x proxy and 8x master
        // next to it so previews can use 2x in a window and 4x in fullscreen.
        // The scene renderer still chooses its normal 4x/8x path separately.
        if d.art_scale == 4 {
            let bg2 = d.root.join("bg_2x");
            let sp2 = d.root.join("sprite_2x");
            let cn2 = d.root.join("bg_cn_2x");
            if bg2.is_dir() {
                bg_trees.push((2, bg2));
                if sp2.is_dir() {
                    sprite_trees.push((2, sp2));
                }
                if d.cn && cn2.is_dir() {
                    bg_cn_trees.push((2, cn2));
                }
            }
            let bg8 = d.root.join("bg_8x");
            let sp8 = d.root.join("sprite_8x");
            let cn8 = d.root.join("bg_cn_8x");
            if bg8.is_dir() {
                bg_trees.push((8, bg8));
                if sp8.is_dir() {
                    sprite_trees.push((8, sp8));
                }
                if d.cn && cn8.is_dir() {
                    bg_cn_trees.push((8, cn8));
                }
            }
        }
        bg_trees.sort_by_key(|(scale, _)| *scale);
        sprite_trees.sort_by_key(|(scale, _)| *scale);
        bg_cn_trees.sort_by_key(|(scale, _)| *scale);
        let bg_original = d.root.join("bg");
        let bg_cn_original = if d.cn { Some(d.root.join("bg_cn")) } else { None };
        let mut index_dirs: Vec<&std::path::Path> = Vec::new();
        for (_, p) in &bg_cn_trees {
            index_dirs.push(p.as_path());
        }
        if let Some(ref p) = bg_cn_original {
            index_dirs.push(p.as_path());
        }
        for (_, p) in &bg_trees {
            index_dirs.push(p.as_path());
        }
        index_dirs.push(bg_original.as_path());
        let bg_index = BgIndex::build(&index_dirs);
        Session {
            script_dir: d.script(),
            cn: d.cn,
            bg_trees,
            bg_cn_trees,
            bg_cn_original,
            bg_original: Some(bg_original),
            sprite_trees,
            movie_dir: d.movie(),
            voice_dir: Some(d.voice()),
            bgm,
            bg_index,
        }
    }

    /// Directories searched for images, in priority order.
    ///
    /// The scaled trees are ordered for the current decode scale: a 4x decode
    /// reads the 4x tree first (about four times faster than shrinking the 8x
    /// file), an 8x decode reads the 8x master. The 1280x720 originals stay as
    /// the last fallback for files the scaled trees are missing.
    fn bg_dirs(&self) -> Vec<&std::path::Path> {
        let want = render_art_scale();
        let mut v = Vec::new();
        v.extend(ordered_tree_paths(&self.bg_cn_trees, want));
        v.extend(ordered_tree_paths(&self.bg_trees, want));
        if let Some(cn) = self.bg_cn_original.as_deref() {
            v.push(cn);
        }
        if let Some(original) = self.bg_original.as_deref() {
            v.push(original);
        }
        v
    }

    /// Sprite directory for the current decode scale, if any.
    fn sprite_dir_now(&self) -> Option<&std::path::Path> {
        ordered_tree_paths(&self.sprite_trees, render_art_scale())
            .into_iter()
            .next()
    }
}

/// Order scaled trees for a decode target: exact match first, then the next
/// larger, then the smaller ones.
fn ordered_tree_paths<'a>(
    trees: &'a [(u32, std::path::PathBuf)],
    want: u32,
) -> Vec<&'a std::path::Path> {
    let mut order: Vec<usize> = (0..trees.len()).collect();
    order.sort_by_key(|&i| {
        let scale = trees[i].0;
        if scale == want {
            0
        } else if scale > want {
            1 + (scale - want)
        } else {
            100 + (want - scale)
        }
    });
    order.into_iter().map(|i| trees[i].1.as_path()).collect()
}

#[cfg(test)]
mod art_tree_tests {
    use super::ordered_tree_paths;
    use std::path::PathBuf;

    #[test]
    fn ordered_trees_prefer_the_decode_scale() {
        let trees = vec![(4u32, PathBuf::from("bg_4x")), (8, PathBuf::from("bg_8x"))];
        let names = |want: u32| -> Vec<String> {
            ordered_tree_paths(&trees, want)
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect()
        };
        // Windowed decodes at 4x and must read the 4x tree first.
        assert_eq!(names(4), ["bg_4x", "bg_8x"]);
        // Fullscreen decodes at 8x and must read the master first.
        assert_eq!(names(8), ["bg_8x", "bg_4x"]);
        // A lower target takes the next larger tree, then the smaller one.
        assert_eq!(names(2), ["bg_4x", "bg_8x"]);
    }
}

#[cfg(test)]
mod prefetch_scan_tests {
    use super::{prefetch_paths, upcoming_art, BgIndex};
    use crate::vm::lscr::{Op, Program};
    use std::collections::HashMap;

    #[test]
    fn upcoming_art_includes_effect_and_overlay_assets() {
        let program = Program {
            ops: vec![
                Op::Rec { off: 0, kind: 30 },
                Op::Int { off: 8, v: 0xA1 },
                Op::Int { off: 20, v: 2001 },
                Op::Int { off: 32, v: 0 },
                Op::Int { off: 44, v: 60 },
                Op::Act { off: 56, id: 146 },
                Op::Rec { off: 64, kind: 30 },
                Op::Tok { off: 72, n: 0 },
                Op::Tok { off: 80, n: 1 },
                Op::Tok { off: 88, n: 2 },
                Op::Int { off: 96, v: 0 },
                Op::Act { off: 108, id: 176 },
            ],
            index: HashMap::new(),
            labels: Vec::new(),
            tokens: vec!["ギター.tga".to_string(), "".to_string(), "grp".to_string()],
            entry: 0,
        };
        let index = BgIndex::build(&[]);
        let (bgs, sprites) = upcoming_art(&program, &index, None);
        assert!(bgs.iter().any(|n| n == "f0033.bmp"), "f0 effect was not queued: {bgs:?}");
        assert!(bgs.iter().any(|n| n == "ギター.tga"), "overlay was not queued: {bgs:?}");
        assert!(sprites.is_empty());
    }

    #[test]
    fn prefetch_paths_deduplicates_shared_assets() {
        let dir = std::env::temp_dir().join(format!("wa2-prefetch-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("f0033.bmp"), b"test").unwrap();
        let paths = prefetch_paths(
            &[dir.as_path()],
            &["f0033.bmp".to_string(), "f0033.bmp".to_string()],
            None,
            &[],
        );
        assert_eq!(paths, vec![dir.join("f0033.bmp")]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod sprite_bank_tests {
    use super::SpriteBank;

    /// The bank is built for the windowed tree and must move to the 8x tree
    /// when the decode scale grows (fullscreen), dropping the old pixels.
    #[test]
    fn sprite_bank_retargets_between_scaled_trees() {
        let tmp = std::env::temp_dir().join(format!("wa2-spritebank-{}", std::process::id()));
        let four = tmp.join("sprite_4x");
        let eight = tmp.join("sprite_8x");
        std::fs::create_dir_all(&four).unwrap();
        std::fs::create_dir_all(&eight).unwrap();
        std::fs::write(four.join("a.tga"), b"x").unwrap();
        std::fs::write(eight.join("b.webp"), b"x").unwrap();
        let mut bank = SpriteBank::new(vec![(4, four.clone()), (8, eight.clone())]);
        bank.retarget(4);
        assert_eq!(bank.dir, four);
        assert!(bank.index.contains("a.tga"));
        bank.retarget(8);
        assert_eq!(bank.dir, eight);
        assert!(bank.index.contains("b.webp"));
        assert!(bank.cache.is_empty(), "sprites from the old scale must be dropped");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

/// Everything that outlives a single scene.
///
/// Quick-loading a save used to rebuild all of this inside run_game_at: the
/// audio device was closed and reopened, the 46k-clip voice directory was
/// scanned again, and the UI sheets were decoded again. Keeping them here means
/// a scene change only replaces what the scene actually owns.
pub struct Runtime {
    /// Vector font for Chinese dialogue (None in Japanese mode).
    vfont: Option<render::text::VectorFont>,
    /// Profile id the dialogue font was opened from. The title screen can
    /// change the Game role while this Runtime is already alive, so the first
    /// scene of a game started from the title has to notice the mismatch and
    /// rebuild the instance (F-01).
    game_font_id: i32,
    bar: cmdbar::CmdBar,
    msg_plate: Option<codec::tga::TgaImage>,
    msg_frame: Option<codec::tga::TgaImage>,
    voices: Option<VoiceIndex>,
    mixer: Option<audio::Mixer>,
    bank: Option<SpriteBank>,
    /// Story-wide variables shared by every scene in this play session.
    pub(crate) global: vm::global::GlobalState,
}

impl Runtime {
    pub fn new(win: &sdl2::Sdl, s: &Session) -> Runtime {
        // The dialogue font is the Game role: the settings page can move it
        // without touching the menu or the history. The saved ids have to be
        // in force first - a run started straight from the command line
        // reaches this point without ever loading the settings.
        menu::Settings::load().apply_text();
        // Which id this instance belongs to: run_game_at compares it with the
        // id in force when a game starts, so a font picked on the title screen
        // reaches the first line instead of the second run.
        let game_font_id = render::text::role_id(render::text::FontRole::Game);
        let vfont = if s.cn {
            render::text::open_role_font(render::text::FontRole::Game, render::DIALOG_FONT_720)
        } else {
            None
        };
        if let Some(vf) = vfont.as_ref() {
            // Which face of the .ttc actually opened, and with what knobs: the
            // file name alone does not say, and a .ttc defaults to its first
            // (Japanese) face.
            eprintln!("font: {}", vf.describe());
        }
        if s.cn && vfont.is_none() {
            eprintln!("dialog: no vector font, falling back to the bitmap atlas");
        }
        let bar = cmdbar::CmdBar::load(&s.bg_dirs());
        let msg_plate = load_background(&s.bg_dirs(), "sys_00001.tga");
        let msg_frame = load_background(&s.bg_dirs(), "sys_00000.tga");
        let voices = s.voice_dir.as_deref().map(VoiceIndex::build);
        let wants_audio = s.bgm.is_some()
            || s.voice_dir.is_some()
            || s.script_dir
                .parent()
                .map(|p| p.join("bgm").is_dir() || p.join("se").is_dir())
                .unwrap_or(false);
        let mixer = if wants_audio {
            match audio::Mixer::new(win) {
                Some(mx) => {
                    if let Some(bgm) = s.bgm.as_deref() {
                        let _ = mx.play(bgm, 0.6, true);
                        eprintln!("BGM: {}", bgm);
                    }
                    Some(mx)
                }
                None => None,
            }
        } else {
            None
        };
        let bank = if s.sprite_trees.is_empty() {
            None
        } else {
            for (scale, dir) in &s.sprite_trees {
                if dir.is_dir() {
                    eprintln!("sprites: {} ({}x)", dir.display(), scale);
                }
            }
            Some(SpriteBank::new(s.sprite_trees.clone()))
        };
        if bank.is_none() {
            eprintln!("note: no sprite directory - character sprites disabled");
        }
        let global = vm::global::GlobalState::load(&s.script_dir);
        Runtime { vfont, game_font_id, bar, msg_plate, msg_frame, voices, mixer, bank, global }
    }
}


/// Full game loop: play scenes starting from `start_id`, following CATCH jumps
/// and presenting choices to the player.
/// Play from the start of a scene.
pub fn run_game(
    win: &mut sdl2::Sdl,
    font: &render::Font,
    s: &Session,
    start_id: &str,
    rt: &mut Runtime,
) -> Result<(), String> {
    // Starting a new game means starting a new global story state.  This is
    // done once outside the reload loop so loading a slot from the command bar
    // does not accidentally reset the flags again.
    rt.global.reset();
    // Loop so that loading a save slot from the command bar restarts the run
    // at the loaded position instead of ending the game.
    let mut resume: Option<save::SaveState> = None;
    loop {
        match run_game_at(win, font, s, resume, start_id, rt)? {
            None => return Ok(()),
            Some(snap) => {
                rt.global.restore_values(&snap.globals);
                resume = Some(snap);
            }
        }
    }
}

/// Resume from a save slot.
pub fn run_game_from(
    win: &mut sdl2::Sdl,
    font: &render::Font,
    s: &Session,
    snap: save::SaveState,
    rt: &mut Runtime,
) -> Result<(), String> {
    rt.global.restore_values(&snap.globals);
    let mut resume = Some(snap);
    let scene = resume.as_ref().unwrap().scene.clone();
    loop {
        match run_game_at(win, font, s, resume, &scene, rt)? {
            None => return Ok(()),
            Some(st) => {
                rt.global.restore_values(&st.globals);
                resume = Some(st);
            }
        }
    }
}
/// Whether the Runtime's dialogue font has to be rebuilt because the Game
/// role now points at a different profile (F-01). Japanese mode has no vector
/// dialogue font, so it never rebuilds.
fn game_font_is_stale(cn: bool, open_id: i32, want_id: i32) -> bool {
    cn && open_id != want_id
}

fn run_game_at(
    win: &mut sdl2::Sdl,
    font: &render::Font,
    s: &Session,
    resume: Option<save::SaveState>,
    start_id: &str,
    rt: &mut Runtime,
) -> Result<Option<save::SaveState>, String> {
    set_render_art_scale(win);
    let mut resume = resume;
    if resume.is_none() {
        reset_script_window_alpha();
    }
    let script_dir = s.script_dir.as_path();
    let bg_dirs = s.bg_dirs();
    let ui = UiSound::resolve(&bg_dirs);
    const W: u32 = 1280;
    const H: u32 = 720;
    // The art is laid out at 1280x720 and stretched to the window; the
    // dialogue is rasterized at the window's own size so it stays sharp.
    let mut win_size = win.window_size();
    // The command bar, the message-window sheets, the voice index, the mixer
    // and the sprite bank all live in the Runtime, so a quick load replaces
    // the scene state instead of the whole engine. The dialogue font is
    // borrowed after the settings below are applied (it may need replacing).
    let bar = &rt.bar;
    let msg_plate = &rt.msg_plate;
    let msg_frame = &rt.msg_frame;
    let mut cmd_auto = false;
    let mut cmd_skip = false;
    // Lines already shown, for the 履历 (backlog) screen.
    let mut backlog: Vec<String> = Vec::new();
    // One snapshot per line the player has been shown, newest last. Rewind
    // pops the line on screen and puts the previous one back, picture included.
    let mut history: Vec<LineSnap> = Vec::new();
    let mut rewind_left: i32 = 0;
    let mut cmd_rewind = false;
    let mut line_voice: Option<vm::lscr::VoiceCue> = None;
    // Set when rewind restored a line whose script is not the one loaded now.
    let mut park_wait = false;
    let mut rewind_vm: Option<vm::lscr::InterpState> = None;
    let mut current_id = start_id.to_string();
    // Black until the script sets one: WA2 opens on a black screen (the IC
    // prologue's "sepia.AMP" is a color-filter LUT, not a background image).
    let mut bg = solid_bg(W, H, [0, 0, 0]);
    // Full-screen f0NNN effect selected by the current ACT146/147 K30 mode.
    // It is separate from the background and survives until the next scene
    // background command clears or replaces it.
    let mut effect_mode: i32 = 0;
    let mut effect: Option<codec::tga::TgaImage> = None;
    // Decode scale of the pixels in `bg`; `None` while the screen is a solid
    // colour. A fullscreen switch changes the wanted scale, which tells the
    // loop to re-decode the picture at the new one.
    let mut bg_scale: Option<u32> = None;
    // Source tree the background was read from (4/8 for bg_4x/bg_8x, 1 for the
    // original TGA tree). Printed with the scene line.
    let mut bg_src_scale: Option<u32> = None;

    let mixer = &rt.mixer;

    // WA2_AUTO       : advance text *and* pick choices automatically (unattended runs)
    // WA2_AUTO_TEXT  : advance text only, stop at choices (interactive verification)
    let auto_pick = std::env::var_os("WA2_AUTO").is_some();
    let auto_text = auto_pick || std::env::var_os("WA2_AUTO_TEXT").is_some();

    // Indexed once by the Runtime: the directory holds 46k clips.
    let voices = &rt.voices;
    // Player settings (option screen / wa2.ini).
    let mut settings = menu::Settings::load();
    // A debug jump (`run <scene>`) starts from the saved 画面模式 too, so the
    // decode scale matches what the title screen would have chosen.
    win.set_fullscreen(settings.win_disp == 1);
    set_render_art_scale(win);
    menu::apply(&settings, mixer.as_ref());
    // F-01: the title screen may have changed the dialogue font while this
    // Runtime - and the Game instance it opened at startup - was already
    // alive. A run started from the title (new game, timeline jump or a save
    // slot) reaches this point with the new id, so rebuild before the first
    // line. The in-game option screen refreshes the instance directly; this
    // comparison covers the path from the title, where nothing else can.
    let game_font_id = render::text::role_id(render::text::FontRole::Game);
    if game_font_is_stale(s.cn, rt.game_font_id, game_font_id) {
        rt.game_font_id = game_font_id;
        rt.vfont =
            render::text::open_role_font(render::text::FontRole::Game, render::DIALOG_FONT_720);
        if let Some(vf) = rt.vfont.as_ref() {
            eprintln!("font: {}", vf.describe());
        }
    }
    // Borrowed after the rebuild: the instance is replaced above, not while
    // this reference is alive.
    let vfont = &mut rt.vfont;

    let mut bgm_track: Option<(i32, usize)> = None;
    let mut fading_bgm: Option<usize> = None;
    let mut voice_stream: Option<usize> = None;
    // Voice of the line on screen while skip is running. Skip does not play
    // it; turning skip off does, so the clip matches the text.
    let mut skipped_voice: Option<vm::lscr::VoiceCue> = None;
    let mut se_channels = std::collections::HashMap::<i32, usize>::new();
    // SE channels that are fading out (ACT166) but still audible. Without this
    // the slot leaves `se_channels` immediately, so a new SE on the same channel
    // plays *on top of* the fading one - the BGM path keeps `fading_bgm` for
    // exactly this reason.
    let mut fading_se = std::collections::HashMap::<i32, usize>::new();

    // The normal data-tree game has no command-line BGM. Clear any stream a
    // title, music, or previous game screen left in the shared mixer before
    // the first script event is interpreted. The explicit legacy CLI keeps
    // its caller-provided BGM untouched.
    if s.bgm.is_none() {
        if let Some(mx) = mixer.as_ref() {
            mx.stop_category(audio::mixer::CAT_BGM);
            mx.stop_category(audio::mixer::CAT_SE);
            mx.stop_category(audio::mixer::CAT_VOICE);
            if std::env::var_os("WA2_DEBUG_AUDIO").is_some() {
                eprintln!("  audio: cleared interface streams before scene {}", current_id);
            }
        }
    }

    let mut cur_surf = render::render_scene(&bg, "", font, W, H);
    let mut bank = &mut rt.bank;
    // Standing sprites live in numbered stage slots (K16's first value): slot 0
    // left, 1 centre, 2 right. Slots are reused as characters move around.
    let mut slots: [Option<ActiveSprite>; SPRITE_SLOTS] = Default::default();
    // Active `.AMP` colour curve (sepia flashbacks, negative, ...).
    let mut tint: Option<Vec<u8>> = None;
    // Ambient light: the ACT170 preset, plus the per-background `.amp`
    // override while that background is on screen. An explicit ACT172
    // filter wins over both.
    let mut ambient_preset: Option<String> = None;
    let mut ambient_preset_id: i32 = 0;
    let mut ambient_override: Option<String> = None;
    let mut filter: Option<String> = None;
    let mut focus: Option<u32> = None;
    // 「ウィンドウ一時消去」. Only the player's button folds the box: ACT170
    // turned out to be an ambient light preset, not a window switch, so
    // fast-forward no longer has to keep a script-side open flag.
    let mut user_hide = false;
    let mut window = true;
    let mut line_no: u32 = 0;
    // ACT176 plates living on top of the background until the next scene.
    let mut plates: Vec<Plate> = Vec::new();
    // True when the current background is a CG (`v*`), which picks the CG
    // window opacity and skips the novel-mode dim.
    let mut bg_cg = false;
    // Lines this install has already shown. Skip mode consults it when
    // 「未读文本」 is selected.
    let mut seen = load_seen();
    // Typewriter: `line_base` is the frame with the window and the name but
    // no body. Each tick paints a longer prefix onto a copy of it.
    let mut line_base: Option<render::Surface> = None;
    let mut line_done = true;
    // Set after the option screen closes: the line on screen still holds
    // glyphs of the old font/style, so the wait loop has to repaint it.
    let mut line_redraw = false;
    let mut reveal: usize = 0;
    let mut type_tick: u32 = 0;
    // Only these three are extra bookkeeping for save/load. The dialog box is
    // not state - `cur_surf` holds the composited pixels but not the string -
    // and the decoded background/tint cannot be stored, so their ids and names
    // are kept next to them and re-resolved when a save is loaded.
    let mut last_text = String::new();
    let mut last_speaker: Option<String> = None;
    let mut tint_name: Option<String> = None;
    let mut bg_id: Option<(i64, u32)> = None;
    // What the screen is actually showing. Printed with every line so a black
    // frame can be told apart from a missing file.
    let mut bg_label = String::from("BLACK");
    // ---- resume from a save slot ----------------------------------------
    if let Some(snap) = resume.as_ref() {
        current_id = snap.scene.clone();
        bg_id = snap.bg;
        effect_mode = snap.effect_mode;
        effect = load_effect(&s.bg_dirs(), effect_mode);
        if let Some((id, variant)) = snap.bg {
            if let Some(name) = s.bg_index.resolve(id, variant) {
                if let Some(asset) = load_background_asset(&s.bg_dirs(), &name) {
                    bg_cg = name.starts_with('v') || name.starts_with('V');
                    bg_label = name.clone();
                    eprintln!("scene {} | bg {} id={} var={}", current_id, name, id, variant);
                    bg_scale = Some(asset.art_scale);
                    bg_src_scale = Some(asset.src_scale);
                    bg = asset.img;
                } else {
                    eprintln!(
                        "scene {} | bg id={} var={} file {} FAILED to decode, screen stays {}",
                        current_id, id, variant, name, bg_label
                    );
                }
            } else if let Some(asset) =
                load_background_asset(&s.bg_dirs(), &event_cg_name(id, variant))
            {
                // A save made on an event CG stores the script id (10170),
                // which is not a `b{id:04}{var:02}` background.
                bg_cg = true;
                bg_label = event_cg_name(id, variant);
                eprintln!("scene {} | cg {} id={} var={}", current_id, bg_label, id, variant);
                bg_scale = Some(asset.art_scale);
                    bg_src_scale = Some(asset.src_scale);
                bg = asset.img;
            } else {
                eprintln!(
                    "scene {} | bg MISSING id={} var={}, screen stays {}",
                    current_id, id, variant, bg_label
                );
            }
        }
        // The save stores the effective curve. Until the script says
        // otherwise it stands in for the ambient preset: the original keeps
        // one global for the light, and only ACT170, ACT172 or a background
        // override replaces it.
        tint_name = snap.tint.clone();
        ambient_preset_id = snap.ambient_preset;
        ambient_preset = ambient_preset_name(ambient_preset_id);
        if let Some(name) = snap.tint.as_deref() {
            tint = load_tint(&s.bg_dirs(), name);
        }
        for (i, entry) in snap.slots.iter().enumerate() {
            let (char_id, name) = match entry {
                Some(e) => e,
                None => continue,
            };
            if let Some(bank) = bank.as_mut() {
                if let Some((img, art_scale, src_scale)) = bank.get(name) {
                    slots[i] = Some(ActiveSprite { char_id: *char_id, name: name.clone(), img, art_scale, src_scale });
                }
            }
        }
        focus = snap.focus;
        let (saved_name, saved_body) = split_saved_line(&snap.text);
        last_text = saved_body;
        last_speaker = snap
            .vm
            .speaker
            .as_ref()
            .map(|(n, _)| n.clone())
            .or(saved_name);
        // BGM comes back by id: the mixer slot it was playing on is gone.
        if let (Some(mx), Some(id)) = (mixer.as_ref(), snap.bgm) {
            let dir = script_dir.parent().unwrap_or(std::path::Path::new(".")).join("bgm");
            let stem = format!("BGM_{:03}", id);
            let path = [format!("{}.OGG", stem), format!("{}_A.OGG", stem)]
                .into_iter()
                .map(|n| dir.join(n))
                .find(|p| p.is_file());
            if let Some(path) = path {
                let slot = mx.play_cat(
                    &path.to_string_lossy(),
                    1.0,
                    true,
                    crate::audio::mixer::CAT_BGM,
                );
                if slot != usize::MAX {
                    bgm_track = Some((id, slot));
                }
            }
        }
        cur_surf = draw_scene(&bg, &last_text, last_speaker.as_deref(), font, &slots, focus, win_size.0 as u32, win_size.1 as u32, tint.as_deref(), effect.as_ref(), window, msg_plate.as_ref(), msg_frame.as_ref(), &plates, scene_look(&settings, bg_cg), vfont.as_mut());
        win.present(&cur_surf);
        eprintln!("load: scene {} at pc {}", current_id, snap.vm.pc);
        remember(
            &mut history,
            capture_line(
                &current_id,
                snap.vm.clone(),
                bg_id,
                effect_mode,
                ambient_preset_id,
                bg_cg,
                &bg_label,
                &tint_name,
                &slots,
                focus,
                &last_text,
                &last_speaker,
                bgm_track.map(|(id, _)| id),
                &plates,
                None,
            ),
        );
        // The save was taken while the engine waited for input, and `pc` is
        // already past that wait, so the saved line has to be shown and waited
        // on here - otherwise the interpreter would run straight into the next
        // line and the player would never see where they left off.
        if auto_text {
            win.delay(300);
        } else {
            match wait_input(win) {
                sdl2::Input::Quit => return Ok(None),
                _ => {}
            }
        }
    }
    loop {
        let prog = match load_program(script_dir, &current_id, s.cn) {
            Some(p) => p,
            None => {
                eprintln!("scene {} not found (.bnr/.txt)", current_id);
                return Ok(None);
            }
        };
        eprintln!(
            "scene {}: {} ops, {} tokens, entry {:#x}",
            current_id,
            prog.ops.len(),
            prog.tokens.len(),
            prog.entry
        );
        let mut it = vm::lscr::Interp::new(&prog, &mut rt.global);
        // Decode what this scene will ask for while the player reads the
        // first lines, so the script never waits on a 59 MP WebP.
        {
            let (bgs, sprites) = upcoming_art(&prog, &s.bg_index, bank.as_ref());
            let queued = prefetch_paths(&s.bg_dirs(), &bgs, s.sprite_dir_now(), &sprites);
            if !queued.is_empty() {
                eprintln!("prefetch: {} pictures", queued.len());
                artcache::prefetch(queued);
            }
        }
        // The saved position belongs to the first scene only; a later scene
        // change means the player moved on, so it starts from its entry point.
        if let Some(vm) = rewind_vm.take() {
            it.restore(vm);
        } else if let Some(snap) = resume.take() {
            it.restore(snap.vm);
        }
        'events: loop {
        win_size = win.window_size();
        if decode_scale_for_window(win) != render_art_scale() {
            set_render_art_scale(win);
        }
        // A decode-scale change (fullscreen switch, window resize) invalidates
        // the pixels on screen: they were decoded for the old scale. Re-decode
        // the background and the standing sprites at the new one instead of
        // showing the old resolution stretched.
        if bg_scale.is_some_and(|scale| scale != render_art_scale()) {
            let dirs = s.bg_dirs();
            let reload = match bg_id {
                Some((id, variant)) => s.bg_index.resolve(id, variant),
                None if !bg_label.eq_ignore_ascii_case("BLACK") => Some(bg_label.clone()),
                None => None,
            };
            if let Some(name) = reload {
                if let Some(asset) = load_background_asset(&dirs, &name) {
                    bg = asset.img;
                    bg_scale = Some(asset.art_scale);
                    bg_src_scale = Some(asset.src_scale);
                    bg_label = name;
                    eprintln!(
                        "art: reloaded {} at decode scale {}x",
                        bg_label,
                        render_art_scale()
                    );
                }
            }
            for slot in slots.iter_mut().flatten() {
                if let Some(bank) = bank.as_mut() {
                    if let Some((img, art_scale, src_scale)) = bank.get(&slot.name) {
                        slot.img = img;
                        slot.art_scale = art_scale;
                        slot.src_scale = src_scale;
                    }
                }
            }
        }
            let event = if park_wait {
                park_wait = false;
                vm::lscr::Event::Wait
            } else {
                it.next_event()
            };
            match event {
                vm::lscr::Event::Text { text, speaker, voice } => {
                    let body = clean_text(&text);
                    let voice_id = match voice {
                        Some(v) => format!("{}_{:04}_{:02}", v.scenario, v.line, v.char_id),
                        None => "-".to_string(),
                    };
                    let muted = voice
                        .and_then(|c| menu::char_row_of(c.char_id))
                        .map(|row| settings.char_voice[row] == 0)
                        .unwrap_or(false);
                    let cue = voice.filter(|_| !muted);
                    line_voice = cue;
                    // 「未读文本」: an unseen line drops out of skip and is shown.
                    let seen_key = format!("{}\t{}", current_id, body.replace('\n', " "));
                    if cmd_skip && settings.text_skip == 1 && !seen.contains(&seen_key) {
                        cmd_skip = false;
                    }
                    if cmd_skip {
                        // Starting a clip per skipped line piles ffmpeg jobs up
                        // across a scene change, and the one still audible is
                        // no longer the line on screen.
                        skipped_voice = cue;
                        if let Some(mx) = mixer.as_ref() {
                            stop_voice(mx, &mut voice_stream);
                        }
                    }
                    last_speaker = speaker.clone();
                    last_text = body.clone();
                    // The box only follows the player's hide button now;
                    // ACT170 is an ambient light preset, not a window switch.
                    window = dialog_open(user_hide);
                    line_no += 1;
                    let cast = slots
                        .iter()
                        .enumerate()
                        .filter_map(|(i, s)| {
                            s.as_ref().map(|a| {
                                format!("{}:{}({}x)", i, a.name, a.src_scale)
                            })
                        })
                        .collect::<Vec<_>>()
                        .join(",");
                    let cast = if cast.is_empty() {
                        "-".to_string()
                    } else {
                        cast
                    };
                    let focus_id = focus
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "-".to_string());
                    // The Chinese patch stores text as Shift-JIS code points and
                    // paints the real glyphs into the font. Those code points are
                    // not the characters on screen, so the log keeps the voice
                    // id instead of printing them.
                    let cn = s.script_dir.file_name().and_then(|n| n.to_str()) == Some("script_cn");
                    // Mark which art tree the picture came from, so the log
                    // says whether this frame is 4x or 8x art.
                    let bg_tag = match bg_src_scale {
                        Some(scale) if !bg_label.eq_ignore_ascii_case("BLACK") => {
                            format!("{}({}x)", bg_label, scale)
                        }
                        _ => bg_label.clone(),
                    };
                    let bgm_tag = bgm_track
                        .map(|(id, _)| id.to_string())
                        .unwrap_or_else(|| "-".to_string());
                    if cn {
                        eprintln!(
                            "scene {} | line {} | bg {} | sprites {} | focus {} | window {} | voice {} | bgm {}",
                            current_id,
                            line_no,
                            bg_tag,
                            cast,
                            focus_id,
                            if window { "open" } else { "hidden" },
                            voice_id,
                            bgm_tag
                        );
                    } else {
                        let who = speaker.as_deref().unwrap_or("-");
                        let preview = body.replace('\n', " / ");
                        let preview: String = preview.chars().take(42).collect();
                        eprintln!(
                            "scene {} | line {} | bg {} | sprites {} | focus {} | window {} | voice {} | bgm {} | {} | {}",
                            current_id,
                            line_no,
                            bg_tag,
                            cast,
                            focus_id,
                            if window { "open" } else { "hidden" },
                            voice_id,
                            bgm_tag,
                            who,
                            preview
                        );
                    }
                    // Speed 0, skip, and the headless auto run reveal the line at
                    // once. Otherwise the wait loop types it onto `line_base`.
                    let instant = cmd_skip
                        || auto_text
                        || settings.text_speed == 0
                        || !window
                        || body.is_empty();
                    if instant {
                        line_base = None;
                        line_done = true;
                        reveal = body.chars().count();
                        cur_surf = draw_scene(&bg, &body, speaker.as_deref(), font, &slots, focus, win_size.0 as u32, win_size.1 as u32, tint.as_deref(), effect.as_ref(), window, msg_plate.as_ref(), msg_frame.as_ref(), &plates, scene_look(&settings, bg_cg), vfont.as_mut());
                        win.present(&cur_surf);
                    } else {
                        line_base = Some(draw_scene(&bg, "", speaker.as_deref(), font, &slots, focus, win_size.0 as u32, win_size.1 as u32, tint.as_deref(), effect.as_ref(), window, msg_plate.as_ref(), msg_frame.as_ref(), &plates, scene_look(&settings, bg_cg), vfont.as_mut()));
                        reveal = 0;
                        line_done = false;
                        type_tick = 0;
                        // The name is on screen before the voice starts. Drawing
                        // the line takes long enough that starting the clip
                        // first played the next sentence over the previous one.
                        if let Some(base) = line_base.as_ref() {
                            let mut frame = clone_surf(base);
                            let active = cmdbar::Active {
                                auto: cmd_auto,
                                skip: cmd_skip,
                                rewind: cmd_rewind,
                                hide: user_hide,
                            };
                            let (fw, fh) = (frame.width as i32, frame.height as i32);
                            bar.draw(&mut frame, None, active, cmd_idle(&settings), fw, fh);
                            cur_surf = frame;
                            win.present(&cur_surf);
                        }
                    }
                    // The picture for this line is up. Start its clip now, so
                    // the `voice:` line in the log belongs to the scene line
                    // just printed and not to the one still on screen.
                    if !cmd_skip {
                        if let (Some(mx), Some(vi), Some(cue)) = (mixer.as_ref(), voices.as_ref(), cue) {
                            skipped_voice = None;
                            play_voice(mx, vi, &settings, &mut voice_stream, cue, &current_id);
                        } else if settings.page_voice == 1 {
                            skipped_voice = None;
                            if let Some(mx) = mixer.as_ref() {
                                stop_voice(mx, &mut voice_stream);
                            }
                        }
                    }
                    // Blank line after this line's log block (scene line and its
                    // voice line) so consecutive clicks are easy to tell apart.
                    eprintln!();
                    // Remember the current line for save (last_text) and backlog.
                    let logged = match &speaker {
                        Some(sp) => format!("{}：{}", sp, body),
                        None => body.clone(),
                    };
                    backlog.push(logged);
                    if backlog.len() > 200 { backlog.remove(0); }
                }
                vm::lscr::Event::Sprite { char_id, series, variant, slot, frames, kind } => {
                    let old = clone_surf(&cur_surf);
                    let mut changed = false;
                    if let Some(bank) = bank.as_mut() {
                        if let Some(name) = bank.resolve(char_id, series, variant) {
                            if let Some((img, art_scale, src_scale)) = bank.get(&name) {
                                let idx = slot as usize;
                                // One character occupies one slot. Moving them
                                // (雪菜 from 2 to 1, and so on) has to clear
                                // the place they just left.
                                for s in slots.iter_mut() {
                                    if s.as_ref().is_some_and(|a| a.char_id == char_id) {
                                        *s = None;
                                    }
                                }
                                if let Some(dst) = slots.get_mut(idx) {
                                    eprintln!(
                                        "  sprite: {} {:?} ({}x source, char {}, slot {}, {}f)",
                                        name, kind, src_scale, char_id, idx, frames
                                    );
                                    *dst = Some(ActiveSprite { char_id, name, img, art_scale, src_scale });
                                    changed = true;
                                } else {
                                    eprintln!("  sprite: slot {} out of range", idx);
                                }
                            }
                        } else {
                            eprintln!("  sprite: char {} series {:03} variant {:03} not found", char_id, series, variant);
                        }
                    }
                    if changed {
                        line_base = None;
                        line_done = true;
                        // The previous line has already been advanced past. Leave
                        // the box empty until the next line, so the old speaker's
                        // words are not left sitting on the new face.
                        cur_surf = draw_scene(&bg, "", None, font, &slots, focus, win_size.0 as u32, win_size.1 as u32, tint.as_deref(), effect.as_ref(), window, msg_plate.as_ref(), msg_frame.as_ref(), &plates, scene_look(&settings, bg_cg), vfont.as_mut());
                        if present_fade(win, &old, &cur_surf, fade_len(&settings, cmd_skip, frames)) {
                            return Ok(None);
                        }
                    }
                }
                vm::lscr::Event::SpriteExit { char_id, frames } => {
                    let old = clone_surf(&cur_surf);
                    let before = slots.iter().flatten().filter(|a| a.char_id == char_id).count();
                    for s in slots.iter_mut() {
                        if s.as_ref().is_some_and(|a| a.char_id == char_id) {
                            *s = None;
                        }
                    }
                    eprintln!("  sprite exit: char {} ({}f, removed {})", char_id, frames, before);
                    if before > 0 {
                        line_base = None;
                        line_done = true;
                        cur_surf = draw_scene(&bg, &last_text, last_speaker.as_deref(), font, &slots, focus, win_size.0 as u32, win_size.1 as u32, tint.as_deref(), effect.as_ref(), window, msg_plate.as_ref(), msg_frame.as_ref(), &plates, scene_look(&settings, bg_cg), vfont.as_mut());
                        if present_fade(win, &old, &cur_surf, fade_len(&settings, cmd_skip, frames)) {
                            return Ok(None);
                        }
                    }
                }
                vm::lscr::Event::SpriteFocus { char_id } => {
                    focus = Some(char_id);
                }
                vm::lscr::Event::Tint { name } => {
                    // Explicit colour filter (sepia flashbacks, negative) -
                    // the background stays as it is. It wins over the ambient
                    // light until the next ACT172.
                    filter = if name.is_empty() { None } else { Some(name.clone()) };
                    let dirs = s.bg_dirs();
                    let effective = effective_tint_name(&filter, &ambient_override, &ambient_preset);
                    tint_name = effective.clone();
                    tint = load_tint_logged(&dirs, &effective);
                    cur_surf = draw_scene(&bg, "", None, font, &slots, focus, win_size.0 as u32, win_size.1 as u32, tint.as_deref(), effect.as_ref(), window, msg_plate.as_ref(), msg_frame.as_ref(), &plates, scene_look(&settings, bg_cg), vfont.as_mut());
                    win.present(&cur_surf);
                }
                vm::lscr::Event::BgScene { id, variant, frames, mode } => {
                    // ACT146/147. id 0 and -2 are a scripted black screen.
                    // A missing file keeps the picture already on screen.
                    let old = clone_surf(&cur_surf);
                    effect_mode = mode;
                    effect = load_effect(&s.bg_dirs(), effect_mode);
                    if id == 0 || id == -2 {
                        plates.clear();
                        bg_cg = false;
                        bg_id = None;
                        bg_label = "BLACK".to_string();
                        bg = solid_bg(W, H, [0, 0, 0]);
                        bg_scale = None;
                        bg_src_scale = None;
                        // id 0 is a fade-out: the shot is over, so the people
                        // from it must leave. id -2 keeps whoever is standing
                        // (the cut to black in 1010 still has them in frame).
                        // Without this, 依緒 and 雪菜 stay on screen through the
                        // whole rest of 1012_030, including the park and the
                        // narration that is only about かずさ.
                        if id == 0 {
                            slots = Default::default();
                        }
                        eprintln!(
                            "scene {} | bg BLACK (script id {} fade {}f)",
                            current_id, id, frames
                        );
                    } else if let Some(name) = s.bg_index.resolve(id, variant) {
                        if let Some(asset) = load_background_asset(&s.bg_dirs(), &name) {
                            plates.clear();
                            bg_cg = name.starts_with('v') || name.starts_with('V');
                            bg_id = Some((id, variant));
                            bg_label = name.clone();
                            bg_scale = Some(asset.art_scale);
                    bg_src_scale = Some(asset.src_scale);
                            bg = asset.img;
                            eprintln!(
                                "scene {} | bg {} id={} var={} fade={}f",
                                current_id, name, id, variant, frames
                            );
                            // Per-background ambient override: the exe uses
                            // the background id, its one-digit variant, and
                            // the active ACT170 preset as three separate
                            // sprintf arguments (`b{ID}{digit}{preset}.amp`).
                            let ov = background_override_name(id, variant, ambient_preset_id);
                            if load_tint(&s.bg_dirs(), &ov).is_some() {
                                eprintln!("  ambient: bg override {}", ov);
                                ambient_override = Some(ov);
                            } else {
                                ambient_override = None;
                            }
                        } else {
                            eprintln!(
                                "scene {} | bg id={} var={} file {} FAILED to decode, screen stays {}",
                                current_id, id, variant, name, bg_label
                            );
                        }
                    } else {
                        eprintln!(
                            "scene {} | bg MISSING id={} var={}, screen stays {}",
                            current_id, id, variant, bg_label
                        );
                    }
                    line_base = None;
                    line_done = true;
                    // The override probe may have changed the effective curve.
                    let effective = effective_tint_name(&filter, &ambient_override, &ambient_preset);
                    if tint_name != effective {
                        tint_name = effective.clone();
                        tint = load_tint_logged(&s.bg_dirs(), &effective);
                    }
                    cur_surf = draw_scene(&bg, "", None, font, &slots, focus, win_size.0 as u32, win_size.1 as u32, tint.as_deref(), effect.as_ref(), window, msg_plate.as_ref(), msg_frame.as_ref(), &plates, scene_look(&settings, bg_cg), vfont.as_mut());
                    if present_fade(win, &old, &cur_surf, fade_len(&settings, cmd_skip, frames)) {
                        return Ok(None);
                    }
                }
                vm::lscr::Event::EventCg { id, variant, frames } => {
                    // ACT148 replaces the standing shot with one picture.
                    // ACT201 turning a layer on comes through as the same
                    // event (frames 0) and swaps that layer in.
                    let old = clone_surf(&cur_surf);
                    let name = event_cg_name(id, variant);
                    if let Some(asset) = load_background_asset(&s.bg_dirs(), &name) {
                        plates.clear();
                        slots = Default::default();
                        bg_cg = true;
                        bg_id = Some((id, variant));
                        bg_label = name.clone();
                        bg_scale = Some(asset.art_scale);
                    bg_src_scale = Some(asset.src_scale);
                        bg = asset.img;
                        eprintln!(
                            "scene {} | cg {} id={} var={} fade={}f",
                            current_id, name, id, variant, frames
                        );
                    } else {
                        eprintln!(
                            "scene {} | cg MISSING {} id={} var={}, screen stays {}",
                            current_id, name, id, variant, bg_label
                        );
                    }
                    line_base = None;
                    line_done = true;
                    cur_surf = draw_scene(&bg, "", None, font, &slots, focus, win_size.0 as u32, win_size.1 as u32, tint.as_deref(), effect.as_ref(), window, msg_plate.as_ref(), msg_frame.as_ref(), &plates, scene_look(&settings, bg_cg), vfont.as_mut());
                    if present_fade(win, &old, &cur_surf, fade_len(&settings, cmd_skip, frames)) {
                        return Ok(None);
                    }
                }
                vm::lscr::Event::Flash { frames, r, g, b } => {
                    // ACT152. The picture snaps to the colour and eases back.
                    // Skip and "effects off" drop it; a click cuts it short.
                    eprintln!(
                        "scene {} | flash {},{},{} {}f",
                        current_id, r, g, b, frames
                    );
                    let play = !cmd_skip && settings.effect != 0 && frames > 0;
                    if play && present_flash(win, &cur_surf, frames, r, g, b) {
                        return Ok(None);
                    }
                }
                vm::lscr::Event::Overlay { name, slot } => {
                    if let Some(asset) = load_background_asset(&s.bg_dirs(), &name) {
                        let img = &asset.img;
                        plates.retain(|p| p.slot != slot);
                        eprintln!("  overlay: slot {} {} ({}x{})", slot, name, img.width, img.height);
                        plates.push(Plate { slot, name, img: asset.img, art_scale: asset.art_scale });
                        line_base = None;
                        line_done = true;
                        cur_surf = draw_scene(&bg, &last_text, last_speaker.as_deref(), font, &slots, focus, win_size.0 as u32, win_size.1 as u32, tint.as_deref(), effect.as_ref(), window, msg_plate.as_ref(), msg_frame.as_ref(), &plates, scene_look(&settings, bg_cg), vfont.as_mut());
                        win.present(&cur_surf);
                    } else {
                        eprintln!("  overlay: slot {} {} NOT FOUND", slot, name);
                    }
                }
                vm::lscr::Event::EnvTint { preset } => {
                    // Ambient light preset. The original executable's table
                    // (0x4be328) maps 0..3 to none / evening / night / indoor;
                    // the message window is not involved.
                    let name = ambient_preset_name(preset);
                    eprintln!("  ambient: preset {} ({})", preset, name.as_deref().unwrap_or("<none>"));
                    ambient_preset_id = preset.clamp(0, 3);
                    ambient_preset = name;
                    // The preset takes the screen back even where a background
                    // override was in force.
                    ambient_override = None;
                    let dirs = s.bg_dirs();
                    let effective = effective_tint_name(&filter, &ambient_override, &ambient_preset);
                    tint_name = effective.clone();
                    tint = load_tint_logged(&dirs, &effective);
                    line_base = None;
                    line_done = true;
                    cur_surf = draw_scene(&bg, "", None, font, &slots, focus, win_size.0 as u32, win_size.1 as u32, tint.as_deref(), effect.as_ref(), window, msg_plate.as_ref(), msg_frame.as_ref(), &plates, scene_look(&settings, bg_cg), vfont.as_mut());
                    win.present(&cur_surf);
                }
                vm::lscr::Event::WindowAlpha { value, channel } => {
                    if matches!(channel, 0 | 1 | 2) {
                        set_script_window_alpha(value, channel);
                        eprintln!(
                            "  window: ACT171 alpha={} channel={} (novel/visual override)",
                            value.clamp(0, 255), channel
                        );
                        line_base = None;
                        line_done = true;
                        cur_surf = draw_scene(&bg, &last_text, last_speaker.as_deref(), font, &slots, focus, win_size.0 as u32, win_size.1 as u32, tint.as_deref(), effect.as_ref(), window, msg_plate.as_ref(), msg_frame.as_ref(), &plates, scene_look(&settings, bg_cg), vfont.as_mut());
                        win.present(&cur_surf);
                    } else {
                        eprintln!("  window: ACT171 ignored unsupported channel {} value={}", channel, value);
                    }
                }
                vm::lscr::Event::WaitFrames(n) => {
                    // Scripted pause (camera holds, fades). Skip mode does not
                    // sit through it: a new scene often opens with one of
                    // these, and the button would ignore clicks until it ended.
                    if !cmd_skip {
                        let total = std::time::Duration::from_secs_f64(n as f64 / 60.0);
                        let t0 = std::time::Instant::now();
                        while t0.elapsed() < total && !cmd_skip {
                            match win.poll_input() {
                                sdl2::Input::Quit => return Ok(None),
                                sdl2::Input::Pointer { x, y, pressed: true } => {
                                    let (sx, sy) = win.surface_pos(x, y, W, H);
                                    if bar.hit(sx, sy) == Some(cmdbar::Cmd::Skip) {
                                        cmd_skip = true;
                                        window = dialog_open(user_hide);
                                    }
                                }
                                _ => {}
                            }
                            win.delay(16);
                        }
                    }
                }
                vm::lscr::Event::Sound { opcode, args } => {
                    let arg = |i: usize| args.get(i).copied().unwrap_or(0);
                    let root = s.script_dir.parent().unwrap_or(std::path::Path::new("."));
                    let volume = |v: i32| v.clamp(0, 255) as f32 / 255.0;
                    if let Some(mx) = mixer.as_ref() {
                        match opcode {
                            158 if args.len() >= 4 => {
                                let id = arg(0);
                                if id < 0 {
                                    if let Some((_, slot)) = bgm_track.take() { mx.fade(slot, 0.0, arg(1), true); }
                                } else if let Some((_, slot)) = bgm_track.filter(|(old, _)| *old == id) {
                                    mx.fade(slot, volume(arg(3)), arg(1), false);
                                } else {
                                    if let Some(slot) = fading_bgm.take() { mx.stop(slot); }
                                    if let Some((_, slot)) = bgm_track.take() {
                                        mx.fade(slot, 0.0, if arg(1) == -2 { 15 } else { arg(1) }, true);
                                        fading_bgm = Some(slot);
                                    }
                                    let dir = root.join("bgm");
                                    let stem = format!("BGM_{:03}", id);
                                    let path = [format!("{}.OGG", stem), format!("{}_{}.OGG", stem, if arg(4) != 0 { "B" } else { "A" })]
                                        .into_iter().map(|name| dir.join(name)).find(|p| p.is_file());
                                    if let Some(path) = path {
                                        let slot = mx.play_cat(&path.to_string_lossy(), if arg(1) > 0 { 0.0 } else { volume(arg(3)) }, arg(2) != 0, crate::audio::mixer::CAT_BGM);
                                        if slot != usize::MAX {
                                            mx.fade(slot, volume(arg(3)), arg(1), false);
                                            bgm_track = Some((id, slot));
                                            eprintln!("  BGM: ACT158 track {} -> {}", id, path.display());
                                        }
                                    } else { eprintln!("  BGM: missing track {}", id); }
                                }
                            }
                            159 => {
                                let fade = arg(0);
                                if let Some((old, slot)) = bgm_track.take() {
                                    eprintln!("  BGM: ACT159 stop track {} fade={}f", old, fade);
                                    mx.fade(slot, 0.0, fade, true);
                                } else {
                                    eprintln!("  BGM: ACT159 stop (no scene track) fade={}f", fade);
                                }
                            }
                            161 => { if let Some((_, slot)) = bgm_track { mx.fade(slot, volume(arg(0)), arg(1), false); } }
                            165 if arg(5) == 2 => {
                                // Original 0x40f2d0 updates the existing decoder in mode 2.
                                if let Some(&slot) = se_channels.get(&arg(0)) {
                                    mx.fade(slot, volume(arg(4)), arg(2), false);
                                }
                            }
                            164 | 165 => {
                                let (channel, id, frames, looping, vol) = if opcode == 164 {
                                    (None, arg(0), 0, false, arg(1))
                                } else { (Some(arg(0)), arg(1), arg(2), arg(3) == 1, arg(4)) };
                                if let Some(ch) = channel {
                                    if let Some(slot) = se_channels.remove(&ch) { mx.stop(slot); }
                                    // The channel may still be fading out from an ACT166.
                                    if let Some(slot) = fading_se.remove(&ch) { mx.stop(slot); }
                                }
                                let dir = root.join("se");
                                let stem = format!("SE_{:04}", id);
                                let path = ["WAV", "OGG", "wav", "ogg"].into_iter()
                                    .map(|ext| dir.join(format!("{}.{}", stem, ext))).find(|p| p.is_file());
                                if let Some(path) = path {
                                    let slot = mx.play_cat(&path.to_string_lossy(), if frames > 0 { 0.0 } else { volume(vol) }, looping, crate::audio::mixer::CAT_SE);
                                    if slot != usize::MAX {
                                        mx.fade(slot, volume(vol), frames, false);
                                        if let Some(ch) = channel { se_channels.insert(ch, slot); }
                                        eprintln!("  SE: ACT{} ch={:?} id={} -> {}", opcode, channel, id, path.display());
                                    }
                                } else { eprintln!("  SE: missing {}", id); }
                            }
                            166 => {
                                if let Some(slot) = se_channels.remove(&arg(0)) {
                                    mx.fade(slot, 0.0, arg(1), true);
                                    // Keep tracking it until the fade finishes so a
                                    // restart on this channel can stop it first.
                                    fading_se.insert(arg(0), slot);
                                }
                            }
                            167 => { if let Some(&slot) = se_channels.get(&arg(0)) { mx.fade(slot, volume(arg(1)), arg(2), false); } }
                            162 | 163 | 169 => {
                                let slot = if opcode == 169 { se_channels.get(&arg(0)).copied() }
                                    else { bgm_track.map(|(_, slot)| slot).or(fading_bgm) };
                                if let Some(slot) = slot {
                                    while !cmd_skip && if opcode == 163 { !mx.finished(slot) } else { !mx.settled(slot) } {
                                        match win.poll_input() {
                                            sdl2::Input::Quit => return Ok(None),
                                            sdl2::Input::Pointer { x, y, pressed: true } => {
                                                let (sx, sy) = win.surface_pos(x, y, W, H);
                                                if bar.hit(sx, sy) == Some(cmdbar::Cmd::Skip) {
                                                    cmd_skip = true;
                                                }
                                            }
                                            _ => {}
                                        }
                                        win.delay(16);
                                    }
                                }
                            }
                            168 => {
                                if let Some(&slot) = se_channels.get(&arg(0)) {
                                    while !cmd_skip && !mx.finished(slot) {
                                        match win.poll_input() {
                                            sdl2::Input::Quit => return Ok(None),
                                            sdl2::Input::Pointer { x, y, pressed: true } => {
                                                let (sx, sy) = win.surface_pos(x, y, W, H);
                                                if bar.hit(sx, sy) == Some(cmdbar::Cmd::Skip) {
                                                    cmd_skip = true;
                                                }
                                            }
                                            _ => {}
                                        }
                                        win.delay(16);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                vm::lscr::Event::Movie { name } => {
                    eprintln!("  movie: {}", name);
                    let _ = play_movie(
                        win,
                        &s.movie_dir,
                        &name,
                        mixer.as_ref(),
                        &bg_dirs,
                        Some(&ui),
                    );
                    win.present(&cur_surf);
                }
                vm::lscr::Event::Choice { options } => {
                    // The script's own lines already carry the option numbers.
                    let opts: Vec<String> = options.iter().map(|o| clean_text(o)).collect();
                    // The original keeps the line that is on screen under the
                    // choices and puts every option in its own one-line message
                    // window, centred on the screen above the dialogue box. A
                    // full scene rebuild per frame would be far too slow while
                    // the mouse moves, so the scene (and that line) is composited
                    // once and the rows + highlight are redrawn on a copy.
                    let look_alpha = scene_look(&settings, bg_cg).alpha;
                    let mut base = draw_scene(
                        &bg,
                        &last_text,
                        last_speaker.as_deref(),
                        font,
                        &slots,
                        focus,
                        win_size.0 as u32,
                        win_size.1 as u32,
                        tint.as_deref(),
                        effect.as_ref(),
                        window,
                        msg_plate.as_ref(),
                        msg_frame.as_ref(),
                        &plates,
                        scene_look(&settings, bg_cg),
                        vfont.as_mut(),
                    );
                    let row_layout = choice_row_layout(opts.len(), base.height);
                    let ui_se = ui_se_path(&s.script_dir);
                    let mut hover: Option<usize> = None;
                    let mut bar_hover: Option<cmdbar::Cmd> = None;
                    cur_surf = render::Surface {
                        width: base.width,
                        height: base.height,
                        pixels: base.pixels.clone(),
                    };
                    win.present(&cur_surf);
                    eprintln!(
                        "  choice: presenting {} options (click one, or press 1-{})",
                        opts.len(),
                        opts.len()
                    );
                    let chosen = if auto_pick {
                        win.delay(300);
                        1u32
                    } else {
                        loop {
                            if cmd_rewind || rewind_left > 0 {
                                let announce = !cmd_rewind && rewind_left <= 1;
                                let cruising = cmd_rewind || rewind_left > 1;
                                match take_rewind(&mut history, false) {
                                    None => {
                                        cmd_rewind = false;
                                        rewind_left = 0;
                                    }
                                    Some(dest) => {
                                        if rewind_left > 0 {
                                            rewind_left -= 1;
                                        }
                                        let step = rewind_step(
                                            &dest,
                                            &mut it,
                                            &mut current_id,
                                            &mut rewind_vm,
                                            s,
                                            font,
                                            vfont.as_mut(),
                                            win,
                                            &mut bg,
                                            &mut effect,
                                            &mut effect_mode,
                                            &mut bg_cg,
                                            &mut bg_label,
                                            &mut bg_id,
                                            &mut tint,
                                            &mut tint_name,
                                            &mut slots,
                                            &mut bank,
                                            &mut focus,
                                            &mut plates,
                                            &mut last_text,
                                            &mut last_speaker,
                                            &mut ambient_preset,
                                            &mut ambient_preset_id,
                                            &mut ambient_override,
                                            &mut filter,
                                            user_hide,
                                            &mut window,
                                            &settings,
                                            msg_plate.as_ref(),
                                            msg_frame.as_ref(),
                                            &mut cur_surf,
                                            &mut line_base,
                                            &mut line_done,
                                            &mut reveal,
                                            mixer.as_ref(),
                                            &mut bgm_track,
                                            &mut fading_bgm,
                                            &mut voice_stream,
                                            voices.as_ref(),
                                            &mut line_voice,
                                            announce,
                                        );
                                        match step {
                                            RewindStep::Shown => {
                                                present_rewind_frame(
                                                    win,
                                                    &cur_surf,
                                                    &bar,
                                                    bar_hover,
                                                    cmdbar::Active {
                                                        auto: cmd_auto,
                                                        skip: false,
                                                        rewind: cmd_rewind,
                                                        hide: user_hide,
                                                    },
                                                    cmd_idle(&settings),
                                                );
                                                if cruising {
                                                    match rewind_cruise_stop(win, &bar, &mut bar_hover) {
                                                        Cruise::Quit => return Ok(None),
                                                        Cruise::Keep => {
                                                            win.delay(16);
                                                            continue;
                                                        }
                                                        Cruise::Stop => {}
                                                    }
                                                }
                                                cmd_rewind = false;
                                                rewind_left = 0;
                                                park_wait = true;
                                                continue 'events;
                                            }
                                            RewindStep::Reload => {
                                                present_rewind_frame(
                                                    win,
                                                    &cur_surf,
                                                    &bar,
                                                    bar_hover,
                                                    cmdbar::Active {
                                                        auto: cmd_auto,
                                                        skip: false,
                                                        rewind: cmd_rewind,
                                                        hide: user_hide,
                                                    },
                                                    cmd_idle(&settings),
                                                );
                                                park_wait = true;
                                                break 'events;
                                            }
                                            RewindStep::Stuck => {}
                                        }
                                    }
                                }
                            }
                            let mut frame = render::Surface {
                                width: base.width,
                                height: base.height,
                                pixels: base.pixels.clone(),
                            };
                            paint_choice_rows(
                                &mut frame,
                                vfont.as_mut(),
                                font,
                                &opts,
                                &row_layout,
                                hover,
                                msg_plate.as_ref(),
                                msg_frame.as_ref(),
                                look_alpha,
                            );
                            let active = cmdbar::Active {
                                auto: cmd_auto,
                                skip: cmd_skip,
                                rewind: cmd_rewind,
                                hide: user_hide,
                            };
                            let (fw, fh) = (frame.width as i32, frame.height as i32);
                            bar.draw(&mut frame, bar_hover, active, cmd_idle(&settings), fw, fh);
                            win.present(&frame);
                            match win.poll_input() {
                                sdl2::Input::Quit => return Ok(None),
                                sdl2::Input::RightClick => {
                                    user_hide = !user_hide;
                                    window = dialog_open(user_hide);
                                    base = draw_scene(
                                        &bg,
                                        &last_text,
                                        last_speaker.as_deref(),
                                        font,
                                        &slots,
                                        focus,
                                        win_size.0 as u32,
                                        win_size.1 as u32,
                                        tint.as_deref(),
                                        effect.as_ref(),
                                        window,
                                        msg_plate.as_ref(),
                                        msg_frame.as_ref(),
                                        &plates,
                                        scene_look(&settings, bg_cg),
                                        vfont.as_mut(),
                                    );
                                    cur_surf = clone_surf(&base);
                                }
                                sdl2::Input::Choice(n) if (n as usize) <= opts.len() => {
                                    play_ui_se(mixer.as_ref(), ui_se.as_deref());
                                    break n as u32
                                }
                                sdl2::Input::Pointer { x, y, pressed } => {
                                    let (sx, sy) = win.surface_pos(x, y, W, H);
                                    if cmd_skip && pressed {
                                        cmd_skip = false;
                                        window = dialog_open(user_hide);
                                        continue;
                                    }
                                    // Command bar takes priority over the choices.
                                    if let Some(cmd) = bar.hit(sx, sy) {
                                        // The cursor left the rows for the command bar.
                                        hover = None;
                                        if pressed {
                                            match cmd {
                                                cmdbar::Cmd::Auto => {
                                                    cmd_auto = !cmd_auto;
                                                    if cmd_auto {
                                                        cmd_rewind = false;
                                                        rewind_left = 0;
                                                    }
                                                }
                                                cmdbar::Cmd::Rewind => {
                                                    cmd_rewind = !cmd_rewind;
                                                    if cmd_rewind {
                                                        cmd_skip = false;
                                                        cmd_auto = false;
                                                    } else {
                                                        rewind_left = 0;
                                                    }
                                                }
                                                cmdbar::Cmd::Skip => {
                                                    cmd_skip = !cmd_skip;
                                                    if cmd_skip {
                                                        cmd_rewind = false;
                                                        rewind_left = 0;
                                                    }
                                                    window = dialog_open(user_hide);
                                                    if let Some(mx) = mixer.as_ref() {
                                                        if cmd_skip {
                                                            stop_voice(mx, &mut voice_stream);
                                                        } else if let (Some(cue), Some(vi)) = (skipped_voice.take(), voices.as_ref()) {
                                                            play_voice(mx, vi, &settings, &mut voice_stream, cue, &current_id);
                                                        }
                                                    }
                                                },
                                                cmdbar::Cmd::Hide => {
                                                    user_hide = !user_hide;
                                                    window = dialog_open(user_hide);
                                                    line_base = None;
                                                    line_done = true;
                                                    cur_surf = draw_scene(
                                                        &bg,
                                                        &last_text,
                                                        last_speaker.as_deref(),
                                                        font,
                                                        &slots,
                                                        focus,
                                                        win_size.0 as u32,
                                                        win_size.1 as u32,
                                                        tint.as_deref(),
                                                        effect.as_ref(),
                                                        window,
                                                        msg_plate.as_ref(),
                                                        msg_frame.as_ref(),
                                                        &plates,
                                                        scene_look(&settings, bg_cg),
                                                        vfont.as_mut(),
                                                    );
                                                }
                                                other => {
                                                    let is_config = other == cmdbar::Cmd::Config;
                                                    match handle_cmd(
                                                        other,
                                                        win,
                                                        font,
                                                        vfont.as_mut(),
                                                        s,
                                                        &mut settings,
                                                        mixer.as_ref(),
                                                        &it,
                                                        &current_id,
                                                        bg_id,
                                                        effect_mode,
                                                        ambient_preset_id,
                                                        &tint_name,
                                                        &slots,
                                                        focus,
                                                        &last_text,
                                                        bgm_track.map(|(id, _)| id),
                                                        &cur_surf,
                                                        &backlog,
                                                    ) {
                                                        CmdOutcome::Loaded(st) => return Ok(Some(st)),
                                                        CmdOutcome::Jump(id) => {
                                                            if let Some(mx) = mixer.as_ref() {
                                                                stop_voice(mx, &mut voice_stream);
                                                                if let Some((_, slot)) = bgm_track.take() {
                                                                    mx.stop(slot);
                                                                }
                                                                for slot in se_channels.drain().map(|(_, slot)| slot) {
                                                                    mx.stop(slot);
                                                                }
                                                                for slot in fading_se.drain().map(|(_, slot)| slot) {
                                                                    mx.stop(slot);
                                                                }
                                                                if let Some(slot) = fading_bgm.take() {
                                                                    mx.stop(slot);
                                                                }
                                                            }
                                                            current_id = id;
                                                            cmd_auto = false;
                                                            cmd_skip = false;
                                                            cmd_rewind = false;
                                                            rewind_left = 0;
                                                            rewind_vm = None;
                                                            park_wait = false;
                                                            skipped_voice = None;
                                                            line_voice = None;
                                                            line_no = 0;
                                                            history.clear();
                                                            user_hide = false;
                                                            window = true;
                                                            tint = None;
                                                            tint_name = None;
                                                            ambient_preset = None;
                                                            ambient_preset_id = 0;
                                                            ambient_override = None;
                                                            filter = None;
                                                            effect_mode = 0;
                                                            effect = None;
                                                            bg = solid_bg(W, H, [0, 0, 0]);
                                                            bg_scale = None;
                                                            bg_src_scale = None;
                                                            bg_id = None;
                                                            bg_label = String::from("BLACK");
                                                            bg_cg = false;
                                                            last_text.clear();
                                                            last_speaker = None;
                                                            cur_surf = render::render_scene(&bg, "", font, W, H);
                                                            line_redraw = false;
                                                            reveal = 0;
                                                            type_tick = 0;
                                                            slots = Default::default();
                                                            plates.clear();
                                                            line_base = None;
                                                            line_done = true;
                                                            focus = None;
                                                            break 'events;
                                                        }
                                                        CmdOutcome::Home | CmdOutcome::Quit => {
                                                            stop_game_audio(
                                                                mixer.as_ref(),
                                                                &mut bgm_track,
                                                                &mut fading_bgm,
                                                                &mut voice_stream,
                                                                &mut se_channels,
                                                                &mut fading_se,
                                                            );
                                                            return Ok(None);
                                                        }
                                                        CmdOutcome::Stay => {
                                                            // F-17: the line under
                                                            // the choice rows still
                                                            // has the old font/style.
                                                            if is_config {
                                                                base = draw_scene(
                                                                    &bg,
                                                                    &last_text,
                                                                    last_speaker.as_deref(),
                                                                    font,
                                                                    &slots,
                                                                    focus,
                                                                    win_size.0 as u32,
                                                                    win_size.1 as u32,
                                                                    tint.as_deref(),
                                                                    effect.as_ref(),
                                                                    window,
                                                                    msg_plate.as_ref(),
                                                                    msg_frame.as_ref(),
                                                                    &plates,
                                                                    scene_look(&settings, bg_cg),
                                                                    vfont.as_mut(),
                                                                );
                                                                cur_surf = clone_surf(&base);
                                                            }
                                                        }
                                                    }
                                                },
                                            }
                                        } else {
                                            bar_hover = Some(cmd);
                                        }
                                        win.delay(16);
                                    } else {
                                        let hit = row_layout.iter().position(|row| {
                                            sy >= row.top && sy < row.top + row.height
                                        });
                                        if hit != hover {
                                            // The original plays its system UI sound
                                            // (SE_8905, the id the menu wrappers at
                                            // 0x40ea10/0x40ea30 pass to 0x4064a0)
                                            // when the cursor moves onto a choice.
                                            if hit.is_some() {
                                                play_ui_se(mixer.as_ref(), ui_se.as_deref());
                                            }
                                            hover = hit;
                                        }
                                        bar_hover = None;
                                        if pressed {
                                            if let Some(i) = hit {
                                                play_ui_se(mixer.as_ref(), ui_se.as_deref());
                                                break (i + 1) as u32;
                                            }
                                        }
                                        win.delay(16);
                                    }
                                }
                                sdl2::Input::Wheel { y } if y > 0 => {
                                    cmd_skip = false;
                                    cmd_auto = false;
                                    rewind_left += y.max(1);
                                }
                                sdl2::Input::Up => {
                                    cmd_skip = false;
                                    cmd_auto = false;
                                    rewind_left += 1;
                                }
                                sdl2::Input::Rewind => {
                                    cmd_rewind = !cmd_rewind;
                                    if cmd_rewind {
                                        cmd_skip = false;
                                        cmd_auto = false;
                                    } else {
                                        rewind_left = 0;
                                    }
                                }
                                _ => win.delay(16),
                            }
                        }
                    };
                    eprintln!("  choice: {} / {}", chosen, opts.len());
                    it.set_choice_result(chosen);
                }

                vm::lscr::Event::Wait => {
                    // Always draw the command bar, even in auto-advance modes, so
                    // the player can toggle auto/skip off or open save/load/config.
                    remember(
                        &mut history,
                        capture_line(
                            &current_id,
                            it.snapshot(),
                            bg_id,
                            effect_mode,
                            ambient_preset_id,
                            bg_cg,
                            &bg_label,
                            &tint_name,
                            &slots,
                            focus,
                            &last_text,
                            &last_speaker,
                            bgm_track.map(|(id, _)| id),
                            &plates,
                            line_voice,
                        ),
                    );
                    let mut cmd_hover: Option<cmdbar::Cmd> = None;
                    let mut dwell: u32 = 0;
                    'wait: loop {
                            // F-17: the option screen may have changed the
                            // font or the dialogue style. Repaint the line that
                            // is already on screen instead of leaving the old
                            // glyphs there until the next line. `last_text` is
                            // untouched - this redraws the same line.
                            if line_redraw {
                                line_redraw = false;
                                win_size = win.window_size();
                                line_base = None;
                                line_done = true;
                                cur_surf = draw_scene(
                                    &bg,
                                    &last_text,
                                    last_speaker.as_deref(),
                                    font,
                                    &slots,
                                    focus,
                                    win_size.0 as u32,
                                    win_size.1 as u32,
                                    tint.as_deref(),
                                    effect.as_ref(),
                                    window,
                                    msg_plate.as_ref(),
                                    msg_frame.as_ref(),
                                    &plates,
                                    scene_look(&settings, bg_cg),
                                    vfont.as_mut(),
                                );
                            }
                            if cmd_rewind || rewind_left > 0 {
                                let announce = !cmd_rewind && rewind_left <= 1;
                                let cruising = cmd_rewind || rewind_left > 1;
                                match take_rewind(&mut history, true) {
                                    None => {
                                        cmd_rewind = false;
                                        rewind_left = 0;
                                    }
                                    Some(dest) => {
                                        if rewind_left > 0 {
                                            rewind_left -= 1;
                                        }
                                        backlog.pop();
                                        let step = rewind_step(
                                            &dest,
                                            &mut it,
                                            &mut current_id,
                                            &mut rewind_vm,
                                            s,
                                            font,
                                            vfont.as_mut(),
                                            win,
                                            &mut bg,
                                            &mut effect,
                                            &mut effect_mode,
                                            &mut bg_cg,
                                            &mut bg_label,
                                            &mut bg_id,
                                            &mut tint,
                                            &mut tint_name,
                                            &mut slots,
                                            &mut bank,
                                            &mut focus,
                                            &mut plates,
                                            &mut last_text,
                                            &mut last_speaker,
                                            &mut ambient_preset,
                                            &mut ambient_preset_id,
                                            &mut ambient_override,
                                            &mut filter,
                                            user_hide,
                                            &mut window,
                                            &settings,
                                            msg_plate.as_ref(),
                                            msg_frame.as_ref(),
                                            &mut cur_surf,
                                            &mut line_base,
                                            &mut line_done,
                                            &mut reveal,
                                            mixer.as_ref(),
                                            &mut bgm_track,
                                            &mut fading_bgm,
                                            &mut voice_stream,
                                            voices.as_ref(),
                                            &mut line_voice,
                                            announce,
                                        );
                                        match step {
                                            RewindStep::Stuck => {}
                                            RewindStep::Shown => {
                                                present_rewind_frame(
                                                    win,
                                                    &cur_surf,
                                                    &bar,
                                                    cmd_hover,
                                                    cmdbar::Active {
                                                        auto: false,
                                                        skip: false,
                                                        rewind: cruising,
                                                        hide: user_hide,
                                                    },
                                                    cmd_idle(&settings),
                                                );
                                                if cruising {
                                                    match rewind_cruise_stop(win, &bar, &mut cmd_hover) {
                                                        Cruise::Quit => return Ok(None),
                                                        Cruise::Keep => {
                                                            win.delay(16);
                                                            continue 'wait;
                                                        }
                                                        Cruise::Stop => {}
                                                    }
                                                }
                                                cmd_rewind = false;
                                                rewind_left = 0;
                                                continue 'wait;
                                            }
                                            RewindStep::Reload => {
                                                present_rewind_frame(
                                                    win,
                                                    &cur_surf,
                                                    &bar,
                                                    cmd_hover,
                                                    cmdbar::Active {
                                                        auto: false,
                                                        skip: false,
                                                        rewind: cmd_rewind,
                                                        hide: user_hide,
                                                    },
                                                    cmd_idle(&settings),
                                                );
                                                park_wait = true;
                                                break 'events;
                                            }
                                        }
                                    }
                                }
                            }
                            if !line_done {
                                if cmd_skip {
                                    reveal = last_text.chars().count();
                                } else {
                                    type_tick += 1;
                                    let (step, every) = match settings.text_speed {
                                        1 => (2usize, 1u32),
                                        2 => (1, 2),
                                        _ => (1, 3),
                                    };
                                    if type_tick >= every {
                                        type_tick = 0;
                                        reveal = reveal.saturating_add(step);
                                    }
                                }
                                if reveal >= last_text.chars().count() {
                                    reveal = last_text.chars().count();
                                    line_done = true;
                                    dwell = 0;
                                }
                            }
                            if line_done {
                                if let Some(base) = line_base.take() {
                                    let mut full = clone_surf(&base);
                                    let (fw, fh) = (full.width, full.height);
                                    draw_body(
                                        &mut full,
                                        font,
                                        vfont.as_mut(),
                                        &last_text,
                                        None,
                                        msg_plate.is_some(),
                                        fw,
                                        fh,
                                        usize::MAX,
                                    );
                                    cur_surf = full;
                                }
                            }
                            let mut frame = if !line_done {
                                if let Some(base) = line_base.as_ref() {
                                    let mut partial = clone_surf(base);
                                    let (fw, fh) = (partial.width, partial.height);
                                    draw_body(
                                        &mut partial,
                                        font,
                                        vfont.as_mut(),
                                        &last_text,
                                        None,
                                        msg_plate.is_some(),
                                        fw,
                                        fh,
                                        reveal,
                                    );
                                    // Saves and the hide button read `cur_surf`, so it
                                    // has to be the text actually on screen.
                                    cur_surf = clone_surf(&partial);
                                    partial
                                } else {
                                    clone_surf(&cur_surf)
                                }
                            } else {
                                clone_surf(&cur_surf)
                            };
                            let active = cmdbar::Active {
                                auto: cmd_auto,
                                skip: cmd_skip,
                                rewind: cmd_rewind,
                                hide: user_hide,
                            };
                            let (fw, fh) = (frame.width as i32, frame.height as i32);
                            bar.draw(&mut frame, cmd_hover, active, cmd_idle(&settings), fw, fh);
                            win.present(&frame);
                            match win.poll_input() {
                                sdl2::Input::Quit => return Ok(None),
                                // F11: toggle 画面模式 without opening the option
                                // screen. The decode scale follows on the next
                                // loop iteration.
                                sdl2::Input::Fullscreen => {
                                    let on = settings.win_disp != 1;
                                    settings.win_disp = if on { 1 } else { 0 };
                                    win.set_fullscreen(on);
                                    set_render_art_scale(win);
                                    if let Err(e) = settings.save() {
                                        eprintln!("settings: save failed: {e}");
                                    }
                                    eprintln!(
                                        "display: {} (decode {}x)",
                                        if on { "fullscreen" } else { "windowed" },
                                        render_art_scale()
                                    );
                                }
                                sdl2::Input::RightClick => {
                                    // Right-click is the direct hide-dialog
                                    // gesture in gameplay. The old configurable
                                    // menu/save actions made the same gesture
                                    // unpredictable and differed from the
                                    // command-bar Hide button.
                                    user_hide = !user_hide;
                                    window = dialog_open(user_hide);
                                    line_base = None;
                                    line_done = true;
                                    cur_surf = draw_scene(
                                        &bg,
                                        &last_text,
                                        last_speaker.as_deref(),
                                        font,
                                        &slots,
                                        focus,
                                        win_size.0 as u32,
                                        win_size.1 as u32,
                                        tint.as_deref(),
                                        effect.as_ref(),
                                        window,
                                        msg_plate.as_ref(),
                                        msg_frame.as_ref(),
                                        &plates,
                                        scene_look(&settings, bg_cg),
                                        vfont.as_mut(),
                                    );
                                }
                                sdl2::Input::Pointer { x, y, pressed } => {
                                    let (sx, sy) = win.surface_pos(x, y, W, H);
                                    // In fast-forward mode every left click is
                                    // an exit gesture, regardless of where it
                                    // lands. Do this before command-bar hit
                                    // testing so the player cannot need a
                                    // second click to leave skip.
                                    if cmd_skip && pressed {
                                        cmd_skip = false;
                                        window = dialog_open(user_hide);
                                        if let Some(mx) = mixer.as_ref() {
                                            if let (Some(cue), Some(vi)) = (skipped_voice.take(), voices.as_ref()) {
                                                play_voice(mx, vi, &settings, &mut voice_stream, cue, &current_id);
                                            }
                                        }
                                        continue 'wait;
                                    }
                                    match bar.hit(sx, sy) {
                                        Some(cmd) if pressed => match cmd {
                                            cmdbar::Cmd::Auto => {
                                                cmd_auto = !cmd_auto;
                                                if cmd_auto {
                                                    cmd_rewind = false;
                                                    rewind_left = 0;
                                                }
                                            }
                                            cmdbar::Cmd::Rewind => {
                                                cmd_rewind = !cmd_rewind;
                                                if cmd_rewind {
                                                    cmd_skip = false;
                                                    cmd_auto = false;
                                                    window = dialog_open(user_hide);
                                                    continue 'wait;
                                                }
                                                rewind_left = 0;
                                            }
                                            cmdbar::Cmd::Skip => {
                                                cmd_skip = !cmd_skip;
                                                if cmd_skip {
                                                    cmd_rewind = false;
                                                    rewind_left = 0;
                                                }
                                                window = dialog_open(user_hide);
                                                if let Some(mx) = mixer.as_ref() {
                                                    if cmd_skip {
                                                        stop_voice(mx, &mut voice_stream);
                                                    } else if let (Some(cue), Some(vi)) = (skipped_voice.take(), voices.as_ref()) {
                                                        play_voice(mx, vi, &settings, &mut voice_stream, cue, &current_id);
                                                    }
                                                }
                                            },
                                            cmdbar::Cmd::Hide => {
                                                user_hide = !user_hide;
                                                window = dialog_open(user_hide);
                                                line_base = None;
                                                line_done = true;
                                                cur_surf = draw_scene(
                                                    &bg,
                                                    &last_text,
                                                    last_speaker.as_deref(),
                                                    font,
                                                    &slots,
                                                    focus,
                                                    win_size.0 as u32,
                                                    win_size.1 as u32,
                                                    tint.as_deref(),
                                                    effect.as_ref(),
                                                    window,
                                                    msg_plate.as_ref(),
                                                    msg_frame.as_ref(),
                                                    &plates,
                                                    scene_look(&settings, bg_cg),
                                                    vfont.as_mut(),
                                                );
                                            }
                                            other => {
                                                let is_config = other == cmdbar::Cmd::Config;
                                                match handle_cmd(
                                                    other,
                                                    win,
                                                    font,
                                                    vfont.as_mut(),
                                                    s,
                                                    &mut settings,
                                                    mixer.as_ref(),
                                                    &it,
                                                    &current_id,
                                                    bg_id,
                                                    effect_mode,
                                                    ambient_preset_id,
                                                    &tint_name,
                                                    &slots,
                                                    focus,
                                                    &last_text,
                                                    bgm_track.map(|(id, _)| id),
                                                    &cur_surf,
                                                    &backlog,
                                                ) {
                                                    CmdOutcome::Loaded(st) => return Ok(Some(st)),
                                                    CmdOutcome::Jump(id) => {
                                                        if let Some(mx) = mixer.as_ref() {
                                                            stop_voice(mx, &mut voice_stream);
                                                            if let Some((_, slot)) = bgm_track.take() {
                                                                mx.stop(slot);
                                                            }
                                                            for slot in se_channels.drain().map(|(_, slot)| slot) {
                                                                mx.stop(slot);
                                                            }
                                                            for slot in fading_se.drain().map(|(_, slot)| slot) {
                                                                mx.stop(slot);
                                                            }
                                                            if let Some(slot) = fading_bgm.take() {
                                                                mx.stop(slot);
                                                            }
                                                        }
                                                        current_id = id;
                                                        cmd_auto = false;
                                                        cmd_skip = false;
                                                        cmd_rewind = false;
                                                        rewind_left = 0;
                                                        rewind_vm = None;
                                                        park_wait = false;
                                                        skipped_voice = None;
                                                        line_voice = None;
                                                        line_no = 0;
                                                        history.clear();
                                                        user_hide = false;
                                                        window = true;
                                                        tint = None;
                                                        tint_name = None;
                                                        ambient_preset = None;
                                                        ambient_preset_id = 0;
                                                        ambient_override = None;
                                                        filter = None;
                                                        effect_mode = 0;
                                                        effect = None;
                                                        bg = solid_bg(W, H, [0, 0, 0]);
                                                        bg_scale = None;
                                                        bg_src_scale = None;
                                                        bg_id = None;
                                                        bg_label = String::from("BLACK");
                                                        bg_cg = false;
                                                        last_text.clear();
                                                        last_speaker = None;
                                                        cur_surf = render::render_scene(&bg, "", font, W, H);
                                                        line_redraw = false;
                                                        reveal = 0;
                                                        type_tick = 0;
                                                        slots = Default::default();
                                                        plates.clear();
                                                        line_base = None;
                                                        line_done = true;
                                                        focus = None;
                                                        break 'events;
                                                    }
                                                    CmdOutcome::Home | CmdOutcome::Quit => {
                                                        stop_game_audio(
                                                            mixer.as_ref(),
                                                            &mut bgm_track,
                                                            &mut fading_bgm,
                                                            &mut voice_stream,
                                                            &mut se_channels,
                                                            &mut fading_se,
                                                        );
                                                        return Ok(None);
                                                    }
                                                    CmdOutcome::Stay => {
                                                        // F-17: the option screen
                                                        // may have changed the
                                                        // font or the style.
                                                        if is_config {
                                                            line_redraw = true;
                                                        }
                                                    }
                                                }
                                            }
                                        },
                                        Some(c) => cmd_hover = Some(c),
                                        None => {
                                            cmd_hover = None;
                                            if pressed {
                                                if cmd_rewind || rewind_left > 0 {
                                                    cmd_rewind = false;
                                                    rewind_left = 0;
                                                } else if !line_done {
                                                    reveal = last_text.chars().count();
                                                    line_done = true;
                                                    dwell = 0;
                                                } else {
                                                    break 'wait;
                                                }
                                            }
                                        }
                                    }
                                },
                                sdl2::Input::Advance | sdl2::Input::Right => {
                                    if cmd_rewind || rewind_left > 0 {
                                        cmd_rewind = false;
                                        rewind_left = 0;
                                    } else if !line_done {
                                        reveal = last_text.chars().count();
                                        line_done = true;
                                        dwell = 0;
                                    } else {
                                        break 'wait;
                                    }
                                }
                                // Wheel down is the next line. Wheel up rewinds.
                                sdl2::Input::Wheel { y } if y < 0 => {
                                    if cmd_rewind || rewind_left > 0 {
                                        cmd_rewind = false;
                                        rewind_left = 0;
                                    } else {
                                        if !line_done {
                                            reveal = last_text.chars().count();
                                            line_done = true;
                                        }
                                        break 'wait;
                                    }
                                }
                                sdl2::Input::Wheel { y } if y > 0 => {
                                    cmd_skip = false;
                                    cmd_auto = false;
                                    rewind_left += y.max(1);
                                    continue 'wait;
                                }
                                sdl2::Input::Up => {
                                    cmd_skip = false;
                                    cmd_auto = false;
                                    rewind_left += 1;
                                    continue 'wait;
                                }
                                sdl2::Input::Rewind => {
                                    cmd_rewind = !cmd_rewind;
                                    if cmd_rewind {
                                        cmd_skip = false;
                                        cmd_auto = false;
                                        continue 'wait;
                                    }
                                    rewind_left = 0;
                                }
                                // No new event: keep the hover caption where it is.
                                sdl2::Input::None => {}
                                _ => cmd_hover = None,
                            }
                            if line_done && (auto_text || cmd_auto || cmd_skip) {
                                dwell += 16;
                                // Skip ignores the slider. Auto holds for the
                                // menu's 「自动模式速度」, and a line that has
                                // voice stays up until that voice finishes.
                                let lim = if cmd_skip {
                                    6
                                } else {
                                    menu::auto_hold_ms(settings.auto_speed, last_text.chars().count())
                                };
                                let voice_open = cmd_auto && !cmd_skip && voice_stream
                                    .map(|slot| mixer.as_ref().is_some_and(|mx| !mx.finished(slot)))
                                    .unwrap_or(false);
                                if !voice_open && dwell >= lim {
                                    break 'wait;
                                }
                            }
                            win.delay(16);
                        }
                        note_seen(&mut seen, &current_id, &last_text);
                    }
                vm::lscr::Event::Jump { target, kind } => {
                    eprintln!("  {} -> {}", kind, target);
                    // The clip from the scene we just skipped must not keep
                    // playing over the next one.
                    if cmd_skip {
                        skipped_voice = None;
                        if let Some(mx) = mixer.as_ref() {
                            stop_voice(mx, &mut voice_stream);
                        }
                    }
                    current_id = target;
                    line_no = 0;
                    // New scene: standing sprites do not carry over, but keep
                    // the decoded sprite bank.  Clearing it here forces every
                    // character in the next scene through WebP/TGA decoding
                    // again and made ordinary scene jumps look like a load.
                    slots = Default::default();
                    plates.clear();
                    effect_mode = 0;
                    effect = None;
                    line_base = None;
                    line_done = true;
                    focus = None;
                    break;
                }
                vm::lscr::Event::End => {
                    eprintln!("scene {} ended", current_id);
                    return Ok(None);
                }
            }
        }
    }
}

/// One line the player already stood on. Rewind restores this instead of
/// replaying the script backwards.
#[derive(Clone)]
struct LineSnap {
    scene: String,
    vm: vm::lscr::InterpState,
    bg: Option<(i64, u32)>,
    effect_mode: i32,
    ambient_preset: i32,
    bg_cg: bool,
    bg_label: String,
    tint_name: Option<String>,
    slots: Vec<Option<(u32, String)>>,
    focus: Option<u32>,
    text: String,
    speaker: Option<String>,
    bgm: Option<i32>,
    plates: Vec<(u32, String)>,
    voice: Option<vm::lscr::VoiceCue>,
}

const HISTORY_MAX: usize = 400;

fn slot_keys(slots: &[Option<ActiveSprite>; SPRITE_SLOTS]) -> Vec<Option<(u32, String)>> {
    slots.iter().map(|s| s.as_ref().map(|a| (a.char_id, a.name.clone()))).collect()
}

fn plate_keys(plates: &[Plate]) -> Vec<(u32, String)> {
    plates.iter().map(|p| (p.slot, p.name.clone())).collect()
}

fn capture_line(
    scene: &str,
    vm: vm::lscr::InterpState,
    bg: Option<(i64, u32)>,
    effect_mode: i32,
    ambient_preset: i32,
    bg_cg: bool,
    bg_label: &str,
    tint_name: &Option<String>,
    slots: &[Option<ActiveSprite>; SPRITE_SLOTS],
    focus: Option<u32>,
    text: &str,
    speaker: &Option<String>,
    bgm: Option<i32>,
    plates: &[Plate],
    voice: Option<vm::lscr::VoiceCue>,
) -> LineSnap {
    LineSnap {
        scene: scene.to_string(),
        vm,
        bg,
        effect_mode,
        ambient_preset,
        bg_cg,
        bg_label: bg_label.to_string(),
        tint_name: tint_name.clone(),
        slots: slot_keys(slots),
        focus,
        text: text.to_string(),
        speaker: speaker.clone(),
        bgm,
        plates: plate_keys(plates),
        voice,
    }
}

fn remember(history: &mut Vec<LineSnap>, snap: LineSnap) {
    let same = history.last().is_some_and(|h| {
        h.scene == snap.scene && h.vm.pc == snap.vm.pc && h.text == snap.text
    });
    if same {
        return;
    }
    history.push(snap);
    if history.len() > HISTORY_MAX {
        history.remove(0);
    }
}

/// `pop_current` drops the line on screen (a text wait). A choice has not
/// pushed its own snapshot, so rewind restores the line already at the top.
fn take_rewind<T: Clone>(history: &mut Vec<T>, pop_current: bool) -> Option<T> {
    if pop_current {
        if history.len() < 2 {
            return None;
        }
        history.pop();
    } else if history.is_empty() {
        return None;
    }
    history.last().cloned()
}

enum RewindStep {
    /// Nothing earlier than the line on screen.
    Stuck,
    /// Picture and interpreter are back on an earlier line of this script.
    Shown,
    /// The earlier line belongs to another script. `rewind_vm` is set.
    Reload,
}

/// Put `dest` on screen and arm the interpreter when the script matches.
fn rewind_step(
    dest: &LineSnap,
    it: &mut vm::lscr::Interp,
    current_id: &mut String,
    rewind_vm: &mut Option<vm::lscr::InterpState>,
    s: &Session,
    font: &render::Font,
    vfont: Option<&mut render::text::VectorFont>,
    win: &mut sdl2::Sdl,
    bg: &mut codec::tga::TgaImage,
    effect: &mut Option<codec::tga::TgaImage>,
    effect_mode: &mut i32,
    bg_cg: &mut bool,
    bg_label: &mut String,
    bg_id: &mut Option<(i64, u32)>,
    tint: &mut Option<Vec<u8>>,
    tint_name: &mut Option<String>,
    slots: &mut [Option<ActiveSprite>; SPRITE_SLOTS],
    bank: &mut Option<SpriteBank>,
    focus: &mut Option<u32>,
    plates: &mut Vec<Plate>,
    last_text: &mut String,
    last_speaker: &mut Option<String>,
    ambient_preset: &mut Option<String>,
    ambient_preset_id: &mut i32,
    ambient_override: &mut Option<String>,
    filter: &mut Option<String>,
    user_hide: bool,
    window: &mut bool,
    settings: &menu::Settings,
    msg_plate: Option<&codec::tga::TgaImage>,
    msg_frame: Option<&codec::tga::TgaImage>,
    cur_surf: &mut render::Surface,
    line_base: &mut Option<render::Surface>,
    line_done: &mut bool,
    reveal: &mut usize,
    mixer: Option<&audio::Mixer>,
    bgm_track: &mut Option<(i32, usize)>,
    fading_bgm: &mut Option<usize>,
    voice_stream: &mut Option<usize>,
    voices: Option<&VoiceIndex>,
    line_voice: &mut Option<vm::lscr::VoiceCue>,
    announce: bool,
) -> RewindStep {
    if mixer.is_some() {
        stop_voice(mixer.unwrap(), voice_stream);
    }
    if bg_id != &dest.bg || bg_label != &dest.bg_label {
        if let Some((id, variant)) = dest.bg {
            let name = if dest.bg_cg {
                event_cg_name(id, variant)
            } else {
                s.bg_index.resolve(id, variant).unwrap_or_else(|| dest.bg_label.clone())
            };
            if let Some(img) = load_background(&s.bg_dirs(), &name) {
                *bg = img;
            }
        } else {
            *bg = solid_bg(1280, 720, [0, 0, 0]);
        }
        *bg_id = dest.bg;
        *bg_cg = dest.bg_cg;
        *bg_label = dest.bg_label.clone();
    }
    if *effect_mode != dest.effect_mode {
        *effect_mode = dest.effect_mode;
        *effect = load_effect(&s.bg_dirs(), *effect_mode);
    }
    if tint_name != &dest.tint_name {
        *tint_name = dest.tint_name.clone();
        *tint = dest.tint_name.as_deref().and_then(|n| load_tint(&s.bg_dirs(), n));
    }
    *ambient_preset_id = dest.ambient_preset;
    *ambient_preset = ambient_preset_name(*ambient_preset_id);
    *ambient_override = None;
    *filter = None;
    if slot_keys(slots) != dest.slots {
        *slots = Default::default();
        for (i, entry) in dest.slots.iter().enumerate() {
            let Some((char_id, name)) = entry else { continue };
            if let Some(bank) = bank.as_mut() {
                let Some((img, art_scale, src_scale)) = bank.get(name) else {
                    continue;
                };
                if let Some(dst) = slots.get_mut(i) {
                    *dst = Some(ActiveSprite {
                        char_id: *char_id,
                        name: name.clone(),
                        img,
                        art_scale,
                        src_scale,
                    });
                }
            }
        }
    }
    if plate_keys(plates) != dest.plates {
        plates.clear();
        for (slot, name) in &dest.plates {
            if let Some(asset) = load_background_asset(&s.bg_dirs(), name) {
                plates.push(Plate { slot: *slot, name: name.clone(), img: asset.img, art_scale: asset.art_scale });
            }
        }
    }
    *focus = dest.focus;
    *last_text = dest.text.clone();
    *last_speaker = dest.speaker.clone();
    // Keep the message box up, so the function buttons stay on the window
    // instead of vanishing into the picture.
    *window = dialog_open(user_hide);
    *line_voice = dest.voice;
    *line_base = None;
    *line_done = true;
    *reveal = last_text.chars().count();
    if let Some(mx) = mixer {
        if bgm_track.map(|(id, _)| id) != dest.bgm {
            if let Some(slot) = fading_bgm.take() {
                mx.stop(slot);
            }
            if let Some((_, slot)) = bgm_track.take() {
                mx.stop(slot);
            }
            if let Some(id) = dest.bgm {
                let dir = s.script_dir.parent().unwrap_or(std::path::Path::new(".")).join("bgm");
                let stem = format!("BGM_{:03}", id);
                let path = [format!("{}.OGG", stem), format!("{}_A.OGG", stem)]
                    .into_iter()
                    .map(|n| dir.join(n))
                    .find(|p| p.is_file());
                if let Some(path) = path {
                    let slot = mx.play_cat(&path.to_string_lossy(), 1.0, true, audio::mixer::CAT_BGM);
                    if slot != usize::MAX {
                        *bgm_track = Some((id, slot));
                    }
                }
            }
        }
    }
    let win_size = win.window_size();
    *cur_surf = draw_scene(
        bg,
        last_text,
        last_speaker.as_deref(),
        font,
        slots,
        *focus,
        win_size.0 as u32,
        win_size.1 as u32,
        tint.as_deref(),
        effect.as_ref(),
        *window,
        msg_plate,
        msg_frame,
        plates,
        scene_look(settings, *bg_cg),
        vfont,
    );
    if announce {
        if let (Some(mx), Some(vi), Some(cue)) = (mixer, voices, dest.voice) {
            play_voice(mx, vi, settings, voice_stream, cue, &dest.scene);
        }
    }
    eprintln!("rewind: scene {} pc {}", dest.scene, dest.vm.pc);
    if dest.scene == *current_id {
        it.restore(dest.vm.clone());
        RewindStep::Shown
    } else {
        *current_id = dest.scene.clone();
        *rewind_vm = Some(dest.vm.clone());
        RewindStep::Reload
    }
}

/// Snapshot of the engine for a save slot, taken while it waits for input.
///
/// Only ids and names go in: the decoded background, the tint LUT, the sprite
/// images and the mixer slots are all re-created on load.
fn make_save(
    it: &vm::lscr::Interp,
    scene: &str,
    bg: Option<(i64, u32)>,
    effect_mode: i32,
    ambient_preset: i32,
    tint: Option<String>,
    slots: &[Option<ActiveSprite>; SPRITE_SLOTS],
    focus: Option<u32>,
    text: &str,
    bgm: Option<i32>,
) -> save::SaveState {
    save::SaveState {
        scene: scene.to_string(),
        vm: it.snapshot(),
        bg,
        effect_mode,
        ambient_preset,
        tint,
        slots: slots
            .iter()
            .map(|s| s.as_ref().map(|a| (a.char_id, a.name.clone())))
            .collect(),
        focus,
        text: text.to_string(),
        bgm,
        time: save::now(),
        globals: it.global_snapshot(),
    }
}

/// Result of handling a command-bar button that opens a modal screen.
enum CmdOutcome {
    /// Screen closed, keep waiting for input.
    Stay,
    /// A save slot was loaded: restart the run with it.
    Loaded(save::SaveState),
    /// The timeline selected a scene to play from its entry point.
    Jump(String),
    /// Return from gameplay to the title screen.
    Home,
    /// Player chose to quit.
    Quit,
}

fn centered_plate(w: i32, h: i32) -> (i32, i32) {
    ((1280 - w) / 2, (720 - h) / 2)
}

fn in_box(sx: i32, sy: i32, b: (i32, i32, i32, i32)) -> bool {
    sx >= b.0 && sx < b.0 + b.2 && sy >= b.1 && sy < b.1 + b.3
}

/// Yes on the left, no on the right, along the bottom of the confirm plate.
fn ask_buttons(px: i32, py: i32, pw: i32, ph: i32) -> [(i32, i32, i32, i32); 2] {
    let bw = 160;
    let bh = 40;
    let x = px + (pw - bw * 2) / 2;
    let y = py + ph - bh - 28;
    [(x, y, bw, bh), (x + bw, y, bw, bh)]
}

fn blit_line(surf: &mut render::Surface, text: Option<&(u32, u32, Vec<u8>)>, px: i32, py: i32, pw: i32, ph: i32) {
    if let Some((tw, th, rgba)) = text {
        let x = px + (pw - *tw as i32) / 2;
        let y = py + (ph - *th as i32) / 2 - 8;
        surf.blit_rgba(rgba, *tw, *th, x.max(px + 8), y.max(py + 8));
    }
}

fn paint_toast(
    frame: &render::Surface,
    plate: Option<&codec::tga::TgaImage>,
    text: Option<&(u32, u32, Vec<u8>)>,
) -> render::Surface {
    let mut s = clone_surf(frame);
    s.dim(110);
    let (pw, ph) = plate
        .map(|p| (p.width as i32, p.height as i32))
        .unwrap_or((480, 112));
    let (px, py) = centered_plate(pw, ph);
    if let Some(p) = plate {
        s.blit(p, px, py);
    } else {
        s.fill_rect(px, py, pw as u32, ph as u32, [16, 90, 104, 255]);
    }
    blit_line(&mut s, text, px, py, pw, ph);
    s
}

fn paint_confirm(
    frame: &render::Surface,
    plate: Option<&codec::tga::TgaImage>,
    buttons: Option<&codec::tga::TgaImage>,
    text: Option<&(u32, u32, Vec<u8>)>,
    hover: Option<bool>,
) -> render::Surface {
    let mut s = clone_surf(frame);
    s.dim(140);
    let (pw, ph) = plate
        .map(|p| (p.width as i32, p.height as i32))
        .unwrap_or((544, 216));
    let (px, py) = centered_plate(pw, ph);
    if let Some(p) = plate {
        s.blit(p, px, py);
    } else {
        s.fill_rect(px, py, pw as u32, ph as u32, [16, 90, 104, 255]);
    }
    // The prompt sits in the upper half; the 是/否 row owns the bottom.
    if let Some((tw, th, rgba)) = text {
        let x = px + (pw - *tw as i32) / 2;
        s.blit_rgba(rgba, *tw, *th, x.max(px + 16), py + 48);
    }
    if let Some(img) = buttons {
        let boxes = ask_buttons(px, py, pw, ph);
        for (i, rect) in boxes.iter().enumerate() {
            let hot = hover == Some(i == 0);
            let row = if hot { 1 } else { 0 };
            s.blit_region(img, (i as i32) * 160, row * 40, 160, 40, rect.0, rect.1);
        }
    }
    s
}

/// Short confirmation that a quick save landed. A click dismisses it early.
/// `false` means the player closed the window.
fn show_toast(
    win: &mut sdl2::Sdl,
    frame: &render::Surface,
    dirs: &[&std::path::Path],
    message: &str,
) -> bool {
    let plate = load_background(dirs, "sys_00110.tga");
    let text = render::text::with_ui_font(22, |f| f.line_rgba(message, 22)).flatten();
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
fn ask_yes_no(
    win: &mut sdl2::Sdl,
    frame: &render::Surface,
    dirs: &[&std::path::Path],
    message: &str,
    mixer: Option<&audio::Mixer>,
    ui: Option<&UiSound>,
) -> Option<bool> {
    let plate = load_background(dirs, "sys_00100.tga");
    let buttons = load_background(dirs, "sys_00101.tga");
    let text = render::text::with_ui_font(22, |f| f.line_rgba(message, 22)).flatten();
    let (pw, ph) = plate
        .as_ref()
        .map(|p| (p.width as i32, p.height as i32))
        .unwrap_or((544, 216));
    let (px, py) = centered_plate(pw, ph);
    let boxes = ask_buttons(px, py, pw, ph);
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
        let surf = paint_confirm(frame, plate.as_ref(), buttons.as_ref(), text.as_ref(), hover);
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
                let (sx, sy) = win.surface_pos(x, y, 1280, 720);
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

/// Handle a command-bar button that opens a modal screen (保存/读档/设置/履历).
/// The toggle buttons (自动/跳过/隐藏) are handled inline by the caller.
fn handle_cmd(
    cmd: cmdbar::Cmd,
    win: &mut sdl2::Sdl,
    font: &render::Font,
    // The history screen is the last modal that still drew from the atlas;
    // the vector font rides along so it matches the dialog it replays. It is
    // the Game role - the History role is opened by the backlog itself.
    mut vfont: Option<&mut render::text::VectorFont>,
    s: &Session,
    settings: &mut menu::Settings,
    mixer: Option<&audio::Mixer>,
    it: &vm::lscr::Interp,
    current_id: &str,
    bg_id: Option<(i64, u32)>,
    effect_mode: i32,
    ambient_preset: i32,
    tint_name: &Option<String>,
    slots: &[Option<ActiveSprite>; SPRITE_SLOTS],
    focus: Option<u32>,
    last_text: &str,
    bgm: Option<i32>,
    frame: &render::Surface,
    backlog: &[String],
) -> CmdOutcome {
    match cmd {
        cmdbar::Cmd::QuickSave => {
            let cn = s.script_dir.file_name().and_then(|n| n.to_str()) == Some("script_cn");
            let st = make_save(
                it,
                current_id,
                bg_id,
                effect_mode,
                ambient_preset,
                tint_name.clone(),
                slots,
                focus,
                last_text,
                bgm,
            );
            let ok = st.save_to(&save::quick_path()).is_ok();
            let message = match (cn, ok) {
                (true, true) => "已快速保存",
                (true, false) => "快速保存失败",
                (false, true) => "クイックセーブしました",
                (false, false) => "クイックセーブに失敗しました",
            };
            eprintln!("scene {} | quick save {}", current_id, if ok { "ok" } else { "FAILED" });
            if show_toast(win, frame, &s.bg_dirs(), message) {
                CmdOutcome::Stay
            } else {
                CmdOutcome::Quit
            }
        }
        cmdbar::Cmd::QuickLoad => {
            let cn = s.script_dir.file_name().and_then(|n| n.to_str()) == Some("script_cn");
            let Some(st) = save::SaveState::load_from(&save::quick_path()) else {
                let message = if cn { "没有快速存档" } else { "クイックセーブがありません" };
                eprintln!("scene {} | quick load: no file", current_id);
                return if show_toast(win, frame, &s.bg_dirs(), message) {
                    CmdOutcome::Stay
                } else {
                    CmdOutcome::Quit
                };
            };
            let message = if cn { "要读取快速存档吗？" } else { "クイックロードしますか？" };
            let dirs = s.bg_dirs();
            let ui = UiSound::resolve(&dirs);
            match ask_yes_no(win, frame, &dirs, message, mixer, Some(&ui)) {
                Some(true) => {
                    eprintln!("scene {} | quick load confirmed", current_id);
                    CmdOutcome::Loaded(st)
                }
                Some(false) => CmdOutcome::Stay,
                None => CmdOutcome::Quit,
            }
        }
        cmdbar::Cmd::Save => {
            let st = make_save(
                it,
                current_id,
                bg_id,
                effect_mode,
                ambient_preset,
                tint_name.clone(),
                slots,
                focus,
                last_text,
                bgm,
            );
            let dirs = s.bg_dirs();
            let ui = UiSound::resolve(&dirs);
            saveload::run(
                win,
                &dirs,
                saveload::Mode::Save,
                Some(st),
                Some(frame),
                mixer,
                Some(&ui),
            );
            CmdOutcome::Stay
        }
        cmdbar::Cmd::Load => {
            if let Some(st) =
                {
                    let dirs = s.bg_dirs();
                    let ui = UiSound::resolve(&dirs);
                    saveload::run(
                        win,
                        &dirs,
                        saveload::Mode::Load,
                        None,
                        Some(frame),
                        mixer,
                        Some(&ui),
                    )
                }
            {
                CmdOutcome::Loaded(st)
            } else {
                CmdOutcome::Stay
            }
        }
        cmdbar::Cmd::Config => {
            let mut st = menu::Settings::load();
            let dirs = s.bg_dirs();
            let ui = UiSound::resolve(&dirs);
            menu::run(win, &dirs, &mut st, mixer, Some(&ui));
            *settings = menu::Settings::load();
            menu::apply(settings, mixer);
            refresh_dialog_font(s.cn, vfont.as_deref_mut());
            win.set_fullscreen(settings.win_disp == 1);
            set_render_art_scale(win);
            CmdOutcome::Stay
        }
        cmdbar::Cmd::Backlog => {
            // The history draws with its own role, opened here so a change
            // made in the settings screen is in force the next time it is
            // opened and no glyph of the old font can be left in it.
            let hf = if s.cn {
                render::text::open_role_font(render::text::FontRole::History, render::DIALOG_FONT_720)
            } else {
                None
            };
            show_backlog(win, font, hf, backlog, frame, &s.bg_dirs());
            CmdOutcome::Stay
        }
        cmdbar::Cmd::Timeline => {
            let dirs = s.bg_dirs();
            let ui = UiSound::resolve(&dirs);
            if let Some(id) = crate::timeline::run(
                win,
                font,
                &dirs,
                s.script_dir.as_path(),
                s.cn,
                mixer,
                Some(&ui),
                Some(current_id),
            ) {
                eprintln!("timeline: selected {} from scene {}", id, current_id);
                CmdOutcome::Jump(id)
            } else {
                CmdOutcome::Stay
            }
        }
        cmdbar::Cmd::Home => CmdOutcome::Home,
        // toggles are handled by the caller
        _ => CmdOutcome::Stay,
    }
}

/// Replace the dialogue font with the Game role's current choice.
///
/// Called after the option screen closes. A new instance starts with an empty
/// glyph cache, which is the point: the next line is drawn entirely in the new
/// face instead of mixing cached glyphs of the old one into it.
fn refresh_dialog_font(cn: bool, vfont: Option<&mut render::text::VectorFont>) {
    if !cn {
        return;
    }
    let Some(dst) = vfont else { return };
    if let Some(nf) = render::text::open_role_font(render::text::FontRole::Game, render::DIALOG_FONT_720) {
        *dst = nf;
    }
}

/// One visual row of the text history. An empty text is the gap between two
/// logged lines; a name row is drawn with the speaker style so the history
/// reads like the dialog it came from.
struct BacklogRow {
    text: String,
    name: bool,
}

/// Wrap the history into rows. With a vector font the wrap follows the font's
/// own advances instead of the atlas cell pitch, so a Chinese line stops using
/// the Japanese layout's 28px-per-character rhythm.
fn backlog_rows(lines: &[String], vf: Option<&render::text::VectorFont>) -> Vec<BacklogRow> {
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

fn push_wrapped(
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

fn paint_backlog(
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
fn show_backlog(
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

/// The Chinese patch keeps its text in `script_cn`: Shift-JIS codes whose
/// glyphs were repainted, which `codec::sjis::decode_cn` resolves.
fn is_cn_script(dir: &std::path::Path) -> bool {
    dir.file_name().and_then(|n| n.to_str()) == Some("script_cn")
}

/// Load and parse an LSCR program (`.bnr` + companion `.txt` token pool).
fn load_program(script_dir: &std::path::Path, id: &str, cn: bool) -> Option<vm::lscr::Program> {
    let bnr = std::fs::read(script_dir.join(format!("{}.bnr", id))).ok()?;
    let txt = std::fs::read(script_dir.join(format!("{}.txt", id))).ok()?;
    match vm::lscr::Program::load(&bnr, &txt, cn) {
        Ok(p) => Some(p),
        Err(e) => {
            eprintln!("lscr parse error for {}: {}", id, e);
            None
        }
    }
}

/// Locate a directory of loose character sprites (legacy layout helper).
///
/// The data tree passes its own sprite directory explicitly; this is only used
/// by the explicit-path CLI form, where it tries the common layouts.
fn find_sprite_dir(bg_dir: &std::path::Path, script_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("WA2_SPRITES") {
        let pb = std::path::PathBuf::from(p);
        if pb.is_dir() {
            return Some(pb);
        }
    }
    let mut cands: Vec<std::path::PathBuf> = Vec::new();
    for base in [
        bg_dir.to_path_buf(),
        bg_dir.join(".."),
        script_dir.to_path_buf(),
        script_dir.join(".."),
    ] {
        cands.push(base.join("sprite"));
        cands.push(base.join("char"));
    }
    cands.into_iter().find(|p| p.is_dir())
}

/// Character id -> sprite file prefix.
///
/// Taken from the game exe's own prefix table (the NUL-separated list at file
/// offset 661252: `mas yan miy saw suz kiz mat ham yos ueh mhh mih aco say nak
/// hon sat tom shi you oto mam pap chi ioo tak mar izu koh set kaz har tan`),
/// cross-checked against every `(series, variant)` request in the shipped
/// scripts: for ids 1,2,3,4,5,10,11,15,16,19,23,25,28 the file set matches
/// exactly one prefix, and the rest follow the table order.
///
/// The previous hand-written table had 12 and 17 both pointing at `oto` (= 孝宏),
/// which made those two characters render as the same person, and 18 at `kaz`.
fn sprite_prefixes(cid: u32) -> &'static [&'static str] {
    match cid {
        0 => &["har"],  // 春希
        1 => &["kaz"],  // 冬馬かずさ
        2 => &["set"],  // 小木曽雪菜
        3 => &["koh"],  // 杉浦小春
        4 => &["izu"],  // 和泉千晶
        5 => &["mar"],  // 風岡麻理
        10 => &["tak"], // 飯塚武也
        11 => &["ioo"], // 水沢依緒
        12 => &["chi"], // 親志
        13 => &["pap"], // 雪菜の父
        14 => &["mam"], // 雪菜の母
        15 => &["oto"], // 孝宏
        16 => &["you"], // 冬馬曜子
        17 => &["shi"], // 担任教師
        19 => &["tom"], // 朋
        20 => &["sat"], // 佐藤
        21 => &["hon"], // 本田
        22 => &["nak"], // 中川
        23 => &["say"], // 早百合
        24 => &["aco"], // 亜子
        25 => &["mih"], // 美穂子
        26 => &["mhh"],
        27 => &["ueh"], // 上原
        28 => &["yos"],
        30 => &["ham"], // 浜田
        31 => &["mat"], // 松岡
        32 => &["kiz"], // 木崎
        33 => &["suz"], // 鈴木
        34 => &["saw"],
        35 => &["miy"],
        36 => &["yan"],
        _ => &[],
    }
}

/// Character sprite bank backed by a directory of loose TGA or WebP files.
///
/// On construction it indexes the directory once, so resolution is an in-memory
/// lookup. (The previous archive-backed version sliced filenames by byte index,
/// which panicked on the archives' one non-ASCII entry name; a directory scan
/// and a prefix rule that walks `char`s cannot panic.)
struct SpriteBank {
    /// Scaled sprite trees, ascending; the decode scale picks the active one.
    trees: Vec<(u32, std::path::PathBuf)>,
    dir: std::path::PathBuf,
    /// Scale of the pixels in `dir`; 0 before the first retarget.
    scale: u32,
    /// lowercased file names present in the directory
    index: std::collections::HashSet<String>,
    /// file name -> (decoded pixels, scale of those pixels, scale of the
    /// source tree they were read from)
    cache: std::collections::HashMap<String, (std::sync::Arc<codec::tga::TgaImage>, u32, u32)>,
    /// High-resolution sprites are large after WebP decode. Do not retain the
    /// entire 1,600-file library in RAM as the story advances.
    cache_limit: usize,
}

impl SpriteBank {
    fn new(trees: Vec<(u32, std::path::PathBuf)>) -> SpriteBank {
        let mut bank = SpriteBank {
            trees,
            dir: std::path::PathBuf::new(),
            scale: 0,
            index: Default::default(),
            cache: Default::default(),
            cache_limit: 64,
        };
        bank.retarget(render_art_scale());
        bank
    }

    /// Point the bank at the tree matching `want`, rebuilding the name index
    /// and dropping sprites decoded at another scale.
    fn retarget(&mut self, want: u32) {
        let mut order: Vec<usize> = (0..self.trees.len()).collect();
        order.sort_by_key(|&i| {
            let scale = self.trees[i].0;
            if scale == want {
                0
            } else if scale > want {
                1 + (scale - want)
            } else {
                100 + (want - scale)
            }
        });
        let Some(&i) = order.first() else { return };
        let (scale, dir) = self.trees[i].clone();
        if self.scale == scale && self.dir == dir {
            return;
        }
        self.scale = scale;
        self.dir = dir;
        self.index.clear();
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            for e in rd.flatten() {
                let name = match e.file_name().into_string() {
                    Ok(n) => n,
                    Err(_) => continue,
                };
                let lower = name.to_ascii_lowercase();
                if lower.ends_with(".tga") || lower.ends_with(".webp") {
                    self.index.insert(lower);
                }
            }
        }
        self.cache.clear();
        eprintln!("sprites: retargeted to {} ({}x)", self.dir.display(), scale);
        // Decoded sprites are 4x larger per art-scale step; keep the cache
        // small enough that the working set stays in the tens of MB.
        self.cache_limit = match scale {
            8 => 8,
            4 => 16,
            2 => 24,
            _ => 64,
        };
    }

    /// Returns the decoded pixels and the art scale they represent. A cached
    /// image keeps the scale it was decoded with, so resizing the window never
    /// mixes pixel scales between already-loaded and newly loaded sprites.
    fn get(&mut self, name: &str) -> Option<(std::sync::Arc<codec::tga::TgaImage>, u32, u32)> {
        if self.scale != render_art_scale() {
            self.retarget(render_art_scale());
        }
        let key = name.to_ascii_lowercase();
        if !self.cache.contains_key(&key) {
            let path = self.dir.join(&key);
            // Shared with the background loader, so a sprite the prefetch
            // thread already decoded costs nothing here.
            let img = (*artcache::load(&path)?).clone();
            let (src_scale, art_scale) = codec::image::art_scales(&path);
            let art_scale = art_scale.max(1);
            if self.cache.len() >= self.cache_limit {
                if let Some(old) = self.cache.keys().next().cloned() {
                    self.cache.remove(&old);
                }
            }
            self.cache.insert(key.clone(), (std::sync::Arc::new(img), art_scale, src_scale.max(1)));
        }
        self.cache
            .get(&key)
            .map(|(img, scale, src)| (std::sync::Arc::clone(img), *scale, *src))
    }

    /// Resolve `(char, series, variant)` to a real sprite file name.
    ///
    /// The shipped scripts' `(series, variant)` values match the file names
    /// directly for the vast majority of cues, so the search walks *outward*
    /// from the exact value (0, -1, +1, -2, +2). The previous order started at
    /// -2, which made almost every pose collapse onto `<prefix>001101.tga`.
    ///
    /// Only this character's own prefix is searched. Filling a missing pose
    /// with somebody else's file put the wrong person on stage.
    ///
    /// The neutral-pose fallback (`...101`) is only tried after the whole
    /// neighbourhood search has failed.
    fn resolve(&self, cid: u32, series: u32, variant: u32) -> Option<String> {
        let prefixes: Vec<String> = sprite_prefixes(cid).iter().map(|s| s.to_string()).collect();
        const DELTAS: [i32; 5] = [0, -1, 1, -2, 2];
        let s_base = series as i32;
        let v_base = variant as i32;
        for pf in &prefixes {
            for ds in DELTAS {
                let s = s_base + ds;
                if s < 0 {
                    continue;
                }
                for dv in DELTAS {
                    let v = v_base + dv;
                    if v < 0 {
                        continue;
                    }
                    for ext in [".webp", ".tga"] {
                        let name = format!("{}{:03}{:03}{}", pf, s as u32, v as u32, ext).to_ascii_lowercase();
                        if self.index.contains(&name) {
                            return Some(name);
                        }
                    }
                }
            }
        }
        // Last resort: the character's neutral pose.
        for pf in &prefixes {
            for s in [s_base, 1i32] {
                if s < 0 {
                    continue;
                }
                for v in [101u32, 102, 100] {
                    for ext in [".webp", ".tga"] {
                        let name = format!("{}{:03}{:03}{}", pf, s as u32, v, ext).to_ascii_lowercase();
                        if self.index.contains(&name) {
                            return Some(name);
                        }
                    }
                }
            }
        }
        None
    }

}

/// A character currently on screen.
struct LoadedAsset {
    img: codec::tga::TgaImage,
    /// Effective scale of the decoded pixels relative to 1280x720 layout
    /// coordinates. UI assets have already been normalized and use 1.
    art_scale: u32,
    /// Scale of the source tree the file was read from (4 for `bg_4x`, 8 for
    /// `bg_8x`, 1 for the original TGA tree). Printed next to the asset name so
    /// the log says where the pixels came from.
    src_scale: u32,
}

struct ActiveSprite {
    char_id: u32,
    #[allow(dead_code)]
    name: String,
    img: std::sync::Arc<codec::tga::TgaImage>,
    art_scale: u32,
    /// Scale of the sprite tree the file came from (4x or 8x), for the log.
    src_scale: u32,
}

/// An ACT176 plate (item, effect, or a layer of a composite CG).
struct Plate {
    slot: u32,
    name: String,
    img: codec::tga::TgaImage,
    art_scale: u32,
}

/// How the message window and the scene behind it are drawn this frame.
struct Look {
    alpha: u8,
    shade: u8,
}

/// Runtime ACT171 overrides for the two message-window roles. The setting
/// page supplies the normal values; a script can temporarily force either
/// role to an exact 0..=255 alpha and the next scene redraw uses it.
fn script_window_alpha_store() -> &'static std::sync::Mutex<[Option<u8>; 2]> {
    static STORE: std::sync::OnceLock<std::sync::Mutex<[Option<u8>; 2]>> = std::sync::OnceLock::new();
    STORE.get_or_init(|| std::sync::Mutex::new([None, None]))
}

fn reset_script_window_alpha() {
    if let Ok(mut state) = script_window_alpha_store().lock() {
        *state = [None, None];
    }
}

fn set_script_window_alpha(value: i32, channel: i32) {
    let value = value.clamp(0, 255) as u8;
    if let Ok(mut state) = script_window_alpha_store().lock() {
        match channel {
            0 => *state = [Some(value), Some(value)],
            // 0x521910 is the novel-window value and 0x521914 is the
            // visual/CG value in the original executable.
            1 => state[0] = Some(value),
            2 => state[1] = Some(value),
            _ => {}
        }
    }
}

fn scene_look(settings: &menu::Settings, cg: bool) -> Look {
    let levels = menu::LEVELS as u32;
    let level = if cg { settings.alpha_cg } else { settings.alpha_novel };
    let level = level.clamp(0, menu::LEVELS) as u32;
    let script_alpha = script_window_alpha_store()
        .lock()
        .ok()
        .and_then(|state| state[usize::from(cg)].or(state[0]));
    Look {
        alpha: script_alpha.unwrap_or((level * 255 / levels) as u8),
        // Novel text gets a mild dim so the window reads. CG stays untouched,
        // and the top of the slider (10) only reaches about a third darker.
        shade: if cg {
            0
        } else {
            (settings.alpha_back.clamp(0, menu::LEVELS) as u32 * 90 / levels) as u8
        },
    }
}

fn cmd_idle(settings: &menu::Settings) -> u8 {
    (settings.alpha_command.clamp(0, menu::LEVELS) as u32 * 100 / menu::LEVELS as u32) as u8
}

/// The box is up unless the player hid it. The script no longer folds it:
/// ACT170 turned out to be an ambient light preset, not a window switch.
fn dialog_open(user_hide: bool) -> bool {
    !user_hide
}

fn clone_surf(s: &render::Surface) -> render::Surface {
    render::Surface {
        width: s.width,
        height: s.height,
        pixels: s.pixels.clone(),
    }
}

fn surface_from_tga(img: &codec::tga::TgaImage) -> render::Surface {
    render::Surface {
        width: img.width,
        height: img.height,
        pixels: img.rgba.clone(),
    }
}

/// What the player did while fast-rewind was stepping.
enum Cruise {
    Keep,
    Stop,
    Quit,
}

fn present_rewind_frame(
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
fn rewind_cruise_stop(win: &mut sdl2::Sdl, bar: &cmdbar::CmdBar, hover: &mut Option<cmdbar::Cmd>) -> Cruise {
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

fn fade_len(settings: &menu::Settings, skipping: bool, frames: u32) -> u32 {
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
fn present_flash(win: &mut sdl2::Sdl, base: &render::Surface, frames: u32, r: u8, g: u8, b: u8) -> bool {
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

fn present_fade(win: &mut sdl2::Sdl, from: &render::Surface, to: &render::Surface, frames: u32) -> bool {
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

fn load_seen() -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    if let Ok(text) = std::fs::read_to_string(save::dir().join("seen.txt")) {
        for line in text.lines() {
            if !line.is_empty() {
                set.insert(line.to_string());
            }
        }
    }
    set
}

fn note_seen(set: &mut std::collections::HashSet<String>, scene: &str, text: &str) {
    if text.is_empty() {
        return;
    }
    let key = format!("{}\t{}", scene, text.replace('\n', " "));
    if !set.insert(key.clone()) {
        return;
    }
    let dir = save::dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("seen.txt"))
    {
        use std::io::Write;
        let _ = writeln!(f, "{}", key);
    }
}

fn place_overlays<'a>(plates: &'a [Plate], w: u32, h: u32) -> Vec<(&'a codec::tga::TgaImage, i32, i32, u32)> {
    plates
        .iter()
        .map(|p| {
            let scale = p.art_scale.max(1);
            let logical_w = (p.img.width / scale).max(1);
            let logical_h = (p.img.height / scale).max(1);
            let (x, y) = if logical_w >= 1100 || logical_h >= 600 {
                (0, 0)
            } else {
                (
                    (w as i32 - logical_w as i32) / 2,
                    (h as i32 - logical_h as i32) / 2,
                )
            };
            (&p.img, x, y, scale)
        })
        .collect()
}

/// Horizontal anchor for the idx-th of `count` on-screen characters.
fn sprite_x_for(idx: usize, count: usize, w: u32) -> i32 {
    let center = match (count, idx) {
        (1, _) => 640,
        (2, 0) => 360,
        (2, 1) => 920,
        (3, 0) => 330,
        (3, 1) => 640,
        (3, 2) => 950,
        (_, i) => 330 + (i.min(2) as i32) * 310,
    };
    center - (w as i32 / 2)
}

/// The composited picture without the dialogue, kept between lines.
///
/// Advancing the text used to stretch the background, blend the sprites, apply
/// the colour filter and stamp the message window again for every line. Only a
/// stage change (background, f0 effect, sprite, overlay, filter, window size) really needs
/// that work; printing a line only has to repaint the text on a copy.
struct SceneLayer {
    key: u64,
    w: u32,
    h: u32,
    surf: render::Surface,
}

fn scene_layer_store() -> &'static std::sync::Mutex<Option<SceneLayer>> {
    static LAYER: std::sync::OnceLock<std::sync::Mutex<Option<SceneLayer>>> = std::sync::OnceLock::new();
    LAYER.get_or_init(|| std::sync::Mutex::new(None))
}

/// FNV-1a over everything the picture is made of. Image identity is the pixel
/// buffer address: the same decoded picture keeps its address for as long as
/// the scene holds it, and a new load necessarily has a different one.
fn hash_u64(state: &mut u64, value: u64) {
    *state = (*state ^ value).wrapping_mul(0x100_0000_01b3);
}

fn scene_art_key(
    bg: &codec::tga::TgaImage,
    effect: Option<&codec::tga::TgaImage>,
    draws: &[render::SpriteDraw],
    tint: Option<&[u8]>,
    window: bool,
    plate: Option<&codec::tga::TgaImage>,
    frame: Option<&codec::tga::TgaImage>,
    look: &Look,
    overlays: &[(&codec::tga::TgaImage, i32, i32, u32)],
    w: u32,
    h: u32,
) -> u64 {
    let mut key = 0xcbf2_9ce4_8422_2325u64;
    hash_u64(&mut key, bg.rgba.as_ptr() as usize as u64);
    hash_u64(&mut key, bg.width as u64);
    hash_u64(&mut key, effect.map(|e| e.rgba.as_ptr() as usize as u64).unwrap_or(0));
    hash_u64(&mut key, effect.map(|e| e.width as u64).unwrap_or(0));
    hash_u64(&mut key, effect.map(|e| e.height as u64).unwrap_or(0));
    for sp in draws {
        hash_u64(&mut key, sp.img.rgba.as_ptr() as usize as u64);
        hash_u64(&mut key, sp.logical_w as u64);
        hash_u64(&mut key, sp.x as u64);
        hash_u64(&mut key, sp.y as u64);
        hash_u64(&mut key, sp.alpha as u64);
    }
    hash_u64(&mut key, tint.map(|t| t.as_ptr() as usize as u64).unwrap_or(0));
    hash_u64(&mut key, tint.map(|t| t.len() as u64).unwrap_or(0));
    hash_u64(&mut key, window as u64);
    hash_u64(&mut key, plate.map(|p| p.rgba.as_ptr() as usize as u64).unwrap_or(0));
    hash_u64(&mut key, frame.map(|p| p.rgba.as_ptr() as usize as u64).unwrap_or(0));
    hash_u64(&mut key, look.alpha as u64);
    hash_u64(&mut key, look.shade as u64);
    for (img, x, y, scale) in overlays {
        hash_u64(&mut key, img.rgba.as_ptr() as usize as u64);
        hash_u64(&mut key, *x as u64);
        hash_u64(&mut key, *y as u64);
        hash_u64(&mut key, *scale as u64);
    }
    hash_u64(&mut key, w as u64);
    hash_u64(&mut key, h as u64);
    key
}

/// Composite the scene picture, reusing the previous frame when nothing about
/// the stage has changed.
#[allow(clippy::too_many_arguments)]
fn scene_art(
    bg: &codec::tga::TgaImage,
    effect: Option<&codec::tga::TgaImage>,
    draws: &[render::SpriteDraw],
    tint: Option<&[u8]>,
    window: bool,
    plate: Option<&codec::tga::TgaImage>,
    frame: Option<&codec::tga::TgaImage>,
    look: &Look,
    overlays: &[(&codec::tga::TgaImage, i32, i32, u32)],
    w: u32,
    h: u32,
    key: u64,
) -> render::Surface {
    let mut store = match scene_layer_store().lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    if let Some(layer) = store.as_ref() {
        if layer.key == key && layer.w == w && layer.h == h {
            return clone_surf(&layer.surf);
        }
    }
    let surf = render::compose_scene_art(
        bg, effect, w, h, draws, tint, window, plate, frame, look.alpha, look.shade, overlays,
    );
    *store = Some(SceneLayer { key, w, h, surf: clone_surf(&surf) });
    surf
}


/// Composite background + sprites + dialog box + text.
fn draw_scene(
    bg: &codec::tga::TgaImage,
    text: &str,
    speaker: Option<&str>,
    font: &render::Font,
    slots: &[Option<ActiveSprite>; SPRITE_SLOTS],
    focus: Option<u32>,
    // Size of the frame to return - the window, not the 1280x720 layout.
    out_w: u32,
    out_h: u32,
    tint: Option<&[u8]>,
    effect: Option<&codec::tga::TgaImage>,
    // 「ウィンドウ一時消去」: false hides the dialog box and its text.
    window: bool,
    plate: Option<&codec::tga::TgaImage>,
    frame: Option<&codec::tga::TgaImage>,
    plates: &[Plate],
    look: Look,
    // Vector font for the dialogue, or None to keep the bitmap atlas.
    vf: Option<&mut render::text::VectorFont>,
) -> render::Surface {
    let t0 = std::time::Instant::now();
    // The Japanese bitmap atlas still belongs to the 1280x720 compositor. The
    // Chinese path uses vector text and can composite high-res art directly at
    // the window size without stretching the bitmap glyphs.
    let hires = vf.is_some() && (out_w > render::LAYOUT_W || out_h > render::LAYOUT_H);
    // Dim the listeners only when the speaker is actually standing there.
    // 春希 speaks thousands of times and is placed twice in the whole game:
    // he is the camera, and the person on screen is who he is talking to.
    // Dimming everyone in that case made the listener look like the wrong
    // speaker. When the speaker is on stage, the others drop well below
    // full brightness so the lit face is the name in the window.
    let speaker_on_stage = focus.is_some_and(|id| {
        slots.iter().flatten().any(|s| s.char_id == id)
    });
    let draws: Vec<render::SpriteDraw> = SPRITE_DRAW_ORDER
        .iter()
        .filter_map(|&i| {
            let s = slots[i].as_ref()?;
            let alpha = if speaker_on_stage && focus != Some(s.char_id) {
                96
            } else {
                255
            };
            let sprite_scale = s.art_scale.max(1);
            let logical_w = (s.img.width / sprite_scale).max(1);
            let logical_h = (s.img.height / sprite_scale).max(1);
            Some(render::SpriteDraw {
                img: &s.img,
                logical_w,
                logical_h,
                x: SPRITE_ANCHORS[i] - logical_w as i32 / 2,
                y: (render::LAYOUT_H as i32 - logical_h as i32).max(0),
                alpha,
            })
        })
        .collect();
    let overlays = place_overlays(plates, render::LAYOUT_W, render::LAYOUT_H);
    // With a vector font the text is drawn after the frame is scaled, so the
    // atlas path hands the text to the compositor and the vector path does not.
    let render_w = if hires { out_w } else { render::LAYOUT_W };
    let render_h = if hires { out_h } else { render::LAYOUT_H };
    let key = scene_art_key(
        bg, effect, &draws, tint, window, plate, frame, &look, &overlays, render_w, render_h,
    );
    let t_art = std::time::Instant::now();
    let mut s = scene_art(
        bg, effect, &draws, tint, window, plate, frame, &look, &overlays, render_w, render_h, key,
    );
    tlog("scene art", t_art);
    // The bitmap atlas stamps its glyphs onto this layer; the vector path
    // draws later, after the frame has been scaled to the window.
    if !vf.is_some() && window {
        render::draw_dialog_text(&mut s, font, text, plate.is_some(), render_w, render_h, usize::MAX);
    }
    let out = match vf {
        Some(vf) => {
            let mut s = if !hires && (out_w, out_h) != (render::LAYOUT_W, render::LAYOUT_H) {
                s.scaled_to(out_w, out_h)
            } else {
                s
            };
            if window {
                // The name sits on the upper rim of the oval, not inside 【】
                // in the line, and only the game's own window has a rim.
                let name = if plate.is_some() { speaker } else { None };
                let td = std::time::Instant::now();
                render::draw_dialog_vector(
                    &mut s,
                    vf,
                    text,
                    name,
                    out_w as i32,
                    out_h as i32,
                    usize::MAX,
                );
                tlog("dialog text", td);
            }
            s
        }
        None => {
            // The name sits on the upper rim of the oval, not inside 【】 in the line.
            if window && plate.is_some() {
                if let Some(name) = speaker {
                    if !name.is_empty() {
                        font.draw_pitch(&mut s, name, render::NAME_X, render::NAME_Y, render::DIALOG_PITCH);
                    }
                }
            }
            s
        }
    };
    tlog("draw_scene", t0);
    out
}

/// Draw the dialogue body onto a frame that is already composited, at the size
/// the frame actually is.
///
/// With a vector font the frame is window-sized and the glyphs are rasterized
/// at that size; otherwise the atlas is stamped onto the 1280x720 layer. `limit`
/// is how many characters to reveal.
fn draw_body(
    frame: &mut render::Surface,
    font: &render::Font,
    vf: Option<&mut render::text::VectorFont>,
    text: &str,
    speaker: Option<&str>,
    plate: bool,
    out_w: u32,
    out_h: u32,
    limit: usize,
) {
    match vf {
        Some(vf) => {
            let name = if plate { speaker } else { None };
            render::draw_dialog_vector(frame, vf, text, name, out_w as i32, out_h as i32, limit);
        }
        None => render::draw_dialog_text(
            frame,
            font,
            text,
            plate,
            render::LAYOUT_W,
            render::LAYOUT_H,
            limit,
        ),
    }
}

/// Older saves stored the line as `【name】\nbody`. Split that so a loaded
/// slot still puts the name on the rim.
fn split_saved_line(text: &str) -> (Option<String>, String) {
    if let Some(rest) = text.strip_prefix('【') {
        if let Some((name, body)) = rest.split_once("】\n") {
            return (Some(name.to_string()), body.to_string());
        }
    }
    (None, text.to_string())
}

/// Clean scenario text for rendering: literal `\n` -> newline, `\k` removed,
/// ruby tags `<Rbase|reading>` collapsed to `base`, other `<...>` tags stripped.
fn clean_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.peek() {
                Some('n') => {
                    chars.next();
                    out.push('\n');
                }
                Some('k') => {
                    chars.next();
                }
                _ => out.push(c),
            },
            '<' => {
                let mut tag = String::new();
                loop {
                    match chars.next() {
                        Some('>') | None => break,
                        Some(d) => tag.push(d),
                    }
                }
                if let Some(rest) = tag.strip_prefix('R') {
                    if let Some(base) = rest.split('|').next() {
                        out.push_str(base);
                    }
                }
            }
            _ => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// branch choices
// ---------------------------------------------------------------------------

/// One choice row while a branch is presented, in output pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ChoiceRow {
    top: i32,
    height: i32,
}

/// A choice row is one line of the message window. The numbers come from the
/// original choice screen at 1280x720: a row is about a third of the dialogue
/// window tall, two rows sit 138px apart and straddle y=262 (so they land on
/// 193 and 331), and the block stays above the dialogue box.
const CHOICE_ROW_H: i32 = 72;
const CHOICE_PITCH: i32 = 138;
const CHOICE_CENTER_Y: i32 = 262;
const CHOICE_MIN_TOP: i32 = 24;
const CHOICE_MAX_BOTTOM: i32 = 476;

/// Vertical centres of every row, in logical 720p coordinates. Up to three
/// options keep the measured pitch; a rare four-option branch is tightened so
/// the block still fits between the top of the screen and the dialogue box.
fn choice_row_centers(count: usize) -> Vec<i32> {
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
fn choice_row_layout(count: usize, surface_h: u32) -> Vec<ChoiceRow> {
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
fn paint_choice_rows(
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

/// The original's system UI sound. WA2.exe plays SE_8905 (id 0x22c9) from the
/// two tiny wrappers at 0x40ea10 / 0x40ea30 for menu interactions; the branch
/// choice rows use the game-wide UI sound for hover and click.
const UI_SE_ID: u32 = 8905;

fn ui_se_path(script_dir: &Path) -> Option<std::path::PathBuf> {
    let dir = script_dir.parent()?.join("se");
    ["WAV", "OGG", "wav", "ogg"]
        .iter()
        .map(|ext| dir.join(format!("SE_{:04}.{}", UI_SE_ID, ext)))
        .find(|p| p.is_file())
}

fn play_ui_se(mixer: Option<&audio::Mixer>, path: Option<&Path>) {
    if let (Some(mx), Some(path)) = (mixer, path) {
        if mx.play_cat(&path.to_string_lossy(), 1.0, false, audio::mixer::CAT_SE) != usize::MAX {
            eprintln!("  SE: choice UI id={} -> {}", UI_SE_ID, path.display());
        }
    }
}

/// CG stills from `bg/v*.tga`. Left and right (or the two halves of the
/// screen) page through them; Esc or the right button returns to the title.
/// CG mode: a grid of thumbnails, then a zoomable full view of one picture.
///
/// The grid reads the source tree selected for the current window. `cg::load_thumb`
/// resizes 2x/4x masters during decode and keeps only the current page in
/// memory, so the fullscreen grid never enlarges the original 256x144 `tv*`
/// copies or leaves a disk cache behind.
fn browse_cg(
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
fn cell_origin(col: usize, row: usize, gx: i32, gy: i32, cw: u32, ch: u32, gap: u32) -> (i32, i32) {
    (gx + col as i32 * (cw + gap) as i32, gy + row as i32 * (ch + gap) as i32)
}

/// Which cell a layout point lands on, if the page really has a CG there.
fn cell_at(
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

fn highlight_cell(frame: &mut render::Surface, cx: i32, cy: i32, cw: u32, ch: u32) {
    let gold = [226, 196, 120, 255];
    frame.fill_rect(cx - 2, cy - 2, cw + 4, 2, gold);
    frame.fill_rect(cx - 2, cy + ch as i32, cw + 4, 2, gold);
    frame.fill_rect(cx - 2, cy - 2, 2, ch + 4, gold);
    frame.fill_rect(cx + cw as i32, cy - 2, 2, ch + 4, gold);
}

/// One line of UI text on a dark plate. `centre` lays it out across the 1280px
/// layout width instead of at `x`, which is what the hint lines want.
fn draw_label(
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

/// Zoom levels the full view steps through. 1.0 shows the whole picture.
const CG_ZOOMS: [f32; 4] = [1.0, 1.5, 2.0, 3.0];

/// How much of the visible area one arrow press moves.
const PAN_STEP: f32 = 0.125;

/// Full view of one CG: zoom with the wheel, pan by dragging or the arrows.
///
/// `sel` stays on the last picture looked at, so the grid comes back where the
/// player left it.
fn view_cg(
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

fn step_zoom(zoom: usize, up: bool) -> usize {
    if up {
        (zoom + 1).min(CG_ZOOMS.len() - 1)
    } else {
        zoom.saturating_sub(1)
    }
}

/// Source rectangle shown by a zoomed CG. At zoom 1 the caller fits the whole
/// image separately; higher zoom levels use a crop matching the window ratio,
/// which is essential for portrait CGs.
fn cg_view_crop(im: &codec::tga::TgaImage, ww: u32, wh: u32, zoom: f32) -> (f32, f32) {
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
fn pan_by(
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
fn browse_novels(
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
const VOICE_PANEL: (i32, i32, u32, u32) = (700, 88, 556, 532);
const VOICE_ROW_X: i32 = 716;
const VOICE_ROW_W: u32 = 524;
const VOICE_ROW_H: u32 = 52;
const VOICE_ROW_PITCH: i32 = 60;
const VOICE_ROW_Y0: i32 = 176;
/// Only this many rows fit between the title and the stop button.
const VOICE_MAX_ROWS: usize = 5;
const VOICE_BTN: (i32, i32, u32, u32) = (716, 520, 220, 56);

/// The original 「ボイスメッセージ」 page: pick a character card, pick one of
/// her messages, play it, stop it.
///
/// Every card and every clip that exists is selectable from the first visit -
/// there is no "heard it before" gate. What the page tracks is what the
/// original shows: the open card, the selected message, whether something is
/// playing, and an explicit notice when the audio is missing (no clips shipped
/// for that character, or no audio device at all).
fn browse_voice_messages(
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
fn voice_stop(mixer: Option<&audio::Mixer>, playing: &mut Option<(usize, usize, usize)>) {
    if let (Some(mx), Some((slot, _, _))) = (mixer, playing.take()) {
        mx.stop(slot);
    }
}

/// Start one message, replacing anything already playing. False when there is
/// no audio device or the decoder could not be started.
fn voice_play(
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
fn voice_clamp(line: &mut usize, count: usize) {
    if *line >= count {
        *line = count.saturating_sub(1);
    }
}

fn point_in(x: i32, y: i32, r: (i32, i32, u32, u32)) -> bool {
    x >= r.0 && x < r.0 + r.2 as i32 && y >= r.1 && y < r.1 + r.3 as i32
}

/// Which message row a layout point lands on, if the bank has it.
fn voice_row_at(x: i32, y: i32, count: usize) -> Option<usize> {
    let i = (y - VOICE_ROW_Y0) / VOICE_ROW_PITCH;
    if i < 0 || i as usize >= count.min(VOICE_MAX_ROWS) {
        return None;
    }
    if !point_in(x, y, (VOICE_ROW_X, VOICE_ROW_Y0 + i * VOICE_ROW_PITCH, VOICE_ROW_W, VOICE_ROW_H)) {
        return None;
    }
    Some(i as usize)
}
/// 「特别模式」. Labels are the system font. The highlight bar and the logo
/// stay the original art. 「时间线」 replaces the atlas row 场景回放.
enum SpecialPick {
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
struct SpecialStats {
    entered: [u32; 5],
    visited: u8,
}

impl SpecialStats {
    fn index(pick: &SpecialPick) -> Option<usize> {
        match pick {
            SpecialPick::Novel => Some(0),
            SpecialPick::Cg => Some(1),
            SpecialPick::Replay => Some(2),
            SpecialPick::Music => Some(3),
            SpecialPick::Voice => Some(4),
            SpecialPick::Notice | SpecialPick::Back => None,
        }
    }

    fn label(&self, cn: bool) -> String {
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

fn special_stats_path(dirs: &[&std::path::Path]) -> Option<std::path::PathBuf> {
    dirs.iter().find_map(|d| d.parent().map(|root| root.join("save").join("special_stats.tsv")))
}

fn load_special_stats(dirs: &[&std::path::Path]) -> SpecialStats {
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

fn save_special_stats(dirs: &[&std::path::Path], stats: SpecialStats) {
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

fn record_special_visit(dirs: &[&std::path::Path], pick: &SpecialPick) {
    let Some(i) = SpecialStats::index(pick) else { return };
    let mut stats = load_special_stats(dirs);
    stats.visited |= 1 << i;
    stats.entered[i] = stats.entered[i].saturating_add(1);
    save_special_stats(dirs, stats);
}

fn crop_image(
    image: &codec::tga::TgaImage,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
) -> Option<codec::tga::TgaImage> {
    if x >= image.width || y >= image.height {
        return None;
    }
    let w = w.min(image.width - x);
    let h = h.min(image.height - y);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for row in 0..h as usize {
        let src = ((y as usize + row) * image.width as usize + x as usize) * 4;
        let dst = row * w as usize * 4;
        rgba[dst..dst + w as usize * 4]
            .copy_from_slice(&image.rgba[src..src + w as usize * 4]);
    }
    Some(codec::tga::TgaImage { width: w, height: h, rgba })
}

/// UI sheet retained at its source resolution. The scene compositor uses
/// logical-size UI sheets; standalone pages such as Music need the 4x/8x
/// pixels so their baked labels stay sharp at fullscreen size.
struct HiUiAsset {
    img: codec::tga::TgaImage,
    scale: u32,
}

fn load_ui_asset_hi(
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
fn menu_labels(items: &[&str], size: u32) -> Vec<Option<(u32, u32, Vec<u8>)>> {
    items
        .iter()
        .map(|s| render::text::with_ui_font(size, |f| f.line_rgba(s, size)).flatten())
        .collect()
}

// Menu sounds. Every screen names what it is doing (`UiSe`) instead of
// picking an SE id, so the mapping lives in exactly one place.

/// `WA2.exe` wraps its SE player three times in one cluster: 0x40ea10 and
/// 0x40ea30 push 0x22c9 (`SE_8905`) and 0x40ea50 pushes 0x22c7 (`SE_8903`).
/// The archives only ship `SE_8905`, so cancel falls back to it instead of
/// staying silent. `SE_9011` is the short two-tap click the title highlight
/// and the menu cursor move to.
const SE_UI: u32 = 8905;
const SE_UI_CANCEL: u32 = 8903;
const SE_CLICK: u32 = 9011;

/// Which menu sound an interaction wants.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UiSe {
    Move,
    Confirm,
    Cancel,
    /// Page flips and prev/next paging: the cursor click, not the menu chime.
    Page,
}

/// The four menu sounds, resolved once per screen.
pub struct UiSound {
    mv: Option<std::path::PathBuf>,
    ok: Option<std::path::PathBuf>,
    cancel: Option<std::path::PathBuf>,
    page: Option<std::path::PathBuf>,
}

impl UiSound {
    /// Resolve every role. `dirs` are the art trees; `se/` sits next to them.
    pub fn resolve(dirs: &[&std::path::Path]) -> UiSound {
        let mv = find_se(dirs, SE_CLICK);
        let ok = find_se(dirs, SE_UI);
        UiSound {
            mv: mv.clone(),
            ok: ok.clone(),
            cancel: find_se(dirs, SE_UI_CANCEL).or(ok),
            page: mv,
        }
    }

    pub fn path(&self, kind: UiSe) -> Option<&std::path::Path> {
        match kind {
            UiSe::Move => self.mv.as_deref(),
            UiSe::Confirm => self.ok.as_deref(),
            UiSe::Cancel => self.cancel.as_deref(),
            UiSe::Page => self.page.as_deref(),
        }
    }
}

/// `SE_{id:04}` next to the art trees, in either container format.
fn find_se(dirs: &[&std::path::Path], id: u32) -> Option<std::path::PathBuf> {
    for d in dirs {
        let Some(root) = d.parent() else { continue };
        for ext in ["WAV", "wav", "OGG", "ogg"] {
            let p = root.join("se").join(format!("SE_{id:04}.{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// Play one menu sound through the mixer at the SE group volume.
pub fn play_ui(mixer: Option<&audio::Mixer>, sounds: Option<&UiSound>, kind: UiSe) {
    let (Some(mx), Some(s)) = (mixer, sounds) else { return };
    let Some(p) = s.path(kind) else { return };
    if mx.play_cat(&p.to_string_lossy(), 1.0, false, audio::mixer::CAT_SE) != usize::MAX {
        eprintln!("  SE: menu {:?} -> {}", kind, p.display());
    }
}
/// Font size that has the same height, in window pixels, as `layout_px` on
/// the 720-tall layout. Labels are drawn after the scene is scaled, so this
/// is the size they must be rasterized at.
fn layout_font_px(layout_px: u32, window_h: i32) -> u32 {
    ((layout_px as i32 * window_h.max(1)) / 720).clamp(8, 160) as u32
}

/// Where the highlight sits in window pixels. `layout_y` stays fractional so
/// a maximized window can move the bar one screen pixel at a time.
fn bar_window_y(layout_y: f32, wh: i32) -> i32 {
    (layout_y * wh.max(1) as f32 / 720.0).round() as i32
}

/// Slide the highlight toward `target`. At 720px tall this is the original
/// per-frame ease. A taller window caps each step at about six screen pixels,
/// otherwise one layout pixel becomes several screen pixels and the bar hops.
fn slide_bar(bar_y: &mut f32, target: f32, wh: i32) {
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
struct FittedMenu {
    ww: i32,
    wh: i32,
    stamp: u64,
    plate: render::Surface,
    bar: [Option<render::Surface>; 2],
}

fn scale_sprite(
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

fn ensure_fitted(
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

fn paint_bar(frame: &mut render::Surface, fitted: &FittedMenu, which: usize, layout_y: f32, x0: i32) {
    let Some(img) = fitted.bar[which].as_ref() else { return };
    let dx = x0 * fitted.ww / 1280;
    let dy = bar_window_y(layout_y, fitted.wh);
    frame.blit_rgba(&img.pixels, img.width, img.height, dx, dy);
}

fn draw_menu_labels(
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
fn draw_menu_labels_pre(
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
fn blit_tinted_label(
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
fn label_surface(rgba: &[u8], w: u32, h: u32, rgb: [u8; 3], stroke: i32) -> render::Surface {
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

fn run_special(
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

/// Music mode.  BGM files are indexed by track number; `_A` is preferred
/// when both internal variants are present because it is the normal in-game
/// stream.  The player keeps one looping stream alive and never decodes the
/// whole library up front.
fn run_music(
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
            let rank = if low.ends_with("_a") { 0 } else if low.ends_with("_b") { 2 } else { 1 };
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

/// What the title screen resolved to.
enum TitleAction {
    Start,
    /// A scene picked on the timeline.
    Jump(String),
    /// Resume from a save slot picked on the load screen.
    Load(save::SaveState),
    Quit,
}

/// One falling flake. Layout numbers are 1280×720 pixels; the draw scales them.
struct TitleFlake {
    x: f32,
    y: f32,
    /// Downward speed, layout pixels per second.
    speed: f32,
    phase: f32,
    /// Which of the eight sizes. 0 is the largest.
    kind: usize,
    alpha: u8,
}

/// Title snowfall. The original loads `weather.tga` and draws the white flake
/// cell over the street. Kind 248..255 is a size of 36..8 px. The exe's own
/// alpha (about 96..40) vanishes on this bright plate, so the flakes are
/// drawn brighter and there are more of them.
struct TitleSnow {
    ww: i32,
    wh: i32,
    flakes: Vec<TitleFlake>,
    sprites: Vec<render::Surface>,
    rng: u32,
}

fn snow_rng(state: &mut u32) -> u32 {
    *state = state.wrapping_mul(1664525).wrapping_add(1013904223);
    *state
}

impl TitleSnow {
    const COUNT: usize = 400;
    const KINDS: usize = 8;

    fn new() -> TitleSnow {
        let mut rng = 0x00a2_5e1du32;
        let mut flakes = Vec::with_capacity(Self::COUNT);
        for i in 0..Self::COUNT {
            flakes.push(Self::make(&mut rng, i % Self::KINDS, true));
        }
        TitleSnow { ww: 0, wh: 0, flakes, sprites: Vec::new(), rng }
    }

    /// `kind` 0..8 maps to the exe's flake id 248..255.
    fn make(rng: &mut u32, kind: usize, anywhere: bool) -> TitleFlake {
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

    fn ensure(&mut self, ww: i32, wh: i32, sheet: &codec::tga::TgaImage) {
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

    fn step(&mut self, dt: f32) {
        for flake in &mut self.flakes {
            flake.y += flake.speed * dt;
            flake.phase += dt * 1.8;
            flake.x += flake.phase.sin() * 18.0 * dt;
            if flake.y > 760.0 || flake.x < -40.0 || flake.x > 1320.0 {
                *flake = Self::make(&mut self.rng, flake.kind, false);
            }
        }
    }

    fn paint(&self, frame: &mut render::Surface, wh: i32) {
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
const TITLE_BGM_ID: i32 = 31;

fn title_bgm_path(bg_dirs: &[&std::path::Path]) -> Option<std::path::PathBuf> {
    let dir = bg_dirs.iter().find_map(|d| d.parent())?.join("bgm");
    let stem = format!("BGM_{:03}", TITLE_BGM_ID);
    [format!("{}.OGG", stem), format!("{}_A.OGG", stem)]
        .into_iter()
        .map(|name| dir.join(name))
        .find(|p| p.is_file())
}

/// Play the title/special track on the title screen's own mixer. That mixer
/// lives exactly as long as the title loop, so leaving the title (starting or
/// loading the game, quitting) drops every stream with it.
fn start_title_bgm(mixer: Option<&audio::Mixer>, bg_dirs: &[&std::path::Path]) -> Option<usize> {
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
fn run_title(
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
                        None,
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
                    menu::run(win, bg_dirs, &mut st, mixer, Some(&ui));
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

/// Play a movie (`mvNN` token) from the movie directory.
///
/// The movies are raw ASF/WMV files (the original `mv*.pak` is not an archive:
/// its first bytes are the ASF GUID), so the data tree stores them as plain
/// `*.wmv`. Scripts name them "mv01".."mv24" while the files ship as version
/// pairs (`mv010`, `mv011`), so we try the plain name first, then 0 and 1.
fn play_movie(
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
                    let snapshot = render::Surface {
                        width: movie.width,
                        height: movie.height,
                        pixels: frame.clone(),
                    }
                    .scaled_to(1280, 720);
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
fn wait_input(win: &mut sdl2::Sdl) -> sdl2::Input {
    loop {
        let ev = win.poll_input();
        match ev {
            sdl2::Input::None | sdl2::Input::Pointer { pressed: false, .. } => {}
            other => return other,
        }
        win.delay(16);
    }
}

/// Load the font for a data tree: atlas + authoritative cell map.
///
/// The cell map (`wa2_cell_map_{jp,cn}.tsv`) is the game exe's own font table
/// expressed as atlas cells, so glyph lookup is exact for both the Japanese and
/// the Chinese atlas - no heuristics, no codec ambiguity.
fn load_font(d: &data::Data) -> Option<render::Font> {
    load_font_paths(&d.atlas(), Some(&d.cell_map()), None)
}

/// Load a font atlas with either an authoritative cell map, a legacy charset
/// table, or both (the cell map wins).
fn load_font_paths(
    atlas_path: &std::path::Path,
    cell_map: Option<&std::path::Path>,
    charset_path: Option<&std::path::Path>,
) -> Option<render::Font> {
    let atlas = match std::fs::read(atlas_path).ok().and_then(|d| codec::tga::decode_font(&d).ok()) {
        Some(i) => i,
        None => { eprintln!("error: could not decode font atlas {}", atlas_path.display()); return None; }
    };
    let cell = if atlas.width >= 3000 { 40 } else { 24 };
    eprintln!("font atlas {}x{} -> cell={}", atlas.width, atlas.height, cell);

    if let Some(map) = cell_map {
        match render::Font::from_cell_map(atlas, cell, map) {
            Ok(f) => {
                eprintln!("font cell map: {} ({} glyphs)", map.display(), f.mapped_glyphs());
                return Some(f);
            }
            Err(e) => {
                eprintln!("error reading cell map {}: {}", map.display(), e);
                return None;
            }
        }
    }

    let charset_path = charset_path?;
    let charset = match std::fs::read_to_string(charset_path) {
        Ok(s) => s,
        Err(e) => { eprintln!("error reading {}: {}", charset_path.display(), e); return None; }
    };
    // Two charset flavours need different handling:
    //  * a 1:1 atlas table (SJIS-ordered charset.txt, ~7.7k chars) whose
    //    leading entries are REAL blank/space glyphs. Stripping whitespace
    //    there shifts every glyph by one cell (cell 0 = space, cell 1 = !).
    //  * a formatted charset (wa2.cht.txt / wa2_cset.txt) containing wrapping
    //    newlines/spaces, which must be stripped; those start at cell 748.
    let ws = charset.chars().filter(|c| c.is_whitespace()).count();
    if ws > 10 {
        eprintln!("font charset: formatted ({} ws stripped) start_pos=748", ws);
        Some(render::Font::from_charset(atlas, &charset, cell, 748))
    } else {
        eprintln!("font charset: 1:1 table ({} ws kept) start_pos=0", ws);
        Some(render::Font::from_charset_raw(atlas, &charset, cell, 0))
    }
}

/// Legacy 2-argument form used by the single-scene debugging commands.
fn load_font_charset(atlas_path: &str, charset_path: &str) -> Option<render::Font> {
    load_font_paths(
        std::path::Path::new(atlas_path),
        None,
        Some(std::path::Path::new(charset_path)),
    )
}

/// Resolve the output directory: explicit arg if present, else `<stem><suffix>`.
fn out_dir(pak: &str, args: &[String], explicit_idx: usize, suffix: &str) -> String {
    if args.len() > explicit_idx { return args[explicit_idx].clone(); }
    let p = Path::new(pak);
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "out".into());
    format!("{}{}", stem, suffix)
}

fn print_usage() {
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

#[cfg(test)]
mod ui_sound_tests {
    use super::*;

    /// A fake art tree: `<root>/bg` plus the `se/` directory next to it.
    fn tree(files: &[&str]) -> (std::path::PathBuf, Vec<std::path::PathBuf>) {
        let root = std::env::temp_dir().join(format!("wa2-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let se = root.join("se");
        std::fs::create_dir_all(root.join("bg")).unwrap();
        std::fs::create_dir_all(&se).unwrap();
        for f in files {
            std::fs::write(se.join(f), b"").unwrap();
        }
        let dirs: Vec<std::path::PathBuf> = vec![root.join("bg")];
        (root, dirs)
    }

    #[test]
    fn every_role_resolves_to_the_ids_the_exe_pushes() {
        let (_root, dirs) = tree(&["SE_9011.WAV", "SE_8905.WAV"]);
        let refs: Vec<&std::path::Path> = dirs.iter().map(|d| d.as_path()).collect();
        let ui = UiSound::resolve(&refs);
        let name = |k: UiSe| ui.path(k).map(|p| p.file_name().unwrap().to_string_lossy().into_owned());
        assert_eq!(name(UiSe::Move).as_deref(), Some("SE_9011.WAV"));
        assert_eq!(name(UiSe::Page).as_deref(), Some("SE_9011.WAV"));
        assert_eq!(name(UiSe::Confirm).as_deref(), Some("SE_8905.WAV"));
    }

    /// SE_8903 (0x22c7, the third UI wrapper) is not in the archives, so cancel
    /// has to fall back to SE_8905 rather than going silent.
    #[test]
    fn a_missing_cancel_sound_falls_back_to_the_menu_chime() {
        let (_root, dirs) = tree(&["SE_8905.WAV", "SE_9011.WAV"]);
        let refs: Vec<&std::path::Path> = dirs.iter().map(|d| d.as_path()).collect();
        let ui = UiSound::resolve(&refs);
        assert_eq!(ui.path(UiSe::Cancel), ui.path(UiSe::Confirm));
    }

    #[test]
    fn with_nothing_shipped_every_role_is_silent_not_a_crash() {
        let (_root, dirs) = tree(&[]);
        let refs: Vec<&std::path::Path> = dirs.iter().map(|d| d.as_path()).collect();
        let ui = UiSound::resolve(&refs);
        for k in [UiSe::Move, UiSe::Confirm, UiSe::Cancel, UiSe::Page] {
            assert!(ui.path(k).is_none(), "{k:?} should be silent");
        }
    }

    #[test]
    fn special_stats_have_one_counter_per_content_page() {
        let mut stats = SpecialStats::default();
        for pick in [
            SpecialPick::Novel,
            SpecialPick::Cg,
            SpecialPick::Replay,
            SpecialPick::Music,
            SpecialPick::Voice,
        ] {
            let i = SpecialStats::index(&pick).unwrap();
            stats.visited |= 1 << i;
            stats.entered[i] += 1;
        }
        assert_eq!(stats.visited.count_ones(), 5);
        assert!(stats.label(true).contains("已浏览 5/5"));
        assert_eq!(SpecialStats::index(&SpecialPick::Back), None);
    }

    #[test]
    fn h_scene_voice_rule_only_affects_the_explicit_sample_play_scenes() {
        let mut st = menu::Settings::default();
        st.ero_voice = 0;
        assert!(!h_scene_voice_allowed(&st, "2031", 2));
        assert!(h_scene_voice_allowed(&st, "2001", 2));
        st.ero_voice = 2;
        assert!(h_scene_voice_allowed(&st, "2031", 2));
        assert!(!h_scene_voice_allowed(&st, "2031", 10));
        st.ero_voice = 1;
        assert!(h_scene_voice_allowed(&st, "2031", 10));
    }
}

#[cfg(test)]
mod menu_bar {
    #[test]
    fn windowed_step_is_thirty_percent() {
        let mut y = 432.0;
        super::slide_bar(&mut y, 472.0, 720);
        assert!((y - 444.0).abs() < 0.01, "{y}");
    }

    #[test]
    fn maximized_step_stays_within_six_screen_pixels() {
        let mut y = 432.0;
        let mut prev = super::bar_window_y(y, 1080);
        for _ in 0..90 {
            super::slide_bar(&mut y, 472.0, 1080);
            let now = super::bar_window_y(y, 1080);
            assert!((now - prev).abs() <= 6, "{prev} -> {now}");
            prev = now;
            if y == 472.0 {
                break;
            }
        }
        assert_eq!(y, 472.0);
        // A fraction of a layout pixel still moves the bar on a tall window.
        assert_eq!(super::bar_window_y(432.0, 1080), 648);
        assert_eq!(super::bar_window_y(432.4, 1080), 649);
    }

    #[test]
    fn highlight_tracks_fractional_layout_y() {
        let rgba = vec![255u8; 320 * 40 * 4];
        let img = crate::codec::tga::TgaImage { width: 320, height: 40, rgba };
        let sprite = super::scale_sprite(&img, 0, 0, 320, 40, 480, 60);
        let fitted = super::FittedMenu {
            ww: 1920,
            wh: 1080,
            stamp: 0,
            plate: crate::render::Surface::new(1, 1, [0, 0, 0, 255]),
            bar: [Some(sprite), None],
        };
        let mut a = crate::render::Surface::new(1920, 1080, [1, 29, 89, 255]);
        let mut b = crate::render::Surface::new(1920, 1080, [1, 29, 89, 255]);
        super::paint_bar(&mut a, &fitted, 0, 432.0, 480);
        super::paint_bar(&mut b, &fitted, 0, 440.0, 480);
        let top = |s: &crate::render::Surface| -> i32 {
            for y in 0..s.height {
                for x in 0..s.width {
                    let i = ((y * s.width + x) * 4) as usize;
                    if s.pixels[i] > 200 {
                        return y as i32;
                    }
                }
            }
            -1
        };
        let dy = top(&b) - top(&a);
        assert!((dy - 12).abs() <= 1, "dy {dy}");
    }
}

#[cfg(test)]
mod rewind_history {
    #[test]
    fn rewind_drops_the_line_on_screen_and_keeps_the_previous() {
        let mut history = vec!["a", "b", "c"];
        assert_eq!(super::take_rewind(&mut history, true).as_deref(), Some("b"));
        assert_eq!(history, ["a", "b"]);
        assert_eq!(super::take_rewind(&mut history, true).as_deref(), Some("a"));
        assert_eq!(history, ["a"]);
        assert!(super::take_rewind(&mut history, true).is_none());
        assert_eq!(history, ["a"]);
        // A choice has not pushed a frame of its own, so the line stays put.
        assert_eq!(super::take_rewind(&mut history, false).as_deref(), Some("a"));
        assert_eq!(history, ["a"]);
    }
}

mod backlog_paint {
    #[test]
    fn history_paints_the_banner_and_the_lines() {
        let atlas = crate::codec::tga::decode_font(
            &std::fs::read("/home/lrxin/wa2-native/data/font/cn_body80.tga").unwrap(),
        )
        .unwrap();
        let font = crate::render::Font::from_cell_map(
            atlas,
            40,
            std::path::Path::new("/home/lrxin/wa2-native/data/font/wa2_cell_map_cn.tsv"),
        )
        .unwrap();
        let dirs = [
            std::path::Path::new("/home/lrxin/wa2-native/data/bg_cn"),
            std::path::Path::new("/home/lrxin/wa2-native/data/bg"),
        ];
        let header = super::load_background(&dirs, "sys_00032.tga");
        let scroll = super::load_background(&dirs, "sys_00030.tga");
        assert!(header.is_some());
        assert!(scroll.is_some());
        let scene = crate::render::Surface::new(1280, 720, [80, 120, 160, 255]);
        let lines = [
            "春希：不我".to_string(),
            "春希：不我".to_string(),
        ];
        let rows = super::backlog_rows(&lines, None);
        assert!(rows.len() > 2, "a name and a body should not collapse into one row");
        assert!(rows.iter().any(|r| r.name), "the speaker should be its own row");
        let per_page = 19;
        let top = rows.len().saturating_sub(per_page);
        let surf = super::paint_backlog(
            &scene,
            &font,
            None,
            &rows,
            top,
            per_page,
            header.as_ref(),
            scroll.as_ref(),
            false,
            false,
        );
        // Banner sits on the top edge and is not the flat scene colour.
        let i = (20 * 1280 + 200) * 4;
        assert!(surf.pixels[i + 2] > surf.pixels[i], "banner should be cyan, not grey");
        let _ = std::fs::create_dir_all("/tmp/wa2btn");
        let _ = surf.save_tga("/tmp/wa2btn/backlog.tga");
    }

    /// The history is the last screen that still reached for the atlas. In
    /// Chinese mode it draws with the vector font, and the two renders have to
    /// differ - that is the whole point of the change.
    #[test]
    fn history_paints_the_lines_with_the_vector_font() {
        let Some(mut vf) = crate::render::text::VectorFont::open(
            crate::render::text::DEFAULT_FONT,
            crate::render::DIALOG_FONT_720,
        ) else {
            return;
        };
        let dirs = [
            std::path::Path::new("/home/lrxin/wa2-native/data/bg_cn"),
            std::path::Path::new("/home/lrxin/wa2-native/data/bg"),
        ];
        let header = super::load_background(&dirs, "sys_00032.tga");
        let scroll = super::load_background(&dirs, "sys_00030.tga");
        let scene = crate::render::Surface::new(1280, 720, [80, 120, 160, 255]);
        let lines = vec![
            "春希：不我".to_string(),
            "冬马和纱：白色的季节里".to_string(),
        ];
        let rows = super::backlog_rows(&lines, Some(&vf));
        assert!(rows.iter().any(|r| r.name), "speaker row is missing");
        let painted = super::paint_backlog(
            &scene,
            &crate::render::Font::from_cell_map(
                crate::codec::tga::decode_font(
                    &std::fs::read("/home/lrxin/wa2-native/data/font/cn_body80.tga").unwrap(),
                )
                .unwrap(),
                40,
                std::path::Path::new("/home/lrxin/wa2-native/data/font/wa2_cell_map_cn.tsv"),
            )
            .unwrap(),
            Some(&mut vf),
            &rows,
            0,
            19,
            header.as_ref(),
            scroll.as_ref(),
            false,
            false,
        );
        // Ink where the lines are, in the text column and below the banner.
        let mut ink = 0usize;
        for y in 52..680 {
            for x in 72..1160 {
                let i = ((y * 1280 + x) * 4) as usize;
                if painted.pixels[i] > 200 {
                    ink += 1;
                }
            }
        }
        assert!(ink > 200, "vector history drew no glyphs ({ink} px)");
        let _ = painted.save_tga("/tmp/wa2btn/backlog_vector.tga");
    }

    #[test]
    fn quick_load_confirm_paints_yes_and_no() {
        let dirs = [
            std::path::Path::new("/home/lrxin/wa2-native/data/bg_cn"),
            std::path::Path::new("/home/lrxin/wa2-native/data/bg"),
        ];
        let plate = super::load_background(&dirs, "sys_00100.tga").unwrap();
        let buttons = super::load_background(&dirs, "sys_00101.tga").unwrap();
        let scene = crate::render::Surface::new(1280, 720, [20, 20, 20, 255]);
        let text = crate::render::text::with_ui_font(22, |f| {
            f.line_rgba("要读取快速存档吗？", 22)
        })
        .flatten();
        assert!(text.is_some(), "system font should rasterize the prompt");
        let surf = super::paint_confirm(&scene, Some(&plate), Some(&buttons), text.as_ref(), Some(true));
        let (px, py) = super::centered_plate(plate.width as i32, plate.height as i32);
        let boxes = super::ask_buttons(px, py, plate.width as i32, plate.height as i32);
        let sample = |x: i32, y: i32| {
            let i = ((y as u32 * 1280 + x as u32) * 4) as usize;
            surf.pixels[i] as u16 + surf.pixels[i + 1] as u16 + surf.pixels[i + 2] as u16
        };
        let yes = sample(boxes[0].0 + 80, boxes[0].1 + 20);
        let no = sample(boxes[1].0 + 80, boxes[1].1 + 20);
        assert!(yes > 400, "yes button should be the gold hover row, sum {yes}");
        assert!(no > 400, "no button should still be the idle row, sum {no}");
        assert!(yes != no, "hover should change the yes button");
        let _ = std::fs::create_dir_all("/tmp/wa2btn");
        let _ = surf.save_tga("/tmp/wa2btn/confirm.tga");
    }
}

#[cfg(test)]
mod bg_index_tests {
    use super::BgIndex;
    use std::path::PathBuf;

    struct TempTree(PathBuf);
    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tree(tag: &str, files: &[&str]) -> TempTree {
        let dir = std::env::temp_dir().join(format!("wa2-bgidx-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for f in files {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        TempTree(dir)
    }

    #[test]
    fn a_missing_variant_falls_back_to_the_background_family() {
        // 3005 asks for id 2067 variant 0; only b206702/b206712 were shipped,
        // and v206700 is an unrelated event CG.
        let d = tree("family", &["b206702.tga", "b206712.tga", "v206700.tga", "tv206700.tga"]);
        let index = BgIndex::build(&[d.0.as_path()]);
        assert_eq!(index.resolve(2067, 0).as_deref(), Some("b206702.tga"));
        assert_eq!(index.resolve(2067, 1).as_deref(), Some("b206702.tga"));
    }

    #[test]
    fn an_event_cg_stands_in_only_when_no_background_was_shipped() {
        let d = tree("cgonly", &["v100100.tga", "v100101.tga"]);
        let index = BgIndex::build(&[d.0.as_path()]);
        assert_eq!(index.resolve(1001, 0).as_deref(), Some("v100100.tga"));
        assert_eq!(index.resolve(1001, 1).as_deref(), Some("v100101.tga"));
        let d2 = tree("cgonly2", &["v100100.tga"]);
        let index2 = BgIndex::build(&[d2.0.as_path()]);
        assert_eq!(index2.resolve(1001, 5).as_deref(), Some("v100100.tga"));
    }

    #[test]
    fn the_exact_background_variant_wins() {
        let d = tree("exact", &["b101701.tga", "b101700.tga", "v101700.tga"]);
        let index = BgIndex::build(&[d.0.as_path()]);
        assert_eq!(index.resolve(1017, 1).as_deref(), Some("b101701.tga"));
        assert_eq!(index.resolve(1017, 0).as_deref(), Some("b101700.tga"));
    }
}

#[cfg(test)]
mod choice_ui_tests {
    use super::{choice_row_centers, choice_row_layout};

    #[test]
    fn choices_sit_where_the_original_screen_puts_them() {
        assert_eq!(choice_row_centers(1), vec![262]);
        assert_eq!(choice_row_centers(2), vec![193, 331]);
        assert_eq!(choice_row_centers(3), vec![124, 262, 400]);
    }

    #[test]
    fn a_branch_of_four_still_fits_above_the_dialogue_box() {
        let rows = choice_row_layout(2, 720);
        assert_eq!(rows[0].top, 157);
        assert_eq!(rows[1].top, 295);
        assert_eq!(rows[0].height, 72);
        let rows = choice_row_layout(4, 720);
        assert!(rows.first().unwrap().top >= 24, "{rows:?}");
        let last = rows.last().unwrap();
        assert!(last.top + last.height <= 476, "{rows:?}");
    }

    #[test]
    fn rows_scale_with_the_window() {
        let rows = choice_row_layout(2, 1080);
        assert_eq!(rows[0].height, 108);
        assert_eq!(rows[0].top, 236);
        assert_eq!(rows[1].top, 443);
    }
}

#[cfg(test)]
mod font_sync_tests {
    use super::game_font_is_stale;

    /// A run started from the title has to rebuild the Game instance exactly
    /// when the profile id moved; Japanese mode (no vector dialogue font)
    /// never does.
    #[test]
    fn game_font_rebuild_is_keyed_on_the_profile_id() {
        assert!(game_font_is_stale(true, 0, 1234));
        assert!(game_font_is_stale(true, 1234, 0));
        assert!(!game_font_is_stale(true, 1234, 1234));
        assert!(!game_font_is_stale(false, 0, 1234));
    }
}

/// ACT170 (ambient light) and ACT172 (explicit filter) share one curve slot.
/// The 86 scenes that use both keep their filter behaviour only if the
/// explicit filter wins; the per-background `.amp` override outranks the
/// preset but not the filter.
#[cfg(test)]
mod ambient_tint_tests {
    use super::{ambient_preset_name, background_override_name, effect_name, effective_tint_name};

    fn s(name: &str) -> Option<String> {
        Some(name.to_string())
    }

    #[test]
    fn nothing_set_means_no_tint() {
        assert_eq!(effective_tint_name(&None, &None, &None), None);
    }

    #[test]
    fn the_preset_stands_alone() {
        assert_eq!(
            effective_tint_name(&None, &None, &s("night.amp")),
            s("night.amp")
        );
    }

    #[test]
    fn the_background_override_outranks_the_preset() {
        assert_eq!(
            effective_tint_name(&None, &s("b205001.amp"), &s("night.amp")),
            s("b205001.amp")
        );
    }

    #[test]
    fn the_explicit_filter_outranks_everything() {
        assert_eq!(
            effective_tint_name(&s("sepia.AMP"), &s("b205001.amp"), &s("night.amp")),
            s("sepia.AMP")
        );
        assert_eq!(
            effective_tint_name(&s("sepia.AMP"), &None, &s("night.amp")),
            s("sepia.AMP")
        );
    }

    /// ACT172 with an empty token clears the filter, so the ambient light
    /// shows through again.
    #[test]
    fn clearing_the_filter_falls_back_to_the_ambient_light() {
        assert_eq!(
            effective_tint_name(&None, &s("b202501.amp"), &None),
            s("b202501.amp")
        );
    }

    #[test]
    fn background_override_uses_variant_and_preset_as_separate_digits() {
        assert_eq!(background_override_name(2025, 0, 1), "b202501.amp");
        assert_eq!(background_override_name(2025, 1, 1), "b202511.amp");
        assert_eq!(background_override_name(2096, 1, 1), "b209611.amp");
        assert_eq!(background_override_name(2, 0, 0), "b000200.amp");
    }

    #[test]
    fn high_bit_background_modes_select_f0_effects() {
        assert_eq!(effect_name(0x80).as_deref(), Some("f0000.bmp"));
        assert_eq!(effect_name(0xA1).as_deref(), Some("f0033.bmp"));
        assert_eq!(effect_name(1), None);
        assert_eq!(ambient_preset_name(3).as_deref(), Some("indoor.amp"));
    }
}
