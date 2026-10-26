// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Studio's RTMP publisher against a real server: ffmpeg in listen mode
//! receives the stream and ffprobe reads what arrived. Skipped when ffmpeg
//! is not installed.

use std::process::{Command, Stdio};
use studio_live::av::{AvConfig, AvEncoder};
use studio_live::rtmp::{Meta, RtmpClient};
use studio_live::source::{AudioSource, TestAudio, TestVideo, VideoSource};

fn have(tool: &str) -> bool {
    Command::new(tool).arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok()
}

#[test]
fn ffmpeg_receives_a_studio_rtmp_stream() {
    if !have("ffmpeg") || !have("ffprobe") {
        eprintln!("ffmpeg not installed: skipped");
        return;
    }
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let out = std::env::temp_dir().join(format!("studio-live-interop-{}.flv", std::process::id()));
    let mut child = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-listen", "1", "-i", &format!("rtmp://127.0.0.1:{port}/live/key"), "-c", "copy", "-f", "flv"])
        .arg(&out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Wait for it to listen.
    let t0 = std::time::Instant::now();
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(t0.elapsed().as_secs() < 10, "ffmpeg never listened");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // That probe connection used up ffmpeg's single accept; start again.
    let _ = child.kill();
    let _ = child.wait();
    let mut child = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-listen", "1", "-i", &format!("rtmp://127.0.0.1:{port}/live/key"), "-c", "copy", "-f", "flv"])
        .arg(&out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(800));
    let mut cfg = AvConfig::new(320, 180, 30.0);
    cfg.cbr = true;
    let mut enc = AvEncoder::new(cfg).unwrap();
    let meta = Meta { width: 320, height: 180, fps: 30.0, video_kbps: cfg.video_kbps, audio_kbps: cfg.audio_kbps, sample_rate: 48000 };
    let mut c = RtmpClient::publish(&format!("rtmp://127.0.0.1:{port}/live/key"), &enc.video_info().avcc, &enc.audio_info().asc, &meta).expect("publish to ffmpeg");
    let mut v = TestVideo::from_spec("counter", 320, 180, 30.0).unwrap();
    let mut a = TestAudio::from_spec("tone:440", 48000).unwrap();
    for n in 0..90 {
        let f = v.frame_at((n as f64 * 1e6 / 30.0) as i64).unwrap();
        c.send(&enc.push_frame(&f)).unwrap();
        for p in enc.push_audio(&a.read(1600)) {
            c.send(&p).unwrap();
        }
    }
    c.close();
    let status = child.wait().unwrap();
    let mut err = String::new();
    use std::io::Read;
    child.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    assert!(status.success(), "ffmpeg: {err}");
    let probe = Command::new("ffprobe").args(["-v", "error", "-count_frames", "-show_entries", "stream=codec_name,width,height,nb_read_frames", "-of", "csv=p=0"]).arg(&out).output().unwrap();
    let text = String::from_utf8_lossy(&probe.stdout);
    let _ = std::fs::remove_file(&out);
    assert!(text.contains("h264,320,180,90"), "ffprobe saw: {text}");
    assert!(text.contains("aac"), "ffprobe saw: {text}");
}
