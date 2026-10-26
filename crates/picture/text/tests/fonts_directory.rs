// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The font directory and chosen fonts, on the three font files the repo
//! carries (Inter, Manrope, JetBrains Mono), so they run anywhere.

use std::path::Path;
use studio_text::*;

fn library() -> FontLibrary {
    let mut lib = FontLibrary::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for p in ["crates/paint/pixel-text/assets/fonts/Inter-Regular.ttf", "crates/paint/pixel-text/assets/fonts/Inter-SemiBold.ttf", "crates/paint/pixel-text/assets/fonts/JetBrainsMono-Regular.ttf", "app/assets/fonts/Manrope-Medium.ttf", "app/assets/fonts/Manrope-Bold.ttf"] {
        lib.load_file(&root.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
    }
    lib
}

#[test]
fn lists_families_with_weights_and_category() {
    let dir = library().directory();
    let names: Vec<&str> = dir.iter().map(|e| e.family.as_str()).collect();
    assert_eq!(names.len(), 3, "{names:?}");
    let inter = dir.iter().find(|e| e.family == "Inter").unwrap();
    assert_eq!(inter.weights, vec![400, 600]);
    assert_eq!(inter.category, "sans");
    let mono = dir.iter().find(|e| e.family.contains("JetBrains")).unwrap();
    assert_eq!(mono.category, "mono", "{:?}", mono.tags);
}

#[test]
fn searches_by_name_category_and_tag() {
    let lib = library();
    assert_eq!(lib.find_fonts("manrope", None, 5).first().map(|h| h.entry.family.as_str()), Some("Manrope"));
    let mono = lib.find_fonts("", Some("monospace"), 5);
    assert_eq!(mono.len(), 1);
    assert!(mono[0].entry.family.contains("JetBrains"));
    assert_eq!(lib.find_fonts("code font", None, 5).len(), 1);
    // A bold family is found by "bold"; one with no italic is not found by it.
    let bold: Vec<String> = lib.find_fonts("bold", None, 5).into_iter().map(|h| h.entry.family).collect();
    assert!(bold.contains(&"Manrope".to_owned()) && bold.contains(&"Inter".to_owned()), "{bold:?}");
    assert!(lib.find_fonts("italic", None, 5).is_empty());
    // Nothing is called art deco here: the search says so rather than guess.
    assert!(lib.find_fonts("art deco", None, 5).is_empty());
    assert!(lib.category_counts().iter().any(|(c, n)| *c == "mono" && *n == 1));
}

fn ink(img: &Image) -> Vec<f32> {
    img.pixels.iter().map(|p| 1.0 - (p[0] + p[1] + p[2]) / 3.0).collect()
}

#[test]
fn a_chosen_family_changes_what_is_drawn() {
    let engine = TextEngine::new(library());
    let sample = "Hamburgefonts 123";
    let draw = |family: &str, weight: u16, italic: bool| engine.specimen(&FontRequest::new(family, weight, italic), sample, 480, 120);
    let (inter, a) = draw("Inter", 400, false);
    let (manrope, b) = draw("Manrope", 400, false);
    let (mono, c) = draw("JetBrains Mono", 400, false);
    assert!(a.exact && b.exact && c.exact);
    assert_eq!(a.family, "Inter");
    assert_eq!(b.family, "Manrope");
    assert!(c.family.contains("JetBrains"));
    let diff = |x: &Image, y: &Image| ink(x).iter().zip(ink(y)).map(|(p, q)| (p - q).abs()).sum::<f32>();
    assert!(diff(&inter, &manrope) > 20.0, "Inter and Manrope drew the same");
    assert!(diff(&inter, &mono) > 20.0, "Inter and the mono drew the same");
    // The words are on the card in every case.
    for img in [&inter, &manrope, &mono] {
        assert!(ink(img).iter().sum::<f32>() > 30.0, "nothing drawn");
    }
}

#[test]
fn weight_and_style_are_honoured_or_reported() {
    let engine = TextEngine::new(library());
    let (light, l) = engine.specimen(&FontRequest::new("Inter", 400, false), "Weight", 400, 100);
    let (heavy, h) = engine.specimen(&FontRequest::new("Inter", 600, false), "Weight", 400, 100);
    assert_eq!((l.weight, h.weight), (400, 600));
    let sum = |i: &Image| ink(i).iter().sum::<f32>();
    assert!(sum(&heavy) > sum(&light) * 1.03, "{} vs {}", sum(&heavy), sum(&light));
    // No italic face: the request is answered with a slanted copy and says so.
    let (_, it) = engine.specimen(&FontRequest::new("Inter", 400, true), "Italic", 300, 100);
    assert!(it.synthetic_italic && it.exact);
    // An unknown family is answered by a fallback, flagged as such.
    let (_, none) = engine.specimen(&FontRequest::new("No Such Family 9", 400, false), "x", 100, 50);
    assert!(!none.exact);
    assert!(engine.fonts.similar_families("Manrop", 3).contains(&"Manrope".to_owned()));
}

#[test]
fn a_run_with_its_own_font_draws_in_it() {
    let engine = TextEngine::new(library());
    let canvas = Canvas::new(600, 120);
    let base = TextStyle::new(FontRequest::new("Inter", 400, false), 300.0, Fill::solid(Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }));
    let plain = engine.layout(&TextBlock::plain("Mix mono"), &base, &canvas);
    let mixed = TextBlock::spans(vec![
        Span::plain("Mix "),
        Span::styled("mono", SpanStyle { font: Some(FontRequest::new("JetBrains Mono", 400, false)), ..Default::default() }),
    ]);
    let l = engine.layout(&mixed, &base, &canvas);
    // Same letters, a different face for the second word: a different width.
    assert!((l.bounds.w - plain.bounds.w).abs() > 1.0, "{} vs {}", l.bounds.w, plain.bounds.w);
    let faces: std::collections::HashSet<usize> = l.glyphs.iter().map(|g| g.font).collect();
    assert!(faces.len() >= 2, "one face for both runs: {faces:?}");
}
