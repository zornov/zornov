// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The caller-facing types: what to pick, how much to fetch at once, and
//! progress.

/// Which quality to take when a page or stream offers several.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    /// The highest resolution (then the highest bit rate).
    Best,
    /// The smallest.
    Lowest,
    /// The largest at or below this many lines; if none is that small, the
    /// smallest above it.
    Height(u32),
    /// Sound only.
    AudioOnly,
}

impl Quality {
    /// "best", "worst" / "lowest", "audio", "720", "720p", "1080p60", "4k".
    pub fn parse(s: &str) -> Result<Quality, String> {
        let t = s.trim().to_ascii_lowercase();
        Ok(match t.as_str() {
            "" | "best" | "highest" | "max" | "auto" => Quality::Best,
            "worst" | "lowest" | "min" | "smallest" | "low" => Quality::Lowest,
            "audio" | "audio only" | "audio-only" | "sound" | "sound only" => Quality::AudioOnly,
            "8k" => Quality::Height(4320),
            "4k" | "uhd" => Quality::Height(2160),
            "2k" | "qhd" => Quality::Height(1440),
            "hd" => Quality::Height(720),
            "fhd" => Quality::Height(1080),
            _ => {
                let digits: String = t.chars().take_while(char::is_ascii_digit).collect();
                match digits.parse::<u32>() {
                    Ok(n) if (100..=8640).contains(&n) => Quality::Height(n),
                    _ => return Err(format!("\"{s}\" is not a quality: use best, lowest, audio, or a height such as 720 or 1080p")),
                }
            }
        })
    }
}

#[derive(Clone, Debug)]
pub struct Options {
    pub quality: Quality,
    /// Segments fetched at once (1 to 16).
    pub parallel: usize,
    /// Most bytes to take in all.
    pub max_bytes: u64,
    /// Longest the segments may take in all.
    pub time_limit: std::time::Duration,
    /// Preferred sound language (ISO 639 code) when a stream has several.
    pub language: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Options { quality: Quality::Best, parallel: 6, max_bytes: 4 << 30, time_limit: std::time::Duration::from_secs(1800), language: None }
    }
}

/// Where a download is. The callback returns `false` to stop (what is
/// fetched stays, so asking again resumes).
#[derive(Clone, Debug)]
pub struct Progress {
    /// What is happening: "looking at the page", "fetching", "joining".
    pub stage: &'static str,
    pub done: u64,
    pub total: u64,
    pub bytes: u64,
}

pub type Report<'a> = &'a (dyn Fn(&Progress) -> bool + Sync);

/// A report that ignores everything and never cancels.
pub fn quiet(_: &Progress) -> bool {
    true
}

/// What was saved.
#[derive(Clone, Debug)]
pub struct Saved {
    pub file: std::path::PathBuf,
    /// The page's title, when it had one.
    pub title: Option<String>,
    /// `"file"`, `"hls"` or `"dash"`.
    pub via: &'static str,
    /// What was chosen, e.g. "1280x720, 2.4 Mbit/s".
    pub chosen: String,
    /// What else was on offer.
    pub available: Vec<String>,
    /// Anything the person should know (a live stream cut off, a part left out).
    pub notes: Vec<String>,
}
