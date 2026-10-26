// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The OFX host proper: the suites a plugin calls and the actions Studio
//! sends it. This runs inside the OFX worker process (see `worker`), never
//! in the editor, because an OFX plugin is native code with the run of the
//! process.
//!
//! Supported: the image effect API 1.4 in the filter, general (with one
//! source), generator and transition contexts; CPU rendering of whole
//! frames in float, 8-bit or 16-bit RGBA; double, integer, boolean,
//! choice, colour, 2D/3D, string and custom parameters (not animated: a
//! parameter has one value per render). Not supported: overlays and
//! custom interacts, GPU (OpenGL, Metal, CUDA, OpenCL) rendering, tiles
//! and multi-resolution, temporal access to other frames, parametric
//! parameters, progress and timeline suites.

use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_double, c_int, c_uint, c_void};
use std::sync::{Condvar, Mutex};

use super::abi::*;

fn cstr<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        return None;
    }
    // SAFETY: OFX passes NUL-terminated strings; null is checked above.
    unsafe { CStr::from_ptr(p) }.to_str().ok()
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ------------------------------------------------------------ properties

/// One property's values.
#[derive(Clone, Debug)]
pub enum Val {
    Ptr(Vec<usize>),
    Str(Vec<CString>),
    Dbl(Vec<f64>),
    Int(Vec<i32>),
}

const PROPS: u32 = 0x5052_4f50;
const EFFECT: u32 = 0x4546_4654;
const CLIP: u32 = 0x434c_4950;
const PARAM: u32 = 0x5041_524d;
const PARAMSET: u32 = 0x5053_4554;
const MEMORY: u32 = 0x4d45_4d4f;

/// A property set. Handles are pointers to these.
#[repr(C)]
pub struct Props {
    magic: u32,
    map: Mutex<HashMap<String, Val>>,
}

impl Props {
    pub fn new() -> Props {
        Props { magic: PROPS, map: Mutex::new(HashMap::new()) }
    }

    pub fn copy(&self) -> Props {
        Props { magic: PROPS, map: Mutex::new(lock(&self.map).clone()) }
    }

    pub fn handle(&self) -> Handle {
        self as *const Props as Handle
    }

    pub fn set(&self, name: &str, v: Val) {
        lock(&self.map).insert(name.to_owned(), v);
    }

    pub fn set_str(&self, name: &str, v: &[&str]) {
        self.set(name, Val::Str(v.iter().map(|s| CString::new(*s).unwrap_or_default()).collect()));
    }

    pub fn set_int(&self, name: &str, v: &[i32]) {
        self.set(name, Val::Int(v.to_vec()));
    }

    pub fn set_dbl(&self, name: &str, v: &[f64]) {
        self.set(name, Val::Dbl(v.to_vec()));
    }

    pub fn get(&self, name: &str) -> Option<Val> {
        lock(&self.map).get(name).cloned()
    }

    pub fn strs(&self, name: &str) -> Vec<String> {
        match self.get(name) {
            Some(Val::Str(v)) => v.iter().map(|s| s.to_string_lossy().into_owned()).collect(),
            _ => Vec::new(),
        }
    }

    pub fn str0(&self, name: &str) -> Option<String> {
        self.strs(name).into_iter().next()
    }

    pub fn dbls(&self, name: &str) -> Vec<f64> {
        match self.get(name) {
            Some(Val::Dbl(v)) => v,
            Some(Val::Int(v)) => v.iter().map(|x| *x as f64).collect(),
            _ => Vec::new(),
        }
    }

    pub fn int0(&self, name: &str) -> Option<i32> {
        match self.get(name) {
            Some(Val::Int(v)) => v.first().copied(),
            Some(Val::Dbl(v)) => v.first().map(|x| *x as i32),
            _ => None,
        }
    }
}

impl Default for Props {
    fn default() -> Self {
        Self::new()
    }
}

/// # Safety
/// `h` is a handle the host gave out (checked by its tag).
unsafe fn props<'a>(h: Handle) -> Option<&'a Props> {
    if h.is_null() {
        return None;
    }
    // SAFETY: every handle the host gives out starts with a u32 tag.
    let p = unsafe { &*(h as *const Props) };
    (p.magic == PROPS).then_some(p)
}

macro_rules! prop_set {
    ($name:ident, $t:ty, $variant:ident, $conv:expr) => {
        unsafe extern "C" fn $name(h: Handle, name: *const c_char, index: c_int, v: $t) -> OfxStatus {
            let (Some(p), Some(n)) = (unsafe { props(h) }, cstr(name)) else { return kOfxStatErrBadHandle };
            if index < 0 {
                return kOfxStatErrBadIndex;
            }
            let i = index as usize;
            let mut m = lock(&p.map);
            let conv = $conv;
            match m.entry(n.to_owned()).or_insert_with(|| Val::$variant(Vec::new())) {
                Val::$variant(vals) => {
                    if vals.len() <= i {
                        vals.resize(i + 1, Default::default());
                    }
                    vals[i] = conv(v);
                    kOfxStatOK
                }
                other => {
                    // A plugin may change a property's type (rare); follow it.
                    let mut vals = Vec::new();
                    vals.resize(i + 1, Default::default());
                    vals[i] = conv(v);
                    *other = Val::$variant(vals);
                    kOfxStatOK
                }
            }
        }
    };
}

prop_set!(prop_set_pointer, *mut c_void, Ptr, |v: *mut c_void| v as usize);
prop_set!(prop_set_string, *const c_char, Str, |v: *const c_char| CString::new(cstr(v).unwrap_or("")).unwrap_or_default());
prop_set!(prop_set_double, c_double, Dbl, |v: f64| v);
prop_set!(prop_set_int, c_int, Int, |v: i32| v);

macro_rules! prop_set_n {
    ($name:ident, $t:ty, $one:ident) => {
        unsafe extern "C" fn $name(h: Handle, name: *const c_char, count: c_int, v: *const $t) -> OfxStatus {
            if v.is_null() || count < 0 {
                return kOfxStatErrValue;
            }
            for i in 0..count {
                // SAFETY: the plugin passes `count` values.
                let s = unsafe { $one(h, name, i, *v.add(i as usize)) };
                if s != kOfxStatOK {
                    return s;
                }
            }
            kOfxStatOK
        }
    };
}

prop_set_n!(prop_set_pointer_n, *mut c_void, prop_set_pointer);
prop_set_n!(prop_set_string_n, *const c_char, prop_set_string);
prop_set_n!(prop_set_double_n, c_double, prop_set_double);
prop_set_n!(prop_set_int_n, c_int, prop_set_int);

unsafe extern "C" fn prop_get_pointer(h: Handle, name: *const c_char, index: c_int, out: *mut *mut c_void) -> OfxStatus {
    let (Some(p), Some(n)) = (unsafe { props(h) }, cstr(name)) else { return kOfxStatErrBadHandle };
    match lock(&p.map).get(n) {
        Some(Val::Ptr(v)) => match v.get(index as usize) {
            Some(x) if index >= 0 && !out.is_null() => {
                // SAFETY: checked non-null.
                unsafe { *out = *x as *mut c_void };
                kOfxStatOK
            }
            _ => kOfxStatErrBadIndex,
        },
        Some(_) => kOfxStatErrValue,
        None => kOfxStatErrUnknown,
    }
}

unsafe extern "C" fn prop_get_string(h: Handle, name: *const c_char, index: c_int, out: *mut *mut c_char) -> OfxStatus {
    let (Some(p), Some(n)) = (unsafe { props(h) }, cstr(name)) else { return kOfxStatErrBadHandle };
    match lock(&p.map).get(n) {
        // The string lives in the set until the property changes, as OFX
        // requires.
        Some(Val::Str(v)) => match v.get(index as usize) {
            Some(x) if index >= 0 && !out.is_null() => {
                unsafe { *out = x.as_ptr() as *mut c_char };
                kOfxStatOK
            }
            _ => kOfxStatErrBadIndex,
        },
        Some(_) => kOfxStatErrValue,
        None => kOfxStatErrUnknown,
    }
}

unsafe extern "C" fn prop_get_double(h: Handle, name: *const c_char, index: c_int, out: *mut c_double) -> OfxStatus {
    let (Some(p), Some(n)) = (unsafe { props(h) }, cstr(name)) else { return kOfxStatErrBadHandle };
    let v = match lock(&p.map).get(n) {
        Some(Val::Dbl(v)) => v.get(index as usize).copied(),
        Some(Val::Int(v)) => v.get(index as usize).map(|x| *x as f64),
        Some(_) => return kOfxStatErrValue,
        None => return kOfxStatErrUnknown,
    };
    match v {
        Some(x) if index >= 0 && !out.is_null() => {
            unsafe { *out = x };
            kOfxStatOK
        }
        _ => kOfxStatErrBadIndex,
    }
}

unsafe extern "C" fn prop_get_int(h: Handle, name: *const c_char, index: c_int, out: *mut c_int) -> OfxStatus {
    let (Some(p), Some(n)) = (unsafe { props(h) }, cstr(name)) else { return kOfxStatErrBadHandle };
    let v = match lock(&p.map).get(n) {
        Some(Val::Int(v)) => v.get(index as usize).copied(),
        Some(Val::Dbl(v)) => v.get(index as usize).map(|x| *x as i32),
        Some(_) => return kOfxStatErrValue,
        None => return kOfxStatErrUnknown,
    };
    match v {
        Some(x) if index >= 0 && !out.is_null() => {
            unsafe { *out = x };
            kOfxStatOK
        }
        _ => kOfxStatErrBadIndex,
    }
}

macro_rules! prop_get_n {
    ($name:ident, $t:ty, $one:ident) => {
        unsafe extern "C" fn $name(h: Handle, name: *const c_char, count: c_int, out: *mut $t) -> OfxStatus {
            if out.is_null() || count < 0 {
                return kOfxStatErrValue;
            }
            for i in 0..count {
                let s = unsafe { $one(h, name, i, out.add(i as usize)) };
                if s != kOfxStatOK {
                    return s;
                }
            }
            kOfxStatOK
        }
    };
}

prop_get_n!(prop_get_pointer_n, *mut c_void, prop_get_pointer);
prop_get_n!(prop_get_string_n, *mut c_char, prop_get_string);
prop_get_n!(prop_get_double_n, c_double, prop_get_double);
prop_get_n!(prop_get_int_n, c_int, prop_get_int);

unsafe extern "C" fn prop_reset(h: Handle, name: *const c_char) -> OfxStatus {
    let (Some(p), Some(n)) = (unsafe { props(h) }, cstr(name)) else { return kOfxStatErrBadHandle };
    if lock(&p.map).contains_key(n) { kOfxStatOK } else { kOfxStatErrUnknown }
}

unsafe extern "C" fn prop_get_dimension(h: Handle, name: *const c_char, out: *mut c_int) -> OfxStatus {
    let (Some(p), Some(n)) = (unsafe { props(h) }, cstr(name)) else { return kOfxStatErrBadHandle };
    let d = match lock(&p.map).get(n) {
        Some(Val::Ptr(v)) => v.len(),
        Some(Val::Str(v)) => v.len(),
        Some(Val::Dbl(v)) => v.len(),
        Some(Val::Int(v)) => v.len(),
        None => return kOfxStatErrUnknown,
    };
    if !out.is_null() {
        unsafe { *out = d as c_int };
    }
    kOfxStatOK
}

static PROPERTY_SUITE: OfxPropertySuiteV1 = OfxPropertySuiteV1 {
    prop_set_pointer,
    prop_set_string,
    prop_set_double,
    prop_set_int,
    prop_set_pointer_n,
    prop_set_string_n,
    prop_set_double_n,
    prop_set_int_n,
    prop_get_pointer,
    prop_get_string,
    prop_get_double,
    prop_get_int,
    prop_get_pointer_n,
    prop_get_string_n,
    prop_get_double_n,
    prop_get_int_n,
    prop_reset,
    prop_get_dimension,
};

// --------------------------------------------------------- image effects

/// Pixel depth of the buffers handed to a plugin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    Float,
    Short,
    Byte,
}

impl Depth {
    pub fn name(self) -> &'static str {
        match self {
            Depth::Float => DEPTH_FLOAT,
            Depth::Short => DEPTH_SHORT,
            Depth::Byte => DEPTH_BYTE,
        }
    }
    fn bytes(self) -> usize {
        match self {
            Depth::Float => 4,
            Depth::Short => 2,
            Depth::Byte => 1,
        }
    }
}

/// The picture on a clip for the current render: rows bottom to top, as
/// OFX lays them out.
pub struct ClipImage {
    pub width: usize,
    pub height: usize,
    pub depth: Depth,
    pub data: Vec<u8>,
}

impl ClipImage {
    /// From top-down premultiplied RGBA floats.
    pub fn from_rgba(px: &[[f32; 4]], width: usize, height: usize, depth: Depth) -> ClipImage {
        let row = width * 4 * depth.bytes();
        let mut data = vec![0u8; row * height];
        for y in 0..height {
            let dst = &mut data[(height - 1 - y) * row..(height - y) * row];
            for x in 0..width {
                let p = px[y * width + x];
                for c in 0..4 {
                    let o = (x * 4 + c) * depth.bytes();
                    match depth {
                        Depth::Float => dst[o..o + 4].copy_from_slice(&p[c].to_ne_bytes()),
                        Depth::Short => dst[o..o + 2].copy_from_slice(&((p[c].clamp(0.0, 1.0) * 65535.0 + 0.5) as u16).to_ne_bytes()),
                        Depth::Byte => dst[o] = (p[c].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                    }
                }
            }
        }
        ClipImage { width, height, depth, data }
    }

    /// Back to top-down RGBA floats.
    pub fn to_rgba(&self) -> Vec<[f32; 4]> {
        let (w, h, d) = (self.width, self.height, self.depth);
        let row = w * 4 * d.bytes();
        let mut out = vec![[0f32; 4]; w * h];
        for y in 0..h {
            let src = &self.data[(h - 1 - y) * row..(h - y) * row];
            for x in 0..w {
                for c in 0..4 {
                    let o = (x * 4 + c) * d.bytes();
                    out[y * w + x][c] = match d {
                        Depth::Float => f32::from_ne_bytes(src[o..o + 4].try_into().unwrap_or([0; 4])),
                        Depth::Short => u16::from_ne_bytes([src[o], src[o + 1]]) as f32 / 65535.0,
                        Depth::Byte => src[o] as f32 / 255.0,
                    };
                }
            }
        }
        out
    }
}

#[repr(C)]
pub struct Clip {
    magic: u32,
    pub name: String,
    pub props: Props,
    pub image: Mutex<Option<ClipImage>>,
}

#[repr(C)]
pub struct Param {
    magic: u32,
    pub name: String,
    pub kind: String,
    pub props: Props,
    pub value: Mutex<Val>,
}

#[repr(C)]
pub struct ParamSet {
    magic: u32,
    pub props: Props,
    pub params: Mutex<Vec<Box<Param>>>,
}

/// A descriptor or an instance.
#[repr(C)]
pub struct Effect {
    magic: u32,
    pub props: Props,
    pub params: ParamSet,
    pub clips: Mutex<Vec<Box<Clip>>>,
}

impl Effect {
    pub fn new() -> Box<Effect> {
        Box::new(Effect { magic: EFFECT, props: Props::new(), params: ParamSet { magic: PARAMSET, props: Props::new(), params: Mutex::new(Vec::new()) }, clips: Mutex::new(Vec::new()) })
    }

    /// A descriptor that starts with these properties (the per-context
    /// copy of a plugin's descriptor).
    pub fn with_props(props: Props) -> Box<Effect> {
        let mut e = Effect::new();
        e.props = props;
        e
    }

    pub fn handle(&self) -> Handle {
        self as *const Effect as Handle
    }

    /// An instance made from a context descriptor: its clips, and its
    /// parameters at their defaults.
    pub fn instance_of(desc: &Effect) -> Box<Effect> {
        let e = Effect::new();
        for c in lock(&desc.clips).iter() {
            lock(&e.clips).push(Box::new(Clip { magic: CLIP, name: c.name.clone(), props: c.props.copy(), image: Mutex::new(None) }));
        }
        for p in lock(&desc.params.params).iter() {
            let props = p.props.copy();
            let value = default_value(&p.kind, &props);
            lock(&e.params.params).push(Box::new(Param { magic: PARAM, name: p.name.clone(), kind: p.kind.clone(), props, value: Mutex::new(value) }));
        }
        e
    }

    pub fn clip(&self, name: &str) -> Option<*const Clip> {
        lock(&self.clips).iter().find(|c| c.name == name).map(|c| &**c as *const Clip)
    }

    pub fn param(&self, name: &str) -> Option<*const Param> {
        lock(&self.params.params).iter().find(|p| p.name == name).map(|p| &**p as *const Param)
    }
}

/// A parameter's value at its default (`kOfxParamPropDefault`).
pub fn default_value(kind: &str, props: &Props) -> Val {
    let n = match kind {
        PARAM_RGBA => 4,
        PARAM_RGB | PARAM_DOUBLE3D | PARAM_INT3D => 3,
        PARAM_DOUBLE2D | PARAM_INT2D => 2,
        _ => 1,
    };
    match kind {
        PARAM_STRING | PARAM_CUSTOM | PARAM_STRCHOICE => Val::Str(vec![CString::new(props.str0("OfxParamPropDefault").unwrap_or_default()).unwrap_or_default()]),
        PARAM_INT | PARAM_BOOL | PARAM_CHOICE | PARAM_INT2D | PARAM_INT3D => {
            let mut v: Vec<i32> = props.dbls("OfxParamPropDefault").iter().map(|x| *x as i32).collect();
            v.resize(n, 0);
            Val::Int(v)
        }
        _ => {
            let mut v = props.dbls("OfxParamPropDefault");
            v.resize(n, if kind == PARAM_RGBA { 1.0 } else { 0.0 });
            Val::Dbl(v)
        }
    }
}

unsafe fn effect<'a>(h: Handle) -> Option<&'a Effect> {
    if h.is_null() {
        return None;
    }
    let e = unsafe { &*(h as *const Effect) };
    (e.magic == EFFECT).then_some(e)
}

unsafe fn clip<'a>(h: Handle) -> Option<&'a Clip> {
    if h.is_null() {
        return None;
    }
    let c = unsafe { &*(h as *const Clip) };
    (c.magic == CLIP).then_some(c)
}

unsafe fn param<'a>(h: Handle) -> Option<&'a Param> {
    if h.is_null() {
        return None;
    }
    let p = unsafe { &*(h as *const Param) };
    (p.magic == PARAM).then_some(p)
}

unsafe fn param_set<'a>(h: Handle) -> Option<&'a ParamSet> {
    if h.is_null() {
        return None;
    }
    let p = unsafe { &*(h as *const ParamSet) };
    (p.magic == PARAMSET).then_some(p)
}

fn out<T>(dst: *mut T, v: T) {
    if !dst.is_null() {
        // SAFETY: checked non-null; the plugin passes room for one value.
        unsafe { *dst = v };
    }
}

unsafe extern "C" fn get_property_set(h: Handle, o: *mut Handle) -> OfxStatus {
    let Some(e) = (unsafe { effect(h) }) else { return kOfxStatErrBadHandle };
    out(o, e.props.handle());
    kOfxStatOK
}

unsafe extern "C" fn get_param_set(h: Handle, o: *mut Handle) -> OfxStatus {
    let Some(e) = (unsafe { effect(h) }) else { return kOfxStatErrBadHandle };
    out(o, &e.params as *const ParamSet as Handle);
    kOfxStatOK
}

unsafe extern "C" fn clip_define(h: Handle, name: *const c_char, o: *mut Handle) -> OfxStatus {
    let (Some(e), Some(n)) = (unsafe { effect(h) }, cstr(name)) else { return kOfxStatErrBadHandle };
    let mut clips = lock(&e.clips);
    if let Some(c) = clips.iter().find(|c| c.name == n) {
        out(o, c.props.handle());
        return kOfxStatOK;
    }
    let c = Box::new(Clip { magic: CLIP, name: n.to_owned(), props: Props::new(), image: Mutex::new(None) });
    c.props.set_str(P_NAME, &[n]);
    c.props.set_str(P_LABEL, &[n]);
    c.props.set_str("OfxImageEffectPropSupportedComponents", &[RGBA]);
    c.props.set_int("OfxImageClipPropOptional", &[0]);
    out(o, c.props.handle());
    clips.push(c);
    kOfxStatOK
}

unsafe extern "C" fn clip_get_handle(h: Handle, name: *const c_char, o: *mut Handle, props_out: *mut Handle) -> OfxStatus {
    let (Some(e), Some(n)) = (unsafe { effect(h) }, cstr(name)) else { return kOfxStatErrBadHandle };
    let clips = lock(&e.clips);
    let Some(c) = clips.iter().find(|c| c.name == n) else { return kOfxStatErrBadIndex };
    out(o, &**c as *const Clip as Handle);
    out(props_out, c.props.handle());
    kOfxStatOK
}

unsafe extern "C" fn clip_get_property_set(h: Handle, o: *mut Handle) -> OfxStatus {
    let Some(c) = (unsafe { clip(h) }) else { return kOfxStatErrBadHandle };
    out(o, c.props.handle());
    kOfxStatOK
}

unsafe extern "C" fn clip_get_image(h: Handle, _time: OfxTime, _region: *const OfxRectD, o: *mut Handle) -> OfxStatus {
    let Some(c) = (unsafe { clip(h) }) else { return kOfxStatErrBadHandle };
    let mut img = lock(&c.image);
    let Some(im) = img.as_mut() else { return kOfxStatFailed };
    let p = Box::new(Props::new());
    let (w, hh) = (im.width as i32, im.height as i32);
    p.set(IMAGE_PROP_DATA, Val::Ptr(vec![im.data.as_mut_ptr() as usize]));
    p.set_int("OfxImagePropBounds", &[0, 0, w, hh]);
    p.set_int("OfxImagePropRegionOfDefinition", &[0, 0, w, hh]);
    p.set_int("OfxImagePropRowBytes", &[w * 4 * im.depth.bytes() as i32]);
    p.set_str("OfxImagePropField", &["OfxImageFieldNone"]);
    p.set_str("OfxImagePropUniqueIdentifier", &[&format!("{}-{:p}", c.name, im.data.as_ptr())]);
    p.set_dbl("OfxImageEffectPropRenderScale", &[1.0, 1.0]);
    p.set_dbl("OfxImagePropPixelAspectRatio", &[1.0]);
    p.set_str(P_COMPONENTS, &[RGBA]);
    p.set_str(P_DEPTH, &[im.depth.name()]);
    p.set_str("OfxImageEffectPropPreMultiplication", &["OfxImagePreMultiplied"]);
    out(o, Box::into_raw(p) as Handle);
    kOfxStatOK
}

const IMAGE_PROP_DATA: &str = "OfxImagePropData";

unsafe extern "C" fn clip_release_image(h: Handle) -> OfxStatus {
    if unsafe { props(h) }.is_none() {
        return kOfxStatErrBadHandle;
    }
    // SAFETY: images are the boxes `clip_get_image` handed out.
    drop(unsafe { Box::from_raw(h as *mut Props) });
    kOfxStatOK
}

unsafe extern "C" fn clip_get_region_of_definition(h: Handle, _time: OfxTime, o: *mut OfxRectD) -> OfxStatus {
    let Some(c) = (unsafe { clip(h) }) else { return kOfxStatErrBadHandle };
    let size = c.props.dbls("StudioFrameSize");
    let (w, hh) = (size.first().copied().unwrap_or(0.0), size.get(1).copied().unwrap_or(0.0));
    out(o, OfxRectD { x1: 0.0, y1: 0.0, x2: w, y2: hh });
    kOfxStatOK
}

unsafe extern "C" fn abort(_h: Handle) -> c_int {
    0
}

#[repr(C)]
struct Memory {
    magic: u32,
    data: Vec<u8>,
}

unsafe extern "C" fn image_memory_alloc(_h: Handle, n: usize, o: *mut Handle) -> OfxStatus {
    let mut data = Vec::new();
    if data.try_reserve_exact(n).is_err() {
        return kOfxStatErrMemory;
    }
    data.resize(n, 0);
    out(o, Box::into_raw(Box::new(Memory { magic: MEMORY, data })) as Handle);
    kOfxStatOK
}

unsafe fn memory<'a>(h: Handle) -> Option<&'a mut Memory> {
    if h.is_null() {
        return None;
    }
    let m = unsafe { &mut *(h as *mut Memory) };
    (m.magic == MEMORY).then_some(m)
}

unsafe extern "C" fn image_memory_free(h: Handle) -> OfxStatus {
    if unsafe { memory(h) }.is_none() {
        return kOfxStatErrBadHandle;
    }
    drop(unsafe { Box::from_raw(h as *mut Memory) });
    kOfxStatOK
}

unsafe extern "C" fn image_memory_lock(h: Handle, o: *mut *mut c_void) -> OfxStatus {
    let Some(m) = (unsafe { memory(h) }) else { return kOfxStatErrBadHandle };
    out(o, m.data.as_mut_ptr() as *mut c_void);
    kOfxStatOK
}

unsafe extern "C" fn image_memory_unlock(h: Handle) -> OfxStatus {
    if unsafe { memory(h) }.is_none() { kOfxStatErrBadHandle } else { kOfxStatOK }
}

static IMAGE_EFFECT_SUITE: OfxImageEffectSuiteV1 = OfxImageEffectSuiteV1 {
    get_property_set,
    get_param_set,
    clip_define,
    clip_get_handle,
    clip_get_property_set,
    clip_get_image,
    clip_release_image,
    clip_get_region_of_definition,
    abort,
    image_memory_alloc,
    image_memory_free,
    image_memory_lock,
    image_memory_unlock,
};

// ------------------------------------------------------------ parameters

unsafe extern "C" fn param_define(set: Handle, kind: *const c_char, name: *const c_char, o: *mut Handle) -> OfxStatus {
    let (Some(s), Some(k), Some(n)) = (unsafe { param_set(set) }, cstr(kind), cstr(name)) else { return kOfxStatErrBadHandle };
    let mut params = lock(&s.params);
    if params.iter().any(|p| p.name == n) {
        return kOfxStatErrExists;
    }
    let props = Props::new();
    props.set_str(P_NAME, &[n]);
    props.set_str(P_LABEL, &[n]);
    props.set_str("OfxParamPropType", &[k]);
    props.set_int("OfxParamPropAnimates", &[0]);
    let value = default_value(k, &props);
    let p = Box::new(Param { magic: PARAM, name: n.to_owned(), kind: k.to_owned(), props, value: Mutex::new(value) });
    out(o, p.props.handle());
    params.push(p);
    kOfxStatOK
}

unsafe extern "C" fn param_get_handle(set: Handle, name: *const c_char, o: *mut Handle, props_out: *mut Handle) -> OfxStatus {
    let (Some(s), Some(n)) = (unsafe { param_set(set) }, cstr(name)) else { return kOfxStatErrBadHandle };
    let params = lock(&s.params);
    let Some(p) = params.iter().find(|p| p.name == n) else { return kOfxStatErrUnknown };
    out(o, &**p as *const Param as Handle);
    out(props_out, p.props.handle());
    kOfxStatOK
}

unsafe extern "C" fn param_set_get_property_set(set: Handle, o: *mut Handle) -> OfxStatus {
    let Some(s) = (unsafe { param_set(set) }) else { return kOfxStatErrBadHandle };
    out(o, s.props.handle());
    kOfxStatOK
}

unsafe extern "C" fn param_get_property_set(h: Handle, o: *mut Handle) -> OfxStatus {
    let Some(p) = (unsafe { param(h) }) else { return kOfxStatErrBadHandle };
    out(o, p.props.handle());
    kOfxStatOK
}

/// Writes a parameter's value through the pointers in `args`.
unsafe fn get_into(p: &Param, args: &mut std::ffi::VaList, scale: f64) -> OfxStatus {
    match &*lock(&p.value) {
        Val::Dbl(v) => {
            for x in v {
                let dst = unsafe { args.next_arg::<*mut c_double>() };
                out(dst, x * scale);
            }
        }
        Val::Int(v) => {
            for x in v {
                let dst = unsafe { args.next_arg::<*mut c_int>() };
                out(dst, (*x as f64 * scale) as c_int);
            }
        }
        Val::Str(v) => {
            let dst = unsafe { args.next_arg::<*mut *const c_char>() };
            out(dst, v.first().map_or(c"".as_ptr(), |s| s.as_ptr()));
        }
        Val::Ptr(_) => return kOfxStatErrUnsupported,
    }
    kOfxStatOK
}

unsafe extern "C" fn param_get_value(h: Handle, mut args: ...) -> OfxStatus {
    let Some(p) = (unsafe { param(h) }) else { return kOfxStatErrBadHandle };
    unsafe { get_into(p, &mut args, 1.0) }
}

unsafe extern "C" fn param_get_value_at_time(h: Handle, _t: OfxTime, mut args: ...) -> OfxStatus {
    let Some(p) = (unsafe { param(h) }) else { return kOfxStatErrBadHandle };
    unsafe { get_into(p, &mut args, 1.0) }
}

unsafe extern "C" fn param_get_derivative(h: Handle, _t: OfxTime, mut args: ...) -> OfxStatus {
    let Some(p) = (unsafe { param(h) }) else { return kOfxStatErrBadHandle };
    // Values do not change over a render, so the slope is zero.
    unsafe { get_into(p, &mut args, 0.0) }
}

unsafe extern "C" fn param_get_integral(h: Handle, t1: OfxTime, t2: OfxTime, mut args: ...) -> OfxStatus {
    let Some(p) = (unsafe { param(h) }) else { return kOfxStatErrBadHandle };
    unsafe { get_into(p, &mut args, t2 - t1) }
}

unsafe fn set_from(p: &Param, args: &mut std::ffi::VaList) -> OfxStatus {
    let mut v = lock(&p.value);
    match &mut *v {
        Val::Dbl(vals) => {
            for x in vals.iter_mut() {
                *x = unsafe { args.next_arg::<c_double>() };
            }
        }
        Val::Int(vals) => {
            for x in vals.iter_mut() {
                *x = unsafe { args.next_arg::<c_int>() };
            }
        }
        Val::Str(vals) => {
            let s = unsafe { args.next_arg::<*const c_char>() };
            *vals = vec![CString::new(cstr(s).unwrap_or("")).unwrap_or_default()];
        }
        Val::Ptr(_) => return kOfxStatErrUnsupported,
    }
    kOfxStatOK
}

unsafe extern "C" fn param_set_value(h: Handle, mut args: ...) -> OfxStatus {
    let Some(p) = (unsafe { param(h) }) else { return kOfxStatErrBadHandle };
    unsafe { set_from(p, &mut args) }
}

unsafe extern "C" fn param_set_value_at_time(h: Handle, _t: OfxTime, mut args: ...) -> OfxStatus {
    let Some(p) = (unsafe { param(h) }) else { return kOfxStatErrBadHandle };
    unsafe { set_from(p, &mut args) }
}

unsafe extern "C" fn param_get_num_keys(_h: Handle, n: *mut c_uint) -> OfxStatus {
    out(n, 0);
    kOfxStatOK
}

unsafe extern "C" fn param_get_key_time(_h: Handle, _n: c_uint, _t: *mut OfxTime) -> OfxStatus {
    kOfxStatErrBadIndex
}

unsafe extern "C" fn param_get_key_index(_h: Handle, _t: OfxTime, _d: c_int, _i: *mut c_int) -> OfxStatus {
    kOfxStatFailed
}

unsafe extern "C" fn param_delete_key(_h: Handle, _t: OfxTime) -> OfxStatus {
    kOfxStatErrBadIndex
}

unsafe extern "C" fn param_delete_all_keys(_h: Handle) -> OfxStatus {
    kOfxStatOK
}

unsafe extern "C" fn param_copy(to: Handle, from: Handle, _o: OfxTime, _r: *const OfxRangeD) -> OfxStatus {
    let (Some(a), Some(b)) = (unsafe { param(to) }, unsafe { param(from) }) else { return kOfxStatErrBadHandle };
    if a.kind != b.kind {
        return kOfxStatErrValue;
    }
    let v = lock(&b.value).clone();
    *lock(&a.value) = v;
    kOfxStatOK
}

unsafe extern "C" fn param_edit_begin(_s: Handle, _n: *const c_char) -> OfxStatus {
    kOfxStatOK
}

unsafe extern "C" fn param_edit_end(_s: Handle) -> OfxStatus {
    kOfxStatOK
}

static PARAMETER_SUITE: OfxParameterSuiteV1 = OfxParameterSuiteV1 {
    param_define,
    param_get_handle,
    param_set_get_property_set,
    param_get_property_set,
    param_get_value,
    param_get_value_at_time,
    param_get_derivative,
    param_get_integral,
    param_set_value,
    param_set_value_at_time,
    param_get_num_keys,
    param_get_key_time,
    param_get_key_index,
    param_delete_key,
    param_delete_all_keys,
    param_copy,
    param_edit_begin,
    param_edit_end,
};

// ---------------------------------------------- memory, threads, messages

unsafe extern "C" fn memory_alloc(_h: Handle, n: usize, o: *mut *mut c_void) -> OfxStatus {
    // A 16-byte header keeps the size for `memory_free`.
    let Ok(layout) = std::alloc::Layout::from_size_align(n + 16, 16) else { return kOfxStatErrMemory };
    let p = unsafe { std::alloc::alloc(layout) };
    if p.is_null() {
        return kOfxStatErrMemory;
    }
    unsafe { (p as *mut usize).write(n) };
    out(o, unsafe { p.add(16) } as *mut c_void);
    kOfxStatOK
}

unsafe extern "C" fn memory_free(p: *mut c_void) -> OfxStatus {
    if p.is_null() {
        return kOfxStatOK;
    }
    let base = unsafe { (p as *mut u8).sub(16) };
    let n = unsafe { (base as *const usize).read() };
    let Ok(layout) = std::alloc::Layout::from_size_align(n + 16, 16) else { return kOfxStatErrBadHandle };
    unsafe { std::alloc::dealloc(base, layout) };
    kOfxStatOK
}

static MEMORY_SUITE: OfxMemorySuiteV1 = OfxMemorySuiteV1 { memory_alloc, memory_free };

thread_local! {
    static THREAD_INDEX: Cell<Option<u32>> = const { Cell::new(None) };
}

struct SendPtr(*mut c_void);
// SAFETY: the plugin asked for its argument to be shared between its
// threads; that is the contract of multiThread.
unsafe impl Send for SendPtr {}
unsafe impl Sync for SendPtr {}

unsafe extern "C" fn multi_thread(f: OfxThreadFunctionV1, n: c_uint, arg: *mut c_void) -> OfxStatus {
    let n = if n == 0 { cpus() } else { n };
    let arg = SendPtr(arg);
    let arg = &arg;
    std::thread::scope(|s| {
        for i in 0..n {
            s.spawn(move || {
                THREAD_INDEX.with(|t| t.set(Some(i)));
                // SAFETY: calling the plugin's own thread function.
                unsafe { f(i, n, arg.0) };
            });
        }
    });
    kOfxStatOK
}

fn cpus() -> u32 {
    std::thread::available_parallelism().map_or(4, |n| n.get() as u32)
}

unsafe extern "C" fn multi_thread_num_cpus(o: *mut c_uint) -> OfxStatus {
    out(o, cpus());
    kOfxStatOK
}

unsafe extern "C" fn multi_thread_index(o: *mut c_uint) -> OfxStatus {
    out(o, THREAD_INDEX.with(|t| t.get()).unwrap_or(0));
    kOfxStatOK
}

unsafe extern "C" fn multi_thread_is_spawned_thread() -> c_int {
    THREAD_INDEX.with(|t| t.get()).is_some() as c_int
}

/// A recursive mutex, as OFX mutexes are.
struct RMutex {
    state: Mutex<(Option<std::thread::ThreadId>, u32)>,
    cv: Condvar,
}

unsafe extern "C" fn mutex_create(o: *mut Handle, count: c_int) -> OfxStatus {
    let me = std::thread::current().id();
    let m = Box::new(RMutex { state: Mutex::new(if count > 0 { (Some(me), count as u32) } else { (None, 0) }), cv: Condvar::new() });
    out(o, Box::into_raw(m) as Handle);
    kOfxStatOK
}

unsafe extern "C" fn mutex_destroy(h: Handle) -> OfxStatus {
    if h.is_null() {
        return kOfxStatErrBadHandle;
    }
    drop(unsafe { Box::from_raw(h as *mut RMutex) });
    kOfxStatOK
}

unsafe extern "C" fn mutex_lock(h: Handle) -> OfxStatus {
    if h.is_null() {
        return kOfxStatErrBadHandle;
    }
    let m = unsafe { &*(h as *const RMutex) };
    let me = std::thread::current().id();
    let mut s = lock(&m.state);
    while s.0.is_some_and(|o| o != me) {
        s = m.cv.wait(s).unwrap_or_else(|e| e.into_inner());
    }
    s.0 = Some(me);
    s.1 += 1;
    kOfxStatOK
}

unsafe extern "C" fn mutex_unlock(h: Handle) -> OfxStatus {
    if h.is_null() {
        return kOfxStatErrBadHandle;
    }
    let m = unsafe { &*(h as *const RMutex) };
    let mut s = lock(&m.state);
    if s.1 > 0 {
        s.1 -= 1;
    }
    if s.1 == 0 {
        s.0 = None;
        m.cv.notify_one();
    }
    kOfxStatOK
}

unsafe extern "C" fn mutex_try_lock(h: Handle) -> OfxStatus {
    if h.is_null() {
        return kOfxStatErrBadHandle;
    }
    let m = unsafe { &*(h as *const RMutex) };
    let me = std::thread::current().id();
    let mut s = lock(&m.state);
    if s.0.is_some_and(|o| o != me) {
        return kOfxStatFailed;
    }
    s.0 = Some(me);
    s.1 += 1;
    kOfxStatOK
}

static THREAD_SUITE: OfxMultiThreadSuiteV1 = OfxMultiThreadSuiteV1 {
    multi_thread,
    multi_thread_num_cpus,
    multi_thread_index,
    multi_thread_is_spawned_thread,
    mutex_create,
    mutex_destroy,
    mutex_lock,
    mutex_unlock,
    mutex_try_lock,
};

/// Messages plugins post, newest last; the worker reports them.
pub static MESSAGES: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn note(kind: *const c_char, format: *const c_char) {
    let line = format!("{}: {}", cstr(kind).unwrap_or("message").trim_start_matches("OfxMessage"), cstr(format).unwrap_or(""));
    let mut m = lock(&MESSAGES);
    m.push(line);
    if m.len() > 100 {
        m.remove(0);
    }
}

// The format arguments are not expanded: the format string alone says
// what happened, and reading arguments of unknown types is unsafe.
unsafe extern "C" fn message(_h: Handle, kind: *const c_char, _id: *const c_char, format: *const c_char, _args: ...) -> OfxStatus {
    note(kind, format);
    kOfxStatOK
}

unsafe extern "C" fn clear_persistent_message(_h: Handle) -> OfxStatus {
    kOfxStatOK
}

static MESSAGE_SUITE_V1: OfxMessageSuiteV1 = OfxMessageSuiteV1 { message };
static MESSAGE_SUITE_V2: OfxMessageSuiteV2 = OfxMessageSuiteV2 { message, set_persistent_message: message, clear_persistent_message };

// ------------------------------------------------------------------ host

unsafe extern "C" fn fetch_suite(_host: Handle, name: *const c_char, version: c_int) -> *const c_void {
    match (cstr(name).unwrap_or(""), version) {
        ("OfxPropertySuite", 1) => &PROPERTY_SUITE as *const _ as *const c_void,
        ("OfxImageEffectSuite", 1) => &IMAGE_EFFECT_SUITE as *const _ as *const c_void,
        ("OfxParameterSuite", 1) => &PARAMETER_SUITE as *const _ as *const c_void,
        ("OfxMemorySuite", 1) => &MEMORY_SUITE as *const _ as *const c_void,
        ("OfxMultiThreadSuite", 1) => &THREAD_SUITE as *const _ as *const c_void,
        ("OfxMessageSuite", 1) => &MESSAGE_SUITE_V1 as *const _ as *const c_void,
        ("OfxMessageSuite", 2) => &MESSAGE_SUITE_V2 as *const _ as *const c_void,
        _ => std::ptr::null(),
    }
}

/// What Studio tells plugins about itself.
pub fn host_props() -> Props {
    let p = Props::new();
    p.set_str(P_NAME, &["com.ashrox.studio"]);
    p.set_str(P_LABEL, &["Zornov"]);
    p.set_int("OfxPropAPIVersion", &[1, 4]);
    p.set_int("OfxPropVersion", &[0, 1, 0]);
    p.set_str("OfxPropVersionLabel", &[env!("CARGO_PKG_VERSION")]);
    p.set_int("OfxImageEffectHostPropIsBackground", &[1]);
    p.set_int("OfxImageEffectPropSupportsOverlays", &[0]);
    p.set_int("OfxImageEffectPropSupportsMultiResolution", &[0]);
    p.set_int("OfxImageEffectPropSupportsTiles", &[0]);
    p.set_int("OfxImageEffectPropTemporalClipAccess", &[0]);
    p.set_str("OfxImageEffectPropSupportedComponents", &[RGBA]);
    p.set_str(P_CONTEXTS, &[CONTEXT_FILTER, CONTEXT_GENERAL, CONTEXT_GENERATOR, CONTEXT_TRANSITION]);
    p.set_str(P_DEPTHS, &[DEPTH_FLOAT, DEPTH_SHORT, DEPTH_BYTE]);
    p.set_int("OfxImageEffectPropSupportsMultipleClipDepths", &[0]);
    p.set_int("OfxImageEffectPropSupportsMultipleClipPARs", &[0]);
    p.set_int("OfxImageEffectPropSetableFrameRate", &[0]);
    p.set_int("OfxImageEffectPropSetableFielding", &[0]);
    p.set_int("OfxParamHostPropSupportsCustomInteract", &[0]);
    p.set_int("OfxParamHostPropSupportsStringAnimation", &[0]);
    p.set_int("OfxParamHostPropSupportsChoiceAnimation", &[0]);
    p.set_int("OfxParamHostPropSupportsBooleanAnimation", &[0]);
    p.set_int("OfxParamHostPropSupportsCustomAnimation", &[0]);
    p.set_int("OfxParamHostPropSupportsParametricAnimation", &[0]);
    p.set_int("OfxParamHostPropMaxParameters", &[-1]);
    p.set_int("OfxParamHostPropMaxPages", &[0]);
    p.set_int("OfxParamHostPropPageRowColumnCount", &[0, 0]);
    p.set_int("OfxImageEffectInstancePropSequentialRender", &[0]);
    p.set_str("OfxImageEffectPropOpenGLRenderSupported", &["false"]);
    p.set_str("OfxImageEffectPropMetalRenderSupported", &["false"]);
    p.set_str("OfxImageEffectPropCudaRenderSupported", &["false"]);
    p.set_str("OfxImageEffectPropOpenCLRenderSupported", &["false"]);
    p.set_int("OfxImageEffectPropRenderQualityDraft", &[0]);
    p
}

/// The host struct handed to `setHost`; lives for the process.
pub fn host() -> *mut OfxHost {
    struct H(*mut OfxHost);
    // SAFETY: the host struct and its properties are never freed and the
    // property set locks internally.
    unsafe impl Send for H {}
    unsafe impl Sync for H {}
    static HOST: std::sync::OnceLock<H> = std::sync::OnceLock::new();
    HOST.get_or_init(|| {
        let props: &'static Props = Box::leak(Box::new(host_props()));
        H(Box::into_raw(Box::new(OfxHost { host: props.handle(), fetch_suite })))
    })
    .0
}
