// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Macroblock types (Tables 7-11, 7-13, 7-14, 7-17, 7-18) and stored per-macroblock state.

/// Prediction list usage bits.
pub const PRED_L0: u8 = 1;
pub const PRED_L1: u8 = 2;
pub const PRED_BI: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    P16x16,
    P16x8,
    P8x16,
    P8x8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MbKind {
    /// I_NxN (Intra_4x4 or Intra_8x8 depending on transform_size_8x8_flag).
    INxN,
    /// I_16x16 with prediction mode, chroma cbp, luma cbp (0 or 15).
    I16x16 { mode: u8, cbp_chroma: u8, cbp_luma: u8 },
    IPcm,
    /// SI macroblock (Intra_4x4 prediction, SI decoding of luma).
    Si,
    PSkip,
    BSkip,
    BDirect16x16,
    /// Inter macroblock with partitions (P or B). `ref0` = P_8x8ref0.
    Inter { part: Part, pred: [u8; 2], ref0: bool },
}

impl MbKind {
    pub fn is_intra(self) -> bool {
        matches!(self, MbKind::INxN | MbKind::I16x16 { .. } | MbKind::IPcm | MbKind::Si)
    }
}

/// I mb_type 0..=25 (Table 7-11).
pub fn i_mb_type(t: u32) -> Option<MbKind> {
    match t {
        0 => Some(MbKind::INxN),
        1..=24 => {
            let k = t - 1;
            Some(MbKind::I16x16 { mode: (k % 4) as u8, cbp_chroma: ((k / 4) % 3) as u8, cbp_luma: if t >= 13 { 15 } else { 0 } })
        }
        25 => Some(MbKind::IPcm),
        _ => None,
    }
}

/// P mb_type 0..=4 (Table 7-13).
pub fn p_mb_type(t: u32) -> Option<MbKind> {
    let k = |part, ref0| MbKind::Inter { part, pred: [PRED_L0, PRED_L0], ref0 };
    match t {
        0 => Some(k(Part::P16x16, false)),
        1 => Some(k(Part::P16x8, false)),
        2 => Some(k(Part::P8x16, false)),
        3 => Some(k(Part::P8x8, false)),
        4 => Some(k(Part::P8x8, true)),
        _ => None,
    }
}

/// B mb_type 0..=22 (Table 7-14).
pub fn b_mb_type(t: u32) -> Option<MbKind> {
    const PAIRS: [[u8; 2]; 9] = [
        [PRED_L0, PRED_L0],
        [PRED_L1, PRED_L1],
        [PRED_L0, PRED_L1],
        [PRED_L1, PRED_L0],
        [PRED_L0, PRED_BI],
        [PRED_L1, PRED_BI],
        [PRED_BI, PRED_L0],
        [PRED_BI, PRED_L1],
        [PRED_BI, PRED_BI],
    ];
    match t {
        0 => Some(MbKind::BDirect16x16),
        1 => Some(MbKind::Inter { part: Part::P16x16, pred: [PRED_L0, 0], ref0: false }),
        2 => Some(MbKind::Inter { part: Part::P16x16, pred: [PRED_L1, 0], ref0: false }),
        3 => Some(MbKind::Inter { part: Part::P16x16, pred: [PRED_BI, 0], ref0: false }),
        4..=21 => {
            let part = if t % 2 == 0 { Part::P16x8 } else { Part::P8x16 };
            Some(MbKind::Inter { part, pred: PAIRS[((t - 4) / 2) as usize], ref0: false })
        }
        22 => Some(MbKind::Inter { part: Part::P8x8, pred: [0, 0], ref0: false }),
        _ => None,
    }
}

/// Sub-macroblock description: (number of partitions, width, height in 4x4 units, pred bits).
/// `pred == 0` marks B_Direct_8x8.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SubType {
    pub num: u8,
    pub w: u8,
    pub h: u8,
    pub pred: u8,
}

pub fn p_sub_type(t: u32) -> Option<SubType> {
    match t {
        0 => Some(SubType { num: 1, w: 2, h: 2, pred: PRED_L0 }),
        1 => Some(SubType { num: 2, w: 2, h: 1, pred: PRED_L0 }),
        2 => Some(SubType { num: 2, w: 1, h: 2, pred: PRED_L0 }),
        3 => Some(SubType { num: 4, w: 1, h: 1, pred: PRED_L0 }),
        _ => None,
    }
}

pub fn b_sub_type(t: u32) -> Option<SubType> {
    let s = |num, w, h, pred| Some(SubType { num, w, h, pred });
    match t {
        0 => s(4, 1, 1, 0),
        1 => s(1, 2, 2, PRED_L0),
        2 => s(1, 2, 2, PRED_L1),
        3 => s(1, 2, 2, PRED_BI),
        4 => s(2, 2, 1, PRED_L0),
        5 => s(2, 1, 2, PRED_L0),
        6 => s(2, 2, 1, PRED_L1),
        7 => s(2, 1, 2, PRED_L1),
        8 => s(2, 2, 1, PRED_BI),
        9 => s(2, 1, 2, PRED_BI),
        10 => s(4, 1, 1, PRED_L0),
        11 => s(4, 1, 1, PRED_L1),
        12 => s(4, 1, 1, PRED_BI),
        _ => None,
    }
}

/// Table 9-4: coded_block_pattern codeNum -> cbp for ChromaArrayType 1/2 (intra, inter).
pub const CBP_INTRA: [u8; 48] = [
    47, 31, 15, 0, 23, 27, 29, 30, 7, 11, 13, 14, 39, 43, 45, 46, 16, 3, 5, 10, 12, 19, 21, 26, 28, 35, 37, 42, 44, 1, 2, 4,
    8, 17, 18, 20, 24, 6, 9, 22, 25, 32, 33, 34, 36, 40, 38, 41,
];
pub const CBP_INTER: [u8; 48] = [
    0, 16, 1, 2, 4, 8, 32, 3, 5, 10, 12, 15, 47, 7, 11, 13, 14, 6, 9, 31, 35, 37, 42, 44, 33, 34, 36, 40, 39, 43, 45, 46, 17,
    18, 20, 24, 19, 21, 26, 28, 23, 27, 29, 30, 22, 25, 38, 41,
];
/// Table 9-4 for ChromaArrayType 0/3.
pub const CBP_INTRA_MONO: [u8; 16] = [15, 0, 7, 11, 13, 14, 3, 5, 10, 12, 1, 2, 4, 8, 6, 9];
pub const CBP_INTER_MONO: [u8; 16] = [0, 1, 2, 4, 8, 3, 5, 10, 12, 15, 7, 11, 13, 14, 6, 9];

// MbInfo flags.
pub const F_INTRA: u16 = 1;
pub const F_NXN: u16 = 2;
pub const F_I16: u16 = 4;
pub const F_PCM: u16 = 8;
pub const F_SKIP: u16 = 16;
/// B_Skip or B_Direct_16x16.
pub const F_DIRECT16: u16 = 32;
pub const F_T8X8: u16 = 64;
pub const F_SI: u16 = 128;
/// Macroblock of an MBAFF field macroblock pair.
pub const F_FIELD: u16 = 256;

// coded_block_flag bit positions in MbInfo::cbf.
pub const CBF_LUMA_DC: u32 = 24;
/// Cb DC; Cr DC is the next bit.
pub const CBF_CB_DC: u32 = 25;

/// State of a decoded macroblock of the current picture needed by later macroblocks.
#[derive(Clone, Copy)]
pub struct MbInfo {
    /// Slice index within the picture; u16::MAX = not (yet) decoded.
    pub slice: u16,
    pub flags: u16,
    /// Bits 0..3: CodedBlockPatternLuma, bits 4..5: CodedBlockPatternChroma.
    pub cbp: u8,
    pub chroma_pred: u8,
    /// Intra4x4/8x8 prediction modes per 4x4 block in raster order (y * 4 + x).
    pub ipred: [u8; 16],
    /// CAVLC TotalCoeff per block: luma raster 0..16, Cb 16..20, Cr 20..24 (2x2 raster).
    pub nnz: [u8; 24],
    /// CABAC coded_block_flag bits: luma raster 0..16, Cb AC 16..20, Cr AC 20..24, DC 24..27.
    pub cbf: u32,
    /// B: bit per 8x8 partition predicted in direct mode.
    pub direct8: u8,
    /// CABAC |mvd| per list, per 4x4 block (raster), per component (clipped to 127).
    pub mvd: [[[u8; 2]; 16]; 2],
}

impl Default for MbInfo {
    fn default() -> Self {
        MbInfo {
            slice: u16::MAX,
            flags: 0,
            cbp: 0,
            chroma_pred: 0,
            ipred: [2; 16],
            nnz: [0; 24],
            cbf: 0,
            direct8: 0,
            mvd: [[[0; 2]; 16]; 2],
        }
    }
}

impl MbInfo {
    #[inline]
    pub fn intra(&self) -> bool {
        self.flags & F_INTRA != 0
    }
    #[inline]
    pub fn skip(&self) -> bool {
        self.flags & F_SKIP != 0
    }
    #[inline]
    pub fn pcm(&self) -> bool {
        self.flags & F_PCM != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cbp_tables_are_permutations() {
        let mut a: Vec<u8> = CBP_INTRA.to_vec();
        a.sort();
        assert_eq!(a, (0..48).collect::<Vec<u8>>());
        let mut b: Vec<u8> = CBP_INTER.to_vec();
        b.sort();
        assert_eq!(b, (0..48).collect::<Vec<u8>>());
    }

    #[test]
    fn b_types() {
        assert_eq!(b_mb_type(12), Some(MbKind::Inter { part: Part::P16x8, pred: [PRED_L0, PRED_BI], ref0: false }));
        assert_eq!(b_mb_type(21), Some(MbKind::Inter { part: Part::P8x16, pred: [PRED_BI, PRED_BI], ref0: false }));
        assert_eq!(i_mb_type(13), Some(MbKind::I16x16 { mode: 0, cbp_chroma: 0, cbp_luma: 15 }));
        assert_eq!(i_mb_type(24), Some(MbKind::I16x16 { mode: 3, cbp_chroma: 2, cbp_luma: 15 }));
    }
}
