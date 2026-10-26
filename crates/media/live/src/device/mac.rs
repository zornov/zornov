// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! macOS capture: cameras through AVFoundation, screens and windows
//! through ScreenCaptureKit (loaded when first needed, so Studio still
//! starts on a Mac without it), and the pointer through CoreGraphics.
//! Everything is reached through the Objective-C runtime; nothing here
//! runs until a device is listed, asked about or opened, and listing and
//! asking about access never prompt.

use super::{Access, Device, Slot, SlotVideo};
use crate::cursor::{CursorSample, CursorSource};
use crate::frame::Frame;
use crate::source::{Kind, VideoSource};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, NSObject};
use objc2::{AllocAnyThread, DefinedClass, class, define_class, msg_send};
use std::ffi::{CStr, c_char, c_void};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {}
#[link(name = "Foundation", kind = "framework")]
unsafe extern "C" {}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

// SAFETY: matches CoreMedia's `{?=qiIq}`.
unsafe impl objc2::encode::Encode for CMTime {
    const ENCODING: objc2::encode::Encoding = objc2::encode::Encoding::Struct("?", &[i64::ENCODING, i32::ENCODING, u32::ENCODING, i64::ENCODING]);
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGGetActiveDisplayList(max: u32, out: *mut u32, count: *mut u32) -> i32;
    fn CGMainDisplayID() -> u32;
    fn CGDisplayBounds(id: u32) -> CGRect;
    fn CGDisplayPixelsWide(id: u32) -> usize;
    fn CGDisplayPixelsHigh(id: u32) -> usize;
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
    fn CGWindowListCopyWindowInfo(option: u32, window: u32) -> *const c_void;
    fn CGEventCreate(source: *const c_void) -> *const c_void;
    fn CGEventGetLocation(e: *const c_void) -> CGPoint;
    fn CGEventSourceButtonState(state: i32, button: u32) -> bool;
    static kCGWindowNumber: *const c_void;
    static kCGWindowName: *const c_void;
    static kCGWindowOwnerName: *const c_void;
    static kCGWindowLayer: *const c_void;
    static kCGWindowBounds: *const c_void;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFArrayGetCount(a: *const c_void) -> isize;
    fn CFArrayGetValueAtIndex(a: *const c_void, i: isize) -> *const c_void;
    fn CFDictionaryGetValue(d: *const c_void, k: *const c_void) -> *const c_void;
    fn CFStringGetCString(s: *const c_void, buf: *mut c_char, size: isize, enc: u32) -> bool;
    fn CFNumberGetValue(n: *const c_void, kind: i32, out: *mut c_void) -> bool;
    fn CFRelease(p: *const c_void);
}

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    fn CMSampleBufferGetImageBuffer(sb: *mut c_void) -> *mut c_void;
}

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    fn CVPixelBufferLockBaseAddress(pb: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(pb: *mut c_void, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddress(pb: *mut c_void) -> *mut c_void;
    fn CVPixelBufferGetBytesPerRow(pb: *mut c_void) -> usize;
    fn CVPixelBufferGetWidth(pb: *mut c_void) -> usize;
    fn CVPixelBufferGetHeight(pb: *mut c_void) -> usize;
    fn CVPixelBufferGetPixelFormatType(pb: *mut c_void) -> u32;
    static kCVPixelBufferPixelFormatTypeKey: *const c_void;
}

unsafe extern "C" {
    fn dispatch_queue_create(label: *const c_char, attr: *const c_void) -> *mut AnyObject;
    fn dlopen(path: *const c_char, mode: i32) -> *mut c_void;
}

const BGRA: u32 = 0x4247_5241;

// ───────────────────────── small helpers ─────────────────────────

unsafe fn nsstr(s: &str) -> Retained<AnyObject> {
    let c = std::ffi::CString::new(s.replace('\0', "")).unwrap_or_default();
    let o: Option<Retained<AnyObject>> = unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] };
    o.expect("NSString")
}

unsafe fn to_string(o: *mut AnyObject) -> String {
    if o.is_null() {
        return String::new();
    }
    let p: *const c_char = unsafe { msg_send![o, UTF8String] };
    if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
}

unsafe fn cf_string(s: *const c_void) -> String {
    if s.is_null() {
        return String::new();
    }
    let mut buf = vec![0 as c_char; 512];
    if unsafe { CFStringGetCString(s, buf.as_mut_ptr(), 512, 0x0800_0100) } { unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned() } else { String::new() }
}

unsafe fn cf_i32(n: *const c_void) -> Option<i32> {
    if n.is_null() {
        return None;
    }
    let mut v = 0i32;
    unsafe { CFNumberGetValue(n, 3, &mut v as *mut i32 as *mut c_void) }.then_some(v)
}

unsafe fn cf_f64(n: *const c_void) -> Option<f64> {
    if n.is_null() {
        return None;
    }
    let mut v = 0f64;
    unsafe { CFNumberGetValue(n, 6, &mut v as *mut f64 as *mut c_void) }.then_some(v)
}

unsafe fn nsarray(items: &[Retained<AnyObject>]) -> Retained<AnyObject> {
    let ptrs: Vec<*mut AnyObject> = items.iter().map(|o| Retained::as_ptr(o) as *mut AnyObject).collect();
    let a: Option<Retained<AnyObject>> = unsafe { msg_send![class!(NSArray), arrayWithObjects: ptrs.as_ptr(), count: ptrs.len()] };
    a.expect("NSArray")
}

fn load_framework(path: &CStr) -> bool {
    // SAFETY: dlopen with a valid path; the handle is intentionally kept open.
    !unsafe { dlopen(path.as_ptr(), 2) }.is_null()
}

fn load_sck() -> Result<(), String> {
    if AnyClass::get(c"SCStream").is_some() || load_framework(c"/System/Library/Frameworks/ScreenCaptureKit.framework/ScreenCaptureKit") {
        Ok(())
    } else {
        Err("ScreenCaptureKit is not available on this Mac (macOS 12.3 or later is needed to record screens and windows)".into())
    }
}

// ───────────────────────── access and listing ─────────────────────────

pub fn access(kind: Kind) -> Access {
    match kind {
        Kind::Camera | Kind::Mic => {
            // SAFETY: a class method taking an NSString.
            let s: isize = unsafe { msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: &*nsstr(if kind == Kind::Camera { "vide" } else { "soun" })] };
            match s {
                0 => Access::NotDetermined,
                1 => Access::Restricted,
                2 => Access::Denied,
                3 => Access::Authorized,
                _ => Access::NotDetermined,
            }
        }
        Kind::Screen | Kind::Window => {
            if unsafe { CGPreflightScreenCaptureAccess() } { Access::Authorized } else { Access::NotDetermined }
        }
        Kind::Test => Access::Authorized,
    }
}

pub fn request(kind: Kind) -> Result<Access, String> {
    match kind {
        Kind::Camera | Kind::Mic => {
            if access(kind) != Access::NotDetermined {
                return Ok(access(kind));
            }
            let (tx, rx) = mpsc::channel::<bool>();
            let block = RcBlock::new(move |granted: Bool| {
                let _ = tx.send(granted.as_bool());
            });
            // SAFETY: the block is copied by the callee; the channel outlives the wait.
            unsafe {
                let _: () = msg_send![class!(AVCaptureDevice), requestAccessForMediaType: &*nsstr(if kind == Kind::Camera { "vide" } else { "soun" }), completionHandler: &*block];
            }
            rx.recv_timeout(Duration::from_secs(300)).map_err(|_| "no answer to the permission question".to_string())?;
            Ok(access(kind))
        }
        Kind::Screen | Kind::Window => {
            // The system shows its question; the answer may need Studio to
            // be restarted before it counts.
            unsafe { CGRequestScreenCaptureAccess() };
            Ok(access(kind))
        }
        Kind::Test => Ok(Access::Authorized),
    }
}

pub fn cameras() -> Vec<Device> {
    let mut out = Vec::new();
    // SAFETY: discovery sessions are plain queries; they do not open a camera.
    unsafe {
        let names = ["AVCaptureDeviceTypeBuiltInWideAngleCamera", "AVCaptureDeviceTypeExternal", "AVCaptureDeviceTypeContinuityCamera", "AVCaptureDeviceTypeDeskViewCamera"];
        let types: Vec<Retained<AnyObject>> = names.iter().map(|n| nsstr(n)).collect();
        let arr = nsarray(&types);
        let ds: Option<Retained<AnyObject>> = msg_send![class!(AVCaptureDeviceDiscoverySession), discoverySessionWithDeviceTypes: &*arr, mediaType: &*nsstr("vide"), position: 0isize];
        let Some(ds) = ds else { return out };
        let devs: Option<Retained<AnyObject>> = msg_send![&*ds, devices];
        let Some(devs) = devs else { return out };
        let n: usize = msg_send![&*devs, count];
        let default: Option<Retained<AnyObject>> = msg_send![class!(AVCaptureDevice), defaultDeviceWithMediaType: &*nsstr("vide")];
        let default_uid = default.map(|d| to_string(msg_send![&*d, uniqueID])).unwrap_or_default();
        for i in 0..n {
            let d: *mut AnyObject = msg_send![&*devs, objectAtIndex: i];
            let uid = to_string(msg_send![d, uniqueID]);
            let name = to_string(msg_send![d, localizedName]);
            let model = to_string(msg_send![d, modelID]);
            out.push(Device { id: format!("camera:{uid}"), name, detail: model, default: uid == default_uid });
        }
    }
    out
}

pub fn screens() -> Vec<Device> {
    let mut ids = [0u32; 16];
    let mut n = 0u32;
    // SAFETY: fills the array up to 16 entries.
    unsafe { CGGetActiveDisplayList(16, ids.as_mut_ptr(), &mut n) };
    let main = unsafe { CGMainDisplayID() };
    ids[..n as usize]
        .iter()
        .enumerate()
        .map(|(i, &id)| {
            let (w, h) = unsafe { (CGDisplayPixelsWide(id), CGDisplayPixelsHigh(id)) };
            Device { id: format!("screen:{id}"), name: if id == main { "Main display".into() } else { format!("Display {}", i + 1) }, detail: format!("{w}x{h}"), default: id == main }
        })
        .collect()
}

pub struct WinInfo {
    pub id: u32,
    pub owner: String,
    pub title: String,
    pub bounds: (f64, f64, f64, f64),
}

pub fn window_infos(only: Option<u32>) -> Vec<WinInfo> {
    let mut out = Vec::new();
    // Option 1 = on screen only, 8 = excluding desktop elements; 8 alone for a single id lists "including window".
    let (opt, id) = match only {
        Some(id) => (8u32, id),
        None => (1 | 16, 0),
    };
    // SAFETY: CoreFoundation containers read while the array is alive.
    unsafe {
        let list = CGWindowListCopyWindowInfo(opt, id);
        if list.is_null() {
            return out;
        }
        for i in 0..CFArrayGetCount(list) {
            let d = CFArrayGetValueAtIndex(list, i);
            let layer = cf_i32(CFDictionaryGetValue(d, kCGWindowLayer)).unwrap_or(0);
            if layer != 0 && only.is_none() {
                continue;
            }
            let Some(num) = cf_i32(CFDictionaryGetValue(d, kCGWindowNumber)) else { continue };
            let b = CFDictionaryGetValue(d, kCGWindowBounds);
            let key = |k: &str| -> *const c_void {
                // Bounds keys are plain strings.
                let s = nsstr(k);
                let p = Retained::as_ptr(&s) as *const c_void;
                std::mem::forget(s);
                p
            };
            let g = |k: &str| cf_f64(CFDictionaryGetValue(b, key(k))).unwrap_or(0.0);
            out.push(WinInfo { id: num as u32, owner: cf_string(CFDictionaryGetValue(d, kCGWindowOwnerName)), title: cf_string(CFDictionaryGetValue(d, kCGWindowName)), bounds: if b.is_null() { (0.0, 0.0, 0.0, 0.0) } else { (g("X"), g("Y"), g("Width"), g("Height")) } });
        }
        CFRelease(list);
    }
    out
}

pub fn windows() -> Vec<Device> {
    window_infos(None)
        .into_iter()
        .filter(|w| !w.owner.is_empty() && w.bounds.2 > 80.0 && w.bounds.3 > 60.0)
        .map(|w| Device { id: format!("window:{}", w.id), name: if w.title.is_empty() { w.owner.clone() } else { format!("{} - {}", w.owner, w.title) }, detail: format!("{}x{}", w.bounds.2 as u32, w.bounds.3 as u32), default: false })
        .collect()
}

// ───────────────────────── the pointer ─────────────────────────

struct MacCursor {
    /// Display bounds in points, or the window whose bounds follow.
    display: Option<u32>,
    window: Option<u32>,
}

impl CursorSource for MacCursor {
    fn at(&mut self, t_us: i64) -> Option<CursorSample> {
        // SAFETY: a fresh event holds the pointer position; released after reading.
        let (p, down) = unsafe {
            let e = CGEventCreate(std::ptr::null());
            if e.is_null() {
                return None;
            }
            let p = CGEventGetLocation(e);
            CFRelease(e);
            (p, CGEventSourceButtonState(0, 0))
        };
        let (bx, by, bw, bh) = if let Some(d) = self.display {
            let r = unsafe { CGDisplayBounds(d) };
            (r.x, r.y, r.w, r.h)
        } else if let Some(w) = self.window {
            window_infos(Some(w)).into_iter().find(|i| i.id == w)?.bounds
        } else {
            return None;
        };
        if bw <= 0.0 || bh <= 0.0 {
            return None;
        }
        let (x, y) = ((p.x - bx) / bw, (p.y - by) / bh);
        // Off the recorded area: no highlight.
        if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
            return None;
        }
        Some(CursorSample { t_us, x: x as f32, y: y as f32, down })
    }
}

pub fn cursor(source_id: &str) -> Option<Box<dyn CursorSource>> {
    let (k, rest) = source_id.split_once(':')?;
    let id = rest.parse::<u32>().ok();
    match k {
        "screen" => Some(Box::new(MacCursor { display: Some(id.unwrap_or_else(|| unsafe { CGMainDisplayID() })), window: None })),
        "window" => Some(Box::new(MacCursor { display: None, window: Some(id?) })),
        _ => None,
    }
}

// ───────────────────────── frames in ─────────────────────────

/// A picture from a sample buffer (BGRA only; the format asked for).
unsafe fn frame_of(sample: *mut c_void) -> Option<Frame> {
    if sample.is_null() {
        return None;
    }
    unsafe {
        let pb = CMSampleBufferGetImageBuffer(sample);
        if pb.is_null() || CVPixelBufferGetPixelFormatType(pb) != BGRA {
            return None;
        }
        if CVPixelBufferLockBaseAddress(pb, 1) != 0 {
            return None;
        }
        let (w, h, stride) = (CVPixelBufferGetWidth(pb), CVPixelBufferGetHeight(pb), CVPixelBufferGetBytesPerRow(pb));
        let base = CVPixelBufferGetBaseAddress(pb) as *const u8;
        let mut rgba = vec![0u8; w * h * 4];
        if !base.is_null() {
            for y in 0..h {
                let row = std::slice::from_raw_parts(base.add(y * stride), w * 4);
                let out = &mut rgba[y * w * 4..(y + 1) * w * 4];
                for (o, p) in out.chunks_exact_mut(4).zip(row.chunks_exact(4)) {
                    o.copy_from_slice(&[p[2], p[1], p[0], 255]);
                }
            }
        }
        CVPixelBufferUnlockBaseAddress(pb, 1);
        if base.is_null() {
            return None;
        }
        Some(Frame { w: w as u32, h: h as u32, rgba, pts_us: 0 })
    }
}

struct SinkIvars {
    slot: Arc<Slot>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the methods only
    // read the sample buffer they are handed.
    #[unsafe(super(NSObject))]
    #[name = "StudioFrameSink"]
    #[ivars = SinkIvars]
    struct FrameSink;

    impl FrameSink {
        // AVCaptureVideoDataOutputSampleBufferDelegate
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        fn capture_output(&self, _output: &AnyObject, sample: *mut c_void, _connection: &AnyObject) {
            if let Some(f) = unsafe { frame_of(sample) } {
                if let Ok(mut g) = self.ivars().slot.frame.lock() {
                    *g = Some(f);
                }
            }
        }

        // SCStreamOutput
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn stream_output(&self, _stream: &AnyObject, sample: *mut c_void, kind: isize) {
            if kind != 0 {
                return;
            }
            if let Some(f) = unsafe { frame_of(sample) } {
                if let Ok(mut g) = self.ivars().slot.frame.lock() {
                    *g = Some(f);
                }
            }
        }
    }
);

impl FrameSink {
    fn new(slot: Arc<Slot>) -> Retained<FrameSink> {
        let this = FrameSink::alloc().set_ivars(SinkIvars { slot });
        // SAFETY: plain NSObject init.
        unsafe { msg_send![super(this), init] }
    }
}

struct SendBox<T>(T);
// SAFETY: the Objective-C objects held are only messaged from drop, and the
// frameworks allow stopping a session from any thread.
unsafe impl<T> Send for SendBox<T> {}

struct CameraGuard {
    session: SendBox<Retained<AnyObject>>,
    _sink: SendBox<Retained<FrameSink>>,
}

impl Drop for CameraGuard {
    fn drop(&mut self) {
        // SAFETY: stops the capture the person started.
        unsafe {
            let _: () = msg_send![&*self.session.0, stopRunning];
        }
    }
}

pub fn open_video(kind: Kind, id: &str, w: u32, h: u32, fps: f64) -> Result<Box<dyn VideoSource>, String> {
    match kind {
        Kind::Camera => camera(id, w),
        Kind::Screen | Kind::Window => screen(kind, id, w, h, fps),
        _ => Err("not a picture device".into()),
    }
}

fn first_frame(slot: &Arc<Slot>, what: &str) -> Result<(u32, u32), String> {
    let t0 = std::time::Instant::now();
    while t0.elapsed() < Duration::from_secs(6) {
        if let Some(f) = slot.frame.lock().ok().and_then(|g| g.as_ref().map(|f| (f.w, f.h))) {
            return Ok(f);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(format!("{what} did not deliver a picture (is something else using it?)"))
}

fn camera(id: &str, w: u32) -> Result<Box<dyn VideoSource>, String> {
    // SAFETY: standard AVCaptureSession wiring; every object is retained
    // until the guard drops, and the session is stopped on drop.
    unsafe {
        let video = nsstr("vide");
        let dev: Option<Retained<AnyObject>> = if id == "default" || id.is_empty() { msg_send![class!(AVCaptureDevice), defaultDeviceWithMediaType: &*video] } else { msg_send![class!(AVCaptureDevice), deviceWithUniqueID: &*nsstr(id)] };
        let dev = dev.ok_or_else(|| format!("no camera \"{id}\" (capture devices lists them)"))?;
        let mut err: *mut AnyObject = std::ptr::null_mut();
        let input: Option<Retained<AnyObject>> = msg_send![class!(AVCaptureDeviceInput), deviceInputWithDevice: &*dev, error: &mut err];
        let input = input.ok_or_else(|| {
            let d: *mut AnyObject = if err.is_null() { std::ptr::null_mut() } else { msg_send![err, localizedDescription] };
            format!("the camera could not be opened: {}", to_string(d))
        })?;
        let session: Retained<AnyObject> = msg_send![class!(AVCaptureSession), new];
        let preset = if w >= 1900 { "AVCaptureSessionPreset1920x1080" } else if w >= 1200 { "AVCaptureSessionPreset1280x720" } else { "AVCaptureSessionPreset640x480" };
        let p = nsstr(preset);
        let can: Bool = msg_send![&*session, canSetSessionPreset: &*p];
        if can.as_bool() {
            let _: () = msg_send![&*session, setSessionPreset: &*p];
        }
        let can: Bool = msg_send![&*session, canAddInput: &*input];
        if !can.as_bool() {
            return Err("the camera cannot be added to a capture session".into());
        }
        let _: () = msg_send![&*session, addInput: &*input];
        let output: Retained<AnyObject> = msg_send![class!(AVCaptureVideoDataOutput), new];
        let key = kCVPixelBufferPixelFormatTypeKey as *mut AnyObject;
        let num: Retained<AnyObject> = msg_send![class!(NSNumber), numberWithUnsignedInt: BGRA];
        let settings: Retained<AnyObject> = msg_send![class!(NSDictionary), dictionaryWithObject: &*num, forKey: key];
        let _: () = msg_send![&*output, setVideoSettings: &*settings];
        let _: () = msg_send![&*output, setAlwaysDiscardsLateVideoFrames: Bool::YES];
        let slot = Arc::new(Slot::default());
        let sink = FrameSink::new(slot.clone());
        let queue = dispatch_queue_create(c"studio.capture.camera".as_ptr(), std::ptr::null());
        let _: () = msg_send![&*output, setSampleBufferDelegate: &*sink, queue: queue];
        let can: Bool = msg_send![&*session, canAddOutput: &*output];
        if !can.as_bool() {
            return Err("the camera's picture cannot be read".into());
        }
        let _: () = msg_send![&*session, addOutput: &*output];
        let _: () = msg_send![&*session, startRunning];
        let guard = CameraGuard { session: SendBox(session), _sink: SendBox(sink) };
        let name = to_string(msg_send![&*dev, localizedName]);
        let size = first_frame(&slot, &format!("the camera \"{name}\""))?;
        Ok(Box::new(SlotVideo { label: format!("camera {name}"), slot, size, _guard: Box::new(guard), start: std::time::Instant::now() }))
    }
}

struct ScreenGuard {
    stream: SendBox<Retained<AnyObject>>,
    _sink: SendBox<Retained<FrameSink>>,
}

impl Drop for ScreenGuard {
    fn drop(&mut self) {
        let done = RcBlock::new(|_err: *mut AnyObject| {});
        // SAFETY: stops the capture the person started.
        unsafe {
            let _: () = msg_send![&*self.stream.0, stopCaptureWithCompletionHandler: &*done];
        }
    }
}

fn screen(kind: Kind, id: &str, w: u32, h: u32, fps: f64) -> Result<Box<dyn VideoSource>, String> {
    load_sck()?;
    let want: Option<u32> = id.parse().ok();
    // SAFETY: ScreenCaptureKit calls on retained objects; the completion
    // blocks hand their results through channels.
    unsafe {
        let (tx, rx) = mpsc::channel::<SendBox<Option<Retained<AnyObject>>>>();
        let block = RcBlock::new(move |content: *mut AnyObject, _err: *mut AnyObject| {
            let c = if content.is_null() { None } else { Retained::retain(content) };
            let _ = tx.send(SendBox(c));
        });
        let _: () = msg_send![class!(SCShareableContent), getShareableContentWithCompletionHandler: &*block];
        let content = rx.recv_timeout(Duration::from_secs(10)).map_err(|_| "the system did not list what can be recorded".to_string())?.0.ok_or("the system would not list what can be recorded: is screen recording allowed for Zornov?")?;
        let (filter, width, height): (Retained<AnyObject>, usize, usize);
        if kind == Kind::Screen {
            let displays: Retained<AnyObject> = msg_send![&*content, displays];
            let n: usize = msg_send![&*displays, count];
            let mut found: Option<Retained<AnyObject>> = None;
            for i in 0..n {
                let d: Retained<AnyObject> = msg_send![&*displays, objectAtIndex: i];
                let did: u32 = msg_send![&*d, displayID];
                if want.map_or(did == CGMainDisplayID(), |w| w == did) {
                    found = Some(d);
                    break;
                }
            }
            let d = found.ok_or_else(|| format!("no display \"{id}\""))?;
            width = msg_send![&*d, width];
            height = msg_send![&*d, height];
            let empty = nsarray(&[]);
            let alloc: objc2::rc::Allocated<AnyObject> = msg_send![class!(SCContentFilter), alloc];
            filter = msg_send![alloc, initWithDisplay: &*d, excludingWindows: &*empty];
        } else {
            let windows: Retained<AnyObject> = msg_send![&*content, windows];
            let n: usize = msg_send![&*windows, count];
            let mut found: Option<Retained<AnyObject>> = None;
            for i in 0..n {
                let win: Retained<AnyObject> = msg_send![&*windows, objectAtIndex: i];
                let wid: u32 = msg_send![&*win, windowID];
                if Some(wid) == want {
                    found = Some(win);
                    break;
                }
            }
            let win = found.ok_or_else(|| format!("no window \"{id}\" (it may have closed)"))?;
            let b = window_infos(Some(want.unwrap_or(0))).into_iter().find(|i| Some(i.id) == want).map(|i| i.bounds).unwrap_or((0.0, 0.0, 1280.0, 720.0));
            width = b.2 as usize;
            height = b.3 as usize;
            let alloc: objc2::rc::Allocated<AnyObject> = msg_send![class!(SCContentFilter), alloc];
            filter = msg_send![alloc, initWithDesktopIndependentWindow: &*win];
        }
        // Pixels, not points, at the size asked for (the display's own when 0).
        let (ow, oh) = if w > 0 && h > 0 { (w as usize, h as usize) } else { (width, height) };
        let cfg: Retained<AnyObject> = msg_send![class!(SCStreamConfiguration), new];
        let _: () = msg_send![&*cfg, setWidth: ow & !1];
        let _: () = msg_send![&*cfg, setHeight: oh & !1];
        let _: () = msg_send![&*cfg, setPixelFormat: BGRA];
        let _: () = msg_send![&*cfg, setShowsCursor: Bool::YES];
        let _: () = msg_send![&*cfg, setMinimumFrameInterval: CMTime { value: 1, timescale: fps.round().max(1.0) as i32, flags: 1, epoch: 0 }];
        let _: () = msg_send![&*cfg, setQueueDepth: 5isize];
        let alloc: objc2::rc::Allocated<AnyObject> = msg_send![class!(SCStream), alloc];
        let stream: Retained<AnyObject> = msg_send![alloc, initWithFilter: &*filter, configuration: &*cfg, delegate: std::ptr::null::<AnyObject>()];
        let slot = Arc::new(Slot::default());
        let sink = FrameSink::new(slot.clone());
        let queue = dispatch_queue_create(c"studio.capture.screen".as_ptr(), std::ptr::null());
        let mut err: *mut AnyObject = std::ptr::null_mut();
        let ok: Bool = msg_send![&*stream, addStreamOutput: &*sink, type: 0isize, sampleHandlerQueue: queue, error: &mut err];
        if !ok.as_bool() {
            return Err("the screen's picture cannot be read".into());
        }
        let (tx, rx) = mpsc::channel::<String>();
        let started = RcBlock::new(move |e: *mut AnyObject| {
            let msg = if e.is_null() { String::new() } else { to_string(msg_send![e, localizedDescription]) };
            let _ = tx.send(msg);
        });
        let _: () = msg_send![&*stream, startCaptureWithCompletionHandler: &*started];
        let msg = rx.recv_timeout(Duration::from_secs(10)).map_err(|_| "screen capture did not start".to_string())?;
        if !msg.is_empty() {
            return Err(format!("screen capture could not start: {msg}"));
        }
        let guard = ScreenGuard { stream: SendBox(stream), _sink: SendBox(sink) };
        let size = first_frame(&slot, "the screen")?;
        Ok(Box::new(SlotVideo { label: format!("{} {id}", if kind == Kind::Screen { "screen" } else { "window" }), slot, size, _guard: Box::new(guard), start: std::time::Instant::now() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_and_asking_about_access_never_prompt() {
        // These are plain queries; on a machine without a camera or with
        // access not yet given they simply return what there is.
        let d = crate::device::list();
        eprintln!("{d:#?}");
        assert!(!d.screens.is_empty(), "a Mac has a display");
        assert!(d.screens[0].id.starts_with("screen:"));
        for k in [Kind::Camera, Kind::Mic, Kind::Screen] {
            let a = crate::device::access(k);
            eprintln!("{k:?}: {a:?}");
            eprintln!("{}", crate::device::explain(k, a));
        }
        // The class that makes frames registers and can be made.
        let slot = Arc::new(Slot::default());
        let sink = FrameSink::new(slot);
        let _ = sink;
    }

    #[test]
    fn opening_without_access_is_refused_before_touching_the_device() {
        // Consent is required by the source layer; access by the device layer.
        let e = crate::source::open_video("camera:default", 640, 360, 30.0, crate::source::Consent::default()).err().unwrap();
        assert!(e.contains("real device"));
    }

    #[test]
    fn the_pointer_can_be_read() {
        // Reading the pointer position needs no permission.
        let mut c = cursor("screen:0").unwrap();
        let _ = c.at(0);
    }
}
