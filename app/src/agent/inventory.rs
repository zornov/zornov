// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What the content library holds, for an AI that cannot see it: counts by
//! panel, category, mood and kind; which packs are installed and which
//! could be; what is free and what needs a credit line; and a search that
//! ranks by meaning and tags, shows varied candidates (not ten takes of one
//! sound) and says which of them the project already uses.
//!
//! Everything here works on a slice of [`Entry`] plus the facts measured
//! about them, so it is tested on a small fixture library and never reads
//! a path of its own: where the library lives comes from Studio's content
//! settings (`content::find_dir`).

use super::sounds::Heard;
use crate::content::Entry;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use studio_content::item::Kind;

/// Moods an editor asks for, and the library words that mean them.
pub const MOODS: &[(&str, &[&str])] = &[
    ("calm", &["calm", "peaceful", "relaxing", "serene", "chill", "gentle", "soft", "ambient", "quiet", "dreamy", "lofi"]),
    ("happy", &["happy", "joyful", "cheerful", "upbeat", "playful", "funny", "fun", "bright", "uplifting", "positive", "quirky"]),
    ("energetic", &["energetic", "driving", "pumping", "exciting", "groovy", "action", "fast", "fight", "powerful", "dance"]),
    ("epic", &["epic", "heroic", "triumphant", "dramatic", "inspiring", "grand", "cinematic", "trailer", "winner", "success"]),
    ("dark", &["dark", "mysterious", "spooky", "tense", "suspense", "scary", "creepy", "horror", "ominous", "danger", "dangerous", "monster"]),
    ("sad", &["sad", "melancholy", "sorrow", "lonely", "emotional", "sadness", "nostalgic", "romantic", "love"]),
    ("retro", &["retro", "chiptune", "8bit", "arcade", "rpg", "game"]),
];

/// The moods a piece of writing (a brief, a plan) asks for: only the strong
/// words of each, so "a cinematic trailer" is not a second mood.
pub fn brief_moods(text: &str) -> Vec<&'static str> {
    let lower = text.to_lowercase();
    let words: std::collections::HashSet<&str> = lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    MOODS.iter().filter(|(m, _)| *m != "retro").filter(|(_, ws)| ws.iter().take(6).any(|w| words.contains(w))).map(|(m, _)| *m).collect()
}

/// Words that name a kind of thing rather than describe it.
fn kind_word(w: &str) -> Option<&'static str> {
    Some(match w {
        "music" | "song" | "songs" | "track" | "tracks" | "score" | "soundtrack" | "bed" => "music",
        "sound" | "sounds" | "sfx" | "effect" | "effects" | "fx" => "sound",
        _ => return None,
    })
}

/// The moods an entry has, from its tags, category and name.
pub fn moods_of(e: &Entry) -> Vec<&'static str> {
    let mut words: Vec<String> = e.tags.iter().map(|t| t.to_lowercase()).collect();
    words.extend(e.category.to_lowercase().split(|c: char| !c.is_alphanumeric()).map(str::to_owned));
    words.extend(e.name.to_lowercase().split(|c: char| !c.is_alphanumeric()).map(str::to_owned));
    MOODS.iter().filter(|(_, ws)| ws.iter().any(|m| words.iter().any(|w| w == m))).map(|(m, _)| *m).collect()
}

/// The mood a search word names, if any (and every word that means it).
fn mood_words(w: &str) -> Option<(&'static str, &'static [&'static str])> {
    MOODS.iter().find(|(m, ws)| *m == w || ws.contains(&w)).map(|(m, ws)| (*m, *ws))
}

pub fn is_audio(e: &Entry) -> bool {
    matches!(e.kind, Kind::Sound | Kind::Music)
}

/// What is known about the entries beyond the index: how short sounds
/// sound (measured by ear) and which are already in the project.
#[derive(Default)]
pub struct Facts {
    pub heard: HashMap<PathBuf, Heard>,
    /// Library file → how many clips of the project play it.
    pub used: HashMap<PathBuf, usize>,
}

impl Facts {
    /// The kind a sound is by ear (hit, boom, swell, swoosh, long, other), or
    /// "music" for a track.
    pub fn kind_of(&self, e: &Entry) -> String {
        match e.kind {
            Kind::Music => "music".into(),
            Kind::Sound => self.heard.get(&e.path).map_or_else(|| "sound".to_owned(), |h| h.kind.clone()),
            k => k.folder().to_owned(),
        }
    }

    /// Length in seconds: measured, else from the index.
    pub fn secs_of(&self, e: &Entry) -> Option<f64> {
        self.heard.get(&e.path).map(|h| h.secs as f64).or(e.duration)
    }

    pub fn uses(&self, e: &Entry) -> usize {
        self.used.get(&e.path).copied().unwrap_or(0)
    }
}

/// Which packs are on this Mac.
#[derive(Default)]
pub struct Packs {
    /// Folder present with its files.
    pub installed: Vec<String>,
    /// Folder present with only its index: the files are fetched on demand.
    pub not_downloaded: Vec<String>,
    /// Listed in the credits but not on this Mac.
    pub available: Vec<Available>,
}

/// A pack that could be installed.
#[derive(Clone, Debug)]
pub struct Available {
    pub id: String,
    pub name: String,
    pub summary: String,
}

fn top(counts: BTreeMap<String, usize>, n: usize) -> Value {
    let mut v: Vec<(String, usize)> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let more = v.len().saturating_sub(n);
    let mut o = serde_json::Map::new();
    for (k, c) in v.into_iter().take(n) {
        o.insert(k, json!(c));
    }
    if more > 0 {
        o.insert("(others)".into(), json!(format!("{more} more")));
    }
    Value::Object(o)
}

fn count_by<'a>(entries: impl Iterator<Item = &'a Entry>, key: impl Fn(&Entry) -> Vec<String>) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for e in entries {
        for k in key(e) {
            *m.entry(k).or_insert(0) += 1;
        }
    }
    m
}

/// Music and sounds: moods, sound kinds, lengths, categories, and how much
/// of it the project already plays.
pub fn audio_overview(entries: &[Entry], facts: &Facts) -> Value {
    let music: Vec<&Entry> = entries.iter().filter(|e| e.kind == Kind::Music).collect();
    let sounds: Vec<&Entry> = entries.iter().filter(|e| e.kind == Kind::Sound).collect();
    let long = music.iter().filter(|e| facts.secs_of(e).is_none_or(|s| s >= 30.0)).count();
    let mut o = json!({
        "music": {
            "tracks": music.len(),
            "full_length_tracks": long,
            "moods": top(count_by(music.iter().copied(), |e| moods_of(e).into_iter().map(str::to_owned).collect()), 8),
            "categories": top(count_by(music.iter().copied(), |e| vec![e.category.clone()]), 10),
        },
        "sounds": {
            "count": sounds.len(),
            "kinds": top(count_by(sounds.iter().copied(), |e| vec![facts.kind_of(e)]), 8),
            "categories": top(count_by(sounds.iter().copied(), |e| vec![e.category.clone()]), 12),
        },
    });
    let used: Vec<String> = music.iter().chain(sounds.iter()).filter(|e| facts.uses(e) > 0).map(|e| e.name.clone()).take(12).collect();
    o["in_this_project"] = json!(used);
    o
}

/// The whole library in one small answer.
pub fn overview(entries: &[Entry], facts: &Facts, packs: &Packs, folder_found: bool) -> Value {
    let by_kind = count_by(entries.iter(), |e| vec![e.kind.folder().to_owned()]);
    let credited = entries.iter().filter(|e| e.credit).count();
    let mut o = json!({
        "items": entries.len(),
        "by_kind": by_kind,
        "audio": audio_overview(entries, facts),
        "pictures": {
            "categories": top(count_by(entries.iter().filter(|e| e.kind.is_image()), |e| vec![format!("{}/{}", e.kind.folder(), e.category)]), 14),
            "sets": top(count_by(entries.iter().filter(|e| e.kind.is_image()), |e| vec![e.set.clone()]), 8),
        },
        "motion_clips": entries.iter().filter(|e| e.kind == Kind::Motion || e.kind == Kind::GreenScreen).count(),
        "rights": {
            "free_to_use": entries.len() - credited,
            "credit_line_needed": credited,
            "note": "a credit line is shown in the film's credits; add/use of a credited item is reported by check",
        },
        "packs": {
            "installed": packs.installed,
            "indexed_not_downloaded": packs.not_downloaded,
            "available_to_install": packs.available.iter().map(|p| json!({"id": p.id, "name": p.name, "about": p.summary})).collect::<Vec<_>>(),
        },
        "find": "catalogue {query: [words]} searches pictures, sounds and motions at once; {panel: audio, query, kind, mood, max_seconds} narrows sounds and music",
    });
    if !folder_found {
        o["note"] = json!("Studio's content library was not found on this Mac: only the built-in samples exist (open Studio.app once, or set the content folder in Settings)");
    }
    o
}

/// What a search asks for.
#[derive(Default, Debug, Clone)]
pub struct Ask {
    pub words: Vec<String>,
    /// music, sound, hit, boom, swoosh, swell, long, or a picture kind.
    pub kind: Option<String>,
    pub mood: Option<String>,
    pub min_secs: Option<f64>,
    pub max_secs: Option<f64>,
    /// Leave out what the project already plays.
    pub fresh: bool,
}

impl Ask {
    pub fn from_words(q: &str) -> Ask {
        Ask { words: q.split(|c: char| !c.is_alphanumeric() && c != '\'').filter(|w| !w.is_empty()).map(str::to_lowercase).collect(), ..Ask::default() }
    }
}

/// One ranked result.
pub struct Hit {
    pub index: usize,
    pub score: f32,
}

fn word_match(hay: &str, w: &str) -> bool {
    crate::services::library::starts_word(hay, w)
}

/// Ranks audio entries (music and sounds) for `ask`; best first but varied:
/// each pick makes the same set and category a little less attractive for
/// the next, and what the project already plays comes after fresh ones.
/// Returns how many matched and the first `limit`.
pub fn search_audio(entries: &[Entry], facts: &Facts, ask: &Ask, also: &dyn Fn(&str) -> Vec<String>, wanted: &dyn Fn(&str) -> Option<&'static str>, limit: usize) -> (usize, Vec<Hit>) {
    search_where(entries, facts, ask, also, wanted, limit, &is_audio)
}

/// As [`search_audio`] over whatever `keep` lets through (pictures, motions
/// and fonts too).
pub fn search_where(entries: &[Entry], facts: &Facts, ask: &Ask, also: &dyn Fn(&str) -> Vec<String>, wanted: &dyn Fn(&str) -> Option<&'static str>, limit: usize, keep: &dyn Fn(&Entry) -> bool) -> (usize, Vec<Hit>) {
    // Words that name a kind, not a quality, narrow the search instead.
    let mut kind = ask.kind.clone().map(|k| k.to_lowercase());
    let mut words: Vec<&String> = Vec::new();
    for w in &ask.words {
        match kind_word(w) {
            Some(k) if kind.is_none() => kind = Some(k.to_owned()),
            Some(_) => {}
            None => words.push(w),
        }
    }
    let want_mood = ask.mood.as_deref().map(str::to_lowercase);
    let mut scored: Vec<(f32, usize)> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        if !keep(e) {
            continue;
        }
        let k = facts.kind_of(e);
        if let Some(want) = &kind {
            let ok = match want.as_str() {
                "music" | "track" => e.kind == Kind::Music,
                "sound" | "sfx" => e.kind == Kind::Sound,
                other => k == other || Kind::parse(other) == Some(e.kind),
            };
            if !ok {
                continue;
            }
        }
        let moods = moods_of(e);
        if let Some(m) = &want_mood {
            let group = mood_words(m).map(|(g, _)| g);
            if !moods.iter().any(|x| Some(*x) == group || *x == m.as_str()) {
                continue;
            }
        }
        let secs = facts.secs_of(e);
        if ask.min_secs.zip(secs).is_some_and(|(m, s)| s < m) || ask.max_secs.zip(secs).is_some_and(|(m, s)| s > m) {
            continue;
        }
        if ask.fresh && facts.uses(e) > 0 {
            continue;
        }
        let name = e.name.to_lowercase();
        let cat = e.category.to_lowercase();
        let set = e.set.to_lowercase();
        let mut score = 0.0f32;
        for w in &words {
            let mut alts: Vec<String> = std::iter::once((*w).clone()).chain(also(w)).collect();
            let mood = mood_words(w);
            if let Some((_, ws)) = mood {
                alts.extend(ws.iter().map(|s| (*s).to_owned()));
            }
            let hit_name = alts.iter().any(|a| word_match(&name, a));
            let hit_tag = alts.iter().any(|a| e.tags.iter().any(|t| t == a || t.starts_with(a.as_str())));
            let hit_cat = alts.iter().any(|a| word_match(&cat, a) || word_match(&set, a));
            let mut s = 0.0;
            if hit_name {
                s += 4.0;
            }
            if hit_tag {
                s += 3.0;
            }
            if hit_cat {
                s += 1.5;
            }
            if let Some((m, _)) = mood {
                if moods.contains(&m) {
                    s += 1.5;
                }
            }
            if wanted(w).is_some_and(|x| x == k) {
                s += 2.0;
            }
            score += s;
        }
        if words.is_empty() || score > 0.0 {
            scored.push((score, i));
        }
    }
    let total = scored.len();
    // A hit is short, a whoosh under a second, a boom longer, a riser longest.
    let ideal = words.iter().find_map(|w| wanted(w)).map(|k| match k {
        "hit" => 0.2,
        "swoosh" => 0.7,
        "boom" => 1.8,
        "swell" => 3.0,
        _ => 1.0,
    });
    let off = |i: usize| match (ideal, facts.secs_of(&entries[i])) {
        (Some(id), Some(s)) => (s.max(0.01) / id).ln().abs(),
        _ => 0.0,
    };
    // Stable tie-breaks keep the answer the same from one call to the next.
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(off(a.1).total_cmp(&off(b.1))).then(entries[a.1].name.cmp(&entries[b.1].name)));
    // Varied: take the best, then prefer other sets and categories.
    let mut picked: Vec<Hit> = Vec::new();
    let mut pool: Vec<(f32, usize)> = scored.into_iter().take(400).collect();
    while picked.len() < limit && !pool.is_empty() {
        let best = pool
            .iter()
            .enumerate()
            .map(|(pi, (s, i))| {
                let e = &entries[*i];
                let same_set = picked.iter().filter(|h| entries[h.index].set == e.set).count() as f32;
                let same_cat = picked.iter().filter(|h| entries[h.index].category == e.category).count() as f32;
                let used = facts.uses(e) as f32;
                (pi, *s - 1.1 * same_set - 0.5 * same_cat - 0.9 * used.min(3.0))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))
            .map(|(pi, _)| pi)
            .unwrap_or(0);
        let (s, i) = pool.remove(best);
        picked.push(Hit { index: i, score: s });
    }
    (total, picked)
}

/// Names of the library sounds and tracks the project already plays.
pub fn used_names(entries: &[Entry], facts: &Facts) -> Vec<String> {
    let mut v: Vec<(usize, &str)> = entries.iter().filter(|e| is_audio(e) && facts.uses(e) > 0).map(|e| (facts.uses(e), e.name.as_str())).collect();
    v.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
    v.into_iter().take(12).map(|(n, name)| if n > 1 { format!("{name} x{n}") } else { name.to_owned() }).collect()
}

/// A short note for a sound or track the project already plays: how often,
/// and a few unused ones of the same kind (and mood) to take instead.
pub fn repeat_note(entries: &[Entry], facts: &Facts, index: usize, also: &dyn Fn(&str) -> Vec<String>, wanted: &dyn Fn(&str) -> Option<&'static str>) -> Option<String> {
    let e = entries.get(index)?;
    if !is_audio(e) {
        return None;
    }
    let n = facts.uses(e);
    if n == 0 {
        return None;
    }
    let ask = Ask { kind: Some(facts.kind_of(e)), mood: moods_of(e).first().map(|m| (*m).to_owned()).filter(|_| e.kind == Kind::Music), fresh: true, ..Ask::default() };
    let (_, alts) = search_audio(entries, facts, &ask, also, wanted, 3);
    let alts: Vec<String> = alts.iter().map(|h| &entries[h.index]).filter(|a| a.path != e.path).map(|a| {
        let d = describe(a, facts);
        format!("{} ({}{})", a.name, d["kind"].as_str().unwrap_or(""), d["dur"].as_f64().map_or(String::new(), |s| format!(", {s}s")))
    }).collect();
    Some(format!(
        "\"{}\" already plays {n} time{} in this project{}",
        e.name,
        if n == 1 { "" } else { "s" },
        if alts.is_empty() { "; nothing unused of its kind is left (sfx makes new ones)".to_owned() } else { format!("; unused of the same kind: {} (reuse only when it is a deliberate motif)", alts.join(", ")) }
    ))
}

/// The best unused match for a search, or the best used one when every
/// match is used (`same` allows a used one first).
pub fn pick(entries: &[Entry], facts: &Facts, ask: &Ask, also: &dyn Fn(&str) -> Vec<String>, wanted: &dyn Fn(&str) -> Option<&'static str>, same: bool) -> Option<usize> {
    if !same {
        let fresh = Ask { fresh: true, ..ask.clone() };
        if let Some(h) = search_audio(entries, facts, &fresh, also, wanted, 1).1.first() {
            return Some(h.index);
        }
    }
    search_audio(entries, facts, ask, also, wanted, 1).1.first().map(|h| h.index)
}

/// One result as the AI reads it: name, kind, length, mood, and whether the
/// project plays it already.
pub fn describe(e: &Entry, facts: &Facts) -> Value {
    let mut o = json!({"name": e.name, "category": e.category});
    let kind = facts.kind_of(e);
    o["kind"] = json!(kind);
    if let Some(s) = facts.secs_of(e) {
        o["dur"] = json!((s * 100.0).round() / 100.0);
    }
    let moods = moods_of(e);
    if !moods.is_empty() {
        o["mood"] = json!(moods);
    }
    let n = facts.uses(e);
    if n > 0 {
        o["used_in_project"] = json!(n);
    }
    if e.credit {
        o["credit"] = json!("needs a credit line");
    }
    o
}

#[cfg(test)]
pub(crate) mod fixture {
    use super::*;

    pub fn entry(name: &str, kind: Kind, category: &str, set: &str, tags: &[&str], secs: Option<f64>, credit: bool) -> Entry {
        Entry {
            name: name.into(),
            kind,
            category: category.into(),
            set: set.into(),
            tags: tags.iter().map(|t| (*t).to_owned()).collect(),
            path: PathBuf::from(format!("/fixture/{}/{}.ogg", set, name)),
            duration: secs,
            size: None,
            attribution: if credit { format!("\"{name}\" by someone, CC BY") } else { String::new() },
            licence: if credit { "Creative Commons Attribution 4.0 International" } else { "CC0 1.0 Universal (Public Domain Dedication)" },
            credit,
        }
    }

    /// A small library: ten whooshes and hits from two sets, music in
    /// three moods, and a sticker.
    pub fn library() -> Vec<Entry> {
        let mut v = Vec::new();
        for i in 0..6 {
            v.push(entry(&format!("Whoosh {i}"), Kind::Sound, "Swooshes", "Alpha SFX", &["whoosh", "air"], Some(0.6), false));
        }
        for i in 0..3 {
            v.push(entry(&format!("Punch {i}"), Kind::Sound, "Hits & punches", "Beta SFX", &["hit", "punch"], Some(0.3), false));
        }
        v.push(entry("Rain Loop", Kind::Sound, "Nature", "Beta SFX", &["rain", "calm"], Some(20.0), false));
        v.push(entry("Sunny Walk", Kind::Music, "Pop", "Studio Tunes", &["happy", "upbeat"], Some(120.0), true));
        v.push(entry("Sunny Day", Kind::Music, "Pop", "Studio Tunes", &["happy", "groovy"], Some(100.0), true));
        v.push(entry("Night Hunt", Kind::Music, "Cinematic", "Dark Works", &["dark", "tense"], Some(150.0), false));
        v.push(entry("Still Water", Kind::Music, "Ambient", "Calm Co", &["chill", "peaceful"], Some(90.0), false));
        v.push(entry("Jingle Box", Kind::Music, "Jingles & stingers", "Calm Co", &["jingles"], Some(6.0), false));
        v.push(entry("Red Star", Kind::Sticker, "Shapes", "Basics", &["star"], None, false));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::*;
    use super::*;

    fn none(_: &str) -> Vec<String> {
        Vec::new()
    }
    fn wanted(w: &str) -> Option<&'static str> {
        super::super::sounds::wanted(w)
    }

    #[test]
    fn the_overview_counts_a_fixture_library() {
        let lib = library();
        let mut facts = Facts::default();
        for e in lib.iter().filter(|e| e.name.starts_with("Whoosh")) {
            facts.heard.insert(e.path.clone(), Heard { kind: "swoosh".into(), secs: 0.6 });
        }
        for e in lib.iter().filter(|e| e.name.starts_with("Punch")) {
            facts.heard.insert(e.path.clone(), Heard { kind: "hit".into(), secs: 0.3 });
        }
        let packs = Packs { installed: vec!["core".into()], not_downloaded: vec!["music-big".into()], available: vec![Available { id: "music-more".into(), name: "More music".into(), summary: "300 tracks".into() }] };
        let o = overview(&lib, &facts, &packs, true);
        assert_eq!(o["items"], 16);
        assert_eq!(o["by_kind"]["sounds"], 10);
        assert_eq!(o["by_kind"]["music"], 5);
        assert_eq!(o["audio"]["music"]["tracks"], 5);
        assert_eq!(o["audio"]["music"]["moods"]["happy"], 2);
        assert_eq!(o["audio"]["music"]["moods"]["dark"], 1);
        assert_eq!(o["audio"]["sounds"]["kinds"]["swoosh"], 6);
        assert_eq!(o["audio"]["sounds"]["kinds"]["hit"], 3);
        assert_eq!(o["rights"]["credit_line_needed"], 2);
        assert_eq!(o["rights"]["free_to_use"], 14);
        assert_eq!(o["packs"]["installed"][0], "core");
        assert_eq!(o["packs"]["indexed_not_downloaded"][0], "music-big");
        assert_eq!(o["packs"]["available_to_install"][0]["id"], "music-more");
        assert!(o["pictures"]["categories"]["stickers/Shapes"] == 1);
    }

    #[test]
    fn search_ranks_by_tags_and_meaning_and_shows_variety() {
        let lib = library();
        let facts = Facts::default();
        // "upbeat" is a tag of the sunny tracks; the dark one must not win.
        let (total, hits) = search_audio(&lib, &facts, &Ask::from_words("upbeat music"), &none, &wanted, 5);
        assert_eq!(total, 2, "{:?}", hits.iter().map(|h| &lib[h.index].name).collect::<Vec<_>>());
        assert!(lib[hits[0].index].name.starts_with("Sunny"));
        // A mood word finds tracks by their mood, not their name.
        let ask = Ask { mood: Some("dark".into()), ..Ask::default() };
        let (_, hits) = search_audio(&lib, &facts, &ask, &none, &wanted, 5);
        assert_eq!(lib[hits[0].index].name, "Night Hunt");
        // "whoosh": six near-identical sounds from one set, then the hits of
        // another set are not mixed in; but varied picks spread across sets
        // when the query is broad.
        let broad = Ask { kind: Some("sound".into()), ..Ask::default() };
        let (_, hits) = search_audio(&lib, &facts, &broad, &none, &wanted, 4);
        let sets: std::collections::BTreeSet<&str> = hits.iter().map(|h| lib[h.index].set.as_str()).collect();
        assert!(sets.len() >= 2, "four results from one set: {sets:?}");
    }

    #[test]
    fn what_the_project_plays_is_marked_and_can_be_left_out() {
        let lib = library();
        let mut facts = Facts::default();
        let used = lib.iter().find(|e| e.name == "Sunny Walk").unwrap();
        facts.used.insert(used.path.clone(), 2);
        let (_, hits) = search_audio(&lib, &facts, &Ask::from_words("happy music"), &none, &wanted, 5);
        // The fresh track comes first; the used one says so.
        assert_eq!(lib[hits[0].index].name, "Sunny Day");
        let d = describe(&lib[hits[1].index], &facts);
        assert_eq!(d["used_in_project"], 2);
        let ask = Ask { fresh: true, ..Ask::from_words("happy music") };
        let (total, _) = search_audio(&lib, &facts, &ask, &none, &wanted, 5);
        assert_eq!(total, 1);
    }

    #[test]
    fn placing_a_used_sound_says_so_and_offers_unused_ones() {
        let lib = library();
        let mut facts = Facts::default();
        for e in lib.iter().filter(|e| e.name.starts_with("Whoosh")) {
            facts.heard.insert(e.path.clone(), Heard { kind: "swoosh".into(), secs: 0.6 });
        }
        let i = lib.iter().position(|e| e.name == "Whoosh 0").unwrap();
        assert!(repeat_note(&lib, &facts, i, &none, &wanted).is_none(), "first use needs no note");
        facts.used.insert(lib[i].path.clone(), 2);
        let note = repeat_note(&lib, &facts, i, &none, &wanted).unwrap();
        assert!(note.contains("already plays 2 times") && note.contains("Whoosh 1"), "{note}");
        assert!(!note.contains("Whoosh 0 ("), "never offers the used one back: {note}");
    }

    #[test]
    fn a_search_pick_takes_a_different_sound_each_time() {
        let lib = library();
        let mut facts = Facts::default();
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..4 {
            let i = pick(&lib, &facts, &Ask::from_words("whoosh"), &none, &wanted, false).unwrap();
            assert!(seen.insert(i), "{} chosen twice", lib[i].name);
            *facts.used.entry(lib[i].path.clone()).or_insert(0) += 1;
        }
        // Asked to repeat, it does.
        let first = pick(&lib, &Facts::default(), &Ask::from_words("whoosh"), &none, &wanted, true).unwrap();
        let again = pick(&lib, &facts, &Ask::from_words("whoosh"), &none, &wanted, true).unwrap();
        assert_eq!(lib[first].set, lib[again].set);
    }

    #[test]
    fn a_brief_with_two_moods_is_read_as_two() {
        assert_eq!(brief_moods("a calm morning that turns tense"), vec!["calm", "dark"]);
        assert_eq!(brief_moods("a cinematic trailer"), Vec::<&str>::new());
    }

    #[test]
    fn lengths_narrow_the_search() {
        let lib = library();
        let facts = Facts::default();
        let ask = Ask { kind: Some("music".into()), min_secs: Some(30.0), ..Ask::default() };
        let (total, _) = search_audio(&lib, &facts, &ask, &none, &wanted, 10);
        assert_eq!(total, 4, "the 6 s jingle is not a bed");
    }
}
