// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What things are made of, for the 3D critic: parts by kind of subject
//! (devices, vehicles, characters, animals, furniture, buildings,
//! containers and tools, plants), the parts every kind of object has, words
//! that name the same part, and the way people list parts in a brief or a
//! plan ("with a hinged lid and a light strip", "parts: body, lid, button").
//!
//! A part word counts for a subject only in the domains the subject is in,
//! so "arm" is a part of a character or a chair but not of a phone, and a
//! sentence saying "back to black" names no part of anything. Parts listed
//! outright ("parts: …", "made of …", "components: …") count whatever they
//! are.
//!
//! In the lists, `*` after a word marks a part that moves, `~` one that
//! gives off light.

use super::{stem, tokens};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// Parts of any object.
const UNIVERSAL: &str = "lid* cover* door* flap* hatch* hinge* drawer* lever* knob* dial* button* switch* handle strap buckle hook latch* lens bezel antenna nozzle spout cap* label window wheel* blade* gate* shutter*";

struct Domain {
    name: &'static str,
    cues: &'static str,
    parts: &'static str,
}

const DOMAINS: &[Domain] = &[
    Domain {
        name: "device",
        cues: "phone earbud earphone headphone watch laptop tablet speaker camera remote console controller keyboard mouse charger case drone gadget device product wearable",
        parts: "screen~ display~ strip~ led~ indicator~ light~ ring~ earbud bud tip stem speaker grille microphone mic charging contact crown* band key* trackpad camera antenna pocket cradle",
    },
    Domain {
        name: "vehicle",
        cues: "car truck van bus bike bicycle motorcycle scooter plane aircraft jet helicopter boat ship train tractor rocket spaceship vehicle sedan taxi",
        parts: "tyre tire rim windscreen windshield hood* bonnet* trunk* boot* bumper headlight~ headlamp~ taillight~ indicator~ mirror grille fuselage tail engine exhaust seat steering* handlebar* pedal* chain fender spoiler cockpit canopy* hull mast sail* rudder* gear fin propeller* rotor* wing body chassis roof",
    },
    Domain {
        name: "character",
        cues: "character person human man woman boy girl robot hero avatar mannequin figure monster humanoid giant",
        parts: "head torso body arm hand finger leg foot toe eye ear nose mouth hair neck shoulder hip knee elbow jaw brow belt shirt jacket trousers shoe boot hat helmet visor backpack cape tail face",
    },
    Domain {
        name: "animal",
        cues: "animal dog cat horse bird fish dragon creature rabbit bear wolf fox lion tiger elephant cow pig sheep deer snake turtle frog owl eagle shark dolphin whale insect bee butterfly spider",
        parts: "head body tail leg paw claw ear eye snout muzzle nose mouth tooth horn antler wing* beak fin mane fur hoof shell tongue trunk tusk whisker feather scale",
    },
    Domain {
        name: "furniture",
        cues: "chair table desk sofa couch bed shelf cabinet wardrobe bench stool lamp bookcase dresser cupboard furniture armchair",
        parts: "leg seat backrest back armrest arm cushion shelf top tabletop frame mattress headboard shade bulb~ base foot panel rail",
    },
    Domain {
        name: "building",
        cues: "house building tower castle church barn cabin hut temple skyscraper bridge lighthouse shop station hotel hall",
        parts: "roof wall chimney stairs step balcony column pillar arch porch gable dome spire tower floor fence shutter* awning lantern~ foundation",
    },
    Domain {
        name: "container",
        cues: "bottle jar mug cup bag box crate barrel bucket kettle teapot pot pan knife sword hammer axe key gun tool wrench pen",
        parts: "neck cork stopper hilt guard pommel shaft trigger* barrel grip head body base rim",
    },
    Domain {
        name: "plant",
        cues: "tree plant flower bush cactus mushroom vine",
        parts: "trunk branch leaf root petal stem bud fruit pot",
    },
];

/// Words naming one part.
const ALIASES: &[&[&str]] = &[
    &["wheel", "tyre", "tire", "rim"],
    &["windscreen", "windshield", "window"],
    &["hood", "bonnet"],
    &["trunk", "boot"],
    &["headlight", "headlamp"],
    &["leg", "thigh", "shin", "calf"],
    &["arm", "forearm", "upper_arm"],
    &["hand", "palm"],
    &["backrest", "back"],
    &["tabletop", "top"],
    &["lid", "cover"],
    &["display", "screen"],
    &["earbud", "earphone", "bud"],
    &["mic", "microphone"],
    &["snout", "muzzle"],
    &["steering", "steer"],
    &["handlebar", "handlebars"],
    &["strip", "led", "indicator"],
    &["body", "torso", "chest", "spine", "hip"],
    &["foot", "feet", "toe"],
    &["tooth", "teeth", "tusk"],
];

/// Words that are not parts of anything however they are listed: colours,
/// finishes, shapes, and the vocabulary of shots.
const NOT_PARTS: &str = "shape look style finish feel size edge corner colour color black white grey gray red blue green yellow orange pink purple coral brown gold silver matte glossy soft hard smooth rough rounded round pill big small large little tiny huge premium modern clean minimal sleek real detail details quality orbit move push pull camera lens shot scene light lighting shadow depth field focus rim key fill beat music sound motion speed second seconds frame film video model object product thing things way kind type sort part parts piece pieces one two three four pair set";

pub struct Info {
    /// stem → (moves, glows)
    pub all: HashMap<String, (bool, bool)>,
}

fn read(list: &str, into: &mut HashMap<String, (bool, bool)>) {
    for w in list.split_whitespace() {
        let (word, moves, glows) = (w.trim_end_matches(['*', '~']), w.ends_with('*'), w.ends_with('~'));
        let e = into.entry(stem(word)).or_insert((false, false));
        e.0 |= moves;
        e.1 |= glows;
    }
}

fn universal() -> &'static HashMap<String, (bool, bool)> {
    static U: OnceLock<HashMap<String, (bool, bool)>> = OnceLock::new();
    U.get_or_init(|| {
        let mut m = HashMap::new();
        read(UNIVERSAL, &mut m);
        m
    })
}

fn domain_parts() -> &'static Vec<(&'static str, HashSet<String>, HashMap<String, (bool, bool)>)> {
    static D: OnceLock<Vec<(&'static str, HashSet<String>, HashMap<String, (bool, bool)>)>> = OnceLock::new();
    D.get_or_init(|| {
        DOMAINS
            .iter()
            .map(|d| {
                let mut parts = HashMap::new();
                read(d.parts, &mut parts);
                (d.name, d.cues.split_whitespace().map(stem).collect(), parts)
            })
            .collect()
    })
}

fn not_parts() -> &'static HashSet<String> {
    static N: OnceLock<HashSet<String>> = OnceLock::new();
    N.get_or_init(|| NOT_PARTS.split_whitespace().map(stem).collect())
}

/// The kinds of thing the words describe.
pub fn domains_of(words: &[String]) -> Vec<&'static str> {
    domain_parts().iter().filter(|(_, cues, _)| words.iter().any(|w| cues.contains(w))).map(|d| d.0).collect()
}

/// Whether `word` (a stem) is a part of a thing in `domains` (or of any
/// thing): (moves, glows).
pub fn lookup(word: &str, domains: &[&str]) -> Option<(bool, bool)> {
    if let Some(v) = universal().get(word) {
        return Some(*v);
    }
    domain_parts().iter().filter(|(n, ..)| domains.contains(n)).find_map(|(_, _, p)| p.get(word).copied())
}

/// Whether `word` is a part word in any domain.
pub fn known_anywhere(word: &str) -> Option<(bool, bool)> {
    universal().get(word).copied().or_else(|| domain_parts().iter().find_map(|(_, _, p)| p.get(word).copied()))
}

/// The word as people write it, for a note.
pub fn display(word: &str) -> String {
    let find = |list: &str| list.split_whitespace().map(|w| w.trim_end_matches(['*', '~'])).find(|w| stem(w) == word).map(str::to_owned);
    find(UNIVERSAL).or_else(|| DOMAINS.iter().find_map(|d| find(d.parts))).unwrap_or_else(|| word.to_owned())
}

/// Whether two stems name the same part.
pub fn same_part(a: &str, b: &str) -> bool {
    a == b || ALIASES.iter().any(|g| g.iter().any(|x| stem(x) == a) && g.iter().any(|x| stem(x) == b))
}

/// Whether any of `names` (bone, material and part names) has a token that
/// names the part `word`.
pub fn present_in(word: &str, names: &[String]) -> bool {
    names.iter().flat_map(|n| tokens(n)).any(|t| same_part(word, &t))
}

const LIST_CUES: &[&str] = &["parts:", "parts -", "components:", "pieces:", "made of ", "made up of ", "built from ", "built of ", "consists of ", "composed of ", "comprises ", "includes ", "including ", "featuring ", "features ", "fitted with ", "complete with ", "has a ", "has an ", "has two ", "has four ", "have a "];

fn head_nouns(items: &str, keep: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut out = Vec::new();
    for item in items.split([',', ';', '&', '+', '/']).flat_map(|i| i.split(" and ")) {
        let words: Vec<String> = item.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty() && !w.starts_with(|c: char| c.is_ascii_digit()) && !(w.starts_with('x') && w[1..].chars().all(|c| c.is_ascii_digit()) && w.len() > 1)).map(stem).collect();
        // "a pair of wheels", "2 wheels", "the light strip": the last word.
        if words.is_empty() || words.len() > 4 {
            continue;
        }
        let Some(head) = words.last() else { continue };
        if head.len() >= 3 && !head.chars().all(|c| c.is_ascii_digit()) && !not_parts().contains(head) && keep(head) {
            out.push(head.clone());
        }
    }
    out
}

/// Parts a sentence lists: after "parts:", "made of", "includes"… any
/// noun counts; in "with a hinged lid and a light strip" and in a
/// bracket after the subject, the nouns that are parts of something.
pub fn listed(sentence: &str) -> Vec<String> {
    let low = sentence.to_lowercase();
    let mut out = Vec::new();
    for cue in LIST_CUES {
        if let Some(i) = low.find(cue) {
            let tail = &low[i + cue.len()..];
            let end = tail.find([')', '.']).unwrap_or(tail.len());
            out.extend(head_nouns(&tail[..end], &|_| true));
        }
    }
    if let Some(i) = low.find(" with ") {
        let tail = &low[i + 6..];
        let end = tail.find([')', '.', ';']).unwrap_or(tail.len());
        out.extend(head_nouns(&tail[..end], &|w| known_anywhere(w).is_some()));
    }
    let mut rest = low.as_str();
    while let Some(i) = rest.find('(') {
        let tail = &rest[i + 1..];
        let end = tail.find(')').unwrap_or(tail.len());
        out.extend(head_nouns(&tail[..end], &|w| known_anywhere(w).is_some()));
        rest = &tail[end..];
    }
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_belong_to_the_kind_of_thing_they_are_parts_of() {
        let car = domains_of(&tokens("sports car"));
        assert_eq!(car, vec!["vehicle"]);
        assert!(lookup(&stem("tyres"), &car).is_some() && lookup(&stem("arm"), &car).is_none());
        assert_eq!(lookup(&stem("headlights"), &car), Some((false, true)));
        assert_eq!(lookup("lid", &[]), Some((true, false)));
        let chair = domains_of(&tokens("wooden chair"));
        assert!(lookup("arm", &chair).is_some() && lookup(&stem("legs"), &chair).is_some());
        assert!(lookup("wing", &domains_of(&tokens("dragon"))).is_some());
        assert!(lookup("roof", &domains_of(&tokens("castle"))).is_some());
        assert!(same_part(&stem("tyre"), &stem("wheel")) && same_part(&stem("windshield"), &stem("windscreen")) && !same_part(&stem("lid"), &stem("wheel")));
    }

    #[test]
    fn parts_are_read_out_of_lists_in_a_brief() {
        assert_eq!(listed("A real 3D model of the case (rounded pill shape, matte black, soft coral light strip)."), vec!["strip"]);
        assert_eq!(listed("A sedan with four wheels, a windscreen and glowing headlights"), vec!["headlight", "wheel", "windscreen"]);
        assert_eq!(listed("Parts: body, lid, hinge, earbuds x2"), vec!["body", "earbud", "hing", "lid"]);
        assert!(listed("Back to black, with a slow orbit").is_empty());
    }
}
