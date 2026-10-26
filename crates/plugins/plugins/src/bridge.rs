// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Puts a plugin's effects, generators and transitions into the `fx`
//! catalogue, so the renderer (CPU and GPU), the inspector, keyframes and
//! the agent use them like built-in ones under `<plugin>:<item>` ids.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use studio_fx::effects::{Category, EffectContext, EffectDef};
use studio_fx::external::{self, EffectRun, TransitionRun};
use studio_fx::image::Image;
use studio_fx::params::{ParamDef, ParamSet};
use studio_fx::transitions::{TransitionContext, TransitionDef};

use crate::manifest::{Item, Manifest, Param, qualified};
use crate::runtime::Runtime;

/// After this many failed calls a plugin's items stop running until the
/// plugin is loaded again, so one broken plugin cannot stall every frame.
pub const MAX_FAULTS: u32 = 8;

fn live() -> &'static RwLock<HashMap<String, Arc<Runtime>>> {
    static L: OnceLock<RwLock<HashMap<String, Arc<Runtime>>>> = OnceLock::new();
    L.get_or_init(Default::default)
}

/// The running instance of a plugin, if it is enabled.
pub fn runtime(plugin: &str) -> Option<Arc<Runtime>> {
    live().read().unwrap_or_else(|e| e.into_inner()).get(plugin).cloned()
}

struct PluginEffect {
    plugin: &'static str,
    export: &'static str,
    index: u32,
    cpu: bool,
}

impl EffectRun for PluginEffect {
    fn run(&self, ctx: &EffectContext, input: &Image, params: &ParamSet) -> Image {
        let Some(rt) = runtime(self.plugin).filter(|_| self.cpu) else { return input.clone() };
        if rt.faults.load(Ordering::Relaxed) >= MAX_FAULTS {
            return input.clone();
        }
        let mut out = input.clone();
        let c = [ctx.time.to_bits(), ctx.frame_rate.to_bits(), ctx.seed, 0];
        match rt.frame_call(self.export, self.index, c, &mut out.pixels, input.width as u32, input.height as u32, &params.pack_vec4()) {
            Ok(()) => out,
            Err(_) => input.clone(),
        }
    }
}

struct PluginTransition {
    plugin: &'static str,
    index: u32,
    cpu: bool,
}

impl TransitionRun for PluginTransition {
    fn run(&self, ctx: &TransitionContext, a: &Image, b: &Image, progress: f32, params: &ParamSet) -> Image {
        // Without CPU code (or with the plugin off) cut half-way.
        let fallback = || if progress < 0.5 { a.clone() } else { b.clone() };
        let Some(rt) = runtime(self.plugin).filter(|_| self.cpu) else { return fallback() };
        if rt.faults.load(Ordering::Relaxed) >= MAX_FAULTS {
            return fallback();
        }
        let mut out = a.clone();
        let c = [0, 0, ctx.seed, progress.to_bits()];
        match rt.transition_call(self.index, c, &mut out.pixels, &b.pixels, a.width as u32, a.height as u32, &params.pack_vec4()) {
            Ok(()) => out,
            Err(_) => fallback(),
        }
    }
}

fn s(v: &str) -> &'static str {
    external::leak_str(v)
}

fn num(v: &serde_json::Value, or: f64) -> f64 {
    v.as_f64().or_else(|| v.as_bool().map(|b| b as u8 as f64)).unwrap_or(or)
}

fn param_def(p: &Param) -> ParamDef {
    let (id, label) = (s(&p.id), s(&p.label));
    let lo = p.min.unwrap_or(0.0);
    let hi = p.max.unwrap_or(1.0).max(lo);
    let d = match p.kind.as_str() {
        "float" => ParamDef::float(id, label, lo as f32, hi as f32, num(&p.default, lo).clamp(lo, hi) as f32),
        "int" => ParamDef::int(id, label, lo as i32, hi as i32, num(&p.default, lo).clamp(lo, hi).round() as i32),
        "bool" => ParamDef::bool(id, label, num(&p.default, 0.0) != 0.0),
        "choice" => {
            let opts: Vec<&'static str> = p.options.iter().map(|o| s(o)).collect();
            let n = opts.len().max(1) as f64;
            ParamDef::choice(id, label, Box::leak(opts.into_boxed_slice()), num(&p.default, 0.0).clamp(0.0, n - 1.0) as u32)
        }
        "colour" => {
            let a = p.default.as_array();
            let c = |i: usize, or: f64| a.and_then(|a| a.get(i)).and_then(|v| v.as_f64()).unwrap_or(or) as f32;
            ParamDef::colour(id, label, [c(0, 1.0), c(1, 1.0), c(2, 1.0), c(3, 1.0)])
        }
        _ => {
            let a = p.default.as_array();
            let c = |i: usize| a.and_then(|a| a.get(i)).and_then(|v| v.as_f64()).unwrap_or(0.5) as f32;
            ParamDef::point(id, label, [c(0), c(1)])
        }
    };
    if p.animatable { d } else { d.fixed() }
}

fn category(c: &str) -> Category {
    match c {
        "blur" => Category::Blur,
        "light" => Category::Light,
        "distort" => Category::Distort,
        "retro" => Category::Retro,
        "time" => Category::Time,
        "keying" => Category::Keying,
        _ => Category::Stylise,
    }
}

/// Puts Studio's shader prelude in front of a plugin's shader body and
/// checks the result, so a broken shader can never reach the GPU.
pub fn full_shader(body: &str, transition: bool) -> Result<String, String> {
    let prelude = if transition { studio_fx::shaders::TRANSITION_PRELUDE } else { studio_fx::shaders::EFFECT_PRELUDE };
    let src = format!("{prelude}\n{body}");
    let module = naga::front::wgsl::parse_str(&src).map_err(|e| format!("the shader does not compile: {}", e.emit_to_string(&src)))?;
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&module).map_err(|e| format!("the shader is not valid: {}", e.emit_to_string(&src)))?;
    if !module.entry_points.iter().any(|e| e.name == "main") {
        return Err(format!("the shader has no `{}` function", if transition { "transition" } else { "effect" }));
    }
    Ok(src)
}

type Key = (String, String, u64, &'static str, String);

enum Def {
    Effect(&'static EffectDef),
    Transition(&'static TransitionDef),
}

/// Leaked definitions by plugin, version, hash, kind and item, so turning a
/// plugin off and on again does not leak again.
fn cache() -> &'static Mutex<HashMap<Key, Def>> {
    static C: OnceLock<Mutex<HashMap<Key, Def>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Makes a loaded plugin's items available. Returns warnings (a shader that
/// did not compile and fell back to the CPU, an item that clashed).
pub fn register(m: &Manifest, hash: u64, rt: Arc<Runtime>) -> Vec<String> {
    let mut warnings = Vec::new();
    let plugin = s(&m.id);
    live().write().unwrap_or_else(|e| e.into_inner()).insert(m.id.clone(), rt.clone());
    let mut cache = cache().lock().unwrap_or_else(|e| e.into_inner());
    let shader = |it: &Item, transition: bool, warnings: &mut Vec<String>| -> Option<&'static str> {
        let body = it.wgsl.as_deref()?;
        match full_shader(body, transition) {
            Ok(src) => Some(s(&src)),
            Err(e) => {
                warnings.push(format!("{}: {e}; {}", it.name, if it.cpu { "using the CPU code" } else { "the item is turned off" }));
                None
            }
        }
    };
    for (kind, export, items) in [("effect", "studio_effect", &m.effects), ("generator", "studio_generate", &m.generators)] {
        for (index, it) in items.iter().enumerate() {
            let key: Key = (m.id.clone(), m.version.clone(), hash, kind, it.id.clone());
            let def = match cache.get(&key) {
                Some(Def::Effect(d)) => *d,
                _ => {
                    let wgsl = shader(it, false, &mut warnings);
                    if wgsl.is_none() && !it.cpu {
                        continue;
                    }
                    let run: &'static PluginEffect = Box::leak(Box::new(PluginEffect { plugin, export, index: index as u32, cpu: it.cpu }));
                    let params = external::leak_params(it.params.iter().map(param_def).collect());
                    let cat = if kind == "generator" { Category::Generator } else { category(&it.category) };
                    let d = external::leak_effect(EffectDef::external(s(&qualified(&m.id, &it.id)), s(&it.name), cat, s(&it.description), params, wgsl, it.animated, run));
                    cache.insert(key, Def::Effect(d));
                    d
                }
            };
            if let Err(e) = external::register_effect(def) {
                warnings.push(e);
            }
        }
    }
    for (index, it) in m.transitions.iter().enumerate() {
        let key: Key = (m.id.clone(), m.version.clone(), hash, "transition", it.id.clone());
        let def = match cache.get(&key) {
            Some(Def::Transition(d)) => *d,
            _ => {
                let wgsl = shader(it, true, &mut warnings);
                if wgsl.is_none() && !it.cpu {
                    continue;
                }
                let run: &'static PluginTransition = Box::leak(Box::new(PluginTransition { plugin, index: index as u32, cpu: it.cpu }));
                let params = external::leak_params(it.params.iter().map(param_def).collect());
                let d = external::leak_transition(TransitionDef::external(s(&qualified(&m.id, &it.id)), s(&it.name), s(&it.description), params, wgsl, run));
                cache.insert(key, Def::Transition(d));
                d
            }
        };
        if let Err(e) = external::register_transition(def) {
            warnings.push(e);
        }
    }
    warnings
}

/// Takes a plugin's items out again and lets its instances go.
pub fn unregister(m: &Manifest) {
    for it in m.effects.iter().chain(&m.generators) {
        external::unregister_effect(&qualified(&m.id, &it.id));
    }
    for it in &m.transitions {
        external::unregister_transition(&qualified(&m.id, &it.id));
    }
    live().write().unwrap_or_else(|e| e.into_inner()).remove(&m.id);
}
