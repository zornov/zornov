// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Motion presets, easing and text stagger.

use studio_fx::motion::{self, apply_pose, apply_poses, DEFAULT_SHUTTER, split_text, stagger, text_poses, MotionKind, Phase, Pose, StaggerOrder, TextUnit};
use studio_fx::params::{validate_defs, ParamKind, ParamValue};
use studio_fx::testcard::test_card;

#[test]
fn catalogue_is_well_formed() {
    let all = motion::all();
    assert!(all.iter().filter(|m| m.kind == MotionKind::InOut).count() >= 15);
    assert!(all.iter().filter(|m| m.kind == MotionKind::Loop).count() >= 8);
    for (i, m) in all.iter().enumerate() {
        assert!(all[..i].iter().all(|o| o.id != m.id), "duplicate {}", m.id);
        validate_defs(m.params).unwrap_or_else(|e| panic!("{}: {e}", m.id));
        if m.kind == MotionKind::Loop {
            assert!(m.params.iter().any(|d| d.id == "period"), "{} has no period", m.id);
        }
    }
}

#[test]
fn entries_end_and_exits_start_at_rest() {
    for m in motion::all().iter().filter(|m| m.kind == MotionKind::InOut) {
        let p = m.defaults();
        assert_eq!(m.pose(Phase::Entry, 1.0, &p), Pose::REST, "{}", m.id);
        assert_eq!(m.pose(Phase::Exit, 0.0, &p), Pose::REST, "{}", m.id);
        // And they actually move.
        assert!(!m.pose(Phase::Entry, 0.0, &p).is_rest(1e-3), "{} does nothing", m.id);
        // Exit is the entry played backwards.
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            assert_eq!(m.pose(Phase::Exit, t, &p), m.pose(Phase::Entry, 1.0 - t, &p), "{}", m.id);
        }
    }
}

#[test]
fn loops_rest_at_zero_amount() {
    for m in motion::all().iter().filter(|m| m.kind == MotionKind::Loop) {
        let id = if m.params.iter().any(|d| d.id == "amount") { "amount" } else { "turns" };
        let p = m.defaults().with(id, ParamValue::Float(0.0));
        for i in 0..50 {
            let pose = m.loop_pose(i as f32 * 0.173, &p);
            assert!(pose.is_rest(1e-6), "{} at {i}: {pose:?}", m.id);
        }
        let moving = (0..50).any(|i| !m.loop_pose(i as f32 * 0.173, &m.defaults()).is_rest(1e-4));
        assert!(moving, "{} never moves", m.id);
    }
}

#[test]
fn finite_at_extremes() {
    for m in motion::all() {
        for d in m.params {
            let vals: Vec<ParamValue> = match d.kind {
                ParamKind::Float { min, max } => vec![ParamValue::Float(min), ParamValue::Float(max)],
                ParamKind::Int { min, max } => vec![ParamValue::Int(min), ParamValue::Int(max)],
                ParamKind::Bool => vec![ParamValue::Bool(false), ParamValue::Bool(true)],
                ParamKind::Choice(o) => (0..o.len() as u32).map(ParamValue::Choice).collect(),
                _ => vec![],
            };
            for v in vals {
                let p = m.defaults().with(d.id, v);
                for i in 0..=20 {
                    let t = i as f32 / 20.0;
                    let pose = match m.kind {
                        MotionKind::InOut => m.pose(Phase::Entry, t, &p),
                        MotionKind::Loop => m.loop_pose(t * 1000.0, &p),
                    };
                    assert!(pose.is_finite(), "{} {}={v:?} at {t}", m.id, d.id);
                    let img = apply_pose(&test_card(16, 9, 0.0), &pose);
                    assert!(img.is_finite(), "{}", m.id);
                }
            }
        }
    }
}

#[test]
fn apply_pose_rest_is_identity() {
    let card = test_card(32, 18, 0.0);
    assert_eq!(apply_pose(&card, &Pose::REST), card);
    let shifted = apply_pose(&card, &Pose { offset: [0.25, 0.0], ..Pose::REST });
    assert_eq!(shifted.get(8 + 2, 5), card.get(2, 5));
    assert_eq!(shifted.get(2, 5), [0.0; 4]);
}

#[test]
fn stagger_spans_the_whole_motion() {
    for order in [StaggerOrder::Forward, StaggerOrder::Backward, StaggerOrder::Centre, StaggerOrder::Random(3)] {
        for overlap in [0.0, 0.5, 1.0] {
            let n = 7;
            let starts: Vec<f32> = (0..n).map(|i| stagger(0.0, i, n, overlap, order)).collect();
            let ends: Vec<f32> = (0..n).map(|i| stagger(1.0, i, n, overlap, order)).collect();
            assert!(starts.iter().all(|&s| s == 0.0));
            assert!(ends.iter().all(|&e| e == 1.0), "{order:?} {overlap}: {ends:?}");
            for i in 0..n {
                let mut last = 0.0;
                for k in 0..=50 {
                    let v = stagger(k as f32 / 50.0, i, n, overlap, order);
                    assert!(v >= last);
                    last = v;
                }
            }
        }
    }
    // Forward with no overlap: the second unit waits for the first.
    assert_eq!(stagger(0.3, 1, 3, 0.0, StaggerOrder::Forward), 0.0);
    assert!(stagger(0.5, 1, 3, 0.0, StaggerOrder::Forward) > 0.0);
}

#[test]
fn text_units() {
    let text = "Hi  there\nnew line\n\n end";
    assert_eq!(split_text(text, TextUnit::Character).len(), 17);
    let words: Vec<&str> = split_text(text, TextUnit::Word).into_iter().map(|r| &text[r]).collect();
    assert_eq!(words, ["Hi", "there", "new", "line", "end"]);
    let lines: Vec<&str> = split_text(text, TextUnit::Line).into_iter().map(|r| &text[r]).collect();
    assert_eq!(lines, ["Hi  there", "new line", " end"]);
    let rise = motion::find("rise").unwrap();
    let poses = text_poses(rise, Phase::Entry, 0.5, &rise.defaults(), 5, 0.3, StaggerOrder::Forward);
    assert_eq!(poses.len(), 5);
    assert!(poses[0].offset[1] < poses[4].offset[1], "first word should be further along");
}

#[test]
fn shutter_motion_blur() {
    let card = test_card(48, 27, 0.0);
    let rise = motion::find("rise").unwrap();
    let p = rise.defaults();
    // At rest every sample is the rest pose, so blur changes nothing.
    let rest = rise.shutter_poses(Phase::Entry, 1.0, 0.5, 30.0, DEFAULT_SHUTTER, 8, &p);
    assert_eq!(rest.len(), 8);
    assert_eq!(apply_poses(&card, &rest), card);
    // Mid-move the samples spread along the path, more for a faster move.
    let spread = |dur: f32| {
        let poses = rise.shutter_poses(Phase::Entry, 0.2, dur, 30.0, DEFAULT_SHUTTER, 8, &p);
        poses.last().unwrap().offset[1] - poses[0].offset[1]
    };
    assert!(spread(0.25).abs() > spread(1.0).abs() && spread(1.0).abs() > 0.0);
    assert_eq!(rise.shutter_poses(Phase::Entry, 0.2, 0.5, 30.0, 0.0, 8, &p).len(), 1);
    let blurred = apply_poses(&card, &rise.shutter_poses(Phase::Entry, 0.2, 0.3, 30.0, DEFAULT_SHUTTER, 8, &p));
    assert!(blurred.is_finite());
    let sway = motion::find("sway").unwrap();
    assert_eq!(sway.loop_shutter_poses(1.0, 30.0, 180.0, 5, &sway.defaults()).len(), 5);
}
