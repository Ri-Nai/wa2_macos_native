//! Unit tests. They live in one file so the subsystem modules stay
//! free of test scaffolding.

use crate::*;

#[cfg(test)]
mod art_tree_tests {
    use crate::ordered_tree_paths;
    use std::path::PathBuf;

    #[test]
    fn ordered_trees_prefer_the_decode_scale() {
        let trees = vec![(4u32, PathBuf::from("bg_4x")), (8, PathBuf::from("bg_8x"))];
        let names = |want: u32| -> Vec<String> {
            ordered_tree_paths(&trees, want)
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect()
        };
        // Windowed decodes at 4x and must read the 4x tree first.
        assert_eq!(names(4), ["bg_4x", "bg_8x"]);
        // Fullscreen decodes at 8x and must read the master first.
        assert_eq!(names(8), ["bg_8x", "bg_4x"]);
        // A lower target takes the next larger tree, then the smaller one.
        assert_eq!(names(2), ["bg_4x", "bg_8x"]);
    }
}

#[cfg(test)]
mod prefetch_scan_tests {
    use crate::{prefetch_paths, upcoming_art, BgIndex};
    use crate::vm::lscr::{Op, Program};
    use std::collections::HashMap;

    #[test]
    fn upcoming_art_includes_effect_and_overlay_assets() {
        let program = Program {
            ops: vec![
                Op::Rec { off: 0, kind: 30 },
                Op::Int { off: 8, v: 0xA1 },
                Op::Int { off: 20, v: 2001 },
                Op::Int { off: 32, v: 0 },
                Op::Int { off: 44, v: 60 },
                Op::Act { off: 56, id: 146 },
                Op::Rec { off: 64, kind: 30 },
                Op::Tok { off: 72, n: 0 },
                Op::Tok { off: 80, n: 1 },
                Op::Tok { off: 88, n: 2 },
                Op::Int { off: 96, v: 0 },
                Op::Act { off: 108, id: 176 },
            ],
            index: HashMap::new(),
            labels: Vec::new(),
            tokens: vec!["ギター.tga".to_string(), "".to_string(), "grp".to_string()],
            entry: 0,
        };
        let index = BgIndex::build(&[]);
        let (bgs, sprites) = upcoming_art(&program, &index, None);
        assert!(bgs.iter().any(|n| n == "f0033.bmp"), "f0 effect was not queued: {bgs:?}");
        assert!(bgs.iter().any(|n| n == "ギター.tga"), "overlay was not queued: {bgs:?}");
        assert!(sprites.is_empty());
    }

    #[test]
    fn prefetch_paths_deduplicates_shared_assets() {
        let dir = std::env::temp_dir().join(format!("wa2-prefetch-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("f0033.bmp"), b"test").unwrap();
        let paths = prefetch_paths(
            &[dir.as_path()],
            &["f0033.bmp".to_string(), "f0033.bmp".to_string()],
            None,
            &[],
        );
        assert_eq!(paths, vec![dir.join("f0033.bmp")]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn upcoming_art_prefetches_the_scroll_cg() {
        // The inline ACT227 form: (mode, cg_id, variant, frames, ...) with
        // cg_id >= 1000. The tall picture must be queued like every other CG,
        // or it decodes synchronously while the screen sits on the black cut.
        let program = Program {
            ops: vec![
                Op::Rec { off: 0, kind: 30 },
                Op::Int { off: 8, v: 1 },
                Op::Int { off: 20, v: 20000 },
                Op::Int { off: 32, v: 0 },
                Op::Int { off: 44, v: 60 },
                Op::Act { off: 56, id: 227 },
            ],
            index: HashMap::new(),
            labels: Vec::new(),
            tokens: vec![],
            entry: 0,
        };
        let index = BgIndex::build(&[]);
        let (bgs, _) = upcoming_art(&program, &index, None);
        assert!(bgs.iter().any(|n| n == "v200000.tga"), "scroll CG not queued: {bgs:?}");
    }

    #[test]
    fn upcoming_art_prefetches_the_record_carried_scroll_cg() {
        // The K30-carried form: (mode, cg_id, variant, frames, 0, 0, h, n)
        // sits in the record before the ACT227. The interpreter raises it in
        // finish_record; the scanner must recognise the same shape.
        let program = Program {
            ops: vec![
                Op::Rec { off: 0, kind: 30 },
                Op::Int { off: 8, v: 1 },
                Op::Int { off: 20, v: 20190 },
                Op::Int { off: 32, v: 0 },
                Op::Int { off: 44, v: 60 },
                Op::Int { off: 56, v: 0 },
                Op::Int { off: 68, v: 0 },
                Op::Int { off: 80, v: 720 },
                Op::Int { off: 92, v: 2 },
                Op::Act { off: 104, id: 227 },
            ],
            index: HashMap::new(),
            labels: Vec::new(),
            tokens: vec![],
            entry: 0,
        };
        let index = BgIndex::build(&[]);
        let (bgs, _) = upcoming_art(&program, &index, None);
        assert!(bgs.iter().any(|n| n == "v201900.tga"), "scroll CG not queued: {bgs:?}");
    }
}

#[cfg(test)]
mod sprite_bank_tests {
    use crate::SpriteBank;

    /// The bank is built for the windowed tree and must move to the 8x tree
    /// when the decode scale grows (fullscreen), dropping the old pixels.
    #[test]
    fn sprite_bank_retargets_between_scaled_trees() {
        let tmp = std::env::temp_dir().join(format!("wa2-spritebank-{}", std::process::id()));
        let four = tmp.join("sprite_4x");
        let eight = tmp.join("sprite_8x");
        std::fs::create_dir_all(&four).unwrap();
        std::fs::create_dir_all(&eight).unwrap();
        std::fs::write(four.join("a.tga"), b"x").unwrap();
        std::fs::write(eight.join("b.webp"), b"x").unwrap();
        let mut bank = SpriteBank::new(vec![(4, four.clone()), (8, eight.clone())]);
        bank.retarget(4);
        assert_eq!(bank.dir, four);
        assert!(bank.index.contains("a.tga"));
        bank.retarget(8);
        assert_eq!(bank.dir, eight);
        assert!(bank.index.contains("b.webp"));
        assert!(bank.cache.is_empty(), "sprites from the old scale must be dropped");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

#[cfg(test)]
mod ui_sound_tests {
    use crate::*;

    /// A fake art tree: `<root>/bg` plus the `se/` directory next to it.
    fn tree(files: &[&str]) -> (std::path::PathBuf, Vec<std::path::PathBuf>) {
        let root = std::env::temp_dir().join(format!("wa2-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let se = root.join("se");
        std::fs::create_dir_all(root.join("bg")).unwrap();
        std::fs::create_dir_all(&se).unwrap();
        for f in files {
            std::fs::write(se.join(f), b"").unwrap();
        }
        let dirs: Vec<std::path::PathBuf> = vec![root.join("bg")];
        (root, dirs)
    }

    #[test]
    fn every_role_resolves_to_the_ids_the_exe_pushes() {
        let (_root, dirs) = tree(&["SE_9011.WAV", "SE_8905.WAV"]);
        let refs: Vec<&std::path::Path> = dirs.iter().map(|d| d.as_path()).collect();
        let ui = UiSound::resolve(&refs);
        let name = |k: UiSe| ui.path(k).map(|p| p.file_name().unwrap().to_string_lossy().into_owned());
        assert_eq!(name(UiSe::Move).as_deref(), Some("SE_9011.WAV"));
        assert_eq!(name(UiSe::Page).as_deref(), Some("SE_9011.WAV"));
        assert_eq!(name(UiSe::Confirm).as_deref(), Some("SE_8905.WAV"));
    }

    /// SE_8903 (0x22c7, the third UI wrapper) is not in the archives, so cancel
    /// has to fall back to SE_8905 rather than going silent.
    #[test]
    fn a_missing_cancel_sound_falls_back_to_the_menu_chime() {
        let (_root, dirs) = tree(&["SE_8905.WAV", "SE_9011.WAV"]);
        let refs: Vec<&std::path::Path> = dirs.iter().map(|d| d.as_path()).collect();
        let ui = UiSound::resolve(&refs);
        assert_eq!(ui.path(UiSe::Cancel), ui.path(UiSe::Confirm));
    }

    #[test]
    fn with_nothing_shipped_every_role_is_silent_not_a_crash() {
        let (_root, dirs) = tree(&[]);
        let refs: Vec<&std::path::Path> = dirs.iter().map(|d| d.as_path()).collect();
        let ui = UiSound::resolve(&refs);
        for k in [UiSe::Move, UiSe::Confirm, UiSe::Cancel, UiSe::Page] {
            assert!(ui.path(k).is_none(), "{k:?} should be silent");
        }
    }

    #[test]
    fn special_stats_have_one_counter_per_content_page() {
        let mut stats = SpecialStats::default();
        for pick in [
            SpecialPick::Novel,
            SpecialPick::Cg,
            SpecialPick::Replay,
            SpecialPick::Music,
            SpecialPick::Voice,
        ] {
            let i = SpecialStats::index(&pick).unwrap();
            stats.visited |= 1 << i;
            stats.entered[i] += 1;
        }
        assert_eq!(stats.visited.count_ones(), 5);
        assert!(stats.label(true).contains("已浏览 5/5"));
        assert_eq!(SpecialStats::index(&SpecialPick::Back), None);
    }

    #[test]
    fn h_scene_voice_rule_only_affects_the_explicit_sample_play_scenes() {
        let mut st = menu::Settings::default();
        st.ero_voice = 0;
        assert!(!h_scene_voice_allowed(&st, "2031", 2));
        assert!(h_scene_voice_allowed(&st, "2001", 2));
        st.ero_voice = 2;
        assert!(h_scene_voice_allowed(&st, "2031", 2));
        assert!(!h_scene_voice_allowed(&st, "2031", 10));
        st.ero_voice = 1;
        assert!(h_scene_voice_allowed(&st, "2031", 10));
    }
}

#[cfg(test)]
mod menu_bar {
    #[test]
    fn windowed_step_is_thirty_percent() {
        let mut y = 432.0;
        crate::slide_bar(&mut y, 472.0, 720);
        assert!((y - 444.0).abs() < 0.01, "{y}");
    }

    #[test]
    fn maximized_step_stays_within_six_screen_pixels() {
        let mut y = 432.0;
        let mut prev = crate::bar_window_y(y, 1080);
        for _ in 0..90 {
            crate::slide_bar(&mut y, 472.0, 1080);
            let now = crate::bar_window_y(y, 1080);
            assert!((now - prev).abs() <= 6, "{prev} -> {now}");
            prev = now;
            if y == 472.0 {
                break;
            }
        }
        assert_eq!(y, 472.0);
        // A fraction of a layout pixel still moves the bar on a tall window.
        assert_eq!(crate::bar_window_y(432.0, 1080), 648);
        assert_eq!(crate::bar_window_y(432.4, 1080), 649);
    }

    #[test]
    fn highlight_tracks_fractional_layout_y() {
        let rgba = vec![255u8; 320 * 40 * 4];
        let img = crate::codec::tga::TgaImage { width: 320, height: 40, rgba };
        let sprite = crate::scale_sprite(&img, 0, 0, 320, 40, 480, 60);
        let fitted = crate::FittedMenu {
            ww: 1920,
            wh: 1080,
            stamp: 0,
            plate: crate::render::Surface::new(1, 1, [0, 0, 0, 255]),
            bar: [Some(sprite), None],
        };
        let mut a = crate::render::Surface::new(1920, 1080, [1, 29, 89, 255]);
        let mut b = crate::render::Surface::new(1920, 1080, [1, 29, 89, 255]);
        crate::paint_bar(&mut a, &fitted, 0, 432.0, 480);
        crate::paint_bar(&mut b, &fitted, 0, 440.0, 480);
        let top = |s: &crate::render::Surface| -> i32 {
            for y in 0..s.height {
                for x in 0..s.width {
                    let i = ((y * s.width + x) * 4) as usize;
                    if s.pixels[i] > 200 {
                        return y as i32;
                    }
                }
            }
            -1
        };
        let dy = top(&b) - top(&a);
        assert!((dy - 12).abs() <= 1, "dy {dy}");
    }
}

#[cfg(test)]
mod rewind_history {
    #[test]
    fn rewind_drops_the_line_on_screen_and_keeps_the_previous() {
        let mut history = vec!["a", "b", "c"];
        assert_eq!(crate::take_rewind(&mut history, true).as_deref(), Some("b"));
        assert_eq!(history, ["a", "b"]);
        assert_eq!(crate::take_rewind(&mut history, true).as_deref(), Some("a"));
        assert_eq!(history, ["a"]);
        assert!(crate::take_rewind(&mut history, true).is_none());
        assert_eq!(history, ["a"]);
        // A choice has not pushed a frame of its own, so the line stays put.
        assert_eq!(crate::take_rewind(&mut history, false).as_deref(), Some("a"));
        assert_eq!(history, ["a"]);
    }
}

#[cfg(test)]
mod backlog_paint {
    #[test]
    fn history_paints_the_banner_and_the_lines() {
        let atlas = crate::codec::tga::decode_font(
            &std::fs::read("/home/lrxin/wa2-native/data/font/cn_body80.tga").unwrap(),
        )
        .unwrap();
        let font = crate::render::Font::from_cell_map(
            atlas,
            40,
            std::path::Path::new("/home/lrxin/wa2-native/data/font/wa2_cell_map_cn.tsv"),
        )
        .unwrap();
        let dirs = [
            std::path::Path::new("/home/lrxin/wa2-native/data/bg_cn"),
            std::path::Path::new("/home/lrxin/wa2-native/data/bg"),
        ];
        let header = crate::load_background(&dirs, "sys_00032.tga");
        let scroll = crate::load_background(&dirs, "sys_00030.tga");
        assert!(header.is_some());
        assert!(scroll.is_some());
        let scene = crate::render::Surface::new(1280, 720, [80, 120, 160, 255]);
        let lines = [
            "春希：不我".to_string(),
            "春希：不我".to_string(),
        ];
        let rows = crate::backlog_rows(&lines, None);
        assert!(rows.len() > 2, "a name and a body should not collapse into one row");
        assert!(rows.iter().any(|r| r.name), "the speaker should be its own row");
        let per_page = 19;
        let top = rows.len().saturating_sub(per_page);
        let surf = crate::paint_backlog(
            &scene,
            &font,
            None,
            &rows,
            top,
            per_page,
            header.as_ref(),
            scroll.as_ref(),
            false,
            false,
        );
        // Banner sits on the top edge and is not the flat scene colour.
        let i = (20 * 1280 + 200) * 4;
        assert!(surf.pixels[i + 2] > surf.pixels[i], "banner should be cyan, not grey");
        let _ = std::fs::create_dir_all("/tmp/wa2btn");
        let _ = surf.save_tga("/tmp/wa2btn/backlog.tga");
    }

    /// The history is the last screen that still reached for the atlas. In
    /// Chinese mode it draws with the vector font, and the two renders have to
    /// differ - that is the whole point of the change.
    #[test]
    fn history_paints_the_lines_with_the_vector_font() {
        let Some(mut vf) = crate::render::text::VectorFont::open(
            crate::render::text::DEFAULT_FONT,
            crate::render::DIALOG_FONT_720,
        ) else {
            return;
        };
        let dirs = [
            std::path::Path::new("/home/lrxin/wa2-native/data/bg_cn"),
            std::path::Path::new("/home/lrxin/wa2-native/data/bg"),
        ];
        let header = crate::load_background(&dirs, "sys_00032.tga");
        let scroll = crate::load_background(&dirs, "sys_00030.tga");
        let scene = crate::render::Surface::new(1280, 720, [80, 120, 160, 255]);
        let lines = vec![
            "春希：不我".to_string(),
            "冬马和纱：白色的季节里".to_string(),
        ];
        let rows = crate::backlog_rows(&lines, Some(&vf));
        assert!(rows.iter().any(|r| r.name), "speaker row is missing");
        let painted = crate::paint_backlog(
            &scene,
            &crate::render::Font::from_cell_map(
                crate::codec::tga::decode_font(
                    &std::fs::read("/home/lrxin/wa2-native/data/font/cn_body80.tga").unwrap(),
                )
                .unwrap(),
                40,
                std::path::Path::new("/home/lrxin/wa2-native/data/font/wa2_cell_map_cn.tsv"),
            )
            .unwrap(),
            Some(&mut vf),
            &rows,
            0,
            19,
            header.as_ref(),
            scroll.as_ref(),
            false,
            false,
        );
        // Ink where the lines are, in the text column and below the banner.
        let mut ink = 0usize;
        for y in 52..680 {
            for x in 72..1160 {
                let i = ((y * 1280 + x) * 4) as usize;
                if painted.pixels[i] > 200 {
                    ink += 1;
                }
            }
        }
        assert!(ink > 200, "vector history drew no glyphs ({ink} px)");
        let _ = painted.save_tga("/tmp/wa2btn/backlog_vector.tga");
    }

    #[test]
    fn quick_load_confirm_paints_yes_and_no() {
        let dirs = [
            std::path::Path::new("/home/lrxin/wa2-native/data/bg_cn"),
            std::path::Path::new("/home/lrxin/wa2-native/data/bg"),
        ];
        let (Some(plate), Some(buttons)) = (
            crate::load_background(&dirs, "sys_00100.tga"),
            crate::load_background(&dirs, "sys_00101.tga"),
        ) else {
            // No art tree at that path (e.g. a macOS checkout): nothing to check.
            return;
        };
        let scene = crate::render::Surface::new(1280, 720, [20, 20, 20, 255]);
        let text = crate::render::text::with_ui_font(22, |f| {
            f.line_rgba("要读取快速存档吗？", 22)
        })
        .flatten();
        assert!(text.is_some(), "system font should rasterize the prompt");
        let backdrop = crate::modal_backdrop(&scene, 140);
        let surf = crate::paint_confirm(&backdrop, Some(&plate), Some(&buttons), text.as_ref(), Some(true));
        let (px, py) = crate::centered_plate(plate.width as i32, plate.height as i32);
        let boxes = crate::ask_buttons(px, py, plate.width as i32, plate.height as i32, 1.0);
        let sample = |x: i32, y: i32| {
            let i = ((y as u32 * 1280 + x as u32) * 4) as usize;
            surf.pixels[i] as u16 + surf.pixels[i + 1] as u16 + surf.pixels[i + 2] as u16
        };
        let yes = sample(boxes[0].0 + 80, boxes[0].1 + 20);
        let no = sample(boxes[1].0 + 80, boxes[1].1 + 20);
        assert!(yes > 400, "yes button should be the gold hover row, sum {yes}");
        assert!(no > 400, "no button should still be the idle row, sum {no}");
        assert!(yes != no, "hover should change the yes button");
        let _ = std::fs::create_dir_all("/tmp/wa2btn");
        let _ = surf.save_tga("/tmp/wa2btn/confirm.tga");
    }
}

#[cfg(test)]
mod bg_index_tests {
    use crate::BgIndex;
    use std::path::PathBuf;

    struct TempTree(PathBuf);
    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tree(tag: &str, files: &[&str]) -> TempTree {
        let dir = std::env::temp_dir().join(format!("wa2-bgidx-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for f in files {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        TempTree(dir)
    }

    #[test]
    fn a_missing_variant_falls_back_to_the_background_family() {
        // 3005 asks for id 2067 variant 0; only b206702/b206712 were shipped,
        // and v206700 is an unrelated event CG.
        let d = tree("family", &["b206702.tga", "b206712.tga", "v206700.tga", "tv206700.tga"]);
        let index = BgIndex::build(&[d.0.as_path()]);
        assert_eq!(index.resolve(2067, 0).as_deref(), Some("b206702.tga"));
        assert_eq!(index.resolve(2067, 1).as_deref(), Some("b206702.tga"));
    }

    #[test]
    fn an_event_cg_stands_in_only_when_no_background_was_shipped() {
        let d = tree("cgonly", &["v100100.tga", "v100101.tga"]);
        let index = BgIndex::build(&[d.0.as_path()]);
        assert_eq!(index.resolve(1001, 0).as_deref(), Some("v100100.tga"));
        assert_eq!(index.resolve(1001, 1).as_deref(), Some("v100101.tga"));
        let d2 = tree("cgonly2", &["v100100.tga"]);
        let index2 = BgIndex::build(&[d2.0.as_path()]);
        assert_eq!(index2.resolve(1001, 5).as_deref(), Some("v100100.tga"));
    }

    #[test]
    fn the_exact_background_variant_wins() {
        let d = tree("exact", &["b101701.tga", "b101700.tga", "v101700.tga"]);
        let index = BgIndex::build(&[d.0.as_path()]);
        assert_eq!(index.resolve(1017, 1).as_deref(), Some("b101701.tga"));
        assert_eq!(index.resolve(1017, 0).as_deref(), Some("b101700.tga"));
    }
}

#[cfg(test)]
mod choice_ui_tests {
    use crate::{choice_row_centers, choice_row_layout};

    #[test]
    fn choices_sit_where_the_original_screen_puts_them() {
        assert_eq!(choice_row_centers(1), vec![262]);
        assert_eq!(choice_row_centers(2), vec![193, 331]);
        assert_eq!(choice_row_centers(3), vec![124, 262, 400]);
    }

    #[test]
    fn a_branch_of_four_still_fits_above_the_dialogue_box() {
        let rows = choice_row_layout(2, 720);
        assert_eq!(rows[0].top, 157);
        assert_eq!(rows[1].top, 295);
        assert_eq!(rows[0].height, 72);
        let rows = choice_row_layout(4, 720);
        assert!(rows.first().unwrap().top >= 24, "{rows:?}");
        let last = rows.last().unwrap();
        assert!(last.top + last.height <= 476, "{rows:?}");
    }

    #[test]
    fn rows_scale_with_the_window() {
        let rows = choice_row_layout(2, 1080);
        assert_eq!(rows[0].height, 108);
        assert_eq!(rows[0].top, 236);
        assert_eq!(rows[1].top, 443);
    }
}

#[cfg(test)]
mod font_sync_tests {
    use crate::game_font_is_stale;

    /// A run started from the title has to rebuild the Game instance exactly
    /// when the profile id moved; Japanese mode (no vector dialogue font)
    /// never does.
    #[test]
    fn game_font_rebuild_is_keyed_on_the_profile_id() {
        assert!(game_font_is_stale(true, 0, 1234));
        assert!(game_font_is_stale(true, 1234, 0));
        assert!(!game_font_is_stale(true, 1234, 1234));
        assert!(!game_font_is_stale(false, 0, 1234));
    }
}

/// ACT170 (ambient light) and ACT172 (explicit filter) share one curve slot.
/// The 86 scenes that use both keep their filter behaviour only if the
/// explicit filter wins; the per-background `.amp` override outranks the
/// preset but not the filter.
#[cfg(test)]
mod ambient_tint_tests {
    use crate::{ambient_preset_name, background_override_name, effect_name, effective_tint_name};

    fn s(name: &str) -> Option<String> {
        Some(name.to_string())
    }

    #[test]
    fn nothing_set_means_no_tint() {
        assert_eq!(effective_tint_name(&None, &None, &None), None);
    }

    #[test]
    fn the_preset_stands_alone() {
        assert_eq!(
            effective_tint_name(&None, &None, &s("night.amp")),
            s("night.amp")
        );
    }

    #[test]
    fn the_background_override_outranks_the_preset() {
        assert_eq!(
            effective_tint_name(&None, &s("b205001.amp"), &s("night.amp")),
            s("b205001.amp")
        );
    }

    #[test]
    fn the_explicit_filter_outranks_everything() {
        assert_eq!(
            effective_tint_name(&s("sepia.AMP"), &s("b205001.amp"), &s("night.amp")),
            s("sepia.AMP")
        );
        assert_eq!(
            effective_tint_name(&s("sepia.AMP"), &None, &s("night.amp")),
            s("sepia.AMP")
        );
    }

    /// ACT172 with an empty token clears the filter, so the ambient light
    /// shows through again.
    #[test]
    fn clearing_the_filter_falls_back_to_the_ambient_light() {
        assert_eq!(
            effective_tint_name(&None, &s("b202501.amp"), &None),
            s("b202501.amp")
        );
    }

    #[test]
    fn background_override_uses_variant_and_preset_as_separate_digits() {
        assert_eq!(background_override_name(2025, 0, 1), "b202501.amp");
        assert_eq!(background_override_name(2025, 1, 1), "b202511.amp");
        assert_eq!(background_override_name(2096, 1, 1), "b209611.amp");
        assert_eq!(background_override_name(2, 0, 0), "b000200.amp");
    }

    #[test]
    fn high_bit_background_modes_select_f0_effects() {
        assert_eq!(effect_name(0x80).as_deref(), Some("f0000.bmp"));
        assert_eq!(effect_name(0xA1).as_deref(), Some("f0033.bmp"));
        assert_eq!(effect_name(1), None);
        assert_eq!(ambient_preset_name(3).as_deref(), Some("indoor.amp"));
    }
}

#[cfg(test)]
mod modal_scale_tests {
    use crate::*;

    /// 1x stays 1x: the modal must not move on a 1280x720 canvas.
    #[test]
    fn the_prompt_keeps_its_position_at_one_x() {
        assert_eq!(ui_scale(1280, 720), 1.0);
        assert_eq!(centered_plate(544, 216), centered_plate_in(1280, 720, 544, 216));
        let (px, py) = centered_plate(544, 216);
        let boxes = ask_buttons(px, py, 544, 216, 1.0);
        assert_eq!((boxes[0].2, boxes[0].3), (160, 40));
        assert_eq!(boxes[1].0, boxes[0].0 + 160);
    }

    /// A Retina frame gets a plate at twice the logical size, centred, with the
    /// yes/no row still inside it - this is what the movie skip prompt needs.
    #[test]
    fn the_prompt_follows_a_retina_canvas() {
        let k = ui_scale(2560, 1440);
        assert_eq!(k, 2.0);
        let (pw, ph) = ((544.0 * k).round() as i32, (216.0 * k).round() as i32);
        let (px, py) = centered_plate_in(2560, 1440, pw, ph);
        assert_eq!((px, py), (2560 / 2 - 544, 1440 / 2 - 216));
        assert!(px > 0 && py > 0 && px + pw < 2560 && py + ph < 1440);
        let boxes = ask_buttons(px, py, pw, ph, k);
        assert_eq!((boxes[0].2, boxes[0].3), (320, 80));
        assert_eq!(boxes[1].0, boxes[0].0 + 320);
        for b in boxes {
            assert!(b.0 >= px && b.0 + b.2 <= px + pw, "button leaves the plate");
            assert!(b.1 >= py && b.1 + b.3 <= py + ph, "button leaves the plate");
        }
    }
}

#[cfg(test)]
mod ctrl_skip_tests {
    use crate::ctrl_skip;

    /// One press, one start: the level being polled again must not report a
    /// second transition, or the caller would stop the voice twice.
    #[test]
    fn holding_control_starts_a_skip_once() {
        let (mut prev, mut skip, mut by) = (false, false, false);
        assert_eq!(ctrl_skip(true, &mut prev, &mut skip, &mut by), Some(true));
        assert!(skip && by);
        for _ in 0..50 {
            assert_eq!(ctrl_skip(true, &mut prev, &mut skip, &mut by), None);
        }
        assert!(skip);
    }

    /// Letting go ends the skip the key started.
    #[test]
    fn releasing_control_ends_the_skip_it_started() {
        let (mut prev, mut skip, mut by) = (false, false, false);
        ctrl_skip(true, &mut prev, &mut skip, &mut by);
        assert_eq!(ctrl_skip(false, &mut prev, &mut skip, &mut by), Some(false));
        assert!(!skip && !by);
        // Idle polls are silent.
        assert_eq!(ctrl_skip(false, &mut prev, &mut skip, &mut by), None);
    }

    /// 快進 clicked first, key pressed afterwards: the button owns the skip, so
    /// the release must leave it running.
    #[test]
    fn the_button_keeps_ownership() {
        let (mut prev, mut by) = (false, false);
        let mut skip = true;
        assert_eq!(ctrl_skip(true, &mut prev, &mut skip, &mut by), None);
        assert!(skip && !by);
        assert_eq!(ctrl_skip(false, &mut prev, &mut skip, &mut by), None);
        assert!(skip, "releasing the key turned off a button skip");
    }

    /// A click (or an unseen line) stops skip while the key is still down, and
    /// holding on must not take it back - otherwise fast-forward could not be
    /// escaped without letting go of the key first.
    #[test]
    fn a_click_during_the_hold_wins() {
        let (mut prev, mut skip, mut by) = (false, false, false);
        ctrl_skip(true, &mut prev, &mut skip, &mut by);
        // The player clicks: the scene loop clears skip itself.
        skip = false;
        for _ in 0..20 {
            assert_eq!(ctrl_skip(true, &mut prev, &mut skip, &mut by), None);
        }
        assert!(!skip, "the held key restarted a skip the player stopped");
        assert_eq!(ctrl_skip(false, &mut prev, &mut skip, &mut by), None);
    }
}

#[cfg(test)]
mod scene_cache_tests {
    use crate::art_id;
    use crate::codec::tga::TgaImage;

    fn picture(seed: u8) -> TgaImage {
        let mut rgba = vec![0u8; 64 * 64 * 4];
        for (i, p) in rgba.iter_mut().enumerate() {
            *p = (i as u8).wrapping_mul(31).wrapping_add(seed);
        }
        TgaImage { width: 64, height: 64, rgba }
    }

    /// The scene cache compares identities, and an identity has to follow the
    /// picture. Keyed on the allocation address alone, a background that was
    /// dropped (every scripted cut to black drops one) freed its buffer, the
    /// next decode of the same size landed on that address, and the cache
    /// happily kept serving the black frame - the picture stayed dark until
    /// something else invalidated it.
    #[test]
    fn a_new_picture_at_the_old_address_is_not_the_old_picture() {
        let mut bg = picture(0);
        let before = art_id(&bg);
        for p in bg.rgba.iter_mut().step_by(3) {
            *p ^= 0xff;
        }
        assert_ne!(
            art_id(&bg),
            before,
            "the same buffer holding a different picture kept its cache key"
        );
    }

    /// The identity is stable for a picture that is not touched, or every frame
    /// would recompose the scene.
    #[test]
    fn an_untouched_picture_keeps_its_id() {
        let bg = picture(7);
        assert_eq!(art_id(&bg), art_id(&bg));
        let same_content = picture(7);
        assert_ne!(art_id(&bg), art_id(&same_content), "two buffers are two pictures");
    }
}

#[cfg(test)]
mod bgm_variant_tests {
    use crate::bgm_file;
    use std::path::PathBuf;

    struct TempTree(PathBuf);
    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tree(tag: &str, files: &[&str]) -> TempTree {
        let dir = std::env::temp_dir().join(format!("wa2-bgm-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for f in files {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        TempTree(dir)
    }

    fn name(dir: &TempTree, id: i32) -> Option<String> {
        bgm_file(&dir.0, id)
            .and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned()))
    }

    /// `_A` is an excerpt for most tracks (BGM_001_A is 9 s against B's 86 s),
    /// and the scripts carry no variant flag, so picking it by default looped a
    /// few seconds of music forever. The full arrangement has to win.
    #[test]
    fn the_full_arrangement_wins_over_the_short_one() {
        let d = tree("both", &["BGM_001_A.OGG", "BGM_001_B.OGG"]);
        assert_eq!(name(&d, 1).as_deref(), Some("BGM_001_B.OGG"));
    }

    #[test]
    fn an_unsuffixed_track_is_used_as_shipped() {
        let d = tree("plain", &["BGM_004.OGG"]);
        assert_eq!(name(&d, 4).as_deref(), Some("BGM_004.OGG"));
    }

    #[test]
    fn a_track_with_only_the_excerpt_still_plays() {
        let d = tree("aonly", &["BGM_050_A.OGG"]);
        assert_eq!(name(&d, 50).as_deref(), Some("BGM_050_A.OGG"));
    }

    #[test]
    fn a_missing_track_is_reported_as_missing() {
        let d = tree("none", &["BGM_004.OGG"]);
        assert_eq!(name(&d, 7), None);
    }
}

#[cfg(test)]
mod bg_request_tests {
    use crate::{bg_request, BgRequest};

    /// `0` is the only id that cuts to black.
    #[test]
    fn zero_cuts_to_black() {
        assert_eq!(bg_request(0), BgRequest::Black);
    }

    /// The negative sentinel stands in front of every `spr Enter`. Reading it
    /// as "black" is what painted the cast on a solid black fill whenever a
    /// second person joined the shot.
    #[test]
    fn the_negative_sentinel_keeps_the_picture() {
        assert_eq!(bg_request(-2), BgRequest::Keep);
    }

    #[test]
    fn a_real_id_wants_a_picture() {
        assert_eq!(bg_request(1007), BgRequest::Picture);
        assert_eq!(bg_request(1), BgRequest::Picture);
    }
}

#[cfg(test)]
mod surface_pos_tests {
    use crate::sdl2::fit_surface_pos;

    /// A 16:9 window is a 1:1 pass-through of the pointer.
    #[test]
    fn a_16_by_9_window_maps_the_pointer_directly() {
        // Window 1280x720, surface 1280x720: no letterbox, identity map.
        assert_eq!(fit_surface_pos(640, 360, 1280, 720, 1280, 720), (640, 360));
        assert_eq!(fit_surface_pos(0, 0, 1280, 720, 1280, 720), (0, 0));
        assert_eq!(fit_surface_pos(1279, 719, 1280, 720, 1280, 720), (1279, 719));
    }

    /// A Retina window reports the same *logical* size, so the pointer (also
    /// reported in points) maps identically whether the backing store is 1x or
    /// 2x — the DPI ratio never enters `fit_surface_pos`, only the logical
    /// window size and the surface's layout size.
    #[test]
    fn a_retina_16_by_9_window_is_still_identity() {
        assert_eq!(fit_surface_pos(640, 360, 1280, 720, 1280, 720), (640, 360));
        assert_eq!(fit_surface_pos(0, 0, 1280, 720, 1280, 720), (0, 0));
        assert_eq!(fit_surface_pos(1279, 719, 1280, 720, 1280, 720), (1279, 719));
    }

    /// A taller-than-16:9 window letterboxes vertically; a click must subtract
    /// the top bar before scaling, or the choices land too low.
    #[test]
    fn a_16_by_10_window_removes_the_vertical_letterbox() {
        // 1280x800 window holding a 1280x720 surface: the surface is fit to
        // width, so scale = 1.0 and the picture occupies y in [40, 760].
        // Clicking at y=40 (the top of the art) must map to surface y=0.
        assert_eq!(fit_surface_pos(640, 40, 1280, 720, 1280, 800), (640, 0));
        // The centre of the art (y=400) is the centre of the surface (360).
        assert_eq!(fit_surface_pos(640, 400, 1280, 720, 1280, 800), (640, 360));
        // The bottom of the art (y=760) is the bottom of the surface (719).
        assert_eq!(fit_surface_pos(640, 760, 1280, 720, 1280, 800), (640, 719));
    }

    /// A wider-than-16:9 window letterboxes horizontally.
    #[test]
    fn an_ultrawide_window_removes_the_horizontal_letterbox() {
        // 1600x720 window holding a 1280x720 surface: fit to height, scale=1.0,
        // picture occupies x in [160, 1440]. Clicking x=160 maps to surface x=0.
        assert_eq!(fit_surface_pos(160, 360, 1280, 720, 1600, 720), (0, 360));
        assert_eq!(fit_surface_pos(800, 360, 1280, 720, 1600, 720), (640, 360));
        assert_eq!(fit_surface_pos(1440, 360, 1280, 720, 1600, 720), (1279, 360));
    }

    /// A click out on the letterbox still clamps to the surface edge instead
    /// of producing a coordinate outside [0, sw/sh).
    #[test]
    fn a_click_on_the_letterbox_clamps_to_the_edge() {
        // Above the art: y < 40 in the 1280x800 example -> clamps to y=0.
        assert_eq!(fit_surface_pos(640, 0, 1280, 720, 1280, 800), (640, 0));
        // Below the art: y > 760 -> clamps to y=719.
        assert_eq!(fit_surface_pos(640, 799, 1280, 720, 1280, 800), (640, 719));
    }
}
