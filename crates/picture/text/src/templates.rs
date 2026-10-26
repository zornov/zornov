// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Studio's animated text templates: titles, lower thirds, callouts,
//! calls to action, end cards, quotes and labels, each a [`Scene`].
//! All compositions, timings, names and palettes are our own design.
//!
//! The scenes are authored here in Rust and also shipped as JSON in
//! `templates/*.json` (regenerate with `cargo run --example gen_templates`);
//! a test keeps the two in step.

use std::sync::OnceLock;

use crate::anim::{LoopKind, LoopMotion, Order, Reveal, RevealKind};
use crate::font::FontRequest;
use crate::geom::Ease;
use crate::rgba::Rgba;
use crate::scene::{Layer, Phase, Property, Scene, TemplateCategory};
use crate::style::*;

fn c(hex: &str) -> Rgba {
    Rgba::hex(hex)
}

fn st(family: &str, weight: u16, size: f32, fill: Rgba) -> TextStyle {
    TextStyle::new(FontRequest::new(family, weight, false), size, Fill::Solid(fill))
}

fn sti(family: &str, weight: u16, size: f32, fill: Rgba) -> TextStyle {
    TextStyle::new(FontRequest::new(family, weight, true), size, Fill::Solid(fill))
}

fn rv(kind: RevealKind, unit: TextUnit) -> Reveal {
    Reveal::new(kind, unit)
}

fn left(w: f32) -> LayoutParams {
    LayoutParams::default().align(Align::Left).wrap(w)
}

fn centre(w: f32) -> LayoutParams {
    LayoutParams::default().align(Align::Centre).wrap(w)
}

const WHITE: Rgba = Rgba::WHITE;
const BLACK: Rgba = Rgba::BLACK;

fn build() -> Vec<Scene> {
    use TemplateCategory::*;
    let mut v: Vec<Scene> = Vec::new();

    // ---------------------------------------------------------------- Titles
    v.push(
        Scene::new("title.clean-rise", "Clean Rise", Title, 5.0, 1.4, 0.8)
            .tags("minimal, sans, rise")
            .field("title", "Title", "Make something great")
            .field("subtitle", "Subtitle", "A short film by Studio")
            .layer(Layer::text("title", "title", st("Inter", 700, 92.0, WHITE).with_shadow(Shadow::new(BLACK, 8.0, 0.0, 0.0).opacity(0.4)), centre(1500.0)).at(0.5, 0.47)
                .reveal(rv(RevealKind::Rise, TextUnit::Word).overlap(0.7).distance(0.6), rv(RevealKind::Fade, TextUnit::Word).overlap(0.9)).reveal_timing(0.0, 1.0))
            .layer(Layer::line("rule", c("#8B6CFF"), 120.0, 4.0).at(0.5, 0.565).in_out(Property::Width, 0.0, 1.0, 0.6, Ease::Out).track(Property::Opacity, Phase::Intro, &[(0.0, 0.0, Ease::Linear), (0.3, 0.0, Ease::Linear), (0.6, 1.0, Ease::Out)]))
            .layer(Layer::text("subtitle", "subtitle", st("Inter", 500, 30.0, c("#D8D8E0")).letter_spacing(22.0).case(Case::Upper), centre(1500.0)).at(0.5, 0.62)
                .reveal(rv(RevealKind::Fade, TextUnit::Character).overlap(0.9).order(Order::Centre), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.6, 0.8)),
    );
    v.push(
        Scene::new("title.cinematic-wide", "Cinematic Wide", Title, 6.0, 2.0, 1.2)
            .tags("film, caps, tracking, slow")
            .field("title", "Title", "THE LAST LIGHT")
            .field("subtitle", "Subtitle", "Part One")
            .layer(Layer::text("title", "title", st("Josefin Sans", 300, 86.0, WHITE).letter_spacing(44.0).case(Case::Upper).with_shadow(Shadow::new(BLACK, 20.0, 0.0, 0.0).opacity(0.8)), centre(1700.0)).at(0.5, 0.5)
                .reveal(rv(RevealKind::Zoom, TextUnit::Character).overlap(0.95).order(Order::Centre).ease(Ease::Out).distance(0.3), rv(RevealKind::Fade, TextUnit::Character).overlap(0.95).order(Order::Centre))
                .track(Property::Scale, Phase::Intro, &[(0.0, 1.06, Ease::Linear), (6.0, 1.0, Ease::Linear)]))
            .layer(Layer::text("subtitle", "subtitle", sti("Cormorant Garamond", 500, 40.0, c("#E8E2D0")).letter_spacing(10.0), centre(1000.0)).at(0.5, 0.6)
                .reveal(rv(RevealKind::Fade, TextUnit::Line), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(1.0, 0.9)),
    );
    v.push(
        Scene::new("title.bold-box", "Bold Box", Title, 5.0, 1.2, 0.7)
            .tags("bold, box, red, punchy")
            .field("title", "Title", "EPISODE FOUR")
            .field("label", "Label", "New series")
            .layer(Layer::rect("box", c("#E63946"), 900.0, 150.0, 0.0).at(0.5, 0.5).anchor(0.0, 0.5).track(Property::X, Phase::Intro, &[(0.0, -450.0, Ease::Linear)]).in_out(Property::Width, 0.0, 1.0, 0.45, Ease::Out))
            .layer(Layer::text("title", "title", st("Anton", 400, 100.0, WHITE).case(Case::Upper).letter_spacing(4.0), centre(880.0)).at(0.5, 0.5)
                .reveal(rv(RevealKind::SlideLeft, TextUnit::Line).distance(0.4), rv(RevealKind::SlideRight, TextUnit::Line).distance(0.4)).reveal_timing(0.35, 0.5).outro_timing(0.0, 0.4))
            .layer(Layer::text("label", "label", st("Inter", 700, 28.0, c("#1A1A1A")).case(Case::Upper).letter_spacing(14.0).with_background(Background::new(WHITE, 999.0, 18.0, 8.0)), centre(600.0)).at(0.5, 0.635)
                .reveal(rv(RevealKind::Drop, TextUnit::Line).distance(0.8).ease(Ease::Overshoot), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.8, 0.4)),
    );
    v.push(
        Scene::new("title.neon-sign", "Neon Sign", Title, 5.0, 1.6, 0.8)
            .tags("neon, glow, night, script")
            .field("title", "Title", "open late")
            .field("subtitle", "Subtitle", "every night till 2am")
            .layer(Layer::text("title", "title", st("Pacifico", 400, 120.0, c("#FFE6F4")).with_glow(Glow::new(c("#FF2E9A"), 0.9, 16.0)).with_stroke(Stroke::new(c("#FF5FB5"), 1.0)), centre(1600.0)).at(0.5, 0.46)
                .reveal(rv(RevealKind::Fade, TextUnit::Character).overlap(0.3).order(Order::Random).ease(Ease::Hold), rv(RevealKind::Fade, TextUnit::Character).overlap(0.4).order(Order::Random).ease(Ease::Hold)).reveal_timing(0.0, 1.1))
            .layer(Layer::text("subtitle", "subtitle", st("Space Grotesk", 500, 34.0, c("#9DF2FF")).with_glow(Glow::new(c("#00E5FF"), 0.7, 8.0)).letter_spacing(8.0), centre(1200.0)).at(0.5, 0.62)
                .reveal(rv(RevealKind::Fade, TextUnit::Line), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(1.0, 0.5)),
    );
    v.push(
        Scene::new("title.typewriter", "Typewriter", Title, 5.0, 2.0, 0.6)
            .tags("mono, type, cursor, tech")
            .field("title", "Text", "Recording log, day 14.")
            .layer(Layer::text("title", "title", st("JetBrains Mono", 500, 56.0, c("#E8F5E9")).with_background(Background::new(c("#0B1A12E6"), 6.0, 26.0, 14.0)), left(1200.0)).at(0.5, 0.5).anchor(0.5, 0.5)
                .reveal(rv(RevealKind::Type, TextUnit::Character).overlap(0.0), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.2, 1.6))
            .layer(Layer::rect("cursor", c("#5CFF8A"), 6.0, 56.0, 1.0).at(0.5, 0.5).anchor(0.5, 0.5).track(Property::X, Phase::Intro, &[(0.0, -560.0, Ease::Linear), (0.2, -560.0, Ease::Linear), (1.8, 560.0, Ease::Linear), (1.9, 560.0, Ease::Linear)])
                .track(Property::Opacity, Phase::Intro, &[(0.0, 0.0, Ease::Linear), (0.1, 1.0, Ease::Linear), (1.9, 1.0, Ease::Linear), (2.0, 0.0, Ease::Linear)]).track(Property::Opacity, Phase::Outro, &[(0.0, 0.0, Ease::Linear)])),
    );
    v.push(
        Scene::new("title.split-lines", "Split Lines", Title, 5.0, 1.3, 0.8)
            .tags("two lines, slide, editorial")
            .field("line1", "Line 1", "Stories from")
            .field("line2", "Line 2", "the North Coast")
            .layer(Layer::text("line1", "line1", st("Playfair Display", 400, 84.0, WHITE), LayoutParams::default().align(Align::Right).wrap(1400.0)).at(0.5, 0.455)
                .reveal(rv(RevealKind::SlideLeft, TextUnit::Line).distance(0.8), rv(RevealKind::SlideLeft, TextUnit::Line).distance(0.8)).reveal_timing(0.0, 0.9))
            .layer(Layer::line("rule", WHITE, 1400.0, 2.0).at(0.5, 0.515).in_out(Property::Width, 0.0, 1.0, 0.8, Ease::InOut))
            .layer(Layer::text("line2", "line2", sti("Playfair Display", 700, 84.0, c("#FFD166")), LayoutParams::default().align(Align::Left).wrap(1400.0)).at(0.5, 0.58)
                .reveal(rv(RevealKind::SlideRight, TextUnit::Line).distance(0.8), rv(RevealKind::SlideRight, TextUnit::Line).distance(0.8)).reveal_timing(0.2, 0.9)),
    );
    v.push(
        Scene::new("title.pop-stack", "Pop Stack", Title, 4.5, 1.2, 0.6)
            .tags("playful, pop, words, bubble")
            .field("title", "Title", "Best Day Ever")
            .layer(Layer::text("title", "title", st("Luckiest Guy", 400, 120.0, WHITE).with_stroke(Stroke::new(c("#FF6FAE"), 5.0)).with_stroke(Stroke::new(WHITE, 2.5)).with_shadow(Shadow::new(c("#C2185B"), 0.0, 8.0, 90.0).opacity(0.6)).with_cycle(ColourCycle::new(&[c("#FFFFFF"), c("#FFF3B0"), c("#CDF5FD")], TextUnit::Word)), centre(1500.0)).at(0.5, 0.5)
                .reveal(rv(RevealKind::Pop, TextUnit::Word).overlap(0.5).ease(Ease::Overshoot), rv(RevealKind::Pop, TextUnit::Word).overlap(0.6).ease(Ease::In))
                .motion(LoopMotion::new(LoopKind::Wobble, TextUnit::Word).speed(0.5).amount(0.25))),
    );
    v.push(
        Scene::new("title.tech-grid", "Tech Grid", Title, 5.0, 1.5, 0.8)
            .tags("tech, glow, brackets, type")
            .field("title", "Title", "SYSTEM ONLINE")
            .field("subtitle", "Subtitle", "v2.4 build 1180")
            .layer(Layer::rect("bracket-l", c("#4CC9F0"), 6.0, 160.0, 0.0).at(0.22, 0.5).in_out(Property::Height, 0.0, 1.0, 0.5, Ease::Out))
            .layer(Layer::rect("bracket-r", c("#4CC9F0"), 6.0, 160.0, 0.0).at(0.78, 0.5).in_out(Property::Height, 0.0, 1.0, 0.5, Ease::Out))
            .layer(Layer::text("title", "title", st("Space Grotesk", 700, 84.0, c("#DFF6FF")).with_glow(Glow::new(c("#4CC9F0"), 0.6, 10.0)).letter_spacing(18.0).case(Case::Upper), centre(1000.0)).at(0.5, 0.48)
                .reveal(rv(RevealKind::Type, TextUnit::Character).overlap(0.0), rv(RevealKind::Fade, TextUnit::Character).overlap(0.2).order(Order::Backward)).reveal_timing(0.3, 1.0))
            .layer(Layer::text("subtitle", "subtitle", st("JetBrains Mono", 400, 28.0, c("#8FD3E8")).letter_spacing(6.0), centre(1000.0)).at(0.5, 0.58)
                .reveal(rv(RevealKind::Fade, TextUnit::Line), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(1.1, 0.4)),
    );
    v.push(
        Scene::new("title.editorial", "Editorial", Title, 5.5, 1.6, 0.9)
            .tags("magazine, serif, caps, elegant")
            .field("kicker", "Kicker", "Issue 12")
            .field("title", "Title", "THE INTERVIEW")
            .field("subtitle", "Subtitle", "with Maya Chen")
            .layer(Layer::text("kicker", "kicker", st("Inter", 500, 24.0, c("#D9D9D9")).letter_spacing(30.0).case(Case::Upper), centre(800.0)).at(0.5, 0.38)
                .reveal(rv(RevealKind::Fade, TextUnit::Line), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.0, 0.5))
            .layer(Layer::text("title", "title", st("Playfair Display", 700, 96.0, WHITE).letter_spacing(12.0).case(Case::Upper), centre(1600.0)).at(0.5, 0.5)
                .reveal(rv(RevealKind::Fade, TextUnit::Character).overlap(0.85).order(Order::Centre), rv(RevealKind::Fade, TextUnit::Character).overlap(0.85).order(Order::Centre)).reveal_timing(0.3, 1.0))
            .layer(Layer::line("rule", c("#FFFFFF"), 80.0, 2.0).at(0.5, 0.585).in_out(Property::Width, 0.0, 1.0, 0.5, Ease::Out).track(Property::Opacity, Phase::Intro, &[(0.0, 0.0, Ease::Linear), (0.9, 0.0, Ease::Linear), (1.2, 1.0, Ease::Out)]))
            .layer(Layer::text("subtitle", "subtitle", sti("Playfair Display", 400, 38.0, c("#E8E2D0")), centre(1000.0)).at(0.5, 0.64)
                .reveal(rv(RevealKind::Rise, TextUnit::Line).distance(0.4), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(1.0, 0.6)),
    );
    v.push(
        Scene::new("title.retro-sunset", "Retro Sunset", Title, 5.0, 1.4, 0.8)
            .tags("80s, synth, gradient, stripes")
            .field("title", "Title", "OUTRUN")
            .field("subtitle", "Subtitle", "1986")
            .layer(Layer::rect("stripe1", c("#FF6B9D"), 1300.0, 10.0, 0.0).at(0.5, 0.60).in_out(Property::Width, 0.0, 1.0, 0.6, Ease::Out))
            .layer(Layer::rect("stripe2", c("#7B2CBF"), 1100.0, 8.0, 0.0).at(0.5, 0.63).in_out(Property::Width, 0.0, 1.0, 0.7, Ease::Out))
            .layer(Layer::rect("stripe3", c("#4CC9F0"), 900.0, 6.0, 0.0).at(0.5, 0.655).in_out(Property::Width, 0.0, 1.0, 0.8, Ease::Out))
            .layer(Layer::text("title", "title", st("Syne", 800, 150.0, WHITE).case(Case::Upper).letter_spacing(6.0).with_stroke(Stroke::new(c("#1A0033"), 2.5)).with_glow(Glow::new(c("#FF2E9A"), 0.6, 14.0)), centre(1600.0)).at(0.5, 0.45)
                .reveal(rv(RevealKind::Zoom, TextUnit::Line).ease(Ease::Bounce).distance(1.0), rv(RevealKind::Zoom, TextUnit::Line).ease(Ease::In)).reveal_timing(0.0, 0.9))
            .layer(Layer::text("subtitle", "subtitle", st("Monoton", 400, 44.0, c("#FFE066")).with_glow(Glow::new(c("#FFE066"), 0.5, 8.0)).letter_spacing(14.0), centre(800.0)).at(0.5, 0.72)
                .reveal(rv(RevealKind::Fade, TextUnit::Line), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.9, 0.5)),
    );
    v.push(
        Scene::new("title.vertical-stack", "Vertical Stack", Title, 5.0, 1.5, 0.8)
            .tags("vertical, 9:16, stacked, words")
            .field("line1", "Line 1", "RUN")
            .field("line2", "Line 2", "FASTER")
            .field("line3", "Line 3", "TODAY")
            .layer(Layer::text("l1", "line1", st("Bebas Neue", 400, 180.0, WHITE), centre(1000.0)).at(0.5, 0.36).reveal(rv(RevealKind::SlideLeft, TextUnit::Line).distance(1.0), rv(RevealKind::SlideLeft, TextUnit::Line)).reveal_timing(0.0, 0.6))
            .layer(Layer::text("l2", "line2", st("Bebas Neue", 400, 180.0, c("#FFD23F")), centre(1000.0)).at(0.5, 0.5).reveal(rv(RevealKind::SlideRight, TextUnit::Line).distance(1.0), rv(RevealKind::SlideRight, TextUnit::Line)).reveal_timing(0.25, 0.6))
            .layer(Layer::text("l3", "line3", st("Bebas Neue", 400, 180.0, WHITE).with_stroke(Stroke::new(WHITE, 2.0)), centre(1000.0)).at(0.5, 0.64).reveal(rv(RevealKind::SlideLeft, TextUnit::Line).distance(1.0), rv(RevealKind::SlideLeft, TextUnit::Line)).reveal_timing(0.5, 0.6)),
    );

    // ---------------------------------------------------------- Lower thirds
    v.push(
        Scene::new("lower.classic-bar", "Classic Bar", LowerThird, 6.0, 1.2, 0.7)
            .tags("name, role, bar, broadcast")
            .field("name", "Name", "Maya Chen")
            .field("role", "Role", "Lead designer, Studio")
            .layer(Layer::rect("accent", c("#8B6CFF"), 12.0, 120.0, 0.0).at(0.08, 0.82).anchor(0.0, 0.5).in_out(Property::Height, 0.0, 1.0, 0.4, Ease::Out))
            .layer(Layer::rect("bar", c("#0F172AE0"), 620.0, 120.0, 0.0).at(0.08, 0.82).anchor(0.0, 0.5).track(Property::X, Phase::Intro, &[(0.0, 12.0, Ease::Linear)]).in_out(Property::Width, 0.0, 1.0, 0.5, Ease::Out))
            .layer(Layer::text("name", "name", st("Inter", 700, 44.0, WHITE), left(580.0)).at(0.08, 0.795).anchor(0.0, 0.5).track(Property::X, Phase::Intro, &[(0.0, 34.0, Ease::Linear)])
                .reveal(rv(RevealKind::SlideLeft, TextUnit::Line).distance(0.5), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.3, 0.5))
            .layer(Layer::text("role", "role", st("Inter", 400, 26.0, c("#C7C9D3")), left(580.0)).at(0.08, 0.85).anchor(0.0, 0.5).track(Property::X, Phase::Intro, &[(0.0, 34.0, Ease::Linear)])
                .reveal(rv(RevealKind::SlideLeft, TextUnit::Line).distance(0.5), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.45, 0.5)),
    );
    v.push(
        Scene::new("lower.minimal-line", "Minimal Line", LowerThird, 6.0, 1.2, 0.7)
            .tags("minimal, line, clean")
            .field("name", "Name", "Jordan Ali")
            .field("role", "Role", "Photographer")
            .layer(Layer::text("name", "name", st("Inter", 600, 46.0, WHITE).with_shadow(Shadow::new(BLACK, 8.0, 0.0, 0.0).opacity(0.5)), left(700.0)).at(0.08, 0.79).anchor(0.0, 0.5)
                .reveal(rv(RevealKind::Rise, TextUnit::Word).distance(0.5), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.0, 0.6))
            .layer(Layer::line("rule", c("#FFD166"), 90.0, 3.0).at(0.08, 0.83).anchor(0.0, 0.5).in_out(Property::Width, 0.0, 1.0, 0.5, Ease::Out).track(Property::Opacity, Phase::Intro, &[(0.0, 0.0, Ease::Linear), (0.3, 1.0, Ease::Linear)]))
            .layer(Layer::text("role", "role", st("Inter", 400, 28.0, c("#D0D3DB")).letter_spacing(6.0).case(Case::Upper), left(700.0)).at(0.08, 0.868).anchor(0.0, 0.5)
                .reveal(rv(RevealKind::Fade, TextUnit::Character).overlap(0.9), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.5, 0.6)),
    );
    v.push(
        Scene::new("lower.pill", "Pill", LowerThird, 6.0, 1.0, 0.6)
            .tags("pill, rounded, friendly")
            .field("name", "Name", "Sam Okafor")
            .field("role", "Role", "Host")
            .layer(Layer::text("name", "name", st("Poppins", 700, 40.0, c("#0B0B0C")).with_background(Background::new(WHITE, 999.0, 26.0, 10.0)), left(800.0)).at(0.08, 0.79).anchor(0.0, 0.5)
                .track(Property::Scale, Phase::Intro, &[(0.0, 0.6, Ease::Linear), (0.5, 1.0, Ease::Overshoot)]).track(Property::Opacity, Phase::Intro, &[(0.0, 0.0, Ease::Linear), (0.25, 1.0, Ease::Out)])
                .track(Property::Opacity, Phase::Outro, &[(0.0, 1.0, Ease::Linear), (0.4, 0.0, Ease::In)]))
            .layer(Layer::text("role", "role", st("Poppins", 600, 26.0, WHITE).with_background(Background::new(c("#8B6CFF"), 999.0, 20.0, 6.0)), left(800.0)).at(0.08, 0.865).anchor(0.0, 0.5)
                .track(Property::Scale, Phase::Intro, &[(0.0, 0.6, Ease::Linear), (0.3, 0.6, Ease::Linear), (0.8, 1.0, Ease::Overshoot)]).track(Property::Opacity, Phase::Intro, &[(0.0, 0.0, Ease::Linear), (0.3, 0.0, Ease::Linear), (0.55, 1.0, Ease::Out)])
                .track(Property::Opacity, Phase::Outro, &[(0.0, 1.0, Ease::Linear), (0.3, 0.0, Ease::In)])),
    );
    v.push(
        Scene::new("lower.broadcast", "Broadcast", LowerThird, 6.0, 1.2, 0.8)
            .tags("news, two tone, wipe")
            .field("name", "Name", "Priya Natarajan")
            .field("role", "Role", "Correspondent · Mumbai")
            .layer(Layer::rect("name-bg", c("#D7263D"), 700.0, 84.0, 0.0).at(0.07, 0.78).anchor(0.0, 0.5).in_out(Property::WipeX, 0.0, 1.0, 0.5, Ease::Out))
            .layer(Layer::rect("role-bg", c("#F4F4F6"), 620.0, 56.0, 0.0).at(0.07, 0.845).anchor(0.0, 0.5).in_out(Property::WipeX, 0.0, 1.0, 0.5, Ease::Out).track(Property::WipeX, Phase::Intro, &[(0.0, 0.0, Ease::Linear), (0.2, 0.0, Ease::Linear), (0.7, 1.0, Ease::Out)]))
            .layer(Layer::text("name", "name", st("Oswald", 600, 46.0, WHITE).letter_spacing(2.0), left(660.0)).at(0.085, 0.78).anchor(0.0, 0.5)
                .reveal(rv(RevealKind::Fade, TextUnit::Line), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.3, 0.4))
            .layer(Layer::text("role", "role", st("Inter", 600, 24.0, c("#1C1C1E")).letter_spacing(3.0).case(Case::Upper), left(600.0)).at(0.085, 0.845).anchor(0.0, 0.5)
                .reveal(rv(RevealKind::Fade, TextUnit::Line), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.55, 0.4)),
    );
    v.push(
        Scene::new("lower.corner-tag", "Corner Tag", LowerThird, 5.0, 0.9, 0.5)
            .tags("small, tag, vlog")
            .field("name", "Name", "@maya.makes")
            .field("role", "Role", "Lisbon")
            .layer(Layer::text("name", "name", st("DM Sans", 700, 34.0, WHITE).with_background(Background::new(c("#000000B3"), 10.0, 18.0, 8.0)), left(600.0)).at(0.06, 0.88).anchor(0.0, 0.5)
                .reveal(rv(RevealKind::Drop, TextUnit::Line).distance(0.8).ease(Ease::Overshoot), rv(RevealKind::Rise, TextUnit::Line).distance(0.5)).reveal_timing(0.0, 0.5))
            .layer(Layer::text("role", "role", st("DM Sans", 500, 24.0, c("#FFD166")).with_background(Background::new(c("#000000B3"), 8.0, 14.0, 5.0)), left(600.0)).at(0.06, 0.935).anchor(0.0, 0.5)
                .reveal(rv(RevealKind::Drop, TextUnit::Line).distance(0.8).ease(Ease::Overshoot), rv(RevealKind::Rise, TextUnit::Line).distance(0.5)).reveal_timing(0.2, 0.5)),
    );
    v.push(
        Scene::new("lower.outline-underline", "Outline", LowerThird, 6.0, 1.2, 0.7)
            .tags("outline, underline, bold")
            .field("name", "Name", "KAI TANAKA")
            .field("role", "Role", "Skater · Tokyo")
            .layer(Layer::text("name", "name", st("Montserrat", 800, 56.0, Rgba::TRANSPARENT).case(Case::Upper), left(800.0)).at(0.08, 0.79).anchor(0.0, 0.5)
                .reveal(rv(RevealKind::SlideLeft, TextUnit::Character).overlap(0.85).distance(0.3), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.0, 0.8))
            .layer(Layer::text("name-outline", "name", TextStyle { fill: Fill::None, ..st("Montserrat", 800, 56.0, WHITE) }.with_stroke(Stroke::new(WHITE, 1.6)).case(Case::Upper), left(800.0)).at(0.08, 0.79).anchor(0.0, 0.5)
                .reveal(rv(RevealKind::SlideLeft, TextUnit::Character).overlap(0.85).distance(0.3), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.0, 0.8))
            .layer(Layer::line("rule", WHITE, 300.0, 4.0).at(0.08, 0.828).anchor(0.0, 0.5).in_out(Property::Width, 0.0, 1.0, 0.6, Ease::Out).track(Property::Opacity, Phase::Intro, &[(0.0, 0.0, Ease::Linear), (0.4, 0.0, Ease::Linear), (0.5, 1.0, Ease::Linear)]))
            .layer(Layer::text("role", "role", st("Montserrat", 500, 26.0, c("#E6E6E6")).letter_spacing(4.0), left(800.0)).at(0.08, 0.865).anchor(0.0, 0.5)
                .reveal(rv(RevealKind::Fade, TextUnit::Word).overlap(0.7), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.6, 0.5)),
    );
    v.push(
        Scene::new("lower.social-handle", "Social Handle", LowerThird, 5.0, 1.0, 0.6)
            .tags("handle, social, dot")
            .field("handle", "Handle", "@studio.app")
            .field("platform", "Platform", "follow for more")
            .layer(Layer::ellipse("dot", c("#5CFF8A"), 22.0, 22.0).at(0.07, 0.838).anchor(0.5, 0.5).in_out(Property::Scale, 0.0, 1.0, 0.4, Ease::Overshoot).motion(LoopMotion::new(LoopKind::Pulse, TextUnit::Line)))
            .layer(Layer::text("handle", "handle", st("Inter", 700, 40.0, WHITE), left(700.0)).at(0.09, 0.838).anchor(0.0, 0.5)
                .reveal(rv(RevealKind::SlideLeft, TextUnit::Line).distance(0.4), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.2, 0.5))
            .layer(Layer::text("platform", "platform", st("Inter", 400, 24.0, c("#BDBDC7")), left(700.0)).at(0.09, 0.885).anchor(0.0, 0.5)
                .reveal(rv(RevealKind::Fade, TextUnit::Line), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.5, 0.4)),
    );
    v.push(
        Scene::new("lower.vertical-accent", "Vertical Accent", LowerThird, 6.0, 1.2, 0.7)
            .tags("bar, serif, elegant")
            .field("name", "Name", "Elena Rossi")
            .field("role", "Role", "Winemaker, Piedmont")
            .layer(Layer::rect("bar", c("#FFD166"), 8.0, 110.0, 4.0).at(0.08, 0.82).anchor(0.0, 1.0).in_out(Property::Height, 0.0, 1.0, 0.5, Ease::Out))
            .layer(Layer::text("name", "name", st("Fraunces", 600, 50.0, WHITE), left(800.0)).at(0.1, 0.78).anchor(0.0, 0.5)
                .reveal(rv(RevealKind::Rise, TextUnit::Line).distance(0.3), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.3, 0.5))
            .layer(Layer::text("role", "role", sti("Fraunces", 400, 28.0, c("#EDE6D6")), left(800.0)).at(0.1, 0.835).anchor(0.0, 0.5)
                .reveal(rv(RevealKind::Rise, TextUnit::Line).distance(0.3), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.5, 0.5)),
    );

    // -------------------------------------------------------------- Callouts
    v.push(
        Scene::new("callout.arrow-note", "Arrow Note", Callout, 4.0, 0.8, 0.5)
            .tags("hand, note, arrow, point")
            .field("note", "Note", "look at this!")
            .layer(Layer::line("arrow", c("#FFE234"), 160.0, 6.0).at(0.5, 0.56).anchor(0.0, 0.5).rotated(35.0).in_out(Property::Width, 0.0, 1.0, 0.35, Ease::Out))
            .layer(Layer::text("note", "note", st("Permanent Marker", 400, 60.0, c("#FFE234")).with_shadow(Shadow::new(BLACK, 3.0, 3.0, 90.0).opacity(0.6)), centre(700.0)).at(0.5, 0.5).rotated(-6.0)
                .reveal(rv(RevealKind::Pop, TextUnit::Word).overlap(0.5), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.25, 0.5).motion(LoopMotion::new(LoopKind::Sway, TextUnit::Line).speed(0.6).amount(0.3))),
    );
    v.push(
        Scene::new("callout.number-badge", "Number Badge", Callout, 4.5, 0.9, 0.5)
            .tags("number, step, badge")
            .field("number", "Number", "1")
            .field("label", "Label", "Mix the dry ingredients")
            .layer(Layer::ellipse("badge", c("#8B6CFF"), 96.0, 96.0).at(0.3, 0.5).in_out(Property::Scale, 0.0, 1.0, 0.45, Ease::Overshoot))
            .layer(Layer::text("number", "number", st("Unbounded", 800, 48.0, WHITE), centre(96.0)).at(0.3, 0.5).in_out(Property::Scale, 0.0, 1.0, 0.45, Ease::Overshoot))
            .layer(Layer::text("label", "label", st("Inter", 600, 42.0, WHITE).with_background(Background::new(c("#000000B3"), 12.0, 24.0, 12.0)), left(900.0)).at(0.335, 0.5).anchor(0.0, 0.5)
                .reveal(rv(RevealKind::SlideLeft, TextUnit::Line).distance(0.6), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.3, 0.5)),
    );
    v.push(
        Scene::new("callout.highlight", "Highlight", Callout, 4.5, 1.0, 0.5)
            .tags("highlighter, marker, wipe")
            .field("text", "Text", "this changes everything")
            .layer(Layer::text("text", "text", st("Poppins", 700, 64.0, c("#1A1A1A")).with_background(Background::new(c("#FFE234"), 6.0, 16.0, 2.0).per_line()), centre(1200.0)).at(0.5, 0.5).rotated(-2.0)
                .in_out(Property::WipeX, 0.0, 1.0, 0.7, Ease::InOut)),
    );
    v.push(
        Scene::new("callout.speech-bubble", "Speech Bubble", Callout, 4.0, 0.8, 0.5)
            .tags("bubble, speech, comic")
            .field("text", "Text", "wait, what?")
            .layer(Layer::rect("tail", WHITE, 40.0, 40.0, 4.0).at(0.46, 0.585).rotated(45.0).in_out(Property::Scale, 0.0, 1.0, 0.4, Ease::Overshoot))
            .layer(Layer::text("text", "text", st("Nunito", 900, 56.0, c("#1F1F1F")).with_background(Background::new(WHITE, 28.0, 34.0, 18.0)), centre(900.0)).at(0.5, 0.5)
                .in_out(Property::Scale, 0.0, 1.0, 0.4, Ease::Overshoot).reveal(rv(RevealKind::Pop, TextUnit::Word).overlap(0.6), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.2, 0.4)),
    );
    v.push(
        Scene::new("callout.price-sticker", "Price Sticker", Callout, 4.0, 0.8, 0.5)
            .tags("price, sticker, shop")
            .field("price", "Price", "$19.99")
            .field("label", "Label", "today only")
            .layer(Layer::ellipse("disc", c("#D7263D"), 300.0, 300.0).at(0.5, 0.5).rotated(-8.0).in_out(Property::Scale, 0.0, 1.0, 0.5, Ease::Elastic))
            .layer(Layer::text("price", "price", st("Rubik", 800, 72.0, WHITE), centre(280.0)).at(0.5, 0.48).rotated(-8.0).in_out(Property::Scale, 0.0, 1.0, 0.5, Ease::Elastic))
            .layer(Layer::text("label", "label", st("Rubik", 600, 26.0, c("#FFD9DE")).case(Case::Upper).letter_spacing(6.0), centre(280.0)).at(0.5, 0.555).rotated(-8.0)
                .reveal(rv(RevealKind::Fade, TextUnit::Line), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.4, 0.3)),
    );
    v.push(
        Scene::new("callout.step-label", "Step Label", Callout, 4.5, 0.9, 0.5)
            .tags("step, tutorial, label")
            .field("step", "Step", "STEP 02")
            .field("text", "Text", "Add the lemon zest")
            .layer(Layer::text("step", "step", st("Inter", 800, 24.0, c("#0B0B0C")).letter_spacing(12.0).with_background(Background::new(c("#5CFF8A"), 4.0, 14.0, 6.0)), left(400.0)).at(0.08, 0.14).anchor(0.0, 0.5)
                .reveal(rv(RevealKind::SlideLeft, TextUnit::Line).distance(0.6), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.0, 0.4))
            .layer(Layer::text("text", "text", st("Inter", 600, 48.0, WHITE).with_shadow(Shadow::new(BLACK, 8.0, 0.0, 0.0).opacity(0.6)), left(1200.0)).at(0.08, 0.205).anchor(0.0, 0.5)
                .reveal(rv(RevealKind::Rise, TextUnit::Word).distance(0.5).overlap(0.75), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.25, 0.6)),
    );

    // ------------------------------------------------------- Calls to action
    v.push(
        Scene::new("cta.subscribe", "Subscribe Button", CallToAction, 4.0, 0.8, 0.5)
            .tags("subscribe, button, bell, youtube")
            .field("text", "Button", "SUBSCRIBE")
            .layer(Layer::text("button", "text", st("Inter", 800, 44.0, WHITE).letter_spacing(4.0).case(Case::Upper).with_background(Background::new(c("#E62117"), 10.0, 34.0, 16.0)), centre(500.0)).at(0.47, 0.5)
                .in_out(Property::Scale, 0.0, 1.0, 0.45, Ease::Overshoot).motion(LoopMotion::new(LoopKind::Pulse, TextUnit::Line).speed(0.8).amount(0.15)))
            .layer(Layer::fixed_text("bell", "🔔", st("Inter", 700, 60.0, WHITE), centre(120.0)).at(0.63, 0.5)
                .in_out(Property::Scale, 0.0, 1.0, 0.45, Ease::Overshoot).track(Property::Scale, Phase::Intro, &[(0.0, 0.0, Ease::Linear), (0.3, 0.0, Ease::Linear), (0.75, 1.0, Ease::Overshoot)])
                .motion(LoopMotion::new(LoopKind::Wobble, TextUnit::Line).speed(1.2).amount(0.8))),
    );
    v.push(
        Scene::new("cta.like-comment-share", "Like · Comment · Share", CallToAction, 4.5, 1.2, 0.6)
            .tags("like, comment, share, pills")
            .field("a", "First", "👍 Like")
            .field("b", "Second", "💬 Comment")
            .field("c", "Third", "↗ Share")
            .layer(Layer::text("a", "a", st("Inter", 700, 36.0, c("#0B0B0C")).with_background(Background::new(WHITE, 999.0, 26.0, 12.0)), centre(400.0)).at(0.3, 0.5)
                .track(Property::Scale, Phase::Intro, &[(0.0, 0.0, Ease::Linear), (0.45, 1.0, Ease::Overshoot)]).track(Property::Scale, Phase::Outro, &[(0.0, 1.0, Ease::Linear), (0.4, 0.0, Ease::In)]))
            .layer(Layer::text("b", "b", st("Inter", 700, 36.0, c("#0B0B0C")).with_background(Background::new(WHITE, 999.0, 26.0, 12.0)), centre(400.0)).at(0.5, 0.5)
                .track(Property::Scale, Phase::Intro, &[(0.0, 0.0, Ease::Linear), (0.25, 0.0, Ease::Linear), (0.7, 1.0, Ease::Overshoot)]).track(Property::Scale, Phase::Outro, &[(0.0, 1.0, Ease::Linear), (0.1, 1.0, Ease::Linear), (0.5, 0.0, Ease::In)]))
            .layer(Layer::text("c", "c", st("Inter", 700, 36.0, c("#0B0B0C")).with_background(Background::new(WHITE, 999.0, 26.0, 12.0)), centre(400.0)).at(0.7, 0.5)
                .track(Property::Scale, Phase::Intro, &[(0.0, 0.0, Ease::Linear), (0.5, 0.0, Ease::Linear), (0.95, 1.0, Ease::Overshoot)]).track(Property::Scale, Phase::Outro, &[(0.0, 1.0, Ease::Linear), (0.2, 1.0, Ease::Linear), (0.6, 0.0, Ease::In)])),
    );
    v.push(
        Scene::new("cta.follow", "Follow Handle", CallToAction, 4.0, 0.9, 0.5)
            .tags("follow, handle, glass")
            .field("handle", "Handle", "@studio.app")
            .field("line", "Line", "follow for part two")
            .layer(Layer::text("handle", "handle", st("DM Sans", 700, 48.0, WHITE).with_background(Background::new(c("#FFFFFF33"), 999.0, 30.0, 14.0)).with_stroke(Stroke::new(c("#FFFFFF40"), 0.6)), centre(700.0)).at(0.5, 0.47)
                .reveal(rv(RevealKind::Rise, TextUnit::Line).distance(0.6).ease(Ease::Overshoot), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.0, 0.5))
            .layer(Layer::text("line", "line", st("DM Sans", 500, 30.0, c("#E0E0E6")), centre(700.0)).at(0.5, 0.56)
                .reveal(rv(RevealKind::Rise, TextUnit::Word).distance(0.4).overlap(0.7), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.3, 0.5)),
    );
    v.push(
        Scene::new("cta.link-in-bio", "Link in Bio", CallToAction, 4.0, 0.8, 0.5)
            .tags("link, bio, arrow")
            .field("text", "Text", "Link in bio ⬆️")
            .layer(Layer::text("text", "text", st("Montserrat", 800, 54.0, WHITE).with_stroke(Stroke::new(BLACK, 3.0)), centre(900.0)).at(0.5, 0.5)
                .reveal(rv(RevealKind::Bounce, TextUnit::Word).overlap(0.6), rv(RevealKind::Fade, TextUnit::Line)).motion(LoopMotion::new(LoopKind::Wave, TextUnit::Word).speed(0.9).amount(0.5))),
    );
    v.push(
        Scene::new("cta.swipe", "Swipe", CallToAction, 3.5, 0.6, 0.4)
            .tags("swipe, story, wave")
            .field("text", "Text", "swipe up ↑")
            .layer(Layer::text("text", "text", st("Poppins", 700, 46.0, WHITE).with_shadow(Shadow::new(BLACK, 6.0, 2.0, 90.0).opacity(0.6)), centre(700.0)).at(0.5, 0.86)
                .reveal(rv(RevealKind::Rise, TextUnit::Character).overlap(0.85).distance(0.5), rv(RevealKind::Fade, TextUnit::Line)).motion(LoopMotion::new(LoopKind::Wave, TextUnit::Character).speed(1.0).amount(0.6))),
    );
    v.push(
        Scene::new("cta.comment-below", "Comment Below", CallToAction, 4.0, 0.8, 0.5)
            .tags("comment, question, pulse")
            .field("question", "Question", "Which one would you pick?")
            .field("hint", "Hint", "comment below 👇")
            .layer(Layer::text("question", "question", st("Inter", 700, 52.0, WHITE).with_background(Background::new(c("#0F172AE0"), 14.0, 28.0, 14.0)), centre(1100.0)).at(0.5, 0.46)
                .reveal(rv(RevealKind::Pop, TextUnit::Line), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.0, 0.5))
            .layer(Layer::text("hint", "hint", st("Inter", 600, 34.0, c("#FFD166")), centre(1100.0)).at(0.5, 0.56)
                .reveal(rv(RevealKind::Fade, TextUnit::Line), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.4, 0.4).motion(LoopMotion::new(LoopKind::Pulse, TextUnit::Line).speed(0.7).amount(0.3))),
    );

    // ------------------------------------------------------------- End cards
    v.push(
        Scene::new("end.thanks", "Thanks for Watching", EndCard, 8.0, 1.5, 0.8)
            .tags("end, thanks, thumbnails")
            .field("title", "Title", "Thanks for watching")
            .field("cta", "Button", "Subscribe")
            .layer(Layer::rect("thumb-a", c("#FFFFFF1F"), 560.0, 315.0, 14.0).at(0.3, 0.56).in_out(Property::Scale, 0.9, 1.0, 0.8, Ease::Out).in_out(Property::Opacity, 0.0, 1.0, 0.8, Ease::Out))
            .layer(Layer::rect("thumb-b", c("#FFFFFF1F"), 560.0, 315.0, 14.0).at(0.7, 0.56).in_out(Property::Scale, 0.9, 1.0, 0.8, Ease::Out).in_out(Property::Opacity, 0.0, 1.0, 0.8, Ease::Out).track(Property::Opacity, Phase::Intro, &[(0.0, 0.0, Ease::Linear), (0.2, 0.0, Ease::Linear), (1.0, 1.0, Ease::Out)]))
            .layer(Layer::text("title", "title", st("Inter", 700, 64.0, WHITE), centre(1400.0)).at(0.5, 0.2)
                .reveal(rv(RevealKind::Rise, TextUnit::Word).distance(0.5), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.0, 0.8))
            .layer(Layer::text("cta", "cta", st("Inter", 800, 34.0, WHITE).case(Case::Upper).letter_spacing(4.0).with_background(Background::new(c("#E62117"), 10.0, 30.0, 14.0)), centre(500.0)).at(0.5, 0.87)
                .track(Property::Scale, Phase::Intro, &[(0.0, 0.0, Ease::Linear), (0.8, 0.0, Ease::Linear), (1.3, 1.0, Ease::Overshoot)]).track(Property::Scale, Phase::Outro, &[(0.0, 1.0, Ease::Linear), (0.4, 0.0, Ease::In)])),
    );
    v.push(
        Scene::new("end.up-next", "Up Next", EndCard, 8.0, 7.4, 0.6)
            .tags("next, progress, bar")
            .field("label", "Label", "UP NEXT")
            .field("title", "Title", "How I built my desk setup")
            .layer(Layer::text("label", "label", st("Inter", 700, 24.0, c("#0B0B0C")).letter_spacing(10.0).with_background(Background::new(c("#FFD166"), 4.0, 14.0, 6.0)), left(600.0)).at(0.08, 0.78).anchor(0.0, 0.5)
                .reveal(rv(RevealKind::SlideLeft, TextUnit::Line), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.0, 0.5))
            .layer(Layer::text("title", "title", st("Inter", 600, 46.0, WHITE), left(1300.0)).at(0.08, 0.84).anchor(0.0, 0.5)
                .reveal(rv(RevealKind::Rise, TextUnit::Word).distance(0.4), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.3, 0.7))
            .layer(Layer::line("track", c("#FFFFFF33"), 1600.0, 6.0).at(0.08, 0.9).anchor(0.0, 0.5).track(Property::Opacity, Phase::Intro, &[(0.0, 0.0, Ease::Linear), (0.6, 1.0, Ease::Linear)]))
            .layer(Layer::line("progress", c("#FFD166"), 1600.0, 6.0).at(0.08, 0.9).anchor(0.0, 0.5).track(Property::Width, Phase::Intro, &[(0.6, 0.0, Ease::Linear), (7.4, 1.0, Ease::Linear)]).track(Property::Width, Phase::Outro, &[(0.0, 1.0, Ease::Linear)])),
    );
    v.push(
        Scene::new("end.credits", "Credits", EndCard, 8.0, 3.0, 1.0)
            .tags("credits, roll, lines")
            .field("credits", "Credits", "Directed by Maya Chen\nCamera · Jordan Ali\nMusic · Sam Okafor\nEdited in Studio")
            .layer(Layer::text("credits", "credits", st("Inter", 500, 34.0, c("#E8E8EC")).letter_spacing(8.0).line_spacing(60.0).case(Case::Upper), centre(1400.0)).at(0.5, 0.5)
                .reveal(rv(RevealKind::Rise, TextUnit::Line).overlap(0.5).distance(0.6), rv(RevealKind::Fade, TextUnit::Line).overlap(0.6)).reveal_timing(0.0, 2.6)),
    );
    v.push(
        Scene::new("end.social-grid", "Social Grid", EndCard, 7.0, 1.6, 0.8)
            .tags("social, handles, dots")
            .field("a", "First", "studio.app")
            .field("b", "Second", "@studio")
            .field("c", "Third", "studio.video")
            .layer(Layer::ellipse("dot-a", c("#E1306C"), 26.0, 26.0).at(0.33, 0.5).track(Property::Scale, Phase::Intro, &[(0.0, 0.0, Ease::Linear), (0.4, 1.0, Ease::Overshoot)]))
            .layer(Layer::text("a", "a", st("Inter", 600, 38.0, WHITE), left(500.0)).at(0.35, 0.5).anchor(0.0, 0.5).reveal(rv(RevealKind::SlideLeft, TextUnit::Line).distance(0.4), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.2, 0.5))
            .layer(Layer::ellipse("dot-b", c("#1DA1F2"), 26.0, 26.0).at(0.33, 0.59).track(Property::Scale, Phase::Intro, &[(0.0, 0.0, Ease::Linear), (0.3, 0.0, Ease::Linear), (0.7, 1.0, Ease::Overshoot)]))
            .layer(Layer::text("b", "b", st("Inter", 600, 38.0, WHITE), left(500.0)).at(0.35, 0.59).anchor(0.0, 0.5).reveal(rv(RevealKind::SlideLeft, TextUnit::Line).distance(0.4), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.5, 0.5))
            .layer(Layer::ellipse("dot-c", c("#FF0000"), 26.0, 26.0).at(0.33, 0.68).track(Property::Scale, Phase::Intro, &[(0.0, 0.0, Ease::Linear), (0.6, 0.0, Ease::Linear), (1.0, 1.0, Ease::Overshoot)]))
            .layer(Layer::text("c", "c", st("Inter", 600, 38.0, WHITE), left(500.0)).at(0.35, 0.68).anchor(0.0, 0.5).reveal(rv(RevealKind::SlideLeft, TextUnit::Line).distance(0.4), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.8, 0.5))
            .layer(Layer::text("title", "title", st("Inter", 800, 56.0, WHITE).letter_spacing(2.0), centre(1200.0)).at(0.5, 0.3).reveal(rv(RevealKind::Fade, TextUnit::Line), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.0, 0.5))
            .field("title", "Title", "Find me here"),
    );
    v.push(
        Scene::new("end.wordmark", "Wordmark", EndCard, 6.0, 1.6, 0.9)
            .tags("logo, wordmark, tagline")
            .field("brand", "Brand", "STUDIO")
            .field("tagline", "Tagline", "made with care")
            .layer(Layer::text("brand", "brand", st("Unbounded", 800, 130.0, WHITE).letter_spacing(10.0).case(Case::Upper), centre(1600.0)).at(0.5, 0.47)
                .reveal(rv(RevealKind::Zoom, TextUnit::Line).distance(0.3).ease(Ease::Out), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.0, 1.0)
                .track(Property::Scale, Phase::Intro, &[(1.0, 1.0, Ease::Linear), (6.0, 1.04, Ease::Linear)]))
            .layer(Layer::text("tagline", "tagline", st("Inter", 400, 32.0, c("#C9C9D4")).letter_spacing(16.0), centre(1000.0)).at(0.5, 0.6)
                .reveal(rv(RevealKind::Fade, TextUnit::Character).overlap(0.9).order(Order::Centre), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.8, 0.8)),
    );

    // ---------------------------------------------------------------- Quotes
    v.push(
        Scene::new("quote.serif-card", "Serif Quote", Quote, 7.0, 2.0, 1.0)
            .tags("quote, serif, attribution")
            .field("quote", "Quote", "Simplicity is the ultimate sophistication.")
            .field("who", "Attribution", "— Leonardo da Vinci")
            .layer(Layer::fixed_text("mark", "“", st("Playfair Display", 900, 220.0, c("#FFFFFF2E")), centre(200.0)).at(0.5, 0.33).in_out(Property::Opacity, 0.0, 1.0, 1.0, Ease::Out))
            .layer(Layer::text("quote", "quote", sti("Lora", 500, 60.0, c("#FFF8EE")).line_spacing(14.0).with_shadow(Shadow::new(BLACK, 10.0, 0.0, 0.0).opacity(0.6)), centre(1300.0)).at(0.5, 0.5)
                .reveal(rv(RevealKind::Fade, TextUnit::Word).overlap(0.75), rv(RevealKind::Fade, TextUnit::Word).overlap(0.85)).reveal_timing(0.2, 1.4))
            .layer(Layer::text("who", "who", st("Inter", 500, 28.0, c("#D8D2C4")).letter_spacing(10.0), centre(1000.0)).at(0.5, 0.67)
                .reveal(rv(RevealKind::Rise, TextUnit::Line).distance(0.4), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(1.3, 0.6)),
    );
    v.push(
        Scene::new("quote.pop-words", "Pop Words", Quote, 5.0, 1.6, 0.7)
            .tags("words, pop, colour, bold")
            .field("quote", "Quote", "Done is better than perfect")
            .layer(Layer::text("quote", "quote", st("Unbounded", 800, 84.0, WHITE).with_cycle(ColourCycle::new(&[c("#FFFFFF"), c("#FFD166"), c("#FFFFFF"), c("#5CFF8A"), c("#FFFFFF")], TextUnit::Word)).with_stroke(Stroke::new(BLACK, 2.5)).line_spacing(10.0), centre(1300.0)).at(0.5, 0.5)
                .reveal(rv(RevealKind::Pop, TextUnit::Word).overlap(0.35).ease(Ease::Overshoot), rv(RevealKind::Fade, TextUnit::Word).overlap(0.8)).reveal_timing(0.0, 1.5)),
    );
    v.push(
        Scene::new("quote.typed-line", "Typed Line", Quote, 6.0, 2.4, 0.7)
            .tags("type, mono, underline")
            .field("quote", "Quote", "write it down before it disappears")
            .layer(Layer::text("quote", "quote", st("Courier Prime", 700, 52.0, c("#F3EBD3")), centre(1300.0)).at(0.5, 0.5)
                .reveal(rv(RevealKind::Type, TextUnit::Character).overlap(0.0), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.0, 2.0))
            .layer(Layer::line("rule", c("#F3EBD3"), 1300.0, 3.0).at(0.5, 0.56).in_out(Property::Width, 0.0, 1.0, 2.0, Ease::Linear)),
    );

    // ---------------------------------------------------------------- Labels
    v.push(
        Scene::new("label.location", "Location Pin", Label, 4.0, 0.7, 0.5)
            .tags("location, pin, travel")
            .field("place", "Place", "📍 Lisbon, Portugal")
            .layer(Layer::text("place", "place", st("Inter", 600, 34.0, WHITE).with_background(Background::new(c("#000000B3"), 999.0, 22.0, 10.0)), left(700.0)).at(0.06, 0.1).anchor(0.0, 0.5)
                .reveal(rv(RevealKind::SlideLeft, TextUnit::Line).distance(0.6).ease(Ease::Overshoot), rv(RevealKind::SlideLeft, TextUnit::Line).distance(0.6))),
    );
    v.push(
        Scene::new("label.date-stamp", "Date Stamp", Label, 4.0, 1.0, 0.4)
            .tags("date, mono, camera")
            .field("date", "Date", "2026.10.23  14:02")
            .layer(Layer::text("date", "date", st("VT323", 400, 44.0, c("#FFB000")).with_glow(Glow::new(c("#FFB000"), 0.4, 3.0)), LayoutParams::default().align(Align::Right).wrap(600.0)).at(0.94, 0.9).anchor(1.0, 0.5)
                .reveal(rv(RevealKind::Type, TextUnit::Character).overlap(0.0), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.0, 0.8)),
    );
    v.push(
        Scene::new("label.chapter", "Chapter", Label, 4.5, 1.0, 0.6)
            .tags("chapter, caps, rule")
            .field("number", "Number", "CHAPTER 03")
            .field("title", "Title", "The crossing")
            .layer(Layer::text("number", "number", st("Inter", 600, 22.0, c("#D9D9D9")).letter_spacing(28.0), left(600.0)).at(0.08, 0.12).anchor(0.0, 0.5)
                .reveal(rv(RevealKind::Fade, TextUnit::Character).overlap(0.9), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.0, 0.6))
            .layer(Layer::line("rule", c("#D9D9D9"), 60.0, 2.0).at(0.08, 0.155).anchor(0.0, 0.5).in_out(Property::Width, 0.0, 1.0, 0.5, Ease::Out))
            .layer(Layer::text("title", "title", st("Playfair Display", 500, 46.0, WHITE), left(800.0)).at(0.08, 0.205).anchor(0.0, 0.5)
                .reveal(rv(RevealKind::Rise, TextUnit::Line).distance(0.3), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.4, 0.5)),
    );
    v.push(
        Scene::new("label.warning-tape", "Warning Tape", Label, 4.0, 0.6, 0.4)
            .tags("warning, tape, yellow")
            .field("text", "Text", "WARNING · LOUD AUDIO · WARNING · LOUD AUDIO")
            .layer(Layer::text("text", "text", st("Archivo Black", 400, 40.0, c("#111111")).letter_spacing(6.0).case(Case::Upper).with_background(Background::new(c("#FFD23F"), 0.0, 2000.0, 14.0)), centre(2400.0)).at(0.5, 0.5).rotated(-6.0)
                .in_out(Property::WipeX, 0.0, 1.0, 0.5, Ease::Out).motion(LoopMotion::new(LoopKind::Jitter, TextUnit::Line).amount(0.3))),
    );

    // ---------------------------------------------------------------- Social
    v.push(
        Scene::new("social.hook", "Hook", Social, 4.0, 1.0, 0.5)
            .tags("hook, reels, bold, box")
            .field("text", "Text", "I tried this for 30 days")
            .layer(Layer::text("text", "text", st("Montserrat", 800, 68.0, WHITE).with_stroke(Stroke::new(BLACK, 3.5)).with_shadow(Shadow::new(BLACK, 2.0, 4.0, 90.0).opacity(0.6)).line_spacing(8.0), centre(900.0)).at(0.5, 0.5)
                .reveal(rv(RevealKind::Pop, TextUnit::Word).overlap(0.45).ease(Ease::Overshoot), rv(RevealKind::Fade, TextUnit::Word).overlap(0.8))),
    );
    v.push(
        Scene::new("social.shimmer-line", "Shimmer Line", Social, 5.0, 0.8, 0.5)
            .tags("shimmer, sweep, highlight")
            .field("text", "Text", "new drop every friday")
            .layer(Layer::text("text", "text", st("Space Grotesk", 700, 64.0, c("#9A9AA8")).letter_spacing(4.0), centre(1300.0)).at(0.5, 0.5)
                .reveal(rv(RevealKind::Fade, TextUnit::Character).overlap(0.9), rv(RevealKind::Fade, TextUnit::Character).overlap(0.9))
                .motion(LoopMotion::new(LoopKind::Shimmer, TextUnit::Character).speed(0.4).amount(0.6).colour(WHITE))),
    );
    v.push(
        Scene::new("social.reaction", "Reaction", Social, 4.0, 0.8, 0.5)
            .tags("emoji, reaction, fire")
            .field("emoji", "Emoji", "🔥🔥🔥")
            .field("text", "Text", "that's wild")
            .layer(Layer::text("emoji", "emoji", st("Inter", 700, 90.0, WHITE), centre(600.0)).at(0.5, 0.44)
                .reveal(rv(RevealKind::Pop, TextUnit::Character).overlap(0.5).ease(Ease::Overshoot), rv(RevealKind::Fade, TextUnit::Line)).motion(LoopMotion::new(LoopKind::Wave, TextUnit::Character).speed(1.2).amount(0.6)))
            .layer(Layer::text("text", "text", st("Bangers", 400, 70.0, c("#FFE600")).with_stroke(Stroke::new(BLACK, 3.0)).letter_spacing(3.0), centre(900.0)).at(0.5, 0.6).rotated(-3.0)
                .reveal(rv(RevealKind::Drop, TextUnit::Line).distance(0.8).ease(Ease::Bounce), rv(RevealKind::Fade, TextUnit::Line)).reveal_timing(0.3, 0.5)),
    );
    v
}

/// Every template.
pub fn all() -> &'static [Scene] {
    static ALL: OnceLock<Vec<Scene>> = OnceLock::new();
    ALL.get_or_init(build)
}

pub fn find(id: &str) -> Option<&'static Scene> {
    all().iter().find(|s| s.id == id)
}

/// Searches names and tags.
pub fn search(query: &str) -> Vec<&'static Scene> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return all().iter().collect();
    }
    all().iter().filter(|s| s.name.to_lowercase().contains(&q) || s.tags.iter().any(|t| t.contains(&q)) || s.id.contains(&q)).collect()
}

/// Loads a scene file (JSON) from disk.
pub fn load(path: &std::path::Path) -> std::io::Result<Scene> {
    let text = std::fs::read_to_string(path)?;
    Scene::from_json(&text).map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_is_large_unique_and_valid() {
        let all = all();
        assert!(all.len() >= 40, "{} templates", all.len());
        let mut ids: Vec<&str> = all.iter().map(|s| s.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), all.len());
        for s in all {
            assert!(s.intro + s.outro <= s.duration + 1e-3, "{} phases longer than duration", s.id);
            assert!(!s.layers.is_empty());
            for l in &s.layers {
                if let crate::scene::LayerKind::Text { field: Some(f), .. } = &l.kind {
                    assert!(s.fields.iter().any(|x| &x.key == f), "{}: layer {} uses unknown field {f}", s.id, l.name);
                }
            }
        }
        for cat in TemplateCategory::ALL {
            assert!(all.iter().any(|s| s.category == cat), "{cat:?} empty");
        }
    }

    #[test]
    fn json_round_trip_every_template() {
        for s in all() {
            let back = Scene::from_json(&s.to_json()).unwrap();
            assert_eq!(&back, s, "{}", s.id);
        }
    }

    #[test]
    fn shipped_json_matches_builders() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
        for s in all() {
            let p = dir.join(format!("{}.json", s.id));
            let on_disk = match std::fs::read_to_string(&p) {
                Ok(t) => t,
                Err(_) => panic!("missing {}: run `cargo run --example gen_templates`", p.display()),
            };
            let parsed = Scene::from_json(&on_disk).unwrap();
            assert_eq!(&parsed, s, "{} differs from templates/{}.json: run gen_templates", s.id, s.id);
        }
    }
}
