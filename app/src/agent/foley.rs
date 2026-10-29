// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `foley`: sounds tied to what characters do. A puppet that walks, runs,
//! sneaks or hops gets its footfalls on the frames its feet land (from the
//! same step cycle that moves its pins), made for the ground it is on
//! (grass, leaves, gravel, stone, wood, snow, sand, mud, water) and
//! weighted by its size in the character bible; swims paddle, pecks tap.
//! One-off action sounds (splash, rustle, thud, flap, swish) are made to
//! measure at a moment. Every act is remembered on the clip, so footsteps
//! can be laid (or laid again on another ground) at any time. Contacts can
//! also be given as times (`contacts`), from keys or a gait worked out
//! elsewhere. Steps and landings are `studio-synth`'s physical footfalls:
//! the foot's weight and the ground (turf pressed down, not a clap), on
//! padded paws for an animal on four feet.

use super::synth::{Band, Noise, RATE};
use super::{Session, f, micros, secs};
use serde_json::{Value, json};
use std::f32::consts::PI;
use studio_model as m;

/// The grounds footsteps are made for.
pub const SURFACES: &str = "grass, leaves, dirt, gravel, stone, wood, snow, sand, mud, water";

/// One-off action sounds.
pub const ACTIONS: &str = "splash, rustle, thud, flap, swish";

fn surface_of(s: &str) -> Option<&'static str> {
    Some(match s.trim().to_lowercase().as_str() {
        "grass" | "meadow" | "lawn" | "field" | "moss" => "grass",
        "leaves" | "forest" | "forest_floor" | "twigs" | "undergrowth" | "brush" => "leaves",
        "dirt" | "earth" | "soil" | "path" | "trail" | "ground" => "dirt",
        "gravel" | "pebbles" | "shingle" | "rubble" => "gravel",
        "stone" | "rock" | "concrete" | "pavement" | "tile" | "tiles" | "cave" | "marble" | "road" | "asphalt" | "tarmac" | "street" | "sidewalk" | "kerb" | "kerbs" | "curb" | "paving" | "cobbles" | "cobblestone" | "brick" => "stone",
        "wood" | "wooden" | "floor" | "boards" | "planks" | "deck" | "bridge" => "wood",
        "snow" | "ice" | "frost" => "snow",
        "sand" | "beach" | "desert" | "dune" => "sand",
        "mud" | "swamp" | "marsh" | "bog" => "mud",
        "water" | "shallows" | "puddle" | "stream" | "river" | "pond" => "water",
        _ => return None,
    })
}

/// A sound event in a foley track: when (s), how hard (0..1), what.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    /// A foot on the ground.
    Step,
    /// Landing from a hop or jump.
    Land,
    /// A paddle through water.
    Paddle,
    /// A beak or a knock on the ground.
    Tap,
}

fn env(t: f32, attack: f32, decay: f32) -> f32 {
    if t < 0.0 {
        0.0
    } else if t < attack {
        t / attack
    } else {
        (-(t - attack) / decay).exp()
    }
}

/// Adds one footfall (or tap, landing, paddle) of `w` at sample `at`;
/// `size` > 1 is heavier and lower.
fn hit(l: &mut [f32], r: &mut [f32], at: usize, kind: Hit, surface: &str, w: f32, size: f32, pan: f32, rng: &mut Noise) {
    let heavy = size.clamp(0.3, 4.0).powf(0.5);
    let low = 1.0 / heavy;
    let (len, w) = match kind {
        Hit::Land => (0.32, (w * 1.3).min(1.0)),
        Hit::Paddle => (0.35, w),
        Hit::Tap => (0.06, w * 0.7),
        Hit::Step => (0.22, w),
    };
    let n = ((len * heavy.max(1.0)) * RATE as f32) as usize;
    let (mut b1, mut b2, mut b3) = (Band::new(), Band::new(), Band::new());
    let mut ph = 0f32;
    // Grains: little cracks for crunchy grounds.
    let grains: Vec<(f32, f32)> = (0..24).map(|_| (rng.unit() * 0.12, 0.3 + 0.7 * rng.unit())).collect();
    let surface = if kind == Hit::Paddle { "water" } else { surface };
    for i in 0..n {
        let k = at + i;
        if k >= l.len() {
            break;
        }
        let t = i as f32 / RATE as f32;
        let x = rng.next();
        let grain = |dur: f32| grains.iter().map(|(g, a)| if t >= *g && t < g + 0.004 { a * (1.0 - (t - g) / 0.004) * dur } else { 0.0 }).sum::<f32>();
        // The thump every step has under it, lower for bigger feet.
        let thump_hz = 70.0 * low + 40.0 * (-t / 0.02).exp();
        ph += 2.0 * PI * thump_hz / RATE as f32;
        let thump = ph.sin() * env(t, 0.002, 0.025 * heavy) * if kind == Hit::Land { 1.4 } else { 0.6 };
        let v = match surface {
            "grass" => b1.run(x, 3200.0 * low.sqrt(), 1.1).0 * env(t, 0.006, 0.05) * 1.3 + 0.35 * thump,
            "leaves" => b1.run(x * (0.3 + 4.0 * grain(1.0)), 4200.0, 0.9).0 * env(t, 0.004, 0.09) * 1.6 + 0.3 * thump,
            "dirt" => b1.run(x, 900.0 * low, 1.0).0 * env(t, 0.003, 0.04) + 0.9 * thump,
            "gravel" => b1.run(x * (0.4 + 5.0 * grain(1.0)), 2400.0, 0.8).0 * env(t, 0.004, 0.07) * 1.5 + 0.5 * thump,
            "stone" => b1.run(x, 2600.0, 0.6).0 * env(t, 0.0005, 0.012) * 1.8 + 0.7 * thump,
            "wood" => {
                let knock = (2.0 * PI * 190.0 * low * t).sin() * env(t, 0.001, 0.06) + (2.0 * PI * 410.0 * low * t).sin() * 0.4 * env(t, 0.001, 0.03);
                knock * 1.1 + b1.run(x, 2000.0, 0.8).0 * env(t, 0.0005, 0.008) + 0.4 * thump
            }
            "snow" => {
                let crunch = 0.5 + 0.5 * (2.0 * PI * 38.0 * t).sin().abs();
                b1.run(x * crunch, 1300.0, 0.25).0 * env(t, 0.02, 0.07) * 1.4 + 0.25 * thump
            }
            "sand" => b1.run(x, 2800.0, 1.4).0 * env(t, 0.015, 0.06) + 0.3 * thump,
            "mud" => {
                let sweep = 320.0 - 200.0 * (t / len).min(1.0);
                b1.run(x, 380.0, 0.7).1 * env(t, 0.01, 0.07) * 1.4 + (2.0 * PI * sweep * low * t).sin() * 0.5 * env(t, 0.01, 0.05) + 0.3 * thump
            }
            "water" => {
                let sweep = 900.0 + 2600.0 * (t / len).min(1.0);
                let bubbles = (2.0 * PI * (600.0 + 2400.0 * t) * t).sin() * 0.25 * env(t - 0.05, 0.01, 0.05);
                b1.run(x, sweep, 0.9).0 * env(t, 0.012, 0.09) * 1.6 + bubbles + 0.2 * b2.run(x, 300.0, 1.0).1 * env(t, 0.01, 0.1)
            }
            _ => b1.run(x, 1500.0, 1.0).0 * env(t, 0.004, 0.05) + 0.5 * thump,
        };
        let v = if kind == Hit::Tap { b3.run(v, 2500.0, 0.7).0 * 1.5 } else { v };
        let v = v * w;
        l[k] += v * (1.0 - pan).sqrt();
        r[k] += v * pan.sqrt();
    }
}

/// Who is walking: how heavy a foot is (0..1) and whether it is a paw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Feet {
    /// 0 a mouse .. 1 a bear.
    pub weight: f32,
    /// Padded paws (an animal on four feet).
    pub paws: bool,
}

impl Feet {
    /// Feet for a character of `size` (1 a person; bigger is heavier).
    pub fn of(size: f32, paws: bool) -> Feet {
        Feet { weight: (0.45 * size.max(0.0).sqrt()).clamp(0.03, 1.0), paws }
    }
}

/// A foley track `dur` seconds long with `hits` at their times.
pub fn track(hits: &[(f64, f32, Hit)], dur: f64, surface: &str, size: f32, pan: f32, seed: u64) -> (Vec<f32>, Vec<f32>) {
    track_for(hits, dur, surface, Feet::of(size, false), pan, seed)
}

/// A foley track for these feet: steps and landings from the physical
/// footfall models, paddles and taps made here.
pub fn track_for(hits: &[(f64, f32, Hit)], dur: f64, surface: &str, feet: Feet, pan: f32, seed: u64) -> (Vec<f32>, Vec<f32>) {
    use studio_synth::foley as sf;
    let n = (dur * RATE as f64) as usize + 1;
    let (mut l, mut r) = (vec![0f32; n], vec![0f32; n]);
    let mut rng = Noise(seed | 1);
    let mut srng = studio_synth::dsp::Rng::new(seed);
    let ground = sf::Surface::parse(surface).unwrap_or(sf::Surface::Dirt);
    let shoe = if feet.paws { sf::Shoe::Paw } else { sf::Shoe::Sneaker };
    // The old size scale, for paddles and taps.
    let size = (feet.weight / 0.45).powi(2);
    for (i, (t, w, kind)) in hits.iter().enumerate() {
        // Feet alternate a hair either side of the body.
        let p = (pan + if i % 2 == 0 { -0.04 } else { 0.04 }).clamp(0.0, 1.0);
        let jitter = 0.9 + 0.2 * rng.unit();
        let at = (t * RATE as f64) as usize;
        let made = match kind {
            Hit::Step => Some(sf::step(ground, shoe, feet.weight, (w * jitter).clamp(0.1, 1.0), &mut srng)),
            Hit::Land => Some(sf::landing(ground, feet.weight, (w * jitter).clamp(0.1, 1.0), &mut srng)),
            Hit::Paddle | Hit::Tap => None,
        };
        match made {
            Some(x) => {
                for (k, v) in x.iter().enumerate() {
                    if at + k >= n {
                        break;
                    }
                    l[at + k] += v * (1.0 - p).sqrt();
                    r[at + k] += v * p.sqrt();
                }
            }
            None => hit(&mut l, &mut r, at, *kind, surface, w * jitter, size, p, &mut rng),
        }
    }
    normalise(&mut l, &mut r, 0.5);
    (l, r)
}

fn normalise(l: &mut [f32], r: &mut [f32], to: f32) {
    let peak = l.iter().chain(r.iter()).fold(0f32, |m, v| m.max(v.abs()));
    if peak > 1e-6 {
        let g = to / peak;
        l.iter_mut().chain(r.iter_mut()).for_each(|v| *v *= g);
    }
}

/// A one-off action sound: (left, right, length in seconds).
pub fn action(kind: &str, size: f32, seed: u64) -> Result<(Vec<f32>, Vec<f32>), String> {
    let heavy = size.clamp(0.3, 4.0).sqrt();
    let dur: f32 = match kind {
        "splash" => 0.9 * heavy,
        "rustle" => 0.8,
        "thud" | "impact" => 0.45 * heavy,
        "flap" => 0.6,
        "swish" => 0.35,
        other => return Err(format!("foley kind \"{other}\": {ACTIONS}")),
    };
    let n = (dur * RATE as f32) as usize;
    let (mut l, mut r) = (vec![0f32; n], vec![0f32; n]);
    let mut rng = Noise(seed | 1);
    let (mut bl, mut br, mut b2) = (Band::new(), Band::new(), Band::new());
    let mut ph = 0f32;
    let drops: Vec<(f32, f32, f32)> = (0..30).map(|_| (0.08 + rng.unit() * dur * 0.8, 800.0 + rng.unit() * 2600.0, rng.unit())).collect();
    for i in 0..n {
        let t = i as f32 / RATE as f32;
        let p = t / dur;
        let (a, b) = (rng.next(), rng.next());
        let (x, y) = match kind {
            // A body into water: a burst, a sweep up, droplets after.
            "splash" => {
                let body = env(t, 0.008, 0.18 * heavy);
                let hz = 600.0 / heavy + 3000.0 * p;
                let thump = { ph += 2.0 * PI * (90.0 / heavy) / RATE as f32; ph.sin() * env(t, 0.003, 0.06) };
                let mut drip = 0.0;
                for (at, hz, pan) in &drops {
                    let d = t - at;
                    if (0.0..0.03).contains(&d) {
                        drip += (2.0 * PI * (hz + 4000.0 * d) * d).sin() * (1.0 - d / 0.03) * 0.35 * (0.5 + pan);
                    }
                }
                (bl.run(a, hz, 0.8).0 * body * 1.6 + thump + drip, br.run(b, hz * 1.05, 0.8).0 * body * 1.6 + thump + drip * 0.8)
            }
            // Pushing through leaves and bushes: grainy, swelling, fading.
            "rustle" => {
                let swell = (PI * p).sin().powf(0.7);
                let g = if rng.unit() < 0.02 { 3.0 } else { 0.6 };
                (bl.run(a * g, 3800.0, 0.7).0 * swell * 1.4, br.run(b * g, 4100.0, 0.7).0 * swell * 1.4)
            }
            // A fall, a body hitting the ground.
            "thud" | "impact" => {
                let hz = 55.0 / heavy + 70.0 * (-t / 0.03).exp();
                ph += 2.0 * PI * hz / RATE as f32;
                let v = ph.sin() * env(t, 0.002, 0.09 * heavy) * 1.3 + b2.run(a, 700.0, 1.0).0 * env(t, 0.001, 0.03);
                let v = (1.5 * v).tanh();
                (v, v)
            }
            // Wings: four beats of moving air.
            "flap" => {
                let beat = (p * 4.0).fract();
                let e = (PI * beat).sin().powf(2.0) * (1.0 - 0.4 * p);
                (bl.run(a, 600.0, 0.9).0 * e * 1.5, br.run(b, 650.0, 0.9).0 * e * 1.5)
            }
            // Something fast passing.
            "swish" => {
                let e = (PI * p).sin().powf(3.0);
                let hz = 1200.0 + 4000.0 * (PI * p).sin();
                (bl.run(a, hz, 0.6).0 * e * (1.0 - p), br.run(b, hz, 0.6).0 * e * p)
            }
            _ => unreachable!(),
        };
        let edge = (t / 0.003).min((dur - t) / 0.01).clamp(0.0, 1.0);
        l[i] = x * edge;
        r[i] = y * edge;
    }
    normalise(&mut l, &mut r, 0.7);
    Ok((l, r))
}

/// What an act sounds like per contact.
fn hit_of(act: &str) -> Option<Hit> {
    Some(match act {
        "walk" | "run" | "sneak" => Hit::Step,
        "hop" | "celebrate" => Hit::Land,
        "swim" => Hit::Paddle,
        "peck" => Hit::Tap,
        _ => return None,
    })
}

/// Keeps an act on the clip (`extra.acts`): which acts, from when, how
/// long and how fast, in clip seconds. An act over the same window
/// replaces the one before.
pub(super) fn remember(mc: &mut m::Clip, acts: &[String], from: f64, dur: f64, speed: f64) {
    let mut list: Vec<Value> = mc.extra.get("acts").and_then(Value::as_array).cloned().unwrap_or_default();
    list.retain(|a| {
        let (f0, d0) = (a["from"].as_f64().unwrap_or(0.0), a["duration"].as_f64().unwrap_or(0.0));
        f0 + d0 <= from + 1e-3 || f0 >= from + dur - 1e-3
    });
    list.push(json!({"acts": acts, "from": from, "duration": dur, "speed": speed}));
    list.sort_by(|a, b| a["from"].as_f64().unwrap_or(0.0).total_cmp(&b["from"].as_f64().unwrap_or(0.0)));
    mc.extra.insert("acts".into(), Value::Array(list));
}

impl Session {
    /// The bible's character for a clip: named in the op, or whose id or
    /// name the clip's name holds.
    fn character_of(&self, op: &Value, mc: &m::Clip) -> Option<studio_model::Character> {
        let story = &self.doc.model.story;
        if let Some(c) = op.get("character").and_then(Value::as_str) {
            return story.character(c).cloned();
        }
        let name = mc.name.to_lowercase();
        story.cast.iter().find(|c| name.split(|ch: char| !ch.is_alphanumeric()).any(|w| w == c.id || (!c.name.is_empty() && c.name.to_lowercase().split_whitespace().any(|n| n.len() > 2 && n == w)))).cloned()
    }

    /// The ground at a moment: the op's, else the place of the story's
    /// scene there, else (for a 3D figure, `clip`) what it stands on.
    fn ground_at(&self, op: &Value, t: m::Micros, clip: Option<u32>) -> Result<(&'static str, String), String> {
        if let Some(s) = op.get("surface").and_then(Value::as_str) {
            return surface_of(s).map(|g| (g, "given".to_owned())).ok_or_else(|| format!("surface \"{s}\": {SURFACES}"));
        }
        let story = &self.doc.model.story;
        let scene = story.at(t).map(|x| &story.scenes[x.scene]);
        match scene.and_then(|s| story.place_of(s)).and_then(|p| p.surface.as_deref().map(|g| (p.id.clone(), g.to_owned()))) {
            Some((place, g)) => surface_of(&g).map(|s| (s, format!("the ground of {place}"))).ok_or_else(|| format!("place {place} has surface \"{g}\": use one of {SURFACES}")),
            None => {
                // The material under a 3D figure's feet, by its name.
                let under = clip.and_then(|id| self.footing(id, t)).and_then(|ft| ft.material.to_lowercase().split(|c: char| !c.is_alphanumeric()).find_map(surface_of).map(|g| (g, ft.material)));
                Ok(match under {
                    Some((g, m)) => (g, format!("it stands on {m}")),
                    None => ("grass", "no surface given and no story place with one here: pass surface".into()),
                })
            }
        }
    }

    /// The feet a character walks on: paws for an animal on four feet (by
    /// the op's `feet`, else by the character's look), weighted by size.
    fn feet_of(&self, op: &Value, ch: Option<&studio_model::Character>, size: f32) -> Feet {
        let said = op.get("feet").and_then(Value::as_str).map(str::to_lowercase);
        let paws = match said.as_deref() {
            Some("paws" | "paw" | "pads" | "animal") => true,
            Some(_) => false,
            None => ch.and_then(|c| studio_synth::animal::find(&format!("{} {}", c.look, c.id))).is_some_and(|(sp, _)| studio_synth::animal::body(sp).1),
        };
        Feet::of(size, paws)
    }

    /// `foley`: footsteps (and paddles, taps, landings) for a puppet's
    /// acts, or a one-off action sound at a moment.
    pub(super) fn foley(&mut self, op: &Value) -> Result<String, String> {
        let lane = op.get("lane").cloned().unwrap_or(json!("sound"));
        let volume = f(op, "volume");
        // Contacts given as timeline seconds: steps (or `hit`: land, tap,
        // paddle) exactly there.
        if let Some(cs) = op.get("contacts").and_then(Value::as_array) {
            let mut times: Vec<f64> = cs.iter().filter_map(Value::as_f64).filter(|t| *t >= 0.0).collect();
            if times.is_empty() {
                return Err("foley contacts are timeline seconds, e.g. [1.0, 1.5, 2.0]".into());
            }
            times.sort_by(f64::total_cmp);
            let kind = match op.get("hit").and_then(Value::as_str).unwrap_or("step") {
                "step" => Hit::Step,
                "land" | "landing" | "hop" => Hit::Land,
                "tap" | "peck" => Hit::Tap,
                "paddle" | "swim" => Hit::Paddle,
                other => return Err(format!("foley hit \"{other}\": step, land, tap or paddle")),
            };
            let from = times[0];
            let hits: Vec<(f64, f32, Hit)> = times.iter().map(|t| (t - from, 1.0, kind)).collect();
            let size = f(op, "size").unwrap_or(1.0) as f32;
            let feet = self.feet_of(op, None, size);
            let (ground, why) = self.ground_at(op, micros(from) as m::Micros, None)?;
            let dur = times[times.len() - 1] - from + 0.8;
            let (l, r) = track_for(&hits, dur, ground, feet, 0.5, (from * 1000.0) as u64);
            let note = self.lay(&l, &r, "foley contacts", from, &lane, volume)?;
            return Ok(format!("{} steps on {ground} ({why}) at the contacts given, {}: {note}", hits.len(), if feet.paws { "on paws" } else { "on feet" }));
        }
        if let Some(kind) = op.get("kind").and_then(Value::as_str) {
            let at = f(op, "at").ok_or_else(|| format!("foley {kind} needs \"at\" (timeline seconds)"))?.max(0.0);
            let size = f(op, "size").unwrap_or(1.0) as f32;
            let (l, r) = action(kind, size, (at * 1000.0) as u64 ^ op.get("variant").and_then(Value::as_u64).unwrap_or(0))?;
            let dur = l.len() as f64 / RATE as f64;
            let note = self.lay(&l, &r, kind, at, &lane, volume)?;
            return Ok(format!("{kind} ({dur:.2}s) at {at:.2}s: {note}"));
        }
        let clip = op.get("clip").ok_or_else(|| format!("foley needs \"clip\" (a puppet that walks, runs, sneaks, hops, swims or pecks) or \"kind\" ({ACTIONS}) with \"at\""))?;
        let id = clip.as_u64().ok_or("foley's clip is a clip id (or \"$1\", \"@4.2\", a name)")? as u32;
        let mc = self.model_clip(id).ok_or("no such clip")?;
        let acts: Vec<Value> = mc.extra.get("acts").and_then(Value::as_array).cloned().unwrap_or_default();
        let ch = self.character_of(op, &mc);
        if acts.is_empty() {
            // A rigged 3D figure: its steps are where its feet come down.
            if let Some(steps) = self.rig_steps(id) {
                let (from, to) = (secs(mc.start as _), secs((mc.start + mc.duration) as _));
                let hits: Vec<(f64, f32, Hit)> = steps.iter().map(|t| (t - from, 1.0, Hit::Step)).collect();
                let size = f(op, "size").map(|s| s as f32).or_else(|| ch.as_ref().map(|c| c.size as f32)).unwrap_or(1.0);
                let feet = self.feet_of(op, ch.as_ref(), size);
                let w = self.doc.project.width.max(1) as f32;
                let pan = (0.5 + mc.transform.position.x as f32 / w * 0.8).clamp(0.1, 0.9);
                let old: Vec<u32> = self.doc.project.clips.iter().filter(|c| c.name.starts_with(&format!("foley {id} "))).map(|c| c.id).collect();
                if !old.is_empty() {
                    self.op(&json!({"op": "delete", "clips": old, "ripple": false}))?;
                }
                let (ground, why) = self.ground_at(op, micros((from + to) / 2.0) as m::Micros, Some(id))?;
                let (l, r) = track_for(&hits, to - from, ground, feet, pan, id as u64);
                let note = self.lay(&l, &r, &format!("foley {id} steps"), from, &lane, volume)?;
                let shown: Vec<String> = steps.iter().take(6).map(|t| format!("{t:.2}")).collect();
                return Ok(format!("{} steps on {ground} ({why}) where the feet come down, at {}{}: {note}", steps.len(), shown.join(", "), if steps.len() > 6 { "…" } else { "" }));
            }
            return Err(format!("clip {id} has not acted yet: a 3D figure needs `act` (a walk, run…) first; a puppet needs puppet {{clip, act: \"walk\"}} (acts are remembered for foley)"));
        }
        let size = f(op, "size").map(|s| s as f32).or_else(|| ch.as_ref().map(|c| c.size as f32)).unwrap_or(1.0);
        let feet = self.feet_of(op, ch.as_ref(), size);
        // Pan from where the character stands.
        let w = self.doc.project.width.max(1) as f32;
        let pan = (0.5 + mc.transform.position.x as f32 / w * 0.8).clamp(0.1, 0.9);
        let mut said = Vec::new();
        let start = mc.start;
        let windows: Vec<(Vec<String>, f64, f64, f64)> = acts
            .iter()
            .map(|a| (a["acts"].as_array().map(|l| l.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default(), a["from"].as_f64().unwrap_or(0.0), a["duration"].as_f64().unwrap_or(0.0), a["speed"].as_f64().unwrap_or(1.0)))
            .collect();
        // Foley laid before for this clip goes: laying again replaces it.
        let old: Vec<u32> = self.doc.project.clips.iter().filter(|c| c.name.starts_with(&format!("foley {id} "))).map(|c| c.id).collect();
        if !old.is_empty() {
            self.op(&json!({"op": "delete", "clips": old, "ripple": false}))?;
        }
        for (acts, from, dur, speed) in windows {
            let mut hits: Vec<(f64, f32, Hit)> = Vec::new();
            for a in &acts {
                if let Some(k) = hit_of(a) {
                    hits.extend(super::pins::contacts(a, dur, speed, id as u64).into_iter().map(|(t, w)| (t, w, k)));
                }
            }
            if hits.is_empty() {
                continue;
            }
            hits.sort_by(|a, b| a.0.total_cmp(&b.0));
            let at = secs(start as _) + from;
            let (ground, why) = self.ground_at(op, micros(at + dur / 2.0) as m::Micros, Some(id))?;
            let (l, r) = track_for(&hits, dur, ground, feet, pan, id as u64 ^ (from * 100.0) as u64);
            let note = self.lay(&l, &r, &format!("foley {id} {}", acts.join("+")), at, &lane, volume)?;
            said.push(format!("{} steps of {} on {ground} ({why}) at {}: {note}", hits.len(), acts.join(" + "), hits.iter().take(6).map(|(t, _, _)| format!("{:.2}", at + t)).collect::<Vec<_>>().join(", ") + if hits.len() > 6 { "…" } else { "" }));
        }
        if said.is_empty() {
            return Err(format!("clip {id}'s acts ({}) make no contact sounds: foley follows walk, run, sneak, hop, celebrate, swim and peck; for others place a kind ({ACTIONS}) at a moment", acts.iter().filter_map(|a| a["acts"].as_array()).flatten().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")));
        }
        Ok(format!("{}{}", said.join("; "), ch.map(|c| format!(" (weighted for {}, size {})", c.id, c.size)).unwrap_or_default()))
    }

    /// Where a rigged 3D figure's feet come down on its clip (timeline
    /// seconds), from its posed feet; none when it does not step.
    fn rig_steps(&self, id: u32) -> Option<Vec<f64>> {
        let c = self.clip(id).ok()?;
        let (s0, s1) = (secs(c.start as _), secs((c.start + c.dur) as _));
        let r = self.pose_report(id, &json!({"pose": {"span": [s0, s1], "step": 0.04}})).ok()?;
        let mut steps = Vec::new();
        for foot in r["feet"].as_array()? {
            for down in foot["down"].as_array().into_iter().flatten() {
                // A foot already down when the clip starts is standing, not stepping.
                if let Some(t) = down[0].as_f64().filter(|t| *t > s0 + 0.05) {
                    steps.push(t);
                }
            }
        }
        steps.sort_by(f64::total_cmp);
        (!steps.is_empty()).then_some(steps)
    }

    /// Writes a made sound and places it on a sound lane.
    fn lay(&mut self, l: &[f32], r: &[f32], name: &str, at: f64, lane: &Value, volume: Option<f64>) -> Result<String, String> {
        let samples: Vec<f32> = l.iter().zip(r.iter()).flat_map(|(a, b)| [*a, *b]).collect();
        let wav = studio_media::wav::Wav { sample_rate: RATE, channels: 2, samples };
        let dir = self.home.clone().map(|h| m::file::ProjectFolder::new(h).generated_dir(m::file::MadeKind::Sounds)).unwrap_or_else(|| studio_paths::cache().join("made-sfx"));
        let file: String = name.chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
        let path = crate::files::write_once(&dir, &file, "wav", &studio_media::wav::encode_pcm16(&wav)).map_err(|e| format!("could not write the sound: {e}"))?;
        let report = crate::services::project::import_media(&mut self.doc, &[path]);
        let media = *report.ids.first().ok_or("the made sound could not be imported")?;
        let before: std::collections::BTreeSet<u32> = self.doc.project.clips.iter().map(|c| c.id).collect();
        let mut place = json!({"op": "place", "media": media, "at": at, "lane": lane});
        if let Some(v) = volume {
            place["volume"] = json!(v);
        }
        let note = self.place(&place)?;
        // Named for the clip it belongs to, so laying again replaces it.
        if let Some(c) = self.doc.project.clips.iter().map(|c| c.id).find(|c| !before.contains(c)) {
            let name = name.to_owned();
            let ok = self.doc.edit("Foley", |p| {
                if let Some(x) = p.clip_mut(c) {
                    x.name = name.clone();
                }
            });
            self.check(ok, "")?;
        }
        Ok(note)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn onset(l: &[f32], from: usize) -> Option<usize> {
        l[from..].iter().position(|v| v.abs() > 0.05).map(|i| i + from)
    }

    #[test]
    fn steps_land_where_they_are_asked_and_grounds_differ() {
        let hits = [(0.5, 1.0, Hit::Step), (1.0, 0.8, Hit::Step), (1.5, 1.0, Hit::Land)];
        let mut spectra = Vec::new();
        for g in SURFACES.split(", ") {
            let (l, r) = track(&hits, 2.0, g, 1.0, 0.5, 7);
            assert_eq!(l.len(), r.len());
            let first = onset(&l, 0).unwrap_or(0) as f64 / RATE as f64;
            assert!((first - 0.5).abs() < 0.02, "{g} starts at {first}");
            assert!(l[..(0.45 * RATE as f64) as usize].iter().all(|v| v.abs() < 1e-6), "{g}: silence before the first step");
            assert!(onset(&l, (0.9 * RATE as f64) as usize).is_some_and(|i| (i as f64 / RATE as f64 - 1.0).abs() < 0.03), "{g}: second step");
            let peak = l.iter().chain(&r).fold(0f32, |m, v| m.max(v.abs()));
            assert!((0.45..=0.51).contains(&peak), "{g} {peak}");
            // Brightness: how much the signal changes sample to sample.
            let diff: f32 = l.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f32>() / l.iter().map(|v| v.abs()).sum::<f32>().max(1e-6);
            spectra.push((g, diff));
        }
        let b = |g: &str| spectra.iter().find(|(x, _)| *x == g).unwrap().1;
        // Loose, crunchy grounds are brighter than a soft sole's tap on
        // stone or the squelch of mud.
        assert!(b("gravel") > b("dirt") && b("leaves") > b("stone"), "{spectra:?}");
        assert!(b("grass") > b("mud"), "{spectra:?}");
        // Bigger feet ring longer.
        let (small, _) = track(&[(0.1, 1.0, Hit::Step)], 1.0, "dirt", 0.5, 0.5, 3);
        let (big, _) = track(&[(0.1, 1.0, Hit::Step)], 1.0, "dirt", 3.0, 0.5, 3);
        let tail = |v: &[f32]| v.iter().rposition(|x| x.abs() > 0.01).unwrap_or(0);
        assert!(tail(&big) > tail(&small));
    }

    /// Spectral centroid (Hz) of a sound.
    fn centroid(x: &[f32]) -> f32 {
        let n = 4096;
        let fft = studio_sound::fft::Fft::new(n);
        let mut win = vec![0.0; n];
        studio_sound::fft::hann(&mut win);
        let (mut num, mut den) = (0.0f64, 0.0f64);
        let mut pos = 0;
        while pos + n <= x.len() {
            let mut re: Vec<f32> = x[pos..pos + n].iter().zip(&win).map(|(a, w)| a * w).collect();
            let mut im = vec![0.0; n];
            fft.forward(&mut re, &mut im);
            for k in 0..n / 2 {
                let p = (re[k] * re[k] + im[k] * im[k]) as f64;
                num += p * k as f64 * RATE as f64 / n as f64;
                den += p;
            }
            pos += n / 2;
        }
        (num / den.max(1e-20)) as f32
    }

    /// Footfalls on grass laid at contacts are turf pressed by a foot, low
    /// for a heavy animal on paws, not bright claps; a hop lands low.
    #[test]
    fn steps_on_grass_are_feet_not_claps() {
        let hits: Vec<(f64, f32, Hit)> = (0..8).map(|i| (0.3 + i as f64 * 0.45, 1.0, Hit::Step)).collect();
        let (bear, _) = track_for(&hits, 4.0, "grass", Feet::of(3.0, true), 0.5, 3);
        let (person, _) = track_for(&hits, 4.0, "grass", Feet::of(1.0, false), 0.5, 3);
        let (cb, cp) = (centroid(&bear), centroid(&person));
        assert!(cb < 600.0, "a bear on grass: centroid {cb:.0} Hz");
        assert!(cp < 1500.0 && cp > cb, "a person on grass: {cp:.0} Hz (bear {cb:.0} Hz)");
        let lands: Vec<(f64, f32, Hit)> = (0..6).map(|i| (0.3 + i as f64 * 0.5, 0.9, Hit::Land)).collect();
        let (hop, _) = track_for(&lands, 3.5, "grass", Feet::of(0.4, true), 0.5, 4);
        assert!(centroid(&hop) < 800.0, "rabbit landings: {:.0} Hz", centroid(&hop));
    }

    /// Contacts given as times (from keys, a gait, another tool) are where
    /// the steps land.
    #[test]
    fn keyed_contacts_lay_steps_where_they_are_given() {
        let mut s = Session::new(crate::services::project::new_custom("Foley", 640, 360, 30.0));
        let r = s.edit(&[json!({"op": "foley", "contacts": [1.0, 1.6, 2.2, 2.8], "surface": "grass", "size": 3, "feet": "paws"})], "x", false).unwrap();
        let note = r["notes"][0].as_str().unwrap().to_owned();
        assert!(note.contains("4 steps on grass"), "{note}");
        let clip = s.doc.project.clips.iter().find(|c| c.kind == crate::view::ClipKind::Audio).unwrap().clone();
        assert!((clip.start as f64 / 1e6 - 1.0).abs() < 0.05, "starts at the first contact: {}", clip.start);
    }

    #[test]
    fn action_sounds_are_made_and_named_grounds_read() {
        for k in ACTIONS.split(", ") {
            let (l, r) = action(k, 1.0, 5).unwrap();
            assert!(l.len() > RATE as usize / 5, "{k}");
            assert!(l.iter().chain(&r).all(|v| v.abs() <= 1.0) && l.iter().any(|v| v.abs() > 0.3), "{k}");
        }
        assert!(action("kazoo", 1.0, 1).is_err());
        assert_eq!(surface_of("Forest floor"), None);
        assert_eq!(surface_of("forest"), Some("leaves"));
        assert_eq!(surface_of("beach"), Some("sand"));
    }

    #[test]
    fn a_walking_puppet_gets_its_footsteps_on_the_ground_of_its_scene() {
        use crate::services;
        let mut s = Session::new(services::project::new_custom("Foley", 640, 360, 30.0));
        // A fox shape: a dark body on white, legs below.
        let (w, h) = (160u32, 120u32);
        let mut rgba = vec![255u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let body = (30..130).contains(&x) && (40..80).contains(&y);
                let head = (120..150).contains(&x) && (25..60).contains(&y);
                let legs = ((40..50).contains(&x) || (60..70).contains(&x) || (100..110).contains(&x) || (115..125).contains(&x)) && (80..112).contains(&y);
                if body || head || legs {
                    let i = ((y * w + x) * 4) as usize;
                    rgba[i..i + 3].copy_from_slice(&[180, 70, 20]);
                }
            }
        }
        let dir = std::env::temp_dir().join(format!("studio-foley-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("fox.png");
        std::fs::write(&png, studio_media::png::encode(&studio_media::rgba::RgbaImage { width: w, height: h, pixels: rgba })).unwrap();
        s.call("import", &json!({"paths": [png.display().to_string()]})).unwrap();
        s.call("story", &json!({"do": "set", "places": [{"id": "lane", "surface": "gravel"}], "cast": [{"id": "fox", "look": "red fox", "size": 1.2}], "scenes": [{"id": "1", "place": "lane", "summary": "x", "shots": [{"id": "1A", "kind": "wide", "duration": 6}]}]})).unwrap();
        let r = s.edit(&[json!({"op": "place", "media": "fox", "at": 0, "duration": 6, "lane": "overlay"})], "x", false).unwrap();
        let id = r["changed"]["added"][0]["id"].as_u64().unwrap();
        s.edit(&[json!({"op": "set", "clip": id, "name": "fox"})], "x", false).unwrap();
        s.edit(&[json!({"op": "puppet", "clip": id, "rig": "quadruped", "act": "walk", "from": 1.0, "duration": 4.0})], "x", false).unwrap();
        let mc = s.model_clip(id as u32).unwrap();
        assert_eq!(mc.extra["acts"][0]["from"], 1.0);
        let r = s.edit(&[json!({"op": "foley", "clip": id})], "x", false).unwrap();
        let note = r["notes"][0].as_str().unwrap();
        assert!(note.contains("on gravel") && note.contains("size 1.2"), "{note}");
        let foley: Vec<_> = s.doc.project.clips.iter().filter(|c| c.name.starts_with(&format!("foley {id} "))).cloned().collect();
        assert_eq!(foley.len(), 1);
        assert!((foley[0].start as f64 / 1e6 - 1.0).abs() < 0.01);
        // Laid again on other ground: replaced, not doubled.
        s.edit(&[json!({"op": "foley", "clip": id, "surface": "snow"})], "x", false).unwrap();
        assert_eq!(s.doc.project.clips.iter().filter(|c| c.name.starts_with(&format!("foley {id} "))).count(), 1);
        // A one-off.
        let r = s.edit(&[json!({"op": "foley", "kind": "splash", "at": 5.0})], "x", false).unwrap();
        assert!(r["notes"][0].as_str().unwrap().starts_with("splash"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_3d_figure_walking_gets_steps_where_its_feet_come_down() {
        let mut s = Session::default();
        s.edit(&[json!({"op": "model", "shape": "character", "size": 400, "detail": 1, "name": "Walker", "duration": 5})], "x", false).unwrap();
        // Before it moves there is nothing to hear, and the refusal says what to do for a 3D figure.
        let e = s.edit(&[json!({"op": "foley", "clip": "Walker"})], "x", false).unwrap_err();
        assert!(e.contains("`act`"), "{e}");
        // On a street, stood on it by the height the read-back gives.
        s.edit(&[json!({"op": "build_place", "kind": "street", "name": "Street", "duration": 5})], "x", false).unwrap();
        let walker = s.doc.project.clips.iter().find(|c| c.name == "Walker").unwrap().id;
        let stand = s.footing(walker, 0).expect("the street is under the figure").stand_y;
        s.edit(&[json!({"op": "set", "clip": walker, "y": stand})], "x", false).unwrap();
        s.edit(&[json!({"op": "act", "clip": "Walker", "motion": "walk", "route": [[0, 0], [600, 0]]})], "x", false).unwrap();
        let r = s.edit(&[json!({"op": "foley", "clip": "Walker"})], "x", false).unwrap();
        let said = r["notes"][0].as_str().unwrap_or_default().to_owned();
        assert!(said.contains("where the feet come down"), "{r}");
        assert!(said.contains("on stone (it stands on"), "the street's paving, not grass: {said}");
        let n: usize = said.split(' ').next().and_then(|n| n.parse().ok()).unwrap_or(0);
        assert!((3..=12).contains(&n), "a 600 px walk takes a handful of steps: {said}");
        assert!(s.doc.project.clips.iter().any(|c| c.name.starts_with("foley ") && c.kind == crate::view::ClipKind::Audio), "the steps are on a sound lane");
    }

}
