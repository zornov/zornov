// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reference images: the same SVG art drawn by this rasterizer and by an
//! independent renderer (resvg), compared pixel by pixel at 1× and 3×.
//! Anti-aliasing models differ slightly at edges (exact area here,
//! supersampling there), so the limits allow small edge differences but
//! catch any wrong fill, paint, blend, clip or mask.

mod common;

use common::*;
use studio_vector_raster::kurbo::Affine;
use studio_vector_raster::*;

const CASES: &[(&str, &str)] = &[
    ("shapes", include_str!("art/shapes.svg")),
    ("fill_rules", include_str!("art/fill_rules.svg")),
    ("strokes", include_str!("art/strokes.svg")),
    ("gradients", include_str!("art/gradients.svg")),
    ("groups_and_blends", include_str!("art/groups.svg")),
    ("clip_and_mask", include_str!("art/clip_mask.svg")),
    ("thin_and_tiny", include_str!("art/thin_and_tiny.svg")),
    ("blur_and_pattern", include_str!("art/pattern_blur.svg")),
];

/// Every blend mode over a gradient backdrop.
fn blend_svg() -> String {
    let modes = [
        "normal", "multiply", "screen", "overlay", "darken", "lighten", "color-dodge", "color-burn", "hard-light", "soft-light", "difference", "exclusion", "hue",
        "saturation", "color", "luminosity",
    ];
    let mut s = String::from(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="240" height="240"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#1a365d"/><stop offset="0.5" stop-color="#e53e3e"/><stop offset="1" stop-color="#faf089"/></linearGradient></defs><rect width="240" height="240" fill="url(#g)"/>"##,
    );
    for (i, m) in modes.iter().enumerate() {
        let (x, y) = ((i % 4) * 60 + 30, (i / 4) * 60 + 30);
        s.push_str(&format!(r##"<circle cx="{x}" cy="{y}" r="24" fill="#38a169" fill-opacity="0.85" style="mix-blend-mode:{m}"/>"##));
        s.push_str(&format!(r##"<rect x="{}" y="{}" width="20" height="20" fill="#805ad5" style="mix-blend-mode:{m}"/>"##, x - 4, y - 4));
    }
    s.push_str("</svg>");
    s
}

fn check(name: &str, svg: &str, scale: f64) -> Diff {
    let (scene, w, h) = svg_scene(svg);
    let (w, h) = ((w as f64 * scale).ceil() as u32, (h as f64 * scale).ceil() as u32);
    let mut opts = RenderOptions::new(w, h).with_transform(Affine::scale(scale));
    opts.dither = false;
    let mine = render(&scene, &opts);
    let theirs = resvg_render(svg, scale as f32);
    let dir = out_dir();
    write_png(&dir.join(format!("{name}@{scale}x.png")), &mine);
    write_png(&dir.join(format!("{name}@{scale}x.ref.png")), &theirs);
    let d = diff(&mine, &theirs);
    eprintln!("{name} @{scale}x: {d:?}");
    d
}

#[test]
fn matches_the_reference_renderer() {
    let mut bad = Vec::new();
    let blends = blend_svg();
    let cases = CASES.iter().map(|(n, s)| (*n, *s)).chain([("blend_modes", blends.as_str())]);
    for (name, svg) in cases {
        for scale in [1.0, 3.0] {
            let d = check(name, svg, scale);
            // Edges may differ by the other renderer's supersampling error
            // (and its hairline approximation of sub-pixel strokes); overall
            // the images must agree closely.
            if d.mean > 0.75 || d.off > 0.05 {
                bad.push(format!("{name} @{scale}x: {d:?}"));
            }
        }
    }
    assert!(bad.is_empty(), "differ from the reference:\n{}", bad.join("\n"));
}
