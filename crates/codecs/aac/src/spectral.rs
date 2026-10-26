// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Inverse quantisation and scaling (ISO/IEC 14496-3 4.6.1, 4.6.2) and pulse
//! data application (4.6.3).

use crate::ics::{ChannelFrame, BAND_STRIDE, ESC_HCB, ZERO_HCB};
use std::sync::OnceLock;

/// `|q|^(4/3)` for 0..=8191 (the escape code book limit).
fn pow43_table() -> &'static [f32] {
    static T: OnceLock<Vec<f32>> = OnceLock::new();
    T.get_or_init(|| (0..8192).map(|i| (i as f64).powf(4.0 / 3.0) as f32).collect())
}

/// `2^(0.25 * (sf - 100))` for sf 0..=255.
fn gain_table() -> &'static [f32] {
    static T: OnceLock<Vec<f32>> = OnceLock::new();
    T.get_or_init(|| (0..256).map(|sf| 2f64.powf(0.25 * (sf as f64 - 100.0)) as f32).collect())
}

#[inline]
pub(crate) fn inv_quant(q: i32) -> f32 {
    let a = q.unsigned_abs() as usize;
    let m = if a < 8192 { pow43_table()[a] } else { (a as f64).powf(4.0 / 3.0) as f32 };
    if q < 0 { -m } else { m }
}

/// Apply pulses and dequantise every spectral band of `ch` into `ch.spec`.
/// Noise and intensity bands are left at zero for the PNS / stereo tools.
pub(crate) fn dequantize(ch: &mut ChannelFrame) {
    for p in 0..ch.pulses.count {
        let pos = ch.pulses.pos[p];
        let q = &mut ch.quant[pos];
        *q += if *q > 0 { ch.pulses.amp[p] } else { -ch.pulses.amp[p] };
    }
    ch.spec.fill(0.0);
    let gains = gain_table();
    let ics = &ch.ics;
    let mut win = 0;
    for g in 0..ics.num_window_groups {
        for sfb in 0..ics.max_sfb {
            let idx = g * BAND_STRIDE + sfb;
            let cb = ch.band_type[idx];
            if cb == ZERO_HCB || cb > ESC_HCB {
                continue;
            }
            let gain = gains[ch.sf[idx] as usize];
            let lo = ics.swb_offset[sfb] as usize;
            let hi = ics.swb_offset[sfb + 1] as usize;
            for w in 0..ics.group_len[g] {
                let base = (win + w) * 128;
                for k in base + lo..base + hi {
                    ch.spec[k] = inv_quant(ch.quant[k]) * gain;
                }
            }
        }
        win += ics.group_len[g];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_quantisation() {
        assert_eq!(inv_quant(0), 0.0);
        assert_eq!(inv_quant(1), 1.0);
        assert!((inv_quant(-8) + 16.0).abs() < 1e-5);
        assert!((inv_quant(8191) - 8191f32.powf(4.0 / 3.0)).abs() / inv_quant(8191) < 1e-6);
        assert!((inv_quant(8200) - 8200f32.powf(4.0 / 3.0)).abs() / inv_quant(8200) < 1e-6);
        assert_eq!(gain_table()[100], 1.0);
        assert!((gain_table()[104] - 2.0).abs() < 1e-6);
    }
}

#[cfg(test)]
mod pulse_tests {
    use super::*;
    use crate::ics::{ChannelFrame, IcsInfo};

    #[test]
    fn pulses_add_to_magnitudes() {
        let mut ch = ChannelFrame::new();
        ch.ics = IcsInfo { max_sfb: 49, num_swb: 49, swb_offset: crate::tables::BANDS[4].long, ..IcsInfo::default() };
        for sfb in 0..49 {
            ch.band_type[sfb] = 5;
            ch.sf[sfb] = 100;
        }
        ch.quant[10] = 3;
        ch.quant[11] = -2;
        ch.quant[12] = 0;
        ch.pulses.count = 3;
        ch.pulses.pos[..3].copy_from_slice(&[10, 11, 12]);
        ch.pulses.amp[..3].copy_from_slice(&[4, 5, 6]);
        dequantize(&mut ch);
        assert!((ch.spec[10] - 7f32.powf(4.0 / 3.0)).abs() < 1e-4);
        assert!((ch.spec[11] + 7f32.powf(4.0 / 3.0)).abs() < 1e-4);
        assert!((ch.spec[12] + 6f32.powf(4.0 / 3.0)).abs() < 1e-4);
    }
}
