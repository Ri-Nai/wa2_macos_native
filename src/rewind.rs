//! Backtracking: the line history, the rewind steps and the save snapshot.

use crate::*;

/// One line the player already stood on. Rewind restores this instead of
/// replaying the script backwards.
#[derive(Clone)]
pub(crate) struct LineSnap {
    pub(crate) scene: String,
    pub(crate) vm: vm::lscr::InterpState,
    pub(crate) bg: Option<(i64, u32)>,
    pub(crate) effect_mode: i32,
    pub(crate) ambient_preset: i32,
    pub(crate) bg_cg: bool,
    pub(crate) bg_label: String,
    pub(crate) tint_name: Option<String>,
    pub(crate) slots: Vec<Option<(u32, String)>>,
    pub(crate) focus: Option<u32>,
    pub(crate) text: String,
    pub(crate) speaker: Option<String>,
    pub(crate) bgm: Option<i32>,
    pub(crate) plates: Vec<(u32, String)>,
    pub(crate) voice: Option<vm::lscr::VoiceCue>,
}

pub(crate) const HISTORY_MAX: usize = 400;

pub(crate) fn slot_keys(slots: &[Option<ActiveSprite>; SPRITE_SLOTS]) -> Vec<Option<(u32, String)>> {
    slots.iter().map(|s| s.as_ref().map(|a| (a.char_id, a.name.clone()))).collect()
}

pub(crate) fn plate_keys(plates: &[Plate]) -> Vec<(u32, String)> {
    plates.iter().map(|p| (p.slot, p.name.clone())).collect()
}

pub(crate) fn capture_line(
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

pub(crate) fn remember(history: &mut Vec<LineSnap>, snap: LineSnap) {
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
pub(crate) fn take_rewind<T: Clone>(history: &mut Vec<T>, pop_current: bool) -> Option<T> {
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

pub(crate) enum RewindStep {
    /// Nothing earlier than the line on screen.
    Stuck,
    /// Picture and interpreter are back on an earlier line of this script.
    Shown,
    /// The earlier line belongs to another script. `rewind_vm` is set.
    Reload,
}

/// Put `dest` on screen and arm the interpreter when the script matches.
pub(crate) fn rewind_step(
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
                let path = bgm_file(&dir, id);
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
pub(crate) fn make_save(
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
