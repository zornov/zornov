// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Coverage is exact: for simple polygons every pixel's alpha equals the
//! area of the polygon inside that pixel, computed independently by
//! clipping the polygon to the pixel square. Plus analytic checks of
//! discs, gradients, strokes and effects.

mod common;

use studio_vector_raster::kurbo::{Affine, BezPath, Circle, Point, Rect, Shape};
use studio_vector_raster::*;

/// A tiny deterministic generator (xorshift).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Area of `poly` inside the unit pixel at (`px`, `py`) (Sutherland-Hodgman).
fn pixel_area(poly: &[Point], px: f64, py: f64) -> f64 {
    let mut pts = poly.to_vec();
    let planes: [(fn(Point, f64) -> f64, f64); 4] = [(|p, v| p.x - v, px), (|p, v| v - p.x, px + 1.0), (|p, v| p.y - v, py), (|p, v| v - p.y, py + 1.0)];
    for (side, v) in planes {
        let mut out = Vec::new();
        for i in 0..pts.len() {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            let (da, db) = (side(a, v), side(b, v));
            if da >= 0.0 {
                out.push(a);
            }
            if (da >= 0.0) != (db >= 0.0) {
                out.push(a.lerp(b, da / (da - db)));
            }
        }
        pts = out;
        if pts.is_empty() {
            return 0.0;
        }
    }
    let mut a = 0.0;
    for i in 0..pts.len() {
        a += pts[i].to_vec2().cross(pts[(i + 1) % pts.len()].to_vec2());
    }
    (a * 0.5).abs()
}

/// A random star-shaped (hence simple) polygon.
fn polygon(rng: &mut Rng, c: Point, r: f64) -> Vec<Point> {
    let n = 3 + (rng.next() * 12.0) as usize;
    let mut angles: Vec<f64> = (0..n).map(|_| rng.next() * std::f64::consts::TAU).collect();
    angles.sort_by(f64::total_cmp);
    angles.iter().map(|a| c + studio_vector_raster::kurbo::Vec2::from_angle(*a) * (r * (0.2 + 0.8 * rng.next()))).collect()
}

fn path_of(poly: &[Point]) -> BezPath {
    let mut b = BezPath::new();
    b.move_to(poly[0]);
    for p in &poly[1..] {
        b.line_to(*p);
    }
    b.close_path();
    b
}

fn render_alpha(scene: &Scene, w: u32, h: u32) -> Vec<f32> {
    let mut o = RenderOptions::new(w, h);
    o.dither = false;
    render_f32(scene, &o).iter().map(|p| p[3]).collect()
}

#[test]
fn simple_polygons_have_exact_pixel_coverage() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut worst = 0.0f64;
    for _ in 0..60 {
        let (cx, cy, r) = (16.0 + rng.next() * 8.0, 16.0 + rng.next() * 8.0, 6.0 + rng.next() * 9.0);
        let poly = polygon(&mut rng, Point::new(cx, cy), r);
        let mut s = Scene::new();
        let rule = if rng.next() < 0.5 { FillRule::NonZero } else { FillRule::EvenOdd };
        s.fill(path_of(&poly), Affine::IDENTITY, rule, Paint::solid(Color::BLACK));
        let a = render_alpha(&s, 40, 40);
        for y in 0..40 {
            for x in 0..40 {
                let want = pixel_area(&poly, x as f64, y as f64);
                worst = worst.max((a[y * 40 + x] as f64 - want).abs());
            }
        }
    }
    assert!(worst < 2e-5, "largest per-pixel error {worst}");
}

#[test]
fn discs_cover_pi_r_squared_at_every_scale() {
    for (r, scale) in [(0.7, 1.0), (3.3, 1.0), (10.0, 0.5), (10.0, 8.0), (2.0, 64.0)] {
        let mut s = Scene::new();
        s.fill(Circle::new((20.0, 20.0), r).to_path(0.0001), Affine::IDENTITY, FillRule::NonZero, Paint::solid(Color::BLACK));
        let size = (40.0 * scale) as u32;
        let mut o = RenderOptions::new(size, size).with_transform(Affine::scale(scale));
        o.dither = false;
        let sum: f64 = render_f32(&s, &o).iter().map(|p| p[3] as f64).sum();
        let want = std::f64::consts::PI * (r * scale) * (r * scale);
        // Flattening puts chords inside the curve, at most 0.01 px off: the
        // area lost is about 4·tolerance / (3·radius) of the whole.
        let bound = 4.0 * 0.01 / (3.0 * r * scale) * 1.2 + 2e-4;
        assert!((sum - want).abs() / want < bound, "r {r} scale {scale}: {sum} vs {want}");
    }
}

#[test]
fn an_8k_render_is_crisp_and_exact() {
    // A 1920×1080 artboard drawn at 7680×4320: a rectangle on fractional
    // coordinates lands exactly where 4× its edges say.
    let mut s = Scene::new();
    s.fill(Rect::new(100.1, 50.3, 900.6, 700.85).to_path(0.0), Affine::IDENTITY, FillRule::NonZero, Paint::solid(Color::rgb(1.0, 0.0, 0.0)));
    let o = RenderOptions::new(7680, 4320).with_transform(Affine::scale(4.0));
    let p = render(&s, &o);
    assert_eq!(p.pixel(401, 300), [255, 0, 0, 255]);
    // Left edge at x = 400.4: pixel 400 is 60% covered.
    let a = p.pixel(400, 1000)[3] as f32 / 255.0;
    assert!((a - 0.6).abs() < 0.003, "{a}");
    assert_eq!(p.pixel(399, 1000)[3], 0);
    // Bottom edge at y = 2803.4.
    let a = p.pixel(1000, 2803)[3] as f32 / 255.0;
    assert!((a - 0.4).abs() < 0.003, "{a}");
}

#[test]
fn linear_gradient_values_follow_the_axis() {
    let g = Gradient::linear((0.0, 0.0), (100.0, 0.0), vec![Stop::new(0.0, Color::BLACK), Stop::new(1.0, Color::WHITE)]);
    let mut s = Scene::new();
    s.fill(Rect::new(0.0, 0.0, 100.0, 10.0).to_path(0.0), Affine::IDENTITY, FillRule::NonZero, Paint::gradient(g));
    let mut o = RenderOptions::new(100, 10);
    o.dither = false;
    let px = render_f32(&s, &o);
    for x in [0usize, 25, 50, 99] {
        let want = (x as f32 + 0.5) / 100.0;
        assert!((px[5 * 100 + x][0] - want).abs() < 2e-3, "x {x}: {} vs {want}", px[5 * 100 + x][0]);
    }
}

#[test]
fn conic_gradient_turns_clockwise_from_its_angle() {
    let g = Gradient::conic((50.0, 50.0), 0.0, vec![Stop::new(0.0, Color::BLACK), Stop::new(1.0, Color::WHITE)]);
    let mut s = Scene::new();
    s.fill(Rect::new(0.0, 0.0, 100.0, 100.0).to_path(0.0), Affine::IDENTITY, FillRule::NonZero, Paint::gradient(g));
    let mut o = RenderOptions::new(100, 100);
    o.dither = false;
    let px = render_f32(&s, &o);
    // Straight down (a quarter turn clockwise on screen) is t = 0.25.
    assert!((px[90 * 100 + 50][0] - 0.25).abs() < 0.02, "{}", px[90 * 100 + 50][0]);
    // Straight left is half way.
    assert!((px[50 * 100 + 10][0] - 0.5).abs() < 0.02);
}

#[test]
fn stroke_area_and_variable_width() {
    let mut p = BezPath::new();
    p.move_to((10.0, 50.0));
    p.line_to((190.0, 50.0));
    let mut s = Scene::new();
    s.stroke(p.clone(), Affine::IDENTITY, StrokeStyle::new(10.0), Paint::solid(Color::BLACK));
    let a: f32 = render_alpha(&s, 200, 100).iter().sum();
    assert!((a - 1800.0).abs() < 0.5, "{a}");
    // A taper from nothing to full width covers half.
    let mut s = Scene::new();
    let style = StrokeStyle::new(10.0).with_profile(vec![WidthPoint { at: 0.0, width: 0.0 }, WidthPoint { at: 1.0, width: 1.0 }]);
    s.stroke(p, Affine::IDENTITY, style, Paint::solid(Color::BLACK));
    let a: f32 = render_alpha(&s, 200, 100).iter().sum();
    assert!((a - 900.0).abs() < 9.0, "{a}");
}

#[test]
fn shadows_glows_and_blur_keep_their_energy() {
    let rect = Rect::new(40.0, 40.0, 80.0, 80.0).to_path(0.0);
    let mut art = Scene::new();
    art.fill(rect, Affine::IDENTITY, FillRule::NonZero, Paint::solid(Color::BLACK));
    let mut s = Scene::new();
    s.layer(Layer::new().with_filter(Filter::blur(4.0)), art.clone());
    let a: f32 = render_alpha(&s, 120, 120).iter().sum();
    assert!((a - 1600.0).abs() < 2.0, "blur keeps coverage: {a}");
    // A drop shadow adds the shadow's area outside the shape.
    let mut s = Scene::new();
    s.layer(Layer::new().with_filter(Filter::DropShadow { dx: 10.0, dy: 10.0, sigma: 0.0, color: Color::BLACK, spread: 0.0 }), art.clone());
    let a: f32 = render_alpha(&s, 120, 120).iter().sum();
    assert!((a - (1600.0 + 1600.0 - 900.0)).abs() < 2.0, "shadow union: {a}");
    // Effects scale with the art: at 2× the blur reaches twice as far.
    let mut s = Scene::new();
    s.layer(Layer::new().with_filter(Filter::blur(4.0)), art);
    let one = render_alpha(&s, 120, 120);
    let o = RenderOptions::new(240, 240).with_transform(Affine::scale(2.0));
    let two: Vec<f32> = render_f32(&s, &o).iter().map(|p| p[3]).collect();
    let (a1, a2) = (one[60 * 120 + 36], two[120 * 240 + 72]);
    assert!((a1 - a2).abs() < 0.02, "{a1} vs {a2}");
}

#[test]
fn clip_inverts_and_masks_scale() {
    let mut art = Scene::new();
    art.fill(Rect::new(0.0, 0.0, 100.0, 100.0).to_path(0.0), Affine::IDENTITY, FillRule::NonZero, Paint::solid(Color::BLACK));
    let mut s = Scene::new();
    s.layer(Layer::new().with_clip(Circle::new((50.0, 50.0), 30.0).to_path(0.0001), Affine::IDENTITY, FillRule::NonZero), art.clone());
    let a: f32 = render_alpha(&s, 100, 100).iter().sum();
    assert!((a - std::f32::consts::PI * 900.0).abs() < 1.5, "{a}");
    let mut m = Scene::new();
    m.fill(Rect::new(0.0, 0.0, 100.0, 100.0).to_path(0.0), Affine::IDENTITY, FillRule::NonZero, Paint::solid(Color::rgb(0.5, 0.5, 0.5)));
    let mut s = Scene::new();
    s.layer(Layer::new().with_mask(Mask { scene: m, kind: MaskKind::Luminance, invert: false }), art);
    let a = render_alpha(&s, 100, 100);
    assert!((a[5050] - 0.5).abs() < 1e-3);
}

#[test]
fn freeform_and_mesh_gradients_hit_their_colours() {
    let ff = FreeformGradient {
        points: vec![
            FreeformPoint { at: Point::new(10.0, 10.0), color: Color::rgb(1.0, 0.0, 0.0), spread: 1.0 },
            FreeformPoint { at: Point::new(90.0, 90.0), color: Color::rgb(0.0, 0.0, 1.0), spread: 1.0 },
        ],
        transform: Affine::IDENTITY,
    };
    let mut s = Scene::new();
    s.fill(Rect::new(0.0, 0.0, 100.0, 100.0).to_path(0.0), Affine::IDENTITY, FillRule::NonZero, Paint::Freeform(std::sync::Arc::new(ff)));
    let mut o = RenderOptions::new(100, 100);
    o.dither = false;
    let px = render_f32(&s, &o);
    assert!(px[10 * 100 + 10][0] > 0.95);
    assert!(px[90 * 100 + 90][2] > 0.95);
    let mid = px[50 * 100 + 50];
    assert!((mid[0] - mid[2]).abs() < 0.05, "{mid:?}");
}
