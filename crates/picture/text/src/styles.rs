// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Studio's text style catalogue: ready-made looks a creator applies with
//! one click. Every style, palette and name here is our own design.
//!
//! Styles are grouped by mood rather than by technique, because that is
//! how people choose them. Each one names a bundled open-licence font; if
//! that font is missing the engine falls back along the general chain,
//! so a style never fails to render.

use std::sync::OnceLock;

use crate::font::FontRequest;
use crate::rgba::Rgba;
use crate::style::*;

/// Catalogue groups.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StyleCategory {
    Clean,
    Bold,
    Outline,
    Neon,
    Retro,
    Playful,
    Elegant,
    Cinematic,
    Social,
    Handmade,
}

impl StyleCategory {
    pub const ALL: [StyleCategory; 10] = [
        StyleCategory::Clean,
        StyleCategory::Bold,
        StyleCategory::Outline,
        StyleCategory::Neon,
        StyleCategory::Retro,
        StyleCategory::Playful,
        StyleCategory::Elegant,
        StyleCategory::Cinematic,
        StyleCategory::Social,
        StyleCategory::Handmade,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            StyleCategory::Clean => "Clean",
            StyleCategory::Bold => "Bold",
            StyleCategory::Outline => "Outline",
            StyleCategory::Neon => "Neon",
            StyleCategory::Retro => "Retro",
            StyleCategory::Playful => "Playful",
            StyleCategory::Elegant => "Elegant",
            StyleCategory::Cinematic => "Cinematic",
            StyleCategory::Social => "Social",
            StyleCategory::Handmade => "Handmade",
        }
    }
}

/// A catalogue entry.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct StylePreset {
    /// Stable id, e.g. `"clean.ink"`.
    pub id: String,
    pub name: String,
    pub category: StyleCategory,
    pub tags: Vec<String>,
    pub style: TextStyle,
    /// Text that shows the style off in a picker.
    pub sample: String,
}

fn c(hex: &str) -> Rgba {
    Rgba::hex(hex)
}

fn f(family: &str, weight: u16) -> FontRequest {
    FontRequest::new(family, weight, false)
}

fn fi(family: &str, weight: u16) -> FontRequest {
    FontRequest::new(family, weight, true)
}

fn preset(id: &str, name: &str, category: StyleCategory, tags: &str, sample: &str, style: TextStyle) -> StylePreset {
    StylePreset {
        id: id.into(),
        name: name.into(),
        category,
        tags: tags.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect(),
        style,
        sample: sample.into(),
    }
}

fn build() -> Vec<StylePreset> {
    use StyleCategory::*;
    let white = Rgba::WHITE;
    let black = Rgba::BLACK;
    let mut v = Vec::new();

    // --- Clean -----------------------------------------------------------
    v.push(preset("clean.ink", "Ink", Clean, "simple, sans, white", "Clean and simple", TextStyle::new(f("Inter", 600), 56.0, Fill::Solid(white))));
    v.push(preset("clean.ink-shadow", "Ink Shadow", Clean, "simple, readable, shadow", "Readable anywhere",
        TextStyle::new(f("Inter", 700), 56.0, Fill::Solid(white)).with_shadow(Shadow::new(black, 6.0, 2.0, 90.0).opacity(0.7))));
    v.push(preset("clean.graphite", "Graphite", Clean, "dark, minimal", "Quiet confidence",
        TextStyle::new(f("Manrope", 500), 52.0, Fill::Solid(c("#1C1C1E"))).with_background(Background::new(c("#F2F2F4"), 6.0, 18.0, 8.0))));
    v.push(preset("clean.light-air", "Light Air", Clean, "thin, airy, tracking", "slow mornings", TextStyle::new(f("Outfit", 300), 54.0, Fill::Solid(white)).letter_spacing(12.0)));
    v.push(preset("clean.mono-note", "Mono Note", Clean, "mono, code, label", "rec 00:12:04",
        TextStyle::new(f("JetBrains Mono", 500), 40.0, Fill::Solid(c("#E8E8E8"))).with_background(Background::new(c("#111111CC"), 4.0, 12.0, 6.0))));
    v.push(preset("clean.caption-pill", "Pill", Clean, "pill, label, tag", "New episode",
        TextStyle::new(f("Inter", 600), 36.0, Fill::Solid(c("#0B0B0C"))).with_background(Background::new(white, 999.0, 20.0, 8.0)).letter_spacing(2.0)));
    v.push(preset("clean.slate", "Slate", Clean, "grey, soft, editorial", "Notes from the road",
        TextStyle::new(f("Work Sans", 500), 50.0, Fill::Solid(c("#DADDE2"))).with_shadow(Shadow::new(c("#0A0E14"), 10.0, 0.0, 90.0).opacity(0.5))));

    // --- Bold ------------------------------------------------------------
    v.push(preset("bold.impact", "Impact Stack", Bold, "loud, heavy, outline", "BIG NEWS",
        TextStyle::new(f("Anton", 400), 96.0, Fill::Solid(white)).with_stroke(Stroke::new(black, 3.0)).with_shadow(Shadow::new(black, 4.0, 6.0, 60.0).opacity(0.9)).case(Case::Upper)));
    v.push(preset("bold.tall-poster", "Tall Poster", Bold, "condensed, poster, tracking", "SUMMER TOUR",
        TextStyle::new(f("Bebas Neue", 400), 110.0, Fill::Solid(c("#FFD23F"))).with_stroke(Stroke::new(c("#1A1A1A"), 2.5)).letter_spacing(6.0)));
    v.push(preset("bold.block", "Block", Bold, "heavy, box, punchy", "Watch this",
        TextStyle::new(f("Archivo Black", 400), 72.0, Fill::Solid(white)).with_background(Background::new(c("#E63946"), 0.0, 22.0, 6.0).per_line())));
    v.push(preset("bold.sunset-slab", "Sunset Slab", Bold, "gradient, warm, slab", "Golden hour",
        TextStyle::new(f("Unbounded", 800), 70.0, Fill::Gradient(Gradient::new(90.0, &[c("#FFD166"), c("#EF476F")]))).with_stroke(Stroke::new(c("#2B0A1E"), 2.0)).with_shadow(Shadow::new(c("#2B0A1E"), 2.0, 5.0, 90.0).opacity(0.6))));
    v.push(preset("bold.deep-3d", "Deep 3D", Bold, "extrusion, 3d, depth", "DROP",
        TextStyle::new(f("Rubik Mono One", 400), 90.0, Fill::Solid(c("#F8F9FA"))).with_extrusion(Extrusion::new(c("#3A86FF"), 12.0, 45.0)).case(Case::Upper)));
    v.push(preset("bold.double-cut", "Double Cut", Bold, "two strokes, sticker", "Sticker shock",
        TextStyle::new(f("Poppins", 800), 76.0, Fill::Solid(c("#FF5D8F"))).with_stroke(Stroke::new(white, 3.0)).with_stroke(Stroke::new(c("#2B2D42"), 3.0)).with_shadow(Shadow::new(c("#2B2D42"), 0.0, 5.0, 90.0).opacity(0.5))));
    v.push(preset("bold.varsity", "Varsity", Bold, "sport, team, stroke", "HOME TEAM",
        TextStyle::new(f("Black Ops One", 400), 84.0, Fill::Solid(c("#FFFFFF"))).with_stroke(Stroke::new(c("#B5121B"), 4.0)).with_stroke(Stroke::new(c("#FFFFFF"), 1.5)).case(Case::Upper)));
    v.push(preset("bold.chrome-wide", "Chrome Wide", Bold, "metal, wide, gradient", "TITANIUM",
        TextStyle::new(f("Syne", 800), 84.0, Fill::Gradient(Gradient::new(90.0, &[c("#F4F4F4"), c("#9AA0A6"), c("#FFFFFF"), c("#6B7075")]))).with_stroke(Stroke::new(c("#2E3136"), 1.5)).letter_spacing(4.0).case(Case::Upper)));

    // --- Outline ---------------------------------------------------------
    v.push(preset("outline.wire", "Wire", Outline, "hollow, thin, outline", "Hollow",
        TextStyle::new(f("Montserrat", 800), 80.0, Fill::None).with_stroke(Stroke::new(white, 1.5))));
    v.push(preset("outline.heavy-wire", "Heavy Wire", Outline, "hollow, thick", "OUTLINE",
        TextStyle::new(f("Anton", 400), 100.0, Fill::None).with_stroke(Stroke::new(c("#FFD23F"), 3.0)).case(Case::Upper)));
    v.push(preset("outline.ghost", "Ghost", Outline, "translucent, outline", "Barely there",
        TextStyle::new(f("Space Grotesk", 700), 74.0, Fill::Solid(c("#FFFFFF22"))).with_stroke(Stroke::new(white, 1.2))));
    v.push(preset("outline.marker-trace", "Marker Trace", Outline, "hand, trace", "trace it",
        TextStyle::new(f("Permanent Marker", 400), 72.0, Fill::None).with_stroke(Stroke::new(c("#FF8FAB"), 2.0)).with_shadow(Shadow::new(c("#FF8FAB"), 8.0, 0.0, 0.0).opacity(0.5))));
    v.push(preset("outline.offset-print", "Offset Print", Outline, "misregistered, print, cmyk", "OFFSET",
        TextStyle::new(f("Bricolage Grotesque", 800), 84.0, Fill::None).with_stroke(Stroke::new(c("#00B4D8"), 2.0)).with_shadow(Shadow::new(c("#F72585"), 0.0, 4.0, 45.0).opacity(1.0)).case(Case::Upper)));
    v.push(preset("outline.inline-cut", "Inline Cut", Outline, "inline, two tone", "Inline",
        TextStyle::new(f("Bungee", 400), 78.0, Fill::Solid(c("#1D3557"))).with_stroke(Stroke::new(c("#A8DADC"), 1.5)).with_stroke(Stroke::new(c("#1D3557"), 2.5))));

    // --- Neon ------------------------------------------------------------
    v.push(preset("neon.pink-sign", "Pink Sign", Neon, "glow, sign, night", "open late",
        TextStyle::new(f("Pacifico", 400), 80.0, Fill::Solid(c("#FFE6F4"))).with_glow(Glow::new(c("#FF2E9A"), 0.9, 14.0)).with_stroke(Stroke::new(c("#FF5FB5"), 1.0))));
    v.push(preset("neon.cyan-tube", "Cyan Tube", Neon, "glow, tube, arcade", "ARCADE",
        TextStyle::new(f("Monoton", 400), 90.0, Fill::Solid(c("#E6FFFF"))).with_glow(Glow::new(c("#00E5FF"), 1.0, 16.0)).case(Case::Upper)));
    v.push(preset("neon.violet-pulse", "Violet Pulse", Neon, "glow, soft, electric", "After hours",
        TextStyle::new(f("Righteous", 400), 78.0, Fill::Solid(c("#F3E8FF"))).with_glow(Glow::new(c("#8B6CFF"), 0.8, 20.0)).with_shadow(Shadow::new(c("#3A1A8C"), 12.0, 0.0, 0.0).opacity(0.8))));
    v.push(preset("neon.lime-edge", "Lime Edge", Neon, "glow, outline, acid", "LEVEL UP",
        TextStyle::new(f("Unbounded", 700), 72.0, Fill::None).with_stroke(Stroke::new(c("#C6FF00"), 1.8)).with_glow(Glow::new(c("#C6FF00"), 0.9, 10.0)).case(Case::Upper)));
    v.push(preset("neon.ember", "Ember", Neon, "fire, warm, glow", "On fire",
        TextStyle::new(f("Bangers", 400), 90.0, Fill::Gradient(Gradient::new(90.0, &[c("#FFF3B0"), c("#FF9F1C"), c("#E71D36")]))).with_glow(Glow::new(c("#FF6B00"), 0.8, 14.0)).with_stroke(Stroke::new(c("#3D0C02"), 1.5))));
    v.push(preset("neon.ice", "Ice", Neon, "cold, frost, glow", "Frost",
        TextStyle::new(f("Raleway", 800), 76.0, Fill::Gradient(Gradient::new(90.0, &[c("#FFFFFF"), c("#BDEBFF")]))).with_glow(Glow::new(c("#7FDBFF"), 0.7, 12.0)).letter_spacing(6.0)));

    // --- Retro -----------------------------------------------------------
    v.push(preset("retro.seventies", "Seventies", Retro, "groovy, warm, extrusion", "Feelin' groovy",
        TextStyle::new(f("Abril Fatface", 400), 84.0, Fill::Solid(c("#FFE8A3"))).with_extrusion(Extrusion::new(c("#D95F2B"), 7.0, 90.0)).with_stroke(Stroke::new(c("#3B1F0E"), 1.2))));
    v.push(preset("retro.vhs", "VHS", Retro, "tape, mono, scanline", "PLAY ▶",
        TextStyle::new(f("VT323", 400), 84.0, Fill::Solid(c("#F2F2F2"))).with_shadow(Shadow::new(c("#FF3B3B"), 0.0, 2.5, 0.0).opacity(0.9)).with_glow(Glow::new(c("#3BB5FF"), 0.5, 6.0)).case(Case::Upper)));
    v.push(preset("retro.arcade-8bit", "8-bit", Retro, "pixel, game, arcade", "HIGH SCORE",
        TextStyle::new(f("Press Start 2P", 400), 48.0, Fill::Solid(c("#FFD23F"))).with_extrusion(Extrusion::new(c("#1A1A2E"), 4.0, 90.0)).line_spacing(40.0)));
    v.push(preset("retro.diner", "Diner", Retro, "script, chrome, 50s", "Milkshakes",
        TextStyle::new(f("Lobster", 400), 86.0, Fill::Gradient(Gradient::new(90.0, &[c("#FFFFFF"), c("#C9D1D9")]))).with_stroke(Stroke::new(c("#C62828"), 2.5)).with_shadow(Shadow::new(c("#4A0E0E"), 2.0, 5.0, 90.0).opacity(0.7))));
    v.push(preset("retro.synth-grid", "Synth Grid", Retro, "80s, sunset, chrome", "OUTRUN",
        TextStyle::new(f("Syne", 800), 92.0, Fill::Gradient(Gradient::new(90.0, &[c("#FFE066"), c("#FF6B9D"), c("#7B2CBF")]))).with_stroke(Stroke::new(c("#1A0033"), 2.0)).with_glow(Glow::new(c("#FF2E9A"), 0.6, 12.0)).case(Case::Upper).letter_spacing(4.0)));
    v.push(preset("retro.typewriter", "Typewriter", Retro, "mono, paper, letter", "Dear reader,",
        TextStyle::new(f("Courier Prime", 700), 52.0, Fill::Solid(c("#2B2B2B"))).with_background(Background::new(c("#F3EBD3"), 2.0, 20.0, 12.0))));
    v.push(preset("retro.western", "Western", Retro, "wood, slab, saloon", "WANTED",
        TextStyle::new(f("Rye", 400), 84.0, Fill::Solid(c("#F2D29B"))).with_stroke(Stroke::new(c("#4E2A14"), 2.0)).with_extrusion(Extrusion::new(c("#8B4A1F"), 6.0, 90.0)).case(Case::Upper)));

    // --- Playful ---------------------------------------------------------
    v.push(preset("playful.bubble", "Bubble", Playful, "round, candy, sticker", "Yay!",
        TextStyle::new(f("Luckiest Guy", 400), 88.0, Fill::Solid(c("#FFFFFF"))).with_stroke(Stroke::new(c("#FF6FAE"), 4.0)).with_stroke(Stroke::new(c("#FFFFFF"), 2.0)).with_shadow(Shadow::new(c("#C2185B"), 0.0, 6.0, 90.0).opacity(0.6))));
    v.push(preset("playful.rainbow-pop", "Rainbow Pop", Playful, "rainbow, colour cycle, kids", "Rainbow",
        TextStyle::new(f("Fredoka", 700), 84.0, Fill::Solid(white)).with_cycle(ColourCycle::new(&[c("#FF595E"), c("#FFCA3A"), c("#8AC926"), c("#1982C4"), c("#6A4C93")], TextUnit::Character)).with_stroke(Stroke::new(c("#1F1F1F"), 2.5))));
    v.push(preset("playful.comic-bam", "Comic", Playful, "comic, bam, pow", "BAM!",
        TextStyle::new(f("Bangers", 400), 100.0, Fill::Solid(c("#FFE600"))).with_stroke(Stroke::new(black, 3.0)).with_extrusion(Extrusion::new(c("#D62828"), 8.0, 60.0)).case(Case::Upper).letter_spacing(3.0)));
    v.push(preset("playful.jelly", "Jelly", Playful, "gradient, soft, wobble", "jelly",
        TextStyle::new(f("Baloo 2", 800), 88.0, Fill::Gradient(Gradient::new(90.0, &[c("#FF9A8B"), c("#FF6A88"), c("#FF99AC")]))).with_stroke(Stroke::new(c("#7A1F3D"), 2.0)).with_shadow(Shadow::new(c("#7A1F3D"), 3.0, 4.0, 90.0).opacity(0.5))));
    v.push(preset("playful.confetti", "Confetti", Playful, "party, words, colour", "Party time",
        TextStyle::new(f("Chewy", 400), 80.0, Fill::Solid(white)).with_cycle(ColourCycle::new(&[c("#FFB703"), c("#8ECAE6"), c("#FB8500"), c("#219EBC")], TextUnit::Word)).with_shadow(Shadow::new(c("#023047"), 0.0, 4.0, 90.0).opacity(0.8))));
    v.push(preset("playful.sticker-white", "Sticker", Playful, "sticker, die cut, white", "cute",
        TextStyle::new(f("Nunito", 900), 76.0, Fill::Solid(c("#2D3142"))).with_stroke(Stroke::new(white, 5.0)).with_shadow(Shadow::new(black, 4.0, 3.0, 90.0).opacity(0.4))));
    v.push(preset("playful.doodle", "Doodle", Playful, "marker, hand, fun", "doodle time",
        TextStyle::new(f("Caveat Brush", 400), 84.0, Fill::Solid(c("#1B1B1B"))).with_background(Background::new(c("#FFF176"), 4.0, 14.0, 2.0).per_line())));
    v.push(preset("playful.crayon", "Crayon", Playful, "kids, crayon, soft", "Art class",
        TextStyle::new(f("Kranky", 400), 80.0, Fill::Solid(c("#FF7043"))).with_stroke(Stroke::new(c("#FFF3E0"), 2.5)).with_shadow(Shadow::new(c("#5D4037"), 2.0, 3.0, 90.0).opacity(0.5))));

    // --- Elegant ---------------------------------------------------------
    v.push(preset("elegant.serif-whisper", "Whisper", Elegant, "serif, thin, wedding", "forever & always",
        TextStyle::new(fi("Cormorant Garamond", 500), 72.0, Fill::Solid(c("#FFFBF2"))).letter_spacing(4.0)));
    v.push(preset("elegant.editorial", "Editorial", Elegant, "magazine, serif, caps", "THE INTERVIEW",
        TextStyle::new(f("Playfair Display", 700), 68.0, Fill::Solid(white)).letter_spacing(14.0).case(Case::Upper)));
    v.push(preset("elegant.gold-leaf", "Gold Leaf", Elegant, "gold, luxury, gradient", "Grand Opening",
        TextStyle::new(f("Cinzel", 700), 68.0, Fill::Gradient(Gradient::new(90.0, &[c("#FFF2B3"), c("#D4A017"), c("#FFE680"), c("#9C7A1C")]))).with_shadow(Shadow::new(c("#2B1D00"), 3.0, 2.0, 90.0).opacity(0.7)).letter_spacing(8.0)));
    v.push(preset("elegant.underscore", "Underscore", Elegant, "underline, minimal, serif", "chapter one",
        TextStyle { underline: true, ..TextStyle::new(f("DM Serif Display", 400), 64.0, Fill::Solid(c("#F5EFE6"))) }));
    v.push(preset("elegant.ivory-card", "Ivory Card", Elegant, "card, invitation, box", "You're invited",
        TextStyle::new(fi("Playfair Display", 500), 56.0, Fill::Solid(c("#3B2F2F"))).with_background(Background::new(c("#F7F1E8"), 2.0, 30.0, 18.0))));
    v.push(preset("elegant.fraunces-soft", "Soft Serif", Elegant, "serif, warm, soft shadow", "Slow living",
        TextStyle::new(f("Fraunces", 600), 70.0, Fill::Solid(c("#FFF4E6"))).with_shadow(Shadow::new(c("#3D2B1F"), 14.0, 0.0, 0.0).opacity(0.6))));
    v.push(preset("elegant.instrument", "Instrument", Elegant, "display, serif, fashion", "Collection N°4",
        TextStyle::new(f("Instrument Serif", 400), 84.0, Fill::Solid(white)).letter_spacing(-1.0)));

    // --- Cinematic -------------------------------------------------------
    v.push(preset("cinema.wide-title", "Wide Title", Cinematic, "film, title, tracking", "THE LAST LIGHT",
        TextStyle::new(f("Josefin Sans", 300), 64.0, Fill::Solid(white)).letter_spacing(40.0).case(Case::Upper)));
    v.push(preset("cinema.trailer", "Trailer", Cinematic, "trailer, heavy, condensed", "THIS SUMMER",
        TextStyle::new(f("Oswald", 600), 90.0, Fill::Solid(c("#F0EDE6"))).with_shadow(Shadow::new(black, 16.0, 0.0, 0.0).opacity(0.9)).letter_spacing(12.0).case(Case::Upper)));
    v.push(preset("cinema.noir", "Noir", Cinematic, "noir, serif, shadow", "No way out",
        TextStyle::new(f("Playfair Display", 900), 80.0, Fill::Solid(c("#EDEDED"))).with_shadow(Shadow::new(black, 2.0, 8.0, 45.0).opacity(0.9))));
    v.push(preset("cinema.credits", "Credits", Cinematic, "credits, small, caps", "DIRECTED BY",
        TextStyle::new(f("Inter", 500), 32.0, Fill::Solid(c("#D9D9D9"))).letter_spacing(30.0).case(Case::Upper)));
    v.push(preset("cinema.lower-third", "Lower Third", Cinematic, "name, bar, broadcast", "Maya Chen",
        TextStyle::new(f("Inter", 700), 44.0, Fill::Solid(white)).with_background(Background::new(c("#0F172A"), 4.0, 24.0, 10.0).per_line())));
    v.push(preset("cinema.documentary", "Documentary", Cinematic, "doc, serif, caption", "Istanbul, 2024",
        TextStyle::new(f("Lora", 500), 42.0, Fill::Solid(c("#F8F6F0"))).with_shadow(Shadow::new(black, 8.0, 0.0, 0.0).opacity(0.8))));
    v.push(preset("cinema.sci-fi", "Sci-fi", Cinematic, "space, thin, glow", "ORBITAL",
        TextStyle::new(f("Space Grotesk", 500), 72.0, Fill::Solid(c("#DFF6FF"))).with_glow(Glow::new(c("#4CC9F0"), 0.5, 8.0)).letter_spacing(20.0).case(Case::Upper)));

    // --- Social ----------------------------------------------------------
    v.push(preset("social.hook", "Hook", Social, "hook, reels, caption", "Wait for it…",
        TextStyle::new(f("Montserrat", 800), 64.0, Fill::Solid(white)).with_stroke(Stroke::new(black, 3.5)).with_shadow(Shadow::new(black, 2.0, 3.0, 90.0).opacity(0.6))));
    v.push(preset("social.highlight-box", "Highlight Box", Social, "box, yellow, highlight", "the best part",
        TextStyle::new(f("Poppins", 700), 56.0, Fill::Solid(c("#1A1A1A"))).with_background(Background::new(c("#FFE234"), 8.0, 16.0, 4.0).per_line())));
    v.push(preset("social.subtitle-bar", "Subtitle Bar", Social, "subtitle, bar, dark", "like and subscribe",
        TextStyle::new(f("Inter", 600), 44.0, Fill::Solid(white)).with_background(Background::new(c("#000000B3"), 10.0, 18.0, 8.0).per_line())));
    v.push(preset("social.pop-word", "Pop Word", Social, "karaoke, words, green", "THIS one",
        TextStyle::new(f("Unbounded", 800), 60.0, Fill::Solid(white)).with_cycle(ColourCycle::new(&[c("#FFFFFF"), c("#5CFF8A")], TextUnit::Word)).with_stroke(Stroke::new(black, 3.0))));
    v.push(preset("social.glass", "Glass", Social, "frosted, pill, modern", "@studio",
        TextStyle::new(f("DM Sans", 600), 44.0, Fill::Solid(white)).with_background(Background::new(c("#FFFFFF33"), 999.0, 22.0, 10.0)).with_stroke(Stroke::new(c("#FFFFFF40"), 0.6))));
    v.push(preset("social.quote", "Quote", Social, "quote, serif, wrap", "“Make it simple, but significant.”",
        TextStyle::new(fi("Lora", 500), 52.0, Fill::Solid(c("#FFF8EE"))).with_shadow(Shadow::new(black, 10.0, 0.0, 0.0).opacity(0.7)).line_spacing(10.0)));
    v.push(preset("social.price-tag", "Price Tag", Social, "shop, price, red", "$19.99",
        TextStyle::new(f("Rubik", 800), 72.0, Fill::Solid(white)).with_background(Background::new(c("#D7263D"), 12.0, 24.0, 8.0)).with_shadow(Shadow::new(black, 6.0, 4.0, 90.0).opacity(0.4))));
    v.push(preset("social.gradient-grotesk", "Gradient Grotesk", Social, "gradient, modern, tech", "Launch day",
        TextStyle::new(f("Space Grotesk", 700), 70.0, Fill::Gradient(Gradient::new(0.0, &[c("#4CC9F0"), c("#7B2CBF"), c("#F72585")])))));

    // --- Handmade --------------------------------------------------------
    v.push(preset("hand.marker", "Marker", Handmade, "marker, vlog, casual", "day in my life",
        TextStyle::new(f("Permanent Marker", 400), 72.0, Fill::Solid(white)).with_shadow(Shadow::new(black, 3.0, 3.0, 90.0).opacity(0.7))));
    v.push(preset("hand.script-tape", "Script Tape", Handmade, "script, tape, journal", "little moments",
        TextStyle::new(f("Caveat", 700), 84.0, Fill::Solid(c("#2F2F2F"))).with_background(Background::new(c("#FFFFFFE6"), 2.0, 22.0, 2.0))));
    v.push(preset("hand.chalk", "Chalk", Handmade, "chalk, board, cafe", "today's special",
        TextStyle::new(f("Shadows Into Light", 400), 80.0, Fill::Solid(c("#F4F1EA"))).with_glow(Glow::new(c("#F4F1EA"), 0.25, 4.0))));
    v.push(preset("hand.brush-red", "Brush", Handmade, "brush, bold, red", "Just go",
        TextStyle::new(f("Caveat Brush", 400), 90.0, Fill::Solid(c("#FF3B30"))).with_stroke(Stroke::new(white, 2.0)).with_shadow(Shadow::new(black, 2.0, 4.0, 90.0).opacity(0.5))));
    v.push(preset("hand.pencil", "Pencil", Handmade, "pencil, sketch, grey", "sketchbook",
        TextStyle::new(f("Homemade Apple", 400), 60.0, Fill::Solid(c("#4A4A4A"))).with_background(Background::new(c("#FBF8F1"), 0.0, 24.0, 12.0))));
    v.push(preset("hand.highlighter", "Highlighter", Handmade, "highlight, neon marker", "remember this",
        TextStyle::new(f("Shadows Into Light Two", 400), 76.0, Fill::Solid(c("#1A1A1A"))).with_background(Background::new(c("#C6FF00CC"), 3.0, 8.0, -6.0).per_line())));
    v
}

/// The whole catalogue.
pub fn catalogue() -> &'static [StylePreset] {
    static ALL: OnceLock<Vec<StylePreset>> = OnceLock::new();
    ALL.get_or_init(build)
}

/// Looks up a style by id.
pub fn find(id: &str) -> Option<&'static StylePreset> {
    catalogue().iter().find(|p| p.id == id)
}

/// Searches names and tags, case-insensitively.
pub fn search(query: &str) -> Vec<&'static StylePreset> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return catalogue().iter().collect();
    }
    catalogue().iter().filter(|p| p.name.to_lowercase().contains(&q) || p.tags.iter().any(|t| t.contains(&q)) || p.id.contains(&q)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_is_large_and_unique() {
        let all = catalogue();
        assert!(all.len() >= 60, "{} styles", all.len());
        let mut ids: Vec<&str> = all.iter().map(|p| p.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), all.len(), "duplicate ids");
        for cat in StyleCategory::ALL {
            assert!(all.iter().any(|p| p.category == cat), "{cat:?} empty");
        }
        for p in all {
            assert!((5.0..=300.0).contains(&p.style.size), "{} size", p.id);
            assert!(!p.sample.is_empty());
        }
    }

    #[test]
    fn search_finds_by_tag() {
        assert!(search("glow").len() >= 5);
        assert!(find("neon.pink-sign").is_some());
    }

    #[test]
    fn styles_serialise() {
        for p in catalogue() {
            let j = serde_json::to_string(p).unwrap();
            let back: StylePreset = serde_json::from_str(&j).unwrap();
            assert_eq!(back.style, p.style, "{}", p.id);
        }
    }

    #[test]
    fn fonts_exist_in_bundle() {
        let lib = crate::font::FontLibrary::bundled_only();
        if lib.faces().is_empty() {
            return;
        }
        let missing: Vec<&str> = catalogue().iter().filter(|p| !lib.has_family(&p.style.font.family)).map(|p| p.style.font.family.as_str()).collect();
        assert!(missing.is_empty(), "fonts missing from the bundle: {missing:?}");
    }
}
