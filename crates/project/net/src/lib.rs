// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! HTTPS for Studio, in Rust (it ran the system `curl` before).
//!
//! * [`post`]: a JSON (or any) body to an API; the reply's status and a
//!   reader for its body, streamed as it arrives (server-sent events from
//!   the Claude and OpenAI APIs are read line by line as they come).
//! * [`download`]: a file to disk, resuming a partial one, with progress,
//!   cancelling, a size limit and stall detection (a transfer that sends
//!   nothing for a while gives up instead of hanging).
//!
//! TLS is rustls with the certificates this computer trusts (so company
//! proxies with their own root work as they did with curl). Proxies in
//! `HTTPS_PROXY` / `ALL_PROXY` are honoured. Secrets travel only in
//! request headers, never on a command line or in a temporary file.

use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

/// A reply: its HTTP status and its body, read as it arrives.
pub struct Reply {
    pub status: u16,
    /// The `Content-Type` header, if any (`image/jpeg`, `text/html; ...`).
    pub content_type: Option<String>,
    /// The address the reply came from, after any redirects.
    pub url: String,
    pub body: Box<dyn Read + Send + 'static>,
}

impl Reply {
    /// The whole body (up to `limit` bytes).
    pub fn bytes(mut self, limit: u64) -> std::io::Result<Vec<u8>> {
        let mut v = Vec::new();
        (&mut self.body).take(limit).read_to_end(&mut v)?;
        Ok(v)
    }
}

/// One agent per read timeout, kept for the life of the program: its
/// connection pool lets many small requests to one server (the segments of
/// a stream) share a connection instead of a new TLS handshake each.
fn agent(read_timeout: Duration) -> ureq::Agent {
    static AGENTS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<u64, ureq::Agent>>> = std::sync::OnceLock::new();
    let mut agents = AGENTS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    agents.entry(read_timeout.as_secs()).or_insert_with(|| build_agent(read_timeout)).clone()
}

fn build_agent(read_timeout: Duration) -> ureq::Agent {
    let mut b = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(20)).timeout_read(read_timeout).timeout_write(Duration::from_secs(60)).redirects(8);
    for var in ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"] {
        if let Some(p) = std::env::var(var).ok().filter(|v| !v.is_empty()).and_then(|v| ureq::Proxy::new(v).ok()) {
            b = b.proxy(p);
            break;
        }
    }
    b.build()
}

/// The error text for a failed connection (no HTTP status).
fn transport(url: &str, e: &ureq::Transport) -> String {
    let host = url.split('/').take(3).collect::<Vec<_>>().join("/");
    format!("could not reach {host}: {e}")
}

/// POSTs `body` to `url` with `headers`. Any HTTP status comes back as a
/// [`Reply`] (the caller reads the error body); only a failed connection
/// is an `Err`. `quiet` is how long the reply may send nothing before the
/// read gives up.
pub fn post(url: &str, headers: &[(&str, &str)], body: &[u8], quiet: Duration) -> Result<Reply, String> {
    let mut req = agent(quiet).post(url);
    for (k, v) in headers {
        req = req.set(k, v);
    }
    match req.send_bytes(body) {
        Ok(r) => Ok(Reply { status: r.status(), content_type: r.header("Content-Type").map(str::to_owned), url: r.get_url().to_owned(), body: Box::new(r.into_reader()) }),
        Err(ureq::Error::Status(code, r)) => Ok(Reply { status: code, content_type: r.header("Content-Type").map(str::to_owned), url: r.get_url().to_owned(), body: Box::new(r.into_reader()) }),
        Err(ureq::Error::Transport(t)) => Err(transport(url, &t)),
    }
}

/// GETs `url` with `headers` (same rules as [`post`]).
pub fn get(url: &str, headers: &[(&str, &str)], quiet: Duration) -> Result<Reply, String> {
    let mut req = agent(quiet).get(url);
    for (k, v) in headers {
        req = req.set(k, v);
    }
    match req.call() {
        Ok(r) => Ok(Reply { status: r.status(), content_type: r.header("Content-Type").map(str::to_owned), url: r.get_url().to_owned(), body: Box::new(r.into_reader()) }),
        Err(ureq::Error::Status(code, r)) => Ok(Reply { status: code, content_type: r.header("Content-Type").map(str::to_owned), url: r.get_url().to_owned(), body: Box::new(r.into_reader()) }),
        Err(ureq::Error::Transport(t)) => Err(transport(url, &t)),
    }
}

/// One GET hop made by [`get_pinned`]: the reply, and where a redirect
/// (3xx) points (the `Location` header, as sent: maybe relative).
pub struct Hop {
    pub reply: Reply,
    pub location: Option<String>,
}

/// GETs `url` once, with no redirects followed, connecting only to
/// `addrs` (addresses the caller has checked) instead of resolving the
/// name again, so a DNS answer that changes between the check and the
/// connection cannot reach elsewhere; TLS and `Host` still use the name.
/// A redirect comes back as a [`Hop`] for the caller to vet and follow.
/// With a proxy configured (`HTTPS_PROXY`), the proxy makes the
/// connection and so the addresses cannot be pinned.
pub fn get_pinned(url: &str, headers: &[(&str, &str)], quiet: Duration, addrs: Vec<std::net::SocketAddr>) -> Result<Hop, String> {
    get_pinned_with(url, headers, quiet, addrs, true)
}

fn get_pinned_with(url: &str, headers: &[(&str, &str)], quiet: Duration, addrs: Vec<std::net::SocketAddr>, env_proxy: bool) -> Result<Hop, String> {
    let mut b = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(20)).timeout_read(quiet).timeout_write(Duration::from_secs(60)).redirects(0);
    let proxy = env_proxy.then(|| ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"].iter().find_map(|var| std::env::var(var).ok().filter(|v| !v.is_empty()).and_then(|v| ureq::Proxy::new(v).ok()))).flatten();
    match proxy {
        Some(p) => b = b.proxy(p),
        None => b = b.resolver(move |_: &str| Ok(addrs.clone())),
    }
    let mut req = b.build().get(url);
    for (k, v) in headers {
        req = req.set(k, v);
    }
    let (status, r) = match req.call() {
        Ok(r) => (r.status(), r),
        Err(ureq::Error::Status(code, r)) => (code, r),
        Err(ureq::Error::Transport(t)) => return Err(transport(url, &t)),
    };
    let location = r.header("Location").map(str::to_owned);
    Ok(Hop { location, reply: Reply { status, content_type: r.header("Content-Type").map(str::to_owned), url: r.get_url().to_owned(), body: Box::new(r.into_reader()) } })
}

/// How a download behaves.
#[derive(Clone, Copy, Debug)]
pub struct Download {
    /// Continue a partial file at `dest` (a `Range` request).
    pub resume: bool,
    /// Largest file accepted.
    pub max_bytes: u64,
    /// Give up when nothing arrives for this long.
    pub stall: Duration,
    /// Give up after this long in all.
    pub total: Duration,
    /// Tries after a dropped connection (each resumes).
    pub retries: u32,
}

impl Default for Download {
    fn default() -> Self {
        Download { resume: true, max_bytes: 4 << 30, stall: Duration::from_secs(30), total: Duration::from_secs(1800), retries: 3 }
    }
}

/// Why a download stopped.
#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    /// `progress` returned false.
    Cancelled,
    /// Anything else, in words.
    Failed(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Cancelled => f.write_str("cancelled"),
            Failure::Failed(s) => f.write_str(s),
        }
    }
}

/// Downloads `url` into `dest`. `progress(bytes in the file)` is called as
/// data arrives and returns `false` to cancel (what is written stays, for
/// a later resume).
pub fn download(url: &str, dest: &Path, opts: Download, progress: &mut dyn FnMut(u64) -> bool) -> Result<(), Failure> {
    let started = Instant::now();
    let mut tries = 0;
    loop {
        match once(url, dest, &opts, started, progress) {
            Err(Failure::Failed(e)) if e.starts_with("dropped:") && tries < opts.retries && opts.resume => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(500 * tries as u64));
            }
            Err(Failure::Failed(e)) => return Err(Failure::Failed(e.trim_start_matches("dropped: ").to_owned())),
            other => return other,
        }
    }
}

fn once(url: &str, dest: &Path, opts: &Download, started: Instant, progress: &mut dyn FnMut(u64) -> bool) -> Result<(), Failure> {
    let have = if opts.resume { std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0) } else { 0 };
    let mut req = agent(opts.stall).get(url);
    if have > 0 {
        req = req.set("Range", &format!("bytes={have}-"));
    }
    let r = match req.call() {
        Ok(r) => r,
        // Asked to resume a file that is already whole.
        Err(ureq::Error::Status(416, _)) if have > 0 => {
            progress(have);
            return Ok(());
        }
        Err(ureq::Error::Status(code, _)) => return Err(Failure::Failed(format!("the server answered {code} for {url}"))),
        Err(ureq::Error::Transport(t)) => return Err(Failure::Failed(format!("dropped: {}", transport(url, &t)))),
    };
    // 206: the rest of the file; 200: all of it (the server cannot resume).
    let append = r.status() == 206 && have > 0;
    let len: Option<u64> = r.header("Content-Length").and_then(|v| v.parse().ok());
    if len.is_some_and(|l| l + if append { have } else { 0 } > opts.max_bytes) {
        return Err(Failure::Failed(format!("the file is larger than {} MB", opts.max_bytes >> 20)));
    }
    let mut file = std::fs::OpenOptions::new().create(true).write(true).append(append).truncate(!append).open(dest).map_err(|e| Failure::Failed(format!("cannot write {}: {e}", dest.display())))?;
    let mut size = if append { have } else { 0 };
    if !progress(size) {
        return Err(Failure::Cancelled);
    }
    let mut body = r.into_reader();
    let mut buf = vec![0u8; 256 << 10];
    loop {
        if started.elapsed() > opts.total {
            return Err(Failure::Failed(format!("the download took longer than {} minutes", opts.total.as_secs() / 60)));
        }
        let n = match body.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) => {
                return Err(Failure::Failed(format!("the download stalled: nothing arrived for {} s", opts.stall.as_secs())));
            }
            Err(e) => return Err(Failure::Failed(format!("dropped: the connection broke ({e})"))),
        };
        file.write_all(&buf[..n]).map_err(|e| Failure::Failed(format!("cannot write {}: {e}", dest.display())))?;
        size += n as u64;
        if size > opts.max_bytes {
            return Err(Failure::Failed(format!("the file is larger than {} MB", opts.max_bytes >> 20)));
        }
        if !progress(size) {
            return Err(Failure::Cancelled);
        }
    }
    if let Some(l) = len {
        let want = l + if append { have } else { 0 };
        if size < want {
            return Err(Failure::Failed(format!("dropped: the connection closed at {size} of {want} bytes")));
        }
    }
    file.flush().map_err(|e| Failure::Failed(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::net::TcpListener;

    /// A tiny HTTP server on localhost: answers each connection with
    /// `respond(request head)`, for a set number of connections.
    fn serve(conns: usize, respond: impl Fn(&str, &mut std::net::TcpStream) + Send + Sync + 'static) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let respond = std::sync::Arc::new(respond);
        std::thread::spawn(move || {
            for s in l.incoming().take(conns) {
                let respond = respond.clone();
                std::thread::spawn(move || {
                let mut s = s.unwrap();
                let mut head = String::new();
                let mut r = std::io::BufReader::new(s.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    head.push_str(&line);
                }
                if let Some(n) = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap())) {
                    let mut b = vec![0; n];
                    r.read_exact(&mut b).unwrap();
                    head.push_str(&String::from_utf8_lossy(&b));
                }
                respond(&head, &mut s);
                });
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn post_sends_headers_and_body_and_streams_the_reply() {
        let url = serve(1, |head, s| {
            assert!(head.contains("x-api-key: secret"), "{head}");
            assert!(head.ends_with("{\"hi\":1}"), "{head}");
            write!(s, "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\ndata: one\n\ndata: two\n\n").unwrap();
        });
        let r = post(&format!("{url}/v1"), &[("x-api-key", "secret")], b"{\"hi\":1}", Duration::from_secs(5)).unwrap();
        assert_eq!(r.status, 200);
        let lines: Vec<String> = std::io::BufReader::new(r.body).lines().map_while(Result::ok).filter(|l| !l.is_empty()).collect();
        assert_eq!(lines, ["data: one", "data: two"]);
    }

    #[test]
    fn error_statuses_come_back_with_their_body() {
        let url = serve(1, |_, s| {
            let body = "{\"error\":\"no key!\"}";
            write!(s, "HTTP/1.1 401 Unauthorized\r\ncontent-length: {}\r\n\r\n{body}", body.len()).unwrap();
        });
        let r = post(&url, &[], b"x", Duration::from_secs(5)).unwrap();
        assert_eq!(r.status, 401);
        assert_eq!(r.bytes(1000).unwrap(), b"{\"error\":\"no key!\"}");
        let Err(e) = post("http://127.0.0.1:1/", &[], b"x", Duration::from_secs(1)) else { panic!("connected to a closed port") };
        assert!(e.contains("could not reach"), "{e}");
    }

    #[test]
    fn downloads_resume_and_report_progress() {
        let dir = std::env::temp_dir().join(format!("studio-net-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("file.bin");
        let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&dest, &data[..40_000]).unwrap();
        let d = data.clone();
        let url = serve(1, move |head, s| {
            assert!(head.contains("Range: bytes=40000-"), "{head}");
            write!(s, "HTTP/1.1 206 Partial Content\r\ncontent-length: 60000\r\n\r\n").unwrap();
            s.write_all(&d[40_000..]).unwrap();
        });
        let mut seen = Vec::new();
        download(&url, &dest, Download::default(), &mut |n| {
            seen.push(n);
            true
        })
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        assert_eq!(seen.first(), Some(&40_000));
        assert_eq!(seen.last(), Some(&100_000));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stalled_download_gives_up_and_cancelling_stops() {
        let dir = std::env::temp_dir().join(format!("studio-net-stall-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let url = serve(2, |_, s| {
            write!(s, "HTTP/1.1 200 OK\r\ncontent-length: 1000000\r\n\r\n").unwrap();
            s.write_all(&[7; 1000]).unwrap();
            std::thread::sleep(Duration::from_secs(4));
        });
        let t = Instant::now();
        let opts = Download { stall: Duration::from_secs(1), resume: false, retries: 0, ..Default::default() };
        let e = download(&url, &dir.join("a"), opts, &mut |_| true).unwrap_err();
        assert!(matches!(&e, Failure::Failed(m) if m.contains("stalled")), "{e}");
        assert!(t.elapsed() < Duration::from_secs(4));
        assert_eq!(download(&url, &dir.join("b"), opts, &mut |n| n == 0), Err(Failure::Cancelled));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pinned_get_connects_to_the_given_address_and_leaves_redirects() {
        let url = serve(2, |head, s| {
            if head.starts_with("GET /go") {
                write!(s, "HTTP/1.1 302 Found\r\nLocation: /there\r\ncontent-length: 0\r\n\r\n").unwrap();
            } else {
                assert!(head.contains("Host: pinned.example"), "{head}");
                write!(s, "HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok").unwrap();
            }
        });
        let addr: std::net::SocketAddr = url.trim_start_matches("http://").parse().unwrap();
        let h = get_pinned_with(&format!("http://pinned.example:{}/go", addr.port()), &[], Duration::from_secs(5), vec![addr], false).unwrap();
        assert_eq!((h.reply.status, h.location.as_deref()), (302, Some("/there")));
        let h = get_pinned_with(&format!("http://pinned.example:{}/there", addr.port()), &[], Duration::from_secs(5), vec![addr], false).unwrap();
        assert_eq!((h.reply.status, h.location), (200, None));
        assert_eq!(h.reply.bytes(10).unwrap(), b"ok");
    }
}
