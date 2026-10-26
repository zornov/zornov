// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Source timecode of QuickTime / MP4 files: the `tmcd` track that
//! cameras (ARRI, Sony, Canon, RED, Blackmagic) and editing systems write,
//! with its reel name.
//!
//! The track's sample entry gives the counting (flags, timescale, frame
//! duration, frames per second, and a `name` atom with the reel); its
//! first sample is the frame number of the file's first frame.

use crate::StartTimecode;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

struct Atom {
    kind: [u8; 4],
    /// Body start and end in the file.
    start: u64,
    end: u64,
}

fn atoms(f: &mut File, start: u64, end: u64) -> Vec<Atom> {
    let mut out = Vec::new();
    let mut pos = start;
    while pos + 8 <= end {
        let mut h = [0u8; 8];
        if f.seek(SeekFrom::Start(pos)).is_err() || f.read_exact(&mut h).is_err() {
            break;
        }
        let mut size = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
        let kind = [h[4], h[5], h[6], h[7]];
        let mut body = pos + 8;
        if size == 1 {
            let mut l = [0u8; 8];
            if f.read_exact(&mut l).is_err() {
                break;
            }
            size = u64::from_be_bytes(l);
            body += 8;
        } else if size == 0 {
            size = end - pos;
        }
        if size < 8 || pos + size > end {
            break;
        }
        out.push(Atom { kind, start: body, end: pos + size });
        pos += size;
    }
    out
}

fn read(f: &mut File, at: u64, n: usize) -> Option<Vec<u8>> {
    f.seek(SeekFrom::Start(at)).ok()?;
    let mut b = vec![0u8; n];
    f.read_exact(&mut b).ok()?;
    Some(b)
}

fn child<'a>(list: &'a [Atom], kind: &[u8; 4]) -> Option<&'a Atom> {
    list.iter().find(|a| &a.kind == kind)
}

/// The start timecode and reel of a QuickTime / MP4 file, if it has a
/// timecode track.
pub fn timecode(path: &Path) -> Option<StartTimecode> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let top = atoms(&mut f, 0, len);
    let moov = child(&top, b"moov")?;
    let moov_kids = atoms(&mut f, moov.start, moov.end);
    for trak in moov_kids.iter().filter(|a| &a.kind == b"trak") {
        let tk = atoms(&mut f, trak.start, trak.end);
        let Some(mdia) = child(&tk, b"mdia") else { continue };
        let md = atoms(&mut f, mdia.start, mdia.end);
        let Some(hd) = child(&md, b"hdlr") else { continue };
        let h = read(&mut f, hd.start, 12)?;
        if &h[8..12] != b"tmcd" {
            continue;
        }
        let minf = child(&md, b"minf")?;
        let mi = atoms(&mut f, minf.start, minf.end);
        let stbl = child(&mi, b"stbl")?;
        let st = atoms(&mut f, stbl.start, stbl.end);
        let stsd = child(&st, b"stsd")?;
        // Full box header + entry count, then the first entry.
        let entries = atoms(&mut f, stsd.start + 8, stsd.end);
        let e = entries.first().filter(|e| &e.kind == b"tmcd")?;
        let body = read(&mut f, e.start, (e.end - e.start).min(4096) as usize)?;
        if body.len() < 26 {
            return None;
        }
        let flags = u32::from_be_bytes([body[12], body[13], body[14], body[15]]);
        let timescale = u32::from_be_bytes([body[16], body[17], body[18], body[19]]);
        let frame_dur = u32::from_be_bytes([body[20], body[21], body[22], body[23]]);
        let mut fps = body[24] as u32;
        if fps == 0 && frame_dur > 0 {
            fps = (timescale as f64 / frame_dur as f64).round() as u32;
        }
        // A `name` atom after the fixed fields holds the reel.
        let mut reel = None;
        for a in atoms(&mut f, e.start + 26, e.end) {
            if &a.kind == b"name" {
                let b = read(&mut f, a.start, (a.end - a.start) as usize)?;
                if b.len() >= 4 {
                    let n = u16::from_be_bytes([b[0], b[1]]) as usize;
                    let s = String::from_utf8_lossy(&b[4..(4 + n).min(b.len())]).trim().to_string();
                    if !s.is_empty() {
                        reel = Some(s);
                    }
                }
            }
        }
        // First sample: the chunk offset table's first entry.
        let offset = if let Some(c) = child(&st, b"co64") {
            let b = read(&mut f, c.start + 8, 8)?;
            u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
        } else {
            let c = child(&st, b"stco")?;
            let b = read(&mut f, c.start + 8, 4)?;
            u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as u64
        };
        let s = read(&mut f, offset, 4)?;
        let frame = u32::from_be_bytes([s[0], s[1], s[2], s[3]]);
        return Some(StartTimecode { frame, fps: fps.max(1), drop_frame: flags & 1 != 0, reel });
    }
    None
}

/// Uncompressed PCM sound of a QuickTime file (what cameras and editing
/// systems put next to ProRes): `lpcm` (any flags), `in24`, `in32`,
/// `twos`, `sowt`, `fl32`, `fl64` and `raw `. Returns the sample rate, the
/// channel count and interleaved samples in −1..1.
pub fn pcm(path: &Path) -> Option<(u32, u16, Vec<f32>)> {
    pcm_inner(path, true)
}

/// The sample rate and channel count of a QuickTime file's PCM sound,
/// without reading the samples.
pub fn pcm_format(path: &Path) -> Option<(u32, u16)> {
    pcm_inner(path, false).map(|(r, c, _)| (r, c))
}

fn pcm_inner(path: &Path, samples: bool) -> Option<(u32, u16, Vec<f32>)> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let top = atoms(&mut f, 0, len);
    let moov = child(&top, b"moov")?;
    for trak in atoms(&mut f, moov.start, moov.end).iter().filter(|a| &a.kind == b"trak") {
        let tk = atoms(&mut f, trak.start, trak.end);
        let Some(mdia) = child(&tk, b"mdia") else { continue };
        let md = atoms(&mut f, mdia.start, mdia.end);
        let Some(hd) = child(&md, b"hdlr") else { continue };
        if read(&mut f, hd.start, 12)?[8..12] != *b"soun" {
            continue;
        }
        let minf = child(&md, b"minf")?;
        let mi = atoms(&mut f, minf.start, minf.end);
        let stbl = child(&mi, b"stbl")?;
        let st = atoms(&mut f, stbl.start, stbl.end);
        let stsd = child(&st, b"stsd")?;
        let entries = atoms(&mut f, stsd.start + 8, stsd.end);
        let e = entries.first()?;
        let b = read(&mut f, e.start, (e.end - e.start).min(256) as usize)?;
        if b.len() < 28 {
            continue;
        }
        let version = u16::from_be_bytes([b[8], b[9]]);
        let (rate, channels, bits, float, little) = match (version, &e.kind) {
            (2, _) if b.len() >= 64 => {
                let rate = f64::from_be_bytes(b[32..40].try_into().ok()?) as u32;
                let ch = u32::from_be_bytes(b[40..44].try_into().ok()?) as u16;
                let bits = u32::from_be_bytes(b[48..52].try_into().ok()?) as u16;
                let flags = u32::from_be_bytes(b[52..56].try_into().ok()?);
                (rate, ch, bits, flags & 1 != 0, flags & 2 == 0)
            }
            _ => {
                let ch = u16::from_be_bytes([b[16], b[17]]);
                let size = u16::from_be_bytes([b[18], b[19]]);
                let rate = u32::from_be_bytes([b[24], b[25], b[26], b[27]]) >> 16;
                match &e.kind {
                    b"in24" => (rate, ch, 24, false, false),
                    b"in32" => (rate, ch, 32, false, false),
                    b"fl32" => (rate, ch, 32, true, false),
                    b"fl64" => (rate, ch, 64, true, false),
                    b"sowt" => (rate, ch, 16, false, true),
                    b"twos" => (rate, ch, size.max(8), false, false),
                    b"raw " => (rate, ch, 8, false, false),
                    _ => continue,
                }
            }
        };
        if !matches!(&e.kind, b"lpcm" | b"in24" | b"in32" | b"fl32" | b"fl64" | b"sowt" | b"twos" | b"raw ") || channels == 0 || rate == 0 {
            continue;
        }
        if !samples {
            return Some((rate, channels, Vec::new()));
        }
        let bytes_per = (bits as usize).div_ceil(8);
        let frame_bytes = bytes_per * channels as usize;
        // Chunk offsets, then how many frames each chunk holds.
        let offsets: Vec<u64> = if let Some(c) = child(&st, b"co64") {
            let h = read(&mut f, c.start + 4, 4)?;
            let n = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as usize;
            read(&mut f, c.start + 8, n * 8)?.chunks_exact(8).map(|x| u64::from_be_bytes(x.try_into().unwrap_or([0; 8]))).collect()
        } else {
            let c = child(&st, b"stco")?;
            let h = read(&mut f, c.start + 4, 4)?;
            let n = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as usize;
            read(&mut f, c.start + 8, n * 4)?.chunks_exact(4).map(|x| u32::from_be_bytes(x.try_into().unwrap_or([0; 4])) as u64).collect()
        };
        let stsc = child(&st, b"stsc")?;
        let h = read(&mut f, stsc.start + 4, 4)?;
        let n = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as usize;
        let runs: Vec<(u32, u32)> = read(&mut f, stsc.start + 8, n * 12)?.chunks_exact(12).map(|x| (u32::from_be_bytes([x[0], x[1], x[2], x[3]]), u32::from_be_bytes([x[4], x[5], x[6], x[7]]))).collect();
        // Old sound descriptions count samples per channel in stsz; v2
        // counts packets (frames). Both mean frames for PCM.
        let mut out: Vec<f32> = Vec::new();
        for (ci, off) in offsets.iter().enumerate() {
            let chunk = ci as u32 + 1;
            let per = runs.iter().rev().find(|r| r.0 <= chunk).map_or(0, |r| r.1) as usize;
            if per == 0 {
                continue;
            }
            let data = read(&mut f, *off, per * frame_bytes)?;
            for s in data.chunks_exact(bytes_per) {
                let v = match (bits, float, little) {
                    (8, _, _) => (s[0] as i8) as f32 / 128.0,
                    (16, false, true) => i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0,
                    (16, false, false) => i16::from_be_bytes([s[0], s[1]]) as f32 / 32768.0,
                    (24, false, true) => (i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8) as f32 / 8_388_608.0,
                    (24, false, false) => (i32::from_be_bytes([s[0], s[1], s[2], 0]) >> 8) as f32 / 8_388_608.0,
                    (32, false, true) => i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f32 / 2_147_483_648.0,
                    (32, false, false) => i32::from_be_bytes([s[0], s[1], s[2], s[3]]) as f32 / 2_147_483_648.0,
                    (32, true, true) => f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
                    (32, true, false) => f32::from_be_bytes([s[0], s[1], s[2], s[3]]),
                    (64, true, true) => f64::from_le_bytes(s.try_into().unwrap_or([0; 8])) as f32,
                    (64, true, false) => f64::from_be_bytes(s.try_into().unwrap_or([0; 8])) as f32,
                    _ => return None,
                };
                out.push(v);
            }
        }
        return (!out.is_empty()).then_some((rate, channels, out));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mov::{AudioTrack, MovWriter, VideoTrack};

    #[test]
    fn reads_back_what_the_writer_wrote() {
        let path = std::env::temp_dir().join(format!("studio-tc-{}.mov", std::process::id()));
        let tc = StartTimecode { frame: 107_895, fps: 30, drop_frame: true, reel: Some("A001C003".into()) };
        let v = VideoTrack { profile: studio_prores::Profile::Hq, width: 64, height: 32, rate_num: 30000, rate_den: 1001, colour: (1, 1, 1), alpha: false };
        let mut w = MovWriter::create(&path, v, Some(AudioTrack { sample_rate: 48000, channels: 2 }), Some(tc.clone())).unwrap();
        w.write_video(&[1, 2, 3, 4]).unwrap();
        w.write_audio(&[0.0; 3204]).unwrap();
        w.finish().unwrap();
        assert_eq!(timecode(&path), Some(tc));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn reads_back_pcm() {
        let path = std::env::temp_dir().join(format!("studio-pcm-{}.mov", std::process::id()));
        let v = VideoTrack { profile: studio_prores::Profile::Proxy, width: 16, height: 16, rate_num: 25, rate_den: 1, colour: (1, 1, 1), alpha: false };
        let mut w = MovWriter::create(&path, v, Some(AudioTrack { sample_rate: 48000, channels: 2 }), None).unwrap();
        let tone: Vec<f32> = (0..3840).map(|i| ((i / 2) as f32 * 0.05).sin() * 0.5).collect();
        for chunk in tone.chunks(1920) {
            w.write_video(&[0; 8]).unwrap();
            w.write_audio(chunk).unwrap();
        }
        w.finish().unwrap();
        let (rate, ch, s) = pcm(&path).unwrap();
        assert_eq!((rate, ch, s.len()), (48000, 2, tone.len()));
        for (a, b) in s.iter().zip(&tone) {
            assert!((a - b).abs() < 1e-6);
        }
        let _ = std::fs::remove_file(&path);
    }
}
