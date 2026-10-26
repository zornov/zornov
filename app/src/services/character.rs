// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Rigged characters in a project, shared by the Inspector, the canvas
//! bone tool and the AI: a skeleton built from a layer's silhouette,
//! motions and poses written as bone keys, IK reaches baked into the
//! bones, the picture split into part layers that hang on its bones, and
//! swap sets (mouth shapes, hand poses, eye states) switched on the frame.
//!
//! The pure geometry is `rigging`; this module reads pictures, writes the
//! files parts are saved in and makes the edits (each one undo step).

use super::rigging::{self as rg, Silhouette};
use crate::doc::Doc;
use crate::editor::pins::LayerMap;
use crate::view::{self, SEC, Time};
use std::path::{Path, PathBuf};
use studio_model as m;

fn model_clip(doc: &Doc, id: u32) -> Result<m::Clip, String> {
    doc.model.active().clip(m::ClipId(id as u64)).cloned().ok_or_else(|| format!("no clip {id}"))
}

fn set_clip(doc: &mut Doc, label: &str, mc: m::Clip) -> Result<(), String> {
    let seq = doc.seq();
    if doc.apply(label, m::Edit::SetClip { seq, clip: Box::new(mc) }) { Ok(()) } else { Err(doc.error.take().unwrap_or_else(|| format!("{label} was refused"))) }
}

fn secs(t: Time) -> f64 {
    t as f64 / SEC as f64
}

fn frame_len(doc: &Doc) -> Time {
    (SEC as f64 / (doc.project.fps as f64).max(1.0)) as Time
}

/// The file behind a picture clip.
pub fn picture_path(doc: &Doc, mc: &m::Clip) -> Result<PathBuf, String> {
    let m::ClipSource::Media(r) = &mc.source else { return Err("only a picture clip can be rigged (this is a title, shape or nested clip)".into()) };
    let mi = doc.project.media(r.media.0 as u32).ok_or("the clip's media is gone")?;
    if mi.kind != view::MediaKind::Image {
        return Err("only a still picture (a PNG with transparency, a cut-out character) can be split or rigged from its silhouette".into());
    }
    mi.path.clone().ok_or_else(|| "that media has no file".into())
}

/// The whole picture as straight RGBA.
pub fn picture(path: &Path) -> Result<(usize, usize, Vec<u8>), String> {
    let img = crate::media_io::read_image(path)?;
    let [w, h] = img.size;
    let mut px = Vec::with_capacity(w * h * 4);
    for c in &img.pixels {
        px.extend_from_slice(&c.to_srgba_unmultiplied());
    }
    Ok((w, h, px))
}

/// How much of each pixel of a `w × h` RGBA picture is the character:
/// its transparency; else what differs from a plain backdrop; else the
/// object SAM (`segment`) finds in the middle of the picture. And how it
/// was found.
pub fn coverage(w: usize, h: usize, px: &[u8]) -> Result<(Vec<u8>, &'static str), String> {
    let alpha: Vec<u8> = px.chunks_exact(4).map(|p| p[3]).collect();
    if alpha.iter().any(|&a| a < 128) {
        return Ok((alpha, "the picture's transparency"));
    }
    if let Some(s) = Silhouette::of_rgba(w, h, px) {
        // The keyed shape, back at full size.
        let mut out = vec![0u8; w * h];
        let (kx, ky) = (s.w as f64 / w as f64, s.h as f64 / h as f64);
        for y in 0..h {
            for x in 0..w {
                if s.at(((x as f64 * kx) as usize).min(s.w - 1), ((y as f64 * ky) as usize).min(s.h - 1)) {
                    out[y * w + x] = 255;
                }
            }
        }
        return Ok((out, "the picture's backdrop colour"));
    }
    let sam = studio_ai::features::segment::ObjectCutout::load(&studio_ai::ModelStore::default()).map_err(|e| format!("the character has no transparency or plain backdrop, and the object model is not ready to find it ({e}): cut it out first (cut_out), or download Object cut-out in Settings ▸ Models"))?;
    let frame = studio_ai::Frame::new(w, h, px.to_vec()).map_err(|e| e.to_string())?;
    use studio_ai::features::segment::Prompt;
    let (fw, fh) = (w as f32, h as f32);
    let sel = sam.cut_out(&frame, &[Prompt::Point { x: fw / 2.0, y: fh / 2.0, positive: true }, Prompt::Box { x0: fw * 0.05, y0: fh * 0.03, x1: fw * 0.95, y1: fh * 0.97 }]).map_err(|e| e.to_string())?;
    if !sel.present {
        return Err("no character found in the middle of the picture: cut it out first (cut_out)".into());
    }
    Ok((sel.mask.data.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8).collect(), "SAM (the object in the middle of the picture)"))
}

/// The character's silhouette as the layer shows it (after crop): its
/// cut-out, or the picture's transparency or plain backdrop, or what SAM
/// finds in it.
pub fn silhouette(doc: &Doc, mc: &m::Clip) -> Result<Silhouette, String> {
    let c = &mc.transform.crop;
    let crop = |s: Silhouette| s.cropped(c.left, c.top, c.right, c.bottom).ok_or_else(|| "the crop leaves nothing of the character".to_owned());
    #[cfg(feature = "render")]
    if let Some(mt) = super::cutout::matte_of(mc) {
        use serde_json::Value;
        let file = mt.get("file").and_then(Value::as_str).unwrap_or_default();
        let (w, h) = (mt.get("width").and_then(Value::as_u64).unwrap_or(0) as usize, mt.get("height").and_then(Value::as_u64).unwrap_or(0) as usize);
        let invert = mt.get("invert").and_then(Value::as_bool).unwrap_or(false);
        if let Ok(plane) = studio_ai::features::matte_file::Reader::open(Path::new(file)).and_then(|mut r| r.plane(0)) {
            if let Some(s) = Silhouette::from_coverage(w, h, &plane, invert, "its cut-out") {
                return crop(s);
            }
        }
    }
    let (w, h, px) = picture(&picture_path(doc, mc)?)?;
    let (cover, from) = coverage(w, h, &px)?;
    crop(Silhouette::from_coverage(w, h, &cover, false, from).ok_or("can't find the character's silhouette: cut it out first (cut_out), or draw the bones yourself")?)
}

/// The layer's picture size in pixels after crop, at timeline time `at`.
fn pixels(doc: &Doc, id: u32, at: Time) -> Result<(f64, f64), String> {
    let map = LayerMap::at(&doc.project, &doc.model, id, at).ok_or_else(|| format!("clip {id} draws nothing at {:.2}s", secs(at)))?;
    let p = map.pixels();
    Ok((p.x.max(1.0) as f64, p.y.max(1.0) as f64))
}

/// Builds a skeleton of `kind` for clip `id` from its silhouette, facing
/// `facing` as the picture shows it: `Some(Some(true))` right,
/// `Some(Some(false))` left, `Some(None)` the camera, `None` a guess.
pub fn build(doc: &mut Doc, id: u32, kind: &str, facing: Option<Option<bool>>, at: Time) -> Result<String, String> {
    let mut mc = model_clip(doc, id)?;
    let s = silhouette(doc, &mc)?;
    let (w, h) = pixels(doc, id, at)?;
    let faces = facing.unwrap_or_else(|| s.facing());
    let right = faces.unwrap_or(true);
    let mut sk = rg::template(kind, &s, right, w / h)?;
    sk.faces_right = faces;
    // Facing the camera, knees and elbows bend toward it: no one-way limits
    // on the picture.
    if sk.faces_right.is_none() {
        for b in &mut sk.bones {
            if matches!(rg::role(&b.name), rg::Role::Shin(_) | rg::Role::Forearm(_)) {
                b.min = None;
                b.max = None;
            }
        }
    }
    // Keep the poses saved on an earlier skeleton.
    if let Some(old) = &mc.skeleton {
        sk.poses = old.poses.clone();
    }
    let n = sk.bones.len();
    mc.keyframes.retain(|k, _| !k.starts_with("bone.") && !k.starts_with("ik."));
    mc.skeleton = Some(sk);
    set_clip(doc, "Skeleton", mc)?;
    Ok(format!(
        "{n} bones for a {} from {}, facing {}",
        rg::kind_of(kind).unwrap_or(kind),
        s.from,
        match faces {
            Some(true) => "right",
            Some(false) => "left",
            None => "the camera",
        }
    ))
}

/// Writes `(clip seconds, value)` keys on `path` over `[from, to]`,
/// keeping the keys outside it (another act or pose).
fn write_keys(mc: &mut m::Clip, path: &str, from: f64, to: f64, keys: &[(f64, m::Value)]) {
    let local = |s: f64| (s * SEC as f64).round() as m::Micros;
    let mut track = mc.keyframes.remove(path).unwrap_or_else(|| m::Track::new(Vec::new()));
    track.keys.retain(|k| k.time < local(from) - 1000 || k.time > local(to) + 1000);
    for (t, v) in keys {
        track.set_key(m::Keyframe::new(local(*t), v.clone(), m::Interp::Auto));
    }
    if !track.keys.is_empty() {
        mc.keyframes.insert(path.to_owned(), track);
    }
}

/// What an act did.
pub struct Acted {
    pub bones: Vec<String>,
    /// Canvas pixels per second the layer should travel to keep its feet
    /// from sliding (a gait), in the way it faces.
    pub travel: Option<f64>,
    pub right: bool,
}

/// Bakes `acts` into the bones of clip `id` over `[from, from + dur]`
/// seconds of the clip, on top of the pose it has at `from`.
pub fn act(doc: &mut Doc, id: u32, acts: &[String], from: f64, dur: f64, speed: f64) -> Result<Acted, String> {
    let mut mc = model_clip(doc, id)?;
    let posed = mc.skeleton_keys_at((from * SEC as f64) as m::Micros).ok_or_else(|| format!("clip {id} has no skeleton: build one first"))?;
    if let Some(bad) = acts.iter().find(|a| !rg::known_act(a)) {
        return Err(format!("act \"{bad}\": {}", rg::ACTS));
    }
    let right = posed.faces_right.unwrap_or(true);
    let keys = rg::act_keys(acts, &posed, from, dur, right, speed, id as u64);
    if keys.rotations.is_empty() && keys.offsets.is_empty() {
        return Err(format!("no bone of this skeleton takes part in {} (bones: {})", acts.join(" + "), posed.bones.iter().map(|b| b.name.as_str()).collect::<Vec<_>>().join(", ")));
    }
    let mut moved = Vec::new();
    for (bone, ks) in &keys.rotations {
        let v: Vec<(f64, m::Value)> = ks.iter().map(|(t, d)| (*t, m::Value::Angle(*d))).collect();
        write_keys(&mut mc, &format!("bone.{bone}.rotation"), from, from + dur, &v);
        moved.push(posed.bone(*bone).map_or(bone.to_string(), |b| b.name.clone()));
    }
    for (bone, ks) in &keys.offsets {
        let v: Vec<(f64, m::Value)> = ks.iter().map(|(t, p)| (*t, m::Value::Point(*p))).collect();
        write_keys(&mut mc, &format!("bone.{bone}.offset"), from, from + dur, &v);
    }
    // An IK handle would hold the limbs it reaches with: let the act move
    // them.
    if let Some(sk) = &mut mc.skeleton {
        for h in &mut sk.ik {
            h.enabled = h.enabled && !keys.rotations.contains_key(&h.bone);
        }
    }
    set_clip(doc, "Act", mc)?;
    let at = doc.project.clip(id).map_or(0, |c| c.start + (from * SEC as f64) as Time);
    let travel = match keys.stride {
        Some(s) => {
            let map = LayerMap::at(&doc.project, &doc.model, id, at);
            map.map(|mp| {
                let top = mp.flat_canvas(m::Vec2::new(0.5, 0.0));
                let bottom = mp.flat_canvas(m::Vec2::new(0.5, 1.0));
                s * (bottom - top).length() as f64
            })
        }
        None => None,
    };
    Ok(Acted { bones: moved, travel, right })
}

/// Sets bones' rotations (degrees from rest) and offsets at `at`: keys
/// when `key` (or when the bone is already animated), the static pose
/// otherwise.
pub fn pose(doc: &mut Doc, id: u32, rotations: &[(u32, f64)], offsets: &[(u32, m::Vec2)], at: Time, key: bool) -> Result<(), String> {
    let mut mc = model_clip(doc, id)?;
    let vc = doc.project.clip(id).cloned().ok_or("no clip")?;
    let local = (at - vc.start).clamp(0, vc.dur);
    let frame = frame_len(doc);
    let sk = mc.skeleton.clone().ok_or_else(|| format!("clip {id} has no skeleton"))?;
    for &(b, deg) in rotations {
        let bone = sk.bone(b).ok_or_else(|| format!("no bone {b}"))?;
        let path = format!("bone.{b}.rotation");
        let v = m::Value::Angle(bone.limit(deg));
        if key && !mc.keyframes.contains_key(&path) {
            mc.keyframes.insert(path, m::Track::new(vec![m::Keyframe::new(local, v, m::Interp::Ease)]));
        } else {
            crate::editor::xform::put(&mut mc, &path, v, local, frame);
        }
    }
    for &(b, off) in offsets {
        let path = format!("bone.{b}.offset");
        let v = m::Value::Point(off);
        if key && !mc.keyframes.contains_key(&path) {
            mc.keyframes.insert(path, m::Track::new(vec![m::Keyframe::new(local, v, m::Interp::Ease)]));
        } else {
            crate::editor::xform::put(&mut mc, &path, v, local, frame);
        }
    }
    set_clip(doc, "Pose", mc)
}

/// The rotations that bring the tail of `bone` (with `chain` bones above
/// it) to `target` (layer coordinates), from the pose at `at`: IK solved
/// once and kept as rotations, within each bone's limits.
pub fn reach(doc: &Doc, id: u32, bone: u32, chain: u32, target: m::Vec2, flip: bool, at: Time) -> Result<Vec<(u32, f64)>, String> {
    let mc = model_clip(doc, id)?;
    let vc = doc.project.clip(id).ok_or("no clip")?;
    let mut sk = mc.skeleton_at((at - vc.start).clamp(0, vc.dur)).ok_or_else(|| format!("clip {id} has no skeleton"))?;
    sk.bone(bone).ok_or_else(|| format!("no bone {bone}"))?;
    sk.ik = vec![m::IkHandle { id: 1, name: String::new(), bone, chain: chain.max(1), target, blend: 1.0, flip, enabled: true, ..Default::default() }];
    let before: Vec<(u32, f64)> = sk.bones.iter().map(|b| (b.id, b.rotation)).collect();
    sk.solve();
    Ok(sk.bones.iter().filter(|b| before.iter().any(|(i, r)| *i == b.id && (r - b.rotation).abs() > 1e-9)).map(|b| (b.id, b.rotation)).collect())
}

/// What [`retarget`] did.
pub struct Retargeted {
    /// Bones of the target that got keys.
    pub bones: Vec<String>,
    pub keys: usize,
}

/// A skeleton's bones as segments in picture pixels (x scaled by the
/// aspect), for comparing rigs of other proportions.
fn segments(sk: &m::Skeleton) -> Vec<([f64; 2], [f64; 2])> {
    sk.forward().bones.iter().map(|b| ([b.head.x * sk.aspect, b.head.y], [b.tail.x * sk.aspect, b.tail.y])).collect()
}

/// How far the rig reaches down from its root at rest, for scaling travel.
fn leg_reach(sk: &m::Skeleton, rest: &[([f64; 2], [f64; 2])]) -> f64 {
    let top = rest.first().map_or(0.0, |s| s.0[1]);
    rest.iter().map(|s| s.1[1]).fold(top, f64::max) - top
}

/// Moves the animation of clip `from` (its skeleton's keys, IK included)
/// onto the skeleton of clip `to`, which may have other proportions and
/// other rest poses: each target bone points the way its partner (found by
/// role, or by `map`: target name → source name) points, and the root's
/// travel is scaled by leg length. Keys are written every 1/15 s from
/// `start` for `duration` seconds (default: the rest of the source clip),
/// on the target clip from its start.
pub fn retarget(doc: &mut Doc, from: u32, to: u32, start: f64, duration: Option<f64>, map: &[(String, String)]) -> Result<Retargeted, String> {
    use studio_rig::retarget as rt;
    let src = model_clip(doc, from)?;
    let mut dst = model_clip(doc, to)?;
    let (sv, dv) = (doc.project.clip(from).cloned().ok_or("no source clip")?, doc.project.clip(to).cloned().ok_or("no target clip")?);
    let ssk = src.skeleton.clone().ok_or_else(|| format!("clip {from} has no skeleton"))?;
    let dsk = dst.skeleton.clone().ok_or_else(|| format!("clip {to} has no skeleton"))?;
    let (mut s_rest, mut d_rest) = (ssk.clone(), dsk.clone());
    for sk in [&mut s_rest, &mut d_rest] {
        for b in &mut sk.bones {
            b.rotation = 0.0;
            b.offset = m::Vec2::ZERO;
            b.stretch = 1.0;
        }
    }
    let (s_segs, d_segs) = (segments(&s_rest), segments(&d_rest));
    let names = |sk: &m::Skeleton| sk.bones.iter().map(|b| b.name.clone()).collect::<Vec<_>>();
    let (sn, dn) = (names(&ssk), names(&dsk));
    let mut pairs = studio_rig::names::pair_by_role(&sn.iter().map(String::as_str).collect::<Vec<_>>(), ssk.faces_right, &dn.iter().map(String::as_str).collect::<Vec<_>>(), dsk.faces_right);
    for (d, s) in map {
        let (Some(di), Some(si)) = (dn.iter().position(|n| n.eq_ignore_ascii_case(d)), sn.iter().position(|n| n.eq_ignore_ascii_case(s))) else { return Err(format!("map: no bone \"{d}\" on the target or \"{s}\" on the source")) };
        pairs.retain(|p| p.0 != di);
        pairs.push((di, si));
    }
    if pairs.is_empty() {
        return Err("no bones of the two rigs share a role: name them by role (spine, head, upper_arm.front, shin.back…) or give \"map\" {target bone: source bone}".into());
    }
    // A character facing the other way is mirrored.
    let mirror = matches!((ssk.faces_right, dsk.faces_right), (Some(a), Some(b)) if a != b);
    let flip = |segs: Vec<([f64; 2], [f64; 2])>| -> Vec<([f64; 2], [f64; 2])> { if mirror { segs.into_iter().map(|(a, b)| ([-a[0], a[1]], [-b[0], b[1]])).collect() } else { segs } };
    let s_rest_m = flip(s_segs.clone());
    let k = rt::travel_scale(leg_reach(&ssk, &s_segs), leg_reach(&dsk, &d_segs));
    let root_pair = pairs.iter().find(|p| dsk.bones[p.0].parent.is_none()).copied();
    let dur = duration.unwrap_or(secs(sv.dur) - start).min(secs(sv.dur) - start).min(secs(dv.dur));
    if dur <= 0.0 {
        return Err("nothing to retarget: \"from\" is past the source clip's end".into());
    }
    let step = 1.0 / 15.0;
    let n = ((dur / step).ceil() as usize + 1).clamp(2, 1200);
    let mut rot: Vec<Vec<m::Keyframe>> = vec![Vec::new(); dsk.bones.len()];
    let mut off: Vec<m::Keyframe> = Vec::new();
    let to_t = |x: f64| (x * SEC as f64).round() as Time;
    for i in 0..n {
        let t = (i as f64 * step).min(dur);
        let Some(now) = src.skeleton_at(to_t(start + t).clamp(0, sv.dur)) else { continue };
        let mut now = now;
        now.solve();
        let s_now = flip(segments(&now));
        let dirs = rt::directions(&s_rest_m, &s_now, &d_segs, &pairs, rt::Mode::Relative);
        let mut pose = d_rest.clone();
        for bi in 0..pose.bones.len() {
            let Some(dir) = dirs[bi] else { continue };
            let turn = pose.bones[bi].parent.and_then(|p| pose.forward().bone(p).map(|b| b.turn)).unwrap_or(0.0);
            let rest = (d_segs[bi].1[1] - d_segs[bi].0[1]).atan2(d_segs[bi].1[0] - d_segs[bi].0[0]).to_degrees();
            let world = dir[1].atan2(dir[0]).to_degrees();
            let b = &mut pose.bones[bi];
            b.rotation = b.limit(m::wrap(world - turn - rest));
            rot[bi].push(m::Keyframe::new(to_t(t), m::Value::Angle(b.rotation), m::Interp::Linear));
        }
        if let Some((d, s)) = root_pair {
            let (a, b) = (s_now[s].0, s_rest_m[s].0);
            let a_px = [(a[0] - b[0]) * k, (a[1] - b[1]) * k];
            off.push(m::Keyframe::new(to_t(t), m::Value::Point(m::Vec2::new(a_px[0] / dsk.aspect.max(1e-6), a_px[1])), m::Interp::Linear));
        }
    }
    let mut done = Vec::new();
    for (bi, keys) in rot.into_iter().enumerate() {
        if keys.len() < 2 {
            continue;
        }
        done.push(dsk.bones[bi].name.clone());
        dst.keyframes.insert(format!("bone.{}.rotation", dsk.bones[bi].id), m::Track::new(keys));
    }
    let total = done.len() * n;
    if let (Some((d, _)), true) = (root_pair, off.len() >= 2) {
        dst.keyframes.insert(format!("bone.{}.offset", dsk.bones[d].id), m::Track::new(off));
    }
    set_clip(doc, "Retarget", dst)?;
    Ok(Retargeted { bones: done, keys: total })
}

/// Clip `child` follows bone `bone` of clip `parent` (and the parent
/// itself), from the moment `at`.
pub fn link_to_bone(doc: &mut Doc, child: u32, parent: u32, bone: u32, at: Time) -> Result<(), String> {
    super::layers::set_parent(doc, child as u64, Some(parent as u64), at)?;
    let map = LayerMap::at(&doc.project, &doc.model, parent, at).ok_or("the parent draws nothing there")?;
    let mut mc = model_clip(doc, child)?;
    let p = mc.parent.as_mut().ok_or("not linked")?;
    let (s, c) = (-p.rest_rotation).to_radians().sin_cos();
    let local = |uv: m::Vec2| {
        let q = map.flat_canvas(uv);
        let (dx, dy) = (q.x as f64 - p.rest_position.x, q.y as f64 - p.rest_position.y);
        m::Vec2::new(dx * c - dy * s, dx * s + dy * c)
    };
    let o = local(m::Vec2::ZERO);
    let (ux, vy) = (local(m::Vec2::new(1.0, 0.0)), local(m::Vec2::new(0.0, 1.0)));
    p.bone = Some(m::BoneLink { bone, origin: o, span: m::Vec2::new(ux.x - o.x, vy.y - o.y) });
    set_clip(doc, "Follow bone", mc)
}

/// Where part pictures go: the project's `Parts` folder (tests: a
/// temporary one).
pub fn parts_dir(doc: &Doc) -> PathBuf {
    let home = if cfg!(test) { std::env::temp_dir().join("studio-tests") } else { studio_paths::studio() };
    doc.folder.as_ref().map(|f| f.root().join("Parts")).unwrap_or_else(|| home.join("Parts"))
}

fn write_png(path: &Path, w: usize, h: usize, rgba: &[u8]) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().map_err(|e| e.to_string())?;
    wr.write_image_data(rgba).map_err(|e| e.to_string())
}

fn slug(s: &str) -> String {
    let out: String = s.chars().map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    out.trim_matches('-').to_owned()
}

/// A layer made by a rig: a picture on its own lane above `below`, with
/// the rig's placement and look, following bone `bone`.
fn add_layer(doc: &mut Doc, rig: u32, file: &Path, name: &str, bone: u32, lane_above: m::LaneId, at: Time) -> Result<(u32, m::LaneId), String> {
    let report = super::project::import_media(doc, &[file.to_path_buf()]);
    let media = *report.ids.first().ok_or_else(|| report.failed.join("; "))?;
    let rigc = model_clip(doc, rig)?;
    let seq = doc.seq();
    let sq = doc.model.sequence(seq).ok_or("no sequence")?;
    let index = sq.lane_index(lane_above).ok_or("the rig's lane is gone")?;
    let lane = m::LaneId(doc.alloc_id());
    if !doc.apply("Add lane", m::Edit::InsertLane { seq, index, lane: m::Lane::new(lane, m::LaneKind::Visual, name) }) {
        return Err(doc.error.take().unwrap_or_else(|| "could not add a lane".into()));
    }
    let id = doc.alloc_id();
    let mut c = m::Clip::new(m::ClipId(id), lane, rigc.start, rigc.duration, m::ClipSource::Media(m::MediaRef { media: m::MediaId(media as u64), ..media_ref(&rigc) }));
    c.name = name.to_owned();
    // Drawn where the rig is drawn at rest, and carried by it from then.
    let placed = doc.model.active().placed_transform(&rigc, at);
    c.transform = placed;
    c.transform.opacity = 1.0;
    c.grade = rigc.grade.clone();
    c.effects = rigc.effects.clone();
    c.blend = rigc.blend;
    c.meta.insert("part".into(), serde_json::json!({"rig": rig, "bone": bone}));
    if !doc.apply("Add part", m::Edit::InsertClip { seq, clip: Box::new(c) }) {
        return Err(doc.error.take().unwrap_or_else(|| "could not add the part".into()));
    }
    link_to_bone(doc, id as u32, rig, bone, at)?;
    Ok((id as u32, lane))
}

fn media_ref(c: &m::Clip) -> m::MediaRef {
    match &c.source {
        m::ClipSource::Media(r) => r.clone(),
        _ => unreachable!("a picture clip"),
    }
}

/// Splits clip `id`'s picture into part layers, one per bone that bends
/// it (head, torso, arms, legs, tail…), each on its own lane above the
/// rig and following its bone; with `face`, the eyes and mouth come off
/// the head as their own layers too (the face under them patched), as the
/// first options of their swap sets (`mouth` "closed", `eye.front` "open"…),
/// named as the face tools look for them. The rig layer
/// itself is hidden (opacity 0): it carries the bones.
pub fn split_layers(doc: &mut Doc, id: u32, at: Time, face: bool) -> Result<Vec<(u32, String)>, String> {
    split_layers_noted(doc, id, at, face).map(|(parts, _)| parts)
}

/// Moves the limb bones of a person onto the pose landmarks found in the
/// picture (shoulder to elbow to wrist, hip to knee to ankle, shoulders to
/// hips), each to the pair of landmarks nearest to where the bone already
/// is, so a template skeleton fitted to a silhouette follows the real body
/// (bent elbows, a raised arm). Returns how many bones moved; `None` when
/// the pose model is not installed or finds no person.
fn snap_to_pose(sk: &mut m::Skeleton, w: usize, h: usize, px: &[u8]) -> Option<usize> {
    use studio_ai::features::faces::{PoseTracker, body};
    let tracker = PoseTracker::load(&studio_ai::ModelStore::default()).ok()?;
    let frame = studio_ai::Frame::new(w, h, px.to_vec()).ok()?;
    let pose = tracker.detect(&frame).ok()??;
    let lm = |i: usize| {
        let l = pose.landmarks.get(i)?;
        (l.visibility >= 0.5 && l.presence >= 0.5).then(|| m::Vec2::new((l.x / w as f32) as f64, (l.y / h as f32) as f64))
    };
    // (parent landmark, child landmark) per limb segment, both sides.
    let upper_arm = [(11, 13), (12, 14)];
    let forearm = [(13, 15), (14, 16)];
    let thigh = [(23, 25), (24, 26)];
    let shin = [(25, 27), (26, 28)];
    let _ = body::COUNT;
    let mid = |a: m::Vec2, b: m::Vec2| m::Vec2::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
    let dist = |a: m::Vec2, b: m::Vec2| ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
    let mut moved = 0;
    for b in sk.bones.iter_mut().filter(|b| b.deform) {
        let pairs: &[(usize, usize)] = match rg::role(&b.name) {
            rg::Role::UpperArm(_) => &upper_arm,
            rg::Role::Forearm(_) => &forearm,
            rg::Role::Thigh(_) => &thigh,
            rg::Role::Shin(_) => &shin,
            rg::Role::Spine => {
                if let (Some(a), Some(c), Some(d), Some(e)) = (lm(11), lm(12), lm(23), lm(24)) {
                    let (top, bottom) = (mid(a, c), mid(d, e));
                    b.head = bottom;
                    b.tail = top;
                    moved += 1;
                }
                continue;
            }
            _ => continue,
        };
        let here = mid(b.head, b.tail);
        let reach = 0.2;
        let best = pairs
            .iter()
            .filter_map(|&(p, c)| Some((lm(p)?, lm(c)?)))
            .map(|(p, c)| (dist(mid(p, c), here), p, c))
            .filter(|(d, _, _)| *d < reach)
            .min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((_, p, c)) = best {
            // Keep the bone pointing the way it did (head to tail).
            let keep = dist(p, b.head) + dist(c, b.tail) <= dist(c, b.head) + dist(p, b.tail);
            (b.head, b.tail) = if keep { (p, c) } else { (c, p) };
            moved += 1;
        }
    }
    (moved > 0).then_some(moved)
}

/// SAM 2.1 outlines of the limbs: for each arm and leg bone, clicks along
/// the bone (on it) and on the other limbs and the trunk (not it), and the
/// bone's box. A mask that is a plausible limb (neither a speck nor most
/// of the body) comes back, picture-sized, for the split to follow.
fn limb_hints(sk: &m::Skeleton, w: usize, h: usize, sam: &studio_ai::features::segment::ObjectCutout, emb: &studio_ai::features::segment::ImageEmbedding, silhouette: usize) -> Vec<(u32, Vec<f32>)> {
    use studio_ai::features::segment::Prompt;
    let bones: Vec<&m::Bone> = sk.bones.iter().filter(|b| b.deform).collect();
    let at = |b: &m::Bone, t: f64| ((b.head.x + (b.tail.x - b.head.x) * t) as f32 * w as f32, (b.head.y + (b.tail.y - b.head.y) * t) as f32 * h as f32);
    let limb = |b: &m::Bone| matches!(rg::role(&b.name), rg::Role::UpperArm(_) | rg::Role::Forearm(_) | rg::Role::Thigh(_) | rg::Role::Shin(_));
    let mut out = Vec::new();
    for b in bones.iter().filter(|b| limb(b)) {
        let mut prompts: Vec<Prompt> = [0.25, 0.5, 0.75].iter().map(|&t| at(b, t)).map(|(x, y)| Prompt::Point { x, y, positive: true }).collect();
        for o in bones.iter().filter(|o| o.id != b.id && (limb(o) || rg::role(&o.name) == rg::Role::Spine)) {
            // The neighbours it joins share a joint; their middle is far enough off.
            let (x, y) = at(o, 0.5);
            prompts.push(Prompt::Point { x, y, positive: false });
        }
        let (a, z) = (at(b, 0.0), at(b, 1.0));
        let pad = 0.04 * w.max(h) as f32;
        prompts.push(Prompt::Box { x0: a.0.min(z.0) - pad, y0: a.1.min(z.1) - pad, x1: a.0.max(z.0) + pad, y1: a.1.max(z.1) + pad });
        let Ok(sel) = sam.segment(emb, &prompts) else { continue };
        let area = sel.mask.data.iter().filter(|&&v| v > 0.5).count();
        if sel.mask.data.len() == w * h && area * 300 > silhouette && area * 3 < silhouette * 2 {
            out.push((b.id, sel.mask.data.clone()));
        }
    }
    out
}

/// [`split_layers`] and what it did, for the reply: bones moved onto pose
/// landmarks, limbs outlined by SAM, pixels filled behind parts, holes
/// closed.
pub fn split_layers_noted(doc: &mut Doc, id: u32, at: Time, face: bool) -> Result<(Vec<(u32, String)>, Vec<String>), String> {
    let mc = model_clip(doc, id)?;
    let sk = mc.skeleton.clone().ok_or_else(|| format!("clip {id} has no skeleton: build one first"))?;
    if doc.model.active().clips.iter().any(|c| c.meta.get("part").and_then(|p| p.get("rig")).and_then(|r| r.as_u64()) == Some(id as u64)) {
        return Err(format!("clip {id} is split already (its parts are on the lanes above it)"));
    }
    let path = picture_path(doc, &mc)?;
    let (w, h, mut px) = picture(&path)?;
    // Only the character goes into parts (a picture with a backdrop is
    // cut out first).
    let (cover, _) = coverage(w, h, &px)?;
    for (p, c) in px.chunks_exact_mut(4).zip(&cover) {
        p[3] = p[3].min(*c);
    }
    // Bones in picture coordinates (the layer's span is after crop).
    let cr = mc.transform.crop.clone();
    let to_src = |p: m::Vec2| m::Vec2::new(cr.left + p.x * (1.0 - cr.left - cr.right), cr.top + p.y * (1.0 - cr.top - cr.bottom));
    let mut src = sk.clone();
    for b in &mut src.bones {
        b.head = to_src(b.head);
        b.tail = to_src(b.tail);
    }
    let mut notes: Vec<String> = Vec::new();
    // The models sharpen the guess when they are installed (not in tests).
    let sam = if cfg!(test) { None } else { studio_ai::features::segment::ObjectCutout::load(&studio_ai::ModelStore::default()).ok() };
    let emb = sam.as_ref().and_then(|m| studio_ai::Frame::new(w, h, px.clone()).ok().and_then(|f| m.embed(&f).ok()));
    if !cfg!(test) {
        if let Some(n) = snap_to_pose(&mut src, w, h, &px) {
            notes.push(format!("{n} bones moved onto the pose landmarks found in the picture"));
        }
    }
    let silhouette = px.chunks_exact(4).filter(|p| p[3] >= 8).count();
    let hints = match (&sam, &emb) {
        (Some(s), Some(e)) => limb_hints(&src, w, h, s, e, silhouette),
        _ => Vec::new(),
    };
    if !hints.is_empty() {
        notes.push(format!("SAM outlined {} limb(s); the part boundaries follow those outlines", hints.len()));
    }
    let (mut parts, report) = rg::split_clean(&px, w, h, &src, &hints, &Default::default())?;
    notes.push(format!(
        "clean split: every picture pixel is in exactly one part; {} hidden pixels filled behind the parts in front of them, {} holes closed, {} specks absorbed, {} loose pieces joined",
        report.filled, report.holes_assigned, report.flipped, report.islands_merged
    ));
    // Eyes and mouth off the head.
    let mut face_layers: Vec<(String, Vec<u8>, u32)> = Vec::new();
    if face {
        if let (Some(head), Some(hp)) = (src.bones.iter().find(|b| rg::role(&b.name) == rg::Role::Head), parts.iter().position(|p| rg::role(&p.name) == rg::Role::Head)) {
            let side = sk.faces_right.is_some();
            // SAM finds each feature's own outline near where it should be;
            // without it (or when it finds the whole head) an ellipse there.
            // The head's own pixels say where its face is.
            let (mut lo, mut hi) = ((w, h), (0usize, 0usize));
            for y in 0..h {
                for x in 0..w {
                    if parts[hp].rgba[(y * w + x) * 4 + 3] > 0 {
                        lo = (lo.0.min(x), lo.1.min(y));
                        hi = (hi.0.max(x + 1), hi.1.max(y + 1));
                    }
                }
            }
            let (lo, hi) = (m::Vec2::new(lo.0 as f64 / w as f64, lo.1 as f64 / h as f64), m::Vec2::new(hi.0 as f64 / w as f64, hi.1 as f64 / h as f64));
            for (name, c, half) in rg::face_boxes(lo, hi, side, sk.faces_right.unwrap_or(true)) {
                let mut layer = vec![0u8; w * h * 4];
                let mut hole = vec![false; w * h];
                let mut n = 0;
                let (cx, cy, hx, hy) = (c.x * w as f64, c.y * h as f64, half.x * w as f64 * 1.6, half.y * h as f64 * 1.6);
                let found: Option<Vec<f32>> = match (&sam, &emb) {
                    (Some(m), Some(e)) => {
                        use studio_ai::features::segment::Prompt;
                        let p = [Prompt::Point { x: cx as f32, y: cy as f32, positive: true }, Prompt::Box { x0: (cx - hx) as f32, y0: (cy - hy) as f32, x1: (cx + hx) as f32, y1: (cy + hy) as f32 }];
                        m.segment(e, &p).ok().map(|s| s.mask.data).filter(|d| {
                            // A feature fills some of its box, not all of it.
                            let (mut inside, mut all) = (0usize, 0usize);
                            for y in ((cy - hy).max(0.0) as usize)..((cy + hy).min(h as f64) as usize) {
                                for x in ((cx - hx).max(0.0) as usize)..((cx + hx).min(w as f64) as usize) {
                                    all += 1;
                                    inside += (d[y * w + x] > 0.5) as usize;
                                }
                            }
                            all > 0 && inside * 20 > all && inside * 10 < all * 9
                        })
                    }
                    _ => None,
                };
                for y in 0..h {
                    for x in 0..w {
                        let (u, v) = ((x as f64 + 0.5) / w as f64, (y as f64 + 0.5) / h as f64);
                        let e = ((u - c.x) / half.x.max(1e-6)).powi(2) + ((v - c.y) / half.y.max(1e-6)).powi(2);
                        let i = (y * w + x) * 4;
                        let mine = match &found {
                            Some(d) => d[y * w + x] > 0.5 && ((x as f64 - cx) / hx).abs() <= 1.0 && ((y as f64 - cy) / hy).abs() <= 1.0,
                            None => e <= 1.0,
                        };
                        if mine && parts[hp].rgba[i + 3] > 0 {
                            layer[i..i + 4].copy_from_slice(&parts[hp].rgba[i..i + 4]);
                            hole[y * w + x] = true;
                            n += 1;
                        }
                    }
                }
                if n > 16 {
                    rg::patch(&mut parts[hp].rgba, w, h, &hole);
                    face_layers.push((name, layer, head.id));
                }
            }
        }
    }
    let dir = parts_dir(doc).join(format!("{}-{}", slug(&mc.name), id));
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    // Back parts first: each new lane goes above the last.
    parts.sort_by(|a, b| {
        let d = |p: &rg::Part| sk.bone(p.bone).map_or(0.0, |b| b.depth) + if rg::role(&p.name) == rg::Role::Head { 0.25 } else { 0.0 };
        d(a).total_cmp(&d(b))
    });
    let mut lane = mc.lane;
    let mut out = Vec::new();
    for p in &parts {
        let file = dir.join(format!("{}-{stamp}.png", slug(&p.name)));
        write_png(&file, w, h, &p.rgba)?;
        let (cid, l) = add_layer(doc, id, &file, &format!("{} · {}", mc.name, p.name), p.bone, lane, at)?;
        lane = l;
        out.push((cid, p.name.clone()));
    }
    for (name, rgba, bone) in &face_layers {
        let file = dir.join(format!("{}-{stamp}.png", slug(name)));
        write_png(&file, w, h, rgba)?;
        // Named as the face tools look for them ("mouth closed", "eye
        // front"): lip sync and blinks find them on the rig.
        let (set, option) = if name.starts_with("eye") { (name.clone(), "open") } else { ("mouth".to_owned(), "closed") };
        let label = if set == "mouth" { "mouth closed".to_owned() } else { name.replace('.', " ") };
        let (cid, l) = add_layer(doc, id, &file, &format!("{} · {label}", mc.name), *bone, lane, at)?;
        lane = l;
        let mut c = model_clip(doc, cid)?;
        rg::set_swap_tag(&mut c, id as u64, &set, option);
        set_clip(doc, "Swap set", c)?;
        out.push((cid, name.clone()));
    }
    // The rig hides: its parts show it.
    let mut rigc = model_clip(doc, id)?;
    rigc.keyframes.remove("transform.opacity");
    rigc.transform.opacity = 0.0;
    // The picture as the split saw it (cut to the character), kept so the
    // cut can be checked against it later (`parts_check`).
    let source = dir.join(format!("source-{stamp}.png"));
    write_png(&source, w, h, &px)?;
    rigc.meta.insert("rig".into(), serde_json::json!({"parts": out.iter().map(|(c, _)| c).collect::<Vec<_>>(), "source": source.display().to_string()}));
    set_clip(doc, "Split into parts", rigc)?;
    Ok((out, notes))
}

/// The side-by-side check of a split rig: the picture it was split from,
/// its parts stacked back to front, and where the stack differs. `crop` is
/// `[x0, y0, x1, y1]` in the picture's pixels (the character's box when
/// `None`).
pub fn parts_check(doc: &Doc, rig: u32, crop: Option<[usize; 4]>, panel: usize) -> Result<studio_ai::features::parts::Diagnosis, String> {
    let mc = model_clip(doc, rig)?;
    let info = mc.meta.get("rig").ok_or_else(|| format!("clip {rig} has not been split into parts (the parts op does that)"))?;
    let source = info.get("source").and_then(|v| v.as_str()).ok_or("this split was made before the check existed: split again (parts) to check it")?;
    let (w, h, px) = picture(Path::new(source))?;
    let ids: Vec<u32> = info.get("parts").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_u64()).map(|v| v as u32).collect()).unwrap_or_default();
    let mut layers: Vec<(String, Vec<u8>)> = Vec::new();
    for id in ids {
        let Ok(c) = model_clip(doc, id) else { continue };
        // A swap option that is not showing is not part of the stack.
        if c.transform.opacity <= 0.0 && c.keyframes.get("transform.opacity").is_none() {
            continue;
        }
        let Ok(path) = picture_path(doc, &c) else { continue };
        let Ok((pw, ph, rgba)) = picture(&path) else { continue };
        if (pw, ph) != (w, h) {
            continue;
        }
        layers.push((c.name.rsplit(" · ").next().unwrap_or(&c.name).to_owned(), rgba));
    }
    if layers.is_empty() {
        return Err(format!("clip {rig} has no part pictures to check (were they deleted?)"));
    }
    let frame = studio_ai::Frame::new(w, h, px).map_err(|e| e.to_string())?;
    Ok(studio_ai::features::parts::diagnose(&frame, &layers, crop, panel))
}

/// The members of swap set `set` of rig `rig`: `(clip, option)`.
pub fn swap_members(doc: &Doc, rig: u32, set: &str) -> Vec<(u32, String)> {
    doc.model.active().clips.iter().filter_map(|c| rg::swap_tag(c).filter(|(r, s, _)| *r == rig as u64 && s.eq_ignore_ascii_case(set)).map(|(_, _, o)| (c.id.0 as u32, o))).collect()
}

/// Every swap set of rig `rig` with its options.
pub fn swap_sets(doc: &Doc, rig: u32) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for c in &doc.model.active().clips {
        if let Some((r, s, o)) = rg::swap_tag(c).filter(|(r, _, _)| *r == rig as u64) {
            let _ = r;
            match out.iter_mut().find(|(n, _)| *n == s) {
                Some((_, opts)) => opts.push(o),
                None => out.push((s, vec![o])),
            }
        }
    }
    out
}

/// Adds option `option` to swap set `set` of rig `rig`: the picture
/// `file` on its own lane, following bone `bone`. A picture the size of
/// the rig's lines up with it; a smaller one is centred on `centre` (rig
/// layer coordinates) and sized to `width` of the rig's width. The new
/// option starts hidden unless it is the set's first.
pub fn add_swap(doc: &mut Doc, rig: u32, set: &str, option: &str, file: &Path, bone: u32, centre: Option<m::Vec2>, width: Option<f64>, at: Time) -> Result<u32, String> {
    let rigc = model_clip(doc, rig)?;
    let members = swap_members(doc, rig, set);
    if members.iter().any(|(_, o)| o.eq_ignore_ascii_case(option)) {
        return Err(format!("swap set \"{set}\" already has \"{option}\""));
    }
    // Above the highest member (or the rig's top part).
    let top_lane = members.iter().filter_map(|(c, _)| doc.model.active().clip(m::ClipId(*c as u64)).map(|c| c.lane)).chain(std::iter::once(rigc.lane)).min_by_key(|l| doc.model.active().lane_index(*l).unwrap_or(usize::MAX)).unwrap_or(rigc.lane);
    let (cid, _) = add_layer(doc, rig, file, &format!("{} · {} {option}", rigc.name, set.replace('.', " ")), bone, top_lane, at)?;
    let mut c = model_clip(doc, cid)?;
    // A small picture: placed on its spot of the rig.
    let (pw, ph) = picture(file).map(|(w, h, _)| (w as f64, h as f64)).unwrap_or((1.0, 1.0));
    let rig_px = pixels(doc, rig, at)?;
    if (pw - rig_px.0).abs() > 2.0 || (ph - rig_px.1).abs() > 2.0 {
        let map = LayerMap::at(&doc.project, &doc.model, rig, at).ok_or("the rig draws nothing there")?;
        let centre = centre.ok_or_else(|| format!("this picture ({pw}×{ph}) is not the rig's size ({}×{}): say where it goes (\"at\" on the rig, layer coordinates or canvas pixels) and how wide", rig_px.0, rig_px.1))?;
        let at_c = map.flat_canvas(centre);
        let across = (map.flat_canvas(m::Vec2::new(1.0, 0.5)) - map.flat_canvas(m::Vec2::new(0.0, 0.5))).length() as f64;
        let want = width.unwrap_or(0.2) * across;
        let fit = doc.project.media(match &c.source {
            m::ClipSource::Media(r) => r.media.0 as u32,
            _ => 0,
        });
        let canvas = (doc.project.width.max(1) as f64, doc.project.height.max(1) as f64);
        let fit = fit.map_or(1.0, |mi| (canvas.0 / mi.width.max(1) as f64).min(canvas.1 / mi.height.max(1) as f64));
        let k = want / (pw * fit).max(1e-6);
        c.transform.scale_x = k;
        c.transform.scale_y = k;
        c.transform.anchor = m::Vec2::ZERO;
        c.transform.position = m::Vec2::new(at_c.x as f64, at_c.y as f64);
        c.transform.crop = m::Crop::default();
        set_clip(doc, "Place swap", c.clone())?;
        // Linked again from its new place.
        link_to_bone(doc, cid, rig, bone, at)?;
        c = model_clip(doc, cid)?;
    }
    rg::set_swap_tag(&mut c, rig as u64, set, option);
    if !members.is_empty() {
        c.transform.opacity = 0.0;
    }
    set_clip(doc, "Swap set", c)?;
    Ok(cid)
}

/// Shows option `keys[i].1` of swap set `set` from timeline second
/// `keys[i].0` on (a cut on the frame).
pub fn swap(doc: &mut Doc, rig: u32, set: &str, keys: &[(f64, String)]) -> Result<(), String> {
    let members = swap_members(doc, rig, set);
    if members.is_empty() {
        let sets = swap_sets(doc, rig);
        return Err(format!("rig {rig} has no swap set \"{set}\"{}", if sets.is_empty() { String::new() } else { format!("; sets: {}", sets.iter().map(|(s, o)| format!("{s} ({})", o.join(", "))).collect::<Vec<_>>().join("; ")) }));
    }
    for (_, o) in keys {
        if !members.iter().any(|(_, m)| m.eq_ignore_ascii_case(o)) {
            return Err(format!("swap set \"{set}\" has no \"{o}\"; options: {}", members.iter().map(|(_, o)| o.as_str()).collect::<Vec<_>>().join(", ")));
        }
    }
    let clips: Vec<(m::Clip, f64, String)> = members.iter().filter_map(|(c, o)| doc.model.active().clip(m::ClipId(*c as u64)).map(|mc| (mc.clone(), secs(mc.start), o.clone()))).collect();
    let refs: Vec<(&m::Clip, f64, String)> = clips.iter().map(|(c, s, o)| (c, *s, o.clone())).collect();
    let tracks = rg::swap_keys(&refs, keys);
    let seq = doc.seq();
    let mut edits = Vec::new();
    for (cid, track) in tracks {
        if let Some((mut c, _, _)) = clips.iter().find(|(c, _, _)| c.id.0 == cid).cloned() {
            c.keyframes.insert("transform.opacity".into(), track);
            edits.push(m::Edit::SetClip { seq, clip: Box::new(c) });
        }
    }
    if doc.apply("Swap", m::Edit::Batch { edits }) { Ok(()) } else { Err(doc.error.take().unwrap_or_else(|| "swap was refused".into())) }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A project with the stick figure from `rigging`'s tests as a PNG
    /// layer.
    pub(crate) fn figure_doc() -> (Doc, u32) {
        let (w, h) = (100usize, 140usize);
        let mut px = vec![0u8; w * h * 4];
        let mut fill = |x0: usize, y0: usize, x1: usize, y1: usize, c: [u8; 3]| {
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = (y * w + x) * 4;
                    px[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
                }
            }
        };
        fill(40, 4, 62, 26, [240, 200, 160]);
        fill(46, 26, 56, 32, [240, 200, 160]);
        fill(38, 32, 64, 80, [40, 90, 200]);
        fill(64, 40, 92, 48, [40, 90, 200]);
        fill(30, 40, 38, 74, [40, 90, 200]);
        fill(40, 80, 49, 136, [30, 30, 30]);
        fill(54, 80, 63, 136, [30, 30, 30]);
        // Eyes and a mouth on the head.
        fill(52, 10, 55, 13, [10, 10, 10]);
        fill(54, 19, 60, 21, [150, 20, 20]);
        let dir = std::env::temp_dir().join(format!("studio-tests/rig-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let file = dir.join("figure.png");
        write_png(&file, w, h, &px).unwrap();
        let mut doc = super::super::project::new_custom("Rig", 640, 360, 30.0);
        let report = super::super::project::import_media(&mut doc, &[file]);
        let media = report.ids[0];
        let index = doc.project.media.iter().position(|m| m.id == media).unwrap();
        let out = super::super::library::add(&mut doc, super::super::library::Panel::Media, index, 0, None).unwrap();
        (doc, out.select[0])
    }

    fn px_dist(sk: &m::Skeleton, a: m::Vec2, b: m::Vec2) -> f64 {
        ((a.x - b.x) * sk.aspect).hypot(a.y - b.y)
    }

    #[test]
    fn a_walk_retargets_onto_a_rig_and_keeps_its_swing() {
        let (mut doc, id) = figure_doc();
        let start = doc.project.clip(id).unwrap().start;
        build(&mut doc, id, "person", Some(Some(true)), start).unwrap();
        act(&mut doc, id, &["walk".into()], 0.0, 2.0, 1.0).unwrap();
        let before = model_clip(&doc, id).unwrap();
        let th = before.skeleton.as_ref().unwrap().named("thigh.front").unwrap().id;
        let times: Vec<Time> = (1..12).map(|k| k * SEC / 6).collect();
        let want: Vec<f64> = times.iter().map(|&t| before.skeleton_at(t).unwrap().bone(th).unwrap().rotation).collect();
        assert!(want.iter().any(|r| r.abs() > 3.0), "the walk swings the thigh: {want:?}");
        // Onto itself it comes back the same (rests and proportions match).
        let done = retarget(&mut doc, id, id, 0.0, Some(2.0), &[]).unwrap();
        assert!(done.bones.iter().any(|b| b == "thigh.front") && done.keys > 20, "{:?}", done.bones);
        let after = model_clip(&doc, id).unwrap();
        for (t, w) in times.iter().zip(&want) {
            let got = after.skeleton_at(*t).unwrap().bone(th).unwrap().rotation;
            assert!((got - w).abs() < 4.0, "{got} vs {w} at {t}");
        }
        assert!(retarget(&mut doc, id, id, 0.0, None, &[("nope".into(), "head".into())]).is_err());
    }

    #[test]
    fn an_unreachable_target_gets_a_straight_arm_pointing_at_it() {
        let (mut doc, id) = figure_doc();
        let start = doc.project.clip(id).unwrap().start;
        build(&mut doc, id, "person", Some(Some(true)), start).unwrap();
        let sk = model_clip(&doc, id).unwrap().skeleton.clone().unwrap();
        let fore = sk.named("forearm.front").unwrap();
        let up = sk.named("upper_arm.front").unwrap();
        let reach_len = px_dist(&sk, up.head, up.tail) + px_dist(&sk, up.tail, fore.tail);
        // The figure's arm is the box x 64–92 of 100, y 40–48 of 140: the fit
        // must measure most of it from the shoulder at the body's edge.
        assert!(reach_len > 0.17, "the arm is measured at {reach_len}");
        let target = m::Vec2::new(up.head.x + 0.8, up.head.y - 0.1);
        assert!(px_dist(&sk, up.head, target) > reach_len * 1.5);
        let mut s = sk.clone();
        s.ik = vec![m::IkHandle { bone: fore.id, target, ..Default::default() }];
        let p = s.posed();
        let (sh, tip) = (p.bone(up.id).unwrap().head, p.bone(fore.id).unwrap().tail);
        // Straight: the tip is a full reach away, along the line to the target.
        assert!((px_dist(&s, sh, tip) - reach_len).abs() < 1e-3);
        let (a, b) = (((tip.y - sh.y).atan2((tip.x - sh.x) * s.aspect)), ((target.y - sh.y).atan2((target.x - sh.x) * s.aspect)));
        assert!((a - b).abs() < 1e-3, "{a} vs {b}");
        // With stretch on, the arm grows toward the target.
        s.ik[0].stretch = 3.0;
        let tip2 = s.posed().bone(fore.id).unwrap().tail;
        assert!(px_dist(&s, sh, tip2) > reach_len * 1.4, "{}", px_dist(&s, sh, tip2));
    }

    #[test]
    fn build_act_pose_reach_split_and_swap() {
        let (mut doc, id) = figure_doc();
        let start = doc.project.clip(id).unwrap().start;
        let said = build(&mut doc, id, "person", Some(Some(true)), start).unwrap();
        assert!(said.contains("bones"), "{said}");
        let mc = model_clip(&doc, id).unwrap();
        let sk = mc.skeleton.clone().unwrap();
        assert!(sk.aspect > 0.6 && sk.aspect < 0.8, "aspect {}", sk.aspect);
        // A walk writes keys on the legs and says how fast to travel.
        let acted = act(&mut doc, id, &["walk".into()], 0.0, 2.0, 1.0).unwrap();
        assert!(acted.bones.iter().any(|b| b == "thigh.front"));
        assert!(acted.travel.unwrap() > 10.0, "{:?}", acted.travel);
        let mc = model_clip(&doc, id).unwrap();
        let th = sk.named("thigh.front").unwrap().id;
        assert!(mc.keyframes.contains_key(&format!("bone.{th}.rotation")));
        // A reach puts the front hand up; posing writes it.
        let fore = sk.named("forearm.front").unwrap().id;
        // A target the arm can reach with its joint limits: where the hand
        // lands when the arm is bent a little (at that moment of the walk).
        let up = sk.named("upper_arm.front").unwrap().id;
        let mut bent = model_clip(&doc, id).unwrap().skeleton_at(SEC / 2).unwrap();
        for (b, d) in [(up, -35.0), (fore, 20.0)] {
            let bone = bent.bone_mut(b).unwrap();
            bone.rotation = bone.limit(bone.rotation + d);
        }
        let target = bent.forward().bone(fore).unwrap().tail;
        let rot = reach(&doc, id, fore, 2, target, false, start + SEC / 2).unwrap();
        assert!(!rot.is_empty());
        pose(&mut doc, id, &rot, &[], start + SEC / 2, true).unwrap();
        let mc = model_clip(&doc, id).unwrap();
        let posed = mc.skeleton_at(SEC / 2).unwrap().forward();
        let tip = posed.bone(fore).unwrap().tail;
        assert!((tip.x - target.x).hypot(tip.y - target.y) < 0.01, "{tip:?} vs {target:?}");
        // Parts: each on its own lane, following its bone; the rig hides.
        let parts = split_layers(&mut doc, id, start, true).unwrap();
        let names: Vec<&str> = parts.iter().map(|p| p.1.as_str()).collect();
        assert!(names.contains(&"head") && names.contains(&"torso") && names.contains(&"mouth"), "{names:?}");
        assert!(split_layers(&mut doc, id, start, true).is_err(), "only once");
        let rig = model_clip(&doc, id).unwrap();
        assert_eq!(rig.transform.opacity, 0.0);
        let head = parts.iter().find(|p| p.1 == "head").unwrap().0;
        let hc = model_clip(&doc, head).unwrap();
        let link = hc.parent.as_ref().unwrap();
        assert_eq!(link.clip.0, id as u64);
        assert!(link.bone.is_some());
        // The head layer turns when the spine turns.
        let spine = sk.named("spine").unwrap().id;
        let before = doc.model.active().placed_transform(&hc, start);
        pose(&mut doc, id, &[(spine, 30.0)], &[], start, false).unwrap();
        let after = doc.model.active().placed_transform(&model_clip(&doc, head).unwrap(), start);
        assert!((after.rotation - before.rotation - 30.0).abs() < 1.0, "{} → {}", before.rotation, after.rotation);
        assert!((after.position.x - before.position.x).abs() > 1.0, "the head swings with the spine");
        // Swap sets: the mouth came off as "original"; add one and switch.
        assert_eq!(swap_sets(&doc, id).iter().find(|s| s.0 == "mouth").map(|s| s.1.clone()), Some(vec!["closed".to_owned()]));
        let (w, h) = (100usize, 140usize);
        let mut o = vec![0u8; w * h * 4];
        for y in 18..23 {
            for x in 54..60 {
                o[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[90, 0, 0, 255]);
            }
        }
        let file = parts_dir(&doc).join(format!("mouth-o-{}.png", std::process::id()));
        write_png(&file, w, h, &o).unwrap();
        let headbone = sk.named("head").unwrap().id;
        let oc = add_swap(&mut doc, id, "mouth", "O", &file, headbone, None, None, start).unwrap();
        assert_eq!(model_clip(&doc, oc).unwrap().transform.opacity, 0.0);
        swap(&mut doc, id, "mouth", &[(0.5, "O".into()), (1.0, "closed".into())]).unwrap();
        let oc = model_clip(&doc, oc).unwrap();
        assert_eq!(oc.property_at("transform.opacity", 3 * SEC / 4), Some(m::Value::Scalar(1.0)));
        assert_eq!(oc.property_at("transform.opacity", 5 * SEC / 4), Some(m::Value::Scalar(0.0)));
        assert!(swap(&mut doc, id, "mouth", &[(0.5, "E".into())]).is_err());
        assert!(swap(&mut doc, id, "hands", &[(0.5, "O".into())]).is_err());
    }
}
