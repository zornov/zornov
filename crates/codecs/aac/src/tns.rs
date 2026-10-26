// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Temporal noise shaping (ISO/IEC 14496-3 4.6.9): tns_data() parsing,
//! coefficient dequantisation (with coefficient compression), parcor to LPC
//! conversion and the all-pole filtering of the spectrum.

use crate::bits::BitReader;
use crate::error::{invalid, Result};
use crate::ics::IcsInfo;

/// TNS_MAX_ORDER for AAC LC: 12 for long windows, 7 for short windows.
pub(crate) const MAX_ORDER_LONG: usize = 12;
pub(crate) const MAX_ORDER_SHORT: usize = 7;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TnsFilter {
    pub length: usize,
    pub order: usize,
    pub direction: bool,
    /// Dequantised reflection (parcor) coefficients.
    pub parcor: [f32; MAX_ORDER_LONG],
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TnsData {
    pub present: bool,
    pub n_filt: [usize; 8],
    pub filt: [[TnsFilter; 3]; 8],
}

/// Inverse quantisation of one transmitted coefficient (`coef_res_bits` is
/// 3 or 4, `len` the transmitted width after compression).
pub(crate) fn dequant_coef(raw: u32, coef_res_bits: u32, len: u32) -> f32 {
    // Sign extend the `len`-bit two's complement value.
    let v = if raw & (1 << (len - 1)) != 0 { raw as i32 - (1 << len) } else { raw as i32 };
    let half_pi = std::f64::consts::FRAC_PI_2;
    let iqfac = ((1 << (coef_res_bits - 1)) as f64 - 0.5) / half_pi;
    let iqfac_m = ((1 << (coef_res_bits - 1)) as f64 + 0.5) / half_pi;
    (v as f64 / if v >= 0 { iqfac } else { iqfac_m }).sin() as f32
}

impl TnsData {
    pub fn parse(&mut self, br: &mut BitReader, ics: &IcsInfo) -> Result<()> {
        let short = ics.is_short();
        let (nf_bits, len_bits, ord_bits, max_order) =
            if short { (1, 4, 3, MAX_ORDER_SHORT) } else { (2, 6, 5, MAX_ORDER_LONG) };
        for w in 0..ics.num_windows {
            let n = br.read_usize(nf_bits)?;
            self.n_filt[w] = n;
            if n == 0 {
                continue;
            }
            let coef_res_bits = br.read(1)? + 3;
            for f in 0..n {
                let filt = &mut self.filt[w][f];
                filt.length = br.read_usize(len_bits)?;
                filt.order = br.read_usize(ord_bits)?;
                if filt.order > max_order {
                    invalid!("TNS order too high");
                }
                if filt.order > 0 {
                    filt.direction = br.read_bit()?;
                    let compress = br.read(1)?;
                    let len = coef_res_bits - compress;
                    for i in 0..filt.order {
                        let raw = br.read(len)?;
                        filt.parcor[i] = dequant_coef(raw, coef_res_bits, len);
                    }
                }
            }
        }
        Ok(())
    }
}

/// Parcor to direct-form LPC coefficients `a[0..=order]` (a[0] = 1).
pub(crate) fn parcor_to_lpc(parcor: &[f32], lpc: &mut [f32; MAX_ORDER_LONG + 1]) {
    let order = parcor.len();
    let mut b = [0f32; MAX_ORDER_LONG + 1];
    lpc[0] = 1.0;
    for m in 1..=order {
        for i in 1..m {
            b[i] = lpc[i] + parcor[m - 1] * lpc[m - i];
        }
        lpc[1..m].copy_from_slice(&b[1..m]);
        lpc[m] = parcor[m - 1];
    }
}

/// Apply the TNS synthesis (all-pole) filters to a channel's spectrum.
pub(crate) fn apply(tns: &TnsData, ics: &IcsInfo, spec: &mut [f32]) {
    if !tns.present {
        return;
    }
    let mmm = ics.tns_max_bands.min(ics.max_sfb);
    for w in 0..ics.num_windows {
        let win = &mut spec[w * 128..];
        let mut bottom = ics.num_swb;
        for f in 0..tns.n_filt[w] {
            let filt = &tns.filt[w][f];
            let top = bottom;
            bottom = top.saturating_sub(filt.length);
            if filt.order == 0 {
                continue;
            }
            let mut lpc = [0f32; MAX_ORDER_LONG + 1];
            parcor_to_lpc(&filt.parcor[..filt.order], &mut lpc);
            let start = ics.swb_offset[bottom.min(mmm)] as usize;
            let end = ics.swb_offset[top.min(mmm)] as usize;
            if end <= start {
                continue;
            }
            let size = end - start;
            let order = filt.order;
            let mut state = [0f32; MAX_ORDER_LONG];
            // y[n] = x[n] - sum_{i=1..order} a[i] y[n - i], state[0] is y[n-1].
            for j in 0..size {
                let k = if filt.direction { end - 1 - j } else { start + j };
                let mut y = win[k];
                for i in 0..order {
                    y -= lpc[i + 1] * state[i];
                }
                for i in (1..order).rev() {
                    state[i] = state[i - 1];
                }
                state[0] = y;
                win[k] = y;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coefficient_tables() {
        // ISO/IEC 14496-3 / 3GPP tnsCoeff4 and tnsCoeff3 (index + 8 / + 4).
        let c4 = [
            -0.99573418, -0.96182567, -0.8951633, -0.79801726, -0.67369568, -0.52643222, -0.3612417, -0.18374953, 0.0,
            0.2079117, 0.40673664, 0.58778524, 0.74314481, 0.86602539, 0.95105654, 0.99452192,
        ];
        for (i, &want) in c4.iter().enumerate() {
            let raw = (i as i32 - 8) as u32 & 0xf;
            assert!((dequant_coef(raw, 4, 4) - want).abs() < 1e-6);
        }
        let c3 = [-0.98480775, -0.86602539, -0.64278758, -0.34202015, 0.0, 0.43388373, 0.7818315, 0.9749279];
        for (i, &want) in c3.iter().enumerate() {
            let raw = (i as i32 - 4) as u32 & 0x7;
            assert!((dequant_coef(raw, 3, 3) - want).abs() < 1e-6);
        }
        // Compressed 4-bit resolution sent in 3 bits: -4..3 of the 4-bit table.
        assert!((dequant_coef(0b100, 4, 3) - c4[4]).abs() < 1e-6);
        assert!((dequant_coef(0b011, 4, 3) - c4[11]).abs() < 1e-6);
    }

    #[test]
    fn lpc_conversion_order_two() {
        let mut lpc = [0f32; 13];
        parcor_to_lpc(&[0.5, -0.25], &mut lpc);
        // a1 = k1 + k2*k1, a2 = k2
        assert!((lpc[1] - (0.5 + -0.25 * 0.5)).abs() < 1e-7);
        assert!((lpc[2] + 0.25).abs() < 1e-7);
    }

    #[test]
    fn filter_is_all_pole() {
        let ics = IcsInfo {
            max_sfb: 49,
            num_swb: 49,
            tns_max_bands: 42,
            swb_offset: crate::tables::BANDS[4].long,
            ..Default::default()
        };
        let mut tns = TnsData { present: true, ..Default::default() };
        tns.n_filt[0] = 1;
        tns.filt[0][0] = TnsFilter { length: 49, order: 1, direction: false, parcor: [0.5; 12] };
        let mut spec = vec![0f32; 1024];
        spec[0] = 1.0;
        apply(&tns, &ics, &mut spec);
        // y[n] = x[n] - 0.5 y[n-1] -> impulse response (-0.5)^n
        for n in 0..10 {
            assert!((spec[n] - (-0.5f32).powi(n as i32)).abs() < 1e-6);
        }
    }
}
