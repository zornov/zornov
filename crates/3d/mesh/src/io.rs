// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `.smesh`: how a project keeps a model. Little-endian, versioned:
//!
//! ```text
//! "SMESH2\0\0" ("SMESH1" files, without subsurface, still read;
//! "SMESH3" files are SMESH2 plus extras after the materials, written
//! only for rigged models and glass or water)
//! u32 vertices, u32 indices, u32 groups, u32 materials
//! vertices × (f32 px, py, pz, nx, ny, nz, u, v)
//! indices  × u32
//! groups   × (u32 start, count, material)
//! materials × (u32 name length, name UTF-8, f32 × 4 base colour,
//!              f32 metallic, roughness, f32 × 3 emissive, f32 subsurface,
//!              u32 texture PNG length (0 = none), PNG bytes)
//! [SMESH3] u32 flags; bit 1: per material f32 transmission, f32 ior;
//!          bit 2: the rig skins with dual quaternions;
//!          bit 0: the rig: bones, skin, shape keys, constraints, actions
//!          (see `write_rig`)
//! ```

use crate::{Group, Material, Mesh, Texture};
use std::path::Path;

const MAGIC: &[u8; 8] = b"SMESH2\0\0";
const MAGIC_3: &[u8; 8] = b"SMESH3\0\0";
const MAGIC_1: &[u8; 8] = b"SMESH1\0\0";

/// The file bytes of a model. A rig whose skin is out of step with the
/// surface is put right first, so no file ever holds a damaged rig.
pub fn to_bytes(m: &Mesh) -> Vec<u8> {
    if m.rig_in_step() {
        return to_bytes_unrepaired(m);
    }
    let mut fixed = m.clone();
    fixed.repair_rig();
    to_bytes_unrepaired(&fixed)
}

/// The file bytes exactly as the model stands (tests of old, damaged files).
pub fn to_bytes_unrepaired(m: &Mesh) -> Vec<u8> {
    let mut b = Vec::with_capacity(16 + m.positions.len() * 32 + m.indices.len() * 4);
    let extras = m.materials.iter().any(|x| x.transmission != 0.0 || x.ior != Material::default().ior);
    b.extend_from_slice(if m.rig.is_some() || extras { MAGIC_3 } else { MAGIC });
    for n in [m.positions.len(), m.indices.len(), m.groups.len(), m.materials.len()] {
        b.extend_from_slice(&(n as u32).to_le_bytes());
    }
    let f = |b: &mut Vec<u8>, v: f32| b.extend_from_slice(&v.to_le_bytes());
    for i in 0..m.positions.len() {
        let (p, n, uv) = (m.positions[i], m.normals.get(i).copied().unwrap_or([0.0; 3]), m.uvs.get(i).copied().unwrap_or([0.0; 2]));
        for v in [p[0], p[1], p[2], n[0], n[1], n[2], uv[0], uv[1]] {
            f(&mut b, v);
        }
    }
    for i in &m.indices {
        b.extend_from_slice(&i.to_le_bytes());
    }
    for g in &m.groups {
        for v in [g.start, g.count, g.material] {
            b.extend_from_slice(&v.to_le_bytes());
        }
    }
    for mat in &m.materials {
        b.extend_from_slice(&(mat.name.len() as u32).to_le_bytes());
        b.extend_from_slice(mat.name.as_bytes());
        for v in mat.base_colour {
            f(&mut b, v);
        }
        f(&mut b, mat.metallic);
        f(&mut b, mat.roughness);
        for v in mat.emissive {
            f(&mut b, v);
        }
        f(&mut b, mat.subsurface);
        let png = mat.texture.as_ref().map(|t| studio_media::png::encode(&studio_media::RgbaImage { width: t.width, height: t.height, pixels: t.rgba.clone() })).unwrap_or_default();
        b.extend_from_slice(&(png.len() as u32).to_le_bytes());
        b.extend_from_slice(&png);
    }
    if m.rig.is_some() || extras {
        put_u32(&mut b, m.rig.is_some() as u32 | (extras as u32) << 1 | (m.rig.as_ref().is_some_and(|r| r.dual_quat) as u32) << 2);
        if extras {
            for mat in &m.materials {
                put_f32(&mut b, mat.transmission);
                put_f32(&mut b, mat.ior);
            }
        }
        if let Some(rig) = &m.rig {
            write_rig(&mut b, rig);
        }
    }
    b
}

fn put_u32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_le_bytes());
}
fn put_f32(b: &mut Vec<u8>, v: f32) {
    b.extend_from_slice(&v.to_le_bytes());
}
fn put_str(b: &mut Vec<u8>, s: &str) {
    put_u32(b, s.len() as u32);
    b.extend_from_slice(s.as_bytes());
}
fn put_v3(b: &mut Vec<u8>, v: [f32; 3]) {
    for x in v {
        put_f32(b, x);
    }
}

/// ```text
/// u32 bones × (name, i32 parent (−1 none), head, tail, f32 roll, u32 deform)
/// u32 skinned vertices × (u16 × 4 joints, f32 × 4 weights)
/// u32 shape keys × (name, u32 n, n × (u32 vertex, f32 × 3 offset))
/// u32 constraints × (u32 kind, name, u32 bone, then by kind:
///     0 IK: u32 chain, target, u32 has pole, f32 × 3 pole, f32 influence
///     1 look-at: target, f32 influence
///     2 limit: f32 × 3 min, f32 × 3 max
///     3 copy rotation: u32 from, f32 influence)
///   target = u32 0 + f32 × 3 point, or u32 1 + u32 bone
/// u32 actions × (name, f32 duration, u32 tracks × (u32 kind (0 rotation,
///     1 location, 2 scale, 3 shape), u32 index, u32 step, u32 keys ×
///     (f32 time, f32 × 4 value)))
/// ```
/// Strings are u32 length + UTF-8.
fn write_rig(b: &mut Vec<u8>, r: &crate::rig::Rig) {
    use crate::rig::{Channel, Constraint, Target};
    put_u32(b, r.bones.len() as u32);
    for bone in &r.bones {
        put_str(b, &bone.name);
        b.extend_from_slice(&(bone.parent.map_or(-1i32, |p| p as i32)).to_le_bytes());
        put_v3(b, bone.head);
        put_v3(b, bone.tail);
        put_f32(b, bone.roll);
        put_u32(b, bone.deform as u32);
    }
    put_u32(b, r.joints.len() as u32);
    for (j, w) in r.joints.iter().zip(&r.weights) {
        for x in j {
            b.extend_from_slice(&x.to_le_bytes());
        }
        for x in w {
            put_f32(b, *x);
        }
    }
    put_u32(b, r.shapes.len() as u32);
    for k in &r.shapes {
        put_str(b, &k.name);
        put_u32(b, k.indices.len() as u32);
        for (i, d) in k.indices.iter().zip(&k.deltas) {
            put_u32(b, *i);
            put_v3(b, *d);
        }
    }
    let target = |b: &mut Vec<u8>, t: &Target| match t {
        Target::Point(p) => {
            put_u32(b, 0);
            put_v3(b, *p);
        }
        Target::Bone(i) => {
            put_u32(b, 1);
            put_u32(b, *i as u32);
        }
    };
    put_u32(b, r.constraints.len() as u32);
    for c in &r.constraints {
        match c {
            Constraint::Ik { name, bone, chain, target: t, pole, influence } => {
                put_u32(b, 0);
                put_str(b, name);
                put_u32(b, *bone as u32);
                put_u32(b, *chain as u32);
                target(b, t);
                put_u32(b, pole.is_some() as u32);
                put_v3(b, pole.unwrap_or([0.0; 3]));
                put_f32(b, *influence);
            }
            Constraint::LookAt { name, bone, target: t, influence } => {
                put_u32(b, 1);
                put_str(b, name);
                put_u32(b, *bone as u32);
                target(b, t);
                put_f32(b, *influence);
            }
            Constraint::Limit { name, bone, min, max } => {
                put_u32(b, 2);
                put_str(b, name);
                put_u32(b, *bone as u32);
                put_v3(b, *min);
                put_v3(b, *max);
            }
            Constraint::CopyRotation { name, bone, from, influence } => {
                put_u32(b, 3);
                put_str(b, name);
                put_u32(b, *bone as u32);
                put_u32(b, *from as u32);
                put_f32(b, *influence);
            }
        }
    }
    put_u32(b, r.actions.len() as u32);
    for a in &r.actions {
        put_str(b, &a.name);
        put_f32(b, a.duration);
        put_u32(b, a.tracks.len() as u32);
        for t in &a.tracks {
            let (kind, index) = match t.channel {
                Channel::Rotation(i) => (0, i),
                Channel::Location(i) => (1, i),
                Channel::Scale(i) => (2, i),
                Channel::Shape(i) => (3, i),
            };
            put_u32(b, kind);
            put_u32(b, index as u32);
            put_u32(b, t.step as u32);
            put_u32(b, t.times.len() as u32);
            for (time, v) in t.times.iter().zip(&t.values) {
                put_f32(b, *time);
                for x in v {
                    put_f32(b, *x);
                }
            }
        }
    }
}

fn read_rig(r: &mut Reader, vertices: usize) -> Result<crate::rig::Rig, String> {
    use crate::rig::{Action, Bone, Channel, Constraint, Rig, ShapeKey, Target, Track};
    let bad = || "the model's rig is damaged".to_string();
    let mut rig = Rig::default();
    let s = |r: &mut Reader| -> Result<String, String> {
        let n = r.u32()? as usize;
        Ok(String::from_utf8_lossy(r.take(n)?).into_owned())
    };
    let v3 = |r: &mut Reader| -> Result<[f32; 3], String> { Ok([r.f32()?, r.f32()?, r.f32()?]) };
    let left = |r: &Reader| r.b.len().saturating_sub(r.at);
    let nb = r.u32()? as usize;
    if nb > 4096 || nb * 33 > left(r) {
        return Err(bad());
    }
    for _ in 0..nb {
        let name = s(r)?;
        let parent = i32::from_le_bytes(r.take(4)?.try_into().map_err(|_| bad())?);
        let head = v3(r)?;
        let tail = v3(r)?;
        let roll = r.f32()?;
        let deform = r.u32()? != 0;
        let parent = (parent >= 0 && (parent as usize) < rig.bones.len()).then_some(parent as usize);
        rig.bones.push(Bone { name, parent, head, tail, roll, deform });
    }
    let nj = r.u32()? as usize;
    // A skin out of step with the surface (an old file, written after an
    // edit that added vertices) is read as it is and put right by `load`.
    if nj > vertices.max(1) * 4 || nj * 24 > left(r) {
        return Err(bad());
    }
    for _ in 0..nj {
        let t = r.take(8)?;
        let j: [u16; 4] = std::array::from_fn(|k| u16::from_le_bytes([t[2 * k], t[2 * k + 1]]).min(nb.saturating_sub(1) as u16));
        let w = [r.f32()?, r.f32()?, r.f32()?, r.f32()?];
        rig.joints.push(j);
        rig.weights.push(w);
    }
    let ns = r.u32()? as usize;
    if ns > 4096 {
        return Err(bad());
    }
    for _ in 0..ns {
        let name = s(r)?;
        let n = r.u32()? as usize;
        if n > vertices || n * 16 > left(r) {
            return Err(bad());
        }
        let mut k = ShapeKey { name, indices: Vec::with_capacity(n), deltas: Vec::with_capacity(n) };
        for _ in 0..n {
            let i = r.u32()?;
            let d = v3(r)?;
            if (i as usize) < vertices {
                k.indices.push(i);
                k.deltas.push(d);
            }
        }
        rig.shapes.push(k);
    }
    let bone = |r: &mut Reader| -> Result<usize, String> {
        let i = r.u32()? as usize;
        if i < nb { Ok(i) } else { Err(bad()) }
    };
    let target = |r: &mut Reader| -> Result<Target, String> {
        Ok(match r.u32()? {
            0 => Target::Point(v3(r)?),
            _ => Target::Bone(bone(r)?),
        })
    };
    let nc = r.u32()? as usize;
    if nc > 4096 {
        return Err(bad());
    }
    for _ in 0..nc {
        let kind = r.u32()?;
        let name = s(r)?;
        let b = bone(r)?;
        rig.constraints.push(match kind {
            0 => {
                let chain = r.u32()? as usize;
                let t = target(r)?;
                let has = r.u32()? != 0;
                let p = v3(r)?;
                Constraint::Ik { name, bone: b, chain, target: t, pole: has.then_some(p), influence: r.f32()? }
            }
            1 => {
                let t = target(r)?;
                Constraint::LookAt { name, bone: b, target: t, influence: r.f32()? }
            }
            2 => Constraint::Limit { name, bone: b, min: v3(r)?, max: v3(r)? },
            3 => {
                let from = bone(r)?;
                Constraint::CopyRotation { name, bone: b, from, influence: r.f32()? }
            }
            _ => return Err(bad()),
        });
    }
    let na = r.u32()? as usize;
    if na > 4096 {
        return Err(bad());
    }
    for _ in 0..na {
        let name = s(r)?;
        let duration = r.f32()?;
        let nt = r.u32()? as usize;
        if nt > 100_000 {
            return Err(bad());
        }
        let mut a = Action { name, duration, tracks: Vec::with_capacity(nt) };
        for _ in 0..nt {
            let kind = r.u32()?;
            let index = r.u32()? as usize;
            let step = r.u32()? != 0;
            let nk = r.u32()? as usize;
            if nk * 20 > left(r) {
                return Err(bad());
            }
            let channel = match kind {
                0 => Channel::Rotation(index),
                1 => Channel::Location(index),
                2 => Channel::Scale(index),
                _ => Channel::Shape(index),
            };
            let mut t = Track { channel, times: Vec::with_capacity(nk), values: Vec::with_capacity(nk), step };
            for _ in 0..nk {
                t.times.push(r.f32()?);
                t.values.push([r.f32()?, r.f32()?, r.f32()?, r.f32()?]);
            }
            a.tracks.push(t);
        }
        rig.actions.push(a);
    }
    Ok(rig)
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let s = self.b.get(self.at..self.at + n).ok_or("the model file is cut short")?;
        self.at += n;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("4 bytes")))
    }
    fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().expect("4 bytes")))
    }
}

pub fn from_bytes(b: &[u8]) -> Result<Mesh, String> {
    let version = match b.get(..8) {
        Some(m) if m == MAGIC_3 => 3,
        Some(m) if m == MAGIC => 2,
        Some(m) if m == MAGIC_1 => 1,
        _ => return Err("not a Zornov model (.smesh) file".into()),
    };
    let mut r = Reader { b, at: 8 };
    let (nv, ni, ng, nm) = (r.u32()? as usize, r.u32()? as usize, r.u32()? as usize, r.u32()? as usize);
    // Sizes are checked against the file before anything is allocated.
    if nv * 32 + ni * 4 + ng * 12 > b.len() || ni % 3 != 0 || nm > 4096 {
        return Err("the model file is damaged".into());
    }
    let mut m = Mesh::default();
    for _ in 0..nv {
        let v: [f32; 8] = std::array::from_fn(|_| r.f32().unwrap_or(0.0));
        m.positions.push([v[0], v[1], v[2]]);
        m.normals.push([v[3], v[4], v[5]]);
        m.uvs.push([v[6], v[7]]);
    }
    for _ in 0..ni {
        let i = r.u32()?;
        if i as usize >= nv {
            return Err("the model file has a triangle pointing past its vertices".into());
        }
        m.indices.push(i);
    }
    for _ in 0..ng {
        m.groups.push(Group { start: r.u32()?, count: r.u32()?, material: r.u32()? });
    }
    for _ in 0..nm {
        let len = r.u32()? as usize;
        let name = String::from_utf8_lossy(r.take(len)?).into_owned();
        let base_colour = [r.f32()?, r.f32()?, r.f32()?, r.f32()?];
        let (metallic, roughness) = (r.f32()?, r.f32()?);
        let emissive = [r.f32()?, r.f32()?, r.f32()?];
        let subsurface = if version >= 2 { r.f32()? } else { 0.0 };
        let png_len = r.u32()? as usize;
        let texture = if png_len > 0 {
            let img = studio_media::png::decode(r.take(png_len)?).map_err(|e| format!("a model texture is damaged: {e:?}"))?;
            Some(Texture { width: img.width, height: img.height, rgba: img.pixels })
        } else {
            None
        };
        m.materials.push(Material { name, base_colour, metallic, roughness, emissive, subsurface, texture, ..Material::default() });
    }
    if version >= 3 {
        let flags = r.u32()?;
        if flags & 2 != 0 {
            for mat in &mut m.materials {
                mat.transmission = r.f32()?.clamp(0.0, 1.0);
                mat.ior = r.f32()?.clamp(1.0, 4.0);
            }
        }
        if flags & 1 != 0 {
            let mut rig = read_rig(&mut r, nv)?;
            rig.dual_quat = flags & 4 != 0;
            m.rig = Some(std::sync::Arc::new(rig));
            m.repair_rig();
        }
    }
    if m.materials.is_empty() {
        m.materials.push(Material::default());
    }
    for g in &mut m.groups {
        g.material = g.material.min(m.materials.len() as u32 - 1);
    }
    m.tidy();
    Ok(m)
}

pub fn save(m: &Mesh, path: &Path) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("smesh.part");
    std::fs::write(&tmp, to_bytes(m)).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn load(path: &Path) -> Result<Mesh, String> {
    from_bytes(&std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?)
}

/// A short content hash: model files are named by it, so a new version
/// never overwrites one an undo step still points to.
pub fn content_hash(bytes: &[u8]) -> String {
    let h = bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3));
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_with_a_texture() {
        let mut m = crate::prim::cube([100.0, 50.0, 25.0]);
        m.materials[0].texture = Some(Texture { width: 2, height: 1, rgba: vec![255, 0, 0, 255, 0, 0, 255, 255] });
        m.materials[0].roughness = 0.3;
        let back = from_bytes(&to_bytes(&m)).unwrap();
        assert_eq!(back, m);
        assert!(from_bytes(b"SMESH1\0\0\xff\xff\xff\xff").is_err());
        assert!(from_bytes(b"nope").is_err());
    }

    #[test]
    fn a_rigged_model_keeps_its_rig() {
        let m = crate::character::character(300.0, &crate::character::Build { detail: 1, ..Default::default() });
        let mut m = m;
        let rig = std::sync::Arc::make_mut(m.rig.as_mut().unwrap());
        rig.actions.push(crate::motion::make(rig, "wave", &crate::motion::Style::default()).unwrap());
        let bytes = to_bytes(&m);
        assert!(bytes.starts_with(MAGIC_3));
        let back = from_bytes(&bytes).unwrap();
        assert_eq!(back.rig, m.rig);
        // Glass without a rig keeps its transmission.
        let mut glass = crate::prim::cube([10.0; 3]);
        glass.materials[0].transmission = 1.0;
        glass.materials[0].ior = 1.5;
        assert_eq!(from_bytes(&to_bytes(&glass)).unwrap(), glass);
        // Cut short anywhere in the rig: refused, never a panic.
        for cut in [bytes.len() - 3, bytes.len() - 200, bytes.len() / 2 + 7] {
            assert!(from_bytes(&bytes[..cut]).is_err());
        }
    }
}
