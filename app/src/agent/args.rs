// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Arguments are checked, not guessed: a misspelt field used to be
//! ignored (a `frame` at `"at": 34` rendered 0 s and looked right). Common
//! other names are mapped; anything else is refused with the list of what
//! the tool or op takes.

use serde_json::{Map, Value};

/// Other names agents reach for, per tool or op: (alias, real name).
fn aliases(name: &str) -> &'static [(&'static str, &'static str)] {
    match name {
        "frame" => &[("at", "t"), ("time", "t"), ("seconds", "t")],
        "chart" => &[("color", "colour"), ("type", "kind"), ("values", "data"), ("rows", "data")],
        "shape" | "text_3d" => &[("color", "colour"), ("shapes", "items"), ("string", "text"), ("kind", "type"), ("shape", "type")],
        "template" => &[("name_or_id", "id"), ("values", "controls"), ("fields", "controls")],
        "see" => &[("at", "t"), ("time", "t"), ("seconds", "t"), ("clip", "clips")],
        "contact_sheet" => &[("at", "times"), ("moments", "times"), ("from", "start"), ("to", "end"), ("frames", "count")],
        "graph" => &[("id", "clip"), ("prop", "property"), ("count", "samples")],
        "scopes" => &[("at", "t"), ("time", "t"), ("seconds", "t"), ("with", "compare"), ("against", "compare")],
        "import" => &[("path", "paths"), ("files", "paths")],
        "set" => &[("color", "colour"), ("size", "text_size"), ("time", "at"), ("t", "at")],
        "text" | "card" => &[("color", "colour")],
        "light" => &[("color", "colour"), ("kind", "type"), ("aim", "look_at"), ("target", "look_at"), ("casts_shadows", "shadows"), ("bloom", "glow"), ("halo", "glow"), ("streak", "flare"), ("shaft", "beam"), ("cone_of_light", "beam"), ("size", "source_size"), ("flicker_hz", "flicker_speed"), ("fire", "flame"), ("air", "atmosphere")],
        "camera" => &[("kind", "type"), ("aim", "look_at"), ("target", "look_at"), ("poi", "look_at"), ("lens", "focal_length"), ("field_of_view", "fov"), ("moves", "move"), ("kit", "lens_kit"), ("lenses", "lens_kit")],
        "shot" => &[("subject", "on"), ("subjects", "on"), ("describe", "say"), ("time", "at"), ("t", "at")],
        "lighting" => &[("color", "colour"), ("setup", "preset"), ("subject", "on"), ("time", "at"), ("bloom", "glow"), ("fire", "flame"), ("air", "atmosphere"), ("weather", "atmosphere")],
        "atmosphere" => &[("look", "preset"), ("mood", "preset"), ("style", "preset"), ("fog_color", "fog_colour"), ("color", "fog_colour"), ("density", "fog_density"), ("bloom", "bloom_strength"), ("ground_y", "ground"), ("floor", "ground"), ("tone_map", "tonemap"), ("tone", "tonemap"), ("clip_id", "clip"), ("time", "at")],
        "body" => &[("kind", "type"), ("material_name", "material"), ("weight", "mass"), ("shape", "collider"), ("restitution", "bounce"), ("bounciness", "bounce")],
        "joint" => &[("type", "kind"), ("parent", "to"), ("position", "at_point"), ("pivot", "at_point")],
        "drop" => &[("objects", "clips"), ("time", "at"), ("length", "duration")],
        "throw" => &[("object", "clip"), ("time", "at"), ("length", "duration"), ("at_clip", "toward"), ("aim", "toward")],
        "push" => &[("object", "clip"), ("time", "at"), ("length", "duration"), ("force", "strength"), ("impulse", "strength"), ("dir", "direction")],
        "break" => &[("object", "clip"), ("time", "at"), ("impact", "point"), ("count", "pieces")],
        "settle" => &[("objects", "clips"), ("time", "at")],
        "ragdoll" => &[("character", "clip"), ("time", "at"), ("length", "duration")],
        "jiggle" => &[("character", "clip"), ("chains", "bones"), ("time", "at"), ("length", "duration")],
        "bake_physics" => &[("time", "at"), ("length", "duration"), ("from", "at")],
        "physics_world" => &[("scale", "px_per_metre"), ("pixels_per_metre", "px_per_metre"), ("px_per_meter", "px_per_metre")],
        "model" => &[("color", "colour"), ("kind", "shape"), ("path", "file"), ("import", "file")],
        "wear" => &[("clothes", "items"), ("garments", "items"), ("outfit", "items"), ("take_off", "remove"), ("clip_name", "clip")],
        "hold" => &[("object", "item"), ("holding", "item"), ("in_hand", "hand"), ("colour_tip", "tip")],
        "rig" => &[("kind", "template"), ("type", "template"), ("bone_list", "bones"), ("constraints", "constraint"), ("shape_keys", "face")],
        "act" => &[("animation", "action"), ("clip_name", "action"), ("bvh", "mocap"), ("capture", "mocap"), ("start", "at"), ("time", "at"), ("rate", "speed"), ("path", "route"), ("waypoints", "route"), ("target", "to"), ("heading", "face"), ("gait", "motion")],
        "simulate" => &[("type", "kind"), ("sim", "kind"), ("objects", "clips"), ("bodies", "clips"), ("time", "at"), ("start", "at"), ("length", "duration"), ("color", "colour"), ("pos", "position"), ("where", "position"), ("emitter", "emit_from"), ("character", "on"), ("wearer", "on")],
        "world" => &[("environment", "hdri"), ("panorama", "hdri"), ("ground", "terrain"), ("landscape", "terrain"), ("scatter_on", "on")],
        "build_world" => &[("type", "preset"), ("kind", "preset"), ("style", "preset"), ("picture", "reference"), ("reference_image", "reference"), ("image", "reference"), ("objects", "props"), ("trees", "scatter"), ("vegetation", "scatter"), ("people", "characters"), ("time", "at")],
        "walk" => &[("path", "route"), ("points", "route"), ("waypoints", "route"), ("target", "look_at"), ("time", "start"), ("at", "start"), ("pace", "speed")],
        "stand" => &[("position", "at"), ("where", "at"), ("target", "look_at"), ("time", "t"), ("eye_height", "eye")],
        "build_place" => &[("type", "kind"), ("place", "kind"), ("era", "style"), ("architecture", "style"), ("storeys", "floors"), ("stories", "floors"), ("height", "size"), ("length", "size"), ("age", "wear"), ("weathering", "wear"), ("night", "lit"), ("windows_lit", "lit"), ("scale", "px_per_metre"), ("pos", "position")],
        "combine" => &[("ids", "clips"), ("models", "clips"), ("layers", "clips"), ("name_contains", "like"), ("matching", "like"), ("group", "name"), ("title", "name")],
        "uncombine" => &[("id", "clip"), ("model", "clip"), ("part", "parts"), ("only", "parts")],
        "edit_world" => &[("add_props", "props"), ("hour", "time_of_day"), ("time", "time_of_day"), ("delete", "remove")],
        "render3d" => &[("engine", "mode"), ("spp", "samples"), ("quality", "samples"), ("blur", "motion_blur"), ("shutter", "motion_blur"), ("aov", "passes")],
        "mesh" => &[("color", "colour"), ("step", "do"), ("op_list", "steps"), ("center", "at"), ("centre", "at"), ("brush_size", "radius")],
        "three_d" => &[("depth", "z"), ("position_z", "z"), ("tilt", "rotation_x"), ("turn", "rotation_y"), ("enabled", "on")],
        "export" => &[("out", "path"), ("output", "path"), ("file", "path")],
        "place" => &[("from", "source_in"), ("to", "source_out"), ("start", "at")],
        "keyframe" => &[("at", "time"), ("t", "time")],
        "split" => &[("time", "at"), ("t", "at")],
        "marker" => &[("time", "at"), ("t", "at"), ("text", "note")],
        "apply" | "add" | "fx_layer" => &[("name", "item")],
        "delete" | "apply" | "reframe" | "unlink" => &[("clip", "clips")],
        "track" => &[("corners", "plane"), ("time", "at"), ("t", "at"), ("follower", "attach"), ("mode", "follow"), ("size", "radius")],
        "stabilize" => &[("smooth", "smoothness"), ("zoom", "max_zoom")],
        "match_colour" => &[("reference", "to"), ("with", "to"), ("amount", "strength")],
        "shadow" => &[("color", "colour"), ("direction", "angle"), ("blur", "softness"), ("strength", "opacity"), ("darkness", "opacity"), ("time", "at")],
        "light_match" => &[("strength", "amount"), ("rim_light", "rim"), ("edge_light", "rim"), ("time", "at")],
        "foreground" => &[("time", "at"), ("rect", "box"), ("over", "hides"), ("in_front_of", "hides"), ("not", "exclude")],
        "cut_out" | "roto" => &[("time", "at"), ("t", "at"), ("include", "points"), ("not", "exclude"), ("rect", "box"), ("what", "subject")],
        _ => &[],
    }
}

/// The fields each edit op takes.
pub fn op_fields(op: &str) -> Option<&'static [&'static str]> {
    if let Some(f) = super::editorial::op_fields(op) {
        return Some(f);
    }
    Some(match op {
        "place" => &["media", "at", "lane", "source_in", "source_out", "duration", "speed", "fit", "x", "y", "volume", "muted", "name"],
        // Motion graphics: listed so their fields are checked and shown by
        // edit_schema like every other op's (they were missing from both).
        "shape" => &["items", "preset", "type", "height", "roundness", "points", "radius", "inner", "stroke", "inputs", "add", "patch", "replace", "lottie", "clip", "at", "duration", "lane", "behind", "x", "y", "z", "opacity", "rotation", "size", "width", "extrude", "side_colour", "colour", "accent", "stroke_width", "path", "speed", "name"],
        "text_3d" => &["text", "runs", "font", "size", "weight", "italic", "kerning", "letter_spacing", "depth", "bevel", "extrude", "colour", "side_colour", "accent", "opacity", "stroke_width", "clip", "at", "duration", "lane", "behind", "x", "y", "z", "rotation", "name"],
        "expression" => &["clip", "property", "expr", "driver", "remove"],
        "time_remap" => &["clip", "expr", "keys", "speed", "remove"],
        "template" => &["id", "template", "controls", "list", "search", "category", "clip", "at", "duration", "lane", "x", "y", "name"],
        "chart" => &["kind", "data", "title", "colour", "text", "prefix", "suffix", "decimals", "hole", "total_label", "route", "at", "duration", "x", "y", "name"],
        "morph" => &["clip", "to", "object", "mask", "at", "duration", "ease"],
        "split" => &["at", "clips"],
        "trim" => &["clip", "start", "end", "ripple"],
        "delete" => &["clips", "ripple"],
        "move" => &["clips", "by", "to", "lane"],
        "set" => &["clip", "at", "anchor", "x", "y", "scale", "fit", "rotation", "opacity", "stroke", "stroke_width", "volume", "pan", "fade_in", "fade_out", "speed", "exposure", "contrast", "saturation", "temperature", "tint", "start", "duration", "text", "style", "text_size", "colour", "name", "muted", "backing", "clips", "reveal", "exit", "loop", "unit_motion", "animators", "animator", "runs", "path", "kerning", "font", "weight", "italic"],
        "animate" => &["clip", "clips", "property", "properties", "keys", "ease", "relative", "timeline", "wiggle", "pulse", "path", "arc", "curve", "formula"],
        "stagger" => &["clips", "each", "amount", "from", "by", "origin", "spread", "animate", "use_move"],
        "keys" => &["clip", "clips", "property", "remove", "move", "curve"],
        "driver" => &["clip", "property", "from", "vars", "expr", "multiply", "add", "delay", "remove", "enabled"],
        "physics" => &["clips", "at", "duration", "gravity", "bounce", "friction", "floor", "walls", "ceiling", "static", "push", "spin", "shape"],
        "modifier" => &["clip", "property", "add", "remove", "amount", "per_second", "octaves", "seed", "from", "to", "before", "after", "count", "offset", "min", "max", "expr"],
        "scene" => &["say", "show", "title", "where", "mood", "duration", "at", "voice", "speaker"],
        "narrate" => &["say", "at", "voice", "rate", "volume", "duck", "duck_db", "speaker", "intensity", "emotion"],
        "music" => &["mood", "item", "volume", "describe", "style", "genre", "instruments", "bpm", "tempo", "key", "cues", "tail", "seed", "variant", "length", "seconds", "duration", "at", "lane", "source", "hit", "hits", "resolve", "riser", "drop", "swell"],
        "apply" => &["item", "clips", "panel", "params", "duration"],
        "add" => &["item", "at", "lane", "panel", "duration", "query", "kind", "mood", "min_seconds", "max_seconds", "same", "x", "y", "scale", "fit", "opacity", "rotation", "volume", "colour"],
        "text" => &["text", "at", "duration", "style", "x", "y", "size", "colour", "lane", "backing", "reveal", "exit", "loop", "unit_motion", "animators", "animator", "runs", "path", "kerning", "font", "weight", "italic"],
        "fx_layer" => &["item", "at", "duration", "params", "lane", "animate"],
        "detach_audio" => &["clip", "linked"],
        "unlink" => &["clips"],
        "slip" => &["clip", "by"],
        "reframe" => &["clips", "smoothing"],
        "keyframe" => &["clip", "property", "time", "value"],
        "marker" => &["at", "note", "remove", "move"],
        "transition" => &["clip", "item", "duration", "align", "keep_timing", "remove", "params"],
        "lane" => &["action", "lane", "name", "kind", "position", "to"],
        "effect" => &["clip", "clips", "effect", "remove", "enabled", "params", "move_to", "item", "panel", "duration"],
        "card" => &["colour", "at", "duration", "lane", "width", "height", "x", "y", "opacity", "roundness", "gradient", "angle", "radial", "radius", "shape", "sides", "stroke", "stroke_width", "shadow", "behind"],
        "motion" => &["clip", "clips", "entry", "exit", "loop"],
        "merge" => &["clips", "name"],
        "flatten" | "unflatten" => &["clip"],
        "align" => &["clips", "to", "relative", "margin", "at"],
        "distribute" => &["clips", "axis", "at"],
        "parent" => &["clip", "clips", "to", "at"],
        "matte" => &["clip", "with", "mode", "remove"],
        "restack" => &["clip", "clips", "in_front_of", "behind", "to"],
        "audio" => &["clip", "clips", "effect", "params", "remove", "fade_in", "fade_out", "curve", "preserve_pitch"],
        "use_move" => &["clip", "clips", "move", "fit", "at"],
        "use_design" => &["design", "at", "x", "y", "text"],
        "unmerge" => &["clip"],
        "sfx" => &["kind", "describe", "at", "ends_at", "duration", "volume", "lane", "variant", "surface", "shoe", "count", "species", "animal", "call", "distance", "size", "speed", "material", "period", "era", "tail", "intensity"],
        "sing" => &["voices", "voice", "kind", "type", "notes", "part", "score", "name", "at", "bpm", "tempo", "vibrato", "breath", "pan", "lane", "volume", "seed"],
        "foley" => &["clip", "kind", "at", "surface", "size", "character", "volume", "lane", "variant"],
        "mask" => &["clip", "shape", "mask", "remove", "invert", "params", "region", "t", "feather", "expand", "backdrop"],
        "particles" => &["preset", "at", "duration", "region", "t", "lane", "behind", "params"],
        "living" => &["recipes", "recipe", "backdrop", "at", "t", "duration", "ramp", "front"],
        "puppet" => &["clip", "at", "space", "pins", "pin", "to", "by", "rotate", "scale", "amount", "extent", "name", "keys", "ease", "timeline", "remove", "density", "expansion", "enabled", "rig", "facing", "act", "from", "duration", "speed", "travel", "voice", "blink", "look_at", "face", "parts", "foley", "cuts", "auto_cut", "intensity", "emotion", "lead", "offset", "extras"],
        "lip_sync" => &["clip", "audio", "from", "duration", "intensity", "emotion", "lead", "offset", "extras", "check", "parts", "face", "look_at", "blink"],
        "mask_puppet" => &["clip", "mask", "at", "pins", "pin", "point", "to", "by", "rotate", "scale", "amount", "extent", "name", "keys", "ease", "timeline", "remove"],
        "skeleton" => &["clip", "at", "build", "kind", "facing", "bones", "bone", "min", "max", "depth", "deform", "rename", "parent", "remove", "enabled", "falloff", "act", "from", "duration", "speed", "travel", "cuts", "auto_cut"],
        "key_background" => &["clip", "at", "colour", "tolerance", "softness", "spill", "feather", "choke", "show_matte", "detect_only", "remove"],
        "polygon_mask" => &["clip", "points", "smooth", "feather", "invert", "name", "remove"],
        "scissor" => &["clip", "at", "kind", "cuts", "cut", "path", "name", "fill", "enabled", "to", "by", "remove", "auto", "auto_cut", "list"],
        "pose" => &["clip", "at", "bones", "offset", "key", "keys", "save", "apply", "copy", "paste", "mirror", "reset", "t", "time", "ease", "shapes", "reach", "look_at", "hand", "body", "beat", "recipe", "amount", "size", "toward", "seconds", "dominant", "hold", "over", "blend", "release", "head", "look"],
        "ik" => &["clip", "at", "bone", "handle", "to", "by", "chain", "flip", "live", "keys", "blend", "key", "off", "pole", "stretch", "soft", "pin"],
        "aim" => &["clip", "at", "bone", "handle", "to", "limit", "live", "keys", "blend", "key", "off"],
        "retarget" => &["clip", "from", "start", "duration", "map"],
        "parts" => &["clip", "at", "face"],
        "swap" => &["clip", "at", "set", "add", "bone", "place", "width", "keys", "show", "list"],
        "character" => &["create", "name", "from", "kind", "description", "save_rig", "add", "generate", "estimate", "provider", "aspect", "use", "at", "duration", "lane", "x", "y", "mouths_on", "place", "width", "bone", "list"],
        "mocap" => &["clip", "at", "video", "from", "duration", "rate", "smooth", "mirror", "swap_sides", "strength", "root", "map"],
        "corner_pin" => &["clip", "at", "top_left", "top_right", "bottom_right", "bottom_left", "skew", "keys", "ease", "reset", "remove"],
        "three_d" => &["clip", "clips", "on", "z", "rotation_x", "rotation_y", "rotation", "rotation_z", "orientation", "scale_z", "anchor_z", "auto_orient", "look_at", "material", "extrude", "casts_shadows", "accepts_lights", "accepts_shadows"],
        "camera" => &["clip", "new", "at", "duration", "lane", "type", "position", "look_at", "zoom", "focal_length", "fov", "dof", "focus", "aperture", "blur_level", "auto_orient", "roll", "move", "moves", "lens_kit", "sensor", "squeeze", "breathing", "bokeh", "distortion", "fringe"],
        "shot" => &["say", "on", "size", "angle", "move", "speed", "at", "duration", "clip", "lens", "focal_length", "dutch", "yaw", "shallow", "dof", "profile", "focus_to"],
        "lighting" => &["preset", "mood", "on", "at", "duration", "intensity", "colour", "replace", "glow", "flare", "beam", "flame", "flicker", "atmosphere"],
        "previs" => &["shots", "cast", "set", "at"],
        "model" => &["math", "render", "style", "preset", "age", "figure", "legs", "arms", "neck", "hands", "dressed", "hair", "shape", "size", "detail", "file", "parts", "at", "duration", "lane", "position", "rotation", "colour", "metallic", "roughness", "emissive", "emissive_strength", "wetness", "opacity", "subsurface", "transmission", "ior", "name", "shadows", "clip", "export", "build", "shoulders", "hips", "head_size", "face", "skin", "top", "trousers", "shoes", "rest", "species", "fur", "hairstyle", "hair_style", "shading", "outline", "outline_colour", "outline_color", "shadow_colour", "shadow_color", "rim", "rim_colour", "rim_color", "bands", "toon_bands", "highlights", "threshold"],
        "shading" | "toon" => &["clip", "clips", "shading", "style", "bands", "toon_bands", "outline", "outline_colour", "outline_color", "shadow_colour", "shadow_color", "rim", "rim_colour", "rim_color", "highlights", "threshold"],
        "asset" => &["name", "kit", "style", "wear", "floors", "width", "depth", "detail", "lit", "textured", "variant", "size", "seed", "scale_px", "parts", "texture", "quick", "at", "duration", "lane", "position", "rotation", "shadows"],
        "build_place" => &["kind", "style", "wear", "size", "floors", "width", "depth", "seed", "detail", "lit", "textured", "variant", "px_per_metre", "name", "at", "duration", "lane", "position", "rotation", "shadows"],
        "combine" => &["clips", "like", "all", "name", "at", "duration"],
        "uncombine" => &["clip", "parts"],
        "body" => &["clip", "clips", "type", "body", "material", "mass", "collider", "friction", "bounce", "breakable"],
        "joint" => &["clip", "to", "kind", "type", "at_point", "point", "anchor", "axis", "limits", "motor", "spring", "breaks", "remove"],
        "drop" => &["clip", "clips", "at", "duration", "height", "spin", "floor", "react", "pieces"],
        "throw" => &["clip", "clips", "at", "duration", "velocity", "toward", "target", "at_target", "time", "speed", "direction", "spin", "floor", "react", "pieces"],
        "push" => &["clip", "clips", "at", "duration", "direction", "from", "strength", "speed", "point", "floor", "react", "pieces"],
        "break" => &["clip", "at", "duration", "point", "pieces", "when_hit", "by", "floor", "react"],
        "settle" => &["clip", "clips", "at", "duration"],
        "ragdoll" => &["clip", "at", "duration", "strength", "fade", "push", "point", "floor", "react"],
        "jiggle" => &["clip", "bones", "stiffness", "stiffness_hz", "damping", "gravity", "drag", "flesh", "at", "duration"],
        "bake_physics" => &["clip", "clips", "at", "duration", "react", "floor", "pieces"],
        "physics_world" => &["px_per_metre", "gravity", "rest_on_place"],
        "simulate" => &["kind", "clips", "clip", "static", "colliders", "floor", "gravity", "at", "duration", "bounce", "friction", "push", "spin", "detail", "pieces", "impact", "strength", "by", "seed", "make", "on", "attach", "fabric", "pin", "pins", "pin_radius", "wind", "gusts", "colour", "position", "stiffness", "squash", "damping", "clusters", "region", "count", "length", "segments", "width", "curl", "comb", "preset", "size", "rate", "burst", "life", "velocity", "emit_from", "emit_for", "resolution", "radius", "smoke", "heat", "swirl", "fire", "density", "cells", "drops", "wakes", "height", "wavelength"],
        "world" => &["sky", "hdri", "hdri_turn", "hdri_strength", "background", "terrain", "scatter", "on"],
        "render3d" => &["camera", "mode", "samples", "preview_samples", "bounces", "clamp", "denoise", "motion_blur", "passes"],
        "build_world" => &["description", "preset", "name", "seed", "size", "quality", "terrain", "sky", "fog", "water", "sculpt", "paint", "scatter", "buildings", "props", "characters", "lights", "roads", "spawn", "look", "without", "reference", "clip", "at", "duration"],
        "edit_world" => &["clip", "list", "scatter", "buildings", "props", "characters", "lights", "roads", "sculpt", "paint", "sky", "water", "terrain", "fog", "look", "spawn", "quality", "remove", "time_of_day"],
        "walk" => &["clip", "camera", "route", "from", "start", "speed", "sprint", "eye", "fov", "look_at", "smooth"],
        "stand" => &["clip", "camera", "at", "eye", "height", "fov", "yaw", "pitch", "look_at", "t"],
        "world_info" => &["clip", "ground", "path"],
        "wear" => &["clip", "items", "kind", "item", "type", "what", "wear", "garment", "style", "colour", "color", "colour2", "color2", "trim", "trim_colour", "second_colour", "lining", "material", "fabric", "cloth", "pattern", "fit", "length", "sleeves", "scale", "size", "open", "skin", "name", "detail", "side", "hand", "loose", "physics", "simulate", "remove", "undress", "list", "melanin", "redness", "seed", "density", "strands", "mode", "as", "anime", "look"],
        "hold" => &["clip", "item", "prop", "kit", "name", "size", "colour", "color", "material", "tip", "grip", "hand", "turn", "offset", "flame", "light", "light_intensity", "fingers", "at", "release", "drop"],
        "rig" => &["clip", "template", "bones", "replace", "count", "start", "name", "face", "constraint", "skinning", "rigid", "remove", "list", "bone", "region", "material"],
        "act" => &["clip", "motion", "mocap", "take", "name", "action", "at", "speed", "offset", "loop", "cycle", "energy", "travel", "duration", "blend_to", "over", "stop", "route", "to", "by", "distance", "face", "turn", "style", "limp", "ground", "near", "pace", "speed_px_s", "hold", "lead"],
        "mesh" => &["clip", "steps", "do", "math", "from", "spacing", "around", "refine", "bind", "bone", "levels", "iterations", "strength", "region", "brush", "at", "by", "path", "radius", "symmetry", "keep", "axis", "distance", "amount", "size", "octaves", "seed", "pivot", "scale", "turn", "shape", "detail", "colour", "metallic", "roughness", "emissive", "emissive_strength", "wetness", "opacity", "subsurface", "transmission", "ior", "name", "mode", "with", "parts", "triangles", "angle", "index", "image", "projection", "flip", "count", "offset", "width", "segments", "profile", "select", "edit", "part", "thickness", "depth", "individual", "cuts", "slide", "factor", "toward", "what", "threshold", "percent", "slot", "sides", "compare", "clear", "fill", "point", "normal", "degrees", "turns", "height", "method", "margin", "kind", "smooth", "extend", "edge", "subdivisions", "vertices", "rings", "radius1", "radius2", "major", "minor", "resolution", "x", "y", "add", "remove", "enable", "on", "apply", "set", "to", "move", "smart", "seam", "crease", "direction", "ratio", "assign", "cursor", "view", "faces", "edges", "verts", "all"],
        "light" => &["clip", "type", "at", "duration", "lane", "position", "look_at", "colour", "intensity", "cone_angle", "cone_feather", "radius", "falloff", "shadows", "shadow_darkness", "glow", "flare", "beam", "source_size", "flicker", "flicker_speed", "flame", "flame_height", "flame_width", "flame_tongues", "atmosphere"],
        "atmosphere" => &["clip", "at", "duration", "lane", "preset", "list", "remove", "ground", "fog", "weather", "tonemap", "auto_exposure", "metre", "fog_density", "fog_falloff", "fog_base", "fog_anisotropy", "fog_range", "fog_patchiness", "bloom_strength", "bloom_threshold", "bloom_knee", "bloom_radius", "exposure", "contrast", "saturation", "temperature", "tint", "richness", "shadow_tint_amount", "highlight_tint_amount", "vignette", "grain", "halation", "fringe", "fog_colour", "shadow_tint", "highlight_tint", "reflection", "air_light"],
        "parallax" => &["clips", "depth", "move", "amount", "ease", "overscan"],
        "roll" | "slide" => &["clip", "by"],
        "duplicate" | "group" | "ungroup" | "link" => &["clips"],
        "freeze" => &["clip", "at", "duration"],
        "replace" => &["clip", "media"],
        "reorder" => &["clip", "before"],
        "copy_look" => &["from", "clips", "parts"],
        "raw" => &["edit"],
        "comp" => &["clip", "shot", "at", "duration", "add", "set", "remove", "output", "reset", "delete"],
        "keying" => &["clip", "screen", "background", "clean_plate", "garbage", "light_wrap", "core", "edge_band", "spill", "despill", "restore", "fix_edges", "clip_black", "clip_white", "gain", "balance"],
        "remove_object" => &["clip", "box", "outline", "method", "range", "step", "grow", "camera"],
        "clean_plate" => &["clip", "samples", "camera"],
        "sky" => &["clip", "sky", "matte", "relight", "haze", "wrap", "edge", "detect", "feather"],
        "wire" => &["clip", "from", "to", "points", "width", "keys"],
        "paint" => &["clip", "strokes"],
        "depth_of_field" => &["clip", "focus", "size", "range", "depth"],
        "grain_match" => &["clip", "reference", "amount"],
        "lens" => &["clip", "k1", "k2", "chroma", "scale", "mode"],
        "motion_blur" => &["clip", "shutter"],
        "matchmove" => &["clip", "focal_mm", "sensor_mm", "nodal", "ignore", "export"],
        "place_on_ground" => &["clip", "camera", "at", "point", "lift", "lie"],
        "export_camera" => &["clip", "path", "units", "first_frame"],
        "import_camera" => &["path", "at", "units"],
        "track" => &["clip", "at", "point", "plane", "radius", "motion", "name", "track", "fix", "both_ways", "retrack", "attach", "follow", "mask", "detach", "clear", "remove"],
        "stabilize" => &["clip", "mode", "smoothness", "rotation", "scale", "max_zoom", "track", "remove"],
        "cut_out" => &["clip", "subject", "at", "point", "points", "exclude", "box", "invert", "smoothing", "enabled", "remove"],
        "roto" => &["clip", "at", "point", "points", "exclude", "box", "every", "detail", "feather", "invert"],
        "match_colour" => &["clip", "clips", "to", "strength", "remove"],
        "smooth_motion" => &["clip", "method", "quality", "estimate", "long", "remove"],
        "shadow" => &["clip", "clips", "at", "auto", "angle", "length", "softness", "opacity", "contact", "colour", "feet", "ground", "enabled", "remove"],
        "light_match" => &["clip", "clips", "at", "amount", "rim", "remove"],
        "foreground" => &["at", "piece", "point", "points", "exclude", "box", "backdrop", "hides", "clip", "remove"],
        _ => return None,
    })
}

/// Free-text fields an agent may add for itself; always allowed.
const NOTES: &[&str] = &["op", "why", "comment", "note_to_self", "inside"];

pub fn is_note(k: &str) -> bool {
    NOTES.contains(&k)
}

/// Property names `set` takes beyond its own fields (resolved per clip;
/// a wrong one is refused there with the clip's property list).
fn is_property(k: &str) -> bool {
    k.contains('.') || matches!(k, "z" | "rotation_x" | "rotation_y" | "rotation_z" | "orientation_x" | "orientation_y" | "orientation_z" | "scale_z" | "anchor_z" | "focal_length" | "fov" | "focus_distance" | "aperture" | "blur_level" | "poi_x" | "poi_y" | "poi_z" | "intensity" | "cone_angle" | "cone_feather" | "radius" | "falloff" | "shadow_darkness" | "scale_x" | "scale_y" | "anchor_x" | "anchor_y" | "crop_left" | "crop_right" | "crop_top" | "crop_bottom" | "blend" | "reversed" | "enabled" | "visible" | "flip" | "flip_x" | "flip_y" | "mirror" | "look_strength" | "look_amount" | "width" | "height" | "fill" | "roundness" | "metallic" | "roughness" | "emissive" | "smooth" | "subsurface" | "emissive_strength" | "wetness" | "glow" | "flare" | "beam" | "source_size" | "flicker" | "flicker_speed")
}

pub fn closest<'a>(want: &str, among: &[&'a str]) -> Option<&'a str> {
    let d = |a: &str, b: &str| {
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
    };
    among.iter().copied().min_by_key(|c| d(want, c)).filter(|c| d(want, c) <= 2.max(want.len() / 3))
}

/// Maps aliases and refuses unknown fields. `what` names the tool or op
/// in messages.
pub fn normalise(name: &str, what: &str, v: &mut Value, allowed: &[&str]) -> Result<(), String> {
    let Some(obj) = v.as_object_mut() else { return Ok(()) };
    for (from, to) in aliases(name) {
        if allowed.contains(to) && !obj.contains_key(*to) {
            if let Some(x) = obj.remove(*from) {
                obj.insert((*to).to_owned(), x);
            }
        }
    }
    // A single path given where a list is wanted.
    if name == "import" {
        if let Some(Value::String(p)) = obj.get("paths").cloned() {
            obj.insert("paths".into(), Value::Array(vec![Value::String(p)]));
        }
    }
    let unknown: Vec<String> = obj.keys().filter(|k| !allowed.contains(&k.as_str()) && !NOTES.contains(&k.as_str()) && !(name == "set" && is_property(k))).cloned().collect();
    if let Some(k) = unknown.first() {
        let hint = closest(k, allowed).map(|c| format!(" (did you mean \"{c}\"?)")).unwrap_or_default();
        return Err(format!("{what} has no field \"{k}\"{hint}; it takes: {}", allowed.join(", ")));
    }
    Ok(())
}

/// An op written as `{"place": {...}}` becomes `{"op": "place", ...}`.
pub fn unwrap_op(v: &Value) -> Value {
    if let Value::Object(o) = v {
        if !o.contains_key("op") && o.len() == 1 {
            let (k, inner) = o.iter().next().expect("one");
            if op_fields(k).is_some() {
                let mut m: Map<String, Value> = inner.as_object().cloned().unwrap_or_default();
                m.insert("op".into(), Value::String(k.clone()));
                return Value::Object(m);
            }
        }
    }
    v.clone()
}

/// How an agent reaches each model edit. A model edit missing here fails
/// the test below, so a new capability never ships without a way in.
pub const REACH: &[(&str, &str)] = &[
    ("InsertClip", "place, add, card, text"),
    ("RemoveClip", "delete"),
    ("SetClip", "set"),
    ("InsertTransition", "transition"),
    ("RemoveTransition", "transition remove"),
    ("SetTransition", "transition"),
    ("InsertMarker", "marker"),
    ("RemoveMarker", "marker remove"),
    ("SetMarker", "marker move"),
    ("InsertLane", "lane add"),
    ("RemoveLane", "lane remove"),
    ("SetLane", "lane"),
    ("MoveLane", "lane move"),
    ("InsertMedia", "import"),
    ("RemoveMedia", "raw (media stays in the bin)"),
    ("SetMedia", "raw"),
    ("InsertSequence", "raw (one timeline per project)"),
    ("RemoveSequence", "raw"),
    ("RenameSequence", "raw"),
    ("SetActiveSequence", "raw"),
    ("SetSettings", "new_project"),
    ("InsertPreset", "raw"),
    ("RemovePreset", "raw"),
    ("SetPreset", "raw"),
    ("SetStory", "the story tool"),
    ("Batch", "edit (every call is one batch)"),
    ("Insert", "place, card"),
    ("Overwrite", "raw"),
    ("Append", "place"),
    ("Split", "split"),
    ("Trim", "trim, set duration"),
    ("RippleDelete", "delete"),
    ("Lift", "delete ripple false"),
    ("Move", "move"),
    ("Reorder", "reorder"),
    ("Slip", "slip"),
    ("Slide", "slide"),
    ("Roll", "roll"),
    ("Duplicate", "duplicate"),
    ("Paste", "duplicate, copy_look"),
    ("PasteAttributes", "copy_look"),
    ("Group", "group"),
    ("Ungroup", "ungroup"),
    ("Link", "link"),
    ("Unlink", "unlink"),
    ("DetachAudio", "detach_audio"),
    ("ReplaceMedia", "replace"),
    ("FreezeFrame", "freeze"),
    ("Reverse", "set reversed"),
    ("SetSpeed", "set speed"),
    ("AddTransition", "transition"),
    ("AddEffect", "apply, fx_layer"),
    ("RemoveEffect", "effect remove"),
    ("SetEffect", "effect, set <effect>.<setting>"),
    ("MoveEffect", "effect move_to"),
    ("SetGrade", "set exposure / contrast / grade.<name>"),
    ("AddMask", "mask"),
    ("RemoveMask", "mask remove"),
    ("SetMask", "mask"),
    ("SetKeyframe", "animate, keyframe"),
    ("RemoveKeyframe", "keys remove"),
    ("SetTrack", "animate"),
    ("AddMarker", "marker"),
    ("DeleteMarker", "marker remove"),
    ("ChangeMarker", "marker move"),
    ("AddLane", "lane add"),
    ("DeleteLane", "lane remove"),
    ("CreateCompound", "merge"),
    ("ExpandCompound", "unmerge"),
];

#[cfg(test)]
mod tests {
    #[test]
    fn every_model_edit_has_a_way_in() {
        let src = include_str!("../../../crates/project/model/src/edit/mod.rs");
        let body = &src[src.find("pub enum Edit").expect("Edit enum")..];
        let body = &body[..body.find("\n}").expect("end of enum")];
        let variants: Vec<&str> = body.lines().filter_map(|l| l.strip_prefix("    ")).filter(|l| l.chars().next().is_some_and(char::is_uppercase)).map(|l| l.split(|c: char| !c.is_alphanumeric()).next().unwrap_or("")).collect();
        assert!(variants.len() > 40, "parsed {} variants", variants.len());
        let missing: Vec<&&str> = variants.iter().filter(|v| !super::REACH.iter().any(|(n, _)| n == *v)).collect();
        assert!(missing.is_empty(), "model edits with no agent op: {missing:?}");
        let raw = super::REACH.iter().filter(|(_, how)| how.starts_with("raw")).count();
        assert!(raw <= 13, "{raw} edits only reachable through raw");
    }
}
