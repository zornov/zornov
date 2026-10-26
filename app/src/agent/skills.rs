// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Skills: how Studio does each kind of work, loaded when the AI starts it.
//!
//! A skill is a markdown file in `skills/` (compiled in). Its front matter
//! gives the **name**, a one-line **use** ("use when ..."), the **tools**
//! (edit ops and tools) it teaches and the **topics** (old `guide` words)
//! that open it. Its body is the procedure: which tools in what order,
//! the checks, and the common mistakes with their fixes. Below the
//! procedure, `## Reference: <title>` sections hold the deeper detail
//! (field tables, chapters of the craft book); `skill {name, part}` reads
//! one of them.
//!
//! Only the one-line index rides in the prompt ([`index_block`]); the body
//! loads through the `skill` tool. Lessons the AI learned (see
//! [`super::lessons`]) are appended to a skill when it loads.

use super::lessons::{self, Kind, Store};
use serde_json::{Value, json};
use std::sync::OnceLock;

/// Every skill file, compiled in.
const FILES: &[&str] = &[
    include_str!("skills/footage-edit.md"),
    include_str!("skills/story-film.md"),
    include_str!("skills/production-office.md"),
    include_str!("skills/puppetry-rigging.md"),
    include_str!("skills/motion-animation.md"),
    include_str!("skills/3d-assets-scenes.md"),
    include_str!("skills/lighting-mood.md"),
    include_str!("skills/physics-3d.md"),
    include_str!("skills/walkable-worlds.md"),
    include_str!("skills/living-places.md"),
    include_str!("skills/sound-design.md"),
    include_str!("skills/music-beat-sync.md"),
    include_str!("skills/colour-grading.md"),
    include_str!("skills/titles-typography.md"),
    include_str!("skills/canvas-layout.md"),
    include_str!("skills/vector-drawing.md"),
    include_str!("skills/green-screen-mattes.md"),
    include_str!("skills/vfx-compositing.md"),
    include_str!("skills/pictures-retouch.md"),
    include_str!("skills/picture-batches.md"),
    include_str!("skills/generated-media.md"),
    include_str!("skills/capture-live.md"),
    include_str!("skills/review-before-export.md"),
    include_str!("skills/export-delivery.md"),
    include_str!("skills/library-project-care.md"),
    include_str!("skills/project-team.md"),
    include_str!("skills/plugins-automation.md"),
    include_str!("skills/fix-failed-call.md"),
    include_str!("skills/objects-in-footage.md"),
];

/// One skill.
pub struct Skill {
    pub name: String,
    /// The one line that says when to use it.
    pub use_when: String,
    /// Old `guide` words that open it.
    pub topics: Vec<String>,
    /// The edit ops and tools it teaches (as the capability map writes
    /// them: ops bare, tools bare too here).
    pub tools: Vec<String>,
    /// The procedure, checks and mistakes.
    pub procedure: String,
    /// Deeper detail by title.
    pub refs: Vec<(String, String)>,
}

fn list(v: &str) -> Vec<String> {
    v.split(',').map(|x| x.trim().to_lowercase()).filter(|x| !x.is_empty()).collect()
}

fn parse(src: &str) -> Skill {
    let rest = src.strip_prefix("---\n").expect("a skill starts with front matter");
    let (head, body) = rest.split_once("\n---\n").expect("front matter ends with ---");
    let mut s = Skill { name: String::new(), use_when: String::new(), topics: Vec::new(), tools: Vec::new(), procedure: String::new(), refs: Vec::new() };
    for l in head.lines() {
        let Some((k, v)) = l.split_once(':') else { continue };
        match k.trim() {
            "name" => s.name = v.trim().to_owned(),
            "use" => s.use_when = v.trim().to_owned(),
            "topics" => s.topics = list(v),
            "tools" => s.tools = list(v),
            _ => {}
        }
    }
    let mut parts = body.trim().split("\n## Reference: ");
    s.procedure = parts.next().unwrap_or("").trim().to_owned();
    for p in parts {
        let (title, text) = p.split_once('\n').unwrap_or((p, ""));
        s.refs.push((title.trim().to_owned(), text.trim().to_owned()));
    }
    s
}

/// All the skills.
pub fn all() -> &'static [Skill] {
    static ALL: OnceLock<Vec<Skill>> = OnceLock::new();
    ALL.get_or_init(|| FILES.iter().map(|f| parse(f)).collect())
}

fn norm(s: &str) -> String {
    s.trim().to_lowercase().replace([' ', '_'], "-")
}

/// A skill by its name, or by one of its topic words.
pub fn find(word: &str) -> Option<&'static Skill> {
    let w = norm(word);
    all().iter().find(|s| s.name == w).or_else(|| all().iter().find(|s| s.topics.iter().any(|t| norm(t) == w)))
}

/// The names, for error messages.
pub fn names() -> String {
    all().iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")
}

/// The one-line index: `- name: use when ...`, a line a skill.
pub fn index() -> String {
    all().iter().map(|s| format!("- {}: {}\n", s.name, s.use_when)).collect()
}

/// The index as the prompt carries it.
pub fn index_block() -> String {
    format!("**Skills** (load one with `skill {{name}}`):\n{}", index())
}

/// The first answer of `guide`: the capability map, the core and the skill index.
pub fn start_text() -> String {
    format!("{}\n\n{}\n\n{}", super::map::capability_map(), super::GUIDE_CORE, index_block())
}

/// The skill for a `guide` topic word, as `guide` serves it: a topic that
/// moved into a skill still answers.
pub fn for_topic(word: &str, part: Option<&str>, full: bool) -> Option<Result<String, String>> {
    let s = find(word)?;
    let w = norm(word);
    // A word that names one reference section opens just that.
    let named = s.refs.iter().find(|(t, _)| norm(t) == w).map(|(t, _)| t.clone());
    let part = part.map(str::to_owned).or(named);
    Some(load(s, part.as_deref(), full, &Store::user(), lessons::now()))
}

/// A skill's procedure for a crew member's brief, once per skill.
pub fn brief_chapter(word: &str, taught: &mut Vec<String>) -> Option<String> {
    let s = find(word)?;
    if taught.contains(&s.name) {
        return None;
    }
    taught.push(s.name.clone());
    Some(format!("# Skill: {}\n{}", s.name, s.procedure))
}

/// A reference section by a word or two of its title.
fn find_ref<'a>(s: &'a Skill, q: &str) -> Option<&'a (String, String)> {
    let q = q.trim().to_lowercase();
    s.refs.iter().find(|(t, _)| t.to_lowercase() == q).or_else(|| s.refs.iter().find(|(t, _)| t.to_lowercase().starts_with(&q))).or_else(|| s.refs.iter().find(|(t, _)| t.to_lowercase().contains(&q)))
}

/// A tool's long form, if a skill carries it: `## Reference: tool `name``.
pub fn tool_long_form(name: &str) -> Option<&'static str> {
    let title = format!("tool `{name}`");
    all().iter().flat_map(|s| s.refs.iter()).find(|(t, _)| *t == title).map(|(_, text)| text.as_str())
}

/// A skill, or one part of it, with its lessons.
pub fn load(s: &Skill, part: Option<&str>, full: bool, store: &Store, now: u64) -> Result<String, String> {
    if let Some(p) = part {
        let parts: Vec<&str> = p.split(';').map(str::trim).filter(|x| !x.is_empty()).collect();
        let mut out = Vec::new();
        for q in parts {
            let (t, text) = find_ref(s, q).ok_or_else(|| format!("skill {} has no part \"{q}\"; parts: {}", s.name, s.refs.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>().join(" | ")))?;
            out.push(format!("## {t}\n\n{text}"));
        }
        return Ok(format!("# Skill: {}, detail\n\n{}", s.name, out.join("\n\n")));
    }
    let mut text = format!("# Skill: {}\nUse when: {}\n\n{}", s.name, s.use_when, s.procedure);
    if full {
        for (t, body) in &s.refs {
            text.push_str(&format!("\n\n## {t}\n\n{body}"));
        }
    } else if !s.refs.is_empty() {
        let outline: Vec<String> = s.refs.iter().map(|(t, b)| format!("{t} ({})", b.len())).collect();
        text.push_str(&format!("\n\nMore detail (`skill {{name: \"{}\", part: title}}`, parts split with `;`, or `full: true`): {}.", s.name, outline.join(" | ")));
    }
    // Lessons are saved text read back into every request: cleaned again.
    text.push_str(&super::outside::clean_saved(&store.section(&s.name, now)));
    Ok(text)
}

fn str_arg<'a>(a: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| a.get(*k).and_then(Value::as_str)).map(str::trim).filter(|s| !s.is_empty())
}

/// What the `skill` tool answers.
pub fn tool(a: &Value) -> Result<String, String> {
    answer(a, &Store::user(), lessons::now())
}

/// The tool with the lessons store and the time given (tests pass their own).
pub fn answer(a: &Value, store: &Store, now: u64) -> Result<String, String> {
    let verb = str_arg(a, &["do"]).map(str::to_lowercase);
    match verb.as_deref() {
        None | Some("load") | Some("read") | Some("show") => {
            let Some(name) = str_arg(a, &["name", "skill", "topic"]) else {
                return Ok(format!("Skills (load one with `skill {{name}}`):\n{}Lessons: `skill {{do: \"lessons\"}}`; save one: `skill {{do: \"learn\", skill, lesson, kind}}`.", index()));
            };
            let s = find(name).ok_or_else(|| format!("no skill \"{name}\"; skills: {}", names()))?;
            let part = str_arg(a, &["part"]);
            let full = a.get("full").and_then(Value::as_bool).unwrap_or(false) || part.is_some_and(|p| p.eq_ignore_ascii_case("all"));
            load(s, part.filter(|_| !full), full, store, now)
        }
        Some("list") | Some("index") => Ok(index()),
        Some("learn") | Some("save") | Some("lesson") => {
            let skill = str_arg(a, &["skill", "name"]).unwrap_or(lessons::GENERAL);
            let skill = skill.to_lowercase();
            let skill = if skill == lessons::GENERAL { skill } else { find(&skill).map(|s| s.name.clone()).ok_or_else(|| format!("no skill \"{skill}\" to tag the lesson to; skills: {}, or \"general\"", names()))? };
            let text = str_arg(a, &["lesson", "text"]).ok_or("learn needs `lesson`: what worked, or what failed and the fix, in a sentence or two")?;
            let kind = str_arg(a, &["kind"]).map_or(Kind::Worked, Kind::parse);
            super::outside::check_saved(text)?;
            let r = store.learn(&skill, text, kind, now)?;
            Ok(if r.confirmed { format!("Already known: lesson {} for {skill} is confirmed again (seen {} times).", r.id, r.seen) } else { format!("Saved lesson {} for {skill}. It rides along the next time that skill loads.", r.id) })
        }
        Some("confirm") => {
            let id = a.get("id").and_then(Value::as_u64).ok_or("confirm needs the lesson `id`")?;
            let seen = store.confirm(id, now)?;
            Ok(format!("Lesson {id} confirmed (seen {seen} times)."))
        }
        Some("lessons") => {
            let only = str_arg(a, &["skill", "name"]).map(str::to_lowercase);
            let rows: Vec<Value> = store.all(now).into_iter().filter(|l| only.as_ref().is_none_or(|s| &l.skill == s)).map(|l| json!({"id": l.id, "skill": l.skill, "kind": l.kind.name(), "seen": l.seen, "lesson": l.text})).collect();
            Ok(json!({"lessons": rows}).to_string())
        }
        Some("forget") => {
            let id = a.get("id").and_then(Value::as_u64).ok_or("forget needs the lesson `id`")?;
            store.forget(id)?;
            Ok(format!("Lesson {id} deleted."))
        }
        Some(other) => Err(format!("skill do \"{other}\": load (default, with name), list, learn, confirm, lessons, forget")),
    }
}

/// The tool's definition, listed with the others. Kept tiny: it rides in
/// every request, and `skill` with no arguments lists the skills.
pub fn tools() -> Vec<Value> {
    vec![json!({
        "name": "skill",
        "description": "Procedure by `name` (+`part`). `do`: learn {name, lesson, kind}, confirm/forget {id}.",
        "inputSchema": {"type": "object", "properties": {
            "name": {"type": "string"}, "part": {"type": "string"}, "do": {"type": "string"},
            "lesson": {"type": "string"}, "id": {"type": "integer"}
        }}
    })]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> Store {
        let d = std::env::temp_dir().join(format!("studio-skills-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        Store::at(&d)
    }

    #[test]
    fn every_skill_file_is_well_formed() {
        let mut seen = std::collections::BTreeSet::new();
        for s in all() {
            assert!(seen.insert(s.name.clone()), "{} twice", s.name);
            assert!(s.name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'), "{}", s.name);
            assert!(s.use_when.len() >= 30 && s.use_when.len() <= 150, "{}: the use line is {} characters", s.name, s.use_when.len());
            assert!(s.use_when.to_lowercase().starts_with("use when") || s.use_when.to_lowercase().starts_with("when"), "{}: the use line says when: {}", s.name, s.use_when);
            assert!(!s.tools.is_empty(), "{} names no tools", s.name);
            assert!(s.procedure.len() > 600, "{} has no procedure", s.name);
            for must in ["Order", "Checks", "Mistakes"] {
                assert!(s.procedure.contains(&format!("**{must}")), "{} lacks its {must} part", s.name);
            }
            assert!(s.procedure.len() <= 7_000, "{}: the procedure is {} characters; the detail goes in a Reference part", s.name, s.procedure.len());
        }
        assert!(all().len() >= 20);
    }

    #[test]
    fn the_prompt_index_stays_under_its_budget() {
        let block = index_block();
        assert!(block.len() <= 3_000, "the skill index in the prompt is {} characters (~{} tokens)", block.len(), block.len() / 4);
        for s in all() {
            assert!(block.contains(&format!("- {}: ", s.name)));
            assert!(!block.contains(&s.procedure), "only the one line rides in the prompt");
        }
    }

    #[test]
    fn a_skill_loads_by_name_by_topic_and_by_part() {
        let store = scratch("load");
        let a = answer(&json!({"name": "story-film"}), &store, 1).unwrap();
        assert!(a.starts_with("# Skill: story-film") && a.contains("More detail"));
        let by_topic = answer(&json!({"name": "story"}), &store, 1);
        assert!(by_topic.is_ok(), "{by_topic:?}");
        let s = find("story-film").unwrap();
        let (title, text) = s.refs.first().expect("story-film has detail parts");
        let part = answer(&json!({"name": "story-film", "part": title}), &store, 1).unwrap();
        assert!(part.contains(text.lines().next().unwrap_or("")));
        assert!(answer(&json!({"name": "story-film", "part": "no such part"}), &store, 1).unwrap_err().contains("parts:"));
        assert!(answer(&json!({"name": "nonsense"}), &store, 1).unwrap_err().contains("skills:"));
        let full = answer(&json!({"name": "story-film", "full": true}), &store, 1).unwrap();
        assert!(full.len() > a.len());
        assert!(answer(&json!({}), &store, 1).unwrap().contains("story-film"));
    }

    #[test]
    fn learning_through_the_tool_saves_and_loads_a_lesson() {
        let store = scratch("tool");
        let r = answer(&json!({"do": "learn", "skill": "sound-design", "lesson": "A bed 20 dB under the voice never fights it.", "kind": "worked"}), &store, 100).unwrap();
        assert!(r.starts_with("Saved lesson 1"), "{r}");
        let again = answer(&json!({"do": "learn", "skill": "sound-design", "lesson": "A bed 20 dB under the voice never fights it.", "kind": "worked"}), &store, 101).unwrap();
        assert!(again.contains("confirmed again") && again.contains("2 times"), "{again}");
        let loaded = answer(&json!({"name": "sound-design"}), &store, 102).unwrap();
        assert!(loaded.contains("Lessons from earlier jobs") && loaded.contains("never fights it") && loaded.contains("seen 2 times"));
        let other = answer(&json!({"name": "story-film"}), &store, 102).unwrap();
        assert!(!other.contains("never fights it"));
        assert!(answer(&json!({"do": "learn", "skill": "nope", "lesson": "A lesson for a skill that does not exist."}), &store, 1).is_err());
        assert!(answer(&json!({"do": "learn", "skill": "sound-design"}), &store, 1).is_err());
        let general = answer(&json!({"do": "learn", "lesson": "Say what you saw apart from what you set.", "kind": "correction"}), &store, 103).unwrap();
        assert!(general.contains("general"));
        assert!(answer(&json!({"name": "colour-grading"}), &store, 104).unwrap().contains("apart from what you set"));
        let listed = answer(&json!({"do": "lessons"}), &store, 105).unwrap();
        assert!(listed.contains("\"id\":1") && listed.contains("correction"));
        assert!(answer(&json!({"do": "confirm", "id": 1}), &store, 106).unwrap().contains("3 times"));
        assert!(answer(&json!({"do": "forget", "id": 1}), &store, 107).is_ok());
        assert!(!answer(&json!({"do": "lessons"}), &store, 108).unwrap().contains("never fights"));
    }

    /// Every edit op and tool a skill names must exist.
    #[test]
    fn every_tool_a_skill_names_exists() {
        let ops = super::super::map::all_ops();
        let tools = super::super::map::all_tools();
        let known = |n: &str| ops.contains(n) || tools.iter().any(|t| t == n);
        let mut bad: Vec<String> = Vec::new();
        for s in all() {
            for t in &s.tools {
                if !known(t) {
                    bad.push(format!("{}: tools list \"{t}\"", s.name));
                }
            }
            // Calls written in the text: `name {` in backticks.
            let text = format!("{}\n{}", s.procedure, s.refs.iter().map(|(_, b)| b.as_str()).collect::<Vec<_>>().join("\n"));
            for span in text.split('`').skip(1).step_by(2) {
                let first = span.split([' ', '{']).next().unwrap_or("");
                let called = span.trim_start().starts_with(first) && span[first.len()..].trim_start().starts_with('{');
                if called && !first.is_empty() && first.chars().all(|c| c.is_ascii_lowercase() || c == '_') && !known(first) && !CALL_WORDS.contains(&first) {
                    bad.push(format!("{}: `{first} {{…}}`", s.name));
                }
            }
        }
        bad.sort();
        bad.dedup();
        assert!(bad.is_empty(), "skills call things that are neither tools, ops nor known sub-actions: {bad:?}");
    }

    /// Words written like a call that are fields or actions of a tool (`editorial {match_frame}`,
    /// `project_care {backup}`), not tools of their own.
    const CALL_WORDS: &[&str] = &[
        "name", "do", "topic", "op", "ops", "part", "skill", "lane", "from", "to", "frame", "clip", "item", "query", "kind", "mix", "title", "steps", "take", "takes", "sides", "points", "speeds", "lines", "region", "ask", "action", "scene", "shot", "look", "cast", "places", "beats", "scenes", "shots", "remove", "edits", "ranges", "mode", "step", "refs", "items", "panel", "doc", "id",
        "consolidate", "use_proxies", "index", "match_frame", "reverse_match", "backup", "backups", "collect", "expiring", "find", "licence", "name_person", "relocate", "restore", "restore_backup", "snapshot", "verify",
        "actual", "availability", "budget", "call_sheet", "pipeline", "rates", "spend", "status", "task", "version", "automate", "beeps", "burn_in", "make", "meters", "sheet", "speakers", "translate", "voice",
    ];

    /// Every department line of the capability map is taught by some skill.
    #[test]
    fn every_capability_in_the_map_is_reachable_from_a_skill() {
        let taught: std::collections::BTreeSet<&str> = all().iter().flat_map(|s| s.tools.iter().map(String::as_str)).collect();
        let lines = super::super::map::lines();
        assert!(lines.len() > 40);
        for (dept, what, names) in lines {
            let mut ok = false;
            for n in names.split_whitespace() {
                ok |= taught.contains(n.trim_start_matches("tool:"));
            }
            assert!(ok, "{dept}: \"{what}\" ({names}) is taught by no skill; add one of its names to a skill's tools");
        }
    }

    /// No tool or op of Studio is missing from every skill and the map's
    /// own text alike: every tool is taught by a skill or is a sense.
    #[test]
    fn most_tools_are_taught_somewhere() {
        let taught: std::collections::BTreeSet<&str> = all().iter().flat_map(|s| s.tools.iter().map(String::as_str)).collect();
        let tools = super::super::map::all_tools();
        let missing: Vec<&String> = tools.iter().filter(|t| !taught.contains(t.as_str())).collect();
        assert!(missing.len() * 4 <= tools.len(), "{} of {} tools are taught by no skill: {missing:?}", missing.len(), tools.len());
    }

    /// What moved into a skill is no longer kept in the guide, the craft book or the tool reference.
    #[test]
    fn moved_guidance_has_one_copy() {
        for s in all() {
            for (title, text) in &s.refs {
                let probe: String = text.split_whitespace().take(14).collect::<Vec<_>>().join(" ");
                if let Some(tool) = title.strip_prefix("tool `").and_then(|t| t.strip_suffix('`')) {
                    assert!(!super::super::TOOL_REFERENCE.contains(&format!("## `{tool}`\n")), "tool {tool} is still in tool_reference.md");
                    continue;
                }
                assert!(!super::super::GUIDE.contains(&format!("## {title}\n")), "\"{title}\" is still a section of guide.md");
                assert!(!super::super::craft_book::BOOK.contains(&format!("## {title}\n")), "\"{title}\" is still a chapter of the craft book");
                // The opening words of its text appear in no other book either.
                let flat = |t: &str| t.split_whitespace().collect::<Vec<_>>().join(" ");
                if probe.len() > 40 {
                    assert!(!flat(super::super::GUIDE).contains(&probe), "the text of \"{title}\" is still in guide.md");
                    assert!(!flat(super::super::craft_book::BOOK).contains(&probe), "the text of \"{title}\" is still in the craft book");
                }
            }
        }
    }

    /// Skills teach Studio's own tools; they name no other product.
    #[test]
    fn skills_name_no_other_editor() {
        let banned = ["after effects", "premiere", "final cut", "photoshop", "illustrator", "blender", "nuke", "moho", "character animator", "lottiefiles", "resolve and"];
        for s in all() {
            let text = format!("{} {} {}", s.use_when, s.procedure, s.refs.iter().map(|(t, b)| format!("{t} {b}")).collect::<Vec<_>>().join(" ")).split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
            for b in banned {
                assert!(!text.contains(b), "skill {} names \"{b}\"", s.name);
            }
        }
    }

    /// Every topic word `guide` knew before the how-to moved into skills still answers.
    #[test]
    fn the_old_guide_topics_still_open_something() {
        let words = [
            "library", "assets", "safety", "backup", "rights", "capture", "record", "live", "stream", "teleprompter", "editing", "editorial", "cutting", "trim", "takes", "footage", "sounds", "catalogue", "analyse", "mix", "mixing", "loudness", "stems", "adr", "subtitles", "dub", "delivery", "qc", "queue", "bundle", "cutdown", "recipes", "moves", "creative", "team", "comments", "versions", "share", "production", "breakdown", "schedule", "budget", "plugins", "macros", "automation", "mograph", "shapes", "templates", "charts", "expressions", "morph", "vector", "svg", "logo", "icons", "models", "mesh", "post", "colour", "grading", "aces", "hdr", "conform", "proxies", "story", "script", "shots", "foley", "vfx", "compositing", "pictures", "paint", "retouch", "living", "weather", "particles",
            "sound", "typography", "puppetry", "sculpting", "walkable", "overdoing", "mistakes", "review", "planning", "staging", "keying", "tracking", "cinematography", "rigging", "3d", "worlds", "generated", "generative", "restyle", "time", "speed",
        ];
        let mut lost = Vec::new();
        for w in words {
            if super::super::guide_answer(&json!({"topic": w})).is_err() {
                lost.push(w);
            }
        }
        assert!(lost.is_empty(), "guide topics that no longer open anything: {lost:?}");
    }

    /// What the skills cost the prompt: the core plus the index is no larger than the core alone
    /// was before skills (5,967 bytes, the 150 of the no-cost-talk line since, and the 140 of the
    /// lighting-mood line), so the pointers to the procedures are paid for by the procedures
    /// leaving the core.
    #[test]
    fn the_prompt_is_not_larger_than_it_was() {
        let prompt = super::super::brain::system_prompt();
        let mine = super::super::GUIDE_CORE.len() + index_block().len();
        eprintln!("system prompt: {} chars (~{} tokens); first guide read: {}; core + index: {mine}; skill index: {}", prompt.len(), prompt.len() / 4, start_text().len(), index_block().len());
        assert!(mine <= 5_967 + 150 + 120 + 140, "the core and the skill index are {mine} bytes");
        assert!(prompt.contains(&index_block()) && prompt.contains("skill {name}"));
    }

    #[test]
    fn a_guide_topic_that_moved_still_answers() {
        for word in ["story", "mograph", "vector", "enterable", "puppetry"] {
            assert!(for_topic(word, None, false).is_some_and(|r| r.is_ok()), "guide topic {word}");
        }
        assert!(tool_long_form("story").is_some() || tool_long_form("picture").is_some());
    }
}
