// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Footsteps: a heel strike and a toe roll on a surface, each step a little
//! different. A step is the body of the impact (a low thud that grows with
//! weight) plus the surface's own sound: the click of stone, the hollow knock
//! of wood, the grains of gravel, the crunch of snow, the squelch of mud.
//! Grass is turf pressed down: blades bending and snapping over the time
//! the foot takes to settle (tens of milliseconds, longer for a heavier
//! foot), never the instant crack of a clap. Animals walk on padded paws,
//! on four feet, and land from hops with the whole body.

use crate::dsp::*;

/// What is underfoot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Surface {
    /// Lawn and meadow.
    Grass,
    /// Dry leaves and twigs.
    Leaves,
    /// Packed earth.
    Dirt,
    /// Loose stones.
    Gravel,
    /// Stone, concrete, pavement.
    Stone,
    /// Wooden floorboards.
    Wood,
    /// Snow.
    Snow,
    /// Sand.
    Sand,
    /// Mud.
    Mud,
    /// Shallow water.
    Water,
    /// A metal walkway.
    Metal,
    /// Carpet.
    Carpet,
    /// Tile.
    Tile,
    /// Wet pavement: stone with a film of water.
    WetStone,
}

impl Surface {
    /// Every surface.
    pub const ALL: [Surface; 14] = [Surface::Grass, Surface::Leaves, Surface::Dirt, Surface::Gravel, Surface::Stone, Surface::Wood, Surface::Snow, Surface::Sand, Surface::Mud, Surface::Water, Surface::Metal, Surface::Carpet, Surface::Tile, Surface::WetStone];

    /// The name.
    pub fn name(self) -> &'static str {
        match self {
            Surface::Grass => "grass",
            Surface::Leaves => "leaves",
            Surface::Dirt => "dirt",
            Surface::Gravel => "gravel",
            Surface::Stone => "stone",
            Surface::Wood => "wood",
            Surface::Snow => "snow",
            Surface::Sand => "sand",
            Surface::Mud => "mud",
            Surface::Water => "water",
            Surface::Metal => "metal",
            Surface::Carpet => "carpet",
            Surface::Tile => "tile",
            Surface::WetStone => "wet pavement",
        }
    }

    /// A surface from words ("on gravel", "cobbles", "a wooden floor").
    pub fn parse(text: &str) -> Option<Surface> {
        let t = format!(" {} ", text.to_lowercase().replace([',', '.'], " "));
        let has = |ws: &[&str]| ws.iter().any(|w| t.contains(&format!(" {w} ")) || t.contains(&format!(" {w}s ")));
        Some(if has(&["wet pavement", "wet road", "wet street", "wet asphalt", "wet concrete", "wet sidewalk", "wet stone", "wet cobbles", "wet cobblestones", "rain-soaked", "rainy street", "rainy pavement", "slick pavement"]) {
            Surface::WetStone
        } else if has(&["gravel", "pebble", "pebbles", "shingle", "rubble", "stones", "driveway"]) {
            Surface::Gravel
        } else if has(&["leaves", "leaf", "twigs", "twig", "forest floor", "undergrowth", "brush", "autumn"]) {
            Surface::Leaves
        } else if has(&["grass", "meadow", "lawn", "field", "moss"]) {
            Surface::Grass
        } else if has(&["snow", "ice", "frost"]) {
            Surface::Snow
        } else if has(&["sand", "beach", "desert", "dune"]) {
            Surface::Sand
        } else if has(&["mud", "swamp", "marsh", "bog", "muddy"]) {
            Surface::Mud
        } else if has(&["water", "puddle", "puddles", "shallows", "stream", "river", "pond", "splash", "splashing"]) {
            Surface::Water
        } else if has(&["wood", "wooden", "floorboard", "floorboards", "boards", "planks", "deck", "bridge", "parquet", "creaky"]) {
            Surface::Wood
        } else if has(&["metal", "steel", "iron", "grate", "catwalk", "stairs"]) {
            Surface::Metal
        } else if has(&["carpet", "rug", "carpeted"]) {
            Surface::Carpet
        } else if has(&["tile", "tiles", "tiled", "marble", "linoleum", "hospital", "corridor"]) {
            Surface::Tile
        } else if has(&["stone", "concrete", "pavement", "cobblestone", "cobblestones", "cobbles", "sidewalk", "asphalt", "road", "street", "cave", "flagstone"]) {
            Surface::Stone
        } else if has(&["dirt", "earth", "soil", "path", "trail", "ground"]) {
            Surface::Dirt
        } else {
            return None;
        })
    }
}

/// What is on the foot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shoe {
    /// Heavy boots.
    Boot,
    /// Soft soles.
    Sneaker,
    /// Hard heels that click.
    Heel,
    /// Bare feet.
    Bare,
    /// An animal's padded paws (claws tick on hard ground).
    Paw,
}

impl Shoe {
    /// A shoe from words.
    pub fn parse(text: &str) -> Option<Shoe> {
        let t = format!(" {} ", text.to_lowercase());
        let has = |ws: &[&str]| ws.iter().any(|w| t.contains(&format!(" {w} ")));
        if has(&["heels", "heel", "stilettos", "high-heeled", "dress shoes", "leather shoes", "brogues"]) {
            Some(Shoe::Heel)
        } else if has(&["boots", "boot", "hiking", "army", "marching", "heavy", "soldier", "soldiers"]) {
            Some(Shoe::Boot)
        } else if has(&["paws", "paw", "pads", "padding", "pawsteps", "pawprints"]) {
            Some(Shoe::Paw)
        } else if has(&["barefoot", "bare", "barefooted"]) {
            Some(Shoe::Bare)
        } else if has(&["sneakers", "trainers", "soft", "slippers", "socks", "sneaker"]) {
            Some(Shoe::Sneaker)
        } else {
            None
        }
    }
}

/// A burst of noise through a band-pass, decaying.
fn burst(rng: &mut Rng, len: f32, hz: f32, q: f32, tau: f32) -> Vec<f32> {
    let n = frames(len);
    let mut f = Svf::new();
    let c = SvfCoef::new(hz, q);
    (0..n).map(|i| f.tick(&c, rng.signed()).bp * (-(i as f32 / SR) / tau).exp()).collect()
}

/// A decaying ring at `hz`.
fn ring(hz: f32, len: f32, tau: f32) -> Vec<f32> {
    let n = frames(len);
    let mut r = Rot::new(hz, 0.0);
    (0..n).map(|i| r.tick() * (-(i as f32 / SR) / tau).exp()).collect()
}

/// Many small grains scattered in `len` seconds: crunch and gravel.
fn grains(rng: &mut Rng, len: f32, count: usize, lo: f32, hi: f32, tau: f32, bend: f32) -> Vec<f32> {
    let mut out = vec![0.0; frames(len + 0.06)];
    for _ in 0..count {
        // Grains cluster early in the step.
        let at = rng.unit().powf(1.5) * len;
        let hz = lo * (hi / lo).powf(rng.unit());
        let tt = tau * rng.range(0.5, 1.5);
        let g = burst(rng, 0.05, hz, 1.8, tt);
        mix_into(&mut out, &g, frames(at), rng.range(0.15, 1.0) * bend);
    }
    out
}

/// Turf pressed by a foot of `weight` 0..1: blades bending and snapping
/// while the foot settles (a granular rustle that swells over 15-70 ms,
/// longer for a heavier foot) over the hush of the turf compressing.
fn turf(rng: &mut Rng, weight: f32, force: f32) -> Vec<f32> {
    let settle = 0.015 + 0.055 * weight;
    let len = settle + 0.12;
    let mut out = vec![0.0; frames(len + 0.05)];
    // The hush of the turf giving way: low-mid noise, soft at the start.
    let mut f = Svf::new();
    let c = SvfCoef::new(380.0 + 300.0 * (1.0 - weight), 0.8);
    for (i, o) in out.iter_mut().enumerate().take(frames(len)) {
        let t = i as f32 / SR;
        let env = smooth(t, settle * 0.6) * (-(t - settle).max(0.0) / (0.03 + 0.04 * weight)).exp();
        *o += f.tick(&c, rng.signed()).bp * env * 0.9;
    }
    // Blades: many small ticks, more for a bigger foot, spread over the settle.
    let count = 14 + (40.0 * weight) as usize;
    for _ in 0..count {
        let at = settle * rng.unit().powf(0.7) + rng.range(0.0, 0.03);
        let hz = 700.0 * (3200.0f32 / 700.0).powf(rng.unit()) * (1.15 - 0.3 * weight);
        let tau = rng.range(0.002, 0.005);
        let g = burst(rng, 0.02, hz, 1.4, tau);
        mix_into(&mut out, &g, frames(at), rng.range(0.05, 0.22) * (0.5 + 0.5 * force));
    }
    out
}

/// A single impact of a foot: `weight` 0..1, `force` 0..1 for how hard this step lands.
fn impact(surface: Surface, shoe: Shoe, weight: f32, force: f32, heel: bool, rng: &mut Rng) -> Vec<f32> {
    let len = frames(0.5);
    let mut out = vec![0.0; len];
    // The body: a soft low thud.
    // Heavier feet thud lower and longer; a mouse's barely thuds at all.
    let thud_hz = (if heel { 85.0 } else { 120.0 }) * (1.45 - 0.8 * weight) * rng.range(0.92, 1.08);
    let thud_tau = 0.022 + 0.015 * weight + if shoe == Shoe::Paw { 0.03 * weight } else { 0.0 };
    let body = {
        let mut f = Svf::new();
        let c = SvfCoef::new(thud_hz, 1.0);
        let mut ph = 0.0f32;
        (0..frames(0.12 + 2.0 * thud_tau)).map(|i| {
            let t = i as f32 / SR;
            ph += std::f32::consts::TAU * thud_hz * (1.0 + 0.6 * (-t / 0.015).exp()) / SR;
            // A padded foot presses in over a few milliseconds.
            let press = if shoe == Shoe::Paw { smooth(t, 0.004 + 0.006 * weight) } else { 1.0 };
            (ph.sin() * 0.8 + f.tick(&c, rng.signed()).lp * 0.4) * (-t / thud_tau).exp() * press
        }).collect::<Vec<f32>>()
    };
    let body_gain = match shoe {
        Shoe::Boot => 1.0,
        Shoe::Sneaker => 0.6,
        Shoe::Heel => 0.5,
        Shoe::Bare => 0.45,
        Shoe::Paw => 0.9,
    } * if shoe == Shoe::Paw { 0.1 + 1.1 * weight } else { 0.4 + 0.8 * weight }
        * match surface {
            Surface::Carpet => 0.6,
            // On a hard floor the foot is heard in the floor's own ring, not as a thud.
            Surface::Stone | Surface::Tile | Surface::Metal | Surface::WetStone => 0.4,
            Surface::Wood => 0.55,
            _ => 1.0,
        };
    mix_into(&mut out, &body, 0, body_gain * force);
    // The surface.
    let tex: Vec<f32> = match surface {
        Surface::Grass => turf(rng, weight, force),
        Surface::Leaves => grains(rng, 0.22, 28, 1800.0, 7000.0, 0.012, 1.0),
        Surface::Dirt => {
            let mut s = burst(rng, 0.14, 1100.0, 0.7, 0.04);
            let g = grains(rng, 0.1, 8, 800.0, 3000.0, 0.01, 0.4);
            mix_into(&mut s, &g, 0, 0.7);
            s
        }
        Surface::Gravel => grains(rng, 0.32, 70, 1200.0, 6500.0, 0.014, 1.0),
        Surface::Stone => {
            let mut s = burst(rng, 0.03, if matches!(shoe, Shoe::Heel) { 3200.0 } else { 1800.0 }, 1.4, 0.004);
            let r = ring(rng.range(650.0, 950.0), 0.12, 0.03);
            mix_into(&mut s, &r, 0, 0.18);
            s
        }
        Surface::Tile => {
            let mut s = burst(rng, 0.03, if matches!(shoe, Shoe::Heel) { 3600.0 } else { 2200.0 }, 1.5, 0.003);
            let r = ring(rng.range(1500.0, 2300.0), 0.18, 0.05);
            mix_into(&mut s, &r, 0, 0.12);
            s
        }
        Surface::Wood => {
            let hz = rng.range(170.0, 260.0);
            let mut s = ring(hz, 0.25, 0.07);
            let r2 = ring(hz * 2.45, 0.15, 0.04);
            mix_into(&mut s, &r2, 0, 0.5);
            let c = burst(rng, 0.02, 1500.0, 1.0, 0.004);
            mix_into(&mut s, &c, 0, 0.5);
            s
        }
        Surface::Snow => {
            let mut s = grains(rng, 0.28, 50, 500.0, 2600.0, 0.02, 0.8);
            let squeak = ring(rng.range(1800.0, 2800.0), 0.05, 0.02);
            mix_into(&mut s, &squeak, frames(0.03), 0.08);
            s
        }
        Surface::Sand => {
            let mut s = burst(rng, 0.3, 3000.0, 0.45, 0.11);
            let g = grains(rng, 0.2, 30, 2500.0, 7000.0, 0.006, 0.3);
            mix_into(&mut s, &g, 0, 0.6);
            s.iter_mut().for_each(|v| *v *= 0.5);
            s
        }
        Surface::Mud => {
            let mut s = burst(rng, 0.25, 300.0, 0.8, 0.07);
            let mut ph = 0.0f32;
            let hz0 = rng.range(110.0, 220.0);
            let squelch: Vec<f32> = (0..frames(0.15)).map(|i| {
                let t = i as f32 / SR;
                ph += std::f32::consts::TAU * (hz0 + 500.0 * t) / SR;
                ph.sin() * (-t / 0.04).exp()
            }).collect();
            mix_into(&mut s, &squelch, frames(0.04), 0.5);
            let sl = burst(rng, 0.2, 1200.0, 0.8, 0.06);
            mix_into(&mut s, &sl, frames(0.05), 0.3);
            s
        }
        Surface::Water => {
            let mut s = burst(rng, 0.3, 1800.0, 0.5, 0.1);
            for k in 0..6 {
                let hz = rng.range(500.0, 2200.0);
                let mut ph = 0.0f32;
                let b: Vec<f32> = (0..frames(0.06)).map(|i| {
                    let t = i as f32 / SR;
                    ph += std::f32::consts::TAU * hz * (1.0 + 4.0 * t) / SR;
                    ph.sin() * (-t / 0.015).exp()
                }).collect();
                mix_into(&mut s, &b, frames(0.01 + k as f32 * 0.03 * rng.unit()), 0.5);
            }
            s
        }
        Surface::Metal => {
            let hz = rng.range(520.0, 760.0);
            let mut s = ring(hz, 0.5, 0.12);
            for (r, g, tau) in [(2.4f32, 0.5f32, 0.09f32), (3.9, 0.35, 0.06), (5.6, 0.2, 0.04)] {
                let x = ring(hz * r, 0.4, tau);
                mix_into(&mut s, &x, 0, g);
            }
            let c = burst(rng, 0.02, 3000.0, 1.0, 0.003);
            mix_into(&mut s, &c, 0, 0.6);
            s
        }
        Surface::Carpet => burst(rng, 0.1, 320.0, 1.6, 0.03),
        Surface::WetStone => {
            // The stone's tap, a slap through the film of water and a few
            // small splashes off the sole.
            let mut s = burst(rng, 0.03, if matches!(shoe, Shoe::Heel) { 3000.0 } else { 1700.0 }, 1.3, 0.004);
            let slap = burst(rng, 0.12, 2600.0, 0.6, 0.035);
            mix_into(&mut s, &slap, 0, 0.55);
            for k in 0..3 {
                let hz = rng.range(900.0, 2600.0);
                let mut ph = 0.0f32;
                let b: Vec<f32> = (0..frames(0.04)).map(|i| {
                    let t = i as f32 / SR;
                    ph += std::f32::consts::TAU * hz * (1.0 + 5.0 * t) / SR;
                    ph.sin() * (-t / 0.01).exp()
                }).collect();
                mix_into(&mut s, &b, frames(0.008 + k as f32 * 0.02 * rng.unit()), 0.3);
            }
            s
        }
    };
    let tex_gain = match shoe {
        Shoe::Boot => 1.0,
        Shoe::Sneaker => 0.75,
        Shoe::Heel => 1.0,
        Shoe::Bare => 0.55,
        Shoe::Paw => 0.7,
    } * (0.4 + 0.9 * force);
    mix_into(&mut out, &tex, 0, tex_gain);
    // A hard heel clicks on anything hard.
    if shoe == Shoe::Heel && matches!(surface, Surface::Stone | Surface::Tile | Surface::Wood | Surface::Metal | Surface::WetStone) {
        let hz = rng.range(2800.0, 4200.0);
        let c = burst(rng, 0.012, hz, 2.0, 0.002);
        mix_into(&mut out, &c, 0, 0.9 * force);
    }
    // Claws tick on hard ground.
    if shoe == Shoe::Paw && matches!(surface, Surface::Stone | Surface::Tile | Surface::Wood | Surface::Metal | Surface::WetStone) {
        for k in 0..3 {
            let hz = rng.range(3000.0, 5200.0);
            let c = burst(rng, 0.006, hz, 2.5, 0.0012);
            mix_into(&mut out, &c, frames(0.002 + 0.004 * k as f32 * rng.unit()), 0.3 * force * (1.0 - 0.5 * weight));
        }
    }
    // A bare foot slaps.
    if shoe == Shoe::Bare {
        let c = burst(rng, 0.03, 700.0, 0.8, 0.008);
        mix_into(&mut out, &c, 0, 0.4 * force);
    }
    out
}

/// One footfall: heel strike, then the ball of the foot a moment later.
pub fn step(surface: Surface, shoe: Shoe, weight: f32, force: f32, rng: &mut Rng) -> Vec<f32> {
    let roll = frames(rng.range(0.07, 0.12) * (1.2 - 0.4 * force));
    let heel = impact(surface, shoe, weight, force, true, rng);
    let toe = impact(surface, shoe, weight * 0.7, force * 0.65, false, rng);
    let mut out = vec![0.0; heel.len().max(roll + toe.len())];
    mix_into(&mut out, &heel, 0, 1.0);
    mix_into(&mut out, &toe, roll, 0.65);
    out
}

/// What a walk is made of.
#[derive(Clone, Copy, Debug)]
pub struct Walk {
    /// The ground.
    pub surface: Surface,
    /// The footwear.
    pub shoe: Shoe,
    /// Steps in all.
    pub steps: usize,
    /// Steps a second (about 1.8 for a walk, 2.8 for a run).
    pub pace: f32,
    /// 0 light, 1 heavy.
    pub weight: f32,
    /// How hard each foot lands, 0..1.
    pub force: f32,
    /// On four feet: footfalls come in pairs (hind then fore), a da-dum
    /// rather than an even left-right.
    pub four_feet: bool,
}

/// A walk, stereo, with the left and right feet a little apart; each step's
/// timing and strength drift. Returns the sound and the time of each step.
pub fn walk(w: &Walk, seed: u64) -> (Stereo, Vec<f32>) {
    let mut rng = Rng::new(seed ^ 0xF007);
    let gap = 1.0 / w.pace.max(0.3);
    let steps = w.steps.clamp(1, 400);
    let len = frames(gap * steps as f32 + 0.7);
    let mut out = Stereo::silent(len);
    let mut times = Vec::new();
    let mut t = 0.05 + rng.range(0.0, 0.08);
    let mut mood = 0.0f32;
    for i in 0..steps {
        mood = mood * 0.7 + rng.signed() * 0.3;
        let force = (w.force * (1.0 + 0.2 * mood) * rng.range(0.9, 1.05)).clamp(0.1, 1.0);
        let s = step(w.surface, w.shoe, w.weight, force, &mut rng);
        let pan = if i % 2 == 0 { -0.12 } else { 0.12 };
        out.add_mono(&s, frames(t), 1.0, pan);
        times.push(t);
        let lilt = if w.four_feet { if i % 2 == 0 { 0.65 } else { 1.35 } } else { 1.0 };
        t += gap * lilt * (1.0 + 0.06 * mood) + rng.range(-0.012, 0.012);
    }
    (out, times)
}

/// A body landing from a hop on four padded feet: the fore feet, then the
/// hind feet together a moment later, heavier, with the whole body's weight
/// behind them. `weight` 0..1, `force` 0..1.
pub fn landing(surface: Surface, weight: f32, force: f32, rng: &mut Rng) -> Vec<f32> {
    let gap = frames(rng.range(0.025, 0.045));
    let fore = impact(surface, Shoe::Paw, weight * 0.8, force * 0.6, false, rng);
    let hind = impact(surface, Shoe::Paw, (weight * 1.3).min(1.0), force, true, rng);
    // The body settling: a low push the feet alone do not make.
    let hz = 70.0 + 90.0 * (1.0 - weight);
    let body: Vec<f32> = (0..frames(0.15)).map(|i| {
        let t = i as f32 / SR;
        (std::f32::consts::TAU * hz * t).sin() * smooth(t, 0.006) * (-t / (0.03 + 0.04 * weight)).exp()
    }).collect();
    let mut out = vec![0.0; (gap + hind.len()).max(fore.len()).max(gap + body.len())];
    mix_into(&mut out, &fore, 0, 0.7);
    mix_into(&mut out, &hind, gap, 1.0);
    mix_into(&mut out, &body, gap, 0.6 * force);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_surface_sounds_and_steps_differ() {
        for s in Surface::ALL {
            let w = Walk { surface: s, shoe: Shoe::Boot, steps: 4, pace: 2.0, weight: 0.6, force: 0.8, four_feet: false };
            let (a, times) = walk(&w, 1);
            let (b, _) = walk(&w, 2);
            assert_eq!(times.len(), 4);
            assert!(a.peak() > 0.05 && a.l.iter().all(|v| v.is_finite()), "{s:?}");
            assert_ne!(a, b, "{s:?}: two walks the same");
            // Steps within a walk are not copies of each other.
            let n = frames(0.4);
            let s0 = &a.l[frames(times[0])..frames(times[0]) + n];
            let s1 = &a.l[frames(times[1])..frames(times[1]) + n];
            assert!(s0.iter().zip(s1).map(|(x, y)| (x - y).abs()).sum::<f32>() > 0.5, "{s:?}: steps are identical");
        }
    }

    #[test]
    fn surfaces_sound_different_from_each_other() {
        let centroid = |s: Surface| {
            let w = Walk { surface: s, shoe: Shoe::Sneaker, steps: 3, pace: 2.0, weight: 0.5, force: 0.8, four_feet: false };
            let (a, _) = walk(&w, 5);
            rms(&high_pass(&a.l, 1500.0, 0.7)) / rms(&a.l).max(1e-9)
        };
        assert!(centroid(Surface::Gravel) > centroid(Surface::Carpet) * 1.5);
        assert!(centroid(Surface::Tile) > centroid(Surface::Carpet) * 1.2);
    }

    #[test]
    fn pace_sets_the_spacing_and_words_find_surfaces() {
        let w = Walk { surface: Surface::Stone, shoe: Shoe::Heel, steps: 6, pace: 2.0, weight: 0.4, force: 0.8, four_feet: false };
        let (_, t) = walk(&w, 3);
        let mean = (t[5] - t[0]) / 5.0;
        assert!((mean - 0.5).abs() < 0.05, "{mean}");
        assert_eq!(Surface::parse("footsteps on gravel"), Some(Surface::Gravel));
        assert_eq!(Surface::parse("walking on a wooden floor"), Some(Surface::Wood));
        assert_eq!(Surface::parse("crunching through snow"), Some(Surface::Snow));
        assert_eq!(Shoe::parse("a woman in heels"), Some(Shoe::Heel));
    }
}
