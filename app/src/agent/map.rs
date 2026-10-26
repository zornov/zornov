// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The capability map: everything Studio can do on one short page, grouped
//! by department, one line per capability with the `edit` op or tool that
//! does it. An agent reads it once (it heads the `guide` answer) instead of
//! the whole guide, so it knows the departments exist before it plans.
//!
//! The lines are written by hand (what a capability is for is a judgement),
//! but the names come from the registry: every tool of `mcp::tools()` and
//! every op `edit` dispatches. A name that no line mentions is added under
//! "More" at run time, so the map is never stale, and a test fails until
//! someone gives it a proper line.

use std::collections::BTreeSet;
use std::sync::OnceLock;

/// One department: its lines are (what it is for, names). A name is an
/// `edit` op, or a tool when written `tool:name`.
struct Dept {
    name: &'static str,
    lines: &'static [(&'static str, &'static str)],
}

const DEPTS: &[Dept] = &[
    Dept {
        name: "Edit (cut and arrange)",
        lines: &[
            ("build a whole beat (voice, picture, title, sound)", "scene narrate"),
            ("put footage and library items on the timeline", "place add replace tool:import tool:download tool:read tool:job tool:media tool:make_playable"),
            ("cut and trim", "split trim delete move nudge reorder duplicate freeze speed_ramp"),
            ("trim neighbours", "roll slide slip ripple extend trim_sides"),
            ("film editing from takes and sequences", "source_edit insert overwrite fit_to_fill lift extract j_cut l_cut split_edit sync_lock target sequence string_out selects take text_cut tool:editorial"),
            ("lanes, markers, grouping", "lane marker group ungroup link unlink merge unmerge flatten unflatten parent restack align distribute detach_audio"),
            ("repeat one op many times", "repeat"),
            ("properties of any clip; the edit call itself", "set copy_look keyframe raw tool:edit tool:inspect tool:edit_schema"),
        ],
    },
    Dept {
        name: "Motion and animation",
        lines: &[
            ("keyframes and curves", "animate keys tool:graph"),
            ("many clips, one after another; keep a move or design for reuse", "stagger use_move use_design tool:save_move tool:save_design"),
            ("procedural motion", "modifier driver expression physics time_remap"),
            ("entry, exit and loop presets; transitions", "motion transition"),
            ("particles and effects over a stretch", "particles fx_layer"),
            ("morph one outline into another", "morph"),
        ],
    },
    Dept {
        name: "Motion graphics and text",
        lines: &[
            ("titles, extruded 3D letters, text animators; any installed font by name (text font/weight/italic)", "text text_3d"),
            ("find a font by name or look (art deco, script, slab, mono...), with a preview", "tool:fonts"),
            ("lower thirds, callouts, end cards, infographics", "template"),
            ("charts from data (bar, line, pie, map)", "chart"),
            ("shape layers and colour cards", "shape card"),
            ("vector art, logos, icons, lettering", "tool:vector"),
            ("captions from speech", "tool:ai"),
        ],
    },
    Dept {
        name: "3D and camera",
        lines: &[
            ("real 3D models, sculpting, materials", "model mesh asset"),
            ("dress any 3D figure (coats, hats, shoes, armour…) and put props in its hands, with a flame that lights the scene", "wear hold"),
            ("places as ONE model: buildings, facades, streets, interiors, bridges, fences, stairs in any style; merge or split models into named parts", "build_place combine uncombine"),
            ("2D layers in 3D space; cameras and lights", "three_d camera light parallax"),
            ("scene fog, weather, bloom, exposure, grade", "atmosphere"),
            ("shots, lighting set-ups, previs", "shot lighting previs tool:compare_cameras tool:animatic"),
            ("rigs, acting, simulation, worlds, final renders", "rig act simulate world render3d"),
            ("physics: what a model is made of and weighs, joints; drop, throw, push, break, settle; the whole scene baked to keys", "body joint drop throw push break settle bake_physics physics_world"),
            ("ragdolls; tails, ears, hair and coat tails that follow through", "ragdoll jiggle"),
            ("enterable worlds: build, explore, film", "build_world edit_world world_info walk stand"),
            ("look around the 3D workspace", "tool:viewport"),
        ],
    },
    Dept {
        name: "Characters",
        lines: &[
            ("puppets, skeletons, poses, inverse kinematics; parts splits a photo or drawing into clean layers (hidden areas filled behind)", "puppet lip_sync skeleton pose ik aim parts swap retarget"),
            ("cut lines: legs, arms and lips that move apart", "scissor"),
            ("make a character; capture performance", "character mocap mask_puppet"),
            ("3D characters: walk, run, sneak, limp or trot along a route with planted feet; turns; steps on uneven ground", "act tool:inspect"),
            ("3D hand poses, sit, crouch, lean, looks that lead with the eyes, reaches that never stretch, nod, shrug, sigh, startle", "pose"),
            ("where a 3D character's bones, soles, hands and gaze are; which feet slide; what is broken and the fix", "tool:inspect tool:review"),
            ("plan a film: script, scenes, cast, shots", "tool:story tool:production tool:plan"),
        ],
    },
    Dept {
        name: "VFX and compositing",
        lines: &[
            ("node graphs and ready-made recipes", "comp keying remove_object clean_plate sky wire paint"),
            ("lens and camera look", "depth_of_field grain_match lens motion_blur"),
            ("3D camera tracking", "matchmove place_on_ground export_camera import_camera"),
            ("tracking, stabilising, retiming", "track stabilize smooth_motion reframe"),
            ("cut-outs, rotoscoping, masks, pins", "cut_out roto mask corner_pin matte foreground"),
            ("remove a plain white, black, grey or green backdrop (colour from the border, edges kept clean)", "key_background"),
            ("a hand-drawn polygon mask in source-picture pixels; check a split into parts side by side", "polygon_mask tool:parts_check"),
            ("light and shadow on cut-outs; living places", "shadow light_match living"),
        ],
    },
    Dept {
        name: "Colour and picture",
        lines: &[
            ("looks and effects", "apply effect"),
            ("grading, colour spaces, HDR, LUTs", "tool:colour"),
            ("match shots", "match_colour tool:scopes"),
            ("timelines in and out, relinking, proxies", "tool:conform tool:media_manage"),
            ("paint, retouch, composite stills", "tool:picture"),
            ("restore, denoise, upscale", "tool:enhance"),
        ],
    },
    Dept {
        name: "Sound and music",
        lines: &[
            ("original music in any genre, key and tempo, fitted to the cut and its cue points, composed here; or a library track", "music"),
            ("foley and sound effects made here from physical models: impacts, fire, matches, traffic, crowds, footsteps, animals; room tones; or from the library", "sfx tool:catalogue"),
            ("sung voices from notes and syllables, one stem per voice for lip sync", "sing"),
            ("footsteps and surface sounds for characters", "foley"),
            ("EQ, compressor, fades, ducking", "audio"),
            ("voices; the sound department: mix buses, loudness, stems, dialogue, ADR, subtitles, dubbing", "tool:voices tool:mix tool:loudness tool:stems tool:dialogue tool:adr tool:subtitles tool:dub"),
        ],
    },
    Dept {
        name: "Generation",
        lines: &[
            ("pictures, video, music, sounds from words", "tool:generate"),
            ("plugins, macros, automation", "tool:plugins tool:automation"),
        ],
    },
    Dept {
        name: "Senses and checking",
        lines: &[
            ("see and hear before cutting", "tool:see tool:listen tool:analyse tool:frame tool:contact_sheet tool:pick tool:match"),
            ("where everything is on the canvas: exact boxes, overlaps, free space", "tool:canvas_layout"),
            ("what is in footage: name, where, when it enters; find a moment; room for titles; cut out", "tool:objects"),
            ("read the project", "tool:summary tool:script tool:notes"),
            ("what a professional would not ship", "tool:check tool:review"),
        ],
    },
    Dept {
        name: "Library and rights",
        lines: &[
            ("what the content library has", "tool:catalogue tool:search"),
            ("your files across projects", "tool:assets tool:project_care"),
            ("Studio help; skills (the procedure for a kind of work) and lessons", "tool:howto tool:capabilities tool:guide tool:skill"),
        ],
    },
    Dept {
        name: "Delivery",
        lines: &[
            ("export, specs, QC, background renders", "tool:export tool:export_status tool:queue tool:qc tool:deliver"),
            ("masters, proxies, 15/30/60 s versions", "tool:bundle tool:cutdown"),
        ],
    },
    Dept {
        name: "Project and team",
        lines: &[
            ("undo, redo, open, save, new", "tool:undo tool:redo tool:open tool:save tool:new_project"),
            ("review notes, versions, sharing, roles", "tool:comments tool:versions tool:share tool:team"),
            #[cfg(not(feature = "commerce"))]
            ("taste, window, capture", "tool:taste tool:layout tool:capture tool:made"),
            #[cfg(feature = "commerce")]
            ("taste, window, account, capture", "tool:taste tool:layout tool:account tool:capture tool:made"),
        ],
    },
];

/// Every written line: (department, what it is for, names).
pub fn lines() -> Vec<(&'static str, &'static str, &'static str)> {
    DEPTS.iter().flat_map(|d| d.lines.iter().map(move |(what, names)| (d.name, *what, *names))).collect()
}

/// Every `edit` op the dispatcher answers, read from the dispatcher itself.
pub fn all_ops() -> &'static BTreeSet<String> {
    static OPS: OnceLock<BTreeSet<String>> = OnceLock::new();
    OPS.get_or_init(|| {
        let mut out: BTreeSet<String> = BTreeSet::new();
        let names = |line: &str, out: &mut BTreeSet<String>| {
            let arm = line.split("=>").next().unwrap_or("");
            let arm = arm.split(" if ").next().unwrap_or(arm);
            for part in arm.split('|') {
                let p = part.trim();
                if let Some(n) = p.strip_prefix('"').and_then(|p| p.strip_suffix('"')) {
                    if n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
                        out.insert(n.to_owned());
                    }
                }
            }
        };
        // The match in `Session::op`: its arms sit at one indent.
        let src = include_str!("mod.rs");
        let from = src.find("fn op(&mut self, op: &Value)").unwrap_or(0);
        let upto = src[from..].find("AI jobs").map_or(src.len(), |i| from + i);
        for l in src[from..upto].lines() {
            if l.starts_with("            \"") && l.contains("=>") {
                names(l, &mut out);
            }
        }
        // The film-editing ops, listed in their own module.
        let ed = include_str!("editorial.rs");
        let from = ed.find("fn op_fields(op: &str)").unwrap_or(0);
        let upto = ed[from..].find("_ => return None").map_or(ed.len(), |i| from + i);
        for l in ed[from..upto].lines() {
            if l.starts_with("        \"") && l.contains("=>") {
                names(l, &mut out);
            }
        }
        out.insert("repeat".into());
        out
    })
}

/// Every tool name.
pub fn all_tools() -> Vec<String> {
    super::mcp::tools().iter().filter_map(|t| t["name"].as_str().map(str::to_owned)).collect()
}

/// The names the written lines mention (ops bare, tools as `tool:name`).
fn mentioned() -> BTreeSet<&'static str> {
    DEPTS.iter().flat_map(|d| d.lines.iter()).flat_map(|(_, names)| names.split_whitespace()).collect()
}

/// Ops and tools that no written line mentions (a new op nobody described).
pub fn missing() -> Vec<String> {
    let said = mentioned();
    let mut out: Vec<String> = all_ops().iter().filter(|o| !said.contains(o.as_str())).cloned().collect();
    out.extend(all_tools().into_iter().filter(|t| !said.contains(format!("tool:{t}").as_str())).map(|t| format!("tool:{t}")));
    out
}

/// Names written in a line that do not exist (a renamed or removed op).
pub fn stale() -> Vec<String> {
    let (ops, tools) = (all_ops(), all_tools());
    mentioned()
        .into_iter()
        .filter(|n| match n.strip_prefix("tool:") {
            Some(t) => !tools.iter().any(|x| x == t),
            None => !ops.contains(*n),
        })
        .map(str::to_owned)
        .collect()
}

/// The map as text: about 1 to 2 thousand tokens.
pub fn capability_map() -> String {
    let mut s = String::from(
        "# What Studio can do (capability map)\n\
         Read once; plan from it. Bare names are ops inside `edit` ({\"op\": name, ...}); `tool:x` is a tool you call on its own. Fields: `edit_schema` or `capabilities {query}`; detail: `guide {topic}`.\n",
    );
    for d in DEPTS {
        s.push_str(&format!("\n**{}**\n", d.name));
        for (what, names) in d.lines {
            s.push_str(&format!("- {what}: {names}\n"));
        }
    }
    let more = missing();
    if !more.is_empty() {
        s.push_str(&format!("\n**More**: {}\n", more.join(" ")));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_is_read_whole() {
        let ops = all_ops();
        for o in ["split", "sfx", "music", "puppet", "keying", "insert", "text_cut", "repeat", "render3d"] {
            assert!(ops.contains(o), "{o} not found in the dispatcher: {ops:?}");
        }
        assert!(ops.len() > 120, "only {} ops read", ops.len());
        assert!(all_tools().len() > 50);
    }

    #[test]
    fn every_op_and_tool_has_a_line() {
        let m = missing();
        assert!(m.is_empty(), "give these a line in the capability map (map.rs): {m:?}");
    }

    #[test]
    fn no_line_names_something_that_is_gone() {
        let s = stale();
        assert!(s.is_empty(), "map.rs mentions names Studio no longer has: {s:?}");
    }

    #[test]
    fn the_map_is_short_and_grouped() {
        let m = capability_map();
        assert!(m.len() < 9_000, "the map is {} bytes; it must stay about 2k tokens", m.len());
        for d in ["Edit", "Motion", "Sound and music", "3D", "Characters", "VFX", "Colour", "Generation", "Delivery"] {
            assert!(m.contains(d), "no department {d}");
        }
        assert!(!m.contains("**More**"), "undescribed names reached the map");
    }
}
