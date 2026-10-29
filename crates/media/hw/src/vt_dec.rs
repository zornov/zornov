// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! VideoToolbox decompression, bound by hand: AVCC H.264, HEVC or AV1 samples in,
//! 8-bit 4:2:0 planes out (the same shape Studio's software decoder
//! gives), tagged with the sample they came from.

#![allow(non_upper_case_globals, non_snake_case)]

use super::Decoded;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};

type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFDictionaryRef = *const c_void;
type CFAllocatorRef = *const c_void;
type OSStatus = i32;
type CVImageBufferRef = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

#[repr(C)]
struct OutputCallback {
    callback: extern "C" fn(*mut c_void, *mut c_void, OSStatus, u32, CVImageBufferRef, CMTime, CMTime),
    refcon: *mut c_void,
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFBooleanTrue: CFTypeRef;
    static kCFTypeDictionaryKeyCallBacks: u8;
    static kCFTypeDictionaryValueCallBacks: u8;
    fn CFRelease(cf: CFTypeRef);
    fn CFDataCreate(alloc: CFAllocatorRef, bytes: *const u8, len: isize) -> CFTypeRef;
    fn CFStringCreateWithCString(alloc: CFAllocatorRef, s: *const std::ffi::c_char, encoding: u32) -> CFStringRef;
    fn CFNumberCreate(alloc: CFAllocatorRef, kind: isize, value: *const c_void) -> CFTypeRef;
    fn CFDictionaryCreate(alloc: CFAllocatorRef, keys: *const CFTypeRef, values: *const CFTypeRef, n: isize, kcb: *const u8, vcb: *const u8) -> CFDictionaryRef;
}

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    fn CMVideoFormatDescriptionCreateFromH264ParameterSets(alloc: CFAllocatorRef, count: usize, sets: *const *const u8, sizes: *const usize, nal_header_len: i32, out: *mut *mut c_void) -> OSStatus;
    fn CMVideoFormatDescriptionCreateFromHEVCParameterSets(alloc: CFAllocatorRef, count: usize, sets: *const *const u8, sizes: *const usize, nal_header_len: i32, extensions: CFDictionaryRef, out: *mut *mut c_void) -> OSStatus;
    static kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms: CFStringRef;
    fn CMVideoFormatDescriptionCreate(alloc: CFAllocatorRef, codec: u32, width: i32, height: i32, extensions: CFDictionaryRef, out: *mut *mut c_void) -> OSStatus;
    fn CMBlockBufferCreateWithMemoryBlock(alloc: CFAllocatorRef, block: *mut c_void, len: usize, block_alloc: CFAllocatorRef, custom: *const c_void, offset: usize, data_len: usize, flags: u32, out: *mut *mut c_void) -> OSStatus;
    fn CMBlockBufferReplaceDataBytes(src: *const c_void, dest: *mut c_void, offset: usize, len: usize) -> OSStatus;
    fn CMSampleBufferCreateReady(alloc: CFAllocatorRef, data: *mut c_void, desc: *mut c_void, n: isize, ntiming: isize, timing: *const c_void, nsizes: isize, sizes: *const usize, out: *mut *mut c_void) -> OSStatus;
}

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    static kCVPixelBufferPixelFormatTypeKey: CFStringRef;
    static kCVPixelBufferIOSurfacePropertiesKey: CFStringRef;
    fn CVPixelBufferLockBaseAddress(pb: CVImageBufferRef, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(pb: CVImageBufferRef, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddressOfPlane(pb: CVImageBufferRef, plane: usize) -> *const u8;
    fn CVPixelBufferGetBytesPerRowOfPlane(pb: CVImageBufferRef, plane: usize) -> usize;
    fn CVPixelBufferGetWidthOfPlane(pb: CVImageBufferRef, plane: usize) -> usize;
    fn CVPixelBufferGetHeightOfPlane(pb: CVImageBufferRef, plane: usize) -> usize;
}

#[link(name = "VideoToolbox", kind = "framework")]
unsafe extern "C" {
    static kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder: CFStringRef;
    static kVTVideoDecoderSpecification_RequireHardwareAcceleratedVideoDecoder: CFStringRef;
    fn VTIsHardwareDecodeSupported(codec: u32) -> u8;
    fn VTRegisterSupplementalVideoDecoderIfAvailable(codec: u32);
    fn VTDecompressionSessionCreate(alloc: CFAllocatorRef, desc: *mut c_void, spec: CFDictionaryRef, attrs: CFDictionaryRef, cb: *const OutputCallback, out: *mut *mut c_void) -> OSStatus;
    fn VTDecompressionSessionDecodeFrame(s: *mut c_void, sample: *mut c_void, flags: u32, refcon: *mut c_void, info_out: *mut u32) -> OSStatus;
    fn VTDecompressionSessionWaitForAsynchronousFrames(s: *mut c_void) -> OSStatus;
    fn VTDecompressionSessionFinishDelayedFrames(s: *mut c_void) -> OSStatus;
    fn VTDecompressionSessionInvalidate(s: *mut c_void);
}

/// 4:2:0, 8-bit, video range, Y plane and interleaved CbCr.
const NV12_VIDEO: u32 = u32::from_be_bytes(*b"420v");

/// `kCMVideoCodecType_AV1`.
const CODEC_AV1: u32 = u32::from_be_bytes(*b"av01");

#[derive(Default)]
struct Out {
    frames: Vec<Decoded>,
    error: Option<String>,
}

extern "C" fn on_output(refcon: *mut c_void, frame: *mut c_void, status: OSStatus, _flags: u32, image: CVImageBufferRef, _pts: CMTime, _dur: CMTime) {
    // SAFETY: refcon is the decoder's Arc<Mutex<Out>>, alive while the session is.
    let out = unsafe { &*(refcon as *const Mutex<Out>) };
    let mut out = out.lock().unwrap_or_else(|e| e.into_inner());
    if status != 0 || image.is_null() {
        if status != 0 {
            out.error = Some(format!("hardware decoder error {status}"));
        }
        return;
    }
    unsafe {
        CVPixelBufferLockBaseAddress(image, 1);
        let (w, h) = (CVPixelBufferGetWidthOfPlane(image, 0), CVPixelBufferGetHeightOfPlane(image, 0));
        let (cw, ch) = (CVPixelBufferGetWidthOfPlane(image, 1), CVPixelBufferGetHeightOfPlane(image, 1));
        let (ys, uvs) = (CVPixelBufferGetBytesPerRowOfPlane(image, 0), CVPixelBufferGetBytesPerRowOfPlane(image, 1));
        let (yp, uvp) = (CVPixelBufferGetBaseAddressOfPlane(image, 0), CVPixelBufferGetBaseAddressOfPlane(image, 1));
        // Luma row by row; chroma split from CbCr pairs in one pass over
        // buffers sized up front (this runs for every picture, 4K included).
        let mut y = vec![0u8; w * h];
        for (r, dst) in y.chunks_exact_mut(w).enumerate() {
            dst.copy_from_slice(std::slice::from_raw_parts(yp.add(r * ys), w));
        }
        let (mut u, mut v) = (vec![0u8; cw * ch], vec![0u8; cw * ch]);
        for r in 0..ch {
            let row = std::slice::from_raw_parts(uvp.add(r * uvs), cw * 2);
            let (ur, vr) = (&mut u[r * cw..(r + 1) * cw], &mut v[r * cw..(r + 1) * cw]);
            for ((p, a), b) in row.chunks_exact(2).zip(ur.iter_mut()).zip(vr.iter_mut()) {
                *a = p[0];
                *b = p[1];
            }
        }
        CVPixelBufferUnlockBaseAddress(image, 1);
        out.frames.push(Decoded { sample: frame as usize as i64, width: w as u32, height: h as u32, y, u, v, chroma_width: cw as u32, chroma_height: ch as u32 });
    }
}

unsafe fn dict(pairs: &[(CFTypeRef, CFTypeRef)]) -> CFDictionaryRef {
    let keys: Vec<CFTypeRef> = pairs.iter().map(|p| p.0).collect();
    let vals: Vec<CFTypeRef> = pairs.iter().map(|p| p.1).collect();
    unsafe { CFDictionaryCreate(std::ptr::null(), keys.as_ptr(), vals.as_ptr(), pairs.len() as isize, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks) }
}

/// SPS and PPS lists and the length-field size from an avcC record.
fn parse_avcc(avcc: &[u8]) -> Option<(Vec<Vec<u8>>, i32)> {
    let mut sets = Vec::new();
    let nal_len = (*avcc.get(4)? & 3) as i32 + 1;
    let mut i = 5;
    let nsps = (*avcc.get(i)? & 0x1f) as usize;
    i += 1;
    for _ in 0..nsps {
        let len = u16::from_be_bytes([*avcc.get(i)?, *avcc.get(i + 1)?]) as usize;
        sets.push(avcc.get(i + 2..i + 2 + len)?.to_vec());
        i += 2 + len;
    }
    let npps = *avcc.get(i)? as usize;
    i += 1;
    for _ in 0..npps {
        let len = u16::from_be_bytes([*avcc.get(i)?, *avcc.get(i + 1)?]) as usize;
        sets.push(avcc.get(i + 2..i + 2 + len)?.to_vec());
        i += 2 + len;
    }
    (!sets.is_empty()).then_some((sets, nal_len))
}

pub struct Decoder {
    session: *mut c_void,
    desc: *mut c_void,
    out: Arc<Mutex<Out>>,
}

unsafe impl Send for Decoder {}

impl Decoder {
    pub fn new(avcc: &[u8]) -> Option<Decoder> {
        let (sets, nal_len) = parse_avcc(avcc)?;
        let ptrs: Vec<*const u8> = sets.iter().map(|s| s.as_ptr()).collect();
        let sizes: Vec<usize> = sets.iter().map(|s| s.len()).collect();
        let mut desc: *mut c_void = std::ptr::null_mut();
        unsafe {
            if CMVideoFormatDescriptionCreateFromH264ParameterSets(std::ptr::null(), sets.len(), ptrs.as_ptr(), sizes.as_ptr(), nal_len, &mut desc) != 0 || desc.is_null() {
                return None;
            }
        }
        Self::with_description(desc)
    }

    /// HEVC: VPS, SPS and PPS NAL units and the length-field size (from
    /// the hvcC record).
    pub fn new_hevc(sets: &[Vec<u8>], nal_len: i32) -> Option<Decoder> {
        if sets.is_empty() {
            return None;
        }
        let ptrs: Vec<*const u8> = sets.iter().map(|s| s.as_ptr()).collect();
        let sizes: Vec<usize> = sets.iter().map(|s| s.len()).collect();
        let mut desc: *mut c_void = std::ptr::null_mut();
        unsafe {
            if CMVideoFormatDescriptionCreateFromHEVCParameterSets(std::ptr::null(), sets.len(), ptrs.as_ptr(), sizes.as_ptr(), nal_len, std::ptr::null(), &mut desc) != 0 || desc.is_null() {
                return None;
            }
        }
        Self::with_description(desc)
    }

    /// Whether this Mac's video hardware decodes AV1.
    pub fn av1_supported() -> bool {
        unsafe {
            VTRegisterSupplementalVideoDecoderIfAvailable(CODEC_AV1);
            VTIsHardwareDecodeSupported(CODEC_AV1) != 0
        }
    }

    /// AV1: the stream's `av1C` record (sent as the `av1C` sample-description
    /// atom) and the coded size. `None` when there is no hardware for it.
    pub fn new_av1(av1c: &[u8], width: u32, height: u32) -> Option<Decoder> {
        if av1c.len() < 4 || width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 || !Self::av1_supported() {
            return None;
        }
        let mut desc: *mut c_void = std::ptr::null_mut();
        unsafe {
            let key = CFStringCreateWithCString(std::ptr::null(), c"av1C".as_ptr(), 0x0800_0100);
            let data = CFDataCreate(std::ptr::null(), av1c.as_ptr(), av1c.len() as isize);
            let atoms = dict(&[(key, data)]);
            let ext = dict(&[(kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms, atoms)]);
            let st = CMVideoFormatDescriptionCreate(std::ptr::null(), CODEC_AV1, width as i32, height as i32, ext, &mut desc);
            for r in [ext, atoms, data, key] {
                CFRelease(r);
            }
            if st != 0 || desc.is_null() {
                return None;
            }
        }
        Self::with_description(desc)
    }

    /// A hardware session for a format description (released on failure).
    fn with_description(desc: *mut c_void) -> Option<Decoder> {
        let out = Arc::new(Mutex::new(Out::default()));
        let mut session: *mut c_void = std::ptr::null_mut();
        unsafe {
            let spec = dict(&[(kVTVideoDecoderSpecification_EnableHardwareAcceleratedVideoDecoder, kCFBooleanTrue), (kVTVideoDecoderSpecification_RequireHardwareAcceleratedVideoDecoder, kCFBooleanTrue)]);
            let fmt_v = NV12_VIDEO as i32;
            let fmt = CFNumberCreate(std::ptr::null(), 3, &fmt_v as *const i32 as *const c_void);
            let io = dict(&[]);
            let attrs = dict(&[(kCVPixelBufferPixelFormatTypeKey, fmt), (kCVPixelBufferIOSurfacePropertiesKey, io)]);
            let cb = OutputCallback { callback: on_output, refcon: Arc::as_ptr(&out) as *mut c_void };
            let st = VTDecompressionSessionCreate(std::ptr::null(), desc, spec, attrs, &cb, &mut session);
            for r in [spec, attrs, io, fmt] {
                CFRelease(r);
            }
            if st != 0 || session.is_null() {
                CFRelease(desc as CFTypeRef);
                return None;
            }
        }
        Some(Decoder { session, desc, out })
    }

    /// Decodes one sample (AVCC framed); returns the pictures that came out.
    pub fn decode(&mut self, data: &[u8], sample: i64) -> Result<Vec<Decoded>, String> {
        unsafe {
            let mut bb: *mut c_void = std::ptr::null_mut();
            if CMBlockBufferCreateWithMemoryBlock(std::ptr::null(), std::ptr::null_mut(), data.len(), std::ptr::null(), std::ptr::null(), 0, data.len(), 0, &mut bb) != 0 {
                return Err("could not wrap a sample".into());
            }
            CMBlockBufferReplaceDataBytes(data.as_ptr() as *const c_void, bb, 0, data.len());
            let size = data.len();
            let mut sb: *mut c_void = std::ptr::null_mut();
            let st = CMSampleBufferCreateReady(std::ptr::null(), bb, self.desc, 1, 0, std::ptr::null(), 1, &size, &mut sb);
            CFRelease(bb as CFTypeRef);
            if st != 0 || sb.is_null() {
                return Err("could not wrap a sample".into());
            }
            let st = VTDecompressionSessionDecodeFrame(self.session, sb, 0, sample as usize as *mut c_void, std::ptr::null_mut());
            CFRelease(sb as CFTypeRef);
            if st != 0 {
                return Err(format!("hardware decoder refused a sample ({st})"));
            }
        }
        self.take()
    }

    pub fn flush(&mut self) -> Vec<Decoded> {
        unsafe {
            VTDecompressionSessionFinishDelayedFrames(self.session);
            VTDecompressionSessionWaitForAsynchronousFrames(self.session);
        }
        self.take().unwrap_or_default()
    }

    /// Ready for a new run from a sync sample (a seek), keeping the session:
    /// a new session is slow for its first frames (about 20 at 4K), so
    /// every seek or replay began with a stutter.
    pub fn reset(&mut self) {
        let _ = self.flush();
    }

    fn take(&mut self) -> Result<Vec<Decoded>, String> {
        let mut o = self.out.lock().unwrap_or_else(|x| x.into_inner());
        if let Some(e) = o.error.take() {
            return Err(e);
        }
        Ok(std::mem::take(&mut o.frames))
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            VTDecompressionSessionWaitForAsynchronousFrames(self.session);
            VTDecompressionSessionInvalidate(self.session);
            CFRelease(self.session as CFTypeRef);
            CFRelease(self.desc as CFTypeRef);
        }
    }
}
