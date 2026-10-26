// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Emoji as images.
//!
//! The bundled colour emoji font is a COLRv1 build, which the rasteriser
//! cannot draw, and system emoji fonts differ between machines. The
//! content folder ships OpenMoji (618 px PNG, CC BY-SA 4.0) and Twemoji
//! (72 px PNG, CC BY 4.0) keyed by codepoint sequence, so emoji clusters
//! are drawn from those images: deterministic everywhere and sharp at
//! title sizes. Colour fonts remain the fallback for sequences the set
//! lacks.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Which bundled set to draw from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmojiStyle {
    /// OpenMoji, 618 px: the default for titles.
    #[default]
    OpenMoji,
    /// Twemoji, 72 px.
    Twemoji,
}

/// A decoded emoji bitmap (straight-alpha sRGB 8-bit).
#[derive(Clone, Debug)]
pub struct EmojiImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// A directory of emoji PNGs.
pub struct EmojiSet {
    dir: PathBuf,
    style: EmojiStyle,
    cache: Mutex<HashMap<String, Option<Arc<EmojiImage>>>>,
}

impl EmojiSet {
    /// Opens the set under `content_dir`, if its folder exists.
    pub fn open(content_dir: &Path, style: EmojiStyle) -> Option<EmojiSet> {
        let dir = match style {
            EmojiStyle::OpenMoji => content_dir.join("emoji/openmoji/618x618-color"),
            EmojiStyle::Twemoji => content_dir.join("emoji/twemoji/png72"),
        };
        if dir.is_dir() { Some(EmojiSet { dir, style, cache: Mutex::new(HashMap::new()) }) } else { None }
    }

    pub fn style(&self) -> EmojiStyle {
        self.style
    }

    /// File key for a cluster: hex codepoints joined by `-`.
    fn keys(&self, cluster: &str) -> Vec<String> {
        let upper = matches!(self.style, EmojiStyle::OpenMoji);
        let fmt = |cs: &[u32]| {
            cs.iter().map(|c| if upper { format!("{c:X}") } else { format!("{c:x}") }).collect::<Vec<_>>().join("-")
        };
        let all: Vec<u32> = cluster.chars().map(|c| c as u32).collect();
        let without_vs: Vec<u32> = all.iter().copied().filter(|&c| c != 0xFE0F).collect();
        let mut keys = vec![fmt(&all)];
        if without_vs != all {
            keys.push(fmt(&without_vs));
        }
        keys
    }

    /// Whether the set has an image for the cluster (cheap: file check,
    /// cached after the first decode).
    pub fn has(&self, cluster: &str) -> bool {
        if let Some(v) = self.cache.lock().unwrap_or_else(|e| e.into_inner()).get(cluster) {
            return v.is_some();
        }
        self.keys(cluster).iter().any(|k| self.dir.join(format!("{k}.png")).is_file())
    }

    /// The decoded image for the cluster, if any.
    pub fn image(&self, cluster: &str) -> Option<Arc<EmojiImage>> {
        if let Some(v) = self.cache.lock().unwrap_or_else(|e| e.into_inner()).get(cluster) {
            return v.clone();
        }
        let mut found = None;
        for k in self.keys(cluster) {
            let p = self.dir.join(format!("{k}.png"));
            if let Some(img) = decode(&p) {
                found = Some(Arc::new(img));
                break;
            }
        }
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).insert(cluster.to_string(), found.clone());
        found
    }
}

fn decode(path: &Path) -> Option<EmojiImage> {
    let file = std::fs::File::open(path).ok()?;
    let mut dec = png::Decoder::new(std::io::BufReader::new(file));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let bytes = &buf[..info.buffer_size()];
    let (w, h) = (info.width, info.height);
    let chans = info.color_type.samples();
    let sixteen = info.bit_depth == png::BitDepth::Sixteen;
    let get = |i: usize| -> u8 { if sixteen { bytes[i * 2] } else { bytes[i] } };
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for i in 0..(w * h) as usize {
        let b = i * chans;
        match info.color_type {
            png::ColorType::Rgba => rgba.extend_from_slice(&[get(b), get(b + 1), get(b + 2), get(b + 3)]),
            png::ColorType::Rgb => rgba.extend_from_slice(&[get(b), get(b + 1), get(b + 2), 255]),
            png::ColorType::GrayscaleAlpha => rgba.extend_from_slice(&[get(b), get(b), get(b), get(b + 1)]),
            _ => rgba.extend_from_slice(&[get(b), get(b), get(b), 255]),
        }
    }
    Some(EmojiImage { width: w, height: h, rgba })
}

/// Whether a grapheme cluster should be drawn as an emoji.
pub fn is_emoji_cluster(cluster: &str) -> bool {
    let chars: Vec<char> = cluster.chars().collect();
    if chars.is_empty() {
        return false;
    }
    // Keycaps (#️⃣), flags, ZWJ sequences, skin tones and VS16 all mark emoji.
    if chars.iter().any(|&c| c == '\u{20E3}' || c == '\u{FE0F}' || c == '\u{200D}' || ('\u{1F3FB}'..='\u{1F3FF}').contains(&c)) {
        return chars.iter().any(|&c| crate::font::is_emoji_presentation(c) || c == '\u{20E3}' || ('\u{2190}'..='\u{2BFF}').contains(&c) || c.is_ascii_digit() || c == '#' || c == '*');
    }
    chars.iter().any(|&c| crate::font::is_emoji_presentation(c) && c as u32 >= 0x1F000)
        || chars.iter().any(|&c| matches!(c as u32, 0x2614 | 0x2615 | 0x2648..=0x2653 | 0x267F | 0x2693 | 0x26A1 | 0x26AA | 0x26AB | 0x26BD | 0x26BE | 0x26C4 | 0x26C5 | 0x26CE | 0x26D4 | 0x26EA | 0x26F2 | 0x26F3 | 0x26F5 | 0x26FA | 0x26FD | 0x2705 | 0x270A | 0x270B | 0x2728 | 0x274C | 0x274E | 0x2753..=0x2755 | 0x2757 | 0x2795..=0x2797 | 0x27B0 | 0x27BF | 0x2B1B | 0x2B1C | 0x2B50 | 0x2B55))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cluster_detection() {
        assert!(is_emoji_cluster("😀"));
        assert!(is_emoji_cluster("👍🏽"));
        assert!(is_emoji_cluster("❤️"));
        assert!(is_emoji_cluster("🇬🇧"));
        assert!(is_emoji_cluster("#️⃣"));
        assert!(!is_emoji_cluster("a"));
        assert!(!is_emoji_cluster("中"));
        assert!(!is_emoji_cluster("©"));
    }

    #[test]
    fn keys_strip_variation_selector() {
        let set = EmojiSet { dir: PathBuf::new(), style: EmojiStyle::OpenMoji, cache: Mutex::new(HashMap::new()) };
        assert_eq!(set.keys("❤️"), vec!["2764-FE0F".to_string(), "2764".to_string()]);
        let tw = EmojiSet { dir: PathBuf::new(), style: EmojiStyle::Twemoji, cache: Mutex::new(HashMap::new()) };
        assert_eq!(tw.keys("🇬🇧"), vec!["1f1ec-1f1e7".to_string()]);
    }

    #[test]
    fn bundled_set_has_common_emoji() {
        let Some(set) = EmojiSet::open(&crate::font::default_content_dir(), EmojiStyle::OpenMoji) else { return };
        assert!(set.has("😀"));
        assert!(set.has("❤️"));
        let img = set.image("🚀").unwrap();
        assert_eq!((img.width, img.height), (618, 618));
    }
}
