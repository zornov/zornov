// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `HeaderTracker` reads every frame header of real streams, in order, and agrees with the decoder.

use studio_av1::header::{HeaderTracker, SequenceHeader};
use studio_av1::spec_tables::KEY_FRAME;
use studio_av1::Decoder;

mod fixtures;

const STREAMS: [(&str, &[u8], u8); 2] = [("tiny8", fixtures::TINY8, 8), ("tiny10", fixtures::TINY10, 10)];

/// IVF frames (the 32-byte file header, then 12-byte frame headers).
fn ivf_packets(b: &[u8]) -> Vec<&[u8]> {
    let mut v = Vec::new();
    let mut i = u16::from_le_bytes([b[6], b[7]]) as usize;
    while i + 12 <= b.len() {
        let n = u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
        v.push(&b[i + 12..i + 12 + n]);
        i += 12 + n;
    }
    v
}

/// (obu_type, temporal_id, spatial_id, payload) of each OBU of a temporal unit.
fn obus(mut d: &[u8]) -> Vec<(u8, u32, u32, &[u8])> {
    let mut v = Vec::new();
    while !d.is_empty() {
        let h = d[0];
        let (ty, ext, has_size) = ((h >> 3) & 15, h & 4 != 0, h & 2 != 0);
        assert!(has_size, "test streams use sized OBUs");
        let (tid, sid) = if ext { (u32::from(d[1] >> 5), u32::from((d[1] >> 3) & 3)) } else { (0, 0) };
        let mut i = 1 + usize::from(ext);
        let (mut size, mut shift) = (0usize, 0);
        loop {
            let b = d[i];
            i += 1;
            size |= usize::from(b & 0x7f) << shift;
            shift += 7;
            if b & 0x80 == 0 {
                break;
            }
        }
        v.push((ty, tid, sid, &d[i..i + size]));
        d = &d[i + size..];
    }
    v
}

#[test]
fn every_header_of_real_streams_parses_in_order() {
    for (name, data, depth) in STREAMS {
        let mut seq = None;
        let mut tracker = HeaderTracker::new();
        let (mut headers, mut shown, mut keys) = (0, 0, 0);
        for tu in ivf_packets(data) {
            for (ty, tid, sid, payload) in obus(tu) {
                match ty {
                    1 => seq = Some(SequenceHeader::parse(payload).expect("sequence header")),
                    3 | 6 => {
                        let seq = seq.as_ref().expect("sequence header first");
                        let (fh, bits) = tracker.parse_frame_header(seq, payload, tid, sid).unwrap_or_else(|e| panic!("{name}: {e}"));
                        assert!(bits > 0 && bits <= payload.len() * 8, "{name}: {bits} bits of {} bytes", payload.len());
                        assert_eq!(seq.color.bit_depth, depth);
                        if !fh.show_existing_frame {
                            assert_eq!((fh.upscaled_width, fh.frame_height), (96, 64));
                        }
                        headers += 1;
                        shown += usize::from(fh.show_frame || fh.show_existing_frame);
                        keys += usize::from(fh.frame_type == KEY_FRAME as u8 && !fh.show_existing_frame);
                    }
                    _ => {}
                }
            }
        }
        // The decoder shows the same number of pictures as the headers announce.
        let mut d = Decoder::new();
        let mut pics = 0;
        for tu in ivf_packets(data) {
            pics += d.decode(tu).expect("decode").len();
        }
        pics += d.flush().len();
        assert_eq!(pics, shown, "{name}");
        assert!(headers >= 10 && keys >= 2, "{name}: {headers} headers, {keys} key frames");
    }
}

#[test]
fn bad_input_is_an_error_not_a_panic() {
    let (_, data, _) = STREAMS[0];
    let first = obus(ivf_packets(data)[0]);
    let seq = SequenceHeader::parse(first.iter().find(|o| o.0 == 1).expect("sequence header").3).expect("parse");
    // empty and truncated payloads
    assert!(HeaderTracker::new().parse_frame_header(&seq, &[], 0, 0).is_err());
    let frame = first.iter().find(|o| o.0 == 6 || o.0 == 3).expect("frame").3;
    assert!(HeaderTracker::new().parse_frame_header(&seq, &frame[..1], 0, 0).is_err());
    // an inter frame before any reference exists
    let second = obus(ivf_packets(data)[1]);
    let inter = second.iter().find(|o| o.0 == 6 || o.0 == 3).expect("frame").3;
    // Whether it is an error is the header parser's business (it checks the references later);
    // what matters here is that a fresh tracker survives it.
    let _ = HeaderTracker::new().parse_frame_header(&seq, inter, 0, 0);
}
