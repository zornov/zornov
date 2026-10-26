// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Transition tests: exact first and last frames, continuity just inside
//! the ends, no NaN/Inf at extreme parameters, golden images mid-way.

mod common;

use common::*;
use studio_fx::effects::{self, EffectContext};
use studio_fx::params::{validate_defs, ParamKind, ParamValue};
use studio_fx::testcard::test_card;
use studio_fx::transitions::{self, TransitionContext};
use studio_fx::Image;

fn inputs(w: usize, h: usize) -> (Image, Image) {
    let a = test_card(w, h, 0.0);
    let grad = effects::find("gradient").unwrap();
    let b = grad.apply(&EffectContext::still(), &Image::new(w, h), &grad.defaults());
    (a, b)
}

#[test]
fn catalogue_is_well_formed() {
    let all = transitions::all();
    assert!(all.len() >= 20);
    for (i, t) in all.iter().enumerate() {
        assert!(all[..i].iter().all(|o| o.id != t.id), "duplicate {}", t.id);
        validate_defs(t.params).unwrap_or_else(|m| panic!("{}: {m}", t.id));
        assert!(transitions::find(t.id).is_some());
    }
}

#[test]
fn first_and_last_frames_are_the_inputs() {
    let (a, b) = inputs(48, 27);
    let ctx = TransitionContext { seed: 4 };
    for t in transitions::all() {
        let p = t.defaults();
        assert_eq!(t.apply(&ctx, &a, &b, 0.0, &p), a, "{} at 0", t.id);
        assert_eq!(t.apply(&ctx, &a, &b, 1.0, &p), b, "{} at 1", t.id);
        assert_eq!(t.apply(&ctx, &a, &b, -0.5, &p), a, "{} below 0", t.id);
        assert_eq!(t.apply(&ctx, &a, &b, 1.5, &p), b, "{} above 1", t.id);
        assert_eq!(t.apply(&ctx, &a, &b, f32::NAN, &p), a, "{} at NaN", t.id);
    }
}

#[test]
fn no_jump_just_inside_the_ends() {
    let (a, b) = inputs(64, 36);
    let ctx = TransitionContext { seed: 4 };
    for t in transitions::all() {
        let p = t.defaults();
        let start = mean_abs_diff(&t.apply(&ctx, &a, &b, 0.002, &p), &a);
        let end = mean_abs_diff(&t.apply(&ctx, &a, &b, 0.998, &p), &b);
        assert!(start < 0.01, "{} jumps at the start: {start}", t.id);
        assert!(end < 0.01, "{} jumps at the end: {end}", t.id);
    }
}

fn extremes(kind: ParamKind) -> Vec<ParamValue> {
    match kind {
        ParamKind::Float { min, max } => vec![ParamValue::Float(min), ParamValue::Float(max)],
        ParamKind::Int { min, max } => vec![ParamValue::Int(min), ParamValue::Int(max)],
        ParamKind::Bool => vec![ParamValue::Bool(false), ParamValue::Bool(true)],
        ParamKind::Choice(opts) => (0..opts.len() as u32).map(ParamValue::Choice).collect(),
        ParamKind::Colour => vec![ParamValue::Colour([0.0; 4]), ParamValue::Colour([64.0, 64.0, 64.0, 1.0])],
        ParamKind::Point => vec![ParamValue::Point([-1.0, -1.0]), ParamValue::Point([2.0, 2.0])],
    }
}

#[test]
fn finite_at_extreme_parameters() {
    let (a, b) = inputs(32, 18);
    let ctx = TransitionContext { seed: 1 };
    for t in transitions::all() {
        for d in t.params {
            for v in extremes(d.kind) {
                let p = t.defaults().with(d.id, v);
                for prog in [0.001, 0.25, 0.5, 0.75, 0.999] {
                    let out = t.apply(&ctx, &a, &b, prog, &p);
                    assert!(out.is_finite(), "{} {}={v:?} at {prog}", t.id, d.id);
                }
            }
        }
    }
}

#[test]
fn golden_images_mid_way() {
    let (a, b) = inputs(GOLDEN_W, GOLDEN_H);
    let ctx = TransitionContext { seed: 2 };
    let mut failures = Vec::new();
    for t in transitions::all() {
        let out = t.apply(&ctx, &a, &b, 0.45, &t.defaults());
        if let Err(m) = check_golden(&format!("transitions/{}", t.id), &out) {
            failures.push(m);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn dissolve_is_linear_light() {
    let a = Image::filled(2, 2, [0.0, 0.0, 0.0, 1.0]);
    let b = Image::filled(2, 2, [1.0, 1.0, 1.0, 1.0]);
    let t = transitions::find("dissolve").unwrap();
    let p = t.defaults().with("curve", ParamValue::Choice(0));
    let mid = t.apply(&TransitionContext::default(), &a, &b, 0.5, &p);
    assert!((mid.get(0, 0)[0] - 0.5).abs() < 1e-6);
}
