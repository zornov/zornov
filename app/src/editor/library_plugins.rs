// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Plugins in the AI library: under Installed, the installed plugins (turn
//! on or off, remove, open their panels); under Available, what can be
//! installed, and OpenFX. Effects and transitions of installed plugins are
//! in the Effects and Cuts tabs under Plugins.

use std::collections::HashMap;

use eframe::egui::{self, CornerRadius};
use serde_json::{Value, json};

use crate::app::StudioApp;
use crate::icons::Icon;
use crate::kit::{self, Button};
use crate::plugins::{self, catalogue, host};
use crate::theme;

/// An open plugin panel: its last view and its slider and toggle values.
#[derive(Clone, Default)]
struct PanelState {
    view: Value,
    values: HashMap<String, f32>,
    error: Option<String>,
}

fn panel_id(plugin: &str, panel: &str) -> egui::Id {
    egui::Id::unique(("plugin-panel", plugin.to_owned(), panel.to_owned()))
}

/// Draws the plugin cards for the AI library's chip (2 Installed, 3
/// Available) and macros under All tools (1).
pub fn show(app: &mut StudioApp, ui: &mut egui::Ui, chip: usize, query: &str) {
    match chip {
        1 => macros(app, ui, query),
        2 => installed(app, ui, query),
        3 => available(app, ui, query),
        _ => {}
    }
}

static LAST_MACRO: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

fn name_id() -> egui::Id {
    egui::Id::unique("macro-name")
}

/// Starts recording on the selection, or stops and saves (under the name
/// typed in the library, or "Macro N").
pub fn toggle_recording(app: &mut StudioApp, ctx: &egui::Context) {
    if crate::macros::recording() {
        let typed: String = ctx.data(|d| d.get_temp(name_id())).unwrap_or_default();
        let name = if typed.trim().is_empty() { format!("Macro {}", crate::macros::list().len() + 1) } else { typed.trim().to_owned() };
        if let Some(m) = crate::macros::stop(&name, "") {
            if m.steps.is_empty() {
                kit::push(ctx, "Stopped recording; nothing was changed, so no macro was kept", None, None);
                return;
            }
            match crate::macros::save(&m) {
                Ok(_) => {
                    *LAST_MACRO.lock().unwrap_or_else(|e| e.into_inner()) = Some(m.name.clone());
                    ctx.data_mut(|d| d.remove::<String>(name_id()));
                    kit::push(ctx, format!("Saved macro \"{}\" ({} steps)", m.name, m.steps.len()), None, Some(Icon::CheckCircle));
                }
                Err(e) => {
                    kit::push(ctx, e, None, Some(Icon::Warning));
                }
            }
        }
    } else {
        crate::macros::start(&app.doc, &app.editor.selection);
        let on = if app.editor.selection.is_empty() { "no clip selected: edits are kept as they are".to_owned() } else { format!("on {} selected clip{}; replay puts the same changes on other clips", app.editor.selection.len(), if app.editor.selection.len() == 1 { "" } else { "s" }) };
        kit::push(ctx, format!("Recording a macro, {on}"), None, Some(Icon::Circle));
    }
}

/// Runs the last macro recorded or run on the selection.
pub fn run_last_macro(app: &mut StudioApp, ctx: &egui::Context) {
    let last = LAST_MACRO.lock().unwrap_or_else(|e| e.into_inner()).clone().or_else(|| crate::macros::list().first().map(|m| m.name.clone()));
    match last {
        Some(name) => run_macro(app, ctx, &name, None),
        None => {
            kit::push(ctx, "No macros yet: record one first", None, None);
        }
    }
}

fn run_macro(app: &mut StudioApp, ctx: &egui::Context, name: &str, every: Option<bool>) {
    let r = crate::macros::load(name).and_then(|m| {
        let targets: Vec<u32> = if every == Some(true) { app.doc.project.clips.iter().filter(|c| c.is_visual()).map(|c| c.id).collect() } else { app.editor.selection.clone() };
        crate::macros::run(&mut app.doc, &m, &targets)
    });
    *LAST_MACRO.lock().unwrap_or_else(|e| e.into_inner()) = Some(name.to_owned());
    match r {
        Ok((n, refused)) if refused.is_empty() => kit::push(ctx, format!("Ran \"{name}\": {n} changes"), None, Some(Icon::CheckCircle)),
        Ok((n, refused)) => kit::push(ctx, format!("Ran \"{name}\": {n} changes, {} skipped ({})", refused.len(), refused[0]), None, Some(Icon::Warning)),
        Err(e) => kit::push(ctx, e, None, Some(Icon::Warning)),
    };
}

fn macros(app: &mut StudioApp, ui: &mut egui::Ui, q: &str) {
    let t = theme::tokens(ui.ctx());
    let ctx = ui.ctx().clone();
    heading(ui, "Macros");
    card(ui, |ui| {
        let rec = crate::macros::recording();
        kit::name_row(ui, egui::RichText::new(if rec { "Recording…" } else { "Record a macro" }).font(theme::control_medium()).color(if rec { t.accent } else { t.text }), |ui| {
            let keys = crate::keymap::keys(&app.prefs, "macro-record").unwrap_or("").to_owned();
            if Button::secondary(if rec { "Stop and save" } else { "Record" }).icon(if rec { Icon::Square } else { Icon::Circle }).show(ui).on_hover_text(keys).clicked() {
                toggle_recording(app, &ctx);
            }
        });
        if rec {
            let mut name: String = ctx.data(|d| d.get_temp(name_id())).unwrap_or_default();
            kit::text_field(ui, &mut name, "Name the macro", None, ui.available_width(), None);
            ctx.data_mut(|d| d.insert_temp(name_id(), name));
        } else {
            ui.label(egui::RichText::new("Select a clip, press Record, make your changes (or have the AI make them), then Stop. Run it on other clips, or on every clip.").font(theme::body()).color(t.text_2));
        }
    });
    for m in crate::macros::list() {
        if !matches(q, &[&m.name, &m.description]) {
            continue;
        }
        card(ui, |ui| {
            kit::name_row(ui, egui::RichText::new(&m.name).font(theme::control_medium()).color(t.text), |ui| {
                if Button::ghost("Delete").icon(Icon::Trash).show(ui).clicked() {
                    let _ = crate::macros::delete(&m.name);
                }
                if Button::secondary("Every clip").show(ui).clicked() {
                    run_macro(app, &ctx, &m.name, Some(true));
                }
                if Button::secondary("Run").icon(Icon::PlayFill).show(ui).clicked() {
                    run_macro(app, &ctx, &m.name, None);
                }
            });
            let what = crate::macros::json(&m)["what"].as_array().map(|a| a.iter().filter_map(Value::as_str).take(4).collect::<Vec<_>>().join(", ")).unwrap_or_default();
            ui.label(egui::RichText::new(format!("{} steps: {what}", m.steps.len())).font(theme::caption()).color(t.text_2));
        });
    }
}

fn heading(ui: &mut egui::Ui, text: &str) {
    let t = theme::tokens(ui.ctx());
    ui.add_space(8.0);
    ui.label(egui::RichText::new(text).font(theme::body()).color(t.text));
    ui.add_space(2.0);
}

fn card(ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui)) {
    let t = theme::tokens(ui.ctx());
    egui::Frame::new().fill(t.raised).corner_radius(CornerRadius::same(theme::radius::CARD)).inner_margin(egui::Margin::symmetric(12, 10)).show(ui, |ui| {
        kit::on_card(ui);
        ui.set_width(ui.available_width());
        body(ui);
    });
    ui.add_space(4.0);
}

fn matches(q: &str, fields: &[&str]) -> bool {
    q.is_empty() || fields.iter().any(|f| f.to_lowercase().contains(q))
}

fn installed(app: &mut StudioApp, ui: &mut egui::Ui, q: &str) {
    let t = theme::tokens(ui.ctx());
    let list = host().list();
    heading(ui, "Plugins");
    let ctx = ui.ctx().clone();
    if app.cli.shot.is_none() && Button::ghost("Install from file…").icon(Icon::FolderOpen).show(ui).clicked() {
        if let Some(path) = rfd::FileDialog::new().add_filter("Zornov plugin", &["wasm"]).pick_file() {
            match host().install_file(&path) {
                Ok(p) => kit::push(&ctx, format!("Installed {} ({})", p.manifest.name, p.manifest.summary()), None, Some(Icon::CheckCircle)),
                Err(e) => kit::push(&ctx, e, None, Some(Icon::Warning)),
            };
        }
    }
    if list.is_empty() {
        ui.label(egui::RichText::new("No plugins yet. Available lists the ones you can add; the AI can find or write one for you.").font(theme::caption()).color(t.text_2));
        return;
    }
    for p in list {
        let m = &p.manifest;
        if !matches(q, &[&m.name, &m.description, &m.id]) {
            continue;
        }
        card(ui, |ui| {
            kit::name_row(ui, egui::RichText::new(format!("{}  {}", m.name, m.version)).font(theme::control_medium()).color(t.text), |ui| {
                if Button::ghost("Remove").icon(Icon::Trash).show(ui).clicked() {
                    app.editor.library_selected = None;
                    match host().uninstall(&m.id) {
                        Ok(()) => kit::push(&ctx, format!("Removed {}", m.name), None, Some(Icon::Trash)),
                        Err(e) => kit::push(&ctx, e, None, Some(Icon::Warning)),
                    };
                }
                if p.error.is_none() {
                    let label = if p.enabled { "Turn off" } else { "Turn on" };
                    if Button::secondary(label).show(ui).clicked() {
                        app.editor.library_selected = None;
                        if let Err(e) = host().set_enabled(&m.id, !p.enabled) {
                            kit::push(&ctx, e, None, Some(Icon::Warning));
                        }
                    }
                }
            });
            let what = match &p.error {
                Some(e) => format!("Cannot run: {e}"),
                None if !p.enabled => format!("Off. {}", m.summary()),
                None => format!("{} {}", m.description, if m.effects.len() + m.generators.len() + m.transitions.len() > 0 { "Its effects are in Effects ▸ Plugins, its transitions in Cuts ▸ Plugins." } else { "" }),
            };
            ui.label(egui::RichText::new(what).font(theme::body()).color(t.text_2));
            for w in &p.warnings {
                ui.label(egui::RichText::new(w).font(theme::caption()).color(t.text_2));
            }
            if p.enabled {
                for panel in &m.panels {
                    panel_ui(app, ui, &m.id, &panel.id, &panel.name);
                }
                if !m.exporters.is_empty() || !m.importers.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        for ex in &m.exporters {
                            if Button::secondary(&format!("Export {}", ex.name)).icon(Icon::Export).show(ui).clicked() {
                                export(app, &ctx, &m.id, &ex.id, ex.extensions.first().map_or("dat", String::as_str));
                            }
                        }
                        for im in &m.importers {
                            ui.label(egui::RichText::new(format!("Reads .{}", im.extensions.join(", ."))).font(theme::caption()).color(t.text_2));
                        }
                    });
                }
            }
        });
    }
}

fn export(app: &mut StudioApp, ctx: &egui::Context, plugin: &str, exporter: &str, ext: &str) {
    let path = plugins::export_path(&app.doc.project.name, exporter, ext);
    let r = host().export(plugin, exporter, &plugins::project_json(&app.doc.project)).and_then(|bytes| {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, bytes).map_err(|e| e.to_string())
    });
    match r {
        Ok(()) => kit::push(ctx, format!("Exported {}", studio_paths::tilde(&path)), None, Some(Icon::Export)),
        Err(e) => kit::push(ctx, e, None, Some(Icon::Warning)),
    };
}

/// One plugin panel: a button to open it, then its widgets.
fn panel_ui(app: &mut StudioApp, ui: &mut egui::Ui, plugin: &str, panel: &str, name: &str) {
    let t = theme::tokens(ui.ctx());
    let id = panel_id(plugin, panel);
    let mut state: Option<PanelState> = ui.ctx().data(|d| d.get_temp(id));
    let mut press: Option<Option<String>> = None;
    match &mut state {
        None => {
            if Button::secondary(&format!("Open {name}")).icon(Icon::SquaresFour).show(ui).clicked() {
                press = Some(None);
            }
        }
        Some(st) => {
            ui.add_space(4.0);
            for w in st.view["widgets"].as_array().cloned().unwrap_or_default() {
                let wid = w["id"].as_str().unwrap_or("").to_owned();
                match w["type"].as_str() {
                    Some("heading") => {
                        ui.label(egui::RichText::new(w["text"].as_str().unwrap_or("")).font(theme::control_medium()).color(t.text));
                    }
                    Some("label") => {
                        ui.label(egui::RichText::new(w["text"].as_str().unwrap_or("")).font(theme::body()).color(t.text_2));
                    }
                    Some("slider") => {
                        let (lo, hi) = (w["min"].as_f64().unwrap_or(0.0) as f32, w["max"].as_f64().unwrap_or(1.0) as f32);
                        let v = st.values.entry(wid.clone()).or_insert(w["value"].as_f64().unwrap_or(lo as f64) as f32);
                        let sr = kit::Slider::new(w["label"].as_str().unwrap_or(""), v, lo..=hi.max(lo)).default(lo).decimals(2).with_label().with_value().show(ui);
                        if sr.drag_stopped() || sr.clicked() {
                            press = Some(None);
                        }
                    }
                    Some("toggle") => {
                        let v = st.values.entry(wid.clone()).or_insert(if w["value"].as_bool().unwrap_or(false) { 1.0 } else { 0.0 });
                        let mut b = *v != 0.0;
                        if ui.checkbox(&mut b, w["label"].as_str().unwrap_or("")).changed() {
                            *v = b as u8 as f32;
                            press = Some(None);
                        }
                    }
                    Some("button") => {
                        if Button::secondary(w["label"].as_str().unwrap_or("")).show(ui).clicked() {
                            press = Some(Some(wid));
                        }
                    }
                    Some("separator") => {
                        ui.separator();
                    }
                    _ => {}
                }
            }
            if let Some(e) = &st.error {
                ui.label(egui::RichText::new(e).font(theme::caption()).color(t.text_2));
            }
            if Button::ghost("Close").show(ui).clicked() {
                ui.ctx().data_mut(|d| d.remove::<PanelState>(id));
                return;
            }
        }
    }
    let Some(button) = press else {
        if let Some(st) = state {
            ui.ctx().data_mut(|d| d.insert_temp(id, st));
        }
        return;
    };
    let mut st = state.unwrap_or_default();
    let event = json!({"button": button, "values": st.values, "selection": app.editor.selection});
    match host().panel(plugin, panel, &event, &plugins::project_json(&app.doc.project)) {
        Ok(view) => {
            if let Some(ops) = view.get("ops").filter(|o| o.as_array().is_some_and(|a| !a.is_empty())) {
                if let Err(e) = crate::agent::run_on(&mut app.doc, "edit", &json!({"ops": ops, "label": name})) {
                    st.error = Some(e);
                }
            }
            if let Some(m) = view["message"].as_str() {
                kit::push(ui.ctx(), m, None, Some(Icon::Sparkle));
            }
            st.view = view;
            st.error = None;
        }
        Err(e) => st.error = Some(e),
    }
    ui.ctx().data_mut(|d| d.insert_temp(id, st));
}

fn available(app: &mut StudioApp, ui: &mut egui::Ui, q: &str) {
    let t = theme::tokens(ui.ctx());
    let ctx = ui.ctx().clone();
    let installed = host().list();
    let all = catalogue::available(host());
    heading(ui, "Plugins");
    let mut shown = 0;
    for e in catalogue::search(&all, q) {
        let have = installed.iter().find(|p| p.id() == e.id);
        if have.is_some_and(|p| !catalogue::newer(&e.version, &p.manifest.version)) {
            continue;
        }
        shown += 1;
        card(ui, |ui| {
            kit::name_row(ui, egui::RichText::new(format!("{}  {}", e.name, e.version)).font(theme::control_medium()).color(t.text), |ui| {
                let label = if have.is_some() { "Update" } else { "Install" };
                if Button::secondary(label).icon(Icon::DownloadSimple).show(ui).clicked() {
                    app.editor.library_selected = None;
                    match catalogue::install(host(), &e.id) {
                        Ok(p) => kit::push(&ctx, format!("Installed {} ({})", p.manifest.name, p.manifest.summary()), None, Some(Icon::CheckCircle)),
                        Err(err) => kit::push(&ctx, err, None, Some(Icon::Warning)),
                    };
                }
            });
            ui.label(egui::RichText::new(format!("{} {}.", e.description, e.provides)).font(theme::body()).color(t.text_2));
            let from = if e.bundled() { "Ships with Zornov. Runs sandboxed: no files, no network.".to_owned() } else { format!("From {}. Runs sandboxed: no files, no network.", e.url) };
            ui.label(egui::RichText::new(from).font(theme::caption()).color(t.text_2));
        });
    }
    if shown == 0 {
        ui.label(egui::RichText::new("Everything listed is installed. Ask the AI to find or write a plugin for something else.").font(theme::caption()).color(t.text_2));
    }
    // OpenFX: other hosts' plugins, as native code.
    heading(ui, "OpenFX");
    let ofx = plugins::ofx_json();
    let on = ofx["enabled"].as_bool().unwrap_or(false);
    card(ui, |ui| {
        kit::name_row(ui, egui::RichText::new("OpenFX plugins").font(theme::control_medium()).color(t.text), |ui| {
            if Button::secondary(if on { "Stop loading" } else { "Load" }).show(ui).clicked() {
                match plugins::set_ofx(!on) {
                    Ok(v) => kit::push(&ctx, format!("{} OpenFX plugins loaded", v["plugins"].as_array().map_or(0, Vec::len)), None, None),
                    Err(e) => kit::push(&ctx, e, None, Some(Icon::Warning)),
                };
            }
        });
        let n = ofx["bundles_found"].as_u64().unwrap_or(0);
        let loaded = ofx["plugins"].as_array().map_or(0, Vec::len);
        let text = if on {
            format!("{n} bundles found, {loaded} effects loaded (Effects ▸ Plugins). Each bundle runs in its own process.")
        } else {
            format!("{n} bundles found on this Mac (/Library/OFX/Plugins). They are native code from their makers, not sandboxed; load them only from makers you trust.")
        };
        ui.label(egui::RichText::new(text).font(theme::body()).color(t.text_2));
        for f in ofx["failed"].as_array().into_iter().flatten() {
            ui.label(egui::RichText::new(format!("{}: {}", f["bundle"].as_str().unwrap_or(""), f["error"].as_str().unwrap_or(""))).font(theme::caption()).color(t.text_2));
        }
    });
}
