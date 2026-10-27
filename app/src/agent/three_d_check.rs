// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The 3D critic: what a 3D lead would say about every hero model in the
//! film before it ships, found from the models themselves, the lights, the
//! cameras and the brief, not from how the frames happen to look.
//!
//! For each model clip that stays on screen it asks:
//! * is it designed, or a primitive (one or two pieces, one material, a
//!   plain outline in every view)?
//! * do its edges catch light (bevelled), or is every corner a razor?
//! * is the surface smooth, or visibly faceted?
//! * does it have a material of its own, or the stock grey?
//! * are the parts the brief and the plan name there: a lid, a strip of
//!   light, a button, the earbuds in the case, and does a part that has to
//!   move have a bone with a pivot that is actually posed?
//! * is a second model clip standing in for the same object in another
//!   state (the case "closing" as a different model)?
//! * is a place made of many separate primitive clips (a cube per
//!   pilaster, a cube per window frame) instead of one model with named
//!   parts; is a character assembled from cones and cubes instead of the
//!   character base; is a walk keyed pose by pose where `act` would do?
//! * is it lit by more than the default rig, seen by a camera that moves,
//!   with depth of field?
//! * does it match the reference picture made for it?
//!
//! Every note is concrete (the clip, the number, the missing part) and
//! carries its fix. `review` stays "not done" while any is open.

use super::motion_check::Problem;
use crate::view;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use studio_mesh::Mesh;
use studio_model as m;
use studio_quality::hero::{self, Craft};

/// Every kind this critic reports; they all start with `3d_`.
pub const KINDS: &[&str] = &["3d_no_model", "3d_state_swap", "3d_missing_part", "3d_missing_emissive", "3d_part_static", "3d_primitive", "3d_no_bevel", "3d_faceted", "3d_default_material", "3d_default_light", "3d_flat_light", "3d_no_camera_move", "3d_no_dof", "3d_reference_mismatch", "3d_many_primitives", "3d_character_from_primitives", "3d_hand_keyed_motion", "3d_buried", "3d_floating", "3d_flat_key", "3d_source_no_effect", "3d_flat_source", "3d_air_missing", "3d_blown_highlights", "3d_over_bloom", "3d_crushed_blacks", "3d_muddy_blacks", "3d_no_separation", "3d_holes", "3d_floating_pieces", "3d_smeared_texture", "3d_flat", "3d_extruded", "3d_open", "3d_sunk", "3d_tips"];

/// Kinds the AI may set aside itself, with a line in the notes saying why
/// (`3d-ok: primitive, no_dof - the brief asks for a flat, low-poly look`).
/// The ones that are about the brief being met are never set aside.
const ACCEPTABLE: &[&str] = &["primitive", "no_bevel", "faceted", "default_material", "default_light", "flat_light", "no_camera_move", "no_dof", "reference_mismatch", "many_primitives", "character_from_primitives", "hand_keyed_motion", "floating", "flat_key", "blown_highlights", "over_bloom", "crushed_blacks", "muddy_blacks", "no_separation", "flat_source", "flat", "extruded", "open", "sunk", "tips"];

/// What was measured on one model file (kept by file: its name is its
/// content).
pub struct Info {
    pub craft: Craft,
    /// Flat, an extruded cut-out, open: is it a real solid.
    pub solid: solidity::Solidity,
    /// How sound a textured, boneless surface is (a reconstruction or a
    /// scan): holes, loose pieces, stretched texture. None for the rest.
    pub health: Option<Health>,
    pub bones: Vec<String>,
    pub materials: Vec<String>,
    /// The brightest glow channel over the materials.
    pub glow: f32,
}

/// What the surface check of a reconstructed model found.
#[derive(Clone, Debug, Default)]
pub struct Health {
    /// Edges with one face: holes.
    pub open_edges: usize,
    /// Pieces under 1% of the surface, not joined to the body.
    pub floating: usize,
    /// Area-weighted texture stretch (1 = none).
    pub stretch: f32,
    /// Spread of texture detail over the surface (0 = even).
    pub spread: f32,
}

impl Info {
    pub fn of(mesh: &Mesh) -> Info {
        let reconstructed = mesh.rig.is_none() && mesh.materials.iter().any(|m| m.texture.is_some()) && mesh.materials.len() <= 2 && mesh.triangles() <= 150_000;
        let health = reconstructed.then(|| {
            let t = studio_quality::topo::analyse(mesh);
            let u = studio_quality::uv::analyse(mesh);
            Health { open_edges: t.boundary_edges, floating: t.specks, stretch: u.stretch, spread: u.density_cov }
        });
        Info { health, craft: hero::measure(mesh), solid: solidity::Solidity::of(mesh), bones: mesh.rig.as_ref().map(|r| r.bones.iter().filter(|b| b.deform).map(|b| b.name.clone()).collect()).unwrap_or_default(), materials: mesh.materials.iter().map(|x| x.name.clone()).collect(), glow: mesh.materials.iter().flat_map(|x| x.emissive).fold(0.0, f32::max) }
    }
}

fn cache() -> &'static Mutex<HashMap<String, Arc<Info>>> {
    static C: OnceLock<Mutex<HashMap<String, Arc<Info>>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

pub(super) fn info_of_file(file: &str) -> Option<Arc<Info>> {
    if let Some(i) = cache().lock().ok()?.get(file) {
        return Some(i.clone());
    }
    let mesh = studio_mesh::io::load(std::path::Path::new(file)).ok()?;
    let info = Arc::new(Info::of(&mesh));
    let mut c = cache().lock().ok()?;
    if c.len() > 400 {
        c.clear();
    }
    c.insert(file.to_owned(), info.clone());
    Some(info)
}

/// A model clip that is in the film for a while.
pub struct Hero {
    pub id: u32,
    pub name: String,
    pub start: f64,
    pub end: f64,
    pub file: String,
    pub info: Arc<Info>,
    /// Keys on bone turns (poses) or a running action.
    pub posed: bool,
    /// The clip's own colour, finish or glow, set over the model's.
    pub look_set: bool,
    pub glow_set: bool,
    /// How well a front-on render matches the reference picture made for it
    /// (0..1), when there is one.
    pub reference: Option<f32>,
}

pub struct Light {
    pub start: f64,
    pub end: f64,
    pub ambient: bool,
}

pub struct Camera {
    pub start: f64,
    pub end: f64,
    pub moves: bool,
    pub dof: bool,
}

/// A model clip that is a single primitive shape (a cube, a cone, a
/// cylinder): one or two pieces, one or two materials, no bones, few
/// triangles.
pub struct PrimClip {
    pub id: u32,
    pub name: String,
    pub start: f64,
    pub end: f64,
}

/// A model clip moved by keys on its bones.
pub struct KeyedClip {
    pub id: u32,
    pub name: String,
    pub start: f64,
    pub end: f64,
    /// Keys on bone tracks, all bones and axes together.
    pub keys: usize,
    /// Different bones keyed.
    pub bones: usize,
    /// Different moments with a key (poses).
    pub poses: usize,
    /// Whether it also runs a named action (`act`).
    pub acts: bool,
}

#[derive(Default)]
pub struct Env {
    pub lights: Vec<Light>,
    pub cameras: Vec<Camera>,
    /// Every model clip in the film (id, name, start, end), heroes or not.
    pub models: Vec<(u32, String, f64, f64)>,
    /// The model clips that are single primitives.
    pub prims: Vec<PrimClip>,
    /// The model clips posed by hand-written bone keys.
    pub keyed: Vec<KeyedClip>,
}

#[derive(Default)]
pub struct Words {
    /// The person's request, as the AI was given it.
    pub brief: String,
    /// The AI's own plan (`notes`).
    pub notes: String,
}

// ------------------------------------------------------------------ words

fn stem(w: &str) -> String {
    let mut w = w.to_lowercase();
    for suf in ["ing", "ed", "es", "s"] {
        if w.len() > suf.len() + 2 && w.ends_with(suf) {
            w.truncate(w.len() - suf.len());
            break;
        }
    }
    if w.len() > 3 && w.ends_with('e') {
        w.pop();
    }
    w
}

/// Lower-case word stems of a name or sentence; camelCase and snake_case
/// split.
fn tokens(s: &str) -> Vec<String> {
    let mut spaced = String::new();
    let mut prev_lower = false;
    for c in s.chars() {
        if c.is_uppercase() && prev_lower {
            spaced.push(' ');
        }
        prev_lower = c.is_lowercase();
        spaced.push(c);
    }
    spaced.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(stem).collect()
}

/// Words that say nothing about what a model is.
const PLAIN: &[&str] = &["model", "mesh", "3d", "hero", "object", "product", "clip", "shot", "scene", "real", "the", "and", "with", "for", "from", "into", "part", "piece", "main", "new", "copy", "final", "open", "close", "clos", "shut", "fold", "unfold", "ajar", "state", "before", "after", "default", "cube", "sphere", "shape"];

/// Words saying a thing lights up.
const GLOW_WORDS: &[&str] = &["glow", "emissive", "illuminat", "backlit", "neon", "luminous", "glowing", "lit"];
/// Verbs saying a part of the object moves.
const MOVE_VERBS: &[&str] = &["open", "close", "clos", "unfold", "fold", "swing", "flip", "lift", "pop", "slide", "extend", "retract", "deploy", "tilt", "raise", "lower"];
const OPENISH: &[&str] = &["open", "ajar", "unfold"];
const CLOSEISH: &[&str] = &["clos", "shut", "fold"];

fn stems(list: &[&str]) -> HashSet<String> {
    list.iter().map(|w| stem(w)).collect()
}

fn subject_words(name: &str) -> Vec<String> {
    let plain = stems(PLAIN);
    tokens(name).into_iter().filter(|w| w.len() >= 3 && !plain.contains(w) && vocab::known_anywhere(w).is_none() && !w.chars().all(|c| c.is_ascii_digit())).collect()
}

fn sentences(text: &str) -> Vec<String> {
    text.split(['.', '!', '?', '\n', ';']).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect()
}

/// The nouns a brief says are "real 3D": "the earbud is real 3D",
/// "a real 3D model of the earbud case".
fn real_3d_nouns(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut out: Vec<String> = Vec::new();
    for key in ["is real 3d", "are real 3d", "is a real 3d", "are real 3d models"] {
        let mut from = 0;
        while let Some(i) = lower[from..].find(key) {
            let at = from + i;
            if let Some(w) = lower[..at].split(|c: char| !c.is_alphanumeric()).rfind(|w| !w.is_empty()) {
                out.push(stem(w));
            }
            from = at + key.len();
        }
    }
    for key in ["3d model of ", "3d models of ", "3d model for "] {
        let mut from = 0;
        while let Some(i) = lower[from..].find(key) {
            let at = from + i + key.len();
            let tail = &lower[at..];
            let end = tail.find(['(', ',', '.', ';', ':', '\n']).unwrap_or(tail.len());
            // The head noun of "the earbud case" is the last word.
            if let Some(w) = tail[..end].split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).next_back() {
                out.push(stem(w));
            }
            from = at;
        }
    }
    let plain = stems(PLAIN);
    out.retain(|w| w.len() >= 3 && !plain.contains(w));
    out.sort();
    out.dedup();
    out
}

fn mentions_3d_model(text: &str) -> bool {
    let l = text.to_lowercase();
    ["real 3d", "3d model", "3d product", "3d object"].iter().any(|k| l.contains(k))
}

/// What the AI has set aside itself, from lines like
/// `3d-ok: primitive, no_dof - the brief asks for a flat look`.
pub(super) fn accepted(notes: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in notes.lines() {
        let l = line.trim().trim_start_matches(['-', '*', ' ']).to_lowercase();
        let Some(rest) = l.strip_prefix("3d-ok:").or_else(|| l.strip_prefix("3d ok:")) else { continue };
        let mut head = rest;
        for cut in [" - ", " \u{2014} ", " \u{2013} ", " because ", "("] {
            head = head.split(cut).next().unwrap_or(head);
        }
        for k in head.split([',', ' ', ';']) {
            let k = k.trim().trim_start_matches("3d_");
            if ACCEPTABLE.contains(&k) {
                out.push(format!("3d_{k}"));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

// ----------------------------------------------------------------- judging

fn secs(t: i64) -> f64 {
    t as f64 / view::SEC as f64
}

fn overlap(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.1.min(b.1) - a.0.max(b.0)).max(0.0)
}

fn pct(v: f32) -> String {
    format!("{:.0}%", v * 100.0)
}

fn list(v: &[String]) -> String {
    if v.is_empty() { "none".into() } else { v.iter().take(8).cloned().collect::<Vec<_>>().join(", ") }
}

/// Every note about the heroes of a film.
pub fn judge(heroes: &[Hero], env: &Env, words: &Words) -> Vec<Problem> {
    let mut out: Vec<Problem> = Vec::new();
    let text = format!("{}\n{}", words.brief, words.notes);
    let sents = sentences(&text);
    let glow_words = stems(GLOW_WORDS);
    let move_verbs = stems(MOVE_VERBS);

    // The brief wants real 3D and there is none.
    if heroes.is_empty() && env.models.is_empty() && mentions_3d_model(&text) {
        out.push(Problem { kind: "3d_no_model", at: 0.0, clip: 0, problem: "the request or the plan asks for a real 3D model, but the film has no model clip at all".into(), fix: "build it: `model` (blended `parts` or a shape), then `mesh` steps for the form and `rig` for what moves; a stack of flat layers is not a 3D model" });
    }

    // A place made of many primitive clips, a person made of cones and
    // cubes, a walk keyed pose by pose: one note each, with the way out;
    // the pieces are not judged one by one.
    let mut folded: HashSet<u32> = HashSet::new();
    out.extend(place_notes(env, &mut folded));
    out.extend(character_notes(heroes, env, &mut folded));
    out.extend(keyed_notes(env));

    for h in heroes {
        if folded.contains(&h.id) {
            continue;
        }
        let c = &h.info.craft;
        let span = (h.start, h.end);
        let label = format!("model \"{}\" (clip {})", h.name, h.id);
        let at = h.start;

        // A real solid: not a card, a cut-out slab or an open shell.
        out.extend(solidity::notes(h.id, &h.name, at, &h.info.solid));

        // Primitive: few parts, one material, nothing but a plain outline.
        if c.shells <= 2 && c.materials <= 1 && c.bones == 0 && c.plain_outline() && c.triangles < 150_000 {
            out.push(Problem {
                kind: "3d_primitive",
                at,
                clip: h.id,
                problem: format!("{label} is a primitive: {} separate piece{}, {} material{}, {} triangles, no bones and an outline as plain as a ball or a box from every side; a hero object is designed from parts", c.shells, plural(c.shells), c.materials, plural(c.materials), c.triangles),
                fix: "block out each real part as its own shape in the one model (`mesh` `add` with `at` and a `name` for the part's material: body, lid, button, light strip), bevel and subdivide them, give each its own material, then `rig` with `rigid` for what moves",
            });
        }
        // Razor edges.
        if c.sharp_share >= 0.06 && c.triangles < 30_000 {
            out.push(Problem {
                kind: "3d_no_bevel",
                at,
                clip: h.id,
                problem: format!("no bevels on hero object {label}: {} of its edge length is razor-sharp corners (bending more than {:.0} degrees); nothing real has an edge like that and it catches no light", pct(c.sharp_share), hero::SHARP),
                fix: "`mesh` `bevel` (width about 1% of the size, 3 segments), then `subdivide` once if the surface should look moulded",
            });
        } else if c.facet_share >= 0.30 && c.triangles < 3_000 {
            out.push(Problem {
                kind: "3d_faceted",
                at,
                clip: h.id,
                problem: format!("{label} is visibly faceted: {} of its edge length bends between {:.0} and {:.0} degrees on only {} triangles, so curves show as flat planes", pct(c.facet_share), hero::FACET, hero::SHARP, c.triangles),
                fix: "`mesh` `subdivide` (1 or 2 levels) or a higher `detail` on the shapes; bevel hard edges first so they keep their line",
            });
        }
        // A reconstructed or scanned surface: holes, floating pieces, smeared texture.
        if let Some(hl) = &h.info.health {
            if hl.open_edges >= 30 {
                out.push(Problem {
                    kind: "3d_holes",
                    at,
                    clip: h.id,
                    problem: format!("{label} has {} open edges: holes in a surface that should be closed (a reconstruction that lost a part, or a cropped picture)", hl.open_edges),
                    fix: "make it again from a reference picture that shows the whole object, uncropped, one object on a plain background (`generate` `transparent: true`), then `generate` `task: \"model3d\"` with it; at most twice, then keep the better one and say what is off",
                });
            }
            if hl.floating > 0 {
                out.push(Problem {
                    kind: "3d_floating_pieces",
                    at,
                    clip: h.id,
                    problem: format!("{label} has {} small piece{} floating free of the body (a shadow, a second object or background the picture left in)", hl.floating, plural(hl.floating)),
                    fix: "regenerate the reference picture with a clean plain background and nothing else in frame (no shadow, no props), then `generate` `task: \"model3d\"` again; or `mesh` `delete` the loose pieces",
                });
            }
            if hl.stretch > 1.6 || hl.spread > 0.9 {
                out.push(Problem {
                    kind: "3d_smeared_texture",
                    at,
                    clip: h.id,
                    problem: format!("the texture of {label} is smeared: texture stretch {:.2} (clean is under 1.3) and detail spread {:.2} (even is under 0.6), so colour streaks across the surface", hl.stretch, hl.spread),
                    fix: "`generate` `task: \"model3d\"` again with a sharper, evenly lit reference picture and a larger `texture` (2048); a picture with hard shadows or glare bakes them into the colour",
                });
            }
        }
        // Stock material.
        if c.materials > 0 && c.default_materials == c.materials && !h.look_set && c.textured_materials == 0 && c.emissive_materials == 0 {
            out.push(Problem {
                kind: "3d_default_material",
                at,
                clip: h.id,
                problem: format!("{label} has only the default material (flat grey, 50% rough, not metal, no glow, no texture) on {} material slot{}", c.materials, plural(c.materials)),
                fix: "give each part a real material: `mesh` `material` with `region` or `add` with `name`, `colour`, `metallic`, `roughness` (matte plastic 0.5-0.7, metal 1 with 0.2-0.4), `emissive` for what lights up, a `texture` from a generated image for grain or print",
            });
        }

        // Parts the brief and the plan name.
        let subject = subject_words(&h.name);
        // Ground, water, sky and lettering have no parts to miss: "door" in
        // the brief was asked of the sea, "camera" of a title.
        let own_words = tokens(&h.name);
        let scenery = SCENERY.iter().any(|w| own_words.contains(&stem(w)));
        // Sentences about it, and the "It opens..." that follow them. A
        // sentence that names no model is about this one only when it is
        // the only one.
        let mut relevant: Vec<&String> = Vec::new();
        let mut carry = false;
        for s in sents.iter().filter(|_| !scenery) {
            let t = tokens(s);
            let about = if subject.is_empty() { heroes.len() == 1 && t.iter().any(|w| w == "3d" || w == "model") } else { subject.iter().any(|w| t.contains(w)) };
            let low = s.to_lowercase();
            let pronoun = ["it ", "its ", "they ", "this ", "when it "].iter().any(|p| low.starts_with(p));
            let names_a_part = t.iter().any(|w| vocab::known_anywhere(w).is_some());
            if about || (carry && (pronoun || names_a_part)) {
                relevant.push(s);
                carry = true;
            } else {
                carry = false;
            }
        }
        // With one hero, a plan's own parts list is about it.
        if heroes.len() == 1 && !scenery {
            for s in &sents {
                let low = s.to_lowercase();
                if (low.starts_with("parts") || low.starts_with("components") || low.starts_with("pieces") || low.contains("parts:")) && !relevant.contains(&s) {
                    relevant.push(s);
                }
            }
        }
        // What kind of thing it is decides which part words are parts of it.
        let mut about_words: Vec<String> = subject.clone();
        about_words.extend(tokens(&h.name));
        about_words.extend(relevant.iter().flat_map(|s| tokens(s)));
        let domains = vocab::domains_of(&about_words);
        let mut named: Vec<String> = Vec::new(); // parts the sentences and the clip's name give it
        let mut implied_move = false;
        let mut glow = false;
        for s in &relevant {
            for w in tokens(s) {
                if vocab::lookup(&w, &domains).is_some() && !named.contains(&w) {
                    named.push(w.clone());
                }
                if move_verbs.contains(&w) {
                    implied_move = true;
                }
                if glow_words.contains(&w) {
                    glow = true;
                }
            }
            for w in vocab::listed(s) {
                if !named.contains(&w) {
                    named.push(w);
                }
            }
        }
        // A part named in the clip's own name, other than the clip being that part.
        let own = tokens(&h.name);
        let is_part_clip = own.last().is_some_and(|w| vocab::lookup(w, &domains).is_some()) || scenery;
        if !is_part_clip {
            for w in &own {
                if vocab::lookup(w, &domains).is_some() && !named.contains(w) {
                    named.push(w.clone());
                }
            }
        }
        let glows = |w: &str| vocab::lookup(w, &domains).is_some_and(|x| x.1);
        let moves = |w: &str| vocab::lookup(w, &domains).is_some_and(|x| x.0);
        if !is_part_clip {
            let own_names: Vec<String> = h.info.bones.iter().chain(&h.info.materials).cloned().collect();
            let other_names: Vec<String> = env.models.iter().filter(|(id, _, s, e)| *id != h.id && overlap(span, (*s, *e)) > 0.0).map(|(_, n, ..)| n.clone()).collect();
            let mut missing: Vec<String> = Vec::new();
            for w in &named {
                let found = vocab::present_in(w, &own_names) || vocab::present_in(w, &other_names) || (*w == stem("hinge") && !h.info.bones.is_empty()) || (glows(w) && (c.emissive_materials > 0 || h.glow_set));
                if !found {
                    missing.push(w.clone());
                }
            }
            let has_moving_part = own_names.iter().chain(&other_names).flat_map(|n| tokens(n)).any(|w| moves(&w));
            if implied_move && !named.iter().any(|w| moves(w)) && !has_moving_part {
                missing.push("a part that moves (a lid, door, drawer, flap or hatch)".into());
            }
            if !missing.is_empty() {
                out.push(Problem {
                    kind: "3d_missing_part",
                    at,
                    clip: h.id,
                    problem: format!("{} named for {label} but there is no such part: no bone, material or piece of the model is called that (bones: {}; materials: {})", missing.iter().map(|w| format!("\"{}\"", vocab::display(w))).collect::<Vec<_>>().join(", "), list(&h.info.bones), list(&h.info.materials)),
                    fix: "build it as its own shape in the same model (`mesh` `add`, `name` it so the material carries the part's name); if it moves, add a bone named for it with its head at the hinge (`rig` `bones` with `rigid`: true)",
                });
            }
            // A moving part must be a bone that is posed.
            let moving_bones: Vec<&String> = h.info.bones.iter().filter(|b| tokens(b).iter().any(|w| moves(w))).collect();
            if implied_move && !moving_bones.is_empty() && !h.posed {
                out.push(Problem {
                    kind: "3d_part_static",
                    at,
                    clip: h.id,
                    problem: format!("{label} has a moving part ({}) that is never posed: no keys on any bone, so nothing opens or turns", moving_bones.iter().take(3).map(|b| format!("\"{b}\"")).collect::<Vec<_>>().join(", ")),
                    fix: "`pose` the clip with keys on that bone (degrees about its hinge) at the moment it should move, and key it back",
                });
            }
            // Light that is described needs light that is made.
            if (glow || named.iter().any(|w| glows(w))) && c.emissive_materials == 0 && !h.glow_set {
                out.push(Problem {
                    kind: "3d_missing_emissive",
                    at,
                    clip: h.id,
                    problem: format!("the request or plan describes something glowing on {label}, but no material of it is emissive (emissive materials: 0 of {})", c.materials),
                    fix: "`mesh` `material` on that part (a `region` or the `add` that made it) with `emissive` set to the light's colour, and a lamp or glow nearby so it lights what is round it",
                });
            }
        }

        // Light.
        let lit: Vec<&Light> = env.lights.iter().filter(|l| overlap(span, (l.start, l.end)) >= 0.5 * (h.end - h.start)).collect();
        let key_lights = lit.iter().filter(|l| !l.ambient).count();
        if lit.is_empty() {
            out.push(Problem {
                kind: "3d_default_light",
                at,
                clip: h.id,
                problem: format!("{label} is shown with default lighting: there are no light clips on the timeline for {:.1}s-{:.1}s, so it gets the stock key, fill and rim", h.start, h.end),
                fix: "light it on purpose: `lighting` with a preset (three-point, noir, golden hour) or `light` clips: a key from the upper left, a weaker fill opposite, a rim from behind, a soft ambient",
            });
        } else if key_lights < 2 {
            out.push(Problem {
                kind: "3d_flat_light",
                at,
                clip: h.id,
                problem: format!("{label} is lit flat: {} directional light{} ({} ambient) where it is on screen; one light has no shape and no edge light", key_lights, plural(key_lights), lit.len() - key_lights),
                fix: "make it three-point: a key (upper left, shadows on), a rim from behind to outline it, and a fill or ambient at 20-30% (`lighting` three-point does all of it)",
            });
        }
        // Camera.
        let seeing: Vec<&Camera> = env.cameras.iter().filter(|k| overlap(span, (k.start, k.end)) > 0.0).collect();
        if h.end - h.start >= 3.0 {
            if !seeing.iter().any(|k| k.moves) {
                out.push(Problem {
                    kind: "3d_no_camera_move",
                    at,
                    clip: h.id,
                    problem: if seeing.is_empty() { format!("{label} is on screen for {:.1}s with no camera: the default static view", h.end - h.start) } else { format!("{label} is on screen for {:.1}s and the camera never moves in that time", h.end - h.start) },
                    fix: "`camera` with a `move` (a slow orbit of 20-30 degrees, or a push in of a quarter of the distance) eased in and out over the shot",
                });
            }
            if !seeing.iter().any(|k| k.dof) {
                out.push(Problem {
                    kind: "3d_no_dof",
                    at,
                    clip: h.id,
                    problem: format!("{label} is a hero shot with no depth of field: everything is equally sharp, so it reads as a render, not a lens"),
                    fix: "`camera` with `dof`: true, `focus` on the model and a modest `aperture` so the background softens",
                });
            }
        }
        // Reference.
        if let Some(score) = h.reference.filter(|s| *s < 0.5) {
            out.push(Problem {
                kind: "3d_reference_mismatch",
                at,
                clip: h.id,
                problem: format!("{label} compares poorly with its reference picture: the best outline overlap from any side is {:.0}% (it should be over 50%)", score * 100.0),
                fix: "look at the reference and the model side by side (`frame` after a `rotation_y` of 0, 45 and 90) and fix the proportions and the missing parts first; then check again",
            });
        }
    }

    // The same object in another state, as a second model.
    for (i, a) in heroes.iter().enumerate() {
        for b in &heroes[i + 1..] {
            let (first, second) = if a.start <= b.start { (a, b) } else { (b, a) };
            if first.file == second.file || overlap((first.start, first.end), (second.start, second.end)) > 0.05 {
                continue;
            }
            let (ta, tb): (HashSet<String>, HashSet<String>) = (tokens(&first.name).into_iter().collect(), tokens(&second.name).into_iter().collect());
            let plain = stems(PLAIN);
            let shared: Vec<&String> = ta.intersection(&tb).filter(|w| w.len() >= 3 && !plain.contains(*w)).collect();
            if shared.is_empty() {
                continue;
            }
            let state = |t: &HashSet<String>| (OPENISH.iter().any(|w| t.contains(&stem(w))), CLOSEISH.iter().any(|w| t.contains(&stem(w))));
            let (sa, sb) = (state(&ta), state(&tb));
            let states_differ = (sa.0 && sb.1) || (sa.1 && sb.0);
            let (ca, cb) = (&first.info.craft, &second.info.craft);
            let close = |x: f32, y: f32, tol: f32| (x - y).abs() <= tol * x.max(y).max(1e-3);
            let same_build = (0..3).all(|k| close(ca.size[k], cb.size[k], 0.2)) && close(ca.triangles as f32, cb.triangles as f32, 0.2);
            if states_differ || same_build {
                out.push(Problem {
                    kind: "3d_state_swap",
                    at: second.start,
                    clip: second.id,
                    problem: format!("model clips {} (\"{}\", {:.1}s-{:.1}s) and {} (\"{}\", {:.1}s-{:.1}s) are the same object in two states, shown by swapping one model for another ({} and {} triangles); a viewer sees a cut, not the object changing", first.id, first.name, first.start, first.end, second.id, second.name, second.start, second.end, ca.triangles, cb.triangles),
                    fix: "keep one model for the object and animate it: `rig` the moving part with `bones` (head at the hinge) and `rigid`: true, `pose` it open and closed across the film, and delete the duplicate; to show it again later, extend or split that one clip",
                });
            }
        }
    }

    // Objects the brief says are real 3D that nothing in the film is.
    let known: HashSet<String> = env.models.iter().flat_map(|(_, n, ..)| tokens(n)).chain(heroes.iter().flat_map(|h| h.info.bones.iter().chain(&h.info.materials).flat_map(|n| tokens(n)))).collect();
    if !heroes.is_empty() || !env.models.is_empty() {
        for noun in real_3d_nouns(&words.brief) {
            if !known.contains(&noun) {
                let at = heroes.iter().map(|h| h.start).fold(f64::INFINITY, f64::min);
                out.push(Problem {
                    kind: "3d_missing_part",
                    at: if at.is_finite() { at } else { 0.0 },
                    clip: 0,
                    problem: format!("the request says the {noun} is real 3D, but no model, part, bone or material in the film is called \"{noun}\" (models: {})", list(&env.models.iter().map(|(_, n, ..)| n.clone()).collect::<Vec<_>>())),
                    fix: "build it: its own shape(s) in the model it belongs to (`mesh` `add` with a `name`), or a model clip of its own, with its own material",
                });
            }
        }
    }

    // What the AI set aside itself (only the kinds a brief can ask for).
    let set_aside = accepted(&words.notes);
    out.retain(|p| !set_aside.iter().any(|k| k == p.kind));
    out.sort_by(|x, y| x.at.total_cmp(&y.at));
    out
}

/// Model names that are scenery or lettering, not objects with parts.
const SCENERY: &[&str] = &["sea", "ocean", "water", "lake", "river", "wave", "waves", "terrain", "ground", "floor", "land", "landscape", "island", "sky", "cloud", "clouds", "fog", "mountain", "mountains", "hill", "hills", "grass", "sand", "beach", "title", "text", "word", "words", "letters", "lettering", "caption", "logo", "scatter", "world"];

/// Words for the parts of a body.
const BODY_PARTS: &[&str] = &["head", "torso", "chest", "arm", "forearm", "leg", "hand", "foot", "feet", "shoulder", "neck", "hip", "knee", "elbow", "thigh", "shin", "pelvis", "calf", "finger", "belly"];

fn body_words(name: &str) -> Vec<String> {
    let body = stems(BODY_PARTS);
    let mut out: Vec<String> = Vec::new();
    for w in tokens(name) {
        if body.contains(&w) && !out.contains(&w) {
            out.push(w);
        }
    }
    out
}

fn ids_said(list: &[(u32, &str)]) -> String {
    let shown: Vec<String> = list.iter().take(10).map(|(id, n)| format!("{id} \"{n}\"")).collect();
    format!("{}{}", shown.join(", "), if list.len() > 10 { format!(", and {} more", list.len() - 10) } else { String::new() })
}

/// Eight or more primitive model clips on screen together: one place built as layers.
fn place_notes(env: &Env, folded: &mut HashSet<u32>) -> Vec<Problem> {
    const MIN: usize = 8;
    let near = |a: &PrimClip, b: &PrimClip| overlap((a.start, a.end), (b.start, b.end)) >= 1.0;
    let mut members: Vec<&PrimClip> = Vec::new();
    for a in &env.prims {
        if env.prims.iter().filter(|b| b.id != a.id && near(a, b)).count() + 1 >= MIN {
            members.push(a);
        }
    }
    if members.len() < MIN {
        return Vec::new();
    }
    members.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.id.cmp(&b.id)));
    folded.extend(members.iter().map(|m| m.id));
    let listed: Vec<(u32, &str)> = members.iter().map(|m| (m.id, m.name.as_str())).collect();
    let start = members.iter().map(|m| m.start).fold(f64::INFINITY, f64::min);
    let n = members.len();
    vec![Problem {
        kind: "3d_many_primitives",
        at: start,
        clip: members[0].id,
        problem: format!("{n} separate primitive model clips (single cubes, cylinders, cones) are on screen together as one place or object: {}. To a viewer that is flat layer placing, not a modelled place, and the timeline carries {n} layers where it needs one", ids_said(&listed)),
        fix: "build it as ONE model with named parts: `build_place` (kind building, facade, street, interior, bridge, fence or stairs; a `style` such as classical, art_deco, brutalist, modern_glass, medieval, oriental, futuristic, rustic; a `wear`) makes windows in real openings, cornices, kerbs and lamps itself; or `combine` these clips into one (`combine` with `clips`: [ids] or `like`: [\"window\"]), then refine with `mesh` and `asset` parts; for a place to walk through use `build_world` / `edit_world`. Delete the separate clips",
    }]
}

/// A person assembled from primitive shapes, or from a model whose parts are all body parts.
fn character_notes(heroes: &[Hero], env: &Env, folded: &mut HashSet<u32>) -> Vec<Problem> {
    let mut out = Vec::new();
    // Several primitive clips named for body parts, on screen together.
    let named: Vec<(&PrimClip, Vec<String>)> = env.prims.iter().map(|p| (p, body_words(&p.name))).filter(|(_, w)| !w.is_empty()).collect();
    let mut taken: HashSet<u32> = HashSet::new();
    for (a, _) in &named {
        if taken.contains(&a.id) {
            continue;
        }
        let group: Vec<&(&PrimClip, Vec<String>)> = named.iter().filter(|(b, _)| b.id == a.id || overlap((a.start, a.end), (b.start, b.end)) >= 1.0).collect();
        let mut words: Vec<&String> = group.iter().flat_map(|(_, w)| w.iter()).collect();
        words.sort();
        words.dedup();
        if words.len() < 3 {
            continue;
        }
        taken.extend(group.iter().map(|(p, _)| p.id));
        let listed: Vec<(u32, &str)> = group.iter().map(|(p, _)| (p.id, p.name.as_str())).collect();
        folded.extend(group.iter().map(|(p, _)| p.id));
        out.push(Problem {
            kind: "3d_character_from_primitives",
            at: group.iter().map(|(p, _)| p.start).fold(f64::INFINITY, f64::min),
            clip: a.id,
            problem: format!("a figure is built from {} separate primitive clips named for body parts ({}): {}. Cones and cubes read as a toy, cannot bend at a joint, and cannot be posed or walked", group.len(), words.iter().map(|w| vocab::display(w)).collect::<Vec<_>>().join(", "), ids_said(&listed)),
            fix: "use the character base: `model` with `shape`: \"character\" (a rigged body with a head, hands and feet; set `colour`, `size`, `name`) or `head` for a bust, dress it with the wardrobe (clothes, shoes, hair) and give it things to carry with `hold`, and move it with `act`; delete the primitive clips",
        });
    }
    // One model whose parts are body parts and nothing else that a character base has.
    for h in heroes {
        let mats: Vec<String> = h.info.materials.clone();
        let words: HashSet<String> = mats.iter().flat_map(|n| body_words(n)).collect();
        let base = mats.iter().any(|n| n.eq_ignore_ascii_case("skin"));
        if words.len() >= 4 && !base && h.info.craft.shells >= 4 && h.info.craft.triangles < 30_000 && !folded.contains(&h.id) {
            folded.insert(h.id);
            let mut w: Vec<String> = words.iter().map(|w| vocab::display(w)).collect();
            w.sort();
            out.push(Problem {
                kind: "3d_character_from_primitives",
                at: h.start,
                clip: h.id,
                problem: format!("model \"{}\" (clip {}) is a figure assembled from {} separate shapes named for body parts ({}) with no skin: blocks and cones, not a character", h.name, h.id, h.info.craft.shells, w.join(", ")),
                fix: "use the character base instead: `model` with `shape`: \"character\" (rigged, with a head, hands and feet), dress it with the wardrobe and give it props with `hold`, move it with `act`; keep this model only for what is not a person",
            });
        }
    }
    out
}

/// Bones keyed pose by pose where a gait or act would do.
fn keyed_notes(env: &Env) -> Vec<Problem> {
    env.keyed
        .iter()
        .filter(|k| !k.acts && k.poses >= 6 && k.bones >= 3 && k.keys >= 18)
        .map(|k| Problem {
            kind: "3d_hand_keyed_motion",
            at: k.start,
            clip: k.id,
            problem: format!("model \"{}\" (clip {}) is moved with {} hand-written bone keys on {} bones across {} poses (a walk, run or other cycle keyed pose by pose in a script): slow to write, uneven in rhythm and weightless", k.name, k.id, k.keys, k.bones, k.poses),
            fix: "use the motion ops instead of keying poses: `act` with a named action (walk, run, idle, wave...) and a `speed`, and the locomotion op to carry the character along a route so the feet match the ground speed; key bones by hand only for a one-off hero gesture (a lid, a nod), a handful of keys",
        })
        .collect()
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

// ------------------------------------------------------------- the project

fn param_set(g: &m::Generator, keys: &[&str]) -> bool {
    keys.iter().any(|k| g.params.contains_key(*k))
}

/// The heroes, lights and cameras of the active sequence.
pub fn scene(model: &m::Project, view: &view::Project, refs: &[(Vec<String>, PathBuf)]) -> (Vec<Hero>, Env) {
    let mut env = Env::default();
    let mut heroes = Vec::new();
    let Some(seq) = model.sequence(model.active_sequence) else { return (heroes, env) };
    let hidden = |c: &m::Clip| seq.lanes.iter().find(|l| l.id == c.lane).is_some_and(|l| l.hidden);
    let skip = ["terrain", "scatter", "stand-in", "stand in", "previs", "placeholder", "shard", "debris", "cloth", "smoke", "fire", "ocean", "water", "particles"];
    for c in seq.clips.iter().filter(|c| c.enabled && !hidden(c)) {
        let m::ClipSource::Generator(g) = &c.source else { continue };
        let (start, end) = (secs(c.start), secs(c.start + c.duration));
        match g.kind {
            m::GeneratorKind::Light => {
                let kind = m::LightSettings::read(&g.params).kind;
                // The clip that only carries the scene's air lights nothing.
                if matches!(g.params.get("atmosphere"), Some(m::Value::Bool(true))) && m::LightSettings::read(&g.params).intensity <= 0.0 {
                    continue;
                }
                env.lights.push(Light { start, end, ambient: kind == m::LightKind::Ambient });
            }
            m::GeneratorKind::Camera => {
                let moves = !c.keyframes.is_empty() || c.motion.entry.is_some() || c.motion.exit.is_some();
                env.cameras.push(Camera { start, end, moves, dof: m::CameraSettings::read(&g.params, view.width as f64).dof });
            }
            m::GeneratorKind::Model => {
                env.models.push((c.id.0 as u32, c.name.clone(), start, end));
                let name = c.name.to_lowercase();
                let skipped = skip.iter().any(|s| name.contains(s));
                // Hand-written bone keys, counted whatever the clip's length.
                let bone_tracks: Vec<(&String, usize)> = c.keyframes.iter().filter(|(k, _)| k.starts_with("generator.bone.")).map(|(k, t)| (k, t.keys.len())).collect();
                if !bone_tracks.is_empty() && !skipped {
                    let bones: HashSet<String> = bone_tracks.iter().map(|(k, _)| k.trim_start_matches("generator.bone.").rsplit_once('.').map_or(k.to_string(), |(b, _)| b.to_owned())).collect();
                    let mut times: Vec<m::Micros> = c.keyframes.iter().filter(|(k, _)| k.starts_with("generator.bone.")).flat_map(|(_, t)| t.keys.iter().map(|k| k.time)).collect();
                    times.sort_unstable();
                    times.dedup();
                    env.keyed.push(KeyedClip { id: c.id.0 as u32, name: c.name.clone(), start, end, keys: bone_tracks.iter().map(|b| b.1).sum(), bones: bones.len(), poses: times.len(), acts: g.params.contains_key("action") });
                }
                if end - start >= 1.0 && !skipped && let Some(info) = crate::mesh_store::file_of(c).and_then(|f| info_of_file(&f)) {
                    let k = &info.craft;
                    if k.triangles > 0 && k.triangles <= 4000 && k.shells <= 2 && k.materials <= 2 && k.bones == 0 {
                        env.prims.push(PrimClip { id: c.id.0 as u32, name: c.name.clone(), start, end });
                    }
                }
                if end - start < 1.5 || skipped {
                    continue;
                }
                let Some(file) = crate::mesh_store::file_of(c) else { continue };
                let Some(info) = info_of_file(&file) else { continue };
                if info.craft.triangles == 0 {
                    continue;
                }
                let words = tokens(&c.name);
                let reference = refs.iter().filter(|(w, _)| refs_for(&words, w)).filter_map(|(_, p)| reference_score(&file, p)).reduce(|a, b| a + b).map(|s| s / refs.iter().filter(|(w, _)| refs_for(&words, w)).count().max(1) as f32);
                heroes.push(Hero {
                    id: c.id.0 as u32,
                    name: c.name.clone(),
                    start,
                    end,
                    file,
                    info,
                    posed: c.keyframes.keys().any(|k| k.starts_with("generator.bone.")) || g.params.contains_key("action"),
                    look_set: param_set(g, &["base_colour", "metallic", "roughness"]),
                    glow_set: param_set(g, &["emissive"]),
                    reference,
                });
            }
            _ => {}
        }
    }
    (heroes, env)
}

fn refs_for(model_words: &[String], ref_words: &[String]) -> bool {
    ref_words.is_empty() || ref_words.iter().any(|w| model_words.contains(w))
}

/// How a model file's render matches a reference picture.
fn reference_score(file: &str, picture: &std::path::Path) -> Option<f32> {
    let key = format!("{file}|{}|{}", picture.display(), std::fs::metadata(picture).ok()?.len());
    static MEMO: OnceLock<Mutex<HashMap<String, Option<f32>>>> = OnceLock::new();
    let memo = MEMO.get_or_init(Default::default);
    if let Some(v) = memo.lock().ok()?.get(&key) {
        return *v;
    }
    let mesh = studio_mesh::io::load(std::path::Path::new(file)).ok()?;
    let tex = super::models::read_picture(&picture.to_string_lossy()).ok()?;
    let score = hero::silhouette_match(&mesh, tex.width as usize, tex.height as usize, &tex.rgba);
    memo.lock().ok()?.insert(key, score);
    score
}

/// Reference pictures in the media bin: images called a reference, front,
/// side or three-quarter view, with the words of the model they are for
/// (none: they are for every hero).
pub fn references(view: &view::Project) -> Vec<(Vec<String>, PathBuf)> {
    let mut out = Vec::new();
    for mi in view.media.iter().filter(|mi| mi.kind == view::MediaKind::Image) {
        let Some(path) = mi.path.clone() else { continue };
        let label = format!("{} {}", mi.name, path.file_stem().and_then(|s| s.to_str()).unwrap_or("")).to_lowercase();
        if !["reference", "front view", "side view", "three-quarter", "three quarter", "turnaround", "ref "].iter().any(|k| label.contains(k)) {
            continue;
        }
        let plain = stems(PLAIN);
        let skip = stems(&["reference", "front", "side", "view", "quarter", "three", "turnaround", "ref", "image", "png", "jpg", "jpeg", "generated"]);
        let words: Vec<String> = tokens(&label).into_iter().filter(|w| w.len() >= 3 && !plain.contains(w) && !skip.contains(w) && !w.chars().all(|c| c.is_ascii_digit())).collect();
        out.push((words, path));
    }
    out
}

impl super::Session {
    /// All 3D notes for this film, with what the AI has set aside.
    pub(super) fn three_d_problems(&self) -> (Vec<Problem>, Vec<String>) {
        let p = &self.doc.project;
        let (heroes, env) = scene(&self.doc.model, p, &references(p));
        let words = Words { brief: self.brief.clone().unwrap_or_default(), notes: self.notes_text().unwrap_or_default() };
        let set_aside = accepted(&words.notes);
        let mut out = judge(&heroes, &env, &words);
        out.extend(self.footing_problems().into_iter().filter(|p| !set_aside.iter().any(|a| a == p.kind)));
        // Physically: what floats, is sunk or would tip, where heroes appear.
        let mut moments: Vec<f64> = heroes.iter().map(|h| h.start).collect();
        moments.sort_by(f64::total_cmp);
        moments.dedup_by(|a, b| (*a - *b).abs() < 0.5);
        let mut seen = HashSet::new();
        for t in moments.into_iter().take(3) {
            let f: Vec<_> = crate::physics3d::rest::check(&self.doc.model, self.doc.model.active_sequence, (t * view::SEC as f64) as i64).into_iter().filter(|f| seen.insert(f.clip)).collect();
            out.extend(solidity::physical_notes(&f, t));
        }
        (out, set_aside)
    }
}

mod solidity;
mod vocab;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod places_tests;
