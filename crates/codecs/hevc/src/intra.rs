// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Intra sample prediction (8.4.4.2): reference sample availability and
//! substitution, filtering (incl. strong intra smoothing), planar, DC and the
//! 33 angular modes with boundary filters.

use crate::ctu::{SliceDecoder, F_INTRA};

pub const INTRA_PRED_ANGLE: [i32; 35] = [
    0, 0, 32, 26, 21, 17, 13, 9, 5, 2, 0, -2, -5, -9, -13, -17, -21, -26, -32, -26, -21, -17, -13,
    -9, -5, -2, 0, 2, 5, 9, 13, 17, 21, 26, 32,
];

pub const INV_ANGLE: [i32; 35] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -4096, -1638, -910, -630, -482, -390, -315, -256, -315, -390,
    -482, -630, -910, -1638, -4096, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// Prediction from prepared reference arrays. `left[0] = top[0] = p[-1][-1]`,
/// `left[1 + y] = p[-1][y]`, `top[1 + x] = p[x][-1]` (each 2n + 1 entries).
/// Writes `n * n` samples to `out` (raster).
#[allow(clippy::too_many_arguments)]
pub fn predict_from_refs(
    left: &[i32],
    top: &[i32],
    n: usize,
    log2: u32,
    mode: u8,
    luma_edge_filters: bool,
    bit_depth: u32,
    out: &mut [i32],
) {
    let maxv = (1i32 << bit_depth) - 1;
    match mode {
        0 => {
            // Planar
            let tr = top[1 + n];
            let bl = left[1 + n];
            for y in 0..n {
                for x in 0..n {
                    out[y * n + x] = (((n - 1 - x) as i32) * left[1 + y]
                        + ((x + 1) as i32) * tr
                        + ((n - 1 - y) as i32) * top[1 + x]
                        + ((y + 1) as i32) * bl
                        + n as i32)
                        >> (log2 + 1);
                }
            }
        }
        1 => {
            // DC
            let mut sum = n as i32;
            for i in 0..n {
                sum += top[1 + i] + left[1 + i];
            }
            let dc = sum >> (log2 + 1);
            for v in out[..n * n].iter_mut() {
                *v = dc;
            }
            if luma_edge_filters && n < 32 {
                out[0] = (left[1] + 2 * dc + top[1] + 2) >> 2;
                for x in 1..n {
                    out[x] = (top[1 + x] + 3 * dc + 2) >> 2;
                }
                for y in 1..n {
                    out[y * n] = (left[1 + y] + 3 * dc + 2) >> 2;
                }
            }
        }
        _ => {
            let angle = INTRA_PRED_ANGLE[mode as usize];
            let inv = INV_ANGLE[mode as usize];
            const OFF: usize = 48;
            let mut refb = [0i32; 48 + 65 + 1];
            let (main, side) = if mode >= 18 { (top, left) } else { (left, top) };
            for x in 0..=n {
                refb[OFF + x] = main[x];
            }
            if angle < 0 {
                let lim = (n as i32 * angle) >> 5;
                if lim < -1 {
                    let mut x = lim;
                    while x <= -1 {
                        let idx = ((x * inv + 128) >> 8) as usize;
                        refb[(OFF as i32 + x) as usize] = side[idx];
                        x += 1;
                    }
                }
            } else {
                for x in n + 1..=2 * n {
                    refb[OFF + x] = main[x];
                }
            }
            if mode >= 18 {
                for y in 0..n {
                    let pos = (y as i32 + 1) * angle;
                    let idx = pos >> 5;
                    let fact = pos & 31;
                    let base = (OFF as i32 + idx + 1) as usize;
                    let row = &mut out[y * n..y * n + n];
                    if fact != 0 {
                        for (x, o) in row.iter_mut().enumerate() {
                            *o = ((32 - fact) * refb[base + x] + fact * refb[base + x + 1] + 16) >> 5;
                        }
                    } else {
                        for (x, o) in row.iter_mut().enumerate() {
                            *o = refb[base + x];
                        }
                    }
                }
                if mode == 26 && luma_edge_filters && n < 32 {
                    for y in 0..n {
                        out[y * n] = (top[1] + ((left[1 + y] - left[0]) >> 1)).clamp(0, maxv);
                    }
                }
            } else {
                for x in 0..n {
                    let pos = (x as i32 + 1) * angle;
                    let idx = pos >> 5;
                    let fact = pos & 31;
                    let base = (OFF as i32 + idx + 1) as usize;
                    if fact != 0 {
                        for y in 0..n {
                            out[y * n + x] =
                                ((32 - fact) * refb[base + y] + fact * refb[base + y + 1] + 16) >> 5;
                        }
                    } else {
                        for y in 0..n {
                            out[y * n + x] = refb[base + y];
                        }
                    }
                }
                if mode == 10 && luma_edge_filters && n < 32 {
                    for x in 0..n {
                        out[x] = (left[1] + ((top[1 + x] - top[0]) >> 1)).clamp(0, maxv);
                    }
                }
            }
        }
    }
}

/// Reference sample filtering (8.4.4.2.3) on the linear array
/// `arr[0] = p[-1][2n-1] .. arr[2n-1] = p[-1][0], arr[2n] = p[-1][-1],
/// arr[2n+1 ..= 4n] = p[0..2n-1][-1]`.
pub fn filter_refs(arr: &mut [i32], n: usize, mode: u8, strong_enabled: bool, bit_depth: u32) {
    if mode == 1 || n == 4 {
        return;
    }
    let min_dist = (mode as i32 - 26).abs().min((mode as i32 - 10).abs());
    let thres = match n {
        8 => 7,
        16 => 1,
        32 => 0,
        _ => return,
    };
    if min_dist <= thres {
        return;
    }
    let len = 4 * n + 1;
    let corner = arr[2 * n];
    let bottom = arr[0];
    let right = arr[4 * n];
    if strong_enabled && n == 32 {
        let thr = 1 << (bit_depth - 5);
        let top_mid = arr[3 * n];
        let left_mid = arr[n];
        if (corner + right - 2 * top_mid).abs() < thr && (corner + bottom - 2 * left_mid).abs() < thr {
            // p[-1][y] for y = 0..62 -> index 2n-1-y
            for y in 0..63usize {
                arr[2 * n - 1 - y] = ((63 - y as i32) * corner + (y as i32 + 1) * bottom + 32) >> 6;
            }
            for x in 0..63usize {
                arr[2 * n + 1 + x] = ((63 - x as i32) * corner + (x as i32 + 1) * right + 32) >> 6;
            }
            return;
        }
    }
    let mut prev = arr[0];
    for i in 1..len - 1 {
        let cur = arr[i];
        arr[i] = (prev + 2 * cur + arr[i + 1] + 2) >> 2;
        prev = cur;
    }
}

impl<'a> SliceDecoder<'a> {
    /// Availability of a neighbouring sample for intra prediction, given in
    /// luma coordinates.
    #[inline]
    fn intra_ref_avail(&self, xl: isize, yl: isize) -> bool {
        if !self.avail(xl, yl) {
            return false;
        }
        if self.pps.constrained_intra_pred {
            let f = self.st.flags.get(self.idx4(xl as usize, yl as usize));
            return f & F_INTRA != 0;
        }
        true
    }

    /// Intra prediction of one transform block at component coordinates (x0, y0).
    pub fn predict_intra(&mut self, x0: usize, y0: usize, log2: u32, c_idx: usize, mode: u8) {
        let n = 1usize << log2;
        let shift = if c_idx > 0 { 1 } else { 0 };
        let bit_depth = if c_idx == 0 { self.sps.bit_depth_luma } else { self.sps.bit_depth_chroma } as u32;
        let plane = self.pic.planes[c_idx];
        let (pw, ph) = (plane.width as isize, plane.height as isize);
        let len = 4 * n + 1;
        let mut arr = [0i32; 4 * 64 + 1];
        let mut av = [false; 4 * 64 + 1];
        // granularity of availability: one 4x4 luma block
        let g = 4 >> shift;
        let mut any = false;
        // left column, bottom to top: arr[i] = p[-1][2n-1-i]
        let xl = x0 as isize - 1;
        let mut i = 0usize;
        while i < 2 * n {
            let y = y0 as isize + (2 * n - 1 - i) as isize;
            // block of g samples ending at y going upwards share availability
            let a = xl >= 0 && y < ph && self.intra_ref_avail(xl << shift, y << shift);
            let cnt = g.min(2 * n - i);
            for k in 0..cnt {
                let yy = y - k as isize;
                av[i + k] = a;
                if a {
                    arr[i + k] = plane.get(xl as usize, yy as usize) as i32;
                }
            }
            any |= a;
            i += cnt;
        }
        // corner
        {
            let a = x0 > 0 && y0 > 0 && self.intra_ref_avail(xl << shift, (y0 as isize - 1) << shift);
            av[2 * n] = a;
            if a {
                arr[2 * n] = plane.get(x0 - 1, y0 - 1) as i32;
            }
            any |= a;
        }
        // top row: arr[2n+1+x] = p[x][-1]
        let yt = y0 as isize - 1;
        let mut x = 0usize;
        while x < 2 * n {
            let xx = x0 as isize + x as isize;
            let a = yt >= 0 && xx < pw && self.intra_ref_avail(xx << shift, yt << shift);
            let cnt = g.min(2 * n - x);
            for k in 0..cnt {
                av[2 * n + 1 + x + k] = a;
                if a {
                    arr[2 * n + 1 + x + k] = plane.get(xx as usize + k, yt as usize) as i32;
                }
            }
            any |= a;
            x += cnt;
        }
        // substitution (8.4.4.2.2)
        if !any {
            let v = 1i32 << (bit_depth - 1);
            for e in arr[..len].iter_mut() {
                *e = v;
            }
        } else {
            if !av[0] {
                let mut k = 1;
                while k < len && !av[k] {
                    k += 1;
                }
                arr[0] = arr[k];
            }
            for k in 1..len {
                if !av[k] {
                    arr[k] = arr[k - 1];
                }
            }
        }
        if c_idx == 0 {
            filter_refs(&mut arr[..len], n, mode, self.sps.strong_intra_smoothing_enabled, bit_depth);
        }
        let mut left = [0i32; 65];
        let mut top = [0i32; 65];
        left[0] = arr[2 * n];
        top[0] = arr[2 * n];
        for y in 0..2 * n {
            left[1 + y] = arr[2 * n - 1 - y];
        }
        for x in 0..2 * n {
            top[1 + x] = arr[2 * n + 1 + x];
        }
        let mut out = std::mem::take(&mut self.intra_out);
        predict_from_refs(&left, &top, n, log2, mode, c_idx == 0, bit_depth, &mut out);
        let plane = self.pic.planes[c_idx];
        let w = n.min(plane.width.saturating_sub(x0));
        let h = n.min(plane.height.saturating_sub(y0));
        for y in 0..h {
            // SAFETY: the transform block lies in the CTU owned by this decoder.
            let row = unsafe { plane.row_mut(x0, y0 + y, w) };
            for (d, &v) in row.iter_mut().zip(&out[y * n..y * n + w]) {
                *d = v as u16;
            }
        }
        self.intra_out = out;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs(n: usize, f: impl Fn(isize, isize) -> i32) -> (Vec<i32>, Vec<i32>) {
        let mut left = vec![0; 2 * n + 1];
        let mut top = vec![0; 2 * n + 1];
        left[0] = f(-1, -1);
        top[0] = f(-1, -1);
        for i in 0..2 * n {
            left[1 + i] = f(-1, i as isize);
            top[1 + i] = f(i as isize, -1);
        }
        (left, top)
    }

    #[test]
    fn dc_flat() {
        let (l, t) = refs(8, |_, _| 100);
        let mut out = vec![0; 64];
        predict_from_refs(&l, &t, 8, 3, 1, true, 8, &mut out);
        assert!(out.iter().all(|&v| v == 100));
    }

    #[test]
    fn pure_vertical_and_horizontal() {
        let (l, t) = refs(4, |x, y| if y < 0 { 10 + x as i32 } else { 50 + y as i32 * 3 });
        let mut out = vec![0; 16];
        // mode 26 without edge filter (chroma): copies the top row
        predict_from_refs(&l, &t, 4, 2, 26, false, 8, &mut out);
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(out[y * 4 + x], 10 + x as i32);
            }
        }
        predict_from_refs(&l, &t, 4, 2, 10, false, 8, &mut out);
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(out[y * 4 + x], 50 + y as i32 * 3);
            }
        }
        // with luma edge filter, first column/row adjusted
        predict_from_refs(&l, &t, 4, 2, 26, true, 8, &mut out);
        assert_eq!(out[0], 10 + ((50 - t[0]) >> 1));
    }

    #[test]
    fn diagonal_modes() {
        // mode 34: pred[x][y] = ref[x + y + 2] = p[x + y + 1][-1]
        let (l, t) = refs(4, |x, y| if y < 0 { 7 * (x as i32 + 1) } else { 1 });
        let mut out = vec![0; 16];
        predict_from_refs(&l, &t, 4, 2, 34, true, 8, &mut out);
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(out[y * 4 + x], t[1 + x + y + 1]);
            }
        }
        // mode 2: pred[x][y] = p[-1][x + y + 1]
        let (l, t) = refs(4, |x, y| if x < 0 { 5 * (y as i32 + 2) } else { 1 });
        predict_from_refs(&l, &t, 4, 2, 2, true, 8, &mut out);
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(out[y * 4 + x], l[1 + x + y + 1]);
            }
        }
        // mode 18: pred[x][y] = p[x - y - 1][-1] (top-left diagonal)
        let (l, t) = refs(4, |x, y| (x as i32 + 1) * 3 + (y as i32 + 1) * 20 + 100);
        predict_from_refs(&l, &t, 4, 2, 18, true, 8, &mut out);
        for y in 0..4isize {
            for x in 0..4isize {
                let d = x - y - 1;
                let expect = if d >= -1 { t[(1 + d) as usize] } else { l[(-d - 1) as usize] };
                assert_eq!(out[(y * 4 + x) as usize], expect, "x {x} y {y}");
            }
        }
    }

    #[test]
    fn planar_values() {
        let (l, t) = refs(8, |_, _| 77);
        let mut out = vec![0; 64];
        predict_from_refs(&l, &t, 8, 3, 0, true, 8, &mut out);
        assert!(out.iter().all(|&v| v == 77));
        // left column 0, top row 0 except top-right p[8][-1] = 160:
        // pred[x][y] = ((x + 1) * 160 + 8) >> 4
        let (l, t) = refs(8, |x, y| if y == -1 && x == 8 { 160 } else { 0 });
        predict_from_refs(&l, &t, 8, 3, 0, true, 8, &mut out);
        for y in 0..8 {
            for x in 0..8 {
                assert_eq!(out[y * 8 + x], (((x + 1) * 160 + 8) >> 4) as i32);
            }
        }
    }

    #[test]
    fn filter_decisions() {
        let n = 8;
        let mut arr: Vec<i32> = (0..(4 * n + 1) as i32).map(|i| (i * 7) % 50).collect();
        let orig = arr.clone();
        filter_refs(&mut arr, n, 26, false, 8); // vertical: no filter
        assert_eq!(arr, orig);
        filter_refs(&mut arr, n, 1, false, 8); // DC: no filter
        assert_eq!(arr, orig);
        filter_refs(&mut arr, n, 0, false, 8); // planar: [1 2 1]
        assert_eq!(arr[0], orig[0]);
        assert_eq!(arr[4 * n], orig[4 * n]);
        assert_eq!(arr[5], (orig[4] + 2 * orig[5] + orig[6] + 2) >> 2);
        // strong smoothing on a smooth 32x32 neighbourhood
        let n = 32;
        let mut arr: Vec<i32> = (0..(4 * n + 1) as i32).map(|i| 100 + i / 16).collect();
        let c = arr[64];
        let b = arr[0];
        let r = arr[128];
        filter_refs(&mut arr, n, 0, true, 8);
        assert_eq!(arr[63], ((63 - 0) * c + b + 32) >> 6);
        assert_eq!(arr[65], ((63 - 0) * c + r + 32) >> 6);
    }
}
