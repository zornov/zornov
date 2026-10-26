// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Frame-level rate control.
//!
//! Each frame type keeps a complexity estimate `C` with
//! `bits ~= C * 2^(-qp / 6)`. A frame's budget is the per-frame bit rate,
//! weighted for I frames across the GOP and corrected by the running error:
//! CBR corrects within about half a second (and pads under-shoots with filler
//! data so every second carries the target), VBR spreads the error over
//! several seconds and moves the quantiser smoothly.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RateMode {
    /// Constant bit rate (CBR): hold the target over short windows.
    Cbr,
    /// Variable bit rate (VBR / ABR): hold the average.
    Vbr,
    /// Fixed quantiser (tests / lossless-ish archival).
    ConstQp(u8),
}

pub struct RateControl {
    mode: RateMode,
    /// Target bits per frame.
    per_frame: f64,
    fps: f64,
    gop: u32,
    /// Complexity estimates: [I, P].
    c: [f64; 2],
    seen: [bool; 2],
    /// Running sum of (bits - per_frame).
    err: f64,
    last_qp: [i32; 2],
    /// Frames coded so far.
    frames: u64,
    pub min_qp: i32,
    pub max_qp: i32,
}

impl RateControl {
    pub fn new(mode: RateMode, kbps: u32, fps: f64, gop: u32, pixels: f64) -> Self {
        let per_frame = kbps.max(1) as f64 * 1000.0 / fps.max(1.0);
        // Rough priors: an I frame at QP 26 ~ 0.6 bit/pixel, a P frame ~ 0.08.
        let k26 = 2f64.powf(26.0 / 6.0);
        let mut rc = RateControl {
            mode,
            per_frame,
            fps: fps.max(1.0),
            gop: gop.max(1),
            c: [0.6 * pixels * k26, 0.08 * pixels * k26],
            seen: [false; 2],
            err: 0.0,
            last_qp: [26, 26],
            frames: 0,
            min_qp: 10,
            max_qp: 51,
        };
        let q = rc.qp_for(0, rc.budget(0));
        rc.last_qp = [q, q + 2];
        rc
    }

    fn ratio(&self) -> f64 {
        (self.c[0] / self.c[1].max(1.0)).clamp(1.5, 25.0)
    }

    /// Bits for a frame of type `t` (0 = I, 1 = P) before error correction.
    fn base(&self, t: usize) -> f64 {
        let r = self.ratio();
        let g = self.gop as f64;
        let unit = g / (r + g - 1.0);
        self.per_frame * if t == 0 { r * unit } else { unit }
    }

    fn budget(&self, t: usize) -> f64 {
        let base = self.base(t);
        let horizon = match self.mode {
            RateMode::Cbr => self.fps * 0.5,
            _ => self.fps * 2.0,
        };
        (base - self.err / horizon).max(base * 0.2)
    }

    fn qp_for(&self, t: usize, bits: f64) -> i32 {
        let q = 6.0 * (self.c[t] / bits.max(1.0)).log2();
        (q.round() as i32).clamp(self.min_qp, self.max_qp)
    }

    /// Quantiser for the next frame.
    pub fn frame_qp(&mut self, idr: bool) -> i32 {
        if let RateMode::ConstQp(q) = self.mode {
            return (q as i32).clamp(0, 51);
        }
        let t = if idr { 0 } else { 1 };
        let mut q = self.qp_for(t, self.budget(t));
        // Move freely during the first second (the priors are rough), then smoothly.
        let startup = (self.frames as f64) < self.fps;
        let step = if self.mode == RateMode::Cbr { 4 } else { 2 };
        if t == 1 && self.seen[1] && !startup {
            q = q.clamp(self.last_qp[1] - step, self.last_qp[1] + step);
        }
        if t == 0 && self.seen[1] {
            // Keep key frames in line with the surrounding P frames.
            q = q.clamp(self.last_qp[1] - 4, self.last_qp[1] + 2);
        }
        q.clamp(self.min_qp, self.max_qp)
    }

    /// Budget of the next frame of this type (for overshoot checks).
    pub fn frame_budget(&self, idr: bool) -> f64 {
        self.budget(if idr { 0 } else { 1 })
    }

    pub fn is_cbr(&self) -> bool {
        self.mode == RateMode::Cbr
    }

    /// Record a coded frame; returns the number of filler bytes to append (CBR).
    pub fn update(&mut self, idr: bool, qp: i32, bits: u64) -> usize {
        let t = if idr { 0 } else { 1 };
        let c = bits.max(64) as f64 * 2f64.powf(qp as f64 / 6.0);
        self.c[t] = if self.seen[t] { 0.5 * self.c[t] + 0.5 * c } else { c };
        if idr && !self.seen[1] {
            // Until a P frame is seen, assume it costs a sixth of the key frame.
            self.c[1] = self.c[0] / 6.0;
        }
        self.seen[t] = true;
        self.frames += 1;
        self.last_qp[t] = qp;
        if matches!(self.mode, RateMode::ConstQp(_)) {
            return 0;
        }
        self.err += bits as f64 - self.per_frame;
        if self.mode == RateMode::Cbr {
            // Keep at most half a second of under-run; pad the rest.
            let floor = -self.per_frame * self.fps * 0.5;
            if self.err < floor {
                let pad = ((floor - self.err) / 8.0).ceil() as usize;
                self.err += pad as f64 * 8.0;
                return pad;
            }
        }
        0
    }
}
