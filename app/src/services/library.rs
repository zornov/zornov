// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Library's data and actions, without drawing: what each of the nine
//! panels lists, how its categories and search filter it, what an item
//! looks like as data, and what clicking, "+" or dropping it does to the
//! project.
//!
//! Items are addressed by `(Panel, index)`, where `index` is stable for the
//! life of the process: the position in the panel's own list (the project's
//! media, the `studio-content` index, the `text` style catalogue, or the
//! `fx` / `colour` catalogues). `search` returns those indices in display
//! order.
//!
//! Assets and Audio come from the content library when it is installed
//! (`content::find_dir`) and from a small built-in list until then.

use super::{Outcome, Refusal};
use crate::content::{self, Entry};
use crate::doc::Doc;
use crate::view::{self, ClipKind, LaneKind, MediaKind, SEC, Time};
use crate::catalogue;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use studio_content::item::Kind;
use studio_model as m;

/// The nine Library panels, in rail order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Panel {
    Media,
    Assets,
    Text,
    Motions,
    Effects,
    Cuts,
    Looks,
    Audio,
    Ai,
}

pub const TEXT_GROUPS: &[&str] = &["All", "Clean", "Bold", "Outline", "Neon", "Retro", "Playful", "Elegant", "Cinematic", "Social", "Handmade"];
pub const CONTENT_ASSET_GROUPS: &[&str] = &["All", "Stickers", "Emoji", "Shapes", "Text art", "Frames", "Overlays", "Backgrounds", "Photos", "Green screen", "Motion", "Models"];
pub const CONTENT_AUDIO_GROUPS: &[&str] = &["All", "Music", "Sound effects"];
pub const MEDIA_GROUPS: &[&str] = &["All", "Video", "Images", "Audio"];
pub const AI_GROUPS: &[&str] = &["All", "Ready", "Needs a model"];

impl Panel {
    pub const ALL: [Panel; 9] = [Panel::Media, Panel::Assets, Panel::Text, Panel::Motions, Panel::Effects, Panel::Cuts, Panel::Looks, Panel::Audio, Panel::Ai];

    pub fn name(self) -> &'static str {
        match self {
            Panel::Media => "Media",
            Panel::Assets => "Assets",
            Panel::Text => "Text",
            Panel::Motions => "Motions",
            Panel::Effects => "Effects",
            Panel::Cuts => "Cuts",
            Panel::Looks => "Looks",
            Panel::Audio => "Audio",
            Panel::Ai => "AI",
        }
    }

    /// Category names; index 0 is always "All".
    pub fn categories(self) -> &'static [&'static str] {
        let content = content::find_dir().is_some() || content::has_models();
        match self {
            Panel::Media => MEDIA_GROUPS,
            Panel::Assets if content => CONTENT_ASSET_GROUPS,
            Panel::Assets => view::ASSET_GROUPS,
            Panel::Text => TEXT_GROUPS,
            Panel::Motions => catalogue::MOTION_GROUPS,
            Panel::Effects => catalogue::EFFECT_GROUPS,
            Panel::Cuts => catalogue::CUT_GROUPS,
            Panel::Looks => catalogue::LOOK_GROUPS,
            Panel::Audio if content => CONTENT_AUDIO_GROUPS,
            Panel::Audio => view::SOUND_GROUPS,
            Panel::Ai => AI_GROUPS,
        }
    }

    /// True when the panel lists `studio-content` items (Assets and Audio
    /// with the library loaded).
    pub fn uses_content(self) -> bool {
        matches!(self, Panel::Assets | Panel::Audio) && content::get().is_some()
    }
}

/// What a tile shows, as data.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub panel: Panel,
    pub index: usize,
    pub name: String,
    /// Category or set name (tooltip, detail line).
    pub category: String,
    pub description: String,
    /// Length for timed items.
    pub duration: Option<Time>,
    /// Licence and credit line for content items.
    pub licence: Option<String>,
    pub attribution: Option<String>,
    /// The file behind the item, when there is one.
    pub path: Option<PathBuf>,
    /// The kind of clip the item becomes on the timeline, if it becomes one.
    pub clip_kind: Option<ClipKind>,
    /// How the item is used: applied to the selection, or added as a clip.
    pub applies: bool,
}

/// What to draw for an item's thumbnail (the UI turns it into a texture,
/// off the UI thread: see `thumbs`).
#[derive(Clone, Debug, PartialEq)]
pub enum Thumb {
    /// A project media item (frame 0 of the file, or the generated sample).
    Media { id: u32, seed: u32 },
    /// A content file (`studio_content::thumb::Thumbnailer` renders it).
    Content { index: usize },
    /// A text style on "Aa" (or its sample text on hover).
    TextStyle { index: usize },
    /// The item applied to the sample photo (`thumbs::SAMPLE_SEED`).
    Effect { id: &'static str },
    Look { id: &'static str },
    /// The motion's pose on the sample photo; animate `t` for the preview.
    Motion { id: &'static str },
    /// Between the two sample photos; animate `t` for the preview.
    Cut { id: &'static str },
    /// A built-in item with no picture: draw an icon.
    Icon,
}

/// What a hover preview does.
#[derive(Clone, Debug, PartialEq)]
pub enum Preview {
    /// Play the first 3 s (`audio_preview::play`).
    Audio(PathBuf),
    /// Loop the thumbnail with time (motions, cuts, motion clips).
    Animate,
    /// Show the style's own sample text instead of "Aa".
    SampleText(String),
    None,
}

// --- listing -------------------------------------------------------------

/// Number of items in a panel before filtering.
pub fn count(doc: &Doc, panel: Panel) -> usize {
    match panel {
        Panel::Media => doc.project.media.len(),
        Panel::Assets | Panel::Audio if panel.uses_content() => content::get().map_or(0, |c| c.entries.len()),
        Panel::Assets => view::ASSETS.len(),
        Panel::Audio => view::SOUNDS.len(),
        Panel::Text => studio_text::styles::catalogue().len(),
        Panel::Motions => catalogue::motions().len(),
        Panel::Effects => catalogue::effects().len(),
        Panel::Cuts => catalogue::cuts().len(),
        Panel::Looks => catalogue::looks().len(),
        Panel::Ai => studio_ai::features::Feature::ALL.len(),
    }
}

/// Content kinds shown for a panel and category.
pub fn content_kinds(panel: Panel, category: usize) -> &'static [Kind] {
    match (panel, category) {
        (Panel::Audio, 1) => &[Kind::Music],
        (Panel::Audio, 2) => &[Kind::Sound],
        (Panel::Audio, _) => &[Kind::Music, Kind::Sound],
        (_, 1) => &[Kind::Sticker],
        (_, 2) => &[Kind::Emoji],
        (_, 3) => &[Kind::Shape],
        (_, 4) => &[Kind::TextArt],
        (_, 5) => &[Kind::Frame],
        (_, 6) => &[Kind::Overlay],
        (_, 7) => &[Kind::Background],
        (_, 8) => &[Kind::Photo],
        (_, 9) => &[Kind::GreenScreen],
        (_, 10) => &[Kind::Motion],
        (_, 11) => &[Kind::Model],
        // "All" leads with the made-for-Studio kinds; the big icon and emoji
        // sets have their own categories.
        _ => &[Kind::Sticker, Kind::TextArt, Kind::Frame, Kind::Overlay, Kind::Background, Kind::Photo, Kind::GreenScreen, Kind::Motion, Kind::Model],
    }
}

/// Indices of the items matching `category` and `query`, in display order.
/// Every word of the query must match the name, category, set or a tag
/// prefix (case-insensitive). Content searches are cached.
pub fn search(doc: &Doc, panel: Panel, category: usize, query: &str) -> Arc<Vec<u32>> {
    if panel.uses_content() {
        if let Some(c) = content::get() {
            return search_content(&c, panel, category, query);
        }
    }
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let n = count(doc, panel);
    let v = (0..n)
        .filter(|&i| category == 0 || in_category(doc, panel, i, category))
        .filter(|&i| {
            let it = item(doc, panel, i);
            let hay = format!("{} {}", it.name, it.category).to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .map(|i| i as u32)
        .collect();
    Arc::new(v)
}

/// One page of a search (`offset..offset + limit`).
pub fn page(doc: &Doc, panel: Panel, category: usize, query: &str, offset: usize, limit: usize) -> Vec<Item> {
    let all = search(doc, panel, category, query);
    all.iter().skip(offset).take(limit).map(|&i| item(doc, panel, i as usize)).collect()
}

fn search_content(c: &content::Content, panel: Panel, category: usize, query: &str) -> Arc<Vec<u32>> {
    type Cache = Mutex<HashMap<(Panel, usize, String, usize), Arc<Vec<u32>>>>;
    static CACHE: std::sync::OnceLock<Cache> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let key = (panel, category, query.to_owned(), c.entries.len());
    if let Some(v) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return v.clone();
    }
    let ks = content_kinds(panel, category);
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let v: Vec<u32> = c
        .entries
        .iter()
        .enumerate()
        .filter(|(_, e)| ks.contains(&e.kind))
        .filter(|(_, e)| {
            // The file's folders often say what a sound is when its name
            // does not ("explosions/A 34P.wav", "swishes-sound-pack/…").
            // Words match at the start of a word ("rain" finds Rain and
            // Rainbow, not Brain or Train).
            let path = e.path.to_string_lossy().to_lowercase();
            words.iter().all(|w| starts_word(&e.name, w) || starts_word(&e.category, w) || starts_word(&e.set, w) || e.tags.iter().any(|t| t.starts_with(w.as_str())) || starts_word(&path, w))
        })
        .map(|(i, _)| i as u32)
        .collect();
    let v = Arc::new(v);
    let mut g = cache.lock().unwrap_or_else(|e| e.into_inner());
    if g.len() > 64 {
        g.clear();
    }
    g.insert(key, v.clone());
    v
}

/// Whether a word of `text` (split at anything not a letter or digit, and
/// where lower case turns upper: "CityCrossing") starts with `w`.
pub fn starts_word(text: &str, w: &str) -> bool {
    let mut word = String::new();
    let mut prev_lower = false;
    for ch in text.chars().chain(std::iter::once(' ')) {
        let boundary = !ch.is_alphanumeric() || (ch.is_uppercase() && prev_lower);
        if boundary && !word.is_empty() {
            if word.starts_with(w) {
                return true;
            }
            word.clear();
        }
        if ch.is_alphanumeric() {
            word.extend(ch.to_lowercase());
        }
        prev_lower = ch.is_lowercase();
    }
    false
}

// --- import results ---------------------------------------------------------

fn files(n: usize) -> String {
    format!("{n} file{}", if n == 1 { "" } else { "s" })
}

/// The toast for an import: what was added, what was already in the
/// project, and every file that failed with its reason. The bool is true
/// when something went wrong (show it as a warning).
pub fn import_summary(r: &super::project::ImportReport) -> (String, bool) {
    let added = r.ids.len().saturating_sub(r.reused);
    let failed = r.failed.len();
    let reasons = match r.failed.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] if failed == 2 => format!("{a}; {b}"),
        [a, b, ..] => format!("{a}; {b}; and {} more", failed - 2),
    };
    let text = match (added, r.reused, failed) {
        (0, 0, 0) => "Nothing to import".to_string(),
        (0, 0, _) => format!("Could not import {reasons}"),
        (0, k, 0) => format!("{} already in the project", if k == 1 { "That file is".to_string() } else { format!("All {k} files are") }),
        (n, 0, 0) => format!("Imported {}", files(n)),
        (n, k, 0) => format!("Imported {}; {k} already in the project", files(n)),
        (n, k, _) => format!("Imported {}{}; could not import {reasons}", files(n), if k > 0 { format!(" ({k} already in the project)") } else { String::new() }),
    };
    (text, failed > 0)
}

// --- organising the bin -------------------------------------------------------

const FOLDER_KEY: &str = "folder";

/// How the bin is ordered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Sort {
    /// Newest import first.
    #[default]
    Added,
    Name,
    Duration,
    Kind,
}

impl Sort {
    pub const ALL: [Sort; 4] = [Sort::Added, Sort::Name, Sort::Duration, Sort::Kind];

    pub fn label(self) -> &'static str {
        match self {
            Sort::Added => "Date added",
            Sort::Name => "Name",
            Sort::Duration => "Length",
            Sort::Kind => "Type",
        }
    }
}

/// The folder a media item is filed in (kept in the project, so it saves
/// and undoes with everything else).
pub fn folder_of(doc: &Doc, id: u32) -> Option<String> {
    doc.model.media.iter().find(|x| x.id.0 == id as u64)?.extra.get(FOLDER_KEY)?.as_str().map(str::to_string).filter(|f| !f.is_empty())
}

/// Order `indices` (positions in the project's media) by `sort`.
pub fn sort_media(doc: &Doc, indices: &mut [usize], sort: Sort) {
    let media = &doc.project.media;
    let added = |i: usize| doc.model.media.iter().find(|x| x.id.0 == media[i].id as u64).map_or(0, |x| x.imported_at);
    match sort {
        Sort::Added => indices.sort_by_key(|&i| std::cmp::Reverse((added(i), media[i].id))),
        Sort::Name => indices.sort_by_key(|&i| media[i].name.to_lowercase()),
        Sort::Duration => indices.sort_by_key(|&i| std::cmp::Reverse(media[i].dur)),
        Sort::Kind => indices.sort_by_key(|&i| (media[i].kind as u8, media[i].name.to_lowercase())),
    }
}

fn media_edit(doc: &mut Doc, id: u32, label: &str, change: impl FnOnce(&mut m::MediaItem)) -> Result<(), Refusal> {
    let Some(mut item) = doc.model.media.iter().find(|x| x.id.0 == id as u64).cloned() else { return Err(Refusal("That media is no longer in the project".into())) };
    change(&mut item);
    let ok = doc.apply(label, m::Edit::SetMedia { item: Box::new(item) });
    commit(ok, doc, label, String::new(), vec![]).map(|_| ())
}

/// Give media item `id` a new name: one undo step.
pub fn rename(doc: &mut Doc, id: u32, name: &str) -> Result<String, Refusal> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Refusal("A name cannot be empty".into()));
    }
    media_edit(doc, id, "Rename media", |x| x.name = name.to_string())?;
    Ok(format!("Renamed to {name}"))
}

/// File media item `id` in `folder` (empty takes it out of its folder).
pub fn set_folder(doc: &mut Doc, id: u32, folder: &str) -> Result<String, Refusal> {
    let folder = folder.trim().to_string();
    media_edit(doc, id, "Move media", |x| {
        if folder.is_empty() {
            x.extra.remove(FOLDER_KEY);
        } else {
            x.extra.insert(FOLDER_KEY.into(), serde_json::json!(folder));
        }
    })?;
    Ok(if folder.is_empty() { "Took it out of its folder".into() } else { format!("Moved to {folder}") })
}

// --- waveforms ---------------------------------------------------------------

/// Number of bars a sound's real waveform is summed to.
pub const PEAK_BARS: usize = 96;

/// The peaks of the sound in `path`, 0..1 over `PEAK_BARS` bars, once they
/// are known. The first ask reads the sound on a worker thread and returns
/// `None`; the tile shows a quiet line meanwhile (never an invented wave).
/// A file with no readable sound stays `None` for good.
pub fn peaks(path: &std::path::Path) -> Option<Arc<Vec<f32>>> {
    type Slot = Option<Arc<Vec<f32>>>;
    static CACHE: Mutex<Option<HashMap<PathBuf, Slot>>> = Mutex::new(None);
    {
        let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        let c = c.get_or_insert_with(Default::default);
        if let Some(slot) = c.get(path) {
            return slot.clone();
        }
        c.insert(path.to_path_buf(), None);
    }
    let path = path.to_path_buf();
    std::thread::spawn(move || {
        let found = crate::sound_bridge::decoded(&path.to_string_lossy()).map(|d| Arc::new(peaks_of(&d.left)));
        CACHE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(path, found);
    });
    None
}

/// `samples` summed to `PEAK_BARS` bars of their loudest value, scaled so
/// the loudest bar is 1.
pub fn peaks_of(samples: &[f32]) -> Vec<f32> {
    if samples.is_empty() {
        return vec![0.0; PEAK_BARS];
    }
    let mut bars: Vec<f32> = (0..PEAK_BARS)
        .map(|k| {
            let (a, b) = (k * samples.len() / PEAK_BARS, ((k + 1) * samples.len() / PEAK_BARS).max(k * samples.len() / PEAK_BARS + 1).min(samples.len()));
            samples[a.min(samples.len() - 1)..b].iter().fold(0.0f32, |m, x| m.max(x.abs()))
        })
        .collect();
    let top = bars.iter().copied().fold(0.0f32, f32::max);
    if top > 0.0 {
        bars.iter_mut().for_each(|x| *x /= top);
    }
    bars
}

// --- offline media and relinking -------------------------------------------

/// Whether the file behind a media path is gone. Media without a file
/// (generated, sample) and frame sequences are never offline.
pub fn is_offline(path: &str) -> bool {
    !path.is_empty() && !studio_imageio::sequence::Sequence::is_pattern(std::path::Path::new(path)) && !std::path::Path::new(path).is_file()
}

type OfflineCache = Option<(std::time::Instant, u64, Arc<std::collections::HashSet<u32>>)>;

/// The bin's media whose files are gone from disk. The disk is looked at
/// again every two seconds, not every frame, so a drive coming back (or
/// going) shows soon without a stat per tile per frame.
pub fn offline_ids(media: &[view::MediaItem]) -> Arc<std::collections::HashSet<u32>> {
    use std::hash::{Hash, Hasher};
    static CACHE: Mutex<OfflineCache> = Mutex::new(None);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for x in media {
        (x.id, &x.path).hash(&mut h);
    }
    let sig = h.finish();
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, s, set)) = cache.as_ref() {
        if *s == sig && at.elapsed() < std::time::Duration::from_secs(2) {
            return set.clone();
        }
    }
    let set: Arc<std::collections::HashSet<u32>> = Arc::new(media.iter().filter(|x| x.path.as_ref().is_some_and(|p| is_offline(&p.to_string_lossy()))).map(|x| x.id).collect());
    *cache = Some((std::time::Instant::now(), sig, set.clone()));
    set
}

/// `item` pointed at `to`, with `to`'s own probe (length, size, tracks).
/// A file of another kind (a picture for a video) is refused.
fn pointed_at(item: &m::MediaItem, to: &std::path::Path) -> Result<m::MediaItem, String> {
    let pr = crate::media_io::probe(to).map_err(|e| format!("Could not read {}: {e}", to.file_name().map_or_else(|| to.display().to_string(), |n| n.to_string_lossy().into_owned())))?;
    if pr.kind != item.info.kind {
        return Err(format!("{} is not the same kind of file as {}", to.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()), item.name));
    }
    let mut out = item.clone();
    out.path = to.display().to_string();
    out.info = m::MediaInfo { kind: pr.kind, container: pr.container, duration: pr.duration.or(item.info.duration), video: pr.video, audio: pr.audio, extra: item.info.extra.clone() };
    Ok(out)
}

/// Point media item `id` at the file `to`: one undo step.
pub fn relink(doc: &mut Doc, id: u32, to: &std::path::Path) -> Result<String, Refusal> {
    let Some(item) = doc.model.media.iter().find(|x| x.id.0 == id as u64).cloned() else { return Err(Refusal("That media is no longer in the project".into())) };
    let new = pointed_at(&item, to).map_err(Refusal)?;
    let ok = doc.apply("Relink media", m::Edit::SetMedia { item: Box::new(new) });
    commit(ok, doc, "Relink media", format!("Relinked {}", item.name), vec![]).map(|o| o.message)
}

/// What a search for offline media found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Relinked {
    /// Names of the media pointed at a file found in the folders.
    pub done: Vec<String>,
    /// Names still offline: not found, or found but unreadable.
    pub left: Vec<String>,
}

impl Relinked {
    /// The one-line result for a toast.
    pub fn message(&self) -> String {
        let (n, k) = (self.done.len(), self.left.len());
        match (n, k) {
            (0, 0) => "Nothing is offline".into(),
            (0, k) => format!("Found none of the {k} offline file{} in that folder", if k == 1 { "" } else { "s" }),
            (n, 0) => format!("Relinked {n} file{}", if n == 1 { "" } else { "s" }),
            (n, k) => format!("Relinked {n} file{}; {k} still offline", if n == 1 { "" } else { "s" }),
        }
    }
}

/// Look in `dirs` (and their folders, a few levels down) for each offline
/// media item's file by name, and point the ones found at it, as one undo
/// step. Nothing changes when nothing is found.
pub fn relink_in(doc: &mut Doc, dirs: &[PathBuf]) -> Relinked {
    let offline: Vec<m::MediaItem> = doc.model.media.iter().filter(|x| is_offline(&x.path)).cloned().collect();
    let mut out = Relinked::default();
    if offline.is_empty() {
        return out;
    }
    let wanted: HashMap<String, usize> = offline.iter().enumerate().filter_map(|(i, x)| std::path::Path::new(&x.path).file_name().map(|n| (n.to_string_lossy().to_lowercase(), i))).collect();
    let mut found: HashMap<usize, PathBuf> = HashMap::new();
    let mut stack: Vec<(PathBuf, usize)> = dirs.iter().map(|d| (d.clone(), 0)).collect();
    let mut seen = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.filter_map(|e| e.ok()) {
            seen += 1;
            if seen > 200_000 {
                stack.clear();
                break;
            }
            let (p, Ok(ft)) = (e.path(), e.file_type()) else { continue };
            if ft.is_dir() {
                if depth < 6 && !e.file_name().to_string_lossy().starts_with('.') {
                    stack.push((p, depth + 1));
                }
            } else if let Some(&i) = wanted.get(&e.file_name().to_string_lossy().to_lowercase()) {
                found.entry(i).or_insert(p);
            }
        }
    }
    let mut edits = Vec::new();
    for (i, item) in offline.iter().enumerate() {
        match found.get(&i).map(|to| pointed_at(item, to)) {
            Some(Ok(new)) => {
                edits.push(m::Edit::SetMedia { item: Box::new(new) });
                out.done.push(item.name.clone());
            }
            _ => out.left.push(item.name.clone()),
        }
    }
    if !edits.is_empty() && !doc.apply("Relink media", m::Edit::Batch { edits }) {
        out.left.append(&mut out.done);
    }
    out
}

#[cfg(test)]
mod word_tests {
    #[test]
    fn words_match_at_their_start() {
        use super::starts_word;
        assert!(starts_word("Rainbow flag", "rain"));
        assert!(starts_word("Cloud with lightning and rain", "rain"));
        assert!(!starts_word("Brain", "rain"));
        assert!(!starts_word("Bullet train", "rain"));
        assert!(starts_word("sounds/rain-heavy.wav", "rain"));
        assert!(starts_word("CityCrossing", "crossing"));
        assert!(starts_word("Cityscape at dusk", "city"));
    }
}

fn in_category(doc: &Doc, panel: Panel, i: usize, category: usize) -> bool {
    match panel {
        Panel::Media => {
            let k = doc.project.media[i].kind;
            matches!((category, k), (1, MediaKind::Video) | (2, MediaKind::Image) | (3, MediaKind::Audio))
        }
        Panel::Audio => {
            let (name, d) = view::SOUNDS[i];
            match category {
                1 => d > 20.0 && !name.contains("ambience") && !name.contains("hum"),
                2 => d < 5.0,
                _ => name.contains("ambience") || name.contains("hum"),
            }
        }
        Panel::Assets => {
            let n = view::ASSET_GROUPS.len() - 1;
            i % n + 1 == category
        }
        Panel::Text => studio_text::styles::StyleCategory::ALL.get(category - 1) == Some(&studio_text::styles::catalogue()[i].category),
        Panel::Motions => catalogue::motions()[i].group == category,
        Panel::Effects => catalogue::effects()[i].group == category,
        Panel::Cuts => catalogue::cuts()[i].group == category,
        Panel::Looks => catalogue::looks()[i].group == category,
        Panel::Ai => {
            let ready = super::models::feature(studio_ai::features::Feature::ALL[i]).ready;
            (category == 1) == ready
        }
    }
}

fn content_entry(index: usize) -> Option<Entry> {
    content::get().and_then(|c| c.entries.get(index).cloned())
}

/// The item as data.
pub fn item(doc: &Doc, panel: Panel, index: usize) -> Item {
    let mut it = Item { panel, index, name: String::new(), category: String::new(), description: String::new(), duration: None, licence: None, attribution: None, path: None, clip_kind: None, applies: false };
    match panel {
        Panel::Media => {
            if let Some(mi) = doc.project.media.get(index) {
                it.name = mi.name.clone();
                it.category = match mi.kind {
                    MediaKind::Video => "Video",
                    MediaKind::Image => "Image",
                    MediaKind::Audio => "Audio",
                }
                .into();
                it.duration = (mi.kind != MediaKind::Image).then_some(mi.dur);
                it.path = mi.path.clone();
                it.clip_kind = Some(if mi.kind == MediaKind::Audio { ClipKind::Audio } else { ClipKind::Video });
            }
        }
        Panel::Assets | Panel::Audio if panel.uses_content() => {
            if let Some(e) = content_entry(index) {
                it.name = e.name.clone();
                it.category = e.set.clone();
                it.description = e.category.clone();
                it.duration = e.duration.map(|s| (s * SEC as f64) as Time);
                it.licence = Some(e.licence.to_owned());
                it.attribution = Some(e.attribution.clone()).filter(|a| !a.is_empty());
                it.path = Some(e.path.clone());
                it.clip_kind = Some(match e.kind {
                    Kind::Sound | Kind::Music => ClipKind::Audio,
                    // Models are shown as titles (see the AI's summary).
                    Kind::Model => ClipKind::Title,
                    _ => ClipKind::Video,
                });
                // What a kept model is made of, for the tooltip and the AI.
                if let Some(meta) = (e.kind == Kind::Model).then(|| crate::my_models::describe(&e.path)).flatten() {
                    it.description = format!("{}: {}", e.category, meta.summary());
                }
            }
        }
        Panel::Assets => {
            it.name = view::ASSETS.get(index).copied().unwrap_or_default().into();
            it.category = "Shape".into();
            it.clip_kind = Some(ClipKind::Title);
        }
        Panel::Audio => {
            if let Some(&(name, secs)) = view::SOUNDS.get(index) {
                it.name = name.into();
                it.duration = Some((secs as f64 * SEC as f64) as Time);
                it.clip_kind = Some(ClipKind::Audio);
            }
        }
        Panel::Text => {
            if let Some(s) = studio_text::styles::catalogue().get(index) {
                it.name = s.name.clone();
                it.category = format!("{:?}", s.category);
                it.description = s.sample.clone();
                it.clip_kind = Some(ClipKind::Title);
            }
        }
        Panel::Motions | Panel::Effects | Panel::Cuts | Panel::Looks => {
            let (list, groups) = match panel {
                Panel::Motions => (catalogue::motions(), catalogue::MOTION_GROUPS),
                Panel::Effects => (catalogue::effects(), catalogue::EFFECT_GROUPS),
                Panel::Cuts => (catalogue::cuts(), catalogue::CUT_GROUPS),
                _ => (catalogue::looks(), catalogue::LOOK_GROUPS),
            };
            if let Some(c) = list.get(index) {
                it.name = c.name.into();
                it.category = groups.get(c.group).copied().unwrap_or_default().into();
                it.description = c.description.into();
                it.applies = true;
                if matches!(panel, Panel::Effects | Panel::Looks) {
                    it.clip_kind = Some(ClipKind::Fx);
                }
            }
        }
        Panel::Ai => {
            let f = super::models::feature(studio_ai::features::Feature::ALL[index]);
            it.name = f.name.into();
            it.category = if f.ready { "Ready" } else { "Needs a model" }.into();
            it.description = f.note;
            it.applies = true;
        }
    }
    it
}

/// The thumbnail recipe for an item.
pub fn thumb(doc: &Doc, panel: Panel, index: usize) -> Thumb {
    match panel {
        Panel::Media => doc.project.media.get(index).map_or(Thumb::Icon, |m| Thumb::Media { id: m.id, seed: m.seed }),
        Panel::Assets | Panel::Audio if panel.uses_content() => Thumb::Content { index },
        Panel::Text => Thumb::TextStyle { index },
        Panel::Effects => Thumb::Effect { id: catalogue::effects()[index].id },
        Panel::Looks => Thumb::Look { id: catalogue::looks()[index].id },
        Panel::Motions => Thumb::Motion { id: catalogue::motions()[index].id },
        Panel::Cuts => Thumb::Cut { id: catalogue::cuts()[index].id },
        _ => Thumb::Icon,
    }
}

/// What hovering an item previews.
pub fn preview(doc: &Doc, panel: Panel, index: usize) -> Preview {
    match panel {
        Panel::Audio if panel.uses_content() => content_entry(index).map_or(Preview::None, |e| Preview::Audio(e.path)),
        Panel::Assets if panel.uses_content() => match content_entry(index) {
            Some(e) if e.kind == Kind::Motion => Preview::Animate,
            _ => Preview::None,
        },
        Panel::Media => match doc.project.media.get(index) {
            Some(mi) if mi.kind == MediaKind::Audio => mi.path.clone().map_or(Preview::None, Preview::Audio),
            Some(mi) if mi.kind == MediaKind::Video => Preview::Animate,
            _ => Preview::None,
        },
        Panel::Text => studio_text::styles::catalogue().get(index).map_or(Preview::None, |s| Preview::SampleText(s.sample.clone())),
        Panel::Motions | Panel::Cuts | Panel::Effects | Panel::Looks => Preview::Animate,
        _ => Preview::None,
    }
}

/// Clip kind and length an item gets when dragged onto the timeline (for
/// the drop ghost).
pub fn drop_shape(doc: &Doc, panel: Panel, index: usize) -> (ClipKind, Time) {
    let it = item(doc, panel, index);
    let kind = it.clip_kind.unwrap_or(ClipKind::Fx);
    let dur = match panel {
        Panel::Media => doc.project.media.get(index).map_or(4 * SEC, |m| if m.kind == MediaKind::Image { 4 * SEC } else { m.dur.max(SEC / 10) }),
        Panel::Audio if !panel.uses_content() => it.duration.unwrap_or(4 * SEC).min(30 * SEC),
        Panel::Assets | Panel::Audio if panel.uses_content() => match content_entry(index) {
            Some(e) if matches!(e.kind, Kind::Sound | Kind::Music | Kind::Motion) => it.duration.unwrap_or(3 * SEC),
            _ => 4 * SEC,
        },
        Panel::Assets => 3 * SEC,
        _ => 4 * SEC,
    };
    (kind, dur)
}

// --- actions -------------------------------------------------------------

/// Click on an item: effects, looks and motions apply to the visual clips
/// in `targets`, cuts go after each target on the main lane; everything
/// else is added at `playhead`.
pub fn activate(doc: &mut Doc, panel: Panel, index: usize, targets: &[u32], playhead: Time) -> Result<Outcome, Refusal> {
    match panel {
        Panel::Effects | Panel::Looks | Panel::Motions | Panel::Cuts => apply(doc, panel, index, targets),
        Panel::Ai => Err(Refusal(format!("{} runs from the AI panel's own button", item(doc, panel, index).name))),
        _ => add(doc, panel, index, playhead, None),
    }
}

/// Apply an effect, look, motion or cut to `targets`.
pub fn apply(doc: &mut Doc, panel: Panel, index: usize, targets: &[u32]) -> Result<Outcome, Refusal> {
    let item = match panel {
        Panel::Effects => catalogue::effects()[index],
        Panel::Looks => catalogue::looks()[index],
        Panel::Motions => catalogue::motions()[index],
        Panel::Cuts => catalogue::cuts()[index],
        _ => return Err(Refusal(format!("{} items are added, not applied", panel.name()))),
    };
    let visual: Vec<u32> = targets.iter().copied().filter(|id| doc.project.clip(*id).is_some_and(|c| c.is_visual())).collect();
    if visual.is_empty() {
        return Err(Refusal(format!("Select a clip to apply {}", item.name)));
    }
    if panel == Panel::Cuts {
        let seq = doc.seq();
        let spine = doc.project.spine();
        let mut edits = Vec::new();
        let mut placed = Vec::new();
        for id in &visual {
            let Some(c) = doc.project.clip(*id).cloned() else { continue };
            if Some(c.lane) != spine {
                continue;
            }
            let Some(next) = doc.project.clips.iter().find(|n| n.lane == c.lane && n.start == c.end()).map(|n| n.id) else { continue };
            for old in doc.project.transitions.iter().filter(|t| t.left == c.id) {
                edits.push(m::Edit::RemoveTransition { seq, id: m::TransitionId(old.id) });
            }
            // Half a second, or less when the media's handles are shorter.
            // With no handles at all (a clip that uses its whole file),
            // ripple-trim a quarter second off each side of the cut to make
            // them, as the cut needs frames from both clips.
            let want = SEC / 2;
            let frame = super::transport::frame(doc.project.fps);
            let limit = doc.model.max_transition_duration(seq, m::ClipId(c.id as u64), m::TransitionAlign::Centre).unwrap_or(0);
            let duration = if limit >= 2 * frame {
                want.min(limit) / frame * frame
            } else {
                let right = doc.project.clip(next).map_or(0, |r| r.dur);
                if c.dur <= want + frame || right <= want + frame {
                    continue;
                }
                let half = want / 2;
                edits.push(m::Edit::Trim { seq, clip: m::ClipId(c.id as u64), edge: m::Edge::End, delta: -half, ripple: true });
                edits.push(m::Edit::Trim { seq, clip: m::ClipId(next as u64), edge: m::Edge::Start, delta: half, ripple: true });
                want
            };
            let tid = doc.alloc_id();
            placed.push(tid as u32);
            edits.push(m::Edit::AddTransition {
                seq,
                transition: m::Transition {
                    id: m::TransitionId(tid),
                    left: m::ClipId(c.id as u64),
                    right: m::ClipId(next as u64),
                    kind: item.id.into(),
                    duration,
                    align: Default::default(),
                    params: Default::default(),
                    extra: Default::default(),
                },
            });
        }
        if placed.is_empty() {
            return Err(Refusal("Cuts go between two clips on the main lane that are each longer than the cut".into()));
        }
        return commit(doc.apply("Add cut", m::Edit::Batch { edits }), doc, "Add cut", format!("{} added", item.name), visual);
    }
    let label = match panel {
        Panel::Effects => "Add effect",
        Panel::Looks => "Apply look",
        _ => "Apply motion",
    };
    let ok = doc.edit(label, |p| {
        for id in &visual {
            let Some(c) = p.clip_mut(*id) else { continue };
            match panel {
                Panel::Effects => {
                    if !c.effects.iter().any(|e| e.kind == item.id) {
                        c.effects.push(view::Effect::new(item.id));
                    }
                }
                Panel::Looks => c.look = Some(item.id.into()),
                _ => c.motion_in = Some(item.id.into()),
            }
        }
    });
    commit(ok, doc, label, format!("{} applied", item.name), visual)
}

/// Add an item to the timeline at `at` on `lane` (or the natural lane for
/// its kind). On the main lane the clip goes in at the nearest cut and
/// later clips move along; free lanes find room without overlapping.
pub fn add(doc: &mut Doc, panel: Panel, index: usize, at: Time, lane: Option<u32>) -> Result<Outcome, Refusal> {
    if panel.uses_content() {
        return add_content(doc, index, at, lane);
    }
    let p = &doc.project;
    let lane_of = |kind: LaneKind, prefer_last: bool| {
        let mut ls = p.lanes.iter().filter(|l| l.kind == kind);
        if prefer_last { ls.last().map(|l| l.id) } else { ls.next().map(|l| l.id) }
    };
    let (mut clip, lane) = match panel {
        Panel::Media => {
            let Some(mi) = p.media.get(index).cloned() else { return Err(Refusal("That media item is gone".into())) };
            let kind = if mi.kind == MediaKind::Audio { ClipKind::Audio } else { ClipKind::Video };
            let lane = lane.or_else(|| if kind == ClipKind::Audio { lane_of(LaneKind::Audio, false) } else { p.spine() });
            let dur = if mi.kind == MediaKind::Image { 4 * SEC } else { mi.dur.max(SEC / 10) };
            (new_clip(at, dur, kind, mi.name.rsplit_once('.').map_or(mi.name.as_str(), |(a, _)| a), Some(mi.id)), lane)
        }
        Panel::Text => {
            let preset = &studio_text::styles::catalogue()[index];
            let mut c = new_clip(at, 4 * SEC, ClipKind::Title, &preset.name, None);
            c.text = if preset.sample.chars().count() <= 24 { preset.sample.clone() } else { "Your title".into() };
            c.text_style = Some(preset.id.clone());
            c.extra.text_size = preset.style.size.clamp(5.0, 300.0);
            (c, lane.or_else(|| lane_of(LaneKind::Visual, false)))
        }
        Panel::Assets => {
            let mut c = new_clip(at, 3 * SEC, ClipKind::Title, view::ASSETS[index], None);
            c.transform.scale = 30.0;
            (c, lane.or_else(|| lane_of(LaneKind::Visual, true)))
        }
        Panel::Audio => {
            let (name, secs) = view::SOUNDS[index];
            // The built-in sounds are names only; they use the project's
            // first audio item.
            let Some(media) = p.media.iter().find(|m| m.kind == MediaKind::Audio).map(|m| m.id) else {
                return Err(Refusal("Import an audio file first".into()));
            };
            let d = (secs as f64 * SEC as f64) as Time;
            (new_clip(at, d.min(30 * SEC), ClipKind::Audio, name, Some(media)), lane.or_else(|| lane_of(LaneKind::Audio, true)))
        }
        Panel::Effects | Panel::Looks => return add_adjustment(doc, panel, index, at, 4 * SEC, lane),
        Panel::Motions | Panel::Cuts | Panel::Ai => return Err(Refusal(format!("{} items apply to a selected clip", panel.name()))),
    };
    let Some(lane) = lane else { return Err(Refusal("No lane for that item".into())) };
    clip.lane = lane;
    let name = clip.name.clone();
    let spine = p.spine() == Some(lane);
    let cut = snap_to_cut(p, lane, at);
    let new_id = doc.alloc_id() as u32;
    clip.id = new_id;
    let ok = if spine {
        clip.start = cut;
        let mc = crate::bridge::new_clip(&doc.model, &clip);
        let seq = doc.seq();
        doc.apply("Add clip", m::Edit::Insert { seq, lane: Some(m::LaneId(lane as u64)), at: cut, clip: Box::new(mc) })
    } else {
        doc.edit("Add clip", |p| {
            p.clips.push(clip);
            p.resolve_overlap(new_id);
        })
    };
    commit(ok, doc, "Add clip", format!("Added {name}"), vec![new_id])
}

/// Where an item dropped at `at` on the main lane lands: the nearest cut
/// (start or end of a clip) on that lane, or 0 on an empty lane. The drop
/// ghost uses the same function, so what is drawn is what happens.
pub fn snap_to_cut(p: &view::Project, lane: u32, at: Time) -> Time {
    p.clips.iter().filter(|c| c.lane == lane).flat_map(|c| [c.start, c.end()]).min_by_key(|x| (x - at).abs()).unwrap_or(0)
}

/// An adjustment clip carrying an effect or look over `start..start + dur`.
pub fn add_adjustment(doc: &mut Doc, panel: Panel, index: usize, start: Time, dur: Time, lane: Option<u32>) -> Result<Outcome, Refusal> {
    let item = match panel {
        Panel::Effects => catalogue::effects()[index],
        Panel::Looks => catalogue::looks()[index],
        _ => return Err(Refusal("Only effects and looks make adjustment clips".into())),
    };
    let Some(lane) = lane.or_else(|| doc.project.lanes.iter().find(|l| l.kind == LaneKind::Visual).map(|l| l.id)) else {
        return Err(Refusal("No lane for an adjustment clip".into()));
    };
    let mut c = new_clip(start, dur, ClipKind::Fx, item.name, None);
    c.lane = lane;
    if panel == Panel::Effects {
        c.effects = vec![view::Effect::new(item.id)];
    } else {
        c.look = Some(item.id.into());
    }
    let id = doc.alloc_id() as u32;
    c.id = id;
    let ok = doc.edit("Add adjustment", |p| {
        p.clips.push(c);
        p.resolve_overlap(id);
    });
    commit(ok, doc, "Add adjustment", format!("{} added as an adjustment clip", item.name), vec![id])
}

/// The media kind and file a content item becomes in a project. Vector
/// shapes, emoji and green-screen stills are turned into cut-out PNGs
/// first (cached by the thumbnailer).
pub fn materialise(e: &Entry) -> Option<(MediaKind, PathBuf, (u32, u32))> {
    let c = content::get()?;
    match e.kind {
        Kind::Sound | Kind::Music => Some((MediaKind::Audio, e.path.clone(), (0, 0))),
        Kind::Motion => Some((MediaKind::Video, e.path.clone(), e.size.unwrap_or((1920, 1080)))),
        Kind::Shape | Kind::GreenScreen | Kind::Emoji => {
            let t = c.thumbnailer.as_ref()?;
            let img = t.thumbnail(&e.path, e.kind, 1024).ok()?;
            let path = t.cache_path(&e.path, e.kind, 1024).ok()?;
            Some((MediaKind::Image, path, (img.width, img.height)))
        }
        _ => Some((MediaKind::Image, e.path.clone(), e.size.unwrap_or((1024, 1024)))),
    }
}

/// Add content item `index` as a media item plus a clip.
pub fn add_content(doc: &mut Doc, index: usize, at: Time, lane: Option<u32>) -> Result<Outcome, Refusal> {
    let Some(e) = content_entry(index) else { return Err(Refusal("The content library is not loaded".into())) };
    if e.kind == Kind::Model {
        return add_kept_model(doc, &e, at);
    }
    let Some((kind, path, (w, h))) = materialise(&e) else { return Err(Refusal(format!("Could not add {}", e.name))) };
    let dur = match kind {
        // The index does not always know a file's length; measure it then,
        // so a clip never runs past its sound into silence.
        MediaKind::Audio => {
            let secs = e.duration.or_else(|| crate::sound_bridge::decoded(&path.to_string_lossy()).map(|d| d.left.len() as f64 / d.rate.max(1) as f64)).unwrap_or(3.0);
            (secs * SEC as f64) as Time
        }
        MediaKind::Video => (e.duration.unwrap_or(3.0) * SEC as f64) as Time,
        MediaKind::Image => 4 * SEC,
    };
    let p = &doc.project;
    let lane = lane.or_else(|| {
        if kind == MediaKind::Audio {
            p.lanes.iter().filter(|l| l.kind == LaneKind::Audio).last().map(|l| l.id)
        } else {
            p.lanes.iter().find(|l| l.kind == LaneKind::Visual).map(|l| l.id)
        }
    });
    let Some(lane) = lane else { return Err(Refusal("No lane for that item".into())) };
    // The same sticker or sound dragged twice is the same media, not a copy.
    let known = doc.project.media.iter().find(|x| x.path.as_deref() == Some(path.as_path())).map(|x| x.id);
    let media_id = known.unwrap_or_else(|| doc.alloc_id() as u32);
    let clip_id = doc.alloc_id() as u32;
    let mi = view::MediaItem { id: media_id, name: e.name.clone(), kind, dur, width: w, height: h, fps: 30.0, seed: media_id, path: Some(path) };
    let mut clip = new_clip(at, dur, if kind == MediaKind::Audio { ClipKind::Audio } else { ClipKind::Video }, &e.name, Some(media_id));
    clip.lane = lane;
    clip.id = clip_id;
    // Cut-outs arrive at a size that leaves room around them.
    if matches!(e.kind, Kind::Sticker | Kind::Emoji | Kind::Shape | Kind::TextArt | Kind::GreenScreen) {
        clip.transform.scale = 40.0;
    }
    let label = format!("Add {}", e.name);
    let ok = doc.edit(&label, |p| {
        if known.is_none() {
            p.media.push(mi);
        }
        p.clips.push(clip);
        p.resolve_overlap(clip_id);
    });
    commit(ok, doc, &label, format!("Added {}", e.name), vec![clip_id])
}

/// A kept 3D model goes into the project as a model clip on a lane of its
/// own, with its own copy of the file in the project's `Models` folder
/// (named by content, so the same model is the same file in any project).
fn add_kept_model(doc: &mut Doc, e: &Entry, at: Time) -> Result<Outcome, Refusal> {
    let mesh = studio_mesh::io::load(&e.path).map_err(|err| Refusal(format!("Could not add {}: {err}", e.name)))?;
    let label = format!("Add {}", e.name);
    let id = crate::mesh_store::add_model_at(doc, &mesh, &e.name, at).map_err(Refusal)?;
    Ok(Outcome { label, message: format!("Added {}", e.name), select: vec![id] })
}

pub(crate) fn commit(ok: bool, doc: &mut Doc, label: &str, message: String, select: Vec<u32>) -> Result<Outcome, Refusal> {
    if ok {
        Ok(Outcome { label: label.into(), message, select })
    } else {
        Err(Refusal(doc.error.take().unwrap_or_else(|| format!("{label} was refused"))))
    }
}

/// An empty clip; `lane` is filled in by the caller.
pub fn new_clip(start: Time, dur: Time, kind: ClipKind, name: &str, media: Option<u32>) -> view::Clip {
    let mut c = view::Clip {
        id: 0,
        lane: u32::MAX,
        start,
        dur,
        src_in: 0,
        kind,
        name: name.into(),
        media,
        text: String::new(),
        text_colour: eframe::egui::Color32::WHITE,
        transform: Default::default(),
        grade: Default::default(),
        audio: Default::default(),
        speed: 100.0,
        effects: vec![],
        motion_in: None,
        look: None,
        keys: vec![],
        text_style: None,
        extra: Default::default(),
    };
    if kind == ClipKind::Title {
        c.transform.y = 0.0;
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo() -> Doc {
        Doc::from_view(&view::demo())
    }

    #[test]
    fn every_panel_lists_and_describes_its_items() {
        let doc = demo();
        for panel in Panel::ALL {
            assert_eq!(panel.categories()[0], "All", "{panel:?}");
            let all = search(&doc, panel, 0, "");
            // Assets and Audio list the content library, which a checkout
            // without it (CI) does not have. (`content::get()` is not the
            // test: a model another test keeps makes a library of models
            // alone, with no sounds.)
            if all.is_empty() && matches!(panel, Panel::Assets | Panel::Audio) && crate::content::find_dir().is_none() {
                continue;
            }
            assert!(!all.is_empty(), "{panel:?} is empty");
            let first = item(&doc, panel, all[0] as usize);
            assert!(!first.name.is_empty(), "{panel:?} item without a name");
            // A page never runs past the end.
            assert!(page(&doc, panel, 0, "", all.len().saturating_sub(1), 10).len() <= 1);
        }
    }

    #[test]
    fn search_narrows_by_word_and_category() {
        let doc = demo();
        let blur = search(&doc, Panel::Effects, 0, "blur");
        assert!(!blur.is_empty());
        assert!(blur.iter().all(|&i| {
            let it = item(&doc, Panel::Effects, i as usize);
            format!("{} {}", it.name, it.category).to_lowercase().contains("blur")
        }));
        let group = search(&doc, Panel::Effects, 1, "");
        assert!(group.len() < search(&doc, Panel::Effects, 0, "").len());
        assert!(search(&doc, Panel::Effects, 0, "no such effect anywhere").is_empty());
    }

    #[test]
    fn applying_a_look_is_one_undo_step() {
        let mut doc = demo();
        let clip = doc.project.clips.iter().find(|c| c.kind == ClipKind::Video).unwrap().id;
        let look = search(&doc, Panel::Looks, 0, "")[0] as usize;
        let out = apply(&mut doc, Panel::Looks, look, &[clip]).unwrap();
        assert_eq!(out.select, vec![clip]);
        assert_eq!(doc.project.clip(clip).unwrap().look.as_deref(), Some(catalogue::looks()[look].id));
        doc.undo();
        assert_ne!(doc.project.clip(clip).unwrap().look.as_deref(), Some(catalogue::looks()[look].id));
    }

    #[test]
    fn apply_without_selection_is_refused_and_changes_nothing() {
        let mut doc = demo();
        let rev = doc.history.revision();
        assert!(apply(&mut doc, Panel::Effects, 0, &[]).is_err());
        assert_eq!(doc.history.revision(), rev);
    }

    #[test]
    fn text_is_added_at_the_playhead_with_its_style() {
        let mut doc = demo();
        let out = add(&mut doc, Panel::Text, 0, 2 * SEC, None).unwrap();
        let c = doc.project.clip(out.select[0]).unwrap();
        assert_eq!(c.kind, ClipKind::Title);
        assert_eq!(c.text_style.as_deref(), Some(studio_text::styles::catalogue()[0].id.as_str()));
        assert_eq!(doc.project.lane(c.lane).unwrap().kind, LaneKind::Visual);
    }

    #[test]
    fn cut_goes_between_two_main_lane_clips() {
        let mut doc = demo();
        let spine = doc.project.spine().unwrap();
        let mut main: Vec<_> = doc.project.clips.iter().filter(|c| c.lane == spine).cloned().collect();
        main.sort_by_key(|c| c.start);
        let cut = catalogue::cuts().iter().position(|c| c.id == "dissolve").unwrap();
        apply(&mut doc, Panel::Cuts, cut, &[main[2].id]).unwrap();
        assert!(doc.project.transitions.iter().any(|t| t.left == main[2].id && t.right == main[3].id && t.kind == "dissolve"));
    }

    #[test]
    fn effect_makes_an_adjustment_clip_when_added() {
        let mut doc = demo();
        let out = add(&mut doc, Panel::Effects, 0, SEC, None).unwrap();
        let c = doc.project.clip(out.select[0]).unwrap();
        assert_eq!(c.kind, ClipKind::Fx);
        assert_eq!(c.effects.len(), 1);
    }

    #[test]
    fn content_items_are_searchable_and_addable() {
        // Needs the built content bundle; skipped without it.
        if content::find_dir().is_none() {
            eprintln!("content-build not found; skipping");
            return;
        }
        let c = content::get_blocking().expect("content loads");
        let stickers = search_content(&c, Panel::Assets, 1, "");
        assert!(!stickers.is_empty());
        assert!(stickers.iter().all(|&i| c.entries[i as usize].kind == Kind::Sticker));
        let music = search_content(&c, Panel::Audio, 1, "");
        assert!(!music.is_empty());
        let mut doc = demo();
        let out = add_content(&mut doc, stickers[0] as usize, SEC, None).unwrap();
        let clip = doc.project.clip(out.select[0]).unwrap();
        let media = doc.project.media(clip.media.unwrap()).unwrap();
        assert!(media.path.as_ref().unwrap().exists());
        // The same item again is another clip of the same media, not a copy.
        let (media_count, media_id) = (doc.project.media.len(), media.id);
        let again = add_content(&mut doc, stickers[0] as usize, 2 * SEC, None).unwrap();
        assert_eq!(doc.project.media.len(), media_count, "no duplicate media on a repeat drag");
        assert_eq!(doc.project.clip(again.select[0]).unwrap().media, Some(media_id));
    }

    fn png(path: &std::path::Path, rgb: u8) {
        std::fs::write(path, studio_media::png::encode(&studio_media::RgbaImage { width: 2, height: 2, pixels: vec![rgb, 0, 0, 255].repeat(4) })).unwrap();
    }

    /// A file that moved is flagged offline, found again by name in a chosen
    /// folder, and relinked as one undo step; a file of another kind is refused.
    #[test]
    fn offline_media_is_flagged_and_relinks_from_a_folder() {
        let dir = std::env::temp_dir().join(format!("studio-relink-{}", std::process::id()));
        let (old, new) = (dir.join("old"), dir.join("new/deep"));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        let (a, b) = (old.join("a.png"), old.join("b.png"));
        png(&a, 255);
        png(&b, 128);
        let mut doc = crate::agent::Session::new(crate::services::project::new_custom("Relink", 640, 360, 30.0)).doc;
        let r = crate::services::project::import_media(&mut doc, &[a.clone(), b.clone()]);
        assert_eq!(r.ids.len(), 2, "{:?}", r.failed);
        assert!(offline_ids(&doc.project.media).is_empty());
        // Both move; only a.png arrives in the new folder.
        std::fs::rename(&a, new.join("a.png")).unwrap();
        std::fs::remove_file(&b).unwrap();
        let gone = offline_ids_fresh(&doc);
        assert_eq!(gone.len(), 2, "both are flagged");
        let res = relink_in(&mut doc, &[dir.join("new")]);
        assert_eq!((res.done.clone(), res.left.clone()), (vec!["a.png".to_string()], vec!["b.png".to_string()]));
        assert_eq!(res.message(), "Relinked 1 file; 1 still offline");
        assert_eq!(offline_ids_fresh(&doc).len(), 1);
        doc.undo();
        assert_eq!(offline_ids_fresh(&doc).len(), 2, "one undo puts both back offline");
        // Relinking one by hand; a sound is not a picture.
        let msg = relink(&mut doc, r.ids[1], &new.join("a.png")).unwrap();
        assert_eq!(msg, "Relinked b.png");
        std::fs::write(new.join("tone.wav"), b"x").unwrap();
        assert!(relink(&mut doc, r.ids[0], &new.join("tone.wav")).is_err());
        assert!(relink(&mut doc, 9999, &new.join("a.png")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn offline_cache_follows_the_files_not_the_frame() {
        assert!(!is_offline(""));
        assert!(is_offline("/no/such/file.mp4"));
    }

    fn offline_ids_fresh(doc: &Doc) -> Vec<u32> {
        doc.project.media.iter().filter(|x| x.path.as_ref().is_some_and(|p| is_offline(&p.to_string_lossy()))).map(|x| x.id).collect()
    }

    #[test]
    fn import_summary_tells_added_reused_and_every_failure() {
        use super::super::project::ImportReport as R;
        let r = |ids: &[u32], reused: usize, failed: &[&str]| R { ids: ids.to_vec(), reused, failed: failed.iter().map(|s| s.to_string()).collect() };
        assert_eq!(import_summary(&r(&[1, 2], 0, &[])), ("Imported 2 files".into(), false));
        assert_eq!(import_summary(&r(&[1], 0, &[])).0, "Imported 1 file");
        assert_eq!(import_summary(&r(&[1], 1, &[])).0, "That file is already in the project");
        assert_eq!(import_summary(&r(&[1, 2], 2, &[])).0, "All 2 files are already in the project");
        assert_eq!(import_summary(&r(&[1, 2, 3], 1, &[])).0, "Imported 2 files; 1 already in the project");
        assert_eq!(import_summary(&r(&[], 0, &["a.png: bad"])), ("Could not import a.png: bad".into(), true));
        assert_eq!(import_summary(&r(&[], 0, &["a: x", "b: y", "c: z", "d: w"])).0, "Could not import a: x; b: y; and 2 more");
        assert_eq!(import_summary(&r(&[1, 2], 1, &["a: x"])).0, "Imported 1 file (1 already in the project); could not import a: x");
    }

    /// Adding to the main lane lands exactly where `snap_to_cut` says (the
    /// drop ghost's position), and later clips move along.
    #[test]
    fn main_lane_drops_land_where_the_ghost_says() {
        let mut doc = demo();
        let spine = doc.project.spine().unwrap();
        let mut main: Vec<_> = doc.project.clips.iter().filter(|c| c.lane == spine).cloned().collect();
        main.sort_by_key(|c| c.start);
        let want = main[1].end();
        let near = want + SEC / 10;
        assert_eq!(snap_to_cut(&doc.project, spine, near), want);
        let before = doc.project.clips.len();
        let out = add(&mut doc, Panel::Media, 0, near, None).unwrap();
        assert_eq!(doc.project.clips.len(), before + 1);
        assert_eq!(doc.project.clip(out.select[0]).unwrap().start, want);
    }

    #[test]
    fn waveform_peaks_are_real_not_invented() {
        let mut tone: Vec<f32> = (0..9600).map(|i| (i as f32 * 0.05).sin() * 0.1).collect();
        tone[4800..4900].iter_mut().for_each(|x| *x = 0.8);
        let p = peaks_of(&tone);
        assert_eq!(p.len(), PEAK_BARS);
        let loud = p.iter().position(|&x| x >= 0.999).unwrap();
        assert_eq!(loud, 4800 * PEAK_BARS / 9600, "the loud burst sits where it is in the file");
        assert!(p.iter().all(|&x| (0.0..=1.0).contains(&x)));
        assert!(peaks_of(&[]).iter().all(|&x| x == 0.0), "silence stays flat");
        assert!(peaks(std::path::Path::new("/no/such/sound.wav")).is_none());
    }

    #[test]
    fn the_bin_sorts_renames_and_files_into_folders_undoably() {
        let mut doc = crate::agent::Session::new(crate::services::project::new_custom("Organise", 640, 360, 30.0)).doc;
        let dir = std::env::temp_dir().join(format!("studio-organise-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let files: Vec<PathBuf> = ["b.png", "a.png", "c.png"].iter().map(|n| dir.join(n)).collect();
        for (k, f) in files.iter().enumerate() {
            png(f, k as u8 * 40);
        }
        let r = crate::services::project::import_media(&mut doc, &files);
        assert_eq!(r.ids.len(), 3, "{:?}", r.failed);
        let mut idx: Vec<usize> = (0..3).collect();
        sort_media(&doc, &mut idx, Sort::Name);
        let names: Vec<&str> = idx.iter().map(|&i| doc.project.media[i].name.as_str()).collect();
        assert_eq!(names, ["a.png", "b.png", "c.png"]);
        let id = r.ids[0];
        assert_eq!(rename(&mut doc, id, "  Opening  ").unwrap(), "Renamed to Opening");
        assert!(rename(&mut doc, id, "  ").is_err());
        assert_eq!(doc.project.media.iter().find(|m| m.id == id).unwrap().name, "Opening");
        assert_eq!(folder_of(&doc, id), None);
        assert_eq!(set_folder(&mut doc, id, "Day 1").unwrap(), "Moved to Day 1");
        assert_eq!(folder_of(&doc, id).as_deref(), Some("Day 1"));
        doc.undo();
        assert_eq!(folder_of(&doc, id), None, "filing is one undo step");
        doc.undo();
        assert_eq!(doc.project.media.iter().find(|m| m.id == id).unwrap().name, "b.png");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The agent's import says why when nothing could be imported, and
    /// lists the failure beside what did import otherwise.
    #[test]
    fn agent_import_reports_conversion_failures() {
        let dir = std::env::temp_dir().join(format!("studio-agent-import-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (bad, good) = (dir.join("broken.webm"), dir.join("ok.png"));
        std::fs::write(&bad, b"not a video").unwrap();
        png(&good, 200);
        let mut s = crate::agent::Session::new(crate::services::project::new_custom("Agent import", 640, 360, 30.0));
        let err = s.call("import", &serde_json::json!({"paths": [bad.display().to_string()]})).err().expect("a failure, not Done");
        assert!(err.contains("broken.webm"), "{err}");
        let ok = s.call("import", &serde_json::json!({"paths": [bad.display().to_string(), good.display().to_string()]})).unwrap();
        let text = match &ok[0] {
            crate::agent::Content::Text(t) => t.clone(),
            _ => panic!("text expected"),
        };
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["media"].as_array().unwrap().len(), 1, "{v}");
        assert!(v["failed"][0].as_str().unwrap().contains("broken.webm"), "{v}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
