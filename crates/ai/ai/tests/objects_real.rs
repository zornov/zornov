// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Real-model tests of the object map. They need the models in
//! `~/Movies/Studio/Models` (see `docs/models.md`) and skip with a message when
//! one is missing:
//!
//! `cargo test --release -p studio-ai --test objects_real -- --ignored --nocapture`
//!
//! The picture is `testdata/objects/street.png`, drawn for these tests: a
//! stop sign on a pole beside a cardboard box (the sign is the octagon
//! centred at (163, 120) with radius 62 in the 640 x 360 picture).

use studio_ai::features::detect::{merge, OpenVocabDetector};
use studio_ai::features::embed::{dot, PictureTextEmbedder};
use studio_ai::features::faces::Rect;
use studio_ai::features::ocr::TextReader;
use studio_ai::frame::{Frame, VecSource};
use studio_ai::objects::build::{build, BuildOptions, Models};
use studio_ai::{JobCtx, ModelId, ModelStore};
use std::path::PathBuf;

fn store_with(ids: &[ModelId]) -> Option<ModelStore> {
    let s = ModelStore::default();
    for &id in ids {
        if !s.is_installed(id) {
            eprintln!("{} not installed in {}; skipping", id.spec().name, s.root().display());
            return None;
        }
    }
    Some(s)
}

fn street() -> Frame {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/objects/street.png");
    let img = studio_media::png::decode(&std::fs::read(path).unwrap()).unwrap();
    Frame::new(img.width as usize, img.height as usize, img.pixels).unwrap()
}

/// The sign's box in the picture.
fn sign_box() -> Rect {
    let r = 62.0 * 22.5f32.to_radians().cos();
    Rect { x: 163.0 - r, y: 120.0 - r, w: 2.0 * r, h: 2.0 * r }
}

#[test]
#[ignore]
fn the_detector_finds_a_sign_and_a_box_by_name() {
    let Some(store) = store_with(&[ModelId::OwlV2Base]) else { return };
    let det = OpenVocabDetector::load(&store).unwrap();
    let names: Vec<String> = ["stop sign", "cardboard box", "dog", "car"].iter().map(|s| s.to_string()).collect();
    let t = std::time::Instant::now();
    let found = merge(det.detect(&street(), &names, 0.2).unwrap(), 0.5);
    println!("detected in {:?}: {:?}", t.elapsed(), found.iter().map(|d| (&names[d.name], d.score)).collect::<Vec<_>>());
    let sign = found.iter().find(|d| names[d.name] == "stop sign").expect("the stop sign");
    assert!(sign.bbox.iou(&sign_box()) > 0.6, "{:?} vs {:?}", sign.bbox, sign_box());
    assert!(found.iter().any(|d| names[d.name] == "cardboard box"), "the box");
    assert!(!found.iter().any(|d| names[d.name] == "dog"), "there is no dog");
}

#[test]
#[ignore]
fn the_text_reader_reads_the_sign() {
    let Some(store) = store_with(&[ModelId::PaddleOcrLatin]) else { return };
    let r = TextReader::load(&store).unwrap();
    let lines = r.read(&street()).unwrap();
    println!("{:?}", lines.iter().map(|l| (&l.text, l.score)).collect::<Vec<_>>());
    let stop = lines.iter().find(|l| l.text.to_uppercase().contains("STOP")).expect("STOP");
    let c = stop.bbox.centre();
    assert!(sign_box().x < c.0 && c.0 < sign_box().x + sign_box().w && sign_box().y < c.1 && c.1 < sign_box().y + sign_box().h);
}

#[test]
#[ignore]
fn a_sentence_is_nearest_the_picture_it_describes() {
    let Some(store) = store_with(&[ModelId::SiglipBase224]) else { return };
    let e = PictureTextEmbedder::load(&store).unwrap();
    let f = e.embed_frame(&street()).unwrap();
    let texts: Vec<String> = ["a photo of a red stop sign on a pole", "a photo of a bowl of soup", "a photo of a crowded beach"].iter().map(|s| s.to_string()).collect();
    let t = e.embed_texts(&texts).unwrap();
    let s: Vec<f32> = t.iter().map(|v| f.similarity(v)).collect();
    println!("{s:?}");
    assert!(s[0] > s[1] + 0.05 && s[0] > s[2] + 0.05, "{s:?}");
    // A crop of the sign is nearer the sign sentence than a crop of the grass.
    let sign = e.embed_crop(&street(), &sign_box()).unwrap();
    let grass = e.embed_crop(&street(), &Rect { x: 300.0, y: 300.0, w: 60.0, h: 40.0 }).unwrap();
    let q = &t[0];
    assert!(dot(&sign, q) > dot(&grass, q), "{} vs {}", dot(&sign, q), dot(&grass, q));
}

#[test]
#[ignore]
fn the_object_map_of_a_picture_names_the_sign_and_what_it_says() {
    let Some(store) = store_with(&[ModelId::OwlV2Base]) else { return };
    let mut models = Models::load(&store).unwrap();
    let mut src = VecSource { frames: vec![street()], fps: 30.0 };
    let o = BuildOptions { names: vec!["stop sign".into(), "cardboard box".into()], default_vocabulary: false, ..Default::default() };
    let t = std::time::Instant::now();
    let m = build(&mut src, 0..1, &mut models, &o, &JobCtx::detached()).unwrap();
    println!("built in {:?}: {:?}", t.elapsed(), m.objects.iter().map(|o| (o.id, &o.label, o.score, &o.texts, &o.colours, o.size)).collect::<Vec<_>>());
    let sign = m.named("stop sign").into_iter().next().expect("the sign");
    assert_eq!(m.objects.iter().filter(|o| o.label == "stop sign").count(), 1);
    assert!(sign.colours.iter().any(|c| c == "red"), "{:?}", sign.colours);
    if store.is_installed(ModelId::PaddleOcrLatin) {
        assert!(sign.texts.iter().any(|t| t.to_uppercase().contains("STOP")), "{:?}", sign.texts);
        assert!(m.texts.iter().any(|t| t.object == Some(sign.id)));
    }
    let bx = m.named("cardboard box").into_iter().next().expect("the box");
    // The box is to the right of the sign and the same height off the ground.
    assert!(bx.obs[0].bbox.cx() > sign.obs[0].bbox.cx());
    assert!(m.duration < 0.2 && m.keyframes.len() == 1);
}
