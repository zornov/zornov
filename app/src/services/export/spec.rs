// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! "Deliver for": what a delivery spec (YouTube, Shorts, Broadcast R128 ...) decides on the
//! Export screen, and why each control it takes over is locked. Pure: the screen applies it.

use studio_deliver::specs::{self, DeliverySpec, Kind, SizeRule};

/// The specs the Export screen offers: files from an export preset. Packages (DCP, IMF) are
/// written by the render queue, not by a single export.
pub fn menu() -> Vec<&'static DeliverySpec> {
    specs::SPECS.iter().filter(|s| s.kind == Kind::File).collect()
}

/// What choosing a spec sets, for a project of `size` at `fps`.
#[derive(Clone, Debug, PartialEq)]
pub struct Fill {
    pub id: &'static str,
    pub name: &'static str,
    /// The export preset that writes it.
    pub preset: &'static str,
    /// The output size; none keeps the project's own.
    pub size: Option<(u32, u32)>,
    /// The frame rate the file must have; none follows the project.
    pub fps: Option<f64>,
    /// "-14 LUFS, -1 dBTP" ...
    pub loudness: Option<String>,
    /// Captions are required, so a captions file is always written.
    pub captions: bool,
    pub max_secs: Option<f64>,
    /// Controls the spec takes over, each with the reason shown when it is disabled.
    pub locks: Vec<(Lock, String)>,
    pub notes: Vec<&'static str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lock {
    Format,
    Resolution,
    FrameRate,
    Captions,
}

pub fn fill(spec: &DeliverySpec, project_size: (u32, u32), project_fps: f64) -> Fill {
    let profile = spec.profile(project_fps, project_size);
    let size = match spec.size {
        SizeRule::Project => None,
        _ => profile.expect_size,
    };
    let mut locks = vec![(Lock::Format, format!("{} is written as {}", spec.name, spec.preset))];
    if size.is_some() {
        locks.push((Lock::Resolution, format!("{} sets the size", spec.name)));
    }
    if spec.fps.is_some() {
        locks.push((Lock::FrameRate, format!("{} sets the frame rate", spec.name)));
    }
    if spec.captions {
        locks.push((Lock::Captions, format!("{} needs a captions file", spec.name)));
    }
    Fill {
        id: spec.id,
        name: spec.name,
        preset: spec.preset,
        size,
        fps: spec.fps,
        loudness: spec.loudness.map(|t| format!("{:.0} LUFS, {:.0} dBTP", t.lufs, t.ceiling)),
        captions: spec.captions,
        max_secs: spec.max_secs,
        locks,
        notes: spec.notes.to_vec(),
    }
}

impl Fill {
    /// Why `control` is disabled by this spec, if it is.
    pub fn reason(&self, control: Lock) -> Option<&str> {
        self.locks.iter().find(|(l, _)| *l == control).map(|(_, r)| r.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_offers_files_and_a_spec_takes_over_what_it_decides() {
        let ids: Vec<&str> = menu().iter().map(|s| s.id).collect();
        for want in ["youtube", "youtube-shorts", "instagram-reel", "tiktok", "broadcast-r128", "web-preview"] {
            assert!(ids.contains(&want), "{want} is offered: {ids:?}");
        }
        assert!(!ids.iter().any(|i| i.starts_with("dcp")), "packages go through the queue");
        let shorts = fill(specs::find("youtube-shorts").unwrap(), (1920, 1080), 30.0);
        assert!(shorts.size.is_some(), "Shorts sets the size");
        assert!(shorts.reason(Lock::Format).unwrap().contains("YouTube Shorts"));
        let r128 = fill(specs::find("broadcast-r128").unwrap(), (1920, 1080), 25.0);
        assert_eq!(r128.loudness.as_deref(), Some("-23 LUFS, -1 dBTP"));
        assert!(r128.reason(Lock::Resolution).is_some() || r128.size.is_none());
    }
}
