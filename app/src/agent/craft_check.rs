// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Craft mistakes `check` can name from the edit itself: things hidden
//! behind other things, footage blown up past its pixels, and overdoing
//! (effects piled on, a transition on every cut, flashes, a sound on every
//! beat, too many fonts and colours). Each is a habit the AI editors fell
//! into; naming it with the clip and the fix is what stops it.

use super::motion_check::Problem;
use crate::view::{self, ClipKind, SEC};
use studio_model as m;

fn secs(t: i64) -> f64 {
    t as f64 / SEC as f64
}

/// Whether `c` paints the whole frame, opaque, as drawn at rest: a flat or
/// gradient card without a size, or footage that fills the frame.
fn covers_frame(c: &m::Clip, view: &view::Project) -> bool {
    let t = &c.transform;
    let plain = c.enabled
        && c.blend == m::BlendMode::Normal
        && c.masks.is_empty()
        && (t.opacity - 1.0).abs() < 1e-6
        && !c.keyframes.keys().any(|k| k.starts_with("transform."))
        && t.position.x.abs() < 1.0
        && t.position.y.abs() < 1.0
        && t.rotation.abs() < 0.01
        && t.scale_x >= 0.999
        && t.scale_y >= 0.999
        && t.crop.left + t.crop.right + t.crop.top + t.crop.bottom < 1e-6
        && c.motion.entry.is_none()
        && c.motion.exit.is_none();
    if !plain {
        return false;
    }
    match &c.source {
        m::ClipSource::Generator(g) => matches!(g.kind, m::GeneratorKind::Solid | m::GeneratorKind::Gradient) && !matches!(g.params.get("colour"), Some(m::Value::Colour(k)) if k.a < 0.999),
        m::ClipSource::Media(r) => view.media(r.media.0 as u32).is_some_and(|mi| mi.kind == view::MediaKind::Video && mi.width as u64 * view.height as u64 == mi.height as u64 * view.width as u64),
        _ => false,
    }
}

/// Luminance of a flat card's colour, if it is one.
fn card_brightness(c: &m::Clip) -> Option<f64> {
    let m::ClipSource::Generator(g) = &c.source else { return None };
    if g.kind != m::GeneratorKind::Solid {
        return None;
    }
    let Some(m::Value::Colour(k)) = g.params.get("colour") else { return None };
    Some(0.2126 * k.r + 0.7152 * k.g + 0.0722 * k.b)
}

pub fn problems(model: &m::Project, view: &view::Project) -> Vec<Problem> {
    let mut out = Vec::new();
    let Some(seq) = model.sequence(model.active_sequence) else { return out };
    let idx = |l: m::LaneId| seq.lane_index(l).unwrap_or(usize::MAX);
    let visual = |c: &m::Clip| seq.lanes.iter().find(|l| l.id == c.lane).is_some_and(|l| l.kind != m::LaneKind::Audio && !l.hidden) && c.enabled;

    // Hidden: an opaque full-frame layer above covers it for all its time.
    for c in seq.clips.iter().filter(|c| visual(c) && !matches!(c.source, m::ClipSource::Gap)) {
        if let m::ClipSource::Generator(g) = &c.source {
            if g.kind == m::GeneratorKind::Adjustment {
                continue;
            }
        }
        let cover = seq.clips.iter().find(|o| o.id != c.id && visual(o) && idx(o.lane) < idx(c.lane) && o.start <= c.start && o.end() >= c.end() && covers_frame(o, view));
        if let Some(o) = cover {
            out.push(Problem {
                kind: "hidden_layer",
                at: secs(c.start),
                clip: c.id.0 as u32,
                problem: format!("clip {} ({}) can never be seen: clip {} ({}) on a lane above covers the whole frame for all of its time", c.id.0, c.name, o.id.0, o.name),
                fix: "lanes higher up draw over lower ones: put the background lowest (restack it to: bottom, or onto the main lane) and what sits on it above (restack with in_front_of), or give the cover a size, a mask or less opacity",
            });
        }
    }

    // A full-frame card (a fade, a cover) with 3D models on both sides of it
    // splits the 3D scene: the models above it show through what was meant
    // to cover the whole world. Opacity keys don't matter here: a fade is
    // exactly the card this is about.
    let is_model = |c: &m::Clip| matches!(&c.source, m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Model);
    let full_card = |c: &m::Clip| {
        let t = &c.transform;
        matches!(&c.source, m::ClipSource::Generator(g) if matches!(g.kind, m::GeneratorKind::Solid | m::GeneratorKind::Gradient))
            && c.blend == m::BlendMode::Normal
            && c.masks.is_empty()
            && t.position.x.abs() < 1.0
            && t.position.y.abs() < 1.0
            && t.scale_x >= 0.999
            && t.scale_y >= 0.999
    };
    let overlap = |a: &m::Clip, b: &m::Clip| a.start < b.end() && b.start < a.end();
    for k in seq.clips.iter().filter(|c| visual(c) && full_card(c)) {
        let below: Vec<&m::Clip> = seq.clips.iter().filter(|o| visual(o) && is_model(o) && idx(o.lane) > idx(k.lane) && overlap(o, k)).collect();
        if below.is_empty() {
            continue;
        }
        let names = below.iter().take(3).map(|o| format!("{} ({})", o.id.0, o.name)).collect::<Vec<_>>().join(", ");
        for top in seq.clips.iter().filter(|o| visual(o) && is_model(o) && idx(o.lane) < idx(k.lane) && overlap(o, k)) {
            out.push(Problem {
                kind: "3d_split_by_2d",
                at: secs(top.start.max(k.start)),
                clip: top.id.0 as u32,
                problem: format!(
                    "3D clip {} ({}) is on a lane above the full-frame card {} ({}) while {} other 3D clip{} ({names}) sit below it: the card covers the scene but not clip {}, which shows through it",
                    top.id.0,
                    top.name,
                    k.id.0,
                    k.name,
                    below.len(),
                    if below.len() == 1 { "" } else { "s" },
                    top.id.0
                ),
                fix: "keep every 3D model on one side of a full-frame card: for a fade or cover, restack the card above all 3D lanes (restack it to: top); for a backdrop, put it below them all",
            });
        }
    }

    // Footage blown up past its pixels: soft, smeared detail.
    let mut told = std::collections::HashSet::new();
    for c in view.clips.iter().filter(|c| c.kind == ClipKind::Video) {
        let Some(mi) = c.media.and_then(|m| view.media(m)) else { continue };
        if mi.kind != view::MediaKind::Video || mi.width == 0 || mi.height == 0 || !told.insert(mi.id) {
            continue;
        }
        let fit = (view.width as f32 / mi.width as f32).min(view.height as f32 / mi.height as f32);
        let shown = fit * c.transform.scale / 100.0;
        if shown > 1.2 {
            out.push(Problem {
                kind: "upscaled",
                at: secs(c.start),
                clip: c.id,
                problem: format!("{} is {}x{} but shown {:.0}% larger than its own pixels: it will look soft and smeared", mi.name, mi.width, mi.height, (shown - 1.0) * 100.0),
                fix: "use a sharper source (download a higher resolution), show it smaller (a picture-in-picture, a framed card), or blur and darken it as a background rather than the hero",
            });
        }
    }

    // Volume faked with flat layers: dozens of flat cards and shapes in
    // 3D at once, stacked into one object.
    let flat3d: Vec<&m::Clip> = seq.clips.iter().filter(|c| visual(c) && c.transform.three_d && matches!(&c.source, m::ClipSource::Generator(g) if matches!(g.kind, m::GeneratorKind::Shape | m::GeneratorKind::Solid | m::GeneratorKind::Gradient | m::GeneratorKind::Text))).collect();
    if flat3d.len() >= 10 {
        let busiest = flat3d.iter().map(|c| (c.start, flat3d.iter().filter(|o| o.start <= c.start && o.end() > c.start).count())).max_by_key(|(_, n)| *n);
        if let Some((at, n)) = busiest.filter(|(_, n)| *n >= 10) {
            out.push(Problem {
                kind: "flat_3d_object",
                at: secs(at),
                clip: 0,
                problem: format!("{n} flat cards and shapes in 3D at once: an object built from flat layers looks like paper cut-outs as soon as it turns, and cannot take light like a solid"),
                fix: "build it as one real 3D model: the model op (a sphere or other shape, or a .glb/.obj file) and mesh steps to sculpt it (guide topic models; craft topic sculpting); then delete the flat layers",
            });
        }
    }

    // Effects piled on one clip.
    const HEAVY: [&str; 7] = ["glow", "god_rays", "gaussian_blur", "bloom", "light_leak", "glitch", "echo"];
    for c in seq.clips.iter().filter(|c| visual(c)) {
        let on: Vec<&str> = c.effects.iter().filter(|e| e.enabled).map(|e| e.kind.rsplit('.').next().unwrap_or(&e.kind)).collect();
        let heavy = on.iter().filter(|k| HEAVY.contains(k)).count();
        if on.len() >= 4 || heavy >= 2 {
            out.push(Problem {
                kind: "effects_piled",
                at: secs(c.start),
                clip: c.id.0 as u32,
                problem: format!("clip {} carries {} effects ({}): stacked looks muddy the picture{}", c.id.0, on.len(), on.join(", "), if heavy >= 2 { " and the heavy ones make playback stutter" } else { "" }),
                fix: "keep one effect that does the job (two at most), at a modest strength; take the rest off",
            });
        }
    }

    // Flashes: short bright full-frame cards.
    let mut flashes: Vec<i64> = seq.clips.iter().filter(|c| visual(c) && c.duration <= SEC / 2 && card_brightness(c).is_some_and(|b| b > 0.7) && !matches!(&c.source, m::ClipSource::Generator(g) if g.params.contains_key("size"))).map(|c| c.start).collect();
    flashes.sort();
    let end = view.duration().max(SEC);
    if flashes.windows(3).any(|w| w[2] - w[0] <= SEC) {
        out.push(Problem {
            kind: "strobing",
            at: secs(flashes[0]),
            clip: 0,
            problem: "three or more bright flashes within one second: this strobes, is hard to watch and can trigger photosensitive seizures".into(),
            fix: "keep flashes at least a second apart, and use them only on the one or two biggest hits",
        });
    } else if flashes.len() as f64 > secs(end) / 6.0 + 1.0 {
        out.push(Problem {
            kind: "flash_overuse",
            at: secs(flashes[0]),
            clip: 0,
            problem: format!("{} white flashes in {:.0} s: a flash only lands when it is rare", flashes.len(), secs(end)),
            fix: "keep a flash for the drop or the reveal (one or two per video); cut straight on the other beats",
        });
    }

    // A transition on every cut, or a grab bag of them.
    let spine = Some(seq.spine_id());
    let cuts = seq.clips.iter().filter(|c| Some(c.lane) == spine).count().saturating_sub(1);
    let kinds: std::collections::BTreeSet<&str> = seq.transitions.iter().map(|t| t.kind.as_str()).collect();
    if cuts >= 5 && seq.transitions.len() * 10 >= cuts * 8 {
        out.push(Problem {
            kind: "transition_every_cut",
            at: 0.0,
            clip: 0,
            problem: format!("{} of {} cuts have a transition: when everything transitions, nothing feels like a moment", seq.transitions.len(), cuts),
            fix: "make most cuts straight cuts on the beat; keep transitions for changes of scene or energy",
        });
    }
    if kinds.len() >= 5 {
        out.push(Problem {
            kind: "transition_grab_bag",
            at: 0.0,
            clip: 0,
            problem: format!("{} different transitions ({}): it reads as a demo of transitions, not one video", kinds.len(), kinds.iter().copied().collect::<Vec<_>>().join(", ")),
            fix: "pick one or two transitions that suit the tone and repeat them",
        });
    }

    // Sound effects on every beat.
    let audio_lane = |c: &m::Clip| seq.lanes.iter().find(|l| l.id == c.lane).is_some_and(|l| l.kind == m::LaneKind::Audio);
    let mut sfx: Vec<i64> = seq.clips.iter().filter(|c| audio_lane(c) && c.duration < 3 * SEC && ["whoosh", "hit", "boom", "riser"].iter().any(|k| c.name.starts_with(k))).map(|c| c.start).collect();
    sfx.sort();
    if sfx.len() as f64 > secs(end) / 2.5 + 2.0 {
        out.push(Problem {
            kind: "sfx_overuse",
            at: 0.0,
            clip: 0,
            problem: format!("{} sound effects in {:.0} s: a whoosh on every move turns into noise", sfx.len(), secs(end)),
            fix: "keep 3-5 cues for the biggest moments (the hook landing, the reveal, a drop); let the music carry the rest",
        });
    }

    // Songs cut dead, and voices the music buries.
    let media_len = |c: &m::Clip| match &c.source {
        m::ClipSource::Media(r) => model.media_item(r.media).and_then(|mi| mi.info.duration),
        _ => None,
    };
    let is_music = |c: &m::Clip| audio_lane(c) && media_len(c).is_some_and(|d| d >= 20 * SEC);
    for c in seq.clips.iter().filter(|c| is_music(c)) {
        let left = media_len(c).unwrap_or(0) - c.source_out();
        if left > SEC && c.audio.fade_out.duration < 400_000 && !c.keyframes.contains_key("audio.volume_db") {
            out.push(Problem {
                kind: "music_cut_off",
                at: secs(c.end()),
                clip: c.id.0 as u32,
                problem: format!("the music (clip {}) stops dead at {:.2}s with {:.0} s of the song left: it sounds like a mistake", c.id.0, secs(c.end()), secs(left)),
                fix: "end on the song's own last hit (move the edit so its ending lands there), or fade it out over 2-4 s (audio fade_out with curve s_curve)",
            });
        }
    }
    let speech: Vec<&m::Clip> = seq.clips.iter().filter(|c| audio_lane(c) && (c.name.starts_with("line-") || seq.lanes.iter().find(|l| l.id == c.lane).is_some_and(|l| l.name.to_lowercase().contains("voice")))).collect();
    for v in &speech {
        let mid = v.start + v.duration / 2;
        for mu in seq.clips.iter().filter(|c| is_music(c) && c.start <= mid && c.end() > mid) {
            let at = mid - mu.start;
            let music = mu.property_at("audio.volume_db", at).and_then(|x| x.as_f64()).unwrap_or(mu.audio.volume_db);
            let voice = v.property_at("audio.volume_db", mid - v.start).and_then(|x| x.as_f64()).unwrap_or(v.audio.volume_db);
            if music > -14.0 && music > voice - 10.0 {
                out.push(Problem {
                    kind: "voice_buried",
                    at: secs(v.start),
                    clip: v.id.0 as u32,
                    problem: format!("under the line at {:.2}s the music (clip {}) plays at {music:.0} dB against the voice at {voice:.0} dB: the words fight the music", secs(v.start), mu.id.0),
                    fix: "dip the music 10-14 dB under every line (narrate does it; or animate the music's volume down 0.15 s before each line and back 0.4 s after)",
                });
                break;
            }
        }
    }

    // Text too small to read on a phone (sizes are points on a 1080-high
    // canvas; scale shrinks them further).
    for c in view.clips.iter().filter(|c| c.kind == ClipKind::Title && !c.text.trim().is_empty()) {
        let size = c.extra.text_size * c.transform.scale / 100.0;
        if size < 28.0 {
            out.push(Problem {
                kind: "text_too_small",
                at: c.start as f64 / view::SEC as f64,
                clip: c.id as u32,
                problem: format!("\"{}\" is {size:.0} pt: too small to read on a phone", c.text.lines().next().unwrap_or("").chars().take(40).collect::<String>()),
                fix: "40 pt or more for captions and lower thirds, 72+ for headlines; cut words rather than shrinking them",
            });
        }
    }

    // Too many fonts, too many colours.
    let styles: std::collections::BTreeSet<&str> = view.clips.iter().filter(|c| c.kind == ClipKind::Title && !c.text.trim().is_empty()).filter_map(|c| c.text_style.as_deref()).collect();
    if styles.len() >= 4 {
        out.push(Problem {
            kind: "too_many_fonts",
            at: 0.0,
            clip: 0,
            problem: format!("{} text styles in one video ({}): it looks unplanned", styles.len(), styles.iter().copied().collect::<Vec<_>>().join(", ")),
            fix: "one display style for headlines and one plain style for the rest, the same all the way through",
        });
    }
    let mut hues: Vec<(f64, f64, f64)> = Vec::new();
    for c in seq.clips.iter().filter(|c| visual(c)) {
        let m::ClipSource::Generator(g) = &c.source else { continue };
        for key in ["colour", "fill"] {
            if let Some(m::Value::Colour(k)) = g.params.get(key) {
                let (mx, mn) = (k.r.max(k.g).max(k.b), k.r.min(k.g).min(k.b));
                // Greys and near-blacks are neutrals, not palette colours.
                if mx - mn < 0.15 || mx < 0.15 {
                    continue;
                }
                if !hues.iter().any(|h| (h.0 - k.r).abs() + (h.1 - k.g).abs() + (h.2 - k.b).abs() < 0.25) {
                    hues.push((k.r, k.g, k.b));
                }
            }
        }
    }
    if hues.len() >= 5 {
        out.push(Problem {
            kind: "palette_sprawl",
            at: 0.0,
            clip: 0,
            problem: format!("{} different strong colours on cards and shapes: the eye has nowhere to go", hues.len()),
            fix: "one accent colour (two at most) over neutrals (near-black, off-white, greys); the accent marks what matters",
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layer_under_a_full_frame_card_is_hidden() {
        let mut p = m::Project::default();
        let act = p.active_sequence;
        let seq = p.sequences.iter_mut().find(|s| s.id == act).unwrap();
        let (top, low) = (m::LaneId(901), m::LaneId(902));
        seq.lanes.insert(0, m::Lane::new(low, m::LaneKind::Visual, "low"));
        seq.lanes.insert(0, m::Lane::new(top, m::LaneKind::Visual, "top"));
        let solid = |id: u64, lane| {
            let mut g = m::Generator::new(m::GeneratorKind::Solid);
            g.params.insert("colour".into(), m::Value::Colour(m::Rgba { r: 0.1, g: 0.1, b: 0.1, a: 1.0 }));
            m::Clip::new(m::ClipId(id), lane, 0, 2 * SEC, m::ClipSource::Generator(g))
        };
        let mut boxed = solid(2, low);
        boxed.source = {
            let mut g = m::Generator::new(m::GeneratorKind::Shape);
            g.params.insert("size".into(), m::Value::Point(m::Vec2 { x: 0.2, y: 0.2 }));
            m::ClipSource::Generator(g)
        };
        seq.clips.push(solid(1, top));
        seq.clips.push(boxed);
        let view = crate::bridge::view_of(&p, 0);
        let found = problems(&p, &view);
        assert!(found.iter().any(|x| x.kind == "hidden_layer" && x.clip == 2), "{:?}", found.iter().map(|x| &x.problem).collect::<Vec<_>>());
    }

    /// A full-frame card (a fade, a cover) between 3D models splits the
    /// scene: the model above it shows through the fade.
    #[test]
    fn a_3d_model_above_a_cover_with_the_scene_below_is_flagged() {
        let build = |model_on_top: bool| {
            let mut p = m::Project::default();
            let act = p.active_sequence;
            let seq = p.sequences.iter_mut().find(|s| s.id == act).unwrap();
            let (top, mid, low) = (m::LaneId(901), m::LaneId(902), m::LaneId(903));
            seq.lanes.insert(0, m::Lane::new(low, m::LaneKind::Visual, "world"));
            seq.lanes.insert(0, m::Lane::new(mid, m::LaneKind::Visual, "fade"));
            seq.lanes.insert(0, m::Lane::new(top, m::LaneKind::Visual, "bear"));
            let model = |id: u64, lane| m::Clip::new(m::ClipId(id), lane, 0, 4 * SEC, m::ClipSource::Generator(m::Generator::new(m::GeneratorKind::Model)));
            let mut g = m::Generator::new(m::GeneratorKind::Solid);
            g.params.insert("colour".into(), m::Value::Colour(m::Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }));
            let mut fade = m::Clip::new(m::ClipId(2), mid, 0, 2 * SEC, m::ClipSource::Generator(g));
            fade.name = "Fade in".into();
            seq.clips.push(model(1, if model_on_top { top } else { low }));
            seq.clips.push(fade);
            seq.clips.push(model(3, low));
            let view = crate::bridge::view_of(&p, 0);
            problems(&p, &view)
        };
        let split = build(true);
        assert!(split.iter().any(|x| x.kind == "3d_split_by_2d" && x.clip == 1), "{:?}", split.iter().map(|x| &x.problem).collect::<Vec<_>>());
        let whole = build(false);
        assert!(!whole.iter().any(|x| x.kind == "3d_split_by_2d"), "every model under the cover is fine");
    }

    #[test]
    fn tiny_text_is_flagged_and_readable_text_is_not() {
        let mut p = m::Project::default();
        let act = p.active_sequence;
        let seq = p.sequences.iter_mut().find(|s| s.id == act).unwrap();
        let lane = m::LaneId(901);
        seq.lanes.insert(0, m::Lane::new(lane, m::LaneKind::Visual, "titles"));
        let title = |id: u64, size: f64| {
            let mut g = m::Generator::new(m::GeneratorKind::Text);
            g.text = vec![m::TextRun { text: format!("words {id}"), style: Default::default() }];
            g.params.insert("size".into(), m::Value::Scalar(size));
            m::Clip::new(m::ClipId(id), lane, 0, 2 * SEC, m::ClipSource::Generator(g))
        };
        seq.clips.push(title(1, 18.0));
        seq.clips.push(title(2, 48.0));
        let view = crate::bridge::view_of(&p, 0);
        let small: Vec<u32> = problems(&p, &view).iter().filter(|x| x.kind == "text_too_small").map(|x| x.clip).collect();
        assert_eq!(small, vec![1]);
    }
}
