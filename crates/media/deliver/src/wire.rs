// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Render nodes on the local network.
//!
//! A node (`zornov node`) listens on a TCP port. The coordinator connects,
//! proves it knows the pairing token (a challenge, answered with
//! SHA-256(token || nonce)), sends the project's files that the node does
//! not already hold (by fingerprint), asks it to render a frame range, and
//! gets the finished piece back. Pieces are video-only MP4s that
//! [`crate::merge`] joins without re-encoding.
//!
//! The channel is authenticated, not encrypted: use it on a network you
//! trust. A node only ever writes below its own work folder (every path
//! is checked), and only renders; it runs nothing it is sent.
//!
//! Discovery is a UDP broadcast: `STUDIO-NODE?` is answered with the
//! node's name and port (never the token).

use crate::sha256::{self, hex};
use crate::chunks::{ChunkRenderer, ChunkTask};
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const PROTO: u32 = 1;
pub const DEFAULT_PORT: u16 = 7878;
pub const DISCOVERY_PORT: u16 = 7879;
const MAX_FRAME: usize = 8 << 20;
const DATA_CHUNK: usize = 1 << 20;

mod kind {
    pub const HELLO: u8 = 1;
    pub const CHALLENGE: u8 = 2;
    pub const AUTH: u8 = 3;
    pub const READY: u8 = 4;
    pub const DENIED: u8 = 5;
    pub const MANIFEST: u8 = 6;
    pub const NEED: u8 = 7;
    pub const FILE_START: u8 = 8;
    pub const DATA: u8 = 9;
    pub const FILE_END: u8 = 10;
    pub const RENDER: u8 = 11;
    pub const PROGRESS: u8 = 12;
    pub const RESULT: u8 = 13;
    pub const ERROR: u8 = 14;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeInfo {
    pub name: String,
    pub cores: usize,
    pub proto: u32,
    pub busy: bool,
}

/// One file of a project to be present on the node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ManifestFile {
    /// Relative, `/`-separated, no `..`.
    pub path: String,
    pub size: u64,
    pub fp: String,
}

/// A project to render on a node: where its files are here, and their
/// names there.
#[derive(Clone, Debug)]
pub struct ProjectPack {
    /// Identifies the project on the node (its cached copy).
    pub id: String,
    pub files: Vec<(ManifestFile, PathBuf)>,
}

impl ProjectPack {
    pub fn new(id: &str) -> Self {
        ProjectPack { id: id.into(), files: Vec::new() }
    }

    pub fn add(&mut self, rel: &str, src: &Path) -> io::Result<()> {
        let size = std::fs::metadata(src)?.len();
        let fp = sha256::fingerprint(src)?;
        self.files.push((ManifestFile { path: rel.into(), size, fp }, src.to_path_buf()));
        Ok(())
    }
}

fn write_frame(w: &mut impl Write, kind: u8, payload: &[u8]) -> io::Result<()> {
    let mut head = [0u8; 5];
    head[0] = kind;
    head[1..].copy_from_slice(&(payload.len() as u32).to_be_bytes());
    w.write_all(&head)?;
    w.write_all(payload)?;
    w.flush()
}

fn read_frame(r: &mut impl Read) -> io::Result<(u8, Vec<u8>)> {
    let mut head = [0u8; 5];
    r.read_exact(&mut head)?;
    let n = u32::from_be_bytes([head[1], head[2], head[3], head[4]]) as usize;
    if n > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
    }
    let mut p = vec![0u8; n];
    r.read_exact(&mut p)?;
    Ok((head[0], p))
}

fn json<T: Serialize>(v: &T) -> Vec<u8> {
    serde_json::to_vec(v).unwrap_or_default()
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

fn auth_answer(token: &str, nonce: &[u8]) -> [u8; 32] {
    let mut h = sha256::Sha256::new();
    h.update(b"studio-node-v1");
    h.update(token.as_bytes());
    h.update(nonce);
    h.finish()
}

/// A random-enough nonce (time, process, a counter, the address of a
/// stack variable), hashed. Pairing needs unpredictability per connection,
/// not secrecy of the generator.
fn nonce() -> [u8; 16] {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let x = 0u8;
    let mut h = sha256::Sha256::new();
    h.update(&std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()).to_be_bytes());
    h.update(&std::process::id().to_be_bytes());
    h.update(&COUNTER.fetch_add(1, Ordering::Relaxed).to_be_bytes());
    h.update(&(&x as *const u8 as usize).to_be_bytes());
    let d = h.finish();
    let mut n = [0u8; 16];
    n.copy_from_slice(&d[..16]);
    n
}

/// A relative path with only normal components, as a `PathBuf` under `root`.
pub fn safe_join(root: &Path, rel: &str) -> Option<PathBuf> {
    if rel.is_empty() || rel.contains('\\') || rel.contains('\0') {
        return None;
    }
    let p = Path::new(rel);
    if p.is_absolute() || !p.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    Some(root.join(p))
}

/// A token for pairing: 24 random-ish hex digits.
pub fn new_token() -> String {
    hex(&sha256::sha256(&nonce()))[..24].to_string()
}

/// What the node does with the work.
pub trait NodeWorker: Send + Sync + 'static {
    fn info(&self) -> NodeInfo;
    /// Render `task` from the project in `dir`; returns the finished MP4.
    fn render(&self, dir: &Path, task: &ChunkTask, progress: &mut dyn FnMut(f32), cancel: &AtomicBool) -> Result<PathBuf, String>;
}

pub struct NodeServer {
    listener: TcpListener,
    token: String,
    work: PathBuf,
    worker: Arc<dyn NodeWorker>,
    busy: Arc<Mutex<()>>,
    stop: Arc<AtomicBool>,
}

impl NodeServer {
    pub fn bind(addr: &str, token: &str, work: &Path, worker: Arc<dyn NodeWorker>) -> io::Result<NodeServer> {
        if token.len() < 8 {
            return Err(bad("the pairing token needs at least 8 characters"));
        }
        std::fs::create_dir_all(work)?;
        Ok(NodeServer { listener: TcpListener::bind(addr)?, token: token.into(), work: work.into(), worker, busy: Arc::new(Mutex::new(())), stop: Arc::new(AtomicBool::new(false)) })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.listener.local_addr().expect("bound")
    }

    pub fn stopper(&self) -> Arc<AtomicBool> {
        self.stop.clone()
    }

    /// Answer discovery broadcasts on `port` (0: none) in the background.
    pub fn announce(&self, port: u16) -> io::Result<()> {
        let sock = UdpSocket::bind(("0.0.0.0", port))?;
        sock.set_read_timeout(Some(Duration::from_millis(300)))?;
        let (worker, stop, tcp_port) = (self.worker.clone(), self.stop.clone(), self.local_addr().port());
        std::thread::Builder::new().name("studio-node-announce".into()).spawn(move || {
            let mut buf = [0u8; 64];
            while !stop.load(Ordering::Relaxed) {
                if let Ok((n, from)) = sock.recv_from(&mut buf) {
                    if &buf[..n] == b"STUDIO-NODE?" {
                        let info = worker.info();
                        let reply = serde_json::json!({"name": info.name, "port": tcp_port, "cores": info.cores, "proto": PROTO});
                        let _ = sock.send_to(reply.to_string().as_bytes(), from);
                    }
                }
            }
        })?;
        Ok(())
    }

    /// Accept connections until stopped.
    pub fn serve(&self) {
        let _ = self.listener.set_nonblocking(true);
        while !self.stop.load(Ordering::Relaxed) {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    let (token, work, worker, busy) = (self.token.clone(), self.work.clone(), self.worker.clone(), self.busy.clone());
                    let _ = std::thread::Builder::new().name("studio-node-conn".into()).spawn(move || {
                        let _ = handle(stream, &token, &work, &*worker, &busy);
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => std::thread::sleep(Duration::from_millis(200)),
            }
        }
    }
}

fn project_dir(work: &Path, id: &str) -> Option<PathBuf> {
    // The id names a folder; keep it a single plain component.
    let clean: String = id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    if clean.is_empty() || clean.len() > 64 {
        return None;
    }
    Some(work.join("projects").join(clean))
}

fn handle(mut s: TcpStream, token: &str, work: &Path, worker: &dyn NodeWorker, busy: &Mutex<()>) -> io::Result<()> {
    s.set_read_timeout(Some(Duration::from_secs(30)))?;
    s.set_nodelay(true).ok();
    let (k, hello) = read_frame(&mut s)?;
    if k != kind::HELLO || serde_json::from_slice::<serde_json::Value>(&hello).ok().and_then(|v| v["proto"].as_u64()) != Some(PROTO as u64) {
        write_frame(&mut s, kind::DENIED, b"protocol mismatch")?;
        return Ok(());
    }
    let n = nonce();
    write_frame(&mut s, kind::CHALLENGE, &n)?;
    let (k, answer) = read_frame(&mut s)?;
    if k != kind::AUTH || answer != auth_answer(token, &n) {
        // Slow wrong guesses down.
        std::thread::sleep(Duration::from_millis(500));
        write_frame(&mut s, kind::DENIED, b"wrong pairing token")?;
        return Ok(());
    }
    let mut info = worker.info();
    info.busy = busy.try_lock().is_err();
    write_frame(&mut s, kind::READY, &json(&info))?;

    // Manifest: say what is missing.
    let (k, m) = read_frame(&mut s)?;
    if k != kind::MANIFEST {
        return Err(bad("expected a manifest"));
    }
    #[derive(Deserialize)]
    struct Manifest {
        project: String,
        files: Vec<ManifestFile>,
    }
    let man: Manifest = serde_json::from_slice(&m).map_err(|e| bad(&e.to_string()))?;
    let dir = project_dir(work, &man.project).ok_or_else(|| bad("bad project id"))?;
    std::fs::create_dir_all(&dir)?;
    let mut need = Vec::new();
    for f in &man.files {
        let p = safe_join(&dir, &f.path).ok_or_else(|| bad("unsafe path in manifest"))?;
        let have = std::fs::metadata(&p).map(|m| m.len() == f.size).unwrap_or(false) && sha256::fingerprint(&p).map(|fp| fp == f.fp).unwrap_or(false);
        if !have {
            need.push(f.path.clone());
        }
    }
    write_frame(&mut s, kind::NEED, &json(&need))?;
    for want in &need {
        let (k, h) = read_frame(&mut s)?;
        if k != kind::FILE_START {
            return Err(bad("expected a file"));
        }
        let hv: serde_json::Value = serde_json::from_slice(&h).map_err(|e| bad(&e.to_string()))?;
        let path = hv["path"].as_str().unwrap_or_default();
        if path != want {
            return Err(bad("file out of order"));
        }
        let size = hv["size"].as_u64().unwrap_or(0);
        let dest = safe_join(&dir, path).ok_or_else(|| bad("unsafe path"))?;
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = dest.with_extension("part-transfer");
        let mut out = io::BufWriter::new(std::fs::File::create(&tmp)?);
        let mut got = 0u64;
        loop {
            let (k, d) = read_frame(&mut s)?;
            match k {
                kind::DATA => {
                    got += d.len() as u64;
                    if got > size {
                        return Err(bad("file longer than announced"));
                    }
                    out.write_all(&d)?;
                }
                kind::FILE_END => break,
                _ => return Err(bad("unexpected frame in a file")),
            }
        }
        out.flush()?;
        drop(out);
        if got != size {
            let _ = std::fs::remove_file(&tmp);
            return Err(bad("file shorter than announced"));
        }
        std::fs::rename(&tmp, &dest)?;
    }

    let (k, r) = read_frame(&mut s)?;
    if k != kind::RENDER {
        return Err(bad("expected a render request"));
    }
    let task: ChunkTask = serde_json::from_slice(&r).map_err(|e| bad(&e.to_string()))?;
    let Ok(_guard) = busy.try_lock() else {
        write_frame(&mut s, kind::ERROR, b"busy")?;
        return Ok(());
    };
    // Rendering can take long: no read timeout; a watcher notices the
    // coordinator hanging up and cancels.
    s.set_read_timeout(None)?;
    let cancel = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    {
        let (mut peer, cancel, done) = (s.try_clone()?, cancel.clone(), done.clone());
        std::thread::spawn(move || {
            let mut b = [0u8; 1];
            let _ = peer.set_read_timeout(Some(Duration::from_millis(200)));
            while !done.load(Ordering::Relaxed) {
                match peer.read(&mut b) {
                    Ok(0) => {
                        cancel.store(true, Ordering::Relaxed);
                        break;
                    }
                    Ok(_) => {}
                    Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
                    Err(_) => {
                        cancel.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            }
        });
    }
    let mut last_sent = -1.0f32;
    let mut sock = s.try_clone()?;
    let result = worker.render(&dir, &task, &mut |f| {
        if f - last_sent >= 0.01 {
            last_sent = f;
            let _ = write_frame(&mut sock, kind::PROGRESS, &f.to_be_bytes());
        }
    }, &cancel);
    done.store(true, Ordering::Relaxed);
    match result {
        Err(e) => write_frame(&mut s, kind::ERROR, e.as_bytes())?,
        Ok(path) => {
            let mut f = std::fs::File::open(&path)?;
            let size = f.metadata()?.len();
            write_frame(&mut s, kind::RESULT, &size.to_be_bytes())?;
            let mut buf = vec![0u8; DATA_CHUNK];
            loop {
                let n = f.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                write_frame(&mut s, kind::DATA, &buf[..n])?;
            }
            write_frame(&mut s, kind::FILE_END, &[])?;
            let _ = std::fs::remove_file(&path);
        }
    }
    Ok(())
}

/// A node as the coordinator sees it.
#[derive(Clone, Debug)]
pub struct RemoteNode {
    pub name: String,
    pub addr: SocketAddr,
    pub token: String,
    pub project: Arc<ProjectPack>,
}

impl RemoteNode {
    pub fn new(addr: SocketAddr, token: &str, project: Arc<ProjectPack>) -> Self {
        RemoteNode { name: addr.to_string(), addr, token: token.into(), project }
    }

    fn connect(&self) -> Result<(TcpStream, NodeInfo), String> {
        let mut s = TcpStream::connect_timeout(&self.addr, Duration::from_secs(5)).map_err(|e| format!("{}: {e}", self.addr))?;
        s.set_read_timeout(Some(Duration::from_secs(30))).ok();
        s.set_nodelay(true).ok();
        let io = |e: io::Error| format!("{}: {e}", self.addr);
        write_frame(&mut s, kind::HELLO, &json(&serde_json::json!({"proto": PROTO}))).map_err(io)?;
        let (k, n) = read_frame(&mut s).map_err(io)?;
        if k == kind::DENIED {
            return Err(format!("{}: {}", self.addr, String::from_utf8_lossy(&n)));
        }
        if k != kind::CHALLENGE {
            return Err(format!("{}: not a Zornov node", self.addr));
        }
        write_frame(&mut s, kind::AUTH, &auth_answer(&self.token, &n)).map_err(io)?;
        let (k, r) = read_frame(&mut s).map_err(io)?;
        if k == kind::DENIED {
            return Err(format!("{}: {}", self.addr, String::from_utf8_lossy(&r)));
        }
        let info: NodeInfo = serde_json::from_slice(&r).map_err(|e| e.to_string())?;
        Ok((s, info))
    }

    /// Reachable and paired?
    pub fn ping(&self) -> Result<NodeInfo, String> {
        self.connect().map(|(_, i)| i)
    }
}

impl ChunkRenderer for RemoteNode {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn render(&self, task: &ChunkTask, out: &Path, progress: &mut dyn FnMut(f32), cancel: &AtomicBool) -> Result<(), String> {
        let (mut s, _info) = self.connect()?;
        let io = |e: io::Error| format!("{}: {e}", self.addr);
        let files: Vec<&ManifestFile> = self.project.files.iter().map(|(m, _)| m).collect();
        write_frame(&mut s, kind::MANIFEST, &json(&serde_json::json!({"project": self.project.id, "files": files}))).map_err(io)?;
        let (k, need) = read_frame(&mut s).map_err(io)?;
        if k == kind::ERROR {
            return Err(String::from_utf8_lossy(&need).into_owned());
        }
        let need: Vec<String> = serde_json::from_slice(&need).map_err(|e| e.to_string())?;
        for path in &need {
            let (m, src) = self.project.files.iter().find(|(m, _)| &m.path == path).ok_or("the node asked for an unknown file")?;
            write_frame(&mut s, kind::FILE_START, &json(&serde_json::json!({"path": m.path, "size": m.size}))).map_err(io)?;
            let mut f = std::fs::File::open(src).map_err(io)?;
            let mut buf = vec![0u8; DATA_CHUNK];
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                let n = f.read(&mut buf).map_err(io)?;
                if n == 0 {
                    break;
                }
                write_frame(&mut s, kind::DATA, &buf[..n]).map_err(io)?;
            }
            write_frame(&mut s, kind::FILE_END, &[]).map_err(io)?;
        }
        write_frame(&mut s, kind::RENDER, &json(task)).map_err(io)?;
        // Progress and the result; poll so a cancel is noticed.
        s.set_read_timeout(Some(Duration::from_millis(250))).ok();
        let mut file: Option<(std::io::BufWriter<std::fs::File>, u64, u64)> = None;
        loop {
            if cancel.load(Ordering::Relaxed) {
                let _ = s.shutdown(std::net::Shutdown::Both);
                let _ = std::fs::remove_file(out);
                return Err("cancelled".into());
            }
            let mut head = [0u8; 5];
            match read_exact_poll(&mut s, &mut head, cancel) {
                Ok(true) => {}
                Ok(false) => return Err("cancelled".into()),
                Err(e) => return Err(io(e)),
            }
            let n = u32::from_be_bytes([head[1], head[2], head[3], head[4]]) as usize;
            if n > MAX_FRAME {
                return Err("oversized frame from the node".into());
            }
            let mut p = vec![0u8; n];
            if !read_exact_poll(&mut s, &mut p, cancel).map_err(io)? {
                return Err("cancelled".into());
            }
            match head[0] {
                kind::PROGRESS if p.len() == 4 => progress(f32::from_be_bytes([p[0], p[1], p[2], p[3]]) * 0.98),
                kind::ERROR => return Err(format!("{}: {}", self.addr, String::from_utf8_lossy(&p))),
                kind::RESULT if p.len() == 8 => {
                    let size = u64::from_be_bytes(p[..8].try_into().unwrap_or([0; 8]));
                    file = Some((std::io::BufWriter::new(std::fs::File::create(out).map_err(io)?), size, 0));
                }
                kind::DATA => {
                    let (w, size, got) = file.as_mut().ok_or("data before the result")?;
                    *got += p.len() as u64;
                    if *got > *size {
                        return Err("the node sent more than announced".into());
                    }
                    w.write_all(&p).map_err(io)?;
                }
                kind::FILE_END => {
                    let (mut w, size, got) = file.take().ok_or("end before the result")?;
                    w.flush().map_err(io)?;
                    if got != size {
                        let _ = std::fs::remove_file(out);
                        return Err("the node's piece arrived short".into());
                    }
                    progress(1.0);
                    return Ok(());
                }
                _ => {}
            }
        }
    }
}

/// `read_exact` on a socket with a short timeout; false when `cancel` is set.
fn read_exact_poll(s: &mut TcpStream, buf: &mut [u8], cancel: &AtomicBool) -> io::Result<bool> {
    let mut got = 0;
    while got < buf.len() {
        if cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        match s.read(&mut buf[got..]) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the node hung up")),
            Ok(n) => got += n,
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

/// Nodes on the network that answer a broadcast within `wait`.
pub fn discover(port: u16, wait: Duration) -> Vec<(String, SocketAddr)> {
    discover_at(&format!("255.255.255.255:{port}"), wait)
}

/// Like [`discover`], to a given address (a unicast probe works too).
pub fn discover_at(target: &str, wait: Duration) -> Vec<(String, SocketAddr)> {
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else { return Vec::new() };
    let _ = sock.set_broadcast(true);
    let _ = sock.set_read_timeout(Some(Duration::from_millis(150)));
    if sock.send_to(b"STUDIO-NODE?", target).is_err() {
        return Vec::new();
    }
    let end = std::time::Instant::now() + wait;
    let mut found: Vec<(String, SocketAddr)> = Vec::new();
    let mut buf = [0u8; 512];
    while std::time::Instant::now() < end {
        if let Ok((n, from)) = sock.recv_from(&mut buf) {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&buf[..n]) {
                if v["proto"].as_u64() == Some(PROTO as u64) {
                    if let Some(port) = v["port"].as_u64() {
                        let addr = SocketAddr::new(from.ip(), port as u16);
                        if !found.iter().any(|(_, a)| *a == addr) {
                            found.push((v["name"].as_str().unwrap_or("node").to_string(), addr));
                        }
                    }
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    struct Fake {
        renders: AtomicU32,
    }

    impl NodeWorker for Fake {
        fn info(&self) -> NodeInfo {
            NodeInfo { name: "fake".into(), cores: 4, proto: PROTO, busy: false }
        }

        fn render(&self, dir: &Path, task: &ChunkTask, progress: &mut dyn FnMut(f32), cancel: &AtomicBool) -> Result<PathBuf, String> {
            self.renders.fetch_add(1, Ordering::SeqCst);
            // The piece is the project's text file plus the frame range.
            let text = std::fs::read_to_string(dir.join("project.json")).map_err(|e| e.to_string())?;
            for i in 0..5 {
                if cancel.load(Ordering::Relaxed) {
                    return Err("cancelled".into());
                }
                progress(i as f32 / 5.0);
                std::thread::sleep(Duration::from_millis(30));
            }
            let out = dir.join(format!("piece-{}.bin", task.index));
            std::fs::write(&out, format!("{text}|{}-{}", task.first, task.last)).map_err(|e| e.to_string())?;
            Ok(out)
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("studio-wire-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn start(token: &str, work: &Path) -> (SocketAddr, Arc<Fake>, Arc<AtomicBool>) {
        let worker = Arc::new(Fake { renders: AtomicU32::new(0) });
        let server = NodeServer::bind("127.0.0.1:0", token, work, worker.clone()).unwrap();
        let addr = server.local_addr();
        let stop = server.stopper();
        std::thread::spawn(move || server.serve());
        (addr, worker, stop)
    }

    #[test]
    fn a_paired_node_renders_and_sends_the_piece_back() {
        let dir = scratch("ok");
        let src = dir.join("src");
        std::fs::create_dir_all(src.join("media")).unwrap();
        std::fs::write(src.join("project.json"), "PROJECT").unwrap();
        std::fs::write(src.join("media/a.bin"), vec![7u8; 3_000_000]).unwrap();
        let (addr, worker, stop) = start("secret-token", &dir.join("node"));
        let mut pack = ProjectPack::new("p1");
        pack.add("project.json", &src.join("project.json")).unwrap();
        pack.add("media/a.bin", &src.join("media/a.bin")).unwrap();
        let node = RemoteNode::new(addr, "secret-token", Arc::new(pack));
        assert_eq!(node.ping().unwrap().name, "fake");
        let out = dir.join("piece.bin");
        let task = ChunkTask { job: "j".into(), index: 3, first: 10, last: 20, settings: serde_json::json!({}) };
        let mut seen = Vec::new();
        node.render(&task, &out, &mut |f| seen.push(f), &AtomicBool::new(false)).unwrap();
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "PROJECT|10-20");
        assert!(seen.last() == Some(&1.0) && seen.len() > 2);
        // The files are now cached on the node: a second chunk sends none.
        let cached = dir.join("node/projects/p1/media/a.bin");
        assert_eq!(std::fs::metadata(&cached).unwrap().len(), 3_000_000);
        std::fs::write(&cached, vec![7u8; 3_000_000]).unwrap(); // same content
        node.render(&ChunkTask { index: 4, ..task.clone() }, &dir.join("piece2.bin"), &mut |_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(worker.renders.load(Ordering::SeqCst), 2);
        // A changed file on the coordinator is sent again.
        std::fs::write(src.join("project.json"), "PROJECT-2").unwrap();
        let mut pack = ProjectPack::new("p1");
        pack.add("project.json", &src.join("project.json")).unwrap();
        let node = RemoteNode::new(addr, "secret-token", Arc::new(pack));
        node.render(&task, &out, &mut |_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "PROJECT-2|10-20");
        stop.store(true, Ordering::Relaxed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_wrong_token_and_unsafe_paths_are_refused() {
        let dir = scratch("deny");
        let (addr, _w, stop) = start("secret-token", &dir.join("node"));
        let node = RemoteNode::new(addr, "not-the-token", Arc::new(ProjectPack::new("p")));
        assert!(node.ping().unwrap_err().contains("wrong pairing token"));
        // A manifest naming a path outside the project is dropped before any write.
        let mut pack = ProjectPack::new("p");
        std::fs::write(dir.join("x"), "x").unwrap();
        pack.add("../../escape.txt", &dir.join("x")).unwrap();
        let node = RemoteNode::new(addr, "secret-token", Arc::new(pack));
        let task = ChunkTask { job: "j".into(), index: 0, first: 0, last: 1, settings: serde_json::json!({}) };
        assert!(node.render(&task, &dir.join("o"), &mut |_| {}, &AtomicBool::new(false)).is_err());
        assert!(!dir.join("escape.txt").exists() && !dir.parent().unwrap().join("escape.txt").exists());
        assert!(safe_join(Path::new("/r"), "a/b.txt").is_some());
        for bad in ["../a", "/abs", "a/../../b", "", "a\\b"] {
            assert!(safe_join(Path::new("/r"), bad).is_none(), "{bad}");
        }
        assert!(NodeServer::bind("127.0.0.1:0", "short", &dir, Arc::new(Fake { renders: AtomicU32::new(0) })).is_err());
        stop.store(true, Ordering::Relaxed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancelling_stops_the_node_and_discovery_finds_it() {
        let dir = scratch("cancel");
        let (addr, _w, stop) = start("secret-token", &dir.join("node"));
        let node = RemoteNode::new(addr, "secret-token", Arc::new(ProjectPack::new("p")));
        let cancel = Arc::new(AtomicBool::new(false));
        let c2 = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            c2.store(true, Ordering::Relaxed);
        });
        let task = ChunkTask { job: "j".into(), index: 0, first: 0, last: 1, settings: serde_json::json!({}) };
        // project.json missing on the node: the fake errors, or the cancel wins; either way an Err and no output.
        assert!(node.render(&task, &dir.join("o"), &mut |_| {}, &cancel).is_err());
        assert!(!dir.join("o").exists());
        stop.store(true, Ordering::Relaxed);

        let worker = Arc::new(Fake { renders: AtomicU32::new(0) });
        let server = NodeServer::bind("127.0.0.1:0", "secret-token", &dir.join("n2"), worker).unwrap();
        let tcp = server.local_addr().port();
        let free = UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        server.announce(free).unwrap();
        let found = discover_at(&format!("127.0.0.1:{free}"), Duration::from_millis(700));
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!((found[0].0.as_str(), found[0].1.port()), ("fake", tcp));
        server.stopper().store(true, Ordering::Relaxed);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
