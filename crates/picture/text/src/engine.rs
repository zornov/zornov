// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! [`TextEngine`]: the one object `render` and `app` hold. It owns the
//! font library, the rasteriser caches and any images used as fills.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::font::FontLibrary;
use crate::layout::{self, Layout};
use crate::paint::{self, Hooks, TextRaster};
use crate::raster::Rasteriser;
use crate::style::{Canvas, TextBlock, TextStyle};
use crate::Image;

pub struct TextEngine {
    pub fonts: FontLibrary,
    raster: Mutex<Rasteriser>,
    images: HashMap<String, Image>,
}

impl TextEngine {
    pub fn new(fonts: FontLibrary) -> TextEngine {
        TextEngine { fonts, raster: Mutex::new(Rasteriser::new()), images: HashMap::new() }
    }

    /// System fonts plus the bundled library.
    pub fn discover() -> TextEngine {
        TextEngine::new(FontLibrary::discover())
    }

    /// Registers an image for [`crate::Fill::Image`] fills.
    pub fn add_image(&mut self, key: impl Into<String>, image: Image) {
        self.images.insert(key.into(), image);
    }

    pub fn images(&self) -> &HashMap<String, Image> {
        &self.images
    }

    pub fn layout(&self, block: &TextBlock, style: &TextStyle, canvas: &Canvas) -> Layout {
        layout::layout(&self.fonts, block, style, canvas)
    }

    pub(crate) fn rasteriser(&self) -> std::sync::MutexGuard<'_, Rasteriser> {
        self.raster.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn paint(&self, layout: &Layout, style: &TextStyle, hooks: &Hooks) -> TextRaster {
        let mut r = self.raster.lock().unwrap_or_else(|e| e.into_inner());
        paint::paint(&mut r, layout, style, hooks, &self.images, self.fonts.emoji())
    }

    /// Layout and paint in one call.
    pub fn render(&self, block: &TextBlock, style: &TextStyle, canvas: &Canvas, hooks: &Hooks) -> TextRaster {
        let l = self.layout(block, style, canvas);
        self.paint(&l, style, hooks)
    }

    /// Renders onto a canvas-sized image with the layout box centred at
    /// `centre` (pixels). Returns the layout for hit-testing.
    pub fn render_onto(&self, target: &mut Image, block: &TextBlock, style: &TextStyle, centre: [f32; 2], hooks: &Hooks) -> Layout {
        let canvas = Canvas::new(target.width, target.height);
        self.render_at(target, &canvas, block, style, centre, hooks)
    }

    /// Renders animated text onto a canvas-sized image, centred at
    /// `centre`: entrances, exits, loops, custom motions and animators at
    /// `t` seconds into a clip `dur` long, with animator blur.
    pub fn render_motion(&self, target: &mut Image, block: &TextBlock, style: &TextStyle, centre: [f32; 2], motion: &crate::anim::TextMotion, t: f32, dur: f32) -> Layout {
        use crate::paint::UnitPose;
        use crate::style::TextUnit;
        let canvas = Canvas::new(target.width, target.height);
        let px = canvas.px_per_unit();
        let size_px = style.size * px;
        let l = self.layout(block, style, &canvas);
        let counts = |u: TextUnit| match u {
            TextUnit::Character => l.char_count,
            TextUnit::Word => l.word_count,
            TextUnit::Line => l.lines.len(),
        };
        let blur_unit = motion.blur_unit();
        let blurs: Vec<f32> = blur_unit.map(|u| (0..counts(u)).map(|i| motion.blur(t, i, counts(u), px)).collect()).unwrap_or_default();
        let most = blurs.iter().copied().fold(0.0f32, f32::max);
        // Up to five blur levels: each unit drawn in the nearest one.
        let step = (most / 4.0).max(0.5);
        let level = |b: f32| (b / step).round() as i32;
        let mut levels: Vec<i32> = if most < 0.25 { vec![0] } else { blurs.iter().map(|b| level(*b)).collect() };
        levels.sort_unstable();
        levels.dedup();
        let (x, y) = (centre[0] - l.bounds.w * 0.5, centre[1] - l.bounds.h * 0.5);
        for lv in levels {
            let pose = |u: TextUnit| {
                let blurs = &blurs;
                move |i: usize, n: usize| -> UnitPose {
                    let p = motion.pose_px(u, t, dur, i, n, size_px, px);
                    if Some(u) == blur_unit && most >= 0.25 && blurs.get(i).map(|b| level(*b)) != Some(lv) { p.opacity(0.0) } else { p }
                }
            };
            let (c, w, li) = (pose(TextUnit::Character), pose(TextUnit::Word), pose(TextUnit::Line));
            let on = |u: TextUnit| motion.uses(u) || (Some(u) == blur_unit && most >= 0.25);
            let hooks = Hooks {
                chars: on(TextUnit::Character).then_some(&c as &dyn Fn(usize, usize) -> UnitPose),
                words: on(TextUnit::Word).then_some(&w as &dyn Fn(usize, usize) -> UnitPose),
                lines: on(TextUnit::Line).then_some(&li as &dyn Fn(usize, usize) -> UnitPose),
            };
            let raster = self.paint(&l, style, &hooks);
            if lv == 0 {
                raster.blit(target, x, y);
            } else {
                crate::raster::blurred(&raster, lv as f32 * step).blit(target, x, y);
            }
        }
        l
    }

    /// Like [`TextEngine::render_onto`] but with an explicit canvas for the
    /// unit scale (the target may be a crop or a sheet cell).
    pub fn render_at(&self, target: &mut Image, canvas: &Canvas, block: &TextBlock, style: &TextStyle, centre: [f32; 2], hooks: &Hooks) -> Layout {
        let l = self.layout(block, style, canvas);
        let raster = self.paint(&l, style, hooks);
        raster.blit(target, centre[0] - l.bounds.w * 0.5, centre[1] - l.bounds.h * 0.5);
        l
    }

    /// A small specimen card: `sample` in the requested face, dark on a
    /// light card, shrunk to fit. Also says which face answered, since a
    /// family the library lacks is replaced by its fallback.
    pub fn specimen(&self, req: &crate::font::FontRequest, sample: &str, w: u32, h: u32) -> (Image, SpecimenFace) {
        let (w, h) = (w.max(32), h.max(16));
        let mut img = Image::new(w, h);
        for p in img.pixels.iter_mut() {
            *p = [0.96, 0.96, 0.95, 1.0];
        }
        let canvas = Canvas::new(w, h);
        let ink = crate::Fill::solid(crate::Rgba { r: 0.06, g: 0.06, b: 0.07, a: 1.0 });
        let mut size = 1080.0 * 0.42;
        let mut style = TextStyle::new(req.clone(), size, ink.clone());
        let mut l = self.layout(&TextBlock::plain(sample), &style, &canvas);
        for _ in 0..3 {
            let fit = (w as f32 * 0.9 / l.bounds.w.max(1.0)).min(h as f32 * 0.8 / l.bounds.h.max(1.0));
            if fit >= 0.98 && fit <= 1.6 {
                break;
            }
            size = (size * fit).clamp(8.0, 1080.0);
            style = TextStyle::new(req.clone(), size, ink.clone());
            l = self.layout(&TextBlock::plain(sample), &style, &canvas);
        }
        let raster = self.paint(&l, &style, &Hooks::NONE);
        raster.blit(&mut img, (w as f32 - l.bounds.w) * 0.5, (h as f32 - l.bounds.h) * 0.5);
        let face = match self.fonts.resolve(req) {
            Some(m) => SpecimenFace {
                family: m.face.info.family.clone(),
                weight: m.weight_variation.map(|v| v.round() as u16).unwrap_or(m.face.info.weight),
                italic: m.face.info.italic || m.synthetic_italic,
                synthetic_bold: m.synthetic_bold,
                synthetic_italic: m.synthetic_italic,
                variable: m.weight_variation.is_some(),
                exact: self.fonts.match_family(req).is_some(),
            },
            None => SpecimenFace { family: String::new(), weight: req.weight, italic: req.italic, synthetic_bold: false, synthetic_italic: false, variable: false, exact: false },
        };
        (img, face)
    }

    /// The face a request resolves to, without drawing anything.
    pub fn resolved(&self, req: &crate::font::FontRequest) -> Option<SpecimenFace> {
        let m = self.fonts.resolve(req)?;
        Some(SpecimenFace {
            family: m.face.info.family.clone(),
            weight: m.weight_variation.map(|v| v.round() as u16).unwrap_or(m.face.info.weight),
            italic: m.face.info.italic || m.synthetic_italic,
            synthetic_bold: m.synthetic_bold,
            synthetic_italic: m.synthetic_italic,
            variable: m.weight_variation.is_some(),
            exact: self.fonts.match_family(req).is_some(),
        })
    }
}

/// The face a font request resolved to.
#[derive(Clone, Debug, PartialEq)]
pub struct SpecimenFace {
    pub family: String,
    pub weight: u16,
    pub italic: bool,
    pub synthetic_bold: bool,
    pub synthetic_italic: bool,
    /// A variable font set to the weight (any weight in range is exact).
    pub variable: bool,
    /// The family asked for is the one that answered (false: a fallback).
    pub exact: bool,
}
