//! Composing the scene: background, effect, tint and the cast on top of it.

use crate::*;

/// An ACT176 plate (item, effect, or a layer of a composite CG).
pub(crate) struct Plate {
    pub(crate) slot: u32,
    pub(crate) name: String,
    pub(crate) img: codec::tga::TgaImage,
    pub(crate) art_scale: u32,
}

/// How the message window and the scene behind it are drawn this frame.
pub(crate) struct Look {
    pub(crate) alpha: u8,
    pub(crate) shade: u8,
}

/// Runtime ACT171 overrides for the two message-window roles. The setting
/// page supplies the normal values; a script can temporarily force either
/// role to an exact 0..=255 alpha and the next scene redraw uses it.
pub(crate) fn script_window_alpha_store() -> &'static std::sync::Mutex<[Option<u8>; 2]> {
    static STORE: std::sync::OnceLock<std::sync::Mutex<[Option<u8>; 2]>> = std::sync::OnceLock::new();
    STORE.get_or_init(|| std::sync::Mutex::new([None, None]))
}

pub(crate) fn reset_script_window_alpha() {
    if let Ok(mut state) = script_window_alpha_store().lock() {
        *state = [None, None];
    }
}

pub(crate) fn set_script_window_alpha(value: i32, channel: i32) {
    let value = value.clamp(0, 255) as u8;
    if let Ok(mut state) = script_window_alpha_store().lock() {
        match channel {
            0 => *state = [Some(value), Some(value)],
            // 0x521910 is the novel-window value and 0x521914 is the
            // visual/CG value in the original executable.
            1 => state[0] = Some(value),
            2 => state[1] = Some(value),
            _ => {}
        }
    }
}

pub(crate) fn scene_look(settings: &menu::Settings, cg: bool) -> Look {
    let levels = menu::LEVELS as u32;
    let level = if cg { settings.alpha_cg } else { settings.alpha_novel };
    let level = level.clamp(0, menu::LEVELS) as u32;
    let script_alpha = script_window_alpha_store()
        .lock()
        .ok()
        .and_then(|state| state[usize::from(cg)].or(state[0]));
    Look {
        alpha: script_alpha.unwrap_or((level * 255 / levels) as u8),
        // Novel text gets a mild dim so the window reads. CG stays untouched,
        // and the top of the slider (10) only reaches about a third darker.
        shade: if cg {
            0
        } else {
            (settings.alpha_back.clamp(0, menu::LEVELS) as u32 * 90 / levels) as u8
        },
    }
}

pub(crate) fn clone_surf(s: &render::Surface) -> render::Surface {
    render::Surface {
        width: s.width,
        height: s.height,
        pixels: s.pixels.clone(),
    }
}

pub(crate) fn surface_from_tga(img: &codec::tga::TgaImage) -> render::Surface {
    render::Surface {
        width: img.width,
        height: img.height,
        pixels: img.rgba.clone(),
    }
}

pub(crate) fn place_overlays<'a>(plates: &'a [Plate], w: u32, h: u32) -> Vec<(&'a codec::tga::TgaImage, i32, i32, u32)> {
    plates
        .iter()
        .map(|p| {
            let scale = p.art_scale.max(1);
            let logical_w = (p.img.width / scale).max(1);
            let logical_h = (p.img.height / scale).max(1);
            let (x, y) = if logical_w >= 1100 || logical_h >= 600 {
                (0, 0)
            } else {
                (
                    (w as i32 - logical_w as i32) / 2,
                    (h as i32 - logical_h as i32) / 2,
                )
            };
            (&p.img, x, y, scale)
        })
        .collect()
}

/// Horizontal anchor for the idx-th of `count` on-screen characters.
pub(crate) fn sprite_x_for(idx: usize, count: usize, w: u32) -> i32 {
    let center = match (count, idx) {
        (1, _) => 640,
        (2, 0) => 360,
        (2, 1) => 920,
        (3, 0) => 330,
        (3, 1) => 640,
        (3, 2) => 950,
        (_, i) => 330 + (i.min(2) as i32) * 310,
    };
    center - (w as i32 / 2)
}

/// The composited picture without the dialogue, kept between lines.
///
/// Advancing the text used to stretch the background, blend the sprites, apply
/// the colour filter and stamp the message window again for every line. Only a
/// stage change (background, f0 effect, sprite, overlay, filter, window size) really needs
/// that work; printing a line only has to repaint the text on a copy.
pub(crate) struct SceneLayer {
    pub(crate) key: u64,
    pub(crate) w: u32,
    pub(crate) h: u32,
    pub(crate) surf: render::Surface,
}

pub(crate) fn scene_layer_store() -> &'static std::sync::Mutex<Option<SceneLayer>> {
    static LAYER: std::sync::OnceLock<std::sync::Mutex<Option<SceneLayer>>> = std::sync::OnceLock::new();
    LAYER.get_or_init(|| std::sync::Mutex::new(None))
}

/// FNV-1a over everything the picture is made of. Image identity is the pixel
/// buffer address: the same decoded picture keeps its address for as long as
/// the scene holds it, and a new load necessarily has a different one.
pub(crate) fn hash_u64(state: &mut u64, value: u64) {
    *state = (*state ^ value).wrapping_mul(0x100_0000_01b3);
}

/// Identity of a decoded picture for the scene cache.
///
/// The allocation address on its own is not an identity. When a background is
/// dropped - every scripted cut to black does exactly that - its buffer goes
/// back to the allocator, and the next decode of the same size usually lands
/// on that same address. The cache then compares equal, decides nothing has
/// changed, and keeps serving the previous picture; that is how a black frame
/// could survive the cut back into a scene. Mixing a few sampled pixels into
/// the hash makes the identity follow the content instead.
pub(crate) fn art_id(img: &codec::tga::TgaImage) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    hash_u64(&mut h, img.rgba.as_ptr() as usize as u64);
    hash_u64(&mut h, img.rgba.len() as u64);
    let n = img.rgba.len();
    if n != 0 {
        for i in 0..64u64 {
            let at = ((i * 7919 + 13) as usize) % n;
            hash_u64(&mut h, img.rgba[at] as u64);
        }
    }
    h
}

pub(crate) fn scene_art_key(
    bg: &codec::tga::TgaImage,
    effect: Option<&codec::tga::TgaImage>,
    draws: &[render::SpriteDraw],
    tint: Option<&[u8]>,
    window: bool,
    plate: Option<&codec::tga::TgaImage>,
    frame: Option<&codec::tga::TgaImage>,
    look: &Look,
    overlays: &[(&codec::tga::TgaImage, i32, i32, u32)],
    w: u32,
    h: u32,
) -> u64 {
    let mut key = 0xcbf2_9ce4_8422_2325u64;
    hash_u64(&mut key, art_id(bg));
    hash_u64(&mut key, bg.width as u64);
    hash_u64(&mut key, effect.map(art_id).unwrap_or(0));
    hash_u64(&mut key, effect.map(|e| e.width as u64).unwrap_or(0));
    hash_u64(&mut key, effect.map(|e| e.height as u64).unwrap_or(0));
    for sp in draws {
        hash_u64(&mut key, art_id(sp.img));
        hash_u64(&mut key, sp.logical_w as u64);
        hash_u64(&mut key, sp.x as u64);
        hash_u64(&mut key, sp.y as u64);
        hash_u64(&mut key, sp.alpha as u64);
    }
    hash_u64(&mut key, tint.map(|t| t.as_ptr() as usize as u64).unwrap_or(0));
    hash_u64(&mut key, tint.map(|t| t.len() as u64).unwrap_or(0));
    hash_u64(&mut key, window as u64);
    hash_u64(&mut key, plate.map(art_id).unwrap_or(0));
    hash_u64(&mut key, frame.map(art_id).unwrap_or(0));
    hash_u64(&mut key, look.alpha as u64);
    hash_u64(&mut key, look.shade as u64);
    for (img, x, y, scale) in overlays {
        hash_u64(&mut key, art_id(img));
        hash_u64(&mut key, *x as u64);
        hash_u64(&mut key, *y as u64);
        hash_u64(&mut key, *scale as u64);
    }
    hash_u64(&mut key, w as u64);
    hash_u64(&mut key, h as u64);
    key
}

/// Composite the scene picture, reusing the previous frame when nothing about
/// the stage has changed.
#[allow(clippy::too_many_arguments)]
pub(crate) fn scene_art(
    bg: &codec::tga::TgaImage,
    effect: Option<&codec::tga::TgaImage>,
    draws: &[render::SpriteDraw],
    tint: Option<&[u8]>,
    window: bool,
    plate: Option<&codec::tga::TgaImage>,
    frame: Option<&codec::tga::TgaImage>,
    look: &Look,
    overlays: &[(&codec::tga::TgaImage, i32, i32, u32)],
    w: u32,
    h: u32,
    key: u64,
) -> render::Surface {
    let mut store = match scene_layer_store().lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    if let Some(layer) = store.as_ref() {
        if layer.key == key && layer.w == w && layer.h == h {
            return clone_surf(&layer.surf);
        }
    }
    let surf = render::compose_scene_art(
        bg, effect, w, h, draws, tint, window, plate, frame, look.alpha, look.shade, overlays,
    );
    *store = Some(SceneLayer { key, w, h, surf: clone_surf(&surf) });
    surf
}

/// Composite background + sprites + dialog box + text.
pub(crate) fn draw_scene(
    bg: &codec::tga::TgaImage,
    text: &str,
    speaker: Option<&str>,
    font: &render::Font,
    slots: &[Option<ActiveSprite>; SPRITE_SLOTS],
    focus: Option<u32>,
    // Size of the frame to return - the window, not the 1280x720 layout.
    out_w: u32,
    out_h: u32,
    tint: Option<&[u8]>,
    effect: Option<&codec::tga::TgaImage>,
    // 「ウィンドウ一時消去」: false hides the dialog box and its text.
    window: bool,
    plate: Option<&codec::tga::TgaImage>,
    frame: Option<&codec::tga::TgaImage>,
    plates: &[Plate],
    look: Look,
    // Vector font for the dialogue, or None to keep the bitmap atlas.
    vf: Option<&mut render::text::VectorFont>,
) -> render::Surface {
    let t0 = std::time::Instant::now();
    // The Japanese bitmap atlas still belongs to the 1280x720 compositor. The
    // Chinese path uses vector text and can composite high-res art directly at
    // the window size without stretching the bitmap glyphs.
    let hires = vf.is_some() && (out_w > render::LAYOUT_W || out_h > render::LAYOUT_H);
    // Dim the listeners only when the speaker is actually standing there.
    // 春希 speaks thousands of times and is placed twice in the whole game:
    // he is the camera, and the person on screen is who he is talking to.
    // Dimming everyone in that case made the listener look like the wrong
    // speaker. When the speaker is on stage, the others drop well below
    // full brightness so the lit face is the name in the window.
    let speaker_on_stage = focus.is_some_and(|id| {
        slots.iter().flatten().any(|s| s.char_id == id)
    });
    let draws: Vec<render::SpriteDraw> = SPRITE_DRAW_ORDER
        .iter()
        .filter_map(|&i| {
            let s = slots[i].as_ref()?;
            let alpha = if speaker_on_stage && focus != Some(s.char_id) {
                96
            } else {
                255
            };
            let sprite_scale = s.art_scale.max(1);
            let logical_w = (s.img.width / sprite_scale).max(1);
            let logical_h = (s.img.height / sprite_scale).max(1);
            Some(render::SpriteDraw {
                img: &s.img,
                logical_w,
                logical_h,
                x: SPRITE_ANCHORS[i] - logical_w as i32 / 2,
                y: (render::LAYOUT_H as i32 - logical_h as i32).max(0),
                alpha,
            })
        })
        .collect();
    let overlays = place_overlays(plates, render::LAYOUT_W, render::LAYOUT_H);
    // With a vector font the text is drawn after the frame is scaled, so the
    // atlas path hands the text to the compositor and the vector path does not.
    let render_w = if hires { out_w } else { render::LAYOUT_W };
    let render_h = if hires { out_h } else { render::LAYOUT_H };
    let key = scene_art_key(
        bg, effect, &draws, tint, window, plate, frame, &look, &overlays, render_w, render_h,
    );
    let t_art = std::time::Instant::now();
    let mut s = scene_art(
        bg, effect, &draws, tint, window, plate, frame, &look, &overlays, render_w, render_h, key,
    );
    tlog("scene art", t_art);
    // The bitmap atlas stamps its glyphs onto this layer; the vector path
    // draws later, after the frame has been scaled to the window.
    if !vf.is_some() && window {
        render::draw_dialog_text(&mut s, font, text, plate.is_some(), render_w, render_h, usize::MAX);
    }
    let out = match vf {
        Some(vf) => {
            let mut s = if !hires && (out_w, out_h) != (render::LAYOUT_W, render::LAYOUT_H) {
                s.scaled_to(out_w, out_h)
            } else {
                s
            };
            if window {
                // The name sits on the upper rim of the oval, not inside 【】
                // in the line, and only the game's own window has a rim.
                let name = if plate.is_some() { speaker } else { None };
                let td = std::time::Instant::now();
                render::draw_dialog_vector(
                    &mut s,
                    vf,
                    text,
                    name,
                    out_w as i32,
                    out_h as i32,
                    usize::MAX,
                );
                tlog("dialog text", td);
            }
            s
        }
        None => {
            // The name sits on the upper rim of the oval, not inside 【】 in the line.
            if window && plate.is_some() {
                if let Some(name) = speaker {
                    if !name.is_empty() {
                        font.draw_pitch(&mut s, name, render::NAME_X, render::NAME_Y, render::DIALOG_PITCH);
                    }
                }
            }
            s
        }
    };
    tlog("draw_scene", t0);
    out
}

/// Draw the dialogue body onto a frame that is already composited, at the size
/// the frame actually is.
///
/// With a vector font the frame is window-sized and the glyphs are rasterized
/// at that size; otherwise the atlas is stamped onto the 1280x720 layer. `limit`
/// is how many characters to reveal.
pub(crate) fn draw_body(
    frame: &mut render::Surface,
    font: &render::Font,
    vf: Option<&mut render::text::VectorFont>,
    text: &str,
    speaker: Option<&str>,
    plate: bool,
    out_w: u32,
    out_h: u32,
    limit: usize,
) {
    match vf {
        Some(vf) => {
            let name = if plate { speaker } else { None };
            render::draw_dialog_vector(frame, vf, text, name, out_w as i32, out_h as i32, limit);
        }
        None => render::draw_dialog_text(
            frame,
            font,
            text,
            plate,
            render::LAYOUT_W,
            render::LAYOUT_H,
            limit,
        ),
    }
}
