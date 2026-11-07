//! Save slots.
//!
//! A save is a snapshot of everything the engine needs to put the player back
//! where they were:
//!
//! ```text
//!   scene id            which .bnr/.txt to reload
//!   interpreter state   pc + the transient fields of `Interp`
//!   background id       re-resolved through `BgIndex` on load
//!   ACT146/147 mode     re-resolves the optional `f0NNN` effect plate
//!   ACT170 preset       retained for later background override lookup
//!   sprite file names   re-resolved through `SpriteBank` on load
//!   tint name           re-resolved through `load_tint` on load
//!   last line of text   the dialog box is not state, it has to be stored
//!   BGM track id        mixer slots are live handles and cannot be saved
//! ```
//!
//! Only ids and names are stored, never decoded pixels: every asset is
//! re-resolved on load, the same way a fresh scene start resolves them.
//!
//! Layout follows the original (`save_%02d.sav` plus a thumbnail next to it),
//! because the save screen shows a thumbnail per slot. The format itself is
//! ours: line oriented `key=value`, versioned, with a global-variable section.
//! The original keeps these values outside the scene VM; the native port
//! stores their indexed values and rebuilds the schema from `Global.vrb`.

use crate::vm::lscr::{InterpState, VoiceCue};
use std::collections::HashMap;
use std::path::PathBuf;

/// Bumped whenever the meaning of a field changes.
pub const VERSION: u32 = 2;
/// `save_00` .. `save_99`, matching the original's two-digit slot names.
pub const SLOT_COUNT: usize = 100;
/// Slots shown per page by the save screen.
pub const PER_PAGE: usize = 12;

/// Pages needed to cover every slot. Integer division of 100/12 drops the last
/// four slots (`save_96`..`save_99`); the original UI has nine pages.
pub fn page_count() -> usize {
    SLOT_COUNT.div_ceil(PER_PAGE)
}

/// Absolute slot for a grid cell, or `None` if that cell is past slot 99
/// (the last page only has four real slots).
pub fn slot_index(page: usize, cell: usize) -> Option<usize> {
    let i = page.checked_mul(PER_PAGE)?.checked_add(cell)?;
    (i < SLOT_COUNT).then_some(i)
}

/// How many of this page's 12 cells are real save slots.
pub fn cells_on_page(page: usize) -> usize {
    let start = page.saturating_mul(PER_PAGE);
    SLOT_COUNT.saturating_sub(start).min(PER_PAGE)
}

pub struct SaveState {
    /// Script id (`1002`), i.e. the `.bnr`/`.txt` pair to reload.
    pub scene: String,
    pub vm: InterpState,
    /// Last `BgScene`; `None` means the black screen.
    pub bg: Option<(i64, u32)>,
    /// Raw ACT146/147 K30 mode used to restore an f0NNN effect plate.
    pub effect_mode: i32,
    /// ACT170 ambient preset (0..=3), needed to resolve later background
    /// override names after loading a save.
    pub ambient_preset: i32,
    /// Name of the active `.AMP` colour curve.
    pub tint: Option<String>,
    /// One entry per stage slot: `(char_id, file name)`.
    pub slots: Vec<Option<(u32, String)>>,
    pub focus: Option<u32>,
    /// The line being shown when the save was made.
    pub text: String,
    /// BGM track id, restarted on load.
    pub bgm: Option<i32>,
    /// Wall clock time of the save, for the slot caption.
    pub time: String,
    /// Story-wide values declared by `Global.vrb`. The schema is loaded from
    /// the current data tree, so the save only needs indexed integers.
    pub globals: Vec<i32>,
}

/// Where the slots live: `<data root>/save`.
pub fn dir() -> PathBuf {
    match crate::data::Data::resolve() {
        Some(d) => d.root.join("save"),
        None => PathBuf::from("data/save"),
    }
}

pub fn slot_path(i: usize) -> PathBuf {
    dir().join(format!("save_{:02}.sav", i))
}

/// Quick save, the original's `save_Q.sav` (separate from the 100 numbered slots).
pub fn quick_path() -> PathBuf {
    dir().join("save_Q.sav")
}

pub fn thumb_path(i: usize) -> PathBuf {
    dir().join(format!("save_{:02}.tga", i))
}

// ---------------------------------------------------------------------------
// text coding
// ---------------------------------------------------------------------------

/// `\` and newlines are the only characters that cannot appear literally in a
/// `key=value` line.
fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\n', "\\n").replace('\r', "")
}

fn unesc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('\\') => out.push('\\'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

const SEP: char = '\u{1f}';

fn num<T: std::str::FromStr>(m: &HashMap<String, String>, k: &str) -> Option<T> {
    m.get(k).and_then(|v| v.trim().parse::<T>().ok())
}

fn nums<T: std::str::FromStr>(m: &HashMap<String, String>, k: &str) -> Vec<T> {
    m.get(k)
        .map(|v| {
            v.split(',')
                .filter(|p| !p.trim().is_empty())
                .filter_map(|p| p.trim().parse::<T>().ok())
                .collect()
        })
        .unwrap_or_default()
}

fn opt_num<T: std::str::FromStr>(m: &HashMap<String, String>, k: &str) -> Option<T> {
    m.get(k).and_then(|v| v.trim().parse::<T>().ok())
}

impl SaveState {
    pub fn save_to(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let mut out = String::from("; WHITE ALBUM 2 native port - save slot\n");
        let mut line = |k: &str, v: String| out.push_str(&format!("{}={}\n", k, esc(&v)));
        line("version", VERSION.to_string());
        line("time", self.time.clone());
        line("scene", self.scene.clone());
        let vm = &self.vm;
        line("pc", vm.pc.to_string());
        line("tok", vm.tok.map(|t| t.to_string()).unwrap_or_default());
        line("ints", join_nums(&vm.ints));
        line("flts", join_nums(&vm.flts));
        line("speaker", vm.speaker.as_ref().map(|s| s.0.clone()).unwrap_or_default());
        line("speaker_id", vm.speaker.as_ref().map(|s| s.1.to_string()).unwrap_or_default());
        line(
            "voice",
            vm.pending_voice
                .map(|c| format!("{},{},{}", c.scenario, c.line, c.char_id))
                .unwrap_or_default(),
        );
        line("voice_scenario", vm.voice_scenario.to_string());
        line("options", esc_list(&vm.options));
        line("choice_result", vm.choice_result.to_string());
        line("pending_b", vm.pending_b.map(|b| b.to_string()).unwrap_or_default());
        line("last_kind", vm.last_kind.clone());
        line("steps", vm.steps.to_string());
        line("rec_kind", vm.rec_kind.to_string());
        line("rec_tok_hit", flag(vm.rec_tok_hit));
        line("rec_act_hit", flag(vm.rec_act_hit));
        line("rec_toks", join_nums(&vm.rec_toks));
        line(
            "pending_sprite",
            vm.pending_sprite
                .map(|(a, b, c)| format!("{},{},{}", a, b, c))
                .unwrap_or_default(),
        );
        line("bg_id", self.bg.map(|b| b.0.to_string()).unwrap_or_default());
        line("bg_variant", self.bg.map(|b| b.1.to_string()).unwrap_or_default());
        line("effect_mode", self.effect_mode.to_string());
        line("ambient_preset", self.ambient_preset.to_string());
        line("tint", self.tint.clone().unwrap_or_default());
        for (i, s) in self.slots.iter().enumerate() {
            line(
                &format!("slot{}", i),
                s.as_ref().map(|(id, name)| format!("{},{}", id, name)).unwrap_or_default(),
            );
        }
        line("focus", self.focus.map(|f| f.to_string()).unwrap_or_default());
        line("bgm", self.bgm.map(|b| b.to_string()).unwrap_or_default());
        line("text", self.text.clone());
        line("global_values", join_nums(&self.globals));
        // Retain the old key for third-party slot browsers; it is ignored by
        // the native loader and no longer carries story state.
        line("flags", String::new());
        let parent = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        std::fs::create_dir_all(parent)?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp = parent.join(format!(
            ".{}.tmp.{}.{}",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("save"),
            std::process::id(),
            stamp
        ));
        if let Err(e) = std::fs::write(&tmp, out).and_then(|_| std::fs::rename(&tmp, path)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        Ok(())
    }

    pub fn load_from(path: &std::path::Path) -> Option<SaveState> {
        let text = std::fs::read_to_string(path).ok()?;
        let mut m = HashMap::new();
        for l in text.lines() {
            let l = l.trim();
            if l.is_empty() || l.starts_with(';') || l.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = l.split_once('=') {
                m.insert(k.trim().to_string(), unesc(v));
            }
        }
        let version = num::<u32>(&m, "version").unwrap_or(0);
        // Version 1 was the pre-global native format. It remains readable and
        // starts with schema defaults; original-game saves were never
        // supported and are intentionally outside this migration path.
        if version != 1 && version != VERSION {
            eprintln!("save: {} has an unsupported version", path.display());
            return None;
        }
        let speaker = match m.get("speaker").map(|s| s.as_str()).filter(|s| !s.is_empty()) {
            Some(name) => Some((name.to_string(), opt_num::<u32>(&m, "speaker_id").unwrap_or(0))),
            None => None,
        };
        let voice = m
            .get("voice")
            .filter(|v| !v.is_empty())
            .and_then(|v| {
                let p: Vec<u32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                if p.len() == 3 {
                    Some(VoiceCue { scenario: p[0], line: p[1], char_id: p[2] })
                } else {
                    None
                }
            });
        let sprite = m
            .get("pending_sprite")
            .filter(|v| !v.is_empty())
            .and_then(|v| {
                let p: Vec<u32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                if p.len() == 3 { Some((p[0], p[1], p[2])) } else { None }
            });
        let bg = match num::<i64>(&m, "bg_id") {
            Some(id) => Some((id, num::<u32>(&m, "bg_variant").unwrap_or(0))),
            None => None,
        };
        let slots = (0..crate::SPRITE_SLOTS)
            .map(|i| {
                m.get(&format!("slot{}", i))
                    .filter(|v| !v.is_empty())
                    .and_then(|v| {
                        let (id, name) = v.split_once(',')?;
                        Some((id.trim().parse::<u32>().ok()?, name.trim().to_string()))
                    })
            })
            .collect();
        Some(SaveState {
            scene: m.get("scene").cloned().unwrap_or_default(),
            vm: InterpState {
                pc: num(&m, "pc").unwrap_or(0),
                tok: num(&m, "tok"),
                ints: nums(&m, "ints"),
                flts: nums(&m, "flts"),
                speaker,
                pending_voice: voice,
                voice_scenario: num(&m, "voice_scenario").unwrap_or(0),
                options: m
                    .get("options")
                    .filter(|v| !v.is_empty())
                    .map(|v| v.split(SEP).map(|s| s.to_string()).collect())
                    .unwrap_or_default(),
                choice_result: num(&m, "choice_result").unwrap_or(1),
                pending_b: num(&m, "pending_b"),
                last_kind: m.get("last_kind").cloned().unwrap_or_default(),
                steps: num(&m, "steps").unwrap_or(0),
                rec_kind: num(&m, "rec_kind").unwrap_or(0),
                rec_tok_hit: num::<u32>(&m, "rec_tok_hit").unwrap_or(0) != 0,
                rec_act_hit: num::<u32>(&m, "rec_act_hit").unwrap_or(0) != 0,
                rec_toks: nums(&m, "rec_toks"),
                pending_sprite: sprite,
                rec_var: None,
                rec_fun: None,
                condition_result: None,
                choice_pc: None,
            },
            bg,
            effect_mode: num(&m, "effect_mode").unwrap_or(0),
            ambient_preset: num(&m, "ambient_preset").unwrap_or(0),
            tint: m.get("tint").filter(|v| !v.is_empty()).cloned(),
            slots,
            focus: num(&m, "focus"),
            text: m.get("text").cloned().unwrap_or_default(),
            bgm: num(&m, "bgm"),
            time: m.get("time").cloned().unwrap_or_default(),
            globals: if version >= 2 { nums(&m, "global_values") } else { Vec::new() },
        })
    }

    /// Caption shown on the save screen: the last spoken line, first sentence.
    pub fn caption(&self) -> String {
        let t = self.text.replace('\n', " ");
        let t = t.trim();
        if t.is_empty() {
            format!("场景 {}", self.scene)
        } else {
            t.chars().take(24).collect()
        }
    }
}

fn flag(b: bool) -> String {
    if b { String::from("1") } else { String::from("0") }
}

fn join_nums<T: std::fmt::Display>(v: &[T]) -> String {
    v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",")
}

fn esc_list(v: &[String]) -> String {
    v.iter().map(|s| esc(s)).collect::<Vec<_>>().join(&SEP.to_string())
}

/// Wall clock time for a slot caption, as `YYYY-MM-DD HH:MM`.
///
/// `date` is asked first because the epoch alone only gives UTC, and a slot
/// caption showing the wrong hour is worse than shelling out once per save.
pub fn now() -> String {
    if let Ok(out) = std::process::Command::new("date").arg("+%Y-%m-%d %H:%M").output() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.len() >= 16 {
            return s;
        }
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        y,
        m,
        d,
        (secs / 3600) % 24,
        (secs / 60) % 60
    )
}

/// Days since 1970-01-01 -> (year, month, day). Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as i64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Every slot that holds a save, oldest index first.
pub fn list() -> Vec<Option<SaveState>> {
    (0..SLOT_COUNT).map(|i| SaveState::load_from(&slot_path(i))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::lscr::InterpState;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn last_four_slots_live_on_a_ninth_page() {
        assert_eq!(page_count(), 9);
        assert_eq!(slot_index(0, 0), Some(0));
        assert_eq!(slot_index(7, 11), Some(95));
        assert_eq!(slot_index(8, 0), Some(96));
        assert_eq!(slot_index(8, 3), Some(99));
        assert_eq!(slot_index(8, 4), None);
        assert_eq!(cells_on_page(7), 12);
        assert_eq!(cells_on_page(8), 4);
        // Truncating 100/12 would hide save_96..save_99.
        assert_ne!(SLOT_COUNT / PER_PAGE, page_count());
        assert!(slot_index(SLOT_COUNT / PER_PAGE, 0).is_some());
    }

    #[test]
    fn version_two_round_trip_preserves_global_values_atomically() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("wa2-save-{}-{}", std::process::id(), stamp));
        let path = dir.join("save_00.sav");
        let st = SaveState {
            scene: "2003".into(),
            vm: InterpState {
                pc: 4,
                tok: None,
                ints: vec![],
                flts: vec![],
                speaker: None,
                pending_voice: None,
                voice_scenario: 0,
                options: vec![],
                choice_result: 1,
                pending_b: None,
                last_kind: String::new(),
                steps: 0,
                rec_kind: 0,
                rec_tok_hit: false,
                rec_act_hit: false,
                rec_toks: vec![],
                pending_sprite: None,
                rec_var: Some(14),
                rec_fun: None,
                condition_result: None,
                choice_pc: None,
            },
            bg: None,
            effect_mode: 0,
            ambient_preset: 0,
            tint: None,
            slots: vec![None; crate::SPRITE_SLOTS],
            focus: None,
            text: "line".into(),
            bgm: None,
            time: "2026-10-21 00:00".into(),
            globals: vec![1, 2, 3, 4, 5],
        };
        st.save_to(&path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.starts_with("; WHITE ALBUM 2 native port - save slot\n"));
        assert!(raw.contains("version=2\n"));
        assert_eq!(SaveState::load_from(&path).unwrap().globals, st.globals);
        assert!(!dir.join(".save_00.sav.tmp").exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
