// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `capabilities`: can Studio do this, and with what? One quick answer
//! from every tool, every `edit` op, the guide's sections and the craft
//! book's chapters, so an agent checks instead of guessing (an agent once
//! put still panels in place of animated characters, sure that animation
//! was not possible; it was one op away).

use serde_json::Value;
use std::collections::HashSet;
use std::sync::OnceLock;

/// One thing Studio has.
struct Entry {
    /// "tool", "op", "row" (a kind or field of an op, in a table),
    /// "guide", "craft" or "skill".
    kind: &'static str,
    name: String,
    /// What it does (searchable, shown short).
    does: String,
    /// The name in lower case, for matching.
    key: String,
    /// The stemmed words of what it does, for matching.
    words: HashSet<String>,
    /// How many words it has (long sections match everything a little).
    len: usize,
}

/// The words of `text`, lower case and stemmed.
fn words(text: &str) -> Vec<String> {
    text.to_lowercase().split(|c: char| !c.is_alphanumeric() && c != '_').filter(|w| !w.is_empty()).map(stem).collect()
}

/// The quoted names before `=>` on the outermost arms of a `match` on an
/// op's name in `body` (arms of matches nested inside an op are deeper).
fn arms(body: &str) -> Vec<String> {
    let indent = |l: &str| l.len() - l.trim_start().len();
    let is_arm = |l: &&str| l.trim_start().starts_with('"') && l.contains("=>");
    let Some(top) = body.lines().filter(is_arm).map(indent).min() else { return Vec::new() };
    let mut out: Vec<String> = Vec::new();
    for line in body.lines().filter(is_arm).filter(|l| indent(l) == top) {
        let Some((names, _)) = line.trim().split_once("=>") else { continue };
        for n in names.split('|') {
            let n = n.trim().trim_matches('"');
            if !n.is_empty() && n.chars().all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit()) && !out.iter().any(|o| o == n) {
                out.push(n.to_owned());
            }
        }
    }
    out
}

/// Every op `edit` runs: the arms of its dispatch and of `args::op_fields`
/// (which checks the fields), so an op taught only in prose, or not at
/// all, is still found.
fn checked_ops() -> Vec<String> {
    let fields = include_str!("args.rs").split("pub fn op_fields").nth(1).and_then(|b| b.split("\n}\n").next()).unwrap_or_default();
    let module = include_str!("mod.rs");
    let dispatch = module.find("other => Err(format!(\"unknown op \\\"").and_then(|end| module[..end].rfind("match name {").map(|start| &module[start..end])).unwrap_or_default();
    let mut out = arms(fields);
    for n in arms(dispatch) {
        if !out.contains(&n) {
            out.push(n);
        }
    }
    out
}

/// The texts that teach the ops, each with what it is called: the guide
/// (its sections name themselves) and every skill, its procedure and its
/// parts.
fn teaching() -> Vec<(String, &'static str)> {
    let mut t = vec![(String::new(), super::GUIDE)];
    for k in super::skills::all() {
        t.push((format!("skill {}", k.name), k.procedure.as_str()));
        t.extend(k.refs.iter().map(|(title, r)| (format!("skill {}, {title}", k.name), r.as_str())));
    }
    t
}

/// What the guide or a skill says about op `name` where it is not in a
/// table: the bullet or paragraph that opens with it.
fn prose_about(name: &str) -> Option<String> {
    let tag = format!("`{name}`");
    for (_, text) in teaching() {
        for block in text.split("\n\n").flat_map(|p| p.split("\n- ")) {
            let b = block.trim_start_matches("- ").trim();
            if b.starts_with(&tag) || b.starts_with(&format!("**{tag}")) {
                return Some(b.chars().take(1_500).collect());
            }
        }
    }
    None
}

fn index() -> &'static [Entry] {
    static INDEX: OnceLock<Vec<Entry>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut out: Vec<Entry> = Vec::new();
        let mut add = |kind: &'static str, name: String, does: String| {
            let w = words(&format!("{name} {does}"));
            out.push(Entry { kind, key: name.split('\u{1f}').next().unwrap_or_default().to_lowercase(), len: w.len(), words: w.into_iter().collect(), name, does });
        };
        let tools: Vec<String> = super::mcp::tools().iter().filter_map(|t| t["name"].as_str().map(str::to_owned)).collect();
        for t in super::mcp::tools() {
            if let (Some(n), Some(d)) = (t["name"].as_str(), t["description"].as_str()) {
                // The short description is what is shown; the long form is searched too.
                let long = super::guide_topic(&format!("tool:{n}")).unwrap_or_default();
                add("tool", n.to_owned(), format!("{d} {long}"));
            }
        }
        // The op tables of the guide and the skills: | `op` / `op2` | fields | does |,
        // each with the section it sits in.
        let mut rows: Vec<(String, String)> = Vec::new();
        for (label, text) in teaching() {
            let mut open = false;
            let mut section = label;
            for l in text.lines() {
                let t = l.trim();
                if let Some(h) = l.strip_prefix("## ").or_else(|| l.strip_prefix("# ")) {
                    section = h.trim().to_owned();
                }
                if open {
                    let last = &mut rows.last_mut().expect("row").0;
                    last.push(' ');
                    last.push_str(t);
                } else if t.starts_with("| `") {
                    rows.push((t.to_owned(), section.clone()));
                } else {
                    continue;
                }
                open = !(t.ends_with('|') && rows.last().is_some_and(|r| r.0.len() > 2));
            }
        }
        let known = checked_ops();
        let mut ops: Vec<String> = Vec::new();
        for (line, section) in &rows {
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            if cells.len() < 4 || !cells[1].starts_with('`') {
                continue;
            }
            // Names only: a row of clip references (`"music@4.2"`) is not a thing to call.
            let names: Vec<String> = cells[1].split(['/', ',']).map(|n| n.trim().trim_matches('`').to_owned()).filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')).collect();
            let does = format!("{} (fields: {})", cells[3], cells[2]);
            for n in names {
                // A row that is not an op of its own is a kind or field of
                // one (`cloth` is a kind of `simulate`): said with its section.
                if known.is_empty() || known.contains(&n) || tools.contains(&n) {
                    if !ops.contains(&n) {
                        ops.push(n.clone());
                        add("op", n, does.clone());
                    }
                } else {
                    add("row", format!("{n}\u{1f}{section}"), does.clone());
                }
            }
        }
        // Ops taught in prose (or not at all): what is said of them, and their fields.
        for n in known {
            if ops.contains(&n) {
                continue;
            }
            let fields = super::args::op_fields(&n).map(|f| f.join(", ")).unwrap_or_default();
            let does = match prose_about(&n) {
                Some(p) => format!("{p} (fields: {fields})"),
                None => format!("the `{n}` op (fields: {fields})"),
            };
            add("op", n, does);
        }
        for (kind, book) in [("guide", super::GUIDE), ("craft", super::craft_book::BOOK)] {
            for sec in book.split("\n## ").skip(1) {
                let (head, body) = sec.split_once('\n').unwrap_or((sec, ""));
                add(kind, head.trim().to_owned(), body.chars().take(4_000).collect());
            }
        }
        // Skills: the kind of work, and each detail part of it.
        for k in super::skills::all() {
            add("skill", k.name.clone(), format!("{} {} {}", k.use_when, k.tools.join(" "), k.procedure.chars().take(3_000).collect::<String>()));
            for (title, text) in &k.refs {
                add("skill", format!("{} part \"{title}\"", k.name), text.chars().take(4_000).collect());
            }
        }
        out
    })
}

/// Words that mean the same thing to an editor.
const SYNONYMS: &[(&str, &[&str])] = &[
    ("animate", &["animate", "animation", "keys", "keyframe", "motion", "puppet", "rig", "move", "stagger", "physics"]),
    ("character", &["puppet", "rig", "character", "face", "talk", "acting", "mask_puppet", "lip", "act", "pose"]),
    ("scissor", &["scissor", "puppet", "skeleton", "rig", "mouth"]),
    ("walk", &["puppet", "motion", "walk", "rig", "use_move", "act"]),
    ("gait", &["act", "pose", "rig", "walk"]),
    ("planted", &["act", "pose", "inspect"]),
    ("foot", &["act", "pose", "rig", "inspect"]),
    ("hand", &["pose", "act", "rig", "skeleton"]),
    ("reach", &["pose", "act", "ik"]),
    ("gaze", &["pose", "act", "inspect"]),
    ("sit", &["pose", "act"]),
    ("wave", &["puppet", "rig", "keys", "animate", "act"]),
    ("talk", &["talk", "lip", "lip_sync", "face", "narrate", "dialogue", "mouth", "viseme", "phoneme"]),
    ("lip", &["lip", "lip_sync", "talk", "face", "mouth", "viseme", "phoneme"]),
    ("mouth", &["lip_sync", "puppet", "face", "talk", "viseme"]),
    ("viseme", &["lip_sync", "puppet", "pose"]),
    ("phoneme", &["lip_sync", "narrate"]),
    ("voice", &["narrate", "voices", "voice", "talk"]),
    ("colour", &["colour", "grade", "match_colour", "look", "temperature", "saturation", "exposure", "scopes"]),
    ("color", &["colour", "grade", "match_colour", "look", "temperature", "saturation", "exposure", "scopes"]),
    ("grade", &["colour", "grade", "look", "exposure", "contrast"]),
    ("3d", &["three_d", "model", "mesh", "camera", "light", "parallax", "3d"]),
    ("camera", &["camera", "three_d", "parallax", "reframe", "stand", "shot"]),
    ("music", &["music", "song", "beat", "montage", "listen", "sing", "score"]),
    ("sound", &["sfx", "audio", "sound", "music", "listen", "ambience", "sing"]),
    ("score", &["music", "cues", "listen", "scoring"]),
    ("compose", &["music", "scoring", "cues"]),
    ("footstep", &["foley", "sfx", "contacts"]),
    ("foley", &["sfx", "foley", "footsteps", "sound"]),
    ("sing", &["sing", "music", "voices", "narrate"]),
    ("singing", &["sing", "voices", "talk", "lip"]),
    ("animal", &["sfx", "puppet", "rig"]),
    ("noise", &["sfx", "audio", "sound"]),
    ("cut", &["split", "montage", "trim", "cuts", "delete"]),
    ("beat", &["beat", "montage", "analyse", "beats"]),
    ("title", &["text", "title", "typography", "card", "fonts"]),
    ("font", &["fonts", "text", "typography", "title"]),
    ("typeface", &["fonts", "text", "typography"]),
    ("lettering", &["fonts", "text", "vector"]),
    ("text", &["text", "title", "typography", "captions"]),
    ("font", &["text", "typography", "fonts", "vector"]),
    ("typeface", &["text", "typography", "fonts"]),
    ("caption", &["captions", "ai", "text"]),
    ("background", &["cut_out", "roto", "matte", "backdrop", "scene", "background"]),
    ("remove", &["cut_out", "roto", "delete", "voice_cleanup"]),
    ("green", &["cut_out", "keying", "matte"]),
    ("track", &["track", "stabilize", "follow"]),
    ("shake", &["stabilize", "track", "modifier", "noise"]),
    ("slow", &["smooth_motion", "speed", "freeze", "time"]),
    ("generate", &["generate", "music", "picture", "video"]),
    ("image", &["generate", "add", "catalogue", "picture"]),
    ("story", &["scene", "staging", "notes", "story"]),
    ("scene", &["scene", "staging", "backdrop", "place"]),
    ("street", &["build_world", "world", "edit_world", "city", "walk", "stand"]),
    ("city", &["build_world", "world", "edit_world", "city"]),
    ("town", &["build_world", "world", "village", "city"]),
    ("village", &["build_world", "world", "village"]),
    ("building", &["build_world", "edit_world", "buildings", "architecture"]),
    ("house", &["build_world", "edit_world", "buildings"]),
    ("room", &["build_world", "interior", "room"]),
    ("interior", &["build_world", "interior", "room"]),
    ("forest", &["build_world", "world", "scatter", "terrain"]),
    ("landscape", &["build_world", "world", "terrain", "sky"]),
    ("environment", &["build_world", "world", "sky", "terrain"]),
    ("fog", &["fog", "world", "living", "particles", "haze", "mist"]),
    ("mist", &["fog", "living", "mist", "particles"]),
    ("haze", &["fog", "haze", "world", "sky"]),
    ("volumetric", &["fog", "rays", "light", "living", "render3d"]),
    ("rain", &["particles", "living", "rain", "simulate", "weather"]),
    ("snow", &["particles", "living", "snow", "simulate", "weather"]),
    ("weather", &["particles", "living", "weather", "sky", "fog"]),
    ("sky", &["sky", "world", "time_of_day"]),
    ("reflection", &["render3d", "path", "metallic", "roughness", "water"]),
    ("wet", &["render3d", "roughness", "metallic", "water"]),
    ("cloth", &["cloth", "simulate", "fabric", "mesh"]),
    ("clothing", &["cloth", "simulate", "character", "top", "mesh"]),
    ("coat", &["cloth", "character", "top", "mesh"]),
    ("hat", &["mesh", "model", "character"]),
    ("costume", &["cloth", "character", "top", "mesh"]),
    ("export", &["export", "delivery", "displays"]),
    ("vertical", &["reframe", "vertical", "aspect"]),
    ("effect", &["effect", "fx_layer", "apply", "glow", "blur"]),
    ("transition", &["transition", "cuts", "fades"]),
    ("model", &["model", "mesh", "sculpting", "three_d"]),
    ("particle", &["physics", "modifier", "fx_layer", "effect", "particles"]),
    ("physics", &["body", "drop", "bake_physics", "settle", "physics-3d"]),
    ("gravity", &["drop", "physics_world", "body", "physics-3d"]),
    ("fall", &["drop", "settle", "ragdoll", "physics-3d"]),
    ("falls", &["drop", "settle", "ragdoll", "physics-3d"]),
    ("drop", &["drop", "body", "physics-3d"]),
    ("throw", &["throw", "body", "physics-3d"]),
    ("toss", &["throw", "physics-3d"]),
    ("push", &["push", "body", "physics-3d"]),
    ("knock", &["push", "throw", "ragdoll", "physics-3d"]),
    ("topple", &["push", "settle", "physics-3d"]),
    ("collide", &["body", "bake_physics", "physics-3d"]),
    ("collision", &["body", "bake_physics", "physics-3d"]),
    ("bounce", &["body", "drop", "physics-3d"]),
    ("stack", &["body", "drop", "settle", "physics-3d"]),
    ("domino", &["push", "body", "physics-3d"]),
    ("dominoes", &["push", "body", "physics-3d"]),
    ("hinge", &["joint", "physics-3d"]),
    ("joint", &["joint", "physics-3d"]),
    ("pendulum", &["joint", "push", "physics-3d"]),
    ("rope", &["joint", "physics-3d"]),
    ("ragdoll", &["ragdoll", "physics-3d"]),
    ("jiggle", &["jiggle", "physics-3d"]),
    ("tail", &["jiggle", "rig", "physics-3d"]),
    ("follow-through", &["jiggle", "physics-3d"]),
    ("secondary", &["jiggle", "physics-3d"]),
    ("shatter", &["break", "body", "physics-3d"]),
    ("break", &["break", "body", "physics-3d"]),
    ("smash", &["break", "throw", "physics-3d"]),
    ("settle", &["settle", "physics-3d"]),
    ("floating", &["settle", "check", "physics-3d"]),
    ("weight", &["body", "physics-3d"]),
    ("mass", &["body", "physics-3d"]),
    ("solid", &["mesh", "model", "3d-assets-scenes"]),
    ("hollow", &["mesh", "model", "3d-assets-scenes"]),
    ("creature", &["model", "parts", "rig", "3d-assets-scenes"]),
    ("fire", &["simulate", "fire", "particles", "embers"]),
    ("smoke", &["simulate", "smoke", "particles"]),
];

const STOP: &[&str] = &["a", "an", "the", "to", "of", "and", "or", "in", "on", "for", "with", "can", "i", "it", "is", "be", "do", "does", "studio", "how", "make", "my", "this", "that", "some", "into", "from", "want", "way", "like", "need", "use", "using", "without", "not", "no", "any", "all", "exact", "real"];

fn stem(w: &str) -> String {
    let w = w.trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
    for suf in ["ing", "ed", "es", "s"] {
        if w.len() > suf.len() + 3 {
            if let Some(s) = w.strip_suffix(suf) {
                return s.to_owned();
            }
        }
    }
    w.to_owned()
}

/// Whether the entry has a word that `t` names (the word itself, or `t`
/// opening a longer one: `rig` finds `rigged`, not `bright`).
fn has_word(e: &Entry, t: &str) -> bool {
    e.words.contains(t) || t.len() >= 4 && e.words.iter().any(|w| w.starts_with(t))
}

/// What Studio has for `query`, best first: (kind, name, what it does).
///
/// Each word counts by how rare it is across everything Studio has (`fog`
/// says more than `model`), a long guide section counts less per word than
/// a short op line, and a query asking for several things gets an answer
/// for each: once a word is answered, the next pick favours the words not
/// answered yet.
pub fn search(query: &str, n: usize) -> Vec<(&'static str, String, String)> {
    let mut terms: Vec<(String, f32)> = Vec::new();
    for w in query.to_lowercase().split(|c: char| !c.is_alphanumeric() && c != '_') {
        if w.is_empty() || STOP.contains(&w) {
            continue;
        }
        let s = stem(w);
        terms.push((s.clone(), 1.0));
        for (k, syn) in SYNONYMS {
            if s.starts_with(&stem(k)) || stem(k).starts_with(&s) && s.len() >= 3 {
                terms.extend(syn.iter().map(|x| (stem(x), 0.6)));
            }
        }
    }
    terms.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.total_cmp(&a.1)));
    terms.dedup_by(|a, b| a.0 == b.0);
    let ix = index();
    if terms.is_empty() || ix.is_empty() {
        return Vec::new();
    }
    let total = ix.len() as f32;
    let avg = ix.iter().map(|e| e.len).sum::<usize>() as f32 / total;
    let rare: Vec<f32> = terms
        .iter()
        .map(|(t, _)| {
            let df = ix.iter().filter(|e| e.key.contains(t.as_str()) || has_word(e, t)).count() as f32;
            (1.0 + total / (df + 1.0)).ln()
        })
        .collect();
    // Per entry, what each term gives it.
    let gains: Vec<Vec<f32>> = ix
        .iter()
        .map(|e| {
            let short = 1.0 / (1.0 + (e.len as f32 / avg).max(1.0).ln());
            // Ops and tools are what an agent can call; a skill is found by what
            // it is for, and its many detail parts must not crowd out the ops.
            let boost = match e.kind {
                "op" | "tool" => 1.3,
                "skill" => 0.55,
                _ => 1.0,
            };
            terms
                .iter()
                .zip(&rare)
                .map(|((t, w), r)| {
                    let mut g = 0.0;
                    if e.key == *t || stem(&e.key) == *t {
                        g += 4.0;
                    } else if e.key.contains(t.as_str()) {
                        g += 2.0;
                    }
                    if has_word(e, t) {
                        g += short;
                    }
                    g * w * r * boost
                })
                .collect()
        })
        .collect();
    // A term counts as answered by a pick that gives it at least half of
    // the most any entry gives it, not by a passing mention.
    let top: Vec<f32> = (0..terms.len()).map(|t| gains.iter().map(|g| g[t]).fold(0.0, f32::max)).collect();
    let mut answered = vec![0u32; terms.len()];
    let mut picked: Vec<usize> = Vec::new();
    while picked.len() < n {
        let best = (0..ix.len())
            .filter(|i| !picked.contains(i))
            .map(|i| (i, gains[i].iter().zip(&answered).map(|(g, a)| g * 0.5f32.powi(*a as i32)).sum::<f32>()))
            .filter(|(_, s)| *s > 0.0)
            .max_by(|a, b| a.1.total_cmp(&b.1));
        let Some((i, _)) = best else { break };
        for (t, g) in gains[i].iter().enumerate() {
            if *g > 0.0 && *g >= 0.5 * top[t] {
                answered[t] += 1;
            }
        }
        picked.push(i);
    }
    picked.into_iter().map(|i| (ix[i].kind, ix[i].name.clone(), super::brain::one_line(&ix[i].does, 220))).collect()
}

/// The `capabilities` tool.
pub fn tool(a: &Value) -> Result<String, String> {
    let q = a.get("query").or_else(|| a.get("what")).and_then(Value::as_str).unwrap_or("").trim();
    if q.is_empty() {
        let ops = index().iter().filter(|e| e.kind == "op").map(|e| e.name.as_str()).collect::<Vec<_>>().join(", ");
        let tools = index().iter().filter(|e| e.kind == "tool").map(|e| e.name.as_str()).collect::<Vec<_>>().join(", ");
        return Ok(format!("Say what you want to do in `query` (\"animate a character waving\"). Studio's tools: {tools}.\n`edit` ops: {ops}."));
    }
    let found = search(q, 12);
    if found.is_empty() {
        return Ok(format!("Nothing matched \"{q}\" by name. Studio is broad: try other words (what it looks or sounds like, the craft term), or read `guide` with topic `all`. Do not tell the person it cannot be done on this alone."));
    }
    let mut out = vec![format!("For \"{q}\", Studio has:")];
    for (kind, name, does) in &found {
        let how = match *kind {
            "op" => format!("`edit` op `{name}`"),
            "tool" => format!("tool `{name}`"),
            "guide" => format!("guide section \"{name}\""),
            "skill" => format!("skill (`skill {{name}}`, a part with `part`) \"{name}\""),
            "row" => {
                let (n, section) = name.split_once('\u{1f}').unwrap_or((name.as_str(), ""));
                format!("`{n}` (section \"{section}\")")
            }
            _ => format!("craft book \"{name}\""),
        };
        out.push(format!("- {how}: {does}"));
    }
    out.push("Use the ops and tools above; read how with `skill {name}` (the procedure for a kind of work) or `guide` (topic word). Only if none of this fits, try other words before saying Studio cannot do it.".into());
    Ok(out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animation_is_found_where_it_lives() {
        let names: Vec<String> = search("animate the characters so they walk and wave", 12).into_iter().map(|f| f.1).collect();
        assert!(names.iter().any(|n| n == "puppet"), "{names:?}");
        assert!(names.iter().any(|n| n == "animate" || n == "keys"), "{names:?}");
        let colour: Vec<String> = search("fix the colour cast", 8).into_iter().map(|f| f.1).collect();
        assert!(colour.iter().any(|n| n == "match_colour" || n == "set" || n.contains("Colour")), "{colour:?}");
        let t = tool(&serde_json::json!({"query": "remove the background behind the presenter"})).unwrap();
        assert!(t.contains("cut_out"), "{t}");
        assert!(tool(&serde_json::json!({})).unwrap().contains("edit` ops"));
    }

    #[test]
    fn long_requests_find_each_thing_they_ask_for() {
        let names = |q: &str, n: usize| search(q, n).into_iter().map(|f| f.1).collect::<Vec<String>>();
        // Asked in the words a film-maker uses, with several wants in one line.
        let street = names("real 3D rigged character walk with planted feet; procedural masonry street world reflections", 12);
        assert!(street.iter().any(|n| n == "build_world"), "a street is a built world: {street:?}");
        assert!(street.iter().any(|n| n == "act" || n == "pose"), "the character's motion: {street:?}");
        let town = names("a small town with houses at dusk", 6);
        assert!(town.iter().any(|n| n == "build_world"), "{town:?}");
        let fog = names("volumetric light fog", 8);
        assert!(fog.iter().any(|n| n == "living" || n == "build_world" || n == "particles" || n.contains("Living")), "{fog:?}");
        let type_ = names("list the fonts I can use for a title", 8);
        assert!(type_.iter().any(|n| n == "text" || n.contains("ypography")), "{type_:?}");
    }

    #[test]
    fn ops_documented_in_prose_are_indexed_with_their_fields() {
        let ix = index();
        assert!(!ix.iter().any(|e| e.kind == "op" && e.name.contains('@')), "clip references are not ops");
        let cloth = ix.iter().find(|e| e.key == "cloth").expect("the cloth row");
        assert_eq!(cloth.kind, "row", "cloth is a kind of simulate, not an op");
        assert!(tool(&serde_json::json!({"query": "cloth"})).unwrap().contains("`cloth` (section \""));
        let w = ix.iter().find(|e| e.kind == "op" && e.name == "build_world").expect("build_world is an op");
        assert!(w.does.contains("description") && w.does.contains("fields:"), "{}", w.does);
        let ops = checked_ops();
        assert!(ops.len() > 80, "every op the edit tool runs: {}", ops.len());
        assert!(ops.iter().any(|o| o == "text_3d") && ops.iter().any(|o| o == "split"), "ops without field checks are ops too");
        assert!(!ops.iter().any(|o| o == "linear" || o == "dual_quaternion"), "values inside an op are not ops: {ops:?}");
    }

    #[test]
    fn every_op_and_tool_is_indexed() {
        let ix = index();
        for op in ["place", "puppet", "three_d", "match_colour", "sfx"] {
            assert!(ix.iter().any(|e| e.kind == "op" && e.name == op), "{op}");
        }
        assert!(ix.iter().any(|e| e.kind == "tool" && e.name == "capabilities"), "it lists itself");
        assert!(ix.iter().any(|e| e.kind == "craft"));
    }
}
