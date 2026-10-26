// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! FCPXML 1.9+ import: the primary storyline, connected clips, titles,
//! gaps, compound clips, transitions and markers.
//!
//! Element mapping:
//!
//! * `asset-clip`, `clip` (with a `video`/`audio` child), `video`, `audio`
//!   → media clips (`video` pointing at an effect becomes a solid);
//! * `title` → a text generator holding the title's characters;
//! * `gap` → a spine gap; `ref-clip` → a compound clip and nested sequence;
//! * `transition` → a transition at the nearest cut;
//! * `marker`, `chapter-marker` → clip markers;
//! * elements with a `lane` attribute → clips on visual (positive) or
//!   audio (negative) lanes; secondary storylines are laid out in order on
//!   their lane.
//!
//! Filters, adjustments, retiming, keyframes, roles and multicam are
//! listed in the report and not imported.

use super::{ImportError, Imported, Report, dry_run, fit_transitions, url_to_path};
use crate::clip::{Clip, ClipSource, Generator, GeneratorKind, MediaRef, SequenceRef};
use crate::edit::Edit;
use crate::id::*;
use crate::media::{AudioInfo, MediaInfo, MediaItem, MediaKind, VideoInfo};
use crate::project::Project;
use crate::sequence::{Lane, LaneKind, Marker, Sequence, Transition, TransitionAlign};
use crate::time::{Micros, TimeRange};
use roxmltree::{Document, Node};
use std::collections::{HashMap, HashSet};

/// Imports the first project of an FCPXML document.
pub fn import(project: &mut Project, xml: &str) -> Result<Imported, ImportError> {
    // FCPXML files usually start with `<!DOCTYPE fcpxml>`.
    let opts = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    let doc = Document::parse_with_options(xml, opts).map_err(|e| ImportError::Parse(e.to_string()))?;
    let root = doc.root_element();
    if root.tag_name().name() != "fcpxml" {
        return Err(ImportError::Unsupported("root element is not <fcpxml>".into()));
    }
    let mut report = Report::default();
    let version = root.attribute("version").unwrap_or("0");
    let (major, minor) = version.split_once('.').map_or((0, 0), |(a, b)| (a.parse().unwrap_or(0), b.parse().unwrap_or(0)));
    if (major, minor) < (1, 9) {
        report.add("fcpxml", format!("version {version} is older than 1.9; imported on a best-effort basis"));
    }
    let resources: HashMap<String, Node> = root.children().filter(|n| n.has_tag_name("resources")).flat_map(|r| r.children()).filter(|n| n.is_element()).filter_map(|n| Some((n.attribute("id")?.to_string(), n))).collect();
    let project_node = root.descendants().find(|n| n.has_tag_name("project"));
    let seq_node = project_node
        .and_then(|p| p.children().find(|n| n.has_tag_name("sequence")))
        .or_else(|| root.descendants().find(|n| n.has_tag_name("sequence") && !n.ancestors().any(|a| a.has_tag_name("resources"))))
        .ok_or_else(|| ImportError::Unsupported("no project sequence found".into()))?;
    let name = project_node.and_then(|p| p.attribute("name")).unwrap_or("Imported").to_string();

    let mut imp = Importer { p: project, report, resources, media: Vec::new(), nested: Vec::new(), compound: HashMap::new(), warned: HashSet::new() };
    imp.check_format(seq_node);
    let seq = imp.sequence(seq_node, &name)?;
    let main = seq.id;
    let mut edits: Vec<Edit> = imp.media.into_iter().map(|m| Edit::InsertMedia { item: Box::new(m), index: None }).collect();
    edits.extend(imp.nested.into_iter().map(|s| Edit::InsertSequence { sequence: Box::new(s), index: None }));
    edits.push(Edit::InsertSequence { sequence: Box::new(seq), index: None });
    let edit = Edit::Batch { edits };
    dry_run(imp.p, &edit)?;
    Ok(Imported { edit, sequence: main, report: imp.report })
}

/// Parses an FCPXML time such as `"1001/30000s"`, `"5s"` or `"0s"`.
pub fn parse_time(s: &str) -> Option<Micros> {
    let s = s.trim().strip_suffix('s')?;
    let (num, den) = match s.split_once('/') {
        Some((n, d)) => (n.trim().parse::<i128>().ok()?, d.trim().parse::<i128>().ok()?),
        None => {
            // Plain seconds, possibly with a decimal point.
            let v: f64 = s.parse().ok()?;
            return Some((v * 1e6).round() as Micros);
        }
    };
    if den == 0 {
        return None;
    }
    let n = num * 1_000_000;
    Some(((n + if n >= 0 { den / 2 } else { -den / 2 }) / den) as Micros)
}

struct Importer<'a, 'x> {
    p: &'a mut Project,
    report: Report,
    resources: HashMap<String, Node<'x, 'x>>,
    media: Vec<MediaItem>,
    nested: Vec<Sequence>,
    /// Imported `media` resources (compound clips) by resource id.
    compound: HashMap<String, SequenceId>,
    /// Unsupported element names already reported.
    warned: HashSet<String>,
}

/// Timing shared by all clip-like elements.
#[derive(Clone, Copy, Debug)]
struct Timing {
    /// Position in the parent's time.
    offset: Micros,
    /// First instant of the element's own (source) time.
    start: Micros,
    duration: Micros,
}

fn timing(n: Node) -> Timing {
    let t = |a: &str| n.attribute(a).and_then(parse_time);
    Timing { offset: t("offset").unwrap_or(0), start: t("start").unwrap_or(0), duration: t("duration").unwrap_or(0) }
}

/// Elements that can sit in a storyline.
const CLIP_TAGS: [&str; 9] = ["asset-clip", "clip", "video", "audio", "title", "gap", "ref-clip", "sync-clip", "mc-clip"];

impl<'x> Importer<'_, 'x> {
    fn warn_once(&mut self, key: &str, item: &str, message: &str) {
        if self.warned.insert(key.to_string()) {
            self.report.add(item, message);
        }
    }

    fn check_format(&mut self, seq: Node) {
        let Some(fmt) = seq.attribute("format").and_then(|f| self.resources.get(f)).copied() else { return };
        if let Some(fd) = fmt.attribute("frameDuration").and_then(parse_time).filter(|d| *d > 0) {
            let ours = self.p.settings.frame_rate.frame_duration();
            if (fd - ours).abs() > 1 {
                self.report.add("sequence", format!("frame rate {:.3} fps differs from the project's; times are kept exactly", 1e6 / fd as f64));
            }
        }
        let size = (fmt.attribute("width").and_then(|w| w.parse::<u32>().ok()), fmt.attribute("height").and_then(|h| h.parse::<u32>().ok()));
        if let (Some(w), Some(h)) = size
            && (w, h) != (self.p.settings.width, self.p.settings.height)
        {
            self.report.add("sequence", format!("canvas {w}×{h} differs from the project's; project settings unchanged"));
        }
    }

    fn sequence(&mut self, node: Node<'x, 'x>, name: &str) -> Result<Sequence, ImportError> {
        let id = SequenceId(self.p.alloc_id());
        let spine_id = LaneId(self.p.alloc_id());
        let mut seq = Sequence::new(id, name, spine_id);
        let tc_start = node.attribute("tcStart").and_then(parse_time).unwrap_or(0);
        let mut transitions = Vec::new();
        if let Some(spine) = node.children().find(|n| n.has_tag_name("spine")) {
            let mut cursor = 0;
            for el in spine.children().filter(|n| n.is_element()) {
                let tag = el.tag_name().name();
                let t = timing(el);
                if tag == "transition" {
                    transitions.push((t.offset - tc_start, t.duration, el.attribute("name").unwrap_or("Cross Dissolve").to_string()));
                    continue;
                }
                if !CLIP_TAGS.contains(&tag) {
                    self.unsupported(el);
                    continue;
                }
                if t.duration <= 0 {
                    continue;
                }
                let mut at = t.offset - tc_start;
                if at > cursor {
                    seq.clips.push(Clip::gap(ClipId(self.p.alloc_id()), spine_id, cursor, at - cursor));
                } else if at < cursor {
                    self.warn_once("overlap", el.attribute("name").unwrap_or(tag), "storyline items overlap; placed end to end");
                    at = cursor;
                }
                let clip = self.element(el, spine_id, at, true)?;
                seq.clips.push(clip.unwrap_or_else(|| Clip::gap(ClipId(self.p.alloc_id()), spine_id, at, t.duration)));
                cursor = at + t.duration;
                // Connected clips hang off this element, in its local time.
                self.connected(&mut seq, el, at - t.start)?;
            }
        }
        for m in node.children().filter(|n| n.has_tag_name("marker") || n.has_tag_name("chapter-marker")) {
            let mk = self.marker(m, tc_start);
            seq.markers.push(mk);
        }
        seq.normalize();
        let ts = self.transitions(&seq, transitions);
        fit_transitions(self.p, &self.media, &self.nested, &mut seq, ts, &mut self.report);
        Ok(seq)
    }

    /// Places the connected clips and storylines of `parent`; `origin` is
    /// the sequence time of the parent's local time zero.
    fn connected(&mut self, seq: &mut Sequence, parent: Node<'x, 'x>, origin: Micros) -> Result<(), ImportError> {
        for el in parent.children().filter(|n| n.is_element() && n.attribute("lane").is_some()) {
            let lane: i32 = el.attribute("lane").and_then(|l| l.parse().ok()).unwrap_or(1);
            if lane == 0 {
                continue;
            }
            let tag = el.tag_name().name();
            let t = timing(el);
            if tag == "spine" {
                self.warn_once("storyline", "secondary storyline", "secondary storylines are flattened onto a lane");
                let mut cursor = origin + t.offset;
                for child in el.children().filter(|n| n.is_element()) {
                    let ct = timing(child);
                    if child.has_tag_name("transition") {
                        self.warn_once("storyline-transition", "transition", "transitions in secondary storylines are not imported");
                        continue;
                    }
                    if !CLIP_TAGS.contains(&child.tag_name().name()) || ct.duration <= 0 {
                        continue;
                    }
                    if !child.has_tag_name("gap") {
                        self.place(seq, child, lane, cursor)?;
                    }
                    cursor += ct.duration;
                }
                continue;
            }
            if !CLIP_TAGS.contains(&tag) {
                self.unsupported(el);
                continue;
            }
            if tag == "gap" || t.duration <= 0 {
                continue;
            }
            self.place(seq, el, lane, origin + t.offset)?;
        }
        Ok(())
    }

    /// Puts a connected element on the first free lane at or beyond `lane`
    /// (moving away from the spine), creating lanes as needed.
    fn place(&mut self, seq: &mut Sequence, el: Node<'x, 'x>, lane: i32, at: Micros) -> Result<(), ImportError> {
        let range = TimeRange::new(at, timing(el).duration);
        let step = lane.signum();
        let mut pos = lane;
        let lane_id = loop {
            let id = self.lane_at(seq, pos);
            if !seq.clips.iter().any(|c| c.lane == id && c.range().overlaps(&range)) {
                break id;
            }
            pos += step;
        };
        if let Some(c) = self.element(el, lane_id, at, false)? {
            seq.clips.push(c);
        }
        Ok(())
    }

    fn lane_at(&mut self, seq: &mut Sequence, pos: i32) -> LaneId {
        loop {
            if let Some(l) = seq.lane_at_position(pos) {
                return l.id;
            }
            let id = LaneId(self.p.alloc_id());
            if pos > 0 {
                let n = seq.lanes.iter().filter(|l| l.kind == LaneKind::Visual).count();
                seq.lanes.insert(0, Lane::new(id, LaneKind::Visual, format!("Video {}", n + 1)));
            } else {
                let n = seq.lanes.iter().filter(|l| l.kind == LaneKind::Audio).count();
                seq.lanes.push(Lane::new(id, LaneKind::Audio, format!("Audio {}", n + 1)));
            }
        }
    }

    /// Converts one clip-like element placed at sequence time `at`.
    /// Returns `None` for elements that become empty space.
    fn element(&mut self, el: Node<'x, 'x>, lane: LaneId, at: Micros, on_spine: bool) -> Result<Option<Clip>, ImportError> {
        let tag = el.tag_name().name();
        let t = timing(el);
        let name = el.attribute("name").unwrap_or(tag).to_string();
        let id = ClipId(self.p.alloc_id());
        let mut clip = match tag {
            "gap" => {
                if !on_spine {
                    return Ok(None);
                }
                Clip::gap(id, lane, at, t.duration)
            }
            "asset-clip" | "video" | "audio" => {
                let Some(r) = el.attribute("ref") else { return Ok(None) };
                match self.source(r, &name, tag == "audio", tag == "video")? {
                    Some((source, asset_start)) => {
                        let mut c = Clip::new(id, lane, at, t.duration, source);
                        c.source_in = (t.start - asset_start).max(0);
                        if tag == "asset-clip"
                            && let ClipSource::Media(m) = &mut c.source
                        {
                            match el.attribute("srcEnable") {
                                Some("video") => m.audio = false,
                                Some("audio") => m.video = false,
                                _ => {}
                            }
                        }
                        c
                    }
                    None => return Ok(None),
                }
            }
            "clip" | "sync-clip" => {
                if tag == "sync-clip" {
                    self.warn_once("sync-clip", &name, "synchronised clips are imported from their first picture source only");
                }
                // The content is the first child with a media reference; its
                // own offset is in the container's local time.
                let inner = el.children().find(|n| (n.has_tag_name("video") || n.has_tag_name("asset-clip") || n.has_tag_name("audio")) && n.attribute("lane").is_none() && n.attribute("ref").is_some());
                let Some(inner) = inner else {
                    self.warn_once(&format!("empty-{name}"), &name, "container clip without media; left empty");
                    return Ok(None);
                };
                let it = timing(inner);
                match self.source(inner.attribute("ref").expect("checked"), &name, inner.has_tag_name("audio"), inner.has_tag_name("video"))? {
                    Some((source, asset_start)) => {
                        let mut c = Clip::new(id, lane, at, t.duration, source);
                        c.source_in = (it.start + (t.start - it.offset) - asset_start).max(0);
                        c
                    }
                    None => return Ok(None),
                }
            }
            "title" => {
                let text: String = el.descendants().filter(|n| n.has_tag_name("text-style") && n.parent().is_some_and(|p| p.has_tag_name("text"))).filter_map(|n| n.text()).collect();
                self.warn_once("title-style", "titles", "title fonts, styles and templates are not imported; text kept");
                Clip::new(id, lane, at, t.duration, ClipSource::Generator(Generator::text(text)))
            }
            "ref-clip" => {
                let Some(r) = el.attribute("ref") else { return Ok(None) };
                let Some((nested, tc)) = self.compound(r)? else {
                    self.report.add(&name, "compound clip resource missing; left empty");
                    return Ok(None);
                };
                let mut c = Clip::new(id, lane, at, t.duration, ClipSource::Sequence(SequenceRef { sequence: nested, extra: Default::default() }));
                c.source_in = (t.start - tc).max(0);
                c
            }
            _ => {
                self.unsupported(el);
                return Ok(None);
            }
        };
        clip.name = if tag == "gap" { String::new() } else { name.clone() };
        clip.enabled = el.attribute("enabled") != Some("0");
        for child in el.children().filter(|n| n.is_element() && n.attribute("lane").is_none()) {
            match child.tag_name().name() {
                "marker" | "chapter-marker" => {
                    let mut m = self.marker(child, 0);
                    // Marker times are in the element's local time.
                    m.time -= t.start;
                    clip.markers.push(m);
                }
                "video" | "audio" | "asset-clip" | "text" | "spine" | "note" | "conform-rate" => {}
                other => {
                    let msg = match other {
                        "timeMap" => "retiming is not imported; clip plays at normal speed",
                        "filter-video" | "filter-audio" | "filter-video-mask" => "effects are not imported",
                        o if o.starts_with("adjust-") => "transform and audio adjustments are not imported",
                        "keyframeAnimation" | "param" => "animated parameters are not imported",
                        _ => "element not imported",
                    };
                    self.warn_once(&format!("{name}/{other}"), &name, &format!("<{other}>: {msg}"));
                }
            }
        }
        // Trust the timeline when it uses more media than the asset says.
        if let ClipSource::Media(r) = &clip.source
            && let Some(m) = self.media.iter_mut().find(|m| m.id == r.media)
            && m.info.kind != MediaKind::Image
            && m.info.duration.is_some_and(|d| d < clip.source_out())
        {
            m.info.duration = Some(clip.source_out());
            self.report.add(&name, "clip uses more media than the asset's duration; duration extended");
        }
        Ok(Some(clip))
    }

    /// The clip source for resource `r`, with the asset's start time.
    fn source(&mut self, r: &str, name: &str, audio_only: bool, video_only: bool) -> Result<Option<(ClipSource, Micros)>, ImportError> {
        let Some(res) = self.resources.get(r).copied() else {
            self.report.add(name, format!("resource `{r}` is missing; left empty"));
            return Ok(None);
        };
        match res.tag_name().name() {
            "asset" => {
                let start = res.attribute("start").and_then(parse_time).unwrap_or(0);
                let media = self.media_for(res);
                let mut m = MediaRef::new(media);
                m.video = res.attribute("hasVideo") == Some("1");
                m.audio = res.attribute("hasAudio") == Some("1");
                if audio_only {
                    m.video = false;
                }
                if video_only {
                    m.audio = false;
                }
                Ok(Some((ClipSource::Media(m), start)))
            }
            "effect" => {
                self.report.add(name, format!("generator `{}` imported as a plain solid", res.attribute("name").unwrap_or("effect")));
                Ok(Some((ClipSource::Generator(Generator::new(GeneratorKind::Solid)), 0)))
            }
            "media" => match self.compound(r)? {
                Some((s, tc)) => Ok(Some((ClipSource::Sequence(SequenceRef { sequence: s, extra: Default::default() }), tc))),
                None => Ok(None),
            },
            other => {
                self.report.add(name, format!("resource kind `{other}` is not supported; left empty"));
                Ok(None)
            }
        }
    }

    /// Imports a `media` resource holding a sequence (once per resource).
    fn compound(&mut self, r: &str) -> Result<Option<(SequenceId, Micros)>, ImportError> {
        let Some(res) = self.resources.get(r).copied() else { return Ok(None) };
        let Some(seq_node) = res.children().find(|n| n.has_tag_name("sequence")) else {
            if res.children().any(|n| n.has_tag_name("multicam")) {
                self.report.add(res.attribute("name").unwrap_or(r), "multicam clips are not supported");
            }
            return Ok(None);
        };
        let tc = seq_node.attribute("tcStart").and_then(parse_time).unwrap_or(0);
        if let Some(id) = self.compound.get(r) {
            return Ok(Some((*id, tc)));
        }
        let name = res.attribute("name").unwrap_or("Compound").to_string();
        let nested = self.sequence(seq_node, &name)?;
        let id = nested.id;
        self.compound.insert(r.to_string(), id);
        self.nested.push(nested);
        Ok(Some((id, tc)))
    }

    fn media_for(&mut self, asset: Node) -> MediaId {
        let src = asset
            .children()
            .find(|n| n.has_tag_name("media-rep") && n.attribute("kind").is_none_or(|k| k == "original-media"))
            .and_then(|n| n.attribute("src"))
            .or_else(|| asset.attribute("src"))
            .unwrap_or_default();
        let path = url_to_path(src);
        if let Some(m) = self.p.media.iter().chain(&self.media).find(|m| m.path == path) {
            return m.id;
        }
        let has_video = asset.attribute("hasVideo") == Some("1");
        let has_audio = asset.attribute("hasAudio") == Some("1");
        let duration = asset.attribute("duration").and_then(parse_time).filter(|d| *d > 0);
        let fmt = asset.attribute("format").and_then(|f| self.resources.get(f)).copied();
        let kind = if !has_video {
            MediaKind::Audio
        } else if duration.is_none() {
            MediaKind::Image
        } else {
            MediaKind::Video
        };
        let video = has_video.then(|| VideoInfo {
            width: fmt.and_then(|f| f.attribute("width")).and_then(|w| w.parse().ok()).unwrap_or(0),
            height: fmt.and_then(|f| f.attribute("height")).and_then(|h| h.parse().ok()).unwrap_or(0),
            ..Default::default()
        });
        let audio = has_audio.then(|| AudioInfo {
            sample_rate: asset.attribute("audioRate").and_then(parse_rate).unwrap_or(48_000),
            channels: asset.attribute("audioChannels").and_then(|c| c.parse().ok()).unwrap_or(2),
            ..Default::default()
        });
        let id = MediaId(self.p.alloc_id());
        let info = MediaInfo { kind, duration, video, audio, ..Default::default() };
        let mut item = MediaItem::new(id, path.clone(), info);
        if let Some(n) = asset.attribute("name") {
            item.name = n.to_string();
        }
        self.warn_once("probe", "media", "imported media was not probed; codec details are filled in when the files are re-probed");
        self.media.push(item);
        id
    }

    fn marker(&mut self, n: Node, base: Micros) -> Marker {
        let t = timing(n);
        let mut m = Marker::new(MarkerId(self.p.alloc_id()), t.start - base, n.attribute("value").unwrap_or_default());
        m.duration = t.duration;
        m.note = n.attribute("note").unwrap_or_default().to_string();
        m
    }

    fn unsupported(&mut self, el: Node) {
        let tag = el.tag_name().name();
        let msg = if tag == "mc-clip" { "multicam clips are not supported; left empty" } else { "element not supported; skipped" };
        self.warn_once(tag, &format!("<{tag}>"), msg);
    }

    /// Turns spine `(offset, duration, name)` transitions into transitions
    /// at the nearest cut.
    fn transitions(&mut self, seq: &Sequence, raw: Vec<(Micros, Micros, String)>) -> Vec<Transition> {
        let spine = seq.spine_clips();
        let mut out: Vec<Transition> = Vec::new();
        for (offset, duration, name) in raw {
            let mid = offset + duration / 2;
            let cut = spine.windows(2).filter(|w| w[0].end() >= offset && w[0].end() <= offset + duration).min_by_key(|w| (w[0].end() - mid).abs());
            let Some(w) = cut else {
                self.report.add(format!("transition {name}"), "no cut under the transition; dropped");
                continue;
            };
            if out.iter().any(|t| t.left == w[0].id) {
                continue;
            }
            let cut_at = w[0].end();
            let align = if offset == cut_at {
                TransitionAlign::StartAtCut
            } else if offset + duration == cut_at {
                TransitionAlign::EndAtCut
            } else {
                TransitionAlign::Centre
            };
            let lname = name.to_lowercase();
            let kind = if lname.contains("dissolve") || lname.contains("cross") {
                "dissolve".to_string()
            } else {
                self.report.add(format!("transition {name}"), "imported as a dissolve");
                "dissolve".to_string()
            };
            out.push(Transition { id: TransitionId(self.p.alloc_id()), left: w[0].id, right: w[1].id, kind, duration, align, params: Default::default(), extra: Default::default() });
        }
        out
    }
}

/// Parses audio rates such as `"48k"`, `"44.1k"` or `"48000"`.
fn parse_rate(s: &str) -> Option<u32> {
    match s.strip_suffix('k') {
        Some(k) => k.parse::<f64>().ok().map(|v| (v * 1000.0).round() as u32),
        None => s.parse().ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times() {
        assert_eq!(parse_time("0s"), Some(0));
        assert_eq!(parse_time("5s"), Some(5_000_000));
        assert_eq!(parse_time("1001/30000s"), Some(33_367));
        assert_eq!(parse_time("3600/1s"), Some(3_600_000_000));
        assert_eq!(parse_time("1.5s"), Some(1_500_000));
        assert_eq!(parse_time("12"), None);
        assert_eq!(parse_rate("44.1k"), Some(44_100));
    }
}
