// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! AV1 in ISO BMFF (AV1-ISOBMFF): the `av1C` record, and the temporal-unit
//! to sample rules.
//!
//! A sample is one temporal unit in the low-overhead format (every OBU has a
//! size field) without temporal delimiter or padding OBUs. A sample is a
//! sync sample when it holds a shown key frame and a sequence header.

use super::ColorDesc;

const OBU_SEQUENCE_HEADER: u8 = 1;
const OBU_TEMPORAL_DELIMITER: u8 = 2;
const OBU_FRAME_HEADER: u8 = 3;
const OBU_FRAME: u8 = 6;
const OBU_PADDING: u8 = 15;

/// Track description for an AV1 video track.
#[derive(Clone, Debug)]
pub struct Av1TrackInfo {
    /// The sequence header's maximum frame size.
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    /// `AV1CodecConfigurationRecord`, see [`av1c_from_sequence_header`].
    pub av1c: Vec<u8>,
    pub color: ColorDesc,
    pub bitrate: u32,
}

/// What the container needs from a sequence header OBU.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SequenceInfo {
    pub av1c: Vec<u8>,
    pub max_width: u32,
    pub max_height: u32,
    pub bit_depth: u8,
    pub color_primaries: u8,
    pub transfer: u8,
    pub matrix: u8,
    pub full_range: bool,
}

impl SequenceInfo {
    /// Track description for `fps_num / fps_den`, with colour taken from the sequence header.
    pub fn track_info(&self, fps_num: u32, fps_den: u32, bitrate: u32) -> Av1TrackInfo {
        let (primaries, transfer, matrix) = (self.color_primaries, self.transfer, self.matrix);
        // Unspecified (2) falls back to BT.709 as for the other codecs.
        let pick = |v: u8| if v == 2 { 1 } else { v };
        Av1TrackInfo {
            width: self.max_width,
            height: self.max_height,
            fps_num,
            fps_den,
            av1c: self.av1c.clone(),
            color: ColorDesc { primaries: pick(primaries), transfer: pick(transfer), matrix: pick(matrix), full_range: self.full_range },
            bitrate,
        }
    }
}

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn f(&mut self, n: u32) -> Option<u32> {
        let mut v = 0u64;
        for _ in 0..n {
            let byte = *self.data.get(self.pos / 8)?;
            v = v << 1 | ((byte >> (7 - self.pos % 8)) & 1) as u64;
            self.pos += 1;
        }
        Some(v as u32)
    }
    fn flag(&mut self) -> Option<bool> {
        Some(self.f(1)? != 0)
    }
    fn uvlc(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while !self.flag()? {
            zeros += 1;
            if zeros >= 32 {
                return Some(u32::MAX);
            }
        }
        Some(self.f(zeros)? + ((1u64 << zeros) - 1) as u32)
    }
}

/// One OBU of a temporal unit.
struct Obu {
    kind: u8,
    /// The whole OBU as stored, header and size field included.
    start: usize,
    end: usize,
    payload: usize,
}

fn leb128(data: &[u8]) -> Option<(u64, usize)> {
    let mut v = 0u64;
    for i in 0..8 {
        let b = *data.get(i)?;
        v |= ((b & 0x7f) as u64) << (i * 7);
        if b & 0x80 == 0 {
            return Some((v, i + 1));
        }
    }
    None
}

fn write_leb128(out: &mut Vec<u8>, mut v: usize) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn obus(data: &[u8]) -> Option<Vec<Obu>> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        let h = data[pos];
        if h & 0x80 != 0 {
            return None; // forbidden bit
        }
        let kind = (h >> 3) & 0xf;
        let mut p = pos + 1 + (h & 4 != 0) as usize;
        let size = if h & 2 != 0 {
            let (v, n) = leb128(data.get(p..)?)?;
            p += n;
            usize::try_from(v).ok()?
        } else {
            data.len().checked_sub(p)?
        };
        let end = p.checked_add(size).filter(|&e| e <= data.len())?;
        out.push(Obu { kind, start: pos, end, payload: p });
        pos = end;
    }
    Some(out)
}

/// The temporal unit as a sample: temporal delimiters and padding OBUs
/// removed. `None` when the data is not a well-formed OBU sequence.
pub fn sample_from_temporal_unit(tu: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(tu.len());
    let list = obus(tu)?;
    let last = list.len().saturating_sub(1);
    for (i, o) in list.iter().enumerate() {
        if matches!(o.kind, OBU_TEMPORAL_DELIMITER | OBU_PADDING) {
            continue;
        }
        let h = tu[o.start];
        if h & 2 != 0 || i != last {
            // Keep the OBU, giving it a size field when it lacks one.
            if h & 2 != 0 {
                out.extend_from_slice(&tu[o.start..o.end]);
            } else {
                let ext = (h & 4 != 0) as usize;
                out.push(h | 2);
                out.extend_from_slice(&tu[o.start + 1..o.start + 1 + ext]);
                write_leb128(&mut out, o.end - o.payload);
                out.extend_from_slice(&tu[o.payload..o.end]);
            }
        } else {
            out.extend_from_slice(&tu[o.start..o.end]);
        }
    }
    Some(out)
}

/// Whether the sample is a sync sample: a shown key frame, with a sequence
/// header before the first frame header.
pub fn is_sync_sample(sample: &[u8]) -> bool {
    let Some(list) = obus(sample) else { return false };
    let mut seq = false;
    for o in &list {
        match o.kind {
            OBU_SEQUENCE_HEADER => seq = true,
            OBU_FRAME_HEADER | OBU_FRAME => {
                let Some(&b) = sample.get(o.payload).filter(|_| o.payload < o.end) else { return false };
                // show_existing_frame = 0, frame_type = KEY_FRAME, show_frame = 1
                // (reduced_still_picture_header streams are not handled).
                return seq && b >> 7 == 0 && (b >> 5) & 3 == 0 && (b >> 4) & 1 == 1;
            }
            _ => {}
        }
    }
    false
}

/// Builds the `av1C` record from a sequence header OBU (header and size
/// field included, as found in a temporal unit) and reads out its size and
/// colour.
pub fn av1c_from_sequence_header(obu: &[u8]) -> Option<SequenceInfo> {
    let list = obus(obu)?;
    let o = list.iter().find(|o| o.kind == OBU_SEQUENCE_HEADER)?;
    let mut b = Bits { data: &obu[o.payload..o.end], pos: 0 };
    let profile = b.f(3)?;
    if profile > 2 {
        return None;
    }
    let _still = b.flag()?;
    let reduced = b.flag()?;
    let (level, tier);
    if reduced {
        level = b.f(5)?;
        tier = 0;
    } else {
        let timing = b.flag()?;
        let mut decoder_model = false;
        let mut delay_len = 0;
        if timing {
            b.f(32)?;
            b.f(32)?;
            if b.flag()? {
                b.uvlc()?;
            }
            decoder_model = b.flag()?;
            if decoder_model {
                delay_len = b.f(5)? + 1;
                b.f(32)?;
                b.f(5)?;
                b.f(5)?;
            }
        }
        let display_delay = b.flag()?;
        let count = b.f(5)? + 1;
        let (mut l0, mut t0) = (0, 0);
        for i in 0..count {
            b.f(12)?;
            let l = b.f(5)?;
            let t = if l > 7 { b.f(1)? } else { 0 };
            if i == 0 {
                (l0, t0) = (l, t);
            }
            if decoder_model && b.flag()? {
                b.f(delay_len)?;
                b.f(delay_len)?;
                b.f(1)?;
            }
            if display_delay && b.flag()? {
                b.f(4)?;
            }
        }
        level = l0;
        tier = t0;
    }
    let wbits = b.f(4)? + 1;
    let hbits = b.f(4)? + 1;
    let max_width = b.f(wbits)? + 1;
    let max_height = b.f(hbits)? + 1;
    if !reduced {
        if b.flag()? {
            b.f(4)?;
            b.f(3)?;
        }
    }
    b.f(3)?; // 128x128 superblock, filter intra, intra edge filter
    if !reduced {
        b.f(4)?; // interintra, masked compound, warped motion, dual filter
        let order_hint = b.flag()?;
        if order_hint {
            b.f(2)?; // jnt_comp, ref_frame_mvs
        }
        let force_sct = if b.flag()? { 2 } else { b.f(1)? };
        if force_sct > 0 && !b.flag()? {
            b.f(1)?; // seq_force_integer_mv
        }
        if order_hint {
            b.f(3)?;
        }
    }
    b.f(3)?; // superres, cdef, restoration
    // color_config
    let high = b.flag()?;
    let twelve = profile == 2 && high && b.flag()?;
    let bit_depth = if twelve { 12 } else if high { 10 } else { 8 };
    let mono = profile != 1 && b.flag()?;
    let (mut cp, mut tc, mut mc) = (2, 2, 2);
    if b.flag()? {
        cp = b.f(8)? as u8;
        tc = b.f(8)? as u8;
        mc = b.f(8)? as u8;
    }
    let (full_range, ssx, ssy, csp);
    if mono {
        full_range = b.flag()?;
        (ssx, ssy, csp) = (1, 1, 0);
    } else if cp == 1 && tc == 13 && mc == 0 {
        full_range = true;
        (ssx, ssy, csp) = (0, 0, 0);
    } else {
        full_range = b.flag()?;
        let (x, y) = match profile {
            0 => (1, 1),
            1 => (0, 0),
            _ if bit_depth == 12 => {
                let x = b.f(1)?;
                (x, if x == 1 { b.f(1)? } else { 0 })
            }
            _ => (1, 0),
        };
        (ssx, ssy) = (x, y);
        csp = if x == 1 && y == 1 { b.f(2)? } else { 0 };
    }
    let mut av1c = vec![
        0x81,
        (profile as u8) << 5 | level as u8,
        (tier as u8) << 7 | (high as u8) << 6 | (twelve as u8) << 5 | (mono as u8) << 4 | (ssx as u8) << 3 | (ssy as u8) << 2 | csp as u8,
        0,
    ];
    av1c.extend_from_slice(&obu[o.start..o.end]);
    Some(SequenceInfo { av1c, max_width, max_height, bit_depth, color_primaries: cp, transfer: tc, matrix: mc, full_range })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obu(kind: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = vec![kind << 3 | 2, payload.len() as u8];
        v.extend_from_slice(payload);
        v
    }

    /// Main profile, level 4.0 (8, tier 0), 8-bit 4:2:0, 640x360 with
    /// BT.709 limited range; the bits laid out as in spec 5.5.
    fn sequence_header() -> Vec<u8> {
        let mut bits = String::new();
        let mut put = |v: u32, n: u32| bits.push_str(&format!("{:0w$b}", v, w = n as usize));
        put(0, 3);
        put(0, 1); // still_picture
        put(0, 1); // reduced_still_picture_header
        put(0, 1); // timing_info_present
        put(0, 1); // initial_display_delay_present
        put(0, 5); // operating_points_cnt_minus_1
        put(0, 12); // operating_point_idc
        put(8, 5); // seq_level_idx
        put(0, 1); // seq_tier_0
        put(9, 4); // frame_width_bits_minus_1
        put(8, 4); // frame_height_bits_minus_1
        put(639, 10);
        put(359, 9);
        put(0, 1); // frame_id_numbers_present
        put(0, 3); // 128x128, filter intra, intra edge
        put(0, 4); // interintra, masked, warped, dual filter
        put(1, 1); // enable_order_hint
        put(0, 2); // jnt_comp, ref_frame_mvs
        put(1, 1); // seq_choose_screen_content_tools
        put(1, 1); // seq_choose_integer_mv
        put(6, 3); // order_hint_bits_minus_1
        put(0, 3); // superres, cdef, restoration
        put(0, 1); // high_bitdepth
        put(0, 1); // mono_chrome
        put(1, 1); // color_description_present
        put(1, 8);
        put(1, 8);
        put(1, 8);
        put(0, 1); // color_range
        put(0, 2); // chroma_sample_position
        put(0, 1); // separate_uv_delta_q
        put(0, 1); // film_grain_params_present
        put(1, 1); // trailing bit
        while bits.len() % 8 != 0 {
            bits.push('0');
        }
        let payload: Vec<u8> = bits.as_bytes().chunks(8).map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 2).unwrap()).collect();
        obu(OBU_SEQUENCE_HEADER, &payload)
    }

    #[test]
    fn av1c_from_a_sequence_header() {
        let seq = sequence_header();
        let info = av1c_from_sequence_header(&seq).unwrap();
        assert_eq!((info.max_width, info.max_height, info.bit_depth), (640, 360, 8));
        assert_eq!((info.color_primaries, info.transfer, info.matrix, info.full_range), (1, 1, 1, false));
        assert_eq!(&info.av1c[..4], [0x81, 8, 0x0c, 0]); // 4:2:0
        assert_eq!(&info.av1c[4..], &seq[..]);
    }

    #[test]
    fn temporal_delimiters_and_padding_are_stripped() {
        let frame_key = obu(OBU_FRAME, &[0x10, 1, 2, 3]); // key frame, shown
        let tu = [obu(OBU_TEMPORAL_DELIMITER, &[]), sequence_header(), obu(OBU_PADDING, &[9, 9]), frame_key.clone()].concat();
        let s = sample_from_temporal_unit(&tu).unwrap();
        assert_eq!(s, [sequence_header(), frame_key].concat());
        assert!(is_sync_sample(&s));
        // An inter frame, and a key frame that is not shown, are not sync samples.
        assert!(!is_sync_sample(&obu(OBU_FRAME, &[0x30, 1])));
        assert!(!is_sync_sample(&[sequence_header(), obu(OBU_FRAME, &[0x00, 1])].concat()));
        // A key frame without a sequence header is not one either.
        assert!(!is_sync_sample(&obu(OBU_FRAME, &[0x10, 1])));
    }

    #[test]
    fn last_obu_without_size_field_gets_one() {
        // temporal delimiter, then a frame OBU that runs to the end of the data
        let tu = [obu(OBU_TEMPORAL_DELIMITER, &[]), vec![OBU_FRAME << 3, 0x30, 7, 7]].concat();
        let s = sample_from_temporal_unit(&tu).unwrap();
        assert_eq!(s, [OBU_FRAME << 3, 0x30, 7, 7]);
        assert!(sample_from_temporal_unit(&[OBU_FRAME << 3 | 2, 9, 1]).is_none());
    }
}
