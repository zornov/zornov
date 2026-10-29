// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Turning an old project folder into a `.zornov` package.
//!
//! Opening an old folder never changes it; this module runs only when the
//! person asks. It is built so that a project is always either entirely the
//! old folder or entirely the package, whatever goes wrong:
//!
//! 1. [`plan`] reads the folder and says what would move where. It writes
//!    nothing.
//! 2. [`convert`] first writes a journal (`.zornov-convert.json`) into the
//!    old folder, then moves each file into a staging folder beside it
//!    (`.Name.zornov-converting-<pid>`), writes the documents with their
//!    paths rewritten into the staging folder, and writes
//!    `.zornov/package.json` last.
//! 3. The commit is one `rename` of the staging folder to `Name.zornov`.
//!    Afterwards the emptied old folder and the journal are removed.
//!
//! Any error before the commit puts every file back, newest move first. A
//! crash is settled by [`recover`] the next time the folder is looked at: if
//! `Name.zornov` has its marker the commit happened and the rest is tidying,
//! otherwise everything goes back. Nothing is deleted except the journal,
//! zero-length lock files, `.DS_Store`, and folders left empty.
//!
//! Files in the old folder that no rule knows go to `.zornov/legacy/` with
//! their relative path. Media outside the folder stays where it is.

use super::layout::{self, ExportKind, MediaKind, MARKER_FILE, PACKAGE_EXT, STATE_DIR};
use super::{write_atomic, ProjectFolder, PROJECT_FILE};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::io;
use std::path::{Component, Path, PathBuf};

/// The journal file in the old folder while a conversion runs.
pub const JOURNAL_FILE: &str = ".zornov-convert.json";

/// How to convert.
#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    /// Copy instead of move: the old folder is left exactly as it was.
    pub copy: bool,
}

/// Where a planned file ends up.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Place {
    /// Inside the package.
    Package,
    /// In the `<Name> Exports` folder beside it.
    Exports,
}

/// One file to move.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    /// Path in the old folder, relative to it.
    pub from: PathBuf,
    /// Where it goes.
    pub place: Place,
    /// Path relative to the package (or the exports folder).
    pub to: PathBuf,
    /// Size in bytes.
    pub size: u64,
}

/// What a conversion would do.
#[derive(Clone, Debug)]
pub struct Plan {
    /// The old folder.
    pub root: PathBuf,
    /// Every file that moves.
    pub moves: Vec<Planned>,
    /// Files that are removed because they carry nothing (lock files,
    /// `.DS_Store`).
    pub dropped: Vec<PathBuf>,
    /// Moved files no rule knew: they go to `.zornov/legacy/`.
    pub unknown: Vec<PathBuf>,
}

impl Plan {
    /// Counts and bytes by the top folder they land in (`Media`, `Generated`,
    /// `.zornov`, … and `Exports` for those that leave the package).
    pub fn areas(&self) -> BTreeMap<String, (usize, u64)> {
        let mut out: BTreeMap<String, (usize, u64)> = BTreeMap::new();
        for m in &self.moves {
            let top = match m.place {
                Place::Exports => "Exports".to_owned(),
                Place::Package => m.to.components().next().map(|c| c.as_os_str().to_string_lossy().into_owned()).unwrap_or_default(),
            };
            let e = out.entry(top).or_default();
            e.0 += 1;
            e.1 += m.size;
        }
        out
    }

    /// Total bytes that move.
    pub fn bytes(&self) -> u64 {
        self.moves.iter().map(|m| m.size).sum()
    }
}

/// What a finished conversion did.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// The package.
    pub package: PathBuf,
    /// Where deliverables went, if any did.
    pub exports: Option<PathBuf>,
    /// Files moved (or copied).
    pub moved: usize,
    /// Documents whose paths were rewritten.
    pub rewritten: Vec<String>,
    /// Media paths that named nothing and were matched to a moved file by
    /// name: (before, after).
    pub relinked: Vec<(String, String)>,
    /// Media paths that name nothing and could not be matched.
    pub unresolved: Vec<String>,
    /// Files that went to `.zornov/legacy/` because no rule knew them.
    pub unknown: Vec<PathBuf>,
    /// Things left in the old folder (it was not empty after the move).
    pub left_behind: Vec<PathBuf>,
}

/// Why a conversion did not happen.
#[derive(Debug)]
pub enum ConvertError {
    /// Another process has the project open.
    Busy,
    /// It is a package already.
    AlreadyPackage,
    /// There is no `project.json` there.
    NotAProject,
    /// Something failed. When `rolled_back` the folder is as it was;
    /// otherwise `stuck` lists files that could not be put back (they are
    /// in the staging folder).
    Failed {
        /// What went wrong.
        cause: String,
        /// Whether everything was put back.
        rolled_back: bool,
        /// Files that could not be put back.
        stuck: Vec<PathBuf>,
    },
    /// A test hook stopped here, as a crash would.
    Stopped(Stage),
}

impl fmt::Display for ConvertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConvertError::Busy => write!(f, "the project is open in another window; close it first"),
            ConvertError::AlreadyPackage => write!(f, "this is a package already"),
            ConvertError::NotAProject => write!(f, "there is no project.json there"),
            ConvertError::Failed { cause, rolled_back: true, .. } => write!(f, "could not convert ({cause}); the project is as it was"),
            ConvertError::Failed { cause, stuck, .. } => write!(f, "could not convert ({cause}) and {} files could not be put back; they are in the staging folder", stuck.len()),
            ConvertError::Stopped(s) => write!(f, "stopped after {s:?}"),
        }
    }
}

impl std::error::Error for ConvertError {}

/// A point in a conversion, for tests that stop there as a crash would.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// The journal is written.
    Journaled,
    /// Every file is moved.
    Moved,
    /// The rewritten documents are written.
    Written,
    /// The marker is written.
    Marked,
    /// The staging folder was renamed to the package.
    Committed,
}

/// Test hooks: fail a move, or stop as a crash would.
#[derive(Default)]
pub struct Hooks<'a> {
    /// Called before move `i`; an error fails that move.
    pub before_move: Option<&'a dyn Fn(usize) -> io::Result<()>>,
    /// Return `ConvertError::Stopped` right after this stage, leaving
    /// everything as it is.
    pub stop_after: Option<Stage>,
}

// ----------------------------------------------------------------- plan

/// Where one file goes by the rules.
enum Dest {
    Package(String),
    Exports(ExportKind, String),
    Drop,
    Project,
}

fn classify(c: &[String]) -> Dest {
    use Dest::{Drop, Exports, Package, Project};
    let top = c[0].to_lowercase();
    let tail = |n: usize| c[n..].join("/");
    let pkg = |p: String| Package(p);
    let legacy = || Package(format!("{STATE_DIR}/legacy/{}", c.join("/")));
    if c.len() == 1 {
        return match top.as_str() {
            PROJECT_FILE => Project,
            ".project.lock" | ".project.open" | ".ds_store" | JOURNAL_FILE => Drop,
            ".last-backup.json" => pkg(format!("{STATE_DIR}/last-backup.json")),
            "workspace-3d.json" | "agent-usage.jsonl" | "agent-policy.json" | "agent-notes.md" | "accepted.json" | "production.json" | "team.json" => pkg(format!("{STATE_DIR}/{}", c[0])),
            _ => legacy(),
        };
    }
    let made = |folder: &str| pkg(format!("Generated/{folder}/{}", tail(1)));
    match top.as_str() {
        "media" => {
            if c[1].eq_ignore_ascii_case("flattened") && c.len() > 2 {
                pkg(format!("Generated/Flattened/{}", tail(2)))
            } else if c.len() == 2 {
                let kind = MediaKind::for_extension(Path::new(&c[1]).extension().and_then(|e| e.to_str()).unwrap_or(""));
                pkg(format!("Media/{}/{}", kind.folder(), c[1]))
            } else {
                // A folder in media/ is an image sequence or the like.
                pkg(format!("Media/Footage/{}", tail(1)))
            }
        }
        "models" => pkg(format!("Media/3D/{}", tail(1))),
        "pictures" => {
            if c[1].eq_ignore_ascii_case("placed") && c.len() > 2 {
                pkg(format!("Generated/Placed/{}", tail(2)))
            } else {
                pkg(format!("Media/Pictures/{}", tail(1)))
            }
        }
        "vector" => {
            if c[1].eq_ignore_ascii_case("renders") && c.len() > 2 {
                Exports(ExportKind::VectorRenders, tail(2))
            } else {
                pkg(format!("Media/Vector/{}", tail(1)))
            }
        }
        "characters" => pkg(format!("Media/Characters/{}", tail(1))),
        "luts" => pkg(format!("Media/LUTs/{}", tail(1))),
        "recordings" => pkg(format!("Media/Recordings/{}", tail(1))),
        "voice-over" => pkg(format!("Media/Recordings/Voice-over/{}", tail(1))),
        "footage" => pkg(format!("Media/Footage/{}", tail(1))),
        "downloads" => {
            if c.len() == 2 {
                let kind = MediaKind::for_extension(Path::new(&c[1]).extension().and_then(|e| e.to_str()).unwrap_or(""));
                pkg(format!("Media/{}/{}", kind.folder(), c[1]))
            } else {
                // A pack (a model and its textures) stays together.
                pkg(format!("Media/3D/{}", tail(1)))
            }
        }
        "music" => made("Music"),
        "sfx" | "foley" => made("Sounds"),
        "sung" | "voice" => made("Voice"),
        "generated" => pkg(format!("Generated/{}", tail(1))),
        "stills" => made("Stills"),
        "cut-outs" => made("Cut-outs"),
        "slow motion" => made("Slow motion"),
        "cameras" => made("Cameras"),
        "clean plates" => made("Clean plates"),
        "depth" => made("Depth"),
        "parts" => made("Parts"),
        "imported" => made("Imported"),
        "dubs" => made("Dubs"),
        "adr" => made("ADR"),
        "production" => made("Production"),
        "worlds" => pkg(format!("Worlds/{}", tail(1))),
        "backups" => pkg(format!("Versions/{}", tail(1))),
        "versions" => pkg(format!("Versions/Review/{}", tail(1))),
        "cache" => {
            if c.len() == 2 && (c[1] == "history.jsonl" || c[1] == "history.json") {
                pkg(format!("{STATE_DIR}/{}", c[1]))
            } else {
                pkg(format!("Cache/{}", tail(1)))
            }
        }
        "proxies" => pkg(format!("Cache/Proxies/{}", tail(1))),
        "simulations" => pkg(format!("Cache/Simulations/{}", tail(1))),
        "agent-frames" => pkg(format!("Cache/agent-frames/{}", tail(1))),
        "sessions" => pkg(format!("{STATE_DIR}/sessions/{}", tail(1))),
        "sync" => pkg(format!("{STATE_DIR}/sync/{}", tail(1))),
        ".zornov" => pkg(format!("{STATE_DIR}/{}", tail(1))),
        "ai" => match (c.len(), c[1].as_str()) {
            (2, "taste.json") => pkg(format!("{STATE_DIR}/ai/taste.json")),
            (2, "Style guide.md") => pkg(format!("{STATE_DIR}/ai/style-guide.md")),
            _ => legacy(),
        },
        "review" => {
            if c.len() == 2 && c[1] == "comments.json" {
                pkg(format!("{STATE_DIR}/review/comments.json"))
            } else {
                Exports(ExportKind::Review, tail(1))
            }
        }
        "exports" => Exports(ExportKind::Renders, tail(1)),
        "deliveries" => Exports(ExportKind::Deliveries, tail(1)),
        "cutdowns" => Exports(ExportKind::Cutdowns, tail(1)),
        "stems" => Exports(ExportKind::Stems, tail(1)),
        _ => legacy(),
    }
}

fn walk(dir: &Path, root: &Path, out: &mut Vec<(PathBuf, u64)>) -> io::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        let ft = e.file_type()?;
        if ft.is_dir() {
            walk(&p, root, out)?;
        } else {
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            out.push((p.strip_prefix(root).unwrap_or(&p).to_path_buf(), size));
        }
    }
    Ok(())
}

fn export_path(kind: ExportKind, tail: &str) -> String {
    let folder = kind.package_folder();
    if folder.is_empty() { tail.to_owned() } else { format!("{folder}/{tail}") }
}

/// Reads the folder and says what a conversion would do. Writes nothing.
pub fn plan(root: &Path) -> Result<Plan, ConvertError> {
    if ProjectFolder::new(root).layout() == layout::Layout::Package {
        return Err(ConvertError::AlreadyPackage);
    }
    if !root.join(PROJECT_FILE).is_file() {
        return Err(ConvertError::NotAProject);
    }
    let fail = |e: io::Error| ConvertError::Failed { cause: format!("{}: {e}", root.display()), rolled_back: true, stuck: Vec::new() };
    let mut files = Vec::new();
    walk(root, root, &mut files).map_err(fail)?;
    let mut plan = Plan { root: root.to_path_buf(), moves: Vec::new(), dropped: Vec::new(), unknown: Vec::new() };
    for (rel, size) in files {
        let comps: Vec<String> = rel.components().filter_map(|c| if let Component::Normal(n) = c { Some(n.to_string_lossy().into_owned()) } else { None }).collect();
        if comps.is_empty() {
            continue;
        }
        let (place, to) = match classify(&comps) {
            Dest::Drop => {
                plan.dropped.push(rel);
                continue;
            }
            Dest::Project => (Place::Package, format!("{STATE_DIR}/legacy/{PROJECT_FILE}")),
            Dest::Package(p) => {
                if p.starts_with(&format!("{STATE_DIR}/legacy/")) {
                    plan.unknown.push(rel.clone());
                }
                (Place::Package, p)
            }
            Dest::Exports(k, tail) => (Place::Exports, export_path(k, &tail)),
        };
        plan.moves.push(Planned { from: rel, place, to: PathBuf::from(to), size });
    }
    keep_sidecars_with_media(&mut plan);
    make_names_unique(&mut plan);
    Ok(plan)
}

/// `a.words.json` follows `a.mp4`, wherever the rules put it.
fn keep_sidecars_with_media(plan: &mut Plan) {
    let by_stem: HashMap<(PathBuf, String), PathBuf> = plan
        .moves
        .iter()
        .filter(|m| !m.from.to_string_lossy().ends_with(".words.json"))
        .filter_map(|m| {
            let dir = m.from.parent()?.to_path_buf();
            let stem = m.from.file_stem()?.to_string_lossy().into_owned();
            Some(((dir, stem), m.to.parent()?.to_path_buf()))
        })
        .collect();
    for m in &mut plan.moves {
        let name = m.from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let Some(stem) = name.strip_suffix(".words.json") else { continue };
        let dir = m.from.parent().map(Path::to_path_buf).unwrap_or_default();
        if let Some(to_dir) = by_stem.get(&(dir, stem.to_owned())) {
            m.place = Place::Package;
            m.to = to_dir.join(&name);
        }
    }
}

/// Two files that would land on one name get a number, as `a 2.png`.
fn make_names_unique(plan: &mut Plan) {
    let mut taken: HashSet<(Place, String)> = HashSet::new();
    for m in &mut plan.moves {
        let key = |p: &Path| (m.place, p.to_string_lossy().to_lowercase());
        if taken.insert(key(&m.to)) {
            continue;
        }
        let stem = m.to.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let ext = m.to.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let dir = m.to.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut n = 2;
        loop {
            let cand = dir.join(format!("{stem} {n}{ext}"));
            if taken.insert((m.place, cand.to_string_lossy().to_lowercase())) {
                m.to = cand;
                break;
            }
            n += 1;
        }
    }
}

// ------------------------------------------------------------- rewriting

/// Rewrites the path strings of project documents for the moved files.
struct Rewriter {
    /// The old folder as written, and as the file system names it.
    roots: Vec<PathBuf>,
    /// The old folder as the plan sees it.
    root: PathBuf,
    /// The package's final path.
    package: PathBuf,
    /// Old absolute file → new absolute file.
    files: HashMap<PathBuf, PathBuf>,
    /// Old absolute folder → new absolute folder, for folders whose files
    /// all went to one place.
    dirs: HashMap<PathBuf, PathBuf>,
    /// File name → where files of that name went.
    by_name: HashMap<String, Vec<PathBuf>>,
}

impl Rewriter {
    fn new(plan: &Plan, package: &Path, exports: &Path) -> Self {
        let mut roots = vec![plan.root.clone()];
        if let Ok(c) = std::fs::canonicalize(&plan.root)
            && c != plan.root
        {
            roots.push(c);
        }
        let mut files = HashMap::new();
        let mut dirs: HashMap<PathBuf, Option<PathBuf>> = HashMap::new();
        let mut by_name: HashMap<String, Vec<PathBuf>> = HashMap::new();
        for m in &plan.moves {
            let new = match m.place {
                Place::Package => package.join(&m.to),
                Place::Exports => exports.join(&m.to),
            };
            let old = plan.root.join(&m.from);
            if let (Some(od), Some(nd)) = (old.parent(), new.parent()) {
                match dirs.get(od) {
                    None => {
                        dirs.insert(od.to_path_buf(), Some(nd.to_path_buf()));
                    }
                    Some(Some(prev)) if prev != nd => {
                        dirs.insert(od.to_path_buf(), None);
                    }
                    _ => {}
                }
            }
            if let Some(n) = m.from.file_name() {
                by_name.entry(n.to_string_lossy().into_owned()).or_default().push(new.clone());
            }
            files.insert(old, new);
        }
        let dirs = dirs.into_iter().filter_map(|(k, v)| v.map(|v| (k, v))).filter(|(k, _)| *k != plan.root).collect();
        Rewriter { roots, root: plan.root.clone(), package: package.to_path_buf(), files, dirs, by_name }
    }

    /// The old folder-relative path a string names, if it names one.
    fn old_key(&self, s: &str) -> Option<PathBuf> {
        if s.is_empty() || s.len() > 4096 || s.contains('\n') {
            return None;
        }
        let p = Path::new(s);
        let mut norm = PathBuf::new();
        for c in p.components() {
            match c {
                Component::CurDir => {}
                Component::ParentDir => {
                    if !norm.pop() {
                        return None;
                    }
                }
                o => norm.push(o.as_os_str()),
            }
        }
        if p.is_absolute() {
            self.roots.iter().find_map(|r| norm.strip_prefix(r).ok()).map(|rel| self.root.join(rel))
        } else {
            Some(self.root.join(norm))
        }
    }

    /// The new text for a path string, if the string names something that
    /// moved. Folders themselves are not rewritten (a clip called "Models"
    /// stays), only paths inside a folder that went to one place.
    fn rewrite(&self, s: &str) -> Option<String> {
        let key = self.old_key(s)?;
        let new = match self.files.get(&key) {
            Some(n) => n.clone(),
            None => {
                let anc = key.ancestors().skip(1).take_while(|a| *a != self.root).find(|a| self.dirs.contains_key(*a))?;
                self.dirs[anc].join(key.strip_prefix(anc).ok()?)
            }
        };
        Some(if Path::new(s).is_absolute() {
            new.to_string_lossy().into_owned()
        } else {
            match new.strip_prefix(&self.package) {
                Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
                Err(_) => new.to_string_lossy().into_owned(),
            }
        })
    }

    /// Rewrites every string in `v`; returns how many changed.
    fn walk(&self, v: &mut Value) -> usize {
        match v {
            Value::String(s) => match self.rewrite(s) {
                Some(n) if n != *s => {
                    *s = n;
                    1
                }
                _ => 0,
            },
            Value::Array(a) => a.iter_mut().map(|x| self.walk(x)).sum(),
            Value::Object(o) => o.values_mut().map(|x| self.walk(x)).sum(),
            _ => 0,
        }
    }
}

/// Media paths of a project document: (index, path, relative_path).
fn media_paths(doc: &Value) -> Vec<(usize, String, Option<String>)> {
    doc.pointer("/project/media")
        .and_then(Value::as_array)
        .map(|a| a.iter().enumerate().filter_map(|(i, m)| Some((i, m.get("path")?.as_str()?.to_owned(), m.get("relative_path").and_then(Value::as_str).map(str::to_owned)))).collect())
        .unwrap_or_default()
}

/// Whether a media path names an existing file, looking in `base` for
/// relative ones.
fn names_a_file(path: &str, base: &Path) -> bool {
    let p = Path::new(path);
    if p.is_absolute() { p.exists() } else { base.join(p).exists() }
}

// --------------------------------------------------------------- moving

fn is_present(p: &Path) -> bool {
    p.symlink_metadata().is_ok()
}

fn move_file(from: &Path, to: &Path) -> io::Result<()> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)?;
    }
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
            let want = std::fs::metadata(from)?.len();
            std::fs::copy(from, to)?;
            if std::fs::metadata(to)?.len() != want {
                let _ = std::fs::remove_file(to);
                return Err(io::Error::other(format!("{} was not copied whole", from.display())));
            }
            std::fs::remove_file(from)
        }
        Err(e) => Err(e),
    }
}

fn copy_file(from: &Path, to: &Path) -> io::Result<()> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let want = std::fs::metadata(from)?.len();
    std::fs::copy(from, to)?;
    if std::fs::metadata(to)?.len() != want {
        let _ = std::fs::remove_file(to);
        return Err(io::Error::other(format!("{} was not copied whole", from.display())));
    }
    Ok(())
}

/// Puts moved files back, newest first; what could not be put back.
/// Works from what is on disk, so it is safe to run twice or after a crash.
fn undo(moves: &[(PathBuf, PathBuf)], copy: bool) -> Vec<PathBuf> {
    let mut stuck = Vec::new();
    for (from, to) in moves.iter().rev() {
        if !is_present(to) {
            continue;
        }
        if copy || is_present(from) {
            // A copy, or half of one: the original is still where it was.
            let _ = std::fs::remove_file(to);
        } else if move_file(to, from).is_err() {
            stuck.push(to.clone());
        }
    }
    stuck
}

/// Removes empty folders under `dir`, deepest first, then `dir` itself if it
/// ended up empty. Never removes a file.
fn remove_empty_dirs(dir: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else { return false };
    let mut empty = true;
    for e in rd.flatten() {
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if !(is_dir && remove_empty_dirs(&e.path())) {
            empty = false;
        }
    }
    empty && std::fs::remove_dir(dir).is_ok()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn busy(root: &Path) -> bool {
    for name in [".project.open", ".project.lock"] {
        let Ok(f) = std::fs::OpenOptions::new().write(true).open(root.join(name)) else { continue };
        if f.try_lock().is_err() {
            return true;
        }
        let _ = f.unlock();
    }
    false
}

struct Journal {
    root: PathBuf,
    staging: PathBuf,
    package: PathBuf,
    exports: PathBuf,
    exports_new: bool,
    copy: bool,
    moves: Vec<(PathBuf, PathBuf)>,
}

impl Journal {
    fn to_json(&self) -> Value {
        json!({
            "format": "zornov-convert", "version": 1,
            "root": self.root, "staging": self.staging, "package": self.package,
            "exports": self.exports, "exports_new": self.exports_new, "copy": self.copy,
            "moves": self.moves.iter().map(|(f, t)| json!([f, t])).collect::<Vec<_>>(),
        })
    }

    fn from_json(v: &Value) -> Option<Journal> {
        let p = |k: &str| v.get(k).and_then(Value::as_str).map(PathBuf::from);
        Some(Journal {
            root: p("root")?,
            staging: p("staging")?,
            package: p("package")?,
            exports: p("exports")?,
            exports_new: v.get("exports_new")?.as_bool()?,
            copy: v.get("copy")?.as_bool()?,
            moves: v.get("moves")?.as_array()?.iter().filter_map(|m| Some((PathBuf::from(m.get(0)?.as_str()?), PathBuf::from(m.get(1)?.as_str()?)))).collect(),
        })
    }

    /// Everything back as it was; the staging folder (ours alone) goes.
    fn roll_back(&self) -> Vec<PathBuf> {
        let stuck = undo(&self.moves, self.copy);
        if stuck.is_empty() {
            let _ = std::fs::remove_dir_all(&self.staging);
            if self.exports_new {
                remove_empty_dirs(&self.exports);
            }
            let _ = std::fs::remove_file(self.root.join(JOURNAL_FILE));
        }
        stuck
    }
}

fn free_package_path(parent: &Path, base: &str) -> PathBuf {
    let mut n = 1;
    loop {
        let name = if n == 1 { format!("{base}.{PACKAGE_EXT}") } else { format!("{base} {n}.{PACKAGE_EXT}") };
        let p = parent.join(name);
        if !is_present(&p) {
            return p;
        }
        n += 1;
    }
}

/// Converts the old folder at `root` into a package beside it. See the
/// module documentation for the steps and what happens on failure.
pub fn convert(root: &Path, opts: Options, hooks: &Hooks) -> Result<Report, ConvertError> {
    let mut plan = plan(root)?;
    if !opts.copy && busy(root) {
        return Err(ConvertError::Busy);
    }
    let parent = root.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new(".")).to_path_buf();
    let base = {
        let n = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Project".into());
        n.strip_suffix(&format!(".{PACKAGE_EXT}")).filter(|s| !s.is_empty()).map(str::to_owned).unwrap_or(n)
    };
    let package = free_package_path(&parent, &base);
    let package_name = package.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or(base);
    let exports = parent.join(format!("{package_name} Exports"));
    let staging = parent.join(format!(".{package_name}.{PACKAGE_EXT}-converting-{}", std::process::id()));

    // Deliverables never overwrite what is already in the exports folder.
    let mut claimed: HashSet<String> = HashSet::new();
    for m in plan.moves.iter_mut().filter(|m| m.place == Place::Exports) {
        let stem = m.to.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let ext = m.to.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let dir = m.to.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut n = 1;
        let mut cand = m.to.clone();
        while is_present(&exports.join(&cand)) || !claimed.insert(cand.to_string_lossy().to_lowercase()) {
            n += 1;
            cand = dir.join(format!("{stem} {n}{ext}"));
        }
        m.to = cand;
    }
    let abs: Vec<(PathBuf, PathBuf)> = plan
        .moves
        .iter()
        .map(|m| (root.join(&m.from), match m.place {
            Place::Package => staging.join(&m.to),
            Place::Exports => exports.join(&m.to),
        }))
        .collect();
    let journal = Journal { root: root.to_path_buf(), staging: staging.clone(), package: package.clone(), exports: exports.clone(), exports_new: !exports.exists(), copy: opts.copy, moves: abs.clone() };

    let project_text = std::fs::read_to_string(root.join(PROJECT_FILE)).map_err(|e| ConvertError::Failed { cause: format!("project.json: {e}"), rolled_back: true, stuck: Vec::new() })?;
    let mut doc: Value = serde_json::from_str(&project_text).map_err(|e| ConvertError::Failed { cause: format!("project.json: {e}"), rolled_back: true, stuck: Vec::new() })?;
    super::from_json(&project_text).map_err(|e| ConvertError::Failed { cause: format!("the project does not open as it is: {e}"), rolled_back: true, stuck: Vec::new() })?;
    let existed_before: Vec<bool> = media_paths(&doc).iter().map(|(_, p, rel)| names_a_file(p, root) || rel.as_deref().is_some_and(|r| names_a_file(r, root))).collect();

    let fail = |cause: String, journal: &Journal| -> ConvertError {
        let stuck = journal.roll_back();
        ConvertError::Failed { cause, rolled_back: stuck.is_empty(), stuck }
    };
    let stop = |s: Stage| -> Result<(), ConvertError> { if hooks.stop_after == Some(s) { Err(ConvertError::Stopped(s)) } else { Ok(()) } };

    // 1. Journal, then the moves.
    std::fs::create_dir_all(&staging).map_err(|e| ConvertError::Failed { cause: format!("{}: {e}", staging.display()), rolled_back: true, stuck: Vec::new() })?;
    if let Err(e) = write_atomic(&root.join(JOURNAL_FILE), serde_json::to_string_pretty(&journal.to_json()).unwrap_or_default().as_bytes()) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(ConvertError::Failed { cause: format!("journal: {e}"), rolled_back: true, stuck: Vec::new() });
    }
    stop(Stage::Journaled)?;
    for (i, (from, to)) in abs.iter().enumerate() {
        let r = match hooks.before_move {
            Some(h) => h(i),
            None => Ok(()),
        }
        .and_then(|()| if opts.copy { copy_file(from, to) } else { move_file(from, to) });
        if let Err(e) = r {
            return Err(fail(format!("{}: {e}", from.display()), &journal));
        }
    }
    // Every file arrived whole (before any document is rewritten).
    for (m, (_, to)) in plan.moves.iter().zip(&abs) {
        if std::fs::symlink_metadata(to).map(|md| md.len()).ok() != Some(m.size) && !to.is_symlink() {
            return Err(fail(format!("{} is not the size it was", to.display()), &journal));
        }
    }
    stop(Stage::Moved)?;

    // 2. The documents, with their paths rewritten.
    let rw = Rewriter::new(&plan, &package, &exports);
    let to_staging = |p: &Path| -> PathBuf { p.strip_prefix(&package).map(|r| staging.join(r)).unwrap_or_else(|_| p.to_path_buf()) };
    let mut report = Report { package: package.clone(), exports: plan.moves.iter().any(|m| m.place == Place::Exports).then(|| exports.clone()), moved: plan.moves.len(), unknown: plan.unknown.clone(), ..Default::default() };
    let outcome = (|| -> Result<(), String> {
        rw.walk(&mut doc);
        // Media that named nothing: the project-relative path, or a moved
        // file of the same name when exactly one matches.
        let exists_now = |p: &str| -> bool {
            let path = Path::new(p);
            if path.is_absolute() { to_staging(path).exists() } else { staging.join(path).exists() }
        };
        let mut edits: Vec<(usize, String, Option<String>)> = Vec::new();
        for (i, path, rel) in media_paths(&doc) {
            if exists_now(&path) {
                continue;
            }
            if let Some(r) = rel.as_deref().filter(|r| exists_now(r)) {
                edits.push((i, package.join(r).to_string_lossy().into_owned(), None));
                continue;
            }
            let name = Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match rw.by_name.get(&name).map(Vec::as_slice) {
                Some([only]) => {
                    report.relinked.push((path.clone(), only.to_string_lossy().into_owned()));
                    edits.push((i, only.to_string_lossy().into_owned(), only.strip_prefix(&package).ok().map(|r| r.to_string_lossy().replace('\\', "/"))));
                }
                _ => report.unresolved.push(path),
            }
        }
        if let Some(arr) = doc.pointer_mut("/project/media").and_then(Value::as_array_mut) {
            for (i, new, rel) in edits {
                if let Some(m) = arr.get_mut(i).and_then(Value::as_object_mut) {
                    m.insert("path".into(), Value::String(new));
                    match rel {
                        Some(r) => m.insert("relative_path".into(), Value::String(r)),
                        None => m.remove("relative_path"),
                    };
                }
            }
        }
        // Nothing that was there may have gone missing.
        for ((_, path, _), was) in media_paths(&doc).iter().zip(&existed_before) {
            if *was && !exists_now(path) {
                return Err(format!("{path} would not be found after the move"));
            }
        }
        let write = |name: &Path, v: &Value| -> Result<(), String> { write_atomic(&staging.join(name), serde_json::to_string_pretty(v).unwrap_or_default().as_bytes()).map_err(|e| format!("{}: {e}", name.display())) };
        write(Path::new(PROJECT_FILE), &doc)?;
        report.rewritten.push(PROJECT_FILE.into());
        for m in plan.moves.iter().filter(|m| m.place == Place::Package && m.to.starts_with("Versions") && m.to.extension().is_some_and(|e| e == "json")) {
            let path = staging.join(&m.to);
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let Ok(mut v) = serde_json::from_str::<Value>(&text) else { continue };
            if rw.walk(&mut v) > 0 {
                write(&m.to, &v)?;
                report.rewritten.push(m.to.to_string_lossy().into_owned());
            }
        }
        Ok(())
    })();
    if let Err(cause) = outcome {
        return Err(fail(cause, &journal));
    }
    stop(Stage::Written)?;

    // 3. The record of what moved, then the marker, last.
    let record = json!({"from": root, "at_ms": now_ms(), "moves": plan.moves.iter().map(|m| json!([m.from, match m.place { Place::Package => m.to.to_string_lossy().into_owned(), Place::Exports => format!("../{package_name} Exports/{}", m.to.display()) }])).collect::<Vec<_>>()});
    let marked = write_atomic(&staging.join(STATE_DIR).join("converted-from.json"), serde_json::to_string_pretty(&record).unwrap_or_default().as_bytes()).and_then(|()| write_atomic(&staging.join(STATE_DIR).join(MARKER_FILE), layout::marker_text().as_bytes()));
    if let Err(e) = marked {
        return Err(fail(format!("marker: {e}"), &journal));
    }
    stop(Stage::Marked)?;

    // 4. Check, then commit with one rename.
    if let Err(e) = super::from_json(&std::fs::read_to_string(staging.join(PROJECT_FILE)).unwrap_or_default()) {
        return Err(fail(format!("the converted project does not open: {e}"), &journal));
    }
    if is_present(&package) {
        return Err(fail(format!("{} appeared meanwhile", package.display()), &journal));
    }
    if let Err(e) = std::fs::rename(&staging, &package) {
        return Err(fail(format!("could not rename the staging folder: {e}"), &journal));
    }
    stop(Stage::Committed)?;

    // 5. Tidy: the carry-nothing files, empty folders, the journal.
    if !opts.copy {
        finish_old_folder(root, &plan, &mut report);
    } else {
        let _ = std::fs::remove_file(root.join(JOURNAL_FILE));
    }
    Ok(report)
}

fn finish_old_folder(root: &Path, plan: &Plan, report: &mut Report) {
    for d in &plan.dropped {
        let p = root.join(d);
        let name = d.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        let nothing = name == ".ds_store" || p.metadata().map(|m| m.len() == 0).unwrap_or(false);
        if nothing && name != JOURNAL_FILE {
            let _ = std::fs::remove_file(&p);
        }
    }
    let _ = std::fs::remove_file(root.join(JOURNAL_FILE));
    if !remove_empty_dirs(root) {
        let mut files = Vec::new();
        let _ = walk(root, root, &mut files);
        report.left_behind = files.into_iter().map(|(p, _)| p).collect();
    }
}

/// What [`recover`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recovered {
    /// No conversion was in progress.
    Nothing,
    /// The conversion had been committed; the old folder was tidied.
    Finished,
    /// The conversion had not been committed; every file was put back.
    RolledBack,
    /// Some files could not be put back; the journal is kept.
    Stuck,
}

/// Settles a conversion that a crash interrupted in the old folder `root`.
/// Safe to call on any folder.
pub fn recover(root: &Path) -> Recovered {
    let Some(j) = std::fs::read_to_string(root.join(JOURNAL_FILE)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| Journal::from_json(&v)) else { return Recovered::Nothing };
    let committed = j.package.join(STATE_DIR).join(MARKER_FILE).is_file();
    if committed {
        let plan = Plan { root: root.to_path_buf(), moves: Vec::new(), dropped: [".DS_Store", ".project.lock", ".project.open"].iter().map(PathBuf::from).collect(), unknown: Vec::new() };
        finish_old_folder(root, &plan, &mut Report::default());
        return Recovered::Finished;
    }
    if j.roll_back().is_empty() { Recovered::RolledBack } else { Recovered::Stuck }
}

#[cfg(test)]
#[path = "convert_tests.rs"]
mod convert_tests;
