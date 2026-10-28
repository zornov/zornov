// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What each op field is, for `edit_schema`: its type, units, range or
//! choices and a line on what it means, plus one example call per op taken
//! from the guides. `edit_schema {op: "act"}` returned bare names, and
//! agents guessed the units (speed 145, opacity 0.5, at in clip time).
//!
//! Fields are described once by name, with the ops that read a name
//! differently overriding it; a field not known here still gets its type
//! guessed from its name rather than nothing.

use serde_json::{Value, json};

/// Ops whose `at` counts from the clip's own start, not the timeline.
const CLIP_TIME_AT: &[&str] = &["act"];

/// (type, units, range or choices, meaning) for a field of `op`.
fn known(op: &str, field: &str) -> Option<(&'static str, &'static str, &'static str, &'static str)> {
    // Ops that read a common name their own way.
    let own = match (op, field) {
        ("act", "speed") => Some(("number", "times the gait's own pace", "0.1 to 6 (1 = normal)", "a multiplier, not px/s; for a set pace give speed_px_s, or duration")),
        ("act", "speed_px_s") => Some(("number", "world px per second", "above 0", "walking pace in px per second, instead of speed")),
        ("act", "duration") => Some(("number", "seconds", "above 0", "with a route, to or by: how long the whole walk takes (the speed is found from it); a plain gait walks this long")),
        ("act", "face") => Some(("number | word | {clip}", "degrees", "0 toward the camera, 90 right, 180 away, -90 left; or right, left, camera, away", "the way to face at the end; with a gait and no route, the way to walk")),
        ("act", "motion") => Some(("string", "", "walk, run, sneak, idle, wave, jump, … (edit_schema lists none: guide {topic: \"act\"})", "the motion to play or the gait to walk")),
        ("act", "route") => Some(("array", "canvas px on the ground", "[[x, z], …] or {at, pause, face}", "points to walk through in order")),
        ("walk", "speed") => Some(("number", "metres per second", "0.3 to 12", "walking pace of the camera")),
        ("walk", "route") => Some(("array", "metres", "[[x, z], …]", "points the camera walks through")),
        ("walk", "start") => Some(("number", "seconds, timeline time", "0 or more", "when the walk starts")),
        ("stand", "t") => Some(("number", "seconds, timeline time", "0 or more", "the moment the camera stands there")),
        ("stand", "at") => Some(("[x, z]", "metres", "", "where to stand in the world")),
        ("light", "intensity") => Some(("number", "percent", "0 to 1000 (100 = normal)", "how bright")),
        ("light", "glow" | "flare" | "beam" | "flicker") => Some(("number | true", "amount", "0 to 1 (true = 1)", "how strong that look is")),
        ("light", "flame") => Some(("boolean", "", "true / false (or 1 / 0)", "draw the light as a flame")),
        ("lighting", "mood" | "preset") => Some(("string", "", "a preset name; free words (\"night city\") map to the nearest", "the lighting set-up to build")),
        ("model", "shape") => Some(("string", "", "sphere, cube, cylinder, cone, torus, plane, head, person, child, toon, … (required unless file or parts)", "what to make")),
        ("model", "figure") => Some(("string | number", "", "masculine (man), neutral, feminine (woman), or -1 to 1", "a character's build")),
        ("model", "style") => Some(("string | number", "", "realistic, stylised, toon (anime, cartoon mean toon), or 0 to 1", "how a character is drawn")),
        ("camera", "new") => Some(("boolean", "", "true / false", "add another camera rather than change the one at that time")),
        ("camera", "focal_length") => Some(("number", "mm", "8 to 300", "lens length (35 normal, 85 portrait)")),
        ("export", "wait") => Some(("boolean", "", "default true", "wait for the file; a long render returns a job id for export_status")),
        ("chart", "kind") => Some(("string", "", "bar, line, pie, donut, map", "the kind of chart")),
        ("chart", "data") => Some(("array | string", "", "[[\"label\", value], …] or \"label, value\" lines; map: [[\"place\", lon, lat], …]", "the numbers to chart")),
        ("shape", "items") => Some(("array", "canvas px", "[{\"type\": \"rect\" | \"ellipse\" | \"star\" | \"path\" | \"group\", …}]", "the shapes to draw (or give preset)")),
        ("shape", "preset") => Some(("string", "", "a shape preset name (edit_schema can't list them: guide {topic: \"shape\"})", "a ready-made shape, instead of items")),
        ("template", "id") => Some(("string", "", "a template id (template {list: true} lists them)", "which template to fill in")),
        ("template", "controls") => Some(("object", "", "{control id: value}", "the template's fields")),
        ("stagger", "animate") => Some(("object", "", "an animate op without clip (opacity keys are 0 to 100)", "what each clip does, timed for the first")),
        ("set", "opacity") => Some(("number", "percent", "0 to 100", "how opaque")),
        ("set", "colour") => Some(("string", "#rrggbb or a name", "", "a title's text colour, a card's or shape's fill, a light's colour")),
        ("sfx", "at") => Some(("number", "seconds, timeline time", "0 or more", "where the sound starts (or give ends_at)")),
        _ => None,
    };
    if own.is_some() {
        return own;
    }
    Some(match field {
        "at" if CLIP_TIME_AT.contains(&op) => ("number", "seconds from the clip's start (clip time)", "0 to the clip's length", "when it happens, counted from the clip's own start"),
        "at" => ("number", "seconds, timeline time", "0 or more", "when it happens on the timeline"),
        "t" => ("number", "seconds, timeline time", "0 or more", "a moment on the timeline"),
        "time" => ("number", "seconds", "0 or more", "a moment"),
        "duration" => ("number", "seconds", "above 0", "how long"),
        "start" => ("number", "seconds, timeline time", "0 or more", "where it starts"),
        "end" | "ends_at" => ("number", "seconds, timeline time", "", "where it ends"),
        "source_in" | "source_out" => ("number", "seconds into the media file", "", "which part of the file is used"),
        "clip" => ("integer | string", "", "a clip id, \"$1\" (the first clip made earlier in this edit), \"$last\", \"lane@4.2\" or a name", "the clip to work on"),
        "clips" => ("array", "", "clip ids or references", "the clips to work on"),
        "media" => ("integer | string", "", "a media id, name or file path", "the media to use"),
        "lane" => ("integer | string", "", "a lane id or name (main, overlay, sound, music, top)", "which lane"),
        "x" | "y" => ("number", "px from the canvas centre", "", "position"),
        "z" => ("number", "px, positive away from the camera", "", "depth"),
        "position" => ("[x, y, z]", "px", "", "where it is"),
        "rotation" | "rotation_x" | "rotation_y" | "rotation_z" | "roll" | "yaw" | "pitch" => ("number", "degrees", "", "turn"),
        "scale" => ("number", "percent", "100 = actual size", "size"),
        "opacity" => ("number", "percent", "0 to 100", "how opaque"),
        "volume" => ("number", "dB", "-60 to +12 (0 = as recorded)", "loudness"),
        "pan" => ("number", "percent", "-100 left to 100 right (0 = centre)", "stereo place"),
        "fade_in" | "fade_out" => ("number", "seconds", "0 or more", "fade length"),
        "speed" => ("number", "times", "above 0 (1 = normal)", "playback speed"),
        "colour" | "side_colour" | "accent" | "fill" | "stroke" | "skin" | "top" | "trousers" | "shoes" | "hair" | "fur" | "emissive" => ("string", "#rrggbb or a basic name", "", "a colour"),
        "metallic" | "roughness" | "transmission" | "subsurface" | "wetness" => ("number", "", "0 to 1", "surface finish"),
        "intensity" | "strength" | "amount" => ("number", "", "", "how strong"),
        "ease" => ("string", "", "linear, smooth, ease_in, ease_out, ease_in_out (in, out, in_out also), back.out, bounce, elastic, snap, …", "how the motion speeds up and slows down"),
        "keys" => ("array", "", "[[t, value], …] or [{t, v, ease}]; t from the clip's start unless timeline: true", "keyframes"),
        "name" => ("string", "", "", "a name for what is made"),
        "text" => ("string", "", "", "the words"),
        "font" => ("string", "", "a font family (fonts lists them)", "typeface"),
        "size" | "text_size" => ("number", "px", "above 0", "size"),
        "width" | "height" | "radius" | "depth" | "bevel" | "stroke_width" | "eye" => ("number", "px (metres in worlds)", "0 or more", "a length"),
        "fov" => ("number", "degrees", "10 to 120", "field of view"),
        "seed" => ("integer", "", "any", "variation; the same seed gives the same result"),
        "count" => ("integer", "", "1 or more", "how many"),
        "look_at" => ("[x, y, z] | clip", "px", "", "what to aim at"),
        "remove" => ("boolean", "", "true", "take it away instead"),
        "loop" => ("boolean", "", "", "repeat"),
        "preset" => ("string", "", "a preset name (guide for the op lists them)", "a ready-made set-up"),
        "region" => ("object", "px", "{sphere: {at, radius}} or a box", "where it applies"),
        "steps" => ("array", "", "[{\"do\": …}, …]", "steps run in order"),
        "ops" => ("array", "", "", "ops run in order"),
        "say" => ("string", "", "", "the words spoken"),
        "voice" => ("string", "", "a voice name (`voices` lists them)", "who speaks"),
        _ => return None,
    })
}

/// One field described.
pub fn field(op: &str, name: &str) -> Value {
    match known(op, name) {
        Some((ty, units, range, means)) => {
            let mut v = json!({"type": ty, "means": means});
            if !units.is_empty() {
                v["units"] = json!(units);
            }
            if !range.is_empty() {
                v["range"] = json!(range);
            }
            v
        }
        // Guessed from the name (a switch, a list, or anything), with the
        // sentence the guides say about it when there is one.
        None => {
            let ty = if ["enabled", "invert", "live", "mirror", "visible", "shadows", "relative", "timeline"].contains(&name) { "boolean" } else if name.ends_with('s') && !name.ends_with("ss") { "array or value" } else { "any" };
            let means = said(op, name).unwrap_or_else(|| "not described in the guides yet; a wrong value is refused with what it takes".to_owned());
            json!({"type": ty, "means": means})
        }
    }
}

/// What the guides and skills say about `op`: paragraphs that call it
/// first, then table rows and sentences that name it as an op.
pub fn paragraphs(op: &str) -> Vec<&'static str> {
    let call = format!("\"op\": \"{op}\"");
    let row = format!("| `{op}`");
    let named = [format!("`{op}` op"), format!("op `{op}`"), format!("**`{op}`**")];
    let all: Vec<&'static str> = sources().into_iter().chain(super::skills::texts().iter().copied()).flat_map(|t| t.split("\n\n")).collect();
    let mut out: Vec<&'static str> = all.iter().copied().filter(|p| p.contains(&call) && !p.trim_start().starts_with('|')).collect();
    for p in &all {
        if p.trim_start().starts_with('|') {
            out.extend(p.lines().filter(|l| l.starts_with(&row)));
        } else if !p.contains(&call) && named.iter().any(|n| p.contains(n.as_str())) {
            out.push(p);
        }
    }
    out
}

/// The sentence the guides say about `op`'s field `name`, if any: a
/// table row's bracketed note first, else a sentence naming both.
fn said(op: &str, name: &str) -> Option<String> {
    let (tick, key) = (format!("`{name}`"), format!("\"{name}\":"));
    let paras = paragraphs(op);
    for p in paras.iter().filter_map(|p| p.strip_prefix('|')) {
        // Only a bracketed note right after the name says something.
        let fields = p.split(" | ").nth(1).unwrap_or("");
        let Some(i) = fields.find(&tick) else { continue };
        let Some(rest) = fields[i + tick.len()..].strip_prefix(" (") else { continue };
        let mut depth = 1;
        let end = rest.char_indices().find(|&(_, c)| {
            depth += match c {
                '(' => 1,
                ')' => -1,
                _ => 0,
            };
            depth == 0
        });
        if let Some((e, _)) = end {
            return Some(rest[..e].trim().to_owned());
        }
    }
    let (call, named) = (format!("\"op\": \"{op}\""), format!("`{op}`"));
    for p in paras.iter().filter(|p| !p.starts_with('|')) {
        let flat = p.replace("\n- ", ". ").replace("\n* ", ". ");
        for sentence in flat.split(". ") {
            if (sentence.contains(&tick) || sentence.contains(&key)) && (sentence.contains(&call) || sentence.contains(&named)) {
                let s: String = sentence.split_whitespace().collect::<Vec<_>>().join(" ");
                let s = s.trim_start_matches(['-', '*', ' ']).to_owned();
                return Some(if s.chars().count() > 240 { s.chars().take(237).collect::<String>() + "..." } else { s });
            }
        }
    }
    None
}

/// The guides an example is looked for in, in order.
fn sources() -> [&'static str; 6] {
    [super::GUIDE, super::TOOL_REFERENCE, super::GUIDE_CORE, super::GUIDE_CHARACTER, super::craft_book::BOOK, super::brain::VIDEO_CRAFT]
}

/// A JSON object starting at byte `i` of `s` (brace-matched, strings
/// skipped), parsed.
fn object_at(s: &str, i: usize) -> Option<Value> {
    let b = s.as_bytes();
    let (mut depth, mut in_str, mut esc) = (0i32, false, false);
    for (k, &c) in b.iter().enumerate().skip(i) {
        if in_str {
            match (esc, c) {
                (true, _) => esc = false,
                (false, b'\\') => esc = true,
                (false, b'"') => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return serde_json::from_str(&s[i..=k]).ok();
                }
            }
            b'\n' if depth == 0 => return None,
            _ => {}
        }
    }
    None
}

/// One example call of `op`, as the guides write it.
pub fn example(op: &str) -> Option<Value> {
    let needle = format!("{{\"op\": \"{op}\"");
    for text in sources() {
        let mut from = 0;
        while let Some(i) = text[from..].find(&needle) {
            let at = from + i;
            if let Some(v) = object_at(text, at) {
                return Some(v);
            }
            from = at + needle.len();
        }
    }
    None
}

/// Every op `edit` runs: the ones it dispatches, read from the dispatcher
/// itself so a new op can't be left out, that have a field list.
pub fn all_ops() -> Vec<&'static str> {
    let src: &'static str = include_str!("mod.rs");
    let body = src.find("fn op(&mut self").map_or("", |i| &src[i..]);
    let body = body.find("\n    }\n").map_or(body, |e| &body[..e]);
    let mut out: Vec<&'static str> = Vec::new();
    for line in body.lines() {
        let l = line.trim_start();
        if !l.starts_with('"') || !l.contains("=>") {
            continue;
        }
        let head = l.split("=>").next().unwrap_or("");
        let head = head.split(" if ").next().unwrap_or(head);
        for w in head.split('|') {
            let w = w.trim().trim_matches('"');
            if !w.is_empty() && w.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') && super::args::op_fields(w).is_some() && !out.contains(&w) {
                out.push(w);
            }
        }
    }
    for &w in super::shell::OPS {
        if super::args::op_fields(w).is_some() && !out.contains(&w) {
            out.push(w);
        }
    }
    out.sort_unstable();
    out
}

/// An op in full: each field described, an example and where to read more.
pub fn op_doc(op: &str, fields: &[&str]) -> Value {
    let mut map = serde_json::Map::new();
    for f in fields {
        map.insert((*f).to_owned(), field(op, f));
    }
    let mut v = json!({"fields": map, "guide": format!("guide {{topic: \"{op}\"}}")});
    if let Some(e) = example(op) {
        v["example"] = e;
    }
    v
}
