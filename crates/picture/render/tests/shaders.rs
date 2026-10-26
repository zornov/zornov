// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Every WGSL module this crate builds (and the `fx` and `colour` shaders
//! it runs) must validate with naga, so a shader mistake fails the tests
//! on machines without a GPU too.

use naga::valid::{Capabilities, ValidationFlags, Validator};

fn validate(name: &str, source: &str) {
    let module = naga::front::wgsl::parse_str(source).unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(source)));
    Validator::new(ValidationFlags::all(), Capabilities::all()).validate(&module).unwrap_or_else(|e| panic!("{name}: {e:?}"));
}

#[test]
fn render_shaders_validate() {
    for (name, src) in studio_render::gpu::shader_sources() {
        validate(name, &src);
    }
    validate("grade", &studio_colour::gpu::grade_shader_source());
}

#[test]
fn fx_shaders_validate() {
    for e in studio_fx::effects::all() {
        if let Some(w) = e.wgsl {
            validate(e.id, w);
        }
    }
    for t in studio_fx::transitions::all() {
        if let Some(w) = t.wgsl {
            validate(t.id, w);
        }
    }
}
