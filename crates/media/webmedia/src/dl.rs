// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The three ways to get media once its address is known: a plain file, an
//! HLS stream, a DASH stream. Streams are fetched in parallel into a
//! working folder beside the result (so a second try resumes), joined into
//! one MP4, and the folder removed.

use crate::assemble::{assemble, Piece};
use crate::dash;
use crate::error::{Error, Result};
use crate::hls::{self, KeyMethod, Playlist};
use crate::http::{self, Job};
use crate::track::TrackData;
use crate::types::{Options, Progress, Quality, Report, Saved};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct Ctx<'a> {
    /// The page the media came from (sent as the referer).
    pub referer: &'a str,
    pub opts: &'a Options,
    pub report: Report<'a>,
    pub dir: &'a Path,
    pub stem: &'a str,
    pub title: Option<String>,
}

/// A file name stem that is safe on every system.
pub fn safe_stem(s: &str) -> String {
    let clean: String = s.chars().map(|c| if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.') { c } else { '-' }).collect();
    let clean = clean.trim_matches(|c: char| c == '.' || c == '-' || c.is_whitespace());
    let clean: String = clean.chars().take(80).collect();
    if clean.is_empty() { "download".into() } else { clean }
}

pub fn free_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    (1..)
        .map(|n| if n == 1 { dir.join(format!("{stem}.{ext}")) } else { dir.join(format!("{stem} {n}.{ext}")) })
        .find(|p| !p.exists())
        .expect("a free name")
}

fn work_dir(ctx: &Ctx, url: &str) -> Result<PathBuf> {
    let fp = http::fingerprint(&[url, &format!("{:?}", ctx.opts.quality), ctx.opts.language.as_deref().unwrap_or("")]);
    let w = ctx.dir.join(format!(".studio-parts-{fp}"));
    http::ensure_dir(&w)?;
    Ok(w)
}

fn say(ctx: &Ctx, stage: &'static str) -> Result<()> {
    if (ctx.report)(&Progress { stage, done: 0, total: 0, bytes: 0 }) { Ok(()) } else { Err(Error::Cancelled) }
}

/// A file's kind from its first bytes.
pub fn sniff_ext(path: &Path) -> Option<&'static str> {
    use std::io::Read;
    let mut h = [0u8; 16];
    let n = std::fs::File::open(path).ok()?.read(&mut h).ok()?;
    let h = &h[..n];
    Some(if n >= 12 && &h[4..8] == b"ftyp" {
        if &h[8..11] == b"M4A" { "m4a" } else if &h[8..12] == b"qt  " { "mov" } else { "mp4" }
    } else if h.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        "webm"
    } else if h.starts_with(b"ID3") || (n > 1 && h[0] == 0xFF && h[1] & 0xE0 == 0xE0) {
        "mp3"
    } else if h.starts_with(b"OggS") {
        "ogg"
    } else if h.starts_with(b"RIFF") && n >= 12 && &h[8..12] == b"WAVE" {
        "wav"
    } else if h.starts_with(b"fLaC") {
        "flac"
    } else {
        return None;
    })
}

// ---- a plain file ----

pub fn download_file(url: &str, ctx: &Ctx) -> Result<Saved> {
    say(ctx, "downloading")?;
    let ext0 = crate::url::ext_of(url).filter(|e| crate::page::is_av_ext(e)).unwrap_or_else(|| "bin".into());
    let fp = http::fingerprint(&[url]);
    let part = ctx.dir.join(format!(".studio-file-{fp}.part"));
    let opts = studio_net::Download { resume: true, max_bytes: ctx.opts.max_bytes, stall: std::time::Duration::from_secs(30), total: ctx.opts.time_limit, retries: 2 };
    let report = ctx.report;
    let result = studio_net::download(url, &part, opts, &mut |bytes| report(&Progress { stage: "downloading", done: bytes, total: 0, bytes }));
    match result {
        Ok(()) => {}
        Err(studio_net::Failure::Cancelled) => return Err(Error::Cancelled),
        Err(studio_net::Failure::Failed(e)) => {
            let low = e.to_ascii_lowercase();
            return Err(if low.contains("401") || low.contains("403") {
                Error::SignIn(format!("{} did not hand that over ({e}). It needs a signed-in account or a licence, and Zornov does not sign in for you or get around that.", crate::url::host_of(url)))
            } else {
                let _ = std::fs::remove_file(&part);
                Error::Net(format!("the download failed: {e}"))
            });
        }
    }
    let ext = sniff_ext(&part).map(str::to_owned).unwrap_or(ext0);
    let dest = free_path(ctx.dir, &safe_stem(ctx.stem), &ext);
    std::fs::rename(&part, &dest)?;
    Ok(Saved { file: dest, title: ctx.title.clone(), via: "file", chosen: "the file as the page links it".into(), available: Vec::new(), notes: Vec::new() })
}

// ---- common ----

fn combine(main: Vec<TrackData>, extra_audio: Option<Vec<TrackData>>, q: Quality) -> Result<Vec<TrackData>> {
    let mut out: Vec<TrackData> = Vec::new();
    let has_extra = extra_audio.as_ref().is_some_and(|a| a.iter().any(|t| t.is_audio()));
    for t in main {
        if t.is_video() && q == Quality::AudioOnly {
            continue;
        }
        if t.is_audio() && has_extra {
            continue;
        }
        // One video, one sound: a second of either is an alternate version.
        if out.iter().any(|o| o.handler == t.handler) {
            continue;
        }
        out.push(t);
    }
    if let Some(a) = extra_audio {
        if let Some(t) = a.into_iter().find(|t| t.is_audio()) {
            out.push(t);
        }
    }
    if out.is_empty() {
        return Err(Error::Unsupported(if q == Quality::AudioOnly { "That stream has no sound track to save.".into() } else { "The stream holds no video or sound Zornov can take.".into() }));
    }
    Ok(out)
}

fn finish(ctx: &Ctx, work: &Path, tracks: &[TrackData], via: &'static str, chosen: String, available: Vec<String>, notes: Vec<String>) -> Result<Saved> {
    say(ctx, "joining")?;
    let has_video = tracks.iter().any(|t| t.is_video());
    let tmp = work.join("result.mp4");
    crate::mp4w::write_mp4(&tmp, tracks)?;
    let dest = free_path(ctx.dir, &safe_stem(ctx.stem), if has_video { "mp4" } else { "m4a" });
    std::fs::rename(&tmp, &dest)?;
    let _ = std::fs::remove_dir_all(work);
    Ok(Saved { file: dest, title: ctx.title.clone(), via, chosen, available, notes })
}

fn fetch_jobs(ctx: &Ctx, jobs: &[Job]) -> Result<()> {
    http::fetch_all(jobs, ctx.referer, ctx.opts.parallel, ctx.opts.max_bytes, ctx.opts.time_limit, "fetching", ctx.report)
}

// ---- HLS ----

struct Prepared {
    tag: &'static str,
    jobs: Vec<Job>,
    inits: Vec<PathBuf>,
    /// Init files that need decrypting after they are fetched: (fetched, decrypted, key, iv).
    init_keys: Vec<(PathBuf, PathBuf, [u8; 16], [u8; 16])>,
    pieces: Vec<Piece>,
    notes: Vec<String>,
}

fn iv_for(seq: u64) -> [u8; 16] {
    (seq as u128).to_be_bytes()
}

fn prepare_hls(media: &hls::Media, tag: &'static str, work: &Path, ctx: &Ctx) -> Result<Prepared> {
    if media.i_frames_only {
        return Err(Error::Unsupported("That playlist lists only key frames (an index for scrubbing), not the video itself.".into()));
    }
    if let Some(why) = media.protection() {
        return Err(Error::Protected(why));
    }
    if media.segments.is_empty() {
        return Err(Error::Bad("the playlist lists no segments".into()));
    }
    if media.segments.len() > 100_000 {
        return Err(Error::Bad("the playlist lists an unreasonable number of segments".into()));
    }
    let mut notes = Vec::new();
    if !media.endlist {
        notes.push(format!("This stream has no end marker (it is live, or still being made); Zornov saved the {:.0} seconds the playlist lists now.", media.seconds()));
    }
    // Keys the playlist hands out openly.
    let mut keys: HashMap<String, [u8; 16]> = HashMap::new();
    for k in &media.keys {
        if k.method != KeyMethod::Aes128 {
            continue;
        }
        let uri = k.uri.clone().ok_or_else(|| Error::Bad("the playlist says the segments are encrypted but gives no key address".into()))?;
        if keys.contains_key(&uri) {
            continue;
        }
        if !crate::url::is_http(&uri) {
            return Err(Error::Protected("This stream's key is not given openly (it is fetched through the player's protection). Zornov does not work around copy protection.".into()));
        }
        let doc = http::get_doc(&uri, ctx.referer, 4096)?;
        let key: [u8; 16] = doc.body.as_slice().try_into().map_err(|_| Error::Protected("This stream's key is not given openly (the key address does not return a plain 16-byte key). Zornov does not work around copy protection.".into()))?;
        keys.insert(uri, key);
    }
    let key_of = |idx: Option<usize>, seq: u64| -> Option<([u8; 16], [u8; 16])> {
        let k = &media.keys[idx?];
        let key = *keys.get(k.uri.as_ref()?)?;
        Some((key, k.iv.unwrap_or_else(|| iv_for(seq))))
    };
    let mut jobs = Vec::new();
    let mut inits = Vec::new();
    let mut init_keys = Vec::new();
    for (i, m) in media.maps.iter().enumerate() {
        let fetched = work.join(format!("{tag}-init-{i}.mp4"));
        jobs.push(Job { url: m.uri.clone(), range: m.range, path: fetched.clone() });
        match key_of(m.key, 0) {
            Some((k, iv)) => {
                let dec = work.join(format!("{tag}-init-{i}.dec"));
                init_keys.push((fetched, dec.clone(), k, iv));
                inits.push(dec);
            }
            None => inits.push(fetched),
        }
    }
    let mut pieces = Vec::new();
    for (i, s) in media.segments.iter().enumerate() {
        let path = work.join(format!("{tag}-{i:06}.seg"));
        jobs.push(Job { url: s.uri.clone(), range: s.range, path: path.clone() });
        pieces.push(Piece { path, key: key_of(s.key, s.seq), discontinuity: s.disc, init: s.map });
    }
    Ok(Prepared { tag, jobs, inits, init_keys, pieces, notes })
}

fn decrypt_inits(p: &Prepared) -> Result<()> {
    for (src, dst, key, iv) in &p.init_keys {
        if dst.exists() {
            continue;
        }
        let data = std::fs::read(src)?;
        let plain = crate::aes::Aes128::new(key).cbc_decrypt(iv, &data).map_err(Error::Bad)?;
        std::fs::write(dst, plain)?;
    }
    Ok(())
}

fn load_media(url: &str, ctx: &Ctx) -> Result<hls::Media> {
    let doc = http::get_doc(url, ctx.referer, 16 << 20)?;
    match hls::parse(&doc.url, &doc.text())? {
        Playlist::Media(m) => Ok(m),
        Playlist::Master(_) => Err(Error::Bad("a playlist of playlists was expected to list segments".into())),
    }
}

pub fn download_hls(url: &str, ctx: &Ctx) -> Result<Saved> {
    say(ctx, "reading the playlist")?;
    let doc = http::get_doc(url, ctx.referer, 16 << 20)?;
    let (video_media, audio_url, chosen, available): (hls::Media, Option<String>, String, Vec<String>) = match hls::parse(&doc.url, &doc.text())? {
        Playlist::Master(m) => {
            let vi = hls::choose_variant(&m, ctx.opts.quality).ok_or_else(|| Error::NotExposed("The playlist lists no streams.".into()))?;
            let v = &m.variants[vi];
            let aud = if ctx.opts.quality == Quality::AudioOnly && v.height == 0 { None } else { hls::choose_audio(&m, v, ctx.opts.language.as_deref()) };
            let mut chosen = hls::describe(v);
            if let Some(a) = aud {
                chosen.push_str(&format!(", sound: {}", if a.name.is_empty() { "alternate" } else { &a.name }));
            }
            let media = load_media(&v.uri, ctx)?;
            (media, aud.and_then(|a| a.uri.clone()), chosen, hls::available(&m))
        }
        Playlist::Media(m) => (m, None, "the only stream".into(), Vec::new()),
    };
    let work = work_dir(ctx, url)?;
    let mut pv = prepare_hls(&video_media, "v", &work, ctx)?;
    let mut pa = match &audio_url {
        Some(u) => Some(prepare_hls(&load_media(u, ctx)?, "a", &work, ctx)?),
        None => None,
    };
    let mut jobs = pv.jobs.clone();
    if let Some(a) = &pa {
        jobs.extend(a.jobs.iter().cloned());
    }
    fetch_jobs(ctx, &jobs)?;
    say(ctx, "joining")?;
    decrypt_inits(&pv)?;
    let mut notes = std::mem::take(&mut pv.notes);
    let main = assemble(&work, pv.tag, &pv.inits, &pv.pieces, &mut notes)?;
    let extra = match pa.as_mut() {
        Some(a) => {
            decrypt_inits(a)?;
            let mut n = std::mem::take(&mut a.notes);
            let t = assemble(&work, a.tag, &a.inits, &a.pieces, &mut n)?;
            notes.extend(n);
            Some(t)
        }
        None => None,
    };
    notes.dedup();
    let tracks = combine(main, extra, ctx.opts.quality)?;
    let chosen = describe_tracks(&tracks, chosen);
    finish(ctx, &work, &tracks, "hls", chosen, available, notes)
}

fn describe_tracks(tracks: &[TrackData], fallback: String) -> String {
    match tracks.iter().find(|t| t.is_video()) {
        Some(v) if fallback == "the only stream" && v.height > 0 => format!("{}x{}", v.width, v.height),
        _ => fallback,
    }
}

// ---- DASH ----

fn rep_jobs(rep: &dash::Rep, tag: &'static str, work: &Path) -> (Vec<Job>, Option<PathBuf>, Vec<Piece>, Option<PathBuf>) {
    match &rep.plan {
        dash::Plan::Single(c) => {
            let p = work.join(format!("{tag}-whole.mp4"));
            (vec![Job { url: c.url.clone(), range: c.range, path: p.clone() }], None, Vec::new(), Some(p))
        }
        dash::Plan::Segments { init, segs } => {
            let mut jobs = Vec::new();
            let init_path = init.as_ref().map(|c| {
                let p = work.join(format!("{tag}-init.mp4"));
                jobs.push(Job { url: c.url.clone(), range: c.range, path: p.clone() });
                p
            });
            let mut pieces = Vec::new();
            for (i, c) in segs.iter().enumerate() {
                let p = work.join(format!("{tag}-{i:06}.seg"));
                jobs.push(Job { url: c.url.clone(), range: c.range, path: p.clone() });
                pieces.push(Piece { path: p, key: None, discontinuity: false, init: init_path.as_ref().map(|_| 0) });
            }
            (jobs, init_path, pieces, None)
        }
    }
}

fn rep_tracks(rep: &dash::Rep, tag: &'static str, work: &Path, notes: &mut Vec<String>) -> Result<Vec<TrackData>> {
    let (_, init, pieces, whole) = rep_jobs(rep, tag, work);
    match whole {
        Some(p) => crate::fmp4::tracks_from_mp4(&p),
        None => assemble(work, tag, init.as_slice(), &pieces, notes),
    }
}

pub fn download_dash(url: &str, ctx: &Ctx) -> Result<Saved> {
    say(ctx, "reading the manifest")?;
    let doc = http::get_doc(url, ctx.referer, 16 << 20)?;
    let mpd = dash::parse(&doc.url, &doc.text())?;
    let (v, a) = dash::choose(&mpd, ctx.opts.quality, ctx.opts.language.as_deref())?;
    let mut chosen = Vec::new();
    if let Some(v) = v {
        chosen.push(dash::describe(v));
    }
    if let Some(a) = a {
        chosen.push(format!("sound {}", dash::describe(a).replace("sound, ", "")));
    }
    let work = work_dir(ctx, url)?;
    let mut jobs = Vec::new();
    if let Some(v) = v {
        jobs.extend(rep_jobs(v, "v", &work).0);
    }
    if let Some(a) = a {
        jobs.extend(rep_jobs(a, "a", &work).0);
    }
    fetch_jobs(ctx, &jobs)?;
    say(ctx, "joining")?;
    let mut notes = Vec::new();
    if mpd.periods > 1 {
        notes.push(format!("The manifest has {} periods (parts, such as ad breaks); Zornov saved the longest one.", mpd.periods));
    }
    if mpd.dynamic {
        notes.push("This stream is still being made (live); Zornov saved what the manifest lists now.".into());
    }
    let main = match v {
        Some(v) => rep_tracks(v, "v", &work, &mut notes)?,
        None => Vec::new(),
    };
    let extra = match a {
        Some(a) => Some(rep_tracks(a, "a", &work, &mut notes)?),
        None => None,
    };
    let (main, extra) = if v.is_none() { (extra.unwrap_or_default(), None) } else { (main, extra) };
    let tracks = combine(main, extra, ctx.opts.quality)?;
    finish(ctx, &work, &tracks, "dash", chosen.join("; "), dash::available(&mpd), notes)
}
