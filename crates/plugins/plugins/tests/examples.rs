// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The two example plugins, installed into a scratch folder and run
//! through the same lookups the renderer, inspector and agent use.

use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::json;
use studio_fx::effects::{self, EffectContext};
use studio_fx::image::Image;
use studio_fx::params::ParamValue;
use studio_fx::transitions::{self, TransitionContext};
use studio_plugins::{Host, Imported, catalogue};

/// The fx catalogue is global; the tests take turns.
static SERIAL: Mutex<()> = Mutex::new(());

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("studio-plugins-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn card() -> Image {
    Image::from_fn(48, 27, |x, y| [x as f32 / 48.0, y as f32 / 27.0, 0.3, 1.0])
}

#[test]
fn duotone_installs_runs_and_turns_off() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("duotone");
    let host = Host::at(&dir);
    let avail = catalogue::available(&host);
    assert!(catalogue::search(&avail, "poster two colour look").first().is_some_and(|e| e.id == "duotone"));
    let p = catalogue::install(&host, "duotone").unwrap();
    assert!(p.enabled && p.warnings.is_empty(), "{:?}", p.warnings);
    assert!(dir.join("duotone/plugin.wasm").is_file());

    let def = effects::find("duotone:duotone").expect("registered with fx");
    assert!(def.is_external());
    let wgsl = def.wgsl.expect("shader kept");
    assert!(wgsl.contains("struct Uniforms") && wgsl.contains("fn effect"), "prelude + body");
    let img = card();
    let out = def.apply(&EffectContext::still(), &img, &def.defaults());
    assert_ne!(out, img);
    // The darkest pixel takes the shadow colour (0.05, 0.02, 0.25).
    let dark = out.pixels[0];
    assert!(dark[2] > dark[0] && dark[2] > dark[1], "{dark:?}");
    // Mix 0 leaves the picture alone.
    let none = def.apply(&EffectContext::still(), &img, &def.defaults().with("mix", ParamValue::Float(0.0)));
    for (a, b) in none.pixels.iter().zip(&img.pixels) {
        for c in 0..4 {
            assert!((a[c] - b[c]).abs() < 1e-5);
        }
    }

    host.set_enabled("duotone", false).unwrap();
    assert!(effects::find("duotone:duotone").is_none());
    // Turning off is remembered by a fresh host on the same folder.
    let again = Host::at(&dir);
    assert!(!again.get("duotone").unwrap().enabled);
    again.set_enabled("duotone", true).unwrap();
    assert!(effects::find("duotone:duotone").is_some());
    again.uninstall("duotone").unwrap();
    assert!(effects::find("duotone:duotone").is_none());
    assert!(!dir.join("duotone").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn toolkit_generator_transition_importers_exporter_and_panel() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("toolkit");
    let host = Host::at(&dir);
    let p = catalogue::install(&host, "toolkit").unwrap();
    assert_eq!(p.manifest.summary(), "1 generator, 1 transition, 2 importers, 1 exporter, 1 panel");

    // Generator: animated, differs over time, is a Generator in fx.
    let plasma = effects::find("toolkit:plasma").unwrap();
    assert_eq!(plasma.category, effects::Category::Generator);
    assert!(plasma.animated);
    let blank = Image::new(32, 18);
    let ctx = |t| EffectContext { time: t, frame_rate: 30.0, seed: 1, history: &[], aux: None };
    let a = plasma.apply(&ctx(0.0), &blank, &plasma.defaults());
    let b = plasma.apply(&ctx(1.0), &blank, &plasma.defaults());
    assert!(a.pixels.iter().all(|p| p[3] > 0.99), "opaque");
    assert_ne!(a, b);

    // Transition: exact at the ends (fx copies), the centre opens first.
    let iris = transitions::find("toolkit:iris").unwrap();
    assert!(iris.wgsl.is_some_and(|w| w.contains("fn transition")));
    let (ia, ib) = (Image::filled(40, 40, [1.0, 0.0, 0.0, 1.0]), Image::filled(40, 40, [0.0, 0.0, 1.0, 1.0]));
    let mid = iris.apply(&TransitionContext::default(), &ia, &ib, 0.3, &iris.defaults());
    assert!(mid.get(20, 20)[2] > 0.9, "centre shows B");
    assert!(mid.get(0, 0)[0] > 0.9, "corner still A");
    assert_eq!(iris.apply(&TransitionContext::default(), &ia, &ib, 1.0, &iris.defaults()), ib);

    // Importers by extension.
    let ppm = dir.join("tiny.ppm");
    std::fs::write(&ppm, b"P3 2 1 255  255 0 0  0 255 0").unwrap();
    assert_eq!(host.importers_for("ppm").len(), 1);
    let Imported::Image { width, height, rgba8 } = host.import(&ppm, None).unwrap() else { panic!("picture") };
    assert_eq!((width, height, &rgba8[..]), (2, 1, &[255, 0, 0, 255, 0, 255, 0, 255][..]));
    let mk = dir.join("cues.markers");
    std::fs::write(&mk, "0.5 Start\n1:30 Drop\n").unwrap();
    let Imported::Ops(ops) = host.import(&mk, None).unwrap() else { panic!("ops") };
    assert_eq!(ops, json!([{"op": "marker", "at": 0.5, "note": "Start"}, {"op": "marker", "at": 90, "note": "Drop"}]));
    assert!(host.import(&dir.join("x.unknown"), None).unwrap_err().contains("no plugin imports .unknown"));

    // Exporter and panel get the project as the agent sees it.
    let project = json!({"clips": [{"id": 1, "name": "Opening", "kind": "video", "lane": 0, "start": 0.0, "dur": 4.0, "effects": ["toolkit:plasma"]}, {"id": 2, "name": "Title", "kind": "text", "lane": 1, "start": 1.0, "dur": 2.0, "effects": []}]});
    let csv = String::from_utf8(host.export("toolkit", "clip_csv", &project).unwrap()).unwrap();
    assert!(csv.lines().nth(1).unwrap().starts_with("1,Opening,video,0,0.000,4.000,4.000,toolkit:plasma"), "{csv}");
    let view = host.panel("toolkit", "shot_list", &json!({}), &project).unwrap();
    assert!(view["widgets"][1]["text"].as_str().unwrap().starts_with("1 shots"));
    let pressed = host.panel("toolkit", "shot_list", &json!({"button": "mark", "values": {"min_len": 0}}), &project).unwrap();
    assert_eq!(pressed["ops"][0]["op"], "marker");
    assert_eq!(pressed["message"], "Marked 1 shots");

    host.uninstall("toolkit").unwrap();
    assert!(transitions::find("toolkit:iris").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn broken_plugins_are_refused_and_listed_errors_do_not_stop_others() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = scratch("broken");
    let host = Host::at(&dir);
    assert!(host.install_bytes(b"not wasm", "test").unwrap_err().contains("not a WebAssembly plugin"));
    // A module copied in by hand that does not start stays listed with its
    // error so it can be removed.
    std::fs::create_dir_all(dir.join("junk")).unwrap();
    std::fs::write(dir.join("junk/plugin.wasm"), b"\0asm\x01\0\0\0").unwrap();
    let list = host.reload();
    let junk = list.iter().find(|p| p.id() == "junk").unwrap();
    assert!(junk.error.is_some() && !junk.enabled);
    host.uninstall("junk").unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bundled_modules_match_their_sources() {
    // The bundled modules are built from plugins/examples; their manifests
    // must say what the sources say.
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for (name, src) in [("duotone", "plugins/examples/duotone/src/lib.rs"), ("toolkit", "plugins/examples/toolkit/src/lib.rs")] {
        let wasm = std::fs::read(root.join(format!("plugins/dist/{name}.wasm"))).unwrap();
        let (m, _) = studio_plugins::store::inspect(&wasm, Default::default()).unwrap();
        let text = std::fs::read_to_string(root.join(src)).unwrap();
        for (_, id, item) in m.items() {
            assert!(text.contains(&format!("\"{id}\"")) && text.contains(&format!("\"{item}\"")), "{name}: {id} / {item} not in the source; rebuild plugins/dist (see docs/plugins.md)");
        }
        for it in m.effects.iter().chain(&m.generators).chain(&m.transitions) {
            for p in &it.params {
                assert!(text.contains(&format!("\"{}\"", p.id)), "{name}: parameter {} not in the source", p.id);
            }
        }
    }
}
