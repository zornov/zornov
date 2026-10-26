// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Files in and out: a document with paths, live shapes, gradients,
//! strokes, clipping, opacity and blend modes goes out as SVG, PDF and EPS
//! and comes back in, and the re-imported art renders like the original.
//! The exported SVG is also drawn by an independent renderer (resvg) and
//! compared with ours.

use serde_json::json;
use studio_vector_art::build::{self, BuildOptions};
use studio_vector_art::ops::{self, Context};
use studio_vector_art::raster::Pixmap;
use studio_vector_art::{Document, interop};

fn art() -> Document {
    let mut d = Document::new(400.0, 300.0);
    let ctx = Context::default();
    let ops = json!([
        {"op": "rect", "x": 20, "y": 20, "w": 160, "h": 110, "radius": 18, "fill": {"linear": {"from": [20, 20], "to": [180, 130], "stops": ["#ff512f", "#f9d423"]}}},
        {"op": "circle", "cx": 270, "cy": 90, "r": 60, "fill": {"radial": {"center": [270, 90], "radius": 60, "stops": ["#ffffff", "#2b6cb0"]}}, "stroke": "#1a202c", "stroke_width": 6},
        {"op": "star", "cx": 110, "cy": 220, "r": 60, "inner": 26, "points": 5, "fill": "#38a169", "opacity": 0.8},
        {"op": "path", "d": "M220 180 C260 150 360 260 380 200 L380 280 L220 280 Z", "fill": "#805ad5", "blend": "multiply"},
        {"op": "polygon", "cx": 300, "cy": 230, "r": 40, "sides": 6, "fill": "none", "stroke": "#e53e3e", "stroke_width": 8, "join": "round"}
    ]);
    ops::apply_all(&mut d, &ops, &ctx).expect("ops apply");
    d
}

fn render(d: &Document) -> Pixmap {
    build::render_artboard(d, None, 400, 300, &BuildOptions::default())
}

/// Mean absolute channel difference (0..255).
fn diff(a: &Pixmap, b: &Pixmap) -> f64 {
    a.data.iter().zip(&b.data).map(|(x, y)| x.abs_diff(*y) as f64).sum::<f64>() / a.data.len() as f64
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("vector-art");
    std::fs::create_dir_all(&d).ok();
    d.join(name)
}

#[test]
fn svg_round_trip_and_reference() {
    let d = art();
    let original = render(&d);
    let svg = interop::svg::export(&d, None);
    std::fs::write(tmp("art.svg"), &svg).ok();
    let back = interop::svg::import(&svg).expect("our SVG reads back");
    let again = render(&back);
    let e = diff(&original, &again);
    assert!(e < 1.0, "SVG round trip differs by {e}");
    // An independent renderer draws our SVG the same.
    let tree = usvg::Tree::from_str(&svg, &usvg::Options::default()).expect("valid SVG");
    let mut pm = resvg::tiny_skia::Pixmap::new(400, 300).expect("pixmap");
    resvg::render(&tree, resvg::tiny_skia::Transform::default(), &mut pm.as_mut());
    let theirs = Pixmap { width: 400, height: 300, data: pm.data().to_vec() };
    let e = diff(&original, &theirs);
    assert!(e < 1.5, "resvg draws our SVG {e} levels away from us");
}

#[test]
fn pdf_round_trip() {
    let d = art();
    let original = render(&d);
    let pdf = interop::pdf::export(&d, None, &BuildOptions::default());
    std::fs::write(tmp("art.pdf"), &pdf).ok();
    assert!(pdf.starts_with(b"%PDF-1.7"));
    let back = interop::pdf::import(&pdf).expect("our PDF reads back");
    assert_eq!(back.artboards.len(), 1);
    let again = render(&back);
    std::fs::write(tmp("pdf-back.png"), interop::png_bytes(&again)).ok();
    std::fs::write(tmp("original.png"), interop::png_bytes(&original)).ok();
    // Shapes, solid paint, shadings and transparency groups come back.
    let e = diff(&original, &again);
    assert!(e < 4.0, "PDF round trip differs by {e}");
}

#[test]
fn eps_round_trip() {
    let d = art();
    let eps = interop::eps::export(&d, None, &BuildOptions::default());
    std::fs::write(tmp("art.eps"), &eps).ok();
    assert!(eps.starts_with("%!PS-Adobe-3.0 EPSF-3.0"));
    let back = interop::eps::import(eps.as_bytes()).expect("our EPS reads back");
    std::fs::write(tmp("eps-back.png"), interop::png_bytes(&render(&back))).ok();
    let mut paths = 0;
    back.walk(&mut |n| {
        if matches!(n.kind, studio_vector_art::NodeKind::Path { .. }) {
            paths += 1;
        }
    });
    assert!(paths >= 4, "{paths} paths back");
    // Vectors, gradients and the image fallbacks of see-through art all come back.
    let e = diff(&render(&d), &render(&back));
    assert!(e < 3.0, "EPS round trip differs by {e}");
}

#[test]
fn files_open_and_save_by_extension() {
    let d = art();
    for ext in ["svart", "svg", "pdf", "eps", "png"] {
        let p = tmp(&format!("saved.{ext}"));
        interop::save(&d, &p, None, 2.0, &BuildOptions::default()).unwrap_or_else(|e| panic!("{ext}: {e}"));
        let back = interop::open(&p).unwrap_or_else(|e| panic!("{ext}: {e}"));
        assert!(!back.artboards.is_empty(), "{ext}");
    }
    let png = interop::open(&tmp("saved.png")).unwrap();
    assert_eq!(png.artboards[0].rect.width(), 800.0, "PNG saved at 2×");
}

#[test]
fn trace_turns_a_render_back_into_shapes() {
    let d = art();
    let pm = render(&d);
    let img = pm.to_image().expect("image");
    let mut t = Document::new(400.0, 300.0);
    let g = studio_vector_art::edit::trace(&mut t, &img, "16 colors", studio_vector_art::raster::kurbo::Rect::new(0.0, 0.0, 400.0, 300.0), None).expect("trace");
    let shapes = t.find(g).and_then(|n| n.children()).map(Vec::len).unwrap_or(0);
    assert!(shapes >= 5, "{shapes} shapes");
    let traced = render(&t);
    let e = diff(&pm, &traced);
    assert!(e < 6.0, "the trace looks {e} levels off");
}
