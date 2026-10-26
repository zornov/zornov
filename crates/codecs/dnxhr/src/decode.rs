// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reference decoder for the frames [`crate::Encoder`] writes (progressive,
//! CBR, no alpha, Y'CbCr coding, HVN 3). It follows section 8 of ST 2019-1.

use crate::bits::BitReader;
use crate::dct::{ZIGZAG, idct};
use crate::header::Header;
use crate::layout::{self, CB, CR};
use crate::vlc::Vlc;
use crate::{Chroma, Error, Frame, Profile, Result};

/// Reads and checks the header of a coding unit.
pub fn parse_header(data: &[u8]) -> Result<Header> {
    Header::parse(data)
}

/// Inverse quantisation of one AC coefficient (equation 8.1, p = 32).
#[inline]
fn dequant(level: i32, w: u32, qsf: u32) -> i32 {
    if level == 0 {
        return 0;
    }
    let a = level.unsigned_abs();
    let wq = w * qsf;
    let v = if w != 32 { (a * wq + wq / 2 + 16) / 32 } else { (a * wq + wq / 2) / 32 };
    if level < 0 { -(v as i32) } else { v as i32 }
}

/// Decodes one progressive frame.
pub fn decode_frame(data: &[u8]) -> Result<Frame> {
    let h = Header::parse(data)?;
    if h.vbr {
        return Err(Error::Unsupported("variable bit rate"));
    }
    if !h.progressive_frame || h.interlaced_source {
        return Err(Error::Unsupported("interlaced coding"));
    }
    if h.sub_sampling == 1 {
        return Err(Error::Unsupported("4:2:0 coding"));
    }
    let profile = Profile::from_cid(h.compression_id).ok_or(Error::Unsupported("compression ID"))?;
    if h.rgb != (profile == Profile::P444) {
        return Err(Error::Unsupported("colour format other than the profile's own"));
    }
    let chroma = profile.chroma();
    if h.bit_depth() != profile.bit_depth() || (h.sub_sampling == 2) != (chroma == Chroma::Yuv444) {
        return Err(Error::Invalid("header disagrees with the compression ID"));
    }
    let params = profile.params();
    let vlc = Vlc::get(params.set);
    let bits = h.bit_depth() as u32;
    let pbits = if bits == 8 { 4 } else { 6 };
    let (maxv, shift) = ((1i32 << bits) - 1, 1i32 << (bits - 1));
    let nw = h.width.div_ceil(16) as usize;
    let ns = h.scan_lines as usize;
    let blocks = layout::blocks(chroma);
    let (pw, ph) = (nw * 16, ns * 16);
    let cpw = if chroma == Chroma::Yuv444 { pw } else { pw / 2 };
    let mut planes = [vec![0u16; pw * ph], vec![0u16; cpw * ph], vec![0u16; cpw * ph]];
    let payload_start = h.header_size as usize;
    for s in 0..ns {
        let start = payload_start + h.scan_offsets[s] as usize;
        if start > data.len() {
            return Err(Error::Truncated);
        }
        let end = if s + 1 < ns { payload_start + h.scan_offsets[s + 1] as usize } else { data.len() };
        if end < start || end > data.len() {
            return Err(Error::Invalid("scan line offsets"));
        }
        let mut r = BitReader::new(&data[start..end]);
        let mut pred = [0i32; 3];
        for m in 0..nw {
            let head = r.get(12);
            // The ACF bit: set on every macroblock of the 444 profile (coded as Y'CbCr in an
            // RGB stream). The planes are returned as coded, without the conversion to RGB.
            if (head & 1 != 0) != (profile == Profile::P444) {
                return Err(Error::Unsupported("alternate colour space macroblock"));
            }
            let qsf = head >> 1;
            if qsf == 0 {
                return Err(Error::Invalid("quantiser scale factor 0"));
            }
            for &(ct, dx, dy) in blocks {
                let wts = if ct == CB || ct == CR { params.chroma } else { params.luma };
                let mut coef = [0f64; 64];
                // DC.
                let (len, eta) = vlc.decode_dc(r.peek16());
                if len == 0 {
                    return Err(Error::Invalid("bad DC code"));
                }
                r.skip(len as u32);
                let eps = if eta == 0 {
                    0
                } else {
                    let rho = r.get(eta as u32) as i32;
                    if rho >= 1 << (eta - 1) { rho } else { rho + 1 - (1 << eta) }
                };
                pred[ct] += eps;
                coef[0] = pred[ct] as f64;
                // AC.
                let mut rr = 1usize;
                while rr < 64 {
                    let sym = vlc.decode_amp(r.peek16());
                    if sym.len == 0 {
                        return Err(Error::Invalid("bad AC code"));
                    }
                    r.skip(sym.len as u32);
                    if sym.amp == 0 {
                        break;
                    }
                    let neg = r.get(1) == 1;
                    let mut a = sym.amp as i32;
                    if sym.index {
                        a += 64 * r.get(pbits) as i32;
                    }
                    if sym.run {
                        let (rl, run) = vlc.decode_run(r.peek16());
                        if rl == 0 {
                            return Err(Error::Invalid("bad run code"));
                        }
                        r.skip(rl as u32);
                        rr += run as usize;
                    }
                    if rr >= 64 {
                        return Err(Error::Invalid("coefficient index past 63"));
                    }
                    let pos = ZIGZAG[rr] as usize;
                    coef[pos] = dequant(if neg { -a } else { a }, wts[pos] as u32, qsf) as f64;
                    rr += 1;
                }
                if r.overrun() {
                    return Err(Error::Truncated);
                }
                let x = idct(&coef);
                let (plane, stride, ox) = match ct {
                    0 => (0, pw, m * 16 + dx),
                    1 => (1, cpw, m * layout::mb_width(chroma, ct) + dx),
                    _ => (2, cpw, m * layout::mb_width(chroma, ct) + dx),
                };
                let oy = s * 16 + dy;
                for j in 0..8 {
                    for i in 0..8 {
                        let v = (x[j * 8 + i].round() as i32 + shift).clamp(0, maxv);
                        planes[plane][(oy + j) * stride + ox + i] = v as u16;
                    }
                }
            }
        }
    }
    let (w, hh) = (h.width as usize, h.height as usize);
    let mut f = Frame::new(h.width, h.height, chroma, bits as u8);
    let cw = f.chroma_width() as usize;
    for y in 0..hh {
        f.y[y * w..(y + 1) * w].copy_from_slice(&planes[0][y * pw..y * pw + w]);
        f.cb[y * cw..(y + 1) * cw].copy_from_slice(&planes[1][y * cpw..y * cpw + cw]);
        f.cr[y * cw..(y + 1) * cw].copy_from_slice(&planes[2][y * cpw..y * cpw + cw]);
    }
    Ok(f)
}
