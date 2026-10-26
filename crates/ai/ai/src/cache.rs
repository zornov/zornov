// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Result cache: finished analyses are stored on disk keyed by
//! (media hash, model, parameters), so re-running a feature on the same clip
//! with the same settings is instant and survives restarts.
//!
//! Values are written with a tiny binary codec ([`Cacheable`]); large outputs
//! (cleaned audio, matte sequences) can instead be written to
//! [`ResultCache::file_for`] by the feature itself.

use crate::sha256::{hex, Sha256};
use std::io;
use std::path::{Path, PathBuf};

/// What a cached result is keyed by.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CacheKey {
    /// [`media_hash`] of the source (or any stable id of the input).
    pub media: String,
    /// Model key ([`crate::ModelId::key`]) or an algorithm name with version.
    pub model: String,
    /// Canonical text of every parameter that changes the result.
    pub params: String,
}

impl CacheKey {
    pub fn new(media: impl Into<String>, model: impl Into<String>, params: impl Into<String>) -> CacheKey {
        CacheKey { media: media.into(), model: model.into(), params: params.into() }
    }

    /// Hex digest naming the cache entry.
    pub fn digest(&self) -> String {
        let mut h = Sha256::new();
        for part in [&self.media, &self.model, &self.params] {
            h.update(&(part.len() as u64).to_le_bytes());
            h.update(part.as_bytes());
        }
        hex(&h.finish())
    }
}

/// A cheap, stable content hash of a media file: SHA-256 over its length and
/// three 1 MiB samples (start, middle, end). Reading whole multi-gigabyte
/// files would make every cache lookup slow; edits to a file almost always
/// change its length or one of these regions.
pub fn media_hash(path: &Path) -> io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    const CHUNK: u64 = 1 << 20;
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    let mut h = Sha256::new();
    h.update(b"studio-media-hash-v1");
    h.update(&len.to_le_bytes());
    let mut buf = vec![0u8; CHUNK as usize];
    // Small files are hashed whole.
    let small = len <= 3 * CHUNK;
    let starts = if small { vec![0] } else { vec![0, len / 2 - CHUNK / 2, len - CHUNK] };
    for s in starts {
        f.seek(SeekFrom::Start(s))?;
        let want = if small { len } else { CHUNK } as usize;
        let mut got = 0;
        if buf.len() < want {
            buf.resize(want, 0);
        }
        while got < want {
            let n = f.read(&mut buf[got..want])?;
            if n == 0 {
                break;
            }
            got += n;
        }
        h.update(&buf[..got]);
    }
    Ok(hex(&h.finish()))
}

/// Hash of in-memory samples or pixels, for inputs that are not files.
pub fn data_hash(bytes: &[u8]) -> String {
    hex(&crate::sha256::sha256(bytes))
}

/// The on-disk cache.
#[derive(Clone, Debug)]
pub struct ResultCache {
    dir: PathBuf,
}

impl ResultCache {
    /// `$HOME/Movies/Studio/Cache/AI`.
    pub fn default_dir() -> PathBuf {
        studio_paths::studio().join("Cache").join("AI")
    }

    pub fn new(dir: impl Into<PathBuf>) -> ResultCache {
        ResultCache { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn entry(&self, key: &CacheKey) -> PathBuf {
        self.dir.join(format!("{}.bin", key.digest()))
    }

    /// A path for a feature's own output file (e.g. `"wav"`), named by `key`.
    /// The directory is created; the file is not.
    pub fn file_for(&self, key: &CacheKey, ext: &str) -> io::Result<PathBuf> {
        std::fs::create_dir_all(&self.dir)?;
        Ok(self.dir.join(format!("{}.{ext}", key.digest())))
    }

    pub fn get<T: Cacheable>(&self, key: &CacheKey) -> Option<T> {
        let bytes = std::fs::read(self.entry(key)).ok()?;
        let mut r = Reader::new(&bytes);
        if r.bytes(4)? != MAGIC || r.str()? != T::TAG {
            return None;
        }
        let v = T::decode(&mut r)?;
        r.is_empty().then_some(v)
    }

    pub fn put<T: Cacheable>(&self, key: &CacheKey, v: &T) -> io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let mut w = Writer::default();
        w.0.extend_from_slice(MAGIC);
        w.str(T::TAG);
        v.encode(&mut w);
        // Write then rename so a crash never leaves a half entry behind.
        let path = self.entry(key);
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &w.0)?;
        std::fs::rename(tmp, path)
    }

    pub fn remove(&self, key: &CacheKey) {
        let _ = std::fs::remove_file(self.entry(key));
    }

    /// Deletes every entry (Settings > Clear AI cache).
    pub fn clear(&self) -> io::Result<()> {
        match std::fs::remove_dir_all(&self.dir) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    /// Total bytes used.
    pub fn size(&self) -> u64 {
        std::fs::read_dir(&self.dir).map(|rd| rd.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()).unwrap_or(0)
    }
}

const MAGIC: &[u8; 4] = b"SAI1";

/// Little-endian byte writer for [`Cacheable`].
#[derive(Default)]
pub struct Writer(pub Vec<u8>);

impl Writer {
    pub fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    pub fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn i64(&mut self, v: i64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn f64(&mut self, v: f64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    pub fn str(&mut self, s: &str) {
        self.u64(s.len() as u64);
        self.0.extend_from_slice(s.as_bytes());
    }
}

/// Reader matching [`Writer`]; every method returns `None` on truncation.
pub struct Reader<'a> {
    b: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Reader { b }
    }
    pub fn is_empty(&self) -> bool {
        self.b.is_empty()
    }
    pub fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.b.len() < n {
            return None;
        }
        let (a, rest) = self.b.split_at(n);
        self.b = rest;
        Some(a)
    }
    fn arr<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.bytes(N)?.try_into().ok()
    }
    pub fn u8(&mut self) -> Option<u8> {
        Some(self.bytes(1)?[0])
    }
    pub fn u32(&mut self) -> Option<u32> {
        self.arr().map(u32::from_le_bytes)
    }
    pub fn u64(&mut self) -> Option<u64> {
        self.arr().map(u64::from_le_bytes)
    }
    pub fn i64(&mut self) -> Option<i64> {
        self.arr().map(i64::from_le_bytes)
    }
    pub fn f32(&mut self) -> Option<f32> {
        self.arr().map(f32::from_le_bytes)
    }
    pub fn f64(&mut self) -> Option<f64> {
        self.arr().map(f64::from_le_bytes)
    }
    pub fn str(&mut self) -> Option<String> {
        let n = usize::try_from(self.u64()?).ok()?;
        String::from_utf8(self.bytes(n)?.to_vec()).ok()
    }
    /// A length prefix, sanity-checked against the bytes left (each element
    /// takes at least `min_elem` bytes) so corrupt data cannot trigger a huge
    /// allocation.
    pub fn len(&mut self, min_elem: usize) -> Option<usize> {
        let n = usize::try_from(self.u64()?).ok()?;
        (n.saturating_mul(min_elem.max(1)) <= self.b.len()).then_some(n)
    }
}

/// A value that can live in the [`ResultCache`].
pub trait Cacheable: Sized {
    /// Type tag stored with the value; change it when the encoding changes.
    const TAG: &'static str;
    fn encode(&self, w: &mut Writer);
    fn decode(r: &mut Reader) -> Option<Self>;
}

macro_rules! prim {
    ($t:ty, $m:ident, $tag:literal) => {
        impl Cacheable for $t {
            const TAG: &'static str = $tag;
            fn encode(&self, w: &mut Writer) {
                w.$m(*self);
            }
            fn decode(r: &mut Reader) -> Option<Self> {
                r.$m()
            }
        }
    };
}
prim!(f32, f32, "f32");
prim!(f64, f64, "f64");
prim!(i64, i64, "i64");
prim!(u32, u32, "u32");
prim!(u64, u64, "u64");

impl Cacheable for String {
    const TAG: &'static str = "str";
    fn encode(&self, w: &mut Writer) {
        w.str(self);
    }
    fn decode(r: &mut Reader) -> Option<Self> {
        r.str()
    }
}

impl<T: Cacheable> Cacheable for Vec<T> {
    const TAG: &'static str = T::TAG;
    fn encode(&self, w: &mut Writer) {
        w.u64(self.len() as u64);
        self.iter().for_each(|v| v.encode(w));
    }
    fn decode(r: &mut Reader) -> Option<Self> {
        let n = r.len(1)?;
        (0..n).map(|_| T::decode(r)).collect()
    }
}

impl<T: Cacheable> Cacheable for Option<T> {
    const TAG: &'static str = T::TAG;
    fn encode(&self, w: &mut Writer) {
        match self {
            None => w.u8(0),
            Some(v) => {
                w.u8(1);
                v.encode(w);
            }
        }
    }
    fn decode(r: &mut Reader) -> Option<Self> {
        match r.u8()? {
            0 => Some(None),
            1 => Some(Some(T::decode(r)?)),
            _ => None,
        }
    }
}

impl<A: Cacheable, B: Cacheable> Cacheable for (A, B) {
    const TAG: &'static str = A::TAG;
    fn encode(&self, w: &mut Writer) {
        self.0.encode(w);
        self.1.encode(w);
    }
    fn decode(r: &mut Reader) -> Option<Self> {
        Some((A::decode(r)?, B::decode(r)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("studio-ai-cache-{tag}-{}", std::process::id()))
    }

    #[test]
    fn round_trip_and_key_separation() {
        let dir = tmp("rt");
        let c = ResultCache::new(&dir);
        let k1 = CacheKey::new("abc", "whisper-tiny", "lang=en");
        let k2 = CacheKey::new("abc", "whisper-tiny", "lang=de");
        assert!(c.get::<Vec<String>>(&k1).is_none());
        c.put(&k1, &vec!["hello".to_string(), "world".to_string()]).unwrap();
        assert_eq!(c.get::<Vec<String>>(&k1).unwrap(), ["hello", "world"]);
        assert!(c.get::<Vec<String>>(&k2).is_none());
        // a different type under the same key is a miss, not garbage
        assert!(c.get::<Vec<f64>>(&k1).is_none());
        // ("ab","c") and ("a","bc") must not collide
        assert_ne!(CacheKey::new("ab", "c", "").digest(), CacheKey::new("a", "bc", "").digest());
        assert!(c.size() > 0);
        c.clear().unwrap();
        assert!(c.get::<Vec<String>>(&k1).is_none());
    }

    #[test]
    fn corrupt_entries_are_misses() {
        let dir = tmp("corrupt");
        let c = ResultCache::new(&dir);
        let k = CacheKey::new("m", "x", "");
        c.put(&k, &vec![1.0f64; 10]).unwrap();
        let p = dir.join(format!("{}.bin", k.digest()));
        let mut b = std::fs::read(&p).unwrap();
        b.truncate(b.len() - 3);
        std::fs::write(&p, &b).unwrap();
        assert!(c.get::<Vec<f64>>(&k).is_none());
        // a huge length prefix must not allocate
        let mut w = Writer::default();
        w.0.extend_from_slice(MAGIC);
        w.str("f64");
        w.u64(u64::MAX / 2);
        std::fs::write(&p, &w.0).unwrap();
        assert!(c.get::<Vec<f64>>(&k).is_none());
        c.clear().unwrap();
    }

    #[test]
    fn media_hash_sees_content_changes() {
        let dir = tmp("media");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("clip.bin");
        let mut data: Vec<u8> = (0..5_000_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&p, &data).unwrap();
        let a = media_hash(&p).unwrap();
        assert_eq!(a, media_hash(&p).unwrap());
        data[4_999_000] ^= 1; // inside the tail sample
        std::fs::write(&p, &data).unwrap();
        assert_ne!(a, media_hash(&p).unwrap());
        std::fs::write(&p, b"tiny").unwrap();
        assert_eq!(media_hash(&p).unwrap().len(), 64);
        let _ = std::fs::remove_dir_all(dir);
    }
}
