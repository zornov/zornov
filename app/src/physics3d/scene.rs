// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A moment of the timeline as a physics world.
//!
//! Every enabled model clip on screen at that moment takes part by its
//! body type: props that are simulated as fitted convex shapes (their
//! mass from their solid volume and material), scenery as its exact
//! triangles, animated clips as kinematic bodies that follow their keys,
//! rigged characters as kinematic ragdolls that follow their animation
//! (and can be knocked loose), a world's terrain and solid objects near
//! the action, and a floor under it all.

use super::{BodyType, Props, clip_pose, scaled_point};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use studio_mesh::Mesh;
use studio_model as m;
use studio_sim::dynamics::decompose::{self, Fit, Shapes};
use studio_sim::dynamics::math::*;
use studio_sim::dynamics::ragdoll::{Placement, Ragdoll, Spec};
use studio_sim::dynamics::shape::{Shape, TriMesh};
use studio_sim::dynamics::{Body, Collider, Kind, World, material};

/// A model clip's mesh, kept by file (files are named by their content).
pub fn load_mesh(c: &m::Clip) -> Option<Arc<Mesh>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Mesh>>>> = OnceLock::new();
    let file = crate::mesh_store::file_of(c)?;
    let cache = CACHE.get_or_init(Default::default);
    if let Some(m) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&file) {
        return Some(m.clone());
    }
    let mesh = Arc::new(crate::mesh_store::load(c).ok()?);
    let mut g = cache.lock().unwrap_or_else(|e| e.into_inner());
    if g.len() > 64 {
        g.clear();
    }
    g.insert(file, mesh.clone());
    Some(mesh)
}

/// The fitted shapes of a model at a size, kept (fitting takes a while,
/// and so does the tree over a big surface). Surfaces are kept up to a
/// few million triangles in all.
fn shapes_cached(file: &str, tris: impl FnOnce() -> Vec<[V3; 3]>, fit: Fit, key: [u32; 7]) -> Option<Arc<Shapes>> {
    type Kept = Mutex<HashMap<(String, [u32; 7], String), Arc<Shapes>>>;
    static FITTED: OnceLock<Kept> = OnceLock::new();
    static SURFACES: OnceLock<Kept> = OnceLock::new();
    let cache = if fit == Fit::Mesh { &SURFACES } else { &FITTED }.get_or_init(Default::default);
    let k = (file.to_string(), key, format!("{fit:?}"));
    if let Some(s) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&k) {
        return Some(s.clone());
    }
    let s = Arc::new(decompose::shapes_for(&tris(), fit, if fit == Fit::Mesh { 0 } else { 32 })?);
    let mut g = cache.lock().unwrap_or_else(|e| e.into_inner());
    let full = if fit == Fit::Mesh { g.values().chain([&s]).map(|s| s.parts.iter().map(|(p, _)| if let Shape::Mesh(m) = p { m.bvh.tris.len() } else { 0 }).sum::<usize>()).sum::<usize>() > 4_000_000 } else { g.len() > 128 };
    if full {
        g.clear();
    }
    g.insert(k, s.clone());
    Some(s)
}

/// How a clip takes part in a run.
#[derive(Clone, Debug)]
pub enum Role {
    Dynamic,
    Static,
    Kinematic,
    /// A rigged character: its ragdoll, following the animation.
    Character(Box<Ragdoll>),
}

/// A clip in the world.
#[derive(Clone, Debug)]
pub struct Item {
    pub clip: u64,
    pub name: String,
    pub role: Role,
    /// Its body (for a character, the first of its ragdoll's).
    pub body: usize,
    pub mesh: Arc<Mesh>,
    pub transform: m::Transform,
    pub start: m::Micros,
    pub stuff: material::Stuff,
    pub breakable: bool,
    /// Where it is (scene px) at the moment the world was made.
    pub bounds: Aabb,
}

/// What floor to put under things.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Floor {
    /// Under the lowest scenery, or the bottom of the frame where the
    /// moving things are when there is none.
    Auto,
    Off,
    /// At this scene y (px).
    At(f32),
}

/// What to build.
#[derive(Clone, Debug)]
pub struct Build {
    pub seq: m::SequenceId,
    pub at: m::Micros,
    /// Clips that move by physics whatever their settings say.
    pub dynamic: Vec<u64>,
    pub floor: Floor,
    /// Leave these out (a clip being placed, say).
    pub skip: Vec<u64>,
    /// Characters as ragdolls (otherwise left out).
    pub characters: bool,
    /// Props set to auto move when they rest on something (a run where
    /// the scene is physical: what is knocked falls); otherwise they stay
    /// put as scenery.
    pub props_move: bool,
}

impl Build {
    pub fn at(seq: m::SequenceId, at: m::Micros) -> Build {
        Build { seq, at, dynamic: Vec::new(), floor: Floor::Auto, skip: Vec::new(), characters: true, props_move: false }
    }
}

pub struct Scene {
    pub world: World,
    pub ppm: f32,
    pub items: Vec<Item>,
    /// The floor's scene y (px), if there is one.
    pub floor: Option<f32>,
    pub at: m::Micros,
    pub seq: m::SequenceId,
    /// Each body's item.
    pub item_of: Vec<Option<usize>>,
}

/// Whether a clip is a model that can take part.
pub fn is_model(c: &m::Clip) -> bool {
    crate::mesh_store::file_of(c).is_some()
}

fn has_cache(c: &m::Clip) -> bool {
    matches!(&c.source, m::ClipSource::Generator(g) if matches!(g.params.get("cache"), Some(m::Value::Text(s)) if !s.is_empty()))
}

/// Whether a clip moves by its own keys.
pub fn keyed(c: &m::Clip) -> bool {
    c.keyframes.iter().any(|(k, t)| k.starts_with("transform.") && !k.ends_with("opacity") && t.keys.len() > 1)
}

/// Whether a rigged model moves its bones (an action or bone keys).
pub fn acts(c: &m::Clip, mesh: &Mesh) -> bool {
    mesh.rig.is_some()
        && (c.keyframes.keys().any(|k| k.starts_with("generator.bone.") || k.starts_with("generator.ik.") || k.starts_with("generator.action"))
            || matches!(&c.source, m::ClipSource::Generator(g) if matches!(g.params.get("action"), Some(m::Value::Text(s)) if !s.is_empty())))
}

/// A clip's model triangles in the physics frame (metres, scaled about
/// its anchor, not yet turned or placed).
pub fn physics_tris(mesh: &Mesh, tr: &m::Transform, ppm: f32) -> Vec<[V3; 3]> {
    let k = super::scale_of(tr);
    let flip = k[0] * k[1] * k[2] < 0.0;
    (0..mesh.triangles())
        .map(|t| {
            let mut tri = mesh.tri(t).map(|v| scale(scaled_point(tr, mesh.positions[v]), 1.0 / ppm));
            if flip {
                tri.swap(1, 2);
            }
            tri
        })
        .collect()
}

/// The clip's physics frame in the world (metres).
pub fn frame_of(tr: &m::Transform, ppm: f32) -> Pose {
    let (q, p) = clip_pose(tr);
    Pose::new(scale(p, 1.0 / ppm), q)
}

/// A rigged clip's pose at clip time `rel` (seconds), in model space.
#[cfg(feature = "render")]
pub fn posed_rig(c: &m::Clip, rig: &studio_mesh::rig::Rig, rel: m::Micros) -> studio_mesh::rig::Posed {
    let params = studio_render::model_scene::convert_params(&c.generator_params_at(rel.clamp(0, c.duration.max(0))));
    let input = studio_render::model_anim::pose_input(&params, rel as f32 / 1e6);
    rig.evaluate(&input)
}

#[cfg(not(feature = "render"))]
pub fn posed_rig(_c: &m::Clip, rig: &studio_mesh::rig::Rig, _rel: m::Micros) -> studio_mesh::rig::Posed {
    rig.evaluate(&Default::default())
}

/// The ragdoll placement of a character clip at a moment.
pub fn character_place(tr: &m::Transform, ppm: f32) -> Placement {
    let k = super::scale_of(tr);
    let ks = (k[0].abs() + k[1].abs() + k[2].abs()) / 3.0;
    let anchor = [tr.anchor.x as f32, tr.anchor.y as f32, tr.anchor_z as f32];
    let frame = frame_of(tr, ppm);
    Placement { place: frame.then(&Pose::at(scale(anchor, -ks / ppm))), k: ks / ppm }
}

/// Bounds of a clip's model in the scene (px) at a transform.
pub fn scene_bounds(mesh: &Mesh, tr: &m::Transform) -> Aabb {
    let (q, p) = clip_pose(tr);
    let pose = Pose::new(p, q);
    let mut b = Aabb::EMPTY;
    for v in &mesh.positions {
        b.grow(pose.apply(scaled_point(tr, *v)));
    }
    b
}

impl Scene {
    pub fn build(project: &m::Project, b: &Build) -> Result<Scene, String> {
        let seq = project.sequence(b.seq).ok_or("no such sequence")?;
        let ppm = super::px_per_metre(project);
        let settings = super::World::of(project);
        let mut world = World::new([0.0, settings.gravity, 0.0]);
        let mut items: Vec<Item> = Vec::new();
        let t = b.at;
        let mut worlds = Vec::new();
        // Props that may move, decided once the floor is in.
        let mut maybe: Vec<usize> = Vec::new();
        for c in &seq.clips {
            if !c.enabled || t < c.start || t >= c.start + c.duration.max(1) || b.skip.contains(&c.id.0) {
                continue;
            }
            if let m::ClipSource::Generator(g) = &c.source
                && let (Some(m::Value::Text(file)), Some(m::Value::Scalar(mpp))) = (g.params.get("world").or(g.params.get("file")), g.params.get("metres_per_px"))
                && !is_model(c)
            {
                worlds.push((file.clone(), *mpp as f32));
                continue;
            }
            if !is_model(c) {
                continue;
            }
            let Some(mesh) = load_mesh(c) else { continue };
            if mesh.volume.is_some() || mesh.triangles() == 0 {
                continue;
            }
            let props = Props::of(c);
            let forced = b.dynamic.contains(&c.id.0);
            let rel = (t - c.start).clamp(0, c.duration.max(0));
            let tr = c.transform_at(rel);
            let rigged = mesh.rig.as_ref().is_some_and(|r| r.is_skinned() && r.bones.len() > 3);
            if rigged && !b.characters && !forced {
                continue;
            }
            let role = match (forced, props.body) {
                (true, _) => BodyType::Dynamic,
                (_, BodyType::None) => continue,
                (_, BodyType::Auto) if has_cache(c) => continue,
                (_, BodyType::Auto) if rigged && b.characters => BodyType::Kinematic,
                (_, BodyType::Auto) if keyed(c) => BodyType::Kinematic,
                (_, BodyType::Auto) if b.props_move && !super::has_word(&c.name, super::GROUNDS) && !super::has_word(&c.name, super::AIRBORNE) => {
                    maybe.push(items.len());
                    BodyType::Dynamic
                }
                (_, BodyType::Auto) => BodyType::Static,
                (_, other) => other,
            };
            let stuff = props.stuff(Some(&mesh), &c.name);
            let bounds = scene_bounds(&mesh, &tr);
            let frame = frame_of(&tr, ppm);
            let item_role;
            let body;
            if rigged && role != BodyType::Static && b.characters && !forced {
                // A character: a ragdoll following its animation.
                let rig = mesh.rig.clone().expect("rigged");
                let posed = posed_rig(c, &rig, rel);
                let place = character_place(&tr, ppm);
                let flesh = props.material.as_deref().and_then(material::named).unwrap_or_else(|| material::named("flesh").expect("flesh"));
                let rd = Ragdoll::build(&mut world, &rig, &mesh, &posed, place, &Spec { stuff: flesh, min_bone: 0.05, kind: Kind::Kinematic });
                body = rd.bodies().first().copied().unwrap_or(0);
                item_role = Role::Character(Box::new(rd));
            } else {
                let file = crate::mesh_store::file_of(c).unwrap_or_default();
                let fit = match (role, props.fit) {
                    (BodyType::Static | BodyType::Kinematic, Fit::Auto) => Fit::Mesh,
                    (_, f) => f,
                };
                // Everything the triangles are made from (see physics_tris).
                let k = super::scale_of(&tr);
                let key = [ppm, k[0], k[1], k[2], tr.anchor.x as f32, tr.anchor.y as f32, tr.anchor_z as f32].map(f32::to_bits);
                let shapes = shapes_cached(&file, || physics_tris(&mesh, &tr, ppm), fit, key);
                let Some(shapes) = shapes else { continue };
                let mut cols: Vec<Collider> = shapes.parts.iter().map(|(s, p)| Collider::new(s.clone(), *p, &stuff)).collect();
                if let Some(mass) = props.mass
                    && role == BodyType::Dynamic
                {
                    let vol: f32 = cols.iter().map(|c| c.shape.volume()).sum();
                    if vol > 1e-9 {
                        for col in &mut cols {
                            col.density = mass / vol;
                        }
                    }
                }
                let kind = match role {
                    BodyType::Dynamic => Kind::Dynamic,
                    BodyType::Kinematic => Kind::Kinematic,
                    _ => Kind::Static,
                };
                body = world.add(Body::new(kind, cols, frame));
                item_role = match kind {
                    Kind::Dynamic => Role::Dynamic,
                    Kind::Kinematic => Role::Kinematic,
                    Kind::Static => Role::Static,
                };
            }
            items.push(Item { clip: c.id.0, name: c.name.clone(), role: item_role, body, mesh, transform: tr, start: c.start, stuff, breakable: props.is_breakable(&stuff), bounds });
        }
        // Worlds: their terrain and solid things near the action.
        let action = items.iter().filter(|i| !matches!(i.role, Role::Static)).fold(Aabb::EMPTY, |a, i| a.union(&i.bounds));
        for (file, mpp) in worlds {
            add_world(&mut world, &file, mpp, ppm, &action);
        }
        // The floor.
        let floor = match b.floor {
            Floor::Off => None,
            Floor::At(y) => Some(y),
            Floor::Auto => {
                let statics: Vec<&Item> = items.iter().filter(|i| matches!(i.role, Role::Static)).collect();
                if !statics.is_empty() {
                    let lowest = items.iter().map(|i| i.bounds.hi[1]).fold(f32::NEG_INFINITY, f32::max);
                    Some(lowest)
                } else {
                    // Nothing to stand on: the bottom of the frame where the
                    // moving things are (or under them, if lower).
                    let movers: Vec<&Item> = items.iter().filter(|i| !matches!(i.role, Role::Static)).collect();
                    if movers.is_empty() {
                        None
                    } else {
                        let z = movers.iter().map(|i| i.bounds.centre()[2]).sum::<f32>() / movers.len() as f32;
                        let lowest = movers.iter().map(|i| i.bounds.hi[1]).fold(f32::NEG_INFINITY, f32::max);
                        Some(frame_bottom(project, seq, t, z).max(lowest))
                    }
                }
            }
        };
        if let Some(y) = floor {
            let ground = material::named("concrete").expect("concrete");
            world.add(Body::new(Kind::Static, vec![Collider::new(Shape::Plane { n: [0.0, -1.0, 0.0], d: -y / ppm }, Pose::IDENTITY, &ground)], Pose::IDENTITY));
        }
        // Props that rest on something move; ones in the air (meant to
        // float there) or as big as the scene stay put.
        let scene_box = items.iter().fold(Aabb::EMPTY, |a, i| a.union(&i.bounds));
        let scene_size = length(scene_box.size());
        let down = normalise_or(world.gravity, [0.0, 1.0, 0.0]);
        for k in maybe {
            let it = &items[k];
            let size = length(it.bounds.size());
            let tol = ((it.bounds.hi[1] - it.bounds.lo[1]) * 0.03).max(2.0) / ppm;
            let supported = world.contacts_of(it.body, tol).iter().any(|(_, mf)| dot(mf.normal, down) > 0.5 && mf.points.iter().any(|p| p.sep < tol));
            if !supported || (items.len() > 2 && size > scene_size * 0.6) {
                world.bodies[it.body].set_kind(Kind::Static);
                items[k].role = Role::Static;
            }
        }
        // Joints kept on the clips.
        let ids: HashMap<u64, usize> = items.iter().enumerate().map(|(k, i)| (i.clip, k)).collect();
        for (k, it) in items.iter().enumerate() {
            let Some(c) = seq.clip(m::ClipId(it.clip)) else { continue };
            for js in Props::of(c).joints {
                let other = js.to.and_then(|o| ids.get(&o).copied());
                if js.to.is_some() && other.is_none() {
                    continue;
                }
                let anchor_body = match other {
                    Some(o) => items[o].body,
                    None => world.add(Body::new(Kind::Static, vec![], Pose::IDENTITY)),
                };
                if let Some(j) = super::run::make_joint(&world, anchor_body, it.body, &js, ppm, &items[k].bounds, other.map(|o| items[o].bounds)) {
                    world.join(j);
                }
            }
        }
        let mut item_of = vec![None; world.bodies.len()];
        for (k, it) in items.iter().enumerate() {
            match &it.role {
                Role::Character(rd) => {
                    for b in rd.bodies() {
                        item_of[b] = Some(k);
                    }
                }
                _ => item_of[it.body] = Some(k),
            }
        }
        Ok(Scene { world, ppm, items, floor, at: t, seq: b.seq, item_of })
    }

    pub fn item(&self, clip: u64) -> Option<usize> {
        self.items.iter().position(|i| i.clip == clip)
    }
}

/// The scene y of the bottom edge of the frame at depth `z`.
fn frame_bottom(project: &m::Project, seq: &m::Sequence, t: m::Micros, z: f32) -> f32 {
    let w = project.settings.width as f64;
    let h = project.settings.height as f64;
    let view = studio_model::space::view_at(seq, t, w);
    let fwd = view.axes[2];
    let down = view.axes[1];
    // Distance along the view to the depth plane.
    let d = if fwd[2].abs() > 1e-6 { (z as f64 - view.position[2]) / fwd[2] } else { w * 1.4 };
    let d = d.max(1.0);
    let bottom = [view.position[0] + fwd[0] * d + down[0] * d * h / 2.0 / view.zoom, view.position[1] + fwd[1] * d + down[1] * d * h / 2.0 / view.zoom];
    bottom[1] as f32
}

/// A world's terrain (as triangles) and its solid objects near `around`
/// (scene px), in physics metres.
fn add_world(world: &mut World, file: &str, mpp: f32, ppm: f32, around: &Aabb) {
    #[cfg(feature = "render")]
    {
        let Some(w) = studio_render::world_scene::load(file) else { return };
        // World metres (y up) to physics metres (y down, the scene's scale).
        let k = 1.0 / (mpp * ppm);
        let to = |p: [f32; 3]| [p[0] * k, -p[1] * k, p[2] * k];
        let region = if around.is_empty() { Aabb { lo: [-2000.0; 3], hi: [2000.0; 3] } } else { around.fattened(around.size().iter().copied().fold(400.0, f32::max)) };
        // The region in world metres.
        let (x0, x1) = (region.lo[0] * mpp, region.hi[0] * mpp);
        let (z0, z1) = (region.lo[2] * mpp, region.hi[2] * mpp);
        let stone = material::named("dirt").expect("dirt");
        if let Some(terrain) = &w.terrain {
            let n = 48usize;
            let mut tris = Vec::with_capacity(n * n * 2);
            let at = |i: usize, j: usize| {
                let x = x0 + (x1 - x0) * i as f32 / n as f32;
                let z = z0 + (z1 - z0) * j as f32 / n as f32;
                to([x, terrain.height_at(x, z), z])
            };
            for i in 0..n {
                for j in 0..n {
                    let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
                    tris.push([a, b, c]);
                    tris.push([a, c, d]);
                }
            }
            world.add(Body::new(Kind::Static, vec![Collider::new(Shape::Mesh(Arc::new(TriMesh::new(tris))), Pose::IDENTITY, &stone)], Pose::IDENTITY));
        }
        for o in &w.objects {
            if !o.collide || o.pos_outside(x0, x1, z0, z1) {
                continue;
            }
            let a = &w.assets[o.asset];
            let (s, c) = o.inst.yaw.to_radians().sin_cos();
            let turn = |p: [f32; 3]| {
                let p = [p[0] * o.inst.scale, p[1] * o.inst.scale, p[2] * o.inst.scale];
                to([o.inst.pos[0] + p[0] * c + p[2] * s, o.inst.pos[1] + p[1], o.inst.pos[2] - p[0] * s + p[2] * c])
            };
            let tris: Vec<[V3; 3]> = match &a.collider {
                studio_world::asset::ColliderShape::Mesh(bvh) => bvh.tris.iter().map(|t| t.map(turn)).collect(),
                studio_world::asset::ColliderShape::Box(bx) => box_tris(bx.min, bx.max).into_iter().map(|t| t.map(turn)).collect(),
                studio_world::asset::ColliderShape::Cylinder { radius, height } => box_tris([-radius, 0.0, -radius], [*radius, *height, *radius]).into_iter().map(|t| t.map(turn)).collect(),
                studio_world::asset::ColliderShape::None => continue,
            };
            if !tris.is_empty() {
                world.add(Body::new(Kind::Static, vec![Collider::new(Shape::Mesh(Arc::new(TriMesh::new(tris))), Pose::IDENTITY, &stone)], Pose::IDENTITY));
            }
        }
    }
    #[cfg(not(feature = "render"))]
    {
        let _ = (world, file, mpp, ppm, around);
    }
}

trait Outside {
    fn pos_outside(&self, x0: f32, x1: f32, z0: f32, z1: f32) -> bool;
}

impl Outside for studio_world::world::Placed {
    fn pos_outside(&self, x0: f32, x1: f32, z0: f32, z1: f32) -> bool {
        let p = self.inst.pos;
        p[0] < x0 || p[0] > x1 || p[2] < z0 || p[2] > z1
    }
}

fn box_tris(lo: [f32; 3], hi: [f32; 3]) -> Vec<[V3; 3]> {
    let c = |i: usize| [if i & 1 == 0 { lo[0] } else { hi[0] }, if i & 2 == 0 { lo[1] } else { hi[1] }, if i & 4 == 0 { lo[2] } else { hi[2] }];
    let quads = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
    quads.iter().flat_map(|q| [[c(q[0]), c(q[1]), c(q[2])], [c(q[0]), c(q[2]), c(q[3])]]).collect()
}
