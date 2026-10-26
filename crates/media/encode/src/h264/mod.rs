// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! H.264 / AVC encoder.
//!
//! Output samples are AVCC framed (4-byte big-endian NAL lengths), ready for
//! an MP4 `avc1` track whose `avcC` comes from [`Encoder::avcc`]. Every frame
//! is a reference frame (IDR or P, no B frames), so decode order equals
//! presentation order and no `ctts` box is needed.
//!
//! Coding tools: one reference picture, P_L0_16x16 / P_Skip with half-sample
//! motion vectors, Intra 4x4 (9 modes) / Intra 16x16 (4 modes), 4:2:0 chroma
//! intra prediction, 4x4 integer transform, flat quantisation, CAVLC
//! (Baseline) or CABAC (Main) entropy coding and the in-loop deblocking filter.
//! Each frame is split into horizontal slices that are coded in parallel.

mod cabac;
mod cabac_tables;
mod cavlc;
mod deblock;
mod mb;
mod params;
mod pred;
mod rc;
mod slice;
mod tables;
mod transform;

pub use params::{pick_level, ColorDesc};
pub use rc::RateMode;

use crate::bits::{nal_unit, push_length_prefixed};
use crate::yuv::Yuv420;
use deblock::deblock_frame;
use mb::{FrameIn, Recon};
use params::{SeqParams, SliceHeader};
use pred::RefPic;

/// H.264 profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    /// Constrained Baseline (profile_idc 66, CAVLC).
    Baseline,
    /// Main (profile_idc 77, CABAC).
    Main,
}

#[derive(Clone, Debug)]
pub struct Config {
    /// Picture size in pixels (even values are best; odd sizes are cropped).
    pub width: u32,
    pub height: u32,
    /// Frame rate as a fraction (30000/1001 for 29.97).
    pub fps_num: u32,
    pub fps_den: u32,
    pub profile: Profile,
    /// Target bit rate in kbit/s.
    pub bitrate_kbps: u32,
    pub rate_mode: RateMode,
    /// Distance between IDR frames, in frames (0 = only the first frame).
    pub gop: u32,
    /// Maximum number of slices per frame (parallelism); 0 = one per CPU.
    pub max_slices: u32,
    /// Use Intra 4x4 prediction (slower, better intra quality).
    pub intra4x4: bool,
    pub color: ColorDesc,
}

impl Config {
    /// Defaults: Main profile, VBR at a bit rate suiting the size, one IDR per second.
    pub fn new(width: u32, height: u32, fps_num: u32, fps_den: u32) -> Config {
        let fps = fps_num as f64 / fps_den.max(1) as f64;
        Config {
            width,
            height,
            fps_num,
            fps_den,
            profile: Profile::Main,
            bitrate_kbps: recommended_kbps(width, height, fps),
            rate_mode: RateMode::Vbr,
            gop: fps.round().max(1.0) as u32,
            max_slices: 0,
            intra4x4: true,
            color: ColorDesc::BT709,
        }
    }

    pub fn fps(&self) -> f64 {
        self.fps_num as f64 / self.fps_den.max(1) as f64
    }
}

/// A bit rate (kbit/s) that gives good quality for this size and rate:
/// about 0.15 bit per pixel.
pub fn recommended_kbps(width: u32, height: u32, fps: f64) -> u32 {
    ((width as f64 * height as f64 * fps * 0.15) / 1000.0).round().clamp(100.0, 400_000.0) as u32
}

/// One coded frame.
#[derive(Clone, Debug)]
pub struct Frame {
    /// AVCC framed NAL units (4-byte length prefixes).
    pub data: Vec<u8>,
    pub keyframe: bool,
    pub qp: u8,
}

/// Track description for a muxer.
#[derive(Clone, Debug)]
pub struct TrackInfo {
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    /// `avcC` record, or `hvcC` when `hevc`.
    pub avcc: Vec<u8>,
    pub color: ColorDesc,
    pub bitrate: u32,
    /// The stream is HEVC (from the hardware encoder): written as `hvc1`.
    pub hevc: bool,
}

pub struct Encoder {
    cfg: Config,
    mb_w: usize,
    mb_h: usize,
    seq: SeqParams,
    sps_nal: Vec<u8>,
    pps_nal: Vec<u8>,
    refp: Option<RefPic>,
    prev_mv: Vec<[i16; 2]>,
    frames: u64,
    since_idr: u32,
    frame_num: u32,
    idr_count: u32,
    force_key: bool,
    rc: rc::RateControl,
    slices: usize,
    recon: Option<Yuv420>,
    keep_recon: bool,
}

impl Encoder {
    pub fn new(cfg: Config) -> Result<Encoder, String> {
        if cfg.width < 16 || cfg.height < 16 || cfg.width > 8192 || cfg.height > 8192 {
            return Err(format!("unsupported picture size {}x{}", cfg.width, cfg.height));
        }
        if cfg.fps_num == 0 || cfg.fps_den == 0 {
            return Err("invalid frame rate".into());
        }
        let mb_w = cfg.width.div_ceil(16) as usize;
        let mb_h = cfg.height.div_ceil(16) as usize;
        // Odd sizes cannot be cropped exactly in 4:2:0; round down to even.
        let (width, height) = (cfg.width & !1, cfg.height & !1);
        let level = pick_level(mb_w as u32, mb_h as u32, cfg.fps(), cfg.bitrate_kbps);
        let (profile_idc, constraint_flags) = match cfg.profile {
            // constraint_set0 + constraint_set1: Constrained Baseline.
            Profile::Baseline => (66, 0xC0),
            // constraint_set1: decodable by Main decoders.
            Profile::Main => (77, 0x40),
        };
        let seq = SeqParams {
            profile_idc,
            constraint_flags,
            level_idc: level,
            mb_w: mb_w as u32,
            mb_h: mb_h as u32,
            width,
            height,
            fps_num: cfg.fps_num,
            fps_den: cfg.fps_den,
            log2_max_frame_num: 8,
            color: cfg.color,
        };
        let sps_nal = nal_unit(3, 7, &seq.sps_rbsp());
        let pps_nal = nal_unit(3, 8, &params::pps_rbsp(cfg.profile == Profile::Main));
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        let want = if cfg.max_slices == 0 { threads } else { cfg.max_slices as usize };
        let slices = want.clamp(1, mb_h);
        let rc = rc::RateControl::new(cfg.rate_mode, cfg.bitrate_kbps, cfg.fps(), if cfg.gop == 0 { 250 } else { cfg.gop }, (cfg.width * cfg.height) as f64);
        Ok(Encoder {
            mb_w,
            mb_h,
            seq,
            sps_nal,
            pps_nal,
            refp: None,
            prev_mv: vec![[0; 2]; mb_w * mb_h],
            frames: 0,
            since_idr: 0,
            frame_num: 0,
            idr_count: 0,
            force_key: false,
            rc,
            slices,
            recon: None,
            keep_recon: false,
            cfg,
        })
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// The SPS NAL unit (no start code).
    pub fn sps(&self) -> &[u8] {
        &self.sps_nal
    }

    /// The PPS NAL unit (no start code).
    pub fn pps(&self) -> &[u8] {
        &self.pps_nal
    }

    /// AVCDecoderConfigurationRecord for `avcC`.
    pub fn avcc(&self) -> Vec<u8> {
        params::avcc(&self.sps_nal, &self.pps_nal)
    }

    pub fn level_idc(&self) -> u8 {
        self.seq.level_idc
    }

    pub fn track_info(&self) -> TrackInfo {
        TrackInfo {
            width: self.seq.width,
            height: self.seq.height,
            fps_num: self.cfg.fps_num,
            fps_den: self.cfg.fps_den,
            avcc: self.avcc(),
            color: self.cfg.color,
            bitrate: self.cfg.bitrate_kbps.saturating_mul(1000),
            hevc: false,
        }
    }

    /// Make the next frame an IDR frame.
    pub fn force_keyframe(&mut self) {
        self.force_key = true;
    }

    /// Keep the reconstructed (decoder-identical) pictures for inspection.
    pub fn keep_reconstruction(&mut self, on: bool) {
        self.keep_recon = on;
    }

    /// The last reconstructed picture (when [`Encoder::keep_reconstruction`] is on).
    pub fn reconstruction(&self) -> Option<&Yuv420> {
        self.recon.as_ref()
    }

    pub fn frames_encoded(&self) -> u64 {
        self.frames
    }

    /// Encode an RGBA frame (converted to BT.709 limited range 4:2:0).
    pub fn encode_rgba(&mut self, rgba: &[u8], width: u32, height: u32) -> Frame {
        let pic = Yuv420::from_rgba(rgba, width, height);
        self.encode(&pic)
    }

    /// Encode one picture. Pictures of another size are not scaled: they must
    /// match the configured size (smaller ones are edge-extended).
    pub fn encode(&mut self, pic: &Yuv420) -> Frame {
        let (sy, su, sv) = self.pad_source(pic);
        let gop = self.cfg.gop;
        let idr = self.refp.is_none() || self.force_key || (gop > 0 && self.since_idr >= gop);
        self.force_key = false;
        if idr {
            self.frame_num = 0;
            self.since_idr = 0;
        }
        let mut qp = self.rc.frame_qp(idr);
        let mut coded = self.encode_picture(&sy, &su, &sv, qp, idr);
        // CBR: re-code a frame that overshoots its budget badly.
        if self.rc.is_cbr() {
            let budget = self.rc.frame_budget(idr);
            let bits = coded.0.iter().map(|n| n.len() as f64 * 8.0).sum::<f64>();
            if bits > budget * 2.5 && qp < 51 {
                qp = (qp + (6.0 * (bits / budget).log2()).ceil() as i32).min(51);
                coded = self.encode_picture(&sy, &su, &sv, qp, idr);
            }
        }
        let (nals, mut y, mut u, mut v, db, mvs) = coded;
        deblock_frame(&mut y, &mut u, &mut v, &db, self.mb_w, self.mb_h);
        let w = self.mb_w * 16;
        let h = self.mb_h * 16;
        if self.keep_recon {
            let (cw, ch) = (w / 2, h / 2);
            let pw = self.seq.width as usize;
            let ph = self.seq.height as usize;
            let mut out = Yuv420::filled(pw as u32, ph as u32, 0, 0, 0);
            for r in 0..ph {
                out.y[r * pw..r * pw + pw].copy_from_slice(&y[r * w..r * w + pw]);
            }
            let (pcw, pch) = (pw.div_ceil(2), ph.div_ceil(2));
            for r in 0..pch.min(ch) {
                out.u[r * pcw..r * pcw + pcw].copy_from_slice(&u[r * cw..r * cw + pcw]);
                out.v[r * pcw..r * pcw + pcw].copy_from_slice(&v[r * cw..r * cw + pcw]);
            }
            self.recon = Some(out);
        }
        self.refp = Some(RefPic::new(&y, &u, &v, w, h));
        self.prev_mv = mvs;
        let mut data = Vec::with_capacity(nals.iter().map(|n| n.len() + 4).sum::<usize>());
        for n in &nals {
            push_length_prefixed(&mut data, n);
        }
        let bits = data.len() as u64 * 8;
        let pad = self.rc.update(idr, qp, bits);
        if pad > 0 {
            // filler_data_rbsp(): 0xFF bytes then rbsp_trailing_bits.
            let mut rbsp = vec![0xFFu8; pad.saturating_sub(6)];
            rbsp.push(0x80);
            push_length_prefixed(&mut data, &nal_unit(0, 12, &rbsp));
        }
        self.frames += 1;
        self.since_idr += 1;
        self.frame_num = (self.frame_num + 1) % (1 << self.seq.log2_max_frame_num);
        if idr {
            self.idr_count += 1;
        }
        Frame { data, keyframe: idr, qp: qp as u8 }
    }

    /// Source planes padded to the coded size by edge replication.
    fn pad_source(&self, pic: &Yuv420) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let (w, h) = (self.mb_w * 16, self.mb_h * 16);
        let pw = (pic.width as usize).max(1);
        let ph = (pic.height as usize).max(1);
        let mut y = vec![0u8; w * h];
        for r in 0..h {
            let sr = r.min(ph - 1);
            let line = &pic.y[sr * pw..sr * pw + pw];
            let n = pw.min(w);
            y[r * w..r * w + n].copy_from_slice(&line[..n]);
            let last = line[n - 1];
            y[r * w + n..r * w + w].fill(last);
        }
        let (cw, ch) = (w / 2, h / 2);
        let (pcw, pch) = (pw.div_ceil(2), ph.div_ceil(2));
        let mut u = vec![0u8; cw * ch];
        let mut v = vec![0u8; cw * ch];
        for (src, dst) in [(&pic.u, &mut u), (&pic.v, &mut v)] {
            for r in 0..ch {
                let sr = r.min(pch - 1);
                let line = &src[sr * pcw..sr * pcw + pcw];
                let n = pcw.min(cw);
                dst[r * cw..r * cw + n].copy_from_slice(&line[..n]);
                let last = line[n - 1];
                dst[r * cw + n..r * cw + cw].fill(last);
            }
        }
        (y, u, v)
    }

    #[allow(clippy::type_complexity)]
    fn encode_picture(&self, sy: &[u8], su: &[u8], sv: &[u8], qp: i32, idr: bool) -> (Vec<Vec<u8>>, Vec<u8>, Vec<u8>, Vec<u8>, Vec<deblock::DbMb>, Vec<[i16; 2]>) {
        let (mb_w, mb_h) = (self.mb_w, self.mb_h);
        let w = mb_w * 16;
        let cw = mb_w * 8;
        let mut ry = vec![0u8; w * mb_h * 16];
        let mut ru = vec![0u8; cw * mb_h * 8];
        let mut rv = vec![0u8; cw * mb_h * 8];
        let f = FrameIn {
            mb_w,
            mb_h,
            src_y: sy,
            src_u: su,
            src_v: sv,
            refp: if idr { None } else { self.refp.as_ref() },
            prev_mv: &self.prev_mv,
            qp,
            use_i4: self.cfg.intra4x4,
        };
        // Split macroblock rows evenly between slices.
        let n = self.slices;
        let bounds: Vec<(usize, usize)> = (0..n).map(|i| (i * mb_h / n, (i + 1) * mb_h / n)).filter(|(a, b)| b > a).collect();
        let mut outs: Vec<Option<slice::SliceOut>> = (0..bounds.len()).map(|_| None).collect();
        std::thread::scope(|s| {
            let (mut y_rest, mut u_rest, mut v_rest) = (&mut ry[..], &mut ru[..], &mut rv[..]);
            let mut handles = Vec::new();
            for (k, &(r0, r1)) in bounds.iter().enumerate() {
                let rows = r1 - r0;
                let (ya, yb) = std::mem::take(&mut y_rest).split_at_mut(rows * 16 * w);
                let (ua, ub) = std::mem::take(&mut u_rest).split_at_mut(rows * 8 * cw);
                let (va, vb) = std::mem::take(&mut v_rest).split_at_mut(rows * 8 * cw);
                y_rest = yb;
                u_rest = ub;
                v_rest = vb;
                let hdr = SliceHeader {
                    first_mb: (r0 * mb_w) as u32,
                    idr,
                    p: !idr,
                    frame_num: self.frame_num,
                    log2_max_frame_num: self.seq.log2_max_frame_num,
                    idr_pic_id: self.idr_count % 2,
                    qp,
                    cabac: self.cfg.profile == Profile::Main,
                };
                let f = &f;
                let mut job = move || {
                    let mut rec = Recon { row0: r0, y: ya, u: ua, v: va };
                    slice::encode_slice(f, &hdr, &mut rec, r0..r1)
                };
                if k + 1 == bounds.len() {
                    handles.push((k, None, Some(job())));
                } else {
                    handles.push((k, Some(s.spawn(job)), None));
                }
            }
            for (k, h, done) in handles {
                outs[k] = Some(match (h, done) {
                    (Some(h), _) => h.join().expect("slice thread"),
                    (None, Some(o)) => o,
                    _ => unreachable!(),
                });
            }
        });
        let mut nals = Vec::with_capacity(outs.len());
        let mut db = Vec::with_capacity(mb_w * mb_h);
        let mut mvs = Vec::with_capacity(mb_w * mb_h);
        for o in outs.into_iter().flatten() {
            nals.push(o.nal);
            db.extend(o.db);
            mvs.extend(o.mvs);
        }
        (nals, ry, ru, rv, db, mvs)
    }

    /// SPS and PPS as Annex-B (start codes), to prefix a raw `.h264` stream.
    pub fn annex_b_headers(&self) -> Vec<u8> {
        let mut v = vec![0, 0, 0, 1];
        v.extend_from_slice(&self.sps_nal);
        v.extend_from_slice(&[0, 0, 0, 1]);
        v.extend_from_slice(&self.pps_nal);
        v
    }
}
