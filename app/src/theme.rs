// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Visual tokens and the egui style built from them.
//!
//! This is the single source of colours, spacing, radii and type sizes for
//! the whole app. Widgets ask for tokens with [`tokens`] instead of writing
//! colour literals, so the light and dark themes stay in step.

use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow, Stroke, Theme, Visuals};
use std::sync::Arc;

/// Colour tokens for one theme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    pub ground: Color32,
    pub panel: Color32,
    pub raised: Color32,
    pub sunken: Color32,
    /// 1 px highlight along the top edge of panels and cards.
    pub edge: Color32,
    /// The alternate panel tone: hover and selected rows, the timeline
    /// lanes, the player surround.
    pub alt: Color32,
    pub line: Color32,
    pub text: Color32,
    pub text_2: Color32,
    pub accent: Color32,
    /// Selection fills.
    pub accent_soft: Color32,
    /// Accent behind text (buttons, chips): the accent, deepened just
    /// enough in the light theme for its label to pass 4.5:1.
    pub accent_fill: Color32,
    pub accent_ink: Color32,
    /// Selection outlines on canvas and timeline (cool, so it never fights
    /// the accent).
    pub select: Color32,
    pub warn: Color32,
    pub danger: Color32,
    pub video: Color32,
    pub audio: Color32,
    pub title: Color32,
    pub fx: Color32,
    /// Text drawn on top of clip fills (same in both themes because the
    /// clip fills are shared).
    pub on_clip: Color32,
    pub dark: bool,
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// Straight-alpha colour as a const (premultiplied for egui).
const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color32 {
    let k = a;
    Color32::from_rgba_premultiplied((r as f32 * k) as u8, (g as f32 * k) as u8, (b as f32 * k) as u8, (a * 255.0) as u8)
}

// The reference look (studio-specs/facts/visual-design.md): neutral
// graphite cards on a near-black window, one cyan accent.
pub const DARK: Tokens = Tokens {
    ground: hex(0x141415),
    panel: hex(0x1F1F21),
    raised: hex(0x323236),
    sunken: hex(0x141415),
    edge: rgba(255, 255, 255, 0.0),
    alt: hex(0x27272A),
    line: hex(0x343438),
    text: hex(0xE4E4E6),
    text_2: hex(0x9A9AA0),
    accent: hex(0x45CEDD),
    accent_soft: rgba(69, 206, 221, 0.16),
    accent_fill: hex(0x45CEDD),
    accent_ink: hex(0x0B1A1C),
    select: hex(0x45CEDD),
    warn: hex(0xF2C14E),
    danger: hex(0xE56B6B),
    video: hex(0x1C5D63),
    audio: hex(0x2F7A5A),
    title: hex(0x8A5A2F),
    fx: hex(0x5A4A8A),
    on_clip: hex(0xFFFFFF),
    dark: true,
};

// The same brand in daylight: white cards on a cool paper grey, the cyan
// accent deepened until it reads on white, and the clip colours of the dark
// theme, darkened just enough to carry white names.
pub const LIGHT: Tokens = Tokens {
    ground: hex(0xECEEF1),
    panel: hex(0xFFFFFF),
    raised: hex(0xF2F4F6),
    sunken: hex(0xE3E6EA),
    edge: rgba(0, 0, 0, 0.06),
    alt: hex(0xE6E9ED),
    line: hex(0xD5D9DF),
    text: hex(0x14171B),
    text_2: hex(0x565F69),
    accent: hex(0x0A8D9A),
    accent_soft: rgba(10, 141, 154, 0.13),
    accent_fill: hex(0x087A86),
    accent_ink: hex(0xFFFFFF),
    select: hex(0x0A8D9A),
    warn: hex(0xA8780B),
    danger: hex(0xC62828),
    video: hex(0x1C6871),
    audio: hex(0x2B7553),
    title: hex(0x8F5E26),
    fx: hex(0x5E4D9A),
    on_clip: hex(0xFFFFFF),
    dark: false,
};

/// Tokens for the theme currently shown.
pub fn tokens(ctx: &egui::Context) -> Tokens {
    let base = match ctx.theme() {
        Theme::Dark => DARK,
        Theme::Light => LIGHT,
    };
    crate::access::adapt(base, crate::access::high_contrast(), crate::access::colour_vision())
}

impl Tokens {
    /// A hover tint over `base`: a slightly lighter (dark theme) or darker
    /// (light theme) version, used for rows and buttons.
    pub fn hover(&self, base: Color32) -> Color32 {
        if self.dark {
            mix(base, Color32::WHITE, 0.05)
        } else {
            mix(base, Color32::BLACK, 0.04)
        }
    }
    pub fn pressed(&self, base: Color32) -> Color32 {
        if self.dark {
            mix(base, Color32::WHITE, 0.09)
        } else {
            mix(base, Color32::BLACK, 0.08)
        }
    }
    /// The selection wash composited over `base`, as an opaque colour.
    pub fn accent_soft_on(&self, base: Color32) -> Color32 {
        let a = self.accent_soft.a() as f32 / 255.0;
        mix(base, self.accent, a)
    }
    /// Hairline width: 1 px at 1x, 0.5 px at 2x (spec §1).
    pub fn hairline(&self, ctx: &egui::Context) -> f32 {
        if crate::access::high_contrast() { 2.0 } else if ctx.pixels_per_point() >= 2.0 { 0.5 } else { 1.0 }
    }
    pub fn hair(&self, ctx: &egui::Context) -> Stroke {
        Stroke::new(self.hairline(ctx), self.line)
    }
    /// Text placed on accent fills.
    pub fn on_accent(&self) -> Color32 {
        self.accent_ink
    }
    /// Disabled text: text-2 at reduced strength.
    pub fn disabled(&self) -> Color32 {
        mix(self.text_2, self.panel, 0.45)
    }
}

/// Linear mix of two colours in gamma space (fine for UI tints).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        l(a.r(), b.r()),
        l(a.g(), b.g()),
        l(a.b(), b.b()),
        l(a.a(), b.a()),
    )
}

/// The same colour at a given opacity.
pub fn alpha(c: Color32, a: f32) -> Color32 {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(r, g, b, (a.clamp(0.0, 1.0) * 255.0).round() as u8)
}

// --- spacing, radii (spec §1) ---------------------------------------------

pub mod space {
    pub const PANEL_PAD: f32 = 10.0;
    pub const SECTION_GAP: f32 = 12.0;
    pub const CONTROL_H: f32 = 24.0;
    pub const CONTROL_H_COMPACT: f32 = 22.0;
    pub const ROW_H: f32 = 26.0;
    pub const GAP: f32 = 4.0;
}

pub mod radius {
    pub const PANEL: u8 = 6;
    pub const CARD: u8 = 6;
    pub const CONTROL: u8 = 4;
    pub const CHIP: u8 = 4;
    pub const CLIP: u8 = 4;
}

// --- type scale ----------------------------------------------------------

pub const SEMIBOLD: &str = "semibold";
pub const BOLD: &str = "bold";
pub const MEDIUM: &str = "medium";

// Manrope 500 for running text, 600 for section titles, 700 for dialog
// titles; JetBrains Mono for
// timecode and numbers in fields (always tabular).
fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(MEDIUM.into()))
}

pub fn caption() -> FontId {
    medium(11.0)
}
pub fn body() -> FontId {
    medium(12.0)
}
pub fn control() -> FontId {
    medium(13.0)
}
pub fn control_medium() -> FontId {
    medium(13.0)
}
pub fn body_medium() -> FontId {
    medium(12.0)
}
pub fn section_title() -> FontId {
    FontId::new(14.0, FontFamily::Name(SEMIBOLD.into()))
}
pub fn dialog_title() -> FontId {
    FontId::new(20.0, FontFamily::Name(BOLD.into()))
}
pub fn timecode() -> FontId {
    FontId::new(13.0, FontFamily::Monospace)
}
pub fn mono_small() -> FontId {
    FontId::new(11.0, FontFamily::Monospace)
}
/// Numbers in value fields.
pub fn number() -> FontId {
    FontId::new(12.5, FontFamily::Monospace)
}

/// Floating elements (popovers, menus, transport pill, dialogs):
/// 0 12 32 rgba(0,0,0,.55).
pub fn popover_shadow() -> Shadow {
    // The reference look has no shadows anywhere; floating surfaces are
    // told apart by tone alone.
    Shadow::NONE
}

/// Paint a docked surface: fill, 1 px `line` border and the 1 px `edge`
/// highlight along its top (depth from tone, not shadows).
pub fn surface(p: &egui::Painter, rect: egui::Rect, radius: u8, fill: Color32, t: &Tokens) {
    let cr = CornerRadius::same(radius);
    // Cards sit on the darker window with no border or highlight.
    let _ = t;
    p.rect_filled(rect, cr, fill);
}

// --- fonts ---------------------------------------------------------------

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let add = |fonts: &mut FontDefinitions, name: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    };
    add(&mut fonts, "manrope-medium", include_bytes!("../assets/fonts/Manrope-Medium.ttf"));
    add(&mut fonts, "manrope-semibold", include_bytes!("../assets/fonts/Manrope-SemiBold.ttf"));
    add(&mut fonts, "manrope-bold", include_bytes!("../assets/fonts/Manrope-Bold.ttf"));
    add(&mut fonts, "jbmono", include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"));

    // Keep egui's built-in fonts as fallbacks for glyphs Manrope lacks.
    let fallbacks: Vec<String> = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let with = |first: &str| {
        let mut v = vec![first.to_owned()];
        v.extend(fallbacks.iter().cloned());
        v
    };
    // Last resort for key and arrow symbols (command, delete, triangles) that
    // neither Manrope nor egui's fonts carry; without it they draw as boxes.
    let with = |first: &str| {
        let mut v = with(first);
        v.push("jbmono".to_owned());
        v
    };
    fonts.families.insert(FontFamily::Proportional, with("manrope-medium"));
    fonts.families.insert(FontFamily::Name(MEDIUM.into()), with("manrope-medium"));
    fonts.families.insert(FontFamily::Name(SEMIBOLD.into()), with("manrope-semibold"));
    fonts.families.insert(FontFamily::Name(BOLD.into()), with("manrope-bold"));
    let mut mono = vec!["jbmono".to_owned()];
    mono.extend(fallbacks.iter().cloned());
    fonts.families.insert(FontFamily::Monospace, mono);
    ctx.set_fonts(fonts);
}

// --- egui style ----------------------------------------------------------

/// Push our tokens into egui's own visuals so built-in pieces (text edit,
/// scroll bars, tooltips, menus) match the rest of the interface.
pub fn apply_style(ctx: &egui::Context) {
    let (hc, cv) = (crate::access::high_contrast(), crate::access::colour_vision());
    for (theme, t) in [(Theme::Dark, crate::access::adapt(DARK, hc, cv)), (Theme::Light, crate::access::adapt(LIGHT, hc, cv))] {
        ctx.style_mut_of(theme, |style| {
            let mut v = if t.dark { Visuals::dark() } else { Visuals::light() };
            v.override_text_color = None;
            v.panel_fill = t.panel;
            v.window_fill = t.panel;
            v.extreme_bg_color = t.sunken;
            v.faint_bg_color = t.raised;
            v.code_bg_color = t.raised;
            v.hyperlink_color = t.accent;
            v.warn_fg_color = t.warn;
            v.error_fg_color = t.danger;
            v.window_stroke = Stroke::new(1.0, t.line);
            v.window_shadow = popover_shadow();
            v.popup_shadow = popover_shadow();
            v.window_corner_radius = CornerRadius::same(radius::CARD);
            v.menu_corner_radius = CornerRadius::same(radius::CARD);
            v.selection.bg_fill = alpha(t.select, 0.35);
            v.selection.stroke = Stroke::new(1.0, t.select);
            v.text_cursor.stroke = Stroke::new(1.5, t.accent);
            v.striped = false;
            v.slider_trailing_fill = true;
            // Any egui slider that slips past the kit still gets the round knob and a thin rail.
            v.handle_shape = egui::style::HandleShape::Circle;
            let cr = CornerRadius::same(radius::CONTROL);
            let w = &mut v.widgets;
            w.noninteractive.bg_fill = t.panel;
            w.noninteractive.weak_bg_fill = t.panel;
            w.noninteractive.bg_stroke = Stroke::new(1.0, t.line);
            w.noninteractive.fg_stroke = Stroke::new(1.0, t.text);
            w.noninteractive.corner_radius = cr;
            w.inactive.bg_fill = t.raised;
            w.inactive.weak_bg_fill = t.raised;
            w.inactive.bg_stroke = Stroke::NONE;
            w.inactive.fg_stroke = Stroke::new(1.0, t.text);
            w.inactive.corner_radius = cr;
            w.hovered.bg_fill = t.hover(t.raised);
            w.hovered.weak_bg_fill = t.hover(t.raised);
            w.hovered.bg_stroke = Stroke::new(1.0, t.line);
            w.hovered.fg_stroke = Stroke::new(1.0, t.text);
            w.hovered.corner_radius = cr;
            w.hovered.expansion = 0.0;
            w.active.bg_fill = t.pressed(t.raised);
            w.active.weak_bg_fill = t.pressed(t.raised);
            w.active.bg_stroke = Stroke::new(1.0, t.accent);
            w.active.fg_stroke = Stroke::new(1.0, t.text);
            w.active.corner_radius = cr;
            w.active.expansion = 0.0;
            w.open = w.active;
            style.visuals = v;

            style.spacing.item_spacing = egui::vec2(8.0, 8.0);
            style.spacing.button_padding = egui::vec2(10.0, 6.0);
            style.spacing.interact_size = egui::vec2(28.0, space::CONTROL_H);
            style.spacing.slider_rail_height = 4.0;
            style.spacing.menu_margin = Margin::same(6);
            style.spacing.window_margin = Margin::same(12);
            thin_scroll(&mut style.spacing.scroll);
            style.spacing.tooltip_width = 280.0;
            style.interaction.tooltip_delay = 0.35;
            style.interaction.show_tooltips_only_when_still = false;
            style.text_styles.insert(egui::TextStyle::Body, control());
            style.text_styles.insert(egui::TextStyle::Button, control());
            style.text_styles.insert(egui::TextStyle::Small, caption());
            style.text_styles.insert(egui::TextStyle::Heading, section_title());
            style.text_styles.insert(egui::TextStyle::Monospace, timecode());
            style.animation_time = 0.12;
        });
    }
}

/// Relative luminance per WCAG 2.x.
pub fn luminance(c: Color32) -> f32 {
    let f = |u: u8| {
        let s = u as f32 / 255.0;
        if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
}

pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spec §8.3: all text passes 4.5:1, captions included.
    #[test]
    fn text_contrast() {
        for t in [DARK, LIGHT] {
            for bg in [t.panel, t.raised, t.ground, t.sunken] {
                assert!(contrast(t.text, bg) >= 4.5, "text on {bg:?}");
                assert!(contrast(t.text_2, bg) >= 4.5, "text-2 on {bg:?} = {}", contrast(t.text_2, bg));
            }
            assert!(contrast(t.accent_ink, t.accent_fill) >= 4.5, "accent ink on {:?}: {}", t.accent_fill, contrast(t.accent_ink, t.accent_fill));
            // Accent text (links, active labels) on panels.
            assert!(contrast(t.accent, t.panel) >= 3.0, "accent marks");
            for clip in [t.video, t.audio, t.title, t.fx] {
                assert!(contrast(t.on_clip, clip) >= 4.5, "clip text on {clip:?}");
            }
        }
    }
}

/// The thin scroll bar of the AI chat: floating over the content (no room
/// taken, so a row that fits keeps its height), only seen while the content
/// overflows and the pointer is near or it scrolls.
pub fn thin_scroll(s: &mut egui::style::ScrollStyle) {
    s.bar_width = 6.0;
    s.floating = true;
    s.bar_inner_margin = 2.0;
    s.bar_outer_margin = 2.0;
}
