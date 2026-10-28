// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Running a scene through time and baking what happened.
//!
//! Every step, animated clips and characters are put where their keys and
//! animation say (kinematic bodies move there, pushing what is in the
//! way); throws and pushes start at their moments; a character hit hard
//! enough goes loose (a powered ragdoll whose strength fades, so it
//! staggers and falls); a breakable thing hit hard enough shatters into
//! pieces that fly on. Each frame is sampled: transforms of moving
//! bodies, bone poses of loose characters, the pieces of broken things.
//! [`bake`] turns that into ordinary keys and caches.

use super::JointSpec;
use super::scene::{Item, Role, Scene, frame_of, posed_rig};
use std::sync::Arc;
use studio_mesh::Mesh;
use studio_mesh::rig::BonePose;
use studio_model as m;
use studio_sim::dynamics::hull::Hull;
use studio_sim::dynamics::joint::{Joint, JointKind, Motor, Spring};
use studio_sim::dynamics::math::*;
use studio_sim::dynamics::shape::Shape;
use studio_sim::dynamics::{Body, Collider, Kind, World};

/// A throw or a push on a clip at a moment (scene units: px/s, deg/s).
#[derive(Clone, Debug, Default)]
pub struct Kick {
    pub clip: u64,
    pub at: m::Micros,
    /// Sets its speed.
    pub velocity: Option<V3>,
    /// Sets its spin (deg/s about the scene axes).
    pub spin: Option<V3>,
    /// Adds a sudden change of speed (px/s) at `point` (its middle if none).
    pub shove: Option<V3>,
    pub point: Option<V3>,
}

#[derive(Clone, Debug)]
pub struct Run {
    pub from: m::Micros,
    pub seconds: f32,
    pub fps: f32,
    pub kicks: Vec<Kick>,
    /// Characters knocked loose when something hits them.
    pub react: bool,
    /// Characters that go loose at these moments.
    pub limp: Vec<(u64, m::Micros)>,
    /// How hard a loose character holds its pose at first (0 limp .. 1),
    /// fading over `fade` seconds.
    pub strength: f32,
    pub fade: f32,
    /// Things that break at a moment (and where, scene px).
    pub breaks: Vec<(u64, m::Micros, Option<V3>)>,
    pub pieces: usize,
    /// Stop early once everything has come to rest.
    pub stop_at_rest: bool,
}

impl Run {
    pub fn new(from: m::Micros, seconds: f32, fps: f32) -> Run {
        Run { from, seconds, fps, kicks: Vec::new(), react: true, limp: Vec::new(), strength: 0.6, fade: 0.8, breaks: Vec::new(), pieces: 18, stop_at_rest: false }
    }
}

/// A clip's moves: (time, position px, rotation degrees X Y Z).
#[derive(Clone, Debug)]
pub struct Track {
    pub clip: u64,
    pub samples: Vec<(m::Micros, [f64; 3], [f64; 3])>,
}

/// A character's bone poses from when it went loose.
#[derive(Clone, Debug)]
pub struct BoneTrack {
    pub clip: u64,
    pub from: m::Micros,
    pub frames: Vec<(m::Micros, Vec<BonePose>)>,
}

/// A thing that broke: its pieces and their moves (in the model's frame).
#[derive(Clone, Debug)]
pub struct Broken {
    pub clip: u64,
    pub at: m::Micros,
    pub mesh: Mesh,
    pub ranges: Vec<(u32, u32)>,
    /// Per frame from the run's start: each piece's turn and shift.
    pub frames: Vec<Vec<(Quat, V3)>>,
}

#[derive(Clone, Debug, Default)]
pub struct Baked {
    pub tracks: Vec<Track>,
    pub bones: Vec<BoneTrack>,
    pub broken: Vec<Broken>,
    /// (time, clip, other clip, speed m/s) for hard hits.
    pub impacts: Vec<(m::Micros, u64, Option<u64>, f32)>,
    pub ppm: f32,
    pub fps: f32,
    pub from: m::Micros,
    /// When everything was still (seconds after the start), if it was.
    pub rested_at: Option<f32>,
    pub notes: Vec<String>,
}

/// A joint from its kept settings: on `b`, to `a` (a clip's body or a
/// fixed point in the world).
pub fn make_joint(world: &World, a: usize, b: usize, js: &JointSpec, ppm: f32, b_bounds: &Aabb, a_bounds: Option<Aabb>) -> Option<Joint> {
    let at = match js.at {
        Some(p) => p,
        None => match a_bounds {
            Some(ab) => scale(add(ab.centre(), b_bounds.centre()), 0.5),
            // Hung from the world: the middle of its top.
            None => [b_bounds.centre()[0], b_bounds.lo[1], b_bounds.centre()[2]],
        },
    };
    let at_m = scale(at, 1.0 / ppm);
    let deg = |v: f32| v.to_radians();
    let kind = match js.kind.to_ascii_lowercase().as_str() {
        "hinge" | "revolute" | "pin" | "door" | "axle" => {
            let axis = js.axis.unwrap_or([0.0, 1.0, 0.0]);
            return Some(Joint::at(
                &world.bodies,
                a,
                b,
                at_m,
                axis,
                JointKind::Hinge {
                    limits: js.limits.map(|l| (deg(l[0].min(l[1])), deg(l[0].max(l[1])))),
                    motor: js.motor.map(|mo| Motor { speed: deg(mo[0]), max: mo[1].max(0.0) }),
                    spring: js.spring.map(|s| Spring { target: 0.0, hertz: s[0].max(0.0), damping: s[1].clamp(0.0, 4.0), max: 0.0 }),
                },
            ))
            .map(|mut j| {
                j.break_force = js.breaks.unwrap_or(0.0);
                j
            });
        }
        "ball" | "socket" | "spherical" => JointKind::Ball { cone: js.limits.map(|l| deg(l[1].abs().max(l[0].abs()))), twist: None, drive: None },
        "slider" | "prismatic" | "rail" | "drawer" => JointKind::Slider {
            limits: js.limits.map(|l| (l[0].min(l[1]) / ppm, l[0].max(l[1]) / ppm)),
            motor: js.motor.map(|mo| Motor { speed: mo[0] / ppm, max: mo[1].max(0.0) }),
            spring: js.spring.map(|s| Spring { target: 0.0, hertz: s[0].max(0.0), damping: s[1].clamp(0.0, 4.0), max: 0.0 }),
        },
        "fixed" | "weld" | "glue" => JointKind::Fixed,
        "spring" | "rope" | "rod" | "chain" | "elastic" | "bungee" => {
            // Between the anchor point and the body's middle.
            let pb = scale(b_bounds.centre(), 1.0 / ppm);
            let pa = at_m;
            let len = length(sub(pb, pa));
            let kind = match js.kind.as_str() {
                "rope" | "chain" => JointKind::Spring { rest: len, hertz: 0.01, damping: 0.0, min: 0.0, max: len },
                "rod" => JointKind::Spring { rest: len, hertz: 0.0, damping: 0.0, min: 0.0, max: f32::INFINITY },
                _ => {
                    let s = js.spring.unwrap_or([2.0, 0.3]);
                    JointKind::Spring { rest: len, hertz: s[0].max(0.01), damping: s[1].clamp(0.0, 4.0), min: 0.0, max: f32::INFINITY }
                }
            };
            let mut j = Joint::between(&world.bodies, a, b, pa, pb, kind);
            j.break_force = js.breaks.unwrap_or(0.0);
            return Some(j);
        }
        _ => return None,
    };
    let axis = js.axis.unwrap_or([1.0, 0.0, 0.0]);
    let mut j = Joint::at(&world.bodies, a, b, at_m, axis, kind);
    j.break_force = js.breaks.unwrap_or(0.0);
    Some(j)
}

/// The pieces of a thing about to break, made ready.
struct Shards {
    mesh: Mesh,
    ranges: Vec<(u32, u32)>,
    parts: Vec<Mesh>,
}

fn shards(it: &Item, impact_model: Option<V3>, pieces: usize, seed: u64) -> Option<Shards> {
    if it.mesh.triangles() > 60_000 {
        return None;
    }
    let (mesh, ranges, parts) = studio_sim::fracture::shatter(&it.mesh, &studio_sim::fracture::Shatter { pieces: pieces.clamp(2, 200), impact: impact_model, seed, ..Default::default() });
    (parts.len() >= 2).then_some(Shards { mesh, ranges, parts })
}

struct BreakState {
    item: usize,
    at: m::Micros,
    /// The clip's frame (physics) when it broke.
    frame0: Pose,
    shards: Shards,
    bodies: Vec<usize>,
}

/// Runs the scene and samples it.
pub fn simulate(project: &m::Project, mut scene: Scene, run: &Run) -> Baked {
    let seq = project.sequence(scene.seq).cloned();
    let ppm = scene.ppm;
    let fps = run.fps.clamp(1.0, 240.0);
    let dt = scene.world.settings.dt;
    let steps = (run.seconds.max(0.0) / dt).round() as usize;
    let frame_of_step = |s: usize| s as f32 * dt;
    let mut baked = Baked { ppm, fps, from: run.from, ..Default::default() };
    let clip = |id: u64| seq.as_ref().and_then(|q| q.clip(m::ClipId(id)).cloned());
    // Who is loose, since when.
    let mut loose: Vec<Option<m::Micros>> = vec![None; scene.items.len()];
    let mut broken: Vec<BreakState> = Vec::new();
    let mut tracks: Vec<Track> = Vec::new();
    let mut bone_tracks: Vec<BoneTrack> = Vec::new();
    let mut kicked = vec![false; run.kicks.len()];
    let mut break_done: Vec<bool> = vec![false; run.breaks.len()];
    let movers: Vec<usize> = (0..scene.items.len()).filter(|&i| matches!(scene.items[i].role, Role::Dynamic)).collect();
    for &i in &movers {
        tracks.push(Track { clip: scene.items[i].clip, samples: Vec::new() });
    }
    let mut next_frame = 0usize;
    let mut still_for = 0.0f32;
    let sample = |scene: &Scene, t: m::Micros, tracks: &mut Vec<Track>, bone_tracks: &mut Vec<BoneTrack>, loose: &[Option<m::Micros>], broken: &[BreakState]| {
        for tr in tracks.iter_mut() {
            let Some(i) = scene.item(tr.clip) else { continue };
            // A broken thing stays where it broke.
            let b = broken.iter().find(|bs| bs.item == i);
            let pose = match b {
                Some(bs) => bs.frame0,
                None => scene.world.bodies[scene.items[i].body].model_pose(),
            };
            let (p, r) = super::pose_to_transform(&pose, ppm);
            tr.samples.push((t, p, r));
        }
        for (i, l) in loose.iter().enumerate() {
            let Some(since) = l else { continue };
            let it = &scene.items[i];
            let Role::Character(rd) = &it.role else { continue };
            let Some(c) = clip(it.clip) else { continue };
            let Some(rig) = it.mesh.rig.as_ref() else { continue };
            let anim = posed_rig(&c, rig, t - c.start);
            let locals = rd.locals(&scene.world, rig, &anim);
            match bone_tracks.iter_mut().find(|b| b.clip == it.clip) {
                Some(bt) => bt.frames.push((t, locals)),
                None => bone_tracks.push(BoneTrack { clip: it.clip, from: *since, frames: vec![(t, locals)] }),
            }
        }
    };
    let mut break_frames: Vec<Vec<Vec<(Quat, V3)>>> = Vec::new();
    for s in 0..=steps {
        let t_rel = frame_of_step(s);
        let t_abs = run.from + (t_rel as f64 * 1e6) as m::Micros;
        // Sample frames that are due.
        while (next_frame as f32) / fps <= t_rel + dt * 0.5 {
            let ft = run.from + ((next_frame as f64 / fps as f64) * 1e6).round() as m::Micros;
            sample(&scene, ft, &mut tracks, &mut bone_tracks, &loose, &broken);
            for (k, bs) in broken.iter().enumerate() {
                let mut frame = Vec::with_capacity(bs.shards.parts.len());
                for (pi, &b) in bs.bodies.iter().enumerate() {
                    let _ = pi;
                    frame.push(piece_move(&scene.items[bs.item], &bs.frame0, &scene.world.bodies[b].model_pose(), ppm));
                }
                while break_frames.len() <= k {
                    break_frames.push(Vec::new());
                }
                // Frames before the break hold the pieces at rest.
                while break_frames[k].len() < next_frame {
                    break_frames[k].push(vec![(Q_ID, [0.0; 3]); bs.shards.parts.len()]);
                }
                break_frames[k].push(frame);
            }
            next_frame += 1;
        }
        if s == steps {
            break;
        }
        let t_next = t_abs + (dt as f64 * 1e6) as m::Micros;
        // Animated things go where their keys say.
        for i in 0..scene.items.len() {
            let it = &scene.items[i];
            let Some(c) = clip(it.clip) else { continue };
            match &it.role {
                Role::Kinematic => {
                    let tr = c.transform_at((t_next - c.start).clamp(0, c.duration.max(0)));
                    let goal = frame_of(&tr, ppm).then(&scene.world.bodies[it.body].offset);
                    scene.world.bodies[it.body].target = Some(goal);
                }
                Role::Character(rd) => {
                    let Some(rig) = it.mesh.rig.as_ref() else { continue };
                    // The figure carried by its clip's own moves too.
                    let tr = c.transform_at((t_next - c.start).clamp(0, c.duration.max(0)));
                    let mut rd2 = (**rd).clone();
                    rd2.place = super::scene::character_place(&tr, ppm);
                    let posed = posed_rig(&c, rig, t_next - c.start);
                    match loose[i] {
                        None => rd2.follow(&mut scene.world, &posed),
                        Some(since) => {
                            let gone = (t_abs - since) as f32 / 1e6;
                            let strength = run.strength * (1.0 - gone / run.fade.max(0.05)).max(0.0);
                            rd2.drive(&mut scene.world, &posed, strength);
                        }
                    }
                    if let Role::Character(r) = &mut scene.items[i].role {
                        r.place = rd2.place;
                    }
                }
                _ => {}
            }
        }
        // Throws and pushes.
        for (k, kick) in run.kicks.iter().enumerate() {
            if kicked[k] || kick.at > t_abs {
                continue;
            }
            kicked[k] = true;
            let Some(i) = scene.item(kick.clip) else { continue };
            let bodies: Vec<usize> = match &scene.items[i].role {
                Role::Character(rd) => {
                    loose[i].get_or_insert(t_abs);
                    rd.set_kind(&mut scene.world, Kind::Dynamic);
                    rd.bodies()
                }
                _ => vec![scene.items[i].body],
            };
            for b in bodies {
                let body = &mut scene.world.bodies[b];
                if let Some(v) = kick.velocity {
                    body.v = scale(v, 1.0 / ppm);
                }
                if let Some(w) = kick.spin {
                    body.w = w.map(f32::to_radians);
                }
                if let Some(dv) = kick.shove {
                    let at = kick.point.map_or(body.pose.p, |p| scale(p, 1.0 / ppm));
                    let j = scale(dv, body.mass / ppm);
                    body.apply_impulse(j, at);
                }
                body.wake();
            }
        }
        for (clip_id, at) in &run.limp {
            if *at > t_abs {
                continue;
            }
            if let Some(i) = scene.item(*clip_id)
                && loose[i].is_none()
                && let Role::Character(rd) = &scene.items[i].role
            {
                loose[i] = Some(t_abs);
                rd.set_kind(&mut scene.world, Kind::Dynamic);
            }
        }
        for (k, (clip_id, at, point)) in run.breaks.iter().enumerate() {
            if break_done[k] || *at > t_abs {
                continue;
            }
            break_done[k] = true;
            if let Some(i) = scene.item(*clip_id) {
                start_break(&mut scene, i, t_abs, *point, run, &mut broken, &mut baked.notes, None);
            }
        }
        scene.world.step();
        // Hard hits: characters go loose, breakable things break.
        let impacts = scene.world.impacts.clone();
        enum Then {
            Loose(usize, f32),
            Break(usize, V3, f32),
        }
        let mut then = Vec::new();
        for im in &impacts {
            let ia = scene.item_of.get(im.a).copied().flatten();
            let ib = scene.item_of.get(im.b).copied().flatten();
            let dv = |b: usize| im.impulse / scene.world.bodies[b].mass.max(1e-6);
            if im.speed > 1.0
                && let Some(a) = ia
            {
                baked.impacts.push((t_next, scene.items[a].clip, ib.map(|b| scene.items[b].clip), im.speed));
            }
            for (who, body, other) in [(ia, im.a, im.b), (ib, im.b, im.a)] {
                let Some(i) = who else { continue };
                let other_item = scene.item_of.get(other).copied().flatten();
                let other_moves = scene.world.bodies[other].is_dynamic() || scene.world.bodies[other].kind == Kind::Kinematic;
                // How hard it was hit: its own change of speed, or the
                // speed it met something fixed at.
                let hit = if other_moves { dv(body).max(im.speed * 0.5) } else { im.speed };
                match &scene.items[i].role {
                    // Struck by a moving thing (not the floor under its feet,
                    // not itself).
                    Role::Character(_) if run.react && loose[i].is_none() && scene.world.bodies[other].is_dynamic() && im.speed > 1.5 && other_item != Some(i) => then.push(Then::Loose(i, im.speed)),
                    Role::Dynamic | Role::Static if scene.items[i].breakable && hit > scene.items[i].stuff.breaks_at && !broken.iter().any(|b| b.item == i) => then.push(Then::Break(i, scale(im.at, ppm), hit)),
                    _ => {}
                }
            }
        }
        for th in then {
            match th {
                Then::Loose(i, speed) => {
                    if loose[i].is_some() {
                        continue;
                    }
                    loose[i] = Some(t_next);
                    if let Role::Character(rd) = &scene.items[i].role {
                        rd.set_kind(&mut scene.world, Kind::Dynamic);
                    }
                    baked.notes.push(format!("{} knocked off balance at {:.2} s ({speed:.1} m/s)", scene.items[i].name, (t_next - run.from) as f32 / 1e6));
                }
                Then::Break(i, at, hit) => start_break(&mut scene, i, t_next, Some(at), run, &mut broken, &mut baked.notes, Some(hit)),
            }
        }
        // Everything at rest?
        let moving = scene.world.bodies.iter().any(|b| b.is_dynamic() && b.awake) || scene.items.iter().any(|i| matches!(i.role, Role::Kinematic | Role::Character(_)) && loose_or_keyed(i, &clip));
        if moving {
            still_for = 0.0;
        } else {
            still_for += dt;
            if baked.rested_at.is_none() {
                baked.rested_at = Some(t_rel);
            }
            if run.stop_at_rest && still_for > 0.25 {
                sample(&scene, t_next, &mut tracks, &mut bone_tracks, &loose, &broken);
                break;
            }
        }
        if moving {
            baked.rested_at = None;
        }
    }
    for (k, bs) in broken.into_iter().enumerate() {
        let it = &scene.items[bs.item];
        baked.broken.push(Broken { clip: it.clip, at: bs.at, mesh: bs.shards.mesh, ranges: bs.shards.ranges, frames: break_frames.get(k).cloned().unwrap_or_default() });
    }
    baked.tracks = tracks;
    baked.bones = bone_tracks;
    baked
}

fn loose_or_keyed(it: &Item, clip: &dyn Fn(u64) -> Option<m::Clip>) -> bool {
    clip(it.clip).is_some_and(|c| super::scene::keyed(&c) || super::scene::acts(&c, &it.mesh))
}

/// A piece's move in its model's frame: from the clip's frame at the
/// break to where its body is.
fn piece_move(it: &Item, frame0: &Pose, now: &Pose, ppm: f32) -> (Quat, V3) {
    let tr = &it.transform;
    let k = super::scale_of(tr);
    let ks = ((k[0].abs() + k[1].abs() + k[2].abs()) / 3.0).max(1e-6);
    let d = frame0.inverse().then(now);
    let a = [tr.anchor.x as f32, tr.anchor.y as f32, tr.anchor_z as f32];
    // p' = a + R (p − a) + t·ppm / k
    let shift = add(sub(a, q_rotate(d.q, a)), scale(d.p, ppm / ks));
    (d.q, shift)
}

#[allow(clippy::too_many_arguments)]
fn start_break(scene: &mut Scene, i: usize, at: m::Micros, point: Option<V3>, run: &Run, broken: &mut Vec<BreakState>, notes: &mut Vec<String>, hit: Option<f32>) {
    if broken.iter().any(|b| b.item == i) {
        return;
    }
    let it = scene.items[i].clone();
    let ppm = scene.ppm;
    let body = it.body;
    let frame0 = scene.world.bodies[body].model_pose();
    // The impact point in the model's own coordinates.
    let tr = &it.transform;
    let k = super::scale_of(tr);
    let impact_model = point.map(|p| {
        let local = frame0.unapply(scale(p, 1.0 / ppm));
        [local[0] * ppm / k[0] + tr.anchor.x as f32, local[1] * ppm / k[1] + tr.anchor.y as f32, local[2] * ppm / k[2] + tr.anchor_z as f32]
    });
    let Some(sh) = shards(&it, impact_model, run.pieces, it.clip ^ 0x5eed) else {
        notes.push(format!("{} could not break (not a closed surface, or too detailed)", it.name));
        return;
    };
    // The whole one goes; its pieces take over with its motion.
    let (v, w) = (scene.world.bodies[body].v, scene.world.bodies[body].w);
    {
        let b = &mut scene.world.bodies[body];
        for c in &mut b.colliders {
            c.mask = 0;
            c.group = 0;
        }
        b.set_kind(Kind::Static);
    }
    let centre = point.map_or(frame0.p, |p| scale(p, 1.0 / ppm));
    let burst = hit.unwrap_or(2.0).min(12.0) * 0.25;
    let mut bodies = Vec::new();
    for part in &sh.parts {
        let pts: Vec<V3> = part.positions.iter().map(|p| scale(super::scaled_point(tr, *p), 1.0 / ppm)).collect();
        let Some(h) = Hull::from_points(&pts, 24, 0.0005) else {
            bodies.push(scene.world.add(Body::new(Kind::Static, vec![], frame0)));
            continue;
        };
        let col = Collider::new(Shape::Hull(Arc::new(h)), Pose::IDENTITY, &it.stuff);
        let mut b = Body::new(Kind::Dynamic, vec![col], frame0);
        let c = b.pose.p;
        b.v = add(add(v, cross(w, sub(c, frame0.p))), scale(normalise_or(sub(c, centre), [0.0, -1.0, 0.0]), burst));
        b.w = w;
        b.angular_damping = 0.3;
        let id = scene.world.add(b);
        scene.item_of.push(Some(i));
        bodies.push(id);
    }
    while scene.item_of.len() < scene.world.bodies.len() {
        scene.item_of.push(None);
    }
    notes.push(format!("{} broke into {} pieces at {:.2} s", it.name, bodies.len(), (at - run.from) as f32 / 1e6));
    broken.push(BreakState { item: i, at, frame0, shards: sh, bodies });
}

/// The edits that put a run on the timeline: keys for moved clips, bone
/// keys for loose characters, pieces for broken things.
pub fn bake(project: &m::Project, seq_id: m::SequenceId, baked: &Baked, save_model: &dyn Fn(&Mesh) -> Result<String, String>, save_cache: &dyn Fn(&studio_mesh::cache::Cache) -> Result<String, String>) -> Result<(Vec<m::Edit>, Vec<String>), String> {
    let seq = project.sequence(seq_id).ok_or("no such sequence")?;
    let end = baked.from + ((baked.tracks.iter().flat_map(|t| t.samples.last()).map(|s| s.0 - baked.from).max().unwrap_or(0)) as m::Micros);
    let mut out = Vec::new();
    let mut said = Vec::new();
    let mut changed: std::collections::HashMap<u64, m::Clip> = std::collections::HashMap::new();
    let get = |id: u64, changed: &mut std::collections::HashMap<u64, m::Clip>| -> Option<m::Clip> { changed.get(&id).cloned().or_else(|| seq.clip(m::ClipId(id)).cloned()) };
    for tr in &baked.tracks {
        let Some(mut c) = get(tr.clip, &mut changed) else { continue };
        if tr.samples.len() < 2 {
            continue;
        }
        let moved = tr.samples.iter().any(|s| (0..3).any(|k| (s.1[k] - tr.samples[0].1[k]).abs() > 0.25 || (s.2[k] - tr.samples[0].2[k]).abs() > 0.1));
        if !moved {
            continue;
        }
        let from = tr.samples[0].0;
        let to = tr.samples.last().map_or(from, |s| s.0).max(end);
        let pos: Vec<(m::Micros, [f64; 3])> = tr.samples.iter().map(|s| (s.0, s.1)).collect();
        let rot = unwrap_angles(&tr.samples.iter().map(|s| (s.0, s.2)).collect::<Vec<_>>());
        let pk = crate::agent::physics::reduce(&pos, [0.5, 0.5, 0.5]);
        let rk = crate::agent::physics::reduce(&rot, [0.4, 0.4, 0.4]);
        let start = c.start;
        let _ = to;
        let put = |c: &mut m::Clip, path: &str, keys: Vec<m::Keyframe>| {
            let mut track = c.keyframes.get(path).cloned().unwrap_or_else(|| m::Track::new(Vec::new()));
            // Keys after the run's start would pull it back: the run holds
            // from there on.
            track.keys.retain(|k| k.time < from - start);
            track.keys.extend(keys);
            track.keys.sort_by_key(|k| k.time);
            track.keys.dedup_by_key(|k| k.time);
            c.keyframes.insert(path.to_string(), track);
        };
        let key = |t: m::Micros, v: m::Value| m::Keyframe::new(t - start, v, m::Interp::Linear);
        put(&mut c, "transform.position", pk.iter().map(|&i| key(pos[i].0, m::Value::Point(m::Vec2::new(pos[i].1[0], pos[i].1[1])))).collect());
        put(&mut c, "transform.position_z", pk.iter().map(|&i| key(pos[i].0, m::Value::Scalar(pos[i].1[2]))).collect());
        put(&mut c, "transform.rotation_x", rk.iter().map(|&i| key(rot[i].0, m::Value::Angle(rot[i].1[0]))).collect());
        put(&mut c, "transform.rotation_y", rk.iter().map(|&i| key(rot[i].0, m::Value::Angle(rot[i].1[1]))).collect());
        put(&mut c, "transform.rotation", rk.iter().map(|&i| key(rot[i].0, m::Value::Angle(rot[i].1[2]))).collect());
        c.transform.orientation = [0.0; 3];
        c.transform.three_d = true;
        if let m::ClipSource::Generator(g) = &mut c.source {
            g.params.insert("physics.baked".into(), m::Value::Text(format!("{:.3},{:.3}", from as f64 / 1e6, to as f64 / 1e6)));
        }
        said.push(format!("clip {} ({} position + {} turn keys)", tr.clip, pk.len(), rk.len()));
        changed.insert(tr.clip, c);
    }
    for bt in &baked.bones {
        let Some(mut c) = get(bt.clip, &mut changed) else { continue };
        let Some(mesh) = super::scene::load_mesh(&c) else { continue };
        let Some(rig) = mesh.rig.clone() else { continue };
        let wrote = bone_keys(&mut c, &rig, bt, None);
        // Reaching and looking hand over to the body while it is loose.
        for con in &rig.constraints {
            let path = match con {
                studio_mesh::rig::Constraint::Ik { name, .. } => format!("generator.ik.{name}.on"),
                studio_mesh::rig::Constraint::LookAt { .. } => "generator.look.on".to_string(),
                _ => continue,
            };
            let start = c.start;
            ensure_param(&mut c, &path, m::Value::Scalar(1.0));
            let mut track = c.keyframes.get(&path).cloned().unwrap_or_else(|| m::Track::new(Vec::new()));
            track.keys.retain(|k| k.time < bt.from - start);
            // On until it goes loose.
            if track.keys.is_empty() && bt.from - start > 0 {
                track.keys.push(m::Keyframe::new(0, m::Value::Scalar(1.0), m::Interp::Hold));
            }
            track.keys.push(m::Keyframe::new(bt.from - start, m::Value::Scalar(0.0), m::Interp::Hold));
            c.keyframes.insert(path, track);
        }
        said.push(format!("clip {} goes loose at {:.2} s ({wrote} bones keyed)", bt.clip, bt.from as f64 / 1e6));
        changed.insert(bt.clip, c);
    }
    for br in &baked.broken {
        let Some(mut c) = get(br.clip, &mut changed) else { continue };
        let model = save_model(&br.mesh)?;
        let cache = studio_mesh::cache::Cache { fps: baked.fps, frames: br.frames.len(), data: studio_mesh::cache::Frames::Pieces { ranges: br.ranges.clone(), data: br.frames.iter().flatten().copied().collect() } };
        let file = save_cache(&cache)?;
        crate::mesh_store::set_file(&mut c, model);
        if let m::ClipSource::Generator(g) = &mut c.source {
            g.params.insert("cache".into(), m::Value::Text(file));
            g.params.insert("cache_start".into(), m::Value::Scalar((baked.from - c.start) as f64 / 1e6));
        }
        said.push(format!("clip {} breaks into {} pieces at {:.2} s", br.clip, br.ranges.len(), br.at as f64 / 1e6));
        changed.insert(br.clip, c);
    }
    let mut ids: Vec<u64> = changed.keys().copied().collect();
    ids.sort_unstable();
    for id in ids {
        out.push(m::Edit::SetClip { seq: seq_id, clip: Box::new(changed.remove(&id).expect("kept")) });
    }
    Ok((out, said))
}

/// Angles made continuous (no jump from 179° to −179°).
fn unwrap_angles(s: &[(m::Micros, [f64; 3])]) -> Vec<(m::Micros, [f64; 3])> {
    let mut out: Vec<(m::Micros, [f64; 3])> = Vec::with_capacity(s.len());
    for &(t, a) in s {
        let mut a = a;
        if let Some(prev) = out.last() {
            for k in 0..3 {
                while a[k] - prev.1[k] > 180.0 {
                    a[k] -= 360.0;
                }
                while a[k] - prev.1[k] < -180.0 {
                    a[k] += 360.0;
                }
            }
        }
        out.push((t, a));
    }
    out
}

/// A generator setting that is keyed needs a resting value of its kind.
fn ensure_param(c: &mut m::Clip, path: &str, rest: m::Value) {
    if let Some(name) = path.strip_prefix("generator.")
        && let m::ClipSource::Generator(g) = &mut c.source
        && !g.params.get(name).is_some_and(|p| p.same_kind(&rest))
    {
        g.params.insert(name.to_string(), rest);
    }
}

/// Writes a run of bone poses as bone keys on top of the clip's action
/// (replacing its bone keys from the run's start); `only` limits it to
/// some bones. Returns how many bones were keyed.
pub fn bone_keys(c: &mut m::Clip, rig: &studio_mesh::rig::Rig, bt: &BoneTrack, only: Option<&[usize]>) -> usize {
    let from = bt.from;
    let clip = c.clone();
    // Bone keys are on top of the action: measured from the action's pose.
    let base = |t: m::Micros| -> Vec<BonePose> {
        #[cfg(feature = "render")]
        {
            let params = studio_render::model_scene::convert_params(&clip.generator_params_at((t - clip.start).clamp(0, clip.duration.max(0))));
            let full = studio_render::model_anim::pose_input(&params, (t - clip.start) as f32 / 1e6);
            let only = studio_mesh::rig::PoseInput { action: full.action, blend: full.blend, ..Default::default() };
            rig.evaluate(&only).local
        }
        #[cfg(not(feature = "render"))]
        {
            let _ = t;
            vec![BonePose::default(); rig.bones.len()]
        }
    };
    let frames: Vec<(m::Micros, &Vec<BonePose>, Vec<BonePose>)> = bt.frames.iter().map(|(t, l)| (*t, l, base(*t))).collect();
    if frames.is_empty() {
        return 0;
    }
    let start = c.start;
    let mut wrote = 0;
    for (bi, bone) in rig.bones.iter().enumerate() {
        if !bone.deform || only.is_some_and(|o| !o.contains(&bi)) {
            continue;
        }
        let rot: Vec<(m::Micros, [f64; 3])> = frames
            .iter()
            .map(|(t, l, b)| {
                let extra = q_normalise(q_mul(q_conj(b[bi].rot), l[bi].rot));
                let e = studio_mesh::rig::q_to_euler(extra);
                (*t, [e[0] as f64, e[1] as f64, e[2] as f64])
            })
            .collect();
        let rot = unwrap_angles(&rot);
        let moved = rot.iter().any(|r| (0..3).any(|k| (r.1[k] - rot[0].1[k]).abs() > 0.5 || r.1[k].abs() > 0.5));
        let loc: Vec<(m::Micros, [f64; 3])> = frames.iter().map(|(t, l, b)| (*t, [(l[bi].loc[0] - b[bi].loc[0]) as f64, (l[bi].loc[1] - b[bi].loc[1]) as f64, (l[bi].loc[2] - b[bi].loc[2]) as f64])).collect();
        let shifted = loc.iter().any(|l| (0..3).any(|k| l.1[k].abs() > 0.5));
        if !moved && !shifted {
            continue;
        }
        let rk = crate::agent::physics::reduce(&rot, [0.6, 0.6, 0.6]);
        for (ci, axis) in ["x", "y", "z"].iter().enumerate() {
            let path = format!("generator.bone.{}.{axis}", bone.name);
            ensure_param(c, &path, m::Value::Angle(0.0));
            let mut track = c.keyframes.get(&path).cloned().unwrap_or_else(|| m::Track::new(Vec::new()));
            track.keys.retain(|k| k.time < from - start);
            track.keys.extend(rk.iter().map(|&i| m::Keyframe::new(rot[i].0 - start, m::Value::Angle(rot[i].1[ci]), m::Interp::Linear)));
            track.keys.dedup_by_key(|k| k.time);
            c.keyframes.insert(path, track);
        }
        if shifted {
            let lk = crate::agent::physics::reduce(&loc, [0.5, 0.5, 0.5]);
            for (ci, axis) in ["lx", "ly", "lz"].iter().enumerate() {
                let path = format!("generator.bone.{}.{axis}", bone.name);
                ensure_param(c, &path, m::Value::Scalar(0.0));
                let mut track = c.keyframes.get(&path).cloned().unwrap_or_else(|| m::Track::new(Vec::new()));
                track.keys.retain(|k| k.time < from - start);
                track.keys.extend(lk.iter().map(|&i| m::Keyframe::new(loc[i].0 - start, m::Value::Scalar(loc[i].1[ci]), m::Interp::Linear)));
                track.keys.dedup_by_key(|k| k.time);
                c.keyframes.insert(path, track);
            }
        }
        wrote += 1;
    }
    wrote
}
