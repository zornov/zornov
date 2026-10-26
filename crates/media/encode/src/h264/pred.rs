// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Intra prediction (8.3) and inter prediction from padded reference pictures
//! (8.4.2.2). All predictions are written into 16-stride buffers.

/// Neighbouring samples of a 4x4 block: `top[0]` = p[-1,-1], `top[1 + x]` =
/// p[x,-1] for x = 0..7; `left[0]` = p[-1,-1], `left[1 + y]` = p[-1,y].
#[derive(Clone, Copy)]
pub struct Edges4 {
    pub top: [i32; 9],
    pub left: [i32; 5],
    pub has_top: bool,
    pub has_left: bool,
    pub has_topleft: bool,
}

/// Gather the neighbours of the 4x4 block at `off` of `plane` (stride `stride`).
pub fn gather4(plane: &[u8], off: usize, stride: usize, has_top: bool, has_left: bool, has_topleft: bool, has_topright: bool) -> Edges4 {
    let mut e = Edges4 { top: [128; 9], left: [128; 5], has_top, has_left, has_topleft };
    if has_top {
        let t = off - stride;
        for x in 0..4 {
            e.top[1 + x] = plane[t + x] as i32;
        }
        for x in 4..8 {
            e.top[1 + x] = if has_topright { plane[t + x] as i32 } else { e.top[4] };
        }
    }
    if has_left {
        for y in 0..4 {
            e.left[1 + y] = plane[off + y * stride - 1] as i32;
        }
    }
    if has_topleft {
        let v = plane[off - stride - 1] as i32;
        e.top[0] = v;
        e.left[0] = v;
    }
    e
}

/// Whether Intra_4x4 mode `mode` may be used with these neighbours.
pub fn mode4_allowed(mode: u8, e: &Edges4) -> bool {
    match mode {
        0 | 3 | 7 => e.has_top,
        1 | 8 => e.has_left,
        2 => true,
        _ => e.has_top && e.has_left && e.has_topleft,
    }
}

/// Intra_4x4 prediction of `mode` into `dst` (stride 16) at `off`.
pub fn pred4(mode: u8, e: &Edges4, dst: &mut [u8], off: usize) {
    let p = |x: isize| e.top[(x + 1) as usize];
    let q = |y: isize| e.left[(y + 1) as usize];
    let mut put = |x: isize, y: isize, v: i32| dst[off + y as usize * 16 + x as usize] = v as u8;
    match mode {
        0 => {
            for y in 0..4 {
                for x in 0..4 {
                    put(x, y, p(x));
                }
            }
        }
        1 => {
            for y in 0..4 {
                for x in 0..4 {
                    put(x, y, q(y));
                }
            }
        }
        2 => {
            let st: i32 = (0..4).map(p).sum();
            let sl: i32 = (0..4).map(q).sum();
            let v = match (e.has_top, e.has_left) {
                (true, true) => (st + sl + 4) >> 3,
                (false, true) => (sl + 2) >> 2,
                (true, false) => (st + 2) >> 2,
                (false, false) => 128,
            };
            for y in 0..4 {
                for x in 0..4 {
                    put(x, y, v);
                }
            }
        }
        3 => {
            for y in 0..4 {
                for x in 0..4 {
                    let v = if x == 3 && y == 3 { (p(6) + 3 * p(7) + 2) >> 2 } else { (p(x + y) + 2 * p(x + y + 1) + p(x + y + 2) + 2) >> 2 };
                    put(x, y, v);
                }
            }
        }
        4 => {
            for y in 0..4 {
                for x in 0..4 {
                    let v = if x > y {
                        (p(x - y - 2) + 2 * p(x - y - 1) + p(x - y) + 2) >> 2
                    } else if x < y {
                        (q(y - x - 2) + 2 * q(y - x - 1) + q(y - x) + 2) >> 2
                    } else {
                        (p(0) + 2 * p(-1) + q(0) + 2) >> 2
                    };
                    put(x, y, v);
                }
            }
        }
        5 => {
            for y in 0..4isize {
                for x in 0..4isize {
                    let z = 2 * x - y;
                    let v = if z >= 0 && z & 1 == 0 {
                        (p(x - (y >> 1) - 1) + p(x - (y >> 1)) + 1) >> 1
                    } else if z >= 0 {
                        (p(x - (y >> 1) - 2) + 2 * p(x - (y >> 1) - 1) + p(x - (y >> 1)) + 2) >> 2
                    } else if z == -1 {
                        (q(0) + 2 * q(-1) + p(0) + 2) >> 2
                    } else {
                        (q(y - 2 * x - 1) + 2 * q(y - 2 * x - 2) + q(y - 2 * x - 3) + 2) >> 2
                    };
                    put(x, y, v);
                }
            }
        }
        6 => {
            for y in 0..4isize {
                for x in 0..4isize {
                    let z = 2 * y - x;
                    let v = if z >= 0 && z & 1 == 0 {
                        (q(y - (x >> 1) - 1) + q(y - (x >> 1)) + 1) >> 1
                    } else if z >= 0 {
                        (q(y - (x >> 1) - 2) + 2 * q(y - (x >> 1) - 1) + q(y - (x >> 1)) + 2) >> 2
                    } else if z == -1 {
                        (q(0) + 2 * q(-1) + p(0) + 2) >> 2
                    } else {
                        (p(x - 2 * y - 1) + 2 * p(x - 2 * y - 2) + p(x - 2 * y - 3) + 2) >> 2
                    };
                    put(x, y, v);
                }
            }
        }
        7 => {
            for y in 0..4isize {
                for x in 0..4isize {
                    let i = x + (y >> 1);
                    let v = if y & 1 == 0 { (p(i) + p(i + 1) + 1) >> 1 } else { (p(i) + 2 * p(i + 1) + p(i + 2) + 2) >> 2 };
                    put(x, y, v);
                }
            }
        }
        _ => {
            for y in 0..4isize {
                for x in 0..4isize {
                    let z = x + 2 * y;
                    let i = y + (x >> 1);
                    let v = if z < 5 && z & 1 == 0 {
                        (q(i) + q(i + 1) + 1) >> 1
                    } else if z < 5 {
                        (q(i) + 2 * q(i + 1) + q(i + 2) + 2) >> 2
                    } else if z == 5 {
                        (q(2) + 3 * q(3) + 2) >> 2
                    } else {
                        q(3)
                    };
                    put(x, y, v);
                }
            }
        }
    }
}

/// Neighbours of a 16x16 luma or 8x8 chroma block.
#[derive(Clone, Copy)]
pub struct EdgesN {
    pub top: [u8; 16],
    pub left: [u8; 16],
    pub topleft: u8,
    pub has_top: bool,
    pub has_left: bool,
    pub has_topleft: bool,
}

pub fn gather_n(plane: &[u8], off: usize, stride: usize, n: usize, has_top: bool, has_left: bool, has_topleft: bool) -> EdgesN {
    let mut e = EdgesN { top: [0; 16], left: [0; 16], topleft: 0, has_top, has_left, has_topleft };
    if has_top {
        e.top[..n].copy_from_slice(&plane[off - stride..off - stride + n]);
    }
    if has_left {
        for y in 0..n {
            e.left[y] = plane[off + y * stride - 1];
        }
    }
    if has_topleft {
        e.topleft = plane[off - stride - 1];
    }
    e
}

/// Intra 16x16 (mode 0 V, 1 H, 2 DC, 3 Plane) and chroma (0 DC, 1 H, 2 V,
/// 3 Plane) availability.
pub fn mode16_allowed(mode: u8, e: &EdgesN) -> bool {
    match mode {
        0 => e.has_top,
        1 => e.has_left,
        2 => true,
        _ => e.has_top && e.has_left && e.has_topleft,
    }
}

pub fn chroma_mode_allowed(mode: u8, e: &EdgesN) -> bool {
    match mode {
        0 => true,
        1 => e.has_left,
        2 => e.has_top,
        _ => e.has_top && e.has_left && e.has_topleft,
    }
}

/// Intra_16x16 prediction into `out` (stride 16).
pub fn pred16(mode: u8, e: &EdgesN, out: &mut [u8; 256]) {
    match mode {
        0 => {
            for y in 0..16 {
                out[y * 16..y * 16 + 16].copy_from_slice(&e.top);
            }
        }
        1 => {
            for y in 0..16 {
                out[y * 16..y * 16 + 16].fill(e.left[y]);
            }
        }
        2 => {
            let st: u32 = e.top.iter().map(|&v| v as u32).sum();
            let sl: u32 = e.left.iter().map(|&v| v as u32).sum();
            let v = match (e.has_top, e.has_left) {
                (true, true) => (st + sl + 16) >> 5,
                (true, false) => (st + 8) >> 4,
                (false, true) => (sl + 8) >> 4,
                _ => 128,
            } as u8;
            out.fill(v);
        }
        _ => {
            let t = |x: isize| if x < 0 { e.topleft as i32 } else { e.top[x as usize] as i32 };
            let l = |y: isize| if y < 0 { e.topleft as i32 } else { e.left[y as usize] as i32 };
            let mut h = 0;
            let mut v = 0;
            for i in 0..8isize {
                h += (i as i32 + 1) * (t(8 + i) - t(6 - i));
                v += (i as i32 + 1) * (l(8 + i) - l(6 - i));
            }
            let a = 16 * (l(15) + t(15));
            let b = (5 * h + 32) >> 6;
            let c = (5 * v + 32) >> 6;
            for y in 0..16 {
                for x in 0..16 {
                    let p = (a + b * (x as i32 - 7) + c * (y as i32 - 7) + 16) >> 5;
                    out[y * 16 + x] = p.clamp(0, 255) as u8;
                }
            }
        }
    }
}

/// 4:2:0 chroma prediction (8.3.4) into `out` (stride 16, 8x8 used).
pub fn pred_chroma(mode: u8, e: &EdgesN, out: &mut [u8; 256]) {
    match mode {
        0 => {
            for by in 0..2 {
                for bx in 0..2 {
                    let st: u32 = if e.has_top { (0..4).map(|x| e.top[bx * 4 + x] as u32).sum() } else { 0 };
                    let sl: u32 = if e.has_left { (0..4).map(|y| e.left[by * 4 + y] as u32).sum() } else { 0 };
                    let both = (st + sl + 4) >> 3;
                    let top = (st + 2) >> 2;
                    let left = (sl + 2) >> 2;
                    let v = if (bx == 0 && by == 0) || (bx == 1 && by == 1) {
                        match (e.has_top, e.has_left) {
                            (true, true) => both,
                            (false, true) => left,
                            (true, false) => top,
                            _ => 128,
                        }
                    } else if bx == 1 {
                        if e.has_top {
                            top
                        } else if e.has_left {
                            left
                        } else {
                            128
                        }
                    } else if e.has_left {
                        left
                    } else if e.has_top {
                        top
                    } else {
                        128
                    };
                    for y in 0..4 {
                        let o = (by * 4 + y) * 16 + bx * 4;
                        out[o..o + 4].fill(v as u8);
                    }
                }
            }
        }
        1 => {
            for y in 0..8 {
                out[y * 16..y * 16 + 8].fill(e.left[y]);
            }
        }
        2 => {
            for y in 0..8 {
                out[y * 16..y * 16 + 8].copy_from_slice(&e.top[..8]);
            }
        }
        _ => {
            let t = |x: isize| if x < 0 { e.topleft as i32 } else { e.top[x as usize] as i32 };
            let l = |y: isize| if y < 0 { e.topleft as i32 } else { e.left[y as usize] as i32 };
            let mut h = 0;
            let mut v = 0;
            for i in 0..4isize {
                h += (i as i32 + 1) * (t(4 + i) - t(2 - i));
                v += (i as i32 + 1) * (l(4 + i) - l(2 - i));
            }
            let a = 16 * (l(7) + t(7));
            let b = (34 * h + 32) >> 6;
            let c = (34 * v + 32) >> 6;
            for y in 0..8 {
                for x in 0..8 {
                    let p = (a + b * (x as i32 - 3) + c * (y as i32 - 3) + 16) >> 5;
                    out[y * 16 + x] = p.clamp(0, 255) as u8;
                }
            }
        }
    }
}

// ---- reference pictures -------------------------------------------------

/// Luma padding around reference pictures (motion vectors are limited so
/// that every interpolation tap stays inside it).
pub const PAD: usize = 48;
/// Chroma padding.
pub const CPAD: usize = 24;
/// How far a 16x16 block may point outside the picture, in luma samples.
pub const MV_OUTSIDE: i32 = 32;

#[inline(always)]
fn clip(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

#[inline(always)]
fn tap6(a: i32, b: i32, c: i32, d: i32, e: i32, f: i32) -> i32 {
    a - 5 * b + 20 * c + 20 * d - 5 * e + f
}

/// A deblocked reference picture with edge-replicated padding and the three
/// half-sample planes (b: horizontal, h: vertical, j: centre).
pub struct RefPic {
    pub stride: usize,
    pub y: Vec<u8>,
    pub half_h: Vec<u8>,
    pub half_v: Vec<u8>,
    pub half_c: Vec<u8>,
    pub cstride: usize,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

fn pad_plane(src: &[u8], w: usize, h: usize, pad: usize) -> (Vec<u8>, usize) {
    let stride = w + 2 * pad;
    let rows = h + 2 * pad;
    let mut out = vec![0u8; stride * rows];
    for r in 0..rows {
        let sy = r.saturating_sub(pad).min(h - 1);
        let line = &src[sy * w..sy * w + w];
        let o = r * stride;
        out[o..o + pad].fill(line[0]);
        out[o + pad..o + pad + w].copy_from_slice(line);
        out[o + pad + w..o + stride].fill(line[w - 1]);
    }
    (out, stride)
}

impl RefPic {
    /// Build from deblocked planes of the coded size.
    pub fn new(y: &[u8], u: &[u8], v: &[u8], width: usize, height: usize) -> RefPic {
        let (py, stride) = pad_plane(y, width, height, PAD);
        let (pu, cstride) = pad_plane(u, width / 2, height / 2, CPAD);
        let (pv, _) = pad_plane(v, width / 2, height / 2, CPAD);
        let rows = height + 2 * PAD;
        let mut half_h = py.clone();
        let mut half_v = py.clone();
        let mut half_c = py.clone();
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 16);
        let band = rows.div_ceil(threads);
        std::thread::scope(|s| {
            let src = &py;
            let it = half_h.chunks_mut(band * stride).zip(half_v.chunks_mut(band * stride)).zip(half_c.chunks_mut(band * stride)).enumerate();
            for (i, ((hh, hv), c)) in it {
                s.spawn(move || {
                    let r0 = i * band;
                    let nrows = hh.len() / stride;
                    for rr in 0..nrows {
                        let r = r0 + rr;
                        if r < 3 || r + 3 >= rows {
                            continue;
                        }
                        let at = |x: usize, y: usize| src[y * stride + x] as i32;
                        for x in 3..stride - 3 {
                            let hb = tap6(at(x - 2, r), at(x - 1, r), at(x, r), at(x + 1, r), at(x + 2, r), at(x + 3, r));
                            hh[rr * stride + x] = clip((hb + 16) >> 5);
                            let vb = tap6(at(x, r - 2), at(x, r - 1), at(x, r), at(x, r + 1), at(x, r + 2), at(x, r + 3));
                            hv[rr * stride + x] = clip((vb + 16) >> 5);
                            let row = |y: usize| tap6(at(x - 2, y), at(x - 1, y), at(x, y), at(x + 1, y), at(x + 2, y), at(x + 3, y));
                            let j = tap6(row(r - 2), row(r - 1), row(r), row(r + 1), row(r + 2), row(r + 3));
                            c[rr * stride + x] = clip((j + 512) >> 10);
                        }
                    }
                });
            }
        });
        RefPic { stride, y: py, half_h, half_v, half_c, cstride, u: pu, v: pv }
    }

    /// Index of luma sample (x, y) (may be negative, inside the padding).
    #[inline]
    pub fn idx(&self, x: i32, y: i32) -> usize {
        ((y + PAD as i32) as usize) * self.stride + (x + PAD as i32) as usize
    }

    #[inline]
    pub fn cidx(&self, x: i32, y: i32) -> usize {
        ((y + CPAD as i32) as usize) * self.cstride + (x + CPAD as i32) as usize
    }

    /// The plane holding samples at quarter-sample fraction (fx, fy), each 0 or 2.
    #[inline]
    pub fn plane(&self, fx: i32, fy: i32) -> &[u8] {
        match (fx, fy) {
            (0, 0) => &self.y,
            (2, 0) => &self.half_h,
            (0, _) => &self.half_v,
            _ => &self.half_c,
        }
    }

    /// 16x16 luma prediction for the block at (bx, by) with half-sample motion
    /// vector `mv` (quarter units, even components) into `out` (stride 16).
    pub fn luma16(&self, bx: i32, by: i32, mv: [i32; 2], out: &mut [u8; 256]) {
        let p = self.plane(mv[0] & 3, mv[1] & 3);
        let o = self.idx(bx + (mv[0] >> 2), by + (mv[1] >> 2));
        for r in 0..16 {
            out[r * 16..r * 16 + 16].copy_from_slice(&p[o + r * self.stride..o + r * self.stride + 16]);
        }
    }

    /// 8x8 chroma prediction (both planes) for macroblock chroma origin (cx, cy).
    pub fn chroma8(&self, cx: i32, cy: i32, mv: [i32; 2], out_u: &mut [u8; 256], out_v: &mut [u8; 256]) {
        let (fx, fy) = ((mv[0] & 7) as u32, (mv[1] & 7) as u32);
        let base = self.cidx(cx + (mv[0] >> 3), cy + (mv[1] >> 3));
        let s = self.cstride;
        let wa = (8 - fx) * (8 - fy);
        let wb = fx * (8 - fy);
        let wc = (8 - fx) * fy;
        let wd = fx * fy;
        for (plane, out) in [(&self.u, out_u), (&self.v, out_v)] {
            for j in 0..8 {
                let o = base + j * s;
                for i in 0..8 {
                    let a = plane[o + i] as u32;
                    let b = plane[o + i + 1] as u32;
                    let c = plane[o + s + i] as u32;
                    let d = plane[o + s + i + 1] as u32;
                    out[j * 16 + i] = ((wa * a + wb * b + wc * c + wd * d + 32) >> 6) as u8;
                }
            }
        }
    }
}
