// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `cargo bench -p studio-vector-raster`: a busy illustration (2,000
//! shapes: curves, stars, gradients, dashed and variable strokes, effect
//! layers) on a 1920×1080 artboard, drawn at 1080p, 4K and 8K by the CPU
//! and GPU renderers, and for scale by resvg's CPU renderer.

use std::time::Instant;

use studio_vector_raster::kurbo::{Affine, BezPath, Circle, Point, Rect, Shape};
use studio_vector_raster::*;

struct Rng(u64);

impl Rng {
    fn f(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn colour(r: &mut Rng) -> Color {
    Color::rgba(r.f() as f32, r.f() as f32, r.f() as f32, 0.6 + 0.4 * r.f() as f32)
}

/// The scene, and the same art as SVG for the other renderer.
fn art() -> (Scene, String) {
    let mut rng = Rng(42);
    let mut s = Scene::new();
    let mut svg = String::from(r##"<svg xmlns="http://www.w3.org/2000/svg" width="1920" height="1080"><rect width="1920" height="1080" fill="#f4f1ea"/>"##);
    s.fill(Rect::new(0.0, 0.0, 1920.0, 1080.0).to_path(0.0), Affine::IDENTITY, FillRule::NonZero, Paint::solid(Color::rgb(0.957, 0.945, 0.918)));
    for i in 0..2000 {
        let (x, y) = (rng.f() * 1920.0, rng.f() * 1080.0);
        let size = 8.0 + rng.f() * 60.0;
        let mut p = BezPath::new();
        let n = 3 + (rng.f() * 6.0) as usize;
        for k in 0..n {
            let a = k as f64 / n as f64 * std::f64::consts::TAU;
            let rr = size * (0.5 + 0.5 * rng.f());
            let q = Point::new(x + rr * a.cos(), y + rr * a.sin());
            let c = Point::new(x + rr * 1.3 * (a + 0.3).cos(), y + rr * 1.3 * (a + 0.3).sin());
            if k == 0 { p.move_to(q) } else { p.quad_to(c, q) }
        }
        p.close_path();
        let c1 = colour(&mut rng);
        let c2 = colour(&mut rng);
        let d = p.to_svg();
        if i % 3 == 0 {
            let g = Gradient::linear((x - size, y), (x + size, y), vec![Stop::new(0.0, c1), Stop::new(1.0, c2)]);
            s.fill(p.clone(), Affine::IDENTITY, FillRule::NonZero, Paint::gradient(g));
            svg.push_str(&format!(
                r#"<linearGradient id="g{i}" gradientUnits="userSpaceOnUse" x1="{}" y1="{y}" x2="{}" y2="{y}"><stop offset="0" stop-color="{}" stop-opacity="{}"/><stop offset="1" stop-color="{}" stop-opacity="{}"/></linearGradient><path d="{d}" fill="url(#g{i})"/>"#,
                x - size,
                x + size,
                &c1.with_alpha(1.0).to_hex(),
                c1.a,
                &c2.with_alpha(1.0).to_hex(),
                c2.a
            ));
        } else {
            s.fill(p.clone(), Affine::IDENTITY, FillRule::EvenOdd, Paint::solid(c1));
            svg.push_str(&format!(r#"<path d="{d}" fill="{}" fill-opacity="{}" fill-rule="evenodd"/>"#, c1.with_alpha(1.0).to_hex(), c1.a));
        }
        if i % 4 == 0 {
            let mut st = StrokeStyle::new(1.0 + rng.f() * 4.0).with_join(Join::Round);
            if i % 8 == 0 {
                st = st.with_dashes(vec![6.0, 4.0], 0.0);
            }
            svg.push_str(&format!(
                r#"<path d="{d}" fill="none" stroke="{}" stroke-width="{}" stroke-linejoin="round"{}/>"#,
                c2.with_alpha(1.0).to_hex(),
                st.width,
                if i % 8 == 0 { r#" stroke-dasharray="6 4""# } else { "" }
            ));
            s.stroke(p, Affine::IDENTITY, st, Paint::solid(c2.with_alpha(1.0)));
        }
    }
    // A few effect layers.
    for k in 0..6 {
        let mut a = Scene::new();
        let c = Circle::new((200.0 + k as f64 * 300.0, 540.0), 90.0);
        a.fill(c.to_path(0.01), Affine::IDENTITY, FillRule::NonZero, Paint::solid(Color::rgb(0.9, 0.3, 0.2)));
        s.layer(Layer::new().with_filter(Filter::DropShadow { dx: 8.0, dy: 8.0, sigma: 6.0, color: Color::rgba(0.0, 0.0, 0.0, 0.5), spread: 0.0 }), a);
    }
    svg.push_str("</svg>");
    (s, svg)
}

fn time(label: &str, runs: usize, mut f: impl FnMut()) -> f64 {
    f();
    let t = Instant::now();
    for _ in 0..runs {
        f();
    }
    let ms = t.elapsed().as_secs_f64() * 1000.0 / runs as f64;
    println!("{label:<34} {ms:9.1} ms");
    ms
}

fn main() {
    let (scene, svg) = art();
    println!("2,000-shape illustration, 1920×1080 artboard");
    let sizes = [("1080p", 1.0f64), ("4K", 2.0), ("8K", 4.0)];
    for (name, k) in sizes {
        let o = RenderOptions::new((1920.0 * k) as u32, (1080.0 * k) as u32).with_transform(Affine::scale(k));
        time(&format!("CPU {name}"), if k > 2.0 { 1 } else { 3 }, || {
            std::hint::black_box(render(&scene, &o));
        });
    }
    #[cfg(feature = "gpu")]
    if let Some(g) = studio_vector_raster::gpu::GpuRaster::new() {
        println!("GPU: {}", g.adapter_name);
        for (name, k) in sizes {
            let o = RenderOptions::new((1920.0 * k) as u32, (1080.0 * k) as u32).with_transform(Affine::scale(k));
            time(&format!("GPU {name} (incl. readback)"), 3, || {
                std::hint::black_box(g.render(&scene, &o).expect("gpu"));
            });
        }
    }
    let tree = usvg::Tree::from_str(&svg, &usvg::Options::default()).expect("svg");
    for (name, k) in sizes {
        let (w, h) = ((1920.0 * k) as u32, (1080.0 * k) as u32);
        time(&format!("resvg (tiny-skia) CPU {name}"), 1, || {
            let mut pm = resvg::tiny_skia::Pixmap::new(w, h).expect("pixmap");
            resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(k as f32, k as f32), &mut pm.as_mut());
            std::hint::black_box(pm);
        });
    }
}
