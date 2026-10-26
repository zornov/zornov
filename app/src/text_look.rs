// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The `text` crate's engine for the app: Manrope (bundled) plus the fonts
//! in the content library. Used for Text panel previews and, through
//! `render`'s text hook, for titles in the viewer and in exports.

use std::sync::{Arc, OnceLock};
use studio_text::{FontLibrary, TextEngine};

pub fn engine() -> &'static Arc<TextEngine> {
    static ENGINE: OnceLock<Arc<TextEngine>> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let mut fonts = FontLibrary::new();
        // The library loads files, so write the bundled faces once.
        let dir = std::env::temp_dir().join("studio-fonts");
        let _ = std::fs::create_dir_all(&dir);
        for (name, bytes) in [
            ("Manrope-Medium.ttf", include_bytes!("../assets/fonts/Manrope-Medium.ttf").as_slice()),
            ("Manrope-SemiBold.ttf", include_bytes!("../assets/fonts/Manrope-SemiBold.ttf").as_slice()),
            ("Manrope-Bold.ttf", include_bytes!("../assets/fonts/Manrope-Bold.ttf").as_slice()),
        ] {
            let path = dir.join(name);
            if !path.exists() {
                let _ = std::fs::write(&path, bytes);
            }
            let _ = fonts.load_file(&path);
        }
        // Fonts shipped in the content bundle.
        if let Some(c) = crate::content::find_dir() {
            let mut stack = vec![c.join("core/fonts")];
            while let Some(d) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&d) else { continue };
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else if p.extension().is_some_and(|x| x == "ttf" || x == "otf") {
                        let _ = fonts.load_file(&p);
                    }
                }
            }
        }
        // The bundled library from the catalogue (families are read lazily)
        // and the fonts installed on this computer, so a title can use any
        // of them by family name. `STUDIO_NO_SYSTEM_FONTS` keeps the system
        // fonts out (for reproducible renders).
        if let Some(c) = crate::content::find_dir() {
            let _ = fonts.load_bundled(&c);
        }
        if std::env::var_os("STUDIO_NO_SYSTEM_FONTS").is_none() {
            fonts.load_system();
        }
        Arc::new(TextEngine::new(fonts))
    })
}

/// A text style preset rendered on a dark card: the preview tile for the
/// Text panel. `sample` replaces the preset's own sample text.
pub fn style_tile(index: usize, w: usize, h: usize, sample: Option<&str>) -> Option<eframe::egui::ColorImage> {
    let preset = studio_text::styles::catalogue().get(index)?;
    let mut img = studio_colour::Image::new(w as u32, h as u32);
    // A soft vertical gradient so light and dark styles both read.
    for y in 0..h {
        let k = y as f32 / h as f32;
        let v = 0.012 + 0.02 * (1.0 - k);
        for x in 0..w {
            img.pixels[y * w + x] = [v, v * 0.95, v * 0.9, 1.0];
        }
    }
    let mut style = preset.style.clone();
    // The tile is the canvas here, so units are 1/1080 of its height:
    // about a quarter of the height reads well.
    style.size = 270.0;
    let text = sample.unwrap_or("Aa");
    let block = studio_text::TextBlock::plain(text);
    engine().render_onto(&mut img, &block, &style, [w as f32 / 2.0, h as f32 / 2.0], &studio_text::Hooks::NONE);
    let f = studio_fx::Image { width: w, height: h, pixels: img.pixels };
    Some(eframe::egui::ColorImage::from_rgba_unmultiplied([w, h], &f.to_srgb8()))
}

/// The text style a title clip draws with: a style from the Text panel
/// (`preset`, may be empty) at `size` (canvas units; below 0 keeps the
/// style's own), or plain Manrope in `colour`. A colour other than the
/// default white wins over the style's fill; `plate: false` drops the
/// style's own box.
pub fn style_of(preset: &str, size: f32, letter_spacing: f32, colour: [f32; 4], plate: bool) -> studio_text::TextStyle {
    let fill = studio_text::Fill::Solid(studio_text::Rgba { r: colour[0], g: colour[1], b: colour[2], a: colour[3] });
    match Some(preset).filter(|s| !s.is_empty()).and_then(studio_text::styles::find) {
        Some(p) => {
            let mut ts = studio_text::TextStyle { size: if size >= 0.0 { size } else { p.style.size }, ..p.style.clone() };
            if colour != [1.0, 1.0, 1.0, 1.0] {
                ts.fill = fill;
            }
            if !plate {
                ts.background = None;
            }
            ts
        }
        None => studio_text::TextStyle {
            font: studio_text::FontRequest::new("Manrope", 700, false),
            size: if size >= 0.0 { size } else { 40.0 },
            letter_spacing,
            fill,
            ..Default::default()
        },
    }
}

/// Applies a title's own font choice over its style: `family` (empty keeps
/// the style's), `weight` (below 1 keeps it) and `italic` (None keeps it).
pub fn set_font(ts: &mut studio_text::TextStyle, family: &str, weight: f32, italic: Option<bool>) {
    if !family.trim().is_empty() {
        ts.font.family = family.trim().to_owned();
    }
    if weight >= 1.0 {
        ts.font.weight = weight.round().clamp(1.0, 1000.0) as u16;
    }
    if let Some(i) = italic {
        ts.font.italic = i;
    }
}

/// What a font request will draw with, for an agent's reply: the face that
/// answers and any stand-in (a slanted copy when no italic exists, an
/// emboldened one when no bold does). An unknown family is an error that
/// names the closest installed ones.
pub fn font_report(family: &str, weight: u16, italic: bool) -> Result<String, String> {
    let engine = engine();
    let req = studio_text::FontRequest::new(family, weight, italic);
    let face = engine.resolved(&req).filter(|f| f.exact).ok_or_else(|| {
        let near = engine.fonts.similar_families(family, 5);
        format!(
            "no font \"{family}\" is installed or bundled{}. `fonts` with a query lists and searches the available ones (by name, serif, sans, display, script, mono, art deco, handwritten...); a stand-in would not look like what you asked for, so nothing was changed",
            if near.is_empty() { String::new() } else { format!(" (close: {})", near.join(", ")) }
        )
    })?;
    let mut s = format!("{} {}{}", face.family, face.weight, if face.italic { " italic" } else { "" });
    if face.variable {
        s += " (variable, exact weight)";
    }
    if face.synthetic_bold {
        s += "; no bold face in this family, so it is emboldened";
    }
    if face.synthetic_italic {
        s += "; no italic face in this family, so it is slanted";
    }
    if !face.variable && face.weight != weight && !face.synthetic_bold {
        s += &format!("; the family has no {weight}, the nearest weight is used");
    }
    Ok(s)
}
