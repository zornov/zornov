// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Automated quality control of a finished picture and sound.
//!
//! Feed decoded pictures to [`VideoQc`] and sound to [`AudioQc`], add the
//! facts about the file ([`Facts`]), and [`report`] says whether the
//! deliverable passes its [`Profile`], with times and a fix for every
//! finding.
//!
//! Checks:
//! - **Levels**: luma outside 16-235 and chroma outside 16-240 (picture
//!   given as YCbCr), and RGB excursions beyond -5 % / +105 % after
//!   conversion (EBU R 103), as a share of each frame's area.
//! - **Flash and pattern** (photosensitive epilepsy): after ITU-R BT.1702 /
//!   Ofcom (Harding): general and red flashes, more than three in any one
//!   second, over at least a quarter of the screen. The spatial test is
//!   approximate (a 64 x 36 grid): it flags regular stripes of five pairs
//!   or more, and cannot see stripes finer than 1/64 of the width. It is a
//!   screening aid, not a certified Harding FPA.
//! - **Black and frozen** frames.
//! - **Bars and aspect**: letterbox / pillarbox found in the picture,
//!   and a picture size that is not the one asked for.
//! - **Sound**: loudness (BS.1770-4 via `studio-sound`), true peak,
//!   sample clipping, dropouts to digital silence inside the programme,
//!   a dead or unbalanced channel, inverted phase, DC offset.
//! - **File**: no sound track when one is required, picture and sound of
//!   different lengths, missing captions, over-long programmes.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Fail,
}

/// One problem (or note) with where it is and how to fix it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub check: String,
    pub severity: Severity,
    /// Seconds into the programme; `end == start` for a point.
    pub start: f64,
    pub end: f64,
    pub message: String,
    pub fix: String,
}

fn finding(check: &str, severity: Severity, start: f64, end: f64, message: String, fix: &str) -> Finding {
    Finding { check: check.into(), severity, start, end, message, fix: fix.into() }
}

/// What a deliverable must satisfy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    /// Share of a frame's area that may be out of legal levels.
    pub max_illegal_share: f64,
    /// Out-of-level frames are Fail (broadcast) or Warning.
    pub strict_levels: bool,
    pub black_secs: f64,
    pub frozen_secs: f64,
    /// Flashes allowed in any one second (3 for Ofcom / ITU).
    pub flash_limit: u32,
    /// Photosensitivity is Fail (broadcast, streaming) or Warning.
    pub strict_flash: bool,
    /// Integrated loudness target in LUFS, tolerance in LU, true-peak ceiling in dBTP.
    pub loudness: Option<(f64, f64, f64)>,
    /// Loudness is measured on dialogue only.
    pub dialogue_gated: bool,
    pub expect_audio: bool,
    pub expect_size: Option<(u32, u32)>,
    pub expect_fps: Option<f64>,
    pub max_duration: Option<f64>,
    pub captions_required: bool,
}

impl Default for Profile {
    fn default() -> Self {
        Profile {
            name: "general".into(),
            max_illegal_share: 0.01,
            strict_levels: false,
            black_secs: 0.5,
            frozen_secs: 3.0,
            flash_limit: 3,
            strict_flash: true,
            loudness: None,
            dialogue_gated: false,
            expect_audio: false,
            expect_size: None,
            expect_fps: None,
            max_duration: None,
            captions_required: false,
        }
    }
}

/// A decoded picture.
pub enum Picture<'a> {
    /// 4:2:0 / 4:2:2 / 4:4:4 planes as decoded (BT.709 matrix), limited
    /// range unless `full_range`.
    Planes { w: usize, h: usize, y: &'a [u8], u: &'a [u8], v: &'a [u8], cw: usize, ch: usize, full_range: bool },
    /// Display RGBA, 8 bits, row-major.
    Rgba { w: usize, h: usize, px: &'a [u8] },
}

const GW: usize = 64;
const GH: usize = 36;

#[derive(Clone, Default)]
struct FrameStat {
    illegal_share: f32,
    luma_under: f32,
    luma_over: f32,
    chroma_out: f32,
    mean_yn: f32,
    max_yn: f32,
    bars_top: f32,
    bars_bottom: f32,
    bars_left: f32,
    bars_right: f32,
}

/// Streaming picture checks.
pub struct VideoQc {
    fps: f64,
    stats: Vec<FrameStat>,
    size: (usize, usize),
    /// Previous frame's linear luminance grid (64 x 36) and red metric.
    prev_lum: Option<Vec<f32>>,
    prev_red: Option<Vec<f32>>,
    prev_sig: Option<Vec<f32>>,
    frozen: Vec<bool>,
    pattern: Vec<bool>,
    /// (frame, sign) of every general-flash transition, and red.
    flash_events: Vec<(u64, i8)>,
    red_events: Vec<(u64, i8)>,
}

fn lut_gamma() -> &'static [f32; 256] {
    static L: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    L.get_or_init(|| {
        let mut t = [0f32; 256];
        for (i, v) in t.iter_mut().enumerate() {
            *v = (i as f32 / 255.0).powf(2.4);
        }
        t
    })
}

impl VideoQc {
    pub fn new(fps: f64) -> Self {
        VideoQc { fps: fps.max(1.0), stats: Vec::new(), size: (0, 0), prev_lum: None, prev_red: None, prev_sig: None, frozen: Vec::new(), pattern: Vec::new(), flash_events: Vec::new(), red_events: Vec::new() }
    }

    pub fn frames(&self) -> u64 {
        self.stats.len() as u64
    }

    pub fn size(&self) -> (usize, usize) {
        self.size
    }

    /// Analyse the next picture.
    pub fn push(&mut self, pic: &Picture) {
        let (w, h) = match pic {
            Picture::Planes { w, h, .. } | Picture::Rgba { w, h, .. } => (*w, *h),
        };
        if w == 0 || h == 0 {
            return;
        }
        self.size = (w, h);
        let step = (w.max(h) / 960).max(1);
        let gamma = lut_gamma();
        let mut lum = vec![0f32; GW * GH];
        let mut red = vec![0f32; GW * GH];
        let mut cnt = vec![0f32; GW * GH];
        let (mut n, mut under, mut over, mut cout, mut illegal) = (0u32, 0u32, 0u32, 0u32, 0u32);
        let (mut sum_yn, mut max_yn) = (0f64, 0f32);
        // Rows / columns that are all black, for bars.
        let mut row_black = vec![true; h.div_ceil(step)];
        let mut col_black = vec![true; w.div_ceil(step)];
        let mut y = 0;
        while y < h {
            let mut x = 0;
            while x < w {
                let (yn, r, g, b);
                match pic {
                    Picture::Planes { y: yp, u, v, cw, ch, full_range, .. } => {
                        let yc = yp[y * w + x] as f32;
                        let cx = (x * cw / w).min(cw - 1);
                        let cy = (y * ch / h).min(ch - 1);
                        let cb = u[cy * cw + cx] as f32;
                        let cr = v[cy * cw + cx] as f32;
                        let (ys, cs) = if *full_range { (1.0, 1.0) } else { (255.0 / 219.0, 255.0 / 224.0) };
                        let yy = if *full_range { yc } else { (yc - 16.0) * ys };
                        let pb = (cb - 128.0) * cs;
                        let pr = (cr - 128.0) * cs;
                        r = yy + 1.5748 * pr;
                        g = yy - 0.1873 * pb - 0.4681 * pr;
                        b = yy + 1.8556 * pb;
                        yn = (yy / 255.0).clamp(0.0, 1.0);
                        if !full_range {
                            if yc < 16.0 {
                                under += 1;
                            }
                            if yc > 235.0 {
                                over += 1;
                            }
                            if cb < 16.0 || cb > 240.0 || cr < 16.0 || cr > 240.0 {
                                cout += 1;
                            }
                        }
                        // EBU R 103: -5 % .. 105 % of the RGB range.
                        if r < -12.75 || g < -12.75 || b < -12.75 || r > 267.75 || g > 267.75 || b > 267.75 {
                            illegal += 1;
                        }
                    }
                    Picture::Rgba { px, .. } => {
                        let i = (y * w + x) * 4;
                        r = px[i] as f32;
                        g = px[i + 1] as f32;
                        b = px[i + 2] as f32;
                        yn = (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0;
                        // A colour channel stuck at an end while another is not:
                        // a clipped, out-of-gamut colour.
                        let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
                        if (mx >= 255.0 && mn < 200.0) || (mn <= 0.0 && mx > 60.0) {
                            illegal += 1;
                        }
                    }
                }
                n += 1;
                sum_yn += yn as f64;
                max_yn = max_yn.max(yn);
                if yn > 0.08 {
                    row_black[y / step] = false;
                    col_black[x / step] = false;
                }
                let (rc, gc, bc) = (r.clamp(0.0, 255.0) as usize, g.clamp(0.0, 255.0) as usize, b.clamp(0.0, 255.0) as usize);
                let (rl, gl, bl) = (gamma[rc.min(255)], gamma[gc.min(255)], gamma[bc.min(255)]);
                let l = 0.2126 * rl + 0.7152 * gl + 0.0722 * bl;
                let cell = (y * GH / h) * GW + (x * GW / w);
                lum[cell] += l;
                // ITU-R BT.1702 red metric: saturated red only.
                let sum = rl + gl + bl;
                if sum > 1e-6 && rl / sum >= 0.8 {
                    red[cell] += (rl - gl - bl) * 320.0;
                }
                cnt[cell] += 1.0;
                x += step;
            }
            y += step;
        }
        for i in 0..GW * GH {
            if cnt[i] > 0.0 {
                lum[i] /= cnt[i];
                red[i] /= cnt[i];
            }
        }
        let nf = n.max(1) as f32;
        let bars = |flags: &[bool], from_start: bool| {
            let it: Box<dyn Iterator<Item = &bool>> = if from_start { Box::new(flags.iter()) } else { Box::new(flags.iter().rev()) };
            let k = it.take_while(|b| **b).count();
            // Every row black is "no picture", not a bar.
            if k == flags.len() { 0.0 } else { k as f32 / flags.len() as f32 }
        };
        let st = FrameStat {
            illegal_share: illegal as f32 / nf,
            luma_under: under as f32 / nf,
            luma_over: over as f32 / nf,
            chroma_out: cout as f32 / nf,
            mean_yn: (sum_yn / n.max(1) as f64) as f32,
            max_yn,
            bars_top: bars(&row_black, true),
            bars_bottom: bars(&row_black, false),
            bars_left: bars(&col_black, true),
            bars_right: bars(&col_black, false),
        };
        let idx = self.stats.len() as u64;
        self.stats.push(st);

        // Frozen: the 32 x 18 signature equals the last frame's.
        let sig: Vec<f32> = (0..18)
            .flat_map(|gy| {
                let lum = &lum;
                (0..32).map(move |gx| {
                    let a = [(2 * gy) * GW + 2 * gx, (2 * gy) * GW + 2 * gx + 1, (2 * gy + 1) * GW + 2 * gx, (2 * gy + 1) * GW + 2 * gx + 1];
                    a.iter().map(|&i| lum[i]).sum::<f32>() / 4.0
                })
            })
            .collect();
        let same = self.prev_sig.as_ref().is_some_and(|p| p.iter().zip(&sig).map(|(a, b)| (a - b).abs()).sum::<f32>() / sig.len() as f32 <= 0.0004);
        self.frozen.push(same);

        // Flash: 16 x 9 cells, 4 x 4 grid cells each.
        if let (Some(pl), Some(pr)) = (&self.prev_lum, &self.prev_red) {
            let (mut up, mut down, mut rup, mut rdown) = (0u32, 0u32, 0u32, 0u32);
            for cy in 0..9 {
                for cx in 0..16 {
                    let (mut a, mut b, mut ra, mut rb) = (0f32, 0f32, 0f32, 0f32);
                    for dy in 0..4 {
                        for dx in 0..4 {
                            let i = (cy * 4 + dy) * GW + cx * 4 + dx;
                            a += pl[i];
                            b += lum[i];
                            ra += pr[i];
                            rb += red[i];
                        }
                    }
                    let (a, b, ra, rb) = (a / 16.0, b / 16.0, ra / 16.0, rb / 16.0);
                    let d = b - a;
                    if d.abs() >= 0.1 && a.min(b) < 0.8 {
                        if d > 0.0 {
                            up += 1;
                        } else {
                            down += 1;
                        }
                    }
                    let dr = rb - ra;
                    if dr.abs() >= 20.0 {
                        if dr > 0.0 {
                            rup += 1;
                        } else {
                            rdown += 1;
                        }
                    }
                }
            }
            let quarter = 144 / 4;
            if up >= quarter || down >= quarter {
                self.flash_events.push((idx, if up >= down { 1 } else { -1 }));
            }
            if rup >= quarter || rdown >= quarter {
                self.red_events.push((idx, if rup >= rdown { 1 } else { -1 }));
            }
        }

        // Pattern: regular stripes (five light-dark pairs) over a quarter of the rows or columns.
        let alt = |get: &dyn Fn(usize) -> f32, n: usize| -> bool {
            let (mut run, mut best, mut last) = (0u32, 0u32, 0i8);
            for i in 1..n {
                let d = get(i) - get(i - 1);
                let s = if d >= 0.1 { 1 } else if d <= -0.1 { -1 } else { 0 };
                if s != 0 && s == -last {
                    run += 1;
                } else if s != 0 {
                    run = 1;
                } else {
                    run = 0;
                }
                if s != 0 {
                    last = s;
                } else {
                    last = 0;
                }
                best = best.max(run);
            }
            best >= 10
        };
        let rows = (0..GH).filter(|&gy| alt(&|i| lum[gy * GW + i], GW)).count();
        let cols = (0..GW).filter(|&gx| alt(&|i| lum[i * GW + gx], GH)).count();
        self.pattern.push(rows * 4 >= GH || cols * 4 >= GW);

        self.prev_lum = Some(lum);
        self.prev_red = Some(red);
        self.prev_sig = Some(sig);
    }
}

/// Runs of at least `min_len` consecutive true flags as `(first, last+1)`.
fn runs(flags: &[bool], min_len: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < flags.len() {
        if flags[i] {
            let s = i;
            while i < flags.len() && flags[i] {
                i += 1;
            }
            if i - s >= min_len {
                out.push((s, i));
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Flash events whose alternating pairs exceed `limit` in some one-second window.
fn flash_windows(events: &[(u64, i8)], fps: f64, limit: u32) -> Vec<(u64, u64, u32)> {
    // A flash is a transition opposite to the one before it.
    let mut flashes: Vec<u64> = Vec::new();
    for w in events.windows(2) {
        if w[0].1 != w[1].1 {
            flashes.push(w[1].0);
        }
    }
    let span = fps.round().max(1.0) as u64;
    let mut out: Vec<(u64, u64, u32)> = Vec::new();
    let mut i = 0;
    while i < flashes.len() {
        // flashes[i..j] inside [flashes[i], flashes[i] + 1 s)
        let j = flashes.iter().skip(i).take_while(|&&f| f < flashes[i] + span).count() + i;
        let count = (j - i) as u32;
        if count > limit {
            let (s, e) = (flashes[i], flashes[j - 1]);
            match out.last_mut() {
                Some(last) if s <= last.1 + span => {
                    last.1 = e;
                    last.2 = last.2.max(count);
                }
                _ => out.push((s, e, count)),
            }
        }
        i += 1;
    }
    out
}

/// A streaming sound meter: loudness and faults of a stereo programme.
pub struct AudioQc {
    rate: u32,
    meter: studio_sound::loudness::LoudnessMeter,
    /// Dialogue-only meter, fed inside `speech` spans (seconds).
    speech_meter: Option<studio_sound::loudness::LoudnessMeter>,
    speech: Vec<(f64, f64)>,
    pos: u64,
    peak: f32,
    clip_run: u32,
    clips: Vec<f64>,
    /// Run of digital silence in progress: (start frame).
    silent_from: Option<u64>,
    silences: Vec<(u64, u64)>,
    sum_l2: f64,
    sum_r2: f64,
    sum_lr: f64,
    sum_l: f64,
    sum_r: f64,
    /// Frames with signal above -60 dBFS in a 100 ms block, per block.
    blocks: Vec<bool>,
    block_peak: f32,
    block_len: u64,
}

impl AudioQc {
    pub fn new(rate: u32) -> Self {
        AudioQc {
            rate,
            meter: studio_sound::loudness::LoudnessMeter::new(rate),
            speech_meter: None,
            speech: Vec::new(),
            pos: 0,
            peak: 0.0,
            clip_run: 0,
            clips: Vec::new(),
            silent_from: None,
            silences: Vec::new(),
            sum_l2: 0.0,
            sum_r2: 0.0,
            sum_lr: 0.0,
            sum_l: 0.0,
            sum_r: 0.0,
            blocks: Vec::new(),
            block_peak: 0.0,
            block_len: (rate as u64 / 10).max(1),
        }
    }

    /// Measure loudness on these spans (seconds) only: dialogue-gated
    /// targets such as Netflix.
    pub fn with_speech(mut self, spans: Vec<(f64, f64)>) -> Self {
        self.speech_meter = Some(studio_sound::loudness::LoudnessMeter::new(self.rate));
        self.speech = spans;
        self
    }

    pub fn seconds(&self) -> f64 {
        self.pos as f64 / self.rate as f64
    }

    /// Feed the next block (equal lengths).
    pub fn push(&mut self, left: &[f32], right: &[f32]) {
        let n = left.len().min(right.len());
        self.meter.process(&left[..n], &right[..n]);
        if let Some(m) = &mut self.speech_meter {
            // The parts of this block inside speech spans, in order.
            let t0 = self.pos as f64 / self.rate as f64;
            let t1 = (self.pos + n as u64) as f64 / self.rate as f64;
            for &(a, b) in &self.speech {
                let (a, b) = (a.max(t0), b.min(t1));
                if b > a {
                    let i0 = ((a - t0) * self.rate as f64) as usize;
                    let i1 = (((b - t0) * self.rate as f64) as usize).min(n);
                    if i1 > i0 {
                        m.process(&left[i0..i1], &right[i0..i1]);
                    }
                }
            }
        }
        for i in 0..n {
            let (l, r) = (left[i], right[i]);
            let a = l.abs().max(r.abs());
            self.peak = self.peak.max(a);
            self.block_peak = self.block_peak.max(a);
            if a >= 0.9995 {
                self.clip_run += 1;
                if self.clip_run == 3 {
                    self.clips.push((self.pos + i as u64) as f64 / self.rate as f64);
                }
            } else {
                self.clip_run = 0;
            }
            let f = self.pos + i as u64;
            if a < 1e-6 {
                self.silent_from.get_or_insert(f);
            } else if let Some(s) = self.silent_from.take() {
                self.silences.push((s, f));
            }
            self.sum_l2 += (l * l) as f64;
            self.sum_r2 += (r * r) as f64;
            self.sum_lr += (l * r) as f64;
            self.sum_l += l as f64;
            self.sum_r += r as f64;
            if (f + 1) % self.block_len == 0 {
                self.blocks.push(self.block_peak > 0.001);
                self.block_peak = 0.0;
            }
        }
        self.pos += n as u64;
    }
}

/// What the container says about the file.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Facts {
    pub file: String,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub video_secs: f64,
    pub audio_secs: Option<f64>,
    pub video_codec: String,
    /// Captions: number of cues the programme has (None when unknown).
    pub caption_cues: Option<usize>,
    /// Seconds of speech in the programme (None when unknown).
    pub speech_secs: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub file: String,
    pub profile: String,
    pub verdict: String,
    pub passed: bool,
    pub fails: usize,
    pub warnings: usize,
    pub measurements: Value,
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// For people: a verdict line, then one line per finding.
    pub fn to_text(&self) -> String {
        let mut s = format!("QC {} for {}: {} ({} fail, {} warning)\n", self.verdict, self.profile, self.file, self.fails, self.warnings);
        for f in &self.findings {
            let t = if f.end > f.start { format!("{:.2}-{:.2}s", f.start, f.end) } else { format!("{:.2}s", f.start) };
            s.push_str(&format!("  [{:?}] {} {}: {} Fix: {}\n", f.severity, f.check, t, f.message, f.fix));
        }
        s
    }
}

fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// Judge what was measured against `profile`.
pub fn report(profile: &Profile, facts: &Facts, video: Option<&VideoQc>, audio: Option<&AudioQc>) -> Report {
    let mut out: Vec<Finding> = Vec::new();
    let mut meas = serde_json::Map::new();
    meas.insert("size".into(), json!([facts.width, facts.height]));
    meas.insert("fps".into(), json!(facts.fps));
    meas.insert("duration".into(), json!(r3(facts.video_secs)));
    meas.insert("bytes".into(), json!(facts.bytes));

    // File-level.
    if let Some((w, h)) = profile.expect_size {
        if (facts.width, facts.height) != (w, h) {
            out.push(finding("size", Severity::Fail, 0.0, 0.0, format!("The picture is {}x{}; {} needs {w}x{h}.", facts.width, facts.height, profile.name), "Export with this delivery spec's size (the preset sets it)."));
        }
    }
    if facts.height > 0 {
        let aspect = facts.width as f64 / facts.height as f64;
        meas.insert("aspect".into(), json!(r3(aspect)));
        if let Some((w, h)) = profile.expect_size {
            let want = w as f64 / h as f64;
            if (aspect / want - 1.0).abs() > 0.01 {
                out.push(finding("aspect", Severity::Fail, 0.0, 0.0, format!("The picture's shape is {aspect:.3}:1; {} wants {want:.3}:1.", profile.name), "Reframe the sequence to the spec's shape (the reframe tool) or export the matching preset."));
            }
        }
    }
    if let Some(fps) = profile.expect_fps {
        if (facts.fps - fps).abs() > 0.01 {
            out.push(finding("frame_rate", Severity::Fail, 0.0, 0.0, format!("The frame rate is {:.3}; {} needs {fps}.", facts.fps, profile.name), "Set the sequence or export frame rate to the spec's."));
        }
    }
    if let Some(max) = profile.max_duration {
        if facts.video_secs > max + 0.05 {
            out.push(finding("duration", Severity::Fail, max, facts.video_secs, format!("The programme runs {:.1} s; {} allows {max:.0} s.", facts.video_secs, profile.name), "Cut it down, or make a cutdown version."));
        }
    }
    let has_audio = facts.audio_secs.is_some();
    if profile.expect_audio && !has_audio {
        out.push(finding("audio_missing", Severity::Fail, 0.0, 0.0, format!("The file has no sound; {} needs it.", profile.name), "Add sound, or export a version with the mix."));
    }
    if let Some(a) = facts.audio_secs {
        if (a - facts.video_secs).abs() > 0.25 {
            out.push(finding("av_length", Severity::Warning, a.min(facts.video_secs), a.max(facts.video_secs), format!("Sound runs {a:.2} s but the picture {:.2} s.", facts.video_secs), "Re-export so both end together; check for a clip ending past the last picture."));
        }
    }
    if let (Some(speech), Some(cues)) = (facts.speech_secs, facts.caption_cues) {
        if speech > 5.0 && cues == 0 {
            out.push(finding("captions", if profile.captions_required { Severity::Fail } else { Severity::Warning }, 0.0, 0.0, format!("About {speech:.0} s of speech and no captions."), "Make captions (ai captions) and export them with the bundle."));
        }
    } else if profile.captions_required && facts.caption_cues.is_none() {
        out.push(finding("captions", Severity::Warning, 0.0, 0.0, "Captions could not be checked.".into(), "Make captions and deliver them with the picture."));
    }

    // Picture.
    if let Some(v) = video {
        let n = v.stats.len();
        meas.insert("frames".into(), json!(n));
        if n > 0 {
            let fps = v.fps;
            let sev_levels = if profile.strict_levels { Severity::Fail } else { Severity::Warning };
            let over: Vec<bool> = v.stats.iter().map(|s| (s.illegal_share as f64) > profile.max_illegal_share || (s.luma_over + s.luma_under + s.chroma_out) as f64 > 4.0 * profile.max_illegal_share).collect();
            for (a, b) in runs(&over, 1) {
                let worst = v.stats[a..b].iter().map(|s| s.illegal_share.max(s.luma_over + s.luma_under + s.chroma_out)).fold(0f32, f32::max);
                out.push(finding("levels", sev_levels, a as f64 / fps, b as f64 / fps, format!("{:.1} % of the picture is outside legal levels (luma 16-235, chroma 16-240, RGB -5..105 %).", worst * 100.0), "Lower the highlights / raise the blacks or desaturate; a broadcast legaliser clip in the grade."));
            }
            let mean = |f: &dyn Fn(&FrameStat) -> f32| v.stats.iter().map(|s| f(s) as f64).sum::<f64>() / n as f64;
            meas.insert("levels".into(), json!({"frames_out_of_range": over.iter().filter(|b| **b).count(), "mean_luma_under": r3(mean(&|s| s.luma_under)), "mean_luma_over": r3(mean(&|s| s.luma_over)), "mean_chroma_out": r3(mean(&|s| s.chroma_out)), "mean_illegal_rgb": r3(mean(&|s| s.illegal_share)), "mean_luma": r3(mean(&|s| s.mean_yn))}));

            // Black.
            let black: Vec<bool> = v.stats.iter().map(|s| s.mean_yn <= 0.02 && s.max_yn <= 0.1).collect();
            let min_black = (profile.black_secs * fps).ceil().max(1.0) as usize;
            let mut black_total = 0usize;
            for (a, b) in runs(&black, min_black) {
                black_total += b - a;
                let (head, tail) = (a == 0, b == n);
                let secs = (b - a) as f64 / fps;
                // A fade in or out is normal; only long ones or ones inside the programme are findings.
                if (head || tail) && secs <= 2.0 {
                    continue;
                }
                let sev = if !head && !tail && secs >= 5.0 { Severity::Fail } else { Severity::Warning };
                out.push(finding("black", sev, a as f64 / fps, b as f64 / fps, format!("Black picture for {secs:.1} s{}.", if head { " at the start" } else if tail { " at the end" } else { "" }), "Trim the black, or fill it with picture; a deliberate black can stay."));
            }
            meas.insert("black_frames".into(), json!(black_total));

            // Frozen.
            let frozen: Vec<bool> = (0..n).map(|i| v.frozen[i] && !black[i]).collect();
            let min_frozen = (profile.frozen_secs * fps).ceil().max(2.0) as usize;
            let mut frozen_total = 0usize;
            for (a, b) in runs(&frozen, min_frozen) {
                frozen_total += b - a;
                out.push(finding("frozen", Severity::Warning, (a.max(1) - 1) as f64 / fps, b as f64 / fps, format!("The picture does not change for {:.1} s.", (b - a + 1) as f64 / fps), "If it is a still on purpose, ignore; otherwise a decode stall or clip gap: re-export, or add motion."));
            }
            meas.insert("frozen_frames".into(), json!(frozen_total));

            // Photosensitive.
            let sev_flash = if profile.strict_flash { Severity::Fail } else { Severity::Warning };
            for (a, b, c) in flash_windows(&v.flash_events, fps, profile.flash_limit) {
                out.push(finding("flash", sev_flash, a as f64 / fps, (b + 1) as f64 / fps, format!("{c} luminance flashes in one second (limit {}), over at least a quarter of the screen: a risk for photosensitive viewers (ITU-R BT.1702).", profile.flash_limit), "Slow the strobing cut, lower its contrast below 10 % luminance step, or shrink the flashing area."));
            }
            for (a, b, c) in flash_windows(&v.red_events, fps, profile.flash_limit) {
                out.push(finding("flash_red", sev_flash, a as f64 / fps, (b + 1) as f64 / fps, format!("{c} saturated-red flashes in one second (limit {}).", profile.flash_limit), "Desaturate the red or slow the alternation."));
            }
            let min_pat = (0.5 * fps).ceil() as usize;
            for (a, b) in runs(&v.pattern, min_pat) {
                out.push(finding("pattern", Severity::Warning, a as f64 / fps, b as f64 / fps, "Regular high-contrast stripes cover a large part of the picture (a risk for photosensitive viewers).".into(), "Soften the pattern's contrast, make the stripes fewer or move them."));
            }
            meas.insert("flash".into(), json!({"flash_events": v.flash_events.len(), "red_events": v.red_events.len()}));

            // Bars: present in nearly every frame.
            let med = |f: &dyn Fn(&FrameStat) -> f32| {
                let mut a: Vec<f32> = v.stats.iter().map(f).collect();
                a.sort_by(|x, y| x.total_cmp(y));
                a[a.len() / 2]
            };
            let (bt, bb, bl, br) = (med(&|s| s.bars_top), med(&|s| s.bars_bottom), med(&|s| s.bars_left), med(&|s| s.bars_right));
            meas.insert("bars".into(), json!({"top": r3(bt as f64), "bottom": r3(bb as f64), "left": r3(bl as f64), "right": r3(br as f64)}));
            let (hbar, vbar) = (bt + bb, bl + br);
            if hbar >= 0.04 || vbar >= 0.04 {
                let active = (v.size.0 as f64 * (1.0 - vbar as f64)) / (v.size.1 as f64 * (1.0 - hbar as f64));
                out.push(finding("bars", Severity::Warning, 0.0, facts.video_secs, format!("{} bars in the frame: the active picture is {active:.2}:1 inside a {}x{} frame.", if hbar >= vbar { "Letterbox" } else { "Pillarbox" }, v.size.0, v.size.1), "Reframe to fill the delivery shape, or deliver at the picture's own shape."));
            }
        }
    }

    // Sound.
    if let Some(a) = audio {
        let rep = a.meter.report();
        let dur = a.seconds();
        let (lufs, tp) = (rep.integrated_lufs, rep.true_peak_dbtp);
        let programme = a.speech_meter.as_ref().filter(|_| profile.dialogue_gated).map(|m| m.report());
        let measured = programme.map_or(lufs, |p| p.integrated_lufs);
        meas.insert("audio".into(), json!({
            "seconds": r3(dur),
            "integrated_lufs": if lufs.is_finite() { json!(r1(lufs)) } else { json!(null) },
            "dialogue_lufs": programme.filter(|p| p.integrated_lufs.is_finite()).map(|p| r1(p.integrated_lufs)),
            "loudness_range_lu": r1(rep.range_lu),
            "true_peak_dbtp": if tp.is_finite() { json!(r1(tp)) } else { json!(null) },
            "sample_peak_dbfs": if rep.sample_peak_dbfs.is_finite() { json!(r1(rep.sample_peak_dbfs)) } else { json!(null) },
        }));
        if !lufs.is_finite() {
            out.push(finding("audio_silent", if profile.expect_audio { Severity::Fail } else { Severity::Warning }, 0.0, dur, "The sound track is silent.".into(), "Check the mix: a muted lane, a missing clip or a bus at zero."));
        } else if let Some((target, tol, ceil)) = profile.loudness {
            if measured.is_finite() && (measured - target).abs() > tol {
                out.push(finding("loudness", Severity::Fail, 0.0, dur, format!("Integrated loudness {measured:.1} LUFS{}; {} wants {target:.0} ±{tol:.1} LU.", if programme.is_some() { " (dialogue)" } else { "" }, profile.name), "Run the loudness normaliser to the target (audio-post), or lower / raise the master."));
            }
            if tp > ceil {
                out.push(finding("true_peak", Severity::Fail, 0.0, dur, format!("True peak {tp:.1} dBTP is over the {ceil:.1} dBTP ceiling."), "Add a true-peak limiter on the master."));
            }
        } else if tp > -0.1 {
            out.push(finding("true_peak", Severity::Warning, 0.0, dur, format!("True peak {tp:.1} dBTP leaves no headroom; lossy encoding can clip."), "Limit the master to -1 dBTP."));
        }
        if !a.clips.is_empty() {
            let first = a.clips[0];
            out.push(finding("clipping", if a.clips.len() > 10 { Severity::Fail } else { Severity::Warning }, first, *a.clips.last().unwrap_or(&first), format!("Sound clips at {} places (3+ samples at full scale), first at {first:.2} s.", a.clips.len()), "Lower the gain of the loud clip or add a limiter."));
        }
        // Dropouts: digital silence 10 ms-2 s with sound on both sides.
        let rate = a.rate as u64;
        let mut drops = 0;
        for &(s, e) in &a.silences {
            let len = e - s;
            if len < rate / 100 || len > rate * 2 {
                continue;
            }
            let block = |f: u64| a.blocks.get((f / a.block_len) as usize).copied().unwrap_or(false);
            let before = s >= a.block_len && (1..=3).all(|k| block(s.saturating_sub(k * a.block_len)));
            let after = (1..=3).all(|k| block(e + k * a.block_len));
            if before && after {
                drops += 1;
                out.push(finding("dropout", Severity::Fail, s as f64 / rate as f64, e as f64 / rate as f64, format!("Sound drops to digital silence for {} ms in the middle of the programme.", len * 1000 / rate), "A clip gap, a bad edit or a decode error: fill the gap or crossfade; re-export."));
                if drops >= 20 {
                    break;
                }
            }
        }
        meas.insert("dropouts".into(), json!(drops));
        let n = (a.pos).max(1) as f64;
        let (rl, rr) = ((a.sum_l2 / n).sqrt(), (a.sum_r2 / n).sqrt());
        if rl > 1e-5 || rr > 1e-5 {
            let diff_db = 20.0 * (rl.max(1e-9) / rr.max(1e-9)).log10();
            if rl < 1e-5 || rr < 1e-5 {
                out.push(finding("channel", Severity::Fail, 0.0, dur, format!("The {} channel is silent while the other has sound.", if rl < 1e-5 { "left" } else { "right" }), "Check the pan / channel mapping: dual-mono or a one-sided mix."));
            } else if diff_db.abs() > 6.0 {
                out.push(finding("balance", Severity::Warning, 0.0, dur, format!("Left and right differ by {:.1} dB.", diff_db.abs()), "Rebalance the master (pan or gain)."));
            }
            let corr = a.sum_lr / (a.sum_l2.sqrt() * a.sum_r2.sqrt()).max(1e-12);
            meas["audio"]["correlation"] = json!(r3(corr));
            if corr < -0.3 {
                out.push(finding("phase", Severity::Warning, 0.0, dur, format!("Left and right are out of phase (correlation {corr:.2}); the mix cancels in mono."), "Flip the polarity of one side of the offending clip."));
            }
            let dc = (a.sum_l / n).abs().max((a.sum_r / n).abs());
            if dc > 0.01 {
                out.push(finding("dc_offset", Severity::Warning, 0.0, dur, format!("DC offset of {:.1} % of full scale.", dc * 100.0), "Add a high-pass filter at 20 Hz."));
            }
        }
    }

    out.sort_by(|a, b| b.severity.cmp(&a.severity).then(a.start.total_cmp(&b.start)));
    let fails = out.iter().filter(|f| f.severity == Severity::Fail).count();
    let warnings = out.iter().filter(|f| f.severity == Severity::Warning).count();
    let verdict = if fails > 0 { "FAIL" } else if warnings > 0 { "PASS with warnings" } else { "PASS" };
    Report { file: facts.file.clone(), profile: profile.name.clone(), verdict: verdict.into(), passed: fails == 0, fails, warnings, measurements: Value::Object(meas), findings: out }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grey(w: usize, h: usize, v: u8) -> Vec<u8> {
        let mut px = vec![v; w * h * 4];
        for p in px.chunks_exact_mut(4) {
            p[3] = 255;
        }
        px
    }

    fn feed(v: &mut VideoQc, px: &[u8]) {
        v.push(&Picture::Rgba { w: 64, h: 36, px });
    }

    fn facts() -> Facts {
        Facts { file: "t.mp4".into(), width: 64, height: 36, fps: 25.0, video_secs: 2.0, ..Default::default() }
    }

    #[test]
    fn a_strobe_fails_and_a_slow_fade_passes() {
        let mut v = VideoQc::new(25.0);
        for i in 0..50 {
            feed(&mut v, &grey(64, 36, if i % 2 == 0 { 20 } else { 235 }));
        }
        let r = report(&Profile::default(), &facts(), Some(&v), None);
        assert!(!r.passed, "{}", r.to_text());
        assert!(r.findings.iter().any(|f| f.check == "flash" && f.severity == Severity::Fail));

        let mut v = VideoQc::new(25.0);
        for i in 0..50u32 {
            feed(&mut v, &grey(64, 36, (40 + i * 3) as u8));
        }
        let r = report(&Profile::default(), &facts(), Some(&v), None);
        assert!(!r.findings.iter().any(|f| f.check == "flash"), "{}", r.to_text());
    }

    #[test]
    fn three_flashes_a_second_is_allowed_and_four_is_not() {
        // 25 fps: a flash cycle of 8 frames = ~3.1 pairs a second.
        let make = |period: usize| {
            let mut v = VideoQc::new(25.0);
            for i in 0..75 {
                feed(&mut v, &grey(64, 36, if (i / period) % 2 == 0 { 20 } else { 200 }));
            }
            report(&Profile::default(), &facts(), Some(&v), None)
        };
        // Switching every 4 frames: transitions 6.25 a second, flashes (opposing pairs) 6.25.
        assert!(make(4).findings.iter().any(|f| f.check == "flash"));
        // Switching every 12 frames: about 2 a second.
        assert!(!make(12).findings.iter().any(|f| f.check == "flash"));
    }

    #[test]
    fn red_flashes_are_found_apart_from_white_ones() {
        let red = |on: bool| {
            let mut px = vec![0u8; 64 * 36 * 4];
            for p in px.chunks_exact_mut(4) {
                p[0] = if on { 230 } else { 0 };
                p[3] = 255;
            }
            px
        };
        let mut v = VideoQc::new(25.0);
        for i in 0..50 {
            feed(&mut v, &red(i % 2 == 0));
        }
        let r = report(&Profile::default(), &facts(), Some(&v), None);
        assert!(r.findings.iter().any(|f| f.check == "flash_red"), "{}", r.to_text());
    }

    #[test]
    fn black_and_frozen_runs_are_timed() {
        let mut v = VideoQc::new(25.0);
        // 1 s of changing picture, 1 s black mid-programme, 4 s frozen grey, 1 s changing.
        for i in 0..25u32 {
            feed(&mut v, &grey(64, 36, (60 + i * 2) as u8));
        }
        for _ in 0..25 {
            feed(&mut v, &grey(64, 36, 0));
        }
        for _ in 0..100 {
            feed(&mut v, &grey(64, 36, 128));
        }
        for i in 0..25u32 {
            feed(&mut v, &grey(64, 36, (60 + i * 3) as u8));
        }
        let mut f = facts();
        f.video_secs = 7.0;
        let r = report(&Profile::default(), &f, Some(&v), None);
        let black = r.findings.iter().find(|f| f.check == "black").expect("black");
        assert!((black.start - 1.0).abs() < 0.05 && (black.end - 2.0).abs() < 0.05, "{black:?}");
        let frozen = r.findings.iter().find(|f| f.check == "frozen").expect("frozen");
        assert!(frozen.end - frozen.start >= 3.5, "{frozen:?}");
    }

    #[test]
    fn super_whites_are_illegal_in_planes_and_the_profile_decides_severity() {
        let (w, h) = (32usize, 18usize);
        let y = vec![250u8; w * h];
        let c = vec![128u8; (w / 2) * (h / 2)];
        let mut v = VideoQc::new(25.0);
        for _ in 0..5 {
            v.push(&Picture::Planes { w, h, y: &y, u: &c, v: &c, cw: w / 2, ch: h / 2, full_range: false });
        }
        let mut f = facts();
        f.width = 32;
        f.height = 18;
        f.video_secs = 0.2;
        let lenient = report(&Profile::default(), &f, Some(&v), None);
        assert!(lenient.passed && lenient.findings.iter().any(|f| f.check == "levels"), "{}", lenient.to_text());
        let strict = report(&Profile { strict_levels: true, ..Profile::default() }, &f, Some(&v), None);
        assert!(!strict.passed);
        // Legal white (235) is clean.
        let y = vec![235u8; w * h];
        let mut v = VideoQc::new(25.0);
        v.push(&Picture::Planes { w, h, y: &y, u: &c, v: &c, cw: w / 2, ch: h / 2, full_range: false });
        assert!(!report(&Profile::default(), &f, Some(&v), None).findings.iter().any(|f| f.check == "levels"));
    }

    #[test]
    fn letterbox_bars_are_measured() {
        let (w, h) = (64usize, 36usize);
        let mut px = grey(w, h, 150);
        for y in 0..h {
            if y < 5 || y >= h - 5 {
                for x in 0..w {
                    let i = (y * w + x) * 4;
                    px[i] = 0;
                    px[i + 1] = 0;
                    px[i + 2] = 0;
                }
            }
        }
        let mut v = VideoQc::new(25.0);
        for _ in 0..10 {
            feed(&mut v, &px);
        }
        let r = report(&Profile::default(), &facts(), Some(&v), None);
        assert!(r.findings.iter().any(|f| f.check == "bars" && f.message.contains("Letterbox")), "{}", r.to_text());
    }

    fn sine(rate: u32, secs: f64, amp: f32) -> Vec<f32> {
        (0..(rate as f64 * secs) as usize).map(|i| amp * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / rate as f32).sin()).collect()
    }

    #[test]
    fn loudness_peak_and_clipping_are_judged_against_the_target() {
        let profile = Profile { loudness: Some((-23.0, 0.5, -1.0)), expect_audio: true, ..Profile::default() };
        let mut f = facts();
        f.audio_secs = Some(10.0);
        f.video_secs = 10.0;
        // A loud tone: well over -23 LUFS and over the true-peak ceiling.
        let loud = sine(48_000, 10.0, 0.98);
        let mut a = AudioQc::new(48_000);
        a.push(&loud, &loud);
        let r = report(&profile, &f, None, Some(&a));
        assert!(r.findings.iter().any(|x| x.check == "loudness"), "{}", r.to_text());
        assert!(r.findings.iter().any(|x| x.check == "true_peak"), "{}", r.to_text());
        // A tone at -23 LUFS passes: a stereo 1 kHz sine of amplitude A reads about 20log10(A) - 3.0 + ...; find the gain by measurement.
        let probe = studio_sound::loudness::measure(&sine(48_000, 10.0, 0.1), &sine(48_000, 10.0, 0.1), 48_000);
        let gain = 10f32.powf(((-23.0 - probe.integrated_lufs) / 20.0) as f32);
        let ok = sine(48_000, 10.0, 0.1 * gain);
        let mut a = AudioQc::new(48_000);
        a.push(&ok, &ok);
        let r = report(&profile, &f, None, Some(&a));
        assert!(r.passed, "{}", r.to_text());
    }

    #[test]
    fn a_dropout_in_the_middle_and_a_dead_channel_are_found() {
        let mut l = sine(48_000, 4.0, 0.3);
        // 50 ms of digital silence at 2 s.
        for s in l.iter_mut().skip(96_000).take(2_400) {
            *s = 0.0;
        }
        let mut a = AudioQc::new(48_000);
        a.push(&l, &l);
        let mut f = facts();
        f.audio_secs = Some(4.0);
        f.video_secs = 4.0;
        let r = report(&Profile::default(), &f, None, Some(&a));
        let d = r.findings.iter().find(|x| x.check == "dropout").expect("dropout");
        assert!((d.start - 2.0).abs() < 0.01, "{d:?}");
        let silent = vec![0f32; l.len()];
        let mut a = AudioQc::new(48_000);
        a.push(&sine(48_000, 4.0, 0.3), &silent);
        assert!(report(&Profile::default(), &f, None, Some(&a)).findings.iter().any(|x| x.check == "channel"));
    }

    #[test]
    fn file_facts_catch_missing_sound_captions_and_wrong_shape() {
        let profile = Profile { name: "Test".into(), expect_audio: true, expect_size: Some((1920, 1080)), expect_fps: Some(24.0), max_duration: Some(60.0), captions_required: true, ..Profile::default() };
        let f = Facts { file: "x.mp4".into(), width: 1080, height: 1920, fps: 30.0, video_secs: 90.0, audio_secs: None, speech_secs: Some(40.0), caption_cues: Some(0), ..Default::default() };
        let r = report(&profile, &f, None, None);
        for c in ["size", "aspect", "frame_rate", "duration", "audio_missing", "captions"] {
            assert!(r.findings.iter().any(|x| x.check == c), "{c} missing in {}", r.to_text());
        }
        assert_eq!(r.verdict, "FAIL");
        assert!(r.to_json().contains("\"findings\""));
    }

    #[test]
    fn a_clean_programme_passes() {
        let mut v = VideoQc::new(25.0);
        for i in 0..50u32 {
            feed(&mut v, &grey(64, 36, (60 + i) as u8));
        }
        let r = report(&Profile::default(), &facts(), Some(&v), None);
        assert_eq!(r.verdict, "PASS", "{}", r.to_text());
    }
}
