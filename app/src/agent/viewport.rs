// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `viewport`: the 3D workspace for the AI, used as a person uses it. The
//! same workspace the Player shows (`editor::viewport`), so the person
//! sees each move: read the editor's eye and what is in the world and
//! where it sits on screen; orbit, pan, dolly, fly, walk, look along an
//! axis, go orthographic, frame things; look through any camera, set a
//! camera to the view, lock it, key a fly-through from views; save and go
//! to views; pick and select what is under a point; move, turn and scale
//! objects in world, view or own axes with snapping and pivots; hide and
//! lock; shading, cards and spread; and see the view as a picture with
//! the grid, cameras, lights and outlines drawn in.

use super::{Content, Session, f, micros, secs};
use crate::editor::viewport::{self as vp, Look, Shade, nav::Axis, nav::Nav};
use crate::view::Time;
use serde_json::{Value, json};
use std::collections::HashMap;
use studio_model as m;
use studio_model::space as sp;

fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn v3(a: Option<&Value>) -> Option<[f64; 3]> {
    let a = a?.as_array()?;
    if a.len() != 3 {
        return None;
    }
    Some([a[0].as_f64()?, a[1].as_f64()?, a[2].as_f64()?])
}

fn nav_json(n: &Nav) -> Value {
    let p = n.position();
    json!({
        "position": p.map(r1), "target": n.target.map(r1), "yaw": r1(n.yaw), "pitch": r1(n.pitch),
        "distance": r1(n.distance), "lens_mm": r1(n.lens), "orthographic": n.ortho,
    })
}

fn clip_ids(a: &Value, k: &str) -> Vec<u64> {
    match a.get(k) {
        Some(Value::Array(v)) => v.iter().filter_map(Value::as_u64).collect(),
        Some(v) => v.as_u64().into_iter().collect(),
        None => Vec::new(),
    }
}

/// A line drawn into a picture (alpha blended).
fn line(img: &mut studio_media::rgba::RgbaImage, a: [f64; 2], b: [f64; 2], c: [u8; 4]) {
    let (w, h) = (img.width as i64, img.height as i64);
    let n = ((b[0] - a[0]).abs().max((b[1] - a[1]).abs()).ceil() as i64).clamp(1, 20_000);
    for i in 0..=n {
        let t = i as f64 / n as f64;
        let x = (a[0] + (b[0] - a[0]) * t).round() as i64;
        let y = (a[1] + (b[1] - a[1]) * t).round() as i64;
        if x < 0 || y < 0 || x >= w || y >= h {
            continue;
        }
        let o = ((y * w + x) * 4) as usize;
        let al = c[3] as u32;
        for k in 0..3 {
            img.pixels[o + k] = ((img.pixels[o + k] as u32 * (255 - al) + c[k] as u32 * al) / 255) as u8;
        }
    }
}

impl Session {
    /// Time the viewport shows: `t`, else the start.
    fn vp_time(&self, a: &Value) -> Time {
        micros(f(a, "t").or_else(|| f(a, "at")).unwrap_or(0.0))
    }

    fn vp_depths(&mut self, t: Time) -> HashMap<u64, f32> {
        let (spread, hidden) = vp::with(|w| (w.spread as f32, w.hidden.clone()));
        let scene = self.frames.model_scene(&self.doc);
        let canvas = studio_render::scene::Scene::canvas(scene.as_ref());
        let mut o = studio_render::viewport::ViewOptions::new(studio_render::space::CameraDesc::default_for(&canvas));
        o.spread = spread;
        o.hidden = hidden.into_iter().collect();
        studio_render::viewport::ViewScene::new(scene, o).card_depths(t)
    }

    fn vp_objects(&mut self, t: Time) -> Vec<vp::Object> {
        let ws = vp::snapshot();
        let depths = self.vp_depths(t);
        vp::objects(&self.doc, t, ws.cards.then_some(&depths), &ws.hidden)
    }

    /// The view the workspace shows now: the eye, or the camera looked
    /// through.
    fn vp_view(&self, t: Time) -> (sp::View, Option<u64>) {
        let ws = vp::snapshot();
        let w = self.doc.project.width as f64;
        if let Look::Camera(_) = ws.look
            && let Some(id) = vp::look_camera(&self.doc, ws.look, t)
            && let Some(mc) = self.doc.model.active().clip(m::ClipId(id))
        {
            return (sp::camera_view(self.doc.model.active(), mc, t, w), Some(id));
        }
        (ws.nav.view(w), None)
    }

    fn vp_state(&mut self, t: Time) -> Value {
        let ws = vp::snapshot();
        let objs = self.vp_objects(t);
        let (view, cam) = self.vp_view(t);
        let (w, h) = (self.doc.project.width as f64, self.doc.project.height as f64);
        let things: Vec<Value> = objs
            .iter()
            .map(|o| {
                let screen = view.project(o.anchor).map(|(c, d)| (c.x / w + 0.5, c.y / h + 0.5, d));
                let size: [f64; 3] = std::array::from_fn(|i| r1(o.max[i] - o.min[i]));
                let mut v = json!({"id": o.id, "name": o.name, "kind": o.kind, "at": o.anchor.map(r1), "size": size});
                if let Some((x, y, d)) = screen {
                    v["on_screen"] = json!({"x": (x * 1000.0).round() / 1000.0, "y": (y * 1000.0).round() / 1000.0, "depth": r1(d), "in_view": (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y)});
                } else {
                    v["on_screen"] = json!("behind the view");
                }
                if ws.locked.contains(&o.id) {
                    v["locked"] = json!(true);
                }
                v
            })
            .collect();
        json!({
            "on": ws.on,
            "looking_through": match cam { Some(id) => json!({"camera": id}), None => json!("editor view") },
            "lock_camera_to_view": ws.lock_camera,
            "eye": nav_json(&ws.nav),
            "quad": ws.quad,
            "shading": ws.shade.name(),
            "cards": ws.cards, "spread": ws.spread,
            "tool": format!("{:?}", ws.tool).to_lowercase(), "orient": format!("{:?}", ws.orient).to_lowercase(), "pivot": format!("{:?}", ws.pivot).to_lowercase(),
            "snap": {"on": ws.snap.on, "step": ws.snap.step, "angle": ws.snap.angle, "scale": ws.snap.scale},
            "cursor": ws.cursor.map(r1),
            "floor_y": r1(ws.floor_y(h)),
            "bookmarks": ws.bookmarks.iter().map(|b| b.name.clone()).collect::<Vec<_>>(),
            "hidden": ws.hidden, "locked": ws.locked,
            "t": secs(t),
            "objects": things,
            "units": "canvas pixels from the canvas centre; x right, y down (negative y is up), z away from the default camera; yaw 0 looks into the canvas, positive turns right; pitch positive looks down",
        })
    }

    /// Changes the eye (or, through a camera locked to the view, the
    /// camera).
    fn vp_nav(&mut self, t: Time, f: impl FnOnce(&mut Nav)) -> Result<(), String> {
        let (look, lock) = vp::with(|w| (w.look, w.lock_camera));
        if let (Look::Camera(_), true) = (look, lock)
            && let Some(id) = vp::look_camera(&self.doc, look, t)
        {
            let mut n = vp::camera_nav(&self.doc, id, t).ok_or("no camera")?;
            f(&mut n);
            n.ortho = false;
            return vp::set_camera_to_view(&mut self.doc, id, &n, t, None);
        }
        if let Look::Camera(_) = look
            && let Some(id) = vp::look_camera(&self.doc, look, t)
            && let Some(n) = vp::camera_nav(&self.doc, id, t)
        {
            vp::with(|w| w.nav = n);
        }
        vp::with(|w| {
            w.look = Look::Editor;
            w.on = true;
            f(&mut w.nav);
            w.changed();
        });
        Ok(())
    }

    /// The workspace view as a picture: the render from the eye (or
    /// camera) over the floor grid, with frustums, lights and outlines.
    fn vp_picture(&mut self, t: Time, width: u32) -> Result<studio_media::rgba::RgbaImage, String> {
        let ws = vp::snapshot();
        let (view, cam) = self.vp_view(t);
        let (cw, ch) = (self.doc.project.width as f64, self.doc.project.height as f64);
        let mut o = studio_render::viewport::ViewOptions::new(studio_render::space::CameraDesc { position: view.position.map(|x| x as f32), axes: view.axes.map(|r| r.map(|x| x as f32)), zoom: view.zoom as f32, dof: None, trace: None });
        o.shading = if cam.is_some() { Shade::Rendered.render() } else { ws.shade.render() };
        o.cards = ws.cards && cam.is_none();
        o.spread = ws.spread as f32;
        o.hidden = ws.hidden.iter().copied().collect();
        o.transparent = cam.is_none();
        let pic = if ws.shade == Shade::Wireframe && cam.is_none() {
            let w = width.min(cw as u32).max(16);
            let h = ((w as f64 * ch / cw).round() as u32).max(16);
            studio_media::rgba::RgbaImage { width: w, height: h, pixels: vec![0; (w * h * 4) as usize] }
        } else {
            self.frames.rgba_through(&self.doc, o, t, width)?
        };
        let (w, h) = (pic.width, pic.height);
        let k = w as f64 / cw;
        // The background (with the grid drawn on it below), the picture
        // over that, then the overlays.
        let mut img = studio_media::rgba::RgbaImage { width: w, height: h, pixels: [40u8, 40, 40, 255].repeat((w * h) as usize) };
        let to = |p: [f64; 3]| -> Option<[f64; 2]> { view.project(p).map(|(c, _)| [w as f64 * 0.5 + c.x * k, h as f64 * 0.5 + c.y * k]) };
        let seg = |img: &mut studio_media::rgba::RgbaImage, a: [f64; 3], b: [f64; 3], c: [u8; 4]| {
            let (va, vb) = (view.view(a), view.view(b));
            let near = 2.0;
            if va[2] < near && vb[2] < near {
                return;
            }
            let cut = |p: [f64; 3], q: [f64; 3], vp: [f64; 3], vq: [f64; 3]| {
                let s = (near - vp[2]) / (vq[2] - vp[2]);
                [p[0] + (q[0] - p[0]) * s, p[1] + (q[1] - p[1]) * s, p[2] + (q[2] - p[2]) * s]
            };
            let a2 = if va[2] < near { cut(a, b, va, vb) } else { a };
            let b2 = if vb[2] < near { cut(b, a, vb, va) } else { b };
            if let (Some(x), Some(y)) = (to(a2), to(b2)) {
                line(img, x, y, c);
            }
        };
        if cam.is_none() {
            if ws.grid {
                // A floor grid round the eye's target, behind the world.
                let step = 10f64.powf((ws.nav.world_per_pixel(cw) * 40.0 / k.max(1e-6) * k).max(1.0).log10().ceil()).max(10.0);
                let floor = ws.floor_y(ch);
                let (cx, cz) = ((ws.nav.target[0] / step).round() * step, (ws.nav.target[2] / step).round() * step);
                for i in -20..=20 {
                    let o = i as f64 * step;
                    let c = if i == 0 { [120, 160, 230, 150] } else { [255, 255, 255, 28] };
                    seg(&mut img, [cx + o, floor, cz - 20.0 * step], [cx + o, floor, cz + 20.0 * step], c);
                    let c = if i == 0 { [230, 90, 90, 150] } else { [255, 255, 255, 28] };
                    seg(&mut img, [cx - 20.0 * step, floor, cz + o], [cx + 20.0 * step, floor, cz + o], c);
                }
            }
            if ws.frame {
                let c = [[-cw / 2.0, -ch / 2.0, 0.0], [cw / 2.0, -ch / 2.0, 0.0], [cw / 2.0, ch / 2.0, 0.0], [-cw / 2.0, ch / 2.0, 0.0]];
                for i in 0..4 {
                    seg(&mut img, c[i], c[(i + 1) % 4], [255, 255, 255, 70]);
                }
            }
        }
        let xray = ws.shade == Shade::Xray && cam.is_none();
        for i in 0..(w * h) as usize {
            let a = pic.pixels[i * 4 + 3] as u32;
            let a = if xray { a * 45 / 100 } else { a };
            for c in 0..3 {
                img.pixels[i * 4 + c] = ((img.pixels[i * 4 + c] as u32 * (255 - a) + pic.pixels[i * 4 + c] as u32 * a) / 255) as u8;
            }
        }
        if ws.overlays {
            let objs = self.vp_objects(t);
            for ob in &objs {
                match ob.kind {
                    vp::Kind::Camera if Some(ob.id) != cam => {
                        if let Some(v) = ob.view {
                            let corner = |x: f64, y: f64| {
                                let d = sp::apply(&sp::transpose(&v.axes), [x * cw * 0.1, y * ch * 0.1, v.zoom * 0.2]);
                                [v.position[0] + d[0], v.position[1] + d[1], v.position[2] + d[2]]
                            };
                            let cs = [corner(-1.0, -1.0), corner(1.0, -1.0), corner(1.0, 1.0), corner(-1.0, 1.0)];
                            for i in 0..4 {
                                seg(&mut img, v.position, cs[i], [220, 220, 230, 220]);
                                seg(&mut img, cs[i], cs[(i + 1) % 4], [220, 220, 230, 220]);
                            }
                        }
                    }
                    vp::Kind::Light => {
                        let r = cw * 0.015;
                        for d in [[r, 0.0, 0.0], [0.0, r, 0.0], [0.0, 0.0, r]] {
                            seg(&mut img, [ob.anchor[0] - d[0], ob.anchor[1] - d[1], ob.anchor[2] - d[2]], [ob.anchor[0] + d[0], ob.anchor[1] + d[1], ob.anchor[2] + d[2]], [255, 220, 120, 230]);
                        }
                        if let Some(a) = ob.aim {
                            seg(&mut img, ob.anchor, a, [255, 220, 120, 120]);
                        }
                    }
                    vp::Kind::Layer | vp::Kind::Card if cam.is_none() => {
                        if let Some(c) = ob.corners {
                            for i in 0..4 {
                                seg(&mut img, c[i], c[(i + 1) % 4], [255, 255, 255, if ob.kind == vp::Kind::Layer { 110 } else { 50 }]);
                            }
                        }
                    }
                    vp::Kind::Model if matches!(ws.shade, Shade::Wireframe | Shade::Xray) => {
                        if let Some((file, mx)) = &ob.model
                            && let Some(mesh) = vp::mesh_of(file)
                        {
                            let every = (mesh.triangles() / 4000).max(1);
                            for tri in (0..mesh.triangles()).step_by(every) {
                                let [a, b, c] = mesh.tri(tri);
                                let p = |i: usize| vp::xform(mx, mesh.positions[i].map(|v| v as f64));
                                seg(&mut img, p(a), p(b), [220, 220, 220, 120]);
                                seg(&mut img, p(b), p(c), [220, 220, 220, 120]);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(img)
    }

    pub(super) fn viewport_tool(&mut self, a: &Value) -> Result<Vec<Content>, String> {
        vp::follow(&self.doc);
        let t = self.vp_time(a);
        let w = self.doc.project.width as f64;
        let h = self.doc.project.height as f64;
        let action = a.get("action").or_else(|| a.get("do")).and_then(Value::as_str).unwrap_or("get").to_lowercase();
        let mut note = String::new();
        match action.as_str() {
            "get" | "read" | "state" => {}
            "on" | "open" | "show" => vp::with(|w| {
                w.on = true;
                w.changed();
            }),
            "off" | "close" => vp::with(|w| {
                w.on = false;
                w.changed();
            }),
            "orbit" => {
                let (dy, dp) = (f(a, "yaw").unwrap_or(0.0), f(a, "pitch").unwrap_or(0.0));
                let about = match a.get("about").and_then(Value::as_str) {
                    Some("selection") | Some("clips") => {
                        let ids = clip_ids(a, "clips");
                        let objs = self.vp_objects(t);
                        vp::extent(&objs, &ids).map(|e| e.0)
                    }
                    Some("cursor") => Some(vp::with(|w| w.cursor)),
                    _ => None,
                };
                self.vp_nav(t, |n| match about {
                    Some(p) => n.orbit_about(p, dy, dp),
                    None => n.orbit(dy, dp),
                })?;
            }
            "pan" => {
                let (x, y) = (f(a, "x").unwrap_or(0.0), f(a, "y").unwrap_or(0.0));
                self.vp_nav(t, |n| n.pan(x, y, w))?;
            }
            "dolly" | "zoom" => {
                let factor = f(a, "factor").unwrap_or(1.0);
                if factor <= 0.0 {
                    return Err("factor must be above 0 (0.5 goes twice as close)".into());
                }
                let at = [f(a, "x").unwrap_or(0.5) * w - w / 2.0, f(a, "y").unwrap_or(0.5) * h - h / 2.0];
                self.vp_nav(t, |n| n.zoom_at(factor, at, w))?;
            }
            "fly" | "travel" => {
                let by = [f(a, "right").unwrap_or(0.0), -f(a, "up").unwrap_or(0.0), f(a, "forward").unwrap_or(0.0)];
                self.vp_nav(t, |n| {
                    n.ortho = false;
                    n.travel(by)
                })?;
            }
            "walk" => {
                let (fw, rt, up) = (f(a, "forward").unwrap_or(0.0), f(a, "right").unwrap_or(0.0), f(a, "up").unwrap_or(0.0));
                self.vp_nav(t, |n| {
                    n.ortho = false;
                    n.walk(fw, rt, up)
                })?;
            }
            "look" => {
                let (dy, dp) = (f(a, "yaw").unwrap_or(0.0), f(a, "pitch").unwrap_or(0.0));
                self.vp_nav(t, |n| n.look(dy, dp))?;
            }
            "set" => {
                let target = v3(a.get("target"));
                let position = v3(a.get("position"));
                let look_at = v3(a.get("look_at"));
                self.vp_nav(t, |n| {
                    if let Some(l) = f(a, "lens").or_else(|| f(a, "lens_mm")) {
                        n.lens = l.clamp(4.0, 2000.0);
                    }
                    if let Some(o) = a.get("orthographic").and_then(Value::as_bool) {
                        n.ortho = o;
                    }
                    if let (Some(p), Some(l)) = (position, look_at.or(target)) {
                        // Standing somewhere is a perspective view unless
                        // asked otherwise.
                        let v = sp::View { position: p, axes: sp::look_at(p, l, 0.0), zoom: n.zoom(w) };
                        let lens = n.lens;
                        *n = Nav::from_view(&v, w, Some(l));
                        n.lens = lens;
                        n.ortho = a.get("orthographic").and_then(Value::as_bool).unwrap_or(false);
                        return;
                    }
                    if let Some(tg) = target {
                        n.target = tg;
                    }
                    if let Some(y) = f(a, "yaw") {
                        n.yaw = vp::nav::wrap(y);
                    }
                    if let Some(p) = f(a, "pitch") {
                        n.pitch = p.clamp(-90.0, 90.0);
                    }
                    if let Some(d) = f(a, "distance") {
                        n.distance = d.max(vp::nav::MIN_DISTANCE);
                    }
                })?;
            }
            "view" | "axis" => {
                let name = a.get("axis").or_else(|| a.get("view")).and_then(Value::as_str).unwrap_or("");
                match name {
                    "perspective" => self.vp_nav(t, |n| n.ortho = false)?,
                    "orthographic" | "ortho" => self.vp_nav(t, |n| n.ortho = true)?,
                    "camera" => vp::with(|w| {
                        w.on = true;
                        w.look = Look::Camera(None);
                        w.changed();
                    }),
                    _ => {
                        let ax = Axis::parse(name).ok_or("view needs \"axis\": front, back, left, right, top, bottom, perspective, orthographic or camera")?;
                        self.vp_nav(t, |n| n.set_axis(ax))?;
                    }
                }
            }
            "frame" => {
                let ids = clip_ids(a, "clips");
                let objs = self.vp_objects(t);
                let (c, r) = vp::extent(&objs, &ids).ok_or("nothing in the world to frame at that moment")?;
                self.vp_nav(t, |n| n.frame(c, r, w, w / h.max(1.0)))?;
            }
            "look_through" | "camera" => {
                let id = match a.get("camera") {
                    Some(Value::Number(n)) => n.as_u64(),
                    _ => None,
                };
                if let Some(id) = id {
                    vp::look_camera(&self.doc, Look::Camera(Some(id)), t).ok_or(format!("clip {id} is not a camera"))?;
                }
                vp::with(|w| {
                    w.on = true;
                    w.look = Look::Camera(id);
                    w.changed();
                });
            }
            "editor" | "editor_view" => vp::with(|w| {
                w.on = true;
                w.look = Look::Editor;
                w.changed();
            }),
            "camera_to_view" | "set_camera" => {
                let nav = vp::with(|w| w.nav);
                let id = match a.get("camera").and_then(Value::as_u64) {
                    Some(id) => id,
                    None => match vp::look_camera(&self.doc, Look::Camera(None), t) {
                        Some(id) => id,
                        None => {
                            let made = crate::editor::viewport::add_camera(&mut self.doc).ok_or("could not add a camera")?;
                            note = format!("added camera {made}");
                            made
                        }
                    },
                };
                vp::set_camera_to_view(&mut self.doc, id, &nav, t, None)?;
            }
            "view_from_camera" => {
                let id = a.get("camera").and_then(Value::as_u64).or_else(|| vp::look_camera(&self.doc, Look::Camera(None), t)).ok_or("no camera")?;
                let n = vp::camera_nav(&self.doc, id, t).ok_or("no such camera")?;
                vp::with(|w| {
                    w.on = true;
                    w.look = Look::Editor;
                    w.nav = n;
                    w.changed();
                });
            }
            "lock_camera" => {
                let on = a.get("on").and_then(Value::as_bool).unwrap_or(true);
                vp::with(|w| {
                    w.lock_camera = on;
                    if on {
                        w.look = Look::Camera(None);
                        w.on = true;
                    }
                    w.changed();
                });
            }
            "bookmark" | "save_view" => {
                let name = a.get("name").and_then(Value::as_str).ok_or("bookmark needs \"name\"")?.to_owned();
                vp::with(|w| {
                    let nav = w.nav;
                    w.bookmarks.retain(|b| b.name != name);
                    w.bookmarks.push(vp::Bookmark { name, nav });
                    w.changed();
                });
            }
            "go" | "go_to" | "goto" => {
                let name = a.get("name").and_then(Value::as_str).ok_or("go needs \"name\" of a saved view")?;
                let b = vp::with(|w| w.bookmarks.iter().find(|b| b.name == name).cloned()).ok_or(format!("no saved view called {name}"))?;
                vp::with(|w| {
                    w.on = true;
                    w.look = Look::Editor;
                    w.nav = b.nav;
                    w.changed();
                });
            }
            "forget" => {
                let name = a.get("name").and_then(Value::as_str).ok_or("forget needs \"name\"")?.to_owned();
                vp::with(|w| {
                    w.bookmarks.retain(|b| b.name != name);
                    w.changed();
                });
            }
            "shading" | "shade" => {
                let s = a.get("mode").or_else(|| a.get("shading")).and_then(Value::as_str).and_then(Shade::parse).ok_or("shading needs \"mode\": solid, material, rendered, wireframe or xray")?;
                vp::with(|w| {
                    w.shade = s;
                    w.changed();
                });
            }
            "spread" => {
                let px = f(a, "px").or_else(|| f(a, "spread")).ok_or("spread needs \"px\": depth between neighbouring 2D layers")?;
                vp::with(|w| {
                    w.spread = px.max(0.0);
                    w.cards = true;
                    w.on = true;
                    w.changed();
                });
            }
            "options" | "settings" => vp::with(|w| {
                let b = |k: &str, cur: bool| a.get(k).and_then(Value::as_bool).unwrap_or(cur);
                w.cards = b("cards", w.cards);
                w.grid = b("grid", w.grid);
                w.overlays = b("overlays", w.overlays);
                w.frame = b("frame", w.frame);
                w.quad = b("quad", w.quad);
                if let Some(s) = a.get("snap").and_then(Value::as_bool) {
                    w.snap.on = s;
                }
                if let Some(v) = f(a, "snap_step") {
                    w.snap.step = v;
                }
                if let Some(v) = f(a, "snap_angle") {
                    w.snap.angle = v;
                }
                if let Some(v) = a.get("tool").and_then(Value::as_str).and_then(vp::Tool::parse) {
                    w.tool = v;
                }
                if let Some(v) = a.get("orient").and_then(Value::as_str).and_then(vp::Orient::parse) {
                    w.orient = v;
                }
                if let Some(v) = a.get("pivot").and_then(Value::as_str).and_then(vp::Pivot::parse) {
                    w.pivot = v;
                }
                if let Some(v) = f(a, "floor_y") {
                    w.floor = Some(v);
                }
                if let Some(v) = f(a, "speed") {
                    w.speed = v.max(1.0);
                }
                w.changed();
            }),
            "hide" | "unhide" | "lock" | "unlock" => {
                let ids = clip_ids(a, "clips");
                if ids.is_empty() {
                    return Err(format!("{action} needs \"clips\""));
                }
                vp::with(|w| {
                    for id in &ids {
                        match action.as_str() {
                            "hide" => {
                                w.hidden.insert(*id);
                            }
                            "unhide" => {
                                w.hidden.remove(id);
                            }
                            "lock" => {
                                w.locked.insert(*id);
                            }
                            _ => {
                                w.locked.remove(id);
                            }
                        }
                    }
                    w.changed();
                });
            }
            "pick" | "select" => {
                let mut ids = clip_ids(a, "clips");
                let mut picked = Value::Null;
                if ids.is_empty()
                    && let (Some(x), Some(y)) = (f(a, "x"), f(a, "y"))
                {
                    let objs = self.vp_objects(t);
                    let (view, _) = self.vp_view(t);
                    let locked = vp::with(|w| w.locked.clone());
                    let hit = vp::pick(&objs, &view, [x * w - w / 2.0, y * h - h / 2.0], w * 0.01, &locked);
                    if let Some((id, dist)) = hit {
                        let name = objs.iter().find(|o| o.id == id).map(|o| o.name.clone()).unwrap_or_default();
                        picked = json!({"clip": id, "name": name, "distance": r1(dist)});
                        ids = vec![id];
                    } else {
                        picked = json!("nothing there");
                    }
                }
                if action == "select" {
                    vp::with(|w| w.rt.select = Some(ids.iter().map(|i| *i as u32).collect()));
                }
                if !picked.is_null() {
                    return Ok(vec![Content::Text(json!({"picked": picked, "selected": action == "select"}).to_string())]);
                }
            }
            "move" | "rotate" | "turn" | "scale" => {
                let ids = clip_ids(a, "clips");
                if ids.is_empty() {
                    return Err(format!("{action} needs \"clips\""));
                }
                let objs = self.vp_objects(t);
                let ws = vp::snapshot();
                let space = a.get("space").and_then(Value::as_str).and_then(vp::Orient::parse).unwrap_or(vp::Orient::World);
                let snap = a.get("snap").and_then(Value::as_bool).unwrap_or(ws.snap.on);
                let pivot_kind = a.get("pivot").and_then(Value::as_str).and_then(vp::Pivot::parse).unwrap_or(ws.pivot);
                let pivot = match pivot_kind {
                    vp::Pivot::Individual => None,
                    vp::Pivot::Cursor => Some(ws.cursor),
                    vp::Pivot::Median | vp::Pivot::Bounds | vp::Pivot::Active => {
                        let pts: Vec<[f64; 3]> = objs.iter().filter(|o| ids.contains(&o.id)).map(|o| o.anchor).collect();
                        let n = pts.len().max(1) as f64;
                        match pivot_kind {
                            vp::Pivot::Active => pts.first().copied(),
                            vp::Pivot::Bounds if !pts.is_empty() => {
                                let lo: [f64; 3] = std::array::from_fn(|i| pts.iter().map(|p| p[i]).fold(f64::INFINITY, f64::min));
                                let hi: [f64; 3] = std::array::from_fn(|i| pts.iter().map(|p| p[i]).fold(f64::NEG_INFINITY, f64::max));
                                Some(std::array::from_fn(|i| (lo[i] + hi[i]) / 2.0))
                            }
                            _ => Some(std::array::from_fn(|i| pts.iter().map(|p| p[i]).sum::<f64>() / n)),
                        }
                    }
                };
                // The axes `by` is given in.
                let axes: sp::Mat3 = match space {
                    vp::Orient::World => [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                    vp::Orient::View => {
                        let (v, _) = self.vp_view(t);
                        // Rows to columns: right, down, forward.
                        sp::transpose(&v.axes)
                    }
                    vp::Orient::Local | vp::Orient::Normal => objs.iter().find(|o| Some(&o.id) == ids.first()).map_or([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], |o| o.axes),
                };
                let mut c = vp::Change::default();
                let label;
                match action.as_str() {
                    "move" => {
                        let mut by = v3(a.get("by")).ok_or("move needs \"by\": [x, y, z] pixels (y down)")?;
                        if snap {
                            by = by.map(|v| vp::Snap::value(v, ws.snap.step));
                        }
                        c.moved = sp::apply(&axes, by);
                        label = "Move in the viewport";
                    }
                    "scale" => {
                        let k = match a.get("by") {
                            Some(Value::Number(n)) => {
                                let k = n.as_f64().unwrap_or(1.0);
                                [k; 3]
                            }
                            other => v3(other).ok_or("scale needs \"by\": a factor, or [x, y, z] factors")?,
                        };
                        if k.iter().any(|v| *v <= 0.0) {
                            return Err("scale factors must be above 0".into());
                        }
                        c.scaled = Some(k);
                        label = "Scale in the viewport";
                    }
                    _ => {
                        let mut by = v3(a.get("by")).ok_or("rotate needs \"by\": [x, y, z] degrees")?;
                        if snap {
                            by = by.map(|v| vp::Snap::value(v, ws.snap.angle));
                        }
                        if space == vp::Orient::World {
                            c.turned = by;
                        } else {
                            // One turn per axis of that space, X then Y then Z.
                            let col = |i: usize| [axes[0][i], axes[1][i], axes[2][i]];
                            let (k, deg) = (0..3).map(|i| (i, by[i])).max_by(|x, y| x.1.abs().partial_cmp(&y.1.abs()).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or((0, 0.0));
                            if by.iter().filter(|v| v.abs() > 1e-9).count() > 1 {
                                return Err("in view or local space turn about one axis at a time".into());
                            }
                            // Degrees as the rotation properties count them
                            // (X and Y turn left-handed).
                            c.turn_about = Some((col(k), if k < 2 { -deg } else { deg }));
                        }
                        label = "Rotate in the viewport";
                    }
                }
                let n = vp::change(&mut self.doc, &ids, t, &c, pivot, label, None)?;
                note = format!("{n} changed");
            }
            "cursor" => {
                let at = v3(a.get("at")).ok_or("cursor needs \"at\": [x, y, z]")?;
                vp::with(|w| {
                    w.cursor = at;
                    w.changed();
                });
            }
            "record_path" | "fly_through" => {
                // Waypoints: views (or saved views by name) at times; the
                // camera is keyed through them a key every other frame.
                let keys = a.get("keys").and_then(Value::as_array).ok_or("record_path needs \"keys\": [{t, target, yaw, pitch, distance} or {t, view: saved name} or {t, position, look_at}]")?.clone();
                if keys.len() < 2 {
                    return Err("record_path needs at least two keys".into());
                }
                let mut way: Vec<(Time, Nav)> = Vec::new();
                let base = vp::with(|w| w.nav);
                for k in &keys {
                    let at = micros(f(k, "t").ok_or("each key needs \"t\" (seconds)")?);
                    let mut n = base;
                    if let Some(name) = k.get("view").and_then(Value::as_str) {
                        n = vp::with(|w| w.bookmarks.iter().find(|b| b.name == name).map(|b| b.nav)).ok_or(format!("no saved view called {name}"))?;
                    }
                    if let (Some(p), Some(l)) = (v3(k.get("position")), v3(k.get("look_at"))) {
                        n = Nav::from_view(&sp::View { position: p, axes: sp::look_at(p, l, 0.0), zoom: n.zoom(w) }, w, Some(l));
                        n.lens = base.lens;
                    }
                    if let Some(tg) = v3(k.get("target")) {
                        n.target = tg;
                    }
                    for (key, slot) in [("yaw", &mut n.yaw), ("pitch", &mut n.pitch), ("distance", &mut n.distance)] {
                        if let Some(v) = f(k, key) {
                            *slot = v;
                        }
                    }
                    n.ortho = false;
                    way.push((at, n));
                }
                way.sort_by_key(|x| x.0);
                let step = (1e6 / self.doc.project.fps.max(1.0) as f64) as Time;
                let mut samples = Vec::new();
                for pair in way.windows(2) {
                    let (t0, a0) = pair[0];
                    let (t1, a1) = pair[1];
                    let mut tt = t0;
                    while tt < t1 {
                        samples.push((tt, Nav::lerp(&a0, &a1, (tt - t0) as f64 / (t1 - t0).max(1) as f64)));
                        tt += step;
                    }
                }
                if let Some(last) = way.last() {
                    samples.push(*last);
                }
                let id = match a.get("camera").and_then(Value::as_u64) {
                    Some(id) => id,
                    None => match vp::look_camera(&self.doc, Look::Camera(None), way[0].0) {
                        Some(id) => id,
                        None => {
                            let made = vp::add_camera(&mut self.doc).ok_or("could not add a camera")?;
                            note = format!("added camera {made}");
                            made
                        }
                    },
                };
                let n = vp::record_path(&mut self.doc, id, &samples)?;
                note = format!("{note} keyed {n} camera keys on clip {id}").trim().to_owned();
            }
            "picture" | "see" | "look_at_view" => {}
            other => return Err(format!("viewport does not know \"{other}\"; actions: get, on, off, orbit, pan, dolly, fly, walk, look, set, view, frame, look_through, editor, camera_to_view, view_from_camera, lock_camera, bookmark, go, forget, shading, spread, options, hide, unhide, lock, unlock, pick, select, move, rotate, scale, cursor, record_path, picture")),
        }
        vp::save(&self.doc);
        let mut state = self.vp_state(t);
        scope_objects(&mut state, a, &action);
        if !note.is_empty() {
            state["note"] = json!(note);
        }
        let want_picture = action == "picture" || action == "see" || action == "look_at_view" || a.get("picture").and_then(Value::as_bool).unwrap_or(false);
        if want_picture {
            let width = a.get("width").and_then(Value::as_u64).unwrap_or(768).clamp(128, 1920) as u32;
            let img = self.vp_picture(t, width)?;
            return Ok(vec![Content::Jpeg(studio_content::jpeg::encode(&img, 82)), Content::Text(state.to_string())]);
        }
        Ok(vec![Content::Text(state.to_string())])
    }
}

/// Objects listed by default (a world of trees and rocks came back as
/// hundreds of objects on every move of the eye).
const OBJECTS_SHOWN: usize = 30;

/// The objects a reply lists: those in `clips` when given; for `get`, the
/// ones in view first, up to `limit` (`all: true` for every one); after
/// any other action only the clips it acted on. The rest are counted.
fn scope_objects(state: &mut Value, a: &Value, action: &str) {
    let Some(all) = state["objects"].as_array().cloned() else { return };
    let total = all.len();
    let wanted: Vec<u64> = match a.get("clips") {
        Some(Value::Array(v)) => v.iter().filter_map(Value::as_u64).collect(),
        Some(v) => v.as_u64().into_iter().collect(),
        None => Vec::new(),
    };
    let listed: Vec<Value> = if a.get("all").and_then(Value::as_bool) == Some(true) {
        all
    } else if !wanted.is_empty() {
        all.into_iter().filter(|o| o["id"].as_u64().is_some_and(|id| wanted.contains(&id))).collect()
    } else if matches!(action, "get" | "read" | "state" | "picture" | "see" | "look_at_view") {
        let limit = a.get("limit").and_then(Value::as_u64).map_or(OBJECTS_SHOWN, |n| n.clamp(1, 500) as usize);
        let (mut seen, rest): (Vec<Value>, Vec<Value>) = all.into_iter().partition(|o| o["on_screen"]["in_view"] == json!(true));
        seen.extend(rest);
        seen.truncate(limit);
        seen
    } else {
        Vec::new()
    };
    if listed.len() < total {
        state["objects_note"] = json!(format!("{} of {total} objects listed (in view first); viewport {{\"action\": \"get\", \"clips\": [ids]}} for some, \"all\": true for every one", listed.len()));
    }
    state["objects"] = json!(listed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_lists_a_few_objects_and_counts_the_rest() {
        let objs: Vec<Value> = (0..100).map(|i| json!({"id": i, "on_screen": {"in_view": i % 2 == 0}})).collect();
        let mut st = json!({"objects": objs});
        scope_objects(&mut st, &json!({}), "get");
        let o = st["objects"].as_array().unwrap();
        assert_eq!(o.len(), OBJECTS_SHOWN);
        assert!(o.iter().all(|x| x["on_screen"]["in_view"] == true), "in view first");
        assert!(st["objects_note"].as_str().unwrap().contains("of 100"));
        let mut st = json!({"objects": objs});
        scope_objects(&mut st, &json!({"clips": [3, 4]}), "move");
        assert_eq!(st["objects"].as_array().unwrap().len(), 2);
        let mut st = json!({"objects": objs});
        scope_objects(&mut st, &json!({}), "orbit");
        assert!(st["objects"].as_array().unwrap().is_empty(), "moving the eye lists nothing");
        let mut st = json!({"objects": objs});
        scope_objects(&mut st, &json!({"all": true}), "get");
        assert_eq!(st["objects"].as_array().unwrap().len(), 100);
    }

    fn session_with_scene() -> (Session, u64, u64) {
        let mut doc = crate::services::project::new_project(&crate::services::project::CANVASES[0]);
        let cam = vp::add_camera(&mut doc).expect("camera");
        let seq = doc.seq();
        let lane = doc.alloc_id();
        let id = doc.alloc_id();
        let mut edits = vec![m::Edit::AddLane { seq, lane: m::Lane::new(m::LaneId(lane), m::LaneKind::Visual, "Cards"), index: Some(0) }];
        let mut g = m::Generator::new(m::GeneratorKind::Solid);
        g.params.insert("colour".into(), m::Value::Colour(m::Rgba::new(0.9, 0.2, 0.2, 1.0)));
        let mut c = m::Clip::new(m::ClipId(id), m::LaneId(lane), 0, 4_000_000, m::ClipSource::Generator(g));
        c.name = "Red card".into();
        c.transform.three_d = true;
        c.transform.scale_x = 0.3;
        c.transform.scale_y = 0.3;
        edits.push(m::Edit::InsertClip { seq, clip: Box::new(c) });
        assert!(doc.apply("Cards", m::Edit::Batch { edits }));
        let s = Session::new(doc);
        vp::with(|w| *w = vp::Workspace::for_canvas(1920.0, 1080.0));
        (s, cam, id)
    }

    fn json_of(out: &[Content]) -> Value {
        out.iter().find_map(|c| if let Content::Text(t) = c { serde_json::from_str(t).ok() } else { None }).expect("json")
    }

    #[test]
    fn the_ai_reads_and_drives_the_workspace() {
        let _g = crate::editor::viewport::test_lock();
        let (mut s, cam, card) = session_with_scene();
        let v = json_of(&s.viewport_tool(&json!({"action": "get"})).unwrap());
        assert!(v["objects"].as_array().unwrap().iter().any(|o| o["id"] == card && o["kind"] == "layer"));
        assert!(v["objects"].as_array().unwrap().iter().any(|o| o["id"] == cam && o["kind"] == "camera"));
        // Axis view, orbit, frame.
        let v = json_of(&s.viewport_tool(&json!({"action": "view", "axis": "top"})).unwrap());
        assert_eq!(v["eye"]["pitch"], 90.0);
        assert_eq!(v["eye"]["orthographic"], true);
        let v = json_of(&s.viewport_tool(&json!({"action": "orbit", "yaw": 30, "pitch": -60})).unwrap());
        assert_eq!(v["eye"]["yaw"], 30.0);
        let v = json_of(&s.viewport_tool(&json!({"action": "frame", "clips": [card]})).unwrap());
        assert!(v["objects"].as_array().unwrap().iter().any(|o| o["id"] == card && o["on_screen"]["in_view"] == true));
        // Bookmark and come back.
        s.viewport_tool(&json!({"action": "bookmark", "name": "overhead"})).unwrap();
        s.viewport_tool(&json!({"action": "orbit", "yaw": 90})).unwrap();
        let v = json_of(&s.viewport_tool(&json!({"action": "go", "name": "overhead"})).unwrap());
        assert_eq!(v["eye"]["yaw"], 30.0);
        // Pick what is in the middle of a framed view.
        let v = json_of(&s.viewport_tool(&json!({"action": "pick", "x": 0.5, "y": 0.5})).unwrap());
        assert_eq!(v["picked"]["clip"], card);
    }

    #[test]
    fn the_ai_moves_things_and_shoots_with_the_camera() {
        let _g = crate::editor::viewport::test_lock();
        let (mut s, cam, card) = session_with_scene();
        s.viewport_tool(&json!({"action": "move", "clips": [card], "by": [100, 0, 400]})).unwrap();
        let mc = s.doc.model.active().clip(m::ClipId(card)).unwrap();
        assert_eq!((mc.transform.position.x, mc.transform.position_z), (100.0, 400.0));
        s.viewport_tool(&json!({"action": "rotate", "clips": [card], "by": [0, 45, 0]})).unwrap();
        let mc = s.doc.model.active().clip(m::ClipId(card)).unwrap();
        assert!((mc.transform.rotation_y - 45.0).abs() < 1e-6, "{}", mc.transform.rotation_y);
        s.viewport_tool(&json!({"action": "scale", "clips": [card], "by": 2.0, "pivot": "individual"})).unwrap();
        let mc = s.doc.model.active().clip(m::ClipId(card)).unwrap();
        assert!((mc.transform.scale_x - 0.6).abs() < 1e-9);
        // Locked: refused.
        s.viewport_tool(&json!({"action": "lock", "clips": [card]})).unwrap();
        assert!(s.viewport_tool(&json!({"action": "move", "clips": [card], "by": [1, 0, 0]})).is_err());
        s.viewport_tool(&json!({"action": "unlock", "clips": [card]})).unwrap();
        // The camera takes the view.
        s.viewport_tool(&json!({"action": "set", "position": [800, -400, -1500], "look_at": [100, 0, 400]})).unwrap();
        s.viewport_tool(&json!({"action": "camera_to_view", "camera": cam})).unwrap();
        let v = sp::camera_view(s.doc.model.active(), s.doc.model.active().clip(m::ClipId(cam)).unwrap(), 0, 1920.0);
        assert!((v.position[0] - 800.0).abs() < 1e-6 && (v.position[2] + 1500.0).abs() < 1e-6, "{:?}", v.position);
        // A fly-through from two views keys the camera.
        let v = json_of(&s.viewport_tool(&json!({"action": "record_path", "camera": cam, "keys": [{"t": 0, "position": [0, -200, -2500], "look_at": [0, 0, 0]}, {"t": 2, "position": [1500, -300, -800], "look_at": [100, 0, 400]}]})).unwrap());
        assert!(v["note"].as_str().unwrap().contains("keyed"));
        let mc = s.doc.model.active().clip(m::ClipId(cam)).unwrap();
        assert!(mc.keyframes.get("transform.position").is_some_and(|k| k.keys.len() > 10));
        let mid = sp::camera_view(s.doc.model.active(), mc, 1_000_000, 1920.0);
        assert!(mid.position[0] > 100.0 && mid.position[0] < 1400.0, "{:?}", mid.position);
        // Looking through it, the picture comes back.
        let out = s.viewport_tool(&json!({"action": "look_through", "camera": cam, "picture": true, "width": 320})).unwrap();
        assert!(matches!(out[0], Content::Jpeg(_)));
    }

    #[test]
    fn the_ai_sees_the_workspace_with_spread_cards_and_shading() {
        let _g = crate::editor::viewport::test_lock();
        let (mut s, _cam, _card) = session_with_scene();
        for mode in ["solid", "material", "rendered", "wireframe", "xray"] {
            let out = s.viewport_tool(&json!({"action": "shading", "mode": mode, "picture": true, "width": 256})).unwrap();
            let Content::Jpeg(j) = &out[0] else { panic!("{mode}: no picture") };
            assert!(j.len() > 500, "{mode}");
        }
        let v = json_of(&s.viewport_tool(&json!({"action": "spread", "px": 300})).unwrap());
        assert_eq!(v["spread"], 300.0);
    }
}
