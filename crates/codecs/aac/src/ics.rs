// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! individual_channel_stream() parsing (ISO/IEC 14496-3 4.4.2.7): ics_info,
//! section data, scale factor data, pulse data, TNS data and spectral data.

use crate::bits::BitReader;
use crate::error::{invalid, Error, Result};
use crate::huffman::{core_books, SPEC_BOOKS};
use crate::tables::BANDS;
use crate::tns::TnsData;

pub(crate) const ONLY_LONG_SEQUENCE: u8 = 0;
pub(crate) const LONG_START_SEQUENCE: u8 = 1;
pub(crate) const EIGHT_SHORT_SEQUENCE: u8 = 2;
pub(crate) const LONG_STOP_SEQUENCE: u8 = 3;

pub(crate) const ZERO_HCB: u8 = 0;
pub(crate) const ESC_HCB: u8 = 11;
pub(crate) const NOISE_HCB: u8 = 13;
pub(crate) const INTENSITY_HCB2: u8 = 14;
pub(crate) const INTENSITY_HCB: u8 = 15;

/// Stride of the per-group band arrays (`g * BAND_STRIDE + sfb`).
pub(crate) const BAND_STRIDE: usize = 64;
pub(crate) const MAX_BANDS: usize = 8 * BAND_STRIDE;

static EMPTY: [u16; 1] = [0];

/// ics_info() plus the derived window grouping and band tables.
#[derive(Clone, Debug)]
pub(crate) struct IcsInfo {
    pub window_sequence: u8,
    pub window_shape: u8,
    pub max_sfb: usize,
    pub num_windows: usize,
    pub num_window_groups: usize,
    pub group_len: [usize; 8],
    pub swb_offset: &'static [u16],
    pub num_swb: usize,
    pub tns_max_bands: usize,
}

impl Default for IcsInfo {
    fn default() -> Self {
        IcsInfo {
            window_sequence: ONLY_LONG_SEQUENCE,
            window_shape: 0,
            max_sfb: 0,
            num_windows: 1,
            num_window_groups: 1,
            group_len: [1, 0, 0, 0, 0, 0, 0, 0],
            swb_offset: &EMPTY,
            num_swb: 0,
            tns_max_bands: 0,
        }
    }
}

impl IcsInfo {
    pub fn is_short(&self) -> bool {
        self.window_sequence == EIGHT_SHORT_SEQUENCE
    }

    pub fn parse(br: &mut BitReader, sampling_index: usize) -> Result<IcsInfo> {
        // ics_reserved_bit: shall be 0, tolerated like most decoders do.
        br.skip(1)?;
        let window_sequence = br.read(2)? as u8;
        let window_shape = br.read(1)? as u8;
        let bands = &BANDS[sampling_index];
        let mut ics = IcsInfo { window_sequence, window_shape, ..Default::default() };
        if window_sequence == EIGHT_SHORT_SEQUENCE {
            ics.max_sfb = br.read_usize(4)?;
            let grouping = br.read(7)?;
            ics.num_windows = 8;
            ics.group_len = [0; 8];
            ics.num_window_groups = 1;
            ics.group_len[0] = 1;
            for w in 0..7 {
                if grouping & (1 << (6 - w)) != 0 {
                    ics.group_len[ics.num_window_groups - 1] += 1;
                } else {
                    ics.num_window_groups += 1;
                    ics.group_len[ics.num_window_groups - 1] = 1;
                }
            }
            ics.swb_offset = bands.short;
            ics.tns_max_bands = bands.tns_max_short;
        } else {
            ics.max_sfb = br.read_usize(6)?;
            if br.read_bit()? {
                // predictor_data_present: only legal for Main / LTP profiles.
                return Err(Error::Unsupported("prediction data (Main/LTP) in an LC stream"));
            }
            ics.swb_offset = bands.long;
            ics.tns_max_bands = bands.tns_max_long;
        }
        ics.num_swb = ics.swb_offset.len() - 1;
        if ics.max_sfb > ics.num_swb {
            invalid!("max_sfb exceeds the number of scale factor bands");
        }
        Ok(ics)
    }

    /// First window of each group.
    #[cfg(test)]
    pub fn group_start(&self, g: usize) -> usize {
        self.group_len[..g].iter().sum()
    }
}

/// Pulse data (4.6.3.3): absolute coefficient position and amplitude.
#[derive(Clone, Debug, Default)]
pub(crate) struct Pulses {
    pub count: usize,
    pub pos: [usize; 4],
    pub amp: [i32; 4],
}

/// Everything parsed from one individual_channel_stream.
pub(crate) struct ChannelFrame {
    pub ics: IcsInfo,
    pub global_gain: i32,
    pub band_type: [u8; MAX_BANDS],
    /// Scale factor, intensity position or noise energy per band.
    pub sf: [i32; MAX_BANDS],
    pub pulses: Pulses,
    pub tns: TnsData,
    pub quant: Vec<i32>,
    /// Dequantised spectrum (window w at `w * 128` for short blocks).
    pub spec: Vec<f32>,
}

impl ChannelFrame {
    pub fn new() -> ChannelFrame {
        ChannelFrame {
            ics: IcsInfo::default(),
            global_gain: 0,
            band_type: [0; MAX_BANDS],
            sf: [0; MAX_BANDS],
            pulses: Pulses::default(),
            tns: TnsData::default(),
            quant: vec![0; 1024],
            spec: vec![0.0; 1024],
        }
    }

    /// Parse individual_channel_stream(common_window, scale_flag = 0). With
    /// `common_window` the caller has already stored the shared ics_info.
    pub fn parse(&mut self, br: &mut BitReader, sampling_index: usize, common_window: bool) -> Result<()> {
        self.global_gain = br.read(8)? as i32;
        if !common_window {
            self.ics = IcsInfo::parse(br, sampling_index)?;
        }
        self.parse_section_data(br)?;
        self.parse_scalefactors(br)?;
        self.pulses.count = 0;
        if br.read_bit()? {
            self.parse_pulses(br)?;
        }
        self.tns.present = br.read_bit()?;
        if self.tns.present {
            self.tns.parse(br, &self.ics)?;
        }
        if br.read_bit()? {
            return Err(Error::Unsupported("gain control data (SSR)"));
        }
        self.parse_spectral_data(br)
    }

    fn parse_section_data(&mut self, br: &mut BitReader) -> Result<()> {
        let (bits, esc) = if self.ics.is_short() { (3, 7) } else { (5, 31) };
        let max_sfb = self.ics.max_sfb;
        for g in 0..self.ics.num_window_groups {
            let mut k = 0;
            while k < max_sfb {
                let cb = br.read(4)? as u8;
                if cb == 12 {
                    invalid!("reserved code book 12");
                }
                let mut len = 0;
                loop {
                    let inc = br.read_usize(bits)?;
                    len += inc;
                    if inc != esc {
                        break;
                    }
                }
                if k + len > max_sfb {
                    invalid!("section exceeds max_sfb");
                }
                self.band_type[g * BAND_STRIDE + k..g * BAND_STRIDE + k + len].fill(cb);
                k += len;
            }
        }
        Ok(())
    }

    fn parse_scalefactors(&mut self, br: &mut BitReader) -> Result<()> {
        let vlc = &core_books().scalefactor;
        let mut sf = self.global_gain;
        let mut noise = self.global_gain - 90;
        let mut is_pos = 0i32;
        let mut first_noise = true;
        for g in 0..self.ics.num_window_groups {
            for sfb in 0..self.ics.max_sfb {
                let idx = g * BAND_STRIDE + sfb;
                self.sf[idx] = match self.band_type[idx] {
                    ZERO_HCB => 0,
                    INTENSITY_HCB | INTENSITY_HCB2 => {
                        is_pos += vlc.decode(br)? as i32 - 60;
                        is_pos
                    }
                    NOISE_HCB => {
                        if first_noise {
                            first_noise = false;
                            noise += br.read(9)? as i32 - 256;
                        } else {
                            noise += vlc.decode(br)? as i32 - 60;
                        }
                        noise
                    }
                    _ => {
                        sf += vlc.decode(br)? as i32 - 60;
                        if !(0..=255).contains(&sf) {
                            invalid!("scale factor out of range");
                        }
                        sf
                    }
                };
            }
        }
        Ok(())
    }

    fn parse_pulses(&mut self, br: &mut BitReader) -> Result<()> {
        if self.ics.is_short() {
            invalid!("pulse data in a short window");
        }
        let count = br.read_usize(2)? + 1;
        let start = br.read_usize(6)?;
        if start >= self.ics.num_swb {
            invalid!("pulse_start_sfb out of range");
        }
        let mut pos = self.ics.swb_offset[start] as usize;
        for i in 0..count {
            pos += br.read_usize(5)?;
            if pos >= 1024 {
                invalid!("pulse position out of range");
            }
            self.pulses.pos[i] = pos;
            self.pulses.amp[i] = br.read(4)? as i32;
        }
        self.pulses.count = count;
        Ok(())
    }

    fn parse_spectral_data(&mut self, br: &mut BitReader) -> Result<()> {
        let books = core_books();
        self.quant.fill(0);
        let ics = &self.ics;
        let mut win = 0;
        for g in 0..ics.num_window_groups {
            for sfb in 0..ics.max_sfb {
                let cb = self.band_type[g * BAND_STRIDE + sfb];
                if cb == ZERO_HCB || cb > ESC_HCB {
                    continue;
                }
                let book = &SPEC_BOOKS[cb as usize - 1];
                let vlc = &books.spectral[cb as usize - 1];
                let values = &books.values[cb as usize - 1];
                let lo = ics.swb_offset[sfb] as usize;
                let hi = ics.swb_offset[sfb + 1] as usize;
                for w in 0..ics.group_len[g] {
                    let base = (win + w) * 128;
                    let out = &mut self.quant[base + lo..base + hi];
                    for tuple in out.chunks_exact_mut(book.dim) {
                        let v = &values[vlc.decode(br)?];
                        for (d, q) in tuple.iter_mut().enumerate() {
                            *q = v[d] as i32;
                        }
                        if !book.signed {
                            for q in tuple.iter_mut() {
                                if *q != 0 && br.read_bit()? {
                                    *q = -*q;
                                }
                            }
                        }
                        if cb == ESC_HCB {
                            for q in tuple.iter_mut() {
                                if q.abs() == 16 {
                                    let mut n = 0;
                                    while br.read_bit()? {
                                        n += 1;
                                        if n > 8 {
                                            invalid!("escape sequence too long");
                                        }
                                    }
                                    let mag = (1i32 << (n + 4)) + br.read(n + 4)? as i32;
                                    *q = if *q < 0 { -mag } else { mag };
                                }
                            }
                        }
                    }
                }
            }
            win += ics.group_len[g];
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grouping_bits() {
        // EIGHT_SHORT, shape 0, max_sfb 3, grouping 0b1011010 -> groups 1,3,1,... computed
        let mut bits: u32 = 0;
        for (v, w) in [(0u32, 1), (2, 2), (0, 1), (3, 4), (0b0110110, 7)] {
            bits = (bits << w) | v;
        }
        bits <<= 32 - 15;
        let b = bits.to_be_bytes();
        let ics = IcsInfo::parse(&mut BitReader::new(&b), 4).unwrap();
        assert_eq!(ics.num_window_groups, 4);
        assert_eq!(&ics.group_len[..4], &[1, 3, 3, 1]);
        assert_eq!(ics.group_start(2), 4);
        assert_eq!(ics.num_swb, 14);
    }

    #[test]
    fn rejects_bad_max_sfb() {
        // long window, max_sfb 63 at 44.1 kHz (49 bands)
        let b = [0b0000_1111, 0b1100_0000];
        assert!(IcsInfo::parse(&mut BitReader::new(&b), 4).is_err());
    }
}
