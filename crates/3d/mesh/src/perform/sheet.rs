// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Contact sheets for looking at a performance: tiles of a character
//! (skin shaded flat, bones drawn over it) as seen from the side by a
//! camera that follows it, with ground marks fixed in the world and a dot
//! under every planted foot, so a foot that slides drifts off its mark.
//! Test-only; writes PNG files with no outside tools.

use super::body::Body;
use super::query::ClipXf;
use crate::rig::{Posed, Rig};
use crate::{Mesh, V3, dot, normalise, sub};
use std::io::Write;

pub struct Tile {
    pub xf: ClipXf,
    pub poses: Vec<crate::rig::BonePose>,
    /// World ankle positions of planted feet.
    pub planted: Vec<V3>,
    pub label: String,
}

struct Canvas {
    w: usize,
    h: usize,
    rgb: Vec<u8>,
}

impl Canvas {
    fn new(w: usize, h: usize) -> Canvas {
        Canvas { w, h, rgb: vec![245; w * h * 3] }
    }
    fn put(&mut self, x: i32, y: i32, c: [u8; 3]) {
        if x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h {
            let i = (y as usize * self.w + x as usize) * 3;
            self.rgb[i..i + 3].copy_from_slice(&c);
        }
    }
    fn line(&mut self, a: (f32, f32), b: (f32, f32), c: [u8; 3]) {
        let n = ((b.0 - a.0).abs().max((b.1 - a.1).abs()) as usize).max(1);
        for i in 0..=n {
            let u = i as f32 / n as f32;
            self.put((a.0 + (b.0 - a.0) * u) as i32, (a.1 + (b.1 - a.1) * u) as i32, c);
        }
    }
    fn dot(&mut self, p: (f32, f32), r: i32, c: [u8; 3]) {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy <= r * r {
                    self.put(p.0 as i32 + dx, p.1 as i32 + dy, c);
                }
            }
        }
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

pub fn write_png(path: &std::path::Path, w: usize, h: usize, rgb: &[u8]) {
    let mut raw = Vec::with_capacity(h * (w * 3 + 1));
    for y in 0..h {
        raw.push(0);
        raw.extend_from_slice(&rgb[y * w * 3..(y + 1) * w * 3]);
    }
    // zlib with stored blocks.
    let mut z = vec![0x78, 0x01];
    let mut i = 0;
    while i < raw.len() {
        let n = (raw.len() - i).min(65535);
        z.push(u8::from(i + n == raw.len()));
        z.extend_from_slice(&(n as u16).to_le_bytes());
        z.extend_from_slice(&(!(n as u16)).to_le_bytes());
        z.extend_from_slice(&raw[i..i + n]);
        i += n;
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut chunk = |kind: &[u8; 4], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    };
    let mut head = Vec::new();
    head.extend_from_slice(&(w as u32).to_be_bytes());
    head.extend_from_slice(&(h as u32).to_be_bytes());
    head.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(b"IHDR", &head);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut f = std::fs::File::create(path).expect("create the sheet");
    f.write_all(&out).expect("write the sheet");
}

/// Draws `tiles` in rows of `per_row` and writes the PNG.
pub fn sheet(path: &std::path::Path, mesh: Option<&Mesh>, rig: &Rig, body: &Body, tiles: &[Tile], per_row: usize, tile: (usize, usize), scale_px: f32) {
    let rows = tiles.len().div_ceil(per_row);
    let mut cv = Canvas::new(per_row * tile.0, rows * tile.1);
    let rest = rig.rest_world();
    let tri_mat: Vec<u32> = mesh.map(|m| m.tri_materials()).unwrap_or_default();
    for (n, t) in tiles.iter().enumerate() {
        let (ox, oy) = ((n % per_row) * tile.0, (n / per_row) * tile.1);
        let world = rig.forward(&t.poses);
        let skin: Vec<_> = world.iter().zip(&rest).map(|(w, r)| w.then(&r.inverse())).collect();
        let posed = Posed { world: world.clone(), skin, local: t.poses.clone(), shapes: vec![0.0; rig.shapes.len()] };
        // The camera: forward is to the right of the tile, up is up.
        let fwd = normalise(t.xf.world_dir(body.fwd));
        let fwd = [fwd[0], 0.0, fwd[2]];
        let fwd = normalise(fwd);
        let side = [fwd[2], 0.0, -fwd[0]];
        let centre_world = t.xf.pos;
        let ground_y = tile.1 as f32 * 0.88;
        // The soles at rest, in the world: the ground line sits under them.
        let sole_y = t.xf.pos[1] + t.xf.scale * body.sole_height(rig) * body.up[1].signum();
        let to_screen = |p: V3| -> (f32, f32, f32) {
            let d = sub(p, centre_world);
            (ox as f32 + tile.0 as f32 / 2.0 + dot(d, fwd) * scale_px, oy as f32 + ground_y + (p[1] - sole_y) * scale_px, dot(d, side))
        };
        let along0 = dot(sub(centre_world, [0.0, 0.0, 0.0]), fwd);
        for i in -8..=8 {
            // World marks at multiples of 50 px along the heading direction.
            let m = ((along0 / 50.0).floor() + i as f32) * 50.0;
            let x = ox as f32 + tile.0 as f32 / 2.0 + (m - along0) * scale_px;
            cv.line((x, oy as f32 + ground_y + 2.0), (x, oy as f32 + ground_y + 7.0), [120, 120, 120]);
        }
        cv.line((ox as f32, oy as f32 + ground_y + 2.0), ((ox + tile.0) as f32, oy as f32 + ground_y + 2.0), [150, 150, 150]);
        // The skin, flat shaded with a depth buffer.
        if let Some(m) = mesh {
            let (pos, nrm) = rig.deform(m, &posed);
            let mut zbuf = vec![f32::INFINITY; tile.0 * tile.1];
            let light = normalise([0.4, -0.6, -0.7]);
            let sp: Vec<(f32, f32, f32)> = pos.iter().map(|p| to_screen(t.xf.world(*p))).collect();
            for tri in 0..m.triangles() {
                let [a, b, c] = m.tri(tri);
                let col = m.materials.get(tri_mat[tri] as usize).map_or([0.7; 3], |x| [x.base_colour[0], x.base_colour[1], x.base_colour[2]]);
                let nw = t.xf.world_dir(normalise(crate::add(crate::add(nrm[a], nrm[b]), nrm[c])));
                let shade = 0.4 + 0.6 * dot(nw, light).max(0.0);
                let rgb = col.map(|v| (v.powf(1.0 / 2.2) * shade * 255.0).clamp(0.0, 255.0) as u8);
                let (pa, pb, pc) = (sp[a], sp[b], sp[c]);
                let (x0, x1) = (pa.0.min(pb.0).min(pc.0).floor().max(ox as f32), pa.0.max(pb.0).max(pc.0).ceil().min((ox + tile.0 - 1) as f32));
                let (y0, y1) = (pa.1.min(pb.1).min(pc.1).floor().max(oy as f32), pa.1.max(pb.1).max(pc.1).ceil().min((oy + tile.1 - 1) as f32));
                let area = (pb.0 - pa.0) * (pc.1 - pa.1) - (pb.1 - pa.1) * (pc.0 - pa.0);
                if area.abs() < 1e-6 {
                    continue;
                }
                for y in y0 as i32..=y1 as i32 {
                    for x in x0 as i32..=x1 as i32 {
                        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                        let w0 = ((pb.0 - px) * (pc.1 - py) - (pb.1 - py) * (pc.0 - px)) / area;
                        let w1 = ((pc.0 - px) * (pa.1 - py) - (pc.1 - py) * (pa.0 - px)) / area;
                        let w2 = 1.0 - w0 - w1;
                        if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                            continue;
                        }
                        let z = w0 * pa.2 + w1 * pb.2 + w2 * pc.2;
                        let zi = (y as usize - oy) * tile.0 + (x as usize - ox);
                        if z < zbuf[zi] {
                            zbuf[zi] = z;
                            cv.put(x, y, rgb);
                        }
                    }
                }
            }
        }
        // Bones over it: left blue, right orange, the rest dark.
        for (i, b) in rig.bones.iter().enumerate() {
            let head = to_screen(t.xf.world(world[i].pos));
            let tail = to_screen(t.xf.world(world[i].apply([0.0, b.length(), 0.0])));
            let n = b.name.to_lowercase();
            let c = if n.ends_with(".l") || n.contains(".l.") { [30, 90, 220] } else if n.ends_with(".r") || n.contains(".r.") { [230, 120, 20] } else { [40, 40, 40] };
            cv.line((head.0, head.1), (tail.0, tail.1), c);
        }
        // A dot on the ground under every planted foot.
        for p in &t.planted {
            let s = to_screen([p[0], sole_y, p[2]]);
            cv.dot((s.0, oy as f32 + ground_y + 2.0), 3, [220, 20, 20]);
        }
        let _ = &t.label;
        // Tile border.
        cv.line((ox as f32, oy as f32), ((ox + tile.0) as f32, oy as f32), [200, 200, 200]);
        cv.line((ox as f32, oy as f32), (ox as f32, (oy + tile.1) as f32), [200, 200, 200]);
    }
    write_png(path, cv.w, cv.h, &cv.rgb);
}
