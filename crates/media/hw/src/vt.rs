// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! VideoToolbox, bound by hand: an H.264 compression session that takes
//! BGRA pixel buffers (the hardware converts to YUV) and hands back AVCC
//! samples in display order (no frame reordering, so the container needs
//! no composition offsets).

#![allow(non_upper_case_globals, non_snake_case)]

use super::Encoded;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};

type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFDictionaryRef = *const c_void;
type CFArrayRef = *const c_void;
type CFAllocatorRef = *const c_void;
type OSStatus = i32;
type VTSession = *mut c_void;
type CVPixelBufferRef = *mut c_void;
type CMSampleBufferRef = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

const VALID: u32 = 1;
const INVALID: CMTime = CMTime { value: 0, timescale: 0, flags: 0, epoch: 0 };

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFBooleanTrue: CFTypeRef;
    static kCFBooleanFalse: CFTypeRef;
    static kCFTypeDictionaryKeyCallBacks: u8;
    static kCFTypeDictionaryValueCallBacks: u8;
    fn CFRelease(cf: CFTypeRef);
    fn CFNumberCreate(alloc: CFAllocatorRef, kind: isize, value: *const c_void) -> CFTypeRef;
    fn CFDictionaryCreate(alloc: CFAllocatorRef, keys: *const CFTypeRef, values: *const CFTypeRef, n: isize, kcb: *const u8, vcb: *const u8) -> CFDictionaryRef;
    fn CFDictionaryGetValue(dict: CFDictionaryRef, key: CFTypeRef) -> CFTypeRef;
    fn CFArrayGetCount(a: CFArrayRef) -> isize;
    fn CFArrayGetValueAtIndex(a: CFArrayRef, i: isize) -> CFTypeRef;
    fn CFStringCreateWithCString(alloc: CFAllocatorRef, s: *const std::ffi::c_char, encoding: u32) -> CFStringRef;
    fn CFDataGetLength(d: CFTypeRef) -> isize;
    fn CFDataGetBytePtr(d: CFTypeRef) -> *const u8;
}

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    static kCMSampleAttachmentKey_NotSync: CFStringRef;
    fn CMSampleBufferGetDataBuffer(s: CMSampleBufferRef) -> *mut c_void;
    fn CMBlockBufferGetDataLength(b: *mut c_void) -> usize;
    fn CMBlockBufferCopyDataBytes(b: *mut c_void, offset: usize, len: usize, dest: *mut c_void) -> OSStatus;
    fn CMSampleBufferGetFormatDescription(s: CMSampleBufferRef) -> *mut c_void;
    fn CMSampleBufferGetSampleAttachmentsArray(s: CMSampleBufferRef, create: u8) -> CFArrayRef;
    fn CMFormatDescriptionGetExtension(desc: *mut c_void, key: CFStringRef) -> CFTypeRef;
    static kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms: CFStringRef;
    fn CMVideoFormatDescriptionGetH264ParameterSetAtIndex(desc: *mut c_void, index: usize, ptr: *mut *const u8, size: *mut usize, count: *mut usize, nal_header_len: *mut i32) -> OSStatus;
}

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    static kCVPixelBufferPixelFormatTypeKey: CFStringRef;
    static kCVPixelBufferWidthKey: CFStringRef;
    static kCVPixelBufferHeightKey: CFStringRef;
    static kCVPixelBufferIOSurfacePropertiesKey: CFStringRef;
    static kCVImageBufferColorPrimaries_ITU_R_709_2: CFStringRef;
    static kCVImageBufferTransferFunction_ITU_R_709_2: CFStringRef;
    static kCVImageBufferYCbCrMatrix_ITU_R_709_2: CFStringRef;
    fn CVPixelBufferPoolCreatePixelBuffer(alloc: CFAllocatorRef, pool: *mut c_void, out: *mut CVPixelBufferRef) -> i32;
    fn CVPixelBufferLockBaseAddress(pb: CVPixelBufferRef, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(pb: CVPixelBufferRef, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddress(pb: CVPixelBufferRef) -> *mut u8;
    fn CVPixelBufferGetBytesPerRow(pb: CVPixelBufferRef) -> usize;
    fn CVPixelBufferGetBaseAddressOfPlane(pb: CVPixelBufferRef, plane: usize) -> *mut u8;
    fn CVPixelBufferGetBytesPerRowOfPlane(pb: CVPixelBufferRef, plane: usize) -> usize;
}

#[link(name = "VideoToolbox", kind = "framework")]
unsafe extern "C" {
    static kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder: CFStringRef;
    static kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder: CFStringRef;
    static kVTCompressionPropertyKey_RealTime: CFStringRef;
    static kVTCompressionPropertyKey_AverageBitRate: CFStringRef;
    static kVTCompressionPropertyKey_ProfileLevel: CFStringRef;
    static kVTProfileLevel_H264_High_AutoLevel: CFStringRef;
    static kVTProfileLevel_HEVC_Main10_AutoLevel: CFStringRef;
    static kVTCompressionPropertyKey_AllowFrameReordering: CFStringRef;
    static kVTCompressionPropertyKey_MaxKeyFrameInterval: CFStringRef;
    static kVTCompressionPropertyKey_ExpectedFrameRate: CFStringRef;
    static kVTCompressionPropertyKey_ColorPrimaries: CFStringRef;
    static kVTCompressionPropertyKey_TransferFunction: CFStringRef;
    static kVTCompressionPropertyKey_YCbCrMatrix: CFStringRef;
    static kVTEncodeFrameOptionKey_ForceKeyFrame: CFStringRef;
    fn VTCompressionSessionCreate(
        alloc: CFAllocatorRef,
        width: i32,
        height: i32,
        codec: u32,
        encoder_spec: CFDictionaryRef,
        source_attrs: CFDictionaryRef,
        compressed_alloc: CFAllocatorRef,
        callback: extern "C" fn(*mut c_void, *mut c_void, OSStatus, u32, CMSampleBufferRef),
        refcon: *mut c_void,
        out: *mut VTSession,
    ) -> OSStatus;
    fn VTSessionSetProperty(session: VTSession, key: CFStringRef, value: CFTypeRef) -> OSStatus;
    fn VTCompressionSessionPrepareToEncodeFrames(session: VTSession) -> OSStatus;
    fn VTCompressionSessionGetPixelBufferPool(session: VTSession) -> *mut c_void;
    fn VTCompressionSessionEncodeFrame(session: VTSession, image: CVPixelBufferRef, pts: CMTime, duration: CMTime, props: CFDictionaryRef, frame_refcon: *mut c_void, info_out: *mut u32) -> OSStatus;
    fn VTCompressionSessionCompleteFrames(session: VTSession, until: CMTime) -> OSStatus;
    fn VTCompressionSessionInvalidate(session: VTSession);
}

const AVC1: u32 = u32::from_be_bytes(*b"avc1");
const HVC1: u32 = u32::from_be_bytes(*b"hvc1");
const BGRA: u32 = u32::from_be_bytes(*b"BGRA");
/// 10-bit 4:2:0, two planes, video range (`x420`).
const X420: u32 = u32::from_be_bytes(*b"x420");

/// What the hardware makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    /// H.264 High, 8-bit, from BGRA.
    H264,
    /// HEVC Main 10, from 16-bit RGB: smooth gradients without banding.
    HevcMain10,
}

/// What the output callback fills.
#[derive(Default)]
struct Out {
    frames: Vec<Encoded>,
    sps: Vec<u8>,
    pps: Vec<u8>,
    /// The codec configuration record (`hvcC`) for HEVC.
    hvcc: Vec<u8>,
    error: Option<String>,
}

extern "C" fn on_output(refcon: *mut c_void, _frame: *mut c_void, status: OSStatus, _flags: u32, sample: CMSampleBufferRef) {
    // SAFETY: refcon is the Arc<Mutex<Out>> pointer the encoder keeps alive
    // for as long as the session exists.
    let out = unsafe { &*(refcon as *const Mutex<Out>) };
    let mut out = out.lock().unwrap_or_else(|e| e.into_inner());
    if status != 0 || sample.is_null() {
        out.error = Some(format!("hardware encoder error {status}"));
        return;
    }
    unsafe {
        let bb = CMSampleBufferGetDataBuffer(sample);
        if bb.is_null() {
            return;
        }
        let len = CMBlockBufferGetDataLength(bb);
        let mut data = vec![0u8; len];
        if CMBlockBufferCopyDataBytes(bb, 0, len, data.as_mut_ptr() as *mut c_void) != 0 {
            out.error = Some("could not read the encoded picture".into());
            return;
        }
        let mut keyframe = true;
        let atts = CMSampleBufferGetSampleAttachmentsArray(sample, 0);
        if !atts.is_null() && CFArrayGetCount(atts) > 0 {
            let d = CFArrayGetValueAtIndex(atts, 0);
            let not_sync = CFDictionaryGetValue(d, kCMSampleAttachmentKey_NotSync);
            keyframe = not_sync.is_null() || not_sync == kCFBooleanFalse;
        }
        if keyframe && out.hvcc.is_empty() {
            // HEVC: the configuration record ready-made, from the sample
            // description's extension atoms.
            let desc = CMSampleBufferGetFormatDescription(sample);
            let atoms = CMFormatDescriptionGetExtension(desc, kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms);
            if !atoms.is_null() {
                let key = CFStringCreateWithCString(std::ptr::null(), c"hvcC".as_ptr(), 0x0800_0100);
                let data = CFDictionaryGetValue(atoms, key);
                CFRelease(key);
                if !data.is_null() {
                    out.hvcc = std::slice::from_raw_parts(CFDataGetBytePtr(data), CFDataGetLength(data) as usize).to_vec();
                }
            }
        }
        if keyframe && out.sps.is_empty() {
            let desc = CMSampleBufferGetFormatDescription(sample);
            for (i, slot) in [0usize, 1].iter().enumerate() {
                let (mut p, mut size, mut count, mut hl) = (std::ptr::null(), 0usize, 0usize, 0i32);
                if CMVideoFormatDescriptionGetH264ParameterSetAtIndex(desc, *slot, &mut p, &mut size, &mut count, &mut hl) == 0 && !p.is_null() {
                    let bytes = std::slice::from_raw_parts(p, size).to_vec();
                    if i == 0 { out.sps = bytes } else { out.pps = bytes }
                }
            }
        }
        out.frames.push(Encoded { data, keyframe });
    }
}

unsafe fn number_i32(v: i32) -> CFTypeRef {
    unsafe { CFNumberCreate(std::ptr::null(), 3, &v as *const i32 as *const c_void) }
}

unsafe fn number_f64(v: f64) -> CFTypeRef {
    unsafe { CFNumberCreate(std::ptr::null(), 13, &v as *const f64 as *const c_void) }
}

unsafe fn dict(pairs: &[(CFTypeRef, CFTypeRef)]) -> CFDictionaryRef {
    let keys: Vec<CFTypeRef> = pairs.iter().map(|p| p.0).collect();
    let vals: Vec<CFTypeRef> = pairs.iter().map(|p| p.1).collect();
    unsafe { CFDictionaryCreate(std::ptr::null(), keys.as_ptr(), vals.as_ptr(), pairs.len() as isize, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks) }
}

unsafe extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const std::ffi::c_char) -> *mut c_void;
}

/// The coarsest quantiser the hardware encoder may use (see `new`).
const MAX_FRAME_QP: i32 = 20;

pub struct Encoder {
    codec: Codec,
    session: VTSession,
    out: Arc<Mutex<Out>>,
    width: u32,
    height: u32,
    fps_num: u32,
    fps_den: u32,
    next: i64,
    force_key: bool,
}

// The session is only touched from the thread that owns the encoder; the
// callback only touches `out`, behind its mutex.
unsafe impl Send for Encoder {}

impl Encoder {
    pub fn new(width: u32, height: u32, fps_num: u32, fps_den: u32, bitrate_kbps: u32, keyframe_every: u32) -> Option<Encoder> {
        Self::open(Codec::H264, width, height, fps_num, fps_den, bitrate_kbps, keyframe_every)
    }

    pub fn open(codec: Codec, width: u32, height: u32, fps_num: u32, fps_den: u32, bitrate_kbps: u32, keyframe_every: u32) -> Option<Encoder> {
        let (fourcc, pixels) = match codec {
            Codec::H264 => (AVC1, BGRA),
            Codec::HevcMain10 => (HVC1, X420),
        };
        let out = Arc::new(Mutex::new(Out::default()));
        let mut session: VTSession = std::ptr::null_mut();
        unsafe {
            let spec = dict(&[(kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder, kCFBooleanTrue), (kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder, kCFBooleanTrue)]);
            let (fmt, w, h) = (number_i32(pixels as i32), number_i32(width as i32), number_i32(height as i32));
            let io = dict(&[]);
            let attrs = dict(&[(kCVPixelBufferPixelFormatTypeKey, fmt), (kCVPixelBufferWidthKey, w), (kCVPixelBufferHeightKey, h), (kCVPixelBufferIOSurfacePropertiesKey, io)]);
            let st = VTCompressionSessionCreate(std::ptr::null(), width as i32, height as i32, fourcc, spec, attrs, std::ptr::null(), on_output, Arc::as_ptr(&out) as *mut c_void, &mut session);
            for r in [spec, attrs, io, fmt, w, h] {
                CFRelease(r);
            }
            if st != 0 || session.is_null() {
                return None;
            }
            let fps = fps_num as f64 / fps_den.max(1) as f64;
            let set = |k: CFStringRef, v: CFTypeRef| {
                VTSessionSetProperty(session, k, v);
            };
            set(kVTCompressionPropertyKey_RealTime, kCFBooleanFalse);
            set(kVTCompressionPropertyKey_AllowFrameReordering, kCFBooleanFalse);
            set(kVTCompressionPropertyKey_ProfileLevel, if codec == Codec::H264 { kVTProfileLevel_H264_High_AutoLevel } else { kVTProfileLevel_HEVC_Main10_AutoLevel });
            let br = number_i32((bitrate_kbps.max(500) * 1000) as i32);
            set(kVTCompressionPropertyKey_AverageBitRate, br);
            CFRelease(br);
            let gop = number_i32(keyframe_every.max(1) as i32);
            set(kVTCompressionPropertyKey_MaxKeyFrameInterval, gop);
            CFRelease(gop);
            let rate = number_f64(fps);
            set(kVTCompressionPropertyKey_ExpectedFrameRate, rate);
            CFRelease(rate);
            set(kVTCompressionPropertyKey_ColorPrimaries, kCVImageBufferColorPrimaries_ITU_R_709_2);
            set(kVTCompressionPropertyKey_TransferFunction, kCVImageBufferTransferFunction_ITU_R_709_2);
            set(kVTCompressionPropertyKey_YCbCrMatrix, kCVImageBufferYCbCrMatrix_ITU_R_709_2);
            // A quality floor. Left alone, the encoder settles on quantiser
            // 26 whenever a frame "looks fine" to it, budget or not: smooth
            // gradients, soft shadows and fine grain come out in hard
            // one-level bands. Looked up at run time: older systems lack it.
            let max_qp = std::env::var("STUDIO_VT_MAX_QP").ok().and_then(|v| v.parse::<i32>().ok()).unwrap_or(MAX_FRAME_QP);
            if max_qp > 0 {
                // RTLD_DEFAULT is -2 on macOS: search every loaded image.
                let key = dlsym(-2isize as *mut c_void, c"kVTCompressionPropertyKey_MaxAllowedFrameQP".as_ptr()) as *const CFStringRef;
                if !key.is_null() {
                    let q = number_i32(max_qp.clamp(1, 51));
                    set(*key, q);
                    CFRelease(q);
                }
            }
            VTCompressionSessionPrepareToEncodeFrames(session);
        }
        let mut e = Encoder { codec, session, out, width, height, fps_num, fps_den, next: 0, force_key: false };
        // The container needs the parameter sets before the first picture:
        // encode a black one to learn them, then start the real stream on
        // a keyframe.
        match codec {
            Codec::H264 => e.encode_rgba(&vec![0u8; (width * height * 4) as usize]).ok()?,
            Codec::HevcMain10 => e.encode_rgba16(&vec![0u16; (width * height * 4) as usize]).ok()?,
        };
        e.flush().ok()?;
        let ok = {
            let mut o = e.out.lock().unwrap_or_else(|x| x.into_inner());
            o.frames.clear();
            o.error.is_none() && if codec == Codec::H264 { !o.sps.is_empty() && !o.pps.is_empty() } else { !o.hvcc.is_empty() }
        };
        e.force_key = true;
        ok.then_some(e)
    }

    pub fn avcc(&self) -> Vec<u8> {
        let o = self.out.lock().unwrap_or_else(|x| x.into_inner());
        let (sps, pps) = (&o.sps, &o.pps);
        let mut v = vec![1, sps.get(1).copied().unwrap_or(100), sps.get(2).copied().unwrap_or(0), sps.get(3).copied().unwrap_or(40), 0xFF, 0xE1];
        v.extend_from_slice(&(sps.len() as u16).to_be_bytes());
        v.extend_from_slice(sps);
        v.push(1);
        v.extend_from_slice(&(pps.len() as u16).to_be_bytes());
        v.extend_from_slice(pps);
        v
    }

    /// The HEVC configuration record (`hvcC`) for the container.
    pub fn hvcc(&self) -> Vec<u8> {
        self.out.lock().unwrap_or_else(|x| x.into_inner()).hvcc.clone()
    }

    /// Hands one 16-bit RGBA picture (sRGB-encoded, straight alpha
    /// ignored) to a 10-bit encoder, as BT.709 video-range Y′CbCr 4:2:0.
    pub fn encode_rgba16(&mut self, rgba: &[u16]) -> Result<Vec<Encoded>, String> {
        let (w, h) = (self.width as usize, self.height as usize);
        if self.codec != Codec::HevcMain10 {
            return Err("this encoder takes 8-bit pictures".into());
        }
        if rgba.len() < w * h * 4 {
            return Err("picture is smaller than the encoder".into());
        }
        unsafe {
            let pool = VTCompressionSessionGetPixelBufferPool(self.session);
            if pool.is_null() {
                return Err("the hardware encoder has no picture pool".into());
            }
            let mut pb: CVPixelBufferRef = std::ptr::null_mut();
            if CVPixelBufferPoolCreatePixelBuffer(std::ptr::null(), pool, &mut pb) != 0 || pb.is_null() {
                return Err("could not get a picture buffer".into());
            }
            CVPixelBufferLockBaseAddress(pb, 0);
            let (yp, ys) = (CVPixelBufferGetBaseAddressOfPlane(pb, 0), CVPixelBufferGetBytesPerRowOfPlane(pb, 0));
            let (cp, cs) = (CVPixelBufferGetBaseAddressOfPlane(pb, 1), CVPixelBufferGetBytesPerRowOfPlane(pb, 1));
            // BT.709 with 10-bit video-range levels (Y 64-940, C 64-960),
            // stored in the top 10 bits of each 16-bit word.
            let rgb = |x: usize, y: usize| {
                let i = (y.min(h - 1) * w + x.min(w - 1)) * 4;
                [rgba[i] as f32 / 65535.0, rgba[i + 1] as f32 / 65535.0, rgba[i + 2] as f32 / 65535.0]
            };
            let luma = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
            let word = |v: f32| ((v.round().clamp(0.0, 1023.0) as u16) << 6).to_le_bytes();
            for y in 0..h {
                let row = std::slice::from_raw_parts_mut(yp.add(y * ys), w * 2);
                for x in 0..w {
                    row[x * 2..x * 2 + 2].copy_from_slice(&word(64.0 + 876.0 * luma(rgb(x, y))));
                }
            }
            for y in 0..h.div_ceil(2) {
                let row = std::slice::from_raw_parts_mut(cp.add(y * cs), w.div_ceil(2) * 4);
                for x in 0..w.div_ceil(2) {
                    // The average of the 2x2 block.
                    let mut a = [0f32; 3];
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let c = rgb(x * 2 + dx, y * 2 + dy);
                        for k in 0..3 {
                            a[k] += c[k] * 0.25;
                        }
                    }
                    let yy = luma(a);
                    let cb = 512.0 + 896.0 * (a[2] - yy) / 1.8556;
                    let cr = 512.0 + 896.0 * (a[0] - yy) / 1.5748;
                    row[x * 4..x * 4 + 2].copy_from_slice(&word(cb));
                    row[x * 4 + 2..x * 4 + 4].copy_from_slice(&word(cr));
                }
            }
            CVPixelBufferUnlockBaseAddress(pb, 0);
            self.submit(pb)
        }
    }

    pub fn encode_rgba(&mut self, rgba: &[u8]) -> Result<Vec<Encoded>, String> {
        let (w, h) = (self.width as usize, self.height as usize);
        if rgba.len() < w * h * 4 {
            return Err("picture is smaller than the encoder".into());
        }
        unsafe {
            let pool = VTCompressionSessionGetPixelBufferPool(self.session);
            if pool.is_null() {
                return Err("the hardware encoder has no picture pool".into());
            }
            let mut pb: CVPixelBufferRef = std::ptr::null_mut();
            if CVPixelBufferPoolCreatePixelBuffer(std::ptr::null(), pool, &mut pb) != 0 || pb.is_null() {
                return Err("could not get a picture buffer".into());
            }
            CVPixelBufferLockBaseAddress(pb, 0);
            let base = CVPixelBufferGetBaseAddress(pb);
            let stride = CVPixelBufferGetBytesPerRow(pb);
            for y in 0..h {
                let src = &rgba[y * w * 4..(y + 1) * w * 4];
                let dst = std::slice::from_raw_parts_mut(base.add(y * stride), w * 4);
                for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                    d[0] = s[2];
                    d[1] = s[1];
                    d[2] = s[0];
                    d[3] = 255;
                }
            }
            CVPixelBufferUnlockBaseAddress(pb, 0);
            self.submit(pb)
        }
    }

    /// Encodes a filled picture buffer (and releases it).
    unsafe fn submit(&mut self, pb: CVPixelBufferRef) -> Result<Vec<Encoded>, String> {
        unsafe {
            let pts = CMTime { value: self.next * self.fps_den as i64, timescale: self.fps_num as i32, flags: VALID, epoch: 0 };
            let dur = CMTime { value: self.fps_den as i64, timescale: self.fps_num as i32, flags: VALID, epoch: 0 };
            self.next += 1;
            let props = if self.force_key { dict(&[(kVTEncodeFrameOptionKey_ForceKeyFrame, kCFBooleanTrue)]) } else { std::ptr::null() };
            self.force_key = false;
            let st = VTCompressionSessionEncodeFrame(self.session, pb, pts, dur, props, std::ptr::null_mut(), std::ptr::null_mut());
            if !props.is_null() {
                CFRelease(props);
            }
            CFRelease(pb);
            if st != 0 {
                return Err(format!("hardware encoder refused a picture ({st})"));
            }
        }
        self.take()
    }

    fn flush(&mut self) -> Result<(), String> {
        unsafe {
            VTCompressionSessionCompleteFrames(self.session, INVALID);
        }
        Ok(())
    }

    fn take(&mut self) -> Result<Vec<Encoded>, String> {
        let mut o = self.out.lock().unwrap_or_else(|x| x.into_inner());
        if let Some(e) = o.error.take() {
            return Err(e);
        }
        Ok(std::mem::take(&mut o.frames))
    }

    pub fn finish(&mut self) -> Result<Vec<Encoded>, String> {
        self.flush()?;
        self.take()
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe {
            VTCompressionSessionCompleteFrames(self.session, INVALID);
            VTCompressionSessionInvalidate(self.session);
            CFRelease(self.session as CFTypeRef);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn hardware_encoder_makes_a_stream() {
        let Some(mut e) = super::Encoder::new(640, 360, 30, 1, 4000, 30) else {
            eprintln!("no hardware encoder here; skipped");
            return;
        };
        assert!(e.avcc().len() > 10);
        let mut got = Vec::new();
        for i in 0..60u32 {
            let px: Vec<u8> = (0..640 * 360).flat_map(|p| [(p as u32 % 640 + i * 4) as u8, (p as u32 / 640) as u8, 128, 255]).collect();
            got.extend(e.encode_rgba(&px).unwrap());
        }
        got.extend(e.finish().unwrap());
        assert_eq!(got.len(), 60, "every picture comes out once");
        assert!(got[0].keyframe, "the stream starts on a keyframe");
        assert!(got.iter().all(|f| f.data.len() > 4));
    }

    #[test]
    fn hardware_hevc_main10_makes_a_stream() {
        let Some(mut e) = super::Encoder::open(super::Codec::HevcMain10, 640, 360, 30, 1, 4000, 30) else {
            eprintln!("no hardware HEVC encoder here; skipped");
            return;
        };
        let hvcc = e.hvcc();
        assert!(hvcc.len() > 20 && hvcc[0] == 1, "an hvcC record");
        // general_profile_idc 2 = Main 10.
        assert_eq!(hvcc[1] & 0x1F, 2, "Main 10 profile");
        let mut got = Vec::new();
        for i in 0..30u32 {
            let px: Vec<u16> = (0..640 * 360).flat_map(|p| [((p % 640) * 100 + i * 50) as u16, ((p / 640) * 180) as u16, 30000, 65535]).collect();
            got.extend(e.encode_rgba16(&px).unwrap());
        }
        got.extend(e.finish().unwrap());
        assert_eq!(got.len(), 30);
        assert!(got[0].keyframe);
    }
}
