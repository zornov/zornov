// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Light, air and look for 3D scenes.
//!
//! A scene's lights can be made visible and its air can be filled; this
//! module is the one place that knows how, so every 3D renderer gets the
//! same result. It works on a finished picture plus its depth: the raster
//! renderer and the path tracer both hand over linear colour and the
//! distance to the first surface, and this pass adds what a camera would
//! have seen between the lens and the surface and in the lens itself.
//!
//! - **Practical lights** (bulbs, lamps, neon, candles, screens, windows)
//!   get a glow with a long soft falloff, an optional lens streak, and a
//!   visible cone or shaft when the air is not empty. A light that has a
//!   `flame` is drawn as a flickering flame, from a candle to a bonfire.
//! - **Air**: height fog (also patchy banks), light scattering through it
//!   from every practical (cones, shafts through windows and trees, which
//!   respect the lights' shadow maps), and **weather and particles**
//!   (rain, snow, dust, embers, sparks, leaves, ash, smoke) that are lit
//!   by the scene's lights, so rain shows where a lamp lights it and
//!   motes show in a beam.
//! - **Wet surfaces** reflect: ground and walls the shader marked wet
//!   reflect what is on screen and the practical lights, stretched along
//!   the vertical on rough wet ground, sharp in puddles; rain makes
//!   ripples on the ground.
//! - **Look**: bloom, exposure, a filmic tone map, contrast, saturation,
//!   white balance, rich shadows, vignette, grain, halation and fringing,
//!   all off or restrained until asked for.
//!
//! The CPU code ([`cpu`]) is the reference and the fallback; the GPU
//! version ([`gpu`], `atmosphere.wgsl`) runs the same algorithm over the
//! same packed parameters, and the tests keep the two together.
//!
//! Distances in the descriptions are metres; a metre is
//! [`Atmos::metre`] pixels of the scene (a person is about 1.75 m, half
//! the canvas height, unless the description says otherwise).

pub mod cpu;
pub mod gpu;
#[cfg(feature = "model")]
pub mod params;
pub mod samples;

use crate::space::{LightDesc, LightKind, V3};

/// Most visible lights in a picture.
pub const MAX_PRACTICALS: usize = 12;
/// Most weather layers in a picture.
pub const MAX_WEATHER: usize = 3;
/// Most lights with a shadow map used for shafts.
pub const MAX_SHADOWS: usize = 3;
/// Side of the shadow maps the air looks up (the raster renderer's are
/// reduced to this).
pub const SHADOW_SIZE: usize = 1024;

/// What a flame looks like.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flame {
    /// Height in metres (a candle 0.04, a campfire 0.8, a bonfire 2.5).
    pub height: f32,
    /// Width at the widest, metres.
    pub width: f32,
    /// How many tongues (1 for a candle, more for a bonfire).
    pub tongues: f32,
    /// Makes this flame's wavering its own.
    pub seed: f32,
}

impl Default for Flame {
    fn default() -> Flame {
        Flame { height: 0.1, width: 0.04, tongues: 1.0, seed: 0.0 }
    }
}

/// How a light shows itself (zero in every field: it does not).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct LightLook {
    /// Strength of the glow round the source.
    pub glow: f32,
    /// Strength of the horizontal lens streak.
    pub flare: f32,
    /// Strength of the cone or shaft its light makes in the air.
    pub beam: f32,
    /// Radius of the source, metres (0 = a small bulb).
    pub size: f32,
    /// Seconds of the scene (flames move, flickers have been applied).
    pub time: f32,
    pub flame: Option<Flame>,
    /// This light carries the scene's air and look (it is not a light).
    pub atmos: Option<Atmos>,
}

impl LightLook {
    /// Whether this light draws anything.
    pub fn visible(&self) -> bool {
        self.glow > 0.0 || self.flare > 0.0 || self.beam > 0.0 || self.flame.is_some()
    }
}

/// Height fog.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fog {
    /// Thickness at its base, per metre (0.01 thin haze, 0.05 mist,
    /// 0.2 dense fog).
    pub density: f32,
    /// How fast it thins with height, per metre (0 = the same all the
    /// way up, 0.5 = a bank a few metres deep).
    pub falloff: f32,
    /// World y of the fog's base (the ground).
    pub base_y: f32,
    /// Colour of the fogged light (linear).
    pub colour: [f32; 3],
    /// Forward scattering of light by the fog, −1..1 (0.4 = a halo round
    /// lamps looking toward them).
    pub anisotropy: f32,
    /// Farthest the fog is followed, metres.
    pub range: f32,
    /// Patchiness: 0 even, 1 banks with clear gaps.
    pub patchiness: f32,
}

impl Default for Fog {
    fn default() -> Fog {
        Fog { density: 0.03, falloff: 0.0, base_y: 0.0, colour: [0.5, 0.55, 0.62], anisotropy: 0.4, range: 80.0, patchiness: 0.0 }
    }
}

/// A kind of falling, floating or flying thing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeatherKind {
    Rain,
    Snow,
    Dust,
    Embers,
    Sparks,
    Leaves,
    Ash,
    Smoke,
}

impl WeatherKind {
    pub const ALL: [WeatherKind; 8] = [WeatherKind::Rain, WeatherKind::Snow, WeatherKind::Dust, WeatherKind::Embers, WeatherKind::Sparks, WeatherKind::Leaves, WeatherKind::Ash, WeatherKind::Smoke];

    pub fn name(self) -> &'static str {
        match self {
            WeatherKind::Rain => "rain",
            WeatherKind::Snow => "snow",
            WeatherKind::Dust => "dust",
            WeatherKind::Embers => "embers",
            WeatherKind::Sparks => "sparks",
            WeatherKind::Leaves => "leaves",
            WeatherKind::Ash => "ash",
            WeatherKind::Smoke => "smoke",
        }
    }

    pub fn parse(s: &str) -> Option<WeatherKind> {
        Some(match s.trim().to_lowercase().replace([' ', '-'], "_").as_str() {
            "rain" | "drizzle" | "downpour" | "storm" | "shower" => WeatherKind::Rain,
            "snow" | "snowfall" | "flurries" | "blizzard" => WeatherKind::Snow,
            "dust" | "motes" | "dust_motes" | "pollen" | "particles" | "specks" => WeatherKind::Dust,
            "embers" | "ember" | "cinders" | "firefly" | "fireflies" => WeatherKind::Embers,
            "sparks" | "spark" | "welding" => WeatherKind::Sparks,
            "leaves" | "leaf" | "falling_leaves" | "petals" | "autumn" => WeatherKind::Leaves,
            "ash" | "ashfall" | "ashes" | "fallout" => WeatherKind::Ash,
            "smoke" | "wisps" | "steam" | "haze_smoke" => WeatherKind::Smoke,
            _ => return None,
        })
    }

    /// The physical numbers of a kind.
    fn spec(self) -> Spec {
        match self {
            // Streaks: fast, long, bright where lit.
            WeatherKind::Rain => Spec { cell: 1.0, size: 0.006, vel: [0.0, 9.0, 0.0], streak: 0.02, ambient: 0.9, hg: 0.55, bright: 1.5, reach: 36.0, prob: 0.16, flutter: 0.0, hz: 0.0, mode: MODE_LIT, colour: [0.82, 0.88, 1.0] },
            WeatherKind::Snow => Spec { cell: 0.45, size: 0.013, vel: [0.0, 1.1, 0.0], streak: 0.0, ambient: 1.6, hg: 0.3, bright: 3.0, reach: 24.0, prob: 0.24, flutter: 0.18, hz: 0.7, mode: MODE_LIT, colour: [1.0, 1.0, 1.0] },
            WeatherKind::Dust => Spec { cell: 0.3, size: 0.0035, vel: [0.0, 0.03, 0.0], streak: 0.0, ambient: 0.03, hg: 0.6, bright: 3.0, reach: 9.0, prob: 0.07, flutter: 0.05, hz: 0.3, mode: MODE_LIT, colour: [1.0, 0.96, 0.88] },
            WeatherKind::Embers => Spec { cell: 0.55, size: 0.012, vel: [0.0, -0.9, 0.0], streak: 0.05, ambient: 0.0, hg: 0.0, bright: 9.0, reach: 14.0, prob: 0.07, flutter: 0.25, hz: 1.1, mode: MODE_EMIT, colour: [1.0, 0.42, 0.08] },
            WeatherKind::Sparks => Spec { cell: 0.5, size: 0.006, vel: [0.0, 3.5, 0.0], streak: 0.06, ambient: 0.0, hg: 0.0, bright: 14.0, reach: 12.0, prob: 0.05, flutter: 0.0, hz: 0.0, mode: MODE_EMIT, colour: [1.0, 0.7, 0.28] },
            WeatherKind::Leaves => Spec { cell: 0.9, size: 0.05, vel: [0.0, 1.3, 0.0], streak: 0.0, ambient: 1.2, hg: 0.1, bright: 1.0, reach: 20.0, prob: 0.07, flutter: 0.35, hz: 0.6, mode: MODE_OPAQUE, colour: [0.62, 0.36, 0.1] },
            WeatherKind::Ash => Spec { cell: 0.4, size: 0.011, vel: [0.0, 0.45, 0.0], streak: 0.0, ambient: 1.3, hg: 0.2, bright: 1.6, reach: 20.0, prob: 0.3, flutter: 0.12, hz: 0.5, mode: MODE_LIT, colour: [0.55, 0.53, 0.5] },
            WeatherKind::Smoke => Spec { cell: 0.7, size: 0.33, vel: [0.0, -0.55, 0.0], streak: 0.0, ambient: 1.0, hg: 0.3, bright: 0.55, reach: 16.0, prob: 0.12, flutter: 0.25, hz: 0.25, mode: MODE_SOFT, colour: [0.55, 0.55, 0.56] },
        }
    }
}

const MODE_LIT: f32 = 0.0;
const MODE_EMIT: f32 = 1.0;
const MODE_OPAQUE: f32 = 2.0;
const MODE_SOFT: f32 = 3.0;

#[derive(Clone, Copy)]
struct Spec {
    cell: f32,
    size: f32,
    vel: V3,
    streak: f32,
    ambient: f32,
    hg: f32,
    bright: f32,
    reach: f32,
    prob: f32,
    flutter: f32,
    hz: f32,
    mode: f32,
    colour: [f32; 3],
}

/// One layer of weather.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Weather {
    pub kind: WeatherKind,
    /// 0..1 (1 = a downpour, a blizzard, a thick swarm).
    pub amount: f32,
    /// Wind, metres per second (x right, y down, z away).
    pub wind: V3,
    /// Multiplies the size of the particles.
    pub size: f32,
    /// Multiplies how fast they move.
    pub speed: f32,
    /// A box (world corners) it is limited to: smoke from a chimney,
    /// embers over a fire. `None` fills the air round the camera.
    pub zone: Option<(V3, V3)>,
    /// Own colour (linear); `None` uses the kind's.
    pub colour: Option<[f32; 3]>,
    /// Rain on the ground: ripples and splashes (0..1).
    pub splash: f32,
}

impl Weather {
    pub fn new(kind: WeatherKind, amount: f32) -> Weather {
        Weather { kind, amount, wind: [0.0; 3], size: 1.0, speed: 1.0, zone: None, colour: None, splash: if kind == WeatherKind::Rain { 1.0 } else { 0.0 } }
    }
}

/// Tone mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tonemap {
    /// Left as it is (anything above 1 clips).
    None,
    /// A filmic S-curve: rich blacks, highlights that roll off to white
    /// keeping their colour.
    Filmic,
    /// A gentle shoulder only (flat looks, comedy, documentary).
    Soft,
}

impl Tonemap {
    pub fn parse(s: &str) -> Option<Tonemap> {
        Some(match s.trim().to_lowercase().as_str() {
            "none" | "off" | "linear" | "" => Tonemap::None,
            "filmic" | "aces" | "film" | "cinematic" => Tonemap::Filmic,
            "soft" | "reinhard" | "gentle" | "shoulder" => Tonemap::Soft,
            _ => return None,
        })
    }
    pub fn name(self) -> &'static str {
        match self {
            Tonemap::None => "none",
            Tonemap::Filmic => "filmic",
            Tonemap::Soft => "soft",
        }
    }
}

/// Bloom: bright light spilling over its surroundings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bloom {
    /// 0 = none.
    pub strength: f32,
    /// Brightness (linear) from which it spills.
    pub threshold: f32,
    /// Softness of the threshold.
    pub knee: f32,
    /// 0..1: how far it reaches.
    pub radius: f32,
}

impl Default for Bloom {
    fn default() -> Bloom {
        Bloom { strength: 0.0, threshold: 0.9, knee: 0.6, radius: 0.5 }
    }
}

/// Exposure, colour and lens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grade {
    /// Stops.
    pub exposure: f32,
    /// 1 = none.
    pub contrast: f32,
    /// 1 = none.
    pub saturation: f32,
    pub tonemap: Tonemap,
    /// −1 cool … 1 warm.
    pub temperature: f32,
    /// −1 green … 1 magenta.
    pub tint: f32,
    /// 0..1: deepens the darks and keeps their colour (no muddy grey).
    pub richness: f32,
    /// Colour pushed into the shadows (rgb) and how much.
    pub shadow_tint: [f32; 4],
    /// Colour pushed into the highlights and how much.
    pub highlight_tint: [f32; 4],
    pub vignette: f32,
    pub grain: f32,
    /// Warm glow round highlights (film halation), 0..1.
    pub halation: f32,
    /// Colour fringing at the edges of the frame, 0..1.
    pub fringe: f32,
    /// Metering: the exposure is lowered, up to 2.5 stops, when the
    /// brightest 1% of the picture is beyond what the tone map can show,
    /// so a bright sky, a lit street or lamps seen through fog do not
    /// blow out; `exposure` adds to it. Never brightens.
    pub auto_exposure: bool,
}

impl Default for Grade {
    fn default() -> Grade {
        Grade { exposure: 0.0, contrast: 1.0, saturation: 1.0, tonemap: Tonemap::None, temperature: 0.0, tint: 0.0, richness: 0.0, shadow_tint: [0.0; 4], highlight_tint: [0.0; 4], vignette: 0.0, grain: 0.0, halation: 0.0, fringe: 0.0, auto_exposure: true }
    }
}

impl Grade {
    /// Whether the finishing step has anything to do.
    pub fn active(&self) -> bool {
        *self != Grade::default()
    }
}

/// The scene's air and look, carried by one light-less light clip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Atmos {
    /// Pixels in a metre (0 = the canvas height over 3.5).
    pub metre: f32,
    /// World y of the ground (rain ripples and splashes, wet reflections).
    pub ground_y: f32,
    pub fog: Option<Fog>,
    pub weather: [Option<Weather>; MAX_WEATHER],
    pub bloom: Bloom,
    pub grade: Grade,
    /// What a mirror sees where nothing is on screen (the sky): linear
    /// RGB; zero uses the fog colour or the ambient light.
    pub reflection: [f32; 3],
    /// Ambient light the air takes (linear); zero takes the scene's ambient
    /// lights.
    pub ambient: [f32; 3],
}

impl Default for Atmos {
    fn default() -> Atmos {
        Atmos { metre: 0.0, ground_y: 0.0, fog: None, weather: [None; MAX_WEATHER], bloom: Bloom::default(), grade: Grade::default(), reflection: [0.0; 3], ambient: [0.0; 3] }
    }
}

/// Everything the pass needs about the lens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lens {
    pub position: V3,
    /// Rows: right, down, forward.
    pub axes: [V3; 3],
    /// Pixels per unit of x/z at the picture's size.
    pub zoom: f32,
    pub centre: [f32; 2],
}

/// A light's depth map, for light shafts that stop at what blocks them.
#[derive(Clone, Debug)]
pub struct ShadowVolume {
    /// Which of the scene's lights it belongs to.
    pub light: usize,
    pub position: V3,
    pub axes: [V3; 3],
    pub zoom: f32,
    pub centre: [f32; 2],
    pub ortho: bool,
    /// World size of a texel (at unit distance when perspective).
    pub texel: f32,
    /// [`SHADOW_SIZE`]² distances along the light's axis.
    pub depth: Vec<f32>,
}

/// What a picture is made of: the colour and where the surfaces are.
pub struct Picture<'a> {
    pub width: usize,
    pub height: usize,
    /// Distance along the lens axis to the first surface (infinite for none).
    pub depth: &'a [f32],
    /// Normal (xyz) and how wet and reflective (w) each surface is; empty
    /// for none.
    pub surface: &'a [[f32; 4]],
}

/// What the air and look pass will do, worked out from the scene's lights.
#[derive(Clone, Debug)]
pub struct Plan {
    /// Packed parameters, read alike by [`cpu`] and [`gpu`].
    pub params: Vec<[f32; 4]>,
    /// Shadow maps for the air, laid out as [`gpu`] reads them.
    pub shadows: Vec<f32>,
    pub stages: Stages,
}

/// Which parts have work to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Stages {
    pub sprites: bool,
    pub reflect: bool,
    pub air: bool,
    pub bloom: bool,
    pub finish: bool,
}

impl Stages {
    pub fn any(&self) -> bool {
        self.sprites || self.reflect || self.air || self.bloom || self.finish
    }
}

// Packed layout (vec4 indices).
pub const P_HEAD: usize = 0;
pub const P_FOG: usize = 6;
pub const P_PRACTICAL: usize = 20;
pub const PRACTICAL_STRIDE: usize = 7;
pub const P_WEATHER: usize = P_PRACTICAL + MAX_PRACTICALS * PRACTICAL_STRIDE;
pub const WEATHER_STRIDE: usize = 7;
pub const P_END: usize = P_WEATHER + MAX_WEATHER * WEATHER_STRIDE;
/// Floats in a shadow map's header.
pub const SHADOW_HEADER: usize = 24;

/// Metres to pixels for a scene of this height.
pub fn default_metre(canvas_height: f32) -> f32 {
    canvas_height / 3.5
}

/// Reads the scene's lights: the air and look one carries, the visible
/// ones among the rest. `None` when nothing in them asks for anything.
pub fn plan(lights: &[LightDesc], lens: &Lens, pic: &Picture, shadows: &[ShadowVolume], canvas_height: f32) -> Option<Plan> {
    let atmos = lights.iter().find_map(|l| l.look.and_then(|k| k.atmos));
    // Visible lights first, then the rest (they still light the air and
    // the particles).
    let mut practicals: Vec<(usize, &LightDesc)> = lights.iter().enumerate().filter(|(_, l)| l.kind != LightKind::Ambient && l.look.is_some_and(|k| k.visible())).collect();
    let any_visible = !practicals.is_empty();
    practicals.extend(lights.iter().enumerate().filter(|(_, l)| l.kind != LightKind::Ambient && !l.look.is_some_and(|k| k.visible())));
    practicals.truncate(MAX_PRACTICALS);
    if atmos.is_none() && !any_visible {
        return None;
    }
    let mut a = atmos.unwrap_or_default();
    if atmos.is_none() {
        // Visible lights on their own get a little bloom to carry the glow.
        a.bloom.strength = 0.12;
        a.bloom.threshold = 0.8;
        a.grade.tonemap = Tonemap::Soft;
    }
    let (w, h) = (pic.width as f32, pic.height as f32);
    let metre = if a.metre > 0.0 { a.metre } else { default_metre(canvas_height) };
    let time = lights.iter().filter_map(|l| l.look).map(|k| k.time).fold(0.0, f32::max);
    let mut p = vec![[0.0f32; 4]; P_END];
    p[0] = [w, h, time, metre];
    p[1] = [lens.position[0], lens.position[1], lens.position[2], lens.zoom];
    p[2] = [lens.axes[0][0], lens.axes[0][1], lens.axes[0][2], lens.centre[0]];
    p[3] = [lens.axes[1][0], lens.axes[1][1], lens.axes[1][2], lens.centre[1]];
    p[4] = [lens.axes[2][0], lens.axes[2][1], lens.axes[2][2], 400.0 * metre];
    // Ambient light the air and the particles take.
    let mut ambient = a.ambient;
    if ambient == [0.0; 3] {
        for l in lights.iter().filter(|l| l.kind == LightKind::Ambient) {
            ambient = std::array::from_fn(|i| ambient[i] + l.colour[i]);
        }
        if let Some(sk) = lights.iter().find_map(|l| l.sky) {
            let irr = crate::sky::Irradiance::of(&sk).at([0.0, -1.0, 0.0]);
            ambient = std::array::from_fn(|i| ambient[i] + irr[i] * 0.5);
        }
    }
    let mut reflection = a.reflection;
    if reflection == [0.0; 3] {
        reflection = match (lights.iter().find_map(|l| l.sky), a.fog) {
            (Some(sk), _) => crate::sky::radiance(&sk, [0.0, -0.05, 1.0]),
            (None, Some(f)) => std::array::from_fn(|i| f.colour[i] * (ambient[i] + 0.02)),
            _ => std::array::from_fn(|i| ambient[i] * 0.6),
        };
    }
    let mut flags = 0u32;
    let mut stages = Stages::default();
    if let Some(f) = a.fog.filter(|f| f.density > 0.0) {
        flags |= 1;
        p[P_FOG] = [f.density, f.falloff, (f.base_y), f.anisotropy.clamp(-0.95, 0.95)];
        p[P_FOG + 1] = [f.colour[0], f.colour[1], f.colour[2], f.range.max(1.0)];
        stages.air = true;
    } else {
        p[P_FOG + 1] = [0.0, 0.0, 0.0, 60.0];
    }
    p[P_FOG + 2] = [ambient[0], ambient[1], ambient[2], a.fog.map_or(0.0, |f| f.patchiness.clamp(0.0, 1.0))];
    p[P_FOG + 3] = [reflection[0], reflection[1], reflection[2], a.ground_y];
    let b = a.bloom;
    if b.strength > 0.0 {
        flags |= 2;
        stages.bloom = true;
    }
    p[P_FOG + 4] = [b.strength, b.threshold, b.knee.max(0.01), b.radius.clamp(0.0, 1.0)];
    let g = a.grade;
    // Metering needs the finishing step to apply what it finds.
    let auto = g.auto_exposure && (atmos.is_some() || any_visible);
    if g.active() || auto {
        flags |= 4;
        stages.finish = true;
    }
    if b.strength > 0.0 {
        stages.finish = true;
    }
    p[P_FOG + 5] = [g.exposure, g.contrast, g.saturation, match g.tonemap { Tonemap::None => 0.0, Tonemap::Filmic => 1.0, Tonemap::Soft => 2.0 }];
    p[P_FOG + 6] = [g.temperature, g.tint, g.richness, g.vignette];
    p[P_FOG + 7] = [g.grain, g.halation, g.fringe, if auto { 1.0 } else { 0.0 }];
    p[P_FOG + 8] = g.shadow_tint;
    p[P_FOG + 9] = g.highlight_tint;
    // Practical lights.
    let mut shadow_floats: Vec<f32> = Vec::new();
    let mut shadow_slot: Vec<(usize, usize)> = Vec::new();
    for s in shadows.iter().take(MAX_SHADOWS) {
        let off = shadow_floats.len();
        shadow_floats.resize(off + SHADOW_HEADER, 0.0);
        shadow_floats[off..off + 4].copy_from_slice(&[s.position[0], s.position[1], s.position[2], s.zoom]);
        shadow_floats[off + 4..off + 8].copy_from_slice(&[s.axes[0][0], s.axes[0][1], s.axes[0][2], s.centre[0]]);
        shadow_floats[off + 8..off + 12].copy_from_slice(&[s.axes[1][0], s.axes[1][1], s.axes[1][2], s.centre[1]]);
        shadow_floats[off + 12..off + 16].copy_from_slice(&[s.axes[2][0], s.axes[2][1], s.axes[2][2], if s.ortho { 1.0 } else { 0.0 }]);
        shadow_floats[off + 16..off + 20].copy_from_slice(&[SHADOW_SIZE as f32, s.texel, (off + SHADOW_HEADER) as f32, 0.0]);
        shadow_floats.extend_from_slice(&s.depth);
        shadow_slot.push((s.light, off));
    }
    let mut n = 0usize;
    for (li, l) in &practicals {
        let k = l.look.unwrap_or_default();
        let at = P_PRACTICAL + n * PRACTICAL_STRIDE;
        let maxc = l.colour[0].max(l.colour[1]).max(l.colour[2]);
        let size_m = if k.size > 0.0 { k.size } else if k.flame.is_some() { 0.05 } else { 0.07 };
        let kind = match l.kind {
            LightKind::Point => 0.0,
            LightKind::Spot => 1.0,
            _ => 2.0,
        };
        p[at] = [l.position[0], l.position[1], l.position[2], kind];
        p[at + 1] = [l.colour[0], l.colour[1], l.colour[2], size_m * metre];
        p[at + 2] = [l.direction[0], l.direction[1], l.direction[2], l.cone[0]];
        p[at + 3] = [l.cone[1], k.glow, k.flare, k.beam];
        if let Some(f) = k.flame {
            p[at + 4] = [f.height * metre, f.width * metre, f.seed, f.tongues.max(1.0)];
            stages.sprites = true;
        }
        // Where its light has faded to half: a light with a falloff fades as it
        // lights surfaces; one without reaches far (a spot's cone is the limit).
        let reach = if l.falloff > 0.0 {
            (l.radius + l.falloff * 0.5).max(0.3 * metre)
        } else if l.kind == LightKind::Spot {
            20.0 * metre
        } else {
            4.0 * metre * maxc.max(0.05).sqrt()
        };
        let slot = shadow_slot.iter().find(|(idx, _)| idx == li).map_or(-1.0, |(_, off)| *off as f32);
        let vis = if k.glow > 0.0 || k.flare > 0.0 || k.flame.is_some() { visibility(&p[at], &p[at + 4], lens, pic, w, h, metre) } else { 1.0 };
        p[at + 5] = [vis, slot, reach, 0.0];
        p[at + 6] = [l.radius, l.falloff, 0.0, 0.0];
        if k.glow > 0.0 || k.flare > 0.0 {
            stages.sprites = true;
        }
        if k.beam > 0.0 {
            flags |= 1;
            stages.air = true;
        }
        n += 1;
    }
    // Weather.
    let mut nw = 0usize;
    for wx in a.weather.iter().flatten() {
        if wx.amount <= 0.0 {
            continue;
        }
        let s = wx.kind.spec();
        let at = P_WEATHER + nw * WEATHER_STRIDE;
        let c = wx.colour.unwrap_or(s.colour);
        let speed = wx.speed.max(0.0);
        let prob = (s.prob * wx.amount.clamp(0.0, 1.5)).min(0.9);
        let size = s.size * wx.size.max(0.05);
        p[at] = [s.mode, prob, s.cell * metre.max(1.0) * (wx.size.max(0.05)).sqrt().clamp(0.7, 2.0), size * metre];
        p[at + 1] = [(s.vel[0] + wx.wind[0]) * speed * metre, (s.vel[1] * speed + wx.wind[1]) * metre, (s.vel[2] + wx.wind[2]) * speed * metre, s.streak];
        p[at + 2] = [c[0], c[1], c[2], s.bright];
        p[at + 3] = [s.flutter * metre, s.hz, s.reach * metre, s.ambient];
        let (zlo, zhi, zon) = match wx.zone {
            Some((lo, hi)) => (lo, hi, 1.0),
            None => ([0.0; 3], [0.0; 3], 0.0),
        };
        p[at + 4] = [zlo[0], zlo[1], zlo[2], zon];
        p[at + 5] = [zhi[0], zhi[1], zhi[2], wx.splash];
        p[at + 6] = [s.hg, 0.0, 0.0, 0.0];
        nw += 1;
        stages.air = true;
        if wx.splash > 0.0 && wx.zone.is_none() {
            flags |= 8;
        }
    }
    if !pic.surface.is_empty() {
        flags |= 32;
    }
    // Wet surfaces reflect when the renderer marked any.
    if !pic.surface.is_empty() && pic.surface.iter().any(|s| s[3] > 0.01) {
        stages.reflect = true;
        flags |= 16;
    }
    p[5] = [n as f32, nw as f32, shadow_slot.len() as f32, flags as f32];
    if !stages.any() {
        return None;
    }
    Some(Plan { params: p, shadows: shadow_floats, stages })
}

/// How much of a light's source the depth leaves in view (1 clear, 0
/// hidden): taps round its screen position.
fn visibility(pos: &[f32; 4], flame: &[f32; 4], lens: &Lens, pic: &Picture, w: f32, h: f32, metre: f32) -> f32 {
    let d = [pos[0] - lens.position[0], pos[1] - lens.position[1], pos[2] - lens.position[2]];
    let z = d[0] * lens.axes[2][0] + d[1] * lens.axes[2][1] + d[2] * lens.axes[2][2];
    if z <= crate::space::NEAR {
        return 0.0;
    }
    let sx = lens.centre[0] + lens.zoom * (d[0] * lens.axes[0][0] + d[1] * lens.axes[0][1] + d[2] * lens.axes[0][2]) / z;
    let sy = lens.centre[1] + lens.zoom * (d[0] * lens.axes[1][0] + d[1] * lens.axes[1][1] + d[2] * lens.axes[1][2]) / z;
    let _ = flame;
    let r = (0.25 * metre * lens.zoom / z).clamp(2.0, 14.0);
    // Allow for the lamp's own housing standing round the point.
    let bias = (0.35 * metre).max(2.0);
    let mut seen = 0.0;
    let taps = [(0.0, 0.0), (1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0), (0.7, 0.7), (-0.7, -0.7), (0.7, -0.7), (-0.7, 0.7)];
    for (dx, dy) in taps {
        let x = sx + dx * r;
        let y = sy + dy * r;
        if x < 0.0 || y < 0.0 || x >= w || y >= h {
            // Off the picture: unknown; count as seen so a lamp just out of
            // frame still glows into it.
            seen += 1.0;
            continue;
        }
        let dz = pic.depth[(y as usize).min(pic.height - 1) * pic.width + (x as usize).min(pic.width - 1)];
        seen += ((dz - (z - 2.0 * bias)) / bias).clamp(0.0, 1.0);
    }
    seen / taps.len() as f32
}

impl Plan {
    /// Whether the exposure is metered from the picture before the finish.
    pub fn metered(&self) -> bool {
        self.stages.finish && self.params[P_FOG + 7][3] > 0.5
    }

    /// This plan with `stops` added to the exposure.
    pub fn exposed(&self, stops: f32) -> Plan {
        let mut p = self.clone();
        p.params[P_FOG + 5][0] += stops;
        p
    }
}

/// How many stops to take off the exposure of `colour` (premultiplied
/// linear) for a tone map (0 none, 1 filmic, 2 soft): 0 when all but the
/// brightest 3% of it sits within what the tone map shows (a lamp, a glint
/// or a sun disc may be hot; a sky, a wall or a fogged street may not),
/// else the stops that bring that 3% down to it (at most 3).
pub fn meter(colour: &[[f32; 4]], tonemap: f32) -> f32 {
    const BINS: usize = 96;
    let mut hist = [0u32; BINS];
    let mut n = 0u32;
    for p in colour {
        if p[3] < 0.05 {
            continue;
        }
        let l = 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
        let b = if l <= 1e-4 { 0 } else { ((l.log2() + 10.0) * 4.0).clamp(0.0, (BINS - 1) as f32) as usize };
        hist[b] += 1;
        n += 1;
    }
    if n < 16 {
        return 0.0;
    }
    let want = (n as f32 * 0.97) as u32;
    let mut acc = 0;
    let mut bin = BINS - 1;
    for (i, c) in hist.iter().enumerate() {
        acc += c;
        if acc > want {
            bin = i;
            break;
        }
    }
    let hi = ((bin as f32 + 0.5) / 4.0 - 10.0).exp2();
    let target = match tonemap as i32 {
        1 => 1.1,
        2 => 0.95,
        _ => 0.9,
    };
    if hi <= target { 0.0 } else { (-(hi / target).log2()).max(-3.0) }
}

/// Runs the plan over `colour` (premultiplied linear, w×h), on the GPU
/// when `gpu` allows and one is there, else on the CPU.
pub fn run(plan: &Plan, pic: &Picture, colour: &mut [[f32; 4]], gpu: bool) {
    if gpu && gpu::run(plan, pic, colour) {
        return;
    }
    cpu::run(plan, pic, colour);
}
