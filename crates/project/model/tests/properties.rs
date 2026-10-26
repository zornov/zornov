// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Property tests: random edit sequences keep every invariant, failed
//! edits change nothing, and undo/redo walk the whole history exactly.

mod common;
use common::*;
use proptest::prelude::*;
use studio_model::*;

/// An edit described by small numbers, resolved against the project at
/// the time it runs (clip choices are indices into the current clips).
#[derive(Clone, Debug)]
enum Op {
    Insert { lane: u8, at: u16, dur: u16, src: u16, overwrite: bool },
    Split { at: u16 },
    Trim { clip: u8, end: bool, delta: i16, ripple: bool },
    RippleDelete { clip: u8 },
    Lift { clip: u8 },
    Move { clip: u8, delta: i16, lane: Option<u8> },
    Reorder { clip: u8, before: u8 },
    Slip { clip: u8, delta: i16 },
    Slide { clip: u8, delta: i16 },
    Roll { clip: u8, delta: i16 },
    Duplicate { clip: u8 },
    Speed { clip: u8, which: u8 },
    Reverse { clip: u8 },
    Transition { clip: u8, dur: u16, align: u8 },
    Detach { clip: u8 },
    Freeze { clip: u8, at: u16, dur: u16 },
    Compound { a: u8, b: u8 },
    Expand { clip: u8 },
    Link { a: u8, b: u8 },
    Keyframe { clip: u8, at: u16, v: u8 },
    Undo,
    Redo,
}

/// Times are tenths of a second.
const T: Micros = S / 10;

fn op() -> impl Strategy<Value = Op> {
    let c = any::<u8>();
    prop_oneof![
        (0u8..4, 0u16..120, 1u16..40, 0u16..60, any::<bool>()).prop_map(|(lane, at, dur, src, overwrite)| Op::Insert { lane, at, dur, src, overwrite }),
        (0u16..120).prop_map(|at| Op::Split { at }),
        (c, any::<bool>(), -30i16..30, any::<bool>()).prop_map(|(clip, end, delta, ripple)| Op::Trim { clip, end, delta, ripple }),
        c.prop_map(|clip| Op::RippleDelete { clip }),
        c.prop_map(|clip| Op::Lift { clip }),
        (c, -40i16..40, proptest::option::of(0u8..4)).prop_map(|(clip, delta, lane)| Op::Move { clip, delta, lane }),
        (c, c).prop_map(|(clip, before)| Op::Reorder { clip, before }),
        (c, -30i16..30).prop_map(|(clip, delta)| Op::Slip { clip, delta }),
        (c, -20i16..20).prop_map(|(clip, delta)| Op::Slide { clip, delta }),
        (c, -20i16..20).prop_map(|(clip, delta)| Op::Roll { clip, delta }),
        c.prop_map(|clip| Op::Duplicate { clip }),
        (c, 0u8..5).prop_map(|(clip, which)| Op::Speed { clip, which }),
        c.prop_map(|clip| Op::Reverse { clip }),
        (c, 1u16..20, 0u8..3).prop_map(|(clip, dur, align)| Op::Transition { clip, dur, align }),
        c.prop_map(|clip| Op::Detach { clip }),
        (c, 0u16..120, 1u16..20).prop_map(|(clip, at, dur)| Op::Freeze { clip, at, dur }),
        (c, c).prop_map(|(a, b)| Op::Compound { a, b }),
        c.prop_map(|clip| Op::Expand { clip }),
        (c, c).prop_map(|(a, b)| Op::Link { a, b }),
        (c, 0u16..40, any::<u8>()).prop_map(|(clip, at, v)| Op::Keyframe { clip, at, v }),
        Just(Op::Undo),
        Just(Op::Redo),
    ]
}

fn pick(p: &Project, i: u8) -> Option<ClipId> {
    let clips = &p.active().clips;
    if clips.is_empty() { None } else { Some(clips[i as usize % clips.len()].id) }
}

fn lane(p: &Project, i: u8) -> LaneId {
    let lanes = &p.active().lanes;
    lanes[i as usize % lanes.len()].id
}

/// Builds the edit for `op`, or `None` when it does not apply.
fn resolve(f: &Fx, p: &Project, op: &Op) -> Option<Edit> {
    let seq = p.active_sequence;
    Some(match *op {
        Op::Insert { lane: l, at, dur, src, overwrite } => {
            let media = [f.video, f.music, f.still][src as usize % 3];
            let clip = Box::new(Clip::media(ClipId(0), LaneId(0), 0, media, src as Micros * T / 10, dur as Micros * T));
            let lane = Some(lane(p, l));
            let at = at as Micros * T;
            if overwrite { Edit::Overwrite { seq, lane, at, clip } } else { Edit::Insert { seq, lane, at, clip } }
        }
        Op::Split { at } => Edit::Split { seq, at: at as Micros * T, clips: None },
        Op::Trim { clip, end, delta, ripple } => Edit::Trim { seq, clip: pick(p, clip)?, edge: if end { Edge::End } else { Edge::Start }, delta: delta as Micros * T, ripple },
        Op::RippleDelete { clip } => Edit::RippleDelete { seq, clips: vec![pick(p, clip)?] },
        Op::Lift { clip } => Edit::Lift { seq, clips: vec![pick(p, clip)?] },
        Op::Move { clip, delta, lane: l } => Edit::Move { seq, clips: vec![pick(p, clip)?], delta: delta as Micros * T, to_lane: l.map(|l| lane(p, l)) },
        Op::Reorder { clip, before } => Edit::Reorder { seq, clip: pick(p, clip)?, before: Some(pick(p, before)?) },
        Op::Slip { clip, delta } => Edit::Slip { seq, clip: pick(p, clip)?, delta: delta as Micros * T },
        Op::Slide { clip, delta } => Edit::Slide { seq, clip: pick(p, clip)?, delta: delta as Micros * T },
        Op::Roll { clip, delta } => Edit::Roll { seq, left: pick(p, clip)?, delta: delta as Micros * T },
        Op::Duplicate { clip } => Edit::Duplicate { seq, clips: vec![pick(p, clip)?] },
        Op::Speed { clip, which } => {
            let speed = match which {
                0 => Speed::Constant(0.5),
                1 => Speed::Constant(2.0),
                2 => Speed::Constant(1.0),
                3 => Speed::Constant(0.0),
                _ => Speed::Curve(Track::new(vec![Keyframe::new(0, 0.5, Interp::Ease), Keyframe::new(S, 2.0, Interp::Linear)])),
            };
            Edit::SetSpeed { seq, clip: pick(p, clip)?, speed }
        }
        Op::Reverse { clip } => Edit::Reverse { seq, clips: vec![pick(p, clip)?] },
        Op::Transition { clip, dur, align } => {
            let align = [TransitionAlign::Centre, TransitionAlign::EndAtCut, TransitionAlign::StartAtCut][align as usize];
            Edit::AddTransition {
                seq,
                transition: Transition { id: TransitionId(0), left: pick(p, clip)?, right: ClipId(0), kind: "dissolve".into(), duration: dur as Micros * T, align, params: Default::default(), extra: Default::default() },
            }
        }
        Op::Detach { clip } => Edit::DetachAudio { seq, clip: pick(p, clip)?, new_clip: ClipId(0) },
        Op::Freeze { clip, at, dur } => Edit::FreezeFrame { seq, clip: pick(p, clip)?, at: at as Micros * T, duration: dur as Micros * T, new_clip: ClipId(0) },
        Op::Compound { a, b } => Edit::CreateCompound { seq, clips: vec![pick(p, a)?, pick(p, b)?], name: "c".into(), new_sequence: SequenceId(0), new_clip: ClipId(0) },
        Op::Expand { clip } => Edit::ExpandCompound { seq, clip: pick(p, clip)? },
        Op::Link { a, b } => Edit::Link { seq, clips: vec![pick(p, a)?, pick(p, b)?], link: LinkId(0) },
        Op::Keyframe { clip, at, v } => Edit::SetKeyframe { seq, clip: pick(p, clip)?, property: "transform.opacity".into(), key: Keyframe::new(at as Micros * T, v as f64 / 255.0, Interp::Auto) },
        Op::Undo | Op::Redo => return None,
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 300, ..ProptestConfig::default() })]

    #[test]
    fn random_edits_keep_invariants_and_undo_exactly(ops in proptest::collection::vec(op(), 1..40)) {
        let f = fixture();
        let mut p = f.p.clone();
        let initial = p.clone();
        let mut h = History::new();
        for op in &ops {
            let before = p.clone();
            let result = match op {
                Op::Undo => h.undo(&mut p).map(|_| ()),
                Op::Redo => h.redo(&mut p).map(|_| ()),
                other => match resolve(&f, &p, other) {
                    Some(edit) => h.apply(&mut p, format!("{other:?}"), edit),
                    None => Ok(()),
                },
            };
            match result {
                Ok(()) => prop_assert!(p.check_invariants().is_ok(), "{op:?} broke {:?}", p.check_invariants()),
                // Ids taken from the counter are not handed back, by design.
                Err(_) => prop_assert!(p.content_eq(&before), "failed {:?} changed the project", op),
            }
        }
        let end = p.clone();
        let mut undone = 0;
        while h.can_undo() {
            h.undo(&mut p).expect("undo of a recorded step");
            prop_assert!(p.check_invariants().is_ok());
            undone += 1;
        }
        prop_assert!(p.content_eq(&initial), "undoing everything did not restore the start");
        for _ in 0..undone {
            h.redo(&mut p).expect("redo of a recorded step");
        }
        prop_assert!(p.content_eq(&end), "redoing everything did not restore the end");

        // Whatever the edits produced also survives the file format.
        let folder = studio_model::file::ProjectFolder::new("/nonexistent-project");
        let doc = folder.document(&p, &Extra::new());
        let text = studio_model::file::to_json(&doc);
        let back = studio_model::file::from_json(&text).expect("reload");
        prop_assert!(back.project.content_eq(&doc.project));
    }

    #[test]
    fn keyframe_tracks_stay_within_auto_bounds(vals in proptest::collection::vec(-100.0f64..100.0, 2..8), probe in 0u32..1000) {
        let keys = vals.iter().enumerate().map(|(i, v)| Keyframe::new(i as i64 * 1000, *v, Interp::Auto)).collect();
        let tr = Track::new(keys);
        let t = probe as i64 * (vals.len() as i64 - 1);
        let i = (t / 1000) as usize;
        let v = tr.eval(t).unwrap().as_f64().unwrap();
        let (a, b) = (vals[i.min(vals.len() - 1)], vals[(i + 1).min(vals.len() - 1)]);
        prop_assert!(v >= a.min(b) - 1e-9 && v <= a.max(b) + 1e-9);
    }
}
