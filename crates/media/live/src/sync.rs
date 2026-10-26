// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Automatic sync of separately recorded sound to picture: the camera's
//! own (scratch) audio and the recorder's file hear the same room, so the
//! lag that lines them up is where their cross-correlation peaks. The
//! search runs on both signals at 8 kHz with phase-transform weighting
//! (robust to different microphones and levels), then the peak is refined
//! sample-accurately at full rate. Drift between two clocks is read from
//! the lag at the start and at the end.

/// Where the external recording belongs against the camera's audio.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct SyncResult {
    /// Seconds into the camera clip at which the external file's first
    /// sample belongs (negative: the recorder started before the camera).
    pub offset_s: f64,
    /// How many standard deviations the peak stands above the rest of the
    /// correlation: under 8 is a guess, over 15 is certain.
    pub confidence: f64,
    /// How much faster (+) or slower the external clock runs, parts per
    /// million; `None` when the clips are too short to tell.
    pub drift_ppm: Option<f64>,
}

const WORK_RATE: u32 = 8000;

pub fn resample(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to {
        return x.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let n = (x.len() as f64 / ratio) as usize;
    let mut out = Vec::with_capacity(n);
    // Box-filter average over each output sample's span: enough lowpass
    // for correlation.
    for i in 0..n {
        let a = i as f64 * ratio;
        let b = ((i + 1) as f64 * ratio).min(x.len() as f64);
        let (ia, ib) = (a as usize, (b.ceil() as usize).min(x.len()).max(a as usize + 1));
        let s: f32 = x[ia..ib.min(x.len())].iter().sum();
        out.push(s / (ib - ia).max(1) as f32);
    }
    out
}

fn fft(re: &mut [f32], im: &mut [f32], inverse: bool) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = 2.0 * std::f64::consts::PI / len as f64 * if inverse { 1.0 } else { -1.0 };
        let (wr, wi) = (ang.cos() as f32, ang.sin() as f32);
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * cr - im[b] * ci, re[b] * ci + im[b] * cr);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let t = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = t;
            }
        }
        len <<= 1;
    }
    if inverse {
        for v in re.iter_mut().chain(im.iter_mut()) {
            *v /= n as f32;
        }
    }
}

/// Lag (in samples) at which `y` best lines up with `x`: positive when
/// `y`'s start belongs after `x`'s start. Also the peak-to-rest ratio.
fn gcc_phat(x: &[f32], y: &[f32]) -> (i64, f64) {
    let n = (x.len() + y.len()).next_power_of_two().max(2);
    let (mut xr, mut xi) = (vec![0f32; n], vec![0f32; n]);
    let (mut yr, mut yi) = (vec![0f32; n], vec![0f32; n]);
    xr[..x.len()].copy_from_slice(x);
    yr[..y.len()].copy_from_slice(y);
    fft(&mut xr, &mut xi, false);
    fft(&mut yr, &mut yi, false);
    // X · conj(Y), whitened with a floor so silent bins stay quiet.
    let mut cr = vec![0f32; n];
    let mut ci = vec![0f32; n];
    let mut peak_mag = 0f32;
    for k in 0..n {
        peak_mag = peak_mag.max((xr[k] * xr[k] + xi[k] * xi[k]).sqrt() * (yr[k] * yr[k] + yi[k] * yi[k]).sqrt());
    }
    let floor = peak_mag * 1e-4 + 1e-12;
    for k in 0..n {
        let (a, b) = (xr[k], xi[k]);
        let (c, d) = (yr[k], -yi[k]);
        let (pr, pi) = (a * c - b * d, a * d + b * c);
        let m = (pr * pr + pi * pi).sqrt().max(floor);
        // Soft PHAT: damp the bins below the floor instead of boosting them.
        let w = 1.0 / m.max(floor);
        let scale = if (pr * pr + pi * pi).sqrt() < floor { (pr * pr + pi * pi).sqrt() / floor } else { 1.0 };
        cr[k] = pr * w * scale;
        ci[k] = pi * w * scale;
    }
    fft(&mut cr, &mut ci, true);
    // cr[i] for i in 0..x.len() is lag +i (x later than y); the tail is
    // negative lags.
    let mut best = (0usize, f32::MIN);
    let (mut sum, mut sum2) = (0f64, 0f64);
    for (i, v) in cr.iter().enumerate() {
        sum += *v as f64;
        sum2 += (*v as f64) * (*v as f64);
        if *v > best.1 {
            best = (i, *v);
        }
    }
    let mean = sum / n as f64;
    let std = (sum2 / n as f64 - mean * mean).max(0.0).sqrt();
    // Positive correlation index i means x[t + i] ~ y[t]: y belongs i
    // samples *earlier*... mapping done below.
    let lag = if best.0 < n / 2 { best.0 as i64 } else { best.0 as i64 - n as i64 };
    (lag, (best.1 as f64 - mean) / std.max(1e-12))
}

/// Refines a lag at full rate: `x` (camera) and `y` (external) at `rate`,
/// searching ±`radius` samples around `lag` (lag as `gcc_phat` gave it).
fn refine(x: &[f32], y: &[f32], lag: i64, radius: i64, span: usize) -> i64 {
    // y[t] ≈ x[t + lag]: compare a stretch of y with x shifted.
    let start_y = (-lag).max(0) as usize + 0;
    let start_y = start_y.min(y.len());
    let len = span.min(y.len().saturating_sub(start_y)).min(x.len().saturating_sub((start_y as i64 + lag).max(0) as usize));
    if len < 64 {
        return lag;
    }
    let mut best = (lag, f64::MIN);
    for d in -radius..=radius {
        let l = lag + d;
        let mut s = 0f64;
        let mut n = 0;
        for t in 0..len {
            let yi = start_y + t;
            let xi = yi as i64 + l;
            if xi < 0 || xi as usize >= x.len() || yi >= y.len() {
                continue;
            }
            s += x[xi as usize] as f64 * y[yi] as f64;
            n += 1;
        }
        if n > 0 && s > best.1 {
            best = (l, s);
        }
    }
    best.0
}

/// Lines `ext` (the recorder's file) up against `cam` (the camera's audio).
/// Both are mono at their own rates.
pub fn align(cam: &[f32], cam_rate: u32, ext: &[f32], ext_rate: u32) -> Result<SyncResult, String> {
    if cam.len() < cam_rate as usize / 2 || ext.len() < ext_rate as usize / 2 {
        return Err("both recordings need at least half a second of sound".into());
    }
    let cap = |x: &[f32], r: u32| -> usize { x.len().min(r as usize * 600) };
    let c8 = resample(&cam[..cap(cam, cam_rate)], cam_rate, WORK_RATE);
    let e8 = resample(&ext[..cap(ext, ext_rate)], ext_rate, WORK_RATE);
    let (lag8, conf) = gcc_phat(&c8, &e8);
    // gcc_phat's sign: lag > 0 means the peak is at index lag of X·conj(Y)
    // → x[t + lag] ~ y[t]: the external file's start sits `lag` into the
    // camera clip.
    let rate = cam_rate.max(ext_rate);
    // Bring both to a common rate for the refinement (the higher one).
    let (cf, ef) = (resample(cam, cam_rate, rate), resample(ext, ext_rate, rate));
    let k = rate as i64 / WORK_RATE as i64;
    let lag = refine(&cf, &ef, lag8 * k, k * 2, (rate as usize * 10).min(cf.len()));
    let offset = lag as f64 / rate as f64;
    // Drift: the same lag read near the end of the overlap.
    let drift = {
        let overlap_start = offset.max(0.0);
        let ext_start = (-offset).max(0.0);
        let overlap = (cam.len() as f64 / cam_rate as f64 - overlap_start).min(ext.len() as f64 / ext_rate as f64 - ext_start);
        if overlap >= 120.0 {
            let win = 20.0;
            let w = |x: &[f32], r: u32, from: f64| -> Vec<f32> { x[(from * r as f64) as usize..((from + win) * r as f64) as usize].to_vec() };
            let t_end = overlap - win;
            let c = w(cam, cam_rate, overlap_start + t_end);
            let e = w(ext, ext_rate, ext_start + t_end);
            let (l, cf2) = gcc_phat(&resample(&c, cam_rate, WORK_RATE), &resample(&e, ext_rate, WORK_RATE));
            if cf2 > 8.0 { Some(l as f64 / WORK_RATE as f64 / t_end * 1e6) } else { None }
        } else {
            None
        }
    };
    Ok(SyncResult { offset_s: offset, confidence: conf, drift_ppm: drift })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{AudioSource, TestAudio};

    fn room(seed: u32, rate: u32, secs: f32) -> Vec<f32> {
        TestAudio::from_spec(&format!("noise:{seed}@0.8"), rate).unwrap().read((secs * rate as f32) as usize)
    }

    #[test]
    fn finds_the_offset_of_a_recorder_that_started_later() {
        let rate = 48000;
        let room = room(3, rate, 12.0);
        // Camera hears the room from 0; the recorder started 4.25 s in,
        // with different level and a little noise of its own.
        let cam: Vec<f32> = room.iter().map(|x| x * 0.3).collect();
        let start = (4.25 * rate as f64) as usize;
        let mut other = TestAudio::from_spec("noise:99@0.05", rate).unwrap();
        let hiss = other.read(room.len() - start);
        let ext: Vec<f32> = room[start..].iter().zip(&hiss).map(|(a, h)| a * 1.7 + h).collect();
        let r = align(&cam, rate, &ext, rate).unwrap();
        assert!((r.offset_s - 4.25).abs() < 0.001, "{r:?}");
        assert!(r.confidence > 15.0, "{r:?}"); eprintln!("conf {}", r.confidence);
    }

    #[test]
    fn finds_a_recorder_that_started_earlier_and_runs_at_another_rate() {
        let room48 = room(8, 48000, 15.0);
        // Recorder began 2.5 s before the camera, at 44.1 kHz.
        let pre = TestAudio::from_spec("noise:41@0.8", 48000).unwrap().read(48000 * 5 / 2);
        let mut heard: Vec<f32> = pre;
        heard.extend_from_slice(&room48);
        let ext = resample(&heard, 48000, 44100);
        let r = align(&room48, 48000, &ext, 44100).unwrap();
        assert!((r.offset_s + 2.5).abs() < 0.002, "{r:?}");
        assert!(r.confidence > 8.0);
    }

    #[test]
    fn unrelated_sound_has_low_confidence() {
        let a = room(1, 16000, 10.0);
        let b = room(2, 16000, 10.0);
        let r = align(&a, 16000, &b, 16000).unwrap();
        eprintln!("unrelated {}", r.confidence); assert!(r.confidence < 8.0, "{r:?}");
    }

    #[test]
    fn fft_round_trips() {
        let mut re: Vec<f32> = (0..64).map(|i| (i as f32 * 0.3).sin()).collect();
        let orig = re.clone();
        let mut im = vec![0.0; 64];
        fft(&mut re, &mut im, false);
        fft(&mut re, &mut im, true);
        for (a, b) in re.iter().zip(orig) {
            assert!((a - b).abs() < 1e-4);
        }
    }
}
