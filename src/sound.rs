//! Interface sounds.

use std::path::Path;

use crate::*;

/// The original's system UI sound. WA2.exe plays SE_8905 (id 0x22c9) from the
/// two tiny wrappers at 0x40ea10 / 0x40ea30 for menu interactions; the branch
/// choice rows use the game-wide UI sound for hover and click.
pub(crate) const UI_SE_ID: u32 = 8905;

pub(crate) fn ui_se_path(script_dir: &Path) -> Option<std::path::PathBuf> {
    let dir = script_dir.parent()?.join("se");
    ["WAV", "OGG", "wav", "ogg"]
        .iter()
        .map(|ext| dir.join(format!("SE_{:04}.{}", UI_SE_ID, ext)))
        .find(|p| p.is_file())
}

pub(crate) fn play_ui_se(mixer: Option<&audio::Mixer>, path: Option<&Path>) {
    if let (Some(mx), Some(path)) = (mixer, path) {
        if mx.play_cat(&path.to_string_lossy(), 1.0, false, audio::mixer::CAT_SE) != usize::MAX {
            eprintln!("  SE: choice UI id={} -> {}", UI_SE_ID, path.display());
        }
    }
}

// Menu sounds. Every screen names what it is doing (`UiSe`) instead of
// picking an SE id, so the mapping lives in exactly one place.

/// `WA2.exe` wraps its SE player three times in one cluster: 0x40ea10 and
/// 0x40ea30 push 0x22c9 (`SE_8905`) and 0x40ea50 pushes 0x22c7 (`SE_8903`).
/// The archives only ship `SE_8905`, so cancel falls back to it instead of
/// staying silent. `SE_9011` is the short two-tap click the title highlight
/// and the menu cursor move to.
pub(crate) const SE_UI: u32 = 8905;

pub(crate) const SE_UI_CANCEL: u32 = 8903;

pub(crate) const SE_CLICK: u32 = 9011;

/// Which menu sound an interaction wants.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UiSe {
    Move,
    Confirm,
    Cancel,
    /// Page flips and prev/next paging: the cursor click, not the menu chime.
    Page,
}

impl UiSe {
    /// Index into [`UiSound::pcm`].
    pub(crate) fn slot(self) -> usize {
        match self {
            UiSe::Move => 0,
            UiSe::Confirm => 1,
            UiSe::Cancel => 2,
            UiSe::Page => 3,
        }
    }
}

/// The four menu sounds, resolved once per screen.
pub struct UiSound {
    pub(crate) mv: Option<std::path::PathBuf>,
    pub(crate) ok: Option<std::path::PathBuf>,
    pub(crate) cancel: Option<std::path::PathBuf>,
    pub(crate) page: Option<std::path::PathBuf>,
    /// Decoded once and kept for the lifetime of the screen.
    ///
    /// A menu sound plays several times a second while the player browses, and
    /// decoding one through a decoder subprocess cost 31 ms warm and 189 ms
    /// cold, with the mixer lock held the whole time. That is visible as a
    /// frozen frame and audible as a hole in the BGM, so the pixel-accurate
    /// short sounds are simply held in memory instead.
    pub(crate) pcm: [Option<std::sync::Arc<Vec<i16>>>; 4],
}

impl UiSound {
    /// Resolve every role. `dirs` are the art trees; `se/` sits next to them.
    pub fn resolve(dirs: &[&std::path::Path]) -> UiSound {
        let mv = find_se(dirs, SE_CLICK);
        let ok = find_se(dirs, SE_UI);
        let cancel = find_se(dirs, SE_UI_CANCEL).or_else(|| ok.clone());
        let page = mv.clone();
        let pcm = [
            cached_pcm(mv.as_deref()),
            cached_pcm(ok.as_deref()),
            cached_pcm(cancel.as_deref()),
            cached_pcm(page.as_deref()),
        ];
        UiSound { mv, ok, cancel, page, pcm }
    }

    /// Decoded buffer for a role, if one could be prepared.
    pub fn pcm(&self, kind: UiSe) -> Option<&std::sync::Arc<Vec<i16>>> {
        self.pcm[kind.slot()].as_ref()
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

/// Process-wide cache of decoded menu sounds.
///
/// Every menu screen calls `UiSound::resolve` when it opens, and they all ask
/// for the same four files. Without this each screen would pay the decode again
/// on open.
pub(crate) fn cached_pcm(path: Option<&std::path::Path>) -> Option<std::sync::Arc<Vec<i16>>> {
    type Cache =
        std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, Option<std::sync::Arc<Vec<i16>>>>>;
    static CACHE: std::sync::OnceLock<Cache> = std::sync::OnceLock::new();
    let path = path?;
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut g = cache.lock().unwrap();
    if let Some(hit) = g.get(path) {
        return hit.clone();
    }
    // A failure is cached too: retrying the decode on every screen would only
    // add the same 31 ms to each open.
    let decoded = audio::mixer::decode_pcm(path);
    g.insert(path.to_path_buf(), decoded.clone());
    decoded
}

/// `SE_{id:04}` next to the art trees, in either container format.
pub(crate) fn find_se(dirs: &[&std::path::Path], id: u32) -> Option<std::path::PathBuf> {
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
    if let Some(buf) = s.pcm(kind) {
        mx.play_pcm(std::sync::Arc::clone(buf), 1.0, false, audio::mixer::CAT_SE);
        return;
    }
    // Only reached if the file could not be decoded up front.
    let Some(p) = s.path(kind) else { return };
    mx.play_cat(&p.to_string_lossy(), 1.0, false, audio::mixer::CAT_SE);
}
