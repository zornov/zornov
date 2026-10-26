// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The `plugins` tool: find, install, use and write plugins.
//!
//! Plugin effects, generators and transitions need nothing special once
//! installed: they are in the catalogue under their names (and as
//! `<plugin>:<item>` ids) and go on clips with `apply`, like built-in ones.
//! This tool covers the rest: what is installed and available, installing
//! (in the window Studio asks the person first), turning plugins on and
//! off, importers, exporters, panels, OpenFX, and building a new effect
//! plugin when nothing fits.

use std::path::PathBuf;

use serde_json::{Value, json};

use super::{Content, Session};
use crate::plugins::{self, ImportDone, catalogue, host};

fn s<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k).and_then(Value::as_str)
}

fn need<'a>(a: &'a Value, k: &str, action: &str) -> Result<&'a str, String> {
    s(a, k).ok_or_else(|| format!("plugins {action} needs \"{k}\""))
}

/// How to use what a plugin provides, for the agent.
fn usage(p: &plugins::Installed) -> Vec<String> {
    let m = &p.manifest;
    let mut v = Vec::new();
    for it in &m.effects {
        v.push(format!("effect \"{}\": edit op {{\"op\":\"apply\",\"item\":\"{}:{}\",\"clips\":[...]}}", it.name, m.id, it.id));
    }
    for it in &m.generators {
        v.push(format!("generator \"{}\": apply \"{}:{}\" to an empty card or a clip (it draws over it)", it.name, m.id, it.id));
    }
    for it in &m.transitions {
        v.push(format!("transition \"{}\": edit op {{\"op\":\"transition\",\"clip\":ID,\"item\":\"{}:{}\"}}", it.name, m.id, it.id));
    }
    for t in &m.importers {
        v.push(format!("importer \"{}\" for .{}: import the file as usual, or plugins {{\"action\":\"import\",\"path\":...}}", t.name, t.extensions.join(", .")));
    }
    for t in &m.exporters {
        v.push(format!("exporter \"{}\" (.{}): plugins {{\"action\":\"export\",\"plugin\":\"{}\",\"exporter\":\"{}\"}}", t.name, t.extensions.join(""), m.id, t.id));
    }
    for t in &m.panels {
        v.push(format!("panel \"{}\": plugins {{\"action\":\"panel\",\"plugin\":\"{}\",\"panel\":\"{}\"}}", t.name, m.id, t.id));
    }
    v
}

/// Runs one agent tool on a window's document (plugin panels, macros):
/// the document goes into a session for the call and comes back.
pub fn run_on(doc: &mut crate::doc::Doc, tool: &str, args: &Value) -> Result<Value, String> {
    let placeholder = crate::services::project::new_custom("", 16, 16, 30.0);
    let mut s = Session::new(std::mem::replace(doc, placeholder));
    let r = s.call(tool, args);
    *doc = s.doc;
    let parts = r?;
    Ok(parts.iter().find_map(|c| if let Content::Text(t) = c { Some(serde_json::from_str(t).unwrap_or_else(|_| Value::String(t.clone()))) } else { None }).unwrap_or(Value::Null))
}

impl Session {
    pub(super) fn plugins_tool(&mut self, a: &Value) -> Result<Vec<Content>, String> {
        let text = |v: Value| Ok(vec![Content::Text(v.to_string())]);
        let action = s(a, "action").unwrap_or("list");
        match action {
            "list" => text(plugins::overview()),
            "search" => {
                let q = s(a, "query").unwrap_or("");
                let all = catalogue::available(host());
                let installed = host().list();
                let found: Vec<Value> = catalogue::search(&all, q).into_iter().map(|e| e.json(installed.iter().find(|p| p.id() == e.id))).collect();
                let hint = if found.is_empty() {
                    "Nothing in the catalogue fits. Built-in effects may (catalogue panel effects); or write one with plugins {\"action\":\"create\"}."
                } else {
                    "Install one with plugins {\"action\":\"install\",\"id\":...} once the person agrees."
                };
                text(json!({"found": found, "hint": hint}))
            }
            "install" => {
                let p = match (s(a, "id"), s(a, "path")) {
                    (_, Some(path)) => host().install_file(&PathBuf::from(path))?,
                    (Some(id), None) => catalogue::install(host(), id)?,
                    (None, None) => return Err("plugins install needs \"id\" (from search) or \"path\" (a .wasm file)".into()),
                };
                text(json!({"installed": p.json(), "use": usage(&p)}))
            }
            "enable" | "disable" => {
                let id = need(a, "id", action)?;
                if id.starts_with("ofx:") {
                    return self.ofx_item(id, action == "enable");
                }
                let p = host().set_enabled(id, action == "enable")?;
                text(json!({"plugin": p.json()}))
            }
            "uninstall" => {
                let id = need(a, "id", action)?;
                host().uninstall(id)?;
                text(json!({"uninstalled": id, "note": "clips that used its effects keep their settings and skip them until it is installed again"}))
            }
            "create" => self.plugin_create(a),
            "import" => self.plugin_import(a),
            "export" => {
                let plugin = need(a, "plugin", action)?;
                let exporter = need(a, "exporter", action)?;
                let p = host().get(plugin).ok_or_else(|| format!("no plugin {plugin}"))?;
                let ext = p.manifest.exporters.iter().find(|t| t.id == exporter).and_then(|t| t.extensions.first().cloned()).unwrap_or_else(|| "dat".into());
                let bytes = host().export(plugin, exporter, &plugins::project_json(&self.doc.project))?;
                let path = s(a, "path").map(PathBuf::from).unwrap_or_else(|| plugins::export_path(&self.doc.project.name, exporter, &ext));
                if let Some(d) = path.parent() {
                    std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
                }
                std::fs::write(&path, &bytes).map_err(|e| format!("could not write {}: {e}", path.display()))?;
                text(json!({"exported": path.display().to_string(), "bytes": bytes.len()}))
            }
            "panel" => {
                let plugin = need(a, "plugin", action)?;
                let panel = need(a, "panel", action)?;
                let event = json!({"button": a.get("button"), "values": a.get("values").cloned().unwrap_or(json!({})), "selection": a.get("selection").cloned().unwrap_or(json!([]))});
                let mut view = host().panel(plugin, panel, &event, &plugins::project_json(&self.doc.project))?;
                if let Some(ops) = view.get("ops").filter(|o| o.as_array().is_some_and(|a| !a.is_empty())).cloned() {
                    let r = self.call("edit", &json!({"ops": ops, "label": format!("{plugin}: {panel}")}))?;
                    view["applied"] = r.iter().find_map(|c| if let Content::Text(t) = c { serde_json::from_str(t).ok() } else { None }).unwrap_or(Value::Null);
                }
                text(view)
            }
            "ofx" => match a.get("on").and_then(Value::as_bool) {
                Some(on) => text(plugins::set_ofx(on)?),
                None => text(plugins::ofx_json()),
            },
            "add_index" => {
                let url = need(a, "url", action)?;
                let n = catalogue::add_index(host(), url)?;
                text(json!({"indexes": "updated", "plugins_listed": n}))
            }
            "refresh" => text(json!({"plugins_listed": catalogue::refresh(host())?})),
            other => Err(format!("plugins action \"{other}\": list, search, install, enable, disable, uninstall, create, import, export, panel, ofx, add_index, refresh")),
        }
    }

    fn ofx_item(&mut self, id: &str, on: bool) -> Result<Vec<Content>, String> {
        let dir = host().dir().to_owned();
        let mut st = plugins::ofx::Settings::load(&dir);
        let raw = id.trim_start_matches("ofx:").to_owned();
        st.off.retain(|x| *x != raw);
        if !on {
            st.off.push(raw);
        }
        st.save(&dir)?;
        plugins::ofx::load(&dir);
        Ok(vec![Content::Text(plugins::ofx_json().to_string())])
    }

    fn plugin_create(&mut self, a: &Value) -> Result<Vec<Content>, String> {
        let id = need(a, "id", "create")?.to_owned();
        let spec = studio_plugins::author::Spec {
            name: s(a, "name").unwrap_or(&id).to_owned(),
            description: s(a, "description").unwrap_or("").to_owned(),
            source: s(a, "source").map(str::to_owned),
            pixel: s(a, "pixel").map(str::to_owned),
            params: a.get("params").and_then(Value::as_array).cloned().unwrap_or_default(),
            wgsl: s(a, "wgsl").map(str::to_owned),
            animated: a.get("animated").and_then(Value::as_bool).unwrap_or(false),
            id,
        };
        let (p, built) = studio_plugins::author::create(host(), &spec)?;
        Ok(vec![Content::Text(json!({"installed": p.json(), "tested": built.tested, "source": built.dir.join("src/lib.rs").display().to_string(), "use": usage(&p)}).to_string())])
    }

    /// For `import`: files only a plugin reads are imported by it; the rest
    /// go through as usual. `None` when no plugin is needed.
    pub(super) fn import_by_plugins(&mut self, paths: &[PathBuf], a: &Value) -> Result<Option<Vec<Content>>, String> {
        let (theirs, ours): (Vec<&PathBuf>, Vec<&PathBuf>) = paths.iter().partition(|p| plugins::takes(p));
        if theirs.is_empty() {
            return Ok(None);
        }
        let mut out = Vec::new();
        for p in theirs {
            let mut b = a.clone();
            b["path"] = json!(p.display().to_string());
            out.extend(self.plugin_import(&b)?);
        }
        if !ours.is_empty() {
            let mut b = a.clone();
            b["paths"] = json!(ours.iter().map(|p| p.display().to_string()).collect::<Vec<_>>());
            out.extend(self.call("import", &b)?);
        }
        Ok(Some(out))
    }

    fn plugin_import(&mut self, a: &Value) -> Result<Vec<Content>, String> {
        let path = PathBuf::from(need(a, "path", "import")?);
        let importer = match (s(a, "plugin"), s(a, "importer")) {
            (Some(p), Some(i)) => Some((p, i)),
            _ => None,
        };
        match plugins::import(&mut self.doc, &path, importer)? {
            ImportDone::Media { id, file, plugin } => {
                if a.get("to_timeline").and_then(Value::as_bool).unwrap_or(false) {
                    let r = self.call("edit", &json!({"ops": [{"op": "add", "item": file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), "panel": "media", "at": a.get("at").cloned().unwrap_or(json!(0))}], "label": "Import"}))?;
                    return Ok(r);
                }
                Ok(vec![Content::Text(json!({"media": id, "file": file.display().to_string(), "read_by": plugin}).to_string())])
            }
            ImportDone::Ops(ops) => self.call("edit", &json!({"ops": ops, "label": format!("Import {}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())})),
        }
    }
}
