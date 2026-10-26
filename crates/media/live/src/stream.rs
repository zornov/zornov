// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A live stream out: RTMP or SRT behind one interface, on its own thread
//! with a bounded queue, so a slow network never stalls the program. When
//! the queue is full, pictures are dropped until the next keyframe (the
//! stream stays decodable); when the connection breaks, it reconnects and
//! resumes on a keyframe.

use crate::av::Packet;
use crate::rtmp::{Meta, RtmpClient};
use crate::srt::SrtSender;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What the codecs look like, which the transports announce.
#[derive(Clone, Debug)]
pub struct StreamInfo {
    pub avcc: Vec<u8>,
    pub sps: Vec<u8>,
    pub pps: Vec<u8>,
    pub asc: Vec<u8>,
    pub meta: Meta,
}

enum Transport {
    Rtmp(RtmpClient),
    Srt(SrtSender),
}

impl Transport {
    fn open(url: &str, info: &StreamInfo) -> Result<Transport, String> {
        if url.starts_with("rtmp://") || url.starts_with("rtmps://") {
            RtmpClient::publish(url, &info.avcc, &info.asc, &info.meta).map(Transport::Rtmp)
        } else if url.starts_with("srt://") {
            SrtSender::connect(url, &info.sps, &info.pps, info.meta.sample_rate).map(Transport::Srt)
        } else {
            Err(format!("cannot stream to {}: use rtmp://host/app/key or srt://host:port", crate::rtmp::redact(url)))
        }
    }

    fn send(&mut self, p: &Packet) -> Result<(), String> {
        match self {
            Transport::Rtmp(c) => c.send(p),
            Transport::Srt(s) => s.send(p),
        }
    }

    fn close(self) {
        match self {
            Transport::Rtmp(c) => c.close(),
            Transport::Srt(s) => s.close(),
        }
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct StreamStatus {
    pub target: String,
    /// connecting, live, reconnecting, failed or stopped.
    pub state: String,
    pub sent: u64,
    pub dropped: u64,
    pub reconnects: u32,
    pub last_error: Option<String>,
    pub live_seconds: f64,
}

pub struct Stream {
    pub label: String,
    tx: SyncSender<Packet>,
    status: Arc<Mutex<StreamStatus>>,
    sent: Arc<AtomicU64>,
    dropped: Arc<AtomicU64>,
    need_key: AtomicBool,
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Stream {
    /// Connects now (so a wrong address or key is an error right away) and
    /// then keeps the connection up on a thread.
    pub fn start(url: &str, info: StreamInfo) -> Result<Stream, String> {
        let label = if url.starts_with("srt://") { crate::srt::redact(url) } else { crate::rtmp::redact(url) };
        let first = Transport::open(url, &info)?;
        let status = Arc::new(Mutex::new(StreamStatus { target: label.clone(), state: "live".into(), sent: 0, dropped: 0, reconnects: 0, last_error: None, live_seconds: 0.0 }));
        let (tx, rx) = mpsc::sync_channel::<Packet>(600);
        let (sent, dropped, stop) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)), Arc::new(AtomicBool::new(false)));
        let (st, se, s2, url2) = (status.clone(), sent.clone(), stop.clone(), url.to_owned());
        let join = std::thread::spawn(move || {
            let mut t = Some(first);
            let started = Instant::now();
            let mut backoff = 1;
            let mut want_key = false;
            let set = |f: &dyn Fn(&mut StreamStatus)| {
                if let Ok(mut g) = st.lock() {
                    f(&mut g);
                    g.live_seconds = started.elapsed().as_secs_f64();
                }
            };
            loop {
                if s2.load(Ordering::Relaxed) {
                    break;
                }
                let p = match rx.recv_timeout(Duration::from_millis(200)) {
                    Ok(p) => p,
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => break,
                };
                if t.is_none() {
                    // Wait for a keyframe before trying again, so the far end
                    // starts on a picture it can decode.
                    if !matches!(p, Packet::Video { keyframe: true, .. }) {
                        continue;
                    }
                    set(&|s| s.state = "reconnecting".into());
                    match Transport::open(&url2, &info) {
                        Ok(n) => {
                            t = Some(n);
                            backoff = 1;
                            want_key = false;
                            set(&|s| {
                                s.state = "live".into();
                                s.reconnects += 1;
                            });
                        }
                        Err(e) => {
                            set(&|s| s.last_error = Some(e.clone()));
                            if backoff > 20 {
                                set(&|s| s.state = "failed".into());
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(500 * backoff as u64));
                            backoff *= 2;
                            continue;
                        }
                    }
                }
                if want_key && !matches!(p, Packet::Video { keyframe: true, .. }) {
                    continue;
                }
                want_key = false;
                if let Some(tr) = t.as_mut() {
                    match tr.send(&p) {
                        Ok(()) => {
                            se.fetch_add(1, Ordering::Relaxed);
                            set(&|s| s.sent = se.load(Ordering::Relaxed));
                        }
                        Err(e) => {
                            set(&|s| {
                                s.last_error = Some(e.clone());
                                s.state = "reconnecting".into();
                            });
                            if let Some(tr) = t.take() {
                                tr.close();
                            }
                        }
                    }
                }
            }
            if let Some(tr) = t {
                tr.close();
            }
            set(&|s| {
                if s.state != "failed" {
                    s.state = "stopped".into()
                }
            });
        });
        Ok(Stream { label, tx, status, sent, dropped, need_key: AtomicBool::new(false), stop, join: Some(join) })
    }

    /// Queues a packet. When the network cannot keep up, pictures are
    /// dropped up to the next keyframe.
    pub fn send(&self, p: &Packet) {
        let is_key = matches!(p, Packet::Video { keyframe: true, .. });
        if self.need_key.load(Ordering::Relaxed) {
            if matches!(p, Packet::Video { .. }) && !is_key {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                return;
            }
            if is_key {
                self.need_key.store(false, Ordering::Relaxed);
            }
        }
        match self.tx.try_send(p.clone()) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                if matches!(p, Packet::Video { .. }) {
                    self.need_key.store(true, Ordering::Relaxed);
                }
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    pub fn status(&self) -> StreamStatus {
        let mut s = self.status.lock().map(|g| g.clone()).unwrap();
        s.dropped = self.dropped.load(Ordering::Relaxed);
        s.sent = self.sent.load(Ordering::Relaxed);
        s
    }

    pub fn stop(mut self) -> StreamStatus {
        // Give the queue a moment to drain, then end.
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_millis(1500) && self.sent.load(Ordering::Relaxed) + self.dropped.load(Ordering::Relaxed) == 0 {
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(150));
        self.stop.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        self.status()
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::av::{AvConfig, AvEncoder};
    use crate::source::{TestVideo, VideoSource};

    #[test]
    fn a_dropped_connection_is_re_made_and_resumes_on_a_keyframe() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let first = crate::rtmp::mock::serve_on(&l, false, Some(20));
            let second = crate::rtmp::mock::serve_on(&l, false, None);
            (first, second)
        });
        let mut cfg = AvConfig::new(160, 96, 30.0);
        cfg.gop_secs = 0.5;
        cfg.cbr = true;
        let mut enc = AvEncoder::new(cfg).unwrap();
        let info = StreamInfo { avcc: enc.video_info().avcc, sps: enc.sps().to_vec(), pps: enc.pps().to_vec(), asc: enc.audio_info().asc, meta: Meta { width: 160, height: 96, fps: 30.0, video_kbps: cfg.video_kbps, audio_kbps: 160, sample_rate: 48000 } };
        let s = Stream::start(&format!("rtmp://127.0.0.1:{port}/live/k"), info).unwrap();
        let mut v = TestVideo::from_spec("counter", 160, 96, 30.0).unwrap();
        for n in 0..120 {
            s.send(&enc.push_frame(&v.frame_at((n as f64 * 1e6 / 30.0) as i64).unwrap()));
            std::thread::sleep(Duration::from_millis(15));
        }
        let st = s.stop();
        assert!(st.reconnects >= 1, "{st:?}");
        let (first, second) = server.join().unwrap();
        assert_eq!(first.video.len(), 20);
        assert!(second.video.len() > 5, "{}", second.video.len());
        // The second connection starts with the codec header and then a keyframe.
        assert_eq!(&second.video[0].1[..2], &[0x17, 0]);
        assert_eq!(second.video[1].1[0], 0x17);
    }
}
