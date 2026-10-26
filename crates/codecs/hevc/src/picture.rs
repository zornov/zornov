// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Decoded picture storage and per-picture motion data.

/// A sample plane (all bit depths are stored in 16-bit containers).
#[derive(Clone)]
pub struct Plane {
    pub data: Vec<u16>,
    pub width: usize,
    pub height: usize,
    pub stride: usize,
}

impl Plane {
    pub fn new(width: usize, height: usize, fill: u16) -> Plane {
        Plane { data: vec![fill; width * height], width, height, stride: width }
    }
    #[inline(always)]
    pub fn get(&self, x: usize, y: usize) -> u16 {
        self.data[y * self.stride + x]
    }
    #[inline(always)]
    pub fn set(&mut self, x: usize, y: usize, v: u16) {
        self.data[y * self.stride + x] = v;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mv {
    pub x: i16,
    pub y: i16,
}

impl Mv {
    pub const ZERO: Mv = Mv { x: 0, y: 0 };
}

/// Motion of a prediction block. `pred_flag` bit 0 = L0, bit 1 = L1; 0 = intra.
/// Unused lists always carry ref_idx -1 and a zero vector so that `==`
/// implements "same motion vectors and reference indices".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MvField {
    pub mv: [Mv; 2],
    pub ref_idx: [i8; 2],
    pub pred_flag: u8,
}

impl Default for MvField {
    fn default() -> Self {
        MvField { mv: [Mv::ZERO; 2], ref_idx: [-1, -1], pred_flag: 0 }
    }
}

impl MvField {
    #[inline]
    pub fn uses(&self, l: usize) -> bool {
        self.pred_flag & (1 << l) != 0
    }
}

/// Reference picture information of one slice, kept with the picture so that
/// it can later serve as a collocated picture.
#[derive(Clone, Debug, Default)]
pub struct SliceRefs {
    pub poc: [[i32; 16]; 2],
    pub is_lt: [[bool; 16]; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefMark {
    Unused,
    ShortTerm,
    LongTerm,
}

pub struct Picture {
    pub planes: [Plane; 3],
    pub poc: i32,
    pub mark: RefMark,
    pub needed_for_output: bool,
    pub latency: u32,
    /// Motion field at 4x4 granularity.
    pub mvf: Vec<MvField>,
    pub w4: usize,
    /// Slice index (into `slice_refs`) per CTB (raster order).
    pub ctb_slice: Vec<u16>,
    pub slice_refs: Vec<SliceRefs>,
    pub log2_ctb: u8,
    pub pic_width_in_ctbs: usize,
    pub bit_depth_luma: u8,
    pub bit_depth_chroma: u8,
    /// Output cropping (left, right, top, bottom) in luma samples.
    pub crop: [usize; 4],
    pub decode_order: u64,
    pub corrupt: bool,
    /// Index of the SPS VUI colour description at decode time.
    pub full_range: bool,
    pub matrix_coeffs: u8,
}

impl Picture {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        width: usize,
        height: usize,
        bit_depth_luma: u8,
        bit_depth_chroma: u8,
        log2_ctb: u8,
        crop: [usize; 4],
    ) -> Picture {
        let cw = width.div_ceil(2);
        let ch = height.div_ceil(2);
        let w4 = width.div_ceil(4);
        let h4 = height.div_ceil(4);
        let ctb = 1usize << log2_ctb;
        let wc = width.div_ceil(ctb);
        let hc = height.div_ceil(ctb);
        Picture {
            planes: [
                Plane::new(width, height, 1 << (bit_depth_luma - 1)),
                Plane::new(cw, ch, 1 << (bit_depth_chroma - 1)),
                Plane::new(cw, ch, 1 << (bit_depth_chroma - 1)),
            ],
            poc: 0,
            mark: RefMark::Unused,
            needed_for_output: false,
            latency: 0,
            mvf: vec![MvField::default(); w4 * h4],
            w4,
            ctb_slice: vec![0; wc * hc],
            slice_refs: vec![SliceRefs::default()],
            log2_ctb,
            pic_width_in_ctbs: wc,
            bit_depth_luma,
            bit_depth_chroma,
            crop,
            decode_order: 0,
            corrupt: false,
            full_range: false,
            matrix_coeffs: 2,
        }
    }

    /// Resets the metadata of a reused picture (planes keep stale samples).
    pub fn reset(&mut self, crop: [usize; 4]) {
        self.poc = 0;
        self.mark = RefMark::Unused;
        self.needed_for_output = false;
        self.latency = 0;
        for c in self.ctb_slice.iter_mut() {
            *c = 0;
        }
        self.slice_refs.clear();
        self.slice_refs.push(SliceRefs::default());
        self.crop = crop;
        self.decode_order = 0;
        self.corrupt = false;
    }

    pub fn width(&self) -> usize {
        self.planes[0].width
    }
    pub fn height(&self) -> usize {
        self.planes[0].height
    }

    #[inline]
    pub fn mvf_at(&self, x: usize, y: usize) -> &MvField {
        &self.mvf[(y >> 2) * self.w4 + (x >> 2)]
    }

    /// Slice reference info for the slice containing luma sample (x, y).
    pub fn slice_refs_at(&self, x: usize, y: usize) -> &SliceRefs {
        let ctb = (y >> self.log2_ctb) * self.pic_width_in_ctbs + (x >> self.log2_ctb);
        let idx = self.ctb_slice.get(ctb).copied().unwrap_or(0) as usize;
        self.slice_refs.get(idx).unwrap_or(&self.slice_refs[0])
    }
}

/// A raw view of a slice that several threads may access concurrently, each
/// thread touching only elements that no other thread writes at the same
/// time (WPP decoding: every CTU row thread writes only its own CTUs and
/// reads only CTUs that are complete, as published through the row progress
/// counters, which provide the required happens-before ordering).
///
/// Element accesses are bounds checked; slice accessors are `unsafe` because
/// the caller must guarantee exclusivity of the elements for the lifetime of
/// the returned slice.
pub struct SharedSlice<T> {
    ptr: *mut T,
    len: usize,
}

impl<T> Clone for SharedSlice<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for SharedSlice<T> {}

// SAFETY: see the type documentation; concurrent accesses are to disjoint
// elements or are ordered by the WPP progress synchronisation.
unsafe impl<T: Send> Send for SharedSlice<T> {}
unsafe impl<T: Send> Sync for SharedSlice<T> {}

impl<T: Copy> SharedSlice<T> {
    pub fn new(v: &mut [T]) -> Self {
        SharedSlice { ptr: v.as_mut_ptr(), len: v.len() }
    }
    #[inline(always)]
    pub fn get(&self, i: usize) -> T {
        assert!(i < self.len);
        // SAFETY: in bounds; no concurrent writer of element i (type docs).
        unsafe { self.ptr.add(i).read() }
    }
    #[inline(always)]
    pub fn set(&self, i: usize, v: T) {
        assert!(i < self.len);
        // SAFETY: in bounds; element i is owned by the calling thread.
        unsafe { self.ptr.add(i).write(v) }
    }
    /// # Safety
    /// No other reference to elements `a..b` (in any thread) may exist
    /// while the slice is alive.
    #[inline(always)]
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn slice_mut(&self, a: usize, b: usize) -> &mut [T] {
        assert!(a <= b && b <= self.len);
        unsafe { std::slice::from_raw_parts_mut(self.ptr.add(a), b - a) }
    }
    /// Sets elements `a..b` to `v`.
    #[inline(always)]
    pub fn fill(&self, a: usize, b: usize, v: T) {
        // SAFETY: the elements are owned by the calling thread (type docs).
        unsafe { self.slice_mut(a, b) }.fill(v);
    }
}

/// Shared view of a sample plane of the picture being decoded.
#[derive(Clone, Copy)]
pub struct PlaneView {
    pub data: SharedSlice<u16>,
    pub width: usize,
    pub height: usize,
    pub stride: usize,
}

impl PlaneView {
    #[inline(always)]
    pub fn get(&self, x: usize, y: usize) -> u16 {
        self.data.get(y * self.stride + x)
    }
    #[inline(always)]
    pub fn set(&self, x: usize, y: usize, v: u16) {
        self.data.set(y * self.stride + x, v)
    }
    /// Samples `x .. x + w` of row `y` for writing.
    ///
    /// # Safety
    /// The samples must be owned by the calling thread (its own CTU).
    #[inline(always)]
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn row_mut(&self, x: usize, y: usize, w: usize) -> &mut [u16] {
        let a = y * self.stride + x;
        unsafe { self.data.slice_mut(a, a + w) }
    }
}

/// Shared view of the picture being decoded (samples, motion field, CTB
/// slice map) plus the metadata needed during CTU decoding.
#[derive(Clone, Copy)]
pub struct PicView {
    pub planes: [PlaneView; 3],
    pub mvf: SharedSlice<MvField>,
    pub ctb_slice: SharedSlice<u16>,
    pub poc: i32,
    pub w4: usize,
}

impl PicView {
    pub fn new(p: &mut Picture) -> PicView {
        let poc = p.poc;
        let w4 = p.w4;
        let mk = |pl: &mut Plane| PlaneView { data: SharedSlice::new(&mut pl.data), width: pl.width, height: pl.height, stride: pl.stride };
        let [p0, p1, p2] = &mut p.planes;
        PicView { planes: [mk(p0), mk(p1), mk(p2)], mvf: SharedSlice::new(&mut p.mvf), ctb_slice: SharedSlice::new(&mut p.ctb_slice), poc, w4 }
    }
    pub fn width(&self) -> usize {
        self.planes[0].width
    }
    pub fn height(&self) -> usize {
        self.planes[0].height
    }
    #[inline]
    pub fn mvf_at(&self, x: usize, y: usize) -> MvField {
        self.mvf.get((y >> 2) * self.w4 + (x >> 2))
    }
}
