// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Hidden component gallery (⇧⌘G, or `--screen gallery`): every shared
//! component in every state, side by side, in the current theme. Used to
//! review the visual system and as the target of screenshot checks.

use crate::app::StudioApp;
use crate::cli::Screen;
use crate::icons::{self, Icon};
use crate::kit::{self, Button, Forced, IconButton, Section, Segment, TileGrid, TileInfo};
use crate::prefs::ThemeChoice;
use crate::{view, theme, thumbs};
use eframe::egui::{self, Align2, Color32, CornerRadius, Rect, Sense, Vec2};
use crate::kit::ShowNoted;

pub struct GalleryState {
    pub slider: f32,
    pub bipolar: f32,
    pub number: f32,
    pub text: String,
    pub toggle: bool,
    pub seg: usize,
    pub seg_icons: usize,
    pub chip: usize,
    pub menu: usize,
    pub colour: Color32,
    pub kf: bool,
    pub enabled: bool,
    pub tile_sel: Option<usize>,
}

impl Default for GalleryState {
    fn default() -> Self {
        Self {
            slider: 64.0,
            bipolar: -18.0,
            number: 1920.0,
            text: String::new(),
            toggle: true,
            seg: 0,
            seg_icons: 1,
            chip: 1,
            menu: 0,
            colour: Color32::from_rgb(0xF2, 0x6B, 0x5B),
            kf: true,
            enabled: true,
            tile_sel: Some(2),
        }
    }
}

fn group(ui: &mut egui::Ui, title: &str, note: &str, add: impl FnOnce(&mut egui::Ui)) {
    let t = theme::tokens(ui.ctx());
    egui::Frame::new()
        .fill(t.panel)
        .corner_radius(CornerRadius::same(theme::radius::PANEL))
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(title).font(theme::section_title()).color(t.text));
                if !note.is_empty() {
                    ui.label(egui::RichText::new(note).font(theme::body()).color(t.text_2));
                }
            });
            ui.add_space(8.0);
            add(ui);
        });
    ui.add_space(theme::space::GAP * 2.0);
}

/// A caption above a cell in a state matrix.
fn cell(ui: &mut egui::Ui, caption: &str, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    let t = theme::tokens(ui.ctx());
    ui.vertical(|ui| {
        ui.set_width(width);
        ui.label(egui::RichText::new(caption).font(theme::caption()).color(t.text_2));
        ui.add_space(2.0);
        add(ui);
    });
}

pub fn show(app: &mut StudioApp, ui: &mut egui::Ui) {
    let t = theme::tokens(ui.ctx());
    let full = ui.max_rect();
    // Header.
    let header = Rect::from_min_size(full.min, Vec2::new(full.width(), 52.0));
    let mut hui = ui.new_child(egui::UiBuilder::new().max_rect(header.shrink2(Vec2::new(16.0, 12.0)).translate(Vec2::new(64.0, 0.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    if IconButton::new(Icon::CaretLeft, "Back to Start").shortcut("⇧⌘G").show(&mut hui).clicked() {
        app.screen = Screen::Start;
    }
    hui.label(egui::RichText::new("Component gallery").font(theme::section_title()).color(t.text));
    hui.label(egui::RichText::new("Every shared component in every state").font(theme::body()).color(t.text_2));
    hui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.add_space(64.0);
        let mut sel = if t.dark { 0 } else { 1 };
        if kit::segmented(ui, "gallery-theme", &mut sel, &[Segment::icon_text(Icon::Moon, "Dark"), Segment::icon_text(Icon::Sun, "Light")], None, None).changed() {
            app.prefs.theme = if sel == 0 { ThemeChoice::Dark } else { ThemeChoice::Light };
        }
        ui.label(egui::RichText::new("Reduce motion").font(theme::body()).color(t.text_2));
        let mut rm = app.prefs.reduce_motion;
        if kit::toggle(ui, &mut rm, "Reduce motion", None).changed() {
            app.prefs.reduce_motion = rm;
        }
    });

    let body = Rect::from_min_max(egui::pos2(full.left(), header.bottom()), full.max);
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(body));
    let mut sa = egui::ScrollArea::vertical().auto_shrink([false, false]);
    if let Some(y) = app.cli.scroll {
        sa = sa.vertical_scroll_offset(y);
    }
    sa.show_v(&mut bui, |ui| {
        let w = ui.available_width();
        let inner = (w - 32.0).min(1100.0);
        let margin = ((w - inner) / 2.0).floor();
        let (r, _) = ui.allocate_exact_size(Vec2::new(w, 0.0), Sense::hover());
        let mut col = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(egui::pos2(r.left() + margin, r.top()), Vec2::new(inner, 100_000.0))));
        contents(app, &mut col);
        col.add_space(24.0);
        let h = col.min_rect().height();
        ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    });
}

fn contents(app: &mut StudioApp, ui: &mut egui::Ui) {
    let t = theme::tokens(ui.ctx());
    let g = &mut app.gallery;

    group(ui, "Tokens", "single source in theme.rs", |ui| {
        let swatches: [(&str, Color32); 19] = [
            ("ground", t.ground), ("panel", t.panel), ("raised", t.raised), ("sunken", t.sunken), ("edge", t.edge), ("line", t.line),
            ("text", t.text), ("text-2", t.text_2), ("accent", t.accent), ("accent-soft", t.accent_soft), ("accent-fill", t.accent_fill), ("accent-ink", t.accent_ink),
            ("select", t.select), ("warn", t.warn), ("danger", t.danger), ("video", t.video), ("audio", t.audio), ("title", t.title), ("fx", t.fx),
        ];
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
            for (name, c) in swatches {
                let (r, _) = ui.allocate_exact_size(Vec2::new(124.0, 64.0), Sense::hover());
                let sw = Rect::from_min_size(r.min, Vec2::new(124.0, 36.0));
                ui.painter().rect(sw, CornerRadius::same(6), c, egui::Stroke::new(1.0, t.line), egui::StrokeKind::Inside);
                ui.painter().text(egui::pos2(r.left(), sw.bottom() + 6.0), Align2::LEFT_TOP, name, theme::caption(), t.text);
                ui.painter().text(egui::pos2(r.right(), sw.bottom() + 6.0), Align2::RIGHT_TOP, kit::to_hex(c), theme::mono_small(), t.text_2);
            }
        });
        ui.add_space(4.0);
        let pass = [t.panel, t.raised, t.ground, t.sunken].iter().all(|bg| theme::contrast(t.text_2, *bg) >= 4.5);
        ui.label(egui::RichText::new(format!(
            "text-2 on panel {:.1}:1 · accent-ink on accent-fill {:.1}:1 · {}",
            theme::contrast(t.text_2, t.panel),
            theme::contrast(t.accent_ink, t.accent_fill),
            if pass { "all text ≥ 4.5:1" } else { "contrast FAIL" }
        )).font(theme::caption()).color(t.text_2));
    });

    group(ui, "Type", "Manrope and JetBrains Mono", |ui| {
        for (name, font) in [
            ("Dialog title 20 / 700", theme::dialog_title()),
            ("Section title 14 / 700", theme::section_title()),
            ("Control 13 — Export, Split, Opacity", theme::control()),
            ("Body 12 — secondary descriptions and labels", theme::body()),
            ("Caption 11 — badges, timestamps, hints", theme::caption()),
            ("00:01:23:14  Timecode 13 tabular", theme::timecode()),
        ] {
            ui.label(egui::RichText::new(name).font(font).color(t.text));
        }
    });

    group(ui, "Icons", "Phosphor regular · 16 px controls, 20 px rail", |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
            for &icon in Icon::ALL {
                let (r, resp) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::hover());
                let st = kit::hover_t(ui.ctx(), resp.id, resp.hovered());
                ui.painter().rect_filled(r, CornerRadius::same(6), theme::alpha(t.raised, st));
                icons::paint(ui.painter(), r, icon, icons::SIZE, t.text);
                resp.on_hover_text(icon.name());
            }
        });
    });

    group(ui, "Buttons", "", |ui| {
        let kinds = [("Primary", kit::Kind::Primary), ("Secondary", kit::Kind::Secondary), ("Ghost", kit::Kind::Ghost), ("Danger", kit::Kind::Danger)];
        for (name, kind) in kinds {
            ui.horizontal(|ui| {
                for f in Forced::ALL {
                    cell(ui, &format!("{name} · {}", f.label()), 132.0, |ui| {
                        let icon = match kind { kit::Kind::Primary => Icon::Export, kit::Kind::Danger => Icon::Trash, _ => Icon::Plus };
                        Button::new(if kind == kit::Kind::Danger { "Delete" } else { "Export" }, kind).icon(icon).forced(f).show(ui);
                    });
                }
                cell(ui, "Busy", 120.0, |ui| {
                    Button::new("Saving", kind).busy(true).show(ui);
                });
            });
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            cell(ui, "Compact", 120.0, |ui| {
                Button::secondary("Import").icon(Icon::DownloadSimple).compact().show(ui);
            });
            cell(ui, "Menu trigger", 140.0, |ui| {
                Button::secondary("Add lane").trailing(Icon::CaretDown).show(ui);
            });
            cell(ui, "Live", 120.0, |ui| {
                if Button::primary("Show toast").show(ui).clicked() {
                    kit::push(ui.ctx(), "Clip split at 00:00:12:04", Some("Undo"), Some(Icon::Scissors));
                }
            });
        });
    });

    group(ui, "Icon buttons and switches", "tooltips show shortcuts", |ui| {
        ui.horizontal(|ui| {
            for f in Forced::ALL {
                cell(ui, f.label(), 64.0, |ui| {
                    IconButton::new(Icon::Scissors, "Blade").shortcut("B").forced(f).show(ui);
                });
            }
            cell(ui, "Active", 64.0, |ui| {
                IconButton::new(Icon::Magnet, "Snapping").shortcut("N").active(true).show(ui);
            });
            cell(ui, "Filled", 64.0, |ui| {
                IconButton::new(Icon::CornersOut, "Full screen").shortcut("F").filled().show(ui);
            });
            ui.add_space(24.0);
            for (label, f, on) in [("On", None, true), ("Off", None, false), ("Focus", Some(Forced::Focused), true), ("Disabled", Some(Forced::Disabled), false)] {
                cell(ui, label, 64.0, |ui| {
                    let mut v = if f.is_none() && label == "On" { g.toggle } else { on };
                    if kit::toggle(ui, &mut v, "Switch", f).changed() && label == "On" {
                        g.toggle = v;
                    }
                });
            }
        });
    });

    group(ui, "Field rows", "drag the label to scrub (⇧ ×10, ⌥ ×0.1), double-click to reset, type +10 or *2", |ui| {
        ui.columns(2, |cols| {
            let ui = &mut cols[0];
            kit::on_card(ui);
            egui::Frame::new().fill(t.raised).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                kit::FieldRow::new("Opacity", &mut g.slider, 0.0..=100.0).default(100.0).unit("%").keyframe(&mut g.kf).show(ui);
                kit::FieldRow::new("Temperature", &mut g.bipolar, -100.0..=100.0).default(0.0).show(ui);
                let mut fixed = 100.0;
                kit::FieldRow::new("Scale (hover)", &mut fixed, 0.0..=400.0).default(100.0).unit("%").forced(Forced::Hover).show(ui);
                let mut fixed = 12.0;
                kit::FieldRow::new("Rotation (focus)", &mut fixed, -180.0..=180.0).default(0.0).unit("°").forced(Forced::Focused).show(ui);
                let mut fixed = 40.0;
                kit::FieldRow::new("Disabled", &mut fixed, 0.0..=100.0).default(0.0).forced(Forced::Disabled).show(ui);
                kit::FieldRow::new("Width", &mut g.number, 16.0..=8192.0).default(1920.0).unit("px").no_slider().show(ui);
                kit::labeled_row(ui, "Fill", |ui| {
                    kit::colour_well(ui, "gallery-fill", &mut g.colour, None);
                    ui.label(egui::RichText::new(kit::to_hex(g.colour)).font(theme::body()).color(t.text_2));
                });
            });
            let ui = &mut cols[1];
            ui.label(egui::RichText::new("Sliders").font(theme::caption()).color(t.text_2));
            ui.horizontal(|ui| {
                for f in [Forced::Rest, Forced::Hover, Forced::Pressed, Forced::Focused, Forced::Disabled] {
                    cell(ui, f.label(), 96.0, |ui| {
                        ui.add_space(if f == Forced::Pressed { 30.0 } else { 0.0 });
                        let mut v = 62.0;
                        kit::Slider::new("Slider", &mut v, 0.0..=100.0).default(0.0).unit("%").forced(f).width(90.0).show(ui);
                    });
                }
            });
            ui.add_space(8.0);
            ui.label(egui::RichText::new("Number fields").font(theme::caption()).color(t.text_2));
            ui.horizontal(|ui| {
                for f in [Forced::Rest, Forced::Hover, Forced::Focused, Forced::Disabled] {
                    cell(ui, f.label(), 80.0, |ui| {
                        let mut v = 24.0;
                        kit::NumberField::new("n", &mut v).unit("fps").forced(f).width(76.0).show(ui);
                    });
                }
            });
            ui.add_space(8.0);
            ui.label(egui::RichText::new("Text fields").font(theme::caption()).color(t.text_2));
            ui.horizontal(|ui| {
                kit::text_field(ui, &mut g.text, "Search media", Some(Icon::MagnifyingGlass), 180.0, None);
                let mut filled = "Lisbon".to_owned();
                kit::text_field(ui, &mut filled, "Name", None, 120.0, Some(Forced::Focused));
            });
        });
    });

    group(ui, "Section cards", "collapse state is remembered per section type", |ui| {
        ui.columns(3, |cols| {
            let mut a = 100.0;
            let mut b = 0.0;
            Section::new("gallery-transform", "Transform").icon(Icon::ArrowsOutCardinal).resettable(true).force_open(true).show(&mut cols[0], |ui| {
                kit::FieldRow::new("Scale", &mut a, 0.0..=400.0).default(100.0).unit("%").show(ui);
                kit::FieldRow::new("Rotation", &mut b, -180.0..=180.0).default(0.0).unit("°").show(ui);
            });
            let mut on = g.enabled;
            let mut c = 20.0;
            let out = Section::new("gallery-colour", "Colour").icon(Icon::DropHalf).enable(&mut on).resettable(true).force_open(true).show(&mut cols[1], |ui| {
                kit::FieldRow::new("Exposure", &mut c, -100.0..=100.0).default(0.0).show(ui);
                kit::FieldRow::new("Contrast", &mut b, -100.0..=100.0).default(0.0).show(ui);
            });
            if out.enable_changed {
                g.enabled = on;
            }
            Section::new("gallery-audio", "Audio (collapsed)").icon(Icon::SpeakerHigh).force_open(false).show(&mut cols[2], |_| {});
            cols[2].add_space(8.0);
            let mut vol = -6.0;
            Section::new("gallery-live", "Live (click header)").icon(Icon::Faders).show(&mut cols[2], |ui| {
                kit::FieldRow::new("Volume", &mut vol, -60.0..=12.0).default(0.0).unit("dB").show(ui);
            });
        });
    });

    group(ui, "Segmented, chips, menu button", "", |ui| {
        ui.horizontal(|ui| {
            for f in [None, Some(Forced::Focused), Some(Forced::Disabled)] {
                cell(ui, f.map_or("Live", |f| f.label()), 230.0, |ui| {
                    let mut s = g.seg;
                    if kit::segmented(ui, &format!("g-seg-{f:?}"), &mut s, &[Segment::text("Full"), Segment::text("Half"), Segment::text("Quarter")], None, f).changed() {
                        g.seg = s;
                    }
                });
            }
            cell(ui, "Icons", 120.0, |ui| {
                kit::segmented(ui, "g-seg-icons", &mut g.seg_icons, &[Segment::icon(Icon::SquaresFour, "Grid"), Segment::icon(Icon::List, "List")], None, None);
            });
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            for (i, l) in ["All", "Selected", "Hover"].iter().enumerate() {
                let f = if i == 2 { Some(Forced::Hover) } else { None };
                kit::chip(ui, l, g.chip == i && f.is_none(), f);
            }
            kit::chip(ui, "Focus", false, Some(Forced::Focused));
            kit::chip(ui, "Disabled", false, Some(Forced::Disabled));
            ui.add_space(16.0);
            kit::chip_group(ui, &mut g.chip, &["Entry", "Exit", "Loop"]);
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            for f in [None, Some(Forced::Hover), Some(Forced::Pressed), Some(Forced::Disabled)] {
                cell(ui, f.map_or("Live", |f| f.label()), 168.0, |ui| {
                    if let Some(i) = kit::menu_button(ui, &format!("g-menu-{f:?}"), g.menu, &["H.264 (MP4)", "HEVC (MP4)", "Intermediate (MOV)"], 160.0, f) {
                        g.menu = i;
                    }
                });
            }
        });
    });

    group(ui, "Popover, menu, tooltip, toast", "only these surfaces cast a shadow", |ui| {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 24.0;
            egui::Frame::new().fill(t.panel).stroke(egui::Stroke::new(1.0, t.line)).corner_radius(CornerRadius::same(8)).shadow(theme::popover_shadow()).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
                ui.set_width(220.0);
                ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                kit::menu_heading(ui, "Clip");
                kit::MenuItem::new("Split at playhead").icon(Icon::Scissors).shortcut("S").show(ui);
                kit::MenuItem::new("Duplicate").icon(Icon::Copy).shortcut("⌘D").show(ui);
                kit::MenuItem::new("Snapping").checked(true).shortcut("N").show(ui);
                kit::MenuItem::new("Detach audio").enabled(false).show(ui);
                kit::menu_separator(ui);
                kit::MenuItem::new("Delete").icon(Icon::Trash).shortcut("⌫").danger().show(ui);
                });
            });
            egui::Frame::new().fill(t.panel).stroke(egui::Stroke::new(1.0, t.line)).corner_radius(CornerRadius::same(8)).shadow(theme::popover_shadow()).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
                ui.set_width(232.0);
                ui.vertical(|ui| kit::picker(ui, egui::Id::unique("gallery-picker"), &mut g.colour));
            });
            ui.vertical(|ui| {
                egui::Frame::new().fill(t.panel).stroke(egui::Stroke::new(1.0, t.line)).corner_radius(CornerRadius::same(6)).shadow(theme::popover_shadow()).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
                    kit::tooltip_body(ui, "Blade", Some("B"));
                });
                ui.add_space(16.0);
                kit::preview(ui, "Clip split at 00:00:12:04", Some("Undo"), Some(Icon::Scissors));
                ui.add_space(12.0);
                kit::preview(ui, "Export finished", None, Some(Icon::CheckCircle));
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::hover());
                    kit::spinner(ui.painter(), r.center(), 8.0, t.accent);
                    let (r, _) = ui.allocate_exact_size(Vec2::new(160.0, 24.0), Sense::hover());
                    let track = Rect::from_center_size(r.center(), Vec2::new(160.0, 4.0));
                    ui.painter().rect_filled(track, CornerRadius::same(2), t.raised);
                    ui.painter().rect_filled(Rect::from_min_size(track.min, Vec2::new(160.0 * 0.62, 4.0)), CornerRadius::same(2), t.accent);
                });
            });
        });
    });

    let thumbs = &mut app.thumbs;
    group(ui, "Tiles", "hover: 1 px ring + live preview · selected: 2 px ring · drag shows a ghost", |ui| {
        let names = ["Rest", "Hover", "Selected", "Progress", "A long name that does not fit"];
        let sel = g.tile_sel;
        let mut grid = TileGrid::new("gallery-tiles").min_width(150.0);
        grid.forced = Some((1, Forced::Hover));
        let out = grid.show(
            ui,
            5,
            |i| TileInfo { label: names[i], badge: Some(["0:12", "Fade", "4.5s", "Adding", "1:02"][i]), sub: None, selected: sel == Some(i), addable: true, progress: (i == 3).then_some(0.4), tip: None, added: i == 0 },
            |i| i,
            &mut |p, r, i, _h, live| {
                let frame = if live { ((kit::clock(p.ctx()) * 8.0) as u8) % thumbs::PREVIEW_FRAMES } else { 0 };
                let key = thumbs::Key { pic: thumbs::Pic::Effect { index: i as u8 * 2, frame }, w: 192, h: 108 };
                if let Some(tex) = thumbs.get(key) {
                    p.image(tex, r, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                }
            },
        );
        if let Some(i) = out.clicked {
            g.tile_sel = Some(i);
        }
        ui.add_space(8.0);
        ui.columns(2, |cols| {
            kit::tile_list(&mut cols[0], "gallery-list", 3, |i| TileInfo { label: ["Arrival at the harbour.mp4", "Tram 28 uphill.mp4", "Voiceover take 3.wav"][i], badge: Some(["0:14", "0:09", "0:58"][i]), sub: None, selected: i == 1, addable: false, progress: None, tip: None, added: false }, |i| i, &mut |p, r, i, _, _| {
                if let Some(tex) = thumbs.scene([11, 23, 127][i], 0.2, 160, 90) {
                    p.image(tex, r, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                }
            });
            let ui = &mut cols[1];
            let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 140.0), Sense::hover());
            ui.painter().rect_filled(r, CornerRadius::same(8), t.panel);
            ui.painter().rect_stroke(r, CornerRadius::same(8), egui::Stroke::new(1.0, t.line), egui::StrokeKind::Inside);
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
            kit::empty_state(&mut child, Icon::FilmStrip, "Your media will appear here.", Some(("Import media", Icon::DownloadSimple)));
        });
    });
    let _ = view::SEC;
}
