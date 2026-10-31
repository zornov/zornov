// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! End-to-end tests against a local web server, behind HTML pages. No real
//! internet is used.

use crate::{Error, Options, Quality};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

// ---- a local web server ----

#[derive(Clone)]
struct Route {
    ct: String,
    body: Arc<Vec<u8>>,
    status: u16,
    location: Option<String>,
}

struct Server {
    port: u16,
    routes: Arc<Mutex<HashMap<String, Route>>>,
    stop: Arc<AtomicBool>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn respond(mut s: TcpStream, routes: &Mutex<HashMap<String, Route>>) {
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
    let key = path.split('?').next().unwrap_or("").to_owned();
    let route = routes.lock().unwrap().get(&key).cloned();
    match route {
        None => {
            let _ = write!(s, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
        Some(r) => {
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
}

impl Server {
    fn start() -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let routes: Arc<Mutex<HashMap<String, Route>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        {
            let (routes, stop) = (routes.clone(), stop.clone());
            std::thread::spawn(move || {
                for conn in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        return;
                    }
                    if let Ok(c) = conn {
                        let routes = routes.clone();
                        std::thread::spawn(move || respond(c, &routes));
                    }
                }
            });
        }
        Server { port, routes, stop }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    fn add(&self, path: &str, ct: &str, body: impl Into<Vec<u8>>) {
        self.routes.lock().unwrap().insert(path.to_owned(), Route { ct: ct.to_owned(), body: Arc::new(body.into()), status: 200, location: None });
    }

    fn page(&self, path: &str, html: &str) {
        self.add(path, "text/html; charset=utf-8", html.as_bytes());
    }

    fn set_status(&self, path: &str, status: u16) {
        if let Some(r) = self.routes.lock().unwrap().get_mut(path) {
            r.status = status;
        }
    }

    fn redirect(&self, path: &str, to: &str) {
        self.routes.lock().unwrap().insert(path.to_owned(), Route { ct: "text/plain".into(), body: Arc::default(), status: 200, location: Some(to.to_owned()) });
    }
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

const M3U8: &str = "application/vnd.apple.mpegurl";

// ---- tests ----

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
