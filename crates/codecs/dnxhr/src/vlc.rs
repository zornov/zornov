// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Lookup structures built from the Annex E tables, for both directions.

use crate::tables::*;
use std::sync::OnceLock;

/// Which of the three table sets a compression ID uses.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Set {
    /// Tables E.1 to E.3 (CIDs 1270, 1271).
    E1,
    /// Tables E.4 to E.6 (CIDs 1273, 1274).
    E4,
    /// Tables E.7 to E.9 (CID 1272).
    E7,
}

/// Decoded meaning of an amplitude codeword.
#[derive(Clone, Copy)]
pub struct AmpSym {
    pub len: u8,
    /// 0 for end of block.
    pub amp: u8,
    pub run: bool,
    pub index: bool,
}

pub struct Vlc {
    /// [run flag][index flag][amplitude 1..=64] -> (code, length).
    pub amp_enc: [[[(u16, u8); 65]; 2]; 2],
    pub eob: (u16, u8),
    /// zero run 1..=62 -> (code, length).
    pub run_enc: [(u16, u8); 63],
    /// eta -> (code, length).
    pub dc_enc: Vec<(u16, u8)>,
    amp_dec: Vec<AmpSym>,
    run_dec: Vec<(u8, u8)>,
    dc_dec: Vec<(u8, u8)>,
}

impl Vlc {
    pub fn get(set: Set) -> &'static Vlc {
        static S: [OnceLock<Vlc>; 3] = [OnceLock::new(), OnceLock::new(), OnceLock::new()];
        match set {
            Set::E1 => S[0].get_or_init(|| Vlc::build(&AMP_E1, &RUN_E2, &DC_E3)),
            Set::E4 => S[1].get_or_init(|| Vlc::build(&AMP_E4, &RUN_E5, &DC_E6)),
            Set::E7 => S[2].get_or_init(|| Vlc::build(&AMP_E7, &RUN_E8, &DC_E9)),
        }
    }

    fn build(amp: &[AmpCode; 257], run: &[(u16, u8, u8); 62], dc: &[(u16, u8, u8)]) -> Vlc {
        let mut amp_enc = [[[(0u16, 0u8); 65]; 2]; 2];
        let mut eob = (0, 0);
        let mut amp_dec = vec![AmpSym { len: 0, amp: 0, run: false, index: false }; 65536];
        for &(code, len, a, r, i) in amp {
            if a == 0 {
                eob = (code, len);
            } else {
                amp_enc[r as usize][i as usize][a as usize] = (code, len);
            }
            fill(&mut amp_dec, code, len, AmpSym { len, amp: a, run: r, index: i });
        }
        let mut run_enc = [(0u16, 0u8); 63];
        let mut run_dec = vec![(0u8, 0u8); 65536];
        for &(code, len, n) in run {
            run_enc[n as usize] = (code, len);
            fill(&mut run_dec, code, len, (len, n));
        }
        let mut dc_enc = vec![(0u16, 0u8); dc.len()];
        let mut dc_dec = vec![(0u8, 0u8); 65536];
        for &(code, len, eta) in dc {
            dc_enc[eta as usize] = (code, len);
            fill(&mut dc_dec, code, len, (len, eta));
        }
        Vlc { amp_enc, eob, run_enc, dc_enc, amp_dec, run_dec, dc_dec }
    }

    #[inline]
    pub fn decode_amp(&self, peek16: u32) -> AmpSym {
        self.amp_dec[peek16 as usize]
    }

    /// (length, run), length 0 for an invalid code.
    #[inline]
    pub fn decode_run(&self, peek16: u32) -> (u8, u8) {
        self.run_dec[peek16 as usize]
    }

    /// (length, eta), length 0 for an invalid code.
    #[inline]
    pub fn decode_dc(&self, peek16: u32) -> (u8, u8) {
        self.dc_dec[peek16 as usize]
    }
}

fn fill<T: Copy>(table: &mut [T], code: u16, len: u8, v: T) {
    let shift = 16 - len as u32;
    let start = (code as usize) << shift;
    for e in &mut table[start..start + (1usize << shift)] {
        *e = v;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_symbol_decodes_to_itself() {
        for set in [Set::E1, Set::E4, Set::E7] {
            let v = Vlc::get(set);
            for r in 0..2 {
                for i in 0..2 {
                    for a in 1..=64usize {
                        let (code, len) = v.amp_enc[r][i][a];
                        assert!(len > 0);
                        let s = v.decode_amp((code as u32) << (16 - len as u32));
                        assert_eq!((s.len, s.amp as usize, s.run as usize, s.index as usize), (len, a, r, i));
                    }
                }
            }
            let (code, len) = v.eob;
            assert_eq!(v.decode_amp((code as u32) << (16 - len as u32)).amp, 0);
            for n in 1..=62usize {
                let (code, len) = v.run_enc[n];
                assert_eq!(v.decode_run((code as u32) << (16 - len as u32)), (len, n as u8));
            }
        }
    }
}
