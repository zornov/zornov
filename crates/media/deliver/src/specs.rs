// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Delivery specifications: what each platform and mastering format asks
//! of a file, as data. Each one names the Studio export preset that makes
//! the file, the shape and length it must have, the loudness it is judged
//! against, and builds the [`qc::Profile`] its QC runs with.
//!
//! The numbers are the published specifications as Studio knew them when
//! this was written (2026); platforms change them, so each spec says what
//! it is based on in `notes` and the QC report prints the profile it used.

use crate::qc::Profile;

/// A loudness specification: integrated programme loudness with its
/// tolerance, and the true-peak ceiling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    pub lufs: f64,
    pub tolerance: f64,
    pub ceiling: f64,
    /// Measured on dialogue only.
    pub dialogue_gated: bool,
}

impl Target {
    /// YouTube, Instagram, TikTok and most streaming platforms: -14 LUFS, -1 dBTP.
    pub const STREAMING: Target = Target { lufs: -14.0, tolerance: 1.0, ceiling: -1.0, dialogue_gated: false };
    /// EBU R 128 broadcast: -23 LUFS +-0.5 LU, -1 dBTP.
    pub const EBU_R128: Target = Target { lufs: -23.0, tolerance: 0.5, ceiling: -1.0, dialogue_gated: false };
    /// US broadcast ATSC A/85: -24 LKFS +-2 LU, -2 dBTP.
    pub const ATSC_A85: Target = Target { lufs: -24.0, tolerance: 2.0, ceiling: -2.0, dialogue_gated: false };
    /// Netflix: -27 LKFS +-2 LU on dialogue, -2 dBTP.
    pub const NETFLIX: Target = Target { lufs: -27.0, tolerance: 2.0, ceiling: -2.0, dialogue_gated: true };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// One file from an export preset.
    File,
    /// A Digital Cinema Package folder (SMPTE or Interop).
    Dcp,
    /// An Interoperable Master Format package (Application 2E).
    Imf,
}

/// How the output size follows from the project.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SizeRule {
    Fixed(u32, u32),
    /// Scale so the short side is this many pixels, keeping the shape.
    ShortSide(u32),
    Project,
}

#[derive(Clone, Copy, Debug)]
pub struct DeliverySpec {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: Kind,
    /// The export preset (`services::export::PRESETS`) that writes it.
    pub preset: &'static str,
    pub size: SizeRule,
    /// A frame rate the file must have (None: the project's own).
    pub fps: Option<f64>,
    pub max_secs: Option<f64>,
    pub max_megabytes: Option<u64>,
    pub loudness: Option<Target>,
    pub expect_audio: bool,
    pub captions: bool,
    /// Out-of-range levels and flashes fail instead of warn.
    pub strict: bool,
    pub notes: &'static [&'static str],
}

pub const SPECS: &[DeliverySpec] = &[
    DeliverySpec {
        id: "youtube",
        name: "YouTube",
        kind: Kind::File,
        preset: "Social 1080p",
        size: SizeRule::ShortSide(1080),
        fps: None,
        max_secs: None,
        max_megabytes: None,
        loudness: Some(Target::STREAMING),
        expect_audio: true,
        captions: false,
        strict: false,
        notes: &["H.264 in MP4 with AAC-LC stereo. Plays any shape; 16:9 for TV screens.", "Loudness: YouTube turns louder mixes down toward -14 LUFS, so aim at -14 with a -1 dBTP ceiling.", "4K: use the 4K master preset."],
    },
    DeliverySpec {
        id: "youtube-4k",
        name: "YouTube 4K",
        kind: Kind::File,
        preset: "4K master",
        size: SizeRule::ShortSide(2160),
        fps: None,
        max_secs: None,
        max_megabytes: None,
        loudness: Some(Target::STREAMING),
        expect_audio: true,
        captions: false,
        strict: false,
        notes: &["H.264 in MP4 at 2160p; YouTube gives 4K uploads its better encode.", "Loudness -14 LUFS, -1 dBTP."],
    },
    DeliverySpec {
        id: "youtube-shorts",
        name: "YouTube Shorts",
        kind: Kind::File,
        preset: "Vertical",
        size: SizeRule::Fixed(1080, 1920),
        fps: None,
        max_secs: Some(180.0),
        max_megabytes: None,
        loudness: Some(Target::STREAMING),
        expect_audio: true,
        captions: false,
        strict: false,
        notes: &["9:16, up to 3 minutes.", "Keep titles clear of the bottom 20 % and the right 12 % (buttons and caption)."],
    },
    DeliverySpec {
        id: "instagram-reel",
        name: "Instagram Reel",
        kind: Kind::File,
        preset: "Vertical",
        size: SizeRule::Fixed(1080, 1920),
        fps: None,
        max_secs: Some(180.0),
        max_megabytes: Some(4096),
        loudness: Some(Target::STREAMING),
        expect_audio: true,
        captions: false,
        strict: false,
        notes: &["9:16 1080 x 1920, H.264 / AAC, up to 3 minutes (90 s for the best reach).", "Safe area: top 14 % and bottom 20 % are covered by the interface."],
    },
    DeliverySpec {
        id: "instagram-feed",
        name: "Instagram Feed",
        kind: Kind::File,
        preset: "Social 1080p",
        size: SizeRule::Fixed(1080, 1350),
        fps: None,
        max_secs: Some(180.0),
        max_megabytes: Some(4096),
        loudness: Some(Target::STREAMING),
        expect_audio: true,
        captions: false,
        strict: false,
        notes: &["4:5 portrait 1080 x 1350 takes the most feed space; 1:1 and 16:9 also play."],
    },
    DeliverySpec {
        id: "tiktok",
        name: "TikTok",
        kind: Kind::File,
        preset: "Vertical",
        size: SizeRule::Fixed(1080, 1920),
        fps: None,
        max_secs: Some(600.0),
        max_megabytes: Some(287),
        loudness: Some(Target::STREAMING),
        expect_audio: true,
        captions: false,
        strict: false,
        notes: &["9:16 1080 x 1920, H.264 / AAC, up to 10 minutes; short videos (under 60 s) travel best.", "Keep the bottom 25 % and the right 15 % free of titles."],
    },
    DeliverySpec {
        id: "netflix",
        name: "Netflix (marketing and promo file)",
        kind: Kind::File,
        preset: "4K master",
        size: SizeRule::Project,
        fps: None,
        max_secs: None,
        max_megabytes: None,
        loudness: Some(Target::NETFLIX),
        expect_audio: true,
        captions: true,
        strict: true,
        notes: &["Dialogue-gated loudness -27 LKFS +-2 LU, true peak -2 dBTP; photosensitivity and legal levels are failures.", "The full programme master for Netflix is an IMF (App 2E): use the imf-netflix spec."],
    },
    DeliverySpec {
        id: "broadcast-r128",
        name: "Broadcast master (EBU R 128)",
        kind: Kind::File,
        preset: "4K master",
        size: SizeRule::Fixed(1920, 1080),
        fps: Some(25.0),
        max_secs: None,
        max_megabytes: None,
        loudness: Some(Target::EBU_R128),
        expect_audio: true,
        captions: true,
        strict: true,
        notes: &["1920 x 1080 at 25 fps, loudness -23 LUFS +-0.5 LU, true peak -1 dBTP, legal levels (R 103), photosensitivity (Ofcom, ITU-R BT.1702).", "Station-specific wrappers (XDCAM, ProRes, MXF OP1a) come from the pro-post codecs when available; this is the H.264 mezzanine."],
    },
    DeliverySpec {
        id: "broadcast-atsc",
        name: "Broadcast master (US, ATSC A/85)",
        kind: Kind::File,
        preset: "4K master",
        size: SizeRule::Fixed(1920, 1080),
        fps: Some(29.97),
        max_secs: None,
        max_megabytes: None,
        loudness: Some(Target::ATSC_A85),
        expect_audio: true,
        captions: true,
        strict: true,
        notes: &["1920 x 1080 at 29.97 fps, loudness -24 LKFS +-2 LU, true peak -2 dBTP; closed captions are a legal requirement for US broadcast."],
    },
    DeliverySpec {
        id: "dcp-smpte",
        name: "Cinema DCP (SMPTE, 2K flat)",
        kind: Kind::Dcp,
        preset: "PNG sequence",
        size: SizeRule::Fixed(1998, 1080),
        fps: Some(24.0),
        max_secs: None,
        max_megabytes: None,
        loudness: None,
        expect_audio: true,
        captions: false,
        strict: true,
        notes: &["JPEG 2000 (Studio's own encoder; X'Y'Z' with the DCI 2.6 gamma, sRGB or Rec.709 pictures converted) in MXF, PCM 24-bit 48 kHz, CPL/PKL/ASSETMAP, SMPTE 429.", "Containers: 2K flat 1998 x 1080, 2K scope 2048 x 858, 4K flat 3996 x 2160, 4K scope 4096 x 1716; 24 fps (also 25, 30, 48 ...).", "Cinema loudness is set by the room (7 on the Dolby scale, about -27 LKFS programme): leave the mix at its mastering level."],
    },
    DeliverySpec {
        id: "dcp-interop",
        name: "Cinema DCP (Interop, 2K flat)",
        kind: Kind::Dcp,
        preset: "PNG sequence",
        size: SizeRule::Fixed(1998, 1080),
        fps: Some(24.0),
        max_secs: None,
        max_megabytes: None,
        loudness: None,
        expect_audio: true,
        captions: false,
        strict: true,
        notes: &["The older Interop flavour (Digital Cinema Initiatives namespaces) that every projector reads."],
    },
    DeliverySpec {
        id: "imf-netflix",
        name: "IMF master (App 2E, Netflix)",
        kind: Kind::Imf,
        preset: "PNG sequence",
        size: SizeRule::Project,
        fps: None,
        max_secs: None,
        max_megabytes: None,
        loudness: Some(Target::NETFLIX),
        expect_audio: true,
        captions: true,
        strict: true,
        notes: &["IMF Application 2E (SMPTE ST 2067-21): JPEG 2000 in MXF, PCM audio, CPL / PKL / ASSETMAP / VOLINDEX.", "The package is written and checked structurally; Netflix's own Photon validator is the final gate."],
    },
    DeliverySpec {
        id: "web-preview",
        name: "Review copy",
        kind: Kind::File,
        preset: "Social 1080p",
        size: SizeRule::ShortSide(720),
        fps: None,
        max_secs: None,
        max_megabytes: None,
        loudness: None,
        expect_audio: false,
        captions: false,
        strict: false,
        notes: &["A light 720p file to send for approval."],
    },
];

impl DeliverySpec {
    /// The QC profile a file made for this spec is judged by.
    pub fn profile(&self, project_fps: f64, project_size: (u32, u32)) -> Profile {
        let size = match self.size {
            SizeRule::Fixed(w, h) => Some((w, h)),
            SizeRule::ShortSide(s) => {
                let k = s as f64 / project_size.0.min(project_size.1).max(2) as f64;
                let even = |v: f64| ((v / 2.0).round() as u32 * 2).max(2);
                Some((even(project_size.0 as f64 * k), even(project_size.1 as f64 * k)))
            }
            SizeRule::Project => None,
        };
        Profile {
            name: self.name.to_string(),
            strict_levels: self.strict,
            strict_flash: true,
            loudness: self.loudness.map(|t| (t.lufs, t.tolerance, t.ceiling)),
            dialogue_gated: self.loudness.is_some_and(|t| t.dialogue_gated),
            expect_audio: self.expect_audio,
            expect_size: size,
            expect_fps: self.fps.or(Some(project_fps)),
            max_duration: self.max_secs,
            captions_required: self.captions,
            ..Profile::default()
        }
    }

    /// Output frame rate for a project at `project_fps`.
    pub fn fps_for(&self, project_fps: f64) -> f64 {
        self.fps.unwrap_or(project_fps)
    }
}

/// A spec by id or by (part of) its name, ignoring case and punctuation.
pub fn find(key: &str) -> Option<&'static DeliverySpec> {
    let norm = |s: &str| s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect::<String>();
    let k = norm(key);
    if k.is_empty() {
        return None;
    }
    SPECS.iter().find(|s| norm(s.id) == k).or_else(|| SPECS.iter().find(|s| norm(s.name) == k)).or_else(|| SPECS.iter().find(|s| norm(s.name).contains(&k) || norm(s.id).contains(&k)))
}

/// One line per spec, for the AI and the dialog.
pub fn listing() -> Vec<(&'static str, String)> {
    SPECS.iter().map(|s| (s.id, format!("{} [{}]: {}", s.name, match s.kind { Kind::File => "file", Kind::Dcp => "DCP", Kind::Imf => "IMF" }, s.notes.first().copied().unwrap_or("")))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_are_found_by_id_name_or_part() {
        assert_eq!(find("youtube").unwrap().id, "youtube");
        assert_eq!(find("Instagram Reel").unwrap().id, "instagram-reel");
        assert_eq!(find("tik tok").unwrap().id, "tiktok");
        assert_eq!(find("r128").map(|s| s.id), Some("broadcast-r128"));
        assert!(find("").is_none() && find("nonsense-xyz").is_none());
    }

    #[test]
    fn ids_are_unique_and_profiles_follow_the_spec() {
        let mut ids: Vec<_> = SPECS.iter().map(|s| s.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), SPECS.len());
        let p = find("netflix").unwrap().profile(24.0, (3840, 2160));
        assert!(p.dialogue_gated && p.strict_levels && p.captions_required);
        assert_eq!(p.loudness, Some((-27.0, 2.0, -2.0)));
        let p = find("youtube").unwrap().profile(30.0, (1080, 1920));
        assert_eq!(p.expect_size, Some((1080, 1920)));
        assert_eq!(p.expect_fps, Some(30.0));
        let p = find("broadcast-r128").unwrap().profile(30.0, (1280, 720));
        assert_eq!((p.expect_size, p.expect_fps), (Some((1920, 1080)), Some(25.0)));
    }
}
