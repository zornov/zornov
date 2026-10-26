// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Staging checks for `check`: characters drawn over each other, and a
//! backdrop moved or zoomed out so far that the empty canvas shows past
//! its edge.

use super::{Session, secs};
use crate::editor::pins::LayerMap;
use crate::view::{self, ClipKind, SEC};
use serde_json::{Value, json};
use studio_model as m;

type Quad = [(f32, f32); 4];

/// Whether `p` is inside the convex quad `q` (either winding).
fn inside(q: &Quad, p: (f32, f32)) -> bool {
    let mut sign = 0.0f32;
    for i in 0..4 {
        let (a, b) = (q[i], q[(i + 1) % 4]);
        let c = (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0);
        if c.abs() < 1e-3 {
            continue;
        }
        if sign == 0.0 {
            sign = c.signum();
        } else if c.signum() != sign {
            return false;
        }
    }
    true
}

fn bounds(q: &Quad) -> (f32, f32, f32, f32) {
    let xs = q.iter().map(|p| p.0);
    let ys = q.iter().map(|p| p.1);
    (xs.clone().fold(f32::MAX, f32::min), ys.clone().fold(f32::MAX, f32::min), xs.fold(f32::MIN, f32::max), ys.fold(f32::MIN, f32::max))
}

/// Times from `start` to `end` every `step`, merged into runs of
/// `(first, last)` where `hit` holds.
fn runs(start: view::Time, end: view::Time, step: view::Time, mut hit: impl FnMut(view::Time) -> bool) -> Vec<(view::Time, view::Time)> {
    let mut out: Vec<(view::Time, view::Time)> = Vec::new();
    let mut t = start;
    while t < end {
        if hit(t) {
            match out.last_mut() {
                Some(r) if r.1 + step >= t => r.1 = t,
                _ => out.push((t, t)),
            }
        }
        t += step;
    }
    out
}

impl Session {
    /// Where layer `id` is drawn at `t` (its picture's four corners on the
    /// canvas, pixels from the centre) if it shows there, nearly opaque.
    fn drawn_quad(&self, id: u32, t: view::Time) -> Option<Quad> {
        let c = self.doc.project.clip(id)?;
        if t < c.start || t >= c.start + c.dur || c.kind == ClipKind::Audio {
            return None;
        }
        let mc = self.doc.model.sequences.iter().find_map(|q| q.clip(m::ClipId(id as u64)))?;
        if !mc.enabled || mc.transform_at(t - c.start).opacity < 0.95 {
            return None;
        }
        let map = LayerMap::at(&self.doc.project, &self.doc.model, id, t)?;
        let q = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].map(|(u, v)| {
            let p = map.canvas(m::Vec2::new(u, v));
            (p.x, p.y)
        });
        Some(q)
    }

    /// Whether a plain, opaque picture covers the whole frame at `t` by its
    /// place alone: each frame corner lands on it (in 2D, or along the 3D
    /// camera's ray). Masks, effects, corner pins and titles may leave
    /// gaps a box cannot show, so those are not taken as covering.
    #[cfg(feature = "render")]
    fn frame_covered(&self, t: view::Time) -> bool {
        let p = &self.doc.project;
        let (hw, hh) = (p.width as f32 / 2.0 - 1.0, p.height as f32 / 2.0 - 1.0);
        p.clips.iter().filter(|c| matches!(c.kind, ClipKind::Video) && c.start <= t && t < c.end() && p.lane(c.lane).is_some_and(|l| !l.hidden)).any(|c| {
            let Some(mc) = self.doc.model.sequences.iter().find_map(|q| q.clip(m::ClipId(c.id as u64))) else { return false };
            let local = t - c.start;
            if !mc.enabled || !mc.masks.is_empty() || !mc.effects.is_empty() || mc.corner_pin_at(local).is_some_and(|k| k.enabled) || mc.transform_at(local).opacity < 0.99 {
                return false;
            }
            let Some(map) = LayerMap::at(p, &self.doc.model, c.id, t) else { return false };
            [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)].iter().all(|&(x, y)| map.flat_layer(eframe::egui::Vec2::new(x, y)).is_some_and(|q| (0.0..=1.0).contains(&q.x) && (0.0..=1.0).contains(&q.y)))
        })
    }

    /// Picture layers that are characters or props: not the main lane, not
    /// filling the frame, drawn on the canvas.
    pub(in crate::agent) fn figures(&self) -> Vec<&view::Clip> {
        let p = &self.doc.project;
        let spine = p.spine();
        let area = p.width as f32 * p.height as f32;
        p.clips
            .iter()
            .filter(|c| c.kind == ClipKind::Video && Some(c.lane) != spine && p.lane(c.lane).is_some_and(|l| !l.hidden))
            .filter(|c| {
                self.drawn_quad(c.id, c.start + c.dur / 2).is_some_and(|q| {
                    let (x0, y0, x1, y1) = bounds(&q);
                    (x1 - x0) * (y1 - y0) < 0.6 * area
                })
            })
            .collect()
    }

    /// Whether a 3D model on a shown lane moves between `s` and `e`: a
    /// motion (`act`) playing, or keys on its bones, reaches, face, look or
    /// place in that time. A model is not a picture, so [`Self::figures`]
    /// does not hold it, but a 3D character acting is acting.
    fn model_acts(&self, s: view::Time, e: view::Time) -> bool {
        let p = &self.doc.project;
        let seq = self.doc.seq();
        self.doc.model.sequences.iter().filter(|q| q.id == seq).flat_map(|q| q.clips.iter()).any(|mc| {
            let m::ClipSource::Generator(g) = &mc.source else { return false };
            if g.kind != m::GeneratorKind::Model || !mc.enabled || p.lane(mc.lane.0 as u32).is_none_or(|l| l.hidden) {
                return false;
            }
            let (cs, ce) = (mc.start, mc.start + mc.duration);
            if ce <= s || cs >= e {
                return false;
            }
            let start = match g.params.get("action_start") {
                Some(m::Value::Scalar(a)) => cs + (*a * SEC as f64) as view::Time,
                _ => cs,
            };
            let playing = matches!(g.params.get("action"), Some(m::Value::Text(a)) if !a.is_empty()) && start < e;
            playing
                || mc.keyframes.iter().any(|(path, tr)| {
                    ["generator.bone.", "generator.ik.", "generator.shape.", "generator.look", "generator.action", "transform."].iter().any(|h| path.starts_with(h))
                        && tr.keys.len() > 1
                        && tr.keys.iter().any(|k| (s..e).contains(&(cs + k.time)))
                })
        })
    }

    pub(in crate::agent) fn staging_problems(&self) -> Vec<Value> {
        let p = &self.doc.project;
        let mut out = Vec::new();
        let step = SEC / 2;
        // Characters on top of each other: one box covering much of
        // another's, held for over half a second.
        let figs = self.figures();
        for (i, a) in figs.iter().enumerate() {
            for b in &figs[i + 1..] {
                let (s, e) = (a.start.max(b.start), (a.start + a.dur).min(b.start + b.dur));
                if e <= s {
                    continue;
                }
                let mut worst = 0f32;
                let rs = runs(s, e, step, |t| {
                    let (Some(qa), Some(qb)) = (self.drawn_quad(a.id, t), self.drawn_quad(b.id, t)) else { return false };
                    let (ax0, ay0, ax1, ay1) = bounds(&qa);
                    let (bx0, by0, bx1, by1) = bounds(&qb);
                    let ix = (ax1.min(bx1) - ax0.max(bx0)).max(0.0);
                    let iy = (ay1.min(by1) - ay0.max(by0)).max(0.0);
                    let small = ((ax1 - ax0) * (ay1 - ay0)).min((bx1 - bx0) * (by1 - by0)).max(1.0);
                    let share = ix * iy / small;
                    worst = worst.max(share);
                    share > 0.45
                });
                if let Some(&(t0, t1)) = rs.iter().find(|r| r.1 > r.0) {
                    // The lower lane is drawn first, so the higher one covers it.
                    let (front, back) = if p.lanes.iter().position(|l| l.id == a.lane) < p.lanes.iter().position(|l| l.id == b.lane) { (a, b) } else { (b, a) };
                    out.push(json!({
                        "kind": "figures_overlap",
                        "problem": format!("clip {} (\"{}\") covers {:.0}% of clip {} (\"{}\") from {:.1}s to {:.1}s: one character hides the other", front.id, front.name, worst * 100.0, back.id, back.name, secs(t0), secs(t1 + step)),
                        "fix": "give each its own spot (see with place + t lists spots on the ground) and keep a gap between them; a character further back goes higher and smaller, not behind another's face",
                        "at": (secs(t0) * 100.0).round() / 100.0,
                        "clip": back.id
                    }));
                }
            }
        }
        // Voice lines with nothing acting: no character moves, enters or
        // leaves while it is said (the words are only heard, never shown).
        let voice: Vec<&view::Clip> = p
            .clips
            .iter()
            .filter(|c| c.kind == ClipKind::Audio && p.lane(c.lane).is_some_and(|l| ["voice", "narrat", "dialog"].iter().any(|w| l.name.to_lowercase().contains(w))))
            .filter(|c| c.dur > SEC)
            .collect();
        let mut still: Vec<(view::Time, view::Time)> = Vec::new();
        for v in &voice {
            let (s, e) = (v.start, v.start + v.dur);
            let acts = figs.iter().any(|f| {
                let (fs, fe) = (f.start, f.start + f.dur);
                if fe <= s || fs >= e {
                    return false;
                }
                if (fs > s && fs < e) || (fe > s && fe < e) {
                    return true;
                }
                self.doc.model.sequences.iter().find_map(|q| q.clip(m::ClipId(f.id as u64))).is_some_and(|mc| {
                    // A 3D character plays a rig action (its bones are keys of the model's own
                    // settings), a looped motion counts as acting for as long as it runs.
                    let plays = matches!(&mc.source, m::ClipSource::Generator(g) if matches!(g.params.get("action"), Some(m::Value::Text(a)) if !a.is_empty()));
                    plays
                        || mc.keyframes.iter().any(|(path, tr)| {
                            crate::agent::perform::is_performance_key(path) && tr.keys.len() > 1 && tr.keys.iter().any(|k| {
                                let at = fs + k.time;
                                at > s && at < e
                            })
                        })
                })
            });
            if !acts && !self.model_acts(s, e) {
                match still.last_mut() {
                    Some(r) if r.1 + SEC >= s => r.1 = e,
                    _ => still.push((s, e)),
                }
            }
        }
        for (s, e) in still.iter().take(6) {
            out.push(json!({
                "kind": "story_still",
                "problem": format!("from {:.1}s to {:.1}s a voice speaks while no character on screen moves, enters or leaves: what is said is only heard, not shown", secs(*s), secs(*e)),
                "fix": "show what the line says: the character it is about acts it (a puppet's act, or a 3D character's act and pose: walk, look, gesture, point…), turns to whoever speaks (listen, nod), or enters or leaves; the place reacts too (a sound, light, breeze)",
                "at": (secs(*s) * 100.0).round() / 100.0
            }));
        }
        if still.len() > 6 {
            out.push(json!({"kind": "story_still", "problem": format!("{} more stretches of voice with nothing acting", still.len() - 6), "fix": "the same"}));
        }
        // The canvas showing past a backdrop's edge.
        let (hw, hh) = (p.width as f32 / 2.0 - 1.0, p.height as f32 / 2.0 - 1.0);
        let corners = [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)];
        let spine = p.spine();
        for c in p.clips.iter().filter(|c| Some(c.lane) == spine && c.kind == ClipKind::Video) {
            let rs = runs(c.start, c.start + c.dur, step, |t| {
                let Some(q) = self.drawn_quad(c.id, t) else { return false };
                if corners.iter().all(|&k| inside(&q, k)) {
                    return false;
                }
                // Something else may fill the frame there.
                !p.clips.iter().any(|o| o.id != c.id && self.drawn_quad(o.id, t).is_some_and(|oq| corners.iter().all(|&k| inside(&oq, k))))
            });
            if let Some(&(t0, t1)) = rs.first() {
                out.push(json!({
                    "kind": "canvas_shows",
                    "problem": format!("clip {} (\"{}\") no longer fills the frame from {:.1}s to {:.1}s: its move or zoom slides its edge in and the empty canvas shows", c.id, c.name, secs(t0), secs(t1 + step)),
                    "fix": "keep a backdrop's scale above what fills the frame for its whole move (scale up, or move it less); for a pan wider than the picture, generate a wider backdrop",
                    "at": (secs(t0) * 100.0).round() / 100.0,
                    "clip": c.id
                }));
            }
        }
        out
    }

    /// The staging problems, with the canvas looked for in rendered
    /// pictures when `look` (pictures may be drawn): a 3D camera moving
    /// past a picture's edge, a 3D model scene running out, a backdrop on
    /// any lane. The rendered runs replace the drawn-box guesses they
    /// cover.
    pub(in crate::agent) fn staging_problems_seen(&mut self, look: bool) -> Vec<Value> {
        let mut out = self.staging_problems();
        #[cfg(feature = "render")]
        if look {
            let seen = self.canvas_gaps();
            let spans: Vec<(f64, f64)> = seen.iter().filter_map(|p| Some((p["from"].as_f64()?, p["to"].as_f64()?))).collect();
            out.retain(|p| p["kind"] != "canvas_shows" || !p["at"].as_f64().is_some_and(|t| spans.iter().any(|&(a, b)| t >= a - 0.5 && t <= b + 0.5)));
            out.extend(seen);
            // Where things really are drawn (their alpha, not their boxes):
            // overlaps, titles over faces, things off screen or cut, lettering
            // outside the safe area, a speaker turned from the listener. These
            // replace the box-based guess at characters on top of each other.
            let drawn = self.layout_problems();
            out.retain(|p| p["kind"] != "figures_overlap");
            out.extend(drawn.iter().map(|p| p.value()));
        }
        #[cfg(not(feature = "render"))]
        let _ = look;
        out
    }

    /// Where the bare canvas shows past the pictures: frames sampled over
    /// the whole picture, drawn small with a see-through canvas, and their
    /// border read for empty bands (the middle drawn, so not a fade or a
    /// hole: other checks report those).
    #[cfg(feature = "render")]
    fn canvas_gaps(&mut self) -> Vec<Value> {
        let p = self.doc.project.clone();
        let end = p.clips.iter().filter(|c| c.kind != ClipKind::Audio && p.lane(c.lane).is_some_and(|l| !l.hidden)).map(|c| c.end()).max().unwrap_or(0);
        if end <= 0 {
            return Vec::new();
        }
        // Every second, at most 600 frames (a long film less often): a
        // camera move lasts seconds.
        let step = SEC.max(end / 600);
        let times: Vec<view::Time> = (0..).map(|i| i * step + step / 2).take_while(|t| *t < end).collect();
        // Drawn only where the layers' places cannot vouch for it: no
        // plain opaque picture covers the whole frame (in 2D or through
        // the 3D camera). Most frames of a film are such a picture.
        let covered: Vec<bool> = times.iter().map(|&t| self.frame_covered(t)).collect();
        let keys: Vec<String> = times.iter().zip(&covered).map(|(&t, &c)| if c { String::new() } else { self.scene_key_pub(t, t, "bare96") }).collect();
        let todo: Vec<view::Time> = times.iter().zip(&keys).filter(|(_, k)| !k.is_empty() && !self.memo_pub().contains_key(*k)).map(|(t, _)| *t).collect();
        let got = bare_frames(&self.doc, &todo, BARE_WIDTH);
        for (t, e) in todo.iter().zip(got) {
            let key = self.scene_key_pub(*t, *t, "bare96");
            self.memo_pub().insert(key, e.map_or(Value::Null, |e| json!(e)));
        }
        let sides: Vec<Option<[f32; 8]>> = keys.iter().map(|k| if k.is_empty() { Some([0.0; 8]) } else { self.memo_pub().get(k).and_then(|v| serde_json::from_value(v.clone()).ok()) }).collect();
        let shows = |e: &Option<[f32; 8]>| e.is_some_and(|e| e[..4].iter().any(|f| *f >= EDGE_SHOWS));
        let mut out = Vec::new();
        let mut i = 0;
        while i < times.len() {
            if !shows(&sides[i]) {
                i += 1;
                continue;
            }
            let mut j = i;
            while j + 1 < times.len() && shows(&sides[j + 1]) {
                j += 1;
            }
            // The widest band on each side over the run.
            let mut band = [0f32; 4];
            for e in sides[i..=j].iter().flatten() {
                for s in 0..4 {
                    if e[s] >= EDGE_SHOWS {
                        band[s] = band[s].max(e[4 + s]);
                    }
                }
            }
            let names = ["left", "right", "top", "bottom"];
            let px = [p.width, p.width, p.height, p.height];
            // Every side it shows along; its width when that was measured.
            let seen: Vec<usize> = (0..4).filter(|&s| band[s] > 0.0 || sides[i..=j].iter().flatten().any(|e| e[s] >= EDGE_SHOWS)).collect();
            let said: Vec<String> = seen.iter().map(|&s| if band[s] > 0.0 { format!("{} ({:.0} px)", names[s], (band[s] * px[s] as f32).max(1.0)) } else { names[s].to_owned() }).collect();
            let (t0, t1) = (times[i].saturating_sub(step / 2).max(0), (times[j] + step / 2).min(end));
            let mid = times[(i + j) / 2];
            let camera = m::space::active_camera(self.doc.model.active(), mid).map(|c| c.id.0 as u32);
            // The picture whose edge it is: the biggest drawn at that time.
            let picture = p
                .clips
                .iter()
                .filter(|c| c.kind != ClipKind::Audio && c.start <= mid && mid < c.end())
                .filter_map(|c| {
                    let (x0, y0, x1, y1) = bounds(&self.drawn_quad(c.id, mid)?);
                    Some(((x1 - x0) * (y1 - y0), c))
                })
                .max_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, c)| c);
            let what = match picture {
                Some(c) => format!("past the edge of clip {} (\"{}\")", c.id, c.name),
                None => "where no picture is".into(),
            };
            // A 3D set with nothing behind it there: no backdrop to keep the camera inside.
            let set_only = picture.is_none() && self.doc.model.active().clips.iter().any(|c| crate::mesh_store::file_of(c).is_some() && c.start <= mid && mid < c.start + c.duration);
            let (problem, fix) = match camera {
                Some(cam) if set_only => (
                    format!("the empty canvas shows along the {} from {:.1}s to {:.1}s: 3D camera clip {cam} sees past the set, where nothing is", said.join(", "), secs(t0), secs(t1)),
                    "give the 3D scene something behind it there: a sky (`world` sky, or `build_world`), a backdrop model or card far behind the set, or aim the camera so the set fills the frame",
                ),
                Some(cam) => (
                    format!("the empty canvas shows along the {} from {:.1}s to {:.1}s: 3D camera clip {cam} looks {what}", said.join(", "), secs(t0), secs(t1)),
                    "keep the camera inside the picture for its whole move: move it less (smaller truck, pan or dolly), push it in more, or scale the backdrop up (parallax with overscan does this); for a wide move, generate a wider backdrop",
                ),
                None => (
                    format!("the empty canvas shows along the {} from {:.1}s to {:.1}s, {what}", said.join(", "), secs(t0), secs(t1)),
                    "keep a backdrop's scale above what fills the frame for its whole move (scale up, or move it less), or put a picture behind it",
                ),
            };
            let mut o = json!({"kind": "canvas_shows", "problem": problem, "fix": fix, "at": (secs(t0) * 100.0).round() / 100.0, "from": (secs(t0) * 100.0).round() / 100.0, "to": (secs(t1) * 100.0).round() / 100.0});
            if let Some(c) = camera.or(picture.map(|c| c.id)) {
                o["clip"] = json!(c);
            }
            if let (Some(_), Some(c)) = (camera, picture) {
                o["picture"] = json!(c.id);
            }
            out.push(o);
            i = j + 1;
        }
        self.memo_save_pub();
        out
    }
}

/// Width the canvas check draws its frames at.
#[cfg(feature = "render")]
const BARE_WIDTH: u32 = 96;
/// A border line counts as showing the canvas when this share of it is
/// bare: a strip, or the wedge a turned or tilted picture leaves.
const EDGE_SHOWS: f32 = 0.1;

/// A frame's border read for the bare canvas: for left, right, top and
/// bottom the share of the outermost line that is see-through, then how
/// deep (a share of the width or height) the lines run that are mostly
/// see-through. `None` when too little is drawn for an edge to fall short
/// (a fade, a hole, a black frame: other checks report those).
fn bare_edges(width: usize, height: usize, alpha: impl Fn(usize, usize) -> u8) -> Option<[f32; 8]> {
    let (w, h) = (width, height);
    if w < 4 || h < 4 {
        return None;
    }
    // Empty, not an edge pixel the picture partly covers (drawn small,
    // a picture that just fills the frame can leave its last pixel half
    // covered).
    let bare = |x: usize, y: usize| alpha(x, y) < 16;
    // Mostly drawn: some of the picture is there for its edge to fall
    // short of.
    let solid = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).filter(|&(x, y)| alpha(x, y) > 200).count();
    if (solid as f32) < 0.2 * (w * h) as f32 {
        return None;
    }
    let share = |f: &dyn Fn(usize) -> bool, from: usize, to: usize| if to > from { (from..to).filter(|&k| f(k)).count() as f32 / (to - from) as f32 } else { 0.0 };
    let col = |x: usize, from: usize, to: usize| share(&|y| bare(x, y), from, to);
    let row = |y: usize, from: usize, to: usize| share(&|x| bare(x, y), from, to);
    // How many lines from each side are mostly bare: the bands.
    let band = |line: &dyn Fn(usize) -> f32, n: usize, from_end: bool| (0..n).take_while(|&k| line(if from_end { n - 1 - k } else { k }) >= 0.5).count();
    let (l, r) = (band(&|x| col(x, 0, h), w, false), band(&|x| col(x, 0, h), w, true));
    // Rows read between the side bands, so a wide side band is not taken
    // for top and bottom ones too.
    let (t, b) = (band(&|y| row(y, l, w - r), h, false), band(&|y| row(y, l, w - r), h, true));
    // Each outer line read between the bands across it: a strip down one
    // side runs across the ends of the top and bottom lines.
    Some([col(0, t, h - b), col(w - 1, t, h - b), row(0, l, w - r), row(h - 1, l, w - r), l as f32 / w as f32, r as f32 / w as f32, t as f32 / h as f32, b as f32 / h as f32])
}

/// Frames at `times`, drawn `width` wide over a see-through canvas, read
/// with [`bare_edges`]; on the GPU when there is one, else on CPU threads.
#[cfg(feature = "render")]
fn bare_frames(doc: &crate::doc::Doc, times: &[view::Time], width: u32) -> Vec<Option<[f32; 8]>> {
    use studio_render::FrameRenderer;
    if times.is_empty() {
        return Vec::new();
    }
    let registry = studio_render::source::SourceRegistry::new();
    crate::editor::render_view::register_sources(doc, &registry);
    crate::editor::render_view::install_text_renderer(doc.project.height);
    let mut model = doc.model.clone();
    model.settings.background.a = 0.0;
    let scene = std::sync::Arc::new(studio_render::model_scene::ModelScene::new(std::sync::Arc::new(model)));
    let (pw, ph) = (doc.project.width.max(1), doc.project.height.max(1));
    let w = width.min(pw).max(16);
    let h = ((w as f64 * ph as f64 / pw as f64).round() as u32).max(16);
    let fps = doc.project.fps.max(1.0) as f64;
    let read = |renderer: &mut dyn FrameRenderer, t: view::Time| -> Option<[f32; 8]> {
        let e = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            renderer.render_encoded(scene.as_ref(), t.max(0), Some((w, h)), &studio_render::OutputSettings { premultiplied: false, ..Default::default() }, studio_render::OutputDepth::Eight, (t.max(0) as f64 / 1e6 * fps) as u32)
        }))
        .ok()?;
        let (ew, eh) = (e.width as usize, e.height as usize);
        match e.pixels {
            studio_render::EncodedPixels::Rgba8(v) => bare_edges(ew, eh, |x, y| v[(y * ew + x) * 4 + 3]),
            studio_render::EncodedPixels::Rgba16(v) => bare_edges(ew, eh, |x, y| (v[(y * ew + x) * 4 + 3] >> 8) as u8),
        }
    };
    // One decode cache for every renderer: the same pictures (backdrops,
    // characters) are in most frames, decoded once.
    let cache = studio_render::DecodeCache::new(512 << 20);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 8);
    let per = times.len().div_ceil(threads).max(1);
    std::thread::scope(|sc| {
        let hs: Vec<_> = times
            .chunks(per)
            .map(|chunk| {
                let (registry, cache, read) = (registry.clone(), cache.clone(), &read);
                (chunk.len(), sc.spawn(move || {
                    let mut r = studio_render::CpuCompositor::new(studio_render::Sources::new(registry, cache));
                    chunk.iter().map(|&t| read(&mut r, t)).collect::<Vec<_>>()
                }))
            })
            .collect();
        hs.into_iter().flat_map(|(n, h)| h.join().unwrap_or_else(|_| vec![None; n])).collect::<Vec<_>>()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_in_quads_and_runs() {
        let q: Quad = [(-10.0, -10.0), (10.0, -10.0), (10.0, 10.0), (-10.0, 10.0)];
        assert!(inside(&q, (0.0, 0.0)) && inside(&q, (9.0, -9.0)));
        assert!(!inside(&q, (11.0, 0.0)));
        let back: Quad = [(-10.0, 10.0), (10.0, 10.0), (10.0, -10.0), (-10.0, -10.0)];
        assert!(inside(&back, (0.0, 0.0)) && !inside(&back, (0.0, 12.0)));
        assert_eq!(runs(0, 10, 1, |t| (2..5).contains(&t) || t == 8), vec![(2, 4), (8, 8)]);
    }

    #[test]
    fn a_3d_character_acting_counts_as_acting() {
        let mut s = Session::default();
        s.edit(&[json!({"op": "model", "shape": "cube", "size": 100, "name": "Box", "duration": 6})], "t", false).unwrap();
        assert!(!s.model_acts(SEC, 3 * SEC), "a still box does not act");
        s.edit(&[json!({"op": "model", "shape": "character", "size": 300, "detail": 1, "name": "Hero", "duration": 6}), json!({"op": "act", "clip": "Hero", "motion": "walk", "at": 2})], "t", false).unwrap();
        assert!(s.model_acts(3 * SEC, 4 * SEC), "a walk playing is acting");
    }
}


#[cfg(test)]
mod canvas_tests {
    use super::*;
    use crate::services;

    #[test]
    fn edges_read_strips_and_skip_fades() {
        let (w, h) = (96, 54);
        let strip = bare_edges(w, h, |x, _| if x < 10 { 0 } else { 255 }).unwrap();
        assert_eq!(strip[0], 1.0);
        assert!((strip[4] - 10.0 / 96.0).abs() < 1e-6, "{strip:?}");
        assert_eq!(&strip[1..4], &[0.0, 0.0, 0.0]);
        // A tilted picture's corner wedge.
        let wedge = bare_edges(w, h, |x, y| if x + y < 12 { 0 } else { 255 }).unwrap();
        assert!(wedge[0] >= EDGE_SHOWS && wedge[4] == 0.0, "{wedge:?}");
        // Slid mostly away: only the left 40% is drawn.
        let off = bare_edges(w, h, |x, _| if x < 38 { 255 } else { 0 }).unwrap();
        assert!(off[1] == 1.0 && off[5] > 0.55, "{off:?}");
        assert_eq!(bare_edges(w, h, |_, _| 255), Some([0.0; 8]));
        // A fade or nothing drawn: not this check's to report.
        assert_eq!(bare_edges(w, h, |_, _| 90), None);
        assert_eq!(bare_edges(w, h, |_, _| 0), None);
    }

    /// A camera trucking past a 3D backdrop that does not cover the move:
    /// the rendered frames show the canvas along one side, and check names
    /// the camera, the backdrop and when.
    #[test]
    fn a_camera_moving_past_a_backdrop_shows_the_canvas() {
        let mut s = Session::new(services::project::new_custom("Gap", 640, 360, 30.0));
        s.edit(&[json!({"op": "card", "colour": "#204060", "at": 0, "duration": 6})], "bg", false).unwrap();
        let bg = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        s.edit(&[json!({"op": "three_d", "clip": bg})], "3d", false).unwrap();
        let size = s.model_clip(bg).unwrap().transform.scale_x;
        s.edit(&[json!({"op": "camera", "at": 0, "duration": 6, "move": {"type": "truck", "by": 400, "from": 2, "duration": 2}})], "cam", false).unwrap();
        let cam = m::space::active_camera(s.doc.model.active(), 0).map(|c| c.id.0 as u32).expect("a camera");
        // Covered: the camera op grew the backdrop for its move.
        assert!(!s.staging_problems_seen(true).iter().any(|p| p["kind"] == "canvas_shows"), "{:?}", s.staging_problems_seen(true));
        // Back to its own size: the move runs off it.
        let mut mc = s.model_clip(bg).unwrap();
        mc.transform.scale_x = size;
        mc.transform.scale_y = size;
        let seq = s.doc.seq();
        s.doc.apply("shrink", m::Edit::SetClip { seq, clip: Box::new(mc) });
        let probs = s.staging_problems_seen(true);
        let shows: Vec<&Value> = probs.iter().filter(|p| p["kind"] == "canvas_shows").collect();
        assert_eq!(shows.len(), 1, "{probs:?}");
        let p = shows[0];
        assert_eq!(p["clip"], json!(cam), "{p}");
        assert_eq!(p["picture"], json!(bg), "{p}");
        let text = p["problem"].as_str().unwrap();
        assert!(text.contains("3D camera clip") && (text.contains("left") || text.contains("right")), "{text}");
        let (from, to) = (p["from"].as_f64().unwrap(), p["to"].as_f64().unwrap());
        assert!(from >= 2.0 && from < 4.5 && to > 4.0 && to <= 6.0, "{from}..{to}");
    }
}
