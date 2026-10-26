// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Settings: general, playback, storage, shortcuts, models and about
//! (with the licences of everything we bundle).

use crate::app::{Overlay, StudioApp};
use crate::icons::{self, Icon};
use crate::kit::{self, Button, FieldRow, Section, Segment};
use crate::prefs::ThemeChoice;
use crate::theme;
use eframe::egui::{self, Align2, CornerRadius, Rect, Sense, Vec2};
use crate::kit::ShowNoted;

#[derive(Default)]
pub struct SettingsState {
    pub page: usize,
    pub shortcut_query: String,
    pub capturing: Option<usize>,
    pub open_licence: Option<usize>,
    /// Settings ▸ AI: the key being typed, and what happened to the last one.
    pub ai_key: String,
    pub ai_key_note: Option<String>,
    /// Whether a key is in the Keychain (asked once per visit).
    pub ai_key_saved: Option<bool>,
    /// Settings ▸ AI ▸ Voice & sound: the choices (read once per visit),
    /// the key being typed and what happened to the last one.
    pub voice: Option<crate::agent::voice_service::VoiceSettings>,
    pub voice_key: String,
    pub voice_key_note: Option<String>,
    pub voice_key_saved: Option<(crate::agent::voice_service::Provider, bool)>,
    pub sound_key_saved: Option<bool>,
    /// Settings ▸ AI ▸ What the AI has learned: the lessons as last read
    /// (and when), the one being edited, and whether "delete all" is asked.
    pub lessons: Option<Vec<crate::agent::lessons::Lesson>>,
    pub lessons_at: f64,
    pub lesson_edit: Option<(u64, String)>,
    pub lesson_clear: bool,
    /// Settings ▸ Models: the real models folder and its downloads (made
    /// on the first visit; downloads go on when the page is left).
    pub models: Option<crate::services::models::Models>,
    /// The rows as last read from the folder, and when (seconds).
    pub model_rows: Vec<crate::services::models::ModelRow>,
    pub model_rows_at: f64,
    /// Settings ▸ Account: the plan, the cloud, credits, receipts, marketplace.
    #[cfg(feature = "commerce")]
    pub account: crate::screens::account::AccountState,
}

const PAGES: &[(&str, Icon)] = &[
    ("General", Icon::GearSix),
    ("Playback", Icon::MonitorPlay),
    ("Storage", Icon::HardDrives),
    ("Shortcuts", Icon::Keyboard),
    ("AI", Icon::Sparkle),
    ("Models", Icon::Cpu),
    #[cfg(feature = "commerce")]
    ("Account", Icon::Star),
    ("About", Icon::Info),
];

/// The default shortcut map (ours, spec §60).
pub const SHORTCUTS: &[(&str, &str, &str)] = &[
    ("play", "Play / pause", "Space"),
    ("shuttle-back", "Play backwards (faster on repeat)", "J"),
    ("stop", "Stop", "K"),
    ("shuttle-fwd", "Play forwards (faster on repeat)", "L"),
    ("in", "Set in point", "I"),
    ("out", "Set out point", "O"),
    ("clear-range", "Clear range", "⌥X"),
    ("select", "Select tool", "A"),
    ("blade", "Blade tool", "B"),
    ("slip", "Slip tool", "Y"),
    ("range", "Range tool", "R"),
    ("split", "Split at playhead", "S"),
    ("delete", "Delete", "⌫"),
    ("duplicate", "Duplicate", "⌘D"),
    ("select-all", "Select all", "⌘A"),
    ("undo", "Undo", "⌘Z"),
    ("redo", "Redo", "⇧⌘Z"),
    ("marker", "Add marker", "M"),
    ("snap", "Snapping", "N"),
    ("magnetic", "Magnetic main lane", "⇧M"),
    ("frame-back", "Previous frame", "←"),
    ("frame-fwd", "Next frame", "→"),
    ("zoom-in", "Zoom timeline in", "⌘="),
    ("zoom-out", "Zoom timeline out", "⌘-"),
    ("fit", "Fit sequence", "⇧Z"),
    ("fullscreen", "Full-screen viewer", "F"),
    ("before-after", "Before / after", "\\"),
    ("export", "Export", "⌘E"),
    ("settings", "Settings", "⌘,"),
    ("macro-record", "Start / stop recording a macro", "⌥⌘R"),
    ("macro-run", "Run the last macro on the selection", "⌥⌘M"),
    ("workspace-edit", "Edit workspace", "⌥1"),
    ("workspace-colour", "Colour workspace", "⌥2"),
    ("workspace-audio", "Audio workspace", "⌥3"),
    ("workspace-vfx", "VFX workspace", "⌥4"),
    ("workspace-motion", "Motion graphics workspace", "⌥5"),
    ("workspace-production", "Production workspace", "⌥6"),
    ("panel-library", "Show or hide the Library", "⌥⌘L"),
    ("panel-inspector", "Show or hide the Inspector", "⌥⌘I"),
    ("panel-timeline", "Show or hide the Timeline", "⌥⌘T"),
    // Film editing (editor::cutting).
    ("source-monitor", "Source monitor", "⇧2"),
    ("program-monitor", "Program monitor", "⇧4"),
    ("insert", "Insert from the source", ","),
    ("overwrite", "Overwrite from the source", "."),
    ("lift", "Lift between the marks", ";"),
    ("extract", "Extract between the marks", "'"),
    ("match-frame", "Match frame", "⇧F"),
    ("extend", "Extend the edit to the playhead", "E"),
    ("ripple-start", "Ripple the clip's start to the playhead", "Q"),
    ("ripple-end", "Ripple the clip's end to the playhead", "W"),
    ("trim-mode", "Trim mode: ripple, roll, off", "T"),
    ("slide-mode", "Slide mode", "U"),
    ("trim-back", "Trim the edit a frame earlier", "["),
    ("trim-fwd", "Trim the edit a frame later", "]"),
    ("trim-back-10", "Trim the edit ten frames earlier", "⇧["),
    ("trim-fwd-10", "Trim the edit ten frames later", "⇧]"),
    ("editorial-panel", "Editorial window", "⌥E"),
];

struct Licence {
    name: &'static str,
    what: &'static str,
    licence: &'static str,
    text: &'static str,
}

const LICENCES: &[Licence] = &[
    Licence { name: "Manrope", what: "Interface typeface by the Manrope Project Authors", licence: "SIL Open Font License 1.1", text: include_str!("../../assets/fonts/Manrope-LICENSE.txt") },
    Licence { name: "JetBrains Mono", what: "Timecode typeface by JetBrains", licence: "SIL Open Font License 1.1", text: include_str!("../../assets/fonts/JetBrainsMono-LICENSE.txt") },
    Licence { name: "Phosphor Icons", what: "Icon set by Phosphor Icons", licence: "MIT License", text: include_str!("../../assets/icons/Phosphor-LICENSE.txt") },
];

pub fn show(app: &mut StudioApp, ctx: &egui::Context) {
    let out = kit::modal(ctx, egui::Id::unique("settings-dialog"), Vec2::new(760.0, 560.0), "Settings", |ui| body(app, ui));
    if out.close && app.settings.capturing.is_none() {
        app.overlay = Overlay::None;
    }
}

fn body(app: &mut StudioApp, ui: &mut egui::Ui) {
    let t = theme::tokens(ui.ctx());
    let r = ui.max_rect();
    // Pages as a row of tabs along the top; the active one is filled.
    let nav = Rect::from_min_size(r.min + Vec2::new(16.0, 6.0), Vec2::new(r.width() - 32.0, 28.0));
    // More pages than the width: the row scrolls sideways.
    let mut bar = ui.new_child(egui::UiBuilder::new().max_rect(nav).id_salt("settings-tabs"));
    bar.set_clip_rect(nav.intersect(ui.clip_rect()));
    kit::scroll_strip(&mut bar, "settings-tabs-scroll", |ui| {
        let natural: f32 = PAGES.iter().map(|(n, _)| kit::text_width(ui, n, theme::body()) + 26.0).sum();
        let (row, _) = ui.allocate_exact_size(Vec2::new(natural.max(nav.width()), nav.height()), Sense::hover());
        let mut x = row.left();
        for (i, (name, _icon)) in PAGES.iter().enumerate() {
            let w = kit::text_width(ui, name, theme::body()) + 22.0;
            let rr = Rect::from_min_size(egui::pos2(x, row.top()), Vec2::new(w, row.height()));
            x += w + 4.0;
            let resp = ui.interact(rr, egui::Id::unique(("settings-tab", i)), Sense::click());
            let st = kit::State::from_response(ui, &resp, None);
            let sel = app.settings.page == i;
            let p = ui.painter();
            let cr = CornerRadius::same(theme::radius::CONTROL);
            if sel {
                p.rect_filled(rr, cr, t.accent_soft);
            } else if st.hover > 0.0 {
                p.rect_filled(rr, cr, theme::alpha(t.alt, st.hover));
            }
            p.text(rr.center(), Align2::CENTER_CENTER, *name, theme::body(), if sel { t.accent } else { t.text });
            if st.focused {
                kit::focus_ring(p, rr, cr, &t);
            }
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, sel, *name));
            if resp.clicked() {
                app.settings.page = i;
            }
        }
    });
    let _ = icons::paint;
    let nav = Rect::from_min_max(egui::pos2(r.left(), r.top()), egui::pos2(r.left(), nav.bottom() + 8.0));
    let content = Rect::from_min_max(egui::pos2(r.left(), nav.bottom()), r.max);
    let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(content).id_salt(("settings-page", app.settings.page)));
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_v(&mut cui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(20, 16)).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = theme::space::SECTION_GAP;
            match app.settings.page {
                0 => general(app, ui),
                1 => playback(app, ui),
                2 => storage(app, ui),
                3 => shortcuts(app, ui),
                4 => ai(app, ui),
                5 => models(app, ui),
                #[cfg(feature = "commerce")]
                6 => crate::screens::account::page(&mut app.settings.account, ui),
                _ => about(app, ui),
            }
        });
    });
}

fn note(ui: &mut egui::Ui, text: &str) {
    let t = theme::tokens(ui.ctx());
    ui.add_space(-4.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().interact_size.y = 14.0;
        ui.add_space(kit::LABEL_W);
        ui.add(egui::Label::new(egui::RichText::new(text).font(theme::caption()).color(t.text_2)).wrap());
    });
}

fn general(app: &mut StudioApp, ui: &mut egui::Ui) {
    let p = &mut app.prefs;
    Section::new("set-appearance", "Appearance").icon(Icon::Sun).fixed().show(ui, |ui| {
        kit::labeled_row(ui, "Theme", |ui| {
            let mut i = match p.theme { ThemeChoice::Dark => 0, ThemeChoice::Light => 1, ThemeChoice::System => 2 };
            if kit::segmented(ui, "set-theme", &mut i, &[Segment::icon_text(Icon::Moon, "Dark"), Segment::icon_text(Icon::Sun, "Light"), Segment::text("System")], Some(260.0), None).changed() {
                p.theme = [ThemeChoice::Dark, ThemeChoice::Light, ThemeChoice::System][i];
            }
        });
        kit::labeled_row(ui, "Language", |ui| {
            let langs = studio_i18n::Lang::ALL;
            let mut names: Vec<&str> = vec!["System default"];
            names.extend(langs.iter().map(|l| l.native_name()));
            let cur = langs.iter().position(|l| l.tag() == p.language).map_or(0, |i| i + 1);
            if let Some(i) = kit::menu_button(ui, "set-language", cur, &names, 220.0, None) {
                p.language = if i == 0 { String::new() } else { langs[i - 1].tag().to_owned() };
            }
        });
        let mut scale = p.ui_scale * 100.0;
        let out = FieldRow::new("Interface size", &mut scale, 80.0..=200.0).default(100.0).unit("%").step(5.0).scrub(0.5).show(ui);
        // Apply when the gesture ends so the slider does not move under
        // the pointer while the whole UI rescales.
        if out.changed {
            ui.data_mut(|d| d.insert_temp(egui::Id::unique("pending-scale"), scale));
        }
        let pending: Option<f32> = ui.data(|d| d.get_temp(egui::Id::unique("pending-scale")));
        if let Some(s) = pending {
            if !ui.input(|i| i.pointer.any_down()) && !ui.ctx().egui_wants_keyboard_input() {
                p.ui_scale = s / 100.0;
                ui.data_mut(|d| d.remove::<f32>(egui::Id::unique("pending-scale")));
            }
        }
        kit::labeled_row(ui, "Reduce motion", |ui| {
            kit::toggle(ui, &mut p.reduce_motion, "Reduce motion", None);
        });
        note(ui, "Turns off hover, panel and popover animations.");
    });
    Section::new("set-access", "Accessibility").icon(Icon::Eye).fixed().show(ui, |ui| {
        kit::labeled_row(ui, "High contrast", |ui| {
            kit::toggle(ui, &mut p.high_contrast, "High contrast", None);
        });
        note(ui, "Black or white backgrounds, pure text and strong outlines.");
        kit::labeled_row(ui, "Colour vision", |ui| {
            let mut i = p.colour_vision as usize;
            if kit::segmented(ui, "set-colour-vision", &mut i, &[Segment::text("Standard"), Segment::text("Red-green safe"), Segment::text("Blue-yellow safe")], Some(360.0), None).changed() {
                p.colour_vision = crate::prefs::ColourVision::from_u8(i as u8);
            }
        });
        note(ui, "Clip and status colours that stay apart for colour blindness.");
        kit::labeled_row(ui, "Spoken updates", |ui| {
            kit::toggle(ui, &mut p.announce, "Spoken updates", None);
        });
        note(ui, "Tell a screen reader what changed: undo, split, selection, export.");
        kit::labeled_row(ui, "Tips", |ui| {
            kit::toggle(ui, &mut p.tips_on, "Tips", None);
        });
        note(ui, "Short hints about what you can do next, one at a time.");
        ui.horizontal(|ui| {
            ui.add_space(kit::LABEL_W);
            if Button::secondary("Replay the welcome tour").show(ui).clicked() {
                p.tour_done = false;
                p.tips_seen.clear();
            }
        });
    });
    Section::new("set-editing", "Editing").icon(Icon::Scissors).fixed().show(ui, |ui| {
        kit::labeled_row(ui, "Hover scrub", |ui| {
            kit::toggle(ui, &mut p.hover_scrub, "Hover scrub", None);
        });
        note(ui, "Preview frames under the pointer in the timeline without moving the playhead.");
        kit::labeled_row(ui, "Autosave", |ui| {
            kit::toggle(ui, &mut p.autosave, "Autosave", None);
        });
        note(ui, "Saves 30 seconds after the last change and keeps 20 backups.");
    });
}

fn playback(app: &mut StudioApp, ui: &mut egui::Ui) {
    let p = &mut app.prefs;
    let ed = &mut app.editor;
    Section::new("set-playback", "Playback").icon(Icon::MonitorPlay).fixed().show(ui, |ui| {
        FieldRow::new("Frame cache", &mut p.cache_gb, 1.0..=64.0).default(8.0).unit("GB").decimals(0).step(1.0).scrub(0.1).show(ui);
        note(ui, "Memory kept for decoded frames. More cache keeps scrubbing smooth on long projects.");
        kit::labeled_row(ui, "Preview quality", |ui| {
            if let Some(i) = kit::menu_button(ui, "set-quality", ed.quality, &["Full", "Half", "Quarter"], 160.0, None) {
                ed.quality = i;
            }
        });
        note(ui, "Zornov never lowers preview quality by itself.");
        kit::labeled_row(ui, "Preview engine", |ui| {
            let mut i = p.preview_engine as usize;
            if kit::segmented(ui, "set-engine", &mut i, &[Segment::text("Zornov render"), Segment::text("Simple")], Some(260.0), None).changed() {
                p.preview_engine = i as u8;
            }
        });
        note(ui, "Simple preview draws each clip on its own and is lighter on older machines.");
        kit::labeled_row(ui, "Audio output", |ui| {
            kit::menu_button(ui, "set-audio", 0, &["System default"], 220.0, None);
        });
    });
}

fn storage(app: &mut StudioApp, ui: &mut egui::Ui) {
    let t = theme::tokens(ui.ctx());
    let p = &mut app.prefs;
    let mut clean = false;
    Section::new("set-storage", "Folders").icon(Icon::Folder).fixed().show(ui, |ui| {
        for (label, path) in [("Cache", &p.cache_dir), ("Proxies", &p.proxy_dir)] {
            kit::labeled_row(ui, label, |ui| {
                let (rr, _) = ui.allocate_exact_size(Vec2::new(ui.available_width() - 92.0, 28.0), Sense::hover());
                ui.painter().rect_filled(rr, CornerRadius::same(6), kit::field_fill(ui));
                icons::paint(ui.painter(), Rect::from_center_size(egui::pos2(rr.left() + 16.0, rr.center().y), Vec2::splat(16.0)), Icon::Folder, 16.0, t.text_2);
                kit::paint_text_clipped(ui.painter(), egui::pos2(rr.left() + 32.0, rr.center().y), Align2::LEFT_CENTER, &studio_paths::tilde(std::path::Path::new(path.as_str())), theme::control(), t.text, rr.width() - 40.0);
                if Button::secondary("Change…").compact().show(ui).clicked() {
                    kit::push(ui.ctx(), "Folder picker arrives with file dialogs", None, Some(Icon::Info));
                }
            });
        }
    });
    Section::new("set-usage", "Disk use").icon(Icon::HardDrives).fixed().show(ui, |ui| {
        for (label, used, cap) in [("Frame cache", 3.4f32, p.cache_gb), ("Proxies", 12.8, 64.0), ("Thumbnails", 0.6, 4.0)] {
            kit::labeled_row(ui, label, |ui| {
                let (rr, _) = ui.allocate_exact_size(Vec2::new(ui.available_width() - 92.0, 28.0), Sense::hover());
                let bar = Rect::from_min_size(egui::pos2(rr.left(), rr.center().y - 3.0), Vec2::new(rr.width(), 6.0));
                ui.painter().rect_filled(bar, CornerRadius::same(3), kit::field_fill(ui));
                ui.painter().rect_filled(Rect::from_min_size(bar.min, Vec2::new(bar.width() * (used / cap).min(1.0), 6.0)), CornerRadius::same(3), t.accent);
                ui.label(egui::RichText::new(format!("{used:.1} GB")).font(theme::body()).color(t.text_2));
            });
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(kit::LABEL_W);
            clean = Button::secondary("Clean up cache").icon(Icon::Trash).show(ui).clicked();
        });
        note(ui, "Removes cached frames, waveforms and proxies. Projects and media are not touched.");
    });
    if clean {
        let ctx = ui.ctx().clone();
        crate::screens::library_safety::clean_for(app, &ctx);
    }
}

fn key_badge(ui: &mut egui::Ui, text: &str, active: bool) -> egui::Response {
    let t = theme::tokens(ui.ctx());
    let w = kit::text_width(ui, text, theme::body_medium()).max(14.0) + 16.0;
    let (r, resp) = ui.allocate_exact_size(Vec2::new(w, 24.0), Sense::click());
    let st = kit::State::from_response(ui, &resp, None);
    let p = ui.painter();
    let fill = if active { t.accent_soft } else { theme::mix(kit::field_fill(ui), t.hover(kit::field_fill(ui)), st.hover) };
    p.rect(r, CornerRadius::same(5), fill, egui::Stroke::new(1.0, if active { t.accent } else { t.line }), egui::StrokeKind::Inside);
    p.text(r.center(), Align2::CENTER_CENTER, text, theme::body_medium(), if active { t.accent } else { t.text });
    if st.focused {
        kit::focus_ring(p, r, CornerRadius::same(5), &t);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn shortcuts(app: &mut StudioApp, ui: &mut egui::Ui) {
    let t = theme::tokens(ui.ctx());
    let w = ui.available_width();
    ui.horizontal(|ui| {
        kit::text_field(ui, &mut app.settings.shortcut_query, "Search actions or keys", Some(Icon::MagnifyingGlass), w - 140.0, None);
        if Button::ghost("Reset all").icon(Icon::ArrowCounterClockwise).show(ui).clicked() {
            app.prefs.shortcuts.clear();
            kit::push(ui.ctx(), "Shortcuts reset to defaults", None, None);
        }
    });
    ui.add_space(4.0);
    let q = app.settings.shortcut_query.to_lowercase();
    // Capture a new key combination.
    if let Some(i) = app.settings.capturing {
        let ev = ui.input(|inp| {
            inp.events.iter().find_map(|e| match e {
                egui::Event::Key { key, pressed: true, modifiers, .. } => Some((*key, *modifiers)),
                _ => None,
            })
        });
        if let Some((key, m)) = ev {
            if key == egui::Key::Escape {
                app.settings.capturing = None;
            } else {
                let mut s = String::new();
                if m.ctrl {
                    s.push('⌃');
                }
                if m.alt {
                    s.push('⌥');
                }
                if m.shift {
                    s.push('⇧');
                }
                if m.mac_cmd || m.command {
                    s.push('⌘');
                }
                s.push_str(key.symbol_or_name());
                let id = SHORTCUTS[i].0.to_owned();
                app.prefs.shortcuts.retain(|(a, _)| *a != id);
                app.prefs.shortcuts.push((id, s));
                app.settings.capturing = None;
            }
        }
    }
    egui::Frame::new().fill(t.raised).corner_radius(CornerRadius::same(theme::radius::CARD)).inner_margin(egui::Margin::symmetric(12, 6)).show(ui, |ui| {
        kit::on_card(ui);
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut shown = 0;
        for (i, (id, name, default)) in SHORTCUTS.iter().enumerate() {
            let current = app.prefs.shortcuts.iter().find(|(a, _)| a == id).map(|(_, k)| k.clone()).unwrap_or_else(|| default.to_string());
            if !q.is_empty() && !name.to_lowercase().contains(&q) && !current.to_lowercase().contains(&q) {
                continue;
            }
            shown += 1;
            kit::name_row(ui, egui::RichText::new(*name).font(theme::control()).color(t.text), |ui| {
                ui.set_min_height(theme::space::ROW_H);
                {
                    let capturing = app.settings.capturing == Some(i);
                    let resp = key_badge(ui, if capturing { "Press keys…" } else { &current }, capturing);
                    if resp.clicked() {
                        app.settings.capturing = Some(i);
                    }
                    kit::tip(resp, "Click, then press the new shortcut", Some("Esc cancels"));
                    if current != *default {
                        ui.label(egui::RichText::new(format!("default {default}")).font(theme::caption()).color(t.text_2));
                    }
                    // Two actions on one key: only the first would run.
                    let clash = crate::keymap::clashes_with(&app.prefs, id);
                    if !clash.is_empty() {
                        ui.label(egui::RichText::new(format!("also {}", clash.iter().map(|c| crate::keymap::name(c)).collect::<Vec<_>>().join(", "))).font(theme::caption()).color(t.accent));
                    }
                }
            });
        }
        if shown == 0 {
            ui.add_space(8.0);
            ui.label(egui::RichText::new(crate::i18n::shown("No actions match.")).font(theme::body()).color(t.text_2));
            ui.add_space(8.0);
        }
    });
}

fn ai(app: &mut StudioApp, ui: &mut egui::Ui) {
    use crate::agent::{brain, keychain, openai};
    let p = &mut app.prefs;
    let st = &mut app.settings;
    Section::new("set-ai-editor", "AI editor").icon(Icon::Sparkle).fixed().show(ui, |ui| {
        kit::labeled_row(ui, "Answered by", |ui| {
            let mut i = p.ai_provider as usize;
            if kit::segmented(ui, "set-ai-provider", &mut i, &[Segment::text("Claude API"), Segment::text("Claude Code"), Segment::text("OpenAI API"), Segment::text("Codex")], Some(420.0), None).changed() {
                p.ai_provider = i as u8;
                st.ai_key.clear();
                st.ai_key_note = None;
                st.ai_key_saved = None;
            }
        });
        // The command-line agents use their own sign-in.
        let cli = match p.ai_provider {
            1 => Some(("Claude Code", brain::find_claude(), "npm install -g @anthropic-ai/claude-code")),
            3 => Some(("Codex", brain::find_codex(), "npm install -g @openai/codex, then codex login")),
            _ => None,
        };
        if let Some((name, found, install)) = cli {
            // The CLI's own default, or one of the models it can run.
            let (models, chosen) = if p.ai_provider == 1 { (brain::MODELS, &mut p.ai_claude_code_model) } else { (openai::MODELS, &mut p.ai_codex_model) };
            kit::labeled_row(ui, "Model", |ui| {
                let mut i = models.iter().position(|m| m.0 == chosen.as_str()).map_or(0, |i| i + 1);
                let segs: Vec<Segment> = std::iter::once(Segment::text("CLI default")).chain(models.iter().map(|m| Segment::text(m.1))).collect();
                if kit::segmented(ui, "set-ai-cli-model", &mut i, &segs, Some(420.0), None).changed() {
                    *chosen = if i == 0 { String::new() } else { models[i - 1].0.to_owned() };
                }
            });
            note(ui, &match found {
                Some(path) => crate::i18n::tf(format!("Uses your {name} sign-in ({}). Zornov's tools are connected to this window.", path.display())),
                None => format!("{name} is not installed. Install it ({install}), or pick another AI editor."),
            });
        } else {
            let (models, chosen, account, hint, from) = if p.ai_provider == 2 {
                (openai::MODELS, &mut p.ai_openai_model, keychain::OPENAI, "sk-…", "platform.openai.com")
            } else {
                (brain::MODELS, &mut p.ai_model, keychain::ANTHROPIC, "sk-ant-…", "console.anthropic.com")
            };
            kit::labeled_row(ui, "Model", |ui| {
                let mut i = models.iter().position(|m| m.0 == chosen.as_str()).unwrap_or(0);
                let segs: Vec<Segment> = models.iter().map(|m| Segment::text(m.1)).collect();
                if kit::segmented(ui, "set-ai-model", &mut i, &segs, Some(380.0), None).changed() {
                    *chosen = models[i].0.to_owned();
                }
            });
            note(ui, "The first is the strongest editor; the last is quickest and cheapest.");
            let saved = *st.ai_key_saved.get_or_insert_with(|| keychain::api_key(account).is_some());
            kit::labeled_row(ui, "API key", |ui| {
                if saved {
                    ui.label(egui::RichText::new(crate::i18n::shown("Saved in the Keychain")).font(theme::body()));
                    if Button::secondary("Remove").icon(Icon::Trash).show(ui).clicked() {
                        keychain::remove_api_key(account);
                        st.ai_key_saved = Some(false);
                        st.ai_key_note = Some("Key removed.".into());
                    }
                } else {
                    ui.add(egui::TextEdit::singleline(&mut st.ai_key).password(true).hint_text(hint).desired_width(220.0));
                    if Button::primary("Save").show(ui).clicked() {
                        match keychain::set_api_key(account, &st.ai_key) {
                            Ok(()) => {
                                st.ai_key.clear();
                                st.ai_key_saved = Some(true);
                                st.ai_key_note = Some("Key saved in the Keychain.".into());
                            }
                            Err(e) => st.ai_key_note = Some(e),
                        }
                    }
                }
            });
            let default_note = crate::i18n::tf(format!("From {from}. Kept in your login Keychain; Zornov never shows it again."));
            note(ui, st.ai_key_note.as_deref().unwrap_or(&default_note));
        }
        kit::labeled_row(ui, "Changes", |ui| {
            let mut i = p.ai_ask_first as usize;
            if kit::segmented(ui, "set-ai-mode", &mut i, &[Segment::text("Auto"), Segment::text("Ask first")], Some(260.0), None).changed() {
                p.ai_ask_first = i == 1;
            }
        });
        note(ui, "Auto: edits land on the timeline as they are made. Ask first: you approve each change. Export, opening another project and saving elsewhere are always asked.");
    });
    voice_and_sound(st, ui);
    generated_media(ui);
    lessons_section(st, ui);
    Section::new("set-ai-privacy", "What is sent").icon(Icon::Info).fixed().show(ui, |ui| {
        note(ui, "The AI editor is Claude (Anthropic) or ChatGPT / Codex (OpenAI): your requests, descriptions of the project and footage, and frames it looks at go to the one you pick. Media files are never uploaded, except a picture you ask to have a new picture generated from (it goes to Codex, see Generated media). The AI tools (captions, background removal, voice cleanup…) run on this computer.");
        kit::labeled_row(ui, "AI editor on", |ui| {
            kit::toggle(ui, &mut p.ai_cloud_ok, "AI editor on", None);
        });
    });
}

/// What the AI editor has learned: the lessons it saved after jobs and
/// corrections (they load with the matching skill), to read, edit and
/// delete. They live in the Studio data folder, never in a project.
fn lessons_section(st: &mut SettingsState, ui: &mut egui::Ui) {
    use crate::agent::lessons::{Store, now};
    let t = theme::tokens(ui.ctx());
    let clock = ui.input(|i| i.time);
    let store = Store::user();
    if st.lessons.is_none() || clock - st.lessons_at > 2.0 {
        st.lessons = Some(store.all(now()));
        st.lessons_at = clock;
    }
    let all = st.lessons.clone().unwrap_or_default();
    let mut changed = false;
    Section::new("set-ai-lessons", "What the AI has learned").icon(Icon::Sparkle).fixed().show(ui, |ui| {
        note(ui, "After a job, and after you correct it, the AI saves a short lesson. A lesson loads with its skill the next time that kind of work starts. Lessons are kept on this computer in your Zornov folder, never in a project. Edit one to make it yours (it is then kept for good), or delete what is wrong.");
        if all.is_empty() {
            note(ui, "Nothing learned yet.");
            return;
        }
        for l in &all {
            egui::Frame::new().fill(t.raised).corner_radius(CornerRadius::same(theme::radius::CARD)).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
                kit::on_card(ui);
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 4.0;
                let mut meta = format!("{} · {}", l.skill, l.kind.name());
                if l.seen > 1 {
                    meta.push_str(&format!(" · seen {} times", l.seen));
                }
                if l.mine {
                    meta.push_str(" · yours");
                }
                ui.label(egui::RichText::new(meta).font(theme::caption()).color(t.text_2));
                let editing = st.lesson_edit.as_ref().is_some_and(|(id, _)| *id == l.id);
                if editing {
                    let w = ui.available_width();
                    let mut done: Option<bool> = None;
                    if let Some((_, text)) = st.lesson_edit.as_mut() {
                        kit::text_field(ui, text, "What to do next time", None, w, None);
                    }
                    ui.horizontal(|ui| {
                        if Button::primary("Save").compact().show(ui).clicked() {
                            done = Some(true);
                        }
                        if Button::ghost("Cancel").compact().show(ui).clicked() {
                            done = Some(false);
                        }
                    });
                    match done {
                        Some(true) => {
                            let text = st.lesson_edit.take().map(|(_, x)| x).unwrap_or_default();
                            match store.edit(l.id, &text, now()) {
                                Ok(()) => changed = true,
                                Err(e) => {
                                    st.lesson_edit = Some((l.id, text));
                                    kit::push(ui.ctx(), e, None, Some(Icon::Warning));
                                }
                            }
                        }
                        Some(false) => st.lesson_edit = None,
                        None => {}
                    }
                } else {
                    ui.add(egui::Label::new(egui::RichText::new(crate::i18n::shown(&l.text)).font(theme::body()).color(t.text)).wrap());
                    ui.horizontal(|ui| {
                        if Button::ghost("Edit").compact().show(ui).clicked() {
                            st.lesson_edit = Some((l.id, l.text.clone()));
                        }
                        if Button::ghost("Delete").icon(Icon::Trash).compact().show(ui).clicked() {
                            match store.forget(l.id) {
                                Ok(()) => {
                                    changed = true;
                                    kit::push(ui.ctx(), "Lesson deleted", None, Some(Icon::Trash));
                                }
                                Err(e) => {
                                    kit::push(ui.ctx(), e, None, Some(Icon::Warning));
                                }
                            }
                        }
                    });
                }
            });
            ui.add_space(4.0);
        }
        ui.horizontal(|ui| {
            ui.add_space(kit::LABEL_W);
            if st.lesson_clear {
                ui.label(egui::RichText::new(crate::i18n::tf(format!("Delete all {} lessons?", all.len()))).font(theme::caption()).color(t.text_2));
                if Button::secondary("Delete all").icon(Icon::Trash).compact().show(ui).clicked() {
                    match store.clear(None) {
                        Ok(n) => {
                            changed = true;
                            kit::push(ui.ctx(), crate::i18n::tf(format!("{n} lessons deleted")), None, Some(Icon::Trash));
                        }
                        Err(e) => {
                            kit::push(ui.ctx(), e, None, Some(Icon::Warning));
                        }
                    }
                    st.lesson_clear = false;
                }
                if Button::ghost("Keep them").compact().show(ui).clicked() {
                    st.lesson_clear = false;
                }
            } else if Button::ghost("Delete all lessons").compact().show(ui).clicked() {
                st.lesson_clear = true;
            }
        });
    });
    if changed {
        st.lessons_at = f64::NEG_INFINITY;
        ui.ctx().request_repaint();
    }
}

/// Who speaks narration, with which voice and key; the switch that keeps
/// the Mac's voices out; and the key described sounds need.
fn voice_and_sound(st: &mut SettingsState, ui: &mut egui::Ui) {
    use crate::agent::keychain;
    use crate::agent::voice_service::{Provider, VoiceSettings, forced_off};
    let mut v = st.voice.take().unwrap_or_else(VoiceSettings::load);
    let before = v.clone();
    Section::new("set-ai-voice", "Voice & sound").icon(Icon::Microphone).fixed().show(ui, |ui| {
        kit::labeled_row(ui, "Spoken by", |ui| {
            let mut i = Provider::ALL.iter().position(|p| *p == v.provider).unwrap_or(0);
            let segs: Vec<Segment> = Provider::ALL.iter().map(|p| Segment::text(p.name())).collect();
            if kit::segmented(ui, "set-voice-provider", &mut i, &segs, Some(420.0), None).changed() {
                v.provider = Provider::ALL[i];
                st.voice_key.clear();
                st.voice_key_note = None;
            }
        });
        if v.provider == Provider::Mac {
            note(ui, &if v.mac_voice_allowed() {
                "Narration uses the voices installed on this Mac (Premium voices sound best: System Settings ▸ Accessibility ▸ Spoken Content).".to_owned()
            } else {
                "The Mac's voices are switched off below, so narration cannot be made until you choose a service.".to_owned()
            });
        } else {
            let (voice, model, voice_hint, model_hint, from) = match v.provider {
                Provider::Fish => (&mut v.fish_voice, &mut v.fish_model, "voice model id (empty: Fish's default)", "s2.1-pro", "fish.audio ▸ API keys; voice ids are on each voice's page"),
                Provider::ElevenLabs => (&mut v.elevenlabs_voice, &mut v.elevenlabs_model, "voice id", "eleven_multilingual_v2", "elevenlabs.io ▸ Developers ▸ API keys; voice ids in the Voice Library"),
                _ => (&mut v.openai_voice, &mut v.openai_model, "alloy, ash, coral, nova, sage…", "gpt-4o-mini-tts", "platform.openai.com (the same key as the OpenAI AI editor)"),
            };
            kit::labeled_row(ui, "Voice", |ui| {
                ui.add(egui::TextEdit::singleline(voice).hint_text(voice_hint).desired_width(260.0));
            });
            kit::labeled_row(ui, "Model", |ui| {
                ui.add(egui::TextEdit::singleline(model).hint_text(model_hint).desired_width(260.0));
            });
            let account = v.provider.key_account().expect("a service has a key");
            let saved = match st.voice_key_saved {
                Some((p, s)) if p == v.provider => s,
                _ => {
                    let s = keychain::api_key(account).is_some();
                    st.voice_key_saved = Some((v.provider, s));
                    s
                }
            };
            key_row(ui, account, saved, &mut st.voice_key, &mut st.voice_key_note, |s| st.voice_key_saved = Some((v.provider, s)));
            let default_note = crate::i18n::tf(format!("Key from {from}. Kept in your login Keychain. If this service fails, the line fails and says why: nothing falls back to another voice."));
            note(ui, st.voice_key_note.as_deref().unwrap_or(&default_note));
        }
        kit::labeled_row(ui, "Never use Mac voices", |ui| {
            if forced_off() {
                ui.label(egui::RichText::new(crate::i18n::shown("Off by STUDIO_NO_SYSTEM_VOICE")).font(theme::body()));
            } else {
                kit::toggle(ui, &mut v.mac_voice_off, "Never use Mac voices", None);
            }
        });
        note(ui, "On: no agent can speak with the Mac's built-in voices, even by asking for one by name; narration comes only from the service above, or not at all. Holds for the AI editor, the studio command and Claude Code / Codex alike. STUDIO_NO_SYSTEM_VOICE=1 in the environment forces it on.");
        let sound_saved = *st.sound_key_saved.get_or_insert_with(|| keychain::api_key(keychain::ELEVENLABS).is_some());
        kit::labeled_row(ui, "Described sounds", |ui| {
            ui.label(egui::RichText::new(if sound_saved { "ElevenLabs key saved: any sound in words" } else { "Need an ElevenLabs key (choose ElevenLabs above to add it)" }).font(theme::body()));
        });
        note(ui, "Risers, whooshes, booms and hits are made by Zornov itself. Any other sound described in words (\"rain on a tin roof\") comes from ElevenLabs.");
    });
    if v != before {
        if let Err(e) = v.save() {
            st.voice_key_note = Some(crate::i18n::tf(format!("Could not save: {e}")));
        }
    }
    st.voice = Some(v);
}

/// How pictures are generated: by Codex, with no key. Video, music and
/// sound from words are not available yet.
fn generated_media(ui: &mut egui::Ui) {
    Section::new("set-ai-generate", "Generated media").icon(Icon::Sparkle).fixed().show(ui, |ui| {
        let codex = crate::agent::brain::find_codex().is_some();
        kit::labeled_row(ui, "Pictures", |ui| {
            ui.label(egui::RichText::new(crate::i18n::shown(if codex { "Codex image generation (your ChatGPT plan)" } else { "Needs the Codex app on this Mac" })).font(theme::body()));
        });
        kit::labeled_row(ui, "Video, music, sound", |ui| {
            ui.label(egui::RichText::new(crate::i18n::shown("Not available yet")).font(theme::body()));
        });
        note(ui, "Pictures the AI editor makes from words come from the Codex app's image generation, on your own ChatGPT plan: no API key, nothing billed by Zornov. Video, music and sound effects made from words are not available yet; Zornov's library music, made-to-measure sound effects and camera moves cover them for now.");
    });
}

/// A key's row: saved (with Remove) or a field to paste one into.
fn key_row(ui: &mut egui::Ui, account: &str, saved: bool, typed: &mut String, note_out: &mut Option<String>, mut set_saved: impl FnMut(bool)) {
    use crate::agent::keychain;
    kit::labeled_row(ui, "API key", |ui| {
        if saved {
            ui.label(egui::RichText::new(crate::i18n::shown("Saved in the Keychain")).font(theme::body()));
            if Button::secondary("Remove").icon(Icon::Trash).show(ui).clicked() {
                keychain::remove_api_key(account);
                set_saved(false);
                *note_out = Some("Key removed.".into());
            }
        } else {
            ui.add(egui::TextEdit::singleline(typed).password(true).hint_text("paste the key").desired_width(220.0));
            if Button::primary("Save").show(ui).clicked() {
                match keychain::set_api_key(account, typed.trim()) {
                    Ok(()) => {
                        typed.clear();
                        set_saved(true);
                        *note_out = Some("Key saved in the Keychain.".into());
                    }
                    Err(e) => *note_out = Some(e),
                }
            }
        }
    });
}

fn models(app: &mut StudioApp, ui: &mut egui::Ui) {
    use crate::services::models::{DownloadState, Models, VerifyState};
    use studio_ai::ModelStatus;
    let t = theme::tokens(ui.ctx());
    ui.label(egui::RichText::new(crate::i18n::shown("These AI features run on this computer. Models download once to Movies ▸ Studio ▸ Models and stay there; nothing is uploaded. (The AI editor in Settings ▸ AI is the exception: it is Claude or ChatGPT / Codex.)")).font(theme::body()).color(t.text_2));
    ui.add_space(4.0);
    // What is installed is read from the models folder itself (it was a
    // fixed list, and a finished "download" was forgotten on quitting).
    let now = ui.input(|i| i.time);
    let m = app.settings.models.get_or_insert_with(Models::default);
    if app.settings.model_rows.is_empty() || now - app.settings.model_rows_at > 1.0 {
        app.settings.model_rows = m.list();
        app.settings.model_rows_at = now;
    }
    let rows = app.settings.model_rows.clone();
    let mut reread = false;
    for row in &rows {
        let w = ui.available_width();
        let (r, _) = ui.allocate_exact_size(Vec2::new(w, 56.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(r, CornerRadius::same(theme::radius::CARD), t.raised);
        let ir = Rect::from_min_size(r.min + Vec2::new(12.0, 12.0), Vec2::splat(32.0));
        p.rect_filled(ir, CornerRadius::same(8), t.accent_soft);
        icons::paint(p, ir, Icon::Cpu, 16.0, t.accent);
        // Name and status end in an ellipsis before the buttons.
        let buttons_w = if row.status == ModelStatus::Installed { 232.0 } else { 128.0 };
        let text_w = r.right() - buttons_w - (ir.right() + 12.0);
        kit::paint_text_clipped(p, egui::pos2(ir.right() + 12.0, r.center().y - 8.0), Align2::LEFT_CENTER, row.name, theme::control_medium(), t.text, text_w);
        let state = m.download_state(row.id);
        let running = matches!(state, Some(DownloadState::Running { .. }));
        if matches!(state, Some(DownloadState::Done)) && row.status != ModelStatus::Installed {
            reread = true;
        }
        let check = m.verify_state(row.id);
        if check.is_some() && row.status != ModelStatus::Installed && !running {
            m.clear_verify(row.id);
        }
        let status = match (&state, &row.status) {
            (_, ModelStatus::Installed) if matches!(check, Some(VerifyState::Running { .. })) => {
                let f = if let Some(VerifyState::Running { fraction }) = check { fraction } else { 0.0 };
                crate::i18n::tf(format!("Checking files · {}%", (f * 100.0) as u32))
            }
            (_, ModelStatus::Installed) if check == Some(VerifyState::Ok) => format!("{} · installed · all files match", row.size_label),
            (_, ModelStatus::Installed) if matches!(check, Some(VerifyState::Damaged(_))) => {
                let bad = if let Some(VerifyState::Damaged(b)) = &check { b.join(", ") } else { String::new() };
                crate::i18n::tf(format!("Damaged: {bad} · repair it"))
            }
            (_, ModelStatus::Installed) if matches!(check, Some(VerifyState::Failed(_))) => {
                let e = if let Some(VerifyState::Failed(e)) = &check { e.clone() } else { String::new() };
                crate::i18n::tf(format!("Could not check the files: {e}"))
            }
            (Some(DownloadState::Running { fraction, .. }), _) => crate::i18n::tf(format!("Downloading · {}% of {}", (fraction * 100.0) as u32, row.size_label)),
            (Some(DownloadState::Failed(e)), s) if *s != ModelStatus::Installed => crate::i18n::tf(format!("Download failed: {e}")),
            (_, ModelStatus::Installed) => format!("{} · installed · for {}", row.size_label, row.used_for),
            (_, ModelStatus::Incomplete { .. }) => format!("{} · incomplete (an interrupted download): repair it", row.size_label),
            (_, ModelStatus::NotInstalled) => format!("{} · not downloaded · for {}", row.size_label, row.used_for),
        };
        if kit::paint_text_clipped(p, egui::pos2(ir.right() + 12.0, r.center().y + 9.0), Align2::LEFT_CENTER, &status, theme::caption(), t.text_2, text_w) {
            ui.interact(Rect::from_min_size(egui::pos2(ir.right() + 12.0, r.top()), Vec2::new(text_w.max(1.0), r.height())), egui::Id::unique(("model-row", row.id)), Sense::hover()).on_hover_text(format!("{}\n{status}", row.name));
        }
        let br = Rect::from_min_size(egui::pos2(r.right() - 116.0, r.center().y - 14.0), Vec2::new(104.0, 28.0));
        let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(br));
        kit::on_card(&mut bui);
        if running {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
            if Button::secondary("Cancel").icon(Icon::X).min_width(104.0).show(&mut bui).clicked() {
                m.cancel(row.id);
            }
        } else if row.status == ModelStatus::Installed {
            let vr = Rect::from_min_size(egui::pos2(br.left() - 112.0, br.top()), br.size());
            let mut vui = ui.new_child(egui::UiBuilder::new().max_rect(vr));
            kit::on_card(&mut vui);
            if matches!(check, Some(VerifyState::Running { .. })) {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
                if Button::secondary("Cancel").icon(Icon::X).min_width(104.0).show(&mut vui).clicked() {
                    m.clear_verify(row.id);
                }
            } else if matches!(check, Some(VerifyState::Damaged(_))) {
                if Button::secondary("Repair").icon(Icon::DownloadSimple).min_width(104.0).show(&mut vui).clicked() {
                    m.clear_verify(row.id);
                    m.download(row.id);
                    ui.ctx().request_repaint();
                }
            } else if Button::secondary("Verify").icon(Icon::Check).min_width(104.0).show(&mut vui).clicked() {
                m.verify(row.id);
                ui.ctx().request_repaint();
            }
            if Button::secondary("Remove").icon(Icon::Trash).min_width(104.0).show(&mut bui).clicked() {
                m.clear_verify(row.id);
                match m.remove(row.id) {
                    Ok(()) => {
                        kit::push(ui.ctx(), format!("{} removed", row.name), None, Some(Icon::Trash));
                    }
                    Err(e) => {
                        kit::push(ui.ctx(), crate::i18n::tf(format!("Could not remove {}: {e}", row.name)), None, Some(Icon::Info));
                    }
                }
                reread = true;
            }
        } else {
            let label = if matches!(row.status, ModelStatus::Incomplete { .. }) { "Repair" } else { "Download" };
            if Button::secondary(label).icon(Icon::DownloadSimple).min_width(104.0).show(&mut bui).clicked() {
                m.download(row.id);
                ui.ctx().request_repaint();
            }
        }
        ui.add_space(4.0);
    }
    if reread {
        app.settings.model_rows_at = f64::NEG_INFINITY;
        ui.ctx().request_repaint();
    }
}

fn about(app: &mut StudioApp, ui: &mut egui::Ui) {
    let t = theme::tokens(ui.ctx());
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 72.0), Sense::hover());
    crate::screens::start::wordmark(ui.painter(), r.left_center() - Vec2::new(0.0, 10.0), &t);
    ui.painter().text(egui::pos2(r.left(), r.center().y + 18.0), Align2::LEFT_CENTER, crate::i18n::tf(format!("Version {} · free and open source (AGPL-3.0-or-later) · works offline", env!("CARGO_PKG_VERSION"))), theme::body(), t.text_2);
    crate::release::settings_section(app, ui, &ui.ctx().clone());
    kit::menu_heading(ui, "Licences");
    for (i, l) in LICENCES.iter().enumerate() {
        let open = app.settings.open_licence == Some(i);
        egui::Frame::new().fill(t.raised).corner_radius(CornerRadius::same(theme::radius::CARD)).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            kit::on_card(ui);
            ui.horizontal(|ui| {
                // The button keeps its place at the end; the name and the
                // licence end in an ellipsis before it (whole on hover).
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if Button::ghost(if open { "Hide licence" } else { "View licence" }).compact().trailing(if open { Icon::CaretUp } else { Icon::CaretDown }).show(ui).clicked() {
                        app.settings.open_licence = if open { None } else { Some(i) };
                    }
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        kit::label_trunc(ui, egui::RichText::new(l.name).font(theme::control_medium()).color(t.text));
                        kit::label_trunc(ui, egui::RichText::new(format!("{} · {}", l.what, l.licence)).font(theme::caption()).color(t.text_2));
                    });
                });
            });
            if open {
                ui.add_space(6.0);
                egui::ScrollArea::vertical().id_salt(("licence", i)).max_height(200.0).show_v(ui, |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(l.text).font(theme::mono_small()).color(t.text_2)).wrap());
                });
            }
        });
    }
    ui.add_space(4.0);
    ui.label(egui::RichText::new(crate::i18n::shown("Library content shows its own source and licence in each item's info.")).font(theme::caption()).color(t.text_2));
}

#[cfg(all(test, not(feature = "commerce")))]
mod no_account_tests {
    #[test]
    fn settings_show_nothing_about_accounts() {
        assert!(super::PAGES.iter().all(|(n, _)| *n != "Account"));
        assert_eq!(super::PAGES.last().map(|p| p.0), Some("About"));
    }
}
