// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Speaker output and microphone input, plus beat detection and panel
//! previews. macOS goes through AudioQueue / CoreAudio with hand-written
//! bindings; Windows and Linux through `cpal` (WASAPI, ALSA / PulseAudio),
//! converting to the device's own rate and channel count.
//!
//! Callers push interleaved stereo f32 samples; the AudioQueue thread pulls
//! them in fixed-size buffers and plays silence when the queue runs dry.

pub mod beats;
pub mod input;
pub mod preview;

pub use beats::{detect_beats, detect_onsets};
pub use input::{input_devices, AudioIn, InputDevice};
pub use preview::{resample_stereo, PanelPreview};

/// Output device (CoreAudio UID) for streams opened from now on; None = the
/// system default. Set from Settings > Performance > Sound output device.
static OUTPUT_DEVICE: Mutex<Option<String>> = Mutex::new(None);

pub fn set_output_device(uid: Option<String>) {
    if let Ok(mut d) = OUTPUT_DEVICE.lock() {
        *d = uid;
    }
}

pub fn output_device() -> Option<String> {
    OUTPUT_DEVICE.lock().ok().and_then(|d| d.clone())
}

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};

pub const RATE: u32 = 48_000;
const BUFFERS: usize = 3;
const FRAMES_PER_BUFFER: u32 = 1024;

#[derive(Default)]
struct Shared {
    queue: VecDeque<f32>,
    played_frames: u64,
    /// Buffers the device wanted that the queue could only partly fill
    /// while sound was flowing (heard as clicks).
    underruns: u64,
    /// Sound has been pushed since the last clear.
    flowing: bool,
    /// Output volume where the platform has no volume control of its own.
    #[cfg(not(target_os = "macos"))]
    volume: Option<f32>,
}

impl Shared {
    /// Fills `out` (interleaved stereo) from the queue, zero-padding.
    fn fill(&mut self, out: &mut [f32]) {
        let n = out.len().min(self.queue.len());
        if self.flowing && n < out.len() {
            self.underruns += 1;
        }
        for (dst, src) in out.iter_mut().zip(self.queue.drain(..n)) {
            *dst = src;
        }
        out[n..].fill(0.0);
        self.played_frames += (out.len() / 2) as u64;
    }
}

#[cfg(target_os = "macos")]
pub(crate) mod ffi {
    use std::ffi::c_void;

    pub type OSStatus = i32;
    pub type AudioQueueRef = *mut c_void;

    #[repr(C)]
    pub struct AudioStreamBasicDescription {
        pub sample_rate: f64,
        pub format_id: u32,
        pub format_flags: u32,
        pub bytes_per_packet: u32,
        pub frames_per_packet: u32,
        pub bytes_per_frame: u32,
        pub channels_per_frame: u32,
        pub bits_per_channel: u32,
        pub reserved: u32,
    }

    #[repr(C)]
    pub struct AudioQueueBuffer {
        pub audio_data_bytes_capacity: u32,
        pub audio_data: *mut c_void,
        pub audio_data_byte_size: u32,
        pub user_data: *mut c_void,
        pub packet_description_capacity: u32,
        pub packet_descriptions: *mut c_void,
        pub packet_description_count: u32,
    }

    pub type AudioQueueBufferRef = *mut AudioQueueBuffer;
    pub type OutputCallback = extern "C" fn(*mut c_void, AudioQueueRef, AudioQueueBufferRef);

    pub const K_AUDIO_FORMAT_LINEAR_PCM: u32 = u32::from_be_bytes(*b"lpcm");
    pub const K_FLAG_FLOAT: u32 = 1;
    pub const K_FLAG_PACKED: u32 = 8;
    pub const K_PARAM_VOLUME: u32 = 1;

    #[link(name = "AudioToolbox", kind = "framework")]
    unsafe extern "C" {
        pub fn AudioQueueNewOutput(
            format: *const AudioStreamBasicDescription,
            callback: OutputCallback,
            user_data: *mut c_void,
            run_loop: *mut c_void,
            run_loop_mode: *const c_void,
            flags: u32,
            out_queue: *mut AudioQueueRef,
        ) -> OSStatus;
        pub fn AudioQueueAllocateBuffer(q: AudioQueueRef, bytes: u32, out: *mut AudioQueueBufferRef) -> OSStatus;
        pub fn AudioQueueEnqueueBuffer(q: AudioQueueRef, b: AudioQueueBufferRef, n: u32, descs: *const c_void) -> OSStatus;
        pub fn AudioQueueStart(q: AudioQueueRef, at: *const c_void) -> OSStatus;
        pub fn AudioQueueStop(q: AudioQueueRef, immediate: u8) -> OSStatus;
        pub fn AudioQueueDispose(q: AudioQueueRef, immediate: u8) -> OSStatus;
        pub fn AudioQueueSetParameter(q: AudioQueueRef, id: u32, value: f32) -> OSStatus;
        pub fn AudioQueueNewInput(
            format: *const AudioStreamBasicDescription,
            callback: InputCallback,
            user_data: *mut c_void,
            run_loop: *mut c_void,
            run_loop_mode: *const c_void,
            flags: u32,
            out_queue: *mut AudioQueueRef,
        ) -> OSStatus;
        pub fn AudioQueueSetProperty(q: AudioQueueRef, id: u32, data: *const c_void, size: u32) -> OSStatus;
    }

    pub type InputCallback = extern "C" fn(*mut c_void, AudioQueueRef, AudioQueueBufferRef, *const c_void, u32, *const c_void);

    pub const fn fourcc(b: &[u8; 4]) -> u32 {
        u32::from_be_bytes(*b)
    }
    pub const K_AUDIO_QUEUE_PROPERTY_CURRENT_DEVICE: u32 = fourcc(b"aqcd");
    pub const K_SYSTEM_OBJECT: u32 = 1;
    pub const K_HW_DEVICES: u32 = fourcc(b"dev#");
    pub const K_HW_DEFAULT_INPUT: u32 = fourcc(b"dIn ");
    pub const K_SCOPE_GLOBAL: u32 = fourcc(b"glob");
    pub const K_SCOPE_INPUT: u32 = fourcc(b"inpt");
    pub const K_DEVICE_STREAMS: u32 = fourcc(b"stm#");
    pub const K_OBJECT_NAME: u32 = fourcc(b"lnam");
    pub const K_DEVICE_UID: u32 = fourcc(b"uid ");
    pub const K_UTF8: u32 = 0x0800_0100;

    #[repr(C)]
    pub struct PropertyAddress {
        pub selector: u32,
        pub scope: u32,
        pub element: u32,
    }

    pub type CFStringRef = *const c_void;

    #[link(name = "CoreAudio", kind = "framework")]
    unsafe extern "C" {
        pub fn AudioObjectGetPropertyDataSize(id: u32, addr: *const PropertyAddress, qual_size: u32, qual: *const c_void, out_size: *mut u32) -> OSStatus;
        pub fn AudioObjectGetPropertyData(id: u32, addr: *const PropertyAddress, qual_size: u32, qual: *const c_void, io_size: *mut u32, out: *mut c_void) -> OSStatus;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        pub fn CFStringGetCString(s: CFStringRef, buf: *mut std::ffi::c_char, size: isize, encoding: u32) -> u8;
        pub fn CFStringCreateWithCString(alloc: *const c_void, s: *const std::ffi::c_char, encoding: u32) -> CFStringRef;
        pub fn CFRelease(p: *const c_void);
    }
}

/// An open output stream at 48 kHz stereo.
pub struct AudioOut {
    shared: Arc<Mutex<Shared>>,
    #[cfg(target_os = "macos")]
    queue: ffi::AudioQueueRef,
    #[cfg(target_os = "macos")]
    user: *mut Mutex<Shared>,
    #[cfg(not(target_os = "macos"))]
    _stream: cpal::Stream,
}

// The queue handle is only used from the owning thread's methods and Drop; the
// callback touches only the shared mutex.
unsafe impl Send for AudioOut {}

#[cfg(target_os = "macos")]
extern "C" fn callback(user: *mut c_void, q: ffi::AudioQueueRef, buf: ffi::AudioQueueBufferRef) {
    // SAFETY: `user` is the Mutex kept alive by AudioOut until the queue is disposed;
    // `buf` is a buffer allocated for this queue with capacity for FRAMES_PER_BUFFER frames.
    unsafe {
        let shared = &*(user as *const Mutex<Shared>);
        let b = &mut *buf;
        let samples = (b.audio_data_bytes_capacity / 4) as usize;
        let out = std::slice::from_raw_parts_mut(b.audio_data as *mut f32, samples);
        match shared.lock() {
            Ok(mut s) => s.fill(out),
            Err(_) => out.fill(0.0),
        }
        b.audio_data_byte_size = b.audio_data_bytes_capacity;
        ffi::AudioQueueEnqueueBuffer(q, buf, 0, std::ptr::null());
    }
}

impl AudioOut {
    #[cfg(target_os = "macos")]
    pub fn open() -> Result<Self, String> {
        let shared = Arc::new(Mutex::new(Shared::default()));
        // The callback gets its own strong reference, released in Drop.
        let user = Arc::into_raw(shared.clone()) as *mut Mutex<Shared>;
        let fmt = ffi::AudioStreamBasicDescription {
            sample_rate: RATE as f64,
            format_id: ffi::K_AUDIO_FORMAT_LINEAR_PCM,
            format_flags: ffi::K_FLAG_FLOAT | ffi::K_FLAG_PACKED,
            bytes_per_packet: 8,
            frames_per_packet: 1,
            bytes_per_frame: 8,
            channels_per_frame: 2,
            bits_per_channel: 32,
            reserved: 0,
        };
        let mut queue: ffi::AudioQueueRef = std::ptr::null_mut();
        // SAFETY: valid format pointer, callback with the expected signature, null run loop
        // (AudioQueue uses its own thread), and an out-pointer to a local.
        let status = unsafe { ffi::AudioQueueNewOutput(&fmt, callback, user as *mut c_void, std::ptr::null_mut(), std::ptr::null(), 0, &mut queue) };
        if status != 0 || queue.is_null() {
            // SAFETY: reclaim the reference handed to the callback; no queue exists.
            unsafe { drop(Arc::from_raw(user as *const Mutex<Shared>)) };
            return Err(format!("Audio output is unavailable (AudioQueueNewOutput error {status})."));
        }
        let me = Self { shared, queue, user };
        if let Some(uid) = output_device().and_then(|u| std::ffi::CString::new(u).ok()) {
            // SAFETY: a CFString made from a valid C string, released after use; the
            // property takes a pointer to the CFStringRef. A failure keeps the default device.
            unsafe {
                let s = ffi::CFStringCreateWithCString(std::ptr::null(), uid.as_ptr(), ffi::K_UTF8);
                if !s.is_null() {
                    ffi::AudioQueueSetProperty(me.queue, ffi::K_AUDIO_QUEUE_PROPERTY_CURRENT_DEVICE, &s as *const _ as *const c_void, std::mem::size_of::<ffi::CFStringRef>() as u32);
                    ffi::CFRelease(s);
                }
            }
        }
        for _ in 0..BUFFERS {
            let mut b: ffi::AudioQueueBufferRef = std::ptr::null_mut();
            // SAFETY: queue is valid; buffers are owned by the queue and freed on dispose.
            unsafe {
                if ffi::AudioQueueAllocateBuffer(me.queue, FRAMES_PER_BUFFER * 8, &mut b) != 0 || b.is_null() {
                    return Err("Audio output could not allocate buffers.".into());
                }
                callback(me.user as *mut c_void, me.queue, b);
            }
        }
        // SAFETY: queue is valid and primed.
        let status = unsafe { ffi::AudioQueueStart(me.queue, std::ptr::null()) };
        if status != 0 {
            return Err(format!("Audio output could not start (error {status})."));
        }
        Ok(me)
    }

    #[cfg(not(target_os = "macos"))]
    pub fn open() -> Result<Self, String> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let host = cpal::default_host();
        let wanted = output_device();
        let device = wanted
            .and_then(|name| host.output_devices().ok()?.find(|d| d.name().ok().as_deref() == Some(name.as_str())))
            .or_else(|| host.default_output_device())
            .ok_or("No sound output device was found.")?;
        let def = device.default_output_config().map_err(|e| format!("Audio output is unavailable: {e}"))?;
        let channels = def.channels().max(1) as usize;
        let rate = def.sample_rate().0.max(1);
        let config = cpal::StreamConfig { channels: channels as u16, sample_rate: cpal::SampleRate(rate), buffer_size: cpal::BufferSize::Default };
        let shared = Arc::new(Mutex::new(Shared::default()));
        let cb_shared = shared.clone();
        // Our 48 kHz stereo stream, resampled (linear) to the device rate and
        // spread over its channels.
        let step = RATE as f64 / rate as f64;
        let mut pos = 0.0f64;
        let mut prev = [0.0f32; 2];
        let mut next = [0.0f32; 2];
        let stream = device
            .build_output_stream(
                &config,
                move |data: &mut [f32], _| {
                    let mut s = cb_shared.lock().unwrap_or_else(|e| e.into_inner());
                    let vol = s.volume.unwrap_or(1.0);
                    for frame in data.chunks_mut(channels) {
                        while pos >= 1.0 {
                            prev = next;
                            let mut two = [0.0f32; 2];
                            s.fill(&mut two);
                            next = two;
                            pos -= 1.0;
                        }
                        let k = pos as f32;
                        let l = (prev[0] + (next[0] - prev[0]) * k) * vol;
                        let r = (prev[1] + (next[1] - prev[1]) * k) * vol;
                        match frame.len() {
                            1 => frame[0] = (l + r) * 0.5,
                            _ => {
                                frame[0] = l;
                                frame[1] = r;
                                for c in frame.iter_mut().skip(2) {
                                    *c = 0.0;
                                }
                            }
                        }
                        pos += step;
                    }
                },
                |e| eprintln!("audio output: {e}"),
                None,
            )
            .map_err(|e| format!("Audio output could not open: {e}"))?;
        stream.play().map_err(|e| format!("Audio output could not start: {e}"))?;
        Ok(Self { shared, _stream: stream })
    }

    /// Appends interleaved stereo samples.
    pub fn push(&self, samples: &[f32]) {
        let mut s = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        s.queue.extend(samples.iter().copied());
        s.flowing = true;
    }

    /// Drops everything not yet played (used on pause and seek).
    pub fn clear(&self) {
        let mut s = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        s.queue.clear();
        s.flowing = false;
    }

    /// Times the device ran out of queued sound mid-buffer.
    pub fn underruns(&self) -> u64 {
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).underruns
    }

    /// Stereo frames waiting to be played.
    pub fn queued_frames(&self) -> usize {
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).queue.len() / 2
    }

    /// Frames handed to the device so far (including silence).
    pub fn played_frames(&self) -> u64 {
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).played_frames
    }

    pub fn set_volume(&self, v: f32) {
        #[cfg(not(target_os = "macos"))]
        {
            self.shared.lock().unwrap_or_else(|e| e.into_inner()).volume = Some(v.clamp(0.0, 1.0));
        }
        #[cfg(target_os = "macos")]
        // SAFETY: queue is valid for the lifetime of self.
        unsafe {
            ffi::AudioQueueSetParameter(self.queue, ffi::K_PARAM_VOLUME, v.clamp(0.0, 1.0));
        }
    }
}

impl Drop for AudioOut {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        // SAFETY: stop and dispose synchronously so the callback can no longer run,
        // then release the callback's reference to the shared state.
        unsafe {
            ffi::AudioQueueStop(self.queue, 1);
            ffi::AudioQueueDispose(self.queue, 1);
            drop(Arc::from_raw(self.user as *const Mutex<Shared>));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_drains_then_pads() {
        let mut s = Shared::default();
        s.queue.extend([0.1, 0.2, 0.3, 0.4]);
        let mut out = [9.0f32; 6];
        s.fill(&mut out);
        assert_eq!(out, [0.1, 0.2, 0.3, 0.4, 0.0, 0.0]);
        assert_eq!(s.played_frames, 3);
        assert!(s.queue.is_empty());
    }

    #[test]
    fn open_push_and_drop() {
        // Runs where an output device exists; skips quietly otherwise.
        let Ok(out) = AudioOut::open() else { return };
        out.set_volume(0.0);
        out.push(&[0.0; 4800]);
        assert!(out.queued_frames() <= 2400);
        out.clear();
        assert_eq!(out.queued_frames(), 0);
    }
}
