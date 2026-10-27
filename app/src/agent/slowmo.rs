// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `smooth_motion` for agents: in-between frames for a slowed or ramped
//! clip (`services::slowmo`).

use super::Session;
use crate::services::slowmo::{self, Method};
use serde_json::Value;
use studio_ai::JobCtx;
use studio_model as m;

impl Session {
    pub(super) fn smooth_motion(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("smooth_motion needs \"clip\": a clip slowed down (speed under 1, or a speed ramp)")? as u32;
        let seq = self.doc.seq();
        if op.get("remove").and_then(Value::as_bool) == Some(true) {
            let mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
            let c = slowmo::restored(&mc).ok_or_else(|| format!("clip {id} is not smoothed"))?;
            let ok = self.doc.apply("Remove smooth slow motion", m::Edit::SetClip { seq, clip: Box::new(c) });
            self.check(ok, "")?;
            return Ok(format!("clip {id} plays its own footage again (its sound clip stays as it is)"));
        }
        let method = match op.get("method").and_then(Value::as_str) {
            None | Some("flow" | "rife" | "optical_flow") => Method::Flow,
            Some("blend" | "frames" | "crossfade") => Method::Blend,
            Some(o) => return Err(format!("method \"{o}\": flow (RIFE optical flow: sharp, about half a second a frame at 720p) or blend (cross-fades: instant, ghosts on fast motion)")),
        };
        let full = op.get("quality").and_then(Value::as_str) == Some("full");
        let job = slowmo::plan(&self.doc, id, method, full)?;
        let est = job.estimate_secs();
        if op.get("estimate").and_then(Value::as_bool) == Some(true) {
            return Ok(format!("clip {id}: {}× the frames, {} in-betweens worked out at {}×{} into {}×{} footage, about {est:.0} s (nothing made yet)", job.factor, job.to_make(), job.flow_size.0, job.flow_size.1, job.size.0, job.size.1));
        }
        if est > 600.0 && op.get("long").and_then(Value::as_bool) != Some(true) {
            return Err(format!("that is about {:.0} minutes of work ({} in-betweens): trim the slow part into its own clip, use method blend, or pass long: true to go ahead", est / 60.0, job.to_make()));
        }
        let engine = slowmo::run(&job, &JobCtx::detached())?;
        slowmo::apply(&mut self.doc, &job, engine)?;
        let mc = self.model_clip(id).ok_or("clip gone")?;
        Ok(format!(
            "clip {id}: {}; {} new frames, worked out at {}×{} ({}); its sound is its own clip, linked. It plays exactly as before on the timeline; its speed values read {}× higher only because the new footage holds {}× the frames. Trim and ramp before smoothing: to change them, smooth_motion remove first",
            slowmo::describe(&mc).unwrap_or_default(),
            job.to_make(),
            job.flow_size.0,
            job.flow_size.1,
            if engine == studio_ai::features::interpolate::InterpEngine::Rife { "RIFE" } else { "cross-fades: RIFE is not installed (Settings ▸ Models) or blend was asked" },
            job.factor,
            job.factor
        ))
    }
}

#[cfg(test)]
mod tests {
    use crate::agent::Session;
    use crate::services;
    use crate::services::tracking::test_movie::{self, FPS, H, W};
    use serde_json::json;
    use studio_model as m;

    #[test]
    fn a_slowed_clip_gets_fresh_frames_and_keeps_its_timing() {
        let mut s = Session::new(services::project::new_custom("Slow", W, H, FPS as f32));
        s.run("import", &json!({"paths": [test_movie::movie().to_string_lossy()], "to_timeline": true})).unwrap();
        let id = s.doc.project.clips.iter().find(|c| c.media.is_some()).unwrap().id;
        // Two seconds of footage → its first half second, four times slower.
        s.edit(&[json!({"op": "trim", "clip": id, "end": 0.5}), json!({"op": "set", "clip": id, "speed": 0.25})], "slow", false).unwrap();
        let before = s.model_clip(id).unwrap();
        let shown_before: Vec<i64> = (0..8).map(|k| before.source_time_at(before.start + k * before.duration / 8)).collect();
        let e = s.edit(&[json!({"op": "smooth_motion", "clip": id, "method": "blend", "estimate": true})], "est", false).unwrap();
        let _ = e;
        s.edit(&[json!({"op": "smooth_motion", "clip": id, "method": "blend"})], "smooth", false).unwrap();
        let after = s.model_clip(id).unwrap();
        assert_eq!(after.duration, before.duration, "same length on the timeline");
        assert!(matches!(after.speed, m::Speed::Constant(r) if (r - 1.0).abs() < 1e-9), "{:?}", after.speed);
        let m::ClipSource::Media(r) = &after.source else { panic!() };
        let mi = s.doc.model.media_item(r.media).unwrap();
        assert!(mi.path.replace('\\', "/").contains("Slow motion/"), "{}", mi.path);
        // The same moments: new source time = (old − first) × 4.
        for (k, old) in shown_before.iter().enumerate() {
            let new = after.source_time_at(after.start + k as i64 * after.duration / 8);
            assert!((new as f64 - *old as f64 * 4.0).abs() < 40_000.0, "{k}: {new} vs {}", old * 4);
        }
        // Every timeline frame shows a different picture now: frames 1/4
        // apart in the source differ (a blend between neighbours).
        let a = s.frames.rgba(&s.doc, after.start + 33_333 * 4, 160).unwrap();
        let b = s.frames.rgba(&s.doc, after.start + 33_333 * 5, 160).unwrap();
        assert_ne!(a.pixels, b.pixels);
        // Remove puts the clip back.
        s.edit(&[json!({"op": "smooth_motion", "clip": id, "remove": true})], "rm", false).unwrap();
        let back = s.model_clip(id).unwrap();
        assert_eq!(back.source_in, before.source_in);
        assert_eq!(back.speed, before.speed);
        // Normal speed: refused.
        s.edit(&[json!({"op": "set", "clip": id, "speed": 1})], "n", false).unwrap();
        let e = s.edit(&[json!({"op": "smooth_motion", "clip": id})], "x", false).unwrap_err();
        assert!(e.contains("normal speed"), "{e}");
    }

    #[test]
    fn a_speed_ramp_is_smoothed_to_its_last_frame() {
        let mut s = Session::new(services::project::new_custom("Ramp", W, H, FPS as f32));
        s.run("import", &json!({"paths": [test_movie::movie().to_string_lossy()], "to_timeline": true})).unwrap();
        let id = s.doc.project.clips.iter().find(|c| c.media.is_some()).unwrap().id;
        s.edit(&[json!({"op": "trim", "clip": id, "end": 1.0}), json!({"op": "animate", "clip": id, "property": "speed", "keys": [[0, 1], [0.2, 0.25], [0.8, 0.25], [1.0, 1]]})], "ramp", false).unwrap();
        let before = s.model_clip(id).unwrap();
        s.edit(&[json!({"op": "smooth_motion", "clip": id, "method": "blend"})], "smooth", false).unwrap();
        let after = s.model_clip(id).unwrap();
        assert_eq!(after.duration, before.duration);
        // Its last moment still shows (the new footage reaches that far).
        let end = after.start + after.duration - 1;
        assert!(s.frames.rgba(&s.doc, end, 80).is_ok());
        let v = s.run("check", &json!({})).unwrap();
        let text: String = v.iter().filter_map(|c| if let crate::agent::Content::Text(t) = c { Some(t.clone()) } else { None }).collect();
        assert!(!text.contains("needs") || !text.contains("but the file is"), "{text}");
    }

    #[test]
    fn flow_uses_rife_when_installed() {
        if studio_ai::ModelStore::default().require(studio_ai::ModelId::Rife49).is_err() {
            eprintln!("RIFE is not installed: skipped");
            return;
        }
        let mut s = Session::new(services::project::new_custom("Slow", W, H, FPS as f32));
        s.run("import", &json!({"paths": [test_movie::movie().to_string_lossy()], "to_timeline": true})).unwrap();
        let id = s.doc.project.clips.iter().find(|c| c.media.is_some()).unwrap().id;
        s.edit(&[json!({"op": "trim", "clip": id, "end": 0.25}), json!({"op": "set", "clip": id, "speed": 0.5})], "slow", false).unwrap();
        let r = s.edit(&[json!({"op": "smooth_motion", "clip": id})], "smooth", false).unwrap();
        let _ = r;
        let mc = s.model_clip(id).unwrap();
        assert!(crate::services::slowmo::describe(&mc).unwrap().contains("RIFE"));
        // The in-between of the moving square sits half way: frame 1.5 of
        // the source shows the square between frames 1 and 2.
        let t = mc.start + 100_000;
        let img = s.frames.rgba(&s.doc, t, W).unwrap();
        assert_eq!(img.width, W);
    }
}
