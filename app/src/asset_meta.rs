// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What a media file says about itself, read without decoding it: a
//! fingerprint that survives a rename or a move (so relinking and
//! duplicate finding work), and the shooting metadata a camera writes:
//! make, model, lens, exposure and place from a photo's Exif; creation
//! time, device, place and start timecode from a QuickTime / MP4 movie.

use serde_json::{Map, Value, json};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

const CHUNK: usize = 64 * 1024;

/// `size-hash`: the length and a hash of the start, middle and end of the
/// file. Cheap on huge files; two files with the same fingerprint are
/// confirmed by [`same_content`] before anything is called a duplicate.
pub fn fingerprint(path: &Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h = (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3);
        }
    };
    let mut buf = vec![0u8; CHUNK];
    for at in [0, len.saturating_sub(CHUNK as u64) / 2, len.saturating_sub(CHUNK as u64)] {
        f.seek(SeekFrom::Start(at)).ok()?;
        let n = f.read(&mut buf).ok()?;
        eat(&buf[..n]);
    }
    Some(format!("{len:x}-{h:016x}"))
}

/// Whether two files hold the same bytes (SHA-256 of both).
pub fn same_content(a: &Path, b: &Path) -> bool {
    let sum = |p: &Path| studio_ai::sha256::sha256_file(p, |_| true).ok().flatten();
    match (sum(a), sum(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// The shooting metadata found in `path` (empty when there is none).
pub fn read(path: &Path) -> Map<String, Value> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let mut out = Map::new();
    match ext.as_str() {
        "jpg" | "jpeg" => {
            if let Ok(b) = std::fs::read(path) {
                exif_of_jpeg(&b, &mut out);
            }
        }
        "mp4" | "mov" | "m4v" => movie_meta(path, &mut out),
        _ => {}
    }
    out
}

// ------------------------------------------------------------------ Exif

fn exif_of_jpeg(b: &[u8], out: &mut Map<String, Value>) {
    if b.len() < 4 || b[0] != 0xFF || b[1] != 0xD8 {
        return;
    }
    let mut i = 2;
    while i + 4 <= b.len() && b[i] == 0xFF {
        let marker = b[i + 1];
        if marker == 0xDA || marker == 0xD9 {
            break;
        }
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        let seg = &b[(i + 4).min(b.len())..(i + 2 + len).min(b.len())];
        if marker == 0xE1 && seg.starts_with(b"Exif\0\0") {
            exif(&seg[6..], out);
            return;
        }
        i += 2 + len;
    }
}

struct Tiff<'a> {
    d: &'a [u8],
    le: bool,
}

impl Tiff<'_> {
    fn u16(&self, at: usize) -> Option<u16> {
        let b: [u8; 2] = self.d.get(at..at + 2)?.try_into().ok()?;
        Some(if self.le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) })
    }
    fn u32(&self, at: usize) -> Option<u32> {
        let b: [u8; 4] = self.d.get(at..at + 4)?.try_into().ok()?;
        Some(if self.le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
    }
    /// The entries of the IFD at `at`: (tag, type, count, value-or-offset position).
    fn ifd(&self, at: usize) -> Vec<(u16, u16, u32, usize)> {
        let n = self.u16(at).unwrap_or(0) as usize;
        (0..n.min(200)).filter_map(|k| {
            let e = at + 2 + k * 12;
            Some((self.u16(e)?, self.u16(e + 2)?, self.u32(e + 4)?, e + 8))
        }).collect()
    }
    /// Where an entry's data is (inline when it fits four bytes).
    fn data(&self, ty: u16, count: u32, slot: usize) -> Option<usize> {
        let size = match ty { 1 | 2 | 7 => 1, 3 => 2, 4 => 4, 5 | 10 => 8, _ => return None } * count as usize;
        if size <= 4 { Some(slot) } else { Some(self.u32(slot)? as usize) }
    }
    fn text(&self, e: (u16, u16, u32, usize)) -> Option<String> {
        let at = self.data(e.1, e.2, e.3)?;
        let raw = self.d.get(at..at + e.2 as usize)?;
        let s = String::from_utf8_lossy(raw).trim_matches(|c: char| c == '\0' || c.is_whitespace()).to_owned();
        (!s.is_empty()).then_some(s)
    }
    fn rational(&self, e: (u16, u16, u32, usize), k: usize) -> Option<f64> {
        let at = self.data(e.1, e.2, e.3)? + k * 8;
        let (n, d) = (self.u32(at)? as f64, self.u32(at + 4)? as f64);
        (d != 0.0).then_some(n / d)
    }
}

fn exif(t: &[u8], out: &mut Map<String, Value>) {
    let le = match t.get(0..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return,
    };
    let tf = Tiff { d: t, le };
    let Some(first) = tf.u32(4) else { return };
    let mut exif_ifd = None;
    let mut gps_ifd = None;
    for e in tf.ifd(first as usize) {
        match e.0 {
            0x010F => put(out, "make", tf.text(e)),
            0x0110 => put(out, "camera", tf.text(e)),
            0x0131 => put(out, "software", tf.text(e)),
            0x8769 => exif_ifd = tf.u32(e.3),
            0x8825 => gps_ifd = tf.u32(e.3),
            _ => {}
        }
    }
    if let Some(at) = exif_ifd {
        for e in tf.ifd(at as usize) {
            match e.0 {
                0x9003 => put(out, "shot_at", tf.text(e).map(|s| exif_time(&s))),
                0xA434 => put(out, "lens", tf.text(e)),
                0x829A => put(out, "shutter", tf.rational(e, 0).map(|s| if s > 0.0 && s < 1.0 { format!("1/{}", (1.0 / s).round()) } else { format!("{s}") })),
                0x829D => put(out, "aperture", tf.rational(e, 0).map(|f| format!("f/{f:.1}"))),
                0x920A => put(out, "focal_length_mm", tf.rational(e, 0)),
                0x8827 => put(out, "iso", tf.u16(e.3)),
                _ => {}
            }
        }
    }
    if let Some(at) = gps_ifd {
        let (mut lat, mut lon, mut lat_ref, mut lon_ref) = (None, None, 'N', 'E');
        let dms = |e: (u16, u16, u32, usize)| Some(tf.rational(e, 0)? + tf.rational(e, 1)? / 60.0 + tf.rational(e, 2)? / 3600.0);
        for e in tf.ifd(at as usize) {
            match e.0 {
                1 => lat_ref = tf.text(e).and_then(|s| s.chars().next()).unwrap_or('N'),
                2 => lat = dms(e),
                3 => lon_ref = tf.text(e).and_then(|s| s.chars().next()).unwrap_or('E'),
                4 => lon = dms(e),
                _ => {}
            }
        }
        if let (Some(la), Some(lo)) = (lat, lon) {
            let (la, lo) = (if lat_ref == 'S' { -la } else { la }, if lon_ref == 'W' { -lo } else { lo });
            out.insert("location".into(), json!({"lat": (la * 1e6).round() / 1e6, "lon": (lo * 1e6).round() / 1e6}));
        }
    }
}

/// `2024:05:01 10:20:30` to `2024-05-01T10:20:30`.
fn exif_time(s: &str) -> String {
    let (d, t) = s.split_once(' ').unwrap_or((s, ""));
    format!("{}T{t}", d.replace(':', "-"))
}

fn put<T: Into<Value>>(out: &mut Map<String, Value>, k: &str, v: Option<T>) {
    if let Some(v) = v {
        out.insert(k.into(), v.into());
    }
}

// ------------------------------------------------------------- QuickTime

struct BoxAt {
    kind: [u8; 4],
    body: std::ops::Range<usize>,
}

fn boxes(d: &[u8], range: std::ops::Range<usize>) -> Vec<BoxAt> {
    let mut out = Vec::new();
    let mut i = range.start;
    while i + 8 <= range.end.min(d.len()) {
        let size = u32::from_be_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]) as usize;
        let kind = [d[i + 4], d[i + 5], d[i + 6], d[i + 7]];
        let (head, size) = if size == 1 && i + 16 <= d.len() {
            (16, u64::from_be_bytes(d[i + 8..i + 16].try_into().unwrap_or([0; 8])) as usize)
        } else if size == 0 {
            (8, range.end - i)
        } else {
            (8, size)
        };
        if size < head || i + size > range.end.min(d.len()) {
            break;
        }
        out.push(BoxAt { kind, body: i + head..i + size });
        i += size;
    }
    out
}

fn find<'a>(list: &'a [BoxAt], kind: &[u8; 4]) -> Option<&'a BoxAt> {
    list.iter().find(|b| &b.kind == kind)
}

/// The `moov` box of a movie, read from the file without the media data.
fn moov_bytes(path: &Path) -> Option<Vec<u8>> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut at = 0u64;
    while at + 8 <= len {
        f.seek(SeekFrom::Start(at)).ok()?;
        let mut h = [0u8; 16];
        f.read_exact(&mut h[..8]).ok()?;
        let mut size = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
        if size == 1 {
            f.read_exact(&mut h[8..16]).ok()?;
            size = u64::from_be_bytes(h[8..16].try_into().ok()?);
        } else if size == 0 {
            size = len - at;
        }
        if size < 8 {
            return None;
        }
        if &h[4..8] == b"moov" {
            if size > 64 << 20 {
                return None;
            }
            f.seek(SeekFrom::Start(at)).ok()?;
            let mut v = vec![0u8; size as usize];
            f.read_exact(&mut v).ok()?;
            return Some(v);
        }
        at += size;
    }
    None
}

fn movie_meta(path: &Path, out: &mut Map<String, Value>) {
    let Some(d) = moov_bytes(path) else { return };
    let top = boxes(&d, 0..d.len());
    let Some(moov) = find(&top, b"moov") else { return };
    let kids = boxes(&d, moov.body.clone());
    if let Some(mvhd) = find(&kids, b"mvhd") {
        let b = &d[mvhd.body.clone()];
        let secs = if b.first() == Some(&1) && b.len() >= 12 { u64::from_be_bytes(b[4..12].try_into().unwrap_or([0; 8])) } else if b.len() >= 8 { u32::from_be_bytes([b[4], b[5], b[6], b[7]]) as u64 } else { 0 };
        // QuickTime counts from 1904-01-01.
        if secs > 2_082_844_800 {
            out.insert("created".into(), civil(secs - 2_082_844_800).into());
        }
    }
    // Apple-style `meta` (keys + ilst) and classic `udta` tags.
    if let Some(meta) = find(&kids, b"meta") {
        apple_meta(&d, meta.body.clone(), out);
    }
    if let Some(udta) = find(&kids, b"udta") {
        for b in boxes(&d, udta.body.clone()) {
            let body = &d[b.body.clone()];
            match &b.kind {
                b"\xA9xyz" => put(out, "location", iso6709(&body_text(body))),
                b"\xA9mak" => put(out, "make", Some(body_text(body))),
                b"\xA9mod" => put(out, "camera", Some(body_text(body))),
                b"meta" => apple_meta(&d, b.body.clone(), out),
                _ => {}
            }
        }
    }
    for trak in kids.iter().filter(|b| &b.kind == b"trak") {
        if let Some(tc) = timecode_of(path, &d, trak.body.clone()) {
            out.insert("timecode".into(), tc.into());
            break;
        }
    }
}

/// The text of a `©xyz`-style box (2-byte length, 2-byte language, text).
fn body_text(b: &[u8]) -> String {
    let t = if b.len() > 4 && usize::from(u16::from_be_bytes([b[0], b[1]])) + 4 <= b.len() { &b[4..] } else { b };
    String::from_utf8_lossy(t).trim_matches(|c: char| c == '\0' || c.is_whitespace()).to_owned()
}

fn apple_meta(d: &[u8], body: std::ops::Range<usize>, out: &mut Map<String, Value>) {
    // `meta` is a full box in QuickTime files (4 bytes of version/flags).
    let start = if d.get(body.start + 4..body.start + 8).is_some_and(|k| k == b"hdlr" || k == b"keys") { body.start + 4 } else { body.start };
    let kids = boxes(d, start..body.end);
    let (Some(keys), Some(ilst)) = (find(&kids, b"keys"), find(&kids, b"ilst")) else { return };
    let kb = &d[keys.body.clone()];
    let mut names = Vec::new();
    let mut i = 8;
    while i + 8 <= kb.len() {
        let n = u32::from_be_bytes([kb[i], kb[i + 1], kb[i + 2], kb[i + 3]]) as usize;
        if n < 8 || i + n > kb.len() {
            break;
        }
        names.push(String::from_utf8_lossy(&kb[i + 8..i + n]).into_owned());
        i += n;
    }
    for item in boxes(d, ilst.body.clone()) {
        let idx = u32::from_be_bytes(item.kind) as usize;
        let Some(name) = idx.checked_sub(1).and_then(|k| names.get(k)) else { continue };
        let Some(data) = boxes(d, item.body.clone()).into_iter().find(|b| &b.kind == b"data") else { continue };
        let text = String::from_utf8_lossy(d.get(data.body.start + 8..data.body.end).unwrap_or_default()).trim().to_owned();
        match name.as_str() {
            "com.apple.quicktime.make" => put(out, "make", Some(text)),
            "com.apple.quicktime.model" => put(out, "camera", Some(text)),
            "com.apple.quicktime.software" => put(out, "software", Some(text)),
            "com.apple.quicktime.creationdate" => put(out, "shot_at", Some(text)),
            "com.apple.quicktime.location.ISO6709" => put(out, "location", iso6709(&text)),
            _ => {}
        }
    }
}

/// `+37.7749-122.4194+010.0/` to {"lat":…, "lon":…}.
pub fn iso6709(s: &str) -> Option<Value> {
    let s = s.trim().trim_end_matches('/');
    let mut parts = Vec::new();
    let mut cur = String::new();
    for c in s.chars() {
        if (c == '+' || c == '-') && !cur.is_empty() {
            parts.push(std::mem::take(&mut cur));
        }
        cur.push(c);
    }
    parts.push(cur);
    let lat: f64 = parts.first()?.parse().ok()?;
    let lon: f64 = parts.get(1)?.parse().ok()?;
    Some(json!({"lat": lat, "lon": lon}))
}

/// Days-since-1970 arithmetic, to `YYYY-MM-DDTHH:MM:SSZ`.
pub fn civil(unix: u64) -> String {
    let (days, rem) = ((unix / 86400) as i64, unix % 86400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Start timecode from a `tmcd` track: its description holds the frame
/// rate, the single sample holds the frame number of the first frame.
fn timecode_of(path: &Path, d: &[u8], trak: std::ops::Range<usize>) -> Option<String> {
    let t = boxes(d, trak);
    let mdia = boxes(d, find(&t, b"mdia")?.body.clone());
    let minf = boxes(d, find(&mdia, b"minf")?.body.clone());
    let stbl = boxes(d, find(&minf, b"stbl")?.body.clone());
    let stsd = find(&stbl, b"stsd")?;
    let s = &d[stsd.body.clone()];
    // version/flags, entry count, then the entry: size, "tmcd", 6 reserved, index, 4 reserved, flags, scale, frame duration, frames.
    if s.get(12..16)? != b"tmcd" {
        return None;
    }
    let e = &s[8..];
    let flags = u32::from_be_bytes(e.get(24..28)?.try_into().ok()?);
    let frames_per_sec = *e.get(36)? as u32;
    let frames_per_sec = if frames_per_sec == 0 { 24 } else { frames_per_sec };
    let drop = flags & 1 != 0;
    // First chunk offset: where the 4-byte frame number lives.
    let off = if let Some(c) = find(&stbl, b"stco") {
        let b = &d[c.body.clone()];
        u32::from_be_bytes(b.get(8..12)?.try_into().ok()?) as u64
    } else {
        let c = find(&stbl, b"co64")?;
        let b = &d[c.body.clone()];
        u64::from_be_bytes(b.get(8..16)?.try_into().ok()?)
    };
    let mut f = std::fs::File::open(path).ok()?;
    f.seek(SeekFrom::Start(off)).ok()?;
    let mut n = [0u8; 4];
    f.read_exact(&mut n).ok()?;
    Some(frames_to_tc(u32::from_be_bytes(n), frames_per_sec, drop))
}

/// A frame count as `HH:MM:SS:FF` (`;` before the frames for drop-frame).
pub fn frames_to_tc(mut n: u32, fps: u32, drop: bool) -> String {
    if drop && fps >= 30 {
        // Drop 2 frames each minute except every tenth (29.97), 4 at 59.94.
        let d = fps / 15;
        let per_min = fps * 60 - d;
        let per_10 = per_min * 10 + d;
        let (tens, rest) = (n / per_10, n % per_10);
        n += d * 9 * tens + if rest > d { d * ((rest - d) / per_min) } else { 0 };
    }
    let (f, s, m, h) = (n % fps, n / fps % 60, n / fps / 60 % 60, n / fps / 3600);
    format!("{h:02}:{m:02}:{s:02}{}{f:02}", if drop { ';' } else { ':' })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timecodes() {
        assert_eq!(frames_to_tc(0, 24, false), "00:00:00:00");
        assert_eq!(frames_to_tc(24 * 3600 + 30, 24, false), "01:00:01:06");
        assert_eq!(frames_to_tc(1800, 30, true), "00:01:00;02");
        assert_eq!(frames_to_tc(17982, 30, true), "00:10:00;00");
    }

    #[test]
    fn places_and_dates() {
        let v = iso6709("+37.7749-122.4194+010.0/").unwrap();
        assert_eq!(v["lat"], 37.7749);
        assert_eq!(v["lon"], -122.4194);
        assert_eq!(civil(0), "1970-01-01T00:00:00Z");
        assert_eq!(civil(1_714_556_430), "2024-05-01T09:40:30Z");
    }

    #[test]
    fn fingerprint_follows_content_not_name() {
        let d = std::env::temp_dir().join(format!("studio-meta-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let big: Vec<u8> = (0..400_000u32).map(|i| (i * 7 % 251) as u8).collect();
        std::fs::write(d.join("a.bin"), &big).unwrap();
        std::fs::write(d.join("b.bin"), &big).unwrap();
        let mut other = big.clone();
        other[200_000] ^= 1;
        std::fs::write(d.join("c.bin"), &other).unwrap();
        assert_eq!(fingerprint(&d.join("a.bin")), fingerprint(&d.join("b.bin")));
        assert!(same_content(&d.join("a.bin"), &d.join("b.bin")));
        assert!(!same_content(&d.join("a.bin"), &d.join("c.bin")));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A little-endian Exif block with make, model, lens, aperture and a GPS position.
    #[test]
    fn exif_fields() {
        fn entry(v: &mut Vec<u8>, tag: u16, ty: u16, count: u32, val: u32) {
            v.extend(tag.to_le_bytes());
            v.extend(ty.to_le_bytes());
            v.extend(count.to_le_bytes());
            v.extend(val.to_le_bytes());
        }
        // Layout: header(8) IFD0 at 8 (3 entries) = 8+2+36+4 = 50; strings after.
        let mut t = b"II*\0".to_vec();
        t.extend(8u32.to_le_bytes());
        let ifd0 = 8;
        let n0 = 4usize;
        let ifd0_len = 2 + n0 * 12 + 4;
        let exif_at = ifd0 + ifd0_len;
        let exif_len = 2 + 2 * 12 + 4;
        let gps_at = exif_at + exif_len;
        let gps_len = 2 + 4 * 12 + 4;
        let data_at = gps_at + gps_len;
        let make = b"Acme\0";
        let lens = b"Prime 35mm\0\0"; // 12 bytes
        let strings_at = data_at as u32;
        t.extend((n0 as u16).to_le_bytes());
        entry(&mut t, 0x010F, 2, 5, strings_at);
        entry(&mut t, 0x0110, 2, 4, u32::from_le_bytes(*b"X1\0\0"));
        entry(&mut t, 0x8769, 4, 1, exif_at as u32);
        entry(&mut t, 0x8825, 4, 1, gps_at as u32);
        t.extend(0u32.to_le_bytes());
        t.extend(2u16.to_le_bytes());
        let lens_at = strings_at + 5 + 1; // make + pad
        let fnum_at = lens_at + 12;
        entry(&mut t, 0x829D, 5, 1, fnum_at);
        entry(&mut t, 0xA434, 2, 12, lens_at);
        t.extend(0u32.to_le_bytes());
        t.extend(4u16.to_le_bytes());
        let lat_at = fnum_at + 8;
        let lon_at = lat_at + 24;
        entry(&mut t, 1, 2, 2, u32::from_le_bytes(*b"N\0\0\0"));
        entry(&mut t, 2, 5, 3, lat_at);
        entry(&mut t, 3, 2, 2, u32::from_le_bytes(*b"W\0\0\0"));
        entry(&mut t, 4, 5, 3, lon_at);
        t.extend(0u32.to_le_bytes());
        assert_eq!(t.len(), data_at);
        t.extend(make);
        t.push(0);
        t.extend(lens);
        for (n, d) in [(28u32, 10u32)] {
            t.extend(n.to_le_bytes());
            t.extend(d.to_le_bytes());
        }
        for (n, d) in [(37u32, 1u32), (30, 1), (0, 1), (122, 1), (15, 1), (0, 1)] {
            t.extend(n.to_le_bytes());
            t.extend(d.to_le_bytes());
        }
        // lat/lon blocks are 24 bytes each, in the order written above.
        let mut m = Map::new();
        exif(&t, &mut m);
        assert_eq!(m["make"], "Acme");
        assert_eq!(m["camera"], "X1");
        assert_eq!(m["lens"], "Prime 35mm");
        assert_eq!(m["aperture"], "f/2.8");
        assert_eq!(m["location"]["lat"], 37.5);
        assert_eq!(m["location"]["lon"], -122.25);
    }
}
