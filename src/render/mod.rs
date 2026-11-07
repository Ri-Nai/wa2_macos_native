//! Offscreen scene compositor.
//!
//! This is display-independent: it renders into an in-memory RGBA buffer so it
//! can be verified headlessly (dumped to TGA) and later blitted to an SDL2
//! texture without changing the compositing logic.
//!
//! Pipeline: load images via `codec::tga`, draw background/sprite/dialog box,
//! rasterize text glyphs from the font atlas, then `save_tga` (or, in the SDL2
//! frontend, upload `pixels` to a texture).

use crate::codec::tga::TgaImage;
use std::fs::File;
use std::io::{self, BufWriter, Write};

pub mod font;
pub mod text;
pub use font::Font;

pub struct Surface {
    pub width: u32,
    pub height: u32,
    /// RGBA, row-major, top-down.
    pub pixels: Vec<u8>,
}

fn blend(dst: &[u8; 4], src: &[u8; 4]) -> [u8; 4] {
    let a = src[3] as u32;
    if a == 0 {
        return *dst;
    }
    if a == 255 {
        return *src;
    }
    let ia = 255 - a;
    [
        ((src[0] as u32 * a + dst[0] as u32 * ia) / 255) as u8,
        ((src[1] as u32 * a + dst[1] as u32 * ia) / 255) as u8,
        ((src[2] as u32 * a + dst[2] as u32 * ia) / 255) as u8,
        255,
    ]
}

/// Box-filter one source rectangle down to a single pixel.
///
/// Used when a decoded asset is larger than the destination: a 2x background
/// on a 1920x1080 window, or an 8x sprite on anything smaller than 8x. Nearest
/// neighbour throws away whole rows and columns when shrinking and makes the
/// AI-upscaled art look noisy; averaging the covered source rectangle keeps the
/// detail instead. Colour is averaged weighted by alpha so transparent sprite
/// pixels do not darken the edge, and the result carries the average alpha.
fn area_sample(img: &TgaImage, x0: i32, y0: i32, x1: i32, y1: i32) -> [u8; 4] {
    let iw = img.width as i32;
    let ih = img.height as i32;
    if iw <= 0 || ih <= 0 {
        return [0, 0, 0, 0];
    }
    let x0 = x0.max(0).min(iw - 1);
    let x1 = x1.max(x0 + 1).min(iw);
    let y0 = y0.max(0).min(ih - 1);
    let y1 = y1.max(y0 + 1).min(ih);
    let (mut sa, mut sr, mut sg, mut sb, mut n) = (0u64, 0u64, 0u64, 0u64, 0u64);
    for y in y0..y1 {
        for x in x0..x1 {
            let i = ((y as u32 * img.width + x as u32) * 4) as usize;
            let a = img.rgba[i + 3] as u64;
            sa += a;
            sr += img.rgba[i] as u64 * a;
            sg += img.rgba[i + 1] as u64 * a;
            sb += img.rgba[i + 2] as u64 * a;
            n += 1;
        }
    }
    if n == 0 || sa == 0 {
        return [0, 0, 0, 0];
    }
    [(sr / sa) as u8, (sg / sa) as u8, (sb / sa) as u8, (sa / n) as u8]
}

impl Surface {
    pub fn new(width: u32, height: u32, bg: [u8; 4]) -> Surface {
        Surface {
            width,
            height,
            pixels: vec![bg[0], bg[1], bg[2], bg[3]].repeat((width * height) as usize),
        }
    }

    /// Resize into `dw`×`dh`. An integer multiple is copied pixel for pixel.
    /// Anything else uses bilinear, so a fullscreen stretch of the scene does
    /// not have to filter text that will be drawn afterwards at window size.
    pub fn scaled_to(&self, dw: u32, dh: u32) -> Surface {
        if dw == 0 || dh == 0 || self.width == 0 || self.height == 0 {
            return Surface::new(dw.max(1), dh.max(1), [0, 0, 0, 255]);
        }
        if dw == self.width && dh == self.height {
            return Surface { width: dw, height: dh, pixels: self.pixels.clone() };
        }
        let mut pixels = vec![0u8; (dw as usize) * (dh as usize) * 4];
        if dw % self.width == 0 && dh % self.height == 0 && dw / self.width == dh / self.height {
            let s = (dw / self.width) as usize;
            let sw = self.width as usize;
            for y in 0..self.height as usize {
                for x in 0..sw {
                    let si = (y * sw + x) * 4;
                    let px = &self.pixels[si..si + 4];
                    for dy in 0..s {
                        let row = ((y * s + dy) * dw as usize) * 4;
                        for dx in 0..s {
                            let di = row + (x * s + dx) * 4;
                            pixels[di..di + 4].copy_from_slice(px);
                        }
                    }
                }
            }
            return Surface { width: dw, height: dh, pixels };
        }
        let sw = self.width as f32;
        let sh = self.height as f32;
        for y in 0..dh {
            let fy = (y as f32 + 0.5) * sh / dh as f32 - 0.5;
            let y0 = fy.floor().clamp(0.0, sh - 1.0) as u32;
            let y1 = (y0 + 1).min(self.height - 1);
            let ty = (fy - y0 as f32).clamp(0.0, 1.0);
            for x in 0..dw {
                let fx = (x as f32 + 0.5) * sw / dw as f32 - 0.5;
                let x0 = fx.floor().clamp(0.0, sw - 1.0) as u32;
                let x1 = (x0 + 1).min(self.width - 1);
                let tx = (fx - x0 as f32).clamp(0.0, 1.0);
                let mut out = [0u8; 4];
                for c in 0..4u32 {
                    let p00 = self.pixels[((y0 * self.width + x0) * 4 + c) as usize] as f32;
                    let p10 = self.pixels[((y0 * self.width + x1) * 4 + c) as usize] as f32;
                    let p01 = self.pixels[((y1 * self.width + x0) * 4 + c) as usize] as f32;
                    let p11 = self.pixels[((y1 * self.width + x1) * 4 + c) as usize] as f32;
                    let top = p00 + (p10 - p00) * tx;
                    let bot = p01 + (p11 - p01) * tx;
                    out[c as usize] = (top + (bot - top) * ty).round().clamp(0.0, 255.0) as u8;
                }
                let di = ((y * dw + x) * 4) as usize;
                pixels[di..di + 4].copy_from_slice(&out);
            }
        }
        Surface { width: dw, height: dh, pixels }
    }

    fn put(&mut self, x: i32, y: i32, c: [u8; 4]) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let i = ((y as u32) * self.width + (x as u32)) as usize * 4;
        // Most game art pixels are opaque.  The old path built two temporary
        // arrays and ran the general alpha blender even for those pixels;
        // background and standing-sprite composition calls this once per
        // output pixel, so the shortcut is a substantial part of the hot path.
        if c[3] == 0 {
            return;
        }
        if c[3] == 255 {
            self.pixels[i..i + 4].copy_from_slice(&c);
            return;
        }
        let cur = [self.pixels[i], self.pixels[i + 1], self.pixels[i + 2], self.pixels[i + 3]];
        let b = blend(&cur, &c);
        self.pixels[i..i + 4].copy_from_slice(&b);
    }

    /// Blit a whole TGA image at (dx, dy) using its alpha channel.
    pub fn blit(&mut self, img: &TgaImage, dx: i32, dy: i32) {
        self.blit_region(img, 0, 0, img.width, img.height, dx, dy);
    }

    /// Blit a sub-rectangle (sx,sy,sw,sh) of `img` to (dx,dy).
    pub fn blit_region(
        &mut self,
        img: &TgaImage,
        sx: i32,
        sy: i32,
        sw: u32,
        sh: u32,
        dx: i32,
        dy: i32,
    ) {
        for row in 0..sh as i32 {
            for col in 0..sw as i32 {
                let sxp = sx + col;
                let syp = sy + row;
                if sxp < 0 || syp < 0 || sxp >= img.width as i32 || syp >= img.height as i32 {
                    continue;
                }
                let si = ((syp as u32) * img.width + (sxp as u32)) as usize * 4;
                let c = [
                    img.rgba[si],
                    img.rgba[si + 1],
                    img.rgba[si + 2],
                    img.rgba[si + 3],
                ];
                self.put(dx + col, dy + row, c);
            }
        }
    }

    /// Blend a single pixel (with alpha).
    pub fn blend_pixel(&mut self, x: i32, y: i32, c: [u8; 4]) {
        self.put(x, y, c);
    }

    /// Darken the whole buffer. `amount` 0 leaves it alone; 255 turns it black.
    /// Used for the novel-mode backdrop opacity, which is a mild dim of the
    /// scene art behind the window rather than a second plate.
    pub fn dim(&mut self, amount: u8) {
        if amount == 0 {
            return;
        }
        let keep = 255 - amount as u32;
        for px in self.pixels.chunks_exact_mut(4) {
            px[0] = (px[0] as u32 * keep / 255) as u8;
            px[1] = (px[1] as u32 * keep / 255) as u8;
            px[2] = (px[2] as u32 * keep / 255) as u8;
        }
    }

    /// Like `blit_region` but scales the source alpha by `alpha` (0..=255),
    /// used to dim non-speaking characters.
    pub fn blit_region_alpha(
        &mut self,
        img: &TgaImage,
        sx: i32,
        sy: i32,
        sw: u32,
        sh: u32,
        dx: i32,
        dy: i32,
        alpha: u8,
    ) {
        if alpha == 255 {
            self.blit_region(img, sx, sy, sw, sh, dx, dy);
            return;
        }
        for row in 0..sh as i32 {
            for col in 0..sw as i32 {
                let sxp = sx + col;
                let syp = sy + row;
                if sxp < 0 || syp < 0 || sxp >= img.width as i32 || syp >= img.height as i32 {
                    continue;
                }
                let si = ((syp as u32) * img.width + (sxp as u32)) as usize * 4;
                let a = (img.rgba[si + 3] as u32 * alpha as u32 / 255) as u8;
                let c = [img.rgba[si], img.rgba[si + 1], img.rgba[si + 2], a];
                self.put(dx + col, dy + row, c);
            }
        }
    }

    /// Blit a sub-rectangle of `img` scaled to (dw, dh), nearest-neighbour.
    /// Used by the command bar, whose icons are 40px cells on the 720 layout
    /// and have to follow the window.
    pub fn blit_region_scaled(
        &mut self,
        img: &TgaImage,
        sx: i32,
        sy: i32,
        sw: u32,
        sh: u32,
        dx: i32,
        dy: i32,
        dw: u32,
        dh: u32,
        alpha: u8,
    ) {
        if dw == 0 || dh == 0 || sw == 0 || sh == 0 {
            return;
        }
        // Shrinking (a 2x sprite on a 1080p window) averages the covered source
        // rectangle; enlarging keeps the cheap nearest sample.
        let shrink = sw > dw || sh > dh;
        for row in 0..dh as i32 {
            for col in 0..dw as i32 {
                let c = if shrink {
                    let x0 = sx + col * sw as i32 / dw as i32;
                    let x1 = sx + (col + 1) * sw as i32 / dw as i32;
                    let y0 = sy + row * sh as i32 / dh as i32;
                    let y1 = sy + (row + 1) * sh as i32 / dh as i32;
                    let mut c = area_sample(img, x0, y0, x1, y1);
                    c[3] = (c[3] as u32 * alpha as u32 / 255) as u8;
                    c
                } else {
                    let sxp = sx + col * sw as i32 / dw as i32;
                    let syp = sy + row * sh as i32 / dh as i32;
                    if sxp < 0 || syp < 0 || sxp >= img.width as i32 || syp >= img.height as i32 {
                        continue;
                    }
                    let si = ((syp as u32) * img.width + (sxp as u32)) as usize * 4;
                    let a = (img.rgba[si + 3] as u32 * alpha as u32 / 255) as u8;
                    [img.rgba[si], img.rgba[si + 1], img.rgba[si + 2], a]
                };
                self.put(dx + col, dy + row, c);
            }
        }
    }

    /// Add a whole image onto the surface, scaled to the given rectangle.
    ///
    /// The `f0NNN` plates an ACT146/147 `mode >= 0x80` selects are greyscale
    /// *light maps*, not pictures: `f0000` is a sun disc, `f0046` a wall of
    /// out-of-focus lights, `f0024` a band of reflected light on water. Their
    /// alpha is 255 everywhere and the luminance carries the effect, so they
    /// have to be added. Pasting one with alpha (what this used to do) replaces
    /// the background with the mask alone - a lit window floating in a black
    /// screen, or an indoor scene turned into a grey blob.
    pub fn add_scaled(&mut self, img: &TgaImage, dw: u32, dh: u32, strength: u8) {
        if dw == 0 || dh == 0 || img.width == 0 || img.height == 0 || strength == 0 {
            return;
        }
        let shrink = img.width > dw || img.height > dh;
        for row in 0..dh {
            for col in 0..dw {
                let x = col as i32;
                let y = row as i32;
                if x >= self.width as i32 || y >= self.height as i32 {
                    continue;
                }
                let c = if shrink {
                    let x0 = (col as u64 * img.width as u64 / dw as u64) as i32;
                    let x1 = ((col as u64 + 1) * img.width as u64 / dw as u64) as i32;
                    let y0 = (row as u64 * img.height as u64 / dh as u64) as i32;
                    let y1 = ((row as u64 + 1) * img.height as u64 / dh as u64) as i32;
                    area_sample(img, x0, y0, x1, y1)
                } else {
                    let sy = (row as u64 * img.height as u64 / dh as u64) as u32;
                    let sx = (col as u64 * img.width as u64 / dw as u64) as u32;
                    let si = ((sy.min(img.height - 1) * img.width + sx.min(img.width - 1)) * 4) as usize;
                    [img.rgba[si], img.rgba[si + 1], img.rgba[si + 2], img.rgba[si + 3]]
                };
                if c[3] == 0 {
                    continue;
                }
                let k = c[3] as u32 * strength as u32 / 255;
                if k == 0 {
                    continue;
                }
                let di = ((y as u32 * self.width + x as u32) * 4) as usize;
                for ch in 0..3 {
                    self.pixels[di + ch] =
                        (self.pixels[di + ch] as u32 + c[ch] as u32 * k / 255).min(255) as u8;
                }
            }
        }
    }

    /// Nearest-neighbour scale of a whole image.  This is kept separate from
    /// `blit_region_scaled` because the scene background is opaque in normal
    /// play and covers almost every output pixel.  It avoids the per-pixel
    /// one-pixel `blit_region` call that used to dominate scene composition.
    pub fn blit_scaled_nearest(
        &mut self,
        img: &TgaImage,
        dx: i32,
        dy: i32,
        dw: u32,
        dh: u32,
    ) {
        if dw == 0 || dh == 0 || img.width == 0 || img.height == 0 {
            return;
        }
        // A decoded asset that is larger than the destination (2x art on a
        // 1920x1080 window, 8x art on anything smaller) is box-filtered instead
        // of point-sampled: nearest would keep one source pixel out of every
        // `scale^2` and alias the AI-upscaled detail badly.
        let shrink = img.width > dw || img.height > dh;
        for row in 0..dh {
            for col in 0..dw {
                let x = dx + col as i32;
                let y = dy + row as i32;
                if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
                    continue;
                }
                let c = if shrink {
                    let x0 = (col as u64 * img.width as u64 / dw as u64) as i32;
                    let x1 = ((col as u64 + 1) * img.width as u64 / dw as u64) as i32;
                    let y0 = (row as u64 * img.height as u64 / dh as u64) as i32;
                    let y1 = ((row as u64 + 1) * img.height as u64 / dh as u64) as i32;
                    area_sample(img, x0, y0, x1, y1)
                } else {
                    let sy = (row as u64 * img.height as u64 / dh as u64) as u32;
                    let sx = (col as u64 * img.width as u64 / dw as u64) as u32;
                    let si = ((sy.min(img.height - 1) * img.width + sx.min(img.width - 1)) * 4) as usize;
                    [img.rgba[si], img.rgba[si + 1], img.rgba[si + 2], img.rgba[si + 3]]
                };
                if c[3] == 0 {
                    continue;
                }
                let di = ((y as u32 * self.width + x as u32) * 4) as usize;
                if c[3] == 255 {
                    self.pixels[di..di + 4].copy_from_slice(&c);
                } else {
                    let a = c[3] as u32;
                    let ia = 255 - a;
                    self.pixels[di] = ((c[0] as u32 * a + self.pixels[di] as u32 * ia) / 255) as u8;
                    self.pixels[di + 1] = ((c[1] as u32 * a + self.pixels[di + 1] as u32 * ia) / 255) as u8;
                    self.pixels[di + 2] = ((c[2] as u32 * a + self.pixels[di + 2] as u32 * ia) / 255) as u8;
                    self.pixels[di + 3] = 255;
                }
            }
        }
    }

    /// Blit a raw RGBA buffer (w*h*4) scaled to (dw, dh), with alpha blending.
    pub fn blit_rgba_scaled(
        &mut self,
        rgba: &[u8],
        w: u32,
        h: u32,
        dx: i32,
        dy: i32,
        dw: u32,
        dh: u32,
    ) {
        if dw == 0 || dh == 0 || w == 0 || h == 0 {
            return;
        }
        for row in 0..dh as i32 {
            for col in 0..dw as i32 {
                let sx = col * w as i32 / dw as i32;
                let sy = row * h as i32 / dh as i32;
                let si = ((sy as u32) * w + sx as u32) as usize * 4;
                if si + 3 >= rgba.len() {
                    continue;
                }
                let c = [rgba[si], rgba[si + 1], rgba[si + 2], rgba[si + 3]];
                self.put(dx + col, dy + row, c);
            }
        }
    }

    /// Blit a raw RGBA buffer (w*h*4) at (dx,dy) with alpha blending.
    pub fn blit_rgba(&mut self, rgba: &[u8], w: u32, h: u32, dx: i32, dy: i32) {
        for row in 0..h as i32 {
            for col in 0..w as i32 {
                let si = ((row as u32) * w + col as u32) as usize * 4;
                if si + 3 >= rgba.len() { continue; }
                let c = [rgba[si], rgba[si+1], rgba[si+2], rgba[si+3]];
                self.put(dx + col, dy + row, c);
            }
        }
    }

    /// Copy a window of `src` into this surface, starting at source row `sy`.
    /// Used for the title background, which is a tall plate panned vertically.
    pub fn copy_rows_from(&mut self, src: &Surface, sy: i32) {
        if self.width == 0 || self.height == 0 || src.width == 0 {
            return;
        }
        let sy = sy.max(0) as u32;
        if sy >= src.height {
            return;
        }
        let rows = self.height.min(src.height - sy);
        let cols = self.width.min(src.width) as usize;
        let dst_stride = self.width as usize * 4;
        let src_stride = src.width as usize * 4;
        let nbytes = cols * 4;
        for row in 0..rows {
            let s = (sy + row) as usize * src_stride;
            let d = row as usize * dst_stride;
            self.pixels[d..d + nbytes].copy_from_slice(&src.pixels[s..s + nbytes]);
        }
    }

    /// Alpha-blit `src` onto this surface. Clear pixels are skipped.
    /// `alpha` scales the source coverage (255 leaves it unchanged).
    pub fn blit_surface_alpha(&mut self, src: &Surface, dx: i32, dy: i32, alpha: u8) {
        if alpha == 0 {
            return;
        }
        let x0 = dx.max(0);
        let y0 = dy.max(0);
        let x1 = (dx + src.width as i32).min(self.width as i32);
        let y1 = (dy + src.height as i32).min(self.height as i32);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let n = (x1 - x0) as usize;
        let mul = alpha as u32;
        for y in y0..y1 {
            let sy = (y - dy) as u32;
            let sx = (x0 - dx) as u32;
            let s0 = (sy * src.width + sx) as usize * 4;
            let d0 = (y as u32 * self.width + x0 as u32) as usize * 4;
            for i in 0..n {
                let s = s0 + i * 4;
                let a = src.pixels[s + 3] as u32 * mul / 255;
                if a == 0 {
                    continue;
                }
                let d = d0 + i * 4;
                let blended = blend(
                    &[
                        self.pixels[d],
                        self.pixels[d + 1],
                        self.pixels[d + 2],
                        self.pixels[d + 3],
                    ],
                    &[src.pixels[s], src.pixels[s + 1], src.pixels[s + 2], a as u8],
                );
                self.pixels[d..d + 4].copy_from_slice(&blended);
            }
        }
    }

    /// Alpha-blit `src` onto this surface. Clear pixels are skipped.
    pub fn blit_surface(&mut self, src: &Surface, dx: i32, dy: i32) {
        let x0 = dx.max(0);
        let y0 = dy.max(0);
        let x1 = (dx + src.width as i32).min(self.width as i32);
        let y1 = (dy + src.height as i32).min(self.height as i32);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let n = (x1 - x0) as usize;
        for y in y0..y1 {
            let sy = (y - dy) as u32;
            let sx = (x0 - dx) as u32;
            let s0 = (sy * src.width + sx) as usize * 4;
            let d0 = (y as u32 * self.width + x0 as u32) as usize * 4;
            for i in 0..n {
                let s = s0 + i * 4;
                let a = src.pixels[s + 3];
                if a == 0 {
                    continue;
                }
                let d = d0 + i * 4;
                if a == 255 {
                    self.pixels[d..d + 4].copy_from_slice(&src.pixels[s..s + 4]);
                } else {
                    let blended = blend(
                        &[
                            self.pixels[d],
                            self.pixels[d + 1],
                            self.pixels[d + 2],
                            self.pixels[d + 3],
                        ],
                        &[
                            src.pixels[s],
                            src.pixels[s + 1],
                            src.pixels[s + 2],
                            src.pixels[s + 3],
                        ],
                    );
                    self.pixels[d..d + 4].copy_from_slice(&blended);
                }
            }
        }
    }

    /// Fill a rectangle (with alpha) - used for the dialog box.
    pub fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, color: [u8; 4]) {
        if w == 0 || h == 0 {
            return;
        }
        // Opaque covers are a straight copy. The blend path calls `put` per
        // pixel, which is too slow for a full chart redraw every frame.
        if color[3] == 255 {
            let x0 = x.max(0);
            let y0 = y.max(0);
            let x1 = x.saturating_add(w as i32).min(self.width as i32);
            let y1 = y.saturating_add(h as i32).min(self.height as i32);
            if x0 >= x1 || y0 >= y1 {
                return;
            }
            let stride = self.width as usize * 4;
            let row_bytes = (x1 - x0) as usize * 4;
            for row in y0..y1 {
                let start = row as usize * stride + x0 as usize * 4;
                for px in self.pixels[start..start + row_bytes].chunks_exact_mut(4) {
                    px.copy_from_slice(&color);
                }
            }
            return;
        }
        for row in 0..h as i32 {
            for col in 0..w as i32 {
                self.put(x + col, y + row, color);
            }
        }
    }

    /// Apply one of the game's `.AMP` colour curves to every pixel.
    ///
    /// The files are plain LUTs: 256 bytes = one curve used for R, G and B
    /// (`nega.AMP` = inversion), 768 bytes = R, G, B curves (`evening.AMP`),
    /// 1280 bytes = five curves of which the first three are R, G and B.
    pub fn apply_lut(&mut self, lut: &[u8]) {
        if lut.len() < 256 {
            return;
        }
        let (r, g, b) = if lut.len() >= 768 {
            (&lut[0..256], &lut[256..512], &lut[512..768])
        } else {
            (&lut[0..256], &lut[0..256], &lut[0..256])
        };
        for px in self.pixels.chunks_exact_mut(4) {
            px[0] = r[px[0] as usize];
            px[1] = g[px[1] as usize];
            px[2] = b[px[2] as usize];
        }
    }

    /// Write the surface as an uncompressed 32-bit TGA (top-down, BGRA on disk).
    /// Box-filtered shrink of this surface, for save-slot thumbnails.
    ///
    /// The screen is 1280x720 and a slot shows it at ~242x136, i.e. every
    /// destination pixel covers about 5x5 source pixels; averaging them beats
    /// nearest-neighbour sampling, which aliases text into mush.
    pub fn thumbnail(&self, w: u32, h: u32) -> TgaImage {
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            let sy0 = (y as u64 * self.height as u64 / h as u64) as u32;
            let sy1 = (((y + 1) as u64 * self.height as u64 / h as u64) as u32).max(sy0 + 1);
            for x in 0..w {
                let sx0 = (x as u64 * self.width as u64 / w as u64) as u32;
                let sx1 = (((x + 1) as u64 * self.width as u64 / w as u64) as u32).max(sx0 + 1);
                let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
                for sy in sy0..sy1.min(self.height) {
                    for sx in sx0..sx1.min(self.width) {
                        let i = ((sy * self.width + sx) as usize) * 4;
                        r += self.pixels[i] as u64;
                        g += self.pixels[i + 1] as u64;
                        b += self.pixels[i + 2] as u64;
                        n += 1;
                    }
                }
                if n == 0 {
                    rgba.extend_from_slice(&[0, 0, 0, 255]);
                } else {
                    rgba.extend_from_slice(&[(r / n) as u8, (g / n) as u8, (b / n) as u8, 255]);
                }
            }
        }
        TgaImage { width: w, height: h, rgba }
    }

    pub fn save_tga(&self, path: &str) -> io::Result<()> {
        let mut f = BufWriter::new(File::create(path)?);
        let w = self.width as u16;
        let h = self.height as u16;
        let header = [
            0u8, 0u8, 2u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, (w & 0xFF) as u8, (w >> 8) as u8,
            (h & 0xFF) as u8, (h >> 8) as u8, 32u8, 0x28u8,
        ];
        f.write_all(&header)?;
        // One row of BGRA at a time: a frame is 1280x720, and writing each
        // pixel with its own write() turned every WA2_FRAME_DUMP capture into
        // a minute of syscalls.
        let mut row = Vec::with_capacity(self.width as usize * 4);
        for y in 0..self.height as usize {
            row.clear();
            let src = &self.pixels[y * self.width as usize * 4..(y + 1) * self.width as usize * 4];
            for px in src.chunks_exact(4) {
                row.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
            }
            f.write_all(&row)?;
        }
        f.flush()
    }
}

/// A character sprite to composite: image plus screen position and an alpha
/// multiplier (used to dim non-speaking characters).
pub struct SpriteDraw<'a> {
    pub img: &'a TgaImage,
    /// Logical 1280x720 size. The source may be an 8x image.
    pub logical_w: u32,
    pub logical_h: u32,
    pub x: i32,
    pub y: i32,
    pub alpha: u8,
}

/// Composite a visual-novel scene: background, standing sprites, bottom dialog
/// box, and the given text rendered with `font` (one line per wrapped logical
/// line).
pub fn render_scene(bg: &TgaImage, text: &str, font: &Font, w: u32, h: u32) -> Surface {
    render_scene_ex(bg, text, font, w, h, &[], None, true, None, None, 255, 0, &[])
}

/// One channel of the screen flash. 128 leaves the pixel alone, 255 pushes
/// it to white, 0 pushes it to black. `remain == frames` is the solid colour;
/// `remain == 0` is the picture.
fn flash_tone(pixel: u8, level: u32) -> u8 {
    if level >= 128 {
        let t = (level - 128) * 255 / 127;
        (pixel as u32 + (255 - pixel as u32) * t / 255) as u8
    } else {
        (pixel as u32 * level / 128) as u8
    }
}

/// ACT152. `remain` runs from `frames` (solid `rgb`) down to 0 (the picture).
/// Alpha is left as it was, so the window does not turn transparent.
pub fn apply_flash(dst: &mut [u8], src: &[u8], r: u8, g: u8, b: u8, remain: u32, frames: u32) {
    let n = dst.len().min(src.len());
    if frames == 0 || remain == 0 {
        dst[..n].copy_from_slice(&src[..n]);
        return;
    }
    let level = |c: u8| (remain * c as u32 + (frames - remain) * 128) / frames;
    let lr = level(r);
    let lg = level(g);
    let lb = level(b);
    let mut i = 0;
    while i + 3 < n {
        dst[i] = flash_tone(src[i], lr);
        dst[i + 1] = flash_tone(src[i + 1], lg);
        dst[i + 2] = flash_tone(src[i + 2], lb);
        dst[i + 3] = src[i + 3];
        i += 4;
    }
}

/// Linear blend of two equal-sized RGBA buffers. `step` runs from 1 to `frames`.
pub fn lerp_pixels(dst: &mut [u8], from: &[u8], to: &[u8], step: u32, frames: u32) {
    let n = dst.len().min(from.len()).min(to.len());
    if frames == 0 {
        dst[..n].copy_from_slice(&to[..n]);
        return;
    }
    let inv = frames.saturating_sub(step);
    let mut i = 0;
    while i < n {
        dst[i] = ((from[i] as u32 * inv + to[i] as u32 * step) / frames) as u8;
        i += 1;
    }
}

/// The scene is always laid out on the original 1280x720 grid; the composed
/// frame is then stretched to the window. Text is drawn after that, at the
/// window's own size (see `draw_dialog_vector`).
pub const LAYOUT_W: u32 = 1280;
pub const LAYOUT_H: u32 = 720;

/// `sys_00000` / `sys_00001` are blitted here. The oval's stroke runs from
/// about y=525 to y=666; glyphs start below the top glow and clear of the
/// snowflakes on either side.
pub const WINDOW_X: i32 = 16;
pub const WINDOW_Y: i32 = 496;
/// Speaker name, on the upper rim. The atlas ink starts ~7px into the cell,
/// so the cell origin sits that far above the white strokes.
pub const NAME_X: i32 = 245;
pub const NAME_Y: i32 = 505;
/// Height of the name's line box. The name is one short line on the rim and
/// gets its own box - it must not share the body's baseline maths.
pub const NAME_LINE: i32 = 28;
/// Dialogue line under the top glow. Measured off the original window:
/// white ink at game (270, 553), cell padding (6, 7), pitch 28.
pub const DIALOG_TEXT_X: i32 = 264;
pub const DIALOG_TEXT_Y: i32 = 546;
pub const DIALOG_TEXT_RIGHT: i32 = 1080;
/// Full-width advance. The 40px cell's ink is only ~25px, and the original
/// steps 28px between characters.
pub const DIALOG_PITCH: i32 = 28;
/// Baseline-to-baseline for a wrapped line.
pub const DIALOG_LINE: i32 = 36;

/// Like [`render_scene`] but composites the supplied character sprites between
/// the background and the dialog box.
pub fn render_scene_ex(
    bg: &TgaImage,
    text: &str,
    font: &Font,
    w: u32,
    h: u32,
    sprites: &[SpriteDraw],
    // Optional `.AMP` colour curve applied to the scene art (background and
    // sprites) but *not* to the dialog box or the text.
    lut: Option<&[u8]>,
    // 「ウィンドウ一時消去」: hide the dialog box and its text.
    window: bool,
    // Filled message window (`sys_00001`) and its oval frame (`sys_00000`).
    plate: Option<&TgaImage>,
    frame: Option<&TgaImage>,
    // 0..=255 scale applied on top of the plate's own alpha.
    window_alpha: u8,
    // 0..=255 darken of the background art in novel scenes (the original's
    // バックグラウンド不透明度). Characters and overlays keep full brightness;
    // the default setting is 0 (no dim).
    shade: u8,
    // Extra plates (ACT176): items, effect layers, composite CG pieces.
    // Already positioned in surface space, drawn under the dialog box.
    overlays: &[(&TgaImage, i32, i32, u32)],
) -> Surface {
    render_scene_ex_art(
        bg, text, font, w, h, sprites, lut, window, plate, frame,
        window_alpha, shade, overlays,
    )
}

/// Scene compositor with per-image source scales. Layout coordinates stay at
/// 1280x720, while the returned surface can be the actual window size. Each
/// overlay carries the scale represented by its decoded pixels, so mixed 1x,
/// 2x, 4x and 8x assets share the same logical placement.
/// Everything except the dialogue: background, sprites, overlays, the colour
/// filter and the message window itself.
///
/// Split out of render_scene_ex_art so the frontend can keep this layer
/// between lines and only repaint the text on top of a copy. Compositing
/// it costs a full-frame stretch of an 8x background plus a LUT pass,
/// which is far too much to redo for every character printed.
pub fn compose_scene_art(
    bg: &TgaImage,
    effect: Option<&TgaImage>,
    w: u32,
    h: u32,
    sprites: &[SpriteDraw],
    lut: Option<&[u8]>,
    window: bool,
    plate: Option<&TgaImage>,
    frame: Option<&TgaImage>,
    window_alpha: u8,
    shade: u8,
    overlays: &[(&TgaImage, i32, i32, u32)],
) -> Surface {
    let mut s = Surface::new(w, h, [0, 0, 0, 255]);
    // Background, scaled to fit (nearest-neighbour stretch).
    let (bw, bh) = (bg.width as f32, bg.height as f32);
    let scale = (w as f32 / bw).min(h as f32 / bh);
    let (dw, dh) = ((bw * scale) as u32, (bh * scale) as u32);
    let (ox, oy) = ((w - dw) / 2, (h - dh) / 2);
    s.blit_scaled_nearest(bg, ox as i32, oy as i32, dw, dh);

    // 「バックグラウンド不透明度（ノベルシーン時）」: the original darkens the
    // background art in normal scenes so the message window reads, but the
    // characters on stage are not background and keep full brightness.
    // Applying the dim here (after the background, before the sprites and
    // overlays) matches that: standing sprites used to go dark with it, which
    // made every character look unlit.
    s.dim(shade);

    // ACT146/147 mode >= 0x80 selects a full-screen `f0NNN` light map. It is
    // added on top of the dimmed background, under the standing sprites: the
    // cast stays crisp while the room behind them is lit.
    if let Some(effect) = effect {
        s.add_scaled(effect, w, h, 255);
    }

    // Standing character sprites (bottom-anchored), below the dialog box.
    //
    // Sprites and overlays live in the 1280x720 *layout* space, while the
    // background above is scaled from its own decoded (possibly 4x/8x) pixel
    // size. Scaling a sprite's width and height independently (w / LAYOUT_W vs
    // h / LAYOUT_H) only matches when the window is exactly 16:9; the moment
    // the window is any other shape - fullscreen on a 4:3 or ultrawide display,
    // or a dragged edge - the character gets squashed or stretched while the
    // background stays proportional. Uniform scaling keeps the portrait's
    // aspect ratio and pins it to the same letterboxed area the background
    // occupies.
    let layout_scale = (w as f32 / LAYOUT_W as f32).min(h as f32 / LAYOUT_H as f32);
    for sp in sprites {
        let dx = ox as i32 + (sp.x as f32 * layout_scale) as i32;
        let dy = oy as i32 + (sp.y as f32 * layout_scale) as i32;
        let dw = (sp.logical_w.max(1) as f32 * layout_scale).max(1.0) as u32;
        let dh = (sp.logical_h.max(1) as f32 * layout_scale).max(1.0) as u32;
        s.blit_region_scaled(
            sp.img, 0, 0, sp.img.width, sp.img.height,
            dx, dy, dw.max(1), dh.max(1), sp.alpha,
        );
    }
    for (img, x, y, art_scale) in overlays {
        let scale = (*art_scale).max(1);
        let logical_w = (img.width / scale).max(1);
        let logical_h = (img.height / scale).max(1);
        let dx = ox as i32 + ((*x).max(0) as f32 * layout_scale) as i32;
        let dy = oy as i32 + ((*y).max(0) as f32 * layout_scale) as i32;
        let dw = (logical_w as f32 * layout_scale).max(1.0) as u32;
        let dh = (logical_h as f32 * layout_scale).max(1.0) as u32;
        s.blit_region_scaled(img, 0, 0, img.width, img.height, dx, dy, dw.max(1), dh.max(1), 255);
    }

    // Colour filter (sepia / nega / ...) over the scene art.
    if let Some(lut) = lut {
        s.apply_lut(lut);
    }

    // Dialog box near the bottom. The lowest 44 px of the screen are left free
    // for the command button row, so the box is shorter than it used to be.
    let box_h = (h / 4).min(160).max(120);
    let box_y = (h - box_h - 44) as i32;
    // 「ウィンドウ一時消去」: the scene stays, the box and its text do not.
    if !window {
        return s;
    }
    // Game window: the feathered plate, then the oval, then the glyphs.
    // Without those assets, fall back to the flat plate the tools use.
    if plate.is_some() {
        if let Some(pl) = plate {
            // UI loaders normalize the message-window sheets to logical
            // coordinates before they reach the compositor.
            let pw = pl.width.max(1) * w / LAYOUT_W;
            let ph = pl.height.max(1) * h / LAYOUT_H;
            let px = WINDOW_X.max(0) as u32 * w / LAYOUT_W;
            let py = WINDOW_Y.max(0) as u32 * h / LAYOUT_H;
            s.blit_region_scaled(pl, 0, 0, pl.width, pl.height, px as i32, py as i32, pw.max(1), ph.max(1), window_alpha);
        }
        if let Some(fr) = frame {
            let fw = fr.width.max(1) * w / LAYOUT_W;
            let fh = fr.height.max(1) * h / LAYOUT_H;
            let fx = WINDOW_X.max(0) as u32 * w / LAYOUT_W;
            let fy = WINDOW_Y.max(0) as u32 * h / LAYOUT_H;
            s.blit_region_scaled(fr, 0, 0, fr.width, fr.height, fx as i32, fy as i32, fw.max(1), fh.max(1), window_alpha);
        }
    } else {
        s.fill_rect(30, box_y, w - 60, box_h, [10, 10, 24, 200]);
        s.fill_rect(30, box_y, w - 60, 2, [180, 200, 255, 220]);
        s.fill_rect(30, box_y + box_h as i32 - 2, w - 60, 2, [180, 200, 255, 220]);
    }
    s
}

/// compose_scene_art plus the dialogue text. Kept for the tools and for the
/// Japanese bitmap path, which stamps glyphs onto the 1280x720 layer.
pub fn render_scene_ex_art(
    bg: &TgaImage,
    text: &str,
    font: &Font,
    w: u32,
    h: u32,
    sprites: &[SpriteDraw],
    lut: Option<&[u8]>,
    window: bool,
    plate: Option<&TgaImage>,
    frame: Option<&TgaImage>,
    window_alpha: u8,
    shade: u8,
    overlays: &[(&TgaImage, i32, i32, u32)],
) -> Surface {
    let mut s = compose_scene_art(
        bg, None, w, h, sprites, lut, window, plate, frame, window_alpha, shade, overlays,
    );
    draw_dialog_text(&mut s, font, text, plate.is_some(), w, h, usize::MAX);
    s
}

/// Draw the dialogue body into an already-composited frame.
///
/// `limit` is how many characters of `text` to reveal. The typewriter paints
/// onto a frame that already has the background, the window and the name, so
/// a character tick does not rebuild the scene.
pub fn draw_dialog_text(
    s: &mut Surface,
    font: &Font,
    text: &str,
    plate: bool,
    w: u32,
    h: u32,
    limit: usize,
) {
    let shown: String = text.chars().take(limit).collect();
    let box_h = (h / 4).min(160).max(120);
    let box_y = (h - box_h - 44) as i32;
    let (x0, x_max, mut y, y_limit) = if plate {
        (DIALOG_TEXT_X, DIALOG_TEXT_RIGHT, DIALOG_TEXT_Y, DIALOG_TEXT_Y + DIALOG_LINE * 3)
    } else {
        (50, (w as i32) - 60, box_y + 18, box_y + box_h as i32)
    };

    // Prepare text: strip region tags, turn literal \n into newlines.
    let cleaned: String = shown.chars().filter(|&c| c != '\r').collect();
    let cleaned = cleaned.replace("<R", "\u{1}").replace('>', "\u{1}");
    let cleaned: String = cleaned
        .split('\u{1}')
        .enumerate()
        .map(|(i, part)| if i % 2 == 0 { part } else { "" })
        .collect();
    let cleaned = cleaned.replace("\\n", "\n");

    let full = font.cell() as i32;
    let step = if plate { DIALOG_PITCH } else { full };
    let line_h = if plate { DIALOG_LINE } else { full + 4 };
    for line in cleaned.lines() {
        let mut x = x0;
        let mut buf = String::new();
        for ch in line.chars() {
            let adv = if plate && (ch as u32) < 128 { step / 2 } else { step };
            if x + adv > x_max {
                break;
            }
            buf.push(ch);
            x += adv;
        }
        // Use `draw` (not `draw_with_fallback`) so unmapped characters leave a
        // gap instead of a magenta placeholder box: the text area then shows
        // only the glyphs that actually exist in the atlas, which reads much
        // cleaner than rows of placeholder bars.
        if plate {
            font.draw_pitch(s, &buf, x0, y, DIALOG_PITCH);
        } else {
            font.draw(s, &buf, x0, y);
        }
        y += line_h;
        if y > y_limit - line_h {
            break;
        }
    }
}


/// Dialogue font size on the 1280x720 layout. The bitmap cell is 40px stepped
/// at a 28px pitch, which puts the strokes at about 26px.
pub const DIALOG_FONT_720: u32 = 26;

/// Scale a 720-layout measurement into window pixels, rounding to nearest:
/// truncating here made the dialog font size jitter by a pixel between window
/// heights that are not multiples of 720.
pub fn layout_px(v: i32, wh: i32) -> i32 {
    (v * wh.max(1) + 360) / 720
}

/// Split scenario text into the lines the player reads: literal backslash-n
/// breaks the line, backslash-k vanishes, a ruby tag ``<Rbase|reading>``
/// keeps its base, and any other tag is dropped.
pub fn dialog_lines(text: &str) -> Vec<String> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
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
    out.lines().map(|l| l.to_string()).collect()
}

/// Draw the speaker name and the dialogue body with a vector font, at the
/// window's own pixel size.
///
/// This is the vector counterpart of ``draw_dialog_text``. The scene art under
/// it has already been stretched to the window, so the glyphs land on real
/// screen pixels instead of being enlarged with the picture. ``limit`` is how
/// many characters of the body to reveal.
pub fn draw_dialog_vector(
    s: &mut Surface,
    vf: &mut text::VectorFont,
    text: &str,
    speaker: Option<&str>,
    ww: i32,
    wh: i32,
    limit: usize,
) {
    // The player sets the dialogue size; a change is as rare as a resize, and
    // both drop the glyph cache, so the check is worth keeping.
    let ds = text::dialog_style();
    let size = layout_px(ds.size as i32, wh).max(8) as u32;
    if vf.size() != size {
        let _ = vf.set_size(size);
    }
    vf.set_edge_px(text::dialog_edge_px(size));
    // §11's remaining knobs: outline opacity, synthetic weight, letter tracking
    // and extra line pitch. The environment's global bold still applies on top
    // of the panel's weight so both overrides keep working.
    let want_bold = ds.weight.max(text::env_bold_px());
    if vf.bold() != want_bold {
        vf.set_bold(want_bold);
    }
    if vf.edge_alpha() != ds.edge_alpha.min(255) as u8 {
        vf.set_edge_alpha(ds.edge_alpha.min(255) as u8);
    }
    // Round to nearest rather than truncate: a truncated baseline or line
    // pitch jitters by a pixel when the window is resized or moved between
    // monitors.
    let sx = |v: i32| (v * ww.max(1) + 640) / 1280;
    let sy = |v: i32| (v * wh.max(1) + 360) / 720;
    let name_style = text::TextStyle::for_role(text::TextRole::Name);
    // The body takes the player's colour and rule; everything else about it
    // is the style the artwork was tuned for.
    let body_style = text::dialog_body_style();
    if let Some(name) = speaker {
        if !name.is_empty() {
            // The name sits on the window's rim, so it gets its own line box
            // instead of sharing the body's.
            let baseline = vf.baseline_in_box(sy(NAME_Y), sy(NAME_LINE));
            vf.draw_styled(s, name, sx(NAME_X), baseline, &name_style);
        }
    }
    let body: String = dialog_lines(text).join("\n");
    let shown: String = body.chars().take(limit).collect();
    let x0 = sx(DIALOG_TEXT_X);
    let x_max = sx(DIALOG_TEXT_RIGHT);
    let line_h = sy(DIALOG_LINE + ds.line_gap).max(1);
    // Three line boxes fit the oval; the check is on the box top, so the
    // centred baseline of the third line still counts as inside it.
    let y_limit = sy(DIALOG_TEXT_Y) + line_h * 3;
    let mut line_top = sy(DIALOG_TEXT_Y);
    for line in shown.lines() {
        if line_top >= y_limit {
            break;
        }
        let baseline = vf.baseline_in_box(line_top, line_h);
        // The pen stays in 26.6 fixed point across the whole line: Latin
        // advances are fractional, and rounding each one before adding the
        // next let a 30-character mixed line drift a couple of pixels.
        let mut pen = (x0 as i32) << 6;
        let x_max_266 = (x_max as i32) << 6;
        let mut prev: Option<char> = None;
        let track_266 = (ds.tracking as i32) << 6;
        let mut first = true;
        for ch in line.chars() {
            // Tracking goes into the gap before each glyph after the first, so
            // the first glyph stays on the left edge (the same rule the label
            // rasterizer uses).
            if !first {
                pen += track_266;
            }
            first = false;
            // The script pre-wraps its lines; anything whose ink would cross
            // the right edge of the oval is dropped rather than reflowed. The
            // check looks at the glyph's ink, not at the advance the next pen
            // position would start from, so a narrow last glyph still fits.
            let (dx, iw) = vf.char_ink(ch);
            if pen + ((dx + iw) as i32) * 64 > x_max_266 {
                break;
            }
            let pen_px = (pen + 32) >> 6;
            vf.draw_char_styled(s, ch, pen_px, baseline, &body_style);
            pen += vf.advance_266(prev, ch);
            prev = Some(ch);
        }
        if body_style.underline {
            // The rule follows the ink, not the box: a short last line gets a
            // short rule, the way a text box would draw it.
            let end = ((pen + 32) >> 6).min(x_max);
            let thick = (size / 16).max(1);
            let y = baseline + (size as i32 / 10).max(2);
            s.fill_rect(x0, y, (end - x0).max(0) as u32, thick as u32, [
                body_style.color[0],
                body_style.color[1],
                body_style.color[2],
                220,
            ]);
        }
        line_top += line_h;
    }
}

/// Build a sample scene: background, optional character sprite, dialog box, and a
/// strip of glyphs drawn from the font atlas. Proves the full offscreen pipeline.
pub fn demo_scene(font: &TgaImage, sprite: Option<&TgaImage>, w: u32, h: u32) -> Surface {
    let mut s = Surface::new(w, h, [20, 20, 40, 255]);

    if let Some(sp) = sprite {
        let dx = ((w as i32 - sp.width as i32) / 2).max(0);
        s.blit(sp, dx, 0);
    }

    let box_h = 200u32;
    let box_y = (h - box_h - 20) as i32;
    s.fill_rect(40, box_y, w - 80, box_h, [0, 0, 0, 180]);

    let cell = 64u32;
    let start_x = 60i32;
    let start_y = box_y + 10;
    let mut n = 0;
    for row in 0..6u32 {
        for col in 0..(w as u32 / cell).min(30) {
            if n >= 180 {
                break;
            }
            let gx = (col * cell) as i32;
            let gy = (row * cell) as i32;
            let dx = start_x + (col as i32) * (cell as i32);
            let dy = start_y + (row as i32) * (cell as i32);
            s.blit_region(font, gx, gy, cell, cell, dx, dy);
            n += 1;
        }
    }
    s
}

/// Like `render_scene`, but rasterizes the dialog text with a SYSTEM CJK font
/// (rasterized from a font file) instead of the game bitmap atlas.
///
/// Use this when the atlas glyph order is unknown: text stays fully readable
/// even though it is not drawn in the game's own face.
pub fn render_scene_sysfont(bg: &TgaImage, text: &str, w: u32, h: u32) -> Surface {
    let mut s = Surface::new(w, h, [0, 0, 0, 255]);
    let (bw, bh) = (bg.width as f32, bg.height as f32);
    let scale = (w as f32 / bw).min(h as f32 / bh);
    let (dw, dh) = ((bw * scale) as u32, (bh * scale) as u32);
    let (ox, oy) = ((w - dw) / 2, (h - dh) / 2);
    s.blit_scaled_nearest(bg, ox as i32, oy as i32, dw, dh);
    let box_h = (h / 4).max(140);
    let box_y = (h - box_h - 24) as i32;
    s.fill_rect(30, box_y, w - 60, box_h, [10, 10, 24, 200]);
    s.fill_rect(30, box_y, w - 60, 2, [180, 200, 255, 220]);
    s.fill_rect(30, box_y + box_h as i32 - 2, w - 60, 2, [180, 200, 255, 220]);

    // strip <R..> region tags and turn literal backslash-n into real newlines
    let cleaned: String = text.chars().filter(|&c| c != '\r').collect();
    let cleaned = cleaned.replace("<R", "\u{1}").replace('>', "\u{1}");
    let cleaned: String = cleaned
        .split('\u{1}')
        .enumerate()
        .map(|(i, s)| if i % 2 == 0 { s } else { "" })
        .collect();
    let cleaned = cleaned.replace("\\n", "\n");
    let lines: Vec<String> = cleaned.lines().map(|l| l.to_string()).collect();

    let tw = w - 100;
    let th = box_h - 30;
    let block = text::with_ui_font(26, |f| f.block_rgba(&lines, tw, th, 26)).flatten();
    if let Some((rw, rh, rgba)) = block {
        s.blit_rgba(&rgba, rw, rh, 50, box_y + 16);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::{apply_flash, draw_dialog_vector, lerp_pixels, Surface};
    use crate::render::text;
    use crate::render::{DIALOG_FONT_720, LAYOUT_H, LAYOUT_W};

    /// `DialogStyle` is process-global, so every test that reads or writes it
    /// takes this lock and puts the previous value back. Without it the
    /// parallel suite lets one test's tracking/underline leak into another's
    /// pixels, and a failing test leaves the style changed for the rest.
    static STYLE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct StyleGuard {
        saved: text::DialogStyle,
        // Held for the whole test: this is what serializes the style tests.
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl StyleGuard {
        fn new() -> StyleGuard {
            let lock = STYLE_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            StyleGuard { saved: text::dialog_style(), _lock: lock }
        }

        /// Apply a style for the lifetime of the guard.
        fn set(&self, style: text::DialogStyle) {
            let _ = text::set_dialog_style(style);
        }
    }

    impl Drop for StyleGuard {
        fn drop(&mut self) {
            let _ = text::set_dialog_style(self.saved);
        }
    }

    /// The shipped style with a knob or two moved.
    fn style_with(f: impl FnOnce(&mut text::DialogStyle)) -> text::DialogStyle {
        let mut style = text::DialogStyle::default_style();
        f(&mut style);
        style
    }

    /// Every pixel above the ink threshold, for a "more/less ink" check.
    fn ink_pixels(s: &Surface) -> u32 {
        (0..(s.width * s.height) as usize)
            .filter(|i| s.pixels[i * 4] > 200)
            .count() as u32
    }

    /// The underline is drawn under the ink of every body line, and only when
    /// the player asked for it: it has to follow the line's own width, not the
    /// box's.
    #[test]
    fn dialogue_underline_follows_the_line() {
        let style = StyleGuard::new();
        let Some(mut vf) = text::VectorFont::open(text::DEFAULT_FONT, 26) else { return };
        let line = "abcde";
        let mut with = Surface::new(LAYOUT_W, LAYOUT_H, [0, 0, 0, 255]);
        let mut without = Surface::new(LAYOUT_W, LAYOUT_H, [0, 0, 0, 255]);
        style.set(style_with(|s| s.underline = true));
        draw_dialog_vector(&mut with, &mut vf, line, None, LAYOUT_W as i32, LAYOUT_H as i32, usize::MAX);
        style.set(style_with(|s| s.underline = false));
        draw_dialog_vector(&mut without, &mut vf, line, None, LAYOUT_W as i32, LAYOUT_H as i32, usize::MAX);

        // A row counts when a run of white pixels longer than any glyph row
        // appears in one frame and not in the other.
        let rows = |s: &Surface| -> Vec<i32> {
            let mut out = Vec::new();
            for y in 0..s.height as i32 {
                let mut run = 0u32;
                for x in 0..s.width as i32 {
                    let i = ((y as u32 * s.width + x as u32) * 4) as usize;
                    if s.pixels[i] > 200 && s.pixels[i + 1] > 200 && s.pixels[i + 2] > 200 {
                        run += 1;
                    } else {
                        if run > 20 {
                            out.push(y);
                        }
                        run = 0;
                    }
                }
            }
            out
        };
        let a = rows(&with);
        let b = rows(&without);
        let extra: Vec<i32> = a.iter().copied().filter(|y| !b.contains(y)).collect();
        assert_eq!(extra.len(), 1, "exactly one line is added, got {:?}", extra);
    }

    /// §11 letter tracking: the gap after each glyph gets `tracking` pixels, so
    /// the line ends further right while it still starts at the same edge.
    #[test]
    fn dialogue_tracking_widens_the_line() {
        let style = StyleGuard::new();
        let Some(mut vf) = text::VectorFont::open(text::DEFAULT_FONT, 26) else { return };
        let line = "abcde";
        style.set(style_with(|s| s.tracking = 0));
        let mut tight = Surface::new(LAYOUT_W, LAYOUT_H, [0, 0, 0, 255]);
        draw_dialog_vector(&mut tight, &mut vf, line, None, LAYOUT_W as i32, LAYOUT_H as i32, usize::MAX);
        style.set(style_with(|s| s.tracking = 6));
        let mut wide = Surface::new(LAYOUT_W, LAYOUT_H, [0, 0, 0, 255]);
        draw_dialog_vector(&mut wide, &mut vf, line, None, LAYOUT_W as i32, LAYOUT_H as i32, usize::MAX);
        let (Some(a), Some(b)) = (ink_box(&tight), ink_box(&wide)) else { panic!("nothing was drawn") };
        assert_eq!(a.0, b.0, "the first glyph must stay on the left edge");
        // Four gaps at 6px each; the last glyph's ink moves by that much.
        assert!((b.2 - b.0) >= (a.2 - a.0) + 22, "width {} -> {}", a.2 - a.0 + 1, b.2 - b.0 + 1);
    }

    /// §11 line gap: the first line keeps its place and each following line is
    /// pushed down by the extra pitch.
    #[test]
    fn dialogue_line_gap_pushes_the_lines_apart() {
        let style = StyleGuard::new();
        let Some(mut vf) = text::VectorFont::open(text::DEFAULT_FONT, 26) else { return };
        let body = "abc\ndef";
        style.set(style_with(|s| s.line_gap = 0));
        let mut tight = Surface::new(LAYOUT_W, LAYOUT_H, [0, 0, 0, 255]);
        draw_dialog_vector(&mut tight, &mut vf, body, None, LAYOUT_W as i32, LAYOUT_H as i32, usize::MAX);
        style.set(style_with(|s| s.line_gap = 12));
        let mut loose = Surface::new(LAYOUT_W, LAYOUT_H, [0, 0, 0, 255]);
        draw_dialog_vector(&mut loose, &mut vf, body, None, LAYOUT_W as i32, LAYOUT_H as i32, usize::MAX);
        let (Some(a), Some(b)) = (ink_box(&tight), ink_box(&loose)) else { panic!("nothing was drawn") };
        // Each line box centres the baseline, so the first line drifts down by
        // half the extra pitch; the second line gets the whole pitch on top of
        // that, so the block grows by about the gap.
        let drift = b.1 - a.1;
        assert!((0..=7).contains(&drift), "the first line drifts by half the gap, got {drift}");
        let grow = (b.3 - b.1) - (a.3 - a.1);
        assert!((8..=16).contains(&grow), "the block should grow by about the extra pitch, got {grow}");
    }

    /// §11 synthetic weight: a heavier body covers more pixels than the face's
    /// own weight. The global `WA2_FONT_BOLD` still wins over the panel's
    /// value, so the two cases start from whatever the environment asks for.
    #[test]
    fn dialogue_weight_thickens_the_ink() {
        let style = StyleGuard::new();
        let env = text::env_bold_px();
        if env >= text::DIALOG_WEIGHT_MAX {
            return;
        }
        let Some(mut vf) = text::VectorFont::open(text::DEFAULT_FONT, 26) else { return };
        let line = "冬馬和紗";
        style.set(style_with(|s| s.weight = env));
        let mut light = Surface::new(LAYOUT_W, LAYOUT_H, [0, 0, 0, 255]);
        draw_dialog_vector(&mut light, &mut vf, line, None, LAYOUT_W as i32, LAYOUT_H as i32, usize::MAX);
        style.set(style_with(|s| s.weight = env + 2));
        let mut heavy = Surface::new(LAYOUT_W, LAYOUT_H, [0, 0, 0, 255]);
        draw_dialog_vector(&mut heavy, &mut vf, line, None, LAYOUT_W as i32, LAYOUT_H as i32, usize::MAX);
        let (a, b) = (ink_pixels(&light), ink_pixels(&heavy));
        assert!(a > 0, "the light body left no ink");
        assert!(b > a + a / 10, "ink {} -> {}", a, b);
    }

    /// §11 outline opacity: the outline is black, so it only shows on a
    /// lighter backdrop. With opacity 0 the darkest pixel is the soft shadow
    /// (110 alpha over the backdrop); a full-strength outline goes past that.
    #[test]
    fn dialogue_edge_alpha_controls_the_outline() {
        let style = StyleGuard::new();
        let Some(mut vf) = text::VectorFont::open(text::DEFAULT_FONT, 26) else { return };
        let line = "abcde";
        let backdrop = [80u8, 80, 80, 255];
        let darkest = |s: &Surface| -> u8 {
            (0..(s.width * s.height) as usize).map(|i| s.pixels[i * 4]).min().unwrap_or(255)
        };
        style.set(style_with(|s| s.edge_alpha = 0));
        let mut bare = Surface::new(LAYOUT_W, LAYOUT_H, backdrop);
        draw_dialog_vector(&mut bare, &mut vf, line, None, LAYOUT_W as i32, LAYOUT_H as i32, usize::MAX);
        style.set(style_with(|s| s.edge_alpha = 255));
        let mut edged = Surface::new(LAYOUT_W, LAYOUT_H, backdrop);
        draw_dialog_vector(&mut edged, &mut vf, line, None, LAYOUT_W as i32, LAYOUT_H as i32, usize::MAX);
        // The shadow floor for 110/255 over 80 is about 45.
        assert!(darkest(&bare) > 35, "no outline should stay above the shadow floor, got {}", darkest(&bare));
        assert!(darkest(&edged) < 20, "the outline should darken past the shadow, got {}", darkest(&edged));
    }


    #[test]
    fn flash_peaks_at_white_and_returns() {
        let src = [10u8, 20, 30, 255];
        let mut dst = [0u8; 4];
        apply_flash(&mut dst, &src, 255, 255, 255, 15, 15);
        assert_eq!(dst, [255, 255, 255, 255]);
        apply_flash(&mut dst, &src, 255, 255, 255, 0, 15);
        assert_eq!(dst, src);
    }

    #[test]
    fn lerp_hits_the_midpoint() {
        let from = [0u8, 10, 20, 255];
        let to = [100, 10, 0, 255];
        let mut dst = [0u8; 4];
        lerp_pixels(&mut dst, &from, &to, 1, 2);
        assert_eq!(dst, [50, 10, 10, 255]);
        lerp_pixels(&mut dst, &from, &to, 2, 2);
        assert_eq!(dst, to);
    }

    /// Shrinking an asset must average the covered source pixels. A 2x
    /// background on a 1080p window, or an 8x sprite on anything smaller, goes
    /// through this path; point sampling would keep one pixel in four and make
    /// the upscaled art look noisy.
    #[test]
    fn downscale_box_filters_instead_of_point_sampling() {
        use crate::codec::tga::TgaImage;
        let img = TgaImage {
            width: 2,
            height: 2,
            rgba: vec![
                0, 0, 0, 255, // black
                255, 255, 255, 255, // white
                255, 255, 255, 255, // white
                0, 0, 0, 255, // black
            ],
        };
        let mut s = Surface::new(1, 1, [0, 0, 0, 255]);
        s.blit_scaled_nearest(&img, 0, 0, 1, 1);
        let v = s.pixels[0];
        assert!(v > 100 && v < 160, "2x2 average should be mid grey, got {v}");
    }

    #[test]
    fn integer_scale_copies_pixels() {
        let mut s = Surface::new(2, 1, [0, 0, 0, 255]);
        s.pixels[0..4].copy_from_slice(&[0, 0, 0, 255]);
        s.pixels[4..8].copy_from_slice(&[255, 255, 255, 255]);
        let up = s.scaled_to(4, 2);
        assert_eq!(&up.pixels[0..4], &[0, 0, 0, 255]);
        assert_eq!(&up.pixels[4..8], &[0, 0, 0, 255]);
        assert_eq!(&up.pixels[8..12], &[255, 255, 255, 255]);
    }

    /// Ink bounding box of a surface, counting anything bright.
    fn ink_box(s: &Surface) -> Option<(i32, i32, i32, i32)> {
        let mut box_ = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for y in 0..s.height as i32 {
            for x in 0..s.width as i32 {
                let i = ((y as u32 * s.width + x as u32) * 4) as usize;
                if s.pixels[i] > 200 {
                    box_.0 = box_.0.min(x);
                    box_.1 = box_.1.min(y);
                    box_.2 = box_.2.max(x);
                    box_.3 = box_.3.max(y);
                }
            }
        }
        if box_.0 == i32::MAX { None } else { Some(box_) }
    }

    /// The whole point of drawing text after the scene is scaled: at 1080p the
    /// glyphs are rasterized 1.5x bigger, not stretched from 720p.
    #[test]
    fn dialog_text_follows_the_window_size() {
        // Reads the global style too: serialized with the tests that change it.
        let _style = StyleGuard::new();
        let Some(mut vf) = text::VectorFont::open(text::DEFAULT_FONT, DIALOG_FONT_720) else {
            return
        };
        let line = "冬馬和紗";
        let mut small = Surface::new(LAYOUT_W, LAYOUT_H, [0, 0, 0, 255]);
        draw_dialog_vector(&mut small, &mut vf, line, None, 1280, 720, usize::MAX);
        let mut big = Surface::new(1920, 1080, [0, 0, 0, 255]);
        draw_dialog_vector(&mut big, &mut vf, line, None, 1920, 1080, usize::MAX);
        let (Some(a), Some(b)) = (ink_box(&small), ink_box(&big)) else {
            panic!("nothing was drawn")
        };
        // Four full-width glyphs at 26px vs 39px.
        let wa = a.2 - a.0 + 1;
        let wb = b.2 - b.0 + 1;
        assert!((wb as f32 - wa as f32 * 1.5).abs() < 6.0, "width {wa} -> {wb}");
        // And the whole block sits 1.5x further down the screen.
        assert!((b.1 as f32 - a.1 as f32 * 1.5).abs() < 6.0, "top {} -> {}", a.1, b.1);
        assert!(
            (b.0 as f32 - a.0 as f32 * 1.5).abs() < 6.0,
            "left {} -> {}",
            a.0,
            b.0
        );
    }

    #[test]
    fn bilinear_scale_blends_an_edge() {
        let mut s = Surface::new(2, 1, [0, 0, 0, 255]);
        s.pixels[0..4].copy_from_slice(&[0, 0, 0, 255]);
        s.pixels[4..8].copy_from_slice(&[255, 255, 255, 255]);
        let up = s.scaled_to(4, 1);
        let mid = up.pixels[4];
        assert!(mid > 40 && mid < 220, "mid {mid}");
        assert_eq!(up.pixels[0], 0);
        assert_eq!(up.pixels[12], 255);
    }

    /// The 「バックグラウンド不透明度」 dim belongs to the background only: the
    /// characters on stage must keep full brightness (they used to go dark
    /// with the background, which made every sprite look unlit).
    #[test]
    fn background_dim_leaves_sprites_bright() {
        use super::{compose_scene_art, SpriteDraw};
        use crate::codec::tga::TgaImage;
        let bg = TgaImage {
            width: LAYOUT_W,
            height: LAYOUT_H,
            rgba: [200u8, 200, 200, 255].repeat((LAYOUT_W * LAYOUT_H) as usize),
        };
        let sprite = TgaImage { width: 64, height: 64, rgba: [250u8, 250, 250, 255].repeat(64 * 64) };
        let draws = [SpriteDraw {
            img: &sprite,
            logical_w: 64,
            logical_h: 64,
            x: 608,
            y: 656,
            alpha: 255,
        }];
        let undimmed = compose_scene_art(&bg, None, LAYOUT_W, LAYOUT_H, &draws, None, false, None, None, 255, 0, &[]);
        let dimmed = compose_scene_art(&bg, None, LAYOUT_W, LAYOUT_H, &draws, None, false, None, None, 255, 90, &[]);
        let px = |s: &Surface, x: u32, y: u32| {
            let i = ((y * s.width + x) * 4) as usize;
            s.pixels[i]
        };
        assert_eq!(px(&undimmed, 10, 10), 200);
        assert!(px(&dimmed, 10, 10) < 150, "background should dim, got {}", px(&dimmed, 10, 10));
        // Inside the 64x64 character at (608,656).
        assert_eq!(px(&undimmed, 640, 690), 250);
        assert_eq!(
            px(&undimmed, 640, 690),
            px(&dimmed, 640, 690),
            "the background dim must not darken the character"
        );
    }

    #[test]
    fn effect_plate_is_between_background_and_sprites() {
        use super::{compose_scene_art, SpriteDraw};
        use crate::codec::tga::TgaImage;
        let bg = TgaImage {
            width: LAYOUT_W,
            height: LAYOUT_H,
            rgba: [20u8, 40, 80, 255].repeat((LAYOUT_W * LAYOUT_H) as usize),
        };
        let effect = TgaImage {
            width: LAYOUT_W,
            height: LAYOUT_H,
            rgba: [180u8, 20, 30, 255].repeat((LAYOUT_W * LAYOUT_H) as usize),
        };
        let sprite = TgaImage { width: 1, height: 1, rgba: vec![250, 250, 250, 255] };
        let draws = [SpriteDraw {
            img: &sprite,
            logical_w: 1,
            logical_h: 1,
            x: 640,
            y: 0,
            alpha: 255,
        }];
        let composed = compose_scene_art(
            &bg,
            Some(&effect),
            LAYOUT_W,
            LAYOUT_H,
            &draws,
            None,
            false,
            None,
            None,
            255,
            120,
            &[],
        );
        let px = |x: u32, y: u32| {
            let i = ((y * composed.width + x) * 4) as usize;
            [composed.pixels[i], composed.pixels[i + 1], composed.pixels[i + 2]]
        };
        // The `f0NNN` plate is a light map, not a picture: it is added over the
        // dimmed background and under the cast. Pasted with alpha (the old
        // behaviour) the mask alone became the screen - the "black background"
        // that hid the classroom behind a wall of bokeh.
        let lit = px(0, 0);
        // Background (20,40,80) dimmed by 120 -> (10,21,42), plus the plate's
        // own (180,20,30).
        assert_eq!(lit, [190, 41, 72]);
        assert_eq!(px(640, 0), [250, 250, 250]);
    }

    #[test]
    fn an_opaque_light_plate_never_covers_the_background() {
        use super::compose_scene_art;
        use crate::codec::tga::TgaImage;
        let bg = TgaImage {
            width: LAYOUT_W,
            height: LAYOUT_H,
            rgba: [60u8, 60, 60, 255].repeat((LAYOUT_W * LAYOUT_H) as usize),
        };
        // A plate with no picture in it at all: alpha 255 everywhere, every
        // pixel black. Added, it changes nothing.
        let black = TgaImage {
            width: LAYOUT_W,
            height: LAYOUT_H,
            rgba: [0u8, 0, 0, 255].repeat((LAYOUT_W * LAYOUT_H) as usize),
        };
        let composed = compose_scene_art(
            &bg, Some(&black), LAYOUT_W, LAYOUT_H, &[], None, false, None, None, 255, 0, &[],
        );
        let i = ((100 * composed.width + 100) * 4) as usize;
        assert_eq!(&composed.pixels[i..i + 3], &[60, 60, 60]);

        // And a fully white one is a whiteout, not a colour of its own.
        let mut white = black.clone();
        for px in white.rgba.chunks_mut(4) {
            px[0] = 255;
            px[1] = 255;
            px[2] = 255;
        }
        let composed = compose_scene_art(
            &bg, Some(&white), LAYOUT_W, LAYOUT_H, &[], None, false, None, None, 255, 0, &[],
        );
        let i = ((100 * composed.width + 100) * 4) as usize;
        assert_eq!(&composed.pixels[i..i + 3], &[255, 255, 255]);
    }

    #[test]
    fn a_standing_sprite_keeps_its_aspect_ratio_off_16_by_9() {
        use super::{compose_scene_art, SpriteDraw};
        use crate::codec::tga::TgaImage;
        // A 2:1 white sprite (200 logical px wide, 100 tall), bottom-anchored
        // and horizontally centred in the 1280-wide layout.
        let sprite = TgaImage {
            width: 200,
            height: 100,
            rgba: [250u8, 250, 250, 255].repeat(200 * 100),
        };
        let draws = [SpriteDraw {
            img: &sprite,
            logical_w: 200,
            logical_h: 100,
            x: 640 - 100,
            y: LAYOUT_H as i32 - 100,
            alpha: 255,
        }];
        let bg = TgaImage {
            width: LAYOUT_W,
            height: LAYOUT_H,
            rgba: [0u8, 0, 0, 255].repeat((LAYOUT_W * LAYOUT_H) as usize),
        };
        // A 1:1 window is very much not 16:9, so independent w/h scaling would
        // squash the sprite into a square. Uniform scaling must keep it 2:1.
        let composed = compose_scene_art(
            &bg, None, 720, 720, &draws, None, false, None, None, 255, 0, &[],
        );
        // Find the white bounding box.
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (u32::MAX, u32::MAX, 0u32, 0u32);
        for y in 0..composed.height {
            for x in 0..composed.width {
                let i = ((y * composed.width + x) * 4) as usize;
                if composed.pixels[i] > 200 {
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                }
            }
        }
        let w = (max_x - min_x + 1) as f32;
        let h = (max_y - min_y + 1) as f32;
        // The sprite keeps its 2:1 shape (tolerating nearest-neighbour rounding).
        assert!(
            (w / h - 2.0).abs() < 0.1,
            "sprite aspect {}x{} ({:.2}) should stay 2:1 on a square window",
            w, h, w / h
        );
    }
}
