// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Where things live in a project folder.
//!
//! [`ProjectFolder`] is the one place that knows the layout; everything
//! else asks it for an area by name (`media_dir`, `generated_dir`, `state`,
//! …) and never joins a folder name itself. A project is in one of two
//! layouts:
//!
//! * [`Layout::Legacy`], the mixed folder earlier versions made: `media/`,
//!   `cache/`, `backups/`, `Models/`, `Worlds/`, `music/`, state files at the
//!   top, and so on. Every area answers with the name it always had, so a
//!   project nobody converts is never touched.
//! * [`Layout::Package`], the `.zornov` document package:
//!
//! ```text
//! Name.zornov/
//!   project.json
//!   Media/{Footage,Pictures,Music,Sounds,3D,Vector,Characters,LUTs,Recordings,Other}/
//!   Generated/   what Zornov made (pictures and video at the top, then
//!                Music, Sounds, Voice, Takes, Stills, …)
//!   Worlds/
//!   Versions/    autosaves and kept backups (Review/ holds named review versions)
//!   Cache/       disposable; left out of backups
//!   .zornov/     locks, undo history, agent traces and usage, layout
//! ```
//!
//! A folder is a package when `.zornov/package.json` says so. That file is
//! written last, so it also marks a finished conversion. A `.zornov/` folder
//! alone means nothing: older projects already keep agent traces there.

use super::ProjectFolder;
use std::path::{Path, PathBuf};

/// Extension of a package folder, without the dot.
pub const PACKAGE_EXT: &str = "zornov";

/// The folder inside a package for the app's own state.
pub const STATE_DIR: &str = ".zornov";

/// File inside [`STATE_DIR`] that marks a package.
pub const MARKER_FILE: &str = "package.json";

/// The package layout number this build writes and reads.
pub const PACKAGE_LAYOUT: u64 = 1;

/// Which arrangement a project folder follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// The mixed folder of earlier versions.
    Legacy,
    /// The `.zornov` document package.
    Package,
}

/// What a file in `Media/` is, so it lands in the right folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MediaKind {
    /// Video.
    Footage,
    /// Still pictures.
    Pictures,
    /// Music tracks.
    Music,
    /// Sound effects and other short audio.
    Sounds,
    /// Models and everything a model needs.
    Models3d,
    /// Vector drawings (`.svart`).
    Vector,
    /// The character library.
    Characters,
    /// Colour tables.
    Luts,
    /// Recordings made in Zornov (camera, screen, voice-over).
    Recordings,
    /// Files that fit no other kind (subtitles, fonts, archives).
    Other,
}

impl MediaKind {
    /// Every kind, in the order a package creates their folders.
    pub const ALL: [MediaKind; 10] = [
        MediaKind::Footage,
        MediaKind::Pictures,
        MediaKind::Music,
        MediaKind::Sounds,
        MediaKind::Models3d,
        MediaKind::Vector,
        MediaKind::Characters,
        MediaKind::Luts,
        MediaKind::Recordings,
        MediaKind::Other,
    ];

    /// The folder name under `Media/`.
    pub fn folder(self) -> &'static str {
        match self {
            MediaKind::Footage => "Footage",
            MediaKind::Pictures => "Pictures",
            MediaKind::Music => "Music",
            MediaKind::Sounds => "Sounds",
            MediaKind::Models3d => "3D",
            MediaKind::Vector => "Vector",
            MediaKind::Characters => "Characters",
            MediaKind::Luts => "LUTs",
            MediaKind::Recordings => "Recordings",
            MediaKind::Other => "Other",
        }
    }

    /// The kind a file goes in by its extension (any case, with or without
    /// the dot). Audio is split by what the format is usually used for:
    /// compressed formats are music, uncompressed ones are sounds. A person
    /// who knows better passes the kind themselves.
    pub fn for_extension(ext: &str) -> MediaKind {
        let e = ext.trim_start_matches('.').to_ascii_lowercase();
        match e.as_str() {
            "mp4" | "mov" | "m4v" | "mkv" | "webm" | "avi" | "mxf" | "mts" | "m2ts" | "mpg" | "mpeg" | "wmv" | "flv" | "3gp" | "ogv" => MediaKind::Footage,
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "heic" | "heif" | "avif" | "svg" | "psd" | "exr" | "dpx" | "hdr" | "tga" | "ico" => MediaKind::Pictures,
            "mp3" | "m4a" | "aac" | "flac" | "ogg" | "opus" | "wma" => MediaKind::Music,
            "wav" | "aif" | "aiff" | "caf" => MediaKind::Sounds,
            "glb" | "gltf" | "obj" | "mtl" | "fbx" | "stl" | "ply" | "usdz" | "usd" | "usda" | "usdc" | "dae" | "3ds" | "blend" | "abc" | "3mf" | "off" | "smesh" | "smodel" | "bvh" | "bin" => MediaKind::Models3d,
            "svart" => MediaKind::Vector,
            "cube" | "3dl" => MediaKind::Luts,
            _ => MediaKind::Other,
        }
    }
}

/// What Zornov made itself, under `Generated/`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MadeKind {
    /// Generated pictures and video (the top of `Generated/`).
    Visuals,
    /// Music Zornov composed.
    Music,
    /// Sound effects and foley.
    Sounds,
    /// Spoken lines.
    Voice,
    /// Sung lines (with the voice in a package).
    Sung,
    /// Layers merged into one clip.
    Flattened,
    /// Alternate takes of a generation.
    Takes,
    /// Inputs kept with what was made from them.
    Sources,
    /// Pictures kept for a saved look.
    Looks,
    /// Saved grade stills.
    Stills,
    /// Cut-out mattes.
    Cutouts,
    /// Slow-motion renders.
    SlowMotion,
    /// Solved cameras.
    Cameras,
    /// Clean plates.
    CleanPlates,
    /// Depth mattes.
    Depth,
    /// Character part pictures.
    Parts,
    /// Pictures an importer made.
    Imported,
    /// Dubbed lines, by language.
    Dubs,
    /// ADR takes.
    Adr,
    /// Pictures placed in the timeline.
    Placed,
    /// Production papers.
    Production,
}

impl MadeKind {
    /// The folder under `Generated/` (empty for the top).
    fn package_folder(self) -> &'static str {
        match self {
            MadeKind::Visuals => "",
            MadeKind::Music => "Music",
            MadeKind::Sounds => "Sounds",
            MadeKind::Voice | MadeKind::Sung => "Voice",
            MadeKind::Flattened => "Flattened",
            MadeKind::Takes => "Takes",
            MadeKind::Sources => "Sources",
            MadeKind::Looks => "Looks",
            MadeKind::Stills => "Stills",
            MadeKind::Cutouts => "Cut-outs",
            MadeKind::SlowMotion => "Slow motion",
            MadeKind::Cameras => "Cameras",
            MadeKind::CleanPlates => "Clean plates",
            MadeKind::Depth => "Depth",
            MadeKind::Parts => "Parts",
            MadeKind::Imported => "Imported",
            MadeKind::Dubs => "Dubs",
            MadeKind::Adr => "ADR",
            MadeKind::Placed => "Placed",
            MadeKind::Production => "Production",
        }
    }

    /// The folder (from the project root) earlier versions used.
    fn legacy_folder(self) -> &'static str {
        match self {
            MadeKind::Visuals => "Generated",
            MadeKind::Music => "music",
            MadeKind::Sounds => "sfx",
            MadeKind::Voice => "voice",
            MadeKind::Sung => "sung",
            MadeKind::Flattened => "media/flattened",
            MadeKind::Takes => "Generated/Takes",
            MadeKind::Sources => "Generated/Sources",
            MadeKind::Looks => "Generated/Looks",
            MadeKind::Stills => "Stills",
            MadeKind::Cutouts => "Cut-outs",
            MadeKind::SlowMotion => "Slow motion",
            MadeKind::Cameras => "Cameras",
            MadeKind::CleanPlates => "Clean plates",
            MadeKind::Depth => "Depth",
            MadeKind::Parts => "Parts",
            MadeKind::Imported => "Imported",
            MadeKind::Dubs => "Dubs",
            MadeKind::Adr => "ADR",
            MadeKind::Placed => "Pictures/Placed",
            MadeKind::Production => "production",
        }
    }
}

/// The app's own state files and folders (under `.zornov/` in a package).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum State {
    /// The OS lock saves take.
    Lock,
    /// The shared hold that says a process has the project open.
    Open,
    /// The undo journal.
    History,
    /// The undo history older versions kept as one file.
    HistoryOld,
    /// Files naming the processes working on the project.
    Sessions,
    /// When the last off-site backup ran.
    LastBackup,
    /// The 3D viewport's layout.
    Workspace3d,
    /// The agent's usage log.
    AgentUsage,
    /// Folder of agent traces.
    AgentTrace,
    /// Whether AI generation is allowed.
    AgentPolicy,
    /// The agent's notes.
    AgentNotes,
    /// What the agent learned of the person's taste.
    Taste,
    /// The style guide.
    StyleGuide,
    /// Findings accepted as intended.
    Accepted,
    /// The production plan.
    Production,
    /// Team roles and locks.
    Team,
    /// Review notes.
    Comments,
    /// Link state with a shared sync folder.
    Sync,
}

impl State {
    /// Path relative to `.zornov/` in a package.
    fn package_path(self) -> &'static str {
        match self {
            State::Lock => "lock",
            State::Open => "open",
            State::History => "history.jsonl",
            State::HistoryOld => "history.json",
            State::Sessions => "sessions",
            State::LastBackup => "last-backup.json",
            State::Workspace3d => "workspace-3d.json",
            State::AgentUsage => "agent-usage.jsonl",
            State::AgentTrace => "agent-trace",
            State::AgentPolicy => "agent-policy.json",
            State::AgentNotes => "agent-notes.md",
            State::Taste => "ai/taste.json",
            State::StyleGuide => "ai/style-guide.md",
            State::Accepted => "accepted.json",
            State::Production => "production.json",
            State::Team => "team.json",
            State::Comments => "review/comments.json",
            State::Sync => "sync",
        }
    }

    /// Path relative to the project root in earlier versions.
    fn legacy_path(self) -> &'static str {
        match self {
            State::Lock => ".project.lock",
            State::Open => ".project.open",
            State::History => "cache/history.jsonl",
            State::HistoryOld => "cache/history.json",
            State::Sessions => "Sessions",
            State::LastBackup => ".last-backup.json",
            State::Workspace3d => "workspace-3d.json",
            State::AgentUsage => "agent-usage.jsonl",
            State::AgentTrace => ".zornov/agent-trace",
            State::AgentPolicy => "agent-policy.json",
            State::AgentNotes => "agent-notes.md",
            State::Taste => "AI/taste.json",
            State::StyleGuide => "AI/Style guide.md",
            State::Accepted => "accepted.json",
            State::Production => "production.json",
            State::Team => "team.json",
            State::Comments => "review/comments.json",
            State::Sync => "sync",
        }
    }
}

/// Things made for handing over, which a package keeps outside itself, in a
/// sibling `<Name> Exports` folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExportKind {
    /// Rendered files.
    Renders,
    /// Delivery bundles.
    Deliveries,
    /// Cut-down projects.
    Cutdowns,
    /// Stem exports.
    Stems,
    /// Review notes and review packages.
    Review,
    /// Renders of vector drawings.
    VectorRenders,
}

impl ExportKind {
    pub(super) fn package_folder(self) -> &'static str {
        match self {
            ExportKind::Renders => "",
            ExportKind::Deliveries => "Deliveries",
            ExportKind::Cutdowns => "Cutdowns",
            ExportKind::Stems => "Stems",
            ExportKind::Review => "Review",
            ExportKind::VectorRenders => "Vector renders",
        }
    }

    fn legacy_folder(self) -> &'static str {
        match self {
            ExportKind::Renders => "exports",
            ExportKind::Deliveries => "deliveries",
            ExportKind::Cutdowns => "cutdowns",
            ExportKind::Stems => "Stems",
            ExportKind::Review => "Review",
            ExportKind::VectorRenders => "Vector/renders",
        }
    }
}

/// What reading `.zornov/package.json` found.
pub(super) fn read_marker(root: &Path) -> Option<u64> {
    let text = std::fs::read_to_string(root.join(STATE_DIR).join(MARKER_FILE)).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v.get("layout").and_then(|n| n.as_u64())
}

/// The text of the marker file.
pub(super) fn marker_text() -> String {
    format!("{{\n  \"layout\": {PACKAGE_LAYOUT}\n}}\n")
}

impl ProjectFolder {
    /// Which layout this folder follows.
    pub fn layout(&self) -> Layout {
        if self.layout_number == 0 { Layout::Legacy } else { Layout::Package }
    }

    /// Whether `path` is a project folder or package (it holds a
    /// `project.json`).
    pub fn is_project(path: &Path) -> bool {
        path.join(super::PROJECT_FILE).is_file()
    }

    pub(super) fn pick(&self, package: &str, legacy: &str) -> PathBuf {
        self.root.join(if self.layout() == Layout::Package { package } else { legacy })
    }

    /// The project's name: the folder's name, without `.zornov`.
    pub fn name(&self) -> String {
        let n = self.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match n.strip_suffix(&format!(".{PACKAGE_EXT}")) {
            Some(stem) if !stem.is_empty() => stem.to_owned(),
            _ => n,
        }
    }

    /// `Media/` (`media/` before the package layout).
    pub fn media_root(&self) -> PathBuf {
        self.pick("Media", "media")
    }

    /// Where files of `kind` are kept. Before the package layout every kind
    /// had its own place at the top, or shared `media/`.
    pub fn media_of(&self, kind: MediaKind) -> PathBuf {
        if self.layout() == Layout::Package {
            return self.root.join("Media").join(kind.folder());
        }
        self.root.join(match kind {
            MediaKind::Pictures => "Pictures",
            MediaKind::Models3d => "Models",
            MediaKind::Vector => "Vector",
            MediaKind::Characters => "Characters",
            MediaKind::Luts => "LUTs",
            MediaKind::Recordings => "Recordings",
            MediaKind::Footage | MediaKind::Music | MediaKind::Sounds | MediaKind::Other => "media",
        })
    }

    /// Where a downloaded file of `kind` goes (a package sorts downloads
    /// with the rest of the media; earlier versions had `Downloads/`).
    pub fn download_dir(&self, kind: MediaKind) -> PathBuf {
        if self.layout() == Layout::Package { self.media_of(kind) } else { self.root.join("Downloads") }
    }

    /// Where Zornov puts what it made.
    pub fn generated_dir(&self, kind: MadeKind) -> PathBuf {
        if self.layout() == Layout::Package {
            let sub = kind.package_folder();
            let base = self.root.join("Generated");
            if sub.is_empty() { base } else { base.join(sub) }
        } else {
            self.root.join(kind.legacy_folder())
        }
    }

    /// `Worlds/`.
    pub fn worlds_dir(&self) -> PathBuf {
        self.root.join("Worlds")
    }

    /// Autosaves and kept backups (`backups/` before the package layout).
    pub fn versions_dir(&self) -> PathBuf {
        self.pick("Versions", "backups")
    }

    /// Named review versions (`versions/` before the package layout).
    pub fn review_versions_dir(&self) -> PathBuf {
        self.pick("Versions/Review", "versions")
    }

    /// Proxies (they are disposable, so they live in the cache in a
    /// package).
    pub fn proxies_dir(&self) -> PathBuf {
        self.pick("Cache/Proxies", "Proxies")
    }

    /// Simulation results, which can be run again.
    pub fn simulations_dir(&self) -> PathBuf {
        self.pick("Cache/Simulations", "Simulations")
    }

    /// Frames the agent writes to look at.
    pub fn agent_frames_dir(&self) -> PathBuf {
        self.pick("Cache/agent-frames", "agent-frames")
    }

    /// One of the app's own state files or folders.
    pub fn state(&self, what: State) -> PathBuf {
        if self.layout() == Layout::Package { self.root.join(STATE_DIR).join(what.package_path()) } else { self.root.join(what.legacy_path()) }
    }

    /// Where `kind` of deliverable goes. A package keeps these next to
    /// itself, in `<Name> Exports`, since a document should not carry its
    /// renders.
    pub fn export_dir(&self, kind: ExportKind) -> PathBuf {
        if self.layout() == Layout::Package {
            let sibling = self.root.with_file_name(format!("{} Exports", self.name()));
            let sub = kind.package_folder();
            if sub.is_empty() { sibling } else { sibling.join(sub) }
        } else {
            self.root.join(kind.legacy_folder())
        }
    }

    /// Folders whose `.json` and `.jsonl` files can name model files (to
    /// tell which ones are still used).
    pub fn reference_dirs(&self) -> Vec<PathBuf> {
        if self.layout() == Layout::Package {
            vec![self.root.clone(), self.versions_dir(), self.review_versions_dir(), self.cache_dir(), self.root.join(STATE_DIR)]
        } else {
            vec![self.root.clone(), self.root.join("versions"), self.root.join(".cache"), self.root.join("cache")]
        }
    }

    /// The project's own folders: paths that are not "files the project
    /// uses".
    pub fn own_dirs(&self) -> Vec<PathBuf> {
        if self.layout() == Layout::Package {
            ["", "Media", "Generated", "Worlds", "Versions", "Cache", STATE_DIR].iter().map(|d| self.root.join(d)).collect()
        } else {
            vec![self.root.clone(), self.root.join("cache"), self.root.join("backups"), self.root.join("media")]
        }
    }

    /// Top-level names an archive leaves out: history, the project file
    /// (written separately), and the cache unless `with_cache`.
    pub fn archive_skips(&self, with_cache: bool) -> Vec<&'static str> {
        let mut v = if self.layout() == Layout::Package { vec!["Versions", STATE_DIR] } else { vec!["backups"] };
        if !with_cache {
            v.push(if self.layout() == Layout::Package { "Cache" } else { "cache" });
        }
        v.push(super::PROJECT_FILE);
        v
    }

    /// Where a name given without a folder is looked for: the project and
    /// the places files are kept.
    pub fn lookup_roots(&self) -> Vec<PathBuf> {
        if self.layout() == Layout::Package {
            let mut v = vec![self.root.clone(), self.media_root()];
            v.extend(MediaKind::ALL.iter().map(|k| self.media_of(*k)));
            v.push(self.generated_dir(MadeKind::Visuals));
            v
        } else {
            ["", "Downloads", "Models", "media", "Media"].iter().map(|d| self.root.join(d)).collect()
        }
    }

    /// Whether Zornov made `path` itself (its parent is in a folder for what
    /// it makes), so no licence is needed for it.
    pub fn is_made_here(&self, path: &Path) -> bool {
        let Some(parent) = path.parent() else { return false };
        if self.layout() == Layout::Package {
            parent.starts_with(self.root.join("Generated"))
        } else {
            ["sfx", "sung", "voice", "foley", "music", "Generated"].iter().any(|d| parent.starts_with(self.root.join(d)))
        }
    }

    /// Files a render node needs, relative to the project: everything but
    /// caches, history and deliverables.
    pub fn portable_files(&self) -> Vec<PathBuf> {
        let package = self.layout() == Layout::Package;
        let skip = |name: &str| -> bool {
            if package {
                matches!(name, STATE_DIR | "Cache" | "Versions") || name.ends_with(".parts") || name == "_out"
            } else {
                (name.starts_with('.') && name != ".studio") || matches!(name, "exports" | "cutdowns" | "deliveries" | "_out") || name.ends_with(".parts")
            }
        };
        fn walk(dir: &Path, root: &Path, skip: &dyn Fn(&str) -> bool, top: bool, out: &mut Vec<PathBuf>) {
            let Ok(rd) = std::fs::read_dir(dir) else { return };
            let mut entries: Vec<_> = rd.flatten().collect();
            entries.sort_by_key(|e| e.file_name());
            for e in entries {
                let name = e.file_name().to_string_lossy().into_owned();
                // A package skips its own folders only at the top; the
                // earlier layout skipped these names at any depth.
                if (top || !PACKAGE_TOP_ONLY.contains(&name.as_str())) && skip(&name) {
                    continue;
                }
                let p = e.path();
                if p.is_dir() {
                    walk(&p, root, skip, false, out);
                } else {
                    out.push(p.strip_prefix(root).unwrap_or(&p).to_path_buf());
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.root, &self.root, &skip, true, &mut out);
        out
    }
}

/// Names a package skips only at the top level.
const PACKAGE_TOP_ONLY: [&str; 3] = [STATE_DIR, "Cache", "Versions"];

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy() -> ProjectFolder {
        ProjectFolder::new("/p/Old")
    }

    fn package() -> ProjectFolder {
        let mut f = ProjectFolder::new("/p/New.zornov");
        f.layout_number = 1;
        f
    }

    #[test]
    fn legacy_keeps_every_old_name() {
        let f = legacy();
        assert_eq!(f.layout(), Layout::Legacy);
        assert_eq!(f.media_of(MediaKind::Models3d), PathBuf::from("/p/Old/Models"));
        assert_eq!(f.media_of(MediaKind::Footage), PathBuf::from("/p/Old/media"));
        assert_eq!(f.download_dir(MediaKind::Models3d), PathBuf::from("/p/Old/Downloads"));
        assert_eq!(f.generated_dir(MadeKind::Visuals), PathBuf::from("/p/Old/Generated"));
        assert_eq!(f.generated_dir(MadeKind::Takes), PathBuf::from("/p/Old/Generated/Takes"));
        assert_eq!(f.generated_dir(MadeKind::Sounds), PathBuf::from("/p/Old/sfx"));
        assert_eq!(f.versions_dir(), PathBuf::from("/p/Old/backups"));
        assert_eq!(f.review_versions_dir(), PathBuf::from("/p/Old/versions"));
        assert_eq!(f.proxies_dir(), PathBuf::from("/p/Old/Proxies"));
        assert_eq!(f.state(State::History), PathBuf::from("/p/Old/cache/history.jsonl"));
        assert_eq!(f.state(State::Lock), PathBuf::from("/p/Old/.project.lock"));
        assert_eq!(f.state(State::AgentTrace), PathBuf::from("/p/Old/.zornov/agent-trace"));
        assert_eq!(f.export_dir(ExportKind::Renders), PathBuf::from("/p/Old/exports"));
        assert_eq!(f.lookup_roots().len(), 5);
        assert_eq!(f.reference_dirs()[1], PathBuf::from("/p/Old/versions"));
    }

    #[test]
    fn package_uses_the_new_layout() {
        let f = package();
        assert_eq!(f.layout(), Layout::Package);
        assert_eq!(f.name(), "New");
        assert_eq!(f.media_of(MediaKind::Models3d), PathBuf::from("/p/New.zornov/Media/3D"));
        assert_eq!(f.download_dir(MediaKind::Pictures), PathBuf::from("/p/New.zornov/Media/Pictures"));
        assert_eq!(f.generated_dir(MadeKind::Visuals), PathBuf::from("/p/New.zornov/Generated"));
        assert_eq!(f.generated_dir(MadeKind::Sung), f.generated_dir(MadeKind::Voice));
        assert_eq!(f.versions_dir(), PathBuf::from("/p/New.zornov/Versions"));
        assert_eq!(f.review_versions_dir(), PathBuf::from("/p/New.zornov/Versions/Review"));
        assert_eq!(f.cache_dir(), PathBuf::from("/p/New.zornov/Cache"));
        assert_eq!(f.proxies_dir(), PathBuf::from("/p/New.zornov/Cache/Proxies"));
        assert_eq!(f.state(State::History), PathBuf::from("/p/New.zornov/.zornov/history.jsonl"));
        assert_eq!(f.export_dir(ExportKind::Renders), PathBuf::from("/p/New Exports"));
        assert_eq!(f.export_dir(ExportKind::Stems), PathBuf::from("/p/New Exports/Stems"));
    }

    #[test]
    fn made_here_follows_the_layout() {
        let f = package();
        assert!(f.is_made_here(Path::new("/p/New.zornov/Generated/Music/a.wav")));
        assert!(!f.is_made_here(Path::new("/p/New.zornov/Media/Music/a.wav")));
        let o = legacy();
        assert!(o.is_made_here(Path::new("/p/Old/music/a.wav")));
        assert!(o.is_made_here(Path::new("/p/Old/Generated/x.png")));
        assert!(!o.is_made_here(Path::new("/p/Old/media/a.wav")));
    }

    #[test]
    fn extensions_pick_a_kind() {
        assert_eq!(MediaKind::for_extension("MP4"), MediaKind::Footage);
        assert_eq!(MediaKind::for_extension(".glb"), MediaKind::Models3d);
        assert_eq!(MediaKind::for_extension("cube"), MediaKind::Luts);
        assert_eq!(MediaKind::for_extension("wav"), MediaKind::Sounds);
        assert_eq!(MediaKind::for_extension("mp3"), MediaKind::Music);
        assert_eq!(MediaKind::for_extension("srt"), MediaKind::Other);
        assert_eq!(MediaKind::for_extension(""), MediaKind::Other);
    }

    #[test]
    fn every_media_kind_has_a_distinct_folder() {
        let mut names: Vec<_> = MediaKind::ALL.iter().map(|k| k.folder()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), MediaKind::ALL.len());
    }
}
