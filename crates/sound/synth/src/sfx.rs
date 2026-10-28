// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sound effects and foley by physical models, chosen from a description.
//!
//! Words map to a model ("match strike", "distant traffic", "café murmur",
//! "footsteps on gravel", "pig oinking", "door knock"); each model is a small
//! simulation (modal bodies struck, stick-slip friction, Minnaert bubbles,
//! firing engines with Doppler, babbling voices through formants…) with
//! parameters read from the words around it (distance, size, speed, count,
//! material, era). Several sounds in one description are layered into one
//! passage; "then" puts them in order. Every call with a different `variant`
//! makes a different take. A description no model's words match is given
//! a model only when one of its words is a near spelling of that model's own
//! word (or names its family), and the reply says which; anything less would
//! be a different kind of sound, so it is refused with the nearest models.
//! An animal named with a walk or a hop is the walker (padded paws, its
//! weight, four feet), not a call.

use crate::animal;
use crate::dsp::*;
use crate::foley::{Shoe, Surface, Walk, landing, walk};
use crate::instrument::{boom, riser};
use crate::level;
use std::f32::consts::{PI, TAU};

/// What was made.
#[derive(Clone, Debug)]
pub struct Made {
    /// The sound, stereo.
    pub audio: Stereo,
    /// What models made it, for the reply.
    pub model: String,
    /// The moment that matters (a riser's peak, a match catching), seconds
    /// from the start, when the sound has one.
    pub anchor: Option<f32>,
}

/// The things that can be made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Kind {
    Impact,
    Knock,
    Scrape,
    Creak,
    Match,
    Lighter,
    Ignite,
    Fire,
    Pour,
    Drip,
    Splash,
    Bubbles,
    Rain,
    Wind,
    Engine,
    Passby,
    Traffic,
    Horn,
    Hooves,
    Murmur,
    Cafe,
    Applause,
    Clock,
    Click,
    Lock,
    Door,
    Typing,
    Machine,
    Servo,
    Camera,
    Phone,
    Heartbeat,
    Breath,
    Whoosh,
    Riser,
    Boom,
    Ui,
    Steps,
    Hops,
    Animal,
    Thunder,
    Explosion,
    Gunshot,
    Bell,
    Siren,
    Zap,
    Glass,
    Paper,
    Cloth,
    Sizzle,
    Bounce,
    Squeak,
    Pop,
}

/// A model on offer.
pub struct ModelInfo {
    /// Its name.
    pub name: &'static str,
    /// The words that ask for it.
    pub words: &'static [&'static str],
    /// A description that works, used by the tests and the guide.
    pub example: &'static str,
    /// What it is.
    pub summary: &'static str,
    /// The kind.
    pub kind: Kind,
    /// A continuous texture that fills the time, rather than an event.
    pub bed: bool,
}

macro_rules! m {
    ($n:expr, $w:expr, $ex:expr, $s:expr, $k:expr, $bed:expr) => {
        ModelInfo { name: $n, words: $w, example: $ex, summary: $s, kind: $k, bed: $bed }
    };
}

/// Every model.
pub const MODELS: &[ModelInfo] = &[
    m!("match", &["match", "matchstick", "matches", "strike a match", "lights a match"], "match strike", "a match scraped and catching, then burning", Kind::Match, false),
    m!("lighter", &["lighter", "zippo", "flint"], "lighter flick", "a lighter's wheel and flame", Kind::Lighter, false),
    m!("ignition", &["ignite", "ignites", "ignition", "whoomp", "torch", "flare", "igniting", "catches fire", "bursts into flame"], "torch ignites", "a gas flame catching", Kind::Ignite, false),
    m!("fire", &["fire", "crackle", "crackling", "campfire", "fireplace", "bonfire", "embers", "flames", "hearth", "burning"], "fire crackling", "a fire: pops, crackles and a low roar", Kind::Fire, true),
    m!("impact", &["impact", "thud", "slam", "bang", "smash", "crash", "punch", "clang", "clank", "thump", "whack", "slap", "drop", "dropped", "hit", "thunk", "bonk", "crunch"], "heavy wooden thud", "something struck: wood, metal, glass, stone, plastic or a body", Kind::Impact, false),
    m!("knock", &["knock", "knocks", "knocking", "rap", "tap", "tapping"], "three knocks on a door", "knuckles on wood", Kind::Knock, false),
    m!("scrape", &["scrape", "scraping", "drag", "dragging", "scratch", "scratching", "grind", "grinding", "screech", "squeak", "squeaking", "rasp", "friction", "scuff"], "chair scraping on a floor", "stick-slip friction", Kind::Scrape, false),
    m!("creak", &["creak", "creaks", "creaking", "creaky", "groan", "hinge"], "door creaking open", "a slow hinge or timber", Kind::Creak, false),
    m!("pour", &["pour", "pouring", "poured", "stream", "trickle", "flowing", "running water", "tap water", "faucet", "gurgle", "gushing", "water"], "water pouring", "liquid running, made of bubbles", Kind::Pour, true),
    m!("drip", &["drip", "drips", "dripping", "droplet", "drop of water", "leak", "leaking", "plink", "cave drip", "tap dripping", "sink drip"], "water dripping", "single drops: a basin, a cave, a tap, a bucket", Kind::Drip, false),
    m!("splash", &["splash", "splashes", "dive", "plunge", "plop", "dunk", "cannonball", "belly flop", "kerplunk", "pebble in water", "stone in water", "paddle", "oar", "jumps in", "falls in"], "a big splash", "water struck: a pebble's plop to a cannonball", Kind::Splash, false),
    m!("bubbles", &["bubble", "bubbles", "bubbling", "boil", "boiling", "simmer", "simmering", "cauldron", "fizz", "fizzing"], "pot boiling", "bubbles rising and bursting", Kind::Bubbles, true),
    m!("rain", &["rain", "raining", "rainfall", "drizzle", "downpour", "shower", "raindrops"], "rain on a window", "rain: a hiss of drops with ticks", Kind::Rain, true),
    m!("wind", &["wind", "windy", "gust", "gusts", "breeze", "howl", "howling", "gale", "blizzard", "draught"], "wind howling", "gusting air through a resonant space", Kind::Wind, true),
    m!("engine", &["engine", "idle", "idling", "motor", "revving", "rev", "revs", "motorcycle", "tractor", "truck", "diesel", "car starting", "ignition key", "starts up"], "car engine idling", "firing cylinders through body and exhaust", Kind::Engine, true),
    m!("passby", &["pass by", "passes", "passing", "drive by", "drives past", "drives by", "flyby", "fly by", "goes past", "speeds past", "car passes"], "a car passing by", "an engine with Doppler and distance", Kind::Passby, false),
    m!("traffic", &["traffic", "cars", "street", "road noise", "city", "motorway", "highway", "rush hour", "vehicles", "streets"], "distant period traffic", "many vehicles at a distance; period adds horns and hooves", Kind::Traffic, true),
    m!("horn", &["horn", "honk", "honking", "hooter", "ahooga", "klaxon"], "car horn honking", "a bulb or electric horn", Kind::Horn, false),
    m!("hooves", &["hooves", "hoofbeats", "clip-clop", "clip clop", "galloping", "gallop", "trotting", "trot", "horse and cart", "carriage"], "horse hooves on cobblestones", "hooves in a walk, trot or gallop", Kind::Hooves, false),
    m!("murmur", &["murmur", "murmuring", "crowd", "babble", "chatter", "chatting", "voices", "people talking", "conversation", "conversations", "hubbub", "restaurant", "party", "bar", "pub"], "crowd murmur", "many voices heard as a babble", Kind::Murmur, true),
    m!("cafe", &["cafe", "café", "coffee shop", "tea room", "diner", "teahouse", "bistro"], "café murmur", "a babble with cups, cutlery and chairs", Kind::Cafe, true),
    m!("applause", &["applause", "clapping", "claps", "ovation", "audience clapping"], "applause", "hands clapping in a crowd", Kind::Applause, true),
    m!("clock", &["clock", "ticking", "tick tock", "tick-tock", "grandfather clock", "metronome", "watch ticking"], "clock ticking", "a mechanical tick and tock", Kind::Clock, true),
    m!("click", &["click", "clicks", "switch", "button", "latch", "snap", "toggle", "light switch", "lever"], "light switch click", "a small mechanical click", Kind::Click, false),
    m!("lock", &["lock", "unlock", "unlocking", "locking", "key in the lock", "deadbolt", "padlock", "bolt"], "a key turning in a lock", "a lock's two-stage clunk", Kind::Lock, false),
    m!("door", &["door", "doors", "door closing", "door slam", "door slams", "gate", "doorbell"], "a door slams", "a door opening, closing or slamming", Kind::Door, false),
    m!("typing", &["typing", "typewriter", "keyboard", "keystrokes", "types"], "typewriter typing", "keys struck at human speed", Kind::Typing, false),
    m!("machine", &["machine", "machinery", "hum", "humming", "factory", "generator", "compressor", "refrigerator", "fan", "air conditioning", "buzz", "buzzing", "drone", "server room", "electrical"], "machine humming", "a mains hum with harmonics and a motor", Kind::Machine, true),
    m!("servo", &["servo", "whirr", "whirring", "robot", "actuator", "mechanical arm", "winding", "winds up", "elevator"], "robot servo whirring", "a motor changing speed", Kind::Servo, false),
    m!("camera", &["camera", "shutter", "photograph", "flash photo", "snapshot"], "camera shutter", "a mechanical shutter", Kind::Camera, false),
    m!("phone", &["phone", "ringing", "telephone", "ringtone", "phone ringing", "rings"], "old phone ringing", "a bell telephone or a modern ring", Kind::Phone, false),
    m!("heartbeat", &["heartbeat", "heart", "pulse", "heart beating"], "heartbeat", "lub-dub", Kind::Heartbeat, true),
    m!("breath", &["breath", "breathing", "breathes", "gasp", "panting", "sigh", "exhale", "inhale", "snoring", "snore"], "heavy breathing", "air in and out", Kind::Breath, true),
    m!("whoosh", &["whoosh", "woosh", "swoosh", "swish", "swing", "sweep", "fast movement", "transition", "swipe", "whip", "zoom past", "flies past", "sword swing", "punch whoosh"], "a whoosh", "air moved past: a swept band of noise", Kind::Whoosh, false),
    m!("riser", &["riser", "rise", "buildup", "build-up", "build up", "tension riser", "uplifter", "swell", "sweep up"], "synth riser with a tail", "a rising sweep to a peak, then a tail", Kind::Riser, false),
    m!("boom", &["boom", "sub drop", "impact boom", "cinematic hit", "stinger", "drop"], "cinematic boom", "a low drop with a long tail", Kind::Boom, false),
    m!("ui", &["beep", "blip", "ding", "chime", "notification", "ui", "pop", "alert", "ping", "bleep", "error sound", "success sound", "confirm", "bloop", "boop", "ui click", "menu click", "interface", "app sound", "mouse click", "tap sound", "digital click", "select sound", "hover sound"], "notification ding", "interface sounds: clicks, taps, blips, pops, chimes, alerts", Kind::Ui, false),
    m!("footsteps", &["footsteps", "footstep", "steps", "walking", "walks", "walk", "running", "runs", "tiptoe", "tiptoeing", "marching", "stomping", "treading", "pacing", "strolling", "sneaking", "footfalls", "jogging", "scurrying", "scurry", "scampering", "scamper", "padding", "prowling", "prowls", "lumbering", "lumbers", "pawsteps"], "footsteps on gravel", "foot falls on a surface, each different", Kind::Steps, false),
    m!("hops", &["hop", "hops", "hopping", "hopped", "bounding", "bounds", "leaping", "leaps", "jumping", "jumps"], "a rabbit hopping on grass", "an animal hopping: fore feet then hind feet land, soft and low", Kind::Hops, false),
    m!("thunder", &["thunder", "thunderclap", "lightning", "rumble", "rumbling"], "distant thunder", "a crack and a rolling rumble", Kind::Thunder, false),
    m!("explosion", &["explosion", "explode", "explodes", "blast", "detonation", "bomb", "grenade", "dynamite"], "an explosion", "a blast, a boom and debris", Kind::Explosion, false),
    m!("gunshot", &["gunshot", "gunshots", "gun", "rifle", "pistol", "shot", "shotgun", "gunfire", "shoots"], "a gunshot", "a crack with a room or outdoor tail", Kind::Gunshot, false),
    m!("bell", &["bell", "bells", "church bell", "gong", "tolling", "toll", "handbell", "cowbell", "temple bell"], "church bell tolling", "a struck bell with inharmonic partials", Kind::Bell, false),
    m!("siren", &["siren", "sirens", "alarm", "police car", "ambulance", "fire engine", "alert siren", "klaxon alarm"], "siren wailing", "a rising and falling siren", Kind::Siren, true),
    m!("zap", &["laser", "zap", "zaps", "power up", "power-up", "powerup", "power down", "power-down", "sci-fi", "scifi", "energy", "plasma", "teleport", "beam", "spaceship"], "laser zap", "synthetic sweeps and pulses", Kind::Zap, false),
    m!("glass", &["glass", "glasses", "clink", "toast", "shatter", "shattering", "breaking glass", "bottle", "window breaks", "cheers", "wine glass"], "glasses clinking", "glass struck, or broken", Kind::Glass, false),
    m!("paper", &["paper", "papers", "rustle", "rustling", "crumple", "crumpling", "page", "pages turning", "leaves rustling", "newspaper", "foil", "wrapper"], "paper rustling", "crumpled sheets", Kind::Paper, false),
    m!("cloth", &["cloth", "fabric", "clothes", "cloak", "coat", "fabric rustle", "cloth rustle", "sleeve", "jacket", "dress rustling", "bedsheets"], "cloth rustling", "fabric moving", Kind::Cloth, false),
    m!("bounce", &["bounce", "bouncing", "bouncy", "ball", "basketball", "tennis ball", "ping pong", "ping-pong", "table tennis", "dribble", "dribbling", "rubber ball", "super ball", "boing", "rebound"], "rubber ball bouncing", "a ball bouncing to rest: rubber, basketball, tennis or ping-pong (dribbling keeps the height)", Kind::Bounce, false),
    m!("squeak", &["squeaky toy", "squeak toy", "rubber duck", "rubber ducky", "dog toy", "squeaker", "chew toy", "squeeze toy", "squeaky", "toy squeak"], "rubber duck squeak", "a squeezed rubber toy's squeal, out and back in", Kind::Squeak, false),
    m!("pop", &["balloon", "cork", "champagne", "bubble wrap", "bubblewrap", "popcorn", "pop gun", "popgun", "popping", "burst", "bursts", "uncork", "bottle opening"], "balloon pop", "pops: a balloon bursting, a cork drawn, bubble wrap, popcorn", Kind::Pop, false),
    m!("sizzle", &["sizzle", "sizzling", "frying", "fry", "bacon", "pan", "sear", "searing", "grill", "barbecue"], "bacon sizzling", "hot fat popping", Kind::Sizzle, true),
    m!("animal", &[], "a dog barking", "an animal voice: see the animals list", Kind::Animal, false),
];

/// The list for replies and the guide.
pub fn catalogue() -> String {
    let mut s: Vec<String> = MODELS.iter().filter(|m| m.kind != Kind::Animal).map(|m| format!("{} ({})", m.name, m.words.iter().take(3).copied().collect::<Vec<_>>().join(", "))).collect();
    s.push(format!("animals: {}", animal::catalogue()));
    s.join("; ")
}

// ----- reading the words -------------------------------------------------------

#[derive(Clone, Debug)]
struct Params {
    dist: f32,
    size: f32,
    speed: f32,
    count: Option<usize>,
    surface: Option<Surface>,
    shoe: Option<Shoe>,
    material: Option<usize>,
    period: bool,
    intensity: f32,
    text: String,
    /// The animal walking or hopping, when one is: how heavy (0..1) and
    /// whether on four feet.
    walker: Option<(f32, bool)>,
}

/// The materials of struck things: (name, words, base Hz range, modes as
/// (ratio, gain, decay seconds)).
type Mode3 = (f32, f32, f32);
struct Material {
    words: &'static [&'static str],
    hz: (f32, f32),
    modes: &'static [Mode3],
    click: f32,
}

const MATERIALS: &[Material] = &[
    Material { words: &["wood", "wooden", "oak", "plank", "table", "desk", "chest", "crate", "door", "timber", "log"], hz: (150.0, 420.0), modes: &[(1.0, 1.0, 0.09), (2.4, 0.6, 0.06), (4.1, 0.35, 0.04), (6.3, 0.2, 0.03)], click: 1800.0 },
    Material { words: &["metal", "steel", "iron", "tin", "pipe", "bucket", "pan", "pot", "anvil", "clang", "clank", "can", "bin"], hz: (380.0, 1500.0), modes: &[(1.0, 1.0, 0.7), (2.76, 0.8, 0.45), (5.4, 0.6, 0.3), (8.9, 0.4, 0.18), (13.0, 0.25, 0.1)], click: 4000.0 },
    Material { words: &["glass", "bottle", "window", "jar", "vase", "mirror"], hz: (1100.0, 2800.0), modes: &[(1.0, 1.0, 0.45), (2.32, 0.7, 0.3), (4.25, 0.5, 0.18), (6.63, 0.3, 0.1)], click: 6000.0 },
    Material { words: &["stone", "rock", "brick", "concrete", "marble", "boulder", "rocks", "wall"], hz: (260.0, 800.0), modes: &[(1.0, 1.0, 0.05), (1.8, 0.5, 0.03), (3.1, 0.3, 0.02)], click: 2500.0 },
    Material { words: &["plastic", "toy", "bottle cap", "tupperware", "bucket plastic"], hz: (700.0, 1500.0), modes: &[(1.0, 1.0, 0.04), (2.2, 0.6, 0.025), (3.7, 0.3, 0.015)], click: 3000.0 },
    Material { words: &["ceramic", "cup", "plate", "porcelain", "saucer", "mug", "dish", "bowl"], hz: (1500.0, 3200.0), modes: &[(1.0, 1.0, 0.2), (2.6, 0.6, 0.1), (4.8, 0.35, 0.06)], click: 5000.0 },
    Material { words: &["body", "punch", "fist", "flesh", "belly", "thud", "bag", "sack", "cloth", "leather", "sandbag", "meat", "cushion", "earth"], hz: (55.0, 110.0), modes: &[(1.0, 1.0, 0.12), (2.1, 0.3, 0.06)], click: 900.0 },
];

fn has_word(text: &str, w: &str) -> bool {
    let t = format!(" {text} ");
    t.contains(&format!(" {w} ")) || t.contains(&format!(" {w}s ")) || t.contains(&format!(" {w}es ")) || t.contains(&format!(" {w}ing "))
}

fn number_in(text: &str) -> Option<usize> {
    let words = ["one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve", "a dozen", "a couple", "a few", "several"];
    let vals = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 12, 2, 3, 4];
    for tok in text.split_whitespace() {
        if let Ok(n) = tok.trim_matches(|c: char| !c.is_ascii_digit()).parse::<usize>() {
            if (1..=200).contains(&n) && tok.chars().any(|c| c.is_ascii_digit()) {
                return Some(n);
            }
        }
    }
    let t = format!(" {text} ");
    words.iter().zip(vals).find(|(w, _)| t.contains(&format!(" {w} "))).map(|(_, v)| v)
}

impl Params {
    fn read(window: &str, whole: &str) -> Params {
        let w = window.to_lowercase();
        let all = whole.to_lowercase();
        let any = |ws: &[&str]| ws.iter().any(|x| has_word(&w, x) || w.contains(&format!("{x} ")) && x.contains(' '));
        let dist = if any(&["distant", "far", "faraway", "far away", "faint", "background", "muffled", "remote", "far off", "far-off", "away"]) {
            0.85
        } else if any(&["close", "near", "nearby", "closeup", "close-up", "right next", "foreground", "loud"]) {
            0.0
        } else {
            -1.0
        };
        let size = if any(&["big", "large", "huge", "heavy", "massive", "giant", "enormous", "thick", "deep"]) {
            0.9
        } else if any(&["small", "tiny", "little", "light", "delicate", "thin", "mini", "baby", "puppy", "kitten", "chick"]) {
            0.15
        } else {
            0.5
        };
        let speed = if any(&["fast", "quick", "quickly", "rapid", "running", "sprint", "hurried", "frantic", "brisk", "jogging", "racing", "galloping", "gallop", "urgent", "panicked"]) {
            0.85
        } else if any(&["slow", "slowly", "lazy", "creeping", "sneaking", "careful", "tiptoe", "tiptoeing", "weary", "plodding", "trudging", "dragging"]) {
            0.2
        } else {
            0.5
        };
        let intensity = if any(&["hard", "violent", "angry", "furious", "loud", "fierce", "aggressive", "urgent", "heavy"]) {
            0.95
        } else if any(&["gentle", "soft", "quiet", "light", "faint", "delicate", "polite", "timid"]) {
            0.35
        } else {
            0.7
        };
        let material = MATERIALS.iter().position(|m| m.words.iter().any(|x| has_word(&w, x))).or_else(|| MATERIALS.iter().position(|m| m.words.iter().any(|x| has_word(&all, x))));
        Params {
            dist,
            size,
            speed,
            count: number_in(&w),
            surface: Surface::parse(&w).or_else(|| Surface::parse(&all)),
            shoe: Shoe::parse(&w).or_else(|| Shoe::parse(&all)),
            material,
            period: ["period", "vintage", "old", "1920s", "1930s", "1940s", "antique", "old-fashioned", "historic", "victorian", "nineteen-thirties", "thirties", "twenties", "forties", "retro", "classic", "old-time"].iter().any(|x| has_word(&all, x)),
            intensity,
            text: w,
            walker: None,
        }
    }
    /// Distance with a default for the kind of sound.
    fn far(&self, default: f32) -> f32 {
        if self.dist < 0.0 { default } else { self.dist }
    }
}

// ----- small tools -------------------------------------------------------------

/// A decaying noise click through a band-pass.
fn click(rng: &mut Rng, hz: f32, q: f32, tau: f32) -> Vec<f32> {
    let n = frames(tau * 7.0).max(8);
    let mut f = Svf::new();
    let c = SvfCoef::new(hz, q);
    (0..n).map(|i| f.tick(&c, rng.signed()).bp * (-(i as f32 / SR) / tau).exp()).collect()
}

/// A bank of struck modes at `hz`, `tau_scale` stretching every decay.
fn struck(hz: f32, modes: &[Mode3], tau_scale: f32, len: f32) -> Vec<f32> {
    let n = frames(len);
    let mut osc: Vec<(Rot, f32, f32)> = modes.iter().filter(|(r, _, _)| hz * r < SR * 0.45).map(|(r, g, t)| (Rot::new(hz * r, 0.0), *g, t * tau_scale)).collect();
    (0..n).map(|i| {
        let t = i as f32 / SR;
        osc.iter_mut().map(|(r, g, tau)| r.tick() * *g * (-t / *tau).exp()).sum::<f32>() * smooth(t, 0.0005)
    }).collect()
}

/// Places `src` at `t` seconds with gain.
fn put(dst: &mut [f32], src: &[f32], t: f32, g: f32) {
    mix_into(dst, src, frames(t), g);
}

/// Dulls a signal for distance: a low-pass and a loss of level.
fn distance(x: &mut [f32], dist: f32) {
    if dist <= 0.02 {
        return;
    }
    let hz = 16_000.0 * (1.0 - 0.93 * dist).powi(2);
    let y = low_pass(x, hz.max(500.0), 0.6);
    let g = 1.0 - 0.6 * dist;
    for (a, b) in x.iter_mut().zip(y) {
        *a = b * g;
    }
}

/// A slow random wander in 0..1, a new target every `period` seconds.
struct Wander {
    cur: f32,
    target: f32,
    left: usize,
    period: usize,
    rng: Rng,
}

impl Wander {
    fn new(seed: u64, period: f32) -> Self {
        let mut rng = Rng::new(seed);
        let t = rng.unit();
        Self { cur: t, target: t, left: 0, period: frames(period).max(1), rng }
    }
    fn tick(&mut self) -> f32 {
        if self.left == 0 {
            self.target = self.rng.unit();
            self.left = (self.period as f32 * self.rng.range(0.6, 1.4)) as usize;
        }
        self.left -= 1;
        self.cur += (self.target - self.cur) * (3.0 / self.period as f32).min(0.01);
        self.cur
    }
}

// ----- the models --------------------------------------------------------------

fn impact(rng: &mut Rng, p: &Params, mat: usize, force: f32) -> Vec<f32> {
    let m = &MATERIALS[mat];
    let size = p.size;
    let hz = m.hz.0 * (m.hz.1 / m.hz.0).powf(1.0 - size) * rng.range(0.93, 1.07);
    let stretch = 0.6 + 1.6 * size;
    let len = 0.15 + m.modes.iter().map(|x| x.2).fold(0.0, f32::max) * stretch * 5.0;
    let len = len.min(3.5);
    let mut out = struck(hz, m.modes, stretch, len);
    let ck = m.click * rng.range(0.8, 1.2);
    let c = click(rng, ck, 1.2, 0.003);
    mix_into(&mut out, &c, 0, 0.7 * force);
    // The weight behind a big strike.
    if size > 0.4 {
        let thump = struck(55.0 + 40.0 * (1.0 - size), &[(1.0, 1.0, 0.12 + 0.2 * size)], 1.0, 0.8);
        mix_into(&mut out, &thump, 0, 0.5 * size * force);
    }
    out.iter_mut().for_each(|v| *v *= force);
    out
}

fn mk_impact(rng: &mut Rng, p: &Params, dur: f32) -> Vec<f32> {
    let mat = p.material.unwrap_or(0);
    let force = 0.6 + 0.4 * p.intensity;
    let hit = impact(rng, p, mat, force);
    let mut out = vec![0.0; frames(dur.max(hit.len() as f32 / SR))];
    put(&mut out, &hit, 0.0, 1.0);
    // Glass and stone that "smash" or "crash" fall into pieces.
    if p.text.contains("smash") || p.text.contains("crash") || p.text.contains("shatter") {
        for _ in 0..(10 + (p.size * 30.0) as usize) {
            let pp = Params { size: 0.1, ..p.clone() };
            let f = rng.range(0.1, 0.5);
            let bit = impact(rng, &pp, if mat == 2 { 2 } else { 3 }, f);
            put(&mut out, &bit, rng.range(0.03, 0.5 + p.size), rng.range(0.2, 0.7));
        }
    }
    out
}

fn mk_knock(rng: &mut Rng, p: &Params, dur: f32) -> Vec<f32> {
    let n = p.count.unwrap_or(3).clamp(1, 20);
    let pp = Params { size: 0.35 + 0.2 * p.size, ..p.clone() };
    let gap = 0.19 + 0.12 * (1.0 - p.speed);
    let mut out = vec![0.0; frames(dur.max(gap * n as f32 + 0.5))];
    let mut t = 0.03;
    let base = rng.range(0.85, 1.0);
    for i in 0..n {
        let f = base * (1.0 - 0.06 * i as f32) * rng.range(0.85, 1.0);
        let hit = impact(rng, &pp, p.material.filter(|m| *m != 6).unwrap_or(0), f);
        put(&mut out, &hit, t, 1.0);
        t += gap * rng.range(0.88, 1.15) * if i == 1 { 1.3 } else { 1.0 };
    }
    out
}

/// A stick-slip scrape or squeak: a velocity that rises and falls drives
/// slip pulses through the resonances of the thing being rubbed.
fn mk_scrape(rng: &mut Rng, p: &Params, dur: f32) -> Vec<f32> {
    let n = frames(dur);
    let squeak = p.text.contains("squeak") || p.text.contains("screech");
    let m = &MATERIALS[p.material.unwrap_or(0)];
    let base = m.hz.0 * (m.hz.1 / m.hz.0).powf(0.5);
    let mut out = vec![0.0; n];
    let (mut f1, mut f2, mut fnz) = (Svf::new(), Svf::new(), Svf::new());
    let mut ph = 0.0f32;
    let mut wander = Wander::new(rng.u64(), dur / 4.0);
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        let p01 = t / dur;
        let v = (PI * p01).sin().powf(0.7) * (0.55 + 0.45 * wander.tick());
        if squeak {
            let hz = (1500.0 + 1500.0 * v + 200.0 * (TAU * 9.0 * t).sin()) * (1.0 + 0.2 * p.size);
            ph += TAU * hz / SR;
            let stick = if ((t * (35.0 + 60.0 * v)) % 1.0) < 0.5 { 1.0 } else { 0.3 };
            *o = (ph.sin() + 0.4 * (2.0 * ph).sin()) * stick * v * 0.6 + fnz.at(rng.signed(), 3500.0, 1.2).bp * v * 0.1;
        } else {
            // Slip events at a rate that follows the speed.
            let rate = 40.0 + 500.0 * v;
            let mut pulse = 0.0;
            if rng.unit() < rate / SR {
                pulse = rng.unit().powf(2.0) * 2.0;
            }
            let grit = rng.signed() * 0.25 * v;
            let x = pulse + grit;
            *o = (f1.at(x, base * (1.0 + 0.5 * v), 3.0).bp * 1.4 + f2.at(x, base * 2.7, 4.0).bp * 0.7 + fnz.at(x, 4500.0, 0.8).bp * 0.4) * v;
        }
    }
    let e = frames(0.02);
    for i in 0..e.min(n / 2) {
        let g = i as f32 / e as f32;
        out[i] *= g;
        out[n - 1 - i] *= g;
    }
    out
}

/// A hinge: slow slip pulses through low resonances, rising in pitch as it
/// speeds up.
fn mk_creak(rng: &mut Rng, p: &Params, dur: f32) -> Vec<f32> {
    let n = frames(dur);
    let mut out = vec![0.0; n];
    let (mut a, mut b) = (Svf::new(), Svf::new());
    let mut phase = 0.0f32;
    let base = rng.range(220.0, 520.0) * (1.2 - 0.5 * p.size);
    let mut wander = Wander::new(rng.u64(), dur / 3.0);
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        let v = (PI * t / dur).sin().powf(0.6) * (0.5 + 0.5 * wander.tick());
        phase += (14.0 + 90.0 * v) / SR;
        if phase >= 1.0 {
            phase -= 1.0;
        }
        // A sawtooth of slips with sharp release.
        let saw = (1.0 - phase).powf(3.0) * 2.0 - 0.5;
        let x = saw + rng.signed() * 0.08;
        *o = (a.at(x, base * (0.8 + 0.8 * v), 7.0).bp * 1.2 + b.at(x, base * 2.3 * (0.8 + 0.8 * v), 9.0).bp * 0.6) * v;
    }
    out
}

/// A match scraped on a strip, catching with a hiss and a pop, then burning.
fn mk_match(rng: &mut Rng, p: &Params, dur: f32) -> (Vec<f32>, f32) {
    let n = frames(dur.max(1.0));
    let mut out = vec![0.0; n];
    let strike = 0.10 + 0.07 * (1.0 - p.speed);
    // The scratch: rapid slips rising in brightness.
    let sn = frames(strike);
    let mut f = Svf::new();
    let mut f2 = Svf::new();
    for i in 0..sn {
        let t = i as f32 / sn as f32;
        let slip = if rng.unit() < 0.04 + 0.05 * t { rng.unit() * 2.0 } else { 0.0 };
        let x = rng.signed() * 0.5 + slip;
        let hz = 2500.0 + 3500.0 * t;
        out[i] += (f.at(x, hz, 0.8).bp * 1.3 + f2.at(x, 7000.0, 1.0).hp * 0.2) * (0.3 + 0.7 * t);
    }
    // The flame catches: a sharp hiss with a soft pop.
    let at = strike;
    let hiss = {
        let mut a = Svf::new();
        let len = frames(0.45);
        (0..len).map(|i| {
            let t = i as f32 / SR;
            let hz = 1500.0 + 3500.0 * (1.0 - (-t / 0.08).exp());
            a.at(rng.signed(), hz, 0.5).bp * smooth(t, 0.006) * (-t / 0.14).exp() * 1.6
        }).collect::<Vec<f32>>()
    };
    put(&mut out, &hiss, at, 1.0);
    let pop = struck(150.0 * rng.range(0.9, 1.1), &[(1.0, 1.0, 0.025)], 1.0, 0.1);
    put(&mut out, &pop, at, 0.55);
    // A small flame: a warm flutter that fades as the match burns down.
    let mut lp = Lp1::default();
    let mut w = Wander::new(rng.u64(), 0.12);
    let mut flick = Svf::new();
    for i in frames(at + 0.05)..n {
        let t = (i as f32 / SR) - at;
        let burn = smooth(t, 0.2) * (1.0 - smooth(t - (dur - at - 0.1).max(0.3), 0.6).max(0.0).min(1.0));
        let amp = 0.07 * burn * (0.6 + 0.8 * w.tick());
        let x = lp.tick(rng.signed(), 700.0) * 0.9 + flick.at(rng.signed(), 2400.0, 0.7).bp * 0.12;
        out[i] += x * amp;
        if rng.unit() < 7.0 / SR {
            let _r1 = rng.range(2500.0, 5000.0);
            let tick = click(rng, _r1, 1.2, 0.002);
            put(&mut out, &tick, i as f32 / SR, rng.range(0.08, 0.3) * burn);
        }
    }
    (out, at)
}

fn mk_lighter(rng: &mut Rng, dur: f32) -> (Vec<f32>, f32) {
    let n = frames(dur.max(1.2));
    let mut out = vec![0.0; n];
    // The wheel.
    let w = click(rng, 4200.0, 1.0, 0.012);
    put(&mut out, &w, 0.0, 0.8);
    let r = struck(2300.0, &[(1.0, 1.0, 0.03), (2.4, 0.5, 0.02)], 1.0, 0.1);
    put(&mut out, &r, 0.005, 0.25);
    // The spark and the flame: a small "fwump" and a steady gas hiss.
    let mut f = Svf::new();
    let mut f2 = Svf::new();
    let at = 0.07;
    let mut lp = Lp1::default();
    for i in frames(at)..n {
        let t = i as f32 / SR - at;
        let whump = (-t / 0.05).exp() * smooth(t, 0.004);
        let burn = smooth(t, 0.05) * (1.0 - smooth(t - (dur - 0.5).max(0.4), 0.4).clamp(0.0, 1.0));
        let x = rng.signed();
        out[i] += f.at(x, 900.0, 0.7).bp * whump * 0.9 + f2.at(x, 3200.0, 0.8).hp * 0.04 * burn + lp.tick(x, 500.0) * 0.07 * burn;
    }
    (out, at)
}

fn mk_ignite(rng: &mut Rng, p: &Params, dur: f32) -> (Vec<f32>, f32) {
    let n = frames(dur.max(1.5));
    let mut out = vec![0.0; n];
    let big = 0.4 + 0.6 * p.size;
    let at = 0.05;
    let (mut a, mut b) = (Svf::new(), Svf::new());
    let mut lp = Lp1::default();
    let mut w = Wander::new(rng.u64(), 0.15);
    for i in 0..n {
        let t = i as f32 / SR;
        let tt = (t - at).max(0.0);
        let whoomp = smooth(tt, 0.02) * (-tt / (0.25 * big)).exp();
        let burn = smooth(tt - 0.05, 0.3) * (1.0 - smooth(t - (dur - 0.6).max(0.6), 0.5).clamp(0.0, 1.0));
        let x = rng.signed();
        out[i] = a.at(x, 400.0 + 1800.0 * (-tt / 0.1).exp(), 0.6).bp * whoomp * 1.8 + b.at(x, 2600.0, 0.6).bp * whoomp * 0.4 + lp.tick(x, 380.0) * (0.12 + 0.05 * w.tick()) * burn;
    }
    let thump = struck(70.0, &[(1.0, 1.0, 0.1 * big + 0.05)], 1.0, 0.5);
    put(&mut out, &thump, at, 0.9 * big);
    (out, at)
}

fn mk_fire(rng: &mut Rng, p: &Params, dur: f32, stereo: &mut Stereo) {
    let n = stereo.len();
    let rate = 8.0 + 18.0 * p.intensity;
    let mut roar = Lp1::default();
    let mut roar2 = Lp1::default();
    let mut w = Wander::new(rng.u64(), 1.2);
    let mut mono = vec![0.0; n];
    for o in mono.iter_mut() {
        let k = w.tick();
        *o = (roar.tick(rng.signed(), 260.0) * 0.5 + roar2.tick(rng.signed(), 90.0) * 0.5) * (0.12 + 0.12 * k);
    }
    stereo.add_mono(&mono, 0, 1.0, 0.0);
    let mut t = 0.0f32;
    while t < dur {
        t += -(rng.unit().max(1e-3)).ln() / (rate * (0.4 + 1.2 * w.tick()));
        let amp = rng.unit().powi(3) * 0.9 + 0.05;
        let hz = rng.range(900.0, 6500.0);
        let _r2 = rng.range(0.8, 2.0);
        let _r3 = rng.range(0.002, 0.012);
        let c = click(rng, hz, _r2, _r3);
        stereo.add_mono(&c, frames(t), amp, rng.range(-0.8, 0.8));
        // The odd log shifting: a thump with a shower of ticks.
        if rng.chance(0.03) {
            let th = struck(rng.range(80.0, 160.0), &[(1.0, 1.0, 0.05)], 1.0, 0.25);
            stereo.add_mono(&th, frames(t), 0.5, rng.range(-0.5, 0.5));
            for _ in 0..6 {
                let _r4 = rng.range(1500.0, 5000.0);
                let c = click(rng, _r4, 1.0, 0.003);
                stereo.add_mono(&c, frames(t + rng.range(0.0, 0.15)), rng.range(0.1, 0.4), rng.range(-0.8, 0.8));
            }
        }
    }
}

/// A Minnaert bubble: a sine whose pitch follows the bubble's radius and
/// rises as it rises, dying fast.
fn bubble(rng: &mut Rng, hz: f32, tau: f32) -> Vec<f32> {
    let n = frames(tau * 6.0);
    let mut ph = 0.0f32;
    let rise = rng.range(0.4, 1.5);
    (0..n).map(|i| {
        let t = i as f32 / SR;
        ph += TAU * hz * (1.0 + rise * t / tau.max(1e-3) * 0.3) / SR;
        ph.sin() * (-t / tau).exp() * smooth(t, 0.0008)
    }).collect()
}

fn mk_pour(rng: &mut Rng, p: &Params, stereo: &mut Stereo) {
    let n = stereo.len();
    let big = p.size;
    let rate = 60.0 + 100.0 * p.speed;
    let mut t = 0.0f32;
    while t < n as f32 / SR {
        t += -(rng.unit().max(1e-3)).ln() / rate;
        let r_mm = 1.0 + 4.0 * rng.unit().powf(1.5) * (0.5 + big);
        let hz = (3260.0 / r_mm).clamp(300.0, 6000.0);
        let _r5 = rng.range(0.004, 0.02);
        let b = bubble(rng, hz, _r5);
        stereo.add_mono(&b, frames(t), rng.range(0.1, 0.5), rng.range(-0.6, 0.6));
    }
    let mut wd = Wander::new(rng.u64(), 0.5);
    let (mut f, mut g) = (Svf::new(), Svf::new());
    let mut mono = vec![0.0; n];
    for o in mono.iter_mut() {
        let k = wd.tick();
        let x = rng.signed();
        *o = (f.at(x, 1500.0 + 900.0 * k, 0.8).bp * 0.5 + g.at(x, 4200.0, 0.8).bp * 0.15) * (0.35 + 0.3 * k);
    }
    stereo.add_mono(&mono, 0, 0.5, -0.2);
}

fn mk_drip(rng: &mut Rng, p: &Params, dur: f32) -> Vec<f32> {
    // Where it drips: a cave's slow drops ringing in the rock, a tap's
    // steady ones into a steel sink, a bucket's hollow plunk, a basin's plink.
    let tx = &p.text;
    let (lo, hi, gap, ring_k, ring_tau, ring_g, echo) = if tx.contains("cave") || tx.contains("cavern") || tx.contains("tunnel") || tx.contains("sewer") {
        (900.0, 1800.0, (0.9, 2.2), 0.5, 0.25, 0.25, true)
    } else if tx.contains("tap") || tx.contains("faucet") || tx.contains("sink") {
        (1800.0, 2600.0, (0.55, 0.65), 0.42, 0.35, 0.3, false)
    } else if tx.contains("bucket") || tx.contains("pail") || tx.contains("pot") {
        (500.0, 900.0, (0.4, 1.1), 0.8, 0.12, 0.35, false)
    } else if tx.contains("leak") || tx.contains("fast") {
        (1600.0, 3200.0, (0.12, 0.35), 0.5, 0.1, 0.12, false)
    } else {
        (1600.0, 3200.0, (0.35, 1.3), 0.5, 0.1, 0.12, false)
    };
    let count = p.count.unwrap_or(((dur / (0.5 * (gap.0 + gap.1))).ceil() as usize).max(1)).clamp(1, 60);
    let mut out = vec![0.0; frames(dur.max(0.4))];
    let mut t = 0.02;
    for _ in 0..count {
        let hz = rng.range(lo, hi) * (1.2 - 0.4 * p.size);
        let _r6 = rng.range(0.012, 0.03);
        let b = bubble(rng, hz, _r6);
        put(&mut out, &b, t, rng.range(0.5, 1.0));
        let tap = click(rng, 3500.0, 1.0, 0.002);
        put(&mut out, &tap, t, 0.2);
        // The plink of the drop landing carries on a little.
        let ring = struck(hz * ring_k, &[(1.0, 0.4, ring_tau)], 1.0, ring_tau * 4.0);
        put(&mut out, &ring, t + 0.002, ring_g);
        // A cave answers back.
        if echo {
            for (k, g) in [(0.11f32, 0.35f32), (0.23, 0.2), (0.37, 0.1)] {
                let e = low_pass(&b, 2500.0, 0.6);
                put(&mut out, &e, t + k, g);
            }
        }
        t += rng.range(gap.0, gap.1);
        if t > dur - 0.2 {
            break;
        }
    }
    out
}

fn mk_splash(rng: &mut Rng, p: &Params, dur: f32, stereo: &mut Stereo) {
    // A pebble's plop, a paddle, a body jumping in, a cannonball.
    let tx = &p.text;
    let big = if tx.contains("pebble") || tx.contains("plop") || tx.contains("stone in") || tx.contains("kerplunk") {
        0.12
    } else if tx.contains("paddle") || tx.contains("oar") {
        0.3
    } else if tx.contains("cannonball") || tx.contains("belly flop") || tx.contains("bellyflop") {
        1.0
    } else if tx.contains("jump") || tx.contains("dive") || tx.contains("falls in") {
        0.8
    } else {
        0.3 + 0.7 * p.size
    };
    let len = (0.4 + 1.2 * big).max(dur.min(2.0));
    let n = frames(len);
    let (mut a, mut b) = (Svf::new(), Svf::new());
    let mono: Vec<f32> = (0..n).map(|i| {
        let t = i as f32 / SR;
        let x = rng.signed();
        (a.at(x, 700.0 + 1800.0 * (-t / 0.1).exp(), 0.5).bp * 1.3 + b.at(x, 3500.0, 0.6).bp * 0.4) * smooth(t, 0.004) * (-t / (0.08 + 0.12 * big)).exp()
    }).collect();
    stereo.add_mono(&mono, 0, 0.9, 0.0);
    for _ in 0..(20.0 + 60.0 * big) as usize {
        let t = rng.unit().powf(1.8) * len * 0.9;
        let hz = rng.range(500.0, 3500.0);
        let _r7 = rng.range(0.005, 0.03);
        let b = bubble(rng, hz, _r7);
        stereo.add_mono(&b, frames(t), rng.range(0.1, 0.6), rng.range(-0.8, 0.8));
    }
    let thump = struck(80.0, &[(1.0, 1.0, 0.07)], 1.0, 0.3);
    stereo.add_mono(&thump, 0, 0.4 * big, 0.0);
}

fn mk_bubbles(rng: &mut Rng, p: &Params, stereo: &mut Stereo) {
    let n = stereo.len();
    let rate = 5.0 + 22.0 * p.intensity;
    let mut t = 0.0f32;
    while t < n as f32 / SR {
        t += -(rng.unit().max(1e-3)).ln() / rate;
        let hz = rng.range(180.0, 1100.0) * (1.4 - 0.7 * p.size);
        let _r8 = rng.range(0.01, 0.045);
        let b = bubble(rng, hz, _r8);
        stereo.add_mono(&b, frames(t), rng.range(0.15, 0.8), rng.range(-0.7, 0.7));
    }
    let mut lp = Lp1::default();
    let mut w = Wander::new(rng.u64(), 0.8);
    let mono: Vec<f32> = (0..n).map(|_| lp.tick(rng.signed(), 350.0) * (0.08 + 0.1 * w.tick())).collect();
    stereo.add_mono(&mono, 0, 1.0, 0.0);
}

fn mk_rain(rng: &mut Rng, p: &Params, stereo: &mut Stereo) {
    let n = stereo.len();
    let heavy = 0.3 + 0.7 * p.intensity;
    let (mut a, mut b, mut c, mut d) = (Svf::new(), Svf::new(), Svf::new(), Svf::new());
    for i in 0..n {
        let (x, y) = (rng.signed(), rng.signed());
        stereo.l[i] += (a.at(x, 6500.0, 0.8).bp * 0.45 + b.at(x, 1200.0, 0.6).bp * 0.15) * heavy;
        stereo.r[i] += (c.at(y, 6300.0, 0.8).bp * 0.45 + d.at(y, 1150.0, 0.6).bp * 0.15) * heavy;
    }
    let drops = 50.0 + 400.0 * heavy;
    let mut t = 0.0f32;
    while t < n as f32 / SR {
        t += -(rng.unit().max(1e-3)).ln() / drops;
        let _r9 = rng.range(1800.0, 7000.0);
        let _r10 = rng.range(0.0015, 0.006);
        let tick = click(rng, _r9, 1.4, _r10);
        stereo.add_mono(&tick, frames(t), rng.unit().powi(2) * 0.8, rng.range(-1.0, 1.0));
    }
}

fn mk_wind(rng: &mut Rng, p: &Params, stereo: &mut Stereo) {
    let n = stereo.len();
    let howl = p.text.contains("howl") || p.text.contains("gale") || p.text.contains("blizzard");
    let (mut a, mut b, mut wa, mut wb, mut tone) = (Svf::new(), Svf::new(), Svf::new(), Svf::new(), Svf::new());
    let mut w1 = Wander::new(rng.u64(), 1.6);
    let mut w2 = Wander::new(rng.u64(), 2.1);
    let mut w3 = Wander::new(rng.u64(), 3.0);
    for i in 0..n {
        let g = w1.tick();
        let hz = 250.0 + 700.0 * w2.tick();
        let gust = 0.08 + 0.92 * g.powf(1.6);
        let (x, y) = (rng.signed(), rng.signed());
        let mut l = a.at(x, hz, 3.0 + 4.0 * g).bp * 1.4 + wa.at(x, 90.0, 0.6).lp * 0.2;
        let mut r = b.at(y, hz * 1.08, 3.0 + 4.0 * g).bp * 1.4 + wb.at(y, 90.0, 0.6).lp * 0.2;
        if howl {
            let t = tone.at(x, 520.0 + 700.0 * w3.tick(), 18.0).bp * 2.5;
            l += t * g;
            r += t * g * 0.8;
        }
        stereo.l[i] += l * gust * 0.5;
        stereo.r[i] += r * gust * 0.5;
    }
}

/// Pulses from firing cylinders through the body and exhaust of an engine.
fn engine_tone(rng: &mut Rng, secs: f32, rpm_at: &dyn Fn(f32) -> f32, cylinders: f32, rough: f32, period: bool) -> Vec<f32> {
    let n = frames(secs);
    let mut out = vec![0.0; n];
    let (mut body, mut exh, mut mech) = (Svf::new(), Svf::new(), Svf::new());
    let (cb, ce) = (SvfCoef::new(130.0, 2.5), SvfCoef::new(420.0, 1.6));
    let mut phase = 0.0f32;
    let mut pulse = 0.0f32;
    let mut lp = Lp1::default();
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        let rpm = rpm_at(t);
        let fire = rpm / 60.0 * cylinders / 2.0;
        phase += fire / SR;
        if phase >= 1.0 {
            phase -= 1.0;
            let miss = period && rng.chance(0.08);
            pulse = if miss { 0.15 } else { (1.0 + rough * rng.signed() * 0.8).max(0.2) };
        }
        pulse *= 1.0 - 90.0 / SR * (fire / 15.0).max(1.0);
        let x = pulse + rng.signed() * 0.02;
        let y = body.tick(&cb, x).bp * 1.5 + exh.tick(&ce, x).bp * 1.0 + lp.tick(x, 900.0) * 0.5;
        let tick = mech.at(rng.signed() * (pulse * pulse), 2200.0, 1.2).bp * if period { 0.5 } else { 0.2 };
        *o = y + tick;
    }
    out
}

fn mk_engine(rng: &mut Rng, p: &Params, dur: f32) -> Vec<f32> {
    let rev = p.text.contains("rev") || p.text.contains("start");
    let idle = if p.period { 520.0 } else { 800.0 } * (1.0 + 0.2 * (1.0 - p.size));
    let cyl = if p.text.contains("motorcycle") { 1.0 } else if p.text.contains("truck") || p.text.contains("diesel") || p.text.contains("tractor") { 6.0 } else if p.period { 4.0 } else { 4.0 };
    let wob = rng.range(0.0, 10.0);
    let d = dur;
    let rpm = move |t: f32| -> f32 {
        let wander = 1.0 + 0.02 * (t * 1.7 + wob).sin() + 0.015 * (t * 4.3).sin();
        let r = if rev { let k = (t / d).clamp(0.0, 1.0); idle * (1.0 + 2.2 * (PI * k).sin().powf(1.5)) } else { idle };
        r * wander
    };
    let mut x = engine_tone(rng, dur, &rpm, cyl, if p.period { 0.45 } else { 0.15 }, p.period);
    distance(&mut x, p.far(0.0));
    x
}

/// A vehicle going past: pitch falls through the pass (Doppler), loudness
/// swells and fades, and it moves across the stereo field.
fn mk_passby(rng: &mut Rng, p: &Params, dur: f32, stereo: &mut Stereo, at: f32, len: f32, dist: f32) {
    let speed_ms = 8.0 + 16.0 * p.speed;
    let cyl = if p.text.contains("truck") { 6.0 } else { 4.0 };
    let rpm_base = if p.period { 1900.0 } else { 2400.0 } * rng.range(0.85, 1.15);
    let src_len = len * 1.3 + 1.0;
    let rpm = |t: f32| rpm_base * (1.0 + 0.03 * (t * 1.3).sin());
    let src = engine_tone(rng, src_len, &rpm, cyl, if p.period { 0.4 } else { 0.12 }, p.period);
    let mut tyre = Svf::new();
    let tyres: Vec<f32> = (0..src.len()).map(|_| tyre.at(rng.signed(), 1400.0 + 700.0 * speed_ms / 24.0, 0.5).bp * 0.5).collect();
    let n = frames(len);
    let near = 3.0 + 20.0 * dist;
    let c = 343.0;
    let mut pos = 0.0f64;
    let (mut lp_state, mut lp_state2) = (0.0f32, 0.0f32);
    let mut out_l = vec![0.0; n];
    let mut out_r = vec![0.0; n];
    for i in 0..n {
        let t = i as f32 / SR - len / 2.0;
        let x = speed_ms * t;
        let r = (near * near + x * x).sqrt();
        let cos_t = x / r;
        // Moving toward the listener while x < 0.
        let factor = 1.0 / (1.0 + speed_ms / c * -cos_t * -1.0 * -1.0).max(0.5);
        let factor = factor * 0.0 + 1.0 / (1.0 - speed_ms / c * (-cos_t));
        pos += factor as f64;
        let idx = pos as usize;
        if idx + 1 >= src.len() {
            break;
        }
        let fr = (pos - idx as f64) as f32;
        let s = (src[idx] * (1.0 - fr) + src[idx + 1] * fr) + (tyres[idx] * (1.0 - fr) + tyres[idx + 1] * fr) * 0.8;
        let gain = near / r;
        // Air takes the highs off with distance.
        let a = (1.0 - (r / 140.0).min(0.97)).powi(2) * 0.9 + 0.05;
        lp_state += a * (s * gain - lp_state);
        lp_state2 += a * (lp_state - lp_state2);
        let pan = (x / (near + 15.0)).clamp(-1.0, 1.0);
        let (gl, gr) = (((1.0 - pan) * 0.5).sqrt(), ((1.0 + pan) * 0.5).sqrt());
        out_l[i] = lp_state2 * gl;
        out_r[i] = lp_state2 * gr;
    }
    let _ = dur;
    mix_into(&mut stereo.l, &out_l, frames(at), 1.0);
    mix_into(&mut stereo.r, &out_r, frames(at), 1.0);
}

fn horn_tone(rng: &mut Rng, period: bool) -> Vec<f32> {
    if period {
        // A bulb horn: a reed that rises, then a second honk lower.
        let mut out = vec![0.0; frames(0.9)];
        for (k, (at, f0, len)) in [(0.0f32, 330.0f32, 0.28f32), (0.34, 247.0, 0.5)].iter().enumerate() {
            let f0 = f0 * rng.range(0.95, 1.05);
            let mut saw = Saw::at(0.0);
            let mut flt = Svf::new();
            let note: Vec<f32> = (0..frames(*len)).map(|i| {
                let t = i as f32 / SR;
                let f = f0 * (0.9 + 0.1 * smooth(t, 0.05)) * if k == 1 { 1.0 + 0.04 * (t / len) } else { 1.0 };
                flt.at(saw.tick(f), 1200.0, 2.0).bp * 2.0 * smooth(t, 0.01) * smooth(len - t, 0.04)
            }).collect();
            put(&mut out, &note, *at, 1.0);
        }
        out
    } else {
        let f0 = rng.range(380.0, 460.0);
        let (mut a, mut b) = (Pulse::at(0.0), Pulse::at(0.3));
        let mut flt = Svf::new();
        (0..frames(0.55)).map(|i| {
            let t = i as f32 / SR;
            flt.at(a.tick(f0, 0.5) + b.tick(f0 * 1.26, 0.5), 2200.0, 0.8).lp * 0.5 * smooth(t, 0.01) * smooth(0.55 - t, 0.03)
        }).collect()
    }
}

fn mk_hooves(rng: &mut Rng, p: &Params, dur: f32) -> Vec<f32> {
    let gait_hz = if p.speed > 0.7 { 6.5 } else if p.speed > 0.4 { 3.2 } else { 2.0 };
    let surf = p.surface.unwrap_or(Surface::Stone);
    let mut out = vec![0.0; frames(dur.max(1.0))];
    let mut t = 0.05;
    let mut k = 0usize;
    while t < dur - 0.1 {
        let hz = match surf {
            Surface::Stone | Surface::Tile | Surface::Metal | Surface::WetStone => rng.range(900.0, 1500.0),
            _ => rng.range(300.0, 600.0),
        };
        let hit = struck(hz, &[(1.0, 1.0, 0.025), (2.7, 0.4, 0.012)], 1.0, 0.12);
        let c = click(rng, 3000.0, 1.0, 0.003);
        let hollow = struck(rng.range(220.0, 300.0), &[(1.0, 1.0, 0.04)], 1.0, 0.15);
        let g = rng.range(0.7, 1.0);
        put(&mut out, &hit, t, g);
        put(&mut out, &c, t, 0.7 * g);
        put(&mut out, &hollow, t, 0.5 * g);
        // A walk is four even beats, a trot two, a gallop a rolling triplet.
        let step = if p.speed > 0.7 { [0.35, 0.2, 0.2, 0.45][k % 4] } else { 1.0 };
        t += step / gait_hz * rng.range(0.96, 1.05);
        k += 1;
    }
    out
}

/// A babble of voices: each a burst of syllables on a drifting pitch through
/// vowel formants, with pauses between phrases.
fn mk_murmur(rng: &mut Rng, p: &Params, stereo: &mut Stereo, cafe: bool) {
    let n = stereo.len();
    let voices = 6 + (14.0 * (0.4 + 0.6 * p.size)) as usize + if cafe { 2 } else { 0 };
    const VOWELS: [(f32, f32); 8] = [(730.0, 1090.0), (530.0, 1840.0), (270.0, 2290.0), (570.0, 840.0), (300.0, 870.0), (660.0, 1720.0), (400.0, 1600.0), (440.0, 1020.0)];
    for v in 0..voices {
        let f0_base = if rng.chance(0.5) { rng.range(95.0, 135.0) } else { rng.range(175.0, 255.0) };
        let pan = rng.range(-0.85, 0.85);
        let mut out = vec![0.0; n];
        let (mut f1, mut f2, mut fn_) = (Svf::new(), Svf::new(), Svf::new());
        let mut saw = Saw::at(rng.unit());
        let mut t = rng.range(0.0, 2.0);
        let rate = rng.range(3.4, 5.4);
        while t < n as f32 / SR {
            let phrase = rng.range(0.7, 2.6);
            let syl = (phrase * rate) as usize + 1;
            let f0_start = f0_base * rng.range(0.9, 1.15);
            let mut st = t;
            for k in 0..syl {
                let sd = 1.0 / rate * rng.range(0.8, 1.2);
                let vow = VOWELS[rng.below(VOWELS.len())];
                let from = (st * SR) as usize;
                let to = (((st + sd) * SR) as usize).min(n);
                for i in from..to {
                    let u = (i - from) as f32 / (to - from).max(1) as f32;
                    let env = (PI * u).sin().powf(1.3);
                    let decl = 1.0 - 0.25 * ((k as f32 + u) / syl as f32);
                    let f = f0_start * decl * (1.0 + 0.04 * (TAU * 5.0 * i as f32 / SR).sin());
                    let src = saw.tick(f);
                    let y = f1.at(src, vow.0, 6.0).bp + 0.7 * f2.at(src, vow.1, 8.0).bp;
                    // A consonant hiss at the syllable edge.
                    let c = if u < 0.12 { fn_.at(rng.signed(), 4500.0, 0.8).hp * 0.25 } else { 0.0 };
                    out[i] += (y * 0.9 + c) * env;
                }
                st += sd;
            }
            t = st + rng.range(0.3, 2.2);
        }
        let g = 0.6 / (voices as f32).sqrt() * rng.range(0.6, 1.0);
        stereo.add_mono(&out, 0, g, pan);
        let _ = v;
    }
    // A room around them.
    let mut wet = level::reverb(stereo, "living_room");
    wet.scale(0.5);
    stereo.add_at(&wet, 0, 1.0);
}

fn mk_cafe_extras(rng: &mut Rng, stereo: &mut Stereo) {
    let n = stereo.len();
    let secs = n as f32 / SR;
    let mut t = rng.range(0.1, 0.8);
    while t < secs {
        t += rng.range(0.4, 1.8);
        let pan = rng.range(-0.8, 0.8);
        match rng.below(5) {
            0 | 1 => {
                // Cutlery on a plate.
                let hz = rng.range(2500.0, 5500.0);
                let c = struck(hz, &[(1.0, 1.0, 0.07), (2.3, 0.5, 0.04), (3.6, 0.3, 0.025)], 1.0, 0.3);
                stereo.add_mono(&c, frames(t), rng.range(0.15, 0.45), pan);
                if rng.chance(0.4) {
                    stereo.add_mono(&c, frames(t + rng.range(0.06, 0.2)), rng.range(0.1, 0.3), pan);
                }
            }
            2 => {
                // A cup set on a saucer.
                let hz = rng.range(1600.0, 2600.0);
                let c = struck(hz, &[(1.0, 1.0, 0.12), (2.6, 0.5, 0.07)], 1.0, 0.4);
                stereo.add_mono(&c, frames(t), rng.range(0.2, 0.5), pan);
                let th = struck(rng.range(250.0, 400.0), &[(1.0, 1.0, 0.03)], 1.0, 0.15);
                stereo.add_mono(&th, frames(t), 0.2, pan);
            }
            3 => {
                // A chair shifting.
                let s = {
                    let p = Params::read("wood scrape", "wood scrape");
                    let _r11 = rng.range(0.25, 0.6);
                    mk_scrape(rng, &p, _r11)
                };
                stereo.add_mono(&s, frames(t), 0.18, pan);
            }
            _ => {
                // A spoon stirring: a quick run of small clinks.
                let hz = rng.range(3000.0, 5000.0);
                let c = struck(hz, &[(1.0, 1.0, 0.04), (2.4, 0.4, 0.02)], 1.0, 0.15);
                for k in 0..rng.range(4.0, 9.0) as usize {
                    stereo.add_mono(&c, frames(t + k as f32 * rng.range(0.07, 0.13)), rng.range(0.06, 0.2), pan);
                }
            }
        }
    }
    // The machine behind the counter, now and then.
    if secs > 4.0 && rng.chance(0.7) {
        let at = rng.range(1.0, secs - 3.0);
        let len = rng.range(1.0, 2.0);
        let mut f = Svf::new();
        let x: Vec<f32> = (0..frames(len)).map(|i| {
            let t = i as f32 / SR;
            f.at(rng.signed(), 5500.0, 0.6).hp * smooth(t, 0.1) * smooth(len - t, 0.4) * 0.12
        }).collect();
        stereo.add_mono(&x, frames(at), 1.0, 0.5);
    }
}

fn mk_applause(rng: &mut Rng, p: &Params, stereo: &mut Stereo) {
    let n = stereo.len();
    let secs = n as f32 / SR;
    let people = 20.0 + 160.0 * (0.2 + 0.8 * p.size);
    let mut t = 0.0f32;
    while t < secs {
        let u = t / secs;
        let swell = smooth(t, 0.8).min(smooth(secs - t, 1.0)).max(0.05) * (0.7 + 0.3 * (u * 9.0).sin());
        t += -(rng.unit().max(1e-3)).ln() / (people * 2.6 * swell.max(0.05));
        let _r12 = rng.range(1100.0, 3800.0);
        let _r13 = rng.range(0.7, 1.6);
        let _r14 = rng.range(0.004, 0.011);
        let c = click(rng, _r12, _r13, _r14);
        stereo.add_mono(&c, frames(t), rng.range(0.1, 0.5) * (0.4 + 0.6 * swell), rng.range(-1.0, 1.0));
    }
    let mut f = Svf::new();
    let wash: Vec<f32> = (0..n).map(|i| {
        let u = i as f32 / n as f32;
        f.at(rng.signed(), 2200.0, 0.4).bp * 0.18 * smooth(i as f32 / SR, 0.8) * smooth(secs - i as f32 / SR, 1.0) * (0.6 + 0.4 * (u * 6.0).sin())
    }).collect();
    stereo.add_mono(&wash, 0, 1.0, 0.0);
}

fn mk_clock(rng: &mut Rng, p: &Params, dur: f32) -> Vec<f32> {
    let mut out = vec![0.0; frames(dur.max(1.0))];
    let period = 1.0 / (0.8 + 0.8 * p.speed);
    let mut t = 0.02;
    let mut k = 0;
    while t < dur {
        let (hz, ring) = if k % 2 == 0 { (3600.0, 1250.0) } else { (2900.0, 980.0) };
        let _r15 = rng.range(0.97, 1.03);
        let tick = click(rng, hz * _r15, 1.4, 0.0025);
        let r = struck(ring * rng.range(0.98, 1.02), &[(1.0, 1.0, 0.03), (2.3, 0.4, 0.018)], 1.0, 0.15);
        put(&mut out, &tick, t, 0.9);
        put(&mut out, &r, t, 0.3);
        t += period * rng.range(0.995, 1.005);
        k += 1;
    }
    out
}

fn mk_click(rng: &mut Rng, p: &Params) -> Vec<f32> {
    let mut out = vec![0.0; frames(0.25)];
    let hz = rng.range(2200.0, 3600.0) * (1.2 - 0.5 * p.size);
    let a = click(rng, hz, 1.6, 0.0018);
    let b = struck(hz * 0.8, &[(1.0, 1.0, 0.012), (2.2, 0.4, 0.008)], 1.0, 0.08);
    put(&mut out, &a, 0.0, 1.0);
    put(&mut out, &b, 0.0, 0.4);
    // Many switches click twice: on and settle.
    let a2 = click(rng, hz * 0.8, 1.4, 0.0015);
    put(&mut out, &a2, rng.range(0.025, 0.05), 0.5);
    out
}

fn mk_lock(rng: &mut Rng, p: &Params) -> Vec<f32> {
    let mut out = vec![0.0; frames(1.0)];
    let hz = rng.range(900.0, 1500.0);
    // The key slides in, the tumblers lift, the bolt throws.
    for k in 0..4 {
        let _r16 = rng.range(3000.0, 5000.0);
        let c = click(rng, _r16, 1.4, 0.002);
        put(&mut out, &c, 0.02 + k as f32 * 0.025, 0.25);
    }
    let tumbler = struck(hz * 1.5, &[(1.0, 1.0, 0.03), (2.7, 0.4, 0.02)], 1.0, 0.15);
    put(&mut out, &tumbler, 0.2, 0.5);
    let bolt = struck(hz * 0.35, &[(1.0, 1.0, 0.06), (2.1, 0.5, 0.04), (3.7, 0.3, 0.025)], 1.0, 0.25);
    let c = click(rng, 2200.0, 1.0, 0.004);
    put(&mut out, &bolt, 0.42 + 0.1 * (1.0 - p.speed), 0.9);
    put(&mut out, &c, 0.42 + 0.1 * (1.0 - p.speed), 0.7);
    let settle = struck(hz * 0.5, &[(1.0, 1.0, 0.04)], 1.0, 0.12);
    put(&mut out, &settle, 0.5 + 0.1 * (1.0 - p.speed), 0.35);
    out
}

fn mk_door(rng: &mut Rng, p: &Params) -> Vec<f32> {
    let slam = p.text.contains("slam") || p.text.contains("bang");
    let bell = p.text.contains("doorbell");
    if bell {
        let hz = rng.range(520.0, 700.0);
        let mut out = vec![0.0; frames(2.2)];
        let a = struck(hz * 1.26, &[(1.0, 1.0, 0.7), (2.0, 0.5, 0.4), (3.0, 0.25, 0.25)], 1.0, 1.4);
        let b = struck(hz, &[(1.0, 1.0, 0.9), (2.0, 0.5, 0.5), (3.0, 0.25, 0.3)], 1.0, 1.8);
        put(&mut out, &a, 0.0, 0.7);
        put(&mut out, &b, 0.55, 0.8);
        return out;
    }
    let mut out = vec![0.0; frames(if slam { 1.6 } else { 1.2 })];
    let pp = Params { size: if slam { 0.95 } else { 0.7 }, ..p.clone() };
    let body = impact(rng, &pp, 0, if slam { 1.0 } else { 0.55 });
    put(&mut out, &body, 0.0, 1.0);
    let latch = click(rng, 3000.0, 1.4, 0.003);
    let l2 = struck(1900.0, &[(1.0, 1.0, 0.02), (2.5, 0.4, 0.01)], 1.0, 0.1);
    let at = if slam { 0.0 } else { 0.01 };
    put(&mut out, &latch, at, 0.6);
    put(&mut out, &l2, at, 0.3);
    // The frame rattles after a slam.
    if slam {
        let rat = struck(180.0, &[(1.0, 1.0, 0.2), (2.6, 0.5, 0.1)], 1.0, 0.8);
        put(&mut out, &rat, 0.02, 0.4);
    }
    out
}

fn mk_typing(rng: &mut Rng, p: &Params, dur: f32) -> Vec<f32> {
    let mut out = vec![0.0; frames(dur.max(0.6))];
    let old = p.text.contains("typewriter") || p.period;
    let rate = 4.0 + 6.0 * p.speed;
    let mut t = 0.05;
    let mut k = 0;
    while t < dur - 0.1 {
        let _r17 = rng.range(0.9, 1.1);
        let c = click(rng, if old { 2400.0 } else { 3400.0 } * _r17, 1.2, 0.0025);
        let th = struck(if old { 160.0 } else { 220.0 } * rng.range(0.9, 1.1), &[(1.0, 1.0, 0.035), (2.5, 0.3, 0.02)], 1.0, 0.15);
        let g = rng.range(0.5, 1.0);
        put(&mut out, &c, t, g);
        put(&mut out, &th, t, 0.7 * g);
        if old {
            let m = click(rng, 5200.0, 1.5, 0.002);
            put(&mut out, &m, t + 0.012, 0.35 * g);
        }
        t += (1.0 / rate) * rng.range(0.45, 1.6) * if rng.chance(0.1) { 2.5 } else { 1.0 };
        k += 1;
        // The carriage return bell.
        if old && k % 28 == 0 {
            let bell = struck(2750.0, &[(1.0, 1.0, 0.5), (2.4, 0.3, 0.3)], 1.0, 1.2);
            put(&mut out, &bell, t, 0.4);
            t += 0.6;
        }
    }
    out
}

fn mk_machine(rng: &mut Rng, p: &Params, stereo: &mut Stereo) {
    let n = stereo.len();
    let mains = if rng.chance(0.5) { 50.0 } else { 60.0 };
    let low = p.size > 0.6;
    let mut osc: Vec<(Rot, f32)> = (1..=8).map(|h| (Rot::new(mains * h as f32 * if low { 0.5 } else { 1.0 }, rng.unit() * TAU), 1.0 / (h as f32).powf(1.1))).collect();
    let mut w = Wander::new(rng.u64(), 2.5);
    let (mut a, mut b) = (Svf::new(), Svf::new());
    let mut motor = Rot::new(rng.range(180.0, 420.0), 0.0);
    for i in 0..n {
        let k = w.tick();
        let hum: f32 = osc.iter_mut().map(|(r, g)| r.tick() * *g).sum();
        let x = rng.signed();
        let air = a.at(x, 700.0 + 500.0 * k, 0.7).bp * 0.12;
        let m = motor.tick() * 0.1 * (0.8 + 0.4 * k);
        let v = hum * 0.18 + air + m + b.at(x, 5500.0, 0.8).hp * 0.012;
        stereo.l[i] += v;
        stereo.r[i] += v * 0.96;
    }
}

fn mk_servo(rng: &mut Rng, p: &Params, dur: f32) -> Vec<f32> {
    let n = frames(dur.max(0.6));
    let mut out = vec![0.0; n];
    let (mut s, mut f) = (Saw::at(0.0), Svf::new());
    let f0 = rng.range(180.0, 380.0) * (1.4 - 0.7 * p.size);
    let stages = if p.speed > 0.6 { 3 } else { 2 };
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / n as f32;
        let seg = (t * stages as f32).floor();
        let u = (t * stages as f32).fract();
        let hz = f0 * (1.0 + 2.2 * smooth(u, 1.0).powf(0.7) * (1.0 - 0.3 * seg / stages as f32));
        let env = smooth(u, 0.12) * smooth(1.0 - u, 0.2);
        *o = f.at(s.tick(hz) + rng.signed() * 0.05, hz * 5.0, 1.6).lp * env * 0.6;
    }
    out
}

fn mk_camera(rng: &mut Rng) -> Vec<f32> {
    let mut out = vec![0.0; frames(0.45)];
    let hz = rng.range(2400.0, 3400.0);
    for (at, g) in [(0.0f32, 1.0f32), (rng.range(0.025, 0.06), 0.8)] {
        let c = click(rng, hz, 1.4, 0.0022);
        let r = struck(hz * 0.6, &[(1.0, 1.0, 0.015), (2.4, 0.4, 0.01)], 1.0, 0.1);
        put(&mut out, &c, at, g);
        put(&mut out, &r, at, 0.3 * g);
    }
    let wind = struck(900.0, &[(1.0, 1.0, 0.03)], 1.0, 0.12);
    put(&mut out, &wind, 0.12, 0.2);
    out
}

fn mk_phone(rng: &mut Rng, p: &Params, dur: f32) -> Vec<f32> {
    let n = frames(dur.max(2.0));
    let mut out = vec![0.0; n];
    if p.period || p.text.contains("old") || p.text.contains("bell") {
        // Two clappers on twin bells, struck about 20 times a second.
        let (a, b) = (rng.range(1050.0, 1250.0), rng.range(1380.0, 1620.0));
        let mut t = 0.0;
        while t < dur {
            for k in 0..38 {
                let hz = if k % 2 == 0 { a } else { b };
                let s = struck(hz, &[(1.0, 1.0, 0.05), (2.3, 0.4, 0.03)], 1.0, 0.12);
                put(&mut out, &s, t + k as f32 * 0.052, 0.55);
            }
            t += 2.0 + 2.2;
        }
    } else {
        let base = rng.range(780.0, 980.0);
        let mut t = 0.0;
        while t < dur {
            for k in 0..4 {
                let s: Vec<f32> = (0..frames(0.12)).map(|i| {
                    let u = i as f32 / SR;
                    ((TAU * base * u).sin() + 0.5 * (TAU * base * 1.5 * u).sin()) * smooth(u, 0.005) * smooth(0.12 - u, 0.02)
                }).collect();
                put(&mut out, &s, t + k as f32 * 0.17, 0.5);
            }
            t += 1.6;
        }
    }
    out
}

fn mk_heartbeat(rng: &mut Rng, p: &Params, dur: f32) -> Vec<f32> {
    let mut out = vec![0.0; frames(dur.max(1.2))];
    let bpm = 52.0 + 90.0 * p.speed;
    let period = 60.0 / bpm;
    let mut t = 0.05;
    while t < dur {
        let lub = struck(rng.range(52.0, 62.0), &[(1.0, 1.0, 0.09), (2.0, 0.3, 0.05)], 1.0, 0.3);
        let dub = struck(rng.range(66.0, 78.0), &[(1.0, 1.0, 0.07), (2.0, 0.3, 0.04)], 1.0, 0.25);
        put(&mut out, &lub, t, 1.0);
        put(&mut out, &dub, t + 0.3 * period.min(0.9), 0.75);
        t += period * rng.range(0.97, 1.03);
    }
    out
}

fn mk_breath(rng: &mut Rng, p: &Params, dur: f32, stereo: &mut Stereo) {
    let n = stereo.len();
    let cycle = 3.8 - 2.6 * p.speed;
    let (mut a, mut b) = (Svf::new(), Svf::new());
    let mono: Vec<f32> = (0..n).map(|i| {
        let t = i as f32 / SR;
        let u = (t / cycle).fract();
        // In for 40 percent of the cycle, a pause, out for the rest.
        let (e, hz) = if u < 0.4 { ((PI * u / 0.4).sin().powf(1.3), 1500.0 + 800.0 * u) } else if u < 0.5 { (0.0, 1000.0) } else { ((PI * (u - 0.5) / 0.5).sin().powf(1.6) * 0.8, 900.0 + 500.0 * (1.0 - u)) };
        let x = rng.signed();
        (a.at(x, hz, 0.8).bp * 1.2 + b.at(x, 3000.0, 0.7).bp * 0.2) * e
    }).collect();
    let _ = dur;
    stereo.add_mono(&mono, 0, 0.5, 0.0);
}

fn mk_whoosh(rng: &mut Rng, p: &Params, dur: f32, stereo: &mut Stereo) {
    let n = stereo.len();
    let (mut a, mut b) = (Svf::new(), Svf::new());
    let bright = 0.5 + 0.5 * p.speed;
    for i in 0..n {
        let u = i as f32 / n as f32;
        let env = (PI * u).sin().powi(2);
        let hz = (500.0 + 5500.0 * bright * (PI * u).sin().powf(1.5)) * rng.range(1.0, 1.0);
        let pan = u * PI / 2.0;
        let (x, y) = (rng.signed(), rng.signed());
        stereo.l[i] += a.at(x, hz, 0.7).bp * env * pan.cos() * 1.4;
        stereo.r[i] += b.at(y, hz, 0.7).bp * env * pan.sin() * 1.4;
    }
    let _ = dur;
}

/// A rising sweep that peaks at `peak` seconds, then rings away through
/// `tail` seconds of reverb.
fn mk_riser(rng: &mut Rng, rise: f32, tail: f32) -> (Stereo, f32) {
    let r = riser(rise, rng);
    let total = frames(rise + tail);
    let mut dry = Stereo::silent(total);
    let (l, rr) = (Stereo::panned(&r, -0.15), Stereo::panned(&high_pass(&r, 400.0, 0.7), 0.15));
    dry.add_at(&l, 0, 0.6);
    dry.add_at(&rr, 0, 0.6);
    let mut send = dry.clone();
    // Cut the send at the peak, so the tail is only the room ringing out.
    for i in frames(rise)..total {
        send.l[i] = 0.0;
        send.r[i] = 0.0;
    }
    let mut room = level::reverb(&send, "hall");
    let mut room2 = level::reverb(&room, "church");
    room.scale(0.7);
    room2.scale(0.9);
    dry.add_at(&room, 0, 0.8);
    dry.add_at(&room2, 0, 0.5);
    // The hit at the top of the rise.
    let hit = struck(70.0, &[(1.0, 1.0, 0.4)], 1.0, 1.2);
    let hit = low_pass(&hit, 300.0, 0.7);
    let hz = frames(rise);
    mix_into(&mut dry.l, &hit, hz, 0.8);
    mix_into(&mut dry.r, &hit, hz, 0.8);
    // The tail fades out.
    for i in hz..total {
        let u = (i - hz) as f32 / (total - hz).max(1) as f32;
        let g = (1.0 - u).powf(1.4);
        dry.l[i] *= g;
        dry.r[i] *= g;
    }
    (dry, rise)
}

fn mk_boom(rng: &mut Rng, p: &Params, dur: f32) -> Stereo {
    let tau = (0.7 + 1.4 * p.size).min(dur.max(0.8) * 0.4);
    let b = boom(80.0, 28.0, tau, rng);
    let mut send = Stereo::from_mono(&b);
    send.fit(frames(dur.max(b.len() as f32 / SR + 1.5)));
    let mut room = level::reverb(&send, "cave");
    room.scale(0.6);
    send.add_at(&room, 0, 1.0);
    send
}

/// A ball bouncing: each landing a struck body (a rubber ball's hollow
/// pock, a basketball's boom with the floor's thump, a tennis ball's
/// thock, a ping-pong ball's tick), the gaps and the force shrinking by the
/// ball's bounciness until it rests; dribbling keeps the height.
fn mk_bounce(rng: &mut Rng, p: &Params, dur: f32) -> Vec<f32> {
    const BASKET: &[Mode3] = &[(1.0, 1.0, 0.09), (1.9, 0.45, 0.05), (3.1, 0.2, 0.03)];
    const PINGPONG: &[Mode3] = &[(1.0, 1.0, 0.008), (1.6, 0.5, 0.005)];
    const TENNIS: &[Mode3] = &[(1.0, 1.0, 0.02), (2.3, 0.4, 0.01)];
    const SUPER: &[Mode3] = &[(1.0, 1.0, 0.03), (2.6, 0.4, 0.015)];
    const RUBBER: &[Mode3] = &[(1.0, 1.0, 0.035), (2.2, 0.35, 0.02), (3.7, 0.12, 0.01)];
    let t = &p.text;
    let dribble = t.contains("dribbl");
    // (pitch, modes, bounciness, first gap, how much floor is heard)
    let (hz, modes, e, gap0, floor): (f32, &[Mode3], f32, f32, f32) = if t.contains("basketball") || dribble {
        (rng.range(95.0, 120.0), BASKET, 0.76, 0.62, 1.0)
    } else if t.contains("ping pong") || t.contains("ping-pong") || t.contains("table tennis") {
        (rng.range(2300.0, 2900.0), PINGPONG, 0.88, 0.32, 0.0)
    } else if t.contains("tennis") {
        (rng.range(700.0, 950.0), TENNIS, 0.72, 0.5, 0.3)
    } else if t.contains("super") || t.contains("bouncy") || t.contains("boing") {
        (rng.range(500.0, 700.0), SUPER, 0.9, 0.45, 0.2)
    } else {
        (rng.range(260.0, 380.0) * (1.3 - 0.6 * p.size), RUBBER, 0.78, 0.5, 0.5)
    };
    let n = p.count.unwrap_or(if dribble { 8 } else { 16 }).clamp(1, 60);
    let mut out = vec![0.0; frames(dur.max(1.0))];
    let (mut at, mut gap) = (0.02f32, gap0 * (0.7 + 0.6 * p.size));
    let mut force = 0.6 + 0.4 * p.intensity;
    for k in 0..n {
        if at > dur - 0.05 {
            break;
        }
        // A softer landing flattens the ball less: a touch higher.
        let f = hz * (1.0 + 0.05 * k as f32 / n as f32) * rng.range(0.98, 1.02);
        let mut hit = struck(f, modes, 1.0, 0.4);
        let c = click(rng, (f * 6.0).min(9000.0), 1.0, 0.002);
        mix_into(&mut hit, &c, 0, 0.35);
        if floor > 0.0 {
            let fl = struck(rng.range(140.0, 200.0), &[(1.0, 1.0, 0.05), (2.4, 0.3, 0.03)], 1.0, 0.3);
            mix_into(&mut hit, &fl, 0, 0.4 * floor);
        }
        put(&mut out, &hit, at, force);
        if dribble {
            at += gap0 * rng.range(0.95, 1.05);
            force = (0.6 + 0.4 * p.intensity) * rng.range(0.85, 1.0);
        } else {
            at += gap;
            gap *= e;
            force *= e;
            if gap < 0.02 {
                break;
            }
        }
    }
    out
}

/// A squeezed rubber toy: air forced through its reed, a bright buzzy
/// squeal rising with the squeeze, then a lower one as it fills again.
fn mk_squeak(rng: &mut Rng, p: &Params, dur: f32) -> Vec<f32> {
    let squeezes = p.count.unwrap_or(2).clamp(1, 12);
    let base = rng.range(1300.0, 1900.0) * (1.25 - 0.5 * p.size);
    let len = 0.16 + 0.1 * (1.0 - p.speed);
    let mut out = vec![0.0; frames(dur.max(squeezes as f32 * (len * 1.8 + 0.25)))];
    let mut at = 0.02;
    for _ in 0..squeezes {
        for (k, l) in [(1.0f32, len), (0.72, len * 0.8)] {
            let jitter = rng.range(0.95, 1.05);
            let mut ph = 0.0f32;
            let mut f = Svf::new();
            let n = frames(l);
            let s: Vec<f32> = (0..n)
                .map(|i| {
                    let u = i as f32 / n as f32;
                    let tt = i as f32 / SR;
                    let env = (PI * u).sin().powf(0.6);
                    let hz = base * k * jitter * (0.85 + 0.3 * u) * (1.0 + 0.02 * (TAU * 28.0 * tt).sin());
                    ph += TAU * hz / SR;
                    let w = ph.sin() + 0.5 * (2.0 * ph).sin() + 0.25 * (3.0 * ph).sin();
                    (f.at(w + rng.signed() * 0.15, hz * 2.0, 1.5).bp * 0.6 + w * 0.5) * env
                })
                .collect();
            put(&mut out, &s, at, 0.8);
            at += l + 0.03;
        }
        at += 0.12 + 0.2 * (1.0 - p.speed);
    }
    out
}

/// Pops: a balloon bursting (a sharp crack of noise on a low thump and a
/// short room), a cork drawn (a hollow falling "thoop", then fizz), bubble
/// wrap or popcorn (many small pops at random), or one round pop.
fn mk_pop(rng: &mut Rng, p: &Params, dur: f32) -> Vec<f32> {
    let t = &p.text;
    let mut out = vec![0.0; frames(dur.max(0.6))];
    let glide = |rng: &mut Rng, from: f32, to: f32, len: f32, noise: f32| -> Vec<f32> {
        let mut ph = 0.0f32;
        (0..frames(len)).map(|i| {
            let u = i as f32 / SR;
            ph += TAU * (to + (from - to) * (-u / (len * 0.3)).exp()) / SR;
            (ph.sin() + noise * rng.signed()) * smooth(u, 0.001) * (-u / (len * 0.35)).exp()
        }).collect()
    };
    if t.contains("balloon") || t.contains("burst") {
        let n = frames(0.3);
        let mut f = Svf::new();
        let crack: Vec<f32> = (0..n).map(|i| {
            let u = i as f32 / SR;
            f.at(rng.signed(), 2500.0, 0.5).hp * (-u / 0.01).exp() * 1.4 + rng.signed() * 0.2 * (-u / 0.08).exp()
        }).collect();
        put(&mut out, &crack, 0.0, 1.0);
        let thump = struck(rng.range(70.0, 95.0) * (1.4 - 0.6 * p.size), &[(1.0, 1.0, 0.06)], 1.0, 0.3);
        put(&mut out, &thump, 0.0, 0.6);
    } else if t.contains("cork") || t.contains("champagne") || t.contains("bottle") {
        let thoop = glide(rng, 700.0, 260.0, 0.09, 0.15);
        put(&mut out, &thoop, 0.0, 1.0);
        // Fizz: bubbles thinning out.
        let mut tt = 0.08f32;
        while tt < dur.max(1.5) - 0.05 {
            let hz = rng.range(2500.0, 6000.0);
            let b = bubble(rng, hz, 0.004);
            put(&mut out, &b, tt, 0.15 * (-(tt - 0.08) / 0.8).exp());
            tt += rng.range(0.004, 0.03) * (1.0 + 4.0 * tt);
        }
    } else if t.contains("bubble") || t.contains("popcorn") {
        let corn = t.contains("popcorn");
        let n = p.count.unwrap_or(if corn { 28 } else { 12 }).clamp(1, 120);
        let span = dur.max(1.0) - 0.15;
        for k in 0..n {
            // Popcorn starts slowly, crowds the middle, thins at the end.
            let x = if corn { 0.5 + 0.5 * (PI * (rng.unit() - 0.5)).sin() } else { (k as f32 + rng.unit()) / n as f32 };
            let hz = if corn { rng.range(600.0, 1500.0) } else { rng.range(1500.0, 4000.0) };
            let mut c = click(rng, hz, 0.9, 0.0015);
            let body = struck(hz * 0.25, &[(1.0, 1.0, if corn { 0.02 } else { 0.008 })], 1.0, 0.06);
            mix_into(&mut c, &body, 0, if corn { 0.8 } else { 0.4 });
            put(&mut out, &c, 0.05 + x * span, rng.range(0.5, 1.0));
        }
    } else {
        let pop = glide(rng, 900.0 * (1.3 - 0.6 * p.size), 200.0, 0.08, 0.05);
        put(&mut out, &pop, 0.0, 1.0);
        let c = click(rng, 3000.0, 1.0, 0.0015);
        put(&mut out, &c, 0.0, 0.4);
    }
    out
}

fn mk_ui(rng: &mut Rng, p: &Params) -> Vec<f32> {
    let t = &p.text;
    let shift = rng.range(0.94, 1.08);
    let tone = |hz: f32, len: f32, wave: u8| -> Vec<f32> {
        let mut ph = 0.0f32;
        (0..frames(len)).map(|i| {
            let u = i as f32 / SR;
            ph += TAU * hz * shift / SR;
            let s = match wave {
                0 => ph.sin(),
                1 => ph.sin() + 0.35 * (2.0 * ph).sin() + 0.12 * (3.0 * ph).sin(),
                _ => ph.sin().signum() * 0.4 + ph.sin() * 0.3,
            };
            s * smooth(u, 0.004) * (-u / (len * 0.4)).exp()
        }).collect()
    };
    let mut out = vec![0.0; frames(1.2)];
    if t.contains("error") || t.contains("alert") || t.contains("buzz") {
        put(&mut out, &tone(180.0, 0.18, 2), 0.0, 0.8);
        put(&mut out, &tone(160.0, 0.22, 2), 0.2, 0.8);
    } else if t.contains("success") || t.contains("confirm") {
        for (k, f) in [660.0f32, 830.0, 990.0].iter().enumerate() {
            put(&mut out, &tone(*f, 0.25, 1), k as f32 * 0.09, 0.6);
        }
    } else if t.contains("notification") || t.contains("chime") {
        put(&mut out, &tone(880.0, 0.4, 1), 0.0, 0.7);
        put(&mut out, &struck(1320.0 * shift, &[(1.0, 1.0, 0.5), (2.0, 0.2, 0.3)], 1.0, 0.9), 0.14, 0.6);
    } else if t.contains("pop") || t.contains("bloop") || t.contains("boop") {
        let mut ph = 0.0f32;
        let pop: Vec<f32> = (0..frames(0.12)).map(|i| {
            let u = i as f32 / SR;
            ph += TAU * (120.0 + 520.0 * (-u / 0.02).exp()) * shift / SR;
            ph.sin() * (-u / 0.03).exp()
        }).collect();
        put(&mut out, &pop, 0.0, 1.0);
    } else if t.contains("ding") || t.contains("ping") {
        put(&mut out, &struck(1760.0 * shift, &[(1.0, 1.0, 0.6), (2.76, 0.3, 0.3), (5.4, 0.1, 0.12)], 1.0, 1.1), 0.0, 0.8);
    } else if t.contains("blip") || t.contains("bleep") {
        put(&mut out, &tone(1200.0, 0.06, 2), 0.0, 0.7);
        put(&mut out, &tone(1700.0, 0.08, 2), 0.08, 0.7);
    } else if t.contains("click") || t.contains("tap") || t.contains("select") || t.contains("hover") {
        // A crisp interface tick: a short bright click on a tiny body.
        let tick = click(rng, 3800.0 * shift, 1.4, 0.0012);
        put(&mut out, &tick, 0.0, 1.0);
        put(&mut out, &tone(if t.contains("hover") { 1500.0 } else { 2400.0 }, 0.025, 0), 0.0, 0.35);
        out.truncate(frames(0.15));
    } else {
        put(&mut out, &tone(1000.0, 0.15, 0), 0.0, 0.8);
    }
    out
}

fn mk_steps(rng: &mut Rng, p: &Params, dur: f32, seed: u64) -> Stereo {
    // Four feet fall half as far apart again as two.
    let four = p.walker.is_some_and(|w| w.1);
    let pace = (1.1 + 2.6 * p.speed).clamp(0.9, 4.2) * if four { 1.4 } else { 1.0 };
    let steps = p.count.unwrap_or(((dur - 0.3) * pace).max(2.0) as usize);
    let w = Walk {
        surface: p.surface.unwrap_or(Surface::Dirt),
        shoe: if p.walker.is_some() { Shoe::Paw } else { p.shoe.unwrap_or(if p.size > 0.7 { Shoe::Boot } else { Shoe::Sneaker }) },
        steps,
        pace,
        weight: p.walker.map_or(0.25 + 0.7 * p.size, |w| w.0),
        force: (0.55 + 0.4 * p.speed) * if p.intensity < 0.5 { 0.8 } else { 1.0 },
        four_feet: four,
    };
    let (s, _) = walk(&w, seed ^ rng.u64());
    s
}

/// An animal hopping: a landing every hop (fore feet, then hind feet), a
/// little irregular, softer when the words say so. Without an animal named
/// the hopper is rabbit-sized.
fn mk_hops(rng: &mut Rng, p: &Params, dur: f32) -> Stereo {
    let weight = p.walker.map_or(0.15 + 0.3 * (p.size - 0.5).max(0.0), |w| w.0);
    let rate = 1.4 + 1.6 * p.speed;
    let surface = p.surface.unwrap_or(Surface::Grass);
    let n = p.count.unwrap_or(((dur - 0.3) * rate).max(1.0) as usize).clamp(1, 200);
    let mut out = Stereo::silent(frames(dur.max(n as f32 / rate + 0.5)));
    let mut t = rng.range(0.03, 0.1);
    for i in 0..n {
        let force = (0.45 + 0.5 * p.intensity) * rng.range(0.8, 1.0);
        let l = landing(surface, weight, force, rng);
        out.add_mono(&l, frames(t), 1.0, if i % 2 == 0 { -0.08 } else { 0.08 });
        t += rng.range(0.85, 1.2) / rate;
    }
    out
}

fn mk_thunder(rng: &mut Rng, p: &Params, dur: f32) -> Stereo {
    let secs = dur.max(3.0);
    let n = frames(secs);
    let far = p.far(0.5);
    let mut st = Stereo::silent(n);
    let (mut a, mut b) = (Svf::new(), Svf::new());
    let mut w = Wander::new(rng.u64(), 0.4);
    let mono: Vec<f32> = (0..n).map(|i| {
        let t = i as f32 / SR;
        let x = rng.signed();
        let crack = (-t / 0.05).exp() * a.at(x, 2500.0, 0.5).hp * (1.0 - far);
        let roll = (smooth(t - 0.05 - 0.3 * far, 0.5 + far) * (-(t - 0.4) / (secs * 0.35)).exp().min(1.0)).max(0.0);
        let rumble = b.at(x, 70.0 + 100.0 * w.tick(), 0.8).lp * 3.0 * roll * (0.5 + 0.5 * (t * 3.0 + w.tick() * 6.0).sin().abs());
        crack + rumble
    }).collect();
    st.add_mono(&mono, 0, 1.0, 0.0);
    let mut room = level::reverb(&st, "warehouse");
    room.scale(0.5);
    st.add_at(&room, 0, 1.0);
    st
}

fn mk_explosion(rng: &mut Rng, p: &Params, dur: f32) -> Stereo {
    let secs = dur.max(2.0);
    let n = frames(secs);
    let mut st = Stereo::silent(n);
    let big = 0.4 + 0.6 * p.size;
    let (mut a, mut b) = (Svf::new(), Svf::new());
    let mono: Vec<f32> = (0..n).map(|i| {
        let t = i as f32 / SR;
        let x = rng.signed();
        let blast = (-t / (0.05 + 0.08 * big)).exp() * a.at(x, 1800.0, 0.4).lp * 1.2;
        let debris = (-t / (secs * 0.3)).exp() * smooth(t - 0.1, 0.3) * b.at(x, 400.0, 0.7).lp * 0.8;
        blast + debris
    }).collect();
    st.add_mono(&mono, 0, 1.0, 0.0);
    let bm = boom(90.0, 26.0, 0.9 * big + 0.4, rng);
    st.add_mono(&bm, 0, 1.1, 0.0);
    for _ in 0..(12.0 * big) as usize {
        let c = struck(rng.range(300.0, 2500.0), &[(1.0, 1.0, 0.02), (2.7, 0.4, 0.012)], 1.0, 0.1);
        st.add_mono(&c, frames(rng.range(0.2, secs * 0.7)), rng.range(0.05, 0.3), rng.range(-0.9, 0.9));
    }
    let mut room = level::reverb(&st, "outdoor");
    room.scale(0.5);
    st.add_at(&room, 0, 1.0);
    st
}

fn mk_gunshot(rng: &mut Rng, p: &Params, dur: f32) -> Stereo {
    let n = frames(dur.max(1.4));
    let mut st = Stereo::silent(n);
    let (mut a, mut b) = (Svf::new(), Svf::new());
    let mono: Vec<f32> = (0..n.min(frames(0.5))).map(|i| {
        let t = i as f32 / SR;
        let x = rng.signed();
        a.at(x, 3500.0, 0.4).hp * (-t / 0.006).exp() * 1.5 + b.at(x, 500.0, 0.5).bp * (-t / 0.04).exp() * 1.2
    }).collect();
    st.add_mono(&mono, 0, 1.0, 0.0);
    let thump = struck(110.0 - 40.0 * p.size, &[(1.0, 1.0, 0.07)], 1.0, 0.3);
    st.add_mono(&thump, 0, 0.7, 0.0);
    let mut room = level::reverb(&st, if p.text.contains("indoor") || p.text.contains("room") { "hall" } else { "outdoor" });
    room.scale(0.9);
    st.add_at(&room, 0, 1.0);
    st
}

fn mk_bell(rng: &mut Rng, p: &Params, dur: f32) -> Vec<f32> {
    let big = p.size;
    let hz = 150.0 * (6.0f32).powf(1.0 - big) * rng.range(0.97, 1.03);
    let parts: &[Mode3] = &[(0.5, 0.7, 6.0), (1.0, 1.0, 5.0), (1.19, 0.6, 3.5), (1.5, 0.5, 3.0), (2.0, 0.7, 2.5), (2.51, 0.35, 1.8), (3.0, 0.3, 1.4), (4.2, 0.2, 0.9)];
    let strikes = if p.text.contains("toll") || p.text.contains("tolling") { p.count.unwrap_or(3) } else { p.count.unwrap_or(1) };
    let one = struck(hz, parts, 0.4 + 1.2 * big, (1.5 + 5.0 * big).min(8.0));
    let gap = 1.5 + 2.0 * big;
    let mut out = vec![0.0; frames(dur.max(gap * strikes as f32 + one.len() as f32 / SR))];
    for k in 0..strikes {
        let c = click(rng, 1800.0, 1.0, 0.004);
        put(&mut out, &one, k as f32 * gap, 0.9);
        put(&mut out, &c, k as f32 * gap, 0.2);
    }
    out
}

fn mk_siren(rng: &mut Rng, p: &Params, stereo: &mut Stereo) {
    let n = stereo.len();
    let yelp = p.speed > 0.7;
    let mut saw = Saw::at(0.0);
    let mut flt = Svf::new();
    let period = if yelp { 0.35 } else { 4.0 } * rng.range(0.95, 1.05);
    let mono: Vec<f32> = (0..n).map(|i| {
        let t = i as f32 / SR;
        let u = (0.5 - 0.5 * (TAU * t / period).cos()).powf(1.3);
        let hz = 650.0 + 700.0 * u;
        flt.at(saw.tick(hz), 2500.0, 0.8).lp * 0.5
    }).collect();
    stereo.add_mono(&mono, 0, 1.0, 0.0);
    let mut room = level::reverb(stereo, "outdoor");
    room.scale(0.6);
    stereo.add_at(&room, 0, 1.0);
}

fn mk_zap(rng: &mut Rng, p: &Params, dur: f32) -> Vec<f32> {
    let t = &p.text;
    let down = t.contains("down");
    let up = t.contains("power up") || t.contains("power-up") || t.contains("powerup") || t.contains("energy");
    let len = (if up || down { 0.9f32 } else { 0.35f32 }).max(dur.min(2.0));
    let mut ph = 0.0f32;
    let mut mod_ph = 0.0f32;
    let shift = rng.range(0.9, 1.15);
    (0..frames(len)).map(|i| {
        let u = i as f32 / SR;
        let k = u / len;
        let f = if up { 200.0 * (12.0f32).powf(k) } else if down { 2200.0 * (0.06f32).powf(k) } else { 3200.0 * (0.05f32).powf(k) } * shift;
        mod_ph += TAU * f * 1.5 / SR;
        ph += TAU * f / SR + 0.6 * mod_ph.sin() * f / SR * 2.0;
        let env = if up { smooth(u, 0.05) * smooth(len - u, 0.1) } else { smooth(u, 0.003) * (-u / (len * 0.35)).exp() };
        (ph.sin() * 0.7 + (2.0 * ph).sin() * 0.2) * env
    }).collect()
}

fn mk_glass(rng: &mut Rng, p: &Params, dur: f32) -> Vec<f32> {
    let t = &p.text;
    let mut out = vec![0.0; frames(dur.max(1.0))];
    if t.contains("shatter") || t.contains("break") || t.contains("smash") {
        let pp = Params { size: 0.2, material: Some(2), ..p.clone() };
        let main = impact(rng, &pp, 2, 1.0);
        put(&mut out, &main, 0.0, 0.9);
        let mut burst = Svf::new();
        let crash: Vec<f32> = (0..frames(0.25)).map(|i| burst.at(rng.signed(), 5200.0, 0.5).hp * (-(i as f32 / SR) / 0.05).exp()).collect();
        put(&mut out, &crash, 0.0, 0.8);
        for _ in 0..(25 + (p.size * 40.0) as usize) {
            let pp = Params { size: 0.05, material: Some(2), ..p.clone() };
            let _r18 = rng.range(0.1, 0.5);
            let bit = impact(rng, &pp, 2, _r18);
            put(&mut out, &bit, rng.range(0.02, 0.9), rng.range(0.15, 0.6));
        }
    } else {
        // A clink: two glasses meeting.
        let n = p.count.unwrap_or(1).clamp(1, 8);
        let mut at = 0.02;
        for _ in 0..n {
            let hz = rng.range(2200.0, 3600.0);
            let c = struck(hz, &[(1.0, 1.0, 0.35), (2.32, 0.5, 0.2), (4.25, 0.3, 0.1)], 1.0, 0.9);
            let tap = click(rng, 6500.0, 1.4, 0.0015);
            put(&mut out, &c, at, 0.8);
            put(&mut out, &tap, at, 0.6);
            at += rng.range(0.12, 0.35);
        }
    }
    out
}

fn mk_paper_cloth(rng: &mut Rng, cloth: bool, stereo: &mut Stereo, dens: f32) {
    let n = stereo.len();
    let secs = n as f32 / SR;
    let mut t = 0.0f32;
    let mut w = Wander::new(rng.u64(), 0.3);
    while t < secs {
        let k = w.tick();
        t += -(rng.unit().max(1e-3)).ln() / (30.0 + 160.0 * dens * k * k + 3.0);
        let hz = if cloth { rng.range(700.0, 3000.0) } else { rng.range(2500.0, 9000.0) };
        let _r19 = rng.range(0.6, 1.4);
        let _r20 = rng.range(0.01, 0.03);
        let _r21 = rng.range(0.002, 0.01);
        let c = click(rng, hz, _r19, if cloth { _r20 } else { _r21 });
        stereo.add_mono(&c, frames(t), rng.range(0.1, 0.6) * (0.3 + k), rng.range(-0.7, 0.7));
    }
}

fn mk_sizzle(rng: &mut Rng, stereo: &mut Stereo) {
    let n = stereo.len();
    let (mut a, mut b) = (Svf::new(), Svf::new());
    let mut w = Wander::new(rng.u64(), 0.7);
    for i in 0..n {
        let k = w.tick();
        let (x, y) = (rng.signed(), rng.signed());
        stereo.l[i] += a.at(x, 7000.0, 0.7).hp * (0.12 + 0.08 * k);
        stereo.r[i] += b.at(y, 7000.0, 0.7).hp * (0.12 + 0.08 * k);
    }
    let secs = n as f32 / SR;
    let mut t = 0.0f32;
    while t < secs {
        t += -(rng.unit().max(1e-3)).ln() / 70.0;
        let _r22 = rng.range(3000.0, 9000.0);
        let _r23 = rng.range(0.001, 0.005);
        let c = click(rng, _r22, 1.4, _r23);
        stereo.add_mono(&c, frames(t), rng.unit().powi(2) * 0.8, rng.range(-0.9, 0.9));
    }
}

// ----- putting a description together ------------------------------------------

/// How long a kind runs by default when it is the main sound.
fn natural(kind: Kind, p: &Params) -> f32 {
    match kind {
        Kind::Match => 2.4,
        Kind::Lighter => 1.4,
        Kind::Ignite => 2.0,
        Kind::Fire | Kind::Pour | Kind::Bubbles | Kind::Rain | Kind::Wind | Kind::Murmur | Kind::Cafe | Kind::Machine | Kind::Traffic | Kind::Engine | Kind::Sizzle | Kind::Siren | Kind::Breath | Kind::Clock | Kind::Heartbeat | Kind::Applause => 6.0,
        Kind::Drip => 4.0,
        Kind::Splash => 1.6,
        Kind::Impact => 1.2,
        Kind::Knock => 1.2,
        Kind::Scrape => 1.2,
        Kind::Creak => 2.2,
        Kind::Passby => 5.0,
        Kind::Horn => 1.0,
        Kind::Hooves => 3.0,
        Kind::Click => 0.3,
        Kind::Lock => 1.0,
        Kind::Door => 1.4,
        Kind::Typing => 3.0,
        Kind::Servo => 1.2,
        Kind::Camera => 0.5,
        Kind::Phone => 4.5,
        Kind::Whoosh => 0.9,
        Kind::Riser => 4.0,
        Kind::Boom => 3.0,
        Kind::Ui => 0.8,
        Kind::Steps => p.count.map_or(5.0, |c| c as f32 / (1.1 + 2.6 * p.speed) + 0.6),
        Kind::Hops => p.count.map_or(4.0, |c| c as f32 / (1.4 + 1.6 * p.speed) + 0.5),
        Kind::Animal => 1.5,
        Kind::Thunder => 5.0,
        Kind::Explosion => 3.0,
        Kind::Gunshot => 1.6,
        Kind::Bell => 5.0,
        Kind::Zap => 0.5,
        Kind::Glass => 1.2,
        Kind::Paper | Kind::Cloth => 1.8,
        Kind::Bounce => if p.text.contains("dribbl") { 4.0 } else { 3.0 },
        Kind::Squeak => 0.4 + 0.55 * p.count.unwrap_or(2) as f32,
        Kind::Pop => if p.text.contains("popcorn") { 5.0 } else if p.text.contains("bubble") { 3.0 } else if p.text.contains("cork") || p.text.contains("champagne") || p.text.contains("bottle") { 2.2 } else { 1.0 },
    }
}

/// Sounds that happen once and keep their own length however long the passage is.
fn one_shot(kind: Kind) -> bool {
    matches!(kind, Kind::Impact | Kind::Click | Kind::Lock | Kind::Door | Kind::Camera | Kind::Ui | Kind::Glass | Kind::Gunshot | Kind::Splash | Kind::Match | Kind::Lighter | Kind::Ignite | Kind::Horn | Kind::Creak | Kind::Scrape | Kind::Paper | Kind::Cloth | Kind::Bounce | Kind::Squeak | Kind::Pop)
}

struct Layer {
    kind: Kind,
    bed: bool,
    p: Params,
    info: String,
}

/// The sounds a description asks for, in the order they are mentioned.
fn layers_of(desc: &str) -> Vec<Layer> {
    let lower = desc.to_lowercase().replace(['\u{2019}', '\''], "").replace(['.', ';', '!', '?', '(', ')', '"'], " ");
    let padded = format!(" {} ", lower.replace(',', " , "));
    let mut hits: Vec<(usize, Kind, String)> = Vec::new();
    // Animals first: they have their own vocabulary.
    if let Some((sp, calls)) = animal::find_calls(&lower) {
        // Where does the species word sit?
        let pos = sp.words.iter().filter_map(|w| padded.find(&format!(" {w} ")).or_else(|| padded.find(&format!(" {w}s "))).or_else(|| padded.find(&format!(" {w}ing ")))).min().unwrap_or(0);
        hits.push((pos, Kind::Animal, format!("{} {}", sp.name, calls.iter().map(|c| c.name).collect::<Vec<_>>().join(" and "))));
    }
    for m in MODELS.iter().filter(|m| m.kind != Kind::Animal) {
        let mut best: Option<usize> = None;
        for w in m.words {
            let pats = [format!(" {w} "), format!(" {w}s "), format!(" {w}ing "), format!(" {w}es "), format!(" {w}ed ")];
            for pat in pats {
                if let Some(pos) = padded.find(&pat) {
                    if best.is_none_or(|b| pos < b) {
                        best = Some(pos);
                    }
                }
            }
        }
        if let Some(pos) = best {
            hits.push((pos, m.kind, m.name.to_owned()));
        }
    }
    // A hit inside another's longer phrase is the other's: "door knock" is a knock on a door,
    // "water pouring" is one sound, "car horn" is a horn not an engine.
    let has = |k: Kind| hits.iter().any(|h| h.1 == k);
    let mut drop: Vec<Kind> = Vec::new();
    if has(Kind::Knock) {
        drop.push(Kind::Door);
        drop.push(Kind::Impact);
    }
    if has(Kind::Horn) {
        drop.extend([Kind::Engine, Kind::Traffic]);
    }
    if has(Kind::Match) {
        drop.extend([Kind::Fire, Kind::Impact, Kind::Ignite, Kind::Lighter, Kind::Scrape]);
    }
    if has(Kind::Lighter) {
        drop.extend([Kind::Fire, Kind::Ignite, Kind::Click]);
    }
    if has(Kind::Passby) {
        drop.extend([Kind::Engine, Kind::Traffic, Kind::Whoosh]);
    }
    if has(Kind::Cafe) {
        drop.push(Kind::Murmur);
    }
    if has(Kind::Steps) {
        drop.extend([Kind::Impact, Kind::Hooves]);
    }
    if has(Kind::Hops) {
        // "Soft thumps" of hopping are the landings themselves.
        drop.extend([Kind::Impact, Kind::Steps, Kind::Bounce]);
    }
    // An animal that walks or hops and is not said to call is the walker.
    let walker = (has(Kind::Steps) || has(Kind::Hops)) && has(Kind::Animal) && !animal::names_a_call(&lower);
    let walker_sp = if walker { animal::find(&lower).map(|(sp, _)| sp) } else { None };
    let walker_body = walker_sp.map(animal::body);
    if walker {
        drop.push(Kind::Animal);
    }
    if has(Kind::Hooves) {
        drop.extend([Kind::Steps, Kind::Impact]);
    }
    if has(Kind::Glass) {
        drop.extend([Kind::Impact, Kind::Bell]);
    }
    if has(Kind::Riser) {
        drop.extend([Kind::Boom, Kind::Whoosh, Kind::Impact]);
    }
    if has(Kind::Boom) && !has(Kind::Riser) {
        drop.extend([Kind::Impact]);
    }
    if has(Kind::Typing) {
        drop.push(Kind::Click);
    }
    if has(Kind::Door) {
        drop.extend([Kind::Impact, Kind::Click]);
    }
    if has(Kind::Lock) {
        drop.extend([Kind::Click, Kind::Door]);
    }
    if has(Kind::Gunshot) {
        drop.extend([Kind::Impact]);
    }
    if has(Kind::Explosion) {
        drop.extend([Kind::Boom, Kind::Impact, Kind::Thunder]);
    }
    if has(Kind::Thunder) {
        drop.extend([Kind::Boom]);
    }
    if has(Kind::Rain) {
        drop.extend([Kind::Pour, Kind::Wind]);
        if lower.contains("wind") {
            drop.retain(|k| *k != Kind::Wind);
        }
    }
    if has(Kind::Fire) && has(Kind::Sizzle) {
        drop.push(Kind::Fire);
    }
    if has(Kind::Splash) {
        drop.extend([Kind::Pour, Kind::Impact]);
    }
    if has(Kind::Drip) {
        drop.extend([Kind::Pour, Kind::Splash]);
    }
    if has(Kind::Whoosh) {
        drop.push(Kind::Impact);
    }
    if has(Kind::Machine) && has(Kind::Servo) {
        drop.push(Kind::Machine);
    }
    if has(Kind::Bounce) {
        drop.extend([Kind::Impact, Kind::Steps]);
    }
    if has(Kind::Squeak) {
        drop.extend([Kind::Scrape, Kind::Animal]);
    }
    if has(Kind::Pop) {
        drop.extend([Kind::Ui, Kind::Impact, Kind::Explosion, Kind::Glass]);
    }
    if has(Kind::Ui) && ["ui", "interface", "menu", "app", "digital", "mouse", "hover", "select"].iter().any(|w| has_word(&lower, w)) {
        drop.push(Kind::Click);
    }
    hits.retain(|h| !drop.contains(&h.1));
    hits.sort_by_key(|h| h.0);
    // Parameters from the words around each hit.
    hits.into_iter()
        .map(|(pos, kind, info)| {
            let before = padded[..pos].split_whitespace().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" ");
            let after = padded[pos..].split_whitespace().take(6).collect::<Vec<_>>().join(" ");
            let window = format!("{before} {after}");
            let bed = MODELS.iter().find(|m| m.kind == kind).is_some_and(|m| m.bed);
            let mut p = Params::read(&window, &lower);
            let mut info = info;
            if matches!(kind, Kind::Steps | Kind::Hops) {
                p.walker = walker_body;
                if let Some(sp) = walker_sp {
                    info = format!("{} {}", sp.name, if kind == Kind::Hops { "hops" } else { "pawsteps" });
                }
            }
            Layer { kind, bed, p, info }
        })
        .collect()
}

fn build(layer: &Layer, dur: f32, seed: u64) -> (Stereo, Option<f32>) {
    let mut rng = Rng::new(seed ^ (layer.kind as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let p = &layer.p;
    let n = frames(dur);
    let mut st = Stereo::silent(n);
    let mut anchor = None;
    match layer.kind {
        Kind::Impact => st = Stereo::from_mono(&mk_impact(&mut rng, p, dur)),
        Kind::Knock => st = Stereo::from_mono(&mk_knock(&mut rng, p, dur)),
        Kind::Scrape => st = Stereo::from_mono(&mk_scrape(&mut rng, p, dur.max(0.3))),
        Kind::Creak => st = Stereo::from_mono(&mk_creak(&mut rng, p, dur.max(0.5))),
        Kind::Match => {
            let (m, a) = mk_match(&mut rng, p, dur);
            st = Stereo::from_mono(&m);
            anchor = Some(a);
        }
        Kind::Lighter => {
            let (m, a) = mk_lighter(&mut rng, dur);
            st = Stereo::from_mono(&m);
            anchor = Some(a);
        }
        Kind::Ignite => {
            let (m, a) = mk_ignite(&mut rng, p, dur);
            st = Stereo::from_mono(&m);
            anchor = Some(a);
        }
        Kind::Fire => mk_fire(&mut rng, p, dur, &mut st),
        Kind::Pour => mk_pour(&mut rng, p, &mut st),
        Kind::Drip => st = Stereo::from_mono(&mk_drip(&mut rng, p, dur)),
        Kind::Splash => mk_splash(&mut rng, p, dur, &mut st),
        Kind::Bubbles => mk_bubbles(&mut rng, p, &mut st),
        Kind::Rain => mk_rain(&mut rng, p, &mut st),
        Kind::Wind => mk_wind(&mut rng, p, &mut st),
        Kind::Engine => st = Stereo::from_mono(&mk_engine(&mut rng, p, dur)),
        Kind::Passby => {
            let len = dur.min(7.0).max(2.0);
            mk_passby(&mut rng, p, dur, &mut st, (dur - len) * 0.5, len, p.far(0.15));
        }
        Kind::Traffic => {
            let far = p.far(0.7);
            let secs = n as f32 / SR;
            // The hum of the city: a low rumble that swells and sags.
            let mut w = Wander::new(rng.u64(), 2.0);
            let (mut a, mut b) = (Lp1::default(), Lp1::default());
            let rumble: Vec<f32> = (0..n).map(|_| (a.tick(rng.signed(), 140.0) * 0.8 + b.tick(rng.signed(), 420.0) * 0.25) * (0.15 + 0.2 * w.tick())).collect();
            st.add_mono(&rumble, 0, 1.0, 0.0);
            let cars = (secs * (0.5 + 0.7 * (1.0 - far * 0.4))).ceil() as usize + 1;
            for _ in 0..cars {
                let len = rng.range(3.0, 6.0);
                let at = rng.range(-1.0, secs - 1.5).max(0.0);
                let pp = Params { speed: rng.range(0.3, 0.8), ..p.clone() };
                let mut one = Stereo::silent(frames(len));
                let _r24 = rng.range(0.6, 1.1);
                mk_passby(&mut rng, &pp, len, &mut one, 0.0, len, (far * _r24).min(1.0));
                st.add_at(&one, frames(at), 0.8);
            }
            if p.period {
                // Horns now and then, and a horse and cart on the stones.
                for _ in 0..(secs / 5.0).ceil() as usize {
                    let mut h = horn_tone(&mut rng, true);
                    distance(&mut h, far);
                    st.add_mono(&h, frames(rng.range(0.0, secs - 1.0).max(0.0)), 0.7, rng.range(-0.7, 0.7));
                }
                if rng.chance(0.8) {
                    let mut hv = mk_hooves(&mut rng, &Params { speed: 0.4, surface: Some(Surface::Stone), ..p.clone() }, (secs * 0.5).min(4.0));
                    distance(&mut hv, far);
                    st.add_mono(&hv, frames(rng.range(0.0, secs * 0.4)), 0.6, rng.range(-0.6, 0.6));
                }
            } else if rng.chance(0.6) {
                let mut h = horn_tone(&mut rng, false);
                distance(&mut h, far);
                st.add_mono(&h, frames(rng.range(0.0, (secs - 1.0).max(0.1))), 0.5, rng.range(-0.7, 0.7));
            }
            for ch in [&mut st.l, &mut st.r] {
                distance(ch, far * 0.8);
            }
        }
        Kind::Horn => {
            let one = horn_tone(&mut rng, p.period);
            let k = p.count.unwrap_or(1).clamp(1, 6);
            let mut out = vec![0.0; frames(dur.max((one.len() as f32 / SR + 0.3) * k as f32))];
            for i in 0..k {
                put(&mut out, &one, i as f32 * (one.len() as f32 / SR + 0.25), 1.0);
            }
            st = Stereo::from_mono(&out);
        }
        Kind::Hooves => st = Stereo::from_mono(&mk_hooves(&mut rng, p, dur)),
        Kind::Murmur => mk_murmur(&mut rng, p, &mut st, false),
        Kind::Cafe => {
            mk_murmur(&mut rng, p, &mut st, true);
            mk_cafe_extras(&mut rng, &mut st);
        }
        Kind::Applause => mk_applause(&mut rng, p, &mut st),
        Kind::Clock => st = Stereo::from_mono(&mk_clock(&mut rng, p, dur)),
        Kind::Click => st = Stereo::from_mono(&mk_click(&mut rng, p)),
        Kind::Lock => st = Stereo::from_mono(&mk_lock(&mut rng, p)),
        Kind::Door => st = Stereo::from_mono(&mk_door(&mut rng, p)),
        Kind::Typing => st = Stereo::from_mono(&mk_typing(&mut rng, p, dur)),
        Kind::Machine => mk_machine(&mut rng, p, &mut st),
        Kind::Servo => st = Stereo::from_mono(&mk_servo(&mut rng, p, dur)),
        Kind::Camera => st = Stereo::from_mono(&mk_camera(&mut rng)),
        Kind::Phone => st = Stereo::from_mono(&mk_phone(&mut rng, p, dur)),
        Kind::Heartbeat => st = Stereo::from_mono(&mk_heartbeat(&mut rng, p, dur)),
        Kind::Breath => mk_breath(&mut rng, p, dur, &mut st),
        Kind::Whoosh => mk_whoosh(&mut rng, p, dur, &mut st),
        Kind::Riser => {
            let tail = if p.text.contains("tail") || p.text.contains("decay") || p.text.contains("ring") || p.text.contains("reverb") { 2.2 } else { 0.8 };
            let rise = (dur - tail).max(0.6);
            let (s, a) = mk_riser(&mut rng, rise, tail);
            st = s;
            anchor = Some(a);
        }
        Kind::Boom => {
            st = mk_boom(&mut rng, p, dur);
            anchor = Some(0.0);
        }
        Kind::Ui => st = Stereo::from_mono(&mk_ui(&mut rng, p)),
        Kind::Steps => st = mk_steps(&mut rng, p, dur, seed),
        Kind::Hops => st = mk_hops(&mut rng, p, dur),
        Kind::Animal => {
            if let Some((sp, calls)) = animal::find_calls(&layer.info) {
                let size = 1.0 / (0.5 + p.size).powf(0.8);
                let one = animal::render(sp, calls[0], seed, if p.size == 0.5 { 1.0 } else { size }, p.intensity);
                // Every call named is heard at least once, in order.
                let k = p.count.unwrap_or(if dur > 2.4 * (one.len() as f32 / SR) { (dur / (one.len() as f32 / SR * 1.5)).floor().max(1.0) as usize } else { 1 }).max(calls.len()).clamp(1, 12);
                let mut out = vec![0.0; frames(dur.max(one.len() as f32 / SR))];
                let mut t = 0.0;
                for i in 0..k {
                    let v = animal::render(sp, calls[i % calls.len()], seed.wrapping_add(i as u64 * 7), size, p.intensity);
                    if frames(t) + v.len() > out.len() {
                        out.resize(frames(t) + v.len(), 0.0);
                    }
                    put(&mut out, &v, t, 1.0 - 0.1 * (i % 3) as f32);
                    t += v.len() as f32 / SR * rng.range(1.15, 1.8);
                }
                st = Stereo::panned(&out, rng.range(-0.3, 0.3));
            }
        }
        Kind::Thunder => st = mk_thunder(&mut rng, p, dur),
        Kind::Explosion => st = mk_explosion(&mut rng, p, dur),
        Kind::Gunshot => st = mk_gunshot(&mut rng, p, dur),
        Kind::Bell => st = Stereo::from_mono(&mk_bell(&mut rng, p, dur)),
        Kind::Siren => mk_siren(&mut rng, p, &mut st),
        Kind::Zap => st = Stereo::from_mono(&mk_zap(&mut rng, p, dur)),
        Kind::Glass => st = Stereo::from_mono(&mk_glass(&mut rng, p, dur)),
        Kind::Paper => mk_paper_cloth(&mut rng, false, &mut st, p.intensity),
        Kind::Cloth => mk_paper_cloth(&mut rng, true, &mut st, p.intensity),
        Kind::Sizzle => mk_sizzle(&mut rng, &mut st),
        Kind::Bounce => st = Stereo::from_mono(&mk_bounce(&mut rng, p, dur)),
        Kind::Squeak => st = Stereo::from_mono(&mk_squeak(&mut rng, p, dur)),
        Kind::Pop => st = Stereo::from_mono(&mk_pop(&mut rng, p, dur)),
    }
    if p.dist > 0.0 && !matches!(layer.kind, Kind::Traffic | Kind::Passby) {
        distance(&mut st.l, p.dist);
        distance(&mut st.r, p.dist);
    }
    (st, anchor)
}

/// Makes the sound a description asks for. `seconds` fixes the length;
/// `variant` makes a different take of the same words.
pub fn make(desc: &str, seconds: Option<f32>, variant: u64) -> Result<Made, String> {
    let desc = desc.trim();
    if desc.is_empty() {
        return Err("describe the sound".into());
    }
    // "then" puts stages in order; inside a stage everything layers.
    let lower = desc.to_lowercase();
    let stages: Vec<&str> = if lower.contains(" then ") || lower.contains(" followed by ") || lower.contains(" before ") {
        let mut out = vec![];
        let mut rest = desc;
        loop {
            let low = rest.to_lowercase();
            let cut = [" then ", " followed by ", " before "].iter().filter_map(|s| low.find(s).map(|i| (i, s.len()))).min();
            match cut {
                Some((i, l)) => {
                    out.push(&rest[..i]);
                    rest = &rest[i + l..];
                }
                None => {
                    out.push(rest);
                    break;
                }
            }
        }
        out
    } else {
        vec![desc]
    };
    let mut layers_by_stage: Vec<Vec<Layer>> = stages.iter().map(|s| layers_of(s)).collect();
    // Qualities said once apply to the whole: "period" and "distant" carry across stages.
    // Nothing matched: a word that is a near spelling of a model's own
    // word (or names its family) gets that model, said so. Anything less
    // (a likeness to words in a model's description, a stop word) would be
    // a different kind of sound, so it is refused with the nearest options.
    let mut stand_in = None;
    if layers_by_stage.iter().all(|l| l.is_empty()) {
        let Some(m) = nearest(desc) else {
            let near = nearest_options(desc, 3);
            let near = if near.is_empty() { String::new() } else { format!(" The nearest models: {}; say it with their words if one is meant.", near.join(", ")) };
            return Err(format!("no sound model matches \"{desc}\" (none is made for it, and a different kind of sound is not made in its place).{near} Models: {}", catalogue()));
        };
        let low = desc.to_lowercase();
        layers_by_stage = vec![vec![Layer { kind: m.kind, bed: m.bed, p: Params::read(&low, &low), info: m.name.to_owned() }]];
        stand_in = Some(m.name);
    }
    layers_by_stage.retain(|l| !l.is_empty());
    let seed = variant.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ desc.bytes().fold(7u64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64));
    // Lengths.
    let stage_nat: Vec<f32> = layers_by_stage.iter().map(|ls| ls.iter().filter(|l| !l.bed).map(|l| natural(l.kind, &l.p)).fold(0.0, f32::max).max(if ls.iter().all(|l| l.bed) { ls.iter().map(|l| natural(l.kind, &l.p)).fold(0.0, f32::max) } else { 0.0 })).collect();
    let total = match seconds {
        Some(s) => s.clamp(0.2, 120.0),
        None => stage_nat.iter().sum::<f32>().clamp(0.2, 120.0),
    };
    let scale = total / stage_nat.iter().sum::<f32>().max(0.01);
    let mut out = Stereo::silent(frames(total));
    let mut names: Vec<String> = Vec::new();
    let mut anchor = None;
    let mut t0 = 0.0f32;
    let mut any_event = false;
    for (si, ls) in layers_by_stage.iter().enumerate() {
        let dur = (stage_nat[si] * scale).max(0.2);
        // Events open the stage slightly staggered; beds fill it.
        let mut stagger = 0.0f32;
        for l in ls {
            let ldur = if l.bed || (seconds.is_some() && !one_shot(l.kind)) { dur } else { natural(l.kind, &l.p).min(dur.max(0.3)).max(0.2) };
            let (mut s, a) = build(l, ldur, seed.wrapping_add((si * 31) as u64));
            let (pk, g): (f32, f32) = if l.bed { (0.28, 0.55) } else { (0.85, 1.0) };
            let p = s.peak().max(1e-6);
            s.scale(pk / p);
            let _ = g;
            let start = t0 + if l.bed { 0.0 } else { stagger };
            if !l.bed {
                any_event = true;
                stagger += 0.0;
            }
            if let (Some(a), None) = (a, anchor) {
                anchor = Some(start + a);
            }
            // Fade beds in and out so they never click.
            if l.bed {
                s.edges(0.15f32.min(ldur * 0.3));
            }
            out.add_at(&s, frames(start), 1.0);
            let name = match l.kind {
                Kind::Steps => format!("{} on {}", if l.p.walker.is_some() { l.info.as_str() } else { "footsteps" }, l.p.surface.map_or("dirt", |s| s.name())),
                Kind::Hops => format!("{} on {}", if l.p.walker.is_some() { l.info.as_str() } else { "hops" }, l.p.surface.map_or("grass", |s| s.name())),
                _ => l.info.clone(),
            };
            if !names.contains(&name) {
                names.push(name);
            }
        }
        t0 += dur;
    }
    // A bed under an event sits lower; a bed alone is a bed.
    let p = out.peak().max(1e-6);
    out.scale((if any_event { 0.85 } else { 0.4 }) / p);
    out.edges(0.01);
    let model = match stand_in {
        Some(n) => format!("{n} (no model is made for \"{desc}\"; this is the nearest: say it with that model's words for more control, or describe what it is made of and how it is struck)"),
        None => names.join(" + "),
    };
    Ok(Made { audio: out, model, anchor })
}

/// How alike two words are, 0..1: the same, one the start of the other,
/// or one or two letters apart.
fn word_likeness(a: &str, b: &str) -> f32 {
    if a == b {
        return 1.0;
    }
    let (la, lb) = (a.chars().count(), b.chars().count());
    if la >= 4 && lb >= 4 && (a.starts_with(b) || b.starts_with(a)) {
        return 0.8;
    }
    if la.min(lb) < 4 || la.abs_diff(lb) > 2 {
        return 0.0;
    }
    // Edit distance, small words only.
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + usize::from(a[i - 1] != b[j - 1]));
        }
        prev = cur;
    }
    match prev[b.len()] {
        1 => 0.7,
        2 if la >= 6 => 0.45,
        _ => 0.0,
    }
}

/// Sound words that name a family more than a model, and the model of
/// that family to stand in.
const FAMILIES: &[(&[&str], &str)] = &[
    (&["hit", "strike", "struck", "smack", "thwack", "wham", "pow", "clonk", "klunk", "donk", "boink", "thock"], "impact"),
    (&["metal", "metallic", "steel", "iron", "anvil", "pipe", "sword", "clash"], "impact"),
    (&["rubber", "spring", "springy", "elastic", "jelly", "boing"], "bounce"),
    (&["toy", "duck", "squish", "squeeze"], "squeak"),
    (&["water", "wet", "liquid", "slosh", "gulp", "glug", "swim"], "splash"),
    (&["air", "swing", "fly", "flying", "throw", "thrown", "fast"], "whoosh"),
    (&["digital", "screen", "game", "computer", "menu", "button", "tap", "coin", "pickup", "level"], "ui"),
    (&["fabric", "clothing", "move", "movement", "rustle", "shuffle"], "cloth"),
    (&["walk", "foot", "feet", "shoe", "shoes", "boot", "stomp", "tread"], "footsteps"),
    (&["glassy", "crystal", "ice", "ceramic", "porcelain", "chink"], "glass"),
    (&["burst", "crackle", "snap", "crack"], "pop"),
    (&["motor", "drill", "saw", "blender", "vacuum", "hairdryer", "washing"], "machine"),
    (&["scary", "horror", "tension", "suspense", "build"], "riser"),
    (&["magic", "sparkle", "spell", "fairy", "shimmer", "twinkle"], "zap"),
];

/// Words that say how or which, never what: they match no model.
const STOP: &[&str] = &[
    "and", "the", "with", "from", "into", "onto", "over", "under", "then", "that", "this", "its", "his", "her", "their", "some", "very", "soft", "softly", "gentle", "gently", "loud", "loudly", "quiet", "quietly", "small", "big", "little", "large", "huge", "tiny", "distant", "close", "slow", "slowly", "fast", "quick", "heavy", "light", "sound", "sounds", "noise", "noises", "like", "kind", "sort", "few", "many", "one", "two", "three", "long", "short", "far", "near", "away", "while", "about",
];

/// The words of a description that could name a sound.
fn content_words(desc: &str) -> Vec<String> {
    desc.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() >= 3 && !STOP.contains(w)).map(str::to_owned).collect()
}

/// How near each model is to the words: by its own words (near spellings
/// count) and the family words above, and, weakly, by the words of its
/// summary and example. `(model, own, said)`: `own` is the best likeness
/// to the model's own or family words, `said` the total with the weak ones.
fn likeness(desc: &str) -> Vec<(&'static ModelInfo, f32, f32)> {
    let words = content_words(desc);
    MODELS
        .iter()
        .filter(|m| m.kind != Kind::Animal)
        .map(|m| {
            let vocab: Vec<&str> = m.words.iter().flat_map(|w| w.split(' ')).chain(m.summary.split(|c: char| !c.is_alphanumeric())).chain(m.example.split(' ')).filter(|w| w.len() >= 3 && !STOP.contains(w)).collect();
            let (mut own_best, mut total) = (0.0f32, 0.0f32);
            for w in &words {
                // One letter apart is a misspelling only in a long word:
                // "snort" is not "snore".
                let own = m.words.iter().flat_map(|x| x.split(' ')).filter(|v| !STOP.contains(v)).map(|v| word_likeness(w, v)).map(|l| if l < 0.8 && w.len() < 6 { 0.0 } else { l }).fold(0.0, f32::max);
                let family = if FAMILIES.iter().any(|(ws, name)| *name == m.name && ws.contains(&w.as_str())) { 0.9 } else { 0.0 };
                let said = vocab.iter().map(|v| word_likeness(w, v) * 0.6).fold(0.0, f32::max);
                own_best = own_best.max(own).max(family);
                total += own.max(family).max(said);
            }
            (m, own_best, total)
        })
        .collect()
}

/// The model a description no model's words match stands for: one of its
/// words is a near spelling of a model's own word, or names its family.
fn nearest(desc: &str) -> Option<&'static ModelInfo> {
    let mut best: Option<(&ModelInfo, f32, f32)> = None;
    for (m, own, total) in likeness(desc) {
        if own >= 0.7 && best.is_none_or(|(_, o, t)| (own, total) > (o, t)) {
            best = Some((m, own, total));
        }
    }
    best.map(|(m, _, _)| m)
}

/// The `n` models with any likeness to the words, nearest first, with an
/// example of how to ask for each.
fn nearest_options(desc: &str, n: usize) -> Vec<String> {
    let mut l: Vec<_> = likeness(desc).into_iter().filter(|x| x.2 > 0.0).collect();
    l.sort_by(|a, b| b.2.total_cmp(&a.2));
    l.into_iter().take(n).map(|(m, _, _)| format!("{} (\"{}\")", m.name, m.example)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn differ(a: &Stereo, b: &Stereo) -> bool {
        if a.len() != b.len() {
            return true;
        }
        let d: f32 = a.l.iter().zip(&b.l).map(|(x, y)| (x - y).abs()).sum();
        d > 1.0
    }

    #[test]
    fn every_model_is_audible_finite_and_varies_per_take() {
        for m in MODELS {
            let a = make(m.example, None, 0).unwrap_or_else(|e| panic!("{}: {e}", m.name));
            let b = make(m.example, None, 1).unwrap();
            assert!(rms(&a.audio.l) > 0.003 && a.audio.peak() > 0.05, "{} ({}) is silent", m.name, m.example);
            assert!(a.audio.l.iter().chain(&a.audio.r).all(|v| v.is_finite()), "{}", m.name);
            assert!(a.audio.peak() <= 0.9, "{}: {}", m.name, a.audio.peak());
            assert!(differ(&a.audio, &b.audio), "{}: takes 0 and 1 are the same", m.name);
        }
    }

    #[test]
    fn the_examples_from_real_stories() {
        for (desc, model) in [
            ("a match strike", "match"),
            ("distant period traffic", "traffic"),
            ("café murmur", "cafe"),
            ("footsteps on gravel", "footsteps on gravel"),
            ("distant period traffic, a match strike, café murmur and footsteps", "traffic"),
            ("synth riser with a tail", "riser"),
            ("pig oinking", "pig oink"),
            ("horse neighing", "horse neigh"),
            ("hen clucking", "hen cluck"),
            ("sheep bleating", "sheep baa"),
            ("dog barking and growling", "dog"),
        ] {
            let made = make(desc, None, 0).unwrap_or_else(|e| panic!("{desc}: {e}"));
            assert!(made.model.contains(model), "{desc}: made {} ", made.model);
            assert!(made.audio.peak() > 0.1, "{desc}");
        }
    }

    #[test]
    fn several_sounds_become_one_passage() {
        let m = make("footsteps on gravel, distant period traffic, café murmur", Some(8.0), 0).unwrap();
        assert!(m.model.contains("gravel") && m.model.contains("traffic") && m.model.contains("cafe"), "{}", m.model);
        assert_eq!(m.audio.len(), frames(8.0));
        // Then puts them in order: the second half differs from the first.
        let seq = make("a door knock then footsteps on gravel", Some(6.0), 0).unwrap();
        let h = seq.audio.len() / 2;
        assert!(rms(&seq.audio.l[..h]) > 0.0 && rms(&seq.audio.l[h..]) > 0.0);
    }

    #[test]
    fn length_distance_count_and_surface_are_honoured() {
        assert_eq!(make("fire crackling", Some(3.5), 0).unwrap().audio.len(), frames(3.5));
        let near = make("traffic", Some(4.0), 3).unwrap();
        let far = make("distant traffic", Some(4.0), 3).unwrap();
        let zc = |x: &[f32]| x.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
        assert!(zc(&far.audio.l) < zc(&near.audio.l), "distance should dull the sound");
        // Three knocks are three events.
        let k = make("three knocks on a door", None, 0).unwrap();
        let env: Vec<f32> = k.audio.l.chunks(480).map(rms).collect();
        let top = env.iter().cloned().fold(0.0, f32::max);
        let mut peaks = 0;
        let mut last = -100i32;
        for i in 1..env.len() {
            if env[i] > 0.35 * top && env[i - 1] <= 0.35 * top && i as i32 - last > 8 {
                peaks += 1;
                last = i as i32;
            }
        }
        assert_eq!(peaks, 3, "three knocks, heard {peaks}");
        let gravel = make("footsteps on gravel", Some(3.0), 0).unwrap();
        let snow = make("footsteps in snow", Some(3.0), 0).unwrap();
        assert!(differ(&gravel.audio, &snow.audio));
    }

    #[test]
    fn a_riser_peaks_where_it_says() {
        let m = make("synth riser with a tail", Some(5.0), 0).unwrap();
        let a = m.anchor.expect("a riser has a peak");
        assert!((a - 5.0 + 2.2).abs() < 0.05, "{a}");
        let e = |from: f32, to: f32| rms(&m.audio.l[frames(from)..frames(to)]);
        assert!(e(a - 0.4, a) > e(0.2, 0.6) * 3.0, "it grows");
        assert!(e(4.8, 5.0) < e(a - 0.4, a) * 0.5, "it falls away in the tail");
    }

    #[test]
    fn unknown_words_say_what_exists() {
        let e = make("a quantum flux capacitor", None, 0).unwrap_err();
        assert!(e.contains("footsteps") && e.contains("animals"), "{e}");
    }

    /// Where a sound's energy sits: (spectral centroid Hz, share of the
    /// energy under 300 Hz).
    fn colour(x: &[f32]) -> (f32, f32) {
        use studio_sound::fft::{Fft, hann};
        let n = 4096;
        let fft = Fft::new(n);
        let mut win = vec![0.0; n];
        hann(&mut win);
        let mut acc = vec![0.0f64; n / 2];
        let mut pos = 0;
        while pos + n <= x.len() {
            let mut re: Vec<f32> = x[pos..pos + n].iter().zip(&win).map(|(a, w)| a * w).collect();
            let mut im = vec![0.0; n];
            fft.forward(&mut re, &mut im);
            for k in 0..n / 2 {
                acc[k] += (re[k] * re[k] + im[k] * im[k]) as f64;
            }
            pos += n / 2;
        }
        let hz = |k: usize| k as f64 * SR as f64 / n as f64;
        let total: f64 = acc.iter().sum::<f64>().max(1e-20);
        let centroid = acc.iter().enumerate().map(|(k, p)| hz(k) * p).sum::<f64>() / total;
        let low = acc.iter().enumerate().filter(|(k, _)| hz(*k) < 300.0).map(|(_, p)| p).sum::<f64>() / total;
        (centroid as f32, low as f32)
    }

    /// How many separate events a sound has (rises of its 10 ms envelope
    /// through a third of its peak, 80 ms apart at least).
    fn events(x: &[f32]) -> usize {
        let env: Vec<f32> = x.chunks(480).map(rms).collect();
        let top = env.iter().cloned().fold(0.0, f32::max);
        let (mut n, mut last) = (0, -100i32);
        for i in 1..env.len() {
            if env[i] > top / 3.0 && env[i - 1] <= top / 3.0 && i as i32 - last >= 8 {
                n += 1;
                last = i as i32;
            }
        }
        n
    }

    /// A bear walking on grass is heavy, padded feet pressing turf: a low
    /// thud under a soft rustle, nothing like a crowd's bright claps.
    #[test]
    fn a_bear_on_grass_is_heavy_and_dull_not_applause() {
        let bear = make("bear footsteps on grass", Some(5.0), 0).unwrap();
        assert!(bear.model.contains("bear pawsteps on grass"), "{}", bear.model);
        let clap = make("applause", Some(5.0), 0).unwrap();
        let (bc, bl) = colour(&bear.audio.l);
        let (ac, _) = colour(&clap.audio.l);
        assert!(bc < 900.0 && bl > 0.3, "bear on grass: centroid {bc:.0} Hz, {:.0}% under 300 Hz (applause {ac:.0} Hz)", bl * 100.0);
        // A person on grass is lighter, but still not a clap.
        let (pc, _) = colour(&make("footsteps on grass", Some(5.0), 0).unwrap().audio.l);
        assert!(pc < 0.6 * ac, "person on grass {pc:.0} Hz, applause {ac:.0} Hz");
        // Heavier is lower.
        let (mc, _) = colour(&make("a mouse scurrying on grass", Some(3.0), 0).unwrap().audio.l);
        assert!(mc > bc * 1.5, "mouse {mc:.0} Hz, bear {bc:.0} Hz");
    }

    /// Rabbits hopping are soft low thumps, one per landing.
    #[test]
    fn rabbits_hopping_are_soft_low_thumps() {
        let m = make("rabbits hopping, soft thumps", Some(4.0), 0).unwrap();
        let (c, low) = colour(&m.audio.l);
        assert!(c < 700.0 && low > 0.4, "{}: centroid {c:.0} Hz, {:.0}% under 300 Hz", m.model, low * 100.0);
        let n = events(&m.audio.l);
        assert!((4..=16).contains(&n), "{}: {n} landings in 4 s", m.model);
    }

    /// An animal no model was made for is made by its voice, from its size
    /// and the call asked for, never by a different class of sound.
    #[test]
    fn a_bear_huffs_and_snorts_as_a_bear() {
        let m = make("a bear's soft huff and snort", None, 0).unwrap();
        assert!(m.model.contains("bear"), "{}", m.model);
        let (c, _) = colour(&m.audio.l);
        assert!(c < 1500.0, "{}: a big animal's breath is low, centroid {c:.0} Hz", m.model);
        let g = make("a bear growling", None, 0).unwrap();
        assert!(g.model.contains("bear growl"), "{}", g.model);
    }

    /// A description that only looks like a model's words by accident
    /// (stop words, words in its summary) is refused with the nearest real
    /// options, not given a sound of another kind.
    #[test]
    fn a_far_likeness_is_refused_with_the_nearest_options() {
        for desc in ["a soft and gentle presence", "the aardvark's tidy burrow", "a moose's snort"] {
            match make(desc, None, 0) {
                Ok(m) => panic!("{desc}: made {}", m.model),
                Err(e) => assert!(e.contains("no sound model matches"), "{desc}: {e}"),
            }
        }
        // What is near is offered, to be asked for in its own words.
        let e = make("a moose's snort", None, 0).map(|m| m.model).unwrap_err();
        assert!(e.contains("nearest models: breath"), "{e}");
    }

    #[test]
    fn a_word_near_a_model_gets_that_model() {
        // A misspelling no model matches outright is near one.
        let m = make("a basketbal", Some(1.0), 0).unwrap();
        assert!(m.model.starts_with("bounce"), "{}", m.model);
    }
}
