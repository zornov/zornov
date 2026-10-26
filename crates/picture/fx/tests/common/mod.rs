// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared helpers for the integration tests: test inputs and golden-image
//! comparison.

#![allow(dead_code)]

use std::path::PathBuf;

use studio_fx::testcard::test_card;
use studio_fx::Image;

pub const GOLDEN_W: usize = 96;
pub const GOLDEN_H: usize = 54;

/// Frames for time effects: the card with its objects a little further
/// left in each earlier frame, as if they were moving right.
pub fn history(w: usize, h: usize, n: usize) -> Vec<Image> {
    (1..=n).map(|k| test_card(w, h, -0.01 * k as f32)).collect()
}

/// Compares `img` with the golden PNG `name` under `tests/golden`. A
/// missing file is written (and the test passes), as is any file when
/// `FX_BLESS=1`, so goldens are regenerated deliberately.
///
/// Tolerance: every channel within 3/255 for at least 99.5 % of pixels and
/// within 32/255 everywhere. Small floating-point differences between
/// platforms stay inside that; real changes do not.
pub fn check_golden(name: &str, img: &Image) -> Result<(), String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(format!("{name}.png"));
    let bytes = img.to_srgb8();
    let bless = std::env::var("FX_BLESS").is_ok_and(|v| v == "1");
    if bless || !path.exists() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let rgba = studio_media::RgbaImage { width: img.width as u32, height: img.height as u32, pixels: bytes };
        std::fs::write(&path, studio_media::png::encode(&rgba)).unwrap();
        eprintln!("wrote golden {}", path.display());
        return Ok(());
    }
    let golden = studio_media::png::read(&path).map_err(|e| format!("{name}: {e}"))?;
    if (golden.width as usize, golden.height as usize) != (img.width, img.height) {
        return Err(format!("{name}: golden is {}x{}, got {}x{}", golden.width, golden.height, img.width, img.height));
    }
    let mut off = 0usize;
    let mut worst = 0u8;
    for (a, b) in golden.pixels.chunks_exact(4).zip(bytes.chunks_exact(4)) {
        let d = a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap();
        worst = worst.max(d);
        if d > 3 {
            off += 1;
        }
    }
    let pixels = img.width * img.height;
    if off * 200 > pixels || worst > 32 {
        return Err(format!("{name}: {off} of {pixels} pixels differ by more than 3/255 (worst {worst}/255)"));
    }
    Ok(())
}

/// Average absolute channel difference.
pub fn mean_abs_diff(a: &Image, b: &Image) -> f32 {
    let n = (a.pixels.len() * 4) as f32;
    a.pixels.iter().zip(&b.pixels).map(|(p, q)| (0..4).map(|i| (p[i] - q[i]).abs()).sum::<f32>()).sum::<f32>() / n
}
