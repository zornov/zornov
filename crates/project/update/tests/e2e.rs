// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The whole path against a real local HTTP server: feed, signature,
//! download, unpack, restart-time swap.

use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use studio_update::apply::{self, Applied, Target};
use studio_update::config::Config;
use studio_update::feed::{self, Entry, Feed};
use studio_update::state::{self, Status};

fn archive(files: &[(&str, &str)]) -> Vec<u8> {
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut tar = tar::Builder::new(gz);
    let manifest: String = files.iter().map(|(n, _)| format!("{n}\n")).collect();
    for (n, d) in std::iter::once((apply::MANIFEST, manifest.as_str())).chain(files.iter().copied()) {
        let mut h = tar::Header::new_gnu();
        h.set_size(d.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        tar.append_data(&mut h, n, d.as_bytes()).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

/// Serves `routes` (path -> body) until the process ends.
fn serve(routes: BTreeMap<String, Vec<u8>>) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let routes = routes.clone();
            std::thread::spawn(move || {
                let mut s = s;
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_owned();
                match routes.get(&path) {
                    Some(b) => {
                        let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", b.len());
                        let _ = s.write_all(b);
                    }
                    None => {
                        let _ = write!(s, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    }
                }
            });
        }
    });
    format!("http://{addr}")
}

#[test]
fn a_signed_update_downloads_and_installs_on_restart() {
    let home = std::env::temp_dir().join(format!("studio-update-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    // Everything Studio writes goes under this home.
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("USERPROFILE", &home);
        std::env::set_var("LOCALAPPDATA", home.join("local"));
        std::env::set_var("XDG_STATE_HOME", home.join("state"));
    }
    let kp = Ed25519KeyPair::from_pkcs8(Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap().as_ref()).unwrap();
    let public = studio_update::hex(kp.public_key().as_ref());

    let new_version = "99.0.0";
    let arc = archive(&[("studio", "NEW BINARY"), ("content/new.txt", "fresh")]);
    let sha256 = feed::sha256_hex(&arc);
    let platform = feed::platform();
    let sig = studio_update::hex(kp.sign(&feed::message(&platform, new_version, &sha256, arc.len() as u64)).as_ref());
    let entry = Entry { url: "studio.update.tar.gz".into(), size: arc.len() as u64, sha256, signature: sig };
    let feed_json = serde_json::to_vec(&Feed { schema: 1, version: new_version.into(), notes: "big news".into(), date: String::new(), platforms: BTreeMap::from([(platform.clone(), entry)]) }).unwrap();
    let base = serve(BTreeMap::from([("/feed.json".to_owned(), feed_json), ("/studio.update.tar.gz".to_owned(), arc)]));

    let install = home.join("Zornov-install");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::write(install.join("studio"), "OLD BINARY").unwrap();
    std::fs::write(install.join("old-only.txt"), "bye").unwrap();
    let target = Target::Folder(install.clone());

    let cfg = Config { feed_url: format!("{base}/feed.json"), public_key: public.clone(), crash_endpoint: String::new() };
    // A check alone downloads nothing.
    assert!(matches!(state::check(&cfg, "0.1.0", Some(&target), &state::net_fetch), Ok(state::Found::Offer(ref o)) if o.version == new_version));
    assert!(apply::pending().is_none());
    let status = state::run(&cfg, "0.1.0", Some(&target), &state::net_fetch, &state::net_download);
    assert_eq!(status, Status::Ready(new_version.into()), "{status:?}");
    // Nothing is touched until the restart.
    assert_eq!(std::fs::read_to_string(install.join("studio")).unwrap(), "OLD BINARY");

    // A wrong key refuses to install what is waiting.
    let other = Ed25519KeyPair::from_pkcs8(Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap().as_ref()).unwrap();
    let wrong = studio_update::hex(other.public_key().as_ref());
    assert!(apply::pending().is_some());
    assert!(matches!(apply::apply_pending(&wrong, "0.1.0", Some(&target)), Applied::Failed(_)));
    assert_eq!(std::fs::read_to_string(install.join("studio")).unwrap(), "OLD BINARY");
    assert!(apply::pending().is_none(), "a refused update is not retried at every start");

    // The right key installs it.
    let again = state::run(&cfg, "0.1.0", Some(&target), &state::net_fetch, &state::net_download);
    assert_eq!(again, Status::Ready(new_version.into()));
    assert_eq!(apply::apply_pending(&public, "0.1.0", Some(&target)), Applied::Installed(new_version.into()));
    assert_eq!(std::fs::read_to_string(install.join("studio")).unwrap(), "NEW BINARY");
    assert_eq!(std::fs::read_to_string(install.join("content/new.txt")).unwrap(), "fresh");
    assert!(!install.join("old-only.txt").exists());
    // Already current: nothing more to do, and the old copy is tidied up.
    assert_eq!(state::run(&cfg, new_version, Some(&target), &state::net_fetch, &state::net_download), Status::UpToDate);
    assert_eq!(apply::apply_pending(&public, new_version, Some(&target)), Applied::Nothing);
    assert!(!install.join("studio.studio-old").exists());
    let _ = std::fs::remove_dir_all(&home);
}
