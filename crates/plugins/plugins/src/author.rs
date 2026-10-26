// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Plugins the AI (or anyone) writes inside Studio.
//!
//! Studio writes a small crate under `<Studio>/Plugin sources/<id>/`
//! against a copy of `studio-plugin-api` it carries, builds it with the
//! Rust toolchain on this Mac (`cargo`, target `wasm32-unknown-unknown`;
//! offline, no other crates), runs every item on a test picture, and only
//! then installs it. The source stays in the folder for the person to read
//! and change.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::store::{Host, Installed, inspect};

const API_LIB: &str = include_str!("../../plugin-api/src/lib.rs");
const API_JSON: &str = include_str!("../../plugin-api/src/json.rs");
const API_VERSION: &str = "1.0.0";

/// What to build.
#[derive(Clone, Debug, Default)]
pub struct Spec {
    /// Plugin id, `lower-kebab-case`.
    pub id: String,
    pub name: String,
    pub description: String,
    /// A whole `lib.rs` using `studio_plugin_api`. When set, the fields
    /// below are ignored.
    pub source: Option<String>,
    /// The body of `fn pixel(px: Px, x: f32, y: f32, w: f32, h: f32, ctx:
    /// &Context, p: &Params) -> Px` for a one-effect plugin: `px` is the
    /// premultiplied linear pixel at (x, y), pixel centres at +0.5.
    pub pixel: Option<String>,
    /// Parameters for `pixel`, as manifest entries: `{"id", "label",
    /// "kind": float|int|bool|choice|colour|point, "min", "max",
    /// "default", "options"}`.
    pub params: Vec<Value>,
    /// Optional WGSL body (`fn effect(pos: vec2<f32>) -> vec4<f32>`).
    pub wgsl: Option<String>,
    /// Whether the result changes over time.
    pub animated: bool,
}

/// Where AI-made plugin sources are kept.
pub fn sources_dir() -> PathBuf {
    studio_paths::studio().join("Plugin sources")
}

/// Finds `cargo` and checks the wasm target is installed.
pub fn toolchain() -> Result<PathBuf, String> {
    let home = studio_paths::home();
    let candidates = [PathBuf::from("cargo"), home.join(".cargo/bin/cargo")];
    let cargo = candidates.into_iter().find(|c| Command::new(c).arg("--version").output().is_ok_and(|o| o.status.success())).ok_or("Rust is not installed on this Mac, so Zornov cannot build plugins (install it from rustup.rs)")?;
    let rustc = if cargo.parent().is_some_and(|p| !p.as_os_str().is_empty()) { cargo.with_file_name("rustc") } else { PathBuf::from("rustc") };
    let sysroot = Command::new(rustc).args(["--print", "sysroot"]).output().map_err(|e| e.to_string())?;
    let sysroot = PathBuf::from(String::from_utf8_lossy(&sysroot.stdout).trim());
    if !sysroot.join("lib/rustlib/wasm32-unknown-unknown").is_dir() {
        return Err("the Rust WebAssembly target is missing; run `rustup target add wasm32-unknown-unknown` once".into());
    }
    Ok(cargo)
}

fn rust_str(s: &str) -> String {
    format!("{s:?}")
}

fn param_code(p: &Value) -> Result<String, String> {
    let id = p["id"].as_str().ok_or("every parameter needs an id")?;
    let label = p["label"].as_str().unwrap_or(id);
    let f = |k: &str, or: f64| p[k].as_f64().unwrap_or(or);
    let arr = |n: usize, or: f64| (0..n).map(|i| format!("{:?}f32", p["default"].get(i).and_then(Value::as_f64).unwrap_or(or) as f32)).collect::<Vec<_>>().join(", ");
    Ok(match p["kind"].as_str().unwrap_or("float") {
        "float" => format!("Param::float({}, {}, {:?}f32, {:?}f32, {:?}f32)", rust_str(id), rust_str(label), f("min", 0.0) as f32, f("max", 1.0) as f32, f("default", f("min", 0.0)) as f32),
        "int" => format!("Param::int({}, {}, {}, {}, {})", rust_str(id), rust_str(label), f("min", 0.0) as i32, f("max", 10.0) as i32, f("default", f("min", 0.0)) as i32),
        "bool" => format!("Param::bool({}, {}, {})", rust_str(id), rust_str(label), p["default"].as_bool().unwrap_or(false)),
        "choice" => {
            let opts: Vec<String> = p["options"].as_array().ok_or("a choice needs options")?.iter().filter_map(Value::as_str).map(rust_str).collect();
            format!("Param::choice({}, {}, &[{}], {})", rust_str(id), rust_str(label), opts.join(", "), f("default", 0.0) as u32)
        }
        "colour" | "color" => format!("Param::colour({}, {}, [{}])", rust_str(id), rust_str(label), arr(4, 1.0)),
        "point" => format!("Param::point({}, {}, [{}])", rust_str(id), rust_str(label), arr(2, 0.5)),
        other => return Err(format!("unknown parameter kind `{other}`")),
    })
}

/// The `lib.rs` for a one-effect plugin made from a pixel function.
pub fn pixel_source(spec: &Spec, body: &str) -> Result<String, String> {
    let params = spec.params.iter().map(param_code).collect::<Result<Vec<_>, _>>()?;
    let mut info = format!("EffectInfo::new(\"effect\", {}).description({})", rust_str(&spec.name), rust_str(&spec.description));
    for p in &params {
        info.push_str(&format!("\n            .param({p})"));
    }
    if spec.animated {
        info.push_str("\n            .animated()");
    }
    if spec.wgsl.is_some() {
        info.push_str("\n            .wgsl(include_str!(\"effect.wgsl\"))");
    }
    Ok(format!(
        r#"// Made in Zornov. SPDX-License-Identifier: Apache-2.0
#![allow(unused_variables, unused_mut, clippy::all)]
use studio_plugin_api::*;

#[inline]
fn pixel(px: Px, x: f32, y: f32, w: f32, h: f32, ctx: &Context, p: &Params) -> Px {{
{body}
}}

struct Fx;

impl Effect for Fx {{
    fn info(&self) -> EffectInfo {{
        {info}
    }}

    fn apply(&self, ctx: &Context, frame: &mut Frame, p: &Params) {{
        let (w, h) = (frame.width as f32, frame.height as f32);
        let width = frame.width;
        for (i, px) in frame.pixels.iter_mut().enumerate() {{
            let (x, y) = ((i % width) as f32 + 0.5, (i / width) as f32 + 0.5);
            *px = pixel(*px, x, y, w, h, ctx, p);
        }}
    }}
}}

fn plugin() -> Plugin {{
    Plugin::new({id}, {name}, "1.0.0").author("Made in Zornov").description({desc}).license("Apache-2.0").effect(Fx)
}}

export_plugin!(plugin);
"#,
        id = rust_str(&spec.id),
        name = rust_str(&spec.name),
        desc = rust_str(&spec.description),
    ))
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("could not make {}: {e}", d.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("could not write {}: {e}", path.display()))
}

/// Writes Studio's copy of the plugin API next to the sources.
fn vendor_api(root: &Path) -> Result<PathBuf, String> {
    let dir = root.join(format!(".api/studio-plugin-api-{API_VERSION}"));
    write(&dir.join("Cargo.toml"), &format!("[package]\nname = \"studio-plugin-api\"\nversion = \"{API_VERSION}\"\nedition = \"2024\"\nlicense = \"Apache-2.0\"\n\n[workspace]\n"))?;
    write(&dir.join("src/lib.rs"), API_LIB)?;
    write(&dir.join("src/json.rs"), API_JSON)?;
    Ok(dir)
}

/// The result of [`build`].
#[derive(Clone, Debug)]
pub struct Built {
    pub wasm: Vec<u8>,
    /// The crate folder, kept for the person.
    pub dir: PathBuf,
    /// What the test run did, one line per item.
    pub tested: Vec<String>,
}

/// Writes, builds and tests a plugin. Compiler errors come back as the
/// error, trimmed to the useful lines, for the AI to fix and try again.
pub fn build(spec: &Spec, root: &Path) -> Result<Built, String> {
    if spec.id.is_empty() || !spec.id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_') || !spec.id.as_bytes()[0].is_ascii_lowercase() {
        return Err("give the plugin an id of lower case letters, digits and -".into());
    }
    let lib = match (&spec.source, &spec.pixel) {
        (Some(src), _) => src.clone(),
        (None, Some(body)) => pixel_source(spec, body)?,
        (None, None) => return Err("give either `pixel` (the body of a per-pixel function) or `source` (a whole lib.rs)".into()),
    };
    let cargo = toolchain()?;
    let api = vendor_api(root)?;
    let dir = root.join(&spec.id);
    let krate = spec.id.replace('-', "_");
    write(
        &dir.join("Cargo.toml"),
        &format!(
            "[package]\nname = \"{krate}\"\nversion = \"1.0.0\"\nedition = \"2024\"\nlicense = \"Apache-2.0\"\npublish = false\n\n[workspace]\n\n[lib]\ncrate-type = [\"cdylib\"]\n\n[dependencies]\nstudio-plugin-api = {{ path = {:?} }}\n\n[profile.release]\nopt-level = \"s\"\nlto = true\npanic = \"abort\"\nstrip = true\n",
            api.display().to_string()
        ),
    )?;
    write(&dir.join("src/lib.rs"), &lib)?;
    if let Some(w) = &spec.wgsl {
        crate::bridge::full_shader(w, false)?;
        write(&dir.join("src/effect.wgsl"), w)?;
    }
    let target = root.join(".build");
    let mut child = Command::new(&cargo)
        .args(["build", "--release", "--offline", "--quiet", "--target", "wasm32-unknown-unknown", "--manifest-path"])
        .arg(dir.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", &target)
        .env_remove("RUSTFLAGS")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run cargo: {e}"))?;
    // Read stderr on a thread so a chatty build cannot block.
    let mut err = child.stderr.take().expect("piped");
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut err, &mut s);
        s
    });
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            break s;
        }
        if start.elapsed() > Duration::from_secs(600) {
            let _ = child.kill();
            return Err("the build took over 10 minutes and was stopped".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let stderr = reader.join().unwrap_or_default();
    if !status.success() {
        return Err(format!("the plugin did not compile:\n{}", trim_errors(&stderr)));
    }
    let wasm_path = target.join(format!("wasm32-unknown-unknown/release/{krate}.wasm"));
    let wasm = std::fs::read(&wasm_path).map_err(|e| format!("the build made no module ({}): {e}", wasm_path.display()))?;
    let tested = test_run(&wasm)?;
    Ok(Built { wasm, dir, tested })
}

/// Keeps compiler errors and their locations, drops the noise.
fn trim_errors(stderr: &str) -> String {
    let mut out = Vec::new();
    let mut keep = false;
    for line in stderr.lines() {
        if line.starts_with("error") {
            keep = !line.starts_with("error: could not compile");
        } else if line.starts_with("warning") {
            keep = false;
        }
        if keep {
            out.push(line);
        }
        if out.len() > 80 {
            break;
        }
    }
    if out.is_empty() { stderr.lines().take(40).collect::<Vec<_>>().join("\n") } else { out.join("\n") }
}

/// Runs every effect, generator and transition on a test picture with
/// default parameters; fails if one crashes or gives non-finite values.
pub fn test_run(wasm: &[u8]) -> Result<Vec<String>, String> {
    let (m, rt) = inspect(wasm, Default::default())?;
    let (w, h) = (64u32, 36u32);
    let card: Vec<[f32; 4]> = (0..w * h).map(|i| {
        let (x, y) = ((i % w) as f32 / w as f32, (i / w) as f32 / h as f32);
        [x, y, 1.0 - x, 1.0]
    }).collect();
    let mut lines = Vec::new();
    let check = |px: &[[f32; 4]], what: &str| -> Result<(), String> {
        if px.iter().flatten().any(|v| !v.is_finite()) { Err(format!("{what} gave non-finite values on the test picture")) } else { Ok(()) }
    };
    for (export, items) in [("studio_effect", &m.effects), ("studio_generate", &m.generators)] {
        for (i, it) in items.iter().enumerate().filter(|(_, it)| it.cpu) {
            let defs: Vec<_> = it.params.iter().map(packed_default).collect();
            let mut px = card.clone();
            rt.frame_call(export, i as u32, [0.5f32.to_bits(), 30f32.to_bits(), 7, 0], &mut px, w, h, &defs)?;
            check(&px, &it.name)?;
            let changed = px.iter().zip(&card).filter(|(a, b)| a != b).count();
            lines.push(format!("{}: ran on a {w}x{h} test picture, {}% of pixels changed", it.name, changed * 100 / card.len()));
        }
    }
    for (i, it) in m.transitions.iter().enumerate().filter(|(_, it)| it.cpu) {
        let defs: Vec<_> = it.params.iter().map(packed_default).collect();
        let mut a = card.clone();
        let b: Vec<[f32; 4]> = card.iter().map(|p| [p[2], p[0], p[1], 1.0]).collect();
        rt.transition_call(i as u32, [0, 0, 7, 0.5f32.to_bits()], &mut a, &b, w, h, &defs)?;
        check(&a, &it.name)?;
        lines.push(format!("{}: ran half-way on a test picture", it.name));
    }
    Ok(lines)
}

fn packed_default(p: &crate::manifest::Param) -> [f32; 4] {
    let n = |or: f64| p.default.as_f64().or(p.default.as_bool().map(|b| b as u8 as f64)).unwrap_or(or) as f32;
    let a = |i: usize, or: f64| p.default.get(i).and_then(Value::as_f64).unwrap_or(or) as f32;
    match p.kind.as_str() {
        "colour" => {
            let al = a(3, 1.0);
            [a(0, 1.0) * al, a(1, 1.0) * al, a(2, 1.0) * al, al]
        }
        "point" => [a(0, 0.5), a(1, 0.5), 0.0, 0.0],
        _ => [n(p.min.unwrap_or(0.0)), 0.0, 0.0, 0.0],
    }
}

/// Builds, tests and installs.
pub fn create(host: &Host, spec: &Spec) -> Result<(Installed, Built), String> {
    let built = build(spec, &sources_dir())?;
    let p = host.install_bytes(&built.wasm, "made in Zornov")?;
    Ok((p, built))
}
