// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! CABAC tables: rangeTabLps / transIdxLps (Tables 9-52, 9-53) and the
//! context variable initialisation values (Tables 9-5 .. 9-37).

/// rangeTabLps[pStateIdx][qRangeIdx]
pub const RANGE_TAB_LPS: [[u8; 4]; 64] = [
    [128, 176, 208, 240],
    [128, 167, 197, 227],
    [128, 158, 187, 216],
    [123, 150, 178, 205],
    [116, 142, 169, 195],
    [111, 135, 160, 185],
    [105, 128, 152, 175],
    [100, 122, 144, 166],
    [95, 116, 137, 158],
    [90, 110, 130, 150],
    [85, 104, 123, 142],
    [81, 99, 117, 135],
    [77, 94, 111, 128],
    [73, 89, 105, 122],
    [69, 85, 100, 116],
    [66, 80, 95, 110],
    [62, 76, 90, 104],
    [59, 72, 86, 99],
    [56, 69, 81, 94],
    [53, 65, 77, 89],
    [51, 62, 73, 85],
    [48, 59, 69, 80],
    [46, 56, 66, 76],
    [43, 53, 63, 72],
    [41, 50, 59, 69],
    [39, 48, 56, 65],
    [37, 45, 54, 62],
    [35, 43, 51, 59],
    [33, 41, 48, 56],
    [32, 39, 46, 53],
    [30, 37, 43, 50],
    [29, 35, 41, 48],
    [27, 33, 39, 45],
    [26, 31, 37, 43],
    [24, 30, 35, 41],
    [23, 28, 33, 39],
    [22, 27, 32, 37],
    [21, 26, 30, 35],
    [20, 24, 29, 33],
    [19, 23, 27, 31],
    [18, 22, 26, 30],
    [17, 21, 25, 28],
    [16, 20, 23, 27],
    [15, 19, 22, 25],
    [14, 18, 21, 24],
    [14, 17, 20, 23],
    [13, 16, 19, 22],
    [12, 15, 18, 21],
    [12, 14, 17, 20],
    [11, 14, 16, 19],
    [11, 13, 15, 18],
    [10, 12, 15, 17],
    [10, 12, 14, 16],
    [9, 11, 13, 15],
    [9, 11, 12, 14],
    [8, 10, 12, 14],
    [8, 9, 11, 13],
    [7, 9, 11, 12],
    [7, 9, 10, 12],
    [7, 8, 10, 11],
    [6, 8, 9, 11],
    [6, 7, 9, 10],
    [6, 7, 8, 9],
    [2, 2, 2, 2],
];

/// transIdxLps[pStateIdx]
pub const TRANS_IDX_LPS: [u8; 64] = [
    0, 0, 1, 2, 2, 4, 4, 5, 6, 7, 8, 9, 9, 11, 11, 12, 13, 13, 15, 15, 16, 16, 18, 18, 19, 19, 21,
    21, 22, 22, 23, 24, 24, 25, 26, 26, 27, 27, 28, 29, 29, 30, 30, 30, 31, 32, 32, 33, 33, 33, 34,
    34, 35, 35, 35, 36, 36, 36, 37, 37, 37, 38, 38, 63,
];

/// transIdxMps[pStateIdx]
pub const TRANS_IDX_MPS: [u8; 64] = {
    let mut t = [0u8; 64];
    let mut i = 0;
    while i < 64 {
        t[i] = if i < 62 { (i + 1) as u8 } else { i as u8 };
        i += 1;
    }
    t
};

// ---------------------------------------------------------------------------
// Context index layout. Each syntax element gets a contiguous range.
// ---------------------------------------------------------------------------

pub const SAO_MERGE_FLAG: usize = 0;
pub const SAO_TYPE_IDX: usize = SAO_MERGE_FLAG + 1;
pub const SPLIT_CU_FLAG: usize = SAO_TYPE_IDX + 1;
pub const CU_TRANSQUANT_BYPASS_FLAG: usize = SPLIT_CU_FLAG + 3;
pub const CU_SKIP_FLAG: usize = CU_TRANSQUANT_BYPASS_FLAG + 1;
pub const PRED_MODE_FLAG: usize = CU_SKIP_FLAG + 3;
pub const PART_MODE: usize = PRED_MODE_FLAG + 1;
pub const PREV_INTRA_LUMA_PRED_FLAG: usize = PART_MODE + 4;
pub const INTRA_CHROMA_PRED_MODE: usize = PREV_INTRA_LUMA_PRED_FLAG + 1;
pub const RQT_ROOT_CBF: usize = INTRA_CHROMA_PRED_MODE + 1;
pub const MERGE_FLAG: usize = RQT_ROOT_CBF + 1;
pub const MERGE_IDX: usize = MERGE_FLAG + 1;
pub const INTER_PRED_IDC: usize = MERGE_IDX + 1;
pub const REF_IDX: usize = INTER_PRED_IDC + 5;
pub const MVP_FLAG: usize = REF_IDX + 2;
pub const SPLIT_TRANSFORM_FLAG: usize = MVP_FLAG + 1;
pub const CBF_LUMA: usize = SPLIT_TRANSFORM_FLAG + 3;
pub const CBF_CHROMA: usize = CBF_LUMA + 2;
pub const ABS_MVD_GREATER0: usize = CBF_CHROMA + 5;
pub const ABS_MVD_GREATER1: usize = ABS_MVD_GREATER0 + 1;
pub const CU_QP_DELTA_ABS: usize = ABS_MVD_GREATER1 + 1;
pub const TRANSFORM_SKIP_FLAG: usize = CU_QP_DELTA_ABS + 2; // [luma, chroma]
pub const LAST_X_PREFIX: usize = TRANSFORM_SKIP_FLAG + 2;
pub const LAST_Y_PREFIX: usize = LAST_X_PREFIX + 18;
pub const CODED_SUB_BLOCK_FLAG: usize = LAST_Y_PREFIX + 18;
pub const SIG_COEFF_FLAG: usize = CODED_SUB_BLOCK_FLAG + 4;
pub const GREATER1_FLAG: usize = SIG_COEFF_FLAG + 44;
pub const GREATER2_FLAG: usize = GREATER1_FLAG + 24;
pub const NUM_CTX: usize = GREATER2_FLAG + 6;

const U: u8 = 154; // value for contexts not used with a given initType

/// Builds the init value table for one initType from per-element value lists.
const fn build(parts: &[&[u8]]) -> [u8; NUM_CTX] {
    let mut out = [0u8; NUM_CTX];
    let mut pos = 0;
    let mut p = 0;
    while p < parts.len() {
        let part = parts[p];
        let mut i = 0;
        while i < part.len() {
            out[pos] = part[i];
            pos += 1;
            i += 1;
        }
        p += 1;
    }
    assert!(pos == NUM_CTX);
    out
}

/// INIT_VALUES[initType][ctxIdx]
pub const INIT_VALUES: [[u8; NUM_CTX]; 3] = [
    // initType 0 (I slices)
    build(&[
        &[153],           // sao_merge
        &[200],           // sao_type_idx
        &[139, 141, 157], // split_cu_flag
        &[154],           // cu_transquant_bypass_flag
        &[U, U, U],       // cu_skip_flag
        &[U],             // pred_mode_flag
        &[184, U, U, U],  // part_mode
        &[184],           // prev_intra_luma_pred_flag
        &[63],            // intra_chroma_pred_mode
        &[U],             // rqt_root_cbf
        &[U],             // merge_flag
        &[U],             // merge_idx
        &[U, U, U, U, U], // inter_pred_idc
        &[U, U],          // ref_idx
        &[U],             // mvp_flag
        &[153, 138, 138], // split_transform_flag
        &[111, 141],      // cbf_luma
        &[94, 138, 182, 154, 154], // cbf_cb/cr
        &[U],             // abs_mvd_greater0
        &[U],             // abs_mvd_greater1
        &[154, 154],      // cu_qp_delta_abs
        &[139, 139],      // transform_skip_flag
        &[110, 110, 124, 125, 140, 153, 125, 127, 140, 109, 111, 143, 127, 111, 79, 108, 123, 63],
        &[110, 110, 124, 125, 140, 153, 125, 127, 140, 109, 111, 143, 127, 111, 79, 108, 123, 63],
        &[91, 171, 134, 141], // coded_sub_block_flag
        &[
            111, 111, 125, 110, 110, 94, 124, 108, 124, 107, 125, 141, 179, 153, 125, 107, 125,
            141, 179, 153, 125, 107, 125, 141, 179, 153, 125, 140, 139, 182, 182, 152, 136, 152,
            136, 153, 136, 139, 111, 136, 139, 111, 141, 111,
        ],
        &[
            140, 92, 137, 138, 140, 152, 138, 139, 153, 74, 149, 92, 139, 107, 122, 152, 140, 179,
            166, 182, 140, 227, 122, 197,
        ],
        &[138, 153, 136, 167, 152, 152],
    ]),
    // initType 1
    build(&[
        &[153],
        &[185],
        &[107, 139, 126],
        &[154],
        &[197, 185, 201],
        &[149],
        &[154, 139, 154, 154],
        &[154],
        &[152],
        &[79],
        &[110],
        &[122],
        &[95, 79, 63, 31, 31],
        &[153, 153],
        &[168],
        &[124, 138, 94],
        &[153, 111],
        &[149, 107, 167, 154, 154],
        &[140],
        &[198],
        &[154, 154],
        &[139, 139],
        &[125, 110, 94, 110, 95, 79, 125, 111, 110, 78, 110, 111, 111, 95, 94, 108, 123, 108],
        &[125, 110, 94, 110, 95, 79, 125, 111, 110, 78, 110, 111, 111, 95, 94, 108, 123, 108],
        &[121, 140, 61, 154],
        &[
            155, 154, 139, 153, 139, 123, 123, 63, 153, 166, 183, 140, 136, 153, 154, 166, 183,
            140, 136, 153, 154, 166, 183, 140, 136, 153, 154, 170, 153, 123, 123, 107, 121, 107,
            121, 167, 151, 183, 140, 151, 183, 140, 140, 140,
        ],
        &[
            154, 196, 196, 167, 154, 152, 167, 182, 182, 134, 149, 136, 153, 121, 136, 137, 169,
            194, 166, 167, 154, 167, 137, 182,
        ],
        &[107, 167, 91, 122, 107, 167],
    ]),
    // initType 2
    build(&[
        &[153],
        &[160],
        &[107, 139, 126],
        &[154],
        &[197, 185, 201],
        &[134],
        &[154, 139, 154, 154],
        &[183],
        &[152],
        &[79],
        &[154],
        &[137],
        &[95, 79, 63, 31, 31],
        &[153, 153],
        &[168],
        &[224, 167, 122],
        &[153, 111],
        &[149, 92, 167, 154, 154],
        &[169],
        &[198],
        &[154, 154],
        &[139, 139],
        &[125, 110, 124, 110, 95, 94, 125, 111, 111, 79, 125, 126, 111, 111, 79, 108, 123, 93],
        &[125, 110, 124, 110, 95, 94, 125, 111, 111, 79, 125, 126, 111, 111, 79, 108, 123, 93],
        &[121, 140, 61, 154],
        &[
            170, 154, 139, 153, 139, 123, 123, 63, 124, 166, 183, 140, 136, 153, 154, 166, 183,
            140, 136, 153, 154, 166, 183, 140, 136, 153, 154, 170, 153, 138, 138, 122, 121, 122,
            121, 167, 151, 183, 140, 151, 183, 140, 140, 140,
        ],
        &[
            154, 196, 167, 167, 154, 152, 167, 182, 182, 134, 149, 136, 153, 121, 136, 122, 169,
            208, 166, 167, 154, 152, 167, 182,
        ],
        &[107, 167, 91, 107, 107, 167],
    ]),
];
