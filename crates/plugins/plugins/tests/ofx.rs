// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The OFX host against a real OFX bundle (built from `tests/ofx_fixture`,
//! a plugin written to the C ABI), loaded in a worker process and used
//! through the `fx` catalogue; and, when installed, a commercial bundle.

use std::path::{Path, PathBuf};
use std::process::Command;

use studio_fx::effects::{self, Category, EffectContext};
use studio_fx::image::Image;
use studio_fx::params::ParamValue;
use studio_plugins::ofx::{self, Settings};

fn fixture_bundle(root: &Path) -> PathBuf {
    let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("ofx-fixture");
    let st = Command::new(env!("CARGO"))
        .args(["build", "--release", "--quiet", "--manifest-path"])
        .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/ofx_fixture/Cargo.toml"))
        .env("CARGO_TARGET_DIR", &target)
        .status()
        .unwrap();
    assert!(st.success(), "fixture builds");
    let lib = target.join("release").join(if cfg!(target_os = "macos") { "libofx_fixture.dylib" } else { "libofx_fixture.so" });
    let bundle = root.join("StudioTest.ofx.bundle");
    let dir = bundle.join("Contents").join(if cfg!(target_os = "macos") { "MacOS" } else { "Linux-x86-64" });
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(&lib, dir.join("StudioTest.ofx")).unwrap();
    bundle
}

#[test]
fn ofx_bundle_describes_renders_and_registers() {
    // SAFETY (test): no other thread reads the environment here.
    unsafe { std::env::set_var("STUDIO_OFX_WORKER", env!("CARGO_BIN_EXE_studio-ofx-worker")) };
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("ofx-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let bundles = root.join("bundles");
    std::fs::create_dir_all(&bundles).unwrap();
    fixture_bundle(&bundles);

    // Off until turned on.
    let plugdir = root.join("Plugins");
    assert!(ofx::load(&plugdir).is_empty());

    let settings = Settings { enabled: true, paths: vec![bundles.clone()], only_paths: true, linear: vec!["com.studio.test.Invert".into(), "com.studio.test.Fill".into()], ..Default::default() };
    settings.save(&plugdir).unwrap();
    let loaded = ofx::load(&plugdir);
    assert_eq!(loaded.len(), 2, "{loaded:?} {:?} {:?}", ofx::failed(), ofx::messages());
    let inv = loaded.iter().find(|l| l.id == "com.studio.test.Invert").unwrap();
    assert_eq!((inv.kind, inv.label.as_str(), inv.params), ("effect", "Test invert", 3));

    let def = effects::find("ofx:com.studio.test.Invert").expect("registered");
    let ids: Vec<&str> = def.params.iter().map(|p| p.id).collect();
    assert_eq!(ids, ["amount", "channel", "tint"]);
    let img = Image::from_fn(16, 9, |x, y| [x as f32 / 16.0 * 0.8, y as f32 / 9.0 * 0.8, 0.25, 0.8]);
    let ctx = EffectContext { time: 1.0, frame_rate: 24.0, seed: 0, history: &[], aux: None };
    let out = def.apply(&ctx, &img, &def.defaults());
    for (o, i) in out.pixels.iter().zip(&img.pixels) {
        for c in 0..3 {
            assert!((o[c] - (i[3] - i[c])).abs() < 1e-5, "{o:?} {i:?}");
        }
        assert_eq!(o[3], i[3]);
    }
    // Parameters reach the plugin: half strength, red only, a tint.
    let p = def.defaults().with("amount", ParamValue::Float(0.5)).with("channel", ParamValue::Choice(1)).with("tint", ParamValue::Colour([0.5, 1.0, 1.0, 1.0]));
    let out = def.apply(&ctx, &img, &p);
    let (o, i) = (out.pixels[20], img.pixels[20]);
    assert!((o[0] - (i[0] + (i[3] - 2.0 * i[0]) * 0.5) * 0.5).abs() < 1e-5, "{o:?}");
    assert_eq!((o[1], o[2]), (0.0, 0.0));

    let fill = effects::find("ofx:com.studio.test.Fill").unwrap();
    assert_eq!(fill.category, Category::Generator);
    let out = fill.apply(&ctx, &Image::new(8, 8), &fill.defaults());
    assert_eq!(out.pixels[0], [0.0, 0.5, 1.0, 1.0]);

    // Turning OFX off takes the plugins out again.
    Settings { enabled: false, ..settings }.save(&plugdir).unwrap();
    ofx::load(&plugdir);
    assert!(effects::find("ofx:com.studio.test.Invert").is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn installed_commercial_bundles_load_in_a_worker() {
    // FxFactory (and any other bundle installed on this Mac): loading and
    // describing must not crash Studio, whatever the bundle does.
    unsafe { std::env::set_var("STUDIO_OFX_WORKER", env!("CARGO_BIN_EXE_studio-ofx-worker")) };
    let found = ofx::find_bundles(&[], false);
    if found.is_empty() {
        eprintln!("no OFX bundles installed; skipped");
        return;
    }
    for b in found {
        let bundle = ofx::Bundle::open(&b);
        eprintln!("{}: {} plugins, {} skipped, error {:?}", b.display(), bundle.plugins.len(), bundle.skipped.len(), bundle.error);
        for p in bundle.plugins.iter().take(5) {
            eprintln!("  {} ({}) {} params, {}", p.label, p.id, p.params.len(), p.context);
        }
    }
}
