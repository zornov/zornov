// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Headless pictures of the interface for tests: egui's tessellated meshes
//! drawn on the CPU (no window, no GPU), and a check for text that runs out
//! of its space and is cut off with no way to see the rest.
//!
//! The picture is close to what a window shows: premultiplied colours
//! blended in gamma space, bilinear texture samples. Custom GPU callbacks
//! (the viewer's frame, 3D viewports) are left empty.

use eframe::egui::{self, epaint, Color32, Rect, TextureId};
use std::collections::HashMap;

/// The textures a backend would hold: the font atlas, thumbnails, icons.
#[derive(Default)]
pub struct Textures(HashMap<TextureId, (usize, usize, Vec<Color32>)>);

impl Textures {
    pub fn apply(&mut self, delta: &egui::TexturesDelta) {
        for (id, patches) in &delta.set {
            for d in patches {
                let egui::ImageData::Color(img) = &d.image;
                let [w, h] = img.size;
                match d.pos {
                    None => {
                        self.0.insert(*id, (w, h, img.pixels.clone()));
                    }
                    Some([x0, y0]) => {
                        let Some((tw, th, px)) = self.0.get_mut(id) else { continue };
                        for y in 0..h.min(th.saturating_sub(y0)) {
                            for x in 0..w.min(tw.saturating_sub(x0)) {
                                px[(y0 + y) * *tw + x0 + x] = img.pixels[y * w + x];
                            }
                        }
                    }
                }
            }
        }
        for id in &delta.free {
            self.0.remove(id);
        }
    }

    fn sample(&self, id: TextureId, u: f32, v: f32) -> [f32; 4] {
        let Some((w, h, px)) = self.0.get(&id) else { return [255.0; 4] };
        let (w, h) = (*w, *h);
        let x = (u * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
        let y = (v * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let at = |x: usize, y: usize| px[y * w + x].to_array().map(f32::from);
        let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
        std::array::from_fn(|i| (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy)
    }
}

/// Draw a frame's primitives into an RGBA image of `size` physical pixels.
pub fn render(prims: &[epaint::ClippedPrimitive], tex: &Textures, size: [usize; 2], ppp: f32, background: Color32) -> Vec<u8> {
    let [w, h] = size;
    let mut img = vec![background.to_array().map(f32::from); w * h];
    for p in prims {
        let epaint::Primitive::Mesh(mesh) = &p.primitive else { continue };
        let clip = p.clip_rect;
        let cx0 = (clip.min.x * ppp).round().max(0.0) as usize;
        let cy0 = (clip.min.y * ppp).round().max(0.0) as usize;
        let cx1 = ((clip.max.x * ppp).round().max(0.0) as usize).min(w);
        let cy1 = ((clip.max.y * ppp).round().max(0.0) as usize).min(h);
        for tri in mesh.indices.chunks_exact(3) {
            let v = [tri[0], tri[1], tri[2]].map(|i| &mesh.vertices[i as usize]);
            let pt = v.map(|v| (v.pos.x * ppp, v.pos.y * ppp));
            let area = (pt[1].0 - pt[0].0) * (pt[2].1 - pt[0].1) - (pt[2].0 - pt[0].0) * (pt[1].1 - pt[0].1);
            if area.abs() < 1e-6 {
                continue;
            }
            let minx = (pt.iter().map(|p| p.0).fold(f32::MAX, f32::min).floor().max(0.0) as usize).max(cx0);
            let maxx = (pt.iter().map(|p| p.0).fold(f32::MIN, f32::max).ceil().max(0.0) as usize).min(cx1);
            let miny = (pt.iter().map(|p| p.1).fold(f32::MAX, f32::min).floor().max(0.0) as usize).max(cy0);
            let maxy = (pt.iter().map(|p| p.1).fold(f32::MIN, f32::max).ceil().max(0.0) as usize).min(cy1);
            let col = v.map(|v| v.color.to_array().map(f32::from));
            for y in miny..maxy {
                for x in minx..maxx {
                    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                    let w0 = ((pt[1].0 - px) * (pt[2].1 - py) - (pt[2].0 - px) * (pt[1].1 - py)) / area;
                    let w1 = ((pt[2].0 - px) * (pt[0].1 - py) - (pt[0].0 - px) * (pt[2].1 - py)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                        continue;
                    }
                    let u = w0 * v[0].uv.x + w1 * v[1].uv.x + w2 * v[2].uv.x;
                    let vv = w0 * v[0].uv.y + w1 * v[1].uv.y + w2 * v[2].uv.y;
                    let t = tex.sample(mesh.texture_id, u, vv);
                    let src: [f32; 4] = std::array::from_fn(|i| (w0 * col[0][i] + w1 * col[1][i] + w2 * col[2][i]) * t[i] / 255.0);
                    let dst = &mut img[y * w + x];
                    let k = 1.0 - src[3] / 255.0;
                    for i in 0..4 {
                        dst[i] = src[i] + dst[i] * k;
                    }
                }
            }
        }
    }
    img.into_iter().flat_map(|p| p.map(|c| c.round().clamp(0.0, 255.0) as u8)).collect()
}

pub fn save_png(path: &std::path::Path, size: [usize; 2], rgba: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let f = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(f, size[0] as u32, size[1] as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().map_err(std::io::Error::other)?;
    wr.write_image_data(rgba).map_err(std::io::Error::other)
}

/// A piece of text drawn wider or taller than the space it was given, so
/// that part of it is cut off by its clip rectangle or the screen.
#[derive(Debug, Clone)]
pub struct Cut {
    pub text: String,
    pub rect: Rect,
    pub clip: Rect,
}

/// Text that runs past its clip rectangle (or the screen) by more than
/// `slack` points across, or past the bottom of its clip with no scroll
/// area to bring it into view. Text inside a registered scroll viewport
/// (see [`crate::kit::scrolled_rects`]) may be cut along the scrolling axis.
pub fn cut_text(ctx: &egui::Context, out: &egui::FullOutput, extra: &[(Rect, bool)], slack: f32) -> Vec<Cut> {
    let screen = ctx.content_rect();
    let mut scrolled = crate::kit::scrolled_rects(ctx);
    scrolled.extend_from_slice(extra);
    // egui's own scroll areas (drop-down lists, menus) put a drag-only
    // widget over their viewport while their content is too large.
    ctx.viewport(|v| {
        for (_, list) in v.this_pass.widgets.layers() {
            for w in list.iter().filter(|w| w.sense == egui::Sense::DRAG) {
                scrolled.push((w.interact_rect, false));
                scrolled.push((w.interact_rect, true));
            }
        }
    });
    let mut cuts = vec![];
    fn walk(shape: &epaint::Shape, f: &mut impl FnMut(&epaint::TextShape)) {
        match shape {
            epaint::Shape::Text(t) => f(t),
            epaint::Shape::Vec(v) => v.iter().for_each(|s| walk(s, f)),
            _ => {}
        }
    }
    for cs in &out.shapes {
        let clip = cs.clip_rect.intersect(screen);
        walk(&cs.shape, &mut |t| {
            if t.angle != 0.0 || t.galley.is_empty() {
                return;
            }
            let rect = t.galley.rect.translate(t.pos.to_vec2());
            // Only text that shows at all: wholly outside its clip it is
            // scrolled away or hidden on purpose.
            if !rect.intersects(clip) || clip.width() < 1.0 || clip.height() < 1.0 {
                return;
            }
            // Cut by a scroll viewport's edge along the axis it scrolls.
            let in_scroll = |horizontal: bool| {
                scrolled.iter().any(|(r, h)| {
                    *h == horizontal
                        && r.intersects(rect)
                        && if horizontal { rect.right() > r.right() - 1.0 || rect.left() < r.left() + 1.0 } else { rect.bottom() > r.bottom() - 1.0 || rect.top() < r.top() + 1.0 }
                })
            };
            let across = rect.left() < clip.left() - slack || rect.right() > clip.right() + slack;
            let down = rect.bottom() > clip.bottom() + slack && rect.top() < clip.bottom();
            if (across && !in_scroll(true)) || (down && !in_scroll(false)) {
                cuts.push(Cut { text: t.galley.text().to_owned(), rect, clip });
            }
        });
    }
    cuts
}

/// Two widgets on the same layer drawn over each other: neither holds the
/// other, and they share more than a sliver (`min_share` of the smaller).
#[derive(Debug, Clone)]
pub struct Overlap {
    pub a: String,
    pub b: String,
    pub ra: Rect,
    pub rb: Rect,
}

pub fn overlaps(ctx: &egui::Context, names: &std::collections::HashMap<u64, String>, min_share: f32) -> Vec<Overlap> {
    let screen = ctx.content_rect();
    ctx.viewport(|v| {
        let w = &v.this_pass.widgets;
        let mut found = vec![];
        for (_, list) in w.layers() {
            let shown: Vec<_> = list
                .iter()
                .filter(|r| r.interact_rect.width() > 1.0 && r.interact_rect.height() > 1.0 && r.interact_rect.intersects(screen))
                .filter_map(|r| Some((r, names.get(&r.id.value()).filter(|n| !n.trim().is_empty())?)))
                .collect();
            for (i, (a, na)) in shown.iter().enumerate() {
                for (b, nb) in &shown[i + 1..] {
                    let (ra, rb) = (a.interact_rect, b.interact_rect);
                    if a.id == b.id || ra.expand(0.5).contains_rect(rb) || rb.expand(0.5).contains_rect(ra) {
                        continue;
                    }
                    let x = ra.intersect(rb);
                    if x.width() <= 0.0 || x.height() <= 0.0 {
                        continue;
                    }
                    if x.area() >= min_share * ra.area().min(rb.area()) {
                        found.push(Overlap { a: (*na).clone(), b: (*nb).clone(), ra, rb });
                    }
                }
            }
        }
        found
    })
}
