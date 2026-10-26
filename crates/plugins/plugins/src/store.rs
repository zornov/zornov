// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Installed plugins: `<Studio>/Plugins/<id>/plugin.wasm`, with which are
//! turned off in `<Studio>/Plugins/state.json`. Installing copies the file
//! in after checking it loads; nothing outside the folder is touched.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::bridge;
use crate::manifest::{Kind, Manifest};
use crate::runtime::{Limits, Runtime, split_blob};

/// One installed plugin.
#[derive(Clone, Debug)]
pub struct Installed {
    pub manifest: Manifest,
    pub path: PathBuf,
    pub enabled: bool,
    /// SHA-256 of the module, hex.
    pub sha256: String,
    /// Where it came from: a file path, a catalogue URL, "bundled", "made by the AI".
    pub source: String,
    /// Shaders that fell back to the CPU, ids that clashed.
    pub warnings: Vec<String>,
    /// Why it is not running, when it should be.
    pub error: Option<String>,
}

impl Installed {
    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    /// For the agent and the panel.
    pub fn json(&self) -> Value {
        let m = &self.manifest;
        let items: Vec<Value> = m
            .items()
            .into_iter()
            .map(|(k, id, name)| {
                let mut v = json!({"kind": k.label(), "name": name});
                if matches!(k, Kind::Effect | Kind::Generator | Kind::Transition) {
                    v["id"] = json!(crate::manifest::qualified(&m.id, id));
                } else {
                    v["id"] = json!(id);
                }
                v
            })
            .collect();
        let mut v = json!({"id": m.id, "name": m.name, "version": m.version, "author": m.author, "description": m.description, "license": m.license, "enabled": self.enabled, "provides": m.summary(), "items": items, "source": self.source});
        if !self.warnings.is_empty() {
            v["warnings"] = json!(self.warnings);
        }
        if let Some(e) = &self.error {
            v["error"] = json!(e);
        }
        if let Some(rt) = bridge::runtime(&m.id) {
            let log = rt.log.lock().unwrap_or_else(|e| e.into_inner());
            if !log.is_empty() {
                v["log"] = json!(log.iter().rev().take(5).rev().collect::<Vec<_>>());
            }
        }
        v
    }
}

#[derive(Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    disabled: Vec<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct Info {
    #[serde(default)]
    source: String,
}

/// What an importer made.
#[derive(Clone, Debug, PartialEq)]
pub enum Imported {
    /// Straight sRGB RGBA8.
    Image { width: u32, height: u32, rgba8: Vec<u8> },
    /// Agent edit ops.
    Ops(Value),
}

/// The plugins of one folder. [`host`] is the one in Studio's folder.
pub struct Host {
    dir: PathBuf,
    limits: Limits,
    plugins: Mutex<Option<Vec<Installed>>>,
}

/// SHA-256 as hex.
pub fn sha256(b: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, b).as_ref().iter().map(|x| format!("{x:02x}")).collect()
}

fn hash64(hex: &str) -> u64 {
    u64::from_str_radix(&hex[..16.min(hex.len())], 16).unwrap_or(0)
}

/// Compiles, starts and reads a module; nothing is registered.
pub fn inspect(wasm: &[u8], limits: Limits) -> Result<(Manifest, Arc<Runtime>), String> {
    let rt = Runtime::compile(wasm, limits)?;
    let m = rt.manifest()?;
    Ok((m, Arc::new(rt)))
}

impl Host {
    pub fn at(dir: impl Into<PathBuf>) -> Host {
        Host { dir: dir.into(), limits: Limits::default(), plugins: Mutex::new(None) }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn state_path(&self) -> PathBuf {
        self.dir.join("state.json")
    }

    fn state(&self) -> State {
        std::fs::read(self.state_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn save_state(&self, s: &State) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        std::fs::write(self.state_path(), serde_json::to_vec_pretty(s).unwrap_or_default()).map_err(|e| format!("could not save plugin settings: {e}"))
    }

    fn lock(&self) -> MutexGuard<'_, Option<Vec<Installed>>> {
        let mut g = self.plugins.lock().unwrap_or_else(|e| e.into_inner());
        if g.is_none() {
            *g = Some(self.scan());
        }
        g
    }

    /// Reads the folder and starts every enabled plugin.
    fn scan(&self) -> Vec<Installed> {
        let state = self.state();
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return out };
        let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.join("plugin.wasm").is_file()).collect();
        dirs.sort();
        for d in dirs {
            let path = d.join("plugin.wasm");
            let info: Info = std::fs::read(d.join("info.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
            let Ok(bytes) = std::fs::read(&path) else { continue };
            let sha = sha256(&bytes);
            match inspect(&bytes, self.limits) {
                Ok((m, rt)) => {
                    let enabled = !state.disabled.contains(&m.id);
                    let warnings = if enabled { bridge::register(&m, hash64(&sha), rt) } else { Vec::new() };
                    out.push(Installed { manifest: m, path, enabled, sha256: sha, source: info.source, warnings, error: None });
                }
                Err(e) => {
                    // Keep it listed so it can be removed.
                    let id = d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let m = Manifest { abi: 0, api: String::new(), id: id.clone(), name: id, version: "?".into(), author: String::new(), description: String::new(), license: String::new(), homepage: String::new(), effects: vec![], generators: vec![], transitions: vec![], importers: vec![], exporters: vec![], panels: vec![] };
                    out.push(Installed { manifest: m, path, enabled: false, sha256: sha, source: info.source, warnings: vec![], error: Some(e) });
                }
            }
        }
        out
    }

    /// Every installed plugin (starting the enabled ones on first use).
    pub fn list(&self) -> Vec<Installed> {
        self.lock().clone().unwrap_or_default()
    }

    pub fn get(&self, id: &str) -> Option<Installed> {
        self.list().into_iter().find(|p| p.manifest.id == id)
    }

    /// Re-reads the folder, e.g. after files were copied in by hand.
    pub fn reload(&self) -> Vec<Installed> {
        let mut g = self.plugins.lock().unwrap_or_else(|e| e.into_inner());
        for p in g.as_deref().unwrap_or_default() {
            bridge::unregister(&p.manifest);
        }
        *g = Some(self.scan());
        g.clone().unwrap_or_default()
    }

    /// Installs (or upgrades) a plugin from its module bytes; it is on
    /// straight away.
    pub fn install_bytes(&self, wasm: &[u8], source: &str) -> Result<Installed, String> {
        let (m, rt) = inspect(wasm, self.limits)?;
        let sha = sha256(wasm);
        let mut g = self.lock();
        let list = g.as_mut().expect("scanned");
        if let Some(old) = list.iter().position(|p| p.manifest.id == m.id) {
            bridge::unregister(&list[old].manifest);
            list.remove(old);
        }
        let dir = self.dir.join(&m.id);
        std::fs::create_dir_all(&dir).map_err(|e| format!("could not make {}: {e}", dir.display()))?;
        let path = dir.join("plugin.wasm");
        let tmp = dir.join("plugin.wasm.part");
        std::fs::write(&tmp, wasm).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|e| format!("could not save the plugin: {e}"))?;
        let _ = std::fs::write(dir.join("info.json"), serde_json::to_vec_pretty(&Info { source: source.to_owned() }).unwrap_or_default());
        let mut st = self.state();
        st.disabled.retain(|d| *d != m.id);
        self.save_state(&st)?;
        let warnings = bridge::register(&m, hash64(&sha), rt);
        let p = Installed { manifest: m, path, enabled: true, sha256: sha, source: source.to_owned(), warnings, error: None };
        list.push(p.clone());
        list.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
        Ok(p)
    }

    /// Installs a `.wasm` file.
    pub fn install_file(&self, path: &Path) -> Result<Installed, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
        self.install_bytes(&bytes, &path.display().to_string())
    }

    pub fn set_enabled(&self, id: &str, on: bool) -> Result<Installed, String> {
        let mut g = self.lock();
        let list = g.as_mut().expect("scanned");
        let p = list.iter_mut().find(|p| p.manifest.id == id).ok_or_else(|| format!("no plugin `{id}` is installed"))?;
        if let Some(e) = &p.error {
            return Err(format!("{id} cannot run: {e}"));
        }
        if on && !p.enabled {
            let bytes = std::fs::read(&p.path).map_err(|e| e.to_string())?;
            let (m, rt) = inspect(&bytes, self.limits)?;
            p.warnings = bridge::register(&m, hash64(&p.sha256), rt);
            p.manifest = m;
        } else if !on && p.enabled {
            bridge::unregister(&p.manifest);
        }
        p.enabled = on;
        let out = p.clone();
        let mut st = self.state();
        st.disabled.retain(|d| d != id);
        if !on {
            st.disabled.push(id.to_owned());
        }
        self.save_state(&st)?;
        Ok(out)
    }

    /// Removes a plugin and its folder. Clips that use its effects keep
    /// their settings and skip them, as with any unknown effect.
    pub fn uninstall(&self, id: &str) -> Result<(), String> {
        let mut g = self.lock();
        let list = g.as_mut().expect("scanned");
        let i = list.iter().position(|p| p.manifest.id == id).ok_or_else(|| format!("no plugin `{id}` is installed"))?;
        let p = list.remove(i);
        bridge::unregister(&p.manifest);
        if let Some(d) = p.path.parent().filter(|d| d.starts_with(&self.dir) && *d != self.dir) {
            std::fs::remove_dir_all(d).map_err(|e| format!("could not remove {}: {e}", d.display()))?;
        }
        let mut st = self.state();
        st.disabled.retain(|d| d != id);
        self.save_state(&st)
    }

    fn running(&self, id: &str) -> Result<(Installed, Arc<Runtime>), String> {
        let p = self.get(id).ok_or_else(|| format!("no plugin `{id}` is installed"))?;
        if !p.enabled {
            return Err(format!("the plugin `{id}` is turned off"));
        }
        let rt = bridge::runtime(id).ok_or_else(|| format!("the plugin `{id}` is not running"))?;
        Ok((p, rt))
    }

    /// Enabled importers that read `ext`: (plugin, importer id, name).
    pub fn importers_for(&self, ext: &str) -> Vec<(String, String, String)> {
        let ext = ext.trim_start_matches('.').to_ascii_lowercase();
        self.list().iter().filter(|p| p.enabled).flat_map(|p| p.manifest.importers.iter().filter(|t| t.extensions.contains(&ext)).map(|t| (p.manifest.id.clone(), t.id.clone(), t.name.clone())).collect::<Vec<_>>()).collect()
    }

    /// Runs an importer on a file. With `importer` None, the first enabled
    /// one for the file's extension.
    pub fn import(&self, file: &Path, importer: Option<(&str, &str)>) -> Result<Imported, String> {
        let ext = file.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let (pid, iid) = match importer {
            Some((p, i)) => (p.to_owned(), i.to_owned()),
            None => self.importers_for(&ext).into_iter().next().map(|(p, i, _)| (p, i)).ok_or_else(|| format!("no plugin imports .{ext} files"))?,
        };
        let (p, rt) = self.running(&pid)?;
        let index = p.manifest.importers.iter().position(|t| t.id == iid).ok_or_else(|| format!("{pid} has no importer `{iid}`"))?;
        let bytes = std::fs::read(file).map_err(|e| format!("could not read {}: {e}", file.display()))?;
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let out = rt.bytes_call("studio_import", index as u32, &[name.as_bytes(), &bytes])?;
        let (head, payload) = split_blob(&out)?;
        match head["kind"].as_str() {
            Some("image") => {
                let (w, h) = (head["width"].as_u64().unwrap_or(0) as u32, head["height"].as_u64().unwrap_or(0) as u32);
                if w == 0 || h == 0 || payload.len() != w as usize * h as usize * 4 {
                    return Err(format!("{} returned a picture of the wrong size", p.manifest.name));
                }
                Ok(Imported::Image { width: w, height: h, rgba8: payload.to_vec() })
            }
            Some("ops") if head["ops"].is_array() => Ok(Imported::Ops(head["ops"].clone())),
            Some("error") => Err(head["message"].as_str().unwrap_or("the importer failed").to_owned()),
            _ => Err(format!("{} returned something Studio does not understand", p.manifest.name)),
        }
    }

    /// Runs an exporter on the project (as the agent sees it).
    pub fn export(&self, plugin: &str, exporter: &str, project: &Value) -> Result<Vec<u8>, String> {
        let (p, rt) = self.running(plugin)?;
        let index = p.manifest.exporters.iter().position(|t| t.id == exporter).ok_or_else(|| format!("{plugin} has no exporter `{exporter}`"))?;
        let out = rt.bytes_call("studio_export", index as u32, &[project.to_string().as_bytes()])?;
        let (head, payload) = split_blob(&out)?;
        if head["ok"].as_bool() == Some(true) { Ok(payload.to_vec()) } else { Err(head["message"].as_str().unwrap_or("the exporter failed").to_owned()) }
    }

    /// Draws a panel, or presses one of its buttons. `event` is
    /// `{"button": id?, "values": {id: number}, "selection": [ids]}`.
    /// Returns `{"widgets": [...], "ops": [...]?, "message": "..."?}`.
    pub fn panel(&self, plugin: &str, panel: &str, event: &Value, project: &Value) -> Result<Value, String> {
        let (p, rt) = self.running(plugin)?;
        let index = p.manifest.panels.iter().position(|t| t.id == panel).ok_or_else(|| format!("{plugin} has no panel `{panel}`"))?;
        let call = json!({"event": event, "project": project}).to_string();
        let out = rt.bytes_call("studio_panel", index as u32, &[call.as_bytes()])?;
        let v: Value = serde_json::from_slice(&out).map_err(|e| format!("the panel returned bad JSON: {e}"))?;
        if let Some(e) = v["error"].as_str() {
            return Err(e.to_owned());
        }
        if !v["widgets"].is_array() {
            return Err("the panel returned no widgets".into());
        }
        Ok(v)
    }
}
