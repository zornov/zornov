// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The mouse in a screen recording: a highlight ring that follows it, a
//! ripple on every click, an optional spotlight, and the whole path kept
//! beside the video so the editor can zoom to where the action is.

use crate::frame::Frame;

/// Where the pointer is, as a fraction of the recorded picture (0..1).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CursorSample {
    pub t_us: i64,
    pub x: f32,
    pub y: f32,
    pub down: bool,
}

/// Where the pointer is at a moment.
pub trait CursorSource: Send {
    fn at(&mut self, t_us: i64) -> Option<CursorSample>;
}

/// A pointer that follows a script (tests, and the agent's `test:` screens).
pub struct ScriptedCursor {
    pub points: Vec<CursorSample>,
}

impl CursorSource for ScriptedCursor {
    fn at(&mut self, t: i64) -> Option<CursorSample> {
        let p = &self.points;
        let i = p.partition_point(|s| s.t_us <= t);
        match (i.checked_sub(1).map(|i| p[i]), p.get(i)) {
            (Some(a), Some(b)) => {
                let k = ((t - a.t_us) as f32 / (b.t_us - a.t_us).max(1) as f32).clamp(0.0, 1.0);
                Some(CursorSample { t_us: t, x: a.x + (b.x - a.x) * k, y: a.y + (b.y - a.y) * k, down: a.down })
            }
            (Some(a), None) => Some(CursorSample { t_us: t, ..a }),
            (None, Some(b)) => Some(CursorSample { t_us: t, ..*b }),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CursorStyle {
    /// Ring radius in pixels at 1080 lines.
    pub radius: f32,
    pub colour: [u8; 4],
    /// How long a click ripple lasts.
    pub ripple_ms: i64,
    /// Dim everything but a disc around the pointer (0 = off, 0.6 = dark).
    pub spotlight: f32,
}

impl Default for CursorStyle {
    fn default() -> Self {
        CursorStyle { radius: 34.0, colour: [255, 214, 10, 200], ripple_ms: 450, spotlight: 0.0 }
    }
}

/// What was recorded of the pointer, sampled as it moved.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct CursorTrack {
    pub samples: Vec<CursorSample>,
    /// Click moments (button went down), microseconds.
    pub clicks: Vec<i64>,
}

impl CursorTrack {
    pub fn push(&mut self, s: CursorSample) {
        if let Some(last) = self.samples.last() {
            if !last.down && s.down {
                self.clicks.push(s.t_us);
            }
        } else if s.down {
            self.clicks.push(s.t_us);
        }
        self.samples.push(s);
    }

    pub fn last_click_before(&self, t: i64) -> Option<i64> {
        self.clicks.iter().rev().find(|&&c| c <= t).copied()
    }
}

/// Draws the highlight for the pointer at `s` onto `f`.
pub fn highlight(f: &mut Frame, style: &CursorStyle, s: CursorSample, since_click_us: Option<i64>) {
    let k = f.h as f32 / 1080.0;
    let (cx, cy) = (s.x * f.w as f32, s.y * f.h as f32);
    let r = style.radius * k;
    if style.spotlight > 0.0 {
        let outer = r * 4.0;
        for y in 0..f.h {
            for x in 0..f.w {
                let d = (((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt() - outer).max(0.0) / (outer * 0.5);
                let dim = style.spotlight * d.min(1.0);
                if dim > 0.0 {
                    f.blend_px(x, y, [0, 0, 0, 255], dim);
                }
            }
        }
    }
    // A soft disc under the pointer, brighter while the button is down.
    let (r_in, alpha) = if s.down { (r * 0.8, 0.6) } else { (r, 0.35) };
    disc(f, cx, cy, r_in, style.colour, alpha, false);
    if let Some(since) = since_click_us.filter(|&t| t >= 0 && t < style.ripple_ms * 1000) {
        let p = since as f32 / (style.ripple_ms * 1000) as f32;
        disc(f, cx, cy, r * (1.0 + 1.6 * p), style.colour, 0.7 * (1.0 - p), true);
    }
}

fn disc(f: &mut Frame, cx: f32, cy: f32, r: f32, c: [u8; 4], alpha: f32, ring: bool) {
    let w = (r * 0.12).max(1.5);
    let (x0, x1) = ((cx - r - 2.0).floor().max(0.0) as u32, (cx + r + 2.0).ceil().min(f.w as f32 - 1.0) as u32);
    let (y0, y1) = ((cy - r - 2.0).floor().max(0.0) as u32, (cy + r + 2.0).ceil().min(f.h as f32 - 1.0) as u32);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
            let cov = if ring { (1.0 - ((d - r).abs() / w)).clamp(0.0, 1.0) } else { (r + 0.5 - d).clamp(0.0, 1.0) * if d > r - w { 1.0 } else { 0.55 } };
            if cov > 0.0 {
                f.blend_px(x, y, c, alpha * cov);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripted_cursor_interpolates_and_clicks_are_found() {
        let mut c = ScriptedCursor { points: vec![CursorSample { t_us: 0, x: 0.0, y: 0.0, down: false }, CursorSample { t_us: 1_000_000, x: 1.0, y: 0.5, down: true }] };
        let m = c.at(500_000).unwrap();
        assert!((m.x - 0.5).abs() < 1e-6 && (m.y - 0.25).abs() < 1e-6);
        let mut t = CursorTrack::default();
        for p in [c.at(0).unwrap(), c.at(1_000_000).unwrap(), c.at(1_100_000).unwrap()] {
            t.push(p);
        }
        assert_eq!(t.clicks, [1_000_000]);
        assert_eq!(t.last_click_before(1_100_000), Some(1_000_000));
    }

    #[test]
    fn highlight_marks_the_pointer_and_ripples_fade() {
        let mut f = Frame::new(640, 360, [20, 20, 20, 255]);
        highlight(&mut f, &CursorStyle::default(), CursorSample { t_us: 0, x: 0.5, y: 0.5, down: false }, None);
        let c = f.px(320, 180);
        assert!(c[0] > c[2] + 25, "{c:?}"); // yellowish over grey
        assert_eq!(f.px(5, 5), [20, 20, 20, 255]);
        let mut g = Frame::new(640, 360, [20, 20, 20, 255]);
        highlight(&mut g, &CursorStyle::default(), CursorSample { t_us: 0, x: 0.5, y: 0.5, down: true }, Some(200_000));
        assert_ne!(f, g);
        let mut late = Frame::new(640, 360, [20, 20, 20, 255]);
        highlight(&mut late, &CursorStyle::default(), CursorSample { t_us: 0, x: 0.5, y: 0.5, down: false }, Some(900_000));
        assert_eq!(late, f);
    }
}
