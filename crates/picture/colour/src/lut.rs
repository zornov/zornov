// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `.cube` LUTs (1D and 3D, any size) with tetrahedral interpolation,
//! an intensity mix and optional skin-tone protection.
//!
//! The parser follows the public Cube LUT format: `TITLE`, `LUT_1D_SIZE`,
//! `LUT_3D_SIZE`, `DOMAIN_MIN`, `DOMAIN_MAX`, `#` comments and one RGB
//! triple per line with red changing fastest. Files that carry both a 1D
//! and a 3D table (a "shaper" plus a cube) are accepted; the 1D table is
//! applied first. `LUT_1D_INPUT_RANGE` / `LUT_3D_INPUT_RANGE` lines are
//! read as a shared domain.
//!
//! # Skin protection
//!
//! Skin tones cluster around one hue line in Y′CbCr (the vectorscope "skin
//! line", ≈123° counter-clockwise from +Cb, on the I axis of YIQ). The
//! mask is computed on the BT.709 gamma-encoded *input* of the LUT:
//!
//! ```text
//! angle  = exp(−(Δθ / 18°)²)                  Δθ: distance to 123°
//! chroma = smoothstep(0.02, 0.06, c) · (1 − smoothstep(0.30, 0.45, c))
//! luma   = smoothstep(0.05, 0.15, Y′) · (1 − smoothstep(0.90, 1.0, Y′))
//! mask   = angle · chroma · luma
//! ```
//!
//! where `c = √(Cb² + Cr²)`. The LUT is then mixed in with
//! `intensity × (1 − protect × mask)`.

use std::fmt::Write as _;
use std::sync::Arc;

use crate::math::{Mat3, Rgb, smoothstep};
use crate::primaries::{Primaries, rgb_to_rgb};
use crate::transfer::Transfer;
use crate::ycbcr::YCbCrMatrix;

/// Skin line angle in the Cb/Cr plane, degrees counter-clockwise from +Cb.
pub const SKIN_LINE_DEG: f32 = 123.0;

/// A 1D LUT: one table per channel.
#[derive(Clone, Debug, PartialEq)]
pub struct Lut1d {
    pub data: Vec<Rgb>,
    pub domain_min: Rgb,
    pub domain_max: Rgb,
}

/// A 3D LUT with `size³` entries, red index changing fastest.
#[derive(Clone, Debug, PartialEq)]
pub struct Lut3d {
    pub size: usize,
    pub data: Vec<Rgb>,
    pub domain_min: Rgb,
    pub domain_max: Rgb,
}

/// Contents of a `.cube` file.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct CubeFile {
    pub title: Option<String>,
    pub lut1d: Option<Lut1d>,
    pub lut3d: Option<Lut3d>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CubeError {
    /// A line could not be understood (1-based line number).
    Syntax { line: usize, message: String },
    /// Neither a 1D nor a 3D size was given.
    NoTable,
    /// The number of data lines does not match the declared sizes.
    WrongCount { expected: usize, found: usize },
    /// A size outside 2..=256 (3D) or 2..=65536 (1D).
    BadSize(usize),
}

impl std::fmt::Display for CubeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CubeError::Syntax { line, message } => write!(f, "line {line}: {message}"),
            CubeError::NoTable => write!(f, "no LUT_1D_SIZE or LUT_3D_SIZE"),
            CubeError::WrongCount { expected, found } => write!(f, "expected {expected} entries, found {found}"),
            CubeError::BadSize(n) => write!(f, "unsupported LUT size {n}"),
        }
    }
}

impl std::error::Error for CubeError {}

fn parse_triple(parts: &[&str], line: usize) -> Result<Rgb, CubeError> {
    if parts.len() != 3 {
        return Err(CubeError::Syntax { line, message: format!("expected 3 numbers, found {}", parts.len()) });
    }
    let mut v = [0.0; 3];
    for (o, s) in v.iter_mut().zip(parts) {
        *o = s.parse::<f32>().map_err(|_| CubeError::Syntax { line, message: format!("bad number {s:?}") })?;
    }
    Ok(v)
}

/// Parses `.cube` text.
pub fn parse_cube(text: &str) -> Result<CubeFile, CubeError> {
    let mut title = None;
    let mut size1 = None;
    let mut size3 = None;
    let mut dmin = [0.0f32; 3];
    let mut dmax = [1.0f32; 3];
    let mut range1: Option<(f32, f32)> = None;
    let mut range3: Option<(f32, f32)> = None;
    let mut data = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line_no = i + 1;
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let first = line.split_whitespace().next().unwrap_or("");
        let starts_numeric = first.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '+' || c == '.');
        if starts_numeric {
            let parts: Vec<&str> = line.split_whitespace().collect();
            data.push(parse_triple(&parts, line_no)?);
            continue;
        }
        let rest = line[first.len()..].trim();
        let parts: Vec<&str> = rest.split_whitespace().collect();
        let size = |p: &[&str]| -> Result<usize, CubeError> {
            p.first()
                .and_then(|s| s.parse::<usize>().ok())
                .ok_or(CubeError::Syntax { line: line_no, message: "bad size".into() })
        };
        let range = |p: &[&str]| -> Result<(f32, f32), CubeError> {
            match (p.first().and_then(|s| s.parse().ok()), p.get(1).and_then(|s| s.parse().ok())) {
                (Some(a), Some(b)) => Ok((a, b)),
                _ => Err(CubeError::Syntax { line: line_no, message: "bad range".into() }),
            }
        };
        match first {
            "TITLE" => title = Some(rest.trim_matches('"').to_string()),
            "LUT_1D_SIZE" => size1 = Some(size(&parts)?),
            "LUT_3D_SIZE" => size3 = Some(size(&parts)?),
            "DOMAIN_MIN" => dmin = parse_triple(&parts, line_no)?,
            "DOMAIN_MAX" => dmax = parse_triple(&parts, line_no)?,
            "LUT_1D_INPUT_RANGE" => range1 = Some(range(&parts)?),
            "LUT_3D_INPUT_RANGE" => range3 = Some(range(&parts)?),
            // Unknown keywords are skipped, as the format allows.
            _ => {}
        }
    }
    if size1.is_none() && size3.is_none() {
        return Err(CubeError::NoTable);
    }
    if let Some(n) = size1
        && !(2..=65536).contains(&n)
    {
        return Err(CubeError::BadSize(n));
    }
    if let Some(n) = size3
        && !(2..=256).contains(&n)
    {
        return Err(CubeError::BadSize(n));
    }
    let n1 = size1.unwrap_or(0);
    let n3 = size3.map(|n| n * n * n).unwrap_or(0);
    if data.len() != n1 + n3 {
        return Err(CubeError::WrongCount { expected: n1 + n3, found: data.len() });
    }
    let dom = |r: Option<(f32, f32)>| r.map(|(a, b)| ([a; 3], [b; 3])).unwrap_or((dmin, dmax));
    let lut1d = size1.map(|n| {
        let (lo, hi) = dom(range1);
        Lut1d { data: data[..n].to_vec(), domain_min: lo, domain_max: hi }
    });
    let lut3d = size3.map(|n| {
        let (lo, hi) = dom(range3);
        Lut3d { size: n, data: data[n1..].to_vec(), domain_min: lo, domain_max: hi }
    });
    Ok(CubeFile { title, lut1d, lut3d })
}

/// Writes a 3D LUT as `.cube` text.
pub fn write_cube(lut: &Lut3d, title: &str) -> String {
    let mut s = String::with_capacity(lut.data.len() * 30 + 128);
    let _ = writeln!(s, "TITLE \"{title}\"");
    let _ = writeln!(s, "LUT_3D_SIZE {}", lut.size);
    if lut.domain_min != [0.0; 3] || lut.domain_max != [1.0; 3] {
        let (a, b) = (lut.domain_min, lut.domain_max);
        let _ = writeln!(s, "DOMAIN_MIN {} {} {}", a[0], a[1], a[2]);
        let _ = writeln!(s, "DOMAIN_MAX {} {} {}", b[0], b[1], b[2]);
    }
    for c in &lut.data {
        let _ = writeln!(s, "{:.6} {:.6} {:.6}", c[0], c[1], c[2]);
    }
    s
}

/// Parses an Autodesk / Lustre `.3dl` 3D LUT.
///
/// The format is a list of integer (or float) triples with **blue**
/// changing fastest, optionally preceded by one line of shaper input
/// positions (as many numbers as the cube's size; Studio uses the cube's
/// own even spacing) and optional `3DMESH` / `Mesh <in bits> <out bits>`
/// header lines. The output scale comes from the `Mesh` line, else from
/// the largest value (1023, 4095 or 65535 full scale); float data are
/// taken as they are.
pub fn parse_3dl(text: &str) -> Result<CubeFile, CubeError> {
    let mut rows: Vec<(usize, Vec<f32>)> = Vec::new();
    let mut out_bits: Option<u32> = None;
    let mut floats = false;
    for (i, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("3dmesh") {
            continue;
        }
        if let Some(rest) = lower.strip_prefix("mesh") {
            let n: Vec<u32> = rest.split_whitespace().filter_map(|v| v.parse().ok()).collect();
            if n.len() == 2 {
                out_bits = Some(n[1]);
            }
            continue;
        }
        if lower.starts_with("lut") || lower.starts_with("gamma") || lower.starts_with("title") {
            continue;
        }
        let mut v = Vec::new();
        for p in line.split_whitespace() {
            floats |= p.contains('.') || p.contains('e');
            v.push(p.parse::<f32>().map_err(|_| CubeError::Syntax { line: i + 1, message: format!("bad number {p:?}") })?);
        }
        rows.push((i + 1, v));
    }
    // A leading row that is not a triple is the shaper line.
    let shaper = rows.first().filter(|r| r.1.len() != 3).map(|r| r.1.len());
    let start = usize::from(shaper.is_some());
    let data: Vec<Rgb> = rows[start..]
        .iter()
        .map(|(line, v)| if v.len() == 3 { Ok([v[0], v[1], v[2]]) } else { Err(CubeError::Syntax { line: *line, message: format!("expected 3 numbers, found {}", v.len()) }) })
        .collect::<Result<_, _>>()?;
    if data.is_empty() {
        return Err(CubeError::NoTable);
    }
    let size = (data.len() as f64).cbrt().round() as usize;
    if size * size * size != data.len() {
        return Err(CubeError::WrongCount { expected: size * size * size, found: data.len() });
    }
    if !(2..=256).contains(&size) {
        return Err(CubeError::BadSize(size));
    }
    let scale = if floats && out_bits.is_none() {
        1.0
    } else {
        let max = data.iter().flat_map(|c| c.iter().copied()).fold(0.0f32, f32::max);
        let bits = out_bits.unwrap_or(if max <= 1023.0 { 10 } else if max <= 4095.0 { 12 } else { 16 });
        ((1u64 << bits) - 1) as f32
    };
    // Reorder blue-fastest into red-fastest.
    let mut out = vec![[0.0f32; 3]; data.len()];
    for r in 0..size {
        for g in 0..size {
            for b in 0..size {
                let c = data[(r * size + g) * size + b];
                out[(b * size + g) * size + r] = c.map(|v| v / scale);
            }
        }
    }
    Ok(CubeFile { title: None, lut1d: None, lut3d: Some(Lut3d { size, data: out, domain_min: [0.0; 3], domain_max: [1.0; 3] }) })
}

/// Writes a 3D LUT as `.3dl` text: a 10-bit shaper line, then 12-bit
/// integer triples with blue changing fastest (what grading systems
/// read). Values are clamped to 0..1.
pub fn write_3dl(lut: &Lut3d) -> String {
    let n = lut.size;
    let mut s = String::with_capacity(n * n * n * 16 + 64);
    let shaper: Vec<String> = (0..n).map(|i| ((i as f64 / (n - 1) as f64) * 1023.0).round().to_string()).collect();
    let _ = writeln!(s, "{}", shaper.join(" "));
    for r in 0..n {
        for g in 0..n {
            for b in 0..n {
                let c = lut.at(r, g, b).map(|v| (v.clamp(0.0, 1.0) * 4095.0).round() as u32);
                let _ = writeln!(s, "{} {} {}", c[0], c[1], c[2]);
            }
        }
    }
    s
}

/// Reads a LUT file by extension: `.cube` or `.3dl`.
pub fn read_lut(path: &std::path::Path) -> Result<CubeFile, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let r = if ext == "3dl" { parse_3dl(&text) } else { parse_cube(&text) };
    r.map_err(|e| format!("{}: {e}", path.display()))
}

/// Writes a 3D LUT by extension (`.3dl`, otherwise `.cube`).
pub fn write_lut(path: &std::path::Path, lut: &Lut3d, title: &str) -> Result<(), String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let text = if ext == "3dl" { write_3dl(lut) } else { write_cube(lut, title) };
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

impl Lut1d {
    pub fn sample(&self, c: Rgb) -> Rgb {
        let n = self.data.len();
        let mut o = [0.0; 3];
        for ch in 0..3 {
            let span = self.domain_max[ch] - self.domain_min[ch];
            let t = ((c[ch] - self.domain_min[ch]) / span).clamp(0.0, 1.0) * (n - 1) as f32;
            let i = (t as usize).min(n - 2);
            let f = t - i as f32;
            o[ch] = self.data[i][ch] + f * (self.data[i + 1][ch] - self.data[i][ch]);
        }
        o
    }
}

impl Lut3d {
    /// Builds a LUT by sampling `f` on the lattice over 0..1.
    pub fn from_fn(size: usize, f: impl Fn(Rgb) -> Rgb) -> Lut3d {
        let s = (size - 1) as f32;
        let mut data = Vec::with_capacity(size * size * size);
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    data.push(f([r as f32 / s, g as f32 / s, b as f32 / s]));
                }
            }
        }
        Lut3d { size, data, domain_min: [0.0; 3], domain_max: [1.0; 3] }
    }

    pub fn identity(size: usize) -> Lut3d {
        Lut3d::from_fn(size, |c| c)
    }

    #[inline]
    pub fn at(&self, r: usize, g: usize, b: usize) -> Rgb {
        self.data[(b * self.size + g) * self.size + r]
    }

    /// Tetrahedral interpolation: the lattice cube around the input is split
    /// into six tetrahedra along its grey diagonal, and the output is the
    /// barycentric blend of the four corners of the one containing the
    /// input. It keeps neutrals on the diagonal exact and is smoother than
    /// trilinear for the same lattice. Inputs outside the domain clamp.
    pub fn sample(&self, c: Rgb) -> Rgb {
        let n = self.size;
        let s = (n - 1) as f32;
        let mut idx = [0usize; 3];
        let mut f = [0.0f32; 3];
        for ch in 0..3 {
            let span = self.domain_max[ch] - self.domain_min[ch];
            let t = ((c[ch] - self.domain_min[ch]) / span).clamp(0.0, 1.0) * s;
            let i = (t as usize).min(n - 2);
            idx[ch] = i;
            f[ch] = t - i as f32;
        }
        let [r, g, b] = idx;
        let [fr, fg, fb] = f;
        let c000 = self.at(r, g, b);
        let c111 = self.at(r + 1, g + 1, b + 1);
        // Each branch walks from c000 to c111 along the edges in order of
        // decreasing fraction.
        let (w, p1, p2) = if fr > fg {
            if fg > fb {
                ([fr, fg, fb], self.at(r + 1, g, b), self.at(r + 1, g + 1, b))
            } else if fr > fb {
                ([fr, fb, fg], self.at(r + 1, g, b), self.at(r + 1, g, b + 1))
            } else {
                ([fb, fr, fg], self.at(r, g, b + 1), self.at(r + 1, g, b + 1))
            }
        } else if fb > fg {
            ([fb, fg, fr], self.at(r, g, b + 1), self.at(r, g + 1, b + 1))
        } else if fb > fr {
            ([fg, fb, fr], self.at(r, g + 1, b), self.at(r, g + 1, b + 1))
        } else {
            ([fg, fr, fb], self.at(r, g + 1, b), self.at(r + 1, g + 1, b))
        };
        let mut o = [0.0; 3];
        for ch in 0..3 {
            o[ch] = c000[ch]
                + w[0] * (p1[ch] - c000[ch])
                + w[1] * (p2[ch] - p1[ch])
                + w[2] * (c111[ch] - p2[ch]);
        }
        o
    }
}

impl CubeFile {
    pub fn sample(&self, c: Rgb) -> Rgb {
        let mut c = c;
        if let Some(l) = &self.lut1d {
            c = l.sample(c);
        }
        if let Some(l) = &self.lut3d {
            c = l.sample(c);
        }
        c
    }
}

/// The signal a LUT expects as input (and produces as output).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LutSpace {
    /// BT.709 primaries, BT.1886 (2.4 gamma) encoded — what most creative
    /// `.cube` looks expect, and what Studio's own looks use.
    #[default]
    Rec709,
    /// BT.709 primaries, sRGB encoded.
    Srgb,
    /// AP1 primaries, ACEScct encoded.
    AcesCct,
    /// Working primaries, linear.
    Linear,
}

impl LutSpace {
    pub fn primaries(self, working: &Primaries) -> Primaries {
        match self {
            LutSpace::Rec709 | LutSpace::Srgb => Primaries::BT709,
            LutSpace::AcesCct => Primaries::ACES_AP1,
            LutSpace::Linear => *working,
        }
    }

    pub fn transfer(self) -> Transfer {
        match self {
            LutSpace::Rec709 => Transfer::Bt1886,
            LutSpace::Srgb => Transfer::Srgb,
            LutSpace::AcesCct => Transfer::AcesCct,
            LutSpace::Linear => Transfer::Linear,
        }
    }
}

/// A LUT placed in a grade.
#[derive(Clone, Debug)]
pub struct LutSettings {
    pub lut: Arc<CubeFile>,
    pub space: LutSpace,
    /// 0..1 mix with the input.
    pub intensity: f32,
    /// 0..1 strength of skin-tone protection.
    pub skin_protect: f32,
}

impl PartialEq for LutSettings {
    fn eq(&self, o: &LutSettings) -> bool {
        Arc::ptr_eq(&self.lut, &o.lut)
            && self.space == o.space
            && self.intensity == o.intensity
            && self.skin_protect == o.skin_protect
    }
}

impl LutSettings {
    pub fn new(lut: Arc<CubeFile>) -> LutSettings {
        LutSettings { lut, space: LutSpace::Rec709, intensity: 1.0, skin_protect: 0.0 }
    }

    pub fn prepare(&self, working: &Primaries) -> PreparedLut {
        let p = self.space.primaries(working);
        PreparedLut {
            lut: self.lut.clone(),
            to_lut: rgb_to_rgb(working, &p),
            from_lut: rgb_to_rgb(&p, working),
            transfer: self.space.transfer(),
            intensity: self.intensity,
            skin_protect: self.skin_protect,
            // The skin mask always looks at BT.709 video-style values.
            to_709: rgb_to_rgb(working, &Primaries::BT709),
        }
    }
}

/// Soft skin-tone weight of a BT.709 gamma-encoded colour.
pub fn skin_mask(encoded709: Rgb) -> f32 {
    let ycc = YCbCrMatrix::Bt709.to_ycbcr(encoded709);
    let (y, cb, cr) = (ycc[0], ycc[1], ycc[2]);
    let c = cb.hypot(cr);
    let mut d = (cr.atan2(cb).to_degrees() - SKIN_LINE_DEG).abs();
    if d > 180.0 {
        d = 360.0 - d;
    }
    let angle = (-(d / 18.0) * (d / 18.0)).exp();
    let chroma = smoothstep(0.02, 0.06, c) * (1.0 - smoothstep(0.30, 0.45, c));
    let luma = smoothstep(0.05, 0.15, y) * (1.0 - smoothstep(0.90, 1.0, y));
    angle * chroma * luma
}

#[derive(Clone, Debug)]
pub struct PreparedLut {
    pub lut: Arc<CubeFile>,
    pub to_lut: Mat3,
    pub from_lut: Mat3,
    pub transfer: Transfer,
    pub intensity: f32,
    pub skin_protect: f32,
    pub to_709: Mat3,
}

impl PreparedLut {
    /// Applies the LUT to working linear RGB.
    pub fn apply(&self, lin: Rgb) -> Rgb {
        if self.intensity == 0.0 {
            return lin;
        }
        let mut k = self.intensity;
        if self.skin_protect > 0.0 {
            let e = Transfer::Bt1886.encode_rgb(self.to_709.apply(lin));
            k *= 1.0 - self.skin_protect * skin_mask(e);
        }
        let enc = self.transfer.encode_rgb(self.to_lut.apply(lin));
        let out = self.from_lut.apply(self.transfer.decode_rgb(self.lut.sample(enc)));
        if k == 1.0 { out } else { crate::math::lerp3(lin, out, k) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_lut_is_exact_on_lattice_and_close_between() {
        let l = Lut3d::identity(17);
        for c in [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [0.25, 0.5, 0.75], [0.123, 0.456, 0.789]] {
            let o = l.sample(c);
            for i in 0..3 {
                assert!((o[i] - c[i]).abs() < 1e-6, "{c:?} → {o:?}");
            }
        }
    }

    #[test]
    fn tetrahedral_reference_points() {
        // A 2-point lattice of f(r,g,b) = r·g (red output). Tetrahedral
        // interpolation of this product is exactly min(r, g) — unlike
        // trilinear, which would return r·g.
        let l = Lut3d::from_fn(2, |c| [c[0] * c[1], 0.0, 0.0]);
        let cases = [
            ([0.5, 0.25, 0.75], 0.25),
            ([0.8, 0.6, 0.1], 0.6),
            ([0.3, 0.9, 0.5], 0.3),
            ([0.7, 0.7, 0.7], 0.7),
            ([1.0, 0.0, 1.0], 0.0),
        ];
        for (c, expect) in cases {
            assert!((l.sample(c)[0] - expect).abs() < 1e-6, "{c:?}");
        }
        // Any affine map is reproduced exactly between lattice points.
        let aff = |c: Rgb| [0.2 + 0.5 * c[0] - 0.1 * c[2], 0.9 * c[1] + 0.05, c[0] * 0.3 + c[1] * 0.3 + c[2] * 0.3];
        let l = Lut3d::from_fn(5, aff);
        for c in [[0.13, 0.77, 0.42], [0.99, 0.01, 0.5], [0.6, 0.6, 0.2]] {
            let (o, e) = (l.sample(c), aff(c));
            for i in 0..3 {
                assert!((o[i] - e[i]).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn parse_and_write() {
        let text = "# comment\nTITLE \"Test\"\nLUT_3D_SIZE 2\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 1 1 1\n\
                    0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1 # last\n";
        let f = parse_cube(text).unwrap();
        assert_eq!(f.title.as_deref(), Some("Test"));
        let l = f.lut3d.as_ref().unwrap();
        assert_eq!(l.size, 2);
        assert_eq!(l.at(1, 0, 0), [1.0, 0.0, 0.0]);
        assert_eq!(l.at(0, 1, 1), [0.0, 1.0, 1.0]);
        let again = parse_cube(&write_cube(l, "Test")).unwrap();
        assert_eq!(again.lut3d.unwrap().data, l.data);

        let one = parse_cube("LUT_1D_SIZE 3\n0 0 0\n0.25 0.5 0.5\n1 1 1\n").unwrap();
        let s = one.sample([0.25, 0.5, 1.5]);
        assert!((s[0] - 0.125).abs() < 1e-6 && (s[1] - 0.5).abs() < 1e-6 && s[2] == 1.0);

        assert_eq!(parse_cube("LUT_3D_SIZE 2\n0 0 0\n"), Err(CubeError::WrongCount { expected: 8, found: 1 }));
        assert!(matches!(parse_cube("LUT_3D_SIZE 2\n0 0 x\n"), Err(CubeError::Syntax { line: 2, .. })));
        assert_eq!(parse_cube("TITLE \"x\"\n"), Err(CubeError::NoTable));
    }

    #[test]
    fn three_dl_round_trip() {
        let l = Lut3d::from_fn(9, |c| [c[0] * 0.9 + 0.05, c[1] * c[1], 1.0 - c[2]]);
        let text = write_3dl(&l);
        let back = parse_3dl(&text).unwrap().lut3d.unwrap();
        assert_eq!(back.size, 9);
        for (a, b) in back.data.iter().zip(&l.data) {
            for k in 0..3 {
                assert!((a[k] - b[k]).abs() <= 0.5 / 4095.0 + 1e-6);
            }
        }
        // Blue-fastest order: the second entry is (r=0, g=0, b=1).
        let first_lines: Vec<&str> = text.lines().skip(1).take(2).collect();
        assert_eq!(first_lines[1], format!("{} {} {}", 205, 0, (0.875f32 * 4095.0).round()));
        // Lustre header with explicit output depth.
        let lustre = "3DMESH\nMesh 1 10\n0 1023\n0 0 0\n0 0 1023\n0 1023 0\n0 1023 1023\n1023 0 0\n1023 0 1023\n1023 1023 0\n1023 1023 1023\n";
        let f = parse_3dl(lustre).unwrap().lut3d.unwrap();
        assert_eq!(f.size, 2);
        assert_eq!(f.at(1, 0, 0), [1.0, 0.0, 0.0]);
        assert_eq!(f.at(0, 0, 1), [0.0, 0.0, 1.0]);
    }

    #[test]
    fn domain_scaling() {
        let mut l = Lut3d::identity(3);
        l.domain_min = [-1.0; 3];
        l.domain_max = [3.0; 3];
        // Input 1.0 is halfway through the domain, so it reads lattice 0.5.
        assert!((l.sample([1.0; 3])[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn skin_protection() {
        let skin = Transfer::Bt1886.encode_rgb([0.85, 0.65, 0.55]);
        let sky = [0.3, 0.5, 0.85];
        assert!(skin_mask([0.85, 0.65, 0.55]) > 0.8, "{}", skin_mask(skin));
        assert!(skin_mask(sky) < 0.01);
        assert!(skin_mask([0.5; 3]) == 0.0);

        let invert = Arc::new(CubeFile { title: None, lut1d: None, lut3d: Some(Lut3d::from_fn(9, |c| c.map(|v| 1.0 - v))) });
        let mut s = LutSettings::new(invert);
        s.skin_protect = 1.0;
        let p = s.prepare(&Primaries::BT709);
        let lin_skin = Transfer::Bt1886.decode_rgb([0.85, 0.65, 0.55]);
        let out = p.apply(lin_skin);
        // Mostly protected: stays close to the input.
        assert!((out[0] - lin_skin[0]).abs() < 0.2 * lin_skin[0], "{out:?} vs {lin_skin:?}");
    }
}
