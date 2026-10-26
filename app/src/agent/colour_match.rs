// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `match_colour` for agents: shots moved towards a reference shot's colour
//! and light (`services::colour_match`).

use super::{Session, f};
use crate::services::colour_match as cm;
use serde_json::Value;
use studio_model as m;

impl Session {
    pub(super) fn match_colour(&mut self, op: &Value) -> Result<String, String> {
        let mut targets: Vec<u32> = op.get("clips").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(|v| v as u32).collect()).unwrap_or_default();
        if let Some(c) = op.get("clip").and_then(Value::as_u64) {
            targets.push(c as u32);
        }
        if targets.is_empty() {
            return Err("match_colour needs \"clip\" or \"clips\": the shots to change".into());
        }
        let seq = self.doc.seq();
        if op.get("remove").and_then(Value::as_bool) == Some(true) {
            let mut edits = Vec::new();
            for id in &targets {
                let mut mc = self.model_clip(*id).ok_or_else(|| format!("no clip {id}"))?;
                if cm::clear(&mut mc) {
                    edits.push(m::Edit::SetClip { seq, clip: Box::new(mc) });
                }
            }
            if edits.is_empty() {
                return Err("none of those clips is matched".into());
            }
            let n = edits.len();
            let ok = self.doc.apply("Remove colour match", m::Edit::Batch { edits });
            self.check(ok, "")?;
            return Ok(format!("took the colour match off {n} clip(s)"));
        }
        let reference = op.get("to").and_then(Value::as_u64).ok_or("match_colour needs \"to\": the shot whose look to match")? as u32;
        if targets.contains(&reference) {
            return Err("a shot cannot be matched to itself".into());
        }
        let strength = f(op, "strength").map(|s| if s > 1.0 { s / 100.0 } else { s }).unwrap_or(1.0).clamp(0.0, 1.0);
        let p = self.doc.project.clone();
        let rc = self.model_clip(reference).ok_or_else(|| format!("no clip {reference}"))?;
        let want = cm::stats(&p, &rc, true)?;
        let mut edits = Vec::new();
        let mut said = Vec::new();
        for id in &targets {
            let mut mc = self.model_clip(*id).ok_or_else(|| format!("no clip {id}"))?;
            cm::clear(&mut mc);
            let have = cm::stats(&p, &mc, false)?;
            let before = cm::stats(&p, &mc, true)?;
            cm::set(&mut mc, &have, &want, strength);
            let after = cm::stats(&p, &mc, true)?;
            said.push(format!("clip {id}: before, {}; after, {}", cm::difference(&before, &want), cm::difference(&after, &want)));
            edits.push(m::Edit::SetClip { seq, clip: Box::new(mc) });
        }
        let ok = self.doc.apply("Match colour", m::Edit::Batch { edits });
        self.check(ok, "")?;
        Ok(format!("matched to clip {reference} at {:.0}%: {}. The match comes first in each clip's grade; its own exposure, contrast and look still apply on top. Look at a cut between them with frame", strength * 100.0, said.join("; ")))
    }
}

#[cfg(test)]
mod tests {
    use crate::agent::Session;
    use crate::services;
    use crate::services::colour_match as cm;
    use serde_json::json;

    fn still(path: &std::path::Path, rgb: [u8; 3], tilt: i32) {
        let (w, h) = (96u32, 54u32);
        let mut px = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let k = (x as i32 * tilt / 96) as i16;
                px.extend([(rgb[0] as i16 + k).clamp(0, 255) as u8, (rgb[1] as i16 + k).clamp(0, 255) as u8, (rgb[2] as i16 + (y as i16 / 4)).clamp(0, 255) as u8, 255]);
            }
        }
        let img = studio_media::rgba::RgbaImage { width: w, height: h, pixels: px };
        std::fs::write(path, studio_media::png::encode(&img)).unwrap();
    }

    #[test]
    fn a_cool_dark_shot_is_matched_to_a_warm_bright_one() {
        let dir = std::env::temp_dir().join(format!("studio-match-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("cool.png"), dir.join("warm.png"));
        still(&a, [40, 60, 110], 30);
        still(&b, [200, 150, 90], 60);
        let mut s = Session::new(services::project::new_custom("Match", 640, 360, 30.0));
        s.run("import", &json!({"paths": [a.to_string_lossy(), b.to_string_lossy()], "to_timeline": true})).unwrap();
        let ids: Vec<u32> = s.doc.project.clips.iter().filter(|c| c.media.is_some()).map(|c| c.id).collect();
        let r = s.edit(&[json!({"op": "match_colour", "clip": ids[0], "to": ids[1]})], "m", false).unwrap();
        let _ = r;
        let p = s.doc.project.clone();
        let (mc, rc) = (s.model_clip(ids[0]).unwrap(), s.model_clip(ids[1]).unwrap());
        let (got, want) = (cm::stats(&p, &mc, true).unwrap(), cm::stats(&p, &rc, true).unwrap());
        for i in 0..3 {
            assert!((got.mean[i] - want.mean[i]).abs() < 0.02, "channel {i}: {:?} vs {:?}", got.mean, want.mean);
        }
        assert!(cm::strength(&mc) == Some(1.0));
        // The renderer shows it: the matched shot's frame is warm now.
        let t = s.doc.project.clip(ids[0]).unwrap().start + 100_000;
        let img = s.frames.rgba(&s.doc, t, 64).unwrap();
        let (r, b) = (img.pixels[(18 * 64 + 32) * 4] as i32, img.pixels[(18 * 64 + 32) * 4 + 2] as i32);
        assert!(r > b + 40, "warm after matching: r {r} b {b}");
        s.edit(&[json!({"op": "match_colour", "clip": ids[0], "remove": true})], "rm", false).unwrap();
        assert!(cm::strength(&s.model_clip(ids[0]).unwrap()).is_none());
        let e = s.edit(&[json!({"op": "match_colour", "clip": ids[0], "to": ids[0]})], "x", false).unwrap_err();
        assert!(e.contains("itself"), "{e}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
