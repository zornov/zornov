// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Every shipped WGSL shader parses and validates.

use studio_fx::{effects, transitions};

fn validate(name: &str, src: &str) {
    let module = naga::front::wgsl::parse_str(src).unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(src)));
    let mut v = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all());
    v.validate(&module).unwrap_or_else(|e| panic!("{name}: {e:?}"));
    assert!(module.entry_points.iter().any(|e| e.name == "main"), "{name}: no main entry point");
}

#[test]
fn shaders_validate() {
    let mut n = 0;
    for e in effects::all() {
        if let Some(src) = e.wgsl {
            validate(e.id, src);
            n += 1;
        }
    }
    for t in transitions::all() {
        if let Some(src) = t.wgsl {
            validate(t.id, src);
            n += 1;
        }
    }
    assert!(n >= 30, "only {n} shaders");
}
