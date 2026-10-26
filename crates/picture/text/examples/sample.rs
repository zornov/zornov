// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Renders a handful of styled blocks to a PNG for a quick visual check.
//! `cargo run --example sample -- out.png`
use studio_text::*;

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "sample.png".into());
    let engine = TextEngine::discover();
    let mut img = Image::from_fn(1920, 1080, |x, y| {
        let v = 0.02 + 0.03 * (((x / 60) + (y / 60)) % 2) as f32;
        [v, v, v, 1.0]
    });
    let red = Rgba::rgb8(240, 60, 70);
    let styles = vec![
        ("Plain Inter 64", TextStyle::new(FontRequest::bold("Inter"), 64.0, Fill::Solid(Rgba::WHITE))),
        (
            "Stroke + shadow",
            TextStyle::new(FontRequest::new("Bebas Neue", 400, false), 90.0, Fill::Solid(Rgba::rgb8(255, 220, 60)))
                .with_stroke(Stroke::new(Rgba::BLACK, 3.0))
                .with_shadow(Shadow::new(Rgba::BLACK, 6.0, 6.0, 60.0).opacity(0.8)),
        ),
        (
            "Gradient glow",
            TextStyle::new(FontRequest::bold("Montserrat"), 70.0, Fill::Gradient(Gradient::new(0.0, &[Rgba::rgb8(90, 200, 255), Rgba::rgb8(230, 90, 255)])))
                .with_glow(Glow::new(Rgba::rgb8(120, 120, 255), 0.8, 10.0)),
        ),
        (
            "Box + two strokes + extrusion",
            TextStyle::new(FontRequest::new("Anton", 400, false), 72.0, Fill::Solid(Rgba::WHITE))
                .with_stroke(Stroke::new(red, 2.0))
                .with_stroke(Stroke::new(Rgba::WHITE, 2.0))
                .with_extrusion(Extrusion::new(Rgba::rgb8(120, 20, 30), 8.0, 45.0))
                .with_background(Background::new(Rgba::rgb8(20, 20, 40), 10.0, 20.0, 10.0)),
        ),
        (
            "Rainbow cycle, mixed script 😀 مرحبا 你好",
            TextStyle::new(FontRequest::bold("Noto Sans"), 56.0, Fill::Solid(Rgba::WHITE))
                .with_cycle(ColourCycle::new(&[Rgba::rgb8(255, 80, 80), Rgba::rgb8(255, 200, 60), Rgba::rgb8(80, 220, 120), Rgba::rgb8(80, 160, 255)], TextUnit::Character))
                .with_stroke(Stroke::new(Rgba::BLACK, 2.0)),
        ),
    ];
    let mut y = 110.0;
    for (text, style) in &styles {
        engine.render_onto(&mut img, &TextBlock::plain(*text), style, [960.0, y], &Hooks::NONE);
        y += 150.0;
    }
    // Curved + underline + italic span.
    let block = TextBlock::spans(vec![Span::plain("Curved "), Span::styled("italic", SpanStyle::italic()), Span::plain(" text on an arc")]).with_layout(LayoutParams::default().bend(90.0));
    let style = TextStyle::new(FontRequest::bold("Playfair Display"), 60.0, Fill::Solid(Rgba::rgb8(255, 240, 200))).with_shadow(Shadow::new(Rgba::BLACK, 4.0, 3.0, 90.0));
    engine.render_onto(&mut img, &block, &style, [960.0, y + 40.0], &Hooks::NONE);
    // Animated hook: per-character wave.
    let wave = |i: usize, _n: usize| UnitPose::REST.offset(0.0, (i as f32 * 0.8).sin() * 14.0).rotation((i as f32 * 0.8).cos() * 10.0);
    let hooks = Hooks { chars: Some(&wave), ..Default::default() };
    let style = TextStyle::new(FontRequest::bold("Poppins"), 56.0, Fill::Solid(Rgba::rgb8(140, 255, 200))).with_stroke(Stroke::new(Rgba::rgb8(0, 60, 40), 3.0));
    engine.render_onto(&mut img, &TextBlock::plain("per character hooks wave"), &style, [960.0, y + 190.0], &hooks);
    png_io::write_png(std::path::Path::new(&out), &img).unwrap();
    println!("wrote {out}");
}
