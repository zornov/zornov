// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! AI selections with the local models: the object at a point or in a box
//! (SAM 2.1), people (portrait matting), the main subject (people when
//! there are any, otherwise the nearest object found from depth and cut out
//! with SAM), the sky (far, open, light regions touching the top edge,
//! refined with SAM) and the background (all but the subject). Each gives a
//! soft mask at document size that becomes the selection (replace, add,
//! subtract or intersect), or a layer mask.
//!
//! The models sit behind [`Vision`], so the selection logic is tested with
//! synthetic masks and the app can share loaded models.

use serde_json::{Value, json};
use studio_pixel_algo::selection::{self as sel, SelectionMode};
use studio_pixel_geom::Rect;
use studio_pixel_raster::Rgba8Image;

/// A prompt for the object model, in image pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Prompt {
    Point { x: f32, y: f32, positive: bool },
    Box { x0: f32, y0: f32, x1: f32, y1: f32 },
}

/// The models a selection needs. Masks are `width × height` of the image
/// they were given, 0..1.
pub trait Vision {
    /// The object the prompts point at.
    fn object(&mut self, img: &Rgba8Image, prompts: &[Prompt]) -> Result<Vec<f32>, String>;
    /// People (soft, hair-level edges).
    fn people(&mut self, img: &Rgba8Image) -> Result<Vec<f32>, String>;
    /// Relative depth, 1 = near.
    fn depth(&mut self, img: &Rgba8Image) -> Result<Vec<f32>, String>;
}

/// No models: every AI selection says so.
pub struct NoVision;

impl Vision for NoVision {
    fn object(&mut self, _: &Rgba8Image, _: &[Prompt]) -> Result<Vec<f32>, String> {
        Err("AI selection needs the object model (SAM 2.1); install it in Settings ▸ AI".into())
    }
    fn people(&mut self, _: &Rgba8Image) -> Result<Vec<f32>, String> {
        Err("AI selection needs the matting model; install it in Settings ▸ AI".into())
    }
    fn depth(&mut self, _: &Rgba8Image) -> Result<Vec<f32>, String> {
        Err("AI selection needs the depth model; install it in Settings ▸ AI".into())
    }
}

/// The installed local models, loaded on first use.
#[cfg(feature = "ai")]
pub struct LocalModels {
    store: studio_ai::store::ModelStore,
    sam: Option<studio_ai::features::segment::ObjectCutout>,
    matte: Option<studio_ai::features::matting::BackgroundRemover>,
    depth: Option<studio_ai::features::depth::DepthEstimator>,
}

#[cfg(feature = "ai")]
impl LocalModels {
    pub fn new(store: studio_ai::store::ModelStore) -> LocalModels {
        LocalModels { store, sam: None, matte: None, depth: None }
    }

    /// The models in Studio's usual folder.
    pub fn installed() -> LocalModels {
        LocalModels::new(studio_ai::store::ModelStore::new(studio_ai::store::ModelStore::default_root()))
    }
}

#[cfg(feature = "ai")]
fn frame(img: &Rgba8Image) -> Result<studio_ai::frame::Frame, String> {
    studio_ai::frame::Frame::new(img.width as usize, img.height as usize, img.pixels.clone()).map_err(|e| e.to_string())
}

#[cfg(feature = "ai")]
impl Vision for LocalModels {
    fn object(&mut self, img: &Rgba8Image, prompts: &[Prompt]) -> Result<Vec<f32>, String> {
        use studio_ai::features::segment as seg;
        if self.sam.is_none() {
            self.sam = Some(seg::ObjectCutout::load(&self.store).map_err(|e| format!("the object model is not ready ({e}); install SAM 2.1 in Settings ▸ AI"))?);
        }
        let ps: Vec<seg::Prompt> = prompts
            .iter()
            .map(|p| match *p {
                Prompt::Point { x, y, positive } => seg::Prompt::Point { x, y, positive },
                Prompt::Box { x0, y0, x1, y1 } => seg::Prompt::Box { x0, y0, x1, y1 },
            })
            .collect();
        let s = self.sam.as_ref().ok_or("no object model")?.cut_out(&frame(img)?, &ps).map_err(|e| e.to_string())?;
        Ok(s.mask.data)
    }
    fn people(&mut self, img: &Rgba8Image) -> Result<Vec<f32>, String> {
        if self.matte.is_none() {
            self.matte = Some(studio_ai::features::matting::BackgroundRemover::load(&self.store).map_err(|e| format!("the matting model is not ready ({e}); install it in Settings ▸ AI"))?);
        }
        Ok(self.matte.as_ref().ok_or("no matting model")?.matte(&frame(img)?).map_err(|e| e.to_string())?.data)
    }
    fn depth(&mut self, img: &Rgba8Image) -> Result<Vec<f32>, String> {
        if self.depth.is_none() {
            self.depth = Some(studio_ai::features::depth::DepthEstimator::load(&self.store).map_err(|e| format!("the depth model is not ready ({e}); install it in Settings ▸ AI"))?);
        }
        Ok(self.depth.as_ref().ok_or("no depth model")?.estimate(&frame(img)?).map_err(|e| e.to_string())?.data)
    }
}

/// What to select.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Subject,
    People,
    Sky,
    Background,
    Object,
}

impl Target {
    pub fn parse(s: &str) -> Result<Target, String> {
        Ok(match s.trim().to_lowercase().as_str() {
            "subject" | "main" | "foreground" => Target::Subject,
            "people" | "person" | "persons" | "humans" | "portrait" => Target::People,
            "sky" | "skies" => Target::Sky,
            "background" | "bg" => Target::Background,
            "object" | "thing" | "point" | "box" => Target::Object,
            other => return Err(format!("cannot select \"{other}\": subject, people, sky, background, or object (with point(s) or a box)")),
        })
    }
}

/// The largest side the models are shown (masks are scaled back up).
const WORK_SIDE: u32 = 1536;

/// A mask at the size of `img` for `target`.
pub fn mask_for(v: &mut dyn Vision, img: &Rgba8Image, target: Target, prompts: &[Prompt]) -> Result<Vec<f32>, String> {
    let (w, h) = (img.width as usize, img.height as usize);
    match target {
        Target::Object => {
            if prompts.is_empty() {
                return Err("selecting an object needs `point` ([x, y]), `points` ([[x, y], …], with `exclude` for points not on it) or `box` ([x0, y0, x1, y1])".into());
            }
            v.object(img, prompts)
        }
        Target::People => v.people(img),
        Target::Subject => subject(v, img),
        Target::Background => Ok(subject(v, img)?.into_iter().map(|m| 1.0 - m).collect()),
        Target::Sky => sky(v, img, w, h),
    }
}

/// People when there are some; else the nearest object, from depth, cut out
/// with the object model.
fn subject(v: &mut dyn Vision, img: &Rgba8Image) -> Result<Vec<f32>, String> {
    let (w, h) = (img.width as usize, img.height as usize);
    if let Ok(p) = v.people(img) {
        let cover = p.iter().filter(|&&m| m > 0.5).count() as f32 / p.len().max(1) as f32;
        // Portrait matting answers for any picture; trust it when it finds a
        // solid figure, not speckle.
        if cover > 0.02 && largest_share(&p, w, h, 0.5) > 0.6 {
            return Ok(p);
        }
    }
    let d = v.depth(img)?;
    let near = top_share(&d, 0.22);
    let blob = largest_component(&near, w, h);
    let (x0, y0, x1, y1) = bbox(&blob, w, h).ok_or("found no subject")?;
    let (cx, cy) = interior(&blob, w, h).unwrap_or(((x0 + x1) as f32 / 2.0, (y0 + y1) as f32 / 2.0));
    let pad = |a: usize, b: usize, n: usize| ((a as f32 - (b - a) as f32 * 0.04).max(0.0), ((b as f32 + (b - a) as f32 * 0.04).min(n as f32)));
    let (bx0, bx1) = pad(x0, x1, w);
    let (by0, by1) = pad(y0, y1, h);
    match v.object(img, &[Prompt::Box { x0: bx0, y0: by0, x1: bx1, y1: by1 }, Prompt::Point { x: cx, y: cy, positive: true }]) {
        Ok(m) => Ok(m),
        // No object model: the depth blob, softened.
        Err(_) => Ok(soften(&blob.iter().map(|&b| if b { 1.0 } else { 0.0 }).collect::<Vec<_>>(), w, h, 2)),
    }
}

/// Far, light, smooth regions connected to the top edge; refined by the
/// object model when it agrees.
fn sky(v: &mut dyn Vision, img: &Rgba8Image, w: usize, h: usize) -> Result<Vec<f32>, String> {
    let d = v.depth(img).ok();
    let px = &img.pixels;
    let luma = |i: usize| (0.299 * px[i * 4] as f32 + 0.587 * px[i * 4 + 1] as f32 + 0.114 * px[i * 4 + 2] as f32) / 255.0;
    // Local contrast: sky is smooth.
    let lum: Vec<f32> = (0..w * h).map(luma).collect();
    let mean = box_blur(&lum, w, h, 3);
    let sq: Vec<f32> = lum.iter().map(|l| l * l).collect();
    let mean_sq = box_blur(&sq, w, h, 3);
    let far_cut = d.as_ref().map(|d| quantile(d, 0.35));
    let cand: Vec<bool> = (0..w * h)
        .map(|i| {
            let var = (mean_sq[i] - mean[i] * mean[i]).max(0.0);
            let (r, g, b) = (px[i * 4] as f32, px[i * 4 + 1] as f32, px[i * 4 + 2] as f32);
            let bluish = lum[i] > 0.15 && b >= r * 0.9 && b >= g * 0.85;
            let light = lum[i] > 0.35;
            let warm_sky = lum[i] > 0.45 && r > b; // dusk and dawn skies
            let far = match (&d, far_cut) {
                (Some(d), Some(c)) => d[i] <= c.max(0.15),
                _ => true,
            };
            far && var < 0.004 && (bluish || light || warm_sky)
        })
        .collect();
    // Only what reaches the top edge.
    let sky = touching_top(&cand, w, h);
    let n = sky.iter().filter(|&&s| s).count();
    if n < w * h / 200 {
        return Err("found no sky in this picture".into());
    }
    let soft = soften(&sky.iter().map(|&s| if s { 1.0 } else { 0.0 }).collect::<Vec<_>>(), w, h, 2);
    // Refine with the object model from the sky's most interior points.
    if let Some((cx, cy)) = interior(&sky, w, h) {
        if let Ok(m) = v.object(img, &[Prompt::Point { x: cx, y: cy, positive: true }]) {
            let agree = iou(&m, &soft);
            // The model's edge is better when it found the same region.
            if agree > 0.6 {
                return Ok(m);
            }
        }
    }
    Ok(soft)
}

/// Put a mask (`w × h` over the whole document) into the selection.
pub fn select_mask(canvas: &mut crate::Canvas, mask: &[f32], w: u32, h: u32, mode: &str, feather: f32, label: &str) -> Result<Value, String> {
    let doc = canvas.doc().ok_or("no picture open")?;
    let (dw, dh) = (doc.size.width, doc.size.height);
    let full = if (w, h) == (dw, dh) { mask.to_vec() } else { resize(mask, w as usize, h as usize, dw as usize, dh as usize) };
    let full = if feather > 0.0 { sel::feather(&full, dw as usize, dh as usize, feather) } else { full };
    let area = Rect::from_xywh(0, 0, dw, dh);
    let m = SelectionMode::parse(mode);
    let covered = full.iter().filter(|&&v| v > 0.5).count() as f64 / full.len().max(1) as f64;
    let any = canvas
        .engine
        .edit(label, |doc, _| {
            doc.selection = sel::combine(doc.selection.as_ref(), &full, area, m);
            Ok(doc.selection.is_some())
        })
        .map_err(crate::err)?;
    Ok(json!({"selected": any, "share": (covered * 1000.0).round() / 1000.0}))
}

/// The composite at the models' working size, with the scale back to the document.
pub fn work_image(canvas: &crate::Canvas) -> Result<Rgba8Image, String> {
    canvas.render(WORK_SIDE)
}

/// Scale prompts given in document pixels to the working image.
pub fn scale_prompts(prompts: &[Prompt], doc_w: u32, img_w: u32) -> Vec<Prompt> {
    let k = img_w as f32 / doc_w.max(1) as f32;
    prompts
        .iter()
        .map(|p| match *p {
            Prompt::Point { x, y, positive } => Prompt::Point { x: x * k, y: y * k, positive },
            Prompt::Box { x0, y0, x1, y1 } => Prompt::Box { x0: x0 * k, y0: y0 * k, x1: x1 * k, y1: y1 * k },
        })
        .collect()
}

/// Prompts from an op: `point`, `points` (+ `exclude`), `box`.
pub fn prompts_from(op: &Value) -> Result<Vec<Prompt>, String> {
    let xy = |v: &Value| -> Option<(f32, f32)> {
        let a = v.as_array()?;
        Some((a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32))
    };
    let mut out = Vec::new();
    if let Some(p) = op.get("point") {
        let (x, y) = xy(p).ok_or("`point` is [x, y]")?;
        out.push(Prompt::Point { x, y, positive: true });
    }
    for (key, positive) in [("points", true), ("exclude", false)] {
        if let Some(a) = op.get(key).and_then(Value::as_array) {
            for p in a {
                let (x, y) = xy(p).ok_or_else(|| format!("each of `{key}` is [x, y]"))?;
                out.push(Prompt::Point { x, y, positive });
            }
        }
    }
    if let Some(b) = op.get("box").and_then(Value::as_array) {
        let g = |i: usize| b.get(i).and_then(Value::as_f64).map(|v| v as f32);
        match (g(0), g(1), g(2), g(3)) {
            (Some(x0), Some(y0), Some(x1), Some(y1)) => out.push(Prompt::Box { x0, y0, x1, y1 }),
            _ => return Err("`box` is [x0, y0, x1, y1]".into()),
        }
    }
    Ok(out)
}

// ---------- mask helpers ----------

pub fn resize(m: &[f32], w: usize, h: usize, nw: usize, nh: usize) -> Vec<f32> {
    let mut out = vec![0.0; nw * nh];
    if w == 0 || h == 0 {
        return out;
    }
    for y in 0..nh {
        let sy = ((y as f32 + 0.5) * h as f32 / nh as f32 - 0.5).clamp(0.0, (h - 1) as f32);
        let (y0, fy) = (sy.floor() as usize, sy.fract());
        let y1 = (y0 + 1).min(h - 1);
        for x in 0..nw {
            let sx = ((x as f32 + 0.5) * w as f32 / nw as f32 - 0.5).clamp(0.0, (w - 1) as f32);
            let (x0, fx) = (sx.floor() as usize, sx.fract());
            let x1 = (x0 + 1).min(w - 1);
            let a = m[y0 * w + x0] * (1.0 - fx) + m[y0 * w + x1] * fx;
            let b = m[y1 * w + x0] * (1.0 - fx) + m[y1 * w + x1] * fx;
            out[y * nw + x] = a * (1.0 - fy) + b * fy;
        }
    }
    out
}

fn quantile(v: &[f32], q: f32) -> f32 {
    let mut s: Vec<f32> = v.iter().copied().filter(|x| x.is_finite()).collect();
    if s.is_empty() {
        return 0.0;
    }
    s.sort_by(f32::total_cmp);
    s[((s.len() - 1) as f32 * q.clamp(0.0, 1.0)) as usize]
}

/// The top `share` of values.
fn top_share(v: &[f32], share: f32) -> Vec<bool> {
    let cut = quantile(v, 1.0 - share);
    v.iter().map(|&x| x >= cut).collect()
}

fn components(m: &[bool], w: usize, h: usize) -> (Vec<u32>, Vec<usize>) {
    let mut label = vec![0u32; w * h];
    let mut sizes = vec![0usize];
    let mut stack = Vec::new();
    for start in 0..w * h {
        if !m[start] || label[start] != 0 {
            continue;
        }
        let id = sizes.len() as u32;
        sizes.push(0);
        label[start] = id;
        stack.push(start);
        while let Some(i) = stack.pop() {
            sizes[id as usize] += 1;
            let (x, y) = (i % w, i / w);
            let mut visit = |j: usize| {
                if m[j] && label[j] == 0 {
                    label[j] = id;
                    stack.push(j);
                }
            };
            if x > 0 {
                visit(i - 1);
            }
            if x + 1 < w {
                visit(i + 1);
            }
            if y > 0 {
                visit(i - w);
            }
            if y + 1 < h {
                visit(i + w);
            }
        }
    }
    (label, sizes)
}

fn largest_component(m: &[bool], w: usize, h: usize) -> Vec<bool> {
    let (label, sizes) = components(m, w, h);
    let Some((best, _)) = sizes.iter().enumerate().skip(1).max_by_key(|(_, s)| **s) else { return vec![false; w * h] };
    label.iter().map(|&l| l == best as u32).collect()
}

/// The share of the above-`t` pixels in the largest connected part.
fn largest_share(m: &[f32], w: usize, h: usize, t: f32) -> f32 {
    let b: Vec<bool> = m.iter().map(|&v| v > t).collect();
    let (_, sizes) = components(&b, w, h);
    let total: usize = sizes.iter().sum();
    sizes.iter().skip(1).max().copied().unwrap_or(0) as f32 / total.max(1) as f32
}

fn touching_top(m: &[bool], w: usize, h: usize) -> Vec<bool> {
    let (label, _) = components(m, w, h);
    let mut keep = std::collections::HashSet::new();
    for x in 0..w {
        if label[x] != 0 {
            keep.insert(label[x]);
        }
    }
    let _ = h;
    label.iter().map(|l| *l != 0 && keep.contains(l)).collect()
}

fn bbox(m: &[bool], w: usize, h: usize) -> Option<(usize, usize, usize, usize)> {
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if m[y * w + x] {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    (x1 > 0).then_some((x0, y0, x1, y1))
}

/// The point deepest inside the mask (largest distance to its edge).
fn interior(m: &[bool], w: usize, h: usize) -> Option<(f32, f32)> {
    // Two-pass chamfer distance.
    let inf = u32::MAX / 4;
    let mut d: Vec<u32> = m.iter().map(|&b| if b { inf } else { 0 }).collect();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if d[i] == 0 {
                continue;
            }
            let mut v = d[i];
            v = v.min(if x > 0 { d[i - 1] + 1 } else { 1 });
            v = v.min(if y > 0 { d[i - w] + 1 } else { 1 });
            d[i] = v;
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let i = y * w + x;
            if d[i] == 0 {
                continue;
            }
            let mut v = d[i];
            v = v.min(if x + 1 < w { d[i + 1] + 1 } else { 1 });
            v = v.min(if y + 1 < h { d[i + w] + 1 } else { 1 });
            d[i] = v;
        }
    }
    let (i, &best) = d.iter().enumerate().max_by_key(|(_, v)| **v)?;
    (best > 0).then(|| ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5))
}

fn box_blur(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut tmp = vec![0.0; w * h];
    for y in 0..h {
        let mut acc = 0.0;
        let row = &v[y * w..(y + 1) * w];
        for x in 0..(r + 1).min(w) {
            acc += row[x];
        }
        for x in 0..w {
            let lo = x.saturating_sub(r);
            let hi = (x + r).min(w - 1);
            tmp[y * w + x] = acc / (hi - lo + 1) as f32;
            if x + r + 1 < w {
                acc += row[x + r + 1];
            }
            if x >= r {
                acc -= row[x - r];
            }
        }
    }
    let mut out = vec![0.0; w * h];
    for x in 0..w {
        let mut acc = 0.0;
        for y in 0..(r + 1).min(h) {
            acc += tmp[y * w + x];
        }
        for y in 0..h {
            let lo = y.saturating_sub(r);
            let hi = (y + r).min(h - 1);
            out[y * w + x] = acc / (hi - lo + 1) as f32;
            if y + r + 1 < h {
                acc += tmp[(y + r + 1) * w + x];
            }
            if y >= r {
                acc -= tmp[(y - r) * w + x];
            }
        }
    }
    out
}

fn soften(m: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    box_blur(m, w, h, r)
}

fn iou(a: &[f32], b: &[f32]) -> f32 {
    let (mut i, mut u) = (0usize, 0usize);
    for (x, y) in a.iter().zip(b) {
        let (p, q) = (*x > 0.5, *y > 0.5);
        i += (p && q) as usize;
        u += (p || q) as usize;
    }
    i as f32 / u.max(1) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic models: a disc is the object / person, depth is high on it.
    struct Fake {
        person: bool,
    }
    fn disc(w: usize, h: usize) -> Vec<f32> {
        (0..w * h).map(|i| if ((i % w) as f32 - 40.0).hypot((i / w) as f32 - 60.0) < 15.0 { 1.0 } else { 0.0 }).collect()
    }
    impl Vision for Fake {
        fn object(&mut self, img: &Rgba8Image, _: &[Prompt]) -> Result<Vec<f32>, String> {
            Ok(disc(img.width as usize, img.height as usize))
        }
        fn people(&mut self, img: &Rgba8Image) -> Result<Vec<f32>, String> {
            let (w, h) = (img.width as usize, img.height as usize);
            Ok(if self.person { disc(w, h) } else { vec![0.0; w * h] })
        }
        fn depth(&mut self, img: &Rgba8Image) -> Result<Vec<f32>, String> {
            let (w, h) = (img.width as usize, img.height as usize);
            Ok(disc(w, h).iter().enumerate().map(|(i, d)| d * 0.9 + 0.1 * (i / w) as f32 / h as f32).collect())
        }
    }

    fn sky_picture(w: u32, h: u32) -> Rgba8Image {
        let mut img = Rgba8Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                let px = if y < h / 3 { [120, 170, 235, 255] } else if (x + y) % 2 == 0 { [60, 120, 40, 255] } else { [20, 70, 20, 255] };
                img.pixels[i..i + 4].copy_from_slice(&px);
            }
        }
        img
    }

    #[test]
    fn subject_is_people_when_there_are_some_else_the_nearest_object() {
        let img = Rgba8Image::new(100, 100);
        let m = mask_for(&mut Fake { person: true }, &img, Target::Subject, &[]).unwrap();
        assert!((m[60 * 100 + 40] - 1.0).abs() < 1e-6 && m[5 * 100 + 5] == 0.0);
        let m = mask_for(&mut Fake { person: false }, &img, Target::Subject, &[]).unwrap();
        assert!(m[60 * 100 + 40] > 0.9, "the near disc is the subject");
        let bg = mask_for(&mut Fake { person: true }, &img, Target::Background, &[]).unwrap();
        assert!(bg[5 * 100 + 5] > 0.9 && bg[60 * 100 + 40] < 0.1);
    }

    #[test]
    fn sky_is_the_smooth_light_top_and_not_the_textured_ground() {
        let img = sky_picture(120, 90);
        let m = mask_for(&mut NoVision, &img, Target::Sky, &[]).unwrap();
        assert!(m[10 * 120 + 60] > 0.9, "sky selected");
        assert!(m[70 * 120 + 60] < 0.1, "ground not selected");
        let none = mask_for(&mut NoVision, &Rgba8Image::new(50, 50), Target::Sky, &[]);
        assert!(none.is_err(), "a black picture has no sky");
    }

    #[test]
    fn an_object_needs_a_prompt_and_prompts_parse() {
        let img = Rgba8Image::new(10, 10);
        assert!(mask_for(&mut NoVision, &img, Target::Object, &[]).unwrap_err().contains("point"));
        let p = prompts_from(&serde_json::json!({"point": [3, 4], "exclude": [[1, 1]], "box": [0, 0, 5, 5]})).unwrap();
        assert_eq!(p.len(), 3);
        assert!(matches!(p[1], Prompt::Point { positive: false, .. }));
        assert_eq!(scale_prompts(&p, 100, 50)[0], Prompt::Point { x: 1.5, y: 2.0, positive: true });
    }
}
