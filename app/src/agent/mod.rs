// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Studio for AI agents: every editing capability as a tool.
//!
//! A [`Session`] owns one open document and answers tool calls with compact
//! JSON (and JPEG pictures for `frame` / `contact_sheet`). It runs on the
//! same services the interface uses, so an agent can do anything a person
//! can, headless (`zornov mcp`) or live in the running app.
//!
//! Design rules for agents:
//! * times are seconds, clips are ids, library items are names (matched
//!   loosely, with suggestions on a miss);
//! * `edit` takes a list of ops applied as one undo step, all or nothing;
//! * answers are small: `summary` is one line per clip, `edit` returns only
//!   what changed and the new revision;
//! * the agent can look at its work (`frame`, `contact_sheet`).

mod analyse;
mod guard;
pub(crate) mod outside;
pub mod picture_batch;
pub mod care;
mod library_index;
pub(crate) mod args;
pub mod cli;
mod craft;
pub(crate) use craft::curve;
mod listen;
mod hear;
mod made;
mod automation;
mod plugins;
pub use plugins::run_on;
mod matcher;
mod more;
mod production;
pub use production::story_of;
pub use story::script_context;
mod props;
mod repeat;
mod stagger;
mod drivers;
mod graph;
mod modifiers;
mod mograph;
mod templates;
pub(crate) use templates::{TextSpot, text_spots};
pub(crate) mod physics;
mod bodies;
#[cfg(feature = "render")]
mod scopes;
#[cfg(feature = "render")]
mod viewport;
mod layers;
mod pins;
mod morph;
mod bones;
mod fonts;
mod polymask;
mod keybg;
mod scissor;
mod space;
pub(crate) mod cine;
mod shots;
mod air;
mod look_check;
mod lighting;
pub mod guides;
mod models;
mod mesh_edit;
mod anim3d;
mod wardrobe;
mod perform;
mod schema;
mod sim3d;
#[cfg(feature = "render")]
mod world3d;
#[cfg(feature = "render")]
mod worlds;
#[cfg(feature = "render")]
mod track;
#[cfg(feature = "render")]
mod cutout;
#[cfg(feature = "render")]
mod objects;
#[cfg(feature = "render")]
mod vfx;
#[cfg(feature = "render")]
mod colour_match;
#[cfg(feature = "render")]
mod post;
#[cfg(feature = "render")]
mod slowmo;
#[cfg(feature = "render")]
mod sit;
mod sound_ops;
mod motion_check;
mod craft_check;
mod three_d_check;
mod asset3d;
mod places3d;
mod image3d;
mod model3d;
pub(crate) mod craft_book;
mod scene;
mod voice;
mod capture;
pub(crate) use capture::{ui_live_frame, ui_prompter_frame, ui_start_follow, ui_stop_follow};
mod stage;
mod layout;
pub(crate) mod shell;
mod term;
mod script;
mod select;
pub(crate) mod editorial;
#[cfg(feature = "render")]
pub(crate) mod sight;
#[cfg(feature = "render")]
mod living;
mod sounds;
mod synth;
mod local_sound;
mod foley;
mod story;
mod sound_dept;
mod quality;
mod review;
pub mod crew;
pub mod taste;
pub mod lessons;
pub mod skills;
mod capabilities;
pub mod map;
mod inventory;
mod variety;
mod shelf;
mod keep;
pub mod eval;
pub mod trace;
pub mod trace_report;
pub mod replay;
#[cfg(feature = "agent-dev")]
pub mod observe;
#[cfg(feature = "agent-dev")]
pub mod dev;
mod hot;
mod local_only;
pub mod inproc;
mod mcp_extras;
mod readback;
mod grounding;
pub mod mcp;
pub mod live;
pub mod brain;
pub mod openai;
pub mod attach;
pub mod keychain;
#[cfg(feature = "commerce")]
pub(crate) mod account_tools;
pub mod voice_service;
pub mod download;
mod search_media;
pub mod web;
pub mod jobs;
pub mod procs;
#[cfg(all(test, unix))]
mod crew_stop;
pub mod generate;
#[cfg(feature = "render")]
pub(crate) mod generative;
#[cfg(feature = "render")]
mod lottie;
mod psd;
mod picture;
mod vector;
mod reframe;
mod render;
mod budget;
pub mod usage;
mod parts;
mod compact;
mod toolindex;
mod collab_tools;
#[cfg(feature = "render")]
mod deliver_tools;
#[cfg(feature = "render")]
pub(crate) use deliver_tools::bundle_for_job;
#[cfg(test)]
mod fix_tests;
#[cfg(all(test, not(feature = "commerce")))]
mod commerce_off;

use crate::doc::Doc;
use crate::services::{self, library::Panel};
use crate::view::{self, ClipKind, MediaKind, SEC};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use studio_model as m;

/// The plain-language manual agents read first.
#[cfg(not(feature = "commerce"))]
pub const GUIDE: &str = include_str!("guide.md");
/// With the `commerce` feature the guide ends with the account section.
#[cfg(feature = "commerce")]
pub const GUIDE: &str = concat!(include_str!("guide.md"), "\n", include_str!("guide_commerce.md"));
/// The long form of each tool's description, by `tool:<name>`.
#[cfg(not(feature = "commerce"))]
const TOOL_REFERENCE: &str = include_str!("tool_reference.md");
#[cfg(feature = "commerce")]
const TOOL_REFERENCE: &str = concat!(include_str!("tool_reference.md"), "\n", include_str!("tool_reference_commerce.md"));
/// What every session needs, short; the rest by topic.
pub const GUIDE_CORE: &str = include_str!("guide_core.md");
/// How to build, dress and give props to a 3D figure (`guide {"topic": "wardrobe"}`).
pub const GUIDE_CHARACTER: &str = include_str!("guide_character.md");

/// One section of the full guide, by topic word.
fn guide_topic(topic: &str) -> Result<String, String> {
    guide_topic_with(topic, None, false)
}

/// The same, with the `part` and `full` of the request (a topic that moved
/// into a skill answers from the skill).
fn guide_topic_with(topic: &str, part: Option<&str>, full: bool) -> Result<String, String> {
    let heading = match topic.trim().to_lowercase().as_str() {
        "all" | "full" => return Ok(GUIDE.to_owned()),
        t if t.starts_with("tool:") => {
            let name = t.trim_start_matches("tool:").trim();
            let head = format!("## `{name}`\n");
            if TOOL_REFERENCE.find(&head).is_none() {
                // A long form that moved into a skill.
                return skills::tool_long_form(name).map(str::to_owned).ok_or_else(|| format!("no long form for tool \"{name}\"; its description says all there is"));
            }
            let start = TOOL_REFERENCE.find(&head).unwrap_or(0);
            let rest = &TOOL_REFERENCE[start + head.len()..];
            let end = rest.find("\n## `").unwrap_or(rest.len());
            return Ok(rest[..end].trim().to_owned());
        }
        "core" | "map" | "start" | "capabilities" => return Ok(skills::start_text()),
        "wardrobe" | "wear" | "hold" | "character_body" | "dress" | "clothes" | "props_in_hand" => return Ok(GUIDE_CHARACTER.to_owned()),
        "senses" | "see" | "listen" => "## Your senses",
        "model" | "project" | "lanes" => "## How a project is built",
        "clips" | "references" => "## Talking about clips",
        "properties" | "units" => "## Properties: one vocabulary",
        "motion" | "animation" | "text" | "made" | "merge" => "## Motion",
        "ops" | "edit" => "## Ops for `edit`",
        "context" | "tokens" => "## Keeping your context small",
        "habits" => "## Good habits",
        "shell" | "cli" => "## From a shell",
        #[cfg(feature = "commerce")]
        "account" | "credits" | "plan" | "plans" | "billing" | "cloud" | "market" | "marketplace" => "## Account, credits and the cloud",
        "layout" | "workspaces" | "workspace" | "panels" | "panel" | "dock" | "docking" | "tabs" => "## The window's panels",
        #[cfg(feature = "render")]
        "nodes" | "comp_nodes" | "node_kinds" => return Ok(crate::services::vfx::catalogue()),
        other => {
            if let Some(r) = skills::for_topic(other, part, full) {
                return r;
            }
            return craft_book::topic(other).or_else(|| section_by_heading(other)).ok_or_else(|| format!("no guide topic \"{other}\"; topics: senses, model, clips, properties, motion, ops, context, habits, shell, all; the craft book: craft (all of it), {}; how to do a kind of work: the skills ({}), `skill {{name}}`", craft_book::names(), skills::names()));
        }
    };
    let start = GUIDE.find(heading).ok_or("that section is missing")?;
    let rest = &GUIDE[start + heading.len()..];
    let end = rest.find("\n## ").map_or(GUIDE.len(), |i| start + heading.len() + i);
    Ok(GUIDE[start..end].trim().to_owned())
}

/// The `## ` sections of the guide: (heading line, whole section).
fn guide_sections() -> Vec<(&'static str, &'static str)> {
    let mut at: Vec<usize> = GUIDE.match_indices("\n## ").map(|(i, _)| i + 1).collect();
    if GUIDE.starts_with("## ") {
        at.insert(0, 0);
    }
    at.iter()
        .enumerate()
        .map(|(n, &i)| {
            let end = at.get(n + 1).map_or(GUIDE.len(), |&e| e);
            let text = GUIDE[i..end].trim_end();
            (text.lines().next().unwrap_or("").trim_start_matches("## "), text)
        })
        .collect()
}

/// A guide section by the words of its heading ("3D animation", "visual
/// effects"), so every section is reachable even without a topic word.
fn section_by_heading(q: &str) -> Option<String> {
    let q = q.trim().trim_start_matches("## ").to_lowercase().replace('_', " ");
    if q.len() < 4 {
        return None;
    }
    let all = guide_sections();
    let pick = all.iter().find(|(h, _)| h.to_lowercase() == q).or_else(|| all.iter().find(|(h, _)| h.to_lowercase().starts_with(&q))).or_else(|| all.iter().find(|(h, _)| h.to_lowercase().contains(&q)));
    pick.map(|(_, text)| (*text).to_owned())
}

/// Words that open a guide section, for the contents.
const TOPIC_WORDS: &[&str] = &["senses", "model", "clips", "properties", "motion", "ops", "context", "habits", "shell", "layout"];

/// The guide as a contents list: each section with its size and the word
/// that opens it. `topic: all` gives this, because the whole guide is many
/// times what any job needs and cannot be read in one go.
fn guide_contents() -> String {
    let sections = guide_sections();
    let mut s = format!("# The guide: {} sections, {} characters in all\nRead one with `guide {{\"topic\": word}}`; the word is the one shown (or, in quotes, the heading). A big section comes as its opening and an outline of its parts; `part` reads one, `full: true` all of it. Never read sections the job does not need.\n\n", sections.len(), GUIDE.len());
    for (heading, text) in &sections {
        let word = TOPIC_WORDS.iter().find(|w| guide_topic(w).is_ok_and(|t| t.starts_with(&format!("## {heading}")))).copied();
        match word {
            Some(w) => s.push_str(&format!("- {heading} ({}): {w}\n", text.len())),
            None => s.push_str(&format!("- {heading} ({}): \"{}\"\n", text.len(), heading.to_lowercase())),
        }
    }
    s.push_str("\nThe craft book (how an editor and motion designer would do it) has its own contents: `guide {\"topic\": \"craft\"}`.\n");
    s
}

/// What `guide` answers. Without a topic: the capability map and the core.
/// With one: that section, in parts when it is long (see [`parts`]).
fn guide_answer(a: &Value) -> Result<String, String> {
    let full = a.get("full").and_then(Value::as_bool).unwrap_or(false);
    let part = a.get("part").and_then(Value::as_str);
    let Some(topic) = a.get("topic").and_then(Value::as_str) else { return Ok(skills::start_text()) };
    let key = topic.trim().to_lowercase();
    if !full && matches!(key.as_str(), "all" | "full") {
        return Ok(guide_contents());
    }
    if !full && matches!(key.as_str(), "craft" | "book") {
        return Ok(craft_book::contents());
    }
    if matches!(key.as_str(), "tools" | "tool index" | "tool_index") {
        return Ok(toolindex::index(&mcp::tools()));
    }
    if let Some(name) = key.strip_prefix("tool:").map(str::trim) {
        // The long form, if the tool has one, then its fields as served.
        let long = guide_topic(&key);
        let fields = toolindex::fields(&mcp::tools(), name);
        return match (long, fields) {
            (Ok(l), Some(f)) => Ok(format!("{l}\n\n{f}")),
            (Ok(l), None) => Ok(l),
            (Err(_), Some(f)) => Ok(f),
            (Err(e), None) => Err(e),
        };
    }
    let section = guide_topic_with(topic, part, full)?;
    if section.starts_with("# Skill: ") {
        return Ok(section);
    }
    if key.starts_with("tool:") || matches!(key.as_str(), "core" | "map" | "start" | "capabilities") {
        return Ok(section);
    }
    parts::serve(&section, &key, part, full)
}

/// Voices `voices` lists before the rest (`all: true`).
const VOICES_LISTED: usize = 30;

/// Most names a plain `catalogue` list shows, however large a `limit` is asked.
const CATALOGUE_LIST_MAX: usize = 150;

/// What a tool returns.
pub enum Content {
    Text(String),
    /// A JPEG picture.
    Jpeg(Vec<u8>),
}

pub struct Session {
    pub doc: Doc,
    ai: services::ai_run::AiRunner,
    #[cfg(feature = "render")]
    exports: Vec<(u32, services::export::ExportJob)>,
    next_export: u32,
    #[cfg(feature = "render")]
    frames: render::Frames,
    analysis: analyse::Analysis,
    /// Project mode: the folder every change is saved to.
    pub home: Option<PathBuf>,
    /// Notes when there is no project folder yet.
    notes: Option<String>,
    /// The production office when there is no project folder yet.
    production: Option<studio_production::Production>,
    /// Picture measurements by scene fingerprint (kept with the project),
    /// so check only re-renders what changed.
    look_memo: Option<std::collections::HashMap<String, Value>>,
    /// What the person asked for, as the AI was given it (the 3D critic
    /// reads the parts and lights the request names).
    brief: Option<String>,
    /// Picture batches being made in the background (job number, batch).
    batches: Vec<(u32, picture_batch::Batch)>,
    /// Image-to-3D models being made in the background (job numbers are shared with batches).
    model_jobs: Vec<(u32, image3d::ModelJob)>,
    next_batch: u32,
    /// How deep in tool calls this is (news of batches goes on the outer one).
    call_depth: u32,
}

impl Default for Session {
    fn default() -> Self {
        Self::new(services::project::new_custom("Untitled", 1920, 1080, 30.0))
    }
}

#[cfg(feature = "render")]
fn quality_key(s: &Session, t0: view::Time, t1: view::Time, tag: &str) -> String {
    s.scene_key_pub(t0, t1, tag)
}

/// Two decimals: enough for any setting, and short to read.
fn r2(v: f32) -> f64 {
    (v as f64 * 100.0).round() / 100.0
}

fn secs(t: view::Time) -> f64 {
    (t as f64 / SEC as f64 * 1000.0).round() / 1000.0
}

fn micros(s: f64) -> view::Time {
    (s * SEC as f64).round() as view::Time
}

/// How long a new shot is when nothing says and the film is not that long
/// yet (a new project is one second, and a scene built in it should not be).
const NEW_SHOT: view::Time = 5 * SEC;

/// Where a new clip starting at `at` ends when no length is given: with the
/// film, or a shot's length on when the film ends within a second of `at`.
fn new_clip_end(film_end: view::Time, at: view::Time) -> view::Time {
    if film_end >= at + SEC { film_end } else { at + NEW_SHOT }
}

/// A number field. A number written as a string ("0.5", " 12 ") counts:
/// models often quote their numbers, and the meaning is not in doubt.
fn f(v: &Value, k: &str) -> Option<f64> {
    match v.get(k)? {
        Value::String(s) => s.trim().parse::<f64>().ok().filter(|x| x.is_finite()),
        x => x.as_f64(),
    }
}

fn ids(v: &Value, k: &str) -> Vec<u32> {
    match v.get(k) {
        Some(Value::Array(a)) => a.iter().filter_map(craft::as_id).map(|x| x as u32).collect(),
        Some(x) => craft::as_id(x).map(|x| vec![x as u32]).unwrap_or_default(),
        None => Vec::new(),
    }
}

fn kind_name(k: ClipKind) -> &'static str {
    match k {
        ClipKind::Video => "video",
        ClipKind::Audio => "audio",
        ClipKind::Title => "title",
        ClipKind::Fx => "adjustment",
    }
}

fn panel_of(name: &str) -> Option<Panel> {
    Some(match name.trim().to_lowercase().as_str() {
        "effects" | "effect" => Panel::Effects,
        "looks" | "look" | "filters" | "filter" => Panel::Looks,
        "motions" | "motion" | "animations" | "animation" => Panel::Motions,
        "text" | "texts" | "styles" | "text styles" => Panel::Text,
        "transitions" | "transition" | "cuts" => Panel::Cuts,
        "assets" | "stickers" | "shapes" | "overlays" | "models" | "my models" | "3d models" => Panel::Assets,
        "audio" | "music" | "sounds" | "sound effects" => Panel::Audio,
        "media" | "bin" => Panel::Media,
        _ => return None,
    })
}

fn panel_name(p: Panel) -> &'static str {
    match p {
        Panel::Effects => "effects",
        Panel::Looks => "looks",
        Panel::Motions => "motions",
        Panel::Text => "text",
        Panel::Cuts => "transitions",
        Panel::Assets => "assets",
        Panel::Audio => "audio",
        Panel::Media => "media",
        Panel::Ai => "ai",
    }
}

/// Edit distance between two short words.
fn lev(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut prev = row[0];
        row[0] = i;
        for j in 1..=b.len() {
            let cur = row[j];
            row[j] = (row[j] + 1).min(row[j - 1] + 1).min(prev + usize::from(a[i - 1] != b[j - 1]));
            prev = cur;
        }
    }
    row[b.len()]
}

/// How well `want` words match a name's words, 0..1 (1 = every word found).
fn similarity(want: &[String], have: &[String]) -> f32 {
    if want.is_empty() || have.is_empty() {
        return 0.0;
    }
    let per: f32 = want
        .iter()
        .map(|w| {
            have.iter()
                .map(|h| {
                    if h.starts_with(w.as_str()) || w.starts_with(h.as_str()) {
                        1.0
                    } else {
                        let d = lev(w, h) as f32;
                        (1.0 - d / w.len().max(h.len()) as f32).max(0.0)
                    }
                })
                .fold(0.0f32, f32::max)
        })
        .sum();
    per / want.len() as f32
}

/// The parameters an effect has: id, range, default, and whether it can
/// be keyframed (so agents can tune and animate without guessing).
fn effect_settings(kind: &str) -> Value {
    use studio_fx::params::{ParamKind, ParamValue};
    let Some(def) = studio_fx::effects::find(kind) else { return json!([]) };
    json!(def.params.iter().map(|p| {
        let mut o = json!({"param": p.id, "label": p.label, "animatable": p.animatable});
        match p.kind {
            ParamKind::Float { min, max } => { o["min"] = json!(min); o["max"] = json!(max); }
            ParamKind::Int { min, max } => { o["min"] = json!(min); o["max"] = json!(max); }
            ParamKind::Bool => { o["type"] = json!("bool"); }
            ParamKind::Choice(c) => { o["choices"] = json!(c); }
            _ => { o["type"] = json!("other"); }
        }
        if let ParamValue::Float(d) = p.default { o["default"] = json!(d); }
        if let ParamValue::Int(d) = p.default { o["default"] = json!(d); }
        o
    }).collect::<Vec<_>>())
}

const ALSO: &[(&str, &[&str])] = &[
        ("boom", &["explosion", "impact", "hit"]),
        ("riser", &["rise", "sweep", "whoosh", "swell"]),
        ("swoosh", &["whoosh", "swish"]),
        ("whoosh", &["swoosh", "swish"]),
        ("hit", &["impact", "punch"]),
        ("bass", &["impact", "low"]),
        ("drop", &["impact", "boom"]),
        ("transition", &["whoosh"]),
        ("glitch", &["digital", "error"]),
        ("fire", &["flame"]),
        ("car", &["vehicle", "automobile"]),
        ("plane", &["airplane", "jet"]),
    ];

/// What else an editor means by a search word.
fn related(word: &str) -> Vec<String> {
    ALSO.iter().filter(|(k, _)| *k == word).flat_map(|(_, alts)| alts.iter().map(|a| (*a).to_owned())).collect()
}

/// Words of a name, lower case.
fn words(s: &str) -> Vec<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_owned).collect()
}

impl Session {
    pub fn new(doc: Doc) -> Self {
        Self {
            doc,
            ai: services::ai_run::AiRunner::default(),
            #[cfg(feature = "render")]
            exports: Vec::new(),
            next_export: 1,
            #[cfg(feature = "render")]
            frames: render::Frames::default(),
            analysis: analyse::Analysis::default(),
            home: None,
            notes: None,
            production: None,
            look_memo: None,
            brief: None,
            batches: Vec::new(),
            model_jobs: Vec::new(),
            next_batch: 1,
            call_depth: 0,
        }
    }

    /// A session bound to a project folder: opens it, or starts a new
    /// project there, and saves after every change.
    pub fn at(dir: &std::path::Path) -> Result<Self, String> {
        let mut s = match services::project::open(dir) {
            Ok(doc) => Session::new(doc),
            Err(_) if !dir.exists() || dir.read_dir().map(|mut d| d.next().is_none()).unwrap_or(true) => {
                let mut s = Session::default();
                let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Untitled".into());
                // Named in the model (the sequence), so the saved file has it.
                let seq = s.doc.seq();
                s.doc.apply("Name", m::Edit::RenameSequence { id: seq, name });
                s.doc.history.clear();
                services::project::save(&mut s.doc, Some(dir))?;
                s
            }
            Err(e) => return Err(e),
        };
        s.home = Some(dir.to_path_buf());
        // On the Start screen of every Studio window from now on.
        crate::files::note_project(dir);
        Ok(s)
    }

    /// Runs a tool and, in project mode, saves when the project changed.
    pub fn run(&mut self, tool: &str, a: &Value) -> Result<Vec<Content>, String> {
        // The flight recorder (trace.rs): who called, what changed.
        let mark = trace::Mark::new(self, tool, a);
        let a = mark.args();
        let started = std::time::Instant::now();
        // hot.rs: tools swapped in from crates/dev/agent-hot (agent-dev builds; nothing otherwise).
        let r = match hot::call(tool, a) {
            Some(r) => r,
            None if tool == trace::FEEDBACK => trace::feedback(a),
            None => self.run_inner(tool, &hot::normalise(tool, a)),
        };
        self.record(tool, a, &r, started.elapsed());
        mark.end(self, tool, &r);
        r
    }

    /// One line per call in `agent-usage.jsonl` (the project folder, or
    /// the cache): what was asked, how long it took, how much came back.
    /// The measure of how well the app serves agents.
    fn record(&self, tool: &str, a: &Value, r: &Result<Vec<Content>, String>, took: std::time::Duration) {
        let (mut text, mut image, mut images) = (0usize, 0usize, 0usize);
        if let Ok(parts) = r {
            for p in parts {
                match p {
                    Content::Text(t) => text += t.len(),
                    Content::Jpeg(b) => {
                        image += b.len();
                        images += 1;
                    }
                }
            }
        }
        let ops: Vec<&str> = a.get("ops").and_then(Value::as_array).map(|v| v.iter().filter_map(|o| o.get("op").and_then(Value::as_str)).collect()).unwrap_or_default();
        let line = json!({
            "at": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0),
            "tool": tool,
            "ops": ops,
            "args_bytes": a.to_string().len(),
            "ms": took.as_millis() as u64,
            "text_bytes": text,
            "image_bytes": image,
            "images": images,
            "error": r.as_ref().err().map(|e| e.chars().take(160).collect::<String>()),
        });
        let path = self.home.clone().map(|h| h.join("agent-usage.jsonl")).unwrap_or_else(|| studio_paths::cache().join("agent-usage.jsonl"));
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            use std::io::Write;
            let _ = writeln!(f, "{line}");
        }
    }

    fn run_inner(&mut self, tool: &str, a: &Value) -> Result<Vec<Content>, String> {
        let home = self.home.clone();
        props::set_canvas(self.doc.project.width as f64, self.doc.project.height as f64);
        let before = (self.doc.history.revision(), self.doc.project.media.len());
        let mut a = a.clone();
        if let Some(t) = mcp::tools().into_iter().find(|t| t["name"] == tool) {
            let allowed: Vec<String> = t["inputSchema"]["properties"].as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
            let mut allowed: Vec<&str> = allowed.iter().map(String::as_str).collect();
            // Filled in by the app: the request for the 3D critic, and the
            // closing check that looks at the 3D notes alone.
            if matches!(tool, "check" | "review") {
                allowed.extend(["brief", "only"]);
            }
            // `generate` with task model3d: a model from pictures.
            if tool == "generate" && a.get("task").and_then(Value::as_str) == Some("model3d") {
                allowed.extend(["file", "size", "cells", "faces", "thickness", "mirror", "texture", "clean_up", "class", "method", "resolution", "wait", "duration", "lane", "position", "rotation", "shadows"]);
            }
            args::normalise(tool, &format!("`{tool}`"), &mut a, &allowed)?;
        }
        if tool != "edit" {
            select::resolve_refs(&self.doc.project, &[], &mut a)?;
        }
        let models_before = self.model_files();
        let mut r = self.call(tool, &a);
        if let Ok(parts) = r.as_mut() {
            compact::parts(tool, parts);
            // Text someone else may have written is cleaned and marked as
            // material, not instructions (outside.rs).
            outside::process(tool, &a, parts);
            // The 3D models made or changed are kept for every project.
            let kept = self.keep_models(tool, &a, &models_before);
            if !kept.is_empty() {
                parts.push(Content::Text(Self::kept_line(&kept)));
            }
        }
        if let Some(dir) = home {
            // `open` / `new_project` replace the session; stay bound.
            let switched = matches!(tool, "open" | "new_project");
            if !switched || tool == "new_project" {
                self.home = Some(dir.clone());
            }
            let changed = (self.doc.history.revision(), self.doc.project.media.len()) != before || tool == "new_project";
            if r.is_ok() && changed && self.home.is_some() {
                let target = if tool == "new_project" { Some(dir.as_path()) } else { None };
                if let Err(e) = services::project::save(&mut self.doc, target) {
                    if let Ok(parts) = r.as_mut() {
                        parts.push(Content::Text(format!("{{\"autosave_failed\": {}}}", json!(e))));
                    }
                }
            }
        }
        r
    }

    // ------------------------------------------------------------- reading

    /// One compact line per clip, plus the canvas, lanes and markers.
    pub fn summary(&self) -> Value {
        let p = &self.doc.project;
        let clips: Vec<Value> = {
            let mut cs: Vec<&view::Clip> = p.clips.iter().collect();
            cs.sort_by_key(|c| (c.lane, c.start));
            cs.iter().map(|c| self.clip_line(c)).collect()
        };
        json!({
            "revision": self.doc.history.revision(),
            "name": p.name,
            "canvas": [p.width, p.height],
            "fps": p.fps,
            "duration": secs(p.duration()),
            "lanes": p.lanes.iter().map(|l| json!({"id": l.id, "name": l.name, "kind": match l.kind { view::LaneKind::Spine => "main", view::LaneKind::Visual => "overlay", view::LaneKind::Audio => "sound" }, "locked": l.locked, "hidden": l.hidden, "muted": l.muted})).collect::<Vec<_>>(),
            "clips": clips,
            "markers": p.markers.iter().map(|m| json!({"t": secs(m.time), "note": m.note})).collect::<Vec<_>>(),
            "media_count": p.media.len(),
            "can_undo": self.doc.can_undo(),
        })
    }

    fn clip_line(&self, c: &view::Clip) -> Value {
        let mut o = serde_json::Map::new();
        o.insert("id".into(), json!(c.id));
        o.insert("lane".into(), json!(c.lane));
        // Cameras and lights look like adjustment clips to the view, models
        // like titles.
        let rig = if matches!(c.kind, ClipKind::Fx | ClipKind::Title) { self.model_clip(c.id).and_then(|mc| space::rig_kind(&mc)) } else { None };
        o.insert("kind".into(), json!(rig.unwrap_or(kind_name(c.kind))));
        o.insert("name".into(), json!(c.name));
        o.insert("start".into(), json!(secs(c.start)));
        o.insert("dur".into(), json!(secs(c.dur)));
        if c.media.is_some() {
            o.insert("media".into(), json!(c.media));
            o.insert("source_in".into(), json!(secs(c.src_in)));
        }
        if !c.text.is_empty() {
            o.insert("text".into(), json!(c.text));
        }
        if let Some(s) = &c.text_style {
            o.insert("style".into(), json!(s));
        }
        if (c.speed - 100.0).abs() > 0.01 {
            o.insert("speed".into(), json!(c.speed / 100.0));
        }
        if !c.effects.is_empty() {
            o.insert("effects".into(), json!(c.effects.iter().map(|e| e.kind.clone()).collect::<Vec<_>>()));
        }
        if let Some(l) = &c.look {
            o.insert("look".into(), json!(l));
        }
        if let Some(mo) = &c.motion_in {
            o.insert("motion".into(), json!(mo));
        }
        if c.audio.volume.abs() > 0.01 {
            o.insert("volume".into(), json!(r2(c.audio.volume)));
        }
        if (c.transform.opacity - 100.0).abs() > 0.01 {
            o.insert("opacity".into(), json!(r2(c.transform.opacity)));
        }
        if (c.transform.scale - 100.0).abs() > 0.01 {
            o.insert("scale".into(), json!(r2(c.transform.scale)));
        }
        if c.transform.x.abs() > 0.01 || c.transform.y.abs() > 0.01 {
            o.insert("x".into(), json!(r2(c.transform.x)));
            o.insert("y".into(), json!(r2(c.transform.y)));
        }
        if c.kind == ClipKind::Title {
            o.insert("text_size".into(), json!(r2(c.extra.text_size)));
        }
        let g = &c.grade;
        if [g.exposure, g.contrast, g.saturation, g.temperature, g.tint].iter().any(|v| v.abs() > 0.01) {
            o.insert("grade".into(), json!({"exposure": r2(g.exposure / 25.0), "contrast": r2(g.contrast), "saturation": r2(g.saturation), "temperature": r2(g.temperature), "tint": r2(g.tint)}));
        }
        if !c.keys.is_empty() {
            o.insert("animated".into(), json!(c.keys.iter().map(|k| k.0.clone()).collect::<Vec<_>>()));
        }
        Value::Object(o)
    }

    fn inspect(&self, id: u32) -> Result<Value, String> {
        let c = self.doc.project.clip(id).ok_or_else(|| format!("No clip {id}"))?;
        let mut v = self.clip_line(c);
        let o = v.as_object_mut().expect("object");
        o.insert("transform".into(), json!({"x": r2(c.transform.x), "y": r2(c.transform.y), "scale": r2(c.transform.scale), "rotation": r2(c.transform.rotation), "opacity": r2(c.transform.opacity)}));
        o.insert("grade".into(), json!({"exposure_stops": r2(c.grade.exposure / 25.0), "contrast": r2(c.grade.contrast), "saturation": r2(c.grade.saturation), "temperature": r2(c.grade.temperature), "tint": r2(c.grade.tint)}));
        if c.kind == ClipKind::Title {
            o.insert("text_size".into(), json!(r2(c.extra.text_size)));
            let k = c.text_colour;
            o.insert("colour".into(), json!(format!("#{:02x}{:02x}{:02x}", k.r(), k.g(), k.b())));
        }
        o.insert("audio".into(), json!({"enabled": c.audio.enabled, "volume_db": c.audio.volume, "pan": c.audio.pan, "fade_in": c.audio.fade_in, "fade_out": c.audio.fade_out}));
        o.insert("effects".into(), json!(c.effects.iter().map(|e| json!({"id": e.id, "kind": e.kind, "enabled": e.enabled, "params": e.params.iter().cloned().collect::<BTreeMap<_, _>>(), "keyframe_path": format!("effect.{}.<param>", e.id), "settings": effect_settings(&e.kind)})).collect::<Vec<_>>()));
        // Every property by its friendly name, now and keyed, in the units
        // set and animate take.
        if let Some(mc) = self.model_clip(id) {
            let d = props::describe(&mc);
            o.insert("properties".into(), d["values"].clone());
            o.insert("keyframes".into(), d["keyframes"].clone());
            o.insert("sound".into(), sound_ops::describe(&mc));
            for k in ["blend", "reversed", "enabled", "masks"] {
                o.insert(k.into(), d[k].clone());
            }
            if let Some(p) = self.pins_of(id) {
                o.insert("pins".into(), p);
            }
            if let Some(sk) = self.skeleton_said(id, c.start) {
                o.insert("skeleton".into(), sk);
            }
            if let Some(sp) = self.space_of(id) {
                o.insert("space".into(), sp);
            }
            if let Ok((_, mesh)) = self.model_of(id) {
                o.insert("model".into(), models::describe(&mesh));
            }
        }
        Ok(v)
    }

    fn media(&self) -> Value {
        json!(self.doc.project.media.iter().map(|m| json!({
            "id": m.id, "name": m.name,
            "kind": match m.kind { MediaKind::Video => "video", MediaKind::Image => "image", MediaKind::Audio => "audio" },
            "dur": secs(m.dur), "size": [m.width, m.height], "fps": m.fps,
            "path": m.path.as_ref().map(|p| p.display().to_string()),
        })).collect::<Vec<_>>())
    }

    // ------------------------------------------------------------- library

    fn items(&self, panel: Panel) -> Vec<services::library::Item> {
        let n = services::library::count(&self.doc, panel);
        services::library::page(&self.doc, panel, 0, "", 0, n)
    }

    fn catalogue(&self, panel: Panel, query: &str, limit: usize) -> Value {
        if panel == Panel::Audio && !query.trim().is_empty() && shelf::content().is_some() {
            return self.sound_search(&inventory::Ask::from_words(query), limit);
        }
        let mut query = query.to_owned();
        let mut widened = None;
        if !query.trim().is_empty() && services::library::search(&self.doc, panel, 0, &query).is_empty() {
            // Nothing for the exact words: try what editors mean by them,
            // then each word alone.
            let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
            let mut tries: Vec<String> = Vec::new();
            for w in &words {
                tries.extend(related(w));
            }
            tries.extend(words.iter().cloned());
            if let Some(t) = tries.into_iter().find(|t| !services::library::search(&self.doc, panel, 0, t).is_empty()) {
                widened = Some(format!("nothing matched \"{query}\"; showing \"{t}\""));
                query = t;
            }
        }
        let query = query.as_str();
        let items = services::library::page(&self.doc, panel, 0, query, 0, limit.max(1));
        let total = services::library::search(&self.doc, panel, 0, query).len();
        // Sound processing is not a picture effect: say where it lives.
        let sound_words = ["eq", "equalizer", "equaliser", "compressor", "compression", "limiter", "gate", "noise gate", "de-esser", "de_esser", "deesser", "reverb", "echo", "delay", "pitch", "duck", "ducking", "fade"];
        if total == 0 && widened.is_none() && query.split_whitespace().any(|w| sound_words.contains(&w.to_lowercase().as_str())) {
            widened = Some(format!("sound effects are not in this panel: use the audio op. {}", sound_ops::EFFECTS));
        }
        json!({
            "panel": panel_name(panel),
            "note": widened,
            "total": total,
            "items": items.iter().map(|i| {
                let mut o = json!({"name": i.name, "category": i.category});
                if !i.description.is_empty() { o["description"] = json!(i.description); }
                if let Some(d) = i.duration { o["dur"] = json!(secs(d)); }
                // Effect settings, so `params` can be written without guessing.
                if panel == Panel::Effects {
                    let settings = effect_settings(&crate::catalogue::effects()[i.index].id);
                    if settings.as_array().is_some_and(|a| !a.is_empty()) { o["settings"] = settings; }
                }
                o
            }).collect::<Vec<_>>(),
        })
    }

    /// Finds a library item by name in `panels` (exact, then every word,
    /// then partial). On a miss the error lists the closest names.
    fn resolve(&self, name: &str, panels: &[Panel]) -> Result<(Panel, usize, String), String> {
        // The library (with the person's kept models) is loaded before
        // names are looked up: a headless run has no window to preload it.
        let _ = shelf::content();
        let want = name.trim().to_lowercase();
        let want_words = words(name);
        let mut scored: Vec<(f32, Panel, usize, String)> = Vec::new();
        for &p in panels {
            for it in self.items(p) {
                if it.name.to_lowercase() == want {
                    return Ok((p, it.index, it.name));
                }
                let score = similarity(&want_words, &words(&it.name));
                if score > 0.0 {
                    scored.push((score, p, it.index, it.name.clone()));
                }
            }
        }
        // A word search ("impact", "neon frame") finds items by name,
        // category and tags; take its best hit before guessing by spelling.
        for &p in panels {
            let hits = services::library::search(&self.doc, p, 0, name);
            let named = hits.iter().map(|&i| services::library::page(&self.doc, p, 0, name, 0, hits.len().min(400))).next().unwrap_or_default();
            let pick = named.iter().find(|it| words(&it.name).iter().any(|w| want_words.contains(w))).or_else(|| named.iter().find(|it| it.duration.is_none_or(|d| d < 10 * SEC))).or(named.first());
            if let Some(it) = pick {
                return Ok((p, it.index, it.name.clone()));
            }
        }
        for alt in want_words.iter().flat_map(|w| related(w)) {
            for &p in panels {
                if let Some(&i) = services::library::search(&self.doc, p, 0, &alt).first() {
                    let it = services::library::item(&self.doc, p, i as usize);
                    return Ok((p, it.index, it.name));
                }
            }
        }
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        // A clear best match (a typo, a shortened name) is taken.
        if let Some(best) = scored.first() {
            let runner_up = scored.get(1).map_or(0.0, |s| s.0);
            if best.0 >= 0.75 && best.0 - runner_up >= 0.08 || best.0 >= 0.999 {
                return Ok((best.1, best.2, best.3.clone()));
            }
        }
        let sugg: Vec<String> = scored.iter().take(6).filter(|s| s.0 >= 0.4).map(|(_, p, _, n)| format!("{n} ({})", panel_name(*p))).collect();
        Err(if sugg.is_empty() {
            format!("nothing called \"{name}\" in {}; use catalogue to list names", panels.iter().map(|p| panel_name(*p)).collect::<Vec<_>>().join(", "))
        } else {
            format!("nothing called \"{name}\"; did you mean {}", sugg.join(", "))
        })
    }

    // ---------------------------------------------------------------- edit

    /// Applies `ops` as one undo step (all or nothing) and reports what
    /// changed. `dry_run` checks the plan and leaves the project as it was.
    pub fn edit(&mut self, ops: &[Value], label: &str, dry_run: bool) -> Result<Value, String> {
        // Repeats written out first, so "$1" and "$last" count every clip.
        let ops = &repeat::expand(ops).map_err(|e| format!("{e}. Nothing was changed."))?;
        // Lines to be spoken are made ahead, several at once.
        if !dry_run {
            self.prepare_narration(ops);
        }
        let before: BTreeMap<u32, Value> = self.doc.project.clips.iter().map(|c| (c.id, self.clip_line(c))).collect();
        let cuts_before = self.cut_lines();
        self.doc.begin_batch(label);
        let mut notes = Vec::new();
        // Clips made by earlier ops in this call, for "$1" / "$last".
        let mut made: Vec<u32> = Vec::new();
        // The clips each op names, and how they were before it, to read back.
        let mut named: Vec<(usize, String, Vec<u32>)> = Vec::new();
        let mut prints: std::collections::HashMap<u32, Option<u64>> = std::collections::HashMap::new();
        for (i, op) in ops.iter().enumerate() {
            let mut op = args::unwrap_op(op);
            if let Err(e) = select::resolve_refs(&self.doc.project, &made, &mut op) {
                self.doc.rollback_batch();
                let name = op.get("op").and_then(Value::as_str).unwrap_or("?");
                return Err(format!("op {} ({name}): {e}. Nothing was changed.", i + 1));
            }
            let ids_before: std::collections::BTreeSet<u32> = self.doc.project.clips.iter().map(|c| c.id).collect();
            if let Some(name) = op.get("op").and_then(Value::as_str).map(str::to_owned) {
                if let Some(allowed) = args::op_fields(&name) {
                    if let Err(e) = args::normalise(&name, &format!("op {} ({name})", i + 1), &mut op, allowed) {
                        self.doc.rollback_batch();
                        return Err(format!("{e}. Nothing was changed."));
                    }
                }
            }
            // A file named where media is wanted comes in as media.
            if let Err(e) = self.media_paths(&mut op, &mut notes) {
                self.doc.rollback_batch();
                let name = op.get("op").and_then(Value::as_str).unwrap_or("?");
                return Err(format!("op {} ({name}): {e}. Nothing was changed.", i + 1));
            }
            let ts = readback::targets(&op);
            for &id in &ts {
                prints.entry(id).or_insert_with(|| self.clip_print(id));
            }
            named.push((i, op.get("op").and_then(Value::as_str).unwrap_or("?").to_owned(), ts));
            // "clips" on an op that works on one clip: once per clip.
            let many: Vec<Value> = match (op.get("op").and_then(Value::as_str), op.get("clips"), op.get("clip")) {
                (Some("set" | "animate" | "keys" | "effect" | "mask" | "motion" | "use_move" | "reframe_one"), Some(Value::Array(cs)), None) => cs
                    .iter()
                    .map(|c| {
                        let mut o = op.clone();
                        if let Some(m) = o.as_object_mut() {
                            m.remove("clips");
                            m.insert("clip".into(), c.clone());
                        }
                        o
                    })
                    .collect(),
                _ => vec![op.clone()],
            };
            let op = &op;
            let r: Result<String, String> = if many.len() > 1 || op.get("clip").is_none() && many.first().is_some_and(|o| o.get("clip").is_some()) {
                many.iter().map(|o| self.op(o)).collect::<Result<Vec<_>, _>>().map(|v| v.join("; "))
            } else {
                self.op(op)
            };
            match r {
                Ok(n) => {
                    notes.push(n);
                    let mut fresh: Vec<&view::Clip> = self.doc.project.clips.iter().filter(|c| !ids_before.contains(&c.id)).collect();
                    fresh.sort_by_key(|c| c.id);
                    made.extend(fresh.iter().map(|c| c.id));
                }
                Err(e) => {
                    self.doc.rollback_batch();
                    let name = op.get("op").and_then(Value::as_str).unwrap_or("?");
                    return Err(format!("op {} ({name}) failed: {}. Nothing was changed.", i + 1, e.trim_end_matches('.')));
                }
            }
        }
        if dry_run {
            let mut report = self.changes(&before);
            report["transitions"] = self.cut_changes(&cuts_before);
            self.doc.rollback_batch();
            return Ok(json!({"dry_run": true, "ok": true, "would_change": report, "notes": notes}));
        }
        self.doc.commit_batch();
        let mut changed = self.changes(&before);
        let cuts = self.cut_changes(&cuts_before);
        if cuts.as_object().is_some_and(|o| o.values().any(|v| v.as_array().is_some_and(|a| !a.is_empty()))) {
            changed["transitions"] = cuts;
        }
        Ok(json!({"ok": true, "revision": self.doc.history.revision(), "changed": changed, "notes": notes, "read_back": self.read_back(&named, &prints, &made), "duration": secs(self.doc.project.duration())}))
    }

    fn cut_lines(&self) -> Vec<Value> {
        self.doc.project.transitions.iter().map(|t| json!({"from_clip": t.left, "to_clip": t.right, "kind": t.kind, "dur": secs(t.dur)})).collect()
    }

    fn cut_changes(&self, before: &[Value]) -> Value {
        let after = self.cut_lines();
        json!({"added": after.iter().filter(|t| !before.contains(t)).collect::<Vec<_>>(), "removed": before.iter().filter(|t| !after.contains(t)).collect::<Vec<_>>()})
    }

    fn changes(&self, before: &BTreeMap<u32, Value>) -> Value {
        let after: BTreeMap<u32, Value> = self.doc.project.clips.iter().map(|c| (c.id, self.clip_line(c))).collect();
        let added: Vec<&Value> = after.iter().filter(|(k, _)| !before.contains_key(k)).map(|(_, v)| v).collect();
        let removed: Vec<u32> = before.keys().filter(|k| !after.contains_key(k)).copied().collect();
        let changed: Vec<&Value> = after.iter().filter(|(k, v)| before.get(k).is_some_and(|b| b != *v)).map(|(_, v)| v).collect();
        json!({"added": added, "removed": removed, "changed": changed})
    }

    fn check(&mut self, ok: bool, what: &str) -> Result<String, String> {
        match self.doc.error.take() {
            Some(e) => Err(e),
            None if ok => Ok(what.to_owned()),
            // Asking for what is already so is not an error.
            None => Ok(format!("{what} (no change)")),
        }
    }

    /// The model's clip (all of it: masks, keys, effects by id).
    /// An op's `media` given as a file (a path, `~/…` too, alone or in a
    /// list): the file is brought into the project, in this edit's undo
    /// step, and the op gets its media id. A file already in the project is
    /// that media.
    fn media_paths(&mut self, op: &mut Value, said: &mut Vec<String>) -> Result<(), String> {
        let Some(media) = op.get_mut("media") else { return Ok(()) };
        let mut one = |v: &mut Value| -> Result<(), String> {
            let Some(s) = v.as_str() else { return Ok(()) };
            let path = match s.strip_prefix("~/") {
                Some(rest) => std::env::var_os("HOME").map_or_else(|| std::path::PathBuf::from(s), |h| std::path::Path::new(&h).join(rest)),
                None => std::path::PathBuf::from(s),
            };
            if !path.is_absolute() || !path.is_file() {
                return Ok(());
            }
            let r = crate::services::project::import_media(&mut self.doc, std::slice::from_ref(&path));
            let id = *r.ids.first().ok_or_else(|| format!("could not bring in {}: {}", path.display(), r.failed.join("; ")))?;
            if r.reused == 0 {
                said.push(format!("imported {} as media {id}", path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())));
            }
            *v = json!(id);
            Ok(())
        };
        match media {
            Value::Array(items) => items.iter_mut().try_for_each(&mut one),
            v => one(v),
        }
    }

    fn model_clip(&self, id: u32) -> Option<m::Clip> {
        self.doc.model.sequences.iter().find_map(|q| q.clip(m::ClipId(id as u64))).cloned()
    }

    fn media_item(&self, v: &Value) -> Result<view::MediaItem, String> {
        let id = craft::media_of(&self.doc.project, v)?;
        self.doc.project.media(id).cloned().ok_or_else(|| "that media is gone".into())
    }

    fn clip(&self, id: u32) -> Result<view::Clip, String> {
        self.doc.project.clip(id).cloned().ok_or_else(|| format!("No clip {id} (see summary for ids)"))
    }

    fn op(&mut self, op: &Value) -> Result<String, String> {
        let name = op.get("op").and_then(Value::as_str).ok_or("every op needs an \"op\" field")?;
        // Aimed inside a merged clip: done in place, no unmerging.
        if let Some(r) = self.route_inside(op) {
            return r;
        }
        let seq = self.doc.seq();
        match name {
            "split" => {
                let at = f(op, "at").ok_or("split needs \"at\" (seconds)")?;
                let clips = ids(op, "clips");
                let edit = m::Edit::Split { seq, at: micros(at), clips: (!clips.is_empty()).then(|| clips.iter().map(|&c| m::ClipId(c as u64)).collect()) };
                let ok = self.doc.apply("Split", edit);
                self.check(ok, &format!("split at {at}s"))
            }
            "trim" => {
                let id = op.get("clip").and_then(Value::as_u64).ok_or("trim needs \"clip\"")? as u32;
                let ripple = op.get("ripple").and_then(Value::as_bool).unwrap_or(false);
                let c = self.clip(id)?;
                if let Some(s) = f(op, "start") {
                    let delta = micros(s) - c.start;
                    let ok = self.doc.apply("Trim", m::Edit::Trim { seq, clip: m::ClipId(id as u64), edge: m::Edge::Start, delta, ripple });
                    self.check(ok || delta == 0, "trim start")?;
                }
                if let Some(e) = f(op, "end") {
                    let c = self.clip(id)?;
                    let delta = micros(e) - (c.start + c.dur);
                    let ok = self.doc.apply("Trim", m::Edit::Trim { seq, clip: m::ClipId(id as u64), edge: m::Edge::End, delta, ripple });
                    self.check(ok || delta == 0, "trim end")?;
                }
                Ok(format!("trimmed clip {id}"))
            }
            "delete" => {
                let clips = ids(op, "clips");
                if clips.is_empty() {
                    return Err("delete needs \"clips\"".into());
                }
                let cl: Vec<m::ClipId> = clips.iter().map(|&c| m::ClipId(c as u64)).collect();
                let ripple = op.get("ripple").and_then(Value::as_bool).unwrap_or(true);
                let ok = if ripple { self.doc.apply("Delete", m::Edit::RippleDelete { seq, clips: cl }) } else { self.doc.apply("Delete", m::Edit::Lift { seq, clips: cl }) };
                self.check(ok, &format!("deleted {} clip(s)", clips.len()))
            }
            // Without the linked picture or sound (linked selection off).
            "move" if op.get("linked") == Some(&json!(false)) => self.editorial("move", op),
            "move" => {
                let clips = ids(op, "clips");
                let first = clips.iter().filter_map(|&c| self.doc.project.clip(c)).map(|c| c.start).min().ok_or("move needs existing \"clips\"")?;
                let delta = match (f(op, "by"), f(op, "to")) {
                    (Some(b), _) => micros(b),
                    (None, Some(t)) => micros(t) - first,
                    _ => 0,
                };
                let to_lane = match op.get("lane") {
                    Some(v) => Some(m::LaneId(craft::lane_of(&self.doc.project, v, None)? as u64)),
                    None => None,
                };
                let ok = self.doc.apply("Move", m::Edit::Move { seq, clips: clips.iter().map(|&c| m::ClipId(c as u64)).collect(), delta, to_lane });
                self.check(ok, "moved")
            }
            "set" => {
                let id = op.get("clip").and_then(Value::as_u64).ok_or("set needs \"clip\"")? as u32;
                if op.get("anchor").is_some() || op.get("at").is_some() {
                    return self.set_staged(id, op);
                }
                let mut o = op.clone();
                let c0 = self.clip(id)?;
                if self.doc.project.spine() == Some(c0.lane) {
                    // The main lane is magnetic: a new length or speed
                    // ripples the clips after it instead of being refused.
                    if o.get("start").is_some() {
                        return Err("the main lane is magnetic (clips sit end to end), so \"start\" cannot be set there; use move with \"to\" to reorder, or put the clip on an overlay lane".into());
                    }
                    if o.get("speed").is_some() || o.get("duration").is_some() {
                        let speed = f(&o, "speed").map(|v| (v * 100.0).clamp(1.0, 10_000.0) as f32).unwrap_or(c0.speed);
                        let dur = match f(&o, "duration") {
                            Some(d) => micros(d).max(SEC / 30),
                            None => ((c0.dur as f64) * c0.speed as f64 / speed as f64).round().max((SEC / 30) as f64) as view::Time,
                        };
                        let delta = dur - c0.dur;
                        let trim = |s: &mut Self| -> Result<(), String> {
                            if delta == 0 {
                                return Ok(());
                            }
                            let seq = s.doc.seq();
                            let ok = s.doc.apply("Clip length", m::Edit::Trim { seq, clip: m::ClipId(id as u64), edge: m::Edge::End, delta, ripple: true });
                            s.check(ok, "").map(|_| ()).map_err(|e| if e.contains("SourceOutOfRange") { format!("clip {id} would need more footage than its file has after its source_in; shorten it or slip it earlier ({e})") } else { e })
                        };
                        let pace = |s: &mut Self| -> Result<(), String> {
                            if (speed - c0.speed).abs() < 1e-3 {
                                return Ok(());
                            }
                            let ok = s.doc.edit("Speed", |p| {
                                if let Some(c) = p.clip_mut(id) {
                                    c.speed = speed;
                                }
                            });
                            s.check(ok, "").map(|_| ())
                        };
                        // Shorter: cut first, then speed up. Longer: slow
                        // down first, then extend. Either way the source
                        // used stays inside the file.
                        if delta < 0 {
                            trim(self)?;
                            pace(self)?;
                        } else {
                            pace(self)?;
                            trim(self)?;
                        }
                        if let Some(m) = o.as_object_mut() {
                            m.remove("speed");
                            m.remove("duration");
                        }
                    }
                }
                // Fields set does not name itself go through the property
                // table (crop, anchor, effect and mask settings, blend…).
                let legacy = args::op_fields("set").unwrap_or(&[]);
                let extra: serde_json::Map<String, Value> = o.as_object().map(|m| m.iter().filter(|(k, _)| !legacy.contains(&k.as_str()) && k.as_str() != "op" && !args::is_note(k)).map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default();
                let mut extra: serde_json::Map<String, Value> = extra.into_iter().filter(|(k, _)| k != "backing" && k != "clips" && !craft::TEXT_MOTION_FIELDS.contains(&k.as_str())).collect();
                // A colour card or drawn box takes its colour as a property
                // (so it can be keyed too), not as a title's text colour.
                let drawn = self.model_clip(id).is_some_and(|mc| matches!(&mc.source, studio_model::ClipSource::Generator(g) if matches!(g.kind, studio_model::GeneratorKind::Solid | studio_model::GeneratorKind::Shape)));
                if drawn {
                    if let Some(v) = op.get("colour").or_else(|| op.get("color")) {
                        extra.insert("colour".into(), v.clone());
                    }
                }
                // An outline: a drawn box's stroke colour and width (pixels).
                for k in ["stroke", "stroke_width"] {
                    if let Some(v) = op.get(k) {
                        extra.insert(k.into(), v.clone());
                    }
                }
                // A colour without a width would not show: 4 px, as a card's.
                if op.get("stroke").is_some()
                    && op.get("stroke_width").is_none()
                    && self.model_clip(id).is_some_and(|mc| matches!(&mc.source, studio_model::ClipSource::Generator(g) if !g.params.get("stroke_width").is_some_and(|w| w.as_f64().is_some_and(|w| w > 0.0))))
                {
                    extra.insert("stroke_width".into(), json!(4.0));
                }
                let motion_said = self.text_motion(id, op)?;
                let extra_done = self.set_properties(id, &extra)?;
                let o = o;
                let fit = match op.get("fit").and_then(Value::as_str) {
                    Some(mode) => {
                        let c = self.clip(id)?;
                        let mi = c.media.and_then(|m| self.doc.project.media(m)).cloned().ok_or("fit needs a clip with picture media")?;
                        Some(craft::fit_scale(&self.doc.project, &mi, mode)?)
                    }
                    None => None,
                };
                let colour = match op.get("colour").or_else(|| op.get("color")).and_then(Value::as_str).filter(|_| !drawn) {
                    Some(s) => Some(craft::colour(s).ok_or_else(|| format!("colour \"{s}\" is not #rrggbb or a basic name"))?),
                    None => None,
                };
                let style = match op.get("style").and_then(Value::as_str) {
                    Some(name) => {
                        if self.clip(id)?.kind != ClipKind::Title {
                            return Err(format!("clip {id} is not a title; style is for titles"));
                        }
                        let index = self.resolve(name, &[Panel::Text])?.1;
                        Some(studio_text::styles::catalogue()[index].id.clone())
                    }
                    None => None,
                };
                let ok = self.doc.edit("Change clip", |p| {
                    let Some(c) = p.clip_mut(id) else { return };
                    if let Some(st) = style { c.text_style = Some(st); }
                    if let Some(s) = fit { c.transform.scale = s; }
                    if let Some(k) = colour { c.text_colour = k; }
                    let g = |k: &str| o.get(k).and_then(Value::as_f64).map(|v| v as f32);
                    if let Some(v) = g("x") { c.transform.x = v; }
                    if let Some(v) = g("y") { c.transform.y = v; }
                    if let Some(v) = g("scale") { c.transform.scale = v; }
                    if let Some(v) = g("rotation") { c.transform.rotation = v; }
                    if let Some(v) = g("opacity") { c.transform.opacity = v.clamp(0.0, 100.0); }
                    if let Some(v) = g("volume") { c.audio.volume = v; }
                    if let Some(v) = g("pan") { c.audio.pan = v.clamp(-100.0, 100.0); }
                    if let Some(v) = g("fade_in") { c.audio.fade_in = v.max(0.0); }
                    if let Some(v) = g("fade_out") { c.audio.fade_out = v.max(0.0); }
                    if let Some(v) = g("speed") {
                        // Same footage, new pace: the clip gets shorter or
                        // longer instead of running out of source.
                        let new = (v * 100.0).clamp(1.0, 10_000.0);
                        if o.get("duration").is_none() {
                            c.dur = ((c.dur as f64) * c.speed as f64 / new as f64).round().max((SEC / 30) as f64) as view::Time;
                        }
                        c.speed = new;
                    }
                    // Exposure in stops (the Inspector shows 25 per stop).
                    if let Some(v) = g("exposure") { c.grade.exposure = v * 25.0; }
                    if let Some(v) = g("text_size") { c.extra.text_size = v.clamp(5.0, 300.0); }
                    if let Some(v) = g("contrast") { c.grade.contrast = v; }
                    if let Some(v) = g("saturation") { c.grade.saturation = v; }
                    if let Some(v) = g("temperature") { c.grade.temperature = v; }
                    if let Some(v) = g("tint") { c.grade.tint = v; }
                    if let Some(v) = o.get("start").and_then(Value::as_f64) { c.start = micros(v).max(0); }
                    if let Some(v) = o.get("duration").and_then(Value::as_f64) { c.dur = micros(v).max(SEC / 30); }
                    if let Some(t) = o.get("text").and_then(Value::as_str) { c.text = t.to_owned(); }
                    if let Some(t) = o.get("name").and_then(Value::as_str) { c.name = t.to_owned(); }
                    if let Some(b) = o.get("muted").and_then(Value::as_bool) { c.audio.enabled = !b; }
                });
                let mut extra_done = extra_done;
                if let Some(m) = motion_said {
                    extra_done.push(m);
                }
                // A value set on a property that has keys does nothing you
                // can see: say so.
                if let Some(mc) = self.model_clip(id) {
                    let keyed: Vec<&str> = [("scale", "transform.scale_x"), ("x", "transform.position"), ("y", "transform.position"), ("opacity", "transform.opacity"), ("rotation", "transform.rotation"), ("volume", "audio.volume_db"), ("pan", "audio.pan"), ("exposure", "grade.exposure"), ("contrast", "grade.contrast"), ("saturation", "grade.saturation"), ("temperature", "grade.temperature"), ("tint", "grade.tint")]
                        .iter()
                        .filter(|(k, path)| op.get(*k).is_some() && mc.keyframes.get(*path).is_some_and(|t| !t.keys.is_empty()))
                        .map(|(k, _)| *k)
                        .collect();
                    if !keyed.is_empty() {
                        extra_done.push(format!("note: {} animated, so its keys decide and this value does not show; change the keys (animate, or keys) instead", keyed.join(", ")));
                    }
                }
                if let Some(spec) = op.get("backing").filter(|v| !matches!(v, Value::Bool(false) | Value::Null)) {
                    extra_done.push(format!("backing clip {}", self.backing(id, spec)?));
                } else if op.get("backing") == Some(&Value::Bool(false)) {
                    self.no_backing(id)?;
                    extra_done.push("no backing".into());
                }
                let msg = if extra_done.is_empty() { format!("changed clip {id}") } else { format!("changed clip {id} ({})", extra_done.join(", ")) };
                self.check(ok || !extra_done.is_empty(), &msg)
            }
            "apply" => {
                let item = op.get("item").and_then(Value::as_str).ok_or("apply needs \"item\" (a name from catalogue)")?;
                let clips = ids(op, "clips");
                if clips.is_empty() {
                    return Err("apply needs \"clips\"".into());
                }
                let panels: Vec<Panel> = match op.get("panel").and_then(Value::as_str).and_then(panel_of) {
                    Some(p) => vec![p],
                    None => vec![Panel::Effects, Panel::Looks, Panel::Motions, Panel::Text, Panel::Cuts],
                };
                let (panel, index, found) = self.resolve(item, &panels)?;
                if panel == Panel::Cuts {
                    let mut out = Vec::new();
                    for &c in &clips {
                        out.push(self.transition(&json!({"clip": c, "item": found, "duration": op.get("duration").cloned().unwrap_or(json!(0.5)), "params": op.get("params").cloned().unwrap_or(json!({}))}))?);
                    }
                    return Ok(out.join("; "));
                }
                let r = services::library::apply(&mut self.doc, panel, index, &clips).map_err(|e| e.0)?;
                // Transition length, effect strength.
                if panel == Panel::Cuts {
                    if let Some(d) = f(op, "duration") {
                        let cl = clips.clone();
                        self.doc.edit("Transition length", |p| {
                            for t in p.transitions.iter_mut().filter(|t| cl.contains(&t.left)) {
                                t.dur = micros(d).max(SEC / 30);
                            }
                        });
                    }
                }
                if let Some(Value::Object(params)) = op.get("params") {
                    let cl = clips.clone();
                    let params: Vec<(String, f32)> = params.iter().filter_map(|(k, v)| v.as_f64().map(|x| (k.clone(), x as f32))).collect();
                    self.doc.edit("Effect settings", |p| {
                        for c in p.clips.iter_mut().filter(|c| cl.contains(&c.id)) {
                            if let Some(e) = c.effects.last_mut() {
                                for (k, v) in &params {
                                    match e.params.iter_mut().find(|(n, _)| n == k) {
                                        Some(slot) => slot.1 = *v,
                                        None => e.params.push((k.clone(), *v)),
                                    }
                                }
                            }
                        }
                    });
                }
                let said = if found.to_lowercase() == item.trim().to_lowercase() { r.message } else { format!("{} (matched \"{item}\" to {found})", r.message) };
                // A key effect says what it takes (the settings, and the right tool for a plain backdrop).
                Ok(match (panel == Panel::Effects).then(|| keybg::key_hint(&found)).flatten() {
                    Some(h) => format!("{said}. {h}"),
                    None => said,
                })
            }
            "add" => {
                let query = op.get("query").and_then(Value::as_str);
                let item = op.get("item").and_then(Value::as_str).or(query).ok_or("add needs \"item\" (a name from catalogue) or \"query\" (words: takes an unused sound or track that fits)")?;
                // By words: the best sound or track the project does not play yet.
                let mut picked: Option<(Panel, usize, String)> = None;
                if let (None, Some(q)) = (op.get("item"), query) {
                    let c = shelf::content().ok_or("the content library is not loaded")?;
                    let ask = inventory::Ask {
                        kind: op.get("kind").and_then(Value::as_str).map(str::to_owned),
                        mood: op.get("mood").and_then(Value::as_str).map(str::to_owned),
                        min_secs: f(op, "min_seconds"),
                        max_secs: f(op, "max_seconds"),
                        ..inventory::Ask::from_words(q)
                    };
                    let facts = self.library_facts();
                    let i = inventory::pick(&c.entries, &facts, &ask, &|w| related(w), &|w| sounds::wanted(w), op.get("same").and_then(Value::as_bool).unwrap_or(false)).ok_or_else(|| format!("no sound or track matches \"{q}\"; catalogue {{panel: audio}} lists the kinds, moods and categories"))?;
                    picked = Some((Panel::Audio, i, c.entries[i].name.clone()));
                }
                let at = micros(f(op, "at").unwrap_or(0.0));
                let lane = match op.get("lane") { Some(v) => Some(craft::lane_of(&self.doc.project, v, None)?), None => None };
                let panels: Vec<Panel> = match op.get("panel").and_then(Value::as_str).and_then(panel_of) {
                    Some(p) => vec![p],
                    None => vec![Panel::Media, Panel::Audio, Panel::Assets, Panel::Text, Panel::Effects, Panel::Looks],
                };
                // The same name in two panels (Fire the emoji, Fire the
                // sound): what the op asks for decides, or it asks.
                let panels = if op.get("panel").is_none() && picked.is_none() {
                    let exact: Vec<Panel> = panels.iter().copied().filter(|&p| services::library::search(&self.doc, p, 0, item).iter().any(|&i| services::library::item(&self.doc, p, i as usize).name.eq_ignore_ascii_case(item.trim()))).collect();
                    if exact.len() > 1 {
                        let picture = ["x", "y", "scale", "fit", "opacity", "rotation"].iter().any(|k| op.get(*k).is_some()) || lane.and_then(|l| self.doc.project.lane(l)).is_some_and(|l| l.kind != view::LaneKind::Audio);
                        let sound = op.get("volume").is_some() || lane.and_then(|l| self.doc.project.lane(l)).is_some_and(|l| l.kind == view::LaneKind::Audio);
                        match (picture, sound) {
                            (true, false) => exact.into_iter().filter(|p| *p != Panel::Audio).collect(),
                            (false, true) => vec![Panel::Audio],
                            _ => return Err(format!("\"{item}\" is in {}; say which with \"panel\"", exact.iter().map(|p| panel_name(*p)).collect::<Vec<_>>().join(" and "))),
                        }
                    } else {
                        panels
                    }
                } else {
                    panels
                };
                let (panel, index, found) = match picked {
                    Some(x) => x,
                    None => self.resolve(item, &panels)?,
                };
                // A sound or track the project already plays: say so, with others to take.
                let repeat = match shelf::content() {
                    Some(c) if panel == Panel::Audio && panel.uses_content() && op.get("same").is_none() => inventory::repeat_note(&c.entries, &self.library_facts(), index, &|w| related(w), &|w| sounds::wanted(w)),
                    _ => None,
                };
                let r = services::library::add(&mut self.doc, panel, index, at, lane).map_err(|e| e.0)?;
                let mut notes = Vec::new();
                let mut all_ids: Vec<u32> = r.select.clone();
                if let (Some(d), Some(&id)) = (f(op, "duration"), r.select.first()) {
                    let c = self.clip(id)?;
                    let want = micros(d).max(SEC / 30);
                    let natural = c.media.and_then(|m| self.doc.project.media(m)).filter(|mi| mi.kind != MediaKind::Image).map(|mi| mi.dur);
                    match natural {
                        // Longer than the item (a 4 s motion graphic for 12 s):
                        // it loops, copy after copy on the same lane.
                        Some(len) if want > len && len > 0 => {
                            let mut t = c.start + len;
                            let end = c.start + want;
                            while t < end {
                                let piece = (end - t).min(len);
                                let copy_id = self.doc.alloc_id() as u32;
                                let ok = self.doc.edit("Loop", |p| {
                                    let mut copy = c.clone();
                                    copy.id = copy_id;
                                    copy.start = t;
                                    copy.dur = piece;
                                    p.clips.push(copy);
                                    p.resolve_overlap(copy_id);
                                });
                                self.check(ok, "")?;
                                all_ids.push(copy_id);
                                t += len;
                            }
                            notes.push(format!("looped to {d}s"));
                        }
                        _ => {
                            let ok = self.doc.edit("Clip length", |p| {
                                if let Some(c) = p.clip_mut(id) {
                                    c.dur = want;
                                }
                            });
                            self.check(ok, "")?;
                        }
                    }
                }
                // Where and how big, in the same op.
                let place_fields: serde_json::Map<String, Value> = op.as_object().map(|o| o.iter().filter(|(k, _)| matches!(k.as_str(), "x" | "y" | "scale" | "fit" | "opacity" | "rotation" | "volume" | "colour")).map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default();
                if !place_fields.is_empty() {
                    for id in all_ids.clone() {
                        let mut s = place_fields.clone();
                        s.insert("op".into(), json!("set"));
                        s.insert("clip".into(), json!(id));
                        self.op(&Value::Object(s))?;
                    }
                }
                Ok(format!("{} ({found}) as clip {:?}{}{}", r.message, r.select, if notes.is_empty() { String::new() } else { format!(" ({})", notes.join(", ")) }, repeat.map_or(String::new(), |n| format!(". Note: {n}"))))
            }
            "text" => {
                let text = op.get("text").and_then(Value::as_str).ok_or("text needs \"text\"")?.to_owned();
                let at = micros(f(op, "at").unwrap_or(0.0));
                let dur = micros(f(op, "duration").unwrap_or(3.0)).max(SEC / 10);
                let index = match op.get("style").and_then(Value::as_str) {
                    Some(s) => self.resolve(s, &[Panel::Text])?.1,
                    None => 0,
                };
                let lane = match op.get("lane") { Some(v) => Some(craft::lane_of(&self.doc.project, v, None)?), None => None };
                let r = services::library::add(&mut self.doc, Panel::Text, index, at, lane).map_err(|e| e.0)?;
                let id = *r.select.first().ok_or("the title was not added")?;
                let y = f(op, "y").map(|v| v as f32);
                // Phone-readable unless asked otherwise.
                let size = Some(f(op, "size").map(|v| (v as f32).clamp(5.0, 300.0)).unwrap_or_else(|| self.doc.project.clip(id).map_or(72.0, |c| c.extra.text_size.max(72.0))));
                let colour = match op.get("colour").or_else(|| op.get("color")).and_then(Value::as_str) {
                    Some(s) => Some(craft::colour(s).ok_or_else(|| format!("colour \"{s}\" is not #rrggbb or a basic name"))?),
                    None => None,
                };
                self.doc.edit("Title text", |p| {
                    if let Some(c) = p.clip_mut(id) {
                        if let Some(s) = size { c.extra.text_size = s; }
                        if let Some(k) = colour { c.text_colour = k; }
                        c.text = text.clone();
                        c.name = text.chars().take(24).collect();
                        c.dur = dur;
                        if let Some(y) = y {
                            c.transform.y = y;
                        }
                    }
                });
                if let Some(x) = f(op, "x") {
                    self.doc.edit("Title place", |p| {
                        if let Some(c) = p.clip_mut(id) {
                            c.transform.x = x as f32;
                        }
                    });
                }
                let motion = self.text_motion(id, op)?;
                if op.get("backing") == Some(&Value::Bool(false)) {
                    self.no_backing(id)?;
                }
                if let Some(spec) = op.get("backing").filter(|v| !matches!(v, Value::Bool(false) | Value::Null)) {
                    let b = self.backing(id, spec)?;
                    return Ok(format!("title clip {id}, backing clip {b}"));
                }
                self.check(true, &format!("title clip {id}{}", motion.map(|m| format!(" ({m})")).unwrap_or_default()))
            }
            "detach_audio" => {
                let id = op.get("clip").and_then(Value::as_u64).ok_or("detach_audio needs \"clip\"")?;
                let new_clip = m::ClipId(self.doc.alloc_id());
                let ok = self.doc.apply("Detach audio", m::Edit::DetachAudio { seq, clip: m::ClipId(id), new_clip });
                self.check(ok, "detach audio")?;
                // Independent by default: picture and sound can then be moved,
                // trimmed or deleted apart.
                if !op.get("linked").and_then(Value::as_bool).unwrap_or(false) {
                    let ok = self.doc.apply("Unlink", m::Edit::Unlink { seq, clips: vec![m::ClipId(id), new_clip] });
                    self.check(ok, "unlink")?;
                }
                Ok(format!("audio of clip {id} is now clip {}", new_clip.0))
            }
            "unlink" => {
                let clips = ids(op, "clips");
                let ok = self.doc.apply("Unlink", m::Edit::Unlink { seq, clips: clips.iter().map(|&c| m::ClipId(c as u64)).collect() });
                self.check(ok, "unlinked")
            }
            "slip" => {
                let id = op.get("clip").and_then(Value::as_u64).ok_or("slip needs \"clip\"")?;
                let by = f(op, "by").ok_or("slip needs \"by\" (seconds of source)")?;
                let ok = self.doc.apply("Slip", m::Edit::Slip { seq, clip: m::ClipId(id), delta: micros(by) });
                self.check(ok, &format!("slipped clip {id} by {by}s"))
            }
            "reframe" => {
                let clips = ids(op, "clips");
                if clips.is_empty() {
                    return Err("reframe needs \"clips\"".into());
                }
                let smoothing = f(op, "smoothing").unwrap_or(0.5) as f32;
                // Plan every clip at once (each decodes its own range).
                let view = self.doc.project.clone();
                let targets: Vec<view::Clip> = clips.iter().filter_map(|&c| view.clip(c).cloned()).collect();
                let plans: Vec<(u32, Result<Vec<(f64, f64, f64)>, String>)> = std::thread::scope(|sc| {
                    let hs: Vec<_> = targets.iter().map(|c| { let v = &view; sc.spawn(move || (c.id, reframe::plan(v, c, smoothing))) }).collect();
                    hs.into_iter().map(|h| h.join().unwrap_or((0, Err("reframe failed".into())))).collect()
                });
                let mut n = 0;
                let mut notes = Vec::new();
                for (id, plan) in plans {
                    match plan {
                        Ok(keys) => {
                            // Replace any earlier framing on this clip.
                            let old: Vec<view::Time> = self.doc.project.clip(id).and_then(|c| c.keys.iter().find(|(p, _)| p == "transform.position").map(|(_, ts)| ts.clone())).unwrap_or_default();
                            for t in old {
                                let ok = self.doc.apply("Reframe", m::Edit::RemoveKeyframe { seq, clip: m::ClipId(id as u64), property: "transform.position".into(), time: t });
                                self.check(ok, "clear framing")?;
                            }
                            for (t, x, y) in keys {
                                let key = m::Keyframe::new(micros(t), m::Value::Point(m::Vec2 { x, y }), m::Interp::Linear);
                                let ok = self.doc.apply("Reframe", m::Edit::SetKeyframe { seq, clip: m::ClipId(id as u64), property: "transform.position".into(), key });
                                self.check(ok, "reframe")?;
                                n += 1;
                            }
                        }
                        Err(e) => notes.push(format!("clip {id}: {e}")),
                    }
                }
                Ok(if notes.is_empty() { format!("{n} framing keys on {} clip(s)", clips.len()) } else { format!("{n} framing keys; {}", notes.join("; ")) })
            }
            "fx_layer" => {
                let item = op.get("item").and_then(Value::as_str).ok_or("fx_layer needs \"item\" (an effect or look)")?;
                let at = micros(f(op, "at").ok_or("fx_layer needs \"at\"")?);
                let dur = micros(f(op, "duration").unwrap_or(0.5)).max(SEC / 30);
                let lane = match op.get("lane") { Some(v) => Some(craft::lane_of(&self.doc.project, v, None)?), None => None };
                let (panel, index, found) = self.resolve(item, &[Panel::Effects, Panel::Looks])?;
                let r = services::library::add_adjustment(&mut self.doc, panel, index, at, dur, lane).map_err(|e| e.0)?;
                let id = *r.select.first().ok_or("the layer was not added")?;
                if let Some(Value::Object(params)) = op.get("params") {
                    let params: Vec<(String, f32)> = params.iter().filter_map(|(k, v)| v.as_f64().map(|x| (k.clone(), x as f32))).collect();
                    self.doc.edit("Effect settings", |p| {
                        if let Some(e) = p.clip_mut(id).and_then(|c| c.effects.last_mut()) {
                            for (k, v) in &params {
                                match e.params.iter_mut().find(|(n, _)| n == k) {
                                    Some(slot) => slot.1 = *v,
                                    None => e.params.push((k.clone(), *v)),
                                }
                            }
                        }
                    });
                }
                // "animate": {"<setting>": [[t, v], ...]} animates the layer's
                // effect in the same op.
                if let Some(Value::Object(anim)) = op.get("animate") {
                    let kind = self.clip(id)?.effects.last().map(|e| e.kind.clone()).unwrap_or_default();
                    for (setting, keys) in anim {
                        self.animate(&json!({"clip": id, "property": format!("{kind}.{setting}"), "keys": keys, "ease": "ease"}))?;
                    }
                }
                Ok(format!("{found} layer as clip {id}"))
            }
            "keyframe" => {
                let id = op.get("clip").and_then(Value::as_u64).ok_or("keyframe needs \"clip\"")?;
                let property = op.get("property").and_then(Value::as_str).ok_or("keyframe needs \"property\"")?.to_owned();
                let t = f(op, "time").ok_or("keyframe needs \"time\" (seconds into the clip)")?;
                let value = f(op, "value").ok_or("keyframe needs a numeric \"value\" (a number, or a string holding one)")?;
                let key = m::Keyframe::new(micros(t), m::Value::Scalar(value), m::Interp::Ease);
                let ok = self.doc.apply("Keyframe", m::Edit::SetKeyframe { seq, clip: m::ClipId(id), property, key });
                self.check(ok, "keyframe set")
            }
            "marker" => self.marker(op),
            "place" => self.place(op),
            "animate" => self.animate(op),
            "keys" => self.keys(op),
            "scene" => self.scene(op),
            "narrate" => self.narrate(op).map(|(note, id, words)| format!("{note}, clip {id}: {}", words.iter().map(|w| format!("{}@{:.2}", w.text, w.start)).collect::<Vec<_>>().join(" "))),
            "music" => self.music(op),
            "transition" => self.transition(op),
            "lane" => self.lane(op),
            "effect" => self.effect(op),
            "card" => self.card(op),
            "sfx" => self.sfx(op),
            "sing" => self.sing(op),
            "foley" => self.foley(op),
            "motion" => self.motion(op),
            "use_move" => self.use_move(op),
            "stagger" => self.stagger(op),
            "modifier" => self.modifier(op),
            "physics" => self.physics(op),
            "driver" => self.driver(op),
            "shape" | "text_3d" | "expression" | "time_remap" | "template" | "chart" => self.mograph(name, op),
            "flatten" | "unflatten" => self.flatten(op),
            "align" | "distribute" => self.align(op),
            "parent" => self.parent(op),
            "matte" => self.matte(op),
            "restack" => self.restack(op),
            "audio" => self.audio(op),
            "use_design" => self.use_design(op),
            "merge" | "unmerge" => self.merge(op),
            "mask" => self.mask(op),
            "morph" => self.morph_op(op),
            #[cfg(feature = "render")]
            "particles" => self.particles(op),
            #[cfg(feature = "render")]
            "living" => self.living(op),
            "puppet" => self.puppet(op),
            "lip_sync" => self.lip_sync(op),
            "skeleton" => self.skeleton(op),
            "scissor" => self.scissor(op),
            "key_background" => self.key_background(op),
            "polygon_mask" => self.polygon_mask(op),
            "pose" if self.is_rigged_model(op) => self.pose3d(op),
            "pose" => self.pose(op),
            "ik" => self.ik(op),
            "aim" => self.aim(op),
            "retarget" => self.retarget(op),
            "parts" => self.parts(op),
            "swap" => self.swap(op),
            "character" => self.character(op),
            #[cfg(feature = "render")]
            "mocap" => self.mocap(op),
            #[cfg(feature = "render")]
            "comp" => self.comp(op),
            #[cfg(feature = "render")]
            "keying" => self.keying(op),
            #[cfg(feature = "render")]
            "remove_object" => self.remove_object(op),
            #[cfg(feature = "render")]
            "clean_plate" => self.clean_plate(op),
            #[cfg(feature = "render")]
            "sky" => self.sky(op),
            #[cfg(feature = "render")]
            "wire" => self.wire(op),
            #[cfg(feature = "render")]
            "paint" => self.paint(op),
            #[cfg(feature = "render")]
            "depth_of_field" => self.depth_of_field(op),
            #[cfg(feature = "render")]
            "grain_match" => self.grain_match(op),
            #[cfg(feature = "render")]
            "lens" => self.lens(op),
            #[cfg(feature = "render")]
            "motion_blur" => self.motion_blur(op),
            #[cfg(feature = "render")]
            "matchmove" => self.matchmove(op),
            #[cfg(feature = "render")]
            "place_on_ground" => self.place_on_ground(op),
            #[cfg(feature = "render")]
            "export_camera" => self.export_camera(op),
            #[cfg(feature = "render")]
            "import_camera" => self.import_camera(op),
            #[cfg(feature = "render")]
            "track" => self.track(op),
            #[cfg(feature = "render")]
            "stabilize" => self.stabilize(op),
            #[cfg(feature = "render")]
            "cut_out" => self.cut_out(op),
            #[cfg(feature = "render")]
            "roto" => self.roto(op),
            #[cfg(feature = "render")]
            "match_colour" => self.match_colour(op),
            #[cfg(feature = "render")]
            "smooth_motion" => self.smooth_motion(op),
            #[cfg(feature = "render")]
            "shadow" => self.shadow(op),
            #[cfg(feature = "render")]
            "light_match" => self.light_match(op),
            #[cfg(feature = "render")]
            "foreground" => self.foreground(op),
            "mask_puppet" => self.mask_puppet(op),
            "corner_pin" => self.corner_pin(op),
            "three_d" => self.three_d(op),
            "camera" => self.camera(op),
            "light" => self.light(op),
            "atmosphere" => self.atmosphere(op),
            "model" => self.model(op),
            "asset" => self.asset(op),
            "build_place" => self.build_place(op),
            "combine" => self.combine(op),
            "uncombine" => self.uncombine(op),
            "mesh" => self.mesh(op),
            "rig" => self.rig(op),
            "wear" => self.wear(op),
            "hold" => self.hold(op),
            "act" => self.act(op),
            "simulate" => self.simulate(op),
            "body" => self.body_op(op),
            "joint" => self.joint(op),
            "drop" => self.drop_op(op),
            "throw" => self.throw_op(op),
            "push" => self.push_op(op),
            "break" => self.break_op(op),
            "settle" => self.settle_op(op),
            "ragdoll" => self.ragdoll_op(op),
            "jiggle" => self.jiggle_op(op),
            "bake_physics" => self.bake_physics(op),
            "physics_world" => self.physics_world(op),
            #[cfg(feature = "render")]
            "world" => self.world(op),
            #[cfg(feature = "render")]
            "render3d" => self.render3d(op),
            #[cfg(feature = "render")]
            "build_world" => self.build_world(op),
            #[cfg(feature = "render")]
            "edit_world" => self.edit_world(op),
            #[cfg(feature = "render")]
            "walk" => self.walk(op),
            #[cfg(feature = "render")]
            "stand" => self.stand(op),
            #[cfg(feature = "render")]
            "world_info" => self.world_info(op),
            "parallax" => self.parallax(op),
            "shot" => self.shot(op),
            "lighting" => self.lighting(op),
            "previs" => self.previs(op),
            "roll" | "slide" | "duplicate" | "freeze" | "replace" | "reorder" | "group" | "ungroup" | "link" | "copy_look" => self.editing(name, op),
            n if editorial::is_op(n) => self.editorial(n, op),
            "raw" => {
                let e = op.get("edit").cloned().ok_or("raw needs \"edit\" (see edit_schema)")?;
                let edit: m::Edit = serde_json::from_value(e).map_err(|e| format!("not a model edit: {e}"))?;
                let ok = self.doc.apply("Edit", edit);
                self.check(ok, "applied")
            }
            other => Err(format!("unknown op \"{other}\"; ops are place, split, trim, delete, move, set, animate, keys, transition, lane, effect, card, sfx, motion, merge, unmerge, flatten, unflatten, align, distribute, parent, matte, restack, audio, use_move, use_design, stagger, modifier, physics, driver, expression, time_remap, shape, text_3d, template, chart, mask, particles, living, puppet, mask_puppet, corner_pin, three_d, camera, light, parallax, shot, lighting, previs, roll, slide, duplicate, freeze, replace, reorder, group, ungroup, link, copy_look, apply, add, text, fx_layer, detach_audio, unlink, slip, reframe, keyframe, marker, model, mesh, asset, build_place, combine, uncombine, rig, wear, hold, pose, act, simulate, body, joint, drop, throw, push, break, settle, ragdoll, jiggle, bake_physics, physics_world, world, render3d, build_world, edit_world, walk, stand, world_info, shadow, light_match, foreground, raw")),
        }
    }

    // ------------------------------------------------------------ AI jobs

    fn run_ai(&mut self, action: &str, clips: &[u32]) -> Result<Value, String> {
        use services::ai_run::Action;
        let a = match action {
            "captions" | "transcribe" | "auto_captions" => Action::Captions,
            "voice_cleanup" | "clean_voice" | "denoise" => Action::VoiceCleanup,
            "beat_markers" | "beats" => Action::BeatMarkers,
            other => return Err(format!("unknown AI action \"{other}\"; use captions, voice_cleanup or beat_markers")),
        };
        let before: BTreeMap<u32, Value> = self.doc.project.clips.iter().map(|c| (c.id, self.clip_line(c))).collect();
        let started = self.ai.start(&self.doc, a, clips).map_err(|e| e.0)?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1800);
        loop {
            if let Some(r) = self.ai.poll(&mut self.doc).into_iter().next() {
                return match r {
                    Ok(o) => Ok(json!({"ok": true, "started": started, "message": o.message, "changed": self.changes(&before), "revision": self.doc.history.revision()})),
                    Err(e) => Err(e.0),
                };
            }
            if std::time::Instant::now() > deadline {
                self.ai.cancel_all();
                return Err("the AI job took over 30 minutes and was cancelled".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
    }

    // --------------------------------------------------------------- tools

    /// Runs one tool. Errors are worded for the agent and never leave a
    /// half-applied change behind.
    pub fn call(&mut self, tool: &str, a: &Value) -> Result<Vec<Content>, String> {
        self.call_depth += 1;
        let mut r = self.call_tool(tool, a);
        self.call_depth = self.call_depth.saturating_sub(1);
        // Pictures finished in the background land now and are announced.
        if self.call_depth == 0 && tool != "generate" {
            if let (Some(note), Ok(parts)) = (self.pictures_note(), r.as_mut()) {
                parts.push(Content::Text(note.to_string()));
            }
            if let (Some(note), Ok(parts)) = (self.models_note(), r.as_mut()) {
                parts.push(Content::Text(note.to_string()));
            }
        }
        r
    }

    fn call_tool(&mut self, tool: &str, a: &Value) -> Result<Vec<Content>, String> {
        let text = |v: Value| Ok(vec![Content::Text(v.to_string())]);
        match tool {
            "capabilities" => Ok(vec![Content::Text(capabilities::tool(a)?)]),
            "fonts" => fonts::tool(a),
            "parts_check" => self.parts_check(a),
            "taste" => Ok(vec![Content::Text(taste::tool(self.home.clone().or_else(|| self.doc.folder.as_ref().map(|f| f.root().to_path_buf())).as_deref(), a)?)]),
            "plan" => Ok(vec![Content::Text(crew::headless_plan(a)?)]),
            "howto" => text(crate::help::answer(a.get("query").and_then(Value::as_str).unwrap_or(""), a.get("limit").and_then(Value::as_u64).unwrap_or(3) as usize)),
            "guide" => Ok(vec![Content::Text(guide_answer(a)?)]),
            "skill" => Ok(vec![Content::Text(skills::tool(a)?)]),
            "assets" | "project_care" => self.care_call(tool, a),
            "listen" => text(self.listen(a)?),
            "capture" => self.capture(a),
            "graph" => text(self.graph_tool(a)?),
            "editorial" => text(self.editorial_tool(a)?),
            "search" => {
                let q = a.get("query").and_then(Value::as_str).unwrap_or("");
                let kinds: Vec<String> = a.get("kinds").and_then(Value::as_array).map(|k| k.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default();
                let limit = a.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
                let mut v = crate::library::search(q, &kinds, limit, self.doc.folder.as_ref().map(|f| f.root()))?;
                // The content library is searched too: sounds, music, pictures, motions.
                if kinds.is_empty() || kinds.iter().any(|k| k == "library") {
                    let items = self.library_hits(q, if kinds.is_empty() { 6 } else { limit.min(40) });
                    if !items.is_empty() {
                        v["library_items"] = json!(items);
                    }
                }
                text(v)
            }
            "plugins" => self.plugins_tool(a),
            "automation" => self.automation_tool(a),
            // The window runs it (`editor::dock_ui::agent_tool`); a shell has no panels.
            "layout" => Err("layout arranges the Zornov window's panels; there is no window here".into()),
            "save_move" => text(self.save_move(a)?),
            "save_design" => text(self.save_design(a)?),
            "made" => text(self.made()),
            #[cfg(feature = "render")]
            "match" => self.match_frames(a),
            "script" => {
                let since = a.get("since").and_then(Value::as_u64);
                Ok(vec![Content::Text(self.script_since(since))])
            }
            #[cfg(feature = "render")]
            "see" => Ok(vec![Content::Text(self.see(a)?)]),
            #[cfg(feature = "render")]
            "objects" => Ok(vec![Content::Text(self.objects(a)?)]),
            "notes" => text(self.notes(a)?),
            "story" => text(self.story_tool(a)?),
            "comments" | "versions" | "share" | "team" => text(self.collab_tool(tool, a)?),
            "production" => text(self.production_tool(a)?),
            #[cfg(feature = "commerce")]
            "account" => text(self.account_tool(a)?),
            "check" => {
                if let Some(b) = a.get("brief").and_then(Value::as_str).filter(|b| !b.trim().is_empty()) {
                    self.brief = Some(b.to_owned());
                }
                let frames = a.get("frames").and_then(Value::as_bool).unwrap_or(true);
                let mut v = self.quality(frames);
                // With fix: Studio mends what it can, then looks again.
                if a.get("fix").and_then(Value::as_bool).unwrap_or(false) {
                    let probs = v["problems"].as_array().cloned().unwrap_or_default();
                    self.doc.begin_batch("Fix");
                    let fixed = self.fix(&probs);
                    self.doc.commit_batch();
                    if !fixed.is_empty() {
                        v = self.quality(frames);
                    }
                    v["fixed"] = json!(fixed);
                }
                text(v)
            }
            "review" => {
                if let Some(b) = a.get("brief").and_then(Value::as_str).filter(|b| !b.trim().is_empty()) {
                    self.brief = Some(b.to_owned());
                }
                let v = self.review(a);
                let passed = v["ok"] == true;
                let mut parts = text(v)?;
                // The last look before delivering is at full detail: a
                // review that passes sends the cut as a full-quality sheet.
                #[cfg(feature = "render")]
                if passed && a.get("frames").and_then(Value::as_bool) != Some(false) && self.doc.project.duration() > 0 {
                    let (w, q) = render::picture_size(&json!({"detail": "full"}), render::SHEET_WIDTH);
                    let dur = self.doc.project.duration();
                    if let Ok((jpeg, times)) = self.frames.sheet(&self.doc, 0, dur, 12, w as u32, q) {
                        parts.push(Content::Jpeg(jpeg));
                        parts.push(Content::Text(json!({"final_look": {"detail": "full", "times": times, "columns": render::sheet_columns(12)}}).to_string()));
                    }
                }
                #[cfg(not(feature = "render"))]
                let _ = passed;
                Ok(parts)
            }
            "summary" => match a.get("since").and_then(Value::as_u64) {
                // What changed since a revision you have already read.
                Some(rev) => text(self.summary_since(rev)),
                None => {
                    self.snapshot();
                    text(self.summary())
                }
            },
            "inspect" => {
                let id = a.get("clip").and_then(Value::as_u64).ok_or("inspect needs \"clip\"")? as u32;
                // A layer inside a merged clip: looked at in place.
                if self.doc.project.clip(id).is_none() {
                    if let Some(seq) = self.sequence_of(id as u64).filter(|s| *s != self.doc.seq()) {
                        return self.within(seq, |s| s.run("inspect", a));
                    }
                }
                let mut v = self.inspect(id)?;
                // Where it is on screen: at `at`, or the middle of the clip.
                let c = self.clip(id)?;
                let t = f(a, "at").map(micros).unwrap_or(c.start + c.dur / 2).clamp(c.start, c.start + c.dur);
                if let Ok(s) = self.on_screen(id, t) {
                    v["on_screen"] = s;
                    // What its pixels cover and what covers them, from the
                    // drawn alpha rather than the box.
                    if c.kind != ClipKind::Audio {
                        if let Some(l) = self.layout_of(id, t) {
                            v["layout"] = l;
                        }
                    }
                }
                if let Some(inside) = self.contents(id as u64) {
                    v["inside"] = inside;
                }
                if let Some(links) = self.links(id as u64) {
                    v["links"] = links;
                }
                // Where the bones, soles, hands and gaze of a rigged character are.
                if a.get("pose").is_some_and(|p| !matches!(p, Value::Bool(false) | Value::Null)) || a.get("bones").is_some() {
                    let mut ask = a.clone();
                    if ask.get("pose").is_none() {
                        ask["pose"] = json!(true);
                    }
                    v["pose"] = self.pose_report(id, &ask)?;
                }
                #[cfg(feature = "render")]
                if let Some(tr) = self.tracking_said(id) {
                    v["tracking"] = tr;
                }
                text(v)
            }
            "pick" => text(self.pick(a)?),
            "canvas_layout" => Ok(vec![Content::Text(self.layout_tool(a)?)]),
            "voices" => {
                let mut v = self.voices();
                // Most natural first; the long tail of compact voices is a
                // few thousand tokens nobody picks from.
                let all = a.get("all").and_then(Value::as_bool).unwrap_or(false);
                if let Some(list) = v.get_mut("voices").and_then(Value::as_array_mut).filter(|l| !all && l.len() > VOICES_LISTED) {
                    let more = list.len() - VOICES_LISTED;
                    list.truncate(VOICES_LISTED);
                    v["more"] = json!(format!("{more} more, all compact: voices with all: true lists them"));
                }
                text(v)
            }
            "media" => text(self.media()),
            "catalogue" => {
                // Several things at once, or across the library: each word's
                // pictures and sounds in one call.
                let words: Vec<String> = match a.get("query") {
                    Some(Value::Array(qs)) => qs.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
                    Some(Value::String(q)) if matches!(a.get("panel").and_then(Value::as_str), None | Some("all")) => q.split(',').map(|w| w.trim().to_owned()).filter(|w| !w.is_empty()).collect(),
                    _ => Vec::new(),
                };
                if !words.is_empty() {
                    let per = a.get("limit").and_then(Value::as_u64).unwrap_or(5) as usize;
                    let panels: Vec<Panel> = match a.get("panel").and_then(Value::as_str).filter(|p| *p != "all").and_then(panel_of) {
                        Some(p) => vec![p],
                        None => vec![Panel::Assets, Panel::Audio, Panel::Motions],
                    };
                    let mut found = serde_json::Map::new();
                    for w in &words {
                        let mut by = serde_json::Map::new();
                        for p in &panels {
                            let v = self.catalogue(*p, w, per);
                            let names: Vec<Value> = v["items"].as_array().into_iter().flatten().filter_map(|i| {
                                let n = i.get("name")?.as_str()?;
                                Some(match (i.get("kind").and_then(Value::as_str), *p) {
                                    (Some(k), Panel::Audio) => json!(format!("{n} ({k}{}{})", i.get("dur").and_then(Value::as_f64).map_or(String::new(), |d| format!(", {d}s")), if i.get("used_in_project").is_some() { ", used" } else { "" })),
                                    _ => json!(n),
                                })
                            }).collect();
                            if !names.is_empty() {
                                by.insert(panel_name(*p).to_owned(), Value::Array(names));
                            }
                        }
                        found.insert(w.clone(), if by.is_empty() { json!("nothing") } else { Value::Object(by) });
                    }
                    let mut v = json!({"found": found, "use": "add with item (and panel) puts one on the timeline"});
                    // Say so when the library itself is missing, not "nothing".
                    if crate::content::find_dir().is_none() {
                        v["note"] = json!("Zornov's content library was not found on this Mac, so only the built-in samples were searched (set STUDIO_CONTENT to its folder, or open Zornov.app once)");
                    }
                    return text(v);
                }
                let q = a.get("query").and_then(Value::as_str).unwrap_or("");
                let limit = a.get("limit").and_then(Value::as_u64).unwrap_or(40) as usize;
                // Sound and music narrowed by what they are: kind, mood, length.
                let ask = inventory::Ask {
                    kind: a.get("kind").and_then(Value::as_str).map(str::to_owned),
                    mood: a.get("mood").and_then(Value::as_str).map(str::to_owned),
                    min_secs: f(a, "min_seconds"),
                    max_secs: f(a, "max_seconds"),
                    fresh: a.get("fresh").and_then(Value::as_bool).unwrap_or(false),
                    ..inventory::Ask::from_words(q)
                };
                let extras = ask.kind.is_some() || ask.mood.is_some() || ask.min_secs.is_some() || ask.max_secs.is_some() || ask.fresh;
                let named = a.get("panel").and_then(Value::as_str).filter(|p| *p != "all").and_then(panel_of);
                let picture_kind = ask.kind.as_deref().and_then(studio_content::item::Kind::parse).filter(|k| k.is_image());
                let panel = match (named, extras) {
                    (Some(p), _) => p,
                    (None, true) if picture_kind.is_some() => Panel::Assets,
                    (None, true) => Panel::Audio,
                    // Nothing asked: what the library holds.
                    (None, false) => return text(self.library_overview()),
                };
                if panel == Panel::Audio && shelf::content().is_some() {
                    if q.trim().is_empty() && !extras {
                        let c = shelf::content().ok_or("the library is not loaded")?;
                        let mut v = json!({"panel": "audio", "inventory": inventory::audio_overview(&c.entries, &self.library_facts())});
                        v["varied_examples"] = self.sound_search(&inventory::Ask::default(), 8)["items"].clone();
                        v["use"] = json!("narrow with query words, kind (music, sound, hit, boom, swoosh, swell, long), mood (calm, happy, energetic, epic, dark, sad, retro), min_seconds, max_seconds, fresh (leave out what the project plays)");
                        return text(v);
                    }
                    // A few varied candidates, not the whole shelf.
                    let few = a.get("limit").and_then(Value::as_u64).unwrap_or(12) as usize;
                    return text(self.sound_search(&ask, few.min(40)));
                }
                // A few hundred names are not a choice: past this the answer
                // says how many more there are and how to narrow.
                let cap = limit.min(CATALOGUE_LIST_MAX);
                let mut v = self.catalogue(panel, q, cap);
                if limit > cap {
                    let total = v["total"].as_u64().unwrap_or(0);
                    if total > cap as u64 {
                        v["more"] = json!(format!("{} more not listed; narrow with query words, kind or mood, or page with a smaller limit", total - cap as u64));
                    }
                }
                // Settings are long: only when asked, or for a few items.
                let few = v["items"].as_array().is_some_and(|a| a.len() <= 5);
                if !a.get("details").and_then(Value::as_bool).unwrap_or(few) {
                    if let Some(items) = v["items"].as_array_mut() {
                        let mut had = false;
                        for it in items.iter_mut() {
                            if let Some(o) = it.as_object_mut() {
                                had |= o.remove("settings").is_some();
                            }
                        }
                        if had {
                            v["settings"] = json!("left out to keep this short: ask with details true, or narrow the query to a few effects");
                        }
                    }
                }
                text(v)
            }
            "edit" => {
                let ops: Vec<Value> = match a.get("ops") {
                    Some(Value::Array(v)) => v.clone(),
                    Some(v @ Value::Object(_)) => vec![v.clone()],
                    _ => return Err("edit needs \"ops\": a list of ops (see guide)".into()),
                };
                let label = a.get("label").and_then(Value::as_str).unwrap_or("Agent edit").to_owned();
                let dry = a.get("dry_run").and_then(Value::as_bool).unwrap_or(false);
                text(self.edit(&ops, &label, dry)?)
            }
            "undo" => text(json!({"undone": self.doc.undo(), "revision": self.doc.history.revision()})),
            "redo" => text(json!({"redone": self.doc.redo(), "revision": self.doc.history.revision()})),
            "new_project" => {
                let name = a.get("name").and_then(Value::as_str).unwrap_or("Untitled");
                let w = a.get("width").and_then(Value::as_u64).unwrap_or(1920) as u32;
                let h = a.get("height").and_then(Value::as_u64).unwrap_or(1080) as u32;
                let fps = f(a, "fps").unwrap_or(30.0) as f32;
                *self = Session::new(services::project::new_custom(name, w.max(16), h.max(16), fps.max(1.0)));
                text(self.summary())
            }
            "open" => {
                let path = a.get("path").and_then(Value::as_str).ok_or("open needs \"path\" (a project folder)")?;
                let doc = services::project::open(std::path::Path::new(path))?;
                *self = Session::new(doc);
                text(self.summary())
            }
            "save" => {
                let path = a.get("path").and_then(Value::as_str).map(PathBuf::from);
                let saved = services::project::save(&mut self.doc, path.as_deref())?;
                text(json!({"saved": saved.display().to_string()}))
            }
            "import" => {
                let paths: Vec<PathBuf> = match a.get("paths") {
                    Some(Value::Array(v)) => v.iter().filter_map(Value::as_str).map(PathBuf::from).collect(),
                    Some(Value::String(s)) => vec![PathBuf::from(s)],
                    _ => return Err("import needs \"paths\"".into()),
                };
                // Links are downloaded and names found in the project's folders;
                // 3D models become model clips (the model op), not media.
                let mut found = Vec::new();
                for p in &paths {
                    found.push(self.input_file(&p.to_string_lossy())?);
                }
                let (models, paths): (Vec<PathBuf>, Vec<PathBuf>) = found.into_iter().partition(|p| download::is_model(p));
                if !models.is_empty() {
                    let mut out = json!({"models": []});
                    for m in &models {
                        let r = self.edit(&[json!({"op": "model", "file": m.display().to_string()})], "import", false).map_err(|e| format!("{}: {e}", m.display()))?;
                        out["models"].as_array_mut().expect("array").push(json!({"file": m.display().to_string(), "added_as": "3D model clip", "result": r}));
                    }
                    if !paths.is_empty() {
                        let mut b = a.clone();
                        b["paths"] = json!(paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>());
                        let parts = self.call("import", &b)?;
                        if let Some(Value::Object(rest)) = parts.iter().find_map(|p| if let Content::Text(t) = p { serde_json::from_str(t).ok() } else { None }) {
                            for (k, v) in rest {
                                out[k] = v;
                            }
                        }
                    }
                    return text(out);
                }
                // Photoshop documents layer by layer (see psd.rs); the rest
                // as usual.
                if a.get("layers").and_then(Value::as_bool).unwrap_or(false) && paths.iter().any(|p| crate::psd_import::is_psd(p)) {
                    let (psds, rest): (Vec<PathBuf>, Vec<PathBuf>) = paths.into_iter().partition(|p| crate::psd_import::is_psd(p));
                    let mut out = Vec::new();
                    for p in &psds {
                        out.push(self.import_psd_layers(p, a)?);
                    }
                    if !rest.is_empty() {
                        let mut b = a.clone();
                        b["paths"] = json!(rest.iter().map(|p| p.display().to_string()).collect::<Vec<_>>());
                        b["layers"] = json!(false);
                        let parts = self.call("import", &b)?;
                        out.extend(parts.iter().filter_map(|p| if let Content::Text(t) = p { serde_json::from_str(t).ok() } else { None }));
                    }
                    return text(if out.len() == 1 { out.remove(0) } else { json!(out) });
                }
                // Files only a plugin reads go to its importer.
                if let Some(r) = self.import_by_plugins(&paths, a)? {
                    return Ok(r);
                }
                let to_timeline = a.get("to_timeline").and_then(Value::as_bool).unwrap_or(false);
                // Formats Studio cannot decode are converted first.
                let mut converted = Vec::new();
                let paths: Vec<PathBuf> = paths
                    .into_iter()
                    .map(|p| match crate::convert::ensure_playable(&p) {
                        Ok(q) => {
                            if q != p {
                                converted.push(json!({"from": p.display().to_string(), "to": q.display().to_string()}));
                            }
                            q
                        }
                        Err(_) => p,
                    })
                    .collect();
                let before: BTreeMap<u32, Value> = self.doc.project.clips.iter().map(|c| (c.id, self.clip_line(c))).collect();
                let (report, clips) = if to_timeline {
                    services::project::import_to_timeline(&mut self.doc, &paths)?
                } else {
                    (services::project::import_media(&mut self.doc, &paths), Vec::new())
                };
                text(json!({"media": report.ids, "failed": report.failed, "converted": converted, "clips": clips, "changed": self.changes(&before), "revision": self.doc.history.revision()}))
            }
            "make_playable" => {
                // Media already in the project in a format Studio cannot
                // decode: converted and relinked, clips untouched.
                let only: Vec<u64> = a.get("media").and_then(Value::as_array).map(|v| v.iter().filter_map(craft::as_id).collect()).unwrap_or_default();
                let mut done = Vec::new();
                let mut failed = Vec::new();
                for (id, path) in crate::convert::unplayable(&self.doc) {
                    if !only.is_empty() && !only.contains(&id) {
                        continue;
                    }
                    match crate::convert::ensure_playable(&path).and_then(|p| crate::convert::relink(&mut self.doc, id, &p).map(|_| p)) {
                        Ok(p) => done.push(json!({"media": id, "file": p.display().to_string()})),
                        Err(e) => failed.push(json!({"media": id, "error": e})),
                    }
                }
                text(json!({"made_playable": done, "failed": failed, "note": if done.is_empty() && failed.is_empty() { "every media file already plays" } else { "" }}))
            }
            "generate" => text(self.generate(a)?),
            "picture" => self.picture(a),
            "vector" | "illustrate" => self.vector_tool(a),
            #[cfg(feature = "render")]
            "enhance" => text(self.enhance(a)?),
            #[cfg(test)]
            "__slow" | "__block" => {
                // A gate holds the call until the test opens it: a test that
                // checks what happens while a call runs must not race a timer.
                if let Some(g) = a.get("gate").and_then(Value::as_u64) {
                    while !test_gate::is_open(g) {
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    }
                }
                // A long fake tool ends early when Stop cancels its job.
                let until = std::time::Instant::now() + std::time::Duration::from_millis(a.get("ms").and_then(Value::as_u64).unwrap_or(0));
                while std::time::Instant::now() < until {
                    if jobs::cancelled() {
                        return Err("stopped".into());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                let ops: Vec<Value> = a.get("edit").and_then(Value::as_array).cloned().unwrap_or_default();
                let r = self.edit(&ops, "slow", false)?;
                text(r)
            }
            "read" => text(web::read(a)?),
            "search_media" => self.search_media_tool(a),
            // Background jobs live in the window; here every call is waited for.
            "job" => text(json!({"jobs": [], "note": "calls run to the end here: there are no background jobs outside the Zornov window"})),
            "download" => {
                let url = a.get("url").and_then(Value::as_str).ok_or("download needs \"url\": a link to a video, sound or picture file, or to a web page whose video is openly available")?;
                // Only the public web (tests fetch from a local server).
                if !cfg!(test) {
                    web::refuse_private(url)?;
                }
                let home = self.home.clone().or_else(|| self.doc.folder.as_ref().map(|f| f.root().to_path_buf()));
                let dir = download::folder_for(home.as_deref());
                let pick = download::Pick { quality: a.get("quality").and_then(Value::as_str).map(str::to_owned), language: a.get("language").and_then(Value::as_str).map(str::to_owned) };
                let fetched = download::fetch_pick(url, &dir, a.get("name").and_then(Value::as_str), &pick)?;
                let file = fetched.file.clone();
                // A 3D model (glb, gltf, obj, fbx, stl, ply...): a model clip,
                // not a video.
                if download::is_model(&file) {
                    let mut op = json!({"op": "model", "file": file.display().to_string()});
                    if let Some(n) = a.get("name").and_then(Value::as_str) {
                        op["name"] = json!(n.rsplit_once('.').filter(|(_, e)| studio_mesh::foreign::is_model_ext(&e.to_ascii_lowercase())).map_or(n, |(s, _)| s));
                    }
                    for k in ["size", "at", "duration", "lane", "position", "rotation"] {
                        if let Some(v) = a.get(k) {
                            op[k] = v.clone();
                        }
                    }
                    let r = self.edit(&[op], "download", false).map_err(|e| format!("downloaded {} but could not add it as a 3D model: {e}", file.display()))?;
                    let mut v = json!({"file": file.display().to_string(), "model": true, "added_as": "3D model clip", "result": r});
                    if !fetched.notes.is_empty() {
                        v["notes"] = json!(fetched.notes);
                    }
                    return text(v);
                }
                // Not media (a README, notes, a PDF): kept as a document, and
                // its words come back to read.
                if let Some(kind) = download::document_kind(&file) {
                    let file = download::settle_document(&file, kind);
                    let mut v = json!({"file": file.display().to_string(), "document": kind, "note": "not a video, sound or picture: kept in Downloads as a document (nothing was added to the project)"});
                    if kind == "text" {
                        let words = std::fs::read_to_string(&file).unwrap_or_else(|_| String::from_utf8_lossy(&std::fs::read(&file).unwrap_or_default()).into_owned());
                        let shown: String = words.chars().take(12_000).collect();
                        if shown.len() < words.len() {
                            v["text_truncated"] = json!(format!("the first 12000 of {} characters", words.chars().count()));
                        }
                        v["text"] = json!(shown);
                    }
                    return text(v);
                }
                // Formats Studio cannot decode (AV1, VP9 from video sites) are
                // converted on arrival.
                let file = match crate::convert::ensure_playable(&file) {
                    Ok(f) => f,
                    Err(e) if fetched.via == "file" && !download::is_media_name(&file) => {
                        return text(json!({"file": file.display().to_string(), "kept_as_file": true, "note": "saved in Downloads as a file: it is not a video, sound, picture or 3D model Zornov opens, so nothing was added to the project", "import_said": e}));
                    }
                    Err(e) => return Err(e),
                };
                let to_timeline = a.get("to_timeline").and_then(Value::as_bool).unwrap_or(false);
                let parts = self.call("import", &json!({"paths": [file.display().to_string()], "to_timeline": to_timeline}))?;
                let mut v: Value = parts.iter().find_map(|p| if let Content::Text(t) = p { serde_json::from_str(t).ok() } else { None }).unwrap_or(json!({}));
                if v["media"].as_array().is_none_or(|m| m.is_empty()) {
                    let why = v["failed"].as_array().and_then(|f| f.first()).and_then(Value::as_str).map(str::to_owned).unwrap_or_default();
                    // A file the link named outright (an archive, a font, a
                    // colour table) is kept as it is; what a page handed over
                    // that is not media is not.
                    if fetched.via == "file" && !download::is_media_name(&file) {
                        return text(json!({"file": file.display().to_string(), "kept_as_file": true, "note": "saved in Downloads as a file: it is not a video, sound, picture or 3D model Zornov opens, so nothing was added to the project", "import_said": why}));
                    }
                    let _ = std::fs::remove_file(&file);
                    return Err(format!("What came from that link is not a video, sound or picture Zornov can open, so it was not kept.{}", if why.is_empty() { String::new() } else { format!(" ({why})") }));
                }
                v["file"] = json!(file.display().to_string());
                search_media::record_source(&file, &search_media::Hit { page: url.to_owned(), ..Default::default() });
                // What a page's playlist offered, and what was taken.
                if fetched.via != "file" {
                    v["from_page"] = json!({"via": fetched.via, "chosen": fetched.chosen, "other_versions": fetched.available, "ask_for_another": "download again with quality (best, lowest, audio, or a height such as 720) to take a different version"});
                }
                if !fetched.notes.is_empty() {
                    v["notes"] = json!(fetched.notes);
                }
                text(v)
            }
            "ai" => {
                let action = a.get("action").and_then(Value::as_str).ok_or("ai needs \"action\": captions, voice_cleanup or beat_markers")?;
                text(self.run_ai(action, &ids(a, "clips"))?)
            }
            "edit_schema" => schema::edit_schema(a),
            "analyse" | "analyze" => {
                let want: Vec<String> = match a.get("what") {
                    Some(Value::Array(v)) => v.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
                    Some(Value::String(s)) => vec![s.clone()],
                    _ => vec!["scenes".into(), "silences".into(), "beats".into()],
                };
                let want: Vec<&str> = want.iter().map(|s| match s.as_str() { "shots" | "shot_changes" | "cuts" => "scenes", "pauses" | "silence" => "silences", "beat" => "beats", o => o }).collect();
                let min = f(a, "min_pause").unwrap_or(0.6);
                let started = std::time::Instant::now();
                let mut v = self.analysis.run(&self.doc.project, &ids(a, "clips"), &want, min)?;
                v["seconds"] = json!((started.elapsed().as_secs_f64() * 100.0).round() / 100.0);
                text(v)
            }
            #[cfg(feature = "render")]
            "scopes" => self.scopes_tool(a),
            #[cfg(feature = "render")]
            "viewport" => self.viewport_tool(a),
            #[cfg(feature = "render")]
            "colour" => self.colour_tool(a),
            #[cfg(feature = "render")]
            "conform" => self.conform_tool(a),
            #[cfg(feature = "render")]
            "media_manage" => self.media_manage(a),
            #[cfg(feature = "render")]
            "frame" => {
                let t = f(a, "t").unwrap_or(0.0);
                let (w, q) = render::picture_size(a, render::FRAME_WIDTH);
                let full = render::is_full(a);
                let region = render::region_of(a)?;
                // A closer look at one part: the part at the picture's own
                // pixels, cut exactly from the full-size frame.
                if let Some(r) = region {
                    let side = if full { render::REGION_SIDE.1 } else { render::REGION_SIDE.0 } as u32;
                    let (src_w, label) = match a.get("media") {
                        Some(mv) => {
                            let mi = self.media_item(mv)?;
                            (mi.width.max(16) as u32, Some(mi.name.clone()))
                        }
                        None => (self.doc.project.width.max(16), None),
                    };
                    let src_w = a.get("width").and_then(Value::as_u64).map_or(src_w, |w| w as u32).clamp(64, 3840);
                    let img = match a.get("media") {
                        Some(mv) => {
                            let mi = self.media_item(mv)?;
                            render::media_frames(&mi, &[t.clamp(0.0, secs(mi.dur).max(0.0))], src_w)?.remove(0)
                        }
                        None => self.frames.rgba(&self.doc, micros(t), src_w)?,
                    };
                    let (whole_w, whole_h) = (img.width, img.height);
                    let (part, (x, y, cw, ch)) = render::crop(&img, r);
                    let part = render::shrink(part, side);
                    let mut note = json!({"t": t, "region": [r.x, r.y, r.w, r.h], "source_pixels": {"frame": [whole_w, whole_h], "rect": [x, y, cw, ch]}, "sent": [part.width, part.height]});
                    if let Some(n) = label {
                        note["media"] = json!(n);
                    }
                    return Ok(vec![Content::Jpeg(studio_content::jpeg::encode(&part, render::DETAIL_QUALITY)), Content::Text(note.to_string())]);
                }
                let w = if full { w } else if let Some(mv) = a.get("media") { let mi = self.media_item(mv)?; render::light_width(w, mi.width as u64, mi.height as u64) } else { render::light_width(w, self.doc.project.width as u64, self.doc.project.height as u64) };
                let w = w.clamp(64, 3840) as u32;
                if let Some(mv) = a.get("media") {
                    let mi = self.media_item(mv)?;
                    let img = render::media_frames(&mi, &[t.clamp(0.0, secs(mi.dur).max(0.0))], w)?.remove(0);
                    return Ok(vec![Content::Jpeg(studio_content::jpeg::encode(&img, q)), Content::Text(json!({"media": mi.name, "t": t}).to_string())]);
                }
                #[cfg(feature = "render")]
                if !a.get("force").and_then(Value::as_bool).unwrap_or(false) {
                    let key = format!("shown:{}", quality_key(self, micros(t), micros(t), &format!("frame{w}q{q}")));
                    if let Some(prev) = self.memo_pub().get(&key).cloned() {
                        return text(json!({"t": t, "unchanged": true, "same_as": prev, "note": "nothing on screen here changed since you got this picture; it is not sent again (force: true sends it)"}));
                    }
                    let rev = self.doc.history.revision();
                    self.memo_pub().insert(key, json!({"t": t, "revision": rev}));
                    self.memo_save_pub();
                }
                // The guides the person has on (or those asked for) are
                // drawn in, with where the figures sit against them.
                let g = guides::from_arg(a.get("guides"))?;
                if g.is_empty() {
                    let jpeg = self.frames.frame(&self.doc, micros(t), w, q)?;
                    return Ok(vec![Content::Jpeg(jpeg), Content::Text(json!({"t": t}).to_string())]);
                }
                let mut img = self.frames.rgba(&self.doc, micros(t), w)?;
                guides::draw(&mut img, &g);
                let framing = self.framing_words(micros(t), &g);
                Ok(vec![Content::Jpeg(studio_content::jpeg::encode(&img, q)), Content::Text(json!({"t": t, "guides": g.iter().map(|g| g.name()).collect::<Vec<_>>(), "framing": framing}).to_string())])
            }
            #[cfg(feature = "render")]
            "compare_cameras" => self.compare_cameras(a),
            #[cfg(feature = "render")]
            "animatic" => {
                self.animatic(a)
            }
            #[cfg(feature = "render")]
            "contact_sheet" => {
                if let Some(mv) = a.get("media") {
                    let mi = self.media_item(mv)?;
                    let len = secs(mi.dur);
                    let start = f(a, "start").unwrap_or(0.0).clamp(0.0, len);
                    let end = f(a, "end").unwrap_or(len).clamp(start, len);
                    let n = a.get("count").and_then(Value::as_u64).unwrap_or(12).clamp(1, 48) as usize;
                    let (w, q) = render::picture_size(a, render::SHEET_WIDTH);
                    let w = w.clamp(256, 3840) as u32;
                    let cols = render::sheet_columns(n);
                    let tile_w = ((w - 4 * (cols as u32 - 1)) / cols as u32).max(32);
                    let last = (end - 0.05).max(start);
                    let times: Vec<f64> = (0..n).map(|i| if n == 1 { start } else { start + (last - start) * i as f64 / (n - 1) as f64 }).map(|t| (t * 1000.0).round() / 1000.0).collect();
                    let tiles = render::media_frames(&mi, &times, tile_w)?;
                    let sheet = render::tiled(&tiles, cols, tile_w);
                    return Ok(vec![Content::Jpeg(studio_content::jpeg::encode(&sheet, q)), Content::Text(json!({"media": mi.name, "times": times, "columns": cols, "note": "times are seconds into the file (use them as source_in / source_out with place)"}).to_string())]);
                }
                let dur = secs(self.doc.project.duration());
                let start = f(a, "start").unwrap_or(0.0).max(0.0);
                let end = f(a, "end").unwrap_or(dur).min(dur.max(0.001));
                let n = a.get("count").and_then(Value::as_u64).unwrap_or(12).clamp(1, 48) as usize;
                let (w, q) = render::picture_size(a, render::SHEET_WIDTH);
                let w = w.clamp(256, 3840) as u32;
                let (jpeg, times) = self.frames.sheet(&self.doc, micros(start), micros(end), n, w, q)?;
                Ok(vec![Content::Jpeg(jpeg), Content::Text(json!({"times": times, "columns": render::sheet_columns(n)}).to_string())])
            }
            #[cfg(feature = "render")]
            "export" => {
                if lottie::wanted(a) {
                    return text(self.export_lottie(a)?);
                }
                let preset_name = a.get("preset").and_then(Value::as_str).unwrap_or("Social 1080p");
                let preset = services::export::preset(preset_name)
                    .or_else(|| services::export::PRESETS.iter().find(|p| p.name.to_lowercase().contains(&preset_name.to_lowercase())))
                    .ok_or_else(|| format!("No preset \"{preset_name}\"; presets: {}", services::export::PRESETS.iter().filter(|p| p.available).map(|p| p.name).collect::<Vec<_>>().join(", ")))?;
                let path = match a.get("path").and_then(Value::as_str) {
                    Some(p) => PathBuf::from(p),
                    // Next to the project when there is one, not in the
                    // user's Movies folder.
                    None => services::export::free_path(&self.home.clone().unwrap_or_else(studio_paths::videos), &self.doc.project.name, preset.format),
                };
                let range = match a.get("range") {
                    Some(Value::Array(r)) if r.len() == 2 => Some((micros(r[0].as_f64().unwrap_or(0.0)), micros(r[1].as_f64().unwrap_or(0.0)))),
                    _ => None,
                };
                // Licences are checked against what the timeline uses; what cannot be used stops the export.
                let rights = care::rights_gate(&self.doc, self.home.as_deref(), a)?;
                let req = services::export::request(&self.doc, preset, path.clone(), range);
                let mut job = services::export::ExportJob::start(&self.doc, req);
                {
                    // Stop ends the render the AI started.
                    let task = job.task.clone();
                    jobs::on_cancel(move || task.progress.cancel());
                }
                if a.get("wait").and_then(Value::as_bool).unwrap_or(true) {
                    let started = std::time::Instant::now();
                    return match job.wait() {
                        services::export::Event::Finished { path } => {
                            let secs_taken = (started.elapsed().as_secs_f64() * 10.0).round() / 10.0;
                            let (s, e) = range.unwrap_or((0, self.doc.project.duration()));
                            let l = quality::loudness(&self.doc.model, s, e);
                            let sound = if l.lufs.is_finite() { json!({"peak_db": (l.peak_db * 10.0).round() / 10.0, "loudness_lufs": (l.lufs * 10.0).round() / 10.0}) } else { json!(null) };
                            {
                                let mut done = json!({"ok": true, "path": path.display().to_string(), "duration": secs(e - s), "took_seconds": secs_taken, "sound": sound});
                                if !rights.is_null() {
                                    done["rights"] = rights.clone();
                                }
                                // Quality control on the file itself, unless asked not to: levels, flashes, black, frozen, sound.
                                if a.get("qc").and_then(Value::as_bool) != Some(false) {
                                    if let Some(q) = self.qc_after_export(&path, (s, e), a.get("spec").and_then(Value::as_str)) {
                                        done["qc"] = q;
                                    }
                                }
                                text(done)
                            }
                        }
                        services::export::Event::Failed(e) => Err(format!("export failed: {e}")),
                        _ => Err("export was cancelled".into()),
                    };
                }
                let id = self.next_export;
                self.next_export += 1;
                self.exports.push((id, job));
                text(json!({"export": id, "path": path.display().to_string(), "rights": rights}))
            }
            #[cfg(feature = "render")]
            "export_status" => {
                let id = a.get("export").and_then(Value::as_u64).ok_or("export_status needs \"export\"")? as u32;
                let job = self.exports.iter_mut().find(|(i, _)| *i == id).map(|(_, j)| j).ok_or_else(|| format!("No export {id}"))?;
                let mut state = json!({"export": id, "progress": job.fraction(), "eta_secs": job.eta_secs()});
                for e in job.poll() {
                    match e {
                        services::export::Event::Finished { path } => state = json!({"export": id, "done": true, "path": path.display().to_string()}),
                        services::export::Event::Failed(m) => state = json!({"export": id, "failed": m}),
                        services::export::Event::Cancelled => state = json!({"export": id, "cancelled": true}),
                        _ => {}
                    }
                }
                text(state)
            }
            t if sound_dept::TOOLS.contains(&t) => text(self.sound_tool(t, a)?),
            #[cfg(feature = "render")]
            "queue" | "qc" | "deliver" | "bundle" | "cutdown" => text(self.deliver_tool(tool, a)?),
            other if args::op_fields(other).is_some() => Err(format!("\"{other}\" is an op inside edit, not a tool: edit {{\"ops\": [{{\"op\": \"{other}\", …}}]}}")),
            other => Err(format!("unknown tool \"{other}\"; tools: {}", mcp::tools().iter().filter_map(|t| t["name"].as_str().map(str::to_owned)).collect::<Vec<_>>().join(", "))),
        }
    }
}

/// Gates for test tools that wait until the test lets them go.
#[cfg(test)]
pub(crate) mod test_gate {
    use std::collections::HashSet;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};

    static OPEN: Mutex<Option<HashSet<u64>>> = Mutex::new(None);
    static NEXT: AtomicU64 = AtomicU64::new(1);

    /// A new, closed gate (each test its own).
    pub fn new() -> u64 {
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    pub fn open(g: u64) {
        OPEN.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashSet::new).insert(g);
    }

    pub fn is_open(g: u64) -> bool {
        OPEN.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|s| s.contains(&g))
    }
}

/// The model's own edit operations, for `raw` ops.
pub const EDIT_SCHEMA: &str = r#"Raw model edits (times in microseconds, ids as numbers; "seq" is the sequence id from summary's internal model, normally use the simple ops instead):
{"op":"split","seq":S,"at":T,"clips":[ids]|null}
{"op":"trim","seq":S,"clip":C,"edge":"start"|"end","delta":T,"ripple":bool}
{"op":"ripple_delete","seq":S,"clips":[ids]}    {"op":"lift","seq":S,"clips":[ids]}
{"op":"move","seq":S,"clips":[ids],"delta":T,"to_lane":L|null}
{"op":"slip","seq":S,"clip":C,"delta":T}   {"op":"slide","seq":S,"clip":C,"delta":T}   {"op":"roll","seq":S,"left":C,"delta":T}
{"op":"duplicate","seq":S,"clips":[ids]}   {"op":"reverse","seq":S,"clips":[ids]}
{"op":"freeze_frame","seq":S,"clip":C,"at":T,"duration":T,"new_clip":0}
{"op":"detach_audio","seq":S,"clip":C,"new_clip":0}   {"op":"replace_media","seq":S,"clip":C,"media":M}
{"op":"set_keyframe","seq":S,"clip":C,"property":"transform.scale_x","key":{...}}
{"op":"remove_keyframe","seq":S,"clip":C,"property":P,"time":T}
{"op":"batch","edits":[...]}
Light, air and look of 3D scenes are parameters of light and model clips (the simple ops light, atmosphere, model and mesh write them; `set` and `animate` reach them as generator.<name>): a lamp light has glow, flare, beam, source_size (m), flicker, flicker_speed, flame, flame_height, flame_width, flame_tongues; the light clip with atmosphere: true carries the scene's ground, metre, fog_density, fog_falloff, fog_base, fog_colour, fog_anisotropy, fog_range, fog_patchiness, weather_1..3 (+ _amount, _wind_x|y|z, _size, _speed, _colour, _splash, _zone), tonemap, auto_exposure, exposure, contrast, saturation, temperature, tint, richness, shadow_tint, highlight_tint, bloom_strength, bloom_threshold, bloom_knee, bloom_radius, vignette, grain, halation, fringe, reflection, air_light; a model clip has wetness (0-1) and emissive_strength. Simple form: {"op":"atmosphere","preset":"neon_rain","ground":"auto"}  {"op":"light","clip":C,"glow":1,"flame":true,"flame_height":0.1,"flicker":0.35}  {"op":"model","shape":"plane","wetness":0.8}
Full list: insert_clip remove_clip set_clip insert_transition remove_transition set_transition insert_marker remove_marker set_marker insert_lane remove_lane set_lane move_lane insert_media remove_media set_media insert_sequence remove_sequence rename_sequence set_active_sequence set_settings insert_preset remove_preset set_preset batch insert overwrite append split trim ripple_delete lift move reorder slip slide roll duplicate paste paste_attributes group ungroup link unlink detach_audio replace_media freeze_frame reverse set_speed add_transition add_effect remove_effect set_effect move_effect set_grade add_mask remove_mask set_mask set_keyframe remove_keyframe set_track add_marker delete_marker change_marker add_lane delete_lane create_compound expand_compound"#;
