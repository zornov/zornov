// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Updates and crash reports in the app: what happens at start, the update
//! notice in the corner of the window, Help ▸ Check for updates, and the
//! Settings ▸ About section. The work is in `studio-update` and
//! `studio-crash`.

use crate::app::StudioApp;
use crate::icons::{self, Icon};
use crate::kit::{self, Button, Section};
use crate::theme;
use eframe::egui;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use studio_update::apply::{self, Applied};
use studio_update::state::{self, Status};

/// The product's name in what the updater says.
const NAME: &str = "Zornov";

/// At the start of the window (not for the headless commands): installs an
/// update that is waiting, offers to send crash reports left by the last
/// run, and starts the background check. `STUDIO_NO_UPDATES=1` skips all.
pub fn startup() {
    if off() {
        return;
    }
    match install_waiting() {
        Applied::Installed(_) if apply::relaunch().is_ok() => std::process::exit(0),
        Applied::Handed(_) => std::process::exit(0),
        Applied::Failed(e) => state::fail(e),
        _ => {}
    }
    crash_prompt();
    state::start_background();
}

fn off() -> bool {
    std::env::var_os("STUDIO_NO_UPDATES").is_some()
}

fn install_waiting() -> Applied {
    let cfg = studio_update::config::load();
    apply::apply_pending(&cfg.public_key, studio_update::CURRENT_VERSION, apply::target().as_ref())
}

/// The release page to send someone to when this copy cannot install the
/// update itself (a `.deb`, a development build): the GitHub Releases page
/// when the feed is there.
fn download_page() -> Option<String> {
    let feed = studio_update::config::load().feed_url;
    let (repo, _) = feed.split_once("/releases/")?;
    repo.starts_with("https://github.com/").then(|| format!("{repo}/releases/latest"))
}

fn open_page(url: &str) {
    let mut cmd = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    } else {
        std::process::Command::new("xdg-open")
    };
    let _ = cmd.arg(url).spawn();
}

/// Set by Help ▸ Check for updates: say the result even when there is
/// nothing to offer.
static TELL: AtomicBool = AtomicBool::new(false);

/// Help ▸ Check for updates.
pub fn check_from_menu() {
    TELL.store(true, Ordering::Relaxed);
    state::check_now();
}

/// Saves the open project if it has a folder, installs the waiting update
/// and starts the new version.
fn restart(app: &mut StudioApp, ctx: &egui::Context) {
    if app.doc.folder.is_some() && app.doc.dirty() {
        app.save(ctx, false);
    }
    match install_waiting() {
        Applied::Installed(_) => match apply::relaunch() {
            Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Err(e) => state::fail(format!("the update is installed, but {e}; quit and open {NAME} again")),
        },
        Applied::Handed(_) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        Applied::Failed(e) => state::fail(e),
        Applied::Nothing => state::fail("the downloaded update is gone; check again".into()),
    }
}

/// The update notice: bottom-right of the window, only when there is
/// something to offer, never twice in a day after Later. Drawn every frame
/// over whatever screen is up.
pub fn notice(app: &mut StudioApp, ctx: &egui::Context) {
    if app.cli.shot.is_some() {
        return;
    }
    // A window left open for days still checks about once a day.
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    if !off() {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_none_or(|t| t.elapsed() > Duration::from_secs(3600)) {
            if last.is_some() {
                state::start_background();
            }
            *last = Some(Instant::now());
        }
    }
    let snap = state::snapshot();
    if matches!(snap.status, Status::Checking | Status::Downloading(..)) {
        ctx.request_repaint_after(Duration::from_millis(250));
    }
    if TELL.load(Ordering::Relaxed) && !matches!(snap.status, Status::Checking | Status::Idle) {
        TELL.store(false, Ordering::Relaxed);
        match &snap.status {
            Status::UpToDate => {
                kit::push(ctx, crate::i18n::tf(format!("{NAME} {} is up to date", snap.current)), None, Some(Icon::CheckCircle));
            }
            Status::Disabled(why) => {
                kit::push(ctx, crate::i18n::tf(format!("Updates are off: {why}")), None, Some(Icon::Info));
            }
            _ => {}
        }
    }
    if !snap.notice() || (off() && !snap.asked) {
        return;
    }
    let t = theme::tokens(ctx);
    let (title, detail): (String, String) = match &snap.status {
        Status::Found(v) => (format!("{NAME} {v} is available"), first_line(&snap.notes)),
        Status::Available(v) => (
            format!("{NAME} {v} is available"),
            if download_page().is_some() { "This copy cannot update itself. Get the new version from the download page.".into() } else { "This copy cannot update itself. Install the new version from where you got it.".into() },
        ),
        Status::Downloading(got, all) if *all > 0 => ("Downloading the update".into(), format!("{} of {} MB", got >> 20, (all >> 20).max(1))),
        Status::Downloading(..) => ("Downloading the update".into(), String::new()),
        Status::Ready(v) => (format!("{NAME} {v} is ready"), format!("It installs when {NAME} restarts.")),
        Status::Failed(e) => ("The update did not work".into(), format!("{e}. This version is unchanged.")),
        _ => return,
    };
    let screen = ctx.content_rect();
    let mut clicked: Option<&str> = None;
    egui::Area::new(egui::Id::new("update-notice"))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::RIGHT_BOTTOM)
        .fixed_pos(egui::pos2(screen.right() - 16.0, screen.bottom() - 16.0))
        .interactable(true)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(t.raised)
                .stroke(egui::Stroke::new(1.0, t.line))
                .corner_radius(egui::CornerRadius::same(theme::radius::CARD))
                .shadow(theme::popover_shadow())
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.set_width(300.0);
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(egui::Vec2::splat(16.0), egui::Sense::hover());
                        icons::paint(ui.painter(), r, if matches!(snap.status, Status::Failed(_)) { Icon::Warning } else { Icon::DownloadSimple }, 16.0, t.accent);
                        ui.add(egui::Label::new(egui::RichText::new(crate::i18n::tf(title)).font(theme::control_medium()).color(t.text)).wrap());
                    });
                    if !detail.is_empty() {
                        ui.add(egui::Label::new(egui::RichText::new(crate::i18n::tf(detail)).font(theme::caption()).color(t.text_2)).wrap());
                    }
                    if let Status::Downloading(got, all) = snap.status {
                        let k = if all > 0 { got as f32 / all as f32 } else { 0.0 };
                        let (r, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 4.0), egui::Sense::hover());
                        ui.painter().rect_filled(r, egui::CornerRadius::same(2), t.sunken);
                        ui.painter().rect_filled(egui::Rect::from_min_size(r.min, egui::vec2(r.width() * k.clamp(0.0, 1.0), r.height())), egui::CornerRadius::same(2), t.accent);
                    }
                    ui.add_space(4.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let main = match snap.status {
                            Status::Found(_) => Some("Update now"),
                            Status::Ready(_) => Some("Restart now"),
                            Status::Available(_) if download_page().is_some() => Some("Download page"),
                            Status::Failed(_) => Some("Try again"),
                            _ => None,
                        };
                        if let Some(m) = main {
                            if Button::primary(m).compact().show(ui).clicked() {
                                clicked = Some(m);
                            }
                        }
                        if Button::secondary("Later").compact().show(ui).clicked() {
                            clicked = Some("Later");
                        }
                    });
                });
        });
    match clicked {
        Some("Update now") => state::update_now(),
        Some("Restart now") => restart(app, ctx),
        Some("Download page") => {
            if let Some(u) = download_page() {
                open_page(&u);
            }
            state::later();
        }
        Some("Try again") => state::update_now(),
        Some("Later") => state::later(),
        _ => {}
    }
}

/// The first line of the release notes, without Markdown heading marks.
fn first_line(notes: &str) -> String {
    let line = notes.lines().map(|l| l.trim().trim_start_matches(['#', '-', '*', ' '])).find(|l| !l.is_empty()).unwrap_or_default();
    let mut s: String = line.chars().take(120).collect();
    if line.chars().count() > 120 {
        s.push('…');
    }
    s
}

/// Asks, once, about the crash reports waiting. Only with a report server
/// configured; nothing is sent unless the answer is Send.
fn crash_prompt() {
    let (Some(endpoint), true) = (studio_crash::endpoint(), studio_crash::settings().ask) else { return };
    let reports = studio_crash::pending();
    if reports.is_empty() {
        return;
    }
    let host = endpoint.split("://").nth(1).and_then(|r| r.split('/').next()).unwrap_or("the report server");
    let what = if reports.len() == 1 { "a crash report".to_owned() } else { format!("{} crash reports", reports.len()) };
    let answer = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Warning)
        .set_title("Studio quit unexpectedly")
        .set_description(crate::i18n::tf(format!(
            "Studio saved {what} on this computer ({}). It has the version, your system and a technical trace, and no projects or files.\n\nSend it to {host}?",
            studio_crash::scrub(&studio_crash::dir().display().to_string())
        )))
        .set_buttons(rfd::MessageButtons::YesNoCancelCustom("Send".into(), "Don't send".into(), "Not now".into()))
        .show();
    match answer {
        rfd::MessageDialogResult::Custom(c) if c == "Send" => {
            for r in &reports {
                if studio_crash::send(r, &endpoint).is_ok() {
                    studio_crash::finish(r, false);
                }
            }
        }
        rfd::MessageDialogResult::Custom(c) if c == "Don't send" => reports.iter().for_each(|r| studio_crash::finish(r, false)),
        _ => {}
    }
}

fn ago(secs: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    match now.saturating_sub(secs) {
        _ if secs == 0 => "never".into(),
        s if s < 90 => "just now".into(),
        s if s < 5400 => format!("{} min ago", s / 60),
        s if s < 172_800 => format!("{} h ago", s / 3600),
        s => format!("{} days ago", s / 86_400),
    }
}

/// Settings ▸ About: updates and crash reports.
pub fn settings_section(app: &mut StudioApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    let t = theme::tokens(ctx);
    let snap = state::snapshot();
    Section::new("set-updates", "Updates").icon(Icon::DownloadSimple).fixed().show(ui, |ui| {
        let mut auto = snap.auto_check;
        kit::labeled_row(ui, "Check automatically", |ui| {
            if kit::toggle(ui, &mut auto, "Check for updates automatically", None).changed() {
                state::set_auto_check(auto);
            }
        });
        let line = match &snap.status {
            Status::Idle => crate::i18n::tf(format!("{NAME} {} · last checked {}", snap.current, ago(snap.last_check))),
            Status::Checking => "Checking for updates…".into(),
            Status::UpToDate => crate::i18n::tf(format!("{NAME} {} is up to date.", snap.current)),
            Status::Found(v) => crate::i18n::tf(format!("{NAME} {v} is available.")),
            Status::Downloading(got, all) if *all > 0 => crate::i18n::tf(format!("Downloading the update… {} of {} MB", got >> 20, all >> 20)),
            Status::Downloading(..) => "Downloading the update…".into(),
            Status::Ready(v) => crate::i18n::tf(format!("{NAME} {v} is ready. It installs when {NAME} restarts.")),
            Status::Available(v) => crate::i18n::tf(format!("{NAME} {v} is available. This copy cannot update itself; install the new version from where you got {NAME}.")),
            Status::Failed(e) => crate::i18n::tf(format!("The update did not work: {e}")),
            Status::Disabled(why) => crate::i18n::tf(format!("Updates are off: {why}.")),
        };
        kit::labeled_row(ui, "Status", |ui| {
            ui.add(egui::Label::new(egui::RichText::new(line).font(theme::caption()).color(t.text_2)).wrap());
        });
        if !snap.notes.is_empty() && matches!(snap.status, Status::Found(_) | Status::Ready(_) | Status::Available(_)) {
            kit::labeled_row(ui, "What's new", |ui| {
                ui.add(egui::Label::new(egui::RichText::new(&snap.notes).font(theme::caption()).color(t.text_2)).wrap());
            });
        }
        kit::labeled_row(ui, "", |ui| {
            let busy = matches!(snap.status, Status::Checking | Status::Downloading(..));
            if ui.add_enabled_ui(!busy, |ui| Button::secondary("Check for updates").compact().show(ui)).inner.clicked() {
                state::check_now();
            }
            if let Status::Found(_) = snap.status {
                if Button::primary("Update now").compact().show(ui).clicked() {
                    state::update_now();
                }
            }
            if let Status::Ready(_) = snap.status {
                if Button::primary("Restart now").compact().show(ui).clicked() {
                    restart(app, ctx);
                }
            }
        });
        if matches!(snap.status, Status::Checking | Status::Downloading(..)) {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    });
    Section::new("set-crash", "Crash reports").icon(Icon::Warning).fixed().show(ui, |ui| {
        let mut ask = studio_crash::settings().ask;
        kit::labeled_row(ui, "Offer to send", |ui| {
            if kit::toggle(ui, &mut ask, "Offer to send crash reports", None).changed() {
                studio_crash::set_ask(ask);
            }
        });
        let waiting = studio_crash::pending().len();
        let where_to = if studio_crash::endpoint().is_some() { "A report is sent only if you choose Send." } else { "No report server is set up, so reports stay on this computer." };
        kit::labeled_row(ui, "Saved reports", |ui| {
            ui.label(egui::RichText::new(format!("{waiting} waiting · {where_to}")).font(theme::caption()).color(t.text_2));
        });
        kit::labeled_row(ui, "", |ui| {
            if Button::secondary("Show reports folder").compact().show(ui).clicked() {
                studio_crash::reveal();
            }
        });
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn notes_give_their_first_real_line() {
        assert_eq!(super::first_line("\n## Faster export\n- more"), "Faster export");
        assert_eq!(super::first_line(""), "");
        assert!(super::first_line(&"x".repeat(200)).ends_with('…'));
    }
}
