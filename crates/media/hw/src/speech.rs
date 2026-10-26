// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Narration through the operating system's own voices (AppKit's speech
//! synthesizer on macOS), bound by hand through the Objective-C runtime:
//! the text is spoken straight into a file, read back here as samples.
//!
//! The voices are whatever the Mac has: the compact ones it ships with and
//! any Enhanced or Premium voices the user has added (System Settings,
//! Accessibility, Spoken Content), which sound far better and are listed
//! first.

#![allow(non_upper_case_globals, non_snake_case)]

#[cfg(target_os = "macos")]
use std::ffi::{CStr, CString, c_char, c_void};

/// One installed voice.
#[derive(Clone, Debug, PartialEq)]
pub struct Voice {
    /// The identifier the synthesizer takes.
    pub id: String,
    pub name: String,
    /// e.g. `en_US`.
    pub locale: String,
    /// `premium`, `enhanced` or `compact`: how natural it sounds.
    pub quality: String,
}

/// Spoken audio: mono samples at `rate` Hz.
pub struct Speech {
    pub samples: Vec<f32>,
    pub rate: u32,
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;

    type Id = *mut c_void;
    type Sel = *const c_void;

    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Sel;
        fn objc_msgSend();
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(pool: *mut c_void);
    }

    #[link(name = "AppKit", kind = "framework")]
    unsafe extern "C" {}

    #[link(name = "Foundation", kind = "framework")]
    unsafe extern "C" {}

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFRunLoopDefaultMode: *const c_void;
        fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after_source_handled: u8) -> i32;
    }

    fn class(name: &str) -> Id {
        let c = CString::new(name).expect("class name");
        unsafe { objc_getClass(c.as_ptr()) }
    }

    fn sel(name: &str) -> Sel {
        let c = CString::new(name).expect("selector");
        unsafe { sel_registerName(c.as_ptr()) }
    }

    // objc_msgSend called with the exact signature of each message.
    unsafe fn send0(obj: Id, s: &str) -> Id {
        let f: unsafe extern "C" fn(Id, Sel) -> Id = unsafe { std::mem::transmute(objc_msgSend as *const ()) };
        unsafe { f(obj, sel(s)) }
    }
    unsafe fn send1(obj: Id, s: &str, a: Id) -> Id {
        let f: unsafe extern "C" fn(Id, Sel, Id) -> Id = unsafe { std::mem::transmute(objc_msgSend as *const ()) };
        unsafe { f(obj, sel(s), a) }
    }
    unsafe fn send_bool0(obj: Id, s: &str) -> bool {
        let f: unsafe extern "C" fn(Id, Sel) -> i8 = unsafe { std::mem::transmute(objc_msgSend as *const ()) };
        unsafe { f(obj, sel(s)) != 0 }
    }
    unsafe fn send_bool2(obj: Id, s: &str, a: Id, b: Id) -> bool {
        let f: unsafe extern "C" fn(Id, Sel, Id, Id) -> i8 = unsafe { std::mem::transmute(objc_msgSend as *const ()) };
        unsafe { f(obj, sel(s), a, b) != 0 }
    }
    unsafe fn send_f32(obj: Id, s: &str, v: f32) {
        let f: unsafe extern "C" fn(Id, Sel, f32) = unsafe { std::mem::transmute(objc_msgSend as *const ()) };
        unsafe { f(obj, sel(s), v) }
    }
    unsafe fn send_usize(obj: Id, s: &str) -> usize {
        let f: unsafe extern "C" fn(Id, Sel) -> usize = unsafe { std::mem::transmute(objc_msgSend as *const ()) };
        unsafe { f(obj, sel(s)) }
    }
    unsafe fn send_index(obj: Id, s: &str, i: usize) -> Id {
        let f: unsafe extern "C" fn(Id, Sel, usize) -> Id = unsafe { std::mem::transmute(objc_msgSend as *const ()) };
        unsafe { f(obj, sel(s), i) }
    }

    fn nsstring(s: &str) -> Id {
        let c = CString::new(s.replace('\0', " ")).expect("no nul");
        let f: unsafe extern "C" fn(Id, Sel, *const c_char) -> Id = unsafe { std::mem::transmute(objc_msgSend as *const ()) };
        unsafe { f(class("NSString"), sel("stringWithUTF8String:"), c.as_ptr()) }
    }

    fn rust_string(ns: Id) -> String {
        if ns.is_null() {
            return String::new();
        }
        let f: unsafe extern "C" fn(Id, Sel) -> *const c_char = unsafe { std::mem::transmute(objc_msgSend as *const ()) };
        let p = unsafe { f(ns, sel("UTF8String")) };
        if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
    }

    struct Pool(*mut c_void);
    impl Pool {
        fn new() -> Pool {
            Pool(unsafe { objc_autoreleasePoolPush() })
        }
    }
    impl Drop for Pool {
        fn drop(&mut self) {
            unsafe { objc_autoreleasePoolPop(self.0) }
        }
    }

    pub fn voices() -> Vec<Voice> {
        let _pool = Pool::new();
        let mut out = Vec::new();
        unsafe {
            let cls = class("NSSpeechSynthesizer");
            if cls.is_null() {
                return out;
            }
            let arr = send0(cls, "availableVoices");
            if arr.is_null() {
                return out;
            }
            let n = send_usize(arr, "count");
            for i in 0..n {
                let vid = send_index(arr, "objectAtIndex:", i);
                let attrs = send1(cls, "attributesForVoice:", vid);
                let get = |k: &str| if attrs.is_null() { String::new() } else { rust_string(send1(attrs, "objectForKey:", nsstring(k))) };
                let id = rust_string(vid);
                let quality = if id.contains(".premium") { "premium" } else if id.contains(".enhanced") || id.contains("siri") { "enhanced" } else { "compact" };
                out.push(Voice { id: id.clone(), name: get("VoiceName"), locale: get("VoiceLocaleIdentifier"), quality: quality.into() });
            }
        }
        out
    }

    pub fn speak_to(text: &str, voice: Option<&str>, words_per_minute: Option<f32>, path: &std::path::Path) -> Result<(), String> {
        let _pool = Pool::new();
        unsafe {
            let cls = class("NSSpeechSynthesizer");
            if cls.is_null() {
                return Err("the system speech synthesizer is not available".into());
            }
            let alloc = send0(cls, "alloc");
            let v = voice.map(nsstring).unwrap_or(std::ptr::null_mut());
            let synth = send1(alloc, "initWithVoice:", v);
            if synth.is_null() {
                return Err(format!("no voice \"{}\"", voice.unwrap_or("default")));
            }
            if let Some(r) = words_per_minute {
                send_f32(synth, "setRate:", r);
            }
            let url = send1(class("NSURL"), "fileURLWithPath:", nsstring(&path.to_string_lossy()));
            let ok = send_bool2(synth, "startSpeakingString:toURL:", nsstring(text), url);
            if !ok {
                send0(synth, "release");
                return Err("the speech synthesizer refused the text".into());
            }
            // It speaks on its own thread; the run loop delivers the finish.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
            while send_bool0(synth, "isSpeaking") {
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.02, 1);
                if std::time::Instant::now() > deadline {
                    send0(synth, "stopSpeaking");
                    send0(synth, "release");
                    return Err("speaking took over 10 minutes".into());
                }
            }
            send0(synth, "release");
        }
        Ok(())
    }
}

/// The installed voices, best sounding first.
pub fn voices() -> Vec<Voice> {
    #[cfg(target_os = "macos")]
    {
        let mut v = mac::voices();
        let rank = |q: &str| match q {
            "premium" => 0,
            "enhanced" => 1,
            _ => 2,
        };
        v.sort_by(|a, b| rank(&a.quality).cmp(&rank(&b.quality)).then(a.name.cmp(&b.name)));
        v
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

/// Speaks `text` and returns the audio. `voice` is an identifier or a
/// name from [`voices`]; `words_per_minute` around 175 is natural.
pub fn speak(text: &str, voice: Option<&str>, words_per_minute: Option<f32>) -> Result<Speech, String> {
    #[cfg(target_os = "macos")]
    {
        let id = voice.map(|v| {
            let all = voices();
            all.iter().find(|x| x.id == v || x.name.eq_ignore_ascii_case(v)).map(|x| x.id.clone()).unwrap_or_else(|| v.to_owned())
        });
        let path = std::env::temp_dir().join(format!("studio-speech-{}-{}.aiff", std::process::id(), text.len() ^ (text.as_ptr() as usize)));
        mac::speak_to(text, id.as_deref(), words_per_minute, &path)?;
        let bytes = std::fs::read(&path).map_err(|e| format!("the spoken file: {e}"))?;
        let _ = std::fs::remove_file(&path);
        aiff(&bytes)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (text, voice, words_per_minute);
        Err("narration uses the system voices, which this platform build does not have yet".into())
    }
}

/// Reads an AIFF / AIFC file (16 or 32-bit PCM, either byte order) as mono.
pub fn aiff(b: &[u8]) -> Result<Speech, String> {
    if b.len() < 12 || &b[0..4] != b"FORM" || !(&b[8..12] == b"AIFF" || &b[8..12] == b"AIFC") {
        return Err("not an AIFF file".into());
    }
    let be16 = |p: usize| i16::from_be_bytes([b[p], b[p + 1]]);
    let be32 = |p: usize| u32::from_be_bytes([b[p], b[p + 1], b[p + 2], b[p + 3]]);
    let (mut channels, mut bits, mut rate, mut little) = (1usize, 16usize, 22050f64, false);
    let mut data: Option<&[u8]> = None;
    let mut p = 12;
    while p + 8 <= b.len() {
        let id = &b[p..p + 4];
        let size = be32(p + 4) as usize;
        let body = p + 8;
        let end = (body + size).min(b.len());
        match id {
            b"COMM" if size >= 18 => {
                channels = be16(body).max(1) as usize;
                bits = be16(body + 6) as usize;
                // 80-bit IEEE extended sample rate.
                let exp = (u16::from_be_bytes([b[body + 8], b[body + 9]]) & 0x7fff) as i32;
                let mant = u64::from_be_bytes(b[body + 10..body + 18].try_into().expect("8 bytes"));
                rate = mant as f64 * 2f64.powi(exp - 16383 - 63);
                if size >= 22 {
                    little = &b[body + 18..body + 22] == b"sowt";
                }
            }
            b"SSND" if size >= 8 => {
                let offset = be32(body) as usize;
                data = Some(&b[(body + 8 + offset).min(end)..end]);
            }
            _ => {}
        }
        p = body + size + (size & 1);
    }
    let data = data.ok_or("the AIFF has no sound")?;
    let step = bits / 8;
    if !(step == 2 || step == 4) {
        return Err(format!("{bits}-bit AIFF is not read"));
    }
    let frame = step * channels;
    let mut samples = Vec::with_capacity(data.len() / frame);
    for f in data.chunks_exact(frame) {
        let mut sum = 0f32;
        for c in 0..channels {
            let s = &f[c * step..(c + 1) * step];
            sum += if step == 2 {
                let v = if little { i16::from_le_bytes([s[0], s[1]]) } else { i16::from_be_bytes([s[0], s[1]]) };
                v as f32 / 32768.0
            } else {
                let v = if little { i32::from_le_bytes([s[0], s[1], s[2], s[3]]) } else { i32::from_be_bytes([s[0], s[1], s[2], s[3]]) };
                v as f32 / 2147483648.0
            };
        }
        samples.push(sum / channels as f32);
    }
    Ok(Speech { samples, rate: rate.round() as u32 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_tiny_aiff() {
        // 22050 Hz mono 16-bit, two samples.
        let mut f = Vec::new();
        f.extend_from_slice(b"FORM");
        f.extend_from_slice(&(4 + 26 + 8 + 12u32).to_be_bytes());
        f.extend_from_slice(b"AIFF");
        f.extend_from_slice(b"COMM");
        f.extend_from_slice(&18u32.to_be_bytes());
        f.extend_from_slice(&1i16.to_be_bytes());
        f.extend_from_slice(&2u32.to_be_bytes());
        f.extend_from_slice(&16i16.to_be_bytes());
        // 22050 as 80-bit extended: exponent 16383+14, mantissa 22050 << 49.
        f.extend_from_slice(&(16383u16 + 14).to_be_bytes());
        f.extend_from_slice(&(22050u64 << 49).to_be_bytes());
        f.extend_from_slice(b"SSND");
        f.extend_from_slice(&12u32.to_be_bytes());
        f.extend_from_slice(&0u32.to_be_bytes());
        f.extend_from_slice(&0u32.to_be_bytes());
        f.extend_from_slice(&16384i16.to_be_bytes());
        f.extend_from_slice(&(-16384i16).to_be_bytes());
        let s = aiff(&f).unwrap();
        assert_eq!(s.rate, 22050);
        assert_eq!(s.samples, vec![0.5, -0.5]);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn the_mac_speaks() {
        let v = voices();
        assert!(!v.is_empty(), "no system voices");
        let s = speak("Zornov can talk.", None, None).unwrap();
        assert!(s.samples.len() as f32 / s.rate as f32 > 0.4, "too short");
        assert!(s.samples.iter().any(|x| x.abs() > 0.05), "silent");
    }
}
