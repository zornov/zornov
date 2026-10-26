// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The pieces of a deliverables bundle that need no project: caption
//! files, 24-bit WAV, a poster-frame score, and the manifest that lists
//! every file with its size and checksum.

use crate::sha256;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// One caption: seconds and text.
#[derive(Clone, Debug, PartialEq)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

fn stamp(t: f64, comma: bool) -> String {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02}{}{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, if comma { ',' } else { '.' }, ms % 1000)
}

/// SubRip (.srt).
pub fn srt(cues: &[Cue]) -> String {
    let mut s = String::new();
    for (i, c) in cues.iter().enumerate() {
        s.push_str(&format!("{}\n{} --> {}\n{}\n\n", i + 1, stamp(c.start, true), stamp(c.end, true), c.text.trim()));
    }
    s
}

/// WebVTT (.vtt).
pub fn vtt(cues: &[Cue]) -> String {
    let mut s = String::from("WEBVTT\n\n");
    for c in cues {
        s.push_str(&format!("{} --> {}\n{}\n\n", stamp(c.start, false), stamp(c.end, false), c.text.trim().replace("-->", "->")));
    }
    s
}

/// Interleaved float samples as a 24-bit PCM WAV file.
pub fn wav24(samples: &[f32], channels: u16, rate: u32) -> Vec<u8> {
    let data_len = samples.len() * 3;
    let mut out = Vec::with_capacity(44 + data_len + 1);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((36 + data_len + data_len % 2) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * channels as u32 * 3).to_le_bytes());
    out.extend_from_slice(&(channels * 3).to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    for &s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32).to_le_bytes()[..3]);
    }
    if data_len % 2 == 1 {
        out.push(0);
    }
    out
}

/// How good a poster this picture would make: contrast and detail, and
/// nothing for black, white or flat frames.
pub fn poster_score(rgba: &[u8], w: usize, h: usize) -> f64 {
    if w < 4 || h < 4 {
        return 0.0;
    }
    let step = (w.max(h) / 240).max(1);
    let (mut n, mut sum, mut sum2, mut edge) = (0f64, 0f64, 0f64, 0f64);
    let mut y = 0;
    while y + step < h {
        let mut x = 0;
        while x + step < w {
            let l = |x: usize, y: usize| {
                let i = (y * w + x) * 4;
                0.2126 * rgba[i] as f64 + 0.7152 * rgba[i + 1] as f64 + 0.0722 * rgba[i + 2] as f64
            };
            let v = l(x, y);
            n += 1.0;
            sum += v;
            sum2 += v * v;
            edge += (v - l(x + step, y)).abs() + (v - l(x, y + step)).abs();
            x += step;
        }
        y += step;
    }
    if n == 0.0 {
        return 0.0;
    }
    let mean = sum / n;
    let sd = (sum2 / n - mean * mean).max(0.0).sqrt();
    // Not too dark, not blown out.
    let exposure = 1.0 - ((mean - 118.0).abs() / 118.0).min(1.0);
    (sd * 0.5 + edge / n * 0.5) * (0.3 + 0.7 * exposure)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Item {
    pub kind: String,
    /// Relative to the bundle folder, `/` separated.
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    pub note: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub title: String,
    pub created: String,
    pub items: Vec<Item>,
    /// What could not be included, and why.
    pub missing: Vec<String>,
}

fn hash_file(p: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(p)?;
    let mut h = sha256::Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(sha256::hex(&h.finish()))
}

impl Manifest {
    pub fn new(title: &str) -> Manifest {
        Manifest { title: title.into(), created: crate::dcp::iso_now(), items: Vec::new(), missing: Vec::new() }
    }

    /// List `root/rel` (a file, or every file below a folder) with sizes and checksums.
    pub fn add(&mut self, root: &Path, rel: &str, kind: &str, note: &str) -> std::io::Result<()> {
        let p = root.join(rel);
        if p.is_dir() {
            let mut files: Vec<_> = std::fs::read_dir(&p)?.filter_map(|e| e.ok()).map(|e| e.path()).collect();
            files.sort();
            for f in files {
                let name = f.file_name().unwrap().to_string_lossy().into_owned();
                self.add(root, &format!("{rel}/{name}"), kind, note)?;
            }
            return Ok(());
        }
        self.items.push(Item { kind: kind.into(), path: rel.into(), bytes: std::fs::metadata(&p)?.len(), sha256: hash_file(&p)?, note: note.into() });
        Ok(())
    }

    /// Write `manifest.json` and a plain `README.txt`.
    pub fn write(&self, root: &Path) -> std::io::Result<()> {
        std::fs::write(root.join("manifest.json"), serde_json::to_vec_pretty(self).unwrap_or_default())?;
        let mut t = format!("{}\nDeliverables bundle made {}\n\n", self.title, self.created);
        let mut kinds: Vec<&str> = self.items.iter().map(|i| i.kind.as_str()).collect();
        kinds.dedup();
        for k in kinds {
            t.push_str(&format!("{}\n", k.to_uppercase()));
            for i in self.items.iter().filter(|i| i.kind == k) {
                t.push_str(&format!("  {}  ({:.1} MB){}\n", i.path, i.bytes as f64 / 1e6, if i.note.is_empty() { String::new() } else { format!("  {}", i.note) }));
            }
            t.push('\n');
        }
        if !self.missing.is_empty() {
            t.push_str("NOT INCLUDED\n");
            for m in &self.missing {
                t.push_str(&format!("  {m}\n"));
            }
        }
        std::fs::write(root.join("README.txt"), t)
    }

    /// Check every listed file against its size and checksum; the names that differ.
    pub fn verify(&self, root: &Path) -> Vec<String> {
        self.items.iter().filter(|i| std::fs::metadata(root.join(&i.path)).map(|m| m.len() != i.bytes).unwrap_or(true) || hash_file(&root.join(&i.path)).map(|h| h != i.sha256).unwrap_or(true)).map(|i| i.path.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caption_files_have_the_right_timing_syntax() {
        let c = vec![Cue { start: 1.5, end: 3.25, text: "Hello".into() }, Cue { start: 3661.001, end: 3662.0, text: " Two --> lines ".into() }];
        let s = srt(&c);
        assert!(s.starts_with("1\n00:00:01,500 --> 00:00:03,250\nHello\n\n2\n01:01:01,001 --> 01:01:02,000\n"), "{s}");
        let v = vtt(&c);
        assert!(v.starts_with("WEBVTT\n\n00:00:01.500 --> 00:00:03.250\n") && v.contains("Two -> lines"), "{v}");
    }

    #[test]
    fn wav24_has_a_valid_header_and_exact_samples() {
        let w = wav24(&[0.0, 1.0, -1.0, 0.5], 2, 48_000);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 12);
        assert_eq!(w.len(), 44 + 12);
        assert_eq!(&w[47..50], &[0xff, 0xff, 0x7f]);
        assert_eq!(&w[50..53], &[0x01, 0x00, 0x80]);
    }

    #[test]
    fn posters_prefer_detail_over_black_and_flat() {
        let (w, h) = (64, 48);
        let flat = vec![128u8; w * h * 4];
        let black = vec![0u8; w * h * 4];
        let mut detail = vec![255u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let v = if (x / 6 + y / 6) % 2 == 0 { 40 } else { 200 };
                let i = (y * w + x) * 4;
                detail[i] = v;
                detail[i + 1] = v;
                detail[i + 2] = v;
            }
        }
        assert!(poster_score(&detail, w, h) > poster_score(&flat, w, h) * 5.0);
        assert!(poster_score(&black, w, h) < 1.0);
    }

    #[test]
    fn manifest_lists_and_verifies_files() {
        let d = std::env::temp_dir().join(format!("studio-bundle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("a.txt"), "alpha").unwrap();
        std::fs::write(d.join("sub/b.txt"), "beta").unwrap();
        let mut m = Manifest::new("T");
        m.add(&d, "a.txt", "text", "first").unwrap();
        m.add(&d, "sub", "more", "").unwrap();
        m.missing.push("stems: none made".into());
        m.write(&d).unwrap();
        assert_eq!(m.items.len(), 2);
        assert!(m.verify(&d).is_empty());
        let readme = std::fs::read_to_string(d.join("README.txt")).unwrap();
        assert!(readme.contains("sub/b.txt") && readme.contains("NOT INCLUDED"));
        std::fs::write(d.join("a.txt"), "ALPHA").unwrap();
        assert_eq!(m.verify(&d), vec!["a.txt".to_string()]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
