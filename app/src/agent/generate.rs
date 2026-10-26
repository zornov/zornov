// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `generate`: pictures made by Codex's image generation (the person's own
//! ChatGPT plan, through the `codex` command, no API key), saved in the
//! project's `Generated` folder and added to the media bin like a
//! download, optionally straight onto the timeline.
//!
//! Pictures to work from are files, media in the project, or the program
//! frame at a moment (`{"frame": 3.5}`): restyle a still, keep a product
//! the same in a new shot. Video, music and sound from words are not
//! available yet; the answers say what to do in Studio instead.

use super::Session;
use super::picture_batch::{self, Batch, Maker, PictureReq};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Biggest picture sent along as a reference.
const MAX_PICTURE_BYTES: u64 = 20 << 20;
/// Longest a batch is waited for.
const WAIT: Duration = Duration::from_secs(20 * 60);

#[cfg(test)]
thread_local! {
    /// Tests make batch pictures with a fake instead of Codex.
    pub(super) static TEST_MAKER: std::cell::RefCell<Option<Maker>> = const { std::cell::RefCell::new(None) };
}

/// What `generate` can be asked to make.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Picture,
    Video,
    /// A score or a song.
    Music,
    /// A sound effect.
    Sound,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Picture => "picture",
            Kind::Video => "video",
            Kind::Music => "music",
            Kind::Sound => "sound",
        }
    }

    /// Reads the `make` argument.
    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "picture" | "image" | "still" | "photo" => Kind::Picture,
            "video" | "clip" | "movie" => Kind::Video,
            "music" | "song" | "score" => Kind::Music,
            "sound" | "sfx" | "sound_effect" | "effect" => Kind::Sound,
            _ => return None,
        })
    }

    /// Why this cannot be made from words yet, and what to do instead.
    pub fn unavailable(self) -> Option<&'static str> {
        match self {
            Kind::Picture => None,
            Kind::Video => Some("video from words is not available yet. Make it move in Zornov instead: generate the pictures (make: picture; the subject with transparent: true, the background separately), put them in 3D (three_d, parallax) with light and a camera move, and keyframe them over time. guide topic \"generated\" has the recipe"),
            Kind::Music => Some("music is made in Zornov by the edit op music, not by generate: it composes original music in a genre (genre or describe, instruments, bpm, key, duration, cues for the moments to land on, tail), or takes a library track for a mood. skill music-beat-sync part Scoring"),
            Kind::Sound => Some("sound is made in Zornov by the edit op sfx, not by generate: foley and effects from a description (footsteps on gravel, a match strike, distant traffic, a pig oinking), made to measure, or catalogue panel audio for library sounds; the edit op sing sings notes and syllables. skill sound-design part Foley"),
        }
    }
}

/// A picture to make.
#[derive(Clone, Debug, Default)]
pub struct Ask {
    pub prompt: String,
    /// Pictures to follow: https links or `data:image/...` URIs.
    pub images: Vec<String>,
    /// "16:9", "9:16", "1:1", "4:3" or "3:4".
    pub aspect: Option<String>,
    pub count: Option<u32>,
    /// What must not be in it.
    pub avoid: Option<String>,
}

/// Pictures from Codex's own image generation: the person's ChatGPT plan
/// through the `codex` command, with no API key. `refs` are pictures to
/// follow (a character to keep, a style); `dir` receives the files.
fn codex_pictures(ask: &Ask, count: u32, transparent: bool, refs: &[PathBuf], dir: &Path, name: Option<&str>) -> Result<Vec<PathBuf>, String> {
    let task = picture_task(&ask.prompt, ask.aspect.as_deref(), transparent, ask.avoid.as_deref(), !refs.is_empty(), count);
    codex_run(&task, count, refs, dir, &file_name(name, &ask.prompt))
}

/// The instruction for one picture.
fn picture_task(prompt: &str, aspect: Option<&str>, transparent: bool, avoid: Option<&str>, has_refs: bool, count: u32) -> String {
    let shape = match aspect {
        Some("9:16") => "vertical 9:16",
        Some("1:1") => "square 1:1",
        Some("4:3") => "4:3",
        Some("3:4") => "vertical 3:4",
        _ => "landscape 16:9",
    };
    let mut task = format!("Use your image generation tool to make {count} image{} for a video edit. Picture: {}. Frame: {shape}.", if count == 1 { "" } else { "s" }, prompt.trim());
    if transparent {
        task.push_str(" Make the subject alone on a fully transparent background (PNG with alpha), nothing else in the frame.");
    }
    if let Some(av) = avoid.filter(|a| !a.trim().is_empty()) {
        task.push_str(&format!(" Avoid: {av}."));
    }
    if has_refs {
        task.push_str(" Follow the attached picture(s): keep the same subject, look and style.");
    }
    task
}

/// A safe file stem: the given name, else one from the prompt.
fn file_name(name: Option<&str>, prompt: &str) -> String {
    name.map(|n| n.chars().filter(|c| !matches!(c, '/' | '\\' | ':' | '\0')).collect::<String>().trim_start_matches('.').to_owned()).filter(|n| !n.is_empty()).unwrap_or_else(|| stem(prompt))
}

/// Saving names a file after checking it is free: one at a time.
static SAVE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// What a `codex exec --json` run said.
#[derive(Debug, Default, PartialEq)]
pub(super) struct CodexRun {
    /// Codex's thread: its pictures are kept under `generated_images/<thread>`.
    pub thread: Option<String>,
    /// Errors it reported (a refusal, a limit, a failed turn).
    pub errors: Vec<String>,
    /// The last thing it said.
    pub said: Option<String>,
}

/// Reads the event lines of a run.
pub(super) fn parse_codex_events(log: &str) -> CodexRun {
    let mut run = CodexRun::default();
    for v in log.lines().filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok()) {
        let kind = v["type"].as_str().unwrap_or("");
        if kind == "thread.started" {
            run.thread = v["thread_id"].as_str().map(str::to_owned);
        }
        let mut error = |m: Option<&str>| {
            if let Some(m) = m.map(str::trim).filter(|m| !m.is_empty() && !m.contains("is deprecated")) {
                run.errors.push(m.to_owned());
            }
        };
        match kind {
            "error" | "turn.failed" => error(v["message"].as_str().or_else(|| v["error"]["message"].as_str())),
            "item.completed" if v["item"]["type"] == "error" => error(v["item"]["message"].as_str()),
            "item.completed" if v["item"]["type"] == "agent_message" => run.said = v["item"]["text"].as_str().map(str::to_owned),
            _ => {}
        }
    }
    run
}

/// Codex's own folder (`CODEX_HOME`, else `~/.codex`).
fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME").map(PathBuf::from).filter(|p| p.is_dir()).unwrap_or_else(|| std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".codex"))
}

fn is_picture_file(p: &Path) -> bool {
    matches!(p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref(), Some("png" | "jpg" | "jpeg" | "webp"))
}

/// Why a run made nothing, in the words of the run itself: what Codex
/// reported, what it said, what it wrote on stderr, how it ended.
pub(super) fn no_picture_reason(run: &CodexRun, stderr: &str, exit: &str, seconds: u64) -> String {
    let stderr_line = stderr.lines().rev().map(str::trim).find(|l| !l.is_empty() && !l.contains("rmcp::") && !l.contains("is deprecated")).unwrap_or("");
    let why = run.errors.last().map(String::as_str).or(run.said.as_deref().map(str::trim).filter(|s| !s.is_empty())).or(Some(stderr_line).filter(|s| !s.is_empty()));
    match why {
        Some(w) => format!("Codex made no picture: {}", w.chars().take(300).collect::<String>()),
        None => format!("Codex made no picture (it ended {exit} after {seconds} s and said nothing)"),
    }
}

static RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Runs Codex's image generation with the instruction `task` (pictures to
/// work from attached as `refs`) and saves up to `count` results in `dir`
/// as `name` (numbered when taken). Runs on any thread, and any number of
/// runs may go at once: each has its own work folder and takes its pictures
/// from its own Codex thread, never from a folder shared with the others.
pub fn codex_run(task: &str, count: u32, refs: &[PathBuf], dir: &Path, name: &str) -> Result<Vec<PathBuf>, String> {
    // The current Codex (an old Homebrew copy made no pictures).
    let codex = super::brain::find_codex().ok_or("generating pictures uses the Codex app's image generation, and the codex command is not on this Mac (install Codex)")?;
    let work = std::env::temp_dir().join(format!("studio-codex-image-{}-{}-{}", std::process::id(), RUNS.fetch_add(1, std::sync::atomic::Ordering::SeqCst), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos())));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut task = task.to_owned();
    // The image tool keeps the picture itself (under its thread's folder);
    // asking for a copy in the work folder is a bonus, never a condition.
    task.push_str(&format!(" Use only the image generation tool, never code, to make {}. When it is made, reply with one short line.", if count == 1 { "the picture" } else { "the pictures" }));
    let mut cmd = std::process::Command::new(codex);
    cmd.current_dir(&work).args(["exec", "--json", "--skip-git-repo-check", "-s", "workspace-write", "-c", "approval_policy=\"never\""]);
    cmd.args(codex_pictures_args(&task, refs));
    let started = std::time::Instant::now();
    // What Codex says on stderr explains a run that made nothing.
    let err_log = work.join("codex-stderr.log");
    let err = std::fs::File::create(&err_log).map(std::process::Stdio::from).unwrap_or_else(|_| std::process::Stdio::null());
    let mut child = super::procs::spawn(cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(err)).map_err(|e| format!("cannot run codex: {e}"))?;
    let out = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(mut o) = out {
            use std::io::Read;
            let _ = o.read_to_string(&mut s);
        }
        s
    });
    // A hard limit: a stuck generation never holds the edit. Starting Codex
    // alone takes 15 s or more, and a picture a minute or two.
    let limit = Duration::from_secs(60 * (4 + 2 * count as u64).min(15));
    let status = loop {
        if let Some(st) = child.try_wait().map_err(|e| e.to_string())? {
            super::procs::release(child.id());
            break st;
        }
        if super::jobs::cancelled() {
            super::procs::kill_tree(child.id());
            let _ = child.wait();
            super::procs::release(child.id());
            let _ = std::fs::remove_dir_all(&work);
            return Err("stopped".into());
        }
        if started.elapsed() > limit {
            super::procs::kill_tree(child.id());
            let _ = child.wait();
            super::procs::release(child.id());
            let _ = std::fs::remove_dir_all(&work);
            return Err(format!("picture generation took longer than {} minutes and was stopped", limit.as_secs() / 60));
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    let log = reader.join().unwrap_or_default();
    let run = parse_codex_events(&log);
    let mut made: Vec<PathBuf> = std::fs::read_dir(&work).map(|d| d.flatten().map(|e| e.path()).filter(|p| is_picture_file(p)).collect()).unwrap_or_default();
    // Codex keeps what it made under <codex home>/generated_images/<thread>.
    if made.is_empty() {
        if let Some(t) = run.thread.as_deref().filter(|t| !t.is_empty() && !t.contains(['/', '\\', '.'])) {
            let mut stack = vec![codex_home().join("generated_images").join(t)];
            while let Some(d) = stack.pop() {
                for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else if is_picture_file(&p) {
                        made.push(p);
                    }
                }
            }
        }
    }
    made.sort();
    if made.is_empty() {
        let stderr = std::fs::read_to_string(&err_log).unwrap_or_default();
        let exit = status.code().map_or_else(|| "from a signal".to_owned(), |c| format!("with exit code {c}"));
        let reason = no_picture_reason(&run, &stderr, &exit, started.elapsed().as_secs());
        let _ = std::fs::remove_dir_all(&work);
        return Err(reason);
    }
    let mut files = Vec::new();
    let _saving = SAVE.lock().unwrap_or_else(|e| e.into_inner());
    for m in made.iter().take(count.max(1) as usize) {
        let ext = m.extension().and_then(|e| e.to_str()).unwrap_or("png").to_ascii_lowercase();
        let dest = free_path(dir, if name.is_empty() { "generated" } else { name }, &ext);
        std::fs::copy(m, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        files.push(dest);
    }
    let _ = std::fs::remove_dir_all(&work);
    Ok(files)
}

/// The arguments after `codex exec …` options: each picture as
/// `--image=<file>` and the instruction after `--`. (`-i` takes several
/// values, so an instruction written after it was read as one more
/// picture and Codex ran with no prompt.)
fn codex_pictures_args(task: &str, refs: &[PathBuf]) -> Vec<std::ffi::OsString> {
    let mut v: Vec<std::ffi::OsString> = refs.iter().map(|r| {
        let mut s = std::ffi::OsString::from("--image=");
        s.push(r);
        s
    }).collect();
    v.push("--".into());
    v.push(task.into());
    v
}

/// A data URI's bytes (`data:<mime>;base64,<data>`).
fn data_uri_bytes(uri: &str) -> Option<Vec<u8>> {
    let b64 = uri.split_once(";base64,")?.1;
    let mut out = Vec::with_capacity(b64.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in b64.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            _ => continue,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// `data:<mime>;base64,<bytes>`.
pub fn data_uri(mime: &str, bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(bytes.len() * 4 / 3 + mime.len() + 16);
    s.push_str("data:");
    s.push_str(mime);
    s.push_str(";base64,");
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        s.push(A[(n >> 18) as usize & 63] as char);
        s.push(A[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { A[n as usize & 63] as char } else { '=' });
    }
    s
}

/// A picture file's MIME type, by its extension.
pub fn picture_mime(p: &Path) -> Option<&'static str> {
    match p.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        _ => None,
    }
}

/// A short, safe file stem from the prompt.
pub fn stem(prompt: &str) -> String {
    let words: Vec<String> = prompt.split_whitespace().take(6).map(|w| w.chars().filter(|c| c.is_alphanumeric() || *c == '-').collect::<String>()).filter(|w| !w.is_empty()).collect();
    let s = words.join(" ");
    if s.is_empty() { "generated".into() } else { s.chars().take(48).collect() }
}

/// `dir/<stem>.<ext>`, numbered when taken.
pub fn free_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    (2..).map(|n| dir.join(format!("{stem} {n}.{ext}"))).find(|p| !p.exists()).expect("a free name")
}

/// Where generated media is saved: `<project>/Generated`, or Studio's own
/// folder for a project not saved yet.
pub fn generated_dir(project: Option<&Path>) -> PathBuf {
    project.map(|h| h.join("Generated")).unwrap_or_else(|| studio_paths::studio().join("Generated"))
}

/// Pictures from words by Codex, saved in `dir` (the editor's Generate card).
pub fn make_pictures(prompt: &str, count: u32, dir: &Path) -> Result<Vec<PathBuf>, String> {
    let ask = Ask { prompt: prompt.to_owned(), ..Default::default() };
    codex_pictures(&ask, count.clamp(1, 4), false, &[], dir, None)
}

/// Files for pictures to follow (data URIs or links), in a folder of their own.
fn write_refs(images: &[String]) -> Result<Vec<PathBuf>, String> {
    let dir = std::env::temp_dir().join(format!("studio-refs-{}-{}", std::process::id(), RUNS.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut refs = Vec::new();
    for (i, uri) in images.iter().enumerate() {
        let path = dir.join(format!("ref-{i}.png"));
        let bytes = match data_uri_bytes(uri) {
            Some(b) => b,
            None => studio_net::get(uri, &[], Duration::from_secs(60))?.bytes(MAX_PICTURE_BYTES).map_err(|e| e.to_string())?,
        };
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
        refs.push(path);
    }
    Ok(refs)
}

impl Session {
    /// A picture for the model: an https link, a file, project media, or
    /// the program frame at `{"frame": seconds}`.
    fn picture_arg(&mut self, v: &Value) -> Result<String, String> {
        if let Some(t) = v.get("frame").and_then(Value::as_f64) {
            #[cfg(feature = "render")]
            {
                let w = self.doc.project.width.clamp(256, 2048);
                let jpeg = self.frames.frame(&self.doc, super::micros(t.max(0.0)), w, 92)?;
                return Ok(data_uri("image/jpeg", &jpeg));
            }
            #[cfg(not(feature = "render"))]
            return Err(format!("this build cannot render the frame at {t} s"));
        }
        let path = match v {
            Value::String(s) if s.starts_with("https://") => return Ok(s.clone()),
            Value::String(s) if s.starts_with("data:image/") => return Ok(s.clone()),
            Value::String(s) if !s.chars().all(|c| c.is_ascii_digit()) => PathBuf::from(s),
            _ => {
                let mi = self.media_item(v).map_err(|e| format!("a picture is a file path, an https link, a media id or {{\"frame\": seconds}} ({e})"))?;
                mi.path.ok_or("that media has no file on disk")?
            }
        };
        let mime = picture_mime(&path).ok_or_else(|| format!("{} is not a PNG, JPEG, WebP or GIF picture", path.display()))?;
        let len = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?.len();
        if len > MAX_PICTURE_BYTES {
            return Err(format!("{} is over 20 MB; send a smaller picture", path.display()));
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(data_uri(mime, &bytes))
    }

    /// `generate {items: [...]}`: every picture asked for is made at the same
    /// time (a few Codex image workers), each lands in the media bin as it is
    /// ready. Returns at once with a job to ask about, or with `wait: true`
    /// when everything is done.
    fn generate_batch(&mut self, a: &Value) -> Result<Value, String> {
        let list = a.get("items").and_then(Value::as_array).filter(|l| !l.is_empty()).ok_or("items is a list of pictures: [{name, prompt, aspect, transparent, image}]")?;
        if list.len() > 60 {
            return Err("a batch is up to 60 pictures; send the rest as a second batch".into());
        }
        let flag = |it: &Value, k: &str| it.get(k).and_then(Value::as_bool).or_else(|| a.get(k).and_then(Value::as_bool)).unwrap_or(false);
        let text = |it: &Value, k: &str| it.get(k).and_then(Value::as_str).or_else(|| a.get(k).and_then(Value::as_str)).map(str::to_owned);
        let mut reqs = Vec::new();
        for (n, it) in list.iter().enumerate() {
            let prompt = text(it, "prompt").filter(|p| !p.trim().is_empty()).ok_or_else(|| format!("items[{n}] needs a prompt"))?;
            let mut images = Vec::new();
            match it.get("images").or_else(|| it.get("image")).or_else(|| a.get("images")).or_else(|| a.get("image")) {
                Some(Value::Array(l)) => {
                    for v in l {
                        images.push(self.picture_arg(v)?);
                    }
                }
                Some(v) => images.push(self.picture_arg(v)?),
                None => {}
            }
            let name = {
                let given = it.get("name").and_then(Value::as_str);
                let base = file_name(given, &prompt);
                // Two pictures with one name get numbers, not each other's file.
                let same = reqs.iter().filter(|r: &&PictureReq| r.name == base || r.name.strip_prefix(&base).is_some_and(|t| t.trim().parse::<u32>().is_ok())).count();
                if same == 0 { base } else { format!("{base} {}", same + 1) }
            };
            reqs.push(PictureReq {
                name,
                prompt,
                aspect: text(it, "aspect"),
                transparent: flag(it, "transparent"),
                avoid: text(it, "avoid"),
                refs: write_refs(&images)?,
                to_timeline: flag(it, "to_timeline"),
            });
        }
        let home = self.home.clone().or_else(|| self.doc.folder.as_ref().map(|f| f.root().to_path_buf()));
        let dir = generated_dir(home.as_deref());
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        #[cfg(test)]
        let test_maker = TEST_MAKER.with(|m| m.borrow().clone());
        #[cfg(not(test))]
        let test_maker: Option<Maker> = None;
        let maker: Maker = match test_maker {
            Some(m) => m,
            None => {
                if super::brain::find_codex().is_none() {
                    return Err("pictures are made by the Codex app's image generation, and the codex command is not on this Mac (install Codex)".into());
                }
                Arc::new(move |r: &PictureReq| {
                    let task = picture_task(&r.prompt, r.aspect.as_deref(), r.transparent, r.avoid.as_deref(), !r.refs.is_empty(), 1);
                    codex_run(&task, 1, &r.refs, &dir, &r.name)
                })
            }
        };
        let workers = picture_batch::workers_for(a.get("workers").and_then(Value::as_u64));
        let batch = Batch::start(reqs, workers, maker);
        let (total, workers) = (batch.total, batch.workers);
        let id = self.next_batch;
        self.next_batch += 1;
        self.batches.push((id, batch));
        if a.get("wait").and_then(Value::as_bool).unwrap_or(false) {
            return self.batch_wait(id);
        }
        Ok(json!({
            "job": id,
            "started": total,
            "workers": workers,
            "note": format!("{total} pictures are being made {workers} at a time and each lands in the media bin when ready. Keep working; generate {{\"job\": {id}}} shows progress, or add wait: true to wait for all of them. Finished pictures are also announced after your next tool calls."),
        }))
    }

    /// `generate {job}`: progress of a batch (`wait`: until it is done,
    /// `cancel`: stop pictures not started).
    fn batch_job(&mut self, a: &Value) -> Result<Value, String> {
        let id = a.get("job").and_then(Value::as_u64).ok_or("job is the number generate returned")? as u32;
        if self.model_jobs.iter().any(|(i, _)| *i == id) {
            return self.model_job_op(a, id);
        }
        if !self.batches.iter().any(|(i, _)| *i == id) {
            return Err(format!("no picture job {id}; running: {}", self.batches.iter().map(|(i, _)| i.to_string()).collect::<Vec<_>>().join(", ")));
        }
        if a.get("cancel").and_then(Value::as_bool).unwrap_or(false) {
            self.batches.iter().find(|(i, _)| *i == id).map(|(_, b)| b.cancel());
        }
        if a.get("wait").and_then(Value::as_bool).unwrap_or(false) {
            return self.batch_wait(id);
        }
        self.batch_view(id)
    }

    fn batch_wait(&mut self, id: u32) -> Result<Value, String> {
        let deadline = std::time::Instant::now() + WAIT;
        loop {
            let v = self.batch_view(id)?;
            if v["finished"] == true || std::time::Instant::now() > deadline {
                return Ok(v);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Lands what is ready, then reports the batch.
    fn batch_view(&mut self, id: u32) -> Result<Value, String> {
        self.land_pictures();
        let b = self.batches.iter().find(|(i, _)| *i == id).map(|(_, b)| b).ok_or_else(|| format!("no picture job {id}"))?;
        let mut v = b.report();
        v["job"] = json!(id);
        // What this report tells need not be told again after the next call.
        let _ = b.news();
        if v["finished"] == true {
            v["note"] = json!("the batch is finished; failed pictures say why (each was already retried once with a reworded request)");
        }
        Ok(v)
    }

    /// Puts every finished picture of every batch in the media bin.
    pub(super) fn land_pictures(&mut self) {
        let work: Vec<(u32, usize, PictureReq, Vec<PathBuf>)> = self.batches.iter().flat_map(|(id, b)| b.to_land().into_iter().map(move |(i, r, f)| (*id, i, r, f))).collect();
        for (id, i, req, files) in work {
            let paths: Vec<String> = files.iter().map(|p| p.display().to_string()).collect();
            let media = self.call_tool("import", &json!({"paths": paths, "to_timeline": req.to_timeline})).ok().and_then(|parts| parts.iter().find_map(|p| if let super::Content::Text(t) = p { serde_json::from_str::<Value>(t).ok() } else { None })).map(|v| v["media"].as_array().cloned().unwrap_or_default());
            if let (Some(media), Some((_, b))) = (media, self.batches.iter().find(|(j, _)| *j == id)) {
                b.landed(i, media);
            } else if let Some((_, b)) = self.batches.iter().find(|(j, _)| *j == id) {
                // Not imported: the file exists, so do not try forever.
                b.landed(i, Vec::new());
            }
        }
    }

    /// News of running batches to add to a tool's reply: pictures that just
    /// landed and ones that failed. None when there is none.
    pub(super) fn pictures_note(&mut self) -> Option<Value> {
        if self.batches.is_empty() {
            return None;
        }
        self.land_pictures();
        let mut ready = Vec::new();
        let mut failed = Vec::new();
        let mut progress = Vec::new();
        for (id, b) in &self.batches {
            let (r, f) = b.news();
            ready.extend(r.into_iter().map(|mut v| {
                v["job"] = json!(id);
                v
            }));
            failed.extend(f.into_iter().map(|mut v| {
                v["job"] = json!(id);
                v
            }));
            if !b.finished() {
                let p = &b.report()["progress"];
                progress.push(json!({"job": id, "ready": p["ready"], "failed": p["failed"], "total": p["total"]}));
            }
        }
        // A finished batch whose news is told is forgotten.
        self.batches.retain(|(_, b)| !b.finished() || !b.to_land().is_empty());
        if ready.is_empty() && failed.is_empty() {
            return None;
        }
        Some(json!({"pictures_ready": ready, "pictures_failed": failed, "still_making": progress}))
    }

    /// The `generate` tool.
    pub(super) fn generate(&mut self, a: &Value) -> Result<Value, String> {
        // A 3D model from pictures (or a raw model file), made on this Mac.
        if a.get("task").and_then(Value::as_str) == Some("model3d") {
            return self.model3d(a);
        }
        // Many pictures at once, and a batch already running.
        if a.get("items").is_some() {
            return self.generate_batch(a);
        }
        if a.get("job").is_some() {
            return self.batch_job(a);
        }
        // Tasks, looks, recipes (the generative media department).
        #[cfg(feature = "render")]
        if let Some(v) = self.generative(a)? {
            return Ok(v);
        }
        if a.get("list").and_then(Value::as_bool).unwrap_or(false) {
            return Ok(json!({
                "makes": {
                    "picture": {"available": true, "by": "Codex image generation", "needs": "the codex command on this Mac"},
                    "video": {"available": false, "note": Kind::Video.unavailable()},
                    "music": {"available": true, "by": "Zornov's own composer", "how": Kind::Music.unavailable()},
                    "sound": {"available": true, "by": "Zornov's own sound models", "how": Kind::Sound.unavailable()},
                },
                "codex": super::brain::find_codex().is_some() || cfg!(test),
            }));
        }
        let kind = match a.get("make").and_then(Value::as_str) {
            Some(m) => Kind::parse(m).ok_or_else(|| format!("make is \"picture\", \"video\", \"music\" or \"sound\", not \"{m}\""))?,
            None => return Err("generate needs \"make\": \"picture\", \"video\", \"music\" or \"sound\" (generate {\"list\": true} says what is available)".into()),
        };
        if let Some(why) = kind.unavailable() {
            return Err(why.into());
        }
        let mut images = Vec::new();
        match a.get("images").or_else(|| a.get("image")) {
            Some(Value::Array(list)) => {
                for v in list {
                    images.push(self.picture_arg(v)?);
                }
            }
            Some(v) => images.push(self.picture_arg(v)?),
            None => {}
        }
        let s = |k: &str| a.get(k).and_then(Value::as_str).map(str::to_owned);
        let ask = Ask { prompt: s("prompt").unwrap_or_default(), images, aspect: s("aspect"), count: a.get("count").and_then(Value::as_u64).map(|c| c as u32), avoid: s("avoid") };
        if a.get("estimate").and_then(Value::as_bool).unwrap_or(false) {
            return Ok(json!({"estimate": {"makes": "picture", "by": "Codex image generation"}, "sent": false}));
        }
        // Pictures come from Codex's own image generation on the person's
        // plan, so the edit never stops at a key.
        let home = self.home.clone().or_else(|| self.doc.folder.as_ref().map(|f| f.root().to_path_buf()));
        let dir = generated_dir(home.as_deref());
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let refs_dir = std::env::temp_dir().join(format!("studio-refs-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&refs_dir);
        let mut refs = Vec::new();
        for (i, uri) in ask.images.iter().enumerate() {
            let path = refs_dir.join(format!("ref-{i}.png"));
            let bytes = match data_uri_bytes(uri) {
                Some(b) => b,
                None => {
                    let reply = studio_net::get(uri, &[], Duration::from_secs(60))?;
                    reply.bytes(MAX_PICTURE_BYTES).map_err(|e| e.to_string())?
                }
            };
            std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
            refs.push(path);
        }
        let transparent = a.get("transparent").and_then(Value::as_bool).unwrap_or(false);
        let count = ask.count.unwrap_or(1).clamp(1, 4);
        let files = codex_pictures(&ask, count, transparent, &refs, &dir, a.get("name").and_then(Value::as_str))?;
        let to_timeline = a.get("to_timeline").and_then(Value::as_bool).unwrap_or(false);
        let parts = self.call("import", &json!({"paths": files.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(), "to_timeline": to_timeline}))?;
        let mut v: Value = parts.iter().find_map(|p| if let super::Content::Text(t) = p { serde_json::from_str(t).ok() } else { None }).unwrap_or(json!({}));
        // The production office keeps the books: which shot this was for.
        let made: Vec<String> = files.iter().map(|p| p.display().to_string()).collect();
        let spend = studio_production::Spend {
            provider: "codex".into(),
            model: "image-generation".into(),
            kind: "picture".into(),
            prompt: ask.prompt.chars().take(160).collect(),
            scene: a.get("scene").and_then(Value::as_str).map(str::to_owned),
            shot: a.get("shot").and_then(Value::as_str).map(str::to_owned),
            files: made.clone(),
            ..Default::default()
        };
        self.production_record_generation(a, spend, &made);
        v["generated"] = json!({"by": "Codex image generation", "count": files.len(), "transparent": transparent});
        v["files"] = json!(files.iter().map(|p| p.display().to_string()).collect::<Vec<_>>());
        v["prompt"] = json!(ask.prompt);
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_says_not_available_yet_and_music_and_sound_point_to_the_ops_that_make_them() {
        let mut s = Session::default();
        let e = s.generate(&json!({"make": "video", "prompt": "x"})).unwrap_err();
        assert!(e.contains("not available yet") && e.contains("three_d"), "{e}");
        for (make, hint) in [("music", "edit op music"), ("sound", "edit op sfx")] {
            let e = s.generate(&json!({"make": make, "prompt": "x"})).unwrap_err();
            assert!(e.contains("made in Zornov") && e.contains(hint) && !e.contains("not available"), "{e}");
        }
        assert!(s.generate(&json!({"prompt": "x"})).unwrap_err().contains("\"make\""));
        assert!(s.generate(&json!({"make": "hologram", "prompt": "x"})).unwrap_err().contains("not \"hologram\""));
        let list = s.generate(&json!({"list": true})).unwrap();
        assert_eq!(list["makes"]["picture"]["available"], true);
        assert_eq!(list["makes"]["video"]["available"], false);
        assert_eq!(list["makes"]["music"]["available"], true);
        let est = s.generate(&json!({"make": "picture", "prompt": "x", "estimate": true})).unwrap();
        assert_eq!(est["sent"], false);
    }

    #[test]
    fn names_and_data_uris() {
        assert_eq!(stem("  a red kite, over the sea at dawn  "), "a red kite over the sea");
        assert_eq!(stem("!!!"), "generated");
        let uri = data_uri("image/png", &[0, 1, 2, 3, 254, 255]);
        assert!(uri.starts_with("data:image/png;base64,"));
        assert_eq!(data_uri_bytes(&uri).unwrap(), [0, 1, 2, 3, 254, 255]);
        assert_eq!(Kind::parse("Clip"), Some(Kind::Video));
        assert_eq!(Kind::parse("sfx"), Some(Kind::Sound));
        assert_eq!(Kind::parse("sora"), None);
    }
}

#[cfg(test)]
mod codex_args_tests {
    use super::*;

    /// The instruction never follows `-i` (which takes several files): it
    /// comes after `--`, each picture as `--image=`.
    #[test]
    fn the_prompt_is_not_read_as_a_picture() {
        let a = codex_pictures_args("restyle it", &[PathBuf::from("/tmp/a.jpg"), PathBuf::from("/tmp/b c.png")]);
        let a: Vec<String> = a.iter().map(|s| s.to_string_lossy().into_owned()).collect();
        assert_eq!(a, ["--image=/tmp/a.jpg", "--image=/tmp/b c.png", "--", "restyle it"]);
        assert_eq!(codex_pictures_args("x", &[]).len(), 2);
    }

    /// A fake picture maker that writes a real PNG in `dir`, fails the
    /// pictures named in `bad`, and keeps the prompts it was given.
    fn fake_maker(dir: PathBuf, bad: &'static [&'static str], seen: Arc<std::sync::Mutex<Vec<String>>>) -> Maker {
        Arc::new(move |r: &PictureReq| {
            seen.lock().unwrap().push(r.prompt.clone());
            std::thread::sleep(Duration::from_millis(40));
            if bad.contains(&r.name.as_str()) {
                return Err("Codex made no picture: the image tool rejected the request".into());
            }
            let p = dir.join(format!("{}.png", r.name));
            let mut out = Vec::new();
            let mut e = png::Encoder::new(&mut out, 32, 18);
            e.set_color(png::ColorType::Rgba);
            e.set_depth(png::BitDepth::Eight);
            e.write_header().unwrap().write_image_data(&vec![200; 32 * 18 * 4]).unwrap();
            std::fs::write(&p, out).unwrap();
            Ok(vec![p])
        })
    }

    #[test]
    fn a_batch_lands_pictures_in_the_bin_reports_failures_per_item_and_announces_news() {
        let dir = std::env::temp_dir().join(format!("studio-batch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        TEST_MAKER.with(|m| *m.borrow_mut() = Some(fake_maker(dir.clone(), &["Cave"], seen.clone())));
        let mut s = Session::default();
        s.home = Some(dir.clone());
        let started = s.generate(&json!({"items": [
            {"name": "Harbour", "prompt": "a harbour at dawn"},
            {"name": "Cave", "prompt": "a cave"},
            {"name": "Captain", "prompt": "a captain", "transparent": true},
        ], "workers": 3})).unwrap();
        assert_eq!(started["started"], 3);
        assert_eq!(started["workers"], 3);
        let job = started["job"].as_u64().unwrap();
        // The AI keeps working: any other tool call announces what landed.
        let t = std::time::Instant::now();
        let mut announced = Vec::new();
        while announced.is_empty() && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
            if let Some(note) = s.pictures_note() {
                announced.push(note);
            }
        }
        assert!(!announced.is_empty(), "nothing was announced");
        let v = s.generate(&json!({"job": job, "wait": true})).unwrap();
        assert_eq!(v["finished"], true, "{v}");
        assert_eq!(v["progress"]["ready"], 2, "{v}");
        assert_eq!(v["progress"]["failed"], 1, "{v}");
        let pics = v["pictures"].as_array().unwrap();
        assert_eq!(pics[0]["status"], "ready");
        assert_eq!(pics[0]["media"].as_array().unwrap().len(), 1, "in the bin: {v}");
        assert_eq!(pics[1]["status"], "failed");
        assert_eq!(pics[1]["attempts"], 2);
        assert!(pics[1]["error"].as_str().unwrap().contains("rejected the request"), "{v}");
        assert_eq!(s.doc.project.media.len(), 2, "two pictures are in the bin");
        // Five calls to the maker: three first tries and the Cave's reworded retry... twice for Cave.
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 4);
        assert!(seen.iter().any(|p| p.contains("own words") && p.contains("a cave")));
        TEST_MAKER.with(|m| *m.borrow_mut() = None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn same_names_in_a_batch_get_numbers_and_a_bad_batch_is_refused() {
        let dir = std::env::temp_dir().join(format!("studio-batch-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TEST_MAKER.with(|m| *m.borrow_mut() = Some(fake_maker(dir.clone(), &[], Arc::default())));
        let mut s = Session::default();
        s.home = Some(dir.clone());
        let v = s.generate(&json!({"items": [{"name": "Sky", "prompt": "a"}, {"name": "Sky", "prompt": "b"}], "wait": true})).unwrap();
        let names: Vec<&str> = v["pictures"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["Sky", "Sky 2"]);
        assert!(s.generate(&json!({"items": [{"name": "x"}]})).unwrap_err().contains("needs a prompt"));
        assert!(s.generate(&json!({"items": []})).is_err());
        assert!(s.generate(&json!({"job": 99})).unwrap_err().contains("no picture job"));
        TEST_MAKER.with(|m| *m.borrow_mut() = None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn codex_events_give_the_thread_and_the_real_reason() {
        let log = r#"{"type":"thread.started","thread_id":"01abc"}
{"type":"item.completed","item":{"id":"i0","type":"error","message":"`[features].memory_tool` is deprecated. Use memories"}}
{"type":"turn.started"}
{"type":"item.completed","item":{"id":"i1","type":"agent_message","text":"I could not save a file."}}
{"type":"turn.failed","error":{"message":"You've hit your usage limit"}}"#;
        let r = parse_codex_events(log);
        assert_eq!(r.thread.as_deref(), Some("01abc"));
        assert_eq!(r.errors, ["You've hit your usage limit"], "the deprecation notice is not a reason");
        assert_eq!(no_picture_reason(&r, "", "with exit code 1", 3), "Codex made no picture: You've hit your usage limit");
        let quiet = parse_codex_events("{\"type\":\"thread.started\",\"thread_id\":\"t\"}");
        assert!(no_picture_reason(&quiet, "ERROR rmcp::transport: x\n", "from a signal", 0).contains("ended from a signal after 0 s"));
        let said = CodexRun { said: Some("No file was saved because the tool rejected the request.".into()), ..Default::default() };
        assert!(no_picture_reason(&said, "", "with exit code 0", 9).ends_with("rejected the request."));
    }
}
