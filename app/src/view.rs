// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The UI's view of a project: plain structs the screens read every frame,
//! projected from the `studio-model` project after each change (see
//! `bridge`). Edits never write here directly: they go to the model as
//! `Edit`s and the view is rebuilt.
//!
//! Also holds the sample projects (built as views, then converted to
//! model projects) and small formatting helpers. Times are integer
//! microseconds like the model.

use eframe::egui::Color32;

pub type Time = i64;
pub const SEC: Time = 1_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipKind {
    Video,
    Audio,
    Title,
    Fx,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneKind {
    Visual,
    Spine,
    Audio,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lane {
    pub id: u32,
    pub name: String,
    pub kind: LaneKind,
    pub locked: bool,
    pub hidden: bool,
    pub muted: bool,
    pub solo: bool,
    pub height: f32,
    pub colour: Color32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    /// Offset of the clip centre from the canvas centre, in canvas pixels.
    pub x: f32,
    pub y: f32,
    /// Percent.
    pub scale: f32,
    /// Degrees.
    pub rotation: f32,
    /// Percent.
    pub opacity: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0, scale: 100.0, rotation: 0.0, opacity: 100.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grade {
    pub enabled: bool,
    pub exposure: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub temperature: f32,
    pub tint: f32,
}

impl Default for Grade {
    fn default() -> Self {
        Self { enabled: true, exposure: 0.0, contrast: 0.0, saturation: 0.0, temperature: 0.0, tint: 0.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioParams {
    pub enabled: bool,
    /// dB.
    pub volume: f32,
    /// -100 (left) .. 100 (right).
    pub pan: f32,
    /// Seconds.
    pub fade_in: f32,
    pub fade_out: f32,
}

impl Default for AudioParams {
    fn default() -> Self {
        Self { enabled: true, volume: 0.0, pan: 0.0, fade_in: 0.0, fade_out: 0.0 }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    pub id: u32,
    pub lane: u32,
    pub start: Time,
    pub dur: Time,
    pub src_in: Time,
    pub kind: ClipKind,
    pub name: String,
    pub media: Option<u32>,
    pub text: String,
    pub text_colour: Color32,
    pub transform: Transform,
    pub grade: Grade,
    pub audio: AudioParams,
    /// Percent.
    pub speed: f32,
    pub effects: Vec<Effect>,
    /// Entry motion preset (an `fx` motion id).
    pub motion_in: Option<String>,
    /// Look (a `colour` look slug).
    pub look: Option<String>,
    /// Keyframed properties (model property paths) and their key times,
    /// relative to the clip start.
    pub keys: Vec<(String, Vec<Time>)>,
    /// Text style preset (a `text` crate style id) for title clips.
    pub text_style: Option<String>,
    pub extra: Extra,
}

/// An effect on a clip: `fx` effect id plus its parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct Effect {
    pub id: u64,
    pub kind: String,
    pub enabled: bool,
    pub params: Vec<(String, f32)>,
}

impl Effect {
    pub fn new(kind: &str) -> Self {
        Self { id: 0, kind: kind.into(), enabled: true, params: Vec::new() }
    }
}

/// Less common clip settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extra {
    /// 0 none, 1 rectangle, 2 ellipse, 3 brush.
    pub mask: u8,
    pub feather: f32,
    pub mask_invert: bool,
    pub crop: [f32; 4],
    pub flip: bool,
    pub reverse: bool,
    /// Canvas-relative points (5–300, default 40).
    pub text_size: f32,
    pub letter_spacing: f32,
    pub text_align: u8,
    pub blend: u8,
    /// Entry motion length in seconds.
    pub motion_dur: f32,
}

impl Default for Extra {
    fn default() -> Self {
        Self { mask: 0, feather: 0.0, mask_invert: false, crop: [0.0; 4], flip: false, reverse: false, text_size: 40.0, letter_spacing: 0.0, text_align: 1, blend: 0, motion_dur: 0.5 }
    }
}

impl Clip {
    /// Key times of a property path, if it is keyframed.
    pub fn key_times(&self, path: &str) -> Option<&[Time]> {
        self.keys.iter().find(|(p, _)| p == path).map(|(_, t)| t.as_slice())
    }
    pub fn end(&self) -> Time {
        self.start + self.dur
    }
    pub fn is_visual(&self) -> bool {
        self.kind != ClipKind::Audio
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Transition {
    pub id: u64,
    pub left: u32,
    pub right: u32,
    pub dur: Time,
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Marker {
    pub id: u64,
    pub time: Time,
    pub note: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    Video,
    Audio,
    Image,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MediaItem {
    pub id: u32,
    pub name: String,
    pub kind: MediaKind,
    pub dur: Time,
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    /// Seed for the generated thumbnail and frames.
    pub seed: u32,
    /// Optional real file on disk (MP4) to decode frames from.
    pub path: Option<std::path::PathBuf>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Project {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    pub background: Color32,
    pub lanes: Vec<Lane>,
    pub clips: Vec<Clip>,
    pub transitions: Vec<Transition>,
    pub markers: Vec<Marker>,
    pub media: Vec<MediaItem>,
    pub range: Option<(Time, Time)>,
    pub next_id: u32,
}

impl Project {
    pub fn duration(&self) -> Time {
        self.clips.iter().map(Clip::end).max().unwrap_or(0)
    }
    pub fn lane(&self, id: u32) -> Option<&Lane> {
        self.lanes.iter().find(|l| l.id == id)
    }
    pub fn lane_mut(&mut self, id: u32) -> Option<&mut Lane> {
        self.lanes.iter_mut().find(|l| l.id == id)
    }
    pub fn clip(&self, id: u32) -> Option<&Clip> {
        self.clips.iter().find(|c| c.id == id)
    }
    pub fn clip_mut(&mut self, id: u32) -> Option<&mut Clip> {
        self.clips.iter_mut().find(|c| c.id == id)
    }
    pub fn media(&self, id: u32) -> Option<&MediaItem> {
        self.media.iter().find(|m| m.id == id)
    }
    pub fn spine(&self) -> Option<u32> {
        self.lanes.iter().find(|l| l.kind == LaneKind::Spine).map(|l| l.id)
    }
    pub fn alloc_id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id
    }

    /// Free lanes cannot hold overlapping clips. If `id` overlaps a
    /// neighbour, move it to the nearest lane of the same kind with room,
    /// adding a lane when none has.
    pub fn resolve_overlap(&mut self, id: u32) {
        let Some(c) = self.clip(id).cloned() else { return };
        let Some(lane) = self.lane(c.lane).cloned() else { return };
        if lane.kind == LaneKind::Spine {
            return;
        }
        let fits = |p: &Project, lane_id: u32| !p.clips.iter().any(|o| o.id != id && o.lane == lane_id && o.start < c.end() && c.start < o.end());
        if fits(self, c.lane) {
            return;
        }
        let here = self.lanes.iter().position(|l| l.id == c.lane).unwrap_or(0);
        let mut candidates: Vec<(usize, u32)> = self.lanes.iter().enumerate().filter(|(_, l)| l.kind == lane.kind && !l.locked).map(|(i, l)| (i.abs_diff(here), l.id)).collect();
        candidates.sort();
        let target = match candidates.into_iter().map(|(_, l)| l).find(|l| fits(self, *l)) {
            Some(l) => l,
            None => {
                // New lane next to the original: above for visuals, below
                // for audio.
                let new_id = self.alloc_id();
                let mut nl = lane.clone();
                nl.id = new_id;
                nl.name = format!("{} {}", if lane.kind == LaneKind::Audio { "Audio" } else { "Overlay" }, new_id);
                let at = if lane.kind == LaneKind::Audio { here + 1 } else { here };
                self.lanes.insert(at.min(self.lanes.len()), nl);
                new_id
            }
        };
        if let Some(c) = self.clip_mut(id) {
            c.lane = target;
        }
    }

    /// Topmost visible visual clip at `time` on non-hidden lanes, ordered
    /// top to bottom (the order the viewer stacks them).
    pub fn visual_stack_at(&self, time: Time) -> Vec<&Clip> {
        let mut out = Vec::new();
        for lane in &self.lanes {
            if lane.hidden || lane.kind == LaneKind::Audio {
                continue;
            }
            for c in &self.clips {
                if c.lane == lane.id && c.is_visual() && time >= c.start && time < c.end() {
                    out.push(c);
                }
            }
        }
        out
    }
}

// --- generators ----------------------------------------------------------

pub(crate) fn lane(id: u32, name: &str, kind: LaneKind, colour: Color32) -> Lane {
    let height = match kind {
        LaneKind::Spine => 52.0,
        LaneKind::Visual => 36.0,
        LaneKind::Audio => 40.0,
    };
    Lane { id, name: name.into(), kind, locked: false, hidden: false, muted: false, solo: false, height, colour }
}

pub(crate) fn clip(id: u32, lane: u32, start: f64, dur: f64, kind: ClipKind, name: &str, media: Option<u32>) -> Clip {
    Clip {
        id,
        lane,
        start: (start * SEC as f64) as Time,
        dur: (dur * SEC as f64) as Time,
        src_in: 0,
        kind,
        name: name.into(),
        media,
        text: String::new(),
        text_colour: Color32::WHITE,
        transform: Transform::default(),
        grade: Grade::default(),
        audio: AudioParams::default(),
        speed: 100.0,
        effects: Vec::new(),
        motion_in: None,
        look: None,
        keys: Vec::new(),
        text_style: None,
        extra: Extra::default(),
    }
}

fn media(id: u32, name: &str, kind: MediaKind, dur: f64, seed: u32) -> MediaItem {
    let (width, height) = if kind == MediaKind::Audio { (0, 0) } else if name.contains("vertical") { (1080, 1920) } else { (1920, 1080) };
    MediaItem { id, name: name.into(), kind, dur: (dur * SEC as f64) as Time, width, height, fps: 30.0, seed, path: None }
}

fn base_lanes() -> Vec<Lane> {
    vec![
        lane(5, "Titles", LaneKind::Visual, Color32::from_rgb(0xB4, 0x8C, 0xF0)),
        lane(4, "Overlay", LaneKind::Visual, Color32::from_rgb(0x7F, 0xA8, 0xE8)),
        lane(1, "Main", LaneKind::Spine, Color32::from_rgb(0x8B, 0x6C, 0xFF)),
        lane(2, "Voice", LaneKind::Audio, Color32::from_rgb(0x6C, 0xC6, 0xA0)),
        lane(3, "Music", LaneKind::Audio, Color32::from_rgb(0xE8, 0xB8, 0x6A)),
    ]
}

/// Seed for sample photo `i` of `content::SAMPLE_PHOTOS`.
pub const fn photo(i: u32) -> u32 {
    crate::thumbs::PHOTO_BIT | i
}

pub fn demo_media() -> Vec<MediaItem> {
    vec![
        media(101, "Lake at dawn", MediaKind::Video, 14.2, photo(0)),
        media(102, "Autumn road", MediaKind::Video, 9.6, photo(1)),
        media(103, "Latte art", MediaKind::Video, 12.0, photo(2)),
        media(104, "Night market", MediaKind::Video, 6.4, photo(3)),
        media(105, "Red canyon", MediaKind::Video, 18.0, photo(6)),
        media(106, "Arcade lights", MediaKind::Video, 10.5, photo(4)),
        media(107, "Kitchen dance", MediaKind::Video, 8.0, photo(8)),
        media(108, "Rice terraces", MediaKind::Video, 11.3, photo(12)),
        media(109, "Talking to camera", MediaKind::Video, 22.0, photo(11)),
        media(110, "Frisbee dog", MediaKind::Image, 5.0, photo(5)),
        media(111, "Milky Way", MediaKind::Image, 5.0, photo(7)),
        media(112, "Voiceover take 3", MediaKind::Audio, 58.0, 127),
        media(113, "Easy morning", MediaKind::Audio, 92.0, 131),
        media(114, "Street ambience", MediaKind::Audio, 40.0, 139),
    ]
}

/// The demo project: a short travel vlog, ~20 clips over 5 lanes.
pub fn demo() -> Project {
    use ClipKind::*;
    let mut clips = Vec::new();
    let spine = [
        (101, "Lake at dawn", 6.0),
        (109, "Talking to camera", 8.5),
        (102, "Autumn road", 5.2),
        (103, "Latte art", 7.0),
        (104, "Night market", 3.6),
        (106, "Arcade lights", 6.4),
        (108, "Rice terraces", 7.5),
        (105, "Red canyon", 9.0),
    ];
    let mut t = 0.0;
    for (i, (m, name, d)) in spine.iter().enumerate() {
        let mut c = clip(i as u32 + 1, 1, t, *d, Video, name, Some(*m));
        c.src_in = SEC;
        if i == 3 {
            c.grade.temperature = 18.0;
            c.grade.saturation = 10.0;
            c.look = Some("warm-afternoon".into());
        }
        clips.push(c);
        t += d;
    }
    let mut title = clip(20, 5, 0.6, 4.0, Title, "Weekend away", None);
    title.text = "Weekend away".into();
    title.extra.text_size = 84.0;
    title.transform.y = -300.0;
    title.motion_in = Some("rise".into());
    clips.push(title);
    let mut lower = clip(21, 5, 7.2, 4.5, Title, "Lower third: Ana", None);
    lower.text = "Ana · travel notes".into();
    lower.extra.text_size = 56.0;
    lower.transform = Transform { x: -560.0, y: 380.0, scale: 70.0, ..Default::default() };
    clips.push(lower);
    let mut t3 = clip(22, 5, 47.0, 4.0, Title, "Golden hour", None);
    t3.text = "Golden hour".into();
    t3.extra.text_size = 72.0;
    clips.push(t3);
    let mut pip = clip(23, 4, 21.5, 4.0, Video, "Frisbee dog", Some(110));
    pip.transform = Transform { x: 560.0, y: -260.0, scale: 36.0, rotation: -4.0, opacity: 100.0 };
    clips.push(pip);
    let mut map = clip(24, 4, 30.0, 5.0, Video, "Milky Way", Some(111));
    map.transform = Transform { x: -520.0, y: 220.0, scale: 40.0, ..Default::default() };
    clips.push(map);
    let mut adj = clip(25, 4, 40.0, 6.0, Fx, "Soft glow", None);
    adj.effects = vec![Effect::new("glow")];
    clips.push(adj);
    let mut vo1 = clip(30, 2, 6.0, 8.5, Audio, "Voiceover take 3", Some(112));
    vo1.audio.fade_in = 0.3;
    clips.push(vo1);
    let mut vo2 = clip(31, 2, 18.5, 12.0, Audio, "Voiceover take 3", Some(112));
    vo2.src_in = 9 * SEC;
    clips.push(vo2);
    let mut music = clip(32, 3, 0.0, 52.2, Audio, "Easy morning", Some(113));
    music.audio.volume = -12.0;
    music.audio.fade_out = 3.0;
    music.keys = vec![("audio.volume_db".into(), vec![0, 20 * SEC, 48 * SEC])];
    clips.push(music);
    let mut amb = clip(33, 2, 36.0, 10.0, Audio, "Street ambience", Some(114));
    amb.audio.volume = -18.0;
    clips.push(amb);

    Project {
        name: "Weekend away".into(),
        width: 1920,
        height: 1080,
        fps: 30.0,
        background: Color32::BLACK,
        lanes: base_lanes(),
        transitions: vec![
            Transition { id: 0, left: 1, right: 2, dur: SEC / 2, kind: "dissolve".into() },
            Transition { id: 0, left: 4, right: 5, dur: SEC / 3, kind: "push".into() },
            Transition { id: 0, left: 7, right: 8, dur: SEC, kind: "dip".into() },
        ],
        markers: vec![
            Marker { id: 0, time: 14 * SEC + SEC / 2, note: "Beat drop".into() },
            Marker { id: 0, time: 33 * SEC, note: "Add map animation".into() },
            Marker { id: 0, time: 47 * SEC, note: "Sunset".into() },
        ],
        media: demo_media(),
        clips,
        range: Some((6 * SEC, 30 * SEC)),
        next_id: 1000,
    }
}

pub fn empty() -> Project {
    Project {
        name: "Untitled project".into(),
        width: 1920,
        height: 1080,
        fps: 30.0,
        background: Color32::BLACK,
        lanes: base_lanes(),
        clips: vec![],
        transitions: vec![],
        markers: vec![],
        media: vec![],
        range: None,
        next_id: 1000,
    }
}

pub fn one_clip() -> Project {
    let mut p = empty();
    p.name = "Quick cut".into();
    p.media = vec![demo_media().remove(4)];
    p.clips.push(clip(1, 1, 0.0, 18.0, ClipKind::Video, "Sunset over the river", Some(105)));
    p
}

/// 200 clips, for the "looks intentional at scale" check.
pub fn big() -> Project {
    let mut p = demo();
    p.name = "Festival recap (200 clips)".into();
    p.clips.clear();
    p.transitions.clear();
    let media = p.media.clone();
    let video: Vec<&MediaItem> = media.iter().filter(|m| m.kind != MediaKind::Audio).collect();
    let mut id = 1;
    let mut rng = 12345u32;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        (rng % 1000) as f64 / 1000.0
    };
    let mut t = 0.0;
    for i in 0..110 {
        let m = video[i % video.len()];
        let d = 1.0 + next() * 3.0;
        p.clips.push(clip(id, 1, t, d, ClipKind::Video, m.name.trim_end_matches(".mp4"), Some(m.id)));
        id += 1;
        t += d;
    }
    let total = t;
    for (lane, kind, count) in [(4, ClipKind::Video, 30), (5, ClipKind::Title, 25), (2, ClipKind::Audio, 20), (3, ClipKind::Audio, 15)] {
        let mut t = next() * 4.0;
        let step = total / count as f64;
        for k in 0..count {
            let mut d = (1.5 + next() * step * 0.6).min(step * 0.9);
            if kind == ClipKind::Video {
                // Never longer than the footage it shows.
                d = d.min(video[k % video.len()].dur as f64 / SEC as f64 - 0.1);
            }
            let name = match kind {
                ClipKind::Title => format!("Caption {}", k + 1),
                ClipKind::Audio if lane == 3 => "Festival mix".into(),
                ClipKind::Audio => format!("Interview {}", k + 1),
                _ => format!("B-roll {}", k + 1),
            };
            let mut c = clip(id, lane, t, d, kind, &name, if kind == ClipKind::Audio { Some(if lane == 3 { 113 } else { 112 }) } else if kind == ClipKind::Video { Some(video[k % video.len()].id) } else { None });
            if kind == ClipKind::Title {
                c.text = name.clone();
                c.transform.y = 400.0;
                c.transform.scale = 60.0;
            }
            if kind == ClipKind::Video {
                c.transform.scale = 35.0;
                c.transform.x = 600.0;
                c.transform.y = -300.0;
            }
            p.clips.push(c);
            id += 1;
            t += step;
        }
    }
    p
}

pub fn by_name(name: &str) -> Project {
    match name {
        "empty" => empty(),
        "one" => one_clip(),
        "big" => big(),
        _ => demo(),
    }
}

// --- library catalogue ---------------------------------------------------






pub const ASSETS: &[&str] = &["Circle", "Rounded box", "Arrow", "Star", "Speech bubble", "Heart", "Underline", "Burst", "Badge", "Ring", "Spark", "Tag"];
pub const ASSET_GROUPS: &[&str] = &["All", "Shapes", "Stickers", "Frames"];

pub const SOUNDS: &[(&str, f32)] = &[("Morning walk", 94.0), ("Lo-fi bed", 128.0), ("Upbeat pop", 112.0), ("Whoosh", 1.2), ("Soft click", 0.3), ("Pop", 0.4), ("Low swoosh", 1.6), ("Rain ambience", 60.0), ("City hum", 45.0), ("Camera shutter", 0.5)];
pub const SOUND_GROUPS: &[&str] = &["All", "Music", "Effects", "Ambience"];

pub struct AiTool {
    pub name: &'static str,
    pub blurb: &'static str,
    pub model_mb: u32,
}

pub const AI_TOOLS: &[AiTool] = &[
    AiTool { name: "Auto captions", blurb: "Transcribe speech into editable captions.", model_mb: 148 },
    AiTool { name: "Remove background", blurb: "Cut the subject out of a clip.", model_mb: 44 },
    AiTool { name: "Voice cleanup", blurb: "Reduce noise and room echo in speech.", model_mb: 22 },
    AiTool { name: "Trim silences", blurb: "Find pauses and filler words to cut.", model_mb: 0 },
    AiTool { name: "Smart reframe", blurb: "Keep the subject in frame for 9:16.", model_mb: 31 },
    AiTool { name: "Scene cuts", blurb: "Split a long clip at every shot change.", model_mb: 9 },
    AiTool { name: "Beat markers", blurb: "Place markers on the music's beats.", model_mb: 0 },
    AiTool { name: "Smooth slow motion", blurb: "Create in-between frames for slow motion.", model_mb: 63 },
];

/// Recent projects on the Start screen.
pub struct Recent {
    pub name: &'static str,
    pub seconds: u32,
    /// What kind of video it is, shown under the name.
    pub edited: &'static str,
    pub seed: u32,
    pub aspect: (u32, u32),
}

pub const RECENTS: &[Recent] = &[
    Recent { name: "Weekend away", seconds: 52, edited: "Travel vlog", seed: photo(0), aspect: (16, 9) },
    Recent { name: "Pottery class reel", seconds: 28, edited: "Reel", seed: photo(9), aspect: (9, 16) },
    Recent { name: "Surf trip", seconds: 184, edited: "Vlog", seed: photo(10), aspect: (16, 9) },
    Recent { name: "Ramen in 60 seconds", seconds: 60, edited: "Short", seed: photo(14), aspect: (9, 16) },
    Recent { name: "Reading corner tour", seconds: 412, edited: "Tutorial", seed: photo(16), aspect: (16, 9) },
    Recent { name: "Café opening teaser", seconds: 15, edited: "Ad", seed: photo(2), aspect: (1, 1) },
    Recent { name: "Album snippet", seconds: 34, edited: "Music promo", seed: photo(13), aspect: (4, 5) },
    Recent { name: "Night market walk", seconds: 96, edited: "Vlog", seed: photo(3), aspect: (16, 9) },
];

/// `m:ss` or `h:mm:ss` for durations shown to people.
pub fn format_duration(t: Time) -> String {
    let s = (t / SEC).max(0);
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// Short clip duration for badges: `4.5s` under a minute, else `m:ss`.
pub fn format_short(t: Time) -> String {
    if t < 60 * SEC { format!("{:.1}s", t as f64 / SEC as f64) } else { format_duration(t) }
}

/// Timecode `hh:mm:ss:ff`.
pub fn timecode(t: Time, fps: f32) -> String {
    let fps_i = fps.round().max(1.0) as i64;
    let total_frames = ((t as f64 / SEC as f64) * fps as f64).floor() as i64;
    let f = total_frames % fps_i;
    let s = total_frames / fps_i;
    format!("{:02}:{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60, f)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_shape() {
        let p = demo();
        assert!((18..=24).contains(&p.clips.len()));
        assert_eq!(p.lanes.len(), 5);
        // Spine is packed.
        let spine = p.spine().unwrap();
        let mut s: Vec<_> = p.clips.iter().filter(|c| c.lane == spine).collect();
        s.sort_by_key(|c| c.start);
        for w in s.windows(2) {
            assert_eq!(w[0].end(), w[1].start);
        }
        assert_eq!(big().clips.len(), 200);
    }

    #[test]
    fn timecodes() {
        assert_eq!(timecode(0, 30.0), "00:00:00:00");
        assert_eq!(timecode(SEC + SEC / 2, 30.0), "00:00:01:15");
        assert_eq!(format_duration(125 * SEC), "2:05");
    }
}

#[cfg(test)]
mod overlap_tests {
    use super::*;

    #[test]
    fn overlapping_overlay_moves_to_a_free_lane() {
        let mut p = demo();
        // Put a clip right on top of "Pastry shop" (21.5 s on Overlay).
        let mut c = p.clip(23).unwrap().clone();
        c.id = 999;
        p.clips.push(c);
        p.resolve_overlap(999);
        let lane = p.clip(999).unwrap().lane;
        assert_ne!(lane, 4);
        let kind = p.lane(lane).unwrap().kind;
        assert_eq!(kind, LaneKind::Visual);
        let clash = p.clips.iter().any(|o| o.id != 999 && o.lane == lane && o.start < p.clip(999).unwrap().end() && p.clip(999).unwrap().start < o.end());
        assert!(!clash);
    }
}
