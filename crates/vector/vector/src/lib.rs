// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shape layers for Studio, the way shape layers are usually built: a shape
//! document is a list of items worked through in order.
//!
//! * Shapes: `rect` (`size`, `roundness`), `ellipse` / `circle` (`size`
//!   or `radius`), `polygon` (`points`, `radius`, `roundness`), `star`
//!   (`points`, `radius`, `inner`), `line` (`from`, `to`), `arc` and
//!   `pie` (`radius`, `inner`, `start`, `end` in degrees from straight
//!   up, clockwise), `path` (`points` + `closed` / `smooth`, or
//!   `vertices` with `in` / `out` handles, or SVG `d`), `curve` (from
//!   mathematics: presets such as rose, lissajous, heart, spirograph,
//!   superformula, koch, dragon; formulas x(u), y(u), polar r(theta),
//!   implicit f(x, y) = 0; L-systems; Fourier epicycles: see [`curve`]).
//!   Each takes a `position`.
//! * Paint: `fill` (`colour`, `opacity`, `rule`, `gradient`, `blend`),
//!   `stroke` (`colour`, `width`, `cap`, `join`, `miter`, `dash`,
//!   `dash_offset`, `gradient`, `opacity`, `blend`). A gradient is
//!   `{"type": "linear", "from", "to", "stops"}` or `{"type": "radial",
//!   "centre", "radius", "focal", "stops"}`; stops are colours, or
//!   `[at, colour]` pairs.
//! * Operators on the outlines so far: `merge` (`mode`: union,
//!   subtract, intersect, exclude), `trim` (`start`, `end`, `offset`,
//!   `mode`: together / sequence), `offset` (`amount`, `join`), `wiggle`
//!   (`size`, `detail`, `per_second`, `correlation`, `seed`), `zigzag`
//!   (`size`, `ridges`, `points`), `round_corners` (`radius`), `pucker`
//!   (`amount`, -1..1), `twist` (`angle`), `morph` (`to`: shapes,
//!   `amount`), `repeater` (`copies`, `offset`, `position`, `rotation`,
//!   `scale`, `anchor`, `start_opacity`, `end_opacity`, `composite`),
//!   `transform`.
//! * `group` (`items`, `position`, `anchor`, `scale`, `rotation`, `skew`,
//!   `opacity`, `blend`).
//!
//! Every number, point and colour can be animated (see [`value`]).
//! Coordinates are canvas pixels from the centre of the picture, y down.

pub mod curve;
pub mod draw;
pub mod extrude;
pub mod lottie;
pub mod ops;
pub mod path;
pub mod presets;
pub mod value;

pub use path::Path;
pub use value::{Ctx, Input};
/// A shape document (JSON).
pub use serde_json::Value as Document;

use serde_json::Value;

/// Reads a shape document.
pub fn parse(src: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(src).map_err(|e| format!("shape document is not JSON: {e}"))?;
    match &v {
        Value::Array(_) => Ok(v),
        Value::Object(o) if o.get("items").is_some_and(Value::is_array) => Ok(v),
        _ => Err("a shape document is {\"items\": [...]} or a list of items".into()),
    }
}

/// A drawn shape layer: premultiplied sRGB RGBA, 8 bits.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// What could not be drawn, in words.
    pub warnings: Vec<String>,
}

/// Draws `doc` at `width × height` pixels for a canvas `canvas` pixels in
/// size (the document's units).
pub fn render(doc: &Value, cx: &Ctx, width: u32, height: u32, canvas: [f64; 2]) -> Picture {
    let (scene, notes) = scene(doc, cx, width, height, canvas);
    let opts = studio_vector_raster::RenderOptions::new(width.max(1), height.max(1)).with_transform(root(width, height, canvas));
    let px = studio_vector_raster::render(&scene, &opts);
    Picture { width, height, rgba: px.data, warnings: notes.warnings }
}

/// Document units to pixels: the canvas centre to the picture centre.
pub fn root(width: u32, height: u32, canvas: [f64; 2]) -> studio_vector_raster::kurbo::Affine {
    let sx = width as f64 / canvas[0].max(1.0);
    let sy = height as f64 / canvas[1].max(1.0);
    studio_vector_raster::kurbo::Affine::new([sx, 0.0, 0.0, sy, width as f64 / 2.0, height as f64 / 2.0])
}

/// The document as a vector rasterizer scene in document units (for the
/// GPU renderer and anything else that draws scenes), with what it could
/// not do. `width`, `height` and `canvas` set how finely operators
/// flatten curves.
pub fn scene(doc: &Value, cx: &Ctx, width: u32, height: u32, canvas: [f64; 2]) -> (studio_vector_raster::Scene, draw::Notes) {
    let scale = (width as f64 / canvas[0].max(1.0)).max(height as f64 / canvas[1].max(1.0));
    draw::scene(doc, cx, scale)
}

/// The outlines `doc` ends with, in its own units.
pub fn outlines(doc: &Value, cx: &Ctx) -> Vec<Path> {
    draw::outlines(doc, cx)
}

/// What is wrong with a document, if anything: drawn once small at its
/// first moment.
pub fn check(doc: &Value) -> Vec<String> {
    render(doc, &Ctx::at(0.0, 1.0), 16, 9, [1920.0, 1080.0]).warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn px(p: &Picture, x: u32, y: u32) -> [u8; 4] {
        let i = (y * p.width + x) as usize * 4;
        [p.rgba[i], p.rgba[i + 1], p.rgba[i + 2], p.rgba[i + 3]]
    }

    fn draw(doc: Value, t: f64) -> Picture {
        render(&doc, &Ctx::at(t, 2.0), 192, 108, [1920.0, 1080.0])
    }

    #[test]
    fn a_filled_and_stroked_rectangle() {
        let p = draw(json!({"items": [{"type": "rect", "size": [600, 400]}, {"type": "fill", "colour": "#ff0000"}, {"type": "stroke", "colour": "#0000ff", "width": 40}]}), 0.0);
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        assert_eq!(px(&p, 96, 54), [255, 0, 0, 255]);
        // On the edge: the stroke, 30 px right of centre in the picture.
        let e = px(&p, 126, 54);
        assert!(e[2] > 200 && e[0] < 40, "{e:?}");
        assert_eq!(px(&p, 5, 5)[3], 0);
    }

    #[test]
    fn subtract_cuts_a_hole_and_repeater_makes_copies() {
        let p = draw(json!([{"type": "circle", "radius": 400}, {"type": "circle", "radius": 200}, {"type": "merge", "mode": "subtract"}, {"type": "fill", "colour": "white"}]), 0.0);
        assert_eq!(px(&p, 96, 54)[3], 0, "the hole");
        assert_eq!(px(&p, 96 + 30, 54)[3], 255, "the ring");
        let r = draw(json!([{"type": "rect", "size": [100, 100], "position": [-600, 0]}, {"type": "fill", "colour": "white"}, {"type": "repeater", "copies": 4, "position": [400, 0]}]), 0.0);
        for k in 0..4 {
            assert_eq!(px(&r, 36 + 40 * k, 54)[3], 255, "copy {k}");
        }
        assert_eq!(px(&r, 56, 54)[3], 0, "between copies");
    }

    #[test]
    fn trim_draws_on_over_time() {
        let doc = json!([{"type": "line", "from": [-800, 0], "to": [800, 0]}, {"type": "trim", "end": {"keys": [[0, 0], [1, 1, "linear"]]}}, {"type": "stroke", "colour": "white", "width": 20, "cap": "butt"}]);
        let early = draw(doc.clone(), 0.25);
        assert!(px(&early, 30, 54)[3] > 200 && px(&early, 120, 54)[3] == 0);
        let late = draw(doc, 1.0);
        assert!(px(&late, 170, 54)[3] > 200);
    }

    #[test]
    fn gradients_dashes_and_inputs() {
        let mut cx = Ctx::at(0.0, 1.0);
        cx.inputs.insert("tint".into(), Input::Colour([0.0, 1.0, 0.0, 1.0]));
        let doc = json!([{"type": "rect", "size": [1920, 1080]}, {"type": "fill", "gradient": {"type": "linear", "from": [-960, 0], "to": [960, 0], "stops": ["#000000", "$tint"]}}]);
        let p = render(&doc, &cx, 192, 108, [1920.0, 1080.0]);
        assert!(px(&p, 2, 54)[1] < 30 && px(&p, 189, 54)[1] > 220);
        let d = draw(json!([{"type": "line", "from": [-900, 0], "to": [900, 0]}, {"type": "stroke", "colour": "white", "width": 30, "dash": [100, 100], "cap": "butt"}]), 0.0);
        let on: Vec<bool> = (0..18).map(|k| px(&d, 5 + k * 10, 54)[3] > 128).collect();
        assert!(on.iter().any(|x| *x) && on.iter().any(|x| !*x), "{on:?}");
    }

    #[test]
    fn groups_draw_themselves_or_hand_on_their_outlines() {
        // A painted group keeps its colour under a later fill.
        let p = draw(json!([{"type": "group", "position": [-400, 0], "items": [{"type": "circle", "radius": 100}, {"type": "fill", "colour": "#ff0000"}]}, {"type": "rect", "size": [100, 100], "position": [400, 0]}, {"type": "fill", "colour": "#0000ff"}]), 0.0);
        assert_eq!(px(&p, 56, 54), [255, 0, 0, 255]);
        assert_eq!(px(&p, 136, 54), [0, 0, 255, 255]);
        // An unpainted group's outlines are painted by the parent.
        let q = draw(json!([{"type": "group", "position": [-400, 0], "items": [{"type": "circle", "radius": 100}]}, {"type": "fill", "colour": "#00ff00"}]), 0.0);
        assert_eq!(px(&q, 56, 54), [0, 255, 0, 255]);
        let w = check(&json!([{"type": "blob"}]));
        assert!(w[0].contains("blob"));
        assert_eq!(outlines(&json!([{"type": "rect", "size": [10, 10]}]), &Ctx::default()).len(), 1);
    }

    #[test]
    fn pies_and_rings() {
        // A quarter wedge from 12 to 3 o'clock.
        let p = draw(json!([{"type": "pie", "radius": 400, "start": 0, "end": 90}, {"type": "fill", "colour": "white"}]), 0.0);
        assert_eq!(px(&p, 96 + 15, 54 - 15)[3], 255);
        assert_eq!(px(&p, 96 - 15, 54 - 15)[3], 0);
        let r = draw(json!([{"type": "pie", "radius": 400, "inner": 300, "start": 0, "end": 360}, {"type": "fill", "colour": "white"}]), 0.0);
        assert_eq!(px(&r, 96, 54)[3], 0);
        assert_eq!(px(&r, 96 + 35, 54)[3], 255);
    }

    #[test]
    fn the_ring_preset_fills_to_its_input_and_ring_slices_close() {
        let o = presets::Options { colour: "#ff0000".into(), accent: "#0000ff".into(), size: 600.0, width: 60.0, speed: 1.0 };
        let doc = presets::make("ring", &o).unwrap();
        let mut cx = Ctx::at(0.0, 4.0);
        cx.inputs.insert("progress".into(), Input::Num(0.25));
        let p = render(&doc, &cx, 192, 108, [1920.0, 1080.0]);
        // A quarter: the arc is at 1:30, the track alone at 9 o'clock.
        let arc = px(&p, 96 + 21, 54 - 21);
        assert!(arc[0] > 150 && arc[2] < 60, "{arc:?}");
        let track = px(&p, 96 - 30, 54);
        assert!(track[0] < 40 && track[2] > 20, "{track:?}");
        // A ring slice from 12 to 4 o'clock.
        let d = draw(json!([{"type": "pie", "radius": 400, "inner": 200, "start": 0, "end": 120}, {"type": "fill", "colour": "white"}]), 0.0);
        assert_eq!(px(&d, 96 + 26, 54 - 15)[3], 255, "inside the slice");
        assert_eq!(px(&d, 96 - 10, 54 + 28)[3], 0, "past its end");
        assert_eq!(px(&d, 96, 54)[3], 0, "the hole");
    }
}
