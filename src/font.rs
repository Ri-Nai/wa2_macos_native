//! Font loading.

use crate::*;

/// Load the font for a data tree: atlas + authoritative cell map.
///
/// The cell map (`wa2_cell_map_{jp,cn}.tsv`) is the game exe's own font table
/// expressed as atlas cells, so glyph lookup is exact for both the Japanese and
/// the Chinese atlas - no heuristics, no codec ambiguity.
pub(crate) fn load_font(d: &data::Data) -> Option<render::Font> {
    load_font_paths(&d.atlas(), Some(&d.cell_map()), None)
}

/// Load a font atlas with either an authoritative cell map, a legacy charset
/// table, or both (the cell map wins).
pub(crate) fn load_font_paths(
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
pub(crate) fn load_font_charset(atlas_path: &str, charset_path: &str) -> Option<render::Font> {
    load_font_paths(
        std::path::Path::new(atlas_path),
        None,
        Some(std::path::Path::new(charset_path)),
    )
}
