// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! SBR frequency band tables (ISO/IEC 14496-3 4.6.18.3): master table,
//! high/low resolution tables, noise floor bands, patches (4.6.18.6.3) and
//! limiter bands (4.6.18.7.4).

use crate::error::{invalid, Result};
use crate::sbr::SbrHeader;

/// Offsets of the start band per bs_start_freq (Table 4.82 / 4.A.? via
/// startMin), selected by the SBR sampling rate.
const START_OFFSETS: [[i8; 16]; 6] = [
    [-8, -7, -6, -5, -4, -3, -2, -1, 0, 1, 2, 3, 4, 5, 6, 7],    // 16 kHz
    [-5, -4, -3, -2, -1, 0, 1, 2, 3, 4, 5, 6, 7, 9, 11, 13],     // 22.05 kHz
    [-5, -3, -2, -1, 0, 1, 2, 3, 4, 5, 6, 7, 9, 11, 13, 16],     // 24 kHz
    [-6, -4, -2, -1, 0, 1, 2, 3, 4, 5, 6, 7, 9, 11, 13, 16],     // 32 kHz
    [-4, -2, -1, 0, 1, 2, 3, 4, 5, 6, 7, 9, 11, 13, 16, 20],     // 44.1 / 48 / 64 kHz
    [-2, -1, 0, 1, 2, 3, 4, 5, 6, 7, 9, 11, 13, 16, 20, 24],     // 88.2 / 96 kHz
];

#[derive(Clone, Debug, Default)]
pub(crate) struct FreqTables {
    pub k0: usize,
    /// First SBR band (= f_tablehigh[0]).
    pub kx: usize,
    /// Number of SBR bands.
    pub m: usize,
    pub n_master: usize,
    pub f_master: Vec<usize>,
    /// n[0] low resolution bands, n[1] high resolution bands.
    pub n: [usize; 2],
    pub f_low: Vec<usize>,
    pub f_high: Vec<usize>,
    pub n_q: usize,
    pub f_noise: Vec<usize>,
    pub num_patches: usize,
    pub patch_num_subbands: [usize; 6],
    pub patch_start_subband: [usize; 6],
    pub n_lim: usize,
    pub f_lim: Vec<usize>,
}

/// `bands[k]` widths of `num_bands` logarithmic bands from start to stop.
fn make_bands(start: usize, stop: usize, num_bands: usize) -> Vec<i32> {
    let base = (stop as f32 / start as f32).powf(1.0 / num_bands as f32);
    let mut prod = start as f32;
    let mut previous = start as i32;
    let mut out = Vec::with_capacity(num_bands);
    for _ in 0..num_bands.saturating_sub(1) {
        prod *= base;
        let present = prod.round_ties_even() as i32;
        out.push(present - previous);
        previous = present;
    }
    out.push(stop as i32 - previous);
    out
}

fn lrint(x: f32) -> i32 {
    x.round_ties_even() as i32
}

impl FreqTables {
    pub fn new(h: &SbrHeader, fs: u32) -> Result<FreqTables> {
        let offsets = match fs {
            0..=19999 => &START_OFFSETS[0],
            20000..=23011 => &START_OFFSETS[1],
            23012..=27999 => &START_OFFSETS[2],
            28000..=39999 => &START_OFFSETS[3],
            40000..=75999 => &START_OFFSETS[4],
            _ => &START_OFFSETS[5],
        };
        let temp: u32 = if fs < 32000 {
            3000
        } else if fs < 64000 {
            4000
        } else {
            5000
        };
        let start_min = ((temp << 7) + (fs >> 1)) / fs;
        let stop_min = ((temp << 8) + (fs >> 1)) / fs;
        let k0 = start_min as i32 + offsets[h.start_freq as usize] as i32;
        if k0 <= 0 {
            invalid!("SBR start band");
        }
        let k0 = k0 as usize;
        let mut k2 = match h.stop_freq {
            0..=13 => {
                let mut dk = make_bands(stop_min as usize, 64, 13);
                dk.sort();
                (stop_min as i32 + dk[..h.stop_freq as usize].iter().sum::<i32>()) as usize
            }
            14 => 2 * k0,
            _ => 3 * k0,
        };
        k2 = k2.min(64);
        let max_bands = if fs <= 32000 {
            48
        } else if fs <= 44100 {
            35
        } else {
            32
        };
        if k2 <= k0 || k2 - k0 > max_bands {
            invalid!("SBR stop band out of range");
        }

        let mut f_master: Vec<usize>;
        if h.freq_scale == 0 {
            let dk = h.alter_scale as usize + 1;
            let n_master = ((k2 - k0 + (dk & 2)) >> dk) << 1;
            if n_master == 0 {
                invalid!("SBR master table empty");
            }
            let mut widths = vec![dk as i32; n_master + 1];
            let k2diff = k2 as i32 - k0 as i32 - (n_master * dk) as i32;
            if k2diff < 0 {
                widths[1] -= 1;
                if k2diff < -1 {
                    widths[2] -= 1;
                }
            } else if k2diff > 0 {
                widths[n_master] += 1;
            }
            f_master = vec![k0; n_master + 1];
            for k in 1..=n_master {
                let v = f_master[k - 1] as i32 + widths[k];
                if v <= 0 {
                    invalid!("SBR master table");
                }
                f_master[k] = v as usize;
            }
        } else {
            let half_bands = 7 - h.freq_scale as i32;
            let (two_regions, k1) = if 49 * k2 > 110 * k0 { (true, 2 * k0) } else { (false, k2) };
            let num0 = (lrint(half_bands as f32 * (k1 as f32 / k0 as f32).log2()) * 2) as usize;
            if num0 == 0 {
                invalid!("SBR master table empty");
            }
            let mut vk0 = make_bands(k0, k1, num0);
            vk0.sort();
            let vdk0_max = *vk0.last().unwrap();
            f_master = Vec::with_capacity(64);
            f_master.push(k0);
            for &d in &vk0 {
                if d <= 0 {
                    invalid!("SBR master band width");
                }
                f_master.push(f_master.last().unwrap() + d as usize);
            }
            if two_regions {
                let invwarp = if h.alter_scale != 0 { 1.0 / 1.3 } else { 1.0 };
                let num1 = (lrint(half_bands as f32 * invwarp * (k2 as f32 / k1 as f32).log2()) * 2) as usize;
                if num1 == 0 {
                    invalid!("SBR master table empty");
                }
                let mut vk1 = make_bands(k1, k2, num1);
                let vdk1_min = *vk1.iter().min().unwrap();
                if vdk1_min < vdk0_max {
                    vk1.sort();
                    let change = (vdk0_max - vk1[0]).min((vk1[num1 - 1] - vk1[0]) >> 1);
                    vk1[0] += change;
                    vk1[num1 - 1] -= change;
                }
                vk1.sort();
                let mut acc = k1;
                for &d in &vk1 {
                    if d <= 0 {
                        invalid!("SBR master band width");
                    }
                    acc += d as usize;
                    f_master.push(acc);
                }
            }
        }
        let n_master = f_master.len() - 1;
        if h.xover_band as usize >= n_master {
            invalid!("SBR crossover band beyond master table");
        }

        // Derived tables.
        let xover = h.xover_band as usize;
        let n_high = n_master - xover;
        let n_low = (n_high + 1) >> 1;
        let f_high: Vec<usize> = f_master[xover..].to_vec();
        let kx = f_high[0];
        let m = f_high[n_high] - kx;
        if kx + m > 64 {
            invalid!("SBR stop frequency too high");
        }
        if kx > 32 {
            invalid!("SBR start frequency too high");
        }
        let odd = n_high & 1;
        let mut f_low = vec![f_high[0]];
        for k in 1..=n_low {
            f_low.push(f_high[2 * k - odd]);
        }
        let n_q = (lrint(h.noise_bands as f32 * (k2 as f32 / kx as f32).log2())).max(1) as usize;
        if n_q > 5 {
            invalid!("too many SBR noise bands");
        }
        let mut f_noise = vec![f_low[0]];
        let mut t = 0;
        for k in 1..=n_q {
            t += (n_low - t) / (n_q + 1 - k);
            f_noise.push(f_low[t]);
        }
        let mut ft = FreqTables {
            k0,
            kx,
            m,
            n_master,
            f_master,
            n: [n_low, n_high],
            f_low,
            f_high,
            n_q,
            f_noise,
            ..Default::default()
        };
        ft.make_patches(fs)?;
        ft.make_limiter(h.limiter_bands);
        Ok(ft)
    }

    /// Patch construction (4.6.18.6.3).
    fn make_patches(&mut self, fs: u32) -> Result<()> {
        let mut msb = self.k0;
        let mut usb = self.kx;
        let goal_sb = ((1000u32 << 11) + (fs >> 1)) / fs;
        let goal_sb = goal_sb as usize;
        let mut k = if goal_sb < self.kx + self.m {
            let mut k = 0;
            while self.f_master[k] < goal_sb {
                k += 1;
            }
            k
        } else {
            self.n_master
        };
        let (mut last_k, mut last_msb) = (usize::MAX, usize::MAX);
        self.num_patches = 0;
        loop {
            if k == last_k && msb == last_msb {
                invalid!("SBR patch construction failed");
            }
            last_k = k;
            last_msb = msb;
            let mut sb;
            let mut odd;
            let mut i = k;
            loop {
                sb = self.f_master[i];
                odd = (sb + self.k0) & 1;
                // Continue while sb > k0 - 1 + msb - odd
                if !(sb as isize > self.k0 as isize - 1 + msb as isize - odd as isize) || i == 0 {
                    break;
                }
                i -= 1;
            }
            if self.num_patches > 5 {
                invalid!("too many SBR patches");
            }
            let nsb = sb.saturating_sub(usb);
            let start = self.k0 as isize - odd as isize - nsb as isize;
            if start < 0 {
                invalid!("SBR patch start");
            }
            self.patch_num_subbands[self.num_patches] = nsb;
            self.patch_start_subband[self.num_patches] = start as usize;
            if nsb > 0 {
                usb = sb;
                msb = sb;
                self.num_patches += 1;
            } else {
                msb = self.kx;
            }
            if self.f_master[k] < sb + 3 {
                k = self.n_master;
            }
            if sb == self.kx + self.m {
                break;
            }
        }
        if self.num_patches > 1 && self.patch_num_subbands[self.num_patches - 1] < 3 {
            self.num_patches -= 1;
        }
        if self.num_patches == 0 {
            invalid!("SBR without patches");
        }
        Ok(())
    }

    /// Limiter band table (4.6.18.7.4).
    fn make_limiter(&mut self, limiter_bands: u8) {
        if limiter_bands == 0 {
            self.f_lim = vec![self.f_low[0], self.f_low[self.n[0]]];
            self.n_lim = 1;
            return;
        }
        let warped = [1.327_151_7f32, 1.185_092_8, 1.119_871_6][limiter_bands as usize - 1];
        let mut borders = vec![self.kx];
        for k in 0..self.num_patches {
            borders.push(borders[k] + self.patch_num_subbands[k]);
        }
        let mut lim: Vec<usize> = self.f_low.clone();
        lim.extend_from_slice(&borders[1..self.num_patches]);
        lim.sort();
        let in_borders = |v: usize| borders[..=self.num_patches].contains(&v);
        let mut n_lim = self.n[0] + self.num_patches - 1;
        let mut out = 0usize;
        let mut inp = 1usize;
        while out < n_lim {
            if lim[inp] as f32 >= lim[out] as f32 * warped {
                out += 1;
                lim[out] = lim[inp];
                inp += 1;
            } else if lim[inp] == lim[out] || !in_borders(lim[inp]) {
                inp += 1;
                n_lim -= 1;
            } else if !in_borders(lim[out]) {
                lim[out] = lim[inp];
                inp += 1;
                n_lim -= 1;
            } else {
                out += 1;
                lim[out] = lim[inp];
                inp += 1;
            }
        }
        lim.truncate(n_lim + 1);
        self.f_lim = lim;
        self.n_lim = n_lim;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> SbrHeader {
        SbrHeader {
            amp_res: 1,
            start_freq: 5,
            stop_freq: 9,
            xover_band: 0,
            freq_scale: 2,
            alter_scale: 1,
            noise_bands: 2,
            limiter_bands: 2,
            limiter_gains: 2,
            interpol_freq: true,
            smoothing_mode: true,
        }
    }

    #[test]
    fn start_bands_match_reference_tables() {
        // 3GPP TS 26.410 sbr_start_freq_44 / _48 / _22 / _24 / _32 / _16.
        let want: [(u32, [usize; 16]); 6] = [
            (16000, [16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31]),
            (22050, [12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 26, 28, 30]),
            (24000, [11, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 25, 27, 29, 32]),
            (32000, [10, 12, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 25, 27, 29, 32]),
            (44100, [8, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 21, 23, 25, 28, 32]),
            (48000, [7, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 20, 22, 24, 27, 31]),
        ];
        for (fs, ks) in want {
            for (s, &k) in ks.iter().enumerate() {
                let mut h = header();
                h.start_freq = s as u8;
                h.stop_freq = 14;
                if let Ok(t) = FreqTables::new(&h, fs) {
                    assert_eq!(t.k0, k, "fs {fs} start {s}");
                }
            }
        }
    }

    #[test]
    fn tables_are_consistent() {
        for fs in [32000u32, 44100, 48000] {
            for start in 0..16u8 {
                for stop in 0..16u8 {
                    for scale in 0..4u8 {
                        let mut h = header();
                        h.start_freq = start;
                        h.stop_freq = stop;
                        h.freq_scale = scale;
                        let Ok(t) = FreqTables::new(&h, fs) else { continue };
                        assert!(t.f_master.windows(2).all(|w| w[0] < w[1]));
                        assert_eq!(t.f_high.len(), t.n[1] + 1);
                        assert_eq!(t.f_low.len(), t.n[0] + 1);
                        assert_eq!(*t.f_low.last().unwrap(), *t.f_high.last().unwrap());
                        assert_eq!(t.f_noise[0], t.kx);
                        assert_eq!(*t.f_noise.last().unwrap(), t.kx + t.m);
                        let patched: usize = t.patch_num_subbands[..t.num_patches].iter().sum();
                        assert!(patched <= t.m);
                        for p in 0..t.num_patches {
                            assert!(t.patch_start_subband[p] + t.patch_num_subbands[p] <= t.k0);
                        }
                        assert_eq!(t.f_lim[0], t.kx);
                        assert!(*t.f_lim.last().unwrap() <= t.kx + t.m);
                        assert!(t.f_lim.windows(2).all(|w| w[0] < w[1]));
                    }
                }
            }
        }
    }
}
