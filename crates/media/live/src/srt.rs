// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! SRT (Secure Reliable Transport) sending, in Rust: the caller side of the
//! handshake, live-mode data packets over UDP, acknowledgements, and
//! retransmission of what the receiver reports lost, carrying the program as
//! MPEG-TS. Encrypted streams (a passphrase) are not supported yet.
//!
//! `srt://host:port?streamid=name&latency=120` (latency in milliseconds).

use crate::av::Packet;
use crate::ts::TsMuxer;
use std::collections::VecDeque;
use std::net::{ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PAYLOAD: usize = 1316;
const HS: u16 = 0;
const KEEPALIVE: u16 = 1;
const ACK: u16 = 2;
const NAK: u16 = 3;
const SHUTDOWN: u16 = 5;
const ACKACK: u16 = 6;
const DROPREQ: u16 = 7;

#[derive(Clone, Debug, PartialEq)]
pub struct SrtUrl {
    pub host: String,
    pub port: u16,
    pub stream_id: Option<String>,
    pub latency_ms: u16,
}

impl SrtUrl {
    pub fn parse(url: &str) -> Result<SrtUrl, String> {
        let rest = url.strip_prefix("srt://").ok_or_else(|| format!("not an SRT address: {}", redact(url)))?;
        let (hostport, query) = rest.split_once('?').unwrap_or((rest, ""));
        let hostport = hostport.trim_end_matches('/');
        let (host, port) = hostport.rsplit_once(':').ok_or("the address needs a port: srt://host:port")?;
        let port = port.parse::<u16>().map_err(|_| format!("bad port \"{port}\""))?;
        let (mut stream_id, mut latency_ms) = (None, 120u16);
        for kv in query.split('&').filter(|s| !s.is_empty()) {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            match k {
                "streamid" => stream_id = Some(v.to_owned()),
                "latency" => latency_ms = v.parse().map_err(|_| format!("bad latency \"{v}\""))?,
                "passphrase" => return Err("encrypted SRT (passphrase) is not supported yet".into()),
                _ => {}
            }
        }
        if host.is_empty() {
            return Err("the address needs a host: srt://host:port".into());
        }
        Ok(SrtUrl { host: host.into(), port, stream_id, latency_ms })
    }
}

/// The address with any stream id hidden, for messages.
pub fn redact(url: &str) -> String {
    match url.split_once("streamid=") {
        Some((head, tail)) => {
            let rest = tail.split_once('&').map(|(_, r)| format!("&{r}")).unwrap_or_default();
            format!("{head}streamid=****{rest}")
        }
        None => url.to_owned(),
    }
}

fn now_ts(start: Instant) -> u32 {
    start.elapsed().as_micros() as u32
}

fn ctrl(typ: u16, sub: u16, info: u32, ts: u32, dest: u32, cif: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(16 + cif.len());
    p.extend_from_slice(&(0x8000_0000u32 | (typ as u32) << 16 | sub as u32).to_be_bytes());
    p.extend_from_slice(&info.to_be_bytes());
    p.extend_from_slice(&ts.to_be_bytes());
    p.extend_from_slice(&dest.to_be_bytes());
    p.extend_from_slice(cif);
    p
}

#[allow(clippy::too_many_arguments)]
fn handshake_cif(version: u32, enc: u16, ext: u16, isn: u32, mtu: u32, window: u32, hs_type: u32, sock: u32, cookie: u32, extensions: &[u8]) -> Vec<u8> {
    let mut c = Vec::with_capacity(48 + extensions.len());
    c.extend_from_slice(&version.to_be_bytes());
    c.extend_from_slice(&enc.to_be_bytes());
    c.extend_from_slice(&ext.to_be_bytes());
    c.extend_from_slice(&isn.to_be_bytes());
    c.extend_from_slice(&mtu.to_be_bytes());
    c.extend_from_slice(&window.to_be_bytes());
    c.extend_from_slice(&hs_type.to_be_bytes());
    c.extend_from_slice(&sock.to_be_bytes());
    c.extend_from_slice(&cookie.to_be_bytes());
    c.extend_from_slice(&[0u8; 16]);
    c.extend_from_slice(extensions);
    c
}

/// A stream id as SRT wants it: padded to four bytes, each group reversed.
pub fn stream_id_bytes(id: &str) -> Vec<u8> {
    let mut b = id.as_bytes().to_vec();
    while b.len() % 4 != 0 {
        b.push(0);
    }
    for g in b.chunks_mut(4) {
        g.reverse();
    }
    b
}

#[derive(Default)]
pub struct Stats {
    pub packets: AtomicU64,
    pub retransmitted: AtomicU64,
    pub bytes: AtomicU64,
}

struct Shared {
    sock: UdpSocket,
    peer_id: u32,
    start: Instant,
    next_seq: u32,
    msg: u32,
    /// Sent and not yet acknowledged: (seq, whole packet).
    sent: VecDeque<(u32, Vec<u8>)>,
    last_sent: Instant,
    dropped_by_buffer: u64,
}

pub struct SrtSender {
    sh: Arc<Mutex<Shared>>,
    mux: TsMuxer,
    stats: Arc<Stats>,
    alive: Arc<AtomicBool>,
    pending: Vec<u8>,
    pub url: String,
    latency: Duration,
}

fn lt(a: u32, b: u32) -> bool {
    // Sequence numbers are 31-bit and wrap.
    a != b && (b.wrapping_sub(a) & 0x7FFF_FFFF) < 0x4000_0000
}

impl SrtSender {
    pub fn connect(url: &str, sps: &[u8], pps: &[u8], sample_rate: u32) -> Result<SrtSender, String> {
        let u = SrtUrl::parse(url)?;
        let addr = (u.host.as_str(), u.port).to_socket_addrs().map_err(|e| format!("could not find {}: {e}", u.host))?.next().ok_or_else(|| format!("could not find {}", u.host))?;
        let sock = UdpSocket::bind(if addr.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" }).map_err(|e| format!("could not open a UDP port: {e}"))?;
        sock.connect(addr).map_err(|e| e.to_string())?;
        sock.set_read_timeout(Some(Duration::from_millis(250))).ok();
        let start = Instant::now();
        let my_id = (std::process::id().wrapping_mul(2654435761) ^ 0x5eed_1234) & 0x7FFF_FFFF | 1;
        let isn = (start.elapsed().as_nanos() as u32 ^ my_id.rotate_left(7)) & 0x7FFF_FFFF;
        // 1. Induction.
        let ind = ctrl(HS, 0, 0, now_ts(start), 0, &handshake_cif(4, 2, 2, isn, 1500, 8192, 1, my_id, 0, &[]));
        let mut resp = None;
        let mut buf = vec![0u8; 2048];
        for _ in 0..12 {
            sock.send(&ind).map_err(|e| format!("could not send to {}:{}: {e}", u.host, u.port))?;
            if let Ok(n) = sock.recv(&mut buf) {
                if n >= 64 && buf[0] & 0x80 != 0 && (u16::from_be_bytes([buf[0], buf[1]]) & 0x7FFF) == HS {
                    resp = Some(buf[..n].to_vec());
                    break;
                }
            }
        }
        let resp = resp.ok_or_else(|| format!("no SRT listener answered at {}:{}", u.host, u.port))?;
        // The cookie sits at CIF offset 28.
        let cookie = u32::from_be_bytes([resp[16 + 28], resp[16 + 29], resp[16 + 30], resp[16 + 31]]);
        // 2. Conclusion with the handshake and (optionally) stream id extensions.
        let lat = u.latency_ms as u32;
        let mut ext = Vec::new();
        ext.extend_from_slice(&1u16.to_be_bytes()); // HSREQ
        ext.extend_from_slice(&3u16.to_be_bytes());
        ext.extend_from_slice(&0x0001_0504u32.to_be_bytes());
        ext.extend_from_slice(&0x0000_003Bu32.to_be_bytes());
        ext.extend_from_slice(&(lat << 16 | lat).to_be_bytes());
        let mut flags = 1u16;
        if let Some(id) = &u.stream_id {
            let b = stream_id_bytes(id);
            ext.extend_from_slice(&5u16.to_be_bytes());
            ext.extend_from_slice(&((b.len() / 4) as u16).to_be_bytes());
            ext.extend_from_slice(&b);
            flags |= 4;
        }
        let conc = ctrl(HS, 0, 0, now_ts(start), 0, &handshake_cif(5, 0, flags, isn, 1500, 8192, 0xFFFF_FFFF, my_id, cookie, &ext));
        let mut peer_id = None;
        for _ in 0..12 {
            sock.send(&conc).map_err(|e| e.to_string())?;
            if let Ok(n) = sock.recv(&mut buf) {
                if n >= 64 && buf[0] & 0x80 != 0 && (u16::from_be_bytes([buf[0], buf[1]]) & 0x7FFF) == HS {
                    let hs_type = u32::from_be_bytes([buf[16 + 20], buf[16 + 21], buf[16 + 22], buf[16 + 23]]);
                    if hs_type == 0xFFFF_FFFF {
                        peer_id = Some(u32::from_be_bytes([buf[16 + 24], buf[16 + 25], buf[16 + 26], buf[16 + 27]]));
                        break;
                    }
                    // A rejection: the type is a reason code (1000+).
                    if hs_type >= 1000 {
                        return Err(format!("the SRT listener refused the connection (code {hs_type}){}", if hs_type == 1002 { ": bad stream id" } else { "" }));
                    }
                }
            }
        }
        let peer_id = peer_id.ok_or("the SRT listener did not accept the connection")?;
        sock.set_read_timeout(Some(Duration::from_millis(100))).ok();
        let sh = Arc::new(Mutex::new(Shared { sock: sock.try_clone().map_err(|e| e.to_string())?, peer_id, start, next_seq: isn, msg: 1, sent: VecDeque::new(), last_sent: Instant::now(), dropped_by_buffer: 0 }));
        let stats = Arc::new(Stats::default());
        let alive = Arc::new(AtomicBool::new(true));
        {
            let (sh, stats, alive) = (sh.clone(), stats.clone(), alive.clone());
            std::thread::spawn(move || reader(sock, sh, stats, alive));
        }
        Ok(SrtSender { sh, mux: TsMuxer::new(sps, pps, sample_rate), stats, alive, pending: Vec::new(), url: redact(url), latency: Duration::from_millis(u.latency_ms as u64) })
    }

    pub fn send(&mut self, p: &Packet) -> Result<(), String> {
        if !self.alive.load(Ordering::Relaxed) {
            return Err("the SRT receiver closed the connection or stopped answering".into());
        }
        self.mux.push(p);
        self.pending.extend(self.mux.take());
        // Whole datagrams of seven TS packets; the rest waits.
        let whole = self.pending.len() / PAYLOAD * PAYLOAD;
        if whole == 0 {
            return Ok(());
        }
        let data: Vec<u8> = self.pending.drain(..whole).collect();
        let mut g = self.sh.lock().map_err(|_| "poisoned")?;
        for chunk in data.chunks(PAYLOAD) {
            let seq = g.next_seq;
            g.next_seq = (seq + 1) & 0x7FFF_FFFF;
            let msg = g.msg;
            g.msg = (msg + 1) & 0x03FF_FFFF;
            let mut pkt = Vec::with_capacity(16 + chunk.len());
            pkt.extend_from_slice(&seq.to_be_bytes());
            // PP = solo, in order, no key, not retransmitted.
            pkt.extend_from_slice(&(0xC000_0000u32 | 1 << 29 | msg).to_be_bytes());
            pkt.extend_from_slice(&now_ts(g.start).to_be_bytes());
            pkt.extend_from_slice(&g.peer_id.to_be_bytes());
            pkt.extend_from_slice(chunk);
            g.sock.send(&pkt).map_err(|e| format!("could not send: {e}"))?;
            g.sent.push_back((seq, pkt));
            self.stats.packets.fetch_add(1, Ordering::Relaxed);
            self.stats.bytes.fetch_add(chunk.len() as u64, Ordering::Relaxed);
        }
        g.last_sent = Instant::now();
        // Never keep more than a few seconds or 8000 packets unacknowledged.
        let cap = 8000.min((self.latency.as_millis() as usize * 40).max(2000));
        while g.sent.len() > cap {
            g.sent.pop_front();
            g.dropped_by_buffer += 1;
        }
        Ok(())
    }

    pub fn stats_handle(&self) -> Arc<Stats> {
        self.stats.clone()
    }

    pub fn packets_sent(&self) -> u64 {
        self.stats.packets.load(Ordering::Relaxed)
    }

    pub fn retransmitted(&self) -> u64 {
        self.stats.retransmitted.load(Ordering::Relaxed)
    }

    pub fn close(self) {
        // Let what was sent be acknowledged (and what was lost be resent)
        // before the stream is ended.
        let deadline = Instant::now() + Duration::from_millis((self.latency.as_millis() as u64 * 4).clamp(500, 5000));
        while Instant::now() < deadline && self.alive.load(Ordering::Relaxed) {
            if self.sh.lock().map(|g| g.sent.is_empty()).unwrap_or(true) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if let Ok(g) = self.sh.lock() {
            let p = ctrl(SHUTDOWN, 0, 0, now_ts(g.start), g.peer_id, &[0, 0, 0, 0]);
            let _ = g.sock.send(&p);
        }
        self.alive.store(false, Ordering::Relaxed);
    }
}

fn reader(sock: UdpSocket, sh: Arc<Mutex<Shared>>, stats: Arc<Stats>, alive: Arc<AtomicBool>) {
    let mut buf = vec![0u8; 2048];
    let mut last_heard = Instant::now();
    while alive.load(Ordering::Relaxed) {
        match sock.recv(&mut buf) {
            Ok(n) if n >= 16 => {
                last_heard = Instant::now();
                if buf[0] & 0x80 == 0 {
                    continue;
                }
                let typ = u16::from_be_bytes([buf[0], buf[1]]) & 0x7FFF;
                let info = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
                let cif = &buf[16..n];
                let Ok(mut g) = sh.lock() else { return };
                match typ {
                    ACK if cif.len() >= 4 => {
                        let upto = u32::from_be_bytes([cif[0], cif[1], cif[2], cif[3]]) & 0x7FFF_FFFF;
                        while g.sent.front().is_some_and(|(s, _)| lt(*s, upto)) {
                            g.sent.pop_front();
                        }
                        let a = ctrl(ACKACK, 0, info, now_ts(g.start), g.peer_id, &[]);
                        let _ = g.sock.send(&a);
                    }
                    NAK => {
                        let mut ranges = Vec::new();
                        let mut i = 0;
                        while i + 4 <= cif.len() {
                            let w = u32::from_be_bytes([cif[i], cif[i + 1], cif[i + 2], cif[i + 3]]);
                            if w & 0x8000_0000 != 0 && i + 8 <= cif.len() {
                                ranges.push((w & 0x7FFF_FFFF, u32::from_be_bytes([cif[i + 4], cif[i + 5], cif[i + 6], cif[i + 7]]) & 0x7FFF_FFFF));
                                i += 8;
                            } else {
                                ranges.push((w, w));
                                i += 4;
                            }
                        }
                        for (first, last) in ranges {
                            let mut s = first;
                            let mut missing: Option<(u32, u32)> = None;
                            loop {
                                if let Some((_, pkt)) = g.sent.iter().find(|(q, _)| *q == s) {
                                    let mut p = pkt.clone();
                                    p[4] |= 0x04; // R: retransmitted
                                    let _ = g.sock.send(&p);
                                    stats.retransmitted.fetch_add(1, Ordering::Relaxed);
                                } else {
                                    missing = Some(match missing {
                                        Some((a, _)) => (a, s),
                                        None => (s, s),
                                    });
                                }
                                if s == last || !lt(s, last) {
                                    break;
                                }
                                s = (s + 1) & 0x7FFF_FFFF;
                            }
                            // What we no longer hold, the receiver is told to skip.
                            if let Some((a, b)) = missing {
                                let mut c = Vec::new();
                                c.extend_from_slice(&a.to_be_bytes());
                                c.extend_from_slice(&b.to_be_bytes());
                                let p = ctrl(DROPREQ, 0, 0, now_ts(g.start), g.peer_id, &c);
                                let _ = g.sock.send(&p);
                            }
                        }
                    }
                    SHUTDOWN => {
                        alive.store(false, Ordering::Relaxed);
                    }
                    HS => {
                        // A repeated conclusion response: nothing to do.
                    }
                    _ => {}
                }
            }
            Ok(_) => {}
            Err(_) => {}
        }
        // Keep the link alive when nothing is being sent.
        if let Ok(g) = sh.lock() {
            if g.last_sent.elapsed() > Duration::from_millis(900) {
                let k = ctrl(KEEPALIVE, 0, 0, now_ts(g.start), g.peer_id, &[]);
                let _ = g.sock.send(&k);
            }
        }
        if last_heard.elapsed() > Duration::from_secs(6) {
            alive.store(false, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
pub(crate) mod mock {
    //! An SRT listener for tests, written apart from the sender: it answers
    //! the handshake, acknowledges, reports gaps, and hands back the payload
    //! it received in order. `drop_every` throws away every nth first
    //! transmission to exercise retransmission.
    use super::*;

    pub struct Got {
        pub payload: Vec<u8>,
        pub stream_id: Option<String>,
        pub latency: u32,
        pub retransmits_seen: u32,
        pub keepalives: u32,
        pub shutdown: bool,
    }

    pub fn listen(drop_every: u32, expect_bytes: usize) -> (u16, std::thread::JoinHandle<Got>) {
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = s.local_addr().unwrap().port();
        s.set_read_timeout(Some(Duration::from_millis(10))).unwrap();
        let h = std::thread::spawn(move || {
            let my_id = 0x1234_5678u32;
            let mut got = Got { payload: Vec::new(), stream_id: None, latency: 0, retransmits_seen: 0, keepalives: 0, shutdown: false };
            let mut buf = vec![0u8; 2048];
            let mut caller_id = 0;
            let mut peer = None;
            let mut next_seq: Option<u32> = None;
            let mut held: std::collections::BTreeMap<u32, Vec<u8>> = Default::default();
            let mut seen_first: std::collections::HashSet<u32> = Default::default();
            let mut ack_no = 1u32;
            let t0 = Instant::now();
            let mut last_ack = Instant::now();
            let mut last_nak = Instant::now();
            let mut count = 0u32;
            while t0.elapsed() < Duration::from_secs(20) && !got.shutdown {
                if let Ok((n, from)) = s.recv_from(&mut buf) {
                    peer = Some(from);
                    if buf[0] & 0x80 != 0 {
                        let typ = u16::from_be_bytes([buf[0], buf[1]]) & 0x7FFF;
                        match typ {
                            HS => {
                                let cif = &buf[16..n];
                                let version = u32::from_be_bytes(cif[0..4].try_into().unwrap());
                                caller_id = u32::from_be_bytes(cif[24..28].try_into().unwrap());
                                let isn = u32::from_be_bytes(cif[8..12].try_into().unwrap());
                                if version == 4 {
                                    // Induction response: version 5, magic 0x4A17, a cookie.
                                    let r = ctrl(HS, 0, 0, 0, caller_id, &handshake_cif(5, 0, 0x4A17, isn, 1500, 8192, 1, my_id, 0xC00C_1E00, &[]));
                                    s.send_to(&r, from).unwrap();
                                } else {
                                    assert_eq!(u32::from_be_bytes(cif[28..32].try_into().unwrap()), 0xC00C_1E00, "cookie echoed");
                                    next_seq = Some(isn);
                                    // Extensions.
                                    let mut e = &cif[48..];
                                    while e.len() >= 4 {
                                        let t = u16::from_be_bytes([e[0], e[1]]);
                                        let l = u16::from_be_bytes([e[2], e[3]]) as usize * 4;
                                        let body = &e[4..4 + l];
                                        if t == 1 {
                                            got.latency = u32::from_be_bytes(body[8..12].try_into().unwrap()) >> 16;
                                        } else if t == 5 {
                                            let mut b = body.to_vec();
                                            for g in b.chunks_mut(4) {
                                                g.reverse();
                                            }
                                            got.stream_id = Some(String::from_utf8_lossy(&b).trim_end_matches('\0').to_owned());
                                        }
                                        e = &e[4 + l..];
                                    }
                                    let mut ext = Vec::new();
                                    ext.extend_from_slice(&2u16.to_be_bytes());
                                    ext.extend_from_slice(&3u16.to_be_bytes());
                                    ext.extend_from_slice(&0x0001_0504u32.to_be_bytes());
                                    ext.extend_from_slice(&0x3Bu32.to_be_bytes());
                                    ext.extend_from_slice(&(got.latency << 16 | got.latency).to_be_bytes());
                                    let r = ctrl(HS, 0, 0, 0, caller_id, &handshake_cif(5, 0, 1, isn, 1500, 8192, 0xFFFF_FFFF, my_id, 0, &ext));
                                    s.send_to(&r, from).unwrap();
                                }
                            }
                            KEEPALIVE => got.keepalives += 1,
                            SHUTDOWN => got.shutdown = true,
                            DROPREQ => {
                                let cif = &buf[16..n];
                                let (a, b) = (u32::from_be_bytes(cif[0..4].try_into().unwrap()), u32::from_be_bytes(cif[4..8].try_into().unwrap()));
                                if let Some(ns) = next_seq {
                                    if !lt(b, ns) {
                                        next_seq = Some((b + 1) & 0x7FFF_FFFF);
                                    }
                                    let _ = a;
                                }
                            }
                            _ => {}
                        }
                    } else {
                        // Data.
                        let seq = u32::from_be_bytes(buf[0..4].try_into().unwrap());
                        assert_eq!(u32::from_be_bytes(buf[12..16].try_into().unwrap()), my_id, "data goes to the listener's socket id");
                        let retrans = buf[4] & 0x04 != 0;
                        if retrans {
                            got.retransmits_seen += 1;
                        }
                        let first = seen_first.insert(seq);
                        count += first as u32;
                        if drop_every > 0 && first && !retrans && count % drop_every == 0 {
                            continue; // lost on the wire
                        }
                        held.insert(seq, buf[16..n].to_vec());
                        while let Some(ns) = next_seq {
                            match held.remove(&ns) {
                                Some(p) => {
                                    got.payload.extend_from_slice(&p);
                                    next_seq = Some((ns + 1) & 0x7FFF_FFFF);
                                }
                                None => break,
                            }
                        }
                    }
                }
                let Some(from) = peer else { continue };
                if next_seq.is_some() && last_ack.elapsed() > Duration::from_millis(10) {
                    last_ack = Instant::now();
                    let mut c = Vec::new();
                    for v in [next_seq.unwrap(), 50_000, 25_000, 8192, 1000, 10_000, 10_000] {
                        c.extend_from_slice(&v.to_be_bytes());
                    }
                    s.send_to(&ctrl(ACK, 0, ack_no, 0, caller_id, &c), from).unwrap();
                    ack_no += 1;
                }
                if let (Some(ns), true) = (next_seq, last_nak.elapsed() > Duration::from_millis(20)) {
                    // Gaps: what is held beyond the next expected sequence.
                    if let Some((&first_held, _)) = held.iter().next() {
                        last_nak = Instant::now();
                        let mut c = Vec::new();
                        if first_held == ns {
                            continue;
                        }
                        if (first_held.wrapping_sub(ns) & 0x7FFF_FFFF) == 1 {
                            c.extend_from_slice(&ns.to_be_bytes());
                        } else {
                            c.extend_from_slice(&(ns | 0x8000_0000).to_be_bytes());
                            c.extend_from_slice(&((first_held - 1) & 0x7FFF_FFFF).to_be_bytes());
                        }
                        s.send_to(&ctrl(NAK, 0, 0, 0, caller_id, &c), from).unwrap();
                    }
                }
                if got.payload.len() >= expect_bytes && expect_bytes > 0 && got.shutdown {
                    break;
                }
            }
            got
        });
        (port, h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::av::{AvConfig, AvEncoder};
    use crate::source::{AudioSource, TestAudio, TestVideo, VideoSource};

    #[test]
    fn urls_parse_and_stream_ids_are_hidden() {
        let u = SrtUrl::parse("srt://ingest.example.com:9000?streamid=live/secret&latency=200").unwrap();
        assert_eq!((u.host.as_str(), u.port, u.stream_id.as_deref(), u.latency_ms), ("ingest.example.com", 9000, Some("live/secret"), 200));
        assert!(SrtUrl::parse("srt://h:1?passphrase=abc").unwrap_err().contains("not supported"));
        assert!(SrtUrl::parse("srt://hostonly").is_err());
        assert!(!redact("srt://h:1?streamid=live/secret&latency=1").contains("secret"));
        assert_eq!(stream_id_bytes("abcde"), [b'd', b'c', b'b', b'a', 0, 0, 0, b'e']);
    }

    fn run(drop_every: u32, url_extra: &str) -> (mock::Got, Vec<u8>, u64) {
        let (port, server) = mock::listen(drop_every, 0);
        let mut cfg = AvConfig::new(320, 180, 30.0);
        cfg.cbr = true;
        let mut enc = AvEncoder::new(cfg).unwrap();
        let mut s = SrtSender::connect(&format!("srt://127.0.0.1:{port}{url_extra}"), enc.sps(), enc.pps(), 48000).unwrap();
        // What the muxer would produce, to compare against what arrives.
        let mut reference = TsMuxer::new(enc.sps(), enc.pps(), 48000);
        let mut v = TestVideo::from_spec("counter", 320, 180, 30.0).unwrap();
        let mut a = TestAudio::from_spec("tone:440", 48000).unwrap();
        for n in 0..90 {
            let f = v.frame_at((n as f64 * 1e6 / 30.0) as i64).unwrap();
            let p = enc.push_frame(&f);
            reference.push(&p);
            s.send(&p).unwrap();
            for p in enc.push_audio(&a.read(1600)) {
                reference.push(&p);
                s.send(&p).unwrap();
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        // Closing waits for the receiver to have everything.
        let st = s.stats_handle();
        s.close();
        let retrans = st.retransmitted.load(Ordering::Relaxed);
        let got = server.join().unwrap();
        (got, reference.take(), retrans)
    }

    #[test]
    fn streams_ts_to_a_listener_in_order_with_stream_id_and_latency() {
        let (got, reference, retrans) = run(0, "?streamid=studio/show&latency=150");
        assert_eq!(got.stream_id.as_deref(), Some("studio/show"));
        assert_eq!(got.latency, 150);
        assert!(got.shutdown);
        assert_eq!(retrans, 0);
        let whole = reference.len() / PAYLOAD * PAYLOAD;
        assert!(got.payload.len() >= whole, "{} of {}", got.payload.len(), whole);
        assert_eq!(&got.payload[..whole], &reference[..whole]);
    }

    #[test]
    fn lost_packets_are_asked_for_and_resent() {
        let (got, reference, retrans) = run(7, "");
        assert!(retrans >= 3, "retransmitted {retrans}");
        assert!(got.retransmits_seen >= 3);
        let whole = reference.len() / PAYLOAD * PAYLOAD;
        assert!(got.payload.len() >= whole, "{} of {}", got.payload.len(), whole);
        assert_eq!(&got.payload[..whole], &reference[..whole], "every byte arrived in order despite the loss");
    }

    #[test]
    fn nobody_listening_is_a_clear_error() {
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = s.local_addr().unwrap().port();
        drop(s);
        let e = SrtSender::connect(&format!("srt://127.0.0.1:{port}"), &[0x67], &[0x68], 48000).err().unwrap();
        assert!(e.contains("no SRT listener") || e.contains("could not send"), "{e}");
    }
}
