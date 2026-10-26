// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Digital Cinema Packages: JPEG 2000 pictures (Studio's own encoder) in
//! an MXF track file, 24-bit PCM sound in another, and the CPL, PKL,
//! ASSETMAP and VOLINDEX that tie them into a folder a cinema server
//! ingests. SMPTE (ST 429-7/-8/-9) and Interop flavours.
//!
//! Pictures are converted from display RGB (Rec. 709, gamma 2.4) to
//! X'Y'Z' (DCI white, gamma 2.6, 12 bits) and placed in the container
//! (2048 x 1080 or 4096 x 2160) with the flat / scope active area.
//!
//! Not done: encryption and KDMs, digital signatures (the DCP is unsigned,
//! which is valid for unencrypted content), subtitle / caption tracks,
//! 3D, HFR beyond what the frame-rate table lists. Not run through a
//! cinema server or a certified DCP validator; FFmpeg and OpenJPEG decode
//! it (see the tests).

use crate::j2k;
use crate::mxf::{self, Essence, J2kDesc, MxfInfo, MxfWriter};
use crate::sha1;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standard {
    Smpte,
    Interop,
}

/// The active picture area inside the container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// 1.85:1 - 1998 x 1080 (2K), 3996 x 2160 (4K).
    Flat,
    /// 2.39:1 - 2048 x 858, 4096 x 1716.
    Scope,
    /// 1.9:1 - the whole container.
    Full,
}

#[derive(Clone, Debug)]
pub struct DcpSpec {
    pub title: String,
    /// Frames per second: 24, 25, 30, 48, 50 or 60.
    pub fps: u32,
    pub standard: Standard,
    pub shape: Shape,
    pub four_k: bool,
    /// feature, short, trailer, test, advertisement, teaser, policy, psa, rating, transitional
    pub kind: String,
    /// Language of the sound, ISO 639-1 ("en", "hi").
    pub language: String,
    pub issuer: String,
    pub creator: String,
    /// Sound channels in the track (6 for 5.1: L R C LFE Ls Rs).
    pub channels: u16,
    /// DCI cap on a frame, bytes (None: the profile default).
    pub frame_cap: Option<usize>,
    /// Quantiser step (smaller: finer).
    pub step: f32,
}

impl DcpSpec {
    pub fn new(title: &str, fps: u32) -> Self {
        DcpSpec { title: title.into(), fps, standard: Standard::Smpte, shape: Shape::Flat, four_k: false, kind: "feature".into(), language: "en".into(), issuer: "Studio".into(), creator: "Studio by Ashrox LLC".into(), channels: 6, frame_cap: None, step: 1.5 }
    }

    /// The container (stored picture) size.
    pub fn container(&self) -> (u32, u32) {
        if self.four_k { (4096, 2160) } else { (2048, 1080) }
    }

    /// The active picture size inside it.
    pub fn active(&self) -> (u32, u32) {
        let k = if self.four_k { 2 } else { 1 };
        match self.shape {
            Shape::Flat => (1998 * k, 1080 * k),
            Shape::Scope => (2048 * k, 858 * k),
            Shape::Full => (2048 * k, 1080 * k),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if ![24, 25, 30, 48, 50, 60].contains(&self.fps) {
            return Err(format!("{} fps is not a DCP frame rate (24, 25, 30, 48, 50, 60)", self.fps));
        }
        if self.four_k && self.fps > 30 {
            return Err("4K DCPs are limited to 30 fps".into());
        }
        if !(1..=16).contains(&self.channels) {
            return Err("1-16 sound channels".into());
        }
        Ok(())
    }

    pub fn j2k_params(&self) -> j2k::Params {
        let (w, h) = self.container();
        let mut p = if self.four_k { j2k::Params::dci_4k(w, h, self.fps as f64) } else { j2k::Params::dci_2k(w, h, self.fps as f64) };
        p.step = self.step;
        if let Some(c) = self.frame_cap {
            p.max_bytes = Some(c);
        }
        p
    }

    /// The ISDCF-style folder name.
    pub fn folder_name(&self) -> String {
        let clean: String = self.title.chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect::<String>().split('-').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("-");
        let kind = match self.kind.as_str() {
            "feature" => "FTR",
            "trailer" => "TLR",
            "short" => "SHR",
            "test" => "TST",
            "advertisement" => "ADV",
            "teaser" => "TSR",
            "psa" => "PSA",
            _ => "XSN",
        };
        let ratio = match self.shape {
            Shape::Flat => "F",
            Shape::Scope => "S",
            Shape::Full => "C",
        };
        let audio = match self.channels {
            1 => "10",
            2 => "20",
            6 => "51",
            8 => "71",
            _ => "XX",
        };
        let (y, m, d) = date_now();
        format!("{}_{}-1_{}_{}-XX_{}_{}_{}{:02}{:02}_{}_{}_OV", clean.chars().take(14).collect::<String>(), kind, ratio, self.language.to_uppercase(), audio, if self.four_k { "4K" } else { "2K" }, y, m, d, "STU", match self.standard { Standard::Smpte => "SMPTE", Standard::Interop => "IOP" })
    }
}

fn date_now() -> (u32, u32, u32) {
    let t = mxf_timestamp();
    (u16::from_be_bytes([t[0], t[1]]) as u32, t[2] as u32, t[3] as u32)
}

fn mxf_timestamp() -> [u8; 8] {
    // The MXF writer's clock, kept in one place.
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let mut t = [0u8; 8];
    t[..2].copy_from_slice(&(y as u16).to_be_bytes());
    t[2] = m as u8;
    t[3] = d as u8;
    t[4] = (rem / 3600) as u8;
    t[5] = (rem % 3600 / 60) as u8;
    t[6] = (rem % 60) as u8;
    t
}

pub fn iso_now() -> String {
    let t = mxf_timestamp();
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}+00:00", u16::from_be_bytes([t[0], t[1]]), t[2], t[3], t[4], t[5], t[6])
}

// ------------------------------------------------------------ colour

/// Display RGB (8-bit, Rec. 709 primaries, gamma 2.4) to 12-bit X'Y'Z'
/// (DCI white, gamma 2.6), placed at `(ox, oy)` in a `cw x ch` container
/// (black elsewhere).
pub fn rgba_to_xyz12(rgba: &[u8], w: usize, h: usize, cw: usize, ch: usize, ox: usize, oy: usize) -> [Vec<i32>; 3] {
    // Rec. 709 primaries -> XYZ (D65), adapted to the DCI white by a Bradford transform, scaled by 48/52.37.
    let m = rec709_to_dci_xyz();
    let dec: Vec<f32> = (0..256).map(|i| (i as f32 / 255.0).powf(2.4)).collect();
    const N: usize = 16384;
    let enc: Vec<i32> = (0..=N).map(|i| ((i as f32 / N as f32).powf(1.0 / 2.6) * 4095.0 + 0.5).min(4095.0) as i32).collect();
    let mut out = [vec![0i32; cw * ch], vec![0i32; cw * ch], vec![0i32; cw * ch]];
    for y in 0..h.min(ch.saturating_sub(oy)) {
        for x in 0..w.min(cw.saturating_sub(ox)) {
            let i = (y * w + x) * 4;
            let (r, g, b) = (dec[rgba[i] as usize], dec[rgba[i + 1] as usize], dec[rgba[i + 2] as usize]);
            let o = (y + oy) * cw + x + ox;
            for c in 0..3 {
                let v = (m[c][0] * r + m[c][1] * g + m[c][2] * b).clamp(0.0, 1.0);
                out[c][o] = enc[(v * N as f32 + 0.5) as usize];
            }
        }
    }
    out
}

/// X'Y'Z' (12-bit planes) back to display RGB, for tests.
pub fn xyz12_to_rgb(planes: &[Vec<i32>; 3], n: usize) -> Vec<[f32; 3]> {
    let m = rec709_to_dci_xyz();
    let inv = invert3(&m);
    (0..n)
        .map(|i| {
            let xyz: Vec<f32> = (0..3).map(|c| (planes[c][i] as f32 / 4095.0).powf(2.6)).collect();
            let mut rgb = [0f32; 3];
            for c in 0..3 {
                rgb[c] = (inv[c][0] * xyz[0] + inv[c][1] * xyz[1] + inv[c][2] * xyz[2]).clamp(0.0, 1.0).powf(1.0 / 2.4);
            }
            rgb
        })
        .collect()
}

fn invert3(m: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let d = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let mut r = [[0f32; 3]; 3];
    r[0][0] = (m[1][1] * m[2][2] - m[1][2] * m[2][1]) / d;
    r[0][1] = (m[0][2] * m[2][1] - m[0][1] * m[2][2]) / d;
    r[0][2] = (m[0][1] * m[1][2] - m[0][2] * m[1][1]) / d;
    r[1][0] = (m[1][2] * m[2][0] - m[1][0] * m[2][2]) / d;
    r[1][1] = (m[0][0] * m[2][2] - m[0][2] * m[2][0]) / d;
    r[1][2] = (m[0][2] * m[1][0] - m[0][0] * m[1][2]) / d;
    r[2][0] = (m[1][0] * m[2][1] - m[1][1] * m[2][0]) / d;
    r[2][1] = (m[0][1] * m[2][0] - m[0][0] * m[2][1]) / d;
    r[2][2] = (m[0][0] * m[1][1] - m[0][1] * m[1][0]) / d;
    r
}

fn mul3(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut r = [[0f64; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}

pub fn rec709_to_dci_xyz() -> [[f32; 3]; 3] {
    // Rec. 709 RGB -> XYZ (D65).
    let rgb2xyz = [[0.4124564, 0.3575761, 0.1804375], [0.2126729, 0.7151522, 0.0721750], [0.0193339, 0.119_192, 0.9503041]];
    // White points as XYZ with Y = 1.
    let d65 = [0.95047, 1.0, 1.08883];
    let dci = {
        let (x, y) = (0.314, 0.351);
        [x / y, 1.0, (1.0 - x - y) / y]
    };
    // Bradford cone response.
    let bfd = [[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]];
    let bfd_inv = {
        let f: [[f32; 3]; 3] = [[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]];
        let i = invert3(&f);
        [[i[0][0] as f64, i[0][1] as f64, i[0][2] as f64], [i[1][0] as f64, i[1][1] as f64, i[1][2] as f64], [i[2][0] as f64, i[2][1] as f64, i[2][2] as f64]]
    };
    let cone = |w: [f64; 3]| -> [f64; 3] { [0, 1, 2].map(|i| bfd[i][0] * w[0] + bfd[i][1] * w[1] + bfd[i][2] * w[2]) };
    let (cs, cd) = (cone(d65), cone(dci));
    let scale = [[cd[0] / cs[0], 0.0, 0.0], [0.0, cd[1] / cs[1], 0.0], [0.0, 0.0, cd[2] / cs[2]]];
    let adapt = mul3(&bfd_inv, &mul3(&scale, &bfd));
    let full = mul3(&adapt, &rgb2xyz);
    // The DCI coefficient: reference white (48 cd/m2) sits at 52.37 in the X'Y'Z' encoding.
    let k = 48.0 / 52.37;
    let mut out = [[0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = (full[i][j] * k) as f32;
        }
    }
    out
}

/// One picture (display RGBA of the active size) to a JPEG 2000 frame of the container.
pub fn encode_frame(spec: &DcpSpec, rgba: &[u8], w: usize, h: usize) -> Result<Vec<u8>, String> {
    let (cw, ch) = spec.container();
    let (cw, ch) = (cw as usize, ch as usize);
    if w > cw || h > ch {
        return Err(format!("the picture {w}x{h} is larger than the {cw}x{ch} container"));
    }
    let (ox, oy) = ((cw - w) / 2, (ch - h) / 2);
    let planes = rgba_to_xyz12(rgba, w, h, cw, ch, ox, oy);
    j2k::encode(&[&planes[0], &planes[1], &planes[2]], &spec.j2k_params())
}

// ----------------------------------------------------- frame packs

const PACK_MAGIC: &[u8; 8] = b"STUDJ2K1";

/// Codestreams one after another in a file, so render pieces can travel.
pub struct PackWriter {
    f: std::io::BufWriter<std::fs::File>,
    pub frames: u64,
}

impl PackWriter {
    pub fn create(path: &Path) -> std::io::Result<PackWriter> {
        let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
        f.write_all(PACK_MAGIC)?;
        Ok(PackWriter { f, frames: 0 })
    }

    pub fn push(&mut self, cs: &[u8]) -> std::io::Result<()> {
        self.f.write_all(&(cs.len() as u32).to_be_bytes())?;
        self.f.write_all(cs)?;
        self.frames += 1;
        Ok(())
    }

    pub fn finish(mut self) -> std::io::Result<u64> {
        self.f.flush()?;
        Ok(self.frames)
    }
}

pub fn read_pack(path: &Path, mut each: impl FnMut(Vec<u8>) -> Result<(), String>) -> Result<u64, String> {
    let mut f = std::io::BufReader::new(std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?);
    let mut magic = [0u8; 8];
    f.read_exact(&mut magic).map_err(|e| e.to_string())?;
    if &magic != PACK_MAGIC {
        return Err(format!("{} is not a frame pack", path.display()));
    }
    let mut n = 0;
    loop {
        let mut len = [0u8; 4];
        match f.read_exact(&mut len) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.to_string()),
        }
        let mut cs = vec![0u8; u32::from_be_bytes(len) as usize];
        f.read_exact(&mut cs).map_err(|e| format!("{}: truncated frame pack: {e}", path.display()))?;
        each(cs)?;
        n += 1;
    }
    Ok(n)
}

// ----------------------------------------------------------- sound

/// Interleaved sound at 48 kHz for the whole programme.
pub struct Audio {
    pub channels: u16,
    pub samples: Vec<f32>,
}

impl Audio {
    /// A stereo mix in the front pair of a `channels`-wide track (L, R, then silence).
    pub fn from_stereo(stereo: &[f32], channels: u16) -> Audio {
        let ch = channels as usize;
        let mut s = vec![0f32; stereo.len() / 2 * ch];
        for (i, f) in stereo.chunks_exact(2).enumerate() {
            s[i * ch] = f[0];
            if ch > 1 {
                s[i * ch + 1] = f[1];
            }
        }
        Audio { channels, samples: s }
    }
}

fn pcm24(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 3);
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32;
        out.extend_from_slice(&v.to_le_bytes()[..3]);
    }
    out
}

// ---------------------------------------------------------- the build

#[derive(Debug)]
pub struct DcpResult {
    pub dir: PathBuf,
    pub cpl: PathBuf,
    pub cpl_id: String,
    pub picture: PathBuf,
    pub sound: Option<PathBuf>,
    pub frames: u64,
    pub bytes: u64,
    pub max_frame_bytes: usize,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn urn(u: &[u8; 16]) -> String {
    format!("urn:uuid:{}", mxf::uuid_string(u))
}

/// Write the package into `parent/<folder name>` from frame packs (in
/// order) and the sound.
pub fn build(spec: &DcpSpec, parent: &Path, packs: &[PathBuf], audio: Option<&Audio>, progress: &mut dyn FnMut(f32), cancel: &std::sync::atomic::AtomicBool) -> Result<DcpResult, String> {
    spec.validate()?;
    let dir = parent.join(spec.folder_name());
    if dir.exists() {
        return Err(format!("{} already exists", dir.display()));
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let r = build_in(spec, &dir, packs, audio, progress, cancel);
    if r.is_err() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    r
}

fn build_in(spec: &DcpSpec, dir: &Path, packs: &[PathBuf], audio: Option<&Audio>, progress: &mut dyn FnMut(f32), cancel: &std::sync::atomic::AtomicBool) -> Result<DcpResult, String> {
    use std::sync::atomic::Ordering;
    let rate = (spec.fps, 1);
    let io = |e: std::io::Error| e.to_string();
    // Picture track file.
    let pic_path = dir.join(format!("jp2k_{}.mxf", &mxf::uuid_string(&mxf::uuid())[..8]));
    let mut writer: Option<MxfWriter> = None;
    let mut max_frame = 0usize;
    let mut frames = 0u64;
    let mut desc_err: Option<String> = None;
    let total_packs = packs.len().max(1);
    for (pi, p) in packs.iter().enumerate() {
        read_pack(p, |cs| {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            if writer.is_none() {
                let d = J2kDesc::from_codestream(&cs)?;
                if (d.width, d.height) != spec.container() {
                    desc_err = Some(format!("the frames are {}x{}, the container is {}x{}", d.width, d.height, spec.container().0, spec.container().1));
                    return Err(desc_err.clone().unwrap());
                }
                writer = Some(MxfWriter::create(&pic_path, Essence::Jpeg2000(d), rate, &spec.title).map_err(|e| e.to_string())?);
            }
            max_frame = max_frame.max(cs.len());
            writer.as_mut().unwrap().write_frame(&cs).map_err(|e| e.to_string())?;
            frames += 1;
            Ok(())
        })?;
        progress(0.9 * (pi + 1) as f32 / total_packs as f32);
    }
    let pic = writer.ok_or("no pictures to package")?.finish().map_err(io)?;

    // Sound track file.
    let mut snd: Option<(PathBuf, MxfInfo)> = None;
    if let Some(a) = audio {
        let path = dir.join(format!("pcm_{}.mxf", &mxf::uuid_string(&mxf::uuid())[..8]));
        let per = 48_000 / spec.fps as usize;
        let ch = a.channels as usize;
        let mut w = MxfWriter::create(&path, Essence::Pcm { rate: 48_000, channels: a.channels, bits: 24 }, rate, &spec.title).map_err(io)?;
        for f in 0..frames as usize {
            let lo = (f * per * ch).min(a.samples.len());
            let hi = ((f + 1) * per * ch).min(a.samples.len());
            let mut chunk = a.samples[lo..hi].to_vec();
            chunk.resize(per * ch, 0.0);
            w.write_frame(&pcm24(&chunk)).map_err(io)?;
        }
        let info = w.finish().map_err(io)?;
        snd = Some((path, info));
    }
    progress(0.95);

    // CPL.
    let (cpl_id, pkl_id, am_id, reel_id, ver_id) = (mxf::uuid(), mxf::uuid(), mxf::uuid(), mxf::uuid(), mxf::uuid());
    let smpte = spec.standard == Standard::Smpte;
    let now = iso_now();
    let (aw, ah) = spec.active();
    let aspect = if smpte {
        format!("{aw} {ah}")
    } else {
        format!("{:.2}", aw as f64 / ah as f64)
    };
    let kind = esc(&spec.kind);
    let ns_cpl = if smpte { "http://www.smpte-ra.org/schemas/429-7/2006/CPL" } else { "http://www.digicine.com/PROTO-ASDCP-CPL-20040511#" };
    let mut cpl = String::new();
    cpl.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n");
    cpl.push_str(&format!("<CompositionPlaylist xmlns=\"{ns_cpl}\" xmlns:dsig=\"http://www.w3.org/2000/09/xmldsig#\">\n"));
    cpl.push_str(&format!("  <Id>{}</Id>\n  <AnnotationText>{}</AnnotationText>\n  <IssueDate>{now}</IssueDate>\n  <Issuer>{}</Issuer>\n  <Creator>{}</Creator>\n  <ContentTitleText>{}</ContentTitleText>\n  <ContentKind>{kind}</ContentKind>\n", urn(&cpl_id), esc(&spec.title), esc(&spec.issuer), esc(&spec.creator), esc(&spec.title)));
    if smpte {
        cpl.push_str(&format!("  <ContentVersion>\n    <Id>{}</Id>\n    <LabelText>{}</LabelText>\n  </ContentVersion>\n", urn(&ver_id), esc(&format!("{}-{}", spec.title, now))));
    }
    cpl.push_str("  <RatingList/>\n  <ReelList>\n    <Reel>\n");
    cpl.push_str(&format!("      <Id>{}</Id>\n      <AssetList>\n", urn(&reel_id)));
    cpl.push_str(&format!(
        "        <MainPicture>\n          <Id>{}</Id>\n          <EditRate>{} 1</EditRate>\n          <IntrinsicDuration>{}</IntrinsicDuration>\n          <EntryPoint>0</EntryPoint>\n          <Duration>{}</Duration>\n          <FrameRate>{} 1</FrameRate>\n          <ScreenAspectRatio>{aspect}</ScreenAspectRatio>\n        </MainPicture>\n",
        urn(&pic.id),
        spec.fps,
        frames,
        frames,
        spec.fps
    ));
    if let Some((_, info)) = &snd {
        cpl.push_str(&format!(
            "        <MainSound>\n          <Id>{}</Id>\n          <EditRate>{} 1</EditRate>\n          <IntrinsicDuration>{}</IntrinsicDuration>\n          <EntryPoint>0</EntryPoint>\n          <Duration>{}</Duration>\n{}        </MainSound>\n",
            urn(&info.id),
            spec.fps,
            frames,
            frames,
            if smpte { format!("          <Language>{}</Language>\n", esc(&spec.language)) } else { String::new() }
        ));
    }
    cpl.push_str("      </AssetList>\n    </Reel>\n  </ReelList>\n</CompositionPlaylist>\n");
    let cpl_name = format!("CPL_{}.xml", mxf::uuid_string(&cpl_id));
    std::fs::write(dir.join(&cpl_name), &cpl).map_err(io)?;

    // PKL.
    let ns_pkl = if smpte { "http://www.smpte-ra.org/schemas/429-8/2007/PKL" } else { "http://www.digicine.com/PROTO-ASDCP-PKL-20040311#" };
    let mut assets: Vec<(String, [u8; 16], String, &str)> = Vec::new(); // (file name, id, annotation, type)
    assets.push((pic_path.file_name().unwrap().to_string_lossy().into_owned(), pic.id, format!("{} picture", spec.title), "application/mxf"));
    if let Some((p, info)) = &snd {
        assets.push((p.file_name().unwrap().to_string_lossy().into_owned(), info.id, format!("{} sound", spec.title), "application/mxf"));
    }
    assets.push((cpl_name.clone(), cpl_id, format!("{} composition", spec.title), if smpte { "text/xml" } else { "text/xml;asdcpKind=CPL" }));
    let mut pkl = format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n<PackingList xmlns=\"{ns_pkl}\" xmlns:dsig=\"http://www.w3.org/2000/09/xmldsig#\">\n  <Id>{}</Id>\n  <AnnotationText>{}</AnnotationText>\n  <IssueDate>{now}</IssueDate>\n  <Issuer>{}</Issuer>\n  <Creator>{}</Creator>\n  <AssetList>\n", urn(&pkl_id), esc(&spec.title), esc(&spec.issuer), esc(&spec.creator));
    let mut sizes: Vec<(String, u64)> = Vec::new();
    for (name, id, note, ty) in &assets {
        let (hash, size) = sha1::file_hash(&dir.join(name)).map_err(io)?;
        sizes.push((name.clone(), size));
        pkl.push_str(&format!("    <Asset>\n      <Id>{}</Id>\n      <AnnotationText>{}</AnnotationText>\n      <Hash>{hash}</Hash>\n      <Size>{size}</Size>\n      <Type>{ty}</Type>\n    </Asset>\n", urn(id), esc(note)));
    }
    pkl.push_str("  </AssetList>\n</PackingList>\n");
    let pkl_name = format!("PKL_{}.xml", mxf::uuid_string(&pkl_id));
    std::fs::write(dir.join(&pkl_name), &pkl).map_err(io)?;

    // ASSETMAP and VOLINDEX.
    let ns_am = if smpte { "http://www.smpte-ra.org/schemas/429-9/2007/AM" } else { "http://www.digicine.com/PROTO-ASDCP-AM-20040311#" };
    let mut am = format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n<AssetMap xmlns=\"{ns_am}\">\n  <Id>{}</Id>\n  <Creator>{}</Creator>\n  <VolumeCount>1</VolumeCount>\n  <IssueDate>{now}</IssueDate>\n  <Issuer>{}</Issuer>\n  <AssetList>\n", urn(&am_id), esc(&spec.creator), esc(&spec.issuer));
    let pkl_len = std::fs::metadata(dir.join(&pkl_name)).map_err(io)?.len();
    am.push_str(&format!("    <Asset>\n      <Id>{}</Id>\n      <PackingList>true</PackingList>\n      <ChunkList>\n        <Chunk>\n          <Path>{pkl_name}</Path>\n          <VolumeIndex>1</VolumeIndex>\n          <Offset>0</Offset>\n          <Length>{pkl_len}</Length>\n        </Chunk>\n      </ChunkList>\n    </Asset>\n", urn(&pkl_id)));
    for ((name, id, _, _), (_, size)) in assets.iter().zip(&sizes) {
        am.push_str(&format!("    <Asset>\n      <Id>{}</Id>\n      <ChunkList>\n        <Chunk>\n          <Path>{name}</Path>\n          <VolumeIndex>1</VolumeIndex>\n          <Offset>0</Offset>\n          <Length>{size}</Length>\n        </Chunk>\n      </ChunkList>\n    </Asset>\n", urn(id)));
    }
    am.push_str("  </AssetList>\n</AssetMap>\n");
    let (am_name, vi_name) = if smpte { ("ASSETMAP.xml", "VOLINDEX.xml") } else { ("ASSETMAP", "VOLINDEX") };
    std::fs::write(dir.join(am_name), &am).map_err(io)?;
    let vi = format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n<VolumeIndex xmlns=\"{ns_am}\">\n  <Index>1</Index>\n</VolumeIndex>\n");
    std::fs::write(dir.join(vi_name), vi).map_err(io)?;

    progress(1.0);
    let bytes: u64 = sizes.iter().map(|s| s.1).sum::<u64>() + pkl_len + am.len() as u64;
    Ok(DcpResult { dir: dir.to_path_buf(), cpl: dir.join(&cpl_name), cpl_id: mxf::uuid_string(&cpl_id), picture: pic_path, sound: snd.map(|s| s.0), frames, bytes, max_frame_bytes: max_frame })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn colour_maps_neutrals_to_neutrals_and_round_trips() {
        let (w, h) = (4usize, 1usize);
        let rgba: Vec<u8> = vec![0, 0, 0, 255, 255, 255, 255, 255, 128, 128, 128, 255, 200, 40, 60, 255];
        let xyz = rgba_to_xyz12(&rgba, w, h, 4, 1, 0, 0);
        assert_eq!((xyz[0][0], xyz[1][0], xyz[2][0]), (0, 0, 0));
        // White is the DCI white at the reference level: Y' = (48/52.37)^(1/2.6) * 4095 ~ 3794; X' and Z' follow the DCI chromaticity.
        let y_white = ((48.0f64 / 52.37).powf(1.0 / 2.6) * 4095.0).round() as i32;
        assert!((xyz[1][1] - y_white).abs() <= 2, "{} vs {y_white}", xyz[1][1]);
        let back = xyz12_to_rgb(&xyz, 4);
        for (i, px) in [[0, 0, 0], [255, 255, 255], [128, 128, 128], [200, 40, 60]].iter().enumerate() {
            for c in 0..3 {
                let v = (back[i][c] * 255.0).round() as i32;
                assert!((v - px[c]).abs() <= 2, "pixel {i} channel {c}: {v} vs {}", px[c]);
            }
        }
    }

    #[test]
    fn containers_names_and_limits() {
        let mut s = DcpSpec::new("My Film: The Cut!", 24);
        assert_eq!((s.container(), s.active()), ((2048, 1080), (1998, 1080)));
        s.shape = Shape::Scope;
        s.four_k = true;
        assert_eq!((s.container(), s.active()), ((4096, 2160), (4096, 1716)));
        let n = s.folder_name();
        assert!(n.starts_with("My-Film-The-Cu_FTR-1_S_EN-XX_51_4K_") && n.ends_with("_STU_SMPTE_OV"), "{n}");
        assert!(DcpSpec::new("x", 29).validate().is_err());
        let mut s = DcpSpec::new("x", 48);
        s.four_k = true;
        assert!(s.validate().is_err());
        assert_eq!(j2k::dci_frame_cap(24.0), 1_302_083);
    }

    #[test]
    fn frame_packs_round_trip() {
        let d = std::env::temp_dir().join(format!("studio-pack-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("a.pack");
        let mut w = PackWriter::create(&p).unwrap();
        for i in 0..4u8 {
            w.push(&vec![i; 100 + i as usize]).unwrap();
        }
        assert_eq!(w.finish().unwrap(), 4);
        let mut got = Vec::new();
        read_pack(&p, |cs| {
            got.push(cs);
            Ok(())
        })
        .unwrap();
        assert_eq!(got.len(), 4);
        assert_eq!(got[3], vec![3u8; 103]);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A 2K DCP of moving colour bars with a tone, built through the whole path.
    fn bars(w: usize, h: usize, f: usize) -> Vec<u8> {
        let cols = [[235u8, 235, 235], [235, 235, 16], [16, 235, 235], [16, 235, 16], [235, 16, 235], [235, 16, 16], [16, 16, 235], [16, 16, 16]];
        let mut px = vec![255u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let c = cols[((x + f * 24) * 8 / w) % 8];
                let i = (y * w + x) * 4;
                px[i] = c[0];
                px[i + 1] = c[1];
                px[i + 2] = c[2];
            }
        }
        px
    }

    /// `cargo test -p studio-deliver perf_2k -- --ignored --nocapture`: how long one detailed 2K frame takes.
    #[test]
    #[ignore]
    fn perf_2k_frame() {
        let (w, h) = (1998usize, 1080usize);
        let mut seed = 7u32;
        let mut px = vec![255u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let grain = ((seed >> 24) as i32 % 24) - 12;
                let base = 90 + ((x as f32 * 0.01).sin() * 50.0 + (y as f32 * 0.013).cos() * 40.0) as i32;
                let edge = if ((x / 90) + (y / 70)) % 3 == 0 { 60 } else { 0 };
                let i = (y * w + x) * 4;
                for c in 0..3 {
                    px[i + c] = (base + edge + grain + c as i32 * 12).clamp(0, 255) as u8;
                }
            }
        }
        let spec = DcpSpec::new("Perf", 24);
        let t = std::time::Instant::now();
        let cs = encode_frame(&spec, &px, w, h).unwrap();
        eprintln!("2K frame: {} bytes in {:.2}s ({} threads)", cs.len(), t.elapsed().as_secs_f64(), std::thread::available_parallelism().map_or(1, |n| n.get()));
    }

    #[test]
    fn a_whole_smpte_dcp_is_written_with_valid_structure_and_decodes() {
        let dir = std::env::temp_dir().join(format!("studio-dcp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut spec = DcpSpec::new("Bars Test", 24);
        spec.channels = 6;
        let (aw, ah) = spec.active();
        // Three frames at the real 2K flat size: the encoder is the part under test.
        let pack = dir.join("a.pack");
        let mut pw = PackWriter::create(&pack).unwrap();
        let t0 = std::time::Instant::now();
        let mut max = 0;
        for f in 0..3 {
            let cs = encode_frame(&spec, &bars(aw as usize, ah as usize, f), aw as usize, ah as usize).unwrap();
            max = max.max(cs.len());
            pw.push(&cs).unwrap();
        }
        pw.finish().unwrap();
        eprintln!("3 frames of 2K J2K in {:.1}s; largest frame {} bytes (cap {})", t0.elapsed().as_secs_f64(), max, j2k::dci_frame_cap(24.0));
        assert!(max <= j2k::dci_frame_cap(24.0));
        let stereo: Vec<f32> = (0..48_000 / 8 * 2).map(|i| 0.3 * ((i / 2) as f32 * 0.131).sin()).collect();
        let audio = Audio::from_stereo(&stereo, 6);
        let r = build(&spec, &dir, &[pack], Some(&audio), &mut |_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(r.frames, 3);
        // Files and structure.
        for f in ["ASSETMAP.xml", "VOLINDEX.xml"] {
            assert!(r.dir.join(f).exists(), "{f}");
        }
        let cpl = std::fs::read_to_string(&r.cpl).unwrap();
        assert!(cpl.contains("<ScreenAspectRatio>1998 1080</ScreenAspectRatio>") && cpl.contains("<Duration>3</Duration>") && cpl.contains("<MainSound>"));
        let pkl_name = std::fs::read_dir(&r.dir).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).find(|n| n.starts_with("PKL_")).unwrap();
        let pkl = std::fs::read_to_string(r.dir.join(pkl_name)).unwrap();
        // Every PKL hash and size matches the file; every CPL asset id is in the PKL.
        let mut checked = 0;
        for chunk in pkl.split("<Asset>").skip(1) {
            let tag = |t: &str| chunk.split(&format!("<{t}>")).nth(1).and_then(|s| s.split(&format!("</{t}>")).next()).unwrap().to_string();
            let id = tag("Id").replace("urn:uuid:", "");
            let ty = tag("Type");
            let name = std::fs::read_dir(&r.dir).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).find(|n| if ty == "text/xml" { n.contains(&id) } else { n.ends_with(".mxf") && std::fs::metadata(r.dir.join(n)).map(|m| m.len().to_string() == tag("Size")).unwrap_or(false) }).unwrap();
            let (hash, size) = crate::sha1::file_hash(&r.dir.join(&name)).unwrap();
            assert_eq!((hash, size.to_string()), (tag("Hash"), tag("Size")), "{name}");
            checked += 1;
        }
        assert_eq!(checked, 3);
        let am = std::fs::read_to_string(r.dir.join("ASSETMAP.xml")).unwrap();
        assert!(am.contains("<PackingList>true</PackingList>") && am.matches("<Chunk>").count() == 4);
        // Interop writes the same package with the older namespaces and extension-less maps.
        let mut iop = spec.clone();
        iop.standard = Standard::Interop;
        let pack2 = dir.join("a.pack");
        let r2 = build(&iop, &dir, &[pack2], None, &mut |_| {}, &AtomicBool::new(false)).unwrap();
        assert!(r2.dir.join("ASSETMAP").exists() && std::fs::read_to_string(&r2.cpl).unwrap().contains("PROTO-ASDCP-CPL-20040511"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
