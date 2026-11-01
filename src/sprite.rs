//! The sprite bank: which portrait is on screen and where it sits.

use crate::*;

/// Standing-character stage slots. K16's first value is the slot: 0/1/2 are
/// the usual left/centre/right, 3/4 stand further out, and 7–10 are the back
/// row used in group shots. Clamping everything above 2 onto the right slot
/// stacked people who should have been standing apart.
pub(crate) const SPRITE_SLOTS: usize = 11;

/// Horizontal centre of each slot, in 1280-wide screen coordinates.
pub(crate) const SPRITE_ANCHORS: [i32; SPRITE_SLOTS] = [
    320, 640, 960, // 0 left, 1 centre, 2 right
    140, 1140, // 3 far left, 4 far right
    640, 640, // 5, 6 (unused by the scripts)
    220, 500, 780, 1060, // 7–10 back row
];

/// Paint the back row and the outer pair before the front three, so a group
/// shot doesn't hide the people standing behind.
pub(crate) const SPRITE_DRAW_ORDER: [usize; SPRITE_SLOTS] = [7, 8, 9, 10, 3, 4, 5, 6, 0, 1, 2];

/// Locate a directory of loose character sprites (legacy layout helper).
///
/// The data tree passes its own sprite directory explicitly; this is only used
/// by the explicit-path CLI form, where it tries the common layouts.
pub(crate) fn find_sprite_dir(bg_dir: &std::path::Path, script_dir: &std::path::Path) -> Option<std::path::PathBuf> {
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
pub(crate) fn sprite_prefixes(cid: u32) -> &'static [&'static str] {
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
pub(crate) struct SpriteBank {
    /// Scaled sprite trees, ascending; the decode scale picks the active one.
    pub(crate) trees: Vec<(u32, std::path::PathBuf)>,
    pub(crate) dir: std::path::PathBuf,
    /// Scale of the pixels in `dir`; 0 before the first retarget.
    pub(crate) scale: u32,
    /// lowercased file names present in the directory
    pub(crate) index: std::collections::HashSet<String>,
    /// file name -> (decoded pixels, scale of those pixels, scale of the
    /// source tree they were read from)
    pub(crate) cache: std::collections::HashMap<String, (std::sync::Arc<codec::tga::TgaImage>, u32, u32)>,
    /// High-resolution sprites are large after WebP decode. Do not retain the
    /// entire 1,600-file library in RAM as the story advances.
    pub(crate) cache_limit: usize,
}

impl SpriteBank {
    pub(crate) fn new(trees: Vec<(u32, std::path::PathBuf)>) -> SpriteBank {
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
    pub(crate) fn retarget(&mut self, want: u32) {
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
    pub(crate) fn get(&mut self, name: &str) -> Option<(std::sync::Arc<codec::tga::TgaImage>, u32, u32)> {
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
    pub(crate) fn resolve(&self, cid: u32, series: u32, variant: u32) -> Option<String> {
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

pub(crate) struct ActiveSprite {
    pub(crate) char_id: u32,
    #[allow(dead_code)]
    pub(crate) name: String,
    pub(crate) img: std::sync::Arc<codec::tga::TgaImage>,
    pub(crate) art_scale: u32,
    /// Scale of the sprite tree the file came from (4x or 8x), for the log.
    pub(crate) src_scale: u32,
}
