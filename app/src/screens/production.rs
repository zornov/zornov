// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Production workspace: the production office of the open project.
//! Breakdown, stripboard and day-out-of-days, budget and task board, over
//! the same `production.json` the AI agent reads and writes, so what the
//! AI does shows here and what is changed here shows to the AI (the file
//! is read again when it changes on disk).

use crate::app::{Overlay, StudioApp};
use crate::kit;
use crate::theme;
use eframe::egui::{self, Color32, Rect, RichText, Vec2};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use studio_production::story::Story;
use studio_production::breakdown::{Category, facts};
use studio_production::budget::{Section, money};
use studio_production::report;
use studio_production::tasks::{Entity, Status, today};
use studio_production::{FILE, Production};
use crate::kit::ShowNoted;

const TABS: [&str; 5] = ["Overview", "Breakdown", "Schedule", "Budget", "Tasks"];

/// What the workspace remembers between frames.
#[derive(Default)]
pub struct ProductionUi {
    tab: usize,
    prod: Production,
    loaded: Option<(PathBuf, std::time::SystemTime)>,
    checked_at: f64,
    scene: String,
    flash: String,
    // Breakdown: the tag being typed.
    tag_cat: usize,
    tag_name: String,
    // Schedule.
    start_date: String,
    // Budget.
    template: usize,
    scale: f32,
    actual_line: u64,
    actual_amount: String,
    currency: String,
    // The story as last read, and the project revision it was read at.
    story: Story,
    story_rev: Option<u64>,
    // Tasks.
    status_filter: Option<Status>,
    note: String,
    new_target: String,
    new_step: String,
    who: String,
}

type Shared = Arc<Mutex<ProductionUi>>;

fn modified(p: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

impl ProductionUi {
    /// Reads the file again when someone else (the AI) changed it.
    fn refresh(&mut self, dir: Option<&Path>, now: f64) {
        let Some(dir) = dir else { return };
        let path = dir.join(FILE);
        let changed = match (&self.loaded, modified(&path)) {
            (Some((p, t)), Some(m)) => *p != path || *t != m,
            (None, _) => true,
            (Some((p, _)), None) => *p != path,
        };
        if changed && now - self.checked_at > 0.3 || self.loaded.is_none() {
            self.checked_at = now;
            match Production::load(dir) {
                Ok(p) => {
                    self.prod = p;
                    self.loaded = Some((path.clone(), modified(&path).unwrap_or(std::time::UNIX_EPOCH)));
                }
                Err(e) => self.flash = e,
            }
            if self.scale == 0.0 {
                self.scale = 1.0;
                self.start_date = self.prod.settings.start_date.clone().unwrap_or_default();
                self.currency = self.prod.budget.currency.clone();
            }
        }
    }

    fn save(&mut self, dir: &Path) {
        match self.prod.save(dir) {
            Ok(()) => self.loaded = Some((dir.join(FILE), modified(&dir.join(FILE)).unwrap_or(std::time::UNIX_EPOCH))),
            Err(e) => self.flash = e,
        }
    }
}

/// The dialog.
pub fn show(app: &mut StudioApp, ctx: &egui::Context) {
    let shared: Shared = ctx.data_mut(|d| d.get_temp_mut_or_insert_with(egui::Id::new("production-ui"), Shared::default).clone());
    let dir = app.doc.folder.as_ref().map(|f| f.root().to_path_buf());
    let now = ctx.input(|i| i.time);
    let mut st = shared.lock().unwrap();
    // The story is read again only when the project changed.
    let rev = app.doc.history.revision();
    if st.story_rev != Some(rev) {
        st.story = crate::agent::story_of(&app.doc.model);
        st.story_rev = Some(rev);
    }
    let story = std::mem::take(&mut st.story);
    st.refresh(dir.as_deref(), now);
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
    let out = kit::modal(ctx, egui::Id::new("production-dialog"), Vec2::new(1080.0, 700.0), "Production", |ui| body(ui, &mut st, &story, dir.as_deref()));
    st.story = story;
    if out.close {
        app.overlay = Overlay::None;
    }
}

fn body(ui: &mut egui::Ui, st: &mut ProductionUi, story: &Story, dir: Option<&Path>) {
    let t = theme::tokens(ui.ctx());
    egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 8)).show(ui, |ui| {
        kit::scroll_strip(ui, "production-tabs", |ui| {
            for (i, name) in TABS.iter().enumerate() {
                if ui.selectable_label(st.tab == i, RichText::new(*name).font(theme::body())).clicked() {
                    st.tab = i;
                }
            }
        });
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !st.flash.is_empty() && ui.small_button("✕").clicked() {
                    st.flash.clear();
                }
                kit::label_trunc(ui, RichText::new(&st.flash).font(theme::caption()).color(t.text_2));
            });
        });
        ui.separator();
        let Some(dir) = dir else {
            ui.add_space(24.0);
            ui.label("Save the project first: the production office is kept in its folder.");
            return;
        };
        if story.scenes.is_empty() {
            ui.add_space(24.0);
            ui.label("There is no script yet. Import a screenplay with the Story tools (or ask the AI), then come back to break it down, schedule it and budget it.");
            if st.tab != 3 && st.tab != 4 {
                return;
            }
        }
        let before = st.prod.clone();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_v(ui, |ui| match st.tab {
            0 => overview(ui, st, story, dir),
            1 => breakdown(ui, st, story),
            2 => schedule(ui, st, story, dir),
            3 => budget(ui, st, story, dir),
            _ => tasks(ui, st, story),
        });
        if st.prod != before {
            st.save(dir);
        }
    });
}

fn card(ui: &mut egui::Ui, title: &str, add: impl Fn(&mut egui::Ui)) {
    let t = theme::tokens(ui.ctx());
    let frame = egui::Frame::new().fill(t.raised).corner_radius(6).inner_margin(egui::Margin::same(10));
    // In a row of cards: one that would only fit squeezed (its words broken
    // up) starts the next row instead.
    if ui.layout().main_wrap && ui.cursor().left() > ui.max_rect().left() + 1.0 {
        let mut probe = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(ui.cursor().min, Vec2::splat(f32::INFINITY))).layout(*ui.layout()).id_salt(("card-probe", title)).sizing_pass().invisible());
        frame.show(&mut probe, |ui| {
            ui.label(RichText::new(title).strong());
            add(ui);
        });
        // (In a wrapping row the available width is the whole row's.)
        if probe.min_rect().width() > ui.max_rect().right() - ui.cursor().left() {
            ui.end_row();
        }
    }
    frame.show(ui, |ui| {
        ui.label(RichText::new(title).strong());
        add(ui);
    });
}

fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

fn write_pdf(st: &mut ProductionUi, dir: &Path, name: &str, bytes: Vec<u8>) {
    let d = dir.join("production");
    let path = d.join(name);
    st.flash = match std::fs::create_dir_all(&d).and_then(|_| std::fs::write(&path, bytes)) {
        Ok(()) => crate::i18n::tf(format!("Wrote {}", path.display())),
        Err(e) => crate::i18n::tf(format!("Could not write {}: {e}", path.display())),
    };
}

// -------------------------------------------------------------- overview

fn overview(ui: &mut egui::Ui, st: &mut ProductionUi, story: &Story, dir: &Path) {
    let p = &st.prod;
    let d = p.dashboard(story, today());
    let r = p.budget_report();
    let cur = &p.budget.currency;
    ui.horizontal_wrapped(|ui| {
        card(ui, "Progress", |ui| {
            ui.add(egui::ProgressBar::new((d.percent / 100.0) as f32).text(format!("{:.0}%", d.percent)).desired_width(200.0));
            ui.label(format!("{} of {} shots finished, {} tasks", d.shots_done.0, d.shots_done.1, d.total));
        });
        card(ui, "Schedule", |ui| {
            ui.label(format!("{} shooting days", p.schedule.days.len()));
            if let Some(w) = p.schedule.days.last().and_then(|d| d.date.clone()) {
                ui.label(format!("wraps {w}"));
            }
            ui.label(format!("{} scenes, {} elements", story.scenes.len(), p.elements.len()));
        });
        card(ui, "Budget", |ui| {
            ui.label(crate::i18n::tf(format!("Budget {}", money(r.total, cur))));
            ui.label(crate::i18n::tf(format!("Spent {}   at completion {}", money(r.actual, cur), money(r.eac, cur))));
            ui.label(crate::i18n::tf(format!("AI generation so far {}", money(r.ai_spend, cur))));
        });
    });
    let warns = p.schedule_warnings(story);
    if !warns.is_empty() {
        card(ui, "Needs attention", |ui| {
            for w in warns.iter().take(8) {
                ui.label(format!("• {w}"));
            }
        });
    }
    if !d.blocked.is_empty() || !d.overdue.is_empty() || !d.in_review.is_empty() {
        card(ui, "On the board", |ui| {
            ui.label(format!("{} blocked, {} overdue, {} waiting for review", d.blocked.len(), d.overdue.len(), d.in_review.len()));
        });
    }
    card(ui, "Recent activity", |ui| {
        for a in p.activity.iter().rev().take(10) {
            ui.label(format!("{}: {}", a.who, a.text));
        }
        if p.activity.is_empty() {
            ui.label("Nothing yet.");
        }
    });
    ui.horizontal_wrapped(|ui| {
        if ui.button("Progress report (PDF)").clicked() {
            let bytes = report::dashboard_pdf(&d, &st.prod, story);
            write_pdf(st, dir, "progress.pdf", bytes);
        }
    });
}

// ------------------------------------------------------------- breakdown

fn breakdown(ui: &mut egui::Ui, st: &mut ProductionUi, story: &Story, ) {
    let all = facts(story);
    if st.scene.is_empty() || story.scene(&st.scene).is_none() {
        st.scene = story.scenes.first().map(|s| s.id.clone()).unwrap_or_default();
    }
    ui.horizontal_wrapped(|ui| {
        if ui.button(if st.prod.tags.is_empty() { "Break down the script" } else { "Read the script again" }).clicked() {
            let r = st.prod.auto_tag(story);
            st.flash = format!("{} tags in {} scenes; {} hand-made kept", r.tags, r.scenes, r.kept_manual);
        }
        ui.label(RichText::new(crate::i18n::shown("Reads each scene for what it needs; fix what it got wrong with the tags.")).font(theme::caption()));
    });
    ui.add_space(6.0);
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(250.0);
            for f in &all {
                let label = format!("{}  {} {}  {}", f.id, f.int_ext(), f.dn(), f.set);
                // Long headings end in an ellipsis; the hover shows them whole.
                if ui.add(egui::Button::selectable(st.scene == f.id, label.as_str()).truncate()).on_hover_text(&label).clicked() {
                    st.scene = f.id.clone();
                }
            }
        });
        ui.separator();
        ui.vertical(|ui| {
            let Some(sheet) = st.prod.sheet(story, &st.scene) else { return };
            let f = &sheet.facts;
            ui.label(RichText::new(crate::i18n::tf(format!("Scene {}   {}", f.id, if f.heading.is_empty() { &f.set } else { &f.heading }))).strong());
            ui.label(format!("{} {}   {} pages   {:.0} s{}", f.int_ext(), f.dn(), f.pages(), f.secs, st.prod.day_of(&f.id).map(|d| format!("   shoot day {}", d + 1)).unwrap_or_default()));
            if !f.summary.is_empty() {
                ui.label(&f.summary);
            }
            ui.add_space(6.0);
            let scene = f.id.clone();
            let mut remove: Option<String> = None;
            for (cat, _) in &sheet.groups {
                egui::Frame::new().fill(rgb(cat.colour()).gamma_multiply(0.22)).corner_radius(4).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(cat.title()).strong());
                    ui.horizontal_wrapped(|ui| {
                        for (e, tag) in st.prod.in_scene(&scene).into_iter().filter(|(e, _)| e.category == *cat) {
                            let name = match (e.category, e.number) {
                                (Category::Cast, Some(n)) => format!("{n}. {}", e.name),
                                _ if tag.qty > 1 => format!("{} x{}", e.name, tag.qty),
                                _ => e.name.clone(),
                            };
                            if ui.small_button(format!("{name}  ✕")).on_hover_text(if tag.auto { "found in the script; click to remove" } else { "added by hand; click to remove" }).clicked() {
                                remove = Some(e.id.clone());
                            }
                        }
                    });
                });
            }
            if let Some(id) = remove {
                st.prod.untag(&scene, &id);
            }
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                egui::ComboBox::from_id_salt("tag-cat").selected_text(Category::ALL[st.tag_cat].title()).show_ui(ui, |ui| {
                    crate::kit::note_list(ui);
                    for (i, c) in Category::ALL.iter().enumerate() {
                        ui.selectable_value(&mut st.tag_cat, i, c.title());
                    }
                });
                ui.add(egui::TextEdit::singleline(&mut st.tag_name).hint_text("add something this scene needs").desired_width(220.0));
                if ui.button("Add").clicked() && !st.tag_name.trim().is_empty() {
                    let (cat, name) = (Category::ALL[st.tag_cat], st.tag_name.trim().to_owned());
                    match st.prod.tag(&scene, cat, &name, 1, "") {
                        Ok(()) => st.tag_name.clear(),
                        Err(e) => st.flash = e,
                    }
                }
            });
        });
    });
}

// -------------------------------------------------------------- schedule

fn schedule(ui: &mut egui::Ui, st: &mut ProductionUi, story: &Story, dir: &Path) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Start date");
        ui.add(egui::TextEdit::singleline(&mut st.start_date).hint_text("YYYY-MM-DD").desired_width(100.0));
        if ui.button("Schedule automatically").clicked() {
            if st.start_date.trim().is_empty() {
                st.prod.settings.start_date = None;
            } else if studio_production::date::Date::parse(&st.start_date).is_some() {
                st.prod.settings.start_date = Some(st.start_date.trim().to_owned());
            } else {
                st.flash = "The start date is YYYY-MM-DD".into();
                return;
            }
            if st.prod.tags.is_empty() {
                st.prod.auto_tag(story);
            }
            let r = st.prod.auto_schedule(story);
            st.flash = format!("{} scenes over {} days{}", r.scenes, r.days, r.wrap.map(|w| format!(", wrapping {w}")).unwrap_or_default());
        }
        if ui.button("Stripboard + day out of days (PDF)").clicked() {
            let b = report::schedule_pdf(&st.prod, story);
            write_pdf(st, dir, "schedule.pdf", b);
        }
        if ui.button("All call sheets (PDF)").clicked() {
            let n = st.prod.schedule.days.len();
            for i in 0..n {
                if let Ok(c) = st.prod.call_sheet(story, i) {
                    write_pdf(st, dir, &format!("call-sheet-day-{}.pdf", i + 1), report::call_sheet_pdf(&c));
                }
            }
            st.flash = crate::i18n::tf(format!("Wrote {n} call sheets to {}", dir.join("production").display()));
        }
    });
    for w in st.prod.schedule_warnings(story) {
        ui.label(RichText::new(format!("⚠ {w}")).font(theme::caption()).color(theme::tokens(ui.ctx()).warn));
    }
    ui.add_space(6.0);
    let strips = st.prod.strips(story);
    let ndays = st.prod.schedule.days.len();
    let mut shown: Option<usize> = Some(usize::MAX);
    let mut moves: Vec<(String, usize, Option<usize>)> = Vec::new();
    for (i, s) in strips.iter().enumerate() {
        if Some(s.day.unwrap_or(usize::MAX - 1)) != shown {
            shown = Some(s.day.unwrap_or(usize::MAX - 1));
            ui.add_space(6.0);
            match s.day {
                Some(d) => {
                    let date = st.prod.schedule.days[d].date.clone().unwrap_or_default();
                    let eighths: u32 = strips.iter().filter(|x| x.day == Some(d)).map(|x| x.facts.eighths()).sum();
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(crate::i18n::tf(format!("Day {}   {}   {} pages", d + 1, date, studio_production::breakdown::pages_text(eighths)))).strong());
                        if ui.small_button("Call sheet PDF").clicked() {
                            if let Ok(c) = st.prod.call_sheet(story, d) {
                                write_pdf(st, dir, &format!("call-sheet-day-{}.pdf", d + 1), report::call_sheet_pdf(&c));
                            }
                        }
                    });
                }
                None => {
                    ui.label(RichText::new(crate::i18n::shown("Not scheduled")).strong());
                }
            }
        }
        let fill = match (s.facts.interior, s.facts.day_night) {
            (false, studio_production::breakdown::DayNight::Day) => Color32::from_rgb(120, 110, 40),
            (true, studio_production::breakdown::DayNight::Day) => Color32::from_rgb(90, 90, 96),
            (false, studio_production::breakdown::DayNight::Night) => Color32::from_rgb(50, 100, 60),
            (true, studio_production::breakdown::DayNight::Night) => Color32::from_rgb(50, 70, 120),
        };
        egui::Frame::new().fill(fill.gamma_multiply(if theme::tokens(ui.ctx()).dark { 0.8 } else { 0.35 })).corner_radius(3).inner_margin(egui::Margin::symmetric(8, 3)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                // The buttons keep their place at the end; the strip's text
                // ends in an ellipsis before them (whole on hover).
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let d = s.day.unwrap_or(ndays);
                    if ui.small_button("▼ later").clicked() {
                        moves.push((s.facts.id.clone(), d + 1, Some(0)));
                    }
                    if ui.small_button("▲ earlier").clicked() && d > 0 {
                        moves.push((s.facts.id.clone(), d - 1, None));
                    }
                    if s.day.is_none() && ui.small_button("Add to a new day").clicked() {
                        moves.push((s.facts.id.clone(), ndays, None));
                    }
                    let strip = format!("{:>4}  {} {}  {:<22} {:<5} cast {}", s.facts.id, s.facts.int_ext(), s.facts.dn(), s.facts.set.chars().take(22).collect::<String>(), s.facts.pages(), s.cast.iter().map(u32::to_string).collect::<Vec<_>>().join(","));
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| kit::label_trunc(ui, RichText::new(strip).monospace()));
                });
            });
        });
        let _ = i;
    }
    for (scene, day, idx) in moves {
        let day = day.min(st.prod.schedule.days.len());
        if let Err(e) = st.prod.move_scene(story, &scene, day, idx) {
            st.flash = e;
        }
    }
    let dood = st.prod.dood();
    if !dood.rows.is_empty() {
        ui.add_space(12.0);
        ui.label(RichText::new(crate::i18n::shown("Day out of days")).strong());
        kit::scroll_wide(ui, "dood-scroll", |ui| egui::Grid::new("dood").striped(true).show(ui, |ui| {
            ui.label("");
            ui.label("Cast");
            for (n, d) in &dood.days {
                ui.label(RichText::new(format!("{n}")).monospace()).on_hover_text(d.clone().unwrap_or_default());
            }
            ui.label("Work");
            ui.label("Hold");
            ui.end_row();
            for r in &dood.rows {
                ui.label(r.number.to_string());
                ui.label(&r.name);
                for c in &r.cells {
                    ui.label(RichText::new(c.code()).monospace());
                }
                ui.label(r.work.to_string());
                ui.label(r.hold.to_string());
                ui.end_row();
            }
        }));
    }
}

// ---------------------------------------------------------------- budget

fn budget(ui: &mut egui::Ui, st: &mut ProductionUi, story: &Story, dir: &Path) {
    ui.horizontal_wrapped(|ui| {
        egui::ComboBox::from_id_salt("budget-template").selected_text(studio_production::budget::TEMPLATES[st.template].0.replace('_', " ")).show_ui(ui, |ui| {
            crate::kit::note_list(ui);
            for (i, (n, d)) in studio_production::budget::TEMPLATES.iter().enumerate() {
                ui.selectable_value(&mut st.template, i, n.replace('_', " ")).on_hover_text(*d);
            }
        });
        ui.label("rates ×");
        kit::Slider::new("Rates", &mut st.scale, 0.05..=20.0).default(1.0).unit("×").decimals(2).logarithmic(true).with_value().width(160.0).show(ui);
        if ui.button("Budget from the breakdown").clicked() {
            if st.prod.tags.is_empty() && !story.scenes.is_empty() {
                st.prod.auto_tag(story);
            }
            let name = studio_production::budget::TEMPLATES[st.template].0;
            st.flash = match st.prod.apply_budget_template(story, name, st.scale as f64) {
                Ok(n) => format!("{n} lines. The rates are placeholders: replace them with real quotes."),
                Err(e) => e,
            };
        }
        ui.label("Currency");
        ui.add(egui::TextEdit::singleline(&mut st.currency).desired_width(48.0));
        if ui.button("Set").clicked() {
            if let Err(e) = st.prod.budget.set_currency(&st.currency) {
                st.flash = e;
            }
            st.currency = st.prod.budget.currency.clone();
        }
        if ui.button("Budget (PDF)").clicked() {
            let b = report::budget_pdf(&st.prod, story);
            write_pdf(st, dir, "budget.pdf", b);
        }
    });
    let r = st.prod.budget_report();
    let cur = r.currency.clone();
    if st.prod.budget.accounts.is_empty() {
        ui.add_space(12.0);
        ui.label("No budget yet: pick a template and build it from the breakdown.");
        return;
    }
    ui.add_space(6.0);
    // Tables keep their columns: wider than the dialog, they scroll sideways.
    kit::scroll_wide(ui, "top-sheet-scroll", |ui| egui::Grid::new("top-sheet").striped(true).min_col_width(70.0).show(ui, |ui| {
        for h in ["Acct", "Account", "Budget", "Actual", "At completion", "Variance"] {
            ui.label(RichText::new(h).strong());
        }
        ui.end_row();
        for s in Section::ALL {
            let rows: Vec<_> = r.accounts.iter().filter(|a| a.section == s).collect();
            if rows.is_empty() {
                continue;
            }
            ui.label("");
            ui.label(RichText::new(s.title()).strong());
            ui.end_row();
            for a in rows {
                ui.label(&a.code);
                ui.label(&a.name);
                ui.label(money(a.budget, &cur));
                ui.label(money(a.actual, &cur));
                ui.label(money(a.eac, &cur));
                let v = a.variance();
                ui.label(RichText::new(money(v, &cur)).color(if v < 0.0 { theme::tokens(ui.ctx()).danger } else { theme::tokens(ui.ctx()).text }));
                ui.end_row();
            }
        }
        ui.label("");
        ui.label(RichText::new(crate::i18n::tf(format!("Contingency {:.0}%", st.prod.budget.contingency_pct))).strong());
        ui.label(money(r.contingency, &cur));
        ui.end_row();
        ui.label("");
        ui.label(RichText::new(crate::i18n::shown("TOTAL")).strong());
        ui.label(RichText::new(money(r.total, &cur)).strong());
        ui.label(RichText::new(money(r.actual, &cur)).strong());
        ui.label(RichText::new(money(r.eac, &cur)).strong());
        ui.label(RichText::new(money(r.variance, &cur)).strong());
        ui.end_row();
    }));
    ui.label(RichText::new(crate::i18n::tf(format!("AI generation so far {} over {} runs{}", money(r.ai_spend, &cur), st.prod.spend.len(), if r.ai_unbudgeted > 0.0 { " (not in any budget line: add one with feed ai_spend)" } else { "" }))).font(theme::caption()));
    ui.add_space(8.0);
    // Actuals: pick a line, type what was spent.
    let lines: Vec<(u64, String, f64, f64)> = st.prod.budget.accounts.iter().flat_map(|a| a.lines.iter().map(move |l| (a, l))).map(|(a, l)| (l.id, format!("{} {}", a.code, l.desc), st.prod.budget.line_budget(l), st.prod.budget.line_actual(l))).collect();
    ui.horizontal_wrapped(|ui| {
        ui.label("Record money spent on");
        let shown = lines.iter().find(|l| l.0 == st.actual_line).map(|l| l.1.clone()).unwrap_or_else(|| "choose a line".into());
        egui::ComboBox::from_id_salt("actual-line").selected_text(shown).width(320.0).show_ui(ui, |ui| {
            crate::kit::note_list(ui);
            for l in &lines {
                ui.selectable_value(&mut st.actual_line, l.0, format!("{}   {} / {}", l.1, money(l.3, &cur), money(l.2, &cur)));
            }
        });
        ui.add(egui::TextEdit::singleline(&mut st.actual_amount).hint_text(format!("amount in {cur}")).desired_width(110.0));
        if ui.button("Add").clicked() {
            match st.actual_amount.trim().replace(',', "").parse::<f64>() {
                Ok(a) if st.actual_line != 0 => match st.prod.add_actual(st.actual_line, a, None, "entered in the Production workspace") {
                    Ok(()) => st.actual_amount.clear(),
                    Err(e) => st.flash = e,
                },
                _ => st.flash = "Choose a line and type a number".into(),
            }
        }
    });
    // Costs per scene.
    ui.add_space(10.0);
    ui.label(RichText::new(crate::i18n::shown("Cost per scene")).strong());
    kit::scroll_wide(ui, "scene-costs-scroll", |ui| egui::Grid::new("scene-costs").striped(true).show(ui, |ui| {
        for h in ["Scene", "Budget", "Spent", "Per second"] {
            ui.label(RichText::new(h).strong());
        }
        ui.end_row();
        for s in st.prod.scene_costs(story) {
            ui.label(&s.scene);
            ui.label(money(s.budget, &cur));
            ui.label(money(s.actual, &cur));
            ui.label(money(s.budget / s.secs.max(1e-9), &cur));
            ui.end_row();
        }
    }));
}

// ----------------------------------------------------------------- tasks

fn tasks(ui: &mut egui::Ui, st: &mut ProductionUi, story: &Story) {
    let d = st.prod.dashboard(story, today());
    ui.horizontal_wrapped(|ui| {
        ui.add(egui::ProgressBar::new((d.percent / 100.0) as f32).text(format!("{:.0}% done", d.percent)).desired_width(220.0));
        ui.label(format!("{} of {} shots finished   {} blocked   {} overdue   {} in review", d.shots_done.0, d.shots_done.1, d.blocked.len(), d.overdue.len(), d.in_review.len()));
    });
    ui.horizontal_wrapped(|ui| {
        for s in &d.steps {
            ui.label(RichText::new(format!("{} {}/{}", s.step, s.done, s.total)).font(theme::caption()));
        }
    });
    ui.horizontal_wrapped(|ui| {
        if ui.button("Task per shot and pipeline stage").clicked() {
            let n = st.prod.ensure_pipeline_tasks(story);
            st.flash = format!("{n} tasks made");
        }
        if ui.button("Design task per asset").clicked() {
            let n = st.prod.ensure_asset_tasks("design");
            st.flash = format!("{n} tasks made");
        }
        ui.label("Show");
        egui::ComboBox::from_id_salt("task-filter").selected_text(st.status_filter.map(Status::name).unwrap_or("all")).show_ui(ui, |ui| {
            crate::kit::note_list(ui);
            ui.selectable_value(&mut st.status_filter, None, "all");
            for s in Status::ALL {
                ui.selectable_value(&mut st.status_filter, Some(s), s.name());
            }
        });
        ui.label("I am");
        ui.add(egui::TextEdit::singleline(&mut st.who).hint_text("name").desired_width(90.0));
    });
    ui.horizontal_wrapped(|ui| {
        egui::ComboBox::from_id_salt("new-task-target").selected_text(if st.new_target.is_empty() { "shot…".to_owned() } else { st.new_target.clone() }).show_ui(ui, |ui| {
            crate::kit::note_list(ui);
            for s in &story.scenes {
                for h in &s.shots {
                    ui.selectable_value(&mut st.new_target, h.id.clone(), &h.id);
                }
            }
        });
        ui.add(egui::TextEdit::singleline(&mut st.new_step).hint_text("step (animation)").desired_width(120.0));
        if ui.button("Add task").clicked() {
            if st.new_target.is_empty() || st.new_step.trim().is_empty() {
                st.flash = "Pick a shot and name the step".into();
            } else {
                let (t, s) = (st.new_target.clone(), st.new_step.trim().to_owned());
                st.prod.add_task(Entity::Shot, &t, &s, "", Status::Ready);
            }
        }
    });
    ui.separator();
    let who = if st.who.trim().is_empty() { "me".to_owned() } else { st.who.trim().to_owned() };
    let ids: Vec<u64> = st.prod.tasks.iter().filter(|t| st.status_filter.is_none_or(|s| t.status == s)).map(|t| t.id).collect();
    let mut set: Option<(u64, Status)> = None;
    let mut version: Option<u64> = None;
    let mut note: Option<u64> = None;
    kit::scroll_wide(ui, "tasks-scroll", |ui| egui::Grid::new("tasks").striped(true).show(ui, |ui| {
        for h in ["Task", "Status", "Assignee", "Due", "Ver", "Latest note", ""] {
            ui.label(RichText::new(h).strong());
        }
        ui.end_row();
        for id in ids.iter().take(400) {
            let Some(i) = st.prod.tasks.iter().position(|t| t.id == *id) else { continue };
            // Long names end in an ellipsis (whole on hover), so the row's
            // controls stay in view.
            let label = st.prod.tasks[i].label();
            kit::within(ui, 300.0, |ui| kit::label_trunc(ui, label));
            let mut s = st.prod.tasks[i].status;
            egui::ComboBox::from_id_salt(("task-status", id)).selected_text(s.name()).width(100.0).show_ui(ui, |ui| {
                crate::kit::note_list(ui);
                for x in Status::ALL {
                    ui.selectable_value(&mut s, x, x.name());
                }
            });
            if s != st.prod.tasks[i].status {
                set = Some((*id, s));
            }
            ui.add(egui::TextEdit::singleline(&mut st.prod.tasks[i].assignee).desired_width(80.0));
            let mut due = st.prod.tasks[i].due.clone().unwrap_or_default();
            if ui.add(egui::TextEdit::singleline(&mut due).hint_text("date").desired_width(80.0)).changed() {
                st.prod.tasks[i].due = if due.trim().is_empty() { None } else { Some(due) };
            }
            ui.label(st.prod.tasks[i].versions.last().map(|v| format!("v{}", v.n)).unwrap_or_default());
            kit::within(ui, 220.0, |ui| kit::label_trunc(ui, st.prod.tasks[i].notes.last().map(|n| format!("{}: {}", n.who, n.text)).unwrap_or_default()));
            ui.horizontal(|ui| {
                if ui.small_button("+ version").on_hover_text(crate::i18n::shown("Stamp a version linked to the shot's clips on the timeline")).clicked() {
                    version = Some(*id);
                }
                if ui.small_button("+ note").clicked() {
                    note = Some(*id);
                }
            });
            ui.end_row();
        }
    }));
    ui.horizontal_wrapped(|ui| {
        ui.label("Note");
        ui.add(egui::TextEdit::singleline(&mut st.note).hint_text("then press + note on a task").desired_width(420.0));
    });
    if let Some((id, s)) = set {
        if let Err(e) = st.prod.set_status(&id.to_string(), s, &who, "") {
            st.flash = e;
        }
    }
    if let Some(id) = version {
        match st.prod.add_version(story, &id.to_string(), &who, &st.note.clone(), None) {
            Ok(n) => {
                st.flash = crate::i18n::tf(format!("Version {n} stamped"));
                st.note.clear();
            }
            Err(e) => st.flash = e,
        }
    }
    if let Some(id) = note {
        if st.note.trim().is_empty() {
            st.flash = "Type the note first".into();
        } else {
            let text = st.note.trim().to_owned();
            if let Err(e) = st.prod.add_note(&id.to_string(), &who, &text) {
                st.flash = e;
            }
            st.note.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_production::story::SECOND;
    use studio_production::story::{Character, Line, Place, Scene, Shot};

    fn story() -> Story {
        let mut st = Story { title: "The Lantern".into(), ..Default::default() };
        st.cast.push(Character { id: "fern".into(), name: "Fern".into(), ..Default::default() });
        st.places.push(Place { id: "meadow".into(), name: "Meadow".into(), ..Default::default() });
        st.scenes.push(Scene {
            id: "1".into(),
            heading: "EXT. MEADOW - DAY".into(),
            place: Some("meadow".into()),
            cast: vec!["fern".into()],
            lines: vec![Line { text: "Fern carries a lantern.".into(), ..Default::default() }],
            shots: vec![Shot { id: "1A".into(), duration: 4 * SECOND, ..Default::default() }],
            ..Default::default()
        });
        st
    }

    /// Draws every tab headless (no window), with data in the office, and
    /// clicks nothing: the point is that no tab panics on real content or
    /// on an empty office.
    #[test]
    fn every_tab_draws_with_and_without_data() {
        let dir = std::env::temp_dir().join(format!("studio-production-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let story = story();
        for filled in [false, true] {
            let mut st = ProductionUi { scale: 1.0, ..Default::default() };
            if filled {
                st.prod.settings.start_date = Some("2026-11-02".into());
                st.prod.auto_tag(&story);
                st.prod.auto_schedule(&story);
                st.prod.apply_budget_template(&story, "live_action_short", 1.0).unwrap();
                st.prod.ensure_pipeline_tasks(&story);
                st.prod.ensure_asset_tasks("design");
                st.prod.record_spend(studio_production::Spend { provider: "openai".into(), model: "image-model".into(), cents: 8, ..Default::default() });
            }
            let ctx = egui::Context::default();
            crate::theme::apply_style(&ctx);
            for tab in 0..TABS.len() {
                st.tab = tab;
                for _ in 0..2 {
                    let mut out = ctx.run_ui(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1200.0, 800.0))), ..Default::default() }, |ui| body(ui, &mut st, &story, Some(&dir)));
                    out.textures_delta.clear();
                }
            }
            let no_dir = egui::Context::default();
            let mut out = no_dir.run_ui(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1200.0, 800.0))), ..Default::default() }, |ui| body(ui, &mut st, &story, None));
            out.textures_delta.clear();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_workspace_follows_the_file_the_ai_writes() {
        let dir = std::env::temp_dir().join(format!("studio-production-ui-sync-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut ui = ProductionUi::default();
        ui.refresh(Some(&dir), 1.0);
        assert!(ui.prod.tasks.is_empty());
        // The AI (another writer) adds a task.
        let mut p = Production::default();
        p.add_task(Entity::Shot, "1A", "layout", "claude", Status::InProgress);
        std::thread::sleep(std::time::Duration::from_millis(20));
        p.save(&dir).unwrap();
        ui.refresh(Some(&dir), 2.0);
        assert_eq!(ui.prod.tasks.len(), 1, "read again when the file changed");
        // A change made here is written back for the AI.
        ui.prod.add_task(Entity::Shot, "1B", "layout", "", Status::Ready);
        ui.save(&dir);
        assert_eq!(Production::load(&dir).unwrap().tasks.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
