// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! End-to-end tests against a local web server. Fixtures are generated
//! here: a few seconds of H.264 and AAC from Studio's own encoders, cut
//! into transport stream and MP4 segments, behind HTML pages, HLS
//! playlists and DASH manifests. No real internet is used.

use crate::mp4w::{avc1_entry, bx, fbx, mp4a_entry};
use crate::{Error, Options, Progress, Quality};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use studio_live::av::{AvConfig, AvEncoder, Packet};
use studio_live::source::{AudioSource, TestAudio, TestVideo, VideoSource};
use studio_live::ts::TsMuxer;

// ---- a local web server ----

#[derive(Clone)]
struct Route {
    ct: String,
    body: Arc<Vec<u8>>,
    status: u16,
    location: Option<String>,
    delay_ms: u64,
}

#[derive(Clone, Debug)]
struct Req {
    path: String,
    range: Option<String>,
    referer: Option<String>,
}

struct Server {
    port: u16,
    routes: Arc<Mutex<HashMap<String, Route>>>,
    log: Arc<Mutex<Vec<Req>>>,
    stop: Arc<AtomicBool>,
    max_inflight: Arc<AtomicUsize>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn respond(mut s: TcpStream, routes: &Mutex<HashMap<String, Route>>, log: &Mutex<Vec<Req>>, inflight: &AtomicUsize, max_inflight: &AtomicUsize) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let head = String::from_utf8_lossy(&buf).into_owned();
    let mut lines = head.lines();
    let first = lines.next().unwrap_or("");
    let path = first.split_whitespace().nth(1).unwrap_or("/").to_owned();
    let header = |name: &str| lines.clone().find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.trim().to_owned()));
    let range = header("range");
    log.lock().unwrap().push(Req { path: path.clone(), range: range.clone(), referer: header("referer") });
    let now = inflight.fetch_add(1, Ordering::SeqCst) + 1;
    max_inflight.fetch_max(now, Ordering::SeqCst);
    let key = path.split('?').next().unwrap_or("").to_owned();
    let route = routes.lock().unwrap().get(&key).cloned();
    match route {
        None => {
            let _ = write!(s, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
        Some(r) => {
            if r.delay_ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(r.delay_ms));
            }
            if let Some(loc) = &r.location {
                let _ = write!(s, "HTTP/1.1 302 Found\r\nLocation: {loc}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            } else if r.status != 200 {
                let _ = write!(s, "HTTP/1.1 {} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", r.status);
            } else if let Some((a, b)) = range.as_deref().and_then(|v| v.strip_prefix("bytes=")).and_then(|v| v.split_once('-')) {
                let len = r.body.len() as u64;
                let a: u64 = a.parse().unwrap_or(0);
                let b: u64 = b.parse().unwrap_or(len - 1).min(len - 1);
                let part = &r.body[a as usize..=b as usize];
                let _ = write!(s, "HTTP/1.1 206 Partial Content\r\nContent-Type: {}\r\nContent-Range: bytes {a}-{b}/{len}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", r.ct, part.len());
                let _ = s.write_all(part);
            } else {
                let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", r.ct, r.body.len());
                let _ = s.write_all(&r.body);
            }
        }
    }
    inflight.fetch_sub(1, Ordering::SeqCst);
}

impl Server {
    fn start() -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let routes: Arc<Mutex<HashMap<String, Route>>> = Arc::default();
        let log: Arc<Mutex<Vec<Req>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let inflight = Arc::new(AtomicUsize::new(0));
        let max_inflight = Arc::new(AtomicUsize::new(0));
        {
            let (routes, log, stop, inflight, max_inflight) = (routes.clone(), log.clone(), stop.clone(), inflight.clone(), max_inflight.clone());
            std::thread::spawn(move || {
                for conn in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        return;
                    }
                    if let Ok(c) = conn {
                        let (routes, log, inflight, max_inflight) = (routes.clone(), log.clone(), inflight.clone(), max_inflight.clone());
                        std::thread::spawn(move || respond(c, &routes, &log, &inflight, &max_inflight));
                    }
                }
            });
        }
        Server { port, routes, log, stop, max_inflight }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    fn add(&self, path: &str, ct: &str, body: impl Into<Vec<u8>>) {
        self.routes.lock().unwrap().insert(path.to_owned(), Route { ct: ct.to_owned(), body: Arc::new(body.into()), status: 200, location: None, delay_ms: 0 });
    }

    fn page(&self, path: &str, html: &str) {
        self.add(path, "text/html; charset=utf-8", html.as_bytes());
    }

    fn set_status(&self, path: &str, status: u16) {
        if let Some(r) = self.routes.lock().unwrap().get_mut(path) {
            r.status = status;
        }
    }

    fn set_delay(&self, prefix: &str, ms: u64) {
        for (k, r) in self.routes.lock().unwrap().iter_mut() {
            if k.starts_with(prefix) {
                r.delay_ms = ms;
            }
        }
    }

    fn redirect(&self, path: &str, to: &str) {
        self.routes.lock().unwrap().insert(path.to_owned(), Route { ct: "text/plain".into(), body: Arc::default(), status: 200, location: Some(to.to_owned()), delay_ms: 0 });
    }

    fn hits(&self, prefix: &str) -> usize {
        self.log.lock().unwrap().iter().filter(|r| r.path.starts_with(prefix)).count()
    }

    fn requests(&self) -> Vec<Req> {
        self.log.lock().unwrap().clone()
    }
}

// ---- fixtures: a clip, cut into segments ----

struct Clip {
    w: u32,
    h: u32,
    sps: Vec<u8>,
    pps: Vec<u8>,
    asc: Vec<u8>,
    /// (pts in microseconds, AVCC access unit, keyframe)
    video: Vec<(i64, Vec<u8>, bool)>,
    /// (pts in microseconds, raw AAC frame)
    audio: Vec<(i64, Vec<u8>)>,
}

const FPS: u32 = 30;

fn make_clip(w: u32, h: u32, secs: u32, seed: &str) -> Clip {
    let mut cfg = AvConfig::new(w, h, FPS as f64);
    cfg.gop_secs = 1.0;
    let mut enc = AvEncoder::new(cfg).unwrap();
    let mut v = TestVideo::from_spec(seed, w, h, FPS as f64).unwrap();
    let mut a = TestAudio::from_spec("tone:440", 48000).unwrap();
    let mut video = Vec::new();
    let mut audio = Vec::new();
    for n in 0..secs * FPS {
        if let Packet::Video { data, keyframe, pts_us } = enc.push_frame(&v.frame_at((n as f64 * 1e6 / FPS as f64) as i64).unwrap()) {
            video.push((pts_us, data, keyframe));
        }
        for p in enc.push_audio(&a.read(1600)) {
            if let Packet::Audio { data, pts_us } = p {
                audio.push((pts_us, data));
            }
        }
    }
    for p in enc.flush() {
        if let Packet::Audio { data, pts_us } = p {
            audio.push((pts_us, data));
        }
    }
    Clip { w: enc.cfg.width, h: enc.cfg.height, sps: enc.sps().to_vec(), pps: enc.pps().to_vec(), asc: enc.audio_info().asc, video, audio }
}

/// MPEG transport stream segments of `seg_secs` each (video and sound).
fn ts_segments(c: &Clip, seg_secs: u32, with_audio: bool) -> Vec<Vec<u8>> {
    let total = c.video.len() as u32 / FPS;
    (0..total / seg_secs)
        .map(|k| {
            let (a, b) = ((k * seg_secs) as i64 * 1_000_000, ((k + 1) * seg_secs) as i64 * 1_000_000);
            let mut mux = TsMuxer::new(&c.sps, &c.pps, 48000);
            for (pts, data, key) in &c.video {
                if *pts >= a && *pts < b {
                    mux.push(&Packet::Video { data: data.clone(), keyframe: *key, pts_us: *pts });
                }
            }
            if with_audio {
                for (pts, data) in &c.audio {
                    if *pts >= a && *pts < b {
                        mux.push(&Packet::Audio { data: data.clone(), pts_us: *pts });
                    }
                }
            }
            mux.take()
        })
        .collect()
}

fn adts_frames(c: &Clip, from_us: i64, to_us: i64) -> Vec<u8> {
    let mut out = Vec::new();
    for (pts, data) in &c.audio {
        if *pts >= from_us && *pts < to_us {
            let len = data.len() + 7;
            out.extend_from_slice(&[0xFF, 0xF1, (1 << 6) | (3 << 2), 0x80 | ((len >> 11) & 3) as u8, (len >> 3) as u8, ((len & 7) << 5) as u8 | 0x1F, 0xFC]);
            out.extend_from_slice(data);
        }
    }
    out
}

#[derive(Clone, Copy, PartialEq)]
enum Which {
    Video,
    Audio,
}

fn trak(id: u32, handler: &[u8; 4], timescale: u32, entry: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut tkhd = vec![0u8; 8];
    tkhd.extend_from_slice(&id.to_be_bytes());
    tkhd.extend_from_slice(&[0; 4 + 4 + 8 + 4]);
    tkhd.extend_from_slice(&(if handler == b"soun" { 0x0100u16 } else { 0 }).to_be_bytes());
    tkhd.extend_from_slice(&[0; 2]);
    for m in crate::track::IDENTITY {
        tkhd.extend_from_slice(&m.to_be_bytes());
    }
    tkhd.extend_from_slice(&(w << 16).to_be_bytes());
    tkhd.extend_from_slice(&(h << 16).to_be_bytes());
    let tkhd = fbx(b"tkhd", 0, 3, &tkhd);
    let mut mdhd = vec![0u8; 8];
    mdhd.extend_from_slice(&timescale.to_be_bytes());
    mdhd.extend_from_slice(&[0; 4]);
    mdhd.extend_from_slice(&0x55C4u16.to_be_bytes());
    mdhd.extend_from_slice(&[0; 2]);
    let mdhd = fbx(b"mdhd", 0, 0, &mdhd);
    let mut hd = vec![0u8; 4];
    hd.extend_from_slice(handler);
    hd.extend_from_slice(&[0; 12]);
    hd.push(0);
    let hdlr = fbx(b"hdlr", 0, 0, &hd);
    let xmhd = if handler == b"vide" { fbx(b"vmhd", 0, 1, &[0; 8]) } else { fbx(b"smhd", 0, 0, &[0; 4]) };
    let mut dref = 1u32.to_be_bytes().to_vec();
    dref.extend_from_slice(&fbx(b"url ", 0, 1, &[]));
    let dinf = bx(b"dinf", &fbx(b"dref", 0, 0, &dref));
    let mut stsd = 1u32.to_be_bytes().to_vec();
    stsd.extend_from_slice(entry);
    let mut stbl = fbx(b"stsd", 0, 0, &stsd);
    for t in [b"stts", b"stsc", b"stco"] {
        stbl.extend_from_slice(&fbx(t, 0, 0, &0u32.to_be_bytes()));
    }
    stbl.extend_from_slice(&fbx(b"stsz", 0, 0, &[0; 8]));
    let mut minf = xmhd;
    minf.extend_from_slice(&dinf);
    minf.extend_from_slice(&bx(b"stbl", &stbl));
    let mut mdia = mdhd;
    mdia.extend_from_slice(&hdlr);
    mdia.extend_from_slice(&bx(b"minf", &minf));
    let mut t = tkhd;
    t.extend_from_slice(&bx(b"mdia", &mdia));
    bx(b"trak", &t)
}

/// An init segment: `ftyp` and a `moov` with empty tables and `mvex`.
fn fmp4_init(c: &Clip, which: Which) -> Vec<u8> {
    let mut out = bx(b"ftyp", b"iso5\0\0\0\x01iso5iso6mp41");
    let mut mv = vec![0u8; 8];
    mv.extend_from_slice(&1000u32.to_be_bytes());
    mv.extend_from_slice(&[0; 4]);
    mv.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    mv.extend_from_slice(&0x0100u16.to_be_bytes());
    mv.extend_from_slice(&[0; 10]);
    for m in crate::track::IDENTITY {
        mv.extend_from_slice(&m.to_be_bytes());
    }
    mv.extend_from_slice(&[0; 24]);
    mv.extend_from_slice(&3u32.to_be_bytes());
    let mut moov = fbx(b"mvhd", 0, 0, &mv);
    let mut mvex = Vec::new();
    if which != Which::Audio {
        moov.extend_from_slice(&trak(1, b"vide", 90000, &avc1_entry(&c.sps, std::slice::from_ref(&c.pps), c.w, c.h).unwrap(), c.w, c.h));
        let mut trex = 1u32.to_be_bytes().to_vec();
        trex.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        mvex.extend_from_slice(&fbx(b"trex", 0, 0, &trex));
    }
    if which != Which::Video {
        moov.extend_from_slice(&trak(2, b"soun", 48000, &mp4a_entry(&c.asc, 2, 48000), 0, 0));
        let mut trex = 2u32.to_be_bytes().to_vec();
        trex.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        mvex.extend_from_slice(&fbx(b"trex", 0, 0, &trex));
    }
    moov.extend_from_slice(&bx(b"mvex", &mvex));
    out.extend_from_slice(&bx(b"moov", &moov));
    out
}

struct Frag {
    track: u32,
    base: u64,
    /// (duration, data, sync)
    samples: Vec<(u32, Vec<u8>, bool)>,
}

fn moof_mdat(seq: u32, frags: &[Frag]) -> Vec<u8> {
    let build = |off: &[u32]| {
        let mut m = fbx(b"mfhd", 0, 0, &seq.to_be_bytes());
        for (f, o) in frags.iter().zip(off) {
            let tfhd = fbx(b"tfhd", 0, 0x020000, &f.track.to_be_bytes());
            let tfdt = fbx(b"tfdt", 1, 0, &f.base.to_be_bytes());
            let mut trun = (f.samples.len() as u32).to_be_bytes().to_vec();
            trun.extend_from_slice(&o.to_be_bytes());
            for (d, data, sync) in &f.samples {
                trun.extend_from_slice(&d.to_be_bytes());
                trun.extend_from_slice(&(data.len() as u32).to_be_bytes());
                trun.extend_from_slice(&(if *sync { 0x0200_0000u32 } else { 0x0101_0000 }).to_be_bytes());
            }
            let trun = fbx(b"trun", 0, 0x000701, &trun);
            let mut traf = tfhd;
            traf.extend_from_slice(&tfdt);
            traf.extend_from_slice(&trun);
            m.extend_from_slice(&bx(b"traf", &traf));
        }
        bx(b"moof", &m)
    };
    let probe = build(&vec![0; frags.len()]);
    let mut offs = Vec::new();
    let mut at = probe.len() as u32 + 8;
    for f in frags {
        offs.push(at);
        at += f.samples.iter().map(|s| s.1.len() as u32).sum::<u32>();
    }
    let mut out = build(&offs);
    let mut mdat = Vec::new();
    for f in frags {
        for s in &f.samples {
            mdat.extend_from_slice(&s.1);
        }
    }
    out.extend_from_slice(&bx(b"mdat", &mdat));
    out
}

/// fMP4 media segments (`moof` + `mdat`) of `seg_secs` each.
fn fmp4_segments(c: &Clip, seg_secs: u32, which: Which) -> Vec<Vec<u8>> {
    let total = c.video.len() as u32 / FPS;
    (0..total / seg_secs)
        .map(|k| {
            let (a, b) = ((k * seg_secs) as i64 * 1_000_000, ((k + 1) * seg_secs) as i64 * 1_000_000);
            let mut frags = Vec::new();
            if which != Which::Audio {
                let s: Vec<_> = c.video.iter().filter(|v| v.0 >= a && v.0 < b).map(|v| (90000 / FPS, v.1.clone(), v.2)).collect();
                frags.push(Frag { track: 1, base: (k * seg_secs) as u64 * 90000, samples: s });
            }
            if which != Which::Video {
                let s: Vec<_> = c.audio.iter().filter(|v| v.0 >= a && v.0 < b).map(|v| (1024u32, v.1.clone(), true)).collect();
                let first = c.audio.iter().position(|v| v.0 >= a).unwrap_or(0) as u64;
                frags.push(Frag { track: 2, base: first * 1024, samples: s });
            }
            moof_mdat(k + 1, &frags)
        })
        .collect()
}

// ---- helpers ----

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("studio-webmedia-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run(url: &str, dir: &std::path::Path, opts: &Options) -> crate::Result<crate::Saved> {
    crate::fetch(url, dir, None, opts, &crate::quiet)
}

struct Summary {
    video: Option<(u32, u32, usize)>,
    audio: Option<usize>,
    secs: f64,
    first_sync: bool,
}

fn open(path: &std::path::Path) -> (studio_mp4::Demuxer, Summary) {
    let d = studio_mp4::open(path).unwrap();
    let m = d.movie();
    let video = m.video().map(|t| (t.width, t.height, t.samples.len()));
    let audio = m.audio().map(|t| t.samples.len());
    let first_sync = m.video().is_some_and(|t| t.samples.first().is_some_and(|s| s.sync));
    let s = Summary { video, audio, secs: m.duration_secs, first_sync };
    (d, s)
}

fn playlist(segs: &[String], dur: f64, extra: &str) -> String {
    let mut s = format!("#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:{}\n#EXT-X-MEDIA-SEQUENCE:0\n{extra}", dur.ceil());
    for u in segs {
        s.push_str(&format!("#EXTINF:{dur:.3},\n{u}\n"));
    }
    s.push_str("#EXT-X-ENDLIST\n");
    s
}

const M3U8: &str = "application/vnd.apple.mpegurl";

fn serve_ts_hls(srv: &Server, base: &str, clip: &Clip, seg_secs: u32) -> Vec<String> {
    let segs = ts_segments(clip, seg_secs, true);
    let names: Vec<String> = (0..segs.len()).map(|i| format!("seg{i}.ts")).collect();
    for (n, s) in names.iter().zip(&segs) {
        srv.add(&format!("{base}/{n}"), "video/mp2t", s.clone());
    }
    srv.add(&format!("{base}/index.m3u8"), M3U8, playlist(&names, seg_secs as f64, "").into_bytes());
    names
}

// ---- tests ----

/// A page that names its master playlist in a script: the best variant is
/// fetched, its transport stream segments are taken apart and written as
/// one MP4 whose samples are the encoder's own.
#[test]
fn hls_from_a_page_best_variant_ts() {
    let srv = Server::start();
    let hi = make_clip(160, 96, 4, "counter");
    let lo = make_clip(96, 64, 4, "bars");
    serve_ts_hls(&srv, "/hi", &hi, 2);
    serve_ts_hls(&srv, "/lo", &lo, 2);
    srv.add("/master.m3u8", M3U8, "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=200000,RESOLUTION=96x64,CODECS=\"avc1.42c00d,mp4a.40.2\"\nlo/index.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=900000,RESOLUTION=160x96,CODECS=\"avc1.4d400d,mp4a.40.2\"\nhi/index.m3u8\n");
    srv.page("/watch", r#"<html><head><title>Beach day</title></head><body><div id="player"></div><script>player.load({"src":"http:\/\/127.0.0.1:PORT\/master.m3u8"})</script></body></html>"#);
    let page = format!("{}", srv.url("/watch"));
    let html = std::fs::read_to_string("/dev/null").unwrap_or_default();
    let _ = html;
    srv.page("/watch", &format!(r#"<html><head><title>Beach day</title></head><body><script>player.load({{"src":"http:\/\/127.0.0.1:{}\/master.m3u8"}})</script></body></html>"#, srv.port));

    let dir = tmp("hls-ts");
    let saved = run(&page, &dir, &Options::default()).unwrap();
    assert_eq!(saved.via, "hls");
    assert_eq!(saved.file.file_name().unwrap(), "Beach day.mp4");
    assert!(saved.chosen.contains("160x96"), "{}", saved.chosen);
    assert_eq!(saved.available.len(), 2);
    let (mut d, s) = open(&saved.file);
    assert_eq!(s.video, Some((160, 96, 120)));
    assert!(s.first_sync);
    assert!(s.audio.unwrap() >= 150, "audio frames {:?}", s.audio);
    assert!((s.secs - 4.0).abs() < 0.2, "{}", s.secs);
    // The first picture is the encoder's first access unit, byte for byte.
    let t = d.movie().video().unwrap().clone();
    assert_eq!(d.read_sample(&t.samples[0]).unwrap(), hi.video[0].1);
    assert_eq!(d.read_sample(&t.samples[119]).unwrap(), hi.video[119].1);
    // The working folder is gone and the page was sent as the referer.
    assert!(std::fs::read_dir(&dir).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().starts_with(".studio-parts")));
    assert!(srv.requests().iter().any(|r| r.path.starts_with("/hi/seg0.ts") && r.referer.as_deref().is_some_and(|x| x.ends_with("/watch"))));
    assert_eq!(srv.hits("/lo/seg"), 0);

    // The user asks for the smallest.
    let dir2 = tmp("hls-ts-low");
    let low = run(&srv.url("/watch"), &dir2, &Options { quality: Quality::Lowest, ..Options::default() }).unwrap();
    let (_, s) = open(&low.file);
    assert_eq!(s.video.map(|v| (v.0, v.1)), Some((96, 64)));
    // Or sound only.
    let dir3 = tmp("hls-ts-audio");
    let snd = run(&srv.url("/watch"), &dir3, &Options { quality: Quality::AudioOnly, ..Options::default() }).unwrap();
    assert_eq!(snd.file.extension().unwrap(), "m4a");
    let (_, s) = open(&snd.file);
    assert!(s.video.is_none() && s.audio.is_some());
}

#[test]
fn hls_with_an_open_aes_128_key() {
    let srv = Server::start();
    let clip = make_clip(96, 64, 4, "counter");
    let key = [0x42u8; 16];
    let aes = crate::aes::Aes128::new(&key);
    let segs = ts_segments(&clip, 2, true);
    let names: Vec<String> = (0..segs.len()).map(|i| format!("e{i}.ts")).collect();
    for (i, (n, s)) in names.iter().zip(&segs).enumerate() {
        // No IV in the playlist: the segment's sequence number is the IV.
        srv.add(&format!("/enc/{n}"), "video/mp2t", aes.cbc_encrypt(&(i as u128).to_be_bytes(), s));
    }
    srv.add("/enc/key.bin", "application/octet-stream", key.to_vec());
    srv.add("/enc/index.m3u8", M3U8, playlist(&names, 2.0, "#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\"\n").into_bytes());
    let dir = tmp("hls-aes");
    let saved = run(&srv.url("/enc/index.m3u8"), &dir, &Options::default()).unwrap();
    let (mut d, s) = open(&saved.file);
    assert_eq!(s.video, Some((96, 64, 120)));
    let t = d.movie().video().unwrap().clone();
    assert_eq!(d.read_sample(&t.samples[60]).unwrap(), clip.video[60].1);
}

#[test]
fn hls_fmp4_with_separate_sound_and_byte_ranges() {
    let srv = Server::start();
    let clip = make_clip(96, 64, 4, "counter");
    // Video: init + fragments in ONE file, addressed by byte ranges.
    let init = fmp4_init(&clip, Which::Video);
    let frags = fmp4_segments(&clip, 2, Which::Video);
    let mut file = init.clone();
    let mut ranges = Vec::new();
    for f in &frags {
        ranges.push((file.len(), f.len()));
        file.extend_from_slice(f);
    }
    srv.add("/f/video.mp4", "video/mp4", file);
    let mut pl = "#EXTM3U\n#EXT-X-VERSION:6\n#EXT-X-TARGETDURATION:2\n".to_string();
    pl.push_str(&format!("#EXT-X-MAP:URI=\"video.mp4\",BYTERANGE=\"{}@0\"\n", init.len()));
    for (o, l) in &ranges {
        pl.push_str(&format!("#EXTINF:2.0,\n#EXT-X-BYTERANGE:{l}@{o}\nvideo.mp4\n"));
    }
    pl.push_str("#EXT-X-ENDLIST\n");
    srv.add("/f/v.m3u8", M3U8, pl.into_bytes());
    // Sound: separate playlist with its own init and fragments.
    srv.add("/f/a-init.mp4", "audio/mp4", fmp4_init(&clip, Which::Audio));
    let afr = fmp4_segments(&clip, 2, Which::Audio);
    let anames: Vec<String> = (0..afr.len()).map(|i| format!("a{i}.m4s")).collect();
    for (n, f) in anames.iter().zip(&afr) {
        srv.add(&format!("/f/{n}"), "audio/mp4", f.clone());
    }
    srv.add("/f/a.m3u8", M3U8, playlist(&anames, 2.0, "#EXT-X-MAP:URI=\"a-init.mp4\"\n").into_bytes());
    srv.add("/f/master.m3u8", M3U8, "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"Main\",LANGUAGE=\"en\",DEFAULT=YES,URI=\"a.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=500000,RESOLUTION=96x64,CODECS=\"avc1.42c00d,mp4a.40.2\",AUDIO=\"a\"\nv.m3u8\n");
    let dir = tmp("hls-fmp4");
    let saved = run(&srv.url("/f/master.m3u8"), &dir, &Options::default()).unwrap();
    assert!(saved.chosen.contains("96x64") && saved.chosen.contains("Main"), "{}", saved.chosen);
    let (mut d, s) = open(&saved.file);
    assert_eq!(s.video, Some((96, 64, 120)));
    assert!(s.audio.unwrap() >= 150);
    let t = d.movie().video().unwrap().clone();
    assert_eq!(d.read_sample(&t.samples[90]).unwrap(), clip.video[90].1);
    let a = d.movie().audio().unwrap().clone();
    assert_eq!(d.read_sample(&a.samples[3]).unwrap(), clip.audio[3].1);
    // Ranges were really requested.
    assert!(srv.requests().iter().any(|r| r.path == "/f/video.mp4" && r.range.is_some()));
}

#[test]
fn protected_streams_are_refused_plainly() {
    let srv = Server::start();
    srv.add("/p/index.m3u8", M3U8, "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"skd://abc\",KEYFORMAT=\"com.apple.streamingkeydelivery\",KEYFORMATVERSIONS=\"1\"\n#EXTINF:6,\ns0.ts\n#EXT-X-ENDLIST\n");
    srv.add("/p/s0.ts", "video/mp2t", vec![0x47; 188]);
    let err = run(&srv.url("/p/index.m3u8"), &tmp("drm-hls"), &Options::default()).unwrap_err();
    assert!(matches!(err, Error::Protected(_)), "{err}");
    assert!(err.to_string().contains("copy protection"));
    assert_eq!(srv.hits("/p/s0.ts"), 0, "no protected segment is even fetched");

    srv.add("/p/m.mpd", "application/dash+xml", r#"<MPD mediaPresentationDuration="PT4S"><Period><AdaptationSet mimeType="video/mp4"><ContentProtection schemeIdUri="urn:uuid:edef8ba9-79d6-4ace-a3c8-27dcd51d21ed"/><Representation id="1" bandwidth="1"><BaseURL>v.mp4</BaseURL></Representation></AdaptationSet></Period></MPD>"#);
    let err = run(&srv.url("/p/m.mpd"), &tmp("drm-dash"), &Options::default()).unwrap_err();
    assert!(matches!(err, Error::Protected(_)), "{err}");
    assert_eq!(srv.hits("/p/v.mp4"), 0);

    // A page whose player is set up for Widevine and exposes nothing else.
    srv.page("/p/page", r#"<script>shaka.configure({drm:{servers:{"com.widevine.alpha":"https://lic.example/x"}}})</script>"#);
    let err = run(&srv.url("/p/page"), &tmp("drm-page"), &Options::default()).unwrap_err();
    assert!(matches!(err, Error::Protected(_)), "{err}");
    assert!(err.to_string().contains("Widevine"));

    // Encrypted pieces inside an otherwise ordinary DASH file.
    let clip = make_clip(96, 64, 2, "counter");
    let mut init = fmp4_init(&clip, Which::Video);
    let at = init.windows(4).position(|w| w == b"avc1").unwrap();
    init[at..at + 4].copy_from_slice(b"encv");
    srv.add("/p/enc-init.mp4", "video/mp4", init);
    srv.add("/p/enc-seg.m4s", "video/mp4", fmp4_segments(&clip, 2, Which::Video).remove(0));
    srv.add("/p/enc.mpd", "application/dash+xml", r#"<MPD mediaPresentationDuration="PT2S"><Period><AdaptationSet mimeType="video/mp4"><Representation id="1" bandwidth="1" height="64"><SegmentList duration="2"><Initialization sourceURL="enc-init.mp4"/><SegmentURL media="enc-seg.m4s"/></SegmentList></Representation></AdaptationSet></Period></MPD>"#);
    let err = run(&srv.url("/p/enc.mpd"), &tmp("drm-enc"), &Options::default()).unwrap_err();
    assert!(matches!(err, Error::Protected(_)), "{err}");
}

#[test]
fn dash_template_video_and_sound_become_one_mp4() {
    let srv = Server::start();
    let clip = make_clip(96, 64, 4, "counter");
    srv.add("/d/v/init.mp4", "video/mp4", fmp4_init(&clip, Which::Video));
    for (i, f) in fmp4_segments(&clip, 2, Which::Video).into_iter().enumerate() {
        srv.add(&format!("/d/v/{}.m4s", i + 1), "video/mp4", f);
    }
    srv.add("/d/a/init.mp4", "audio/mp4", fmp4_init(&clip, Which::Audio));
    for (i, f) in fmp4_segments(&clip, 2, Which::Audio).into_iter().enumerate() {
        srv.add(&format!("/d/a/{}.m4s", i + 1), "audio/mp4", f);
    }
    // A lower, unused video representation too.
    srv.add("/d/manifest.mpd", "application/dash+xml", r#"<?xml version="1.0"?><MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT4S">
<Period>
<AdaptationSet contentType="video" mimeType="video/mp4">
 <SegmentTemplate timescale="1" duration="2" startNumber="1" initialization="$RepresentationID$/init.mp4" media="$RepresentationID$/$Number$.m4s"/>
 <Representation id="v" bandwidth="800000" width="96" height="64" codecs="avc1.42c00d"/>
 <Representation id="vlow" bandwidth="100000" width="48" height="32" codecs="avc1.42c00d"/>
</AdaptationSet>
<AdaptationSet contentType="audio" mimeType="audio/mp4" lang="en">
 <SegmentTemplate timescale="1" duration="2" startNumber="1" initialization="$RepresentationID$/init.mp4" media="$RepresentationID$/$Number$.m4s"/>
 <Representation id="a" bandwidth="128000" codecs="mp4a.40.2"/>
</AdaptationSet>
</Period></MPD>"#);
    srv.page("/dash-page", r#"<html><head><link rel="alternate" type="application/dash+xml" href="/d/manifest.mpd"><title>Dash clip</title></head><body></body></html>"#);
    let dir = tmp("dash-tpl");
    let saved = run(&srv.url("/dash-page"), &dir, &Options::default()).unwrap();
    assert_eq!(saved.via, "dash");
    assert_eq!(saved.file.file_name().unwrap(), "Dash clip.mp4");
    let (mut d, s) = open(&saved.file);
    assert_eq!(s.video, Some((96, 64, 120)));
    assert!(s.audio.unwrap() >= 150);
    assert!((s.secs - 4.0).abs() < 0.2, "{}", s.secs);
    let t = d.movie().video().unwrap().clone();
    assert_eq!(d.read_sample(&t.samples[100]).unwrap(), clip.video[100].1);
    assert_eq!(srv.hits("/d/vlow/"), 0);
}

#[test]
fn dash_segment_list_and_segment_base() {
    let srv = Server::start();
    let clip = make_clip(96, 64, 4, "counter");
    // SegmentList with byte ranges in one file.
    let init = fmp4_init(&clip, Which::Video);
    let frags = fmp4_segments(&clip, 2, Which::Video);
    let mut file = init.clone();
    let mut ranges = Vec::new();
    for f in &frags {
        ranges.push((file.len(), file.len() + f.len() - 1));
        file.extend_from_slice(f);
    }
    srv.add("/l/video.mp4", "video/mp4", file.clone());
    let segs: String = ranges.iter().map(|(a, b)| format!("<SegmentURL mediaRange=\"{a}-{b}\"/>")).collect();
    srv.add("/l/list.mpd", "application/dash+xml", format!(r#"<MPD mediaPresentationDuration="PT4S"><Period><AdaptationSet mimeType="video/mp4"><Representation id="1" bandwidth="1" width="96" height="64"><BaseURL>video.mp4</BaseURL><SegmentList duration="2"><Initialization range="0-{}"/>{segs}</SegmentList></Representation></AdaptationSet></Period></MPD>"#, init.len() - 1));
    let saved = run(&srv.url("/l/list.mpd"), &tmp("dash-list"), &Options::default()).unwrap();
    let (_, s) = open(&saved.file);
    assert_eq!(s.video, Some((96, 64, 120)));
    assert!(s.audio.is_none());
    assert!(srv.requests().iter().any(|r| r.path == "/l/video.mp4" && r.range.as_deref().is_some_and(|x| x.starts_with("bytes=0-"))));
    // SegmentBase: one file with its index; fetched whole.
    srv.add("/l/base.mpd", "application/dash+xml", r#"<MPD mediaPresentationDuration="PT4S"><Period><AdaptationSet mimeType="video/mp4"><Representation id="1" bandwidth="1" width="96" height="64"><BaseURL>video.mp4</BaseURL><SegmentBase indexRange="0-10"><Initialization range="0-9"/></SegmentBase></Representation></AdaptationSet></Period></MPD>"#);
    let saved = run(&srv.url("/l/base.mpd"), &tmp("dash-base"), &Options::default()).unwrap();
    let (mut d, s) = open(&saved.file);
    assert_eq!(s.video, Some((96, 64, 120)));
    let t = d.movie().video().unwrap().clone();
    assert_eq!(d.read_sample(&t.samples[7]).unwrap(), clip.video[7].1);
}

#[test]
fn files_named_by_elements_meta_and_data() {
    let srv = Server::start();
    srv.add("/m/a.mp4", "video/mp4", vec![1u8; 3000]);
    srv.add("/m/hd.mp4", "video/mp4", vec![2u8; 4000]);
    srv.add("/m/ld.webm", "video/webm", vec![3u8; 1000]);
    srv.add("/m/og.mp4", "video/mp4", vec![4u8; 2000]);
    // <video>/<source> with sizes: the tallest first.
    srv.page("/m/video", r#"<title>Two sizes</title><video controls><source src="a.mp4" type="video/mp4" size="480"><source src="hd.mp4" type="video/mp4" size="1080"></video>"#);
    let saved = run(&srv.url("/m/video"), &tmp("el"), &Options::default()).unwrap();
    assert_eq!(saved.via, "file");
    assert_eq!(std::fs::read(&saved.file).unwrap().len(), 4000);
    let small = run(&srv.url("/m/video"), &tmp("el-low"), &Options { quality: Quality::Height(480), ..Options::default() }).unwrap();
    assert_eq!(std::fs::read(&small.file).unwrap().len(), 3000);
    // JSON-LD VideoObject.
    srv.page("/m/ld", &format!(r#"<script type="application/ld+json">{{"@type":"VideoObject","name":"From data","contentUrl":"{}"}}</script>"#, srv.url("/m/ld.webm")));
    let saved = run(&srv.url("/m/ld"), &tmp("ld"), &Options::default()).unwrap();
    assert_eq!(saved.file.file_name().unwrap(), "From data.webm");
    // Open Graph.
    srv.page("/m/og", r#"<meta property="og:video" content="/m/og.mp4"><meta property="og:video:type" content="video/mp4"><meta property="og:title" content="OG clip">"#);
    let saved = run(&srv.url("/m/og"), &tmp("og"), &Options::default()).unwrap();
    assert_eq!(saved.file.file_name().unwrap(), "OG clip.mp4");
    assert_eq!(std::fs::read(&saved.file).unwrap(), vec![4u8; 2000]);
    // Twitter stream.
    srv.page("/m/tw", r#"<meta name="twitter:player:stream" content="/m/a.mp4"><meta name="twitter:player:stream:content_type" content="video/mp4">"#);
    let saved = run(&srv.url("/m/tw"), &tmp("tw"), &Options::default()).unwrap();
    assert_eq!(std::fs::read(&saved.file).unwrap().len(), 3000);
}

#[test]
fn oembed_and_embedded_players_are_followed() {
    let srv = Server::start();
    srv.add("/o/real.mp4", "video/mp4", vec![9u8; 2500]);
    // The page itself exposes nothing; its oEmbed names an iframe; the
    // iframe's page has the video.
    srv.page("/o/page", &format!(r#"<title>Page</title><link rel="alternate" type="application/json+oembed" href="{}">"#, srv.url("/o/oembed?url=x")));
    srv.add("/o/oembed", "application/json", format!(r#"{{"type":"video","title":"Via oEmbed","html":"<iframe src=\"{}\" width=\"640\"></iframe>"}}"#, srv.url("/o/player")).into_bytes());
    srv.page("/o/player", r#"<html><body><video src="real.mp4"></video></body></html>"#);
    let saved = run(&srv.url("/o/page"), &tmp("oembed"), &Options::default()).unwrap();
    assert_eq!(saved.file.file_name().unwrap(), "Via oEmbed.mp4");
    assert_eq!(std::fs::read(&saved.file).unwrap().len(), 2500);
    // A plain iframe, two steps down.
    srv.page("/o/outer", r#"<iframe src="/o/mid"></iframe>"#);
    srv.page("/o/mid", r#"<iframe src="/o/player"></iframe>"#);
    let saved = run(&srv.url("/o/outer"), &tmp("frames"), &Options::default()).unwrap();
    assert_eq!(std::fs::read(&saved.file).unwrap().len(), 2500);
    // An og:video that is an embed page.
    srv.page("/o/ogembed", &format!(r#"<meta property="og:video" content="{}"><meta property="og:video:type" content="text/html">"#, srv.url("/o/player")));
    let saved = run(&srv.url("/o/ogembed"), &tmp("ogembed"), &Options::default()).unwrap();
    assert_eq!(std::fs::read(&saved.file).unwrap().len(), 2500);
}

#[test]
fn a_page_with_nothing_says_so_and_never_names_another_program() {
    let srv = Server::start();
    srv.page("/n/none", "<html><body><h1>Hello</h1><p>No video here. See clip.mp4 maybe.</p></body></html>");
    let err = run(&srv.url("/n/none"), &tmp("none"), &Options::default()).unwrap_err();
    assert!(matches!(err, Error::NotExposed(_)));
    let low = err.to_string().to_ascii_lowercase();
    for banned in ["yt-dlp", "youtube-dl", "ffmpeg", "curl", "wget", "brew"] {
        assert!(!low.contains(banned), "{low}");
    }
    assert!(low.contains("openly"));
    srv.page("/n/blob", r#"<video src="blob:http://x/1234"></video>"#);
    let err = run(&srv.url("/n/blob"), &tmp("blob"), &Options::default()).unwrap_err();
    assert!(err.to_string().contains("inside the browser"), "{err}");
    // Sign-in walls are reported as such, not worked around.
    srv.add("/n/private.m3u8", M3U8, "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2,\nseg.ts\n#EXT-X-ENDLIST\n");
    srv.add("/n/seg.ts", "video/mp2t", vec![0u8; 10]);
    srv.set_status("/n/seg.ts", 403);
    let err = run(&srv.url("/n/private.m3u8"), &tmp("403"), &Options::default()).unwrap_err();
    assert!(matches!(err, Error::SignIn(_)), "{err}");
    assert!(err.to_string().contains("does not sign in"));
}

#[test]
fn redirects_move_the_base_for_relative_addresses() {
    let srv = Server::start();
    srv.add("/r/deep/clip.mp4", "video/mp4", vec![5u8; 1500]);
    srv.page("/r/deep/page", r#"<video src="clip.mp4"></video>"#);
    srv.redirect("/short", "/r/deep/page");
    let saved = run(&srv.url("/short"), &tmp("redirect"), &Options::default()).unwrap();
    assert_eq!(std::fs::read(&saved.file).unwrap().len(), 1500);
}

#[test]
fn downloads_run_in_parallel_up_to_the_cap_and_resume() {
    let srv = Server::start();
    let clip = make_clip(96, 64, 6, "counter");
    let names = serve_ts_hls(&srv, "/par", &clip, 1);
    assert_eq!(names.len(), 6);
    srv.set_delay("/par/seg", 80);
    let dir = tmp("par");
    let saved = run(&srv.url("/par/index.m3u8"), &dir, &Options { parallel: 3, ..Options::default() }).unwrap();
    let peak = srv.max_inflight.load(Ordering::SeqCst);
    assert!((2..=3).contains(&peak), "peak {peak}");
    let (_, s) = open(&saved.file);
    assert_eq!(s.video.unwrap().2, 180);
    let _ = std::fs::remove_file(&saved.file);

    // Resume: the fourth segment fails on the first try; the pieces that
    // arrived are not fetched again on the second.
    let srv = Server::start();
    serve_ts_hls(&srv, "/res", &clip, 1);
    srv.set_status("/res/seg3.ts", 500);
    let dir = tmp("res");
    let err = run(&srv.url("/res/index.m3u8"), &dir, &Options { parallel: 1, ..Options::default() }).unwrap_err();
    assert!(matches!(err, Error::Net(_)), "{err}");
    assert!(std::fs::read_dir(&dir).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with(".studio-parts")));
    let before: Vec<usize> = (0..6).map(|i| srv.hits(&format!("/res/seg{i}.ts"))).collect();
    assert_eq!(&before[..3], &[1, 1, 1]);
    srv.set_status("/res/seg3.ts", 200);
    let saved = run(&srv.url("/res/index.m3u8"), &dir, &Options { parallel: 1, ..Options::default() }).unwrap();
    for i in 0..3 {
        assert_eq!(srv.hits(&format!("/res/seg{i}.ts")), 1, "segment {i} was fetched again");
    }
    let (_, s) = open(&saved.file);
    assert_eq!(s.video.unwrap().2, 180);
}

#[test]
fn cancelling_stops_and_keeps_what_arrived() {
    let srv = Server::start();
    let clip = make_clip(96, 64, 6, "counter");
    serve_ts_hls(&srv, "/c", &clip, 1);
    let dir = tmp("cancel");
    let seen = AtomicUsize::new(0);
    let report = |p: &Progress| {
        if p.stage == "fetching" && p.done >= 2 {
            seen.fetch_add(1, Ordering::SeqCst);
            return false;
        }
        true
    };
    let err = crate::fetch(&srv.url("/c/index.m3u8"), &dir, None, &Options { parallel: 1, ..Options::default() }, &report).unwrap_err();
    assert_eq!(err, Error::Cancelled);
    assert!(srv.hits("/c/seg") < 6);
    assert!(std::fs::read_dir(&dir).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with(".studio-parts")));
}

#[test]
fn packed_audio_and_discontinuities() {
    let srv = Server::start();
    let clip = make_clip(96, 64, 4, "counter");
    // Sound-only HLS: ADTS frames behind ID3 tags.
    let names: Vec<String> = (0..2).map(|i| format!("p{i}.aac")).collect();
    for (i, n) in names.iter().enumerate() {
        let mut seg = b"ID3\x04\0\0\0\0\0\0".to_vec();
        seg.extend_from_slice(&adts_frames(&clip, i as i64 * 2_000_000, (i as i64 + 1) * 2_000_000));
        srv.add(&format!("/pk/{n}"), "audio/aac", seg);
    }
    srv.add("/pk/index.m3u8", M3U8, playlist(&names, 2.0, "").into_bytes());
    let saved = run(&srv.url("/pk/index.m3u8"), &tmp("packed"), &Options::default()).unwrap();
    assert_eq!(saved.file.extension().unwrap(), "m4a");
    let (mut d, s) = open(&saved.file);
    assert!(s.video.is_none());
    let a = d.movie().audio().unwrap().clone();
    assert_eq!(a.samples.len(), clip.audio.iter().filter(|x| x.0 < 4_000_000).count());
    assert_eq!(d.read_sample(&a.samples[5]).unwrap(), clip.audio[5].1);

    // A discontinuity: the second part's clock starts from zero again;
    // the picture still runs on without a gap or a jump.
    let segs = ts_segments(&clip, 2, true);
    // Re-time the second segment by building it from the first's packets.
    let again = ts_segments(&Clip { w: clip.w, h: clip.h, sps: clip.sps.clone(), pps: clip.pps.clone(), asc: clip.asc.clone(), video: clip.video.iter().take(60).cloned().collect(), audio: clip.audio.iter().filter(|a| a.0 < 2_000_000).cloned().collect() }, 2, true);
    srv.add("/dc/a.ts", "video/mp2t", segs[0].clone());
    srv.add("/dc/b.ts", "video/mp2t", again[0].clone());
    srv.add("/dc/index.m3u8", M3U8, "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2,\na.ts\n#EXT-X-DISCONTINUITY\n#EXTINF:2,\nb.ts\n#EXT-X-ENDLIST\n");
    let saved = run(&srv.url("/dc/index.m3u8"), &tmp("disc"), &Options::default()).unwrap();
    let (d, s) = open(&saved.file);
    assert_eq!(s.video.unwrap().2, 120);
    assert!((s.secs - 4.0).abs() < 0.2, "{}", s.secs);
    let t = d.movie().video().unwrap();
    let last = t.samples.last().unwrap();
    assert!(last.dts >= 3 * 90000, "the second part continues after the first: {}", last.dts);
}

#[test]
fn a_live_playlist_is_saved_as_listed_with_a_note() {
    let srv = Server::start();
    let clip = make_clip(96, 64, 4, "counter");
    let names = serve_ts_hls(&srv, "/live", &clip, 2);
    let mut pl = playlist(&names, 2.0, "");
    pl = pl.replace("#EXT-X-ENDLIST\n", "");
    srv.add("/live/index.m3u8", M3U8, pl.into_bytes());
    let saved = run(&srv.url("/live/index.m3u8"), &tmp("live"), &Options::default()).unwrap();
    assert!(saved.notes.iter().any(|n| n.contains("no end marker")), "{:?}", saved.notes);
}
