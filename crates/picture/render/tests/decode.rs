// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Decoding real files through `Mp4Source` and the decode cache, checked
//! against the reference YUV dumps next to the fixtures: frame index `k`
//! of the source must be picture `k` of the dump whatever the seek order.

use std::path::PathBuf;

use studio_render::source::{DecodeCache, MediaSource, Mp4Source, SourceRegistry, Sources};
use studio_render::yuv::{PixelData, Planes};
use studio_render::{MediaKey, SourceSpec};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/media").join(name)
}

fn reference_frames(name: &str, w: usize, h: usize) -> Vec<Vec<u8>> {
    let data = read_yuv(&fixture(name));
    let frame = w * h * 3 / 2;
    data.chunks_exact(frame).map(|c| c[..w * h].to_vec()).collect()
}

fn luma(p: &studio_render::yuv::DecodedPicture) -> Vec<u8> {
    match &p.data {
        PixelData::Yuv { y: Planes::U8(v), .. } => v.clone(),
        _ => panic!("expected 8-bit yuv"),
    }
}

fn check(video: &str, yuv: &str) {
    let src = Mp4Source::open(&fixture(video)).expect("open");
    let info = src.info();
    let (w, h) = (info.width as usize, info.height as usize);
    let refs = reference_frames(yuv, w, h);
    assert_eq!(info.frame_count, Some(refs.len() as u64), "{video}: frame count");
    let reg = SourceRegistry::new();
    reg.register_path(MediaKey(1), fixture(video));
    let cache = DecodeCache::new(64 << 20);
    let mut s = Sources::new(reg, cache.clone());
    let fps = info.frame_rate.expect("frame rate");
    let n = refs.len() as u64;
    // Forward, backward, random: the mapping must hold for every order.
    let order: Vec<u64> = [5, 6, 7, 2, n - 1, 0, n / 2, n / 2 + 1, 3].into_iter().map(|i| i.min(n - 1)).collect();
    for &k in &order {
        let t = ((k as f64 + 0.5) / fps * 1e6) as i64;
        let pic = s.picture(MediaKey(1), t).unwrap_or_else(|| panic!("{video}: frame {k}"));
        assert_eq!(luma(&pic), refs[k as usize], "{video}: frame {k} luma differs from the reference");
    }
    let st = cache.stats();
    assert!(st.hits >= 2, "{video}: sequential frames should come from the cache: {st:?}");
}

#[test]
fn h264_with_b_frames_maps_indices() {
    check("h264_main_cabac_bframes.mp4", "h264_main_cabac_bframes.yuv");
}

#[test]
fn h264_baseline_maps_indices() {
    check("h264_baseline_cavlc.mp4", "h264_baseline_cavlc.yuv");
}

#[test]
fn hevc_decodes_and_seeks() {
    let path = fixture("hevc_main.mp4");
    let src = Mp4Source::open(&path).expect("open");
    let info = src.info();
    let n = info.frame_count.unwrap();
    assert!(n > 2);
    let reg = SourceRegistry::new();
    reg.register(MediaKey(2), SourceSpec::Path(path));
    let mut s = Sources::new(reg, DecodeCache::new(64 << 20));
    let fps = info.frame_rate.unwrap();
    let frame = |s: &mut Sources, k: u64| s.picture(MediaKey(2), ((k as f64 + 0.5) / fps * 1e6) as i64).unwrap();
    let a = luma(&frame(&mut s, 0));
    let b = luma(&frame(&mut s, n - 1));
    let a2 = luma(&frame(&mut s, 0));
    assert_eq!(a, a2, "seeking back gives the same first frame");
    assert_eq!(a.len(), (info.width * info.height) as usize);
    assert!(a != b || n == 1);
    // Whole-frame conversion works and is opaque.
    let f = frame(&mut s, 1).to_frame(true);
    assert_eq!((f.width, f.height), (info.width, info.height));
    assert!(f.pixels.iter().all(|p| p[3] == 1.0));
}

/// Reads a reference I420 dump kept gzipped in the repository (`name` + `.gz`).
fn read_yuv(path: &std::path::Path) -> Vec<u8> {
    use std::io::Read;
    let gz = std::fs::File::open(path.with_extension("yuv.gz")).expect("reference yuv.gz");
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(gz).read_to_end(&mut out).expect("gunzip reference yuv");
    out
}
