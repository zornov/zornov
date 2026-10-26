// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The render queue in the jobs popover: each queued export with its
//! progress, pause / resume and cancel, and how many render nodes are set up.
//! The queue itself lives in a file shared with the AI and `zornov queue`
//! (see `deliver_run`); this only draws it and sends clicks back.

use crate::icons::Icon;
use crate::kit::{self, IconButton};
use crate::theme;
use eframe::egui::{self, Align2, CornerRadius, Rect, Sense, Vec2};
use std::cell::RefCell;
use std::time::{Duration, Instant};
use studio_deliver::queue::{Queue, State};

thread_local! {
    static SNAP: RefCell<Option<(Instant, Queue, usize)>> = const { RefCell::new(None) };
}

/// The queue as the file has it, read at most every 700 ms.
fn snapshot() -> (Queue, usize) {
    SNAP.with(|s| {
        let mut s = s.borrow_mut();
        if s.as_ref().is_none_or(|(t, _, _)| t.elapsed() > Duration::from_millis(700)) {
            *s = Some((Instant::now(), crate::deliver_run::store().read(), crate::deliver_run::load_nodes().len()));
        }
        let (_, q, n) = s.as_ref().expect("just set");
        (q.clone(), *n)
    })
}

/// Add the open project to the render queue with `preset`, saving it first.
pub fn enqueue(doc: &mut crate::doc::Doc, name: &str, preset: &str, qc: bool) -> Result<u32, String> {
    let root = doc.folder.as_ref().map(|f| f.root().to_path_buf()).ok_or("Save the project to a folder first: the queue renders the saved project")?;
    crate::services::project::save(doc, None)?;
    let mut spec = studio_deliver::queue::JobSpec::new(name, &root);
    spec.preset = preset.to_owned();
    spec.qc = qc;
    let store = crate::deliver_run::store();
    let id = store.update(|q| q.add(spec))?;
    SNAP.with(|s| *s.borrow_mut() = None);
    Ok(id)
}

pub fn section(ui: &mut egui::Ui) {
    let t = theme::tokens(ui.ctx());
    let (q, nodes) = snapshot();
    if q.jobs.is_empty() && nodes == 0 {
        return;
    }
    ui.ctx().request_repaint_after(Duration::from_millis(700));
    kit::menu_heading(ui, "Render queue");
    let mut act: Option<(u32, &str)> = None;
    for j in q.jobs.iter().rev().take(8) {
        let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 52.0), Sense::hover());
        let p = ui.painter();
        kit::paint_text_clipped(p, egui::pos2(r.left() + 8.0, r.top() + 14.0), Align2::LEFT_CENTER, &j.spec.name, theme::control_medium(), t.text, r.width() - 90.0);
        let status = match j.state {
            State::Queued => format!("Waiting · priority {}", j.spec.priority),
            State::Running => format!("Rendering · {}%{}", (j.progress * 100.0) as u32, if j.machines.len() > 1 || j.spec.distributed { " · render nodes" } else { "" }),
            State::Paused => format!("Paused · {}%", (j.progress * 100.0) as u32),
            State::Done => format!("Done{}", j.qc.as_ref().map_or(String::new(), |v| format!(" · QC {v}"))),
            State::Failed => format!("Failed · {}", j.error.as_deref().unwrap_or("")),
            State::Cancelled => "Cancelled".to_owned(),
        };
        kit::paint_text_clipped(p, egui::pos2(r.left() + 8.0, r.top() + 32.0), Align2::LEFT_CENTER, &status, theme::caption(), if j.state == State::Failed { t.accent } else { t.text_2 }, r.width() - 90.0);
        let bar = Rect::from_min_size(egui::pos2(r.left() + 8.0, r.bottom() - 6.0), Vec2::new(r.width() - 90.0, 3.0));
        p.rect_filled(bar, CornerRadius::same(2), t.raised);
        p.rect_filled(Rect::from_min_size(bar.min, Vec2::new(bar.width() * j.progress, 3.0)), CornerRadius::same(2), t.accent);
        let mut x = r.right() - 18.0;
        if !j.state.finished() {
            let br = Rect::from_center_size(egui::pos2(x, r.center().y), Vec2::splat(24.0));
            if IconButton::new(Icon::X, "Cancel").size(24.0, 14.0).show_at(ui, br).clicked() {
                act = Some((j.id, "cancel"));
            }
            x -= 26.0;
            let (icon, tip, what) = if j.state == State::Paused { (Icon::Play, "Resume", "resume") } else { (Icon::Pause, "Pause", "pause") };
            let br = Rect::from_center_size(egui::pos2(x, r.center().y), Vec2::splat(24.0));
            if IconButton::new(icon, tip).size(24.0, 14.0).show_at(ui, br).clicked() {
                act = Some((j.id, what));
            }
            x -= 26.0;
            if j.state == State::Queued {
                let br = Rect::from_center_size(egui::pos2(x, r.center().y), Vec2::splat(24.0));
                if IconButton::new(Icon::CaretUp, "Run before the others").size(24.0, 14.0).show_at(ui, br).clicked() {
                    act = Some((j.id, "first"));
                }
            }
        }
    }
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::hover());
    let msg = if nodes == 0 { "No render nodes · run `zornov node` on another Mac and add it with the AI (queue add_node)".to_owned() } else { format!("{nodes} render node(s) set up · jobs marked distributed share their pieces") };
    kit::paint_text_clipped(ui.painter(), egui::pos2(r.left() + 8.0, r.center().y), Align2::LEFT_CENTER, &msg, theme::caption(), t.text_2, r.width() - 12.0);
    if let Some((id, what)) = act {
        let store = crate::deliver_run::store();
        let top = q.jobs.iter().filter(|j| !j.state.finished()).map(|j| j.spec.priority).max().unwrap_or(0);
        let _ = store.update(|q| match what {
            "cancel" => q.cancel(id),
            "pause" => q.pause(id),
            "resume" => q.resume(id),
            _ => q.set_priority(id, top + 1),
        });
        SNAP.with(|s| *s.borrow_mut() = None);
    }
}
