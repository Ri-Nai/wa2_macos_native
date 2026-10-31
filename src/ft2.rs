//! Minimal FreeType binding via runtime dynamic loading (dlopen/dlsym).
//!
//! Same approach as sdl2.rs: no crates, no build-time dependency, and a
//! graceful None when the library is absent. FreeType is used for one thing -
//! turning a character of a font *file* into an 8-bit coverage mask plus its
//! metrics - so text can be drawn at the window's real pixel size instead of
//! being stretched up from the 1280x720 layer.
//!
//! Several struct fields are read straight out of FreeType's structs. Their
//! offsets are the LP64 ones for freetype 2.x, verified with `offsetof` against
//! /usr/include/freetype2/freetype/freetype.h on this machine:
//!
//!   * FT_FaceRec.num_faces      @0
//!   * FT_FaceRec.face_index     @8
//!   * FT_FaceRec.family_name    @40   (char*)
//!   * FT_FaceRec.style_name     @48   (char*)
//!   * FT_FaceRec.units_per_EM   @136
//!   * FT_FaceRec.ascender       @138  (16-bit, font units)
//!   * FT_FaceRec.descender      @140
//!   * FT_FaceRec.height         @142
//!   * FT_FaceRec.glyph          @152  (the glyph slot)
//!   * FT_FaceRec.size           @160  (FT_Size)
//!   * FT_SizeRec.metrics.ascender  @48  (26.6, already scaled to the size)
//!   * FT_SizeRec.metrics.descender @56
//!   * FT_SizeRec.metrics.height    @64
//!   * FT_GlyphSlotRec.face         @8
//!   * FT_GlyphSlotRec.format       @144
//!   * FT_GlyphSlotRec.metrics.horiAdvance @80 (26.6 fixed point)
//!   * FT_GlyphSlotRec.bitmap       @152, an FT_Bitmap of 40 bytes:
//!       rows @0, width @4, pitch @8, buffer @16, pixel_mode @26
//!   * FT_GlyphSlotRec.bitmap_left @192, bitmap_top @196
//!
//! Rather than trusting that silently, Ft::open renders one probe glyph and
//! checks that the slot points back at the face we opened and that FreeType
//! reports a bitmap. A wrong offset makes the whole font unusable instead of
//! producing garbage pixels.

use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_long, c_uchar, c_uint, c_ulong, c_void};

extern "C" {
    fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

const RTLD_NOW: c_int = 2;

/// FT_LOAD_RENDER: load the glyph and render it into the slot's bitmap.
const FT_LOAD_RENDER: c_int = 0x0000_0004;
/// FT_LOAD_NO_BITMAP: ignore embedded bitmap strikes and render the outline.
///
/// Several CJK faces (the arphic uming/ukai families, for one) ship monochrome
/// strikes at some sizes. FreeType would answer with a 1-bit bitmap there,
/// which this module's coverage pipeline cannot use - `Ft::open` would report
/// the face as unusable and `glyph` would report the character as missing, so a
/// whole size band would drop out (or fall into the per-glyph fallback with a
/// different face). The outline rasterizer is the one path this engine measures
/// and draws with, so the flag is on everywhere.
const FT_LOAD_NO_BITMAP: c_int = 0x0000_0008;
/// FT_LOAD_TARGET_LIGHT: hint horizontally only, and lightly. The baseline
/// for CJK dialog text: MONO would quantize the strokes, LCD sub-pixel
/// rendering would leave coloured fringes once the frame is scaled and
/// composited. The other load flags live next to the hint modes that pick
/// them (see text::HintMode).
const FT_LOAD_TARGET_LIGHT: c_int = 0x0001_0000;
/// FT_RENDER_MODE_NORMAL: 8-bit anti-aliased coverage.
const FT_RENDER_MODE_NORMAL: c_int = 0;
/// FT_ENCODING_UNICODE, as FT_IMAGE_TAG('u','n','i','c').
const FT_ENCODING_UNICODE: c_int = 0x756E_6963u32 as c_int;
/// FT_GLYPH_FORMAT_BITMAP, as FT_IMAGE_TAG('b','i','t','s').
const FT_GLYPH_FORMAT_BITMAP: u32 = 0x6269_7473;
/// FT_PIXEL_MODE_GRAY: buffer holds one coverage byte per pixel.
const FT_PIXEL_MODE_GRAY: u8 = 2;
/// FT_KERNING_DEFAULT: kerning scaled to the current size, in 26.6 pixels.
const FT_KERNING_DEFAULT: c_uint = 0;

// Struct offsets, see the module comment. The face-level units_per_EM,
// ascender, descender and height (@136/@138/@140/@142) are deliberately not
// read: the size object carries the same metrics already scaled to pixels.
const FACE_NUM_FACES: usize = 0;
const FACE_FACE_INDEX: usize = 8;
const FACE_FAMILY_NAME: usize = 40;
const FACE_STYLE_NAME: usize = 48;
const FACE_GLYPH: usize = 152;
const FACE_SIZE: usize = 160;
const SLOT_FACE: usize = 8;
const SLOT_FORMAT: usize = 144;
const SLOT_HORI_ADVANCE: usize = 80;
const SLOT_BITMAP: usize = 152;
const SLOT_BITMAP_LEFT: usize = 192;
const SLOT_BITMAP_TOP: usize = 196;
// Inside FT_Bitmap (40 bytes):
const BM_ROWS: usize = 0;
const BM_WIDTH: usize = 4;
const BM_PITCH: usize = 8;
const BM_BUFFER: usize = 16;
const BM_PIXEL_MODE: usize = 26;
// Inside FT_SizeRec, relative to the size pointer:
const SIZE_METRICS_ASCENDER: usize = 48;
const SIZE_METRICS_DESCENDER: usize = 56;
const SIZE_METRICS_HEIGHT: usize = 64;

type FtError = c_int;

struct Fns {
    new_face: unsafe extern "C" fn(*mut c_void, *const c_char, c_long, *mut *mut c_void) -> FtError,
    set_pixel_sizes: unsafe extern "C" fn(*mut c_void, c_uint, c_uint) -> FtError,
    select_charmap: unsafe extern "C" fn(*mut c_void, c_int) -> FtError,
    get_char_index: unsafe extern "C" fn(*mut c_void, c_ulong) -> c_uint,
    get_kerning: unsafe extern "C" fn(*mut c_void, c_uint, c_uint, c_uint, *mut [i64; 2]) -> FtError,
    load_glyph: unsafe extern "C" fn(*mut c_void, c_uint, c_int) -> FtError,
    render_glyph: unsafe extern "C" fn(*mut c_void, c_int) -> FtError,
    done_face: unsafe extern "C" fn(*mut c_void) -> FtError,
    done_library: unsafe extern "C" fn(*mut c_void) -> FtError,
}

/// One rasterized glyph: a coverage mask and where it sits relative to the pen.
pub struct Glyph {
    /// Mask size in pixels.
    pub w: u32,
    pub h: u32,
    /// Coverage, w * h bytes, row-major. 0 is transparent, 255 solid.
    pub mask: Vec<u8>,
    /// Horizontal offset from the pen position to the left edge of the mask.
    pub left: i32,
    /// Vertical offset from the baseline *up* to the top edge of the mask.
    pub top: i32,
    /// How far the pen moves right, in FreeType's 26.6 fixed point at the
    /// current pixel size. Layout accumulates these and rounds only when it
    /// stamps a glyph, so a long mixed CJK/Latin line does not drift a pixel
    /// per character.
    pub advance_266: i32,
}

/// Vertical metrics of the face at the current pixel size.
///
/// Read from `size->metrics`, which FreeType has already scaled - unlike the
/// face-level fields, these are in pixels, not font units. `descender` is
/// negative, matching FreeType's convention.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Metrics {
    /// Pixels above the baseline.
    pub ascender: i32,
    /// Pixels below the baseline (<= 0).
    pub descender: i32,
    /// Suggested line height: ascender - descender plus the line gap.
    pub height: i32,
}

/// An open font file at one pixel size.
pub struct Ft {
    library: *mut c_void,
    face: *mut c_void,
    size: u32,
    /// FT_Load_Glyph flags in use; see the FT_LOAD_* constants.
    load_flags: c_int,
    fns: Fns,
}

unsafe fn sym(lib: *mut c_void, name: &str) -> Option<*mut c_void> {
    let c = CString::new(name).ok()?;
    let p = dlsym(lib, c.as_ptr());
    if p.is_null() { None } else { Some(p) }
}

impl Ft {
    /// Open `path` (a .ttf / .otf / .ttc) at `size` pixels, on face
    /// `face_index` of the file. A .ttc holds one face per language/weight, so
    /// the index is what picks Simplified Chinese (2) over Japanese (0).
    /// None when FreeType, the file, the face index, or a struct offset does
    /// not line up.
    pub fn open(path: &str, size: u32, face_index: i64) -> Option<Ft> {
        if size == 0 {
            return None;
        }
        unsafe {
            let mut lib = std::ptr::null_mut();
            // Same runtime-loading story as SDL2: nothing links against
            // FreeType, so macOS needs the Homebrew prefix spelled out.
            let candidates: &[&str] = if cfg!(target_os = "macos") {
                &[
                    "/opt/homebrew/lib/libfreetype.6.dylib",
                    "/usr/local/lib/libfreetype.6.dylib",
                    "/usr/lib/libfreetype.6.dylib",
                    "libfreetype.6.dylib",
                    "libfreetype.dylib",
                ]
            } else {
                &["libfreetype.so.6", "libfreetype.so"]
            };
            for cand in candidates {
                let c = CString::new(*cand).unwrap();
                lib = dlopen(c.as_ptr(), RTLD_NOW);
                if !lib.is_null() {
                    break;
                }
            }
            if lib.is_null() {
                return None;
            }
            let init: unsafe extern "C" fn(*mut *mut c_void) -> FtError =
                std::mem::transmute(sym(lib, "FT_Init_FreeType")?);
            let fns = Fns {
                done_library: std::mem::transmute(sym(lib, "FT_Done_FreeType")?),
                new_face: std::mem::transmute(sym(lib, "FT_New_Face")?),
                set_pixel_sizes: std::mem::transmute(sym(lib, "FT_Set_Pixel_Sizes")?),
                select_charmap: std::mem::transmute(sym(lib, "FT_Select_Charmap")?),
                get_char_index: std::mem::transmute(sym(lib, "FT_Get_Char_Index")?),
                get_kerning: std::mem::transmute(sym(lib, "FT_Get_Kerning")?),
                load_glyph: std::mem::transmute(sym(lib, "FT_Load_Glyph")?),
                render_glyph: std::mem::transmute(sym(lib, "FT_Render_Glyph")?),
                done_face: std::mem::transmute(sym(lib, "FT_Done_Face")?),
            };
            let mut library: *mut c_void = std::ptr::null_mut();
            if init(&mut library) != 0 {
                return None;
            }
            let cpath = CString::new(path).ok()?;
            let mut face: *mut c_void = std::ptr::null_mut();
            if (fns.new_face)(library, cpath.as_ptr(), face_index as c_long, &mut face) != 0
                || face.is_null()
            {
                let _ = (fns.done_library)(library);
                return None;
            }
            let mut ft = Ft {
                library,
                face,
                size: 0,
                load_flags: FT_LOAD_RENDER | FT_LOAD_NO_BITMAP | FT_LOAD_TARGET_LIGHT,
                fns,
            };
            // A .ttc can open on a non-Unicode charmap; pick it explicitly.
            let _ = (ft.fns.select_charmap)(face, FT_ENCODING_UNICODE);
            if ft.set_size(size).is_none() || !ft.offsets_line_up() {
                return None;
            }
            // The dlopen handle stays open for the life of the process: the
            // function pointers kept in Fns would dangle otherwise.
            Some(ft)
        }
    }

    /// Rasterize at a new pixel size.
    pub fn set_size(&mut self, size: u32) -> Option<()> {
        if size == 0 {
            return None;
        }
        let r = unsafe { (self.fns.set_pixel_sizes)(self.face, 0, size as c_uint) };
        if r != 0 {
            return None;
        }
        self.size = size;
        Some(())
    }

    /// Swap the FT_Load_Glyph flags (hinting / render target). Returns false
    /// when the flags are not usable, leaving the old ones in place.
    pub fn set_load_flags(&mut self, flags: c_int) -> bool {
        if flags & FT_LOAD_RENDER == 0 {
            // Everything downstream reads the slot's bitmap.
            return false;
        }
        self.load_flags = flags;
        true
    }

    /// Vertical metrics in pixels at the current size, straight from the size
    /// object FreeType has already scaled.
    pub fn metrics(&self) -> Option<Metrics> {
        unsafe {
            let size = self.read_ptr(self.face, FACE_SIZE);
            if size.is_null() {
                return None;
            }
            let asc = self.fixed(size, SIZE_METRICS_ASCENDER);
            let desc = self.fixed(size, SIZE_METRICS_DESCENDER);
            let h = self.fixed(size, SIZE_METRICS_HEIGHT);
            Some(Metrics { ascender: asc, descender: desc, height: h })
        }
    }

    /// Family name of the open face, e.g. "Noto Sans CJK SC". A .ttc reports
    /// one per face, which is how the engine proves it picked the right one.
    pub fn family(&self) -> Option<String> {
        unsafe { self.cstr(self.face, FACE_FAMILY_NAME) }
    }

    /// Style name of the open face, e.g. "Medium".
    pub fn style(&self) -> Option<String> {
        unsafe { self.cstr(self.face, FACE_STYLE_NAME) }
    }

    /// Index of the open face inside its file.
    pub fn face_index(&self) -> i64 {
        unsafe { self.read_i64(self.face, FACE_FACE_INDEX) }
    }

    /// How many faces the file holds.
    pub fn num_faces(&self) -> i64 {
        unsafe { self.read_i64(self.face, FACE_NUM_FACES) }
    }

    /// Kerning between two adjacent characters, in 26.6 fixed point at the
    /// current pixel size. Zero unless the face carries a kern table for the
    /// pair, or on error.
    pub fn kerning_266(&mut self, left: char, right: char) -> i32 {
        unsafe {
            let l = (self.fns.get_char_index)(self.face, left as u32 as c_ulong);
            let r = (self.fns.get_char_index)(self.face, right as u32 as c_ulong);
            if l == 0 || r == 0 {
                return 0;
            }
            let mut delta = [0i64; 2];
            if (self.fns.get_kerning)(self.face, l, r, FT_KERNING_DEFAULT, &mut delta) != 0 {
                return 0;
            }
            // Already scaled to the pixel size; keep it in 26.6 for the
            // fixed-point pen to accumulate without rounding.
            delta[0] as i32
        }
    }

    /// Render one character. None when the font has no glyph for it, or the
    /// bitmap comes back in a pixel format we do not handle.
    pub fn glyph(&mut self, ch: char) -> Option<Glyph> {
        let slot = self.slot()?;
        unsafe {
            let index = (self.fns.get_char_index)(self.face, ch as u32 as c_ulong);
            // Index 0 is .notdef: loading it succeeds and hands back the face's
            // "no glyph here" box, which is exactly the tofu the caller has to
            // decide about. Report the absence instead of the picture.
            if index == 0 {
                return None;
            }
            if (self.fns.load_glyph)(self.face, index, self.load_flags) != 0 {
                return None;
            }
            // FT_LOAD_RENDER already renders; this keeps the mode explicit if
            // the load flags ever change.
            let _ = (self.fns.render_glyph)(slot, FT_RENDER_MODE_NORMAL);
            let rows = self.read_u32(slot, SLOT_BITMAP + BM_ROWS) as usize;
            let width = self.read_u32(slot, SLOT_BITMAP + BM_WIDTH) as usize;
            if self.read_u8(slot, SLOT_BITMAP + BM_PIXEL_MODE) != FT_PIXEL_MODE_GRAY {
                return None;
            }
            let left = self.read_i32(slot, SLOT_BITMAP_LEFT);
            let top = self.read_i32(slot, SLOT_BITMAP_TOP);
            let advance_266 = self.read_i64(slot, SLOT_HORI_ADVANCE) as i32;
            let mut mask = vec![0u8; width * rows];
            if rows > 0 && width > 0 {
                let buf = self.read_ptr(slot, SLOT_BITMAP + BM_BUFFER) as *const c_uchar;
                if buf.is_null() {
                    return None;
                }
                let pitch = self.read_i32(slot, SLOT_BITMAP + BM_PITCH);
                for y in 0..rows {
                    // `pitch` is signed: FreeType pads rows downwards too.
                    let row = if pitch >= 0 { y * pitch as usize } else { (rows - 1 - y) * (-pitch) as usize };
                    let src = buf.add(row);
                    let dst = y * width;
                    for x in 0..width {
                        mask[dst + x] = *src.add(x);
                    }
                }
            }
            Some(Glyph {
                w: width as u32,
                h: rows as u32,
                mask,
                left,
                top,
                advance_266,
            })
        }
    }

    fn slot(&self) -> Option<*mut c_void> {
        let p = unsafe { self.read_ptr(self.face, FACE_GLYPH) };
        if p.is_null() { None } else { Some(p) }
    }

    /// Render a probe glyph and confirm the struct offsets we assumed.
    fn offsets_line_up(&mut self) -> bool {
        let Some(slot) = self.slot() else { return false };
        if unsafe { self.read_ptr(slot, SLOT_FACE) } != self.face {
            return false;
        }
        let g = match self.glyph('A') {
            Some(g) => g,
            None => return false,
        };
        // The slot must say it holds a bitmap now, and a glyph must advance.
        let format = unsafe { self.read_u32(slot, SLOT_FORMAT) };
        format == FT_GLYPH_FORMAT_BITMAP && g.advance_266 > 0
    }

    /// Read a field at byte offset `off` from the struct at `p`.
    unsafe fn field(&self, p: *mut c_void, off: usize) -> *mut c_void {
        (p as *mut u8).add(off) as *mut c_void
    }
    unsafe fn read_ptr(&self, p: *mut c_void, off: usize) -> *mut c_void {
        *(self.field(p, off) as *mut *mut c_void)
    }
    unsafe fn read_u8(&self, p: *mut c_void, off: usize) -> u8 {
        *(self.field(p, off) as *mut u8)
    }
    unsafe fn read_u32(&self, p: *mut c_void, off: usize) -> u32 {
        *(self.field(p, off) as *mut u32)
    }
    unsafe fn read_i32(&self, p: *mut c_void, off: usize) -> i32 {
        *(self.field(p, off) as *mut i32)
    }
    unsafe fn read_i64(&self, p: *mut c_void, off: usize) -> i64 {
        *(self.field(p, off) as *mut i64)
    }
    /// A 26.6 fixed-point value rounded to whole pixels.
    unsafe fn fixed(&self, p: *mut c_void, off: usize) -> i32 {
        let v = self.read_i64(p, off);
        ((v + 32) >> 6) as i32
    }
    /// A NUL-terminated C string field.
    unsafe fn cstr(&self, p: *mut c_void, off: usize) -> Option<String> {
        let s = self.read_ptr(p, off) as *const c_char;
        if s.is_null() {
            return None;
        }
        Some(std::ffi::CStr::from_ptr(s).to_string_lossy().into_owned())
    }
}

impl Drop for Ft {
    fn drop(&mut self) {
        unsafe {
            let _ = (self.fns.done_face)(self.face);
            let _ = (self.fns.done_library)(self.library);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Ft;

    /// The system CJK face the engine ships with. Tests skip when it is absent
    /// rather than failing a machine that has no Noto installed.
    const FONT: &str = "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc";
    /// Face 0 of that file is Japanese, face 2 Simplified Chinese.
    const SC_FACE: i64 = 2;

    #[test]
    fn a_full_width_glyph_advances_one_em() {
        let Some(mut ft) = Ft::open(FONT, 40, 0) else { return };
        let g = match ft.glyph('一') {
            Some(g) => g,
            None => return,
        };
        // An ideograph fills its advance: one em at 40px, in 26.6.
        assert_eq!(g.advance_266, 40 * 64);
        assert!(g.w > 30 && g.w <= 40, "ink width {}", g.w);
        // 一 is one horizontal stroke, far wider than tall. A wrong struct
        // offset would instead hand back 40x40 of noise.
        assert!(g.h > 0 && g.h < 12, "ink height {}", g.h);
        assert!(g.mask.iter().any(|&v| v > 200), "no solid coverage");
    }

    #[test]
    fn latin_is_narrower_than_a_kanji() {
        let Some(mut ft) = Ft::open(FONT, 40, 0) else { return };
        let (Some(a), Some(i)) = (ft.glyph('A'), ft.glyph('一')) else { return };
        assert!(a.advance_266 < i.advance_266, "A {} vs 一 {}", a.advance_266, i.advance_266);
        // Same baseline: both sit on it, so 一 rises far less than the cap.
        assert!(a.top > i.top, "A top {} vs 一 top {}", a.top, i.top);
    }

    /// The face index is what picks the language inside a .ttc: opening the
    /// same file at 0 and at 2 must report different families, and index 2
    /// must be the Simplified Chinese one the Chinese text needs.
    #[test]
    fn the_face_index_picks_the_language_inside_a_ttc() {
        let Some(jp) = Ft::open(FONT, 26, 0) else { return };
        let Some(sc) = Ft::open(FONT, 26, SC_FACE) else { return };
        assert_eq!(jp.face_index(), 0);
        assert_eq!(sc.face_index(), SC_FACE);
        assert!(sc.num_faces() >= 3, "only {} faces", sc.num_faces());
        let (Some(jpf), Some(scf)) = (jp.family(), sc.family()) else { return };
        assert_ne!(jpf, scf, "face 0 and 2 share the family {jpf}");
        assert!(scf.contains("SC"), "face 2 is {scf}, not SC");
    }

    /// The size object, not the face record, carries the scaled metrics - the
    /// numbers layout needs for the line box.
    #[test]
    fn size_metrics_are_in_pixels() {
        let Some(mut ft) = Ft::open(FONT, 26, SC_FACE) else { return };
        let Some(m) = ft.metrics() else { return };
        assert!(m.ascender > 20 && m.ascender < 34, "ascender {}", m.ascender);
        // Descender is negative and much shallower than the ascender.
        assert!(m.descender < 0 && m.descender > -14, "descender {}", m.descender);
        assert!(m.height > m.ascender, "height {} <= ascender", m.height);
        // Same face at 52px must be about twice as tall.
        assert!(ft.set_size(52).is_some());
        let Some(big) = ft.metrics() else { return };
        let ratio = big.ascender as f32 / m.ascender as f32;
        assert!((ratio - 2.0).abs() < 0.15, "ascender {:?} -> {:?}", m, big);
    }

    /// Latin pairs carry kerning in the face; CJK pairs do not need it.
    #[test]
    fn kerning_moves_a_latin_pair() {
        let Some(mut ft) = Ft::open(FONT, 26, SC_FACE) else { return };
        let k = ft.kerning_266('A', 'V');
        // Noto Sans CJK kerns AV; the value is in 26.6, so a pixel of kern is
        // 64 units and the pair must not read as an error (which is 0 anyway).
        assert!(k.abs() <= 4 * 64, "AV kerning {k}");
        let cjk = ft.kerning_266('一', '丁');
        assert_eq!(cjk, 0, "CJK pair kerned {cjk}");
    }

    /// Some CJK faces carry monochrome bitmap strikes (the arphic uming/ukai
    /// families do, from 16px up). Loading a strike would hand back a pixel
    /// mode this module cannot use - and an earlier build even failed to open
    /// the face at that size - so the outline is rendered instead, at every
    /// size a window scale can ask for.
    #[test]
    fn embedded_bitmap_strikes_do_not_break_a_size() {
        const UMING: &str = "/usr/share/fonts/truetype/arphic/uming.ttc";
        if !std::path::Path::new(UMING).is_file() {
            return;
        }
        for size in [9u32, 12, 15, 16, 17, 20, 24, 26, 40] {
            let Some(mut ft) = Ft::open(UMING, size, 0) else {
                panic!("uming.ttc does not open at {}px", size);
            };
            let Some(g) = ft.glyph('A') else { panic!("no A at {}px", size) };
            assert!(g.w > 0 && g.h > 0, "A has no coverage at {}px", size);
            assert!(g.mask.iter().any(|&v| v > 0), "A is blank at {}px", size);
        }
    }
}
