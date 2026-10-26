// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! "Find the moment where...": the frames of a piece of footage as picture
//! embeddings, searched with the embedding of a sentence. The index is
//! made once per footage (a frame a second or so) and cached by content
//! hash; a search is a few thousand multiplications.

use crate::cache::{CacheKey, Cacheable, Reader, ResultCache, Writer};
use crate::error::Result;
use crate::features::embed::{dot, PictureTextEmbedder, FrameEmbedding, DIM};
use crate::frame::FrameSource;
use crate::jobs::JobCtx;
use std::ops::Range;

pub const VERSION: u32 = 1;

/// One frame's embeddings.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub t: f32,
    pub emb: FrameEmbedding,
}

/// The embeddings of a piece of footage.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct FrameIndex {
    pub entries: Vec<Entry>,
}

/// A place in the footage that matches.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Moment {
    /// Seconds into the footage.
    pub t: f32,
    /// Cosine similarity of the sentence and the frame.
    pub score: f32,
}

impl Moment {
    /// How to read the score (see [`crate::features::embed::match_strength`]).
    pub fn strength(&self) -> &'static str {
        crate::features::embed::match_strength(self.score)
    }
}

/// Seconds between indexed frames for footage `secs` long: one a second,
/// fewer on long footage so the index stays small.
pub fn spacing(secs: f32) -> f32 {
    (secs / 24.0).max(1.0)
}

/// Frame indices (relative to the range) to embed.
pub fn frame_indices(n: usize, fps: f64) -> Vec<usize> {
    if n <= 1 {
        return vec![0];
    }
    let secs = n as f32 / fps as f32;
    let step = (spacing(secs) as f64 * fps).round().max(1.0) as usize;
    let mut v: Vec<usize> = (0..n).step_by(step).map(|i| (i + step / 2).min(n - 1)).collect();
    v.dedup();
    v
}

impl FrameIndex {
    /// Embeds the frames of `range` of `src`.
    pub fn build(src: &mut dyn FrameSource, range: Range<usize>, clip: &PictureTextEmbedder, ctx: &JobCtx) -> Result<FrameIndex> {
        let n = range.len();
        let fps = src.fps().max(1.0);
        let idx = frame_indices(n, fps);
        let mut entries = Vec::with_capacity(idx.len());
        for (k, &i) in idx.iter().enumerate() {
            ctx.check()?;
            ctx.set_progress(k as f32 / idx.len() as f32);
            let f = src.frame(range.start + i)?;
            entries.push(Entry { t: i as f32 / fps as f32, emb: clip.embed_frame(&f)? });
        }
        Ok(FrameIndex { entries })
    }

    /// The best matches for a text embedding, best first, at most `top`,
    /// no two closer than `gap` seconds (one moment, not its neighbours).
    pub fn search(&self, text: &[f32; DIM], top: usize, gap: f32) -> Vec<Moment> {
        let mut all: Vec<Moment> = self.entries.iter().map(|e| Moment { t: e.t, score: e.emb.similarity(text) }).collect();
        all.sort_by(|a, b| b.score.total_cmp(&a.score));
        let mut out: Vec<Moment> = Vec::new();
        for m in all {
            if out.len() >= top {
                break;
            }
            if out.iter().all(|o| (o.t - m.t).abs() >= gap) {
                out.push(m);
            }
        }
        out
    }

    /// The cache key for footage hash `media` and the frames used.
    pub fn cache_key(media: &str, params: &str) -> CacheKey {
        CacheKey::new(media, format!("frame-embeddings-v{VERSION}"), params)
    }

    pub fn load(cache: &ResultCache, key: &CacheKey) -> Option<FrameIndex> {
        cache.get(key)
    }

    pub fn store(&self, cache: &ResultCache, key: &CacheKey) -> std::io::Result<()> {
        cache.put(key, self)
    }
}

impl Cacheable for FrameIndex {
    const TAG: &'static str = "frameindex";
    fn encode(&self, w: &mut Writer) {
        w.u64(self.entries.len() as u64);
        for e in &self.entries {
            w.f32(e.t);
            w.u8(e.emb.views.len() as u8);
            for v in &e.emb.views {
                v.iter().for_each(|x| w.f32(*x));
            }
        }
    }
    fn decode(r: &mut Reader) -> Option<Self> {
        let n = r.len(5)?;
        let mut entries = Vec::with_capacity(n);
        for _ in 0..n {
            let t = r.f32()?;
            let views = r.u8()? as usize;
            let mut vs = Vec::with_capacity(views);
            for _ in 0..views {
                let mut v = [0f32; DIM];
                for x in v.iter_mut() {
                    *x = r.f32()?;
                }
                vs.push(v);
            }
            entries.push(Entry { t, emb: FrameEmbedding { views: vs } });
        }
        Some(FrameIndex { entries })
    }
}

/// Cosine similarity of two unit vectors (a convenience for callers that
/// compare crops).
pub fn similarity(a: &[f32; DIM], b: &[f32; DIM]) -> f32 {
    dot(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::embed::normalised;

    fn unit(i: usize) -> [f32; DIM] {
        let mut v = [0.0f32; DIM];
        v[i] = 1.0;
        v
    }

    fn index() -> FrameIndex {
        // Frame k looks like axis k; frame 2 also looks a bit like axis 5.
        let mut mix = [0.0f32; DIM];
        mix[2] = 1.0;
        mix[5] = 0.5;
        FrameIndex {
            entries: vec![
                Entry { t: 0.5, emb: FrameEmbedding { views: vec![unit(0)] } },
                Entry { t: 1.5, emb: FrameEmbedding { views: vec![unit(1), unit(4)] } },
                Entry { t: 2.5, emb: FrameEmbedding { views: vec![normalised(mix)] } },
                Entry { t: 3.5, emb: FrameEmbedding { views: vec![unit(3)] } },
            ],
        }
    }

    #[test]
    fn the_best_moment_comes_first() {
        let ix = index();
        let m = ix.search(&unit(2), 3, 0.0);
        assert_eq!(m[0].t, 2.5);
        // A frame with several views matches on its best view.
        let m = ix.search(&unit(4), 1, 0.0);
        assert_eq!((m[0].t, m[0].score), (1.5, 1.0));
    }

    #[test]
    fn neighbouring_hits_collapse_into_one_moment() {
        let mut ix = index();
        ix.entries.push(Entry { t: 2.9, emb: FrameEmbedding { views: vec![unit(2)] } });
        let m = ix.search(&unit(2), 4, 1.0);
        let near: Vec<_> = m.iter().filter(|x| x.t > 2.0 && x.t < 3.5).collect();
        assert_eq!(near.len(), 1, "{m:?}");
    }

    #[test]
    fn scores_have_words() {
        assert_eq!(Moment { t: 0.0, score: 0.2 }.strength(), "strong");
        assert_eq!(Moment { t: 0.0, score: 0.07 }.strength(), "likely");
        assert_eq!(Moment { t: 0.0, score: 0.02 }.strength(), "weak");
    }

    #[test]
    fn indices_are_spaced_by_length() {
        assert_eq!(frame_indices(1, 30.0), vec![0]);
        let short = frame_indices(150, 30.0);
        assert_eq!(short.len(), 5);
        assert!(short.iter().all(|&i| i < 150));
        // A ten-minute clip is 24 frames, not 600.
        assert!(frame_indices(18_000, 30.0).len() <= 25);
        assert!(spacing(10.0) >= 1.0 && spacing(1200.0) > 40.0);
    }

    #[test]
    fn the_index_survives_the_cache() {
        let dir = std::env::temp_dir().join(format!("studio-moments-cache-{}", std::process::id()));
        let cache = ResultCache::new(&dir);
        let key = FrameIndex::cache_key("hash", "0..90");
        assert!(FrameIndex::load(&cache, &key).is_none());
        let ix = index();
        ix.store(&cache, &key).unwrap();
        assert_eq!(FrameIndex::load(&cache, &key), Some(ix));
        assert!(FrameIndex::load(&cache, &FrameIndex::cache_key("hash", "0..91")).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
