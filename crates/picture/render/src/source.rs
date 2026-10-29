// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Media sources and the decode cache.
//!
//! A [`MediaSource`] turns a source time into a decoded picture. Video
//! files go through the in-repo `mp4` demuxer and `h264` / `hevc` decoders;
//! stills through `media`'s PNG reader; tests and benchmarks use
//! [`ProceduralSource`]. Decoding is sequential within a group of pictures,
//! so a source keeps its decoder position and only seeks (to the nearest
//! earlier sync sample) when the requested frame is behind it or far ahead.
//!
//! Every picture decoded on the way to the requested one goes into the
//! [`DecodeCache`], keyed by `(media, frame index)`, which holds the most
//! recently used pictures within a byte budget. Playing forward therefore
//! finds every frame already decoded by the previous request, and scrubbing
//! back over recent frames costs nothing.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// How close (in seconds) a time may fall before a frame's start and still
/// show that frame. Times are whole microseconds, so a frame at 1/30 s
/// (33 333.33 µs) is asked for as 33 333 µs: without this margin it picked
/// the frame before, and 30 fps footage played two pictures in three
/// late (repeat, then skip: judder). Far below any frame's length.
const FRAME_EDGE: f64 = 5e-6;

/// The frame on screen at `secs`, from sorted presentation times.
fn index_at(pres_times: &[f64], secs: f64) -> u64 {
    let i = pres_times.partition_point(|&t| t <= secs + FRAME_EDGE);
    (i.saturating_sub(1)).min(pres_times.len().saturating_sub(1)) as u64
}

use crate::colour_space::{ColourDesc, InputColour};
use crate::scene::{MediaKey, Micros, SECOND};
use crate::yuv::{DecodedPicture, PictureRef, Planes, PixelData};

/// Static facts about a source.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceInfo {
    /// Coded picture size before display rotation.
    pub width: u32,
    pub height: u32,
    /// Display rotation in degrees (0, 90, 180, 270), clockwise.
    pub rotation: u32,
    pub frame_rate: Option<f64>,
    pub duration: Option<Micros>,
    pub frame_count: Option<u64>,
    pub colour: ColourDesc,
}

impl SourceInfo {
    /// Size after rotation, as the viewer sees it.
    pub fn display_size(&self) -> (u32, u32) {
        if self.rotation == 90 || self.rotation == 270 { (self.height, self.width) } else { (self.width, self.height) }
    }
}

/// Something that decodes pictures.
pub trait MediaSource: Send {
    fn info(&self) -> SourceInfo;

    /// Frame index shown at source time `time`.
    fn frame_index(&self, time: Micros) -> u64;

    /// Decodes until frame `index` is available, handing every picture
    /// decoded on the way (including `index`) to `emit`. Returns `false`
    /// when the frame does not exist.
    fn decode_to(&mut self, index: u64, emit: &mut dyn FnMut(u64, PictureRef)) -> bool;

    /// The frame index of the sync (key) frame at or before `index`: the
    /// cheapest picture near it to decode. Sources without such frames
    /// return `index`.
    fn nearest_sync(&self, index: u64) -> u64 {
        index
    }
}

thread_local! {
    /// How long a picture request on this thread may wait for the decode
    /// thread before a stand-in is used (`None`: wait for the exact frame,
    /// as export does).
    static WAIT_BUDGET: std::cell::Cell<Option<Duration>> = const { std::cell::Cell::new(None) };
    /// Set when a stand-in picture was used since the last `take_provisional`.
    static PROVISIONAL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Sets how long picture requests on this thread wait for decoding before
/// showing the nearest decoded picture instead (`None`: always exact).
pub fn set_wait_budget(budget: Option<Duration>) {
    WAIT_BUDGET.with(|b| b.set(budget));
}

/// Whether a stand-in picture was drawn on this thread since the last call;
/// the caller renders again once the exact pictures are decoded.
pub fn take_provisional() -> bool {
    PROVISIONAL.with(|p| p.replace(false))
}

/// Fast seeking: while the person scrubs, a missing frame is replaced by
/// the key frame before it (one cheap decode) instead of decoding up to
/// the exact frame; the exact frame is drawn once scrubbing stops. It is
/// the preview thread's own setting: a render on any other thread (an
/// export, the AI's frames) stays exact while the person scrubs.
thread_local! {
    static FAST_SEEK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Turns fast seeking on or off for the pictures this thread decodes next.
pub fn set_fast_seek(on: bool) {
    FAST_SEEK.with(|f| f.set(on));
}

// ---------------------------------------------------------------- cache

/// Least-recently-used picture cache with a byte budget.
pub struct DecodeCache {
    inner: Mutex<CacheInner>,
}

struct CacheInner {
    budget: usize,
    bytes: usize,
    stamp: u64,
    map: HashMap<(MediaKey, u64), (PictureRef, u64)>,
    order: BTreeMap<u64, (MediaKey, u64)>,
    hits: u64,
    misses: u64,
}

/// Cache statistics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub bytes: usize,
    pub budget: usize,
    pub pictures: usize,
    pub hits: u64,
    pub misses: u64,
}

impl DecodeCache {
    /// A cache holding at most `budget` bytes of picture data.
    pub fn new(budget: usize) -> Arc<DecodeCache> {
        Arc::new(DecodeCache {
            inner: Mutex::new(CacheInner { budget, bytes: 0, stamp: 0, map: HashMap::new(), order: BTreeMap::new(), hits: 0, misses: 0 }),
        })
    }

    pub fn get(&self, media: MediaKey, index: u64) -> Option<PictureRef> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.stamp += 1;
        let stamp = g.stamp;
        let key = (media, index);
        match g.map.get_mut(&key) {
            Some((pic, old)) => {
                let pic = pic.clone();
                let old_stamp = *old;
                *old = stamp;
                g.order.remove(&old_stamp);
                g.order.insert(stamp, key);
                g.hits += 1;
                Some(pic)
            }
            None => {
                g.misses += 1;
                None
            }
        }
    }

    pub fn insert(&self, media: MediaKey, index: u64, pic: PictureRef) {
        let size = pic.byte_size();
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let key = (media, index);
        if let Some((old, old_stamp)) = g.map.remove(&key) {
            g.bytes -= old.byte_size();
            g.order.remove(&old_stamp);
        }
        g.stamp += 1;
        let stamp = g.stamp;
        g.bytes += size;
        g.map.insert(key, (pic, stamp));
        g.order.insert(stamp, key);
        // Evict the least recently used pictures, but never the one just
        // inserted (a single picture over budget is still usable).
        while g.bytes > g.budget && g.order.len() > 1 {
            let (&oldest, &okey) = g.order.iter().next().unwrap();
            if okey == key {
                break;
            }
            g.order.remove(&oldest);
            if let Some((p, _)) = g.map.remove(&okey) {
                g.bytes -= p.byte_size();
            }
        }
    }

    /// Drops every cached picture of `media` (its decoding changed).
    pub fn remove_media(&self, media: MediaKey) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let keys: Vec<(MediaKey, u64)> = g.map.keys().filter(|k| k.0 == media).copied().collect();
        for k in keys {
            if let Some((p, stamp)) = g.map.remove(&k) {
                g.bytes -= p.byte_size();
                g.order.remove(&stamp);
            }
        }
    }

    /// Whether a picture is cached, without touching its recency.
    pub fn contains(&self, media: MediaKey, index: u64) -> bool {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).map.contains_key(&(media, index))
    }

    /// The cached picture of `media` closest to `index`, preferring one at
    /// or before it: the stand-in while the exact picture is decoded.
    pub fn nearest(&self, media: MediaKey, index: u64) -> Option<PictureRef> {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut best: Option<(u64, bool, &PictureRef)> = None;
        for ((m, i), (pic, _)) in g.map.iter() {
            if *m != media {
                continue;
            }
            let before = *i <= index;
            let d = i.abs_diff(index);
            let better = match best {
                None => true,
                Some((bd, bb, _)) => (before && !bb) || (before == bb && d < bd),
            };
            if better {
                best = Some((d, before, pic));
            }
        }
        best.map(|b| b.2.clone())
    }

    /// Drops every picture of one media item (after a file changed).
    pub fn forget(&self, media: MediaKey) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let keys: Vec<_> = g.map.keys().filter(|k| k.0 == media).copied().collect();
        for k in keys {
            if let Some((p, stamp)) = g.map.remove(&k) {
                g.bytes -= p.byte_size();
                g.order.remove(&stamp);
            }
        }
    }

    pub fn clear(&self) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.map.clear();
        g.order.clear();
        g.bytes = 0;
    }

    pub fn stats(&self) -> CacheStats {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        CacheStats { bytes: g.bytes, budget: g.budget, pictures: g.map.len(), hits: g.hits, misses: g.misses }
    }

    pub fn set_budget(&self, budget: usize) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.budget = budget;
        while g.bytes > g.budget && g.order.len() > 1 {
            let (&oldest, &okey) = g.order.iter().next().unwrap();
            g.order.remove(&oldest);
            if let Some((p, _)) = g.map.remove(&okey) {
                g.bytes -= p.byte_size();
            }
        }
    }
}

// ------------------------------------------------------------- registry

/// How to open a media item.
#[derive(Clone)]
pub enum SourceSpec {
    /// A file on disk; the kind is decided from its contents.
    Path(PathBuf),
    /// A picture generator for tests and benchmarks.
    Procedural(Arc<ProceduralSource>),
}

/// Maps media keys to how to open them. Shared by every worker; each
/// worker opens its own decoders from it.
#[derive(Default)]
pub struct SourceRegistry {
    specs: Mutex<HashMap<MediaKey, SourceSpec>>,
    /// Input colour spaces chosen per media (see [`InputColour`]).
    inputs: Mutex<HashMap<MediaKey, InputColour>>,
    /// Proxy files shown instead of the media (see [`SourceRegistry::set_proxy`]).
    proxies: Mutex<HashMap<MediaKey, Proxy>>,
    /// Bumped when how a media item decodes changes (another file, input
    /// colour or proxy), so open sources and cached pictures are renewed.
    stamps: Mutex<HashMap<MediaKey, u64>>,
}

/// A lower-resolution stand-in for a media item.
#[derive(Clone, Debug, PartialEq)]
pub struct Proxy {
    pub path: PathBuf,
    /// The original's coded size: the proxy is drawn at it.
    pub size: (u32, u32),
    /// How the proxy's code values are encoded, when its file cannot say
    /// (camera log kept as it was); the media's own input colour wins.
    pub input: Option<InputColour>,
}

impl SourceRegistry {
    pub fn new() -> Arc<SourceRegistry> {
        Arc::new(SourceRegistry::default())
    }

    pub fn register(&self, key: MediaKey, spec: SourceSpec) {
        let mut g = self.specs.lock().unwrap_or_else(|e| e.into_inner());
        let same = matches!((g.get(&key), &spec), (Some(SourceSpec::Path(a)), SourceSpec::Path(b)) if a == b);
        g.insert(key, spec);
        drop(g);
        if !same {
            self.bump(key);
        }
    }

    fn bump(&self, key: MediaKey) {
        *self.stamps.lock().unwrap_or_else(|e| e.into_inner()).entry(key).or_insert(0) += 1;
    }

    /// Changes whenever how `key` decodes changes.
    pub fn stamp(&self, key: MediaKey) -> u64 {
        self.stamps.lock().unwrap_or_else(|e| e.into_inner()).get(&key).copied().unwrap_or(0)
    }

    pub fn register_path(&self, key: MediaKey, path: impl Into<PathBuf>) {
        self.register(key, SourceSpec::Path(path.into()));
    }

    pub fn remove(&self, key: MediaKey) {
        self.specs.lock().unwrap_or_else(|e| e.into_inner()).remove(&key);
    }

    pub fn spec(&self, key: MediaKey) -> Option<SourceSpec> {
        self.specs.lock().unwrap_or_else(|e| e.into_inner()).get(&key).cloned()
    }

    /// Chooses (or with `None` clears) the input colour space of a media
    /// item. Sources opened afterwards use it; callers drop their open
    /// sources (and cached pictures) when it changes.
    pub fn set_input(&self, key: MediaKey, input: Option<InputColour>) {
        let mut g = self.inputs.lock().unwrap_or_else(|e| e.into_inner());
        let new = input.filter(|i| *i != InputColour::default());
        if g.get(&key).copied() == new {
            return;
        }
        match new {
            Some(i) => {
                g.insert(key, i);
            }
            None => {
                g.remove(&key);
            }
        }
        drop(g);
        self.bump(key);
    }

    pub fn input(&self, key: MediaKey) -> Option<InputColour> {
        self.inputs.lock().unwrap_or_else(|e| e.into_inner()).get(&key).copied()
    }

    /// Shows `proxy` instead of the media's file (or with `None` the file
    /// again). Sources opened afterwards use it.
    pub fn set_proxy(&self, key: MediaKey, proxy: Option<Proxy>) {
        let mut g = self.proxies.lock().unwrap_or_else(|e| e.into_inner());
        if g.get(&key) == proxy.as_ref() {
            return;
        }
        match proxy {
            Some(p) => {
                g.insert(key, p);
            }
            None => {
                g.remove(&key);
            }
        }
        drop(g);
        self.bump(key);
    }

    pub fn proxy(&self, key: MediaKey) -> Option<Proxy> {
        self.proxies.lock().unwrap_or_else(|e| e.into_inner()).get(&key).cloned()
    }

    pub fn keys(&self) -> Vec<MediaKey> {
        self.specs.lock().unwrap_or_else(|e| e.into_inner()).keys().copied().collect()
    }
}

/// A proxy file standing in for a media item: decodes the proxy and
/// reports the original's size, so the renderer draws the smaller picture
/// over the whole original frame.
struct ProxySource {
    inner: Box<dyn MediaSource>,
    size: (u32, u32),
}

impl MediaSource for ProxySource {
    fn info(&self) -> SourceInfo {
        let mut i = self.inner.info();
        i.width = self.size.0.max(1);
        i.height = self.size.1.max(1);
        i
    }

    fn frame_index(&self, time: Micros) -> u64 {
        self.inner.frame_index(time)
    }

    fn decode_to(&mut self, index: u64, emit: &mut dyn FnMut(u64, PictureRef)) -> bool {
        self.inner.decode_to(index, emit)
    }

    fn nearest_sync(&self, index: u64) -> u64 {
        self.inner.nearest_sync(index)
    }
}

/// Opens a file as a source by sniffing its contents.
pub fn open_path(path: &Path) -> Result<Box<dyn MediaSource>, String> {
    Ok(match open_any(path, None)? {
        Opened::Video(v) => Box::new(v),
        Opened::Other(o) => o,
    })
}

/// An opened file: a video track (decoded ahead when used by a renderer)
/// or any other source.
enum Opened {
    Video(Mp4Source),
    Other(Box<dyn MediaSource>),
}

fn open_any(path: &Path, input: Option<InputColour>) -> Result<Opened, String> {
    let still = |mut pic: DecodedPicture| {
        if let Some(i) = input {
            pic.colour = i.apply(pic.colour);
        }
        Ok(Opened::Other(Box::new(StillSource::new(pic))))
    };
    // Frame sequences are named by a pattern (shot.####.exr), not a file.
    if studio_imageio::sequence::Sequence::is_pattern(path) && !path.exists() {
        return crate::stills::SequenceSource::open(path, input, None).map(|s| Opened::Other(Box::new(s)));
    }
    let mut head = [0u8; 32];
    {
        use std::io::Read;
        let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let n = f.read(&mut head).map_err(|e| e.to_string())?;
        if n < 8 {
            return Err(format!("{}: file too short", path.display()));
        }
    }
    // OpenEXR, DPX, TIFF and 16-bit PNG at full precision.
    if crate::stills::sniff(&head) || crate::stills::is_png16(&head) {
        return crate::stills::open_still(path, input).map(|s| Opened::Other(Box::new(s)));
    }
    if &head[..8] == b"\x89PNG\r\n\x1a\n" {
        let img = studio_media::png::read(path)?;
        return still(DecodedPicture::from_rgba8(img.width, img.height, img.pixels));
    }
    // JPEG (photos): it fell through to "unsupported" and drew black.
    if head[..3] == [0xFF, 0xD8, 0xFF] {
        let img = studio_media::jpeg::read(path)?;
        return still(DecodedPicture::from_rgba8(img.width, img.height, img.pixels));
    }
    // Lottie animations (JSON, or a dotLottie ZIP): vector, drawn per frame.
    let text_start = head.iter().find(|b| !b.is_ascii_whitespace() && **b != 0xEF && **b != 0xBB && **b != 0xBF);
    if text_start == Some(&b'{') || head.starts_with(b"PK\x03\x04") {
        if std::fs::read(path).is_ok_and(|b| studio_lottie::sniff(&b)) {
            return LottieSource::open(path).map(|l| Opened::Other(Box::new(l)));
        }
    }
    if &head[4..8] == b"ftyp" || &head[4..8] == b"moov" || &head[4..8] == b"wide" || &head[4..8] == b"mdat" || &head[4..8] == b"free" {
        return match Mp4Source::open(path) {
            Ok(mut v) => {
                v.set_input(input);
                Ok(Opened::Video(v))
            }
            // ProRes plays natively (intra-only, decoded on request).
            Err(e) => match crate::prores_source::ProResSource::open(path, input) {
                Ok(p) => Ok(Opened::Other(Box::new(p))),
                Err(_) => Err(e),
            },
        };
    }
    Err(format!("{}: unsupported file type", path.display()))
}

/// A Lottie animation (`.json` / `.lottie`), drawn frame by frame with its
/// transparency by `studio-lottie`.
pub struct LottieSource {
    anim: Arc<studio_lottie::Animation>,
    size: (u32, u32),
}

impl LottieSource {
    pub fn open(path: &Path) -> Result<LottieSource, String> {
        let anim = studio_lottie::Animation::open(path)?;
        let size = anim.picture_size();
        Ok(LottieSource { anim: Arc::new(anim), size })
    }
}

impl MediaSource for LottieSource {
    fn info(&self) -> SourceInfo {
        SourceInfo {
            width: self.size.0,
            height: self.size.1,
            rotation: 0,
            frame_rate: Some(self.anim.fps),
            duration: Some((self.anim.duration() * 1e6).round() as Micros),
            frame_count: Some(self.anim.frame_count()),
            colour: ColourDesc::SRGB8,
        }
    }

    fn frame_index(&self, time: Micros) -> u64 {
        ((time.max(0) as f64 / 1e6 * self.anim.fps).floor() as u64).min(self.anim.frame_count() - 1)
    }

    fn decode_to(&mut self, index: u64, emit: &mut dyn FnMut(u64, PictureRef)) -> bool {
        if index >= self.anim.frame_count() {
            return false;
        }
        let rgba = self.anim.render_rgba(self.anim.in_frame + index as f64, self.size.0, self.size.1);
        emit(index, Arc::new(DecodedPicture::from_rgba8(self.size.0, self.size.1, rgba)));
        true
    }
}

/// One worker's set of open sources plus the shared cache.
pub struct Sources {
    registry: Arc<SourceRegistry>,
    cache: Arc<DecodeCache>,
    open: HashMap<MediaKey, Box<dyn MediaSource>>,
    failed: HashMap<MediaKey, String>,
    /// The registry stamp each key was opened with.
    stamps: HashMap<MediaKey, u64>,
}

impl Sources {
    pub fn new(registry: Arc<SourceRegistry>, cache: Arc<DecodeCache>) -> Sources {
        Sources { registry, cache, open: HashMap::new(), failed: HashMap::new(), stamps: HashMap::new() }
    }

    pub fn cache(&self) -> &Arc<DecodeCache> {
        &self.cache
    }

    pub fn registry(&self) -> &Arc<SourceRegistry> {
        &self.registry
    }

    fn source(&mut self, key: MediaKey) -> Option<&mut Box<dyn MediaSource>> {
        // Reopen when the registry changed how this media decodes.
        let stamp = self.registry.stamp(key);
        if self.stamps.get(&key).is_some_and(|s| *s != stamp) {
            self.open.remove(&key);
            self.failed.remove(&key);
            self.cache.remove_media(key);
        }
        self.stamps.insert(key, stamp);
        if !self.open.contains_key(&key) {
            if self.failed.contains_key(&key) {
                return None;
            }
            let spec = self.registry.spec(key)?;
            // A proxy, when one is set and its file is there.
            if let Some(px) = self.registry.proxy(key).filter(|p| p.path.exists()) {
                let input = self.registry.input(key).or(px.input);
                let opened = match open_any(&px.path, input) {
                    Ok(Opened::Video(v)) => Ok(Box::new(AheadSource::start(v, key, self.cache.clone())) as Box<dyn MediaSource>),
                    Ok(Opened::Other(o)) => Ok(o),
                    Err(e) => Err(e),
                };
                if let Ok(inner) = opened {
                    self.open.insert(key, Box::new(ProxySource { inner, size: px.size }));
                    return self.open.get_mut(&key);
                }
            }
            let opened: Result<Box<dyn MediaSource>, String> = match spec {
                // Video files decode on their own thread, ahead of requests.
                SourceSpec::Path(p) => match open_any(&p, self.registry.input(key)) {
                    Ok(Opened::Video(v)) => Ok(Box::new(AheadSource::start(v, key, self.cache.clone())) as Box<dyn MediaSource>),
                    Ok(Opened::Other(o)) => Ok(o),
                    Err(e) => Err(e),
                },
                SourceSpec::Procedural(p) => Ok(Box::new(ProceduralHandle(p))),
            };
            match opened {
                Ok(s) => {
                    self.open.insert(key, s);
                }
                Err(e) => {
                    self.failed.insert(key, e);
                    return None;
                }
            }
        }
        self.open.get_mut(&key)
    }

    /// Why a media item could not be opened, if it failed.
    pub fn failure(&self, key: MediaKey) -> Option<&str> {
        self.failed.get(&key).map(String::as_str)
    }

    /// Facts about a media item (opens it if needed).
    pub fn info(&mut self, key: MediaKey) -> Option<SourceInfo> {
        self.source(key).map(|s| s.info())
    }

    /// The picture shown at `time`, from the cache or freshly decoded.
    pub fn picture(&mut self, key: MediaKey, time: Micros) -> Option<PictureRef> {
        let mut index = self.source(key)?.frame_index(time);
        if let Some(p) = self.cache.get(key, index) {
            return Some(p);
        }
        if FAST_SEEK.with(|f| f.get()) {
            index = self.source(key)?.nearest_sync(index);
            if let Some(p) = self.cache.get(key, index) {
                return Some(p);
            }
        }
        let cache = self.cache.clone();
        let src = self.source(key)?;
        let mut wanted = None;
        let ok = src.decode_to(index, &mut |i, pic| {
            if i == index {
                wanted = Some(pic.clone());
            }
            cache.insert(key, i, pic);
        });
        if ok && wanted.is_some() {
            return wanted;
        }
        // Not decoded in time (or superseded): the nearest decoded picture
        // stands in, and the caller is told to draw again.
        let standin = self.cache.nearest(key, index);
        if standin.is_some() {
            PROVISIONAL.with(|p| p.set(true));
        }
        standin
    }

    /// The frame index a media item shows at `time` (opens it if needed).
    pub fn frame_index(&mut self, key: MediaKey, time: Micros) -> Option<u64> {
        Some(self.source(key)?.frame_index(time))
    }

    /// Frame `index` of a media item, exactly (from the cache or decoded).
    pub fn picture_at(&mut self, key: MediaKey, index: u64) -> Option<PictureRef> {
        if let Some(p) = self.cache.get(key, index) {
            return Some(p);
        }
        let cache = self.cache.clone();
        let src = self.source(key)?;
        let mut wanted = None;
        src.decode_to(index, &mut |i, pic| {
            if i == index {
                wanted = Some(pic.clone());
            }
            cache.insert(key, i, pic);
        });
        wanted
    }

    /// Forgets open decoders (e.g. after an idle period) without touching
    /// the cache.
    pub fn close_all(&mut self) {
        self.open.clear();
        self.failed.clear();
    }
}

// ---------------------------------------------------------------- still

/// A single picture shown for any time.
pub struct StillSource {
    picture: PictureRef,
}

impl StillSource {
    pub fn new(picture: DecodedPicture) -> StillSource {
        StillSource { picture: Arc::new(picture) }
    }
}

impl MediaSource for StillSource {
    fn info(&self) -> SourceInfo {
        SourceInfo {
            width: self.picture.width,
            height: self.picture.height,
            rotation: 0,
            frame_rate: None,
            duration: None,
            frame_count: Some(1),
            colour: self.picture.colour,
        }
    }

    fn frame_index(&self, _time: Micros) -> u64 {
        0
    }

    fn decode_to(&mut self, index: u64, emit: &mut dyn FnMut(u64, PictureRef)) -> bool {
        if index != 0 {
            return false;
        }
        emit(0, self.picture.clone());
        true
    }
}

// ----------------------------------------------------------- procedural

/// Generates pictures from a function of the frame index.
pub struct ProceduralSource {
    pub info: SourceInfo,
    pub generate: Box<dyn Fn(u64) -> DecodedPicture + Send + Sync>,
}

impl ProceduralSource {
    /// A source of `frame_rate` fps, `frames` frames, calling `generate`.
    pub fn new(width: u32, height: u32, frame_rate: f64, frames: u64, generate: impl Fn(u64) -> DecodedPicture + Send + Sync + 'static) -> Arc<ProceduralSource> {
        Arc::new(ProceduralSource {
            info: SourceInfo {
                width,
                height,
                rotation: 0,
                frame_rate: Some(frame_rate),
                duration: Some((frames as f64 / frame_rate * SECOND as f64).round() as Micros),
                frame_count: Some(frames),
                colour: ColourDesc::default(),
            },
            generate: Box::new(generate),
        })
    }

    /// A moving test pattern in 8-bit BT.709 4:2:0: a colour ramp with a
    /// bright square that moves one pixel per frame, so caches cannot cheat.
    pub fn test_pattern(width: u32, height: u32, frame_rate: f64, frames: u64) -> Arc<ProceduralSource> {
        ProceduralSource::new(width, height, frame_rate, frames, move |i| {
            let (w, h) = (width as usize, height as usize);
            let cw = w.div_ceil(2);
            let ch = h.div_ceil(2);
            let mut y = vec![0u8; w * h];
            let mut u = vec![128u8; cw * ch];
            let mut v = vec![128u8; cw * ch];
            let sq = (i as usize * 2) % w.max(1);
            for yy in 0..h {
                for xx in 0..w {
                    let mut l = 16 + ((xx * 180) / w.max(1)) as u32 + ((yy * 30) / h.max(1)) as u32;
                    if xx >= sq && xx < sq + w / 8 && yy >= h / 4 && yy < h / 2 {
                        l = 235;
                    }
                    y[yy * w + xx] = l.min(235) as u8;
                }
            }
            for yy in 0..ch {
                for xx in 0..cw {
                    u[yy * cw + xx] = (64 + (xx * 128) / cw.max(1)) as u8;
                    v[yy * cw + xx] = (64 + (yy * 128) / ch.max(1)) as u8;
                }
            }
            DecodedPicture::from_i420(width, height, y, u, v, ColourDesc::default())
        })
    }
}

struct ProceduralHandle(Arc<ProceduralSource>);

impl MediaSource for ProceduralHandle {
    fn info(&self) -> SourceInfo {
        self.0.info.clone()
    }

    fn frame_index(&self, time: Micros) -> u64 {
        let fps = self.0.info.frame_rate.unwrap_or(30.0);
        let i = (time.max(0) as f64 / SECOND as f64 * fps + FRAME_EDGE * fps).floor() as u64;
        match self.0.info.frame_count {
            Some(n) if n > 0 => i.min(n - 1),
            _ => i,
        }
    }

    fn decode_to(&mut self, index: u64, emit: &mut dyn FnMut(u64, PictureRef)) -> bool {
        if let Some(n) = self.0.info.frame_count
            && index >= n
        {
            return false;
        }
        emit(index, Arc::new((self.0.generate)(index)));
        true
    }
}

// ------------------------------------------------------------------ mp4

enum VideoDecoder {
    /// The machine's video hardware (H.264).
    Hardware(studio_hw::H264Decoder),
    /// The machine's video hardware (HEVC).
    HardwareHevc(studio_hw::HevcDecoder),
    /// The machine's video hardware (AV1, 8-bit 4:2:0).
    HardwareAv1(studio_hw::Av1Decoder),
    H264(studio_h264::Decoder),
    Hevc(studio_hevc::Decoder),
    Av1(Box<studio_av1::Decoder>),
}

/// H.264 / HEVC / AV1 video in an MP4 or MOV file.
pub struct Mp4Source {
    demuxer: studio_mp4::Demuxer,
    track: studio_mp4::Track,
    /// Sample indices in presentation order (one per frame index).
    pres_order: Vec<usize>,
    /// Presentation time (seconds) of each frame index.
    pres_times: Vec<f64>,
    /// Frame index of each sample index (`u64::MAX` when not presented).
    frame_of_sample: Vec<u64>,
    decoder: Option<VideoDecoder>,
    /// Input colour space chosen for this media, over what the file says.
    input: Option<InputColour>,
    /// Decode on Studio's own decoders even where hardware exists (also
    /// `STUDIO_SOFTWARE_DECODE`).
    software_only: bool,
    /// Next sample (decode order) to feed.
    next_sample: usize,
    /// First frame not yet decoded since the decoder last started.
    next_out: u64,
    /// Studio's HEVC decoder hands pictures out in presentation order
    /// without timestamps: the samples fed and not yet out, by presentation
    /// time, so each picture takes the earliest.
    pending: std::collections::BTreeSet<(i64, usize)>,
    /// Frames handed out since the decoder last started (pictures come out
    /// of order with B-frames). A request for one of them cannot be met by
    /// decoding on: it restarts at its sync sample instead of running to
    /// the end of the file looking for it.
    emitted: std::collections::HashSet<u64>,
    /// Samples fed to decoders since opening (work done, for tests).
    fed_total: u64,
    colour: ColourDesc,
    info: SourceInfo,
    /// How far ahead a request may be before seeking instead of decoding
    /// through (frames).
    max_run_ahead: u64,
    /// Set by the decode thread: the generation this decode is for; a
    /// newer request makes `decode_to` stop early.
    abort: Option<(Arc<AtomicU64>, u64)>,
}

impl Mp4Source {
    pub fn open(path: &Path) -> Result<Mp4Source, String> {
        let demuxer = studio_mp4::open(path).map_err(|e| format!("{}: {e:?}", path.display()))?;
        let track = demuxer.movie().video().ok_or_else(|| format!("{}: no video track", path.display()))?.clone();
        let mut frames: Vec<(f64, usize)> = track.samples.iter().enumerate().filter_map(|(i, s)| track.presentation_secs(s).map(|t| (t, i))).collect();
        frames.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.cmp(&b.1)));
        let pres_order: Vec<usize> = frames.iter().map(|f| f.1).collect();
        let pres_times: Vec<f64> = frames.iter().map(|f| f.0).collect();
        let mut frame_of_sample = vec![u64::MAX; track.samples.len()];
        for (k, &s) in pres_order.iter().enumerate() {
            frame_of_sample[s] = k as u64;
        }
        let bit_depth = match &track.codec {
            studio_mp4::Codec::Avc { config } => config.bit_depth_luma.unwrap_or(8),
            studio_mp4::Codec::Hevc { config } => config.bit_depth_luma.max(8),
            studio_mp4::Codec::Av1 { config } => config.bit_depth(),
            other => return Err(format!("{}: unsupported video codec {other:?}", path.display())),
        };
        // Colour from the container; the bitstream VUI overrides it per
        // frame when it is specified.
        let colour = match &track.color {
            Some(studio_mp4::ColorInfo::Nclx { primaries, transfer, matrix, full_range }) => {
                ColourDesc::from_h273(*primaries as u8, *transfer as u8, *matrix as u8, *full_range, bit_depth)
            }
            _ => ColourDesc { bit_depth, ..ColourDesc::default() },
        };
        let frame_rate = if pres_times.len() >= 2 {
            let span = pres_times[pres_times.len() - 1] - pres_times[0];
            if span > 0.0 { Some((pres_times.len() - 1) as f64 / span) } else { None }
        } else {
            None
        };
        let info = SourceInfo {
            width: track.width.max(1),
            height: track.height.max(1),
            rotation: (track.rotation as u32) % 360,
            frame_rate,
            duration: Some((track.duration_secs * SECOND as f64).round() as Micros),
            frame_count: Some(pres_order.len() as u64),
            colour,
        };
        Ok(Mp4Source {
            demuxer,
            track,
            pres_order,
            pres_times,
            frame_of_sample,
            decoder: None,
            input: None,
            software_only: false,
            next_sample: 0,
            next_out: 0,
            pending: std::collections::BTreeSet::new(),
            emitted: std::collections::HashSet::new(),
            fed_total: 0,
            colour,
            info,
            max_run_ahead: 90,
            abort: None,
        })
    }

    /// Whether the machine's video hardware may be used.
    fn hardware_allowed(&self) -> bool {
        !self.software_only && std::env::var_os("STUDIO_SOFTWARE_DECODE").is_none()
    }

    /// Decodes on Studio's own decoders only (`true`), or hardware first.
    pub fn set_software_only(&mut self, on: bool) {
        self.software_only = on;
        self.decoder = None;
    }

    fn new_decoder(&self) -> Result<VideoDecoder, String> {
        match &self.track.codec {
            studio_mp4::Codec::Avc { config } => {
                // Hardware first: the media engine decodes and the CPU is
                // left to render. Studio's own decoder otherwise.
                if config.bit_depth_luma.unwrap_or(8) == 8 && self.hardware_allowed() {
                    if let Some(hw) = studio_hw::H264Decoder::new(&config.raw) {
                        return Ok(VideoDecoder::Hardware(hw));
                    }
                }
                studio_h264::Decoder::from_avcc_bytes(&config.raw).map(VideoDecoder::H264).map_err(|e| format!("h264: {e:?}"))
            }
            studio_mp4::Codec::Hevc { config } => {
                let sets: Vec<Vec<u8>> = config.arrays.iter().flat_map(|a| a.nalus.iter().cloned()).collect();
                // Hardware first here too (iPhone and camera footage is
                // HEVC, often 4K); Studio's own decoder otherwise.
                if self.hardware_allowed() {
                    if let Some(hw) = studio_hw::HevcDecoder::new(&sets, config.nal_length_size as usize) {
                        return Ok(VideoDecoder::HardwareHevc(hw));
                    }
                }
                studio_hevc::Decoder::new(&sets, config.nal_length_size as usize).map(VideoDecoder::Hevc).map_err(|e| format!("hevc: {e:?}"))
            }
            studio_mp4::Codec::Av1 { config } => {
                // Hardware first (8-bit 4:2:0 on a Mac with an AV1 engine);
                // Studio's own decoder otherwise, which also reads 10-bit.
                if self.hardware_allowed() {
                    if let Ok(hw) = studio_hw::Av1Decoder::new(&config.raw, self.track.width, self.track.height) {
                        return Ok(VideoDecoder::HardwareAv1(hw));
                    }
                }
                Ok(VideoDecoder::Av1(Box::new(studio_av1::Decoder::new())))
            }
            _ => Err("unsupported codec".into()),
        }
    }

    /// Resets decoding to start at `sync` (a sample index).
    fn restart_at(&mut self, sync: usize) -> bool {
        // A hardware session is kept (warm) and only emptied: a new one is
        // slow for its first frames, which made every seek and replay stutter.
        let kept = match self.decoder.as_mut() {
            Some(VideoDecoder::Hardware(d)) => {
                d.reset();
                true
            }
            Some(VideoDecoder::HardwareHevc(d)) => {
                d.reset();
                true
            }
            Some(VideoDecoder::HardwareAv1(d)) => {
                d.reset();
                true
            }
            _ => false,
        };
        if !kept {
            match self.new_decoder() {
                Ok(d) => self.decoder = Some(d),
                Err(_) => return false,
            }
        }
        self.next_sample = sync;
        // A sync sample before an edit list's start is not shown itself:
        // the first frame it leads to is.
        self.next_out = self.frame_of_sample[sync..].iter().take(512).copied().filter(|&f| f != u64::MAX).min().unwrap_or(0);
        self.emitted.clear();
        self.pending.clear();
        true
    }

    /// Frame index of a decoded sample; `None` for one the edit list hides
    /// (pre-roll before a trimmed start), which must not stand for a frame.
    fn frame_of(&self, sample: usize) -> Option<u64> {
        self.frame_of_sample.get(sample).copied().filter(|&i| i != u64::MAX)
    }

    /// Frame index of the next picture out of Studio's HEVC decoder.
    fn next_hevc_frame(&mut self) -> Option<u64> {
        let (_, s) = self.pending.pop_first()?;
        self.frame_of(s)
    }

    fn h264_index(&self, f: &studio_h264::Frame) -> Option<u64> {
        self.frame_of(f.pts? as usize)
    }

    fn picture_from_h264(&self, f: &studio_h264::Frame) -> DecodedPicture {
        let c = f.color;
        let mut colour = self.colour;
        colour.bit_depth = 8;
        if c.matrix_coefficients != 2 {
            colour.matrix = crate::colour_space::Matrix::from_h273(c.matrix_coefficients);
        }
        if c.colour_primaries != 2 {
            colour.primaries = crate::colour_space::SourcePrimaries::from_h273(c.colour_primaries);
        }
        if c.transfer_characteristics != 2 {
            colour.transfer = crate::colour_space::SourceTransfer::from_h273(c.transfer_characteristics);
        }
        colour.full_range = c.full_range || (self.colour.full_range && c.matrix_coefficients == 2);
        DecodedPicture {
            width: f.width,
            height: f.height,
            data: PixelData::Yuv {
                y: Planes::U8(f.y.clone()),
                u: Planes::U8(f.u.clone()),
                v: Planes::U8(f.v.clone()),
                chroma_width: f.chroma_width,
                chroma_height: f.chroma_height,
            },
            colour: self.recolour(colour),
        }
    }

    fn picture_from_hardware(&self, f: studio_hw::Decoded) -> Option<(u64, DecodedPicture)> {
        let idx = self.frame_of(f.sample as usize)?;
        let mut colour = self.colour;
        colour.bit_depth = 8;
        colour.full_range = false;
        Some((
            idx,
            DecodedPicture {
                width: f.width,
                height: f.height,
                data: PixelData::Yuv { y: Planes::U8(f.y), u: Planes::U8(f.u), v: Planes::U8(f.v), chroma_width: f.chroma_width, chroma_height: f.chroma_height },
                colour: self.recolour(colour),
            },
        ))
    }

    /// Frame index of a picture from Studio's AV1 decoder (its pts is the
    /// sample index it was fed).
    fn av1_index(&self, p: &studio_av1::Picture) -> Option<u64> {
        self.frame_of(usize::try_from(p.pts).ok()?)
    }

    fn picture_from_av1(&self, p: &studio_av1::Picture) -> DecodedPicture {
        let mut colour = self.colour;
        colour.bit_depth = p.bit_depth.max(8);
        if p.matrix_coefficients != 2 {
            colour.matrix = crate::colour_space::Matrix::from_h273(p.matrix_coefficients);
        }
        if p.color_primaries != 2 {
            colour.primaries = crate::colour_space::SourcePrimaries::from_h273(p.color_primaries);
        }
        if p.transfer_characteristics != 2 {
            colour.transfer = crate::colour_space::SourceTransfer::from_h273(p.transfer_characteristics);
        }
        colour.full_range = p.full_range;
        let (cw, ch) = (p.plane_width(1) as usize, p.plane_height(1) as usize);
        let mid = 1u16 << (p.bit_depth.max(8) - 1);
        let plane = |k: usize| -> Vec<u16> {
            // Monochrome streams carry no chroma: neutral grey.
            if p.mono_chrome && k > 0 { vec![mid; cw * ch] } else { p.planes[k].clone() }
        };
        let planes = |v: Vec<u16>| if p.bit_depth > 8 { Planes::U16(v) } else { Planes::U8(v.into_iter().map(|x| x as u8).collect()) };
        DecodedPicture {
            width: p.width,
            height: p.height,
            data: PixelData::Yuv { y: planes(plane(0)), u: planes(plane(1)), v: planes(plane(2)), chroma_width: cw as u32, chroma_height: ch as u32 },
            colour: self.recolour(colour),
        }
    }

    fn picture_from_hevc(&self, f: &studio_hevc::Frame) -> DecodedPicture {
        let mut colour = self.colour;
        colour.bit_depth = f.bit_depth.max(8);
        colour.matrix = crate::colour_space::Matrix::from_h273(f.color.matrix_coeffs);
        colour.full_range = f.color.full_range;
        let planes = |p: &studio_hevc::PlaneData| match p {
            studio_hevc::PlaneData::U8(v) => Planes::U8(v.clone()),
            studio_hevc::PlaneData::U16(v) => Planes::U16(v.clone()),
        };
        DecodedPicture {
            width: f.width as u32,
            height: f.height as u32,
            data: PixelData::Yuv {
                y: planes(&f.y),
                u: planes(&f.u),
                v: planes(&f.v),
                chroma_width: f.chroma_width as u32,
                chroma_height: f.chroma_height as u32,
            },
            colour: self.recolour(colour),
        }
    }

    /// Applies the chosen input colour space, if any.
    fn recolour(&self, c: ColourDesc) -> ColourDesc {
        self.input.map_or(c, |i| i.apply(c))
    }

    /// Chooses an input colour space for this file.
    pub fn set_input(&mut self, input: Option<InputColour>) {
        self.input = input;
        self.info.colour = self.recolour(self.colour);
    }

    /// Frame index for a presentation time in seconds.
    fn index_for_secs(&self, secs: f64) -> u64 {
        let n = self.pres_times.len();
        if n == 0 {
            return 0;
        }
        index_at(&self.pres_times, secs)
    }

    /// Feeds one sample and emits what comes out. Returns `false` at the end.
    fn step(&mut self, emit: &mut dyn FnMut(u64, PictureRef), emitted_target: &mut bool, target: u64) -> bool {
        if self.next_sample >= self.track.samples.len() {
            return false;
        }
        let sample = self.track.samples[self.next_sample];
        self.fed_total += 1;
        let data = match self.demuxer.read_sample(&sample) {
            Ok(d) => d,
            Err(_) => {
                self.next_sample += 1;
                return true;
            }
        };
        let frames: Vec<(u64, DecodedPicture)> = match self.decoder.as_mut() {
            Some(VideoDecoder::Hardware(d)) => {
                let out = d.decode(&data, self.next_sample as i64).unwrap_or_default();
                out.into_iter().filter_map(|f| self.picture_from_hardware(f)).collect()
            }
            Some(VideoDecoder::HardwareHevc(d)) => {
                let out = d.decode(&data, self.next_sample as i64).unwrap_or_default();
                out.into_iter().filter_map(|f| self.picture_from_hardware(f)).collect()
            }
            Some(VideoDecoder::HardwareAv1(d)) => {
                let out = d.decode(&data, self.next_sample as i64).unwrap_or_default();
                out.into_iter().filter_map(|f| self.picture_from_hardware(f)).collect()
            }
            Some(VideoDecoder::Av1(d)) => {
                // Pictures carry the sample index as their pts, as with H.264.
                let out = d.decode_pts(&data, self.next_sample as i64).unwrap_or_default();
                out.iter().filter_map(|p| Some((self.av1_index(p)?, self.picture_from_av1(p)))).collect()
            }
            Some(VideoDecoder::H264(d)) => {
                // The pts handed to the decoder is the sample index, which
                // maps to a frame index exactly, whatever the reordering.
                let out = d.decode_with_pts(&data, Some(self.next_sample as i64)).unwrap_or_default();
                out.iter().filter_map(|f| Some((self.h264_index(f)?, self.picture_from_h264(f)))).collect()
            }
            Some(VideoDecoder::Hevc(d)) => {
                let out = d.decode(&data).unwrap_or_default();
                self.pending.insert((sample.pts, self.next_sample));
                let mut v = Vec::with_capacity(out.len());
                for f in &out {
                    if let Some(i) = self.next_hevc_frame() {
                        v.push((i, self.picture_from_hevc(f)));
                    }
                }
                v
            }
            None => Vec::new(),
        };
        for (idx, pic) in frames {
            if idx == target {
                *emitted_target = true;
            }
            self.emitted.insert(idx);
            emit(idx, Arc::new(pic));
        }
        // How far decoding has got: the first frame not yet out (followed,
        // or a request more than `max_run_ahead` past the last sync frame
        // restarted the whole group every time).
        while self.emitted.contains(&self.next_out) {
            self.next_out += 1;
        }
        self.next_sample += 1;
        true
    }

    fn flush(&mut self, emit: &mut dyn FnMut(u64, PictureRef), emitted_target: &mut bool, target: u64) {
        let frames: Vec<(u64, DecodedPicture)> = match self.decoder.as_mut() {
            Some(VideoDecoder::Hardware(d)) => {
                let out = d.flush();
                out.into_iter().filter_map(|f| self.picture_from_hardware(f)).collect()
            }
            Some(VideoDecoder::HardwareHevc(d)) => {
                let out = d.flush();
                out.into_iter().filter_map(|f| self.picture_from_hardware(f)).collect()
            }
            Some(VideoDecoder::HardwareAv1(d)) => {
                let out = d.flush();
                out.into_iter().filter_map(|f| self.picture_from_hardware(f)).collect()
            }
            Some(VideoDecoder::Av1(d)) => {
                let out = d.flush();
                out.iter().filter_map(|p| Some((self.av1_index(p)?, self.picture_from_av1(p)))).collect()
            }
            Some(VideoDecoder::H264(d)) => {
                let out = d.flush();
                out.iter().filter_map(|f| Some((self.h264_index(f)?, self.picture_from_h264(f)))).collect()
            }
            Some(VideoDecoder::Hevc(d)) => {
                let out = d.flush();
                let mut v = Vec::with_capacity(out.len());
                for f in &out {
                    if let Some(i) = self.next_hevc_frame() {
                        v.push((i, self.picture_from_hevc(f)));
                    }
                }
                v
            }
            None => Vec::new(),
        };
        for (idx, pic) in frames {
            if idx == target {
                *emitted_target = true;
            }
            emit(idx, Arc::new(pic));
        }
        // The decoder has been drained; the next request restarts it (a
        // hardware session stays, emptied, for the next run).
        if !matches!(self.decoder, Some(VideoDecoder::Hardware(_) | VideoDecoder::HardwareHevc(_) | VideoDecoder::HardwareAv1(_))) {
            self.decoder = None;
        }
    }
}

impl MediaSource for Mp4Source {
    fn info(&self) -> SourceInfo {
        self.info.clone()
    }

    fn frame_index(&self, time: Micros) -> u64 {
        self.index_for_secs(time.max(0) as f64 / SECOND as f64)
    }

    fn nearest_sync(&self, index: u64) -> u64 {
        let Some(&t) = self.pres_times.get(index as usize) else { return index };
        match self.track.sample_for_time(t) {
            Some(seek) => match self.frame_of_sample.get(seek.sync_index) {
                Some(&f) if f != u64::MAX && f <= index => f,
                _ => index,
            },
            None => index,
        }
    }

    fn decode_to(&mut self, index: u64, emit: &mut dyn FnMut(u64, PictureRef)) -> bool {
        if index as usize >= self.pres_order.len() {
            return false;
        }
        let Some(seek) = self.track.sample_for_time(self.pres_times[index as usize]) else { return false };
        // Decode forward from where we are when the target is ahead but not
        // too far, and the sync point we would seek to is already behind
        // us; otherwise restart at the sync sample.
        let continuing = self.decoder.is_some()
            && seek.sync_index < self.next_sample
            && !self.emitted.contains(&index)
            && index >= self.next_out
            && index - self.next_out <= self.max_run_ahead;
        if !continuing && !self.restart_at(seek.sync_index) {
            return false;
        }
        let mut done = false;
        // Decode until the target comes out. Samples after the target in
        // decode order may be needed (B-frame reordering), so keep going a
        // little past it, and flush at the end of the stream.
        let limit = self.track.samples.len();
        let mut fed = 0usize;
        while !done {
            if self.abort.as_ref().is_some_and(|(g, mine)| g.load(Ordering::Relaxed) != *mine) {
                break;
            }
            if self.next_sample >= limit {
                self.flush(emit, &mut done, index);
                break;
            }
            if !self.step(emit, &mut done, index) {
                break;
            }
            fed += 1;
            if fed > 4096 {
                break;
            }
        }
        done
    }
}

#[cfg(test)]
#[path = "../../../media/encode/tests/data/svtav1_160x96_15f.rs"]
mod av1_stream;

#[cfg(test)]
mod tests {
    use super::*;

    /// Every frame of 30, 29.97, 25, 24 and 60 fps footage is shown at the
    /// whole-microsecond time the viewer and export ask for it, rounded
    /// either way: no repeats, no skips.
    #[test]
    fn frames_are_found_at_whole_microsecond_times() {
        for fps in [30.0, 30000.0 / 1001.0, 25.0, 24.0, 24000.0 / 1001.0, 60.0, 120.0] {
            let pres: Vec<f64> = (0..600).map(|k| k as f64 / fps).collect();
            for k in 0..600u64 {
                let exact = k as f64 * 1e6 / fps;
                for t in [exact.floor(), exact.round(), exact.ceil()] {
                    assert_eq!(index_at(&pres, t / 1e6), k, "{fps} fps frame {k} at {t} µs");
                }
                // Just before the next frame is still this one.
                let before_next = ((k + 1) as f64 * 1e6 / fps).floor() - 10.0;
                assert_eq!(index_at(&pres, before_next / 1e6), k);
            }
        }
    }

    fn pic(bytes: usize) -> PictureRef {
        Arc::new(DecodedPicture::from_rgba8(bytes as u32 / 4, 1, vec![0; bytes]))
    }

    /// A JPEG photo opens as a still with its colours (it drew black).
    #[test]
    fn jpeg_photos_open_as_stills() {
        const RED_16X8: &[u8] = &[0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, 0x49, 0x46, 0x00, 0x01, 0x02, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0xff, 0xfe, 0x00, 0x0f, 0x4c, 0x61, 0x76, 0x63, 0x36, 0x33, 0x2e, 0x31, 0x2e, 0x31, 0x30, 0x31, 0x00, 0xff, 0xdb, 0x00, 0x43, 0x00, 0x08, 0x04, 0x04, 0x04, 0x04, 0x04, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x07, 0x07, 0x07, 0x08, 0x08, 0x08, 0x07, 0x07, 0x07, 0x06, 0x06, 0x07, 0x07, 0x08, 0x08, 0x08, 0x08, 0x09, 0x09, 0x09, 0x08, 0x08, 0x08, 0x08, 0x09, 0x09, 0x0a, 0x0a, 0x0a, 0x0c, 0x0c, 0x0b, 0x0b, 0x0e, 0x0e, 0x0e, 0x11, 0x11, 0x14, 0xff, 0xc4, 0x00, 0x4c, 0x00, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x07, 0x10, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x11, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0xc0, 0x00, 0x11, 0x08, 0x00, 0x08, 0x00, 0x10, 0x03, 0x01, 0x22, 0x00, 0x02, 0x11, 0x00, 0x03, 0x11, 0x00, 0xff, 0xda, 0x00, 0x0c, 0x03, 0x01, 0x00, 0x02, 0x11, 0x03, 0x11, 0x00, 0x3f, 0x00, 0x8b, 0x00, 0x4d, 0x7f, 0x7f, 0xff, 0xd9];
        let path = std::env::temp_dir().join(format!("studio-red-{}.jpg", std::process::id()));
        std::fs::write(&path, RED_16X8).unwrap();
        let mut src = open_path(&path).unwrap();
        assert_eq!((src.info().width, src.info().height), (16, 8));
        let mut got = None;
        assert!(src.decode_to(0, &mut |_, p| got = Some(p)));
        let PixelData::Rgba8(px) = &got.unwrap().data else { panic!("rgba") };
        assert!(px[0] > 200 && px[1] < 60 && px[2] < 60, "red, not black: {:?}", &px[..4]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn cache_evicts_least_recently_used() {
        let c = DecodeCache::new(1000);
        let m = MediaKey(1);
        c.insert(m, 0, pic(400));
        c.insert(m, 1, pic(400));
        assert!(c.get(m, 0).is_some()); // 0 is now newer than 1
        c.insert(m, 2, pic(400)); // over budget: evict 1
        assert!(c.get(m, 1).is_none());
        assert!(c.get(m, 0).is_some());
        assert!(c.get(m, 2).is_some());
        let s = c.stats();
        assert_eq!(s.pictures, 2);
        assert_eq!(s.bytes, 800);
        assert!(s.bytes <= s.budget);
    }

    #[test]
    fn cache_keeps_oversized_single_picture() {
        let c = DecodeCache::new(100);
        c.insert(MediaKey(1), 0, pic(400));
        assert!(c.get(MediaKey(1), 0).is_some());
        c.insert(MediaKey(1), 1, pic(400));
        assert!(c.get(MediaKey(1), 0).is_none());
        assert!(c.get(MediaKey(1), 1).is_some());
    }

    #[test]
    fn cache_forget_and_budget() {
        let c = DecodeCache::new(10_000);
        for i in 0..10 {
            c.insert(MediaKey(1), i, pic(400));
            c.insert(MediaKey(2), i, pic(400));
        }
        c.forget(MediaKey(1));
        assert_eq!(c.stats().pictures, 10);
        c.set_budget(1000);
        assert!(c.stats().pictures <= 2);
    }

    #[test]
    fn procedural_source_through_sources() {
        let reg = SourceRegistry::new();
        let src = ProceduralSource::test_pattern(64, 32, 30.0, 10);
        reg.register(MediaKey(7), SourceSpec::Procedural(src));
        let cache = DecodeCache::new(1 << 20);
        let mut s = Sources::new(reg, cache.clone());
        let p = s.picture(MediaKey(7), 100_000).unwrap();
        assert_eq!((p.width, p.height), (64, 32));
        let p2 = s.picture(MediaKey(7), 100_000).unwrap();
        assert!(Arc::ptr_eq(&p, &p2));
        assert_eq!(cache.stats().hits, 1);
        // Past the end clamps to the last frame.
        assert!(s.picture(MediaKey(7), 10 * SECOND).is_some());
        assert_eq!(s.info(MediaKey(7)).unwrap().frame_count, Some(10));
    }

    /// The test stream as MP4 samples: temporal units without delimiters.
    fn av1_units() -> Vec<Vec<u8>> {
        let stream = super::av1_stream::STREAM;
        let mut units: Vec<Vec<u8>> = Vec::new();
        let mut pos = 0;
        while pos < stream.len() {
            let h = stream[pos];
            let hdr = 1 + (h & 4 != 0) as usize;
            let (mut size, mut n) = (0usize, 0);
            for i in 0..8 {
                let b = stream[pos + hdr + i];
                size |= ((b & 0x7f) as usize) << (i * 7);
                n += 1;
                if b & 0x80 == 0 {
                    break;
                }
            }
            let end = pos + hdr + n + size;
            match (h >> 3) & 0xf {
                2 => units.push(Vec::new()),
                15 => {}
                _ => units.last_mut().expect("a temporal delimiter first").extend_from_slice(&stream[pos..end]),
            }
            pos = end;
        }
        units
    }

    fn av1_mp4(name: &str) -> std::path::PathBuf {
        use studio_encode::mux::{Muxer, Options, av1c_from_sequence_header};
        let dir = std::env::temp_dir().join(format!("studio-render-av1-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("clip.mp4");
        let units = av1_units();
        assert_eq!(units.len(), 15);
        let info = av1c_from_sequence_header(&units[0][..2 + units[0][1] as usize]).unwrap();
        let mut mux = Muxer::create(&path, Options::mp4_av1()).unwrap();
        let track = mux.add_av1_video(info.track_info(30, 1, 200_000)).unwrap();
        for u in &units {
            mux.write_temporal_unit(track, u).unwrap();
        }
        mux.finish().unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() < super::av1_stream::STREAM.len() as u64 + 4096);
        path
    }

    /// All 15 frames in order, plus frame 12 again after a seek back.
    fn decode_all(src: &mut Mp4Source, path_in_use: fn(&VideoDecoder) -> bool) -> Vec<PictureRef> {
        let mut frames: Vec<Option<PictureRef>> = vec![None; 15];
        for i in 0..15u64 {
            src.decode_to(i, &mut |k, p| frames[k as usize] = Some(p));
            if i == 0 {
                assert!(src.decoder.as_ref().is_some_and(path_in_use), "the expected decode path is in use");
            }
        }
        let mut again = None;
        src.decode_to(0, &mut |_, _| {});
        src.decode_to(12, &mut |k, p| {
            if k == 12 {
                again = Some(p);
            }
        });
        let frames: Vec<PictureRef> = frames.into_iter().enumerate().map(|(i, f)| f.unwrap_or_else(|| panic!("frame {i} missing"))).collect();
        let (a, b) = (&frames[12], again.expect("frame 12 after a seek"));
        assert!(matches!((&a.data, &b.data), (PixelData::Yuv { y: Planes::U8(x), .. }, PixelData::Yuv { y: Planes::U8(z), .. }) if *x == *z), "a seek gives the same picture");
        frames
    }

    fn luma(p: &DecodedPicture) -> &[u8] {
        match &p.data {
            PixelData::Yuv { y: Planes::U8(y), .. } => y,
            _ => panic!("8-bit planes expected"),
        }
    }

    #[test]
    fn mp4_source_plays_av1_on_both_paths() {
        let path = av1_mp4("both");
        let mut sw = Mp4Source::open(&path).unwrap();
        assert_eq!((sw.info().width, sw.info().height, sw.info().frame_count), (160, 96, Some(15)));
        sw.set_software_only(true);
        let soft = decode_all(&mut sw, |d| matches!(d, VideoDecoder::Av1(_)));
        for f in &soft {
            assert_eq!((f.width, f.height), (160, 96));
            assert_eq!(luma(f).len(), 160 * 96);
        }
        assert_ne!(luma(&soft[0]), luma(&soft[14]), "the pictures differ from frame to frame");
        if !studio_hw::Av1Decoder::supported() {
            eprintln!("no AV1 hardware here; hardware path skipped");
            return;
        }
        let mut hw = Mp4Source::open(&path).unwrap();
        let hard = decode_all(&mut hw, |d| matches!(d, VideoDecoder::HardwareAv1(_)));
        let mut exact = true;
        for (h, s) in hard.iter().zip(&soft) {
            assert_eq!((h.width, h.height), (s.width, s.height));
            exact &= luma(h) == luma(s);
        }
        assert!(exact, "hardware and software frames match");
    }

    /// Decode speed, headless: frames in order, as playback asks for them.
    /// `STUDIO_DECODE_FILES=a.mp4,b.mp4 cargo test --release -p studio-render -- --ignored decode_speed --nocapture`
    /// (`STUDIO_SOFTWARE_DECODE=1` for Studio's own decoders).
    #[test]
    #[ignore]
    fn decode_speed() {
        let Ok(files) = std::env::var("STUDIO_DECODE_FILES") else { return };
        let path_of = if std::env::var_os("STUDIO_SOFTWARE_DECODE").is_some() { "software" } else { "hardware first" };
        for f in files.split(',') {
            let mut src = Mp4Source::open(Path::new(f)).unwrap();
            let n = 150u64.min(src.info.frame_count.unwrap_or(150));
            let mut worst = 0f64;
            let t0 = Instant::now();
            // Pictures already handed out stand for the decode cache.
            let mut have = std::collections::HashSet::new();
            for i in 0..n {
                let t = Instant::now();
                if have.contains(&i) {
                    continue;
                }
                let mut got = Vec::new();
                let ok = src.decode_to(i, &mut |k, _| got.push(k));
                have.extend(got.iter().copied());
                if std::env::var_os("STUDIO_DECODE_TIMES").is_some() && i < 60 {
                    eprintln!("TIME {i} {:.1}", t.elapsed().as_secs_f64() * 1000.0);
                }
                if std::env::var_os("STUDIO_DECODE_TRACE").is_some() && i < 8 {
                    let which = match &src.decoder {
                        Some(VideoDecoder::Hardware(_)) => "hardware h264",
                        Some(VideoDecoder::HardwareHevc(_)) => "hardware hevc",
                        Some(VideoDecoder::H264(_)) => "software h264",
                        Some(VideoDecoder::Hevc(_)) => "software hevc",
                        Some(VideoDecoder::HardwareAv1(_)) => "hardware av1",
                        Some(VideoDecoder::Av1(_)) => "software av1",
                        None => "none",
                    };
                    eprintln!("ask {i}: emitted {got:?} ok={ok} next_sample={} decoder={which} took={:?}", src.next_sample, t.elapsed());
                }
                assert!(ok, "frame {i}");
                // A frame asked for again (the cache lost it) comes back
                // without decoding the rest of the file.
                if i == 2 {
                    let t2 = Instant::now();
                    let mut again = Vec::new();
                    assert!(src.decode_to(1, &mut |k, _| again.push(k)), "frame 1 again");
                    assert!(again.contains(&1) && again.len() < 30, "re-decoded only its group: {again:?} in {:?}", t2.elapsed());
                }
                worst = worst.max(t.elapsed().as_secs_f64() * 1000.0);
            }
            let ms = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
            // Replay from the start (a seek back), as after pressing Play
            // again: the first frames must not be slow.
            let mut have = std::collections::HashSet::new();
            let t1 = Instant::now();
            for i in 0..30u64.min(n) {
                if !have.contains(&i) {
                    assert!(src.decode_to(i, &mut |k, _| {
                        have.insert(k);
                    }));
                }
            }
            let replay = t1.elapsed().as_secs_f64() * 1000.0;
            eprintln!("  replay: first 30 frames in {replay:.0} ms ({:.1} ms/frame)", replay / 30.0);
            let name = Path::new(f).file_name().unwrap().to_string_lossy();
            eprintln!("{path_of:15} {name:22} {}x{}  {ms:6.1} ms/frame ({:5.0} fps)  worst {worst:5.0} ms", src.info.width, src.info.height, 1000.0 / ms);
        }
    }

    /// Real-time playback the way the preview does it, headless: each frame
    /// asked for on a 30 fps clock with the preview's wait budget; counts
    /// stand-in pictures (a stutter each).
    /// `STUDIO_DECODE_FILES=a.mp4,b.mp4 cargo test --release -p studio-render -- --ignored playback_in_real_time --nocapture`
    #[test]
    #[ignore]
    fn playback_in_real_time() {
        let Ok(files) = std::env::var("STUDIO_DECODE_FILES") else { return };
        for f in files.split(',') {
            let reg = SourceRegistry::new();
            reg.register_path(MediaKey(1), f);
            let mut s = Sources::new(reg, DecodeCache::new(1 << 30));
            set_wait_budget(Some(Duration::from_millis(40)));
            let frames = 300u64;
            let (mut stand_ins, mut late, mut worst) = (0, 0, 0f64);
            // Paused on the first frame first, as before pressing Play.
            let _ = s.picture(MediaKey(1), 0);
            let _ = take_provisional();
            let start = Instant::now();
            for i in 0..frames {
                let due = start + Duration::from_micros(i * 33_333);
                if let Some(w) = due.checked_duration_since(Instant::now()) {
                    std::thread::sleep(w);
                }
                let t = Instant::now();
                let _ = s.picture(MediaKey(1), (i * 33_333) as Micros);
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                worst = worst.max(ms);
                if take_provisional() {
                    stand_ins += 1;
                    if std::env::var_os("STUDIO_DECODE_TRACE").is_some() {
                        eprintln!("STANDIN {i}");
                    }
                }
                if ms > 33.3 {
                    late += 1;
                }
            }
            // Replay: back to the start and play 3 s again.
            let mut replay = 0;
            let start = Instant::now();
            for i in 0..90u64 {
                let due = start + Duration::from_micros(i * 33_333);
                if let Some(w) = due.checked_duration_since(Instant::now()) {
                    std::thread::sleep(w);
                }
                let _ = s.picture(MediaKey(1), (i * 33_333) as Micros);
                if take_provisional() {
                    replay += 1;
                }
            }
            let name = Path::new(f).file_name().unwrap().to_string_lossy();
            eprintln!("RESULT {name:22} play {frames} frames: stand-ins {stand_ins}, late {late}, worst {worst:.0} ms | replay 90: stand-ins {replay}");
        }
    }

    #[test]
    fn missing_media_is_none() {
        let reg = SourceRegistry::new();
        reg.register_path(MediaKey(1), "/nonexistent/file.mp4");
        let mut s = Sources::new(reg, DecodeCache::new(1 << 20));
        assert!(s.picture(MediaKey(1), 0).is_none());
        assert!(s.failure(MediaKey(1)).is_some());
        assert!(s.picture(MediaKey(2), 0).is_none());
    }
}

// ---------------------------------------------------------------- decode ahead

/// Frames decoded past the last requested one while nothing newer is asked.
const DECODE_AHEAD: u64 = 8;

struct AheadState {
    target: Option<u64>,
    quit: bool,
    /// The last frame asked for that the decoder could not produce (a
    /// damaged or unsupported stream): the request fails instead of asking
    /// again forever.
    failed: Option<u64>,
}

struct AheadShared {
    state: Mutex<AheadState>,
    wake: Condvar,
    /// Bumped by every request; the decoder stops work for older ones.
    generation: Arc<AtomicU64>,
    /// A picture went into the cache (waiters re-check).
    decoded: Condvar,
    dead: AtomicBool,
}

/// A video file decoded on its own thread: the newest requested frame
/// first, then a few frames ahead of it, all into the shared cache, so
/// rendering picks up ready pictures instead of decoding in line. A newer
/// request interrupts work for an older one. Requests wait at most the
/// thread's budget ([`set_wait_budget`]); past it, the nearest decoded
/// picture stands in and the renderer draws again when the exact one is in.
pub struct AheadSource {
    info: SourceInfo,
    pres_times: Arc<Vec<f64>>,
    /// Frame index of the sync picture each frame decodes from.
    sync_of: Arc<Vec<u64>>,
    key: MediaKey,
    cache: Arc<DecodeCache>,
    shared: Arc<AheadShared>,
}

impl AheadSource {
    fn start(mut src: Mp4Source, key: MediaKey, cache: Arc<DecodeCache>) -> AheadSource {
        // Sync frame of every frame, from the decode order.
        let mut sync_of = vec![0u64; src.pres_order.len()];
        let mut last_sync = 0usize;
        let mut sync_sample_of = vec![0usize; src.track.samples.len()];
        for (i, smp) in src.track.samples.iter().enumerate() {
            if smp.sync {
                last_sync = i;
            }
            sync_sample_of[i] = last_sync;
        }
        for (k, &smp) in src.pres_order.iter().enumerate() {
            let f = src.frame_of_sample.get(sync_sample_of[smp]).copied().unwrap_or(u64::MAX);
            sync_of[k] = if f == u64::MAX || f > k as u64 { k as u64 } else { f };
        }
        let shared = Arc::new(AheadShared {
            state: Mutex::new(AheadState { target: None, quit: false, failed: None }),
            wake: Condvar::new(),
            generation: Arc::new(AtomicU64::new(0)),
            decoded: Condvar::new(),
            dead: AtomicBool::new(false),
        });
        let info = src.info.clone();
        let pres_times = Arc::new(src.pres_times.clone());
        let frames = src.pres_order.len() as u64;
        let worker_shared = shared.clone();
        let worker_cache = cache.clone();
        let spawned = std::thread::Builder::new().name("studio-decode".into()).spawn(move || {
            let sh = worker_shared;
            loop {
                let target = {
                    let mut g = sh.state.lock().unwrap_or_else(|e| e.into_inner());
                    loop {
                        if g.quit {
                            return;
                        }
                        if let Some(t) = g.target.take() {
                            break t;
                        }
                        g = sh.wake.wait(g).unwrap_or_else(|e| e.into_inner());
                    }
                };
                let generation = sh.generation.load(Ordering::Relaxed);
                src.abort = Some((sh.generation.clone(), generation));
                let mut emit = |i: u64, pic: PictureRef| {
                    worker_cache.insert(key, i, pic);
                    sh.decoded.notify_all();
                };
                let decode = |src: &mut Mp4Source, k: u64, emit: &mut dyn FnMut(u64, PictureRef)| {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| src.decode_to(k, emit))).unwrap_or(false)
                };
                if !worker_cache.contains(key, target) && !decode(&mut src, target, &mut emit) && !worker_cache.contains(key, target) {
                    sh.state.lock().unwrap_or_else(|e| e.into_inner()).failed = Some(target);
                    sh.decoded.notify_all();
                    continue;
                }
                // Run ahead while nobody asks for something else.
                let mut k = target + 1;
                while k < frames && k <= target + DECODE_AHEAD && sh.generation.load(Ordering::Relaxed) == generation {
                    if !worker_cache.contains(key, k) && !decode(&mut src, k, &mut emit) {
                        break;
                    }
                    k += 1;
                }
            }
        });
        if spawned.is_err() {
            shared.dead.store(true, Ordering::Relaxed);
        }
        AheadSource { info, pres_times, sync_of: Arc::new(sync_of), key, cache, shared }
    }
}

impl Drop for AheadSource {
    fn drop(&mut self) {
        self.shared.state.lock().unwrap_or_else(|e| e.into_inner()).quit = true;
        self.shared.wake.notify_all();
    }
}

impl MediaSource for AheadSource {
    fn info(&self) -> SourceInfo {
        self.info.clone()
    }

    fn frame_index(&self, time: Micros) -> u64 {
        let secs = time.max(0) as f64 / SECOND as f64;
        let n = self.pres_times.len();
        if n == 0 {
            return 0;
        }
        index_at(&self.pres_times, secs)
    }

    fn nearest_sync(&self, index: u64) -> u64 {
        self.sync_of.get(index as usize).copied().unwrap_or(index)
    }

    fn decode_to(&mut self, index: u64, emit: &mut dyn FnMut(u64, PictureRef)) -> bool {
        if index as usize >= self.pres_times.len() || self.shared.dead.load(Ordering::Relaxed) {
            return false;
        }
        if let Some(p) = self.cache.get(self.key, index) {
            emit(index, p);
            self.ask_ahead(index);
            return true;
        }
        self.shared.generation.fetch_add(1, Ordering::Relaxed);
        {
            let mut g = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
            g.target = Some(index);
            g.failed = None;
        }
        self.shared.wake.notify_all();
        let budget = WAIT_BUDGET.with(|b| b.get());
        let deadline = budget.map(|b| Instant::now() + b);
        let mut g = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(p) = self.cache.get(self.key, index) {
                drop(g);
                emit(index, p);
                return true;
            }
            if g.failed == Some(index) {
                return false;
            }
            let wait = match deadline {
                Some(d) => match d.checked_duration_since(Instant::now()) {
                    Some(w) if !w.is_zero() => w,
                    _ => return false,
                },
                // Exact frames (export): wait in short slices so a decode
                // that cannot produce the frame does not hang forever.
                None => Duration::from_millis(50),
            };
            let (ng, timeout) = self.shared.decoded.wait_timeout(g, wait).unwrap_or_else(|e| e.into_inner());
            g = ng;
            if timeout.timed_out() && deadline.is_none() && g.target.is_none() && !self.cache.contains(self.key, index) {
                // The decoder finished its work without this frame: give
                // it one more nudge, then report failure.
                g.target = Some(index);
                self.shared.wake.notify_all();
                let (ng, t2) = self.shared.decoded.wait_timeout(g, Duration::from_millis(2000)).unwrap_or_else(|e| e.into_inner());
                g = ng;
                if t2.timed_out() && !self.cache.contains(self.key, index) {
                    return false;
                }
            }
        }
    }
}

impl AheadSource {
    /// A cached frame was used: keep the decoder running ahead of it.
    fn ask_ahead(&self, index: u64) {
        let next = index + 1;
        if (next as usize) < self.pres_times.len() && !self.cache.contains(self.key, (index + DECODE_AHEAD / 2).min(self.pres_times.len() as u64 - 1)) {
            let mut g = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
            if g.target.is_none() {
                g.target = Some(next);
                drop(g);
                self.shared.wake.notify_all();
            }
        }
    }
}
