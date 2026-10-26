// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Flexible macroblock ordering: macroblock to slice group map (8.2.2).

use crate::params::{Pps, Sps};
use crate::slice::SliceHeader;

/// Slice group of every macroblock of the current picture (8.2.2.8).
pub fn slice_group_map(sps: &Sps, pps: &Pps, hdr: &SliceHeader) -> Vec<u8> {
    let units = map_units(sps, pps, hdr);
    let w = sps.pic_width_in_mbs as usize;
    if sps.frame_mbs_only || hdr.field_pic {
        return units;
    }
    let n = w * sps.frame_height_in_mbs() as usize;
    if sps.mb_adaptive_frame_field {
        (0..n).map(|i| units[i / 2]).collect()
    } else {
        (0..n).map(|i| units[(i / (2 * w)) * w + i % w]).collect()
    }
}

/// mapUnitToSliceGroupMap (8.2.2.1 .. 8.2.2.7).
fn map_units(sps: &Sps, pps: &Pps, hdr: &SliceHeader) -> Vec<u8> {
    let w = sps.pic_width_in_mbs as usize;
    let h = sps.pic_height_in_map_units as usize;
    let size = w * h;
    let num = pps.num_slice_groups as usize;
    let mut map = vec![0u8; size];
    let dir = pps.slice_group_change_direction as usize;
    let units0 = ((hdr.slice_group_change_cycle as u64 * pps.slice_group_change_rate as u64).min(size as u64)) as usize;
    match pps.slice_group_map_type {
        0 => {
            let mut i = 0usize;
            'outer: loop {
                let mut g = 0;
                while g < num && i < size {
                    let run = pps.run_length_minus1.get(g).copied().unwrap_or(0) as usize + 1;
                    let mut j = 0;
                    while j < run && i + j < size {
                        map[i + j] = g as u8;
                        j += 1;
                    }
                    i += run;
                    g += 1;
                }
                if i >= size {
                    break 'outer;
                }
            }
        }
        1 => {
            for i in 0..size {
                map[i] = (((i % w) + (((i / w) * num) / 2)) % num) as u8;
            }
        }
        2 => {
            map.fill((num - 1) as u8);
            for g in (0..num.saturating_sub(1)).rev() {
                let tl = pps.top_left.get(g).copied().unwrap_or(0) as usize;
                let br = pps.bottom_right.get(g).copied().unwrap_or(0) as usize;
                let (yt, xt) = (tl / w, tl % w);
                let (yb, xb) = (br / w, br % w);
                for y in yt..=yb.min(h.saturating_sub(1)) {
                    for x in xt..=xb.min(w - 1) {
                        map[y * w + x] = g as u8;
                    }
                }
            }
        }
        3 => {
            map.fill(1);
            let d = dir as i64;
            let (wi, hi) = (w as i64, h as i64);
            let mut x = (wi - d) / 2;
            let mut y = (hi - d) / 2;
            let (mut left, mut top, mut right, mut bottom) = (x, y, x, y);
            let (mut xdir, mut ydir) = (d - 1, d);
            let mut k = 0usize;
            let mut guard = 0usize;
            while k < units0 {
                guard += 1;
                if guard > 4 * size + 16 {
                    break;
                }
                let idx = (y * wi + x) as usize;
                let vacant = map[idx] == 1;
                if vacant {
                    map[idx] = 0;
                }
                if xdir == -1 && x == left {
                    left = (left - 1).max(0);
                    x = left;
                    xdir = 0;
                    ydir = 2 * d - 1;
                } else if xdir == 1 && x == right {
                    right = (right + 1).min(wi - 1);
                    x = right;
                    xdir = 0;
                    ydir = 1 - 2 * d;
                } else if ydir == -1 && y == top {
                    top = (top - 1).max(0);
                    y = top;
                    xdir = 1 - 2 * d;
                    ydir = 0;
                } else if ydir == 1 && y == bottom {
                    bottom = (bottom + 1).min(hi - 1);
                    y = bottom;
                    xdir = 2 * d - 1;
                    ydir = 0;
                } else {
                    x += xdir;
                    y += ydir;
                }
                if vacant {
                    k += 1;
                }
            }
        }
        4 => {
            let upper = if dir == 1 { size - units0 } else { units0 };
            for i in 0..size {
                map[i] = if i < upper { dir as u8 } else { 1 - dir as u8 };
            }
        }
        5 => {
            let upper = if dir == 1 { size - units0 } else { units0 };
            let mut k = 0;
            for j in 0..w {
                for i in 0..h {
                    map[i * w + j] = if k < upper { dir as u8 } else { 1 - dir as u8 };
                    k += 1;
                }
            }
        }
        _ => {
            for i in 0..size {
                map[i] = pps.slice_group_id.get(i).copied().unwrap_or(0).min(7) as u8;
            }
        }
    }
    map
}
