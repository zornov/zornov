// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Turning channels into what each kind of character has: shape keys and
//! bones of a 3D face (found through a case-insensitive alias map, so any
//! reasonable naming works), a pose for a 2D puppet's mouth pins, or a
//! sequence of drawings from a mouth chart.

use crate::coart::{self, Curves, RATE};
use crate::viseme::{Channel, VISEMES, Viseme};

/// A name with case, separators and a few noise words removed:
/// `Mouth_Open`, `mouthOpen` and `mouth.open` all read `mouthopen`.
pub fn norm(s: &str) -> String {
    let mut k: String = s.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    for noise in ["blendshape", "blendshapes", "shapekey", "key", "morph", "bs"] {
        if let Some(r) = k.strip_prefix(noise).filter(|r| r.len() >= 3) {
            k = r.to_owned();
        }
    }
    k
}

/// A side marker at the end of a name, folded to `left` / `right`.
fn sided(k: &str) -> String {
    for (suf, to) in [("left", "left"), ("right", "right")] {
        if k.ends_with(suf) {
            return format!("{}{}", &k[..k.len() - suf.len()], to);
        }
    }
    k.to_owned()
}

/// The names (as given) that read as any of `cands`: whole-name matches
/// first; a candidate ending in `*` matches names that start with it
/// (so `mouthstretch*` finds both sides).
fn find(names: &[String], cands: &[&str]) -> Vec<String> {
    let normed: Vec<(String, &String)> = names.iter().map(|n| (sided(&norm(n)), n)).collect();
    let mut out: Vec<String> = Vec::new();
    for c in cands {
        if let Some(stem) = c.strip_suffix('*') {
            for (k, n) in &normed {
                if k.starts_with(stem) && !out.contains(*n) {
                    out.push((*n).clone());
                }
            }
        } else {
            for (k, n) in &normed {
                if k == c && !out.contains(*n) {
                    out.push((*n).clone());
                }
            }
        }
        // First candidate that finds anything wins (names are listed best first).
        if !out.is_empty() && !c.ends_with('*') {
            break;
        }
        if !out.is_empty() {
            break;
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Separate jaw, closure, pucker, stretch… keys (the ARKit way).
    Channels,
    /// One key per viseme (the Oculus / VRM way).
    Visemes,
    /// A jaw only (an animal, a stylised thing).
    Jaw,
    /// Nothing to drive.
    None,
}

/// Keys for a 3D character: `(shape key name, keys of (time, weight))`,
/// and bone turns `(bone, axis-free amount in degrees, keys)`.
#[derive(Clone, Debug, Default)]
pub struct Drive3 {
    pub mode: Option<Mode>,
    pub shapes: Vec<(String, Vec<(f64, f32)>)>,
    /// The jaw bone, when the rig has one to drive (opening, degrees).
    pub jaw: Option<(String, Vec<(f64, f32)>)>,
    /// Bones for the tongue: `(bone, which, keys)` where `which` is
    /// "up", "down", "forward" or "curl" (degrees, positive is that way).
    pub tongue_bones: Vec<(String, &'static str, Vec<(f64, f32)>)>,
    /// Eye bones with gaze `(bone, "x" | "y", keys in degrees)`.
    pub eye_bones: Vec<(String, &'static str, Vec<(f64, f32)>)>,
    /// What this rig lacks, in words (so the AI can say so).
    pub missing: Vec<&'static str>,
}

fn viseme_names(v: Viseme) -> &'static [&'static str] {
    match v {
        Viseme::Rest => &["visemesil", "sil"],
        Viseme::MBP => &["visemepp", "pp", "mbp", "visemembp", "visemem"],
        Viseme::FV => &["visemeff", "ff", "fv", "visemefv"],
        Viseme::TH => &["visemeth", "th"],
        Viseme::L => &["visemel", "visemenn", "nn", "l"],
        Viseme::R => &["visemerr", "rr", "visemer", "r"],
        Viseme::WOO => &["visemeu", "ou", "u", "oo", "visemeoo", "visemewoo"],
        Viseme::CHSHJ => &["visemech", "ch", "visemesh", "sh", "j"],
        Viseme::SZ => &["visemess", "ss", "visemes", "s"],
        Viseme::AA => &["visemeaa", "aa", "visemea", "a"],
        Viseme::AE => &["visemee", "e", "ee", "visemeae", "ae", "eh"],
        Viseme::AH => &["visemeah", "ah", "visemeaa", "aa"],
        Viseme::EE => &["visemei", "i", "ih", "visemeee"],
        Viseme::IH => &["visemeih", "ih", "visemei", "i"],
        Viseme::OH => &["visemeo", "oh", "o", "visemeoh"],
        Viseme::UH => &["visemeu", "ou", "u", "visemeuh"],
        Viseme::ER => &["visemerr", "rr", "er"],
        Viseme::Etc => &["visemedd", "dd", "visemekk", "kk", "visemenn", "nn", "etc"],
    }
}

fn k(v: &[f32], tol: f32) -> Vec<(f64, f32)> {
    coart::keys(v, tol)
}

fn scaled(v: &[f32], m: f32) -> Vec<f32> {
    v.iter().map(|x| (x * m).clamp(0.0, 1.0)).collect()
}

fn lerp_curves(a: &[f32], b: &[f32], t: f32) -> Vec<f32> {
    a.iter().zip(b).map(|(x, y)| x + (y - x) * t).collect()
}

/// Keys for a 3D rig with these shape key and bone names. `tol` is the
/// largest error a dropped key may leave (0.02 is invisible).
pub fn drive3(c: &Curves, shapes: &[String], bones: &[String], tol: f32) -> Drive3 {
    let mut d = Drive3::default();
    let ch = |x: Channel| c.channel(x);
    let jaw_shape = find(shapes, &["jawopen", "mouthopen", "jawdrop", "jaw"]);
    let close = find(shapes, &["mouthclose", "lipsclosed", "mouthclosed", "lipspressed", "lipsclose"]);
    let pucker = find(shapes, &["mouthpucker", "pucker", "lipspucker", "mouthround", "lipsround"]);
    let funnel = find(shapes, &["mouthfunnel", "funnel"]);
    let stretch = find(shapes, &["mouthstretch*", "mouthwide", "widemouth"]);
    let roll = find(shapes, &["mouthrolllower", "lowerliprollin", "lowerlipin"]);
    let upper = find(shapes, &["mouthupperup*", "teethupper", "upperteeth"]);
    let channel_hits = [!jaw_shape.is_empty(), !close.is_empty(), !pucker.is_empty() || !funnel.is_empty(), !stretch.is_empty()].iter().filter(|b| **b).count();
    let viseme_hits = VISEMES.iter().filter(|v| **v != Viseme::Rest && !find(shapes, viseme_names(**v)).is_empty()).count();
    let put = |names: Vec<String>, v: Vec<f32>, d: &mut Drive3| {
        let keys = k(&v, tol);
        for n in names {
            d.shapes.push((n, keys.clone()));
        }
    };
    let jaw_bone = bones.iter().find(|b| {
        let n = norm(b);
        n.contains("jaw") || n.contains("mandible")
    });
    if channel_hits >= 2 {
        d.mode = Some(Mode::Channels);
        put(jaw_shape.clone(), ch(Channel::Jaw).to_vec(), &mut d);
        put(close, ch(Channel::Close).to_vec(), &mut d);
        put(pucker.clone(), ch(Channel::Pucker).to_vec(), &mut d);
        put(funnel, scaled(ch(Channel::Pucker), 0.6), &mut d);
        put(stretch, ch(Channel::Stretch).to_vec(), &mut d);
        put(roll, ch(Channel::Roll).to_vec(), &mut d);
        put(upper, scaled(ch(Channel::Teeth), 0.5), &mut d);
        if pucker.is_empty() {
            d.missing.push("lip pucker/round shape");
        }
    } else if viseme_hits >= 5 {
        d.mode = Some(Mode::Visemes);
        // Several visemes share a key on some rigs: take the strongest.
        let mut by_key: Vec<(String, Vec<f32>)> = Vec::new();
        for v in VISEMES {
            let names = find(shapes, viseme_names(v));
            let Some(n) = names.first() else { continue };
            let w = c.weight(v).to_vec();
            match by_key.iter_mut().find(|(m, _)| m == n) {
                Some((_, o)) => o.iter_mut().zip(&w).for_each(|(a, b)| *a = a.max(*b)),
                None => by_key.push((n.clone(), w)),
            }
        }
        for (n, w) in by_key {
            put(vec![n], w, &mut d);
        }
    } else if jaw_shape.is_empty() && jaw_bone.is_none() {
        d.mode = Some(Mode::None);
        d.missing.push("any mouth shape keys or a jaw bone");
    } else {
        d.mode = Some(Mode::Jaw);
    }
    // The jaw bone, when no shape key already opens the mouth.
    if let Some(b) = jaw_bone
        && jaw_shape.is_empty()
        && d.mode != Some(Mode::Visemes)
    {
        let open: Vec<f32> = if d.mode == Some(Mode::Jaw) {
            // An animal: the jaw does the talking, softened.
            let j = ch(Channel::Jaw);
            let cl = ch(Channel::Close);
            let round = ch(Channel::Pucker);
            smooth((0..j.len()).map(|i| (j[i] * (1.0 - 0.9 * cl[i]) * 0.85 + 0.1 * round[i]).clamp(0.0, 1.0)).collect(), 3)
        } else {
            ch(Channel::Jaw).iter().zip(ch(Channel::Close)).map(|(j, cl)| j * (1.0 - 0.6 * cl)).collect()
        };
        let max_deg = if d.mode == Some(Mode::Jaw) { 22.0 } else { 16.0 };
        d.jaw = Some((b.clone(), k(&open, tol).into_iter().map(|(t, v)| (t, v * max_deg)).collect()));
    }
    // The tongue: shape keys or bones, whatever the rig has.
    for (chan, cands, which) in [
        (Channel::TongueUp, &["tongueup", "tongueraise", "tongueraised", "tonguetipup"][..], "up"),
        (Channel::TongueDown, &["tonguedown", "tonguelower", "tonguelowered"][..], "down"),
        (Channel::TongueFwd, &["tongueout", "tongueforward", "tongueforwards", "tonguefwd", "tonguetipout"][..], "forward"),
        (Channel::TongueCurl, &["tonguecurl", "tonguecurled", "tonguecurve", "tongueroll", "tongueback"][..], "curl"),
    ] {
        let names = find(shapes, cands);
        if names.is_empty() {
            continue;
        }
        put(names, ch(chan).to_vec(), &mut d);
        let _ = which;
    }
    let has_tongue_shape = d.shapes.iter().any(|(n, _)| norm(n).contains("tongue"));
    if !has_tongue_shape {
        // Bones: a tongue bone (or tip) turned by the same channels.
        let tb: Vec<&String> = bones.iter().filter(|b| norm(b).contains("tongue")).collect();
        for (i, b) in tb.iter().enumerate() {
            let (up, down, fwd, curl) = (ch(Channel::TongueUp), ch(Channel::TongueDown), ch(Channel::TongueFwd), ch(Channel::TongueCurl));
            // The base bone raises and lowers; the tip bones curl.
            let tip = i > 0;
            let lift: Vec<f32> = (0..up.len()).map(|j| (up[j] - down[j]).clamp(-1.0, 1.0) * if tip { 20.0 } else { 18.0 }).collect();
            d.tongue_bones.push(((*b).clone(), "up", k(&lift, tol * 20.0)));
            d.tongue_bones.push(((*b).clone(), "forward", k(fwd, tol)));
            d.tongue_bones.push(((*b).clone(), "curl", k(curl, tol).into_iter().map(|(t, v)| (t, v * if tip { 35.0 } else { 15.0 })).collect()));
        }
        if tb.is_empty() && matches!(d.mode, Some(Mode::Channels | Mode::Visemes)) {
            d.missing.push("tongue (no tongue keys or bones)");
        }
    }
    // Expression and eyes.
    let blink = find(shapes, &["eyeblink*", "blink", "blinkleft", "blinkright", "eyesclosed", "eyeclosed*"]);
    put(blink, ch(Channel::Blink).to_vec(), &mut d);
    let brow_up = find(shapes, &["browinnerup", "browup", "browsup"]);
    put(brow_up, ch(Channel::BrowUp).to_vec(), &mut d);
    let outer = find(shapes, &["browouterup*"]);
    put(outer, scaled(ch(Channel::BrowUp), 0.7), &mut d);
    let brow_down = find(shapes, &["browdown*", "browsdown"]);
    put(brow_down, ch(Channel::BrowDown).to_vec(), &mut d);
    if ch(Channel::Smile).iter().any(|v| *v > 0.01) {
        let sm = find(shapes, &["mouthsmile*", "smile"]);
        put(sm, ch(Channel::Smile).to_vec(), &mut d);
    }
    if ch(Channel::Frown).iter().any(|v| *v > 0.01) {
        let fr = find(shapes, &["mouthfrown*", "frown"]);
        put(fr, ch(Channel::Frown).to_vec(), &mut d);
    }
    // Eyes: gaze bones (a few degrees), else look keys.
    let eyes: Vec<&String> = bones.iter().filter(|b| {
        let n = norm(b);
        n.contains("eye") && !n.contains("lid") && !n.contains("brow") && !n.contains("lash")
    }).collect();
    for b in eyes {
        let gx: Vec<(f64, f32)> = k(ch(Channel::EyeX), tol).into_iter().map(|(t, v)| (t, v * 10.0)).collect();
        let gy: Vec<(f64, f32)> = k(ch(Channel::EyeY), tol).into_iter().map(|(t, v)| (t, v * 10.0)).collect();
        d.eye_bones.push((b.clone(), "x", gx));
        d.eye_bones.push((b.clone(), "y", gy));
    }
    d
}

/// A moving average over `n` samples either side.
pub fn smooth(v: Vec<f32>, n: usize) -> Vec<f32> {
    (0..v.len())
        .map(|i| {
            let (a, b) = (i.saturating_sub(n), (i + n + 1).min(v.len()));
            v[a..b].iter().sum::<f32>() / (b - a) as f32
        })
        .collect()
}

/// The pose a 2D puppet's mouth pins take: how open (0..1) and its shape
/// (-1 rounded … 1 stretched).
pub fn pose2d(c: &Curves, t: f64) -> (f64, f64) {
    let open = c.open(t) as f64;
    let wide = c.at(Channel::Stretch, t) as f64 - c.at(Channel::Pucker, t) as f64 + 0.3 * c.at(Channel::Smile, t) as f64;
    (open.clamp(0.0, 1.0), wide.clamp(-1.0, 1.0))
}

/// Poses at the moments something changes, for a puppet's mouth pins:
/// `(time, open, wide)`, at least a frame apart.
pub fn pose_keys(c: &Curves, fps: f64) -> Vec<(f64, f64, f64)> {
    let n = c.len;
    let open: Vec<f32> = (0..n).map(|i| c.open(i as f64 / RATE)).collect();
    let wide: Vec<f32> = (0..n).map(|i| pose2d(c, i as f64 / RATE).1 as f32).collect();
    let mut ts: Vec<f64> = coart::keys(&open, 0.03).into_iter().map(|k| k.0).chain(coart::keys(&wide, 0.06).into_iter().map(|k| k.0)).collect();
    ts.sort_by(f64::total_cmp);
    let gap = 1.0 / fps.max(1.0) * 0.99;
    ts.dedup_by(|b, a| *b - *a < gap);
    ts.into_iter().map(|t| {
        let (o, w) = pose2d(c, t);
        (t, o, w)
    }).collect()
}

/// Which drawing of a mouth chart shows when: `(time, chart index)`,
/// held. `chart` lists the visemes the character has drawings for, in
/// the order of the caller's own list; a missing viseme falls back to
/// its nearest. Every drawing stays at least a frame (a closure two),
/// at `fps`.
pub fn chart_sequence(c: &Curves, chart: &[Viseme], fps: f64) -> Vec<(f64, usize)> {
    if chart.is_empty() {
        return Vec::new();
    }
    let pick = |v: Viseme| -> usize { v.fallbacks().iter().find_map(|f| chart.iter().position(|x| x == f)).unwrap_or(0) };
    let frame = 1.0 / fps.max(1.0);
    let end = c.duration();
    let mut seq: Vec<(f64, Viseme)> = Vec::new();
    let mut t = 0.0;
    while t <= end {
        // Sample the middle of the frame.
        let m = t + frame / 2.0;
        let i = ((m * RATE) as usize).min(c.len - 1);
        let v = if c.data[Channel::Close.index()][i] > 0.55 {
            Viseme::MBP
        } else if c.data[Channel::Jaw.index()][i] < 0.05 && c.data[Channel::Pucker.index()][i] < 0.1 && c.data[Channel::Stretch.index()][i] < 0.1 {
            Viseme::Rest
        } else {
            VISEMES.iter().copied().filter(|v| *v != Viseme::Rest && *v != Viseme::MBP).max_by(|a, b| c.weights[a.index()][i].total_cmp(&c.weights[b.index()][i])).unwrap_or(Viseme::Rest)
        };
        if seq.last().is_none_or(|l| l.1 != v) {
            seq.push((t, v));
        }
        t += frame;
    }
    // A drawing that lasts one frame is skipped unless it is a closure
    // (closures are held two frames).
    let mut out: Vec<(f64, Viseme)> = Vec::new();
    for (i, &(t, v)) in seq.iter().enumerate() {
        let next = seq.get(i + 1).map_or(end + frame, |n| n.0);
        let need = if v == Viseme::MBP { 2.0 * frame } else { frame };
        if next - t + 1e-9 < need && v != Viseme::MBP && out.last().is_some() && next - t < frame * 1.5 && seq.len() > i + 1 {
            // Too brief to read: let the previous drawing run on.
            if v != Viseme::Rest {
                continue;
            }
        }
        out.push((t, v));
    }
    // Hold closures two frames: push the next change later.
    for i in 0..out.len() {
        if out[i].1 == Viseme::MBP && i + 1 < out.len() && out[i + 1].0 - out[i].0 < 2.0 * frame - 1e-9 {
            out[i + 1].0 = out[i].0 + 2.0 * frame;
        }
    }
    out.dedup_by(|b, a| b.1 == a.1);
    let mut res: Vec<(f64, usize)> = Vec::new();
    let mut last_t = f64::NEG_INFINITY;
    for (t, v) in out {
        if t <= last_t {
            continue;
        }
        last_t = t;
        let idx = pick(v);
        if res.last().is_none_or(|l| l.1 != idx) {
            res.push((t, idx));
        }
    }
    res
}

/// A mixture of two curves (for easing between emotions, say).
pub fn blend(a: &[f32], b: &[f32], t: f32) -> Vec<f32> {
    lerp_curves(a, b, t)
}

/// The keys and bone that make a rig's mouth open and its lips close, for
/// reading a mouth back (the sync check) as well as driving it.
#[derive(Clone, Debug, Default)]
pub struct OpenNames {
    pub jaw: Vec<String>,
    pub close: Vec<String>,
    pub jaw_bone: Option<String>,
    /// One key per viseme, when that is how the rig is built.
    pub visemes: Vec<(Viseme, String)>,
}

pub fn open_names(shapes: &[String], bones: &[String]) -> OpenNames {
    let jaw = find(shapes, &["jawopen", "mouthopen", "jawdrop", "jaw"]);
    let close = find(shapes, &["mouthclose", "lipsclosed", "mouthclosed", "lipspressed", "lipsclose"]);
    let jaw_bone = bones.iter().find(|b| {
        let n = norm(b);
        n.contains("jaw") || n.contains("mandible")
    }).cloned();
    let mut visemes = Vec::new();
    if jaw.is_empty() {
        for v in VISEMES {
            if let Some(n) = find(shapes, viseme_names(v)).first() {
                visemes.push((v, n.clone()));
            }
        }
    }
    OpenNames { jaw, close, jaw_bone, visemes }
}
