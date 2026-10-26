// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The layout read against pictures whose answer can be worked out by hand.

use super::*;
use crate::services;
use serde_json::{Value, json};

fn session(w: u32, h: u32) -> Session {
    Session::new(services::project::new_custom("Layout", w, h, 30.0))
}

/// A picture `w` by `h` that is see-through except for an opaque box.
fn padded_png(path: &std::path::Path, w: u32, h: u32, opaque: [u32; 4]) {
    let mut px = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let on = x >= opaque[0] && x < opaque[2] && y >= opaque[1] && y < opaque[3];
            px.extend(if on { [230, 40, 160, 255] } else { [0, 0, 0, 0] });
        }
    }
    let img = studio_media::rgba::RgbaImage { width: w, height: h, pixels: px };
    std::fs::write(path, studio_media::png::encode(&img)).unwrap();
}

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("studio-layout-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Puts the picture on the timeline and returns its clip id.
fn import(s: &mut Session, path: &std::path::Path) -> u32 {
    let before: Vec<u32> = s.doc.project.clips.iter().map(|c| c.id).collect();
    s.run("import", &json!({"paths": [path.to_string_lossy()], "to_timeline": true})).unwrap();
    s.doc.project.clips.iter().map(|c| c.id).find(|id| !before.contains(id)).expect("a new clip")
}

fn put(s: &mut Session, id: u32, change: impl FnOnce(&mut m::Clip)) {
    let mut mc = s.model_clip(id).unwrap();
    change(&mut mc);
    let seq = s.doc.seq();
    assert!(s.doc.apply("test", m::Edit::SetClip { seq, clip: Box::new(mc) }));
}

fn close(a: f32, b: f32, tol: f32, what: &str) {
    assert!((a - b).abs() <= tol, "{what}: {a} is not within {tol} of {b}");
}

fn near_box(got: grid::Rect, want: grid::Rect, tol: f32, what: &str) {
    for k in 0..4 {
        close(got[k], want[k], tol, &format!("{what} edge {k} ({got:?} against {want:?})"));
    }
}

fn card(s: &mut Session, colour: &str, x: f64, y: f64, scale: f64) -> u32 {
    let before: Vec<u32> = s.doc.project.clips.iter().map(|c| c.id).collect();
    s.edit(&[json!({"op": "card", "colour": colour, "at": 0, "duration": 4})], "card", false).unwrap();
    let id = s.doc.project.clips.iter().map(|c| c.id).find(|id| !before.contains(id)).unwrap();
    put(s, id, |c| {
        c.transform.position = m::Vec2::new(x, y);
        c.transform.scale_x = scale;
        c.transform.scale_y = scale;
    });
    id
}

fn item<'a>(lay: &'a Layout, id: u32) -> &'a Item {
    lay.items.iter().find(|i| i.id == id as u64 && i.object.is_none()).unwrap_or_else(|| panic!("clip {id} is not in the layout: {:?}", lay.items.iter().map(|i| (i.id, &i.name)).collect::<Vec<_>>()))
}

/// A picture with transparent padding, cropped, scaled, moved and masked:
/// its bounds are the opaque pixels that show, not the transform box.
#[test]
fn bounds_are_the_drawn_alpha_of_a_transformed_cropped_masked_picture() {
    let dir = temp("bounds");
    let path = dir.join("padded.png");
    // 320 by 180 fills a 640 by 360 canvas at exactly twice its size. The
    // opaque box is source x 80..240, y 45..135.
    padded_png(&path, 320, 180, [80, 45, 240, 135]);
    let mut s = session(640, 360);
    let id = import(&mut s, &path);
    let t = s.doc.project.clip(id).unwrap().start + 500_000;

    // Untouched: the opaque box at twice its size, centred.
    let lay = s.layout_at(t, &Opts::default()).unwrap();
    let it = item(&lay, id);
    assert!(it.from_alpha);
    near_box(it.bounds.unwrap(), [-160.0, -90.0, 160.0, 90.0], 3.0, "untouched");
    // The transform box is the whole picture, far bigger than what shows.
    let q = it.quad_box();
    near_box(q, [-320.0, -180.0, 320.0, 180.0], 1.0, "transform box");

    // Crop (removes the left and right padding exactly and the bottom half
    // of the box), halve it, move it right and up.
    put(&mut s, id, |c| {
        c.transform.crop = m::Crop { left: 0.25, right: 0.25, top: 0.1, bottom: 0.5 };
        c.transform.scale_x = 0.5;
        c.transform.scale_y = 0.5;
        c.transform.position = m::Vec2::new(100.0, -50.0);
    });
    let lay = s.layout_at(t, &Opts::default()).unwrap();
    let it = item(&lay, id);
    // Source (sx, sy) lands at (100 + sx - 160, -50 + sy - 90): the box
    // x 80..240 is 20..180, y 45..90 (cut at 90) is -95..-50.
    near_box(it.bounds.unwrap(), [20.0, -95.0, 180.0, -50.0], 3.0, "cropped");
    // The crop box reaches 27 px higher than anything drawn.
    let q = it.quad_box();
    near_box(q, [20.0, -122.0, 180.0, -50.0], 1.0, "crop box");
    assert!(it.bounds.unwrap()[1] - q[1] > 20.0, "the padding is not counted");

    // A mask over the right half of the picture takes the left half of
    // what is left away.
    put(&mut s, id, |c| {
        let mut mk = m::Mask::new(m::MaskId(900), m::MaskShape::Rectangle);
        mk.params.insert("position".into(), m::Value::Point(m::Vec2::new(0.75, 0.5)));
        mk.params.insert("size".into(), m::Value::Point(m::Vec2::new(0.5, 1.0)));
        c.masks.push(mk);
    });
    let lay = s.layout_at(t, &Opts::default()).unwrap();
    let it = item(&lay, id);
    near_box(it.bounds.unwrap(), [100.0, -95.0, 180.0, -50.0], 3.0, "masked");
    // The tool says the same in words and numbers.
    let said: Value = serde_json::from_str(&s.layout_tool(&json!({"t": 0.5, "view": "item", "clip": id})).unwrap()).unwrap();
    let b = said["box"].as_array().unwrap();
    assert!((b[0].as_i64().unwrap() - 100).abs() <= 3 && (b[3].as_i64().unwrap() + 50).abs() <= 3, "{said}");
    assert_eq!(said["is"], "picture");
    // Moved right off the frame: it is off screen.
    put(&mut s, id, |c| c.transform.position = m::Vec2::new(2000.0, 0.0));
    let lay = s.layout_at(t, &Opts::default()).unwrap();
    let it = item(&lay, id);
    assert!(it.bounds.is_none(), "{:?}", it.bounds);
    assert!(lay.flags(lay.items.iter().position(|i| i.id == id as u64).unwrap()).contains(&"off_screen".to_owned()));
    let _ = std::fs::remove_dir_all(dir);
}

/// What is drawn above hides what is below, by the pixels.
#[test]
fn z_order_and_occlusion_follow_the_draw_order() {
    let mut s = session(640, 360);
    // 256 by 144 pixels each; the second starts 60 px to the right of the
    // first, so 196 px of the first's 256 are under it (or the other way).
    let a = card(&mut s, "#cc2222", -100.0, 0.0, 0.4);
    let b = card(&mut s, "#2222cc", -40.0, 0.0, 0.4);
    let c = card(&mut s, "#22cc22", 230.0, 0.0, 0.2);
    let lay = s.layout_at(1_000_000, &Opts::default()).unwrap();
    let (ia, ib, ic) = (lay.index_of(a as u64, None).unwrap(), lay.index_of(b as u64, None).unwrap(), lay.index_of(c as u64, None).unwrap());
    // Draw order is the list order.
    assert!(lay.items.windows(2).all(|w| w[0].z < w[1].z));
    let (low, high) = if ia < ib { (ia, ib) } else { (ib, ia) };
    let share = 196.0 / 256.0;
    close(lay.items[low].visible, 1.0 - share, 0.03, "the lower card's visible share");
    close(lay.items[high].visible, 1.0, 0.01, "the upper card");
    assert!(lay.items[low].covered_by.iter().any(|&(j, s)| j == high && (s - share).abs() < 0.03), "{:?}", lay.items[low].covered_by);
    assert!(lay.items[high].covers.iter().any(|&(j, s)| j == low && (s - share).abs() < 0.03), "{:?}", lay.items[high].covers);
    // The third is clear of both.
    assert!(lay.items[ic].covered_by.is_empty() && lay.items[ic].covers.is_empty());
    close(lay.items[ic].visible, 1.0, 0.01, "the far card");
    // Overlap list: one pair, the smaller one's share.
    let pairs = lay.pairs(0.05);
    assert_eq!(pairs.len(), 1, "{pairs:?}");
    close(pairs[0].share, share, 0.03, "pair share");
    assert_eq!((pairs[0].below, pairs[0].above), (low, high));
    // The compact answer lists front to back and names what covers what.
    let said: Value = serde_json::from_str(&s.layout_tool(&json!({"t": 1.0})).unwrap()).unwrap();
    let items = said["items"].as_array().unwrap();
    assert!(items.len() >= 3);
    let z: Vec<i64> = items.iter().map(|i| i["z"].as_i64().unwrap()).collect();
    assert!(z.windows(2).all(|w| w[0] > w[1]), "front to back: {z:?}");
    assert_eq!(said["overlaps"].as_array().unwrap().len(), 1);
    // Fully hidden: the same card behind a bigger one.
    put(&mut s, a, |c| {
        c.transform.position = m::Vec2::ZERO;
        c.transform.scale_x = 0.2;
        c.transform.scale_y = 0.2;
    });
    put(&mut s, b, |c| {
        c.transform.position = m::Vec2::ZERO;
        c.transform.scale_x = 0.6;
        c.transform.scale_y = 0.6;
    });
    let lay = s.layout_at(1_000_000, &Opts::default()).unwrap();
    let (ia, ib) = (lay.index_of(a as u64, None).unwrap(), lay.index_of(b as u64, None).unwrap());
    let small = if lay.items[ia].z < lay.items[ib].z { ia } else { usize::MAX };
    if small != usize::MAX {
        assert!(lay.items[small].visible < 0.02);
        assert!(lay.flags(small).contains(&"hidden".to_owned()));
    }
}

/// Empty space and where a box would fit.
#[test]
fn free_space_and_fit_avoid_what_is_drawn() {
    let mut s = session(640, 360);
    // Two cards on the left half.
    let a = card(&mut s, "#cc2222", -200.0, -60.0, 0.3);
    let b = card(&mut s, "#2222cc", -200.0, 90.0, 0.3);
    let lay = s.layout_at(1_000_000, &Opts::default()).unwrap();
    let boxes: Vec<grid::Rect> = [a, b].iter().map(|&c| item(&lay, c).bounds.unwrap()).collect();
    let free = lay.free_boxes(&[], 0.0, (40.0, 40.0), 3);
    assert!(!free.is_empty());
    // The biggest free box is the right part of the frame and touches none.
    for r in &free {
        for b in &boxes {
            assert!(grid::rect_area(&grid::rect_intersect(r, b)) < 1.0, "free {r:?} meets {b:?}");
        }
    }
    assert!(free[0][2] - free[0][0] >= 300.0, "{:?}", free[0]);
    // A 200 by 80 title, wanted near the bottom: it fits, clear of both.
    let said: Value = serde_json::from_str(&s.layout_tool(&json!({"t": 1.0, "view": "fit", "size": [200, 80], "prefer": "lower_third"})).unwrap()).unwrap();
    assert_eq!(said["fits"], true, "{said}");
    let spot: Vec<f32> = said["spots"][0]["box"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap() as f32).collect();
    let spot: grid::Rect = [spot[0], spot[1], spot[2], spot[3]];
    for b in &boxes {
        assert!(grid::rect_area(&grid::rect_intersect(&spot, b)) < 1.0, "{spot:?} meets {b:?}");
    }
    assert!(spot[3] > 0.0, "lower third: {spot:?}");
    // Bigger than any gap: it does not fit, and the answer says how much smaller does.
    let said: Value = serde_json::from_str(&s.layout_tool(&json!({"t": 1.0, "view": "fit", "size": [900, 500]})).unwrap()).unwrap();
    assert_eq!(said["fits"], false, "{said}");
    assert!(said["fits_at_scale"].as_f64().unwrap() < 1.0);
    // Taken from another clip's size, near it.
    let said: Value = serde_json::from_str(&s.layout_tool(&json!({"t": 1.0, "view": "fit", "like": a, "near": b})).unwrap()).unwrap();
    assert_eq!(said["fits"], true, "{said}");
    // Balance: the weight is on the left.
    let bal = lay.balance_json().unwrap();
    assert!(bal["lean"].as_str().unwrap().contains("left"), "{bal}");
}

fn skeleton_for(aspect: f64) -> m::Skeleton {
    let mut sk = m::Skeleton { aspect, faces_right: Some(true), ..m::Skeleton::default() };
    let hips = sk.add("hips", None, m::Vec2::new(0.5, 0.7), m::Vec2::new(0.5, 0.5));
    sk.add("head", Some(hips), m::Vec2::new(0.5, 0.5), m::Vec2::new(0.5, 0.2));
    sk.add("forearm.front", Some(hips), m::Vec2::new(0.5, 0.5), m::Vec2::new(0.7, 0.5));
    sk.add("shin.front", Some(hips), m::Vec2::new(0.5, 0.7), m::Vec2::new(0.5, 0.95));
    sk
}

fn part(it: &Item, name: &str) -> (f32, f32) {
    it.parts.iter().find(|p| p.name == name).unwrap_or_else(|| panic!("no part {name} in {:?}", it.parts)).at
}

/// Each named part is where the rig puts it on the canvas, and moves with
/// the bones and with the layer.
#[test]
fn puppet_parts_follow_the_rig() {
    let dir = temp("rig");
    let path = dir.join("hero.png");
    padded_png(&path, 320, 180, [100, 20, 220, 170]);
    let mut s = session(640, 360);
    let id = import(&mut s, &path);
    put(&mut s, id, |c| c.skeleton = Some(skeleton_for(640.0 / 360.0)));
    let t = s.doc.project.clip(id).unwrap().start + 500_000;
    let lay = s.layout_at(t, &Opts::default()).unwrap();
    let it = item(&lay, id);
    assert_eq!(it.kind, Kind::Puppet);
    assert_eq!(it.role, Role::Figure);
    // uv (u, v) is canvas ((u - 0.5) * 640, (v - 0.5) * 360).
    let (hx, hy) = part(it, "head");
    close(hx, 0.0, 2.0, "head x");
    close(hy, (0.35 - 0.5) * 360.0, 2.0, "head y (the middle of the head bone)");
    let (hand_x, hand_y) = part(it, "hand.front");
    close(hand_x, 128.0, 2.0, "hand x");
    close(hand_y, 0.0, 2.0, "hand y");
    let (fx, fy) = part(it, "foot.front");
    close(fx, 0.0, 2.0, "foot x");
    close(fy, (0.95 - 0.5) * 360.0, 2.0, "foot y");
    assert_eq!(it.facing, Some("right"));

    // The forearm turns a quarter clockwise about its elbow: the hand is
    // now straight below it, the same 128 px away.
    put(&mut s, id, |c| {
        let sk = c.skeleton.as_mut().unwrap();
        let fid = sk.named("forearm.front").unwrap().id;
        sk.bone_mut(fid).unwrap().rotation = 90.0;
    });
    let lay = s.layout_at(t, &Opts::default()).unwrap();
    let it = item(&lay, id);
    let (hand_x, hand_y) = part(it, "hand.front");
    close(hand_x, 0.0, 2.5, "turned hand x");
    close(hand_y, 128.0, 2.5, "turned hand y");
    // The head did not move.
    close(part(it, "head").1, (0.35 - 0.5) * 360.0, 2.0, "head still");

    // The layer moved and mirrored: every part moves with it, and the
    // character now faces the other way.
    put(&mut s, id, |c| {
        c.transform.position = m::Vec2::new(100.0, 20.0);
        c.transform.flip_x = true;
    });
    let lay = s.layout_at(t, &Opts::default()).unwrap();
    let it = item(&lay, id);
    let (hand_x, hand_y) = part(it, "hand.front");
    close(hand_x, 100.0, 2.5, "moved hand x (mirrored, straight down)");
    close(hand_y, 148.0, 2.5, "moved hand y");
    assert_eq!(it.facing, Some("left"));
    // The compact answer carries the head and the way it faces.
    let said: Value = serde_json::from_str(&s.layout_tool(&json!({"t": 0.5})).unwrap()).unwrap();
    let me = said["items"].as_array().unwrap().iter().find(|i| i["id"] == json!(id)).unwrap();
    assert_eq!(me["faces"], "left");
    assert!(me["parts"].as_array().unwrap().iter().any(|p| p["part"] == "head"), "{me}");
    let _ = std::fs::remove_dir_all(dir);
}

/// A card in 3D: the projection of its box through the camera is where the
/// renderer draws it, with its depth.
#[test]
fn a_3d_card_projects_where_the_renderer_draws_it() {
    let mut s = session(640, 360);
    let id = card(&mut s, "#cc8822", 60.0, -20.0, 0.4);
    put(&mut s, id, |c| {
        c.transform.three_d = true;
        c.transform.position_z = 300.0;
        c.transform.rotation_y = 40.0;
    });
    // The default camera, then a camera that has trucked to one side.
    for camera in [false, true] {
        if camera {
            s.edit(&[json!({"op": "camera", "at": 0, "duration": 4, "move": {"type": "truck", "by": 120, "from": 0, "duration": 0.1}})], "cam", false).unwrap();
        }
        let lay = s.layout_at(3_000_000, &Opts::default()).unwrap();
        let it = item(&lay, id);
        assert!(it.from_alpha && it.bounds.is_some(), "camera {camera}: {:?}", it.bounds);
        let q = it.quad_box();
        let frame = lay.frame();
        let drawn = it.bounds.unwrap();
        // The turned card is a trapezoid: its box and what is drawn agree.
        near_box(drawn, grid::rect_intersect(&q, &frame), 4.0, &format!("projection (camera {camera})"));
        // Depth is the renderer's: further than the plane of the canvas.
        let zoom = studio_render::space::default_zoom(640.0);
        let d = it.depth.expect("a 3D card has a depth");
        if !camera {
            close(d, zoom + 300.0, 3.0, "depth");
        } else {
            assert!(d > 0.0);
        }
        assert!(it.tilt.is_some_and(|t| (t.1 - 40.0).abs() < 0.1), "{:?}", it.tilt);
        // The turned card's left edge is nearer the viewer than its right
        // edge is, so its box is taller on one side.
        let left = (it.quad[3].1 - it.quad[0].1).abs();
        let right = (it.quad[2].1 - it.quad[1].1).abs();
        assert!((left - right).abs() > 1.0, "foreshortened: {left} vs {right}");
    }
    let said: Value = serde_json::from_str(&s.layout_tool(&json!({"t": 3.0})).unwrap()).unwrap();
    assert!(said["camera"]["vertical_fov_deg"].as_f64().unwrap() > 1.0, "{said}");
    assert!(said["items"][0]["depth"].as_i64().unwrap() > 0, "{said}");
}

/// Motion over a range, from the placed boxes.
#[test]
fn motion_gives_a_path_and_speed() {
    let mut s = session(640, 360);
    let id = card(&mut s, "#cc2222", 0.0, 0.0, 0.2);
    put(&mut s, id, |c| {
        let mut tr = m::Track::default();
        tr.keys.push(m::Keyframe::new(0, m::Value::Point(m::Vec2::new(-200.0, 0.0)), m::Interp::Linear));
        tr.keys.push(m::Keyframe::new(2_000_000, m::Value::Point(m::Vec2::new(200.0, 0.0)), m::Interp::Linear));
        c.keyframes.insert("transform.position".into(), tr);
    });
    let said: Value = serde_json::from_str(&s.layout_tool(&json!({"view": "item", "clip": id, "from": 0.0, "to": 2.0})).unwrap()).unwrap();
    let mo = &said["motion"];
    assert!(mo["travel"].as_i64().unwrap() > 300, "{said}");
    assert!(mo["speed_avg"].as_i64().unwrap() > 100, "{said}");
    assert!(mo["path"].as_array().unwrap().len() >= 3);
    let first = mo["path"][0][1].as_i64().unwrap();
    let last = mo["path"].as_array().unwrap().last().unwrap()[1].as_i64().unwrap();
    assert!(last - first > 300, "{mo}");
}

/// A title over a character's head is found; the same title in free space
/// is not.
#[test]
fn a_title_over_a_face_is_flagged() {
    let dir = temp("face");
    let path = dir.join("hero.png");
    padded_png(&path, 320, 180, [100, 20, 220, 170]);
    let mut s = session(640, 360);
    let id = import(&mut s, &path);
    put(&mut s, id, |c| c.skeleton = Some(skeleton_for(640.0 / 360.0)));
    s.edit(&[json!({"op": "text", "text": "HELLO THERE", "at": 0, "duration": 4})], "title", false).unwrap();
    let title = s.doc.project.clips.iter().filter(|c| c.kind == view::ClipKind::Title).map(|c| c.id).max().unwrap();
    // The head is at (0, -54); put the title there.
    put(&mut s, title, |c| c.transform.position = m::Vec2::new(0.0, -54.0));
    let problems = s.layout_problems();
    let kinds: Vec<&str> = problems.iter().map(|p| p.kind).collect();
    assert!(kinds.contains(&"title_over_face"), "{problems:?}");
    let p = problems.iter().find(|p| p.kind == "title_over_face").unwrap();
    assert_eq!(p.clip, title as u64);
    assert!(p.problem.contains("head"), "{}", p.problem);
    // And staging_problems_seen carries it into check.
    assert!(s.staging_problems_seen(true).iter().any(|p| p["kind"] == "title_over_face"));
    // Lines of the text were read as boxes.
    let lay = s.layout_at(1_000_000, &Opts::default()).unwrap();
    let t = item(&lay, title);
    assert_eq!(t.role, Role::Text);
    assert!(!t.lines.is_empty(), "no line boxes");
    let tb = t.bounds.unwrap();
    for l in &t.lines {
        assert!(l[0] >= tb[0] - 2.0 && l[2] <= tb[2] + 2.0 && l[1] >= tb[1] - 2.0 && l[3] <= tb[3] + 2.0, "{l:?} not inside {tb:?}");
    }
    // Moved down into empty picture: no longer over the face.
    put(&mut s, title, |c| c.transform.position = m::Vec2::new(0.0, 150.0));
    let problems = s.layout_problems();
    assert!(!problems.iter().any(|p| p.kind == "title_over_face"), "{problems:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// A title cut by the frame, and lettering outside the title-safe box.
#[test]
fn off_screen_cut_and_unsafe_things_are_flagged() {
    let mut s = session(640, 360);
    s.edit(&[json!({"op": "text", "text": "A LONG TITLE THAT RUNS", "at": 0, "duration": 4})], "title", false).unwrap();
    let title = s.doc.project.clips.iter().filter(|c| c.kind == view::ClipKind::Title).map(|c| c.id).max().unwrap();
    put(&mut s, title, |c| c.transform.position = m::Vec2::new(300.0, 0.0));
    let kinds: Vec<&str> = s.layout_problems().iter().map(|p| p.kind).collect();
    assert!(kinds.contains(&"cut_by_frame") || kinds.contains(&"title_unsafe"), "{kinds:?}");
    put(&mut s, title, |c| c.transform.position = m::Vec2::new(0.0, 0.0));
    let kinds: Vec<&str> = s.layout_problems().iter().map(|p| p.kind).collect();
    assert!(!kinds.contains(&"cut_by_frame") && !kinds.contains(&"title_unsafe"), "{kinds:?}");
}

/// Object labels (from detection) ride on the item they were found in.
#[test]
fn labels_attach_to_items() {
    let mut s = session(640, 360);
    let id = card(&mut s, "#cc2222", 0.0, 0.0, 0.5);
    put(&mut s, id, |c| {
        c.meta.insert("labels".into(), json!([{"name": "dog", "box": [0.25, 0.25, 0.75, 0.75], "score": 0.9, "source": "objects"}]));
    });
    let lay = s.layout_at(1_000_000, &Opts::default()).unwrap();
    let it = item(&lay, id);
    assert_eq!(it.labels.len(), 1);
    // The card is 320 by 180 on the canvas; the middle half of it.
    near_box(it.labels[0].canvas, [-80.0, -45.0, 80.0, 45.0], 1.5, "label box");
    let said: Value = serde_json::from_str(&s.layout_tool(&json!({"t": 1.0})).unwrap()).unwrap();
    assert_eq!(said["items"][0]["labels"][0]["label"], "dog", "{said}");
}

/// World objects are projected through the project camera: the cabin the
/// camera was told to look at sits at the middle of the frame, at the
/// distance the camera stands from it.
#[test]
fn world_objects_are_projected_through_the_camera() {
    let _lock = crate::editor::viewport::test_lock();
    let mut s = session(320, 180);
    s.edit(
        &[json!({"op": "build_world", "description": "a clearing with a small cabin", "preset": "forest", "size": 120, "quality": "fast", "seed": 3,
            "buildings": [{"kind": "house", "at": [10, 8], "floors": 1, "width": 7, "depth": 6}]})],
        "world",
        false,
    )
    .unwrap();
    s.edit(&[json!({"op": "stand", "at": [3, 14], "look_at": [10, 8], "t": 0.0, "eye": 1.7})], "stand", false).unwrap();
    let lay = s.layout_at(500_000, &Opts { geometry_only: true, ..Opts::default() }).unwrap();
    let objects: Vec<&Item> = lay.items.iter().filter(|i| i.kind == Kind::WorldObject).collect();
    let clips: Vec<String> = s.doc.model.active().clips.iter().map(|c| format!("{} {:?} {}..{} {:?}", c.id.0, c.name, c.start, c.end(), match &c.source { m::ClipSource::Generator(g) => format!("{:?}", g.kind), _ => "other".into() })).collect();
    let nodes = studio_render::resolve::resolve(&studio_render::model_scene::ModelScene::new(std::sync::Arc::new(s.doc.model.clone())), 500_000).nodes.len();
    assert!(!objects.is_empty(), "no world objects were placed: {:?}; clips {clips:?}; {nodes} nodes", lay.items.iter().map(|i| (&i.name, i.kind)).collect::<Vec<_>>());
    assert!(lay.items.iter().any(|i| i.kind == Kind::World && i.role == Role::Backdrop));
    // The camera looks at the cabin: its box holds the middle of the frame.
    let cabin = objects.iter().filter(|o| o.quad_box()[0] <= 0.0 && o.quad_box()[2] >= 0.0 && o.quad_box()[1] <= 0.0 && o.quad_box()[3] >= 0.0).min_by(|a, b| a.depth.unwrap().total_cmp(&b.depth.unwrap()));
    let cabin = cabin.unwrap_or_else(|| panic!("nothing holds the middle of the frame: {:?}", objects.iter().map(|o| (&o.name, o.quad_box(), o.depth)).collect::<Vec<_>>()));
    // From (3, 14) to (10, 8) is 9.2 m; the world is 100 px to the metre.
    let d = cabin.depth.unwrap();
    assert!(d > 600.0 && d < 1300.0, "depth {d}");
    // Far to near: the list draws the far objects first.
    let depths: Vec<f32> = objects.iter().map(|o| o.depth.unwrap()).collect();
    assert!(depths.windows(2).all(|w| w[0] >= w[1]), "{depths:?}");
    let said: Value = serde_json::from_str(&s.layout_tool(&json!({"t": 0.5, "view": "frame", "draw": false})).unwrap()).unwrap();
    assert!(said["items"].as_array().unwrap().iter().any(|i| i["is"] == "world object" && i["depth"].is_number()), "{said}");
}
