// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The GPU side of the node compositor: which nodes have WGSL kernels,
//! the complete shader modules, and their uniform packing. The renderer
//! runs these and falls back to the CPU code for every other node (and
//! for nodes whose settings a kernel does not cover, such as transform
//! motion blur or blurs too wide for one pass).

use super::nodes::{self, transform_matrix, track_at, track_samples};
use super::{Node, geo};
use crate::params::ParamSet;

macro_rules! comp_shader {
    ($($name:ident = $($part:expr),+;)*) => {
        $(
            pub const $name: &str = concat!(include_str!("wgsl/comp_common.wgsl"), $("\n", $part,)+);
        )*
    };
}

comp_shader! {
    MERGE = include_str!("wgsl/merge.wgsl");
    GRADE = include_str!("wgsl/grade.wgsl");
    INVERT = "const OP: i32 = 0;", include_str!("wgsl/simple.wgsl");
    CLAMP = "const OP: i32 = 1;", include_str!("wgsl/simple.wgsl");
    PREMULT = "const OP: i32 = 2;", include_str!("wgsl/simple.wgsl");
    UNPREMULT = "const OP: i32 = 3;", include_str!("wgsl/simple.wgsl");
    CROP = "const OP: i32 = 4;", include_str!("wgsl/simple.wgsl");
    CONSTANT = "const OP: i32 = 5;", include_str!("wgsl/simple.wgsl");
    RAMP = "const OP: i32 = 6;", include_str!("wgsl/simple.wgsl");
    WARP = include_str!("wgsl/warp.wgsl");
    LENS = include_str!("wgsl/lens.wgsl");
    ZDEFOCUS = include_str!("wgsl/zdefocus.wgsl");
    VECTOR_BLUR = include_str!("wgsl/vector_blur.wgsl");
    SHUFFLE = include_str!("wgsl/shuffle.wgsl");
    ID_MATTE = "const OP: i32 = 0;", include_str!("wgsl/mattes.wgsl");
    DEPTH_MATTE = "const OP: i32 = 1;", include_str!("wgsl/mattes.wgsl");
    LUMA_MATTE = "const OP: i32 = 2;", include_str!("wgsl/mattes.wgsl");
    COPY_ALPHA = "const OP: i32 = 3;", include_str!("wgsl/mattes.wgsl");
    DESPILL = "const OP: i32 = 4;", include_str!("wgsl/mattes.wgsl");
    BLUR = include_str!("wgsl/blur.wgsl");
}

/// Every shader module by name (for validation).
pub fn all_shaders() -> Vec<(&'static str, &'static str)> {
    vec![
        ("merge", MERGE),
        ("grade", GRADE),
        ("invert", INVERT),
        ("clamp", CLAMP),
        ("premult", PREMULT),
        ("unpremult", UNPREMULT),
        ("crop", CROP),
        ("constant", CONSTANT),
        ("ramp", RAMP),
        ("warp", WARP),
        ("lens", LENS),
        ("zdefocus", ZDEFOCUS),
        ("vector_blur", VECTOR_BLUR),
        ("shuffle", SHUFFLE),
        ("id_matte", ID_MATTE),
        ("depth_matte", DEPTH_MATTE),
        ("luma_matte", LUMA_MATTE),
        ("copy_alpha", COPY_ALPHA),
        ("despill", DESPILL),
        ("blur", BLUR),
    ]
}

/// A node's GPU work: the shader and its packed parameters. Blur is two
/// passes (see [`blur_passes`]); every other kernel is one.
pub struct Kernel {
    pub name: &'static str,
    pub wgsl: &'static str,
    pub params: Vec<[f32; 4]>,
}

fn mat_rows(m: &geo::Mat3, extra: f32) -> Vec<[f32; 4]> {
    vec![
        [m[0][0] as f32, m[0][1] as f32, m[0][2] as f32, 0.0],
        [m[1][0] as f32, m[1][1] as f32, m[1][2] as f32, 0.0],
        [m[2][0] as f32, m[2][1] as f32, m[2][2] as f32, 0.0],
        [extra, 0.0, 0.0, 0.0],
    ]
}

/// The kernel for node `n` with values `p` on a `w`×`h` comp at `time`,
/// or `None` when it runs on the CPU.
pub fn kernel(n: &Node, p: &ParamSet, w: usize, h: usize, time: f64, fps: f64) -> Option<Kernel> {
    if n.disabled {
        return None;
    }
    let k = |name: &'static str, wgsl: &'static str, params: Vec<[f32; 4]>| Some(Kernel { name, wgsl, params });
    let def = n.def()?;
    if !(def.offsets)(n).is_empty() {
        return None;
    }
    match n.kind.as_str() {
        "merge" => k("merge", MERGE, p.pack_vec4()),
        "grade" => k("grade", GRADE, p.pack_vec4()),
        "invert" => k("invert", INVERT, p.pack_vec4()),
        "clamp" => k("clamp", CLAMP, p.pack_vec4()),
        "premult" => k("premult", PREMULT, p.pack_vec4()),
        "unpremult" => k("unpremult", UNPREMULT, p.pack_vec4()),
        "crop" => k("crop", CROP, p.pack_vec4()),
        "constant" => k("constant", CONSTANT, p.pack_vec4()),
        "ramp" => k("ramp", RAMP, p.pack_vec4()),
        "transform" => {
            let moving = n.keys.values().any(|k| k.len() > 1);
            if p.f("motion_blur") > 0.0 && moving {
                return None;
            }
            k("warp", WARP, mat_rows(&transform_matrix(p, w, h), p.choice("edge") as f32))
        }
        "corner_pin" => {
            let (wf, hf) = (w as f64, h as f64);
            let from = [[0.0, 0.0], [wf, 0.0], [wf, hf], [0.0, hf]];
            let to = ["to1", "to2", "to3", "to4"].map(|id| {
                let q = p.point(id);
                [q[0] as f64 * wf, q[1] as f64 * hf]
            });
            let m = if p.b("invert") { geo::homography(&from, &to) } else { geo::homography(&to, &from) }?;
            k("warp", WARP, mat_rows(&m, 0.0))
        }
        "track" => {
            let samples = track_samples(&n.data);
            let cur = track_at(&samples, time * fps)?;
            let reference = n.data.get("reference").and_then(|v| v.as_f64()).unwrap_or(samples[0].frame);
            let r = track_at(&samples, reference).unwrap_or(cur);
            let (dx, dy) = if p.b("translate") { (cur.x - r.x, cur.y - r.y) } else { (0.0, 0.0) };
            let rot = if p.b("rotate") { (cur.rotation - r.rotation).to_radians() } else { 0.0 };
            let kk = if p.b("scale") { cur.scale / r.scale.max(1e-6) } else { 1.0 };
            let (s, c) = rot.sin_cos();
            let lin = [[c * kk, -s * kk, 0.0], [s * kk, c * kk, 0.0], [0.0, 0.0, 1.0]];
            let fwd = geo::mul3(&[[1.0, 0.0, r.x + dx], [0.0, 1.0, r.y + dy], [0.0, 0.0, 1.0]], &geo::mul3(&lin, &[[1.0, 0.0, -r.x], [0.0, 1.0, -r.y], [0.0, 0.0, 1.0]]));
            let m = if p.choice("mode") == 1 { fwd } else { geo::inverse3(&fwd).unwrap_or(geo::IDENTITY) };
            k("warp", WARP, mat_rows(&m, p.choice("edge") as f32))
        }
        "lens_distort" => k("lens", LENS, p.pack_vec4()),
        "zdefocus" => k("zdefocus", ZDEFOCUS, p.pack_vec4()),
        "vector_blur" => k("vector_blur", VECTOR_BLUR, p.pack_vec4()),
        "shuffle" => k("shuffle", SHUFFLE, p.pack_vec4()),
        "id_matte" => k("id_matte", ID_MATTE, p.pack_vec4()),
        "depth_matte" => k("depth_matte", DEPTH_MATTE, p.pack_vec4()),
        "luma_matte" => k("luma_matte", LUMA_MATTE, p.pack_vec4()),
        "copy_alpha" => k("copy_alpha", COPY_ALPHA, p.pack_vec4()),
        "despill" => k("despill", DESPILL, p.pack_vec4()),
        "dot" => k("unpremult_noop", CLAMP, vec![[-1.0e9, 0.0, 0.0, 0.0], [1.0e9, 0.0, 0.0, 0.0]]).filter(|_| false),
        _ => None,
    }
}

/// The two passes of a blur node (horizontal then vertical), or `None`
/// when it is too wide for the GPU kernel.
pub fn blur_passes(p: &ParamSet) -> Option<[Kernel; 2]> {
    let sx = p.f("size");
    let sy = sx * p.f("aspect");
    // The CPU blurs wider than 6 px with three box passes; keep the GPU to
    // the exact range so both agree.
    if sx > 6.0 || sy > 6.0 {
        return None;
    }
    let ch = p.choice("channels") as f32;
    Some([
        Kernel { name: "blur", wgsl: BLUR, params: vec![[sx, 0.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0], [ch, 0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 0.0]] },
        Kernel { name: "blur", wgsl: BLUR, params: vec![[sy, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [ch, 0.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0]] },
    ])
}

/// Which extra channels a GPU kernel's output carries, from its inputs'.
/// Bits: 1 depth, 2 motion, 4 id; `main` and `second` are the inputs'.
pub fn out_channels(n: &Node, p: &ParamSet, main: u32, second: u32) -> u32 {
    match n.kind.as_str() {
        "merge" => main | second,
        "shuffle" => {
            let mut o = main;
            o = match p.choice("depth") {
                1 => (o & !1) | (second & 1),
                2 | 3 => o | 1,
                4 => o & !1,
                _ => o,
            };
            o = match p.choice("motion") {
                1 => (o & !2) | (second & 2),
                2 => o & !2,
                _ => o,
            };
            match p.choice("id") {
                1 => (o & !4) | (second & 4),
                2 => o & !4,
                _ => o,
            }
        }
        "constant" | "ramp" => 0,
        _ => main,
    }
}

/// Flags for the shader's `has_aux` uniform from channel bits.
pub fn aux_flags(main: u32, second: u32) -> u32 {
    let mut f = 0;
    if main != 0 {
        f |= 1;
    }
    if second != 0 {
        f |= 2;
    }
    f | ((main & 7) << 2) | ((second & 7) << 5)
}

#[allow(dead_code)]
fn _uses(_: &nodes::NodeDef) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_comp_shader_validates() {
        for (name, src) in all_shaders() {
            let module = naga::front::wgsl::parse_str(src).unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(src)));
            naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
                .validate(&module)
                .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        }
    }
}
