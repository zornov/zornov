// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Effects and transitions added while Studio runs (plugins).
//!
//! A plugin host registers an [`EffectDef`] or [`TransitionDef`] whose work
//! is done by an [`EffectRun`] / [`TransitionRun`] object instead of a
//! built-in function. Registered items are found by the same
//! [`crate::effects::find`] and [`crate::transitions::find`] as the
//! built-in catalogue, so the renderer (CPU and GPU), the inspector and the
//! agent need no special case. Built-in ids always win over registered ones.
//!
//! Definitions are leaked to give them the `'static` lifetime the rest of
//! the crate expects; a host should register a given plugin build once and
//! keep the returned reference to re-register it after a disable.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};

use crate::effects::{EffectContext, EffectDef};
use crate::image::Image;
use crate::params::{ParamDef, ParamSet};
use crate::transitions::{TransitionContext, TransitionDef};

/// The work of a registered effect.
pub trait EffectRun: Send + Sync {
    /// Same contract as a built-in effect: the output has the input's size.
    fn run(&self, ctx: &EffectContext, input: &Image, params: &ParamSet) -> Image;
}

/// The work of a registered transition.
pub trait TransitionRun: Send + Sync {
    /// `a` and `b` have the same size; `progress` is strictly inside 0..1.
    fn run(&self, ctx: &TransitionContext, a: &Image, b: &Image, progress: f32, params: &ParamSet) -> Image;
}

static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Goes up whenever something is registered or removed, so catalogues
/// built from [`effects`] and [`transitions`] know when to rebuild.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

fn bump() {
    GENERATION.fetch_add(1, Ordering::AcqRel);
}

fn effects_reg() -> &'static RwLock<Vec<&'static EffectDef>> {
    static R: OnceLock<RwLock<Vec<&'static EffectDef>>> = OnceLock::new();
    R.get_or_init(|| RwLock::new(Vec::new()))
}

fn transitions_reg() -> &'static RwLock<Vec<&'static TransitionDef>> {
    static R: OnceLock<RwLock<Vec<&'static TransitionDef>>> = OnceLock::new();
    R.get_or_init(|| RwLock::new(Vec::new()))
}

/// Leaks a parameter list so a definition built at run time can use it.
pub fn leak_params(params: Vec<ParamDef>) -> &'static [ParamDef] {
    Box::leak(params.into_boxed_slice())
}

/// Leaks a string (ids, names, labels, WGSL built at run time).
pub fn leak_str(s: &str) -> &'static str {
    Box::leak(s.to_owned().into_boxed_str())
}

/// Leaks a definition for [`register_effect`].
pub fn leak_effect(def: EffectDef) -> &'static EffectDef {
    Box::leak(Box::new(def))
}

/// Leaks a definition for [`register_transition`].
pub fn leak_transition(def: TransitionDef) -> &'static TransitionDef {
    Box::leak(Box::new(def))
}

/// Makes an effect findable, replacing a registered one with the same id.
/// Fails when the id belongs to a built-in effect or the parameter list is
/// malformed.
pub fn register_effect(def: &'static EffectDef) -> Result<(), String> {
    if crate::effects::all().iter().any(|e| e.id == def.id) {
        return Err(format!("`{}` is a built-in effect", def.id));
    }
    crate::params::validate_defs(def.params).map_err(|e| format!("effect `{}`: {e}", def.id))?;
    let mut r = effects_reg().write().unwrap_or_else(|e| e.into_inner());
    r.retain(|e| e.id != def.id);
    r.push(def);
    bump();
    Ok(())
}

/// Removes a registered effect. Clips that use it then skip it, as they do
/// any unknown effect.
pub fn unregister_effect(id: &str) {
    effects_reg().write().unwrap_or_else(|e| e.into_inner()).retain(|e| e.id != id);
    bump();
}

/// Makes a transition findable; see [`register_effect`].
pub fn register_transition(def: &'static TransitionDef) -> Result<(), String> {
    if crate::transitions::all().iter().any(|t| t.id == def.id) {
        return Err(format!("`{}` is a built-in transition", def.id));
    }
    crate::params::validate_defs(def.params).map_err(|e| format!("transition `{}`: {e}", def.id))?;
    let mut r = transitions_reg().write().unwrap_or_else(|e| e.into_inner());
    r.retain(|t| t.id != def.id);
    r.push(def);
    bump();
    Ok(())
}

pub fn unregister_transition(id: &str) {
    transitions_reg().write().unwrap_or_else(|e| e.into_inner()).retain(|t| t.id != id);
    bump();
}

/// Registered effects, in registration order.
pub fn effects() -> Vec<&'static EffectDef> {
    effects_reg().read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Registered transitions, in registration order.
pub fn transitions() -> Vec<&'static TransitionDef> {
    transitions_reg().read().unwrap_or_else(|e| e.into_inner()).clone()
}

pub(crate) fn find_effect(id: &str) -> Option<&'static EffectDef> {
    effects_reg().read().unwrap_or_else(|e| e.into_inner()).iter().copied().find(|e| e.id == id)
}

pub(crate) fn find_transition(id: &str) -> Option<&'static TransitionDef> {
    transitions_reg().read().unwrap_or_else(|e| e.into_inner()).iter().copied().find(|t| t.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{self, Category};
    use crate::params::ParamValue;

    struct Invert;
    impl EffectRun for Invert {
        fn run(&self, _: &EffectContext, input: &Image, p: &ParamSet) -> Image {
            let k = p.f("amount");
            let mut out = input.clone();
            for px in &mut out.pixels {
                for c in 0..3 {
                    px[c] = px[c] + (px[3] - 2.0 * px[c]) * k;
                }
            }
            out
        }
    }

    struct Cut;
    impl TransitionRun for Cut {
        fn run(&self, _: &TransitionContext, a: &Image, b: &Image, t: f32, _: &ParamSet) -> Image {
            if t < 0.5 { a.clone() } else { b.clone() }
        }
    }

    #[test]
    fn registered_effects_are_found_and_run() {
        static RUN: Invert = Invert;
        let params = leak_params(vec![ParamDef::float("amount", "Amount", 0.0, 1.0, 1.0)]);
        let def = leak_effect(EffectDef::external("test:invert", "Invert", Category::Stylise, "Test.", params, None, false, &RUN));
        register_effect(def).unwrap();
        let found = effects::find("test:invert").expect("registered");
        let img = Image::filled(4, 2, [0.25, 0.5, 1.0, 1.0]);
        let out = found.apply(&EffectContext::still(), &img, &found.defaults().with("amount", ParamValue::Float(1.0)));
        assert!((out.pixels[0][0] - 0.75).abs() < 1e-6);
        unregister_effect("test:invert");
        assert!(effects::find("test:invert").is_none());
        // Built-in ids cannot be taken over.
        let clash = leak_effect(EffectDef::external("posterise", "X", Category::Stylise, "", params, None, false, &RUN));
        assert!(register_effect(clash).is_err());
    }

    #[test]
    fn registered_transitions_are_found_and_run() {
        static RUN: Cut = Cut;
        let def = leak_transition(TransitionDef::external("test:cut", "Cut", "Test.", &[], None, &RUN));
        register_transition(def).unwrap();
        let t = crate::transitions::find("test:cut").unwrap();
        let a = Image::filled(2, 2, [1.0, 0.0, 0.0, 1.0]);
        let b = Image::filled(2, 2, [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(t.apply(&TransitionContext::default(), &a, &b, 0.7, &t.defaults()).pixels[0], [0.0, 1.0, 0.0, 1.0]);
        unregister_transition("test:cut");
    }
}
