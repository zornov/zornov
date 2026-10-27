// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Small 3D scenes for the lighting, atmosphere and look tests, the
//! before/after gallery and the checks' own tests. Each scene builder takes `cinematic`: off, it is
//! the scene as a plain lighting set-up leaves it; on, it has the air,
//! practical lights, wetness and look a lighting artist would give it.

use std::sync::Arc;
use super::{Atmos, Bloom, Flame, Fog, Grade, LightLook, Tonemap, Weather, WeatherKind};
use crate::mesh_render::{MaterialOverride, MeshDesc, MeshInstance};
use crate::scene::Canvas;
use crate::space::{CameraDesc, LightDesc, LightKind, normalise};
use studio_mesh::Mesh;

pub struct Scene {
    pub desc: MeshDesc,
    pub canvas: Canvas,
    next: u64,
}

pub fn metre(canvas: &Canvas) -> f32 {
    canvas.height as f32 / 3.5
}

impl Scene {
    pub fn new(w: u32, h: u32) -> Scene {
        let canvas = Canvas::new(w, h, 24.0);
        let camera = CameraDesc::default_for(&canvas);
        Scene { desc: MeshDesc { instances: Vec::new(), camera: Some(camera), lights: Vec::new() }, canvas, next: 1 }
    }

    pub fn m(&self) -> f32 {
        metre(&self.canvas)
    }

    /// Camera at `from` looking at `to`.
    pub fn camera(&mut self, from: [f32; 3], to: [f32; 3]) {
        let zoom = crate::space::default_zoom(self.canvas.width as f32);
        self.desc.camera = Some(CameraDesc { position: from, axes: crate::space::look_at(from, to, 0.0), zoom, dof: None, trace: None });
    }

    pub fn add(&mut self, mesh: Mesh, at: [f32; 3], o: MaterialOverride) -> u64 {
        let id = self.next;
        self.next += 1;
        self.desc.instances.push(MeshInstance {
            id,
            mesh: Arc::new(mesh),
            key: format!("look{id}"),
            world: [[1.0, 0.0, 0.0, at[0]], [0.0, 1.0, 0.0, at[1]], [0.0, 0.0, 1.0, at[2]]],
            overrides: o,
            smooth: 1.0,
            accepts_lights: true,
            casts_shadows: true,
            accepts_shadows: true,
            motion: None,
            direct: None,
        });
        id
    }

    /// Scales the last instance non-uniformly.
    pub fn scale_last(&mut self, k: [f32; 3]) {
        let w = &mut self.desc.instances.last_mut().unwrap().world;
        for r in 0..3 {
            w[r][r] = k[r];
        }
    }

    pub fn light(&mut self, l: LightDesc) {
        self.desc.lights.push(l);
    }
}

pub fn mat(colour: [f32; 3], rough: f32) -> MaterialOverride {
    MaterialOverride { base_colour: Some([colour[0], colour[1], colour[2], 1.0]), metallic: Some(0.0), roughness: Some(rough), ..Default::default() }
}

pub fn glowing(colour: [f32; 3], strength: f32) -> MaterialOverride {
    MaterialOverride { base_colour: Some([0.02, 0.02, 0.02, 1.0]), metallic: Some(0.0), roughness: Some(0.4), emissive: Some(colour.map(|c| c * strength)), ..Default::default() }
}

pub fn wet(mut o: MaterialOverride, wetness: f32) -> MaterialOverride {
    o.wetness = Some(wetness);
    o
}

pub fn base_light(kind: LightKind, position: [f32; 3], direction: [f32; 3], colour: [f32; 3]) -> LightDesc {
    LightDesc { kind, position, direction: normalise(direction), colour, cone: [0.0, 1.0], radius: 0.0, falloff: 0.0, casts_shadows: false, shadow_darkness: 0.85, sky: None, look: None }
}

/// A sun that brings a daylight sky (`hour` of the day, travelling toward
/// `heading` on the ground): the sky shows behind and lights the models.
pub fn sun_with_sky(hour: f32, heading: f32, turbidity: f32, strength: f32, shadows: bool) -> LightDesc {
    let mut sky = crate::sky::SkyDesc { turbidity, strength, ..Default::default() };
    sky.sun = crate::sky::sun_for_time(hour, 172.0, 40.0, heading);
    let mut l = base_light(LightKind::Parallel, [0.0; 3], sky.sun, crate::space::scale(crate::sky::sun_colour(&sky), 1.0));
    l.casts_shadows = shadows;
    l.sky = Some(sky);
    l
}

pub fn ambient(colour: [f32; 3]) -> LightDesc {
    base_light(LightKind::Ambient, [0.0; 3], [0.0, 1.0, 0.0], colour)
}

pub fn spot(position: [f32; 3], aim: [f32; 3], colour: [f32; 3], cone_deg: f32, shadows: bool) -> LightDesc {
    let d = [aim[0] - position[0], aim[1] - position[1], aim[2] - position[2]];
    let half = (cone_deg * 0.5).to_radians();
    LightDesc { cone: [half.cos(), (half * 0.6).cos()], casts_shadows: shadows, ..base_light(LightKind::Spot, position, d, colour) }
}

pub fn with_look(mut l: LightDesc, look: LightLook) -> LightDesc {
    l.look = Some(look);
    l
}

pub fn lamp_look(glow: f32, flare: f32, beam: f32, size: f32) -> LightLook {
    LightLook { glow, flare, beam, size, ..Default::default() }
}

/// The light that carries the scene's air and look.
pub fn air(a: Atmos) -> LightDesc {
    LightDesc { look: Some(LightLook { atmos: Some(a), ..Default::default() }), ..ambient([0.0; 3]) }
}

pub fn lum(p: [f32; 4]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

pub fn render(s: &Scene, w: u32, h: u32) -> crate::Frame {
    crate::mesh_render::render(&s.desc, &s.canvas, (w, h))
}

/// [`render`] with the air and look pass on the CPU or on the GPU.
pub fn render_on(s: &Scene, w: u32, h: u32, gpu: bool) -> crate::Frame {
    crate::mesh_render::render_with(&s.desc, &s.canvas, (w, h), gpu)
}

// ------------------------------------------------------------ scenes

/// A night street: wet road, lamps, shopfront neon, rain.
pub fn neon_alley(cinematic: bool, w: u32, h: u32) -> Scene {
    let mut s = Scene::new(w, h);
    let m = s.m();
    let cam_y = -1.0 * m;
    s.camera([0.0, cam_y, -3.0 * m], [0.0, cam_y + 0.05 * m, 8.0 * m]);
    let ground = 0.6 * m;
    let road = mat([0.16, 0.16, 0.17], 0.8);
    let road = if cinematic { wet(road, 0.9) } else { road };
    s.add(studio_mesh::prim::plane(60.0 * m, 80.0 * m, 4), [0.0, ground, 30.0 * m], road);
    // Buildings either side.
    let bricks = [[0.20, 0.10, 0.09], [0.12, 0.13, 0.16], [0.16, 0.14, 0.11], [0.10, 0.12, 0.14]];
    for (i, z) in [4.0, 9.0, 14.0, 19.0, 24.0].iter().enumerate() {
        let hgt = (7.0 + (i % 3) as f32 * 2.5) * m;
        for side in [-1.0f32, 1.0] {
            let c = bricks[(i + if side > 0.0 { 1 } else { 0 }) % 4];
            s.add(studio_mesh::prim::cube([6.0 * m, hgt, 4.2 * m]), [side * 5.2 * m, ground - hgt * 0.5, z * m], mat(c, 0.85));
        }
    }
    // A neon sign and a lit window.
    let neon = [1.0, 0.1, 0.55];
    s.add(studio_mesh::prim::cube([0.1 * m, 0.5 * m, 2.4 * m]), [-2.15 * m, ground - 3.0 * m, 9.0 * m], glowing(neon, if cinematic { 5.0 } else { 1.0 }));
    let cyan = [0.1, 0.8, 1.0];
    s.add(studio_mesh::prim::cube([0.1 * m, 0.4 * m, 1.6 * m]), [2.15 * m, ground - 2.4 * m, 14.0 * m], glowing(cyan, if cinematic { 5.0 } else { 1.0 }));
    // Lamp posts with bulbs.
    let warm = [1.0, 0.72, 0.38];
    for z in [6.0f32, 13.0, 20.0] {
        let x = 2.0 * m;
        s.add(studio_mesh::prim::cylinder(0.06 * m, 0.08 * m, 4.2 * m, 8), [x, ground - 2.1 * m, z * m], mat([0.05, 0.05, 0.06], 0.5));
        s.add(studio_mesh::prim::uv_sphere(0.16 * m, 12, 8), [x, ground - 4.3 * m, z * m], glowing(warm, if cinematic { 7.0 } else { 1.0 }));
        let mut l = base_light(LightKind::Point, [x, ground - 4.3 * m, z * m], [0.0, 1.0, 0.0], warm.map(|c| c * 1.4));
        l.radius = 0.0;
        l.falloff = 7.0 * m;
        if cinematic {
            l = with_look(l, lamp_look(0.8, 0.2, 0.3, 0.16));
        }
        s.light(l);
    }
    // Neon colour spills onto the street.
    for (pos, c) in [([-1.6 * m, ground - 3.0 * m, 9.0 * m], neon), ([1.6 * m, ground - 2.4 * m, 14.0 * m], cyan)] {
        let mut l = base_light(LightKind::Point, pos, [0.0, 1.0, 0.0], c.map(|v| v * 1.0));
        l.falloff = 6.0 * m;
        if cinematic {
            l = with_look(l, lamp_look(0.8, 0.0, 0.0, 0.2));
        }
        s.light(l);
    }
    s.light(ambient([0.05, 0.06, 0.1]));
    s.light(base_light(LightKind::Parallel, [0.0; 3], [0.3, 1.0, 0.2], [0.05, 0.07, 0.14]));
    if cinematic {
        let mut a = Atmos { metre: m, ground_y: ground, ..Default::default() };
        a.fog = Some(Fog { density: 0.02, falloff: 0.1, base_y: ground, colour: [0.55, 0.6, 0.8], anisotropy: 0.5, range: 60.0, patchiness: 0.2 });
        a.weather[0] = Some(Weather::new(WeatherKind::Rain, 0.8));
        a.bloom = Bloom { strength: 0.35, threshold: 0.9, knee: 0.6, radius: 0.55 };
        a.grade = Grade { tonemap: Tonemap::Filmic, richness: 0.6, contrast: 1.1, saturation: 1.05, vignette: 0.25, grain: 0.15, shadow_tint: [0.1, 0.25, 0.5, 0.6], halation: 0.2, ..Default::default() };
        s.light(air(a));
    }
    s
}

/// Dawn in a forest: low sun through trees, mist, pollen.
pub fn forest_dawn(cinematic: bool, w: u32, h: u32) -> Scene {
    let mut s = Scene::new(w, h);
    let m = s.m();
    let cam_y = -1.6 * m;
    s.camera([0.0, cam_y, -4.0 * m], [0.0, cam_y + 0.2 * m, 8.0 * m]);
    let ground = 0.0;
    s.add(studio_mesh::prim::plane(80.0 * m, 80.0 * m, 4), [0.0, ground, 30.0 * m], mat([0.16, 0.22, 0.1], 0.95));
    let mut k = 7u32;
    for i in 0..26 {
        k = k.wrapping_mul(1664525).wrapping_add(1013904223);
        let rx = ((k >> 8) % 1000) as f32 / 1000.0;
        k = k.wrapping_mul(1664525).wrapping_add(1013904223);
        let rz = ((k >> 8) % 1000) as f32 / 1000.0;
        let x = (rx - 0.5) * 22.0 * m;
        let z = (1.0 + rz * 26.0) * m;
        if x.abs() < 1.0 * m {
            continue;
        }
        let hgt = (6.0 + (i % 5) as f32) * m;
        s.add(studio_mesh::prim::cylinder(0.22 * m, 0.3 * m, hgt, 8), [x, ground - hgt * 0.5, z], mat([0.2, 0.14, 0.09], 0.9));
        s.add(studio_mesh::prim::cone(1.6 * m, hgt * 0.8, 10), [x, ground - hgt * 0.9, z], mat([0.06, 0.16, 0.07], 0.9));
    }
    let mut sun = sun_with_sky(6.4, -62.0, 4.0, 1.0, true);
    if cinematic {
        sun = with_look(sun, LightLook { beam: 0.35, ..Default::default() });
    }
    s.light(sun);
    if cinematic {
        let mut a = Atmos { metre: m, ground_y: ground, ..Default::default() };
        a.fog = Some(Fog { density: 0.011, falloff: 0.2, base_y: ground, colour: [1.0, 0.88, 0.72], anisotropy: 0.65, range: 60.0, patchiness: 0.5 });
        a.weather[0] = Some(Weather::new(WeatherKind::Dust, 0.8));
        a.bloom = Bloom { strength: 0.12, threshold: 1.0, knee: 0.7, radius: 0.55 };
        a.grade = Grade { tonemap: Tonemap::Filmic, richness: 0.45, contrast: 1.15, temperature: 0.1, saturation: 1.2, vignette: 0.22, grain: 0.08, shadow_tint: [0.05, 0.25, 0.2, 0.4], ..Default::default() };
        // A cool fill from the sky so the trunks keep their form against the glow.
        s.light(ambient([0.07, 0.09, 0.13]));
        s.light(air(a));
    }
    s
}

/// A candlelit room.
pub fn candlelit_room(cinematic: bool, w: u32, h: u32) -> Scene {
    let mut s = Scene::new(w, h);
    let m = s.m();
    s.camera([0.0, -1.4 * m, -2.0 * m], [0.0, -0.9 * m, 4.0 * m]);
    let floor = 0.0;
    s.add(studio_mesh::prim::plane(14.0 * m, 14.0 * m, 4), [0.0, floor, 4.0 * m], mat([0.28, 0.17, 0.1], 0.5));
    s.add(studio_mesh::prim::cube([14.0 * m, 4.0 * m, 0.3 * m]), [0.0, floor - 2.0 * m, 7.0 * m], mat([0.3, 0.26, 0.22], 0.9));
    s.add(studio_mesh::prim::cube([0.3 * m, 4.0 * m, 14.0 * m]), [-4.0 * m, floor - 2.0 * m, 3.0 * m], mat([0.3, 0.26, 0.22], 0.9));
    s.add(studio_mesh::prim::cube([0.3 * m, 4.0 * m, 14.0 * m]), [4.0 * m, floor - 2.0 * m, 3.0 * m], mat([0.3, 0.26, 0.22], 0.9));
    // A table with a candle and a goblet.
    s.add(studio_mesh::prim::cube([2.4 * m, 0.08 * m, 1.2 * m]), [0.0, floor - 0.78 * m, 3.4 * m], mat([0.3, 0.18, 0.1], 0.5));
    for (dx, dz) in [(-1.0f32, -0.45f32), (1.0, -0.45), (-1.0, 0.45), (1.0, 0.45)] {
        s.add(studio_mesh::prim::cylinder(0.05 * m, 0.05 * m, 0.76 * m, 6), [dx * m, floor - 0.38 * m, 3.4 * m + dz * m], mat([0.25, 0.15, 0.08], 0.6));
    }
    s.add(studio_mesh::prim::cylinder(0.035 * m, 0.035 * m, 0.17 * m, 10), [0.0, floor - 0.9 * m, 3.4 * m], mat([0.9, 0.85, 0.7], 0.6));
    let mut gob = mat([0.8, 0.6, 0.2], 0.2);
    gob.metallic = Some(1.0);
    s.add(studio_mesh::prim::cylinder(0.07 * m, 0.04 * m, 0.14 * m, 12), [0.5 * m, floor - 0.87 * m, 3.2 * m], gob);
    s.add(studio_mesh::prim::uv_sphere(0.3 * m, 14, 8), [-0.9 * m, floor - 0.95 * m, 3.7 * m], mat([0.55, 0.12, 0.1], 0.4));
    let flame_pos = [0.0, floor - 1.02 * m, 3.4 * m];
    let mut l = base_light(LightKind::Point, flame_pos, [0.0, 1.0, 0.0], [1.0, 0.55, 0.2].map(|c| c * 2.2));
    l.casts_shadows = true;
    l.radius = 0.5 * m;
    l.falloff = 4.5 * m;
    if cinematic {
        l = with_look(l, LightLook { glow: 0.3, beam: 0.0, size: 0.012, flame: Some(Flame { height: 0.06, width: 0.025, tongues: 1.0, seed: 0.3 }), time: 0.4, ..Default::default() });
    }
    s.light(l);
    s.light(ambient([0.03, 0.025, 0.03]));
    if cinematic {
        let mut a = Atmos { metre: m, ground_y: floor, ..Default::default() };
        a.fog = Some(Fog { density: 0.012, falloff: 0.0, base_y: floor, colour: [1.0, 0.8, 0.6], anisotropy: 0.5, range: 25.0, patchiness: 0.0 });
        a.weather[0] = Some(Weather::new(WeatherKind::Dust, 0.9));
        a.bloom = Bloom { strength: 0.3, threshold: 0.8, knee: 0.7, radius: 0.5 };
        a.grade = Grade { tonemap: Tonemap::Filmic, richness: 0.5, temperature: 0.2, vignette: 0.35, grain: 0.18, ..Default::default() };
        s.light(air(a));
    }
    s
}

/// A snowy village at dusk.
pub fn snowy_village(cinematic: bool, w: u32, h: u32) -> Scene {
    let mut s = Scene::new(w, h);
    let m = s.m();
    s.camera([0.0, -1.7 * m, -4.0 * m], [0.0, -1.2 * m, 10.0 * m]);
    let ground = 0.0;
    s.add(studio_mesh::prim::plane(90.0 * m, 90.0 * m, 4), [0.0, ground, 30.0 * m], mat([0.82, 0.86, 0.92], 0.9));
    let warm = [1.0, 0.7, 0.35];
    for (i, (x, z)) in [(-5.0f32, 7.0f32), (4.5, 9.0), (-4.0, 14.0), (6.0, 15.0), (0.5, 22.0)].iter().enumerate() {
        let bw = 4.0 * m;
        let bh = 2.6 * m;
        s.add(studio_mesh::prim::cube([bw, bh, 3.2 * m]), [x * m, ground - bh * 0.5, z * m], mat([0.42, 0.26, 0.18], 0.85));
        let mut roof = studio_mesh::prim::cone(3.0 * m, 1.6 * m, 4);
        roof.materials[0].base_colour = [0.9, 0.93, 0.98, 1.0];
        roof.materials[0].roughness = 0.9;
        s.add(roof, [x * m, ground - bh - 0.8 * m, z * m], MaterialOverride::default());
        // A lit window facing the camera.
        let ws = if cinematic { 4.0 } else { 1.0 };
        s.add(studio_mesh::prim::cube([0.9 * m, 0.7 * m, 0.05 * m]), [x * m, ground - 1.4 * m, (z - 1.62) * m], glowing(warm, ws));
        if i % 2 == 0 {
            let mut l = spot([x * m, ground - 1.4 * m, (z - 2.2) * m], [x * m, ground, (z - 5.0) * m], warm.map(|c| c * 1.2), 100.0, false);
            l.falloff = 6.0 * m;
            if cinematic {
                l = with_look(l, lamp_look(0.9, 0.0, 0.7, 0.4));
            }
            s.light(l);
        }
    }
    s.light(sun_with_sky(17.3, 80.0, 6.0, 0.9, false));
    s.light(ambient([0.03, 0.04, 0.07]));
    if cinematic {
        let mut a = Atmos { metre: m, ground_y: ground, ..Default::default() };
        a.fog = Some(Fog { density: 0.03, falloff: 0.05, base_y: ground, colour: [0.6, 0.68, 0.85], anisotropy: 0.4, range: 70.0, patchiness: 0.3 });
        a.weather[0] = Some(Weather::new(WeatherKind::Snow, 0.9));
        a.bloom = Bloom { strength: 0.25, threshold: 0.85, knee: 0.6, radius: 0.6 };
        a.grade = Grade { tonemap: Tonemap::Filmic, richness: 0.3, temperature: -0.2, vignette: 0.2, grain: 0.1, ..Default::default() };
        s.light(air(a));
    }
    s
}

/// Desert at noon.
pub fn desert_noon(cinematic: bool, w: u32, h: u32) -> Scene {
    let mut s = Scene::new(w, h);
    let m = s.m();
    s.camera([0.0, -1.7 * m, -4.0 * m], [0.0, -1.5 * m, 14.0 * m]);
    let ground = 0.0;
    s.add(studio_mesh::prim::plane(200.0 * m, 200.0 * m, 4), [0.0, ground, 60.0 * m], mat([0.78, 0.6, 0.38], 0.95));
    for (x, z, sx, sy, sz) in [(-14.0f32, 30.0f32, 14.0f32, 2.6f32, 9.0f32), (10.0, 40.0, 18.0, 3.6, 10.0), (0.0, 70.0, 40.0, 5.0, 14.0), (-30.0, 55.0, 22.0, 3.0, 12.0)] {
        s.add(studio_mesh::prim::uv_sphere(m, 24, 12), [x * m, ground, z * m], mat([0.82, 0.62, 0.38], 0.95));
        s.scale_last([sx, sy, sz]);
    }
    // A rock and a dead tree for scale.
    s.add(studio_mesh::prim::ico_sphere(0.8 * m, 2), [3.0 * m, ground - 0.2 * m, 9.0 * m], mat([0.4, 0.3, 0.24], 0.9));
    s.add(studio_mesh::prim::cylinder(0.06 * m, 0.1 * m, 2.4 * m, 6), [-2.5 * m, ground - 1.2 * m, 7.0 * m], mat([0.2, 0.15, 0.1], 0.9));
    s.light(sun_with_sky(12.4, 150.0, 2.6, 1.0, true));
    if cinematic {
        let mut a = Atmos { metre: m, ground_y: ground, ..Default::default() };
        a.fog = Some(Fog { density: 0.003, falloff: 0.03, base_y: ground, colour: [1.0, 0.85, 0.65], anisotropy: 0.3, range: 120.0, patchiness: 0.2 });
        a.weather[0] = Some(Weather { wind: [4.0, 0.0, 0.5], ..Weather::new(WeatherKind::Dust, 0.5) });
        a.bloom = Bloom { strength: 0.08, threshold: 1.2, knee: 0.5, radius: 0.5 };
        a.grade = Grade { tonemap: Tonemap::Filmic, richness: 0.45, contrast: 1.25, temperature: 0.12, saturation: 1.25, vignette: 0.15, grain: 0.05, shadow_tint: [0.2, 0.15, 0.35, 0.5], ..Default::default() };
        s.light(air(a));
    }
    s
}
