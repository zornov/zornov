// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What an Inspector action tells the person: what happened, or why it
//! could not. The ops behind the panels answer the AI in its own terms
//! ("No clip 12 (see summary for ids)"); [`plain`] turns that into a
//! sentence for a person, with the clip's name for its id.

use crate::doc::Doc;
use crate::icons::Icon;
use crate::kit;
use eframe::egui;

/// `e` for a person: the AI's asides and op names gone, a clip's id as
/// its name, a capital to start and no period to end.
pub(crate) fn plain(doc: &Doc, e: &str) -> String {
    let mut s = e.trim().to_owned();
    // Asides meant for the AI: "(see summary for ids)", "(use the `x` op)".
    while let Some(open) = s.find(" (") {
        let Some(len) = s[open..].find(')') else { break };
        let aside = &s[open..open + len + 1];
        if ["summary", " op", "`", "ids", "\""].iter().any(|w| aside.contains(w)) {
            s.replace_range(open..open + len + 1, "");
        } else {
            break;
        }
    }
    // "No clip 12": the clip is not there any more.
    if let Some(n) = s.strip_prefix("No clip ")
        && !n.is_empty()
        && n.chars().all(|c| c.is_ascii_digit())
    {
        return "That clip is gone".into();
    }
    // "clip 12" → its name.
    let mut out = String::new();
    let mut rest = s.as_str();
    while let Some(i) = rest.to_ascii_lowercase().find("clip ") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 5..];
        let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
        let name = digits.parse::<u32>().ok().and_then(|id| doc.project.clip(id)).map(|c| c.name.trim().to_owned()).filter(|n| !n.is_empty());
        match name {
            Some(n) => {
                out.push_str(&format!("“{n}”"));
                rest = &after[digits.len()..];
            }
            None if !digits.is_empty() => {
                out.push_str("that clip");
                rest = &after[digits.len()..];
            }
            None => {
                out.push_str(&rest[i..i + 5]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    let mut s = out.trim().trim_end_matches('.').to_owned();
    if let Some(c) = s.chars().next() {
        s.replace_range(..c.len_utf8(), &c.to_uppercase().to_string());
    }
    s
}

/// A rig's raw name as a person reads it: `upper_arm.L` is "Upper arm ·
/// left", `leg_front.R.1` is "Leg front · right · 1", `mouth` is "Mouth".
/// The raw name stays what the ops and the AI use.
pub(crate) fn shown_name(raw: &str) -> String {
    let mut parts = raw.split(['.', ':']).filter(|p| !p.is_empty());
    let Some(first) = parts.next() else { return String::new() };
    let mut base = first.replace(['_', '-'], " ");
    // camelCase words apart: "LeftUpLeg" → "left up leg".
    let mut spaced = String::new();
    let mut prev: Option<char> = None;
    for c in base.chars() {
        if c.is_uppercase() && prev.is_some_and(char::is_lowercase) {
            spaced.push(' ');
        }
        spaced.extend(c.to_lowercase());
        prev = Some(c);
    }
    // Acronyms stay as they are.
    if !(first.len() > 1 && first.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())) {
        base = spaced;
    }
    let mut out = vec![base];
    for p in parts {
        out.push(
            match p {
                "L" | "l" => "left",
                "R" | "r" => "right",
                other => other,
            }
            .replace('_', " ")
            .to_lowercase(),
        );
    }
    let s = out.join(" · ");
    let mut c = s.chars();
    c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
}

/// Tells the person an action worked.
pub(crate) fn done(ctx: &egui::Context, text: impl Into<String>) {
    kit::push(ctx, text, None, Some(Icon::Check));
}

/// Tells the person why an action could not run (`e` made [`plain`]).
pub(crate) fn failed(ctx: &egui::Context, doc: &Doc, e: &str) {
    kit::push(ctx, plain(doc, e), None, Some(Icon::Warning));
}

/// Tells the person what to do first.
pub(crate) fn hint(ctx: &egui::Context, text: impl Into<String>) {
    kit::push(ctx, text, None, Some(Icon::Info));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_for_the_ai_read_plainly() {
        let mut s = crate::agent::Session::new(crate::services::project::new_custom("Plain", 640, 360, 30.0));
        s.edit(&[serde_json::json!({"op": "card", "colour": "#ffcc00", "at": 0, "duration": 2})], "card", false).unwrap();
        let id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        let name = s.doc.project.clip(id).unwrap().name.clone();
        assert_eq!(plain(&s.doc, "No clip 9999 (see summary for ids)"), "That clip is gone");
        let said = plain(&s.doc, &format!("clip {id} is a 3D layer; physics works on flat layers."));
        assert!(said.starts_with('“') || said.starts_with("That clip"), "{said}");
        if !name.is_empty() {
            assert!(said.contains(&name), "{said}");
        }
        assert!(!said.ends_with('.'));
        assert_eq!(plain(&s.doc, "the shot is gone"), "The shot is gone");
    }

    #[test]
    fn rig_names_read_plainly() {
        assert_eq!(shown_name("upper_arm.L"), "Upper arm · left");
        assert_eq!(shown_name("leg_front.R.1"), "Leg front · right · 1");
        assert_eq!(shown_name("mouth"), "Mouth");
        assert_eq!(shown_name("upper_arm.front"), "Upper arm · front");
        assert_eq!(shown_name("Bone 3"), "Bone 3");
        assert_eq!(shown_name("LeftHand"), "Left hand");
        assert_eq!(shown_name("IK"), "IK");
    }
}
