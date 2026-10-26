// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Joint stereo tools of a channel pair (ISO/IEC 14496-3 4.6.8): M/S,
//! intensity stereo and correlated PNS.

use crate::ics::{ChannelFrame, BAND_STRIDE, INTENSITY_HCB, INTENSITY_HCB2, MAX_BANDS, NOISE_HCB};
use crate::pns::scale_band;

/// M/S decision of a channel pair element.
pub(crate) struct MsInfo {
    /// ms_mask_present: 0 none, 1 per band, 2 all bands.
    pub present: u8,
    pub used: [bool; MAX_BANDS],
}

impl MsInfo {
    pub fn new() -> MsInfo {
        MsInfo { present: 0, used: [false; MAX_BANDS] }
    }
    #[inline]
    pub fn used(&self, idx: usize) -> bool {
        match self.present {
            0 => false,
            1 => self.used[idx],
            _ => true,
        }
    }
}

fn is_intensity(cb: u8) -> bool {
    cb == INTENSITY_HCB || cb == INTENSITY_HCB2
}

/// Apply PNS correlation, M/S and intensity stereo to a common-window pair.
pub(crate) fn apply(l: &mut ChannelFrame, r: &mut ChannelFrame, ms: &MsInfo) {
    let ics = l.ics.clone();
    let mut win = 0;
    for g in 0..ics.num_window_groups {
        for sfb in 0..ics.max_sfb {
            let idx = g * BAND_STRIDE + sfb;
            let (lcb, rcb) = (l.band_type[idx], r.band_type[idx]);
            let lo = ics.swb_offset[sfb] as usize;
            let hi = ics.swb_offset[sfb + 1] as usize;
            for w in 0..ics.group_len[g] {
                let base = (win + w) * 128;
                let (ls, rs) = (&mut l.spec[base + lo..base + hi], &mut r.spec[base + lo..base + hi]);
                if lcb == NOISE_HCB && rcb == NOISE_HCB {
                    if ms.used(idx) {
                        // Correlated noise: the right channel reuses the left
                        // channel's noise vector at its own energy.
                        rs.copy_from_slice(ls);
                        scale_band(rs, r.sf[idx]);
                    }
                } else if is_intensity(rcb) {
                    let mut sign = if rcb == INTENSITY_HCB { 1.0 } else { -1.0 };
                    if ms.present == 1 && ms.used[idx] {
                        sign = -sign;
                    }
                    let scale = sign * 0.5f32.powf(0.25 * r.sf[idx] as f32);
                    for (rv, lv) in rs.iter_mut().zip(ls.iter()) {
                        *rv = lv * scale;
                    }
                } else if ms.used(idx) && lcb != NOISE_HCB && rcb != NOISE_HCB && !is_intensity(lcb) {
                    for (a, b) in ls.iter_mut().zip(rs.iter_mut()) {
                        let (m, s) = (*a, *b);
                        *a = m + s;
                        *b = m - s;
                    }
                }
            }
        }
        win += ics.group_len[g];
    }
}
