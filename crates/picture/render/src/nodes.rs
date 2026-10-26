// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Grade nodes: secondary corrections after a clip's grade, applied in
//! order. Each node grades the picture coming out of the one before,
//! keyed by an HSL qualifier (on that incoming picture) times a power
//! window (one of the clip's masks, so it can be animated and tracked),
//! softened by an optional blur and scaled by the node's mix:
//!
//! ```text
//! key = mix × qualifier(in) × window          (window inverted on request)
//! out = in + key × (grade(in) − in)
//! ```
//!
//! Nodes run on the CPU; on the GPU path a clip with nodes is read back,
//! processed and uploaded again.

use studio_colour::{FrameInfo, Qualifier};

use crate::effects::grade_from_desc;
use crate::frame::Frame;
use crate::mask::{MaskSpec, PreparedMask};
use crate::scene::{GradeDesc, Param, Params, ParamsExt};

/// One node, evaluated for a frame.
#[derive(Clone, Debug, PartialEq)]
pub struct GradeNodeDesc {
    pub grade: GradeDesc,
    pub qualifier: Option<Qualifier>,
    pub window: Option<MaskSpec>,
    pub window_invert: bool,
    pub mix: f32,
    pub key_blur: f32,
}

/// A qualifier from node parameters: a named `preset` and/or the window
/// fields of [`Qualifier`]. `None` when nothing is set.
pub fn qualifier_from(p: &Params) -> Option<Qualifier> {
    if p.is_empty() {
        return None;
    }
    let mut q = match p.get("preset") {
        Some(Param::Choice(n)) | Some(Param::Text(n)) => Qualifier::preset(n).unwrap_or_default(),
        _ => Qualifier::default(),
    };
    q.hue = p.f("hue", q.hue);
    q.hue_width = p.f("hue_width", q.hue_width);
    q.hue_soft = p.f("hue_soft", q.hue_soft);
    q.sat_low = p.f("sat_low", q.sat_low);
    q.sat_high = p.f("sat_high", q.sat_high);
    q.sat_soft = p.f("sat_soft", q.sat_soft);
    q.lum_low = p.f("lum_low", q.lum_low);
    q.lum_high = p.f("lum_high", q.lum_high);
    q.lum_soft = p.f("lum_soft", q.lum_soft);
    if let Some(Param::Bool(b)) = p.get("invert") {
        q.invert = *b;
    }
    Some(q)
}

/// Separable box blur of a key buffer (two passes approximate a tent).
fn blur(key: &mut [f32], w: usize, h: usize, radius: usize) {
    if radius == 0 || w == 0 || h == 0 {
        return;
    }
    let mut tmp = vec![0f32; key.len()];
    for _ in 0..2 {
        for y in 0..h {
            let row = &key[y * w..(y + 1) * w];
            let mut acc = 0f32;
            let mut n = 0usize;
            for x in 0..radius.min(w) {
                acc += row[x];
                n += 1;
            }
            for x in 0..w {
                if x + radius < w {
                    acc += row[x + radius];
                    n += 1;
                }
                tmp[y * w + x] = acc / n as f32;
                if x >= radius {
                    acc -= row[x - radius];
                    n -= 1;
                }
            }
        }
        for x in 0..w {
            let mut acc = 0f32;
            let mut n = 0usize;
            for y in 0..radius.min(h) {
                acc += tmp[y * w + x];
                n += 1;
            }
            for y in 0..h {
                if y + radius < h {
                    acc += tmp[(y + radius) * w + x];
                    n += 1;
                }
                key[y * w + x] = acc / n as f32;
                if y >= radius {
                    acc -= tmp[(y - radius) * w + x];
                    n -= 1;
                }
            }
        }
    }
}

/// The key of one node over `img` (linear premultiplied).
pub fn node_key(node: &GradeNodeDesc, img: &studio_colour::Image) -> Vec<f32> {
    let (w, h) = (img.width as usize, img.height as usize);
    let window = node.window.as_ref().map(|m| PreparedMask::new(m, img.width, img.height));
    let mut key = vec![0f32; w * h];
    for (i, k) in key.iter_mut().enumerate() {
        let p = img.pixels[i];
        let q = match &node.qualifier {
            Some(q) => {
                let a = p[3];
                let c = if a > 0.0 && a < 1.0 { [p[0] / a, p[1] / a, p[2] / a] } else { [p[0], p[1], p[2]] };
                q.key(c)
            }
            None => 1.0,
        };
        let win = match &window {
            Some(m) => {
                let c = m.coverage((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
                if node.window_invert { 1.0 - c } else { c }
            }
            None => 1.0,
        };
        *k = q * win;
    }
    blur(&mut key, w, h, node.key_blur.max(0.0).round() as usize);
    for k in &mut key {
        *k = (*k * node.mix).clamp(0.0, 1.0);
    }
    key
}

/// Applies the nodes in order.
pub fn apply_nodes(frame: Frame, nodes: &[GradeNodeDesc], frame_index: u64) -> Frame {
    if nodes.is_empty() {
        return frame;
    }
    let mut img = frame.into_colour();
    for node in nodes {
        if node.mix <= 0.0 {
            continue;
        }
        let grade = grade_from_desc(&node.grade);
        let key = node_key(node, &img);
        let mut graded = img.clone();
        if !grade.is_identity() {
            grade.apply(&mut graded, FrameInfo { frame: frame_index });
        }
        for ((p, g), k) in img.pixels.iter_mut().zip(&graded.pixels).zip(&key) {
            if *k <= 0.0 {
                continue;
            }
            for c in 0..4 {
                p[c] += k * (g[c] - p[c]);
            }
        }
    }
    Frame::from_colour(img)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mask::MaskShape;

    fn frame() -> Frame {
        // Left half skin, right half sky.
        let skin = studio_colour::Transfer::Bt1886.decode_rgb([0.85, 0.62, 0.5]);
        let sky = studio_colour::Transfer::Bt1886.decode_rgb([0.45, 0.65, 0.92]);
        Frame::from_pixels(8, 4, (0..32).map(|i| if i % 8 < 4 { [skin[0], skin[1], skin[2], 1.0] } else { [sky[0], sky[1], sky[2], 1.0] }).collect())
    }

    fn darker() -> GradeDesc {
        let mut params = Params::default();
        params.insert("exposure".into(), Param::Float(-1.0));
        GradeDesc { params, lut: None }
    }

    #[test]
    fn qualifier_limits_the_node() {
        let mut q = Params::default();
        q.insert("preset".into(), Param::Text("sky".into()));
        let node = GradeNodeDesc { grade: darker(), qualifier: qualifier_from(&q), window: None, window_invert: false, mix: 1.0, key_blur: 0.0 };
        let before = frame();
        let after = apply_nodes(before.clone(), &[node], 0);
        // Skin untouched, sky darker by a stop.
        assert!((after.pixels[0][0] - before.pixels[0][0]).abs() < 1e-4);
        assert!((after.pixels[6][2] / before.pixels[6][2] - 0.5).abs() < 0.02, "{:?} {:?}", after.pixels[6], before.pixels[6]);
    }

    #[test]
    fn window_and_mix() {
        // A rectangle over the left half.
        let window = MaskSpec { shape: MaskShape::Rectangle, centre: [0.25, 0.5], size: [0.5, 1.0], feather: 0.0, ..MaskSpec::default() };
        let node = GradeNodeDesc { grade: darker(), qualifier: None, window: Some(window.clone()), window_invert: false, mix: 0.5, key_blur: 0.0 };
        let before = frame();
        let after = apply_nodes(before.clone(), &[node.clone()], 0);
        let k = after.pixels[1][0] / before.pixels[1][0];
        assert!((k - 0.75).abs() < 0.02, "half mix of a stop down: {k}");
        assert!((after.pixels[6][0] - before.pixels[6][0]).abs() < 1e-4, "outside the window");
        let inv = GradeNodeDesc { window_invert: true, mix: 1.0, ..node };
        let after = apply_nodes(before.clone(), &[inv], 0);
        assert!((after.pixels[1][0] - before.pixels[1][0]).abs() < 1e-4);
        assert!(after.pixels[6][0] < before.pixels[6][0] * 0.6);
    }

    #[test]
    fn key_blur_softens_edges() {
        let mut key = vec![0f32; 16];
        key[8..].fill(1.0);
        blur(&mut key, 16, 1, 3);
        assert!(key[6] > 0.0 && key[6] < 0.5 && key[9] > 0.5 && key[9] < 1.0, "{key:?}");
    }
}
