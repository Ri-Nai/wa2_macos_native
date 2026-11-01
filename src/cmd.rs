//! The command bar and the modal prompts it opens.

use crate::*;

/// Result of handling a command-bar button that opens a modal screen.
pub(crate) enum CmdOutcome {
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

/// Handle a command-bar button that opens a modal screen (保存/读档/设置/履历).
/// The toggle buttons (自动/跳过/隐藏) are handled inline by the caller.
pub(crate) fn handle_cmd(
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
            menu::run(win, &dirs, &mut st, mixer, Some(&ui), Some(frame));
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
pub(crate) fn refresh_dialog_font(cn: bool, vfont: Option<&mut render::text::VectorFont>) {
    if !cn {
        return;
    }
    let Some(dst) = vfont else { return };
    if let Some(nf) = render::text::open_role_font(render::text::FontRole::Game, render::DIALOG_FONT_720) {
        *dst = nf;
    }
}
