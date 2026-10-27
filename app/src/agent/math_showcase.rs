// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The math showcase, built the way an agent builds it: JSON-RPC messages
//! through the MCP server (`mcp::handle`), the same path `zornov mcp`
//! serves. A gallery of shapes from mathematics, lit and moving, then the
//! same shapes meshed and drawn directly side by side, with what each
//! costs. Slow (it renders), so it runs when asked:
//!
//! ```text
//! MATH_SHOWCASE_OUT=/some/folder cargo test -p studio-app --release math_showcase -- --ignored --nocapture
//! ```
//!
//! The pictures, the replies and the timings go to that folder, and the
//! project is saved in it (`Math showcase`).

use super::Session;
use crate::services;
use serde_json::{Value, json};
use std::time::Instant;

struct Driver {
    s: Session,
    n: u64,
    out: std::path::PathBuf,
    log: String,
}

impl Driver {
    /// One `tools/call`; its text, with any pictures saved as `tag_k.jpg`.
    fn call(&mut self, name: &str, mut args: Value, tag: &str) -> (String, f64) {
        // A gallery floats: its models stay where they are put instead of
        // coming to rest on what is under them.
        for op in args.get_mut("ops").and_then(Value::as_array_mut).into_iter().flatten() {
            if op["op"] == "model" && op.get("rest").is_none() {
                op["rest"] = json!(false);
            }
        }
        self.n += 1;
        let msg = json!({"jsonrpc": "2.0", "id": self.n, "method": "tools/call", "params": {"name": name, "arguments": args}});
        let t0 = Instant::now();
        let r = super::mcp::handle(&mut self.s, &msg).expect("a reply");
        let took = t0.elapsed().as_secs_f64();
        let res = &r["result"];
        let mut text = Vec::new();
        for (k, c) in res["content"].as_array().into_iter().flatten().enumerate() {
            match c["type"].as_str() {
                Some("text") => text.push(c["text"].as_str().unwrap_or("").to_owned()),
                Some("image") => {
                    let f = self.out.join(format!("{tag}_{k}.jpg"));
                    let bytes = super::mcp::unbase64(c["data"].as_str().unwrap_or("")).expect("base64");
                    std::fs::write(&f, bytes).expect("write picture");
                    text.push(format!("[picture {}]", f.display()));
                }
                _ => {}
            }
        }
        let said = text.join("\n");
        let err = res["isError"].as_bool().unwrap_or(false) || r.get("error").is_some();
        self.log.push_str(&format!("=== {name} {tag} ({took:.2} s){}\n{}\n--> {}\n\n", if err { " ERROR" } else { "" }, args, said.chars().take(4000).collect::<String>()));
        println!("{name} {tag}: {took:.2} s{} {}", if err { " ERROR" } else { "" }, said.chars().take(240).collect::<String>());
        assert!(!err, "{name} {tag}: {said} {r}");
        (said, took)
    }
}

#[test]
#[ignore]
fn math_showcase() {
    let out = std::env::var_os("MATH_SHOWCASE_OUT").map(std::path::PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("zornov-math-showcase"));
    std::fs::create_dir_all(&out).unwrap();
    let mut d = Driver { s: Session::new(services::project::new_custom("Math showcase", 1280, 720, 30.0)), n: 0, out: out.clone(), log: String::new() };
    let init = super::mcp::handle(&mut d.s, &json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "showcase", "version": "1"}}})).unwrap();
    assert!(init["result"]["serverInfo"].is_object());
    let (skill, _) = d.call("skill", json!({"name": "gyroid", "part": "math shapes"}), "skill");
    assert!(skill.contains("Shape functions in formulas"), "{skill}");
    let (schema, _) = d.call("edit_schema", json!({"ops": [{"op": "model", "math": "gyroid", "render": "direct"}, {"op": "animate", "clip": 1, "property": "position", "curve": "rose"}, {"op": "mesh", "clip": 1, "do": "twist", "degrees": 90}]}), "schema");
    assert!(!schema.contains("\"ok\":false"), "{schema}");

    // 0..6 s: the gallery.
    let g = 6.0;
    let spin = |k: f64| json!({"op": "animate", "clip": "$last", "property": "rotation_y", "formula": format!("{} * t + {}", 30.0 + 8.0 * k, k * 40.0)});
    let tilt = json!({"op": "animate", "clip": "$last", "property": "rotation_x", "formula": "15 * sin(t * tau / 6)"});
    d.call("edit", json!({"ops": [{"op": "card", "colour": "#0d1017", "gradient": ["#1c2537", "#07080c"], "radial": true, "at": 0, "duration": 12}]}), "backdrop");
    let gallery = [
        json!({"op": "model", "name": "Gyroid lattice", "math": {"preset": "gyroid", "cells": 2.5, "thickness": 0.28}, "size": 230, "detail": 4, "position": [-400, -150, 0], "colour": "#e0b44c", "metallic": 0.5, "roughness": 0.35, "duration": g}),
        json!({"op": "model", "name": "Swirl torus", "math": "swirl_torus", "size": 250, "detail": 4, "position": [0, -150, 0], "colour": "#3a86ff", "roughness": 0.35, "duration": g}),
        json!({"op": "model", "name": "Mandelbulb", "math": {"preset": "mandelbulb", "power": 8}, "render": "direct", "size": 240, "position": [400, -150, 0], "colour": "#c2c8d6", "roughness": 0.5, "duration": g}),
        json!({"op": "model", "name": "Lattice sphere", "math": {"preset": "sphere", "cut": "fract(u * 18) < 0.55 || fract(v * 10) < 0.5", "thickness": 4}, "size": 220, "detail": 4, "position": [-400, 170, 0], "colour": "#ff6b6b", "roughness": 0.4, "duration": g}),
        json!({"op": "model", "name": "Torus knot", "math": {"preset": "torus_knot", "p": 3, "q": 7, "radius": 0.11}, "size": 240, "detail": 4, "position": [0, 170, 0], "colour": "#2ec4b6", "metallic": 0.6, "roughness": 0.25, "duration": g}),
        json!({"op": "model", "name": "Rippling sphere", "math": {"preset": "ripple_sphere", "waves": 7, "amount": 0.07, "speed": 3}, "size": 220, "detail": 3, "position": [400, 170, 0], "colour": "#b388ff", "roughness": 0.3, "duration": g}),
    ];
    for (k, op) in gallery.into_iter().enumerate() {
        let tag = op["name"].as_str().unwrap().replace(' ', "_");
        d.call("edit", json!({"ops": [op, spin(k as f64), tilt]}), &tag);
    }
    d.call("edit", json!({"ops": [{"op": "model", "name": "Seashell", "math": {"preset": "seashell", "turns": 3}, "size": 150, "detail": 3, "position": [-190, 10, -60], "colour": "#f4e1c1", "roughness": 0.45, "duration": g}, spin(6.0)]}), "seashell");
    d.call("edit", json!({"ops": [{"op": "model", "name": "Twisted column", "shape": "cube", "size": [50, 200, 50], "detail": 2, "position": [200, 10, -60], "colour": "#9ad0ec", "duration": g}, {"op": "mesh", "clip": "$last", "steps": [{"do": "twist", "degrees": 220}, {"do": "taper", "from": 0.6, "to": 1.1}, {"do": "bend", "degrees": 25}]}, spin(7.0)]}), "column");
    d.call("edit", json!({"ops": [
        {"op": "shape", "name": "Rose", "items": [{"type": "curve", "preset": "rose", "n": {"keys": [[0, 3], [6, 7]]}, "d": 4, "size": 110}, {"type": "stroke", "colour": "#ffd166", "width": 2}], "at": 0, "duration": g, "x": -570, "y": 300},
        {"op": "shape", "name": "Epicycles", "items": [{"type": "curve", "fourier": {"of": "heart", "terms": 12, "period": 6}, "size": 110}, {"type": "stroke", "colour": "#e0e6f0", "width": 1.5}], "at": 0, "duration": g, "x": 570, "y": 300},
    ]}), "curves");
    d.call("edit", json!({"ops": [{"op": "model", "name": "Comet", "shape": "sphere", "size": 24, "detail": 2, "colour": "#ffffff", "emissive": "#ffe6a8", "emissive_strength": 4, "duration": g, "position": [0, 10, -200]}, {"op": "animate", "clip": "$last", "property": "position", "curve": {"preset": "lissajous", "a": 3, "b": 2, "size": 900}}]}), "comet");

    // 6..10 s: meshed (left) and drawn from the formula (right), side by side.
    let mut cost = Vec::new();
    for (row, (name, math, colour)) in [("Mandelbulb", json!({"preset": "mandelbulb", "power": 8, "iterations": 10}), "#c9ced9"), ("Gyroid", json!({"preset": "gyroid", "cells": 3, "thickness": 0.22}), "#e0b44c")].into_iter().enumerate() {
        let y = if row == 0 { -150 } else { 170 };
        let (said, t_mesh) = d.call("edit", json!({"ops": [{"op": "model", "name": format!("{name} mesh"), "math": math, "render": "mesh", "size": 280, "detail": 5, "position": [-240, y, 0], "colour": colour, "roughness": 0.4, "at": g, "duration": 4}, {"op": "animate", "clip": "$last", "property": "rotation_y", "formula": "20 * t"}]}), &format!("{name}_mesh"));
        let tris = said.split("\\\"triangles\\\":").nth(1).and_then(|r| r.split(',').next()).unwrap_or("?").to_owned();
        let (_, t_direct) = d.call("edit", json!({"ops": [{"op": "model", "name": format!("{name} direct"), "math": math, "render": "direct", "size": 280, "position": [240, y, 0], "colour": colour, "roughness": 0.4, "at": g, "duration": 4}, {"op": "animate", "clip": "$last", "property": "rotation_y", "formula": "20 * t"}]}), &format!("{name}_direct"));
        cost.push((name, tris, t_mesh, t_direct));
    }
    d.call("edit", json!({"ops": [
        {"op": "text", "text": "mesh: surface nets, detail 5", "at": g, "duration": 4, "x": -240, "y": -320, "size": 40, "colour": "white"},
        {"op": "text", "text": "direct: sphere traced from the formula", "at": g, "duration": 4, "x": 240, "y": -320, "size": 40, "colour": "white"},
    ]}), "labels");
    // 10..11 s the mesh alone, 11..12 s the traced one alone: what a frame of each costs.
    let lone = json!({"preset": "mandelbulb", "power": 8, "iterations": 10});
    d.call("edit", json!({"ops": [{"op": "model", "name": "Lone mesh", "math": lone, "render": "mesh", "size": 460, "detail": 5, "position": [0, 0, 0], "colour": "#c9ced9", "at": 10, "duration": 1}]}), "lone_mesh");
    d.call("edit", json!({"ops": [{"op": "model", "name": "Lone direct", "math": lone, "render": "direct", "size": 460, "position": [0, 0, 0], "colour": "#c9ced9", "at": 11, "duration": 1}]}), "lone_direct");
    d.call("edit", json!({"ops": [{"op": "lighting", "preset": "three_point"}, {"op": "camera", "focal_length": 40, "move": {"type": "dolly", "by": 80, "duration": 12, "ease": "ease_in_out"}}]}), "light_camera");

    // Look, as an agent would.
    d.call("contact_sheet", json!({"start": 0, "end": 5.9, "count": 6, "detail": "full"}), "gallery_sheet");
    d.call("frame", json!({"t": 2.5, "detail": "full"}), "gallery_frame");
    d.call("frame", json!({"t": 8.0, "detail": "full"}), "compare_frame");
    let (_, f_mesh) = d.call("frame", json!({"t": 10.5, "detail": "full"}), "lone_mesh_frame");
    let (_, f_direct) = d.call("frame", json!({"t": 11.5, "detail": "full"}), "lone_direct_frame");
    let stats = studio_render::direct::last_stats();
    d.call("see", json!({}), "see");
    d.call("check", json!({}), "check");
    let project = out.join("Math showcase");
    d.call("save", json!({"path": project.display().to_string()}), "save");

    let mut table = String::from("shape | mesh build (s) | triangles | direct build (s)\n");
    for (name, tris, a, b) in &cost {
        table.push_str(&format!("{name} | {a:.2} | {tris} | {b:.2}\n"));
    }
    table.push_str(&format!("\nA 1280x720 frame of one Mandelbulb, 460 px: mesh {f_mesh:.2} s, direct {f_direct:.2} s (last trace: {} px marched, {} hit, {}, {:.0} ms)\n", stats.pixels, stats.hits, if stats.gpu { "GPU" } else { "CPU" }, stats.millis));
    println!("{table}");
    std::fs::write(out.join("timings.txt"), &table).unwrap();
    std::fs::write(out.join("log.txt"), &d.log).unwrap();
}
