// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A plugin written the way the AI writes one: a pixel function and its
//! parameters, built with the local Rust toolchain, test-run, installed.

use serde_json::json;
use studio_fx::effects::{self, EffectContext};
use studio_fx::image::Image;
use studio_plugins::Host;
use studio_plugins::author::{self, Spec};

#[test]
fn pixel_function_becomes_an_installed_effect() {
    if let Err(e) = author::toolchain() {
        eprintln!("skipped: {e}");
        return;
    }
    let root = std::env::temp_dir().join(format!("studio-author-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let spec = Spec {
        id: "warm-vignette".into(),
        name: "Warm vignette".into(),
        description: "Darkens the corners and warms the middle.".into(),
        pixel: Some(
            "    let (dx, dy) = (x / w - 0.5, y / h - 0.5);\n    let r = (dx * dx + dy * dy).sqrt() * 1.4;\n    let k = 1.0 - p.float(\"amount\") * r.min(1.0).powf(2.0);\n    [px[0] * k * 1.05, px[1] * k, px[2] * k * 0.9, px[3]]".into(),
        ),
        params: vec![json!({"id": "amount", "label": "Amount", "kind": "float", "min": 0.0, "max": 1.0, "default": 0.7})],
        ..Default::default()
    };
    // A compile error comes back worded for the AI to fix.
    let bad = Spec { pixel: Some("    px + 1".into()), ..spec.clone() };
    let err = author::build(&bad, &root).unwrap_err();
    assert!(err.contains("did not compile") && err.contains("error"), "{err}");

    let built = author::build(&spec, &root).unwrap();
    assert!(built.dir.join("src/lib.rs").is_file(), "the source is kept");
    assert!(built.tested[0].starts_with("Warm vignette: ran"), "{:?}", built.tested);
    let host = Host::at(root.join("installed"));
    host.install_bytes(&built.wasm, "made in Zornov").unwrap();
    let def = effects::find("warm-vignette:effect").unwrap();
    let img = Image::filled(40, 20, [0.5, 0.5, 0.5, 1.0]);
    let out = def.apply(&EffectContext::still(), &img, &def.defaults());
    assert!(out.get(0, 0)[1] < out.get(20, 10)[1], "corners darker than the middle");
    host.uninstall("warm-vignette").unwrap();
    let _ = std::fs::remove_dir_all(&root);
}
