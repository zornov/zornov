// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! # Studio plugin API
//!
//! Write Studio plugins in Rust and compile them to WebAssembly
//! (`wasm32-unknown-unknown`). Studio runs them in a sandbox: a plugin gets
//! pictures, parameters and bytes in and gives pictures, bytes and edit
//! ops back. It has no files, network, clock or other system access, a
//! memory cap, and a time limit per call.
//!
//! A plugin can provide any mix of:
//!
//! - [`Effect`]s: change a clip's frames (with an optional WGSL shader the
//!   GPU renderer runs instead of the CPU code).
//! - [`Generator`]s: draw a picture of their own over their input (put one
//!   on an empty layer).
//! - [`Transition`]s: blend the outgoing picture into the incoming one.
//! - [`Importer`]s: turn a file Studio does not read into a picture or
//!   into edit ops.
//! - [`Exporter`]s: turn the project into a file.
//! - [`Panel`]s: a small panel of labels, sliders and buttons whose
//!   buttons can change the project.
//!
//! ```ignore
//! use studio_plugin_api::*;
//!
//! struct Invert;
//! impl Effect for Invert {
//!     fn info(&self) -> EffectInfo {
//!         EffectInfo::new("invert", "Invert").param(Param::float("amount", "Amount", 0.0, 1.0, 1.0))
//!     }
//!     fn apply(&self, _ctx: &Context, frame: &mut Frame, p: &Params) {
//!         let k = p.float("amount");
//!         for px in frame.pixels.iter_mut() {
//!             for c in 0..3 {
//!                 px[c] += (px[3] - 2.0 * px[c]) * k;
//!             }
//!         }
//!     }
//! }
//!
//! fn plugin() -> Plugin {
//!     Plugin::new("invert", "Invert", "1.0.0").effect(Invert)
//! }
//! export_plugin!(plugin);
//! ```
//!
//! Pictures are linear-light, premultiplied RGBA `f32`, rows top to
//! bottom, the same as Studio's own effects. See `docs/plugins.md` in the
//! Studio repository for the full guide, the ABI and the WGSL binding
//! layout.

pub mod json;

pub use json::Json;

/// The host/plugin ABI this crate speaks. Studio refuses plugins built
/// for an ABI it does not know. Every 1.x release of this crate keeps it.
pub const ABI_VERSION: u32 = 1;

/// One premultiplied linear RGBA pixel.
pub type Px = [f32; 4];

/// Where an effect shows up in Studio's effect browser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Blur,
    Light,
    Stylise,
    Distort,
    Retro,
    Time,
    Keying,
}

impl Category {
    fn key(self) -> &'static str {
        match self {
            Category::Blur => "blur",
            Category::Light => "light",
            Category::Stylise => "stylise",
            Category::Distort => "distort",
            Category::Retro => "retro",
            Category::Time => "time",
            Category::Keying => "keying",
        }
    }
}

/// Per-call information besides the picture and parameters.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Context {
    /// Seconds since the start of the clip (effects, generators).
    pub time: f32,
    pub frame_rate: f32,
    /// Per-clip seed, so two clips with the same effect differ.
    pub seed: u32,
    /// Transitions: 0 (all outgoing) to 1 (all incoming), strictly inside.
    pub progress: f32,
}

/// A picture to read and change in place.
pub struct Frame<'a> {
    pub width: usize,
    pub height: usize,
    pub pixels: &'a mut [Px],
}

impl Frame<'_> {
    #[inline]
    pub fn get(&self, x: usize, y: usize) -> Px {
        self.pixels[y * self.width + x]
    }

    #[inline]
    pub fn set(&mut self, x: usize, y: usize, px: Px) {
        self.pixels[y * self.width + x] = px;
    }

    /// Bilinear sample at pixel coordinates (pixel centres at +0.5),
    /// clamped at the edges.
    pub fn sample(&self, x: f32, y: f32) -> Px {
        sample(self.pixels, self.width, self.height, x, y)
    }

    /// A copy of the pixels, for effects that read neighbours while they
    /// write.
    pub fn snapshot(&self) -> Vec<Px> {
        self.pixels.to_vec()
    }
}

/// Bilinear sample of a pixel buffer at pixel coordinates, clamped.
pub fn sample(pixels: &[Px], width: usize, height: usize, x: f32, y: f32) -> Px {
    if width == 0 || height == 0 {
        return [0.0; 4];
    }
    let fx = (x - 0.5).clamp(0.0, (width - 1) as f32);
    let fy = (y - 0.5).clamp(0.0, (height - 1) as f32);
    let (x0, y0) = (fx as usize, fy as usize);
    let (x1, y1) = ((x0 + 1).min(width - 1), (y0 + 1).min(height - 1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let p = |x: usize, y: usize| pixels[y * width + x];
    let mut out = [0.0; 4];
    for c in 0..4 {
        let a = p(x0, y0)[c] + (p(x1, y0)[c] - p(x0, y0)[c]) * tx;
        let b = p(x0, y1)[c] + (p(x1, y1)[c] - p(x0, y1)[c]) * tx;
        out[c] = a + (b - a) * ty;
    }
    out
}

// ------------------------------------------------------------ parameters

#[derive(Clone, Debug, PartialEq)]
enum Kind {
    Float { min: f32, max: f32, default: f32 },
    Int { min: i32, max: i32, default: i32 },
    Bool(bool),
    Choice(Vec<String>, u32),
    Colour([f32; 4]),
    Point([f32; 2]),
}

/// A parameter: a control in the inspector, a value the AI can set and
/// animate. Ids are `lower_snake_case` and must never change once
/// projects use them.
#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    id: String,
    label: String,
    kind: Kind,
    animatable: bool,
}

impl Param {
    fn new(id: &str, label: &str, kind: Kind) -> Self {
        Param { id: id.to_owned(), label: label.to_owned(), kind, animatable: true }
    }
    /// A slider from `min` to `max`.
    pub fn float(id: &str, label: &str, min: f32, max: f32, default: f32) -> Self {
        Self::new(id, label, Kind::Float { min, max, default })
    }
    pub fn int(id: &str, label: &str, min: i32, max: i32, default: i32) -> Self {
        Self::new(id, label, Kind::Int { min, max, default })
    }
    pub fn bool(id: &str, label: &str, default: bool) -> Self {
        Self::new(id, label, Kind::Bool(default))
    }
    /// A drop-down; the value is the index of the chosen option.
    pub fn choice(id: &str, label: &str, options: &[&str], default: u32) -> Self {
        Self::new(id, label, Kind::Choice(options.iter().map(|s| (*s).to_owned()).collect(), default))
    }
    /// Linear RGB with straight alpha as set; plugins read it premultiplied.
    pub fn colour(id: &str, label: &str, default: [f32; 4]) -> Self {
        Self::new(id, label, Kind::Colour(default))
    }
    /// A position on the frame: (0, 0) top-left, (1, 1) bottom-right.
    pub fn point(id: &str, label: &str, default: [f32; 2]) -> Self {
        Self::new(id, label, Kind::Point(default))
    }
    /// The value cannot be keyframed.
    pub fn fixed(mut self) -> Self {
        self.animatable = false;
        self
    }

    fn json(&self) -> Json {
        let j = Json::obj().with("id", self.id.as_str()).with("label", self.label.as_str()).with("animatable", self.animatable);
        match &self.kind {
            Kind::Float { min, max, default } => j.with("kind", "float").with("min", *min).with("max", *max).with("default", *default),
            Kind::Int { min, max, default } => j.with("kind", "int").with("min", *min).with("max", *max).with("default", *default),
            Kind::Bool(d) => j.with("kind", "bool").with("default", *d),
            Kind::Choice(opts, d) => j.with("kind", "choice").with("options", opts.iter().map(|s| s.as_str()).collect::<Vec<_>>()).with("default", *d),
            Kind::Colour(c) => j.with("kind", "colour").with("default", *c),
            Kind::Point(p) => j.with("kind", "point").with("default", *p),
        }
    }
}

/// The values of an item's parameters for one call, read by id.
///
/// The host packs one `vec4` per parameter in declaration order (numbers,
/// switches and choices in `.x`, points in `.xy`, colours premultiplied in
/// all four), exactly as the WGSL uniforms get them.
pub struct Params<'a> {
    defs: &'a [Param],
    packed: &'a [[f32; 4]],
}

impl<'a> Params<'a> {
    pub fn new(defs: &'a [Param], packed: &'a [[f32; 4]]) -> Self {
        Params { defs, packed }
    }

    fn slot(&self, id: &str) -> [f32; 4] {
        match self.defs.iter().position(|d| d.id == id) {
            Some(i) => self.packed.get(i).copied().unwrap_or_else(|| default_slot(&self.defs[i].kind)),
            None => panic!("no parameter `{id}`"),
        }
    }

    pub fn float(&self, id: &str) -> f32 {
        self.slot(id)[0]
    }
    pub fn int(&self, id: &str) -> i32 {
        self.slot(id)[0].round() as i32
    }
    pub fn bool(&self, id: &str) -> bool {
        self.slot(id)[0] != 0.0
    }
    pub fn choice(&self, id: &str) -> usize {
        self.slot(id)[0].max(0.0) as usize
    }
    /// Premultiplied linear RGBA.
    pub fn colour(&self, id: &str) -> [f32; 4] {
        self.slot(id)
    }
    pub fn point(&self, id: &str) -> [f32; 2] {
        let s = self.slot(id);
        [s[0], s[1]]
    }
}

fn default_slot(k: &Kind) -> [f32; 4] {
    match k {
        Kind::Float { default, .. } => [*default, 0.0, 0.0, 0.0],
        Kind::Int { default, .. } => [*default as f32, 0.0, 0.0, 0.0],
        Kind::Bool(b) => [*b as u8 as f32, 0.0, 0.0, 0.0],
        Kind::Choice(_, d) => [*d as f32, 0.0, 0.0, 0.0],
        Kind::Colour(c) => [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]],
        Kind::Point(p) => [p[0], p[1], 0.0, 0.0],
    }
}

/// The packed defaults of a parameter list (for tests of your own).
pub fn defaults(defs: &[Param]) -> Vec<[f32; 4]> {
    defs.iter().map(|d| default_slot(&d.kind)).collect()
}

// ------------------------------------------------------------- the items

/// What the host shows for an effect, generator or transition.
#[derive(Clone, Debug)]
pub struct EffectInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: Category,
    pub params: Vec<Param>,
    /// WGSL compute shader for the GPU renderer (see `docs/plugins.md`).
    pub wgsl: Option<String>,
    /// The result depends on time or seed, so it must be redrawn each frame.
    pub animated: bool,
    /// False for a shader-only item with no CPU code: the CPU renderer then
    /// leaves the picture as it is.
    pub cpu: bool,
}

impl EffectInfo {
    /// `id` is unique within the plugin, `lower_snake_case`.
    pub fn new(id: &str, name: &str) -> Self {
        EffectInfo { id: id.to_owned(), name: name.to_owned(), description: String::new(), category: Category::Stylise, params: Vec::new(), wgsl: None, animated: false, cpu: true }
    }
    pub fn description(mut self, d: &str) -> Self {
        self.description = d.to_owned();
        self
    }
    pub fn category(mut self, c: Category) -> Self {
        self.category = c;
        self
    }
    pub fn param(mut self, p: Param) -> Self {
        self.params.push(p);
        self
    }
    pub fn wgsl(mut self, src: &str) -> Self {
        self.wgsl = Some(src.to_owned());
        self
    }
    pub fn animated(mut self) -> Self {
        self.animated = true;
        self
    }
    /// Shader only: no CPU implementation.
    pub fn gpu_only(mut self) -> Self {
        self.cpu = false;
        self
    }

    fn json(&self) -> Json {
        let mut j = Json::obj()
            .with("id", self.id.as_str())
            .with("name", self.name.as_str())
            .with("description", self.description.as_str())
            .with("category", self.category.key())
            .with("animated", self.animated)
            .with("cpu", self.cpu)
            .with("params", Json::Arr(self.params.iter().map(Param::json).collect()));
        if let Some(w) = &self.wgsl {
            j = j.with("wgsl", w.as_str());
        }
        j
    }
}

/// Changes a clip's frames.
pub trait Effect: Send + Sync {
    fn info(&self) -> EffectInfo;
    /// Changes `frame` in place.
    fn apply(&self, ctx: &Context, frame: &mut Frame, params: &Params);
}

/// Draws a picture of its own over its input (`frame` holds the input).
pub trait Generator: Send + Sync {
    fn info(&self) -> EffectInfo;
    fn generate(&self, ctx: &Context, frame: &mut Frame, params: &Params);
}

/// Blends `a` (outgoing) into `b` (incoming); writes the result into `a`.
pub trait Transition: Send + Sync {
    fn info(&self) -> EffectInfo;
    fn apply(&self, ctx: &Context, a: &mut Frame, b: &Frame, params: &Params);
}

/// What an importer, exporter or panel is called.
#[derive(Clone, Debug)]
pub struct ToolInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Importers: the file extensions read (lower case, no dot). Exporters:
    /// the one extension written.
    pub extensions: Vec<String>,
}

impl ToolInfo {
    pub fn new(id: &str, name: &str) -> Self {
        ToolInfo { id: id.to_owned(), name: name.to_owned(), description: String::new(), extensions: Vec::new() }
    }
    pub fn description(mut self, d: &str) -> Self {
        self.description = d.to_owned();
        self
    }
    pub fn extension(mut self, e: &str) -> Self {
        self.extensions.push(e.trim_start_matches('.').to_ascii_lowercase());
        self
    }

    fn json(&self) -> Json {
        Json::obj().with("id", self.id.as_str()).with("name", self.name.as_str()).with("description", self.description.as_str()).with("extensions", self.extensions.iter().map(|s| s.as_str()).collect::<Vec<_>>())
    }
}

/// What an importer made of a file.
#[derive(Clone, Debug, PartialEq)]
pub enum Imported {
    /// A picture: straight-alpha sRGB RGBA8, rows top to bottom. Studio
    /// saves it as a PNG in the project and adds it to the media bin.
    Image { width: u32, height: u32, rgba8: Vec<u8> },
    /// Edit ops (the agent's `edit` ops, as a JSON array) applied as one
    /// undo step: an edit list, markers, captions.
    Ops(Json),
}

/// Reads a file Studio does not know.
pub trait Importer: Send + Sync {
    fn info(&self) -> ToolInfo;
    /// `name` is the file name (no folders); `bytes` its contents.
    fn import(&self, name: &str, bytes: &[u8]) -> Result<Imported, String>;
}

/// Writes the project in a format of its own.
pub trait Exporter: Send + Sync {
    fn info(&self) -> ToolInfo;
    /// `project` is the project as Studio's agent sees it: `canvas`, `fps`,
    /// `duration`, `clips` (id, name, kind, lane, start, dur, effects,
    /// text), `markers`.
    fn export(&self, project: &Json) -> Result<Vec<u8>, String>;
}

/// One control on a panel.
#[derive(Clone, Debug, PartialEq)]
pub enum Widget {
    Heading(String),
    Label(String),
    /// A slider whose value comes back in [`PanelEvent::values`].
    Slider { id: String, label: String, min: f32, max: f32, value: f32 },
    Toggle { id: String, label: String, value: bool },
    /// A button; pressing it calls [`Panel::update`] with its id.
    Button { id: String, label: String },
    Separator,
}

impl Widget {
    fn json(&self) -> Json {
        match self {
            Widget::Heading(t) => Json::obj().with("type", "heading").with("text", t.as_str()),
            Widget::Label(t) => Json::obj().with("type", "label").with("text", t.as_str()),
            Widget::Slider { id, label, min, max, value } => Json::obj().with("type", "slider").with("id", id.as_str()).with("label", label.as_str()).with("min", *min).with("max", *max).with("value", *value),
            Widget::Toggle { id, label, value } => Json::obj().with("type", "toggle").with("id", id.as_str()).with("label", label.as_str()).with("value", *value),
            Widget::Button { id, label } => Json::obj().with("type", "button").with("id", id.as_str()).with("label", label.as_str()),
            Widget::Separator => Json::obj().with("type", "separator"),
        }
    }
}

/// Why a panel is being drawn.
#[derive(Clone, Debug, Default)]
pub struct PanelEvent {
    /// The button pressed, or `None` when the panel is just shown.
    pub button: Option<String>,
    /// Current slider and toggle values by id (numbers; toggles 0 or 1).
    pub values: Vec<(String, f32)>,
    /// Ids of the selected clips.
    pub selection: Vec<u32>,
}

impl PanelEvent {
    pub fn value(&self, id: &str) -> Option<f32> {
        self.values.iter().find(|(k, _)| k == id).map(|(_, v)| *v)
    }
}

/// What a panel shows, and what to change.
#[derive(Clone, Debug, Default)]
pub struct PanelView {
    pub widgets: Vec<Widget>,
    /// Edit ops to apply as one undo step (the agent's `edit` ops).
    pub ops: Option<Json>,
    /// A short message shown as a notification.
    pub message: Option<String>,
}

/// A small panel in Studio's library.
pub trait Panel: Send + Sync {
    fn info(&self) -> ToolInfo;
    fn update(&self, event: &PanelEvent, project: &Json) -> PanelView;
}

// ---------------------------------------------------------------- plugin

/// Everything a plugin provides. Build one in the function you pass to
/// [`export_plugin!`].
pub struct Plugin {
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub license: String,
    pub homepage: String,
    effects: Vec<(EffectInfo, Box<dyn Effect>)>,
    generators: Vec<(EffectInfo, Box<dyn Generator>)>,
    transitions: Vec<(EffectInfo, Box<dyn Transition>)>,
    importers: Vec<(ToolInfo, Box<dyn Importer>)>,
    exporters: Vec<(ToolInfo, Box<dyn Exporter>)>,
    panels: Vec<(ToolInfo, Box<dyn Panel>)>,
}

impl Plugin {
    /// `id` is the plugin's unique name, `lower-kebab-case` or
    /// `lower_snake_case`; `version` is semver.
    pub fn new(id: &str, name: &str, version: &str) -> Self {
        Plugin {
            id: id.to_owned(),
            name: name.to_owned(),
            version: version.to_owned(),
            author: String::new(),
            description: String::new(),
            license: String::new(),
            homepage: String::new(),
            effects: Vec::new(),
            generators: Vec::new(),
            transitions: Vec::new(),
            importers: Vec::new(),
            exporters: Vec::new(),
            panels: Vec::new(),
        }
    }
    pub fn author(mut self, s: &str) -> Self {
        self.author = s.to_owned();
        self
    }
    pub fn description(mut self, s: &str) -> Self {
        self.description = s.to_owned();
        self
    }
    /// An SPDX licence id.
    pub fn license(mut self, s: &str) -> Self {
        self.license = s.to_owned();
        self
    }
    pub fn homepage(mut self, s: &str) -> Self {
        self.homepage = s.to_owned();
        self
    }
    pub fn effect(mut self, e: impl Effect + 'static) -> Self {
        self.effects.push((e.info(), Box::new(e)));
        self
    }
    pub fn generator(mut self, g: impl Generator + 'static) -> Self {
        self.generators.push((g.info(), Box::new(g)));
        self
    }
    pub fn transition(mut self, t: impl Transition + 'static) -> Self {
        self.transitions.push((t.info(), Box::new(t)));
        self
    }
    pub fn importer(mut self, i: impl Importer + 'static) -> Self {
        self.importers.push((i.info(), Box::new(i)));
        self
    }
    pub fn exporter(mut self, e: impl Exporter + 'static) -> Self {
        self.exporters.push((e.info(), Box::new(e)));
        self
    }
    pub fn panel(mut self, p: impl Panel + 'static) -> Self {
        self.panels.push((p.info(), Box::new(p)));
        self
    }

    /// The manifest the host reads: everything above as JSON.
    pub fn manifest(&self) -> Json {
        let tools = |v: Vec<&ToolInfo>| Json::Arr(v.into_iter().map(ToolInfo::json).collect());
        Json::obj()
            .with("abi", ABI_VERSION)
            .with("api", env!("CARGO_PKG_VERSION"))
            .with("id", self.id.as_str())
            .with("name", self.name.as_str())
            .with("version", self.version.as_str())
            .with("author", self.author.as_str())
            .with("description", self.description.as_str())
            .with("license", self.license.as_str())
            .with("homepage", self.homepage.as_str())
            .with("effects", infos(&self.effects))
            .with("generators", infos(&self.generators))
            .with("transitions", infos(&self.transitions))
            .with("importers", tools(self.importers.iter().map(|(i, _)| i).collect()))
            .with("exporters", tools(self.exporters.iter().map(|(i, _)| i).collect()))
            .with("panels", tools(self.panels.iter().map(|(i, _)| i).collect()))
    }

    // Native entry points, used by the wasm exports and by plugin tests run
    // on the host.

    /// Runs effect `index` on `frame`.
    pub fn run_effect(&self, index: usize, ctx: &Context, frame: &mut Frame, packed: &[[f32; 4]]) -> Result<(), String> {
        let (info, e) = self.effects.get(index).ok_or("no such effect")?;
        e.apply(ctx, frame, &Params::new(&info.params, packed));
        Ok(())
    }

    pub fn run_generator(&self, index: usize, ctx: &Context, frame: &mut Frame, packed: &[[f32; 4]]) -> Result<(), String> {
        let (info, g) = self.generators.get(index).ok_or("no such generator")?;
        g.generate(ctx, frame, &Params::new(&info.params, packed));
        Ok(())
    }

    pub fn run_transition(&self, index: usize, ctx: &Context, a: &mut Frame, b: &Frame, packed: &[[f32; 4]]) -> Result<(), String> {
        let (info, t) = self.transitions.get(index).ok_or("no such transition")?;
        t.apply(ctx, a, b, &Params::new(&info.params, packed));
        Ok(())
    }

    pub fn run_import(&self, index: usize, name: &str, bytes: &[u8]) -> Result<Imported, String> {
        self.importers.get(index).ok_or("no such importer")?.1.import(name, bytes)
    }

    pub fn run_export(&self, index: usize, project: &Json) -> Result<Vec<u8>, String> {
        self.exporters.get(index).ok_or("no such exporter")?.1.export(project)
    }

    pub fn run_panel(&self, index: usize, event: &PanelEvent, project: &Json) -> Result<PanelView, String> {
        Ok(self.panels.get(index).ok_or("no such panel")?.1.update(event, project))
    }
}

fn infos<T: ?Sized>(v: &[(EffectInfo, Box<T>)]) -> Json {
    Json::Arr(v.iter().map(|(i, _)| i.json()).collect())
}

/// Implementation details of [`export_plugin!`]; not part of the stable
/// API.
#[doc(hidden)]
pub mod __rt {
    use super::*;
    use std::sync::OnceLock;

    static PLUGIN: OnceLock<Plugin> = OnceLock::new();

    pub fn plugin(build: fn() -> Plugin) -> &'static Plugin {
        PLUGIN.get_or_init(build)
    }

    pub fn alloc(n: u32) -> u32 {
        let layout = std::alloc::Layout::from_size_align(n.max(1) as usize, 16).expect("layout");
        // SAFETY: the layout has a non-zero size.
        unsafe { std::alloc::alloc(layout) as usize as u32 }
    }

    /// # Safety
    /// `ptr` and `n` must come from [`alloc`].
    pub unsafe fn free(ptr: u32, n: u32) {
        let layout = std::alloc::Layout::from_size_align(n.max(1) as usize, 16).expect("layout");
        // SAFETY: the caller passes what `alloc` returned.
        unsafe { std::alloc::dealloc(ptr as usize as *mut u8, layout) }
    }

    /// Hands bytes to the host: copied into an [`alloc`] block the host
    /// frees, returned as `ptr << 32 | len`.
    pub fn give(bytes: &[u8]) -> u64 {
        let p = alloc(bytes.len() as u32);
        // SAFETY: `p` has room for `bytes.len()` bytes.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), p as usize as *mut u8, bytes.len()) };
        ((p as u64) << 32) | bytes.len() as u64
    }

    /// A blob of `[u32 LE header length][header JSON][payload]`.
    pub fn blob(header: &Json, payload: &[u8]) -> u64 {
        let h = header.to_json();
        let mut out = Vec::with_capacity(4 + h.len() + payload.len());
        out.extend_from_slice(&(h.len() as u32).to_le_bytes());
        out.extend_from_slice(h.as_bytes());
        out.extend_from_slice(payload);
        give(&out)
    }

    /// # Safety
    /// The pointer must be valid for `len` bytes (the host guarantees it).
    pub unsafe fn bytes<'a>(ptr: u32, len: u32) -> &'a [u8] {
        if len == 0 {
            return &[];
        }
        // SAFETY: the host wrote `len` bytes at `ptr`.
        unsafe { std::slice::from_raw_parts(ptr as usize as *const u8, len as usize) }
    }

    /// # Safety
    /// The host wrote 16 bytes at `ptr`.
    pub unsafe fn ctx(ptr: u32) -> Context {
        // SAFETY: see above.
        let w = unsafe { std::slice::from_raw_parts(ptr as usize as *const u32, 4) };
        Context { time: f32::from_bits(w[0]), frame_rate: f32::from_bits(w[1]), seed: w[2], progress: f32::from_bits(w[3]) }
    }

    /// # Safety
    /// The host wrote `w * h` pixels at `ptr`.
    pub unsafe fn frame<'a>(ptr: u32, w: u32, h: u32) -> Frame<'a> {
        let n = w as usize * h as usize;
        // SAFETY: see above; the buffer is 16-byte aligned.
        let pixels = unsafe { std::slice::from_raw_parts_mut(ptr as usize as *mut Px, n) };
        Frame { width: w as usize, height: h as usize, pixels }
    }

    /// # Safety
    /// The host wrote `n` vec4s at `ptr`.
    pub unsafe fn packed<'a>(ptr: u32, n: u32) -> &'a [[f32; 4]] {
        if n == 0 {
            return &[];
        }
        // SAFETY: see above.
        unsafe { std::slice::from_raw_parts(ptr as usize as *const [f32; 4], n as usize) }
    }

    pub fn status(r: Result<(), String>) -> i32 {
        match r {
            Ok(()) => 0,
            Err(e) => {
                super::log(&e);
                1
            }
        }
    }

    pub fn import_result(r: Result<Imported, String>) -> u64 {
        match r {
            Ok(Imported::Image { width, height, rgba8 }) => blob(&Json::obj().with("kind", "image").with("width", width).with("height", height), &rgba8),
            Ok(Imported::Ops(ops)) => blob(&Json::obj().with("kind", "ops").with("ops", ops), &[]),
            Err(e) => blob(&Json::obj().with("kind", "error").with("message", e), &[]),
        }
    }

    pub fn export_result(r: Result<Vec<u8>, String>) -> u64 {
        match r {
            Ok(b) => blob(&Json::obj().with("ok", true), &b),
            Err(e) => blob(&Json::obj().with("ok", false).with("message", e), &[]),
        }
    }

    /// Reads the panel call `{"event": {...}, "project": {...}}`.
    pub fn panel_call(src: &[u8]) -> (PanelEvent, Json) {
        let v = std::str::from_utf8(src).ok().and_then(|s| Json::parse(s).ok()).unwrap_or(Json::Null);
        let e = v.get("event");
        let mut values = Vec::new();
        if let Json::Obj(m) = e.get("values") {
            for (k, x) in m {
                let n = x.as_f64().or(x.as_bool().map(|b| b as u8 as f64)).unwrap_or(0.0);
                values.push((k.clone(), n as f32));
            }
        }
        let event = PanelEvent {
            button: e.get("button").as_str().map(str::to_owned),
            values,
            selection: e.get("selection").items().iter().filter_map(|x| x.as_f64()).map(|x| x as u32).collect(),
        };
        (event, v.get("project").clone())
    }

    pub fn panel_result(r: Result<PanelView, String>) -> u64 {
        let j = match r {
            Ok(v) => {
                let mut j = Json::obj().with("widgets", Json::Arr(v.widgets.iter().map(Widget::json).collect()));
                if let Some(ops) = v.ops {
                    j = j.with("ops", ops);
                }
                if let Some(m) = v.message {
                    j = j.with("message", m);
                }
                j
            }
            Err(e) => Json::obj().with("error", e),
        };
        give(j.to_json().as_bytes())
    }
}

/// Writes a line to Studio's plugin log (shown in the plugin's details).
pub fn log(msg: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        #[link(wasm_import_module = "studio")]
        unsafe extern "C" {
            #[link_name = "log"]
            fn host_log(ptr: u32, len: u32);
        }
        // SAFETY: the host only reads `len` bytes at `ptr`.
        unsafe { host_log(msg.as_ptr() as usize as u32, msg.len() as u32) }
    }
    #[cfg(not(target_arch = "wasm32"))]
    eprintln!("[plugin] {msg}");
}

/// Exports a plugin built by `$build` (a `fn() -> Plugin`) with Studio's
/// ABI. Use it once, at the root of a `cdylib` crate.
#[macro_export]
macro_rules! export_plugin {
    ($build:path) => {
        #[cfg(target_arch = "wasm32")]
        mod __studio_exports {
            #[allow(unused_imports)]
            use super::*;
            use $crate::__rt;
            fn p() -> &'static $crate::Plugin {
                __rt::plugin($build)
            }
            #[unsafe(no_mangle)]
            pub extern "C" fn studio_abi() -> u32 {
                $crate::ABI_VERSION
            }
            #[unsafe(no_mangle)]
            pub extern "C" fn studio_alloc(n: u32) -> u32 {
                __rt::alloc(n)
            }
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn studio_free(ptr: u32, n: u32) {
                unsafe { __rt::free(ptr, n) }
            }
            #[unsafe(no_mangle)]
            pub extern "C" fn studio_manifest() -> u64 {
                __rt::give(p().manifest().to_json().as_bytes())
            }
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn studio_effect(i: u32, ctx: u32, px: u32, w: u32, h: u32, prm: u32, n: u32) -> i32 {
                let (c, mut f, pk) = unsafe { (__rt::ctx(ctx), __rt::frame(px, w, h), __rt::packed(prm, n)) };
                __rt::status(p().run_effect(i as usize, &c, &mut f, pk))
            }
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn studio_generate(i: u32, ctx: u32, px: u32, w: u32, h: u32, prm: u32, n: u32) -> i32 {
                let (c, mut f, pk) = unsafe { (__rt::ctx(ctx), __rt::frame(px, w, h), __rt::packed(prm, n)) };
                __rt::status(p().run_generator(i as usize, &c, &mut f, pk))
            }
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn studio_transition(i: u32, ctx: u32, a: u32, b: u32, w: u32, h: u32, prm: u32, n: u32) -> i32 {
                let (c, mut fa, fb, pk) = unsafe { (__rt::ctx(ctx), __rt::frame(a, w, h), __rt::frame(b, w, h), __rt::packed(prm, n)) };
                __rt::status(p().run_transition(i as usize, &c, &mut fa, &fb, pk))
            }
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn studio_import(i: u32, name: u32, name_len: u32, data: u32, len: u32) -> u64 {
                let (nm, d) = unsafe { (__rt::bytes(name, name_len), __rt::bytes(data, len)) };
                __rt::import_result(p().run_import(i as usize, core::str::from_utf8(nm).unwrap_or(""), d))
            }
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn studio_export(i: u32, data: u32, len: u32) -> u64 {
                let d = unsafe { __rt::bytes(data, len) };
                let project = core::str::from_utf8(d).ok().and_then(|s| $crate::Json::parse(s).ok()).unwrap_or($crate::Json::Null);
                __rt::export_result(p().run_export(i as usize, &project))
            }
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn studio_panel(i: u32, data: u32, len: u32) -> u64 {
                let (event, project) = __rt::panel_call(unsafe { __rt::bytes(data, len) });
                __rt::panel_result(p().run_panel(i as usize, &event, &project))
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Gain;
    impl Effect for Gain {
        fn info(&self) -> EffectInfo {
            EffectInfo::new("gain", "Gain").param(Param::float("gain", "Gain", 0.0, 4.0, 2.0)).param(Param::colour("tint", "Tint", [1.0, 0.5, 0.0, 0.5]))
        }
        fn apply(&self, _: &Context, f: &mut Frame, p: &Params) {
            let g = p.float("gain");
            for px in f.pixels.iter_mut() {
                px[0] *= g;
            }
        }
    }

    #[test]
    fn manifest_and_native_calls() {
        let pl = Plugin::new("t", "Test", "1.2.3").effect(Gain);
        let m = Json::parse(&pl.manifest().to_json()).unwrap();
        assert_eq!(m.get("abi").as_f64(), Some(1.0));
        assert_eq!(m.get("effects").at(0).get("params").at(1).get("kind").as_str(), Some("colour"));
        let mut px = vec![[0.25, 0.0, 0.0, 1.0]; 4];
        let mut f = Frame { width: 2, height: 2, pixels: &mut px };
        let defs = Gain.info().params;
        let packed = defaults(&defs);
        assert_eq!(packed[1], [0.5, 0.25, 0.0, 0.5]);
        pl.run_effect(0, &Context::default(), &mut f, &packed).unwrap();
        assert_eq!(f.get(1, 1)[0], 0.5);
        assert!(pl.run_effect(3, &Context::default(), &mut f, &packed).is_err());
    }

    #[test]
    fn sampling_is_bilinear_and_clamped() {
        let px = vec![[0.0; 4], [1.0; 4]];
        assert_eq!(sample(&px, 2, 1, 1.0, 0.5)[0], 0.5);
        assert_eq!(sample(&px, 2, 1, -5.0, 0.5)[0], 0.0);
        assert_eq!(sample(&px, 2, 1, 9.0, 9.0)[0], 1.0);
    }
}
