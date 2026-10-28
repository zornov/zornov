// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Ops an agent tripped over on a real project, kept working.

use super::Session;
use crate::services;
use serde_json::{Value, json};
use studio_model as m;

fn session() -> Session {
    Session::new(services::project::new_custom("Fixes", 640, 360, 30.0))
}

fn last_clip(s: &Session) -> u32 {
    s.doc.project.clips.iter().map(|c| c.id).max().unwrap()
}

fn still(name: &str, rgb: u8) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("studio-fix-tests-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, studio_media::png::encode(&studio_media::RgbaImage { width: 16, height: 9, pixels: [rgb, rgb, rgb, 255].repeat(16 * 9) })).unwrap();
    p
}

/// `place` and `replace` take a file path for `media`: the file comes in
/// as media in the same undo step; a file already there is that media.
#[test]
fn media_can_be_named_by_its_file() {
    let mut s = session();
    let a = still("a.png", 40);
    let b = still("b.png", 200);
    let r = s.edit(&[json!({"op": "place", "media": a.display().to_string(), "at": 0, "duration": 2})], "place", false).unwrap();
    assert!(r["notes"].to_string().contains("imported a.png"), "{r}");
    let clip = last_clip(&s);
    let a_id = s.doc.project.clip(clip).unwrap().media.unwrap();
    s.edit(&[json!({"op": "replace", "clip": clip, "media": b.display().to_string()})], "replace", false).unwrap();
    let b_id = s.doc.project.clip(clip).unwrap().media.unwrap();
    assert_ne!(a_id, b_id, "the clip shows the new file");
    assert!(s.doc.project.media(b_id).is_some_and(|m| m.path.as_deref() == Some(b.as_path())));
    // The same file again: the same media, nothing imported twice.
    let n = s.doc.project.media.len();
    s.edit(&[json!({"op": "replace", "clip": clip, "media": a.display().to_string()})], "back", false).unwrap();
    assert_eq!(s.doc.project.media.len(), n);
    assert_eq!(s.doc.project.clip(clip).unwrap().media, Some(a_id));
    // One undo takes the replace back, import and all.
    s.doc.undo();
    assert_eq!(s.doc.project.clip(clip).unwrap().media, Some(b_id));
    // A path to nothing is still an unknown name.
    let e = s.edit(&[json!({"op": "replace", "clip": clip, "media": "/no/such/file.png"})], "x", false).unwrap_err();
    assert!(e.contains("no media"), "{e}");
}

/// A drawn box takes an outline through `set`: a colour and a width in
/// pixels (a colour alone gets a 4 px line).
#[test]
fn set_gives_a_box_an_outline() {
    let mut s = session();
    s.edit(&[json!({"op": "card", "colour": "#203040", "at": 0, "duration": 2, "width": 200, "height": 100})], "box", false).unwrap();
    let id = last_clip(&s);
    // Through `run`, as an agent calls it: pixel lengths need the canvas.
    s.run("edit", &json!({"ops": [{"op": "set", "clip": id, "stroke": "#55d8c0", "stroke_width": 6}]})).unwrap();
    let params = |s: &Session| match &s.model_clip(id).unwrap().source {
        m::ClipSource::Generator(g) => g.params.clone(),
        _ => panic!("a box"),
    };
    let p = params(&s);
    assert!(matches!(p.get("stroke"), Some(m::Value::Colour(c)) if (c.g - 0xd8 as f64 / 255.0).abs() < 1e-3), "{p:?}");
    assert!((p["stroke_width"].as_f64().unwrap() * 360.0 - 6.0).abs() < 1e-6, "6 px of the short side");
    s.edit(&[json!({"op": "card", "colour": "#203040", "at": 2, "duration": 2, "width": 200, "height": 100})], "box", false).unwrap();
    let id2 = last_clip(&s);
    s.run("edit", &json!({"ops": [{"op": "set", "clip": id2, "stroke": "white"}]})).unwrap();
    let p = match &s.model_clip(id2).unwrap().source {
        m::ClipSource::Generator(g) => g.params.clone(),
        _ => panic!(),
    };
    assert!((p["stroke_width"].as_f64().unwrap() * 360.0 - 4.0).abs() < 1e-6, "{p:?}");
}

/// A 3D model fades like any clip: opacity in percent, from the value it
/// has before anything was stored.
#[test]
fn a_model_fades_in_and_out() {
    let mut s = session();
    s.edit(&[json!({"op": "model", "shape": "sphere", "size": 100, "detail": 2})], "cube", false).unwrap();
    let id = last_clip(&s);
    s.edit(&[json!({"op": "animate", "clip": id, "property": "opacity", "keys": [[0, 0], [0.3, 100], [0.7, 100], [1, 0]]})], "fade", false).unwrap();
    let mc = s.model_clip(id).unwrap();
    let keys: Vec<f64> = mc.keyframes["generator.opacity"].keys.iter().filter_map(|k| k.value.as_f64()).collect();
    assert_eq!(keys, vec![0.0, 1.0, 1.0, 0.0]);
    let v: Value = s.edit(&[json!({"op": "animate", "clip": id, "property": "roughness", "keys": [[0, 0.2], [1, 0.8]]})], "rough", false).unwrap();
    assert!(v["ok"].as_bool().unwrap_or(true), "{v}");
}

/// A merged clip that plays on past the end of its layers flattens: the
/// render covers what it plays, or the clip keeps to the render.
#[cfg(feature = "render")]
#[test]
fn a_merged_clip_longer_than_its_layers_flattens() {
    let mut s = session();
    s.edit(&[json!({"op": "card", "colour": "#ff8800", "at": 0, "duration": 1, "width": 100, "height": 100, "lane": "overlay"})], "card", false).unwrap();
    let card = last_clip(&s);
    s.edit(&[json!({"op": "card", "colour": "#0088ff", "at": 0, "duration": 1, "width": 50, "height": 50, "lane": "overlay"})], "card", false).unwrap();
    let other = last_clip(&s);
    s.edit(&[json!({"op": "merge", "clips": [card, other], "name": "Box"})], "merge", false).unwrap();
    let merged = last_clip(&s);
    s.edit(&[json!({"op": "set", "clip": merged, "duration": 1.6})], "longer", false).unwrap();
    let mut r = s.edit(&[json!({"op": "flatten", "clip": merged})], "flatten", false);
    // A slow machine renders past export's wait: ask again with the job.
    let until = std::time::Instant::now() + std::time::Duration::from_secs(600);
    while let Some(eid) = r.as_ref().ok().and_then(super::layers::pending_flatten) {
        assert!(std::time::Instant::now() < until, "the flatten render never finished");
        std::thread::sleep(std::time::Duration::from_millis(200));
        r = s.edit(&[json!({"op": "flatten", "clip": merged, "export": eid})], "flatten", false);
    }
    assert!(r.is_ok(), "{r:?}");
    let mc = s.model_clip(merged).unwrap();
    let m::ClipSource::Media(r) = &mc.source else { panic!("plays from a file") };
    let total = s.doc.model.media.iter().find(|x| x.id == r.media).and_then(|x| x.duration()).unwrap();
    assert!(mc.source_out() <= total, "{} of {total}", mc.source_out());
    assert!(mc.duration >= 990_000, "{}", mc.duration);
}
