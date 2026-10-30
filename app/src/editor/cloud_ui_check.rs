// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Headless checks for how an Inspector panel reads: the panel is drawn on
//! the CPU in a column as wide as the Inspector, and the words it shows are
//! checked for what a professional editor would flag (a trailing period on
//! a label, a lower-case start, Title Case, developer text, long
//! sentences, words cut off, controls drawn over each other).
//!
//! Set `ZORNOV_PANEL_SHOTS` to a folder to also get a PNG of each panel.

use eframe::egui::{self, Rect};

/// One panel, drawn.
pub(crate) struct Drawn {
    pub ctx: egui::Context,
    pub out: egui::FullOutput,
    /// Every text drawn, with where it was drawn.
    pub texts: Vec<(String, Rect)>,
    /// Texts drawn shortened with an ellipsis to fit.
    pub shortened: Vec<String>,
    textures: crate::ui_shot::Textures,
}

/// The Inspector's usual width.
pub(crate) const WIDTH: f32 = 300.0;

const SCREEN: egui::Vec2 = egui::vec2(WIDTH + 16.0, 1400.0);

/// Draws `panel` in a column [`WIDTH`] points wide for `frames` frames.
pub(crate) fn draw(ctx: &egui::Context, frames: usize, mut panel: impl FnMut(&mut egui::Ui)) -> Drawn {
    ctx.enable_accesskit();
    let mut textures = crate::ui_shot::Textures::default();
    let mut last = None;
    for _ in 0..frames.max(1) {
        let time = ctx.input(|i| i.time) + 1.0 / 60.0;
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, SCREEN)), time: Some(time), ..Default::default() };
        let mut out = ctx.run_ui(input, |ui| {
            let column = Rect::from_min_size(egui::pos2(8.0, 8.0), egui::vec2(WIDTH, SCREEN.y - 16.0));
            ui.scope_builder(egui::UiBuilder::new().max_rect(column), |ui| {
                ui.set_width(WIDTH);
                panel(ui);
            });
        });
        textures.apply(&out.textures_delta);
        out.textures_delta.clear();
        last = Some(out);
    }
    let out = last.expect("a frame");
    let (mut texts, mut shortened) = (Vec::new(), Vec::new());
    for c in &out.shapes {
        collect(&c.shape, &mut texts, &mut shortened);
    }
    Drawn { ctx: ctx.clone(), out, texts, shortened, textures }
}

fn collect(shape: &egui::Shape, into: &mut Vec<(String, Rect)>, shortened: &mut Vec<String>) {
    match shape {
        egui::Shape::Text(t) => {
            let s = t.galley.text().trim().to_owned();
            if t.galley.elided {
                shortened.push(s.clone());
            }
            if !s.is_empty() {
                into.push((s, t.galley.rect.translate(t.pos.to_vec2())));
            }
        }
        egui::Shape::Vec(v) => v.iter().for_each(|s| collect(s, into, shortened)),
        _ => {}
    }
}

/// Words that keep their capital inside a sentence.
const PROPER: [&str; 14] = ["X", "Y", "Z", "IK", "AI", "Codex", "ChatGPT", "Mac", "ElevenLabs", "Zornov", "Esc", "Best", "PNG", "I"];

/// What reads wrong in `text`, if anything.
pub(crate) fn problem(text: &str) -> Option<&'static str> {
    let t = text.trim();
    // Numbers, units, icons and single letters are not sentences.
    if t.chars().count() <= 3 || !t.chars().any(char::is_alphabetic) || t.chars().next().is_some_and(|c| c.is_ascii_digit() || c == '-' || c == '+') {
        return None;
    }
    if t.ends_with('.') && !t.ends_with("...") {
        return Some("ends with a period");
    }
    if ["Some(", "None", "{", "}", "::", "Ok(", "Err("].iter().any(|d| t.contains(d)) {
        return Some("developer text");
    }
    let snake = t.as_bytes().windows(3).any(|w| w[0].is_ascii_alphanumeric() && w[1] == b'_' && w[2].is_ascii_alphanumeric());
    if snake {
        return Some("a raw id");
    }
    if t.split_whitespace().count() > 14 {
        return Some("too long");
    }
    if t.chars().find(|c| c.is_alphabetic()).is_some_and(char::is_lowercase) {
        return Some("starts in lower case");
    }
    // Title Case: a capital after the first word, not after a sentence's end
    // and not a name.
    let words: Vec<&str> = t.split_whitespace().collect();
    for (i, w) in words.iter().enumerate().skip(1) {
        let after_stop = words[i - 1].ends_with([':', '.', '?', '!', '·', '—']);
        let word = w.trim_matches(|c: char| !c.is_alphanumeric());
        // A quoted name keeps its capitals: under “Change”.
        let quoted = t[..t.find(w).unwrap_or(0)].matches('“').count() > t[..t.find(w).unwrap_or(0)].matches('”').count() || w.starts_with('“');
        if !after_stop && !quoted && word.chars().next().is_some_and(char::is_uppercase) && !PROPER.contains(&word) && word.chars().skip(1).any(char::is_lowercase) {
            return Some("Title Case");
        }
    }
    None
}

impl Drawn {
    /// Every problem in the panel's words, skipping texts in `allow` (names
    /// that come from the project, such as clip names).
    pub(crate) fn problems(&self, allow: &[&str]) -> Vec<String> {
        let mut out: Vec<String> = self.texts.iter().filter(|(s, _)| !allow.iter().any(|a| s.contains(a))).filter_map(|(s, _)| problem(s).map(|p| format!("{p}: {s:?}"))).collect();
        for s in self.shortened.iter().filter(|s| !allow.iter().any(|a| s.contains(a))) {
            out.push(format!("shortened to fit: {s:?}"));
        }
        for c in crate::ui_shot::cut_text(&self.ctx, &self.out, &[], 1.0) {
            out.push(format!("cut off: {:?}", c.text));
        }
        let names = self.out.platform_output.accesskit_update.as_ref().map(|u| u.nodes.iter().map(|(id, n)| (id.0, n.label().or(n.value()).unwrap_or("").to_owned())).collect()).unwrap_or_default();
        for o in crate::ui_shot::overlaps(&self.ctx, &names, 0.2) {
            out.push(format!("overlap: {:?} and {:?}", o.a, o.b));
        }
        out.sort();
        out.dedup();
        out
    }

    /// Panics with every problem found.
    pub(crate) fn assert_tidy(&self, name: &str, allow: &[&str]) {
        self.shot(name);
        let p = self.problems(allow);
        assert!(p.is_empty(), "{name}:\n  {}", p.join("\n  "));
    }

    /// Whether `text` was drawn.
    pub(crate) fn shows(&self, text: &str) -> bool {
        self.texts.iter().any(|(s, _)| s == text)
    }

    /// Where `text` was drawn.
    pub(crate) fn rect_of(&self, text: &str) -> Option<Rect> {
        self.texts.iter().find(|(s, _)| s == text).map(|(_, r)| *r)
    }

    /// A PNG of the panel when `ZORNOV_PANEL_SHOTS` names a folder.
    pub(crate) fn shot(&self, name: &str) {
        let Some(dir) = std::env::var_os("ZORNOV_PANEL_SHOTS") else { return };
        let prims = self.ctx.tessellate(self.out.shapes.clone(), 1.0);
        let size = [SCREEN.x as usize, SCREEN.y as usize];
        let rgba = crate::ui_shot::render(&prims, &self.textures, size, 1.0, crate::theme::tokens(&self.ctx).panel);
        let _ = std::fs::create_dir_all(&dir);
        let _ = crate::ui_shot::save_png(&std::path::Path::new(&dir).join(format!("{name}.png")), size, &rgba);
    }
}

/// A fresh editor whose context has the app's fonts and theme.
pub(crate) fn editor() -> (egui::Context, crate::app::StudioApp) {
    let ctx = egui::Context::default();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    crate::i18n::set_lang(studio_i18n::Lang::En);
    let app = crate::app::StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
    (ctx, app)
}

#[cfg(test)]
mod tests {
    use super::problem;

    #[test]
    fn the_word_check_flags_what_an_editor_would() {
        assert_eq!(problem("Drop from the playhead"), None);
        assert_eq!(problem("Place them above the frame."), Some("ends with a period"));
        assert_eq!(problem("push_in"), Some("a raw id"));
        assert_eq!(problem("Some(3)"), Some("developer text"));
        assert_eq!(problem("biped"), Some("starts in lower case"));
        assert_eq!(problem("Act From Playhead"), Some("Title Case"));
        assert_eq!(problem("Needs the Codex app on this Mac"), None);
        assert_eq!(problem("Turn on live IK"), None);
        assert_eq!(problem("35 %"), None);
    }
}
