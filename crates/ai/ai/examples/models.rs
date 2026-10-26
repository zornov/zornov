// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Manage the models folder from the command line:
//!
//! ```text
//! cargo run -p studio-ai --example models -- list
//! cargo run -p studio-ai --example models -- download whisper-tiny   # network!
//! cargo run -p studio-ai --example models -- verify whisper-tiny
//! ```

use studio_ai::registry::human_size;
use studio_ai::{Jobs, ModelId, ModelStore};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let store = ModelStore::default();
    let id = |i: usize| {
        args.get(i).and_then(|k| ModelId::from_key(k)).unwrap_or_else(|| {
            eprintln!("model ids: {}", ModelId::ALL.map(|m| m.key()).join(", "));
            std::process::exit(2)
        })
    };
    match args.first().map(String::as_str) {
        Some("list") | None => {
            println!("models folder: {}", store.root().display());
            for m in ModelId::ALL {
                let s = m.spec();
                println!("{:<26} {:>8}  {:<18} {:?}", s.dir, human_size(s.download_size()), s.licence, store.status(m));
            }
        }
        Some("download") => {
            let m = id(1);
            let jobs = Jobs::new(1);
            let h = jobs.spawn(format!("download {}", m.key()), move |ctx| store.download(m, ctx));
            while !h.is_finished() {
                eprint!("\r{:5.1}% {}   ", h.progress() * 100.0, h.message());
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            eprintln!();
            match h.wait() {
                Ok(()) => println!("installed {}", m.key()),
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1)
                }
            }
        }
        Some("verify") => {
            let m = id(1);
            let bad = store.verify(m, &studio_ai::JobCtx::detached()).unwrap();
            if bad.is_empty() {
                println!("{}: all files match their SHA-256", m.key());
            } else {
                println!("{}: bad or missing: {}", m.key(), bad.join(", "));
                std::process::exit(1)
            }
        }
        Some(other) => eprintln!("unknown command {other}; use list | download <id> | verify <id>"),
    }
}
