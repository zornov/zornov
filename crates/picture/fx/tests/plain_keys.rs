// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Keying a plain white, black or green backdrop with thin details kept
//! (a hair, a key), and finding the backdrop colour from the border.

use studio_fx::effects::{self, EffectContext};
use studio_fx::image::{Image, srgb_to_linear};
use studio_fx::keycolour::detect_plain_background;
use studio_fx::params::ParamValue;

const W: usize = 80;
const H: usize = 60;

/// A product on a plain backdrop: a solid mid-tone block, a one-pixel hair
/// drawn half over the backdrop, and a pale key (brass-grey) that is only
/// a little darker than a white sheet.
struct Scene {
    img: Image,
    block: (usize, usize),
    hair: (usize, usize),
    key: (usize, usize),
}

fn scene(bg: [f32; 3], block: [f32; 3], hair: [f32; 3], key: [f32; 3]) -> Scene {
    let img = Image::from_fn(W, H, |x, y| {
        let mut c = bg;
        if (10..30).contains(&x) && (10..30).contains(&y) {
            c = block;
        }
        // A hair: one pixel wide, half covered, so it is the average of
        // hair and backdrop.
        if x == 50 && (8..40).contains(&y) {
            c = std::array::from_fn(|i| 0.5 * hair[i] + 0.5 * bg[i]);
        }
        if (60..72).contains(&x) && (20..32).contains(&y) {
            c = key;
        }
        [c[0], c[1], c[2], 1.0]
    });
    Scene { img, block: (20, 20), hair: (50, 20), key: (66, 26) }
}

fn run(id: &str, img: &Image, set: &[(&str, ParamValue)]) -> Image {
    let e = effects::find(id).expect("effect");
    let mut p = e.defaults();
    if e.params.iter().any(|d| d.id == "feather") {
        p = p.with("feather", ParamValue::Float(0.0));
    }
    for (k, v) in set {
        p = p.with(k, v.clone());
    }
    e.apply(&EffectContext::still(), img, &p)
}

fn colour(rgb: [f32; 3]) -> ParamValue {
    ParamValue::Colour([rgb[0], rgb[1], rgb[2], 1.0])
}

fn straight(p: [f32; 4]) -> [f32; 3] {
    [p[0] / p[3].max(1e-6), p[1] / p[3].max(1e-6), p[2] / p[3].max(1e-6)]
}

#[test]
fn white_backdrop_goes_and_thin_details_stay() {
    let grey = srgb_to_linear(0.62);
    let s = scene([1.0; 3], [0.1, 0.25, 0.5], [0.02; 3], [grey; 3]);
    // A white sheet is clean: a tight tolerance keeps pale details.
    let out = run("chroma_key", &s.img, &[("key", colour([1.0; 3])), ("tolerance", ParamValue::Float(0.08)), ("softness", ParamValue::Float(0.3))]);
    assert!(out.get(2, 2)[3] < 0.01, "white kept: {:?}", out.get(2, 2));
    assert!(out.get(75, 55)[3] < 0.01);
    assert!(out.get(s.block.0, s.block.1)[3] > 0.99, "subject lost");
    assert!(out.get(s.key.0, s.key.1)[3] > 0.9, "pale key lost: {:?}", out.get(s.key.0, s.key.1));
    // The hair is half over white: kept as a half-transparent dark line,
    // not erased and not left as a grey fringe.
    let hair = out.get(s.hair.0, s.hair.1);
    assert!(hair[3] > 0.35 && hair[3] < 0.95, "hair alpha {}", hair[3]);
    let fg = straight(hair);
    assert!(fg[0] < 0.3 && fg[1] < 0.3 && fg[2] < 0.3, "hair colour carries white: {fg:?}");
}

#[test]
fn black_backdrop_goes_and_a_thin_bright_line_stays() {
    let s = scene([0.0; 3], [0.7, 0.6, 0.2], [0.9; 3], [srgb_to_linear(0.35); 3]);
    let out = run("chroma_key", &s.img, &[("key", colour([0.0; 3])), ("tolerance", ParamValue::Float(0.08)), ("softness", ParamValue::Float(0.3))]);
    assert!(out.get(2, 2)[3] < 0.01);
    assert!(out.get(s.block.0, s.block.1)[3] > 0.99);
    assert!(out.get(s.key.0, s.key.1)[3] > 0.9, "dim detail lost");
    let hair = out.get(s.hair.0, s.hair.1);
    assert!(hair[3] > 0.3, "hair alpha {}", hair[3]);
}

#[test]
fn a_mid_grey_backdrop_works_too() {
    let g = srgb_to_linear(0.5);
    let s = scene([g; 3], [0.9, 0.2, 0.1], [0.0; 3], [srgb_to_linear(0.9); 3]);
    let out = run("chroma_key", &s.img, &[("key", colour([g; 3])), ("tolerance", ParamValue::Float(0.08)), ("softness", ParamValue::Float(0.3))]);
    assert!(out.get(2, 2)[3] < 0.01);
    assert!(out.get(s.block.0, s.block.1)[3] > 0.99);
    assert!(out.get(s.key.0, s.key.1)[3] > 0.9);
}

#[test]
fn green_screen_still_keys_with_thin_detail_kept() {
    let green = [0.05, 0.55, 0.1];
    // A golden strand and a grey key: neither is near the screen's colour.
    let s = scene(green, [0.6, 0.3, 0.2], [0.7, 0.5, 0.2], [0.5, 0.45, 0.4]);
    let out = run("chroma_key", &s.img, &[("key", colour(green))]);
    assert!(out.get(2, 2)[3] < 0.01);
    assert!(out.get(s.block.0, s.block.1)[3] > 0.99);
    assert!(out.get(s.key.0, s.key.1)[3] > 0.99);
    assert!(out.get(s.hair.0, s.hair.1)[3] > 0.3, "hair alpha {}", out.get(s.hair.0, s.hair.1)[3]);
}

#[test]
fn luma_key_removes_white_and_black_with_clean_edges() {
    let s = scene([1.0; 3], [0.1, 0.25, 0.5], [0.02; 3], [0.3; 3]);
    let out = run("luma_key", &s.img, &[("mode", ParamValue::Choice(1)), ("threshold", ParamValue::Float(0.1)), ("softness", ParamValue::Float(0.35))]);
    assert!(out.get(2, 2)[3] < 0.01, "white kept: {:?}", out.get(2, 2));
    assert!(out.get(s.block.0, s.block.1)[3] > 0.99);
    let hair = out.get(s.hair.0, s.hair.1);
    assert!(hair[3] > 0.3, "hair alpha {}", hair[3]);
    assert!(straight(hair)[0] < 0.25, "white fringe: {:?}", straight(hair));
    let s = scene([0.0; 3], [0.7, 0.6, 0.2], [0.9; 3], [0.4; 3]);
    let out = run("luma_key", &s.img, &[("threshold", ParamValue::Float(0.1)), ("softness", ParamValue::Float(0.1))]);
    assert!(out.get(2, 2)[3] < 0.01);
    assert!(out.get(s.block.0, s.block.1)[3] > 0.99);
}

#[test]
fn the_backdrop_is_found_from_the_border() {
    for (bg, name) in [([1.0; 3], "white"), ([0.0; 3], "black"), ([0.05, 0.55, 0.1], "green"), ([srgb_to_linear(0.5); 3], "grey")] {
        let s = scene(bg, [0.5, 0.2, 0.1], [0.0; 3], [0.3; 3]);
        let found = detect_plain_background(&s.img).unwrap_or_else(|| panic!("{name}: no backdrop"));
        assert_eq!(found.name(), name, "{found:?}");
        assert!(found.share > 0.95);
        assert_eq!(found.neutral, name != "green");
        // The suggested numbers key the scene.
        let out = run("chroma_key", &s.img, &[("key", colour(found.colour)), ("tolerance", ParamValue::Float(found.tolerance())), ("softness", ParamValue::Float(found.softness()))]);
        assert!(out.get(2, 2)[3] < 0.02, "{name}: backdrop kept");
        assert!(out.get(s.block.0, s.block.1)[3] > 0.98, "{name}: subject lost");
    }
}
