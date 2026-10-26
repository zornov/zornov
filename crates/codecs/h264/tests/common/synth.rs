// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Deterministic synthetic streams built with the syntax writer.

#![allow(dead_code)]

use super::writer::*;

pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn below(&mut self, n: u32) -> u32 {
        (self.next() % n.max(1) as u64) as u32
    }
    pub fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + self.below((hi - lo + 1) as u32) as i32
    }
}

/// PCM macroblock samples: a textured pattern unique to (`seed`, mb).
pub fn pcm_block(rng: &mut Rng, style: u32) -> Vec<u8> {
    let base = rng.below(200) as i32 + 20;
    let gx = rng.range(-6, 6);
    let gy = rng.range(-6, 6);
    let mut v = Vec::with_capacity(384);
    for y in 0..16 {
        for x in 0..16 {
            let n = if style == 0 { rng.range(-8, 8) } else { 0 };
            v.push((base + gx * x + gy * y + n).clamp(1, 254) as u8);
        }
    }
    for _c in 0..2 {
        let cb = rng.below(180) as i32 + 40;
        for y in 0..8 {
            for x in 0..8 {
                v.push((cb + x * 2 - y * 2 + rng.range(-3, 3)).clamp(1, 254) as u8);
            }
        }
    }
    v
}

pub fn all_pcm(rng: &mut Rng, n: usize) -> Vec<Mb> {
    (0..n).map(|_| Mb::Pcm(pcm_block(rng, 0))).collect()
}

pub struct Stream {
    pub name: &'static str,
    pub data: Vec<u8>,
    /// Expected output for streams the generator can predict exactly (no prediction filters).
    pub expected: Option<Vec<u8>>,
}

fn header(sps_c: &SpsCfg, ppss: &[PpsCfg]) -> Vec<u8> {
    let mut out = sps(sps_c);
    for p in ppss {
        out.extend(pps(p, sps_c.mb_w * sps_c.mb_h));
    }
    out
}

/// I_PCM, P_Skip and P_L0_16x16 with motion vectors pointing well outside the picture.
pub fn pcm_mix() -> Stream {
    let mut rng = Rng(11);
    let s = SpsCfg { profile_idc: 66, mb_w: 5, mb_h: 4, num_ref_frames: 1, ..Default::default() };
    let p = PpsCfg::default();
    let n = (s.mb_w * s.mb_h) as usize;
    let mut out = header(&s, &[p.clone()]);
    out.extend(cavlc_slice(&SliceCfg { ..Default::default() }, &s, &p, &all_pcm(&mut rng, n)));
    for f in 1..6u32 {
        let mbs: Vec<Mb> = (0..n)
            .map(|_| match rng.below(4) {
                0 => Mb::Skip,
                1 => Mb::Pcm(pcm_block(&mut rng, 1)),
                _ => Mb::P16x16(0, [rng.range(-90, 90), rng.range(-70, 70)]),
            })
            .collect();
        let sl = SliceCfg {
            slice_type: 0,
            idr: false,
            frame_num: f,
            poc_lsb: 2 * f,
            qp_delta: rng.range(-10, 20),
            ..Default::default()
        };
        out.extend(cavlc_slice(&sl, &s, &p, &mbs));
    }
    Stream { name: "pcm_mix", data: out, expected: None }
}

fn mosaic(rng: &mut Rng, n: usize, nref: u32, mv: i32) -> Vec<Mb> {
    (0..n).map(|_| Mb::P16x16(rng.below(nref), [rng.range(-mv, mv), rng.range(-mv, mv)])).collect()
}

/// Long-term references, every MMCO, list modification with short and long-term pictures.
pub fn long_term() -> Stream {
    let mut rng = Rng(22);
    let s = SpsCfg { mb_w: 4, mb_h: 3, num_ref_frames: 4, log2_max_frame_num: 4, ..Default::default() };
    let p = PpsCfg { num_ref_idx_default: [4, 1], ..Default::default() };
    let n = (s.mb_w * s.mb_h) as usize;
    let mut out = header(&s, &[p.clone()]);
    let pcm = |rng: &mut Rng| all_pcm(rng, n);
    let base = SliceCfg { slice_type: 0, idr: false, ..Default::default() };
    // f0 IDR
    out.extend(cavlc_slice(&SliceCfg { ..Default::default() }, &s, &p, &pcm(&mut rng)));
    // f1: becomes long-term 0 (MMCO 4 then 6)
    out.extend(cavlc_slice(
        &SliceCfg { frame_num: 1, poc_lsb: 2, num_ref_idx: Some([1, 1]), mmcos: Some(vec![Mmco::MaxLong(3), Mmco::CurrentToLong(0)]), ..base.clone() },
        &s,
        &p,
        &pcm(&mut rng),
    ));
    // f2: short-term
    out.extend(cavlc_slice(&SliceCfg { frame_num: 2, poc_lsb: 4, num_ref_idx: Some([1, 1]), ..base.clone() }, &s, &p, &pcm(&mut rng)));
    // f3: f2 -> long-term 1 (MMCO 3)
    out.extend(cavlc_slice(
        &SliceCfg { frame_num: 3, poc_lsb: 6, num_ref_idx: Some([1, 1]), mmcos: Some(vec![Mmco::ShortToLong(0, 1)]), ..base.clone() },
        &s,
        &p,
        &pcm(&mut rng),
    ));
    // f4: non-reference mosaic over [f3, f0, LT0=f1, LT1=f2]
    out.extend(cavlc_slice(&SliceCfg { frame_num: 4, poc_lsb: 8, nal_ref_idc: 0, ..base.clone() }, &s, &p, &mosaic(&mut rng, n, 4, 6)));
    // f5: modification: LT1 first, then picNum 3 (f3), then picNum 0 (f0); then forget LT0 and f0.
    out.extend(cavlc_slice(
        &SliceCfg {
            frame_num: 4,
            poc_lsb: 10,
            modification: [vec![(2, 1), (0, 0), (0, 2)], vec![]],
            mmcos: Some(vec![Mmco::ForgetLong(0), Mmco::ForgetShort(3)]),
            ..base.clone()
        },
        &s,
        &p,
        &mosaic(&mut rng, n, 4, 12),
    ));
    // f6: refs now short f5, f3 and long LT1 (f2); MMCO 4 with 0 removes all long-term.
    out.extend(cavlc_slice(
        &SliceCfg {
            frame_num: 5,
            poc_lsb: 12,
            num_ref_idx: Some([3, 1]),
            modification: [vec![(1, 13), (2, 1)], vec![]],
            mmcos: Some(vec![Mmco::MaxLong(0)]),
            ..base.clone()
        },
        &s,
        &p,
        &mosaic(&mut rng, n, 3, 20),
    ));
    // f7: PCM with MMCO 5 (resets POC and frame_num).
    out.extend(cavlc_slice(
        &SliceCfg { frame_num: 6, poc_lsb: 14, num_ref_idx: Some([1, 1]), mmcos: Some(vec![Mmco::ForgetAll]), ..base.clone() },
        &s,
        &p,
        &pcm(&mut rng),
    ));
    // f8..f10: after MMCO 5 frame_num restarts at 1, POC lsb small.
    for (i, f) in (1..4u32).enumerate() {
        let nr = (i as u32 + 1).min(3);
        out.extend(cavlc_slice(
            &SliceCfg { frame_num: f, poc_lsb: 2 * f, num_ref_idx: Some([nr, 1]), ..base.clone() },
            &s,
            &p,
            &mosaic(&mut rng, n, nr, 9),
        ));
    }
    // IDR with long_term_reference_flag, then P using it via long-term index 0.
    out.extend(cavlc_slice(&SliceCfg { idr_pic_id: 1, long_term_reference_flag: true, ..Default::default() }, &s, &p, &pcm(&mut rng)));
    out.extend(cavlc_slice(&SliceCfg { frame_num: 1, poc_lsb: 2, num_ref_idx: Some([1, 1]), ..base.clone() }, &s, &p, &pcm(&mut rng)));
    out.extend(cavlc_slice(&SliceCfg { frame_num: 2, poc_lsb: 4, num_ref_idx: Some([2, 1]), ..base.clone() }, &s, &p, &mosaic(&mut rng, n, 2, 5)));
    Stream { name: "long_term", data: out, expected: None }
}

fn bi_weights(rng: &mut Rng, n: [usize; 2]) -> (u32, u32, [Vec<(Option<(i32, i32)>, Option<[(i32, i32); 2]>)>; 2]) {
    let ld = rng.below(8);
    let cd = rng.below(8);
    let mut lists: [Vec<_>; 2] = [Vec::new(), Vec::new()];
    for l in 0..2 {
        for _ in 0..n[l] {
            let lw = if rng.below(4) > 0 { Some(((rng.range(-20, 1 << ld.min(6)) + (1 << ld) / 2).clamp(-64, 63), rng.range(-30, 30))) } else { None };
            let cw = if rng.below(4) > 0 {
                Some([
                    ((rng.range(-10, 1 << cd.min(6)) + (1 << cd) / 2).clamp(-64, 63), rng.range(-20, 20)),
                    ((rng.range(-10, 1 << cd.min(6)) + (1 << cd) / 2).clamp(-64, 63), rng.range(-20, 20)),
                ])
            } else {
                None
            };
            lists[l].push((lw, cw));
        }
    }
    (ld, cd, lists)
}

fn b_mbs(rng: &mut Rng, n: usize, nref: [u32; 2], mv: i32, allow_direct: bool) -> Vec<Mb> {
    (0..n)
        .map(|_| {
            let k = rng.below(if allow_direct { 6 } else { 4 });
            let mvd = |rng: &mut Rng| [rng.range(-mv, mv), rng.range(-mv, mv)];
            match k {
                0 | 1 => {
                    let pred = 1 + rng.below(3);
                    let refs = [rng.below(nref[0]), rng.below(nref[1])];
                    let m = [mvd(rng), mvd(rng)];
                    Mb::B16x16(pred, refs, m)
                }
                2 | 3 => {
                    let lo = if allow_direct { 0 } else { 1 };
                    let subs = [rng.range(lo, 12) as u32, rng.range(lo, 12) as u32, rng.range(lo, 12) as u32, rng.range(lo, 12) as u32];
                    let refs = [[rng.below(nref[0]), rng.below(nref[0]), rng.below(nref[0]), rng.below(nref[0])], [
                        rng.below(nref[1]),
                        rng.below(nref[1]),
                        rng.below(nref[1]),
                        rng.below(nref[1]),
                    ]];
                    // mvds in syntax order: all L0 sub-partitions then all L1.
                    let mut m = Vec::new();
                    for l in 0..2 {
                        for &st in &subs {
                            let (pred, num) = match st {
                                0 => (0, 0),
                                1 => (1, 1),
                                2 => (2, 1),
                                3 => (3, 1),
                                4 | 5 => (1, 2),
                                6 | 7 => (2, 2),
                                8 | 9 => (3, 2),
                                10 => (1, 4),
                                11 => (2, 4),
                                _ => (3, 4),
                            };
                            if pred & (1 << l) != 0 {
                                for _ in 0..num {
                                    m.push(mvd(rng));
                                }
                            }
                        }
                    }
                    Mb::B8x8(subs, refs, m)
                }
                4 => Mb::BDirect,
                _ => Mb::Skip,
            }
        })
        .collect()
}

fn p8x8_mbs(rng: &mut Rng, n: usize, nref: u32, mv: i32) -> Vec<Mb> {
    (0..n)
        .map(|_| {
            if rng.below(5) == 0 {
                return Mb::P16x16(rng.below(nref), [rng.range(-mv, mv), rng.range(-mv, mv)]);
            }
            let subs = [rng.below(4), rng.below(4), rng.below(4), rng.below(4)];
            let refs = [rng.below(nref), rng.below(nref), rng.below(nref), rng.below(nref)];
            let mut m = Vec::new();
            for &st in &subs {
                let num = [1, 2, 2, 4][st as usize];
                for _ in 0..num {
                    m.push([rng.range(-mv, mv), rng.range(-mv, mv)]);
                }
            }
            Mb::P8x8(subs, refs, m)
        })
        .collect()
}

/// Explicit weighted bi-prediction (weighted_bipred_idc = 1) plus B sub-partitions and
/// direct prediction, with temporal and spatial direct.
pub fn bipred_explicit() -> Stream {
    let mut rng = Rng(33);
    let s = SpsCfg { mb_w: 5, mb_h: 4, num_ref_frames: 4, reorder: Some((6, 8)), ..Default::default() };
    let p = PpsCfg { num_ref_idx_default: [2, 2], weighted_bipred_idc: 1, weighted_pred: true, ..Default::default() };
    let n = (s.mb_w * s.mb_h) as usize;
    let mut out = header(&s, &[p.clone()]);
    out.extend(cavlc_slice(&SliceCfg { ..Default::default() }, &s, &p, &all_pcm(&mut rng, n)));
    let base = SliceCfg { idr: false, ..Default::default() };
    let w1 = bi_weights(&mut rng, [1, 0]);
    out.extend(cavlc_slice(
        &SliceCfg { slice_type: 0, frame_num: 1, poc_lsb: 16, num_ref_idx: Some([1, 1]), weights: Some(w1), ..base.clone() },
        &s,
        &p,
        &p8x8_mbs(&mut rng, n, 1, 40),
    ));
    let w2 = bi_weights(&mut rng, [2, 0]);
    out.extend(cavlc_slice(
        &SliceCfg { slice_type: 0, frame_num: 2, poc_lsb: 32, weights: Some(w2), ..base.clone() },
        &s,
        &p,
        &p8x8_mbs(&mut rng, n, 2, 30),
    ));
    for (k, poc) in [4u32, 12, 20, 28, 24, 8].iter().enumerate() {
        let w = bi_weights(&mut rng, [2, 2]);
        let mbs = b_mbs(&mut rng, n, [2, 2], 24, true);
        out.extend(cavlc_slice(
            &SliceCfg { slice_type: 1, frame_num: 3, poc_lsb: *poc, nal_ref_idc: 0, direct_spatial: k % 2 == 0, weights: Some(w), ..base.clone() },
            &s,
            &p,
            &mbs,
        ));
    }
    Stream { name: "bipred_explicit", data: out, expected: None }
}

/// direct_8x8_inference_flag = 0 with 4x4 co-located motion, spatial and temporal direct,
/// implicit weighting, multiple references in both lists.
pub fn direct_no_inference() -> Stream {
    let mut rng = Rng(44);
    let s = SpsCfg { mb_w: 5, mb_h: 4, level_idc: 21, num_ref_frames: 3, direct_8x8_inference: false, reorder: Some((6, 8)), ..Default::default() };
    let p0 = PpsCfg { num_ref_idx_default: [2, 2], ..Default::default() };
    let p1 = PpsCfg { pps_id: 1, num_ref_idx_default: [2, 2], weighted_bipred_idc: 2, ..Default::default() };
    let n = (s.mb_w * s.mb_h) as usize;
    let mut out = header(&s, &[p0.clone(), p1.clone()]);
    out.extend(cavlc_slice(&SliceCfg { ..Default::default() }, &s, &p0, &all_pcm(&mut rng, n)));
    let base = SliceCfg { idr: false, ..Default::default() };
    out.extend(cavlc_slice(&SliceCfg { slice_type: 0, frame_num: 1, poc_lsb: 12, num_ref_idx: Some([1, 1]), ..base.clone() }, &s, &p0, &p8x8_mbs(&mut rng, n, 1, 6)));
    out.extend(cavlc_slice(&SliceCfg { slice_type: 0, frame_num: 2, poc_lsb: 24, ..base.clone() }, &s, &p0, &p8x8_mbs(&mut rng, n, 2, 6)));
    for (k, poc) in [6u32, 18, 3, 9, 15, 21].iter().enumerate() {
        let pps = if k >= 3 { &p1 } else { &p0 };
        let mbs = b_mbs(&mut rng, n, [2, 2], 8, true);
        out.extend(cavlc_slice(
            &SliceCfg { slice_type: 1, pps_id: pps.pps_id, frame_num: 3, poc_lsb: *poc, nal_ref_idc: 0, direct_spatial: k % 2 == 1, ..base.clone() },
            &s,
            pps,
            &mbs,
        ));
    }
    Stream { name: "direct_no_inference", data: out, expected: None }
}

/// Several slices per picture with different deblocking modes (idc 0/1/2), offsets and QPs.
pub fn deblock_slices() -> Stream {
    let mut rng = Rng(55);
    let s = SpsCfg { mb_w: 6, mb_h: 5, num_ref_frames: 2, ..Default::default() };
    let p = PpsCfg { num_ref_idx_default: [2, 1], chroma_qp_offset: 3, ..Default::default() };
    let n = s.mb_w * s.mb_h;
    let mut out = header(&s, &[p.clone()]);
    out.extend(cavlc_slice(&SliceCfg { ..Default::default() }, &s, &p, &all_pcm(&mut rng, n as usize)));
    out.extend(cavlc_slice(
        &SliceCfg { slice_type: 0, idr: false, frame_num: 1, poc_lsb: 2, num_ref_idx: Some([1, 1]), ..Default::default() },
        &s,
        &p,
        &all_pcm(&mut rng, n as usize),
    ));
    for f in 2..7u32 {
        let mut first = 0;
        while first < n {
            let len = (1 + rng.below(9)).min(n - first);
            let mbs: Vec<Mb> = (0..len)
                .map(|_| {
                    if rng.below(6) == 0 {
                        Mb::Pcm(pcm_block(&mut rng, 1))
                    } else {
                        Mb::P16x16(rng.below(2), [rng.range(-12, 12), rng.range(-12, 12)])
                    }
                })
                .collect();
            let idc = rng.below(3);
            let sl = SliceCfg {
                first_mb: first,
                slice_type: 0,
                idr: false,
                frame_num: f,
                poc_lsb: 2 * f,
                qp_delta: rng.range(-6, 25),
                deblock: Some((idc, rng.range(-6, 6), rng.range(-6, 6))),
                ..Default::default()
            };
            out.extend(cavlc_slice(&sl, &s, &p, &mbs));
            first += len;
        }
    }
    Stream { name: "deblock_slices", data: out, expected: None }
}

/// frame_num gaps (gaps_in_frame_num_value_allowed_flag = 1) with POC type 2.
pub fn frame_num_gaps() -> Stream {
    let mut rng = Rng(66);
    let s = SpsCfg { profile_idc: 66, mb_w: 4, mb_h: 3, num_ref_frames: 3, gaps_allowed: true, poc_type: 2, ..Default::default() };
    let p = PpsCfg { num_ref_idx_default: [3, 1], ..Default::default() };
    let n = (s.mb_w * s.mb_h) as usize;
    let mut out = header(&s, &[p.clone()]);
    out.extend(cavlc_slice(&SliceCfg { ..Default::default() }, &s, &p, &all_pcm(&mut rng, n)));
    let base = SliceCfg { slice_type: 0, idr: false, ..Default::default() };
    out.extend(cavlc_slice(&SliceCfg { frame_num: 1, num_ref_idx: Some([1, 1]), ..base.clone() }, &s, &p, &all_pcm(&mut rng, n)));
    // Gap: frame_num 2 and 3 missing. Refs then: fn3, fn2 (non-existing), fn1.
    let mbs: Vec<Mb> = (0..n).map(|_| Mb::P16x16(2, [rng.range(-8, 8), rng.range(-8, 8)])).collect();
    out.extend(cavlc_slice(&SliceCfg { frame_num: 4, num_ref_idx: Some([3, 1]), ..base.clone() }, &s, &p, &mbs));
    let mbs: Vec<Mb> = (0..n).map(|_| Mb::P16x16(0, [rng.range(-8, 8), rng.range(-8, 8)])).collect();
    out.extend(cavlc_slice(&SliceCfg { frame_num: 5, num_ref_idx: Some([1, 1]), ..base.clone() }, &s, &p, &mbs));
    // Non-reference picture, then a further gap.
    out.extend(cavlc_slice(&SliceCfg { frame_num: 6, nal_ref_idc: 0, num_ref_idx: Some([1, 1]), ..base.clone() }, &s, &p, &mosaic(&mut rng, n, 1, 4)));
    let mbs: Vec<Mb> = (0..n).map(|_| Mb::P16x16(2, [rng.range(-8, 8), rng.range(-8, 8)])).collect();
    out.extend(cavlc_slice(&SliceCfg { frame_num: 9, num_ref_idx: Some([3, 1]), ..base.clone() }, &s, &p, &mbs));
    Stream { name: "frame_num_gaps", data: out, expected: None }
}

/// FMO (all map types) and ASO with I_PCM / P_Skip content and deblocking disabled, so the
/// expected output is known exactly.
pub fn fmo_aso() -> Stream {
    let mut rng = Rng(77);
    let s = SpsCfg { profile_idc: 66, mb_w: 7, mb_h: 5, num_ref_frames: 1, log2_max_frame_num: 8, ..Default::default() };
    let n = s.mb_w * s.mb_h;
    let w = s.mb_w;
    let groups = vec![
        SliceGroups::Interleaved(vec![3, 5, 2]),
        SliceGroups::Dispersed(4),
        SliceGroups::Foreground(3, vec![(8, 17), (1, 24)]),
        SliceGroups::Evolving(3, false, 2),
        SliceGroups::Evolving(3, true, 3),
        SliceGroups::Evolving(4, false, 5),
        SliceGroups::Evolving(4, true, 4),
        SliceGroups::Evolving(5, false, 3),
        SliceGroups::Evolving(5, true, 6),
        SliceGroups::Explicit(3, (0..n).map(|i| (i * 7 / 3) % 3).collect()),
    ];
    let ppss: Vec<PpsCfg> = groups
        .iter()
        .enumerate()
        .map(|(i, g)| PpsCfg { pps_id: i as u32 + 1, slice_groups: Some(g.clone()), ..Default::default() })
        .collect();
    let plain = PpsCfg::default();
    let mut all = vec![plain.clone()];
    all.extend(ppss.iter().cloned());
    let mut out = header(&s, &all);
    let (pw, ph) = (w as usize * 16, s.mb_h as usize * 16);
    let mut frame = vec![0u8; pw * ph * 3 / 2];
    let mut expected = Vec::new();
    let put_mb = |frame: &mut Vec<u8>, addr: u32, d: &[u8]| {
        let (mx, my) = ((addr % w) as usize, (addr / w) as usize);
        for y in 0..16 {
            for x in 0..16 {
                frame[(my * 16 + y) * pw + mx * 16 + x] = d[y * 16 + x];
            }
        }
        for c in 0..2 {
            let off = pw * ph + c * (pw / 2) * (ph / 2);
            for y in 0..8 {
                for x in 0..8 {
                    frame[off + (my * 8 + y) * (pw / 2) + mx * 8 + x] = d[256 + c * 64 + y * 8 + x];
                }
            }
        }
    };
    let mut frame_num = 0u32;
    for (pi, pp) in ppss.iter().enumerate() {
        // Compute the slice group map like the decoder would (independent implementation).
        let cycle_bits = |rate: u32| 32 - (n.div_ceil(rate) + 1 - 1).leading_zeros();
        let (map, change) = map_for(&groups[pi], w, s.mb_h, rng.below(n) + 1);
        let sgcc = match &groups[pi] {
            SliceGroups::Evolving(_, _, rate) => Some((cycle_bits(*rate), change)),
            _ => None,
        };
        let ngroups = map.iter().max().unwrap() + 1;
        let idr = pi == 0;
        // Two pictures per PPS: an intra one (all PCM) and a P one (PCM in group 0, skips elsewhere).
        for kind in 0..2 {
            let mut slices: Vec<Vec<u8>> = Vec::new();
            for g in 0..ngroups {
                let addrs: Vec<u32> = (0..n).filter(|&a| map[a as usize] == g).collect();
                // Split each group into up to two slices.
                let split = if addrs.len() > 3 { 1 + rng.below(addrs.len() as u32 - 1) as usize } else { addrs.len() };
                for part in [&addrs[..split], &addrs[split..]] {
                    if part.is_empty() {
                        continue;
                    }
                    let intra = kind == 0;
                    let mbs: Vec<Mb> = part
                        .iter()
                        .map(|&a| {
                            if intra || (g == 0 && rng.below(2) == 0) {
                                let d = pcm_block(&mut rng, 0);
                                put_mb(&mut frame, a, &d);
                                Mb::Pcm(d)
                            } else {
                                Mb::Skip
                            }
                        })
                        .collect();
                    let sl = SliceCfg {
                        first_mb: part[0],
                        slice_type: if intra { 2 } else { 0 },
                        pps_id: pp.pps_id,
                        idr: idr && kind == 0,
                        frame_num,
                        poc_lsb: (2 * frame_num) % 64,
                        nal_ref_idc: 3,
                        deblock: Some((1, 0, 0)),
                        slice_group_change_cycle: sgcc,
                        num_ref_idx: if intra { None } else { Some([1, 1]) },
                        ..Default::default()
                    };
                    slices.push(cavlc_slice(&sl, &s, pp, &mbs));
                }
            }
            // ASO: shuffle slice order within the picture (Baseline allows it).
            for i in (1..slices.len()).rev() {
                let j = rng.below(i as u32 + 1) as usize;
                slices.swap(i, j);
            }
            for sl in slices {
                out.extend(sl);
            }
            expected.extend_from_slice(&frame);
            frame_num += 1;
        }
    }
    Stream { name: "fmo_aso", data: out, expected: Some(expected) }
}

/// Independent slice group map computation (8.2.2) for the generator.
fn map_for(g: &SliceGroups, w: u32, h: u32, cycle: u32) -> (Vec<u32>, u32) {
    let n = (w * h) as usize;
    let mut map = vec![0u32; n];
    match g {
        SliceGroups::Interleaved(runs) => {
            let mut i = 0usize;
            while i < n {
                for (gi, &r) in runs.iter().enumerate() {
                    for j in 0..r as usize {
                        if i + j < n {
                            map[i + j] = gi as u32;
                        }
                    }
                    i += r as usize;
                    if i >= n {
                        break;
                    }
                }
            }
            (map, 0)
        }
        SliceGroups::Dispersed(num) => {
            for i in 0..n as u32 {
                map[i as usize] = ((i % w) + (((i / w) * num) / 2)) % num;
            }
            (map, 0)
        }
        SliceGroups::Foreground(num, rects) => {
            map.fill(num - 1);
            for (gi, &(tl, br)) in rects.iter().enumerate().rev() {
                for y in tl / w..=br / w {
                    for x in tl % w..=br % w {
                        map[(y * w + x) as usize] = gi as u32;
                    }
                }
            }
            (map, 0)
        }
        SliceGroups::Evolving(t, dir, rate) => {
            let d = *dir as i32;
            let cycle = cycle.min((n as u32).div_ceil(*rate));
            let units0 = ((cycle * rate) as usize).min(n);
            match t {
                3 => {
                    map.fill(1);
                    let (wi, hi) = (w as i32, h as i32);
                    let (mut x, mut y) = ((wi - d) / 2, (hi - d) / 2);
                    let (mut l, mut tp, mut r, mut b) = (x, y, x, y);
                    let (mut xd, mut yd) = (d - 1, d);
                    let mut k = 0;
                    while k < units0 {
                        let i = (y * wi + x) as usize;
                        let vac = map[i] == 1;
                        if vac {
                            map[i] = 0;
                        }
                        if xd == -1 && x == l {
                            l = (l - 1).max(0);
                            x = l;
                            xd = 0;
                            yd = 2 * d - 1;
                        } else if xd == 1 && x == r {
                            r = (r + 1).min(wi - 1);
                            x = r;
                            xd = 0;
                            yd = 1 - 2 * d;
                        } else if yd == -1 && y == tp {
                            tp = (tp - 1).max(0);
                            y = tp;
                            xd = 1 - 2 * d;
                            yd = 0;
                        } else if yd == 1 && y == b {
                            b = (b + 1).min(hi - 1);
                            y = b;
                            xd = 2 * d - 1;
                            yd = 0;
                        } else {
                            x += xd;
                            y += yd;
                        }
                        if vac {
                            k += 1;
                        }
                    }
                }
                4 => {
                    let upper = if d == 1 { n - units0 } else { units0 };
                    for i in 0..n {
                        map[i] = if i < upper { d as u32 } else { 1 - d as u32 };
                    }
                }
                _ => {
                    let upper = if d == 1 { n - units0 } else { units0 };
                    let mut k = 0;
                    for j in 0..w as usize {
                        for i in 0..h as usize {
                            map[i * w as usize + j] = if k < upper { d as u32 } else { 1 - d as u32 };
                            k += 1;
                        }
                    }
                }
            }
            (map, cycle)
        }
        SliceGroups::Explicit(_, ids) => (ids.clone(), 0),
    }
}

/// Redundant pictures (must be ignored) and an IDR with no_output_of_prior_pics_flag.
pub fn redundant_and_no_output() -> Stream {
    redundant_impl(true)
}

/// The same stream without the redundant pictures (reference for `redundant`).
pub fn redundant_clean() -> Stream {
    redundant_impl(false)
}

fn redundant_impl(with_redundant: bool) -> Stream {
    let mut rng = Rng(88);
    let s = SpsCfg { profile_idc: 66, mb_w: 3, mb_h: 2, num_ref_frames: 1, ..Default::default() };
    let p = PpsCfg { redundant_pic_cnt_present: true, ..Default::default() };
    let n = (s.mb_w * s.mb_h) as usize;
    let mut out = header(&s, &[p.clone()]);
    let base = SliceCfg { slice_type: 0, idr: false, num_ref_idx: Some([1, 1]), ..Default::default() };
    out.extend(cavlc_slice(&SliceCfg { ..Default::default() }, &s, &p, &all_pcm(&mut rng, n)));
    let red = cavlc_slice(&SliceCfg { redundant_pic_cnt: Some(1), ..Default::default() }, &s, &p, &all_pcm(&mut rng, n));
    if with_redundant {
        out.extend(red);
    }
    out.extend(cavlc_slice(&SliceCfg { frame_num: 1, poc_lsb: 2, ..base.clone() }, &s, &p, &mosaic(&mut rng, n, 1, 10)));
    let red = cavlc_slice(&SliceCfg { frame_num: 1, poc_lsb: 2, redundant_pic_cnt: Some(2), ..base.clone() }, &s, &p, &all_pcm(&mut rng, n));
    if with_redundant {
        out.extend(red);
    }
    out.extend(cavlc_slice(&SliceCfg { frame_num: 2, poc_lsb: 4, ..base.clone() }, &s, &p, &mosaic(&mut rng, n, 1, 10)));
    Stream { name: if with_redundant { "redundant" } else { "redundant_clean" }, data: out, expected: None }
}

pub fn all() -> Vec<Stream> {
    vec![pcm_mix(), long_term(), bipred_explicit(), direct_no_inference(), deblock_slices(), frame_num_gaps(), fmo_aso(), redundant_and_no_output(), redundant_clean(), cabac_pcm(), paff_direct(), paff_marking()]
}

/// CABAC I_PCM in I, P and B slices with P_Skip / B_Skip, cabac_init_idc 0..2.
pub fn cabac_pcm() -> Stream {
    let mut rng = Rng(99);
    let s = SpsCfg { mb_w: 5, mb_h: 3, num_ref_frames: 2, reorder: Some((1, 3)), ..Default::default() };
    let p = PpsCfg { cabac: true, num_ref_idx_default: [1, 1], ..Default::default() };
    let n = (s.mb_w * s.mb_h) as usize;
    let mut out = header(&s, &[p.clone()]);
    out.extend(cabac_pcm_slice(&SliceCfg { qp_delta: 5, ..Default::default() }, &s, &p, &all_pcm(&mut rng, n)));
    let mut fnum = 1;
    for k in 0..6u32 {
        let mbs: Vec<Mb> = (0..n).map(|_| if rng.below(3) == 0 { Mb::Pcm(pcm_block(&mut rng, 0)) } else { Mb::Skip }).collect();
        let sl = SliceCfg {
            slice_type: 0,
            idr: false,
            frame_num: fnum,
            poc_lsb: 4 * fnum,
            cabac_init_idc: k % 3,
            qp_delta: rng.range(-20, 20),
            ..Default::default()
        };
        out.extend(cabac_pcm_slice(&sl, &s, &p, &mbs));
        // A B picture between the last two P pictures.
        let mbs: Vec<Mb> = (0..n).map(|_| if rng.below(3) == 0 { Mb::Pcm(pcm_block(&mut rng, 0)) } else { Mb::Skip }).collect();
        let sl = SliceCfg {
            slice_type: 1,
            idr: false,
            frame_num: fnum + 1,
            poc_lsb: 4 * fnum - 2,
            nal_ref_idc: 0,
            cabac_init_idc: (k + 1) % 3,
            direct_spatial: k % 2 == 0,
            qp_delta: rng.range(-20, 20),
            ..Default::default()
        };
        if k > 0 {
            out.extend(cabac_pcm_slice(&sl, &s, &p, &mbs));
        }
        fnum += 1;
    }
    Stream { name: "cabac_pcm", data: out, expected: None }
}

/// Macroblocks for a P picture: PCM with probability 1/`pcm_every`, otherwise P_L0_16x16
/// with a reference index from `allowed`.
fn p_mix(rng: &mut Rng, n: usize, allowed: &[u32], mv: i32, pcm_every: u32) -> Vec<Mb> {
    (0..n)
        .map(|_| {
            if pcm_every > 0 && rng.below(pcm_every) == 0 {
                Mb::Pcm(pcm_block(rng, 1))
            } else {
                let r = allowed[rng.below(allowed.len() as u32) as usize];
                Mb::P16x16(r, [rng.range(-mv, mv), rng.range(-mv, mv)])
            }
        })
        .collect()
}

/// Field pictures (PAFF) mixed with frame pictures: P fields, B fields and B frames with
/// temporal and spatial direct prediction across all frame/field co-location cases
/// (FLD/FLD, FLD/FRM, FRM/FLD), and implicit weighted prediction for fields.
pub fn paff_direct() -> Stream {
    let mut rng = Rng(111);
    let s = SpsCfg {
        mb_w: 5,
        mb_h: 2,
        num_ref_frames: 4,
        frame_mbs_only: false,
        log2_max_poc_lsb: 8,
        reorder: Some((6, 8)),
        ..Default::default()
    };
    let p = PpsCfg { num_ref_idx_default: [2, 2], ..Default::default() };
    let pi = PpsCfg { pps_id: 1, num_ref_idx_default: [2, 2], weighted_bipred_idc: 2, ..Default::default() };
    let nf = (s.mb_w * s.mb_h) as usize; // MBs per field
    let nfr = 2 * nf; // MBs per frame
    let mut out = header(&s, &[p.clone(), pi.clone()]);
    let base = SliceCfg { idr: false, ..Default::default() };
    // a: IDR top field (I) + bottom I field.
    out.extend(cavlc_slice(&SliceCfg { field: Some(false), poc_lsb: 0, ..Default::default() }, &s, &p, &all_pcm(&mut rng, nf)));
    out.extend(cavlc_slice(&SliceCfg { field: Some(true), poc_lsb: 1, ..base.clone() }, &s, &p, &all_pcm(&mut rng, nf)));
    // b: P frame (frame picture) referencing frame a.
    out.extend(cavlc_slice(
        &SliceCfg { slice_type: 0, frame_num: 1, poc_lsb: 16, num_ref_idx: Some([1, 1]), ..base.clone() },
        &s,
        &p,
        &p_mix(&mut rng, nfr, &[0], 14, 6),
    ));
    // c: P field pair. Top: [b.top, b.bot, a.top, a.bot]; bottom: [b.bot, c.top, a.bot, b.top, a.top]
    // (the bottom field avoids c.top so co-located references stay in later lists).
    out.extend(cavlc_slice(
        &SliceCfg { slice_type: 0, frame_num: 2, poc_lsb: 32, field: Some(false), num_ref_idx: Some([4, 1]), ..base.clone() },
        &s,
        &p,
        &p_mix(&mut rng, nf, &[0, 1, 2, 3], 10, 5),
    ));
    out.extend(cavlc_slice(
        &SliceCfg { slice_type: 0, frame_num: 2, poc_lsb: 33, field: Some(true), num_ref_idx: Some([5, 1]), ..base.clone() },
        &s,
        &p,
        &p_mix(&mut rng, nf, &[0, 2, 3, 4], 10, 5),
    ));
    // d: B frame between b (16) and c (32): co-located c is field coded (FRM <- FLD).
    for (k, (poc, spatial, pps)) in [(24u32, false, &p), (28, true, &pi)].iter().enumerate() {
        let mbs = b_mbs(&mut rng, nfr, [2, 2], 6, true);
        let _ = k;
        out.extend(cavlc_slice(
            &SliceCfg { slice_type: 1, pps_id: pps.pps_id, frame_num: 3, poc_lsb: *poc, nal_ref_idc: 0, direct_spatial: *spatial, ..base.clone() },
            &s,
            pps,
            &mbs,
        ));
    }
    // e: B field pair between b and c: L0 = fields of b, a; L1[0] = field of c (FLD <- FLD).
    for (poc, bottom, spatial, pps) in [(20u32, false, false, &p), (21, true, false, &p), (22, false, true, &pi), (23, true, true, &pi)] {
        let mbs = b_mbs(&mut rng, nf, [4, 2], 6, true);
        out.extend(cavlc_slice(
            &SliceCfg {
                slice_type: 1,
                pps_id: pps.pps_id,
                frame_num: 3,
                poc_lsb: poc,
                field: Some(bottom),
                nal_ref_idc: 0,
                direct_spatial: spatial,
                num_ref_idx: Some([4, 2]),
                ..base.clone()
            },
            &s,
            pps,
            &mbs,
        ));
    }
    // f: B field pair between a and b: L1[0] = field of the frame coded b (FLD <- FRM).
    for (poc, bottom, spatial) in [(8u32, false, false), (9, true, false), (10, false, true), (11, true, true)] {
        let mbs = b_mbs(&mut rng, nf, [2, 2], 6, true);
        out.extend(cavlc_slice(
            &SliceCfg {
                slice_type: 1,
                frame_num: 3,
                poc_lsb: poc,
                field: Some(bottom),
                nal_ref_idc: 0,
                direct_spatial: spatial,
                num_ref_idx: Some([2, 2]),
                ..base.clone()
            },
            &s,
            &p,
            &mbs,
        ));
    }
    Stream { name: "paff_direct", data: out, expected: None }
}

/// Reference marking of fields: long-term IDR field pair, MMCO 1/2/3/4 on fields,
/// list modification with field picture numbers, frames referencing field pairs.
pub fn paff_marking() -> Stream {
    let mut rng = Rng(222);
    let s = SpsCfg { mb_w: 4, mb_h: 2, num_ref_frames: 4, frame_mbs_only: false, log2_max_poc_lsb: 8, ..Default::default() };
    let p = PpsCfg { num_ref_idx_default: [1, 1], ..Default::default() };
    let nf = (s.mb_w * s.mb_h) as usize;
    let mut out = header(&s, &[p.clone()]);
    let base = SliceCfg { idr: false, slice_type: 0, ..Default::default() };
    let all = |n: u32| (0..n).collect::<Vec<u32>>();
    // F0: IDR long-term frame (LongTermFrameIdx 0); its fields are referenced below.
    // (A long-term IDR *field pair*, second field marked by MMCO 6, is legal but ffmpeg
    // drops one of its fields from the long-term list, so it cannot serve as the oracle.)
    out.extend(cavlc_slice(&SliceCfg { long_term_reference_flag: true, ..Default::default() }, &s, &p, &all_pcm(&mut rng, 2 * nf)));
    // F1 top: [F0.top, F0.bot]; bottom: [F1.top, F0.bot, F0.top].
    out.extend(cavlc_slice(
        &SliceCfg { frame_num: 1, poc_lsb: 4, field: Some(false), num_ref_idx: Some([2, 1]), ..base.clone() },
        &s,
        &p,
        &p_mix(&mut rng, nf, &all(2), 8, 4),
    ));
    out.extend(cavlc_slice(
        &SliceCfg { frame_num: 1, poc_lsb: 5, field: Some(true), num_ref_idx: Some([3, 1]), ..base.clone() },
        &s,
        &p,
        &p_mix(&mut rng, nf, &all(3), 8, 4),
    ));
    // F2 top: [F1.top, F1.bot, F0.top, F0.bot]; then MMCO 4 (max idx 2), MMCO 1 (F1.bot,
    // PicNum 2), MMCO 3 (F1.top, PicNum 3 -> LongTermFrameIdx 1).
    out.extend(cavlc_slice(
        &SliceCfg {
            frame_num: 2,
            poc_lsb: 8,
            field: Some(false),
            num_ref_idx: Some([4, 1]),
            mmcos: Some(vec![Mmco::MaxLong(3), Mmco::ForgetShort(2), Mmco::ShortToLong(1, 1)]),
            ..base.clone()
        },
        &s,
        &p,
        &p_mix(&mut rng, nf, &all(4), 8, 4),
    ));
    // F2 bottom: [F2.top, F0.bot, F0.top, F1.top].
    out.extend(cavlc_slice(
        &SliceCfg { frame_num: 2, poc_lsb: 9, field: Some(true), num_ref_idx: Some([4, 1]), ..base.clone() },
        &s,
        &p,
        &p_mix(&mut rng, nf, &all(4), 8, 4),
    ));
    // F3 top: init [F2.top, F2.bot, F0.top, F0.bot, F1.top]; modification: long-term pic num 3
    // (F1.top) first, then PicNum 4 (F2.bot): [F1.top, F2.bot, F2.top, F0.top, F0.bot].
    // MMCO 2 removes F0.bot (LongTermPicNum 0).
    out.extend(cavlc_slice(
        &SliceCfg {
            frame_num: 3,
            poc_lsb: 12,
            field: Some(false),
            num_ref_idx: Some([5, 1]),
            modification: [vec![(2, 3), (0, 2)], vec![]],
            mmcos: Some(vec![Mmco::ForgetLong(0)]),
            ..base.clone()
        },
        &s,
        &p,
        &p_mix(&mut rng, nf, &all(5), 8, 5),
    ));
    // F3 bottom: [F2.bot, F3.top, F2.top, F0.top, F1.top].
    out.extend(cavlc_slice(
        &SliceCfg { frame_num: 3, poc_lsb: 13, field: Some(true), num_ref_idx: Some([5, 1]), ..base.clone() },
        &s,
        &p,
        &p_mix(&mut rng, nf, &all(5), 8, 5),
    ));
    // F4: frame picture; only frames with both fields marked qualify: [F3, F2].
    // MMCO 4 with 0 removes all long-term fields.
    out.extend(cavlc_slice(
        &SliceCfg { frame_num: 4, poc_lsb: 16, num_ref_idx: Some([2, 1]), mmcos: Some(vec![Mmco::MaxLong(0)]), ..base.clone() },
        &s,
        &p,
        &p_mix(&mut rng, 2 * nf, &all(2), 10, 6),
    ));
    // F5: field pair over the short-term frames F4, F3, F2 (sliding window).
    out.extend(cavlc_slice(
        &SliceCfg { frame_num: 5, poc_lsb: 20, field: Some(false), num_ref_idx: Some([6, 1]), ..base.clone() },
        &s,
        &p,
        &p_mix(&mut rng, nf, &all(6), 10, 6),
    ));
    out.extend(cavlc_slice(
        &SliceCfg { frame_num: 5, poc_lsb: 21, field: Some(true), num_ref_idx: Some([7, 1]), ..base.clone() },
        &s,
        &p,
        &p_mix(&mut rng, nf, &all(7), 10, 6),
    ));
    // F6, F7: more field pairs to exercise the sliding window with fields.
    for f in 6..8u32 {
        out.extend(cavlc_slice(
            &SliceCfg { frame_num: f, poc_lsb: 4 * f, field: Some(false), num_ref_idx: Some([8, 1]), ..base.clone() },
            &s,
            &p,
            &p_mix(&mut rng, nf, &all(8), 10, 6),
        ));
        out.extend(cavlc_slice(
            // The top field's sliding window dropped the oldest pair: 7 fields left.
            &SliceCfg { frame_num: f, poc_lsb: 4 * f + 1, field: Some(true), num_ref_idx: Some([7, 1]), ..base.clone() },
            &s,
            &p,
            &p_mix(&mut rng, nf, &all(7), 10, 6),
        ));
    }
    Stream { name: "paff_marking", data: out, expected: None }
}
