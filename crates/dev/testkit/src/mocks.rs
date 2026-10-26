// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Test doubles: a decoder that stalls or fails on command, an audio output
//! that records what was queued and counts underruns, a stand-in neural
//! network, a deterministic clock and a file system for atomic-write and
//! crash tests.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use studio_render::colour_space::ColourDesc;
use studio_render::scene::{MediaKey, Micros, SECOND};
use studio_render::source::{MediaSource, ProceduralSource, SourceInfo, SourceRegistry, SourceSpec};
use studio_render::yuv::{DecodedPicture, PictureRef};

// ------------------------------------------------------------ MockDecoder

/// Shared, adjustable behaviour of a [`MockDecoder`]. Cloning shares state.
#[derive(Clone, Default)]
pub struct DecoderControl {
    inner: Arc<DecoderShared>,
}

#[derive(Default)]
struct DecoderShared {
    /// Sleep this long (µs) for every decoded frame.
    stall_us: AtomicU64,
    /// Frames at or past this index fail to decode (u64::MAX = never).
    fail_from: AtomicU64,
    /// Every frame index requested, in order.
    calls: Mutex<Vec<u64>>,
    /// Frames actually produced.
    produced: AtomicU64,
}

impl DecoderControl {
    /// Makes every subsequent decode take at least `d`.
    pub fn stall(&self, d: Duration) {
        self.inner.stall_us.store(d.as_micros() as u64, Ordering::Relaxed);
    }
    /// Decodes of frame `index` and later fail (return no picture).
    pub fn fail_from(&self, index: u64) {
        self.inner.fail_from.store(index, Ordering::Relaxed);
    }
    /// Decoding works again.
    pub fn recover(&self) {
        self.inner.fail_from.store(u64::MAX, Ordering::Relaxed);
    }
    /// Every frame index asked for so far.
    pub fn calls(&self) -> Vec<u64> {
        self.inner.calls.lock().unwrap().clone()
    }
    /// Number of pictures produced so far.
    pub fn produced(&self) -> u64 {
        self.inner.produced.load(Ordering::Relaxed)
    }
    fn record(&self, index: u64) -> bool {
        self.inner.calls.lock().unwrap().push(index);
        let stall = self.inner.stall_us.load(Ordering::Relaxed);
        if stall > 0 {
            std::thread::sleep(Duration::from_micros(stall));
        }
        if index >= self.inner.fail_from.load(Ordering::Relaxed) {
            return false;
        }
        self.inner.produced.fetch_add(1, Ordering::Relaxed);
        true
    }
}

/// A deterministic video source for the `render` crate. Every picture
/// encodes its own frame index in its colour, so a test can check which
/// frame ended up on screen: see [`MockDecoder::expected_rgb`].
pub struct MockDecoder {
    pub width: u32,
    pub height: u32,
    pub frame_rate: f64,
    pub frames: u64,
    pub control: DecoderControl,
}

impl MockDecoder {
    pub fn new(width: u32, height: u32, frame_rate: f64, frames: u64) -> MockDecoder {
        let control = DecoderControl::default();
        control.recover();
        MockDecoder { width, height, frame_rate, frames, control }
    }

    /// The flat colour (sRGB 8-bit) of frame `index`.
    pub fn expected_rgb(index: u64) -> [u8; 3] {
        [(index & 0xff) as u8, ((index >> 8) & 0xff) as u8, 0x80]
    }

    /// Recovers the frame index from a decoded pixel.
    pub fn index_of(rgb: [u8; 3]) -> u64 {
        rgb[0] as u64 | (rgb[1] as u64) << 8
    }

    /// The picture of frame `index` as the decoder returns it.
    pub fn picture(width: u32, height: u32, index: u64) -> DecodedPicture {
        let c = Self::expected_rgb(index);
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..width * height {
            rgba.extend_from_slice(&[c[0], c[1], c[2], 255]);
        }
        DecodedPicture::from_rgba8(width, height, rgba)
    }

    fn make_info(&self) -> SourceInfo {
        SourceInfo {
            width: self.width,
            height: self.height,
            rotation: 0,
            frame_rate: Some(self.frame_rate),
            duration: Some((self.frames as f64 / self.frame_rate * SECOND as f64).round() as Micros),
            frame_count: Some(self.frames),
            colour: ColourDesc::SRGB8,
        }
    }

    /// A `SourceSpec` for the render crate's registry that behaves like this
    /// decoder (same control handle).
    pub fn spec(&self) -> SourceSpec {
        let (w, h, control, frames) = (self.width, self.height, self.control.clone(), self.frames);
        SourceSpec::Procedural(ProceduralSource::new(w, h, self.frame_rate, frames, move |i| {
            // A failing frame still has to return a picture here; use a
            // magenta frame nobody asks for, so a wrong frame is visible.
            if control.record(i) { Self::picture(w, h, i) } else { DecodedPicture::from_rgba8(w, h, vec![255, 0, 255, 255].repeat((w * h) as usize)) }
        }))
    }

    /// Registers this decoder under `key` and returns the control handle.
    pub fn register(&self, registry: &SourceRegistry, key: MediaKey) -> DecoderControl {
        registry.register(key, self.spec());
        self.control.clone()
    }
}

impl MediaSource for MockDecoder {
    fn info(&self) -> SourceInfo {
        self.make_info()
    }

    fn frame_index(&self, time: Micros) -> u64 {
        ((time.max(0) as f64 / SECOND as f64) * self.frame_rate).floor() as u64
    }

    fn decode_to(&mut self, index: u64, emit: &mut dyn FnMut(u64, PictureRef)) -> bool {
        if index >= self.frames {
            return false;
        }
        if !self.control.record(index) {
            return false;
        }
        emit(index, Arc::new(Self::picture(self.width, self.height, index)));
        true
    }
}

// ----------------------------------------------------------- MockAudioOut

/// A stand-in for the speaker: the app pushes interleaved stereo samples,
/// the "device" pulls fixed blocks; a pull that finds the queue short is
/// an underrun and is padded with silence (which is what the real output
/// does, so a test hears what the user would).
#[derive(Clone, Default)]
pub struct MockAudioOut {
    inner: Arc<Mutex<AudioOutState>>,
}

#[derive(Default)]
struct AudioOutState {
    queue: VecDeque<f32>,
    pushed: Vec<f32>,
    played: Vec<f32>,
    underruns: u64,
    underrun_frames: u64,
    played_frames: u64,
    volume: f32,
}

impl MockAudioOut {
    pub fn new() -> MockAudioOut {
        let m = MockAudioOut::default();
        m.inner.lock().unwrap().volume = 1.0;
        m
    }

    /// Queues interleaved stereo samples (what `studio_audio::AudioOut::push` takes).
    pub fn push(&self, samples: &[f32]) {
        let mut s = self.inner.lock().unwrap();
        s.queue.extend(samples.iter().copied());
        s.pushed.extend_from_slice(samples);
    }

    /// Simulates the device callback asking for `frames` stereo frames.
    /// Returns the block as played (silence where the queue ran dry).
    pub fn pull(&self, frames: usize) -> Vec<f32> {
        let mut s = self.inner.lock().unwrap();
        let want = frames * 2;
        let have = s.queue.len().min(want);
        let mut block: Vec<f32> = s.queue.drain(..have).collect();
        if have < want {
            s.underruns += 1;
            s.underrun_frames += ((want - have) / 2) as u64;
            block.resize(want, 0.0);
        }
        s.played_frames += frames as u64;
        s.played.extend_from_slice(&block);
        block
    }

    pub fn clear(&self) {
        self.inner.lock().unwrap().queue.clear();
    }
    pub fn set_volume(&self, v: f32) {
        self.inner.lock().unwrap().volume = v;
    }
    pub fn volume(&self) -> f32 {
        self.inner.lock().unwrap().volume
    }
    /// Stereo frames waiting to be played.
    pub fn queued_frames(&self) -> usize {
        self.inner.lock().unwrap().queue.len() / 2
    }
    pub fn played_frames(&self) -> u64 {
        self.inner.lock().unwrap().played_frames
    }
    /// Device pulls that found too little audio.
    pub fn underruns(&self) -> u64 {
        self.inner.lock().unwrap().underruns
    }
    /// Frames of silence inserted by underruns.
    pub fn underrun_frames(&self) -> u64 {
        self.inner.lock().unwrap().underrun_frames
    }
    /// Everything ever pushed, in order.
    pub fn pushed(&self) -> Vec<f32> {
        self.inner.lock().unwrap().pushed.clone()
    }
    /// Everything the device played, in order (including underrun silence).
    pub fn played(&self) -> Vec<f32> {
        self.inner.lock().unwrap().played.clone()
    }
}

// ---------------------------------------------------------------- MockNn

/// Canned inference. Keyed by a model id (the `ai` crate's
/// `ModelId::key()` strings, or any name); each model returns fixed output
/// tensors, or is "missing".
#[derive(Clone, Default)]
pub struct MockNn {
    inner: Arc<Mutex<MockNnState>>,
}

#[derive(Default)]
struct MockNnState {
    models: BTreeMap<String, Vec<studio_nn::Tensor>>,
    missing: Vec<String>,
    calls: Vec<String>,
    latency: Duration,
}

/// Why a mock run failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MockNnError {
    /// The model is not installed.
    Missing(String),
    /// No canned output is registered.
    Unknown(String),
}

impl std::fmt::Display for MockNnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MockNnError::Missing(m) => write!(f, "model {m} is not installed"),
            MockNnError::Unknown(m) => write!(f, "no canned output for model {m}"),
        }
    }
}

impl std::error::Error for MockNnError {}

impl MockNn {
    pub fn new() -> MockNn {
        MockNn::default()
    }

    /// Registers canned outputs for `model`.
    pub fn canned(&self, model: &str, outputs: Vec<studio_nn::Tensor>) {
        let mut s = self.inner.lock().unwrap();
        s.models.insert(model.to_string(), outputs);
        s.missing.retain(|m| m != model);
    }

    /// Marks `model` as not installed.
    pub fn missing(&self, model: &str) {
        let mut s = self.inner.lock().unwrap();
        s.models.remove(model);
        s.missing.push(model.to_string());
    }

    /// Every run sleeps this long (to test cancellation and progress).
    pub fn set_latency(&self, d: Duration) {
        self.inner.lock().unwrap().latency = d;
    }

    /// Runs `model` on `inputs` (ignored) and returns the canned outputs.
    pub fn run(&self, model: &str, _inputs: &[studio_nn::Tensor]) -> Result<Vec<studio_nn::Tensor>, MockNnError> {
        let (latency, result) = {
            let mut s = self.inner.lock().unwrap();
            s.calls.push(model.to_string());
            let r = if s.missing.iter().any(|m| m == model) {
                Err(MockNnError::Missing(model.to_string()))
            } else {
                s.models.get(model).cloned().ok_or_else(|| MockNnError::Unknown(model.to_string()))
            };
            (s.latency, r)
        };
        if !latency.is_zero() {
            std::thread::sleep(latency);
        }
        result
    }

    /// Models run so far, in order.
    pub fn calls(&self) -> Vec<String> {
        self.inner.lock().unwrap().calls.clone()
    }

    /// An ONNX model (bytes) with one float input `x` of `shape` whose
    /// outputs are the given constants, regardless of the input. Loadable
    /// by `studio_nn::Model::from_bytes`, so real code paths can be driven
    /// with known answers.
    pub fn constant_model(input_shape: &[i64], outputs: &[(&str, studio_nn::Tensor)]) -> Vec<u8> {
        use studio_nn::DType;
        use studio_nn::onnx::{GraphBuilder, encode_model};
        let mut b = GraphBuilder::new(13);
        b.input("x", DType::F32, input_shape);
        for (name, t) in outputs {
            let init = format!("{name}_const");
            b.init(&init, t.clone());
            b.node("Identity", &[&init], &[name], vec![]);
            b.output(name, DType::F32);
        }
        encode_model(&b.build(), None).expect("encode constant model")
    }

    /// Creates the files of a model in a `ModelStore` root so its status
    /// reads as installed, without downloading anything: every file gets
    /// the registered size (sparse, so it costs no disk) and is otherwise
    /// empty. Loading such a model must fail cleanly, which is what the
    /// "degrades gracefully" tests check.
    pub fn fake_install(store_root: &Path, dir: &str, files: &[(&str, u64)]) -> std::io::Result<()> {
        let d = store_root.join(dir);
        std::fs::create_dir_all(&d)?;
        for (name, size) in files {
            let p = d.join(name);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let f = std::fs::File::create(&p)?;
            f.set_len(*size)?;
        }
        Ok(())
    }
}

// ------------------------------------------------------------- MockClock

/// Deterministic time. `now()` is a real `Instant` offset by a controlled
/// amount, so code that takes `Instant`s (autosave, playback clocks) can be
/// driven exactly; `system_time()` does the same for wall-clock stamps.
#[derive(Clone)]
pub struct MockClock {
    base: Instant,
    base_sys: SystemTime,
    offset_us: Arc<AtomicU64>,
    frozen: Arc<AtomicBool>,
}

impl Default for MockClock {
    fn default() -> Self {
        Self::new()
    }
}

impl MockClock {
    pub fn new() -> MockClock {
        MockClock { base: Instant::now(), base_sys: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000), offset_us: Arc::new(AtomicU64::new(0)), frozen: Arc::new(AtomicBool::new(true)) }
    }

    /// The current mock time.
    pub fn now(&self) -> Instant {
        self.base + Duration::from_micros(self.offset_us.load(Ordering::Relaxed))
    }

    /// The current mock wall-clock time.
    pub fn system_time(&self) -> SystemTime {
        self.base_sys + Duration::from_micros(self.offset_us.load(Ordering::Relaxed))
    }

    /// Time since the clock was created.
    pub fn elapsed(&self) -> Duration {
        Duration::from_micros(self.offset_us.load(Ordering::Relaxed))
    }

    /// Moves time forward.
    pub fn advance(&self, d: Duration) {
        self.offset_us.fetch_add(d.as_micros() as u64, Ordering::Relaxed);
    }

    /// Moves time forward by whole microseconds.
    pub fn advance_us(&self, us: i64) {
        self.advance(Duration::from_micros(us.max(0) as u64));
    }

    /// Sets the elapsed time directly.
    pub fn set(&self, since_start: Duration) {
        self.offset_us.store(since_start.as_micros() as u64, Ordering::Relaxed);
    }

    /// Whether the clock only moves when told (always true for the mock;
    /// kept so code can ask).
    pub fn is_frozen(&self) -> bool {
        self.frozen.load(Ordering::Relaxed)
    }
}

// --------------------------------------------------------------- MockFs

/// A scratch folder under `target-qa/tmp` with helpers for atomic-write
/// tests. The folder is removed when the mock is dropped.
pub struct MockFs {
    root: PathBuf,
    keep: bool,
}

impl MockFs {
    /// A new empty folder named after `label`.
    pub fn new(label: &str) -> MockFs {
        MockFs { root: crate::fixtures::scratch_dir(label), keep: false }
    }

    /// Keeps the folder after the test (to inspect a failure).
    pub fn keep(mut self) -> MockFs {
        self.keep = true;
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Absolute path of a file inside the folder.
    pub fn path(&self, rel: impl AsRef<Path>) -> PathBuf {
        self.root.join(rel)
    }

    /// Writes a file (creating parents).
    pub fn write(&self, rel: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> PathBuf {
        let p = self.path(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, bytes).unwrap();
        p
    }

    pub fn read(&self, rel: impl AsRef<Path>) -> Vec<u8> {
        std::fs::read(self.path(rel)).unwrap()
    }

    pub fn exists(&self, rel: impl AsRef<Path>) -> bool {
        self.path(rel).exists()
    }

    /// The temporary file name `studio_model::file::write_atomic` uses for
    /// `target` in this process.
    pub fn atomic_temp_name(target: &Path) -> PathBuf {
        let dir = target.parent().unwrap_or(Path::new("."));
        dir.join(format!(".{}.tmp-{}", target.file_name().unwrap().to_string_lossy(), std::process::id()))
    }

    /// Simulates a crash between writing the temporary file and renaming
    /// it: the new bytes land in the temp file, the target is untouched.
    pub fn crash_between_write_and_rename(&self, rel: impl AsRef<Path>, new_bytes: &[u8]) -> PathBuf {
        let target = self.path(rel);
        let tmp = Self::atomic_temp_name(&target);
        std::fs::write(&tmp, new_bytes).unwrap();
        tmp
    }

    /// Simulates a crash mid-write: a half-written temp file.
    pub fn crash_mid_write(&self, rel: impl AsRef<Path>, new_bytes: &[u8]) -> PathBuf {
        let target = self.path(rel);
        let tmp = Self::atomic_temp_name(&target);
        std::fs::write(&tmp, &new_bytes[..new_bytes.len() / 2]).unwrap();
        tmp
    }

    /// Files in `dir` (relative) that look like leftover temporaries.
    pub fn leftover_temps(&self, dir: impl AsRef<Path>) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(self.path(dir)) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if n.starts_with('.') && n.contains(".tmp-") {
                    out.push(e.path());
                }
            }
        }
        out.sort();
        out
    }

    /// Every file below the root, relative, sorted.
    pub fn list(&self) -> Vec<PathBuf> {
        fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
            if let Ok(rd) = std::fs::read_dir(dir) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        walk(root, &p, out);
                    } else {
                        out.push(p.strip_prefix(root).unwrap().to_path_buf());
                    }
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.root, &self.root, &mut out);
        out.sort();
        out
    }

    /// Makes a file read-only (to provoke write failures).
    pub fn set_readonly(&self, rel: impl AsRef<Path>, ro: bool) {
        let p = self.path(rel);
        let mut perms = std::fs::metadata(&p).unwrap().permissions();
        perms.set_readonly(ro);
        std::fs::set_permissions(&p, perms).unwrap();
    }
}

impl Drop for MockFs {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_encodes_index_in_colour() {
        for i in [0u64, 1, 255, 256, 65535] {
            let p = MockDecoder::picture(2, 2, i);
            if let studio_render::yuv::PixelData::Rgba8(px) = &p.data {
                assert_eq!(MockDecoder::index_of([px[0], px[1], px[2]]), i);
            } else {
                panic!("rgba expected");
            }
        }
    }

    #[test]
    fn decoder_fail_and_stall_are_observable() {
        let mut d = MockDecoder::new(8, 8, 30.0, 10);
        d.control.fail_from(5);
        let mut got = Vec::new();
        assert!(d.decode_to(2, &mut |i, _| got.push(i)));
        assert!(!d.decode_to(7, &mut |i, _| got.push(i)));
        assert_eq!(got, vec![2]);
        assert_eq!(d.control.calls(), vec![2, 7]);
        d.control.stall(Duration::from_millis(20));
        let t = Instant::now();
        assert!(d.decode_to(1, &mut |_, _| {}));
        assert!(t.elapsed() >= Duration::from_millis(20));
    }

    #[test]
    fn audio_out_counts_underruns() {
        let out = MockAudioOut::new();
        out.push(&[0.5; 2 * 100]);
        let b = out.pull(60);
        assert_eq!(b.len(), 120);
        assert_eq!(out.underruns(), 0);
        let b = out.pull(60);
        assert_eq!(out.underruns(), 1);
        assert_eq!(out.underrun_frames(), 20);
        assert_eq!(&b[80..], &[0.0; 40]);
        assert_eq!(out.played_frames(), 120);
    }

    #[test]
    fn nn_missing_and_canned() {
        let nn = MockNn::new();
        nn.missing("whisper");
        assert_eq!(nn.run("whisper", &[]), Err(MockNnError::Missing("whisper".into())));
        nn.canned("whisper", vec![studio_nn::Tensor::f32(&[2], vec![1.0, 2.0])]);
        let out = nn.run("whisper", &[]).unwrap();
        assert_eq!(out[0].as_f32().unwrap(), &[1.0, 2.0]);
        assert_eq!(nn.calls().len(), 2);
    }

    #[test]
    fn constant_model_loads_and_runs() {
        let bytes = MockNn::constant_model(&[1, 3], &[("y", studio_nn::Tensor::f32(&[2], vec![4.0, 5.0]))]);
        let model = studio_nn::Model::from_bytes(&bytes).expect("model parses");
        let session = studio_nn::Session::new(&model, studio_nn::Options::default()).expect("session");
        let out = session.run(&[("x", studio_nn::Tensor::f32(&[1, 3], vec![0.0; 3]))]).expect("run");
        let y = out.iter().find(|(n, _)| n == "y").map(|(_, t)| t.clone()).expect("output y");
        assert_eq!(y.as_f32().unwrap(), &[4.0, 5.0]);
    }

    #[test]
    fn clock_only_moves_when_told() {
        let c = MockClock::new();
        let a = c.now();
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(c.now(), a);
        c.advance(Duration::from_secs(31));
        assert_eq!(c.now() - a, Duration::from_secs(31));
    }

    #[test]
    fn fs_cleans_up_and_finds_temps() {
        let root;
        {
            let fs = MockFs::new("mockfs");
            root = fs.root().to_path_buf();
            fs.write("a/project.json", b"old");
            let tmp = fs.crash_between_write_and_rename("a/project.json", b"new");
            assert!(tmp.exists());
            assert_eq!(fs.read("a/project.json"), b"old");
            assert_eq!(fs.leftover_temps("a").len(), 1);
        }
        assert!(!root.exists());
    }
}
