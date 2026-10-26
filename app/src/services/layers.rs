// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Parenting and track mattes, shared by the Inspector and the AI editor.
//!
//! * **Parent**: a clip follows another clip's position, scale and
//!   rotation from the moment they are linked (nothing jumps when linking).
//! * **Track matte**: a clip shows only through the clip on the lane above
//!   it, by that clip's shape (alpha) or brightness (luma), or the reverse.
//!   As in pre-composing, the two become one merged clip
//!   with the matte on top in a stencil blend, so the matte cuts only its
//!   own layer and the pair still moves and animates as one.

use crate::doc::Doc;
use crate::view::Time;
use studio_model as m;

/// Links `child` to `parent` (or unlinks with `None`), with the parent's
/// pose at sequence time `at` as the rest it is measured from.
pub fn set_parent(doc: &mut Doc, child: u64, parent: Option<u64>, at: Time) -> Result<String, String> {
    let seq = doc.seq();
    let sq = doc.model.sequence(seq).ok_or("no sequence")?;
    let mut c = sq.clip(m::ClipId(child)).cloned().ok_or_else(|| format!("no clip {child}"))?;
    match parent {
        None => {
            if c.parent.is_none() {
                return Ok(format!("clip {child} follows nothing"));
            }
            // Where it is drawn now becomes where it is, so it stays put.
            let placed = sq.placed_transform(&c, at.clamp(c.start, c.end()));
            if c.keyframes.keys().all(|k| !k.starts_with("transform.")) {
                c.transform.position = placed.position;
                c.transform.scale_x = placed.scale_x;
                c.transform.scale_y = placed.scale_y;
                c.transform.rotation = placed.rotation;
            }
            c.parent = None;
            if doc.apply("Unparent", m::Edit::SetClip { seq, clip: Box::new(c) }) {
                Ok(format!("clip {child} no longer follows a parent"))
            } else {
                Err(doc.error.take().unwrap_or_else(|| "could not unlink".into()))
            }
        }
        Some(p) => {
            if p == child {
                return Err("a clip cannot follow itself".into());
            }
            let pc = sq.clip(m::ClipId(p)).ok_or_else(|| format!("no clip {p} on this timeline (parents sit on the same timeline, or inside the same merged clip)"))?;
            // No loops: the parent must not already follow the child.
            let mut up = pc.parent.as_ref().map(|q| q.clip);
            let mut hops = 0;
            while let Some(id) = up {
                if id.0 == child || hops > 16 {
                    return Err(format!("clip {p} already follows clip {child}; parents cannot go round in a loop"));
                }
                up = sq.clip(id).and_then(|q| q.parent.as_ref().map(|r| r.clip));
                hops += 1;
            }
            // Rest: the parent as drawn at that moment (its own parents
            // included), so linking never makes the child jump.
            let rest = sq.placed_transform(pc, at.clamp(pc.start, pc.end()));
            let name = pc.name.clone();
            c.parent = Some(m::Parent { clip: m::ClipId(p), rest_position: rest.position, rest_scale: m::Vec2 { x: rest.scale_x, y: rest.scale_y }, rest_rotation: rest.rotation, rest_z: rest.position_z, rest_rotation_x: rest.rotation_x, rest_rotation_y: rest.rotation_y, bone: None });
            if doc.apply("Parent", m::Edit::SetClip { seq, clip: Box::new(c) }) {
                Ok(format!("clip {child} follows clip {p} ({name}): it moves, scales and turns with it"))
            } else {
                Err(doc.error.take().unwrap_or_else(|| "could not link".into()))
            }
        }
    }
}

/// The four ways a matte can cut.
pub const MATTES: [(&str, &str, m::BlendMode); 4] = [
    ("alpha", "Alpha matte", m::BlendMode::StencilAlpha),
    ("alpha_inverted", "Alpha inverted", m::BlendMode::SilhouetteAlpha),
    ("luma", "Luma matte", m::BlendMode::StencilLuma),
    ("luma_inverted", "Luma inverted", m::BlendMode::SilhouetteLuma),
];

/// The visual clip on the nearest lane above `id` that overlaps it in
/// time: the usual track matte.
pub fn layer_above(doc: &Doc, id: u64) -> Option<u64> {
    let sq = doc.model.sequence(doc.seq())?;
    let c = sq.clip(m::ClipId(id))?;
    let mine = sq.lane_index(c.lane)?;
    sq.clips
        .iter()
        .filter(|o| o.id != c.id && o.start < c.end() && o.end() > c.start)
        .filter(|o| sq.lanes.iter().find(|l| l.id == o.lane).is_some_and(|l| l.kind != m::LaneKind::Audio))
        .filter_map(|o| sq.lane_index(o.lane).map(|i| (i, o.id.0)))
        // Lanes are listed top first: above means a smaller index.
        .filter(|(i, _)| *i < mine)
        .max_by_key(|(i, _)| *i)
        .map(|(_, id)| id)
}

/// Makes `content` show through `matte` (`mode` one of [`MATTES`]): the
/// pair becomes one merged clip, returned.
pub fn track_matte(doc: &mut Doc, content: u64, matte: u64, mode: &str, alloc: &mut dyn FnMut() -> u64) -> Result<u64, String> {
    let blend = MATTES.iter().find(|(id, label, _)| *id == mode || label.eq_ignore_ascii_case(mode)).map(|m| m.2).ok_or_else(|| format!("matte mode \"{mode}\": alpha, alpha_inverted, luma or luma_inverted"))?;
    let seq = doc.seq();
    let sq = doc.model.sequence(seq).ok_or("no sequence")?;
    let (c, mt) = (sq.clip(m::ClipId(content)).ok_or_else(|| format!("no clip {content}"))?, sq.clip(m::ClipId(matte)).ok_or_else(|| format!("no clip {matte}"))?);
    let (ci, mi) = (sq.lane_index(c.lane).unwrap_or(0), sq.lane_index(mt.lane).unwrap_or(0));
    if mi >= ci {
        return Err(format!("the matte (clip {matte}) must be on a lane above clip {content}; move it up first"));
    }
    if mt.start >= c.end() || mt.end() <= c.start {
        return Err("the matte and the clip it cuts must overlap in time".into());
    }
    let name = format!("{} (matted)", if c.name.is_empty() { "Clip".into() } else { c.name.clone() });
    let (new_sequence, new_clip) = (m::SequenceId(alloc()), m::ClipId(alloc()));
    if !doc.apply("Track matte", m::Edit::CreateCompound { seq, clips: vec![m::ClipId(content), m::ClipId(matte)], name, new_sequence, new_clip }) {
        return Err(doc.error.take().unwrap_or_else(|| "those clips cannot be put together".into()));
    }
    let mut inner = doc.model.sequence(new_sequence).and_then(|s| s.clip(m::ClipId(matte))).cloned().ok_or("the matte went missing")?;
    inner.blend = blend;
    if !doc.apply("Track matte", m::Edit::SetClip { seq: new_sequence, clip: Box::new(inner) }) {
        return Err(doc.error.take().unwrap_or_else(|| "the matte could not be set".into()));
    }
    Ok(new_clip.0)
}

/// The matte a merged clip holds (its mode's id and the matte clip), if
/// it was made with [`track_matte`].
pub fn matte_of(doc: &Doc, merged: u64) -> Option<(&'static str, u64)> {
    let c = doc.model.sequences.iter().find_map(|s| s.clip(m::ClipId(merged)))?;
    let m::ClipSource::Sequence(r) = &c.source else { return None };
    let inner = doc.model.sequence(r.sequence)?;
    inner.clips.iter().find_map(|ic| MATTES.iter().find(|(_, _, b)| *b == ic.blend).map(|(id, _, _)| (*id, ic.id.0)))
}
