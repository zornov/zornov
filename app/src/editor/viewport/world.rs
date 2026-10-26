// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Walkable worlds in the 3D workspace: the world is drawn live by the
//! real-time renderer (on its own, much faster path than the compositor),
//! and walk mode is a game's: first or third person, WASD and mouse look,
//! gravity, collision with the ground and everything standing on it,
//! steps, slopes, jumping and sprinting. "Record walk" turns what you walk
//! into a camera path, smoothed, keyed into a camera over the timeline.
//!
//! The navigation state stays the workspace's [`Nav`] (Studio pixels, y
//! down): walking only moves it, so the overlays, the camera tools and
//! recording see the same eye as ever.

use super::nav::Nav;
use crate::doc::Doc;
use crate::view::Time;
use eframe::egui;
use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::Arc;
use studio_model as m;
use studio_model::space as sp;
use studio_render::realtime::{Avatar, Options, Quality, Renderer, frame::View};
use studio_world::record::{Recorder, Smoothing};
use studio_world::walk::{Controls, Explorer, Input, Mode, Pose};
use studio_world::world::World;

/// Metres per Studio pixel unless a world says.
pub const DEFAULT_K: f32 = 0.01;

/// A world layer in the open sequence.
#[derive(Clone)]
pub struct Found {
    pub id: u64,
    pub file: String,
    /// Metres per pixel.
    pub k: f32,
    pub world: Arc<World>,
    /// The layer (a scene's own geometry has none).
    pub clip: Option<m::Clip>,
    /// A real world layer drawn by the live renderer; else the geometry of
    /// the scene's models, only walked on.
    pub live: bool,
}

thread_local! {
    static BAD: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

/// Not thread-local: GPU objects must outlive the thread that made them
/// being torn down (tests run the UI on short-lived threads).
static VIEW: std::sync::LazyLock<std::sync::Mutex<ViewState>> = std::sync::LazyLock::new(|| std::sync::Mutex::new(ViewState::default()));

#[derive(Default)]
struct ViewState {
    renderer: Option<Option<Renderer>>,
    tex: Option<egui::TextureHandle>,
    /// Fraction of the full resolution while moving, adapted to the frame time.
    dynamic: f32,
    last_ms: f32,
    frozen_time: f32,
    grabbed: bool,
    /// What the picture shown was made of, once it has refined fully.
    done_key: Option<u64>,
}

/// The world layer shown at `t` (an active one if there are several).
pub fn find(doc: &Doc, t: Time) -> Option<Found> {
    let mut pick: Option<(bool, &m::Clip, String)> = None;
    for c in &doc.model.active().clips {
        let m::ClipSource::Generator(g) = &c.source else { continue };
        if g.kind != m::GeneratorKind::Model {
            continue;
        }
        let Some(m::Value::Text(file)) = g.params.get("world") else { continue };
        if file.is_empty() {
            continue;
        }
        let active = c.start <= t && t < c.start + c.duration;
        if pick.as_ref().is_none_or(|p| !p.0 && active) {
            pick = Some((active, c, file.clone()));
        }
    }
    let (_, clip, file) = pick?;
    if BAD.with(|b| b.borrow().contains(&file)) {
        return None;
    }
    let Some(world) = studio_render::world_scene::load(&file) else {
        BAD.with(|b| b.borrow_mut().insert(file));
        return None;
    };
    let k = match &clip.source {
        m::ClipSource::Generator(g) => g.params.get("metres_per_px").and_then(|v| v.as_f64()).map_or(DEFAULT_K, |v| v as f32),
        _ => DEFAULT_K,
    };
    Some(Found { id: clip.id.0, file, k, world, clip: Some(clip.clone()), live: true })
}

/// The hour the layer is lit for at `t`, when it sets one (keyframes too).
pub fn time_of_day(clip: &m::Clip, t: Time) -> Option<f32> {
    let rel = (t - clip.start).clamp(0, clip.duration);
    if let Some(v) = clip.keyframes.keys().find(|k| k.as_str() == "generator.time_of_day").and_then(|k| clip.property_at(k, rel)).and_then(|v| v.as_f64()) {
        return Some(v as f32);
    }
    match &clip.source {
        m::ClipSource::Generator(g) => g.params.get("time_of_day").and_then(|v| v.as_f64()).map(|v| v as f32),
        _ => None,
    }
}

/// A walker's view as the workspace's navigation (Studio pixels, y down).
pub fn nav_of(p: &Pose, canvas_w: f64, canvas_h: f64, k: f32) -> Nav {
    let s = 1.0 / k.max(1e-6) as f64;
    let pos = [p.pos[0] as f64 * s, -(p.pos[1] as f64) * s, p.pos[2] as f64 * s];
    let f = p.forward();
    let look = [pos[0] + f[0] as f64 * 1000.0, pos[1] - f[1] as f64 * 1000.0, pos[2] + f[2] as f64 * 1000.0];
    let zoom = (canvas_h * 0.5) / (p.fov as f64 * 0.5).to_radians().tan().max(1e-6);
    let mut n = Nav::from_view(&sp::View { position: pos, axes: sp::look_at(pos, look, 0.0), zoom }, canvas_w, Some(look));
    n.lens = sp::focal_length_from_zoom(zoom, canvas_w).clamp(4.0, 2000.0);
    n
}

/// The workspace's eye as a walker's view (metres, y up).
pub fn pose_of(n: &Nav, canvas_w: f64, canvas_h: f64, k: f32) -> Pose {
    let p = n.position();
    let fov = 2.0 * ((canvas_h * 0.5) / n.zoom(canvas_w).max(1e-6)).atan().to_degrees();
    Pose { pos: [p[0] as f32 * k, -(p[1] as f32) * k, p[2] as f32 * k], yaw: n.yaw as f32, pitch: -(n.pitch as f32), fov: fov as f32 }
}

/// Lengthens the world layers to cover `to`.
pub fn extend_layers(doc: &mut Doc, to: Time) {
    let ids: Vec<u64> = doc
        .model
        .active()
        .clips
        .iter()
        .filter(|c| matches!(&c.source, m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Model && g.params.contains_key("world")))
        .map(|c| c.id.0)
        .collect();
    for id in ids {
        let Some(mut c) = doc.model.active().clip(m::ClipId(id)).cloned() else { continue };
        if c.start + c.duration < to {
            c.duration = to - c.start;
            let seq = doc.seq();
            doc.apply("Lengthen world", m::Edit::SetClip { seq, clip: Box::new(c) });
        }
    }
}

/// The geometry of the scene's own 3D models as ground to walk on: a floor
/// plane at the workspace's floor and every model's triangles as solid, so
/// a walker stands on them, climbs their steps and is stopped by their
/// walls. `None` when the scene has no models.
pub fn scene_found(doc: &Doc, t: Time, floor_px: f64, hidden: &std::collections::BTreeSet<u64>) -> Option<Found> {
    use std::hash::{Hash, Hasher};
    let objs = super::objects(doc, t, None, hidden);
    let models: Vec<&super::Object> = objs.iter().filter(|o| o.model.is_some()).collect();
    if models.is_empty() {
        return None;
    }
    let k = DEFAULT_K;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    floor_px.to_bits().hash(&mut h);
    for o in &models {
        let (file, mx) = o.model.as_ref().unwrap();
        (o.id, file).hash(&mut h);
        for r in mx {
            for v in r {
                v.to_bits().hash(&mut h);
            }
        }
    }
    let key = h.finish();
    static CACHE: std::sync::LazyLock<std::sync::Mutex<Option<(u64, Arc<World>)>>> = std::sync::LazyLock::new(|| std::sync::Mutex::new(None));
    if let Some((kk, w)) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
        && *kk == key
    {
        return Some(Found { id: 0, file: "scene".into(), k, world: w.clone(), clip: None, live: false });
    }
    let mut world = World::new("scene");
    let floor_m = -(floor_px as f32) * k;
    let mut terrain = studio_world::terrain::Terrain::flat([400.0, 400.0], 2.0);
    terrain.heights.iter_mut().for_each(|v| *v = floor_m);
    world.terrain = Some(terrain);
    for o in &models {
        let (file, mx) = o.model.as_ref().unwrap();
        let Some(mesh) = super::mesh_of(file) else { continue };
        let tris: Vec<[[f32; 3]; 3]> = (0..mesh.triangles())
            .map(|t| {
                mesh.tri(t).map(|i| {
                    let p = super::xform(mx, [mesh.positions[i][0] as f64, mesh.positions[i][1] as f64, mesh.positions[i][2] as f64]);
                    [p[0] as f32 * k, -(p[1] as f32) * k, p[2] as f32 * k]
                })
            })
            .collect();
        if tris.is_empty() {
            continue;
        }
        world.extra_solids.push(studio_world::collide::Solid::Mesh { bvh: Arc::new(studio_mesh::bvh::Bvh::new(tris)), at: [0.0; 3], yaw: 0.0, scale: 1.0 });
    }
    world.spawn.position = [0.0, floor_m, -3.0];
    world.finish();
    let world = Arc::new(world);
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = Some((key, world.clone()));
    Some(Found { id: 0, file: "scene".into(), k, world, clip: None, live: false })
}

// ------------------------------------------------------------ world files

/// Where a project keeps its worlds.
pub fn worlds_dir(doc: &Doc) -> std::path::PathBuf {
    match doc.folder.as_ref() {
        Some(f) => f.root().join("Worlds"),
        None => std::env::temp_dir().join("Zornov Worlds"),
    }
}

/// Saves a world's description by its content; the file's path.
pub fn save_spec(doc: &Doc, spec: &studio_world::build::WorldSpec) -> Result<String, String> {
    let bytes = serde_json::to_vec(spec).map_err(|e| e.to_string())?;
    let path = worlds_dir(doc).join(format!("{}.sworld", studio_mesh::io::content_hash(&bytes)));
    if !path.exists() {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        std::fs::write(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(path.to_string_lossy().into_owned())
}

pub fn read_spec(path: &str) -> Result<studio_world::build::WorldSpec, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))
}

/// The world file a clip shows, if it is a world layer.
pub fn world_file_of(c: &m::Clip) -> Option<String> {
    match &c.source {
        m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Model => match g.params.get("world") {
            Some(m::Value::Text(s)) if !s.is_empty() => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

// ------------------------------------------------------------ terrain brushes

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BrushKind {
    Raise,
    Lower,
    Smooth,
    Flatten,
    Noise,
    /// A paint layer 0-3.
    Paint(usize),
}

impl BrushKind {
    pub const ALL: [BrushKind; 9] = [BrushKind::Raise, BrushKind::Lower, BrushKind::Smooth, BrushKind::Flatten, BrushKind::Noise, BrushKind::Paint(0), BrushKind::Paint(1), BrushKind::Paint(2), BrushKind::Paint(3)];

    pub fn label(self) -> String {
        match self {
            BrushKind::Raise => "Raise".into(),
            BrushKind::Lower => "Lower".into(),
            BrushKind::Smooth => "Smooth".into(),
            BrushKind::Flatten => "Flatten".into(),
            BrushKind::Noise => "Roughen".into(),
            BrushKind::Paint(i) => format!("Paint {}", ["grass", "rock", "dirt", "snow"][i.min(3)]),
        }
    }
}

/// The terrain brush being held.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Brush {
    pub kind: BrushKind,
    /// Metres.
    pub radius: f32,
    /// Metres a stroke raises at the centre (or how far it smooths, 0-1).
    pub strength: f32,
}

impl Default for Brush {
    fn default() -> Brush {
        Brush { kind: BrushKind::Raise, radius: 8.0, strength: 0.4 }
    }
}

struct Session {
    file: String,
    world: Arc<World>,
    sculpt: Vec<studio_world::build::SculptJ>,
    paint: Vec<studio_world::build::PaintJ>,
    last: Option<([f32; 2], f64)>,
}

static SESSION: std::sync::LazyLock<std::sync::Mutex<Option<Session>>> = std::sync::LazyLock::new(|| std::sync::Mutex::new(None));
static HIT: std::sync::LazyLock<std::sync::Mutex<Option<([f32; 3], f32)>>> = std::sync::LazyLock::new(|| std::sync::Mutex::new(None));

/// The world being sculpted, if it is `file`'s.
pub fn session_world(file: &str) -> Option<Arc<World>> {
    SESSION.lock().unwrap_or_else(|e| e.into_inner()).as_ref().filter(|s| s.file == file).map(|s| s.world.clone())
}

/// Where the pointer's ray meets the ground (metres), for the brush.
pub fn hover(found: &Found, view: &sp::View, canvas_point: [f64; 2], brush: &Brush) -> Option<[f32; 3]> {
    let k = found.k;
    let d = view.ray(m::Vec2::new(canvas_point[0], canvas_point[1]));
    let o = [view.position[0] as f32 * k, -(view.position[1] as f32) * k, view.position[2] as f32 * k];
    let dir = [d[0] as f32, -(d[1] as f32), d[2] as f32];
    let world = session_world(&found.file).unwrap_or_else(|| found.world.clone());
    let t = world.terrain.as_ref()?;
    let hit = t.raycast(o, dir, 2000.0)?;
    let l = studio_world::math::len(dir).max(1e-6);
    let p = [o[0] + dir[0] / l * hit, o[1] + dir[1] / l * hit, o[2] + dir[2] / l * hit];
    *HIT.lock().unwrap_or_else(|e| e.into_inner()) = Some((p, brush.radius));
    Some(p)
}

pub fn clear_hover() {
    *HIT.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The brush ring on the ground, in Studio pixels, for drawing.
pub fn ring(found: &Found) -> Vec<[f64; 3]> {
    let Some((p, r)) = *HIT.lock().unwrap_or_else(|e| e.into_inner()) else { return Vec::new() };
    let world = session_world(&found.file).unwrap_or_else(|| found.world.clone());
    let s = 1.0 / found.k as f64;
    (0..=48)
        .map(|i| {
            let a = i as f32 / 48.0 * std::f32::consts::TAU;
            let (x, z) = (p[0] + a.cos() * r, p[2] + a.sin() * r);
            let y = world.ground_y(x, z) + 0.08;
            [x as f64 * s, -(y as f64) * s, z as f64 * s]
        })
        .collect()
}

/// Applies one stroke at `hit` (when the brush has moved on or enough
/// time has passed); strokes are kept to be written into the world when
/// the button is released, and replay the same way when it is rebuilt.
pub fn stroke(found: &Found, brush: &Brush, hit: [f32; 3], now: f64) -> bool {
    let mut g = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    if g.as_ref().is_none_or(|s| s.file != found.file) {
        *g = Some(Session { file: found.file.clone(), world: Arc::new((*found.world).clone()), sculpt: Vec::new(), paint: Vec::new(), last: None });
    }
    let s = g.as_mut().unwrap();
    let at = [hit[0], hit[2]];
    if let Some((p, t)) = s.last
        && ((p[0] - at[0]).powi(2) + (p[1] - at[1]).powi(2)).sqrt() < brush.radius * 0.25
        && now - t < 0.08
    {
        return false;
    }
    s.last = Some((at, now));
    let world = Arc::make_mut(&mut s.world);
    let Some(t) = world.terrain.as_mut() else { return false };
    use studio_world::terrain::Brush as B;
    match brush.kind {
        BrushKind::Paint(l) => {
            t.paint(l, at, brush.radius, brush.strength.clamp(0.05, 1.0));
            s.paint.push(studio_world::build::PaintJ { layer: l.to_string(), at, radius: brush.radius, strength: brush.strength.clamp(0.05, 1.0) });
        }
        kind => {
            let (name, b, strength) = match kind {
                BrushKind::Raise => ("raise", B::Raise, brush.strength),
                BrushKind::Lower => ("lower", B::Lower, brush.strength),
                BrushKind::Smooth => ("smooth", B::Smooth, brush.strength.clamp(0.05, 1.0)),
                BrushKind::Flatten => ("flatten", B::Flatten, brush.strength.clamp(0.05, 1.0)),
                _ => ("noise", B::Noise, brush.strength),
            };
            t.sculpt(b, at, brush.radius, strength);
            s.sculpt.push(studio_world::build::SculptJ { brush: name.into(), at, radius: brush.radius, strength, level: None, passes: 1 });
        }
    }
    true
}

/// Whether strokes are waiting to be written.
pub fn pending() -> bool {
    SESSION.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|s| !s.sculpt.is_empty() || !s.paint.is_empty())
}

/// Writes the strokes into the world's description, rebuilds it (trees
/// and buildings follow the new ground) and points the layer at it: one
/// undo step. Returns a line for the person.
pub fn commit(doc: &mut Doc) -> Result<String, String> {
    let Some(s) = SESSION.lock().unwrap_or_else(|e| e.into_inner()).take() else { return Ok(String::new()) };
    if s.sculpt.is_empty() && s.paint.is_empty() {
        return Ok(String::new());
    }
    let mut spec = read_spec(&s.file)?;
    let (n_s, n_p) = (s.sculpt.len(), s.paint.len());
    spec.sculpt.extend(s.sculpt);
    spec.paint.extend(s.paint);
    let path = save_spec(doc, &spec)?;
    let world = Arc::new(studio_world::build::build(&spec, &studio_world::build::FileLibrary)?);
    studio_render::world_scene::remember(&path, world);
    let Some(mut clip) = doc.model.active().clips.iter().find(|c| world_file_of(c).as_deref() == Some(s.file.as_str())).cloned() else { return Err("the world layer is gone".into()) };
    if let m::ClipSource::Generator(g) = &mut clip.source {
        g.params.insert("world".into(), m::Value::Text(path));
    }
    let seq = doc.seq();
    doc.apply("Sculpt terrain", m::Edit::SetClip { seq, clip: Box::new(clip) });
    Ok(format!("Terrain edited: {n_s} sculpt and {n_p} paint strokes"))
}

/// Drops strokes not yet written (the tool was closed).
pub fn discard() {
    *SESSION.lock().unwrap_or_else(|e| e.into_inner()) = None;
    clear_hover();
}

/// A walk being recorded.
#[derive(Clone, Debug)]
pub struct WalkRec {
    pub recorder: Recorder,
    pub t0: f64,
    pub camera: Option<u64>,
    /// Where on the timeline the shot starts.
    pub at: Time,
}

/// The walker.
#[derive(Clone, Debug)]
pub struct WalkState {
    pub ex: Explorer,
    pub file: String,
    pub k: f32,
    pub rec: Option<WalkRec>,
    pub clock: f64,
}

impl WalkState {
    /// Stands where the eye is (dropping to the ground below it).
    pub fn enter(found: &Found, nav: &Nav, w: f64, h: f64) -> WalkState {
        let mut ex = Explorer::new(&found.world);
        let p = pose_of(nav, w, h, found.k);
        ex.mode = Mode::Fly;
        ex.fly_pos = p.pos;
        ex.yaw = p.yaw;
        ex.pitch = p.pitch.clamp(-85.0, 85.0);
        ex.fov = p.fov.clamp(10.0, 120.0);
        ex.set_mode(&found.world, Mode::Walk);
        WalkState { ex, file: found.file.clone(), k: found.k, rec: None, clock: 0.0 }
    }
}

/// What the keys and mouse ask this frame: (controls, scroll).
pub fn read_controls(ctx: &egui::Context, look: bool) -> (Controls, f32, f32) {
    let (dt, mouse, pointer, mods, keys, scroll, space) = ctx.input(|i| {
        let k = |key: egui::Key| i.key_down(key);
        let moved = i.events.iter().fold(egui::Vec2::ZERO, |a, e| if let egui::Event::MouseMoved(d) = e { a + *d } else { a });
        (i.stable_dt.min(0.1), moved, i.pointer.delta(), i.modifiers, [k(egui::Key::W), k(egui::Key::S), k(egui::Key::A), k(egui::Key::D), k(egui::Key::Q), k(egui::Key::E)], i.smooth_scroll_delta.y, k(egui::Key::Space))
    });
    // The keys the editor would act on are the walker's.
    ctx.input_mut(|i| {
        for k in [egui::Key::W, egui::Key::S, egui::Key::A, egui::Key::D, egui::Key::Q, egui::Key::E, egui::Key::Space, egui::Key::V] {
            i.events.retain(|e| !matches!(e, egui::Event::Key { key, .. } if *key == k));
        }
    });
    let d = if mouse != egui::Vec2::ZERO { mouse } else { pointer };
    let look_deg = if look { [d.x * 0.16, d.y * 0.16] } else { [0.0, 0.0] };
    let c = Controls {
        input: Input { forward: (keys[0] as i32 - keys[1] as i32) as f32, right: (keys[3] as i32 - keys[2] as i32) as f32, up: (keys[5] as i32 - keys[4] as i32) as f32, jump: space, sprint: mods.shift },
        look: look_deg,
        zoom: (scroll / 120.0).clamp(-3.0, 3.0),
        pan: [0.0; 2],
    };
    (c, dt, scroll)
}

/// One frame of walking, flying or orbiting the world with the keys and
/// mouse; moves the workspace's eye.
pub fn step(ctx: &egui::Context, st: &mut WalkState, found: &Found, nav: &mut Nav, w: f64, h: f64, now: f64) -> bool {
    let (c, dt, _) = read_controls(ctx, true);
    st.ex.update(&found.world, &c, dt);
    // Anything going on: keys held, looking, falling or gliding to a stop.
    let active = c.input.forward != 0.0 || c.input.right != 0.0 || c.input.jump || c.look != [0.0, 0.0] || st.ex.mode == Mode::Walk && (!st.ex.body.grounded || st.ex.body.speed_h() > 0.05 || st.ex.body.vel[1].abs() > 0.05) || st.rec.is_some();
    let pose = st.ex.camera(&found.world);
    *nav = nav_of(&pose, w, h, found.k);
    nav.ortho = false;
    st.clock = now;
    if let Some(rec) = &mut st.rec {
        // The recorded camera is the walker's eye, not the third-person camera.
        let eye = Pose { pos: st.ex.eye(), yaw: st.ex.yaw, pitch: st.ex.pitch, fov: st.ex.fov };
        rec.recorder.push((now - rec.t0) as f32, eye);
    }
    active
}

/// Hides and holds the pointer while walking (look with raw motion).
pub fn grab(ctx: &egui::Context, on: bool) {
    let changed = {
        let mut v = VIEW.lock().unwrap_or_else(|e| e.into_inner());
        let was = v.grabbed;
        v.grabbed = on;
        was != on
    };
    if changed {
        ctx.send_viewport_cmd(egui::ViewportCommand::CursorGrab(if on { egui::viewport::CursorGrab::Locked } else { egui::viewport::CursorGrab::None }));
        ctx.send_viewport_cmd(egui::ViewportCommand::CursorVisible(!on));
    }
}

/// Turns a finished walk into camera keys: smoothed, sampled each frame,
/// from `rec.at` on. Returns a line for the person.
pub fn finish_recording(doc: &mut Doc, camera: u64, rec: &WalkRec, k: f32) -> Result<String, String> {
    let path = rec.recorder.finish(&Smoothing::default());
    if path.keys.len() < 2 {
        return Err("nothing walked: move while recording".into());
    }
    let (w, h) = (doc.project.width as f64, doc.project.height as f64);
    let fps = doc.project.fps.max(1.0) as f64;
    let step = (1e6 / fps) as Time;
    let n = (path.duration() as f64 * fps).ceil() as i64;
    let mut samples = Vec::new();
    for i in 0..=n {
        let t = (i as f64 / fps).min(path.duration() as f64) as f32;
        if let Some(p) = path.at(t) {
            samples.push((rec.at + i * step, nav_of(&p, w, h, k)));
        }
    }
    let keys = super::record_path(doc, camera, &samples)?;
    extend_layers(doc, rec.at + (path.duration() as f64 * 1e6) as Time);
    Ok(format!("Recorded the walk: {:.1} s as {keys} camera keys, smoothed", path.duration()))
}

/// Draws the world live from the editor's eye; the texture to show.
#[allow(clippy::too_many_arguments)]
pub fn draw(ctx: &egui::Context, found: &Found, st: Option<&WalkState>, camera: &studio_render::space::CameraDesc, canvas: (u32, u32), size_pts: egui::Vec2, quality: usize, moving: bool, playing: bool, playhead: Time, now: f64) -> Option<egui::TextureId> {
    let draw = || -> Option<egui::TextureId> {
        let mut v = VIEW.lock().unwrap_or_else(|e| e.into_inner());
        if v.renderer.is_none() {
            v.renderer = Some(studio_render::GpuContext::new().and_then(|c| Renderer::new(c).ok()));
        }
        if v.dynamic <= 0.0 {
            v.dynamic = 1.0;
        }
        let ppp = ctx.pixels_per_point();
        let base = [1.0, 0.75, 0.55][quality.min(2)];
        // While moving the resolution follows the frame time (the picture
        // stays smooth); at rest it is full and refines.
        let target_ms = 14.0;
        if moving && v.last_ms > 0.0 {
            let f = (target_ms / v.last_ms).powf(0.25);
            v.dynamic = (v.dynamic * f).clamp(0.4, 1.0);
        } else if !moving {
            v.dynamic = (v.dynamic * 1.1).min(1.0);
        }
        let s = base * if moving { v.dynamic } else { 1.0 };
        let (pw, ph) = (((size_pts.x * ppp * s).round() as u32).max(64), ((size_pts.y * ppp * s).round() as u32).max(64));
        if moving || playing {
            v.frozen_time = if playing { playhead as f32 / 1e6 } else { now as f32 };
        }
        // A fully refined picture of the same thing is kept as it is.
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (pw, ph, camera.position.map(f32::to_bits), camera.axes.map(|r| r.map(f32::to_bits)), camera.zoom.to_bits(), Arc::as_ptr(&found.world) as usize, found.clip.as_ref().and_then(|c| time_of_day(c, playhead)).map(f32::to_bits), quality, st.is_some_and(|s| s.ex.third_person)).hash(&mut h);
            h.finish()
        };
        if !moving && !playing && v.done_key == Some(key) && v.tex.is_some() {
            return v.tex.as_ref().map(|t| t.id());
        }
        let canvas = studio_render::scene::Canvas::new(canvas.0, canvas.1, 30.0);
        let b = found.world.bounds();
        let far = (studio_world::math::len(studio_world::math::sub(b.max, b.min)) * 1.2).clamp(300.0, 3000.0);
        let view: View = studio_render::world_scene::view_from_camera(camera, &canvas, found.k, far);
        let mut o = Options::new(pw, ph);
        o.quality = if moving { Quality::Fast } else if quality == 0 { Quality::High } else if quality == 1 { Quality::Balanced } else { Quality::Fast };
        o.time = v.frozen_time;
        o.refine = !moving && !playing;
        o.sky_time = found.clip.as_ref().and_then(|c| time_of_day(c, playhead));
        if let Some(st) = st
            && let Some((pos, yaw)) = st.ex.avatar()
        {
            o.avatar = Some(Avatar { pos, yaw, speed: st.ex.body.speed_h(), time: now as f32 });
        }
        let r = v.renderer.as_mut()?.as_mut()?;
        let (img, stats) = r.render_srgb(&found.world, &view, &o).ok()?;
        v.last_ms = stats.ms;
        let color = egui::ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.rgba);
        let opts = egui::TextureOptions::LINEAR;
        match &mut v.tex {
            Some(t) => t.set(color, opts),
            None => v.tex = Some(ctx.load_texture("world-view", color, opts)),
        }
        // Still refining: come back for the next frame.
        if !moving && stats.refined < o.max_refine {
            v.done_key = None;
            ctx.request_repaint();
        } else {
            v.done_key = if moving || playing { None } else { Some(key) };
        }
        v.tex.as_ref().map(|t| t.id())
    };
    draw()
}

/// Milliseconds the last world frame took (for the status text).
pub fn last_ms() -> f32 {
    VIEW.lock().unwrap_or_else(|e| e.into_inner()).last_ms
}
