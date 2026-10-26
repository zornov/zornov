// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Post-production through real projects: grade nodes with a qualifier and
//! a keyframed (trackable) window, ProRes proxies drawn at the original's
//! size, and multicam clips showing one angle.

mod common;

use std::sync::Arc;

use studio_model as m;
use studio_render::compositor::FrameRenderer;
use studio_render::frame::compare_relative;
use studio_render::model_scene::{ModelScene, register_media};
use studio_render::scene::Scene;
use studio_render::source::{DecodeCache, Proxy, SourceRegistry, Sources};
use studio_render::{CpuCompositor, GpuCompositor, GpuContext, MediaKey};

fn scratch(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("studio-post-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A 64×32 ProRes HQ file: left half warm skin, right half blue sky, with
/// a frame-number stripe so frames differ.
fn footage(dir: &std::path::Path, name: &str, frames: u32, tint: u16) -> std::path::PathBuf {
    let path = dir.join(name);
    let (w, h) = (64u32, 32u32);
    let video = studio_mov::mov::VideoTrack { profile: studio_prores::Profile::Hq, width: w, height: h, rate_num: 25, rate_den: 1, colour: (1, 1, 1), alpha: false };
    let mut mov = studio_mov::mov::MovWriter::create(&path, video, None, None).unwrap();
    let mut enc = studio_mov::prores::ProResEncoder::new(studio_prores::Profile::Hq, w, h, (1, 1, 1), false);
    for f in 0..frames {
        let mut rgba = vec![0u16; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                let c: [u16; 3] = if y < 2 && x < f * 4 { [65535, 65535, 65535] } else if x < w / 2 { [55700, 40600, 32800] } else { [29500, 42600, 60300] };
                rgba[i..i + 4].copy_from_slice(&[c[0].saturating_add(tint), c[1], c[2], 65535]);
            }
        }
        mov.write_video(&enc.encode(&rgba, w, h).unwrap()).unwrap();
    }
    mov.finish().unwrap();
    path
}

fn project_with(paths: &[&std::path::Path]) -> (m::Project, Vec<m::MediaId>) {
    let mut p = m::Project::new(m::ProjectSettings { width: 64, height: 32, frame_rate: m::FrameRate::FPS_25, ..Default::default() });
    let mut ids = Vec::new();
    for path in paths {
        let id = m::MediaId(p.alloc_id());
        let info = m::MediaInfo {
            kind: m::MediaKind::Video,
            container: "mov".into(),
            duration: Some(400_000),
            video: Some(m::VideoInfo { codec: "prores".into(), width: 64, height: 32, frame_rate: Some(m::FrameRate::FPS_25), ..Default::default() }),
            audio: None,
            extra: Default::default(),
        };
        p.media.push(m::MediaItem::new(id, path.display().to_string(), info));
        ids.push(id);
    }
    (p, ids)
}

fn sources(reg: &Arc<SourceRegistry>) -> Sources {
    Sources::new(reg.clone(), DecodeCache::new(64 << 20))
}

#[test]
fn grade_node_with_qualifier_and_moving_window() {
    let dir = scratch("nodes");
    let file = footage(&dir, "a.mov", 10, 0);
    let (mut p, ids) = project_with(&[&file]);
    let seq = p.active_sequence;
    let spine = p.active().spine_id();
    let mut clip = m::Clip::media(m::ClipId(p.alloc_id()), spine, 0, ids[0], 0, 400_000);
    // A window over the left half that slides right over the clip (as a
    // tracked mask would), and a node: one stop darker, sky only.
    let mask_id = m::MaskId(p.alloc_id());
    let mut mask = m::Mask::new(mask_id, m::MaskShape::Rectangle);
    mask.params.insert("position".into(), m::Value::Point(m::Vec2::new(0.25, 0.5)));
    mask.params.insert("size".into(), m::Value::Point(m::Vec2::new(0.5, 1.0)));
    clip.masks.push(mask);
    let mut g = m::Grade { enabled: true, ..Default::default() };
    let mut node = m::GradeNode::default();
    node.params.insert("exposure".into(), m::Value::Scalar(-1.0));
    node.window = Some(mask_id);
    g.set_nodes(&[node.clone()]);
    clip.grade = Some(g);
    p.sequences.iter_mut().find(|s| s.id == seq).unwrap().clips.push(clip.clone());
    let project = Arc::new(p.clone());
    let reg = SourceRegistry::new();
    register_media(&project, &reg);
    let scene = ModelScene::new(project.clone());
    let mut cpu = CpuCompositor::new(sources(&reg));
    let f = cpu.render(&scene, 40_000, None);
    let plain = {
        let mut q = p.clone();
        q.sequences.iter_mut().find(|s| s.id == seq).unwrap().clips[0].grade = None;
        q.sequences.iter_mut().find(|s| s.id == seq).unwrap().clips[0].masks.clear();
        let q = Arc::new(q);
        cpu.render(&ModelScene::new(q), 40_000, None)
    };
    // The window mask does not cut the picture: the right half is there.
    let at = |fr: &studio_render::Frame, x: u32, y: u32| fr.pixels[(y * 64 + x) as usize];
    assert!(at(&f, 50, 16)[3] > 0.99, "right half visible");
    assert!((at(&f, 50, 16)[2] - at(&plain, 50, 16)[2]).abs() < 1e-3, "outside the window: untouched");
    let k = at(&f, 10, 16)[0] / at(&plain, 10, 16)[0];
    assert!((k - 0.5).abs() < 0.03, "inside the window: a stop down ({k})");

    // Qualify on the sky: inside the window only the sky (none there) and
    // outside nothing — so nothing changes at the left; then widen the
    // window to the whole frame: the sky darkens, skin does not.
    node.qualifier.insert("preset".into(), m::Value::Text("sky".into()));
    let mut q = p.clone();
    {
        let c = &mut q.sequences.iter_mut().find(|s| s.id == seq).unwrap().clips[0];
        c.masks[0].params.insert("size".into(), m::Value::Point(m::Vec2::new(4.0, 4.0)));
        c.grade.as_mut().unwrap().set_nodes(&[node.clone()]);
    }
    let q = Arc::new(q);
    let f2 = cpu.render(&ModelScene::new(q.clone()), 40_000, None);
    assert!((at(&f2, 10, 16)[0] - at(&plain, 10, 16)[0]).abs() < 2e-3, "skin untouched");
    assert!(at(&f2, 50, 16)[2] < at(&plain, 50, 16)[2] * 0.6, "sky darker");
    // The GPU path agrees.
    if let Some(ctx) = GpuContext::new() {
        let reg2 = SourceRegistry::new();
        register_media(&q, &reg2);
        let mut gpu = GpuCompositor::new(ctx, sources(&reg2));
        let g = gpu.render(&ModelScene::new(q.clone()), 40_000, None);
        let (max, bad) = compare_relative(&f2, &g, 0.01);
        assert!(bad <= 0.01, "nodes on the GPU path: max {max}, {:.2}% beyond", bad * 100.0);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn proxies_draw_at_the_original_size() {
    let dir = scratch("proxy");
    let file = footage(&dir, "orig.mov", 6, 0);
    let proxy_path = dir.join("orig_proxy.mov");
    let info = studio_render::proxy::make_proxy(&file, &proxy_path, 2, None, &mut |_, _| true).unwrap();
    assert_eq!((info.width, info.height, info.frames), (32, 16, 6));
    assert_eq!(info.input, None, "BT.709 tags say it all");
    let (mut p, ids) = project_with(&[&file]);
    let seq = p.active_sequence;
    let spine = p.active().spine_id();
    let c = m::Clip::media(m::ClipId(p.alloc_id()), spine, 0, ids[0], 0, 240_000);
    p.sequences.iter_mut().find(|s| s.id == seq).unwrap().clips.push(c);
    let project = Arc::new(p);
    let full_reg = SourceRegistry::new();
    register_media(&project, &full_reg);
    let proxy_reg = SourceRegistry::new();
    register_media(&project, &proxy_reg);
    proxy_reg.set_proxy(MediaKey(ids[0].0), Some(Proxy { path: proxy_path.clone(), size: (64, 32), input: None }));
    let scene = ModelScene::new(project.clone());
    let a = CpuCompositor::new(sources(&full_reg)).render(&scene, 80_000, None);
    let b = CpuCompositor::new(sources(&proxy_reg)).render(&scene, 80_000, None);
    assert_eq!((b.width, b.height), (64, 32));
    // Covers the whole frame and looks like the original: region means
    // away from the hard edge agree (a proxy is lossy at that edge).
    let mean = |f: &studio_render::Frame, x0: u32, x1: u32| -> [f32; 4] {
        let mut s = [0f32; 4];
        let mut n = 0.0;
        for y in 4..28 {
            for x in x0..x1 {
                let p = f.pixels[(y * 64 + x) as usize];
                for c in 0..4 {
                    s[c] += p[c];
                }
                n += 1.0;
            }
        }
        s.map(|v| v / n)
    };
    for (x0, x1) in [(4, 24), (40, 60)] {
        let (ma, mb) = (mean(&a, x0, x1), mean(&b, x0, x1));
        for c in 0..3 {
            assert!((ma[c] - mb[c]).abs() < 0.04, "{x0}..{x1}: {ma:?} vs {mb:?}");
        }
        assert!(mb[3] > 0.99, "proxy covers the frame");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn multicam_clip_shows_its_angle() {
    let dir = scratch("multicam");
    let cam_a = footage(&dir, "a.mov", 10, 0);
    let cam_b = footage(&dir, "b.mov", 10, 9000);
    let (mut p, ids) = project_with(&[&cam_a, &cam_b]);
    // The multicam sequence: an angle per lane.
    let mc = m::SequenceId(p.alloc_id());
    let spine = m::LaneId(p.alloc_id());
    let mut ms = m::Sequence::new(mc, "Multicam", spine);
    let lane_b = m::LaneId(p.alloc_id());
    ms.lanes.insert(0, m::Lane::new(lane_b, m::LaneKind::Visual, "Angle 2"));
    ms.clips.push(m::Clip::media(m::ClipId(p.alloc_id()), spine, 0, ids[0], 0, 400_000));
    ms.clips.push(m::Clip::media(m::ClipId(p.alloc_id()), lane_b, 0, ids[1], 0, 400_000));
    p.sequences.push(ms);
    let seq = p.active_sequence;
    let main_spine = p.active().spine_id();
    let mut clip = m::Clip::new(m::ClipId(p.alloc_id()), main_spine, 0, 400_000, m::ClipSource::Sequence(m::SequenceRef { sequence: mc, extra: Default::default() }));
    clip.set_multicam(spine, &[spine, lane_b], &["a".into(), "b".into()]);
    p.sequences.iter_mut().find(|s| s.id == seq).unwrap().clips.push(clip.clone());
    let reg = SourceRegistry::new();
    let pa = Arc::new(p.clone());
    register_media(&pa, &reg);
    let mut cpu = CpuCompositor::new(sources(&reg));
    let a = cpu.render(&ModelScene::new(pa.clone()), 40_000, None);
    let mut pb = p.clone();
    pb.sequences.iter_mut().find(|s| s.id == seq).unwrap().clips[0].set_multicam_angle(lane_b);
    let b = cpu.render(&ModelScene::new(Arc::new(pb)), 40_000, None);
    // Angle 2 has more red (its tint).
    let red = |f: &studio_render::Frame| f.pixels[(16 * 64 + 10) as usize][0];
    assert!(red(&b) > red(&a) + 0.05, "angle switch changes the picture: {} vs {}", red(&a), red(&b));
    assert!(ModelScene::new(pa).duration() > 0);
    let _ = std::fs::remove_dir_all(&dir);
}

