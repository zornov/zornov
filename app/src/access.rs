// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Accessibility themes: high contrast and colour-blind-safe palettes.
//!
//! The base themes in `theme.rs` stay as designed. [`adapt`] takes one of
//! them and returns the tokens to use for the person's settings, so every
//! widget that asks `theme::tokens(ctx)` follows without knowing.
//!
//! * **High contrast**: black or white grounds, pure text, strong borders,
//!   a saturated accent; all text pairs reach 7:1 (WCAG AAA).
//! * **Colour vision**: the four clip colours, the warning and danger
//!   colours are replaced by sets that stay apart for red-green (the
//!   Okabe-Ito family) or blue-yellow colour blindness. Tests simulate the
//!   deficiencies and require the colours to stay distinguishable.

use crate::prefs::ColourVision;
use crate::theme::Tokens;
use eframe::egui::Color32;
use std::sync::atomic::{AtomicU8, Ordering};

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// bit 0: high contrast; bits 1..: colour vision.
static FLAGS: AtomicU8 = AtomicU8::new(0);

/// Set the active modes; true when they changed (styles must be rebuilt).
pub fn set(high_contrast: bool, cv: ColourVision) -> bool {
    let v = high_contrast as u8 | ((cv as u8) << 1);
    FLAGS.swap(v, Ordering::Relaxed) != v
}

pub fn high_contrast() -> bool {
    FLAGS.load(Ordering::Relaxed) & 1 == 1
}

pub fn colour_vision() -> ColourVision {
    ColourVision::from_u8(FLAGS.load(Ordering::Relaxed) >> 1)
}

/// `base` adjusted for the given modes.
pub fn adapt(base: Tokens, high_contrast: bool, cv: ColourVision) -> Tokens {
    let mut t = base;
    match (cv, base.dark) {
        (ColourVision::Standard, _) => {}
        (ColourVision::RedGreen, true) => {
            t.video = hex(0x225588);
            t.audio = hex(0x666611);
            t.title = hex(0x5555DD);
            t.fx = hex(0x447766);
            t.warn = hex(0xF0E442);
            t.danger = hex(0xD55E00);
        }
        (ColourVision::RedGreen, false) => {
            t.video = hex(0x225588);
            t.audio = hex(0x666611);
            t.title = hex(0x5555DD);
            t.fx = hex(0x447766);
            t.warn = hex(0x7A6C00);
            t.danger = hex(0xB3265E);
        }
        (ColourVision::BlueYellow, true) => {
            t.video = hex(0x227744);
            t.audio = hex(0x991111);
            t.title = hex(0xBB33BB);
            t.fx = hex(0x8855CC);
            t.warn = hex(0xFFA83D);
            t.danger = hex(0xFF6B5E);
        }
        (ColourVision::BlueYellow, false) => {
            t.video = hex(0x227744);
            t.audio = hex(0x991111);
            t.title = hex(0xBB33BB);
            t.fx = hex(0x8855CC);
            t.warn = hex(0x9A5A00);
            t.danger = hex(0xB3261E);
        }
    }
    if high_contrast {
        if base.dark {
            t.ground = hex(0x000000);
            t.sunken = hex(0x000000);
            t.panel = hex(0x0B0B0B);
            t.raised = hex(0x2B2B2B);
            t.line = hex(0xD0D0D0);
            t.text = hex(0xFFFFFF);
            t.text_2 = hex(0xF0F0F0);
            t.accent = hex(0xFFD60A);
            t.accent_fill = hex(0xFFD60A);
            t.accent_ink = hex(0x000000);
            t.accent_soft = Color32::from_rgba_premultiplied(64, 54, 3, 64);
            t.select = hex(0x6FE0FF);
            if cv == ColourVision::Standard {
                t.warn = hex(0xFFB000);
                t.danger = hex(0xFF7B7B);
            }
        } else {
            t.ground = hex(0xFFFFFF);
            t.sunken = hex(0xE6E6E6);
            t.panel = hex(0xFFFFFF);
            t.raised = hex(0xF0F0F0);
            t.line = hex(0x333333);
            t.text = hex(0x000000);
            t.text_2 = hex(0x1A1A1A);
            t.accent = hex(0x0033CC);
            t.accent_fill = hex(0x0033CC);
            t.accent_ink = hex(0xFFFFFF);
            t.accent_soft = Color32::from_rgba_premultiplied(0, 8, 51, 51);
            t.select = hex(0x0033CC);
            if cv == ColourVision::Standard {
                t.warn = hex(0x6B3F00);
                t.danger = hex(0x9A0000);
            }
        }
        // Clip fills carry white text: dark enough for 7:1 whatever the hue.
        if cv == ColourVision::Standard {
            t.video = hex(0x0A3F8F);
            t.audio = hex(0x0A5A30);
            t.title = hex(0x5E1F7E);
            t.fx = hex(0x6A3A00);
        } else {
            for c in [&mut t.video, &mut t.audio, &mut t.title, &mut t.fx] {
                *c = darken_to_contrast(*c, t.on_clip, 7.0);
            }
        }
    }
    t
}

/// Darken `c` until `on` reads on it at `ratio`.
fn darken_to_contrast(c: Color32, on: Color32, ratio: f32) -> Color32 {
    let mut c = c;
    for _ in 0..40 {
        if crate::theme::contrast(on, c) >= ratio {
            break;
        }
        c = crate::theme::mix(c, Color32::BLACK, 0.06);
    }
    c
}

// --- colour-vision simulation (for tests and the gallery) -------------------

/// How a colour looks to someone with the given deficiency (Machado et al.
/// 2009, full severity), as sRGB.
pub fn simulate(c: Color32, kind: Deficiency) -> Color32 {
    let lin = |u: u8| {
        let s = u as f32 / 255.0;
        if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
    };
    let enc = |l: f32| {
        let l = l.clamp(0.0, 1.0);
        let s = if l <= 0.0031308 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 };
        (s * 255.0).round() as u8
    };
    let m: [[f32; 3]; 3] = match kind {
        Deficiency::Protan => [[0.152286, 1.052583, -0.204868], [0.114503, 0.786281, 0.099216], [-0.003882, -0.048116, 1.051998]],
        Deficiency::Deutan => [[0.367322, 0.860646, -0.227968], [0.280085, 0.672501, 0.047413], [-0.011820, 0.042940, 0.968881]],
        Deficiency::Tritan => [[1.255528, -0.076749, -0.178779], [-0.078411, 0.930809, 0.147602], [0.004733, 0.691367, 0.303900]],
    };
    let (r, g, b) = (lin(c.r()), lin(c.g()), lin(c.b()));
    let row = |i: usize| m[i][0] * r + m[i][1] * g + m[i][2] * b;
    Color32::from_rgb(enc(row(0)), enc(row(1)), enc(row(2)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Deficiency {
    Protan,
    Deutan,
    Tritan,
}

/// CIE76 colour difference.
pub fn delta_e(a: Color32, b: Color32) -> f32 {
    fn lab(c: Color32) -> [f32; 3] {
        let lin = |u: u8| {
            let s = u as f32 / 255.0;
            if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
        };
        let (r, g, b) = (lin(c.r()), lin(c.g()), lin(c.b()));
        let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
        let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
        let f = |t: f32| if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 };
        [116.0 * f(y) - 16.0, 500.0 * (f(x) - f(y)), 200.0 * (f(y) - f(z))]
    }
    let (la, lb) = (lab(a), lab(b));
    ((la[0] - lb[0]).powi(2) + (la[1] - lb[1]).powi(2) + (la[2] - lb[2]).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{contrast, DARK, LIGHT};

    const MODES: [(bool, ColourVision); 6] = [
        (false, ColourVision::Standard),
        (true, ColourVision::Standard),
        (false, ColourVision::RedGreen),
        (false, ColourVision::BlueYellow),
        (true, ColourVision::RedGreen),
        (true, ColourVision::BlueYellow),
    ];

    #[test]
    fn standard_is_the_base_theme() {
        assert_eq!(adapt(DARK, false, ColourVision::Standard), DARK);
        assert_eq!(adapt(LIGHT, false, ColourVision::Standard), LIGHT);
    }

    #[test]
    fn text_passes_in_every_mode() {
        for base in [DARK, LIGHT] {
            for (hc, cv) in MODES {
                let t = adapt(base, hc, cv);
                let need = if hc { 7.0 } else { 4.5 };
                for bg in [t.panel, t.raised, t.ground, t.sunken] {
                    assert!(contrast(t.text, bg) >= need, "text on {bg:?} (hc {hc}, {cv:?}, dark {}): {}", base.dark, contrast(t.text, bg));
                    assert!(contrast(t.text_2, bg) >= need, "text-2 on {bg:?} (hc {hc}, {cv:?}, dark {}): {}", base.dark, contrast(t.text_2, bg));
                }
                assert!(contrast(t.accent_ink, t.accent_fill) >= need, "accent ink (hc {hc}, {cv:?}, dark {})", base.dark);
                for clip in [t.video, t.audio, t.title, t.fx] {
                    assert!(contrast(t.on_clip, clip) >= need, "clip text on {clip:?} (hc {hc}, {cv:?}, dark {}): {}", base.dark, contrast(t.on_clip, clip));
                }
                // Warnings and errors read as text on the panel.
                assert!(contrast(t.warn, t.panel) >= 3.0, "warn (hc {hc}, {cv:?}, dark {})", base.dark);
                assert!(contrast(t.danger, t.panel) >= 3.0, "danger (hc {hc}, {cv:?}, dark {})", base.dark);
                // Borders are visible against the panel.
                assert!(contrast(t.line, t.panel) >= if hc { 4.5 } else { 1.0 }, "line (hc {hc})");
            }
        }
    }

    #[test]
    fn clip_colours_stay_apart_when_colours_are_confused() {
        let cases = [
            (ColourVision::RedGreen, [Deficiency::Protan, Deficiency::Deutan]),
            (ColourVision::BlueYellow, [Deficiency::Tritan, Deficiency::Tritan]),
        ];
        for base in [DARK, LIGHT] {
            for (cv, defs) in cases {
                let t = adapt(base, false, cv);
                let kinds = [("video", t.video), ("audio", t.audio), ("title", t.title), ("fx", t.fx)];
                for d in defs {
                    for i in 0..kinds.len() {
                        for j in i + 1..kinds.len() {
                            let de = delta_e(simulate(kinds[i].1, d), simulate(kinds[j].1, d));
                            assert!(de >= 18.0, "{} and {} too close for {d:?} in {cv:?} (dark {}): ΔE {de:.1}", kinds[i].0, kinds[j].0, base.dark);
                        }
                    }
                    let de = delta_e(simulate(t.warn, d), simulate(t.danger, d));
                    assert!(de >= 18.0, "warn and danger too close for {d:?} in {cv:?} (dark {}): ΔE {de:.1}", base.dark);
                }
            }
        }
    }

    #[test]
    fn the_unsafe_default_would_fail_the_check() {
        // Guards the test itself: the standard video and audio fills
        // (teal and green) do collapse for red-green colour blindness.
        let t = DARK;
        assert!(delta_e(simulate(t.audio, Deficiency::Deutan), simulate(t.video, Deficiency::Deutan)) < 25.0);
    }

    #[test]
    fn flags_round_trip() {
        set(true, ColourVision::BlueYellow);
        assert!(high_contrast());
        assert_eq!(colour_vision(), ColourVision::BlueYellow);
        assert!(!set(true, ColourVision::BlueYellow));
        set(false, ColourVision::Standard);
        assert!(!high_contrast());
    }
}
