// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Export output formats (shared by the dialog and the exporter).

/// Output formats, in the order the dialog lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    H264,
    Clv,
    Png,
    Gif,
    /// 10-bit HEVC on the video hardware: finer steps in gradients.
    Hevc10,
    /// Sound only: AAC in M4A.
    M4a,
    /// Apple ProRes 422 HQ in QuickTime, 24-bit PCM sound, timecode.
    ProRes422Hq,
    /// Apple ProRes 4444 (12-bit 4:4:4) in QuickTime, sound, timecode.
    ProRes4444,
    /// Apple ProRes 422 Proxy at the export size (offline / review).
    ProResProxy,
    /// OpenEXR sequence, ACES2065-1 (AP0 linear, half float): the ACES
    /// interchange deliverable for VFX and colour.
    Exr,
    /// 10-bit DPX sequence in the project's display encoding.
    Dpx,
    /// 16-bit TIFF sequence.
    Tiff16,
    /// 16-bit PNG sequence.
    Png16,
    /// DNxHR LB (8-bit 4:2:2, low bandwidth) in QuickTime, sound, timecode.
    DnxHrLb,
    /// DNxHR SQ (8-bit 4:2:2) in QuickTime.
    DnxHrSq,
    /// DNxHR HQ (8-bit 4:2:2) in QuickTime.
    DnxHrHq,
    /// DNxHR HQX (10-bit 4:2:2) in QuickTime.
    DnxHr,
    /// DNxHR 444 (10-bit 4:4:4) in QuickTime.
    DnxHr444,
}

impl Format {
    pub const ALL: [Format; 18] = [
        Format::H264,
        Format::Clv,
        Format::Png,
        Format::Gif,
        Format::Hevc10,
        Format::M4a,
        Format::ProRes422Hq,
        Format::ProRes4444,
        Format::ProResProxy,
        Format::Exr,
        Format::Dpx,
        Format::Tiff16,
        Format::Png16,
        Format::DnxHrLb,
        Format::DnxHrSq,
        Format::DnxHrHq,
        Format::DnxHr,
        Format::DnxHr444,
    ];

    /// The mastering formats written by `export_pro` (16-bit pictures).
    pub fn is_pro(self) -> bool {
        matches!(self, Format::ProRes422Hq | Format::ProRes4444 | Format::ProResProxy | Format::Exr | Format::Dpx | Format::Tiff16 | Format::Png16 | Format::DnxHrLb | Format::DnxHrSq | Format::DnxHrHq | Format::DnxHr | Format::DnxHr444)
    }

    /// The DNxHR profile of a DNxHR format.
    pub fn dnxhr(self) -> Option<studio_mov::dnxhr::Profile> {
        use studio_mov::dnxhr::Profile;
        match self {
            Format::DnxHrLb => Some(Profile::Lb),
            Format::DnxHrSq => Some(Profile::Sq),
            Format::DnxHrHq => Some(Profile::Hq),
            Format::DnxHr => Some(Profile::Hqx),
            Format::DnxHr444 => Some(Profile::P444),
            _ => None,
        }
    }

    /// A frame sequence written into a folder.
    pub fn is_sequence(self) -> bool {
        matches!(self, Format::Png | Format::Exr | Format::Dpx | Format::Tiff16 | Format::Png16)
    }
    pub fn label(self) -> &'static str {
        match self {
            Format::H264 => "H.264 (MP4)",
            Format::Clv => "Zornov intermediate (CLV)",
            Format::Png => "PNG sequence",
            Format::Gif => "GIF",
            Format::Hevc10 => "HEVC 10-bit (MP4)",
            Format::M4a => "Audio only (M4A)",
            Format::ProRes422Hq => "ProRes 422 HQ (MOV)",
            Format::ProRes4444 => "ProRes 4444 (MOV)",
            Format::ProResProxy => "ProRes 422 Proxy (MOV)",
            Format::Exr => "OpenEXR sequence (ACES)",
            Format::Dpx => "DPX sequence (10-bit)",
            Format::Tiff16 => "TIFF sequence (16-bit)",
            Format::Png16 => "PNG sequence (16-bit)",
            Format::DnxHrLb => "DNxHR LB (MOV)",
            Format::DnxHrSq => "DNxHR SQ (MOV)",
            Format::DnxHrHq => "DNxHR HQ (MOV)",
            Format::DnxHr => "DNxHR HQX 10-bit (MOV)",
            Format::DnxHr444 => "DNxHR 444 10-bit (MOV)",
        }
    }
    pub fn extension(self) -> &'static str {
        match self {
            Format::H264 => ".mp4",
            Format::Clv => ".clv",
            Format::Png => "/",
            Format::Gif => ".gif",
            Format::Hevc10 => ".mp4",
            Format::M4a => ".m4a",
            Format::ProRes422Hq | Format::ProRes4444 | Format::ProResProxy | Format::DnxHrLb | Format::DnxHrSq | Format::DnxHrHq | Format::DnxHr | Format::DnxHr444 => ".mov",
            Format::Exr | Format::Dpx | Format::Tiff16 | Format::Png16 => "/",
        }
    }
}

