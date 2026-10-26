// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The content library (`studio-content`): stickers, emoji, shapes, frames,
//! overlays, backgrounds, photos, green-screen and motion clips, sounds and
//! music. Loaded once on a background thread; the Library panels read it
//! when it is ready and show their own samples until then.
//!
//! The bundle lives in `content-build/` next to the repository while
//! developing, or wherever `STUDIO_CONTENT` points.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use studio_content::item::{Kind, Library};

/// One library item, flattened for the UI.
#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub kind: Kind,
    pub category: String,
    pub set: String,
    pub tags: Vec<String>,
    pub path: PathBuf,
    pub duration: Option<f64>,
    pub size: Option<(u32, u32)>,
    pub attribution: String,
    pub licence: &'static str,
    /// The licence asks for a credit line to be shown.
    pub credit: bool,
}

pub struct Content {
    pub entries: Vec<Entry>,
    pub thumbnailer: Option<Arc<studio_content::thumb::Thumbnailer>>,
}

/// The bundle as loaded from disk.
static CONTENT: OnceLock<Option<Arc<Content>>> = OnceLock::new();
/// The bundle with the person's own 3D models (`my_models`) after it;
/// rebuilt when one is kept. Models only ever join the end, so an item's
/// number never changes while Studio runs.
static WITH_MODELS: Mutex<Option<Option<Arc<Content>>>> = Mutex::new(None);
static HAS_MODELS: AtomicBool = AtomicBool::new(false);
static LOADING: std::sync::Once = std::sync::Once::new();

/// A model was kept: the library is put together again with it.
pub fn models_changed() {
    *WITH_MODELS.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Whether the person has kept 3D models (as of the last time the library
/// was put together).
pub fn has_models() -> bool {
    HAS_MODELS.load(Ordering::Relaxed)
}

fn with_models(base: &Option<Arc<Content>>) -> Option<Arc<Content>> {
    let mut slot = WITH_MODELS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(done) = slot.as_ref() {
        return done.clone();
    }
    let models = crate::my_models::entries();
    HAS_MODELS.store(!models.is_empty(), Ordering::Relaxed);
    let made = match (base, models.is_empty()) {
        (b, true) => b.clone(),
        (Some(b), false) => Some(Arc::new(Content { entries: b.entries.iter().cloned().chain(models).collect(), thumbnailer: b.thumbnailer.clone() })),
        (None, false) => Some(Arc::new(Content { entries: models, thumbnailer: studio_content::thumb::Thumbnailer::new(studio_paths::cache().join("thumbs")).ok().map(Arc::new) })),
    };
    *slot = Some(made.clone());
    made
}

/// Where the last content folder found is remembered, so every Studio on
/// this Mac (test builds, the AI's runs) finds the same library, not only
/// the app that carries it.
fn remembered() -> PathBuf {
    studio_paths::cache().join("content-dir.txt")
}

/// Where the content folder is: `STUDIO_CONTENT`, a `content-build` folder
/// above the executable or the working directory, or the one found last.
pub fn find_dir() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("STUDIO_CONTENT").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let found = look();
    match &found {
        Some(dir) => {
            let r = remembered();
            if std::fs::read_to_string(&r).ok().as_deref() != Some(&*dir.to_string_lossy()) {
                let _ = std::fs::create_dir_all(studio_paths::cache());
                let _ = std::fs::write(r, dir.to_string_lossy().as_bytes());
            }
            found
        }
        None => std::fs::read_to_string(remembered()).ok().map(|s| PathBuf::from(s.trim())).filter(|d| d.join("core/index.json").exists()),
    }
}

fn look() -> Option<PathBuf> {
    let mut starts: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        starts.push(exe);
    }
    if let Ok(cwd) = std::env::current_dir() {
        starts.push(cwd);
    }
    // Next to the crate during development.
    starts.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    for s in starts {
        for dir in s.ancestors() {
            let c = dir.join("content-build");
            if c.join("core/index.json").exists() {
                return Some(c);
            }
        }
    }
    None
}

/// Start loading in the background (idempotent).
pub fn preload(ctx: &eframe::egui::Context) {
    LOADING.call_once(|| {
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("studio-content".into())
            .spawn(move || {
                let _ = CONTENT.set(load());
                ctx.request_repaint();
            })
            .expect("spawn content loader");
    });
}

/// Load synchronously (tests and the sample builders call this).
pub fn get_blocking() -> Option<Arc<Content>> {
    with_models(CONTENT.get_or_init(load))
}

/// The library if it has finished loading.
pub fn get() -> Option<Arc<Content>> {
    with_models(CONTENT.get()?)
}

fn load() -> Option<Arc<Content>> {
    let dir = find_dir()?;
    let lib: Library = studio_content::bundle::load_all(&dir).ok()?;
    let mut entries: Vec<Entry> = lib
        .items()
        .map(|(index, it)| Entry {
            name: it.name.clone(),
            kind: it.kind,
            category: it.category.clone(),
            set: it.set.clone(),
            tags: it.tags.clone(),
            path: index.path(&it.file),
            duration: it.details.duration_secs,
            size: it.details.width.zip(it.details.height),
            attribution: it.attribution.clone(),
            licence: it.licence.name(),
            credit: it.licence.needs_credit(),
        })
        // Only what is on disk: a pack can be indexed before its files
        // are fetched, and an item that cannot be added is no use.
        .filter(|e: &Entry| e.path.exists())
        .collect();
    // Stable, human order: by category, then name.
    // Music before sound effects, made-for-Studio sets before the big
    // open icon and emoji sets.
    let rank = |k: Kind| match k {
        Kind::Music => 0,
        Kind::Sound => 1,
        Kind::Sticker => 2,
        Kind::TextArt => 3,
        Kind::GreenScreen => 4,
        Kind::Motion => 5,
        Kind::Frame => 6,
        Kind::Overlay => 7,
        Kind::Background => 8,
        Kind::Photo => 9,
        Kind::Emoji => 10,
        Kind::Shape => 11,
        _ => 12,
    };
    entries.sort_by(|a, b| (rank(a.kind), &a.category, &a.name).cmp(&(rank(b.kind), &b.category, &b.name)));
    let cache = studio_paths::cache().join("thumbs");
    let thumbnailer = studio_content::thumb::Thumbnailer::new(cache).ok().map(Arc::new);
    Some(Arc::new(Content { entries, thumbnailer }))
}

impl Content {
    /// A photo by its library name ("Alpine Lake 02").
    pub fn photo(&self, name: &str) -> Option<&Path> {
        self.entries.iter().find(|e| matches!(e.kind, Kind::Photo | Kind::Background) && e.name == name).map(|e| e.path.as_path())
    }
}

/// Names of photos used by the sample projects, with the label each
/// sample clip gets.
pub const SAMPLE_PHOTOS: &[(&str, &str)] = &[
    ("Alpine Lake 02", "Lake at dawn"),
    ("Autumn Road 01", "Autumn road"),
    ("Latte Art 01", "Latte art"),
    ("Bangkok Market 01", "Night market"),
    ("Arcade 03", "Arcade lights"),
    ("Frisbee Dog 01", "Frisbee dog"),
    ("Red Canyon 01", "Red canyon"),
    ("Milky Way 01", "Milky Way"),
    ("Kitchen Dance 01", "Kitchen dance"),
    ("Pottery 01", "Pottery wheel"),
    ("Surf 01", "Surf break"),
    ("Golden Hour Portrait 01", "Golden hour portrait"),
    ("Rice Terraces 01", "Rice terraces"),
    ("Record Store 01", "Record store"),
    ("Chef 01", "Chef at the pass"),
    ("Snowy Village 01", "Snowy village"),
    ("Reading Nook 01", "Reading nook"),
    ("Cliff Yoga 01", "Cliff yoga"),
];
