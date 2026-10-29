// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Edit mode's bar over the 3D workspace (object, edit and sculpt mode;
//! vertex, edge and face select modes; the Select, Add, Mesh, Vertex,
//! Edge, Face and UV menus, every item with its shortcut and a tip; the
//! select tool, proportional editing, snapping, symmetry and what is
//! drawn), the menus that open at the pointer (right click, X, M,
//! Shift+G…), the operation search (F3) and the panel that adjusts the
//! last operation.

use super::input::{self, launch};
use super::{SelectTool, SnapTo, Stage, with};
use crate::app::StudioApp;
use crate::editor::viewport as vp;
use crate::icons::Icon;
use crate::kit::{self, IconButton, MenuItem};
use crate::theme;
use eframe::egui::{self, Rect, Vec2};
use serde_json::{Map, Value, json};
use std::cell::RefCell;
use studio_modeling::selection::{Mode, Similar};
use studio_modeling::tools::{self, Kind, Menu, TOOLS, Tool};

const BAR_H: f32 = 28.0;

thread_local! {
    /// A shape added in object mode, while its settings can still change.
    static OBJECT_LAST: RefCell<Option<(u32, &'static str, Map<String, Value>)>> = const { RefCell::new(None) };
    static SEARCH: RefCell<String> = const { RefCell::new(String::new()) };
}

fn search_id() -> egui::Id {
    egui::Id::unique("model-edit-search")
}

fn at_id() -> egui::Id {
    egui::Id::unique("model-edit-menu-at")
}

pub fn open_search(ctx: &egui::Context) {
    SEARCH.with(|s| s.borrow_mut().clear());
    kit::set_open(ctx, search_id(), true);
}

/// "edge_loops" → "Edge loops".
pub fn words(id: &str) -> String {
    let s = id.replace('_', " ");
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => s,
    }
}

/// A menu item for an operation: its label, shortcut and tip.
fn item(ui: &mut egui::Ui, t: &Tool) -> bool {
    let keys = shown_keys(t.keys);
    let mut it = MenuItem::new(t.label);
    if !keys.is_empty() {
        it = it.shortcut(&keys);
    }
    let r = it.show(ui).on_hover_text(crate::i18n::t(t.tip));
    r.clicked()
}

/// A shortcut as this platform writes it.
pub fn shown_keys(k: &str) -> String {
    if cfg!(target_os = "macos") {
        k.replace("Cmd+", "⌘").replace("Shift+", "⇧").replace("Alt+", "⌥").replace("Ctrl+", "⌃")
    } else {
        k.replace("Cmd+", "Ctrl+")
    }
}

/// Every operation of a menu (Mesh holds Normals and Clean up too).
fn list(ui: &mut egui::Ui, menu: Menu, chosen: &mut Option<&'static str>) {
    let mut group = |ui: &mut egui::Ui, m: Menu, heading: bool| {
        if heading {
            kit::menu_separator(ui);
            kit::menu_heading(ui, m.label());
        }
        for t in TOOLS.iter().filter(|t| t.menu == m) {
            if item(ui, t) {
                *chosen = Some(t.id);
            }
        }
    };
    group(ui, menu, false);
    if menu == Menu::Mesh {
        group(ui, Menu::Normals, true);
        group(ui, Menu::Clean, true);
    }
}

/// Adds the shape `id` as a new 3D object at the 3D cursor, the one path the
/// viewer's Add menu and this workspace's Add share.
pub fn add_shape(app: &mut StudioApp, ctx: &egui::Context, id: &'static str) {
    let args = Map::new();
    match super::add_object(app, ctx, id, &args) {
        Ok(clip) => OBJECT_LAST.with(|o| *o.borrow_mut() = Some((clip, id, args))),
        Err(e) => {
            kit::push(ctx, e, None, Some(Icon::Warning));
        }
    }
}

/// The shapes of the Add menu: (id, label).
pub fn shapes() -> Vec<(&'static str, &'static str)> {
    TOOLS.iter().filter(|t| t.menu == Menu::Add).map(|t| (t.id, t.label)).collect()
}

/// The bar along the top of the workspace.
pub fn bar(app: &mut StudioApp, ui: &mut egui::Ui, body: Rect) {
    let ctx = ui.ctx().clone();
    let stage = super::stage();
    let candidate = super::candidate(app).is_some() || super::open_clip().is_some();
    // Right of the pane's name, and of the outliner when it is open.
    let left = if crate::editor::viewport::ui::outliner_open(&ctx) { 250.0 } else { 170.0 };
    let r = Rect::from_min_size(body.left_top() + Vec2::new(left, 4.0), Vec2::new((body.width() - left - 130.0).max(200.0), BAR_H));
    let th = theme::tokens(&ctx);
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::left_to_right(egui::Align::Center)));
    bui.spacing_mut().item_spacing.x = 3.0;
    // Mode.
    if candidate {
        let labels: Vec<&str> = Stage::ALL.iter().map(|s| s.label()).collect();
        let i = Stage::ALL.iter().position(|s| *s == stage).unwrap_or(0);
        let sw = crate::editor::viewer::menu_w(&bui, &labels);
        if let Some(k) = kit::menu_button(&mut bui, "model-stage", i, &labels, sw, None)
            && let Err(e) = super::enter(app, &ctx, Stage::ALL[k])
        {
            kit::push(&ctx, e, None, Some(Icon::Info));
        }
    }
    let mut chosen: Option<&'static str> = None;
    match stage {
        Stage::Object => {
            // Shapes as new objects.
            let id = egui::Id::unique("model-object-add");
            let resp = kit::Button::ghost("Add").compact().show(&mut bui).on_hover_text(crate::i18n::t("Add a shape as a new 3D object at the 3D cursor"));
            if resp.clicked() {
                kit::toggle_open(&ctx, id);
            }
            let mut add: Option<&'static str> = None;
            kit::popover(&ctx, id, resp.rect, kit::Side::Below, 220.0, |ui| {
                kit::menu_heading(ui, "Mesh");
                crate::editor::viewer::tall_menu(ui, |ui| {
                    for t in TOOLS.iter().filter(|t| t.menu == Menu::Add) {
                        if item(ui, t) {
                            add = Some(t.id);
                        }
                    }
                });
            });
            if let Some(id) = add {
                kit::set_open(&ctx, egui::Id::unique("model-object-add"), false);
                add_shape(app, &ctx, id);
            }
            if candidate && kit::Button::ghost("Edit").compact().shortcut("Tab").show(&mut bui).on_hover_text(crate::i18n::t("Edit the selected model's vertices, edges and faces")).clicked()
                && let Err(e) = super::enter(app, &ctx, Stage::Edit)
            {
                kit::push(&ctx, e, None, Some(Icon::Info));
            }
        }
        Stage::Edit => {
            let mode = with(|s| s.as_ref().map(|s| s.sel.mode)).unwrap_or(Mode::Vertex);
            for (m, icon, key) in [(Mode::Vertex, Icon::DiamondFill, "1"), (Mode::Edge, Icon::Path, "2"), (Mode::Face, Icon::Square, "3")] {
                if IconButton::new(icon, m.label()).shortcut(key).active(mode == m).compact().show(&mut bui).clicked() {
                    with(|s| {
                        if let Some(s) = s {
                            let p = s.poly().clone();
                            s.sel.set_mode(&p, m);
                        }
                    });
                }
            }
            bui.add_space(6.0);
            for menu in [Menu::Select, Menu::Add, Menu::Mesh, Menu::Vertex, Menu::Edge, Menu::Face, Menu::Uv] {
                let id = egui::Id::unique(("model-edit-menu", menu.label()));
                let resp = kit::Button::ghost(menu.label()).compact().show(&mut bui);
                if resp.clicked() {
                    kit::toggle_open(&ctx, id);
                }
                kit::popover(&ctx, id, resp.rect, kit::Side::Below, 280.0, |ui| crate::editor::viewer::tall_menu(ui, |ui| list(ui, menu, &mut chosen)));
                if chosen.is_some() {
                    kit::set_open(&ctx, id, false);
                }
            }
            // The second row: how picks and moves behave, what shows.
            let r2 = Rect::from_min_size(r.left_bottom() + Vec2::new(0.0, 2.0), Vec2::new(r.width(), BAR_H));
            let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(r2).layout(egui::Layout::left_to_right(egui::Align::Center)));
            bui.spacing_mut().item_spacing.x = 3.0;
            let (tool, prop, snap_to, mirror, show) = with(|s| s.as_ref().map(|s| (s.tool, s.prop, s.snap_to, s.mirror, s.show))).unwrap_or_default();
            for (t, icon) in [(SelectTool::Box, Icon::Rectangle), (SelectTool::Lasso, Icon::Selection), (SelectTool::Paint, Icon::Circle)] {
                if IconButton::new(icon, t.label()).shortcut("W").active(tool == t).compact().show(&mut bui).clicked() {
                    with(|s| {
                        if let Some(s) = s {
                            s.tool = t;
                        }
                    });
                }
            }
            bui.add_space(6.0);
            // Proportional editing.
            let pid = egui::Id::unique("model-edit-proportional");
            let presp = IconButton::new(Icon::Gauge, "Proportional editing").shortcut("O").active(prop.on).compact().show(&mut bui);
            if presp.clicked() {
                kit::toggle_open(&ctx, pid);
            }
            kit::popover(&ctx, pid, presp.rect, kit::Side::Below, 240.0, |ui| {
                let mut p = prop;
                kit::labeled_row(ui, "Proportional", |ui| {
                    kit::toggle(ui, &mut p.on, "Proportional editing", None);
                });
                kit::labeled_row(ui, "Reach", |ui| {
                    kit::NumberField::new("Reach", &mut p.radius).range(0.0..=1.0e6).decimals(1).unit("px").width(90.0).show(ui);
                });
                let names: Vec<String> = tools::FALLOFFS.iter().map(|(n, _)| words(n)).collect();
                let refs: Vec<&str> = names.iter().map(String::as_str).collect();
                let fi = tools::FALLOFFS.iter().position(|(_, f)| *f == p.falloff).unwrap_or(0);
                kit::labeled_row(ui, "Falloff", |ui| {
                    if let Some(i) = kit::menu_button(ui, "model-falloff", fi, &refs, crate::editor::viewer::menu_w(ui, &refs), None) {
                        p.falloff = tools::FALLOFFS[i].1;
                    }
                });
                kit::labeled_row(ui, "Connected only", |ui| {
                    kit::toggle(ui, &mut p.connected, "Measure along the surface", None);
                });
                if p != prop {
                    with(|s| {
                        if let Some(s) = s {
                            if p.on && p.radius <= 0.0 {
                                let (lo, hi) = s.poly().bounds();
                                p.radius = studio_mesh::length(studio_mesh::sub(hi, lo)).max(1.0) * 0.25;
                            }
                            s.prop = p;
                        }
                    });
                }
            });
            // Snapping target.
            let names: Vec<&str> = SnapTo::ALL.iter().map(|s| s.label()).collect();
            let si = SnapTo::ALL.iter().position(|s| *s == snap_to).unwrap_or(0);
            let nw = crate::editor::viewer::menu_w(&bui, &names);
            if let Some(i) = kit::menu_button(&mut bui, "model-snap-to", si, &names, nw, None) {
                with(|s| {
                    if let Some(s) = s {
                        s.snap_to = SnapTo::ALL[i];
                    }
                });
                vp::with(|w| {
                    w.snap.on = true;
                    w.changed();
                });
            }
            // Symmetry.
            for (i, name) in ["X", "Y", "Z"].iter().enumerate() {
                let on = mirror[i];
                let b = if on { kit::Button::secondary(name) } else { kit::Button::ghost(name) };
                if b.compact().show(&mut bui).on_hover_text(crate::i18n::tf(format!("Symmetric editing across {name}: the mirrored side follows"))).clicked() {
                    with(|s| {
                        if let Some(s) = s {
                            s.mirror[i] = !on;
                        }
                    });
                }
            }
            // What is drawn.
            let oid = egui::Id::unique("model-edit-overlays");
            let oresp = kit::Button::ghost("Overlays").compact().show(&mut bui);
            if oresp.clicked() {
                kit::toggle_open(&ctx, oid);
            }
            kit::popover(&ctx, oid, oresp.rect, kit::Side::Below, 230.0, |ui| {
                let mut sh = show;
                for (label, v) in [("Face normals", &mut sh.face_normals), ("Vertex normals", &mut sh.vertex_normals), ("Face orientation", &mut sh.face_orientation), ("Seams", &mut sh.seams), ("Sharp edges", &mut sh.sharp), ("Creases", &mut sh.creases), ("Statistics", &mut sh.stats)] {
                    if MenuItem::new(label).checked(*v).show(ui).clicked() {
                        *v = !*v;
                    }
                }
                if sh != show {
                    with(|s| {
                        if let Some(s) = s {
                            s.show = sh;
                        }
                    });
                }
                kit::menu_separator(ui);
                kit::menu_heading(ui, "View");
                if MenuItem::new("Frame selected").shortcut(".").show(ui).clicked() {
                    input::frame_selected(app, ui.ctx());
                }
                if MenuItem::new("Local view").shortcut("/").checked(with(|s| s.as_ref().is_some_and(|s| s.local.is_some()))).show(ui).clicked() {
                    input::toggle_local_view(app);
                }
                if MenuItem::new("X-ray").shortcut(&shown_keys("Alt+Z")).checked(vp::snapshot().shade == vp::Shade::Xray).show(ui).clicked() {
                    vp::with(|w| {
                        w.shade = if w.shade == vp::Shade::Xray { vp::Shade::Solid } else { vp::Shade::Xray };
                        w.changed();
                    });
                }
                if MenuItem::new("Search operations…").shortcut("F3").show(ui).clicked() {
                    open_search(ui.ctx());
                }
            });
        }
        Stage::Sculpt => {
            super::sculpt3d::bar(app, &mut bui);
        }
    }
    let _ = th;
    if let Some(id) = chosen {
        launch(app, &ctx, id);
    }
}

/// Menus at the pointer, the search, and object mode's shape panel.
pub fn popups(app: &mut StudioApp, ui: &mut egui::Ui, body: Rect) {
    let ctx = ui.ctx().clone();
    if let Some((at, which)) = input::take_menu() {
        ctx.data_mut(|d| d.insert_temp(at_id(), (at, which.to_string())));
        kit::set_open(&ctx, egui::Id::unique("model-edit-at"), true);
    }
    let pending: Option<(egui::Pos2, String)> = ctx.data(|d| d.get_temp(at_id()));
    if let Some((at, which)) = pending {
        let id = egui::Id::unique("model-edit-at");
        let mut chosen: Option<(&'static str, Map<String, Value>)> = None;
        let mode = with(|s| s.as_ref().map(|s| s.sel.mode)).unwrap_or(Mode::Vertex);
        let shown = kit::popover(&ctx, id, Rect::from_min_size(at, Vec2::splat(1.0)), kit::Side::Below, 260.0, |ui| at_menu(ui, &which, mode, &mut chosen));
        if shown.is_none() || !kit::is_open(&ctx, id) {
            ctx.data_mut(|d| d.remove::<(egui::Pos2, String)>(at_id()));
        }
        if let Some((tid, args)) = chosen {
            kit::set_open(&ctx, id, false);
            ctx.data_mut(|d| d.remove::<(egui::Pos2, String)>(at_id()));
            if args.is_empty() {
                launch(app, &ctx, tid);
            } else {
                let _ = super::run(app, &ctx, tid, args);
            }
        }
    }
    search(app, ui, body);
    object_panel(app, ui, body);
}

fn at_menu(ui: &mut egui::Ui, which: &str, mode: Mode, chosen: &mut Option<(&'static str, Map<String, Value>)>) {
    let with_arg = |key: &str, v: &str| {
        let mut m = Map::new();
        m.insert(key.to_string(), json!(v));
        m
    };
    let choices = |ui: &mut egui::Ui, id: &'static str, key: &str, chosen: &mut Option<(&'static str, Map<String, Value>)>| {
        let Some(t) = tools::find(id) else { return };
        if let Some(Kind::Choice { options, .. }) = t.param(key).map(|p| p.kind) {
            for o in options {
                if MenuItem::new(&words(o)).show(ui).on_hover_text(crate::i18n::t(t.tip)).clicked() {
                    *chosen = Some((id, with_arg(key, o)));
                }
            }
        }
    };
    match which {
        "delete" => {
            kit::menu_heading(ui, "Delete");
            choices(ui, "delete", "what", chosen);
            kit::menu_separator(ui);
            kit::menu_heading(ui, "Dissolve");
            for (label, what) in [("Dissolve vertices", "vertices"), ("Dissolve edges", "edges"), ("Dissolve faces", "faces")] {
                if MenuItem::new(label).show(ui).clicked() {
                    *chosen = Some(("dissolve", with_arg("what", what)));
                }
            }
            if MenuItem::new("Limited dissolve").show(ui).clicked() {
                *chosen = Some(("limited_dissolve", Map::new()));
            }
        }
        "merge" => {
            kit::menu_heading(ui, "Merge");
            choices(ui, "merge", "at", chosen);
        }
        "separate" => {
            kit::menu_heading(ui, "Separate");
            choices(ui, "separate", "by", chosen);
        }
        "similar" => {
            kit::menu_heading(ui, "Select similar");
            let list: &[(&str, Similar)] = match mode {
                Mode::Face => &Similar::FACE,
                Mode::Edge => &Similar::EDGE,
                Mode::Vertex => &Similar::VERTEX,
            };
            for (name, _) in list {
                if MenuItem::new(&words(name)).show(ui).clicked() {
                    let mut m = with_arg("by", name);
                    let threshold = if matches!(*name, "normal" | "direction" | "coplanar") { 5.0 } else { 0.0 };
                    m.insert("threshold".into(), json!(threshold));
                    *chosen = Some(("select_similar", m));
                }
            }
        }
        "normals" => {
            kit::menu_heading(ui, "Normals");
            for t in TOOLS.iter().filter(|t| t.menu == Menu::Normals || matches!(t.id, "shade_smooth" | "shade_flat")) {
                if item(ui, t) {
                    *chosen = Some((t.id, Map::new()));
                }
            }
        }
        "uv" => {
            kit::menu_heading(ui, "UV");
            for (label, method) in [("Unwrap at seams", "seams"), ("Unwrap at sharp angles", "angle"), ("Smart unwrap", "smart")] {
                if MenuItem::new(label).shortcut("U").show(ui).clicked() {
                    *chosen = Some(("unwrap", with_arg("method", method)));
                }
            }
            kit::menu_separator(ui);
            for k in ["box", "planar", "front", "cylinder", "sphere", "view"] {
                if MenuItem::new(&crate::i18n::tf(format!("Project: {}", words(k)))).show(ui).clicked() {
                    *chosen = Some(("project_uvs", with_arg("kind", k)));
                }
            }
            kit::menu_separator(ui);
            for t in TOOLS.iter().filter(|t| t.menu == Menu::Uv && !matches!(t.id, "unwrap" | "project_uvs")) {
                if item(ui, t) {
                    *chosen = Some((t.id, Map::new()));
                }
            }
            for id in ["mark_seam"] {
                if let Some(t) = tools::find(id)
                    && item(ui, t)
                {
                    *chosen = Some((t.id, Map::new()));
                }
            }
        }
        "add" => {
            kit::menu_heading(ui, "Add");
            for t in TOOLS.iter().filter(|t| t.menu == Menu::Add) {
                if item(ui, t) {
                    *chosen = Some((t.id, Map::new()));
                }
            }
        }
        _ => {
            // The right-click menu for this select mode.
            kit::menu_heading(ui, &crate::i18n::tf(format!("{} context menu", mode.label())));
            let mut last_menu: Option<Menu> = None;
            for t in TOOLS.iter().filter(|t| t.context.contains(&mode)) {
                if last_menu.is_some_and(|m| m != t.menu) {
                    kit::menu_separator(ui);
                }
                last_menu = Some(t.menu);
                if item(ui, t) {
                    *chosen = Some((t.id, Map::new()));
                }
            }
            kit::menu_separator(ui);
            for t in TOOLS.iter().filter(|t| matches!(t.id, "select_all" | "select_invert" | "hide" | "reveal")) {
                if item(ui, t) {
                    *chosen = Some((t.id, Map::new()));
                }
            }
        }
    }
}

/// F3: find an operation by name.
fn search(app: &mut StudioApp, ui: &mut egui::Ui, body: Rect) {
    let ctx = ui.ctx().clone();
    let id = search_id();
    if !kit::is_open(&ctx, id) {
        return;
    }
    let anchor = Rect::from_center_size(egui::pos2(body.center().x - 170.0, body.top() + 40.0), Vec2::new(1.0, 1.0));
    let mut chosen: Option<&'static str> = None;
    kit::popover(&ctx, id, anchor, kit::Side::Below, 340.0, |ui| {
        let mut q = SEARCH.with(|s| s.borrow().clone());
        let r = kit::text_field(ui, &mut q, "Search operations", Some(Icon::MagnifyingGlass), 326.0, None);
        r.request_focus();
        SEARCH.with(|s| *s.borrow_mut() = q.clone());
        let hits = tools::search(&q);
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
        if enter && let Some(t) = hits.first() {
            chosen = Some(t.id);
        }
        kit::menu_separator(ui);
        for t in hits.iter().take(14) {
            let keys = shown_keys(t.keys);
            let label = crate::i18n::tf(format!("{} › {}", crate::i18n::t(t.menu.label()), crate::i18n::t(t.label)));
            let mut it = MenuItem::new(&label);
            if !keys.is_empty() {
                it = it.shortcut(&keys);
            }
            if it.show(ui).on_hover_text(crate::i18n::t(t.tip)).clicked() {
                chosen = Some(t.id);
            }
        }
        if hits.is_empty() {
            ui.label(egui::RichText::new(crate::i18n::t("No operation by that name")).font(theme::caption()).color(theme::tokens(ui.ctx()).text_2));
        }
    });
    if !kit::is_open(&ctx, id) || chosen.is_some() {
        input::drop_focus();
    }
    if let Some(tid) = chosen {
        kit::set_open(&ctx, id, false);
        if super::stage() == Stage::Edit {
            launch(app, &ctx, tid);
        } else if tools::find(tid).is_some_and(|t| t.menu == Menu::Add) {
            if let Ok(clip) = super::add_object(app, &ctx, tid, &Map::new()) {
                OBJECT_LAST.with(|o| *o.borrow_mut() = Some((clip, tid, Map::new())));
            }
        } else {
            kit::push(&ctx, "Open a model in edit mode (Tab) to use that", None, Some(Icon::Info));
        }
    }
}

/// One setting's control; returns the new value when it changed.
fn setting(ui: &mut egui::Ui, p: &tools::Param, v: &Value) -> Option<Value> {
    let mut out = None;
    kit::labeled_row(ui, p.label, |ui| match p.kind {
        Kind::Length { min, max, .. } | Kind::Number { min, max, .. } | Kind::Angle { min, max, .. } => {
            let mut x = v.as_f64().unwrap_or(0.0) as f32;
            let unit = match p.kind {
                Kind::Length { .. } => "px",
                Kind::Angle { .. } => "°",
                _ => "",
            };
            let dec = if matches!(p.kind, Kind::Number { .. }) { 3 } else { 1 };
            if kit::NumberField::new(p.label, &mut x).range(min..=max).decimals(dec).unit(unit).width(110.0).show(ui).changed() {
                out = Some(json!(x));
            }
        }
        Kind::Percent { .. } => {
            let mut x = v.as_f64().unwrap_or(0.0) as f32;
            if kit::NumberField::new(p.label, &mut x).range(0.0..=100.0).decimals(0).unit("%").width(110.0).show(ui).changed() {
                out = Some(json!(x));
            }
        }
        Kind::Count { min, max, .. } => {
            let mut x = v.as_f64().unwrap_or(0.0) as f32;
            if kit::NumberField::new(p.label, &mut x).range(min as f32..=max as f32).decimals(0).width(110.0).show(ui).changed() {
                out = Some(json!(x.round() as i64));
            }
        }
        Kind::Toggle { .. } => {
            let mut b = v.as_bool().unwrap_or(false);
            if kit::toggle(ui, &mut b, p.label, None).changed() {
                out = Some(json!(b));
            }
        }
        Kind::Choice { options, .. } => {
            let names: Vec<String> = options.iter().map(|o| words(o)).collect();
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            let i = options.iter().position(|o| Some(*o) == v.as_str()).unwrap_or(0);
            if let Some(k) = kit::menu_button(ui, p.key, i, &refs, 130.0, None) {
                out = Some(json!(options[k]));
            }
        }
        Kind::Axis { .. } => {
            let i = match v.as_str() {
                Some("y") => 1,
                Some("z") => 2,
                _ => 0,
            };
            if let Some(k) = kit::menu_button(ui, p.key, i, &["X", "Y", "Z"], 70.0, None) {
                out = Some(json!(["x", "y", "z"][k]));
            }
        }
        Kind::Vector { .. } => {
            let mut a: [f32; 3] = std::array::from_fn(|j| v.get(j).and_then(Value::as_f64).unwrap_or(0.0) as f32);
            let mut changed = false;
            for (j, n) in ["X", "Y", "Z"].iter().enumerate() {
                changed |= kit::NumberField::new(n, &mut a[j]).decimals(2).width(52.0).show(ui).changed();
            }
            if changed {
                out = Some(json!(a));
            }
        }
    });
    out
}

/// The last operation's settings, to change after the fact.
pub fn last_panel(app: &mut StudioApp, ui: &mut egui::Ui, body: Rect) {
    let ctx = ui.ctx().clone();
    if super::stage() != Stage::Edit {
        return;
    }
    let Some(last) = with(|s| s.as_ref().and_then(|s| s.last.clone())) else { return };
    if with(|s| s.as_ref().is_some_and(|s| s.modal.is_some())) {
        return;
    }
    let Some(t) = tools::find(last.id) else { return };
    if t.params.is_empty() {
        return;
    }
    let mut changed: Option<Map<String, Value>> = None;
    let open_id = egui::Id::unique("model-edit-last-open");
    let open: bool = ctx.data(|d| d.get_temp(open_id)).unwrap_or(true);
    egui::Area::new(egui::Id::unique("model-edit-last"))
        .order(egui::Order::Middle)
        .fixed_pos(body.left_bottom() + Vec2::new(10.0, -40.0))
        .pivot(egui::Align2::LEFT_BOTTOM)
        .show(&ctx, |ui| {
            let th = theme::tokens(ui.ctx());
            egui::Frame::new().fill(th.panel).stroke(egui::Stroke::new(1.0, th.line)).corner_radius(egui::CornerRadius::same(theme::radius::CARD)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                ui.set_width(270.0);
                if kit::Button::ghost(&crate::i18n::tf(format!("{} {}", if open { "▾" } else { "▸" }, crate::i18n::t(t.label)))).compact().show(ui).on_hover_text(crate::i18n::t(t.tip)).clicked() {
                    ctx.data_mut(|d| d.insert_temp(open_id, !open));
                }
                if !open {
                    return;
                }
                for p in t.params {
                    let v = tools::value(t, &last.args, p.key, &last.ctx);
                    if let Some(nv) = setting(ui, p, &v) {
                        let mut args = last.args.clone();
                        args.insert(p.key.to_string(), nv);
                        changed = Some(args);
                    }
                }
                if !last.said.is_empty() {
                    ui.label(egui::RichText::new(&last.said).font(theme::caption()).color(th.text_2));
                }
            });
        });
    if let Some(args) = changed {
        super::rerun(app, &ctx, args);
    }
}

/// Object mode: the shape just added, its settings still open.
fn object_panel(app: &mut StudioApp, ui: &mut egui::Ui, body: Rect) {
    let ctx = ui.ctx().clone();
    if super::stage() != Stage::Object {
        return;
    }
    let Some((clip, id, args)) = OBJECT_LAST.with(|o| o.borrow().clone()) else { return };
    // Only while that shape is still the one selected and nothing else
    // changed since.
    if app.editor.selection != vec![clip] || app.doc.model.active().clip(studio_model::ClipId(clip as u64)).is_none() || app.doc.undo_label().is_none_or(|l| !l.starts_with("Add ")) {
        OBJECT_LAST.with(|o| *o.borrow_mut() = None);
        return;
    }
    let Some(t) = tools::find(id) else { return };
    let w = app.doc.project.width.min(app.doc.project.height) as f32;
    let c = tools::Ctx { size: w * 1.6, ..tools::Ctx::default() };
    let mut changed: Option<Map<String, Value>> = None;
    egui::Area::new(egui::Id::unique("model-object-last")).order(egui::Order::Middle).fixed_pos(body.left_bottom() + Vec2::new(10.0, -40.0)).pivot(egui::Align2::LEFT_BOTTOM).show(&ctx, |ui| {
        let th = theme::tokens(ui.ctx());
        egui::Frame::new().fill(th.panel).stroke(egui::Stroke::new(1.0, th.line)).corner_radius(egui::CornerRadius::same(theme::radius::CARD)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
            ui.set_width(270.0);
            ui.label(egui::RichText::new(crate::i18n::tf(format!("{} {}", crate::i18n::t("Add"), crate::i18n::t(t.label)))).font(theme::body_medium()).color(th.text));
            for p in t.params {
                let v = tools::value(t, &args, p.key, &c);
                if let Some(nv) = setting(ui, p, &v) {
                    let mut a = args.clone();
                    a.insert(p.key.to_string(), nv);
                    changed = Some(a);
                }
            }
        });
    });
    if let Some(a) = changed {
        // The shape again, in place of the one just added.
        app.doc.undo();
        match super::add_object(app, &ctx, id, &a) {
            Ok(clip) => OBJECT_LAST.with(|o| *o.borrow_mut() = Some((clip, id, a))),
            Err(e) => {
                kit::push(&ctx, e, None, Some(Icon::Warning));
            }
        }
    }
}
