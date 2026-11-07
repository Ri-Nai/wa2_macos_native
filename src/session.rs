//! State that outlives a single scene: the art index, the prefetch queue and the play session.

use crate::*;

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
pub(crate) struct BgIndex {
    pub(crate) by_name: std::collections::HashMap<String, String>,
    /// Background id -> its `b*` files, sorted lowest variant first.
    pub(crate) bgs: std::collections::HashMap<u32, Vec<(u32, String)>>,
    /// Background id -> `v*` event-CG stand-ins, used only when `bgs` has no
    /// entry for the id.
    pub(crate) cgs: std::collections::HashMap<u32, Vec<(u32, String)>>,
}

impl BgIndex {
    pub(crate) fn build(dirs: &[&std::path::Path]) -> BgIndex {
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
    pub(crate) fn resolve(&self, id: i64, variant: u32) -> Option<String> {
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
pub(crate) fn upcoming_art(
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
                // A scroll CG whose payload sits in the preceding K30 record
                // (`mode, cg_id, variant, frames, 0, 0, h, n`), picked up by
                // the ACT227 that follows. The interpreter raises it in
                // `finish_record`; the prefetch scanner must recognise the same
                // shape or the tall picture is decoded synchronously on the
                // main thread while the screen sits on the black cut before it.
                if *k == 30 && !tok_hit && !act_hit && ints.len() >= 6 {
                    let mode = ints[0];
                    let cg = ints[1];
                    if (0..=5).contains(&mode)
                        && cg >= 1000
                        && p.ops[i..]
                            .iter()
                            .take(16)
                            .any(|o| matches!(o, Op::Act { id, .. } if *id == 227))
                    {
                        let n = event_cg_name(cg as i64, ints[2].max(0) as u32);
                        if !bgs.contains(&n) {
                            bgs.push(n);
                        }
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
                    227 => {
                        // Scroll/effects CG. The inline form carries the
                        // picture in its own record: `(mode, cg_id, variant,
                        // frames, ...)` with `cg_id >= 1000`. These are several
                        // screens tall and the slowest decode in the tree, so
                        // they must be prefetched like every other CG.
                        if ints.len() >= 4 && ints[1] >= 1000 {
                            let n = event_cg_name(ints[1] as i64, ints[2].max(0) as u32);
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
pub(crate) const PREFETCH_BG: usize = 6;

pub(crate) const PREFETCH_SPRITE: usize = 8;

/// Turn the names the script will ask for into file paths for the prefetch
/// thread. Missing files are dropped here rather than in the worker.
pub(crate) fn prefetch_paths(
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
pub(crate) struct Session {
    pub(crate) script_dir: std::path::PathBuf,
    /// True when `script_dir` is the CK-GAL Chinese patch: its text needs the
    /// glyph-table reading (see `codec::sjis::decode_cn`).
    pub(crate) cn: bool,
    /// Scaled art trees, `(scale, dir)` ascending. Both the 4x tree and the
    /// 8x master are kept when present: the decode scale decides which one is
    /// read first, so entering fullscreen switches to the 8x master without
    /// rebuilding the session.
    pub(crate) bg_trees: Vec<(u32, std::path::PathBuf)>,
    /// Chinese-patch UI overrides - the 31 images shipped loose in the game's
    /// `grp/` directory, which win over the same-named files inside `grp.pak`.
    /// Consulted before `bg_trees`; empty in Japanese mode.
    pub(crate) bg_cn_trees: Vec<(u32, std::path::PathBuf)>,
    /// Original Chinese UI tree used as a per-file fallback while the
    /// optional high-resolution sibling is still being generated.
    pub(crate) bg_cn_original: Option<std::path::PathBuf>,
    /// Original background tree used when the selected high-resolution file
    /// is not present yet.
    pub(crate) bg_original: Option<std::path::PathBuf>,
    /// Scaled sprite trees, ascending; empty disables sprites.
    pub(crate) sprite_trees: Vec<(u32, std::path::PathBuf)>,
    /// Movies as plain `*.wmv` / `*.asf` files.
    pub(crate) movie_dir: std::path::PathBuf,
    pub(crate) voice_dir: Option<std::path::PathBuf>,
    pub(crate) bgm: Option<String>,
    /// ACT146/147 background id -> file index.
    pub(crate) bg_index: BgIndex,
}

impl Session {
    pub(crate) fn from_data(d: &data::Data, bgm: Option<String>) -> Session {
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
    pub(crate) fn bg_dirs(&self) -> Vec<&std::path::Path> {
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
    pub(crate) fn sprite_dir_now(&self) -> Option<&std::path::Path> {
        ordered_tree_paths(&self.sprite_trees, render_art_scale())
            .into_iter()
            .next()
    }
}

/// Order scaled trees for a decode target: exact match first, then the next
/// larger, then the smaller ones.
pub(crate) fn ordered_tree_paths<'a>(
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
