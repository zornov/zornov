// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A test OpenFX bundle with two plugins, written against the OFX C ABI the
//! way a C++ plugin would use it:
//!
//! - `com.studio.test.Invert` (filter): inverts colour by `amount`, keeps
//!   only the red channel when `channel` is 1, multiplies by `tint`; rows
//!   are processed through the host's multithread suite, parameters read
//!   with the variadic `paramGetValueAtTime`.
//! - `com.studio.test.Fill` (generator): fills the output with `colour`.

#![allow(non_snake_case, clippy::missing_safety_doc)]

use std::ffi::{CStr, c_char, c_double, c_int, c_uint, c_void};

type H = *mut c_void;
type St = c_int;

#[repr(C)]
pub struct OfxHost {
    host: H,
    fetch_suite: unsafe extern "C" fn(H, *const c_char, c_int) -> *const c_void,
}

#[repr(C)]
pub struct OfxPlugin {
    api: *const c_char,
    api_version: c_int,
    id: *const c_char,
    major: c_uint,
    minor: c_uint,
    set_host: unsafe extern "C" fn(*mut OfxHost),
    main: unsafe extern "C" fn(*const c_char, *const c_void, H, H) -> St,
}

#[repr(C)]
struct Props {
    set_pointer: unsafe extern "C" fn(H, *const c_char, c_int, *mut c_void) -> St,
    set_string: unsafe extern "C" fn(H, *const c_char, c_int, *const c_char) -> St,
    set_double: unsafe extern "C" fn(H, *const c_char, c_int, c_double) -> St,
    set_int: unsafe extern "C" fn(H, *const c_char, c_int, c_int) -> St,
    _set_n: [usize; 4],
    get_pointer: unsafe extern "C" fn(H, *const c_char, c_int, *mut *mut c_void) -> St,
    get_string: unsafe extern "C" fn(H, *const c_char, c_int, *mut *mut c_char) -> St,
    _get_double: usize,
    get_int: unsafe extern "C" fn(H, *const c_char, c_int, *mut c_int) -> St,
    _rest: [usize; 6],
}

#[repr(C)]
struct Effects {
    get_property_set: unsafe extern "C" fn(H, *mut H) -> St,
    get_param_set: unsafe extern "C" fn(H, *mut H) -> St,
    clip_define: unsafe extern "C" fn(H, *const c_char, *mut H) -> St,
    clip_get_handle: unsafe extern "C" fn(H, *const c_char, *mut H, *mut H) -> St,
    _clip_props: usize,
    clip_get_image: unsafe extern "C" fn(H, c_double, *const c_void, *mut H) -> St,
    clip_release_image: unsafe extern "C" fn(H) -> St,
    _rest: [usize; 6],
}

#[repr(C)]
struct Params {
    define: unsafe extern "C" fn(H, *const c_char, *const c_char, *mut H) -> St,
    get_handle: unsafe extern "C" fn(H, *const c_char, *mut H, *mut H) -> St,
    _set_props: usize,
    _props: usize,
    _get_value: usize,
    get_value_at_time: unsafe extern "C" fn(H, c_double, ...) -> St,
    _rest: [usize; 12],
}

#[repr(C)]
struct Threads {
    multi_thread: unsafe extern "C" fn(unsafe extern "C" fn(c_uint, c_uint, *mut c_void), c_uint, *mut c_void) -> St,
    _rest: [usize; 8],
}

#[repr(C)]
struct Messages {
    message: unsafe extern "C" fn(H, *const c_char, *const c_char, *const c_char, ...) -> St,
}

struct Sync<T>(T);
// SAFETY: the plugin structs only hold pointers to static strings.
unsafe impl<T> std::marker::Sync for Sync<T> {}

static mut HOST: *mut OfxHost = std::ptr::null_mut();

unsafe fn suite<T>(name: &CStr, v: c_int) -> &'static T {
    unsafe { &*((*HOST).fetch_suite)((*HOST).host, name.as_ptr(), v).cast::<T>() }
}

unsafe fn props() -> &'static Props {
    unsafe { suite(c"OfxPropertySuite", 1) }
}
unsafe fn fx() -> &'static Effects {
    unsafe { suite(c"OfxImageEffectSuite", 1) }
}
unsafe fn params() -> &'static Params {
    unsafe { suite(c"OfxParameterSuite", 1) }
}

unsafe extern "C" fn set_host(h: *mut OfxHost) {
    unsafe { HOST = h };
}

fn action(a: *const c_char) -> &'static str {
    unsafe { CStr::from_ptr(a) }.to_str().unwrap_or("")
}

unsafe fn describe(effect: H, label: &CStr, context: &CStr) -> St {
    unsafe {
        let mut p: H = std::ptr::null_mut();
        (fx().get_property_set)(effect, &mut p);
        (props().set_string)(p, c"OfxPropLabel".as_ptr(), 0, label.as_ptr());
        (props().set_string)(p, c"OfxImageEffectPluginPropGrouping".as_ptr(), 0, c"Zornov tests".as_ptr());
        (props().set_string)(p, c"OfxImageEffectPropSupportedContexts".as_ptr(), 0, context.as_ptr());
        (props().set_string)(p, c"OfxImageEffectPropSupportedPixelDepths".as_ptr(), 0, c"OfxBitDepthFloat".as_ptr());
        (props().set_string)(p, c"OfxImageEffectPropSupportedPixelDepths".as_ptr(), 1, c"OfxBitDepthByte".as_ptr());
    }
    0
}

unsafe fn define_double(set: H, name: &CStr, label: &CStr, default: f64, max: f64) {
    unsafe {
        let mut p: H = std::ptr::null_mut();
        (params().define)(set, c"OfxParamTypeDouble".as_ptr(), name.as_ptr(), &mut p);
        (props().set_string)(p, c"OfxPropLabel".as_ptr(), 0, label.as_ptr());
        (props().set_double)(p, c"OfxParamPropDefault".as_ptr(), 0, default);
        (props().set_double)(p, c"OfxParamPropMin".as_ptr(), 0, 0.0);
        (props().set_double)(p, c"OfxParamPropMax".as_ptr(), 0, max);
    }
}

unsafe fn image(effect: H, clip: &CStr, time: f64) -> (H, *mut f32, [c_int; 4], c_int) {
    unsafe {
        let mut c: H = std::ptr::null_mut();
        (fx().clip_get_handle)(effect, clip.as_ptr(), &mut c, std::ptr::null_mut());
        let mut img: H = std::ptr::null_mut();
        if (fx().clip_get_image)(c, time, std::ptr::null(), &mut img) != 0 {
            return (std::ptr::null_mut(), std::ptr::null_mut(), [0; 4], 0);
        }
        let mut data: *mut c_void = std::ptr::null_mut();
        (props().get_pointer)(img, c"OfxImagePropData".as_ptr(), 0, &mut data);
        let mut b = [0; 4];
        for (i, v) in b.iter_mut().enumerate() {
            (props().get_int)(img, c"OfxImagePropBounds".as_ptr(), i as c_int, v);
        }
        let mut row = 0;
        (props().get_int)(img, c"OfxImagePropRowBytes".as_ptr(), 0, &mut row);
        // This test plugin asks for float only when the host picks it.
        let mut depth: *mut c_char = std::ptr::null_mut();
        (props().get_string)(img, c"OfxImageEffectPropPixelDepth".as_ptr(), 0, &mut depth);
        assert_eq!(CStr::from_ptr(depth).to_str().unwrap(), "OfxBitDepthFloat");
        (img, data.cast(), b, row)
    }
}

struct Job {
    src: *const f32,
    dst: *mut f32,
    width: usize,
    height: usize,
    row: usize,
    amount: f64,
    red_only: bool,
    tint: [f64; 4],
}

unsafe extern "C" fn rows(i: c_uint, n: c_uint, arg: *mut c_void) {
    let j = unsafe { &*(arg as *const Job) };
    let per = j.height.div_ceil(n as usize);
    for y in (i as usize * per)..((i as usize + 1) * per).min(j.height) {
        for x in 0..j.width {
            let o = y * j.row / 4 + x * 4;
            unsafe {
                let a = *j.src.add(o + 3);
                for c in 0..3 {
                    let v = *j.src.add(o + c) as f64;
                    let inv = v + (a as f64 - 2.0 * v) * j.amount;
                    let keep = if j.red_only && c > 0 { 0.0 } else { 1.0 };
                    *j.dst.add(o + c) = (inv * j.tint[c] * keep) as f32;
                }
                *j.dst.add(o + 3) = a;
            }
        }
    }
}

unsafe extern "C" fn invert_main(a: *const c_char, h: *const c_void, in_args: H, _out: H) -> St {
    let effect = h as H;
    unsafe {
        match action(a) {
            "OfxActionLoad" | "OfxActionCreateInstance" | "OfxActionDestroyInstance" | "OfxActionUnload" => 0,
            "OfxActionDescribe" => describe(effect, c"Test invert", c"OfxImageEffectContextFilter"),
            "OfxImageEffectActionDescribeInContext" => {
                let mut p: H = std::ptr::null_mut();
                (fx().clip_define)(effect, c"Source".as_ptr(), &mut p);
                (fx().clip_define)(effect, c"Output".as_ptr(), &mut p);
                let mut set: H = std::ptr::null_mut();
                (fx().get_param_set)(effect, &mut set);
                define_double(set, c"amount", c"Amount", 1.0, 1.0);
                (params().define)(set, c"OfxParamTypeChoice".as_ptr(), c"channel".as_ptr(), &mut p);
                (props().set_string)(p, c"OfxParamPropChoiceOption".as_ptr(), 0, c"All".as_ptr());
                (props().set_string)(p, c"OfxParamPropChoiceOption".as_ptr(), 1, c"Red only".as_ptr());
                (params().define)(set, c"OfxParamTypeRGBA".as_ptr(), c"tint".as_ptr(), &mut p);
                for i in 0..4 {
                    (props().set_double)(p, c"OfxParamPropDefault".as_ptr(), i, 1.0);
                }
                0
            }
            "OfxImageEffectActionRender" => {
                let mut time = 0.0;
                let _ = in_args;
                let mut set: H = std::ptr::null_mut();
                (fx().get_param_set)(effect, &mut set);
                let get = |name: &CStr| {
                    let mut ph: H = std::ptr::null_mut();
                    (params().get_handle)(set, name.as_ptr(), &mut ph, std::ptr::null_mut());
                    ph
                };
                let mut amount = 0.0f64;
                (params().get_value_at_time)(get(c"amount"), time, &mut amount as *mut f64);
                let mut channel: c_int = 0;
                (params().get_value_at_time)(get(c"channel"), time, &mut channel as *mut c_int);
                let mut t = [0.0f64; 4];
                (params().get_value_at_time)(get(c"tint"), time, &mut t[0] as *mut f64, &mut t[1] as *mut f64, &mut t[2] as *mut f64, &mut t[3] as *mut f64);
                time += 0.0;
                let (si, src, b, row) = image(effect, c"Source", time);
                let (di, dst, _, _) = image(effect, c"Output", time);
                if src.is_null() || dst.is_null() {
                    return 1;
                }
                let job = Job { src, dst, width: (b[2] - b[0]) as usize, height: (b[3] - b[1]) as usize, row: row as usize, amount, red_only: channel == 1, tint: t };
                let threads: &Threads = suite(c"OfxMultiThreadSuite", 1);
                (threads.multi_thread)(rows, 3, &job as *const Job as *mut c_void);
                let msg: &Messages = suite(c"OfxMessageSuite", 1);
                (msg.message)(effect, c"OfxMessageLog".as_ptr(), std::ptr::null(), c"rendered %d rows".as_ptr(), job.height as c_int);
                (fx().clip_release_image)(si);
                (fx().clip_release_image)(di);
                0
            }
            _ => 14,
        }
    }
}

unsafe extern "C" fn fill_main(a: *const c_char, h: *const c_void, _in: H, _out: H) -> St {
    let effect = h as H;
    unsafe {
        match action(a) {
            "OfxActionLoad" | "OfxActionCreateInstance" | "OfxActionDestroyInstance" | "OfxActionUnload" => 0,
            "OfxActionDescribe" => describe(effect, c"Test fill", c"OfxImageEffectContextGenerator"),
            "OfxImageEffectActionDescribeInContext" => {
                let mut p: H = std::ptr::null_mut();
                (fx().clip_define)(effect, c"Output".as_ptr(), &mut p);
                let mut set: H = std::ptr::null_mut();
                (fx().get_param_set)(effect, &mut set);
                (params().define)(set, c"OfxParamTypeRGBA".as_ptr(), c"colour".as_ptr(), &mut p);
                for (i, v) in [0.0, 0.5, 1.0, 1.0].iter().enumerate() {
                    (props().set_double)(p, c"OfxParamPropDefault".as_ptr(), i as c_int, *v);
                }
                0
            }
            "OfxImageEffectActionRender" => {
                let mut set: H = std::ptr::null_mut();
                (fx().get_param_set)(effect, &mut set);
                let mut ph: H = std::ptr::null_mut();
                (params().get_handle)(set, c"colour".as_ptr(), &mut ph, std::ptr::null_mut());
                let mut c = [0.0f64; 4];
                (params().get_value_at_time)(ph, 0.0, &mut c[0] as *mut f64, &mut c[1] as *mut f64, &mut c[2] as *mut f64, &mut c[3] as *mut f64);
                let (di, dst, b, row) = image(effect, c"Output", 0.0);
                for y in 0..(b[3] - b[1]) as usize {
                    for x in 0..(b[2] - b[0]) as usize {
                        for k in 0..4 {
                            *dst.add(y * row as usize / 4 + x * 4 + k) = c[k] as f32;
                        }
                    }
                }
                (fx().clip_release_image)(di);
                0
            }
            _ => 14,
        }
    }
}

static INVERT: Sync<OfxPlugin> = Sync(OfxPlugin { api: c"OfxImageEffectPluginAPI".as_ptr(), api_version: 1, id: c"com.studio.test.Invert".as_ptr(), major: 1, minor: 2, set_host, main: invert_main });
static FILL: Sync<OfxPlugin> = Sync(OfxPlugin { api: c"OfxImageEffectPluginAPI".as_ptr(), api_version: 1, id: c"com.studio.test.Fill".as_ptr(), major: 1, minor: 0, set_host, main: fill_main });


#[unsafe(no_mangle)]
pub extern "C" fn OfxGetNumberOfPlugins() -> c_int {
    2
}

#[unsafe(no_mangle)]
pub extern "C" fn OfxGetPlugin(i: c_int) -> *const OfxPlugin {
    match i {
        0 => &INVERT.0,
        1 => &FILL.0,
        _ => std::ptr::null(),
    }
}
