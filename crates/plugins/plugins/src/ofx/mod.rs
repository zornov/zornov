// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! OpenFX: loading existing OFX image effect plugins (`.ofx.bundle`).
//!
//! OFX plugins are native code, so Studio never loads one into the
//! editor. Each bundle runs in its own worker process (the Studio binary
//! started as `zornov ofx-worker <bundle>`, or `studio-ofx-worker`), which
//! hosts the plugin with the suites in [`host`] and renders frames sent
//! over a pipe ([`worker`]). A worker that crashes or hangs is restarted a
//! few times and then left off; the picture passes through unchanged.
//!
//! Loading OFX plugins is off until the person turns it on (Settings, or
//! the agent with their consent), because it runs code from other
//! vendors with the person's rights: unlike the WebAssembly plugins, OFX
//! plugins are not sandboxed.
//!
//! Each plugin becomes an effect (`ofx:<identifier>`; generators are
//! generator effects, transitions transitions) with its double, integer,
//! boolean, choice, colour and 2D/3D parameters. Studio renders in linear
//! light; most OFX plugins expect display-referred pixels, so frames go to
//! them sRGB-encoded unless the plugin is set to linear. See [`host`] for
//! what of the OFX API is supported.

pub mod abi;
pub mod bundle;
pub mod host;
pub mod worker;

use std::ffi::OsString;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use studio_fx::effects::{Category, EffectContext, EffectDef};
use studio_fx::external::{self, EffectRun, TransitionRun};
use studio_fx::image::Image;
use studio_fx::params::{ParamDef, ParamSet, ParamValue};
use studio_fx::transitions::{TransitionContext, TransitionDef};

pub use bundle::{ParamInfo, PluginInfo};

/// Where OFX bundles are looked for: `OFX_PLUGIN_PATH`, then the standard
/// folders of the platform, then folders the person added.
pub fn search_paths(extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::env::var_os("OFX_PLUGIN_PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if cfg!(target_os = "macos") {
        v.push(PathBuf::from("/Library/OFX/Plugins"));
        v.push(studio_paths::home().join("Library/OFX/Plugins"));
    } else if cfg!(windows) {
        v.push(PathBuf::from(r"C:\Program Files\Common Files\OFX\Plugins"));
    } else {
        v.push(PathBuf::from("/usr/OFX/Plugins"));
    }
    v.extend(extra.iter().cloned());
    v
}

/// Every `.ofx.bundle` in the search paths (and one folder down).
pub fn find_bundles(extra: &[PathBuf], only_extra: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let dirs = if only_extra { extra.to_vec() } else { search_paths(extra) };
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.to_string_lossy().ends_with(".ofx.bundle") {
                out.push(p);
            } else if p.is_dir() {
                for e in std::fs::read_dir(&p).into_iter().flatten().flatten() {
                    if e.path().to_string_lossy().ends_with(".ofx.bundle") {
                        out.push(e.path());
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The person's OFX settings, `<Studio>/Plugins/ofx.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Off until turned on.
    #[serde(default)]
    pub enabled: bool,
    /// Plugin identifiers turned off.
    #[serde(default)]
    pub off: Vec<String>,
    /// Plugin identifiers that get linear-light pixels instead of sRGB.
    #[serde(default)]
    pub linear: Vec<String>,
    /// Folders to search besides the standard ones.
    #[serde(default)]
    pub paths: Vec<PathBuf>,
    /// Search only `paths` (tests, or a person who keeps OFX plugins
    /// somewhere of their own).
    #[serde(default)]
    pub only_paths: bool,
}

impl Settings {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join("ofx.json")
    }
    pub fn load(dir: &Path) -> Settings {
        std::fs::read(Self::path(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        std::fs::write(Self::path(dir), serde_json::to_vec_pretty(self).unwrap_or_default()).map_err(|e| e.to_string())
    }
}

/// A bundled worker program that may be launched: a regular file (not a
/// symlink) in the same folder as this program, and on unix not writable by
/// group or others. Returns its canonical path.
fn verified_sibling(candidate: &Path, exe_dir: &Path) -> Option<PathBuf> {
    if !std::fs::symlink_metadata(candidate).ok()?.file_type().is_file() {
        return None;
    }
    let canon = candidate.canonicalize().ok()?;
    if canon.parent()? != exe_dir {
        return None;
    }
    let meta = std::fs::metadata(&canon).ok()?;
    if !meta.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o022 != 0 {
            return None;
        }
    }
    Some(canon)
}

/// Which program runs a worker: `env` (`STUDIO_OFX_WORKER`) only when
/// `allow_env` (debug and test builds); otherwise a verified
/// `studio-ofx-worker` next to `exe`, else `exe` itself. Never a `PATH`
/// lookup: an `exe` that cannot be resolved is an error.
fn worker_program(env: Option<OsString>, exe: &Path, allow_env: bool) -> Result<PathBuf, String> {
    if allow_env {
        if let Some(p) = env {
            return Ok(PathBuf::from(p));
        }
    }
    let exe = exe.canonicalize().map_err(|e| format!("could not locate the OFX worker: {e}"))?;
    let dir = exe.parent().ok_or("could not locate the OFX worker")?;
    Ok(verified_sibling(&dir.join("studio-ofx-worker"), dir).unwrap_or(exe))
}

/// How to start a worker (see `worker_program`).
fn worker_command(bundle: &Path) -> Result<Command, String> {
    let exe = std::env::current_exe().map_err(|e| format!("could not locate the OFX worker: {e}"))?;
    let program = worker_program(std::env::var_os("STUDIO_OFX_WORKER"), &exe, cfg!(debug_assertions) || cfg!(test))?;
    let mut c = Command::new(program);
    c.arg("ofx-worker").arg(bundle);
    // `studio-ofx-worker` is a console program; the window must not get one.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    Ok(c)
}

struct Worker {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    replies: Receiver<std::io::Result<(Value, Vec<u8>)>>,
}

impl Worker {
    fn start(bundle: &Path) -> Result<(Worker, Value), String> {
        let mut child = worker_command(bundle)?.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| format!("could not start the OFX worker: {e}"))?;
        let stdin = BufWriter::new(child.stdin.take().ok_or("no stdin")?);
        let mut stdout = BufReader::new(child.stdout.take().ok_or("no stdout")?);
        let (tx, rx) = channel();
        std::thread::Builder::new()
            .name("ofx-reader".into())
            .spawn(move || {
                loop {
                    let r = worker::read_msg(&mut stdout);
                    let end = r.is_err();
                    if tx.send(r).is_err() || end {
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        let mut w = Worker { child, stdin, replies: rx };
        let (hello, _) = w.reply(Duration::from_secs(60)).inspect_err(|_| w.kill())?;
        if hello["ok"] != true {
            w.kill();
            return Err(hello["error"].as_str().unwrap_or("the bundle did not load").to_owned());
        }
        Ok((w, hello))
    }

    fn reply(&mut self, timeout: Duration) -> Result<(Value, Vec<u8>), String> {
        match self.replies.recv_timeout(timeout) {
            Ok(Ok(m)) => Ok(m),
            Ok(Err(e)) => Err(format!("the OFX worker stopped ({e})")),
            Err(RecvTimeoutError::Timeout) => Err("the OFX plugin did not answer in time".into()),
            Err(RecvTimeoutError::Disconnected) => Err("the OFX worker stopped".into()),
        }
    }

    fn call(&mut self, head: &Value, payload: &[u8], timeout: Duration) -> Result<(Value, Vec<u8>), String> {
        worker::write_msg(&mut self.stdin, head, payload).map_err(|e| format!("the OFX worker stopped ({e})"))?;
        self.reply(timeout)
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = worker::write_msg(&mut self.stdin, &json!({"op": "quit"}), &[]);
        std::thread::sleep(Duration::from_millis(20));
        self.kill();
    }
}

/// One bundle and its worker.
pub struct Bundle {
    pub path: PathBuf,
    pub plugins: Vec<PluginInfo>,
    pub skipped: Vec<(String, String)>,
    pub error: Option<String>,
    worker: Mutex<Option<Worker>>,
    crashes: Mutex<u32>,
}

/// Restarts allowed before a bundle is left off.
const MAX_CRASHES: u32 = 3;

impl Bundle {
    /// Starts a worker for a bundle and reads what it holds.
    pub fn open(path: &Path) -> Bundle {
        match Worker::start(path) {
            Ok((w, hello)) => Bundle {
                path: path.to_owned(),
                plugins: serde_json::from_value(hello["plugins"].clone()).unwrap_or_default(),
                skipped: serde_json::from_value(hello["skipped"].clone()).unwrap_or_default(),
                error: None,
                worker: Mutex::new(Some(w)),
                crashes: Mutex::new(0),
            },
            Err(e) => Bundle { path: path.to_owned(), plugins: Vec::new(), skipped: Vec::new(), error: Some(e), worker: Mutex::new(None), crashes: Mutex::new(0) },
        }
    }

    /// Renders a frame in the worker, restarting it after a crash.
    #[allow(clippy::too_many_arguments)]
    pub fn render(&self, index: usize, inputs: &[&[[f32; 4]]], w: usize, h: usize, frame: f64, fps: f64, progress: f64, params: &serde_json::Map<String, Value>) -> Result<Vec<[f32; 4]>, String> {
        let mut g = self.worker.lock().unwrap_or_else(|e| e.into_inner());
        if g.is_none() {
            let mut c = self.crashes.lock().unwrap_or_else(|e| e.into_inner());
            if *c >= MAX_CRASHES {
                return Err(format!("{} crashed {MAX_CRASHES} times and is off until Zornov restarts", self.path.display()));
            }
            *c += 1;
            *g = Some(Worker::start(&self.path)?.0);
        }
        let worker = g.as_mut().expect("started");
        let head = json!({"op": "render", "index": index, "width": w, "height": h, "frame": frame, "fps": fps, "progress": progress, "params": params, "inputs": inputs.len()});
        match worker.call(&head, &worker::frames_to_bytes(inputs), Duration::from_secs(120)) {
            Ok((r, payload)) if r["ok"] == true && payload.len() == w * h * 16 => Ok(worker::bytes_to_frame(&payload)),
            Ok((r, _)) => Err(r["error"].as_str().unwrap_or("the plugin returned no picture").to_owned()),
            Err(e) => {
                // Dead or hung: throw the worker away; the next call restarts it.
                if let Some(mut w) = g.take() {
                    w.kill();
                }
                Err(e)
            }
        }
    }
}

// ------------------------------------------------- parameters and pixels

/// How a Studio parameter feeds an OFX one.
#[derive(Clone, Debug)]
enum Feed {
    Scalar(String),
    Colour(String, usize),
    /// A point: normalised (0..1, y up) or canonical pixels.
    Point(String, bool),
    Parts(String, Vec<String>),
}

fn snake(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 && !out.ends_with('_') {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let out = out.trim_matches('_').to_owned();
    if out.is_empty() || !out.as_bytes()[0].is_ascii_alphabetic() { format!("p_{out}") } else { out }
}

fn range(p: &ParamInfo, def: f64) -> (f64, f64) {
    let lo = p.min.unwrap_or(def.min(0.0));
    let hi = p.max.unwrap_or(if def.abs() > 1.0 { def.abs() * 2.0 } else { 1.0 });
    let (lo, hi) = (lo.min(def), hi.max(def));
    if hi > lo { (lo, hi) } else { (lo, lo + 1.0) }
}

/// Studio parameters for an OFX plugin, and how each feeds the plugin.
fn map_params(info: &PluginInfo) -> (Vec<ParamDef>, Vec<Feed>) {
    let mut defs = Vec::new();
    let mut feeds = Vec::new();
    let mut used = std::collections::HashSet::new();
    let leak = external::leak_str;
    let mut id = |name: &str| {
        let mut base = snake(name);
        while !used.insert(base.clone()) {
            base.push('_');
        }
        leak(&base)
    };
    for p in &info.params {
        if p.secret || (info.context == abi::CONTEXT_TRANSITION && p.name == "Transition") {
            continue;
        }
        let d0 = p.default.first().copied().unwrap_or(0.0);
        let label = leak(&p.label);
        match p.kind.as_str() {
            abi::PARAM_DOUBLE => {
                let (lo, hi) = range(p, d0);
                defs.push(ParamDef::float(id(&p.name), label, lo as f32, hi as f32, d0 as f32));
                feeds.push(Feed::Scalar(p.name.clone()));
            }
            abi::PARAM_INT => {
                let (lo, hi) = range(p, d0);
                defs.push(ParamDef::int(id(&p.name), label, lo as i32, hi.max(lo + 1.0) as i32, d0 as i32));
                feeds.push(Feed::Scalar(p.name.clone()));
            }
            abi::PARAM_BOOL => {
                defs.push(ParamDef::bool(id(&p.name), label, d0 != 0.0));
                feeds.push(Feed::Scalar(p.name.clone()));
            }
            abi::PARAM_CHOICE if !p.options.is_empty() => {
                let opts: Vec<&'static str> = p.options.iter().map(|o| leak(o)).collect();
                let n = opts.len();
                defs.push(ParamDef::choice(id(&p.name), label, Box::leak(opts.into_boxed_slice()), (d0.max(0.0) as usize).min(n - 1) as u32));
                feeds.push(Feed::Scalar(p.name.clone()));
            }
            abi::PARAM_RGBA | abi::PARAM_RGB => {
                let c = |i: usize| p.default.get(i).copied().unwrap_or(1.0) as f32;
                defs.push(ParamDef::colour(id(&p.name), label, [c(0), c(1), c(2), if p.kind == abi::PARAM_RGBA { c(3) } else { 1.0 }]));
                feeds.push(Feed::Colour(p.name.clone(), if p.kind == abi::PARAM_RGBA { 4 } else { 3 }));
            }
            abi::PARAM_DOUBLE2D if p.double_type.contains("XY") => {
                let normalised = p.double_type.contains("Normalised");
                let (x, y) = (d0, p.default.get(1).copied().unwrap_or(0.0));
                // Shown as a frame position; canonical defaults are pixels
                // of a frame Studio does not know yet, so assume 1920x1080.
                let (nx, ny) = if normalised { (x, 1.0 - y) } else { (x / 1920.0, 1.0 - y / 1080.0) };
                defs.push(ParamDef::point(id(&p.name), label, [nx.clamp(-1.0, 2.0) as f32, ny.clamp(-1.0, 2.0) as f32]));
                feeds.push(Feed::Point(p.name.clone(), normalised));
            }
            abi::PARAM_DOUBLE2D | abi::PARAM_DOUBLE3D | abi::PARAM_INT2D | abi::PARAM_INT3D => {
                let axes = if p.kind.ends_with("2D") { &["x", "y"][..] } else { &["x", "y", "z"][..] };
                let mut ids = Vec::new();
                for (i, a) in axes.iter().enumerate() {
                    let d = p.default.get(i).copied().unwrap_or(0.0);
                    let (lo, hi) = range(p, d);
                    let pid = id(&format!("{}_{a}", p.name));
                    let plabel = leak(&format!("{} {}", p.label, a.to_uppercase()));
                    defs.push(if p.kind.starts_with("OfxParamTypeInteger") { ParamDef::int(pid, plabel, lo as i32, hi.max(lo + 1.0) as i32, d as i32) } else { ParamDef::float(pid, plabel, lo as f32, hi as f32, d as f32) });
                    ids.push(pid.to_owned());
                }
                feeds.push(Feed::Parts(p.name.clone(), ids));
            }
            _ => {}
        }
    }
    (defs, feeds)
}

/// The OFX values for a Studio parameter set.
fn values(defs: &[ParamDef], feeds: &[Feed], set: &ParamSet, w: usize, h: usize) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    let mut di = 0;
    for f in feeds {
        match f {
            Feed::Scalar(name) => {
                let v = match set.get(defs[di].id) {
                    Some(ParamValue::Float(x)) => json!(x),
                    Some(ParamValue::Int(x)) => json!(x),
                    Some(ParamValue::Bool(b)) => json!(b as u8),
                    Some(ParamValue::Choice(c)) => json!(c),
                    _ => Value::Null,
                };
                m.insert(name.clone(), v);
                di += 1;
            }
            Feed::Colour(name, n) => {
                let c = set.colour(defs[di].id);
                m.insert(name.clone(), json!(c[..*n]));
                di += 1;
            }
            Feed::Point(name, normalised) => {
                let p = set.point(defs[di].id);
                let (x, y) = (p[0] as f64, 1.0 - p[1] as f64);
                m.insert(name.clone(), if *normalised { json!([x, y]) } else { json!([x * w as f64, y * h as f64]) });
                di += 1;
            }
            Feed::Parts(name, ids) => {
                let vals: Vec<f64> = ids.iter().map(|id| match set.get(id) {
                    Some(ParamValue::Float(x)) => x as f64,
                    Some(ParamValue::Int(x)) => x as f64,
                    _ => 0.0,
                }).collect();
                m.insert(name.clone(), json!(vals));
                di += ids.len();
            }
        }
    }
    m
}

fn to_display(px: &[[f32; 4]]) -> Vec<[f32; 4]> {
    px.iter().map(|p| {
        let a = p[3];
        if a <= 1e-6 {
            return *p;
        }
        let e = |v: f32| studio_fx::image::linear_to_srgb((v / a).max(0.0)) * a;
        [e(p[0]), e(p[1]), e(p[2]), a]
    }).collect()
}

fn from_display(px: &mut [[f32; 4]]) {
    for p in px {
        let a = p[3];
        if a <= 1e-6 {
            continue;
        }
        let d = |v: f32| studio_fx::image::srgb_to_linear((v / a).max(0.0)) * a;
        *p = [d(p[0]), d(p[1]), d(p[2]), a];
    }
}

struct OfxEffect {
    bundle: Arc<Bundle>,
    index: usize,
    defs: &'static [ParamDef],
    feeds: Vec<Feed>,
    srgb: bool,
    generator: bool,
}

impl EffectRun for OfxEffect {
    fn run(&self, ctx: &EffectContext, input: &Image, params: &ParamSet) -> Image {
        let (w, h) = (input.width, input.height);
        let vals = values(self.defs, &self.feeds, params, w, h);
        let src = if self.srgb { to_display(&input.pixels) } else { input.pixels.clone() };
        let inputs: Vec<&[[f32; 4]]> = if self.generator { Vec::new() } else { vec![&src] };
        match self.bundle.render(self.index, &inputs, w, h, (ctx.time * ctx.frame_rate) as f64, ctx.frame_rate as f64, 0.0, &vals) {
            Ok(mut px) => {
                if self.srgb {
                    from_display(&mut px);
                }
                if self.generator {
                    // Generators draw over their input, as Studio's do.
                    for (o, i) in px.iter_mut().zip(&input.pixels) {
                        let k = 1.0 - o[3];
                        for c in 0..4 {
                            o[c] += i[c] * k;
                        }
                    }
                }
                Image { width: w, height: h, pixels: px }
            }
            Err(e) => {
                log(format!("{}: {e}", self.bundle.path.display()));
                input.clone()
            }
        }
    }
}

struct OfxTransition {
    bundle: Arc<Bundle>,
    index: usize,
    defs: &'static [ParamDef],
    feeds: Vec<Feed>,
    srgb: bool,
}

impl TransitionRun for OfxTransition {
    fn run(&self, _ctx: &TransitionContext, a: &Image, b: &Image, progress: f32, params: &ParamSet) -> Image {
        let (w, h) = (a.width, a.height);
        let vals = values(self.defs, &self.feeds, params, w, h);
        let (sa, sb) = if self.srgb { (to_display(&a.pixels), to_display(&b.pixels)) } else { (a.pixels.clone(), b.pixels.clone()) };
        match self.bundle.render(self.index, &[&sa, &sb], w, h, 0.0, 30.0, progress as f64, &vals) {
            Ok(mut px) => {
                if self.srgb {
                    from_display(&mut px);
                }
                Image { width: w, height: h, pixels: px }
            }
            Err(e) => {
                log(format!("{}: {e}", self.bundle.path.display()));
                if progress < 0.5 { a.clone() } else { b.clone() }
            }
        }
    }
}

/// Recent OFX problems, newest last.
pub fn messages() -> Vec<String> {
    LOG.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn log(line: String) {
    let mut l = LOG.lock().unwrap_or_else(|e| e.into_inner());
    if l.last() != Some(&line) {
        l.push(line);
    }
    if l.len() > 100 {
        l.remove(0);
    }
}

/// What a loaded OFX plugin became in Studio.
#[derive(Clone, Debug, Serialize)]
pub struct Loaded {
    pub id: String,
    pub studio_id: String,
    pub label: String,
    pub grouping: String,
    pub kind: &'static str,
    pub bundle: PathBuf,
    pub enabled: bool,
    pub params: usize,
}

struct State {
    bundles: Vec<Arc<Bundle>>,
    loaded: Vec<Loaded>,
}

fn state() -> &'static Mutex<Option<State>> {
    static S: OnceLock<Mutex<Option<State>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

fn category(grouping: &str) -> Category {
    let g = grouping.to_lowercase();
    if g.contains("blur") {
        Category::Blur
    } else if g.contains("key") || g.contains("matte") {
        Category::Keying
    } else if g.contains("distort") || g.contains("warp") {
        Category::Distort
    } else if g.contains("light") || g.contains("glow") {
        Category::Light
    } else if g.contains("time") {
        Category::Time
    } else {
        Category::Stylise
    }
}

/// Starts workers for every bundle and registers their plugins with `fx`.
/// Does nothing unless OFX is turned on in `dir`'s settings. Returns what
/// was loaded (including plugins turned off), and replaces an earlier load.
pub fn load(dir: &Path) -> Vec<Loaded> {
    unload();
    let settings = Settings::load(dir);
    if !settings.enabled {
        return Vec::new();
    }
    let mut bundles = Vec::new();
    let mut loaded = Vec::new();
    for path in find_bundles(&settings.paths, settings.only_paths) {
        let b = Arc::new(Bundle::open(&path));
        if let Some(e) = &b.error {
            log(format!("{}: {e}", path.display()));
        }
        for (id, why) in &b.skipped {
            log(format!("{id} ({}): {why}", path.display()));
        }
        for info in &b.plugins {
            let studio_id = format!("ofx:{}", info.id);
            let enabled = !settings.off.contains(&info.id);
            let srgb = !settings.linear.contains(&info.id);
            let (defs, feeds) = map_params(info);
            let defs = external::leak_params(defs);
            let kind = match info.context.as_str() {
                abi::CONTEXT_TRANSITION => "transition",
                abi::CONTEXT_GENERATOR => "generator",
                _ => "effect",
            };
            if enabled {
                let name = external::leak_str(&info.label);
                let desc = external::leak_str(if info.description.is_empty() { &info.grouping } else { &info.description });
                let sid = external::leak_str(&studio_id);
                let r = if kind == "transition" {
                    let run: &'static OfxTransition = Box::leak(Box::new(OfxTransition { bundle: b.clone(), index: info.index, defs, feeds, srgb }));
                    external::register_transition(external::leak_transition(TransitionDef::external(sid, name, desc, defs, None, run)))
                } else {
                    let generator = kind == "generator";
                    let run: &'static OfxEffect = Box::leak(Box::new(OfxEffect { bundle: b.clone(), index: info.index, defs, feeds, srgb, generator }));
                    let cat = if generator { Category::Generator } else { category(&info.grouping) };
                    // OFX plugins may animate on their own; never reuse frames.
                    external::register_effect(external::leak_effect(EffectDef::external(sid, name, cat, desc, defs, None, true, run)))
                };
                if let Err(e) = r {
                    log(format!("{}: {e}", info.id));
                }
            }
            loaded.push(Loaded { id: info.id.clone(), studio_id, label: info.label.clone(), grouping: info.grouping.clone(), kind, bundle: path.clone(), enabled, params: defs.len() });
        }
        bundles.push(b);
    }
    *state().lock().unwrap_or_else(|e| e.into_inner()) = Some(State { bundles, loaded: loaded.clone() });
    loaded
}

/// Takes every OFX plugin out of `fx` and stops the workers.
pub fn unload() {
    if let Some(s) = state().lock().unwrap_or_else(|e| e.into_inner()).take() {
        for l in &s.loaded {
            external::unregister_effect(&l.studio_id);
            external::unregister_transition(&l.studio_id);
        }
        drop(s.bundles);
    }
}

/// What the last [`load`] found.
pub fn loaded() -> Vec<Loaded> {
    state().lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|s| s.loaded.clone()).unwrap_or_default()
}

/// Bundles that failed to start, with why.
pub fn failed() -> Vec<(PathBuf, String)> {
    state().lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|s| s.bundles.iter().filter_map(|b| b.error.clone().map(|e| (b.path.clone(), e))).collect()).unwrap_or_default()
}

/// Runs the worker when this process was started as one
/// (`<program> ofx-worker <bundle>`). Call first thing in `main`.
pub fn worker_if_asked() {
    let mut args = std::env::args_os().skip(1);
    if args.next().is_some_and(|a| a == "ofx-worker")
        && let Some(b) = args.next()
    {
        worker::main(Path::new(&b));
    }
}

#[cfg(test)]
mod worker_program_tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("zornov-ofx-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.canonicalize().unwrap()
    }

    fn touch(p: &Path) {
        std::fs::write(p, b"x").unwrap();
    }

    #[cfg(unix)]
    fn chmod(p: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn env_honoured_when_allowed() {
        let d = tmp("env-on");
        let exe = d.join("zornov");
        touch(&exe);
        let got = worker_program(Some("/opt/custom-worker".into()), &exe, true).unwrap();
        assert_eq!(got, PathBuf::from("/opt/custom-worker"));
    }

    #[test]
    fn env_ignored_when_not_allowed() {
        let d = tmp("env-off");
        let exe = d.join("zornov");
        let evil = d.join("evil");
        touch(&exe);
        touch(&evil);
        let got = worker_program(Some(evil.clone().into_os_string()), &exe, false).unwrap();
        assert_ne!(got, evil);
        assert_eq!(got, exe);
    }

    #[test]
    fn sibling_chosen_when_valid() {
        let d = tmp("sibling");
        let exe = d.join("zornov");
        let sib = d.join("studio-ofx-worker");
        touch(&exe);
        touch(&sib);
        #[cfg(unix)]
        chmod(&sib, 0o755);
        assert_eq!(worker_program(None, &exe, false).unwrap(), sib);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_sibling_rejected() {
        let d = tmp("symlink");
        let exe = d.join("zornov");
        let target = d.join("elsewhere");
        touch(&exe);
        touch(&target);
        std::os::unix::fs::symlink(&target, d.join("studio-ofx-worker")).unwrap();
        assert_eq!(worker_program(None, &exe, false).unwrap(), exe);
    }

    #[cfg(unix)]
    #[test]
    fn world_writable_sibling_rejected() {
        let d = tmp("writable");
        let exe = d.join("zornov");
        let sib = d.join("studio-ofx-worker");
        touch(&exe);
        touch(&sib);
        chmod(&sib, 0o757);
        assert_eq!(worker_program(None, &exe, false).unwrap(), exe);
        chmod(&sib, 0o775);
        assert_eq!(worker_program(None, &exe, false).unwrap(), exe);
    }

    #[test]
    fn falls_back_to_exe() {
        let d = tmp("fallback");
        let exe = d.join("zornov");
        touch(&exe);
        assert_eq!(worker_program(None, &exe, false).unwrap(), exe);
    }

    #[test]
    fn unresolvable_exe_is_an_error_not_a_path_lookup() {
        let d = tmp("noexe");
        assert!(worker_program(None, &d.join("missing"), false).is_err());
        assert!(worker_program(None, Path::new("zornov"), false).is_err());
    }
}
