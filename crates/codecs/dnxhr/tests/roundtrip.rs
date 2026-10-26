// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Structure, quality and determinism of the DNxHR encoder, checked with the
//! crate's own reference decoder.

use studio_dnxhr::{Chroma, Encoder, EncoderConfig, Frame, Profile, decode_frame, parse_header};

/// A synthetic test pattern: ramps, a colour bar strip, a disc and a fine
/// grating, in limited-range Y'CbCr.
fn pattern(w: u32, h: u32, chroma: Chroma, bits: u8) -> Frame {
    let mut f = Frame::new(w, h, chroma, bits);
    let s = 1i32 << (bits - 8);
    let cw = f.chroma_width() as usize;
    let (wi, hi) = (w as i32, h as i32);
    for y in 0..hi {
        for x in 0..wi {
            let mut luma = 64 + (x * 800 / wi.max(1) + y * 100 / hi.max(1)) / 4 * 2;
            let (cx, cy) = (wi * 3 / 4, hi / 2);
            let r2 = (x - cx).pow(2) + (y - cy).pow(2);
            let rad = hi.min(wi) / 5;
            if r2 < rad * rad {
                luma = 900;
            }
            if y > hi * 4 / 5 && (x / 3) % 2 == 0 {
                luma = 300;
            }
            f.y[(y * wi + x) as usize] = ((luma.clamp(64, 940) * s) / 4).clamp(0, (1 << bits) - 1) as u16;
        }
        for cx in 0..cw as i32 {
            let x = if chroma == Chroma::Yuv422 { cx * 2 } else { cx };
            let band = (x * 8 / wi.max(1)).clamp(0, 7);
            let cb = 512 + (band - 4) * 50 + (y * 40 / hi.max(1));
            let cr = 512 + (3 - band) * 45;
            f.cb[y as usize * cw + cx as usize] = ((cb * s) / 4).clamp(0, (1 << bits) - 1) as u16;
            f.cr[y as usize * cw + cx as usize] = ((cr * s) / 4).clamp(0, (1 << bits) - 1) as u16;
        }
    }
    f
}

fn psnr(a: &[u16], b: &[u16], bits: u8) -> f64 {
    let mse = a.iter().zip(b).map(|(&x, &y)| (x as f64 - y as f64).powi(2)).sum::<f64>() / a.len() as f64;
    let peak = ((1u32 << bits) - 1) as f64;
    10.0 * (peak * peak / mse.max(1e-12)).log10()
}

fn enc(p: Profile, w: u32, h: u32) -> Encoder {
    Encoder::new(EncoderConfig::new(p, w, h)).unwrap()
}

#[test]
fn header_fields_and_sizes_per_profile() {
    for p in Profile::ALL {
        let (w, h) = (1280u32, 720u32);
        let f = pattern(w, h, p.chroma(), p.bit_depth());
        let bytes = enc(p, w, h).encode(&f).unwrap();
        // Frame size is the equation 7.1 size, a multiple of 4096.
        assert_eq!(bytes.len(), p.frame_size(w, h), "{p:?}");
        assert_eq!(bytes.len() % 4096, 0);
        let hd = parse_header(&bytes).unwrap();
        assert_eq!(hd.header_size, 640);
        assert_eq!(hd.version, 3);
        assert_eq!(hd.compression_id, p.cid());
        assert_eq!((hd.width, hd.height), (w, h));
        assert_eq!(hd.bit_depth(), p.bit_depth());
        assert!(!hd.vbr && hd.progressive_frame && hd.frame_encoding && !hd.interlaced_source);
        assert_eq!(hd.sub_sampling, if p == Profile::P444 { 2 } else { 0 });
        assert_eq!(hd.scan_lines, 45);
        // Raw bytes of the fixed fields.
        assert_eq!(&bytes[0..5], &[0, 0, 0x02, 0x80, 3]);
        assert_eq!(bytes[5], 1);
        assert_eq!(&bytes[0x28..0x2C], &p.cid().to_be_bytes());
        assert_eq!(&bytes[bytes.len() - 4..], &[0x60, 0x0D, 0xC0, 0xDE]);
        assert_eq!(&bytes[0x167..0x170], &[2, 0, 0, 0x01, 0x14, 0, 45, 0, 0x10]);
    }
}

#[test]
fn slice_table_is_consistent() {
    for p in Profile::ALL {
        let (w, h) = (200u32, 120u32);
        let bytes = enc(p, w, h).encode(&pattern(w, h, p.chroma(), p.bit_depth())).unwrap();
        let hd = parse_header(&bytes).unwrap();
        assert_eq!(hd.scan_lines, 8);
        assert_eq!(hd.scan_offsets[0], 0);
        let payload_end = bytes.len() - 4;
        let mut prev = 0;
        for (i, &o) in hd.scan_offsets.iter().enumerate() {
            assert_eq!(o % 4, 0, "scan line {i} starts on a 4-byte boundary");
            assert!(i == 0 || o > prev, "offsets increase");
            assert!(640 + o as usize <= payload_end);
            prev = o;
        }
        // Everything between the end of the last line and the EOF signature is zero padding.
        let last_line_end = bytes[640 + prev as usize..payload_end].iter().rposition(|&b| b != 0).map(|i| 640 + prev as usize + i + 1).unwrap();
        assert!(bytes[last_line_end..payload_end].iter().all(|&b| b == 0));
        // The table beyond the used lines is zero.
        assert!(bytes[0x170 + 4 * 8..0x280].iter().all(|&b| b == 0));
    }
}

#[test]
fn tall_frames_grow_the_header() {
    let (w, h) = (64u32, 1100u32);
    let p = Profile::Sq;
    let bytes = enc(p, w, h).encode(&pattern(w, h, p.chroma(), 8)).unwrap();
    let hd = parse_header(&bytes).unwrap();
    // 69 scan lines: one more than 1088 lines, so 4 more header bytes.
    assert_eq!(hd.header_size, 644);
    assert_eq!(hd.scan_lines, 69);
    assert_eq!(&bytes[0..4], &644u32.to_be_bytes());
    let f = decode_frame(&bytes).unwrap();
    assert_eq!((f.width, f.height), (w, h));
}

#[test]
fn quality_meets_the_bar() {
    // HQ and HQX above 40 dB on every plane; the lower profiles are held to lower bars.
    let cases = [(Profile::Hqx, 42.0), (Profile::Hq, 40.0), (Profile::P444, 42.0), (Profile::Sq, 36.0), (Profile::Lb, 30.0)];
    for (p, bar) in cases {
        let (w, h) = (1280u32, 720u32);
        let src = pattern(w, h, p.chroma(), p.bit_depth());
        let bytes = enc(p, w, h).encode(&src).unwrap();
        let out = decode_frame(&bytes).unwrap();
        assert_eq!((out.width, out.height, out.chroma, out.bit_depth), (w, h, p.chroma(), p.bit_depth()));
        for (name, a, b) in [("Y", &src.y, &out.y), ("Cb", &src.cb, &out.cb), ("Cr", &src.cr, &out.cr)] {
            let db = psnr(a, b, p.bit_depth());
            eprintln!("{p:?} {name}: {db:.1} dB");
            assert!(db > bar, "{p:?} {name} {db:.1} dB <= {bar}");
        }
    }
}

#[test]
fn odd_sizes_round_trip() {
    for (w, h) in [(2u32, 2u32), (18, 10), (100, 60), (130, 33)] {
        for p in [Profile::Hq, Profile::Hqx, Profile::P444] {
            let src = pattern(w, h, p.chroma(), p.bit_depth());
            let bytes = enc(p, w, h).encode(&src).unwrap();
            assert_eq!(bytes.len(), p.frame_size(w, h));
            let out = decode_frame(&bytes).unwrap();
            assert_eq!((out.width, out.height), (w, h));
            let db = psnr(&src.y, &out.y, p.bit_depth());
            assert!(db > 38.0, "{p:?} {w}x{h}: {db:.1} dB");
        }
    }
}

#[test]
fn output_is_deterministic() {
    for p in Profile::ALL {
        let (w, h) = (336u32, 190u32);
        let src = pattern(w, h, p.chroma(), p.bit_depth());
        let a = enc(p, w, h).encode(&src).unwrap();
        let b = enc(p, w, h).encode(&src).unwrap();
        assert_eq!(a, b);
    }
}

#[test]
fn flat_and_noisy_pictures_fit_the_frame() {
    let (w, h) = (256u32, 128u32);
    for p in Profile::ALL {
        let flat = Frame::new(w, h, p.chroma(), p.bit_depth());
        let e = enc(p, w, h);
        assert_eq!(e.encode(&flat).unwrap().len(), e.frame_size());
        let mut noisy = flat.clone();
        let mut s = 12345u32;
        for v in noisy.y.iter_mut().chain(noisy.cb.iter_mut()).chain(noisy.cr.iter_mut()) {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            *v = ((s >> 20) & ((1 << p.bit_depth()) - 1)) as u16;
        }
        let bytes = e.encode(&noisy).unwrap();
        assert_eq!(bytes.len(), e.frame_size());
        decode_frame(&bytes).unwrap();
    }
}

#[test]
fn bad_input_is_refused() {
    assert!(Encoder::new(EncoderConfig::new(Profile::Hq, 0, 10)).is_err());
    assert!(Encoder::new(EncoderConfig::new(Profile::Hq, 17, 10)).is_err(), "4:2:2 needs an even width");
    assert!(Encoder::new(EncoderConfig::new(Profile::P444, 17, 10)).is_ok());
    let e = enc(Profile::Hqx, 64, 64);
    assert!(e.encode(&Frame::new(64, 64, Chroma::Yuv422, 8)).is_err(), "wrong bit depth");
    assert!(e.encode(&Frame::new(32, 64, Chroma::Yuv422, 10)).is_err(), "wrong size");
    assert!(decode_frame(&[0u8; 100]).is_err());
}

#[test]
fn textured_picture_still_clears_40_db_at_hq_and_hqx() {
    // The pattern plus fine deterministic grain, as a camera picture would have.
    for p in [Profile::Hq, Profile::Hqx] {
        let (w, h) = (1280u32, 720u32);
        let mut src = pattern(w, h, p.chroma(), p.bit_depth());
        let mut s = 7u32;
        let amp = 1i32 << (p.bit_depth() - 8);
        for v in src.y.iter_mut().chain(src.cb.iter_mut()).chain(src.cr.iter_mut()) {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            let n = ((s >> 24) as i32 % 5 - 2) * amp / 2;
            *v = (*v as i32 + n).clamp(0, (1 << p.bit_depth()) - 1) as u16;
        }
        let out = decode_frame(&enc(p, w, h).encode(&src).unwrap()).unwrap();
        for (name, a, b) in [("Y", &src.y, &out.y), ("Cb", &src.cb, &out.cb), ("Cr", &src.cr, &out.cr)] {
            let db = psnr(a, b, p.bit_depth());
            assert!(db > 40.0, "{p:?} {name} {db:.1} dB");
        }
    }
}
