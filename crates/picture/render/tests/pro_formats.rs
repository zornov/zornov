// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pro formats as sources: ProRes QuickTime masters (written by
//! `studio-mov`) and OpenEXR / DPX frame sequences open, decode at
//! full precision and land on the right linear values.

use studio_render::source::open_path;
use studio_render::yuv::PixelData;

fn scratch(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("studio-proformats-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A horizontal grey ramp with a red patch, as 16-bit BT.709 code values.
fn picture(w: u32, h: u32) -> Vec<u16> {
    let mut v = vec![0u16; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let g = (x as f32 / (w - 1) as f32 * 65535.0) as u16;
            let rgb = if x < w / 4 && y < h / 4 { [52000, 9000, 9000] } else { [g, g, g] };
            v[i..i + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 65535]);
        }
    }
    v
}

#[test]
fn prores_master_plays_back_at_full_precision() {
    let dir = scratch("prores");
    let path = dir.join("master.mov");
    let (w, h) = (128u32, 64u32);
    let rgba = picture(w, h);
    let tc = studio_mov::StartTimecode { frame: 86400, fps: 24, drop_frame: false, reel: Some("A001C007".into()) };
    for profile in [studio_prores::Profile::Hq, studio_prores::Profile::P4444] {
        let video = studio_mov::mov::VideoTrack { profile, width: w, height: h, rate_num: 24000, rate_den: 1001, colour: (1, 1, 1), alpha: false };
        let audio = studio_mov::mov::AudioTrack { sample_rate: 48000, channels: 2 };
        let mut mov = studio_mov::mov::MovWriter::create(&path, video, Some(audio), Some(tc.clone())).unwrap();
        let mut enc = studio_mov::prores::ProResEncoder::new(profile, w, h, (1, 1, 1), false);
        for _ in 0..3 {
            mov.write_video(&enc.encode(&rgba, w, h).unwrap()).unwrap();
            mov.write_audio(&vec![0.25f32; 2002 * 2]).unwrap();
        }
        mov.finish().unwrap();

        assert_eq!(studio_mov::probe::timecode(&path), Some(tc.clone()));
        let mut src = open_path(&path).unwrap_or_else(|e| panic!("{profile:?}: {e}"));
        let info = src.info();
        assert_eq!((info.width, info.height, info.frame_count), (w, h, Some(3)));
        assert!((info.frame_rate.unwrap() - 23.976).abs() < 0.01);
        let mut got = None;
        assert!(src.decode_to(2, &mut |_, p| got = Some(p)));
        let pic = got.unwrap();
        let PixelData::Yuv { y, .. } = &pic.data else { panic!() };
        assert!(matches!(y, studio_render::yuv::Planes::U16(_)), "decoded at 10/12 bits");
        let f = pic.to_frame(false);
        // Mid-grey ramp: code 0.5 decodes through BT.1886 to about 0.19.
        let mid = f.pixels[(h / 2 * w + w / 2) as usize];
        let want = (0.5f32 * 128.0 / 127.0).powf(2.4);
        assert!((mid[1] - want).abs() < 0.01, "{profile:?}: {mid:?} vs {want}");
        let red = f.pixels[(2 * w + 2) as usize];
        assert!(red[0] > 0.5 && red[1] < 0.02, "{profile:?}: red patch {red:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn exr_and_dpx_sequences_open_as_media() {
    let dir = scratch("seq");
    for n in 0..4 {
        let v = 0.1 + 0.1 * n as f32;
        let px = vec![[v, v, v, 1.0]; 16 * 8];
        let exr = studio_imageio::exr::encode(16, 8, &px, &studio_imageio::exr::WriteOptions { frame_rate: Some((24, 1)), ..Default::default() });
        std::fs::write(dir.join(format!("plate.{:04}.exr", 1001 + n)), exr).unwrap();
        let code = [studio_colour::LogCurve::Cineon.encode(v); 3];
        let dpx = studio_imageio::dpx::encode(16, 8, &vec![[code[0], code[1], code[2], 1.0]; 16 * 8], &studio_imageio::dpx::WriteOptions { transfer: studio_imageio::Transfer::PrintingDensity, frame_rate: Some(24.0), ..Default::default() });
        std::fs::write(dir.join(format!("scan_{:06}.dpx", 86400 + n)), dpx).unwrap();
    }
    for (pattern, name) in [("plate.####.exr", "exr"), ("scan_######.dpx", "dpx")] {
        let mut src = open_path(&dir.join(pattern)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(src.info().frame_count, Some(4), "{name}");
        let mut got = None;
        assert!(src.decode_to(3, &mut |_, p| got = Some(p)));
        let p = got.unwrap().to_frame(false).pixels[20];
        assert!((p[1] - 0.4).abs() < 0.004, "{name}: {p:?}");
    }
    // A single EXR frame opens as a still.
    let mut still = open_path(&dir.join("plate.1002.exr")).unwrap();
    let mut got = None;
    assert!(still.decode_to(0, &mut |_, p| got = Some(p)));
    assert!((got.unwrap().to_frame(false).pixels[0][0] - 0.2).abs() < 0.003);
    let _ = std::fs::remove_dir_all(&dir);
}
