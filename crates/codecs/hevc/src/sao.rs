// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sample adaptive offset (8.7.3): band offset and edge offset, applied on
//! the deblocked picture with slice/tile boundary handling.
//!
//! The deblocked samples of each plane are copied once; every CTB then reads
//! the copy and writes the picture, so CTB rows can be processed in parallel
//! (each task owns the rows of one CTB row of one plane).

use std::sync::Mutex;

use crate::ctu::{PicState, SaoParams, F_NOFILTER};
use crate::params::{Pps, Sps, TileLayout};
use crate::picture::Picture;
use crate::pool::Pool;
use crate::slice::SliceHeader;

#[derive(Clone, Copy)]
struct PlaneRef<'a> {
    data: &'a [u16],
    width: usize,
    height: usize,
    stride: usize,
}

const EO_POS: [[(isize, isize); 2]; 4] = [
    [(-1, 0), (1, 0)],
    [(0, -1), (0, 1)],
    [(-1, -1), (1, 1)],
    [(1, -1), (-1, 1)],
];

struct Ctx<'a> {
    st: &'a PicState,
    sps: &'a Sps,
    pps: &'a Pps,
    headers: &'a [SliceHeader],
    layout: &'a TileLayout,
    poc: i32,
}

pub fn sao_picture(
    pic: &mut Picture,
    st: &PicState,
    sps: &Sps,
    pps: &Pps,
    headers: &[SliceHeader],
    layout: &TileLayout,
    scratch: &mut Vec<u16>,
    pool: Option<&Pool>,
) {
    let nctb = sps.pic_size_in_ctbs();
    let mut used = [false; 3];
    for rs in 0..nctb {
        if st.ctb_hdr[rs] == u16::MAX {
            continue;
        }
        for (c, u) in used.iter_mut().enumerate() {
            if st.sao[rs].type_idx[c] != 0 {
                *u = true;
            }
        }
    }
    if sps.chroma_format_idc == 0 {
        used[1] = false;
        used[2] = false;
    }
    if !used.iter().any(|&u| u) {
        return;
    }
    // Copy of the deblocked planes (the SAO input).
    let mut offs = [0usize; 4];
    scratch.clear();
    for c in 0..3 {
        offs[c] = scratch.len();
        if used[c] {
            scratch.extend_from_slice(&pic.planes[c].data);
        }
    }
    offs[3] = scratch.len();
    let cx = Ctx { st, sps, pps, headers, layout, poc: pic.poc };
    let hc = sps.pic_height_in_ctbs;
    // Split each used plane into CTB-row bands.
    let mut tasks: Vec<(usize, usize, Mutex<&mut [u16]>)> = Vec::new();
    let mut src_refs: [Option<PlaneRef>; 3] = [None, None, None];
    for (c, plane) in pic.planes.iter_mut().enumerate() {
        if !used[c] {
            continue;
        }
        src_refs[c] = Some(PlaneRef {
            data: &scratch[offs[c]..offs[c + 1]],
            width: plane.width,
            height: plane.height,
            stride: plane.stride,
        });
        let shift = if c > 0 { 1 } else { 0 };
        let rows = sps.ctb_size >> shift;
        let stride = plane.stride;
        let data = &mut plane.data[..plane.height * stride];
        for (ry, band) in data.chunks_mut(rows * stride).enumerate().take(hc) {
            tasks.push((c, ry, Mutex::new(band)));
        }
    }
    let run = |i: usize| {
        let (c, ry, ref band) = tasks[i];
        let mut band = band.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(src) = src_refs[c] {
            sao_ctb_row(&cx, c, ry, src, &mut band);
        }
    };
    match pool {
        Some(p) => p.run(tasks.len(), &run),
        None => (0..tasks.len()).for_each(run),
    }
}

/// Applies SAO to all CTBs of CTB row `ry` of component `c`. `dst` holds the
/// picture rows of that CTB row (starting at its first row).
fn sao_ctb_row(cx: &Ctx, c: usize, ry: usize, src: PlaneRef, dst: &mut [u16]) {
    let st = cx.st;
    let sps = cx.sps;
    let wc = sps.pic_width_in_ctbs;
    let shift = if c > 0 { 1 } else { 0 };
    let ctb_size = sps.ctb_size >> shift;
    let bd = if c == 0 { sps.bit_depth_luma } else { sps.bit_depth_chroma } as u32;
    let maxv = (1i32 << bd) - 1;
    let (pw, ph) = (src.width, src.height);
    let stride = src.stride;
    let y0 = ry * ctb_size;
    let y1 = (y0 + ctb_size).min(ph);
    for rx in 0..wc {
        let rs = ry * wc + rx;
        let hi = st.ctb_hdr[rs];
        if hi == u16::MAX {
            continue;
        }
        let p = &st.sao[rs];
        let ty = p.type_idx[c];
        if ty == 0 {
            continue;
        }
        if let Some(dp) = crate::debug::opts().dump_sao {
            if dp == cx.poc {
                eprintln!("poc {} c {} ctb ({},{}) {:?}", cx.poc, c, rx, ry, p);
            }
        }
        let cur_hdr = match cx.headers.get(hi as usize) {
            Some(h) => h,
            None => continue,
        };
        let x0 = rx * ctb_size;
        let x1 = (x0 + ctb_size).min(pw);
        // Samples that must not be modified exist in this CTB?
        let nofilter_ctb = st.any_nofilter && {
            let mut any = false;
            for y in (y0 << shift >> 2)..((y1 << shift).div_ceil(4)) {
                for x in (x0 << shift >> 2)..((x1 << shift).div_ceil(4)) {
                    any |= st.flags[y * st.w4 + x] & F_NOFILTER != 0;
                }
            }
            any
        };
        let nofilter = |x: usize, y: usize| -> bool {
            nofilter_ctb && st.flags[((y << shift) >> 2) * st.w4 + ((x << shift) >> 2)] & F_NOFILTER != 0
        };
        let offs = p.offset[c];
        if ty == 1 {
            let band_shift = bd - 5;
            let mut table = [0i32; 32];
            for k in 0..4 {
                table[(k + p.band_position[c] as usize) & 31] = offs[k + 1] as i32;
            }
            for y in y0..y1 {
                let s = &src.data[y * stride + x0..y * stride + x1];
                let d = &mut dst[(y - y0) * stride + x0..(y - y0) * stride + x1];
                if nofilter_ctb {
                    for (i, (o, &v)) in d.iter_mut().zip(s).enumerate() {
                        if !nofilter(x0 + i, y) {
                            let v = v as i32;
                            *o = (v + table[(v >> band_shift) as usize & 31]).clamp(0, maxv) as u16;
                        }
                    }
                } else {
                    bo_row(d, s, &table, band_shift, maxv);
                }
            }
            continue;
        }
        // Edge offset: neighbour CTB usability (3x3 around the current CTB).
        let ok = neighbour_usable(cx, rs, rx, ry, cur_hdr);
        let cls = p.eo_class[c] as usize & 3;
        let [(ax, ay), (bx, by)] = EO_POS[cls];
        let region = |v: isize, lo: usize| -> usize {
            if v < lo as isize {
                0
            } else if v >= (lo + ctb_size) as isize {
                2
            } else {
                1
            }
        };
        let (o1, o2, o3, o4) = (offs[1] as i32, offs[2] as i32, offs[3] as i32, offs[4] as i32);
        let edge_off = |v: i32, a: i32, b: i32| -> i32 {
            let s = ((v > a) as i32 - (v < a) as i32) + ((v > b) as i32 - (v < b) as i32);
            if s == -2 {
                o1
            } else if s == -1 {
                o2
            } else if s == 1 {
                o3
            } else if s == 2 {
                o4
            } else {
                0
            }
        };
        // Generic per-sample path (picture / CTB borders and no-filter samples).
        let one = |dst: &mut [u16], x: usize, y: usize| {
            let xa = x as isize + ax;
            let ya = y as isize + ay;
            let xb = x as isize + bx;
            let yb = y as isize + by;
            if xa < 0 || ya < 0 || xb < 0 || yb < 0 {
                return;
            }
            if xa as usize >= pw || xb as usize >= pw || ya as usize >= ph || yb as usize >= ph {
                return;
            }
            if !ok[region(ya, y0)][region(xa, x0)] || !ok[region(yb, y0)][region(xb, x0)] {
                return;
            }
            if nofilter(x, y) {
                return;
            }
            let v = src.data[y * stride + x] as i32;
            let a = src.data[ya as usize * stride + xa as usize] as i32;
            let b = src.data[yb as usize * stride + xb as usize] as i32;
            let e = edge_off(v, a, b);
            if e != 0 {
                dst[(y - y0) * stride + x] = (v + e).clamp(0, maxv) as u16;
            }
        };
        if nofilter_ctb {
            for y in y0..y1 {
                for x in x0..x1 {
                    one(dst, x, y);
                }
            }
            continue;
        }
        // Interior columns x0 + 1 .. x1 - 1: horizontal neighbours lie inside
        // the CTB and the picture, so only the row decides usability.
        let (xi0, xi1) = (x0 + 1, x1.saturating_sub(1).max(x0 + 1));
        for y in y0..y1 {
            let ya = y as isize + ay;
            let yb = y as isize + by;
            let row_ok = ya >= 0
                && yb >= 0
                && (ya as usize) < ph
                && (yb as usize) < ph
                && ok[region(ya, y0)][1]
                && ok[region(yb, y0)][1];
            if row_ok && xi1 > xi0 {
                let ra = ya as usize * stride;
                let rb = yb as usize * stride;
                let cur = &src.data[y * stride + xi0..y * stride + xi1];
                let na = &src.data[(ra + xi0).wrapping_add_signed(ax)..(ra + xi1).wrapping_add_signed(ax)];
                let nb = &src.data[(rb + xi0).wrapping_add_signed(bx)..(rb + xi1).wrapping_add_signed(bx)];
                let d = &mut dst[(y - y0) * stride + xi0..(y - y0) * stride + xi1];
                eo_row(d, cur, na, nb, [o1 as i16, o2 as i16, o3 as i16, o4 as i16], maxv as i16);
            }
            one(dst, x0, y);
            if x1 - 1 > x0 {
                one(dst, x1 - 1, y);
            }
        }
    }
}

/// Edge offset of one row segment (vectorisable leaf).
#[inline(never)]
fn eo_row(d: &mut [u16], cur: &[u16], na: &[u16], nb: &[u16], o: [i16; 4], maxv: i16) {
    let n = d.len().min(cur.len()).min(na.len()).min(nb.len());
    let (d, cur, na, nb) = (&mut d[..n], &cur[..n], &na[..n], &nb[..n]);
    let [o1, o2, o3, o4] = o;
    for i in 0..n {
        let (v, a, b) = (cur[i] as i16, na[i] as i16, nb[i] as i16);
        let s = ((v > a) as i16 - (v < a) as i16) + ((v > b) as i16 - (v < b) as i16);
        let e = if s == -2 {
            o1
        } else if s == -1 {
            o2
        } else if s == 1 {
            o3
        } else if s == 2 {
            o4
        } else {
            0
        };
        d[i] = (v + e).max(0).min(maxv) as u16;
    }
}

/// Band offset of one row segment.
#[inline(never)]
fn bo_row(d: &mut [u16], s: &[u16], table: &[i32; 32], band_shift: u32, maxv: i32) {
    for (o, &v) in d.iter_mut().zip(s) {
        let v = v as i32;
        *o = (v + table[(v >> band_shift) as usize & 31]).max(0).min(maxv) as u16;
    }
}

/// Usability of the 3x3 neighbourhood of CTB `rs` for edge offset
/// (slice / tile boundaries with loop filtering disabled, undecoded CTBs).
fn neighbour_usable(cx: &Ctx, rs: usize, rx: usize, ry: usize, cur_hdr: &SliceHeader) -> [[bool; 3]; 3] {
    let st = cx.st;
    let sps = cx.sps;
    let layout = cx.layout;
    let wc = sps.pic_width_in_ctbs;
    let cur_ts = layout.rs_to_ts[rs];
    let mut ok = [[false; 3]; 3];
    for (dy, row) in ok.iter_mut().enumerate() {
        for (dx, o) in row.iter_mut().enumerate() {
            let nx = rx as isize + dx as isize - 1;
            let ny = ry as isize + dy as isize - 1;
            if nx < 0 || ny < 0 || nx as usize >= wc || ny as usize >= sps.pic_height_in_ctbs {
                continue;
            }
            let nrs = ny as usize * wc + nx as usize;
            if nrs == rs {
                *o = true;
                continue;
            }
            let nh = st.ctb_hdr[nrs];
            if nh == u16::MAX {
                continue;
            }
            let nts = layout.rs_to_ts[nrs];
            let mut usable = true;
            if st.ctb_slice_addr[nrs] != st.ctb_slice_addr[rs] {
                let flag = if nts < cur_ts {
                    cur_hdr.loop_filter_across_slices_enabled
                } else {
                    cx.headers.get(nh as usize).map(|h| h.loop_filter_across_slices_enabled).unwrap_or(false)
                };
                if !flag {
                    usable = false;
                }
            }
            if !cx.pps.loop_filter_across_tiles_enabled && layout.tile_id[nts] != layout.tile_id[cur_ts] {
                usable = false;
            }
            *o = usable;
        }
    }
    ok
}

// Keep the parameter type referenced for documentation purposes.
const _: Option<SaoParams> = None;
