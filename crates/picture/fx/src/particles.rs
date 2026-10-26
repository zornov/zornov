// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Particles: rain, snow, falling leaves, dust motes, fireflies, sparks,
//! smoke, mist, splashes, bubbles, confetti, magic dust, clouds, glints on
//! water, stars and embers.
//!
//! The system is stateless: particle `k` is born at `k / rate` seconds
//! (with jitter) and everything about it (where it starts, how it moves,
//! how big and what colour it is) is a hash of `k` and the seed. Its place
//! at any moment is a closed-form function of its age: a drag-limited
//! fall towards the wind's speed under gravity, plus sway and noise
//! turbulence, landing on a ground line that rises towards the horizon
//! with depth. So scrubbing, rendering on another machine and rendering
//! frames out of order all give the same picture, and the system has
//! always been running (the frame is full of rain from the first frame).
//!
//! Drawing is tiled: the live particles, far to near, are binned into
//! 16-pixel tiles and every pixel composites only the particles of its
//! tile. [`pack`] writes particles and tiles into one `u32` buffer; the
//! CPU path ([`draw_packed`]) and the GPU shader (`wgsl/particles.wgsl`)
//! both draw from that same buffer with the same sprite formulas.
//!
//! Units: lengths in percent of frame height, speeds in percent of frame
//! height per second, positions as fractions of the frame (0,0 top left).

use crate::image::{over, premultiply, Image};
use crate::math::{hash2, hsv_to_rgb, noise1, value_noise};
use crate::params::ParamSet;

/// How a particle is drawn. The code is what the shader switches on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sprite {
    /// A soft round dot (dust, fireflies).
    Dot = 0,
    /// A line along the motion (rain).
    Streak = 1,
    /// A soft dot with a brighter core (snow).
    Flake = 2,
    /// A turning ellipse with a shaded side (leaves).
    Leaf = 3,
    /// A bright streak (sparks).
    Spark = 4,
    /// A large soft blob with a noise texture (smoke, mist, clouds).
    Puff = 5,
    /// A thin ring with a highlight (bubbles).
    Bubble = 6,
    /// A tumbling rectangle (confetti).
    Confetti = 7,
    /// A glowing point with a cross glare (magic dust, stars, glints).
    Star = 8,
    /// A flattened ring that spreads and fades (splashes).
    Ring = 9,
}

impl Sprite {
    pub const ALL: [Sprite; 10] = [Sprite::Dot, Sprite::Streak, Sprite::Flake, Sprite::Leaf, Sprite::Spark, Sprite::Puff, Sprite::Bubble, Sprite::Confetti, Sprite::Star, Sprite::Ring];

    fn from_code(c: u32) -> Sprite {
        Sprite::ALL.get(c as usize).copied().unwrap_or(Sprite::Dot)
    }
}

/// Where new particles appear.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Emitter {
    /// Just above the area's top edge, falling in (widened upwind so the
    /// wind never leaves a gap).
    Top,
    /// Anywhere inside the area.
    Area,
    /// The area's centre.
    Point,
    /// Just below the area's bottom edge, rising in.
    Bottom,
}

/// What a particle does when it reaches the ground line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnGround {
    Pass,
    /// Lies where it landed until it fades (snow, leaves).
    Stop,
    /// Hops back up, losing most of its speed, then stops (sparks).
    Bounce,
    /// Becomes a short spreading ring (rain).
    Splash,
}

/// Everything that makes rain rain.
#[derive(Clone, Copy, Debug)]
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub sprite: Sprite,
    pub emitter: Emitter,
    /// Default area, `[x0, y0, x1, y1]` as frame fractions.
    pub area: [f32; 4],
    /// Particles per second for the default area.
    pub rate: f32,
    /// Life in seconds, shortest and longest.
    pub life: (f32, f32),
    /// Start speed range (% height per second).
    pub speed: (f32, f32),
    /// Direction of the start velocity in degrees (0 right, 90 down).
    pub direction: f32,
    /// Spread of the direction in degrees (360 = any way).
    pub spread: f32,
    /// Downward acceleration (% height / s²); negative rises.
    pub gravity: f32,
    /// Air drag (1/s): with drag, particles settle to the wind's speed
    /// plus gravity / drag.
    pub drag: f32,
    /// Horizontal air speed (% height per second).
    pub wind: f32,
    /// Noise wander (% height) and how fast it changes (Hz).
    pub turbulence: f32,
    pub turbulence_hz: f32,
    /// Side-to-side sway (% height) and its frequency (Hz).
    pub sway: f32,
    pub sway_hz: f32,
    /// Spin range in degrees per second (either way).
    pub spin: (f32, f32),
    /// Size range (% height): the radius, or a streak's half width.
    pub size: (f32, f32),
    /// Size at the end of life relative to the start.
    pub grow: f32,
    /// Straight linear colours; each particle picks between them (or,
    /// with `colour_over_life`, moves from the first to the second).
    pub colour_a: [f32; 4],
    pub colour_b: [f32; 4],
    pub colour_over_life: bool,
    /// Every particle its own bright hue (confetti).
    pub rainbow: bool,
    /// Fade in and out, as fractions of the life.
    pub fade_in: f32,
    pub fade_out: f32,
    /// Brightness flicker frequency (Hz); 0 = steady.
    pub twinkle: f32,
    /// Light that adds rather than covers.
    pub glow: bool,
    pub on_ground: OnGround,
    /// How far back particles spread (0 = all on the nearest plane, 1 =
    /// out to a third of the size and speed).
    pub depth: f32,
    /// Streak length as seconds of motion.
    pub streak: f32,
    /// Minor axis over major axis (leaves, splash rings).
    pub aspect: f32,
}

const BASE: Preset = Preset {
    id: "",
    name: "",
    sprite: Sprite::Dot,
    emitter: Emitter::Area,
    area: [0.0, 0.0, 1.0, 1.0],
    rate: 10.0,
    life: (2.0, 3.0),
    speed: (0.0, 0.0),
    direction: 90.0,
    spread: 0.0,
    gravity: 0.0,
    drag: 0.0,
    wind: 0.0,
    turbulence: 0.0,
    turbulence_hz: 0.3,
    sway: 0.0,
    sway_hz: 0.3,
    spin: (0.0, 0.0),
    size: (0.2, 0.4),
    grow: 1.0,
    colour_a: [1.0, 1.0, 1.0, 1.0],
    colour_b: [1.0, 1.0, 1.0, 1.0],
    colour_over_life: false,
    rainbow: false,
    fade_in: 0.1,
    fade_out: 0.2,
    twinkle: 0.0,
    glow: false,
    on_ground: OnGround::Pass,
    depth: 0.6,
    streak: 0.0,
    aspect: 1.0,
};

/// The presets, in menu order (the `preset` parameter's choices).
pub static PRESETS: &[Preset] = &[
    Preset {
        id: "rain",
        name: "Rain",
        sprite: Sprite::Streak,
        emitter: Emitter::Top,
        area: [0.0, 0.0, 1.0, 1.0],
        rate: 700.0,
        life: (1.6, 2.2),
        speed: (150.0, 200.0),
        direction: 94.0,
        spread: 2.0,
        size: (0.07, 0.11),
        colour_a: [0.7, 0.75, 0.85, 0.5],
        colour_b: [0.85, 0.88, 0.95, 0.7],
        fade_in: 0.0,
        fade_out: 0.05,
        on_ground: OnGround::Splash,
        depth: 1.0,
        streak: 0.045,
        aspect: 0.35,
        ..BASE
    },
    Preset {
        id: "snow",
        name: "Snow",
        sprite: Sprite::Flake,
        emitter: Emitter::Top,
        rate: 120.0,
        life: (8.0, 12.0),
        speed: (7.0, 13.0),
        direction: 90.0,
        spread: 20.0,
        turbulence: 3.0,
        turbulence_hz: 0.25,
        sway: 1.5,
        sway_hz: 0.3,
        size: (0.25, 0.6),
        colour_a: [0.92, 0.95, 1.0, 0.85],
        colour_b: [1.0, 1.0, 1.0, 0.95],
        fade_in: 0.0,
        fade_out: 0.15,
        on_ground: OnGround::Stop,
        depth: 1.0,
        ..BASE
    },
    Preset {
        id: "leaves",
        name: "Falling leaves",
        sprite: Sprite::Leaf,
        emitter: Emitter::Top,
        rate: 5.0,
        life: (8.0, 12.0),
        speed: (8.0, 13.0),
        direction: 98.0,
        spread: 25.0,
        turbulence: 3.0,
        turbulence_hz: 0.2,
        sway: 5.0,
        sway_hz: 0.25,
        spin: (60.0, 200.0),
        size: (1.0, 1.8),
        colour_a: [0.55, 0.16, 0.02, 1.0],
        colour_b: [0.5, 0.33, 0.03, 1.0],
        fade_in: 0.0,
        fade_out: 0.12,
        on_ground: OnGround::Stop,
        depth: 0.8,
        aspect: 0.5,
        ..BASE
    },
    Preset {
        id: "dust",
        name: "Dust motes",
        sprite: Sprite::Dot,
        emitter: Emitter::Area,
        rate: 20.0,
        life: (6.0, 10.0),
        speed: (0.5, 2.0),
        direction: 0.0,
        spread: 360.0,
        turbulence: 2.0,
        turbulence_hz: 0.15,
        size: (0.08, 0.22),
        colour_a: [1.0, 0.88, 0.65, 0.55],
        colour_b: [1.0, 0.95, 0.85, 0.7],
        fade_in: 0.3,
        fade_out: 0.3,
        twinkle: 0.4,
        glow: true,
        depth: 1.0,
        ..BASE
    },
    Preset {
        id: "fireflies",
        name: "Fireflies",
        sprite: Sprite::Dot,
        emitter: Emitter::Area,
        area: [0.0, 0.45, 1.0, 1.0],
        rate: 3.0,
        life: (6.0, 10.0),
        speed: (1.0, 3.0),
        direction: 0.0,
        spread: 360.0,
        turbulence: 6.0,
        turbulence_hz: 0.2,
        size: (0.45, 0.8),
        colour_a: [0.7, 1.0, 0.2, 1.0],
        colour_b: [1.0, 0.95, 0.35, 1.0],
        fade_in: 0.2,
        fade_out: 0.25,
        twinkle: 0.5,
        glow: true,
        depth: 0.8,
        ..BASE
    },
    Preset {
        id: "sparks",
        name: "Sparks",
        sprite: Sprite::Spark,
        emitter: Emitter::Point,
        area: [0.45, 0.85, 0.55, 0.95],
        rate: 50.0,
        life: (0.6, 1.4),
        speed: (35.0, 75.0),
        direction: -90.0,
        spread: 60.0,
        gravity: 70.0,
        drag: 0.6,
        size: (0.08, 0.14),
        colour_a: [3.0, 1.6, 0.45, 1.0],
        colour_b: [0.9, 0.18, 0.02, 1.0],
        colour_over_life: true,
        fade_in: 0.0,
        fade_out: 0.3,
        glow: true,
        on_ground: OnGround::Bounce,
        depth: 0.3,
        streak: 0.04,
        ..BASE
    },
    Preset {
        id: "smoke",
        name: "Smoke",
        sprite: Sprite::Puff,
        emitter: Emitter::Point,
        area: [0.45, 0.8, 0.55, 0.9],
        rate: 6.0,
        life: (4.0, 6.0),
        speed: (6.0, 10.0),
        direction: -90.0,
        spread: 20.0,
        gravity: -2.0,
        drag: 0.3,
        turbulence: 4.0,
        turbulence_hz: 0.3,
        spin: (5.0, 20.0),
        size: (3.0, 5.0),
        grow: 3.0,
        colour_a: [0.22, 0.22, 0.24, 0.35],
        colour_b: [0.3, 0.3, 0.32, 0.3],
        fade_in: 0.15,
        fade_out: 0.6,
        depth: 0.3,
        ..BASE
    },
    Preset {
        id: "mist",
        name: "Mist",
        sprite: Sprite::Puff,
        emitter: Emitter::Area,
        area: [0.0, 0.55, 1.0, 0.95],
        rate: 4.0,
        life: (10.0, 16.0),
        speed: (1.0, 2.5),
        direction: 0.0,
        spread: 20.0,
        turbulence: 1.5,
        turbulence_hz: 0.1,
        spin: (2.0, 6.0),
        size: (8.0, 14.0),
        grow: 1.3,
        colour_a: [0.85, 0.88, 0.92, 0.12],
        colour_b: [0.9, 0.92, 0.95, 0.16],
        fade_in: 0.3,
        fade_out: 0.4,
        depth: 0.6,
        ..BASE
    },
    Preset {
        id: "splashes",
        name: "Splashes",
        sprite: Sprite::Ring,
        emitter: Emitter::Area,
        area: [0.0, 0.7, 1.0, 1.0],
        rate: 40.0,
        life: (0.25, 0.4),
        size: (0.6, 1.2),
        colour_a: [0.8, 0.85, 0.95, 0.6],
        colour_b: [0.9, 0.92, 1.0, 0.7],
        fade_in: 0.0,
        fade_out: 0.0,
        depth: 1.0,
        aspect: 0.35,
        ..BASE
    },
    Preset {
        id: "bubbles",
        name: "Bubbles",
        sprite: Sprite::Bubble,
        emitter: Emitter::Bottom,
        rate: 10.0,
        life: (4.0, 7.0),
        speed: (8.0, 15.0),
        direction: -90.0,
        spread: 10.0,
        turbulence: 1.0,
        sway: 1.5,
        sway_hz: 0.6,
        size: (0.4, 1.0),
        colour_a: [0.8, 0.9, 1.0, 0.7],
        colour_b: [0.9, 0.95, 1.0, 0.8],
        fade_in: 0.05,
        fade_out: 0.1,
        depth: 0.6,
        ..BASE
    },
    Preset {
        id: "confetti",
        name: "Confetti",
        sprite: Sprite::Confetti,
        emitter: Emitter::Top,
        rate: 60.0,
        life: (5.0, 7.0),
        speed: (10.0, 20.0),
        direction: 90.0,
        spread: 30.0,
        sway: 3.0,
        sway_hz: 0.5,
        turbulence: 2.0,
        spin: (200.0, 500.0),
        size: (0.5, 0.8),
        rainbow: true,
        fade_in: 0.0,
        fade_out: 0.1,
        on_ground: OnGround::Stop,
        depth: 0.6,
        ..BASE
    },
    Preset {
        id: "magic",
        name: "Magic dust",
        sprite: Sprite::Star,
        emitter: Emitter::Area,
        rate: 30.0,
        life: (1.5, 3.0),
        speed: (2.0, 6.0),
        direction: 0.0,
        spread: 360.0,
        gravity: -2.0,
        turbulence: 2.0,
        turbulence_hz: 0.5,
        spin: (20.0, 60.0),
        size: (0.3, 0.7),
        colour_a: [0.8, 0.6, 1.0, 1.0],
        colour_b: [1.0, 0.9, 0.5, 1.0],
        fade_in: 0.2,
        fade_out: 0.4,
        twinkle: 3.0,
        glow: true,
        depth: 0.5,
        ..BASE
    },
    Preset {
        id: "clouds",
        name: "Clouds",
        sprite: Sprite::Puff,
        emitter: Emitter::Area,
        area: [0.0, 0.0, 1.0, 0.35],
        rate: 0.8,
        life: (40.0, 60.0),
        speed: (1.0, 2.0),
        direction: 0.0,
        spread: 5.0,
        turbulence: 0.5,
        turbulence_hz: 0.05,
        spin: (0.5, 2.0),
        size: (12.0, 22.0),
        grow: 1.2,
        colour_a: [1.0, 1.0, 1.0, 0.22],
        colour_b: [0.92, 0.94, 0.98, 0.28],
        fade_in: 0.2,
        fade_out: 0.2,
        depth: 0.5,
        ..BASE
    },
    Preset {
        id: "glints",
        name: "Glints",
        sprite: Sprite::Star,
        emitter: Emitter::Area,
        area: [0.0, 0.6, 1.0, 1.0],
        rate: 40.0,
        life: (0.3, 0.8),
        size: (0.2, 0.5),
        colour_a: [1.0, 0.97, 0.9, 1.0],
        colour_b: [1.0, 1.0, 1.0, 1.0],
        fade_in: 0.5,
        fade_out: 0.5,
        glow: true,
        depth: 0.5,
        ..BASE
    },
    Preset {
        id: "stars",
        name: "Stars",
        sprite: Sprite::Star,
        emitter: Emitter::Area,
        area: [0.0, 0.0, 1.0, 0.5],
        rate: 8.0,
        life: (20.0, 30.0),
        size: (0.08, 0.2),
        colour_a: [0.9, 0.95, 1.0, 1.0],
        colour_b: [1.0, 0.95, 0.85, 1.0],
        fade_in: 0.1,
        fade_out: 0.1,
        twinkle: 0.8,
        glow: true,
        depth: 0.0,
        ..BASE
    },
    Preset {
        id: "embers",
        name: "Embers",
        sprite: Sprite::Dot,
        emitter: Emitter::Point,
        area: [0.35, 0.9, 0.65, 1.0],
        rate: 12.0,
        life: (3.0, 5.0),
        speed: (8.0, 15.0),
        direction: -90.0,
        spread: 30.0,
        gravity: -3.0,
        turbulence: 4.0,
        turbulence_hz: 0.4,
        size: (0.12, 0.25),
        colour_a: [2.0, 0.7, 0.12, 1.0],
        colour_b: [0.6, 0.08, 0.02, 1.0],
        colour_over_life: true,
        fade_in: 0.05,
        fade_out: 0.4,
        twinkle: 2.0,
        glow: true,
        depth: 0.4,
        ..BASE
    },
];

/// Preset names as the `preset` parameter lists them.
pub const PRESET_NAMES: &[&str] = &[
    "Rain", "Snow", "Falling leaves", "Dust motes", "Fireflies", "Sparks", "Smoke", "Mist", "Splashes", "Bubbles", "Confetti", "Magic dust", "Clouds", "Glints", "Stars", "Embers",
];
pub const EMITTER_NAMES: &[&str] = &["Preset", "Top edge", "Area", "Point", "Bottom edge"];
pub const GROUND_NAMES: &[&str] = &["Preset", "Pass through", "Stop", "Bounce", "Splash"];
pub const LOOK_NAMES: &[&str] = &["Preset", "Soft dots", "Streaks", "Flakes", "Leaves", "Sparks", "Puffs", "Bubbles", "Confetti", "Stars", "Rings"];

/// A preset by id (`rain`) or name (`Falling leaves`), any case.
pub fn preset(name: &str) -> Option<(usize, &'static Preset)> {
    let n = name.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    PRESETS.iter().enumerate().find(|(_, p)| p.id == n || p.name.to_ascii_lowercase().replace(' ', "_") == n || p.id.trim_end_matches('s') == n.trim_end_matches('s'))
}

/// Most particles alive at once; denser settings are thinned.
pub const MAX_PARTICLES: usize = 12_000;
/// Tile size of the binned drawing, in pixels.
pub const TILE: u32 = 16;
/// Seconds a splash ring lasts.
const SPLASH_LIFE: f32 = 0.22;

/// The settings after the effect's parameters are folded into a preset.
#[derive(Clone, Copy, Debug)]
pub struct System {
    pub p: Preset,
    pub area: [f32; 4],
    pub amount: f32,
    pub opacity: f32,
    pub tint: [f32; 4],
    pub ground: f32,
    pub horizon: f32,
    pub seed: u32,
}

impl System {
    /// From the effect's parameters (see `effects::generate`).
    pub fn from_params(params: &ParamSet, seed: u32) -> System {
        let mut p = PRESETS[params.choice("preset").min(PRESETS.len() - 1)];
        let emitter = params.choice("emitter");
        let area = if emitter == 0 {
            p.area
        } else {
            p.emitter = [Emitter::Top, Emitter::Area, Emitter::Point, Emitter::Bottom][(emitter - 1).min(3)];
            let (a, b) = (params.point("area_a"), params.point("area_b"));
            [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]
        };
        // Keep the density when the area changes size.
        let (dw, dh) = ((p.area[2] - p.area[0]).max(0.01), (p.area[3] - p.area[1]).max(0.01));
        let (aw, ah) = ((area[2] - area[0]).max(0.01), (area[3] - area[1]).max(0.01));
        p.rate *= match p.emitter {
            Emitter::Top | Emitter::Bottom => aw / dw,
            Emitter::Area => (aw * ah) / (dw * dh),
            Emitter::Point => 1.0,
        };
        let size = params.f("size");
        p.size = (p.size.0 * size, p.size.1 * size);
        let speed = params.f("speed");
        p.speed = (p.speed.0 * speed, p.speed.1 * speed);
        p.direction += params.f("direction");
        p.gravity *= params.f("gravity");
        p.wind += params.f("wind");
        p.turbulence *= params.f("turbulence");
        p.sway *= params.f("turbulence").sqrt();
        let life = params.f("life");
        p.life = (p.life.0 * life, p.life.1 * life);
        p.depth = (p.depth * params.f("depth")).clamp(0.0, 1.0);
        match params.choice("on_ground") {
            1 => p.on_ground = OnGround::Pass,
            2 => p.on_ground = OnGround::Stop,
            3 => p.on_ground = OnGround::Bounce,
            4 => p.on_ground = OnGround::Splash,
            _ => {}
        }
        let look = params.choice("look");
        if look > 0 {
            p.sprite = Sprite::from_code(look as u32 - 1);
            if p.sprite == Sprite::Streak || p.sprite == Sprite::Spark {
                p.streak = p.streak.max(0.03);
            }
            if p.sprite == Sprite::Leaf || p.sprite == Sprite::Ring {
                p.aspect = if p.aspect < 1.0 { p.aspect } else { 0.5 };
            }
        }
        System {
            p,
            area,
            amount: params.f("amount"),
            opacity: params.f("opacity") * params.f("mix"),
            tint: params.colour("colour"),
            ground: params.f("ground"),
            horizon: params.f("horizon"),
            seed: seed ^ (params.i("seed") as u32).wrapping_mul(0x9e37_79b9),
        }
    }
}

/// One particle as drawn at a moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Particle {
    /// Centre (or a streak's head) in pixels.
    pub x: f32,
    pub y: f32,
    /// Radius (or half width) in pixels.
    pub size: f32,
    /// Turn in radians.
    pub angle: f32,
    /// Premultiplied colour; for glows the light it adds.
    pub colour: [f32; 4],
    /// A streak's tail offset from the head, in pixels.
    pub tail: [f32; 2],
    /// Minor over major axis (leaves, rings), or a confetti piece's width.
    pub aspect: f32,
    pub sprite: Sprite,
    /// A splash ring's progress, or a puff's noise offset.
    pub extra: f32,
    pub glow: bool,
    /// 0 nearest .. 1 farthest; drawing order.
    pub depth: f32,
    pub id: i64,
}

#[inline]
fn h(k: i64, i: i32, seed: u32) -> f32 {
    hash2(k as i32, i ^ ((k >> 32) as i32).wrapping_mul(0x632b_e5ab), seed)
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Perspective size of a particle at `depth`: 1 near, a third far.
#[inline]
pub fn depth_scale(depth: f32) -> f32 {
    1.0 / (1.0 + 2.0 * depth)
}

/// The base path (no sway or turbulence): a drag-limited fall.
#[derive(Clone, Copy)]
struct Path {
    p0: [f32; 2],
    v0: [f32; 2],
    acc: [f32; 2],
    wind: [f32; 2],
    drag: f32,
}

impl Path {
    fn at(&self, t: f32) -> [f32; 2] {
        let k = self.drag;
        std::array::from_fn(|i| {
            if k > 1e-4 {
                let term = self.wind[i] + self.acc[i] / k;
                self.p0[i] + term * t + (self.v0[i] - term) * (1.0 - (-k * t).exp()) / k
            } else {
                self.p0[i] + (self.v0[i] + self.wind[i]) * t + 0.5 * self.acc[i] * t * t
            }
        })
    }

    fn vel(&self, t: f32) -> [f32; 2] {
        let k = self.drag;
        std::array::from_fn(|i| {
            if k > 1e-4 {
                let term = self.wind[i] + self.acc[i] / k;
                term + (self.v0[i] - term) * (-k * t).exp()
            } else {
                self.v0[i] + self.wind[i] + self.acc[i] * t
            }
        })
    }

    /// When the path first comes down through `ground` (pixels) by `until`,
    /// if it starts above it.
    fn landing(&self, ground: f32, until: f32) -> Option<f32> {
        if self.p0[1] >= ground {
            return None;
        }
        // The highest point first, then the fall is monotonic.
        let mut lo = 0.0f32;
        if self.vel(0.0)[1] < 0.0 {
            let (mut a, mut b) = (0.0f32, until);
            if self.vel(b)[1] < 0.0 {
                return None;
            }
            for _ in 0..24 {
                let m = 0.5 * (a + b);
                if self.vel(m)[1] < 0.0 {
                    a = m;
                } else {
                    b = m;
                }
            }
            lo = b;
        }
        if self.at(until)[1] < ground {
            return None;
        }
        let (mut a, mut b) = (lo, until);
        for _ in 0..28 {
            let m = 0.5 * (a + b);
            if self.at(m)[1] < ground {
                a = m;
            } else {
                b = m;
            }
        }
        Some(b)
    }
}

/// Every particle alive at `time` (seconds), far to near, for a picture
/// of `w`×`h` pixels.
pub fn simulate(sys: &System, time: f32, w: usize, h_px: usize) -> Vec<Particle> {
    let p = &sys.p;
    let (wf, hf) = (w as f32, h_px as f32);
    let u = hf / 100.0; // pixels per % height
    let life_max = p.life.1.max(p.life.0).max(0.01);
    // Emission runs at up to twice the preset rate so `amount` animates
    // smoothly between 0 and 2 (each particle has a fixed threshold).
    let span = sys.amount.max(2.0);
    let mut rate = (p.rate * span).max(0.0);
    if rate * life_max > MAX_PARTICLES as f32 {
        rate = MAX_PARTICLES as f32 / life_max;
    }
    if rate <= 0.0 || sys.amount <= 0.0 || sys.opacity <= 0.0 {
        return Vec::new();
    }
    let keep = (sys.amount / span).clamp(0.0, 1.0);
    let first = ((time - life_max) * rate).floor() as i64;
    let last = (time * rate).floor() as i64;
    let seed = sys.seed;
    let tint = sys.tint;
    let area = [sys.area[0] * wf, sys.area[1] * hf, sys.area[2] * wf, sys.area[3] * hf];
    let mut out = Vec::new();
    for k in first..=last {
        // Thinning by amount, with a soft edge so animating it fades.
        let thr = h(k, 1, seed);
        if thr >= keep {
            continue;
        }
        let soft = ((keep - thr) / 0.05).min(1.0);
        let birth = (k as f32 + h(k, 2, seed)) / rate;
        let age = time - birth;
        let life = lerp(p.life.0, p.life.1, h(k, 3, seed));
        if age < 0.0 || age >= life {
            continue;
        }
        let depth = p.depth * h(k, 4, seed);
        let s = depth_scale(depth);
        let size0 = lerp(p.size.0, p.size.1, h(k, 5, seed)) * u * s;
        // Start position.
        let speed = lerp(p.speed.0, p.speed.1, h(k, 6, seed)) * u * s;
        let dir = (p.direction + (h(k, 7, seed) - 0.5) * p.spread).to_radians();
        let v0 = [dir.cos() * speed, dir.sin() * speed];
        let wind = [p.wind * u * s, 0.0];
        let acc = [0.0, p.gravity * u * s];
        let drift = (wind[0] + v0[0]) * life;
        let (rx, ry) = (h(k, 8, seed), h(k, 9, seed));
        let p0 = match p.emitter {
            Emitter::Top | Emitter::Bottom => {
                // Widen upwind so the wind does not leave an empty side.
                let (x0, x1) = if drift > 0.0 { (area[0] - drift, area[2]) } else { (area[0], area[2] - drift) };
                let y = if p.emitter == Emitter::Top { area[1] - size0 - 0.02 * hf - p.streak * speed } else { area[3] + size0 + 0.02 * hf };
                [lerp(x0, x1, rx), y]
            }
            Emitter::Area => [lerp(area[0], area[2], rx), lerp(area[1], area[3], ry)],
            Emitter::Point => {
                let (cx, cy) = (0.5 * (area[0] + area[2]), 0.5 * (area[1] + area[3]));
                let r = 0.5 * (area[2] - area[0]).abs().min((area[3] - area[1]).abs()) * rx.sqrt();
                let a = ry * std::f32::consts::TAU;
                [cx + r * a.cos(), cy + r * a.sin()]
            }
        };
        let path = Path { p0, v0, acc, wind, drag: p.drag };
        let ground = (sys.horizon + (sys.ground - sys.horizon) * s) * hf;
        let mut pos = path.at(age);
        let mut vel = path.vel(age);
        let mut landed: Option<f32> = None;
        if p.on_ground != OnGround::Pass && sys.ground < 1.5 {
            if let Some(th) = path.landing(ground, age) {
                landed = Some(th);
            }
        }
        let mut sprite = p.sprite;
        let mut extra = h(k, 10, seed) * 50.0;
        let mut aspect = p.aspect;
        let mut fade = 1.0f32;
        let mut wander = 1.0f32;
        if let Some(th) = landed {
            let hit = path.at(th);
            match p.on_ground {
                OnGround::Splash => {
                    let since = age - th;
                    if since > SPLASH_LIFE {
                        continue;
                    }
                    sprite = Sprite::Ring;
                    extra = since / SPLASH_LIFE;
                    aspect = 0.35;
                    pos = [hit[0], ground];
                    vel = [0.0, 0.0];
                    wander = 0.0;
                }
                OnGround::Stop => {
                    pos = [hit[0], ground];
                    vel = [0.0, 0.0];
                    wander = 0.0;
                    // A landed flake or leaf fades over a few seconds.
                    fade = 1.0 - ((age - th) / 3.0).clamp(0.0, 1.0);
                }
                OnGround::Bounce => {
                    let vh = path.vel(th);
                    let since = age - th;
                    let up = -0.35 * vh[1];
                    let g = p.gravity.max(1.0) * u * s;
                    let t_back = 2.0 * up / g;
                    if since < t_back {
                        pos = [hit[0] + 0.6 * vh[0] * since, ground - (up * since - 0.5 * g * since * since)];
                        vel = [0.6 * vh[0], -(up - g * since)];
                    } else {
                        pos = [hit[0] + 0.6 * vh[0] * t_back, ground];
                        vel = [0.0, 0.0];
                    }
                }
                OnGround::Pass => {}
            }
        }
        // Sway and turbulence on top of the path.
        if wander > 0.0 {
            if p.sway > 0.0 {
                let ph = h(k, 11, seed) * std::f32::consts::TAU;
                pos[0] += p.sway * u * s * (std::f32::consts::TAU * p.sway_hz * age + ph).sin();
            }
            if p.turbulence > 0.0 {
                let f = p.turbulence_hz.max(0.01);
                let o = (k.rem_euclid(1 << 20)) as f32 * 7.13;
                let n = |sd: u32, t: f32| noise1(t * f + o, seed ^ sd) + 0.5 * noise1(t * f * 2.1 + o * 0.37, seed ^ sd.rotate_left(7));
                pos[0] += p.turbulence * u * s * n(0x1234_5678, age);
                let dy = p.turbulence * u * s * n(0x0bad_cafe, age);
                // Turbulence never pushes a falling thing below its ground.
                pos[1] = if landed.is_none() && p.on_ground != OnGround::Pass { (pos[1] + dy).min(ground) } else { pos[1] + dy };
            }
        }
        let t01 = age / life.max(1e-4);
        if sprite == Sprite::Ring && landed.is_none() {
            // A ring of its own (not a splash) spreads over its life.
            extra = t01;
        }
        let size = size0 * lerp(1.0, p.grow, t01);
        // Opacity over life.
        let mut a = 1.0f32;
        if p.fade_in > 0.0 {
            a *= (t01 / p.fade_in).min(1.0);
        }
        if p.fade_out > 0.0 {
            a *= ((1.0 - t01) / p.fade_out).min(1.0);
        }
        if p.twinkle > 0.0 {
            let ph = h(k, 12, seed) * std::f32::consts::TAU;
            let tw = 0.5 + 0.5 * (std::f32::consts::TAU * p.twinkle * age * lerp(0.7, 1.3, h(k, 13, seed)) + ph).sin();
            a *= 0.25 + 0.75 * tw * tw;
        }
        a *= soft * fade;
        // Colour.
        let pick = if p.colour_over_life { t01 } else { h(k, 14, seed) };
        let mut c: [f32; 4] = std::array::from_fn(|i| lerp(p.colour_a[i], p.colour_b[i], pick));
        if p.rainbow {
            let rgb = hsv_to_rgb(h(k, 15, seed) * 360.0, 0.8, 0.9);
            c = [rgb[0], rgb[1], rgb[2], 1.0];
        }
        let alpha = c[3] * tint[3] * sys.opacity * a;
        let rgb = [c[0] * tint[0], c[1] * tint[1], c[2] * tint[2]];
        // Thin sprites keep their weight when drawn at least a pixel wide.
        let (mut size_d, mut alpha_d) = (size, alpha);
        let line = matches!(sprite, Sprite::Streak | Sprite::Spark);
        if line && size_d < 0.75 {
            alpha_d *= size_d / 0.75;
            size_d = 0.75;
        } else if !line && size_d < 0.6 {
            alpha_d *= (size_d / 0.6) * (size_d / 0.6);
            size_d = 0.6;
        }
        if alpha_d <= 1e-4 {
            continue;
        }
        let colour = if p.glow { [rgb[0] * alpha_d, rgb[1] * alpha_d, rgb[2] * alpha_d, 0.0] } else { premultiply(rgb, alpha_d) };
        let tail = if line { [-vel[0] * p.streak, -vel[1] * p.streak] } else { [0.0, 0.0] };
        let spin = lerp(p.spin.0, p.spin.1, h(k, 16, seed)) * if h(k, 17, seed) < 0.5 { -1.0 } else { 1.0 };
        let mut angle = (h(k, 18, seed) * 360.0 + spin * age).to_radians();
        if landed.is_some() && p.on_ground == OnGround::Stop {
            let th = landed.unwrap_or(age);
            angle = (h(k, 18, seed) * 360.0 + spin * th).to_radians();
        }
        if sprite == Sprite::Confetti {
            // The piece tumbles: its width follows the flip.
            let flip = (h(k, 19, seed) * 6.0 + age * spin.abs().to_radians() * 0.7).cos().abs();
            aspect = if landed.is_some() { 1.0 } else { flip.max(0.12) };
        }
        if sprite == Sprite::Leaf && landed.is_none() {
            // A falling leaf rocks as it sways: its outline narrows.
            aspect = p.aspect * lerp(0.55, 1.0, (std::f32::consts::TAU * p.sway_hz * age + h(k, 11, seed) * 6.0).cos().abs());
        }
        let particle = Particle { x: pos[0], y: pos[1], size: size_d, angle, colour, tail, aspect, sprite, extra, glow: p.glow, depth, id: k };
        let r = reach(&particle);
        if particle.x + r < 0.0 || particle.y + r < 0.0 || particle.x - r > wf || particle.y - r > hf {
            continue;
        }
        out.push(particle);
    }
    out.sort_by(|a, b| b.depth.total_cmp(&a.depth).then(a.id.cmp(&b.id)));
    out
}

/// How far a particle's drawing reaches from its centre (pixels).
fn reach(p: &Particle) -> f32 {
    let tail = (p.tail[0] * p.tail[0] + p.tail[1] * p.tail[1]).sqrt();
    p.size + tail + 1.0
}

/// Words per particle in the packed buffer.
const WORDS: usize = 16;
/// Header words before the tile offsets.
const HEADER: usize = 8;

/// Packs particles for drawing on a `w`×`h` picture: header (tiles across,
/// tiles down, tile size, particle count, offset of the particle data),
/// one offset per tile plus an end, the particle indices of every tile in
/// drawing order, then 16 words per particle (see `wgsl/particles.wgsl`).
pub fn pack(particles: &[Particle], w: usize, h_px: usize) -> Vec<u32> {
    let tx = (w as u32).div_ceil(TILE).max(1) as usize;
    let ty = (h_px as u32).div_ceil(TILE).max(1) as usize;
    let mut counts = vec![0u32; tx * ty];
    let ranges: Vec<(usize, usize, usize, usize)> = particles
        .iter()
        .map(|p| {
            let (cx, cy) = (p.x + 0.5 * p.tail[0], p.y + 0.5 * p.tail[1]);
            let half = 0.5 * (p.tail[0] * p.tail[0] + p.tail[1] * p.tail[1]).sqrt();
            let r = p.size + half + 1.0;
            let t = TILE as f32;
            let x0 = ((cx - r) / t).floor().clamp(0.0, tx as f32 - 1.0) as usize;
            let x1 = ((cx + r) / t).floor().clamp(0.0, tx as f32 - 1.0) as usize;
            let y0 = ((cy - r) / t).floor().clamp(0.0, ty as f32 - 1.0) as usize;
            let y1 = ((cy + r) / t).floor().clamp(0.0, ty as f32 - 1.0) as usize;
            (x0, x1, y0, y1)
        })
        .collect();
    for &(x0, x1, y0, y1) in &ranges {
        for y in y0..=y1 {
            for x in x0..=x1 {
                counts[y * tx + x] += 1;
            }
        }
    }
    let tiles = tx * ty;
    let total: usize = counts.iter().map(|&c| c as usize).sum();
    let index_base = HEADER + tiles + 1;
    let data_base = index_base + total;
    let mut buf = vec![0u32; data_base + particles.len() * WORDS];
    buf[0] = tx as u32;
    buf[1] = ty as u32;
    buf[2] = TILE;
    buf[3] = particles.len() as u32;
    buf[4] = data_base as u32;
    let mut off = 0u32;
    for (i, c) in counts.iter().enumerate() {
        buf[HEADER + i] = off;
        off += c;
    }
    buf[HEADER + tiles] = off;
    let mut fill: Vec<u32> = buf[HEADER..HEADER + tiles].to_vec();
    for (pi, &(x0, x1, y0, y1)) in ranges.iter().enumerate() {
        for y in y0..=y1 {
            for x in x0..=x1 {
                let t = y * tx + x;
                buf[index_base + fill[t] as usize] = pi as u32;
                fill[t] += 1;
            }
        }
    }
    for (i, p) in particles.iter().enumerate() {
        let words = [
            p.x,
            p.y,
            p.size,
            p.angle,
            p.colour[0],
            p.colour[1],
            p.colour[2],
            p.colour[3],
            p.tail[0],
            p.tail[1],
            p.aspect,
            p.sprite as u32 as f32,
            p.extra,
            0.0,
            if p.glow { 1.0 } else { 0.0 },
            p.depth,
        ];
        for (j, v) in words.iter().enumerate() {
            buf[data_base + i * WORDS + j] = v.to_bits();
        }
    }
    buf
}

#[inline]
fn bump(d: f32, r: f32) -> f32 {
    let t = (1.0 - d / r.max(1e-4)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A sprite's coverage at offset (`dx`, `dy`) from its centre, and a shade
/// that multiplies its colour. The WGSL `sprite` function is the same.
pub fn coverage(sprite: Sprite, dx: f32, dy: f32, size: f32, angle: f32, tail: [f32; 2], aspect: f32, extra: f32) -> (f32, f32) {
    let (s, c) = angle.sin_cos();
    let lx = dx * c + dy * s;
    let ly = -dx * s + dy * c;
    let d = (dx * dx + dy * dy).sqrt();
    match sprite {
        Sprite::Dot => {
            let b = bump(d, size);
            (b * b, 1.0)
        }
        Sprite::Flake => {
            let b = bump(d, size);
            (b * (0.6 + 0.4 * b), 1.0)
        }
        Sprite::Streak | Sprite::Spark => {
            let l2 = tail[0] * tail[0] + tail[1] * tail[1];
            let t = if l2 > 1e-6 { ((dx * tail[0] + dy * tail[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
            let (ex, ey) = (dx - t * tail[0], dy - t * tail[1]);
            let b = bump((ex * ex + ey * ey).sqrt(), size);
            if sprite == Sprite::Spark { (b * b * (1.0 - 0.8 * t), 1.0) } else { (b * (1.0 - 0.7 * t), 1.0) }
        }
        Sprite::Leaf => {
            let minor = (size * aspect).max(0.5);
            let q = ((lx / size).powi(2) + (ly / minor).powi(2)).sqrt();
            let cov = ((1.0 - q) * minor).clamp(0.0, 1.0);
            // Lit on one side, with a darker midrib.
            let rib = 1.0 - 0.35 * bump(ly.abs(), (0.12 * minor).max(0.5));
            (cov, (0.75 + 0.3 * (lx / size).clamp(-1.0, 1.0)) * rib)
        }
        Sprite::Puff => {
            let b = bump(d, size);
            let n = value_noise(lx / size * 1.7 + extra, ly / size * 1.7 + extra * 0.7, 0x5eed);
            (b * b * (0.45 + 0.55 * n), 1.0)
        }
        Sprite::Bubble => {
            let ring = bump((d - 0.85 * size).abs(), (0.15 * size).max(0.7));
            let body = 0.08 * bump(d, size);
            let (hx, hy) = (dx + 0.4 * size, dy + 0.4 * size);
            let hl = bump((hx * hx + hy * hy).sqrt(), (0.22 * size).max(0.6));
            ((0.8 * ring + body + hl).min(1.0), 1.0)
        }
        Sprite::Confetti => {
            let (hw, hh) = ((size * aspect).max(0.4), size * 0.6);
            let (qx, qy) = (lx.abs() - hw, ly.abs() - hh);
            let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
            let sd = outside + qx.max(qy).min(0.0);
            ((0.5 - sd).clamp(0.0, 1.0), 0.7 + 0.3 * aspect)
        }
        Sprite::Star => {
            let core = bump(d, size * 0.35);
            let thin = (size * 0.06).max(0.5);
            let glare = bump(ly.abs(), thin) * bump(lx.abs(), size) + bump(lx.abs(), thin) * bump(ly.abs(), size);
            ((core * core + 0.6 * glare).min(1.0), 1.0)
        }
        Sprite::Ring => {
            let q = (dx * dx + (dy / aspect.max(0.1)).powi(2)).sqrt();
            let rr = size * (0.3 + 0.7 * extra);
            let b = bump((q - rr).abs(), (size * 0.12).max(0.8));
            (b * (1.0 - extra), 1.0)
        }
    }
}

#[inline]
fn word(buf: &[u32], i: usize) -> f32 {
    f32::from_bits(buf[i])
}

/// Draws a packed buffer over `under` (the CPU twin of the shader).
pub fn draw_packed(under: &Image, buf: &[u32]) -> Image {
    if buf.len() < HEADER + 1 || buf[3] == 0 {
        return under.clone();
    }
    let (tx, tile) = (buf[0] as usize, buf[2] as usize);
    let tiles = tx * buf[1] as usize;
    let index_base = HEADER + tiles + 1;
    let data = buf[4] as usize;
    under.map_xy(|x, y, px| {
        let t = (y / tile) * tx + x / tile;
        let (a, b) = (buf[HEADER + t] as usize, buf[HEADER + t + 1] as usize);
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let mut acc = px;
        for i in a..b {
            let base = data + buf[index_base + i] as usize * WORDS;
            let w = |j: usize| word(buf, base + j);
            let (cov, shade) = coverage(Sprite::from_code(w(11) as u32), fx - w(0), fy - w(1), w(2), w(3), [w(8), w(9)], w(10), w(12));
            if cov <= 0.0 {
                continue;
            }
            let c = [w(4) * shade * cov, w(5) * shade * cov, w(6) * shade * cov, w(7) * cov];
            acc = if w(14) > 0.5 {
                let l = [c[0], c[1], c[2]];
                [acc[0] + l[0], acc[1] + l[1], acc[2] + l[2], acc[3].max(l[0].max(l[1]).max(l[2]).min(1.0))]
            } else {
                over(c, acc)
            };
        }
        acc
    })
}

/// The effect's CPU path: simulate, pack, draw.
pub fn render(time: f32, seed: u32, img: &Image, params: &ParamSet) -> Image {
    draw_packed(img, &packed(time, seed, img.width, img.height, params))
}

/// The buffer the GPU path draws from.
pub fn packed(time: f32, seed: u32, w: usize, h_px: usize, params: &ParamSet) -> Vec<u32> {
    let sys = System::from_params(params, seed);
    pack(&simulate(&sys, time, w, h_px), w, h_px)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects;
    use crate::params::ParamValue;

    fn params(preset: &str) -> ParamSet {
        let def = effects::find("particles").unwrap();
        let (i, _) = preset_index(preset);
        def.defaults().with("preset", ParamValue::Choice(i as u32))
    }

    fn preset_index(name: &str) -> (usize, &'static Preset) {
        preset(name).unwrap_or_else(|| panic!("no preset {name}"))
    }

    #[test]
    fn presets_line_up_with_their_names() {
        assert_eq!(PRESETS.len(), PRESET_NAMES.len());
        for (p, n) in PRESETS.iter().zip(PRESET_NAMES) {
            assert_eq!(p.name, *n);
            assert!(p.life.0 > 0.0 && p.life.1 >= p.life.0, "{}", p.id);
            assert!(p.size.0 > 0.0 && p.size.1 >= p.size.0, "{}", p.id);
        }
        assert_eq!(preset("Falling leaves").unwrap().1.id, "leaves");
        assert_eq!(preset("sparks").unwrap().1.id, "sparks");
        assert_eq!(preset("spark").unwrap().1.id, "sparks");
        assert_eq!(preset("magic_dust").unwrap().1.id, "magic");
    }

    #[test]
    fn the_same_moment_gives_the_same_particles() {
        let sys = System::from_params(&params("snow"), 7);
        let a = simulate(&sys, 3.25, 320, 180);
        let b = simulate(&sys, 3.25, 320, 180);
        assert_eq!(a, b);
        assert!(!a.is_empty());
        // Scrubbing back and forth changes nothing either.
        let _ = simulate(&sys, 9.0, 320, 180);
        assert_eq!(simulate(&sys, 3.25, 320, 180), a);
    }

    #[test]
    fn rain_fills_the_frame_from_the_first_frame_and_falls() {
        let sys = System::from_params(&params("rain"), 1);
        let (w, hh) = (320, 180);
        let a = simulate(&sys, 0.0, w, hh);
        assert!(a.len() > 200, "{}", a.len());
        // Spread over the frame, top to bottom.
        let top = a.iter().filter(|p| p.y < hh as f32 * 0.3 && p.sprite == Sprite::Streak).count();
        let bottom = a.iter().filter(|p| p.y > hh as f32 * 0.6 && p.sprite == Sprite::Streak).count();
        assert!(top > 20 && bottom > 20, "{top} {bottom}");
        // Drops move down between frames.
        let b = simulate(&sys, 1.0 / 30.0, w, hh);
        let pa = a.iter().find(|p| p.sprite == Sprite::Streak && p.y > 20.0 && p.y < 100.0).unwrap();
        let pb = b.iter().find(|p| p.id == pa.id).unwrap();
        assert!(pb.y > pa.y + 1.0, "{} -> {}", pa.y, pb.y);
        // Some have hit the ground and splashed.
        assert!(a.iter().any(|p| p.sprite == Sprite::Ring));
    }

    #[test]
    fn nothing_falls_through_the_ground() {
        let mut ps = params("snow");
        ps.set("ground", ParamValue::Float(0.8)).unwrap();
        ps.set("horizon", ParamValue::Float(0.5)).unwrap();
        let sys = System::from_params(&ps, 3);
        let (w, hh) = (320usize, 180usize);
        for t in [0.0f32, 2.0, 5.5] {
            for p in simulate(&sys, t, w, hh) {
                let ground = (0.5 + (0.8 - 0.5) * depth_scale(p.depth)) * hh as f32;
                assert!(p.y <= ground + 0.01, "t {t}: y {} below ground {ground}", p.y);
            }
        }
    }

    #[test]
    fn amount_thins_and_zero_is_nothing() {
        let mut ps = params("dust");
        let sys = System::from_params(&ps, 0);
        let full = simulate(&sys, 4.0, 320, 180).len();
        ps.set("amount", ParamValue::Float(0.5)).unwrap();
        let half = simulate(&System::from_params(&ps, 0), 4.0, 320, 180).len();
        ps.set("amount", ParamValue::Float(0.0)).unwrap();
        let none = simulate(&System::from_params(&ps, 0), 4.0, 320, 180).len();
        assert!(full > 20 && half < full && half > full / 4, "{full} {half}");
        assert_eq!(none, 0);
    }

    #[test]
    fn particles_stay_in_their_area() {
        let def = effects::find("particles").unwrap();
        let ps = def
            .defaults()
            .with("preset", ParamValue::Choice(preset_index("glints").0 as u32))
            .with("emitter", ParamValue::Choice(2))
            .with("area_a", ParamValue::Point([0.2, 0.6]))
            .with("area_b", ParamValue::Point([0.5, 0.9]));
        let ps = ps.with("turbulence", ParamValue::Float(0.0));
        let sys = System::from_params(&ps, 0);
        let v = simulate(&sys, 2.0, 400, 200);
        assert!(!v.is_empty());
        for p in v {
            assert!(p.x >= 0.2 * 400.0 - 1.0 && p.x <= 0.5 * 400.0 + 1.0 && p.y >= 0.6 * 200.0 - 1.0 && p.y <= 0.9 * 200.0 + 1.0, "{} {}", p.x, p.y);
        }
    }

    #[test]
    fn sparks_rise_then_fall_and_glow() {
        let sys = System::from_params(&params("sparks"), 5);
        let v = simulate(&sys, 1.0, 320, 180);
        assert!(v.iter().all(|p| p.glow && p.colour[3] == 0.0));
        assert!(v.iter().any(|p| p.y < 0.85 * 180.0 - 10.0), "some fly up");
    }

    #[test]
    fn packed_tiles_hold_every_particle_they_touch() {
        let sys = System::from_params(&params("snow"), 2);
        let (w, hh) = (200usize, 120usize);
        let v = simulate(&sys, 2.0, w, hh);
        let buf = pack(&v, w, hh);
        assert_eq!(buf[3] as usize, v.len());
        let tx = buf[0] as usize;
        // The tile under each particle's centre lists it.
        for (i, p) in v.iter().enumerate() {
            if p.x < 0.0 || p.y < 0.0 || p.x >= w as f32 || p.y >= hh as f32 {
                continue;
            }
            let t = (p.y as usize / TILE as usize) * tx + p.x as usize / TILE as usize;
            let tiles = tx * buf[1] as usize;
            let (a, b) = (buf[HEADER + t] as usize, buf[HEADER + t + 1] as usize);
            let base = HEADER + tiles + 1;
            assert!(buf[base + a..base + b].contains(&(i as u32)), "particle {i} missing from its tile");
        }
    }

    #[test]
    fn drawing_puts_snow_on_a_dark_picture() {
        let img = Image::filled(480, 270, [0.02, 0.02, 0.03, 1.0]);
        let out = render(3.0, 0, &img, &params("snow"));
        let bright = out.pixels.iter().filter(|p| p[0] > 0.3).count();
        assert!(bright > 20, "{bright}");
        assert!(out.is_finite());
        // An empty layer gets flakes with alpha and stays clear elsewhere.
        let layer = render(3.0, 0, &Image::new(480, 270), &params("snow"));
        assert!(layer.pixels.iter().any(|p| p[3] > 0.3));
        assert!(layer.pixels.iter().filter(|p| p[3] == 0.0).count() > 480 * 270 / 2);
    }

    #[test]
    fn every_preset_draws_something_finite() {
        for (i, p) in PRESETS.iter().enumerate() {
            let def = effects::find("particles").unwrap();
            let ps = def.defaults().with("preset", ParamValue::Choice(i as u32));
            let bg = Image::filled(480, 270, [0.2, 0.25, 0.3, 1.0]);
            let out = render(5.0, 11, &bg, &ps);
            assert!(out.is_finite(), "{}", p.id);
            let changed = out.pixels.iter().zip(bg.pixels.iter()).filter(|(a, b)| (a[0] - b[0]).abs() > 0.01).count();
            assert!(changed > 0, "{} draws nothing", p.id);
        }
    }

    #[test]
    fn sprites_cover_their_middle_and_not_far_away() {
        for s in Sprite::ALL {
            let (far, _) = coverage(s, 50.0, 50.0, 5.0, 0.3, [0.0, -8.0], 0.5, 0.3);
            assert_eq!(far, 0.0, "{s:?}");
            let at = match s {
                Sprite::Bubble => 4.25,
                Sprite::Ring => 2.55,
                _ => 0.2,
            };
            let (near, _) = coverage(s, at, 0.0, 5.0, 0.0, [0.0, -8.0], 0.5, 0.3);
            assert!(near > 0.05, "{s:?}: {near}");
        }
    }
}
