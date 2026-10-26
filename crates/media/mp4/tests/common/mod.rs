// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared helpers for the integration tests: test-data paths, a tiny JSON
//! reader for `index.json`, and a deterministic PRNG.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;

pub fn media_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/media")
}

#[derive(Debug, Clone)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(BTreeMap<String, Json>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.get(key),
            _ => None,
        }
    }

    pub fn num(&self, key: &str) -> Option<f64> {
        match self.get(key)? {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }

    pub fn str(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn parse(text: &str) -> Json {
        let mut p = Parser { s: text.as_bytes(), i: 0 };
        p.value()
    }
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) {
        self.ws();
        assert_eq!(self.s[self.i], c, "json: expected '{}'", c as char);
        self.i += 1;
    }

    fn value(&mut self) -> Json {
        self.ws();
        match self.s[self.i] {
            b'{' => {
                self.i += 1;
                let mut m = BTreeMap::new();
                loop {
                    self.ws();
                    if self.s[self.i] == b'}' {
                        self.i += 1;
                        return Json::Obj(m);
                    }
                    let Json::Str(k) = self.value() else { panic!("json: key") };
                    self.eat(b':');
                    m.insert(k, self.value());
                    self.ws();
                    if self.s[self.i] == b',' {
                        self.i += 1;
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut v = Vec::new();
                loop {
                    self.ws();
                    if self.s[self.i] == b']' {
                        self.i += 1;
                        return Json::Arr(v);
                    }
                    v.push(self.value());
                    self.ws();
                    if self.s[self.i] == b',' {
                        self.i += 1;
                    }
                }
            }
            b'"' => {
                self.i += 1;
                let mut out = String::new();
                while self.s[self.i] != b'"' {
                    if self.s[self.i] == b'\\' {
                        self.i += 1;
                    }
                    out.push(self.s[self.i] as char);
                    self.i += 1;
                }
                self.i += 1;
                Json::Str(out)
            }
            b't' => {
                self.i += 4;
                Json::Bool(true)
            }
            b'f' => {
                self.i += 5;
                Json::Bool(false)
            }
            b'n' => {
                self.i += 4;
                Json::Null
            }
            _ => {
                let start = self.i;
                while self.i < self.s.len() && b"+-.eE0123456789".contains(&self.s[self.i]) {
                    self.i += 1;
                }
                let t = std::str::from_utf8(&self.s[start..self.i]).unwrap();
                Json::Num(t.parse().unwrap())
            }
        }
    }
}

/// Entries of `testdata/media/index.json`.
pub fn index() -> Vec<Json> {
    let text = std::fs::read_to_string(media_dir().join("index.json")).expect("index.json");
    match Json::parse(&text) {
        Json::Arr(v) => v,
        _ => panic!("index.json is not an array"),
    }
}

/// xorshift64* — tiny deterministic PRNG.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }

    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}
