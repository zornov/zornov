// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Library catalogue: effects, transitions ("Cuts") and motion presets come
//! from `studio-fx`, looks from `studio-colour`, so names, parameters and
//! defaults are the ones the renderer uses. Effects and transitions from
//! enabled plugins (WebAssembly and OpenFX) follow the built-in ones, in a
//! "Plugins" group of their own.

use std::sync::{Mutex, OnceLock};
use studio_fx::{Category, effects, motion, transitions};

/// One Library entry.
#[derive(Clone, Copy, Debug)]
pub struct Item {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// Index into the matching `*_GROUPS` list (0 is "All").
    pub group: usize,
}

pub const EFFECT_GROUPS: &[&str] = &["All", "Blur", "Light", "Stylise", "Distort", "Retro", "Time", "Keying", "Plugins"];
pub const CUT_GROUPS: &[&str] = &["All", "Basic", "Motion", "Light", "Plugins"];

/// `base` followed by the registered plugin items, rebuilt when plugins
/// change (the old list is leaked: a few hundred bytes per change).
fn with_plugins(base: &'static [Item], cache: &'static Mutex<Option<(u64, &'static [Item])>>, extra: impl Fn() -> Vec<Item>) -> &'static [Item] {
    let generation = studio_fx::external::generation();
    let mut c = cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((g, items)) = *c
        && g == generation
    {
        return items;
    }
    let more = extra();
    let items: &'static [Item] = if more.is_empty() { base } else { Box::leak(base.iter().copied().chain(more).collect::<Vec<_>>().into_boxed_slice()) };
    *c = Some((generation, items));
    items
}
pub const MOTION_GROUPS: &[&str] = &["All", "In and out", "Loop"];
pub const LOOK_GROUPS: &[&str] = &["All"];

pub fn effects() -> &'static [Item] {
    static CACHE: Mutex<Option<(u64, &'static [Item])>> = Mutex::new(None);
    let plugins = EFFECT_GROUPS.len() - 1;
    with_plugins(builtin_effects(), &CACHE, || studio_fx::external::effects().iter().map(|e| Item { id: e.id, name: e.name, description: e.description, group: plugins }).collect())
}

fn builtin_effects() -> &'static [Item] {
    static ITEMS: OnceLock<Vec<Item>> = OnceLock::new();
    ITEMS.get_or_init(|| {
        effects::all()
            .iter()
            .filter_map(|e| {
                let group = match e.category {
                    Category::Blur => 1,
                    Category::Light => 2,
                    Category::Stylise => 3,
                    Category::Distort => 4,
                    Category::Retro => 5,
                    Category::Time => 6,
                    Category::Keying => 7,
                    // Generators draw pictures of their own; they belong
                    // with Assets, not with effects applied to a clip.
                    Category::Generator => return None,
                };
                Some(Item { id: e.id, name: e.name, description: e.description, group })
            })
            .collect()
    })
}

pub fn cuts() -> &'static [Item] {
    static CACHE: Mutex<Option<(u64, &'static [Item])>> = Mutex::new(None);
    let plugins = CUT_GROUPS.len() - 1;
    with_plugins(builtin_cuts(), &CACHE, || studio_fx::external::transitions().iter().map(|t| Item { id: t.id, name: t.name, description: t.description, group: plugins }).collect())
}

fn builtin_cuts() -> &'static [Item] {
    static ITEMS: OnceLock<Vec<Item>> = OnceLock::new();
    ITEMS.get_or_init(|| {
        transitions::all()
            .iter()
            .map(|t| {
                let group = match t.id {
                    "dissolve" | "dip" | "wipe" | "clock_wipe" | "iris" | "luma_fade" | "shape_reveal" => 1,
                    "blur_dissolve" | "glitch_cut" | "light_flash" => 3,
                    _ => 2,
                };
                Item { id: t.id, name: t.name, description: t.description, group }
            })
            .collect()
    })
}

pub fn motions() -> &'static [Item] {
    static ITEMS: OnceLock<Vec<Item>> = OnceLock::new();
    ITEMS.get_or_init(|| {
        motion::all()
            .iter()
            .map(|m| Item { id: m.id, name: m.name, description: m.description, group: if matches!(m.kind, motion::MotionKind::Loop) { 2 } else { 1 } })
            .collect()
    })
}

pub fn looks() -> &'static [Item] {
    static ITEMS: OnceLock<Vec<Item>> = OnceLock::new();
    ITEMS.get_or_init(|| studio_colour::looks::LOOKS.iter().map(|l| Item { id: l.slug, name: l.title, description: "", group: 0 }).collect())
}

pub fn effect_name(id: &str) -> &str {
    effects::find(id).map_or(id, |e| e.name)
}

pub fn cut_name(id: &str) -> &str {
    transitions::find(id).map_or(id, |t| t.name)
}

pub fn look_name(slug: &str) -> &str {
    studio_colour::looks::LOOKS.iter().find(|l| l.slug == slug).map_or(slug, |l| l.title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_is_populated() {
        assert!(effects().len() > 20);
        assert!(cuts().len() > 10);
        assert!(motions().len() > 10);
        assert!(!looks().is_empty());
        // Sample projects refer to these ids.
        for id in ["glow", "gaussian_blur"] {
            assert!(effects::find(id).is_some(), "{id}");
        }
        for id in ["dissolve", "push", "dip"] {
            assert!(transitions::find(id).is_some(), "{id}");
        }
        assert!(motion::find("rise").is_some());
        assert!(studio_colour::looks::LOOKS.iter().any(|l| l.slug == "warm-afternoon"));
    }
}
