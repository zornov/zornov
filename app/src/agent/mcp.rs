// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Model Context Protocol server: `zornov mcp` speaks JSON-RPC 2.0 over
//! stdin / stdout (one message per line), so any MCP client (Claude Code,
//! Claude Desktop, other agents) can drive Studio headless.
//!
//! Tools are listed with a description and a JSON schema each; the guide is
//! also a resource (`studio://guide`) and the server's instructions tell the
//! agent to read it first.

use super::{Content, GUIDE, Session, production};
use serde_json::{Value, json};
use std::io::{BufRead, Write};

const PROTOCOL: &str = "2025-06-18";

pub(crate) const INSTRUCTIONS_LEN: usize = INSTRUCTIONS.len();
/// Kept to one line: some clients copy the server's instructions into the
/// description of every tool, so each word here is paid for once per tool
/// whenever the AI reads its tool list. The detail lives in `guide`.
const INSTRUCTIONS: &str = "Zornov, a video editor driven by tools: read `guide` once first; `edit` changes the project, `frame` and `contact_sheet` show it, `check` before `export`.";

fn obj(props: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": props, "required": required})
}

/// `schema` with more properties (one `json!` per part keeps each small).
fn merge(mut schema: Value, parts: &[Value]) -> Value {
    for p in parts {
        for (k, v) in p.as_object().into_iter().flatten() {
            schema["properties"][k.as_str()] = v.clone();
        }
    }
    schema
}

/// Every tool with its description and input schema.
/// Every tool with its description and input schema.
pub fn tools() -> Vec<Value> {
    let mut v = core_tools();
    v.extend(super::care::tools());
    v.extend(super::search_media::tools());
    v.extend(super::crew::tools());
    v.extend(super::skills::tools());
    v
}

/// The `edit` tool's description: how to call it and, from the op registry
/// (so it cannot go stale), the main fields of the ops used most.
fn edit_description() -> String {
    let mut s = String::from(
        "Change the project with ops applied as ONE undo step, all or nothing: {\"ops\": [{\"op\": name, ...fields}]}. The ops are listed in the guide's capability map. Clips by id, \"$1\" / \"$last\" (made earlier in this call), \"@4.2\" / \"music@4.2\", a name, or {lane, from, to}. An unknown field is refused with the right ones; the reply ends with read_back (the clips as they now are). dry_run checks only. Main fields (all ops: `edit_schema`, or guide topic ops): ",
    );
    let common = ["place", "add", "text", "card", "sfx", "music", "scene", "narrate", "set", "animate", "split", "trim", "move", "transition", "apply", "effect"];
    let parts: Vec<String> = common
        .iter()
        .filter_map(|op| {
            let f = super::args::op_fields(op)?;
            let shown: Vec<&str> = f.iter().copied().take(9).collect();
            Some(format!("{op}: {}{}", shown.join(", "), if f.len() > shown.len() { ", ..." } else { "" }))
        })
        .collect();
    s.push_str(&parts.join("; "));
    s.push_str(". text takes font (a family from the `fonts` tool), weight and italic; `style` names a text style from catalogue {panel: text}. add takes item (a name) or query (+ kind, mood: an unused sound each time).");
    s
}

fn core_tools() -> Vec<Value> {
    let num = |d: &str| json!({"type": "number", "description": d});
    let int = |d: &str| json!({"type": "integer", "description": d});
    let s = |d: &str| json!({"type": "string", "description": d});
    let ids = |d: &str| json!({"type": "array", "items": {"type": "integer"}, "description": d});
    let t = |name: &str, desc: &str, schema: Value| json!({"name": name, "description": desc, "inputSchema": schema});
    vec![
        t("guide", "The capability map (everything Zornov can do, by department) and the short core of how to work. Read it once at the start. With `topic`: the detail of one subject (see the topic list at the end of the core), `tool:<name>` for a tool's long form, `all` or `craft` for a contents list. A long subject comes as its opening and an outline of its parts: `part` reads one (or several, comma-separated), `full: true` all of it.", obj(json!({"topic": s("optional: one subject's detail, or all"), "part": s("a part of a long subject, by its title or first words"), "full": {"type": "boolean", "description": "the whole subject, not the outline (or the whole guide / craft book for all / craft)"}}), &[])),
        t("howto", "Search Zornov's built-in help (templates, shortcuts, workspaces, accessibility, exporting) to answer a person's question about Zornov itself. Fields: query, limit.", obj(json!({"query": s("what the person wants to know, in a few words"), "limit": int("how many articles (default 3)")}), &["query"])),
        t("summary", "The project in brief: canvas, lanes, and one line per clip (id, lane, kind, start/dur in seconds, what is applied). With since (a revision you already read): only what was added, changed or removed since.", obj(json!({"since": int("a revision you have already read")}), &[])),
        t("inspect", "Everything about one clip: transform, grade, audio, effects with parameters, keyframes, and where it is on screen (box, corners, anchor point, what its pixels cover) at `at` or its middle.", obj(json!({"clip": int("clip id"), "at": num("timeline seconds for on_screen (default: the middle of the clip)"), "pose": json!({"description": "a rigged 3D character: true for where its bones and key points are at `at` (feet soles with contact and sliding, hands with reach, head, eyes, gaze; world canvas px), or {\"span\": [from, to], \"step\": 0.1} for which feet are down, how far they slide and every finding (sliding or crossed feet, stretched or broken limbs, not looking) with its fix; {\"bones\": [names]} adds those bones' places"}), "bones": json!({"type": "array", "items": {"type": "string"}, "description": "with pose: bone names to give heads and tails of"})}), &["clip"])),
        t("fonts", "The fonts titles can use: installed system fonts and Zornov's bundled ones, searched by name, category and look (serif, sans, display, script, handwritten, mono, art deco, slab, condensed, retro, elegant...), with a small preview picture on request. Fields: query (free text, e.g. \"art deco\", \"heavy slab\"), category, family (one family in detail), source (system, bundled, imported), preview (true, or a sample text to draw), weight and italic (with family), limit. Then `text` / `set` take font, weight and italic; a run takes font too. With no arguments: what is available.", obj(json!({"query": s("what you want, in words: a name or a look"), "category": s("sans, serif, display, script, handwritten, mono, symbols"), "family": s("one family's details"), "source": s("system, bundled or imported"), "preview": {"description": "true, or the sample text to draw the first matches with"}, "weight": int("100 to 900, with family"), "italic": {"type": "boolean", "description": "with family"}, "limit": int("matches to list (20, up to 60)")}), &[])),
        t("parts_check", "Checks a split into parts (the parts op) side by side: the picture it was split from, the parts tinted with their seams, and where the stacked parts differ from the picture (holes, extras, wrong colours), with counts per part. Fields: clip (the rig), crop ([x0, y0, x1, y1] in picture pixels to zoom in), width (panel width, 120 to 900). Returns a picture and a verdict.", obj(json!({"clip": int("the rig clip that was split"), "crop": {"type": "array", "items": {"type": "number"}, "description": "x0, y0, x1, y1 in pixels of the source picture"}, "width": int("panel width in pixels")}), &["clip"])),
        t("voices", "The voices narration can use, most natural first (the first 30; all: true for every one).", obj(json!({"all": {"type": "boolean", "description": "list every voice, not the first 30"}}), &[])),
        t("canvas_layout", "Where everything is on the canvas, exactly (px from the centre, y down): the box of what really shows, order, depth, what covers it, cut or off screen, motion, puppet head and hands, text lines, 3D projected; overlaps, safe areas, free space, where a box fits. Topic canvas_layout.", obj(json!({"t": num("seconds"), "view": s("frame, item, overlaps, free, fit, safe"), "clip": json!({"description": "item: id or name"}), "from": num("range start"), "to": num("range end"), "size": json!({"description": "fit: [w, h]"}), "like": json!({"description": "fit: clip"}), "near": json!({"description": "fit: clip or [x, y]"}), "prefer": s("fit: top, bottom, left, right, lower_third"), "draw": json!({"type": "boolean"})}), &[])),
        t("pick", "The layers under a canvas point at a moment, topmost first (canvas pixels from the centre, y down).", obj(json!({"x": num("canvas x"), "y": num("canvas y"), "at": num("timeline seconds")}), &["x", "y"])),
        t("media", "Imported media files: id, kind, length, size, path.", obj(json!({}), &[])),
        t("search", "Search across every project on this Mac, and the content library: on-screen text, narration (and when), markers, media files and where they were used, your notes, saved moves and designs, library sounds, music and pictures. Every word must match (prefixes count). Fields: query, kinds (title, spoken, line, marker, media, note, move, design, project, library), limit.", obj(json!({"query": s("the words to find"), "kinds": {"type": "array", "items": {"type": "string"}, "description": "only these: title, spoken, line, marker, media, note, move, design, library, project"}, "limit": int("default 20")}), &["query"])),
        t("catalogue", "What the library has. No arguments: an inventory (counts by kind, category, mood, sound kind; packs; free versus credited). `query` words (a list searches pictures, sounds and motions at once). Fields: panel (effects, looks, motions, text, transitions, assets, audio, media), query, kind (music, sound, hit, boom, swoosh, swell, long, sticker, emoji...), mood (calm, happy, energetic, epic, dark, sad, retro), min_seconds, max_seconds, fresh (leave out what the project plays), limit, details. Results show kind, length, mood and what the project already uses.", obj(json!({"panel": {"type": "string", "enum": ["all", "effects", "looks", "motions", "text", "transitions", "assets", "audio", "media"]}, "query": {"description": "a search, or a list of words to find everywhere at once"}, "limit": int("default 40 (5 per word for a list)"), "details": {"type": "boolean", "description": "include each effect's settings (default: only when 5 or fewer items)"}}), &[])),
        t(
            "edit",
            &edit_description(),
            obj(json!({"ops": {"type": "array", "items": {"type": "object"}, "description": "ops, e.g. [{\"op\":\"split\",\"at\":4.2},{\"op\":\"apply\",\"item\":\"gaussian blur\",\"clips\":[7]}]"}, "label": s("undo label"), "dry_run": {"type": "boolean", "description": "check the ops without changing anything"}}), &["ops"]),
        ),
        t("undo", "Undo your last call's change (an `edit` call undoes as a whole). Only your own work: what the person did by hand stays.", obj(json!({}), &[])),
        t("redo", "Redo what `undo` took back.", obj(json!({}), &[])),
        t("new_project", "Start a new, empty project.", obj(json!({"name": s("project name"), "width": int("default 1920"), "height": int("default 1080; 1920 for vertical"), "fps": num("default 30")}), &[])),
        t("open", "Open a saved project folder.", obj(json!({"path": s("project folder")}), &["path"])),
        t("save", "Save the project (to `path`, a folder, or where it was opened from).", obj(json!({"path": s("project folder")}), &[])),
        t("import", "Bring media files into the project (video, audio, images, Photoshop, Lottie). Fields: paths, to_timeline (also lay on the main lane), layers (PSD: one clip per layer), merge, at, duration. More: guide {topic: \"tool:import\"}.", obj(json!({"paths": {"type": "array", "items": {"type": "string"}}, "to_timeline": {"type": "boolean"}, "layers": {"type": "boolean", "description": "Photoshop documents layer by layer"}, "merge": {"type": "boolean", "description": "with layers: merge them into one clip (default true)"}, "at": num("with layers: timeline seconds"), "duration": num("with layers: seconds (default 5)")}), &["paths"])),
        t("make_playable", "Media already in the project that Zornov cannot decode (AV1, VP9, WebM, ProRes…: preview says unsupported codec) is converted to H.264 in Zornov and relinked; clips stay as they are. import and download already do this for new files.", obj(json!({"media": ids("optional media ids (default: every one that needs it)")}), &[])),
        t("read", "Read a web page or a GitHub repository as text. A page comes back as readable text (headings, paragraphs, lists); a github.com repository link gives its description, README and file tree, and file: \"path/in/repo\" gives one file's text (a /tree/branch/folder link lists inside that folder). Long text is cut: from continues, max_chars sizes a part (default 12000). links: true also lists the page's links. Public web only; for a video, sound or picture use download.", obj(json!({"url": s("the page or repository link"), "file": s("GitHub: a file path inside the repository"), "from": int("continue a long text from this character"), "max_chars": int("most characters to return (default 12000, at most 40000)"), "links": {"type": "boolean", "description": "also list the page's links"}}), &["url"])),
        t("job", "Long work (a download, scouting footage with see, a transcode, optical flow in smooth_motion, a generation, a render) moves to the background if it is not done in a few seconds: you get a job number at once, every other tool keeps working, and you are told in a later answer when it finished. `job` with no arguments lists the jobs; with id it gives that job's progress and, when done, its result. A job has a time limit and is given up on after it.", obj(json!({"id": int("the job number")}), &[])),
        t("download", "Bring a file from the internet into the project: a direct link to any file (video, sound, picture, 3D model such as glb, gltf, obj, fbx, stl or ply, document; GitHub blob and raw links work) or a page whose video or sound is openly exposed (HLS and DASH are joined into one MP4). A 3D model is added as a model clip; a document comes back as text. Protected, signed-in or hidden media is refused with the reason: never log in or work around it; tell the person. Only media they may use. Fields: url, name, quality (best, lowest, audio, 720...), language, to_timeline. More: guide {topic: \"tool:download\"}.", obj(json!({"url": s("the link: a file or a web page"), "name": s("optional file name"), "quality": s("pages and playlists with several versions: best (default), lowest, audio, or a height such as 720 or 1080p"), "language": s("sound language when a stream has several (en, de...)"), "to_timeline": {"type": "boolean", "description": "also put it on the timeline"}}), &["url"])),
        t("picture", "Paint and edit raster pictures (painting, photo retouching, selections, adjustment layers, layers, frame-by-frame animation); `ops` run on the active picture as one step. Fields: ops (new, open, select, paint, fill, gradient, remove, generative_fill, adjust, layer, trace, from_video, to_timeline, undo, snapshot...), picture, look (return how it looks), commands (search engine commands by id), list. Coordinates are picture pixels from the top-left. More: guide {topic: \"tool:picture\"}.", obj(json!({"ops": {"type": "array", "items": {"type": "object"}, "description": "ops, e.g. [{\"op\":\"select\",\"what\":\"sky\"},{\"op\":\"adjust\",\"kind\":\"curves\",\"points\":[[0,0],[128,90],[255,230]]}]"}, "picture": s("the picture to work on (default: the active one, else the last saved)"), "look": {"description": "true or a size in pixels: return the picture as it looks after the ops"}, "commands": s("search the engine's commands (words); returns ids, labels and parameters"), "list": {"type": "boolean", "description": "the open and saved pictures"}}), &[])),
        t("generate", "Make a picture from words with Codex's image generation, only for what the person asked for (real subjects: search_media first); video, music and sound from words are not available yet. Fields: prompt, make (picture), items (a batch of pictures made at once: use it for more than one), image/images/end_image, aspect, count, to_timeline, task (animate, between, restyle, relight, time_of_day, season, outpaint, inpaint, upscale), look/save_look, again. `list: true` says what is available. More: guide {topic: \"tool:generate\"}.", merge(obj(json!({"list": {"type": "boolean", "description": "what can be made and what each task can do here"}, "make": s("picture"), "prompt": s("what to make"), "image": json!({"description": "a picture to work from: file path, https link, media id or {\"frame\": seconds}"}), "images": {"type": "array", "description": "several pictures to follow"}, "end_image": json!({"description": "between: the last frame"}), "aspect": s("16:9, 9:16, 1:1, 4:3 or 3:4"), "seconds": num("animate or between: length"), "items": {"type": "array", "description": "BATCH of pictures made at the same time by Codex image workers (4 at once): plan every picture needed first and send them in ONE call, never one by one. Each {name, prompt, aspect, transparent, image}. Returns a job at once so you can keep working; finished pictures land in the media bin and are announced after your next tool calls; job: N shows progress, wait: true waits for all. A failure is retried once with a reworded request and reports its real reason.", "items": {"type": "object"}}, "workers": int("items: workers at once (default 4, up to 8)"), "wait": {"type": "boolean", "description": "items or job: wait until every picture is done (default: return a job at once)"}, "job": int("a picture batch: its progress and what is ready"), "cancel": {"type": "boolean", "description": "job: stop pictures not started yet"}, "count": int("pictures per run"), "transparent": {"type": "boolean", "description": "picture: the subject alone on a transparent background (for layering in 3D)"}, "provider": s("with a task: codex, local or auto"), "avoid": s("what to keep out"), "seed": int("for a repeatable result"), "estimate": {"type": "boolean", "description": "what it would use; nothing is made"}, "shot": s("the shot this is for: a version is recorded in the production office"), "scene": s("the scene this is for (when it is not for one shot)"), "name": s("file name"), "to_timeline": {"type": "boolean", "description": "also put it on the timeline (a task's result lands there unless false)"}, "at": num("timeline seconds to place it at (puts it on the timeline)")}), &[]), &[json!({"task": s("animate, between, restyle, relight, time_of_day, season, outpaint, inpaint, upscale, enhance (omit to make from words)"), "clip": int("task: the clip to work on: footage, a still, or anything else (a title, card, shape or nested clip is rendered with what lies under it); the result goes back into it, same timing"), "replace": {"type": "boolean", "description": "task on a clip: put the result in the clip (default true); false only imports it"}, "style_ref": json!({"description": "restyle: the look to follow: a picture file, media id or {\"frame\": seconds}"}), "refs": {"type": "array", "description": "more pictures to follow (a character, a place)"}, "look": s("a saved look or a story-bible character or place: its pictures, words and seed go with the generation"), "save_look": s("save a look under this name (with refs, words, seed)"), "words": s("save_look: what they look like, in words"), "looks": {"type": "boolean", "description": "list the looks"}, "forget_look": s("remove a saved look"), "again": json!({"description": "a generated media id or file: made again from its recipe (other keys override)"}), "vary": {"type": "boolean", "description": "again: a new seed"}, "restore": int("a clip a task replaced: back to its original")}), json!({"direction": s("relight: left, right, top, bottom, top-left, front, back (or degrees)"), "elevation": num("relight: degrees above the picture plane"), "colour": s("relight: warm, golden, blue, moonlight, neon… or #rrggbb"), "ambient": num("relight: 0..1 of the old light kept"), "strength": num("0..1: how far the change goes (restyle: from the original)"), "time": s("time_of_day: dawn, morning, noon, golden hour, sunset, blue hour, night"), "season": s("season: spring, summer, autumn, winter")}), json!({"left": num("outpaint: share of the width added on the left"), "right": num("outpaint: share added on the right"), "top": num("outpaint: share of the height added on top"), "bottom": num("outpaint: share added below"), "wider": num("outpaint: times as wide, centred"), "taller": num("outpaint: times as tall, centred"), "region": {"type": "array", "items": {"type": "number"}, "description": "inpaint: [x, y, width, height] as shares of the picture"}, "scale": num("upscale or enhance: 1 to 4 times"), "height": num("upscale or enhance: to this many pixels high"), "fps": num("animate or between: frames a second; enhance: a higher frame rate made with RIFE in-betweens"), "motion": s("animate on this Mac: push_in, pull_out, pan_left, pan_right, rise, fall, orbit_left, orbit_right, dolly_zoom"), "long": {"type": "boolean", "description": "go ahead with more than 15 minutes of work on this Mac"}, "best_of": int("picture tasks by Codex: make this many takes (up to 6), score each (composition kept, detail, clipping, closeness to the reference) and keep the best; the rest go to Generated/Takes")})])),
        t("vector", "Vector illustration: draw, edit and lay out vector art in a document. Fields: doc (a .svart name; .svg, .pdf, .eps or an image open as new), new, ops (shapes, paths, type, paint, pathfinder, trace, repeat, blend, charts, export...; {\"op\": \"help\"} lists them with fields), artboard, look (return a picture), place (put the art on the timeline: at, duration, x, y, fit). More: guide {topic: \"tool:vector\"}.", obj(json!({"doc": s("document name or path (default art.svart in the Vector folder)"), "new": json!({"type": "object", "description": "start a fresh document: width, height, background"}), "ops": {"type": "array", "items": {"type": "object"}, "description": "vector ops, e.g. [{\"op\":\"circle\",\"cx\":960,\"cy\":540,\"r\":200,\"fill\":\"#e8573f\"}]"}, "look": {"type": "boolean", "description": "return a picture of the artboard"}, "width": num("look: picture width (default 1200)"), "artboard": s("which artboard to look at"), "summary": {"type": "boolean", "description": "include the document outline (default when there are no ops)"}, "place": json!({"type": "object", "description": "put the art on the timeline: at, duration, lane, x, y, fit (none, contain, cover), background, params (start values of bound parameters), name"})}), &[])),
        t("enhance", "Restore and improve a clip or picture on this Mac (free by default). Fields: clip or image, steps (auto, dust, scratches, deflicker, deblock, denoise, fade, faces, clean, upscale, sharpen), old_film, scale or height (upscale), fps (in-between frames), estimate, remove (put the original back). More: guide {topic: \"tool:enhance\"}.", obj(json!({"clip": int("the clip"), "image": json!({"description": "a picture instead: file, media id or {\"frame\": seconds}"}), "steps": {"type": "array", "items": {"type": "string"}, "description": "auto, dust, scratches, deflicker, deblock, denoise, fade, faces, clean, upscale, sharpen"}, "old_film": {"type": "boolean", "description": "old film restoration"}, "strength": num("denoise strength 0..1 (default 0.6)"), "scale": num("1 to 4 times larger"), "height": num("to this many pixels high"), "fps": num("a higher frame rate (RIFE in-betweens)"), "faces": {"type": "boolean", "description": "clean up faces"}, "estimate": {"type": "boolean", "description": "how long or how much, nothing made"}, "long": {"type": "boolean", "description": "go ahead with more than 15 minutes of work"}, "remove": int("a clip enhanced before: back to its original"), "name": s("file name")}), &[])),
        t("ai", "Run an AI job and apply its result: captions (speech to timed caption clips), voice_cleanup (cleaner dialogue as a new audio clip), beat_markers (markers on the music's beats). Blocks until done.", obj(json!({"action": {"type": "string", "enum": ["captions", "voice_cleanup", "beat_markers"]}, "clips": ids("optional target clips")}), &["action"])),
        t("analyse", "Understand the footage: shot changes, pauses (silences) and beats (with bpm), found in the source files and given in timeline seconds for each clip. Use the times directly in edit ops (split at shot changes, cut pauses, cut on beats). Cached per file.", obj(json!({"what": {"type": "array", "items": {"type": "string", "enum": ["scenes", "silences", "beats"]}, "description": "default all"}, "clips": ids("default: every clip with media"), "min_pause": num("shortest pause to report, seconds (default 0.6)")}), &[])),
        t("frame", "Look at the picture at time `t` (seconds) of the timeline, or of a media file with `media` (t is then seconds into the file). Returns a JPEG. For several moments use contact_sheet (one picture), not many frames.", obj(json!({"t": num("seconds"), "media": s("optional media id or name: look at the footage itself"), "width": int("pixels, default 512, at most 1024 unless detail is full (1280 with it)"), "detail": {"description": "\"full\" (or true): 1280 px, less compressed. Use it whenever you judge how it looks (colour, grade, sharpness, the final look), and to read small text. Off by default: a light picture costs fewer tokens, fine for checking that a change landed"}, "region": {"description": "look closely at one part at the picture's own pixels, cut exactly from the full-size frame: [x, y, w, h] as fractions of the frame from its top left, e.g. [0.5, 0, 0.5, 0.5] is the top right quarter; much cheaper than a full-detail frame"}, "force": {"type": "boolean", "description": "send the picture even if nothing changed since you last got it"}, "guides": json!({"description": "composition guides drawn on the picture, with where the figures sit against them: a list of thirds, golden, centre, safe, or an aspect (2.39, 1.85, 4:3, 1:1, 9:16); true: the overlays the person has on in the viewer (the default); false: none"})}), &["t"])),
        t("compare_cameras", "Camera options side by side: each option (words like \"low angle close-up on Pip\", or shot op fields) framed on the same moment, one JPEG with a tile each and where the figures sit in each; nothing is changed (make the one you choose with the shot op).", obj(json!({"options": json!({"description": "two to six shots: words or shot fields"}), "t": num("the moment to compare (default: the middle of each shot)"), "at": num("when the shots start"), "duration": num("how long they run"), "guides": json!({"description": "guides drawn on each tile (default: the viewer's)"}), "width": int("tile width, default 420")}), &["options"])),
        t("animatic", "The cut as an animatic: each camera's shot in order, its first, middle and last moment in a row, with the shot list (lens, moves, focus). Play or export the timeline for the moving one.", obj(json!({"guides": json!({"description": "guides drawn on each frame"}), "width": int("frame width, default 320")}), &[])),
        t("contact_sheet", "Look at many moments at once: `count` frames spread over start..end tiled into one JPEG, with their times. With `media`, samples a media file (times are seconds into the file: use them as place's source_in / source_out).", obj(json!({"start": num("seconds, default 0"), "end": num("seconds, default the end"), "count": int("default 12"), "media": s("optional media id or name"), "width": int("sheet width, default 1024, at most 1536 unless detail is full (1920 with it)"), "detail": {"description": "\"full\" (or true): 1920 px, less compressed. Use it when judging how the cut looks; leave it off for checking that a change landed"}}), &[])),
        t("script", "The whole timeline read like an edit decision list: one line per clip with time, source range, speed, looks, effects, keys and sound; transitions; markers; your notes. Ends with the revision. Field: since (a revision you read: only changed lines).", obj(json!({"since": int("a revision you have already read")}), &[])),
        #[cfg(feature = "commerce")]
        super::account_tools::schema(),
        t("production", production::DESCRIPTION, production::schema()),
        t("notes", "Your notebook, kept with the project: the brief, your plan, the beat map, what is left. No arguments reads it; write replaces it, append adds a line.", obj(json!({"write": s("replace the notes with this"), "append": s("add this to the notes")}), &[])),
        t("story", "The production plan kept in the project: script, beats, scenes (place, light, time of day, cast, shots) and the character bible. Plan with it before building. Field `do`: show, cast, import, export, set, remove, plan, line, retime, check, build, bind, brief; plus scene, shot, text, path, format, title, logline, beats, places, cast, scenes, shots. Times are seconds or timecodes (0:12); lines count from 1. `line` reads/sets a script line's start, end, text, note. More: guide {topic: \"tool:story\"}.", obj(json!({"do": s("show (default), cast, import, export, set, remove, plan, line, retime, check, build, bind, brief"), "scene": json!({"description": "show: a scene id; remove: one"}), "shot": s("brief / bind / remove: a shot id"), "text": s("import: the screenplay or narration"), "path": s("import: a .fountain / .txt / .md / .srt / .vtt / .docx file; export: where to write"), "format": s("import: fountain, text, markdown, srt, vtt (default: whichever it reads as); export: fountain, script, text, srt or shots"), "merge": {"type": "boolean", "description": "import over the story scene by scene, keeping shots and the bible"}, "line": int("line: the line number to change (from 1; none adds one)"), "edits": {"type": "array", "description": "line: several line changes [{scene, line, text, who, start, end, duration, time, note, clear_time, remove, after}]"}, "start": json!({"description": "line: start (seconds or 0:12)"}), "end": json!({"description": "line: end"}), "duration": json!({"description": "line: length"}), "time": s("line: \"0:12-0:18\""), "note": s("line: a note for the crew"), "shift": num("line: move given times by seconds"), "replace": {"type": "boolean", "description": "import over an existing story; plan: plan scenes that have shots again"}, "plan": {"type": "boolean", "description": "import: also plan every scene's shots"}, "title": s("set"), "logline": s("set"), "narrator_voice": s("set: the voice narration lines use"), "beats": {"type": "array", "description": "set: [{id, summary, purpose}]"}, "places": {"type": "array", "description": "set: [{id, name, interior, backdrop (media name or file), ambience (stream, forest, birds, night…), light, time_of_day, surface (grass, gravel, wood, snow, water…), notes}]"}, "cast": {"type": "array", "description": "set: [{id, name, look, references, cast_item, voice, size (1 = the reference), personality, moves (puppet acts, gait first), facing, notes}]"}, "scenes": {"type": "array", "description": "set: [{id, heading, beat, place, time_of_day, cast, summary, lines (\"FOX: words\", plain narration or {kind, who, text, how}), shots: [{id, kind, subjects, framing, camera, duration, lines, action, side, crosses, looks, after}], replace_shots, after, remove}]; plan: scene ids; build: scene ids"}, "shots": {"type": "array", "description": "set: shot patches by id ({id, duration, kind, …} or remove)"}, "clips": ids("bind: the clips that show the shot"), "add": {"type": "boolean", "description": "bind: add to the shot's clips instead of replacing them"}, "from": num("bind: start of the range"), "to": num("bind: end of the range"), "at": num("build: where the first shot starts (default after the main lane)"), "rebuild": {"type": "boolean", "description": "build: take the picked scenes' built clips off and lay them again"}, "ambience": {"type": "boolean", "description": "build: lay each place's ambience bed (default true)"}, "markers": {"type": "boolean", "description": "build: a marker at each scene (default true)"}, "voices": {"type": "boolean", "description": "build: speak each shot's lines with the character's (or narrator's) voice"}, "all": {"type": "boolean", "description": "remove: the whole story"}}), &[])),
        t("comments", "Review notes from the team, pinned to a time or clip, with replies. Fields: action (list, add, reply, resolve, reopen, edit, delete, export, import), id, note, at, until, clip, region, status, format, path. To work through notes: list, fix each with edit, look, then resolve with a note saying what changed.", obj(json!({"action": s("list, add, reply, resolve, reopen, edit, delete, export, import"), "id": json!({"description": "a note id (\"sam-3\"), a list of them, or \"all\" (every open note)"}), "note": s("the note, reply or what was done"), "at": num("timeline seconds"), "until": num("end of a stretch, seconds"), "clip": int("the clip the note is about"), "region": json!({"type": "array", "items": {"type": "number"}, "description": "[x0, y0, x1, y1] canvas pixels from the centre, y down"}), "status": s("list/export: open (default for list), resolved, all"), "from": num("list: from seconds"), "to": num("list: to seconds"), "format": s("export: csv, markdown, edl, json"), "path": s("export: file to write; import: notes file or package folder"), "as": s("sign as this name (default: the person, marked as AI)")}), &[])),
        t("versions", "Named versions of the project. Fields: action (list, save, compare, restore, branch, switch), name, note, version, from, to.", obj(json!({"action": s("list, save, compare, restore, branch, switch"), "name": s("version or cut name"), "note": s("what this version is"), "version": s("a version id (v3), name or latest"), "from": s("compare from (default: the current version)"), "to": s("compare to (default: now)"), "switch": {"type": "boolean", "description": "branch: make the new cut active (default true)"}, "as": s("sign as this name")}), &[])),
        t("share", "Working with others through files the person chooses. Fields: action (package, import_notes, merge, link, sync, status), path, folder, base, proxy.", obj(json!({"action": s("package, import_notes, merge, link, sync, status"), "path": s("package: parent folder; import_notes: package or file; merge: the other copy's project folder"), "folder": s("link: the shared folder"), "base": s("merge: the version both copies started from (default: the newest version they share)"), "proxy": {"type": "boolean", "description": "package: render the proxy (default true)"}, "as": s("act as this name")}), &[])),
        t("team", "Who works on this project: roles (owner manages the team, editor edits, reviewer only adds notes; with no team everyone is an owner) and locks (an editor holds a lane or a clip; nobody else's edits may change it, ripples included). show, set {name, role}, remove {name}, lock {lane | clip, note}, unlock {lane | clip}, set_name {name} (the name notes are signed with on this computer).", obj(json!({"action": s("show, set, remove, lock, unlock, set_name"), "name": s("a person"), "role": s("owner, editor or reviewer"), "lane": int("lane id from summary"), "clip": int("clip id"), "note": s("lock: what it is held for"), "as": s("act as this name")}), &[])),
        t("graph", "The graph editor as numbers: a clip's animated values as curves, each with a sparkline, samples in the property's units, its keys (time, value, curve, bezier handles) and what to fix: overshoot past the keys, kinks (the speed jumps at a key), the fastest moment, holds. `property` narrows it to one. Reshape with the edit op `keys` (move, curve, out / in handles).", obj(json!({"clip": json!({"description": "the clip"}), "property": s("optional: one property (rotation, scale, x, position, opacity, an effect setting…)"), "samples": int("samples per curve, default 24")}), &["clip"])),
        {
            let props: serde_json::Map<String, Value> = [
            ("action", s("get (default), on, off, orbit, pan, dolly, fly, walk, look, set, view, frame, look_through, editor, camera_to_view, view_from_camera, lock_camera, record_path, bookmark, go, forget, pick, select, move, rotate, scale, hide, unhide, lock, unlock, cursor, shading, spread, options, picture")),
            ("t", num("timeline seconds the world is read at (default 0)")),
            ("yaw", num("degrees")),
            ("pitch", num("degrees (positive looks down)")),
            ("about", s("orbit round: selection (with clips) or cursor")),
            ("clips", ids("clip ids")),
            ("x", num("pan: canvas pixels; pick/dolly: 0..1 across the view")),
            ("y", num("pan: canvas pixels; pick/dolly: 0..1 down the view")),
            ("factor", num("dolly: < 1 closer, > 1 further")),
            ("forward", num("pixels")),
            ("right", num("pixels")),
            ("up", num("pixels")),
            ("target", json!({"type": "array", "items": {"type": "number"}, "description": "[x, y, z] the eye looks at"})),
            ("position", json!({"type": "array", "items": {"type": "number"}, "description": "[x, y, z] the eye stands at"})),
            ("look_at", json!({"type": "array", "items": {"type": "number"}, "description": "[x, y, z]"})),
            ("distance", num("eye to target, pixels")),
            ("lens", num("focal length mm")),
            ("orthographic", json!({"type": "boolean"})),
            ("axis", s("front, back, left, right, top, bottom, perspective, orthographic, camera")),
            ("camera", int("camera clip id")),
            ("name", s("saved view name")),
            ("mode", s("shading: solid, material, rendered, wireframe, xray")),
            ("px", num("spread: depth between 2D layers")),
            ("on", json!({"type": "boolean"})),
            ("cards", json!({"type": "boolean"})),
            ("grid", json!({"type": "boolean"})),
            ("overlays", json!({"type": "boolean"})),
            ("frame", json!({"type": "boolean"})),
            ("quad", json!({"type": "boolean"})),
            ("snap", json!({"type": "boolean"})),
            ("snap_step", num("pixels")),
            ("snap_angle", num("degrees")),
            ("tool", s("select, move, rotate, scale")),
            ("orient", s("world, local, view")),
            ("pivot", s("median, individual, cursor")),
            ("space", s("move/rotate in: world, view, local")),
            ("floor_y", num("floor height (y down)")),
            ("speed", num("fly speed px/s")),
            ("by", json!({"description": "move [x,y,z] px, rotate [x,y,z] degrees, scale factor or [x,y,z]"})),
            ("at", json!({"type": "array", "items": {"type": "number"}, "description": "cursor: [x, y, z]"})),
            ("keys", json!({"type": "array", "items": {"type": "object"}, "description": "record_path waypoints"})),
            ("picture", json!({"type": "boolean", "description": "also send the view as a picture"})),
            ("width", int("picture width, default 768")),
            ]
            .into_iter()
            .map(|(k, v): (&str, Value)| (k.to_owned(), v))
            .collect();
            t("viewport", "The 3D workspace as a person uses it: read the eye and every object (`get`), move the eye, use cameras, move objects, take a picture of the view. Fields: action (get, orbit, pan, dolly, fly, look, set, view, frame, look_through, camera_to_view, record_path, bookmark, go, pick, select, move, rotate, scale, hide, shading, options...), t, clips, yaw, pitch, x, y, target, position, look_at, distance, lens, axis, camera, by, keys, picture. Units: canvas pixels from the centre, y down, z away. More: guide {topic: \"tool:viewport\"}.", obj(Value::Object(props), &[]))
        },
        t("colour", "Colour management and grading. Fields: action (spaces, input, view, output, ocio, grade, node, lut, cdl, hdr, stills), media, space, view, clip/clips, params (exposure, contrast, wheels, curves, hsl_*, cdl_*), lut, qualifier, window, path, slope, offset, power, sat, t. More: guide {topic: \"tool:colour\"}.", obj(json!({"action": {"type": "string", "enum": ["spaces", "input", "view", "output", "ocio", "grade", "node", "lut", "cdl", "hdr", "stills"]}, "media": json!({"description": "media id, name, list, or \"all\""}), "space": s("input / output colour space"), "view": s("view transform"), "hdr_peak": num("HDR peak in cd/m² (1000, 2000, 4000)"), "config": s("OCIO config file"), "clip": int("clip id"), "clips": ids("clip ids"), "params": json!({"type": "object", "description": "grade parameters {name: value | null to remove}"}), "lut": s("LUT file (.cube or .3dl); empty removes"), "reset": {"type": "boolean"}, "enabled": {"type": "boolean"}, "node": int("node number (1-based); leave out to add a node"), "name": s("node or still name"), "qualifier": json!({"description": "preset name or {hue, hue_width, hue_soft, sat_low, sat_high, sat_soft, lum_low, lum_high, lum_soft, invert}"}), "window": json!({"description": "{shape: ellipse|rectangle|gradient, x, y, width, height (0..1 of the picture), feather, rotation} or a mask id; null removes"}), "window_invert": {"type": "boolean"}, "mix": num("node strength 0..1"), "key_blur": num("soften the key, pixels"), "remove": {"type": "boolean"}, "path": s("file to read (LUT, CDL)"), "export": s("file to write (LUT .cube/.3dl, CDL .cdl/.ccc)"), "size": int("LUT size, default 33"), "intensity": num("LUT strength 0..1"), "slope": json!({"description": "number or [r, g, b]"}), "offset": json!({"description": "number or [r, g, b]"}), "power": json!({"description": "number or [r, g, b]"}), "sat": num("CDL saturation"), "t": num("timeline seconds"), "from": num("seconds"), "to": num("seconds"), "count": int("frames to sample"), "save": s("still name to keep"), "apply": s("still to apply")}), &["action"])),
        t("conform", "Exchange timelines and relink media. Fields: action (import, export, relink, offline), path (.edl, .fcpxml, .otio), format, rate, start_timecode, folder, media, by_timecode. AAF is not supported.", obj(json!({"action": {"type": "string", "enum": ["import", "export", "relink", "offline"]}, "path": s("file"), "format": s("edl, fcpxml or otio (default: from the extension)"), "rate": s("EDL timecode rate, e.g. 23.976, 29.97 (default: the project's)"), "start_timecode": s("record timecode of the start, default 01:00:00:00"), "open": {"type": "boolean", "description": "open the imported sequence (default true)"}, "folder": s("where to look for media"), "media": json!({"description": "media to relink (default: every offline / missing one)"}), "by_timecode": {"type": "boolean", "description": "match by timecode when names and reels do not (default true)"}}), &["action"])),
        t("media_manage", "Media management. Fields: action (info, set, proxies, use_proxies, forget_proxies, consolidate, multicam, angle), media, timecode, reel, scale, folder, sync (sound or timecode), audio, name, place, at, clip, angle.", obj(json!({"action": {"type": "string", "enum": ["info", "set", "proxies", "use_proxies", "forget_proxies", "consolidate", "multicam", "angle"]}, "media": json!({"description": "media id, name, list or \"all\""}), "timecode": s("first-frame timecode, e.g. 14:22:10:05 (; for drop-frame)"), "reel": s("reel / camera roll name"), "scale": int("proxy size divisor: 2 (half) or 4 (quarter)"), "again": {"type": "boolean", "description": "remake proxies that exist"}, "use": {"type": "boolean", "description": "switch to the proxies after making them (default true)"}, "on": {"type": "boolean"}, "folder": s("consolidate destination"), "all": {"type": "boolean", "description": "also unused media"}, "sync": s("sound or timecode"), "audio": int("which angle's sound to use (1-based, default 1)"), "name": s("multicam name"), "place": {"type": "boolean", "description": "put the multicam clip on the main lane (default true)"}, "at": num("timeline seconds"), "clip": int("multicam clip id"), "angle": int("angle number (1-based)")}), &["action"])),
        t("scopes", "The video scopes at time `t` as numbers: luma percentiles, blown and crushed shares, colour cast, saturation, skin line. Fields: t, compare (a second moment, to match shots), picture (RGB parade and vectorscope as a JPEG).", obj(json!({"t": num("seconds on the timeline"), "compare": num("another moment to measure against"), "picture": {"type": "boolean", "description": "also send the parade and vectorscope picture"}}), &["t"])),
        t("see", "Zornov looks for you and says what it sees in words (a few dozen tokens a line instead of a picture): light, colour, faces, subject position, camera motion, sharpness, lettering. No arguments: one line per main-lane shot. Fields: t, clips, media (+ from, to, every: footage before use), place + t (backdrop read for staging), regions + t (sky, water, ground, trees...). Use frame / contact_sheet only when you need the picture itself.", obj(json!({"t": num("a timeline moment"), "clips": json!({"description": "clips to describe (at their middle)"}), "media": s("a media file to scout"), "from": num("with media"), "to": num("with media"), "every": num("with media: seconds between lines"), "place": json!({"type": "boolean", "description": "with t: read the backdrop for staging (ground, depth, spots to stand on and how tall to be there)"}), "without": json!({"description": "with place: clip ids to leave out (default: everything above the main lane)"}), "guides": json!({"description": "with t: say where the figures sit against these guides (thirds, golden, centre, safe, an aspect like 2.39); default: the overlays on in the viewer, else thirds"}), "regions": json!({"type": "boolean", "description": "with t: what the backdrop's parts are (sky, water, trees...) and where"}), "backdrop": num("with regions: the backdrop's clip id (default: the lowest picture showing)"), "objects": json!({"type": "boolean", "description": "add each shot's objects (false: leave out)"})}), &[])),
        t("objects", "What is in footage and pictures, as a map read once and cached: each thing with id, label, box per frame, distance and size in metres, colours, text, facing, when it enters and leaves, its own motion. action: list (default), where {object, t}, when, around (on, holding, in front, behind), find {query} (moment matching words), space {t, to} (room for titles), text, mask (ready cut_out / track ops; apply: true cuts). object: id or name. A first read takes seconds a shot.", obj(json!({"action": s("list | where | when | around | find | space | text | mask"), "clip": num("clip id"), "clips": json!({"description": "clip ids"}), "t": num("timeline moment"), "to": num("with space: until"), "object": json!({"description": "id (3) or name"}), "query": s("with find"), "names": json!({"description": "extra things to look for"}), "detail": json!({"type": "boolean"}), "max": num("most lines"), "apply": json!({"type": "boolean"}), "quick": json!({"type": "boolean"}), "refresh": json!({"type": "boolean"}), "max_shots": num("unread shots per call")}), &[])),
        t("save_move", "Keep what is animated on a clip (keyframes on any property, effect settings, text motion, motion presets) as a move of your own, for every project. Put it on other clips with the edit op use_move {move, clips, fit}: timed to each clip's length, position and scale relative to each clip.", obj(json!({"clip": json!({"description": "the clip whose animation to keep"}), "name": s("name for the move"), "description": s("what it does, for later")}), &["clip", "name"])),
        t("save_design", "Keep a group of clips (layers, timing, media, text, their animation) as a design of your own, for every project. Place it with the edit op use_design {design, at, x, y, text: {old: new}}.", obj(json!({"clips": json!({"description": "the clips that make the design"}), "name": s("name for the design"), "description": s("what it is, for later")}), &["clips", "name"])),
        t("plugins", "Plugins: effects, generators, transitions, importers, exporters, panels (sandboxed WebAssembly) and OpenFX. Fields: action (list, search, install, enable, disable, uninstall, create, import, export, panel, ofx, add_index, refresh), query, id, path, name, pixel, params, source. Install only after the person says yes. Installed effects and transitions are used by name with the ops apply and transition.", obj(json!({"action": s("list, search, install, enable, disable, uninstall, create, import, export, panel, ofx, add_index, refresh"), "query": s("search words"), "id": s("plugin id (install, enable, disable, uninstall, create)"), "path": s("install: a .wasm file; import: the file; export: where to write"), "plugin": s("plugin id"), "exporter": s("exporter id"), "panel": s("panel id"), "button": s("panel button to press"), "values": {"type": "object", "description": "panel slider and toggle values"}, "name": s("create: the effect's name"), "description": s("create: one line"), "pixel": s("create: Rust body returning [r,g,b,a] (premultiplied linear); p.float(\"id\"), p.colour(\"id\")..."), "params": {"type": "array", "description": "create: parameters"}, "source": s("create: a whole lib.rs using studio_plugin_api"), "wgsl": s("create: optional fn effect(pos) -> vec4<f32> for the GPU"), "animated": {"type": "boolean", "description": "create: changes over time (ctx.time)"}, "on": {"type": "boolean", "description": "ofx: turn OpenFX loading on or off"}, "url": s("add_index: https link to a plugin index"), "to_timeline": {"type": "boolean", "description": "import: also put it on the timeline"}}), &[])),
        t("layout", "The editor window's panels (only in the Zornov window). Fields: action (get, open, close, dock, float, pop_out, dock_back, workspace, save_workspace, delete_workspace, reset, set), panel, target, zone, name, tree. Change the layout only when asked or when a panel you need is closed.", obj(json!({"action": s("get, open, close, dock, float, pop_out, dock_back, workspace, save_workspace, delete_workspace, reset, set"), "panel": s("library, viewer, inspector, timeline, sound, scopes, review, record, editorial, ai"), "target": s("dock: the panel to dock against, or window for the window's edge"), "zone": s("dock: left, right, top, bottom or tab"), "name": s("workspace name"), "tree": json!({"type": "object", "description": "set: a tree as get returns it"})}), &[])),
        t("automation", "Macros and batch work. Fields: action (record, stop, run, macros, delete, batch, shortcuts, workspaces), name, description, clips, macro, ops, export, projects.", obj(json!({"action": s("macros, record, stop, run, delete, batch, shortcuts, workspaces"), "name": s("macro name"), "description": s("what the macro does"), "clips": json!({"description": "clip ids, or all, main, video, titles, sound"}), "macro": s("batch: macro to run"), "ops": {"type": "array", "description": "batch: edit ops to apply"}, "export": json!({"description": "batch: export preset name or export arguments"}), "projects": {"type": "array", "items": {"type": "string"}, "description": "batch: project folders"}}), &[])),
        t("made", "The moves and designs you (or other agents) have made and saved, with their descriptions.", obj(json!({}), &[])),
        t("match", "Find moments in the footage that look like a frame: for match cuts (a wheel into a jet intake, a headlight into the sun) and for 'more like this'. The frame is a clip's last frame (\"edge\": \"in\" for its first) or media at t. by: shape (where the strong lines and edges are), look (colour and light), or both. Returns ranked moments (media, t in the file, score) and a contact sheet: the query first, then the matches.", obj(json!({"clip": json!({"description": "a clip (id, \"@time\", name…)"}), "edge": s("out (default: the clip's last frame) or in"), "media": s("instead of clip: a media file"), "t": num("with media: seconds into the file"), "in": json!({"description": "media to search (a name or a list); default every video"}), "by": s("shape, look or both (default)"), "count": int("default 8"), "step": num("seconds between sampled frames, default 0.5")}), &[])),
        t("listen", "Hear a song, a sound or a clip's sound as an editor does: songs (sections, drops, bpm, beats), short sounds (shape, attack, tail), and always what it is by ear, faults and loudness. Fields: media, clip, item, mix (hear the timeline mix), from, to. More: guide {topic: \"tool:listen\"}.", obj(json!({"media": s("a media id or name"), "clip": json!({"description": "a clip (id, \"@time\", name…): times come back on the timeline"}), "item": s("a library sound by name (catalogue audio)"), "mix": json!({"type": "boolean", "description": "the timeline mix from..to (timeline seconds): voice against music, masking"}), "from": num("seconds, optional"), "to": num("seconds, optional")}), &[])),
        t(
            "capture",
            "Capture and live production: record cameras, microphones, screens, voice-over, teleprompter, live vision mixer and streaming, card ingest, sync. Real devices need user_approved: true, which only the person's own request gives; `test:` sources need nothing. Fields: action (devices, permissions, record, stop, status, voiceover, prompter, live, ingest, cards, verify, sync), inputs, seconds, at, to_timeline, do, sources, stream. More: guide {topic: \"tool:capture\"}.",
            obj(
                Value::Object(
                    [
                        ("action", json!({"type": "string", "enum": ["devices", "permissions", "record", "stop", "status", "voiceover", "prompter", "live", "ingest", "cards", "verify", "sync"]})),
                        ("do", s("prompter: load, play, pause, advance, hear, jump, back, settings, listen, frame, status, unload; live: open, take, cut, preview, scene, title, title_off, mute, unmute, gain, state, frame, multiview, run, stream, stop_stream, record_program, replay_save, replay_play, close")),
                        ("session", int("a recording, voice-over or live session")),
                        ("user_approved", json!({"type": "boolean", "description": "the person asked for this real device / stream / permission question"})),
                        ("request", s("permissions: camera, microphone or screen")),
                        ("inputs", json!({"type": "array", "items": {"type": "object"}, "description": "record: [{label, video, audio, cursor}] source ids from devices (camera:…, mic:…, screen:…, window:…) or test: sources"})),
                        ("video", json!({"description": "record: one picture source id; sync: the camera clip (id) or media"})),
                        ("audio", json!({"description": "record: one sound source id; sync: the separately recorded sound (media)"})),
                        ("name", s("file name base / session name")),
                        ("seconds", num("how long (record, voiceover, advance, run, replay_save)")),
                        ("at", num("timeline seconds")),
                        ("to_timeline", json!({"type": "boolean", "description": "place the result on the timeline"})),
                        ("width", int("picture width")),
                        ("height", int("picture height")),
                        ("fps", num("frame rate")),
                        ("mic", s("voiceover / listen: microphone id (mic:default) or test:voice")),
                        ("count_in", int("voiceover: beats of count-in (0, 3, 4)")),
                        ("bpm", num("count-in tempo")),
                        ("gain", num("input gain 0..2")),
                        ("latency_ms", num("voiceover: the microphone's delay to take off")),
                        ("click", json!({"type": "boolean", "description": "voiceover: play the count-in clicks on the speakers"})),
                        ("text", s("prompter load: the script text; title: the words")),
                        ("scene", s("prompter load: a scene id of the story, or all")),
                        ("wpm", num("prompter: words a minute")),
                        ("mode", s("prompter: pace, follow or manual")),
                        ("mirror", json!({"type": "boolean"})),
                        ("size", num("prompter text size, px")),
                        ("words", json!({"type": "array", "items": {"type": "string"}, "description": "prompter hear: the words just heard"})),
                        ("media", json!({"description": "prompter listen: a recording played as if live"})),
                        ("sources", json!({"type": "array", "items": {"type": "object"}, "description": "live open: [{name, video, audio}]"})),
                        ("to", json!({"description": "live: a source (name or number), black, a saved scene, {pip:{main,inset,corner,scale}}, {split:[a,b]}, {grid:[…]}; ingest: the working folder"})),
                        ("transition", s("cut, mix, dip, wipe[_left|_up|_down], push[_right|_up|_down]")),
                        ("ms", int("transition / title fade, milliseconds")),
                        ("layout", json!({"description": "live scene: the layout to save"})),
                        ("kind", s("live title: lower, centre or bug")),
                        ("sub", s("live title: second line")),
                        ("source", json!({"description": "live mute / gain: the source"})),
                        ("stream", json!({"description": "live open: rtmp:// or srt:// address(es) to stream to (needs user_approved)"})),
                        ("url", s("live stream: rtmp://host/app/key or srt://host:port?streamid=…")),
                        ("replay_secs", num("live open: seconds of program kept for replay")),
                        ("record_program", json!({"type": "boolean"})),
                        ("iso", json!({"type": "boolean", "description": "live open: record every source on its own"})),
                        ("realtime", json!({"type": "boolean", "description": "run on the wall clock (default for real devices and streams)"})),
                        ("from", s("ingest: the card or folder")),
                        ("backup", json!({"description": "ingest: folder(s) on other drives for second copies"})),
                        ("import", json!({"type": "boolean"})),
                        ("dir", s("verify: an ingested folder")),
                        ("apply", json!({"type": "boolean", "description": "sync: place the sound and mute the camera's own"})),
                        ("label", s("record: name of a single input")),
                        ("cursor", json!({"type": "boolean", "description": "record: the pointer highlight on a screen or window (on by default for them)"})),
                        ("trim", json!({"type": "boolean", "description": "voiceover: trim silence from both ends (default true)"})),
                        ("volume", num("voiceover: clip volume, dB")),
                        ("lane", s("voiceover / sync: the lane to place on")),
                        ("speed", num("live replay_play: playback speed, 0.5 is half speed")),
                        ("as", s("live replay_play: the name of the new source")),
                        ("title", int("live title_off: which title (default all)")),
                        ("word", int("prompter jump: the word number")),
                        ("no_backup", json!({"type": "boolean", "description": "ingest: no second copy (only when the person says so)"})),
                        ("allow_same_drive", json!({"type": "boolean", "description": "ingest: allow a copy on the card's own drive"})),
                        ("mute_camera", json!({"type": "boolean", "description": "sync: mute the camera clip's own sound (default true)"})),
                    ]
                    .into_iter()
                    .map(|(k, v): (&str, Value)| (k.to_owned(), v))
                    .collect(),
                ),
                &["action"],
            ),
        ),
        t("check", "Find what is wrong before exporting: gaps, clips past their footage, titles off the canvas, unknown effects, audio clipping and loudness, black or flat frames, a sound repeated too often, one music track under several moods. Each problem has a time, a clip and a fix. Fields: frames, fix (mend what Zornov can).", obj(json!({"frames": {"type": "boolean", "description": "also look at the picture (default true)"}, "fix": {"type": "boolean", "description": "mend what Zornov can by itself (titles over pictures, off the frame or under app buttons, titles past the picture, loudness), then check again"}}), &[])),
        t("review", "The director's review: the whole film as a viewer sees it, for what makes it lifeless though `check` finds nothing (backdrop held, even cuts, cuts mid-sentence, gliding characters, copied acting, things said but not shown, places without sound, repeated sounds, one track for several moods). Notes with times, clips and fixes. Run it before saying the film is done and act on every note. When it passes it also sends a full-detail contact sheet of the cut for the last look. Field: frames.", obj(json!({"frames": {"type": "boolean", "description": "also sample the cut's frames (default true)"}}), &[])),
        t("export", "Render a file. Field preset (Social 1080p, 4K master, Vertical, GIF, Intermediate, PNG sequence, HEVC 10-bit, Audio only, ProRes / EXR / DPX / DNxHR masters, Lottie), path, range, wait (false: poll export_status), qc, spec. More: guide {topic: \"tool:export\"}.", obj(json!({"preset": s("preset name"), "path": s("output file (default: the Videos folder)"), "range": {"type": "array", "items": {"type": "number"}, "description": "[start, end] seconds"}, "wait": {"type": "boolean"}, "qc": {"type": "boolean", "description": "check the file afterwards (default true): picture levels, photosensitive flashes, black and frozen frames, sound peaks, loudness, dropouts; the verdict and the worst problems come back and the full report is written beside the file"}, "spec": s("delivery spec to judge it by (youtube, tiktok, netflix, broadcast-r128 ...); see deliver")}), &[])),
        t("queue", "The render queue: background exports with priority, pause, presets, optionally shared with render nodes. Fields: action (add, list, pause, resume, cancel, retry, priority, clear, run, notices, presets, save_preset, nodes, add_node, remove_node, discover), job, name, spec, preset, path, range, qc, bundle, priority, distributed.", obj(json!({"action": s("add, list, pause, resume, cancel, retry, priority, clear, run, notices, presets, save_preset, nodes, add_node, remove_node, discover"), "job": int("a job id"), "name": s("job or preset name; node name"), "spec": s("delivery spec id"), "delivery": s("same as spec"), "preset": s("export preset"), "path": s("output file or package folder"), "range": json!({"type": "array", "items": {"type": "number"}, "description": "[start, end] seconds"}), "qc": json!({"type": "boolean"}), "bundle": json!({"type": "boolean", "description": "also make the deliverables bundle around the master"}), "priority": int("higher runs first"), "distributed": json!({"type": "boolean", "description": "share the render with render nodes"}), "chunk_secs": num("seconds per piece"), "size": json!({"type": "array", "items": {"type": "integer"}, "description": "[width, height] overriding the preset or spec"}), "from_preset": s("a saved queue preset"), "wait": json!({"description": "run: wait for the queue to empty (default true); discover: seconds to listen"}), "addr": s("node address HOST:PORT"), "token": s("node pairing token")}), &["action"])),
        t("qc", "Automated quality control of a finished video file (default: the last export): levels, flashes, black and frozen frames, loudness, clipping, dropouts, missing captions. Findings have a time, severity and fix. Fields: path, spec (youtube, tiktok, netflix, broadcast-r128...), preflight (check the project without a file), safe_area. Fix every fail before saying a delivery is ready.", obj(json!({"path": s("video file (default: the last export)"), "spec": s("delivery spec id"), "preflight": json!({"type": "boolean"}), "project_context": json!({"type": "boolean", "description": "use the project's captions and speech for the caption and dialogue checks (default true)"}), "safe_area": json!({"type": "boolean", "description": "preflight: also judge titles cut by the frame edge or under the app buttons (default true)"}), "max_seconds": num("check only the first seconds")}), &[])),
        t("deliver", "Render for a platform or mastering, check it, report. No arguments: the specs (youtube, youtube-shorts, instagram-reel, tiktok, netflix, broadcast-r128, dcp-smpte, imf-netflix, web-preview...). Fields: spec, name, path, range, qc, bundle, queue, distributed.", obj(json!({"spec": s("delivery spec id"), "list": json!({"type": "boolean"}), "name": s("file or package name"), "path": s("output file or folder"), "range": json!({"type": "array", "items": {"type": "number"}, "description": "[start, end] seconds"}), "qc": json!({"type": "boolean"}), "bundle": json!({"type": "boolean"}), "queue": json!({"type": "boolean"}), "distributed": json!({"type": "boolean"}), "priority": int("queue priority"), "chunk_secs": num("seconds per piece"), "size": json!({"type": "array", "items": {"type": "integer"}, "description": "[width, height] overriding the spec"})}), &[])),
        t("bundle", "The deliverables bundle in one folder with a manifest: master, proxy, mix WAV, stems, captions, thumbnails, poster, cutdowns. Fields: name, out, spec, master, proxy, audio, stems, captions, thumbnails, poster, cutdowns ([15, 30, 60]), verticals.", obj(json!({"name": s("bundle name"), "out": s("folder (default: deliveries/ in the project)"), "spec": s("delivery spec for the master and cutdowns"), "master": json!({"type": "boolean"}), "proxy": json!({"type": "boolean"}), "audio": json!({"type": "boolean"}), "stems": json!({"type": "boolean"}), "captions": json!({"type": "boolean"}), "thumbnails": json!({"description": "false, or how many (default 8)"}), "poster": json!({"type": "boolean"}), "cutdowns": json!({"type": "array", "items": {"type": "number"}, "description": "lengths in seconds"}), "verticals": json!({"type": "boolean"}), "size": json!({"type": "array", "items": {"type": "integer"}, "description": "[width, height] for the master and cutdowns (the proxy is half)"})}), &[])),
        t("cutdown", "A shorter version of the programme (15, 30, 60 s) as its own project in cutdowns/; yours is never changed. Fields: seconds, name, keep, drop (clip ids), vertical, render, out, spec, qc. Look at the result and rerun with keep / drop.", obj(json!({"seconds": num("target length"), "name": s("name of the cutdown"), "keep": ids("main-lane clips to keep"), "drop": ids("main-lane clips to remove"), "vertical": json!({"type": "boolean"}), "render": json!({"type": "boolean"}), "out": s("folder for the video"), "spec": s("delivery spec for the render"), "qc": json!({"type": "boolean"}), "size": json!({"type": "array", "items": {"type": "integer"}, "description": "[width, height] of the render"})}), &["seconds"])),
        t("export_status", "Progress of an export started with wait=false.", obj(json!({"export": int("export id")}), &["export"])),
        t("editorial", "The look-ups an editor makes while cutting. Field ask: match_frame, reverse_match, gaps, duplicates, index, sequences, lanes, takes, transcript, find_takes; with t, lane, media, sequence, min, kind, scene, clip, line, limit.", obj(json!({"ask": {"type": "string", "enum": ["match_frame", "reverse_match", "gaps", "duplicates", "index", "sequences", "lanes", "takes", "transcript", "find_takes"]}, "t": num("seconds (timeline for match_frame, into the file for reverse_match)"), "lane": s("lane name or id"), "media": s("media id or name"), "sequence": s("sequence id or name"), "min": num("seconds"), "kind": s("index rows of one kind"), "scene": s("scene number"), "clip": int("clip id"), "line": s("a line of the script"), "limit": int("most takes to list (default 10)")}), &["ask"])),
        t("edit_schema", "The model's raw edit operations, for `raw` ops (rarely needed). Also the fields of the simple ops: `ops` (a list of op names, or ops written out to be checked without running them; `op` for one) answers with what each takes; `long: true` lists every op with all its fields.", obj(json!({"ops": json!({"description": "op names, or ops ({\"op\": name, ...}) to check without running them"}), "op": s("one op's name"), "long": json!({"type": "boolean", "description": "every op with all its fields, and the raw edits"})}), &[])),
    ]
    .into_iter()
    .chain(super::sound_dept::tools())
    .collect::<Vec<_>>()
}

pub(crate) fn result_of(r: Result<Vec<Content>, String>) -> Value {
    match r {
        Ok(parts) => json!({"content": parts.into_iter().map(|c| match c {
            Content::Text(t) => json!({"type": "text", "text": t}),
            Content::Jpeg(b) => json!({"type": "image", "mimeType": "image/jpeg", "data": base64(&b)}),
        }).collect::<Vec<_>>()}),
        Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
    }
}

/// Handles one JSON-RPC message; `None` for notifications.
pub fn handle(session: &mut Session, msg: &Value) -> Option<Value> {
    handle_with(msg, &mut |name, args| {
        let r = super::guard::run(name, || session.run(name, args));
        result_of(r)
    })
}

/// Handles one JSON-RPC message, tool calls through `call` (which returns
/// the MCP result); `None` for notifications.
pub fn handle_with(msg: &Value, call: &mut dyn FnMut(&str, &Value) -> Value) -> Option<Value> {
    handle_with_list(msg, call, &mut tools)
}

/// [`handle_with`] with the tool list from `list`: a helper process lists
/// the tools of the Studio it is attached to, not its own.
pub fn handle_with_list(msg: &Value, call: &mut dyn FnMut(&str, &Value) -> Value, list: &mut dyn FnMut() -> Vec<Value>) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL),
            "capabilities": {"tools": {}, "resources": {}},
            "serverInfo": {"name": "studio", "version": env!("CARGO_PKG_VERSION")},
            "instructions": INSTRUCTIONS,
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": list()})),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            Ok(call(name, &args))
        }
        "resources/list" => Ok(json!({"resources": [{"uri": "studio://guide", "name": "Zornov guide", "mimeType": "text/markdown"}]})),
        "resources/read" => Ok(json!({"contents": [{"uri": "studio://guide", "mimeType": "text/markdown", "text": GUIDE}]})),
        _ if id.is_none() => return None,
        other => Err(json!({"code": -32601, "message": format!("unknown method {other}")})),
    };
    let id = id?;
    Some(match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e}),
    })
}

/// Serves MCP on stdin / stdout until stdin closes.
pub fn serve_stdio(mut session: Session) {
    serve_lines(&mut |name, args| {
        let r = super::guard::run(name, || session.run(name, args));
        result_of(r)
    });
}

/// MCP on stdin / stdout, tool calls through `call`.
pub fn serve_lines(call: &mut dyn FnMut(&str, &Value) -> Value) {
    serve_lines_listing(call, &mut tools)
}

/// [`serve_lines`] listing the tools `list` gives.
pub fn serve_lines_listing(call: &mut dyn FnMut(&str, &Value) -> Value, list: &mut dyn FnMut() -> Vec<Value>) {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(Value::Array(batch)) => {
                let rs: Vec<Value> = batch.iter().filter_map(|m| handle_with_list(m, call, list)).collect();
                (!rs.is_empty()).then(|| Value::Array(rs))
            }
            Ok(msg) => handle_with_list(&msg, call, list),
            Err(e) => Some(json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("parse error: {e}")}})),
        };
        if let Some(r) = reply {
            let _ = writeln!(out, "{r}");
            let _ = out.flush();
        }
    }
}

/// Standard base64 (for image content).
pub fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    s
}

/// Standard base64 back to bytes; `None` on anything else.
pub fn unbase64(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let s = s.trim_end_matches('=').as_bytes();
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    for chunk in s.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= val(c)? << (18 - 6 * i);
        }
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..chunk.len().saturating_sub(1)]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instructions_stay_one_short_line() {
        assert!(INSTRUCTIONS_LEN < 200 && !INSTRUCTIONS.contains('\n'), "copied into every tool's description by some clients: {INSTRUCTIONS_LEN} chars");
        assert!(INSTRUCTIONS.contains("`guide`"), "still points at the guide");
    }

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        for b in [&b""[..], b"f", b"fo", b"foo", b"foobar", &[0, 255, 7, 128]] {
            assert_eq!(unbase64(&base64(b)).as_deref(), Some(b));
        }
    }

    #[test]
    fn tool_descriptions_are_short_and_their_long_forms_exist() {
        let tools = tools();
        let mut total = 0;
        for t in &tools {
            let (name, d) = (t["name"].as_str().unwrap(), t["description"].as_str().unwrap());
            total += d.len();
            assert!(d.len() <= 1_600, "{name}: {} chars; keep the description short and move detail to tool_reference.md", d.len());
            // A description that points at its long form must have one.
            if d.contains("topic: \"tool:") {
                let long = super::super::guide_topic(&format!("tool:{}", name)).unwrap_or_else(|e| panic!("{name}: {e}"));
                assert!(long.len() > d.len() / 2, "{name}: the long form is thinner than the description");
            }
        }
        assert!(total < 28_000, "all {} tool descriptions are {total} chars; they ride in every request", tools.len());
    }

    #[test]
    fn the_core_prompt_stays_under_its_budget() {
        // The system prompt of the API brain and the end of every `guide` read.
        assert!(super::super::GUIDE_CORE.len() <= 6_200, "guide_core.md is {} bytes; it was 11372 before the capability map took over the lists", super::super::GUIDE_CORE.len());
        for must in ["Plan, then build", "Library", "3D means real 3D", "Finish the job", "Report what is true"] {
            assert!(super::super::GUIDE_CORE.contains(must), "{must} missing from the core");
        }
    }

    #[test]
    fn handshake_lists_tools_and_guide() {
        let mut s = Session::default();
        let init = handle(&mut s, &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": PROTOCOL}})).unwrap();
        assert_eq!(init["result"]["serverInfo"]["name"], "studio");
        assert!(handle(&mut s, &json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).is_none());
        let list = handle(&mut s, &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).unwrap();
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        for n in ["guide", "summary", "edit", "frame", "export", "catalogue"] {
            assert!(names.contains(&n), "{n} missing");
        }
        let g = handle(&mut s, &json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "guide", "arguments": {}}})).unwrap();
        assert!(g["result"]["content"][0]["text"].as_str().unwrap().contains("Skills"), "the core guide lists the skills");
    }
}
