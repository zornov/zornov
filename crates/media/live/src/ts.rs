// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! MPEG-2 transport stream muxing of H.264 and AAC (ADTS): what SRT (and
//! most broadcast links) carry. 188-byte packets, PAT and PMT repeated,
//! PCR on the picture, 90 kHz timestamps.

use crate::av::Packet;

const PID_PMT: u16 = 0x1000;
const PID_VIDEO: u16 = 0x100;
const PID_AUDIO: u16 = 0x101;

pub struct TsMuxer {
    sps: Vec<u8>,
    pps: Vec<u8>,
    sample_rate: u32,
    cc: [u8; 4],
    since_psi: u32,
    /// Output so far, waiting to be taken.
    out: Vec<u8>,
}

fn crc32_mpeg(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= (b as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 { (crc << 1) ^ 0x04C1_1DB7 } else { crc << 1 };
        }
    }
    crc
}

impl TsMuxer {
    pub fn new(sps: &[u8], pps: &[u8], sample_rate: u32) -> TsMuxer {
        TsMuxer { sps: sps.to_vec(), pps: pps.to_vec(), sample_rate, cc: [0; 4], since_psi: u32::MAX, out: Vec::new() }
    }

    fn cc_of(&mut self, pid: u16) -> u8 {
        let i = match pid {
            0 => 0,
            PID_PMT => 1,
            PID_VIDEO => 2,
            _ => 3,
        };
        let c = self.cc[i];
        self.cc[i] = (c + 1) & 15;
        c
    }

    fn psi(&mut self, pid: u16, table: &[u8]) {
        let mut pkt = vec![0xFFu8; 188];
        pkt[0] = 0x47;
        pkt[1] = 0x40 | (pid >> 8) as u8;
        pkt[2] = pid as u8;
        pkt[3] = 0x10 | self.cc_of(pid);
        pkt[4] = 0; // pointer field
        pkt[5..5 + table.len()].copy_from_slice(table);
        self.out.extend_from_slice(&pkt);
    }

    fn write_psi(&mut self) {
        // PAT: program 1 → PMT pid.
        let mut pat = vec![0x00, 0xB0, 13, 0x00, 0x01, 0xC1, 0x00, 0x00, 0x00, 0x01, 0xE0 | (PID_PMT >> 8) as u8, PID_PMT as u8];
        let c = crc32_mpeg(&pat);
        pat.extend_from_slice(&c.to_be_bytes());
        self.psi(0, &pat);
        // PMT: H.264 on the video pid (also the PCR pid), ADTS AAC on audio.
        let mut pmt = vec![0x02, 0xB0, 0, 0x00, 0x01, 0xC1, 0x00, 0x00, 0xE0 | (PID_VIDEO >> 8) as u8, PID_VIDEO as u8, 0xF0, 0x00];
        pmt.extend_from_slice(&[0x1B, 0xE0 | (PID_VIDEO >> 8) as u8, PID_VIDEO as u8, 0xF0, 0x00]);
        pmt.extend_from_slice(&[0x0F, 0xE0 | (PID_AUDIO >> 8) as u8, PID_AUDIO as u8, 0xF0, 0x00]);
        let len = pmt.len() - 3 + 4;
        pmt[1] = 0xB0 | (len >> 8) as u8;
        pmt[2] = len as u8;
        let c = crc32_mpeg(&pmt);
        pmt.extend_from_slice(&c.to_be_bytes());
        self.psi(PID_PMT, &pmt);
    }

    fn pes(&mut self, pid: u16, stream_id: u8, pts_us: i64, payload: &[u8], pcr: bool, unbounded: bool) {
        // One second of headroom so decoders never see a negative time.
        let pts = (((pts_us.max(0) as i128 * 90_000 / 1_000_000) as u64) + 90_000) & 0x1_FFFF_FFFF;
        let mut pes = vec![0, 0, 1, stream_id, 0, 0, 0x80, 0x80, 5];
        pes.push(0x21 | ((pts >> 29) & 0x0E) as u8);
        pes.push((pts >> 22) as u8);
        pes.push(0x01 | ((pts >> 14) & 0xFE) as u8);
        pes.push((pts >> 7) as u8);
        pes.push(0x01 | ((pts << 1) & 0xFE) as u8);
        if !unbounded {
            let l = pes.len() - 6 + payload.len();
            if l <= 0xFFFF {
                pes[4] = (l >> 8) as u8;
                pes[5] = l as u8;
            }
        }
        pes.extend_from_slice(payload);
        let mut off = 0;
        let mut first = true;
        while off < pes.len() {
            let mut pkt = [0xFFu8; 188];
            pkt[0] = 0x47;
            pkt[1] = (if first { 0x40 } else { 0 }) | (pid >> 8) as u8;
            pkt[2] = pid as u8;
            let cc = self.cc_of(pid);
            let remaining = pes.len() - off;
            let with_pcr = first && pcr;
            if remaining >= 184 && !with_pcr {
                pkt[3] = 0x10 | cc;
                pkt[4..].copy_from_slice(&pes[off..off + 184]);
                off += 184;
            } else {
                // An adaptation field: carries the PCR, or pads a short last packet.
                let take = remaining.min(184 - 1 - if with_pcr { 7 } else { 0 });
                let af_len = 183 - take;
                pkt[3] = 0x30 | cc;
                pkt[4] = af_len as u8;
                if af_len > 0 {
                    pkt[5] = if with_pcr { 0x10 } else { 0 };
                    if with_pcr {
                        let b = pts.saturating_sub(45_000) & 0x1_FFFF_FFFF;
                        pkt[6..12].copy_from_slice(&[(b >> 25) as u8, (b >> 17) as u8, (b >> 9) as u8, (b >> 1) as u8, ((b & 1) << 7) as u8 | 0x7E, 0]);
                    }
                }
                pkt[188 - take..].copy_from_slice(&pes[off..off + take]);
                off += take;
            }
            self.out.extend_from_slice(&pkt);
            first = false;
        }
    }

    /// Adds one packet; the bytes to send so far come from [`TsMuxer::take`].
    pub fn push(&mut self, p: &Packet) {
        match p {
            Packet::Video { data, keyframe, pts_us } => {
                if *keyframe || self.since_psi > 30 {
                    self.write_psi();
                    self.since_psi = 0;
                }
                self.since_psi += 1;
                let mut es = vec![0, 0, 0, 1, 0x09, 0xF0]; // access unit delimiter
                if *keyframe {
                    es.extend_from_slice(&[0, 0, 0, 1]);
                    es.extend_from_slice(&self.sps);
                    es.extend_from_slice(&[0, 0, 0, 1]);
                    es.extend_from_slice(&self.pps);
                }
                let mut i = 0;
                while i + 4 <= data.len() {
                    let n = u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
                    i += 4;
                    if i + n > data.len() {
                        break;
                    }
                    es.extend_from_slice(&[0, 0, 0, 1]);
                    es.extend_from_slice(&data[i..i + n]);
                    i += n;
                }
                self.pes(PID_VIDEO, 0xE0, *pts_us, &es, true, true);
            }
            Packet::Audio { data, pts_us } => {
                let sf = match self.sample_rate {
                    48_000 => 3,
                    44_100 => 4,
                    32_000 => 5,
                    _ => 3,
                };
                let len = data.len() + 7;
                let mut adts = vec![0xFF, 0xF1, (1 << 6) | (sf << 2), 0x80 | (len >> 11) as u8 & 0x03, (len >> 3) as u8, ((len & 7) << 5) as u8 | 0x1F, 0xFC];
                adts[3] = 0x80 | ((len >> 11) & 0x03) as u8;
                adts.extend_from_slice(data);
                self.pes(PID_AUDIO, 0xC0, *pts_us, &adts, false, false);
            }
        }
    }

    pub fn take(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::av::{AvConfig, AvEncoder};
    use crate::source::{AudioSource, TestAudio, TestVideo, VideoSource};

    pub(crate) fn make_ts(frames: u32) -> (Vec<u8>, AvConfig) {
        let mut cfg = AvConfig::new(320, 180, 30.0);
        cfg.gop_secs = 1.0;
        let mut enc = AvEncoder::new(cfg).unwrap();
        let mut mux = TsMuxer::new(enc.sps(), enc.pps(), 48000);
        let mut v = TestVideo::from_spec("counter", 320, 180, 30.0).unwrap();
        let mut a = TestAudio::from_spec("tone:440", 48000).unwrap();
        for n in 0..frames {
            mux.push(&enc.push_frame(&v.frame_at((n as f64 * 1e6 / 30.0) as i64).unwrap()));
            for p in enc.push_audio(&a.read(1600)) {
                mux.push(&p);
            }
        }
        (mux.take(), cfg)
    }

    #[test]
    fn packets_are_well_formed_and_counters_continuous() {
        let (ts, _) = make_ts(45);
        assert_eq!(ts.len() % 188, 0);
        let mut last_cc: std::collections::HashMap<u16, u8> = Default::default();
        let mut pids = std::collections::BTreeSet::new();
        for p in ts.chunks(188) {
            assert_eq!(p[0], 0x47);
            let pid = ((p[1] as u16 & 0x1F) << 8) | p[2] as u16;
            pids.insert(pid);
            let has_payload = p[3] & 0x10 != 0;
            let cc = p[3] & 15;
            if let Some(prev) = last_cc.get(&pid) {
                assert_eq!(cc, (prev + 1) & 15, "pid {pid:#x} continuity");
            }
            if has_payload {
                last_cc.insert(pid, cc);
            }
        }
        assert_eq!(pids.into_iter().collect::<Vec<_>>(), [0, 0x100, 0x101, 0x1000]);
        assert_eq!(crc32_mpeg(b"123456789"), 0x0376_E6E7);
    }
}
