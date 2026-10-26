// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

use studio_speech::check::{self, Input, Kind};
use studio_speech::coart::{self, Style};
use studio_speech::drive::{self, Mode};
use studio_speech::phoneme::{Ph, Timed};
use studio_speech::viseme::{Channel, Viseme};
use studio_speech::{WordIn, align, dict, features};

fn t(ph: Ph, s: f64, e: f64) -> Timed {
    Timed { ph, stress: if ph.is_vowel() { 1 } else { 0 }, start: s, end: e, word: Some(0) }
}

// ----------------------------------------------------------- pronunciations

#[test]
fn the_dictionary_and_the_rules_pronounce_words() {
    assert!(dict::size() > 100_000);
    let hello: Vec<Ph> = dict::pronounce("Hello").iter().map(|p| p.0).collect();
    assert_eq!(hello, vec![Ph::HH, Ph::AH, Ph::L, Ph::OW]);
    // Stress is kept.
    assert_eq!(dict::pronounce("hello")[3].1, 1);
    // Plurals of known words, numbers, and words by rule all give sounds.
    assert!(dict::pronounce("cats").last().unwrap().0 == Ph::S);
    let n42: Vec<Ph> = dict::pronounce("42").iter().map(|p| p.0).collect();
    assert!(n42.starts_with(&[Ph::F, Ph::AO, Ph::R]), "{n42:?}");
    let by_rule = dict::pronounce("zorblatting");
    assert!(by_rule.len() >= 7 && by_rule[0].0 == Ph::Z, "{by_rule:?}");
    let made: Vec<Ph> = studio_speech::lts::letters_to_sound("make").iter().map(|p| p.0).collect();
    assert_eq!(made, vec![Ph::M, Ph::EY, Ph::K]);
    assert!(dict::pronounce("...").is_empty());
}

// ------------------------------------------------- synthetic speech, known truth

fn buzz(out: &mut [f32], rate: u32, from: f64, to: f64, f0: f64, amp: f32, harmonics: u32) {
    for i in (from * rate as f64) as usize..((to * rate as f64) as usize).min(out.len()) {
        let tm = i as f64 / rate as f64;
        let env = ((tm - from) / 0.01).min((to - tm) / 0.01).clamp(0.0, 1.0) as f32;
        let mut s = 0.0;
        for h in 1..=harmonics {
            s += (std::f64::consts::TAU * f0 * h as f64 * tm).sin() / h as f64;
        }
        out[i] += amp * env * s as f32 * 0.5;
    }
}

fn hiss(out: &mut [f32], rate: u32, from: f64, to: f64, amp: f32, seed: &mut u64) {
    // High-passed noise: the difference of white noise.
    let mut prev = 0.0f32;
    for i in (from * rate as f64) as usize..((to * rate as f64) as usize).min(out.len()) {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        let w = ((*seed >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) as f32;
        out[i] += amp * (w - prev);
        prev = w;
    }
}

/// "map" then "sit": m (hum) a (vowel) p (closure, burst) | s (hiss) i t.
fn synthetic() -> (Vec<f32>, u32) {
    let rate = 16_000;
    let mut s = vec![0.0f32; rate as usize * 2];
    let mut seed = 99u64;
    buzz(&mut s, rate, 0.30, 0.38, 120.0, 0.12, 2); // M
    buzz(&mut s, rate, 0.38, 0.55, 120.0, 0.6, 12); // AE
    hiss(&mut s, rate, 0.62, 0.66, 0.5, &mut seed); // P burst
    hiss(&mut s, rate, 0.66, 0.70, 0.12, &mut seed); // aspiration
    hiss(&mut s, rate, 1.00, 1.14, 0.45, &mut seed); // S
    buzz(&mut s, rate, 1.14, 1.25, 120.0, 0.5, 10); // IH
    hiss(&mut s, rate, 1.32, 1.35, 0.4, &mut seed); // T burst
    (s, rate)
}

#[test]
fn phonemes_land_on_synthetic_speech_within_a_few_frames() {
    let (s, rate) = synthetic();
    let words = [WordIn::timed("map", 0.28, 0.72), WordIn::timed("sit", 0.98, 1.38)];
    let a = align(&s, rate, &words).unwrap();
    let find = |ph: Ph, w: usize| a.phones.iter().find(|p| p.ph == ph && p.word == Some(w)).copied().unwrap();
    let (m, ae, p) = (find(Ph::M, 0), find(Ph::AE, 0), find(Ph::P, 0));
    let (sp, ih) = (find(Ph::S, 1), find(Ph::IH, 1));
    assert!((m.start - 0.30).abs() < 0.05, "{m:?}");
    assert!((ae.start - 0.38).abs() < 0.05 && (ae.end - 0.55).abs() < 0.06, "{ae:?}");
    // The P's closure is the silence after the vowel, its burst at 0.62.
    assert!(p.start < 0.62 && p.end > 0.63, "{p:?}");
    assert!((sp.start - 1.00).abs() < 0.05 && (sp.end - 1.14).abs() < 0.05, "{sp:?}");
    assert!((ih.start - 1.14).abs() < 0.05, "{ih:?}");
    // In order, no overlaps, and silence between the words.
    assert!(a.phones.windows(2).all(|w| w[1].start >= w[0].end - 1e-9));
    assert!(a.phones.iter().any(|p| p.ph == Ph::Sil && p.start > 0.7 && p.end < 1.0));
    // Round trip through the text form.
    let back = studio_speech::Alignment::from_tsv(&a.to_tsv()).unwrap();
    assert_eq!(back.phones.len(), a.phones.len());
}

#[test]
fn words_without_times_are_found_in_the_audio() {
    let (s, rate) = synthetic();
    let a = align(&s, rate, &[WordIn::plain("map"), WordIn::plain("sit")]).unwrap();
    let w0 = a.words[0].unwrap();
    let w1 = a.words[1].unwrap();
    assert!(w0.0 > 0.2 && w0.1 < 0.85, "{w0:?}");
    assert!(w1.0 > 0.85 && w1.1 < 1.5, "{w1:?}");
}

// ------------------------------------------------------------- coarticulation

#[test]
fn closures_hold_two_frames_and_the_mouth_leads_the_sound() {
    // "mama": M a M a with short closures.
    let ph = [t(Ph::Sil, 0.0, 0.3), t(Ph::M, 0.3, 0.36), t(Ph::AA, 0.36, 0.52), t(Ph::M, 0.52, 0.57), t(Ph::AA, 0.57, 0.75), t(Ph::Sil, 0.75, 1.2)];
    let st = Style { extras: false, ..Style::default() };
    let c = coart::curves(&ph, 1.2, &st, &[]);
    for (s, e) in [(0.30, 0.36), (0.52, 0.57)] {
        // Shut at the sound of the plosive, and for >= 2 frames at 30 fps.
        let shut: Vec<f64> = (0..120).map(|i| i as f64 / 100.0).filter(|&tm| c.at(Channel::Close, tm) > 0.9).collect();
        let inside: Vec<&f64> = shut.iter().filter(|&&tm| tm > s - 0.08 && tm < e + 0.08).collect();
        assert!(inside.len() as f64 * 0.01 >= 2.0 / 30.0 - 0.011, "closure {s}: {} samples", inside.len());
        assert!(c.at(Channel::Close, (s + e) / 2.0 - st.lead / 2.0) > 0.9);
    }
    // Open for the vowel in between, with the jaw (not the lips) opening.
    assert!(c.at(Channel::Jaw, 0.44) > 0.6 && c.at(Channel::Close, 0.44) < 0.1);
    assert!(c.open(0.44) > 0.5 && c.open(0.28) < 0.2);
    // Anticipation: the jaw crosses half-way earlier by the lead.
    let cross = |lead: f64| {
        let c = coart::curves(&ph, 1.2, &Style { lead, extras: false, ..Style::default() }, &[]);
        (10..60).map(|i| i as f64 / 100.0).find(|&tm| c.at(Channel::Jaw, tm) > 0.4).unwrap()
    };
    let d = cross(0.0) - cross(0.08);
    assert!((d - 0.08).abs() < 0.02, "shifted by {d}");
    // Offset moves everything.
    let later = coart::curves(&ph, 1.2, &Style { offset: 0.1, extras: false, ..Style::default() }, &[]);
    assert!((later.at(Channel::Jaw, 0.54) - c.at(Channel::Jaw, 0.44)).abs() < 0.08);
    // Rest is shut.
    assert!(c.open(0.1) < 0.02 && c.open(1.1) < 0.02);
}

#[test]
fn fast_speech_is_not_over_articulated_and_visemes_follow_phonemes() {
    let slow: Vec<Timed> = vec![t(Ph::Sil, 0.0, 0.2), t(Ph::AA, 0.2, 0.5), t(Ph::Sil, 0.5, 0.9)];
    let fast: Vec<Timed> = vec![t(Ph::Sil, 0.0, 0.2), t(Ph::AA, 0.2, 0.24), t(Ph::Sil, 0.24, 0.9)];
    let st = Style { extras: false, ..Style::default() };
    let pk = |p: &[Timed]| coart::curves(p, 0.9, &st, &[]).channel(Channel::Jaw).iter().cloned().fold(0.0, f32::max);
    assert!(pk(&slow) > 0.85 && pk(&fast) < pk(&slow) * 0.8, "{} {}", pk(&slow), pk(&fast));
    // EE stretches, OO rounds, FV rolls the lip.
    let c = coart::curves(&[t(Ph::Sil, 0.0, 0.1), t(Ph::IY, 0.1, 0.4), t(Ph::UW, 0.4, 0.7), t(Ph::F, 0.7, 0.9), t(Ph::Sil, 0.9, 1.0)], 1.0, &st, &[]);
    assert!(c.at(Channel::Stretch, 0.19) > 0.65 && c.at(Channel::Pucker, 0.19) < 0.25);
    assert!(c.at(Channel::Pucker, 0.55) > 0.7 && c.at(Channel::Stretch, 0.55) < 0.2);
    assert!(c.at(Channel::Roll, 0.8) > 0.8);
    assert!(c.weight(Viseme::EE)[19] > 0.5 && c.weight(Viseme::WOO)[55] > 0.5);
    // Tongue: forward for TH, up for T.
    let c = coart::curves(&[t(Ph::Sil, 0.0, 0.1), t(Ph::TH, 0.1, 0.3), t(Ph::AH, 0.3, 0.5), t(Ph::T, 0.5, 0.6), t(Ph::Sil, 0.6, 0.9)], 0.9, &st, &[]);
    assert!(c.at(Channel::TongueFwd, 0.2) > 0.6 && c.at(Channel::TongueUp, 0.55) > 0.5);
    // Intensity and emotion scale it.
    let big = coart::curves(&slow, 0.9, &Style { intensity: 0.5, extras: false, ..Style::default() }, &[]);
    assert!(big.channel(Channel::Jaw).iter().cloned().fold(0.0, f32::max) < 0.6);
    let happy = coart::curves(&slow, 0.9, &Style { emotion: studio_speech::Emotion::Happy, extras: false, ..Style::default() }, &[]);
    assert!(happy.at(Channel::Smile, 0.3) > 0.3);
}

#[test]
fn blinks_brows_and_gaze_come_with_speech() {
    let ph = [t(Ph::Sil, 0.0, 0.3), t(Ph::AA, 0.3, 0.6), t(Ph::Sil, 0.6, 1.0), t(Ph::AA, 1.0, 1.3), t(Ph::Sil, 1.3, 10.0)];
    let c = coart::curves(&ph, 10.0, &Style::default(), &[]);
    let blinking = c.channel(Channel::Blink).iter().filter(|v| **v > 0.9).count();
    assert!(blinking > 5, "blinks: {blinking}");
    let gaze = c.channel(Channel::EyeX).iter().filter(|v| v.abs() > 0.02).count();
    assert!(gaze > 100);
    assert!(c.channel(Channel::EyeX).iter().all(|v| v.abs() <= 0.4));
}

// ---------------------------------------------------------------- 3D rigs

#[test]
fn the_head_base_and_other_namings_are_driven() {
    let ph = [t(Ph::Sil, 0.0, 0.2), t(Ph::B, 0.2, 0.28), t(Ph::AA, 0.28, 0.5), t(Ph::Sil, 0.5, 0.8)];
    let c = coart::curves(&ph, 0.8, &Style::default(), &[]);
    // Studio's own head: ARKit-named keys, separate jaw and lips.
    let names: Vec<String> = studio_mesh::face::NAMES.iter().map(|s| s.to_string()).collect();
    let d = drive::drive3(&c, &names, &[], 0.02);
    assert_eq!(d.mode, Some(Mode::Channels));
    let keys = |n: &str| d.shapes.iter().find(|s| s.0 == n).map(|s| s.1.clone()).unwrap_or_default();
    assert!(keys("jawOpen").iter().any(|k| k.1 > 0.7), "jaw");
    assert!(keys("mouthClose").iter().any(|k| k.1 > 0.9 && k.0 < 0.3), "closure on the b");
    assert!(keys("mouthStretchLeft").len() > 1 && keys("mouthStretchRight").len() > 1);
    assert!(d.shapes.iter().any(|s| s.0 == "eyeBlinkLeft"));
    // Fewer keys than samples.
    assert!(keys("jawOpen").len() < 30);
    // Other names, any case and separators.
    let other: Vec<String> = ["Mouth_Open", "MOUTH.CLOSE", "mouth_pucker", "Mouth_Stretch_L", "Mouth_Stretch_R", "Tongue_Up", "Tongue_Out"].iter().map(|s| s.to_string()).collect();
    let d = drive::drive3(&c, &other, &[], 0.02);
    assert_eq!(d.mode, Some(Mode::Channels));
    for n in ["Mouth_Open", "MOUTH.CLOSE", "mouth_pucker", "Mouth_Stretch_L", "Mouth_Stretch_R", "Tongue_Up", "Tongue_Out"] {
        assert!(d.shapes.iter().any(|s| s.0 == n), "{n} driven");
    }
    // One key per viseme (Oculus and VRM styles).
    let vis: Vec<String> = ["viseme_sil", "viseme_PP", "viseme_FF", "viseme_TH", "viseme_DD", "viseme_kk", "viseme_CH", "viseme_SS", "viseme_nn", "viseme_RR", "viseme_aa", "viseme_E", "viseme_I", "viseme_O", "viseme_U"].iter().map(|s| s.to_string()).collect();
    let d = drive::drive3(&c, &vis, &[], 0.02);
    assert_eq!(d.mode, Some(Mode::Visemes));
    let pp = d.shapes.iter().find(|s| s.0 == "viseme_PP").unwrap();
    let aa = d.shapes.iter().find(|s| s.0 == "viseme_aa").unwrap();
    assert!(pp.1.iter().any(|k| k.1 > 0.6 && k.0 < 0.3), "{:?}", pp.1);
    assert!(aa.1.iter().any(|k| k.1 > 0.4 && k.0 > 0.25 && k.0 < 0.5), "{:?}", aa.1);
    // Tongue bones and a jaw bone when there are no keys.
    let bones: Vec<String> = ["head", "jaw", "tongue", "tongue.tip", "eye.L", "eye.R"].iter().map(|s| s.to_string()).collect();
    let d = drive::drive3(&c, &[], &bones, 0.02);
    assert_eq!(d.mode, Some(Mode::Jaw));
    assert!(d.jaw.as_ref().unwrap().1.iter().any(|k| k.1 > 8.0));
    assert!(!d.tongue_bones.is_empty() && d.eye_bones.len() == 4);
    // Nothing to drive says so.
    let d = drive::drive3(&c, &["cube".to_string()], &["root".to_string()], 0.02);
    assert_eq!(d.mode, Some(Mode::None));
    assert!(!d.missing.is_empty());
}

// ------------------------------------------------------------------- 2D

#[test]
fn a_puppet_mouth_and_a_mouth_chart_follow_the_curves() {
    let ph = [t(Ph::Sil, 0.0, 0.2), t(Ph::M, 0.2, 0.3), t(Ph::AA, 0.3, 0.5), t(Ph::IY, 0.5, 0.7), t(Ph::UW, 0.7, 0.9), t(Ph::Sil, 0.9, 1.3)];
    let c = coart::curves(&ph, 1.3, &Style { extras: false, ..Style::default() }, &[]);
    let (open, wide) = drive::pose2d(&c, 0.33);
    assert!(open > 0.6 && wide.abs() < 0.4);
    assert!(drive::pose2d(&c, 0.52).1 > 0.4, "ee is wide");
    assert!(drive::pose2d(&c, 0.74).1 < -0.3, "oo is round");
    assert!(drive::pose2d(&c, 0.22).0 < 0.15, "m is shut");
    let keys = drive::pose_keys(&c, 30.0);
    assert!(keys.windows(2).all(|w| w[1].0 - w[0].0 > 1.0 / 30.0 * 0.9));
    // A full chart and a small one.
    let full = [Viseme::Rest, Viseme::MBP, Viseme::AA, Viseme::EE, Viseme::WOO, Viseme::Etc];
    let seq = drive::chart_sequence(&c, &full, 24.0);
    let shown = |tm: f64| seq.iter().rev().find(|s| s.0 <= tm).map(|s| full[s.1]).unwrap();
    assert_eq!(shown(0.05), Viseme::Rest);
    assert_eq!(shown(0.23), Viseme::MBP);
    assert_eq!(shown(0.40), Viseme::AA);
    assert_eq!(shown(0.8), Viseme::WOO);
    // The closure stays two frames at least.
    let i = seq.iter().position(|s| full[s.1] == Viseme::MBP).unwrap();
    assert!(seq[i + 1].0 - seq[i].0 >= 2.0 / 24.0 - 1e-6, "{seq:?}");
    // A four-drawing chart still gets something for every sound.
    let small = [Viseme::Rest, Viseme::MBP, Viseme::AA, Viseme::OH];
    let seq = drive::chart_sequence(&c, &small, 24.0);
    assert!(seq.iter().all(|s| s.1 < 4));
    assert_eq!(Viseme::named("MBP"), Some(Viseme::MBP));
    assert_eq!(Viseme::named("mouth_open"), None);
    assert_eq!(Viseme::named("closed"), Some(Viseme::Rest));
}

// ------------------------------------------------------------- the check

/// A loudness that never repeats within a second, so a shift has one answer.
fn env(tm: f64) -> f64 {
    0.5 + 0.25 * (std::f64::consts::TAU * tm / 0.19).sin() + 0.25 * (std::f64::consts::TAU * tm / 0.31 + 1.0).sin()
}

fn fake_audio(speech: &[(f64, f64)], total: f64) -> features::Frames {
    let n = (total * 100.0) as usize;
    let mut f = features::Frames::default();
    for i in 0..n {
        let tm = i as f64 / 100.0;
        let on = speech.iter().find(|s| tm >= s.0 && tm < s.1);
        let l = on.map_or(0.0, |_| env(tm));
        f.loud.push(l as f32);
        f.hf.push(0.1);
        f.lf.push(0.3);
        f.voice.push(if on.is_some() { 0.8 } else { 0.0 });
        f.db.push(-40.0 + 40.0 * l as f32);
    }
    f
}

fn track(f: impl Fn(f64) -> f32, total: f64) -> Vec<f32> {
    (0..(total * 100.0) as usize).map(|i| f(i as f64 / 100.0)).collect()
}

#[test]
fn the_check_flags_bad_sync_with_fixes_and_passes_good_sync() {
    let speech = [(0.5, 2.5)];
    let audio = fake_audio(&speech, 3.5);
    let phones = vec![t(Ph::Sil, 0.0, 0.5), t(Ph::B, 0.5, 0.58), t(Ph::AA, 0.58, 2.5), t(Ph::Sil, 2.5, 3.5)];
    let good_open = |shift: f64| move |tm: f64| {
        let u = tm - shift;
        if (0.5..2.5).contains(&u) { env(u) as f32 * 0.8 } else { 0.0 }
    };
    let run = |open: &[f32], close: Option<&[f32]>| check::check(&Input { open, close, audio: &audio, phones: &phones, fps: 30.0, lead: 0.0 });
    // Good: in step, with the b closed at the start.
    let open = track(|tm| if tm < 0.58 { 0.0 } else { good_open(0.0)(tm) }, 3.5);
    let r = run(&open, None);
    assert!(r.issues.is_empty(), "{:?}", r.issues);
    // Late by 5 frames: out of sync, with the shift that fixes it.
    let late = track(|tm| if tm < 0.74 { 0.0 } else { good_open(0.1667)(tm) }, 3.5);
    let r = run(&late, None);
    let o = r.issues.iter().find(|i| i.kind == Kind::OutOfSync).unwrap_or_else(|| panic!("{:?}", r));
    assert!((o.shift.unwrap() + 0.1667).abs() < 0.05, "{:?}", o);
    assert!(o.fix.contains("offset"));
    // Open in silence.
    let noisy = track(|tm| if tm < 0.58 { 0.0 } else if (2.8..3.3).contains(&tm) { 0.6 } else { good_open(0.0)(tm) }, 3.5);
    assert!(run(&noisy, None).issues.iter().any(|i| i.kind == Kind::MovesInSilence && i.t0 > 2.7 && i.t0 < 3.0));
    // Still while speaking.
    let still = track(|tm| if (0.6..2.4).contains(&tm) { 0.4 } else { 0.0 }, 3.5);
    assert!(run(&still, None).issues.iter().any(|i| i.kind == Kind::StillWhileSpeaking));
    // The b never closes.
    let no_close = track(|tm| if tm < 0.4 { 0.0 } else { 0.3 + good_open(0.0)(tm) }, 3.5);
    let r = run(&no_close, None);
    let m = r.issues.iter().find(|i| i.kind == Kind::MissedClosure).unwrap();
    assert!((m.t0 - 0.5).abs() < 0.01 && !m.fix.is_empty());
    // A one-frame blink of closure is too short.
    let close = track(|tm| if (0.5..0.52).contains(&tm) { 1.0 } else { 0.0 }, 3.5);
    let open2 = track(|tm| if tm < 0.4 { 0.0 } else { 0.3 + good_open(0.0)(tm) }, 3.5);
    assert!(run(&open2, Some(&close)).issues.iter().any(|i| i.kind == Kind::ShortClosure));
}

// ------------------------------------------- real speech from the Mac voice

fn read_wav16(path: &std::path::Path) -> Option<(Vec<f32>, u32)> {
    let b = std::fs::read(path).ok()?;
    if b.len() < 44 || &b[0..4] != b"RIFF" {
        return None;
    }
    let mut i = 12;
    let (mut rate, mut chans) = (22_050u32, 1u16);
    while i + 8 <= b.len() {
        let id = &b[i..i + 4];
        let len = u32::from_le_bytes(b[i + 4..i + 8].try_into().ok()?) as usize;
        if id == b"fmt " {
            chans = u16::from_le_bytes(b[i + 10..i + 12].try_into().ok()?);
            rate = u32::from_le_bytes(b[i + 12..i + 16].try_into().ok()?);
        } else if id == b"data" {
            let d = &b[i + 8..(i + 8 + len).min(b.len())];
            let s: Vec<f32> = d.chunks_exact(2 * chans as usize).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect();
            return Some((s, rate));
        }
        i += 8 + len + (len & 1);
    }
    None
}

fn say(text: &str, name: &str) -> Option<(Vec<f32>, u32)> {
    let dir = std::env::temp_dir().join("studio-speech-tests");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{name}.wav"));
    let ok = std::process::Command::new("say").args(["-o"]).arg(&path).args(["--data-format=LEI16@22050", text]).status().ok()?.success();
    ok.then(|| read_wav16(&path)).flatten()
}

#[test]
fn phonemes_and_closures_land_on_a_real_voice() {
    let text = "Peter picked a bit of buttery maple bread by the bay";
    let Some((s, rate)) = say(text, "bread") else {
        eprintln!("no `say` voice here; skipped");
        return;
    };
    let words: Vec<WordIn> = text.split(' ').map(WordIn::plain).collect();
    let a = align(&s, rate, &words).unwrap();
    let dur = s.len() as f64 / rate as f64;
    // Every word got sounds, in order, inside the audio.
    assert!(a.words.iter().all(|w| w.is_some()));
    assert!(a.phones.windows(2).all(|w| w[1].start >= w[0].end - 1e-9));
    assert!(a.end() <= dur + 0.05);
    // Word spans move forward.
    let spans: Vec<(f64, f64)> = a.words.iter().map(|w| w.unwrap()).collect();
    assert!(spans.windows(2).all(|w| w[1].0 >= w[0].0));
    // Closures sit in quiet: the frame loudness inside the first half of
    // each P/B is under what the neighbouring vowels reach.
    let f = features::analyse(&s, rate);
    let mut quiet = 0;
    let mut total = 0;
    for (i, p) in a.phones.iter().enumerate() {
        if !matches!(p.ph, Ph::P | Ph::B) || p.dur() < 0.025 {
            continue;
        }
        let vowel_peak = a.phones[i.saturating_sub(2)..(i + 3).min(a.phones.len())].iter().filter(|q| q.ph.is_vowel()).map(|q| (q.start * 100.0) as usize..(q.end * 100.0) as usize).flat_map(|r| r).filter_map(|k| f.loud.get(k)).cloned().fold(0.0, f32::max);
        let (s0, s1) = ((p.start * 100.0) as usize, ((p.start + 0.6 * p.dur()) * 100.0) as usize + 1);
        let mean = f.loud[s0..s1.min(f.loud.len())].iter().sum::<f32>() / (s1 - s0).max(1) as f32;
        total += 1;
        if mean < vowel_peak * 0.75 {
            quiet += 1;
        }
    }
    assert!(total >= 5 && quiet as f64 >= 0.7 * total as f64, "{quiet}/{total} closures in quiet");
    // The mouth: lips close on every M/B/P and open on the vowels.
    let st = Style::default();
    let c = coart::curves(&a.phones, dur, &st, &f.loud);
    for p in a.phones.iter().filter(|p| p.ph.is_bilabial_stop()) {
        let peak = (0..30).map(|k| c.at(Channel::Close, p.start - st.lead + k as f64 * (p.dur() + st.lead) / 30.0)).fold(0.0, f32::max);
        assert!(peak > 0.85, "{} at {:.2}s closes to {peak}", p.ph.name(), p.start);
    }
    // And the check passes the mouth made from the alignment itself.
    let open: Vec<f32> = (0..(dur * 100.0) as usize).map(|i| c.open(i as f64 / 100.0)).collect();
    let close: Vec<f32> = (0..open.len()).map(|i| c.at(Channel::Close, i as f64 / 100.0)).collect();
    let r = check::check(&Input { open: &open, close: Some(&close), audio: &f, phones: &a.phones, fps: 30.0, lead: st.lead });
    let bad: Vec<_> = r.issues.iter().filter(|i| i.kind != Kind::ShortClosure).collect();
    assert!(bad.is_empty(), "{bad:?}");
}
