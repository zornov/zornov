// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The effect catalogue. Each effect is an [`EffectDef`]: metadata, a
//! parameter list, and a CPU implementation (plus WGSL for the per-pixel
//! ones).

use std::sync::OnceLock;

use crate::image::Image;
use crate::params::{ParamDef, ParamSet};

pub mod blur;
pub mod distort;
pub mod generate;
pub mod keying;
pub mod light;
pub mod retro;
pub mod stylise;
pub mod time;

/// Groups shown in the effects browser.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    Blur,
    Light,
    Stylise,
    Distort,
    Retro,
    Time,
    Keying,
    Generator,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Category::Blur => "Blur & focus",
            Category::Light => "Light",
            Category::Stylise => "Stylise",
            Category::Distort => "Distort",
            Category::Retro => "Retro",
            Category::Time => "Time",
            Category::Keying => "Keying",
            Category::Generator => "Generators",
        }
    }
}

/// Everything an effect may need besides its input and parameters.
#[derive(Clone, Copy, Debug)]
pub struct EffectContext<'a> {
    /// Seconds since the start of the clip. Animated effects (noise,
    /// shake, flicker) are pure functions of this, so scrubbing and
    /// rendering give the same frame.
    pub time: f32,
    /// Frames per second of the sequence.
    pub frame_rate: f32,
    /// Per-clip seed so two clips with the same effect do not move in step.
    pub seed: u32,
    /// Earlier frames of the same clip, already through the effects that
    /// come before this one: `history[0]` is the previous frame. Effects
    /// declare how many they need with [`EffectDef::history_needed`]; fewer
    /// may be given (at the start of a clip) and effects cope.
    pub history: &'a [Image],
    /// Second input for effects that compare against another image, such
    /// as the clean plate of the difference key.
    pub aux: Option<&'a Image>,
}

impl EffectContext<'static> {
    /// A context for a still frame at time zero.
    pub fn still() -> Self {
        EffectContext { time: 0.0, frame_rate: 30.0, seed: 0, history: &[], aux: None }
    }
}

impl Default for EffectContext<'static> {
    fn default() -> Self {
        Self::still()
    }
}

/// One effect.
#[derive(Clone, Copy)]
pub struct EffectDef {
    /// Stable identifier for project files.
    pub id: &'static str,
    pub name: &'static str,
    pub category: Category,
    /// One line for tooltips.
    pub description: &'static str,
    pub params: &'static [ParamDef],
    /// A parameter and value at which the effect leaves its input
    /// untouched (usually a strength or mix of zero).
    pub identity: Option<(&'static str, f32)>,
    /// WGSL compute shader for the GPU path, if there is one. See the
    /// crate README for the binding layout.
    pub wgsl: Option<&'static str>,
    /// Whether the result depends on the clip time, frame rate or seed
    /// (noise, shake, flicker, flares). Every other effect gives the same
    /// picture for the same input and parameters, so a renderer may keep
    /// its result while nothing on the layer changes.
    pub animated: bool,
    /// For shaders that draw from data worked out on the CPU (particles):
    /// the `u32` words bound as `var<storage, read> data` (binding 4) for
    /// a picture of the given size.
    pub storage: Option<fn(&EffectContext, usize, usize, &ParamSet) -> Vec<u32>>,
    history: fn(&ParamSet, f32) -> usize,
    run: fn(&EffectContext, &Image, &ParamSet) -> Image,
    /// Set for effects registered at run time (plugins); does the work
    /// instead of `run`.
    ext: Option<&'static dyn crate::external::EffectRun>,
}

impl std::fmt::Debug for EffectDef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EffectDef").field("id", &self.id).field("category", &self.category).finish()
    }
}

fn no_history(_: &ParamSet, _: f32) -> usize {
    0
}

fn passthrough(_: &EffectContext, img: &Image, _: &ParamSet) -> Image {
    img.clone()
}

/// Template for the catalogue entries; each entry overrides what it needs.
pub(crate) const BASE: EffectDef = EffectDef {
    id: "",
    name: "",
    category: Category::Blur,
    description: "",
    params: &[],
    identity: None,
    wgsl: None,
    animated: false,
    storage: None,
    history: no_history,
    run: passthrough,
    ext: None,
};

impl EffectDef {
    pub(crate) const fn new(
        id: &'static str,
        name: &'static str,
        category: Category,
        description: &'static str,
        params: &'static [ParamDef],
        run: fn(&EffectContext, &Image, &ParamSet) -> Image,
    ) -> Self {
        EffectDef { id, name, category, description, params, run, ..BASE }
    }

    /// An effect whose work is done by `run` (a plugin), for
    /// [`crate::external::register_effect`].
    #[allow(clippy::too_many_arguments)]
    pub const fn external(
        id: &'static str,
        name: &'static str,
        category: Category,
        description: &'static str,
        params: &'static [ParamDef],
        wgsl: Option<&'static str>,
        animated: bool,
        run: &'static dyn crate::external::EffectRun,
    ) -> Self {
        EffectDef { id, name, category, description, params, wgsl, animated, ext: Some(run), ..BASE }
    }

    /// Whether this effect was registered at run time (a plugin).
    pub fn is_external(&self) -> bool {
        self.ext.is_some()
    }

    pub(crate) const fn identity(mut self, id: &'static str, value: f32) -> Self {
        self.identity = Some((id, value));
        self
    }

    pub(crate) const fn wgsl(mut self, src: &'static str) -> Self {
        self.wgsl = Some(src);
        self
    }

    pub(crate) const fn animated(mut self) -> Self {
        self.animated = true;
        self
    }

    pub(crate) const fn storage(mut self, f: fn(&EffectContext, usize, usize, &ParamSet) -> Vec<u32>) -> Self {
        self.storage = Some(f);
        self
    }

    pub(crate) const fn history(mut self, f: fn(&ParamSet, f32) -> usize) -> Self {
        self.history = f;
        self
    }

    /// A parameter set with every value at its default.
    pub fn defaults(&self) -> ParamSet {
        ParamSet::defaults(self.params)
    }

    /// How many previous frames this effect wants in
    /// [`EffectContext::history`] with these parameters.
    pub fn history_needed(&self, params: &ParamSet, frame_rate: f32) -> usize {
        (self.history)(params, frame_rate)
    }

    /// Runs the effect. The output has the input's size.
    ///
    /// `params` must have been built from this effect's parameter list.
    pub fn apply(&self, ctx: &EffectContext, input: &Image, params: &ParamSet) -> Image {
        assert!(same_defs(params.defs(), self.params), "parameter set belongs to another effect");
        if input.width == 0 || input.height == 0 {
            return input.clone();
        }
        let mut out = match self.ext {
            Some(ext) => ext.run(ctx, input, params),
            None => (self.run)(ctx, input, params),
        };
        if (out.width, out.height) != (input.width, input.height) {
            // Only a misbehaving plugin gets here; leave the picture as it was.
            return input.clone();
        }
        debug_assert_eq!((out.width, out.height), (input.width, input.height));
        // Debug builds (and so the tests) catch numeric bugs; release builds
        // must never hand NaNs to the compositor.
        debug_assert!(out.is_finite(), "effect `{}` produced a non-finite value", self.id);
        out.sanitise();
        out
    }
}

/// Every effect, in browser order.
pub fn all() -> &'static [&'static EffectDef] {
    static ALL: OnceLock<Vec<&'static EffectDef>> = OnceLock::new();
    ALL.get_or_init(|| {
        [
            blur::EFFECTS,
            light::EFFECTS,
            stylise::EFFECTS,
            distort::EFFECTS,
            retro::EFFECTS,
            time::EFFECTS,
            keying::EFFECTS,
            generate::EFFECTS,
        ]
        .into_iter()
        .flatten()
        .collect()
    })
}

/// Looks an effect up by id: the built-in catalogue, then effects
/// registered at run time ([`crate::external`]).
pub fn find(id: &str) -> Option<&'static EffectDef> {
    all().iter().copied().find(|e| e.id == id).or_else(|| crate::external::find_effect(id))
}

/// Shared parameter for effects whose result is blended with the input.
pub(crate) const MIX: ParamDef = ParamDef::float("mix", "Mix", 0.0, 1.0, 1.0).unit(crate::params::Unit::Fraction);

/// Blends an effect result back with its input by the `mix` parameter.
pub(crate) fn apply_mix(input: &Image, result: Image, p: &ParamSet) -> Image {
    let m = p.f("mix");
    if m >= 1.0 {
        result
    } else {
        crate::ops::mix(input, &result, m)
    }
}

/// Two parameter lists are the same when their ids match in order; the
/// slices themselves may live at different addresses.
pub(crate) fn same_defs(a: &[ParamDef], b: &[ParamDef]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.id == y.id)
}
