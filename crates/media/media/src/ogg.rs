// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Ogg bitstream demuxing (RFC 3533): page parsing with CRC check, packet
//! reassembly across pages (lacing), and granule positions. The codec inside
//! is handed to `vorbis`. Hand-written, no third-party code.

use crate::wav::Wav;
use std::sync::OnceLock;

fn crc_table() -> &'static [u32; 256] {
    static T: OnceLock<[u32; 256]> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut r = (i as u32) << 24;
            for _ in 0..8 {
                r = if r & 0x8000_0000 != 0 { (r << 1) ^ 0x04c1_1db7 } else { r << 1 };
            }
            *e = r;
        }
        t
    })
}

/// Continues an Ogg CRC (polynomial 0x04c11db7, MSB-first, init 0, no final xor).
pub fn crc_update(mut crc: u32, data: &[u8]) -> u32 {
    let t = crc_table();
    for &b in data {
        crc = (crc << 8) ^ t[((crc >> 24) as u8 ^ b) as usize];
    }
    crc
}

/// Ogg CRC-32 of `data`.
pub fn crc32(data: &[u8]) -> u32 {
    crc_update(0, data)
}

/// One parsed Ogg page, borrowing from the file bytes.
#[derive(Clone, Debug)]
pub struct Page<'a> {
    pub header_type: u8,
    /// Raw granule position (`u64::MAX` = no packet finishes on this page).
    pub granule: u64,
    pub serial: u32,
    pub sequence: u32,
    pub lacing: &'a [u8],
    pub body: &'a [u8],
}

impl Page<'_> {
    pub fn continued(&self) -> bool {
        self.header_type & 1 != 0
    }
    pub fn bos(&self) -> bool {
        self.header_type & 2 != 0
    }
    pub fn eos(&self) -> bool {
        self.header_type & 4 != 0
    }
    pub fn granule_pos(&self) -> Option<u64> {
        (self.granule != u64::MAX).then_some(self.granule)
    }
}

/// Parses the page at the start of `d`; returns it and its total length.
pub fn parse_page(d: &[u8]) -> Result<(Page<'_>, usize), String> {
    if d.len() < 27 || &d[..4] != b"OggS" {
        return Err("no Ogg capture pattern".into());
    }
    if d[4] != 0 {
        return Err(format!("unsupported Ogg version {}", d[4]));
    }
    let nseg = d[26] as usize;
    let hlen = 27 + nseg;
    if d.len() < hlen {
        return Err("truncated Ogg page header".into());
    }
    let lacing = &d[27..hlen];
    let blen: usize = lacing.iter().map(|&l| l as usize).sum();
    let total = hlen + blen;
    if d.len() < total {
        return Err("truncated Ogg page".into());
    }
    let stored = u32::from_le_bytes([d[22], d[23], d[24], d[25]]);
    let mut crc = crc_update(0, &d[..22]);
    crc = crc_update(crc, &[0; 4]);
    crc = crc_update(crc, &d[26..total]);
    if crc != stored {
        return Err("Ogg page CRC mismatch".into());
    }
    let le32 = |o: usize| u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
    let page = Page {
        header_type: d[5],
        granule: u64::from_le_bytes(d[6..14].try_into().unwrap()),
        serial: le32(14),
        sequence: le32(18),
        lacing,
        body: &d[hlen..total],
    };
    Ok((page, total))
}

/// Iterator over the valid pages of a file; skips garbage and pages with a
/// bad CRC by resynchronising on the next "OggS".
pub struct Pages<'a> {
    d: &'a [u8],
    pos: usize,
}

impl<'a> Iterator for Pages<'a> {
    type Item = Page<'a>;
    fn next(&mut self) -> Option<Page<'a>> {
        while self.pos + 27 <= self.d.len() {
            let rest = &self.d[self.pos..];
            if &rest[..4] != b"OggS" {
                match rest.windows(4).position(|w| w == b"OggS") {
                    Some(i) => self.pos += i,
                    None => {
                        self.pos = self.d.len();
                        return None;
                    }
                }
                continue;
            }
            match parse_page(rest) {
                Ok((p, len)) => {
                    self.pos += len;
                    return Some(p);
                }
                Err(_) => self.pos += 1,
            }
        }
        None
    }
}

pub fn pages(d: &[u8]) -> Pages<'_> {
    Pages { d, pos: 0 }
}

/// Serial number of the first logical stream whose first packet starts with `sig`.
pub fn find_stream(d: &[u8], sig: &[u8]) -> Option<u32> {
    pages(d).find(|p| p.bos() && p.body.starts_with(sig)).map(|p| p.serial)
}

/// One reassembled packet.
#[derive(Clone, Debug, PartialEq)]
pub struct Packet {
    pub data: Vec<u8>,
    /// Granule position, set on the last packet that completes on a page.
    pub granule: Option<u64>,
    /// The packet ended on an end-of-stream page.
    pub eos: bool,
}

/// All packets of logical stream `serial`, in order. Packets broken by a lost
/// or corrupt page are dropped.
pub fn packets(d: &[u8], serial: u32) -> Vec<Packet> {
    let mut out: Vec<Packet> = Vec::new();
    let mut partial: Option<Vec<u8>> = None;
    let mut last_seq: Option<u32> = None;
    for page in pages(d).filter(|p| p.serial == serial) {
        if last_seq.is_some_and(|s| page.sequence != s.wrapping_add(1)) {
            partial = None;
        }
        last_seq = Some(page.sequence);
        let carried = partial.take();
        // A continuation whose start we never saw is skipped up to its end.
        let mut skipping = page.continued() && carried.is_none();
        let mut buf = if page.continued() { carried.unwrap_or_default() } else { Vec::new() };
        let first_new = out.len();
        let mut off = 0;
        for &l in page.lacing {
            let l = l as usize;
            if !skipping {
                buf.extend_from_slice(&page.body[off..off + l]);
            }
            off += l;
            if l < 255 {
                if !skipping {
                    out.push(Packet { data: std::mem::take(&mut buf), granule: None, eos: false });
                }
                skipping = false;
                buf = Vec::new();
            }
        }
        if out.len() > first_new {
            let last = out.last_mut().unwrap();
            last.granule = page.granule_pos();
            last.eos = page.eos();
        }
        if page.lacing.last() == Some(&255) && !skipping {
            partial = Some(buf);
        }
        if page.eos() {
            break;
        }
    }
    out
}

/// Reads an Ogg Vorbis file to interleaved PCM at its native rate.
pub fn read(path: &std::path::Path) -> Result<Wav, String> {
    crate::vorbis::decode(&std::fs::read(path).map_err(|e| e.to_string())?)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Builds one page (test helper, also used by the vorbis tests).
    pub(crate) fn make_page(htype: u8, granule: u64, serial: u32, seq: u32, lacing: &[u8], body: &[u8]) -> Vec<u8> {
        let mut p = b"OggS".to_vec();
        p.push(0);
        p.push(htype);
        p.extend_from_slice(&granule.to_le_bytes());
        p.extend_from_slice(&serial.to_le_bytes());
        p.extend_from_slice(&seq.to_le_bytes());
        p.extend_from_slice(&[0; 4]);
        p.push(lacing.len() as u8);
        p.extend_from_slice(lacing);
        p.extend_from_slice(body);
        let c = crc32(&p);
        p[22..26].copy_from_slice(&c.to_le_bytes());
        p
    }

    #[test]
    fn crc_check_value() {
        // CRC-32 with poly 0x04c11db7, init 0, no reflection, no xor-out.
        assert_eq!(crc32(b"123456789"), 0x89A1_897F);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn page_roundtrip_and_crc_rejection() {
        let page = make_page(2, 0, 7, 0, &[3], b"abc");
        let (p, len) = parse_page(&page).unwrap();
        assert_eq!(len, page.len());
        assert!(p.bos() && !p.eos() && !p.continued());
        assert_eq!((p.serial, p.sequence, p.body), (7, 0, &b"abc"[..]));
        let mut bad = page.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(parse_page(&bad).unwrap_err().contains("CRC"));
    }

    #[test]
    fn packets_span_pages_with_granules() {
        // Packet A: 600 bytes split 510 | 90 across pages; packet B: 10 bytes.
        let a: Vec<u8> = (0..600).map(|i| i as u8).collect();
        let b = vec![9u8; 10];
        let mut f = b"junk".to_vec();
        f.extend(make_page(2, u64::MAX, 5, 0, &[255, 255], &a[..510]));
        f.extend(make_page(0, 0, 99, 0, &[1], b"x")); // other stream, interleaved
        let mut body2 = a[510..].to_vec();
        body2.extend_from_slice(&b);
        f.extend(make_page(1 | 4, 1234, 5, 1, &[90, 10], &body2));
        let pk = packets(&f, 5);
        assert_eq!(pk.len(), 2);
        assert_eq!(pk[0].data, a);
        assert_eq!(pk[0].granule, None);
        assert_eq!(pk[1].data, b);
        assert_eq!(pk[1].granule, Some(1234));
        assert!(pk[1].eos);
        assert_eq!(packets(&f, 99).len(), 1);
    }

    #[test]
    fn lost_page_drops_broken_packet() {
        let a = vec![1u8; 300];
        let mut f = make_page(2, u64::MAX, 1, 0, &[255], &a[..255]);
        let mut p2 = make_page(1, 10, 1, 1, &[45, 2], &[a[255..].to_vec(), vec![7, 7]].concat());
        p2[30] ^= 0xFF; // corrupt body: CRC fails, page skipped
        f.extend(p2);
        f.extend(make_page(0, 20, 1, 2, &[3], &[4, 5, 6]));
        let pk = packets(&f, 1);
        assert_eq!(pk, vec![Packet { data: vec![4, 5, 6], granule: Some(20), eos: false }]);
    }
}
