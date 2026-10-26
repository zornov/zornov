// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Decoded picture storage: padded sample planes (frame and, for interlaced streams, the
//! two fields) plus the per-macroblock motion data later pictures need (co-located
//! blocks for direct prediction).

use std::sync::Arc;

/// Luma border (samples) around each plane, enough for clamped 6-tap MC of 16x16 blocks.
pub const PAD_Y: usize = 32;
/// Chroma border.
pub const PAD_C: usize = 16;

/// Picture parity codes used in reference identities.
pub const PAR_TOP: u8 = 0;
pub const PAR_BOTTOM: u8 = 1;
pub const PAR_FRAME: u8 = 2;

#[derive(Clone)]
pub struct Plane {
    pub data: Vec<u8>,
    pub stride: usize,
    pub width: usize,
    pub height: usize,
    pub pad: usize,
}

impl Plane {
    pub fn new(width: usize, height: usize, pad: usize, fill: u8) -> Plane {
        let stride = width + 2 * pad;
        Plane { data: vec![fill; stride * (height + 2 * pad)], stride, width, height, pad }
    }

    /// Index of sample (x, y) (may be negative within the padding).
    #[inline(always)]
    pub fn idx(&self, x: isize, y: isize) -> usize {
        ((y + self.pad as isize) as usize) * self.stride + (x + self.pad as isize) as usize
    }

    /// Replicate edge samples into the padding.
    pub fn extend_borders(&mut self) {
        let (w, h, p, s) = (self.width, self.height, self.pad, self.stride);
        for y in 0..h {
            let row = (y + p) * s;
            let l = self.data[row + p];
            let r = self.data[row + p + w - 1];
            self.data[row..row + p].fill(l);
            self.data[row + p + w..row + s].fill(r);
        }
        let first = p * s;
        let last = (p + h - 1) * s;
        for y in 0..p {
            self.data.copy_within(first..first + s, y * s);
            self.data.copy_within(last..last + s, (p + h + y) * s);
        }
    }

    /// Copy the visible area (cropped) into a tightly packed buffer.
    pub fn crop_to_vec(&self, x0: usize, y0: usize, w: usize, h: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(w * h);
        for y in 0..h {
            let i = self.idx(x0 as isize, (y0 + y) as isize);
            out.extend_from_slice(&self.data[i..i + w]);
        }
        out
    }

    /// The field of the given parity as a separate (padded) plane.
    pub fn field(&self, parity: u8) -> Plane {
        let mut f = Plane::new(self.width, self.height / 2, self.pad, 0);
        for y in 0..f.height {
            let s = self.idx(0, (2 * y + parity as usize) as isize);
            let d = f.idx(0, y as isize);
            f.data[d..d + self.width].copy_from_slice(&self.data[s..s + self.width]);
        }
        f.extend_borders();
        f
    }

    /// Write a field plane into the lines of the given parity.
    pub fn put_field(&mut self, field: &Plane, parity: u8) {
        for y in 0..field.height {
            let s = field.idx(0, y as isize);
            let d = self.idx(0, (2 * y + parity as usize) as isize);
            self.data[d..d + self.width].copy_from_slice(&field.data[s..s + self.width]);
        }
    }
}

/// Luma and chroma planes of a frame or a field.
#[derive(Clone)]
pub struct Planes {
    pub y: Plane,
    pub cb: Plane,
    pub cr: Plane,
}

impl Planes {
    pub fn new(width: usize, height: usize, chroma: bool) -> Planes {
        let (cw, ch) = if chroma { (width / 2, height / 2) } else { (8, 8) };
        Planes { y: Plane::new(width, height, PAD_Y, 0), cb: Plane::new(cw, ch, PAD_C, 128), cr: Plane::new(cw, ch, PAD_C, 128) }
    }

    pub fn extend_borders(&mut self) {
        self.y.extend_borders();
        self.cb.extend_borders();
        self.cr.extend_borders();
    }

    pub fn field(&self, parity: u8, chroma: bool) -> Planes {
        Planes {
            y: self.y.field(parity),
            cb: if chroma { self.cb.field(parity) } else { self.cb.clone() },
            cr: if chroma { self.cr.field(parity) } else { self.cr.clone() },
        }
    }
}

/// Motion data of one macroblock, in the macroblock's own units (field macroblocks carry
/// field motion vectors and field reference indices).
#[derive(Clone, Copy)]
pub struct MbMotion {
    pub intra: bool,
    /// Field macroblock (field picture or MBAFF field macroblock pair).
    pub field: bool,
    pub mv: [[[i16; 2]; 16]; 2],
    pub ref_idx: [[i8; 4]; 2],
    /// Identity of the referenced frame or field (`RefPic::code`), -1 for none.
    pub ref_pic: [[i32; 4]; 2],
}

impl Default for MbMotion {
    fn default() -> Self {
        MbMotion { intra: true, field: false, mv: [[[0; 2]; 16]; 2], ref_idx: [[-1; 4]; 2], ref_pic: [[-1; 4]; 2] }
    }
}

/// How a picture was coded (which motion layout it has).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Structure {
    Frame,
    Mbaff,
    Fields,
}

/// A decoded frame (or complementary field pair) together with its motion information.
pub struct Picture {
    /// Unique id (never reused) used to compare reference pictures.
    pub id: u32,
    pub mb_width: usize,
    /// Height in frame macroblocks.
    pub mb_height: usize,
    pub y: Plane,
    pub cb: Plane,
    pub cr: Plane,
    /// Padded planes of the two fields (interlaced streams only).
    pub fields: [Option<Arc<Planes>>; 2],
    pub poc: i32,
    pub top_poc: i32,
    pub bottom_poc: i32,
    /// True for frames inferred by the frame_num gap process (8.2.5.2).
    pub non_existing: bool,
    pub structure: Structure,
    /// Frame / MBAFF coded: motion per macroblock address.
    pub motion: Vec<MbMotion>,
    /// Field coded: motion per field macroblock address, for each parity.
    pub field_motion: [Vec<MbMotion>; 2],
}

impl Picture {
    pub fn new(id: u32, mb_width: usize, mb_height: usize, chroma: bool) -> Picture {
        let p = Planes::new(mb_width * 16, mb_height * 16, chroma);
        Picture {
            id,
            mb_width,
            mb_height,
            y: p.y,
            cb: p.cb,
            cr: p.cr,
            fields: [None, None],
            poc: 0,
            top_poc: 0,
            bottom_poc: 0,
            non_existing: false,
            structure: Structure::Frame,
            motion: Vec::new(),
            field_motion: [Vec::new(), Vec::new()],
        }
    }

    /// Planes for a reference of the given parity (fields fall back to the frame).
    pub fn planes(&self, parity: u8) -> (&Plane, &Plane, &Plane) {
        if parity != PAR_FRAME {
            if let Some(f) = &self.fields[parity as usize] {
                return (&f.y, &f.cb, &f.cr);
            }
            if let Some(f) = &self.fields[1 - parity as usize] {
                return (&f.y, &f.cb, &f.cr);
            }
        }
        (&self.y, &self.cb, &self.cr)
    }

    /// Build padded field planes from the frame (interlaced streams, frame coded pictures).
    pub fn make_fields(&mut self, chroma: bool) {
        let fr = Planes { y: self.y.clone(), cb: self.cb.clone(), cr: self.cr.clone() };
        self.fields = [Some(Arc::new(fr.field(PAR_TOP, chroma))), Some(Arc::new(fr.field(PAR_BOTTOM, chroma)))];
    }

    pub fn extend_borders(&mut self) {
        self.y.extend_borders();
        self.cb.extend_borders();
        self.cr.extend_borders();
    }

    /// POC of a parity (frame: Min(top, bottom)).
    pub fn poc_of(&self, parity: u8) -> i32 {
        match parity {
            PAR_TOP => self.top_poc,
            PAR_BOTTOM => self.bottom_poc,
            _ => self.poc,
        }
    }
}
