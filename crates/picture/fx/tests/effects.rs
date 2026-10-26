// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Effect catalogue tests: identity at zero strength, no NaN/Inf at
//! extreme parameters, golden images at defaults.

mod common;

use common::*;
use studio_fx::effects::{self, EffectContext, EffectDef};
use studio_fx::params::{validate_defs, ParamKind, ParamSet, ParamValue};
use studio_fx::testcard::test_card;
use studio_fx::Image;

fn ctx<'a>(history: &'a [Image], plate: &'a Image, time: f32) -> EffectContext<'a> {
    EffectContext { time, frame_rate: 30.0, seed: 1, history, aux: Some(plate) }
}

#[test]
fn catalogue_is_well_formed() {
    let all = effects::all();
    assert!(all.len() >= 50, "only {} effects", all.len());
    for (i, e) in all.iter().enumerate() {
        assert!(all[..i].iter().all(|o| o.id != e.id), "duplicate id {}", e.id);
        validate_defs(e.params).unwrap_or_else(|m| panic!("{}: {m}", e.id));
        assert!(!e.name.is_empty() && !e.description.is_empty(), "{}", e.id);
        if let Some((id, v)) = e.identity {
            let def = e.params.iter().find(|d| d.id == id).unwrap_or_else(|| panic!("{}: identity param {id}", e.id));
            let (lo, hi) = def.range().unwrap();
            assert!((lo..=hi).contains(&v), "{}: identity value out of range", e.id);
        }
        assert!(std::ptr::eq(effects::find(e.id).unwrap(), *e));
    }
}

#[test]
fn identity_at_zero_strength() {
    let (w, h) = (64, 36);
    let card = test_card(w, h, 0.0);
    let hist = history(w, h, 40);
    let plate = test_card(w, h, 0.4);
    let mut checked = 0;
    for e in effects::all() {
        let Some((id, v)) = e.identity else { continue };
        let p = e.defaults().with(id, ParamValue::Float(v));
        for time in [0.0, 0.77, 5.3] {
            let out = e.apply(&ctx(&hist, &plate, time), &card, &p);
            let d = out.max_abs_diff(&card);
            assert!(d < 1e-6, "{} with {id} = {v} at t={time} differs by {d}", e.id);
        }
        checked += 1;
    }
    // Every effect except pure generators of time has an identity setting.
    assert_eq!(checked, effects::all().len(), "some effects have no identity setting");
}

/// Every value worth trying for one parameter.
fn extremes(kind: ParamKind) -> Vec<ParamValue> {
    match kind {
        ParamKind::Float { min, max } => vec![ParamValue::Float(min), ParamValue::Float(max)],
        ParamKind::Int { min, max } => vec![ParamValue::Int(min), ParamValue::Int(max)],
        ParamKind::Bool => vec![ParamValue::Bool(false), ParamValue::Bool(true)],
        ParamKind::Choice(opts) => (0..opts.len() as u32).map(ParamValue::Choice).collect(),
        ParamKind::Colour => vec![ParamValue::Colour([0.0; 4]), ParamValue::Colour([64.0, 64.0, 64.0, 1.0])],
        ParamKind::Point => vec![ParamValue::Point([-1.0, -1.0]), ParamValue::Point([2.0, 2.0]), ParamValue::Point([0.5, 0.5])],
    }
}

fn all_at(e: &EffectDef, pick_max: bool) -> ParamSet {
    let mut p = e.defaults();
    for d in e.params {
        let v = extremes(d.kind);
        let v = if pick_max { *v.last().unwrap() } else { v[0] };
        p.set(d.id, v).unwrap();
    }
    p
}

#[test]
fn finite_at_extreme_parameters() {
    let (w, h) = (40, 24);
    let card = test_card(w, h, 0.0);
    let hist = history(w, h, 60);
    let plate = test_card(w, h, 0.3);
    for e in effects::all() {
        let mut sets = vec![all_at(e, false), all_at(e, true)];
        for d in e.params {
            for v in extremes(d.kind) {
                sets.push(e.defaults().with(d.id, v));
            }
        }
        for (i, p) in sets.iter().enumerate() {
            for time in [0.0, 123.456] {
                let c = ctx(&hist, &plate, time);
                let out = e.apply(&c, &card, p);
                assert!(out.is_finite(), "{} set {i} at t={time}", e.id);
                // Also without history or plate, as at the start of a clip.
                let bare = EffectContext { time, frame_rate: 24.0, seed: 9, history: &[], aux: None };
                assert!(e.apply(&bare, &card, p).is_finite(), "{} set {i} bare", e.id);
            }
        }
    }
}

#[test]
fn tiny_and_hdr_inputs() {
    let hdr = Image::from_fn(9, 7, |x, y| {
        let v = if (x + y) % 3 == 0 { 50.0 } else { 0.0 };
        [v, v * 0.5, 0.0, if x == 0 { 0.0 } else { 1.0 }]
    });
    let inputs = [Image::filled(1, 1, [0.5, 0.4, 0.3, 1.0]), Image::new(3, 2), hdr];
    for e in effects::all() {
        for img in &inputs {
            let out = e.apply(&EffectContext::still(), img, &e.defaults());
            assert_eq!((out.width, out.height), (img.width, img.height), "{}", e.id);
            assert!(out.is_finite(), "{} on {}x{}", e.id, img.width, img.height);
        }
    }
}

#[test]
fn golden_images_at_defaults() {
    let (w, h) = (GOLDEN_W, GOLDEN_H);
    let card = test_card(w, h, 0.0);
    let hist = history(w, h, 40);
    let plate = test_card(w, h, 0.5);
    let mut failures = Vec::new();
    for e in effects::all() {
        let out = e.apply(&ctx(&hist, &plate, 1.3), &card, &e.defaults());
        if let Err(m) = check_golden(&format!("effects/{}", e.id), &out) {
            failures.push(m);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn effects_are_deterministic() {
    let card = test_card(32, 18, 0.0);
    let hist = history(32, 18, 10);
    let plate = test_card(32, 18, 0.2);
    for e in effects::all() {
        let c = ctx(&hist, &plate, 2.5);
        let a = e.apply(&c, &card, &e.defaults());
        let b = e.apply(&c, &card, &e.defaults());
        assert_eq!(a, b, "{}", e.id);
    }
}

#[test]
fn history_requests_are_bounded() {
    for e in effects::all() {
        let n = e.history_needed(&all_at(e, true), 60.0);
        assert!(n <= 160, "{} wants {n} frames", e.id);
    }
    let echo = effects::find("echo").unwrap();
    assert_eq!(echo.history_needed(&echo.defaults(), 30.0), 8);
}

#[test]
fn bars_tone_level() {
    let bars = effects::find("bars").unwrap();
    let s = effects::generate::bars_tone(&bars.defaults(), 48_000, 0, 48_000);
    let peak = s.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    // -20 dBFS sine peaks at 0.1.
    assert!((peak - 0.1).abs() < 1e-3, "{peak}");
    let crossings = s.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
    assert!((999..=1001).contains(&crossings), "{crossings}");
}

#[test]
fn chroma_key_removes_screen() {
    let e = effects::find("chroma_key").unwrap();
    let green = [0.05f32, 0.55, 0.1];
    let img = Image::from_fn(8, 4, |x, _| if x < 4 { [green[0], green[1], green[2], 1.0] } else { [0.6, 0.3, 0.2, 1.0] });
    let p = e.defaults().with("feather", ParamValue::Float(0.0));
    let out = e.apply(&EffectContext::still(), &img, &p);
    assert!(out.get(0, 0)[3] < 0.01, "screen kept: {:?}", out.get(0, 0));
    assert!(out.get(7, 0)[3] > 0.99, "subject removed: {:?}", out.get(7, 0));
}

#[test]
fn still_effects_ignore_time_and_seed() {
    // Renderers keep the result of an effect that is not `animated` while
    // its input and parameters stay the same, so it must not read the
    // clip time, frame rate or seed.
    let card = test_card(48, 27, 0.0);
    let at = |time: f32, frame_rate: f32, seed: u32| EffectContext { time, frame_rate, seed, history: &[], aux: None };
    let mut animated = Vec::new();
    for e in effects::all() {
        if e.animated {
            animated.push(e.id);
            continue;
        }
        for p in [e.defaults(), all_at(e, true), all_at(e, false)] {
            let a = e.apply(&at(0.0, 30.0, 1), &card, &p);
            for c in [at(1.37, 30.0, 1), at(4.2, 24.0, 1), at(0.0, 30.0, 0xdead_beef)] {
                assert_eq!(a, e.apply(&c, &card, &p), "{} changes with the clip time, frame rate or seed: mark it `.animated()`", e.id);
            }
        }
    }
    assert!(animated.contains(&"wave") && animated.contains(&"film"), "{animated:?}");
}
