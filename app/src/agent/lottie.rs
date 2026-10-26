// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `export` as a Lottie animation (preset "Lottie", or a `.json` /
//! `.lottie` path): see `crate::lottie_export`. Lottie files come in through
//! `import` like any video, with their transparency.

use super::{Session, micros, secs};
use serde_json::{Value, json};
use std::path::PathBuf;

/// Whether an `export` call asks for Lottie.
pub(super) fn wanted(a: &Value) -> bool {
    a.get("preset").and_then(Value::as_str).is_some_and(|p| p.to_lowercase().contains("lottie"))
        || a.get("path").and_then(Value::as_str).is_some_and(|p| {
            let l = p.to_lowercase();
            l.ends_with(".lottie") || l.ends_with(".json")
        })
}

impl Session {
    pub(super) fn export_lottie(&mut self, a: &Value) -> Result<Value, String> {
        let dotlottie = a.get("preset").and_then(Value::as_str).is_some_and(|p| p.to_lowercase().contains("dotlottie"));
        let path = match a.get("path").and_then(Value::as_str) {
            Some(p) => PathBuf::from(p),
            None => {
                let dir = self.home.clone().unwrap_or_else(studio_paths::videos);
                let ext = if dotlottie { "lottie" } else { "json" };
                let name = &self.doc.project.name;
                let first = dir.join(format!("{name}.{ext}"));
                if first.exists() { (2..).map(|n| dir.join(format!("{name} {n}.{ext}"))).find(|p| !p.exists()).expect("a free name") } else { first }
            }
        };
        let (start, end) = match a.get("range") {
            Some(Value::Array(r)) if r.len() == 2 => (micros(r[0].as_f64().unwrap_or(0.0)), micros(r[1].as_f64().unwrap_or(0.0))),
            _ => (0, self.doc.project.duration()),
        };
        let r = crate::lottie_export::export(&self.doc, &path, start, end)?;
        Ok(json!({"ok": true, "path": path.display().to_string(), "format": "lottie", "layers": r.layers, "frames": r.frames, "duration": secs(end - start), "bytes": r.bytes, "left_out": r.left_out}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Content;

    fn call(s: &mut Session, tool: &str, a: Value) -> Value {
        let parts = s.call(tool, &a).unwrap_or_else(|e| panic!("{tool}: {e}"));
        let Content::Text(t) = &parts[0] else { panic!() };
        serde_json::from_str(t).unwrap()
    }

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../crates/picture/lottie/tests/fixtures").join(name)
    }

    /// A Lottie file imports as a video with transparency: on the timeline
    /// it draws over what is below, as the player draws it.
    #[test]
    fn a_lottie_imports_and_plays_over_the_picture() {
        let dir = std::env::temp_dir().join(format!("studio-lottie-in-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("ball.json");
        std::fs::copy(fixture("bouncing-ball.json"), &src).unwrap();
        let mut s = Session::default();
        let v = call(&mut s, "import", json!({"paths": [src.display().to_string()]}));
        let id = v["media"][0].as_u64().unwrap();
        let mi = s.doc.model.media.iter().find(|m| m.id.0 == id).unwrap();
        assert_eq!(mi.info.container, "lottie");
        assert!(mi.info.video.as_ref().unwrap().has_alpha);
        assert!((mi.info.duration.unwrap() as f64 / 1e6 - 1.0).abs() < 0.05, "{:?}", mi.info.duration);
        // A white card under it on the main lane, the ball on top.
        s.edit(&[json!({"op": "card", "colour": "#ffffff", "at": 0, "duration": 1}), json!({"op": "place", "media": id, "at": 0, "lane": "overlay", "fit": "fit"})], "test", false).unwrap();
        let img = s.frames.rgba(&s.doc, micros(0.4), 480).unwrap();
        let (w, h) = (img.width as usize, img.height as usize);
        let px = |x: usize, y: usize| { let i = (y * w + x) * 4; [img.pixels[i], img.pixels[i + 1], img.pixels[i + 2]] };
        // Corners: the white card shows through the animation's transparency.
        assert!(px(2, 2).iter().all(|c| *c >= 250), "{:?}", px(2, 2));
        // Somewhere the ball is drawn (not white).
        assert!((0..h).any(|y| (0..w).any(|x| px(x, y).iter().any(|c| *c < 200))), "the ball shows");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A title moving across with an ease, exported and played back by the
    /// Lottie player, lands where Studio draws it, frame by frame.
    #[test]
    fn an_export_plays_back_where_studio_draws_it() {
        let dir = std::env::temp_dir().join(format!("studio-lottie-out-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = Session::default();
        s.edit(
            &[
                json!({"op": "card", "colour": "#2060d0", "at": 0, "duration": 2, "width": 300, "height": 200, "lane": "overlay"}),
                json!({"op": "animate", "clip": "$last", "property": "x", "keys": [[0, -600], [2, 600]], "ease": "ease_in_out"}),
                json!({"op": "animate", "clip": "$1", "property": "rotation", "keys": [[0, 0], [2, 90]], "ease": "linear"}),
            ],
            "test",
            false,
        )
        .unwrap();
        let out = dir.join("move.json");
        let v = call(&mut s, "export", json!({"path": out.display().to_string()}));
        assert_eq!(v["layers"], 1, "{v}");
        let anim = studio_lottie::Animation::open(&out).unwrap();
        assert_eq!((anim.width, anim.height), (1920, 1080));
        // The steady turn needs only its two ends; the eased move a few, each
        // carrying its ease.
        let doc: Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!(doc["layers"][0]["ks"]["r"]["k"].as_array().unwrap().len(), 2);
        let keys = doc["layers"][0]["ks"]["p"]["x"]["k"].as_array().unwrap().len();
        assert!(keys <= 6, "{keys} keys for an eased move over 60 frames");
        assert_eq!(doc["layers"][0]["ks"]["p"]["y"]["a"], 0, "y stays put");
        // Where the blue card's middle is, in Studio and in the player.
        let centre = |rgba: &[u8], w: usize| -> (f64, f64) {
            let (mut sx, mut sy, mut n) = (0.0, 0.0, 0.0);
            for (i, p) in rgba.chunks_exact(4).enumerate() {
                if p[2] > 150 && p[0] < 100 && p[3] > 128 {
                    sx += (i % w) as f64;
                    sy += (i / w) as f64;
                    n += 1.0;
                }
            }
            (sx / n, sy / n)
        };
        for t in [0.3, 1.0, 1.7] {
            let studio = s.frames.rgba(&s.doc, micros(t), 1920).unwrap();
            let played = anim.render_rgba(t * anim.fps, 1920, 1080);
            let (a, b) = (centre(&studio.pixels, 1920), centre(&played, 1920));
            assert!((a.0 - b.0).abs() < 2.0 && (a.1 - b.1).abs() < 2.0, "at {t}s Studio {a:?}, Lottie {b:?}");
        }
        // dotLottie too.
        let lot = dir.join("move.lottie");
        call(&mut s, "export", json!({"path": lot.display().to_string()}));
        assert_eq!(studio_lottie::Animation::open(&lot).unwrap().width, 1920);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Writes an export for checking with other players (`$LOTTIE_OUT`, by hand).
#[cfg(test)]
#[test]
#[ignore]
fn write_sample_export() {
    let Ok(out) = std::env::var("LOTTIE_OUT") else { return };
    let mut s = Session::default();
    s.edit(
        &[
            json!({"op": "card", "colour": "#2060d0", "at": 0, "duration": 2, "width": 300, "height": 200, "lane": "overlay"}),
            json!({"op": "animate", "clip": "$last", "property": "x", "keys": [[0, -600], [2, 600]], "ease": "ease_in_out"}),
            json!({"op": "animate", "clip": "$1", "property": "rotation", "keys": [[0, 0], [2, 90]], "ease": "linear"}),
            json!({"op": "animate", "clip": "$1", "property": "opacity", "keys": [[0, 30], [1, 100]]}),
        ],
        "sample",
        false,
    )
    .unwrap();
    s.export_lottie(&json!({"path": out})).unwrap();
    for t in [0.3, 1.0, 1.7] {
        let img = s.frames.rgba(&s.doc, micros(t), 1920).unwrap();
        let (mut sx, mut n) = (0.0, 0.0);
        for (i, p) in img.pixels.chunks_exact(4).enumerate() {
            if p[2] > 150 && p[0] < 100 {
                sx += (i % 1920) as f64;
                n += 1.0;
            }
        }
        eprintln!("studio {t}: card centre x {:.1}", sx / n);
    }
}
