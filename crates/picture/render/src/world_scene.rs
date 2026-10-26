// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A walkable world as a layer of the timeline: drawn through the scene's
//! camera by the real-time renderer (on its own GPU device), by the CPU
//! rasteriser when there is no GPU, or by the path tracer for final
//! quality, from the same camera and lights.
//!
//! The scene's camera is in Studio's pixels (y down); the world is in
//! metres (y up), `metres_per_px` apart, so the cameras, keyframes and
//! tools of ordinary 3D layers move through a world unchanged. A world
//! lives in a `.sworld` file (its [`WorldSpec`] as JSON), built once and
//! kept.

use crate::frame::Frame;
use crate::mesh_render::{MaterialOverride, MeshDesc, MeshInstance};
use crate::realtime::{Options, Quality, Renderer, frame::View};
use crate::scene::Canvas;
use crate::space::{CameraDesc, LightDesc, LightKind};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use studio_mesh::{Material, Mesh, Texture};
use studio_world::asset::mesh_bounds;
use studio_world::build::{NoLibrary, WorldSpec};
use studio_world::cull::{CullParams, cull};
use studio_world::math::{self, Frustum, V3, mat_mul};
use studio_world::world::World;

/// Metres per Studio pixel for worlds that do not say.
pub const DEFAULT_METRES_PER_PX: f32 = 0.01;

/// A world layer at one instant.
#[derive(Clone)]
pub struct WorldDesc {
    pub world: Arc<World>,
    /// The scene camera (pixels); filled in when the frame is resolved.
    pub camera: Option<CameraDesc>,
    /// Seconds into the layer (characters act, wind and water move).
    pub time: f32,
    /// An hour of the day to light it for, instead of the world's own.
    pub time_of_day: Option<f32>,
    pub metres_per_px: f32,
    pub quality: Quality,
    /// The scene's air and look from the project's light clips, filled in
    /// when the frame is resolved: the fog (density, thinning, colour) and
    /// the exposure, bloom, saturation and vignette reach the world's own
    /// renderer; the CPU and path-traced render of a world get all of it
    /// (weather, wet reflections, tone map, grade). The world keeps its own
    /// sun, sky and lamps.
    pub carrier: Option<crate::atmosphere::Atmos>,
}

impl PartialEq for WorldDesc {
    fn eq(&self, o: &WorldDesc) -> bool {
        Arc::ptr_eq(&self.world, &o.world) && self.camera == o.camera && self.time.to_bits() == o.time.to_bits() && self.time_of_day.map(f32::to_bits) == o.time_of_day.map(f32::to_bits) && self.metres_per_px.to_bits() == o.metres_per_px.to_bits() && self.quality == o.quality && self.carrier == o.carrier
    }
}

impl std::fmt::Debug for WorldDesc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "World({}, t {:.2}{})", self.world.name, self.time, self.carrier.map(|a| format!(", air {a:?}")).unwrap_or_default())
    }
}

fn worlds() -> &'static Mutex<(HashMap<String, Arc<World>>, Vec<String>)> {
    static W: OnceLock<Mutex<(HashMap<String, Arc<World>>, Vec<String>)>> = OnceLock::new();
    W.get_or_init(|| Mutex::new((HashMap::new(), Vec::new())))
}

/// A world by its file (built once; files are named by their content).
pub fn load(path: &str) -> Option<Arc<World>> {
    if let Some(w) = worlds().lock().unwrap_or_else(|e| e.into_inner()).0.get(path) {
        return Some(w.clone());
    }
    let text = std::fs::read_to_string(path).ok()?;
    let spec: WorldSpec = serde_json::from_str(&text).ok()?;
    let world = Arc::new(studio_world::build::build(&spec, &studio_world::build::FileLibrary).ok()?);
    remember(path, world.clone());
    Some(world)
}

/// Keeps a world under a name (one being edited, not yet saved).
pub fn remember(name: &str, world: Arc<World>) {
    let mut g = worlds().lock().unwrap_or_else(|e| e.into_inner());
    if g.0.insert(name.to_string(), world).is_none() {
        g.1.push(name.to_string());
    }
    if g.1.len() > 6 {
        let old = g.1.remove(0);
        g.0.remove(&old);
    }
}

/// The scene camera as the world's view: metres, yaw and pitch, and a
/// vertical field of view from the lens and the canvas.
pub fn view_from_camera(c: &CameraDesc, canvas: &Canvas, metres_per_px: f32, far: f32) -> View {
    let k = metres_per_px;
    let to_world = |v: V3| [v[0], -v[1], v[2]];
    let right = to_world(c.axes[0]);
    let down = to_world(c.axes[1]);
    let fwd = to_world(c.axes[2]);
    let yaw = fwd[0].atan2(fwd[2]).to_degrees();
    let pitch = fwd[1].clamp(-1.0, 1.0).asin().to_degrees();
    // Roll: where the camera's right lies against the level right.
    let level_right = math::norm(math::cross([0.0, 1.0, 0.0], fwd));
    let level_up = math::cross(fwd, level_right);
    let up = math::mul(down, -1.0);
    let roll = (math::dot(right, level_up)).atan2(math::dot(right, level_right)).to_degrees();
    let _ = up;
    let fov = 2.0 * ((canvas.height as f32 * 0.5) / c.zoom.max(1e-3)).atan().to_degrees();
    View { position: [c.position[0] * k, -c.position[1] * k, c.position[2] * k], yaw, pitch, roll: -roll, fov: fov.clamp(5.0, 150.0), near: (0.1_f32).max(k * 20.0), far }
}

fn renderer() -> &'static Mutex<Option<Renderer>> {
    static R: OnceLock<Mutex<Option<Renderer>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(crate::gpu::GpuContext::new().and_then(|c| Renderer::new(c).ok())))
}

/// Draws the world layer at `target` pixels (the canvas at that scale).
pub fn render(desc: &WorldDesc, canvas: &Canvas, target: (u32, u32)) -> Frame {
    let (tw, th) = (target.0.max(16), target.1.max(16));
    let camera = desc.camera.unwrap_or_else(|| CameraDesc::default_for(canvas));
    let b = desc.world.bounds();
    let far = (math::len(math::sub(b.max, b.min)) * 1.2).clamp(300.0, 3000.0);
    if camera.trace.is_none() {
        let view = view_from_camera(&camera, canvas, desc.metres_per_px, far);
        let mut o = Options::new(tw, th);
        o.quality = desc.quality;
        o.time = desc.time;
        o.sky_time = desc.time_of_day;
        if let Some(a) = &desc.carrier {
            (o.fog, o.look) = shared_with_realtime(a, &desc.world.look);
        }
        let mut g = renderer().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(r) = g.as_mut()
            && let Ok((f, _)) = r.render_linear(&desc.world, &view, &o)
        {
            return f;
        }
    }
    // Path tracing, or no GPU: the world as triangles through the CPU.
    let mesh = to_mesh_desc(desc, canvas, &camera, 3_000_000);
    crate::mesh_render::render(&mesh, canvas, (tw, th))
}

/// What of a project's scene air and look the world's own renderer takes:
/// the fog and the look (exposure, bloom, saturation, vignette), over the
/// world's, with its own occlusion kept.
pub fn shared_with_realtime(a: &crate::atmosphere::Atmos, own: &studio_world::world::Look) -> (Option<(f32, f32, Option<[f32; 3]>)>, Option<[f32; 5]>) {
    let fog = a.fog.map(|f| (f.density, f.falloff, Some(f.colour)));
    let g = a.grade;
    let look = [own.exposure + g.exposure, if a.bloom.strength > 0.0 { a.bloom.strength.min(0.6) * 1.2 } else { own.bloom }, own.ssao, own.saturation * g.saturation, if g.vignette > 0.0 { g.vignette } else { own.vignette }];
    (fog, Some(look))
}

/// A cache of a world's assets as Studio models (pixels, y down).
fn px_meshes() -> &'static Mutex<HashMap<(u64, usize), Arc<Mesh>>> {
    static M: OnceLock<Mutex<HashMap<(u64, usize), Arc<Mesh>>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A mesh in metres (y up) as a Studio model at `px_per_m`.
pub fn mesh_to_studio(m: &Mesh, px_per_m: f32) -> Mesh {
    let mut out = m.clone();
    for p in out.positions.iter_mut() {
        *p = [p[0] * px_per_m, -p[1] * px_per_m, p[2] * px_per_m];
    }
    for n in out.normals.iter_mut() {
        *n = [n[0], -n[1], n[2]];
    }
    for t in out.indices.chunks_mut(3) {
        t.swap(1, 2);
    }
    out
}

fn asset_mesh(world: &World, asset: usize, lod: usize, px_per_m: f32) -> Arc<Mesh> {
    let a = &world.assets[asset];
    let key = (a.key ^ px_per_m.to_bits() as u64, lod);
    if let Some(m) = px_meshes().lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return m.clone();
    }
    let m = Arc::new(mesh_to_studio(&a.lods[lod.min(a.lods.len() - 1)], px_per_m));
    let mut g = px_meshes().lock().unwrap_or_else(|e| e.into_inner());
    if g.len() > 600 {
        g.clear();
    }
    g.insert(key, m.clone());
    m
}

/// The ground's colours as a picture over the whole terrain.
pub fn terrain_texture(t: &studio_world::terrain::Terrain) -> Texture {
    let res = (t.cells[0].max(t.cells[1]) as u32).clamp(256, 2048);
    let size = t.size();
    let to8 = |v: f32| {
        let c = v.clamp(0.0, 1.0);
        let s = if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
        (s * 255.0).round() as u8
    };
    let mut rgba = Vec::with_capacity((res * res * 4) as usize);
    for py in 0..res {
        for px in 0..res {
            let x = t.origin[0] + (px as f32 + 0.5) / res as f32 * size[0];
            let z = t.origin[1] + (py as f32 + 0.5) / res as f32 * size[1];
            let w = t.weights_at(x, z);
            let n = 0.88 + 0.24 * studio_world::noise::fbm(x * 0.4, z * 0.4, 2, 7) * 0.5 + 0.12;
            let mut c = [0.0f32; 3];
            for k in 0..4 {
                for ch in 0..3 {
                    c[ch] += t.layers[k].colour[ch] * w[k];
                }
            }
            rgba.extend_from_slice(&[to8(c[0] * n), to8(c[1] * n), to8(c[2] * n), 255]);
        }
    }
    Texture { width: res, height: res, rgba }
}

/// The world as models, lights and a camera for the CPU rasteriser and the
/// path tracer: what the camera can see, at levels of detail by distance,
/// thinned until it fits `budget` triangles.
pub fn to_mesh_desc(desc: &WorldDesc, canvas: &Canvas, camera: &CameraDesc, budget: usize) -> MeshDesc {
    let w = &*desc.world;
    let px = 1.0 / desc.metres_per_px.max(1e-6);
    let b = w.bounds();
    let far = (math::len(math::sub(b.max, b.min)) * 1.2).clamp(300.0, 3000.0);
    let view = view_from_camera(camera, canvas, desc.metres_per_px, far);
    let aspect = canvas.width as f32 / canvas.height.max(1) as f32;
    let vp = mat_mul(&view.proj_matrix(aspect, [0.0; 2]), &view.view_matrix());
    let frustum = Frustum::from_view_proj(&vp);
    let mut bias = 1.0f32;
    let mut instances: Vec<MeshInstance> = Vec::new();
    for _ in 0..5 {
        instances.clear();
        let list = cull(w, &CullParams { eye: view.position, frustum, lod_bias: bias, density: bias.min(1.0), terrain_lod: [60.0 * bias, 150.0 * bias, 350.0 * bias], time: desc.time });
        let mut tris = 0usize;
        let mut id = 1u64 << 40;
        // Terrain: chunks drawn as models with one picture over the land.
        if let Some(t) = &w.terrain {
            let tex = Arc::new(terrain_texture(t));
            let size = t.size();
            for (i, j, lod) in &list.chunks {
                let cm = t.chunk_mesh(*i, *j, *lod);
                let mut m = Mesh { positions: cm.positions.iter().map(|p| [p[0] * px, -p[1] * px, p[2] * px]).collect(), normals: cm.normals.iter().map(|n| [n[0], -n[1], n[2]]).collect(), uvs: cm.positions.iter().map(|p| [(p[0] - t.origin[0]) / size[0], (p[2] - t.origin[1]) / size[1]]).collect(), indices: Vec::new(), ..Default::default() };
                for tri in cm.indices.chunks(3) {
                    m.indices.extend_from_slice(&[tri[0], tri[2], tri[1]]);
                }
                m.materials = vec![Material { name: "Ground".into(), base_colour: [1.0; 4], roughness: 0.92, texture: Some((*tex).clone()), ..Material::default() }];
                m.single_group();
                tris += m.triangles();
                id += 1;
                instances.push(MeshInstance { id, mesh: Arc::new(m), key: format!("chunk {i} {j} {lod}"), world: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]], overrides: MaterialOverride::default(), smooth: 1.0, accepts_lights: true, casts_shadows: true, accepts_shadows: true, motion: None });
            }
        }
        for batch in &list.batches {
            let m = asset_mesh(w, batch.asset, batch.lod, px);
            for raw in &batch.instances {
                let (pos, s, sn, cs) = ([raw[0], raw[1], raw[2]], raw[3], raw[4], raw[5]);
                id += 1;
                tris += m.triangles();
                instances.push(MeshInstance {
                    id,
                    mesh: m.clone(),
                    key: format!("a{} {}", w.assets[batch.asset].key, batch.lod),
                    world: [[s * cs, 0.0, s * sn, pos[0] * px], [0.0, s, 0.0, -pos[1] * px], [-s * sn, 0.0, s * cs, pos[2] * px]],
                    overrides: MaterialOverride::default(),
                    smooth: 1.0,
                    accepts_lights: true,
                    casts_shadows: true,
                    accepts_shadows: true,
                    motion: None,
                });
            }
        }
        // Characters, posed for this moment.
        for pf in &list.performers {
            let a = &w.assets[w.objects[pf.object].asset];
            let posed = studio_world::character::posed(a, &pf.clip, pf.clip_time).unwrap_or_else(|| (*a.lods[0]).clone());
            let m = Arc::new(mesh_to_studio(&posed, px));
            let (pos, s, sn, cs) = (pf.inst.pos, pf.inst.scale, pf.inst.yaw.to_radians().sin(), pf.inst.yaw.to_radians().cos());
            id += 1;
            tris += m.triangles();
            instances.push(MeshInstance {
                id,
                mesh: m,
                key: format!("performer {} {} {}", a.key, pf.clip, (pf.clip_time * 24.0).round()),
                world: [[s * cs, 0.0, s * sn, pos[0] * px], [0.0, s, 0.0, -pos[1] * px], [-s * sn, 0.0, s * cs, pos[2] * px]],
                overrides: MaterialOverride::default(),
                smooth: 1.0,
                accepts_lights: true,
                casts_shadows: true,
                accepts_shadows: true,
                motion: None,
            });
        }
        // Water: a thin clear slab at each level.
        for wt in w.water.iter().filter(|wt| frustum.aabb(&math::Aabb::new([wt.min[0], wt.level - 0.5, wt.min[1]], [wt.max[0], wt.level + 0.5, wt.max[1]]))) {
            let (x0, x1, z0, z1) = (wt.min[0].max(b.min[0] - 300.0), wt.max[0].min(b.max[0] + 300.0), wt.min[1].max(b.min[2] - 300.0), wt.max[1].min(b.max[2] + 300.0));
            let y = -wt.level * px;
            let mut m = Mesh { positions: vec![[x0 * px, y, z0 * px], [x1 * px, y, z0 * px], [x1 * px, y, z1 * px], [x0 * px, y, z1 * px]], normals: vec![[0.0, -1.0, 0.0]; 4], uvs: vec![[0.0; 2]; 4], indices: vec![0, 2, 1, 0, 3, 2], ..Default::default() };
            m.materials = vec![Material { name: "Water".into(), base_colour: [wt.colour[0].max(0.05), wt.colour[1].max(0.2), wt.colour[2].max(0.25), 1.0], roughness: 0.04, transmission: 1.0, ior: 1.33, ..Material::default() }];
            m.single_group();
            id += 1;
            instances.push(MeshInstance { id, mesh: Arc::new(m), key: "water".into(), world: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]], overrides: MaterialOverride::default(), smooth: 1.0, accepts_lights: true, casts_shadows: false, accepts_shadows: true, motion: None });
        }
        if tris <= budget {
            break;
        }
        bias *= 0.6;
    }
    // Lights: the sun and sky, and the lamps.
    let mut sky = w.sky.clone();
    if let Some(t) = desc.time_of_day {
        sky.time = t.rem_euclid(24.0);
    }
    let sd = crate::realtime::frame::sky_desc(&sky);
    let mut lights = vec![LightDesc {
        kind: LightKind::Parallel,
        position: [0.0; 3],
        direction: sd.sun,
        colour: [1.0; 3],
        cone: [0.0, 1.0],
        radius: 0.0,
        falloff: 0.0,
        casts_shadows: true,
        shadow_darkness: 0.9,
        sky: Some(crate::sky::SkyDesc { background: true, ..sd }),
        look: None,
    }];
    for l in w.lights.iter().filter(|l| math::dist(l.position, view.position) < 150.0).take(8) {
        lights.push(LightDesc {
            kind: LightKind::Point,
            position: [l.position[0] * px, -l.position[1] * px, l.position[2] * px],
            direction: [0.0, 1.0, 0.0],
            colour: [(l.colour[0] / (l.range * l.range * 0.35)).min(8.0), (l.colour[1] / (l.range * l.range * 0.35)).min(8.0), (l.colour[2] / (l.range * l.range * 0.35)).min(8.0)],
            cone: [0.0, 1.0],
            radius: l.range * 0.3 * px,
            falloff: l.range * 0.7 * px,
            casts_shadows: false,
            shadow_darkness: 0.0,
            sky: None,
            look: None,
        });
    }
    // The project's air and look, in the world's pixels: the camera is at
    // the world's own height, so the ground is its sea level.
    if let Some(a) = desc.carrier {
        let mut a = a;
        a.metre = px;
        a.ground_y = 0.0;
        if let Some(f) = a.fog.as_mut() {
            f.base_y = 0.0;
        }
        lights.push(LightDesc { kind: LightKind::Ambient, position: [0.0; 3], direction: [0.0, 1.0, 0.0], colour: [0.0; 3], cone: [0.0, 1.0], radius: 0.0, falloff: 0.0, casts_shadows: false, shadow_darkness: 0.0, sky: None, look: Some(crate::atmosphere::LightLook { atmos: Some(a), ..Default::default() }) });
    }
    let _ = mesh_bounds;
    MeshDesc { instances, camera: Some(*camera), lights }
}
