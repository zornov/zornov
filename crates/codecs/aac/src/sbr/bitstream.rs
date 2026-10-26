// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! SBR payload syntax (ISO/IEC 14496-3 4.4.2.8 / 4.5.2.8): sbr_header(),
//! sbr_grid(), sbr_dtdf(), sbr_invf(), sbr_envelope(), sbr_noise() and the
//! sinusoidal coding, plus envelope / noise Huffman books.

use crate::bits::BitReader;
use crate::error::{invalid, Result};
use crate::huffman::Vlc;
use crate::sbr::freq::FreqTables;
use crate::sbr::tables as t;
use crate::sbr::SbrHeader;
use std::sync::OnceLock;

pub(crate) const FIXFIX: u8 = 0;
pub(crate) const FIXVAR: u8 = 1;
pub(crate) const VARFIX: u8 = 2;
pub(crate) const VARVAR: u8 = 3;

/// A Huffman book whose symbols are signed differences.
pub(crate) struct DeltaBook {
    vlc: Vlc,
    min: i32,
}

impl DeltaBook {
    fn new(lens: &[u8], codes: &[u32], min: i32) -> DeltaBook {
        DeltaBook { vlc: Vlc::new(lens, codes, 9), min }
    }
    #[inline]
    pub fn decode(&self, br: &mut BitReader) -> Result<i32> {
        Ok(self.vlc.decode(br)? as i32 + self.min)
    }
}

pub(crate) struct SbrBooks {
    pub env_1_5_t: DeltaBook,
    pub env_1_5_f: DeltaBook,
    pub bal_1_5_t: DeltaBook,
    pub bal_1_5_f: DeltaBook,
    pub env_3_0_t: DeltaBook,
    pub env_3_0_f: DeltaBook,
    pub bal_3_0_t: DeltaBook,
    pub bal_3_0_f: DeltaBook,
    pub noise_t: DeltaBook,
    pub noise_bal_t: DeltaBook,
}

pub(crate) fn books() -> &'static SbrBooks {
    static B: OnceLock<SbrBooks> = OnceLock::new();
    B.get_or_init(|| SbrBooks {
        env_1_5_t: DeltaBook::new(&t::ENV_1_5DB_T_LENS, &t::ENV_1_5DB_T_CODES, t::ENV_1_5DB_T_MIN),
        env_1_5_f: DeltaBook::new(&t::ENV_1_5DB_F_LENS, &t::ENV_1_5DB_F_CODES, t::ENV_1_5DB_F_MIN),
        bal_1_5_t: DeltaBook::new(&t::ENV_BAL_1_5DB_T_LENS, &t::ENV_BAL_1_5DB_T_CODES, t::ENV_BAL_1_5DB_T_MIN),
        bal_1_5_f: DeltaBook::new(&t::ENV_BAL_1_5DB_F_LENS, &t::ENV_BAL_1_5DB_F_CODES, t::ENV_BAL_1_5DB_F_MIN),
        env_3_0_t: DeltaBook::new(&t::ENV_3_0DB_T_LENS, &t::ENV_3_0DB_T_CODES, t::ENV_3_0DB_T_MIN),
        env_3_0_f: DeltaBook::new(&t::ENV_3_0DB_F_LENS, &t::ENV_3_0DB_F_CODES, t::ENV_3_0DB_F_MIN),
        bal_3_0_t: DeltaBook::new(&t::ENV_BAL_3_0DB_T_LENS, &t::ENV_BAL_3_0DB_T_CODES, t::ENV_BAL_3_0DB_T_MIN),
        bal_3_0_f: DeltaBook::new(&t::ENV_BAL_3_0DB_F_LENS, &t::ENV_BAL_3_0DB_F_CODES, t::ENV_BAL_3_0DB_F_MIN),
        noise_t: DeltaBook::new(&t::NOISE_3_0DB_T_LENS, &t::NOISE_3_0DB_T_CODES, t::NOISE_3_0DB_T_MIN),
        noise_bal_t: DeltaBook::new(
            &t::NOISE_BAL_3_0DB_T_LENS,
            &t::NOISE_BAL_3_0DB_T_CODES,
            t::NOISE_BAL_3_0DB_T_MIN,
        ),
    })
}

impl SbrHeader {
    /// sbr_header(); returns whether the frequency tables must be rebuilt.
    pub fn parse(&mut self, br: &mut BitReader) -> Result<bool> {
        let old = self.clone();
        self.amp_res = br.read(1)? as u8;
        self.start_freq = br.read(4)? as u8;
        self.stop_freq = br.read(4)? as u8;
        self.xover_band = br.read(3)? as u8;
        br.skip(2)?; // bs_reserved
        let extra1 = br.read_bit()?;
        let extra2 = br.read_bit()?;
        if extra1 {
            self.freq_scale = br.read(2)? as u8;
            self.alter_scale = br.read(1)? as u8;
            self.noise_bands = br.read(2)? as u8;
        } else {
            self.freq_scale = 2;
            self.alter_scale = 1;
            self.noise_bands = 2;
        }
        if extra2 {
            self.limiter_bands = br.read(2)? as u8;
            self.limiter_gains = br.read(2)? as u8;
            self.interpol_freq = br.read_bit()?;
            self.smoothing_mode = br.read_bit()?;
        } else {
            self.limiter_bands = 2;
            self.limiter_gains = 2;
            self.interpol_freq = true;
            self.smoothing_mode = true;
        }
        Ok(old.start_freq != self.start_freq
            || old.stop_freq != self.stop_freq
            || old.xover_band != self.xover_band
            || old.freq_scale != self.freq_scale
            || old.alter_scale != self.alter_scale
            || old.noise_bands != self.noise_bands
            || old.limiter_bands != self.limiter_bands)
    }
}

/// Bitstream data and per-channel history of one SBR channel.
#[derive(Clone)]
pub(crate) struct SbrChannelData {
    pub frame_class: u8,
    pub num_env: usize,
    /// freq_res[0] is the last envelope's resolution of the previous frame.
    pub freq_res: [u8; 7],
    pub num_noise: usize,
    pub t_env: [usize; 7],
    pub t_q: [usize; 3],
    pub t_env_num_env_old: usize,
    pub amp_res: u8,
    /// e_a[0] = l_APrev, e_a[1] = l_A (-1 when absent).
    pub e_a: [i32; 2],
    pub df_env: [bool; 5],
    pub df_noise: [bool; 2],
    /// invf_mode[0] current, [1] previous frame.
    pub invf_mode: [[u8; 5]; 2],
    /// Row 0 holds the previous frame's last envelope.
    pub env_q: [[i32; 48]; 6],
    pub noise_q: [[i32; 5]; 3],
    pub add_harmonic_flag: bool,
    pub add_harmonic: [bool; 48],
}

impl Default for SbrChannelData {
    fn default() -> Self {
        SbrChannelData {
            frame_class: 0,
            num_env: 0,
            freq_res: [0; 7],
            num_noise: 0,
            t_env: [0; 7],
            t_q: [0; 3],
            t_env_num_env_old: 0,
            amp_res: 0,
            e_a: [-1, -1],
            df_env: [false; 5],
            df_noise: [false; 2],
            invf_mode: [[0; 5]; 2],
            env_q: [[0; 48]; 6],
            noise_q: [[0; 5]; 3],
            add_harmonic_flag: false,
            add_harmonic: [false; 48],
        }
    }
}

const CEIL_LOG2: [u32; 6] = [0, 1, 2, 2, 3, 3];

impl SbrChannelData {
    /// sbr_grid()
    pub fn parse_grid(&mut self, br: &mut BitReader, header_amp_res: u8) -> Result<()> {
        let num_env_old = self.num_env;
        self.freq_res[0] = self.freq_res[self.num_env];
        self.amp_res = header_amp_res;
        self.t_env_num_env_old = self.t_env[num_env_old];
        let mut abs_bord_trail = 16usize;
        let mut pointer = 0usize;
        let class = br.read(2)? as u8;
        let mut t_env = [0usize; 7];
        let num_env;
        match class {
            FIXFIX => {
                num_env = 1usize << br.read(2)?;
                if num_env > 4 {
                    invalid!("SBR FIXFIX with 8 envelopes");
                }
                if num_env == 1 {
                    self.amp_res = 0;
                }
                t_env[num_env] = abs_bord_trail;
                let step = (abs_bord_trail + (num_env >> 1)) / num_env;
                for i in 0..num_env - 1 {
                    t_env[i + 1] = t_env[i] + step;
                }
                let r = br.read(1)? as u8;
                for i in 1..=num_env {
                    self.freq_res[i] = r;
                }
            }
            FIXVAR => {
                abs_bord_trail += br.read_usize(2)?;
                let num_rel = br.read_usize(2)?;
                num_env = num_rel + 1;
                t_env[num_env] = abs_bord_trail;
                for i in 0..num_rel {
                    let d = 2 * br.read_usize(2)? + 2;
                    let hi = t_env[num_env - i];
                    if d > hi {
                        invalid!("SBR envelope border");
                    }
                    t_env[num_env - 1 - i] = hi - d;
                }
                pointer = br.read_usize(CEIL_LOG2[num_env])?;
                for i in 0..num_env {
                    self.freq_res[num_env - i] = br.read(1)? as u8;
                }
            }
            VARFIX => {
                t_env[0] = br.read_usize(2)?;
                let num_rel = br.read_usize(2)?;
                num_env = num_rel + 1;
                t_env[num_env] = abs_bord_trail;
                for i in 0..num_rel {
                    t_env[i + 1] = t_env[i] + 2 * br.read_usize(2)? + 2;
                }
                pointer = br.read_usize(CEIL_LOG2[num_env])?;
                for i in 1..=num_env {
                    self.freq_res[i] = br.read(1)? as u8;
                }
            }
            _ => {
                debug_assert_eq!(class, VARVAR);
                t_env[0] = br.read_usize(2)?;
                abs_bord_trail += br.read_usize(2)?;
                let num_lead = br.read_usize(2)?;
                let num_trail = br.read_usize(2)?;
                num_env = num_lead + num_trail + 1;
                if num_env > 5 {
                    invalid!("SBR VARVAR with more than 5 envelopes");
                }
                t_env[num_env] = abs_bord_trail;
                for i in 0..num_lead {
                    t_env[i + 1] = t_env[i] + 2 * br.read_usize(2)? + 2;
                }
                for i in 0..num_trail {
                    let d = 2 * br.read_usize(2)? + 2;
                    let hi = t_env[num_env - i];
                    if d > hi {
                        invalid!("SBR envelope border");
                    }
                    t_env[num_env - 1 - i] = hi - d;
                }
                pointer = br.read_usize(CEIL_LOG2[num_env])?;
                for i in 1..=num_env {
                    self.freq_res[i] = br.read(1)? as u8;
                }
            }
        }
        if pointer > num_env + 1 {
            invalid!("SBR bs_pointer out of range");
        }
        for i in 1..=num_env {
            if t_env[i - 1] >= t_env[i] {
                invalid!("SBR envelope borders not increasing");
            }
        }
        self.frame_class = class;
        self.num_env = num_env;
        self.t_env = t_env;
        self.num_noise = if num_env > 1 { 2 } else { 1 };
        self.t_q = [t_env[0], t_env[num_env], 0];
        if self.num_noise > 1 {
            let idx = if class == FIXFIX {
                num_env >> 1
            } else if class & 1 == 1 {
                num_env - (pointer as isize - 1).max(1) as usize
            } else if pointer == 0 {
                1
            } else if pointer == 1 {
                num_env - 1
            } else {
                pointer - 1
            };
            self.t_q = [t_env[0], t_env[idx], t_env[num_env]];
        }
        self.e_a[0] = if self.e_a[1] == num_env_old as i32 { 0 } else { -1 };
        self.e_a[1] = -1;
        if class & 1 == 1 && pointer > 0 {
            self.e_a[1] = (num_env + 1 - pointer) as i32;
        } else if class == VARFIX && pointer > 1 {
            self.e_a[1] = pointer as i32 - 1;
        }
        Ok(())
    }

    /// Coupled channel: take the grid of `src`, keeping own history.
    pub fn copy_grid(&mut self, src: &SbrChannelData) {
        self.freq_res[0] = self.freq_res[self.num_env];
        self.t_env_num_env_old = self.t_env[self.num_env];
        self.e_a[0] = if self.e_a[1] == self.num_env as i32 { 0 } else { -1 };
        self.freq_res[1..].copy_from_slice(&src.freq_res[1..]);
        self.t_env = src.t_env;
        self.t_q = src.t_q;
        self.num_env = src.num_env;
        self.amp_res = src.amp_res;
        self.num_noise = src.num_noise;
        self.frame_class = src.frame_class;
        self.e_a[1] = src.e_a[1];
    }

    pub fn parse_dtdf(&mut self, br: &mut BitReader) -> Result<()> {
        for i in 0..self.num_env {
            self.df_env[i] = br.read_bit()?;
        }
        for i in 0..self.num_noise {
            self.df_noise[i] = br.read_bit()?;
        }
        Ok(())
    }

    pub fn parse_invf(&mut self, br: &mut BitReader, n_q: usize) -> Result<()> {
        self.invf_mode[1] = self.invf_mode[0];
        for i in 0..n_q {
            self.invf_mode[0][i] = br.read(2)? as u8;
        }
        Ok(())
    }

    /// sbr_envelope(); `balance` for the second channel of a coupled pair.
    pub fn parse_envelope(&mut self, br: &mut BitReader, ft: &FreqTables, balance: bool) -> Result<()> {
        let b = books();
        let delta = if balance { 2 } else { 1 };
        let (bits, tb, fb) = match (balance, self.amp_res) {
            (true, 1) => (5, &b.bal_3_0_t, &b.bal_3_0_f),
            (true, _) => (6, &b.bal_1_5_t, &b.bal_1_5_f),
            (false, 1) => (6, &b.env_3_0_t, &b.env_3_0_f),
            (false, _) => (7, &b.env_1_5_t, &b.env_1_5_f),
        };
        let odd = ft.n[1] & 1;
        for i in 0..self.num_env {
            let res = self.freq_res[i + 1] as usize;
            let prev_res = self.freq_res[i] as usize;
            let n = ft.n[res];
            if self.df_env[i] {
                for j in 0..n {
                    let k = if res == prev_res {
                        j
                    } else if res == 1 {
                        (j + odd) >> 1
                    } else if j == 0 {
                        0
                    } else {
                        2 * j - odd
                    };
                    let v = self.env_q[i][k] + delta * tb.decode(br)?;
                    if !(0..=127).contains(&v) {
                        invalid!("SBR envelope value out of range");
                    }
                    self.env_q[i + 1][j] = v;
                }
            } else {
                let mut v = delta * br.read(bits)? as i32;
                self.env_q[i + 1][0] = v;
                for j in 1..n {
                    v += delta * fb.decode(br)?;
                    if !(0..=127).contains(&v) {
                        invalid!("SBR envelope value out of range");
                    }
                    self.env_q[i + 1][j] = v;
                }
            }
        }
        self.env_q[0] = self.env_q[self.num_env];
        Ok(())
    }

    /// sbr_noise()
    pub fn parse_noise(&mut self, br: &mut BitReader, ft: &FreqTables, balance: bool) -> Result<()> {
        let b = books();
        let delta = if balance { 2 } else { 1 };
        let (tb, fb) = if balance { (&b.noise_bal_t, &b.bal_3_0_f) } else { (&b.noise_t, &b.env_3_0_f) };
        for i in 0..self.num_noise {
            if self.df_noise[i] {
                for j in 0..ft.n_q {
                    let v = self.noise_q[i][j] + delta * tb.decode(br)?;
                    if !(0..=30).contains(&v) {
                        invalid!("SBR noise floor out of range");
                    }
                    self.noise_q[i + 1][j] = v;
                }
            } else {
                let mut v = delta * br.read(5)? as i32;
                self.noise_q[i + 1][0] = v;
                for j in 1..ft.n_q {
                    v += delta * fb.decode(br)?;
                    if !(0..=30).contains(&v) {
                        invalid!("SBR noise floor out of range");
                    }
                    self.noise_q[i + 1][j] = v;
                }
            }
        }
        self.noise_q[0] = self.noise_q[self.num_noise];
        Ok(())
    }

    pub fn parse_harmonics(&mut self, br: &mut BitReader, ft: &FreqTables) -> Result<()> {
        self.add_harmonic_flag = br.read_bit()?;
        if self.add_harmonic_flag {
            for i in 0..ft.n[1] {
                self.add_harmonic[i] = br.read_bit()?;
            }
        } else {
            self.add_harmonic = [false; 48];
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::huffman::tests::{assert_complete_prefix_code, assert_round_trip};

    #[test]
    fn sbr_books_are_complete() {
        let all: [(&[u8], &[u32], usize); 10] = [
            (&t::ENV_1_5DB_T_LENS, &t::ENV_1_5DB_T_CODES, 121),
            (&t::ENV_1_5DB_F_LENS, &t::ENV_1_5DB_F_CODES, 121),
            (&t::ENV_BAL_1_5DB_T_LENS, &t::ENV_BAL_1_5DB_T_CODES, 49),
            (&t::ENV_BAL_1_5DB_F_LENS, &t::ENV_BAL_1_5DB_F_CODES, 49),
            (&t::ENV_3_0DB_T_LENS, &t::ENV_3_0DB_T_CODES, 63),
            (&t::ENV_3_0DB_F_LENS, &t::ENV_3_0DB_F_CODES, 63),
            (&t::ENV_BAL_3_0DB_T_LENS, &t::ENV_BAL_3_0DB_T_CODES, 25),
            (&t::ENV_BAL_3_0DB_F_LENS, &t::ENV_BAL_3_0DB_F_CODES, 25),
            (&t::NOISE_3_0DB_T_LENS, &t::NOISE_3_0DB_T_CODES, 63),
            (&t::NOISE_BAL_3_0DB_T_LENS, &t::NOISE_BAL_3_0DB_T_CODES, 25),
        ];
        for (i, (l, c, n)) in all.iter().enumerate() {
            assert_eq!(l.len(), *n);
            assert_complete_prefix_code(l, c, &format!("sbr book {i}"));
            assert_round_trip(l, c, 9);
        }
        // Zero difference is the shortest code of each time-direction book.
        assert_eq!(t::ENV_1_5DB_T_LENS[60], 2);
        assert_eq!(t::NOISE_3_0DB_T_LENS[31], 1);
    }
}
