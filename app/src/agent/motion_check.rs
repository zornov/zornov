// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Motion that reads as a mistake on screen, found from the keyframes
//! themselves (a still frame or a contact sheet cannot show it):
//!
//! * **snap**: most of a move happens in one or two frames of a longer
//!   move (an `expo_in` fill that sits still, then jumps; a `whip` used
//!   for something the eye follows).
//! * **stall**: a move that spends most of its time barely moving, then
//!   rushes (the same curves, seen from the other side).
//! * **speed step**: a footage speed change with no ramp: the picture
//!   lurches from one speed to the next.
//!
//! Hits are allowed to be hits: a move of a few frames, `hold` keys, and
//! `steps` curves are what they say.

use crate::view::SEC;
use studio_model as m;

pub struct Problem {
    pub kind: &'static str,
    pub at: f64,
    pub clip: u32,
    pub problem: String,
    pub fix: &'static str,
}

/// The friendly name of a keyed property on `c`, for the report.
fn name_of(c: &m::Clip, path: &str) -> String {
    if let Some(rest) = path.strip_prefix("effect.") {
        // effect.<id>.<setting> → zoom_blur.amount
        if let Some((id, setting)) = rest.split_once('.') {
            if let Some(e) = c.effects.iter().find(|e| e.id.0.to_string() == id) {
                return format!("{}.{setting}", super::props::effect_name(&e.kind));
            }
        }
    }
    match path {
        "transform.position" => "position".into(),
        "generator.size" => "width/height".into(),
        p => p.strip_prefix("transform.").unwrap_or(p).into(),
    }
}

/// How far the value moved between two samples (points by distance).
fn delta(a: &m::Value, b: &m::Value) -> Option<f64> {
    Some(match (a, b) {
        (m::Value::Scalar(x), m::Value::Scalar(y)) | (m::Value::Angle(x), m::Value::Angle(y)) => (y - x).abs(),
        (m::Value::Point(p), m::Value::Point(q)) => (q.x - p.x).hypot(q.y - p.y),
        _ => return None,
    })
}

/// Every move on the timeline (merged clips' insides too, at their place
/// on the timeline), at `fps`. One problem per clip and moment, however
/// many of its properties share the move.
pub fn problems(project: &m::Project, fps: f64) -> Vec<Problem> {
    let mut found: Vec<(f64, u32, &'static str, Vec<String>, String, &'static str)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    visit(project, project.active_sequence, 0, None, fps, &mut found, &mut seen);
    // Properties of one move, together.
    let mut out: Vec<Problem> = Vec::new();
    found.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    for (at, clip, kind, props, detail, fix) in found {
        // Several layers inside one merged clip moving together: once.
        if out.last().is_some_and(|p| p.clip == clip && (p.at - at).abs() < 1e-6 && p.kind == kind) {
            continue;
        }
        out.push(Problem { kind, at, clip, problem: format!("clip {clip}'s {} {detail}", props.join(", ")), fix });
    }
    out
}

#[allow(clippy::type_complexity)]
fn visit(project: &m::Project, seq: m::SequenceId, offset: i64, outer: Option<u32>, fps: f64, out: &mut Vec<(f64, u32, &'static str, Vec<String>, String, &'static str)>, seen: &mut std::collections::HashSet<(m::SequenceId, i64)>) {
    if !seen.insert((seq, offset)) {
        return;
    }
    let Some(sq) = project.sequence(seq) else { return };
    let frame = (SEC as f64 / fps.max(1.0)).round() as i64;
    for c in &sq.clips {
        // Problems inside a merged clip are reported on the merged clip,
        // the one the timeline shows.
        let shown = outer.unwrap_or(c.id.0 as u32);
        if let m::ClipSource::Sequence(r) = &c.source {
            visit(project, r.sequence, offset + c.start - c.source_in, Some(shown), fps, out, seen);
        }
        let mut moves: std::collections::BTreeMap<(i64, &'static str), (Vec<String>, String, &'static str)> = Default::default();
        for (path, track) in &c.keyframes {
            if path.starts_with("audio.") {
                continue;
            }
            for w in track.keys.windows(2) {
                let (k0, k1) = (&w[0], &w[1]);
                let frames = ((k1.time - k0.time) / frame.max(1)) as usize;
                // Hits (a third of a second or less), holds and steps are
                // what they say.
                if frames < 10 || k0.interp == m::Interp::Hold || matches!(k0.curve, Some(m::Curve::Steps { .. })) {
                    continue;
                }
                let samples: Vec<m::Value> = (0..=frames).filter_map(|i| track.eval(k0.time + (k1.time - k0.time) * i as i64 / frames as i64)).collect();
                let steps: Vec<f64> = samples.windows(2).filter_map(|s| delta(&s[0], &s[1])).collect();
                let travel: f64 = steps.iter().sum();
                if steps.len() != frames || travel < 1e-6 {
                    continue;
                }
                // Too small to see (a 1% opacity drift, a 2 px nudge).
                let visible = match &k0.value {
                    m::Value::Point(_) => travel >= 24.0,
                    _ if path.contains("opacity") || path.contains("scale") => travel >= 0.08,
                    m::Value::Angle(_) => travel >= 8.0,
                    _ => true,
                };
                if !visible {
                    continue;
                }
                let mut sorted = steps.clone();
                sorted.sort_by(|a, b| b.total_cmp(a));
                let top2 = sorted.iter().take(2).sum::<f64>() / travel;
                let first_half = steps[..frames / 2].iter().sum::<f64>() / travel;
                let start = offset + c.start + k0.time;
                let at = start as f64 / SEC as f64;
                let span = (k1.time - k0.time) as f64 / SEC as f64;
                let found = if top2 > 0.5 {
                    Some(("motion_snap", format!("({at:.2}–{:.2}s): {:.0}% of the move happens in 2 of its {frames} frames, so it jumps instead of moving", at + span, top2 * 100.0), "use an _out curve (power2_out, expo_out, back) for things arriving or filling, smooth or sine_in_out for moves the eye follows; keep _in, whip and snap for things leaving or for hits a third of a second or shorter"))
                } else if first_half < 0.12 {
                    Some(("motion_stall", format!("({at:.2}–{:.2}s): it barely moves for the first half of its {span:.2}s ({:.0}% of the change), then rushes", at + span, first_half * 100.0), "an arrival or a fill starts fast and settles (power2_out, expo_out, back): put the motion where the eye is looking, or shorten the move"))
                } else {
                    None
                };
                if let Some((kind, detail, fix)) = found {
                    let e = moves.entry((start, kind)).or_insert_with(|| (Vec::new(), detail, fix));
                    let n = name_of(c, path);
                    if !e.0.contains(&n) {
                        e.0.push(n);
                    }
                }
            }
        }
        for ((start, kind), (props, detail, fix)) in moves {
            out.push((start as f64 / SEC as f64, shown, kind, props, detail, fix));
        }
        // Footage speed that jumps between rates.
        if let m::Speed::Curve(tr) = &c.speed {
            for w in tr.keys.windows(2) {
                let (Some(a), Some(b)) = (w[0].value.as_f64(), w[1].value.as_f64()) else { continue };
                if w[0].interp == m::Interp::Hold && (a - b).abs() > 0.25 && a > 0.05 && b > 0.05 {
                    let at = (offset + c.start + w[1].time) as f64 / SEC as f64;
                    out.push((at, shown, "speed_step", vec!["speed".into()], format!("jumps from {a:.2}x to {b:.2}x at {at:.2}s with no ramp: the picture lurches"), "ramp it: give the speed keys a curve (smooth, or power2_in_out) over at least 0.2 s; keep hold only for a freeze"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip_with(path: &str, keys: Vec<m::Keyframe>) -> m::Project {
        let mut p = m::Project::default();
        let mut c = m::Clip::new(m::ClipId(7), m::LaneId(1), 0, 4 * SEC, m::ClipSource::Generator(m::Generator::new(m::GeneratorKind::Solid)));
        c.keyframes.insert(path.into(), m::Track::new(keys));
        let active = p.active_sequence;
        p.sequences.iter_mut().find(|s| s.id == active).unwrap().clips.push(c);
        p
    }

    #[test]
    fn an_expo_in_fill_is_caught_and_a_good_one_is_not() {
        // The bar from the comparison video: scale_x 0 → 1 over 1 s, expo_in.
        let bad = clip_with("transform.scale_x", vec![m::Keyframe::with_curve(0, 0.0, m::Curve::Expo { end: m::CurveEnd::In }), m::Keyframe::new(SEC, 1.0, m::Interp::Linear)]);
        let p = problems(&bad, 30.0);
        assert_eq!(p.len(), 1, "{:?}", p.iter().map(|x| &x.problem).collect::<Vec<_>>());
        assert!(matches!(p[0].kind, "motion_snap" | "motion_stall"));
        for good in [m::Curve::Expo { end: m::CurveEnd::Out }, m::Curve::Power { power: 3.0, end: m::CurveEnd::Out }, m::Curve::Back { overshoot: 1.7, end: m::CurveEnd::Out }, m::Curve::Sine { end: m::CurveEnd::InOut }] {
            let ok = clip_with("transform.scale_x", vec![m::Keyframe::with_curve(0, 0.0, good), m::Keyframe::new(SEC, 1.0, m::Interp::Linear)]);
            assert!(problems(&ok, 30.0).is_empty(), "{good:?}");
        }
        // A smooth move and a short hit are fine.
        let smooth = clip_with("transform.position", vec![m::Keyframe::new(0, m::Vec2::ZERO, m::Interp::Auto), m::Keyframe::new(SEC, m::Vec2::new(400.0, 0.0), m::Interp::Linear)]);
        assert!(problems(&smooth, 30.0).is_empty());
        let hit = clip_with("transform.scale_x", vec![m::Keyframe::with_curve(0, 1.0, m::Curve::Expo { end: m::CurveEnd::In }), m::Keyframe::new(SEC / 6, 1.3, m::Interp::Linear)]);
        assert!(problems(&hit, 30.0).is_empty(), "under 8 frames is a hit");
    }

    #[test]
    fn speed_jumps_are_caught() {
        let mut p = m::Project::default();
        let mut c = m::Clip::new(m::ClipId(3), m::LaneId(1), 0, 4 * SEC, m::ClipSource::Generator(m::Generator::new(m::GeneratorKind::Solid)));
        c.speed = m::Speed::Curve(m::Track::new(vec![m::Keyframe::new(0, 1.0, m::Interp::Hold), m::Keyframe::new(SEC, 0.3, m::Interp::Hold), m::Keyframe::new(2 * SEC, 0.0, m::Interp::Hold)]));
        let active = p.active_sequence;
        p.sequences.iter_mut().find(|s| s.id == active).unwrap().clips.push(c);
        let found = problems(&p, 30.0);
        assert_eq!(found.len(), 1, "1 → 0.3 lurches; dropping to 0 is a freeze");
        assert_eq!(found[0].kind, "speed_step");
    }
}
