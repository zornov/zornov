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
        let status = status_line(j);
        kit::paint_text_clipped(p, egui::pos2(r.left() + 8.0, r.top() + 32.0), Align2::LEFT_CENTER, &status, theme::caption(), if j.state == State::Failed { t.danger } else { t.text_2 }, r.width() - 90.0);
        let bar = Rect::from_min_size(egui::pos2(r.left() + 8.0, r.bottom() - 6.0), Vec2::new(r.width() - 90.0, 3.0));
        p.rect_filled(bar, CornerRadius::same(2), t.raised);
        p.rect_filled(Rect::from_min_size(bar.min, Vec2::new(bar.width() * j.progress, 3.0)), CornerRadius::same(2), t.accent);
        let mut x = r.right() - 18.0;
        if j.state.finished() {
            if matches!(j.state, State::Failed | State::Cancelled) {
                let br = Rect::from_center_size(egui::pos2(x, r.center().y), Vec2::splat(24.0));
                if IconButton::new(Icon::ArrowCounterClockwise, "Try again").size(24.0, 14.0).show_at(ui, br).clicked() {
                    act = Some((j.id, "retry"));
                }
            }
        } else {
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
    if q.jobs.iter().any(|j| j.state.finished()) {
        let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), Sense::hover());
        if IconButton::new(Icon::Trash, "Clear finished").size(24.0, 14.0).show_at(ui, Rect::from_center_size(egui::pos2(r.right() - 18.0, r.center().y), Vec2::splat(24.0))).clicked() {
            act = Some((0, "clear"));
        }
        kit::paint_text_clipped(ui.painter(), egui::pos2(r.left() + 8.0, r.center().y), Align2::LEFT_CENTER, "Finished jobs stay until cleared", theme::caption(), t.text_2, r.width() - 50.0);
    }
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::hover());
    let msg = if nodes == 0 { "No render nodes · run `zornov node` on another Mac and add it with the AI (queue add_node)".to_owned() } else { format!("{nodes} render node(s) set up · jobs marked distributed share their pieces") };
    kit::paint_text_clipped(ui.painter(), egui::pos2(r.left() + 8.0, r.center().y), Align2::LEFT_CENTER, &msg, theme::caption(), t.text_2, r.width() - 12.0);
    if let Some((id, what)) = act {
        let store = crate::deliver_run::store();
        let top = q.jobs.iter().filter(|j| !j.state.finished()).map(|j| j.spec.priority).max().unwrap_or(0);
        let res = store.update(|q| apply(q, id, what, top)).and_then(|r| r);
        match res {
            Ok(Some(done)) => {
                kit::push(ui.ctx(), done, None, Some(Icon::CheckCircle));
            }
            Ok(None) => {}
            Err(e) => {
                kit::push(ui.ctx(), format!("Could not {} the job: {e}", verb(what)), None, Some(Icon::Warning));
            }
        }
        SNAP.with(|s| *s.borrow_mut() = None);
    }
}

fn verb(what: &str) -> &'static str {
    match what {
        "cancel" => "cancel",
        "pause" => "pause",
        "resume" => "resume",
        "retry" => "restart",
        "clear" => "clear",
        _ => "move",
    }
}

/// One click on a job row, applied to the queue. Returns what to tell the
/// person when the click is worth a toast.
fn apply(q: &mut Queue, id: u32, what: &str, top: i32) -> Result<Option<String>, String> {
    let name = q.get(id).map(|j| j.spec.name.clone()).unwrap_or_default();
    match what {
        "cancel" => q.cancel(id).map(|_| Some(format!("Cancelled {name}"))),
        "pause" => q.pause(id).map(|_| None),
        "resume" => q.resume(id).map(|_| None),
        "retry" => q.retry(id).map(|_| Some(format!("Queued {name} again"))),
        "clear" => {
            let n = q.clear_finished();
            Ok(Some(format!("Cleared {n} finished job{}", if n == 1 { "" } else { "s" })))
        }
        _ => q.set_priority(id, top + 1).map(|_| None),
    }
}

/// The second line of a job row: what it is doing, or how it ended.
fn status_line(j: &studio_deliver::queue::Job) -> String {
    let pct = (j.progress * 100.0) as u32;
    match j.state {
        State::Queued => format!("Waiting · priority {}", j.spec.priority),
        State::Running => format!("Rendering · {pct}%{}", if j.machines.len() > 1 || j.spec.distributed { " · render nodes" } else { "" }),
        State::Paused => format!("Paused · {pct}%"),
        State::Done => format!("Done{}", j.qc.as_ref().map_or(String::new(), |v| format!(" · QC {v}"))),
        State::Failed => match j.error.as_deref().map(str::trim).filter(|e| !e.is_empty()) {
            Some(e) => format!("Failed · {e}"),
            None => "Failed · try again".to_owned(),
        },
        State::Cancelled => "Cancelled · try again to restart".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_deliver::queue::JobSpec;

    fn q_with(n: usize) -> Queue {
        let mut q = Queue::default();
        for i in 0..n {
            q.add(JobSpec::new(&format!("Job {i}"), std::path::Path::new("/nonexistent")));
        }
        q
    }

    #[test]
    fn cancelled_and_failed_jobs_can_be_retried() {
        let mut q = q_with(2);
        assert_eq!(apply(&mut q, 1, "cancel", 0).unwrap().as_deref(), Some("Cancelled Job 0"));
        assert_eq!(q.get(1).unwrap().state, State::Cancelled);
        assert!(status_line(q.get(1).unwrap()).starts_with("Cancelled"));
        assert_eq!(apply(&mut q, 1, "retry", 0).unwrap().as_deref(), Some("Queued Job 0 again"));
        assert_eq!(q.get(1).unwrap().state, State::Queued);
        q.begin(2).unwrap();
        q.fail(2, "No space left on disk");
        assert_eq!(status_line(q.get(2).unwrap()), "Failed · No space left on disk");
    }

    #[test]
    fn a_failure_without_a_reason_still_says_what_to_do() {
        let mut q = q_with(1);
        q.begin(1).unwrap();
        q.fail(1, "  ");
        assert_eq!(status_line(q.get(1).unwrap()), "Failed · try again");
    }

    #[test]
    fn clearing_removes_only_finished_jobs() {
        let mut q = q_with(3);
        apply(&mut q, 1, "cancel", 0).unwrap();
        assert_eq!(apply(&mut q, 1, "clear", 0).unwrap().as_deref(), Some("Cleared 1 finished job"));
        assert_eq!(q.jobs.len(), 2);
    }

    #[test]
    fn a_click_that_cannot_apply_returns_the_reason() {
        let mut q = q_with(1);
        assert!(apply(&mut q, 99, "cancel", 0).is_err());
        apply(&mut q, 1, "cancel", 0).unwrap();
        assert!(apply(&mut q, 1, "pause", 0).is_err());
    }
}
