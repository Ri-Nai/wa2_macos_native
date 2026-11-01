//! Voice playback and the rules that decide when a line may be spoken.

use crate::*;

/// Exact voice lookup by the identifiers from ACT137 and ACT138.
pub(crate) struct VoiceIndex {
    pub(crate) map: std::collections::HashMap<(u32, u32, u32), std::path::PathBuf>,
}

impl VoiceIndex {
    pub(crate) fn build(dir: &std::path::Path) -> VoiceIndex {
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

    pub(crate) fn find(&self, cue: vm::lscr::VoiceCue) -> Option<&std::path::Path> {
        self.map.get(&(cue.scenario, cue.line, cue.char_id)).map(|p| p.as_path())
    }
}

pub(crate) fn stop_voice(mx: &audio::Mixer, voice_stream: &mut Option<usize>) {
    if let Some(slot) = voice_stream.take() {
        mx.stop(slot);
    }
}

/// Stop all scene-owned audio and clear any modal-screen streams that may not
/// have a scene slot. The latter matters when returning Home and immediately
/// starting another game: the title/music screen and the game share one SDL
/// mixer, so leaving an untracked BGM slot alive makes the next scene sound
/// like another interface.
pub(crate) fn stop_game_audio(
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
pub(crate) const H_SCENE_IDS: [&str; 18] = [
    "1012_030_2", "2031", "2312", "2313", "2401", "2402", "2504", "2507",
    "2511", "2513", "3001", "3015", "3210", "3901", "3902", "3904", "3906", "3907",
];

pub(crate) fn is_h_scene_id(scene: &str) -> bool {
    H_SCENE_IDS.iter().any(|candidate| *candidate == scene)
}

/// Character ids that the settings page classifies as female. Unknown ids are
/// treated as male for the female-only H-scene mode, which avoids accidentally
/// playing an unclassified male voice.
pub(crate) fn is_female_voice_char(char_id: u32) -> bool {
    matches!(char_id, 1 | 2 | 3 | 4 | 5 | 16 | 19 | 23 | 24 | 25)
}

pub(crate) fn h_scene_voice_allowed(settings: &menu::Settings, scene: &str, char_id: u32) -> bool {
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
pub(crate) fn play_voice(
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
