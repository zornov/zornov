// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The acceptance tests named in `40-colour.md`, through the public API.

use std::sync::Arc;

use studio_colour::lut::{Lut3d, parse_cube};
use studio_colour::spline::Spline;
use studio_colour::{Cdl, CubeFile, FrameInfo, Grade, Image, LutSettings, LutSpace, Primaries};

fn test_image() -> Image {
    // Gradients, HDR highlights, negatives, transparent and part-alpha pixels.
    Image::from_fn(128, 72, |x, y| {
        let a = match (x + y) % 7 {
            0 => 0.0,
            1 => 0.37,
            _ => 1.0,
        };
        let r = x as f32 / 32.0;
        let g = y as f32 / 72.0;
        let b = ((x * y) % 13) as f32 / 13.0 - 0.05;
        [r * a, g * a, b * a, a]
    })
}

#[test]
fn identity_at_defaults_is_bit_exact() {
    let src = test_image();
    for frame in [0, 1, 1000] {
        let mut out = src.clone();
        Grade::default().apply(&mut out, FrameInfo { frame });
        for (a, b) in src.pixels.iter().zip(&out.pixels) {
            assert_eq!(a.map(f32::to_bits), b.map(f32::to_bits));
        }
    }
    // The same holds for other working spaces.
    let g = Grade { working: Primaries::BT2020, ..Grade::default() };
    let mut out = src.clone();
    g.apply(&mut out, FrameInfo::default());
    assert_eq!(out, src);
    // And for the per-pixel processor on its own.
    let p = Grade::default().prepare();
    for px in &src.pixels {
        let c = [px[0], px[1], px[2]];
        assert_eq!(p.apply_rgb(c).map(f32::to_bits), c.map(f32::to_bits));
    }
}

#[test]
fn cdl_against_reference_values() {
    // Expected values evaluated from the ASC CDL v1.2 equations in double
    // precision: out = (in·slope + offset)^power, then saturation with
    // Rec. 709 luma weights.
    let cdl = Cdl { slope: [1.2, 1.0, 0.8], offset: [0.05, -0.02, 0.1], power: [1.1, 0.9, 1.0], saturation: 0.8 };
    let out = cdl.apply([0.5, 0.25, 0.75]);
    let expect = [0.572_763_3, 0.287_818_6, 0.634_688_4];
    for i in 0..3 {
        assert!((out[i] - expect[i]).abs() < 2e-6, "{out:?}");
    }
    let cdl = Cdl { slope: [0.9, 1.1, 1.05], offset: [0.02, 0.0, -0.03], power: [0.95, 1.0, 1.2], saturation: 1.3 };
    let out = cdl.apply([0.18, 0.18, 0.18]);
    let expect = [0.200_131_6, 0.199_892_8, 0.085_586_3];
    for i in 0..3 {
        assert!((out[i] - expect[i]).abs() < 2e-6, "{out:?}");
    }
}

#[test]
fn lut_reference_points() {
    // 2-point cube whose red output is r·g: tetrahedral interpolation of a
    // product of two inputs is min(r, g).
    let mut text = String::from("LUT_3D_SIZE 2\n");
    for b in 0..2 {
        for g in 0..2 {
            for r in 0..2 {
                text.push_str(&format!("{} {} {}\n", r * g, g, b));
            }
        }
    }
    let file = parse_cube(&text).unwrap();
    let l = file.lut3d.as_ref().unwrap();
    for (c, expect) in [([0.5, 0.25, 0.75], 0.25), ([0.9, 0.4, 0.2], 0.4), ([0.1, 0.6, 0.9], 0.1)] {
        let o = l.sample(c);
        assert!((o[0] - expect).abs() < 1e-6 && (o[1] - c[1]).abs() < 1e-6 && (o[2] - c[2]).abs() < 1e-6);
    }

    // Identity LUT in a grade: lattice-exact, and close everywhere.
    let lut = Arc::new(CubeFile { title: None, lut1d: None, lut3d: Some(Lut3d::identity(33)) });
    let g = Grade { lut: Some(LutSettings { space: LutSpace::Rec709, ..LutSettings::new(lut) }), ..Grade::default() };
    let p = g.prepare();
    for c in [[0.0, 0.0, 0.0], [0.18, 0.18, 0.18], [0.9, 0.5, 0.1]] {
        let o = p.apply_rgb(c);
        for i in 0..3 {
            assert!((o[i] - c[i]).abs() < 1e-4, "{c:?} → {o:?}");
        }
    }
}

#[test]
fn monotone_curves_never_invert() {
    let mut seed = 7u64;
    let mut rnd = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((seed >> 40) as f32) / (1u64 << 24) as f32
    };
    for _ in 0..300 {
        let n = 2 + (rnd() * 14.0) as usize;
        let pts: Vec<[f32; 2]> = (0..n).map(|_| [rnd() * 1.2 - 0.1, rnd() * 1.2 - 0.1]).collect();
        let s = Spline::monotone(&pts);
        let mut prev = f32::MIN;
        for i in 0..=3000 {
            let x = -0.5 + i as f32 / 1500.0;
            let y = s.eval(x);
            assert!(y >= prev - 1e-6, "{pts:?} inverts at {x}");
            prev = y;
        }
        // Through a full grade, a grey ramp stays ordered.
        let mut g = Grade::default();
        g.curves.master = pts.clone();
        let p = g.prepare();
        let mut prev = f32::MIN;
        for i in 0..=400 {
            let v = i as f32 / 200.0;
            let o = p.apply_rgb([v; 3])[1];
            assert!(o >= prev - 1e-5, "{pts:?} inverts at {v}");
            prev = o;
        }
    }
}
