// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What the AI panel tells the person about a request, in plain words.
//!
//! The AI works through many tool calls; most of them are its own
//! harness (reading the guide, the summary, its notes) and mean nothing
//! to the person. This turns the raw steps of one request into what does:
//! the edits made (same kind in a row grouped with a count), what was
//! generated or brought in, the checks and how they came out, and which
//! steps showed pictures. It also words the one live "doing" line. It
//! never carries a guide, a prompt or a tool's arguments into the text.

use crate::icons::Icon;

/// How far along one tool call is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Running,
    Ok,
    Failed,
}

/// One tool call as the digest sees it.
#[derive(Clone, Copy, Debug)]
pub struct Step<'a> {
    pub tool: &'a str,
    pub brief: &'a str,
    pub what: &'a str,
    pub state: State,
    pub result: &'a str,
}

/// What a tool call is to the person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// The AI's own reading and bookkeeping: never shown as a result.
    Harness,
    Edit,
    Asset,
    Look,
    Check,
    /// Anything else: only in the full step list.
    Other,
}

pub fn class(tool: &str) -> Class {
    match tool {
        "guide" | "summary" | "script" | "inspect" | "pick" | "canvas_layout" | "notes" | "edit_schema" | "capabilities" | "plan" | "taste" | "skill" | "media" | "catalogue" | "made" | "voices" | "moments" | "analyse" | "listen" | "match" | "export_status" | "schema" | "tools" | "craft" | "craft_book" | "prompt" => Class::Harness,
        "edit" | "undo" | "redo" | "export" | "save" | "open" | "new_project" | "save_move" | "save_design" | "make_playable" => Class::Edit,
        "generate" | "import" | "download" => Class::Asset,
        "see" | "frame" | "contact_sheet" => Class::Look,
        "check" => Class::Check,
        _ => Class::Other,
    }
}

/// Steps whose answer is the AI's reading material: the answer is not
/// kept for display at all.
pub fn is_reading(tool: &str) -> bool {
    class(tool) == Class::Harness || matches!(tool, "guide" | "edit_schema")
}

/// One line of the compact result list.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub icon: Icon,
    pub text: String,
    pub count: usize,
    /// Which steps (indices into the digested slice) it stands for.
    pub steps: Vec<usize>,
}

impl Row {
    /// The line as shown: the text, with `×n` when it stands for several.
    pub fn shown(&self) -> String {
        if self.count > 1 { format!("{} ×{}", self.text, self.count) } else { self.text.clone() }
    }
}

#[derive(Debug, Default)]
pub struct Digest {
    pub rows: Vec<Row>,
    /// Steps that showed pictures of the work (frames, contact sheets).
    pub looked: Vec<usize>,
    /// Steps that made assets (their pictures are thumbnails).
    pub made: Vec<usize>,
}

fn word(s: &str) -> String {
    crate::i18n::t(s).into_owned()
}

/// The ops named in an `edit` step's brief (`"3 ops: montage, set"`, or a
/// single op, possibly after a crew role).
fn ops_of(brief: &str) -> Vec<&str> {
    let b = brief.rsplit(" · ").next().unwrap_or(brief);
    let list = b.split_once(" ops: ").map_or(b, |(_, l)| l);
    list.split(',').map(str::trim).filter(|o| !o.is_empty() && o.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')).filter(|o| *o != "no_ops").collect()
}

/// An edit in plain words from its first op.
fn edit_phrase(op: Option<&str>) -> String {
    word(match op.unwrap_or("") {
        "music" => "Placed music",
        "sfx" | "foley" => "Added sound effects",
        "montage" | "string_out" | "sequence" => "Built a sequence",
        "cut" | "split" | "text_cut" | "roll" | "lift" | "trim" => "Cut and trimmed",
        "delete" | "remove" => "Removed clips",
        "merge" | "unmerge" => "Merged clips",
        "move" | "swap" | "duplicate" | "replace" | "take" | "overwrite" => "Rearranged clips",
        "place" | "add" | "insert" | "shot" | "track" | "lane" => "Placed clips",
        "card" | "text" | "shape" | "chart" | "template" | "layer" => "Added graphics",
        "animate" | "keys" | "camera" | "motion" | "smooth_motion" | "stagger" | "expression" | "driver" | "repeat" => "Added motion",
        "apply" | "effect" | "modifier" | "match_colour" | "lighting" | "light" | "light_match" | "shadow" | "particles" | "mask" | "roto" | "cut_out" | "foreground" | "paint" | "corner_pin" | "stabilize" => "Applied effects",
        "three_d" | "model" | "world" | "simulate" | "physics" | "puppet" | "lip_sync" | "scissor" | "mask_puppet" | "rig" | "skeleton" | "pose" | "ik" | "living" | "act" => "Worked on 3D and characters",
        "marker" => "Added markers",
        "transition" => "Added a transition",
        "speed_ramp" | "time_remap" | "slowmo" => "Changed speed",
        "set" => "Adjusted settings",
        _ => "Edited the timeline",
    })
}

/// Whether an `edit` step really changed the project (a dry run, or an
/// answer that changed nothing, is not an edit).
fn changed_something(result: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(result) else { return true };
    if v.get("would_change").is_some() {
        return false;
    }
    match v.get("changed") {
        Some(c) => ["added", "removed", "changed"].iter().any(|k| c[*k].as_array().is_none_or(|a| !a.is_empty())) || c.as_object().is_none_or(|o| o.is_empty()),
        None => true,
    }
}

/// The problems a `check` found, if its answer says.
fn problems(result: &str) -> Option<usize> {
    serde_json::from_str::<serde_json::Value>(result).ok()?.get("problems")?.as_array().map(Vec::len)
}

/// Words and icon for a finished step that is a result, with a key that
/// lets neighbours of the same kind merge.
fn result_row(i: usize, s: &Step, prior_problems: &mut Option<usize>) -> Option<(String, Row)> {
    let row = |icon, text: String| Row { icon, text, count: 1, steps: vec![i] };
    match class(s.tool) {
        Class::Edit => {
            let (text, key) = match s.tool {
                "edit" => {
                    if !changed_something(s.result) {
                        return None;
                    }
                    let phrase = edit_phrase(ops_of(s.brief).first().copied());
                    if s.what.is_empty() { (phrase.clone(), phrase) } else { (s.what.to_owned(), s.what.to_lowercase()) }
                }
                "undo" => (word("Undid a change"), "undo".into()),
                "redo" => (word("Redid a change"), "redo".into()),
                "export" => (word("Started an export"), "export".into()),
                "save" => (word("Saved the project"), "save".into()),
                "open" => (word("Opened a project"), "open".into()),
                "new_project" => (word("Started a new project"), "new".into()),
                "save_move" | "save_design" => (word("Saved to your library"), "lib".into()),
                _ => (word("Converted media so it plays"), "playable".into()),
            };
            let icon = match s.tool {
                "undo" => Icon::ArrowUUpLeft,
                "redo" => Icon::ArrowUUpRight,
                "export" => Icon::Export,
                "edit" => Icon::Scissors,
                _ => Icon::FolderOpen,
            };
            Some((format!("e:{key}"), row(icon, text)))
        }
        Class::Asset => {
            let (text, key) = match s.tool {
                "generate" if s.what.is_empty() => (word("Generated media"), "gen".to_owned()),
                "generate" => (format!("{} {}", word("Generated"), s.what), format!("gen:{}", s.what)),
                "download" => (word("Downloaded media"), "dl".into()),
                _ => (word("Imported media"), "imp".into()),
            };
            Some((format!("a:{key}"), row(if s.tool == "generate" { Icon::Sparkle } else { Icon::DownloadSimple }, text)))
        }
        Class::Check => {
            let now = problems(s.result);
            let text = match (now, *prior_problems) {
                (Some(0), Some(n)) if n > 0 => word("Fixed"),
                (Some(0), _) => word("No problems found"),
                (Some(n), _) => format!("{n} {}", word(if n == 1 { "problem to fix" } else { "problems to fix" })),
                (None, _) => word("Checked the edit"),
            };
            if now.is_some() {
                *prior_problems = now;
            }
            Some((format!("c:{text}"), row(Icon::CheckCircle, text)))
        }
        _ => None,
    }
}

/// The compact list for a request's steps, oldest first.
pub fn digest(steps: &[Step]) -> Digest {
    let mut d = Digest::default();
    let mut last_key = String::new();
    let mut prior = None;
    for (i, s) in steps.iter().enumerate() {
        if s.state != State::Ok {
            continue;
        }
        match class(s.tool) {
            Class::Look => d.looked.push(i),
            _ => {
                if s.tool == "generate" {
                    d.made.push(i);
                }
                if let Some((key, row)) = result_row(i, s, &mut prior) {
                    match d.rows.last_mut() {
                        Some(last) if last_key == key => {
                            last.count += 1;
                            last.steps.push(i);
                        }
                        _ => {
                            d.rows.push(row);
                            last_key = key;
                        }
                    }
                }
            }
        }
    }
    d
}

/// The one live line: what the AI is doing now, in plain words. `plan` is
/// the title of the plan step under way, if there is one.
pub fn doing(steps: &[Step], plan: Option<&str>) -> String {
    if let Some(t) = plan.map(str::trim).filter(|t| !t.is_empty()) {
        return t.trim_end_matches(['.', '…']).to_owned();
    }
    let Some(s) = steps.iter().rev().find(|s| s.state == State::Running) else { return word("Thinking") };
    match s.tool {
        "edit" if !s.what.is_empty() => s.what.trim_end_matches(['.', '…']).to_owned(),
        "edit" => word("Editing the timeline"),
        "undo" | "redo" => word("Adjusting the edit"),
        "generate" if !s.what.is_empty() => format!("{} {}", word("Generating"), s.what),
        "generate" | "ai" => word("Generating media"),
        "import" | "download" | "make_playable" => word("Bringing in media"),
        "export" => word("Exporting"),
        "save" | "save_move" | "save_design" => word("Saving"),
        "open" | "new_project" => word("Opening the project"),
        "see" | "frame" | "contact_sheet" => word("Looking at the result"),
        "check" => word("Checking the edit"),
        "listen" => word("Listening to the sound"),
        "moments" | "analyse" => word("Studying the footage"),
        "catalogue" | "made" | "voices" => word("Browsing the library"),
        "match" => word("Finding matching shots"),
        _ => word("Thinking"),
    }
}

/// One line for the full step list: what the step was, plus a trimmed
/// hint of its call and answer. Reading steps carry nothing but their
/// name, so no guide or prompt text can get through.
pub fn detail(s: &Step) -> String {
    if is_reading(s.tool) {
        return String::new();
    }
    let one = |t: &str| crate::agent::brain::one_line(t, 90);
    let brief = one(s.brief);
    let answer = match s.state {
        State::Failed => one(s.result),
        _ => match serde_json::from_str::<serde_json::Value>(s.result) {
            Ok(v) => outcome(s.tool, &v).unwrap_or_default(),
            Err(_) => one(s.result),
        },
    };
    match (brief.is_empty(), answer.is_empty()) {
        (true, true) => String::new(),
        (false, true) => brief,
        (true, false) => answer,
        (false, false) => one(&format!("{brief} → {answer}")),
    }
}

/// A finished step's result in a few words (edits: what changed).
pub fn outcome(tool: &str, v: &serde_json::Value) -> Option<String> {
    if tool == "edit" {
        let c = &v["changed"];
        let n = |k: &str| c[k].as_array().map_or(0, Vec::len);
        let bits: Vec<String> = [("added", "added"), ("removed", "removed"), ("changed", "changed")].iter().filter(|(k, _)| n(k) > 0).map(|(k, w)| format!("{} {w}", n(k))).collect();
        return (!bits.is_empty()).then(|| bits.join(" · "));
    }
    v["problems"].as_array().map(|p| if p.is_empty() { "no problems found".into() } else { format!("{} {}", p.len(), if p.len() == 1 { "problem to fix" } else { "problems to fix" }) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step<'a>(tool: &'a str, brief: &'a str, what: &'a str, state: State, result: &'a str) -> Step<'a> {
        Step { tool, brief, what, state, result }
    }

    #[test]
    fn guide_reads_never_become_rows() {
        let steps: Vec<Step> = (0..6).map(|_| step("guide", "", "", State::Ok, "SECRET GUIDE TEXT")).chain([step("summary", "", "", State::Ok, "{}")]).collect();
        let d = digest(&steps);
        assert!(d.rows.is_empty() && d.looked.is_empty());
        assert_eq!(detail(&steps[0]), "", "a guide read carries no text");
    }

    #[test]
    fn edits_of_one_kind_in_a_row_share_a_row() {
        let ok = r#"{"changed":{"added":[{"id":1}],"removed":[],"changed":[]}}"#;
        let steps: Vec<Step> = (0..12).map(|_| step("edit", "cut", "Cut on the beat", State::Ok, ok)).collect();
        let d = digest(&steps);
        assert_eq!(d.rows.len(), 1);
        assert_eq!(d.rows[0].shown(), "Cut on the beat ×12");
    }

    #[test]
    fn dry_runs_are_not_edits_and_ops_give_words() {
        let dry = r#"{"would_change":{"added":[]}}"#;
        let real = r#"{"changed":{"added":[{"id":1}],"removed":[],"changed":[]}}"#;
        let steps = [step("edit", "2 ops: music, set", "", State::Ok, dry), step("edit", "2 ops: music, set", "", State::Ok, real)];
        let d = digest(&steps);
        assert_eq!(d.rows.iter().map(Row::shown).collect::<Vec<_>>(), vec!["Placed music"]);
    }

    #[test]
    fn a_check_then_the_fix() {
        let steps = [step("check", "", "", State::Ok, r#"{"problems":[1,2]}"#), step("check", "", "", State::Ok, r#"{"problems":[]}"#)];
        let d = digest(&steps);
        assert_eq!(d.rows.iter().map(Row::shown).collect::<Vec<_>>(), vec!["2 problems to fix", "Fixed"]);
    }

    #[test]
    fn looks_collect_and_the_live_line_is_plain() {
        let steps = [step("see", "Trailer · 2s", "", State::Ok, ""), step("contact_sheet", "", "", State::Ok, ""), step("guide", "", "", State::Running, "")];
        assert_eq!(digest(&steps).looked, vec![0, 1]);
        assert_eq!(doing(&steps, None), "Thinking");
        assert_eq!(doing(&steps, Some("Mixing sound.")), "Mixing sound");
        let editing = [step("edit", "montage", "Build the product shot", State::Running, "")];
        assert_eq!(doing(&editing, None), "Build the product shot");
    }

    #[test]
    fn details_are_one_trimmed_line() {
        let long = "x".repeat(500);
        let answer = format!("{long}\n{long}");
        let s = step("generate", &long, "", State::Failed, &answer);
        let line = detail(&s);
        assert!(!line.contains('\n') && line.chars().count() <= 95, "{}", line.len());
    }
}
