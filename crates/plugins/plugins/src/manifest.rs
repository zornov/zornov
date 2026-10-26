// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What a plugin says it provides (`studio_manifest`), checked before
//! anything of it is registered.

use serde::{Deserialize, Serialize};

/// ABIs this host can run.
pub const SUPPORTED_ABI: &[u32] = &[1];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub abi: u32,
    /// Version of `studio-plugin-api` the plugin was built with.
    #[serde(default)]
    pub api: String,
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub homepage: String,
    #[serde(default)]
    pub effects: Vec<Item>,
    #[serde(default)]
    pub generators: Vec<Item>,
    #[serde(default)]
    pub transitions: Vec<Item>,
    #[serde(default)]
    pub importers: Vec<Tool>,
    #[serde(default)]
    pub exporters: Vec<Tool>,
    #[serde(default)]
    pub panels: Vec<Tool>,
}

/// An effect, generator or transition.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub animated: bool,
    #[serde(default = "yes")]
    pub cpu: bool,
    #[serde(default)]
    pub wgsl: Option<String>,
    #[serde(default)]
    pub params: Vec<Param>,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Param {
    pub id: String,
    pub label: String,
    pub kind: String,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub default: serde_json::Value,
    #[serde(default = "yes")]
    pub animatable: bool,
}

/// An importer, exporter or panel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub extensions: Vec<String>,
}

/// The kinds of item a plugin can provide.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Effect,
    Generator,
    Transition,
    Importer,
    Exporter,
    Panel,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Effect => "effect",
            Kind::Generator => "generator",
            Kind::Transition => "transition",
            Kind::Importer => "importer",
            Kind::Exporter => "exporter",
            Kind::Panel => "panel",
        }
    }
}

fn good_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-') && s.as_bytes()[0].is_ascii_alphabetic()
}

/// Shader parameters fill `p: array<vec4<f32>, 16>`.
pub const MAX_PARAMS: usize = 16;

impl Manifest {
    pub fn parse(json: &str) -> Result<Manifest, String> {
        let m: Manifest = serde_json::from_str(json).map_err(|e| format!("the plugin's manifest is not valid: {e}"))?;
        m.validate()?;
        Ok(m)
    }

    /// Checks ids, kinds and limits. Parameter ranges are checked again by
    /// `fx` when items are registered.
    pub fn validate(&self) -> Result<(), String> {
        if !SUPPORTED_ABI.contains(&self.abi) {
            return Err(format!("the plugin is built for plugin ABI {} and this Studio runs ABI {}; update {}", self.abi, SUPPORTED_ABI.iter().map(u32::to_string).collect::<Vec<_>>().join(", "), if self.abi > 1 { "Studio" } else { "the plugin" }));
        }
        if !good_id(&self.id) {
            return Err(format!("plugin id `{}` must be lower case letters, digits, - or _", self.id));
        }
        if self.name.trim().is_empty() || self.version.trim().is_empty() {
            return Err("the plugin has no name or version".into());
        }
        for (kind, items) in [("effect", &self.effects), ("generator", &self.generators), ("transition", &self.transitions)] {
            let mut seen = std::collections::HashSet::new();
            for it in items {
                if !good_id(&it.id) || !seen.insert(&it.id) {
                    return Err(format!("{kind} id `{}` is malformed or repeated", it.id));
                }
                if it.params.len() > MAX_PARAMS {
                    return Err(format!("{kind} `{}` has {} parameters; the most is {MAX_PARAMS}", it.id, it.params.len()));
                }
                if !it.cpu && it.wgsl.is_none() {
                    return Err(format!("{kind} `{}` has neither CPU code nor a shader", it.id));
                }
                for p in &it.params {
                    if !matches!(p.kind.as_str(), "float" | "int" | "bool" | "choice" | "colour" | "point") {
                        return Err(format!("parameter `{}` of `{}` has unknown kind `{}`", p.id, it.id, p.kind));
                    }
                    if p.kind == "choice" && p.options.is_empty() {
                        return Err(format!("choice `{}` of `{}` has no options", p.id, it.id));
                    }
                }
            }
        }
        for (kind, tools) in [("importer", &self.importers), ("exporter", &self.exporters), ("panel", &self.panels)] {
            let mut seen = std::collections::HashSet::new();
            for t in tools {
                if !good_id(&t.id) || !seen.insert(&t.id) {
                    return Err(format!("{kind} id `{}` is malformed or repeated", t.id));
                }
            }
        }
        Ok(())
    }

    /// One line per item, for lists and the agent.
    pub fn items(&self) -> Vec<(Kind, &str, &str)> {
        let mut v = Vec::new();
        v.extend(self.effects.iter().map(|i| (Kind::Effect, i.id.as_str(), i.name.as_str())));
        v.extend(self.generators.iter().map(|i| (Kind::Generator, i.id.as_str(), i.name.as_str())));
        v.extend(self.transitions.iter().map(|i| (Kind::Transition, i.id.as_str(), i.name.as_str())));
        v.extend(self.importers.iter().map(|i| (Kind::Importer, i.id.as_str(), i.name.as_str())));
        v.extend(self.exporters.iter().map(|i| (Kind::Exporter, i.id.as_str(), i.name.as_str())));
        v.extend(self.panels.iter().map(|i| (Kind::Panel, i.id.as_str(), i.name.as_str())));
        v
    }

    /// "2 effects, 1 transition", for one-line descriptions.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        for (n, k) in [
            (self.effects.len(), "effect"),
            (self.generators.len(), "generator"),
            (self.transitions.len(), "transition"),
            (self.importers.len(), "importer"),
            (self.exporters.len(), "exporter"),
            (self.panels.len(), "panel"),
        ] {
            if n > 0 {
                parts.push(format!("{n} {k}{}", if n == 1 { "" } else { "s" }));
            }
        }
        if parts.is_empty() { "nothing".into() } else { parts.join(", ") }
    }
}

/// The id effects, generators and transitions get in project files:
/// `<plugin>:<item>`.
pub fn qualified(plugin: &str, item: &str) -> String {
    format!("{plugin}:{item}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_manifests() {
        let ok = r#"{"abi":1,"id":"a-b","name":"A","version":"1.0.0","effects":[{"id":"x","name":"X","params":[{"id":"k","label":"K","kind":"float","min":0,"max":1,"default":0.5}]}]}"#;
        let m = Manifest::parse(ok).unwrap();
        assert_eq!(m.summary(), "1 effect");
        assert!(Manifest::parse(&ok.replace("\"abi\":1", "\"abi\":9")).unwrap_err().contains("ABI 9"));
        assert!(Manifest::parse(&ok.replace("a-b", "A B")).is_err());
        assert!(Manifest::parse(&ok.replace("\"float\"", "\"curve\"")).is_err());
        assert!(Manifest::parse("{}").is_err());
    }
}
