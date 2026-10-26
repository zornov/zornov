// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A writer for MXF OP-Atom track files (SMPTE ST 390 / ST 377-1), the
//! container of digital cinema picture and sound track files (ST 429-3)
//! and of IMF essence (ST 2067-5):
//!
//! - one essence per file, frame wrapped: JPEG 2000 pictures (ST 422),
//!   DNxHR / VC-3 pictures (ST 2019-4: each coding unit its own element, CDCI
//!   descriptor with the compression label of the compression ID) or 24-bit
//!   PCM sound (ST 382);
//! - header partition (reserved space, rewritten when the file is
//!   finished), body partition with the essence, footer partition with the
//!   index table (variable frame sizes), random index pack;
//! - a material package and a file package with timecode and essence
//!   tracks, a JPEG 2000 picture sub-descriptor holding the codestream's
//!   SIZ, COD and QCD values.
//!
//! The metadata layout (sets, tags and their universal labels) follows the
//! standards' registers; where a label could be confirmed against an
//! independent writer (FFmpeg's MXF muxer) it was. Files read back with
//! FFmpeg's demuxer in the tests. They have not been through a cinema
//! server or a certified MXF analyser.

use crate::sha256;
use std::io::{self, Seek, SeekFrom, Write};
use std::path::Path;

/// 16 pseudo-random bytes, unique per call (time, process, counter).
pub fn uuid() -> [u8; 16] {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let mut h = sha256::Sha256::new();
    h.update(b"studio-uuid");
    h.update(&std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()).to_be_bytes());
    h.update(&std::process::id().to_be_bytes());
    h.update(&N.fetch_add(1, Ordering::Relaxed).to_be_bytes());
    let d = h.finish();
    let mut u = [0u8; 16];
    u.copy_from_slice(&d[..16]);
    // Version 4, variant 1 bits so that it reads as a UUID.
    u[6] = (u[6] & 0x0f) | 0x40;
    u[8] = (u[8] & 0x3f) | 0x80;
    u
}

pub fn uuid_string(u: &[u8; 16]) -> String {
    let h = sha256::hex(u);
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

fn ul(s: &str) -> [u8; 16] {
    let mut out = [0u8; 16];
    let hex: Vec<u8> = s.bytes().filter(|b| b.is_ascii_hexdigit()).collect();
    assert_eq!(hex.len(), 32, "{s}");
    for i in 0..16 {
        out[i] = u8::from_str_radix(std::str::from_utf8(&hex[2 * i..2 * i + 2]).unwrap(), 16).unwrap();
    }
    out
}

// Keys.
const K_HEADER: &str = "060e2b34 02050101 0d010201 01020400";
const K_BODY: &str = "060e2b34 02050101 0d010201 01030400";
const K_FOOTER: &str = "060e2b34 02050101 0d010201 01040400";
const K_RIP: &str = "060e2b34 02050101 0d010201 01110100";
const K_PRIMER: &str = "060e2b34 02050101 0d010201 01050100";
const K_FILL: &str = "060e2b34 01010102 03010210 01000000";
const K_INDEX: &str = "060e2b34 02530101 0d010201 01100100";
const OP_ATOM: &str = "060e2b34 04010102 0d010201 10030000";
const EC_JP2K: &str = "060e2b34 04010107 0d010301 020c0100";
const EC_DNX: &str = "060e2b34 0401010a 0d010301 02110100";
const EC_PCM: &str = "060e2b34 04010101 0d010301 02060100";
const E_JP2K: &str = "060e2b34 01020101 0d010301 15010800";
const E_DNX: &str = "060e2b34 01020101 0d010301 150c0100";
const E_PCM: &str = "060e2b34 01020101 0d010301 16010100";
const SET_PREFACE: &str = "060e2b34 02530101 0d010101 01012f00";
const SET_IDENT: &str = "060e2b34 02530101 0d010101 01013000";
const SET_CONTENT: &str = "060e2b34 02530101 0d010101 01011800";
const SET_ECD: &str = "060e2b34 02530101 0d010101 01012300";
const SET_MATERIAL: &str = "060e2b34 02530101 0d010101 01013600";
const SET_SOURCE: &str = "060e2b34 02530101 0d010101 01013700";
const SET_TRACK: &str = "060e2b34 02530101 0d010101 01013b00";
const SET_SEQUENCE: &str = "060e2b34 02530101 0d010101 01010f00";
const SET_CLIP: &str = "060e2b34 02530101 0d010101 01011100";
const SET_TIMECODE: &str = "060e2b34 02530101 0d010101 01011400";
const SET_CDCI: &str = "060e2b34 02530101 0d010101 01012800";
const SET_WAVE: &str = "060e2b34 02530101 0d010101 01014800";
const SET_J2K: &str = "060e2b34 02530101 0d010101 01015a00";
const DD_PICTURE: &str = "060e2b34 04010101 01030202 01000000";
const DD_SOUND: &str = "060e2b34 04010101 01030202 02000000";
const DD_TIMECODE: &str = "060e2b34 04010101 01030201 01000000";
const PIC_CODING_J2K: &str = "060e2b34 04010107 04010202 03010100";

/// (local tag, label) for every tag the writer uses.
const PRIMER: &[(u16, &str)] = &[
    (0x3c0a, "060e2b34 01010101 01011502 00000000"),
    (0x3b02, "060e2b34 01010102 07020110 02040000"),
    (0x3b05, "060e2b34 01010102 03010201 05000000"),
    (0x3b07, "060e2b34 01010102 03010201 04000000"),
    (0x3b06, "060e2b34 01010102 06010104 06040000"),
    (0x3b03, "060e2b34 01010102 06010104 02010000"),
    (0x3b09, "060e2b34 01010105 01020203 00000000"),
    (0x3b0a, "060e2b34 01010105 01020210 02010000"),
    (0x3b0b, "060e2b34 01010105 01020210 02020000"),
    (0x3c09, "060e2b34 01010102 05200701 01000000"),
    (0x3c01, "060e2b34 01010102 05200701 02010000"),
    (0x3c02, "060e2b34 01010102 05200701 03010000"),
    (0x3c03, "060e2b34 01010102 05200701 04000000"),
    (0x3c04, "060e2b34 01010102 05200701 05010000"),
    (0x3c05, "060e2b34 01010102 05200701 07000000"),
    (0x3c06, "060e2b34 01010102 07020110 02030000"),
    (0x3c07, "060e2b34 01010102 05200701 0a000000"),
    (0x3c08, "060e2b34 01010102 05200701 06010000"),
    (0x1901, "060e2b34 01010102 06010104 05010000"),
    (0x1902, "060e2b34 01010102 06010104 05020000"),
    (0x2701, "060e2b34 01010102 06010106 01000000"),
    (0x3f07, "060e2b34 01010104 01030404 00000000"),
    (0x3f06, "060e2b34 01010104 01030404 05000000"),
    (0x4401, "060e2b34 01010101 01011510 00000000"),
    (0x4405, "060e2b34 01010102 07020110 01030000"),
    (0x4404, "060e2b34 01010102 07020110 02050000"),
    (0x4403, "060e2b34 01010102 06010104 06050000"),
    (0x4701, "060e2b34 01010102 06010104 02030000"),
    (0x4801, "060e2b34 01010102 01070101 00000000"),
    (0x4804, "060e2b34 01010102 01040103 00000000"),
    (0x4b01, "060e2b34 01010102 05300405 00000000"),
    (0x4b02, "060e2b34 01010102 07020103 01030000"),
    (0x4803, "060e2b34 01010102 06010104 02040000"),
    (0x0201, "060e2b34 01010102 04070100 00000000"),
    (0x0202, "060e2b34 01010102 07020201 01030000"),
    (0x1001, "060e2b34 01010102 06010104 06090000"),
    (0x1501, "060e2b34 01010102 07020103 01050000"),
    (0x1502, "060e2b34 01010102 04040101 02060000"),
    (0x1503, "060e2b34 01010101 04040101 05000000"),
    (0x1201, "060e2b34 01010102 07020103 01040000"),
    (0x1101, "060e2b34 01010102 06010103 01000000"),
    (0x1102, "060e2b34 01010102 06010103 02000000"),
    (0x3006, "060e2b34 01010105 06010103 05000000"),
    (0x3001, "060e2b34 01010101 04060101 00000000"),
    (0x3002, "060e2b34 01010101 04060102 00000000"),
    (0x3004, "060e2b34 01010102 06010104 01020000"),
    (0x320c, "060e2b34 01010101 04010301 04000000"),
    (0x320d, "060e2b34 01010102 04010302 05000000"),
    (0x3203, "060e2b34 01010101 04010502 02000000"),
    (0x3202, "060e2b34 01010101 04010502 01000000"),
    (0x3205, "060e2b34 01010101 04010501 08000000"),
    (0x3204, "060e2b34 01010101 04010501 07000000"),
    (0x3206, "060e2b34 01010101 04010501 09000000"),
    (0x3207, "060e2b34 01010101 04010501 0a000000"),
    (0x3209, "060e2b34 01010101 04010501 0c000000"),
    (0x3208, "060e2b34 01010101 04010501 0b000000"),
    (0x320a, "060e2b34 01010101 04010501 0d000000"),
    (0x320b, "060e2b34 01010101 04010501 0e000000"),
    (0x320e, "060e2b34 01010101 04010101 01000000"),
    (0x3201, "060e2b34 01010102 04010601 00000000"),
    (0x3301, "060e2b34 01010102 0401050 3 0a000000"),
    (0x3302, "060e2b34 01010101 04010501 05000000"),
    (0x3308, "060e2b34 01010102 04010501 10000000"),
    (0x3303, "060e2b34 01010101 04010501 06000000"),
    (0x3307, "060e2b34 01010102 04180104 00000000"),
    (0x3304, "060e2b34 01010101 04010503 03000000"),
    (0x3305, "060e2b34 01010101 04010503 04000000"),
    (0x3306, "060e2b34 01010102 04010503 05000000"),
    (0x8100, "060e2b34 01010109 06010104 06100000"),
    (0x8401, "060e2b34 0101010a 04010603 01000000"),
    (0x8402, "060e2b34 0101010a 04010603 02000000"),
    (0x8403, "060e2b34 0101010a 04010603 03000000"),
    (0x8404, "060e2b34 0101010a 04010603 04000000"),
    (0x8405, "060e2b34 0101010a 04010603 05000000"),
    (0x8406, "060e2b34 0101010a 04010603 06000000"),
    (0x8407, "060e2b34 0101010a 04010603 07000000"),
    (0x8408, "060e2b34 0101010a 04010603 08000000"),
    (0x8409, "060e2b34 0101010a 04010603 09000000"),
    (0x840a, "060e2b34 0101010a 04010603 0a000000"),
    (0x840b, "060e2b34 0101010a 04010603 0b000000"),
    (0x840c, "060e2b34 0101010a 04010603 0c000000"),
    (0x840d, "060e2b34 0101010a 04010603 0d000000"),
    (0x3d02, "060e2b34 01010104 04020301 04000000"),
    (0x3d03, "060e2b34 01010105 04020301 01010000"),
    (0x3d07, "060e2b34 01010105 04020101 04000000"),
    (0x3d01, "060e2b34 01010104 04020303 04000000"),
    (0x3d0a, "060e2b34 01010105 04020302 01000000"),
    (0x3d09, "060e2b34 01010105 04020303 05000000"),
];

struct Set {
    key: [u8; 16],
    body: Vec<u8>,
}

impl Set {
    fn new(key: &str, instance: &[u8; 16]) -> Set {
        let mut s = Set { key: ul(key), body: Vec::new() };
        s.raw(0x3c0a, instance);
        s
    }

    fn raw(&mut self, tag: u16, data: &[u8]) -> &mut Set {
        self.body.extend_from_slice(&tag.to_be_bytes());
        self.body.extend_from_slice(&(data.len() as u16).to_be_bytes());
        self.body.extend_from_slice(data);
        self
    }

    fn u8(&mut self, tag: u16, v: u8) -> &mut Set {
        self.raw(tag, &[v])
    }

    fn u16(&mut self, tag: u16, v: u16) -> &mut Set {
        self.raw(tag, &v.to_be_bytes())
    }

    fn u32(&mut self, tag: u16, v: u32) -> &mut Set {
        self.raw(tag, &v.to_be_bytes())
    }

    fn i64(&mut self, tag: u16, v: i64) -> &mut Set {
        self.raw(tag, &v.to_be_bytes())
    }

    fn rational(&mut self, tag: u16, n: u32, d: u32) -> &mut Set {
        let mut b = [0u8; 8];
        b[..4].copy_from_slice(&n.to_be_bytes());
        b[4..].copy_from_slice(&d.to_be_bytes());
        self.raw(tag, &b)
    }

    fn ts(&mut self, tag: u16, t: &[u8; 8]) -> &mut Set {
        self.raw(tag, t)
    }

    fn utf16(&mut self, tag: u16, s: &str) -> &mut Set {
        let mut b: Vec<u8> = s.encode_utf16().flat_map(|c| c.to_be_bytes()).collect();
        b.extend_from_slice(&[0, 0]);
        self.raw(tag, &b)
    }

    fn batch(&mut self, tag: u16, items: &[Vec<u8>]) -> &mut Set {
        let size = items.first().map_or(0, |i| i.len());
        let mut b = Vec::new();
        b.extend_from_slice(&(items.len() as u32).to_be_bytes());
        b.extend_from_slice(&(size as u32).to_be_bytes());
        for i in items {
            b.extend_from_slice(i);
        }
        self.raw(tag, &b)
    }

    fn klv(&self) -> Vec<u8> {
        let mut out = self.key.to_vec();
        out.extend(ber4(self.body.len()));
        out.extend_from_slice(&self.body);
        out
    }
}

fn ber4(n: usize) -> Vec<u8> {
    vec![0x83, (n >> 16) as u8, (n >> 8) as u8, n as u8]
}

fn klv_ber(key: &[u8; 16], len: usize) -> Vec<u8> {
    let mut out = key.to_vec();
    if len < 1 << 24 {
        out.extend(ber4(len));
    } else {
        out.extend_from_slice(&[0x84, (len >> 24) as u8, (len >> 16) as u8, (len >> 8) as u8, len as u8]);
    }
    out
}

/// An 8-byte MXF timestamp of now.
fn timestamp_now() -> [u8; 8] {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // Civil from days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let mut t = [0u8; 8];
    t[..2].copy_from_slice(&(y as u16).to_be_bytes());
    t[2] = m as u8;
    t[3] = d as u8;
    t[4] = (rem / 3600) as u8;
    t[5] = (rem % 3600 / 60) as u8;
    t[6] = (rem % 60) as u8;
    t
}

/// A package id: the UMID label, then 3 instance bytes and a material number.
fn umid(material: &[u8; 16]) -> Vec<u8> {
    let mut u = ul("060a2b34 01010105 01010d00 13000000").to_vec();
    u.truncate(13);
    u.extend_from_slice(&[0, 0, 0]);
    u.extend_from_slice(material);
    u
}

/// What the essence is.
#[derive(Clone, Debug)]
pub enum Essence {
    Jpeg2000(J2kDesc),
    Dnxhr(DnxDesc),
    Pcm { rate: u32, channels: u16, bits: u16 },
}

/// A DNxHR (VC-3) picture stream, read from the header of a coding unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DnxDesc {
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    /// SMPTE ST 2019-1 compression ID (1270 to 1274).
    pub cid: u32,
    pub yuv444: bool,
}

impl DnxDesc {
    /// Reads the fields the descriptor needs from a compressed frame (ST 2019-1 section 7.2).
    pub fn from_frame(f: &[u8]) -> Result<DnxDesc, String> {
        if f.len() < 0x2d || f[0..3] != [0, 0, 2] || f[4] != 3 {
            return Err("not a DNxHR frame".into());
        }
        let cid = u32::from_be_bytes([f[0x28], f[0x29], f[0x2a], f[0x2b]]);
        if !(1270..=1274).contains(&cid) {
            return Err(format!("compression ID {cid} is not DNxHR"));
        }
        let bit_depth = match f[0x21] >> 5 {
            1 => 8,
            2 => 10,
            3 => 12,
            _ => return Err("bad sample bit depth".into()),
        };
        Ok(DnxDesc { width: u16::from_be_bytes([f[0x1a], f[0x1b]]) as u32, height: u16::from_be_bytes([f[0x18], f[0x19]]) as u32, bit_depth, cid, yuv444: f[0x2c] >> 5 & 3 == 2 })
    }

    /// Picture Essence Coding label (ST 2019-4 Tables 3 and 4).
    fn coding_label(&self) -> String {
        format!("060e2b34 0401010a 04010202 71{:02x}0000", 0x24 + (self.cid - 1270))
    }
}

#[derive(Clone, Debug)]
pub struct J2kDesc {
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    pub components: u8,
    pub rsiz: u16,
    /// COD and QCD segment bodies (after the marker and length) copied from the codestream.
    pub cod: Vec<u8>,
    pub qcd: Vec<u8>,
}

impl J2kDesc {
    /// Read SIZ, COD and QCD from a codestream.
    pub fn from_codestream(cs: &[u8]) -> Result<J2kDesc, String> {
        if cs.len() < 4 || cs[0] != 0xff || cs[1] != 0x4f {
            return Err("not a JPEG 2000 codestream".into());
        }
        let (mut p, mut siz, mut cod, mut qcd) = (2usize, None, None, None);
        while p + 4 <= cs.len() && cs[p] == 0xff {
            let m = cs[p + 1];
            if m == 0x90 || m == 0x93 {
                break;
            }
            let l = u16::from_be_bytes([cs[p + 2], cs[p + 3]]) as usize;
            let body = cs.get(p + 4..p + 2 + l).ok_or("truncated codestream header")?;
            match m {
                0x51 => siz = Some(body.to_vec()),
                0x52 => cod = Some(body.to_vec()),
                0x5c => qcd = Some(body.to_vec()),
                _ => {}
            }
            p += 2 + l;
        }
        let siz = siz.ok_or("no SIZ")?;
        if siz.len() < 36 {
            return Err("short SIZ".into());
        }
        let u32at = |i: usize| u32::from_be_bytes([siz[i], siz[i + 1], siz[i + 2], siz[i + 3]]);
        Ok(J2kDesc { width: u32at(2), height: u32at(6), components: siz[35], bit_depth: (siz[36] & 0x7f) + 1, rsiz: u16::from_be_bytes([siz[0], siz[1]]), cod: cod.ok_or("no COD")?, qcd: qcd.ok_or("no QCD")? })
    }
}

/// What a finished file holds.
#[derive(Clone, Debug)]
pub struct MxfInfo {
    pub frames: u64,
    pub bytes: u64,
    /// The file package's UMID material number, as a UUID (for the CPL / PKL).
    pub id: [u8; 16],
}

pub struct MxfWriter {
    file: io::BufWriter<std::fs::File>,
    essence: Essence,
    rate: (u32, u32),
    id: [u8; 16],
    ids: Ids,
    /// Where the body partition starts, and how much essence has been written.
    body_at: u64,
    sizes: Vec<u64>,
    stream: u64,
    key: [u8; 16],
    title: String,
}

struct Ids {
    preface: [u8; 16],
    ident: [u8; 16],
    content: [u8; 16],
    ecd: [u8; 16],
    mat_pkg: [u8; 16],
    src_pkg: [u8; 16],
    m_tc_track: [u8; 16],
    m_tc_seq: [u8; 16],
    m_tc_comp: [u8; 16],
    m_track: [u8; 16],
    m_seq: [u8; 16],
    m_clip: [u8; 16],
    s_tc_track: [u8; 16],
    s_tc_seq: [u8; 16],
    s_tc_comp: [u8; 16],
    s_track: [u8; 16],
    s_seq: [u8; 16],
    s_clip: [u8; 16],
    desc: [u8; 16],
    sub: [u8; 16],
    mat_id: [u8; 16],
    file_id: [u8; 16],
}

const RESERVE: u64 = 64 * 1024;

impl MxfWriter {
    /// A track file for `essence` at `rate` (edit units per second).
    pub fn create(path: &Path, essence: Essence, rate: (u32, u32), title: &str) -> io::Result<MxfWriter> {
        let file = io::BufWriter::with_capacity(1 << 20, std::fs::File::create(path)?);
        let ids = Ids { preface: uuid(), ident: uuid(), content: uuid(), ecd: uuid(), mat_pkg: uuid(), src_pkg: uuid(), m_tc_track: uuid(), m_tc_seq: uuid(), m_tc_comp: uuid(), m_track: uuid(), m_seq: uuid(), m_clip: uuid(), s_tc_track: uuid(), s_tc_seq: uuid(), s_tc_comp: uuid(), s_track: uuid(), s_seq: uuid(), s_clip: uuid(), desc: uuid(), sub: uuid(), mat_id: uuid(), file_id: uuid() };
        let key = ul(match essence {
            Essence::Jpeg2000(_) => E_JP2K,
            Essence::Dnxhr(_) => E_DNX,
            Essence::Pcm { .. } => E_PCM,
        });
        let mut w = MxfWriter { file, essence, rate, id: [0; 16], ids, body_at: 0, sizes: Vec::new(), stream: 0, key, title: title.into() };
        w.id = w.ids.file_id;
        // Header partition pack + reserved metadata space; filled in at the end.
        let pack = w.partition_pack(K_HEADER, 0, 0, 0, 0, RESERVE, 0, 0, 0);
        w.file.write_all(&pack)?;
        w.file.write_all(&vec![0u8; RESERVE as usize])?;
        w.body_at = pack.len() as u64 + RESERVE;
        let body = w.partition_pack(K_BODY, w.body_at, 0, 0, 0, 0, 0, 0, 1);
        w.file.write_all(&body)?;
        Ok(w)
    }

    #[allow(clippy::too_many_arguments)]
    fn partition_pack(&self, key: &str, this: u64, prev: u64, footer: u64, hbc: u64, _reserve: u64, ibc: u64, isid: u32, bsid: u32) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&1u16.to_be_bytes());
        v.extend_from_slice(&3u16.to_be_bytes());
        v.extend_from_slice(&1u32.to_be_bytes());
        for x in [this, prev, footer, if key == K_HEADER { RESERVE } else { hbc }, ibc] {
            v.extend_from_slice(&x.to_be_bytes());
        }
        v.extend_from_slice(&isid.to_be_bytes());
        v.extend_from_slice(&0u64.to_be_bytes());
        v.extend_from_slice(&bsid.to_be_bytes());
        v.extend_from_slice(&ul(OP_ATOM));
        v.extend_from_slice(&1u32.to_be_bytes());
        v.extend_from_slice(&16u32.to_be_bytes());
        v.extend_from_slice(&ul(match self.essence {
            Essence::Jpeg2000(_) => EC_JP2K,
            Essence::Dnxhr(_) => EC_DNX,
            Essence::Pcm { .. } => EC_PCM,
        }));
        let mut out = ul(key).to_vec();
        out.extend(ber4(v.len()));
        out.extend(v);
        out
    }

    /// Append one edit unit (a codestream, or the PCM of one frame time).
    pub fn write_frame(&mut self, data: &[u8]) -> io::Result<()> {
        let head = klv_ber(&self.key, data.len());
        self.file.write_all(&head)?;
        self.file.write_all(data)?;
        let total = (head.len() + data.len()) as u64;
        self.sizes.push(self.stream);
        self.stream += total;
        Ok(())
    }

    pub fn frames(&self) -> u64 {
        self.sizes.len() as u64
    }

    fn metadata(&self) -> Vec<u8> {
        let n = self.sizes.len() as i64;
        let (rn, rd) = self.rate;
        let ids = &self.ids;
        let now = timestamp_now();
        let is_pic = matches!(self.essence, Essence::Jpeg2000(_) | Essence::Dnxhr(_));
        let dd = if is_pic { DD_PICTURE } else { DD_SOUND };
        let ec = match self.essence {
            Essence::Jpeg2000(_) => EC_JP2K,
            Essence::Dnxhr(_) => EC_DNX,
            Essence::Pcm { .. } => EC_PCM,
        };
        let mut sets: Vec<Vec<u8>> = Vec::new();
        let mut p = Set::new(SET_PREFACE, &ids.preface);
        p.ts(0x3b02, &now).u16(0x3b05, 0x0103).u32(0x3b07, 1).batch(0x3b06, &[ids.ident.to_vec()]).raw(0x3b03, &ids.content).raw(0x3b09, &ul(OP_ATOM)).batch(0x3b0a, &[ul(ec).to_vec()]).batch(0x3b0b, &[]);
        sets.push(p.klv());
        let mut i = Set::new(SET_IDENT, &ids.ident);
        i.raw(0x3c09, &uuid()).utf16(0x3c01, "Ashrox LLC").utf16(0x3c02, "Studio").raw(0x3c03, &[0, 1, 0, 0, 0, 0, 0, 0, 0, 0]).utf16(0x3c04, "Studio deliver 1.0").raw(0x3c05, &uuid()).ts(0x3c06, &now);
        sets.push(i.klv());
        let mut c = Set::new(SET_CONTENT, &ids.content);
        c.batch(0x1901, &[ids.mat_pkg.to_vec(), ids.src_pkg.to_vec()]).batch(0x1902, &[ids.ecd.to_vec()]);
        sets.push(c.klv());
        let mut e = Set::new(SET_ECD, &ids.ecd);
        e.raw(0x2701, &umid(&ids.file_id)).u32(0x3f07, 1).u32(0x3f06, 2);
        sets.push(e.klv());

        for (pkg_key, pkg_inst, pkg_umid, tc_track, tc_seq, tc_comp, track, seq, clip, with_desc) in [
            (SET_MATERIAL, ids.mat_pkg, ids.mat_id, ids.m_tc_track, ids.m_tc_seq, ids.m_tc_comp, ids.m_track, ids.m_seq, ids.m_clip, false),
            (SET_SOURCE, ids.src_pkg, ids.file_id, ids.s_tc_track, ids.s_tc_seq, ids.s_tc_comp, ids.s_track, ids.s_seq, ids.s_clip, true),
        ] {
            let mut pk = Set::new(pkg_key, &pkg_inst);
            pk.raw(0x4401, &umid(&pkg_umid)).ts(0x4405, &now).ts(0x4404, &now).batch(0x4403, &[tc_track.to_vec(), track.to_vec()]);
            if !with_desc {
                pk.utf16(0x4402, &self.title);
            } else {
                pk.raw(0x4701, &ids.desc);
            }
            sets.push(pk.klv());
            // Timecode track.
            let mut t = Set::new(SET_TRACK, &tc_track);
            t.u32(0x4801, 1).u32(0x4804, 0).rational(0x4b01, rn, rd).i64(0x4b02, 0).raw(0x4803, &tc_seq);
            sets.push(t.klv());
            let mut s = Set::new(SET_SEQUENCE, &tc_seq);
            s.raw(0x0201, &ul(DD_TIMECODE)).i64(0x0202, n).batch(0x1001, &[tc_comp.to_vec()]);
            sets.push(s.klv());
            let mut tc = Set::new(SET_TIMECODE, &tc_comp);
            let base = (rn as f64 / rd as f64).round() as u16;
            tc.raw(0x0201, &ul(DD_TIMECODE)).i64(0x0202, n).i64(0x1501, 0).u16(0x1502, base).u8(0x1503, 0);
            sets.push(tc.klv());
            // Essence track.
            let mut t = Set::new(SET_TRACK, &track);
            t.u32(0x4801, 2).u32(0x4804, if with_desc { u32::from_be_bytes(self.key[12..16].try_into().unwrap()) } else { 0 }).rational(0x4b01, rn, rd).i64(0x4b02, 0).raw(0x4803, &seq);
            sets.push(t.klv());
            let mut s = Set::new(SET_SEQUENCE, &seq);
            s.raw(0x0201, &ul(dd)).i64(0x0202, n).batch(0x1001, &[clip.to_vec()]);
            sets.push(s.klv());
            let mut sc = Set::new(SET_CLIP, &clip);
            if with_desc {
                sc.raw(0x0201, &ul(dd)).i64(0x0202, n).i64(0x1201, 0).raw(0x1101, &[0u8; 32]).u32(0x1102, 0);
            } else {
                sc.raw(0x0201, &ul(dd)).i64(0x0202, n).i64(0x1201, 0).raw(0x1101, &umid(&ids.file_id)).u32(0x1102, 2);
            }
            sets.push(sc.klv());
        }

        match &self.essence {
            Essence::Jpeg2000(j) => {
                let mut d = Set::new(SET_CDCI, &ids.desc);
                d.u32(0x3006, 2).rational(0x3001, rn, rd).i64(0x3002, n).raw(0x3004, &ul(EC_JP2K));
                d.u32(0x3203, j.width).u32(0x3202, j.height).u32(0x3205, j.width).u32(0x3204, j.height).u32(0x3206, 0).u32(0x3207, 0).u32(0x3209, j.width).u32(0x3208, j.height).u32(0x320a, 0).u32(0x320b, 0);
                d.u32(0x3301, j.bit_depth as u32).u32(0x3302, 1).u32(0x3308, 1).u8(0x3303, 0).u16(0x3307, 0).u32(0x3304, 0).u32(0x3305, (1u32 << j.bit_depth) - 1).u32(0x3306, 1u32 << j.bit_depth);
                d.u8(0x320c, 0);
                let mut vlm = Vec::new();
                vlm.extend_from_slice(&2u32.to_be_bytes());
                vlm.extend_from_slice(&4u32.to_be_bytes());
                vlm.extend_from_slice(&[0u8; 8]);
                d.raw(0x320d, &vlm);
                let g = gcd(j.width, j.height).max(1);
                d.rational(0x320e, j.width / g, j.height / g);
                d.raw(0x3201, &ul(PIC_CODING_J2K));
                d.batch(0x8100, &[ids.sub.to_vec()]);
                sets.push(d.klv());
                let mut s = Set::new(SET_J2K, &ids.sub);
                s.u16(0x8401, j.rsiz).u32(0x8402, j.width).u32(0x8403, j.height).u32(0x8404, 0).u32(0x8405, 0).u32(0x8406, j.width).u32(0x8407, j.height).u32(0x8408, 0).u32(0x8409, 0).u16(0x840a, j.components as u16);
                let mut sizing = Vec::new();
                sizing.extend_from_slice(&(j.components as u32).to_be_bytes());
                sizing.extend_from_slice(&3u32.to_be_bytes());
                for _ in 0..j.components {
                    sizing.extend_from_slice(&[j.bit_depth - 1, 1, 1]);
                }
                s.raw(0x840b, &sizing);
                // COD and QCD are stored without their marker and length.
                s.raw(0x840c, &j.cod[..j.cod.len().min(0xfff0)]);
                s.raw(0x840d, &j.qcd[..j.qcd.len().min(0xfff0)]);
                sets.push(s.klv());
            }
            Essence::Dnxhr(x) => {
                let mut d = Set::new(SET_CDCI, &ids.desc);
                let stored_w = x.width.div_ceil(16) * 16;
                let stored_h = x.height.div_ceil(16) * 16;
                let scale = 1u32 << (x.bit_depth - 8);
                d.u32(0x3006, 2).rational(0x3001, rn, rd).i64(0x3002, n).raw(0x3004, &ul(EC_DNX));
                // Progressive: full frame layout; stored size is a multiple of 16, sampled and display sizes are the picture's.
                d.u32(0x3203, stored_w).u32(0x3202, stored_h).u32(0x3205, x.width).u32(0x3204, x.height).u32(0x3206, 0).u32(0x3207, 0).u32(0x3209, x.width).u32(0x3208, x.height).u32(0x320a, 0).u32(0x320b, 0);
                d.u32(0x3301, x.bit_depth as u32).u32(0x3302, if x.yuv444 { 1 } else { 2 }).u32(0x3308, 1).u8(0x3303, 0).u16(0x3307, 0).u32(0x3304, 16 * scale).u32(0x3305, 235 * scale).u32(0x3306, 224 * scale + 1);
                d.u8(0x320c, 0);
                let mut vlm = Vec::new();
                vlm.extend_from_slice(&2u32.to_be_bytes());
                vlm.extend_from_slice(&4u32.to_be_bytes());
                vlm.extend_from_slice(&[0u8; 8]);
                d.raw(0x320d, &vlm);
                let g = gcd(x.width, x.height).max(1);
                d.rational(0x320e, x.width / g, x.height / g);
                d.raw(0x3201, &ul(&x.coding_label()));
                sets.push(d.klv());
            }
            Essence::Pcm { rate, channels, bits } => {
                let mut d = Set::new(SET_WAVE, &ids.desc);
                let block = (*channels as u32) * (*bits as u32 / 8);
                d.u32(0x3006, 2).rational(0x3001, rn, rd).i64(0x3002, n).raw(0x3004, &ul(EC_PCM)).u8(0x3d02, 1).rational(0x3d03, *rate, 1).u32(0x3d07, *channels as u32).u32(0x3d01, *bits as u32).u16(0x3d0a, block as u16).u32(0x3d09, rate * block);
                sets.push(d.klv());
            }
        }

        // Primer: only the tags the sets above carry (all of them are listed; unused ones are harmless).
        let mut primer = Vec::new();
        primer.extend_from_slice(&(PRIMER.len() as u32).to_be_bytes());
        primer.extend_from_slice(&18u32.to_be_bytes());
        for (tag, l) in PRIMER {
            primer.extend_from_slice(&tag.to_be_bytes());
            primer.extend_from_slice(&ul(l));
        }
        let mut out = ul(K_PRIMER).to_vec();
        out.extend(ber4(primer.len()));
        out.extend(primer);
        for s in sets {
            out.extend(s);
        }
        out
    }

    /// Write the index and footer, then the header metadata and the final partition packs.
    pub fn finish(mut self) -> io::Result<MxfInfo> {
        let n = self.sizes.len();
        let footer_at = self.body_at + 16 + 4 + body_pack_value_len() as u64 + self.stream;
        // Index table segment(s), split to keep each local set under 64 KiB.
        let mut delta = Vec::new();
        delta.extend_from_slice(&1u32.to_be_bytes());
        delta.extend_from_slice(&6u32.to_be_bytes());
        delta.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        let mut index_bytes = Vec::new();
        let per_segment = ((60_000 - 8) / 11).max(1);
        for (si, chunk) in self.sizes.chunks(per_segment).enumerate() {
            let mut seg = Set { key: ul(K_INDEX), body: Vec::new() };
            seg.raw(0x3c0a, &uuid());
            seg.rational(0x3f0b, self.rate.0, self.rate.1).i64(0x3f0c, (si * per_segment) as i64).i64(0x3f0d, chunk.len() as i64).u32(0x3f05, 0).u32(0x3f06, 2).u32(0x3f07, 1).u8(0x3f08, 0).u8(0x3f0e, 0);
            seg.raw(0x3f09, &delta);
            let mut e = Vec::new();
            e.extend_from_slice(&(chunk.len() as u32).to_be_bytes());
            e.extend_from_slice(&11u32.to_be_bytes());
            for off in chunk {
                e.extend_from_slice(&[0, 0, 0x80]);
                e.extend_from_slice(&off.to_be_bytes());
            }
            seg.raw(0x3f0a, &e);
            index_bytes.extend(seg.klv());
        }
        let footer = self.partition_pack(K_FOOTER, footer_at, self.body_at, footer_at, 0, 0, index_bytes.len() as u64, 2, 0);
        self.file.write_all(&footer)?;
        self.file.write_all(&index_bytes)?;
        // Random index pack.
        let rip_at = footer_at + footer.len() as u64 + index_bytes.len() as u64;
        let mut rip = Vec::new();
        for (sid, off) in [(0u32, 0u64), (1, self.body_at), (0, footer_at)] {
            rip.extend_from_slice(&sid.to_be_bytes());
            rip.extend_from_slice(&off.to_be_bytes());
        }
        let total = 16 + 4 + rip.len() + 4;
        rip.extend_from_slice(&(total as u32).to_be_bytes());
        let mut rip_klv = ul(K_RIP).to_vec();
        rip_klv.extend(ber4(rip.len()));
        rip_klv.extend(rip);
        self.file.write_all(&rip_klv)?;
        let total_bytes = rip_at + rip_klv.len() as u64;
        // Header partition: now the metadata and the real footer position.
        let md = self.metadata();
        if md.len() as u64 + 20 > RESERVE {
            return Err(io::Error::other("MXF header metadata is larger than the reserved space"));
        }
        let mut region = md;
        let pad = RESERVE as usize - region.len();
        // A fill KLV of exactly the remaining size (key 16 + length 4 + zeros).
        let fill_body = pad - 20;
        region.extend_from_slice(&ul(K_FILL));
        region.extend(ber4(fill_body));
        region.extend(vec![0u8; fill_body]);
        let header = self.partition_pack(K_HEADER, 0, 0, footer_at, RESERVE, 0, 0, 0, 0);
        let body = self.partition_pack(K_BODY, self.body_at, 0, footer_at, 0, 0, 0, 0, 1);
        self.file.flush()?;
        let f = self.file.get_mut();
        f.seek(SeekFrom::Start(0))?;
        f.write_all(&header)?;
        f.write_all(&region)?;
        // The body partition learns where the footer is.
        f.seek(SeekFrom::Start(self.body_at))?;
        f.write_all(&body)?;
        f.flush()?;
        Ok(MxfInfo { frames: n as u64, bytes: total_bytes, id: self.id })
    }
}

fn body_pack_value_len() -> usize {
    // 2+2+4 + 5*8 + 4 + 8 + 4 + 16 + 4 + 4 + 16
    8 + 40 + 4 + 8 + 4 + 16 + 8 + 16
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_parse_and_ids_differ() {
        assert_eq!(ul("060e2b34 02050101 0d010201 01020400")[15], 0);
        for (_, l) in PRIMER {
            let l: String = l.split_whitespace().collect();
            assert_eq!(l.len(), 32, "{l}");
        }
        assert_ne!(uuid(), uuid());
        let u = uuid_string(&uuid());
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
    }

    #[test]
    fn a_dnxhr_track_file_has_the_vc3_labels_and_frames() {
        // A frame header with just the fields the descriptor reads.
        let mut frame = vec![0u8; 8192];
        frame[0..5].copy_from_slice(&[0, 0, 2, 0x80, 3]);
        frame[0x18..0x1a].copy_from_slice(&72u16.to_be_bytes());
        frame[0x1a..0x1c].copy_from_slice(&128u16.to_be_bytes());
        frame[0x21] = 2 << 5 | 0x18;
        frame[0x28..0x2c].copy_from_slice(&1271u32.to_be_bytes());
        let d = DnxDesc::from_frame(&frame).unwrap();
        assert_eq!((d.width, d.height, d.bit_depth, d.cid, d.yuv444), (128, 72, 10, 1271, false));
        assert_eq!(ul(&d.coding_label())[12..14], [0x71, 0x25]);
        assert!(DnxDesc::from_frame(&frame[..20]).is_err());
        let dir = std::env::temp_dir().join(format!("studio-mxf-dnx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dnx.mxf");
        let mut m = MxfWriter::create(&path, Essence::Dnxhr(d), (24, 1), "DNx test").unwrap();
        for _ in 0..3 {
            m.write_frame(&frame).unwrap();
        }
        let info = m.finish().unwrap();
        assert_eq!(info.frames, 3);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.len() as u64, info.bytes);
        let find = |pat: &[u8]| bytes.windows(pat.len()).position(|w| w == pat);
        assert!(find(&ul(E_DNX)).is_some(), "element key");
        assert!(find(&ul(EC_DNX)).is_some(), "essence container label");
        assert!(find(&ul(&d.coding_label())).is_some(), "compression label");
        assert!(find(&ul(E_JP2K)).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_timestamp_is_a_plausible_date() {
        let t = timestamp_now();
        let y = u16::from_be_bytes([t[0], t[1]]);
        assert!((2024..2100).contains(&y) && (1..=12).contains(&t[2]) && (1..=31).contains(&t[3]) && t[4] < 24);
    }
}

