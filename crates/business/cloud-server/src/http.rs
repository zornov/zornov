// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A small HTTP/1.1 server on the standard library: one thread per
//! connection, one request per connection, bodies by `Content-Length`.
//! Enough for a reference server; put it behind a TLS proxy (Caddy, nginx)
//! to serve other computers.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub struct Request {
    pub method: String,
    /// Without the query string, percent-decoded per segment by the router.
    pub path: String,
    pub query: Vec<(String, String)>,
    /// Header names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    pub fn query(&self, name: &str) -> Option<&str> {
        self.query.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

pub struct Response {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json(status: u16, v: &serde_json::Value) -> Response {
        Response { status, content_type: "application/json".into(), body: v.to_string().into_bytes() }
    }

    pub fn error(status: u16, msg: impl Into<String>) -> Response {
        Response::json(status, &serde_json::json!({"error": msg.into()}))
    }

    pub fn html(status: u16, body: String) -> Response {
        Response { status, content_type: "text/html; charset=utf-8".into(), body: body.into_bytes() }
    }

    pub fn bytes(content_type: &str, body: Vec<u8>) -> Response {
        Response { status: 200, content_type: content_type.into(), body }
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        413 => "Payload Too Large",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

/// Reads one request (`None` for a connection that closed or sent nonsense).
fn read(stream: &TcpStream, max_body: usize) -> Result<Option<Request>, Response> {
    let mut r = BufReader::new(stream);
    let mut line = String::new();
    if r.read_line(&mut line).unwrap_or(0) == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else { return Ok(None) };
    let mut headers = Vec::new();
    let mut head = line.len();
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).unwrap_or(0) == 0 {
            return Ok(None);
        }
        head += h.len();
        if head > 64 << 10 {
            return Err(Response::error(413, "the request headers are too long"));
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_owned()));
        }
    }
    let len: usize = headers.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    if len > max_body {
        return Err(Response::error(413, format!("the upload is larger than {} MB", max_body >> 20)));
    }
    let mut body = vec![0; len];
    r.read_exact(&mut body).map_err(|_| Response::error(400, "the body ended early"))?;
    let (path, q) = target.split_once('?').unwrap_or((target, ""));
    Ok(Some(Request { method: method.to_owned(), path: path.to_owned(), query: studio_commerce::stripe::form_decode(q), headers, body }))
}

fn write(mut stream: &TcpStream, resp: &Response) {
    let head = format!("HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\r\n", resp.status, reason(resp.status), resp.content_type, resp.body.len());
    let _ = stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(&resp.body)).and_then(|_| stream.flush());
}

pub type Handler = Arc<dyn Fn(Request) -> Response + Send + Sync>;

/// Serves until `stop` is set (checked between connections).
pub fn serve(listener: TcpListener, handler: Handler, stop: Arc<AtomicBool>, max_body: usize) {
    for s in listener.incoming() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let Ok(s) = s else { continue };
        let handler = handler.clone();
        std::thread::spawn(move || {
            let _ = s.set_read_timeout(Some(Duration::from_secs(120)));
            match read(&s, max_body) {
                Ok(Some(req)) => write(&s, &handler(req)),
                Ok(None) => {}
                Err(resp) => write(&s, &resp),
            }
        });
    }
}
