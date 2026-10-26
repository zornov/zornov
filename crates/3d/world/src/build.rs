// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Making a world from a description. A [`WorldSpec`] says what is wanted
//! (a forest in morning mist, a city at dusk, a ruined desert outpost, a
//! sci-fi hab on a dune planet, a flat with three rooms) in a handful of
//! fields; a preset fills in what the description leaves out. [`build`]
//! turns it into real geometry: a heightfield sculpted and painted,
//! vegetation and rocks scattered by rules as instances, buildings and
//! rooms from the modular kits, water, lights, a sky for the time of day,
//! props and characters placed on the ground. The spec is also the saved
//! form of the world: editing adds to it and rebuilding gives the same
//! world back, since everything runs from seeds.

use crate::asset::{Asset, ColliderShape, mesh_bounds, mesh_from_studio};
use crate::collide::Solid;
use crate::kit::{self, Built};
use crate::look::{self, LookHints};
use crate::math::{V3, dist_xz};
use crate::noise::Rng;
use crate::terrain::{Brush, Style, Terrain, TerrainSpec};
use crate::world::{Instance, Light, LightKind, ObjectKind, Performance, Sky, Spawn, Water, World};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use studio_mesh::Mesh;

/// A time of day: hours, or a name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Time {
    Hours(f32),
    Name(String),
}

impl Time {
    pub fn hours(&self) -> Result<f32, String> {
        match self {
            Time::Hours(h) => Ok(h.rem_euclid(24.0)),
            Time::Name(s) => Ok(match s.trim().to_lowercase().as_str() {
                "dawn" | "sunrise" => 6.0,
                "morning" => 9.0,
                "noon" | "midday" => 12.5,
                "afternoon" => 15.5,
                "golden" | "golden_hour" | "golden hour" => 18.0,
                "sunset" | "dusk" => 19.0,
                "twilight" | "blue_hour" | "blue hour" => 19.7,
                "night" | "midnight" => 23.0,
                other => return other.parse::<f32>().map(|h| h.rem_euclid(24.0)).map_err(|_| format!("time \"{other}\": hours (0-24) or dawn, morning, noon, afternoon, golden_hour, sunset, twilight, night")),
            }),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerrainJ {
    pub style: String,
    /// Metres of relief.
    pub height: Option<f32>,
    /// Metres across.
    pub size: Option<f32>,
    /// Metres per cell.
    pub cell: Option<f32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SkyJ {
    pub time: Option<Time>,
    pub day: Option<f32>,
    pub latitude: Option<f32>,
    pub heading: Option<f32>,
    pub turbidity: Option<f32>,
    pub strength: Option<f32>,
    pub clouds: Option<f32>,
    pub hdri: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WaterJ {
    /// "sea" (everything below `level`), "lake" (a basin dug at `at`),
    /// "river" (the valley floor).
    pub kind: String,
    /// Surface height in metres (sea), or a fraction of the relief when
    /// `fraction` is set.
    pub level: Option<f32>,
    pub at: Option<[f32; 2]>,
    pub radius: Option<f32>,
    pub depth: Option<f32>,
    pub colour: Option<String>,
    pub waves: Option<f32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SculptJ {
    pub brush: String,
    pub at: [f32; 2],
    pub radius: f32,
    pub strength: f32,
    pub level: Option<f32>,
    /// Repeats of the stroke (a long press).
    pub passes: u32,
}

impl Default for SculptJ {
    fn default() -> SculptJ {
        SculptJ { brush: "raise".into(), at: [0.0, 0.0], radius: 10.0, strength: 1.0, level: None, passes: 1 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaintJ {
    /// A layer by name (grass, rock, dirt, snow; or sand, dune...) or 0-3.
    pub layer: String,
    pub at: [f32; 2],
    pub radius: f32,
    pub strength: f32,
}

impl Default for PaintJ {
    fn default() -> PaintJ {
        PaintJ { layer: "dirt".into(), at: [0.0, 0.0], radius: 8.0, strength: 1.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScatterJ {
    /// pine, tree, palm, bush, rock, boulder, grass, flower, cactus,
    /// dead_tree.
    pub kind: String,
    pub count: usize,
    /// Typical height in metres (0 takes the kind's own).
    pub size: f32,
    pub max_slope: f32,
    /// Fractions of the land's relief.
    pub min_height: f32,
    pub max_height: f32,
    pub clumping: f32,
    /// Keep to this ground layer (grass, rock, dirt, snow...).
    pub on: Option<String>,
    /// Only within `radius` metres of this point.
    pub near: Option<[f32; 2]>,
    pub radius: Option<f32>,
    pub seed: Option<u32>,
}

impl Default for ScatterJ {
    fn default() -> ScatterJ {
        ScatterJ { kind: "tree".into(), count: 100, size: 0.0, max_slope: 32.0, min_height: 0.0, max_height: 1.0, clumping: 0.3, on: None, near: None, radius: None, seed: None }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BuildingJ {
    /// house, apartment, tower, shop, warehouse, room, hall...
    pub kind: String,
    pub at: Option<[f32; 2]>,
    pub yaw: f32,
    pub width: f32,
    pub depth: f32,
    pub floors: usize,
    pub seed: Option<u32>,
    /// Furniture inside, by name (table, chair, sofa, bed, shelf, desk...).
    pub furnish: Vec<String>,
    /// An architectural style (classical, art_deco, brutalist, modern_glass,
    /// medieval, oriental, futuristic, rustic): the building is then made by
    /// the architecture generators of `studio_modeling::places`.
    pub style: Option<String>,
}

impl Default for BuildingJ {
    fn default() -> BuildingJ {
        BuildingJ { kind: "house".into(), at: None, yaw: 0.0, width: 9.0, depth: 7.0, floors: 2, seed: None, furnish: Vec::new(), style: None }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PropJ {
    /// crate, barrel, table, chair, column, stairs, lamp, fence, car,
    /// sofa, bed, shelf, desk, cactus... or a library name.
    pub kind: String,
    pub at: [f32; 2],
    /// Height above the ground (on a table, say).
    pub lift: f32,
    pub yaw: f32,
    pub size: f32,
    pub count: usize,
    /// Scatter `count` copies within this radius.
    pub spread: f32,
    pub seed: Option<u32>,
}

impl Default for PropJ {
    fn default() -> PropJ {
        PropJ { kind: "crate".into(), at: [0.0, 0.0], lift: 0.0, yaw: 0.0, size: 0.0, count: 1, spread: 0.0, seed: None }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CharJ {
    pub name: String,
    pub at: [f32; 2],
    pub yaw: f32,
    pub height: f32,
    /// idle, walk, run, wave, talk, dance...
    pub clip: String,
    /// sRGB top colour.
    pub top: Option<String>,
    pub skin: Option<String>,
    /// Ground points to walk to, in order.
    pub walk_to: Vec<[f32; 2]>,
    /// Metres per second (0 takes the clip's own pace).
    pub pace: f32,
}

impl Default for CharJ {
    fn default() -> CharJ {
        CharJ { name: "Person".into(), at: [0.0, 0.0], yaw: 0.0, height: 1.75, clip: "idle".into(), top: None, skin: None, walk_to: Vec::new(), pace: 0.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LightJ {
    pub kind: String,
    /// x, z (on the ground, lifted by `height`) .
    pub at: [f32; 2],
    pub height: f32,
    pub colour: String,
    /// 1 is an ordinary lamp.
    pub intensity: f32,
    pub range: f32,
    pub aim: Option<[f32; 3]>,
    pub cone: f32,
}

impl Default for LightJ {
    fn default() -> LightJ {
        LightJ { kind: "point".into(), at: [0.0, 0.0], height: 3.0, colour: "#ffd9a0".into(), intensity: 1.0, range: 10.0, aim: None, cone: 35.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RoadJ {
    pub from: [f32; 2],
    pub to: [f32; 2],
    pub width: f32,
}

impl Default for RoadJ {
    fn default() -> RoadJ {
        RoadJ { from: [0.0, 0.0], to: [50.0, 0.0], width: 8.0 }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LookJ {
    pub exposure: Option<f32>,
    pub bloom: Option<f32>,
    pub ao: Option<f32>,
    pub saturation: Option<f32>,
    pub vignette: Option<f32>,
    /// Colour words or #rrggbb that re-dress the geometry.
    pub foliage: Option<String>,
    pub ground: Option<String>,
    pub fog_colour: Option<String>,
    /// What a reference picture said (see [`crate::look::hints_from_rgba`]).
    pub hints: Option<HintsJ>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HintsJ {
    pub sky: V3,
    pub horizon: V3,
    pub ground: V3,
    pub foliage: Option<V3>,
    pub time: f32,
    pub haze: f32,
    pub brightness: f32,
    pub clouds: f32,
}

impl From<&LookHints> for HintsJ {
    fn from(h: &LookHints) -> HintsJ {
        HintsJ { sky: h.sky, horizon: h.horizon, ground: h.ground, foliage: h.foliage, time: h.time, haze: h.haze, brightness: h.brightness, clouds: h.clouds }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorldSpec {
    pub name: String,
    pub description: String,
    /// forest, meadow, desert, mountains, island, canyon, arctic, village,
    /// city, interior, scifi.
    pub preset: String,
    pub seed: Option<u32>,
    /// Metres across (default by preset).
    pub size: Option<f32>,
    /// "high" uses the modeled tree and rock kits; "fast" low-poly ones.
    pub quality: String,
    pub terrain: Option<TerrainJ>,
    pub sky: Option<SkyJ>,
    pub fog: Option<f32>,
    pub water: Option<WaterJ>,
    pub sculpt: Vec<SculptJ>,
    pub paint: Vec<PaintJ>,
    pub scatter: Vec<ScatterJ>,
    pub buildings: Vec<BuildingJ>,
    pub props: Vec<PropJ>,
    pub characters: Vec<CharJ>,
    pub lights: Vec<LightJ>,
    pub roads: Vec<RoadJ>,
    pub spawn: Option<[f32; 3]>,
    pub look: Option<LookJ>,
    /// Parts of the preset to leave out: scatter, buildings, props,
    /// characters, lights, roads, water, sculpt, paint.
    pub without: Vec<String>,
}

impl Default for WorldSpec {
    fn default() -> WorldSpec {
        WorldSpec {
            name: "World".into(),
            description: String::new(),
            preset: String::new(),
            seed: None,
            size: None,
            quality: "high".into(),
            terrain: None,
            sky: None,
            fog: None,
            water: None,
            sculpt: Vec::new(),
            paint: Vec::new(),
            scatter: Vec::new(),
            buildings: Vec::new(),
            props: Vec::new(),
            characters: Vec::new(),
            lights: Vec::new(),
            roads: Vec::new(),
            spawn: None,
            look: None,
            without: Vec::new(),
        }
    }
}

pub const PRESETS: &[(&str, &str)] = &[
    ("forest", "hills thick with pine and broadleaf trees, undergrowth, a stream or pond, morning light and mist"),
    ("meadow", "open rolling grass and wildflowers, a few trees, a pond, afternoon sun"),
    ("desert", "dunes with cacti, dry rock and dead trees, hard light"),
    ("mountains", "high ridges with snow, pines low down, boulders and a lake"),
    ("island", "a beach island ringed by sea, palms and rocks"),
    ("canyon", "layered red rock walls, scrub and boulders"),
    ("arctic", "snow-covered hills, bare trees, cold light"),
    ("village", "a few houses with fences and trees on gentle ground, paths, a pond"),
    ("city", "a street grid with towers, shops and houses, lamps and cars, at dusk"),
    ("interior", "a flat of connected rooms with furniture, lamps and windows"),
    ("scifi", "a sealed hab of glowing halls on a dune planet at night"),
];

/// The preset a free-text description most likely means.
pub fn guess_preset(text: &str) -> &'static str {
    let t = text.to_lowercase();
    let has = |ws: &[&str]| ws.iter().any(|w| t.contains(w));
    if has(&["sci-fi", "scifi", "sci fi", "space station", "spaceship", "alien", "futuristic", "hab ", "colony"]) {
        "scifi"
    } else if has(&["interior", "apartment", "living room", "bedroom", "kitchen", "office interior", "inside a", "indoor", "flat ", "studio apartment", "room"]) && !has(&["forest", "city street"]) {
        "interior"
    } else if has(&["city", "street", "downtown", "urban", "skyscraper", "metropolis", "town square"]) {
        "city"
    } else if has(&["village", "hamlet", "farm", "cottage", "town"]) {
        "village"
    } else if has(&["desert", "dune", "sahara", "arid", "cactus", "wasteland"]) {
        "desert"
    } else if has(&["snow", "arctic", "tundra", "winter", "frozen", "ice"]) {
        "arctic"
    } else if has(&["island", "beach", "tropical", "palm", "coast", "shore", "ocean"]) {
        "island"
    } else if has(&["canyon", "mesa", "badlands", "red rock"]) {
        "canyon"
    } else if has(&["mountain", "alpine", "alps", "peak", "highland"]) {
        "mountains"
    } else if has(&["meadow", "field", "prairie", "grassland", "plain", "pasture"]) {
        "meadow"
    } else {
        "forest"
    }
}

fn time_from_words(text: &str) -> Option<f32> {
    let t = text.to_lowercase();
    let has = |w: &str| t.contains(w);
    Some(if has("midnight") || has("night") || has("moonlit") {
        23.0
    } else if has("golden hour") || has("golden-hour") || has("late afternoon") {
        18.0
    } else if has("sunset") || has("dusk") || has("evening") {
        19.0
    } else if has("twilight") || has("blue hour") {
        19.7
    } else if has("sunrise") || has("dawn") || has("early morning") {
        6.3
    } else if has("morning") {
        9.0
    } else if has("noon") || has("midday") {
        12.5
    } else if has("afternoon") {
        15.5
    } else {
        return None;
    })
}

fn sc(kind: &str, count: usize, size: f32) -> ScatterJ {
    ScatterJ { kind: kind.into(), count, size, ..ScatterJ::default() }
}

/// What a preset puts in, for fields the description leaves empty.
pub fn preset_spec(preset: &str, size: f32, seed: u32) -> WorldSpec {
    let mut s = WorldSpec { preset: preset.into(), ..WorldSpec::default() };
    let area = (size / 400.0).powi(2);
    let n = |c: f32| ((c * area) as usize).max(1);
    let terrain = |style: &str, relief: f32| Some(TerrainJ { style: style.into(), height: Some(relief), size: Some(size), cell: None });
    let sky = |time: f32, turb: f32, clouds: f32| Some(SkyJ { time: Some(Time::Hours(time)), turbidity: Some(turb), clouds: Some(clouds), ..SkyJ::default() });
    match preset {
        "forest" => {
            s.terrain = terrain("hills", 34.0);
            s.sky = sky(9.2, 3.2, 0.3);
            s.fog = Some(0.0042);
            s.water = Some(WaterJ { kind: "lake".into(), at: Some([size * 0.18, size * 0.1]), radius: Some(size * 0.06), depth: Some(3.0), ..WaterJ::default() });
            s.scatter = vec![
                ScatterJ { max_slope: 30.0, clumping: 0.5, on: Some("grass".into()), ..sc("pine", n(700.0), 14.0) },
                ScatterJ { max_slope: 26.0, clumping: 0.5, on: Some("grass".into()), ..sc("tree", n(380.0), 11.0) },
                ScatterJ { clumping: 0.4, ..sc("bush", n(380.0), 1.6) },
                ScatterJ { max_slope: 60.0, ..sc("rock", n(70.0), 1.6) },
                ScatterJ { on: Some("grass".into()), clumping: 0.6, ..sc("grass", n(26000.0), 0.7) },
                ScatterJ { on: Some("grass".into()), clumping: 0.8, ..sc("flower", n(900.0), 0.5) },
            ];
        }
        "meadow" => {
            s.terrain = terrain("plains", 16.0);
            s.sky = sky(15.0, 2.6, 0.35);
            s.fog = Some(0.0014);
            s.water = Some(WaterJ { kind: "lake".into(), at: Some([-size * 0.12, size * 0.2]), radius: Some(size * 0.05), depth: Some(2.0), ..WaterJ::default() });
            s.scatter = vec![
                ScatterJ { on: Some("grass".into()), clumping: 0.5, ..sc("tree", n(36.0), 10.0) },
                ScatterJ { clumping: 0.4, ..sc("bush", n(100.0), 1.4) },
                ScatterJ { on: Some("grass".into()), clumping: 0.3, ..sc("grass", n(52000.0), 0.7) },
                ScatterJ { on: Some("grass".into()), clumping: 0.9, ..sc("flower", n(3000.0), 0.5) },
                sc("rock", n(18.0), 1.2),
            ];
        }
        "desert" => {
            s.terrain = terrain("dunes", 22.0);
            s.sky = sky(15.5, 5.0, 0.05);
            s.fog = Some(0.0016);
            s.scatter = vec![ScatterJ { max_slope: 25.0, ..sc("cactus", n(160.0), 3.0) }, ScatterJ { max_slope: 60.0, ..sc("rock", n(120.0), 2.2) }, sc("dead_tree", n(26.0), 4.0), ScatterJ { clumping: 0.7, ..sc("bush", n(120.0), 0.9) }];
        }
        "mountains" => {
            s.terrain = terrain("mountains", 150.0);
            s.sky = sky(10.5, 2.4, 0.3);
            s.fog = Some(0.0016);
            s.water = Some(WaterJ { kind: "lake".into(), at: Some([size * 0.1, -size * 0.15]), radius: Some(size * 0.05), depth: Some(4.0), ..WaterJ::default() });
            s.scatter = vec![ScatterJ { max_slope: 32.0, max_height: 0.5, clumping: 0.6, ..sc("pine", n(500.0), 15.0) }, ScatterJ { max_slope: 70.0, ..sc("rock", n(300.0), 2.4) }, ScatterJ { max_height: 0.55, on: Some("grass".into()), ..sc("grass", n(14000.0), 0.7) }];
        }
        "island" => {
            s.terrain = terrain("island", 22.0);
            s.sky = sky(14.0, 2.5, 0.2);
            s.fog = Some(0.0009);
            s.water = Some(WaterJ { kind: "sea".into(), level: Some(0.0), waves: Some(0.5), ..WaterJ::default() });
            s.scatter = vec![ScatterJ { min_height: 0.02, max_slope: 24.0, clumping: 0.5, ..sc("palm", n(160.0), 9.0) }, ScatterJ { min_height: 0.03, ..sc("bush", n(180.0), 1.4) }, ScatterJ { max_slope: 70.0, ..sc("rock", n(90.0), 2.0) }, ScatterJ { min_height: 0.05, on: Some("grass".into()), ..sc("grass", n(12000.0), 0.6) }];
        }
        "canyon" => {
            s.terrain = terrain("canyon", 70.0);
            s.sky = sky(16.5, 4.0, 0.1);
            s.fog = Some(0.0014);
            s.scatter = vec![ScatterJ { max_slope: 70.0, ..sc("rock", n(300.0), 3.0) }, ScatterJ { clumping: 0.7, ..sc("bush", n(220.0), 1.0) }, sc("dead_tree", n(40.0), 4.0), ScatterJ { max_slope: 25.0, ..sc("cactus", n(60.0), 3.0) }];
        }
        "arctic" => {
            s.terrain = terrain("hills", 40.0);
            s.sky = sky(11.0, 2.2, 0.55);
            s.fog = Some(0.003);
            s.paint = vec![PaintJ { layer: "snow".into(), at: [0.0, 0.0], radius: size * 0.8, strength: 0.95 }];
            s.scatter = vec![ScatterJ { max_slope: 28.0, clumping: 0.6, ..sc("pine", n(260.0), 12.0) }, ScatterJ { max_slope: 70.0, ..sc("rock", n(110.0), 2.0) }, sc("dead_tree", n(30.0), 4.5)];
        }
        "village" => {
            s.terrain = terrain("hills", 8.0);
            s.sky = sky(16.0, 2.8, 0.3);
            s.fog = Some(0.0016);
            s.water = Some(WaterJ { kind: "lake".into(), at: Some([size * 0.28, -size * 0.2]), radius: Some(size * 0.05), depth: Some(2.0), ..WaterJ::default() });
            s.scatter = vec![
                ScatterJ { on: Some("grass".into()), clumping: 0.4, ..sc("tree", n(110.0), 10.0) },
                ScatterJ { clumping: 0.3, ..sc("bush", n(160.0), 1.4) },
                ScatterJ { on: Some("grass".into()), ..sc("grass", n(22000.0), 0.7) },
                ScatterJ { on: Some("grass".into()), clumping: 0.9, ..sc("flower", n(1500.0), 0.5) },
                sc("rock", n(30.0), 1.3),
            ];
            let mut rng = Rng::new(seed ^ 0xA11);
            for i in 0..7 {
                let a = i as f32 / 7.0 * std::f32::consts::TAU + rng.range(-0.2, 0.2);
                let r = size * rng.range(0.06, 0.18);
                s.buildings.push(BuildingJ { kind: "house".into(), at: Some([a.cos() * r, a.sin() * r]), yaw: a.to_degrees() + 90.0 + rng.range(-12.0, 12.0), width: rng.range(7.0, 10.0), depth: rng.range(6.0, 8.0), floors: if rng.next() < 0.4 { 1 } else { 2 }, seed: Some(seed + i), furnish: vec!["table".into(), "chair".into(), "bed".into()], style: None });
                s.props.push(PropJ { kind: "fence".into(), at: [a.cos() * (r + 9.0), a.sin() * (r + 9.0)], yaw: a.to_degrees(), size: 3.5, count: 4, spread: 5.0, ..PropJ::default() });
            }
        }
        "city" => {
            s.terrain = terrain("flat", 1.0);
            s.sky = sky(19.0, 3.4, 0.35);
            s.fog = Some(0.0028);
            city_layout(&mut s, size, seed);
        }
        "interior" => {
            s.terrain = Some(TerrainJ { style: "flat".into(), height: Some(0.5), size: Some(size.min(120.0)), cell: None });
            s.sky = sky(14.0, 3.0, 0.3);
            s.fog = Some(0.0);
            interior_layout(&mut s, seed, false);
        }
        "scifi" => {
            s.terrain = terrain("dunes", 8.0);
            s.sky = Some(SkyJ { time: Some(Time::Hours(22.5)), turbidity: Some(6.0), strength: Some(0.5), clouds: Some(0.1), ..SkyJ::default() });
            s.fog = Some(0.003);
            s.scatter = vec![ScatterJ { max_slope: 60.0, ..sc("rock", n(140.0), 2.4) }];
            interior_layout(&mut s, seed, true);
            s.lights.push(LightJ { at: [-6.0, -18.0], height: 6.0, colour: "#40d8ff".into(), intensity: 1.4, range: 22.0, ..LightJ::default() });
        }
        _ => {}
    }
    s
}

fn city_layout(s: &mut WorldSpec, size: f32, seed: u32) {
    let mut rng = Rng::new(seed ^ 0xC17);
    let block = 56.0;
    let road = 12.0;
    let n = ((size * 0.7 / (block + road)) as i32).clamp(2, 5);
    let pitch = block + road;
    let off = -(n as f32 - 1.0) * pitch * 0.5;
    let extent = n as f32 * pitch * 0.5;
    for i in 0..=n {
        let p = -extent + i as f32 * pitch;
        s.roads.push(RoadJ { from: [p, -extent], to: [p, extent], width: road });
        s.roads.push(RoadJ { from: [-extent, p], to: [extent, p], width: road });
    }
    for bx in 0..n {
        for bz in 0..n {
            let cx = off + bx as f32 * pitch;
            let cz = off + bz as f32 * pitch;
            let centre = (((bx as f32 - (n as f32 - 1.0) / 2.0).powi(2) + (bz as f32 - (n as f32 - 1.0) / 2.0).powi(2)).sqrt() / n as f32).clamp(0.0, 1.0);
            // Two to four buildings per block, taller toward the middle.
            let lots = if rng.next() < 0.5 { 2 } else { 4 };
            for l in 0..lots {
                let (dx, dz) = if lots == 2 { (0.0, if l == 0 { -1.0 } else { 1.0 }) } else { (if l % 2 == 0 { -1.0 } else { 1.0 }, if l < 2 { -1.0 } else { 1.0 }) };
                let (lw, ld) = if lots == 2 { (block * 0.84, block * 0.4) } else { (block * 0.4, block * 0.4) };
                let kind_roll = rng.next();
                let (kind, floors) = if centre < 0.3 && kind_roll < 0.7 {
                    ("tower", (rng.range(14.0, 34.0)) as usize)
                } else if kind_roll < 0.4 {
                    ("tower", (rng.range(5.0, 12.0)) as usize)
                } else if kind_roll < 0.7 {
                    ("shop", 1)
                } else {
                    ("apartment", rng.range(4.0, 8.0) as usize)
                };
                let (w, d) = ((lw * rng.range(0.78, 0.95)).max(8.0), (ld * rng.range(0.78, 0.95)).max(8.0));
                s.buildings.push(BuildingJ {
                    kind: kind.into(),
                    at: Some([cx + dx * (block * 0.25), cz + dz * (block * 0.25)]),
                    // Front faces the nearer road.
                    yaw: if dz < 0.0 { 0.0 } else { 180.0 },
                    width: w,
                    depth: d,
                    floors,
                    seed: Some(rng.next_u32() & 0xffff),
                    furnish: Vec::new(),
                    style: None,
                });
            }
        }
    }
    // Lamps along the roads, trees in the verges, cars on the road.
    for i in 0..=n {
        let p = -extent + i as f32 * pitch;
        let mut t = -extent + 14.0;
        while t < extent - 10.0 {
            for side in [-1.0f32, 1.0] {
                s.props.push(PropJ { kind: "lamp".into(), at: [p + side * (road * 0.5 - 1.2), t], size: 6.5, yaw: if side > 0.0 { 180.0 } else { 0.0 }, ..PropJ::default() });
                s.props.push(PropJ { kind: "lamp".into(), at: [t, p + side * (road * 0.5 - 1.2)], size: 6.5, yaw: if side > 0.0 { 90.0 } else { -90.0 }, ..PropJ::default() });
            }
            t += 30.0;
        }
        for _ in 0..3 {
            let t = rng.range(-extent + 10.0, extent - 10.0);
            s.props.push(PropJ { kind: "car".into(), at: [p + rng.range(-1.0, 1.0) * 3.0 + if rng.next() < 0.5 { 2.0 } else { -2.0 }, t], size: 4.4, yaw: if rng.next() < 0.5 { 0.0 } else { 180.0 }, seed: Some(rng.next_u32() & 0xff), ..PropJ::default() });
        }
    }
    s.scatter = vec![ScatterJ { kind: "tree".into(), count: 36, size: 8.0, max_slope: 90.0, near: Some([0.0, 0.0]), radius: Some(extent), clumping: 0.0, ..ScatterJ::default() }];
    s.spawn = Some([0.0, -extent - 8.0, 0.0]);
}

fn interior_layout(s: &mut WorldSpec, seed: u32, scifi: bool) {
    let mut rng = Rng::new(seed ^ 0x1A7);
    // A row of rooms along x, each connected to the next.
    let rooms: Vec<(&str, f32, f32, Vec<&str>)> = if scifi {
        vec![("hall", 6.0, 12.0, vec!["crate", "crate"]), ("hab", 10.0, 9.0, vec!["desk", "shelf", "lamp_table"]), ("hab", 8.0, 8.0, vec!["bed", "shelf"])]
    } else {
        vec![("room", 7.0, 6.0, vec!["sofa", "table", "chair", "chair", "lamp_table", "plant_pot"]), ("room", 5.0, 6.0, vec!["bed", "shelf", "lamp_table"]), ("room", 6.0, 6.0, vec!["desk", "chair", "shelf", "plant_pot"])]
    };
    let total: f32 = rooms.iter().map(|r| r.1 + 0.4).sum();
    let mut x = -total * 0.5;
    for (i, (kind, w, d, furniture)) in rooms.into_iter().enumerate() {
        let cx = x + w * 0.5;
        x += w + 0.4;
        s.buildings.push(BuildingJ { kind: kind.into(), at: Some([cx, 0.0]), yaw: 0.0, width: w, depth: d, floors: 1, seed: Some(seed + i as u32), furnish: furniture.into_iter().map(String::from).collect(), style: None });
        let _ = rng.next();
    }
    s.spawn = Some([-total * 0.5 + 2.0, 0.0, 0.0]);
}

/// The colour in `#rrggbb` or a basic name as linear RGB.
pub fn parse_colour(s: &str) -> Option<V3> {
    let s = s.trim().to_lowercase();
    let rgb: [u8; 3] = if let Some(h) = s.strip_prefix('#') {
        if h.len() != 6 {
            return None;
        }
        [u8::from_str_radix(&h[0..2], 16).ok()?, u8::from_str_radix(&h[2..4], 16).ok()?, u8::from_str_radix(&h[4..6], 16).ok()?]
    } else {
        match s.as_str() {
            "white" => [255, 255, 255],
            "warm" | "warm white" | "amber" | "orange" => [255, 190, 110],
            "red" => [220, 40, 40],
            "green" => [60, 190, 80],
            "blue" => [60, 110, 230],
            "cyan" | "teal" => [60, 220, 240],
            "yellow" => [250, 220, 70],
            "purple" | "violet" => [150, 80, 210],
            "pink" => [240, 120, 180],
            "brown" => [120, 80, 50],
            "grey" | "gray" => [128, 128, 128],
            "black" => [20, 20, 20],
            "olive" => [110, 120, 50],
            "autumn" | "rust" => [170, 90, 30],
            "cold" | "cool white" => [190, 215, 255],
            _ => return None,
        }
    };
    Some(rgb.map(|v| crate::look_lin(v)))
}

// ------------------------------------------------------------ the builder

/// Where extra models come from (the app's 3D library).
pub trait Library: Sync {
    /// A model by words, in studio units (y down), and metres per unit.
    fn find(&self, query: &str) -> Option<(Mesh, f32)>;
}

/// Models by file name: a path to a `.smesh`, `.glb`, `.obj` or other
/// model file the mesh crate reads.
pub struct FileLibrary;
impl Library for FileLibrary {
    fn find(&self, query: &str) -> Option<(Mesh, f32)> {
        let p = std::path::Path::new(query);
        if !p.is_file() {
            return None;
        }
        let m = if p.extension().is_some_and(|e| e == "smesh") { studio_mesh::io::load(p).ok()? } else { studio_mesh::import::load(p, 0.0).ok()?.mesh };
        // Studio models are pixels: a hundred to the metre.
        Some((m, 0.01))
    }
}

pub struct NoLibrary;
impl Library for NoLibrary {
    fn find(&self, _: &str) -> Option<(Mesh, f32)> {
        None
    }
}

fn cache() -> &'static Mutex<HashMap<String, Arc<Vec<Asset>>>> {
    static C: OnceLock<Mutex<HashMap<String, Arc<Vec<Asset>>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cached(key: String, make: impl FnOnce() -> Vec<Asset>) -> Arc<Vec<Asset>> {
    if let Some(a) = cache().lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return a.clone();
    }
    let made = Arc::new(make());
    let mut g = cache().lock().unwrap_or_else(|e| e.into_inner());
    if g.len() > 400 {
        g.clear();
    }
    g.insert(key, made.clone());
    made
}

fn kit_mesh(name: &str, size: f32, seed: u64, budget: usize) -> Option<Mesh> {
    let asset = studio_modeling::kits::build(name, size, seed).ok()?;
    let mut m = asset.to_mesh(true).ok()?;
    if m.triangles() == 0 {
        return None;
    }
    if m.triangles() > budget {
        studio_mesh::decimate::decimate(&mut m, budget);
    }
    Some(m)
}

struct Kind {
    /// Collider.
    trunk: Option<(f32, f32)>,
    cull: f32,
    sway: f32,
}

/// Assets (a few variants) for a scatter kind at a height.
fn vegetation(kind: &str, size: f32, seed: u32, high: bool, recolour: Option<(V3, f32)>) -> Result<Arc<Vec<Asset>>, String> {
    let q = if high { "h" } else { "f" };
    let rk = recolour.map_or(String::new(), |(c, a)| format!("{:.2}{:.2}{:.2}{:.2}", c[0], c[1], c[2], a));
    let key = format!("veg|{kind}|{size:.2}|{seed}|{q}|{rk}");
    let k: Kind = match kind {
        "pine" | "fir" | "conifer" => Kind { trunk: Some((size * 0.025, size)), cull: 0.0, sway: size * 0.006 },
        "tree" | "oak" | "broadleaf" | "birch" => Kind { trunk: Some((size * 0.03, size)), cull: 0.0, sway: size * 0.008 },
        "palm" => Kind { trunk: Some((size * 0.03, size)), cull: 0.0, sway: size * 0.015 },
        "bush" | "shrub" => Kind { trunk: None, cull: 150.0 + size * 30.0, sway: size * 0.01 },
        "rock" | "boulder" | "stone" => Kind { trunk: Some((size * 0.38, size * 0.5)), cull: 0.0, sway: 0.0 },
        "grass" | "tuft" => Kind { trunk: None, cull: 45.0 + size * 25.0, sway: size * 0.12 },
        "flower" | "flowers" => Kind { trunk: None, cull: 40.0, sway: size * 0.1 },
        "cactus" => Kind { trunk: Some((size * 0.08, size)), cull: 0.0, sway: 0.0 },
        "dead_tree" | "deadwood" => Kind { trunk: Some((size * 0.03, size)), cull: 0.0, sway: 0.0 },
        other => return Err(format!("scatter kind \"{other}\": pine, tree, palm, bush, rock, grass, flower, cactus, dead_tree")),
    };
    Ok(cached(key, || {
        let variants = 3;
        let mut out = Vec::new();
        for v in 0..variants {
            let sd = seed.wrapping_add(v * 7919);
            let mut mesh: Option<Mesh> = None;
            if high {
                let kitname = match kind {
                    "pine" | "fir" | "conifer" => Some("pine"),
                    "tree" | "oak" | "broadleaf" | "birch" => Some("tree"),
                    "rock" | "boulder" | "stone" => Some("rock"),
                    _ => None,
                };
                if let Some(n) = kitname {
                    let budget = if n == "rock" { 2500 } else { 7000 };
                    mesh = kit_mesh(n, size, sd as u64, budget);
                }
            }
            if mesh.is_none() {
                let m = match kind {
                    "cactus" => kit::plant("cactus", size, sd),
                    "dead_tree" | "deadwood" => kit::plant("dead_tree", size, sd),
                    "pine" | "fir" | "conifer" => Some(studio_mesh::flora::make("pine", size, sd)),
                    "tree" | "oak" | "broadleaf" | "birch" => Some(studio_mesh::flora::make("tree", size, sd)),
                    "boulder" | "stone" => Some(studio_mesh::flora::make("rock", size, sd)),
                    other => Some(studio_mesh::flora::make(other, size, sd)),
                };
                // The kit plants are already y-up metres; flora is studio.
                match kind {
                    "cactus" | "dead_tree" | "deadwood" => {
                        if let Some(mut m) = m {
                            if let Some((c, a)) = recolour {
                                look::recolour(&mut m, &["cactus"], c, a * 0.5);
                            }
                            let asset = Asset::new(kind, m, &[0.5]).with_sway(k.sway);
                            out.push(if let Some((r, h)) = k.trunk { asset.trunk(r, h) } else { asset });
                            continue;
                        }
                    }
                    _ => mesh = m,
                }
            }
            let Some(mut m) = mesh else { continue };
            if let Some((c, a)) = recolour {
                look::recolour(&mut m, &["leaf", "leaves", "needle", "frond", "grass", "bush"], c, a);
            }
            let is_big = matches!(kind, "pine" | "fir" | "conifer" | "tree" | "oak" | "broadleaf" | "birch" | "rock" | "boulder" | "stone") && high;
            let ratios: &[f32] = if is_big { &[0.3, 0.08] } else { &[] };
            let mut a = Asset::from_studio(kind, &m, 1.0, ratios);
            a.sway = k.sway;
            a.cull_distance = k.cull;
            a.casts_shadow = !matches!(kind, "grass" | "tuft" | "flower" | "flowers");
            // Sunk a little: rocks half buried look settled.
            out.push(if let Some((r, h)) = k.trunk { a.trunk(r.max(0.05), h) } else { a });
        }
        out
    }))
}

struct Ctx<'a> {
    world: World,
    lib: &'a dyn Library,
    high: bool,
    foliage: Option<(V3, f32)>,
    /// Circles kept clear of plants: (x, z, radius).
    keep_clear: Vec<[f32; 3]>,
    /// Road rectangles (centre x, z, half length along dir, half width, yaw).
    clear_rects: Vec<[f32; 5]>,
    water_level: Option<f32>,
    said: Vec<String>,
}

impl Ctx<'_> {
    fn terrain(&self) -> Option<&Terrain> {
        self.world.terrain.as_ref()
    }

    fn blocked(&self, x: f32, z: f32, margin: f32) -> bool {
        if self.keep_clear.iter().any(|c| dist_xz([x, 0.0, z], [c[0], 0.0, c[1]]) < c[2] + margin) {
            return true;
        }
        self.clear_rects.iter().any(|r| {
            let (s, c) = r[4].to_radians().sin_cos();
            let (dx, dz) = (x - r[0], z - r[1]);
            let (lx, lz) = (dx * c - dz * s, dx * s + dz * c);
            lx.abs() < r[2] + margin && lz.abs() < r[3] + margin
        })
    }

    fn ground(&self, x: f32, z: f32) -> f32 {
        self.world.ground_y(x, z)
    }
}

fn layer_index(t: &Terrain, name: &str) -> Option<usize> {
    if let Ok(i) = name.parse::<usize>() {
        return (i < 4).then_some(i);
    }
    let n = name.to_lowercase();
    t.layers.iter().position(|l| l.name == n).or(match n.as_str() {
        "grass" | "lawn" => Some(0),
        "rock" | "stone" | "cliff" => Some(1),
        "dirt" | "mud" | "path" | "earth" | "sand" => Some(2),
        "snow" | "ice" => Some(3),
        _ => None,
    })
}

fn place_scatter(cx: &mut Ctx, rule: &ScatterJ, idx: usize) -> Result<(), String> {
    let Some(t) = cx.terrain().cloned() else { return Err("scatter needs terrain".into()) };
    let kind = rule.kind.to_lowercase();
    let size = if rule.size > 0.0 { rule.size } else { 2.0 };
    let seed = rule.seed.unwrap_or(cx.world.seed.wrapping_add(idx as u32 * 131 + 17));
    let vars = vegetation(&kind, size, seed, cx.high, cx.foliage)?;
    if vars.is_empty() {
        return Err(format!("could not make {kind}"));
    }
    let b = t.bounds();
    let relief = (b.max[1] - b.min[1]).max(0.1);
    let mut rng = Rng::new(seed);
    let layer = rule.on.as_deref().and_then(|n| layer_index(&t, n));
    let spacing = match kind.as_str() {
        "pine" | "fir" | "conifer" | "tree" | "oak" | "broadleaf" | "birch" | "palm" => size * 0.16,
        "rock" | "boulder" | "stone" | "cactus" | "dead_tree" => size * 0.4,
        "bush" => size * 0.3,
        _ => 0.0,
    };
    let mut taken: HashMap<(i32, i32), Vec<[f32; 2]>> = HashMap::new();
    let (area_c, area_r) = match (rule.near, rule.radius) {
        (Some(c), Some(r)) => (c, r),
        (Some(c), None) => (c, 40.0),
        (None, Some(r)) => ([0.0, 0.0], r),
        _ => ([0.0, 0.0], f32::INFINITY),
    };
    let sz = t.size();
    let (x0, x1) = (t.origin[0] + 2.0, t.origin[0] + sz[0] - 2.0);
    let (z0, z1) = (t.origin[1] + 2.0, t.origin[1] + sz[1] - 2.0);
    let clumps: Vec<[f32; 2]> = (0..((rule.count / 40).clamp(3, 60))).map(|_| [rng.range(x0, x1), rng.range(z0, z1)]).collect();
    let mut sets: Vec<Vec<Instance>> = vec![Vec::new(); vars.len()];
    let (mut placed, mut tries) = (0usize, 0usize);
    let max_tries = rule.count * 25 + 200;
    let clump_r = (sz[0].max(sz[1]) * 0.05).max(8.0);
    while placed < rule.count && tries < max_tries {
        tries += 1;
        let (mut x, mut z) = (rng.range(x0, x1), rng.range(z0, z1));
        if area_r.is_finite() {
            let a = rng.range(0.0, std::f32::consts::TAU);
            let r = area_r * rng.next().sqrt();
            x = area_c[0] + a.cos() * r;
            z = area_c[1] + a.sin() * r;
            if x < x0 || x > x1 || z < z0 || z > z1 {
                continue;
            }
        } else if rule.clumping > 0.0 && rng.next() < rule.clumping {
            let c = clumps[rng.index(clumps.len())];
            x = (c[0] + (rng.next() - 0.5) * clump_r * 2.0).clamp(x0, x1);
            z = (c[1] + (rng.next() - 0.5) * clump_r * 2.0).clamp(z0, z1);
        }
        let y = t.height_at(x, z);
        let frac = (y - b.min[1]) / relief;
        if frac < rule.min_height || frac > rule.max_height || t.slope_at(x, z) > rule.max_slope {
            continue;
        }
        if let Some(l) = layer
            && t.weights_at(x, z)[l] < 0.35
        {
            continue;
        }
        if let Some(w) = cx.water_level {
            let margin = if spacing > 0.0 { 0.6 } else { 0.05 };
            if y < w + margin {
                continue;
            }
        }
        if cx.world.water.iter().any(|w| x > w.min[0] - 1.0 && x < w.max[0] + 1.0 && z > w.min[1] - 1.0 && z < w.max[1] + 1.0 && y < w.level + 0.5) {
            continue;
        }
        if cx.blocked(x, z, if spacing > 0.0 { size * 0.1 } else { 0.0 }) {
            continue;
        }
        if spacing > 0.0 {
            let key = ((x / spacing) as i32, (z / spacing) as i32);
            let mut clash = false;
            'n: for dx in -1..=1 {
                for dz in -1..=1 {
                    if let Some(v) = taken.get(&(key.0 + dx, key.1 + dz)) {
                        for p in v {
                            if (p[0] - x).powi(2) + (p[1] - z).powi(2) < spacing * spacing {
                                clash = true;
                                break 'n;
                            }
                        }
                    }
                }
            }
            if clash {
                continue;
            }
            taken.entry(key).or_default().push([x, z]);
        }
        let k = rng.range(0.72, 1.28);
        let sink = if matches!(kind.as_str(), "rock" | "boulder" | "stone") { size * k * 0.12 } else { 0.0 };
        sets[rng.index(vars.len())].push(Instance { pos: [x, y - sink, z], yaw: rng.range(0.0, 360.0), scale: k, rank: (rng.next() * 255.0) as u8 });
        placed += 1;
    }
    let collide = !matches!(kind.as_str(), "grass" | "tuft" | "flower" | "flowers" | "bush" | "shrub");
    for (v, inst) in sets.into_iter().enumerate() {
        if inst.is_empty() {
            continue;
        }
        let a = cx.world.add_asset(vars[v].clone());
        cx.world.add_scatter(&format!("{kind} {}", v + 1), a, inst, collide);
    }
    cx.said.push(format!("{placed} {kind}"));
    Ok(())
}

fn to_light(cx: &Ctx, j: &LightJ) -> Result<Light, String> {
    let c = parse_colour(&j.colour).ok_or_else(|| format!("light colour \"{}\"", j.colour))?;
    let y = cx.ground(j.at[0], j.at[1]) + j.height;
    let power = j.range * j.range * 0.35 * j.intensity;
    let kind = if j.kind.to_lowercase() == "spot" { LightKind::Spot } else { LightKind::Point };
    let pos = [j.at[0], y, j.at[1]];
    let dir = j.aim.map_or([0.0, -1.0, 0.0], |a| crate::math::norm(crate::math::sub(a, pos)));
    Ok(Light { kind, position: pos, direction: dir, colour: [c[0] * power, c[1] * power, c[2] * power], range: j.range, cone: j.cone })
}

fn light_power(range: f32) -> f32 {
    range * range * 0.35
}

/// A building from the architecture generators: metres, y up, one box to bump into.
fn styled_structure(style: studio_modeling::places::Style, j: &BuildingJ, seed: u32) -> Result<Built, String> {
    use studio_modeling::places::{self, Spec};
    let mut spec = Spec::new("building", style);
    spec.floors = Some(j.floors.max(1));
    spec.width = Some(j.width * 100.0);
    spec.depth = Some(j.depth * 100.0);
    spec.seed = seed as u64;
    spec.detail = 1;
    let (_, mut mesh) = places::build_mesh(&spec)?;
    // Studio pixels with y down, 100 to the metre, ground at 0 → metres with y up.
    for p in &mut mesh.positions {
        *p = [p[0] * 0.01, -p[1] * 0.01, p[2] * 0.01];
    }
    for n in &mut mesh.normals {
        n[1] = -n[1];
    }
    for t in mesh.indices.chunks_mut(3) {
        t.swap(1, 2);
    }
    let (lo, hi) = mesh.bounds();
    let size = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
    let mut b = Built::from_mesh(mesh, size);
    b.solids.push(Solid::Box { centre: [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0], half: [size[0] / 2.0, size[1] / 2.0, size[2] / 2.0], yaw: 0.0 });
    Ok(b)
}

fn add_structure(cx: &mut Ctx, j: &BuildingJ, i: usize) -> Result<(), String> {
    let seed = j.seed.unwrap_or(cx.world.seed.wrapping_add(i as u32 * 31));
    let built: Built = match j.style.as_deref().and_then(studio_modeling::places::Style::parse).filter(|_| !matches!(j.kind.as_str(), "room" | "hall" | "corridor" | "hab")) {
        Some(style) => styled_structure(style, j, seed)?,
        None => kit::structure(&j.kind, j.width, j.depth, j.floors.max(1), seed).ok_or_else(|| format!("building kind \"{}\": {}", j.kind, kit::STRUCTURES.join(", ")))?,
    };
    let at = j.at.unwrap_or([0.0, 0.0]);
    let (w, d) = (j.width.max(2.0), j.depth.max(2.0));
    let radius = (w * w + d * d).sqrt() * 0.5;
    // A level pad under it (not for rooms in a flat scene).
    let mut base = cx.ground(at[0], at[1]);
    if let Some(t) = cx.world.terrain.as_mut() {
        let near = (0..9).map(|k| t.height_at(at[0] + ((k % 3) as f32 - 1.0) * w * 0.5, at[1] + ((k / 3) as f32 - 1.0) * d * 0.5)).fold(f32::MIN, f32::max);
        base = base.max(near.min(base + 3.0));
        if !matches!(j.kind.as_str(), "room" | "hall" | "corridor" | "hab") || t.slope_at(at[0], at[1]) > 1.0 {
            t.pad(at, radius * 0.9, radius * 0.7 + 3.0, base);
        }
    }
    let pos = [at[0], base, at[1]];
    let (mesh, solids, lights) = built.mesh();
    let asset = Asset::new(&j.kind, mesh, &[]);
    let a = cx.world.add_asset(asset);
    cx.world.place(&j.kind, a, ObjectKind::Building, Instance { pos, yaw: j.yaw, scale: 1.0, rank: 0 }, false);
    cx.world.extra_solids.extend(Built::solids_placed(&solids, pos, j.yaw));
    let (s, c) = j.yaw.to_radians().sin_cos();
    for (lp, col, range) in lights {
        let rp = [lp[0] * c + lp[2] * s, lp[1], -lp[0] * s + lp[2] * c];
        let p = light_power(range) * 0.5;
        cx.world.lights.push(Light { kind: LightKind::Point, position: [pos[0] + rp[0], pos[1] + rp[1], pos[2] + rp[2]], direction: [0.0, -1.0, 0.0], colour: [col[0] * p, col[1] * p, col[2] * p], range, cone: 0.0 });
    }
    cx.keep_clear.push([at[0], at[1], radius + 1.5]);
    // Furniture inside.
    if !j.furnish.is_empty() {
        let mut rng = Rng::new(seed ^ 0xF0);
        let n = j.furnish.len();
        for (k, f) in j.furnish.iter().enumerate() {
            // Spread round the walls inside, facing the middle.
            let a = (k as f32 + 0.5) / n as f32;
            let (lx, lz) = (((a * 7.0).fract() - 0.5) * (w - 2.0).max(0.5) * 0.9, ((a * 3.0 + 0.13).fract() - 0.5) * (d - 2.0).max(0.5) * 0.9);
            let floor_y = pos[1] + 0.2;
            let (wx, wz) = (pos[0] + lx * c + lz * s, pos[2] - lx * s + lz * c);
            let size = prop_size(f);
            let yaw = j.yaw + (lx.atan2(lz)).to_degrees() + 180.0 + rng.range(-20.0, 20.0);
            let flo = if j.kind == "room" || j.kind == "hall" || j.kind == "hab" { pos[1] + 0.12 } else { floor_y };
            add_prop(cx, f, [wx, wz], flo - cx.ground(wx, wz), yaw, size, seed.wrapping_add(k as u32))?;
        }
    }
    Ok(())
}

fn prop_size(kind: &str) -> f32 {
    match kind {
        "crate" => 0.9,
        "barrel" => 1.0,
        "table" => 1.6,
        "chair" => 0.95,
        "column" => 4.0,
        "stairs" => 3.0,
        "lamp" => 6.0,
        "fence" => 3.5,
        "car" => 4.4,
        "sofa" => 2.0,
        "bed" => 2.0,
        "shelf" | "bookcase" => 1.9,
        "desk" | "counter" => 1.6,
        "lamp_table" | "floor_lamp" => 1.5,
        "pot" | "plant_pot" => 0.8,
        _ => 1.0,
    }
}

fn add_prop(cx: &mut Ctx, kind: &str, at: [f32; 2], lift: f32, yaw: f32, size: f32, seed: u32) -> Result<(), String> {
    let size = if size > 0.0 { size } else { prop_size(kind) };
    let key = format!("prop|{kind}|{size:.2}|{seed}");
    let high = cx.high;
    let lib = cx.lib;
    let assets = cached(key, || {
        let mut mesh: Option<Mesh> = None;
        let mut studio = true;
        match kind {
            "table" | "chair" | "crate" | "barrel" | "column" | "stairs" | "bolt" | "pipe" | "building" if high || matches!(kind, "table" | "chair" | "barrel" | "column" | "stairs") => {
                mesh = kit_mesh(kind, size, seed as u64, 6000);
            }
            _ => {}
        }
        if mesh.is_none() {
            let y_up = kit::plant(kind, size, seed).or_else(|| kit::furniture(kind, size, seed));
            if let Some(m) = y_up {
                mesh = Some(m);
                studio = false;
            }
        }
        if mesh.is_none() {
            // Anything else is asked of the library.
            if let Some((m, k)) = lib.find(kind) {
                let (lo, hi) = m.bounds();
                let h = (hi[1] - lo[1]).max(1e-4) * k;
                let mut out = mesh_from_studio(&m, k * size / h);
                // Stand on y = 0.
                let low = mesh_bounds(&out).min[1];
                for p in out.positions.iter_mut() {
                    p[1] -= low;
                }
                return vec![Asset::new(kind, out, &[0.4, 0.1]).boxed()];
            }
        }
        let Some(m) = mesh else { return Vec::new() };
        let a = if studio { Asset::from_studio(kind, &m, 1.0, &[]) } else { Asset::new(kind, m, &[]) };
        // Stand on y = 0 and cover the shape for walkers.
        let lo = a.bounds.min;
        let mut a = a;
        if lo[1].abs() > 1e-3 {
            let mut lods = Vec::new();
            for l in &a.lods {
                let mut m = (**l).clone();
                for p in m.positions.iter_mut() {
                    p[1] -= lo[1];
                }
                lods.push(Arc::new(m));
            }
            a = Asset { lods, bounds: crate::math::Aabb::new([a.bounds.min[0], 0.0, a.bounds.min[2]], [a.bounds.max[0], a.bounds.max[1] - lo[1], a.bounds.max[2]]), ..a };
        }
        let b = a.bounds;
        let c = match kind {
            "barrel" | "column" | "lamp" => ColliderShape::Cylinder { radius: ((b.max[0] - b.min[0]).max(b.max[2] - b.min[2])) * 0.5, height: b.max[1] },
            "stairs" => ColliderShape::Box(b),
            "fence" | "plant_pot" | "pot" | "lamp_table" | "floor_lamp" => ColliderShape::None,
            _ => ColliderShape::Box(b),
        };
        vec![a.with_collider(c)]
    });
    let Some(asset) = assets.first() else { return Err(format!("prop \"{kind}\": crate, barrel, table, chair, column, stairs, lamp, fence, car, sofa, bed, shelf, desk, cactus, or a model in the library")) };
    let a = cx.world.add_asset(asset.clone());
    let y = cx.ground(at[0], at[1]) + lift;
    cx.world.place(kind, a, ObjectKind::Prop, Instance { pos: [at[0], y, at[1]], yaw, scale: 1.0, rank: 0 }, true);
    if kind == "lamp" {
        let (s, c) = yaw.to_radians().sin_cos();
        let p = [at[0] + (size * 0.2) * c, y + size - 0.2, at[1] - (size * 0.2) * s];
        let power = light_power(size * 2.2);
        cx.world.lights.push(Light { kind: LightKind::Point, position: p, direction: [0.0, -1.0, 0.0], colour: [1.0 * power, 0.78 * power, 0.5 * power], range: size * 2.2, cone: 0.0 });
    }
    Ok(())
}

/// The spec with its preset's contents filled in and its choices (preset,
/// size, seed) made: what [`build`] builds from. Building the result gives
/// the same world, so a world can be edited item by item.
pub fn materialize(spec: &WorldSpec) -> Result<WorldSpec, String> {
    let (mut p, preset, size, seed) = resolve(spec)?;
    p.preset = preset;
    p.size = Some(size);
    p.seed = Some(seed);
    p.description = spec.description.clone();
    p.name = spec.name.clone();
    p.quality = spec.quality.clone();
    p.without.clear();
    Ok(p)
}

fn resolve(spec: &WorldSpec) -> Result<(WorldSpec, String, f32, u32), String> {
    let desc = format!("{} {}", spec.description, spec.name);
    let preset_name = if spec.preset.is_empty() { guess_preset(&desc).to_string() } else { spec.preset.to_lowercase() };
    if !PRESETS.iter().any(|p| p.0 == preset_name) && !spec.preset.is_empty() {
        return Err(format!("preset \"{}\": {}", spec.preset, PRESETS.iter().map(|p| p.0).collect::<Vec<_>>().join(", ")));
    }
    let seed = spec.seed.unwrap_or(1);
    let size = spec.size.unwrap_or(match preset_name.as_str() {
        "interior" => 60.0,
        "city" => 420.0,
        "scifi" => 300.0,
        "mountains" | "canyon" => 600.0,
        _ => 400.0,
    });
    // Preset first (minus what the spec leaves out), the spec's own fields over it.
    let mut p = preset_spec(&preset_name, size, seed);
    for what in &spec.without {
        match what.as_str() {
            "scatter" => p.scatter.clear(),
            "buildings" => p.buildings.clear(),
            "props" => p.props.clear(),
            "characters" => p.characters.clear(),
            "lights" => p.lights.clear(),
            "roads" => p.roads.clear(),
            "water" => p.water = None,
            "sculpt" => p.sculpt.clear(),
            "paint" => p.paint.clear(),
            other => return Err(format!("without \"{other}\": scatter, buildings, props, characters, lights, roads, water, sculpt, paint")),
        }
    }
    macro_rules! over {
        ($f:ident) => {
            if !spec.$f.is_empty() {
                p.$f = spec.$f.clone();
            }
        };
    }
    over!(sculpt);
    over!(paint);
    over!(scatter);
    over!(buildings);
    over!(props);
    over!(characters);
    over!(lights);
    over!(roads);
    if spec.terrain.is_some() {
        p.terrain = spec.terrain.clone();
    }
    if spec.sky.is_some() {
        p.sky = spec.sky.clone();
    }
    if spec.fog.is_some() {
        p.fog = spec.fog;
    }
    if spec.water.is_some() {
        p.water = spec.water.clone();
    }
    if spec.spawn.is_some() {
        p.spawn = spec.spawn;
    }
    p.look = spec.look.clone();
    // A time of day in the description, when the spec has none.
    let said_time = time_from_words(&desc);
    if let (Some(h), None) = (said_time, spec.sky.as_ref().and_then(|s| s.time.as_ref())) {
        let mut sky = p.sky.clone().unwrap_or_default();
        sky.time = Some(Time::Hours(h));
        p.sky = Some(sky);
    }
    Ok((p, preset_name, size, seed))
}

/// Builds the world a spec describes.
pub fn build(spec: &WorldSpec, lib: &dyn Library) -> Result<World, String> {
    let desc = format!("{} {}", spec.description, spec.name);
    let (p, preset_name, size, seed) = resolve(spec)?;
    let said_time = time_from_words(&desc);

    let mut world = World::new(if spec.name.is_empty() { &preset_name } else { &spec.name });
    world.seed = seed;
    let hints = p.look.as_ref().and_then(|l| l.hints.clone());
    let look_j = p.look.clone().unwrap_or_default();
    // Terrain.
    let tj = p.terrain.clone().unwrap_or(TerrainJ { style: "hills".into(), height: Some(30.0), size: Some(size), cell: None });
    let style = Style::parse(if tj.style.is_empty() { "hills" } else { &tj.style }).ok_or_else(|| format!("terrain style \"{}\": {}", tj.style, crate::terrain::STYLES.join(", ")))?;
    let tsize = tj.size.unwrap_or(size);
    let cell = tj.cell.unwrap_or(if tsize > 700.0 { 2.0 } else { 1.0 });
    let mut terrain = Terrain::generate(&TerrainSpec { size: [tsize, tsize], cell, style, height: tj.height.unwrap_or(30.0), seed });
    if let Some(g) = look_j.ground.as_deref().and_then(parse_colour).or(hints.as_ref().map(|h| h.ground)) {
        // The look's ground colour tints the soft layers.
        let amount = if look_j.ground.is_some() { 0.6 } else { 0.35 };
        for k in [0usize, 2] {
            let l = &mut terrain.layers[k];
            let lum = |c: V3| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
            let gl = lum(g).max(1e-4);
            for c in 0..3 {
                let target = g[c] * lum(l.colour).max(1e-4) / gl;
                l.colour[c] += (target - l.colour[c]) * amount;
            }
        }
    }
    let foliage = look_j.foliage.as_deref().and_then(parse_colour).map(|c| (c, 0.75)).or(hints.as_ref().and_then(|h| h.foliage).map(|c| (c, 0.55)));
    if let Some((fc, a)) = foliage {
        let l = &mut terrain.layers[0];
        for c in 0..3 {
            l.colour[c] += (fc[c] * 0.6 - l.colour[c]) * a * 0.5;
        }
    }
    if preset_name == "city" {
        // Paved all over.
        terrain.layers[0] = crate::terrain::Layer { name: "paving".into(), colour: [0.12, 0.12, 0.125], roughness: 0.9, tile: 3.0 };
    }
    world.terrain = Some(terrain);
    let mut cx = Ctx { world, lib, high: spec.quality != "fast", foliage, keep_clear: Vec::new(), clear_rects: Vec::new(), water_level: None, said: Vec::new() };

    // Water.
    if let Some(w) = p.water.as_ref().filter(|w| w.kind != "none") {
        let t = cx.world.terrain.as_mut().unwrap();
        let b = t.bounds();
        let colour = w.colour.as_deref().and_then(parse_colour).unwrap_or([0.012, 0.075, 0.1]);
        match w.kind.as_str() {
            "sea" | "ocean" => {
                let level = w.level.unwrap_or(0.0);
                let s = t.size();
                cx.world.water.push(Water { level, min: [t.origin[0] - 400.0, t.origin[1] - 400.0], max: [t.origin[0] + s[0] + 400.0, t.origin[1] + s[1] + 400.0], colour, clarity: 5.0, wave_height: 0.22 * w.waves.unwrap_or(1.0), wave_speed: 1.0 });
                cx.water_level = Some(level);
            }
            "river" | "stream" => {
                let level = b.min[1] + w.level.unwrap_or(0.9);
                let s = t.size();
                cx.world.water.push(Water { level, min: t.origin, max: [t.origin[0] + s[0], t.origin[1] + s[1]], colour, clarity: 2.5, wave_height: 0.05, wave_speed: 1.4 });
                cx.water_level = Some(level);
            }
            _ => {
                let at = w.at.unwrap_or([0.0, 0.0]);
                let r = w.radius.unwrap_or(25.0);
                let depth = w.depth.unwrap_or(3.0);
                let rim = t.height_at(at[0], at[1]);
                t.basin(at, r, depth + 1.0);
                // A lake's surface sits where the bank meets it.
                let level = w.level.unwrap_or(rim - 0.6);
                let pad = r * 1.35;
                cx.world.water.push(Water { level, min: [at[0] - pad, at[1] - pad], max: [at[0] + pad, at[1] + pad], colour, clarity: 3.0, wave_height: 0.04 * w.waves.unwrap_or(1.0), wave_speed: 0.8 });
            }
        }
    }
    // Roads first, then structures flatten what they stand on.
    for j in &p.roads {
        cx.clear_rects.push({
            let d = [j.to[0] - j.from[0], j.to[1] - j.from[1]];
            let l = (d[0] * d[0] + d[1] * d[1]).sqrt();
            [(j.from[0] + j.to[0]) * 0.5, (j.from[1] + j.to[1]) * 0.5, l * 0.5, j.width * 0.5 + 3.5, (-d[1]).atan2(d[0]).to_degrees()]
        });
    }
    add_roads_meshes(&mut cx, &p.roads);
    for (i, b) in p.buildings.iter().enumerate() {
        add_structure(&mut cx, b, i)?;
    }
    // Hand-sculpting and painting.
    for s in &p.sculpt {
        let t = cx.world.terrain.as_mut().unwrap();
        let brush = Brush::parse(&s.brush, s.level).ok_or_else(|| format!("sculpt brush \"{}\": raise, lower, smooth, flatten, level (with \"level\"), noise", s.brush))?;
        for _ in 0..s.passes.clamp(1, 200) {
            t.sculpt(brush, s.at, s.radius, s.strength);
        }
    }
    for s in &p.paint {
        let t = cx.world.terrain.as_mut().unwrap();
        let l = layer_index(t, &s.layer).ok_or_else(|| format!("paint layer \"{}\": grass, rock, dirt, snow or 0-3", s.layer))?;
        t.paint(l, s.at, s.radius, s.strength);
    }
    // Plants and rocks.
    for (i, rule) in p.scatter.iter().enumerate() {
        place_scatter(&mut cx, rule, i)?;
    }
    for (i, pr) in p.props.iter().enumerate() {
        let n = pr.count.max(1);
        let mut rng = Rng::new(pr.seed.unwrap_or(seed.wrapping_add(i as u32 * 17)));
        for k in 0..n {
            let at = if n > 1 && pr.spread > 0.0 { [pr.at[0] + rng.range(-pr.spread, pr.spread), pr.at[1] + rng.range(-pr.spread, pr.spread)] } else { pr.at };
            add_prop(&mut cx, &pr.kind.to_lowercase(), at, pr.lift, pr.yaw + if n > 1 { rng.range(-8.0, 8.0) } else { 0.0 }, pr.size, pr.seed.unwrap_or(seed).wrapping_add(k as u32))?;
        }
    }
    for c in &p.characters {
        add_character(&mut cx, c)?;
    }
    for l in &p.lights {
        let light = to_light(&cx, l)?;
        cx.world.lights.push(light);
    }
    // Sky and air.
    let mut sky = Sky::default();
    if let Some(sj) = &p.sky {
        if let Some(t) = &sj.time {
            sky.time = t.hours()?;
        }
        sky.day = sj.day.unwrap_or(sky.day);
        sky.latitude = sj.latitude.unwrap_or(sky.latitude);
        sky.heading = sj.heading.unwrap_or(sky.heading);
        sky.turbidity = sj.turbidity.unwrap_or(sky.turbidity).clamp(1.7, 12.0);
        sky.strength = sj.strength.unwrap_or(sky.strength);
        sky.clouds = sj.clouds.unwrap_or(sky.clouds);
        sky.hdri = sj.hdri.clone();
    }
    if let (Some(h), None, None) = (&hints, spec.sky.as_ref().and_then(|s| s.time.as_ref()), said_time) {
        sky.time = h.time;
        sky.turbidity = (sky.turbidity + h.haze * 4.0).clamp(1.7, 12.0);
        sky.clouds = h.clouds.max(sky.clouds * 0.5);
    }
    cx.world.sky = sky;
    cx.world.atmosphere.fog = p.fog.unwrap_or(cx.world.atmosphere.fog);
    if let Some(h) = &hints {
        cx.world.atmosphere.fog += h.haze * 0.002;
        cx.world.atmosphere.colour = Some(h.horizon);
    }
    if let Some(c) = look_j.fog_colour.as_deref().and_then(parse_colour) {
        cx.world.atmosphere.colour = Some(c);
    }
    let night = cx.world.sky.sun_elevation() < -3.0;
    cx.world.look.exposure = look_j.exposure.unwrap_or(if night { 0.5 } else { 0.0 });
    if let Some(v) = look_j.bloom {
        cx.world.look.bloom = v;
    }
    if let Some(v) = look_j.ao {
        cx.world.look.ssao = v;
    }
    if let Some(v) = look_j.saturation {
        cx.world.look.saturation = v;
    }
    if let Some(v) = look_j.vignette {
        cx.world.look.vignette = v;
    }
    cx.world.finish();
    // Where to start.
    let sp = match p.spawn {
        Some(s) => find_spawn(&cx.world, Some([s[0], s[2]]), s[1]),
        None => find_spawn(&cx.world, None, 0.0),
    };
    cx.world.spawn = sp;
    cx.world.finish();
    let w = cx.world;
    Ok(w)
}

/// A free, level, dry spot near the middle (or near `want`), facing the
/// direction with the longest clear view.
pub fn find_spawn(w: &World, want: Option<[f32; 2]>, yaw_hint: f32) -> Spawn {
    let centre = want.unwrap_or([0.0, 0.0]);
    let eye = 1.0;
    let mut best: Option<(f32, [f32; 2])> = None;
    for ring in 0..24 {
        for k in 0..16 {
            let r = ring as f32 * 3.0;
            let a = k as f32 / 16.0 * std::f32::consts::TAU;
            let (x, z) = (centre[0] + a.cos() * r, centre[1] + a.sin() * r);
            let Some(t) = w.terrain.as_ref() else { continue };
            if !t.contains(x, z) {
                continue;
            }
            let ty = t.height_at(x, z);
            // The floor you would stand on: a slab or road a little above the ground.
            let Some(g) = w.ground_at(x, z, ty + 0.6) else { continue };
            let wet = w.water_at(x, z).is_some_and(|l| g.y < l + 0.3);
            let slope = t.slope_at(x, z);
            // Nothing solid right here.
            let push = w.push_out([x, g.y, z], 0.6, g.y + 0.5, g.y + 1.8);
            if wet || slope > 12.0 || push[0].abs() + push[1].abs() > 1e-3 {
                continue;
            }
            let score = r + slope * 0.5;
            if best.is_none_or(|b| score < b.0) {
                best = Some((score, [x, z]));
            }
        }
        if best.is_some() && ring > 3 {
            break;
        }
    }
    let pos2 = best.map_or(centre, |b| b.1);
    let ty = w.ground_y(pos2[0], pos2[1]);
    let y = w.ground_at(pos2[0], pos2[1], ty + 0.6).map_or(ty, |g| g.y);
    // Face where one can see furthest.
    let mut bestyaw = yaw_hint;
    let mut far = 0.0f32;
    for k in 0..24 {
        let yaw = k as f32 * 15.0;
        let (s, c) = yaw.to_radians().sin_cos();
        let t = w.raycast([pos2[0], y + eye, pos2[1]], [s, 0.0, c], 120.0).map_or(120.0, |h| h.t);
        if t > far + 0.5 {
            far = t;
            bestyaw = yaw;
        }
    }
    Spawn { position: [pos2[0], y, pos2[1]], yaw: if want.is_some() && yaw_hint != 0.0 { yaw_hint } else { bestyaw } }
}

fn add_roads_meshes(cx: &mut Ctx, roads: &[RoadJ]) {
    if roads.is_empty() {
        return;
    }
    let mut b = kit::MeshBuilder::default();
    let asphalt = b.material(kit::mat("Asphalt", [0.035, 0.036, 0.04], 0.85, 0.0));
    let paving = b.material(kit::mat("Pavement", [0.3, 0.29, 0.27], 0.9, 0.0));
    let paint = b.material(kit::mat("Road paint", [0.7, 0.62, 0.2], 0.7, 0.0));
    let mut solids = Vec::new();
    let base = cx.ground(0.0, 0.0);
    for r in roads {
        let d = [r.to[0] - r.from[0], r.to[1] - r.from[1]];
        let l = (d[0] * d[0] + d[1] * d[1]).sqrt();
        if l < 1.0 {
            continue;
        }
        let yaw = (-d[1]).atan2(d[0]).to_degrees();
        let c = [(r.from[0] + r.to[0]) * 0.5, (r.from[1] + r.to[1]) * 0.5];
        let w = r.width;
        // The carriageway slightly raised, the kerbs higher.
        b.cuboid([c[0], base + 0.04, c[1]], [l * 0.5, 0.04, w * 0.5 - 1.8], yaw, asphalt);
        let nrm = [-d[1] / l, d[0] / l];
        for side in [-1.0f32, 1.0] {
            let off = side * (w * 0.5 - 0.9);
            let sc = [c[0] + nrm[0] * off, c[1] + nrm[1] * off];
            b.cuboid([sc[0], base + 0.1, sc[1]], [l * 0.5, 0.1, 0.9 + 0.9], yaw, paving);
            solids.push(Solid::Box { centre: [sc[0], base + 0.1, sc[1]], half: [l * 0.5, 0.1, 1.8], yaw });
        }
        let dashes = (l / 6.0) as usize;
        for k in 0..dashes {
            let t = (k as f32 + 0.5) / dashes as f32 - 0.5;
            let p = [c[0] + d[0] / l * t * l, c[1] + d[1] / l * t * l];
            b.cuboid([p[0], base + 0.085, p[1]], [1.4, 0.006, 0.07], yaw, paint);
        }
    }
    let mesh = b.finish();
    let a = cx.world.add_asset(Asset::new("roads", mesh, &[]));
    cx.world.place("roads", a, ObjectKind::Building, Instance::at([0.0, 0.0, 0.0]), false);
    cx.world.extra_solids.extend(solids);
}

/// A rigged character asset (feet on y = 0, 1.75 m tall), its top and
/// skin in sRGB; `detail` 1 is light, 2 finer.
pub fn character_asset(top: Option<[u8; 3]>, skin: Option<[u8; 3]>, detail: usize) -> Asset {
    let mut build = studio_mesh::character::Build { detail, ..Default::default() };
    if let Some(t) = top {
        build.top = t;
    }
    if let Some(s) = skin {
        build.skin = s;
    }
    let units = 175.0;
    let key = format!("char|{:?}|{:?}|{}", build.top, build.skin, build.detail);
    let assets = cached(key, || {
        let m = studio_mesh::character::character(units, &build);
        let k = 1.75 / units;
        let mut a = Asset::from_studio("character", &m, k, &[0.5]);
        // Feet on y = 0 (the figure is centred).
        let lo = a.bounds.min[1];
        let mut lods = Vec::new();
        for l in &a.lods {
            let mut mm = (**l).clone();
            for p in mm.positions.iter_mut() {
                p[1] -= lo;
            }
            lods.push(Arc::new(mm));
        }
        a.bounds = crate::math::Aabb::new([a.bounds.min[0], 0.0, a.bounds.min[2]], [a.bounds.max[0], a.bounds.max[1] - lo, a.bounds.max[2]]);
        a.lods = lods;
        a.rigged = Some((Arc::new(m), k, lo));
        vec![a.with_collider(ColliderShape::Cylinder { radius: 0.3, height: 1.75 })]
    });
    assets[0].clone()
}

fn add_character(cx: &mut Ctx, c: &CharJ) -> Result<(), String> {
    let srgb = |s: &str| -> Option<[u8; 3]> {
        let h = s.trim().strip_prefix('#')?;
        (h.len() == 6).then(|| [u8::from_str_radix(&h[0..2], 16).ok().unwrap_or(0), u8::from_str_radix(&h[2..4], 16).ok().unwrap_or(0), u8::from_str_radix(&h[4..6], 16).ok().unwrap_or(0)])
    };
    let mut a = character_asset(c.top.as_deref().and_then(srgb), c.skin.as_deref().and_then(srgb), if cx.high { 2 } else { 1 });
    if !crate::character::can_play(&a, &c.clip) {
        return Err(format!("character clip \"{}\": {}", c.clip, studio_mesh::motion::NAMES.join(", ")));
    }
    let scale = c.height / 1.75;
    a.name = c.name.clone();
    let ai = cx.world.add_asset(a);
    let y = cx.ground(c.at[0], c.at[1]);
    let oi = cx.world.place(&c.name, ai, ObjectKind::Character, Instance { pos: [c.at[0], y, c.at[1]], yaw: c.yaw, scale, rank: 0 }, true);
    cx.world.objects[oi].performance = Some(Performance { clip: c.clip.clone(), offset: 0.0, speed: 1.0, route: c.walk_to.clone(), pace: if c.walk_to.is_empty() { 0.0 } else if c.pace > 0.0 { c.pace } else if c.clip == "run" { 3.8 } else { 1.4 } });
    Ok(())
}

/// A readable summary of a world, for the AI and the status line.
pub fn describe(w: &World) -> String {
    let trees: usize = w.scatters.iter().map(|s| s.instances.len()).sum();
    let t = w.terrain.as_ref().map(|t| {
        let b = t.bounds();
        format!("{:.0} x {:.0} m terrain, {:.0} m of relief", t.size()[0], t.size()[1], b.max[1] - b.min[1])
    });
    format!(
        "{}{} scattered copies in {} groups, {} placed objects ({} buildings, {} characters), {} water surfaces, {} lights; sun {:.0} degrees up at {:.1} h; {:.1}M triangles in all",
        t.map(|s| s + ", ").unwrap_or_default(),
        trees,
        w.scatters.len(),
        w.objects.len(),
        w.objects.iter().filter(|o| o.kind == ObjectKind::Building).count(),
        w.objects.iter().filter(|o| o.kind == ObjectKind::Character).count(),
        w.water.len(),
        w.lights.len(),
        w.sky.sun_elevation(),
        w.sky.time,
        w.triangle_count() as f32 / 1e6
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fast(preset: &str, size: f32) -> WorldSpec {
        WorldSpec { preset: preset.into(), size: Some(size), quality: "fast".into(), seed: Some(5), ..WorldSpec::default() }
    }

    #[test]
    fn descriptions_pick_presets_and_times_of_day() {
        assert_eq!(guess_preset("a misty pine forest at dawn"), "forest");
        assert_eq!(guess_preset("a cyberpunk city street at night"), "city");
        assert_eq!(guess_preset("a small apartment living room"), "interior");
        assert_eq!(guess_preset("dunes and cacti, hot desert"), "desert");
        assert_eq!(guess_preset("sci-fi space station corridor"), "scifi");
        assert_eq!(guess_preset("snowy mountain pass"), "arctic");
        assert_eq!(time_from_words("golden hour glow"), Some(18.0));
        assert_eq!(time_from_words("a plain field"), None);
        assert!(Time::Name("teatime".into()).hours().is_err());
        assert_eq!(Time::Name("noon".into()).hours().unwrap(), 12.5);
    }

    #[test]
    fn a_forest_has_terrain_trees_water_and_a_place_to_stand() {
        let w = build(&fast("forest", 200.0), &NoLibrary).unwrap();
        assert!(w.terrain.is_some());
        let trees: usize = w.scatters.iter().filter(|s| s.name.contains("pine") || s.name.contains("tree")).map(|s| s.instances.len()).sum();
        assert!(trees > 200, "{}", describe(&w));
        assert!(!w.water.is_empty());
        // Every tree stands on the ground, none in the lake.
        for s in w.scatters.iter().filter(|s| s.name.contains("pine")) {
            for i in &s.instances {
                assert!((i.pos[1] - w.ground_y(i.pos[0], i.pos[2])).abs() < 0.01);
                assert!(w.water_at(i.pos[0], i.pos[2]).is_none_or(|l| i.pos[1] > l));
            }
        }
        // Spawn is dry ground, not inside anything.
        let sp = w.spawn;
        assert!((sp.position[1] - w.ground_y(sp.position[0], sp.position[2])).abs() < 0.3);
        assert!(w.water_at(sp.position[0], sp.position[2]).is_none_or(|l| sp.position[1] > l));
        // Deterministic.
        let w2 = build(&fast("forest", 200.0), &NoLibrary).unwrap();
        assert_eq!(describe(&w), describe(&w2));
    }

    #[test]
    fn the_spec_overrides_the_preset_and_the_sky_follows_the_time_in_words() {
        let mut s = fast("meadow", 160.0);
        s.description = "a quiet field at sunset".into();
        s.scatter = vec![ScatterJ { kind: "rock".into(), count: 30, size: 1.5, max_slope: 60.0, ..ScatterJ::default() }];
        let w = build(&s, &NoLibrary).unwrap();
        assert!(w.scatters.iter().all(|sc| sc.name.contains("rock")), "only the spec's rocks");
        assert!((w.sky.time - 19.0).abs() < 0.01);
        assert!(w.sky.sun_elevation() < 12.0);
        s.sky = Some(SkyJ { time: Some(Time::Name("noon".into())), ..SkyJ::default() });
        assert!(build(&s, &NoLibrary).unwrap().sky.sun_elevation() > 50.0);
        s.preset = "swamp".into();
        assert!(build(&s, &NoLibrary).unwrap_err().contains("preset"));
        let mut bad = fast("forest", 100.0);
        bad.scatter = vec![ScatterJ { kind: "unicorn".into(), ..ScatterJ::default() }];
        assert!(build(&bad, &NoLibrary).unwrap_err().contains("scatter kind"));
    }

    #[test]
    fn a_city_has_roads_towers_lamps_and_you_can_walk_the_streets_and_into_a_lobby() {
        let w = build(&fast("city", 320.0), &NoLibrary).unwrap();
        let towers = w.objects.iter().filter(|o| o.kind == ObjectKind::Building && o.name != "roads").count();
        assert!(towers >= 8, "{}", describe(&w));
        assert!(w.lights.len() > 20, "street lamps and windows light the dusk: {}", w.lights.len());
        // A dusk city: the sun is low.
        assert!(w.sky.sun_elevation() < 12.0);
        // Everything fits in the terrain.
        let t = w.terrain.as_ref().unwrap();
        for o in &w.objects {
            assert!(t.contains(o.inst.pos[0], o.inst.pos[2]), "{}", o.name);
        }
        let ex_spawn = w.spawn;
        assert!(w.ground_at(ex_spawn.position[0], ex_spawn.position[2], 5.0).is_some());
    }

    #[test]
    fn a_building_can_take_an_architectural_style() {
        let mut spec = fast("village", 60.0);
        spec.buildings = vec![BuildingJ { kind: "apartment".into(), at: Some([20.0, 10.0]), width: 10.0, depth: 8.0, floors: 3, style: Some("art_deco".into()), ..BuildingJ::default() }];
        let w = build(&spec, &NoLibrary).unwrap();
        let b = w.objects.iter().find(|o| o.kind == ObjectKind::Building).expect("a building");
        let asset = &w.assets[b.asset as usize];
        assert!(asset.lods[0].triangles() > 5_000, "modelled detail, not a box: {}", asset.lods[0].triangles());
        let (lo, hi) = asset.lods[0].bounds();
        assert!(lo[1] > -0.5 && hi[1] > 8.0, "metres, y up: {lo:?} {hi:?}");
        assert!(w.extra_solids.len() >= 1);
    }

    #[test]
    fn an_interior_is_connected_rooms_with_furniture_you_can_walk_through() {
        let w = build(&fast("interior", 60.0), &NoLibrary).unwrap();
        let rooms = w.objects.iter().filter(|o| o.kind == ObjectKind::Building).count();
        assert_eq!(rooms, 3);
        assert!(w.objects.iter().any(|o| o.kind == ObjectKind::Prop), "furnished");
        assert!(w.lights.len() >= 3);
        // Walk the length of the flat from one end to the other through the doors.
        let mut ex = crate::walk::Explorer::new(&w);
        ex.teleport(&w, w.spawn.position, 90.0);
        let route = crate::walk::Route { points: vec![[w.spawn.position[0] + 14.0, 0.0, 0.0]], sprint: false, pause: 0.0, arrive: 1.2 };
        let (secs, ok) = crate::walk::walk_route(&w, &mut ex, &route, 30.0, |_, _| {});
        assert!(ok, "reached the far room after {secs}s at {:?}", ex.body.pos);
    }

    #[test]
    fn scifi_desert_island_mountains_canyon_arctic_and_village_all_build() {
        for p in ["scifi", "desert", "island", "mountains", "canyon", "arctic", "village", "meadow"] {
            let w = build(&fast(p, 160.0), &NoLibrary).unwrap_or_else(|e| panic!("{p}: {e}"));
            assert!(w.terrain.is_some() && w.triangle_count() > 1000, "{p}: {}", describe(&w));
        }
        let island = build(&fast("island", 200.0), &NoLibrary).unwrap();
        assert!(island.water.iter().any(|w| w.max[0] - w.min[0] > 200.0));
        let sci = build(&fast("scifi", 160.0), &NoLibrary).unwrap();
        assert!(sci.sky.sun_elevation() < 0.0 && sci.look.exposure >= 0.5, "night is exposed up");
    }

    #[test]
    fn edits_pad_terrain_for_buildings_and_sculpt_and_paint_apply() {
        let mut s = fast("meadow", 160.0);
        s.buildings = vec![BuildingJ { kind: "house".into(), at: Some([20.0, 10.0]), width: 8.0, depth: 6.0, floors: 1, ..BuildingJ::default() }];
        s.sculpt = vec![SculptJ { brush: "raise".into(), at: [-40.0, -40.0], radius: 12.0, strength: 3.0, passes: 4, ..SculptJ::default() }];
        s.paint = vec![PaintJ { layer: "dirt".into(), at: [-40.0, 30.0], radius: 6.0, strength: 1.0 }];
        let w = build(&s, &NoLibrary).unwrap();
        let t = w.terrain.as_ref().unwrap();
        // Level under the house.
        let h = [(18.0, 8.0), (22.0, 8.0), (18.0, 12.0), (22.0, 12.0)].map(|(x, z)| t.height_at(x, z));
        assert!(h.iter().all(|v| (v - h[0]).abs() < 0.3), "{h:?}");
        assert!(t.height_at(-40.0, -40.0) > 8.0);
        assert!(t.weights_at(-40.0, 30.0)[2] > 0.9);
    }

    #[test]
    fn a_reference_look_recolours_the_land_and_sets_the_hour() {
        let day = hints_from_synthetic([90, 150, 230], [110, 120, 70], Some([60, 130, 40]));
        let dusk = hints_from_synthetic([230, 130, 70], [60, 40, 30], None);
        let mut a = fast("meadow", 120.0);
        a.look = Some(LookJ { hints: Some(HintsJ::from(&day)), ..LookJ::default() });
        let mut b = fast("meadow", 120.0);
        b.look = Some(LookJ { hints: Some(HintsJ::from(&dusk)), ..LookJ::default() });
        let (wa, wb) = (build(&a, &NoLibrary).unwrap(), build(&b, &NoLibrary).unwrap());
        assert!(wa.sky.time > 10.0 && wa.sky.time < 15.0, "{}", wa.sky.time);
        assert!(wb.sky.time > 16.0 && wb.sky.time < 20.0, "{}", wb.sky.time);
        assert_ne!(wa.terrain.as_ref().unwrap().layers[0].colour, wb.terrain.as_ref().unwrap().layers[0].colour);
        assert!(wb.atmosphere.colour.is_some());
    }

    fn hints_from_synthetic(sky: [u8; 3], ground: [u8; 3], tree: Option<[u8; 3]>) -> LookHints {
        let (w, h) = (64usize, 64usize);
        let mut px = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let c = if y < h / 2 { sky } else if tree.is_some() && x % 8 < 4 && y > h * 2 / 3 { tree.unwrap() } else { ground };
                px.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        look::hints_from_rgba(&px, w, h).unwrap()
    }

    #[test]
    fn a_materialized_spec_builds_the_same_world_and_parts_can_be_left_out() {
        let mut s = fast("forest", 160.0);
        s.description = "a forest at dawn".into();
        let plain = build(&s, &NoLibrary).unwrap();
        let m = materialize(&s).unwrap();
        assert!(!m.scatter.is_empty() && m.water.is_some() && m.preset == "forest" && m.size == Some(160.0));
        let again = build(&m, &NoLibrary).unwrap();
        assert_eq!(describe(&plain), describe(&again));
        assert_eq!(plain.sky.time, again.sky.time);
        // Remove the first scatter rule and the water by editing the lists.
        let mut e = m.clone();
        e.scatter.remove(0);
        e.water = Some(WaterJ { kind: "none".into(), ..Default::default() });
        let edited = build(&e, &NoLibrary).unwrap();
        assert!(edited.water.is_empty() && edited.scatters.len() < plain.scatters.len());
        // Or leave parts of the preset out without spelling it out.
        let mut w = s.clone();
        w.without = vec!["water".into(), "scatter".into()];
        let bare = build(&w, &NoLibrary).unwrap();
        assert!(bare.water.is_empty() && bare.scatters.is_empty());
        w.without = vec!["sky".into()];
        assert!(build(&w, &NoLibrary).unwrap_err().contains("without"));
    }

    #[test]
    fn high_quality_uses_the_modeled_kits_with_levels_of_detail() {
        let mut s = fast("meadow", 120.0);
        s.quality = "high".into();
        s.water = None;
        s.scatter = vec![ScatterJ { kind: "pine".into(), count: 12, size: 12.0, ..ScatterJ::default() }];
        let w = build(&s, &NoLibrary).unwrap();
        let a = &w.assets[w.scatters[0].asset];
        assert!(a.lods.len() >= 3, "{} levels", a.lods.len());
        assert!(a.lods[0].triangles() > 2000 && a.lods[0].triangles() > 6 * a.lods[2].triangles(), "{} vs {}", a.lods[0].triangles(), a.lods[2].triangles());
        assert!(a.bounds.max[1] > 9.0 && a.bounds.max[1] < 16.0, "{:?}", a.bounds);
        assert!(matches!(a.collider, ColliderShape::Cylinder { .. }));
    }

    #[test]
    fn the_spec_round_trips_through_json_and_tolerates_partial_input() {
        let s: WorldSpec = serde_json::from_str(r#"{"description":"a pine forest at dawn","scatter":[{"kind":"pine","count":50}],"sky":{"time":"dawn"},"buildings":[{"kind":"shop","at":[5,5]}]}"#).unwrap();
        assert_eq!(s.scatter[0].count, 50);
        assert_eq!(s.quality, "high");
        let back: WorldSpec = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(s, back);
        let sky = s.sky.unwrap();
        assert_eq!(sky.time.unwrap().hours().unwrap(), 6.0);
    }
}
