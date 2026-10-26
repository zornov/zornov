// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Real capture devices: what is connected, whether the system lets
//! Studio use it, asking for permission (only when the person has acted),
//! and opening it as a source.
//!
//! Nothing here asks the system for permission, switches on a camera light
//! or starts recording a screen by itself. Listing devices never prompts
//! (window titles stay hidden until screen recording is allowed). Opening
//! a device needs access already granted; asking for access is its own
//! call ([`request`]), which shows the system's dialog and so must come
//! from something the person did.

use crate::cursor::CursorSource;
use crate::frame::Frame;
use crate::source::{AudioSource, Kind, VideoSource};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Device {
    /// The id to give a recorder or mixer (`camera:…`, `mic:…`, `screen:…`, `window:…`).
    pub id: String,
    pub name: String,
    pub detail: String,
    pub default: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct Devices {
    pub cameras: Vec<Device>,
    pub mics: Vec<Device>,
    pub screens: Vec<Device>,
    pub windows: Vec<Device>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// Never asked: opening the device will not work until the person is asked.
    NotDetermined,
    Denied,
    /// Blocked by the system or a profile; the person cannot change it here.
    Restricted,
    Authorized,
    /// This computer has no such capture.
    Unavailable,
}

impl Access {
    pub fn ok(self) -> bool {
        self == Access::Authorized
    }
}

/// The text to give a person whose access is not (yet) there.
pub fn explain(kind: Kind, a: Access) -> String {
    let (what, pane) = match kind {
        Kind::Camera => ("the camera", "Camera"),
        Kind::Mic => ("the microphone", "Microphone"),
        _ => ("screen recording", "Screen & System Audio Recording"),
    };
    match a {
        Access::Authorized => String::new(),
        Access::NotDetermined => format!("Studio has not been given {what} yet. Ask the person to allow it (capture permissions with request: true, which shows the system's own question)"),
        Access::Denied => format!("Access to {what} was turned off. The person can turn it on for Studio in System Settings, Privacy & Security, {pane}"),
        Access::Restricted => format!("Access to {what} is blocked on this computer by a system setting or profile"),
        Access::Unavailable => format!("This computer cannot provide {what}"),
    }
}

pub fn list() -> Devices {
    let mut d = Devices::default();
    let default_in = studio_audio::input_devices();
    d.mics = default_in.into_iter().map(|m| Device { id: format!("mic:{}", if m.is_default { "default".into() } else { m.uid.clone() }), name: m.name, detail: m.uid, default: m.is_default }).collect();
    #[cfg(target_os = "macos")]
    {
        d.cameras = mac::cameras();
        d.screens = mac::screens();
        d.windows = mac::windows();
    }
    d
}

pub fn access(kind: Kind) -> Access {
    match kind {
        #[cfg(target_os = "macos")]
        Kind::Camera | Kind::Mic | Kind::Screen | Kind::Window => mac::access(kind),
        Kind::Test => Access::Authorized,
        #[cfg(not(target_os = "macos"))]
        _ => Access::Unavailable,
    }
}

/// Asks the system for access (its dialog appears, and this waits for the
/// answer). Call it only from something the person did.
pub fn request(kind: Kind) -> Result<Access, String> {
    match kind {
        Kind::Test => Ok(Access::Authorized),
        #[cfg(target_os = "macos")]
        _ => mac::request(kind),
        #[cfg(not(target_os = "macos"))]
        _ => Err("this computer cannot provide capture devices".into()),
    }
}

fn need(kind: Kind) -> Result<(), String> {
    let a = access(kind);
    if a.ok() { Ok(()) } else { Err(explain(kind, a)) }
}

/// The latest picture a device delivered.
#[derive(Default)]
pub struct Slot {
    pub frame: Mutex<Option<Frame>>,
}

pub struct SlotVideo {
    pub label: String,
    pub slot: Arc<Slot>,
    pub size: (u32, u32),
    /// Keeps the capture running; stops it when dropped.
    pub _guard: Box<dyn std::any::Any + Send>,
    pub start: std::time::Instant,
}

impl VideoSource for SlotVideo {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn frame_at(&mut self, pts_us: i64) -> Option<Frame> {
        let mut f = self.slot.frame.lock().ok()?.clone()?;
        f.pts_us = pts_us;
        Some(f)
    }
}

pub fn open_video(kind: Kind, id: &str, w: u32, h: u32, fps: f64) -> Result<Box<dyn VideoSource>, String> {
    need(kind)?;
    #[cfg(target_os = "macos")]
    {
        mac::open_video(kind, id, w, h, fps)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (id, w, h, fps);
        Err(format!("{kind:?} capture is not available on this computer yet"))
    }
}

/// A microphone through Studio's audio input.
struct MicSource {
    label: String,
    input: studio_audio::AudioIn,
    queue: std::collections::VecDeque<f32>,
    rate: u32,
}

impl AudioSource for MicSource {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn rate(&self) -> u32 {
        self.rate
    }

    fn read(&mut self, n: usize) -> Vec<f32> {
        self.queue.extend(self.input.take_samples());
        // Keep at most a second in hand: a consumer that stalls does not
        // build up delay.
        while self.queue.len() > self.rate as usize {
            self.queue.pop_front();
        }
        (0..n).map(|_| self.queue.pop_front().unwrap_or(0.0)).collect()
    }
}

pub fn open_audio(id: &str, rate: u32) -> Result<Box<dyn AudioSource>, String> {
    need(Kind::Mic)?;
    if rate != studio_audio::RATE {
        return Err(format!("microphones run at {} Hz here", studio_audio::RATE));
    }
    let uid = if id == "default" || id.is_empty() { None } else { Some(id) };
    let input = studio_audio::AudioIn::open(uid)?;
    input.set_recording(true);
    Ok(Box::new(MicSource { label: format!("mic {id}"), input, queue: Default::default(), rate }))
}

/// The pointer on a screen or window, for the highlight in a recording.
pub fn system_cursor(source_id: &str) -> Option<Box<dyn CursorSource>> {
    #[cfg(target_os = "macos")]
    {
        mac::cursor(source_id)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = source_id;
        None
    }
}

#[cfg(target_os = "macos")]
mod mac;
