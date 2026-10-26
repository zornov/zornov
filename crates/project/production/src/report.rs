// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The production papers as plain text (for the AI and the terminal) and as
//! PDF (for the crew): breakdown sheets, the stripboard with the
//! day-out-of-days, call sheets, the budget, the pipeline schedule and the
//! progress dashboard.

use crate::Production;
use crate::breakdown::{Category, DayNight, SceneSheet, facts};
use crate::budget::{BudgetReport, SceneCost, money, money_ascii};
use crate::date::Date;
use crate::pdf::{Col, Flow, Row};
use crate::pipeline::PipelinePlan;
use crate::schedule::{CallSheet, Dood, Strip};
use crate::tasks::Dashboard;
use crate::story::Story;

fn footer(story: &Story, what: &str) -> String {
    format!("{} - {what}", if story.title.is_empty() { "Untitled" } else { &story.title })
}

fn pct(v: f64) -> String {
    format!("{v:.0}%")
}

fn tint(c: [u8; 3], mix: f32) -> [f32; 3] {
    [c[0], c[1], c[2]].map(|v| 1.0 - (1.0 - v as f32 / 255.0) * mix)
}

// ------------------------------------------------------------- breakdown

fn sheet_text(s: &SceneSheet, p: &Production) -> String {
    let f = &s.facts;
    let mut out = format!("SCENE {}  {}  {} {}  {} pages  {:.0}s\n", f.id, if f.heading.is_empty() { &f.set } else { &f.heading }, f.int_ext(), f.dn(), f.pages(), f.secs);
    if !f.summary.is_empty() {
        out.push_str(&format!("  {}\n", f.summary));
    }
    for (c, names) in &s.groups {
        out.push_str(&format!("  {:<16} {}\n", c.title(), names.join(", ")));
    }
    if s.groups.is_empty() {
        out.push_str("  (nothing tagged: run breakdown)\n");
    }
    let _ = p;
    out
}

/// Breakdown sheets as text: one scene, or all.
pub fn breakdown_text(p: &Production, story: &Story, scene: Option<&str>) -> Result<String, String> {
    let mut out = String::new();
    match scene {
        Some(s) => out.push_str(&sheet_text(&p.sheet(story, s).ok_or_else(|| format!("no scene \"{s}\""))?, p)),
        None => {
            for sc in &story.scenes {
                if let Some(s) = p.sheet(story, &sc.id) {
                    out.push_str(&sheet_text(&s, p));
                    out.push('\n');
                }
            }
        }
    }
    Ok(out)
}

/// Breakdown sheets as a PDF: a page per scene.
pub fn breakdown_pdf(p: &Production, story: &Story, scene: Option<&str>) -> Result<Vec<u8>, String> {
    let ids: Vec<String> = match scene {
        Some(s) => vec![story.scene(s).ok_or_else(|| format!("no scene \"{s}\""))?.id.clone()],
        None => story.scenes.iter().map(|s| s.id.clone()).collect(),
    };
    let mut f = Flow::new("Breakdown", false, &footer(story, "script breakdown"));
    for (i, id) in ids.iter().enumerate() {
        let Some(s) = p.sheet(story, id) else { continue };
        if i > 0 {
            f.new_page();
        }
        let fc = &s.facts;
        f.title(&format!("Breakdown sheet - scene {}", fc.id));
        f.facts(
            &[
                ("Scene", fc.id.clone()),
                ("Int / Ext", fc.int_ext().to_owned()),
                ("Day / Night", fc.dn().to_owned()),
                ("Pages", fc.pages()),
                ("Set", fc.set.clone()),
                ("Running time", format!("{:.0} s", fc.secs)),
                ("Shoot day", p.day_of(&fc.id).map(|d| (d + 1).to_string()).unwrap_or_else(|| "-".into())),
                ("Slug line", fc.heading.clone()),
            ],
            4,
        );
        if !fc.summary.is_empty() {
            f.heading("Description");
            f.para(&fc.summary, 10.0);
        }
        let rows: Vec<Row> = s.groups.iter().map(|(c, names)| Row { cells: vec![c.title().to_owned(), names.join(", ")], fill: Some(tint(c.colour(), 0.18)), bold: false }).collect();
        f.heading("Elements");
        f.table(&[Col::new("Category", 1.4), Col::new("Needs", 5.0)], &rows, 10.0);
    }
    Ok(f.finish())
}

// ------------------------------------------------------------ stripboard

fn strip_fill(s: &Strip) -> [f32; 3] {
    match (s.facts.interior, s.facts.day_night) {
        (false, DayNight::Day) => [1.0, 0.96, 0.6],
        (true, DayNight::Day) => [1.0, 1.0, 1.0],
        (false, DayNight::Night) => [0.72, 0.9, 0.72],
        (true, DayNight::Night) => [0.72, 0.8, 0.95],
    }
}

/// The stripboard as text.
pub fn stripboard_text(p: &Production, story: &Story) -> String {
    let strips = p.strips(story);
    let mut out = String::new();
    let mut day = None;
    for s in &strips {
        if s.day != day {
            day = s.day;
            match day {
                Some(d) => {
                    let sd = &p.schedule.days[d];
                    let pages: u32 = strips.iter().filter(|x| x.day == Some(d)).map(|x| x.facts.eighths()).sum();
                    out.push_str(&format!("\n== DAY {} {} ({} scenes, {} eighths) ==\n", d + 1, sd.date.as_deref().and_then(Date::parse).map(|x| format!("{} {x}", x.weekday_name())).unwrap_or_default(), sd.scenes.len(), pages));
                }
                None => out.push_str("\n== NOT SCHEDULED ==\n"),
            }
        }
        out.push_str(&format!(
            "{:>4} {} {}  {:<22} {:<5} cast {:<10} {}\n",
            s.facts.id,
            s.facts.int_ext(),
            s.facts.dn(),
            s.facts.set.chars().take(22).collect::<String>(),
            s.facts.pages(),
            s.cast.iter().map(u32::to_string).collect::<Vec<_>>().join(","),
            s.facts.summary.chars().take(60).collect::<String>()
        ));
    }
    out
}

/// The day-out-of-days as text.
pub fn dood_text(d: &Dood) -> String {
    let mut out = format!("{:<4}{:<20}", "#", "Cast");
    for (n, _) in &d.days {
        out.push_str(&format!("{n:>4}"));
    }
    out.push_str("   work hold span\n");
    for r in &d.rows {
        out.push_str(&format!("{:<4}{:<20}", r.number, r.name.chars().take(19).collect::<String>()));
        for c in &r.cells {
            out.push_str(&format!("{:>4}", c.code()));
        }
        out.push_str(&format!("   {:>4} {:>4} {:>4}\n", r.work, r.hold, r.span));
    }
    out
}

/// The stripboard and day-out-of-days as a PDF.
pub fn schedule_pdf(p: &Production, story: &Story) -> Vec<u8> {
    let mut f = Flow::new("Schedule", true, &footer(story, "stripboard and day out of days"));
    f.title("Stripboard");
    let strips = p.strips(story);
    let mut day = usize::MAX;
    let mut rows: Vec<Row> = Vec::new();
    let cols = [Col::new("Sc", 0.6), Col::new("I/E D/N", 0.9), Col::new("Set", 2.2), Col::new("Pgs", 0.6), Col::new("Cast", 1.3), Col::new("Description", 5.0)];
    let flush = |f: &mut Flow, rows: &mut Vec<Row>| {
        if !rows.is_empty() {
            f.table(&cols, rows, 8.5);
            rows.clear();
        }
    };
    for s in &strips {
        let d = s.day.unwrap_or(usize::MAX - 1);
        if d != day {
            flush(&mut f, &mut rows);
            day = d;
            match s.day {
                Some(i) => {
                    let sd = &p.schedule.days[i];
                    let eighths: u32 = strips.iter().filter(|x| x.day == Some(i)).map(|x| x.facts.eighths()).sum();
                    f.heading(&format!("Day {}   {}   {} pages", i + 1, sd.date.as_deref().and_then(Date::parse).map(|x| format!("{} {x}", x.weekday_name())).unwrap_or_default(), crate::breakdown::pages_text(eighths)));
                }
                None => f.heading("Not scheduled"),
            }
        }
        rows.push(Row {
            cells: vec![
                s.facts.id.clone(),
                format!("{} {}", s.facts.int_ext(), s.facts.dn()),
                s.facts.set.clone(),
                s.facts.pages(),
                s.cast.iter().map(u32::to_string).collect::<Vec<_>>().join(", "),
                s.facts.summary.clone(),
            ],
            fill: Some(strip_fill(s)),
            bold: false,
        });
    }
    flush(&mut f, &mut rows);
    let dood = p.dood();
    if !dood.rows.is_empty() {
        f.new_page();
        f.title("Day out of days");
        let mut cols = vec![Col::new("#", 0.5), Col::new("Cast", 2.5)];
        for (n, d) in &dood.days {
            let wd = d.as_deref().and_then(Date::parse).map(|x| x.weekday_name()).unwrap_or("");
            cols.push(Col::new(&format!("{n} {wd}").trim_end().to_owned(), 0.8));
        }
        cols.extend([Col::num("Work", 0.7), Col::num("Hold", 0.7)]);
        let rows: Vec<Row> = dood
            .rows
            .iter()
            .map(|r| {
                let mut c = vec![r.number.to_string(), r.name.clone()];
                c.extend(r.cells.iter().map(|x| x.code().to_owned()));
                c.extend([r.work.to_string(), r.hold.to_string()]);
                Row { cells: c, fill: None, bold: false }
            })
            .collect();
        f.table(&cols, &rows, 8.5);
        f.note("SW start work, W work, WF work finish, SWF start-work-finish, H hold.");
    }
    f.finish()
}

// ------------------------------------------------------------ call sheet

/// A call sheet as text.
pub fn call_sheet_text(c: &CallSheet) -> String {
    let mut out = format!("CALL SHEET  {}  Day {} of {}", c.title, c.day, c.of);
    if let Some(d) = c.date.as_deref().and_then(Date::parse) {
        out.push_str(&format!("  {} {d}", d.weekday_name()));
    }
    out.push_str(&format!("\nGeneral call {}   Est. wrap {}   {} pages\n", c.general_call, c.wrap, c.pages));
    for (lbl, v) in [("Director", &c.director), ("Producer", &c.producer), ("Hospital", &c.hospital), ("Office", &c.office)] {
        if !v.is_empty() {
            out.push_str(&format!("{lbl}: {v}\n"));
        }
    }
    out.push_str("\nLocations\n");
    for (n, note) in &c.locations {
        out.push_str(&format!("  {n}{}\n", if note.is_empty() { String::new() } else { format!(" - {note}") }));
    }
    out.push_str("\nScenes\n");
    for s in &c.scenes {
        out.push_str(&format!("  {}  {:<6} sc {:<4} {:<6} cast {:<8} {}\n", s.start, s.kind, s.id, s.pages, s.cast.iter().map(u32::to_string).collect::<Vec<_>>().join(","), s.summary.chars().take(60).collect::<String>()));
    }
    out.push_str("\nCast\n");
    for k in &c.cast {
        out.push_str(&format!("  {}. {:<18} make-up {}  on set {}  scenes {}\n", k.number, k.name, k.makeup, k.on_set, k.scenes.join(",")));
    }
    for (cat, items) in &c.requirements {
        out.push_str(&format!("\n{}: {}\n", cat.title(), items.join("; ")));
    }
    if !c.notes.is_empty() {
        out.push_str(&format!("\nNotes: {}\n", c.notes));
    }
    if let Some((d, sc)) = &c.tomorrow {
        out.push_str(&format!("\nTomorrow: {} scenes {}\n", d.clone().unwrap_or_else(|| "next day".into()), sc.join(", ")));
    }
    out
}

/// A call sheet as a PDF.
pub fn call_sheet_pdf(c: &CallSheet) -> Vec<u8> {
    let mut f = Flow::new(&format!("Call sheet day {}", c.day), false, &format!("{} - call sheet day {}", c.title, c.day));
    f.title(&format!("{} - Call sheet", c.title));
    let date = c.date.as_deref().and_then(Date::parse).map(|d| format!("{} {d}", d.weekday_name())).unwrap_or_else(|| "date to be set".into());
    f.facts(
        &[
            ("Day", format!("{} of {}", c.day, c.of)),
            ("Date", date),
            ("General call", c.general_call.clone()),
            ("Est. wrap", c.wrap.clone()),
            ("Director", c.director.clone()),
            ("Producer", c.producer.clone()),
            ("Pages today", c.pages.clone()),
            ("Office", c.office.clone()),
        ],
        4,
    );
    if !c.hospital.is_empty() {
        f.note(&format!("Nearest hospital: {}", c.hospital));
    }
    f.heading("Locations");
    f.table(&[Col::new("Location", 2.0), Col::new("Notes", 5.0)], &c.locations.iter().map(|(n, t)| Row::new(vec![n.clone(), t.clone()])).collect::<Vec<_>>(), 9.5);
    f.heading("Scenes");
    let rows: Vec<Row> = c
        .scenes
        .iter()
        .map(|s| Row::new(vec![s.start.clone(), s.id.clone(), s.kind.clone(), s.set.clone(), s.pages.clone(), s.cast.iter().map(u32::to_string).collect::<Vec<_>>().join(", "), s.summary.clone()]))
        .collect();
    f.table(&[Col::new("Start", 0.8), Col::new("Sc", 0.6), Col::new("I/E D/N", 0.9), Col::new("Set", 1.8), Col::new("Pgs", 0.6), Col::new("Cast", 1.0), Col::new("Description", 4.0)], &rows, 8.5);
    f.heading("Cast");
    let rows: Vec<Row> = c.cast.iter().map(|k| Row::new(vec![k.number.to_string(), k.name.clone(), k.makeup.clone(), k.on_set.clone(), k.scenes.join(", ")])).collect();
    f.table(&[Col::new("#", 0.5), Col::new("Cast", 2.5), Col::new("Make-up", 1.0), Col::new("On set", 1.0), Col::new("Scenes", 2.0)], &rows, 9.5);
    if !c.requirements.is_empty() {
        f.heading("Departments");
        let rows: Vec<Row> = c.requirements.iter().map(|(cat, items)| Row { cells: vec![cat.title().to_owned(), items.join("; ")], fill: Some(tint(cat.colour(), 0.15)), bold: false }).collect();
        f.table(&[Col::new("Department", 1.5), Col::new("Needs", 6.0)], &rows, 9.0);
    }
    if !c.notes.is_empty() {
        f.heading("Notes");
        f.para(&c.notes, 10.0);
    }
    if let Some((d, sc)) = &c.tomorrow {
        f.heading("Advance schedule");
        f.para(&format!("{}: scenes {}", d.clone().unwrap_or_else(|| "Next day".into()), sc.join(", ")), 10.0);
    }
    f.finish()
}

// ---------------------------------------------------------------- budget

/// The top sheet as text.
pub fn budget_text(r: &BudgetReport, scenes: Option<&[SceneCost]>) -> String {
    let m = |v: f64| money(v, &r.currency);
    let mut out = format!("BUDGET TOP SHEET ({})\n{:<6}{:<34}{:>14}{:>14}{:>14}{:>14}\n", r.currency, "Acct", "Name", "Budget", "Actual", "At completion", "Variance");
    for s in crate::budget::Section::ALL {
        let rows: Vec<_> = r.accounts.iter().filter(|a| a.section == s).collect();
        if rows.is_empty() {
            continue;
        }
        out.push_str(&format!("{}\n", s.title()));
        for a in rows {
            out.push_str(&format!("{:<6}{:<34}{:>14}{:>14}{:>14}{:>14}\n", a.code, a.name.chars().take(33).collect::<String>(), m(a.budget), m(a.actual), m(a.eac), m(a.variance())));
        }
    }
    out.push_str(&format!("{:<40}{:>14}\n{:<40}{:>14}\n{:<40}{:>14}\n", "Subtotal", m(r.subtotal), "Contingency", m(r.contingency), "TOTAL", m(r.total)));
    out.push_str(&format!("Spent {}   Estimate at completion {}   Headroom {}\n", m(r.actual), m(r.eac), m(r.variance)));
    out.push_str(&format!("AI generation spend so far {}{}\n", m(r.ai_spend), if r.ai_unbudgeted > 0.0 { format!(" ({} not in any budget line)", m(r.ai_unbudgeted)) } else { String::new() }));
    if let Some(sc) = scenes {
        out.push_str(&format!("\nCOSTS PER SCENE / SHOT\n{:<10}{:>14}{:>14}{:>14}\n", "", "Budget", "Spent", "per second"));
        for s in sc {
            out.push_str(&format!("scene {:<4}{:>14}{:>14}{:>14}\n", s.scene, m(s.budget), m(s.actual), m(s.budget / s.secs.max(1e-9))));
            for h in &s.shots {
                out.push_str(&format!("  shot {:<5}{:>14}{:>14}{:>14}\n", h.shot, m(h.budget), m(h.actual), m(h.budget / h.secs.max(1e-9))));
            }
        }
    }
    out
}

/// The budget as a PDF: top sheet, accounts in detail, costs per scene.
pub fn budget_pdf(p: &Production, story: &Story) -> Vec<u8> {
    let r = p.budget_report();
    let m = |v: f64| money_ascii(v, &r.currency);
    let mut f = Flow::new("Budget", false, &footer(story, &format!("budget in {}", r.currency)));
    f.title(&format!("{} - Budget{}", if story.title.is_empty() { "Untitled" } else { &story.title }, if p.budget.name.is_empty() { String::new() } else { format!(" ({})", p.budget.name) }));
    let mut rows: Vec<Row> = Vec::new();
    for s in crate::budget::Section::ALL {
        let accts: Vec<_> = r.accounts.iter().filter(|a| a.section == s).collect();
        if accts.is_empty() {
            continue;
        }
        rows.push(Row { cells: vec![String::new(), s.title().to_owned(), String::new(), String::new(), String::new(), String::new()], fill: Some([0.93, 0.93, 0.95]), bold: true });
        for a in accts {
            rows.push(Row::new(vec![a.code.clone(), a.name.clone(), m(a.budget), m(a.actual), m(a.eac), m(a.variance())]));
        }
    }
    rows.push(Row { cells: vec![String::new(), "Subtotal".into(), m(r.subtotal), String::new(), String::new(), String::new()], fill: None, bold: true });
    rows.push(Row { cells: vec![String::new(), format!("Contingency {:.0}%", p.budget.contingency_pct), m(r.contingency), String::new(), String::new(), String::new()], fill: None, bold: false });
    rows.push(Row { cells: vec![String::new(), "TOTAL".into(), m(r.total), m(r.actual), m(r.eac), m(r.variance)], fill: Some([0.88, 0.94, 0.88]), bold: true });
    f.table(&[Col::new("Acct", 0.8), Col::new("Account", 3.5), Col::num("Budget", 1.5), Col::num("Actual", 1.5), Col::num("At completion", 1.6), Col::num("Variance", 1.5)], &rows, 9.0);
    f.note(&format!("AI generation spend so far: {}. Rates: {}", m(r.ai_spend), p.budget.rates.iter().filter(|(c, _)| **c != r.currency).take(6).map(|(c, v)| format!("1 {c} = {v:.4} {}", r.currency)).collect::<Vec<_>>().join(", ")));
    for a in &p.budget.accounts {
        f.heading(&format!("{} {}", a.code, a.name));
        let rows: Vec<Row> = a
            .lines
            .iter()
            .map(|l| {
                let est = p.budget.line_budget(l);
                let act = p.budget.line_actual(l);
                Row::new(vec![l.desc.clone(), format!("{} {}{}", trim(l.qty * l.mult), l.unit, ""), money_ascii(l.rate, l.currency.as_deref().unwrap_or(&r.currency)), m(est), m(act)])
            })
            .collect();
        f.table(&[Col::new("Line", 4.0), Col::new("Qty", 1.4), Col::num("Rate", 1.4), Col::num("Budget", 1.4), Col::num("Actual", 1.4)], &rows, 8.5);
    }
    let sc = p.scene_costs(story);
    if !sc.is_empty() {
        f.heading("Cost per scene and shot");
        let mut rows = Vec::new();
        for s in &sc {
            rows.push(Row { cells: vec![format!("Scene {}", s.scene), m(s.budget), m(s.actual), m(s.budget / s.secs.max(1e-9))], fill: Some([0.95, 0.95, 0.97]), bold: true });
            for h in &s.shots {
                rows.push(Row::new(vec![format!("   shot {}", h.shot), m(h.budget), m(h.actual), m(h.budget / h.secs.max(1e-9))]));
            }
        }
        f.table(&[Col::new("", 3.0), Col::num("Budget", 1.5), Col::num("Spent", 1.5), Col::num("Per second", 1.5)], &rows, 8.5);
    }
    f.finish()
}

fn trim(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 { format!("{v:.0}") } else { format!("{v:.2}") }
}

// ---------------------------------------------------- pipeline / progress

/// The pipeline schedule as text.
pub fn pipeline_text(p: &Production, plan: &PipelinePlan) -> String {
    let mut out = format!("PIPELINE: {} shots, {:.1} working days{}\n", plan.shots.len(), plan.total_days, plan.end_date.as_deref().map(|d| format!(", done {d}")).unwrap_or_default());
    if let Some(b) = &plan.bottleneck {
        out.push_str(&format!("Bottleneck: {b} (shots queue for its artists)\n"));
    }
    out.push_str(&format!("{:<7}{:<7}", "shot", "cplx"));
    for s in &p.pipeline.stages {
        out.push_str(&format!("{:>14}", s.key));
    }
    out.push('\n');
    for s in &plan.shots {
        out.push_str(&format!("{:<7}{:<7.2}", s.shot, s.complexity));
        for sp in &s.spans {
            out.push_str(&format!("{:>14}", format!("{:.1}-{:.1}", sp.start, sp.end)));
        }
        out.push('\n');
    }
    out
}

/// The pipeline schedule as a PDF.
pub fn pipeline_pdf(p: &Production, story: &Story, plan: &PipelinePlan) -> Vec<u8> {
    let mut f = Flow::new("Pipeline", true, &footer(story, "pipeline schedule"));
    f.title("Pipeline schedule");
    f.para(&format!("{} shots, {:.1} working days{}.{}", plan.shots.len(), plan.total_days, plan.end_date.as_deref().map(|d| format!(", finishing {d}")).unwrap_or_default(), plan.bottleneck.as_deref().map(|b| format!(" Bottleneck: {b}.")).unwrap_or_default()), 10.0);
    let mut cols = vec![Col::new("Shot", 0.8), Col::new("Complexity", 0.9)];
    cols.extend(p.pipeline.stages.iter().map(|s| Col::new(&format!("{} ({})", s.key, s.artists), 1.3)));
    let rows: Vec<Row> = plan
        .shots
        .iter()
        .map(|s| {
            let mut c = vec![s.shot.clone(), format!("{:.2}", s.complexity)];
            c.extend(s.spans.iter().map(|sp| format!("d{:.1}-{:.1}{}", sp.start + 1.0, sp.end + 1.0, p.pipeline_date(sp.start).map(|d| format!(" {d}")).unwrap_or_default())));
            Row::new(c)
        })
        .collect();
    f.table(&cols, &rows, 8.0);
    f.finish()
}

/// The dashboard as text.
pub fn dashboard_text(d: &Dashboard, p: &Production) -> String {
    let mut out = format!("PROGRESS {}  ({} tasks; shots finished {}/{})\n", pct(d.percent), d.total, d.shots_done.0, d.shots_done.1);
    out.push_str(&format!("By status: {}\n", d.by_status.iter().map(|(s, n)| format!("{} {n}", s.name())).collect::<Vec<_>>().join(", ")));
    for s in &d.steps {
        out.push_str(&format!("  {:<12} {:>3}/{:<3} done  {}\n", s.step, s.done, s.total, pct(s.percent)));
    }
    if !d.people.is_empty() {
        out.push_str(&format!("People: {}\n", d.people.iter().map(|(n, o, dn)| format!("{n} {o} open/{dn} done")).collect::<Vec<_>>().join(", ")));
    }
    if d.unassigned > 0 {
        out.push_str(&format!("{} open tasks have nobody on them\n", d.unassigned));
    }
    let list = |ids: &[u64]| ids.iter().filter_map(|i| p.tasks.iter().find(|t| t.id == *i)).map(|t| format!("#{} {}", t.id, t.label())).collect::<Vec<_>>().join(", ");
    for (lbl, ids) in [("Overdue", &d.overdue), ("Blocked", &d.blocked), ("Waiting for review", &d.in_review)] {
        if !ids.is_empty() {
            out.push_str(&format!("{lbl}: {}\n", list(ids)));
        }
    }
    out.push_str("Shots:\n");
    for s in &d.shots {
        out.push_str(&format!(
            "  {:<6} {:>4}  {}{}\n",
            s.shot,
            pct(s.percent),
            match (&s.at_step, s.status) {
                (Some(st), Some(status)) => format!("at {st} ({})", status.name()),
                _ => "done".into(),
            },
            if s.version > 0 { format!("  v{}", s.version) } else { String::new() }
        ));
    }
    out
}

/// The dashboard as a PDF.
pub fn dashboard_pdf(d: &Dashboard, p: &Production, story: &Story) -> Vec<u8> {
    let mut f = Flow::new("Progress", false, &footer(story, "progress report"));
    f.title(&format!("Progress - {}", pct(d.percent)));
    f.facts(&[("Tasks", d.total.to_string()), ("Shots finished", format!("{}/{}", d.shots_done.0, d.shots_done.1)), ("Overdue", d.overdue.len().to_string()), ("Blocked", d.blocked.len().to_string())], 4);
    f.heading("Steps");
    f.table(&[Col::new("Step", 2.0), Col::num("Done", 1.0), Col::num("Tasks", 1.0), Col::num("Progress", 1.0)], &d.steps.iter().map(|s| Row::new(vec![s.step.clone(), s.done.to_string(), s.total.to_string(), pct(s.percent)])).collect::<Vec<_>>(), 9.5);
    f.heading("Shots");
    let rows: Vec<Row> = d
        .shots
        .iter()
        .map(|s| Row::new(vec![s.shot.clone(), s.scene.clone(), pct(s.percent), s.at_step.clone().map(|x| format!("{x} ({})", s.status.map(|y| y.name()).unwrap_or(""))).unwrap_or_else(|| "done".into()), s.version.to_string()]))
        .collect();
    f.table(&[Col::new("Shot", 1.0), Col::new("Scene", 1.0), Col::num("Progress", 1.0), Col::new("At", 3.0), Col::num("Ver", 0.6)], &rows, 9.5);
    let blocked: Vec<Row> = d.blocked.iter().chain(&d.overdue).filter_map(|i| p.tasks.iter().find(|t| t.id == *i)).map(|t| Row::new(vec![t.label(), t.status.name().into(), t.assignee.clone(), t.notes.last().map(|n| n.text.clone()).unwrap_or_default()])).collect();
    if !blocked.is_empty() {
        f.heading("Needs attention");
        f.table(&[Col::new("Task", 2.5), Col::new("Status", 1.2), Col::new("Who", 1.2), Col::new("Last note", 4.0)], &blocked, 9.0);
    }
    f.finish()
}

/// Which scenes the film has, for callers that want a one-line summary.
pub fn overview_line(p: &Production, story: &Story) -> String {
    let n = facts(story).len();
    let cast = p.of(Category::Cast).count();
    format!("{n} scenes, {} elements ({cast} cast), {} shooting days, {} tasks, budget {}", p.elements.len(), p.schedule.days.len(), p.tasks.len(), money(p.budget_report().total, &p.budget.currency))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::breakdown::tests::story;
    use crate::pdf::tests::check_structure;
    use crate::tasks::today;

    fn office() -> (Production, Story) {
        let st = story();
        let mut p = Production::default();
        p.settings.start_date = Some("2026-11-02".into());
        p.settings.director = "Ira Sen".into();
        p.settings.hospital = "City Hospital, 12 Main Rd".into();
        p.auto_tag(&st);
        p.auto_schedule(&st);
        p.apply_budget_template(&st, "live_action_short", 1.0).unwrap();
        p.ensure_pipeline_tasks(&st);
        (p, st)
    }

    #[test]
    fn every_paper_is_written_as_text_and_as_a_valid_pdf() {
        let (p, st) = office();
        let text = breakdown_text(&p, &st, None).unwrap();
        assert!(text.contains("SCENE 1") && text.contains("Props") && text.contains("lantern"), "{text}");
        assert!(breakdown_text(&p, &st, Some("9")).is_err());
        assert!(check_structure(&breakdown_pdf(&p, &st, None).unwrap()) >= 2, "a page a scene");
        assert_eq!(check_structure(&breakdown_pdf(&p, &st, Some("2")).unwrap()), 1);

        let board = stripboard_text(&p, &st);
        assert!(board.contains("DAY 1") && board.contains("Mon"), "{board}");
        assert!(dood_text(&p.dood()).contains("SW") || dood_text(&p.dood()).contains("SWF"));
        assert!(check_structure(&schedule_pdf(&p, &st)) >= 2);

        let cs = p.call_sheet(&st, 0).unwrap();
        let t = call_sheet_text(&cs);
        assert!(t.contains("CALL SHEET") && t.contains("Ira Sen") && t.contains("General call 07:00") && t.contains("City Hospital"), "{t}");
        let pdf = call_sheet_pdf(&cs);
        check_structure(&pdf);
        assert!(String::from_utf8_lossy(&pdf).contains("/Title"));

        let r = p.budget_report();
        let bt = budget_text(&r, Some(&p.scene_costs(&st)));
        assert!(bt.contains("TOP SHEET") && bt.contains("Cast") && bt.contains("TOTAL") && bt.contains("scene 1"), "{bt}");
        check_structure(&budget_pdf(&p, &st));

        let plan = p.pipeline_plan(&st).unwrap();
        assert!(pipeline_text(&p, &plan).contains("layout"));
        check_structure(&pipeline_pdf(&p, &st, &plan));

        let d = p.dashboard(&st, today());
        let dt = dashboard_text(&d, &p);
        assert!(dt.contains("PROGRESS 0%") && dt.contains("1A"), "{dt}");
        check_structure(&dashboard_pdf(&d, &p, &st));
        assert!(overview_line(&p, &st).contains("2 scenes"));
    }

    /// With STUDIO_PRODUCTION_PDF_DIR set, leaves the sample papers there to look at.
    #[test]
    fn sample_papers_can_be_written_out() {
        let Ok(dir) = std::env::var("STUDIO_PRODUCTION_PDF_DIR") else { return };
        let (p, st) = office();
        let d = std::path::Path::new(&dir);
        std::fs::write(d.join("breakdown.pdf"), breakdown_pdf(&p, &st, None).unwrap()).unwrap();
        std::fs::write(d.join("schedule.pdf"), schedule_pdf(&p, &st)).unwrap();
        std::fs::write(d.join("callsheet.pdf"), call_sheet_pdf(&p.call_sheet(&st, 0).unwrap())).unwrap();
        std::fs::write(d.join("budget.pdf"), budget_pdf(&p, &st)).unwrap();
        let plan = p.pipeline_plan(&st).unwrap();
        std::fs::write(d.join("pipeline.pdf"), pipeline_pdf(&p, &st, &plan)).unwrap();
        let dash = p.dashboard(&st, today());
        std::fs::write(d.join("progress.pdf"), dashboard_pdf(&dash, &p, &st)).unwrap();
    }

    #[test]
    fn rupees_group_in_lakhs_in_the_budget() {
        let (mut p, st) = office();
        p.budget.set_currency("INR").unwrap();
        let bt = budget_text(&p.budget_report(), None);
        assert!(bt.contains('₹'), "{bt}");
        // The PDF's fonts have no rupee sign: it prints as INR.
        let pdf = budget_pdf(&p, &st);
        check_structure(&pdf);
        assert!(!String::from_utf8_lossy(&pdf).contains('₹'));
    }
}
