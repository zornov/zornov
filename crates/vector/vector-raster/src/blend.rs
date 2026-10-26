// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Blend modes on premultiplied colours, after the W3C *Compositing and
//! Blending* formulas (source over, with the mode's mix where both are
//! present). `shaders/fine.wgsl` carries the same formulas.

use crate::scene::BlendMode;

#[inline]
pub(crate) fn over(d: [f32; 4], s: [f32; 4]) -> [f32; 4] {
    let k = 1.0 - s[3];
    [s[0] + d[0] * k, s[1] + d[1] * k, s[2] + d[2] * k, s[3] + d[3] * k]
}

#[inline]
fn sep(mode: BlendMode, cb: f32, cs: f32) -> f32 {
    match mode {
        BlendMode::Multiply => cb * cs,
        BlendMode::Screen => cb + cs - cb * cs,
        BlendMode::Overlay => hard_light(cs, cb),
        BlendMode::Darken => cb.min(cs),
        BlendMode::Lighten => cb.max(cs),
        BlendMode::ColorDodge => {
            if cb <= 0.0 {
                0.0
            } else if cs >= 1.0 {
                1.0
            } else {
                (cb / (1.0 - cs)).min(1.0)
            }
        }
        BlendMode::ColorBurn => {
            if cb >= 1.0 {
                1.0
            } else if cs <= 0.0 {
                0.0
            } else {
                1.0 - ((1.0 - cb) / cs).min(1.0)
            }
        }
        BlendMode::HardLight => hard_light(cb, cs),
        BlendMode::SoftLight => {
            if cs <= 0.5 {
                cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb)
            } else {
                let d = if cb <= 0.25 { ((16.0 * cb - 12.0) * cb + 4.0) * cb } else { cb.sqrt() };
                cb + (2.0 * cs - 1.0) * (d - cb)
            }
        }
        BlendMode::Difference => (cb - cs).abs(),
        BlendMode::Exclusion => cb + cs - 2.0 * cb * cs,
        _ => cs,
    }
}

#[inline]
fn hard_light(cb: f32, cs: f32) -> f32 {
    if cs <= 0.5 { cb * 2.0 * cs } else { let s = 2.0 * cs - 1.0; cb + s - cb * s }
}

fn lum(c: [f32; 3]) -> f32 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

fn clip_color(c: [f32; 3]) -> [f32; 3] {
    let l = lum(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    let mut c = c;
    if n < 0.0 {
        let d = (l - n).max(1e-9);
        c = c.map(|v| l + (v - l) * l / d);
    }
    if x > 1.0 {
        let d = (x - l).max(1e-9);
        c = c.map(|v| l + (v - l) * (1.0 - l) / d);
    }
    c
}

fn set_lum(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - lum(c);
    clip_color(c.map(|v| v + d))
}

fn sat(c: [f32; 3]) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

fn set_sat(c: [f32; 3], s: f32) -> [f32; 3] {
    let mx = c[0].max(c[1]).max(c[2]);
    let mn = c[0].min(c[1]).min(c[2]);
    if mx - mn <= 1e-9 {
        return [0.0; 3];
    }
    c.map(|v| (v - mn) * s / (mx - mn))
}

/// `s` composited onto `d` with `mode`; both premultiplied.
#[inline]
pub(crate) fn blend(d: [f32; 4], s: [f32; 4], mode: BlendMode) -> [f32; 4] {
    match mode {
        BlendMode::Normal => over(d, s),
        BlendMode::Plus => [(d[0] + s[0]).min(1.0), (d[1] + s[1]).min(1.0), (d[2] + s[2]).min(1.0), (d[3] + s[3]).min(1.0)],
        _ => {
            let (sa, da) = (s[3], d[3]);
            if sa <= 0.0 {
                return d;
            }
            if da <= 0.0 {
                return s;
            }
            let cs = [s[0] / sa, s[1] / sa, s[2] / sa];
            let cb = [d[0] / da, d[1] / da, d[2] / da];
            let mixed: [f32; 3] = match mode {
                BlendMode::Hue => set_lum(set_sat(cs, sat(cb)), lum(cb)),
                BlendMode::Saturation => set_lum(set_sat(cb, sat(cs)), lum(cb)),
                BlendMode::Color => set_lum(cs, lum(cb)),
                BlendMode::Luminosity => set_lum(cb, lum(cs)),
                m => [sep(m, cb[0], cs[0]), sep(m, cb[1], cs[1]), sep(m, cb[2], cs[2])],
            };
            let ao = sa + da - sa * da;
            let mut o = [0.0; 4];
            for i in 0..3 {
                o[i] = s[i] * (1.0 - da) + d[i] * (1.0 - sa) + sa * da * mixed[i].clamp(0.0, 1.0);
            }
            o[3] = ao;
            o
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_match_reference_values() {
        let d = [0.25, 0.5, 0.75, 1.0];
        let s = [0.5, 0.5, 0.5, 1.0];
        assert_eq!(blend(d, s, BlendMode::Multiply), [0.125, 0.25, 0.375, 1.0]);
        assert_eq!(blend(d, s, BlendMode::Screen), [0.625, 0.75, 0.875, 1.0]);
        assert_eq!(blend(d, s, BlendMode::Difference), [0.25, 0.0, 0.25, 1.0]);
        // Normal with half alpha.
        let o = blend([0.0, 0.0, 1.0, 1.0], [0.5, 0.0, 0.0, 0.5], BlendMode::Normal);
        assert_eq!(o, [0.5, 0.0, 0.5, 1.0]);
        // Blending onto nothing is the source itself.
        assert_eq!(blend([0.0; 4], s, BlendMode::Multiply), s);
        // Luminosity keeps the backdrop's hue.
        let o = blend([1.0, 0.0, 0.0, 1.0], [0.5, 0.5, 0.5, 1.0], BlendMode::Luminosity);
        assert!(o[0] > o[1] && (lum([o[0], o[1], o[2]]) - 0.5).abs() < 1e-5);
    }
}
