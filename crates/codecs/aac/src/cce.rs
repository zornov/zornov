// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! coupling_channel_element() (ISO/IEC 14496-3 4.4.2.6, 4.6.8.3): parsing of
//! the coupled ICS and gain element lists, and application to the target
//! channels in the spectral (dependent) or time (independent) domain.

use crate::bits::BitReader;
use crate::error::Result;
use crate::huffman::core_books;
use crate::ics::{ChannelFrame, BAND_STRIDE, MAX_BANDS, ZERO_HCB};

/// Where the coupled signal is added to its targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CouplingPoint {
    BeforeTns,
    BetweenTnsAndImdct,
    AfterImdct,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CouplingTarget {
    pub is_cpe: bool,
    pub tag: u8,
    /// cc_l, cc_r for CPE targets.
    pub cc_l: bool,
    pub cc_r: bool,
}

pub(crate) struct Cce {
    pub present: bool,
    pub point: CouplingPoint,
    pub targets: Vec<CouplingTarget>,
    /// Per gain list: gain per band index (`g * BAND_STRIDE + sfb`), or a
    /// single common gain at index 0 for independently switched coupling.
    pub gains: Vec<Vec<f32>>,
    pub ch: ChannelFrame,
    pub overlap: Vec<f32>,
    pub prev_shape: u8,
    pub time: Vec<f32>,
}

impl Cce {
    pub fn new() -> Cce {
        Cce {
            present: false,
            point: CouplingPoint::BeforeTns,
            targets: Vec::new(),
            gains: Vec::new(),
            ch: ChannelFrame::new(),
            overlap: vec![0.0; 1024],
            prev_shape: 0,
            time: vec![0.0; 1024],
        }
    }

    pub fn parse(&mut self, br: &mut BitReader, sampling_index: usize) -> Result<()> {
        let ind_sw = br.read_bit()?;
        let num_coupled = br.read_usize(3)?;
        self.targets.clear();
        let mut num_gain_lists = 0;
        for _ in 0..=num_coupled {
            num_gain_lists += 1;
            let is_cpe = br.read_bit()?;
            let tag = br.read(4)? as u8;
            let (mut cc_l, mut cc_r) = (true, false);
            if is_cpe {
                cc_l = br.read_bit()?;
                cc_r = br.read_bit()?;
                if cc_l && cc_r {
                    num_gain_lists += 1;
                }
            }
            self.targets.push(CouplingTarget { is_cpe, tag, cc_l, cc_r });
        }
        let domain = br.read_bit()?;
        self.point = if ind_sw {
            CouplingPoint::AfterImdct
        } else if domain {
            CouplingPoint::BetweenTnsAndImdct
        } else {
            CouplingPoint::BeforeTns
        };
        let sign = br.read_bit()?;
        let scale = [1.0905077326652577f32, 1.189207115002721, std::f32::consts::SQRT_2, 2.0][br.read_usize(2)?];
        self.ch.parse(br, sampling_index, false)?;
        let vlc = &core_books().scalefactor;
        self.gains.clear();
        let ics = self.ch.ics.clone();
        for c in 0..num_gain_lists {
            let mut gains = vec![0f32; if ind_sw { 1 } else { MAX_BANDS }];
            let mut cge = true;
            let mut gain = 0i32;
            let mut cache = 1.0f32;
            if c > 0 {
                cge = if ind_sw { true } else { br.read_bit()? };
                if cge {
                    gain = vlc.decode(br)? as i32 - 60;
                    cache = scale.powi(-gain);
                }
            }
            if ind_sw {
                gains[0] = cache;
            } else {
                for g in 0..ics.num_window_groups {
                    for sfb in 0..ics.max_sfb {
                        let idx = g * BAND_STRIDE + sfb;
                        if self.ch.band_type[idx] == ZERO_HCB {
                            continue;
                        }
                        if !cge {
                            let t = vlc.decode(br)? as i32 - 60;
                            if t != 0 {
                                gain += t;
                                let mut v = gain;
                                let mut s = 1.0;
                                if sign {
                                    if v & 1 != 0 {
                                        s = -1.0;
                                    }
                                    v >>= 1;
                                }
                                cache = scale.powi(-v) * s;
                            }
                        }
                        gains[idx] = cache;
                    }
                }
            }
            self.gains.push(gains);
        }
        self.present = true;
        Ok(())
    }

    /// Gain-list indices that apply to channel `ch` (0/1) of the target
    /// element (`is_cpe`, `tag`).
    pub fn lists_for(&self, is_cpe: bool, tag: u8, ch: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut index = 0;
        for t in &self.targets {
            let lists = if t.is_cpe && t.cc_l && t.cc_r { 2 } else { 1 };
            if t.is_cpe == is_cpe && t.tag == tag {
                if !t.is_cpe {
                    if ch == 0 {
                        out.push(index);
                    }
                } else {
                    match (t.cc_l, t.cc_r) {
                        // Neither flag: both channels with the one gain list.
                        (false, false) => out.push(index),
                        (true, false) if ch == 0 => out.push(index),
                        (false, true) if ch == 1 => out.push(index),
                        (true, true) => out.push(index + ch),
                        _ => {}
                    }
                }
            }
            index += lists;
        }
        out
    }

    /// Add the coupled spectrum with gain list `list` to `target`.
    pub fn apply_spectral(&self, list: usize, target: &mut [f32]) {
        let ics = &self.ch.ics;
        let gains = &self.gains[list];
        let mut win = 0;
        for g in 0..ics.num_window_groups {
            for sfb in 0..ics.max_sfb {
                let idx = g * BAND_STRIDE + sfb;
                if self.ch.band_type[idx] == ZERO_HCB {
                    continue;
                }
                let gain = gains[idx];
                let lo = ics.swb_offset[sfb] as usize;
                let hi = ics.swb_offset[sfb + 1] as usize;
                for w in 0..ics.group_len[g] {
                    let base = (win + w) * 128;
                    for k in base + lo..base + hi {
                        target[k] += gain * self.ch.spec[k];
                    }
                }
            }
            win += ics.group_len[g];
        }
    }

    /// Add the coupled time signal with gain list `list` to `target`.
    pub fn apply_time(&self, list: usize, target: &mut [f32]) {
        let gain = self.gains[list][0];
        for (t, s) in target.iter_mut().zip(&self.time) {
            *t += gain * s;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ics::IcsInfo;

    #[test]
    fn gain_list_mapping() {
        let mut c = Cce::new();
        c.targets = vec![
            CouplingTarget { is_cpe: false, tag: 0, cc_l: true, cc_r: false },
            CouplingTarget { is_cpe: true, tag: 1, cc_l: true, cc_r: true },
            CouplingTarget { is_cpe: true, tag: 2, cc_l: false, cc_r: false },
            CouplingTarget { is_cpe: true, tag: 3, cc_l: false, cc_r: true },
        ];
        assert_eq!(c.lists_for(false, 0, 0), vec![0]);
        assert_eq!(c.lists_for(true, 1, 0), vec![1]);
        assert_eq!(c.lists_for(true, 1, 1), vec![2]);
        assert_eq!(c.lists_for(true, 2, 0), vec![3]);
        assert_eq!(c.lists_for(true, 2, 1), vec![3]);
        assert!(c.lists_for(true, 3, 0).is_empty());
        assert_eq!(c.lists_for(true, 3, 1), vec![4]);
        assert!(c.lists_for(false, 5, 0).is_empty());
    }

    #[test]
    fn spectral_and_time_coupling() {
        let mut c = Cce::new();
        c.ch.ics = IcsInfo { max_sfb: 2, num_swb: 49, swb_offset: crate::tables::BANDS[4].long, ..IcsInfo::default() };
        c.ch.band_type[0] = 1;
        c.ch.band_type[1] = 0;
        c.ch.spec.iter_mut().for_each(|v| *v = 1.0);
        let mut g = vec![0f32; MAX_BANDS];
        g[0] = 0.5;
        c.gains = vec![g, vec![2.0]];
        let mut target = vec![0f32; 1024];
        c.apply_spectral(0, &mut target);
        assert_eq!(&target[..4], &[0.5; 4]);
        assert_eq!(target[4], 0.0); // band 1 is ZERO_HCB
        c.time.iter_mut().for_each(|v| *v = 3.0);
        let mut t = vec![1f32; 1024];
        c.apply_time(1, &mut t);
        assert!(t.iter().all(|&v| v == 7.0));
    }
}
