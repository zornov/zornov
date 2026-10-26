// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! One `.ofx.bundle`: its plugins, described, and instances that render.
//! Runs in the OFX worker process.

use std::ffi::{CStr, CString, c_char, c_int};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::abi::*;
use super::host::{self, ClipImage, Depth, Effect, Props, Val};

/// A parameter Studio can show and set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParamInfo {
    pub name: String,
    pub label: String,
    /// The OFX type (`OfxParamTypeDouble`...).
    pub kind: String,
    pub default: Vec<f64>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub options: Vec<String>,
    pub hint: String,
    pub secret: bool,
    /// `OfxParamPropDoubleType` (how a 2D value is meant: a point...).
    pub double_type: String,
}

/// A plugin in a bundle, as described.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginInfo {
    pub index: usize,
    pub id: String,
    pub version: String,
    pub label: String,
    pub grouping: String,
    pub description: String,
    /// The context Studio runs it in.
    pub context: String,
    pub depth: String,
    pub params: Vec<ParamInfo>,
    pub clips: Vec<String>,
}

pub struct Plugin {
    pub info: PluginInfo,
    raw: *const OfxPlugin,
    desc: Box<Effect>,
    ctx_desc: Option<Box<Effect>>,
    instance: Option<Box<Effect>>,
    began: bool,
}

pub struct Bundle {
    pub path: PathBuf,
    pub plugins: Vec<Plugin>,
    /// Plugins that failed to describe, with why.
    pub skipped: Vec<(String, String)>,
    _lib: libloading::Library,
}

/// The binary inside a bundle: `Contents/MacOS/<name>.ofx` on macOS.
pub fn binary(bundle: &Path) -> Option<PathBuf> {
    let name = bundle.file_name()?.to_string_lossy().trim_end_matches(".bundle").to_owned();
    let dir = if cfg!(target_os = "macos") {
        "MacOS"
    } else if cfg!(windows) {
        "Win64"
    } else {
        "Linux-x86-64"
    };
    let p = bundle.join("Contents").join(dir).join(&name);
    p.is_file().then_some(p)
}

fn s(p: *const c_char) -> String {
    if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
}

fn call(raw: *const OfxPlugin, action: &str, handle: Handle, in_args: Option<&Props>, out_args: Option<&Props>) -> OfxStatus {
    let a = CString::new(action).unwrap_or_default();
    // SAFETY: `raw` came from OfxGetPlugin and stays valid while the
    // library is loaded.
    let Some(main) = (unsafe { (*raw).main_entry }) else { return kOfxStatErrMissingHostFeature };
    unsafe { main(a.as_ptr(), handle, in_args.map_or(std::ptr::null_mut(), Props::handle), out_args.map_or(std::ptr::null_mut(), Props::handle)) }
}

fn ok(st: OfxStatus) -> bool {
    st == kOfxStatOK || st == kOfxStatReplyDefault
}

impl Bundle {
    /// Loads a bundle and describes its image effect plugins.
    ///
    /// # Safety
    /// Runs the bundle's native code; call it only in the worker process.
    pub unsafe fn load(path: &Path) -> Result<Bundle, String> {
        let bin = binary(path).ok_or_else(|| format!("{} has no binary for this computer", path.display()))?;
        let lib = unsafe { libloading::Library::new(&bin) }.map_err(|e| format!("could not load {}: {e}", bin.display()))?;
        type Count = unsafe extern "C" fn() -> c_int;
        type Get = unsafe extern "C" fn(c_int) -> *const OfxPlugin;
        type SetHost = unsafe extern "C" fn(*mut OfxHost) -> OfxStatus;
        if let Ok(set) = unsafe { lib.get::<SetHost>(b"OfxSetHost\0") } {
            unsafe { set(host::host()) };
        }
        let count = unsafe { lib.get::<Count>(b"OfxGetNumberOfPlugins\0") }.map_err(|_| "not an OpenFX plugin (no OfxGetNumberOfPlugins)".to_owned())?;
        let get = unsafe { lib.get::<Get>(b"OfxGetPlugin\0") }.map_err(|_| "not an OpenFX plugin (no OfxGetPlugin)".to_owned())?;
        let n = unsafe { count() };
        let mut plugins = Vec::new();
        let mut skipped = Vec::new();
        for i in 0..n.max(0) {
            let raw = unsafe { get(i) };
            if raw.is_null() {
                continue;
            }
            let (api, version, id) = unsafe { (s((*raw).plugin_api), (*raw).api_version, s((*raw).plugin_identifier)) };
            if api != API || version != 1 {
                skipped.push((id, format!("{api} v{version} is not the image effect API")));
                continue;
            }
            match unsafe { Plugin::describe(i as usize, raw) } {
                Ok(p) => plugins.push(p),
                Err(e) => skipped.push((id, e)),
            }
        }
        Ok(Bundle { path: path.to_owned(), plugins, skipped, _lib: lib })
    }
}

impl Plugin {
    unsafe fn describe(index: usize, raw: *const OfxPlugin) -> Result<Plugin, String> {
        if let Some(set) = unsafe { (*raw).set_host } {
            unsafe { set(host::host()) };
        }
        let id = s(unsafe { (*raw).plugin_identifier });
        let version = unsafe { format!("{}.{}", (*raw).plugin_version_major, (*raw).plugin_version_minor) };
        let st = call(raw, ACTION_LOAD, std::ptr::null_mut(), None, None);
        if !ok(st) {
            return Err(format!("refused to load (status {st})"));
        }
        let desc = Effect::new();
        let st = call(raw, ACTION_DESCRIBE, desc.handle(), None, None);
        if !ok(st) {
            return Err(format!("could not describe itself (status {st})"));
        }
        let contexts = desc.props.strs(P_CONTEXTS);
        let context = [CONTEXT_FILTER, CONTEXT_GENERAL, CONTEXT_GENERATOR, CONTEXT_TRANSITION].into_iter().find(|c| contexts.iter().any(|x| x == c)).ok_or_else(|| format!("runs only in contexts Zornov does not offer ({})", contexts.join(", ")))?;
        let depths = desc.props.strs(P_DEPTHS);
        let depth = [DEPTH_FLOAT, DEPTH_SHORT, DEPTH_BYTE].into_iter().find(|d| depths.is_empty() || depths.iter().any(|x| x == d)).ok_or("supports no pixel depth Zornov offers")?;
        // Described again for the chosen context, on a copy of the
        // descriptor.
        let cd = Effect::with_props(desc.props.copy());
        let in_args = Props::new();
        in_args.set_str(P_CONTEXT, &[context]);
        let st = call(raw, ACTION_DESCRIBE_IN_CONTEXT, cd.handle(), Some(&in_args), None);
        if !ok(st) {
            return Err(format!("could not describe itself as a {} (status {st})", context.trim_start_matches("OfxImageEffectContext")));
        }
        let params = cd.params.params.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|p| param_info(&p.name, &p.kind, &p.props)).collect();
        let clips = cd.clips.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|c| c.name.clone()).collect();
        let label = cd.props.str0(P_LABEL).filter(|l| !l.is_empty()).unwrap_or_else(|| id.clone());
        let info = PluginInfo {
            index,
            id,
            version,
            label,
            grouping: cd.props.str0(P_GROUPING).unwrap_or_default(),
            description: cd.props.str0(P_DESCRIPTION).unwrap_or_default(),
            context: context.to_owned(),
            depth: depth.to_owned(),
            params,
            clips,
        };
        Ok(Plugin { info, raw, desc, ctx_desc: Some(cd), instance: None, began: false })
    }

    fn depth(&self) -> Depth {
        match self.info.depth.as_str() {
            DEPTH_BYTE => Depth::Byte,
            DEPTH_SHORT => Depth::Short,
            _ => Depth::Float,
        }
    }

    fn instance(&mut self, w: usize, h: usize, fps: f64) -> Result<&Effect, String> {
        if self.instance.is_none() {
            let cd = self.ctx_desc.as_ref().ok_or("no context descriptor")?;
            let inst = Effect::instance_of(cd);
            setup_instance(&inst, &self.info.context, w, h, fps, self.depth());
            let st = call(self.raw, ACTION_CREATE_INSTANCE, inst.handle(), None, None);
            if !ok(st) {
                return Err(format!("{} could not start (status {st})", self.info.label));
            }
            // Clip preferences: Studio decides components and depth, but the
            // plugin may want to know.
            let out_args = Props::new();
            for c in &self.info.clips {
                out_args.set_str(&format!("OfxImageClipPropComponents_{c}"), &[RGBA]);
                out_args.set_str(&format!("OfxImageClipPropDepth_{c}"), &[self.depth().name()]);
                out_args.set_dbl(&format!("OfxImageClipPropPAR_{c}"), &[1.0]);
            }
            out_args.set_dbl("OfxImageEffectPropFrameRate", &[fps]);
            out_args.set_str("OfxImageClipPropFieldOrder", &["OfxImageFieldNone"]);
            out_args.set_str("OfxImageEffectPropPreMultiplication", &["OfxImagePreMultiplied"]);
            out_args.set_int("OfxImageClipPropContinuousSamples", &[0]);
            out_args.set_int("OfxImageEffectFrameVarying", &[0]);
            call(self.raw, ACTION_CLIP_PREFERENCES, inst.handle(), None, Some(&out_args));
            self.instance = Some(inst);
        }
        let inst = self.instance.as_ref().expect("made above");
        setup_instance(inst, &self.info.context, w, h, fps, self.depth());
        if !self.began {
            let a = Props::new();
            a.set_dbl("OfxImageEffectPropFrameRange", &[0.0, 1.0e7]);
            a.set_dbl("OfxImageEffectPropFrameStep", &[1.0]);
            a.set_int("OfxPropIsInteractive", &[0]);
            a.set_dbl("OfxImageEffectPropRenderScale", &[1.0, 1.0]);
            a.set_int("OfxImageEffectPropSequentialRenderStatus", &[0]);
            a.set_int("OfxImageEffectPropInteractiveRenderStatus", &[0]);
            call(self.raw, ACTION_BEGIN_SEQUENCE, inst.handle(), Some(&a), None);
            self.began = true;
        }
        Ok(inst)
    }

    /// Renders one frame. `inputs` are top-down premultiplied RGBA floats:
    /// none for a generator, one for a filter, two (from, to) for a
    /// transition. `params` maps parameter names to numbers, lists or
    /// strings.
    #[allow(clippy::too_many_arguments)]
    pub fn render(&mut self, inputs: &[Vec<[f32; 4]>], w: usize, h: usize, frame: f64, fps: f64, progress: f64, params: &serde_json::Map<String, Value>) -> Result<Vec<[f32; 4]>, String> {
        let depth = self.depth();
        let raw = self.raw;
        let context = self.info.context.clone();
        let label = self.info.label.clone();
        let inst = self.instance(w, h, fps)?;
        for (name, v) in params {
            let Some(p) = inst.param(name) else { continue };
            // SAFETY: the param lives in the instance.
            let p = unsafe { &*p };
            let mut val = p.value.lock().unwrap_or_else(|e| e.into_inner());
            let nums: Vec<f64> = match v {
                Value::Array(a) => a.iter().filter_map(Value::as_f64).collect(),
                Value::Bool(b) => vec![*b as u8 as f64],
                Value::Number(n) => vec![n.as_f64().unwrap_or(0.0)],
                _ => Vec::new(),
            };
            match &mut *val {
                Val::Dbl(d) => d.iter_mut().zip(&nums).for_each(|(a, b)| *a = *b),
                Val::Int(d) => d.iter_mut().zip(&nums).for_each(|(a, b)| *a = b.round() as i32),
                Val::Str(d) => {
                    if let Some(t) = v.as_str() {
                        *d = vec![CString::new(t).unwrap_or_default()];
                    }
                }
                Val::Ptr(_) => {}
            }
        }
        if context == CONTEXT_TRANSITION
            && let Some(p) = inst.param("Transition")
        {
            *unsafe { &*p }.value.lock().unwrap_or_else(|e| e.into_inner()) = Val::Dbl(vec![progress]);
        }
        let sources: &[&str] = match context.as_str() {
            CONTEXT_TRANSITION => &["SourceFrom", "SourceTo"],
            CONTEXT_GENERATOR => &[],
            _ => &["Source"],
        };
        for (i, name) in sources.iter().enumerate() {
            let input = inputs.get(i).ok_or_else(|| format!("{label} needs {} pictures", sources.len()))?;
            if let Some(c) = inst.clip(name) {
                *unsafe { &*c }.image.lock().unwrap_or_else(|e| e.into_inner()) = Some(ClipImage::from_rgba(input, w, h, depth));
            }
        }
        let out_clip = inst.clip("Output").ok_or_else(|| format!("{label} has no output"))?;
        let blank = vec![[0f32; 4]; w * h];
        *unsafe { &*out_clip }.image.lock().unwrap_or_else(|e| e.into_inner()) = Some(ClipImage::from_rgba(inputs.first().unwrap_or(&blank), w, h, depth));
        let a = Props::new();
        a.set_dbl(P_TIME, &[frame]);
        a.set_str("OfxImageEffectPropFieldToRender", &["OfxImageFieldNone"]);
        a.set_int("OfxImageEffectPropRenderWindow", &[0, 0, w as i32, h as i32]);
        a.set_dbl("OfxImageEffectPropRenderScale", &[1.0, 1.0]);
        a.set_int("OfxImageEffectPropSequentialRenderStatus", &[0]);
        a.set_int("OfxImageEffectPropInteractiveRenderStatus", &[0]);
        a.set_int("OfxImageEffectPropRenderQualityDraft", &[0]);
        for gpu in ["OfxImageEffectPropOpenGLEnabled", "OfxImageEffectPropMetalEnabled", "OfxImageEffectPropCudaEnabled", "OfxImageEffectPropOpenCLEnabled"] {
            a.set_int(gpu, &[0]);
        }
        let st = call(raw, ACTION_RENDER, inst.handle(), Some(&a), None);
        if st != kOfxStatOK {
            let why = host::MESSAGES.lock().unwrap_or_else(|e| e.into_inner()).last().cloned().unwrap_or_default();
            return Err(format!("{label} failed to render (status {st}){}", if why.is_empty() { String::new() } else { format!(": {why}") }));
        }
        let img = unsafe { &*out_clip }.image.lock().unwrap_or_else(|e| e.into_inner());
        Ok(img.as_ref().map(ClipImage::to_rgba).unwrap_or(blank))
    }
}

impl Drop for Plugin {
    fn drop(&mut self) {
        if let Some(inst) = &self.instance {
            if self.began {
                let a = Props::new();
                a.set_dbl("OfxImageEffectPropFrameRange", &[0.0, 1.0e7]);
                a.set_int("OfxPropIsInteractive", &[0]);
                call(self.raw, ACTION_END_SEQUENCE, inst.handle(), Some(&a), None);
            }
            call(self.raw, ACTION_DESTROY_INSTANCE, inst.handle(), None, None);
        }
        call(self.raw, ACTION_UNLOAD, std::ptr::null_mut(), None, None);
        let _ = &self.desc;
    }
}

fn setup_instance(inst: &Effect, context: &str, w: usize, h: usize, fps: f64, depth: Depth) {
    let (wf, hf) = (w as f64, h as f64);
    let p = &inst.props;
    p.set_str(P_CONTEXT, &[context]);
    p.set_dbl("OfxImageEffectPropProjectSize", &[wf, hf]);
    p.set_dbl("OfxImageEffectPropProjectExtent", &[wf, hf]);
    p.set_dbl("OfxImageEffectPropProjectOffset", &[0.0, 0.0]);
    p.set_dbl("OfxImageEffectPropProjectPixelAspectRatio", &[1.0]);
    p.set_dbl("OfxImageEffectInstancePropEffectDuration", &[1.0e7]);
    p.set_dbl("OfxImageEffectPropFrameRate", &[fps]);
    p.set_int("OfxPropIsInteractive", &[0]);
    p.set_int("OfxImageEffectInstancePropSequentialRender", &[0]);
    if p.get("OfxPropInstanceData").is_none() {
        p.set("OfxPropInstanceData", Val::Ptr(vec![0]));
    }
    for c in inst.clips.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        let cp = &c.props;
        cp.set_str(P_COMPONENTS, &[RGBA]);
        cp.set_str("OfxImageClipPropUnmappedComponents", &[RGBA]);
        cp.set_str(P_DEPTH, &[depth.name()]);
        cp.set_str("OfxImageClipPropUnmappedPixelDepth", &[depth.name()]);
        cp.set_str("OfxImageEffectPropPreMultiplication", &["OfxImagePreMultiplied"]);
        cp.set_dbl("OfxImagePropPixelAspectRatio", &[1.0]);
        cp.set_dbl("OfxImageEffectPropFrameRate", &[fps]);
        cp.set_dbl("OfxImageEffectPropUnmappedFrameRate", &[fps]);
        cp.set_dbl("OfxImageEffectPropFrameRange", &[0.0, 1.0e7]);
        cp.set_dbl("OfxImageEffectPropUnmappedFrameRange", &[0.0, 1.0e7]);
        cp.set_str("OfxImageClipPropFieldOrder", &["OfxImageFieldNone"]);
        cp.set_int("OfxImageClipPropConnected", &[1]);
        cp.set_int("OfxImageClipPropContinuousSamples", &[0]);
        cp.set_dbl("StudioFrameSize", &[wf, hf]);
    }
}

fn param_info(name: &str, kind: &str, p: &Props) -> ParamInfo {
    let first = |k: &str| p.dbls(k).first().copied().filter(|v| v.is_finite() && v.abs() < 1.0e12);
    ParamInfo {
        name: name.to_owned(),
        label: p.str0(P_LABEL).unwrap_or_else(|| name.to_owned()),
        kind: kind.to_owned(),
        default: p.dbls("OfxParamPropDefault"),
        min: first("OfxParamPropDisplayMin").or_else(|| first("OfxParamPropMin")),
        max: first("OfxParamPropDisplayMax").or_else(|| first("OfxParamPropMax")),
        options: p.strs("OfxParamPropChoiceOption"),
        hint: p.str0("OfxParamPropHint").unwrap_or_default(),
        secret: p.int0("OfxParamPropSecret").unwrap_or(0) != 0,
        double_type: p.str0("OfxParamPropDoubleType").unwrap_or_default(),
    }
}
