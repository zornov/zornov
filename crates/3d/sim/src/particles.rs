// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Particles: sparks, rain, snow, dust, confetti, embers, magic glitter,
//! fountains, debris. An emitter (a point, a ball, a box, or a model's
//! surface) sends them out at a rate with a speed and spread; gravity,
//! air drag, wind and swirling turbulence carry them; they bounce off,
//! slide along, stick to or die on the floor and any 3D model; they grow,
//! shrink, fade and cool over their life.

use crate::{Collider, Rng, V3, add, dot, length, normalise, scale, sub};
use studio_mesh::cache::Point;

/// Where particles come from.
#[derive(Clone, Debug)]
pub enum Source {
    Point(V3),
    Ball { centre: V3, radius: f32 },
    Box { min: V3, max: V3 },
    /// Triangles in the world (a model's surface), with their normals.
    Surface { tris: Vec<[V3; 3]>, normals: Vec<V3> },
}

/// What happens when a particle hits something.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OnHit {
    Bounce,
    Stick,
    Die,
}

/// An emitter's settings.
#[derive(Clone, Debug)]
pub struct Emitter {
    pub source: Source,
    /// Particles per second.
    pub rate: f32,
    /// A burst of this many at the start.
    pub burst: usize,
    /// Seconds the emitter runs (from the start).
    pub duration: f32,
    /// Seconds each lives (± `life_spread` × it).
    pub life: f32,
    pub life_spread: f32,
    /// Starting velocity (px/s) and how widely it varies (0 a jet – 1 every way).
    pub velocity: V3,
    pub spread: f32,
    /// Extra speed straight out of a surface source along its normal.
    pub normal_speed: f32,
    pub size: f32,
    /// Size at the end of life as a fraction of the start.
    pub size_end: f32,
    /// sRGB colours at birth and death (blended), straight alpha.
    pub colour: [u8; 4],
    pub colour_end: [u8; 4],
    /// Heat at birth, cooling to `heat_end` (sparks and embers glow).
    pub heat: f32,
    pub heat_end: f32,
    /// Gravity scale (1 falls, 0 floats, negative rises).
    pub gravity: f32,
    /// Air drag per second, 0–1.
    pub drag: f32,
    /// Swirl strength (px/s) and size (px).
    pub turbulence: f32,
    pub swirl_size: f32,
    pub wind: V3,
    pub on_hit: OnHit,
    pub bounce: f32,
    pub friction: f32,
    pub seed: u64,
    /// Most alive at once.
    pub max: usize,
}

impl Default for Emitter {
    fn default() -> Emitter {
        Emitter {
            source: Source::Point([0.0; 3]),
            rate: 200.0,
            burst: 0,
            duration: f32::INFINITY,
            life: 2.0,
            life_spread: 0.3,
            velocity: [0.0, -300.0, 0.0],
            spread: 0.3,
            normal_speed: 0.0,
            size: 4.0,
            size_end: 1.0,
            colour: [255, 255, 255, 255],
            colour_end: [255, 255, 255, 255],
            heat: 0.0,
            heat_end: 0.0,
            gravity: 1.0,
            drag: 0.1,
            turbulence: 0.0,
            swirl_size: 200.0,
            wind: [0.0; 3],
            on_hit: OnHit::Bounce,
            bounce: 0.4,
            friction: 0.3,
            seed: 1,
            max: 20_000,
        }
    }
}

/// Ready-made emitters by name, around `at`, scaled to `size` px (the
/// span of the effect).
pub fn preset(name: &str, at: V3, size: f32) -> Option<Emitter> {
    let s = size.max(10.0);
    let d = Emitter::default();
    Some(match name {
        "sparks" => Emitter { source: Source::Point(at), rate: 0.0, burst: 300, duration: 0.0, life: 1.2, velocity: [0.0, -s * 1.2, 0.0], spread: 1.0, size: s * 0.008, size_end: 0.3, colour: [255, 220, 150, 255], colour_end: [255, 90, 20, 255], heat: 1.0, heat_end: 0.2, drag: 0.4, bounce: 0.35, ..d },
        "fountain" | "spray" | "water" => Emitter { source: Source::Ball { centre: at, radius: s * 0.02 }, rate: 600.0, life: 1.6, velocity: [0.0, -s * 2.2, 0.0], spread: 0.12, size: s * 0.012, size_end: 0.8, colour: [180, 210, 255, 220], colour_end: [200, 225, 255, 160], drag: 0.05, on_hit: OnHit::Die, ..d },
        "rain" => Emitter { source: Source::Box { min: add(at, [-s, -s * 0.6, -s]), max: add(at, [s, -s * 0.55, s]) }, rate: 2500.0, life: 1.0, velocity: [s * 0.1, s * 3.0, 0.0], spread: 0.02, size: s * 0.004, colour: [190, 200, 220, 170], colour_end: [190, 200, 220, 170], drag: 0.0, on_hit: OnHit::Die, ..d },
        "snow" => Emitter { source: Source::Box { min: add(at, [-s, -s * 0.6, -s]), max: add(at, [s, -s * 0.55, s]) }, rate: 500.0, life: 6.0, velocity: [0.0, s * 0.15, 0.0], spread: 0.3, size: s * 0.008, size_end: 1.0, gravity: 0.05, drag: 0.6, turbulence: s * 0.12, swirl_size: s * 0.3, on_hit: OnHit::Stick, ..d },
        "dust" | "motes" => Emitter { source: Source::Box { min: add(at, [-s * 0.5; 3]), max: add(at, [s * 0.5; 3]) }, rate: 60.0, life: 5.0, velocity: [0.0; 3], spread: 1.0, size: s * 0.003, colour: [255, 245, 220, 150], colour_end: [255, 245, 220, 0], gravity: 0.0, drag: 0.8, turbulence: s * 0.04, swirl_size: s * 0.2, ..d },
        "confetti" => Emitter { source: Source::Point(at), rate: 0.0, burst: 400, duration: 0.0, life: 4.0, velocity: [0.0, -s * 1.4, 0.0], spread: 0.6, size: s * 0.012, colour: [255, 80, 120, 255], colour_end: [80, 180, 255, 255], gravity: 0.35, drag: 0.9, turbulence: s * 0.1, swirl_size: s * 0.15, on_hit: OnHit::Stick, ..d },
        "embers" | "fire" => Emitter { source: Source::Ball { centre: at, radius: s * 0.05 }, rate: 120.0, life: 2.5, velocity: [0.0, -s * 0.35, 0.0], spread: 0.35, size: s * 0.006, size_end: 0.2, colour: [255, 160, 60, 255], colour_end: [120, 20, 0, 0], heat: 0.9, heat_end: 0.1, gravity: -0.1, drag: 0.5, turbulence: s * 0.15, swirl_size: s * 0.25, ..d },
        "magic" | "glitter" => Emitter { source: Source::Ball { centre: at, radius: s * 0.08 }, rate: 300.0, life: 1.5, velocity: [0.0; 3], spread: 1.0, size: s * 0.005, size_end: 0.0, colour: [200, 220, 255, 255], colour_end: [255, 180, 255, 0], heat: 0.6, heat_end: 0.6, gravity: -0.05, drag: 0.6, turbulence: s * 0.2, swirl_size: s * 0.15, ..d },
        "debris" => Emitter { source: Source::Point(at), rate: 0.0, burst: 150, duration: 0.0, life: 3.0, velocity: [0.0, -s * 1.0, 0.0], spread: 0.8, size: s * 0.02, size_end: 1.0, colour: [120, 110, 100, 255], colour_end: [120, 110, 100, 255], drag: 0.1, bounce: 0.25, friction: 0.7, ..d },
        "bubbles" => Emitter { source: Source::Ball { centre: at, radius: s * 0.05 }, rate: 40.0, life: 4.0, velocity: [0.0, -s * 0.2, 0.0], spread: 0.3, size: s * 0.03, colour: [220, 240, 255, 110], colour_end: [220, 240, 255, 60], gravity: -0.08, drag: 0.7, turbulence: s * 0.05, swirl_size: s * 0.2, on_hit: OnHit::Die, ..d },
        _ => return None,
    })
}

pub const PRESETS: &[&str] = &["sparks", "fountain", "rain", "snow", "dust", "confetti", "embers", "magic", "debris", "bubbles"];

#[derive(Clone, Copy)]
struct P {
    pos: V3,
    vel: V3,
    age: f32,
    life: f32,
    stuck: bool,
    seed: u32,
}

/// Runs an emitter for `seconds` at `fps`; each frame's particles.
pub fn run(e: &Emitter, gravity: V3, colliders: &[Collider], seconds: f32, fps: f32) -> Vec<Vec<Point>> {
    let mut rng = Rng::new(e.seed);
    let frames = (seconds * fps).round().max(1.0) as usize + 1;
    let mut out = Vec::with_capacity(frames);
    let mut live: Vec<P> = Vec::new();
    let mut owed = 0.0f32;
    let sub_n = 3usize;
    let dt = 1.0 / fps;
    let h = dt / sub_n as f32;
    let areas: Vec<f32> = match &e.source {
        Source::Surface { tris, .. } => {
            let mut acc = 0.0;
            tris.iter().map(|t| {
                acc += length(crate::cross(sub(t[1], t[0]), sub(t[2], t[0]))) * 0.5;
                acc
            }).collect()
        }
        _ => Vec::new(),
    };
    let spawn = |rng: &mut Rng, live: &mut Vec<P>| {
        if live.len() >= e.max {
            return;
        }
        let (pos, normal) = match &e.source {
            Source::Point(p) => (*p, [0.0; 3]),
            Source::Ball { centre, radius } => (add(*centre, scale(rng.ball(), *radius)), [0.0; 3]),
            Source::Box { min, max } => ([rng.range(min[0], max[0]), rng.range(min[1], max[1]), rng.range(min[2], max[2])], [0.0; 3]),
            Source::Surface { tris, normals } => {
                if tris.is_empty() {
                    return;
                }
                let x = rng.next() * areas.last().copied().unwrap_or(0.0);
                let k = areas.partition_point(|&a| a < x).min(tris.len() - 1);
                let (mut u, mut v) = (rng.next(), rng.next());
                if u + v > 1.0 {
                    u = 1.0 - u;
                    v = 1.0 - v;
                }
                let t = tris[k];
                (add(t[0], add(scale(sub(t[1], t[0]), u), scale(sub(t[2], t[0]), v))), normals[k])
            }
        };
        let speed = length(e.velocity);
        let dir = if speed > 0.0 { normalise(add(scale(normalise(e.velocity), 1.0 - e.spread.clamp(0.0, 1.0)), scale(rng.dir(), e.spread.clamp(0.0, 1.0)))) } else { rng.dir() };
        let v = add(scale(dir, speed * rng.range(0.7, 1.1)), scale(normal, e.normal_speed));
        let v = if speed == 0.0 && e.normal_speed == 0.0 { scale(rng.dir(), e.turbulence * 0.2) } else { v };
        let life = (e.life * (1.0 + e.life_spread * rng.range(-1.0, 1.0))).max(0.05);
        live.push(P { pos, vel: v, age: 0.0, life, stuck: false, seed: (rng.next() * 1e6) as u32 });
    };
    for _ in 0..e.burst {
        spawn(&mut rng, &mut live);
    }
    let radius_of = |p: &P| {
        let k = (p.age / p.life).clamp(0.0, 1.0);
        e.size * (1.0 + (e.size_end - 1.0) * k) * 0.5
    };
    for f in 0..frames {
        let t = f as f32 * dt;
        if f > 0 {
            if t <= e.duration {
                owed += e.rate * dt;
                while owed >= 1.0 {
                    spawn(&mut rng, &mut live);
                    owed -= 1.0;
                }
            }
            for _ in 0..sub_n {
                for p in &mut live {
                    p.age += h;
                    if p.stuck {
                        continue;
                    }
                    let mut acc = scale(gravity, e.gravity);
                    let mut air = e.wind;
                    if e.turbulence > 0.0 {
                        air = add(air, scale(crate::curl(add(p.pos, [0.0, 0.0, t * 50.0]), e.swirl_size, p.seed % 3 + e.seed as u32), e.turbulence));
                    }
                    acc = add(acc, scale(sub(air, p.vel), e.drag.clamp(0.0, 20.0)));
                    p.vel = add(p.vel, scale(acc, h));
                    let before = p.pos;
                    p.pos = add(p.pos, scale(p.vel, h));
                    let r = (e.size * 0.5).max(0.1);
                    for c in colliders {
                        if let Some(hit) = c.push_out(p.pos, r) {
                            match e.on_hit {
                                OnHit::Die => p.age = p.life + 1.0,
                                OnHit::Stick => {
                                    p.pos = hit.point;
                                    p.stuck = true;
                                    p.vel = [0.0; 3];
                                }
                                OnHit::Bounce => {
                                    p.pos = hit.point;
                                    let vn = dot(p.vel, hit.normal);
                                    if vn < 0.0 {
                                        let tangent = sub(p.vel, scale(hit.normal, vn));
                                        p.vel = add(scale(tangent, 1.0 - e.friction.max(hit.friction * 0.5).clamp(0.0, 1.0)), scale(hit.normal, -vn * e.bounce));
                                    }
                                }
                            }
                            let _ = before;
                        }
                    }
                }
            }
            live.retain(|p| p.age <= p.life);
        }
        let lerp_c = |k: f32| -> [u8; 4] { std::array::from_fn(|i| (e.colour[i] as f32 + (e.colour_end[i] as f32 - e.colour[i] as f32) * k).round().clamp(0.0, 255.0) as u8) };
        out.push(
            live.iter()
                .map(|p| {
                    let k = (p.age / p.life).clamp(0.0, 1.0);
                    // A short fade in and out so nothing pops.
                    let mut c = lerp_c(k);
                    let fade = (p.age / 0.06).min(1.0).min((p.life - p.age) / 0.15).clamp(0.0, 1.0);
                    c[3] = (c[3] as f32 * fade) as u8;
                    Point { pos: p.pos, size: radius_of(p) * 2.0, rgba: c, heat: e.heat + (e.heat_end - e.heat) * k }
                })
                .collect(),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparks_burst_fall_bounce_and_cool() {
        let e = preset("sparks", [0.0, 0.0, 0.0], 400.0).unwrap();
        let floor = Collider::Ground { y: 200.0, friction: 0.4 };
        let frames = run(&e, [0.0, 980.0, 0.0], std::slice::from_ref(&floor), 1.5, 30.0);
        assert_eq!(frames[0].len(), 300);
        // Nothing goes through the floor; they cool as they age.
        for f in &frames {
            assert!(f.iter().all(|p| p.pos[1] <= 200.0 + 0.01));
        }
        let h0: f32 = frames[1].iter().map(|p| p.heat).sum::<f32>() / frames[1].len() as f32;
        let h1: f32 = frames[30].iter().map(|p| p.heat).sum::<f32>() / frames[30].len().max(1) as f32;
        assert!(h1 < h0);
        // Snow lands on a ball and stays there.
        let snow = Emitter { source: Source::Box { min: [-30.0, -200.0, -30.0], max: [30.0, -190.0, 30.0] }, rate: 300.0, life: 10.0, velocity: [0.0, 200.0, 0.0], spread: 0.0, on_hit: OnHit::Stick, drag: 0.0, ..Default::default() };
        let ball = Collider::Sphere { centre: [0.0, 0.0, 0.0], radius: 80.0 };
        let fr = run(&snow, [0.0, 980.0, 0.0], std::slice::from_ref(&ball), 2.0, 30.0);
        let last = fr.last().unwrap();
        let resting = last.iter().filter(|p| (length(p.pos) - 82.0).abs() < 3.0).count();
        assert!(resting > 50, "{resting} flakes on the ball");
    }
}
