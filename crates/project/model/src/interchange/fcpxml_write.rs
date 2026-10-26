// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! FCPXML 1.10 export (read by Final Cut Pro and most tools that
//! accept FCPXML).
//!
//! * One `format` for the project (frame duration, size, colour space).
//! * One `asset` per media file, its `start` at the file's own timecode,
//!   with a `media-rep` file URL; compound clips become `media` resources
//!   holding their sequence and are placed with `ref-clip`.
//! * The spine holds the main lane in order, with `gap`s; clips on other
//!   lanes are connected to the spine element under their start, with
//!   `lane` = the lane's position (positive above, negative below).
//! * Titles use the Basic Title effect; dissolves (and every other
//!   transition, reported) use Cross Dissolve.
//! * Clip and sequence markers become `marker`s on their clip or the spine
//!   element under them; constant speed and reverse become a `timeMap`.
//!
//! Effects, grades, masks, transforms and keyframes are listed in the
//! report. All times are whole frames written as exact rationals.

use super::{Report, path_to_url};
use crate::clip::{Clip, ClipSource, GeneratorKind, Speed};
use crate::edit::EditError;
use crate::id::{MediaId, SequenceId};
use crate::project::{ColourSpace, Project};
use crate::sequence::{Marker, Sequence};
use crate::time::{FrameRate, Micros};
use crate::timecode::Timecode;
use std::collections::BTreeMap;
use std::fmt::Write;

/// Export settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FcpxmlOptions {
    /// Timecode of the sequence start (default 01:00:00:00).
    pub start_timecode: Micros,
}

impl Default for FcpxmlOptions {
    fn default() -> Self {
        Self { start_timecode: 3600 * crate::time::SECOND }
    }
}

const BASIC_TITLE: &str = ".../Titles.localized/Bumper:Opener.localized/Basic Title.localized/Basic Title.moti";
const CROSS_DISSOLVE: &str = "FxPlug:4731E73A-8DAC-4113-9A30-AE85B1761265";

fn gcd(a: i128, b: i128) -> i128 {
    if b == 0 { a.abs() } else { gcd(b, a % b) }
}

/// Time as an exact FCPXML rational (whole frames at `rate`).
fn rational(frames: i64, rate: FrameRate) -> String {
    if frames == 0 {
        return "0s".into();
    }
    let num = frames as i128 * rate.den as i128;
    let den = rate.num as i128;
    let g = gcd(num, den).max(1);
    let (n, d) = (num / g, den / g);
    if d == 1 { format!("{n}s") } else { format!("{n}/{d}s") }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

struct Writer<'a> {
    p: &'a Project,
    rate: FrameRate,
    report: Report,
    resources: String,
    next: u32,
    assets: BTreeMap<MediaId, String>,
    compounds: BTreeMap<SequenceId, String>,
    title_effect: Option<String>,
    dissolve_effect: Option<String>,
    styles: u32,
}

impl Writer<'_> {
    fn id(&mut self) -> String {
        self.next += 1;
        format!("r{}", self.next)
    }

    fn f(&self, t: Micros) -> i64 {
        self.rate.micros_to_frame_round(t)
    }

    fn t(&self, t: Micros) -> String {
        rational(self.f(t), self.rate)
    }

    fn asset(&mut self, m: MediaId) -> Option<String> {
        if let Some(id) = self.assets.get(&m) {
            return Some(id.clone());
        }
        let item = self.p.media_item(m)?;
        let id = self.id();
        let start = self.t(item.timecode_start(self.rate));
        let dur = item.info.duration.map_or("0s".into(), |d| self.t(d));
        let has_v = item.has_video();
        let has_a = item.has_audio();
        let mut attrs = format!("id=\"{id}\" name=\"{}\" start=\"{start}\" duration=\"{dur}\" hasVideo=\"{}\" hasAudio=\"{}\"", esc(&item.name), has_v as u8, has_a as u8);
        if has_v {
            attrs.push_str(" format=\"r1\" videoSources=\"1\"");
        }
        if let Some(a) = &item.info.audio {
            write!(attrs, " audioSources=\"1\" audioChannels=\"{}\" audioRate=\"{}\"", a.channels.max(1), a.sample_rate.max(1)).ok();
        }
        writeln!(self.resources, "    <asset {attrs}>").ok();
        writeln!(self.resources, "      <media-rep kind=\"original-media\" src=\"{}\"/>", esc(&path_to_url(&item.path))).ok();
        if let Some(reel) = &item.reel {
            writeln!(self.resources, "      <metadata><md key=\"com.apple.proapps.studio.reel\" value=\"{}\"/></metadata>", esc(reel)).ok();
        }
        writeln!(self.resources, "    </asset>").ok();
        self.assets.insert(m, id.clone());
        Some(id)
    }

    fn effect(&mut self, which: &str) -> String {
        let slot = if which == "title" { &self.title_effect } else { &self.dissolve_effect };
        if let Some(id) = slot {
            return id.clone();
        }
        let id = self.id();
        let (name, uid) = if which == "title" { ("Basic Title", BASIC_TITLE) } else { ("Cross Dissolve", CROSS_DISSOLVE) };
        writeln!(self.resources, "    <effect id=\"{id}\" name=\"{name}\" uid=\"{uid}\"/>").ok();
        if which == "title" {
            self.title_effect = Some(id.clone());
        } else {
            self.dissolve_effect = Some(id.clone());
        }
        id
    }

    fn compound(&mut self, s: SequenceId, tc_start: Micros) -> Option<String> {
        if let Some(id) = self.compounds.get(&s) {
            return Some(id.clone());
        }
        let seq = self.p.sequence(s)?;
        let id = self.id();
        self.compounds.insert(s, id.clone());
        let body = self.sequence(seq, tc_start, "        ");
        writeln!(self.resources, "    <media id=\"{id}\" name=\"{}\">", esc(&seq.name)).ok();
        self.resources.push_str(&body);
        writeln!(self.resources, "    </media>").ok();
        Some(id)
    }

    /// `<sequence>` with its spine.
    fn sequence(&mut self, s: &Sequence, tc_start: Micros, ind: &str) -> String {
        let tcf = if Timecode::for_rate(self.rate).is_drop_frame() { "DF" } else { "NDF" };
        let mut out = String::new();
        let dur = self.t(s.duration());
        // The start is a timecode label: 01:00:00:00 is frame 86400 at
        // 23.976, not an hour of real time; offsets count real frames on.
        let tc_start = self.rate.frame_to_micros(Timecode::for_rate(self.rate).label_frames(tc_start));
        writeln!(out, "{ind}<sequence format=\"r1\" duration=\"{dur}\" tcStart=\"{}\" tcFormat=\"{tcf}\" audioLayout=\"stereo\" audioRate=\"48k\">", self.t(tc_start)).ok();
        writeln!(out, "{ind}  <spine>").ok();
        let spine = s.spine_id();
        let spine_clips: Vec<&Clip> = s.spine_clips();
        let others: Vec<&Clip> = s.clips.iter().filter(|c| c.lane != spine).collect();
        let mut attached: Vec<Vec<&Clip>> = vec![Vec::new(); spine_clips.len() + 1];
        let mut markers: Vec<Vec<&Marker>> = vec![Vec::new(); spine_clips.len() + 1];
        let under = |t: Micros| spine_clips.iter().position(|c| t >= c.start && t < c.end()).unwrap_or(spine_clips.len());
        for c in &others {
            attached[under(c.start)].push(c);
        }
        for m in &s.markers {
            markers[under(m.time)].push(m);
        }
        let mut cursor = 0;
        for (i, c) in spine_clips.iter().enumerate() {
            if let Some(t) = s.transitions.iter().find(|t| t.right == c.id) {
                let (pre, _) = t.split();
                let eff = self.effect("dissolve");
                if t.kind != "dissolve" {
                    self.report.add(format!("transition {}", t.kind), "exported as a cross dissolve");
                }
                writeln!(out, "{ind}    <transition name=\"Cross Dissolve\" offset=\"{}\" duration=\"{}\">", self.t(tc_start + c.start - pre), self.t(t.duration)).ok();
                writeln!(out, "{ind}      <filter-video ref=\"{eff}\" name=\"Cross Dissolve\"/>").ok();
                writeln!(out, "{ind}    </transition>").ok();
            }
            let local_start = self.local_start(c);
            let inner = self.connected(s, &attached[i], c.start, local_start, &markers[i], ind);
            out.push_str(&self.element(c, Some(tc_start + c.start), None, local_start, &inner, &format!("{ind}    ")));
            cursor = c.end();
        }
        // Clips past the end of the spine hang off a trailing gap.
        if !attached[spine_clips.len()].is_empty() || !markers[spine_clips.len()].is_empty() {
            let end = attached[spine_clips.len()].iter().map(|c| c.end()).chain(markers[spine_clips.len()].iter().map(|m| m.time + 1)).max().unwrap_or(cursor);
            let inner = self.connected(s, &attached[spine_clips.len()], cursor, 0, &markers[spine_clips.len()], ind);
            writeln!(out, "{ind}    <gap name=\"Gap\" offset=\"{}\" duration=\"{}\" start=\"0s\">", self.t(tc_start + cursor), self.t((end - cursor).max(self.rate.frame_duration()))).ok();
            out.push_str(&inner);
            writeln!(out, "{ind}    </gap>").ok();
        }
        writeln!(out, "{ind}  </spine>").ok();
        writeln!(out, "{ind}</sequence>").ok();
        out
    }

    /// Timecode 01:00:00:00 as a time (frames at the real rate).
    fn hour(&self) -> Micros {
        self.rate.frame_to_micros(Timecode::for_rate(self.rate).label_frames(3600 * crate::time::SECOND))
    }

    /// The element's own `start` (its local time at its first frame).
    fn local_start(&self, c: &Clip) -> Micros {
        match &c.source {
            ClipSource::Media(r) => self.p.media_item(r.media).map_or(0, |m| m.timecode_start(self.rate)) + c.source_in,
            ClipSource::Sequence(_) => self.hour() + c.source_in,
            ClipSource::Generator(_) => self.hour(),
            ClipSource::Gap => 0,
        }
    }

    fn connected(&mut self, seq: &Sequence, clips: &[&Clip], parent_start: Micros, parent_local: Micros, markers: &[&Marker], ind: &str) -> String {
        let mut out = String::new();
        for m in markers {
            let at = parent_local + (m.time - parent_start);
            writeln!(out, "{ind}      <marker start=\"{}\" duration=\"{}\" value=\"{}\"{}/>", self.t(at), self.t(m.duration.max(self.rate.frame_duration())), esc(&m.name), note_attr(&m.note)).ok();
        }
        for c in clips {
            let pos = seq.lane_position(c.lane).unwrap_or(1);
            let offset = parent_local + (c.start - parent_start);
            let local = self.local_start(c);
            out.push_str(&self.element(c, Some(offset), Some(pos), local, "", &format!("{ind}      ")));
        }
        out
    }

    /// One clip element. `offset` is in the parent's time.
    fn element(&mut self, c: &Clip, offset: Option<Micros>, lane: Option<i32>, local: Micros, inner: &str, ind: &str) -> String {
        let mut out = String::new();
        let name = if c.name.is_empty() { None } else { Some(c.name.clone()) };
        let off = offset.map(|o| format!(" offset=\"{}\"", self.t(o))).unwrap_or_default();
        let lane_attr = lane.map(|l| format!(" lane=\"{l}\"")).unwrap_or_default();
        let dur = self.t(c.duration);
        let enabled = if c.enabled { "" } else { " enabled=\"0\"" };
        self.note_lossy(c);
        let mut body = String::new();
        for m in &c.markers {
            writeln!(body, "{ind}  <marker start=\"{}\" duration=\"{}\" value=\"{}\"{}/>", self.t(local + m.time), self.t(m.duration.max(self.rate.frame_duration())), esc(&m.name), note_attr(&m.note)).ok();
        }
        if let Some(tm) = self.time_map(c, local) {
            for line in tm.lines() {
                writeln!(body, "{ind}  {line}").ok();
            }
        }
        body.push_str(inner);
        match &c.source {
            ClipSource::Gap => {
                writeln!(out, "{ind}<gap name=\"Gap\"{off} duration=\"{dur}\" start=\"0s\">").ok();
                out.push_str(&body);
                writeln!(out, "{ind}</gap>").ok();
            }
            ClipSource::Media(r) => {
                let Some(asset) = self.asset(r.media) else {
                    self.report.add(c.name.clone(), "media missing; exported as a gap");
                    writeln!(out, "{ind}<gap name=\"Gap\"{off} duration=\"{dur}\" start=\"0s\"/>").ok();
                    return out;
                };
                let item = self.p.media_item(r.media);
                let src_en = match (r.video && item.is_some_and(|m| m.has_video()), r.audio && item.is_some_and(|m| m.has_audio())) {
                    (true, false) => " srcEnable=\"video\"",
                    (false, true) => " srcEnable=\"audio\"",
                    _ => "",
                };
                let n = name.unwrap_or_else(|| item.map_or("Clip".into(), |m| m.name.clone()));
                writeln!(out, "{ind}<asset-clip ref=\"{asset}\"{lane_attr}{off} name=\"{}\" start=\"{}\" duration=\"{dur}\" format=\"r1\"{src_en}{enabled}>", esc(&n), self.t(local)).ok();
                out.push_str(&body);
                writeln!(out, "{ind}</asset-clip>").ok();
            }
            ClipSource::Sequence(s) => {
                let Some(media) = self.compound(s.sequence, 3600 * crate::time::SECOND) else {
                    writeln!(out, "{ind}<gap name=\"Gap\"{off} duration=\"{dur}\" start=\"0s\"/>").ok();
                    return out;
                };
                let n = name.unwrap_or_else(|| "Compound Clip".into());
                writeln!(out, "{ind}<ref-clip ref=\"{media}\"{lane_attr}{off} name=\"{}\" start=\"{}\" duration=\"{dur}\"{enabled}>", esc(&n), self.t(local)).ok();
                out.push_str(&body);
                writeln!(out, "{ind}</ref-clip>").ok();
            }
            ClipSource::Generator(g) if g.kind == GeneratorKind::Text => {
                let eff = self.effect("title");
                self.styles += 1;
                let ts = format!("ts{}", self.styles);
                let text = g.plain_text();
                let n = name.unwrap_or_else(|| text.chars().take(40).collect());
                writeln!(out, "{ind}<title ref=\"{eff}\"{lane_attr}{off} name=\"{}\" start=\"{}\" duration=\"{dur}\"{enabled}>", esc(&n), self.t(local)).ok();
                writeln!(out, "{ind}  <text><text-style ref=\"{ts}\">{}</text-style></text>", esc(&text)).ok();
                writeln!(out, "{ind}  <text-style-def id=\"{ts}\"><text-style font=\"Helvetica\" fontSize=\"63\" fontColor=\"1 1 1 1\" alignment=\"center\"/></text-style-def>").ok();
                out.push_str(&body);
                writeln!(out, "{ind}</title>").ok();
            }
            ClipSource::Generator(g) => {
                self.report.add(c.name.clone(), format!("{:?} generator exported as a gap", g.kind).to_lowercase());
                writeln!(out, "{ind}<gap name=\"Gap\"{off} duration=\"{dur}\" start=\"0s\">").ok();
                out.push_str(&body);
                writeln!(out, "{ind}</gap>").ok();
            }
        }
        out
    }

    fn time_map(&mut self, c: &Clip, local: Micros) -> Option<String> {
        let speed = match &c.speed {
            Speed::Constant(r) => *r,
            Speed::Curve(_) => {
                self.report.add(c.name.clone(), "speed curve exported as its average speed");
                c.source_len() as f64 / c.duration.max(1) as f64
            }
        };
        if speed == 1.0 && !c.reversed {
            return None;
        }
        let src_len = (c.duration as f64 * speed).round() as Micros;
        let (v0, v1) = if c.reversed { (local + src_len, local) } else { (local, local + src_len) };
        Some(format!(
            "<timeMap>\n  <timept time=\"{}\" value=\"{}\" interp=\"linear\"/>\n  <timept time=\"{}\" value=\"{}\" interp=\"linear\"/>\n</timeMap>",
            self.t(local),
            self.t(v0),
            self.t(local + c.duration),
            self.t(v1)
        ))
    }

    fn note_lossy(&mut self, c: &Clip) {
        if !c.effects.is_empty() || c.grade.is_some() || !c.masks.is_empty() || !c.keyframes.is_empty() || c.transform != Default::default() {
            let name = if c.name.is_empty() { format!("clip {}", c.id.0) } else { c.name.clone() };
            self.report.add(name, "effects, grade, masks, keyframes and transform are not exported");
        }
    }
}

fn note_attr(n: &str) -> String {
    if n.is_empty() { String::new() } else { format!(" note=\"{}\"", esc(n)) }
}

/// Exports `seq` (and the compound clips and media it uses) as FCPXML 1.10.
pub fn export(project: &Project, seq: SequenceId, opts: FcpxmlOptions) -> Result<(String, Report), EditError> {
    let s = project.sequence(seq).ok_or_else(|| EditError::NotFound(seq.to_string()))?;
    let rate = project.settings.frame_rate;
    let mut w = Writer { p: project, rate, report: Report::default(), resources: String::new(), next: 1, assets: BTreeMap::new(), compounds: BTreeMap::new(), title_effect: None, dissolve_effect: None, styles: 0 };
    let colour = match project.settings.colour_space {
        ColourSpace::Rec709 => "1-1-1 (Rec. 709)",
        ColourSpace::DisplayP3 => "12-1-1 (P3 D65)",
        ColourSpace::Rec2020Pq => "9-16-9 (Rec. 2020 PQ)",
        ColourSpace::Rec2020Hlg => "9-18-9 (Rec. 2020 HLG)",
    };
    let fmt = format!(
        "    <format id=\"r1\" name=\"FFVideoFormat{}p{}\" frameDuration=\"{}\" width=\"{}\" height=\"{}\" colorSpace=\"{colour}\"/>\n",
        project.settings.height,
        (rate.as_f64() * 100.0).round() as u64,
        rational(1, rate),
        project.settings.width,
        project.settings.height
    );
    let body = w.sequence(s, opts.start_timecode, "        ");
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE fcpxml>\n<fcpxml version=\"1.10\">\n  <resources>\n");
    out.push_str(&fmt);
    out.push_str(&w.resources);
    out.push_str("  </resources>\n  <library>\n    <event name=\"Zornov\">\n");
    writeln!(out, "      <project name=\"{}\">", esc(&s.name)).ok();
    out.push_str(&body);
    out.push_str("      </project>\n    </event>\n  </library>\n</fcpxml>\n");
    Ok((out, w.report))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rationals() {
        assert_eq!(rational(0, FrameRate::FPS_25), "0s");
        assert_eq!(rational(25, FrameRate::FPS_25), "1s");
        assert_eq!(rational(1, FrameRate::FPS_25), "1/25s");
        assert_eq!(rational(1, FrameRate::new(24000, 1001)), "1001/24000s");
        assert_eq!(rational(86400, FrameRate::new(24000, 1001)), "18018/5s");
        assert_eq!(esc("a<b&\"c\""), "a&lt;b&amp;&quot;c&quot;");
    }
}
