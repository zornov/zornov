// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Constant tables from ITU-T H.264.

/// 4x4 zig-zag scan (frame), Table 8-13: scan index -> raster index (row * 4 + col).
pub const ZIGZAG4: [u8; 16] = [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15];

/// 8x8 zig-zag scan (frame), Table 8-14: scan index -> raster index (row * 8 + col).
#[rustfmt::skip]
pub const ZIGZAG8: [u8; 64] = [
     0,  1,  8, 16,  9,  2,  3, 10, 17, 24, 32, 25, 18, 11,  4,  5,
    12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13,  6,  7, 14, 21, 28,
    35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51,
    58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// 4x4 field scan, Table 8-13 (field macroblocks): scan index -> raster index.
pub const FIELD4: [u8; 16] = [0, 4, 1, 8, 12, 5, 9, 13, 2, 6, 10, 14, 3, 7, 11, 15];

/// 8x8 field scan, Table 8-14 (field macroblocks): scan index -> raster index.
#[rustfmt::skip]
pub const FIELD8: [u8; 64] = [
     0,  8, 16,  1,  9, 24, 32, 17,  2, 25, 40, 48, 56, 33, 10,  3,
    18, 41, 49, 57, 26, 11,  4, 19, 34, 42, 50, 58, 27, 12,  5, 20,
    35, 43, 51, 59, 28, 13,  6, 21, 36, 44, 52, 60, 29, 14, 22, 37,
    45, 53, 61, 30,  7, 15, 38, 46, 54, 62, 23, 31, 39, 47, 55, 63,
];

/// Table 7-3, in zig-zag order.
pub const DEFAULT_4X4_INTRA: [u8; 16] = [6, 13, 13, 20, 20, 20, 28, 28, 28, 28, 32, 32, 32, 37, 37, 42];
pub const DEFAULT_4X4_INTER: [u8; 16] = [10, 14, 14, 20, 20, 20, 24, 24, 24, 24, 27, 27, 27, 30, 30, 34];

/// Table 7-4, in zig-zag order.
#[rustfmt::skip]
pub const DEFAULT_8X8_INTRA: [u8; 64] = [
     6, 10, 10, 13, 11, 13, 16, 16, 16, 16, 18, 18, 18, 18, 18, 23,
    23, 23, 23, 23, 23, 25, 25, 25, 25, 25, 25, 25, 27, 27, 27, 27,
    27, 27, 27, 27, 29, 29, 29, 29, 29, 29, 29, 31, 31, 31, 31, 31,
    31, 33, 33, 33, 33, 33, 36, 36, 36, 36, 38, 38, 38, 40, 40, 42,
];
#[rustfmt::skip]
pub const DEFAULT_8X8_INTER: [u8; 64] = [
     9, 13, 13, 15, 13, 15, 17, 17, 17, 17, 19, 19, 19, 19, 19, 21,
    21, 21, 21, 21, 21, 22, 22, 22, 22, 22, 22, 22, 24, 24, 24, 24,
    24, 24, 24, 24, 25, 25, 25, 25, 25, 25, 25, 27, 27, 27, 27, 27,
    27, 28, 28, 28, 28, 28, 30, 30, 30, 30, 32, 32, 32, 33, 33, 35,
];

/// Table 8-15: QPc as a function of qPI.
#[rustfmt::skip]
pub const CHROMA_QP: [u8; 52] = [
     0,  1,  2,  3,  4,  5,  6,  7,  8,  9, 10, 11, 12, 13, 14, 15,
    16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 29, 30,
    31, 32, 32, 33, 34, 34, 35, 35, 36, 36, 37, 37, 37, 38, 38, 38,
    39, 39, 39, 39,
];

/// QPc for a luma QP and a chroma_qp_index_offset (8-bit video).
#[inline]
pub fn chroma_qp(qp_y: i32, offset: i32) -> i32 {
    CHROMA_QP[(qp_y + offset).clamp(0, 51) as usize] as i32
}

/// v matrix of (8-315).
const V4: [[i32; 3]; 6] = [[10, 16, 13], [11, 18, 14], [13, 20, 16], [14, 23, 18], [16, 25, 20], [18, 29, 23]];

/// v matrix of (8-318).
const V8: [[i32; 6]; 6] = [
    [20, 18, 32, 19, 25, 24],
    [22, 19, 35, 21, 28, 26],
    [26, 23, 42, 24, 33, 31],
    [28, 25, 45, 26, 35, 33],
    [32, 28, 51, 30, 40, 38],
    [36, 32, 58, 34, 46, 43],
];

/// normAdjust4x4(m, i, j) for raster position `pos`.
pub const fn norm_adjust4(m: usize, pos: usize) -> i32 {
    let (i, j) = (pos / 4, pos % 4);
    if i % 2 == 0 && j % 2 == 0 {
        V4[m][0]
    } else if i % 2 == 1 && j % 2 == 1 {
        V4[m][1]
    } else {
        V4[m][2]
    }
}

/// normAdjust8x8(m, i, j) for raster position `pos`.
pub const fn norm_adjust8(m: usize, pos: usize) -> i32 {
    let (i, j) = (pos / 8, pos % 8);
    let k = if i % 4 == 0 && j % 4 == 0 {
        0
    } else if i % 2 == 1 && j % 2 == 1 {
        1
    } else if i % 4 == 2 && j % 4 == 2 {
        2
    } else if (i % 4 == 0 && j % 2 == 1) || (i % 2 == 1 && j % 4 == 0) {
        3
    } else if (i % 4 == 0 && j % 4 == 2) || (i % 4 == 2 && j % 4 == 0) {
        4
    } else {
        5
    };
    V8[m][k]
}

/// Table 8-16: alpha' indexed by indexA.
#[rustfmt::skip]
pub const ALPHA: [u8; 52] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 4, 5, 6, 7, 8, 9, 10, 12, 13,
    15, 17, 20, 22, 25, 28, 32, 36, 40, 45, 50, 56, 63, 71, 80, 90, 101, 113, 127, 144, 162, 182, 203, 226, 255, 255,
];

/// Table 8-16: beta' indexed by indexB.
#[rustfmt::skip]
pub const BETA: [u8; 52] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4,
    6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13, 14, 14, 15, 15, 16, 16, 17, 17, 18, 18,
];

/// Table 8-17: tC0' indexed by [indexA][bS - 1].
#[rustfmt::skip]
pub const TC0: [[u8; 3]; 52] = [
    [0, 0, 0], [0, 0, 0], [0, 0, 0], [0, 0, 0], [0, 0, 0], [0, 0, 0], [0, 0, 0], [0, 0, 0],
    [0, 0, 0], [0, 0, 0], [0, 0, 0], [0, 0, 0], [0, 0, 0], [0, 0, 0], [0, 0, 0], [0, 0, 0],
    [0, 0, 0], [0, 0, 1], [0, 0, 1], [0, 0, 1], [0, 0, 1], [0, 1, 1], [0, 1, 1], [1, 1, 1],
    [1, 1, 1], [1, 1, 1], [1, 1, 1], [1, 1, 2], [1, 1, 2], [1, 1, 2], [1, 1, 2], [1, 2, 3],
    [1, 2, 3], [2, 2, 3], [2, 2, 4], [2, 3, 4], [2, 3, 4], [3, 3, 5], [3, 4, 6], [3, 4, 6],
    [4, 5, 7], [4, 5, 8], [4, 6, 9], [5, 7, 10], [6, 8, 11], [6, 8, 13], [7, 10, 14], [8, 11, 16],
    [9, 12, 18], [10, 13, 20], [11, 15, 23], [13, 17, 25],
];

/// luma4x4BlkIdx -> (x, y) of the block in 4-sample units (6.4.3).
pub const BLK4_XY: [(usize, usize); 16] = [
    (0, 0), (1, 0), (0, 1), (1, 1), (2, 0), (3, 0), (2, 1), (3, 1),
    (0, 2), (1, 2), (0, 3), (1, 3), (2, 2), (3, 2), (2, 3), (3, 3),
];

/// Raster 4x4 position (y * 4 + x) -> luma4x4BlkIdx.
pub const RASTER_TO_BLK4: [usize; 16] = [0, 1, 4, 5, 2, 3, 6, 7, 8, 9, 12, 13, 10, 11, 14, 15];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zigzag8_is_permutation_and_matches_rule() {
        // Rebuild the scan from its definition (alternating anti-diagonals).
        let mut zz = Vec::new();
        for d in 0..15i32 {
            let mut cells: Vec<(i32, i32)> = (0..8).map(|i| (d - i, i)).filter(|&(r, c)| (0..8).contains(&r) && (0..8).contains(&c)).collect();
            if d % 2 == 1 {
                cells.reverse();
            }
            // even diagonals run up-right (row decreasing)
            for (r, c) in cells {
                zz.push((r * 8 + c) as u8);
            }
        }
        assert_eq!(&zz[..], &ZIGZAG8[..]);
    }

    #[test]
    fn blk4_tables_agree() {
        for (b, &(x, y)) in BLK4_XY.iter().enumerate() {
            assert_eq!(RASTER_TO_BLK4[y * 4 + x], b);
        }
    }

    #[test]
    fn tc0_columns() {
        let b1: Vec<u8> = TC0.iter().map(|t| t[0]).collect();
        assert_eq!(&b1[23..], &[1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 4, 4, 4, 5, 6, 6, 7, 8, 9, 10, 11, 13]);
        let b3: Vec<u8> = TC0.iter().map(|t| t[2]).collect();
        assert_eq!(&b3[17..], &[1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 4, 4, 4, 5, 6, 6, 7, 8, 9, 10, 11, 13, 14, 16, 18, 20, 23, 25]);
    }
}
