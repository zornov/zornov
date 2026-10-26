// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Size budgets for what the AI reads. Everything a tool returns stays in
//! the AI's context for the rest of the job, so a first read, a state
//! read and an edit reply each have a ceiling here; a change that makes
//! them bigger must say so by raising the number. About four characters
//! make a token.

use super::*;
use serde_json::{Value, json};

fn session() -> Session {
    Session::new(services::project::new_custom("Budget", 1280, 720, 30.0))
}

fn answer(s: &mut Session, tool: &str, args: Value) -> String {
    match s.run(tool, &args).unwrap_or_else(|e| panic!("{tool}: {e}")).remove(0) {
        Content::Text(t) => t,
        Content::Jpeg(_) => panic!("{tool} sent a picture"),
    }
}

/// The first read: the capability map and the core.
#[test]
fn the_first_guide_read_stays_small() {
    let first = guide_answer(&json!({})).unwrap();
    // 13_500: the map and the skill index each gained lines for physics, objects in
    // footage, canvas layout, fonts and search_media (about 1 400 characters in all).
    assert!(first.len() <= 13_500, "guide with no topic is {} characters (~{} tokens)", first.len(), first.len() / 4);
}

/// Reading a subject asks for its outline first, and no first read of any
/// subject is more than a few thousand tokens.
#[test]
fn no_first_read_of_a_guide_subject_is_large() {
    let mut worst = (0, String::new());
    for (heading, _) in guide_sections() {
        let key = heading.to_lowercase();
        let t = guide_answer(&json!({"topic": key})).unwrap_or_else(|e| panic!("{heading}: {e}"));
        if t.len() > worst.0 {
            worst = (t.len(), heading.to_owned());
        }
        assert!(t.len() <= 16_000, "\"{heading}\" is {} characters on a first read; parts.rs serves a long section as an outline", t.len());
    }
    eprintln!("largest first read: {} characters ({})", worst.0, worst.1);
    for whole in ["all", "craft"] {
        let t = guide_answer(&json!({"topic": whole})).unwrap();
        assert!(t.len() <= 6_000, "`{whole}` is {} characters; it is a contents list", t.len());
    }
}

/// The craft book's chapters are served the same way.
#[test]
fn no_first_read_of_a_craft_chapter_is_large() {
    for name in craft_book::topic_words() {
        let t = guide_answer(&json!({"topic": name})).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(t.len() <= 16_000, "craft chapter \"{name}\" is {} characters on a first read", t.len());
    }
}

/// Nothing is lost by serving in parts: every part of every long section
/// can be asked for, and the parts together carry all of the text.
#[test]
fn every_section_is_still_fully_reachable() {
    let mut books: Vec<(String, String)> = guide_sections().into_iter().map(|(h, t)| (h.to_owned(), t.to_owned())).collect();
    for name in craft_book::topic_words() {
        books.push((name.to_owned(), craft_book::topic(name).unwrap()));
    }
    let mut split_sections = 0;
    for (name, text) in &books {
        let (intro, parts) = parts::split(text);
        if text.len() <= parts::BIG || parts.len() < 3 {
            continue;
        }
        split_sections += 1;
        let mut got = intro.to_owned();
        for p in &parts {
            got.push('\n');
            got.push_str(&parts::serve(text, name, Some(p.title), false).unwrap_or_else(|e| panic!("{name}: part {}: {e}", p.title)));
        }
        let words = |s: &str| s.split_whitespace().count();
        assert!(words(&got) >= words(text) - 2, "{name}: the parts carry {} words of {}", words(&got), words(text));
        assert_eq!(parts::serve(text, name, None, true).unwrap(), *text);
    }
    assert!(split_sections >= 2, "only {split_sections} long sections were split; did the split stop recognising titles?");
    // The whole guide and the whole book are still one request away.
    assert_eq!(guide_answer(&json!({"topic": "all", "full": true})).unwrap(), GUIDE);
    assert_eq!(guide_answer(&json!({"topic": "craft", "full": true})).unwrap(), craft_book::BOOK);
    // And every guide section has a name that opens it.
    for (heading, text) in guide_sections() {
        let key = heading.to_lowercase();
        let got = guide_answer(&json!({"topic": key, "full": true})).unwrap();
        assert!(got.starts_with(&format!("## {heading}")) || got == text, "\"{key}\" does not open \"{heading}\"");
    }
}

/// An op is read by name: its row, not the page it is on.
#[test]
fn an_op_is_read_by_name() {
    let rows = guide_answer(&json!({"topic": "ops", "part": "split, trim"})).unwrap();
    assert!(rows.contains("| `split` |") && rows.contains("| `trim` |"), "{rows}");
    assert!(rows.len() < 1_200, "two rows came as {} characters", rows.len());
    let outline = guide_answer(&json!({"topic": "ops"})).unwrap();
    assert!(outline.len() < 6_000, "the ops outline is {} characters", outline.len());
    for op in ["place", "split", "text", "animate", "puppet"] {
        assert!(outline.contains(op) || guide_answer(&json!({"topic": "ops", "part": op})).is_ok(), "op {op} cannot be found in the ops section");
    }
}

/// The tool list rides in every request; reading it is a line a tool.
#[test]
fn the_tool_list_stays_within_its_budget() {
    let tools = mcp::tools();
    let json = serde_json::to_string(&tools).unwrap();
    eprintln!("tool list: {} tools, {} characters (~{} tokens)", tools.len(), json.len(), json.len() / 4);
    // 98_500: the objects tool (about 1 000 characters) is one tool more, the search_media
    // tool (about 2 200) and the fonts tool (about 1 300) two more.
    assert!(json.len() <= 98_500, "the tool list is {} characters; keep property descriptions short and put the detail in tool_reference.md", json.len());
    let index = guide_answer(&json!({"topic": "tools"})).unwrap();
    eprintln!("tool index: {} characters", index.len());
    assert!(index.len() <= 12_000, "the tool index is {} characters; it replaces dumping the {} raw definitions", index.len(), tools.len());
    for t in &tools {
        let name = t["name"].as_str().unwrap();
        assert!(index.contains(&format!("- {name}(")), "{name} is missing from the tool index");
        let one = guide_answer(&json!({"topic": format!("tool:{name}")})).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(one.contains("Fields:"), "{name}: its fields are not listed");
    }
    let edit = guide_answer(&json!({"topic": "tool:edit"})).unwrap();
    assert!(edit.contains("`ops`"), "{edit}");
}

/// `search_media` is one more line in every request; it stays short.
#[test]
fn the_search_tool_stays_small() {
    let t = mcp::tools().into_iter().find(|t| t["name"] == "search_media").expect("search_media is listed");
    let n = serde_json::to_string(&t).unwrap().len();
    assert!(n <= 2_200, "search_media is {n} characters in the tool list");
    let one = guide_answer(&json!({"topic": "tool:search_media"})).unwrap();
    assert!(one.contains("Fields:"), "{one}");
}

/// A state read of a project with a hundred clips is a few thousand tokens.
#[test]
fn a_state_read_stays_small() {
    let mut s = session();
    let ops: Vec<Value> = (0..100).map(|i| json!({"op": "card", "colour": "#336699", "at": i as f64 * 0.5, "duration": 0.4})).collect();
    s.edit(&ops, "cards", false).unwrap();
    let summary = answer(&mut s, "summary", json!({}));
    assert!(summary.len() <= 40_000, "summary of 100 clips is {} characters", summary.len());
    let per_clip = summary.len() / 100;
    assert!(per_clip <= 330, "{per_clip} characters a clip in summary; a clip line is who, when and what is different");
    let rev = s.doc.history.revision();
    let same = answer(&mut s, "summary", json!({"since": rev}));
    assert!(same.len() <= 400, "summary since the last read is {} characters", same.len());
    let script = answer(&mut s, "script", json!({}));
    assert!(script.len() / 100 <= 120, "{} characters a clip in script", script.len() / 100);
}

/// An edit that makes forty clips answers with a few thousand characters,
/// not a page per clip.
#[test]
fn a_large_edit_reply_stays_small() {
    let mut s = session();
    let ops: Vec<Value> = (0..40).map(|i| json!({"op": "card", "colour": "#336699", "at": i as f64 * 0.5, "duration": 0.4})).collect();
    let reply = answer(&mut s, "edit", json!({"ops": ops, "label": "forty cards"}));
    assert!(reply.len() <= 9_000, "an edit of 40 clips replied with {} characters (~{} tokens)", reply.len(), reply.len() / 4);
    let v: Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(v["changed"]["added"].as_array().unwrap().len(), 40, "every new clip is still named");
}

/// A model built of a hundred parts has a hundred materials; a reply about
/// it does not list a hundred identical ones.
#[test]
fn a_model_with_many_materials_is_described_briefly() {
    let mut mesh = studio_mesh::Mesh::default();
    mesh.materials = (0..120).map(|_| studio_mesh::Material::default()).collect();
    mesh.materials[7].name = "Copper".into();
    let said = models::describe(&mesh).to_string();
    assert!(said.len() < 700, "{} characters: {said}", said.len());
    assert!(said.contains("Copper") && said.contains("119") || said.contains("\"total\":120"), "{said}");
    mesh.materials.truncate(3);
    assert!(models::describe(&mesh).to_string().matches("\"index\"").count() == 3, "a few materials are all listed");
}

// ------------------------------------------------------------ pictures

/// (width, height) from a JPEG's frame header.
fn jpeg_size(j: &[u8]) -> (u32, u32) {
    let mut i = 2;
    while i + 9 < j.len() {
        assert_eq!(j[i], 0xFF, "a JPEG marker at {i}");
        let m = j[i + 1];
        let len = u16::from_be_bytes([j[i + 2], j[i + 3]]) as usize;
        if matches!(m, 0xC0..=0xC2) {
            return (u16::from_be_bytes([j[i + 7], j[i + 8]]) as u32, u16::from_be_bytes([j[i + 5], j[i + 6]]) as u32);
        }
        i += 2 + len;
    }
    panic!("no frame header in the JPEG");
}

fn pictures(s: &mut Session, tool: &str, args: Value) -> (Vec<u8>, String) {
    let parts = s.run(tool, &args).unwrap_or_else(|e| panic!("{tool}: {e}"));
    let jpeg = parts.iter().find_map(|p| if let Content::Jpeg(j) = p { Some(j.clone()) } else { None }).unwrap_or_else(|| panic!("{tool} sent no picture"));
    let said = parts.iter().filter_map(|p| if let Content::Text(t) = p { Some(t.clone()) } else { None }).collect::<Vec<_>>().join(" ");
    (jpeg, said)
}

/// A 1080p cut with something to see.
#[cfg(feature = "render")]
fn busy_session() -> Session {
    let mut s = Session::new(services::project::new_custom("Pictures", 1920, 1080, 30.0));
    let ops: Vec<Value> = (0..8).map(|i| json!({"op": "card", "colour": format!("#{:02x}{:02x}{:02x}", 40 + i * 25, 200 - i * 20, 90 + i * 10), "at": i as f64, "duration": 1.0, "lane": "main"})).collect();
    s.edit(&ops, "cards", false).unwrap();
    s.edit(&[json!({"op": "text", "text": "Zornov picture budget 0123456789", "at": 0, "duration": 8})], "title", false).unwrap();
    s
}

/// Looking while working is light: a frame and a contact sheet of the
/// whole cut stay under a byte budget, and asking for more width does not
/// get past the light limit.
#[cfg(feature = "render")]
#[test]
fn default_pictures_stay_under_a_byte_budget() {
    let mut s = busy_session();
    let (frame, _) = pictures(&mut s, "frame", json!({"t": 1.5}));
    assert!(frame.len() <= 60_000, "a default frame is {} bytes", frame.len());
    assert_eq!(jpeg_size(&frame).0, 512);
    let (sheet, said) = pictures(&mut s, "contact_sheet", json!({}));
    assert!(sheet.len() <= 150_000, "a default contact sheet is {} bytes", sheet.len());
    eprintln!("default frame {} bytes, default sheet {} bytes", frame.len(), sheet.len());
    assert_eq!(jpeg_size(&sheet).0, 1024);
    assert!(said.contains("times"));
    let (wide, _) = pictures(&mut s, "frame", json!({"t": 2.5, "width": 3000}));
    assert!(jpeg_size(&wide).0 <= 1024, "a light frame is limited to 1024 px, not {}", jpeg_size(&wide).0);
    let (wide_sheet, _) = pictures(&mut s, "contact_sheet", json!({"width": 3000}));
    assert!(jpeg_size(&wide_sheet).0 <= 1536);
    // A tall canvas is limited by its height.
    assert!(render::light_width(2000, 1080, 1920) <= 576);
    assert_eq!(render::light_width(512, 1080, 1920), 512);
}

/// Full detail is what it always was: 1280 px frames and 1920 px sheets at
/// JPEG 85, whether asked as `true` or `"full"`.
#[cfg(feature = "render")]
#[test]
fn full_detail_is_unchanged() {
    assert_eq!(render::picture_size(&json!({"detail": true}), render::FRAME_WIDTH), (1280, 85));
    assert_eq!(render::picture_size(&json!({"detail": "full"}), render::FRAME_WIDTH), (1280, 85));
    assert_eq!(render::picture_size(&json!({"detail": "full"}), render::SHEET_WIDTH), (1920, 85));
    assert_eq!(render::picture_size(&json!({"detail": "full", "width": 1600}), render::FRAME_WIDTH), (1600, 85), "full is not limited");
    assert_eq!(render::picture_size(&json!({"detail": false}), render::FRAME_WIDTH), (512, 70));
    assert_eq!(render::picture_size(&json!({}), render::SHEET_WIDTH), (1024, 70));
    let mut s = busy_session();
    let (old, _) = pictures(&mut s, "frame", json!({"t": 1.5, "detail": true}));
    let (new, _) = pictures(&mut s, "frame", json!({"t": 1.5, "detail": "full", "force": true}));
    assert_eq!(old, new, "the same picture, byte for byte");
    assert_eq!(jpeg_size(&new).0, 1280);
    eprintln!("full frame {} bytes", new.len());
    let (sheet, _) = pictures(&mut s, "contact_sheet", json!({"detail": "full"}));
    assert_eq!(jpeg_size(&sheet).0, 1920);
}

/// The review that ends a job sends the cut at full detail.
#[cfg(feature = "render")]
#[test]
fn a_passing_review_sends_a_full_detail_sheet() {
    let mut s = busy_session();
    let parts = s.run("review", &json!({})).unwrap();
    let text = parts.iter().find_map(|p| if let Content::Text(t) = p { Some(t.clone()) } else { None }).unwrap();
    let v: Value = serde_json::from_str(&text).unwrap();
    if v["ok"] == true {
        let j = parts.iter().find_map(|p| if let Content::Jpeg(j) = p { Some(j) } else { None }).expect("the final look");
        assert_eq!(jpeg_size(j).0, 1920);
    } else {
        assert!(!parts.iter().any(|p| matches!(p, Content::Jpeg(_))), "no picture while notes stand");
    }
}

/// A region is cut from the source pixels as they are: no resampling.
#[test]
fn a_region_crop_is_pixel_exact() {
    use studio_media::rgba::RgbaImage;
    let (w, h) = (1920u32, 1080u32);
    let mut pixels = Vec::new();
    for y in 0..h {
        for x in 0..w {
            pixels.extend([(x % 251) as u8, (y % 241) as u8, ((x / 7 + y / 3) % 256) as u8, 255]);
        }
    }
    let src = RgbaImage { width: w, height: h, pixels };
    let r = render::region_of(&json!({"region": [0.25, 0.5, 0.125, 0.25]})).unwrap().unwrap();
    let (part, (x0, y0, cw, ch)) = render::crop(&src, r);
    assert_eq!((x0, y0, cw, ch), (480, 540, 240, 270));
    assert_eq!((part.width, part.height), (cw, ch));
    for y in 0..ch {
        for x in 0..cw {
            let (a, b) = (((y * cw + x) * 4) as usize, (((y0 + y) * w + x0 + x) * 4) as usize);
            assert_eq!(part.pixels[a..a + 4], src.pixels[b..b + 4], "pixel {x},{y}");
        }
    }
    assert_eq!(render::region_of(&json!({"region": {"x": 0.0, "y": 0.0, "w": 0.5, "h": 0.5}})).unwrap().unwrap(), render::Region { x: 0.0, y: 0.0, w: 0.5, h: 0.5 });
    assert!(render::region_of(&json!({"region": [0.5, 0.5, 0.2]})).is_err());
    assert!(render::region_of(&json!({"region": [2.0, 0.0, 0.5, 0.5]})).is_err());
    assert!(render::region_of(&json!({})).unwrap().is_none());
    // Only a crop longer than the limit is brought down.
    assert_eq!(render::shrink(part.clone(), 1024).pixels, part.pixels);
    let small = render::shrink(src, 960);
    assert_eq!((small.width, small.height), (960, 540));
}

/// Through the tool: a region of the frame comes back at the frame's own
/// pixels, a fraction of the bytes of a full-detail frame.
#[cfg(feature = "render")]
#[test]
fn a_region_of_a_frame_is_close_and_cheap() {
    let mut s = busy_session();
    let (full, _) = pictures(&mut s, "frame", json!({"t": 1.5, "detail": "full"}));
    let (part, said) = pictures(&mut s, "frame", json!({"t": 1.5, "region": [0.25, 0.25, 0.25, 0.25]}));
    let v: Value = serde_json::from_str(&said).unwrap();
    assert_eq!(v["source_pixels"]["rect"], json!([480, 270, 480, 270]), "{said}");
    assert_eq!(jpeg_size(&part), (480, 270), "native pixels, not scaled");
    assert!(part.len() < full.len(), "{} against {} bytes", part.len(), full.len());
}

/// What `objects` says about a crowded shot stays short: the list, a
/// place, the neighbours, the free space, each a few hundred tokens at
/// most, however many things the map holds.
#[test]
fn the_objects_answers_stay_small() {
    let map = objects::tests::sample_map(80);
    let (mut s, id) = objects::tests::session_with_objects(&map);
    let mut worst = (0, String::new());
    let mut check = |name: &str, args: Value, max: usize| {
        let t = answer(&mut s, "objects", args);
        eprintln!("objects {name}: {} characters (~{} tokens)", t.len(), t.len() / 4);
        if t.len() > worst.0 {
            worst = (t.len(), name.to_owned());
        }
        assert!(t.len() <= max, "objects {name} is {} characters; it should stay under {max} so a crowded shot costs a few hundred tokens", t.len());
        t
    };
    let list = check("list", json!({"clip": id}), 2_000);
    assert!(list.lines().count() <= 15, "{list}");
    let detail = check("list with detail", json!({"clip": id, "detail": true}), 5_000);
    assert!(detail.len() > list.len());
    check("where", json!({"clip": id, "object": "#3", "t": 1.0}), 900);
    check("when (all of them)", json!({"clip": id, "action": "when"}), 2_500);
    check("when (one)", json!({"clip": id, "object": "dog"}), 1_500);
    check("around", json!({"clip": id, "action": "around", "object": 5, "t": 1.0}), 1_200);
    check("space", json!({"clip": id, "action": "space", "t": 0.0, "to": 3.0}), 900);
    check("text", json!({"clip": id, "action": "text"}), 900);
    // `see` gains one short line a shot.
    let see = answer(&mut s, "see", json!({}));
    let line = see.lines().find(|l| l.trim_start().starts_with("objects:")).expect("see lists the objects");
    assert!(line.len() <= 200, "the objects line of see is {} characters: {line}", line.len());
}

/// Where-things-are answers: a frame of forty things, the line inspect
/// carries for one clip, and the line see carries for a moment.
#[cfg(feature = "render")]
#[test]
fn a_layout_answer_stays_small() {
    let mut s = session();
    let ops: Vec<Value> = (0..40).map(|_| json!({"op": "card", "colour": "#336699", "at": 0, "duration": 4})).collect();
    s.edit(&ops, "cards", false).unwrap();
    let ids: Vec<u32> = s.doc.project.clips.iter().map(|c| c.id).collect();
    for (i, id) in ids.iter().enumerate() {
        let mut mc = s.model_clip(*id).unwrap();
        // An 8 by 5 grid of small cards, each overlapping its neighbour a little.
        mc.transform.position = m::Vec2::new((i % 8) as f64 * 150.0 - 525.0, (i / 8) as f64 * 130.0 - 260.0);
        mc.transform.scale_x = 0.12;
        mc.transform.scale_y = 0.12;
        let seq = s.doc.seq();
        s.doc.apply("spread", m::Edit::SetClip { seq, clip: Box::new(mc) });
    }
    let frame = answer(&mut s, "canvas_layout", json!({"t": 1.0}));
    eprintln!("layout of forty things: {} characters (~{} tokens)", frame.len(), frame.len() / 4);
    assert!(frame.len() <= 9_000, "a whole-frame layout of 40 things is {} characters; the compact answer lists {} things and names the rest", frame.len(), 24);
    let v: Value = serde_json::from_str(&frame).unwrap();
    assert!(v["items"].as_array().unwrap().len() <= 24 && v["more"].is_string(), "the list is capped and says so");
    // Per thing: a short object, not a page.
    let per = frame.len() / v["items"].as_array().unwrap().len().max(1);
    assert!(per <= 330, "{per} characters a thing in the compact frame answer");
    // The other views stay as small.
    for view in ["overlaps", "free", "safe"] {
        let r = answer(&mut s, "canvas_layout", json!({"t": 1.0, "view": view}));
        assert!(r.len() <= 6_000, "view {view} is {} characters", r.len());
    }
    let fit = answer(&mut s, "canvas_layout", json!({"t": 1.0, "view": "fit", "size": [200, 80]}));
    assert!(fit.len() <= 900, "fit is {} characters", fit.len());
    let one = answer(&mut s, "canvas_layout", json!({"t": 1.0, "view": "item", "clip": ids[9]}));
    assert!(one.len() <= 2_400, "one item in detail is {} characters", one.len());
    // What inspect carries.
    let inspected: Value = serde_json::from_str(&answer(&mut s, "inspect", json!({"clip": ids[9], "at": 1.0}))).unwrap();
    let layout = inspected["layout"].to_string();
    assert!(inspected["layout"].is_object(), "inspect carries a layout summary");
    assert!(layout.len() <= 700, "the layout summary in inspect is {} characters", layout.len());
    // What see carries for a moment.
    let line = s.layout_line(1_000_000).expect("a layout line");
    assert!(line.len() <= 700, "the layout line in see is {} characters: {line}", line.len());
}
