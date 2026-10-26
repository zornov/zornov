// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Proxy files: a half or quarter resolution Apple ProRes 422 LT copy
//! of a media file, for smooth editing of heavy camera originals. Any
//! source Studio can decode works (H.264 / HEVC, ProRes, image sequences).
//!
//! The proxy keeps the original's code values: Y′CbCr planes are box
//! filtered and re-quantised to 10 bits (full-range video is mapped to
//! video range); RGB pictures (EXR, DPX, TIFF) are turned into BT.709
//! Y′CbCr of the same encoded values. So the original's transfer — camera
//! log included — still describes the proxy: the stream's tags carry the
//! standard ones, and [`ProxyInfo::input`] names the rest as an input
//! colour space for [`crate::source::Proxy::input`]. Pictures only: sound
//! keeps coming from the original.

use std::collections::BTreeMap;
use std::path::Path;

use crate::colour_space::{ColourDesc, Matrix, SourcePrimaries, SourceTransfer};
use crate::source::open_path;
use crate::yuv::{DecodedPicture, PixelData, Planes};

/// What was written.
#[derive(Clone, Debug, PartialEq)]
pub struct ProxyInfo {
    pub width: u32,
    pub height: u32,
    pub frames: u64,
    /// The proxy's input colour space when its tags cannot say it
    /// (`"slog3_sgamut3cine"`, `"acescct_ap0"`, …).
    pub input: Option<String>,
}

fn h273(desc: &ColourDesc) -> (u8, u8, u8, Option<String>) {
    let prim = match desc.primaries {
        SourcePrimaries::Bt709 => Some(1),
        SourcePrimaries::Bt601_625 => Some(5),
        SourcePrimaries::Bt601_525 => Some(6),
        SourcePrimaries::Bt2020 => Some(9),
        SourcePrimaries::P3D65 => Some(12),
        SourcePrimaries::Gamut(_) => None,
    };
    let tf = match desc.transfer {
        SourceTransfer::Srgb => Some(13),
        SourceTransfer::Bt709 => Some(1),
        SourceTransfer::Gamma22 => Some(4),
        SourceTransfer::Gamma28 => Some(5),
        SourceTransfer::Pq => Some(16),
        SourceTransfer::Hlg => Some(18),
        SourceTransfer::Linear => Some(8),
        SourceTransfer::Camera { .. } | SourceTransfer::Custom(_) => None,
    };
    let matrix = match desc.matrix {
        Matrix::Bt601 => 6,
        Matrix::Bt2020 => 9,
        Matrix::Bt709 | Matrix::Rgb => 1,
    };
    let input = match (desc.transfer, tf, prim) {
        (_, Some(_), Some(_)) => None,
        (SourceTransfer::Camera { curve, .. }, _, _) => {
            let gamut = match desc.primaries {
                SourcePrimaries::Gamut(g) => g,
                SourcePrimaries::Bt2020 => studio_colour::Gamut::Rec2020,
                SourcePrimaries::P3D65 => studio_colour::Gamut::P3D65,
                _ => studio_colour::Gamut::Rec709,
            };
            Some(format!("{}_{}", curve.slug(), gamut.slug()))
        }
        (SourceTransfer::Linear, _, None) => match desc.primaries {
            SourcePrimaries::Gamut(studio_colour::Gamut::AcesAp1) => Some("acescg".into()),
            SourcePrimaries::Gamut(studio_colour::Gamut::AcesAp0) => Some("aces2065-1".into()),
            _ => None,
        },
        _ => None,
    };
    (prim.unwrap_or(2), tf.unwrap_or(2), matrix, input)
}

/// Downscales a decoded picture by `scale` into 10-bit 4:2:2 planes.
fn proxy_frame(pic: &DecodedPicture, scale: u32, out_w: u32, out_h: u32) -> studio_prores::Frame {
    let mut f = studio_prores::Frame::new(out_w, out_h, studio_prores::ChromaFormat::Yuv422, 10, false);
    let (w, h) = (pic.width as usize, pic.height as usize);
    let s = scale as usize;
    let desc = pic.colour;
    match &pic.data {
        PixelData::Yuv { y, u, v, chroma_width, .. } => {
            let cw = *chroma_width as usize;
            let xdiv = if cw >= w { 1 } else { 2 };
            let ydiv = if (y.len() / w.max(1)) <= (u.len() / cw.max(1)) { 1 } else { 2 };
            let ch = u.len() / cw.max(1);
            let max = ((1u32 << desc.bit_depth.clamp(8, 16)) - 1) as f32;
            let avg = |p: &Planes, pw: usize, ph: usize, x0: usize, y0: usize, sx: usize, sy: usize| -> f32 {
                let mut acc = 0f32;
                let mut n = 0f32;
                for yy in y0..(y0 + sy).min(ph) {
                    for xx in x0..(x0 + sx).min(pw) {
                        acc += p.get(yy * pw + xx);
                        n += 1.0;
                    }
                }
                if n > 0.0 { acc / n } else { 0.0 }
            };
            let rgb = desc.matrix == Matrix::Rgb;
            let (kr, kb) = studio_colour::YCbCrMatrix::Bt709.kr_kb();
            for oy in 0..out_h as usize {
                for ox in 0..out_w as usize {
                    let (x0, y0) = (ox * s, oy * s);
                    let ly = avg(y, w, h, x0, y0, s, s) / max;
                    let (cx0, cy0, sx, sy) = (x0 / xdiv, y0 / ydiv, (s / xdiv).max(1), (s / ydiv).max(1));
                    let cb = avg(u, cw, ch, cx0, cy0, sx, sy) / max;
                    let cr = avg(v, cw, ch, cx0, cy0, sx, sy) / max;
                    let (yy, cbn, crn) = if rgb {
                        // Planes hold G, B, R codes (full range): BT.709.
                        let (g, b, r) = (ly, cb, cr);
                        let l = kr * r + (1.0 - kr - kb) * g + kb * b;
                        (l, (b - l) / (2.0 * (1.0 - kb)), (r - l) / (2.0 * (1.0 - kr)))
                    } else if desc.full_range {
                        (ly, cb - 0.5, cr - 0.5)
                    } else {
                        // Limited range: back to 0..1 luma, ±0.5 chroma.
                        let sh = (1u32 << (desc.bit_depth.clamp(8, 16) - 8)) as f32;
                        ((ly * max - 16.0 * sh) / (219.0 * sh), (cb * max - 128.0 * sh) / (224.0 * sh), (cr * max - 128.0 * sh) / (224.0 * sh))
                    };
                    f.y[oy * out_w as usize + ox] = (64.0 + 876.0 * yy).round().clamp(4.0, 1019.0) as u16;
                    if ox % 2 == 0 {
                        let ci = oy * (out_w as usize).div_ceil(2) + ox / 2;
                        f.cb[ci] = (512.0 + 896.0 * cbn).round().clamp(4.0, 1019.0) as u16;
                        f.cr[ci] = (512.0 + 896.0 * crn).round().clamp(4.0, 1019.0) as u16;
                    }
                }
            }
        }
        PixelData::Rgba8(px) => {
            let (kr, kb) = studio_colour::YCbCrMatrix::Bt709.kr_kb();
            for oy in 0..out_h as usize {
                for ox in 0..out_w as usize {
                    let mut c = [0f32; 3];
                    let mut n = 0f32;
                    for yy in oy * s..((oy + 1) * s).min(h) {
                        for xx in ox * s..((ox + 1) * s).min(w) {
                            let i = (yy * w + xx) * 4;
                            for k in 0..3 {
                                c[k] += px[i + k] as f32 / 255.0;
                            }
                            n += 1.0;
                        }
                    }
                    let [r, g, b] = c.map(|v| v / n.max(1.0));
                    let l = kr * r + (1.0 - kr - kb) * g + kb * b;
                    f.y[oy * out_w as usize + ox] = (64.0 + 876.0 * l).round() as u16;
                    if ox % 2 == 0 {
                        let ci = oy * (out_w as usize).div_ceil(2) + ox / 2;
                        f.cb[ci] = (512.0 + 896.0 * (b - l) / (2.0 * (1.0 - kb))).round() as u16;
                        f.cr[ci] = (512.0 + 896.0 * (r - l) / (2.0 * (1.0 - kr))).round() as u16;
                    }
                }
            }
        }
    }
    f
}

/// Writes a ProRes 422 LT copy of `source` at 1/`scale` size to `out`
/// (a `.mov`). `progress` gets (frames done, total) and returns `false`
/// to stop.
pub fn make_proxy(source: &Path, out: &Path, scale: u32, timecode: Option<studio_mov::StartTimecode>, progress: &mut dyn FnMut(u64, u64) -> bool) -> Result<ProxyInfo, String> {
    let scale = scale.clamp(1, 8);
    let mut src = open_path(source)?;
    let info = src.info();
    let total = info.frame_count.unwrap_or(1).max(1);
    let even = |v: u32| (v / 2 * 2).max(2);
    let (out_w, out_h) = (even(info.width.div_ceil(scale)), even(info.height.div_ceil(scale)));
    let fps = info.frame_rate.unwrap_or(24.0);
    let (rate_num, rate_den) = {
        let n = (fps * 1.001).round();
        if (fps - n * 1000.0 / 1001.0).abs() < 0.005 && (fps - fps.round()).abs() > 0.005 { ((n as u32) * 1000, 1001) } else { (fps.round().max(1.0) as u32, 1) }
    };
    let (p, t, m, input) = h273(&info.colour);
    let video = studio_mov::mov::VideoTrack { profile: studio_prores::Profile::Lt, width: out_w, height: out_h, rate_num, rate_den, colour: (p as u16, t as u16, m as u16), alpha: false };
    let tmp = out.with_extension("mov.part");
    let mut mov = studio_mov::mov::MovWriter::create(&tmp, video, None, timecode).map_err(|e| e.to_string())?;
    let mut enc = studio_prores::Encoder::with_config({
        let mut c = studio_prores::EncoderConfig::new(studio_prores::Profile::Lt, out_w, out_h);
        c.color = studio_prores::ColorInfo { primaries: p, transfer: t, matrix: m };
        c.encoder_id = *b"stdo";
        c
    });
    let mut pending: BTreeMap<u64, std::sync::Arc<DecodedPicture>> = BTreeMap::new();
    let mut next = 0u64;
    let mut last: Option<std::sync::Arc<DecodedPicture>> = None;
    while next < total {
        if !pending.contains_key(&next) {
            let ok = src.decode_to(next, &mut |i, pic| {
                if i >= next {
                    pending.insert(i, pic);
                }
            });
            if !ok && !pending.contains_key(&next) {
                // A frame that cannot be decoded repeats the one before.
                match &last {
                    Some(p) => {
                        pending.insert(next, p.clone());
                    }
                    None => return Err(format!("{}: frame {next} cannot be decoded", source.display())),
                }
            }
        }
        let Some(pic) = pending.remove(&next) else { continue };
        let frame = proxy_frame(&pic, scale, out_w, out_h);
        let bytes = enc.encode(&frame).map_err(|e| e.to_string())?;
        mov.write_video(&bytes).map_err(|e| e.to_string())?;
        last = Some(pic);
        next += 1;
        if !progress(next, total) {
            let _ = std::fs::remove_file(&tmp);
            return Err("stopped".into());
        }
    }
    mov.finish().map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, out).map_err(|e| e.to_string())?;
    Ok(ProxyInfo { width: out_w, height: out_h, frames: total, input })
}
