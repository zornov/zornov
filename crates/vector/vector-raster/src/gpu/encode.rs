// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Coarse rasterization on the CPU: the prepared tree becomes, for every
//! 16×16 tile, a list of commands for the fine shader. A fill lists the
//! segments that cross the tile plus, for each of its 16 rows, the winding
//! of the segments wholly to its left; a tile no edge crosses gets one
//! uniform coverage. The shader then integrates exactly the same per-pixel
//! area as the CPU renderer.

use crate::paint::{Extend, ImageQuality, LUT_SIZE};
use crate::prepare::{IRect, ImgSrc, Lines, PGroup, PNode, PPaint};
use crate::scene::FillRule;

pub(crate) const TILE: i32 = 16;

pub(crate) const OP_FILL: u32 = 1;
pub(crate) const OP_SOLID: u32 = 2;
pub(crate) const OP_PUSH: u32 = 3;
pub(crate) const OP_POP: u32 = 4;

pub(crate) const PAINT_SOLID: u32 = 0;
pub(crate) const PAINT_LINEAR: u32 = 1;
pub(crate) const PAINT_RADIAL: u32 = 2;
pub(crate) const PAINT_CONIC: u32 = 3;
pub(crate) const PAINT_FREEFORM: u32 = 4;
pub(crate) const PAINT_IMAGE: u32 = 5;

/// Words per paint record.
pub(crate) const PAINT_WORDS: usize = 16;
/// The fixed-size stack of isolated groups each pixel keeps.
pub(crate) const MAX_DEPTH: usize = 8;

/// Where every image the pass may sample lives in the image buffer.
pub(crate) struct ImageTable<'a> {
    /// For each prepass, its descriptor index (`u32::MAX` if not on the GPU).
    pub prepass_desc: &'a [u32],
    /// Looks up (or adds) a user image, returning its descriptor index.
    pub user: &'a mut dyn FnMut(&crate::paint::Image) -> u32,
}

/// One pass's encoded commands and data.
#[derive(Default)]
pub(crate) struct Encoded {
    /// Tile table (`offset`, `count` into `udata` per tile), then commands,
    /// segment lists and paints.
    pub udata: Vec<u32>,
    /// Segments, row backdrops, gradient tables and freeform points.
    pub fdata: Vec<[f32; 4]>,
    pub paint_off: u32,
    pub tiles_x: u32,
    pub tiles_y: u32,
    pub depth_exceeded: bool,
}

struct Enc<'a, 'b> {
    region: IRect,
    tiles_x: i32,
    cmds: Vec<Vec<u32>>,
    fdata: Vec<[f32; 4]>,
    /// Indices into `fdata` of segments, per tile list.
    lists: Vec<u32>,
    paints: Vec<u32>,
    images: &'a mut ImageTable<'b>,
    depth: usize,
    exceeded: bool,
    // Reused per band.
    delta: Vec<[f32; 16]>,
    col_lists: Vec<Vec<u32>>,
}

/// How a fill or clip covers one tile.
enum TileCov {
    /// Exact per pixel: segment list, its length, and the 16 row backdrops.
    Geom { list: u32, count: u32, backdrop: u32 },
    /// The same coverage everywhere in the tile.
    Uniform(f32),
}

fn fold(rule: FillRule, w: f32) -> f32 {
    crate::cpu::fold(rule, w)
}

pub(crate) fn encode(root: &PGroup, region: IRect, images: &mut ImageTable) -> Encoded {
    let tiles_x = (region.width() + TILE - 1) / TILE;
    let tiles_y = (region.height() + TILE - 1) / TILE;
    let mut e = Enc {
        region,
        tiles_x,
        cmds: vec![Vec::new(); (tiles_x * tiles_y) as usize],
        fdata: Vec::new(),
        lists: Vec::new(),
        paints: Vec::new(),
        images,
        depth: 0,
        exceeded: false,
        delta: Vec::new(),
        col_lists: Vec::new(),
    };
    e.group(root);
    // Lay out: tile table, commands, segment lists, paints.
    let ntiles = (tiles_x * tiles_y) as usize;
    let mut udata = vec![0u32; ntiles * 2];
    let cmd_words: usize = e.cmds.iter().map(Vec::len).sum();
    udata.reserve(cmd_words + e.lists.len() + e.paints.len());
    // Command offsets are filled first; lists and paints follow, and
    // commands refer to them by offsets relative to their own starts, fixed
    // up below.
    let lists_base = (ntiles * 2 + cmd_words) as u32;
    let paint_base = lists_base + e.lists.len() as u32;
    for (t, c) in e.cmds.iter().enumerate() {
        udata[t * 2] = udata.len() as u32;
        udata[t * 2 + 1] = c.len() as u32;
        let mut i = 0;
        while i < c.len() {
            let op = c[i];
            match op {
                OP_FILL => {
                    // [op, list, count, backdrop, flags, paint, blend, opacity]
                    udata.extend_from_slice(&[op, c[i + 1] + lists_base, c[i + 2], c[i + 3], c[i + 4], c[i + 5] + paint_base, c[i + 6], c[i + 7]]);
                    i += 8;
                }
                OP_SOLID => {
                    // [op, coverage, paint, blend, opacity]
                    udata.extend_from_slice(&[op, c[i + 1], c[i + 2] + paint_base, c[i + 3], c[i + 4]]);
                    i += 5;
                }
                OP_PUSH => {
                    udata.push(op);
                    i += 1;
                }
                _ => {
                    // [op, blend, opacity, list, count, backdrop, flags, mask desc, mask flags]
                    let has_geom = c[i + 6] & 4 != 0;
                    let list = if has_geom { c[i + 3] + lists_base } else { c[i + 3] };
                    udata.extend_from_slice(&[op, c[i + 1], c[i + 2], list, c[i + 4], c[i + 5], c[i + 6], c[i + 7], c[i + 8]]);
                    i += 9;
                }
            }
        }
    }
    debug_assert_eq!(udata.len() as u32, lists_base);
    udata.extend_from_slice(&e.lists);
    udata.extend_from_slice(&e.paints);
    Encoded { udata, fdata: e.fdata, paint_off: paint_base, tiles_x: tiles_x as u32, tiles_y: tiles_y as u32, depth_exceeded: e.exceeded }
}

impl Enc<'_, '_> {
    fn tile_range(&self, r: IRect) -> IRect {
        let r = r.intersect(&self.region);
        if r.is_empty() {
            return IRect::default();
        }
        let ox = self.region.x0;
        let oy = self.region.y0;
        IRect::new((r.x0 - ox) / TILE, (r.y0 - oy) / TILE, (r.x1 - ox + TILE - 1) / TILE, (r.y1 - oy + TILE - 1) / TILE)
    }

    fn tile_index(&self, tx: i32, ty: i32) -> usize {
        (ty * self.tiles_x + tx) as usize
    }

    fn group(&mut self, g: &PGroup) {
        for child in &g.children {
            match child {
                PNode::Draw(d) => {
                    let paint = self.paint(&d.paint);
                    let Some(paint) = paint else { continue };
                    let (blend, opacity) = (d.blend.index(), d.opacity.to_bits());
                    let rule_flags = (d.lines.rule == FillRule::EvenOdd) as u32 | ((d.lines.invert as u32) << 1);
                    self.bin(&d.lines, |e, t, cov| match cov {
                        TileCov::Geom { list, count, backdrop } => e.cmds[t].extend_from_slice(&[OP_FILL, list, count, backdrop, rule_flags, paint, blend, opacity]),
                        TileCov::Uniform(c) => {
                            if c > 0.0 {
                                e.cmds[t].extend_from_slice(&[OP_SOLID, c.to_bits(), paint, blend, opacity]);
                            }
                        }
                    });
                }
                PNode::Group(sub) => self.subgroup(sub),
            }
        }
    }

    fn subgroup(&mut self, sub: &PGroup) {
        let tr = self.tile_range(sub.bbox);
        if tr.is_empty() {
            return;
        }
        // Per tile clip coverage (None = no clip).
        let n = (tr.width() * tr.height()) as usize;
        let mut clip: Vec<Option<TileCovWords>> = vec![None; n];
        if let Some(cl) = &sub.clip {
            // Tiles the clip's binning never visits are wholly outside it
            // (or wholly inside, for an inverted clip).
            let outside = if cl.invert { 1.0 } else { 0.0 };
            for c in clip.iter_mut() {
                *c = Some(TileCovWords::Uniform(outside));
            }
            let flags = (cl.rule == FillRule::EvenOdd) as u32 | ((cl.invert as u32) << 1) | 4;
            let mut found: Vec<(usize, TileCovWords)> = Vec::new();
            self.bin(cl, |e, t, cov| {
                let (tx, ty) = (t as i32 % e.tiles_x, t as i32 / e.tiles_x);
                if tx >= tr.x0 && tx < tr.x1 && ty >= tr.y0 && ty < tr.y1 {
                    let k = ((ty - tr.y0) * tr.width() + (tx - tr.x0)) as usize;
                    found.push((
                        k,
                        match cov {
                            TileCov::Geom { list, count, backdrop } => TileCovWords::Geom([list, count, backdrop, flags]),
                            TileCov::Uniform(c) => TileCovWords::Uniform(c),
                        },
                    ));
                }
            });
            for (k, v) in found {
                clip[k] = Some(v);
            }
        }
        let (mask_desc, mask_flags) = match &sub.mask {
            Some(m) => (self.images.prepass_desc[m.prepass], (m.kind == crate::scene::MaskKind::Alpha) as u32 | ((m.invert as u32) << 1)),
            None => (u32::MAX, 0),
        };
        // Tiles where the group shows at all.
        let mut live = vec![false; n];
        for ty in tr.y0..tr.y1 {
            for tx in tr.x0..tr.x1 {
                let k = ((ty - tr.y0) * tr.width() + (tx - tr.x0)) as usize;
                let shows = !matches!(clip[k], Some(TileCovWords::Uniform(c)) if c <= 0.0);
                live[k] = shows;
                if shows {
                    let t = self.tile_index(tx, ty);
                    self.cmds[t].push(OP_PUSH);
                }
            }
        }
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            self.exceeded = true;
        }
        // Children only in live tiles: record their commands, then drop those
        // that landed in tiles the clip hides.
        let before: Vec<usize> = (0..self.cmds.len()).map(|t| self.cmds[t].len()).collect();
        self.group(sub);
        for ty in tr.y0..tr.y1 {
            for tx in tr.x0..tr.x1 {
                let k = ((ty - tr.y0) * tr.width() + (tx - tr.x0)) as usize;
                let t = self.tile_index(tx, ty);
                if !live[k] {
                    self.cmds[t].truncate(before[t]);
                    continue;
                }
                let (list, count, backdrop, flags) = match &clip[k] {
                    None => (0, 0, 0, 0),
                    Some(TileCovWords::Uniform(c)) => (c.to_bits(), 0, 0, 8),
                    Some(TileCovWords::Geom(w)) => (w[0], w[1], w[2], w[3]),
                };
                self.cmds[t].extend_from_slice(&[OP_POP, sub.blend.index(), sub.opacity.to_bits(), list, count, backdrop, flags, mask_desc, mask_flags]);
            }
        }
        self.depth -= 1;
    }

    /// Bins `lines` into the tiles of its bbox, calling `emit` with each
    /// tile's coverage.
    fn bin(&mut self, lines: &Lines, mut emit: impl FnMut(&mut Self, usize, TileCov)) {
        let tr = self.tile_range(lines.bbox);
        if tr.is_empty() {
            return;
        }
        let ncols = tr.width() as usize;
        let (ox, oy) = (self.region.x0 as f32, self.region.y0 as f32);
        // Segments go into fdata once, shared by every tile that lists them.
        let seg_base = self.fdata.len() as u32;
        self.fdata.extend(lines.segs.iter().copied());
        let mut delta = std::mem::take(&mut self.delta);
        let mut cols = std::mem::take(&mut self.col_lists);
        cols.resize_with(ncols, Vec::new);
        for ty in tr.y0..tr.y1 {
            let by0 = oy + (ty * TILE) as f32;
            let by1 = by0 + TILE as f32;
            delta.clear();
            delta.resize(ncols + 1, [0.0; 16]);
            for c in cols.iter_mut() {
                c.clear();
            }
            for (i, s) in lines.segs.iter().enumerate() {
                let (ymin, ymax) = (s[1].min(s[3]), s[1].max(s[3]));
                if ymax <= by0 || ymin >= by1 || s[1] == s[3] {
                    continue;
                }
                // The segment's x extent within the band.
                let (x0, y0, x1, y1) = (s[0], s[1], s[2], s[3]);
                let at = |y: f32| x0 + (y - y0) * (x1 - x0) / (y1 - y0);
                let (ya, yb) = (ymin.max(by0), ymax.min(by1));
                let (xa, xb) = (at(ya), at(yb));
                let (xmin, xmax) = (xa.min(xb), xa.max(xb));
                let cmin = ((xmin - ox) / TILE as f32).floor() as i64 - tr.x0 as i64;
                let cmax = ((xmax - ox) / TILE as f32).floor() as i64 - tr.x0 as i64;
                if cmin >= ncols as i64 {
                    continue;
                }
                let first_right = cmax + 1;
                if first_right <= ncols as i64 - 1 || cmax < 0 {
                    // Rows' winding for tiles to the right.
                    let sign = if y1 > y0 { 1.0 } else { -1.0 };
                    let k = first_right.max(0) as usize;
                    for r in 0..16 {
                        let ry0 = by0 + r as f32;
                        let lo = ymin.max(ry0);
                        let hi = ymax.min(ry0 + 1.0);
                        if hi > lo {
                            delta[k][r] += (hi - lo) * sign;
                        }
                    }
                }
                for c in cmin.max(0)..=cmax.min(ncols as i64 - 1) {
                    cols[c as usize].push(seg_base + i as u32);
                }
            }
            let mut running = [0.0f32; 16];
            for c in 0..ncols {
                for r in 0..16 {
                    running[r] += delta[c][r];
                }
                let t = self.tile_index(tr.x0 + c as i32, ty);
                let uniform = cols[c].is_empty() && running.iter().all(|v| (v - running[0]).abs() < 1e-5);
                if uniform {
                    let cov = fold(lines.rule, running[0]);
                    emit(self, t, TileCov::Uniform(if lines.invert { 1.0 - cov } else { cov }));
                } else {
                    let list = self.lists.len() as u32;
                    self.lists.extend_from_slice(&cols[c]);
                    let backdrop = self.fdata.len() as u32;
                    self.fdata.extend_from_slice(&[
                        [running[0], running[1], running[2], running[3]],
                        [running[4], running[5], running[6], running[7]],
                        [running[8], running[9], running[10], running[11]],
                        [running[12], running[13], running[14], running[15]],
                    ]);
                    emit(self, t, TileCov::Geom { list, count: cols[c].len() as u32, backdrop });
                }
            }
        }
        self.delta = delta;
        self.col_lists = cols;
    }

    /// Writes a paint record; returns its offset within the paint words.
    fn paint(&mut self, p: &PPaint) -> Option<u32> {
        let mut w = [0u32; PAINT_WORDS];
        let f = |v: f32| v.to_bits();
        let ext = |e: Extend| match e {
            Extend::Pad => 0,
            Extend::Repeat => 1,
            Extend::Reflect => 2,
            Extend::None => 3,
        };
        let put_inv = |w: &mut [u32; PAINT_WORDS], inv: &[f32; 6]| {
            for (i, v) in inv.iter().enumerate() {
                w[4 + i] = f(*v);
            }
        };
        let lut = |fdata: &mut Vec<[f32; 4]>, l: &crate::paint::Lut| {
            let off = fdata.len() as u32;
            debug_assert_eq!(l.0.len(), LUT_SIZE);
            fdata.extend_from_slice(&l.0);
            off
        };
        match p {
            PPaint::Solid(c) => {
                w[0] = PAINT_SOLID;
                for i in 0..4 {
                    w[4 + i] = f(c[i]);
                }
            }
            PPaint::Linear { inv, lut: l, extend } => {
                w[0] = PAINT_LINEAR;
                w[1] = ext(*extend);
                w[2] = lut(&mut self.fdata, l);
                put_inv(&mut w, inv);
            }
            PPaint::Radial { inv, lut: l, extend, c0, r0, c1, r1 } => {
                w[0] = PAINT_RADIAL;
                w[1] = ext(*extend);
                w[2] = lut(&mut self.fdata, l);
                put_inv(&mut w, inv);
                w[10] = f(c0[0]);
                w[11] = f(c0[1]);
                w[12] = f(*r0);
                w[13] = f(c1[0]);
                w[14] = f(c1[1]);
                w[15] = f(*r1);
            }
            PPaint::Conic { inv, lut: l, extend } => {
                w[0] = PAINT_CONIC;
                w[1] = ext(*extend);
                w[2] = lut(&mut self.fdata, l);
                put_inv(&mut w, inv);
            }
            PPaint::Freeform { pts } => {
                w[0] = PAINT_FREEFORM;
                w[2] = self.fdata.len() as u32;
                w[3] = pts.len() as u32;
                for (p, c, s) in pts {
                    self.fdata.push([p[0], p[1], *s, 0.0]);
                    self.fdata.push(*c);
                }
            }
            PPaint::Image { src, inv, extend, quality, opacity, lod } => {
                let desc = match src {
                    ImgSrc::Prepass(i) => self.images.prepass_desc[*i],
                    ImgSrc::User(img) => (self.images.user)(img),
                };
                if desc == u32::MAX {
                    return None;
                }
                w[0] = PAINT_IMAGE;
                w[1] = ext(*extend);
                w[2] = desc;
                w[3] = match quality {
                    ImageQuality::Nearest => 0,
                    ImageQuality::Bilinear => 1,
                    ImageQuality::Bicubic => 2,
                };
                put_inv(&mut w, inv);
                w[10] = f(*opacity);
                w[11] = f(*lod);
            }
        }
        let off = self.paints.len() as u32;
        self.paints.extend_from_slice(&w);
        Some(off)
    }
}

#[derive(Clone, Copy)]
enum TileCovWords {
    Geom([u32; 4]),
    Uniform(f32),
}
