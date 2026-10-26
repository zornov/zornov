// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The place generators: every kind in every style builds, comes out as a
//! modest number of named parts with real geometry, and can be drawn.
//! Set `STUDIO_PLACES_OUT` to a folder to also write pictures of them.

use studio_mesh::{Mesh, V3, cross, dot, normalise, sub};
use studio_modeling::places::{self, Spec, Style};

/// A small software shaded view of a mesh (for checking by eye).
pub fn shade(mesh: &Mesh, yaw: f32, pitch: f32, w: usize, h: usize, dist_k: f32) -> Vec<u8> {
    let (lo, hi) = mesh.bounds();
    let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
    let r = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt() / 2.0;
    let (ya, pa) = (yaw.to_radians(), pitch.to_radians());
    // The camera sits at -z looking +z when yaw = 0; positive pitch looks down.
    let dir = [ya.sin() * pa.cos(), -pa.sin(), -ya.cos() * pa.cos()];
    let dist = r * dist_k;
    let eye = [c[0] + dir[0] * dist, c[1] + dir[1] * dist, c[2] + dir[2] * dist];
    let f = normalise(sub(c, eye));
    let up = [0.0, -1.0, 0.0];
    let rt = normalise(cross(f, up));
    let u = cross(rt, f);
    let focal = h as f32 * 0.5 / (20.0f32.to_radians()).tan();
    let mut depth = vec![f32::INFINITY; w * h];
    let mut img = vec![0.0f32; w * h * 3];
    // Sky gradient.
    for y in 0..h {
        let t = y as f32 / h as f32;
        for x in 0..w {
            let k = (y * w + x) * 3;
            img[k] = 0.62 + 0.2 * (1.0 - t);
            img[k + 1] = 0.72 + 0.15 * (1.0 - t);
            img[k + 2] = 0.86 + 0.08 * (1.0 - t);
        }
    }
    let proj = |p: V3| -> (f32, f32, f32) {
        let v = sub(p, eye);
        let z = dot(v, f);
        (w as f32 / 2.0 + dot(v, rt) / z * focal, h as f32 / 2.0 - dot(v, u) / z * focal, z)
    };
    let light = normalise([-0.5, -0.8, -0.45]);
    let tri_mats = mesh.tri_materials();
    let order: Vec<usize> = {
        let mut o: Vec<usize> = (0..mesh.triangles()).collect();
        // Opaque first, glass last.
        o.sort_by_key(|&t| mesh.materials.get(tri_mats[t] as usize).is_some_and(|m| m.base_colour[3] < 0.9) as u8);
        o
    };
    let srgb = |v: f32| -> f32 { v.max(0.0).min(1.0).powf(1.0 / 2.2) };
    for t in order {
        let idx = mesh.tri(t);
        let p: Vec<V3> = idx.iter().map(|&i| mesh.positions[i]).collect();
        let n = normalise(cross(sub(p[1], p[0]), sub(p[2], p[0])));
        let mat = &mesh.materials[tri_mats[t] as usize];
        let facing = dot(n, normalise(sub(eye, p[0])));
        let n = if facing < 0.0 { [-n[0], -n[1], -n[2]] } else { n };
        let diff = (dot(n, [-light[0], -light[1], -light[2]])).max(0.0);
        let shade = 0.34 + 0.72 * diff;
        let base = mat.base_colour;
        let col = [0, 1, 2].map(|i| (base[i] * shade + mat.emissive[i]).min(1.6));
        let alpha = base[3];
        let q: Vec<(f32, f32, f32)> = p.iter().map(|&v| proj(v)).collect();
        if q.iter().any(|v| v.2 < 1.0) {
            continue;
        }
        let (minx, maxx) = (q.iter().map(|v| v.0).fold(f32::INFINITY, f32::min).floor().max(0.0), q.iter().map(|v| v.0).fold(f32::NEG_INFINITY, f32::max).ceil().min(w as f32 - 1.0));
        let (miny, maxy) = (q.iter().map(|v| v.1).fold(f32::INFINITY, f32::min).floor().max(0.0), q.iter().map(|v| v.1).fold(f32::NEG_INFINITY, f32::max).ceil().min(h as f32 - 1.0));
        let area = (q[1].0 - q[0].0) * (q[2].1 - q[0].1) - (q[2].0 - q[0].0) * (q[1].1 - q[0].1);
        if area.abs() < 1e-6 {
            continue;
        }
        for y in miny as usize..=maxy as usize {
            for x in minx as usize..=maxx as usize {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let w0 = ((q[1].0 - px) * (q[2].1 - py) - (q[2].0 - px) * (q[1].1 - py)) / area;
                let w1 = ((q[2].0 - px) * (q[0].1 - py) - (q[0].0 - px) * (q[2].1 - py)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = w0 * q[0].2 + w1 * q[1].2 + w2 * q[2].2;
                let k = y * w + x;
                if z >= depth[k] {
                    continue;
                }
                if alpha >= 0.9 {
                    depth[k] = z;
                    for i in 0..3 {
                        img[k * 3 + i] = col[i];
                    }
                } else {
                    for i in 0..3 {
                        img[k * 3 + i] = img[k * 3 + i] * (1.0 - alpha * 1.6).max(0.0) + col[i] * alpha * 1.6;
                    }
                }
            }
        }
    }
    let mut out = Vec::with_capacity(w * h * 4);
    for k in 0..w * h {
        for i in 0..3 {
            out.push((srgb(img[k * 3 + i]) * 255.0) as u8);
        }
        out.push(255);
    }
    out
}

fn write_png(path: &std::path::Path, w: usize, h: usize, rgba: Vec<u8>) {
    let img = studio_media::RgbaImage { width: w as u32, height: h as u32, pixels: rgba };
    std::fs::write(path, studio_media::png::encode(&img)).unwrap();
}

fn out_dir() -> Option<std::path::PathBuf> {
    let d = std::env::var("STUDIO_PLACES_OUT").ok()?;
    let p = std::path::PathBuf::from(d);
    std::fs::create_dir_all(&p).ok()?;
    Some(p)
}

fn spec(kind: &str, style: Style) -> Spec {
    Spec::new(kind, style)
}

#[test]
fn every_kind_builds_in_every_style() {
    let out = out_dir();
    for style in Style::all() {
        for (kind, _) in places::KINDS {
            let t0 = std::time::Instant::now();
            let s = spec(kind, style);
            let (asset, mesh) = places::build_mesh(&s).unwrap_or_else(|e| panic!("{kind} {}: {e}", style.name()));
            assert!(mesh.triangles() > 40, "{kind} {}: {} triangles", style.name(), mesh.triangles());
            assert!(mesh.triangles() < 600_000, "{kind} {}: {} triangles", style.name(), mesh.triangles());
            assert!(asset.parts.len() >= 2 && asset.parts.len() <= 80, "{kind} {}: {} parts", style.name(), asset.parts.len());
            let (lo, hi) = mesh.bounds();
            assert!(lo.iter().chain(hi.iter()).all(|v| v.is_finite()), "{kind} {}", style.name());
            eprintln!("{:>9} {:<13} {:>7} tris {:>3} parts {:>5} ms", kind, style.name(), mesh.triangles(), asset.parts.len(), t0.elapsed().as_millis());
            if let Some(dir) = &out {
                let (yaw, pitch, k) = match *kind {
                    "street" => (0.0, 14.0, 1.25),
                    "interior" => (0.0, 12.0, 1.35),
                    "bridge" => (25.0, 12.0, 1.6),
                    _ => (28.0, 12.0, 1.9),
                };
                write_png(&dir.join(format!("{kind}-{}.png", style.name())), 960, 600, shade(&mesh, yaw, pitch, 960, 600, k));
            }
        }
    }
}

/// What the 3D critic measures on a model: it must read as designed.
fn craft(mesh: &Mesh) -> studio_quality::hero::Craft {
    studio_quality::hero::measure(mesh)
}

#[test]
fn places_read_as_designed_to_the_critic() {
    use studio_quality::hero;
    let mut bad = Vec::new();
    for style in Style::all() {
        for kind in ["building", "facade", "street", "interior", "bridge", "fence", "stairs"] {
            let (_, mesh) = places::build_mesh(&spec(kind, style)).unwrap();
            let c = craft(&mesh);
            eprintln!("{:>9} {:<13} tris {:>6} shells {:>5} mats {:>3} sharp {:.3} facet {:.3} outline {:.2} default {}", kind, style.name(), c.triangles, c.shells, c.materials, c.sharp_share, c.facet_share, c.outline, c.default_materials);
            let primitive = c.shells <= 2 && c.materials <= 1 && c.bones == 0 && c.plain_outline() && c.triangles < 150_000;
            let razor = c.sharp_share >= 0.06 && c.triangles < 30_000;
            let faceted = c.facet_share >= 0.30 && c.triangles < 3_000;
            let stock = c.materials > 0 && c.default_materials == c.materials;
            if primitive || razor || faceted || stock {
                bad.push(format!("{kind} {}: primitive {primitive} razor {razor} ({:.3}, limit 0.06) faceted {faceted} stock {stock}", style.name(), c.sharp_share));
            }
        }
    }
    let _ = hero::SHARP;
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
