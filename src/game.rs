//! The runtime and the script interpreter that drives a scene.

use crate::*;

/// Everything that outlives a single scene.
///
/// Quick-loading a save used to rebuild all of this inside run_game_at: the
/// audio device was closed and reopened, the 46k-clip voice directory was
/// scanned again, and the UI sheets were decoded again. Keeping them here means
/// a scene change only replaces what the scene actually owns.
pub struct Runtime {
    /// Vector font for Chinese dialogue (None in Japanese mode).
    pub(crate) vfont: Option<render::text::VectorFont>,
    /// Profile id the dialogue font was opened from. The title screen can
    /// change the Game role while this Runtime is already alive, so the first
    /// scene of a game started from the title has to notice the mismatch and
    /// rebuild the instance (F-01).
    pub(crate) game_font_id: i32,
    pub(crate) bar: cmdbar::CmdBar,
    pub(crate) msg_plate: Option<codec::tga::TgaImage>,
    pub(crate) msg_frame: Option<codec::tga::TgaImage>,
    pub(crate) voices: Option<VoiceIndex>,
    pub(crate) mixer: Option<audio::Mixer>,
    pub(crate) bank: Option<SpriteBank>,
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
pub(crate) fn game_font_is_stale(cn: bool, open_id: i32, want_id: i32) -> bool {
    cn && open_id != want_id
}

pub(crate) fn run_game_at(
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
    // True while 快进 is only on because Control is held, so releasing the key
    // can turn it back off without disturbing the sticky 快進 button.
    let mut skip_by_ctrl = false;
    let mut ctrl_prev = false;
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
                    // ACT146/147. id 0 is a scripted black screen. A negative
                    // id is not a picture at all - see below. A missing file
                    // keeps the picture already on screen.
                    let old = clone_surf(&cur_surf);
                    effect_mode = mode;
                    effect = load_effect(&s.bg_dirs(), effect_mode);
                    match bg_request(id) {
                        BgRequest::Keep => {
                            // The negative sentinel: re-composite the shot
                            // that is already on screen. See `bg_request`.
                            eprintln!(
                                "scene {} | bg unchanged (script id {} fade {}f)",
                                current_id, id, frames
                            );
                        }
                        BgRequest::Black => {
                            plates.clear();
                            bg_cg = false;
                            bg_id = None;
                            bg_label = "BLACK".to_string();
                            bg = solid_bg(W, H, [0, 0, 0]);
                            bg_scale = None;
                            bg_src_scale = None;
                            // id 0 is a fade-out: the shot is over, so the
                            // people from it must leave. Without this, 依緒 and
                            // 雪菜 stay on screen through the whole rest of
                            // 1012_030, including the park and the narration
                            // that is only about かずさ.
                            slots = Default::default();
                            eprintln!(
                                "scene {} | bg BLACK (script id {} fade {}f)",
                                current_id, id, frames
                            );
                        }
                        BgRequest::Picture => {
                            if let Some(name) = s.bg_index.resolve(id, variant) {
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
                                    // Per-background ambient override: the exe
                                    // uses the background id, its one-digit
                                    // variant, and the active ACT170 preset as
                                    // three separate sprintf arguments
                                    // (`b{ID}{digit}{preset}.amp`).
                                    let ov =
                                        background_override_name(id, variant, ambient_preset_id);
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
                        }
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
                vm::lscr::Event::ScrollCg { id, variant, frames, pages } => {
                    // ACT227. Two pictures in the game are several screens
                    // tall (v201900 is 1280x2160, v200000 is 1280x3362) and the
                    // original drags them up through the frame like a scroll;
                    // everything else the opcode shows behaves like an ACT148.
                    // Without this arm the black ACT146 in front of it stayed
                    // on screen for the whole scene.
                    let old = clone_surf(&cur_surf);
                    let name = event_cg_name(id, variant);
                    match load_background_asset(&s.bg_dirs(), &name) {
                        Some(asset) => {
                            plates.clear();
                            slots = Default::default();
                            bg_cg = true;
                            bg_id = Some((id, variant));
                            bg_label = name.clone();
                            bg_scale = Some(asset.art_scale);
                            bg_src_scale = Some(asset.src_scale);
                            let img = asset.img;
                            // One screenful of the picture, in its own pixels.
                            let page_h = (img.width as u64 * 9 / 16) as u32;
                            // How far the window walks. The record names it
                            // outright ("720, 2" for v201900, "720, 3" for
                            // v200000); a short picture says 0 and gets the
                            // height it happens to have.
                            let travel = if pages > 0 {
                                (pages as u64 * page_h as u64)
                                    .min(img.height.saturating_sub(page_h) as u64) as u32
                            } else {
                                img.height.saturating_sub(page_h)
                            };
                            eprintln!(
                                "scene {} | cg-scroll {} id={} var={} fade={}f pages={} ({}x{}, travel {})",
                                current_id, name, id, variant, frames, pages, img.width, img.height, travel
                            );
                            let dragged = travel > 2 && !cmd_skip;
                            if dragged {
                                // The script's own transition length, which for
                                // these pictures is the drag itself.
                                let steps = frames.clamp(90, 200);
                                let mut quit = false;
                                for step in 0..=steps {
                                    // The window starts on the last screenful
                                    // of the picture - her feet - and is
                                    // dragged up to the first, her face. The
                                    // smoothstep keeps both ends of the move
                                    // slow, the way a camera pull reads.
                                    let t = step as f64 / steps as f64;
                                    let e = t * t * (3.0 - 2.0 * t);
                                    let y = (travel as f64 * (1.0 - e)) as u32;
                                    let Some(crop) = crop_image(&img, 0, y, img.width, page_h) else { break };
                                    bg = crop;
                                    line_base = None;
                                    line_done = true;
                                    cur_surf = draw_scene(&bg, "", None, font, &slots, focus, win_size.0 as u32, win_size.1 as u32, tint.as_deref(), effect.as_ref(), window, msg_plate.as_ref(), msg_frame.as_ref(), &plates, scene_look(&settings, bg_cg), vfont.as_mut());
                                    win.present(&cur_surf);
                                    // A click during the drag drops the rest of
                                    // it and keeps the picture, like every other
                                    // timed beat in the engine.
                                    match win.poll_input() {
                                        sdl2::Input::Quit => { quit = true; break }
                                        sdl2::Input::None
                                        | sdl2::Input::Pointer { pressed: false, .. }
                                        | sdl2::Input::Drag { .. } => {}
                                        _ => break,
                                    }
                                    win.delay(16);
                                }
                                if quit {
                                    return Ok(None);
                                }
                            }
                            // The picture comes to rest on its first
                            // screenful: the drag ended at her face, so that
                            // is what stays behind the dialogue.
                            if let Some(crop) = crop_image(&img, 0, 0, img.width, page_h) {
                                bg = crop;
                            }
                            line_base = None;
                            line_done = true;
                            cur_surf = draw_scene(&bg, "", None, font, &slots, focus, win_size.0 as u32, win_size.1 as u32, tint.as_deref(), effect.as_ref(), window, msg_plate.as_ref(), msg_frame.as_ref(), &plates, scene_look(&settings, bg_cg), vfont.as_mut());
                            let fade = if dragged { 0 } else { fade_len(&settings, cmd_skip, frames) };
                            if present_fade(win, &old, &cur_surf, fade) {
                                return Ok(None);
                            }
                        }
                        None => eprintln!(
                            "scene {} | cg-scroll MISSING {} id={} var={}, screen stays {}",
                            current_id, name, id, variant, bg_label
                        ),
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
                            // ⌃ cuts the hold short exactly like the 快進
                            // button does, so a key-held fast-forward is not
                            // stuck watching a two-second camera pause.
                            ctrl_skip(win.ctrl_held(), &mut ctrl_prev, &mut cmd_skip, &mut skip_by_ctrl);
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
                                    let path = bgm_file(&root.join("bgm"), id);
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
                    // The row geometry is kept in the 720-line layout space so it
                    // matches the hit-testing (`surface_pos` is also fed 1280x720)
                    // and `bar.hit`'s fixed layout constants. `paint_choice_rows`
                    // scales it onto the actual (possibly high-DPI) frame.
                    let row_layout = choice_row_layout(opts.len(), H);
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
                        // The choice screen's frame only changes when the hover,
                        // the command-bar highlight, or a toggle moves. Between
                        // those it is byte-for-byte the last presented frame, so
                        // rebuilding it every 16 ms - cloning the whole (possibly
                        // 2560x1440) scene and re-rasterising the option rows -
                        // is what made moving the mouse across the options feel
                        // laggy. Cache the last frame and its state signature and
                        // only recompose when the signature actually changed.
                        let mut cached_frame: Option<render::Surface> = None;
                        let mut cached_sig: Option<(Option<usize>, Option<cmdbar::Cmd>, bool, bool, bool, bool)> =
                            None;
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
                            let sig = (hover, bar_hover, cmd_auto, cmd_skip, cmd_rewind, user_hide);
                            let rebuild = cached_frame.is_none() || cached_sig != Some(sig);
                            if rebuild {
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
                                cached_frame = Some(frame);
                                cached_sig = Some(sig);
                            } else if let Some(f) = cached_frame.as_ref() {
                                win.present(f);
                            }
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
                                                                // The scene underneath the
                                                                // rows changed but none of
                                                                // the state in the cached
                                                                // signature did, so drop the
                                                                // stale frame.
                                                                cached_frame = None;
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
                            // Holding ⌃ is 快进 for as long as it is down. The
                            // bar button remains the toggle; see `ctrl_skip`.
                            match ctrl_skip(win.ctrl_held(), &mut ctrl_prev, &mut cmd_skip, &mut skip_by_ctrl) {
                                Some(true) => {
                                    // Same bookkeeping as pressing 快進: the
                                    // clip for the line on screen stops, and the
                                    // line it belongs to is remembered so the
                                    // voice can resume when skip ends.
                                    window = dialog_open(user_hide);
                                    if let Some(mx) = mixer.as_ref() {
                                        stop_voice(mx, &mut voice_stream);
                                    }
                                }
                                Some(false) => {
                                    window = dialog_open(user_hide);
                                    if let (Some(mx), Some(vi), Some(cue)) =
                                        (mixer.as_ref(), voices.as_ref(), skipped_voice.take())
                                    {
                                        play_voice(mx, vi, &settings, &mut voice_stream, cue, &current_id);
                                    }
                                }
                                None => {}
                            }
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

/// The Chinese patch keeps its text in `script_cn`: Shift-JIS codes whose
/// glyphs were repainted, which `codec::sjis::decode_cn` resolves.
pub(crate) fn is_cn_script(dir: &std::path::Path) -> bool {
    dir.file_name().and_then(|n| n.to_str()) == Some("script_cn")
}

/// Load and parse an LSCR program (`.bnr` + companion `.txt` token pool).
pub(crate) fn load_program(script_dir: &std::path::Path, id: &str, cn: bool) -> Option<vm::lscr::Program> {
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

pub(crate) fn cmd_idle(settings: &menu::Settings) -> u8 {
    (settings.alpha_command.clamp(0, menu::LEVELS) as u32 * 100 / menu::LEVELS as u32) as u8
}

/// Hold Control to fast-forward, the way a visual novel normally does it.
///
/// The engine only ever exposed 快進 as the command-bar button, which is a
/// sticky toggle: click once to start skipping, click again to stop. The key
/// has to behave like a *hold* instead, so it is tracked separately from the
/// button - it turns skip on when the key goes down, and turns it back off
/// when the key comes up **only if the key was what turned it on**. Toggling
/// with the button while the key is down, or 「未读文本」 dropping out of skip on
/// an unseen line, therefore still wins.
///
/// Returns `Some(true)` when this call started a skip, `Some(false)` when it
/// ended one, and `None` when nothing changed - the caller uses that to stop
/// or resume the voice of the line on screen.
pub(crate) fn ctrl_skip(
    down: bool,
    prev_down: &mut bool,
    cmd_skip: &mut bool,
    by_ctrl: &mut bool,
) -> Option<bool> {
    // Only the edges do anything. While the key just sits there it must not
    // take skip back from a click or from 「未读文本」, or the player could not
    // get out of fast-forward without letting go of the key first.
    let rising = down && !*prev_down;
    let falling = !down && *prev_down;
    *prev_down = down;
    if rising {
        // Pressing the key while the button already has skip on just rides
        // along: the button keeps ownership, so the release does not stop it.
        if *cmd_skip {
            return None;
        }
        *cmd_skip = true;
        *by_ctrl = true;
        return Some(true);
    }
    if falling {
        let owned = *by_ctrl;
        *by_ctrl = false;
        if owned && *cmd_skip {
            *cmd_skip = false;
            return Some(false);
        }
    }
    None
}

/// The box is up unless the player hid it. The script no longer folds it:
/// ACT170 turned out to be an ambient light preset, not a window switch.
pub(crate) fn dialog_open(user_hide: bool) -> bool {
    !user_hide
}

pub(crate) fn load_seen() -> std::collections::HashSet<String> {
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

pub(crate) fn note_seen(set: &mut std::collections::HashSet<String>, scene: &str, text: &str) {
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

/// Older saves stored the line as `【name】\nbody`. Split that so a loaded
/// slot still puts the name on the rim.
pub(crate) fn split_saved_line(text: &str) -> (Option<String>, String) {
    if let Some(rest) = text.strip_prefix('【') {
        if let Some((name, body)) = rest.split_once("】\n") {
            return (Some(name.to_string()), body.to_string());
        }
    }
    (None, text.to_string())
}

/// Clean scenario text for rendering: literal `\n` -> newline, `\k` removed,
/// ruby tags `<Rbase|reading>` collapsed to `base`, other `<...>` tags stripped.
pub(crate) fn clean_text(s: &str) -> String {
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
