// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pictures and sound as they move through capture and the mixer: RGBA
//! frames with a presentation time, and blocks of mono float audio.

/// One picture, 8-bit RGBA (straight alpha), `w * h * 4` bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
    /// Presentation time in microseconds since the source started.
    pub pts_us: i64,
}

impl Frame {
    pub fn new(w: u32, h: u32, colour: [u8; 4]) -> Frame {
        let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
        for _ in 0..w as usize * h as usize {
            rgba.extend_from_slice(&colour);
        }
        Frame { w, h, rgba, pts_us: 0 }
    }

    pub fn px(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.w as usize + x as usize) * 4;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }

    pub fn set(&mut self, x: u32, y: u32, c: [u8; 4]) {
        if x < self.w && y < self.h {
            let i = (y as usize * self.w as usize + x as usize) * 4;
            self.rgba[i..i + 4].copy_from_slice(&c);
        }
    }

    /// Fills a rectangle (clipped to the frame).
    pub fn fill_rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: [u8; 4]) {
        for yy in y.max(0)..(y + h).min(self.h as i32) {
            for xx in x.max(0)..(x + w).min(self.w as i32) {
                self.set(xx as u32, yy as u32, c);
            }
        }
    }

    /// `over` blended onto this frame at `alpha` (0..1) times its own alpha.
    pub fn blend_px(&mut self, x: u32, y: u32, c: [u8; 4], alpha: f32) {
        if x >= self.w || y >= self.h {
            return;
        }
        let a = (c[3] as f32 / 255.0) * alpha.clamp(0.0, 1.0);
        let i = (y as usize * self.w as usize + x as usize) * 4;
        let da = self.rgba[i + 3] as f32 / 255.0;
        let oa = a + da * (1.0 - a);
        if oa <= 0.0 {
            return;
        }
        for k in 0..3 {
            self.rgba[i + k] = ((c[k] as f32 * a + self.rgba[i + k] as f32 * da * (1.0 - a)) / oa).round() as u8;
        }
        self.rgba[i + 3] = (oa * 255.0).round() as u8;
    }

    /// This frame scaled to `w × h` (bilinear).
    pub fn scaled(&self, w: u32, h: u32) -> Frame {
        if w == self.w && h == self.h {
            return self.clone();
        }
        let mut out = Frame { w, h, rgba: vec![0; w as usize * h as usize * 4], pts_us: self.pts_us };
        let (sw, sh) = (self.w.max(1) as f32, self.h.max(1) as f32);
        for y in 0..h {
            let fy = ((y as f32 + 0.5) * sh / h as f32 - 0.5).clamp(0.0, sh - 1.0);
            let (y0, ty) = (fy.floor() as u32, fy.fract());
            let y1 = (y0 + 1).min(self.h - 1);
            for x in 0..w {
                let fx = ((x as f32 + 0.5) * sw / w as f32 - 0.5).clamp(0.0, sw - 1.0);
                let (x0, tx) = (fx.floor() as u32, fx.fract());
                let x1 = (x0 + 1).min(self.w - 1);
                let (a, b, c, d) = (self.px(x0, y0), self.px(x1, y0), self.px(x0, y1), self.px(x1, y1));
                let mut o = [0u8; 4];
                for k in 0..4 {
                    let top = a[k] as f32 * (1.0 - tx) + b[k] as f32 * tx;
                    let bot = c[k] as f32 * (1.0 - tx) + d[k] as f32 * tx;
                    o[k] = (top * (1.0 - ty) + bot * ty).round() as u8;
                }
                out.set(x, y, o);
            }
        }
        out
    }

    /// `src` drawn into the rectangle (scaled to fit it exactly) at `alpha`.
    pub fn draw(&mut self, src: &Frame, x: i32, y: i32, w: u32, h: u32, alpha: f32) {
        let s = src.scaled(w.max(1), h.max(1));
        for yy in 0..s.h {
            for xx in 0..s.w {
                let (dx, dy) = (x + xx as i32, y + yy as i32);
                if dx >= 0 && dy >= 0 {
                    self.blend_px(dx as u32, dy as u32, s.px(xx, yy), alpha);
                }
            }
        }
    }

    /// Left and right swapped (mirror output for a teleprompter glass).
    pub fn mirrored(&self) -> Frame {
        let mut out = self.clone();
        for y in 0..self.h {
            for x in 0..self.w {
                out.set(x, y, self.px(self.w - 1 - x, y));
            }
        }
        out
    }

    /// A mix of two frames of the same size: `t` 0 is `a`, 1 is `b`.
    pub fn mix(a: &Frame, b: &Frame, t: f32) -> Frame {
        let t = t.clamp(0.0, 1.0);
        let mut out = a.clone();
        let b = if (b.w, b.h) == (a.w, a.h) { b.clone() } else { b.scaled(a.w, a.h) };
        for (o, (&x, &y)) in out.rgba.iter_mut().zip(a.rgba.iter().zip(b.rgba.iter())) {
            *o = (x as f32 * (1.0 - t) + y as f32 * t).round() as u8;
        }
        out
    }
}

/// A block of mono float samples.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioBlock {
    pub rate: u32,
    pub samples: Vec<f32>,
}

/// Peak level of samples in dBFS (−120 for silence).
pub fn peak_db(samples: &[f32]) -> f32 {
    let p = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if p <= 1e-6 { -120.0 } else { 20.0 * p.log10() }
}

/// RMS level of samples in dBFS (−120 for silence).
pub fn rms_db(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return -120.0;
    }
    let m = (samples.iter().map(|s| (*s as f64) * (*s as f64)).sum::<f64>() / samples.len() as f64).sqrt() as f32;
    if m <= 1e-6 { -120.0 } else { 20.0 * m.log10() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaling_and_mirroring_keep_pixels_where_they_belong() {
        let mut f = Frame::new(4, 2, [0, 0, 0, 255]);
        f.set(0, 0, [255, 0, 0, 255]);
        let m = f.mirrored();
        assert_eq!(m.px(3, 0), [255, 0, 0, 255]);
        assert_eq!(m.px(0, 0), [0, 0, 0, 255]);
        let s = f.scaled(8, 4);
        assert_eq!((s.w, s.h), (8, 4));
        assert_eq!(s.px(0, 0)[0], 255);
        assert_eq!(s.px(7, 3), [0, 0, 0, 255]);
    }

    #[test]
    fn mixing_is_halfway_at_half() {
        let a = Frame::new(2, 2, [0, 0, 0, 255]);
        let b = Frame::new(2, 2, [200, 100, 50, 255]);
        assert_eq!(Frame::mix(&a, &b, 0.5).px(1, 1), [100, 50, 25, 255]);
    }

    #[test]
    fn levels() {
        assert!((peak_db(&[0.5, -0.5]) + 6.02).abs() < 0.05);
        assert_eq!(peak_db(&[0.0; 8]), -120.0);
        assert!((rms_db(&[1.0, -1.0, 1.0, -1.0]) - 0.0).abs() < 0.01);
    }
}

impl Frame {
    /// A transparent frame, to draw overlays on.
    pub fn clear(w: u32, h: u32) -> Frame {
        Frame::new(w, h, [0, 0, 0, 0])
    }

    /// `overlay` (straight alpha) laid over this frame, scaled by `alpha`.
    pub fn over(&mut self, overlay: &Frame, alpha: f32) {
        if alpha <= 0.0 {
            return;
        }
        let o = if (overlay.w, overlay.h) == (self.w, self.h) { std::borrow::Cow::Borrowed(overlay) } else { std::borrow::Cow::Owned(overlay.scaled(self.w, self.h)) };
        for y in 0..self.h {
            for x in 0..self.w {
                let p = o.px(x, y);
                if p[3] > 0 {
                    self.blend_px(x, y, p, alpha);
                }
            }
        }
    }
}
