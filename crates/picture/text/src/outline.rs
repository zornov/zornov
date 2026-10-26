// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Text as outlines: every glyph's contours, placed as the layout places
//! them, flattened to polygons. 3D text extrudes these; shape layers can
//! use them as paths.

use swash::zeno::{Command, Vector};

use crate::engine::TextEngine;
use crate::geom::Affine;
use crate::style::{Canvas, TextBlock, TextStyle};

/// One glyph's closed contours (outer outlines and holes, as the font
/// draws them), in canvas units from the centre of the text box, y down.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphOutline {
    pub char_index: usize,
    pub contours: Vec<Vec<[f32; 2]>>,
}

fn flatten(cmds: &[Command], tol: f32) -> Vec<Vec<[f32; 2]>> {
    let mut out: Vec<Vec<[f32; 2]>> = Vec::new();
    let mut cur: Vec<[f32; 2]> = Vec::new();
    let mut pen = [0.0f32; 2];
    let v = |p: Vector| [p.x, p.y];
    let steps = |len: f32| ((len / tol.max(1e-3)).sqrt().ceil() as usize).clamp(2, 32);
    for c in cmds {
        match *c {
            Command::MoveTo(p) => {
                if cur.len() >= 3 {
                    out.push(std::mem::take(&mut cur));
                }
                cur.clear();
                pen = v(p);
                cur.push(pen);
            }
            Command::LineTo(p) => {
                pen = v(p);
                cur.push(pen);
            }
            Command::QuadTo(a, p) => {
                let (a, p) = (v(a), v(p));
                let n = steps(((a[0] - pen[0]).abs() + (a[1] - pen[1]).abs() + (p[0] - a[0]).abs() + (p[1] - a[1]).abs()) * 0.5);
                for k in 1..=n {
                    let t = k as f32 / n as f32;
                    let u = 1.0 - t;
                    cur.push([u * u * pen[0] + 2.0 * u * t * a[0] + t * t * p[0], u * u * pen[1] + 2.0 * u * t * a[1] + t * t * p[1]]);
                }
                pen = p;
            }
            Command::CurveTo(a, b, p) => {
                let (a, b, p) = (v(a), v(b), v(p));
                let n = steps(((a[0] - pen[0]).abs() + (a[1] - pen[1]).abs() + (b[0] - a[0]).abs() + (b[1] - a[1]).abs() + (p[0] - b[0]).abs() + (p[1] - b[1]).abs()) * 0.5);
                for k in 1..=n {
                    let t = k as f32 / n as f32;
                    let u = 1.0 - t;
                    let (c0, c1, c2, c3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                    cur.push([c0 * pen[0] + c1 * a[0] + c2 * b[0] + c3 * p[0], c0 * pen[1] + c1 * a[1] + c2 * b[1] + c3 * p[1]]);
                }
                pen = p;
            }
            Command::Close => {
                if cur.len() >= 3 {
                    out.push(std::mem::take(&mut cur));
                }
                cur.clear();
            }
        }
    }
    if cur.len() >= 3 {
        out.push(cur);
    }
    // No repeated closing point.
    for c in &mut out {
        if c.len() > 3 && c.first() == c.last() {
            c.pop();
        }
    }
    out
}

impl TextEngine {
    /// The text's glyph outlines (see [`GlyphOutline`]) on `canvas`.
    pub fn outlines(&self, block: &TextBlock, style: &TextStyle, canvas: &Canvas) -> Vec<GlyphOutline> {
        let l = self.layout(block, style, canvas);
        let ppu = canvas.px_per_unit().max(1e-6);
        let (cx, cy) = (l.bounds.w * 0.5, l.bounds.h * 0.5);
        let mut r = self.rasteriser();
        let mut out = Vec::new();
        for g in &l.glyphs {
            if g.is_space || g.emoji {
                continue;
            }
            let path = r.path(&l.fonts[g.font], g.glyph, g.size_px);
            if path.is_empty() {
                continue;
            }
            let base = if g.rotation.abs() > 1e-4 { Affine::rotate(g.rotation).then(&Affine::translate(g.x, g.y)) } else { Affine::translate(g.x, g.y) };
            let placed = path.transformed(&base);
            let contours = flatten(&placed.commands, 0.25)
                .into_iter()
                .map(|c| c.into_iter().map(|p| [(p[0] - cx) / ppu, (p[1] - cy) / ppu]).collect())
                .collect();
            out.push(GlyphOutline { char_index: g.char_index, contours });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::FontLibrary;

    #[test]
    fn letters_come_out_as_closed_contours_around_the_centre() {
        let lib = FontLibrary::bundled_only();
        if lib.faces().is_empty() {
            return;
        }
        let e = TextEngine::new(lib);
        let o = e.outlines(&TextBlock::plain("OHO"), &TextStyle::default(), &Canvas::HD);
        assert_eq!(o.len(), 3);
        // An O has an outer contour and a hole.
        assert!(o[0].contours.len() >= 2, "{}", o[0].contours.len());
        let xs: Vec<f32> = o.iter().flat_map(|g| g.contours.iter().flatten().map(|p| p[0])).collect();
        let (lo, hi) = (xs.iter().copied().fold(f32::MAX, f32::min), xs.iter().copied().fold(f32::MIN, f32::max));
        assert!(lo < 0.0 && hi > 0.0 && (lo + hi).abs() < 10.0, "{lo} {hi}");
    }
}
