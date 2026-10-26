// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tolerant golden-image comparison.
//!
//! Goldens are small PNGs committed next to the tests. A comparison
//! passes when every channel is within `max_channel` for all but an
//! `outlier_fraction` of pixels, the mean channel error is under
//! `mean_channel`, and (optionally) the luma SSIM is at least `ssim_min`.
//! On failure an HTML report with expected / actual / amplified difference
//! and the statistics is written under `target-qa/golden-reports/`.
//! `UPDATE_GOLDEN=1` writes the actual image as the new golden instead.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use studio_media::RgbaImage;

/// An 8-bit RGBA picture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rgba8 {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Rgba8 {
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Rgba8 {
        assert_eq!(data.len(), (width * height * 4) as usize, "RGBA8 size mismatch");
        Rgba8 { width, height, data }
    }

    pub fn from_image(img: &RgbaImage) -> Rgba8 {
        Rgba8 { width: img.width, height: img.height, data: img.pixels.clone() }
    }

    pub fn to_image(&self) -> RgbaImage {
        RgbaImage { width: self.width, height: self.height, pixels: self.data.clone() }
    }

    /// From a premultiplied linear f32 frame (as `render` and `fx` use),
    /// encoded to sRGB with straight alpha.
    pub fn from_linear_premultiplied(width: u32, height: u32, px: impl Iterator<Item = [f32; 4]>) -> Rgba8 {
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for p in px {
            let a = p[3].clamp(0.0, 1.0);
            let un = if a > 0.0 { [p[0] / a, p[1] / a, p[2] / a] } else { [0.0; 3] };
            for c in un {
                data.push((linear_to_srgb(c.clamp(0.0, 1.0)) * 255.0).round() as u8);
            }
            data.push((a * 255.0).round() as u8);
        }
        Rgba8 { width, height, data }
    }

    pub fn load_png(path: &Path) -> Result<Rgba8, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let img = studio_media::png::decode(&bytes).map_err(|e| format!("decode {}: {e}", path.display()))?;
        Ok(Rgba8::from_image(&img))
    }

    pub fn save_png(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, studio_media::png::encode(&self.to_image()))
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [self.data[i], self.data[i + 1], self.data[i + 2], self.data[i + 3]]
    }
}

fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.003_130_8 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// How close is close enough.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tolerance {
    /// Largest per-channel difference a pixel may have and still count as matching.
    pub max_channel: u8,
    /// Fraction of pixels allowed beyond `max_channel`.
    pub outlier_fraction: f32,
    /// Largest allowed mean absolute channel difference.
    pub mean_channel: f32,
    /// Smallest allowed luma SSIM (8×8 windows), if checked.
    pub ssim_min: Option<f32>,
}

impl Tolerance {
    /// The render crate's convention: 3/255 on 99.5 % of pixels.
    pub const RENDER: Tolerance = Tolerance { max_channel: 3, outlier_fraction: 0.005, mean_channel: 1.0, ssim_min: None };
    /// Bit-exact.
    pub const EXACT: Tolerance = Tolerance { max_channel: 0, outlier_fraction: 0.0, mean_channel: 0.0, ssim_min: None };
    /// Loose, for lossy codecs and GPU/CPU agreement.
    pub const LOOSE: Tolerance = Tolerance { max_channel: 12, outlier_fraction: 0.02, mean_channel: 3.0, ssim_min: Some(0.9) };
}

/// What a comparison measured.
#[derive(Clone, Debug, PartialEq)]
pub struct Diff {
    pub max_channel: u8,
    pub mean_channel: f32,
    pub outlier_fraction: f32,
    pub ssim: f32,
    pub size_mismatch: bool,
}

impl Diff {
    pub fn within(&self, tol: &Tolerance) -> bool {
        !self.size_mismatch && self.outlier_fraction <= tol.outlier_fraction && self.mean_channel <= tol.mean_channel && tol.ssim_min.is_none_or(|m| self.ssim >= m) && (self.outlier_fraction > 0.0 || self.max_channel <= tol.max_channel)
    }
}

/// Compares two pictures.
pub fn diff(expected: &Rgba8, actual: &Rgba8, tol: &Tolerance) -> Diff {
    if expected.width != actual.width || expected.height != actual.height {
        return Diff { max_channel: 255, mean_channel: 255.0, outlier_fraction: 1.0, ssim: 0.0, size_mismatch: true };
    }
    let mut max = 0u8;
    let mut sum = 0u64;
    let mut outliers = 0usize;
    let n_px = (expected.width * expected.height) as usize;
    for (e, a) in expected.data.chunks_exact(4).zip(actual.data.chunks_exact(4)) {
        let mut px_max = 0u8;
        for c in 0..4 {
            let d = (e[c] as i16 - a[c] as i16).unsigned_abs() as u8;
            sum += d as u64;
            px_max = px_max.max(d);
        }
        max = max.max(px_max);
        if px_max > tol.max_channel {
            outliers += 1;
        }
    }
    Diff { max_channel: max, mean_channel: sum as f32 / (n_px.max(1) * 4) as f32, outlier_fraction: outliers as f32 / n_px.max(1) as f32, ssim: ssim_luma(expected, actual), size_mismatch: false }
}

fn luma(p: &[u8]) -> f32 {
    (p[0] as f32 * 0.2126 + p[1] as f32 * 0.7152 + p[2] as f32 * 0.0722) * (p[3] as f32 / 255.0)
}

/// Mean SSIM over non-overlapping 8×8 windows of luma (standard constants).
pub fn ssim_luma(a: &Rgba8, b: &Rgba8) -> f32 {
    if a.width != b.width || a.height != b.height || a.width < 8 || a.height < 8 {
        return if a == b { 1.0 } else { 0.0 };
    }
    let (c1, c2) = ((0.01f32 * 255.0).powi(2), (0.03f32 * 255.0).powi(2));
    let mut total = 0.0f32;
    let mut windows = 0usize;
    for wy in (0..a.height - 7).step_by(8) {
        for wx in (0..a.width - 7).step_by(8) {
            let (mut ma, mut mb, mut va, mut vb, mut cov) = (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
            let mut xs = [0.0f32; 64];
            let mut ys = [0.0f32; 64];
            for dy in 0..8 {
                for dx in 0..8 {
                    let i = (((wy + dy) * a.width + wx + dx) * 4) as usize;
                    let (x, y) = (luma(&a.data[i..i + 4]), luma(&b.data[i..i + 4]));
                    xs[(dy * 8 + dx) as usize] = x;
                    ys[(dy * 8 + dx) as usize] = y;
                    ma += x;
                    mb += y;
                }
            }
            ma /= 64.0;
            mb /= 64.0;
            for k in 0..64 {
                va += (xs[k] - ma).powi(2);
                vb += (ys[k] - mb).powi(2);
                cov += (xs[k] - ma) * (ys[k] - mb);
            }
            va /= 63.0;
            vb /= 63.0;
            cov /= 63.0;
            total += ((2.0 * ma * mb + c1) * (2.0 * cov + c2)) / ((ma * ma + mb * mb + c1) * (va + vb + c2));
            windows += 1;
        }
    }
    if windows == 0 { 1.0 } else { total / windows as f32 }
}

/// Where failure reports go.
pub fn reports_dir() -> PathBuf {
    let d = crate::fixtures::qa_target_dir().join("golden-reports");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn update_mode() -> bool {
    std::env::var("UPDATE_GOLDEN").map(|v| v == "1").unwrap_or(false)
}

/// Compares `actual` with the golden `<dir>/<name>.png`. Writes the golden
/// when `UPDATE_GOLDEN=1`; otherwise a mismatch (or a missing golden) is an
/// `Err` with a message that names the HTML report.
pub fn check_golden(dir: &Path, name: &str, actual: &Rgba8, tol: &Tolerance) -> Result<(), String> {
    let golden = dir.join(format!("{name}.png"));
    if update_mode() {
        actual.save_png(&golden).map_err(|e| format!("write golden {}: {e}", golden.display()))?;
        eprintln!("golden {name}: written to {}", golden.display());
        return Ok(());
    }
    let expected = match Rgba8::load_png(&golden) {
        Ok(e) => e,
        Err(e) => {
            let report = write_report(name, None, actual, None, tol);
            return Err(format!("golden {name} missing or unreadable ({e}). Run with UPDATE_GOLDEN=1 to create it. Actual saved in {}", report.display()));
        }
    };
    let d = diff(&expected, actual, tol);
    if d.within(tol) {
        return Ok(());
    }
    let report = write_report(name, Some(&expected), actual, Some(&d), tol);
    Err(format!(
        "golden {name} mismatch: max {} mean {:.3} outliers {:.3}% ssim {:.4}{} (tolerance max {} mean {} outliers {:.3}%). Report: {}",
        d.max_channel,
        d.mean_channel,
        d.outlier_fraction * 100.0,
        d.ssim,
        if d.size_mismatch { " SIZE MISMATCH" } else { "" },
        tol.max_channel,
        tol.mean_channel,
        tol.outlier_fraction * 100.0,
        report.display()
    ))
}

/// Amplified absolute difference picture (×8, clamped), opaque.
pub fn diff_image(expected: &Rgba8, actual: &Rgba8) -> Rgba8 {
    let (w, h) = (expected.width.min(actual.width), expected.height.min(actual.height));
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let e = expected.pixel(x, y);
            let a = actual.pixel(x, y);
            for c in 0..3 {
                data.push(((e[c] as i16 - a[c] as i16).unsigned_abs() as u16 * 8).min(255) as u8);
            }
            data.push(255);
        }
    }
    Rgba8 { width: w, height: h, data }
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

fn img_tag(label: &str, img: &Rgba8) -> String {
    let png = studio_media::png::encode(&img.to_image());
    let scale = (256 / img.width.max(1)).max(1);
    format!(
        "<figure><figcaption>{label} ({}×{})</figcaption><img style=\"image-rendering:pixelated;width:{}px\" src=\"data:image/png;base64,{}\"></figure>",
        img.width,
        img.height,
        img.width * scale,
        base64(&png)
    )
}

fn write_report(name: &str, expected: Option<&Rgba8>, actual: &Rgba8, d: Option<&Diff>, tol: &Tolerance) -> PathBuf {
    let dir = reports_dir();
    let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    let path = dir.join(format!("{safe}.html"));
    let _ = actual.save_png(&dir.join(format!("{safe}.actual.png")));
    let mut html = String::new();
    let _ = write!(
        html,
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Golden report: {name}</title><style>body{{font-family:system-ui;background:#1b1b1e;color:#ddd;padding:24px}}figure{{display:inline-block;margin:8px;vertical-align:top}}figcaption{{font-size:12px;color:#aaa;margin-bottom:4px}}img{{background:repeating-conic-gradient(#444 0 25%,#333 0 50%) 0 0/16px 16px}}table{{border-collapse:collapse}}td,th{{padding:4px 12px;text-align:left;border-bottom:1px solid #333}}</style></head><body><h1>{name}</h1>"
    );
    match (expected, d) {
        (Some(e), Some(d)) => {
            let _ = write!(
                html,
                "<table><tr><th></th><th>measured</th><th>tolerance</th></tr><tr><td>max channel</td><td>{}</td><td>{}</td></tr><tr><td>mean channel</td><td>{:.3}</td><td>{}</td></tr><tr><td>outliers</td><td>{:.3} %</td><td>{:.3} %</td></tr><tr><td>SSIM</td><td>{:.4}</td><td>{}</td></tr><tr><td>size</td><td>{}×{} vs {}×{}</td><td></td></tr></table>",
                d.max_channel,
                tol.max_channel,
                d.mean_channel,
                tol.mean_channel,
                d.outlier_fraction * 100.0,
                tol.outlier_fraction * 100.0,
                d.ssim,
                tol.ssim_min.map(|s| format!("≥ {s}")).unwrap_or_else(|| "not checked".into()),
                e.width,
                e.height,
                actual.width,
                actual.height
            );
            html.push_str(&img_tag("expected", e));
            html.push_str(&img_tag("actual", actual));
            html.push_str(&img_tag("difference ×8", &diff_image(e, actual)));
        }
        _ => {
            html.push_str("<p>No golden exists yet. Run with <code>UPDATE_GOLDEN=1</code> to accept the actual image.</p>");
            html.push_str(&img_tag("actual", actual));
        }
    }
    html.push_str("</body></html>");
    let _ = std::fs::write(&path, html);
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pic(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Rgba8 {
        let mut d = Vec::new();
        for y in 0..h {
            for x in 0..w {
                d.extend_from_slice(&f(x, y));
            }
        }
        Rgba8::new(w, h, d)
    }

    #[test]
    fn identical_pictures_pass_exact() {
        let a = pic(16, 16, |x, y| [x as u8 * 10, y as u8 * 10, 7, 255]);
        let d = diff(&a, &a, &Tolerance::EXACT);
        assert!(d.within(&Tolerance::EXACT));
        assert_eq!(d.ssim, 1.0);
    }

    #[test]
    fn small_noise_passes_render_but_not_exact() {
        let a = pic(16, 16, |x, y| [x as u8 * 10, y as u8 * 10, 7, 255]);
        let b = pic(16, 16, |x, y| [x as u8 * 10 + ((x + y) % 3) as u8, y as u8 * 10, 7, 255]);
        assert!(diff(&a, &b, &Tolerance::RENDER).within(&Tolerance::RENDER));
        assert!(!diff(&a, &b, &Tolerance::EXACT).within(&Tolerance::EXACT));
    }

    #[test]
    fn one_wrong_block_fails_render_tolerance() {
        let a = pic(32, 32, |_, _| [100, 100, 100, 255]);
        let b = pic(32, 32, |x, y| if x < 8 && y < 8 { [200, 100, 100, 255] } else { [100, 100, 100, 255] });
        let d = diff(&a, &b, &Tolerance::RENDER);
        assert!(!d.within(&Tolerance::RENDER));
        assert!(d.outlier_fraction > 0.06);
    }

    #[test]
    fn size_mismatch_fails_and_reports() {
        let a = pic(8, 8, |_, _| [0, 0, 0, 255]);
        let b = pic(9, 8, |_, _| [0, 0, 0, 255]);
        assert!(diff(&a, &b, &Tolerance::LOOSE).size_mismatch);
        let dir = crate::fixtures::scratch_dir("golden-size");
        a.save_png(&dir.join("g.png")).unwrap();
        let err = check_golden(&dir, "g", &b, &Tolerance::LOOSE).unwrap_err();
        assert!(err.contains("SIZE MISMATCH"), "{err}");
        assert!(err.contains("golden-reports"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn base64_matches_known_values() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
    }
}
