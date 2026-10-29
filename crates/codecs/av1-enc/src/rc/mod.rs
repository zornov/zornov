// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! M8 (phase c): rate control.
//!
//! * [`RateMode::Cqp`]: the quantiser index as given.
//! * [`RateMode::Crf`]`(c)`: constant quality, `c` in 0 (best) to 63 (worst). The quantiser index
//!   is `4 * c` (255 for 63), key frames [`KEY_QP_OFFSET`] finer than the frames that predict from
//!   them, because every later frame inherits their errors.
//! * [`RateMode::Bitrate`]`(kbps)`: one pass, average bitrate.
//!
//! # The bitrate controller
//!
//! The controller works on the quantiser *step* rather than the index: `s = log2(ac_step(q))`.
//! The AV1 step table is linear in the index at the fine end (step = index + 7 up to ~100) and
//! exponential at the coarse end, so "N indices halve the size" holds nowhere across the whole
//! range, while a frame's size follows a power of the step closely: `bits = A * step^-GAMMA`
//! (`ln bits = a - GAMMA * ln 2 * s`). `GAMMA` is near 1 for content of flat areas and hard edges
//! and near 1.4 for textured content; the controller uses one value for both and lets feedback
//! absorb the difference.
//!
//! * **Target.** A virtual buffer ("debt") counts the bits spent beyond the per-frame average.
//!   An inter frame is aimed at the average less the debt spread over [`DEBT_WINDOW_S`] seconds
//!   (the integral action), a key frame at [`KEY_FRAME_RATIO`] times the average (a ceiling).
//! * **Model.** `a` (the complexity) is kept per frame type. Each coded frame implies a value of
//!   `a`; the running value moves [`GAIN`] of the way towards it, but never more than
//!   [`MAX_INNOVATION`] (in `ln` bits) per frame. Inter frames predict from the previous frame,
//!   so a frame coded finer than the one before re-codes what that one left behind and is far
//!   larger than its steady size, and one coded coarser skips nearly everything and is far
//!   smaller: single frames are poor evidence of the complexity, and the clamp keeps one such
//!   frame from throwing the model (and the next quantiser) far off.
//! * **Quantiser.** The step the model says hits the target, reached at no more than
//!   [`MAX_STEP_MOVE`] octaves per frame once the frame type has found its level (four times that
//!   before), for the same reason: a steady quantiser keeps the sizes steady.
//! * **Retry.** A frame that misses its target by far can be coded again, coarser or finer
//!   ([`RateControl::retry_qidx`]); the attempt closest to the target is kept.

pub mod gop;

use studio_av1::spec_tables::AC_QLOOKUP;

use crate::types::{Config, FrameType, RateMode};

/// How much finer (in quantiser index) key frames are coded than inter frames under CRF.
pub const KEY_QP_OFFSET: u8 = 8;
/// A key frame may use this many times the average frame budget.
pub const KEY_FRAME_RATIO: f64 = 4.0;
/// The exponent of the size model: `bits ~ step^-GAMMA`.
const GAMMA: f64 = 1.2;
/// The seconds over which a debt (or a saving) is spread over the coming frames.
const DEBT_WINDOW_S: f64 = 0.5;
/// How far the complexity moves towards what a frame implies.
const GAIN: f64 = 0.3;
/// The most a frame may move the complexity, in `ln` bits.
const MAX_INNOVATION: f64 = 0.5;
/// The most the step (in octaves) of a frame type may move from one frame to the next once it
/// has found its level.
const MAX_STEP_MOVE: f64 = 0.15;
/// A frame above this multiple of its target is coded again at a coarser quantiser, one below
/// this fraction of it at a finer one.
const RETRY_OVER: f64 = 2.5;
const RETRY_UNDER: f64 = 0.4;
/// How often a frame may be coded again.
pub const MAX_RETRIES: u32 = 2;

/// `log2` of the AC quantiser step of an 8-bit quantiser index.
fn log_step(q: u8) -> f64 {
    (AC_QLOOKUP[0][q as usize] as f64).log2()
}

/// The quantiser index (1..=255) whose step is nearest `s` (in `log2`).
fn qidx_for(s: f64) -> u8 {
    // the table rises monotonically: the first index at or above, or the one below it
    let hi = (1..=255u8).find(|&q| log_step(q) >= s).unwrap_or(255);
    if hi > 1 && (s - log_step(hi - 1)) < (log_step(hi) - s) { hi - 1 } else { hi }
}

pub struct RateControl {
    mode: RateMode,
    /// Average bits of a frame at the target rate (`Bitrate` only).
    avg_bits: f64,
    /// Frames of the window the debt is repaid over.
    window: f64,
    /// The complexity `a` of the size model (`ln` bits at step 1), per frame type (key, inter).
    a: [f64; 2],
    /// The step (`log2`) the last frame of each type was coded with.
    last_s: [Option<f64>; 2],
    /// Whether each frame type has come close to its target once.
    settled: [bool; 2],
    /// Bits spent beyond the average so far (negative: bits saved).
    debt: f64,
    frames: u32,
    /// The size the frame being coded is aimed at.
    target: f64,
}

impl RateControl {
    pub fn new(cfg: &Config) -> RateControl {
        let fps = cfg.fps.0 as f64 / cfg.fps.1 as f64;
        let (avg_bits, window) = match cfg.rate {
            RateMode::Bitrate(kbps) => (kbps as f64 * 1000.0 / fps, (fps * DEBT_WINDOW_S).max(6.0)),
            _ => (0.0, 1.0),
        };
        // a first guess from the bits per sample, corrected by the first frames
        let bpp = avg_bits / (cfg.width as f64 * cfg.height as f64);
        let q0 = if avg_bits > 0.0 { (150.0 - 40.0 * (bpp.max(1e-4) / 0.08).log2()).clamp(20.0, 255.0) } else { 100.0 };
        let a_inter = avg_bits.max(1.0).ln() + GAMMA * std::f64::consts::LN_2 * log_step(q0 as u8);
        RateControl {
            mode: cfg.rate,
            avg_bits,
            window,
            // key frames cost about three times what inter frames do at the same step
            a: [a_inter + 3f64.ln(), a_inter],
            last_s: [None, None],
            settled: [false, false],
            debt: 0.0,
            frames: 0,
            target: 0.0,
        }
    }

    /// The `base_q_idx` for the next frame, which is of type `frame_type`.
    pub fn frame_qidx(&mut self, frame_type: FrameType) -> u8 {
        let key = frame_type == FrameType::Key;
        match self.mode {
            RateMode::Cqp(q) => q.max(1),
            RateMode::Crf(c) => {
                let base = if c >= 63 { 255 } else { 4 * c as u32 };
                let q = if key { base.saturating_sub(KEY_QP_OFFSET as u32) } else { base };
                q.clamp(1, 255) as u8
            }
            RateMode::Bitrate(_) => {
                let t = !key as usize;
                self.target = if key {
                    self.avg_bits * KEY_FRAME_RATIO
                } else {
                    (self.avg_bits - self.debt / self.window).clamp(0.35 * self.avg_bits, 2.0 * self.avg_bits)
                };
                let wanted = (self.a[t] - self.target.max(1.0).ln()) / (GAMMA * std::f64::consts::LN_2);
                let s = match self.last_s[t] {
                    Some(prev) => {
                        let mv = if self.settled[t] { MAX_STEP_MOVE } else { 4.0 * MAX_STEP_MOVE };
                        wanted.clamp(prev - mv, prev + mv)
                    }
                    None => wanted,
                };
                qidx_for(s)
            }
        }
    }

    /// If the frame just coded at `qidx` took `bytes` and that is far off its target, the
    /// quantiser to code it again with: coarser after an overshoot, finer after an undershoot of an
    /// inter frame (a frame that left most of its bits unspent is better spent finer than lost;
    /// a key frame's target is only a ceiling).
    pub fn retry_qidx(&self, frame_type: FrameType, qidx: u8, bytes: usize) -> Option<u8> {
        if !matches!(self.mode, RateMode::Bitrate(_)) {
            return None;
        }
        let ratio = (bytes as f64 * 8.0 / self.target).max(1e-3);
        // the step move the model says brings the size to the target
        let mv = ratio.log2() / GAMMA;
        if ratio > RETRY_OVER && qidx < 255 {
            Some(qidx_for(log_step(qidx) + mv).max(qidx + 1))
        } else if ratio < RETRY_UNDER && qidx > 1 && frame_type == FrameType::Inter {
            // finer frames also pay for what the previous frame left: go part of the way
            Some(qidx_for(log_step(qidx) + 0.7 * mv.max(-1.0)).min(qidx - 1).max(1))
        } else {
            None
        }
    }

    /// How far `bytes` is from the frame's target, as `|ln(size / target)|`: smaller is better.
    pub fn miss(&self, bytes: usize) -> f64 {
        (bytes as f64 * 8.0 / self.target.max(1.0)).max(1e-3).ln().abs()
    }

    /// Reports the size of the frame just coded (the attempt kept) and the quantiser it used.
    pub fn update(&mut self, frame_type: FrameType, qidx: u8, bytes: usize) {
        self.frames += 1;
        if !matches!(self.mode, RateMode::Bitrate(_)) {
            return;
        }
        let key = frame_type == FrameType::Key;
        let t = !key as usize;
        let s = log_step(qidx);
        self.last_s[t] = Some(s);
        let bits = (bytes as f64 * 8.0).max(8.0);
        let ratio = bits / self.target.max(1.0);
        self.settled[t] |= (0.67..=1.5).contains(&ratio);
        self.debt += bits - self.avg_bits;
        // the debt is bounded: a stream far off target is not made worse by remembering it forever
        let cap = 4.0 * self.window * self.avg_bits;
        self.debt = self.debt.clamp(-cap, cap);
        let seen = bits.ln() + GAMMA * std::f64::consts::LN_2 * s;
        if key && self.frames == 1 {
            // the first frame sets the level of both types
            self.a = [seen, seen - 3f64.ln()];
        } else {
            self.a[t] += (GAIN * (seen - self.a[t])).clamp(-MAX_INNOVATION, MAX_INNOVATION);
        }
    }

    /// Bits spent beyond the average so far.
    pub fn debt_bits(&self) -> f64 {
        self.debt
    }
}
