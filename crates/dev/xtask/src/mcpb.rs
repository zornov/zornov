// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The AI app installs: `cargo xtask package-mcpb` makes
//! `dist/release/Zornov-<ver>.mcpb`, the Claude Desktop extension (an MCP
//! Bundle: a zip with `manifest.json` at its top). It is thin: it starts the
//! Zornov installed on the machine (`zornov mcp --project DIR`, through
//! `zornov-cli` on Windows) and carries only its manifest and icon. The
//! manifest is `packaging/mcpb/manifest.json` with the workspace version
//! written in.
//!
//! The Claude Code plugin needs no build: `.claude-plugin/marketplace.json`
//! lists the plugin in `packaging/claude-code`, read from the repository.

use crate::release::{VERSION, release_dir};
use crate::repro::{self, Made};
use serde_json::Value;
use std::path::Path;

const MANIFEST: &str = "packaging/mcpb/manifest.json";
const ICON: &str = "packaging/brand/hicolor/512x512/apps/zornov.png";

fn json(root: &Path, rel: &str) -> Result<Value, String> {
    let text = std::fs::read_to_string(root.join(rel)).map_err(|e| format!("{rel}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("{rel}: {e}"))
}

/// The bundle's `manifest.json`: the one in `packaging/mcpb` with `version`
/// written over its placeholder (as text, so the fields keep their order).
fn manifest(root: &Path, version: &str) -> Result<Vec<u8>, String> {
    const PLACEHOLDER: &str = "\"version\": \"0.0.0\"";
    json(root, MANIFEST)?;
    let text = std::fs::read_to_string(root.join(MANIFEST)).map_err(|e| format!("{MANIFEST}: {e}"))?;
    if text.matches(PLACEHOLDER).count() != 1 {
        return Err(format!("{MANIFEST} needs {PLACEHOLDER} once, for the version"));
    }
    Ok(text.replace(PLACEHOLDER, &format!("\"version\": {}", Value::String(version.into()))).into_bytes())
}

/// Writes the bundle for `version` to `out`.
fn write_bundle(root: &Path, version: &str, out: &Path) -> Result<(), String> {
    let icon = std::fs::read(root.join(ICON)).map_err(|e| format!("{ICON}: {e}"))?;
    repro::zip(out, "", &[], &[Made::file("manifest.json", manifest(root, version)?, false), Made::file("icon.png", icon, false)])
}

/// `cargo xtask package-mcpb`
pub fn package_mcpb() -> Result<String, String> {
    let dir = release_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let out = dir.join(format!("Zornov-{VERSION}.mcpb"));
    write_bundle(&crate::root(), VERSION, &out)?;
    Ok(out.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn root() -> std::path::PathBuf {
        crate::root()
    }

    /// The `[[bin]]` names in `app/Cargo.toml`.
    fn app_bins() -> Vec<String> {
        let manifest = std::fs::read_to_string(root().join("app/Cargo.toml")).unwrap();
        manifest.split("[[bin]]").skip(1).filter_map(|b| b.lines().find_map(|l| l.trim().strip_prefix("name = ")).map(|n| n.trim().trim_matches('"').to_owned())).collect()
    }

    fn strings(v: &Value) -> Vec<&str> {
        v.as_array().unwrap().iter().map(|a| a.as_str().unwrap()).collect()
    }

    #[test]
    fn the_bundle_manifest_starts_the_installed_zornov_on_the_chosen_project() {
        let m = json(&root(), MANIFEST).unwrap();
        // The manifest version the MCPB tools write by default; Claude
        // Desktop reads it.
        assert_eq!(m["manifest_version"], "0.3");
        for field in ["name", "version", "description", "author", "server"] {
            assert!(!m[field].is_null(), "manifest.json has no {field}");
        }
        assert_eq!(m["name"], "zornov");
        let server = &m["server"];
        assert_eq!(server["type"], "binary");
        assert!(server["entry_point"].is_string());
        let cfg = &server["mcp_config"];
        let bins = app_bins();
        assert_eq!(cfg["command"], "zornov");
        assert!(bins.iter().any(|b| b == "zornov"));
        assert_eq!(strings(&cfg["args"]), ["mcp", "--project", "${user_config.project}"]);
        let darwin = &cfg["platform_overrides"]["darwin"];
        assert_eq!(darwin["command"], "/Applications/Zornov.app/Contents/MacOS/zornov");
        // On Windows zornov.exe is a window program: the console program
        // beside it starts it with the pipes.
        let win = cfg["platform_overrides"]["win32"]["command"].as_str().unwrap();
        let exe = win.rsplit('\\').next().unwrap();
        assert_eq!(exe, "zornov-cli");
        assert!(bins.iter().any(|b| b == exe));
        assert!(win.starts_with("C:\\Program Files\\Zornov\\"), "{win}: the MSI's install folder");
        // Overrides change the program only; the arguments are the same.
        for o in cfg["platform_overrides"].as_object().unwrap().values() {
            assert!(o.get("args").is_none());
        }
        let project = &m["user_config"]["project"];
        assert_eq!(project["type"], "directory");
        assert_eq!(project["required"], true);
        assert!(project["title"].is_string() && project["description"].is_string());
        assert_eq!(m["icon"], "icon.png");
        assert!(root().join(ICON).is_file());
    }

    #[test]
    fn the_claude_code_plugin_runs_zornov_mcp() {
        let market = json(&root(), ".claude-plugin/marketplace.json").unwrap();
        assert!(market["owner"]["name"].is_string());
        let entries = market["plugins"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry["name"], "zornov");
        let source = entry["source"].as_str().unwrap();
        assert!(source.starts_with("./") && !source.contains(".."), "{source}");
        let plugin = json(&root(), &format!("{}/.claude-plugin/plugin.json", &source[2..])).unwrap();
        // The install id is the entry's name; skills and tools take the
        // manifest's: keep them the same.
        assert_eq!(plugin["name"], entry["name"]);
        let servers = plugin["mcpServers"].as_object().unwrap();
        assert_eq!(servers.len(), 1);
        let server = &servers["zornov"];
        assert_eq!(server["command"], "zornov");
        assert_eq!(strings(&server["args"]), ["mcp"]);
    }

    #[test]
    fn the_bundle_is_a_zip_with_the_manifest_at_its_top() {
        let dir = std::env::temp_dir().join(format!("xtask-mcpb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.mcpb"), dir.join("b.mcpb"));
        write_bundle(&root(), "9.8.7", &a).unwrap();
        write_bundle(&root(), "9.8.7", &b).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap(), "the same sources give the same bundle");
        let mut z = zip::ZipArchive::new(std::fs::File::open(&a).unwrap()).unwrap();
        let names: Vec<String> = z.file_names().map(str::to_owned).collect();
        assert_eq!(names, ["icon.png", "manifest.json"]);
        let mut text = String::new();
        z.by_name("manifest.json").unwrap().read_to_string(&mut text).unwrap();
        let m: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(m["version"], "9.8.7");
        let mut original = json(&root(), MANIFEST).unwrap();
        original["version"] = m["version"].clone();
        assert_eq!(m, original, "only the version is changed");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
