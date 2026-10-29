// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `zornov app --ui-script FILE --ui-out DIR`: the real app drives itself.
//!
//! No window is made and nothing is activated: the app's own `ui` runs on
//! raw egui input (the script's clicks and keys), each frame is painted
//! offscreen with egui-wgpu on a headless wgpu device (Metal on a Mac) and
//! read back to a PNG at 2x pixels. Nothing touches the person's screen,
//! pointer, keyboard or focus.
//!
//! Pictures from the viewer come from the preview renderer's own thread and
//! GPU and reach egui as ordinary textures, so they appear in the shot like
//! everything else. The app has no egui paint callbacks. If no GPU can be
//! opened the CPU painter of `ui_shot` draws the same frame (and says so).
//! A window is never the fallback.
//!
//! The script has one step per line (`#` starts a comment, `"…"` quotes):
//!
//! ```text
//! size W H              window size in points (default 1800 1028)
//! open DIR              open a project folder
//! home                  the start screen
//! click X Y [mods]      rclick, dclick: right button, double click
//! click "Label"[#N] [mods]   the widget a screen reader calls Label (its
//!                       centre); #N is the Nth match in reading order
//! move X Y              or move "Label"[#N]
//! drag X1 Y1 X2 Y2 [mods] [hold]   or drag "Label"[#N] X2 Y2; `hold` keeps
//!                       the button down (a shot is taken, then it is let go)
//! release               let the button go where the pointer is
//! scroll X Y DY [mods]  DY in points, positive scrolls up
//! key NAME [mods]       an egui key name (A, Enter, Escape, ArrowDown…)
//! text "…"              typed text
//! wait S                S seconds of app time, 60 frames a second
//! frames N              N frames
//! shot NAME             DIR/NAME.png once the picture has settled
//! ```
//!
//! `mods` are any of `cmd`, `shift`, `alt`, `ctrl`. Coordinates are window
//! points with the origin at the top left of the content. File dialogs are
//! stubbed (the app's screenshot mode has none). Panels popped out into
//! windows are drawn inside the one window, so they are in the shot.
//! Time is the app's own clock, advanced a fixed 1/60 s a frame, so a run
//! repeats; background work (thumbnails, the viewer's renderer) is waited
//! for before a shot is taken.

use crate::app::{Overlay, StudioApp};
use crate::cli::{Cli, Screen};
use eframe::egui::{self, Event, Key, Modifiers, PointerButton, RawInput, Rect, TextureId};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const DT: f64 = 1.0 / 60.0;
const DEFAULT_SIZE: (f32, f32) = (1800.0, 1028.0);

/// Where a pointer step goes: window points, or the widget a screen reader
/// calls `label` (`"Label"#2`: the second such widget in reading order).
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    At(f32, f32),
    Label(String, usize),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Size(f32, f32),
    Open(PathBuf),
    Home,
    Click { at: Target, mods: Modifiers, button: PointerButton, count: u8 },
    Move { at: Target },
    Drag { from: Target, to: (f32, f32), mods: Modifiers, hold: bool },
    Release,
    Scroll { x: f32, y: f32, dy: f32, mods: Modifiers },
    Key { key: Key, mods: Modifiers },
    Text(String),
    Wait(f64),
    Frames(u32),
    Shot(String),
}

/// Words of a line, `"…"` kept together (`\"` and `\\` inside).
fn words(line: &str) -> Result<Vec<String>, String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quoted = false;
    let mut open = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                open = true;
            }
            '\\' if quoted => match chars.next() {
                Some('n') => cur.push('\n'),
                Some(o) => cur.push(o),
                None => return Err("a backslash at the end".into()),
            },
            '#' if !quoted && cur.is_empty() && !open => break,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() || open {
                    out.push(std::mem::take(&mut cur));
                    open = false;
                }
            }
            c => cur.push(c),
        }
    }
    if quoted {
        return Err("a quote is not closed".into());
    }
    if !cur.is_empty() || open {
        out.push(cur);
    }
    Ok(out)
}

fn mods_of(words: &[String]) -> Result<Modifiers, String> {
    let mut m = Modifiers::NONE;
    for w in words {
        match w.to_ascii_lowercase().as_str() {
            "cmd" | "command" | "meta" => {
                m.command = true;
                m.mac_cmd = true;
            }
            "shift" => m.shift = true,
            "alt" | "option" => m.alt = true,
            "ctrl" | "control" => m.ctrl = true,
            other => return Err(format!("`{other}` is not a modifier (cmd, shift, alt, ctrl)")),
        }
    }
    Ok(m)
}

fn key_of(name: &str) -> Option<Key> {
    let alias = match name.to_ascii_lowercase().as_str() {
        "esc" => "Escape",
        "return" => "Enter",
        "del" => "Delete",
        "up" => "ArrowUp",
        "down" => "ArrowDown",
        "left" => "ArrowLeft",
        "right" => "ArrowRight",
        _ => name,
    };
    Key::from_name(alias).or_else(|| {
        let mut c = alias.chars();
        let first = c.next()?.to_ascii_uppercase();
        Key::from_name(&format!("{first}{}", c.as_str().to_ascii_lowercase()))
    })
}

/// `"Label"` or `"Label"#2` at the start of `rest`, and what follows it.
fn label_target(rest: &str) -> Result<(Target, &str), String> {
    let mut label = String::new();
    let mut chars = rest.char_indices().skip(1);
    let mut end = None;
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some((_, o)) => label.push(o),
                None => break,
            },
            '"' => {
                end = Some(i + 1);
                break;
            }
            c => label.push(c),
        }
    }
    let end = end.ok_or("a quote is not closed")?;
    let mut tail = &rest[end..];
    let mut nth = 1;
    if let Some(n) = tail.strip_prefix('#') {
        let digits: String = n.chars().take_while(char::is_ascii_digit).collect();
        nth = digits.parse::<usize>().ok().filter(|n| *n >= 1).ok_or("the number after # starts at 1")?;
        tail = &n[digits.len()..];
    }
    if label.is_empty() {
        return Err("the label is empty".into());
    }
    Ok((Target::Label(label, nth), tail))
}

/// One line of a script: `None` for a blank line or a comment.
pub fn parse_line(line: &str) -> Result<Option<Step>, String> {
    // Pointer steps may name a widget instead of giving coordinates.
    let trimmed = line.trim_start();
    if let Some((cmd, rest)) = trimmed.split_once(char::is_whitespace) {
        if matches!(cmd, "click" | "rclick" | "dclick" | "move") && rest.trim_start().starts_with('"') {
            let (at, tail) = label_target(rest.trim_start())?;
            let w = words(tail)?;
            return Ok(Some(match cmd {
                "move" if w.is_empty() => Step::Move { at },
                "move" => return Err("move takes only a target".into()),
                _ => Step::Click {
                    at,
                    mods: mods_of(&w)?,
                    button: if cmd == "rclick" { PointerButton::Secondary } else { PointerButton::Primary },
                    count: if cmd == "dclick" { 2 } else { 1 },
                },
            }));
        }
    }
    if let Some(rest) = trimmed.strip_prefix("drag").map(str::trim_start).filter(|r| r.starts_with('"')) {
        let (from, tail) = label_target(rest)?;
        let w = words(tail)?;
        let n = |i: usize| w.get(i).and_then(|v| v.parse::<f32>().ok()).ok_or_else(|| "drag \"Label\" needs X2 Y2".to_owned());
        let (to, rest) = ((n(0)?, n(1)?), &w[2..]);
        let hold = rest.iter().any(|x| x == "hold");
        let mods = mods_of(&rest.iter().filter(|x| *x != "hold").cloned().collect::<Vec<_>>())?;
        return Ok(Some(Step::Drag { from, to, mods, hold }));
    }
    let w = words(line)?;
    let Some(cmd) = w.first() else { return Ok(None) };
    let num = |i: usize, what: &str| -> Result<f32, String> { w.get(i).ok_or_else(|| format!("{what} is missing"))?.parse::<f32>().map_err(|_| format!("{what} `{}` is not a number", w[i])) };
    let step = match cmd.as_str() {
        "size" => Step::Size(num(1, "width")?, num(2, "height")?),
        "open" => Step::Open(PathBuf::from(w.get(1).ok_or("a folder is missing")?)),
        "home" => Step::Home,
        "click" | "rclick" | "dclick" => Step::Click {
            at: Target::At(num(1, "x")?, num(2, "y")?),
            mods: mods_of(&w[3.min(w.len())..])?,
            button: if cmd == "rclick" { PointerButton::Secondary } else { PointerButton::Primary },
            count: if cmd == "dclick" { 2 } else { 1 },
        },
        "move" => Step::Move { at: Target::At(num(1, "x")?, num(2, "y")?) },
        "drag" => {
            let rest = &w[5.min(w.len())..];
            let mods = mods_of(&rest.iter().filter(|x| *x != "hold").cloned().collect::<Vec<_>>())?;
            Step::Drag { from: Target::At(num(1, "x1")?, num(2, "y1")?), to: (num(3, "x2")?, num(4, "y2")?), mods, hold: rest.iter().any(|x| x == "hold") }
        }
        "release" => Step::Release,
        "scroll" => Step::Scroll { x: num(1, "x")?, y: num(2, "y")?, dy: num(3, "dy")?, mods: mods_of(&w[4.min(w.len())..])? },
        "key" => {
            let name = w.get(1).ok_or("a key name is missing")?;
            Step::Key { key: key_of(name).ok_or_else(|| format!("`{name}` is not a key name"))?, mods: mods_of(&w[2..])? }
        }
        "text" => Step::Text(w.get(1).ok_or("the text is missing")?.clone()),
        "wait" => Step::Wait(f64::from(num(1, "seconds")?.max(0.0))),
        "frames" => Step::Frames(num(1, "count")?.max(0.0) as u32),
        "shot" => {
            let n = w.get(1).ok_or("a name is missing")?;
            if n.contains('/') || n.contains("..") || n.is_empty() {
                return Err(format!("`{n}` is not a plain file name"));
            }
            Step::Shot(n.trim_end_matches(".png").to_owned())
        }
        other => return Err(format!("`{other}` is not a step")),
    };
    if matches!(step, Step::Home | Step::Release) && w.len() > 1 {
        return Err(format!("{cmd} takes nothing"));
    }
    Ok(Some(step))
}

// --- the GPU -------------------------------------------------------------

struct Gpu {
    state: eframe::egui_wgpu::RenderState,
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    struct Wake(std::thread::Thread);
    impl std::task::Wake for Wake {
        fn wake(self: std::sync::Arc<Self>) {
            self.0.unpark();
        }
    }
    let mut f = std::pin::pin!(f);
    let waker = std::task::Waker::from(std::sync::Arc::new(Wake(std::thread::current())));
    let mut cx = std::task::Context::from_waker(&waker);
    loop {
        if let std::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::park();
    }
}

impl Gpu {
    /// A device with no window or surface.
    fn open() -> Result<Self, String> {
        use eframe::egui_wgpu::{RendererOptions, WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew};
        let setup = WgpuSetupCreateNew::without_display_handle();
        let instance = eframe::wgpu::Instance::new(WgpuSetupCreateNew::without_display_handle().instance_descriptor);
        let config = WgpuConfiguration { wgpu_setup: WgpuSetup::CreateNew(setup), ..Default::default() };
        let state = block_on(eframe::egui_wgpu::RenderState::create(&config, &instance, None, RendererOptions::default())).map_err(|e| format!("{e}"))?;
        Ok(Gpu { state })
    }

    fn upload(&self, delta: &egui::TexturesDelta) {
        let mut r = self.state.renderer.write();
        for (id, patches) in &delta.set {
            for d in patches {
                r.update_texture(&self.state.device, &self.state.queue, *id, d);
            }
        }
        for id in &delta.free {
            r.free_texture(id);
        }
    }

    /// The primitives painted into `size` pixels and read back as RGBA.
    fn paint(&self, prims: &[egui::epaint::ClippedPrimitive], size: [u32; 2], ppp: f32) -> Result<Vec<u8>, String> {
        use eframe::wgpu as w;
        let (device, queue) = (&self.state.device, &self.state.queue);
        let target = device.create_texture(&w::TextureDescriptor {
            label: Some("ui-script target"),
            size: w::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: w::TextureDimension::D2,
            format: self.state.target_format,
            usage: w::TextureUsages::RENDER_ATTACHMENT | w::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let row = (size[0] * 4).div_ceil(256) * 256;
        let buffer = device.create_buffer(&w::BufferDescriptor { label: Some("ui-script readback"), size: u64::from(row) * u64::from(size[1]), usage: w::BufferUsages::COPY_DST | w::BufferUsages::MAP_READ, mapped_at_creation: false });
        let screen = eframe::egui_wgpu::ScreenDescriptor { size_in_pixels: size, pixels_per_point: ppp };
        let mut encoder = device.create_command_encoder(&Default::default());
        let extra = self.state.renderer.write().update_buffers(device, queue, &mut encoder, prims, &screen);
        {
            let pass = encoder.begin_render_pass(&w::RenderPassDescriptor {
                label: Some("ui-script pass"),
                color_attachments: &[Some(w::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: w::Operations { load: w::LoadOp::Clear(w::Color::BLACK), store: w::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            self.state.renderer.read().render(&mut pass, prims, &screen);
        }
        encoder.copy_texture_to_buffer(
            w::TexelCopyTextureInfo { texture: &target, mip_level: 0, origin: w::Origin3d::ZERO, aspect: w::TextureAspect::All },
            w::TexelCopyBufferInfo { buffer: &buffer, layout: w::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: None } },
            w::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
        );
        queue.submit(extra.into_iter().chain(Some(encoder.finish())));
        let (tx, rx) = std::sync::mpsc::channel();
        buffer.slice(..).map_async(w::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        device.poll(w::PollType::wait_indefinitely()).map_err(|e| format!("{e}"))?;
        rx.recv().map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
        let mapped = buffer.slice(..).get_mapped_range().map_err(|e| e.to_string())?;
        let mut rgba = Vec::with_capacity((size[0] * size[1] * 4) as usize);
        for y in 0..size[1] as usize {
            let start = y * row as usize;
            rgba.extend_from_slice(&mapped[start..start + (size[0] * 4) as usize]);
        }
        drop(mapped);
        buffer.unmap();
        Ok(rgba)
    }
}

// --- the session ---------------------------------------------------------

/// A picture of the window: pixels, RGBA, `size` wide and high.
pub struct Picture {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
}

/// The app running with no window, between script steps.
pub struct Session {
    pub app: StudioApp,
    pub ctx: egui::Context,
    frame: eframe::Frame,
    time: f64,
    /// Window size in points.
    pub size: egui::Vec2,
    ppp: f32,
    gpu: Option<Gpu>,
    /// The CPU painter's textures; kept only when there is no GPU.
    cpu: crate::ui_shot::Textures,
    /// Sizes of the textures egui has made (the font atlas is the first).
    pub textures: HashMap<TextureId, [usize; 2]>,
    shapes: Vec<egui::epaint::ClippedShape>,
    frame_ppp: f32,
    mods: Modifiers,
    sent_mods: Modifiers,
    /// Frames since the picture last changed.
    quiet: u32,
    last_hash: u64,
    pub gpu_note: Option<String>,
    /// What a screen reader would be told last frame: name and place.
    nodes: Vec<(String, Rect, u64)>,
    /// Pictures large enough to be a frame, as egui was given them.
    /// The viewer's frame has been asked for again since the project opened.
    nudged: bool,
    /// A drag with `hold`: where the pointer is and the modifiers held.
    held: Option<(egui::Pos2, Modifiers)>,
    pub frames_seen: HashMap<TextureId, std::sync::Arc<egui::ColorImage>>,
}

impl Session {
    pub fn new(cli: Cli, want_gpu: bool) -> Self {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let ppp = cli.ui_ppp.unwrap_or(2.0);
        let size = cli.size.map_or(egui::vec2(DEFAULT_SIZE.0, DEFAULT_SIZE.1), |(w, h)| egui::vec2(w, h));
        let app = StudioApp::new(&cc, cli);
        let (gpu, gpu_note) = if want_gpu {
            match Gpu::open() {
                Ok(g) => (Some(g), None),
                Err(e) => (None, Some(format!("no GPU for the shots ({e}); drawn on the CPU"))),
            }
        } else {
            (None, None)
        };
        let mut s = Session { app, ctx, frame: eframe::Frame::_new_kittest(), time: 0.0, size, ppp, gpu, cpu: Default::default(), textures: HashMap::new(), shapes: vec![], frame_ppp: ppp, mods: Modifiers::NONE, sent_mods: Modifiers::NONE, quiet: 0, last_hash: 0, gpu_note, nodes: vec![], nudged: false, held: None, frames_seen: HashMap::new() };
        s.frames(3);
        s
    }

    pub fn on_gpu(&self) -> bool {
        self.gpu.is_some()
    }

    /// One frame of the app with `events`.
    pub fn frame(&mut self, events: Vec<Event>) {
        self.time += DT;
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), self.size);
        let mut raw = RawInput { screen_rect: Some(rect), time: Some(self.time), predicted_dt: DT as f32, focused: true, events, ..Default::default() };
        // Held modifiers are told to egui as a window tells them.
        if self.mods != self.sent_mods {
            raw.events.insert(0, Event::ModifiersChanged(self.mods));
            self.sent_mods = self.mods;
        }
        let vp = raw.viewports.entry(egui::ViewportId::ROOT).or_default();
        vp.native_pixels_per_point = Some(self.ppp);
        vp.focused = Some(true);
        vp.inner_rect = Some(rect);
        let (app, frame) = (&mut self.app, &mut self.frame);
        let mut out = self.ctx.run_ui(raw, |ui| eframe::App::ui(app, ui, frame));
        for (id, patches) in &out.textures_delta.set {
            for d in patches {
                if d.pos.is_none() {
                    self.textures.insert(*id, d.image.size());
                    // Whole pictures (the viewer's frame) are kept to look at.
                    let egui::ImageData::Color(img) = &d.image;
                    if *id != TextureId::default() && img.size[0] >= 160 && img.size[1] >= 90 {
                        self.frames_seen.insert(*id, img.clone());
                    }
                }
            }
        }
        for id in &out.textures_delta.free {
            self.textures.remove(id);
        }
        match &self.gpu {
            Some(g) => g.upload(&out.textures_delta),
            None => self.cpu.apply(&out.textures_delta),
        }
        if let Some(u) = &out.platform_output.accesskit_update {
            self.nodes = u
                .nodes
                .iter()
                .filter_map(|(id, n)| {
                    let b = n.bounds()?;
                    let label = n.label().or(n.value())?.to_owned();
                    Some((label, Rect::from_min_max(egui::pos2(b.x0 as f32, b.y0 as f32), egui::pos2(b.x1 as f32, b.y1 as f32)), id.0))
                })
                .collect();
        }
        out.textures_delta.clear();
        self.frame_ppp = out.pixels_per_point;
        // A cheap fingerprint of what is drawn, to see when it stops changing.
        let prims = self.ctx.tessellate(out.shapes.clone(), self.frame_ppp);
        let mut h: u64 = 1469598103934665603;
        for p in &prims {
            if let egui::epaint::Primitive::Mesh(m) = &p.primitive {
                for v in &m.vertices {
                    for b in [v.pos.x.to_bits(), v.pos.y.to_bits(), u32::from_le_bytes(v.color.to_array())] {
                        h = (h ^ u64::from(b)).wrapping_mul(1099511628211);
                    }
                }
                let tex = match m.texture_id {
                    TextureId::Managed(n) => n,
                    TextureId::User(n) => n | (1 << 63),
                };
                h = (h ^ tex).wrapping_mul(1099511628211);
            }
        }
        self.quiet = if h == self.last_hash { self.quiet + 1 } else { 0 };
        self.last_hash = h;
        self.shapes = out.shapes;
    }

    pub fn frames(&mut self, n: u32) {
        for _ in 0..n {
            self.frame(vec![]);
        }
    }

    fn pause(&self) {
        // Work on other threads (thumbnails, the viewer's renderer) gets real time.
        std::thread::sleep(std::time::Duration::from_millis(3));
    }

    /// Whether the widget with this AccessKit id is on the topmost layer at
    /// its middle (not behind a dialog or sheet), so a click reaches it.
    fn on_top(&self, node: u64) -> bool {
        let found = self.ctx.viewport(|v| v.this_pass.widgets.layers().flat_map(|(_, l)| l.iter()).find(|w| w.id.value() == node).map(|w| (w.layer_id, w.interact_rect)));
        match found {
            Some((layer, r)) if r.width() > 0.0 && r.height() > 0.0 => self.ctx.layer_id_at(r.center()) == Some(layer),
            Some(_) => false,
            None => true,
        }
    }

    /// Text painted exactly as `label`, for what has no screen-reader name.
    fn painted(&self, label: &str) -> Vec<Rect> {
        fn walk(shape: &egui::epaint::Shape, text: &str, out: &mut Vec<Rect>) {
            match shape {
                egui::epaint::Shape::Text(t) if t.galley.text() == text => out.push(t.galley.rect.translate(t.pos.to_vec2())),
                egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| walk(s, text, out)),
                _ => {}
            }
        }
        let mut out = vec![];
        for cs in &self.shapes {
            walk(&cs.shape, label, &mut out);
        }
        let screen = Rect::from_min_size(egui::pos2(0.0, 0.0), self.size);
        out.retain(|r| r.width() > 0.0 && screen.contains(r.center()) && self.ctx.layer_id_at(r.center()).is_some_and(|l| l.order == egui::Order::Background || !self.ctx.memory(|m| m.area_rect(l.id)).is_some_and(|a| a.expand(1.0).contains_rect(screen))));
        out
    }

    /// The `nth` (from 1) widget named `label`, in reading order (top to
    /// bottom, then left to right); text painted as `label` when no widget
    /// has that name.
    pub fn locate(&self, label: &str, nth: usize) -> Result<egui::Pos2, String> {
        let screen = Rect::from_min_size(egui::pos2(0.0, 0.0), self.size);
        let mut hits: Vec<Rect> = self.nodes.iter().filter(|(l, r, id)| l == label && r.width() > 0.0 && r.height() > 0.0 && screen.contains(r.center()) && self.on_top(*id)).map(|(_, r, _)| *r).collect();
        if hits.is_empty() {
            hits = self.painted(label);
        }
        hits.sort_by(|a, b| (a.top() / 4.0).round().total_cmp(&(b.top() / 4.0).round()).then(a.left().total_cmp(&b.left())));
        hits.get(nth - 1).map(|r| r.center()).ok_or_else(|| if hits.is_empty() { format!("nothing on screen is called \"{label}\"") } else { format!("only {} of \"{label}\", not {nth}", hits.len()) })
    }

    fn target(&self, t: &Target) -> Result<egui::Pos2, String> {
        match t {
            Target::At(x, y) => self.pos(*x, *y),
            Target::Label(l, n) => self.locate(l, *n),
        }
    }

    fn pos(&self, x: f32, y: f32) -> Result<egui::Pos2, String> {
        if x < 0.0 || y < 0.0 || x > self.size.x || y > self.size.y {
            return Err(format!("({x}, {y}) is outside the {}x{} window", self.size.x, self.size.y));
        }
        Ok(egui::pos2(x, y))
    }

    fn set_mods(&mut self, m: Modifiers) {
        self.mods = m;
    }

    fn button(pos: egui::Pos2, button: PointerButton, pressed: bool, modifiers: Modifiers) -> Event {
        Event::PointerButton { pos, button, pressed, modifiers }
    }

    /// Let go of a held drag.
    fn release(&mut self) {
        if let Some((p, mods)) = self.held.take() {
            self.frame(vec![Self::button(p, PointerButton::Primary, false, mods)]);
            self.set_mods(Modifiers::NONE);
            self.frames(2);
        }
    }

    /// Run one step.
    pub fn step(&mut self, step: &Step) -> Result<(), String> {
        match step {
            Step::Size(w, h) => {
                if *w < 200.0 || *h < 150.0 || *w > 6000.0 || *h > 4000.0 {
                    return Err(format!("{w}x{h} is not a window size"));
                }
                self.size = egui::vec2(*w, *h);
                self.frames(2);
            }
            Step::Open(dir) => {
                if !studio_model::file::ProjectFolder::is_project(dir) {
                    return Err(format!("{} is not a project folder", dir.display()));
                }
                let ctx = self.ctx.clone();
                self.app.open_folder(&ctx, Some(dir.clone()));
                if self.app.doc.folder.is_none() {
                    return Err(format!("{} did not open", dir.display()));
                }
                self.app.overlay = Overlay::None;
                self.app.screen = Screen::Editor;
                self.nudged = false;
                self.frames(4);
            }
            Step::Home => {
                self.app.screen = Screen::Start;
                self.app.overlay = Overlay::None;
                self.frames(3);
            }
            Step::Move { at } => {
                let p = self.target(at)?;
                self.frame(vec![Event::PointerMoved(p)]);
                self.frames(1);
            }
            Step::Click { at, mods, button, count } => {
                let p = self.target(at)?;
                self.set_mods(*mods);
                self.frame(vec![Event::PointerMoved(p)]);
                for _ in 0..*count {
                    self.frame(vec![Self::button(p, *button, true, *mods)]);
                    self.frame(vec![Self::button(p, *button, false, *mods)]);
                }
                self.set_mods(Modifiers::NONE);
                self.frames(2);
            }
            Step::Drag { from, to, mods, hold } => {
                if self.held.is_some() {
                    self.release();
                }
                let (a, b) = (self.target(from)?, self.pos(to.0, to.1)?);
                self.set_mods(*mods);
                self.frame(vec![Event::PointerMoved(a)]);
                self.frame(vec![Self::button(a, PointerButton::Primary, true, *mods)]);
                for i in 1..=8 {
                    let p = a + (b - a) * (i as f32 / 8.0);
                    self.frame(vec![Event::PointerMoved(p)]);
                }
                if *hold {
                    self.held = Some((b, *mods));
                    self.frames(2);
                } else {
                    self.frame(vec![Self::button(b, PointerButton::Primary, false, *mods)]);
                    self.set_mods(Modifiers::NONE);
                    self.frames(2);
                }
            }
            Step::Release => {
                if self.held.is_none() {
                    return Err("no drag is being held".into());
                }
                self.release();
            }
            Step::Scroll { x, y, dy, mods } => {
                let p = self.pos(*x, *y)?;
                self.set_mods(*mods);
                self.frame(vec![Event::PointerMoved(p)]);
                self.frame(vec![Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, *dy), modifiers: *mods, source: egui::MouseWheelSource::Wheel, phase: egui::TouchPhase::Move }]);
                self.set_mods(Modifiers::NONE);
                self.frames(2);
            }
            Step::Key { key, mods } => {
                self.set_mods(*mods);
                let mut down = vec![Event::Key { key: *key, physical_key: Some(*key), pressed: true, repeat: false, modifiers: *mods }];
                // A key that types a character does so when nothing is held but Shift.
                if !mods.command && !mods.ctrl && !mods.alt {
                    let name = key.symbol_or_name();
                    if name.chars().count() == 1 && name.chars().all(|c| c.is_alphanumeric() || c.is_ascii_punctuation()) {
                        down.push(Event::Text(if mods.shift { name.to_uppercase() } else { name.to_lowercase() }));
                    } else if *key == Key::Space {
                        down.push(Event::Text(" ".into()));
                    }
                }
                self.frame(down);
                self.frame(vec![Event::Key { key: *key, physical_key: Some(*key), pressed: false, repeat: false, modifiers: *mods }]);
                self.set_mods(Modifiers::NONE);
                self.frames(2);
            }
            Step::Text(t) => {
                self.frame(vec![Event::Text(t.clone())]);
                self.frames(2);
            }
            Step::Wait(s) => {
                for _ in 0..(s / DT).round() as u32 {
                    self.frame(vec![]);
                    self.pause();
                }
            }
            Step::Frames(n) => {
                for _ in 0..*n {
                    self.frame(vec![]);
                    self.pause();
                }
            }
            Step::Shot(_) => unreachable!("shots are taken by `shot_to`"),
        }
        Ok(())
    }

    /// Frames until the picture holds still and background work is in:
    /// thumbnails loaded, and (with a project open) the viewer's frame.
    pub fn settle(&mut self) {
        let started = std::time::Instant::now();
        let want_viewer = self.app.screen == Screen::Editor && !self.app.doc.project.clips.is_empty() && !self.app.cli.flag("simple-preview");
        for i in 0.. {
            self.frame(vec![]);
            self.pause();
            if want_viewer && !self.nudged && self.viewer_frames() > 0 {
                self.nudge_viewer();
            }
            let in_viewer = !want_viewer || self.viewer_frames() > 0;
            if i >= 6 && self.app.thumbs.pending() == 0 && in_viewer && self.quiet >= 8 {
                break;
            }
            // Spinners and the caret never hold still: the clock ends it.
            if started.elapsed() > std::time::Duration::from_secs(if self.viewer_frames() > 0 { 5 } else { 12 }) {
                break;
            }
        }
    }

    /// The first frame the renderer sends can be drawn before the clip is
    /// decoded (black); a window asks again as its layout settles, a still
    /// session does not. Ask for the next frame and then this one again, so
    /// the one on screen is the exact frame.
    fn nudge_viewer(&mut self) {
        self.nudged = true;
        let at = self.app.editor.playhead;
        for to in [at + 33_334, at] {
            let before = self.viewer_frames();
            self.app.editor.playhead = to;
            let started = std::time::Instant::now();
            while self.viewer_frames() == before && started.elapsed() < std::time::Duration::from_secs(2) {
                self.frame(vec![]);
                self.pause();
            }
        }
    }

    /// Frames the preview renderer has sent to the viewer so far (they
    /// reach egui as a texture, so they are in the picture).
    pub fn viewer_frames(&self) -> u64 {
        self.app.render.as_ref().map_or(0, |r| r.frames)
    }

    /// The window as it is now.
    pub fn picture(&mut self) -> Result<Picture, String> {
        let ppp = self.frame_ppp;
        let size = [(self.size.x * ppp).round() as usize, (self.size.y * ppp).round() as usize];
        let prims = self.ctx.tessellate(self.shapes.clone(), ppp);
        let rgba = match &self.gpu {
            Some(g) => g.paint(&prims, [size[0] as u32, size[1] as u32], ppp)?,
            None => crate::ui_shot::render(&prims, &self.cpu, size, ppp, crate::theme::tokens(&self.ctx).ground),
        };
        Ok(Picture { size, rgba })
    }

    pub fn shot_to(&mut self, dir: &Path, name: &str) -> Result<PathBuf, String> {
        self.settle();
        let pic = self.picture()?;
        let path = dir.join(format!("{name}.png"));
        crate::ui_shot::save_png(&path, pic.size, &pic.rgba).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(path)
    }
}

/// What a run came to.
#[derive(Debug, Default)]
pub struct Report {
    pub steps: usize,
    pub shots: Vec<PathBuf>,
    pub failures: Vec<String>,
    /// One line a step: `ok` or `failed`, the place, the step, the reason.
    pub log: Vec<String>,
}

/// Run script text against a session; every failed step is noted and the
/// run goes on with the next one.
pub fn run_text(s: &mut Session, text: &str, out: &Path, file: &str) -> Report {
    let mut r = Report::default();
    for (n, line) in text.lines().enumerate() {
        let at = format!("{file}:{}", n + 1);
        let step = match parse_line(line) {
            Ok(Some(s)) => s,
            Ok(None) => continue,
            Err(e) => {
                r.log.push(format!("failed {at}: {} :: {e}", line.trim()));
                r.failures.push(format!("{at}: {e}"));
                continue;
            }
        };
        r.steps += 1;
        let result = match &step {
            Step::Shot(name) => {
                let shot = s.shot_to(out, name).map(|p| r.shots.push(p));
                s.release();
                shot
            }
            other => s.step(other),
        };
        match result {
            Ok(()) => r.log.push(format!("ok {at}: {}", line.trim())),
            Err(e) => {
                r.log.push(format!("failed {at}: {} :: {e}", line.trim()));
                r.failures.push(format!("{at}: {}: {e}", line.trim()));
            }
        }
    }
    r
}

/// `zornov app --ui-script FILE --ui-out DIR`: the exit code.
pub fn main(cli: Cli) -> i32 {
    let (Some(script), Some(out)) = (cli.ui_script.clone(), cli.ui_out.clone()) else {
        eprintln!("ui-script: --ui-script FILE and --ui-out DIR go together");
        return 2;
    };
    let text = match std::fs::read_to_string(&script) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("ui-script: cannot read {}: {e}", script.display());
            return 2;
        }
    };
    if let Err(e) = std::fs::create_dir_all(&out) {
        eprintln!("ui-script: cannot make {}: {e}", out.display());
        return 2;
    }
    let log = cli.ui_log.clone();
    let mut s = Session::new(cli, true);
    if let Some(note) = &s.gpu_note {
        eprintln!("ui-script: {note}");
    }
    let report = run_text(&mut s, &text, &out, &script.display().to_string());
    if let Some(log) = &log {
        if let Err(e) = std::fs::write(log, report.log.join("\n") + "\n") {
            eprintln!("ui-script: cannot write {}: {e}", log.display());
        }
    }
    for p in &report.shots {
        eprintln!("ui-script: saved {}", p.display());
    }
    for f in &report.failures {
        eprintln!("ui-script: FAILED {f}");
    }
    if s.app.screen == Screen::Editor && !s.app.doc.project.clips.is_empty() && !s.app.cli.flag("simple-preview") && s.viewer_frames() == 0 {
        eprintln!("ui-script: the viewer has no GPU frame (the preview renderer sent none); its area is empty in the shots");
    }
    eprintln!("ui-script: {} steps, {} shots, {} failures, painted on {}", report.steps, report.shots.len(), report.failures.len(), if s.on_gpu() { "the GPU (offscreen)" } else { "the CPU" });
    i32::from(!report.failures.is_empty())
}
