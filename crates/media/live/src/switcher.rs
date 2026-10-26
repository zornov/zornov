// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The vision mixer: several live sources, a program output and a
//! preview, scenes (full, picture-in-picture, side by side, grid),
//! transitions (cut, mix, dip, wipe, push), titles over the picture and
//! audio that follows the picture. One `step` makes one frame of program
//! and the sound that goes with it.

use crate::frame::{AudioBlock, Frame, peak_db};
use crate::source::{AudioSource, Consent, VideoSource, open_audio, open_video};
use crate::text::{TextOpts, draw_text, measure};
use studio_text::Align;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// What the picture is made of.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    Black,
    Full(usize),
    Pip { main: usize, inset: usize, corner: Corner, scale: f32 },
    Split { left: usize, right: usize },
    Grid(Vec<usize>),
}

impl Layout {
    /// How much of each source's sound goes with this picture.
    fn weights(&self, n: usize) -> Vec<f32> {
        let mut w = vec![0.0; n];
        let mut set = |i: usize, v: f32| {
            if i < n {
                w[i] = v;
            }
        };
        match self {
            Layout::Black => {}
            Layout::Full(s) => set(*s, 1.0),
            Layout::Pip { main, .. } => set(*main, 1.0),
            Layout::Split { left, right } => {
                set(*left, 0.7);
                set(*right, 0.7);
            }
            Layout::Grid(v) => v.iter().for_each(|&i| set(i, 0.7)),
        }
        w
    }

    pub fn sources(&self) -> Vec<usize> {
        match self {
            Layout::Black => vec![],
            Layout::Full(s) => vec![*s],
            Layout::Pip { main, inset, .. } => vec![*main, *inset],
            Layout::Split { left, right } => vec![*left, *right],
            Layout::Grid(v) => v.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transition {
    Cut,
    Mix,
    /// Through black.
    Dip,
    Wipe(Dir),
    Push(Dir),
}

impl Transition {
    pub fn parse(s: &str) -> Result<Transition, String> {
        Ok(match s {
            "cut" => Transition::Cut,
            "mix" | "dissolve" | "fade" => Transition::Mix,
            "dip" | "dip_to_black" | "fade_to_black" => Transition::Dip,
            "wipe" | "wipe_right" => Transition::Wipe(Dir::Right),
            "wipe_left" => Transition::Wipe(Dir::Left),
            "wipe_up" => Transition::Wipe(Dir::Up),
            "wipe_down" => Transition::Wipe(Dir::Down),
            "push" | "push_left" => Transition::Push(Dir::Left),
            "push_right" => Transition::Push(Dir::Right),
            "push_up" => Transition::Push(Dir::Up),
            "push_down" => Transition::Push(Dir::Down),
            o => return Err(format!("no transition \"{o}\"; cut, mix, dip, wipe[_left|_up|_down], push[_right|_up|_down]")),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TitleKind {
    /// Name and role, bottom left.
    Lower,
    /// Big, in the middle.
    Centre,
    /// A small mark in the top corner.
    Bug,
}

#[derive(Clone, Debug)]
pub struct Title {
    pub id: u32,
    pub kind: TitleKind,
    pub text: String,
    pub sub: String,
    shown_at: i64,
    hidden_at: Option<i64>,
    fade_us: i64,
    overlay: Option<Frame>,
}

struct Input {
    name: String,
    v: Box<dyn VideoSource>,
    a: Option<Box<dyn AudioSource>>,
    last: Option<Frame>,
    gain: f32,
    muted: bool,
    audio_read: u64,
    peak_db: f32,
}

struct Active {
    from: Layout,
    to: Layout,
    kind: Transition,
    start_us: i64,
    dur_us: i64,
}

/// One frame out.
pub struct Out {
    pub program: Frame,
    pub audio: AudioBlock,
    pub t_us: i64,
    /// Each source as it came in (when [`Switcher::want_iso`] is on).
    pub iso: Vec<(Option<Frame>, Vec<f32>)>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct State {
    pub program: Layout,
    pub preview: Layout,
    pub sources: Vec<SourceState>,
    pub in_transition: bool,
    pub titles: Vec<String>,
    pub t_s: f64,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct SourceState {
    pub index: usize,
    pub name: String,
    pub on_program: bool,
    pub on_preview: bool,
    pub muted: bool,
    pub peak_db: f32,
}

pub struct Switcher {
    pub w: u32,
    pub h: u32,
    pub fps: f64,
    rate: u32,
    inputs: Vec<Input>,
    program: Layout,
    preview: Layout,
    active: Option<Active>,
    titles: Vec<Title>,
    next_title: u32,
    frame: u64,
    scenes: Vec<(String, Layout)>,
    consent: Consent,
    iso: bool,
}

impl Switcher {
    /// Hand each source's own picture and sound out with every frame (ISO recording).
    pub fn want_iso(&mut self, on: bool) {
        self.iso = on;
    }

    pub fn new(w: u32, h: u32, fps: f64, consent: Consent) -> Switcher {
        Switcher { w: w & !1, h: h & !1, fps, rate: 48_000, inputs: Vec::new(), program: Layout::Black, preview: Layout::Black, active: None, titles: Vec::new(), next_title: 1, frame: 0, scenes: Vec::new(), consent, iso: false }
    }

    pub fn add_source(&mut self, name: &str, video: &str, audio: Option<&str>) -> Result<usize, String> {
        let v = open_video(video, self.w, self.h, self.fps, self.consent)?;
        let a = audio.map(|id| open_audio(id, self.rate, self.consent)).transpose()?;
        self.inputs.push(Input { name: name.into(), v, a, last: None, gain: 1.0, muted: false, audio_read: 0, peak_db: -120.0 });
        Ok(self.inputs.len() - 1)
    }

    /// Adds a picture source that is already made (a replay player).
    pub fn add_custom(&mut self, name: &str, v: Box<dyn VideoSource>) -> usize {
        self.inputs.push(Input { name: name.into(), v, a: None, last: None, gain: 1.0, muted: false, audio_read: 0, peak_db: -120.0 });
        self.inputs.len() - 1
    }

    pub fn source_count(&self) -> usize {
        self.inputs.len()
    }

    pub fn find(&self, name_or_index: &str) -> Result<usize, String> {
        if let Ok(i) = name_or_index.parse::<usize>() {
            if i < self.inputs.len() {
                return Ok(i);
            }
        }
        self.inputs.iter().position(|i| i.name.eq_ignore_ascii_case(name_or_index)).ok_or_else(|| format!("no source \"{name_or_index}\"; sources are {}", self.inputs.iter().enumerate().map(|(i, s)| format!("{i}:{}", s.name)).collect::<Vec<_>>().join(", ")))
    }

    pub fn check(&self, l: &Layout) -> Result<(), String> {
        for s in l.sources() {
            if s >= self.inputs.len() {
                return Err(format!("source {s} does not exist ({} sources)", self.inputs.len()));
            }
        }
        Ok(())
    }

    pub fn set_preview(&mut self, l: Layout) -> Result<(), String> {
        self.check(&l)?;
        self.preview = l;
        Ok(())
    }

    /// Straight to program, no transition (a hot punch).
    pub fn cut_to(&mut self, l: Layout) -> Result<(), String> {
        self.check(&l)?;
        self.active = None;
        self.program = l;
        Ok(())
    }

    /// Takes the preview to program with `kind` over `ms`; the old
    /// program becomes the preview (as a hardware mixer does).
    pub fn take(&mut self, kind: Transition, ms: i64) {
        let to = self.preview.clone();
        self.take_layout(to, kind, ms);
    }

    /// Goes to `to` with `kind`; the layout left behind goes to preview.
    pub fn take_layout(&mut self, to: Layout, kind: Transition, ms: i64) {
        let now = self.now_us();
        // If a transition is running it finishes where it is.
        let from = self.current_target();
        if kind == Transition::Cut || ms <= 0 {
            self.active = None;
            self.program = to;
        } else {
            self.active = Some(Active { from: from.clone(), to: to.clone(), kind, start_us: now, dur_us: ms * 1000 });
            self.program = from.clone();
        }
        self.preview = from;
    }

    fn current_target(&self) -> Layout {
        self.active.as_ref().map(|a| a.to.clone()).unwrap_or_else(|| self.program.clone())
    }

    pub fn program(&self) -> Layout {
        self.current_target()
    }

    pub fn save_scene(&mut self, name: &str, l: Layout) -> Result<(), String> {
        self.check(&l)?;
        self.scenes.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
        self.scenes.push((name.into(), l));
        Ok(())
    }

    pub fn scene(&self, name: &str) -> Result<Layout, String> {
        self.scenes.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, l)| l.clone()).ok_or_else(|| format!("no scene \"{name}\"; scenes are {}", self.scenes.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")))
    }

    pub fn mute(&mut self, source: usize, muted: bool) {
        if let Some(i) = self.inputs.get_mut(source) {
            i.muted = muted;
        }
    }

    pub fn set_gain(&mut self, source: usize, g: f32) {
        if let Some(i) = self.inputs.get_mut(source) {
            i.gain = g.clamp(0.0, 4.0);
        }
    }

    pub fn now_us(&self) -> i64 {
        (self.frame as f64 * 1e6 / self.fps).round() as i64
    }

    pub fn title_show(&mut self, kind: TitleKind, text: &str, sub: &str, fade_ms: i64) -> u32 {
        let id = self.next_title;
        self.next_title += 1;
        self.titles.push(Title { id, kind, text: text.into(), sub: sub.into(), shown_at: self.now_us(), hidden_at: None, fade_us: fade_ms.max(0) * 1000, overlay: None });
        id
    }

    pub fn title_hide(&mut self, id: Option<u32>) -> usize {
        let now = self.now_us();
        let mut n = 0;
        for t in &mut self.titles {
            if t.hidden_at.is_none() && id.is_none_or(|i| i == t.id) {
                t.hidden_at = Some(now);
                n += 1;
            }
        }
        n
    }

    fn title_alpha(t: &Title, now: i64) -> f32 {
        let fade = t.fade_us.max(1) as f32;
        let a_in = ((now - t.shown_at) as f32 / fade).clamp(0.0, 1.0);
        match t.hidden_at {
            Some(h) => a_in.min(1.0 - ((now - h) as f32 / fade).clamp(0.0, 1.0)),
            None => a_in,
        }
    }

    fn title_overlay(w: u32, h: u32, t: &Title) -> Frame {
        let mut o = Frame::clear(w, h);
        let k = h as f32 / 1080.0;
        match t.kind {
            TitleKind::Lower => {
                let main = TextOpts { size: 54.0 * k, weight: 700, align: Align::Left, ..Default::default() };
                let sub = TextOpts { size: 34.0 * k, weight: 400, colour: [230, 230, 230, 255], align: Align::Left, ..Default::default() };
                let (mw, mh) = measure(&t.text, &main, w, h);
                let (sw, sh) = if t.sub.is_empty() { (0.0, 0.0) } else { measure(&t.sub, &sub, w, h) };
                let pad = 22.0 * k;
                let (bw, bh) = (mw.max(sw) + 2.0 * pad, mh + sh + 2.0 * pad);
                let (x, y) = (w as f32 * 0.06, h as f32 * 0.84 - bh);
                o.fill_rect_blend(x as i32, y as i32, bw as i32, bh as i32, [12, 16, 28, 215]);
                o.fill_rect_blend(x as i32, y as i32, (8.0 * k).max(3.0) as i32, bh as i32, [255, 190, 40, 255]);
                draw_text(&mut o, &t.text, x + pad + mw * 0.5 + 6.0 * k, y + pad + mh * 0.5, &main);
                if !t.sub.is_empty() {
                    draw_text(&mut o, &t.sub, x + pad + sw * 0.5 + 6.0 * k, y + pad + mh + sh * 0.5, &sub);
                }
            }
            TitleKind::Centre => {
                let main = TextOpts { size: 120.0 * k, weight: 800, wrap: Some(w as f32 * 0.8), plate: Some(([0, 0, 0, 150], 28.0 * k)), ..Default::default() };
                draw_text(&mut o, &t.text, w as f32 / 2.0, h as f32 / 2.0, &main);
            }
            TitleKind::Bug => {
                let main = TextOpts { size: 36.0 * k, weight: 700, plate: Some(([200, 30, 40, 230], 14.0 * k)), ..Default::default() };
                let (mw, mh) = measure(&t.text, &main, w, h);
                draw_text(&mut o, &t.text, w as f32 * 0.94 - mw * 0.5 - 14.0 * k, h as f32 * 0.07 + mh * 0.5 + 14.0 * k, &main);
            }
        }
        o
    }

    fn render_layout(&self, l: &Layout, frames: &[Option<Frame>]) -> Frame {
        let (w, h) = (self.w, self.h);
        let mut out = Frame::new(w, h, [0, 0, 0, 255]);
        let get = |i: usize| frames.get(i).and_then(|f| f.as_ref());
        match l {
            Layout::Black => {}
            Layout::Full(s) => {
                if let Some(f) = get(*s) {
                    draw_fit(&mut out, f, 0, 0, w, h);
                }
            }
            Layout::Pip { main, inset, corner, scale } => {
                if let Some(f) = get(*main) {
                    draw_fit(&mut out, f, 0, 0, w, h);
                }
                if let Some(f) = get(*inset) {
                    let iw = ((w as f32 * scale.clamp(0.1, 0.6)) as u32) & !1;
                    let ih = iw * h / w;
                    let m = (w as f32 * 0.03) as u32;
                    let (x, y) = match corner {
                        Corner::TopLeft => (m, m),
                        Corner::TopRight => (w - iw - m, m),
                        Corner::BottomLeft => (m, h - ih - m),
                        Corner::BottomRight => (w - iw - m, h - ih - m),
                    };
                    out.fill_rect(x as i32 - 2, y as i32 - 2, iw as i32 + 4, ih as i32 + 4, [255, 255, 255, 255]);
                    draw_fit(&mut out, f, x as i32, y as i32, iw, ih);
                }
            }
            Layout::Split { left, right } => {
                for (k, s) in [left, right].into_iter().enumerate() {
                    if let Some(f) = get(*s) {
                        draw_fit(&mut out, f, (k as u32 * w / 2) as i32, 0, w / 2, h);
                    }
                }
            }
            Layout::Grid(v) => {
                let n = v.len().max(1);
                let cols = (n as f32).sqrt().ceil() as u32;
                let rows = (n as u32).div_ceil(cols);
                let (cw, ch) = (w / cols, h / rows);
                for (k, s) in v.iter().enumerate() {
                    if let Some(f) = get(*s) {
                        draw_fit(&mut out, f, ((k as u32 % cols) * cw) as i32, ((k as u32 / cols) * ch) as i32, cw, ch);
                    }
                }
            }
        }
        out
    }

    fn blend_transition(&self, a: &Frame, b: &Frame, kind: Transition, p: f32) -> Frame {
        match kind {
            Transition::Cut => b.clone(),
            Transition::Mix => Frame::mix(a, b, p),
            Transition::Dip => {
                let black = Frame::new(self.w, self.h, [0, 0, 0, 255]);
                if p < 0.5 { Frame::mix(a, &black, p * 2.0) } else { Frame::mix(&black, b, p * 2.0 - 1.0) }
            }
            Transition::Wipe(dir) => {
                let mut o = a.clone();
                let soft = (self.w.max(self.h) as f32 * 0.02).max(2.0);
                for y in 0..self.h {
                    for x in 0..self.w {
                        // How far along the wipe direction this pixel is, in pixels.
                        let (pos, len) = match dir {
                            Dir::Right => (x as f32, self.w as f32),
                            Dir::Left => ((self.w - 1 - x) as f32, self.w as f32),
                            Dir::Down => (y as f32, self.h as f32),
                            Dir::Up => ((self.h - 1 - y) as f32, self.h as f32),
                        };
                        let edge = p * (len + soft);
                        let k = ((edge - pos) / soft).clamp(0.0, 1.0);
                        if k > 0.0 {
                            let (pa, pb) = (a.px(x, y), b.px(x, y));
                            let mut c = [0u8; 4];
                            for i in 0..4 {
                                c[i] = (pa[i] as f32 * (1.0 - k) + pb[i] as f32 * k).round() as u8;
                            }
                            o.set(x, y, c);
                        }
                    }
                }
                o
            }
            Transition::Push(dir) => {
                let mut o = Frame::new(self.w, self.h, [0, 0, 0, 255]);
                let (dx, dy) = match dir {
                    Dir::Left => (-((self.w as f32 * p) as i32), 0),
                    Dir::Right => ((self.w as f32 * p) as i32, 0),
                    Dir::Up => (0, -((self.h as f32 * p) as i32)),
                    Dir::Down => (0, (self.h as f32 * p) as i32),
                };
                let (bx, by) = (dx - dx.signum() * self.w as i32 * (dx != 0) as i32, dy - dy.signum() * self.h as i32 * (dy != 0) as i32);
                for y in 0..self.h as i32 {
                    for x in 0..self.w as i32 {
                        let (ax, ay) = (x - dx, y - dy);
                        let (cx, cy) = (x - bx, y - by);
                        let c = if (0..self.w as i32).contains(&ax) && (0..self.h as i32).contains(&ay) {
                            a.px(ax as u32, ay as u32)
                        } else if (0..self.w as i32).contains(&cx) && (0..self.h as i32).contains(&cy) {
                            b.px(cx as u32, cy as u32)
                        } else {
                            [0, 0, 0, 255]
                        };
                        o.set(x as u32, y as u32, c);
                    }
                }
                o
            }
        }
    }

    /// Makes the next frame of program (and its sound).
    pub fn step(&mut self) -> Out {
        let t = self.now_us();
        let want = (((self.frame + 1) as f64 * self.rate as f64 / self.fps).round()) as u64;
        // Every source runs whether or not it is on air.
        let mut frames: Vec<Option<Frame>> = Vec::with_capacity(self.inputs.len());
        let mut blocks: Vec<Vec<f32>> = Vec::with_capacity(self.inputs.len());
        for inp in &mut self.inputs {
            if let Some(f) = inp.v.frame_at(t) {
                inp.last = Some(f);
            }
            frames.push(inp.last.clone());
            let n = want.saturating_sub(inp.audio_read) as usize;
            let b = inp.a.as_mut().map(|a| a.read(n)).unwrap_or_else(|| vec![0.0; n]);
            inp.audio_read += n as u64;
            inp.peak_db = peak_db(&b);
            blocks.push(b);
        }
        // Where is the transition?
        let progress = self.active.as_ref().map(|a| ((t - a.start_us) as f32 / a.dur_us as f32).clamp(0.0, 1.0));
        let program = match (&self.active, progress) {
            (Some(a), Some(p)) => {
                let (fa, fb) = (self.render_layout(&a.from, &frames), self.render_layout(&a.to, &frames));
                self.blend_transition(&fa, &fb, a.kind, p)
            }
            _ => self.render_layout(&self.program, &frames),
        };
        // Sound follows the picture, crossfading through a transition.
        let n = self.inputs.len();
        let weights: Vec<f32> = match (&self.active, progress) {
            (Some(a), Some(p)) => {
                let (wa, wb) = (a.from.weights(n), a.to.weights(n));
                wa.iter().zip(&wb).map(|(x, y)| x * (1.0 - p) + y * p).collect()
            }
            _ => self.program.weights(n),
        };
        let len = blocks.first().map(Vec::len).unwrap_or(((want - (want * self.frame / (self.frame + 1).max(1))) as usize).max(0));
        let mut mix = vec![0.0f32; len];
        for (i, b) in blocks.iter().enumerate() {
            let g = if self.inputs[i].muted { 0.0 } else { weights[i] * self.inputs[i].gain };
            if g > 0.0 {
                for (m, s) in mix.iter_mut().zip(b) {
                    *m += s * g;
                }
            }
        }
        for m in &mut mix {
            *m = m.clamp(-1.0, 1.0);
        }
        // The transition ends.
        if progress == Some(1.0) {
            if let Some(a) = self.active.take() {
                self.program = a.to;
            }
        }
        // Titles on top.
        let mut program = program;
        for tl in &mut self.titles {
            let a = Self::title_alpha(tl, t);
            if a > 0.0 {
                if tl.overlay.is_none() {
                    tl.overlay = Some(Self::title_overlay(self.w, self.h, tl));
                }
                if let Some(o) = &tl.overlay {
                    program.over(o, a);
                }
            }
        }
        self.titles.retain(|tl| tl.hidden_at.is_none_or(|h| t - h <= tl.fade_us));
        program.pts_us = t;
        self.frame += 1;
        let iso = if self.iso { frames.iter().cloned().zip(blocks.into_iter()).collect() } else { Vec::new() };
        Out { program, audio: AudioBlock { rate: self.rate, samples: mix }, t_us: t, iso }
    }

    pub fn state(&self) -> State {
        let prog = self.current_target();
        let on_p = prog.sources();
        let on_v = self.preview.sources();
        State {
            program: self.program.clone(),
            preview: self.preview.clone(),
            sources: self.inputs.iter().enumerate().map(|(i, s)| SourceState { index: i, name: s.name.clone(), on_program: on_p.contains(&i), on_preview: on_v.contains(&i), muted: s.muted, peak_db: s.peak_db }).collect(),
            in_transition: self.active.is_some(),
            titles: self.titles.iter().filter(|t| t.hidden_at.is_none()).map(|t| t.text.clone()).collect(),
            t_s: self.now_us() as f64 / 1e6,
        }
    }

    /// The multiview: preview and program large, every source small with a
    /// red (program) or green (preview) tally border and its name.
    pub fn multiview(&self, program: &Frame) -> Frame {
        let (w, h) = (self.w, self.h);
        let mut m = Frame::new(w, h, [16, 18, 24, 255]);
        let frames: Vec<Option<Frame>> = self.inputs.iter().map(|i| i.last.clone()).collect();
        let prev = self.render_layout(&self.preview, &frames);
        let (bw, bh) = (w * 45 / 100, h * 45 / 100);
        let gap = w / 40;
        let x0 = (w - 2 * bw - gap) / 2;
        for (k, (f, c, label)) in [(&prev, [60u8, 200, 90, 255], "PREVIEW"), (program, [230, 50, 50, 255], "PROGRAM")].into_iter().enumerate() {
            let x = x0 + k as u32 * (bw + gap);
            m.fill_rect(x as i32 - 3, gap as i32 / 2 - 3, bw as i32 + 6, bh as i32 + 6, c);
            draw_fit(&mut m, f, x as i32, gap as i32 / 2, bw, bh);
            draw_text(&mut m, label, (x + bw / 2) as f32, (gap / 2 + bh) as f32 + h as f32 * 0.03, &TextOpts { size: h as f32 * 0.04, weight: 700, ..Default::default() });
        }
        let n = self.inputs.len().max(1) as u32;
        let cw = (w / n.max(4)).min(w / 4);
        let ch = cw * h / w;
        let y = h - ch - h / 14;
        let total = n * cw + (n - 1) * (gap / 2);
        let start = (w.saturating_sub(total)) / 2;
        let on_p = self.current_target().sources();
        let on_v = self.preview.sources();
        for (i, inp) in self.inputs.iter().enumerate() {
            let x = start + i as u32 * (cw + gap / 2);
            let tally = if on_p.contains(&i) { [230, 50, 50, 255] } else if on_v.contains(&i) { [60, 200, 90, 255] } else { [60, 64, 76, 255] };
            m.fill_rect(x as i32 - 2, y as i32 - 2, cw as i32 + 4, ch as i32 + 4, tally);
            if let Some(f) = &inp.last {
                draw_fit(&mut m, f, x as i32, y as i32, cw, ch);
            }
            draw_text(&mut m, &format!("{i} {}", inp.name), (x + cw / 2) as f32, (y + ch) as f32 + h as f32 * 0.03, &TextOpts { size: h as f32 * 0.032, weight: 600, ..Default::default() });
        }
        m
    }
}

/// `src` drawn into the box, whole picture kept (bars at the sides or top).
pub fn draw_fit(dst: &mut Frame, src: &Frame, x: i32, y: i32, w: u32, h: u32) {
    if w == 0 || h == 0 {
        return;
    }
    let k = (w as f32 / src.w as f32).min(h as f32 / src.h as f32);
    let (fw, fh) = (((src.w as f32 * k).round() as u32).max(1), ((src.h as f32 * k).round() as u32).max(1));
    let (ox, oy) = (x + (w - fw) as i32 / 2, y + (h - fh) as i32 / 2);
    if (fw, fh) == (src.w, src.h) {
        // Same size: copy rows.
        for yy in 0..fh as i32 {
            let dy = oy + yy;
            if dy < 0 || dy >= dst.h as i32 {
                continue;
            }
            for xx in 0..fw as i32 {
                let dx = ox + xx;
                if dx >= 0 && dx < dst.w as i32 {
                    dst.set(dx as u32, dy as u32, src.px(xx as u32, yy as u32));
                }
            }
        }
    } else {
        let s = src.scaled(fw, fh);
        for yy in 0..fh as i32 {
            let dy = oy + yy;
            if dy < 0 || dy >= dst.h as i32 {
                continue;
            }
            for xx in 0..fw as i32 {
                let dx = ox + xx;
                if dx >= 0 && dx < dst.w as i32 {
                    dst.set(dx as u32, dy as u32, s.px(xx as u32, yy as u32));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::TestVideo;

    fn mixer() -> Switcher {
        let mut s = Switcher::new(320, 180, 30.0, Consent::default());
        s.add_source("red", "test:red", Some("test:tone:300@0.5")).unwrap();
        s.add_source("green", "test:green", Some("test:tone:900@0.5")).unwrap();
        s.add_source("cam", "test:counter:4", None).unwrap();
        s
    }

    fn main_colour(f: &Frame) -> [u8; 4] {
        f.px(f.w / 2, f.h / 2)
    }

    #[test]
    fn cut_and_take_swap_program_and_preview() {
        let mut s = mixer();
        s.cut_to(Layout::Full(0)).unwrap();
        s.set_preview(Layout::Full(1)).unwrap();
        assert_eq!(main_colour(&s.step().program)[0], 220);
        s.take(Transition::Cut, 0);
        assert_eq!(main_colour(&s.step().program)[1], 200);
        assert_eq!(s.state().preview, Layout::Full(0), "the old program waits in preview");
        assert!(s.cut_to(Layout::Full(9)).is_err());
    }

    #[test]
    fn a_mix_passes_through_halfway_and_lands() {
        let mut s = mixer();
        s.cut_to(Layout::Full(0)).unwrap();
        s.set_preview(Layout::Full(1)).unwrap();
        s.take(Transition::Mix, 1000);
        let mut mid = None;
        for n in 0..40 {
            let o = s.step();
            if n == 15 {
                mid = Some(main_colour(&o.program));
            }
        }
        let m = mid.unwrap();
        assert!(m[0] > 60 && m[0] < 180 && m[1] > 60 && m[1] < 190, "{m:?}");
        assert!(!s.state().in_transition);
        assert_eq!(main_colour(&s.step().program)[1], 200);
    }

    #[test]
    fn sound_follows_the_picture_and_crossfades() {
        let mut s = mixer();
        s.cut_to(Layout::Full(0)).unwrap();
        let rate = 48000;
        let freq = |x: &[f32]| x.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count() as f32 * 30.0 * (x.len() as f32 / rate as f32 * 30.0).recip() * 30.0 / 30.0;
        let mut a = Vec::new();
        for _ in 0..30 {
            a.extend(s.step().audio.samples);
        }
        assert_eq!(a.len(), 48000);
        let crossings = a.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        assert!((crossings as i32 - 300).abs() < 4, "300 Hz tone: {crossings}");
        let _ = freq;
        s.take_layout(Layout::Full(1), Transition::Mix, 1000);
        for _ in 0..40 {
            s.step();
        }
        let mut b = Vec::new();
        for _ in 0..30 {
            b.extend(s.step().audio.samples);
        }
        let crossings = b.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        assert!((crossings as i32 - 900).abs() < 6, "900 Hz tone: {crossings}");
        // Muted sources are silent whatever the picture.
        s.mute(1, true);
        let o = s.step();
        assert!(o.audio.samples.iter().all(|x| *x == 0.0));
    }

    #[test]
    fn layouts_place_sources() {
        let mut s = mixer();
        s.cut_to(Layout::Pip { main: 0, inset: 1, corner: Corner::BottomRight, scale: 0.3 }).unwrap();
        let f = s.step().program;
        assert_eq!(f.px(20, 20)[0], 220, "main fills");
        assert_eq!(f.px(290, 160)[1], 200, "inset bottom right");
        s.cut_to(Layout::Split { left: 0, right: 1 }).unwrap();
        let f = s.step().program;
        assert_eq!((f.px(80, 90)[0], f.px(240, 90)[1]), (220, 200));
        s.cut_to(Layout::Grid(vec![0, 1, 2, 0])).unwrap();
        let f = s.step().program;
        assert_eq!((f.px(10, 60)[0], f.px(300, 170)[0], f.px(300, 60)[1]), (220, 220, 200));
    }

    #[test]
    fn wipes_and_pushes_move_across() {
        for kind in [Transition::Wipe(Dir::Right), Transition::Push(Dir::Left), Transition::Dip] {
            let mut s = mixer();
            s.cut_to(Layout::Full(0)).unwrap();
            s.take_layout(Layout::Full(1), kind, 1000);
            let mut mid = None;
            for n in 0..40 {
                let o = s.step();
                if n == 15 {
                    mid = Some(o.program);
                }
            }
            let m = mid.unwrap();
            match kind {
                Transition::Wipe(_) => assert!(m.px(5, 90)[1] == 200 && m.px(315, 90)[0] == 220, "wipe leaves the old picture on the far side"),
                Transition::Push(_) => assert!(m.px(5, 90)[0] == 220 || m.px(5, 90)[1] == 200),
                _ => assert!(m.px(160, 90)[0] < 120, "dip is dark in the middle"),
            }
            assert_eq!(main_colour(&s.step().program)[1], 200);
        }
    }

    #[test]
    fn titles_fade_in_stay_and_go() {
        let mut s = mixer();
        s.cut_to(Layout::Full(2)).unwrap();
        let before = s.step().program;
        let id = s.title_show(TitleKind::Lower, "Priya Sharma", "Host", 300);
        let mut shown = None;
        for _ in 0..15 {
            shown = Some(s.step().program);
        }
        let shown = shown.unwrap();
        assert_ne!(before, shown);
        // The bar is dark navy at the lower left.
        assert!(shown.px(40, 150)[2] < 100 || shown.px(40, 140) != before.px(40, 140));
        assert_eq!(s.state().titles, ["Priya Sharma"]);
        assert_eq!(s.title_hide(Some(id)), 1);
        for _ in 0..15 {
            s.step();
        }
        assert!(s.state().titles.is_empty());
        let after = s.step().program;
        // The counter row differs by frame number; compare away from it.
        assert_eq!(after.px(40, 150), before.px(40, 150));
        let _ = TestVideo::read_counter;
    }

    #[test]
    fn multiview_marks_tallies() {
        let mut s = mixer();
        s.cut_to(Layout::Full(0)).unwrap();
        s.set_preview(Layout::Full(1)).unwrap();
        let o = s.step();
        let mv = s.multiview(&o.program);
        assert_eq!((mv.w, mv.h), (320, 180));
        let reds = mv.rgba.chunks(4).filter(|p| p[0] > 220 && p[1] < 60 && p[2] < 60).count();
        assert!(reds > 50);
    }
}
