// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Scene regions: which parts of a picture are sky, water, ground, path,
//! trees, rocks, mountains, buildings, people and animals, as soft masks
//! and as traced outlines, so effects can be put inside them (ripples in
//! the water, sway in the trees, mist over the river).
//!
//! The model is UperNet with a ConvNeXt-T backbone trained on ADE20K
//! scene parsing (150 classes); its class probabilities are summed into
//! the coarse regions here. A depth map (Depth Anything V2), when given,
//! corrects the commonest mistake on landscapes: a reflection of the sky
//! in water read as sky. Near "sky" low in the frame becomes water.
//!
//! Xiao et al., "Unified Perceptual Parsing for Scene Understanding"
//! (ECCV 2018); Liu et al., "A ConvNet for the 2020s" (CVPR 2022). Weights
//! (openmmlab/upernet-convnext-tiny): MIT.

use crate::engine;
use crate::error::{invalid, Result};
use crate::frame::{Frame, Mask};
use crate::registry::ModelId;
use crate::store::ModelStore;
use std::sync::Arc;
use studio_nn::image::Norm;
use studio_nn::Session;

/// A kind of place in a picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Region {
    Sky,
    Water,
    Ground,
    Path,
    Trees,
    Rocks,
    Mountains,
    Buildings,
    People,
    Animals,
}

impl Region {
    pub const ALL: [Region; 10] = [Region::Sky, Region::Water, Region::Ground, Region::Path, Region::Trees, Region::Rocks, Region::Mountains, Region::Buildings, Region::People, Region::Animals];

    pub fn name(self) -> &'static str {
        match self {
            Region::Sky => "sky",
            Region::Water => "water",
            Region::Ground => "ground",
            Region::Path => "path",
            Region::Trees => "trees",
            Region::Rocks => "rocks",
            Region::Mountains => "mountains",
            Region::Buildings => "buildings",
            Region::People => "people",
            Region::Animals => "animals",
        }
    }

    /// From a name or a common word for it (`river` → water, `foliage` →
    /// trees, `road` → path).
    pub fn from_name(s: &str) -> Option<Region> {
        let s = s.trim().to_ascii_lowercase();
        let s = s.as_str();
        Some(match s {
            "sky" | "clouds" | "heavens" => Region::Sky,
            "water" | "river" | "lake" | "sea" | "ocean" | "stream" | "creek" | "pond" | "waterfall" => Region::Water,
            "ground" | "grass" | "field" | "earth" | "sand" | "floor" | "land" => Region::Ground,
            "path" | "road" | "track" | "trail" | "street" | "sidewalk" => Region::Path,
            "trees" | "tree" | "foliage" | "plants" | "plant" | "forest" | "leaves" | "bushes" | "canopy" => Region::Trees,
            "rocks" | "rock" | "stones" | "stone" | "boulders" => Region::Rocks,
            "mountains" | "mountain" | "hills" | "hill" => Region::Mountains,
            "buildings" | "building" | "houses" | "house" | "walls" | "bridge" | "structures" => Region::Buildings,
            "people" | "person" | "figures" => Region::People,
            "animals" | "animal" => Region::Animals,
            _ => return None,
        })
    }

    /// The ADE20K classes (0-based ids) that make up the region.
    fn classes(self) -> &'static [usize] {
        match self {
            Region::Sky => &[2],
            // water, sea, river, swimming pool, waterfall, fountain, lake
            Region::Water => &[21, 26, 60, 109, 113, 104, 128],
            // floor, grass, earth, field, sand, land
            Region::Ground => &[3, 9, 13, 29, 46, 94],
            // road, sidewalk, path, runway, dirt track, stairs, step
            Region::Path => &[6, 11, 52, 54, 91, 53, 121],
            // tree, plant, flower, palm
            Region::Trees => &[4, 17, 66, 72],
            Region::Rocks => &[34],
            // mountain, hill
            Region::Mountains => &[16, 68],
            // wall, building, house, skyscraper, fence, railing, column,
            // bridge, hovel, tower, awning, tent, pier, canopy
            Region::Buildings => &[0, 1, 25, 48, 32, 38, 42, 61, 79, 84, 86, 114, 140, 106],
            Region::People => &[12],
            Region::Animals => &[126],
        }
    }
}

/// Soft region masks for a picture.
#[derive(Clone, Debug)]
pub struct RegionMap {
    pub width: usize,
    pub height: usize,
    /// One probability plane per region, in [`Region::ALL`] order.
    pub probs: Vec<Vec<f32>>,
}

impl RegionMap {
    pub fn plane(&self, r: Region) -> &[f32] {
        &self.probs[Region::ALL.iter().position(|&x| x == r).unwrap_or(0)]
    }

    /// The region's soft mask (0..1).
    pub fn mask(&self, r: Region) -> Mask {
        Mask::new(self.width, self.height, self.plane(r).to_vec())
    }

    /// The region each pixel most likely is, if any region is likelier
    /// than everything else together with the leftover classes.
    pub fn label(&self, i: usize) -> Option<Region> {
        let (mut best, mut bv) = (None, 0.0f32);
        let mut sum = 0.0;
        for (k, &r) in Region::ALL.iter().enumerate() {
            let v = self.probs[k][i];
            sum += v;
            if v > bv {
                bv = v;
                best = Some(r);
            }
        }
        // "Other" holds what the regions leave.
        if bv >= (1.0 - sum).max(0.0) { best } else { None }
    }

    /// A hard mask: 1 where the region is the most likely label.
    pub fn hard(&self, r: Region) -> Vec<f32> {
        (0..self.width * self.height).map(|i| if self.label(i) == Some(r) { 1.0 } else { 0.0 }).collect()
    }

    /// Fraction of the picture the region takes.
    pub fn share(&self, r: Region) -> f32 {
        let n = (self.width * self.height).max(1);
        (0..n).filter(|&i| self.label(i) == Some(r)).count() as f32 / n as f32
    }

    /// Fixes what a depth map (1 = near, any size) shows is wrong: sky is
    /// the farthest thing there is, so "sky" that is near and in the lower
    /// half of the picture is its reflection in water.
    pub fn refine_with_depth(&mut self, depth: &Mask) {
        let d = depth.resized(self.width, self.height);
        let (si, wi) = (0usize, 1usize);
        for y in 0..self.height {
            let low = y as f32 / self.height as f32 > 0.45;
            for x in 0..self.width {
                let i = y * self.width + x;
                let near = d.data[i];
                if near > 0.35 {
                    let sky = self.probs[si][i];
                    let k = ((near - 0.35) / 0.25).clamp(0.0, 1.0);
                    let moved = sky * k;
                    self.probs[si][i] -= moved;
                    if low {
                        self.probs[wi][i] += moved;
                    }
                }
            }
        }
    }
}

/// Runs the scene-parsing model.
pub struct SceneParser {
    session: Arc<Session>,
    /// Side of the square picture the model sees.
    pub size: usize,
}

impl SceneParser {
    pub fn load(store: &ModelStore) -> Result<SceneParser> {
        let dir = store.require(ModelId::UperNetConvNextTiny)?;
        Ok(SceneParser { session: engine::session(&dir.join("model.onnx"))?, size: 512 })
    }

    /// Region masks for a picture, `width` across (the height keeps the
    /// picture's shape).
    pub fn parse(&self, frame: &Frame, width: usize) -> Result<RegionMap> {
        if frame.width < 16 || frame.height < 16 {
            return invalid("picture too small to read");
        }
        // The model was trained on 512-pixel crops; a wide picture is read
        // as two overlapping squares (left and right) and the halves are
        // blended, so nothing is squashed.
        let (w, h) = (frame.width, frame.height);
        let out_w = width.max(16);
        let out_h = (out_w * h / w).max(8);
        let mut acc = vec![vec![0.0f32; out_w * out_h]; Region::ALL.len()];
        let mut weight = vec![0.0f32; out_w * out_h];
        let crops: Vec<(usize, usize, usize, usize)> = if w > h * 5 / 4 {
            vec![(0, 0, h, h), (w - h, 0, h, h)]
        } else if h > w * 5 / 4 {
            vec![(0, 0, w, w), (0, h - w, w, w)]
        } else {
            vec![(0, 0, w, h)]
        };
        for &(cx, cy, cw, ch) in &crops {
            let px = studio_nn::image::crop_rgba(frame.view(), cx, cy, cw, ch);
            let view = studio_nn::image::Rgba { w: cw, h: ch, px: &px };
            let input = studio_nn::image::to_nchw(view, self.size, self.size, Norm::IMAGENET);
            let mut o = engine::run(&self.session, vec![("pixel_values", input)])?;
            let logits = o.take("logits")?;
            let (nc, lh, lw) = (logits.shape[1], logits.shape[2], logits.shape[3]);
            if nc != 150 {
                return invalid(format!("scene model gave {nc} classes, expected 150"));
            }
            let l = logits.into_f32_vec();
            let plane = lh * lw;
            // Softmax per position, summed into regions.
            let mut groups = vec![vec![0.0f32; plane]; Region::ALL.len()];
            for i in 0..plane {
                let mx = (0..nc).map(|c| l[c * plane + i]).fold(f32::MIN, f32::max);
                let mut e = [0.0f32; 150];
                let mut s = 0.0f32;
                for (c, ec) in e.iter_mut().enumerate() {
                    *ec = (l[c * plane + i] - mx).exp();
                    s += *ec;
                }
                for (g, r) in Region::ALL.iter().enumerate() {
                    groups[g][i] = r.classes().iter().map(|&c| e[c]).sum::<f32>() / s;
                }
            }
            // Into the output grid, feathering the crop's inner edge.
            let (ox0, oy0) = (cx * out_w / w, cy * out_h / h);
            let (ow, oh) = ((cw * out_w / w).max(1), (ch * out_h / h).max(1));
            let resized: Vec<Vec<f32>> = groups.iter().map(|g| studio_nn::image::resize_plane(g, lw, lh, ow, oh)).collect();
            for y in 0..oh {
                for x in 0..ow {
                    let (gx, gy) = (ox0 + x, oy0 + y);
                    if gx >= out_w || gy >= out_h {
                        continue;
                    }
                    // Weight falls off towards an edge that is inside the picture.
                    let fx = (x as f32 + 0.5) / ow as f32;
                    let fy = (y as f32 + 0.5) / oh as f32;
                    let edge = |f: f32, at_start: bool, at_end: bool| {
                        let a = if at_start { 1.0 } else { (f / 0.25).min(1.0) };
                        let b = if at_end { 1.0 } else { ((1.0 - f) / 0.25).min(1.0) };
                        a.min(b).max(1e-3)
                    };
                    let wgt = edge(fx, cx == 0, cx + cw >= w) * edge(fy, cy == 0, cy + ch >= h);
                    let i = gy * out_w + gx;
                    for g in 0..Region::ALL.len() {
                        acc[g][i] += resized[g][y * ow + x] * wgt;
                    }
                    weight[i] += wgt;
                }
            }
        }
        for g in acc.iter_mut() {
            for (v, wt) in g.iter_mut().zip(&weight) {
                *v /= wt.max(1e-6);
            }
        }
        Ok(RegionMap { width: out_w, height: out_h, probs: acc })
    }
}

/// One closed outline, points as fractions of the picture (0..1).
#[derive(Clone, Debug, PartialEq)]
pub struct Outline {
    pub points: Vec<[f32; 2]>,
    /// A hole inside another outline (cut it out of the mask).
    pub hole: bool,
    /// Area as a fraction of the picture.
    pub area: f32,
}

/// Traces the parts of a mask (`w`×`h`, above `threshold`) into closed
/// outlines with marching squares, drops pieces smaller than `min_area`
/// (fraction of the picture), and simplifies each to within `tolerance`
/// grid cells (raised until all outlines together have at most
/// `max_points` points).
pub fn outlines(mask: &[f32], w: usize, h: usize, threshold: f32, min_area: f32, tolerance: f32, max_points: usize) -> Vec<Outline> {
    let inside = |x: i64, y: i64| x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && mask[y as usize * w + x as usize] >= threshold;
    // Segments between edge midpoints, in doubled coordinates (corner
    // (x, y) of the pixel grid is pixel centre (x, y)).
    let mut segs: Vec<((i64, i64), (i64, i64))> = Vec::new();
    for y in -1..h as i64 {
        for x in -1..w as i64 {
            let tl = inside(x, y);
            let tr = inside(x + 1, y);
            let br = inside(x + 1, y + 1);
            let bl = inside(x, y + 1);
            let case = (tl as u8) << 3 | (tr as u8) << 2 | (br as u8) << 1 | bl as u8;
            let t = (2 * x + 1, 2 * y);
            let r = (2 * x + 2, 2 * y + 1);
            let b = (2 * x + 1, 2 * y + 2);
            let l = (2 * x, 2 * y + 1);
            match case {
                0 | 15 => {}
                1 | 14 => segs.push((l, b)),
                2 | 13 => segs.push((b, r)),
                3 | 12 => segs.push((l, r)),
                4 | 11 => segs.push((t, r)),
                6 | 9 => segs.push((t, b)),
                7 | 8 => segs.push((l, t)),
                5 => {
                    segs.push((l, t));
                    segs.push((b, r));
                }
                10 => {
                    segs.push((t, r));
                    segs.push((l, b));
                }
                _ => {}
            }
        }
    }
    // Link segments into loops through their shared midpoints.
    let mut at: std::collections::HashMap<(i64, i64), Vec<usize>> = std::collections::HashMap::new();
    for (i, s) in segs.iter().enumerate() {
        at.entry(s.0).or_default().push(i);
        at.entry(s.1).or_default().push(i);
    }
    let mut used = vec![false; segs.len()];
    let mut loops: Vec<Vec<[f32; 2]>> = Vec::new();
    for start in 0..segs.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let first = segs[start].0;
        let mut cur = segs[start].1;
        let mut pts = vec![first];
        while cur != first {
            pts.push(cur);
            let Some(next) = at.get(&cur).and_then(|v| v.iter().copied().find(|&j| !used[j])) else { break };
            used[next] = true;
            cur = if segs[next].0 == cur { segs[next].1 } else { segs[next].0 };
        }
        if pts.len() >= 3 {
            // Back to pixel units: doubled corner coordinates / 2 + 0.5.
            loops.push(pts.into_iter().map(|(x, y)| [x as f32 * 0.5 + 0.5, y as f32 * 0.5 + 0.5]).collect());
        }
    }
    let total = (w * h) as f32;
    let area = |p: &[[f32; 2]]| -> f32 {
        let mut a = 0.0;
        for i in 0..p.len() {
            let (q, r) = (p[i], p[(i + 1) % p.len()]);
            a += q[0] * r[1] - r[0] * q[1];
        }
        0.5 * a
    };
    let kept: Vec<(Vec<[f32; 2]>, f32)> = loops.into_iter().map(|p| { let a = area(&p).abs() / total; (p, a) }).filter(|(_, a)| *a >= min_area).collect();
    // A loop inside an odd number of others is a hole.
    let holes: Vec<bool> = kept.iter().enumerate().map(|(i, (p, _))| kept.iter().enumerate().filter(|(j, (q, _))| *j != i && contains(q, p[0])).count() % 2 == 1).collect();
    let mut tol = tolerance.max(0.1);
    loop {
        let simplified: Vec<Vec<[f32; 2]>> = kept.iter().map(|(p, _)| simplify_closed(p, tol)).collect();
        let n: usize = simplified.iter().map(Vec::len).sum();
        if n <= max_points || tol > 64.0 {
            return simplified
                .into_iter()
                .zip(&kept)
                .zip(&holes)
                .filter(|((p, _), _)| p.len() >= 3)
                .map(|((p, (_, a)), &hole)| Outline { points: p.into_iter().map(|q| [q[0] / w as f32, q[1] / h as f32]).collect(), hole, area: *a })
                .collect();
        }
        tol *= 1.5;
    }
}

/// Point in polygon (even-odd).
pub fn contains(poly: &[[f32; 2]], p: [f32; 2]) -> bool {
    let mut c = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + n - 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            c = !c;
        }
    }
    c
}

/// Douglas-Peucker on a closed loop: split at the two points farthest
/// apart, simplify each half.
fn simplify_closed(p: &[[f32; 2]], tol: f32) -> Vec<[f32; 2]> {
    if p.len() < 4 {
        return p.to_vec();
    }
    let far = (1..p.len()).max_by(|&a, &b| dist2(p[0], p[a]).total_cmp(&dist2(p[0], p[b]))).unwrap_or(p.len() / 2);
    let mut a = simplify(&p[..=far], tol);
    let mut rest: Vec<[f32; 2]> = p[far..].to_vec();
    rest.push(p[0]);
    let b = simplify(&rest, tol);
    a.pop();
    a.extend_from_slice(&b[..b.len() - 1]);
    a
}

fn dist2(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)
}

fn simplify(p: &[[f32; 2]], tol: f32) -> Vec<[f32; 2]> {
    if p.len() < 3 {
        return p.to_vec();
    }
    let (a, b) = (p[0], p[p.len() - 1]);
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = (dx * dx + dy * dy).sqrt();
    let (mut best, mut bi) = (0.0f32, 0usize);
    for (i, q) in p.iter().enumerate().take(p.len() - 1).skip(1) {
        let d = if len < 1e-6 { dist2(*q, a).sqrt() } else { ((q[0] - a[0]) * dy - (q[1] - a[1]) * dx).abs() / len };
        if d > best {
            best = d;
            bi = i;
        }
    }
    if best <= tol {
        return vec![a, b];
    }
    let mut left = simplify(&p[..=bi], tol);
    let right = simplify(&p[bi..], tol);
    left.pop();
    left.extend(right);
    left
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_words_find_regions() {
        for r in Region::ALL {
            assert_eq!(Region::from_name(r.name()), Some(r));
        }
        assert_eq!(Region::from_name("River"), Some(Region::Water));
        assert_eq!(Region::from_name("foliage"), Some(Region::Trees));
        assert_eq!(Region::from_name("road"), Some(Region::Path));
        assert_eq!(Region::from_name("spaceship"), None);
        // No class is in two regions.
        let mut seen = std::collections::HashSet::new();
        for r in Region::ALL {
            for &c in r.classes() {
                assert!(c < 150 && seen.insert(c), "class {c} twice");
            }
        }
    }

    #[test]
    fn a_square_traces_to_four_corners() {
        let (w, h) = (40, 30);
        let mut m = vec![0.0; w * h];
        for y in 10..20 {
            for x in 5..25 {
                m[y * w + x] = 1.0;
            }
        }
        let o = outlines(&m, w, h, 0.5, 0.0, 0.6, 500);
        assert_eq!(o.len(), 1);
        assert!(!o[0].hole);
        // Corners cut at 45° give eight points at most.
        assert!(o[0].points.len() >= 4 && o[0].points.len() <= 8, "{:?}", o[0].points);
        let xs: Vec<f32> = o[0].points.iter().map(|p| p[0] * w as f32).collect();
        let ys: Vec<f32> = o[0].points.iter().map(|p| p[1] * h as f32).collect();
        let (x0, x1) = (xs.iter().cloned().fold(f32::MAX, f32::min), xs.iter().cloned().fold(f32::MIN, f32::max));
        let (y0, y1) = (ys.iter().cloned().fold(f32::MAX, f32::min), ys.iter().cloned().fold(f32::MIN, f32::max));
        // The edge runs half way between inside and outside pixel centres.
        assert!((x0 - 5.0).abs() < 0.6 && (x1 - 25.0).abs() < 0.6 && (y0 - 10.0).abs() < 0.6 && (y1 - 20.0).abs() < 0.6, "{x0} {x1} {y0} {y1}");
        assert!((o[0].area - 200.0 / 1200.0).abs() < 0.02, "{}", o[0].area);
    }

    #[test]
    fn a_ring_has_a_hole_and_specks_are_dropped() {
        let (w, h) = (60, 60);
        let mut m = vec![0.0; w * h];
        for y in 0..h {
            for x in 0..w {
                let d = ((x as f32 - 30.0).powi(2) + (y as f32 - 30.0).powi(2)).sqrt();
                if (10.0..22.0).contains(&d) {
                    m[y * w + x] = 1.0;
                }
            }
        }
        m[2 * w + 2] = 1.0; // a speck
        let o = outlines(&m, w, h, 0.5, 0.005, 0.8, 500);
        assert_eq!(o.len(), 2, "{o:?}");
        assert_eq!(o.iter().filter(|x| x.hole).count(), 1);
        let hole = o.iter().find(|x| x.hole).unwrap();
        assert!(contains(&hole.points, [0.5, 0.5]));
        let outer = o.iter().find(|x| !x.hole).unwrap();
        assert!(!contains(&outer.points, [0.03, 0.03]));
    }

    #[test]
    fn the_point_budget_is_kept() {
        let (w, h) = (128, 72);
        let m: Vec<f32> = (0..w * h).map(|i| if ((i % w) as f32 * 0.37).sin() + ((i / w) as f32 * 0.21).cos() > 0.3 { 1.0 } else { 0.0 }).collect();
        let o = outlines(&m, w, h, 0.5, 0.001, 0.5, 120);
        let n: usize = o.iter().map(|x| x.points.len()).sum();
        assert!(n <= 120 && !o.is_empty(), "{n}");
    }

    #[test]
    fn reflected_sky_low_and_near_becomes_water() {
        let (w, h) = (8, 8);
        let mut probs = vec![vec![0.0f32; w * h]; Region::ALL.len()];
        probs[0] = vec![0.9; w * h];
        let mut map = RegionMap { width: w, height: h, probs };
        // Far at the top, near at the bottom.
        let depth = Mask::new(w, h, (0..w * h).map(|i| (i / w) as f32 / (h - 1) as f32).collect());
        map.refine_with_depth(&depth);
        assert_eq!(map.label(0), Some(Region::Sky));
        assert_eq!(map.label(7 * w + 3), Some(Region::Water));
    }

    #[test]
    fn needs_the_model() {
        let store = ModelStore::new(std::env::temp_dir().join("studio-ai-no-models"));
        assert!(matches!(SceneParser::load(&store), Err(crate::Error::ModelMissing(ModelId::UperNetConvNextTiny))));
    }
}
