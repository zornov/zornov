// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Work done once and reused: the processed picture of a layer is kept
//! while nothing that shapes it changes, the way a game draws a sprite
//! once and stamps it many times.
//!
//! A layer's *content* is everything up to its placement: the source
//! picture (a decoded frame or a generated text, shape, gradient or
//! solid), crop, effects, grade, masks and pins. None of that depends on
//! where the layer sits, how large it is drawn or how its motion preset
//! moves it, so a still, a title or a card that only moves is processed
//! on the first frame and placed from the cache on every frame after. The
//! same key also matches identical copies (twelve copies of one sticker
//! are processed once) and the minified versions drawn for small copies
//! are kept with it.
//!
//! The key is the full description of the content, compared exactly (no
//! hash collisions can show a wrong picture). Media pictures are told
//! apart by identity: the decode cache hands out the same shared picture
//! for the same frame, so a still keeps its key and each new video frame
//! gets a new one. Effects that read the clip time or seed (see
//! `studio_fx::effects::EffectDef::animated`) and grain put the time in
//! the key, so they render every frame.
//!
//! Memory is bounded: entries are counted in bytes against a budget (and
//! all caches in the process against a shared cap, see
//! [`set_total_budget`]) and
//! the least recently used go first. An entry nobody asked for again
//! within a couple of frames (a video frame, a scrubbed-past title) is
//! dropped early, so passing pictures never crowd out the ones that stay.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Weak};

use crate::effects;
use crate::mask::MaskSpec;
use crate::puppet::WarpDesc;
use crate::scene::{Canvas, ClipDesc, EffectDesc, GeneratorDesc, GradeDesc, Micros, SECOND, SourceDesc};
use crate::source::Sources;
use crate::yuv::{DecodedPicture, PictureRef};

/// Bytes held by every layer cache in the process (the viewer's, each
/// export worker's), and the most they may hold together.
static TOTAL: AtomicUsize = AtomicUsize::new(0);
static TOTAL_LIMIT: AtomicUsize = AtomicUsize::new(1 << 30);

/// Caps the bytes all layer caches together keep (1 GiB by default); each
/// cache also has its own budget.
pub fn set_total_budget(bytes: usize) {
    TOTAL_LIMIT.store(bytes, Ordering::Relaxed);
}

/// Whether `bytes` more still fit under the process-wide cap.
fn fits_total(bytes: usize) -> bool {
    TOTAL.load(Ordering::Relaxed) + bytes <= TOTAL_LIMIT.load(Ordering::Relaxed)
}

/// Where a layer's pixels come from, as far as reuse is concerned.
#[derive(Clone, Debug)]
pub enum SourceKey {
    /// A decoded picture (by identity), whether it is tone mapped, and
    /// the rotation applied to it.
    Picture { picture: Weak<DecodedPicture>, to_sdr: bool, rotation: u32 },
    /// A generated picture at a pixel size, drawn by the text renderer of
    /// a generation (see [`crate::text::text_generation`]).
    Generator { desc: GeneratorDesc, size: (u32, u32), text: u64 },
    /// Models drawn at a pixel size.
    Mesh { desc: crate::mesh_render::MeshDesc, size: (u32, u32) },
    /// A world seen through a camera at a pixel size.
    World { desc: crate::world_scene::WorldDesc, size: (u32, u32) },
}

impl PartialEq for SourceKey {
    fn eq(&self, other: &SourceKey) -> bool {
        match (self, other) {
            // A live weak reference pins the allocation, so equal pointers
            // are the same picture.
            (SourceKey::Picture { picture: a, to_sdr: sa, rotation: ra }, SourceKey::Picture { picture: b, to_sdr: sb, rotation: rb }) => {
                a.ptr_eq(b) && sa == sb && ra == rb && a.strong_count() > 0
            }
            (SourceKey::Generator { desc: a, size: sa, text: ta }, SourceKey::Generator { desc: b, size: sb, text: tb }) => sa == sb && ta == tb && a == b,
            (SourceKey::Mesh { desc: a, size: sa }, SourceKey::Mesh { desc: b, size: sb }) => sa == sb && a == b,
            (SourceKey::World { desc: a, size: sa }, SourceKey::World { desc: b, size: sb }) => sa == sb && a == b,
            _ => false,
        }
    }
}

/// Everything that decides a layer's content.
#[derive(Clone, Debug, PartialEq)]
pub struct ContentKey {
    pub source: SourceKey,
    pub crop: [f32; 4],
    pub effects: Vec<EffectDesc>,
    /// Clip time (bits of the seconds), frame rate and seed, only when an
    /// effect reads them.
    pub clock: Option<(u32, u32, u32)>,
    pub grade: Option<GradeDesc>,
    /// Frame number, only when the grade has grain.
    pub grade_frame: Option<u64>,
    pub grade_nodes: Vec<crate::nodes::GradeNodeDesc>,
    pub masks: Vec<MaskSpec>,
    pub warp: Option<WarpDesc>,
    pub matte: Option<crate::scene::MatteDesc>,
}

impl ContentKey {
    /// The key of `clip`'s content at timeline time `t`, drawn at
    /// `target` pixels, with the picture of a media clip from
    /// [`media_picture`]; `None` when it is not kept (nested scenes,
    /// adjustments, unavailable media).
    pub fn of(clip: &ClipDesc, canvas: &Canvas, target: (u32, u32), t: Micros, picture: Option<&(PictureRef, u32)>) -> Option<ContentKey> {
        let source = match &clip.source {
            SourceDesc::Media { .. } => {
                let (picture, rotation) = picture?;
                SourceKey::Picture { picture: Arc::downgrade(picture), to_sdr: !canvas.output.is_hdr(), rotation: *rotation }
            }
            SourceDesc::Generator(g) => SourceKey::Generator { desc: g.clone(), size: target, text: crate::text::text_generation() },
            SourceDesc::Mesh(m) => SourceKey::Mesh { desc: m.clone(), size: target },
            SourceDesc::World(w) => SourceKey::World { desc: w.clone(), size: target },
            _ => return None,
        };
        let animated = clip.effects.iter().any(|e| effect_is_animated(&e.kind) || comp_is_animated(e));
        let clock = animated.then(|| (((t - clip.start).max(0) as f32 / SECOND as f32).to_bits(), (canvas.frame_rate as f32).to_bits(), clip.seed));
        let grain = clip.grade.as_ref().into_iter().chain(clip.grade_nodes.iter().map(|n| &n.grade)).any(|g| effects::grade_from_desc(g).basic.grain.amount > 0.0);
        let grade_frame = grain.then(|| (t as f64 / SECOND as f64 * canvas.frame_rate).floor().max(0.0) as u64);
        Some(ContentKey {
            source,
            crop: clip.placement.crop,
            effects: clip.effects.clone(),
            clock,
            grade: clip.grade.clone(),
            grade_frame,
            grade_nodes: clip.grade_nodes.clone(),
            masks: clip.masks.clone(),
            warp: clip.warp.clone(),
            matte: clip.matte,
        })
    }
}

/// A media clip's decoded picture at its source time and the rotation it
/// is shown with; `None` for other clips or unavailable media. Fetched
/// once per layer, so the key and the pixels are of the same picture.
pub fn media_picture(sources: &mut Sources, clip: &ClipDesc) -> Option<(PictureRef, u32)> {
    let SourceDesc::Media { media, time } = &clip.source else { return None };
    let rotation = sources.info(*media)?.rotation;
    Some((sources.picture(*media, *time)?, rotation))
}

/// Source pixels per picture pixel of a media clip: 1, or more when the
/// picture is a proxy smaller than the original (`width` is the picture's
/// width after display rotation).
pub fn media_scale(sources: &mut Sources, clip: &ClipDesc, width: u32) -> f32 {
    let SourceDesc::Media { media, .. } = &clip.source else { return 1.0 };
    let Some(info) = sources.info(*media) else { return 1.0 };
    let (w, _) = info.display_size();
    if w == width || width == 0 { 1.0 } else { w as f32 / width as f32 }
}

/// The matte picture for a media clip's frame at its source time: the
/// matte's frame with the picture's own frame index.
pub fn matte_picture(sources: &mut Sources, clip: &ClipDesc) -> Option<(PictureRef, bool)> {
    let (SourceDesc::Media { media, time }, Some(m)) = (&clip.source, &clip.matte) else { return None };
    let index = sources.frame_index(*media, *time)?;
    Some((sources.picture_at(m.media, index)?, m.invert))
}

/// Whether a `comp` effect's graph changes over time (video reads, keys).
fn comp_is_animated(e: &EffectDesc) -> bool {
    #[cfg(feature = "fx")]
    {
        crate::comp::is_animated(e)
    }
    #[cfg(not(feature = "fx"))]
    {
        let _ = e;
        false
    }
}

/// Whether an effect changes from frame to frame on its own.
pub fn effect_is_animated(kind: &str) -> bool {
    #[cfg(feature = "fx")]
    {
        studio_fx::effects::find(kind).is_some_and(|d| d.animated)
    }
    #[cfg(not(feature = "fx"))]
    {
        let _ = kind;
        false
    }
}

/// A layer's processed picture before placement.
#[derive(Clone)]
pub struct Content<P> {
    pub image: P,
    /// Source pixels per image pixel.
    pub k: f32,
    /// Where the image's top-left sits in crop pixels (pins may grow it).
    pub origin: [f32; 2],
    /// Size the source is treated as having.
    pub source_size: [f32; 2],
}

/// How many bytes a cached picture holds.
pub trait Sized2d {
    fn bytes(&self) -> usize;
}

struct Entry<P> {
    id: u64,
    key: ContentKey,
    content: Content<P>,
    /// Halved versions: `mips[0]` is half size, `mips[1]` a quarter...
    mips: Vec<P>,
    bytes: usize,
    used: u64,
    /// Frame it was made in, and whether it was asked for since.
    born: u64,
    reused: bool,
}

/// Counters for benchmarks and the stats overlay.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReuseStats {
    pub hits: u64,
    pub misses: u64,
    pub entries: usize,
    pub bytes: usize,
}

/// Bounded cache of layer contents (CPU frames or GPU textures).
pub struct LayerCache<P> {
    entries: Vec<Entry<P>>,
    budget: usize,
    bytes: usize,
    clock: u64,
    frame: u64,
    hits: u64,
    misses: u64,
}

/// An entry found or made by [`LayerCache::get`] / [`LayerCache::insert`],
/// for adding minified versions to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntryId(u64);

impl<P: Clone + Sized2d> LayerCache<P> {
    pub fn new(budget: usize) -> LayerCache<P> {
        LayerCache { entries: Vec::new(), budget, bytes: 0, clock: 0, frame: 0, hits: 0, misses: 0 }
    }

    /// Starts a frame. Returns pictures dropped because nobody asked for
    /// them again (the GPU puts their textures back in its pool).
    pub fn begin_frame(&mut self) -> Vec<P> {
        self.frame += 1;
        let frame = self.frame;
        self.evict_where(|e| !e.reused && e.born + 2 <= frame)
    }

    /// The content kept for `key`.
    pub fn get(&mut self, key: &ContentKey) -> Option<(Content<P>, EntryId)> {
        self.clock += 1;
        let Some(e) = self.entries.iter_mut().find(|e| e.key == *key) else {
            self.misses += 1;
            return None;
        };
        e.used = self.clock;
        e.reused = true;
        self.hits += 1;
        Some((e.content.clone(), EntryId(e.id)))
    }

    /// Keeps `content` for `key`. Returns pictures evicted to stay within
    /// budget, and the new entry (none when it alone is over budget).
    pub fn insert(&mut self, key: ContentKey, content: Content<P>) -> (Vec<P>, Option<EntryId>) {
        let bytes = content.image.bytes();
        if bytes > self.budget / 2 {
            return (Vec::new(), None);
        }
        let mut out = self.evict_where(|e| e.key == key);
        out.extend(self.make_room(bytes));
        if !fits_total(bytes) {
            return (out, None);
        }
        self.clock += 1;
        self.grow(bytes);
        let id = self.clock;
        self.entries.push(Entry { id, key, content, mips: Vec::new(), bytes, used: self.clock, born: self.frame, reused: false });
        (out, Some(EntryId(id)))
    }

    /// The `level`-times halved picture of an entry, if kept.
    pub fn mip(&self, id: EntryId, level: usize) -> Option<P> {
        let e = self.entries.iter().find(|e| e.id == id.0)?;
        e.mips.get(level.checked_sub(1)?).cloned()
    }

    /// Keeps the `level`-times halved picture of an entry (levels are
    /// added in order). Returns evicted pictures, or `None` when it is not
    /// kept.
    pub fn add_mip(&mut self, id: EntryId, level: usize, mip: P) -> Option<Vec<P>> {
        let bytes = mip.bytes();
        let e = self.entries.iter_mut().find(|e| e.id == id.0 && e.mips.len() + 1 == level)?;
        // Recently used, so it goes after older entries while making room
        // (a minified picture alone does not make it reused).
        self.clock += 1;
        e.used = self.clock;
        let out = self.make_room(bytes);
        if !fits_total(bytes) {
            return None;
        }
        let e = self.entries.iter_mut().find(|e| e.id == id.0)?;
        e.mips.push(mip);
        e.bytes += bytes;
        self.grow(bytes);
        Some(out)
    }

    /// Changes the budget, evicting as needed.
    pub fn set_budget(&mut self, budget: usize) -> Vec<P> {
        self.budget = budget;
        self.make_room(0)
    }

    /// Drops everything.
    pub fn clear(&mut self) -> Vec<P> {
        self.evict_where(|_| true)
    }

    pub fn stats(&self) -> ReuseStats {
        ReuseStats { hits: self.hits, misses: self.misses, entries: self.entries.len(), bytes: self.bytes }
    }

    fn grow(&mut self, bytes: usize) {
        self.bytes += bytes;
        TOTAL.fetch_add(bytes, Ordering::Relaxed);
    }

    fn shrink(&mut self, bytes: usize) {
        self.bytes -= bytes;
        TOTAL.fetch_sub(bytes, Ordering::Relaxed);
    }

    fn make_room(&mut self, bytes: usize) -> Vec<P> {
        let mut out = Vec::new();
        // Pictures whose media is gone can never match again.
        out.extend(self.evict_where(|e| matches!(&e.key.source, SourceKey::Picture { picture, .. } if picture.strong_count() == 0)));
        while (self.bytes + bytes > self.budget || !fits_total(bytes)) && !self.entries.is_empty() {
            // Never-reused entries first, then the least recently used.
            let i = (0..self.entries.len()).min_by_key(|&i| (self.entries[i].reused, self.entries[i].used)).unwrap();
            let e = self.entries.swap_remove(i);
            self.shrink(e.bytes);
            out.push(e.content.image);
            out.extend(e.mips);
        }
        out
    }

    fn evict_where(&mut self, mut f: impl FnMut(&Entry<P>) -> bool) -> Vec<P> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.entries.len() {
            if f(&self.entries[i]) {
                let e = self.entries.swap_remove(i);
                self.shrink(e.bytes);
                out.push(e.content.image);
                out.extend(e.mips);
            } else {
                i += 1;
            }
        }
        out
    }
}

impl<P> Drop for LayerCache<P> {
    fn drop(&mut self) {
        TOTAL.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}

impl Sized2d for Arc<crate::frame::Frame> {
    fn bytes(&self) -> usize {
        self.byte_size()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{GeneratorKind, Params};
    use crate::yuv::PixelData;

    /// A stand-in picture of some bytes.
    #[derive(Clone, Debug, PartialEq)]
    struct Pic(usize);

    impl Sized2d for Pic {
        fn bytes(&self) -> usize {
            self.0
        }
    }

    fn key(n: u32) -> ContentKey {
        let desc = GeneratorDesc { kind: GeneratorKind::Solid, params: Params::new(), text: vec![] };
        ContentKey {
            source: SourceKey::Generator { desc, size: (n, 1), text: 0 },
            crop: [0.0; 4],
            effects: vec![],
            clock: None,
            grade: None,
            grade_frame: None,
            grade_nodes: vec![],
            masks: vec![],
            warp: None,
            matte: None,
        }
    }

    fn content(bytes: usize) -> Content<Pic> {
        Content { image: Pic(bytes), k: 1.0, origin: [0.0; 2], source_size: [1.0; 2] }
    }

    #[test]
    fn least_recently_used_goes_first() {
        let mut c = LayerCache::new(300);
        c.begin_frame();
        c.insert(key(1), content(100));
        c.insert(key(2), content(100));
        c.insert(key(3), content(100));
        assert!(c.get(&key(1)).is_some() && c.get(&key(3)).is_some() && c.get(&key(2)).is_some());
        c.get(&key(1));
        let (gone, _) = c.insert(key(4), content(100));
        assert_eq!(gone, vec![Pic(100)]);
        assert!(c.get(&key(3)).is_none(), "3 was used longest ago");
        assert_eq!(c.stats().bytes, 300);
        // Nothing larger than half the budget is kept.
        assert_eq!(c.insert(key(5), content(200)).1, None);
    }

    #[test]
    fn pictures_never_asked_for_again_leave_early() {
        let mut c = LayerCache::new(1 << 20);
        c.begin_frame();
        c.insert(key(1), content(10));
        // A minified copy alone does not count as asked for again.
        let id = c.insert(key(2), content(10)).1.unwrap();
        assert!(c.add_mip(id, 1, Pic(3)).is_some());
        c.begin_frame();
        assert!(c.get(&key(1)).is_some());
        // Two frames on, the one nobody wanted is gone; the reused one stays.
        assert_eq!(c.begin_frame(), vec![Pic(10), Pic(3)]);
        assert!(c.get(&key(1)).is_some() && c.get(&key(2)).is_none());
    }

    #[test]
    fn minified_pictures_stay_with_their_entry() {
        let mut c = LayerCache::new(900);
        let (_, id) = c.insert(key(1), content(400));
        let id = id.unwrap();
        assert_eq!(c.mip(id, 1), None);
        assert!(c.add_mip(id, 2, Pic(25)).is_none(), "levels go in order");
        assert!(c.add_mip(id, 1, Pic(100)).is_some());
        assert_eq!(c.mip(id, 1), Some(Pic(100)));
        assert_eq!(c.stats().bytes, 500);
        let (gone, _) = c.insert(key(2), content(450));
        assert_eq!(gone, vec![Pic(400), Pic(100)]);
        assert_eq!(c.stats().bytes, 450);
    }

    #[test]
    fn media_pictures_match_by_identity() {
        let pic = |v: u8| Arc::new(DecodedPicture { width: 1, height: 1, colour: Default::default(), data: PixelData::Rgba8(vec![v; 4]) });
        let (a, b) = (pic(1), pic(1));
        let k = |p: &Arc<DecodedPicture>| SourceKey::Picture { picture: Arc::downgrade(p), to_sdr: true, rotation: 0 };
        assert_eq!(k(&a), k(&a));
        assert_ne!(k(&a), k(&b), "equal pixels, different frames");
        let ka = k(&a);
        drop(a);
        assert_ne!(ka, ka.clone(), "a dropped picture never matches");
    }
}
