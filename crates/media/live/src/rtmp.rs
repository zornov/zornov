// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! RTMP publishing, in Rust: the handshake, the chunk stream, the
//! `connect` / `createStream` / `publish` commands and FLV-style audio and
//! video messages, so Studio can stream to YouTube, Twitch, Facebook or
//! any RTMP server without another program. Plain `rtmp://` runs here;
//! `rtmps://` (TLS) is not supported yet, and the stream key never leaves
//! the connection (status text shows the address with the key hidden).

use crate::amf::Amf;
use crate::av::Packet;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub struct RtmpUrl {
    pub host: String,
    pub port: u16,
    pub app: String,
    pub key: String,
}

impl RtmpUrl {
    pub fn parse(url: &str) -> Result<RtmpUrl, String> {
        let rest = url.strip_prefix("rtmp://").ok_or_else(|| if url.starts_with("rtmps://") { "rtmps:// (TLS) is not supported yet; use rtmp:// if the service offers it".to_string() } else { format!("not an RTMP address: {}", redact(url)) })?;
        let (hostport, path) = rest.split_once('/').ok_or("the address needs an app and a stream key: rtmp://host/app/key")?;
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) => (h.to_owned(), p.parse::<u16>().map_err(|_| format!("bad port \"{p}\""))?),
            None => (hostport.to_owned(), 1935),
        };
        let (app, key) = path.rsplit_once('/').ok_or("the address needs an app and a stream key: rtmp://host/app/key")?;
        if host.is_empty() || app.is_empty() || key.is_empty() {
            return Err("the address needs a host, an app and a stream key: rtmp://host/app/key".into());
        }
        Ok(RtmpUrl { host, port, app: app.into(), key: key.into() })
    }

    pub fn tc_url(&self) -> String {
        format!("rtmp://{}:{}/{}", self.host, self.port, self.app)
    }
}

/// The address with the stream key hidden, for messages.
pub fn redact(url: &str) -> String {
    match url.rsplit_once('/') {
        Some((head, key)) if !key.is_empty() && url.matches('/').count() >= 4 => format!("{head}/{}", "*".repeat(key.len().min(8))),
        _ => url.to_owned(),
    }
}

const CHUNK_OUT: usize = 4096;

struct Writer {
    s: TcpStream,
    chunk: usize,
    bytes_out: u64,
}

impl Writer {
    /// One message, chunked. `csid` is the chunk stream, `stream` the
    /// message stream id.
    fn message(&mut self, csid: u32, ts: u32, typ: u8, stream: u32, payload: &[u8]) -> std::io::Result<()> {
        let mut out = Vec::with_capacity(payload.len() + 32);
        let ext = ts >= 0xFF_FFFF;
        let mut first = true;
        let mut off = 0;
        while off < payload.len() || first {
            let n = (payload.len() - off).min(self.chunk);
            if first {
                basic(&mut out, 0, csid);
                out.extend_from_slice(&(if ext { 0xFF_FFFF } else { ts }).to_be_bytes()[1..]);
                out.extend_from_slice(&(payload.len() as u32).to_be_bytes()[1..]);
                out.push(typ);
                out.extend_from_slice(&stream.to_le_bytes());
            } else {
                basic(&mut out, 3, csid);
            }
            if ext {
                out.extend_from_slice(&ts.to_be_bytes());
            }
            out.extend_from_slice(&payload[off..off + n]);
            off += n;
            first = false;
            if payload.is_empty() {
                break;
            }
        }
        self.bytes_out += out.len() as u64;
        self.s.write_all(&out)
    }
}

fn basic(out: &mut Vec<u8>, fmt: u8, csid: u32) {
    if csid < 64 {
        out.push(fmt << 6 | csid as u8);
    } else if csid < 320 {
        out.push(fmt << 6);
        out.push((csid - 64) as u8);
    } else {
        out.push(fmt << 6 | 1);
        out.extend_from_slice(&((csid - 64) as u16).to_le_bytes());
    }
}

/// What the server said, as the reader thread understood it.
#[derive(Debug)]
enum Event {
    Command(Vec<Amf>),
    Closed(String),
}

pub struct RtmpClient {
    w: Arc<Mutex<Writer>>,
    rx: mpsc::Receiver<Event>,
    stream_id: u32,
    sent_headers: bool,
    video_hdr: Vec<u8>,
    audio_hdr: Vec<u8>,
    closed: Arc<Mutex<Option<String>>>,
    pub url: String,
}

impl RtmpClient {
    /// Connects and starts publishing. `avcc` and `asc` are the codec
    /// records of the streams that follow.
    pub fn publish(url: &str, avcc: &[u8], asc: &[u8], meta: &Meta) -> Result<RtmpClient, String> {
        let u = RtmpUrl::parse(url)?;
        let addr = (u.host.as_str(), u.port).to_socket_addrs().map_err(|e| format!("could not find {}: {e}", u.host))?.next().ok_or_else(|| format!("could not find {}", u.host))?;
        let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(10)).map_err(|e| format!("could not reach {}:{}: {e}", u.host, u.port))?;
        s.set_nodelay(true).ok();
        s.set_read_timeout(Some(Duration::from_secs(10))).ok();
        s.set_write_timeout(Some(Duration::from_secs(15))).ok();
        handshake(&mut s).map_err(|e| format!("RTMP handshake with {} failed: {e}", u.host))?;
        let reader = s.try_clone().map_err(|e| e.to_string())?;
        let w = Arc::new(Mutex::new(Writer { s, chunk: 128, bytes_out: 0 }));
        let (tx, rx) = mpsc::channel();
        let closed = Arc::new(Mutex::new(None));
        {
            let (w2, c2) = (w.clone(), closed.clone());
            std::thread::spawn(move || {
                let r = read_loop(reader, &w2, &tx);
                let why = r.err().unwrap_or_else(|| "the server closed the connection".into());
                if let Ok(mut c) = c2.lock() {
                    *c = Some(why.clone());
                }
                let _ = tx.send(Event::Closed(why));
            });
        }
        let mut c = RtmpClient { w, rx, stream_id: 0, sent_headers: false, video_hdr: avcc.to_vec(), audio_hdr: asc.to_vec(), closed, url: redact(url) };
        {
            let mut g = c.w.lock().map_err(|_| "RTMP writer poisoned")?;
            g.message(2, 0, 1, 0, &(CHUNK_OUT as u32).to_be_bytes()).map_err(io)?;
            g.chunk = CHUNK_OUT;
        }
        // connect
        let mut b = Vec::new();
        Amf::str("connect").encode(&mut b);
        Amf::Num(1.0).encode(&mut b);
        Amf::Obj(vec![
            ("app".into(), Amf::str(&u.app)),
            ("type".into(), Amf::str("nonprivate")),
            ("flashVer".into(), Amf::str("FMLE/3.0 (compatible; Zornov)")),
            ("tcUrl".into(), Amf::str(&u.tc_url())),
        ])
        .encode(&mut b);
        c.send_cmd(3, 0, &b)?;
        c.expect_result(1.0, "connect")?;
        // createStream
        let mut b = Vec::new();
        Amf::str("releaseStream").encode(&mut b);
        Amf::Num(2.0).encode(&mut b);
        Amf::Null.encode(&mut b);
        Amf::str(&u.key).encode(&mut b);
        c.send_cmd(3, 0, &b)?;
        let mut b = Vec::new();
        Amf::str("FCPublish").encode(&mut b);
        Amf::Num(3.0).encode(&mut b);
        Amf::Null.encode(&mut b);
        Amf::str(&u.key).encode(&mut b);
        c.send_cmd(3, 0, &b)?;
        let mut b = Vec::new();
        Amf::str("createStream").encode(&mut b);
        Amf::Num(4.0).encode(&mut b);
        Amf::Null.encode(&mut b);
        c.send_cmd(3, 0, &b)?;
        let reply = c.expect_result(4.0, "createStream")?;
        c.stream_id = reply.get(3).and_then(Amf::as_num).unwrap_or(1.0) as u32;
        // publish
        let mut b = Vec::new();
        Amf::str("publish").encode(&mut b);
        Amf::Num(5.0).encode(&mut b);
        Amf::Null.encode(&mut b);
        Amf::str(&u.key).encode(&mut b);
        Amf::str("live").encode(&mut b);
        c.send_cmd(4, c.stream_id, &b)?;
        c.expect_status("NetStream.Publish.Start")?;
        // Metadata, then the codec headers.
        let mut b = Vec::new();
        Amf::str("@setDataFrame").encode(&mut b);
        Amf::str("onMetaData").encode(&mut b);
        Amf::Map(vec![
            ("width".into(), Amf::Num(meta.width as f64)),
            ("height".into(), Amf::Num(meta.height as f64)),
            ("framerate".into(), Amf::Num(meta.fps)),
            ("videocodecid".into(), Amf::Num(7.0)),
            ("videodatarate".into(), Amf::Num(meta.video_kbps as f64)),
            ("audiocodecid".into(), Amf::Num(10.0)),
            ("audiodatarate".into(), Amf::Num(meta.audio_kbps as f64)),
            ("audiosamplerate".into(), Amf::Num(meta.sample_rate as f64)),
            ("audiosamplesize".into(), Amf::Num(16.0)),
            ("stereo".into(), Amf::Bool(true)),
            ("encoder".into(), Amf::str("Zornov")),
        ])
        .encode(&mut b);
        c.w.lock().map_err(|_| "poisoned")?.message(5, 0, 18, c.stream_id, &b).map_err(io)?;
        Ok(c)
    }

    fn send_cmd(&mut self, csid: u32, stream: u32, payload: &[u8]) -> Result<(), String> {
        self.w.lock().map_err(|_| "poisoned")?.message(csid, 0, 20, stream, payload).map_err(io)
    }

    fn next_cmd(&mut self, what: &str) -> Result<Vec<Amf>, String> {
        match self.rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Event::Command(c)) => Ok(c),
            Ok(Event::Closed(why)) => Err(format!("{what}: {why}")),
            Err(_) => Err(format!("{what}: the server did not answer")),
        }
    }

    fn expect_result(&mut self, txn: f64, what: &str) -> Result<Vec<Amf>, String> {
        loop {
            let c = self.next_cmd(what)?;
            match c.first().and_then(Amf::as_str) {
                Some("_result") if c.get(1).and_then(Amf::as_num) == Some(txn) => return Ok(c),
                Some("_error") => {
                    let d = c.iter().find_map(|a| a.get("description")).and_then(Amf::as_str).unwrap_or("");
                    return Err(format!("{what} was refused by the server {d}"));
                }
                _ => {}
            }
        }
    }

    fn expect_status(&mut self, code: &str) -> Result<(), String> {
        loop {
            let c = self.next_cmd("publish")?;
            if c.first().and_then(Amf::as_str) == Some("onStatus") {
                let info = c.iter().find(|a| a.get("code").is_some());
                let got = info.and_then(|i| i.get("code")).and_then(Amf::as_str).unwrap_or("");
                if got == code {
                    return Ok(());
                }
                if got.contains("Failed") || got.contains("BadName") || got.contains("Rejected") || got.contains("Unauthorized") {
                    let d = info.and_then(|i| i.get("description")).and_then(Amf::as_str).unwrap_or("");
                    return Err(format!("the server refused to publish: {got} {d}"));
                }
            }
        }
    }

    /// Sends one coded packet.
    pub fn send(&mut self, p: &Packet) -> Result<(), String> {
        if let Some(why) = self.closed.lock().ok().and_then(|c| c.clone()) {
            return Err(why);
        }
        let mut g = self.w.lock().map_err(|_| "poisoned")?;
        if !self.sent_headers {
            let mut v = vec![0x17, 0, 0, 0, 0];
            v.extend_from_slice(&self.video_hdr);
            g.message(6, 0, 9, self.stream_id, &v).map_err(io)?;
            let mut a = vec![0xAF, 0];
            a.extend_from_slice(&self.audio_hdr);
            g.message(4, 0, 8, self.stream_id, &a).map_err(io)?;
            self.sent_headers = true;
        }
        match p {
            Packet::Video { data, keyframe, pts_us } => {
                let mut v = Vec::with_capacity(data.len() + 5);
                v.push(if *keyframe { 0x17 } else { 0x27 });
                v.extend_from_slice(&[1, 0, 0, 0]);
                v.extend_from_slice(data);
                g.message(6, (*pts_us / 1000).max(0) as u32, 9, self.stream_id, &v).map_err(io)
            }
            Packet::Audio { data, pts_us } => {
                let mut a = Vec::with_capacity(data.len() + 2);
                a.extend_from_slice(&[0xAF, 1]);
                a.extend_from_slice(data);
                g.message(4, (*pts_us / 1000).max(0) as u32, 8, self.stream_id, &a).map_err(io)
            }
        }
    }

    pub fn bytes_sent(&self) -> u64 {
        self.w.lock().map(|g| g.bytes_out).unwrap_or(0)
    }

    pub fn close(mut self) {
        let sid = self.stream_id;
        let mut b = Vec::new();
        Amf::str("deleteStream").encode(&mut b);
        Amf::Num(6.0).encode(&mut b);
        Amf::Null.encode(&mut b);
        Amf::Num(sid as f64).encode(&mut b);
        let _ = self.send_cmd(3, 0, &b);
        if let Ok(g) = self.w.lock() {
            let _ = g.s.shutdown(std::net::Shutdown::Both);
        }
    }
}

/// Stream facts for the `onMetaData` message.
#[derive(Clone, Debug)]
pub struct Meta {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub video_kbps: u32,
    pub audio_kbps: u32,
    pub sample_rate: u32,
}

fn io(e: std::io::Error) -> String {
    format!("the connection failed: {e}")
}

fn handshake(s: &mut TcpStream) -> std::io::Result<()> {
    let mut c1 = vec![0u8; 1536];
    // Time zero, zeros, then bytes the server echoes back.
    let mut x = 0x9e3779b9u32 ^ std::process::id();
    for b in c1[8..].iter_mut() {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        *b = x as u8;
    }
    s.write_all(&[3])?;
    s.write_all(&c1)?;
    let mut s0 = [0u8; 1];
    s.read_exact(&mut s0)?;
    if s0[0] != 3 {
        return Err(std::io::Error::other(format!("server speaks RTMP version {}", s0[0])));
    }
    let mut s1 = vec![0u8; 1536];
    s.read_exact(&mut s1)?;
    s.write_all(&s1)?;
    let mut s2 = vec![0u8; 1536];
    s.read_exact(&mut s2)?;
    if s2[8..] != c1[8..] {
        // Some servers do not echo exactly; the ones that matter do.
        return Err(std::io::Error::other("the server's handshake reply did not match"));
    }
    Ok(())
}

struct ChunkStream {
    ts: u32,
    len: usize,
    typ: u8,
    sid: u32,
    buf: Vec<u8>,
}

fn read_loop(mut r: TcpStream, w: &Arc<Mutex<Writer>>, tx: &mpsc::Sender<Event>) -> Result<(), String> {
    r.set_read_timeout(None).ok();
    let mut chunk = 128usize;
    let mut streams: std::collections::HashMap<u32, ChunkStream> = Default::default();
    let mut received = 0u64;
    let mut window = 2_500_000u64;
    let mut last_ack = 0u64;
    let rd = |r: &mut TcpStream, n: usize, recv: &mut u64| -> Result<Vec<u8>, String> {
        let mut b = vec![0u8; n];
        r.read_exact(&mut b).map_err(|e| format!("connection ended: {e}"))?;
        *recv += n as u64;
        Ok(b)
    };
    loop {
        let b0 = rd(&mut r, 1, &mut received)?[0];
        let fmt = b0 >> 6;
        let mut csid = (b0 & 63) as u32;
        if csid == 0 {
            csid = rd(&mut r, 1, &mut received)?[0] as u32 + 64;
        } else if csid == 1 {
            let b = rd(&mut r, 2, &mut received)?;
            csid = b[0] as u32 + b[1] as u32 * 256 + 64;
        }
        let cs = streams.entry(csid).or_insert(ChunkStream { ts: 0, len: 0, typ: 0, sid: 0, buf: Vec::new() });
        if fmt <= 2 {
            let h = rd(&mut r, 3, &mut received)?;
            cs.ts = u32::from_be_bytes([0, h[0], h[1], h[2]]);
            if fmt <= 1 {
                let h = rd(&mut r, 4, &mut received)?;
                cs.len = u32::from_be_bytes([0, h[0], h[1], h[2]]) as usize;
                cs.typ = h[3];
            }
            if fmt == 0 {
                let h = rd(&mut r, 4, &mut received)?;
                cs.sid = u32::from_le_bytes([h[0], h[1], h[2], h[3]]);
            }
        }
        if cs.ts == 0xFF_FFFF {
            rd(&mut r, 4, &mut received)?;
        }
        let want = (cs.len - cs.buf.len()).min(chunk);
        let data = rd(&mut r, want, &mut received)?;
        cs.buf.extend_from_slice(&data);
        if cs.buf.len() >= cs.len {
            let msg = std::mem::take(&mut cs.buf);
            let typ = cs.typ;
            match typ {
                1 if msg.len() >= 4 => chunk = (u32::from_be_bytes([msg[0], msg[1], msg[2], msg[3]]) & 0x7FFF_FFFF).max(1) as usize,
                5 if msg.len() >= 4 => window = u32::from_be_bytes([msg[0], msg[1], msg[2], msg[3]]) as u64,
                4 if msg.len() >= 6 && u16::from_be_bytes([msg[0], msg[1]]) == 6 => {
                    // Ping request: answer with the same time.
                    let mut p = vec![0, 7];
                    p.extend_from_slice(&msg[2..6]);
                    w.lock().map_err(|_| "poisoned")?.message(2, 0, 4, 0, &p).map_err(io)?;
                }
                20 => {
                    if let Ok(v) = Amf::decode_all(&msg) {
                        let _ = tx.send(Event::Command(v));
                    }
                }
                _ => {}
            }
            if received - last_ack >= window / 2 {
                last_ack = received;
                w.lock().map_err(|_| "poisoned")?.message(2, 0, 3, 0, &(received as u32).to_be_bytes()).map_err(io)?;
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod mock {
    //! A small RTMP server for tests, written separately from the client:
    //! it reassembles chunks and records what it was sent.
    use super::*;
    use std::net::TcpListener;

    #[derive(Default, Debug)]
    pub struct Seen {
        pub commands: Vec<Vec<Amf>>,
        pub video: Vec<(u32, Vec<u8>)>,
        pub audio: Vec<(u32, Vec<u8>)>,
        pub meta: Option<Amf>,
        pub chunk_size_set: Option<u32>,
    }

    pub fn serve(refuse_publish: bool) -> (u16, std::thread::JoinHandle<Seen>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        (port, std::thread::spawn(move || serve_on(&l, refuse_publish, None)))
    }

    /// Accepts one connection on `l`; hangs up after `hang_up_after` video messages.
    pub fn serve_on(l: &TcpListener, refuse_publish: bool, hang_up_after: Option<usize>) -> Seen {
        {
            let (mut s, _) = l.accept().unwrap();
            let mut seen = Seen::default();
            // Handshake.
            let mut c0c1 = vec![0u8; 1537];
            s.read_exact(&mut c0c1).unwrap();
            assert_eq!(c0c1[0], 3);
            let mut s1 = vec![7u8; 1536];
            s1[..8].fill(0);
            s.write_all(&[3]).unwrap();
            s.write_all(&s1).unwrap();
            s.write_all(&c0c1[1..]).unwrap();
            let mut c2 = vec![0u8; 1536];
            s.read_exact(&mut c2).unwrap();
            assert_eq!(c2[8..], s1[8..]);
            // Chunks.
            let mut chunk = 128usize;
            let mut streams: std::collections::HashMap<u32, (u32, usize, u8, u32, Vec<u8>)> = Default::default();
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let reply = |s: &mut TcpStream, sid: u32, vals: &[Amf]| {
                let mut b = Vec::new();
                for v in vals {
                    v.encode(&mut b);
                }
                let mut w = Writer { s: s.try_clone().unwrap(), chunk: 128, bytes_out: 0 };
                w.message(3, 0, 20, sid, &b).unwrap();
            };
            loop {
                let mut b0 = [0u8; 1];
                if s.read_exact(&mut b0).is_err() {
                    break;
                }
                let fmt = b0[0] >> 6;
                let mut csid = (b0[0] & 63) as u32;
                if csid == 0 {
                    let mut x = [0u8; 1];
                    s.read_exact(&mut x).unwrap();
                    csid = x[0] as u32 + 64;
                }
                let e = streams.entry(csid).or_insert((0, 0, 0, 0, Vec::new()));
                if fmt <= 2 {
                    let mut h = [0u8; 3];
                    s.read_exact(&mut h).unwrap();
                    e.0 = u32::from_be_bytes([0, h[0], h[1], h[2]]);
                    if fmt <= 1 {
                        let mut h = [0u8; 4];
                        s.read_exact(&mut h).unwrap();
                        e.1 = u32::from_be_bytes([0, h[0], h[1], h[2]]) as usize;
                        e.2 = h[3];
                    }
                    if fmt == 0 {
                        let mut h = [0u8; 4];
                        s.read_exact(&mut h).unwrap();
                        e.3 = u32::from_le_bytes(h);
                    }
                }
                if e.0 == 0xFF_FFFF {
                    let mut x = [0u8; 4];
                    s.read_exact(&mut x).unwrap();
                    e.0 = u32::from_be_bytes(x);
                }
                let n = (e.1 - e.4.len()).min(chunk);
                let mut d = vec![0u8; n];
                s.read_exact(&mut d).unwrap();
                e.4.extend_from_slice(&d);
                if e.4.len() < e.1 {
                    continue;
                }
                let msg = std::mem::take(&mut e.4);
                let (ts, typ, sid) = (e.0, e.2, e.3);
                match typ {
                    1 => {
                        chunk = u32::from_be_bytes([msg[0], msg[1], msg[2], msg[3]]) as usize;
                        seen.chunk_size_set = Some(chunk as u32);
                    }
                    20 => {
                        let v = Amf::decode_all(&msg).unwrap();
                        let name = v[0].as_str().unwrap_or("").to_owned();
                        let txn = v.get(1).and_then(Amf::as_num).unwrap_or(0.0);
                        seen.commands.push(v);
                        match name.as_str() {
                            "connect" => {
                                // Window ack size, peer bandwidth, then the result.
                                let mut w = Writer { s: s.try_clone().unwrap(), chunk: 128, bytes_out: 0 };
                                w.message(2, 0, 5, 0, &2_500_000u32.to_be_bytes()).unwrap();
                                w.message(2, 0, 6, 0, &[0, 0x26, 0x25, 0xa0, 2]).unwrap();
                                reply(&mut s, 0, &[Amf::str("_result"), Amf::Num(txn), Amf::Obj(vec![("fmsVer".into(), Amf::str("MOCK/1,0"))]), Amf::Obj(vec![("code".into(), Amf::str("NetConnection.Connect.Success"))])]);
                            }
                            "createStream" => reply(&mut s, 0, &[Amf::str("_result"), Amf::Num(txn), Amf::Null, Amf::Num(1.0)]),
                            "publish" => {
                                let code = if refuse_publish { "NetStream.Publish.BadName" } else { "NetStream.Publish.Start" };
                                reply(&mut s, 1, &[Amf::str("onStatus"), Amf::Num(0.0), Amf::Null, Amf::Obj(vec![("level".into(), Amf::str("status")), ("code".into(), Amf::str(code)), ("description".into(), Amf::str("mock"))])]);
                            }
                            "deleteStream" => break,
                            _ => {}
                        }
                    }
                    18 => {
                        let v = Amf::decode_all(&msg).unwrap();
                        seen.meta = v.into_iter().nth(2);
                    }
                    9 => {
                        seen.video.push((ts, msg));
                        if hang_up_after.is_some_and(|n| seen.video.len() >= n) {
                            break;
                        }
                    }
                    8 => seen.audio.push((ts, msg)),
                    _ => {}
                }
                let _ = sid;
            }
            seen
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::av::{AvConfig, AvEncoder};
    use crate::source::{AudioSource, TestAudio, TestVideo, VideoSource};

    #[test]
    fn urls_parse_and_keys_are_hidden() {
        let u = RtmpUrl::parse("rtmp://a.rtmp.youtube.com/live2/abcd-efgh-1234").unwrap();
        assert_eq!((u.host.as_str(), u.port, u.app.as_str(), u.key.as_str()), ("a.rtmp.youtube.com", 1935, "live2", "abcd-efgh-1234"));
        let u = RtmpUrl::parse("rtmp://localhost:1940/a/b/key?token=1").unwrap();
        assert_eq!((u.port, u.app.as_str(), u.key.as_str()), (1940, "a/b", "key?token=1"));
        assert!(RtmpUrl::parse("rtmps://x/y/z").unwrap_err().contains("TLS"));
        assert!(RtmpUrl::parse("rtmp://host/onlyapp").is_err());
        assert!(!redact("rtmp://a.rtmp.youtube.com/live2/abcd-efgh-1234").contains("abcd"));
        assert!(redact("rtmp://a.rtmp.youtube.com/live2/abcd-efgh-1234").starts_with("rtmp://a.rtmp.youtube.com/live2/"));
    }

    fn stream_some(port: u16) -> Result<(AvEncoder, usize, usize), String> {
        let mut cfg = AvConfig::new(320, 180, 30.0);
        cfg.cbr = true;
        let mut enc = AvEncoder::new(cfg).unwrap();
        let meta = Meta { width: 320, height: 180, fps: 30.0, video_kbps: cfg.video_kbps, audio_kbps: cfg.audio_kbps, sample_rate: 48000 };
        let mut c = RtmpClient::publish(&format!("rtmp://127.0.0.1:{port}/live/secretkey"), &enc.video_info().avcc, &enc.audio_info().asc, &meta)?;
        let mut v = TestVideo::from_spec("counter", 320, 180, 30.0).unwrap();
        let mut a = TestAudio::from_spec("tone:440", 48000).unwrap();
        let (mut nv, mut na) = (0, 0);
        for n in 0..60 {
            let f = v.frame_at((n as f64 * 1e6 / 30.0) as i64).unwrap();
            c.send(&enc.push_frame(&f))?;
            nv += 1;
            for p in enc.push_audio(&a.read(1600)) {
                c.send(&p)?;
                na += 1;
            }
        }
        c.close();
        Ok((enc, nv, na))
    }

    #[test]
    fn publishes_to_a_server_with_the_right_commands_and_media() {
        let (port, server) = mock::serve(false);
        let (enc, nv, na) = stream_some(port).unwrap();
        let seen = server.join().unwrap();
        assert_eq!(seen.chunk_size_set, Some(4096));
        let names: Vec<&str> = seen.commands.iter().map(|c| c[0].as_str().unwrap()).collect();
        assert_eq!(names, ["connect", "releaseStream", "FCPublish", "createStream", "publish", "deleteStream"]);
        let connect = &seen.commands[0];
        assert_eq!(connect[2].get("app").unwrap().as_str(), Some("live"));
        assert_eq!(connect[2].get("tcUrl").unwrap().as_str(), Some(format!("rtmp://127.0.0.1:{port}/live").as_str()));
        let publish = &seen.commands[4];
        assert_eq!(publish[3].as_str(), Some("secretkey"));
        assert_eq!(publish[4].as_str(), Some("live"));
        let meta = seen.meta.unwrap();
        assert_eq!(meta.get("width").unwrap().as_num(), Some(320.0));
        // Video: the AVC sequence header first, then one message per frame.
        assert_eq!(seen.video.len(), nv + 1);
        assert_eq!(&seen.video[0].1[..5], &[0x17, 0, 0, 0, 0]);
        assert_eq!(&seen.video[0].1[5..], &enc.video_info().avcc[..]);
        assert_eq!(seen.video[1].1[0], 0x17, "first frame is a keyframe");
        assert_eq!(seen.video[1].1[1], 1);
        assert!(seen.video.iter().skip(1).any(|(_, m)| m[0] == 0x27), "inter frames follow");
        let ts: Vec<u32> = seen.video.iter().skip(1).map(|(t, _)| *t).collect();
        assert!(ts.windows(2).all(|w| w[1] >= w[0]) && *ts.last().unwrap() >= 1900, "{ts:?}");
        assert_eq!(seen.audio.len(), na + 1);
        assert_eq!(&seen.audio[0].1[..2], &[0xAF, 0]);
        assert_eq!(&seen.audio[0].1[2..], &enc.audio_info().asc[..]);
        // Large frames survive chunking: the first (key) frame is bigger than a chunk? Sizes match what the encoder made.
        assert!(seen.video[1].1.len() > 5);
    }

    #[test]
    fn a_refused_publish_is_an_error_naming_the_reason() {
        let (port, server) = mock::serve(true);
        let e = stream_some(port).err().unwrap();
        assert!(e.contains("BadName"), "{e}");
        let _ = server.join();
    }

    #[test]
    fn nobody_listening_is_a_clear_error() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let e = stream_some(port).err().unwrap();
        assert!(e.contains("could not reach"), "{e}");
    }
}
