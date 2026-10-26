// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Mesh gradients: each Coons patch is diced into a fine grid of small
//! triangles whose corner colours blend across them, drawn into an image
//! that the shape then uses as its paint (so the shape's own edges keep
//! exact anti-aliasing).

use kurbo::{Affine, Point};

use crate::paint::MeshGradient;
use crate::prepare::IRect;

pub(crate) fn rasterize(mesh: &MeshGradient, m: Affine, region: IRect) -> Vec<[f32; 4]> {
    let (w, h) = (region.width() as usize, region.height() as usize);
    let mut px = vec![[0.0f32; 4]; w * h];
    for patch in &mesh.patches {
        let dev: Vec<Point> = patch.points.iter().map(|&p| m * p).collect();
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in &dev {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        if x1 < region.x0 as f64 || y1 < region.y0 as f64 || x0 > region.x1 as f64 || y0 > region.y1 as f64 {
            continue;
        }
        // About two pixels per grid cell keeps the curved edges and the
        // colour blend smooth.
        let n = (((x1 - x0).max(y1 - y0) / 2.0).ceil() as usize).clamp(2, 256);
        let mut grid: Vec<(Point, [f32; 4])> = Vec::with_capacity((n + 1) * (n + 1));
        for j in 0..=n {
            for i in 0..=n {
                let (u, v) = (i as f64 / n as f64, j as f64 / n as f64);
                grid.push((m * patch.eval(u, v), patch.color(u as f32, v as f32)));
            }
        }
        let at = |i: usize, j: usize| grid[j * (n + 1) + i];
        for j in 0..n {
            for i in 0..n {
                let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
                triangle(&mut px, region, a, b, c);
                triangle(&mut px, region, a, c, d);
            }
        }
    }
    px
}

/// Fills pixels whose centres lie in the triangle (edges included, so
/// neighbouring triangles leave no gaps).
fn triangle(px: &mut [[f32; 4]], r: IRect, a: (Point, [f32; 4]), b: (Point, [f32; 4]), c: (Point, [f32; 4])) {
    let (pa, pb, pc) = (a.0, b.0, c.0);
    let area = (pb - pa).cross(pc - pa);
    if area.abs() < 1e-12 {
        return;
    }
    let (a, b, c) = if area < 0.0 { (a, c, b) } else { (a, b, c) };
    let (pa, pb, pc) = (a.0, b.0, c.0);
    let area = area.abs();
    let minx = (pa.x.min(pb.x).min(pc.x) - 0.5).ceil().max(r.x0 as f64) as i32;
    let maxx = (pa.x.max(pb.x).max(pc.x) - 0.5).floor().min(r.x1 as f64 - 1.0) as i32;
    let miny = (pa.y.min(pb.y).min(pc.y) - 0.5).ceil().max(r.y0 as f64) as i32;
    let maxy = (pa.y.max(pb.y).max(pc.y) - 0.5).floor().min(r.y1 as f64 - 1.0) as i32;
    let w = r.width();
    let edge = |p: Point, q: Point, s: Point| (q - p).cross(s - p);
    for y in miny..=maxy {
        for x in minx..=maxx {
            let s = Point::new(x as f64 + 0.5, y as f64 + 0.5);
            let (w0, w1, w2) = (edge(pb, pc, s), edge(pc, pa, s), edge(pa, pb, s));
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let (l0, l1, l2) = ((w0 / area) as f32, (w1 / area) as f32, (w2 / area) as f32);
            let col = [0, 1, 2, 3].map(|k| a.1[k] * l0 + b.1[k] * l1 + c.1[k] * l2);
            let i = ((y - r.y0) * w + (x - r.x0)) as usize;
            // Later patches cover earlier ones; shared edges are simply written twice.
            px[i] = col;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Color;
    use crate::paint::CoonsPatch;
    use kurbo::Rect;

    #[test]
    fn rect_patch_blends_corners() {
        let colors = [Color::rgb(1.0, 0.0, 0.0), Color::rgb(0.0, 1.0, 0.0), Color::rgb(0.0, 0.0, 1.0), Color::rgb(1.0, 1.0, 1.0)];
        let mesh = MeshGradient { patches: vec![CoonsPatch::rect(Rect::new(0.0, 0.0, 100.0, 100.0), colors)], transform: Affine::IDENTITY };
        let px = rasterize(&mesh, Affine::IDENTITY, IRect::wh(100, 100));
        let tl = px[0];
        assert!(tl[0] > 0.95 && tl[1] < 0.05);
        let mid = px[50 * 100 + 50];
        assert!((mid[0] - 0.5).abs() < 0.03 && (mid[1] - 0.5).abs() < 0.03 && (mid[2] - 0.5).abs() < 0.03, "{mid:?}");
        // Every pixel covered once, fully opaque.
        assert!(px.iter().all(|p| (p[3] - 1.0).abs() < 1e-4));
    }
}
