// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The node catalogue: inputs and generators, merges, transforms, colour,
//! filters and matte operations, channels. Keyers live in [`super::key`],
//! lens and camera nodes in [`super::lens`], cleanup in [`super::clean`],
//! roto in [`super::roto`].

use std::sync::{Arc, OnceLock};

use super::geo::{self, Mat3};
use super::{Node, NodeCtx, Plate};
use crate::image::{Edge, Image, Px, luma, premultiply, unpremultiply};
use crate::ops::gaussian_blur;
use crate::params::{ParamDef, ParamSet, Unit};

/// Groups in the node menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NodeCategory {
    Input,
    Merge,
    Transform,
    Colour,
    Filter,
    Keyer,
    Matte,
    Channel,
    Lens,
    Cleanup,
}

impl NodeCategory {
    pub fn label(self) -> &'static str {
        match self {
            NodeCategory::Input => "Inputs",
            NodeCategory::Merge => "Merge",
            NodeCategory::Transform => "Transform",
            NodeCategory::Colour => "Colour",
            NodeCategory::Filter => "Filter",
            NodeCategory::Keyer => "Keying",
            NodeCategory::Matte => "Mattes & roto",
            NodeCategory::Channel => "Channels",
            NodeCategory::Lens => "Lens & camera",
            NodeCategory::Cleanup => "Cleanup & paint",
        }
    }

    pub const ALL: [NodeCategory; 10] = [
        NodeCategory::Input,
        NodeCategory::Merge,
        NodeCategory::Transform,
        NodeCategory::Colour,
        NodeCategory::Filter,
        NodeCategory::Keyer,
        NodeCategory::Matte,
        NodeCategory::Channel,
        NodeCategory::Lens,
        NodeCategory::Cleanup,
    ];
}

/// What a node does with its inputs.
pub type RunFn = fn(&NodeCtx, &[Option<Arc<Plate>>], &ParamSet) -> Result<Plate, String>;

/// One kind of node.
#[derive(Clone, Copy)]
pub struct NodeDef {
    /// Stable id, used in project files.
    pub id: &'static str,
    pub name: &'static str,
    pub category: NodeCategory,
    /// One line for the menu and the AI.
    pub description: &'static str,
    /// Input slot names; the first is the main input (passed through when
    /// the node is off, mixed against by `mix`).
    pub inputs: &'static [&'static str],
    pub params: &'static [ParamDef],
    /// What the node reads from [`Node::data`], for the AI and the docs;
    /// empty when nothing.
    pub data: &'static str,
    /// A WGSL compute shader for the GPU path (see `wgsl/comp_common.wgsl`
    /// for the bindings), when the node is per-pixel.
    pub wgsl: Option<&'static str>,
    pub run: RunFn,
    /// Frames (relative) of the inputs the node also needs.
    pub offsets: fn(&Node) -> Vec<i32>,
    /// Whether the result changes over time by itself.
    pub animated: fn(&Node) -> bool,
}

impl std::fmt::Debug for NodeDef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeDef").field("id", &self.id).finish()
    }
}

fn no_offsets(_: &Node) -> Vec<i32> {
    Vec::new()
}

fn still(_: &Node) -> bool {
    false
}

fn always(_: &Node) -> bool {
    true
}

fn nothing(ctx: &NodeCtx, _: &[Option<Arc<Plate>>], _: &ParamSet) -> Result<Plate, String> {
    Ok(ctx.blank())
}

pub(crate) const BASE: NodeDef = NodeDef {
    id: "",
    name: "",
    category: NodeCategory::Filter,
    description: "",
    inputs: &["in"],
    params: &[],
    data: "",
    wgsl: None,
    run: nothing,
    offsets: no_offsets,
    animated: still,
};

const AB: &[&str] = &["B", "A"];

/// Every node kind, in menu order.
pub fn all() -> &'static [&'static NodeDef] {
    static ALL: OnceLock<Vec<&'static NodeDef>> = OnceLock::new();
    ALL.get_or_init(|| [NODES, super::key::NODES, super::roto::NODES, super::lens::NODES, super::clean::NODES].into_iter().flatten().collect())
}

pub fn find(id: &str) -> Option<&'static NodeDef> {
    all().iter().copied().find(|d| d.id == id)
}

/// Every kind id, comma-separated (for error messages).
pub fn names() -> String {
    all().iter().map(|d| d.id).collect::<Vec<_>>().join(", ")
}

// --------------------------------------------------------------- helpers

/// The plate in slot `i`, if connected.
pub(crate) fn slot(inputs: &[Option<Arc<Plate>>], i: usize) -> Option<&Plate> {
    inputs.get(i).and_then(|p| p.as_deref())
}

/// The main input or a blank plate.
pub(crate) fn main_or_blank(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>]) -> Plate {
    slot(inputs, 0).cloned().unwrap_or_else(|| ctx.blank())
}

/// A per-pixel map over the main input's RGBA.
pub(crate) fn map_main(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], f: impl Fn(usize, usize, Px) -> Px + Sync) -> Plate {
    let p = main_or_blank(ctx, inputs);
    let rgba = p.rgba.map_xy(f);
    p.with_rgba(rgba)
}

/// Picture of `src` fitted into `w`×`h`: 0 fit, 1 fill, 2 stretch, 3 none
/// (centred at its own size).
pub(crate) fn fit_into(src: &Image, w: usize, h: usize, mode: usize) -> Image {
    if (src.width, src.height) == (w, h) || src.width == 0 || src.height == 0 {
        return src.clone();
    }
    let (sw, sh) = (src.width as f32, src.height as f32);
    let (kx, ky) = match mode {
        2 => (w as f32 / sw, h as f32 / sh),
        3 => (1.0, 1.0),
        1 => {
            let k = (w as f32 / sw).max(h as f32 / sh);
            (k, k)
        }
        _ => {
            let k = (w as f32 / sw).min(h as f32 / sh);
            (k, k)
        }
    };
    let (ox, oy) = ((w as f32 - sw * kx) * 0.5, (h as f32 - sh * ky) * 0.5);
    Image::from_fn(w, h, |x, y| src.sample((x as f32 + 0.5 - ox) / kx, (y as f32 + 0.5 - oy) / ky, Edge::Transparent))
}

/// Straight colour of a premultiplied pixel and its alpha.
#[inline]
pub(crate) fn straight(p: Px) -> ([f32; 3], f32) {
    (unpremultiply(p), p[3])
}

// --------------------------------------------------------------- catalogue

const FIT: ParamDef = ParamDef::choice("fit", "Fit", &["Fit", "Fill", "Stretch", "None"], 0);

pub static NODES: &[NodeDef] = &[
    NodeDef { id: "input", name: "Input", category: NodeCategory::Input, description: "The picture coming in: the clip, or everything below an adjustment layer.", inputs: &[], ..BASE },
    NodeDef {
        id: "read",
        name: "Read",
        category: NodeCategory::Input,
        description: "A picture or video file: a background plate, a clean plate, a CG render, a depth or matte pass.",
        inputs: &[],
        params: &[
            ParamDef::float("start", "Start", -36000.0, 36000.0, 0.0).unit(Unit::Seconds).fixed(),
            ParamDef::int("offset", "Frame offset", -100000, 100000, 0),
            FIT,
            ParamDef::choice("layer", "Use as", &["Picture", "Matte"], 0),
            ParamDef::bool("hold", "Hold first frame", false),
        ],
        data: "path: the file",
        run: read,
        animated: read_animated,
        ..BASE
    },
    NodeDef {
        id: "constant",
        name: "Constant",
        category: NodeCategory::Input,
        description: "A flat colour.",
        inputs: &[],
        params: &[ParamDef::colour("colour", "Colour", [0.0, 0.0, 0.0, 1.0])],
        run: constant,
        ..BASE
    },
    NodeDef {
        id: "ramp",
        name: "Ramp",
        category: NodeCategory::Input,
        description: "A linear or radial gradient between two colours.",
        inputs: &[],
        params: &[
            ParamDef::point("from", "From", [0.5, 0.0]),
            ParamDef::point("to", "To", [0.5, 1.0]),
            ParamDef::colour("colour_a", "Colour A", [1.0, 1.0, 1.0, 1.0]),
            ParamDef::colour("colour_b", "Colour B", [0.0, 0.0, 0.0, 1.0]),
            ParamDef::choice("shape", "Shape", &["Linear", "Radial"], 0),
        ],
        run: ramp,
        ..BASE
    },
    NodeDef {
        id: "noise",
        name: "Noise",
        category: NodeCategory::Input,
        description: "Smooth fractal noise (clouds, smoke, mattes that wander), evolving over time.",
        inputs: &[],
        params: &[
            ParamDef::float("size", "Size", 1.0, 2000.0, 120.0),
            ParamDef::int("octaves", "Detail", 1, 8, 4),
            ParamDef::float("evolve", "Evolution speed", 0.0, 10.0, 0.5),
            ParamDef::float("gain", "Contrast", 0.0, 4.0, 1.0),
            ParamDef::point("drift", "Drift per second", [0.0, 0.0]),
        ],
        run: noise,
        animated: always,
        ..BASE
    },
    NodeDef {
        id: "merge",
        name: "Merge",
        category: NodeCategory::Merge,
        description: "Combines A with B: over, under, plus, multiply, screen, in, out, atop, mask, stencil and more (B is the main input).",
        inputs: AB,
        params: &[ParamDef::choice("operation", "Operation", MERGE_OPS, 0)],
        run: merge,
        ..BASE
    },
    NodeDef {
        id: "keymix",
        name: "Keymix",
        category: NodeCategory::Merge,
        description: "A where the matte's alpha is, B elsewhere.",
        inputs: &["B", "A", "matte"],
        params: &[ParamDef::bool("invert", "Invert matte", false)],
        run: keymix,
        ..BASE
    },
    NodeDef {
        id: "switch",
        name: "Switch",
        category: NodeCategory::Merge,
        description: "Passes one of its inputs, chosen by number (keyframe it to cut between versions).",
        inputs: &["0", "1", "2", "3"],
        params: &[ParamDef::int("which", "Which", 0, 3, 0).animated()],
        run: switch,
        ..BASE
    },
    NodeDef { id: "dot", name: "Dot", category: NodeCategory::Merge, description: "Passes its input through (tidies the graph).", run: dot, ..BASE },
    NodeDef {
        id: "transform",
        name: "Transform",
        category: NodeCategory::Transform,
        description: "Moves, turns and scales, with motion blur from its keyframes.",
        params: &[
            ParamDef::float("translate_x", "Move X", -100000.0, 100000.0, 0.0),
            ParamDef::float("translate_y", "Move Y", -100000.0, 100000.0, 0.0),
            ParamDef::float("rotate", "Rotate", -36000.0, 36000.0, 0.0).unit(Unit::Degrees),
            ParamDef::float("scale", "Scale", 0.0, 100.0, 1.0).unit(Unit::Times),
            ParamDef::float("scale_y", "Scale Y", 0.0, 100.0, 1.0).unit(Unit::Times),
            ParamDef::point("centre", "Centre", [0.5, 0.5]),
            ParamDef::bool("invert", "Invert", false),
            ParamDef::choice("edge", "Edges", &["Transparent", "Clamp", "Mirror", "Wrap"], 0),
            ParamDef::float("motion_blur", "Motion blur (shutter)", 0.0, 2.0, 0.0).unit(Unit::Fraction),
            ParamDef::int("samples", "Blur samples", 2, 64, 12),
        ],
        run: transform,
        ..BASE
    },
    NodeDef {
        id: "corner_pin",
        name: "Corner pin",
        category: NodeCategory::Transform,
        description: "Pins the picture's four corners to four points (screens, signs, windows).",
        params: &[
            ParamDef::point("to1", "Top left", [0.0, 0.0]),
            ParamDef::point("to2", "Top right", [1.0, 0.0]),
            ParamDef::point("to3", "Bottom right", [1.0, 1.0]),
            ParamDef::point("to4", "Bottom left", [0.0, 1.0]),
            ParamDef::bool("invert", "Invert (unpin)", false),
        ],
        run: corner_pin,
        ..BASE
    },
    NodeDef {
        id: "crop",
        name: "Crop",
        category: NodeCategory::Transform,
        description: "Clears everything outside a box, with soft edges.",
        params: &[
            ParamDef::point("top_left", "Top left", [0.0, 0.0]),
            ParamDef::point("bottom_right", "Bottom right", [1.0, 1.0]),
            ParamDef::float("softness", "Softness", 0.0, 500.0, 0.0),
        ],
        run: crop,
        ..BASE
    },
    NodeDef {
        id: "track",
        name: "Tracker",
        category: NodeCategory::Transform,
        description: "Applies a 2D track: match-move (the input follows it) or stabilise (the input is held still).",
        params: &[
            ParamDef::choice("mode", "Mode", &["Match move", "Stabilise"], 0),
            ParamDef::bool("translate", "Position", true),
            ParamDef::bool("rotate", "Rotation", true),
            ParamDef::bool("scale", "Scale", true),
            ParamDef::choice("edge", "Edges", &["Transparent", "Clamp", "Mirror"], 0),
        ],
        data: "samples: [[frame, x, y, rotation°, scale], …] in pixels per clip frame; reference: the frame where the input sits as it is",
        run: track,
        animated: always,
        ..BASE
    },
    NodeDef {
        id: "grade",
        name: "Grade",
        category: NodeCategory::Colour,
        description: "Black and white points, lift, gain, multiply, offset, gamma and saturation, in linear light.",
        params: &[
            ParamDef::float("blackpoint", "Black point", -1.0, 1.0, 0.0),
            ParamDef::float("whitepoint", "White point", 0.01, 4.0, 1.0),
            ParamDef::float("lift", "Lift", -1.0, 1.0, 0.0),
            ParamDef::float("gain", "Gain", 0.0, 8.0, 1.0),
            ParamDef::float("multiply", "Multiply", 0.0, 8.0, 1.0),
            ParamDef::float("offset", "Offset", -1.0, 1.0, 0.0),
            ParamDef::float("gamma", "Gamma", 0.05, 5.0, 1.0),
            ParamDef::colour("tint", "Tint (multiply)", [1.0, 1.0, 1.0, 1.0]),
            ParamDef::float("saturation", "Saturation", 0.0, 4.0, 1.0),
            ParamDef::bool("clamp_black", "Clamp black", true),
        ],
        run: grade,
        ..BASE
    },
    NodeDef {
        id: "colour_match",
        name: "Colour match",
        category: NodeCategory::Colour,
        description: "Matches the input's colour and contrast to a reference plate (sits CG and inserts into footage).",
        inputs: &["in", "reference"],
        params: &[
            ParamDef::choice("match", "Match", &["Colour and contrast", "Colour", "Brightness"], 0),
            ParamDef::float("amount", "Amount", 0.0, 1.0, 1.0).unit(Unit::Fraction),
        ],
        run: colour_match,
        ..BASE
    },
    NodeDef {
        id: "invert",
        name: "Invert",
        category: NodeCategory::Colour,
        description: "Inverts colour, alpha or both.",
        params: &[ParamDef::choice("channels", "Channels", &["Alpha", "Colour", "All"], 0)],
        run: invert,
        ..BASE
    },
    NodeDef {
        id: "clamp",
        name: "Clamp",
        category: NodeCategory::Colour,
        description: "Holds values between a minimum and maximum (removes negative and superwhite values).",
        params: &[ParamDef::float("min", "Minimum", -1.0, 1.0, 0.0), ParamDef::float("max", "Maximum", 0.0, 100.0, 1.0)],
        run: clamp,
        ..BASE
    },
    NodeDef { id: "premult", name: "Premultiply", category: NodeCategory::Channel, description: "Multiplies colour by alpha (after a Shuffle or Copy that set alpha).", run: premult, ..BASE },
    NodeDef { id: "unpremult", name: "Unpremultiply", category: NodeCategory::Channel, description: "Divides colour by alpha.", run: unpremult, ..BASE },
    NodeDef {
        id: "blur",
        name: "Blur",
        category: NodeCategory::Filter,
        description: "Gaussian blur of colour, alpha or both.",
        params: &[
            ParamDef::float("size", "Size", 0.0, 1000.0, 4.0),
            ParamDef::float("aspect", "Vertical / horizontal", 0.0, 10.0, 1.0),
            ParamDef::choice("channels", "Channels", &["All", "Colour", "Alpha"], 0),
        ],
        run: blur,
        ..BASE
    },
    NodeDef {
        id: "erode",
        name: "Dilate / erode",
        category: NodeCategory::Matte,
        description: "Shrinks (positive) or grows (negative) a matte by pixels, then softens it.",
        params: &[
            ParamDef::float("size", "Size", -200.0, 200.0, 1.0),
            ParamDef::float("soften", "Soften", 0.0, 200.0, 0.0),
            ParamDef::choice("channels", "Channels", &["Alpha", "All"], 0),
        ],
        run: erode,
        ..BASE
    },
    NodeDef {
        id: "median",
        name: "Median",
        category: NodeCategory::Filter,
        description: "Median filter: removes grain and specks while keeping edges.",
        params: &[ParamDef::int("radius", "Radius", 1, 4, 1)],
        run: median,
        ..BASE
    },
    NodeDef {
        id: "effect",
        name: "Effect",
        category: NodeCategory::Filter,
        description: "Any effect from the effects browser as a node (its parameters are the effect's).",
        inputs: &["in", "aux"],
        data: "effect: the effect id",
        run: effect,
        offsets: effect_offsets,
        animated: effect_animated,
        ..BASE
    },
    NodeDef {
        id: "shuffle",
        name: "Shuffle",
        category: NodeCategory::Channel,
        description: "Routes channels: any of R, G, B, A, depth or luma of either input into R, G, B, A and depth.",
        inputs: AB,
        params: &[
            ParamDef::choice("r", "Red from", SHUFFLE_FROM, 0),
            ParamDef::choice("g", "Green from", SHUFFLE_FROM, 1),
            ParamDef::choice("b", "Blue from", SHUFFLE_FROM, 2),
            ParamDef::choice("a", "Alpha from", SHUFFLE_FROM, 3),
            ParamDef::choice("depth", "Depth from", DEPTH_FROM, 0),
            ParamDef::choice("motion", "Motion from", &["Keep", "A", "None"], 0),
            ParamDef::choice("id", "Id from", &["Keep", "A", "None"], 0),
        ],
        run: shuffle,
        ..BASE
    },
    NodeDef {
        id: "set_depth",
        name: "Depth from picture",
        category: NodeCategory::Channel,
        description: "Makes the depth channel from a depth pass picture (an AI depth map: white near), for depth of field, fog and depth mattes.",
        inputs: &["in", "depth pass"],
        params: &[
            ParamDef::choice("source", "The picture is", &["White near", "White far", "Disparity"], 0),
            ParamDef::float("near", "Near distance", 0.0, 100000.0, 1.0),
            ParamDef::float("far", "Far distance", 0.0, 100000.0, 100.0),
        ],
        run: set_depth,
        ..BASE
    },
    NodeDef {
        id: "id_from",
        name: "Ids from mattes",
        category: NodeCategory::Channel,
        description: "Builds the object-id channel from mattes: id n where matte n is (cut-outs, rotos, AI selections).",
        inputs: &["in", "1", "2", "3", "4", "5", "6"],
        run: id_from,
        ..BASE
    },
    NodeDef {
        id: "id_matte",
        name: "Id matte",
        category: NodeCategory::Matte,
        description: "Alpha from one object id.",
        params: &[ParamDef::int("id", "Id", 0, 1000, 1), ParamDef::choice("output", "Output", MATTE_OUT, 0)],
        run: id_matte,
        ..BASE
    },
    NodeDef {
        id: "depth_matte",
        name: "Depth matte",
        category: NodeCategory::Matte,
        description: "Alpha from a depth range (near to far, with soft edges).",
        params: &[
            ParamDef::float("near", "Near", 0.0, 100000.0, 0.0),
            ParamDef::float("far", "Far", 0.0, 100000.0, 10.0),
            ParamDef::float("softness", "Softness", 0.0, 100000.0, 1.0),
            ParamDef::bool("invert", "Invert", false),
            ParamDef::choice("output", "Output", MATTE_OUT, 0),
        ],
        run: depth_matte,
        ..BASE
    },
    NodeDef {
        id: "luma_matte",
        name: "Luma matte",
        category: NodeCategory::Matte,
        description: "Alpha from brightness between a low and a high level.",
        params: &[
            ParamDef::float("low", "Low", 0.0, 1.0, 0.25).unit(Unit::Fraction),
            ParamDef::float("high", "High", 0.0, 1.0, 0.75).unit(Unit::Fraction),
            ParamDef::bool("invert", "Invert", false),
            ParamDef::choice("output", "Output", MATTE_OUT, 0),
        ],
        run: luma_matte,
        ..BASE
    },
    NodeDef {
        id: "copy_alpha",
        name: "Copy alpha",
        category: NodeCategory::Matte,
        description: "Takes alpha from A (a roto, key or AI matte) and premultiplies B by it.",
        inputs: AB,
        params: &[ParamDef::choice("operation", "Combine", &["Replace", "Multiply", "Add", "Subtract"], 0), ParamDef::bool("invert", "Invert A", false)],
        run: copy_alpha,
        ..BASE
    },
    NodeDef {
        id: "motion_vectors",
        name: "Motion vectors",
        category: NodeCategory::Channel,
        description: "Measures how every pixel moved since the previous frame (dense optical flow) into the motion channel.",
        params: &[ParamDef::float("smoothness", "Smoothness", 0.0, 1.0, 0.5).unit(Unit::Fraction), ParamDef::int("detail", "Detail levels", 1, 8, 5)],
        run: motion_vectors,
        offsets: prev_frame,
        ..BASE
    },
];

pub const MERGE_OPS: &[&str] = &[
    "Over", "Under", "Plus", "Minus", "From", "Multiply", "Screen", "Overlay", "Difference", "In", "Out", "Atop", "Xor", "Mask", "Stencil", "Max", "Min", "Average", "Copy",
];

pub const SHUFFLE_FROM: &[&str] = &["B red", "B green", "B blue", "B alpha", "B depth", "B luma", "A red", "A green", "A blue", "A alpha", "A depth", "A luma", "Black", "White"];

const DEPTH_FROM: &[&str] = &["Keep", "A depth", "B luma", "A luma", "None"];

pub const MATTE_OUT: &[&str] = &["Cut out (input × matte)", "Matte (white × matte)", "Alpha only (colour kept)"];

fn prev_frame(_: &Node) -> Vec<i32> {
    vec![-1]
}

// ------------------------------------------------------------- generators

/// The file and time a `read` node asks for.
pub fn read_request(n: &Node, p: &ParamSet, t: f64, fps: f64) -> Option<(String, f64)> {
    let path = n.data.get("path").and_then(|v| v.as_str())?.to_owned();
    let s = if p.b("hold") { p.f("start") as f64 } else { p.f("start") as f64 + t + p.i("offset") as f64 / fps.max(1e-6) };
    Some((path, s.max(0.0)))
}

fn read_animated(n: &Node) -> bool {
    // The reader's frame key already changes with the frame, so the node
    // itself only needs the clock when it holds nothing.
    let _ = n;
    false
}

fn read(ctx: &NodeCtx, _: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let Some((path, s)) = read_request(ctx.node, p, ctx.time, ctx.frame_rate) else { return Err(format!("read node {} has no file (data.path)", ctx.node.id)) };
    let Some((img, _)) = ctx.reader.read(&path, s) else { return Err(format!("read node {}: cannot read {path}", ctx.node.id)) };
    let mut out = fit_into(&img, ctx.width, ctx.height, p.choice("fit"));
    if p.choice("layer") == 1 {
        out = out.map(|q| {
            let l = luma(q).clamp(0.0, 1.0);
            [l, l, l, l]
        });
    }
    Ok(Plate::new(out))
}

fn constant(ctx: &NodeCtx, _: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let c = p.colour("colour");
    Ok(Plate::new(Image::filled(ctx.width, ctx.height, premultiply([c[0], c[1], c[2]], c[3]))))
}

fn ramp(ctx: &NodeCtx, _: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let (w, h) = (ctx.width as f32, ctx.height as f32);
    let (a, b) = (p.point("from"), p.point("to"));
    let (a, b) = ([a[0] * w, a[1] * h], [b[0] * w, b[1] * h]);
    let (ca, cb) = (p.colour("colour_a"), p.colour("colour_b"));
    let d = [b[0] - a[0], b[1] - a[1]];
    let len2 = (d[0] * d[0] + d[1] * d[1]).max(1e-6);
    let radial = p.choice("shape") == 1;
    Ok(Plate::new(Image::from_fn(ctx.width, ctx.height, |x, y| {
        let q = [x as f32 + 0.5 - a[0], y as f32 + 0.5 - a[1]];
        let t = if radial { ((q[0] * q[0] + q[1] * q[1]) / len2).sqrt() } else { (q[0] * d[0] + q[1] * d[1]) / len2 };
        let t = t.clamp(0.0, 1.0);
        let c: [f32; 4] = std::array::from_fn(|i| ca[i] + (cb[i] - ca[i]) * t);
        premultiply([c[0], c[1], c[2]], c[3])
    })))
}

fn noise(ctx: &NodeCtx, _: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let size = p.f("size").max(1.0);
    let oct = p.i("octaves").max(1) as u32;
    let z = ctx.time as f32 * p.f("evolve");
    let gain = p.f("gain");
    let dr = p.point("drift");
    let (dx, dy) = (dr[0] * ctx.time as f32 * ctx.width as f32, dr[1] * ctx.time as f32 * ctx.height as f32);
    let seed = ctx.seed ^ ctx.node.id.wrapping_mul(0x9e37_79b9);
    Ok(Plate::new(Image::from_fn(ctx.width, ctx.height, |x, y| {
        let v = crate::math::fbm((x as f32 - dx) / size, (y as f32 - dy) / size, z, oct, seed);
        let v = ((v - 0.5) * gain + 0.5).clamp(0.0, 1.0);
        [v, v, v, 1.0]
    })))
}

// ------------------------------------------------------------------ merges

/// Porter-Duff and blend operations on premultiplied pixels; `a` is the
/// foreground (A), `b` the background (B).
pub fn merge_px(op: usize, a: Px, b: Px) -> Px {
    let (aa, ba) = (a[3], b[3]);
    let union = aa + ba - aa * ba;
    let per = |f: &dyn Fn(f32, f32) -> f32, alpha: f32| -> Px { [f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2]), alpha] };
    match MERGE_OPS.get(op).copied().unwrap_or("Over") {
        "Under" => std::array::from_fn(|i| b[i] + a[i] * (1.0 - ba)),
        "Plus" => std::array::from_fn(|i| a[i] + b[i]),
        "Minus" => std::array::from_fn(|i| a[i] - b[i]),
        "From" => std::array::from_fn(|i| b[i] - a[i]),
        "Multiply" => per(&|x, y| x * y, union),
        "Screen" => per(&|x, y| x + y - x * y, union),
        "Overlay" => per(&|x, y| if y < 0.5 { 2.0 * x * y } else { 1.0 - 2.0 * (1.0 - x) * (1.0 - y) }, union),
        "Difference" => per(&|x, y| (x - y).abs(), union),
        "In" => std::array::from_fn(|i| a[i] * ba),
        "Out" => std::array::from_fn(|i| a[i] * (1.0 - ba)),
        "Atop" => std::array::from_fn(|i| a[i] * ba + b[i] * (1.0 - aa)),
        "Xor" => std::array::from_fn(|i| a[i] * (1.0 - ba) + b[i] * (1.0 - aa)),
        "Mask" => std::array::from_fn(|i| b[i] * aa),
        "Stencil" => std::array::from_fn(|i| b[i] * (1.0 - aa)),
        "Max" => std::array::from_fn(|i| a[i].max(b[i])),
        "Min" => std::array::from_fn(|i| a[i].min(b[i])),
        "Average" => std::array::from_fn(|i| (a[i] + b[i]) * 0.5),
        "Copy" => a,
        _ => std::array::from_fn(|i| a[i] + b[i] * (1.0 - aa)),
    }
}

fn merge(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let b = main_or_blank(ctx, inputs);
    let Some(a) = slot(inputs, 1) else { return Ok(b) };
    let op = p.choice("operation");
    let rgba = b.rgba.map_xy(|x, y, q| merge_px(op, a.rgba.get(x, y), q));
    let mut out = b.with_rgba(rgba);
    // Depth: the nearer surface wins where A covers.
    if let (Some(da), Some(db)) = (&a.depth, &mut out.depth) {
        for (i, d) in db.iter_mut().enumerate() {
            if a.rgba.pixels[i][3] > 0.5 {
                *d = d.min(da[i]);
            }
        }
    } else if out.depth.is_none() {
        out.depth = a.depth.clone();
    }
    for (ach, bch) in [(&a.id, &mut out.id)] {
        if let Some(ai) = ach {
            let bi = bch.get_or_insert_with(|| vec![0.0; ai.len()]);
            for (i, v) in bi.iter_mut().enumerate() {
                if a.rgba.pixels[i][3] > 0.5 {
                    *v = ai[i];
                }
            }
        }
    }
    if out.motion.is_none() {
        out.motion = a.motion.clone();
    }
    Ok(out)
}

fn keymix(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let b = main_or_blank(ctx, inputs);
    let Some(a) = slot(inputs, 1) else { return Ok(b) };
    let inv = p.b("invert");
    let m = slot(inputs, 2).map(|m| m.alpha());
    let rgba = b.rgba.map_xy(|x, y, q| {
        let k = m.as_ref().map_or(1.0, |m| m[y * ctx.width + x].clamp(0.0, 1.0));
        let k = if inv { 1.0 - k } else { k };
        crate::image::lerp_px(q, a.rgba.get(x, y), k)
    });
    Ok(b.with_rgba(rgba))
}

fn switch(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let i = p.i("which").clamp(0, 3) as usize;
    Ok(slot(inputs, i).cloned().unwrap_or_else(|| ctx.blank()))
}

fn dot(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], _: &ParamSet) -> Result<Plate, String> {
    Ok(main_or_blank(ctx, inputs))
}

// -------------------------------------------------------------- transforms

/// The output-to-source matrix of a transform node's values.
pub(crate) fn transform_matrix(p: &ParamSet, w: usize, h: usize) -> Mat3 {
    let c = p.point("centre");
    let (cx, cy) = (c[0] as f64 * w as f64, c[1] as f64 * h as f64);
    let (tx, ty) = (p.f("translate_x") as f64, p.f("translate_y") as f64);
    let (s, c2) = (p.f("rotate") as f64).to_radians().sin_cos();
    let (kx, ky) = ((p.f("scale") as f64).max(1e-6), (p.f("scale") as f64 * p.f("scale_y") as f64).max(1e-6));
    // Forward: translate to centre, scale, rotate (clockwise, y down), back, move.
    let fwd = [[c2 * kx, -s * ky, 0.0], [s * kx, c2 * ky, 0.0], [0.0, 0.0, 1.0]];
    let fwd = geo::mul3(&[[1.0, 0.0, cx + tx], [0.0, 1.0, cy + ty], [0.0, 0.0, 1.0]], &geo::mul3(&fwd, &[[1.0, 0.0, -cx], [0.0, 1.0, -cy], [0.0, 0.0, 1.0]]));
    if p.b("invert") { fwd } else { geo::inverse3(&fwd).unwrap_or(geo::IDENTITY) }
}

pub(crate) fn edge_mode(i: usize) -> Edge {
    match i {
        1 => Edge::Clamp,
        2 => Edge::Mirror,
        3 => Edge::Wrap,
        _ => Edge::Transparent,
    }
}

fn transform(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let src = main_or_blank(ctx, inputs);
    let (w, h) = (ctx.width, ctx.height);
    let edge = edge_mode(p.choice("edge"));
    let shutter = p.f("motion_blur") as f64;
    let moving = ctx.node.keys.values().any(|k| k.len() > 1);
    let mats: Vec<Mat3> = if shutter > 0.0 && moving {
        let n = p.i("samples").max(2) as usize;
        (0..n)
            .map(|k| {
                let t = ctx.time + (k as f64 / (n - 1) as f64 - 0.5) * shutter / ctx.frame_rate.max(1e-6);
                transform_matrix(&ctx.node.params_at(t), w, h)
            })
            .collect()
    } else {
        vec![transform_matrix(p, w, h)]
    };
    let rgba = Image::from_fn(w, h, |x, y| {
        let mut acc = [0.0f32; 4];
        for m in &mats {
            let s = geo::apply_h(m, [x as f64 + 0.5, y as f64 + 0.5]);
            let q = src.rgba.sample(s[0] as f32, s[1] as f32, edge);
            for i in 0..4 {
                acc[i] += q[i];
            }
        }
        acc.map(|v| v / mats.len() as f32)
    });
    let m = mats[mats.len() / 2];
    Ok(warp_channels(&src, rgba, &m))
}

/// The extra channels of `src` warped by an output-to-source matrix.
pub(crate) fn warp_channels(src: &Plate, rgba: Image, m: &Mat3) -> Plate {
    let (w, h) = (src.width(), src.height());
    let warp = |p: &Vec<f32>| -> Vec<f32> {
        let mut out = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let s = geo::apply_h(m, [x as f64 + 0.5, y as f64 + 0.5]);
                out.push(geo::sample_plane(p, w, h, s[0] as f32, s[1] as f32));
            }
        }
        out
    };
    Plate { rgba, depth: src.depth.as_ref().map(warp), motion: src.motion.clone(), id: src.id.as_ref().map(warp) }
}

fn corner_pin(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let src = main_or_blank(ctx, inputs);
    let (w, h) = (ctx.width as f64, ctx.height as f64);
    let from = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]];
    let to = ["to1", "to2", "to3", "to4"].map(|k| {
        let q = p.point(k);
        [q[0] as f64 * w, q[1] as f64 * h]
    });
    // Output to source: the inverse of corners → pins.
    let m = if p.b("invert") { geo::homography(&from, &to) } else { geo::homography(&to, &from) };
    let Some(m) = m else { return Ok(ctx.blank()) };
    let rgba = geo::warp_h(&src.rgba, &m, Edge::Transparent);
    Ok(warp_channels(&src, rgba, &m))
}

fn crop(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let (a, b) = (p.point("top_left"), p.point("bottom_right"));
    let (w, h) = (ctx.width as f32, ctx.height as f32);
    let (x0, y0, x1, y1) = (a[0] * w, a[1] * h, b[0] * w, b[1] * h);
    let soft = p.f("softness").max(1e-3);
    Ok(map_main(ctx, inputs, |x, y, q| {
        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
        let d = (px - x0).min(x1 - px).min(py - y0).min(y1 - py);
        let k = (d / soft + 0.5).clamp(0.0, 1.0);
        q.map(|v| v * k)
    }))
}

/// One sample of a 2D track: where the tracked feature is at a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackSample {
    pub frame: f64,
    pub x: f64,
    pub y: f64,
    pub rotation: f64,
    pub scale: f64,
}

/// The track samples in a `track` node's data, sorted by frame.
pub fn track_samples(data: &serde_json::Value) -> Vec<TrackSample> {
    let mut out: Vec<TrackSample> = data
        .get("samples")
        .and_then(|s| s.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| {
                    let s = s.as_array()?;
                    let g = |i: usize, d: f64| s.get(i).and_then(|v| v.as_f64()).unwrap_or(d);
                    Some(TrackSample { frame: s.first()?.as_f64()?, x: g(1, 0.0), y: g(2, 0.0), rotation: g(3, 0.0), scale: g(4, 1.0) })
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| a.frame.total_cmp(&b.frame));
    out
}

/// A track's sample at a (fractional) frame, interpolated, held at the ends.
pub fn track_at(samples: &[TrackSample], frame: f64) -> Option<TrackSample> {
    let first = samples.first()?;
    if frame <= first.frame {
        return Some(*first);
    }
    let i = samples.iter().position(|s| s.frame > frame);
    let Some(i) = i else { return samples.last().copied() };
    let (a, b) = (samples[i - 1], samples[i]);
    let u = (frame - a.frame) / (b.frame - a.frame).max(1e-9);
    let l = |x: f64, y: f64| x + (y - x) * u;
    Some(TrackSample { frame, x: l(a.x, b.x), y: l(a.y, b.y), rotation: l(a.rotation, b.rotation), scale: l(a.scale, b.scale) })
}

fn track(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let src = main_or_blank(ctx, inputs);
    let samples = track_samples(&ctx.node.data);
    let Some(cur) = track_at(&samples, ctx.time * ctx.frame_rate) else { return Ok(src) };
    let reference = ctx.node.data.get("reference").and_then(|v| v.as_f64()).unwrap_or(samples[0].frame);
    let r = track_at(&samples, reference).unwrap_or(cur);
    let (dx, dy) = if p.b("translate") { (cur.x - r.x, cur.y - r.y) } else { (0.0, 0.0) };
    let rot = if p.b("rotate") { (cur.rotation - r.rotation).to_radians() } else { 0.0 };
    let k = if p.b("scale") { cur.scale / r.scale.max(1e-6) } else { 1.0 };
    // Forward (source → output) for match-move: about the reference point.
    let (s, c) = rot.sin_cos();
    let lin = [[c * k, -s * k, 0.0], [s * k, c * k, 0.0], [0.0, 0.0, 1.0]];
    let fwd = geo::mul3(&[[1.0, 0.0, r.x + dx], [0.0, 1.0, r.y + dy], [0.0, 0.0, 1.0]], &geo::mul3(&lin, &[[1.0, 0.0, -r.x], [0.0, 1.0, -r.y], [0.0, 0.0, 1.0]]));
    let m = if p.choice("mode") == 1 { fwd } else { geo::inverse3(&fwd).unwrap_or(geo::IDENTITY) };
    let rgba = geo::warp_h(&src.rgba, &m, edge_mode(p.choice("edge")));
    Ok(warp_channels(&src, rgba, &m))
}

// ------------------------------------------------------------------ colour

/// Grade (black point, white point, lift, gain, multiply, offset, gamma) of one straight-colour value.
#[inline]
pub(crate) fn grade_value(v: f32, bp: f32, wp: f32, lift: f32, gain: f32, mul: f32, off: f32, gamma: f32, clamp_black: bool) -> f32 {
    let a = mul * (gain - lift) / (wp - bp).max(1e-6);
    let b = off + lift - a * bp;
    let mut o = a * v + b;
    if clamp_black {
        o = o.max(0.0);
    }
    if (gamma - 1.0).abs() > 1e-6 && o > 0.0 { o.powf(1.0 / gamma) } else { o }
}

fn grade(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let (bp, wp, lift, gain, mul, off, gamma) = (p.f("blackpoint"), p.f("whitepoint"), p.f("lift"), p.f("gain"), p.f("multiply"), p.f("offset"), p.f("gamma"));
    let tint = p.colour("tint");
    let sat = p.f("saturation");
    let cb = p.b("clamp_black");
    Ok(map_main(ctx, inputs, |_, _, q| {
        let (c, a) = straight(q);
        if a <= 1e-6 {
            return q;
        }
        let g: [f32; 3] = std::array::from_fn(|i| grade_value(c[i], bp, wp, lift, gain, mul, off, gamma, cb) * tint[i]);
        let l = 0.2126 * g[0] + 0.7152 * g[1] + 0.0722 * g[2];
        let s: [f32; 3] = std::array::from_fn(|i| l + (g[i] - l) * sat);
        premultiply(s, a)
    }))
}

/// Mean and spread of `log2(channel)` over the covered pixels.
pub(crate) fn log_stats(img: &Image) -> ([f32; 3], [f32; 3]) {
    let mut sum = [0.0f64; 3];
    let mut sq = [0.0f64; 3];
    let mut wsum = 0.0f64;
    for q in &img.pixels {
        let (c, a) = straight(*q);
        if a < 0.5 {
            continue;
        }
        wsum += 1.0;
        for i in 0..3 {
            let l = (c[i].max(1e-4) as f64).log2();
            sum[i] += l;
            sq[i] += l * l;
        }
    }
    if wsum < 1.0 {
        return ([0.0; 3], [1.0; 3]);
    }
    let mean = sum.map(|s| s / wsum);
    let sd: [f64; 3] = std::array::from_fn(|i| (sq[i] / wsum - mean[i] * mean[i]).max(1e-8).sqrt());
    (mean.map(|v| v as f32), sd.map(|v| v as f32))
}

fn colour_match(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let src = main_or_blank(ctx, inputs);
    let Some(r) = slot(inputs, 1) else { return Ok(src) };
    let (mi, si) = log_stats(&src.rgba);
    let (mr, sr) = log_stats(&r.rgba);
    let mode = p.choice("match");
    let amount = p.f("amount");
    // Brightness only: shift every channel by the luma difference.
    let lum = |m: [f32; 3]| 0.2126 * m[0] + 0.7152 * m[1] + 0.0722 * m[2];
    let rgba = src.rgba.map(|q| {
        let (c, a) = straight(q);
        if a <= 1e-6 {
            return q;
        }
        let o: [f32; 3] = std::array::from_fn(|i| {
            let l = c[i].max(1e-4).log2();
            let m = match mode {
                0 => (l - mi[i]) * (sr[i] / si[i]).clamp(0.25, 4.0) + mr[i],
                1 => l - mi[i] + mr[i],
                _ => l - lum(mi) + lum(mr),
            };
            let v = m.exp2();
            c[i] + (v - c[i]) * amount
        });
        premultiply(o, a)
    });
    Ok(src.with_rgba(rgba))
}

fn invert(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let ch = p.choice("channels");
    Ok(map_main(ctx, inputs, |_, _, q| {
        let (c, a) = straight(q);
        let a2 = if ch != 1 { 1.0 - a.clamp(0.0, 1.0) } else { a };
        let c2 = if ch != 0 { c.map(|v| 1.0 - v) } else { c };
        // Colour inverted where it is seen; alpha inverted keeps colour.
        if ch == 0 { [c[0] * a2, c[1] * a2, c[2] * a2, a2] } else { premultiply(c2, a2) }
    }))
}

fn clamp(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let (lo, hi) = (p.f("min"), p.f("max").max(p.f("min")));
    Ok(map_main(ctx, inputs, |_, _, q| [q[0].clamp(lo, hi), q[1].clamp(lo, hi), q[2].clamp(lo, hi), q[3].clamp(0.0, 1.0)]))
}

fn premult(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], _: &ParamSet) -> Result<Plate, String> {
    Ok(map_main(ctx, inputs, |_, _, q| [q[0] * q[3], q[1] * q[3], q[2] * q[3], q[3]]))
}

fn unpremult(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], _: &ParamSet) -> Result<Plate, String> {
    Ok(map_main(ctx, inputs, |_, _, q| if q[3] > 1e-6 { [q[0] / q[3], q[1] / q[3], q[2] / q[3], q[3]] } else { q }))
}

// ------------------------------------------------------------------ filters

fn blur(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let src = main_or_blank(ctx, inputs);
    let s = p.f("size");
    let b = gaussian_blur(&src.rgba, s, s * p.f("aspect"), Edge::Clamp);
    let ch = p.choice("channels");
    let rgba = match ch {
        0 => b,
        1 => src.rgba.map_xy(|x, y, q| {
            let o = b.get(x, y);
            [o[0], o[1], o[2], q[3]]
        }),
        _ => src.rgba.map_xy(|x, y, q| {
            let a = b.get(x, y)[3];
            let k = if q[3] > 1e-6 { a / q[3] } else { 0.0 };
            [q[0] * k.min(1.0), q[1] * k.min(1.0), q[2] * k.min(1.0), a]
        }),
    };
    Ok(src.with_rgba(rgba))
}

fn erode(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let src = main_or_blank(ctx, inputs);
    let (w, h) = (ctx.width, ctx.height);
    let size = p.f("size");
    let soft = p.f("soften");
    if p.choice("channels") == 1 {
        let planes: Vec<Vec<f32>> = (0..4).map(|c| {
            let pl: Vec<f32> = src.rgba.pixels.iter().map(|q| q[c]).collect();
            geo::blur_plane(&geo::erode_plane(&pl, w, h, size), w, h, soft)
        }).collect();
        let rgba = Image::from_fn(w, h, |x, y| std::array::from_fn(|c| planes[c][y * w + x]));
        return Ok(src.with_rgba(rgba));
    }
    let a = geo::blur_plane(&geo::erode_plane(&src.alpha(), w, h, size), w, h, soft);
    let rgba = src.rgba.map_xy(|x, y, q| {
        let na = a[y * w + x].clamp(0.0, 1.0);
        let k = if q[3] > 1e-6 { (na / q[3]).min(1.0) } else { 0.0 };
        [q[0] * k, q[1] * k, q[2] * k, na]
    });
    Ok(src.with_rgba(rgba))
}

fn median(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let src = main_or_blank(ctx, inputs);
    let r = p.i("radius").clamp(1, 4) as isize;
    let img = &src.rgba;
    let rgba = Image::from_fn(img.width, img.height, |x, y| {
        let mut out = [0.0; 4];
        let mut v: Vec<f32> = Vec::with_capacity(((2 * r + 1) * (2 * r + 1)) as usize);
        for (c, o) in out.iter_mut().enumerate() {
            v.clear();
            for dy in -r..=r {
                for dx in -r..=r {
                    v.push(img.fetch(x as isize + dx, y as isize + dy, Edge::Clamp)[c]);
                }
            }
            let mid = v.len() / 2;
            v.select_nth_unstable_by(mid, f32::total_cmp);
            *o = v[mid];
        }
        out
    });
    Ok(src.with_rgba(rgba))
}

fn effect_offsets(n: &Node) -> Vec<i32> {
    let Some(e) = n.effect_def() else { return Vec::new() };
    let k = e.history_needed(&n.params_at(0.0), 30.0).min(16);
    (1..=k as i32).map(|i| -i).collect()
}

fn effect_animated(n: &Node) -> bool {
    n.effect_def().is_some_and(|e| e.animated)
}

fn effect(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let src = main_or_blank(ctx, inputs);
    let def = ctx.node.effect_def().ok_or("an effect node needs data.effect")?;
    let history: Vec<Image> = ctx.neighbours.iter().filter_map(|(_, at)| at.first().cloned().flatten().map(|p| p.rgba.clone())).collect();
    let aux = slot(inputs, 1).map(|a| &a.rgba);
    let ectx = crate::EffectContext { time: ctx.time as f32, frame_rate: ctx.frame_rate as f32, seed: ctx.seed ^ ctx.node.id, history: &history, aux };
    Ok(src.with_rgba(def.apply(&ectx, &src.rgba, p)))
}

// ------------------------------------------------------------------ channels

fn shuffle(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let b = main_or_blank(ctx, inputs);
    let a = slot(inputs, 1);
    let (w, h) = (ctx.width, ctx.height);
    let pick = |src: usize, x: usize, y: usize| -> f32 {
        let i = y * w + x;
        let (pl, ch) = if src < 6 { (Some(&b), src) } else if src < 12 { (a, src - 6) } else { return if src == 13 { 1.0 } else { 0.0 } };
        let Some(pl) = pl else { return 0.0 };
        match ch {
            0..=3 => pl.rgba.pixels[i][ch],
            4 => pl.depth.as_ref().map_or(0.0, |d| d[i]),
            _ => luma(pl.rgba.pixels[i]),
        }
    };
    let (r, g, bb, al) = (p.choice("r"), p.choice("g"), p.choice("b"), p.choice("a"));
    let rgba = Image::from_fn(w, h, |x, y| [pick(r, x, y), pick(g, x, y), pick(bb, x, y), pick(al, x, y)]);
    let mut out = b.with_rgba(rgba);
    out.depth = match p.choice("depth") {
        1 => a.and_then(|a| a.depth.clone()),
        2 => Some(b.rgba.pixels.iter().map(|q| luma(*q)).collect()),
        3 => a.map(|a| a.rgba.pixels.iter().map(|q| luma(*q)).collect()),
        4 => None,
        _ => b.depth.clone(),
    };
    out.motion = match p.choice("motion") {
        1 => a.and_then(|a| a.motion.clone()),
        2 => None,
        _ => b.motion.clone(),
    };
    out.id = match p.choice("id") {
        1 => a.and_then(|a| a.id.clone()),
        2 => None,
        _ => b.id.clone(),
    };
    Ok(out)
}

fn set_depth(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let src = main_or_blank(ctx, inputs);
    let pass = slot(inputs, 1).unwrap_or(&src);
    let (near, far) = (p.f("near"), p.f("far").max(p.f("near") + 1e-4));
    let mode = p.choice("source");
    let depth = pass
        .rgba
        .pixels
        .iter()
        .map(|q| {
            // Depth passes are stored display-encoded: read them as seen.
            let l = crate::image::linear_to_srgb(luma(*q).clamp(0.0, 1.0));
            match mode {
                1 => near + l * (far - near),
                2 => (near / l.max(near / far)).clamp(near, far),
                _ => far + l * (near - far),
            }
        })
        .collect();
    let mut out = src;
    out.depth = Some(depth);
    Ok(out)
}

fn id_from(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], _: &ParamSet) -> Result<Plate, String> {
    let mut out = main_or_blank(ctx, inputs);
    let mut ids = vec![0.0f32; ctx.width * ctx.height];
    for k in 1..inputs.len() {
        if let Some(m) = slot(inputs, k) {
            for (i, v) in ids.iter_mut().enumerate() {
                if m.rgba.pixels[i][3] > 0.5 {
                    *v = k as f32;
                }
            }
        }
    }
    out.id = Some(ids);
    Ok(out)
}

/// Applies a matte to the main input per a [`MATTE_OUT`] choice.
pub(crate) fn matte_output(src: &Plate, matte: &[f32], out: usize) -> Plate {
    let rgba = src.rgba.map_xy(|x, y, q| {
        let m = matte[y * src.width() + x].clamp(0.0, 1.0);
        match out {
            1 => [m, m, m, m],
            2 => {
                let (c, _) = straight(q);
                premultiply(c, m)
            }
            _ => q.map(|v| v * m),
        }
    });
    src.with_rgba(rgba)
}

fn id_matte(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let src = main_or_blank(ctx, inputs);
    let want = p.i("id") as f32;
    let m: Vec<f32> = match &src.id {
        Some(ids) => ids.iter().map(|&v| if (v - want).abs() < 0.5 { 1.0 } else { 0.0 }).collect(),
        None => vec![0.0; ctx.width * ctx.height],
    };
    Ok(matte_output(&src, &m, p.choice("output")))
}

fn depth_matte(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let src = main_or_blank(ctx, inputs);
    let (near, far, soft) = (p.f("near"), p.f("far"), p.f("softness").max(1e-6));
    let inv = p.b("invert");
    let m: Vec<f32> = match &src.depth {
        Some(d) => d
            .iter()
            .map(|&z| {
                let k = crate::math::smoothstep(near - soft, near, z) * (1.0 - crate::math::smoothstep(far, far + soft, z));
                if inv { 1.0 - k } else { k }
            })
            .collect(),
        None => vec![if inv { 1.0 } else { 0.0 }; ctx.width * ctx.height],
    };
    Ok(matte_output(&src, &m, p.choice("output")))
}

fn luma_matte(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let src = main_or_blank(ctx, inputs);
    let (lo, hi) = (p.f("low"), p.f("high"));
    let inv = p.b("invert");
    let m: Vec<f32> = src
        .rgba
        .pixels
        .iter()
        .map(|q| {
            let (c, _) = straight(*q);
            let l = crate::image::linear_to_srgb((0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]).clamp(0.0, 1.0));
            let k = crate::math::smoothstep(lo, hi.max(lo + 1e-4), l);
            if inv { 1.0 - k } else { k }
        })
        .collect();
    Ok(matte_output(&src, &m, p.choice("output")))
}

fn copy_alpha(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let b = main_or_blank(ctx, inputs);
    let Some(a) = slot(inputs, 1) else { return Ok(b) };
    let op = p.choice("operation");
    let inv = p.b("invert");
    let rgba = b.rgba.map_xy(|x, y, q| {
        let mut m = a.rgba.get(x, y)[3].clamp(0.0, 1.0);
        if inv {
            m = 1.0 - m;
        }
        let (c, ba) = straight(q);
        let na = match op {
            1 => ba * m,
            2 => (ba + m).min(1.0),
            3 => (ba - m).max(0.0),
            _ => m,
        };
        // Pixels with no colour (alpha 0) stay black, as in a Copy node.
        premultiply(c, na)
    });
    Ok(b.with_rgba(rgba))
}

fn motion_vectors(ctx: &NodeCtx, inputs: &[Option<Arc<Plate>>], p: &ParamSet) -> Result<Plate, String> {
    let cur = main_or_blank(ctx, inputs);
    let prev = ctx.neighbours.iter().find(|(o, _)| *o == -1).and_then(|(_, at)| at.first().cloned().flatten());
    let mut out = cur.clone();
    out.motion = Some(match prev {
        Some(prev) => crate::flow::dense_flow(&prev.rgba, &cur.rgba, p.i("detail").max(1) as usize, p.f("smoothness")),
        None => vec![[0.0, 0.0]; ctx.width * ctx.height],
    });
    Ok(out)
}
