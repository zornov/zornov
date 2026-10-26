// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Renders review sheets into `crates/picture/text/review/`:
//! `styles.png` (every text style), `templates-<category>.png` (every
//! template at three moments: entering, holding, leaving) and
//! `captions.png` (every caption style at two moments of a sample cue).
//!
//! `cargo run --release --example contact_sheet [styles|templates|captions|all]`
use std::collections::HashMap;
use std::path::Path;

use studio_text::*;

fn checker(w: u32, h: u32, dark: bool) -> Image {
    Image::from_fn(w, h, |x, y| {
        let k = ((x / 40) + (y / 40)) % 2;
        let v = if dark { 0.015 + 0.012 * k as f32 } else { 0.55 + 0.12 * k as f32 };
        [v, v, v, 1.0]
    })
}

/// A soft "video-like" backdrop: a dark gradient with a brighter blob, so
/// captions and lower thirds are judged over something closer to footage.
fn footage(w: u32, h: u32, seed: u32) -> Image {
    Image::from_fn(w, h, |x, y| {
        let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
        let cx = 0.3 + 0.4 * ((seed * 37) % 10) as f32 / 10.0;
        let d = ((fx - cx).powi(2) + (fy - 0.4).powi(2)).sqrt();
        let blob = (1.0 - d * 1.8).clamp(0.0, 1.0).powi(2);
        let base = 0.02 + 0.06 * (1.0 - fy);
        let tint = [0.9 + 0.1 * (seed % 3) as f32, 0.75, 0.6 + 0.3 * ((seed + 1) % 3) as f32];
        [base + blob * 0.18 * tint[0], base + blob * 0.14 * tint[1], base + blob * 0.12 * tint[2], 1.0]
    })
}

fn label(engine: &TextEngine, img: &mut Image, text: &str, x: f32, y: f32, size: f32) {
    let style = TextStyle::new(FontRequest::new("Inter", 500, false), size, Fill::Solid(Rgba::rgb8(160, 160, 170)));
    let block = TextBlock::plain(text).with_layout(LayoutParams::default().align(Align::Left));
    let l = engine.layout(&block, &style, &Canvas::HD);
    let r = engine.paint(&l, &style, &Hooks::NONE);
    r.blit(img, x, y);
}

fn styles_sheet(engine: &TextEngine, out: &Path) {
    let all = styles::catalogue();
    let cols = 4u32;
    let (cell_w, cell_h) = (480u32, 200u32);
    let rows = (all.len() as u32).div_ceil(cols);
    let mut sheet = Image::from_fn(cols * cell_w, rows * cell_h, |_, _| [0.03, 0.03, 0.035, 1.0]);
    for (i, p) in all.iter().enumerate() {
        let (cx, cy) = ((i as u32 % cols) * cell_w, (i as u32 / cols) * cell_h);
        // Light backgrounds for dark styles so they stay visible.
        let visible = match &p.style.fill {
            Fill::None => p.style.strokes.first().map(|s| s.colour).unwrap_or(Rgba::WHITE),
            f => f.representative(),
        };
        let dark_text = visible.luma() < 0.2 && visible.a > 0.3 && p.style.background.is_none();
        let mut cell = checker(cell_w * 2, cell_h * 2, !dark_text);
        let block = TextBlock::plain(&p.sample).with_layout(LayoutParams::default().boxed(900.0, 300.0).fit());
        let style = p.style.clone();
        engine.render_at(&mut cell, &Canvas::HD, &block, &style, [cell_w as f32, cell_h as f32 - 10.0], &Hooks::NONE);
        raster::draw_image_affine(&mut sheet, &cell, &Affine::scale(0.5, 0.5).then(&Affine::translate(cx as f32, cy as f32)), 1.0);
        label(engine, &mut sheet, &format!("{}  ·  {}", p.name, p.id), cx as f32 + 10.0, cy as f32 + cell_h as f32 - 30.0, 16.0);
    }
    png_io::write_png(out, &sheet).unwrap();
    println!("wrote {} ({} styles)", out.display(), all.len());
}

fn templates_sheets(engine: &TextEngine, dir: &Path) {
    let all = templates::all();
    for cat in TemplateCategory::ALL {
        let scenes: Vec<&Scene> = all.iter().filter(|s| s.category == cat).collect();
        if scenes.is_empty() {
            continue;
        }
        let (fw, fh) = (640u32, 360u32);
        let row_h = fh + 34;
        let mut sheet = Image::from_fn(fw * 3 + 20, row_h * scenes.len() as u32, |_, _| [0.03, 0.03, 0.035, 1.0]);
        for (ri, s) in scenes.iter().enumerate() {
            let values: HashMap<String, String> = s.default_values();
            let d = s.duration;
            let times = [s.intro * 0.45, (s.intro + (d - s.outro)) * 0.5, d - s.outro * 0.5];
            for (ci, t) in times.iter().enumerate() {
                let mut frame = footage(fw * 2, fh * 2, ri as u32);
                s.render_onto(engine, &mut frame, *t, d, &values);
                let (x, y) = (ci as u32 * (fw + 10), ri as u32 * row_h);
                raster::draw_image_affine(&mut sheet, &frame, &Affine::scale(0.5, 0.5).then(&Affine::translate(x as f32, y as f32)), 1.0);
            }
            label(engine, &mut sheet, &format!("{}  ·  {}  ·  {:.1}s  (t = {:.2}, {:.2}, {:.2})", s.name, s.id, d, times[0], times[1], times[2]), 8.0, (ri as u32 * row_h + fh + 6) as f32, 16.0);
        }
        let out = dir.join(format!("templates-{}.png", cat.label().to_lowercase().replace(' ', "-")));
        png_io::write_png(&out, &sheet).unwrap();
        println!("wrote {} ({} templates)", out.display(), scenes.len());
    }
}

fn captions_sheet(engine: &TextEngine, out: &Path) {
    let all = captions::catalogue();
    let (fw, fh) = (640u32, 360u32);
    let row_h = fh + 34;
    let mut sheet = Image::from_fn(fw * 2 + 10, row_h * all.len() as u32, |_, _| [0.03, 0.03, 0.035, 1.0]);
    for (ri, st) in all.iter().enumerate() {
        let mut track = CaptionTrack::new(st.clone());
        let a = track.add_speaker("Maya", Rgba::rgb8(255, 209, 102));
        let b = track.add_speaker("Sam", Rgba::rgb8(126, 220, 255));
        let mut c1 = Cue::from_text("Every word lights up as it is spoken, two lines at most", 0.0, 4.0).with_speaker(a);
        c1.words[1].keyword = true;
        track.cues.push(c1);
        track.cues.push(Cue::from_text("then the other speaker takes over in their own colour", 4.2, 7.5).with_speaker(b));
        for (ci, t) in [1.9f32, 5.6].iter().enumerate() {
            let mut frame = footage(fw * 2, fh * 2, ri as u32 + 3);
            track.render_onto(engine, &mut frame, *t);
            let (x, y) = (ci as u32 * (fw + 10), ri as u32 * row_h);
            raster::draw_image_affine(&mut sheet, &frame, &Affine::scale(0.5, 0.5).then(&Affine::translate(x as f32, y as f32)), 1.0);
        }
        label(engine, &mut sheet, &format!("{}  ·  {}  ·  {:?}", st.name, st.id, st.position), 8.0, (ri as u32 * row_h + fh + 6) as f32, 16.0);
    }
    png_io::write_png(out, &sheet).unwrap();
    println!("wrote {} ({} caption styles)", out.display(), all.len());
}

fn main() {
    let what = std::env::args().nth(1).unwrap_or_else(|| "all".into());
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("review");
    std::fs::create_dir_all(&dir).unwrap();
    let engine = TextEngine::new(FontLibrary::bundled_only());
    if what == "styles" || what == "all" {
        styles_sheet(&engine, &dir.join("styles.png"));
    }
    if what == "templates" || what == "all" {
        templates_sheets(&engine, &dir);
    }
    if what == "captions" || what == "all" {
        captions_sheet(&engine, &dir.join("captions.png"));
    }
}
