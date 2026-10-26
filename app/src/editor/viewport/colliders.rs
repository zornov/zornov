// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What models collide as, drawn over them in the 3D workspace (View menu,
//! "Physics colliders"): the convex shapes of props that move (orange),
//! the outline of scenery that stays put (blue), joints as small crosses.

use super::Object;
use super::ui::Pane;
use crate::app::StudioApp;
use crate::physics3d::{BodyType, Props};
use eframe::egui::{self, Color32, Stroke};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use studio_model as m;
use studio_sim::dynamics::decompose::{self, Fit, Shapes};
use studio_sim::dynamics::shape::Shape;

const MOVING: Color32 = Color32::from_rgb(255, 150, 50);
const FIXED: Color32 = Color32::from_rgb(90, 160, 255);

/// A model's shapes in its own frame, kept by file and fit.
fn shapes(file: &str, fit: Fit) -> Option<Arc<Shapes>> {
    static CACHE: OnceLock<Mutex<HashMap<(String, String), Option<Arc<Shapes>>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let key = (file.to_string(), format!("{fit:?}"));
    if let Some(s) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return s.clone();
    }
    let mesh = super::mesh_of(file)?;
    let tris: Vec<[[f32; 3]; 3]> = (0..mesh.triangles()).map(|t| mesh.tri(t).map(|v| mesh.positions[v])).collect();
    let s = decompose::shapes_for(&tris, fit, 24).map(Arc::new);
    let mut g = cache.lock().unwrap_or_else(|e| e.into_inner());
    if g.len() > 64 {
        g.clear();
    }
    g.insert(key, s.clone());
    s
}

fn clip(app: &StudioApp, id: u64) -> Option<m::Clip> {
    app.doc.model.sequences.iter().find_map(|q| q.clip(m::ClipId(id))).cloned()
}

pub fn draw(app: &StudioApp, p: &egui::Painter, pane: &Pane, objs: &[Object], _time: crate::view::Time) {
    let line = |a: [f64; 3], b: [f64; 3], s: Stroke| {
        if let Some(seg) = pane.segment(a, b) {
            p.line_segment(seg, s);
        }
    };
    for o in objs {
        let Some((file, mx)) = &o.model else { continue };
        let Some(c) = clip(app, o.id) else { continue };
        let props = Props::of(&c);
        if props.body == BodyType::None {
            continue;
        }
        let to = |v: [f32; 3]| super::xform(mx, v.map(|x| x as f64));
        let fixed = matches!(props.body, BodyType::Static | BodyType::Kinematic) || (props.body == BodyType::Auto && crate::physics3d::scene::keyed(&c));
        if fixed || props.fit == Fit::Mesh {
            // Scenery collides as its own triangles: its outline is enough.
            let (lo, hi) = (o.min, o.max);
            let cn = |i: usize| [if i & 1 == 0 { lo[0] } else { hi[0] }, if i & 2 == 0 { lo[1] } else { hi[1] }, if i & 4 == 0 { lo[2] } else { hi[2] }];
            for (a, b) in [(0, 1), (1, 3), (3, 2), (2, 0), (4, 5), (5, 7), (7, 6), (6, 4), (0, 4), (1, 5), (2, 6), (3, 7)] {
                line(cn(a), cn(b), Stroke::new(1.0, FIXED));
            }
            continue;
        }
        let Some(sh) = shapes(file, props.fit) else { continue };
        let s = Stroke::new(1.4, MOVING);
        for (shape, pose) in &sh.parts {
            match shape {
                Shape::Hull(h) => {
                    for e in &h.edges {
                        line(to(pose.apply(h.verts[e.a as usize])), to(pose.apply(h.verts[e.b as usize])), s);
                    }
                }
                Shape::Sphere { r } => {
                    for axis in 0..3 {
                        let ring: Vec<[f64; 3]> = (0..=32)
                            .map(|k| {
                                let a = k as f32 / 32.0 * std::f32::consts::TAU;
                                let mut v = [0.0f32; 3];
                                v[(axis + 1) % 3] = r * a.cos();
                                v[(axis + 2) % 3] = r * a.sin();
                                to(pose.apply(v))
                            })
                            .collect();
                        for w in ring.windows(2) {
                            line(w[0], w[1], s);
                        }
                    }
                }
                Shape::Capsule { half, r } => {
                    for y in [-*half, *half] {
                        let ring: Vec<[f64; 3]> = (0..=24).map(|k| {
                            let a = k as f32 / 24.0 * std::f32::consts::TAU;
                            to(pose.apply([r * a.cos(), y, r * a.sin()]))
                        }).collect();
                        for w in ring.windows(2) {
                            line(w[0], w[1], s);
                        }
                    }
                    for k in 0..4 {
                        let a = k as f32 * std::f32::consts::FRAC_PI_2;
                        line(to(pose.apply([r * a.cos(), -half, r * a.sin()])), to(pose.apply([r * a.cos(), *half, r * a.sin()])), s);
                    }
                }
                _ => {}
            }
        }
        // Joints: a cross where each one is.
        for j in &props.joints {
            if let Some(at) = j.at {
                let a = at.map(|v| v as f64);
                let d = 10.0;
                for k in 0..3 {
                    let (mut lo, mut hi) = (a, a);
                    lo[k] -= d;
                    hi[k] += d;
                    line(lo, hi, Stroke::new(2.0, Color32::from_rgb(120, 230, 120)));
                }
            }
        }
    }
}
