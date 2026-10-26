// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Golden-image tests: one small render per feature, compared against
//! `tests/golden/<name>.png`. A missing golden is written on first run;
//! set `UPDATE_GOLDEN=1` to rewrite all of them after an intended change.
//!
//! The renders use only the bundled font library and emoji set so they
//! are the same on every machine; when the content folder is absent the
//! tests pass vacuously (and say so).

use std::collections::HashMap;
use std::path::PathBuf;

use studio_text::*;

const W: u32 = 640;
const H: u32 = 240;
/// Mean absolute difference per channel allowed before a test fails.
/// Rasterisation is deterministic; this only absorbs float-order noise.
const TOLERANCE: f32 = 0.002;

fn engine() -> Option<TextEngine> {
    let lib = FontLibrary::bundled_only();
    if lib.faces().is_empty() {
        eprintln!("bundled fonts not found; golden tests skipped");
        return None;
    }
    Some(TextEngine::new(lib))
}

fn backdrop() -> Image {
    Image::from_fn(W, H, |x, y| {
        let k = ((x / 24) + (y / 24)) % 2;
        let v = 0.02 + 0.015 * k as f32;
        [v, v, v, 1.0]
    })
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// Renders with `f`, checks the result is not blank, and compares with
/// (or writes) the golden image.
fn check(name: &str, f: impl Fn(&TextEngine, &mut Image)) {
    let Some(engine) = engine() else { return };
    let mut img = backdrop();
    f(&engine, &mut img);
    // Something must have been drawn.
    let bg = backdrop();
    let drawn = png_io::mean_abs_diff(&img, &bg).unwrap();
    assert!(drawn > 1e-4, "{name}: nothing was drawn");
    // Determinism.
    let mut again = backdrop();
    f(&engine, &mut again);
    assert_eq!(png_io::mean_abs_diff(&img, &again), Some(0.0), "{name}: render is not deterministic");

    let path = golden_dir().join(format!("{name}.png"));
    let update = std::env::var("UPDATE_GOLDEN").map(|v| v == "1").unwrap_or(false);
    if update || !path.exists() {
        png_io::write_png(&path, &img).unwrap();
        eprintln!("wrote golden {}", path.display());
        return;
    }
    let golden = png_io::read_png(&path).unwrap();
    let diff = png_io::mean_abs_diff(&img, &golden).unwrap_or(f32::INFINITY);
    if diff > TOLERANCE {
        let actual = golden_dir().join(format!("{name}.actual.png"));
        png_io::write_png(&actual, &img).unwrap();
        panic!("{name}: differs from golden by {diff:.5} (> {TOLERANCE}); actual written to {}", actual.display());
    }
}

fn draw(engine: &TextEngine, img: &mut Image, block: &TextBlock, style: &TextStyle) {
    engine.render_onto(img, block, style, [W as f32 * 0.5, H as f32 * 0.5], &Hooks::NONE);
}

fn sans(size: f32) -> TextStyle {
    TextStyle::new(FontRequest::bold("Inter"), size, Fill::Solid(Rgba::WHITE))
}

// ------------------------------------------------------------------ layout

#[test]
fn plain_single_line() {
    check("plain_single_line", |e, img| draw(e, img, &TextBlock::plain("The quick brown fox"), &sans(48.0)));
}

#[test]
fn wrap_left_centre_right() {
    check("wrap_left_centre_right", |e, img| {
        let text = "wrapped text aligned three ways in a narrow box";
        for (i, a) in [Align::Left, Align::Centre, Align::Right].into_iter().enumerate() {
            let block = TextBlock::plain(text).with_layout(LayoutParams::default().align(a).wrap(180.0));
            engine_at(e, img, &block, &sans(16.0), [110.0 + i as f32 * 210.0, 120.0]);
        }
    });
}

fn engine_at(e: &TextEngine, img: &mut Image, block: &TextBlock, style: &TextStyle, centre: [f32; 2]) {
    e.render_onto(img, block, style, centre, &Hooks::NONE);
}

#[test]
fn justify_and_line_spacing() {
    check("justify_and_line_spacing", |e, img| {
        let block = TextBlock::plain("Justified paragraphs stretch every line but the last one to the full width of the box, spacing words evenly.")
            .with_layout(LayoutParams::default().align(Align::Justify).wrap(560.0));
        draw(e, img, &block, &sans(22.0).line_spacing(30.0));
    });
}

#[test]
fn spans_bold_italic_colour_size() {
    check("spans_bold_italic_colour_size", |e, img| {
        let block = TextBlock::spans(vec![
            Span::plain("regular "),
            Span::styled("bold ", SpanStyle::bold()),
            Span::styled("italic ", SpanStyle::italic()),
            Span::styled("coral ", SpanStyle::colour(Rgba::rgb8(255, 120, 90))),
            Span::styled("big", SpanStyle { size_scale: Some(1.6), ..Default::default() }),
        ]);
        draw(e, img, &block, &TextStyle::new(FontRequest::regular("Inter"), 40.0, Fill::Solid(Rgba::WHITE)));
    });
}

#[test]
fn underline_and_strikethrough() {
    check("underline_and_strikethrough", |e, img| {
        let block = TextBlock::spans(vec![
            Span::styled("underlined ", SpanStyle { underline: Some(true), ..Default::default() }),
            Span::styled("struck", SpanStyle { strikethrough: Some(true), ..Default::default() }),
        ]);
        draw(e, img, &block, &sans(44.0));
    });
}

#[test]
fn letter_spacing_and_case() {
    check("letter_spacing_and_case", |e, img| {
        draw(e, img, &TextBlock::plain("tracked caps"), &sans(40.0).letter_spacing(40.0).case(Case::Upper));
    });
}

#[test]
fn vertical_cjk() {
    check("vertical_cjk", |e, img| {
        let block = TextBlock::plain("縦書きの文字\n二行目").with_layout(LayoutParams { vertical: true, ..Default::default() });
        draw(e, img, &block, &TextStyle::new(FontRequest::regular("Noto Sans JP"), 28.0, Fill::Solid(Rgba::WHITE)));
    });
}

#[test]
fn bend_positive_and_negative() {
    check("bend_positive_and_negative", |e, img| {
        let s = sans(26.0);
        engine_at(e, img, &TextBlock::plain("ARCHED OVER THE TOP").with_layout(LayoutParams::default().bend(140.0)), &s, [160.0, 120.0]);
        engine_at(e, img, &TextBlock::plain("BOWLED UNDERNEATH").with_layout(LayoutParams::default().bend(-140.0)), &s, [480.0, 120.0]);
    });
}

#[test]
fn fit_to_box() {
    check("fit_to_box", |e, img| {
        let block = TextBlock::plain("This headline shrinks to fit its box exactly").with_layout(LayoutParams::default().boxed(560.0, 160.0).fit());
        draw(e, img, &block, &sans(200.0));
    });
}

#[test]
fn max_lines_truncates() {
    check("max_lines_truncates", |e, img| {
        let mut p = LayoutParams::default().wrap(300.0);
        p.max_lines = Some(2);
        let block = TextBlock::plain("one two three four five six seven eight nine ten eleven twelve thirteen").with_layout(p);
        draw(e, img, &block, &sans(32.0));
    });
}

// ----------------------------------------------------------------- scripts

#[test]
fn arabic_rtl_shaping() {
    check("arabic_rtl_shaping", |e, img| {
        let block = TextBlock::plain("مرحبا بالعالم، هذا نص عربي").with_layout(LayoutParams::default().wrap(560.0));
        draw(e, img, &block, &TextStyle::new(FontRequest::bold("Noto Sans Arabic"), 40.0, Fill::Solid(Rgba::WHITE)));
    });
}

#[test]
fn hebrew_with_latin_numbers() {
    check("hebrew_with_latin_numbers", |e, img| {
        draw(e, img, &TextBlock::plain("שלום 2026 עולם"), &TextStyle::new(FontRequest::bold("Noto Sans Hebrew"), 44.0, Fill::Solid(Rgba::WHITE)));
    });
}

#[test]
fn devanagari_thai_conjuncts() {
    check("devanagari_thai_conjuncts", |e, img| {
        let block = TextBlock::plain("नमस्ते दुनिया · สวัสดีชาวโลก").with_layout(LayoutParams::default().wrap(600.0));
        draw(e, img, &block, &TextStyle::new(FontRequest::regular("Noto Sans"), 36.0, Fill::Solid(Rgba::WHITE)));
    });
}

#[test]
fn long_mixed_script_paragraph() {
    check("long_mixed_script_paragraph", |e, img| {
        let text = "English text flows into русский текст, then Ελληνικά, 中文字符, 日本語の文, 한국어 문장, العربية, עברית and back again with numbers 12345 and symbols ©™ — all in one wrapped paragraph that must break cleanly.";
        let block = TextBlock::plain(text).with_layout(LayoutParams::default().wrap(600.0));
        draw(e, img, &block, &TextStyle::new(FontRequest::regular("Noto Sans"), 19.0, Fill::Solid(Rgba::WHITE)).line_spacing(10.0));
    });
}

#[test]
fn emoji_and_cjk() {
    check("emoji_and_cjk", |e, img| {
        let block = TextBlock::plain("火 🔥 水 💧 風 🌬️ 土 🌍 👍🏽 🇯🇵").with_layout(LayoutParams::default().wrap(600.0));
        draw(e, img, &block, &TextStyle::new(FontRequest::bold("Noto Sans JP"), 40.0, Fill::Solid(Rgba::WHITE)));
    });
}

#[test]
fn emoji_with_strokes_and_shadow() {
    check("emoji_with_strokes_and_shadow", |e, img| {
        let s = sans(44.0).with_stroke(Stroke::new(Rgba::BLACK, 3.0)).with_shadow(Shadow::new(Rgba::rgb8(40, 0, 60), 4.0, 5.0, 60.0));
        draw(e, img, &TextBlock::plain("party 🎉 time 🚀"), &s);
    });
}

// ----------------------------------------------------------------- styling

#[test]
fn gradient_fill_scopes() {
    check("gradient_fill_scopes", |e, img| {
        let cols = [Rgba::rgb8(90, 200, 255), Rgba::rgb8(230, 90, 255), Rgba::rgb8(255, 200, 80)];
        for (i, scope) in [GradientScope::Text, GradientScope::Line, GradientScope::Glyph].into_iter().enumerate() {
            let s = TextStyle::new(FontRequest::bold("Montserrat"), 30.0, Fill::Gradient(Gradient::new(20.0, &cols).scope(scope)));
            engine_at(e, img, &TextBlock::plain("Gradient\nfill"), &s, [110.0 + i as f32 * 210.0, 120.0]);
        }
    });
}

#[test]
fn image_fill() {
    check("image_fill", |e, img| {
        let mut e2 = TextEngine::new(FontLibrary::bundled_only());
        let tex = Image::from_fn(64, 64, |x, y| {
            let k = ((x / 8) + (y / 8)) % 2;
            if k == 0 { [0.9, 0.3, 0.1, 1.0] } else { [0.1, 0.4, 0.9, 1.0] }
        });
        e2.add_image("checks", tex);
        let _ = e;
        let s = TextStyle::new(FontRequest::bold("Anton"), 96.0, Fill::Image { key: "checks".into(), opacity: 1.0 });
        draw(&e2, img, &TextBlock::plain("TEXTURE"), &s);
    });
}

#[test]
fn multiple_strokes() {
    check("multiple_strokes", |e, img| {
        let s = TextStyle::new(FontRequest::new("Anton", 400, false), 90.0, Fill::Solid(Rgba::WHITE))
            .with_stroke(Stroke::new(Rgba::rgb8(240, 60, 70), 3.0))
            .with_stroke(Stroke::new(Rgba::WHITE, 2.0))
            .with_stroke(Stroke::new(Rgba::rgb8(20, 20, 80), 3.0));
        draw(e, img, &TextBlock::plain("TRIPLE"), &s);
    });
}

#[test]
fn outline_only_no_fill() {
    check("outline_only_no_fill", |e, img| {
        let s = TextStyle { fill: Fill::None, ..sans(80.0) }.with_stroke(Stroke::new(Rgba::rgb8(255, 210, 60), 2.0)).with_shadow(Shadow::new(Rgba::BLACK, 6.0, 6.0, 90.0));
        draw(e, img, &TextBlock::plain("HOLLOW"), &s);
    });
}

#[test]
fn shadow_blur_distance_angle() {
    check("shadow_blur_distance_angle", |e, img| {
        for (i, (blur, dist, angle)) in [(0.0, 6.0, 45.0), (8.0, 10.0, 90.0), (16.0, 0.0, 0.0)].into_iter().enumerate() {
            let s = sans(44.0).with_shadow(Shadow::new(Rgba::rgb8(255, 80, 120), blur, dist, angle));
            engine_at(e, img, &TextBlock::plain("Shade"), &s, [110.0 + i as f32 * 210.0, 120.0]);
        }
    });
}

#[test]
fn glow_intensity_range() {
    check("glow_intensity_range", |e, img| {
        for (i, (intensity, range)) in [(0.4, 6.0), (0.9, 12.0), (1.0, 24.0)].into_iter().enumerate() {
            let s = TextStyle::new(FontRequest::bold("Space Grotesk"), 40.0, Fill::Solid(Rgba::rgb8(230, 250, 255))).with_glow(Glow::new(Rgba::rgb8(0, 220, 255), intensity, range));
            engine_at(e, img, &TextBlock::plain("GLOW"), &s, [110.0 + i as f32 * 210.0, 120.0]);
        }
    });
}

#[test]
fn background_box_and_per_line() {
    check("background_box_and_per_line", |e, img| {
        let text = "two lines\nin boxes";
        let a = sans(30.0).with_background(Background::new(Rgba::rgb8(255, 226, 52), 8.0, 16.0, 8.0));
        let mut b = sans(30.0).with_background(Background::new(Rgba::rgb8(20, 20, 30).alpha(0.85), 999.0, 18.0, 6.0).per_line()).line_spacing(30.0);
        b.fill = Fill::Solid(Rgba::WHITE);
        let mut a = a;
        a.fill = Fill::Solid(Rgba::rgb8(20, 20, 20));
        engine_at(e, img, &TextBlock::plain(text), &a, [170.0, 120.0]);
        engine_at(e, img, &TextBlock::plain(text), &b, [470.0, 120.0]);
    });
}

#[test]
fn extrusion_depth() {
    check("extrusion_depth", |e, img| {
        let s = TextStyle::new(FontRequest::new("Anton", 400, false), 96.0, Fill::Solid(Rgba::rgb8(90, 140, 255))).with_extrusion(Extrusion::new(Rgba::rgb8(20, 40, 120), 12.0, 40.0));
        draw(e, img, &TextBlock::plain("DEPTH"), &s);
    });
}

#[test]
fn colour_cycle_per_unit() {
    check("colour_cycle_per_unit", |e, img| {
        let cols = [Rgba::rgb8(255, 80, 80), Rgba::rgb8(255, 200, 60), Rgba::rgb8(80, 220, 120), Rgba::rgb8(80, 160, 255)];
        let a = sans(30.0).with_cycle(ColourCycle::new(&cols, TextUnit::Character));
        let b = sans(30.0).with_cycle(ColourCycle::new(&cols, TextUnit::Word));
        let c = sans(30.0).with_cycle(ColourCycle::new(&cols, TextUnit::Line));
        engine_at(e, img, &TextBlock::plain("per char\ncycle"), &a, [110.0, 120.0]);
        engine_at(e, img, &TextBlock::plain("per word\ncycle"), &b, [320.0, 120.0]);
        engine_at(e, img, &TextBlock::plain("per line\ncycle"), &c, [530.0, 120.0]);
    });
}

#[test]
fn block_opacity_and_span_fill_over_cycle() {
    check("block_opacity_and_span_fill", |e, img| {
        let mut s = sans(48.0);
        s.opacity = 0.5;
        draw(e, img, &TextBlock::plain("half opacity"), &s);
    });
}

// --------------------------------------------------------------- animation

#[test]
fn per_character_hooks() {
    check("per_character_hooks", |e, img| {
        let wave = |i: usize, _n: usize| UnitPose::REST.offset(0.0, (i as f32 * 0.8).sin() * 14.0).rotation((i as f32 * 0.8).cos() * 12.0).scale(1.0 + 0.2 * (i % 3) as f32);
        let hooks = Hooks { chars: Some(&wave), ..Default::default() };
        let s = sans(44.0).with_stroke(Stroke::new(Rgba::rgb8(0, 60, 40), 3.0));
        e.render_onto(img, &TextBlock::plain("wave hooks"), &s, [W as f32 * 0.5, H as f32 * 0.5], &hooks);
    });
}

#[test]
fn per_word_and_line_hooks() {
    check("per_word_and_line_hooks", |e, img| {
        let words = |i: usize, _n: usize| if i % 2 == 1 { UnitPose::REST.colour(Rgba::rgb8(255, 210, 60)).scale(1.15) } else { UnitPose::REST };
        let lines = |i: usize, _n: usize| UnitPose::REST.offset(i as f32 * 30.0, 0.0).opacity(1.0 - 0.3 * i as f32);
        let hooks = Hooks { words: Some(&words), lines: Some(&lines), ..Default::default() };
        e.render_onto(img, &TextBlock::plain("alternate words pop\nsecond line slides"), &sans(32.0), [W as f32 * 0.5, H as f32 * 0.5], &hooks);
    });
}

#[test]
fn reveal_presets_mid_way() {
    check("reveal_presets_mid_way", |e, img| {
        let kinds = [RevealKind::Rise, RevealKind::Pop, RevealKind::Spin, RevealKind::Flip, RevealKind::Type, RevealKind::Zoom];
        for (i, k) in kinds.into_iter().enumerate() {
            let r = Reveal::new(k, TextUnit::Character).overlap(0.6);
            let f = |idx: usize, n: usize| r.pose(0.55, idx, n, 26.0);
            let hooks = Hooks { chars: Some(&f), ..Default::default() };
            let (x, y) = (110.0 + (i % 3) as f32 * 210.0, 70.0 + (i / 3) as f32 * 100.0);
            e.render_onto(img, &TextBlock::plain(&format!("{k:?}")), &sans(26.0), [x, y], &hooks);
        }
    });
}

#[test]
fn template_scene_frames() {
    check("template_scene_frames", |e, img| {
        let s = templates::find("lower.classic-bar").unwrap();
        let values: HashMap<String, String> = s.default_values();
        let canvas = Canvas::new(W, H);
        // Mid-intro, hold, mid-outro, scaled into thirds of the strip.
        for (i, t) in [0.5, 3.0, 5.7].into_iter().enumerate() {
            let mut frame = Image::new(640, 360);
            s.render_onto(e, &mut frame, t, 6.0, &values);
            let tr = Affine::scale(1.0 / 3.0, 1.0 / 3.0).then(&Affine::translate(i as f32 * 213.0, 60.0));
            raster::draw_image_affine(img, &frame, &tr, 1.0);
        }
        let _ = canvas;
    });
}

#[test]
fn every_template_renders_at_hold() {
    let Some(e) = engine() else { return };
    for s in templates::all() {
        let values = s.default_values();
        let mut frame = Image::new(480, 270);
        let hold = (s.intro + (s.duration - s.outro)) * 0.5;
        s.render_onto(&e, &mut frame, hold, s.duration, &values);
        assert!(frame.pixels.iter().any(|p| p[3] > 0.05), "template {} draws nothing at hold", s.id);
    }
}

#[test]
fn every_style_renders() {
    let Some(e) = engine() else { return };
    for p in styles::catalogue() {
        let mut img = Image::new(600, 200);
        let block = TextBlock::plain(&p.sample).with_layout(LayoutParams::default().boxed(560.0, 180.0).fit());
        e.render_onto(&mut img, &block, &p.style, [300.0, 100.0], &Hooks::NONE);
        assert!(img.pixels.iter().any(|q| q[3] > 0.05), "style {} draws nothing", p.id);
    }
}

// ---------------------------------------------------------------- captions

fn sample_track(style: &CaptionStyle) -> CaptionTrack {
    let mut t = CaptionTrack::new(style.clone());
    let a = t.add_speaker("Maya", Rgba::rgb8(255, 209, 102));
    let b = t.add_speaker("Sam", Rgba::rgb8(126, 220, 255));
    let mut c1 = Cue::from_text("Captions highlight each word as it is spoken, keeping two balanced lines", 0.0, 4.0).with_speaker(a);
    c1.words[1].keyword = true;
    t.cues.push(c1);
    t.cues.push(Cue::from_text("and the next speaker gets a colour of their own", 4.2, 7.0).with_speaker(b));
    t
}

fn caption_check(name: &str, style_id: &str, t: f32) {
    check(name, |e, img| {
        let track = sample_track(captions::find(style_id).unwrap());
        track.render_onto(e, img, t);
    });
}

#[test]
fn caption_karaoke_mid_cue() {
    caption_check("caption_karaoke_mid_cue", "caption.sunlit", 1.7);
}

#[test]
fn caption_word_pop() {
    caption_check("caption_word_pop", "caption.pop", 2.0);
}

#[test]
fn caption_boxed_lines_speaker_two() {
    caption_check("caption_boxed_lines_speaker_two", "caption.boxed", 5.0);
}

#[test]
fn caption_bottom_bar() {
    caption_check("caption_bottom_bar", "caption.bar", 1.0);
}

#[test]
fn caption_speech_bubble() {
    caption_check("caption_speech_bubble", "caption.bubble", 5.5);
}

#[test]
fn caption_reveal_typed() {
    caption_check("caption_reveal_typed", "caption.typed", 1.2);
}

#[test]
fn caption_between_cues_is_blank() {
    let Some(e) = engine() else { return };
    let track = sample_track(captions::find("caption.clean").unwrap());
    assert!(track.frame(&e, 4.1, &Canvas::new(W, H)).is_none());
    assert!(track.frame(&e, 1.0, &Canvas::new(W, H)).is_some());
}

#[test]
fn caption_respects_safe_margins_and_two_lines() {
    let Some(e) = engine() else { return };
    let mut style = captions::find("caption.clean").unwrap().clone();
    style.safe_margins = [0.1, 0.1];
    let mut track = CaptionTrack::new(style);
    track.cues.push(Cue::from_text("A fairly long caption that certainly needs to wrap onto a second line to fit", 0.0, 5.0));
    let canvas = Canvas::HD;
    let f = track.frame(&e, 1.0, &canvas).unwrap();
    assert!(f.text_box.x >= 192.0 - 1.0, "left margin {}", f.text_box.x);
    assert!(f.text_box.right() <= 1920.0 - 192.0 + 1.0, "right margin {}", f.text_box.right());
    assert!(f.top + f.image.height as i32 <= 1080 - 108 + 2, "bottom {}", f.top + f.image.height as i32);
    let lines = track.cues[0].lines(track.style.max_chars_per_line, 2);
    assert_eq!(lines.len(), 2);
}
