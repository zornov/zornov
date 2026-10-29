// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Media Studio cannot decode, made playable.
//!
//! Studio's own decoders read H.264 and HEVC video in MP4 / MOV, AAC, WAV,
//! MP3, FLAC, Ogg, PNG and JPEG. VP9, AV1 and ProRes video in WebM, MKV,
//! MP4 or MOV with Opus or AAC sound are converted in Rust
//! (`crate::transcode`) to H.264 + AAC in MP4; Photoshop documents are
//! flattened in Rust. Anything else is refused with a plain message:
//! Studio converts with its own media code only.
//! The converted file sits next to the original, named
//! "<name> (Studio).<ext>", and is reused if it is already there.

use crate::media_io;
use std::path::{Path, PathBuf};

/// Whether Studio can decode `path` itself.
pub fn playable(path: &Path) -> bool {
    // ProRes, pro stills and frame sequences play as they are.
    if crate::services::media_meta::native(path) {
        return true;
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if media_io::IMAGE_EXT.contains(&ext.as_str()) {
        return true;
    }
    if media_io::lottie(path).is_some() {
        return true;
    }
    // Sound files: playable when Studio's decoders actually read them (an
    // Ogg may hold Opus, a FLAC may be 32-bit). Decoded once and cached.
    if media_io::SOUND_DECODED.contains(&ext.as_str()) {
        return path.to_str().is_some_and(|p| crate::sound_bridge::decoded(p).is_some());
    }
    let Ok(dm) = studio_mp4::open(path) else { return false };
    let movie = dm.movie();
    use studio_mp4::demux::{Codec, TrackKind};
    movie.tracks.iter().all(|t| match t.kind {
        // H.264 beyond 8-bit 4:2:0 (High 10, 4:2:2, 4:4:4) is neither in
        // Studio's decoder nor the Mac's: converted, or it hung on decode.
        TrackKind::Video => match &t.codec {
            Codec::Avc { config } => config.bit_depth_luma.unwrap_or(8) == 8 && config.chroma_format.unwrap_or(1) == 1 && !matches!(config.profile_idc, 110 | 122 | 244 | 44),
            Codec::Hevc { .. } => true,
            Codec::Av1 { config } => av1_plays(config),
            _ => false,
        },
        // Sound Studio cannot decode (Opus, AC-3, ALAC in MP4) is converted too.
        TrackKind::Audio => matches!(t.codec, Codec::Aac { .. }),
        _ => true,
    }) && movie.tracks.iter().any(|t| matches!(t.kind, TrackKind::Video | TrackKind::Audio))
}

/// AV1 that `Mp4Source` plays as it is (the Mac's AV1 engine, else
/// `studio-av1`): 8-bit 4:2:0. 10-bit, monochrome and 4:4:4 are converted.
fn av1_plays(config: &studio_mp4::Av1Config) -> bool {
    config.bit_depth() == 8 && !config.monochrome && (config.chroma_subsampling_x, config.chroma_subsampling_y) == (1, 1)
}

/// What a file is, by its bytes (or its name): picture, sound or
/// video.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Picture,
    Sound,
    Video,
}

/// What a file holds by its first bytes, as an extension (`jpg`, `png`,
/// `mp4`, `mp3`...), whatever its name says.
pub fn sniff(path: &Path) -> Option<&'static str> {
    use std::io::Read;
    let mut b = [0u8; 16];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut b)).ok()?;
    let b = &b[..n];
    let at = |i: usize, s: &[u8]| b.get(i..i + s.len()) == Some(s);
    Some(match () {
        _ if at(0, &[0xFF, 0xD8, 0xFF]) => "jpg",
        _ if at(0, b"\x89PNG") => "png",
        _ if at(0, b"GIF8") => "gif",
        _ if at(0, b"RIFF") && at(8, b"WEBP") => "webp",
        _ if at(0, b"RIFF") && at(8, b"WAVE") => "wav",
        _ if at(0, b"BM") => "bmp",
        _ if at(0, b"II*\0") || at(0, b"MM\0*") => "tiff",
        _ if at(4, b"ftypavif") => "avif",
        _ if at(4, b"ftypheic") || at(4, b"ftypheix") || at(4, b"ftypmif1") => "heic",
        _ if at(4, b"ftypM4A") => "m4a",
        _ if at(4, b"ftypqt") => "mov",
        _ if at(4, b"ftyp") => "mp4",
        _ if at(0, &[0x1A, 0x45, 0xDF, 0xA3]) => "mkv",
        _ if at(0, b"OggS") => "ogg",
        _ if at(0, b"fLaC") => "flac",
        _ if at(0, b"ID3") || (b.len() > 1 && b[0] == 0xFF && b[1] & 0xE0 == 0xE0) => "mp3",
        _ => return None,
    })
}

fn kind_of(path: &Path) -> Kind {
    // A picture is a picture, whatever the file is called: a photo saved
    // without its extension was made into a 0.04 s video.
    if matches!(sniff(path), Some("jpg" | "png" | "webp" | "bmp" | "tiff" | "avif" | "heic")) {
        return Kind::Picture;
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if matches!(ext.as_str(), "webp" | "heic" | "heif" | "tif" | "tiff" | "bmp" | "avif" | "psd" | "psb") {
        return Kind::Picture;
    }
    if matches!(ext.as_str(), "opus" | "m4a" | "aac" | "weba" | "aif" | "aiff" | "wma") { Kind::Sound } else { Kind::Video }
}

/// `path` itself when Studio can play it, else a playable copy (made now,
/// or found from an earlier conversion).
pub fn ensure_playable(path: &Path) -> Result<PathBuf, String> {
    if playable(path) {
        return Ok(path.to_path_buf());
    }
    let kind = kind_of(path);
    let ext = match kind {
        Kind::Picture => "png",
        Kind::Sound => "m4a",
        Kind::Video => "mp4",
    };
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "media".into());
    let out = path.with_file_name(format!("{stem} (Zornov).{ext}"));
    if out.is_file() && playable(&out) {
        return Ok(out);
    }
    // Photoshop documents: their merged image, read in Rust (the `import`
    // tool's `layers: true` brings each layer instead).
    if kind == Kind::Picture && crate::psd_import::is_psd(path) {
        return crate::psd_import::flatten(path, &out).map(|()| out.clone()).map_err(|e| format!("Could not open {}: {e}", path.display()));
    }
    if kind == Kind::Video && crate::transcode::handles(path) {
        return crate::transcode::to_mp4(path, &out).map(|()| out.clone()).map_err(|e| format!("Could not convert {}: {e}", path.display()));
    }
    Err(format!("{} is in a format Zornov cannot play or convert yet.", path.display()))
}

/// Media items in `doc` whose files Studio cannot play: (id, path).
pub fn unplayable(doc: &crate::doc::Doc) -> Vec<(u64, PathBuf)> {
    doc.model.media.iter().map(|m| (m.id.0, PathBuf::from(&m.path))).filter(|(_, p)| p.is_file() && !playable(p)).collect()
}

/// Points media item `id` at the playable file `path` (one undo step):
/// every clip using it plays from then on.
pub fn relink(doc: &mut crate::doc::Doc, id: u64, path: &Path) -> Result<(), String> {
    let Some(old) = doc.model.media.iter().find(|m| m.id.0 == id).cloned() else { return Err(format!("no media {id}")) };
    let pr = media_io::probe(path)?;
    let mut item = old.clone();
    item.path = path.display().to_string();
    item.info = studio_model::MediaInfo { kind: pr.kind, container: pr.container, duration: pr.duration.or(old.info.duration), video: pr.video, audio: pr.audio, extra: old.info.extra.clone() };
    if doc.apply("Make playable", studio_model::Edit::SetMedia { item: Box::new(item) }) { Ok(()) } else { Err(doc.error.take().unwrap_or_else(|| "the media could not be relinked".into())) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A photo saved without its extension is still a picture (it became
    /// a 0.04 s video); sniffing goes by the bytes, not the name.
    #[test]
    fn files_are_known_by_their_bytes() {
        let dir = std::env::temp_dir().join(format!("studio-sniff-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, bytes, want) in [
            ("photo", &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F'][..], Some("jpg")),
            ("img.bin", b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR", Some("png")),
            ("w", b"RIFF\0\0\0\0WEBPVP8 ", Some("webp")),
            ("clip", b"\0\0\0\x18ftypmp42\0\0\0\0", Some("mp4")),
            ("song", b"ID3\x04\0\0\0\0\0\0", Some("mp3")),
            ("page", b"<!doctype html>", None),
        ] {
            let p = dir.join(name);
            std::fs::write(&p, bytes).unwrap();
            assert_eq!(sniff(&p), want, "{name}");
        }
        assert_eq!(kind_of(&dir.join("photo")), Kind::Picture);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// AV1 in MP4 plays as it is when it is 8-bit 4:2:0; the rest converts.
    #[test]
    fn av1_plays_only_as_8_bit_420() {
        let cfg = |b1: u8, b2: u8| studio_mp4::Av1Config::parse(&[0x81, b1, b2, 0]).unwrap();
        assert!(av1_plays(&cfg(0x00, 0x0c)), "8-bit 4:2:0");
        assert!(!av1_plays(&cfg(0x00, 0x4c)), "10-bit");
        assert!(!av1_plays(&cfg(0x00, 0x1c)), "monochrome");
        assert!(!av1_plays(&cfg(0x20, 0x00)), "4:4:4");
    }
}
