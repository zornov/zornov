// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Baked simulations (`.scache`): what a cloth, soft body, hair, shattered
//! object, particle spray or smoke plume did, frame by frame, so it plays
//! and scrubs without simulating again. A model clip plays one over its
//! model (see the renderer's `model_anim`).
//!
//! Kinds:
//! - [`Frames::Vertices`]: every vertex's position per frame (cloth, soft
//!   bodies, hair strands, water surfaces): the model's triangles stay,
//!   its points move.
//! - [`Frames::Pieces`]: a turn and a shift per piece per frame (shattered
//!   objects, piles of rigid bodies): the model is the pieces at rest,
//!   each a run of vertices.
//! - [`Frames::Points`]: particles: a position, a size, a colour and a
//!   heat per point per frame; points come and go between frames.
//! - [`Frames::Volume`]: a grid of density and heat per frame (smoke, fire,
//!   mist), drawn by marching rays through it.
//!
//! ```text
//! "SCACHE1\0", u32 kind, f32 fps, u32 frames, then by kind:
//!   0 vertices: u32 n, frames × n × f32 × 3
//!   1 pieces:   u32 n, n × (u32 first vertex, u32 count),
//!               frames × n × (f32 × 4 turn, f32 × 3 shift)
//!   2 points:   frames × (u32 n, n × (f32 × 3, f32 size, u8 × 4 rgba, f32 heat))
//!   3 volume:   u32 nx, ny, nz, f32 × 3 min, f32 × 3 max,
//!               frames × nx·ny·nz × (u8 density, u8 heat)
//! ```

use crate::V3;
use crate::rig::Quat;

/// One particle at one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub pos: V3,
    pub size: f32,
    /// sRGB, straight alpha.
    pub rgba: [u8; 4],
    /// 0 cold – 1 white-hot (fire, sparks glow by it).
    pub heat: f32,
}

/// A baked run.
#[derive(Clone, Debug, PartialEq)]
pub enum Frames {
    Vertices { count: usize, data: Vec<V3> },
    Pieces { ranges: Vec<(u32, u32)>, data: Vec<(Quat, V3)> },
    Points(Vec<Vec<Point>>),
    Volume { dims: [usize; 3], min: V3, max: V3, data: Vec<Vec<[u8; 2]>> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cache {
    pub fps: f32,
    pub frames: usize,
    pub data: Frames,
}

const MAGIC: &[u8; 8] = b"SCACHE1\0";

impl Cache {
    /// Seconds the run lasts.
    pub fn duration(&self) -> f32 {
        if self.frames > 1 { (self.frames - 1) as f32 / self.fps.max(1e-3) } else { 0.0 }
    }

    /// The frame for `t` seconds in (held at the ends), and how far towards
    /// the next one.
    pub fn frame_at(&self, t: f32) -> (usize, f32) {
        let f = (t * self.fps).clamp(0.0, self.frames.saturating_sub(1) as f32);
        let i = f.floor() as usize;
        (i.min(self.frames.saturating_sub(1)), f - i as f32)
    }

    /// Vertex positions at `t` (vertices kind), blended between frames.
    pub fn vertices_at(&self, t: f32) -> Option<Vec<V3>> {
        let Frames::Vertices { count, data } = &self.data else { return None };
        let (i, u) = self.frame_at(t);
        let a = data.get(i * count..(i + 1) * count)?;
        let b = data.get((i + 1) * count..(i + 2) * count).unwrap_or(a);
        Some(a.iter().zip(b).map(|(p, q)| crate::lerp(*p, *q, u)).collect())
    }

    /// Each piece's turn and shift at `t` (pieces kind).
    pub fn pieces_at(&self, t: f32) -> Option<Vec<(Quat, V3)>> {
        let Frames::Pieces { ranges, data } = &self.data else { return None };
        let n = ranges.len();
        let (i, u) = self.frame_at(t);
        let a = data.get(i * n..(i + 1) * n)?;
        let b = data.get((i + 1) * n..(i + 2) * n).unwrap_or(a);
        Some(a.iter().zip(b).map(|(p, q)| (crate::rig::q_slerp(p.0, q.0, u), crate::lerp(p.1, q.1, u))).collect())
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(MAGIC);
        let kind: u32 = match self.data {
            Frames::Vertices { .. } => 0,
            Frames::Pieces { .. } => 1,
            Frames::Points(_) => 2,
            Frames::Volume { .. } => 3,
        };
        let u = |b: &mut Vec<u8>, v: u32| b.extend_from_slice(&v.to_le_bytes());
        let f = |b: &mut Vec<u8>, v: f32| b.extend_from_slice(&v.to_le_bytes());
        u(&mut b, kind);
        f(&mut b, self.fps);
        u(&mut b, self.frames as u32);
        match &self.data {
            Frames::Vertices { count, data } => {
                u(&mut b, *count as u32);
                for p in data {
                    for x in p {
                        f(&mut b, *x);
                    }
                }
            }
            Frames::Pieces { ranges, data } => {
                u(&mut b, ranges.len() as u32);
                for r in ranges {
                    u(&mut b, r.0);
                    u(&mut b, r.1);
                }
                for (q, p) in data {
                    for x in q {
                        f(&mut b, *x);
                    }
                    for x in p {
                        f(&mut b, *x);
                    }
                }
            }
            Frames::Points(frames) => {
                for fr in frames {
                    u(&mut b, fr.len() as u32);
                    for p in fr {
                        for x in p.pos {
                            f(&mut b, x);
                        }
                        f(&mut b, p.size);
                        b.extend_from_slice(&p.rgba);
                        f(&mut b, p.heat);
                    }
                }
            }
            Frames::Volume { dims, min, max, data } => {
                for d in dims {
                    u(&mut b, *d as u32);
                }
                for x in min.iter().chain(max) {
                    f(&mut b, *x);
                }
                for fr in data {
                    for c in fr {
                        b.extend_from_slice(c);
                    }
                }
            }
        }
        b
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Cache, String> {
        let bad = || "the simulation cache is damaged".to_string();
        if bytes.get(..8) != Some(MAGIC) {
            return Err("not a Studio simulation cache (.scache)".into());
        }
        let mut at = 8usize;
        let mut take = |n: usize| -> Result<&[u8], String> {
            let s = bytes.get(at..at + n).ok_or_else(bad)?;
            at += n;
            Ok(s)
        };
        macro_rules! u32_ {
            () => {
                u32::from_le_bytes(take(4)?.try_into().map_err(|_| bad())?) as usize
            };
        }
        macro_rules! f32_ {
            () => {
                f32::from_le_bytes(take(4)?.try_into().map_err(|_| bad())?)
            };
        }
        let kind = u32_!();
        let fps = f32_!();
        let frames = u32_!();
        if frames > 1_000_000 || !(fps > 0.0) {
            return Err(bad());
        }
        let left = bytes.len();
        let data = match kind {
            0 => {
                let count = u32_!();
                if count.saturating_mul(frames).saturating_mul(12) > left {
                    return Err(bad());
                }
                let mut data = Vec::with_capacity(count * frames);
                for _ in 0..count * frames {
                    data.push([f32_!(), f32_!(), f32_!()]);
                }
                Frames::Vertices { count, data }
            }
            1 => {
                let n = u32_!();
                if n.saturating_mul(frames).saturating_mul(28) > left {
                    return Err(bad());
                }
                let mut ranges = Vec::with_capacity(n);
                for _ in 0..n {
                    ranges.push((u32_!() as u32, u32_!() as u32));
                }
                let mut data = Vec::with_capacity(n * frames);
                for _ in 0..n * frames {
                    let q = [f32_!(), f32_!(), f32_!(), f32_!()];
                    let p = [f32_!(), f32_!(), f32_!()];
                    data.push((q, p));
                }
                Frames::Pieces { ranges, data }
            }
            2 => {
                let mut all = Vec::with_capacity(frames);
                for _ in 0..frames {
                    let n = u32_!();
                    if n.saturating_mul(24) > left {
                        return Err(bad());
                    }
                    let mut fr = Vec::with_capacity(n);
                    for _ in 0..n {
                        let pos = [f32_!(), f32_!(), f32_!()];
                        let size = f32_!();
                        let c = take(4)?;
                        let rgba = [c[0], c[1], c[2], c[3]];
                        fr.push(Point { pos, size, rgba, heat: f32_!() });
                    }
                    all.push(fr);
                }
                Frames::Points(all)
            }
            3 => {
                let dims = [u32_!(), u32_!(), u32_!()];
                let cells = dims[0].saturating_mul(dims[1]).saturating_mul(dims[2]);
                if cells == 0 || cells > 64_000_000 || cells.saturating_mul(frames).saturating_mul(2) > left {
                    return Err(bad());
                }
                let min = [f32_!(), f32_!(), f32_!()];
                let max = [f32_!(), f32_!(), f32_!()];
                let mut data = Vec::with_capacity(frames);
                for _ in 0..frames {
                    let raw = take(cells * 2)?;
                    data.push(raw.chunks_exact(2).map(|c| [c[0], c[1]]).collect());
                }
                Frames::Volume { dims, min, max, data }
            }
            _ => return Err(bad()),
        };
        Ok(Cache { fps, frames, data })
    }

    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        let tmp = path.with_extension("scache.part");
        std::fs::write(&tmp, self.to_bytes()).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn load(path: &std::path::Path) -> Result<Cache, String> {
        Cache::from_bytes(&std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caches_round_trip_and_blend_between_frames() {
        let c = Cache { fps: 10.0, frames: 2, data: Frames::Vertices { count: 1, data: vec![[0.0; 3], [10.0, 0.0, 0.0]] } };
        let back = Cache::from_bytes(&c.to_bytes()).unwrap();
        assert_eq!(back, c);
        assert_eq!(back.vertices_at(0.05).unwrap()[0], [5.0, 0.0, 0.0]);
        let p = Cache { fps: 24.0, frames: 1, data: Frames::Points(vec![vec![Point { pos: [1.0, 2.0, 3.0], size: 4.0, rgba: [1, 2, 3, 4], heat: 0.5 }]]) };
        assert_eq!(Cache::from_bytes(&p.to_bytes()).unwrap(), p);
        let v = Cache { fps: 24.0, frames: 1, data: Frames::Volume { dims: [2, 1, 1], min: [0.0; 3], max: [1.0; 3], data: vec![vec![[9, 1], [3, 4]]] } };
        assert_eq!(Cache::from_bytes(&v.to_bytes()).unwrap(), v);
        let pc = Cache { fps: 24.0, frames: 1, data: Frames::Pieces { ranges: vec![(0, 3)], data: vec![(crate::rig::Q_ID, [1.0, 2.0, 3.0])] } };
        assert_eq!(Cache::from_bytes(&pc.to_bytes()).unwrap(), pc);
        let bytes = c.to_bytes();
        assert!(Cache::from_bytes(&bytes[..bytes.len() - 2]).is_err());
    }
}
