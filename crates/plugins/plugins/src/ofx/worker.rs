// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The OFX worker: a separate process per bundle, so a plugin that
//! crashes, hangs or leaks takes only its worker down. Messages on stdin
//! and stdout are `[u32 LE JSON length][JSON][u64 LE payload length]
//! [payload]`; frames are top-down premultiplied RGBA `f32`.
//!
//! On start the worker loads the bundle and sends
//! `{"ok": true, "plugins": [...], "skipped": [[id, why]...]}`. Then it
//! answers `{"op": "render", "index", "width", "height", "frame", "fps",
//! "progress", "params": {...}, "inputs": n}` (payload: n frames) with
//! `{"ok": true}` and the frame, or `{"ok": false, "error"}`; and
//! `{"op": "quit"}` by exiting.

use std::io::{Read, Write};
use std::path::Path;

use serde_json::{Value, json};

pub fn write_msg(w: &mut impl Write, head: &Value, payload: &[u8]) -> std::io::Result<()> {
    let h = head.to_string();
    w.write_all(&(h.len() as u32).to_le_bytes())?;
    w.write_all(h.as_bytes())?;
    w.write_all(&(payload.len() as u64).to_le_bytes())?;
    w.write_all(payload)?;
    w.flush()
}

pub fn read_msg(r: &mut impl Read) -> std::io::Result<(Value, Vec<u8>)> {
    let mut n4 = [0u8; 4];
    r.read_exact(&mut n4)?;
    let n = u32::from_le_bytes(n4) as usize;
    if n > 64 << 20 {
        return Err(std::io::Error::other("message too long"));
    }
    let mut h = vec![0u8; n];
    r.read_exact(&mut h)?;
    let mut n8 = [0u8; 8];
    r.read_exact(&mut n8)?;
    let m = u64::from_le_bytes(n8) as usize;
    if m > 4 << 30 {
        return Err(std::io::Error::other("payload too long"));
    }
    let mut p = vec![0u8; m];
    r.read_exact(&mut p)?;
    let v = serde_json::from_slice(&h).map_err(std::io::Error::other)?;
    Ok((v, p))
}

pub fn frames_to_bytes(frames: &[&[[f32; 4]]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(frames.iter().map(|f| f.len() * 16).sum());
    for f in frames {
        for p in f.iter() {
            for c in p {
                out.extend_from_slice(&c.to_le_bytes());
            }
        }
    }
    out
}

pub fn bytes_to_frame(b: &[u8]) -> Vec<[f32; 4]> {
    b.chunks_exact(16).map(|c| std::array::from_fn(|i| f32::from_le_bytes(c[i * 4..i * 4 + 4].try_into().unwrap_or([0; 4])))).collect()
}

/// The worker's main loop. Never returns.
pub fn main(bundle: &Path) -> ! {
    let mut stdin = std::io::stdin().lock();
    #[allow(unused_mut)]
    let mut stdout = std::io::stdout().lock();
    // Plugins that print would corrupt the channel: their stdout goes to
    // stderr from here on.
    #[cfg(unix)]
    let mut stdout = redirect_stdout(stdout);
    // SAFETY: this process exists to run the bundle's code.
    let mut b = match unsafe { super::bundle::Bundle::load(bundle) } {
        Ok(b) => b,
        Err(e) => {
            let _ = write_msg(&mut stdout, &json!({"ok": false, "error": e}), &[]);
            std::process::exit(2);
        }
    };
    let infos: Vec<_> = b.plugins.iter().map(|p| p.info.clone()).collect();
    if write_msg(&mut stdout, &json!({"ok": true, "plugins": infos, "skipped": b.skipped}), &[]).is_err() {
        std::process::exit(3);
    }
    loop {
        let Ok((req, payload)) = read_msg(&mut stdin) else { std::process::exit(0) };
        match req["op"].as_str() {
            Some("render") => {
                let (w, h) = (req["width"].as_u64().unwrap_or(0) as usize, req["height"].as_u64().unwrap_or(0) as usize);
                let n = req["inputs"].as_u64().unwrap_or(0) as usize;
                let frame = w * h * 16;
                let res = (|| -> Result<Vec<[f32; 4]>, String> {
                    if w == 0 || h == 0 || payload.len() != n * frame {
                        return Err("bad render request".into());
                    }
                    let inputs: Vec<Vec<[f32; 4]>> = (0..n).map(|i| bytes_to_frame(&payload[i * frame..(i + 1) * frame])).collect();
                    let index = req["index"].as_u64().unwrap_or(0) as usize;
                    let p = b.plugins.iter_mut().find(|p| p.info.index == index).ok_or("no such plugin")?;
                    let params = req["params"].as_object().cloned().unwrap_or_default();
                    p.render(&inputs, w, h, req["frame"].as_f64().unwrap_or(0.0), req["fps"].as_f64().unwrap_or(30.0), req["progress"].as_f64().unwrap_or(0.0), &params)
                })();
                let ok = match res {
                    Ok(px) => write_msg(&mut stdout, &json!({"ok": true}), &frames_to_bytes(&[&px])),
                    Err(e) => write_msg(&mut stdout, &json!({"ok": false, "error": e}), &[]),
                };
                if ok.is_err() {
                    std::process::exit(0);
                }
            }
            Some("messages") => {
                let m = super::host::MESSAGES.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let _ = write_msg(&mut stdout, &json!({"ok": true, "messages": m}), &[]);
            }
            _ => {
                drop(b);
                std::process::exit(0);
            }
        }
    }
}

/// Keeps the real stdout for messages and points file descriptor 1 at
/// stderr, so a plugin's `printf` cannot break the channel.
#[cfg(unix)]
fn redirect_stdout(lock: std::io::StdoutLock<'static>) -> std::fs::File {
    use std::os::fd::FromRawFd;
    drop(lock);
    unsafe extern "C" {
        fn dup(fd: i32) -> i32;
        fn dup2(a: i32, b: i32) -> i32;
    }
    // SAFETY: plain descriptor duplication on this process's own fds.
    unsafe {
        let keep = dup(1);
        dup2(2, 1);
        std::fs::File::from_raw_fd(keep)
    }
}
