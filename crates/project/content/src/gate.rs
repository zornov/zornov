// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Quality gate and contact sheets.
//!
//! Rejects stills that are too small (under 300 px on the longest side, 180
//! for text art), cut-outs with hard aliased edges, pictures that are
//! mostly empty, and cut-outs that touch the frame. Vector items pass the
//! pixel checks (they have no pixels yet). The pack tool keeps a contact
//! sheet per set so a person can review the verdicts.
//!
//! Green-screen stills are the one cut-out kind where touching the frame
//! is usually by design (confetti bursts, sparks, falling leaves fill the
//! picture), so for them it is a *warning*: the item is kept, listed in the
//! gate report and marked amber on the contact sheet for a person to
//! check, instead of being rejected outright.

use crate::image::{self, alpha_bbox, blit, checkerboard, draw_label, fill_rect, label_width, resize_to_fit};
use crate::item::Kind;
use serde::{Deserialize, Serialize};
use studio_media::rgba::RgbaImage;

/// Longest side a still must reach.
pub const MIN_PX: u32 = 300;
/// Text art may be smaller.
pub const MIN_PX_TEXT_ART: u32 = 180;

/// Why a still failed the gate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "reason")]
pub enum Reason {
    TooSmall { width: u32, height: u32, min: u32 },
    /// Fewer than a third of the boundary pixels have a partial alpha.
    HardEdge { soft_fraction: f32 },
    /// Opaque content covers under 1 % of the picture, or its bounding box
    /// under 4 %.
    MostlyEmpty { coverage: f32 },
    /// Opaque pixels sit on the outer row or column.
    TouchesFrame { sides: String },
    /// The picture is fully transparent.
    Empty,
}

impl Reason {
    /// Short code for labels on the contact sheet.
    pub fn code(&self) -> &'static str {
        match self {
            Reason::TooSmall { .. } => "SMALL",
            Reason::HardEdge { .. } => "HARD EDGE",
            Reason::MostlyEmpty { .. } => "EMPTY",
            Reason::TouchesFrame { .. } => "TOUCHES",
            Reason::Empty => "BLANK",
        }
    }
}

/// Minimum longest side for a kind.
pub fn min_px(kind: Kind) -> u32 {
    if kind == Kind::TextArt { MIN_PX_TEXT_ART } else { MIN_PX }
}

/// Outcome of the gate: `rejects` keep an item out of every pack,
/// `warnings` keep it in but flag it for review.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Verdict {
    pub rejects: Vec<Reason>,
    pub warnings: Vec<Reason>,
}

impl Verdict {
    pub fn passed(&self) -> bool {
        self.rejects.is_empty()
    }
}

/// Checks a decoded still of `kind`. Empty vector means pass; this is
/// [`verdict`] without the warnings.
pub fn check(kind: Kind, img: &RgbaImage) -> Vec<Reason> {
    verdict(kind, img).rejects
}

/// Checks a decoded still of `kind`, separating rejections from warnings.
pub fn verdict(kind: Kind, img: &RgbaImage) -> Verdict {
    let mut v = Verdict::default();
    let touching_is_warning = kind == Kind::GreenScreen;
    let reasons = reasons(kind, img);
    for r in reasons {
        if touching_is_warning && matches!(r, Reason::TouchesFrame { .. }) {
            v.warnings.push(r);
        } else {
            v.rejects.push(r);
        }
    }
    v
}

fn reasons(kind: Kind, img: &RgbaImage) -> Vec<Reason> {
    let mut out = Vec::new();
    let min = min_px(kind);
    if img.width.max(img.height) < min {
        out.push(Reason::TooSmall { width: img.width, height: img.height, min });
    }
    if !kind.is_cutout() && kind != Kind::Frame {
        return out;
    }
    let bbox = match alpha_bbox(img, 8) {
        Some(b) => b,
        None => {
            out.push(Reason::Empty);
            return out;
        }
    };
    let area = (img.width as f32) * (img.height as f32);
    let opaque = image::coverage(img, 8) as f32;
    let bbox_cov = ((bbox.2 - bbox.0) as f32 * (bbox.3 - bbox.1) as f32) / area;
    if opaque < 0.01 || bbox_cov < 0.04 {
        out.push(Reason::MostlyEmpty { coverage: opaque });
    }
    if kind.is_cutout() {
        let touched = touching_sides(img);
        if !touched.is_empty() {
            out.push(Reason::TouchesFrame { sides: touched });
        }
    }
    if let Some(soft) = soft_edge_fraction(img) {
        if soft < 0.33 {
            out.push(Reason::HardEdge { soft_fraction: soft });
        }
    }
    out
}

/// Sides (`L`, `R`, `T`, `B`) whose outer row/column carries opaque pixels
/// beyond a sliver (0.5 % of that side's length, at least 2 px).
fn touching_sides(img: &RgbaImage) -> String {
    let (w, h) = (img.width as usize, img.height as usize);
    let a = |x: usize, y: usize| img.pixels[(y * w + x) * 4 + 3];
    let limit_h = ((w as f32) * 0.005).ceil().max(2.0) as usize;
    let limit_v = ((h as f32) * 0.005).ceil().max(2.0) as usize;
    let mut s = String::new();
    if (0..h).filter(|&y| a(0, y) > 64).count() > limit_v {
        s.push('L');
    }
    if (0..h).filter(|&y| a(w - 1, y) > 64).count() > limit_v {
        s.push('R');
    }
    if (0..w).filter(|&x| a(x, 0) > 64).count() > limit_h {
        s.push('T');
    }
    if (0..w).filter(|&x| a(x, h - 1) > 64).count() > limit_h {
        s.push('B');
    }
    s
}

/// Share of boundary pixels (visible pixels next to a transparent one)
/// whose alpha is partial. The picture border does not count as a
/// boundary: a subject cut by the frame is a *touching* problem, not a
/// hard-edge one. `None` when the picture has no boundary, which happens
/// for fully opaque pictures.
pub fn soft_edge_fraction(img: &RgbaImage) -> Option<f32> {
    let (w, h) = (img.width as usize, img.height as usize);
    let a = |x: usize, y: usize| img.pixels[(y * w + x) * 4 + 3];
    let mut boundary = 0u32;
    let mut soft = 0u32;
    for y in 0..h {
        for x in 0..w {
            let v = a(x, y);
            if v < 8 {
                continue;
            }
            let clear = |nx: isize, ny: isize| -> bool {
                nx >= 0 && ny >= 0 && nx < w as isize && ny < h as isize && a(nx as usize, ny as usize) < 8
            };
            let (xi, yi) = (x as isize, y as isize);
            if clear(xi - 1, yi) || clear(xi + 1, yi) || clear(xi, yi - 1) || clear(xi, yi + 1) {
                boundary += 1;
                if v < 248 {
                    soft += 1;
                }
            }
        }
    }
    (boundary > 0).then(|| soft as f32 / boundary as f32)
}

/// One cell of a contact sheet.
pub struct SheetEntry {
    pub name: String,
    pub thumb: RgbaImage,
    /// Rejections: a red bar with the reason codes.
    pub reasons: Vec<Reason>,
    /// Warnings: an amber bar with the reason codes.
    pub warnings: Vec<Reason>,
}

/// Lays out thumbnails in a grid on a checkerboard, each with its name and,
/// for rejects, a red bar carrying the reason codes (amber for warnings).
pub fn contact_sheet(title: &str, entries: &[SheetEntry], columns: usize) -> RgbaImage {
    const CELL: u32 = 180;
    const THUMB: u32 = 150;
    const LABEL: u32 = 22;
    let columns = columns.max(1);
    let rows = entries.len().div_ceil(columns).max(1) as u32;
    let width = CELL * columns as u32;
    let height = 28 + rows * (CELL + LABEL);
    let mut sheet = RgbaImage::new(width, height);
    fill_rect(&mut sheet, 0, 0, width, height, [36, 36, 40, 255]);
    let passed = entries.iter().filter(|e| e.reasons.is_empty()).count();
    let header = format!("{title}  {passed}/{} OK", entries.len());
    draw_label(&mut sheet, 8, 8, &header, 2, [240, 240, 240, 255]);
    for (i, e) in entries.iter().enumerate() {
        let col = (i % columns) as u32;
        let row = (i / columns) as u32;
        let x0 = (col * CELL) as i64;
        let y0 = (28 + row * (CELL + LABEL)) as i64;
        let board = checkerboard(CELL - 8, CELL - 8, 10);
        blit(&mut sheet, &board, x0 + 4, y0 + 4);
        let t = resize_to_fit(&e.thumb, THUMB);
        let tx = x0 + 4 + ((CELL - 8 - t.width) / 2) as i64;
        let ty = y0 + 4 + ((CELL - 8 - t.height) / 2) as i64;
        blit(&mut sheet, &t, tx, ty);
        let mut name = e.name.clone();
        while label_width(&name, 1) > CELL - 8 && name.len() > 1 {
            name.pop();
        }
        draw_label(&mut sheet, x0 + 4, y0 + CELL as i64 + 2, &name, 1, [230, 230, 230, 255]);
        let (bar, colour) = if !e.reasons.is_empty() {
            (&e.reasons, [200, 40, 40, 230])
        } else {
            (&e.warnings, [210, 150, 20, 230])
        };
        if !bar.is_empty() {
            fill_rect(&mut sheet, x0 + 4, y0 + 4, CELL - 8, 12, colour);
            let codes: Vec<&str> = bar.iter().map(|r| r.code()).collect();
            let mut text = codes.join(" ");
            while label_width(&text, 1) > CELL - 12 && text.len() > 1 {
                text.pop();
            }
            draw_label(&mut sheet, x0 + 6, y0 + 6, &text, 1, [255, 255, 255, 255]);
        }
    }
    sheet
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disc(size: u32, radius: f32, soft: f32) -> RgbaImage {
        let mut img = RgbaImage::new(size, size);
        let c = size as f32 / 2.0;
        for y in 0..size {
            for x in 0..size {
                let d = ((x as f32 + 0.5 - c).powi(2) + (y as f32 + 0.5 - c).powi(2)).sqrt();
                let a = if soft > 0.0 { (1.0 - (d - radius) / soft).clamp(0.0, 1.0) } else if d <= radius { 1.0 } else { 0.0 };
                let i = ((y * size + x) * 4) as usize;
                img.pixels[i..i + 4].copy_from_slice(&[200, 80, 40, (a * 255.0).round() as u8]);
            }
        }
        img
    }

    #[test]
    fn soft_disc_passes_hard_disc_fails() {
        assert!(check(Kind::Sticker, &disc(400, 150.0, 1.5)).is_empty());
        let hard = check(Kind::Sticker, &disc(400, 150.0, 0.0));
        assert!(hard.iter().any(|r| matches!(r, Reason::HardEdge { .. })), "{hard:?}");
    }

    #[test]
    fn size_rules_per_kind() {
        let small = disc(200, 80.0, 1.5);
        assert!(check(Kind::Sticker, &small).iter().any(|r| matches!(r, Reason::TooSmall { min: 300, .. })));
        assert!(check(Kind::TextArt, &small).is_empty());
        assert!(check(Kind::Background, &RgbaImage::new(2000, 1000)).is_empty());
    }

    #[test]
    fn empty_and_touching() {
        let tiny = disc(400, 10.0, 1.5);
        assert!(check(Kind::Sticker, &tiny).iter().any(|r| matches!(r, Reason::MostlyEmpty { .. })));
        let touching = disc(400, 260.0, 1.5);
        assert!(check(Kind::Sticker, &touching).iter().any(|r| matches!(r, Reason::TouchesFrame { .. })));
        // Frames are allowed to touch the frame.
        assert!(!check(Kind::Frame, &touching).iter().any(|r| matches!(r, Reason::TouchesFrame { .. })));
        // Green-screen effects often fill the picture: a warning, not a rejection.
        let v = verdict(Kind::GreenScreen, &touching);
        assert!(v.passed() && v.warnings.iter().any(|r| matches!(r, Reason::TouchesFrame { .. })), "{v:?}");
        assert_eq!(check(Kind::Sticker, &RgbaImage::new(400, 400)), vec![Reason::Empty]);
    }

    #[test]
    fn contact_sheet_has_expected_size() {
        let entries: Vec<SheetEntry> = (0..5)
            .map(|i| SheetEntry { name: format!("Disc {i}"), thumb: disc(64, 20.0, 1.0), reasons: if i == 2 { vec![Reason::Empty] } else { vec![] }, warnings: vec![] })
            .collect();
        let sheet = contact_sheet("Discs", &entries, 4);
        assert_eq!(sheet.width, 180 * 4);
        assert_eq!(sheet.height, 28 + 2 * (180 + 22));
    }
}
