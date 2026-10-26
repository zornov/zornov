// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! CAVLC residual decoding (9.2): coeff_token, level_prefix/suffix, total_zeros, run_before.

use std::sync::OnceLock;

use crate::bitreader::BitReader;
use crate::error::{Error, Result};
use crate::vlc::Vlc;

/// Table 9-5: (TrailingOnes, TotalCoeff, codes for nC in [0,2), [2,4), [4,8), >=8, -1, -2).
#[rustfmt::skip]
static COEFF_TOKEN: [(u8, u8, [&str; 6]); 62] = [
    (0, 0, ["1", "11", "1111", "000011", "01", "1"]),
    (0, 1, ["000101", "001011", "001111", "000000", "000111", "0001111"]),
    (1, 1, ["01", "10", "1110", "000001", "1", "01"]),
    (0, 2, ["00000111", "000111", "001011", "000100", "000100", "0001110"]),
    (1, 2, ["000100", "00111", "01111", "000101", "000110", "0001101"]),
    (2, 2, ["001", "011", "1101", "000110", "001", "001"]),
    (0, 3, ["000000111", "0000111", "001000", "001000", "000011", "000000111"]),
    (1, 3, ["00000110", "001010", "01100", "001001", "0000011", "0001100"]),
    (2, 3, ["0000101", "001001", "01110", "001010", "0000010", "0001011"]),
    (3, 3, ["00011", "0101", "1100", "001011", "000101", "00001"]),
    (0, 4, ["0000000111", "00000111", "0001111", "001100", "000010", "000000110"]),
    (1, 4, ["000000110", "000110", "01010", "001101", "00000011", "000000101"]),
    (2, 4, ["00000101", "000101", "01011", "001110", "00000010", "0001010"]),
    (3, 4, ["000011", "0100", "1011", "001111", "0000000", "000001"]),
    (0, 5, ["00000000111", "00000100", "0001011", "010000", "", "0000000111"]),
    (1, 5, ["0000000110", "0000110", "01000", "010001", "", "0000000110"]),
    (2, 5, ["000000101", "0000101", "01001", "010010", "", "000000100"]),
    (3, 5, ["0000100", "00110", "1010", "010011", "", "0001001"]),
    (0, 6, ["0000000001111", "000000111", "0001001", "010100", "", "00000000111"]),
    (1, 6, ["00000000110", "00000110", "001110", "010101", "", "00000000110"]),
    (2, 6, ["0000000101", "00000101", "001101", "010110", "", "0000000101"]),
    (3, 6, ["00000100", "001000", "1001", "010111", "", "0001000"]),
    (0, 7, ["0000000001011", "00000001111", "0001000", "011000", "", "000000000111"]),
    (1, 7, ["0000000001110", "000000110", "001010", "011001", "", "000000000110"]),
    (2, 7, ["00000000101", "000000101", "001001", "011010", "", "00000000101"]),
    (3, 7, ["000000100", "000100", "1000", "011011", "", "0000000100"]),
    (0, 8, ["0000000001000", "00000001011", "00001111", "011100", "", "0000000000111"]),
    (1, 8, ["0000000001010", "00000001110", "0001110", "011101", "", "000000000101"]),
    (2, 8, ["0000000001101", "00000001101", "0001101", "011110", "", "000000000100"]),
    (3, 8, ["0000000100", "0000100", "01101", "011111", "", "00000000100"]),
    (0, 9, ["00000000001111", "000000001111", "00001011", "100000", "", ""]),
    (1, 9, ["00000000001110", "00000001010", "00001110", "100001", "", ""]),
    (2, 9, ["0000000001001", "00000001001", "0001010", "100010", "", ""]),
    (3, 9, ["00000000100", "000000100", "001100", "100011", "", ""]),
    (0, 10, ["00000000001011", "000000001011", "000001111", "100100", "", ""]),
    (1, 10, ["00000000001010", "000000001110", "00001010", "100101", "", ""]),
    (2, 10, ["00000000001101", "000000001101", "00001101", "100110", "", ""]),
    (3, 10, ["0000000001100", "00000001100", "0001100", "100111", "", ""]),
    (0, 11, ["000000000001111", "000000001000", "000001011", "101000", "", ""]),
    (1, 11, ["000000000001110", "000000001010", "000001110", "101001", "", ""]),
    (2, 11, ["00000000001001", "000000001001", "00001001", "101010", "", ""]),
    (3, 11, ["00000000001100", "00000001000", "00001100", "101011", "", ""]),
    (0, 12, ["000000000001011", "0000000001111", "000001000", "101100", "", ""]),
    (1, 12, ["000000000001010", "0000000001110", "000001010", "101101", "", ""]),
    (2, 12, ["000000000001101", "0000000001101", "000001101", "101110", "", ""]),
    (3, 12, ["00000000001000", "000000001100", "00001000", "101111", "", ""]),
    (0, 13, ["0000000000001111", "0000000001011", "0000001101", "110000", "", ""]),
    (1, 13, ["000000000000001", "0000000001010", "000000111", "110001", "", ""]),
    (2, 13, ["000000000001001", "0000000001001", "000001001", "110010", "", ""]),
    (3, 13, ["000000000001100", "0000000001100", "000001100", "110011", "", ""]),
    (0, 14, ["0000000000001011", "0000000000111", "0000001001", "110100", "", ""]),
    (1, 14, ["0000000000001110", "00000000001011", "0000001100", "110101", "", ""]),
    (2, 14, ["0000000000001101", "0000000000110", "0000001011", "110110", "", ""]),
    (3, 14, ["000000000001000", "0000000001000", "0000001010", "110111", "", ""]),
    (0, 15, ["0000000000000111", "00000000001001", "0000000101", "111000", "", ""]),
    (1, 15, ["0000000000001010", "00000000001000", "0000001000", "111001", "", ""]),
    (2, 15, ["0000000000001001", "00000000001010", "0000000111", "111010", "", ""]),
    (3, 15, ["0000000000001100", "0000000000001", "0000000110", "111011", "", ""]),
    (0, 16, ["0000000000000100", "00000000000111", "0000000001", "111100", "", ""]),
    (1, 16, ["0000000000000110", "00000000000110", "0000000100", "111101", "", ""]),
    (2, 16, ["0000000000000101", "00000000000101", "0000000011", "111110", "", ""]),
    (3, 16, ["0000000000001000", "00000000000100", "0000000010", "111111", "", ""]),
];

/// Tables 9-7, 9-8: total_zeros for 4x4 blocks, [tzVlcIndex - 1][total_zeros].
#[rustfmt::skip]
static TOTAL_ZEROS_4X4: [[&str; 16]; 15] = [
    ["1", "011", "010", "0011", "0010", "00011", "00010", "000011", "000010", "0000011", "0000010", "00000011", "00000010", "000000011", "000000010", "000000001"],
    ["111", "110", "101", "100", "011", "0101", "0100", "0011", "0010", "00011", "00010", "000011", "000010", "000001", "000000", ""],
    ["0101", "111", "110", "101", "0100", "0011", "100", "011", "0010", "00011", "00010", "000001", "00001", "000000", "", ""],
    ["00011", "111", "0101", "0100", "110", "101", "100", "0011", "011", "0010", "00010", "00001", "00000", "", "", ""],
    ["0101", "0100", "0011", "111", "110", "101", "100", "011", "0010", "00001", "0001", "00000", "", "", "", ""],
    ["000001", "00001", "111", "110", "101", "100", "011", "010", "0001", "001", "000000", "", "", "", "", ""],
    ["000001", "00001", "101", "100", "011", "11", "010", "0001", "001", "000000", "", "", "", "", "", ""],
    ["000001", "0001", "00001", "011", "11", "10", "010", "001", "000000", "", "", "", "", "", "", ""],
    ["000001", "000000", "0001", "11", "10", "001", "01", "00001", "", "", "", "", "", "", "", ""],
    ["00001", "00000", "001", "11", "10", "01", "0001", "", "", "", "", "", "", "", "", ""],
    ["0000", "0001", "001", "010", "1", "011", "", "", "", "", "", "", "", "", "", ""],
    ["0000", "0001", "01", "1", "001", "", "", "", "", "", "", "", "", "", "", ""],
    ["000", "001", "1", "01", "", "", "", "", "", "", "", "", "", "", "", ""],
    ["00", "01", "1", "", "", "", "", "", "", "", "", "", "", "", "", ""],
    ["0", "1", "", "", "", "", "", "", "", "", "", "", "", "", "", ""],
];

/// Table 9-9 (a): total_zeros for 4:2:0 chroma DC.
static TOTAL_ZEROS_2X2: [[&str; 4]; 3] = [["1", "01", "001", "000"], ["1", "01", "00", ""], ["1", "0", "", ""]];

/// Table 9-10: run_before, [Min(zerosLeft, 7) - 1][run_before].
#[rustfmt::skip]
static RUN_BEFORE: [[&str; 15]; 7] = [
    ["1", "0", "", "", "", "", "", "", "", "", "", "", "", "", ""],
    ["1", "01", "00", "", "", "", "", "", "", "", "", "", "", "", ""],
    ["11", "10", "01", "00", "", "", "", "", "", "", "", "", "", "", ""],
    ["11", "10", "01", "001", "000", "", "", "", "", "", "", "", "", "", ""],
    ["11", "10", "011", "010", "001", "000", "", "", "", "", "", "", "", "", ""],
    ["11", "000", "001", "011", "010", "101", "100", "", "", "", "", "", "", "", ""],
    ["111", "110", "101", "100", "011", "010", "001", "0001", "00001", "000001", "0000001", "00000001", "000000001", "0000000001", "00000000001"],
];

struct Tables {
    /// [nC class 0..=5]: symbol = TotalCoeff << 2 | TrailingOnes.
    coeff_token: Vec<Vlc>,
    total_zeros_4x4: Vec<Vlc>,
    total_zeros_2x2: Vec<Vlc>,
    run_before: Vec<Vlc>,
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        // Column 5 (nC = -2, 4:2:2 chroma DC) is not needed for 4:2:0.
        let coeff_token = (0..5)
            .map(|c| {
                let codes: Vec<(&str, u16)> =
                    COEFF_TOKEN.iter().map(|&(t1, tc, ref s)| (s[c], ((tc as u16) << 2) | t1 as u16)).collect();
                Vlc::from_strings(&codes, 8)
            })
            .collect();
        let mk = |rows: &[&[&str]]| -> Vec<Vlc> {
            rows.iter()
                .map(|row| {
                    let codes: Vec<(&str, u16)> = row.iter().enumerate().map(|(i, s)| (*s, i as u16)).collect();
                    Vlc::from_strings(&codes, 6)
                })
                .collect()
        };
        let tz4: Vec<&[&str]> = TOTAL_ZEROS_4X4.iter().map(|r| &r[..]).collect();
        let tz2: Vec<&[&str]> = TOTAL_ZEROS_2X2.iter().map(|r| &r[..]).collect();
        let rb: Vec<&[&str]> = RUN_BEFORE.iter().map(|r| &r[..]).collect();
        Tables {
            coeff_token,
            total_zeros_4x4: mk(&tz4),
            total_zeros_2x2: mk(&tz2),
            run_before: mk(&rb),
        }
    })
}

/// Force table construction (so the first slice does not pay for it).
pub fn init_tables() {
    let _ = tables();
}

/// Kind of block for coeff_token table selection.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NcKind {
    /// Regular block with predicted nC.
    Nc(i32),
    /// Chroma DC 4:2:0 (nC = -1).
    ChromaDc420,
}

/// residual_block_cavlc(): decode up to `max_coeff` coefficients.
/// `out[k]` receives the level at scan position k (relative to startIdx).
/// Returns TotalCoeff.
pub fn residual_block(r: &mut BitReader, nc: NcKind, max_coeff: usize, out: &mut [i32]) -> Result<u32> {
    let t = tables();
    let table = match nc {
        NcKind::Nc(n) if n < 2 => &t.coeff_token[0],
        NcKind::Nc(n) if n < 4 => &t.coeff_token[1],
        NcKind::Nc(n) if n < 8 => &t.coeff_token[2],
        NcKind::Nc(_) => &t.coeff_token[3],
        NcKind::ChromaDc420 => &t.coeff_token[4],
    };
    let sym = table.decode(r).ok_or(Error::Bitstream("invalid coeff_token"))?;
    let total_coeff = (sym >> 2) as usize;
    let trailing_ones = (sym & 3) as usize;
    for v in out.iter_mut().take(max_coeff) {
        *v = 0;
    }
    if total_coeff == 0 {
        return Ok(0);
    }
    if total_coeff > max_coeff {
        return Err(Error::Bitstream("TotalCoeff exceeds block size"));
    }
    let mut levels = [0i32; 16];
    let mut suffix_length: u32 = if total_coeff > 10 && trailing_ones < 3 { 1 } else { 0 };
    for i in 0..total_coeff {
        if i < trailing_ones {
            levels[i] = if r.read_bit() { -1 } else { 1 };
            continue;
        }
        // level_prefix: leading zero bits followed by a one.
        let mut level_prefix = 0u32;
        loop {
            let p = r.peek(16);
            if p == 0 {
                level_prefix += 16;
                r.skip(16);
                if level_prefix > 32 || r.overrun() {
                    return Err(Error::Bitstream("invalid level_prefix"));
                }
                continue;
            }
            let lz = p.leading_zeros() - 16;
            level_prefix += lz;
            r.skip(lz + 1);
            break;
        }
        if level_prefix > 31 {
            return Err(Error::Bitstream("level_prefix too large"));
        }
        let mut level_code: i64 = ((level_prefix.min(15) as i64) << suffix_length) as i64;
        if suffix_length > 0 || level_prefix >= 14 {
            let size = if level_prefix == 14 && suffix_length == 0 {
                4
            } else if level_prefix >= 15 {
                level_prefix - 3
            } else {
                suffix_length
            };
            if size > 0 {
                level_code += r.read(size) as i64;
            }
        }
        if level_prefix >= 15 && suffix_length == 0 {
            level_code += 15;
        }
        if level_prefix >= 16 {
            level_code += (1i64 << (level_prefix - 3)) - 4096;
        }
        if i == trailing_ones && trailing_ones < 3 {
            level_code += 2;
        }
        let level = if level_code % 2 == 0 { (level_code + 2) >> 1 } else { (-level_code - 1) >> 1 };
        let level = level.clamp(-(1 << 24), 1 << 24) as i32;
        levels[i] = level;
        if suffix_length == 0 {
            suffix_length = 1;
        }
        if level.unsigned_abs() > (3u32 << (suffix_length - 1)) && suffix_length < 6 {
            suffix_length += 1;
        }
    }
    let total_zeros = if total_coeff < max_coeff {
        let tz = match nc {
            NcKind::ChromaDc420 => &t.total_zeros_2x2[total_coeff - 1],
            NcKind::Nc(_) => &t.total_zeros_4x4[total_coeff - 1],
        };
        tz.decode(r).ok_or(Error::Bitstream("invalid total_zeros"))? as usize
    } else {
        0
    };
    if total_zeros + total_coeff > max_coeff {
        return Err(Error::Bitstream("total_zeros exceeds block size"));
    }
    let mut zeros_left = total_zeros;
    // Place coefficients from the highest frequency down.
    let mut pos = (total_coeff + total_zeros) as isize - 1;
    for i in 0..total_coeff {
        out[pos as usize] = levels[i];
        if i + 1 == total_coeff {
            break;
        }
        let run = if zeros_left > 0 {
            let rb = &t.run_before[zeros_left.min(7) - 1];
            let run = rb.decode(r).ok_or(Error::Bitstream("invalid run_before"))? as usize;
            if run > zeros_left {
                return Err(Error::Bitstream("run_before exceeds zerosLeft"));
            }
            run
        } else {
            0
        };
        zeros_left -= run;
        pos -= 1 + run as isize;
    }
    Ok(total_coeff as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(s: &str) -> Vec<u8> {
        let s: String = s.chars().filter(|c| *c == '0' || *c == '1').collect();
        let mut out = vec![0u8; s.len().div_ceil(8) + 4];
        for (i, c) in s.chars().enumerate() {
            if c == '1' {
                out[i / 8] |= 0x80 >> (i % 8);
            }
        }
        out
    }

    #[test]
    fn spec_example_block() {
        // Encoding of the scan [0,3,-1,0,0,-1,1,0,1,0,...] with nC = 0:
        // nonzero from the highest frequency: 1 (pos 8), 1 (pos 6), -1 (pos 5), -1 (pos 2), 3 (pos 1)
        // TotalCoeff=5, T1=3 -> coeff_token "0000100"; T1 signs "001"
        // level -1: suffixLength 0, levelCode 1 -> "01"; level 3: suffixLength 1, levelCode 4 -> "001"+"0"
        // total_zeros=4 (tzVlcIndex 5) -> "110"
        // run_before: pos8 run 1 (zerosLeft 4) "10"; pos6 run 0 (zl 3) "11"; pos5 run 2 (zl 3) "01";
        // pos2 run 0 (zl 1) "1"; pos1 takes the remaining zero.
        let data = bits("0000100 001 01 0010 110 10 11 01 1");
        let mut r = BitReader::new(&data);
        let mut out = [0i32; 16];
        let tc = residual_block(&mut r, NcKind::Nc(0), 16, &mut out).unwrap();
        assert_eq!(tc, 5);
        assert_eq!(&out[..10], &[0, 3, -1, 0, 0, -1, 1, 0, 1, 0]);
        assert_eq!(r.pos(), 26);
    }

    #[test]
    fn invalid_codes_error() {
        let data = [0u8; 8];
        let mut r = BitReader::new(&data);
        let mut out = [0i32; 16];
        assert!(residual_block(&mut r, NcKind::Nc(0), 16, &mut out).is_err());
    }

    #[test]
    fn flc_table_nc8() {
        // nC >= 8: 6-bit FLC, TotalCoeff=3, T1=2 -> ((3-1) << 2) | 2 = 001010
        let data = bits("001010 0 1 0001 0 0 1 0 1");
        let mut r = BitReader::new(&data);
        let mut out = [0i32; 16];
        let tc = residual_block(&mut r, NcKind::Nc(9), 16, &mut out).unwrap();
        assert_eq!(tc, 3);
    }
}
