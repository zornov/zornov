// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn open(name: &str) -> Animation {
    Animation::open(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// Straight RGBA of pixel (x, y).
fn px(a: &Animation, frame: f64, x: u32, y: u32) -> [u8; 4] {
    let img = a.render_rgba(frame, a.width, a.height);
    let i = ((y * a.width + x) * 4) as usize;
    [img[i], img[i + 1], img[i + 2], img[i + 3]]
}

#[test]
fn fixtures_open_with_their_size_rate_and_length() {
    let ball = open("bouncing-ball.json");
    assert_eq!((ball.width, ball.height, ball.fps), (200, 200, 30.0));
    assert!(ball.warnings.is_empty(), "{:?}", ball.warnings);
    assert!(sniff(&std::fs::read(fixture("bouncing-ball.json")).unwrap()));
    assert!(!sniff(b"{\"name\": \"not lottie\"}"));
    let f = open("studio-features.json");
    assert_eq!((f.frame_count(), f.duration()), (60, 2.0));
    assert!(f.warnings.is_empty(), "{:?}", f.warnings);
    let text = open("precomp-matte-text.json");
    assert!(text.warnings.iter().any(|w| w.contains("text")), "{:?}", text.warnings);
}

#[test]
fn every_fixture_draws_something_at_every_frame() {
    for name in ["bouncing-ball.json", "shapes-gradient.json", "legacy-masks.json", "precomp-matte-text.json", "studio-features.json"] {
        let a = open(name);
        // From a quarter in: legacy-masks fades in from nothing at frame 0.
        for k in 1..4 {
            let f = a.in_frame + (a.out_frame - a.in_frame) * k as f64 / 4.0;
            let img = a.render_rgba(f, a.width, a.height);
            assert!(img.chunks_exact(4).any(|p| p[3] > 0), "{name} at frame {f} is empty");
        }
    }
}

#[test]
fn features_land_where_the_document_says() {
    let a = open("studio-features.json");
    // The paper background: opaque everywhere.
    assert_eq!(px(&a, 0.0, 2, 2)[3], 255);
    // The green morph fills the band near the bottom…
    let g = px(&a, 0.0, 30, 140);
    assert!(g[1] > 150 && g[0] < 100, "{g:?}");
    // …except where the subtract mask cuts it out (the paper shows).
    let hole = px(&a, 0.0, 120, 135);
    assert!(hole[0] > 230 && hole[1] > 220, "{hole:?}");
    // The luma matte shows red only inside its white rounded rectangle.
    let red = px(&a, 0.0, 200, 30);
    assert!(red[0] > 180 && red[1] < 80, "{red:?}");
    let beside = px(&a, 0.0, 200, 60);
    assert!(beside[0] > 230 && beside[1] > 220, "outside the matte: paper {beside:?}");
    // The badge rides its null: at frame 0 at (70, 80), by frame 40 at (170, 80).
    let at = |f: f64, x: u32| px(&a, f, x, 80);
    assert!(at(0.0, 70)[2] < 120, "badge at the start {:?}", at(0.0, 70));
    assert!(at(40.0, 170)[2] < 120, "badge at the end {:?}", at(40.0, 170));
    assert!(at(40.0, 70)[2] > 200, "and gone from the start {:?}", at(40.0, 70));
}

#[test]
fn time_remapped_precomp_spins_on_its_own_clock() {
    let a = open("studio-features.json");
    // Starts at frame 10 (st): before that the spinner's arc (blue) is
    // at its first pose; frames later it has turned.
    let blue = |f: f64| {
        let img = a.render_rgba(f, a.width, a.height);
        (0..80u32).flat_map(|y| (10..90u32).map(move |x| (x, y))).filter(|&(x, y)| { let i = ((y * a.width + x) * 4) as usize; img[i + 2] > 180 && img[i] < 80 }).count()
    };
    assert!(blue(15.0) > 50, "the arc shows");
    // Different poses at different times: the arc has turned, so many
    // pixels changed between them.
    let (p, q) = (a.render_rgba(15.0, a.width, a.height), a.render_rgba(45.0, a.width, a.height));
    let moved = p.chunks_exact(4).zip(q.chunks_exact(4)).filter(|(x, y)| (x[2] as i32 - y[2] as i32).abs() > 60).count();
    assert!(moved > 100, "{moved} pixels changed");
}

#[test]
fn renders_at_any_size() {
    let a = open("bouncing-ball.json");
    let small = a.render(15.0, 50, 50);
    let big = a.render(15.0, 400, 400);
    let cover = |p: &tiny_skia::Pixmap| p.pixels().iter().filter(|c| c.alpha() > 128).count() as f64 / (p.width() * p.height()) as f64;
    assert!((cover(&small) - cover(&big)).abs() < 0.03, "{} vs {}", cover(&small), cover(&big));
}

#[test]
fn dotlottie_archives_open() {
    let json = std::fs::read(fixture("bouncing-ball.json")).unwrap();
    let manifest = br#"{"version": "1", "animations": [{"id": "ball"}]}"#;
    let zip = zip::write(&[("manifest.json", manifest), ("animations/ball.json", &json)]);
    assert!(sniff(&zip));
    let a = Animation::from_bytes(&zip, None).unwrap();
    assert_eq!((a.width, a.height), (200, 200));
}

#[test]
fn hostile_input_is_refused_not_panicked() {
    for bad in [&b""[..], b"{", br#"{"w": 0, "h": 10, "fr": 30, "layers": []}"#, br#"{"w": 10, "h": 10, "fr": 30}"#, b"PK\x03\x04junk"] {
        assert!(Animation::from_bytes(bad, None).is_err());
    }
    // Deep precomp loops stop at the depth limit.
    let looped = br#"{"w": 10, "h": 10, "fr": 30, "ip": 0, "op": 10, "assets": [{"id": "a", "layers": [{"ty": 0, "refId": "a", "w": 10, "h": 10, "ks": {}}]}], "layers": [{"ty": 0, "refId": "a", "w": 10, "h": 10, "ks": {}}]}"#;
    let a = Animation::from_bytes(looped, None).unwrap();
    let _ = a.render(0.0, 10, 10);
}

/// Over white, as RGB bytes.
pub(crate) fn over_white(rgba: &[u8]) -> Vec<f64> {
    rgba.chunks_exact(4).flat_map(|p| { let a = p[3] as f64 / 255.0; [0, 1, 2].map(move |c| p[c] as f64 * a + 255.0 * (1.0 - a)) }).collect()
}

pub(crate) fn psnr(a: &[f64], b: &[f64]) -> f64 {
    let mse = a.iter().zip(b).map(|(x, y)| (x - y).powi(2)).sum::<f64>() / a.len() as f64;
    10.0 * (255.0f64 * 255.0 / mse.max(1e-9)).log10()
}

/// Against another player's frames (raw RGBA in `$LOTTIE_ORACLE/<name>/<frame>.rgba`):
/// prints PSNR per frame and writes side-by-side PNGs next to them.
#[test]
#[ignore]
fn compare_with_oracle() {
    let Ok(dir) = std::env::var("LOTTIE_ORACLE") else { return };
    for name in ["bouncing-ball", "shapes-gradient", "legacy-masks", "precomp-matte-text", "studio-features"] {
        let a = open(&format!("{name}.json"));
        for f in [5.0, 12.0, 20.0, 37.0] {
            let Ok(theirs) = std::fs::read(format!("{dir}/{name}/{f}.rgba")) else { continue };
            let ours = a.render_rgba(f, a.width, a.height);
            let p = psnr(&over_white(&ours), &over_white(&theirs));
            eprintln!("{name} frame {f}: PSNR {p:.1} dB");
            let (w, h) = (a.width as usize, a.height as usize);
            let mut side = vec![0u8; w * 2 * h * 4];
            for y in 0..h {
                side[y * w * 8..y * w * 8 + w * 4].copy_from_slice(&ours[y * w * 4..(y + 1) * w * 4]);
                side[y * w * 8 + w * 4..(y + 1) * w * 8].copy_from_slice(&theirs[y * w * 4..(y + 1) * w * 4]);
            }
            let png = studio_media::png::encode(&studio_media::rgba::RgbaImage { width: 2 * w as u32, height: h as u32, pixels: side });
            std::fs::write(format!("{dir}/{name}/{f}-side.png"), png).unwrap();
        }
    }
}

/// Writes the other player's frames as the goldens `golden_frames_match` reads
/// (run by hand with `$LOTTIE_ORACLE` set, after a fixture changes).
#[test]
#[ignore]
fn write_goldens() {
    let Ok(dir) = std::env::var("LOTTIE_ORACLE") else { return };
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    std::fs::create_dir_all(&out).unwrap();
    for (name, f) in GOLDEN {
        let a = open(&format!("{name}.json"));
        let raw = std::fs::read(format!("{dir}/{name}/{f}.rgba")).unwrap();
        let png = studio_media::png::encode(&studio_media::rgba::RgbaImage { width: a.width, height: a.height, pixels: raw });
        std::fs::write(out.join(format!("{name}-{f}.png")), png).unwrap();
    }
}

/// Frames drawn by an independent player (EffectCraft's Lottie importer and
/// CPU compositor, `tests/golden/README.md`), and the PSNR this player
/// reaches against each over white. shapes-gradient is left out: there the
/// two differ on where a rectangle's path starts under trim paths, and this
/// one follows lottie-web.
const GOLDEN: [(&str, f64); 8] = [("bouncing-ball", 5.0), ("bouncing-ball", 12.0), ("legacy-masks", 12.0), ("precomp-matte-text", 12.0), ("studio-features", 5.0), ("studio-features", 12.0), ("studio-features", 20.0), ("studio-features", 37.0)];

#[test]
fn golden_frames_match() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    for (name, f) in GOLDEN {
        let a = open(&format!("{name}.json"));
        let want = studio_media::png::read(&dir.join(format!("{name}-{f}.png"))).unwrap();
        let ours = a.render_rgba(f, a.width, a.height);
        let p = psnr(&over_white(&ours), &over_white(&want.pixels));
        assert!(p > 33.0, "{name} frame {f}: PSNR {p:.1} dB against the reference");
    }
}
