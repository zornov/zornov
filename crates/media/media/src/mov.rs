// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! QuickTime movie (.mov) writer for exports with an alpha channel.
//!
//! Two video codecs, both lossless with straight (non-premultiplied) alpha:
//!
//! * **Animation (`rle `)**, 32-bit ARGB: every line is run-length coded
//!   against the previous frame (unchanged leading pixels and lines are
//!   skipped); key frames every `gop` frames are coded whole.
//! * **PNG (`png `)**: every frame is a PNG (Studio's own encoder).
//!
//! The file is `ftyp` + `mdat` (streamed, 64-bit size patched on finish) +
//! `moov`; with `fast_start` the finished file is rewritten with `moov`
//! in front so players can start before the whole file has arrived. Video
//! only: the export writes the sound as a separate file when asked.
//!
//! Layout follows Apple's QuickTime File Format specification
//! (moov/trak/mdia/minf/stbl, `alis` self-reference data reference,
//! Animation codec line format: skip byte, then RLE codes −1 = end of
//! line, 0 = another skip, n > 0 literal pixels, n < 0 a run of −n).

use crate::RgbaImage;
use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MovCodec {
    /// QuickTime Animation, 32-bit ARGB.
    Rle,
    /// PNG frames.
    Png,
}

impl MovCodec {
    pub fn fourcc(self) -> &'static [u8; 4] {
        match self {
            MovCodec::Rle => b"rle ",
            MovCodec::Png => b"png ",
        }
    }
    fn name(self) -> &'static str {
        match self {
            MovCodec::Rle => "Animation",
            MovCodec::Png => "PNG",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MovOptions {
    pub codec: MovCodec,
    /// Frame rate as a rational (e.g. 30000/1001).
    pub fps_num: u32,
    pub fps_den: u32,
    /// Key frame interval for RLE (0 = only the first frame).
    pub gop: u32,
    /// Put `moov` before `mdat` on finish.
    pub fast_start: bool,
}

/// Streams frames into a .mov file.
pub struct MovWriter {
    file: BufWriter<File>,
    path: PathBuf,
    w: u32,
    h: u32,
    opts: MovOptions,
    mdat_start: u64,
    pos: u64,
    sizes: Vec<u32>,
    offsets: Vec<u64>,
    keys: Vec<u32>,
    prev: Option<RgbaImage>,
}

fn be16(v: u16) -> [u8; 2] {
    v.to_be_bytes()
}
fn be32(v: u32) -> [u8; 4] {
    v.to_be_bytes()
}

/// An atom: size + type + body.
fn atom(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(body.len() + 8);
    v.extend_from_slice(&be32(body.len() as u32 + 8));
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}

fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

/// Identity matrix in 16.16 / 2.30 fixed point.
fn matrix() -> Vec<u8> {
    let mut m = Vec::new();
    for v in [0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000] {
        m.extend_from_slice(&be32(v));
    }
    m
}

impl MovWriter {
    pub fn create(path: &Path, w: u32, h: u32, opts: MovOptions) -> io::Result<Self> {
        if w == 0 || h == 0 || w > 0xffff || h > 0xffff || opts.fps_num == 0 || opts.fps_den == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "bad movie size or frame rate"));
        }
        let mut file = BufWriter::new(File::create(path)?);
        let ftyp = atom(b"ftyp", &cat(&[b"qt  ", &be32(0x2005_0300), b"qt  "]));
        file.write_all(&ftyp)?;
        // mdat with a 64-bit size, patched on finish.
        file.write_all(&be32(1))?;
        file.write_all(b"mdat")?;
        file.write_all(&0u64.to_be_bytes())?;
        let mdat_start = ftyp.len() as u64;
        Ok(Self { file, path: path.to_path_buf(), w, h, opts, mdat_start, pos: mdat_start + 16, sizes: Vec::new(), offsets: Vec::new(), keys: Vec::new(), prev: None })
    }

    pub fn frames(&self) -> usize {
        self.sizes.len()
    }

    /// Bytes of sample data written so far.
    pub fn video_bytes(&self) -> u64 {
        self.pos - self.mdat_start - 16
    }

    /// Appends one frame (straight-alpha RGBA, the movie's size).
    pub fn push(&mut self, img: &RgbaImage) -> io::Result<()> {
        if img.width != self.w || img.height != self.h {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "frame size differs from the movie"));
        }
        let n = self.sizes.len() as u32;
        let (data, key) = match self.opts.codec {
            MovCodec::Png => (crate::png::encode(img), true),
            MovCodec::Rle => {
                let key = self.prev.is_none() || (self.opts.gop > 0 && n % self.opts.gop == 0);
                let prev = if key { None } else { self.prev.as_ref() };
                let d = rle_encode(img, prev);
                self.prev = Some(img.clone());
                (d, key)
            }
        };
        self.file.write_all(&data)?;
        self.offsets.push(self.pos);
        self.sizes.push(data.len() as u32);
        if key {
            self.keys.push(n + 1);
        }
        self.pos += data.len() as u64;
        Ok(())
    }

    /// Writes the index and closes the file.
    pub fn finish(mut self) -> io::Result<PathBuf> {
        let mdat_size = self.pos - self.mdat_start;
        if !self.opts.fast_start {
            let moov = self.moov(0);
            self.file.write_all(&moov)?;
            let mut f = self.file.into_inner().map_err(|e| e.into_error())?;
            f.seek(SeekFrom::Start(self.mdat_start + 8))?;
            f.write_all(&mdat_size.to_be_bytes())?;
            f.sync_all()?;
            return Ok(self.path);
        }
        // Fast start: ftyp + moov + mdat, offsets shifted by the moov size.
        // The moov size can change when offsets need 64 bits; settle it.
        let mut moov = self.moov(0);
        loop {
            let next = self.moov(moov.len() as u64);
            if next.len() == moov.len() {
                moov = next;
                break;
            }
            moov = next;
        }
        let mut f = self.file.into_inner().map_err(|e| e.into_error())?;
        f.seek(SeekFrom::Start(self.mdat_start + 8))?;
        f.write_all(&mdat_size.to_be_bytes())?;
        f.flush()?;
        drop(f);
        let tmp = self.path.with_extension("mov.part");
        {
            let mut src = File::open(&self.path)?;
            let mut out = BufWriter::new(File::create(&tmp)?);
            let mut head = vec![0u8; self.mdat_start as usize];
            src.read_exact(&mut head)?;
            out.write_all(&head)?;
            out.write_all(&moov)?;
            io::copy(&mut src.take(mdat_size), &mut out)?;
            out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        }
        std::fs::rename(&tmp, &self.path)?;
        Ok(self.path)
    }

    /// Removes the partial file.
    pub fn abandon(self) {
        let path = self.path.clone();
        drop(self.file);
        let _ = std::fs::remove_file(path);
    }

    fn moov(&self, shift: u64) -> Vec<u8> {
        let timescale = self.opts.fps_num;
        let delta = self.opts.fps_den;
        let n = self.sizes.len() as u32;
        let duration = (n as u64 * delta as u64).min(u32::MAX as u64) as u32;
        let (w, h) = (self.w, self.h);
        let mvhd = atom(
            b"mvhd",
            &cat(&[
                &[0, 0, 0, 0],
                &be32(0),
                &be32(0),
                &be32(timescale),
                &be32(duration),
                &be32(0x0001_0000), // rate 1.0
                &be16(0x0100),      // volume 1.0
                &[0u8; 10],
                &matrix(),
                &[0u8; 24], // preview / poster / selection / current time
                &be32(2),   // next track id
            ]),
        );
        let tkhd = atom(
            b"tkhd",
            &cat(&[
                &[0, 0, 0, 0x0f], // enabled, in movie, preview, poster
                &be32(0),
                &be32(0),
                &be32(1),
                &be32(0),
                &be32(duration),
                &[0u8; 8],
                &be16(0), // layer
                &be16(0), // alternate group
                &be16(0), // volume (video)
                &be16(0),
                &matrix(),
                &be32(w << 16),
                &be32(h << 16),
            ]),
        );
        let mdhd = atom(b"mdhd", &cat(&[&[0, 0, 0, 0], &be32(0), &be32(0), &be32(timescale), &be32(duration), &be16(0x7fff), &be16(0)]));
        let mhlr = atom(b"hdlr", &cat(&[&[0, 0, 0, 0], b"mhlr", b"vide", b"appl", &be32(0), &be32(0), &[11], b"VideoHandle"]));
        let vmhd = atom(b"vmhd", &cat(&[&[0, 0, 0, 1], &be16(0x40), &be16(0x8000), &be16(0x8000), &be16(0x8000)]));
        let dhlr = atom(b"hdlr", &cat(&[&[0, 0, 0, 0], b"dhlr", b"alis", b"appl", &be32(0), &be32(0), &[10], b"DataHandle"]));
        let alis = atom(b"alis", &[0, 0, 0, 1]); // self reference
        let dinf = atom(b"dinf", &atom(b"dref", &cat(&[&[0, 0, 0, 0], &be32(1), &alis])));
        let mut cname = [0u8; 32];
        let name = self.opts.codec.name().as_bytes();
        cname[0] = name.len() as u8;
        cname[1..1 + name.len()].copy_from_slice(name);
        let entry_body = cat(&[
            &[0u8; 6],
            &be16(1), // data reference index
            &be16(0), // version
            &be16(0), // revision
            b"appl",
            &be32(0),    // temporal quality
            &be32(1023), // spatial quality (lossless)
            &be16(w as u16),
            &be16(h as u16),
            &be32(72 << 16),
            &be32(72 << 16),
            &be32(0),
            &be16(1), // frames per sample
            &cname,
            &be16(32),     // depth: 32 = millions of colours + alpha
            &be16(0xffff), // no colour table
        ]);
        let stsd = atom(b"stsd", &cat(&[&[0, 0, 0, 0], &be32(1), &atom(self.opts.codec.fourcc(), &entry_body)]));
        let stts = atom(b"stts", &cat(&[&[0, 0, 0, 0], &be32(1), &be32(n), &be32(delta)]));
        let all_keys = self.keys.len() == self.sizes.len();
        let stss = (!all_keys).then(|| {
            let mut b = cat(&[&[0, 0, 0, 0], &be32(self.keys.len() as u32)]);
            for k in &self.keys {
                b.extend_from_slice(&be32(*k));
            }
            atom(b"stss", &b)
        });
        let stsc = atom(b"stsc", &cat(&[&[0, 0, 0, 0], &be32(1), &be32(1), &be32(1), &be32(1)]));
        let mut sz = cat(&[&[0, 0, 0, 0], &be32(0), &be32(n)]);
        for s in &self.sizes {
            sz.extend_from_slice(&be32(*s));
        }
        let stsz = atom(b"stsz", &sz);
        let wide = self.offsets.last().is_some_and(|&o| o + shift > u32::MAX as u64);
        let mut co = cat(&[&[0, 0, 0, 0], &be32(n)]);
        for o in &self.offsets {
            if wide {
                co.extend_from_slice(&(o + shift).to_be_bytes());
            } else {
                co.extend_from_slice(&be32((o + shift) as u32));
            }
        }
        let stco = atom(if wide { b"co64" } else { b"stco" }, &co);
        let mut stbl_body = cat(&[&stsd, &stts]);
        if let Some(s) = &stss {
            stbl_body.extend_from_slice(s);
        }
        stbl_body.extend_from_slice(&cat(&[&stsc, &stsz, &stco]));
        let stbl = atom(b"stbl", &stbl_body);
        let minf = atom(b"minf", &cat(&[&vmhd, &dhlr, &dinf, &stbl]));
        let mdia = atom(b"mdia", &cat(&[&mdhd, &mhlr, &minf]));
        let trak = atom(b"trak", &cat(&[&tkhd, &mdia]));
        atom(b"moov", &cat(&[&mvhd, &trak]))
    }
}

// ---------------------------------------------------------------------------
// Animation codec

fn argb(img: &RgbaImage, x: usize, y: usize) -> [u8; 4] {
    let i = (y * img.width as usize + x) * 4;
    let p = &img.pixels[i..i + 4];
    [p[3], p[0], p[1], p[2]]
}

/// Encodes one 32-bit Animation frame; `prev` = the previous frame for an
/// inter frame (unchanged lines and leading pixels are skipped).
pub fn rle_encode(img: &RgbaImage, prev: Option<&RgbaImage>) -> Vec<u8> {
    let (w, h) = (img.width as usize, img.height as usize);
    fn row(im: &RgbaImage, y: usize, w: usize) -> &[u8] {
        &im.pixels[y * w * 4..(y + 1) * w * 4]
    }
    let same = |y: usize| prev.is_some_and(|p| row(p, y, w) == row(img, y, w));
    let first = (0..h).find(|&y| !same(y));
    let mut out = vec![0u8; 4];
    let Some(first) = first else {
        // Nothing changed: a bare chunk size means "repeat the last frame".
        out.copy_from_slice(&be32(4));
        return out;
    };
    let last = (0..h).rev().find(|&y| !same(y)).unwrap_or(first);
    if prev.is_some() || first > 0 || last + 1 < h {
        out.extend_from_slice(&be16(0x0008));
        out.extend_from_slice(&be16(first as u16));
        out.extend_from_slice(&be16(0));
        out.extend_from_slice(&be16((last - first + 1) as u16));
        out.extend_from_slice(&be16(0));
    } else {
        out.extend_from_slice(&be16(0));
    }
    for y in first..=last {
        // Leading unchanged pixels become skip codes.
        let mut x = 0usize;
        if let Some(p) = prev {
            while x < w && argb(p, x, y) == argb(img, x, y) {
                x += 1;
            }
        }
        let mut skip = x + 1; // a skip byte of n means "skip n − 1 pixels"
        out.push(skip.min(255) as u8);
        skip -= skip.min(255);
        while skip > 0 {
            // Further skips: code 0, then another skip byte.
            let s = skip.min(254);
            out.push(0);
            out.push((s + 1) as u8);
            skip -= s;
        }
        while x < w {
            let px = argb(img, x, y);
            let mut run = 1;
            while x + run < w && run < 128 && argb(img, x + run, y) == px {
                run += 1;
            }
            if run >= 2 {
                out.push((-(run as i32)) as i8 as u8);
                out.extend_from_slice(&px);
                x += run;
                continue;
            }
            // Literal pixels until a run of 2+ starts (max 127).
            let start = x;
            let mut n = 0;
            while x < w && n < 127 {
                if x + 1 < w && argb(img, x + 1, y) == argb(img, x, y) {
                    break;
                }
                x += 1;
                n += 1;
            }
            if n == 0 {
                // A run starts right here; handled by the next iteration.
                continue;
            }
            out.push(n as u8);
            for k in start..start + n {
                out.extend_from_slice(&argb(img, k, y));
            }
        }
        out.push(0xff); // −1: end of line
    }
    let len = out.len() as u32;
    out[..4].copy_from_slice(&be32(len));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (type, body offset, body len) of the atoms in `buf[from..to]`.
    fn atoms(buf: &[u8], from: usize, to: usize) -> Vec<([u8; 4], usize, usize)> {
        let mut out = Vec::new();
        let mut at = from;
        while at + 8 <= to {
            let mut size = u32::from_be_bytes(buf[at..at + 4].try_into().unwrap()) as usize;
            let kind: [u8; 4] = buf[at + 4..at + 8].try_into().unwrap();
            let mut head = 8;
            if size == 1 {
                size = u64::from_be_bytes(buf[at + 8..at + 16].try_into().unwrap()) as usize;
                head = 16;
            }
            assert!(size >= head && at + size <= to, "atom {:?} overruns", std::str::from_utf8(&kind));
            out.push((kind, at + head, size - head));
            at += size;
        }
        assert_eq!(at, to, "atoms fill their parent exactly");
        out
    }

    fn child<'a>(list: &'a [([u8; 4], usize, usize)], kind: &[u8; 4]) -> &'a ([u8; 4], usize, usize) {
        list.iter().find(|a| &a.0 == kind).unwrap_or_else(|| panic!("no {}", std::str::from_utf8(kind).unwrap()))
    }

    fn walk<'a>(buf: &[u8], parent: &([u8; 4], usize, usize), path: &[&[u8; 4]]) -> ([u8; 4], usize, usize) {
        let mut cur = *parent;
        for k in path {
            let skip = match &cur.0 {
                b"dref" | b"stsd" => 8,
                _ => 0,
            };
            let kids = atoms(buf, cur.1 + skip, cur.1 + cur.2);
            cur = *child(&kids, k);
        }
        cur
    }

    fn u32_at(b: &[u8], at: usize) -> u32 {
        u32::from_be_bytes(b[at..at + 4].try_into().unwrap())
    }

    /// A test-only Animation decoder (32-bit), onto `canvas` (RGBA).
    fn rle_decode(data: &[u8], canvas: &mut RgbaImage) {
        let w = canvas.width as usize;
        let size = u32_at(data, 0) as usize;
        assert_eq!(size, data.len());
        if size < 8 {
            return;
        }
        let header = u16::from_be_bytes([data[4], data[5]]);
        let mut at = 6;
        let (start, lines) = if header & 8 != 0 {
            let s = u16::from_be_bytes([data[at], data[at + 1]]) as usize;
            let n = u16::from_be_bytes([data[at + 4], data[at + 5]]) as usize;
            at += 8;
            (s, n)
        } else {
            (0, canvas.height as usize)
        };
        for y in start..start + lines {
            let mut x = data[at] as usize - 1;
            at += 1;
            loop {
                let code = data[at] as i8;
                at += 1;
                match code {
                    -1 => break,
                    0 => {
                        x += data[at] as usize - 1;
                        at += 1;
                    }
                    n if n < 0 => {
                        let px = &data[at..at + 4];
                        at += 4;
                        for _ in 0..(-(n as i32)) {
                            let i = (y * w + x) * 4;
                            canvas.pixels[i..i + 4].copy_from_slice(&[px[1], px[2], px[3], px[0]]);
                            x += 1;
                        }
                    }
                    n => {
                        for _ in 0..n {
                            let px = &data[at..at + 4];
                            at += 4;
                            let i = (y * w + x) * 4;
                            canvas.pixels[i..i + 4].copy_from_slice(&[px[1], px[2], px[3], px[0]]);
                            x += 1;
                        }
                    }
                }
                assert!(x <= w);
            }
        }
        assert_eq!(at, data.len());
    }

    fn frame(w: u32, h: u32, k: u32) -> RgbaImage {
        let mut img = RgbaImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                // Runs, literals, transparency and a moving square.
                let sq = x >= k * 3 && x < k * 3 + 6 && (4..10).contains(&y);
                let edge = x == w - 1;
                let p = if edge { [(k * 40) as u8, 0, 0, 255] } else if sq { [255, 0, 0, 255] } else if x < w / 2 { [0, 0, 0, 0] } else { [(x * 7 % 256) as u8, (y * 13 % 256) as u8, 40, (x * 3 % 256) as u8] };
                img.pixels[i..i + 4].copy_from_slice(&p);
            }
        }
        img
    }

    #[test]
    fn rle_round_trips_key_and_inter_frames() {
        // Wide enough for long runs (>128) and long skips (>254).
        let (w, h) = (300, 16);
        let mut canvas = RgbaImage::new(w, h);
        let mut prev: Option<RgbaImage> = None;
        for k in 0..6 {
            let img = frame(w, h, k);
            let data = rle_encode(&img, prev.as_ref());
            rle_decode(&data, &mut canvas);
            assert_eq!(canvas.pixels, img.pixels, "frame {k}");
            prev = Some(img);
        }
        // An unchanged frame is a bare 4-byte chunk.
        let same = rle_encode(prev.as_ref().unwrap(), prev.as_ref());
        assert_eq!(same, vec![0, 0, 0, 4]);
        // A uniform key frame compresses to runs.
        let flat = RgbaImage::new(w, h);
        assert_eq!(rle_encode(&flat, None).len(), 6 + 16 * 17);
    }

    fn write_movie(codec: MovCodec, fast: bool) -> (Vec<u8>, Vec<RgbaImage>) {
        let dir = std::env::temp_dir().join(format!("studio-mov-{}-{:?}-{fast}", std::process::id(), codec));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("alpha.mov");
        let opts = MovOptions { codec, fps_num: 30000, fps_den: 1001, gop: 3, fast_start: fast };
        let mut m = MovWriter::create(&path, 40, 16, opts).unwrap();
        let frames: Vec<RgbaImage> = (0..5).map(|k| frame(40, 16, k)).collect();
        for f in &frames {
            m.push(f).unwrap();
        }
        let p = m.finish().unwrap();
        let bytes = std::fs::read(&p).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        (bytes, frames)
    }

    #[test]
    fn movie_atoms_parse_and_samples_decode() {
        for codec in [MovCodec::Rle, MovCodec::Png] {
            for fast in [false, true] {
                let (buf, frames) = write_movie(codec, fast);
                let top = atoms(&buf, 0, buf.len());
                let kinds: Vec<&[u8; 4]> = top.iter().map(|a| &a.0).collect();
                if fast {
                    assert_eq!(kinds, vec![b"ftyp", b"moov", b"mdat"]);
                } else {
                    assert_eq!(kinds, vec![b"ftyp", b"mdat", b"moov"]);
                }
                assert_eq!(&buf[child(&top, b"ftyp").1..][..4], b"qt  ");
                let moov = *child(&top, b"moov");
                let mvhd = walk(&buf, &moov, &[b"mvhd"]);
                assert_eq!(u32_at(&buf, mvhd.1 + 12), 30000, "timescale");
                assert_eq!(u32_at(&buf, mvhd.1 + 16), 5 * 1001, "duration");
                let tkhd = walk(&buf, &moov, &[b"trak", b"tkhd"]);
                assert_eq!(u32_at(&buf, tkhd.1 + tkhd.2 - 8), 40 << 16);
                let stbl = walk(&buf, &moov, &[b"trak", b"mdia", b"minf", b"stbl"]);
                let entry = walk(&buf, &stbl, &[b"stsd", codec.fourcc()]);
                assert_eq!(u16::from_be_bytes([buf[entry.1 + 24], buf[entry.1 + 25]]), 40);
                assert_eq!(u16::from_be_bytes([buf[entry.1 + 74], buf[entry.1 + 75]]), 32, "depth");
                let _ = walk(&buf, &moov, &[b"trak", b"mdia", b"minf", b"dinf", b"dref", b"alis"]);
                let stsz = walk(&buf, &stbl, &[b"stsz"]);
                let stco = walk(&buf, &stbl, &[b"stco"]);
                assert_eq!(u32_at(&buf, stsz.1 + 8), 5);
                assert_eq!(u32_at(&buf, stco.1 + 4), 5);
                let has_stss = atoms(&buf, stbl.1, stbl.1 + stbl.2).iter().any(|a| &a.0 == b"stss");
                assert_eq!(has_stss, codec == MovCodec::Rle, "RLE inter frames need a sync table");
                let mdat = *child(&top, b"mdat");
                let mut canvas = RgbaImage::new(40, 16);
                for (k, f) in frames.iter().enumerate() {
                    let size = u32_at(&buf, stsz.1 + 12 + 4 * k) as usize;
                    let off = u32_at(&buf, stco.1 + 8 + 4 * k) as usize;
                    assert!(off >= mdat.1 && off + size <= mdat.1 + mdat.2, "sample inside mdat");
                    let sample = &buf[off..off + size];
                    match codec {
                        MovCodec::Rle => {
                            rle_decode(sample, &mut canvas);
                            assert_eq!(canvas.pixels, f.pixels, "frame {k}");
                        }
                        MovCodec::Png => assert_eq!(crate::png::decode(sample).unwrap().pixels, f.pixels),
                    }
                }
            }
        }
    }
}
