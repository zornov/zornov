// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Microphone input through AudioQueue (mono f32 at [`crate::RATE`]), and the
//! list of CoreAudio input devices for the voiceover device picker.
//!
//! The queue meters all the time (for the input level bar); samples are kept
//! only while recording. Input volume (0..2, shown as 0-200 %) is applied to
//! both. macOS asks for microphone permission on first use; until it is given
//! the queue delivers silence.

use std::sync::{Arc, Mutex};

/// A CoreAudio device with at least one input stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputDevice {
    /// Persistent device UID (what the queue is told to use).
    pub uid: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Default)]
struct InShared {
    recording: bool,
    gain: f32,
    samples: Vec<f32>,
    peak: f32,
    /// Frames delivered by the device since opening.
    frames: u64,
}

impl InShared {
    fn feed(&mut self, input: &[f32]) {
        let g = self.gain;
        for &x in input {
            let v = (x * g).clamp(-1.0, 1.0);
            self.peak = self.peak.max(v.abs());
            if self.recording {
                self.samples.push(v);
            }
        }
        self.frames += input.len() as u64;
    }
}

/// An open microphone stream.
pub struct AudioIn {
    shared: Arc<Mutex<InShared>>,
    #[cfg(target_os = "macos")]
    queue: crate::ffi::AudioQueueRef,
    #[cfg(target_os = "macos")]
    user: *mut Mutex<InShared>,
    #[cfg(not(target_os = "macos"))]
    _stream: cpal::Stream,
}

// The queue handle is used only by the owner's methods and Drop; the callback
// touches only the shared mutex.
unsafe impl Send for AudioIn {}

const IN_BUFFERS: usize = 4;
const IN_FRAMES: u32 = 1024;

#[cfg(target_os = "macos")]
extern "C" fn input_callback(user: *mut std::ffi::c_void, q: crate::ffi::AudioQueueRef, buf: crate::ffi::AudioQueueBufferRef, _ts: *const std::ffi::c_void, _n: u32, _d: *const std::ffi::c_void) {
    // SAFETY: `user` is the Mutex kept alive by AudioIn until the queue is
    // disposed; `buf` belongs to this queue and holds `audio_data_byte_size`
    // bytes of f32 mono samples.
    unsafe {
        let shared = &*(user as *const Mutex<InShared>);
        let b = &*buf;
        let n = (b.audio_data_byte_size / 4) as usize;
        if n > 0 && !b.audio_data.is_null() {
            let data = std::slice::from_raw_parts(b.audio_data as *const f32, n);
            if let Ok(mut s) = shared.lock() {
                s.feed(data);
            }
        }
        crate::ffi::AudioQueueEnqueueBuffer(q, buf, 0, std::ptr::null());
    }
}

impl AudioIn {
    /// Opens the device with `uid` (None = system default input).
    #[cfg(target_os = "macos")]
    pub fn open(uid: Option<&str>) -> Result<Self, String> {
        use crate::ffi;
        use std::ffi::c_void;
        let shared = Arc::new(Mutex::new(InShared { gain: 1.0, ..Default::default() }));
        let user = Arc::into_raw(shared.clone()) as *mut Mutex<InShared>;
        let fmt = ffi::AudioStreamBasicDescription {
            sample_rate: crate::RATE as f64,
            format_id: ffi::K_AUDIO_FORMAT_LINEAR_PCM,
            format_flags: ffi::K_FLAG_FLOAT | ffi::K_FLAG_PACKED,
            bytes_per_packet: 4,
            frames_per_packet: 1,
            bytes_per_frame: 4,
            channels_per_frame: 1,
            bits_per_channel: 32,
            reserved: 0,
        };
        let mut queue: ffi::AudioQueueRef = std::ptr::null_mut();
        // SAFETY: valid format, callback of the expected signature, null run
        // loop (the queue's own thread) and an out-pointer to a local.
        let status = unsafe { ffi::AudioQueueNewInput(&fmt, input_callback, user as *mut c_void, std::ptr::null_mut(), std::ptr::null(), 0, &mut queue) };
        if status != 0 || queue.is_null() {
            // SAFETY: no queue exists; reclaim the callback's reference.
            unsafe { drop(Arc::from_raw(user as *const Mutex<InShared>)) };
            return Err(format!("Microphone not detected (AudioQueueNewInput error {status})."));
        }
        let me = Self { shared, queue, user };
        if let Some(uid) = uid.filter(|u| !u.is_empty()) {
            let c = std::ffi::CString::new(uid).map_err(|_| "Bad device id.".to_string())?;
            // SAFETY: a CFString created from a valid C string, released after use;
            // the property takes a pointer to the CFStringRef.
            unsafe {
                let s = ffi::CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), ffi::K_UTF8);
                if !s.is_null() {
                    let st = ffi::AudioQueueSetProperty(me.queue, ffi::K_AUDIO_QUEUE_PROPERTY_CURRENT_DEVICE, &s as *const _ as *const c_void, std::mem::size_of::<ffi::CFStringRef>() as u32);
                    ffi::CFRelease(s);
                    if st != 0 {
                        return Err(format!("Couldn't connect to the microphone (error {st})."));
                    }
                }
            }
        }
        for _ in 0..IN_BUFFERS {
            let mut b: ffi::AudioQueueBufferRef = std::ptr::null_mut();
            // SAFETY: queue is valid; buffers are owned by the queue.
            unsafe {
                if ffi::AudioQueueAllocateBuffer(me.queue, IN_FRAMES * 4, &mut b) != 0 || b.is_null() {
                    return Err("Microphone buffers could not be allocated.".into());
                }
                ffi::AudioQueueEnqueueBuffer(me.queue, b, 0, std::ptr::null());
            }
        }
        // SAFETY: queue is valid and has buffers enqueued.
        let status = unsafe { ffi::AudioQueueStart(me.queue, std::ptr::null()) };
        if status != 0 {
            return Err(format!("Microphone is occupied by another application or unavailable (error {status})."));
        }
        Ok(me)
    }

    #[cfg(not(target_os = "macos"))]
    pub fn open(uid: Option<&str>) -> Result<Self, String> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let host = cpal::default_host();
        let device = uid
            .and_then(|name| host.input_devices().ok()?.find(|d| d.name().ok().as_deref() == Some(name)))
            .or_else(|| host.default_input_device())
            .ok_or("No microphone was found.")?;
        let def = device.default_input_config().map_err(|e| format!("Microphone is unavailable: {e}"))?;
        let channels = def.channels().max(1) as usize;
        let rate = def.sample_rate().0.max(1);
        let config = cpal::StreamConfig { channels: channels as u16, sample_rate: cpal::SampleRate(rate), buffer_size: cpal::BufferSize::Default };
        let shared = Arc::new(Mutex::new(InShared { gain: 1.0, ..Default::default() }));
        let cb_shared = shared.clone();
        // Mono mix of the device's channels, resampled (linear) to RATE.
        let step = rate as f64 / crate::RATE as f64;
        let mut pos = 0.0f64;
        let mut last = 0.0f32;
        let mut mono: Vec<f32> = Vec::new();
        let mut out: Vec<f32> = Vec::new();
        let stream = device
            .build_input_stream(
                &config,
                move |data: &[f32], _| {
                    mono.clear();
                    mono.extend(data.chunks(channels).map(|f| f.iter().sum::<f32>() / f.len() as f32));
                    out.clear();
                    while (pos as usize) < mono.len() {
                        let i = pos as usize;
                        let a = if i == 0 { last } else { mono[i - 1] };
                        let k = (pos - i as f64) as f32;
                        out.push(a + (mono[i] - a) * k);
                        pos += step;
                    }
                    pos -= mono.len() as f64;
                    if let Some(&l) = mono.last() {
                        last = l;
                    }
                    cb_shared.lock().unwrap_or_else(|e| e.into_inner()).feed(&out);
                },
                |e| eprintln!("microphone: {e}"),
                None,
            )
            .map_err(|e| format!("Microphone is occupied by another application or unavailable: {e}"))?;
        stream.play().map_err(|e| format!("Microphone could not start: {e}"))?;
        Ok(Self { shared, _stream: stream })
    }

    /// Starts (true) or pauses keeping samples; metering continues either way.
    pub fn set_recording(&self, on: bool) {
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).recording = on;
    }

    /// Input volume, 0..2 (1 = unchanged).
    pub fn set_gain(&self, g: f32) {
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).gain = g.clamp(0.0, 2.0);
    }

    /// Recorded samples so far (mono, [`crate::RATE`]), leaving the buffer empty.
    pub fn take_samples(&self) -> Vec<f32> {
        std::mem::take(&mut self.shared.lock().unwrap_or_else(|e| e.into_inner()).samples)
    }

    /// Number of recorded samples waiting.
    pub fn recorded_len(&self) -> usize {
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).samples.len()
    }

    /// Peak level (0..1) since the last call.
    pub fn take_peak(&self) -> f32 {
        std::mem::take(&mut self.shared.lock().unwrap_or_else(|e| e.into_inner()).peak)
    }

    /// Whether the device has delivered any audio yet.
    pub fn is_live(&self) -> bool {
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).frames > 0
    }
}

impl Drop for AudioIn {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        // SAFETY: stop and dispose synchronously so the callback cannot run
        // again, then release its reference to the shared state.
        unsafe {
            crate::ffi::AudioQueueStop(self.queue, 1);
            crate::ffi::AudioQueueDispose(self.queue, 1);
            drop(Arc::from_raw(self.user as *const Mutex<InShared>));
        }
    }
}

#[cfg(target_os = "macos")]
fn cf_to_string(s: crate::ffi::CFStringRef) -> String {
    let mut buf = vec![0 as std::ffi::c_char; 512];
    // SAFETY: `s` is a valid CFString; the buffer length is passed.
    let ok = unsafe { crate::ffi::CFStringGetCString(s, buf.as_mut_ptr(), buf.len() as isize, crate::ffi::K_UTF8) };
    if ok == 0 {
        return String::new();
    }
    // SAFETY: CFStringGetCString wrote a NUL-terminated string into `buf`.
    unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned()
}

#[cfg(target_os = "macos")]
fn string_property(id: u32, selector: u32) -> Option<String> {
    use crate::ffi;
    let addr = ffi::PropertyAddress { selector, scope: ffi::K_SCOPE_GLOBAL, element: 0 };
    let mut s: ffi::CFStringRef = std::ptr::null();
    let mut size = std::mem::size_of::<ffi::CFStringRef>() as u32;
    // SAFETY: the property is a CFStringRef that we own and release.
    unsafe {
        if ffi::AudioObjectGetPropertyData(id, &addr, 0, std::ptr::null(), &mut size, &mut s as *mut _ as *mut std::ffi::c_void) != 0 || s.is_null() {
            return None;
        }
        let out = cf_to_string(s);
        ffi::CFRelease(s);
        Some(out)
    }
}

/// Input devices, the system default first.
#[cfg(target_os = "macos")]
pub fn input_devices() -> Vec<InputDevice> {
    use crate::ffi;
    let sys = ffi::K_SYSTEM_OBJECT;
    let addr = ffi::PropertyAddress { selector: ffi::K_HW_DEVICES, scope: ffi::K_SCOPE_GLOBAL, element: 0 };
    let mut size = 0u32;
    // SAFETY: plain property queries on the system object with correctly sized buffers.
    let ids: Vec<u32> = unsafe {
        if ffi::AudioObjectGetPropertyDataSize(sys, &addr, 0, std::ptr::null(), &mut size) != 0 {
            return Vec::new();
        }
        let mut ids = vec![0u32; size as usize / 4];
        if ffi::AudioObjectGetPropertyData(sys, &addr, 0, std::ptr::null(), &mut size, ids.as_mut_ptr() as *mut std::ffi::c_void) != 0 {
            return Vec::new();
        }
        ids.truncate(size as usize / 4);
        ids
    };
    let default = {
        let addr = ffi::PropertyAddress { selector: ffi::K_HW_DEFAULT_INPUT, scope: ffi::K_SCOPE_GLOBAL, element: 0 };
        let mut id = 0u32;
        let mut size = 4u32;
        // SAFETY: the property is one AudioObjectID.
        let st = unsafe { ffi::AudioObjectGetPropertyData(sys, &addr, 0, std::ptr::null(), &mut size, &mut id as *mut u32 as *mut std::ffi::c_void) };
        if st == 0 { id } else { 0 }
    };
    let mut out: Vec<InputDevice> = ids
        .into_iter()
        .filter(|&id| {
            let addr = ffi::PropertyAddress { selector: ffi::K_DEVICE_STREAMS, scope: ffi::K_SCOPE_INPUT, element: 0 };
            let mut size = 0u32;
            // SAFETY: size query only.
            let st = unsafe { ffi::AudioObjectGetPropertyDataSize(id, &addr, 0, std::ptr::null(), &mut size) };
            st == 0 && size > 0
        })
        .filter_map(|id| {
            let uid = string_property(id, ffi::K_DEVICE_UID)?;
            let name = string_property(id, ffi::K_OBJECT_NAME).filter(|n| !n.is_empty()).unwrap_or_else(|| uid.clone());
            Some(InputDevice { uid, name, is_default: id == default })
        })
        .collect();
    out.sort_by_key(|d| !d.is_default);
    out
}

#[cfg(not(target_os = "macos"))]
pub fn input_devices() -> Vec<InputDevice> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let default = host.default_input_device().and_then(|d| d.name().ok());
    let mut out: Vec<InputDevice> = host
        .input_devices()
        .map(|it| it.filter_map(|d| d.name().ok()).map(|name| InputDevice { uid: name.clone(), is_default: Some(&name) == default.as_ref(), name }).collect())
        .unwrap_or_default();
    out.sort_by_key(|d| !d.is_default);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feed_meters_always_and_keeps_only_while_recording() {
        let mut s = InShared { gain: 2.0, ..Default::default() };
        s.feed(&[0.1, -0.3]);
        assert!(s.samples.is_empty());
        assert!((s.peak - 0.6).abs() < 1e-6);
        s.recording = true;
        s.feed(&[0.7, 0.2]);
        // Gain applied and clipped to full scale.
        assert_eq!(s.samples, vec![1.0, 0.4]);
        assert_eq!(s.frames, 4);
    }

    #[test]
    fn devices_are_listed_default_first() {
        // Machine dependent: only checks consistency.
        let d = input_devices();
        assert!(d.iter().filter(|x| x.is_default).count() <= 1);
        if let Some(i) = d.iter().position(|x| x.is_default) {
            assert_eq!(i, 0);
        }
        assert!(d.iter().all(|x| !x.uid.is_empty()));
    }
}
