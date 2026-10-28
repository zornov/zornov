// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The generative media department: tasks that make or change pictures and
//! clips, each run by whichever provider can do it.
//!
//! | task | Codex (your ChatGPT plan) | on this Mac |
//! |---|---|---|
//! | animate a still | – | a camera move through its depth |
//! | between two frames | – | RIFE in-betweens (a morph) |
//! | restyle | pictures | look transfer from a reference |
//! | relight | pictures | a key light shaped by depth |
//! | time of day, season | pictures | light and grade |
//! | outpaint (wider, taller) | pictures | reflected and softened edges |
//! | inpaint a region | pictures | filled from its surroundings |
//! | upscale | – | Real-ESRGAN on the NN engine |
//! | enhance (restore) | – | denoise, deblock, deflicker, dust, scratches, fade, upscale, frame rate |
//!
//! [`run`] works on files and runs on any thread (the Generate panel runs
//! it in the background; the AI's `generate` and `enhance` tools call it
//! and then put the result in the project). Every result gets a recipe
//! beside it (`<file>.recipe.json`: the task, provider, prompt, seed and
//! settings), so it can be made again or varied. [`Look`]s keep a
//! character or place the same across generations: reference pictures,
//! words and a seed under a name.

use std::ops::Range;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use studio_ai::features::{depth::DepthEstimator, fill, interpolate::FrameInterpolator, look, parallax, relight, restore, upscale::Upscaler};
use studio_ai::{Frame, JobCtx, ModelId, ModelStore};

use super::tracking::Footage;
use crate::agent::generate as gm;

/// Who makes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    /// Codex's image generation on the person's ChatGPT plan.
    Codex,
    /// On this Mac, with Studio's own NN engine and image code.
    Local,
}

impl Provider {
    pub fn parse(s: &str) -> Option<Provider> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "codex" | "chatgpt" | "openai" => Provider::Codex,
            "local" | "mac" | "here" | "studio" | "offline" => Provider::Local,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Provider::Codex => "codex",
            Provider::Local => "local",
        }
    }
}

/// What to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Task {
    /// A still made into a moving clip.
    Animate,
    /// A clip from a first frame to a last frame.
    Between,
    /// A picture or clip given a new style or look.
    Restyle,
    Relight,
    TimeOfDay,
    Season,
    /// A picture made wider or taller.
    Outpaint,
    /// A region of a picture repainted.
    Inpaint,
    Upscale,
    /// Restoration: noise, blocks, flicker, dirt, scratches, fading, size,
    /// frame rate, in one pass.
    Enhance,
}

impl Task {
    pub const ALL: [Task; 10] = [Task::Animate, Task::Between, Task::Restyle, Task::Relight, Task::TimeOfDay, Task::Season, Task::Outpaint, Task::Inpaint, Task::Upscale, Task::Enhance];

    pub fn parse(s: &str) -> Option<Task> {
        Some(match s.trim().to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
            "animate" | "image_to_video" | "i2v" | "move" => Task::Animate,
            "between" | "first_last" | "frames" | "first_last_frame" | "morph" | "tween" => Task::Between,
            "restyle" | "style" | "style_transfer" | "video_to_video" | "v2v" | "look" => Task::Restyle,
            "relight" | "light" => Task::Relight,
            "time_of_day" | "time" | "tod" => Task::TimeOfDay,
            "season" => Task::Season,
            "outpaint" | "extend" | "expand" | "widen" | "uncrop" => Task::Outpaint,
            "inpaint" | "fill" | "repaint" | "remove" => Task::Inpaint,
            "upscale" | "super_resolution" | "superres" | "sr" => Task::Upscale,
            "enhance" | "restore" | "restoration" | "clean" | "old_film" => Task::Enhance,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Task::Animate => "animate",
            Task::Between => "between",
            Task::Restyle => "restyle",
            Task::Relight => "relight",
            Task::TimeOfDay => "time_of_day",
            Task::Season => "season",
            Task::Outpaint => "outpaint",
            Task::Inpaint => "inpaint",
            Task::Upscale => "upscale",
            Task::Enhance => "enhance",
        }
    }

    /// Makes a video (from pictures).
    pub fn makes_video(self) -> bool {
        matches!(self, Task::Animate | Task::Between)
    }
}

/// A clip to work on: footage and the frames of it to use.
#[derive(Clone, Debug)]
pub struct ClipIn {
    pub footage: Footage,
    pub range: Range<u64>,
}

impl ClipIn {
    pub fn seconds(&self) -> f64 {
        (self.range.end - self.range.start) as f64 / self.footage.fps.max(1.0)
    }
}

/// Footage from a file.
pub fn footage_of(path: &Path) -> Result<Footage, String> {
    let src = studio_render::source::open_path(path)?;
    let i = src.info();
    Ok(Footage { path: path.to_path_buf(), fps: i.frame_rate.unwrap_or(30.0).max(1.0), frames: i.frame_count.unwrap_or(1).max(1), width: i.width.max(1), height: i.height.max(1) })
}

/// Everything a run needs. `settings` holds the task's knobs (see the
/// guide): `direction`, `colour`, `strength`, `time`, `season`, `left`…
/// `bottom`, `region`, `scale`, `fps`, `motion`, `seconds`, `steps`, `model`.
#[derive(Clone, Debug)]
pub struct Request {
    pub task: Task,
    /// None: the best one available.
    pub provider: Option<Provider>,
    pub prompt: String,
    pub image: Option<PathBuf>,
    pub end_image: Option<PathBuf>,
    /// Pictures to follow: a look reference, a character, a place.
    pub refs: Vec<PathBuf>,
    pub clip: Option<ClipIn>,
    pub settings: Value,
    pub seed: Option<u64>,
    /// Where results go, and their name.
    pub dir: PathBuf,
    pub name: String,
    /// Price (or plan) only.
    pub estimate: bool,
    /// Long local work goes ahead.
    pub long: bool,
}

impl Request {
    pub fn new(task: Task, dir: PathBuf) -> Request {
        Request { task, provider: None, prompt: String::new(), image: None, end_image: None, refs: Vec::new(), clip: None, settings: json!({}), seed: None, dir, name: String::new(), estimate: false, long: false }
    }

    fn f(&self, k: &str) -> Option<f64> {
        self.settings.get(k).and_then(Value::as_f64)
    }

    fn s(&self, k: &str) -> Option<&str> {
        self.settings.get(k).and_then(Value::as_str)
    }

    fn b(&self, k: &str) -> Option<bool> {
        self.settings.get(k).and_then(Value::as_bool)
    }
}

/// What a run made.
#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub files: Vec<PathBuf>,
    pub video: bool,
    pub provider: Option<Provider>,
    /// Who made it, in words.
    pub by: String,
    pub price: Option<String>,
    /// What Codex would do better, when the result came from a fallback.
    pub note: Option<String>,
    /// Task details for the AI (steps taken, sizes, engine).
    pub details: Value,
    /// Nothing was made: an estimate.
    pub estimate: bool,
}

/// Which providers are here.
#[derive(Clone, Copy, Debug)]
pub struct Available {
    pub codex: bool,
}

impl Available {
    pub fn now() -> Available {
        Available { codex: crate::agent::brain::find_codex().is_some() }
    }
}

/// The providers that can do `task` (on a clip or not), best first.
pub fn providers(task: Task, on_clip: bool) -> &'static [Provider] {
    use Provider::*;
    match (task, on_clip) {
        (Task::Animate | Task::Between, _) => &[Local],
        (Task::Upscale | Task::Enhance, _) => &[Local],
        (Task::Restyle, true) => &[Local],
        (Task::Relight | Task::TimeOfDay | Task::Season, true) => &[Local],
        (Task::Outpaint | Task::Inpaint, true) => &[],
        _ => &[Codex, Local],
    }
}

/// The provider to use: the one asked for (if it can), else the first that
/// can and is here.
pub fn route(task: Task, on_clip: bool, want: Option<Provider>, here: Available) -> Result<Provider, String> {
    let can = providers(task, on_clip);
    if can.is_empty() {
        return Err(format!("{} works on pictures; for a clip, take a frame (image: {{\"frame\": seconds}}) or use the picture the clip shows", task.name()));
    }
    let present = |p: Provider| match p {
        Provider::Codex => here.codex,
        Provider::Local => true,
    };
    if let Some(p) = want {
        if !can.contains(&p) {
            return Err(format!("{} {} cannot be done by {}; it can by {}", task.name(), if on_clip { "on a clip" } else { "on a picture" }, p.name(), can.iter().map(|p| p.name()).collect::<Vec<_>>().join(" or ")));
        }
        if !present(p) {
            return Err("the codex command is not on this Mac (install the Codex app)".into());
        }
        return Ok(p);
    }
    Ok(can.iter().copied().find(|&p| present(p)).unwrap_or(Provider::Local))
}

/// What Codex would add when `task` ran locally.
fn key_note(task: Task, on_clip: bool) -> Option<String> {
    let s = match task {
        Task::Animate => "made on this Mac as a camera move through the picture's depth (nothing in it moves by itself; motion invented inside the picture is not available yet)",
        Task::Between => "made on this Mac with RIFE in-betweens: right for two frames of the same shot a moment apart, a dissolve-like morph for very different ones (motion invented between them is not available yet)",
        Task::Restyle if on_clip => "restyled on this Mac by look transfer (colour, contrast and light of the reference); redrawing a clip in a new style (drawing, materials, brushwork) is not available yet",
        Task::Restyle => "restyled on this Mac by look transfer (colour and light only). With Codex, the picture is redrawn in the style",
        Task::Relight => "relit on this Mac: a key light shaped by the depth of the picture. With Codex, the light is repainted with real shadows",
        Task::TimeOfDay | Task::Season => "changed on this Mac by light and grade. With Codex, the scene is repainted (sky, shadows, snow, lit windows)",
        Task::Outpaint => "extended on this Mac by reflecting and softening the edges (fine a little past the edge, plain further out). With Codex, the scene beyond is invented",
        Task::Inpaint => "filled on this Mac from the surroundings (right for wires, specks and small objects). With Codex, the prompt is painted in",
        Task::Upscale | Task::Enhance => return None,
    };
    Some(s.into())
}

/// Runs a request. `status` hears what is happening.
pub fn run(req: &Request, ctx: &JobCtx, status: &mut dyn FnMut(String)) -> Result<Outcome, String> {
    let on_clip = req.clip.is_some();
    let here = Available::now();
    let provider = route(req.task, on_clip, req.provider, here)?;
    std::fs::create_dir_all(&req.dir).map_err(|e| format!("{}: {e}", req.dir.display()))?;
    let mut out = match provider {
        Provider::Codex if best_of(req) > 1 && !req.estimate && req.clip.is_none() && !req.task.makes_video() => takes(req, status)?,
        Provider::Codex => run_codex(req, status)?,
        Provider::Local => {
            let mut o = run_local(req, ctx, status)?;
            // Said only when Codex would really have done more.
            let better_absent = providers(req.task, on_clip).iter().take_while(|p| **p != Provider::Local).any(|p| match p {
                Provider::Codex => !here.codex,
                Provider::Local => false,
            });
            if req.provider.is_none() && better_absent {
                o.note = key_note(req.task, on_clip);
            }
            o
        }
    };
    out.provider = Some(provider);
    if !out.estimate {
        for f in &out.files {
            write_recipe(f, req, &out);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- pictures

/// A picture file as a frame (PNG, JPEG; WebP and others through the
/// media reader).
pub fn load_picture(path: &Path) -> Result<Frame, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let img = match ext.as_str() {
        "png" => studio_media::png::read(path)?,
        "jpg" | "jpeg" => studio_media::jpeg::read(path)?,
        _ => {
            let p = crate::convert::ensure_playable(path)?;
            if p != path {
                return load_picture(&p);
            }
            return Err(format!("{} is not a picture Zornov reads (PNG or JPEG)", path.display()));
        }
    };
    Frame::new(img.width as usize, img.height as usize, img.pixels).map_err(|e| e.to_string())
}

/// Writes `f` as a PNG.
pub fn save_png(f: &Frame, path: &Path) -> Result<(), String> {
    let img = studio_media::rgba::RgbaImage { width: f.width as u32, height: f.height as u32, pixels: f.rgba.clone() };
    std::fs::write(path, studio_media::png::encode(&img)).map_err(|e| format!("{}: {e}", path.display()))
}

fn out_path(req: &Request, suffix: &str, ext: &str) -> PathBuf {
    let base = if req.name.trim().is_empty() { gm::stem(&req.prompt) } else { req.name.clone() };
    let stem = if suffix.is_empty() { base } else { format!("{base} {suffix}") };
    gm::free_path(&req.dir, stem.trim(), ext)
}

fn picture_in(req: &Request) -> Result<&Path, String> {
    req.image.as_deref().ok_or_else(|| format!("{} needs a picture: image (a file, a media id or {{\"frame\": seconds}}) or a clip", req.task.name()))
}

/// The region to repaint, as shares of the frame `[x, y, w, h]`.
fn region(req: &Request) -> Result<[f32; 4], String> {
    let v = req.settings.get("region").ok_or("inpaint needs region: [x, y, width, height] as shares of the picture (0..1), e.g. [0.6, 0.1, 0.3, 0.4]")?;
    let a: Vec<f32> = v.as_array().map(|a| a.iter().filter_map(Value::as_f64).map(|x| x as f32).collect()).unwrap_or_default();
    if a.len() != 4 || a[2] <= 0.0 || a[3] <= 0.0 {
        return Err("region is [x, y, width, height], each 0..1 of the picture".into());
    }
    Ok([a[0], a[1], a[2], a[3]])
}

/// The extended canvas for an outpaint: shares added on each side, or a
/// target aspect (`aspect: "21:9"`) centred.
fn canvas(req: &Request, w: usize, h: usize) -> Result<fill::Placement, String> {
    if let Some(a) = req.s("aspect") {
        let (x, y) = a.split_once(':').and_then(|(x, y)| Some((x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?))).ok_or_else(|| format!("aspect is like \"21:9\", not \"{a}\""))?;
        let want = x / y;
        let have = w as f32 / h as f32;
        let (l, r, t, b) = if want > have { let add = (want / have - 1.0) / 2.0; (add, add, 0.0, 0.0) } else { let add = (have / want - 1.0) / 2.0; (0.0, 0.0, add, add) };
        return Ok(fill::placement(w, h, l, r, t, b));
    }
    let side = |k: &str| req.f(k).unwrap_or(0.0) as f32;
    let (l, r, t, b) = (side("left"), side("right"), side("top"), side("bottom"));
    if l + r + t + b <= 0.0 {
        if let Some(wider) = req.f("wider") {
            let add = (wider as f32 - 1.0).max(0.0) / 2.0;
            return Ok(fill::placement(w, h, add, add, 0.0, 0.0));
        }
        if let Some(taller) = req.f("taller") {
            let add = (taller as f32 - 1.0).max(0.0) / 2.0;
            return Ok(fill::placement(w, h, 0.0, 0.0, add, add));
        }
        return Err("outpaint needs how much to add: left/right/top/bottom as shares of the picture (right: 0.5 adds half its width), wider: 1.5, taller: 1.3, or aspect: \"21:9\"".into());
    }
    Ok(fill::placement(w, h, l, r, t, b))
}

fn direction(req: &Request) -> (f32, f32, String) {
    let d = req.s("direction").unwrap_or("left").to_owned();
    let (az, el) = relight::Light::from_direction(&d).or_else(|| d.trim().trim_end_matches('°').parse::<f32>().ok().map(|a| (a, 30.0))).unwrap_or((180.0, 25.0));
    (az, req.f("elevation").map_or(el, |e| e as f32), d)
}

/// The instruction Codex is given for a picture edit.
fn instruction(req: &Request, size: (usize, usize)) -> Result<String, String> {
    let p = req.prompt.trim();
    let keep = "Keep the composition, framing, subjects and their poses exactly as they are.";
    Ok(match req.task {
        Task::Restyle => {
            let mut s = format!("Redraw the first attached picture in this style: {}.", if p.is_empty() { "the style of the other attached picture" } else { p });
            if !req.refs.is_empty() {
                s.push_str(" Match the look of the other attached picture(s): palette, light, medium and brushwork.");
            }
            format!("{s} {keep}")
        }
        Task::Relight => {
            let (_, _, d) = direction(req);
            let colour = req.s("colour").or(req.s("color")).unwrap_or("");
            format!("Relight the attached picture: the key light comes from the {d}{}{}. Real shadows and highlights to match. {keep}", if colour.is_empty() { String::new() } else { format!(", {colour} light") }, if p.is_empty() { String::new() } else { format!("; {p}") })
        }
        Task::TimeOfDay => {
            let t = req.s("time").or(req.s("time_of_day")).ok_or_else(|| format!("time_of_day needs time: {}", relight::TimeOfDay::NAMES))?;
            format!("Change the time of day in the attached picture to {t}: sky, sun or moon, light direction, shadows, colour temperature{}. {}. {keep}", if t.contains("night") { ", windows and street lights lit" } else { "" }, if p.is_empty() { "Nothing else changes" } else { p })
        }
        Task::Season => {
            let s = req.s("season").ok_or_else(|| format!("season needs season: {}", relight::Season::NAMES))?;
            format!("Change the season in the attached picture to {s}: foliage, ground{}, sky and light. {}. {keep}", if s.eq_ignore_ascii_case("winter") { " (snow where it would settle)" } else { "" }, if p.is_empty() { "Nothing else changes" } else { p })
        }
        Task::Outpaint => {
            let at = canvas(req, size.0, size.1)?;
            format!(
                "Extend the attached picture outwards to a {}×{} canvas: the original sits with its top-left corner at ({}, {}) and stays exactly as it is; continue the scene naturally beyond its edges{}. No borders, no frames.",
                at.width,
                at.height,
                at.x,
                at.y,
                if p.is_empty() { String::new() } else { format!(": {p}") }
            )
        }
        Task::Inpaint => {
            let r = region(req)?;
            if p.is_empty() {
                return Err("inpaint needs a prompt: what to paint in the region (\"empty sky\" to remove something)".into());
            }
            format!("In the attached picture, repaint only the box from {:.0}% to {:.0}% across and {:.0}% to {:.0}% down with: {p}. Match the light and grain. Leave everything outside the box unchanged.", r[0] * 100.0, (r[0] + r[2]) * 100.0, r[1] * 100.0, (r[1] + r[3]) * 100.0)
        }
        other => return Err(format!("{} is not a picture edit", other.name())),
    })
}

/// An extension made by a model, at the canvas size with the original
/// pasted back where it sits (models drift a little). Returns the PNG.
fn paste_original(made: &Path, pic: &Frame, at: fill::Placement) -> Result<PathBuf, String> {
    let m = load_picture(made)?;
    let mut big = if (m.width, m.height) == (at.width, at.height) { m } else { studio_ai::features::upscale::bicubic(&m, at.width, at.height) };
    // Feather the paste over 1 % of the size so no seam shows.
    let feather = (at.width.max(at.height) / 100).max(2) as f32;
    for y in 0..pic.height {
        for x in 0..pic.width {
            let edge = [x as f32, y as f32, (pic.width - 1 - x) as f32, (pic.height - 1 - y) as f32].into_iter().fold(f32::MAX, f32::min);
            // Edges of the original that are edges of the canvas need no feather.
            let k = (edge / feather).min(1.0);
            let k = if (at.x == 0 && x < feather as usize) || (at.y == 0 && y < feather as usize) || (at.x + pic.width == at.width && pic.width - x <= feather as usize) || (at.y + pic.height == at.height && pic.height - y <= feather as usize) { 1.0 } else { k };
            let o = ((at.y + y) * at.width + at.x + x) * 4;
            let s = (y * pic.width + x) * 4;
            for c in 0..3 {
                big.rgba[o + c] = (big.rgba[o + c] as f32 * (1.0 - k) + pic.rgba[s + c] as f32 * k + 0.5) as u8;
            }
            big.rgba[o + 3] = 255;
        }
    }
    let out = made.with_extension("png");
    save_png(&big, &out)?;
    if out != made {
        let _ = std::fs::remove_file(made);
    }
    Ok(out)
}

fn run_codex(req: &Request, status: &mut dyn FnMut(String)) -> Result<Outcome, String> {
    if req.estimate {
        return Ok(Outcome { estimate: true, by: "Codex image generation".into(), price: Some("included in your ChatGPT plan".into()), ..Default::default() });
    }
    let first = picture_in(req)?;
    let pic = load_picture(first)?;
    let mut refs = vec![first.to_path_buf()];
    refs.extend(req.refs.iter().cloned());
    let mut task = format!("Use your image generation tool to edit the first attached picture for a film. {}", instruction(req, (pic.width, pic.height))?);
    if req.task != Task::Outpaint {
        task.push_str(&format!(" The result is {}×{} like the original.", pic.width, pic.height));
    }
    status("Codex is painting it".into());
    let name = if req.name.trim().is_empty() { format!("{} {}", gm::stem(&req.prompt), req.task.name().replace('_', " ")) } else { req.name.clone() };
    let made = gm::codex_run(&task, 1, &refs, &req.dir, name.trim())?;
    // Back to the wanted size, and what must not change pasted back.
    let mut files = Vec::new();
    for f in made {
        let m = load_picture(&f)?;
        let fixed = match req.task {
            Task::Outpaint => {
                let at = canvas(req, pic.width, pic.height)?;
                paste_original(&f, &pic, at)?
            }
            Task::Inpaint => {
                let r = region(req)?;
                let m = if (m.width, m.height) == (pic.width, pic.height) { m } else { studio_ai::features::upscale::bicubic(&m, pic.width, pic.height) };
                let hole = fill::rect_hole(pic.width, pic.height, r);
                let mut out = pic.clone();
                for (i, &h) in hole.iter().enumerate() {
                    if h {
                        out.rgba[i * 4..i * 4 + 3].copy_from_slice(&m.rgba[i * 4..i * 4 + 3]);
                    }
                }
                let p = f.with_extension("png");
                save_png(&out, &p)?;
                p
            }
            _ if (m.width, m.height) != (pic.width, pic.height) => {
                let p = f.with_extension("png");
                save_png(&studio_ai::features::upscale::bicubic(&m, pic.width, pic.height), &p)?;
                p
            }
            _ => f.clone(),
        };
        if fixed != f {
            let _ = std::fs::remove_file(&f);
        }
        files.push(fixed);
    }
    Ok(Outcome { files, by: "Codex image generation (your ChatGPT plan, no API key)".into(), ..Default::default() })
}

// ------------------------------------------------------------ on this Mac

/// Writes frames to an H.264 MP4.
pub struct VideoOut {
    enc: studio_encode::h264::Encoder,
    muxer: studio_encode::mux::Muxer,
    track: usize,
    part: PathBuf,
    file: PathBuf,
    w: u32,
    h: u32,
    pub frames: usize,
}

impl VideoOut {
    /// `w`×`h` (made even) at `fps` into `file`.
    pub fn create(file: &Path, w: usize, h: usize, fps: f64) -> Result<VideoOut, String> {
        use studio_encode::{h264, mux};
        let (w, h) = ((w as u32 & !1).max(16), (h as u32 & !1).max(16));
        if let Some(d) = file.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        let rate = studio_model::time::FrameRate::from_f64(fps, 0.01);
        let (num, den) = (rate.num, rate.den);
        let mut cfg = h264::Config::new(w, h, num, den);
        cfg.bitrate_kbps = ((w * h) as f64 * fps * 0.25 / 1000.0).clamp(2_000.0, 80_000.0) as u32;
        let enc = h264::Encoder::new(cfg).map_err(|e| e.to_string())?;
        let part = file.with_extension("mp4.part");
        let mut muxer = mux::Muxer::create(&part, mux::Options::mp4()).map_err(|e| e.to_string())?;
        let track = muxer.add_video(enc.track_info()).map_err(|e| e.to_string())?;
        Ok(VideoOut { enc, muxer, track, part, file: file.to_path_buf(), w, h, frames: 0 })
    }

    pub fn size(&self) -> (usize, usize) {
        (self.w as usize, self.h as usize)
    }

    pub fn put(&mut self, f: &Frame) -> Result<(), String> {
        let f = if (f.width, f.height) == (self.w as usize, self.h as usize) { std::borrow::Cow::Borrowed(f) } else { std::borrow::Cow::Owned(studio_ai::features::upscale::bicubic(f, self.w as usize, self.h as usize)) };
        let pic = studio_encode::yuv::Yuv420::from_rgba(&f.rgba, self.w, self.h);
        let e = self.enc.encode(&pic);
        self.frames += 1;
        self.muxer.write_sample(self.track, &e.data, e.keyframe).map_err(|e| e.to_string())
    }

    pub fn finish(self) -> Result<PathBuf, String> {
        self.muxer.finish().map_err(|e| e.to_string())?;
        std::fs::rename(&self.part, &self.file).map_err(|e| e.to_string())?;
        Ok(self.file)
    }
}

/// Writes a clip's frames, each passed through `each`, to `file`.
fn write_clip(c: &ClipIn, size: (usize, usize), file: &Path, ctx: &JobCtx, each: &mut dyn FnMut(Frame) -> Result<Frame, String>) -> Result<PathBuf, String> {
    let src_size = ((c.footage.width as usize & !1).max(16), (c.footage.height as usize & !1).max(16));
    let mut frames = super::cutout::RgbaFrames::open(&c.footage, src_size, &c.range)?;
    let mut out: Option<VideoOut> = None;
    let n = (c.range.end - c.range.start) as usize;
    for i in 0..n {
        ctx.check().map_err(|e| e.to_string())?;
        let f = each(frames.get(c.range.start + i as u64)?)?;
        let o = match &mut out {
            Some(o) => o,
            None => out.insert(VideoOut::create(file, if size.0 == 0 { f.width } else { size.0 }, if size.1 == 0 { f.height } else { size.1 }, c.footage.fps)?),
        };
        o.put(&f)?;
        ctx.set_progress((i + 1) as f32 / n.max(1) as f32);
    }
    out.ok_or("the clip has no frames")?.finish()
}

fn store() -> ModelStore {
    ModelStore::default()
}

/// Depth for a picture: Depth Anything when installed, else None.
fn depth_of(f: &Frame) -> Option<studio_ai::Mask> {
    DepthEstimator::load(&store()).ok().and_then(|d| d.estimate(f).ok())
}

/// Seconds of local work a request will take, roughly (for the long gate).
pub fn local_estimate(req: &Request) -> f64 {
    let (frames, px) = match &req.clip {
        Some(c) => ((c.range.end - c.range.start) as f64, c.footage.width as f64 * c.footage.height as f64),
        None if req.task.makes_video() => (req.f("seconds").unwrap_or(5.0) * req.f("fps").unwrap_or(30.0), 1920.0 * 1080.0),
        None => (1.0, 4e6),
    };
    let mpx = px / 1e6;
    let per = match req.task {
        // Real-ESRGAN: about 1.5 s per input megapixel on an M-series Mac.
        Task::Upscale => 1.5 * mpx,
        Task::Enhance => {
            let esr = if store().is_installed(ModelId::RealEsrganGeneralX4v3) && (req.f("scale").unwrap_or(1.0) > 1.0 || req.b("clean").unwrap_or(false)) { 1.5 * mpx } else { 0.0 };
            0.15 * mpx + esr + if req.f("fps").is_some() { 0.6 } else { 0.0 }
        }
        Task::Relight | Task::TimeOfDay if req.clip.is_some() => 0.4 + 0.1 * mpx,
        Task::Between => 0.6,
        _ => 0.08 * mpx,
    };
    frames * (per + 0.25 * mpx)
}

fn run_local(req: &Request, ctx: &JobCtx, status: &mut dyn FnMut(String)) -> Result<Outcome, String> {
    let est = local_estimate(req);
    if req.estimate {
        return Ok(Outcome { estimate: true, by: "this Mac".into(), price: Some("free".into()), video: req.clip.is_some() || req.task.makes_video(), details: json!({"seconds_of_work": est.round()}), ..Default::default() });
    }
    if est > 900.0 && !req.long {
        return Err(format!("that is about {:.0} minutes of work on this Mac: trim the clip shorter, or pass long: true to go ahead", est / 60.0));
    }
    match (req.task, &req.clip) {
        (Task::Animate, _) => animate_local(req, ctx, status),
        (Task::Between, _) => between_local(req, ctx, status),
        (_, Some(c)) => clip_local(req, c, ctx, status),
        (_, None) => picture_local(req, ctx, status),
    }
}

fn animate_local(req: &Request, ctx: &JobCtx, status: &mut dyn FnMut(String)) -> Result<Outcome, String> {
    let still = load_picture(picture_in(req)?)?;
    // At most 1920 wide: each frame is worked out pixel by pixel.
    let k = (1920.0 / still.width.max(still.height) as f32).min(1.0);
    let still = if k < 1.0 { studio_ai::features::upscale::area(&still, (still.width as f32 * k) as usize & !1, (still.height as f32 * k) as usize & !1) } else { still };
    let mv = match req.s("motion").or(req.s("move")) {
        Some(m) => parallax::Move::parse(m).ok_or_else(|| format!("motion is one of {}", parallax::Move::NAMES))?,
        None => parallax::Move::PushIn,
    };
    status("Working out the picture's depth".into());
    let (depth, depth_by) = match depth_of(&still) {
        Some(d) => (d, "Depth Anything V2"),
        None => (parallax::ground_plane(still.width, still.height), "a ground-plane guess (install Depth Anything in Settings ▸ AI models for real depth)"),
    };
    let fps = req.f("fps").unwrap_or(30.0).clamp(12.0, 60.0);
    let seconds = req.f("seconds").unwrap_or(5.0).clamp(1.0, 30.0);
    let n = (seconds * fps).round() as usize;
    let amount = req.f("strength").unwrap_or(1.0) as f32;
    let file = out_path(req, "moving", "mp4");
    let mut out = VideoOut::create(&file, still.width, still.height, fps)?;
    status("Moving the camera through it".into());
    for i in 0..n {
        ctx.check().map_err(|e| e.to_string())?;
        out.put(&parallax::frame(&still, &depth, mv, amount, i, n))?;
        ctx.set_progress((i + 1) as f32 / n as f32);
    }
    let file = out.finish()?;
    Ok(Outcome { files: vec![file], video: true, by: "this Mac (depth parallax)".into(), price: Some("free".into()), details: json!({"motion": format!("{mv:?}"), "seconds": seconds, "fps": fps, "depth": depth_by}), ..Default::default() })
}

fn between_local(req: &Request, ctx: &JobCtx, status: &mut dyn FnMut(String)) -> Result<Outcome, String> {
    let a = load_picture(picture_in(req)?)?;
    let b = load_picture(req.end_image.as_deref().ok_or("between needs end_image: the last frame")?)?;
    let k = (1920.0 / a.width.max(a.height) as f32).min(1.0);
    let (w, h) = (((a.width as f32 * k) as usize & !1).max(16), ((a.height as f32 * k) as usize & !1).max(16));
    let a = studio_ai::features::upscale::bicubic(&a, w, h);
    let b = studio_ai::features::upscale::bicubic(&b, w, h);
    let fi = FrameInterpolator::load(&store()).map_err(|e| e.to_string())?;
    let fps = req.f("fps").unwrap_or(30.0).clamp(12.0, 60.0);
    let seconds = req.f("seconds").unwrap_or(2.0).clamp(0.5, 10.0);
    let n = ((seconds * fps).round() as usize).max(2);
    let file = out_path(req, "between", "mp4");
    let mut out = VideoOut::create(&file, w, h, fps)?;
    status("Making the in-betweens".into());
    for i in 0..n {
        ctx.check().map_err(|e| e.to_string())?;
        let t = i as f32 / (n - 1) as f32;
        let t = t * t * (3.0 - 2.0 * t);
        let f = if i == 0 { a.clone() } else if i + 1 == n { b.clone() } else { fi.interpolate(&a, &b, t).map_err(|e| e.to_string())? };
        out.put(&f)?;
        ctx.set_progress((i + 1) as f32 / n as f32);
    }
    let file = out.finish()?;
    let engine = if fi.engine() == studio_ai::features::interpolate::InterpEngine::Rife { "RIFE" } else { "cross-fades (install RIFE in Settings ▸ AI models)" };
    Ok(Outcome { files: vec![file], video: true, by: format!("this Mac ({engine})"), price: Some("free".into()), details: json!({"seconds": seconds, "fps": fps}), ..Default::default() })
}

/// The look reference's statistics: from the refs, or a named look.
fn reference_stats(req: &Request) -> Result<look::Stats, String> {
    let pics: Vec<Frame> = req.refs.iter().map(|r| load_picture(r)).collect::<Result<_, _>>()?;
    if pics.is_empty() {
        return Err("restyling on this Mac copies the look of a reference picture: pass style_ref (a file, a media id or {\"frame\": seconds}); a style in words needs Codex".into());
    }
    Ok(look::stats(&pics.iter().collect::<Vec<_>>()))
}

/// One picture through the local task.
fn picture_step(req: &Request, f: &Frame, depth: Option<&studio_ai::Mask>, reference: Option<&look::Stats>, from: Option<&look::Stats>) -> Result<Frame, String> {
    let amount = req.f("strength").unwrap_or(1.0) as f32;
    Ok(match req.task {
        Task::Restyle => {
            let to = reference.ok_or("no reference look")?;
            let own;
            let from = match from {
                Some(s) => s,
                None => {
                    own = look::stats(&[f]);
                    &own
                }
            };
            look::transfer(f, from, to, req.f("strength").unwrap_or(0.85) as f32)
        }
        Task::Relight => {
            let (az, el, _) = direction(req);
            let colour = req.s("colour").or(req.s("color")).map(|c| relight::colour_by_name(c).ok_or_else(|| format!("colour \"{c}\": a name (warm, golden, blue, moonlight, neon…) or #rrggbb"))).transpose()?.unwrap_or([1.0, 0.95, 0.88]);
            relight::relight(f, depth, &relight::Light { azimuth: az, elevation: el, colour, strength: amount.clamp(0.0, 2.0) * 0.9, ambient: req.f("ambient").unwrap_or(0.55) as f32 })
        }
        Task::TimeOfDay => {
            let t = req.s("time").or(req.s("time_of_day")).ok_or_else(|| format!("time_of_day needs time: {}", relight::TimeOfDay::NAMES))?;
            let t = relight::TimeOfDay::parse(t).ok_or_else(|| format!("time is one of {}", relight::TimeOfDay::NAMES))?;
            relight::time_of_day(f, depth, t, amount)
        }
        Task::Season => {
            let s = req.s("season").ok_or_else(|| format!("season needs season: {}", relight::Season::NAMES))?;
            let s = relight::Season::parse(s).ok_or_else(|| format!("season is one of {}", relight::Season::NAMES))?;
            relight::season(f, s, amount)
        }
        other => return Err(format!("{} has no per-frame step", other.name())),
    })
}

fn picture_local(req: &Request, ctx: &JobCtx, status: &mut dyn FnMut(String)) -> Result<Outcome, String> {
    let path = picture_in(req)?;
    let pic = load_picture(path)?;
    let mut details = json!({});
    let (made, suffix) = match req.task {
        Task::Outpaint => {
            let at = canvas(req, pic.width, pic.height)?;
            let (big, _) = fill::extend(&pic, at);
            details = json!({"canvas": [at.width, at.height], "original_at": [at.x, at.y]});
            (big, "wider")
        }
        Task::Inpaint => {
            let hole = fill::rect_hole(pic.width, pic.height, region(req)?);
            (fill::inpaint(&pic, &hole), "filled")
        }
        Task::Upscale => {
            let up = Upscaler::load(&store()).map_err(|e| e.to_string())?;
            let scale = req.f("scale").unwrap_or(2.0).clamp(1.0, 4.0) as f32;
            status(format!("Upscaling {scale}× with {}", if up.engine() == studio_ai::features::upscale::UpscaleEngine::RealEsrgan { "Real-ESRGAN" } else { "bicubic" }));
            let g = up.upscale(&pic, scale, ctx).map_err(|e| e.to_string())?;
            details = json!({"size": [g.width, g.height], "engine": engine_name(&up)});
            (g, "upscaled")
        }
        Task::Enhance => {
            let (g, steps) = enhance_still(req, &pic, ctx, status)?;
            details = json!({"steps": steps, "size": [g.width, g.height]});
            (g, "enhanced")
        }
        _ => {
            let depth = if matches!(req.task, Task::Relight | Task::TimeOfDay) {
                status("Working out the picture's depth".into());
                depth_of(&pic)
            } else {
                None
            };
            let reference = if req.task == Task::Restyle { Some(reference_stats(req)?) } else { None };
            details = json!({"depth": depth.is_some()});
            (picture_step(req, &pic, depth.as_ref(), reference.as_ref(), None)?, req.task.name())
        }
    };
    let file = out_path(req, &suffix.replace('_', " "), "png");
    save_png(&made, &file)?;
    Ok(Outcome { files: vec![file], by: "this Mac".into(), price: Some("free".into()), details, ..Default::default() })
}

fn engine_name(up: &Upscaler) -> &'static str {
    match up.engine() {
        studio_ai::features::upscale::UpscaleEngine::RealEsrgan => "Real-ESRGAN",
        studio_ai::features::upscale::UpscaleEngine::Bicubic => "bicubic (install Real-ESRGAN in Settings ▸ AI models for real detail)",
    }
}

/// The restoration steps asked for (`steps: [...]`), or worked out from
/// the picture (`auto`).
fn enhance_steps(req: &Request, sample: &Frame, video: bool) -> Vec<&'static str> {
    let asked: Vec<String> = req.settings.get("steps").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_ascii_lowercase).collect()).unwrap_or_default();
    let old = req.b("old_film").unwrap_or(false) || asked.iter().any(|s| s == "old_film");
    let mut steps = Vec::new();
    let has = |s: &str| asked.iter().any(|a| a == s);
    let auto = asked.is_empty() || has("auto");
    if old || has("dust") {
        steps.push("dust");
    }
    if old || has("scratches") {
        steps.push("scratches");
    }
    if video && (old || has("deflicker")) {
        steps.push("deflicker");
    }
    if has("deblock") || (auto && restore::blockiness(sample) > 0.12) {
        steps.push("deblock");
    }
    if has("denoise") || old || (auto && restore::noise_level(sample) > 0.012) {
        steps.push("denoise");
    }
    if has("fade") || has("colour") || old {
        steps.push("fade");
    }
    if has("faces") || req.b("faces") == Some(true) {
        steps.push("faces");
    }
    if has("clean") || req.b("clean") == Some(true) {
        steps.push("clean");
    }
    if req.f("scale").is_some_and(|s| s > 1.0) || req.f("height").is_some() || has("upscale") {
        steps.push("upscale");
    }
    if has("sharpen") {
        steps.push("sharpen");
    }
    steps
}

/// One frame through the enhance steps (temporal ones need `prev`/`next`).
#[allow(clippy::too_many_arguments)]
fn enhance_frame(req: &Request, steps: &[&str], f: Frame, prev: Option<&Frame>, next: Option<&Frame>, deflicker: &mut restore::Deflicker, up: &Upscaler, faces: Option<&studio_ai::features::faces::FaceDetector>, out_size: (usize, usize), ctx: &JobCtx) -> Result<Frame, String> {
    let mut f = f;
    let strength = req.f("strength").unwrap_or(0.6) as f32;
    for &s in steps {
        f = match s {
            "dust" => match (prev, next) {
                (Some(p), Some(n)) => restore::Dust::default().temporal(p, &f, n).0,
                _ => restore::Dust::default().spatial(&f),
            },
            "scratches" => restore::unscratch(&f).0,
            "deflicker" => deflicker.next(&f),
            "deblock" => restore::deblock(&f, 18.0),
            "denoise" => restore::denoise(&f, strength),
            "fade" => restore::restore_fade(&f, 0.8),
            "clean" => up.clean(&f, ctx).map_err(|e| e.to_string())?,
            "faces" => clean_faces(&f, up, faces, ctx)?,
            "sharpen" => studio_ai::features::upscale::sharpen(&f, 0.4),
            "upscale" if (f.width, f.height) != out_size => {
                let (w, h) = out_size;
                if w <= f.width * 4 && h <= f.height * 4 && w >= f.width {
                    up.upscale_to(&f, w, h, ctx).map_err(|e| e.to_string())?
                } else {
                    studio_ai::features::upscale::bicubic(&f, w, h)
                }
            }
            _ => f,
        };
    }
    Ok(f)
}

/// Faces cleaned: each face's box run through the upscaler's clean-up and
/// feathered back.
fn clean_faces(f: &Frame, up: &Upscaler, det: Option<&studio_ai::features::faces::FaceDetector>, ctx: &JobCtx) -> Result<Frame, String> {
    let Some(det) = det else { return Ok(f.clone()) };
    let found = det.detect(f).map_err(|e| e.to_string())?;
    let mut out = f.clone();
    for face in found {
        let b = face.bbox;
        let (bx, by, bw, bh) = (b.x.max(0.0) as usize, b.y.max(0.0) as usize, b.w.max(0.0) as usize, b.h.max(0.0) as usize);
        let pad = (bw.max(bh) as f32 * 0.35) as usize;
        let (x0, y0) = (bx.saturating_sub(pad), by.saturating_sub(pad));
        let (x1, y1) = ((bx + bw + pad).min(f.width), (by + bh + pad).min(f.height));
        if x1 <= x0 + 8 || y1 <= y0 + 8 {
            continue;
        }
        let mut px = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
        for yy in y0..y1 {
            px.extend_from_slice(&f.rgba[(yy * f.width + x0) * 4..(yy * f.width + x1) * 4]);
        }
        let crop = Frame::new(x1 - x0, y1 - y0, px).map_err(|e| e.to_string())?;
        let clean = studio_ai::features::upscale::sharpen(&up.clean(&crop, ctx).map_err(|e| e.to_string())?, 0.25);
        let (cw, ch) = (clean.width as f32, clean.height as f32);
        for y in 0..clean.height {
            for x in 0..clean.width {
                let e = [x as f32, y as f32, cw - 1.0 - x as f32, ch - 1.0 - y as f32].into_iter().fold(f32::MAX, f32::min);
                let k = (e / (pad.max(1) as f32 * 0.6)).min(1.0);
                let o = ((y0 + y) * f.width + x0 + x) * 4;
                let s = (y * clean.width + x) * 4;
                for c in 0..3 {
                    out.rgba[o + c] = (out.rgba[o + c] as f32 * (1.0 - k) + clean.rgba[s + c] as f32 * k + 0.5) as u8;
                }
            }
        }
    }
    Ok(out)
}

fn out_size_for(req: &Request, w: usize, h: usize) -> (usize, usize) {
    let even = |v: f64| ((v.round() as usize) & !1).max(16);
    if let Some(t) = req.f("height") {
        let k = t / h as f64;
        return (even(w as f64 * k), even(t));
    }
    let s = req.f("scale").unwrap_or(1.0).clamp(1.0, 4.0);
    // Clips stop at 4K.
    let k = s.min(3840.0 / w.max(1) as f64).min(2160.0 / h.max(1) as f64).max(1.0).min(s.max(1.0));
    (even(w as f64 * k), even(h as f64 * k))
}

fn enhance_still(req: &Request, pic: &Frame, ctx: &JobCtx, status: &mut dyn FnMut(String)) -> Result<(Frame, Vec<&'static str>), String> {
    let steps = enhance_steps(req, pic, false);
    let up = Upscaler::load(&store()).map_err(|e| e.to_string())?;
    let faces = studio_ai::features::faces::FaceDetector::load(&store()).ok();
    status(format!("Enhancing: {}", if steps.is_empty() { "nothing needed".into() } else { steps.join(", ") }));
    let s = req.f("scale").unwrap_or(1.0).clamp(1.0, 4.0);
    let size = (((pic.width as f64 * s).round() as usize).max(1), ((pic.height as f64 * s).round() as usize).max(1));
    let size = if let Some(t) = req.f("height") { (((pic.width as f64 * t / pic.height as f64).round() as usize).max(1), t as usize) } else { size };
    let g = enhance_frame(req, &steps, pic.clone(), None, None, &mut restore::Deflicker::default(), &up, faces.as_ref(), size, ctx)?;
    Ok((g, steps))
}

fn clip_local(req: &Request, c: &ClipIn, ctx: &JobCtx, status: &mut dyn FnMut(String)) -> Result<Outcome, String> {
    let src_size = ((c.footage.width as usize & !1).max(16), (c.footage.height as usize & !1).max(16));
    let mut frames = super::cutout::RgbaFrames::open(&c.footage, src_size, &c.range)?;
    let n = (c.range.end - c.range.start) as usize;
    let first = frames.get(c.range.start)?;
    let (suffix, out_size, steps, fps_factor) = match req.task {
        Task::Enhance | Task::Upscale => {
            let mut steps = enhance_steps(req, &first, true);
            if req.task == Task::Upscale && !steps.contains(&"upscale") {
                steps.push("upscale");
            }
            let scale_default = if req.task == Task::Upscale && req.f("scale").is_none() && req.f("height").is_none() { 2.0 } else { 1.0 };
            let size = if scale_default > 1.0 { out_size_for(&Request { settings: json!({"scale": scale_default}), ..req.clone() }, src_size.0, src_size.1) } else { out_size_for(req, src_size.0, src_size.1) };
            // Frame interpolation: to `fps` (a whole multiple of the footage's).
            let factor = match req.f("fps") {
                Some(target) if target > c.footage.fps * 1.05 => ((target / c.footage.fps).round() as usize).clamp(2, 8),
                _ => 1,
            };
            (if req.task == Task::Upscale { "upscaled" } else { "enhanced" }, size, steps, factor)
        }
        _ => (req.task.name(), src_size, Vec::new(), 1),
    };
    let up = Upscaler::load(&store()).map_err(|e| e.to_string())?;
    let faces = if steps.contains(&"faces") { studio_ai::features::faces::FaceDetector::load(&store()).ok() } else { None };
    let fi = if fps_factor > 1 { Some(FrameInterpolator::load(&store()).map_err(|e| e.to_string())?) } else { None };
    let reference = if req.task == Task::Restyle { Some(reference_stats(req)?) } else { None };
    // The clip's own look, measured over a dozen frames so all move alike.
    let from = if req.task == Task::Restyle {
        let picks: Vec<Frame> = (0..12).map(|k| c.range.start + (k * n / 12) as u64).filter_map(|i| frames.get(i).ok()).collect();
        Some(look::stats(&picks.iter().collect::<Vec<_>>()))
    } else {
        None
    };
    let mut depth = if matches!(req.task, Task::Relight | Task::TimeOfDay) { DepthEstimator::load(&store()).ok() } else { None };
    let file = out_path(req, &suffix.replace('_', " "), "mp4");
    let mut out = VideoOut::create(&file, out_size.0, out_size.1, c.footage.fps * fps_factor as f64)?;
    let mut deflicker = restore::Deflicker::default();
    status(match req.task {
        Task::Enhance | Task::Upscale => format!("{} {n} frames: {}", if req.task == Task::Upscale { "Upscaling" } else { "Enhancing" }, steps.join(", ")),
        t => format!("{} {n} frames", t.name().replace('_', " ")),
    });
    let mut prev_out: Option<Frame> = None;
    for i in 0..n {
        ctx.check().map_err(|e| e.to_string())?;
        let idx = c.range.start + i as u64;
        let cur = frames.get(idx)?;
        let made = match req.task {
            Task::Enhance | Task::Upscale => {
                let prev = if i > 0 { frames.get(idx - 1).ok() } else { None };
                let next = if i + 1 < n { frames.get(idx + 1).ok() } else { None };
                enhance_frame(req, &steps, cur, prev.as_ref(), next.as_ref(), &mut deflicker, &up, faces.as_ref(), out_size, ctx)?
            }
            _ => {
                let d = match &mut depth {
                    Some(e) => e.next_frame(&cur).ok(),
                    None => None,
                };
                picture_step(req, &cur, d.as_ref(), reference.as_ref(), from.as_ref())?
            }
        };
        // In-betweens before each new frame.
        if let (Some(fi), Some(p)) = (&fi, &prev_out) {
            for s in 1..fps_factor {
                let t = s as f32 / fps_factor as f32;
                out.put(&fi.interpolate(p, &made, t).map_err(|e| e.to_string())?)?;
            }
        }
        out.put(&made)?;
        prev_out = Some(made);
        ctx.set_progress((i + 1) as f32 / n as f32);
    }
    // The last frame is held for its in-between slots, so lengths match.
    if let Some(p) = &prev_out {
        for _ in 1..fps_factor {
            out.put(p)?;
        }
    }
    let file = out.finish()?;
    let mut details = json!({"frames": n, "size": [out_size.0, out_size.1]});
    if !steps.is_empty() {
        details["steps"] = json!(steps);
    }
    if fps_factor > 1 {
        details["fps"] = json!(c.footage.fps * fps_factor as f64);
        details["interpolation"] = json!(if fi.as_ref().is_some_and(|f| f.engine() == studio_ai::features::interpolate::InterpEngine::Rife) { "RIFE" } else { "cross-fades" });
    }
    if steps.contains(&"upscale") || steps.contains(&"clean") {
        details["upscaler"] = json!(engine_name(&up));
    }
    Ok(Outcome { files: vec![file], video: true, by: "this Mac".into(), price: Some("free".into()), details, ..Default::default() })
}

// ------------------------------------------------------------------ takes

/// How many takes to make and choose from (`best_of`, 1 to 6).
fn best_of(req: &Request) -> u32 {
    req.f("best_of").map_or(1, |n| n.round().clamp(1.0, 6.0) as u32)
}

/// How good a take is, from what can be measured: the composition kept
/// (edges where the original's are), detail without noise, no clipped
/// highlights or crushed shadows, and closeness to the look being
/// followed. Higher is better; each part is reported.
pub fn score_take(take: &Frame, original: Option<&Frame>, reference: Option<&look::Stats>) -> (f32, Value) {
    let (w, h) = (take.width, take.height);
    let luma = take.luma();
    // Detail: mean absolute Laplacian, against noise in flat areas.
    let mut lap = 0f32;
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let i = y * w + x;
            lap += (4.0 * luma[i] - luma[i - 1] - luma[i + 1] - luma[i - w] - luma[i + w]).abs();
        }
    }
    let detail = (lap / ((w.saturating_sub(2) * h.saturating_sub(2)).max(1)) as f32 * 20.0).min(1.0);
    let noise = (restore::noise_level(take) * 25.0).min(1.0);
    let clipped = take.rgba.chunks_exact(4).filter(|p| p[..3].iter().all(|&v| v >= 254) || p[..3].iter().all(|&v| v <= 1)).count() as f32 / (w * h).max(1) as f32;
    let mut parts = json!({"detail": detail, "noise": noise, "clipped": clipped});
    let mut score = 0.35 * detail - 0.25 * noise - 1.5 * clipped.min(0.3);
    if let Some(o) = original {
        // Composition: correlation of edge maps at a common small size.
        let edges = |f: &Frame| {
            let s = studio_ai::features::upscale::area(f, 96, (96 * f.height / f.width.max(1)).max(8));
            let l = s.luma();
            let (sw, sh) = (s.width, s.height);
            let mut e = vec![0f32; sw * sh];
            for y in 1..sh - 1 {
                for x in 1..sw - 1 {
                    let i = y * sw + x;
                    e[i] = (l[i + 1] - l[i - 1]).abs() + (l[i + sw] - l[i - sw]).abs();
                }
            }
            (e, sw, sh)
        };
        let (a, aw, ah) = edges(o);
        let (b, bw, bh) = edges(take);
        let kept = if (aw, ah) == (bw, bh) {
            let (ma, mb) = (a.iter().sum::<f32>() / a.len() as f32, b.iter().sum::<f32>() / b.len() as f32);
            let (mut num, mut da, mut db) = (0f32, 0f32, 0f32);
            for (x, y) in a.iter().zip(&b) {
                num += (x - ma) * (y - mb);
                da += (x - ma).powi(2);
                db += (y - mb).powi(2);
            }
            (num / (da.sqrt() * db.sqrt()).max(1e-6)).clamp(-1.0, 1.0)
        } else {
            0.0
        };
        parts["composition_kept"] = json!(kept);
        score += 0.6 * kept;
    }
    if let Some(r) = reference {
        let s = look::stats(&[take]);
        let d: f32 = (0..3).map(|c| ((s.mean[c] - r.mean[c]) / r.sd[c].max(0.05)).abs() + ((s.sd[c] - r.sd[c]) / r.sd[c].max(0.05)).abs() * 0.5).sum::<f32>() / 3.0;
        let close = 1.0 / (1.0 + d);
        parts["look_match"] = json!(close);
        score += 0.5 * close;
    }
    parts["score"] = json!(score);
    (score, parts)
}

/// Several takes by Codex, each with its own seed, scored; the
/// best is the result and the rest are kept in `Takes/` beside it.
fn takes(req: &Request, status: &mut dyn FnMut(String)) -> Result<Outcome, String> {
    let n = best_of(req);
    let original = req.image.as_deref().and_then(|p| load_picture(p).ok());
    let reference = if req.refs.is_empty() { None } else { reference_stats(req).ok() };
    // Composition counts for edits of the picture, not for extensions
    // (another size) or a new style (strokes change on purpose).
    let compare = matches!(req.task, Task::Relight | Task::TimeOfDay | Task::Season | Task::Inpaint | Task::Upscale | Task::Enhance);
    let base = req.seed.unwrap_or_else(|| seed_for(&format!("{}{}", req.prompt, req.task.name())));
    let mut made: Vec<(f32, PathBuf, Value)> = Vec::new();
    let mut last = Outcome::default();
    let mut errors = Vec::new();
    for k in 0..n {
        status(format!("Take {} of {n}", k + 1));
        let mut one = req.clone();
        one.seed = Some(base.wrapping_add(k as u64 * 7919) % 4_294_967_291);
        one.settings["best_of"] = json!(1);
        if k > 0 {
            one.name = format!("{} take {}", if req.name.is_empty() { gm::stem(&req.prompt) } else { req.name.clone() }, k + 1);
        }
        let out = run_codex(&one, status);
        match out {
            Ok(o) => {
                for f in &o.files {
                    let pic = load_picture(f)?;
                    let orig = original.as_ref().filter(|_| compare).map(|o| if (o.width, o.height) == (pic.width, pic.height) { o.clone() } else { studio_ai::features::upscale::bicubic(o, pic.width, pic.height) });
                    let (s, parts) = score_take(&pic, orig.as_ref(), reference.as_ref());
                    made.push((s, f.clone(), parts));
                }
                last = o;
            }
            Err(e) => errors.push(e),
        }
    }
    if made.is_empty() {
        return Err(errors.into_iter().next().unwrap_or_else(|| "no take was made".into()));
    }
    made.sort_by(|a, b| b.0.total_cmp(&a.0));
    let keep = req.dir.join("Takes");
    let _ = std::fs::create_dir_all(&keep);
    let mut others = Vec::new();
    for (_, f, parts) in made.iter().skip(1) {
        let dest = keep.join(f.file_name().unwrap_or_default());
        if std::fs::rename(f, &dest).is_ok() {
            others.push(json!({"file": dest.display().to_string(), "scores": parts}));
        }
    }
    let (_, best, parts) = made.remove(0);
    last.files = vec![best];
    last.details["best_of"] = json!(n);
    last.details["chosen"] = parts;
    last.details["other_takes"] = json!(others);
    if !errors.is_empty() {
        last.details["failed_takes"] = json!(errors);
    }
    last.by = format!("{} (best of {n})", last.by);
    Ok(last)
}

// ----------------------------------------------------------- recipes, looks

/// The recipe file beside a generated file.
pub fn recipe_path(file: &Path) -> PathBuf {
    let mut s = file.as_os_str().to_owned();
    s.push(".recipe.json");
    PathBuf::from(s)
}

fn write_recipe(file: &Path, req: &Request, out: &Outcome) {
    // Inputs that live in a temporary folder (rendered frames) are kept
    // with the project, so the recipe still works later.
    let keep = |p: &Path| -> String {
        if p.starts_with(std::env::temp_dir()) || p.starts_with("/tmp") || p.starts_with("/private/var") {
            let dir = req.dir.join("Sources");
            let dest = dir.join(p.file_name().unwrap_or_default());
            if std::fs::create_dir_all(&dir).is_ok() && (dest.exists() || std::fs::copy(p, &dest).is_ok()) {
                return dest.display().to_string();
            }
        }
        p.display().to_string()
    };
    let p = |v: &Option<PathBuf>| v.as_deref().map(keep);
    let r = json!({
        "task": req.task.name(),
        "provider": out.provider.map(|p| p.name()),
        "by": out.by,
        "prompt": req.prompt,
        "seed": req.seed,
        "settings": req.settings,
        "image": p(&req.image),
        "end_image": p(&req.end_image),
        "refs": req.refs.iter().map(|r| keep(r)).collect::<Vec<_>>(),
        "clip": req.clip.as_ref().map(|c| json!({"file": c.footage.path.display().to_string(), "frames": [c.range.start, c.range.end], "fps": c.footage.fps})),
        "price": out.price,
        "made": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
    });
    let _ = std::fs::write(recipe_path(file), serde_json::to_string_pretty(&r).unwrap_or_default());
}

/// Writes the recipe of a plain generation (`generate` without a task).
pub fn write_make_recipe(file: &Path, args: &Value, by: &str, price: Option<&str>) {
    let mut a = args.clone();
    // Data and frames are not kept: files and moments are.
    if let Some(o) = a.as_object_mut() {
        for k in ["estimate", "max_price", "to_timeline", "at"] {
            o.remove(k);
        }
    }
    let r = json!({"task": "make", "by": by, "args": a, "price": price, "made": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())});
    let _ = std::fs::write(recipe_path(file), serde_json::to_string_pretty(&r).unwrap_or_default());
}

/// A recipe, if the file has one.
pub fn recipe(file: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(recipe_path(file)).ok()?).ok()
}

/// A character or place kept the same across generations.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Look {
    pub name: String,
    /// What they look like, added to every prompt.
    pub words: String,
    /// Reference pictures (files).
    pub refs: Vec<String>,
    pub seed: Option<u64>,
    /// Where it came from: "looks" (saved here) or "story" (the bible).
    pub from: &'static str,
}

impl Look {
    pub fn to_json(&self) -> Value {
        json!({"name": self.name, "words": self.words, "refs": self.refs, "seed": self.seed, "from": self.from})
    }
}

fn looks_file(dir: &Path) -> PathBuf {
    dir.join("looks.json")
}

/// The looks saved in a project's Generated folder.
pub fn looks(dir: &Path) -> Vec<Look> {
    let v: Value = std::fs::read_to_string(looks_file(dir)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({}));
    v.as_object()
        .map(|o| {
            o.iter()
                .map(|(k, l)| Look {
                    name: k.clone(),
                    words: l["words"].as_str().unwrap_or("").to_owned(),
                    refs: l["refs"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default(),
                    seed: l["seed"].as_u64(),
                    from: "looks",
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Saves (or replaces) a look.
pub fn save_look(dir: &Path, l: &Look) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut v: Value = std::fs::read_to_string(looks_file(dir)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({}));
    if !v.is_object() {
        v = json!({});
    }
    v[l.name.as_str()] = json!({"words": l.words, "refs": l.refs, "seed": l.seed});
    std::fs::write(looks_file(dir), serde_json::to_string_pretty(&v).unwrap_or_default()).map_err(|e| e.to_string())
}

/// Removes a look; whether it was there.
pub fn forget_look(dir: &Path, name: &str) -> bool {
    let Some(mut v) = std::fs::read_to_string(looks_file(dir)).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()) else { return false };
    let key = v.as_object().and_then(|o| o.keys().find(|k| k.eq_ignore_ascii_case(name)).cloned());
    let gone = key.is_some_and(|k| v.as_object_mut().is_some_and(|o| o.remove(&k).is_some()));
    if gone {
        let _ = std::fs::write(looks_file(dir), serde_json::to_string_pretty(&v).unwrap_or_default());
    }
    gone
}

/// The cast and places of the project's story bible as looks (read from
/// the saved project, so it works whatever version wrote it): a
/// character's look and reference pictures, a place's backdrop and light.
pub fn story_looks(project: &Value) -> Vec<Look> {
    let story = &project["story"];
    let mut out = Vec::new();
    for c in story["cast"].as_array().into_iter().flatten() {
        let id = c["id"].as_str().unwrap_or("").to_owned();
        if id.is_empty() {
            continue;
        }
        let name = c["name"].as_str().filter(|n| !n.is_empty()).unwrap_or(&id);
        let words = [Some(name), c["look"].as_str()].into_iter().flatten().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(": ");
        out.push(Look { name: id, words, refs: c["references"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default(), seed: c["seed"].as_u64(), from: "story" });
    }
    for p in story["places"].as_array().into_iter().flatten() {
        let id = p["id"].as_str().unwrap_or("").to_owned();
        if id.is_empty() {
            continue;
        }
        let words = [p["name"].as_str(), p["light"].as_str(), p["notes"].as_str()].into_iter().flatten().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ");
        let refs = p["backdrop"].as_str().filter(|b| Path::new(b).is_file()).map(|b| vec![b.to_owned()]).unwrap_or_default();
        out.push(Look { name: id, words, refs, seed: p["seed"].as_u64(), from: "story" });
    }
    out
}

/// The look called `name`: saved looks first, then the story bible.
pub fn find_look(dir: &Path, project: &Value, name: &str) -> Option<Look> {
    looks(dir).into_iter().chain(story_looks(project)).find(|l| l.name.eq_ignore_ascii_case(name.trim()))
}

/// A stable seed for a name (a look without one still repeats).
pub fn seed_for(name: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.to_ascii_lowercase().bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h % 4_294_967_291
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("studio-genmedia-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn picture(dir: &Path) -> PathBuf {
        let mut f = Frame::filled(96, 54, [90, 140, 70, 255]);
        for y in 0..54 {
            for x in 0..96 {
                if (x / 12 + y / 9) % 2 == 0 {
                    f.rgba[(y * 96 + x) * 4..(y * 96 + x) * 4 + 3].copy_from_slice(&[200, 180, 150]);
                }
            }
        }
        let p = dir.join("still.png");
        save_png(&f, &p).unwrap();
        p
    }

    #[test]
    fn routes_pick_what_is_here() {
        let none = Available { codex: false };
        let codex = Available { codex: true };
        assert_eq!(route(Task::Animate, false, Some(Provider::Local), none).unwrap(), Provider::Local);
        assert_eq!(route(Task::Upscale, false, None, codex).unwrap(), Provider::Local);
        assert_eq!(route(Task::Relight, true, None, codex).unwrap(), Provider::Local);
        assert!(route(Task::Outpaint, true, None, codex).is_err());
        assert!(route(Task::Animate, false, Some(Provider::Codex), codex).unwrap_err().contains("it can by local"));
        assert_eq!(route(Task::Season, false, Some(Provider::Codex), codex).unwrap(), Provider::Codex);
        for t in Task::ALL {
            assert_eq!(Task::parse(t.name()), Some(t));
        }
    }

    #[test]
    fn local_picture_tasks_make_files_with_recipes() {
        let dir = scratch("pics");
        let still = picture(&dir);
        let run1 = |task: Task, settings: Value, refs: Vec<PathBuf>| {
            let mut r = Request::new(task, dir.join("Generated"));
            r.provider = Some(Provider::Local);
            r.image = Some(still.clone());
            r.settings = settings;
            r.refs = refs;
            r.prompt = "test".into();
            run(&r, &JobCtx::detached(), &mut |_| {})
        };
        let o = run1(Task::Outpaint, json!({"right": 0.5}), vec![]).unwrap();
        let big = load_picture(&o.files[0]).unwrap();
        assert_eq!((big.width, big.height), (144, 54));
        let rec = recipe(&o.files[0]).unwrap();
        assert_eq!(rec["task"], "outpaint");
        assert_eq!(rec["provider"], "local");
        let o = run1(Task::Inpaint, json!({"region": [0.1, 0.1, 0.2, 0.2]}), vec![]).unwrap();
        assert!(o.files[0].exists());
        let o = run1(Task::Relight, json!({"direction": "left", "colour": "warm"}), vec![]).unwrap();
        assert!(o.files[0].exists());
        let o = run1(Task::TimeOfDay, json!({"time": "night"}), vec![]).unwrap();
        let night = load_picture(&o.files[0]).unwrap();
        assert!(night.px(5, 5)[0] < 150);
        assert!(run1(Task::Restyle, json!({}), vec![]).unwrap_err().contains("style_ref"));
        let o = run1(Task::Restyle, json!({}), vec![still.clone()]).unwrap();
        assert!(o.files[0].exists());
        let o = run1(Task::Upscale, json!({"scale": 2}), vec![]).unwrap();
        assert_eq!(load_picture(&o.files[0]).unwrap().width, 192);
        let o = run1(Task::Enhance, json!({"steps": ["denoise", "fade"]}), vec![]).unwrap();
        assert_eq!(o.details["steps"], json!(["denoise", "fade"]));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_still_is_animated_and_two_frames_joined() {
        let dir = scratch("anim");
        let still = picture(&dir);
        let mut r = Request::new(Task::Animate, dir.join("Generated"));
        r.provider = Some(Provider::Local);
        r.image = Some(still.clone());
        r.settings = json!({"seconds": 1, "fps": 12, "motion": "pan_right"});
        let o = run(&r, &JobCtx::detached(), &mut |_| {}).unwrap();
        assert!(o.video);
        let f = footage_of(&o.files[0]).unwrap();
        assert_eq!(f.frames, 12);
        r.task = Task::Between;
        r.end_image = Some(still.clone());
        r.settings = json!({"seconds": 1, "fps": 12});
        let o = run(&r, &JobCtx::detached(), &mut |_| {}).unwrap();
        assert_eq!(footage_of(&o.files[0]).unwrap().frames, 12);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn looks_are_saved_and_read_from_the_story() {
        let dir = scratch("looks");
        save_look(&dir, &Look { name: "fern".into(), words: "a red fox with a white-tipped tail".into(), refs: vec!["/x/fern.png".into()], seed: Some(7), from: "looks" }).unwrap();
        let story = json!({"story": {"cast": [{"id": "owl", "name": "Olu", "look": "a small grey owl with round glasses"}], "places": [{"id": "meadow", "name": "The meadow", "light": "low warm sun from the left"}]}});
        assert_eq!(find_look(&dir, &story, "Fern").unwrap().seed, Some(7));
        let owl = find_look(&dir, &story, "owl").unwrap();
        assert_eq!(owl.from, "story");
        assert!(owl.words.contains("round glasses"));
        assert!(find_look(&dir, &story, "meadow").unwrap().words.contains("warm sun"));
        assert!(forget_look(&dir, "FERN"));
        assert!(find_look(&dir, &story, "fern").is_none());
        assert_eq!(seed_for("Fern"), seed_for("fern"));
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod take_tests {
    use super::*;

    fn pattern(w: usize, h: usize, shift: usize) -> Frame {
        let mut f = Frame::filled(w, h, [0, 0, 0, 255]);
        for y in 0..h {
            for x in 0..w {
                let v = if ((x + shift) / 8 + y / 8) % 2 == 0 { 200 } else { 60 };
                f.rgba[(y * w + x) * 4..(y * w + x) * 4 + 3].copy_from_slice(&[v, v, v]);
            }
        }
        f
    }

    #[test]
    fn takes_that_keep_the_picture_and_its_detail_score_higher() {
        let orig = pattern(96, 64, 0);
        let soft = studio_ai::features::upscale::bicubic(&studio_ai::features::upscale::area(&orig, 24, 16), 96, 64);
        let moved = pattern(96, 64, 4);
        let (same, _) = score_take(&orig, Some(&orig), None);
        let (blurred, _) = score_take(&soft, Some(&orig), None);
        let (other, parts) = score_take(&moved, Some(&orig), None);
        assert!(same > blurred, "{same} vs {blurred}");
        assert!(same > other, "{same} vs {other}");
        assert!(parts["composition_kept"].as_f64().unwrap() < 0.9);
        let warm = look::stats(&[&Frame::filled(8, 8, [220, 150, 80, 255])]);
        let (a, _) = score_take(&Frame::filled(16, 16, [215, 150, 85, 255]), None, Some(&warm));
        let (b, _) = score_take(&Frame::filled(16, 16, [60, 120, 220, 255]), None, Some(&warm));
        assert!(a > b);
        let mut r = Request::new(Task::Relight, std::env::temp_dir());
        assert_eq!(best_of(&r), 1);
        r.settings = json!({"best_of": 9});
        assert_eq!(best_of(&r), 6);
    }
}
