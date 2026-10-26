// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Rendering the mix: measuring it against a delivery specification,
//! normalising to it in one step, stems (DX / MX / FX / BG / M&E / full mix
//! and each dubbed language), an ADM file with objects for immersive
//! mastering, and the audio tracks an MP4 carries (stereo and 5.1 per
//! language).

use super::mix::MixRequest;
use super::{Delivery, Desk, LaneMix};
use std::path::{Path, PathBuf};
use studio_model as m;
use studio_sound::loudness::{LoudnessReport, LoudnessTarget, measure_layout};
use studio_sound::mixer::{BusKind, mix_offline_layout};
use studio_sound::resample::Quality;
use studio_sound::surround::Layout;
use studio_sound::wav::{Bext, SampleFormat};

const RATE: u32 = crate::sound_bridge::RATE;

/// The project's mix for `[start, end)` as planar channels of the
/// request's layout.
pub fn render(project: &m::Project, req: &MixRequest, start: m::Micros, end: m::Micros) -> Vec<Vec<f32>> {
    let plan = crate::sound_bridge::plan_for(project, req);
    mix_offline_layout(plan, RATE, start, end, Quality::Best)
}

/// End of the sound (the sequence length).
pub fn end_of(project: &m::Project) -> m::Micros {
    project.sequence(project.active_sequence).map(|s| s.duration()).unwrap_or(0)
}

/// A loudness reading of the mix.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub report: LoudnessReport,
    /// The loudness the specification is judged on (dialogue-gated when
    /// it asks for that).
    pub lufs: f64,
    pub dialogue_gated: bool,
    /// Share of the programme that is speech (for dialogue gating).
    pub speech_share: f64,
}

/// Speech spans (frames) of the dialogue stem.
fn speech_spans(project: &m::Project, layout: Layout, end: m::Micros) -> Vec<(usize, usize)> {
    let req = MixRequest { layout: Some(Layout::Stereo), stem: Some(vec![BusKind::Dialogue]), normalise: false, limiter: false, ..Default::default() };
    let _ = layout;
    let dx = render(project, &req, 0, end);
    let mono: Vec<f32> = dx[0].iter().zip(&dx[1]).map(|(a, b)| 0.5 * (a + b)).collect();
    studio_sound::vad::speech_segments(&mono, RATE, studio_sound::vad::VadParams::default())
        .into_iter()
        .map(|(a, b)| (studio_sound::micros_to_frames(a, RATE).max(0) as usize, studio_sound::micros_to_frames(b, RATE).max(0) as usize))
        .collect()
}

/// Measures planar channels, optionally only inside `spans`.
fn measure_chans(chans: &[Vec<f32>], layout: Layout, spans: Option<&[(usize, usize)]>) -> LoudnessReport {
    match spans {
        Some(sp) if !sp.is_empty() => {
            let cut: Vec<Vec<f32>> = chans.iter().map(|c| sp.iter().flat_map(|&(a, b)| c[a.min(c.len())..b.min(c.len())].iter().copied()).collect()).collect();
            measure_layout(&cut, RATE, layout)
        }
        _ => measure_layout(chans, RATE, layout),
    }
}

/// Measures the delivered mix (normalised as the desk says, with its
/// limiter) against `target`.
pub fn measure(project: &m::Project, target: LoudnessTarget, layout: Layout) -> Reading {
    measure_with(project, target, &MixRequest::delivered(layout))
}

fn measure_with(project: &m::Project, target: LoudnessTarget, req: &MixRequest) -> Reading {
    let end = end_of(project);
    let layout = req.layout.unwrap_or(Layout::Stereo);
    let chans = render(project, req, 0, end);
    let report = measure_layout(&chans, RATE, layout);
    if !target.dialogue_gated() {
        return Reading { lufs: report.integrated_lufs, report, dialogue_gated: false, speech_share: 0.0 };
    }
    let spans = speech_spans(project, layout, end);
    let total = chans.first().map_or(1, Vec::len).max(1);
    let speech: usize = spans.iter().map(|(a, b)| b.saturating_sub(*a)).sum();
    let gated = measure_chans(&chans, layout, Some(&spans));
    // No speech at all: the whole programme is measured (as the spec says).
    let lufs = if gated.integrated_lufs.is_finite() { gated.integrated_lufs } else { report.integrated_lufs };
    Reading { report, lufs, dialogue_gated: gated.integrated_lufs.is_finite(), speech_share: speech as f64 / total as f64 }
}

/// Works out the master gain that brings the mix to `target`: measures it
/// without the limiter, sets the gain to reach the target, then measures
/// the delivered mix with the target's true-peak ceiling and corrects (up
/// to twice) if the limiter pulled it down. Pure: runs on a copy, so the
/// window can do it on a worker thread.
pub fn normalise_gain(project: &m::Project, target: LoudnessTarget) -> Result<(f32, Reading), String> {
    let layout = Desk::of(project).layout();
    if end_of(project) <= 0 {
        return Err("The timeline is empty: nothing to normalise".into());
    }
    let mut project = project.clone();
    let set = |p: &mut m::Project, gain: f32| {
        let mut d = Desk::of(p);
        d.delivery = Some(Delivery { target: target.id().into(), gain_db: gain, ..Default::default() });
        p.settings = d.stored(&p.settings);
    };
    set(&mut project, 0.0);
    let raw = measure_with(&project, target, &MixRequest { layout: Some(layout), limiter: false, ..Default::default() });
    if !raw.lufs.is_finite() {
        return Err("The mix is silent: nothing to normalise".into());
    }
    let mut gain = (target.lufs() - raw.lufs) as f32;
    set(&mut project, gain);
    let mut reading = measure(&project, target, layout);
    for _ in 0..2 {
        let miss = target.lufs() - reading.lufs;
        if miss.abs() < 0.1 || !reading.lufs.is_finite() {
            break;
        }
        gain += miss as f32;
        set(&mut project, gain);
        reading = measure(&project, target, layout);
    }
    Ok((gain, reading))
}

/// Stores a normalisation in the desk (one undo step).
pub fn store_normalised(doc: &mut crate::doc::Doc, target: LoudnessTarget, gain: f32, r: &Reading) -> Result<(), String> {
    let r = r.clone();
    super::edit_desk(doc, "Normalise loudness", move |d| {
        d.delivery = Some(Delivery {
            target: target.id().into(),
            gain_db: (gain * 100.0).round() / 100.0,
            measured_lufs: Some((r.lufs * 10.0).round() / 10.0),
            true_peak_dbtp: Some((r.report.true_peak_dbtp * 10.0).round() / 10.0),
            range_lu: Some((r.report.range_lu * 10.0).round() / 10.0),
        });
        Ok(String::new())
    })
    .map(|_| ())
}

/// Normalises the mix to `target` in one step ([`normalise_gain`], then
/// [`store_normalised`]).
pub fn normalise(doc: &mut crate::doc::Doc, target: LoudnessTarget) -> Result<Reading, String> {
    let (gain, reading) = normalise_gain(&doc.model, target)?;
    store_normalised(doc, target, gain, &reading)?;
    Ok(reading)
}

/// Whether a reading meets a target, and what is wrong when it does not.
pub fn verdict(target: LoudnessTarget, r: &Reading) -> (bool, Vec<String>) {
    let mut out = Vec::new();
    let off = r.lufs - target.lufs();
    if !r.lufs.is_finite() {
        out.push("silent".into());
    } else if off.abs() > target.tolerance_lu() {
        out.push(format!("{:.1} LUFS is {:.1} LU {} the {} target of {:.0} (±{})", r.lufs, off.abs(), if off > 0.0 { "over" } else { "under" }, target.label(), target.lufs(), target.tolerance_lu()));
    }
    if r.report.true_peak_dbtp > target.ceiling_dbtp() + 0.05 {
        out.push(format!("true peak {:.1} dBTP is over the {:.0} dBTP ceiling", r.report.true_peak_dbtp, target.ceiling_dbtp()));
    }
    if target.dialogue_gated() && !r.dialogue_gated {
        out.push("no dialogue found to gate on: the whole programme was measured".into());
    }
    (out.is_empty(), out)
}

/// One written file.
#[derive(Clone, Debug)]
pub struct Written {
    pub name: String,
    pub path: PathBuf,
    pub peak_dbfs: f64,
}

fn peak_db(chans: &[Vec<f32>]) -> f64 {
    let p = chans.iter().flat_map(|c| c.iter()).fold(0.0f32, |m, v| m.max(v.abs()));
    (20.0 * (p.max(1e-9) as f64).log10() * 10.0).round() / 10.0
}

/// The stems a delivery needs: DX, MX, FX (effects and backgrounds), M&E
/// (everything but dialogue) and the full mix, plus, for every dubbed
/// language, its dialogue stem and its full mix. Each is a 24-bit
/// Broadcast Wave in `layout` at 48 kHz; stems are unlimited (they add up
/// to the mix before its limiter) and carry the normalisation gain.
pub fn export_stems(project: &m::Project, dir: &Path, layout: Layout, separate_bg: bool, title: &str) -> Result<Vec<Written>, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let end = end_of(project);
    if end <= 0 {
        return Err("The timeline is empty: no stems to write".into());
    }
    let desk = Desk::of(project);
    let title = if title.trim().is_empty() { "Zornov".to_owned() } else { title.trim().to_owned() };
    let mut jobs: Vec<(String, MixRequest)> = vec![
        ("DX".into(), MixRequest { layout: Some(layout), stem: Some(vec![BusKind::Dialogue]), ..Default::default() }),
        ("MX".into(), MixRequest { layout: Some(layout), stem: Some(vec![BusKind::Music]), ..Default::default() }),
    ];
    if separate_bg {
        jobs.push(("FX".into(), MixRequest { layout: Some(layout), stem: Some(vec![BusKind::Effects]), ..Default::default() }));
        jobs.push(("BG".into(), MixRequest { layout: Some(layout), stem: Some(vec![BusKind::Ambience]), ..Default::default() }));
    } else {
        jobs.push(("FX".into(), MixRequest { layout: Some(layout), stem: Some(vec![BusKind::Effects, BusKind::Ambience]), ..Default::default() }));
    }
    jobs.push(("M&E".into(), MixRequest { layout: Some(layout), stem: Some(vec![BusKind::Music, BusKind::Effects, BusKind::Ambience]), ..Default::default() }));
    jobs.push(("Mix".into(), MixRequest::delivered(layout)));
    for l in &desk.languages {
        jobs.push((format!("DX-{}", l.lang), MixRequest { layout: Some(layout), stem: Some(vec![BusKind::Dialogue]), language: Some(l.lang.clone()), ..Default::default() }));
        jobs.push((format!("Mix-{}", l.lang), MixRequest { language: Some(l.lang.clone()), ..MixRequest::delivered(layout) }));
    }
    let mut out = Vec::new();
    for (name, req) in jobs {
        let chans = render(project, &req, 0, end);
        let bext = Bext { description: format!("{title} {name} {}", layout.id()), originator: "Zornov".into(), time_reference: 0 };
        let bytes = studio_sound::wav::write(&chans, RATE, SampleFormat::Pcm24, Some(layout), Some(&bext));
        let path = dir.join(format!("{} {}.wav", safe(&title), name.replace('&', "and")));
        std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        out.push(Written { peak_dbfs: peak_db(&chans), name, path });
    }
    Ok(out)
}

fn safe(s: &str) -> String {
    s.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' }).collect::<String>().trim().to_owned()
}

/// An ADM Broadcast Wave for immersive mastering: a bed of `layout` with
/// every lane not marked as an object, plus one mono object per object
/// lane moving along its surround automation (sampled every 100 ms where
/// it moves).
pub fn export_adm(project: &m::Project, path: &Path, layout: Layout, title: &str) -> Result<(usize, usize), String> {
    let end = end_of(project);
    if end <= 0 {
        return Err("The timeline is empty".into());
    }
    let layout = if layout == Layout::Stereo { Layout::Surround51 } else { layout };
    let bed = render(project, &MixRequest { layout: Some(layout), without_objects: true, limiter: false, ..Default::default() }, 0, end);
    let seq = project.sequence(project.active_sequence).ok_or("no sequence")?;
    let mut objects = Vec::new();
    for lane in &seq.lanes {
        let mix = LaneMix::of(lane);
        if !mix.object || lane.muted {
            continue;
        }
        let st = render(project, &MixRequest { layout: Some(Layout::Stereo), only_lane: Some(lane.id.0), limiter: false, ..Default::default() }, 0, end);
        let mono: Vec<f32> = st[0].iter().zip(&st[1]).map(|(a, b)| 0.5 * (a + b)).collect();
        let spec = super::mix::lane_spec(lane, false);
        let moving = !(mix.surround.x_keys.is_empty() && mix.surround.y_keys.is_empty() && mix.surround.z_keys.is_empty());
        let mut path_pts = Vec::new();
        let step = if moving { 100_000 } else { end.max(1) };
        let mut t = 0;
        while t < end {
            let p = spec.surround.at(t);
            if path_pts.last().is_none_or(|q: &(f64, f32, f32, f32)| (q.1 - p.x).abs() > 1e-3 || (q.2 - p.y).abs() > 1e-3 || (q.3 - p.z).abs() > 1e-3) {
                path_pts.push((t as f64 / 1e6, p.x, p.y, p.z));
            }
            t += step;
        }
        objects.push(studio_sound::adm::Object { name: lane.name.clone(), samples: mono, path: path_pts, width: (spec.surround.shape.width / 180.0).clamp(0.0, 1.0) });
    }
    let n = objects.len();
    let bytes = studio_sound::adm::write(&bed, layout, &objects, RATE, if title.is_empty() { "Zornov" } else { title });
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((layout.channels(), n))
}

/// One audio track for an MP4: language, title, layout, interleaved PCM in
/// AAC channel order input (stereo `L R`, 5.1 `L R C LFE Ls Rs`).
pub struct Track {
    pub lang: String,
    pub title: String,
    pub layout: Layout,
    pub pcm: Vec<f32>,
}

/// The audio tracks an export carries for `[t0, t1)`: the original
/// language first (stereo, then 5.1 when the desk mixes in surround), then
/// each dubbed language the same way. A 7.1 desk adds its 5.1 render (MP4's
/// AAC here carries up to 5.1; 7.1 goes out as stems).
pub fn tracks(project: &m::Project, t0: m::Micros, t1: m::Micros) -> Vec<Track> {
    let desk = Desk::of(project);
    let surround = desk.layout() != Layout::Stereo;
    let mut langs: Vec<(Option<String>, String)> = vec![(None, desk.original_language.clone())];
    for l in &desk.languages {
        langs.push((Some(l.lang.clone()), l.lang.clone()));
    }
    let mut out = Vec::new();
    for (lang, code) in langs {
        let name = studio_text::subtitles::language_name(&code);
        let dub = lang.is_some();
        for layout in if surround { vec![Layout::Stereo, Layout::Surround51] } else { vec![Layout::Stereo] } {
            let req = MixRequest { language: lang.clone(), ..MixRequest::delivered(layout) };
            let chans = render(project, &req, t0, t1);
            let n = chans.first().map_or(0, Vec::len);
            let ch = chans.len();
            let mut pcm = Vec::with_capacity(n * ch);
            for i in 0..n {
                for c in &chans {
                    pcm.push(c[i]);
                }
            }
            let title = format!("{name}{}{}", if dub { " (dub)" } else { "" }, if layout == Layout::Stereo { "" } else { " 5.1" });
            out.push(Track { lang: studio_text::subtitles::iso639_2(&code).into(), title, layout, pcm });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sound_desk::{Language, LaneMix};
    use studio_sound::loudness::LoudnessTarget;

    /// A project with a dialogue-ish tone lane, a music lane and an effects
    /// lane, all from WAVs written into `dir`.
    pub(crate) fn project(dir: &Path) -> m::Project {
        std::fs::create_dir_all(dir).unwrap();
        let mut p = crate::services::project::new_custom("Mix test", 640, 360, 25.0).model;
        let write = |name: &str, f: f32, amp: f32, gaps: bool| {
            let n = RATE as usize * 4;
            let samples: Vec<f32> = (0..n)
                .flat_map(|i| {
                    let t = i as f32 / RATE as f32;
                    let on = !gaps || (t % 1.0) < 0.6;
                    let v = if on { amp * (std::f32::consts::TAU * f * t).sin() * (1.0 + 0.5 * (std::f32::consts::TAU * 3.0 * t).sin()) } else { 0.0 };
                    [v, v]
                })
                .collect();
            let path = dir.join(name);
            std::fs::write(&path, studio_media::wav::encode_pcm16(&studio_media::wav::Wav { sample_rate: RATE, channels: 2, samples })).unwrap();
            path
        };
        let files = [("voice.wav", 220.0, 0.3, true, "Dialogue"), ("music.wav", 440.0, 0.1, false, "Music"), ("fx.wav", 880.0, 0.05, false, "Effects")];
        let seq_id = p.active_sequence;
        for (k, (file, f, amp, gaps, lane_name)) in files.iter().enumerate() {
            let path = write(file, *f, *amp, *gaps);
            let mid = m::MediaId(100 + k as u64);
            let info = m::MediaInfo { kind: m::MediaKind::Audio, duration: Some(4_000_000), ..Default::default() };
            p.media.push(m::MediaItem::new(mid, path.display().to_string(), info));
            let lane = m::LaneId(10 + k as u64);
            let seq = p.sequences.iter_mut().find(|s| s.id == seq_id).unwrap();
            seq.lanes.push(m::Lane::new(lane, m::LaneKind::Audio, *lane_name));
            let mut mref = m::MediaRef::new(mid);
            mref.video = false;
            let mut c = m::Clip::new(m::ClipId(200 + k as u64), lane, 0, 4_000_000, m::ClipSource::Media(mref));
            c.name = file.to_string();
            seq.clips.push(c);
        }
        p
    }

    #[test]
    fn normalising_reaches_each_target_and_dialogue_gating_measures_speech() {
        let dir = std::env::temp_dir().join(format!("studio-desk-norm-{}", std::process::id()));
        let p = project(&dir);
        let mut doc = crate::doc::Doc::new(p);
        for target in [LoudnessTarget::Broadcast, LoudnessTarget::Netflix, LoudnessTarget::Streaming] {
            let r = normalise(&mut doc, target).unwrap();
            let (ok, why) = verdict(target, &r);
            assert!(ok, "{target:?}: {why:?} ({r:?})");
            assert_eq!(r.dialogue_gated, target.dialogue_gated());
            let d = Desk::of(&doc.model).delivery.unwrap();
            assert_eq!(d.target, target.id());
            // The stored reading matches a fresh one.
            let again = measure(&doc.model, target, Layout::Stereo);
            assert!((again.lufs - target.lufs()).abs() <= target.tolerance_lu(), "{again:?}");
        }
        let r = measure(&doc.model, LoudnessTarget::Netflix, Layout::Stereo);
        assert!(r.speech_share > 0.3 && r.speech_share < 0.9, "{}", r.speech_share);
        assert!(doc.undo().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stems_and_language_mixes() {
        let dir = std::env::temp_dir().join(format!("studio-desk-stems-{}", std::process::id()));
        let mut p = project(&dir);
        // A Spanish dub lane with its own line.
        let mut d = Desk::of(&p);
        d.add_film_buses();
        d.languages.push(Language { lang: "es".into(), name: "Español".into(), lane: Some(13) });
        d.layout = "5.1".into();
        p.settings = d.stored(&p.settings);
        let seq_id = p.active_sequence;
        let seq = p.sequences.iter_mut().find(|s| s.id == seq_id).unwrap();
        let mut dub = m::Lane::new(m::LaneId(13), m::LaneKind::Audio, "Dub es");
        LaneMix { dub: Some("es".into()), role: Some("dialogue".into()), ..Default::default() }.store(&mut dub);
        seq.lanes.push(dub);
        let src = seq.clips[1].source.clone();
        seq.clips.push(m::Clip::new(m::ClipId(299), m::LaneId(13), 0, 4_000_000, src));
        let out = dir.join("stems");
        let w = export_stems(&p, &out, Layout::Surround51, false, "Mix test").unwrap();
        let names: Vec<&str> = w.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, vec!["DX", "MX", "FX", "M&E", "Mix", "DX-es", "Mix-es"]);
        let read = |name: &str| studio_sound::wav::parse(&std::fs::read(&w.iter().find(|x| x.name == name).unwrap().path).unwrap()).unwrap();
        let dx = read("DX");
        assert_eq!((dx.chans.len(), dx.bits, dx.mask), (6, 24, 0x3F));
        // Dialogue sits in the centre channel of the DX stem.
        let e = |c: &Vec<f32>| c.iter().map(|v| v * v).sum::<f32>();
        assert!(e(&dx.chans[2]) > 10.0 * (e(&dx.chans[0]) + 1e-6), "DX in C");
        // M&E carries no dialogue; the Spanish mix has the dub, not the original.
        let me = read("M&E");
        assert!(e(&me.chans[2]) < 1e-3 * e(&dx.chans[2]).max(1e-6), "M&E centre is empty");
        let dx_es = read("DX-es");
        assert!(e(&dx_es.chans[2]) > 0.0);
        let t = tracks(&p, 0, 1_000_000);
        assert_eq!(t.iter().map(|x| (x.lang.as_str(), x.layout.channels())).collect::<Vec<_>>(), vec![("eng", 2), ("eng", 6), ("spa", 2), ("spa", 6)]);
        assert_eq!(t[3].title, "Español (dub) 5.1");
        let (bed, objs) = export_adm(&p, &dir.join("mix.wav"), Layout::Surround51, "Mix test").unwrap();
        assert_eq!((bed, objs), (6, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
