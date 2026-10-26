// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Script breakdown: reading each scene of the [`Story`] for everything it
//! needs (cast, locations, props, wardrobe, VFX, sound, vehicles, animals,
//! extras, stunts, make-up, music) and keeping those as tagged elements.
//!
//! [`Production::auto_tag`] tags from the script and replaces only what it
//! tagged before: tags added by hand ([`Tag::auto`] false) are kept.

use crate::{Production, slug};
use serde::{Deserialize, Serialize};
use crate::story::{LineKind, Scene, Story, TimeOfDay};
use crate::story::SECOND;

/// What kind of thing an element is. The colours are the usual breakdown
/// colours (cast red, extras green, props purple…).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Speaking and principal cast.
    Cast,
    /// Background people.
    Extras,
    /// Where it is shot.
    Location,
    /// Things people handle.
    Props,
    /// Clothes worn.
    Wardrobe,
    /// Make-up, hair, wounds, prosthetics.
    Makeup,
    /// Cars, boats, trains…
    Vehicles,
    /// Animals (not characters of the story).
    Animals,
    /// Stunts and fights.
    Stunts,
    /// Practical effects done on set.
    Sfx,
    /// Digital effects added in post.
    Vfx,
    /// Sound cues.
    Sound,
    /// Songs, score, dance.
    Music,
}

impl Category {
    /// Every category in sheet order.
    pub const ALL: [Category; 13] = [
        Category::Cast,
        Category::Extras,
        Category::Location,
        Category::Props,
        Category::Wardrobe,
        Category::Makeup,
        Category::Vehicles,
        Category::Animals,
        Category::Stunts,
        Category::Sfx,
        Category::Vfx,
        Category::Sound,
        Category::Music,
    ];

    /// The snake_case name.
    pub fn name(self) -> &'static str {
        match self {
            Category::Cast => "cast",
            Category::Extras => "extras",
            Category::Location => "location",
            Category::Props => "props",
            Category::Wardrobe => "wardrobe",
            Category::Makeup => "makeup",
            Category::Vehicles => "vehicles",
            Category::Animals => "animals",
            Category::Stunts => "stunts",
            Category::Sfx => "sfx",
            Category::Vfx => "vfx",
            Category::Sound => "sound",
            Category::Music => "music",
        }
    }

    /// Heading on a breakdown sheet.
    pub fn title(self) -> &'static str {
        match self {
            Category::Cast => "Cast",
            Category::Extras => "Extras",
            Category::Location => "Location",
            Category::Props => "Props",
            Category::Wardrobe => "Wardrobe",
            Category::Makeup => "Make-up / Hair",
            Category::Vehicles => "Vehicles",
            Category::Animals => "Animals",
            Category::Stunts => "Stunts",
            Category::Sfx => "Special effects",
            Category::Vfx => "Visual effects",
            Category::Sound => "Sound",
            Category::Music => "Music",
        }
    }

    /// The usual breakdown colour, as RGB.
    pub fn colour(self) -> [u8; 3] {
        match self {
            Category::Cast => [226, 70, 70],
            Category::Extras => [86, 184, 98],
            Category::Location => [120, 120, 130],
            Category::Props => [150, 90, 200],
            Category::Wardrobe => [225, 120, 190],
            Category::Makeup => [230, 170, 120],
            Category::Vehicles => [236, 140, 170],
            Category::Animals => [225, 140, 60],
            Category::Stunts => [235, 200, 50],
            Category::Sfx => [70, 160, 220],
            Category::Vfx => [60, 120, 230],
            Category::Sound => [70, 200, 210],
            Category::Music => [150, 210, 90],
        }
    }

    /// Reads a name, singular or plural, or a common way of saying it.
    pub fn parse(s: &str) -> Option<Category> {
        Some(match s.trim().to_lowercase().replace([' ', '-', '/'], "_").as_str() {
            "cast" | "actor" | "actors" | "character" | "characters" => Category::Cast,
            "extras" | "extra" | "background" | "atmosphere" | "crowd" => Category::Extras,
            "location" | "locations" | "set" | "sets" | "place" => Category::Location,
            "props" | "prop" | "set_dressing" => Category::Props,
            "wardrobe" | "costume" | "costumes" | "clothes" => Category::Wardrobe,
            "makeup" | "make_up" | "hair" | "makeup_hair" | "hair_makeup" => Category::Makeup,
            "vehicles" | "vehicle" | "cars" | "picture_cars" => Category::Vehicles,
            "animals" | "animal" | "livestock" | "creatures" => Category::Animals,
            "stunts" | "stunt" | "fights" => Category::Stunts,
            "sfx" | "special_effects" | "practical" | "practical_effects" => Category::Sfx,
            "vfx" | "visual_effects" | "cgi" | "digital" => Category::Vfx,
            "sound" | "sounds" | "sound_effects" | "audio" => Category::Sound,
            "music" | "songs" | "song" | "dance" => Category::Music,
            _ => return None,
        })
    }
}

/// One thing the film needs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Element {
    /// Stable id, `category:slug`.
    pub id: String,
    /// What kind.
    pub category: Category,
    /// Display name.
    pub name: String,
    /// Cast number (1 = first billed), for cast.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u32>,
    /// Anything the crew should know.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// The story's own id: the character or place this element stands for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub story_id: Option<String>,
}

impl Element {
    /// The id for a category and name.
    pub fn make_id(category: Category, name: &str) -> String {
        format!("{}:{}", category.name(), slug(name))
    }
}

/// An element needed in a scene.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tag {
    /// The scene id.
    pub scene: String,
    /// The element id.
    pub element: String,
    /// How many.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub qty: u32,
    /// Found in the script by [`Production::auto_tag`] (false: added by a
    /// person or the AI, and kept when the script is read again).
    #[serde(default)]
    pub auto: bool,
    /// Where it came from or what is wanted of it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

fn one() -> u32 {
    1
}
fn is_one(v: &u32) -> bool {
    *v == 1
}

/// Day or night for scheduling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DayNight {
    /// Dawn to golden hour.
    Day,
    /// Dusk and after dark.
    Night,
}

/// What the schedule and budget need to know about a scene.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneFacts {
    /// Scene id.
    pub id: String,
    /// Index in story order.
    pub index: usize,
    /// Slug line.
    pub heading: String,
    /// Inside.
    pub interior: bool,
    /// The set: place name, or the slug line's place.
    pub set: String,
    /// Place id when the story has one (else the slug of the set).
    pub place: String,
    /// Time of day.
    pub tod: TimeOfDay,
    /// Day or night.
    pub day_night: DayNight,
    /// Running time in seconds.
    pub secs: f64,
    /// Cast ids (story character ids) in the scene.
    pub cast: Vec<String>,
    /// One line of what happens.
    pub summary: String,
}

impl SceneFacts {
    /// Page length in eighths of a page (a page is a minute of screen time).
    pub fn eighths(&self) -> u32 {
        ((self.secs / 60.0 * 8.0).round() as u32).max(1)
    }

    /// `3 2/8`, `5/8`, `2`.
    pub fn pages(&self) -> String {
        pages_text(self.eighths())
    }

    /// `INT` or `EXT`.
    pub fn int_ext(&self) -> &'static str {
        if self.interior { "INT" } else { "EXT" }
    }

    /// `D` or `N`.
    pub fn dn(&self) -> &'static str {
        if self.day_night == DayNight::Day { "D" } else { "N" }
    }
}

/// How long a scene runs: its shots when it has them, else what its lines
/// take to say and do.
pub fn scene_secs(s: &Scene) -> f64 {
    let shots: f64 = s.shots.iter().map(|sh| sh.duration as f64 / SECOND as f64).sum();
    if shots > 0.0 {
        return shots;
    }
    let t: f64 = s
        .lines
        .iter()
        .map(|l| if l.is_spoken() { l.spoken_secs() } else { (l.text.split_whitespace().count() as f64 / 3.0).max(1.0) })
        .sum();
    t.max(3.0)
}

/// Facts of every scene, in story order.
pub fn facts(story: &Story) -> Vec<SceneFacts> {
    story.scenes.iter().enumerate().map(|(i, s)| scene_facts(story, i, s)).collect()
}

fn scene_facts(story: &Story, index: usize, s: &Scene) -> SceneFacts {
    let place = story.place_of(s);
    let head = s.heading.trim().to_uppercase();
    let head_int = head.starts_with("INT");
    let head_ext = head.starts_with("EXT");
    let interior = match (place.map(|p| p.interior), head_int, head_ext) {
        (_, true, false) => true,
        (_, false, true) => false,
        (Some(i), _, _) => i,
        _ => head_int,
    };
    // The set: the place's name, else the slug line between the INT./EXT.
    // and the time of day.
    let from_heading = || {
        let rest = head.trim_start_matches(|c: char| c.is_alphabetic() || c == '.' || c == '/').trim();
        rest.split(" - ").next().unwrap_or(rest).trim().to_owned()
    };
    let (set, place_id) = match place {
        Some(p) => (if p.name.is_empty() { p.id.clone() } else { p.name.clone() }, p.id.clone()),
        None => {
            let h = from_heading();
            let id = slug(&h);
            (if h.is_empty() { "Unplaced".to_owned() } else { h }, if id.is_empty() { "unplaced".to_owned() } else { id })
        }
    };
    let head_tod = head.rsplit(" - ").next().filter(|_| head.contains(" - ")).and_then(TimeOfDay::parse);
    let tod = s.time_of_day.or(head_tod).or(place.and_then(|p| p.time_of_day)).unwrap_or_default();
    let day_night = if matches!(tod, TimeOfDay::Dusk | TimeOfDay::Night) { DayNight::Night } else { DayNight::Day };
    let mut cast: Vec<String> = Vec::new();
    let mut add = |id: &str| {
        if let Some(c) = story.character(id) {
            if !cast.contains(&c.id) {
                cast.push(c.id.clone());
            }
        }
    };
    for c in &s.cast {
        add(c);
    }
    for l in &s.lines {
        if let Some(w) = &l.who {
            add(w);
        }
    }
    for sh in &s.shots {
        for sub in &sh.subjects {
            add(sub);
        }
    }
    let summary = if s.summary.is_empty() { s.lines.iter().find(|l| l.kind == LineKind::Action).map(|l| l.text.clone()).unwrap_or_default() } else { s.summary.clone() };
    SceneFacts { id: s.id.clone(), index, heading: s.heading.clone(), interior, set, place: place_id, tod, day_night, secs: scene_secs(s), cast, summary }
}

// --------------------------------------------------------------- lexicons

const PROPS: &[&str] = &[
    "lantern", "sword", "letter", "book", "phone", "telephone", "key", "keys", "cup", "teacup", "gun", "pistol", "knife", "dagger", "map", "ring", "suitcase", "bag", "backpack", "umbrella", "candle", "mirror", "photograph",
    "photo", "necklace", "flute", "guitar", "camera", "lamp", "basket", "bottle", "coin", "coins", "diary", "journal", "envelope", "newspaper", "radio", "clock", "watch", "treasure", "chest", "box", "scroll", "torch", "rope",
    "ladder", "flag", "bouquet", "flowers", "tray", "plate", "bowl", "glass", "laptop", "tablet", "briefcase", "wallet", "money", "cash", "locket", "compass", "telescope", "binoculars", "crown", "wand", "staff", "shield",
    "bow", "arrow", "spear", "axe", "hammer", "broom", "kite", "ball", "toy", "doll", "balloon", "cake", "diya", "garland", "tabla", "sitar", "harmonium", "dhol", "chai", "thali", "pot", "pan", "mug",
];
const WARDROBE: &[&str] = &[
    "coat", "cape", "cloak", "dress", "gown", "saree", "sari", "sherwani", "lehenga", "turban", "uniform", "hat", "scarf", "gloves", "shawl", "helmet", "armor", "armour", "crown", "mask", "boots", "kurta", "dupatta", "suit",
    "robe", "jacket", "hoodie", "sweater", "glasses", "sunglasses", "veil", "bandana", "apron", "costume", "jeans", "sneakers", "shoes", "sandals", "tie", "cap", "dhoti", "pagdi", "bindi",
];
const VEHICLES: &[&str] = &[
    "car", "truck", "bus", "train", "boat", "ship", "bicycle", "bike", "motorcycle", "motorbike", "rickshaw", "taxi", "jeep", "helicopter", "plane", "airplane", "aeroplane", "carriage", "cart", "tractor", "canoe", "van",
    "ambulance", "scooter", "tram", "ferry", "yacht", "rocket", "spaceship", "limousine", "limo", "chariot", "sled", "wagon", "lorry",
];
const ANIMALS: &[&str] = &[
    "dog", "cat", "horse", "bird", "owl", "deer", "wolf", "rabbit", "cow", "elephant", "tiger", "lion", "snake", "monkey", "eagle", "crow", "parrot", "goat", "sheep", "bull", "camel", "fish", "bear", "mouse", "frog",
    "butterfly", "bee", "peacock", "pigeon", "duck", "chicken", "hen", "rooster", "pig", "donkey", "dolphin", "whale", "shark", "squirrel", "hawk", "fox", "puppy", "kitten", "lizard", "spider", "crocodile", "buffalo", "ox",
];
const VFX: &[&str] = &[
    "magic", "magical", "spell", "glow", "glows", "glowing", "portal", "hologram", "transform", "transforms", "teleport", "teleports", "flies", "flying", "levitate", "levitates", "lightning", "disintegrate", "energy", "sparkle",
    "sparkles", "shockwave", "morph", "particles", "monster", "dragon", "ghost", "spirit", "invisible", "vanishes", "vanish", "cgi", "hovers", "floats", "supernatural", "aurora", "comet",
];
const SFX: &[&str] = &["rain", "fire", "flames", "smoke", "fog", "mist", "wind", "snow", "storm", "explosion", "explodes", "sparks", "dust", "steam", "flood", "burning", "fireworks", "blast"];
const SOUND: &[&str] = &[
    "thunder", "siren", "bell", "bells", "knock", "knocks", "gunshot", "gunshots", "scream", "screams", "crash", "crashes", "bark", "barks", "howl", "howls", "roar", "roars", "honk", "whistle", "echo", "echoes", "drum", "drums",
    "footsteps", "ringing", "rings", "creak", "creaks", "slam", "slams", "buzz", "explosion", "shatters", "splash", "laughter", "applause", "whispers", "chime", "chirping", "ticking",
];
const MUSIC: &[&str] = &["song", "sings", "sing", "singing", "dance", "dances", "dancing", "score", "lullaby", "music", "qawwali", "anthem", "melody", "choir", "orchestra", "serenade", "tune", "hums", "humming"];
const STUNTS: &[&str] = &["fight", "fights", "fighting", "punch", "punches", "plunges", "jumps", "leaps", "chase", "chases", "swordfight", "duel", "drowning", "dives", "tumbles", "crashes", "kicks", "wrestle", "ambush", "climbs"];
const MAKEUP: &[&str] = &["wound", "wounded", "bruise", "bruised", "blood", "bleeding", "bleeds", "scar", "tattoo", "mehndi", "henna", "aged", "beard", "wig", "prosthetic", "makeup", "sweat", "sweating", "dirty", "muddy", "tears", "injured"];
const EXTRAS: &[&str] = &[
    "crowd", "villagers", "soldiers", "audience", "passersby", "mob", "guests", "students", "customers", "townspeople", "travellers", "travelers", "worshippers", "dancers", "spectators", "shoppers", "bystanders",
    "pedestrians", "patrons", "refugees", "servants", "guards", "children",
];

fn lexicon() -> [(&'static [&'static str], Category); 10] {
    [
        (PROPS, Category::Props),
        (WARDROBE, Category::Wardrobe),
        (VEHICLES, Category::Vehicles),
        (ANIMALS, Category::Animals),
        (VFX, Category::Vfx),
        (SFX, Category::Sfx),
        (SOUND, Category::Sound),
        (MUSIC, Category::Music),
        (STUNTS, Category::Stunts),
        (MAKEUP, Category::Makeup),
    ]
}

/// The words of a text, lower-cased, without punctuation.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '\'').filter(|w| !w.is_empty()).map(|w| w.trim_matches('\'').to_lowercase()).collect()
}

fn singular(w: &str) -> Option<&str> {
    w.strip_suffix("es").filter(|s| s.len() > 2).or_else(|| w.strip_suffix('s').filter(|s| s.len() > 2))
}

/// A thing found in text: its category and the word that named it.
pub fn scan(text: &str) -> Vec<(Category, String)> {
    let mut found: Vec<(Category, String)> = Vec::new();
    let lex = lexicon();
    for w in words(text) {
        let cand = [Some(w.as_str()), singular(&w)];
        let mut hit: Option<(Category, String)> = None;
        'find: for c in cand.into_iter().flatten() {
            if EXTRAS.contains(&c) {
                hit = Some((Category::Extras, c.to_owned()));
                break 'find;
            }
            for (list, cat) in &lex {
                if list.contains(&c) {
                    hit = Some((*cat, c.to_owned()));
                    break 'find;
                }
            }
        }
        if let Some((cat, name)) = hit {
            // Some words belong to several departments: an explosion is a
            // practical effect and a sound; crashes are sound and stunt.
            let also: &[Category] = match name.as_str() {
                "explosion" | "explodes" | "blast" => &[Category::Sound],
                "crash" | "crashes" => &[Category::Sound, Category::Stunts],
                _ => &[],
            };
            for c in std::iter::once(cat).chain(also.iter().copied()) {
                if !found.iter().any(|(fc, n)| *fc == c && *n == name) {
                    found.push((c, name.clone()));
                }
            }
        }
    }
    found
}

/// What `auto_tag` did.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct TagReport {
    /// Scenes read.
    pub scenes: usize,
    /// Tags now from the script.
    pub tags: usize,
    /// Elements now in the breakdown.
    pub elements: usize,
    /// Hand-made tags that were kept.
    pub kept_manual: usize,
}

impl Production {
    /// Finds or makes an element.
    pub fn element_mut(&mut self, category: Category, name: &str, story_id: Option<&str>) -> &mut Element {
        let id = Element::make_id(category, name);
        if let Some(i) = self.elements.iter().position(|e| e.id == id) {
            return &mut self.elements[i];
        }
        self.elements.push(Element { id, category, name: name.to_owned(), number: None, notes: String::new(), story_id: story_id.map(str::to_owned) });
        self.elements.last_mut().unwrap()
    }

    /// An element by id or name.
    pub fn element(&self, key: &str) -> Option<&Element> {
        self.elements.iter().find(|e| e.id == key).or_else(|| self.elements.iter().find(|e| e.name.eq_ignore_ascii_case(key)))
    }

    /// The elements of a category.
    pub fn of(&self, c: Category) -> impl Iterator<Item = &Element> {
        self.elements.iter().filter(move |e| e.category == c)
    }

    /// Reads the script and tags each scene with what it needs.
    ///
    /// Tags made by an earlier read are replaced; hand-made tags stay.
    /// Elements nothing uses any more (and that a person never edited) go.
    pub fn auto_tag(&mut self, story: &Story) -> TagReport {
        let kept: Vec<Tag> = self.tags.iter().filter(|t| !t.auto).cloned().collect();
        self.tags = kept;
        let mut report = TagReport { scenes: story.scenes.len(), kept_manual: self.tags.len(), ..Default::default() };
        let all = facts(story);
        let mut found: Vec<Tag> = Vec::new();
        // Words that are characters or places of the story are not props.
        let own = |w: &str| story.character(w).is_some() || story.place(w).is_some();
        for (scene, f) in story.scenes.iter().zip(&all) {
            // Cast, in order of first appearance across the film.
            for id in &f.cast {
                let c = story.character(id).unwrap();
                let name = if c.name.is_empty() { c.id.clone() } else { c.name.clone() };
                let e = self.element_mut(Category::Cast, &name, Some(&c.id));
                let eid = e.id.clone();
                found.push(Tag { scene: f.id.clone(), element: eid, qty: 1, auto: true, note: String::new() });
            }
            // The location.
            let e = self.element_mut(Category::Location, &f.set, Some(&f.place));
            let eid = e.id.clone();
            found.push(Tag { scene: f.id.clone(), element: eid, qty: 1, auto: true, note: format!("{} {}", f.int_ext(), f.dn()) });
            // Everything the words of the scene ask for.
            let mut texts: Vec<(String, String)> = Vec::new();
            if !scene.summary.is_empty() {
                texts.push((scene.summary.clone(), "summary".into()));
            }
            for l in &scene.lines {
                if l.kind == LineKind::Action {
                    texts.push((l.text.clone(), "action".into()));
                }
            }
            for sh in &scene.shots {
                for t in [&sh.action, &sh.framing, &sh.notes] {
                    if !t.is_empty() {
                        texts.push((t.clone(), format!("shot {}", sh.id)));
                    }
                }
            }
            let mut counts: Vec<(Category, String, u32, String)> = Vec::new();
            for (text, src) in &texts {
                for (cat, name) in scan(text) {
                    if own(&name) {
                        continue;
                    }
                    match counts.iter_mut().find(|(c, n, _, _)| *c == cat && *n == name) {
                        Some(x) => x.2 += 1,
                        None => counts.push((cat, name, 1, src.clone())),
                    }
                }
            }
            // Things named as subjects of a shot that are not cast.
            for sh in &scene.shots {
                for sub in &sh.subjects {
                    if story.character(sub).is_some() || story.place(sub).is_some() {
                        continue;
                    }
                    let hits = scan(sub);
                    if hits.is_empty() {
                        let name = sub.trim().to_lowercase();
                        if !counts.iter().any(|(c, n, _, _)| *c == Category::Props && *n == name) {
                            counts.push((Category::Props, name, 1, format!("shot {}", sh.id)));
                        }
                    }
                    for (cat, name) in hits {
                        if !own(&name) && !counts.iter().any(|(c, n, _, _)| *c == cat && *n == name) {
                            counts.push((cat, name, 1, format!("shot {}", sh.id)));
                        }
                    }
                }
            }
            for (cat, name, n, src) in counts {
                let e = self.element_mut(cat, &name, None);
                let eid = e.id.clone();
                found.push(Tag { scene: f.id.clone(), element: eid, qty: n.min(9), auto: true, note: format!("from {src}") });
            }
        }
        // A tag by hand wins over the same tag from the script.
        found.retain(|t| !self.tags.iter().any(|m| m.scene == t.scene && m.element == t.element));
        report.tags = found.len();
        self.tags.extend(found);
        // Number the cast by first appearance, drop elements nothing uses.
        let tags = &self.tags;
        self.elements.retain(|e| tags.iter().any(|t| t.element == e.id) || !e.notes.is_empty());
        let order: Vec<String> = story.scenes.iter().map(|s| s.id.clone()).collect();
        let first = |eid: &str| self.tags.iter().filter(|t| t.element == eid).filter_map(|t| order.iter().position(|s| *s == t.scene)).min().unwrap_or(usize::MAX);
        let mut cast: Vec<(usize, usize, String)> = self.elements.iter().enumerate().filter(|(_, e)| e.category == Category::Cast).map(|(i, e)| (first(&e.id), i, e.id.clone())).collect();
        cast.sort();
        for (n, (_, _, id)) in cast.into_iter().enumerate() {
            if let Some(e) = self.elements.iter_mut().find(|e| e.id == id) {
                e.number = Some(n as u32 + 1);
            }
        }
        self.elements.sort_by_key(|e| (e.category, e.number.unwrap_or(u32::MAX), e.name.to_lowercase()));
        report.elements = self.elements.len();
        report
    }

    /// Adds a tag by hand (making the element if it is new).
    pub fn tag(&mut self, scene: &str, category: Category, name: &str, qty: u32, note: &str) -> Result<(), String> {
        if name.trim().is_empty() {
            return Err("a tag needs a name".into());
        }
        let eid = self.element_mut(category, name.trim(), None).id.clone();
        match self.tags.iter_mut().find(|t| t.scene == scene && t.element == eid) {
            Some(t) => {
                t.qty = qty.max(1);
                t.auto = false;
                if !note.is_empty() {
                    t.note = note.to_owned();
                }
            }
            None => self.tags.push(Tag { scene: scene.to_owned(), element: eid, qty: qty.max(1), auto: false, note: note.to_owned() }),
        }
        Ok(())
    }

    /// Takes an element out of a scene. A tag found in the script is kept
    /// as dismissed (quantity 0) so reading the script again does not bring
    /// it back; a tag made by hand is deleted.
    pub fn untag(&mut self, scene: &str, element: &str) -> bool {
        let Some(id) = self.element(element).map(|e| e.id.clone()) else { return false };
        let Some(i) = self.tags.iter().position(|t| t.scene == scene && t.element == id && t.qty > 0) else { return false };
        if self.tags[i].auto {
            let t = &mut self.tags[i];
            t.auto = false;
            t.qty = 0;
        } else {
            self.tags.remove(i);
        }
        true
    }

    /// The elements tagged in a scene, by category.
    pub fn in_scene(&self, scene: &str) -> Vec<(&Element, &Tag)> {
        let mut v: Vec<(&Element, &Tag)> = self.tags.iter().filter(|t| t.scene == scene && t.qty > 0).filter_map(|t| self.elements.iter().find(|e| e.id == t.element).map(|e| (e, t))).collect();
        v.sort_by_key(|(e, _)| (e.category, e.number.unwrap_or(u32::MAX), e.name.to_lowercase()));
        v
    }

    /// The scenes an element is in.
    pub fn scenes_of(&self, element: &str) -> Vec<&str> {
        self.tags.iter().filter(|t| t.element == element && t.qty > 0).map(|t| t.scene.as_str()).collect()
    }

    /// The breakdown sheet of one scene.
    pub fn sheet(&self, story: &Story, scene: &str) -> Option<SceneSheet> {
        let f = facts(story).into_iter().find(|f| f.id.eq_ignore_ascii_case(scene))?;
        let mut groups: Vec<(Category, Vec<String>)> = Vec::new();
        for (e, t) in self.in_scene(&f.id) {
            let label = match (e.category, e.number) {
                (Category::Cast, Some(n)) => format!("{n}. {}", e.name),
                _ if t.qty > 1 => format!("{} x{}", e.name, t.qty),
                _ => e.name.clone(),
            };
            match groups.iter_mut().find(|(c, _)| *c == e.category) {
                Some(g) => g.1.push(label),
                None => groups.push((e.category, vec![label])),
            }
        }
        Some(SceneSheet { facts: f, groups })
    }
}

/// A breakdown sheet: a scene's facts and what it needs, by category.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneSheet {
    /// The scene.
    pub facts: SceneFacts,
    /// What it needs, category by category, in sheet order.
    pub groups: Vec<(Category, Vec<String>)>,
}

/// Eighths of a page as pages are written: `5/8`, `2`, `3 2/8`.
pub fn pages_text(e: u32) -> String {
    match (e / 8, e % 8) {
        (0, r) => format!("{r}/8"),
        (p, 0) => p.to_string(),
        (p, r) => format!("{p} {r}/8"),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::story::{Character, Line, Place, Shot};

    pub(crate) fn story() -> Story {
        let mut st = Story { title: "The Lantern".into(), ..Default::default() };
        st.cast.push(Character { id: "fern".into(), name: "Fern the Fox".into(), ..Default::default() });
        st.cast.push(Character { id: "asha".into(), name: "Asha".into(), ..Default::default() });
        st.places.push(Place { id: "meadow".into(), name: "Meadow".into(), ..Default::default() });
        st.places.push(Place { id: "hut".into(), name: "Asha's Hut".into(), interior: true, ..Default::default() });
        let act = |t: &str| Line { text: t.into(), ..Default::default() };
        let say = |w: &str, t: &str| Line { kind: LineKind::Dialogue, who: Some(w.into()), text: t.into(), ..Default::default() };
        st.scenes.push(Scene {
            id: "1".into(),
            heading: "EXT. MEADOW - DAWN".into(),
            place: Some("meadow".into()),
            cast: vec!["fern".into()],
            lines: vec![act("Fern carries a lantern across the meadow. Thunder rolls as rain falls."), say("fern", "Who is there?")],
            shots: vec![Shot { id: "1A".into(), duration: 6 * SECOND, ..Default::default() }, Shot { id: "1B".into(), duration: 4 * SECOND, subjects: vec!["fern".into(), "silver pendant".into()], ..Default::default() }],
            ..Default::default()
        });
        st.scenes.push(Scene {
            id: "2".into(),
            heading: "INT. ASHA'S HUT - NIGHT".into(),
            place: Some("hut".into()),
            lines: vec![act("Asha wearing a shawl lights a candle. A dragon glows in the smoke. Villagers sing a song."), say("asha", "Come in, little fox.")],
            shots: vec![Shot { id: "2A".into(), duration: 20 * SECOND, ..Default::default() }],
            ..Default::default()
        });
        st
    }

    #[test]
    fn the_script_is_broken_down_by_scene() {
        let st = story();
        let mut p = Production::default();
        let r = p.auto_tag(&st);
        assert_eq!(r.scenes, 2);
        let s1 = p.sheet(&st, "1").unwrap();
        let get = |s: &SceneSheet, c: Category| s.groups.iter().find(|g| g.0 == c).map(|g| g.1.clone()).unwrap_or_default();
        assert_eq!(get(&s1, Category::Cast), vec!["1. Fern the Fox"]);
        assert_eq!(get(&s1, Category::Location), vec!["Meadow"]);
        assert!(get(&s1, Category::Props).contains(&"lantern".to_owned()), "{:?}", s1.groups);
        assert!(get(&s1, Category::Props).contains(&"silver pendant".to_owned()), "a shot subject that is no character is a prop");
        assert!(get(&s1, Category::Sound).contains(&"thunder".to_owned()));
        assert!(get(&s1, Category::Sfx).contains(&"rain".to_owned()));
        assert!(get(&s1, Category::Animals).is_empty(), "fox is a character, not an animal extra");
        let s2 = p.sheet(&st, "2").unwrap();
        assert_eq!(get(&s2, Category::Cast), vec!["2. Asha"], "Asha is found from her dialogue; Fern's words are not hers");
        assert!(get(&s2, Category::Wardrobe).contains(&"shawl".to_owned()));
        assert!(get(&s2, Category::Vfx).contains(&"dragon".to_owned()));
        assert!(get(&s2, Category::Extras).contains(&"villagers".to_owned()));
        assert!(get(&s2, Category::Music).contains(&"song".to_owned()));
        assert_eq!(s2.facts.int_ext(), "INT");
        assert_eq!(s2.facts.dn(), "N");
        assert_eq!(s1.facts.dn(), "D");
    }

    #[test]
    fn reading_again_keeps_what_a_person_added() {
        let st = story();
        let mut p = Production::default();
        p.auto_tag(&st);
        p.tag("1", Category::Props, "silver bell", 2, "Asha's gift").unwrap();
        let before = p.tags.len();
        let r = p.auto_tag(&st);
        assert_eq!(r.kept_manual, 1);
        assert_eq!(p.tags.len(), before, "the same tags again, none doubled");
        assert!(p.sheet(&st, "1").unwrap().groups.iter().any(|g| g.1.contains(&"silver bell x2".to_owned())));
        assert!(p.untag("1", "silver bell"));
        assert!(!p.untag("1", "silver bell"));
    }

    #[test]
    fn pages_come_from_screen_time() {
        let st = story();
        let f = facts(&st);
        assert_eq!(f[0].secs, 10.0);
        assert_eq!(f[0].pages(), "1/8");
        assert_eq!(f[1].pages(), "3/8");
    }
}
