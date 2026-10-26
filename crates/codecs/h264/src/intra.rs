// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Intra prediction (8.3): Intra_4x4, Intra_8x8 (with reference sample filtering),
//! Intra_16x16 and chroma.

/// Neighbouring samples of a 4x4 or 8x8 block.
/// `top[0]` is p[-1,-1], `top[1 + x]` is p[x,-1] (x = 0..2N-1);
/// `left[0]` is p[-1,-1], `left[1 + y]` is p[-1,y].
pub struct Edges {
    pub top: [i32; 17],
    pub left: [i32; 9],
    pub has_top: bool,
    pub has_left: bool,
    pub has_topleft: bool,
    pub has_topright: bool,
}

/// Gather neighbours for an NxN block at `off` in a plane with `stride`.
/// The caller guarantees the referenced samples exist when flagged available.
pub fn gather(
    plane: &[u8],
    off: usize,
    stride: usize,
    n: usize,
    has_top: bool,
    has_left: bool,
    has_topleft: bool,
    has_topright: bool,
) -> Edges {
    let mut e = Edges { top: [128; 17], left: [128; 9], has_top, has_left, has_topleft, has_topright };
    if has_top {
        let t = off - stride;
        for x in 0..n {
            e.top[1 + x] = plane[t + x] as i32;
        }
        if has_topright {
            for x in n..2 * n {
                e.top[1 + x] = plane[t + x] as i32;
            }
        } else {
            for x in n..2 * n {
                e.top[1 + x] = e.top[n];
            }
        }
    }
    if has_left {
        for y in 0..n {
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

/// Reference sample filtering for Intra_8x8 (8.3.2.2.1).
pub fn filter8(e: &Edges) -> Edges {
    let mut f = Edges {
        top: e.top,
        left: e.left,
        has_top: e.has_top,
        has_left: e.has_left,
        has_topleft: e.has_topleft,
        has_topright: e.has_topright,
    };
    let p = |x: isize| e.top[(x + 1) as usize]; // p[x,-1]
    let q = |y: isize| e.left[(y + 1) as usize]; // p[-1,y]
    if e.has_top {
        f.top[1] =
            if e.has_topleft { (p(-1) + 2 * p(0) + p(1) + 2) >> 2 } else { (3 * p(0) + p(1) + 2) >> 2 };
        for x in 1..15 {
            f.top[1 + x as usize] = (p(x - 1) + 2 * p(x) + p(x + 1) + 2) >> 2;
        }
        f.top[16] = (p(14) + 3 * p(15) + 2) >> 2;
    }
    if e.has_topleft {
        let v = if !e.has_top || !e.has_left {
            if e.has_top {
                (3 * p(-1) + p(0) + 2) >> 2
            } else if e.has_left {
                (3 * p(-1) + q(0) + 2) >> 2
            } else {
                p(-1)
            }
        } else {
            (p(0) + 2 * p(-1) + q(0) + 2) >> 2
        };
        f.top[0] = v;
        f.left[0] = v;
    }
    if e.has_left {
        f.left[1] =
            if e.has_topleft { (q(-1) + 2 * q(0) + q(1) + 2) >> 2 } else { (3 * q(0) + q(1) + 2) >> 2 };
        for y in 1..7 {
            f.left[1 + y as usize] = (q(y - 1) + 2 * q(y) + q(y + 1) + 2) >> 2;
        }
        f.left[8] = (q(6) + 3 * q(7) + 2) >> 2;
    }
    f
}

/// Directional NxN prediction (N = 4 or 8), modes 0..8 of Intra_4x4 / Intra_8x8.
/// Writes the prediction into `dst` at `off`.
pub fn pred_nxn(mode: u8, e: &Edges, n: usize, dst: &mut [u8], off: usize, stride: usize) {
    let p = |x: isize| e.top[(x + 1) as usize];
    let q = |y: isize| e.left[(y + 1) as usize];
    let ni = n as isize;
    let mut put = |x: usize, y: usize, v: i32| dst[off + y * stride + x] = v as u8;
    match mode {
        0 => {
            for y in 0..n {
                for x in 0..n {
                    put(x, y, p(x as isize));
                }
            }
        }
        1 => {
            for y in 0..n {
                for x in 0..n {
                    put(x, y, q(y as isize));
                }
            }
        }
        2 => {
            let st: i32 = (0..ni).map(p).sum();
            let sl: i32 = (0..ni).map(q).sum();
            let shift = if n == 4 { 2 } else { 3 };
            let v = match (e.has_top, e.has_left) {
                (true, true) => (st + sl + n as i32) >> (shift + 1),
                (false, true) => (sl + (n as i32 >> 1)) >> shift,
                (true, false) => (st + (n as i32 >> 1)) >> shift,
                (false, false) => 128,
            };
            for y in 0..n {
                for x in 0..n {
                    put(x, y, v);
                }
            }
        }
        3 => {
            for y in 0..ni {
                for x in 0..ni {
                    let v = if x == ni - 1 && y == ni - 1 {
                        (p(2 * ni - 2) + 3 * p(2 * ni - 1) + 2) >> 2
                    } else {
                        (p(x + y) + 2 * p(x + y + 1) + p(x + y + 2) + 2) >> 2
                    };
                    put(x as usize, y as usize, v);
                }
            }
        }
        4 => {
            for y in 0..ni {
                for x in 0..ni {
                    let v = if x > y {
                        (p(x - y - 2) + 2 * p(x - y - 1) + p(x - y) + 2) >> 2
                    } else if x < y {
                        (q(y - x - 2) + 2 * q(y - x - 1) + q(y - x) + 2) >> 2
                    } else {
                        (p(0) + 2 * p(-1) + q(0) + 2) >> 2
                    };
                    put(x as usize, y as usize, v);
                }
            }
        }
        5 => {
            for y in 0..ni {
                for x in 0..ni {
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
                    put(x as usize, y as usize, v);
                }
            }
        }
        6 => {
            for y in 0..ni {
                for x in 0..ni {
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
                    put(x as usize, y as usize, v);
                }
            }
        }
        7 => {
            for y in 0..ni {
                for x in 0..ni {
                    let i = x + (y >> 1);
                    let v = if y & 1 == 0 {
                        (p(i) + p(i + 1) + 1) >> 1
                    } else {
                        (p(i) + 2 * p(i + 1) + p(i + 2) + 2) >> 2
                    };
                    put(x as usize, y as usize, v);
                }
            }
        }
        _ => {
            let lim = 2 * ni - 3;
            for y in 0..ni {
                for x in 0..ni {
                    let z = x + 2 * y;
                    let i = y + (x >> 1);
                    let v = if z < lim && z & 1 == 0 {
                        (q(i) + q(i + 1) + 1) >> 1
                    } else if z < lim {
                        (q(i) + 2 * q(i + 1) + q(i + 2) + 2) >> 2
                    } else if z == lim {
                        (q(ni - 2) + 3 * q(ni - 1) + 2) >> 2
                    } else {
                        q(ni - 1)
                    };
                    put(x as usize, y as usize, v);
                }
            }
        }
    }
}

/// Intra_16x16 prediction (8.3.3). `mode`: 0 V, 1 H, 2 DC, 3 Plane.
pub fn pred16x16(mode: u8, plane: &mut [u8], off: usize, stride: usize, has_top: bool, has_left: bool) {
    match mode {
        0 => {
            for y in 1..16 {
                plane.copy_within(off - stride..off - stride + 16, off + (y - 1) * stride);
            }
            plane.copy_within(off - stride..off - stride + 16, off + 15 * stride);
        }
        1 => {
            for y in 0..16 {
                let v = plane[off + y * stride - 1];
                plane[off + y * stride..off + y * stride + 16].fill(v);
            }
        }
        2 => {
            let st: u32 = if has_top { plane[off - stride..off - stride + 16].iter().map(|&v| v as u32).sum() } else { 0 };
            let sl: u32 = if has_left { (0..16).map(|y| plane[off + y * stride - 1] as u32).sum() } else { 0 };
            let v = match (has_top, has_left) {
                (true, true) => (st + sl + 16) >> 5,
                (true, false) => (st + 8) >> 4,
                (false, true) => (sl + 8) >> 4,
                _ => 128,
            } as u8;
            for y in 0..16 {
                plane[off + y * stride..off + y * stride + 16].fill(v);
            }
        }
        _ => {
            let t = |x: isize| plane[(off as isize - stride as isize + x) as usize] as i32;
            let l = |y: isize| plane[(off as isize + y * stride as isize - 1) as usize] as i32;
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
                    plane[off + y * stride + x] = p.clamp(0, 255) as u8;
                }
            }
        }
    }
}

/// 4:2:0 chroma prediction (8.3.4). `mode`: 0 DC, 1 H, 2 V, 3 Plane.
pub fn pred_chroma(mode: u8, plane: &mut [u8], off: usize, stride: usize, has_top: bool, has_left: bool) {
    match mode {
        0 => {
            for by in 0..2 {
                for bx in 0..2 {
                    let o = off + by * 4 * stride + bx * 4;
                    let st: u32 = if has_top { (0..4).map(|x| plane[off - stride + bx * 4 + x] as u32).sum() } else { 0 };
                    let sl: u32 = if has_left { (0..4).map(|y| plane[off + (by * 4 + y) * stride - 1] as u32).sum() } else { 0 };
                    let both = (st + sl + 4) >> 3;
                    let top = (st + 2) >> 2;
                    let left = (sl + 2) >> 2;
                    let v = if (bx == 0 && by == 0) || (bx == 1 && by == 1) {
                        match (has_top, has_left) {
                            (true, true) => both,
                            (false, true) => left,
                            (true, false) => top,
                            _ => 128,
                        }
                    } else if bx == 1 {
                        if has_top {
                            top
                        } else if has_left {
                            left
                        } else {
                            128
                        }
                    } else if has_left {
                        left
                    } else if has_top {
                        top
                    } else {
                        128
                    } as u32;
                    for y in 0..4 {
                        plane[o + y * stride..o + y * stride + 4].fill(v as u8);
                    }
                }
            }
        }
        1 => {
            for y in 0..8 {
                let v = plane[off + y * stride - 1];
                plane[off + y * stride..off + y * stride + 8].fill(v);
            }
        }
        2 => {
            for y in 0..8 {
                plane.copy_within(off - stride..off - stride + 8, off + y * stride);
            }
        }
        _ => {
            let t = |x: isize| plane[(off as isize - stride as isize + x) as usize] as i32;
            let l = |y: isize| plane[(off as isize + y * stride as isize - 1) as usize] as i32;
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
                    plane[off + y * stride + x] = p.clamp(0, 255) as u8;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dc_4x4_variants() {
        let mut buf = vec![0u8; 64];
        let stride = 8;
        let off = stride + 1;
        for x in 0..8 {
            buf[x] = 10; // top row incl. top-left
        }
        for y in 1..8 {
            buf[y * stride] = 30;
        }
        let e = gather(&buf, off, stride, 4, true, true, true, true);
        pred_nxn(2, &e, 4, &mut buf, off, stride);
        assert_eq!(buf[off], 20);
        let e = gather(&buf, off, stride, 4, false, false, false, false);
        pred_nxn(2, &e, 4, &mut buf, off, stride);
        assert_eq!(buf[off], 128);
    }

    #[test]
    fn horizontal_up_tail() {
        let mut buf = vec![0u8; 100];
        let stride = 10;
        let off = stride + 1;
        for y in 0..4 {
            buf[off + y * stride - 1] = (y * 10) as u8;
        }
        let e = gather(&buf, off, stride, 4, false, true, false, false);
        pred_nxn(8, &e, 4, &mut buf, off, stride);
        // zHU > 5 -> p[-1,3]
        assert_eq!(buf[off + 3 * stride + 3], 30);
        // zHU == 5 (x=1,y=2) -> (p[-1,2] + 3p[-1,3] + 2) >> 2 = (20 + 90 + 2) >> 2 = 28
        assert_eq!(buf[off + 2 * stride + 1], 28);
    }
}
