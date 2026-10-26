// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The project's character library: each character kept as one look from
//! shot to shot. A character is a folder in the project's `Characters`
//! folder holding its reference picture, the description its pictures are
//! made from, its turnaround views, poses and expressions (pictures made
//! from the reference, so they keep its design), its mouth and hand
//! shapes for swap sets, and its rig: a skeleton (with its pose library)
//! that goes onto any layer of it.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use studio_model as m;

/// What a picture of a character shows.
pub const KINDS: &str = "view (front, side, back, three_quarter), pose (running, sitting…), expression (happy, angry…), mouth (closed, open, wide, round), hand (open, fist, point), eyes (open, closed)";

/// One picture of a character.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Picture {
    /// "view", "pose", "expression", "mouth", "hand", "eyes".
    pub kind: String,
    /// What it is ("side", "running", "happy", "open").
    pub name: String,
    /// The file, inside the character's folder.
    pub file: String,
}

/// A character in the library.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Character {
    pub name: String,
    /// What kind of body it has (a skeleton kind: biped, quadruped…).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// The words its pictures are made from: what it looks like.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// The reference picture every other one is made from.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reference: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pictures: Vec<Picture>,
    /// Its rig, drawn on the reference picture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skeleton: Option<m::Skeleton>,
    /// Its pose library (also on its skeleton when it has one).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub poses: Vec<m::Pose>,
}

impl Character {
    /// The picture of `kind` called `name` (any case).
    pub fn picture(&self, kind: &str, name: &str) -> Option<&Picture> {
        self.pictures.iter().find(|p| p.kind.eq_ignore_ascii_case(kind) && p.name.eq_ignore_ascii_case(name))
    }
}

fn slug(s: &str) -> String {
    let out: String = s.trim().chars().map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let out = out.trim_matches('-').to_owned();
    if out.is_empty() { "character".into() } else { out }
}

/// The library's folder in a project folder.
pub fn library(project: &Path) -> PathBuf {
    project.join("Characters")
}

/// A character's folder.
pub fn folder(project: &Path, name: &str) -> PathBuf {
    library(project).join(slug(name))
}

/// Saves a character (its `character.json`).
pub fn save(project: &Path, c: &Character) -> Result<(), String> {
    let dir = folder(project, &c.name);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let text = serde_json::to_string_pretty(c).map_err(|e| e.to_string())?;
    let tmp = dir.join("character.json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, dir.join("character.json")).map_err(|e| e.to_string())
}

/// Loads the character called `name`.
pub fn load(project: &Path, name: &str) -> Result<Character, String> {
    let file = folder(project, name).join("character.json");
    let text = std::fs::read_to_string(&file).map_err(|_| format!("no character \"{name}\" in this project's library{}", names(project).map_or(String::new(), |n| if n.is_empty() { String::new() } else { format!(" (characters: {})", n.join(", ")) })))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", file.display()))
}

/// Every character's name.
pub fn names(project: &Path) -> Option<Vec<String>> {
    let mut out: Vec<String> = std::fs::read_dir(library(project))
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| std::fs::read_to_string(e.path().join("character.json")).ok())
        .filter_map(|t| serde_json::from_str::<Character>(&t).ok())
        .map(|c| c.name)
        .collect();
    out.sort();
    Some(out)
}

/// The full path of one of a character's files.
pub fn path_of(project: &Path, c: &Character, file: &str) -> PathBuf {
    folder(project, &c.name).join(file)
}

/// Copies picture `from` into the character's folder as `kind`/`name`
/// (replacing one of the same kind and name) and returns its file name.
pub fn add_picture(project: &Path, c: &mut Character, kind: &str, name: &str, from: &Path) -> Result<String, String> {
    let ext = from.extension().and_then(|e| e.to_str()).unwrap_or("png").to_lowercase();
    let file = format!("{}-{}.{ext}", slug(kind), slug(name));
    let dir = folder(project, &c.name);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    std::fs::copy(from, dir.join(&file)).map_err(|e| format!("{}: {e}", from.display()))?;
    c.pictures.retain(|p| !(p.kind.eq_ignore_ascii_case(kind) && p.name.eq_ignore_ascii_case(name)));
    c.pictures.push(Picture { kind: kind.to_lowercase(), name: name.to_lowercase(), file: file.clone() });
    Ok(file)
}

/// The words for making one more picture of a character: its own
/// description, what this picture shows, and the instruction to keep
/// everything else of the reference.
pub fn prompt(c: &Character, kind: &str, name: &str) -> String {
    let what = match kind {
        "view" => match name {
            "side" | "profile" => "seen from the side, in profile, facing right".to_owned(),
            "back" => "seen from behind".to_owned(),
            "three_quarter" | "3/4" => "in a three-quarter view, turned a little to the right".to_owned(),
            "front" => "seen from the front".to_owned(),
            other => format!("seen {other}"),
        },
        "pose" => format!("in the pose: {name}, full body"),
        "expression" => format!("with a {name} expression, head and shoulders"),
        "mouth" => format!("only its mouth, {name} (as when saying a sound), the same drawing style, nothing else"),
        "hand" => format!("only its hand, {name}, the same drawing style"),
        "eyes" => format!("only its eyes, {name}"),
        other => format!("{other}: {name}"),
    };
    let desc = if c.description.is_empty() { "the character in the reference picture".to_owned() } else { c.description.clone() };
    format!("{desc}, {what}. The same character as the reference picture: identical design, face, proportions, colours, outfit and line style; one character only, centred, on a transparent background.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn characters_are_kept_with_their_pictures_and_rig() {
        let home = std::env::temp_dir().join(format!("studio-tests/cast-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&home).unwrap();
        let pic = home.join("ref.png");
        std::fs::write(&pic, b"not really a png").unwrap();
        let mut c = Character { name: "Mia the Fox".into(), kind: "quadruped".into(), description: "a small orange fox with a white-tipped tail".into(), ..Character::default() };
        c.reference = add_picture(&home, &mut c, "view", "front", &pic).unwrap();
        add_picture(&home, &mut c, "view", "side", &pic).unwrap();
        add_picture(&home, &mut c, "view", "side", &pic).unwrap();
        assert_eq!(c.pictures.len(), 2, "a picture of the same name is replaced");
        let mut sk = m::Skeleton::default();
        sk.add("spine", None, m::Vec2::new(0.3, 0.4), m::Vec2::new(0.7, 0.4));
        c.skeleton = Some(sk);
        c.poses.push(m::Pose { name: "sit".into(), ..m::Pose::default() });
        save(&home, &c).unwrap();
        let back = load(&home, "mia the fox").unwrap();
        assert_eq!(back, c);
        assert_eq!(names(&home).unwrap(), vec!["Mia the Fox".to_owned()]);
        assert!(path_of(&home, &c, &c.picture("view", "SIDE").unwrap().file).exists());
        assert!(load(&home, "Rex").unwrap_err().contains("Mia the Fox"));
        let p = prompt(&c, "view", "side");
        assert!(p.contains("orange fox") && p.contains("profile") && p.contains("same character"));
    }
}
