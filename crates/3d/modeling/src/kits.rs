// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Kits: well-made starting assets built procedurally, each with real
//! proportions, bevelled edges and fitting smart materials — rocks,
//! trees, terrain, buildings, crates, barrels, columns, stairs, bolts,
//! pipes, tables and chairs. Every kit takes a size and a seed, so a set
//! dresser can fill a scene with variations instead of copies.

use crate::asset::{Asset, Part};
use crate::curves::{Along, Curve, Profile};
use crate::groom::Rng;
use crate::material;
use crate::modifiers::Modifier;
use crate::poly::Poly;
use crate::prim;
use studio_mesh::{V3, add, normalise, scale};

pub const KITS: &[(&str, &str)] = &[
    ("rock", "a weathered boulder (size = its width)"),
    ("tree", "a broadleaf tree with a trunk, branches and leaf clusters (size = height)"),
    ("pine", "a conifer (size = height)"),
    ("terrain", "rolling ground with ridges, eroded (size = width)"),
    ("building", "a block of floors with windows and a cornice (size = height)"),
    ("crate", "a wooden crate with planks and frame (size = width)"),
    ("barrel", "a staved barrel with iron hoops (size = height)"),
    ("column", "a fluted classical column with base and capital (size = height)"),
    ("stairs", "a straight flight of steps (size = height)"),
    ("bolt", "a hex bolt with a threaded shank (size = length)"),
    ("pipe", "a pipe run with flanges and a bend (size = length)"),
    ("table", "a wooden table (size = width)"),
    ("chair", "a wooden chair (size = seat height × 2)"),
    ("facade", "a dressed front wall: windows in real openings, doors, cornice, pilasters (size = height; `style` picks the era)"),
    ("street", "a street between rows of buildings with road, kerbs, sidewalks, lamps (size = length; `style`)"),
    ("interior", "a furnished room (size = width; `style`)"),
    ("bridge", "a bridge: arches, beams or cables (size = length; `style`)"),
    ("fence", "a fence with posts, infill and a gate (size = length; `style`)"),
];

fn part(name: &str, mesh: Poly, material: &str) -> Part {
    // A part is one material: merged pieces must not keep their own slots.
    let mut mesh = mesh;
    mesh.single_material();
    let mut p = Part::new(name, mesh);
    p.materials = vec![material.into()];
    p
}

fn bevel(width: f32, segments: usize) -> Modifier {
    Modifier::Bevel { width, segments, profile: 0.5, angle: 35.0, only_marked: false }
}

fn add_mats(a: &mut Asset, size: f32, names: &[&str]) {
    for n in names {
        if a.material(n).is_none()
            && let Some(m) = material::smart(n, size, None)
        {
            a.materials.push(m);
        }
    }
}

/// A box of `size` with its bottom at `y0` (y down: bottom is +y).
fn block(size: V3, at: V3) -> Poly {
    let mut p = prim::cube(size, 1);
    p.map_positions(|v| add(v, at));
    p
}

/// A kit by name.
pub fn build(name: &str, size: f32, seed: u64) -> Result<Asset, String> {
    let s = size.max(1.0);
    let mut rng = Rng::new(seed.wrapping_add(1));
    let mut a = Asset::new(name);
    match name {
        // Places: made by the architecture generators (a `style` there picks the era).
        "facade" | "street" | "interior" | "bridge" | "fence" => {
            let mut sp = crate::places::Spec::new(name, crate::places::Style::Classical);
            sp.size = s;
            sp.seed = seed;
            return crate::places::build(&sp);
        }
        "rock" | "boulder" => {
            let mut p = prim::quad_sphere(s * 0.5, 12);
            // Flattened, lumpy, then broken into facets.
            let (kx, kz) = (0.9 + 0.2 * rng.next(), 0.8 + 0.3 * rng.next());
            p.map_positions(|v| [v[0] * kx, v[1] * 0.65, v[2] * kz]);
            let mut m = vec![Modifier::Displace { amount: s * 0.12, size: s * 0.6, octaves: 4, seed: seed as u32 }, Modifier::Displace { amount: s * 0.03, size: s * 0.12, octaves: 3, seed: seed as u32 + 1 }];
            m.push(Modifier::Remesh { size: s / 90.0 });
            m.push(Modifier::Smooth { iterations: 2, factor: 0.3 });
            let mut r = part("Rock", p, "stone");
            r.modifiers = m;
            r.smooth_angle = 70.0;
            a.parts.push(r);
            add_mats(&mut a, s, &["stone"]);
        }
        "tree" | "pine" => {
            let pine = name == "pine";
            let h = s;
            let trunk_r = h * if pine { 0.03 } else { 0.04 };
            // Trunk: a tapering, slightly bending sweep.
            let lean = [rng.range(-0.05, 0.05) * h, 0.0, rng.range(-0.05, 0.05) * h];
            let trunk_path = Curve::Smooth { points: vec![[0.0, 0.0, 0.0], [lean[0] * 0.3, -h * 0.35, lean[2] * 0.3], [lean[0], -h * if pine { 0.95 } else { 0.6 }, lean[2]]], closed: false };
            let trunk = crate::curves::sweep(&trunk_path, &Profile::Circle { radius: trunk_r, segments: 12 }, &Along { scale: vec![1.3, 0.9, 0.35], twist: 0.0, caps: true }, 10);
            let mut t = part("Trunk", trunk, "bark");
            t.smooth_angle = 80.0;
            a.parts.push(t);
            let mut leaves = Poly::new();
            let mut branches = Poly::new();
            if pine {
                // Tiers of drooping cones.
                let tiers = 7;
                for i in 0..tiers {
                    let f = i as f32 / tiers as f32;
                    let y = -h * (0.25 + 0.7 * f);
                    let r = h * 0.28 * (1.0 - f).powf(0.9) + h * 0.03;
                    let mut c = prim::cone(r, h * 0.22, 14);
                    c.map_positions(|v| add([v[0], v[1], v[2]], [lean[0] * (0.3 + 0.7 * f), y, lean[2] * (0.3 + 0.7 * f)]));
                    leaves.merge(&c);
                }
            } else {
                // Branches up and out, a leaf cluster at each tip.
                let n = 7;
                for i in 0..n {
                    let ang = std::f32::consts::TAU * (i as f32 / n as f32 + rng.range(-0.05, 0.05));
                    let up = h * rng.range(0.42, 0.62);
                    let out = h * rng.range(0.2, 0.3);
                    let start = [lean[0] * 0.6, -up * 0.85, lean[2] * 0.6];
                    let tip = [start[0] + out * ang.cos(), -up - h * rng.range(0.1, 0.2), start[2] + out * ang.sin()];
                    let mid = [(start[0] + tip[0]) / 2.0, (start[1] + tip[1]) / 2.0 - h * 0.05, (start[2] + tip[2]) / 2.0];
                    let b = crate::curves::sweep(&Curve::Smooth { points: vec![start, mid, tip], closed: false }, &Profile::Circle { radius: trunk_r * 0.45, segments: 8 }, &Along { scale: vec![1.0, 0.3], twist: 0.0, caps: true }, 6);
                    branches.merge(&b);
                    let mut blob = prim::quad_sphere(h * rng.range(0.13, 0.18), 6);
                    blob.map_positions(|v| add([v[0] * 1.2, v[1] * 0.85, v[2] * 1.1], tip));
                    leaves.merge(&blob);
                }
                let mut crown = prim::quad_sphere(h * 0.22, 6);
                crown.map_positions(|v| add([v[0] * 1.3, v[1], v[2] * 1.2], [lean[0], -h * 0.78, lean[2]]));
                leaves.merge(&crown);
                let mut b = part("Branches", branches, "bark");
                b.smooth_angle = 80.0;
                a.parts.push(b);
            }
            let mut l = part("Leaves", leaves, "leaf");
            l.modifiers = vec![Modifier::Remesh { size: h / 70.0 }, Modifier::Displace { amount: h * 0.025, size: h * 0.05, octaves: 3, seed: seed as u32 }, Modifier::Smooth { iterations: 1, factor: 0.4 }];
            l.smooth_angle = 80.0;
            a.parts.push(l);
            add_mats(&mut a, s, &["bark", "leaf"]);
        }
        "terrain" => {
            let mut p = prim::plane(s, s, 96, 96);
            p.map_positions(|v| {
                let q = [v[0] / (s * 0.35), 0.0, v[2] / (s * 0.35)];
                let hills = crate::noise::fbm(q, 5, 0.5, seed as u32) * 0.12 * s;
                let ridge = crate::noise::ridged(scale(q, 0.7), 4, seed as u32 + 3) * 0.08 * s;
                [v[0], -(hills + ridge).max(-0.02 * s), v[2]]
            });
            let mut t = part("Ground", p, "terrain");
            t.modifiers = vec![Modifier::Smooth { iterations: 2, factor: 0.4 }];
            t.smooth_angle = 80.0;
            a.parts.push(t);
            add_mats(&mut a, s, &["terrain"]);
        }
        "building" => {
            let floors = ((s / 60.0).round() as usize).clamp(2, 40);
            let fh = s / floors as f32;
            let (w, d) = (fh * 5.0, fh * 3.5);
            // The block, with a recess for every window.
            let mut body = prim::cube([w, s, d], 1);
            body.map_positions(|v| add(v, [0.0, -s / 2.0, 0.0]));
            let mut b = part("Walls", body, "concrete");
            b.modifiers.push(bevel(fh * 0.02, 1));
            a.parts.push(b);
            let mut win = Poly::new();
            let cols = 5;
            for f in 0..floors {
                for c in 0..cols {
                    for (z, sign) in [(-d / 2.0, -1.0), (d / 2.0, 1.0)] {
                        let x = -w / 2.0 + w * (c as f32 + 0.5) / cols as f32;
                        let y = -fh * (f as f32 + 0.55);
                        let mut g = prim::cube([w / cols as f32 * 0.55, fh * 0.55, fh * 0.06], 1);
                        g.map_positions(|v| add(v, [x, y, z + sign * fh * 0.01]));
                        win.merge(&g);
                    }
                }
            }
            a.parts.push(part("Windows", win, "glass"));
            let mut cornice = block([w * 1.04, fh * 0.15, d * 1.04], [0.0, -s - fh * 0.07, 0.0]);
            cornice.creases.clear();
            let mut c = part("Cornice", cornice, "stone");
            c.modifiers.push(bevel(fh * 0.02, 2));
            a.parts.push(c);
            add_mats(&mut a, s, &["concrete", "glass", "stone"]);
        }
        "crate" => {
            let h = s;
            let t = h * 0.08;
            let mut frame = Poly::new();
            // Twelve edge beams.
            for (sz, at) in [
                ([h, t, t], [0.0, -t / 2.0, -h / 2.0 + t / 2.0]),
                ([h, t, t], [0.0, -t / 2.0, h / 2.0 - t / 2.0]),
                ([h, t, t], [0.0, -h + t / 2.0, -h / 2.0 + t / 2.0]),
                ([h, t, t], [0.0, -h + t / 2.0, h / 2.0 - t / 2.0]),
                ([t, t, h], [-h / 2.0 + t / 2.0, -t / 2.0, 0.0]),
                ([t, t, h], [h / 2.0 - t / 2.0, -t / 2.0, 0.0]),
                ([t, t, h], [-h / 2.0 + t / 2.0, -h + t / 2.0, 0.0]),
                ([t, t, h], [h / 2.0 - t / 2.0, -h + t / 2.0, 0.0]),
                ([t, h, t], [-h / 2.0 + t / 2.0, -h / 2.0, -h / 2.0 + t / 2.0]),
                ([t, h, t], [h / 2.0 - t / 2.0, -h / 2.0, -h / 2.0 + t / 2.0]),
                ([t, h, t], [-h / 2.0 + t / 2.0, -h / 2.0, h / 2.0 - t / 2.0]),
                ([t, h, t], [h / 2.0 - t / 2.0, -h / 2.0, h / 2.0 - t / 2.0]),
            ] {
                frame.merge(&block(sz, at));
            }
            let mut f = part("Frame", frame, "wood");
            f.modifiers.push(bevel(t * 0.12, 2));
            f.modifiers.push(Modifier::WeightedNormals);
            a.parts.push(f);
            // Planks on each side, slightly inset, with gaps.
            let mut planks = Poly::new();
            let n = 5;
            let gap = h * 0.012;
            for side in 0..4 {
                for i in 0..n {
                    let ph = (h - 2.0 * t) / n as f32 - gap;
                    let y = -t - (i as f32 + 0.5) * (ph + gap);
                    let len = h - 2.0 * t;
                    let jitter = rng.range(-0.004, 0.004) * h;
                    let mut b = prim::cube([len, ph, t * 0.5], 1);
                    b.map_positions(|v| add(v, [0.0, y, h / 2.0 - t * 0.35 + jitter]));
                    let ang = side as f32 * 90.0;
                    b.map_positions(|v| crate::modifiers::rotate_axis(v, 1, ang.to_radians()));
                    planks.merge(&b);
                }
            }
            let mut p = part("Planks", planks, "wood");
            p.modifiers.push(bevel(t * 0.08, 2));
            p.modifiers.push(Modifier::WeightedNormals);
            a.parts.push(p);
            add_mats(&mut a, s, &["wood"]);
        }
        "barrel" => {
            let h = s;
            let r = h * 0.33;
            // A bulging profile turned round.
            let prof: Vec<V3> = (0..=12).map(|i| {
                let t = i as f32 / 12.0;
                let bulge = 1.0 + 0.16 * (std::f32::consts::PI * t).sin();
                [r * bulge, -h * t, 0.0]
            }).collect();
            let mut prof = prof;
            prof.insert(0, [0.0, 0.0, 0.0]);
            prof.push([0.0, -h, 0.0]);
            let body = crate::edit::spin(&prof, 360.0, 24, 1);
            let mut b = part("Staves", body, "wood");
            b.smooth_angle = 60.0;
            a.parts.push(b);
            let mut hoops = Poly::new();
            for t in [0.12f32, 0.3, 0.7, 0.88] {
                let bulge = 1.0 + 0.16 * (std::f32::consts::PI * t).sin();
                let ring = crate::curves::sweep(&Curve::Circle { radius: r * bulge + h * 0.006, axis: 1, centre: [0.0, -h * t, 0.0], degrees: 360.0 }, &Profile::Rect { width: h * 0.012, height: h * 0.05, corner: h * 0.003, corner_segments: 2 }, &Along::default(), 6);
                hoops.merge(&ring);
            }
            a.parts.push(part("Hoops", hoops, "rusty_iron"));
            add_mats(&mut a, s, &["wood", "rusty_iron"]);
        }
        "column" => {
            let h = s;
            let r = h * 0.07;
            let mut shaft = prim::cylinder(r * 0.88, r, h * 0.82, 48, 8, prim::Cap::Ngon);
            shaft.map_positions(|v| add(v, [0.0, -h * 0.5, 0.0]));
            let mut sh = part("Shaft", shaft, "marble");
            // Flutes: grooves cut by a radial array of thin cylinders.
            let mut groove = prim::cylinder(r * 0.1, r * 0.1, h * 0.78, 8, 1, prim::Cap::Ngon);
            groove.map_positions(|v| add(v, [r * 0.97, -h * 0.5, 0.0]));
            let mut g = part("Flutes", groove, "marble");
            g.hidden = true;
            g.modifiers.push(Modifier::Radial { count: 20, axis: 1, degrees: 360.0, centre: [0.0; 3] });
            a.parts.push(g);
            sh.modifiers.push(Modifier::Boolean { op: "subtract".into(), with: "Flutes".into() });
            a.parts.push(sh);
            let mut base = block([r * 2.8, h * 0.06, r * 2.8], [0.0, -h * 0.03, 0.0]);
            base.merge(&{
                let mut c = prim::cylinder(r * 1.25, r * 1.25, h * 0.05, 48, 1, prim::Cap::Ngon);
                c.map_positions(|v| add(v, [0.0, -h * 0.085, 0.0]));
                c
            });
            let mut bp = part("Base", base, "marble");
            bp.modifiers.push(bevel(h * 0.004, 2));
            a.parts.push(bp);
            let mut cap = block([r * 2.6, h * 0.05, r * 2.6], [0.0, -h * 0.975, 0.0]);
            cap.merge(&{
                let mut c = prim::cylinder(r * 1.25, r * 0.9, h * 0.07, 48, 1, prim::Cap::Ngon);
                c.map_positions(|v| add(v, [0.0, -h * 0.915, 0.0]));
                c
            });
            let mut cp = part("Capital", cap, "marble");
            cp.modifiers.push(bevel(h * 0.004, 2));
            a.parts.push(cp);
            add_mats(&mut a, s, &["marble"]);
        }
        "stairs" => {
            let steps = ((s / 18.0).round() as usize).clamp(3, 40);
            let rise = s / steps as f32;
            let run = rise * 1.6;
            let width = rise * 6.0;
            let mut p = Poly::new();
            for i in 0..steps {
                p.merge(&block([width, rise * (i + 1) as f32, run], [0.0, -rise * (i + 1) as f32 / 2.0, run * i as f32]));
            }
            let mut st = part("Steps", p, "concrete");
            st.modifiers.push(bevel(rise * 0.04, 2));
            st.modifiers.push(Modifier::WeightedNormals);
            a.parts.push(st);
            add_mats(&mut a, s, &["concrete"]);
        }
        "bolt" => {
            let l = s;
            let r = l * 0.09;
            let mut head = prim::cylinder(r * 1.7, r * 1.7, l * 0.13, 6, 1, prim::Cap::Ngon);
            head.map_positions(|v| add(v, [0.0, -l * 0.065, 0.0]));
            let mut hp = part("Head", head, "steel");
            hp.modifiers.push(bevel(r * 0.12, 2));
            hp.modifiers.push(Modifier::WeightedNormals);
            a.parts.push(hp);
            let mut shank = prim::cylinder(r, r, l * 0.87, 24, 1, prim::Cap::Ngon);
            shank.map_positions(|v| add(v, [0.0, -l * 0.13 - l * 0.435, 0.0]));
            let mut sp = part("Shank", shank, "steel");
            sp.modifiers.push(bevel(r * 0.08, 2));
            a.parts.push(sp);
            let thread = crate::curves::sweep(&Curve::Helix { radius: r, height: l * 0.6, turns: 12.0, axis: 1 }, &Profile::Circle { radius: r * 0.12, segments: 6 }, &Along { caps: true, ..Default::default() }, 4);
            let mut tp = part("Thread", thread, "steel");
            tp.position = [0.0, -l * 0.62, 0.0];
            a.parts.push(tp);
            add_mats(&mut a, s, &["steel"]);
        }
        "pipe" => {
            let l = s;
            let r = l * 0.04;
            let path = Curve::Bezier { knots: vec![crate::curves::BezierKnot { at: [-l * 0.5, 0.0, 0.0], before: None, after: Some([0.0, 0.0, 0.0]) }, crate::curves::BezierKnot { at: [l * 0.3, -l * 0.3, 0.0], before: Some([l * 0.3, 0.0, 0.0]), after: None }], closed: false };
            let tube = crate::curves::sweep(&path, &Profile::Circle { radius: r, segments: 20 }, &Along { caps: true, ..Default::default() }, 16);
            let mut t = part("Pipe", tube, "painted_metal");
            t.smooth_angle = 60.0;
            a.parts.push(t);
            let pts = path.sample(16);
            let mut fl = Poly::new();
            for (i, at) in [(0usize, pts[0]), (pts.len() - 1, *pts.last().unwrap())] {
                let dir = if i == 0 { normalise(studio_mesh::sub(pts[1], pts[0])) } else { normalise(studio_mesh::sub(pts[pts.len() - 1], pts[pts.len() - 2])) };
                let mut f = prim::cylinder(r * 1.8, r * 1.8, r * 0.6, 24, 1, prim::Cap::Ngon);
                // Stand the flange along the pipe's end direction.
                let ang_z = if dir[0].abs() > 0.5 { 90f32 } else { 0.0 };
                f.map_positions(|v| add(crate::modifiers::rotate_axis(v, 2, ang_z.to_radians()), at));
                fl.merge(&f);
            }
            let mut f = part("Flanges", fl, "painted_metal");
            f.modifiers.push(bevel(r * 0.1, 2));
            a.parts.push(f);
            add_mats(&mut a, s, &["painted_metal"]);
        }
        "table" => {
            let w = s;
            let h = w * 0.45;
            let top_t = w * 0.04;
            let mut top = part("Top", block([w, top_t, w * 0.55], [0.0, -h, 0.0]), "wood_varnished");
            top.modifiers.push(bevel(top_t * 0.25, 3));
            top.modifiers.push(Modifier::WeightedNormals);
            a.parts.push(top);
            let mut legs = Poly::new();
            let leg = w * 0.045;
            for (x, z) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                legs.merge(&block([leg, h - top_t / 2.0, leg], [x * (w / 2.0 - leg * 1.5), -(h - top_t / 2.0) / 2.0, z * (w * 0.275 - leg * 1.5)]));
            }
            let mut l = part("Legs", legs, "wood_varnished");
            l.modifiers.push(Modifier::Taper { axis: 1, factor: 1.0 });
            l.modifiers.push(bevel(leg * 0.15, 2));
            a.parts.push(l);
            add_mats(&mut a, s, &["wood_varnished"]);
        }
        "chair" => {
            let seat_h = s / 2.0;
            let w = seat_h * 0.95;
            let t = w * 0.06;
            let mut seat = part("Seat", block([w, t, w], [0.0, -seat_h, 0.0]), "wood");
            seat.modifiers.push(bevel(t * 0.3, 3));
            a.parts.push(seat);
            let leg = w * 0.07;
            let mut legs = Poly::new();
            for (x, z) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                legs.merge(&block([leg, seat_h, leg], [x * (w / 2.0 - leg), -seat_h / 2.0, z * (w / 2.0 - leg)]));
            }
            // Back posts rise past the seat.
            for x in [-1.0f32, 1.0] {
                legs.merge(&block([leg, seat_h * 0.9, leg], [x * (w / 2.0 - leg), -seat_h - seat_h * 0.45, w / 2.0 - leg]));
            }
            let mut l = part("Frame", legs, "wood");
            l.modifiers.push(bevel(leg * 0.15, 2));
            a.parts.push(l);
            let mut back = part("Back", block([w * 0.85, seat_h * 0.3, t], [0.0, -seat_h * 1.65, w / 2.0 - leg]), "wood");
            back.modifiers.push(Modifier::Bend { axis: 0, toward: 2, degrees: 20.0 });
            back.modifiers.push(bevel(t * 0.3, 2));
            a.parts.push(back);
            add_mats(&mut a, s, &["wood"]);
        }
        _ => return Err(format!("kit \"{name}\": {}", KITS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", "))),
    }
    Ok(a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kit_builds_and_draws() {
        for (name, _) in KITS {
            let a = build(name, 200.0, 7).unwrap_or_else(|e| panic!("{name}: {e}"));
            let m = a.to_mesh(true).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(m.triangles() > 20, "{name}");
            let (lo, hi) = m.bounds();
            let big = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(hi[2] - lo[2]);
            assert!(big > 100.0 && big < 500.0, "{name}: size {big}");
            assert!(m.positions.iter().all(|p| p.iter().all(|v| v.is_finite())), "{name}");
        }
    }

    #[test]
    fn every_kit_part_is_wholly_on_a_defined_material() {
        for (name, _) in KITS {
            let a = build(name, 200.0, 7).unwrap_or_else(|e| panic!("{name}: {e}"));
            for p in &a.parts {
                let ctx = format!("{name}/{}", p.name);
                assert_eq!(p.materials.len(), 1, "{ctx}: one material name");
                assert!(p.mesh.face_material.iter().all(|&m| m == 0), "{ctx}: face off slot 0");
                assert_eq!(p.mesh.materials.len(), 1, "{ctx}: extra mesh materials");
                let def = a.material(&p.materials[0]).unwrap_or_else(|| panic!("{ctx}: material \"{}\" undefined", p.materials[0]));
                assert_eq!(def.name, p.materials[0], "{ctx}");
            }
        }
    }
}
