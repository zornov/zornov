// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What things are made of, physically: density (kg/m³), friction,
//! bounce, how hard they are to break and how much they roll. A model's
//! look (its material names, metalness, glassiness, softness) tells which
//! one it is when nobody says.

/// A physical material.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stuff {
    pub name: &'static str,
    /// kg/m³.
    pub density: f32,
    pub friction: f32,
    /// 0 = lands dead, 1 = bounces back as high.
    pub bounce: f32,
    /// Rolling resistance (a ball on carpet stops, on glass it runs).
    pub rolling: f32,
    /// Impulse per kilogram (m/s of sudden speed change) that breaks it;
    /// infinity for things that do not shatter.
    pub breaks_at: f32,
    /// Soft things squash (0 rigid .. 1 jelly).
    pub soft: f32,
}

const fn s(name: &'static str, density: f32, friction: f32, bounce: f32, rolling: f32, breaks_at: f32, soft: f32) -> Stuff {
    Stuff { name, density, friction, bounce, rolling, breaks_at, soft }
}

const NEVER: f32 = f32::INFINITY;

/// Every material by name.
pub const TABLE: &[Stuff] = &[
    s("wood", 650.0, 0.5, 0.35, 0.02, 9.0, 0.0),
    s("hardwood", 750.0, 0.5, 0.35, 0.02, 12.0, 0.0),
    s("plywood", 600.0, 0.5, 0.3, 0.02, 8.0, 0.0),
    s("cork", 240.0, 0.6, 0.4, 0.03, NEVER, 0.05),
    s("bamboo", 700.0, 0.45, 0.35, 0.02, 10.0, 0.0),
    s("steel", 7850.0, 0.45, 0.25, 0.005, NEVER, 0.0),
    s("iron", 7870.0, 0.5, 0.2, 0.005, NEVER, 0.0),
    s("aluminium", 2700.0, 0.4, 0.3, 0.005, NEVER, 0.0),
    s("copper", 8960.0, 0.4, 0.25, 0.005, NEVER, 0.0),
    s("brass", 8500.0, 0.4, 0.25, 0.005, NEVER, 0.0),
    s("gold", 19300.0, 0.4, 0.2, 0.005, NEVER, 0.0),
    s("silver", 10490.0, 0.4, 0.2, 0.005, NEVER, 0.0),
    s("metal", 7800.0, 0.45, 0.25, 0.005, NEVER, 0.0),
    s("glass", 2500.0, 0.35, 0.3, 0.002, 2.5, 0.0),
    s("crystal", 2600.0, 0.3, 0.3, 0.002, 3.0, 0.0),
    s("porcelain", 2400.0, 0.45, 0.25, 0.004, 3.5, 0.0),
    s("ceramic", 2300.0, 0.5, 0.25, 0.005, 4.0, 0.0),
    s("ice", 917.0, 0.03, 0.2, 0.001, 5.0, 0.0),
    s("stone", 2600.0, 0.6, 0.2, 0.01, 18.0, 0.0),
    s("marble", 2700.0, 0.4, 0.2, 0.005, 15.0, 0.0),
    s("granite", 2700.0, 0.6, 0.2, 0.01, 20.0, 0.0),
    s("concrete", 2400.0, 0.7, 0.15, 0.02, 15.0, 0.0),
    s("brick", 1900.0, 0.65, 0.15, 0.02, 12.0, 0.0),
    s("plaster", 1100.0, 0.6, 0.1, 0.02, 6.0, 0.0),
    s("clay", 1700.0, 0.7, 0.05, 0.05, 5.0, 0.2),
    s("sand", 1600.0, 0.8, 0.05, 0.2, NEVER, 0.0),
    s("dirt", 1500.0, 0.8, 0.05, 0.15, NEVER, 0.0),
    s("plastic", 950.0, 0.4, 0.45, 0.01, 30.0, 0.0),
    s("rubber", 1100.0, 1.0, 0.8, 0.02, NEVER, 0.15),
    s("foam", 50.0, 0.7, 0.3, 0.06, NEVER, 0.4),
    s("sponge", 60.0, 0.8, 0.15, 0.08, NEVER, 0.6),
    s("jelly", 1050.0, 0.6, 0.4, 0.05, NEVER, 0.9),
    s("flesh", 1050.0, 0.7, 0.1, 0.05, NEVER, 0.35),
    s("skin", 1050.0, 0.7, 0.1, 0.05, NEVER, 0.35),
    s("bone", 1900.0, 0.5, 0.2, 0.01, 25.0, 0.0),
    s("fabric", 300.0, 0.7, 0.05, 0.1, NEVER, 0.8),
    s("leather", 860.0, 0.7, 0.15, 0.04, NEVER, 0.1),
    s("paper", 700.0, 0.5, 0.05, 0.05, NEVER, 0.3),
    s("cardboard", 690.0, 0.55, 0.1, 0.04, NEVER, 0.1),
    s("feather", 25.0, 0.5, 0.05, 0.1, NEVER, 0.6),
    s("fur", 200.0, 0.8, 0.05, 0.1, NEVER, 0.6),
    s("wax", 900.0, 0.4, 0.15, 0.02, 6.0, 0.05),
    s("fruit", 950.0, 0.5, 0.25, 0.03, NEVER, 0.3),
    s("food", 1000.0, 0.55, 0.15, 0.04, NEVER, 0.3),
    s("snow", 300.0, 0.3, 0.02, 0.2, 2.0, 0.3),
    s("water", 1000.0, 0.05, 0.0, 0.0, NEVER, 1.0),
];

/// Other words for them.
const ALIASES: &[(&str, &str)] = &[
    ("oak", "hardwood"),
    ("walnut", "hardwood"),
    ("mahogany", "hardwood"),
    ("pine", "wood"),
    ("timber", "wood"),
    ("plank", "wood"),
    ("chrome", "steel"),
    ("stainless", "steel"),
    ("aluminum", "aluminium"),
    ("tin", "aluminium"),
    ("bronze", "brass"),
    ("lead", "iron"),
    ("china", "porcelain"),
    ("pottery", "ceramic"),
    ("tile", "ceramic"),
    ("rock", "stone"),
    ("pebble", "stone"),
    ("cement", "concrete"),
    ("asphalt", "concrete"),
    ("vinyl", "plastic"),
    ("acrylic", "plastic"),
    ("nylon", "plastic"),
    ("silicone", "rubber"),
    ("tyre", "rubber"),
    ("tire", "rubber"),
    ("cloth", "fabric"),
    ("cotton", "fabric"),
    ("wool", "fabric"),
    ("silk", "fabric"),
    ("denim", "fabric"),
    ("linen", "fabric"),
    ("velvet", "fabric"),
    ("carpet", "fabric"),
    ("meat", "flesh"),
    ("body", "flesh"),
    ("mud", "dirt"),
    ("soil", "dirt"),
    ("earth", "dirt"),
    ("grass", "dirt"),
    ("gel", "jelly"),
    ("slime", "jelly"),
    ("hair", "fur"),
    ("candle", "wax"),
    ("apple", "fruit"),
    ("orange", "fruit"),
    ("bread", "food"),
    ("cake", "food"),
    ("diamond", "crystal"),
    ("gem", "crystal"),
    ("mirror", "glass"),
    ("window", "glass"),
    ("bottle", "glass"),
    ("cup", "ceramic"),
    ("mug", "ceramic"),
    ("plate", "porcelain"),
    ("vase", "porcelain"),
    ("box", "cardboard"),
];

/// A material by name or a word for it ("oak", "chrome", "a glass bottle").
pub fn named(name: &str) -> Option<Stuff> {
    let lower = name.to_ascii_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    for w in &words {
        if let Some(s) = TABLE.iter().find(|s| s.name == *w) {
            return Some(*s);
        }
    }
    for w in &words {
        let stem = w.strip_suffix('s').unwrap_or(w);
        if let Some(s) = TABLE.iter().find(|s| s.name == stem) {
            return Some(*s);
        }
        if let Some((_, to)) = ALIASES.iter().find(|(a, _)| *a == *w || *a == stem) {
            return TABLE.iter().find(|s| s.name == *to).copied();
        }
    }
    None
}

pub fn default_stuff() -> Stuff {
    named("plastic").expect("plastic is in the table")
}

/// The likeliest material for a look: its name first, then its finish
/// (glassy, metal, soft and translucent, rough and dull).
pub fn guess(look_name: &str, metallic: f32, roughness: f32, transmission: f32, subsurface: f32) -> Stuff {
    if let Some(s) = named(look_name) {
        return s;
    }
    let by = |n| named(n).expect("in the table");
    if transmission > 0.5 {
        by("glass")
    } else if metallic > 0.5 {
        by("metal")
    } else if subsurface > 0.3 {
        by("flesh")
    } else if roughness > 0.8 {
        by("wood")
    } else {
        default_stuff()
    }
}

/// Two materials touching: friction is the geometric mean (rubber on ice
/// slides), bounce the larger.
pub fn mix(a: &Stuff, b: &Stuff) -> (f32, f32) {
    ((a.friction * b.friction).sqrt(), a.bounce.max(b.bounce))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_find_materials() {
        assert_eq!(named("Oak table top").unwrap().name, "hardwood");
        assert_eq!(named("glass bottle").unwrap().name, "glass");
        assert_eq!(named("Bricks").unwrap().name, "brick");
        assert!(named("Default").is_none());
        assert_eq!(guess("Default", 1.0, 0.2, 0.0, 0.0).name, "metal");
        assert_eq!(guess("Body", 0.0, 0.5, 0.0, 0.0).name, "flesh");
        assert!(named("steel").unwrap().density > named("wood").unwrap().density * 10.0);
    }
}
