// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Box headers and bounds-checked byte readers.
//!
//! Top-level boxes are read straight from the `Read + Seek` source (header only);
//! metadata payloads are then parsed from in-memory slices with [`Reader`] and
//! [`Boxes`], which never index out of bounds.

use std::io::{Read, Seek, SeekFrom};

use crate::error::{Error, Result};

/// A four-character code such as `*b"moov"`.
pub type FourCC = [u8; 4];

/// Header of a box found at the top level of the file.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TopBox {
    pub kind: FourCC,
    /// File offset of the first header byte.
    pub start: u64,
    /// File offset of the first payload byte.
    pub payload: u64,
    /// File offset just past the box.
    pub end: u64,
}

impl TopBox {
    pub fn payload_len(&self) -> u64 {
        self.end - self.payload
    }
}

/// Reads the box header at `pos`; `Ok(None)` at a clean end of file.
pub(crate) fn read_top_header<R: Read + Seek>(
    r: &mut R,
    pos: u64,
    file_len: u64,
) -> Result<Option<TopBox>> {
    if pos >= file_len {
        return Ok(None);
    }
    if file_len - pos < 8 {
        return Err(Error::Truncated("top-level box header"));
    }
    r.seek(SeekFrom::Start(pos))?;
    let mut head = [0u8; 8];
    r.read_exact(&mut head)?;
    let size32 = u32::from_be_bytes([head[0], head[1], head[2], head[3]]);
    let kind = [head[4], head[5], head[6], head[7]];
    let mut header_len = 8u64;
    let size = match size32 {
        0 => file_len - pos,
        1 => {
            if file_len - pos < 16 {
                return Err(Error::Truncated("64-bit box header"));
            }
            let mut big = [0u8; 8];
            r.read_exact(&mut big)?;
            header_len = 16;
            u64::from_be_bytes(big)
        }
        n => n as u64,
    };
    if &kind == b"uuid" {
        header_len += 16;
    }
    if size < header_len {
        return Err(Error::malformed(format!(
            "box '{}' size {size} smaller than its header",
            fourcc_str(&kind)
        )));
    }
    if size > file_len - pos {
        return Err(Error::Truncated("top-level box (extends past end of file)"));
    }
    Ok(Some(TopBox { kind, start: pos, payload: pos + header_len, end: pos + size }))
}

/// A child box inside an in-memory payload.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BoxRef<'a> {
    pub kind: FourCC,
    pub data: &'a [u8],
}

/// Iterator over the boxes packed in a slice. Yields an error once and stops.
pub(crate) struct Boxes<'a> {
    data: &'a [u8],
    pos: usize,
    done: bool,
}

impl<'a> Boxes<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Boxes { data, pos: 0, done: false }
    }

    fn next_box(&mut self) -> Result<Option<BoxRef<'a>>> {
        let rest = &self.data[self.pos..];
        if rest.len() < 8 {
            // QuickTime pads some containers with a 32-bit zero terminator.
            if rest.iter().all(|&b| b == 0) {
                return Ok(None);
            }
            return Err(Error::Truncated("box header"));
        }
        let size32 = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) as u64;
        let kind = [rest[4], rest[5], rest[6], rest[7]];
        let mut header = 8usize;
        let size = match size32 {
            0 => rest.len() as u64,
            1 => {
                if rest.len() < 16 {
                    return Err(Error::Truncated("64-bit box header"));
                }
                header = 16;
                u64::from_be_bytes(rest[8..16].try_into().unwrap())
            }
            n => n,
        };
        if &kind == b"uuid" {
            header += 16;
        }
        if size < header as u64 {
            return Err(Error::malformed(format!(
                "box '{}' size {size} smaller than its header",
                fourcc_str(&kind)
            )));
        }
        if size > rest.len() as u64 {
            return Err(Error::malformed(format!(
                "box '{}' overruns its parent",
                fourcc_str(&kind)
            )));
        }
        let size = size as usize;
        let item = BoxRef { kind, data: &rest[header..size] };
        self.pos += size;
        Ok(Some(item))
    }
}

impl<'a> Iterator for Boxes<'a> {
    type Item = Result<BoxRef<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        match self.next_box() {
            Ok(Some(b)) => Some(Ok(b)),
            Ok(None) => {
                self.done = true;
                None
            }
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}

/// Returns the payload of the first child of type `kind`, if any.
pub(crate) fn find<'a>(data: &'a [u8], kind: &FourCC) -> Result<Option<&'a [u8]>> {
    for b in Boxes::new(data) {
        let b = b?;
        if &b.kind == kind {
            return Ok(Some(b.data));
        }
    }
    Ok(None)
}

/// Like [`find`] but the box is mandatory.
pub(crate) fn require<'a>(data: &'a [u8], kind: &FourCC, name: &'static str) -> Result<&'a [u8]> {
    find(data, kind)?.ok_or(Error::Missing(name))
}

/// Printable form of a four-character code.
pub(crate) fn fourcc_str(k: &FourCC) -> String {
    k.iter()
        .map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '?' })
        .collect()
}

/// Big-endian cursor over a payload; every read is bounds checked.
#[derive(Debug, Clone)]
pub(crate) struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    what: &'static str,
}

impl<'a> Reader<'a> {
    /// `what` names the structure in truncation errors.
    pub fn new(data: &'a [u8], what: &'static str) -> Self {
        Reader { data, pos: 0, what }
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.remaining() {
            return Err(Error::Truncated(self.what));
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.bytes(n).map(|_| ())
    }

    pub fn rest(&mut self) -> &'a [u8] {
        let s = &self.data[self.pos..];
        self.pos = self.data.len();
        s
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.bytes(N)?.try_into().unwrap())
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    pub fn u24(&mut self) -> Result<u32> {
        let b = self.array::<3>()?;
        Ok(u32::from_be_bytes([0, b[0], b[1], b[2]]))
    }

    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_be_bytes(self.array()?))
    }

    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.array()?))
    }

    pub fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_be_bytes(self.array()?))
    }

    pub fn fourcc(&mut self) -> Result<FourCC> {
        self.array()
    }

    /// Reads a full-box header and returns `(version, flags)`.
    pub fn full_header(&mut self) -> Result<(u8, u32)> {
        let v = self.u32()?;
        Ok(((v >> 24) as u8, v & 0x00ff_ffff))
    }

    /// Reads an entry count and verifies that `count * entry_size` bytes remain,
    /// so callers can allocate `count` entries safely.
    pub fn count(&mut self, entry_size: usize) -> Result<usize> {
        let n = self.u32()? as usize;
        if n.checked_mul(entry_size).is_none_or(|need| need > self.remaining()) {
            return Err(Error::Truncated(self.what));
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iterates_children_and_rejects_overruns() {
        let mut data = Vec::new();
        data.extend_from_slice(&[0, 0, 0, 12, b'f', b'r', b'e', b'e', 1, 2, 3, 4]);
        data.extend_from_slice(&[0, 0, 0, 1, b'b', b'i', b'g', b' ', 0, 0, 0, 0, 0, 0, 0, 17, 9]);
        data.extend_from_slice(&[0, 0, 0, 0]);
        let kids: Vec<_> = Boxes::new(&data).collect::<Result<_>>().unwrap();
        assert_eq!(kids.len(), 2);
        assert_eq!(kids[0].data, &[1, 2, 3, 4]);
        assert_eq!(kids[1].data, &[9]);

        let bad = [0, 0, 0, 40, b'f', b'r', b'e', b'e', 0];
        assert!(Boxes::new(&bad).next().unwrap().is_err());
        let tiny = [0, 0, 0, 4, b'f', b'r', b'e', b'e'];
        assert!(Boxes::new(&tiny).next().unwrap().is_err());
    }

    #[test]
    fn reader_counts_are_bounded() {
        let data = [0xff, 0xff, 0xff, 0xff, 0, 0];
        assert!(Reader::new(&data, "t").count(4).is_err());
        let data = [0, 0, 0, 1, 0, 0, 0, 7];
        let mut r = Reader::new(&data, "t");
        assert_eq!(r.count(4).unwrap(), 1);
        assert_eq!(r.u32().unwrap(), 7);
        assert!(r.u8().is_err());
    }
}
