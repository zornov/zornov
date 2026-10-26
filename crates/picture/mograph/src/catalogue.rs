// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The templates. Each is designed on a 1920 × 1080 canvas and scaled to
//! the one it is built for (`Ctx::k`); entrances take the first second or
//! so, exits the last half second, and everything between holds still or
//! breathes, so a template stretches to any length.

use std::sync::OnceLock;

use crate::charts::{self, Look};
use crate::{Anchor, Category, Control, Counter, Ctx, Layer, ShapeLayer, Template, TextLayer};
use serde_json::{Value, json};
use studio_text::animator;
use studio_text::{Reveal, RevealKind, TextMotion, TextUnit};

const FONT: &str = "Manrope";

fn reveal(kind: RevealKind, unit: TextUnit, dur: f64, exit: RevealKind) -> TextMotion {
    TextMotion { reveal: Some(Reveal::new(kind, unit).overlap(0.75)), reveal_duration: dur as f32, exit: Some(Reveal::new(exit, unit).overlap(0.9)), exit_duration: 0.45, ..Default::default() }
}

#[allow(clippy::too_many_arguments)]
fn text(name: &str, t: String, size: f64, colour: String, weight: u16, x: f64, y: f64, anchor: Anchor, start: f64, end: f64, motion: TextMotion) -> Layer {
    Layer::Text(TextLayer { name: name.into(), text: t, font: FONT.into(), weight, italic: false, size, colour, letter_spacing: 0.0, anchor, x, y, start, duration: (end - start).max(0.1), motion: Some(motion), counter: None, path: None })
}

fn spaced(l: Layer, spacing: f64) -> Layer {
    match l {
        Layer::Text(mut t) => {
            t.letter_spacing = spacing;
            Layer::Text(t)
        }
        other => other,
    }
}

/// Lays a title along a path (`{"circle": r}`, `{"arc": r}`…, as the
/// `text` op's `path`), its start keyed by `start` (0..1 along it).
fn on_path(l: Layer, path: Value, start: Value) -> Layer {
    match l {
        Layer::Text(mut t) => {
            let mut p = path;
            p["start"] = start;
            t.path = Some(p);
            Layer::Text(t)
        }
        other => other,
    }
}

fn shape(name: &str, items: Value, x: f64, y: f64, start: f64, end: f64) -> Layer {
    Layer::Shape(ShapeLayer { name: name.into(), doc: json!({"items": items}), x, y, start, duration: (end - start).max(0.1) })
}

/// Two keys.
fn k2(t0: f64, v0: Value, t1: f64, v1: Value, ease: &str) -> Value {
    json!({"keys": [[t0, v0], [t1, v1, ease]]})
}

/// In over `[a, b]`, out over the last `out` seconds of a layer `len` long.
fn in_out(a: f64, b: f64, len: f64, out: f64, v_off: Value, v_on: Value, ease_in: &str) -> Value {
    json!({"keys": [[a, v_off.clone()], [b, v_on.clone(), ease_in], [(len - out).max(b), v_on], [len, v_off, "in"]]})
}

fn lower_thirds(v: &mut Vec<Template>) {
    v.push(Template {
        id: "lower_third.bar",
        name: "Accent Bar",
        category: Category::LowerThird,
        description: "Name and role beside an accent bar, on a dark box that wipes open from the bar",
        tags: "lower third name title bar box clean corporate interview",
        duration: 5.0,
        controls: vec![Control::text("name", "Name", "Jordan Rivers"), Control::text("role", "Role", "Director of Photography"), Control::colour("accent", "Accent", "#7c5cff"), Control::colour("box", "Box", "#0f1218"), Control::colour("text", "Text", "#ffffff")],
        build: |c| {
            let k = c.k();
            let (name, role) = (c.text("name"), c.text("role"));
            let (ns, rs) = (56.0 * k, 30.0 * k);
            let nw = c.measure(&name, FONT, 800, ns, 0.0)[0];
            let rw = c.measure(&role, FONT, 500, rs, 4.0)[0];
            let x0 = -c.w / 2.0 + 0.07 * c.w;
            let y0 = c.h / 2.0 - 0.2 * c.h;
            let box_w = nw.max(rw) + 70.0 * k;
            let box_h = 130.0 * k;
            let d = c.dur;
            vec![
                shape("Box", json!([{"type": "group", "scale": in_out(0.1, 0.55, d, 0.45, json!([0, 1]), json!([1, 1]), "out"), "items": [
                    {"type": "rect", "position": [box_w / 2.0, 0], "size": [box_w, box_h]},
                    {"type": "fill", "colour": c.colour("box"), "opacity": 0.82},
                ]}]), x0, y0, 0.0, d),
                shape("Bar", json!([{"type": "group", "scale": in_out(0.0, 0.35, d, 0.3, json!([1, 0]), json!([1, 1]), "overshoot"), "items": [
                    {"type": "rect", "position": [-5.0 * k, 0], "size": [10.0 * k, box_h]},
                    {"type": "fill", "colour": c.colour("accent")},
                ]}]), x0, y0, 0.0, d),
                text("Name", name, ns, c.colour("text"), 800, x0 + 34.0 * k, y0 - 20.0 * k, Anchor::Left, 0.3, d, reveal(RevealKind::Rise, TextUnit::Word, 0.6, RevealKind::Fade)),
                spaced(text("Role", role, rs, c.colour("text"), 500, x0 + 35.0 * k, y0 + 34.0 * k, Anchor::Left, 0.45, d, reveal(RevealKind::SlideRight, TextUnit::Character, 0.7, RevealKind::Fade)), 4.0),
            ]
        },
    });
    v.push(Template {
        id: "lower_third.news",
        name: "Newsroom",
        category: Category::LowerThird,
        description: "Two-tone bars: the name on a bold colour, the story on white below, sliding in from the left",
        tags: "lower third news broadcast strap two tone breaking story",
        duration: 6.0,
        controls: vec![Control::text("name", "Name", "SAM OKAFOR"), Control::text("story", "Story", "Live from the harbour as the storm passes"), Control::colour("accent", "Accent", "#e63946"), Control::colour("strip", "Strip", "#ffffff"), Control::colour("strip_text", "Strip text", "#101418")],
        build: |c| {
            let k = c.k();
            let (name, story) = (c.text("name"), c.text("story"));
            let (ns, ss) = (44.0 * k, 32.0 * k);
            let nw = c.measure(&name, FONT, 800, ns, 2.0)[0] + 60.0 * k;
            let sw = c.measure(&story, FONT, 600, ss, 0.0)[0] + 60.0 * k;
            let x0 = -c.w / 2.0 + 0.06 * c.w;
            let y0 = c.h / 2.0 - 0.22 * c.h;
            let (h1, h2) = (72.0 * k, 60.0 * k);
            let d = c.dur;
            let slide = |delay: f64, w: f64| in_out(delay, delay + 0.45, d, 0.4, json!([-w - c.w * 0.1, 0]), json!([0, 0]), "out");
            vec![
                shape("Story strip", json!([{"type": "group", "position": slide(0.15, sw), "items": [{"type": "rect", "position": [sw / 2.0, h1 / 2.0 + h2 / 2.0], "size": [sw, h2]}, {"type": "fill", "colour": c.colour("strip")}]}]), x0, y0, 0.0, d),
                shape("Name bar", json!([{"type": "group", "position": slide(0.0, nw), "items": [{"type": "rect", "position": [nw / 2.0, 0], "size": [nw, h1]}, {"type": "fill", "colour": c.colour("accent")}]}]), x0, y0, 0.0, d),
                spaced(text("Name", name, ns, "#ffffff".into(), 800, x0 + 30.0 * k, y0, Anchor::Left, 0.35, d, reveal(RevealKind::Fade, TextUnit::Character, 0.4, RevealKind::Fade)), 2.0),
                text("Story", story, ss, c.colour("strip_text"), 600, x0 + 30.0 * k, y0 + h1 / 2.0 + h2 / 2.0, Anchor::Left, 0.5, d, reveal(RevealKind::Rise, TextUnit::Word, 0.6, RevealKind::Fade)),
            ]
        },
    });
    v.push(Template {
        id: "lower_third.underline",
        name: "Signature",
        category: Category::LowerThird,
        description: "A name typed on over a hand-drawn underline that draws itself, the role fading in beneath",
        tags: "lower third minimal underline handwritten signature elegant vlog",
        duration: 5.0,
        controls: vec![Control::text("name", "Name", "Mira Chen"), Control::text("role", "Role", "Founder, Kiln Studio"), Control::colour("line", "Line", "#ffb020"), Control::colour("text", "Text", "#ffffff")],
        build: |c| {
            let k = c.k();
            let (name, role) = (c.text("name"), c.text("role"));
            let ns = 60.0 * k;
            let nw = c.measure(&name, FONT, 700, ns, 0.0)[0];
            let x0 = -c.w / 2.0 + 0.08 * c.w;
            let y0 = c.h / 2.0 - 0.22 * c.h;
            let d = c.dur;
            vec![
                text("Name", name, ns, c.colour("text"), 700, x0, y0, Anchor::Left, 0.0, d, reveal(RevealKind::Type, TextUnit::Character, 0.7, RevealKind::Fade)),
                shape("Underline", json!([
                    {"type": "path", "points": [[0, 0], [nw * 0.35, -6.0 * k], [nw * 0.75, 2.0 * k], [nw * 1.08, -8.0 * k]], "smooth": true},
                    {"type": "trim", "end": k2(0.35, json!(0), 1.0, json!(1), "out"), "start": k2(d - 0.5, json!(0), d, json!(1), "in")},
                    {"type": "stroke", "colour": c.colour("line"), "width": 7.0 * k, "cap": "round"},
                ]), x0, y0 + ns * 0.62, 0.0, d),
                text("Role", role, 30.0 * k, c.colour("text"), 500, x0, y0 + ns * 1.2, Anchor::Left, 0.8, d, reveal(RevealKind::Fade, TextUnit::Word, 0.5, RevealKind::Fade)),
            ]
        },
    });
    v.push(Template {
        id: "lower_third.pill",
        name: "Pill",
        category: Category::LowerThird,
        description: "A rounded pill that pops in with a coloured dot, the name and handle inside",
        tags: "lower third pill rounded social handle creator friendly modern",
        duration: 5.0,
        controls: vec![Control::text("name", "Name", "Riley Park"), Control::text("handle", "Handle", "@rileymakes"), Control::colour("pill", "Pill", "#ffffff"), Control::colour("dot", "Dot", "#21c7a8"), Control::colour("text", "Text", "#101418")],
        build: |c| {
            let k = c.k();
            let (name, handle) = (c.text("name"), c.text("handle"));
            let (ns, hs) = (40.0 * k, 28.0 * k);
            let w = c.measure(&name, FONT, 800, ns, 0.0)[0].max(c.measure(&handle, FONT, 500, hs, 0.0)[0]) + 140.0 * k;
            let h = 100.0 * k;
            let x0 = -c.w / 2.0 + 0.07 * c.w;
            let y0 = c.h / 2.0 - 0.2 * c.h;
            let d = c.dur;
            vec![
                shape("Pill", json!([{"type": "group", "position": [w / 2.0, 0], "scale": in_out(0.0, 0.45, d, 0.35, json!(0), json!(1), "overshoot"), "items": [
                    {"type": "rect", "size": [w, h], "roundness": h / 2.0},
                    {"type": "fill", "colour": c.colour("pill")},
                    {"type": "group", "items": [{"type": "circle", "radius": 26.0 * k, "position": [-w / 2.0 + h / 2.0, 0]}, {"type": "fill", "colour": c.colour("dot")}]},
                ]}]), x0, y0, 0.0, d),
                text("Name", name, ns, c.colour("text"), 800, x0 + h, y0 - 14.0 * k, Anchor::Left, 0.25, d, reveal(RevealKind::Rise, TextUnit::Word, 0.45, RevealKind::Fade)),
                text("Handle", handle, hs, c.colour("text"), 500, x0 + h, y0 + 22.0 * k, Anchor::Left, 0.35, d, reveal(RevealKind::Fade, TextUnit::Character, 0.5, RevealKind::Fade)),
            ]
        },
    });
}

fn titles(v: &mut Vec<Template>) {
    v.push(Template {
        id: "title.kinetic",
        name: "Kinetic Pop",
        category: Category::Title,
        description: "A big title whose words pop in with overshoot, a highlight sweeping across the letters, a subtitle rising",
        tags: "title kinetic pop bold energetic youtube intro highlight",
        duration: 4.0,
        controls: vec![Control::text("title", "Title", "MAKE IT MOVE"), Control::text("subtitle", "Subtitle", "Episode one"), Control::colour("text", "Text", "#ffffff"), Control::colour("highlight", "Highlight", "#ffd166")],
        build: |c| {
            let k = c.k();
            let d = c.dur;
            let mut m = reveal(RevealKind::Pop, TextUnit::Word, 0.7, RevealKind::Zoom);
            if let Some(mut a) = animator::preset("highlight_sweep", TextUnit::Character, 0.8, 1.0) {
                a.colour = Some(studio_text::Rgba::hex(&c.colour("highlight")));
                m.animators.push(a);
            }
            vec![
                text("Title", c.text("title"), 150.0 * k, c.colour("text"), 900, 0.0, -30.0 * k, Anchor::Centre, 0.0, d, m),
                spaced(text("Subtitle", c.text("subtitle"), 40.0 * k, c.colour("text"), 500, 0.0, 90.0 * k, Anchor::Centre, 0.6, d, reveal(RevealKind::Rise, TextUnit::Character, 0.6, RevealKind::Fade)), 20.0),
            ]
        },
    });
    v.push(Template {
        id: "title.focus",
        name: "Focus Pull",
        category: Category::Title,
        description: "Letters come into focus one by one from a soft blur, with a thin rule growing under them",
        tags: "title cinematic blur focus elegant film trailer",
        duration: 5.0,
        controls: vec![Control::text("title", "Title", "The Quiet Hours"), Control::text("subtitle", "Subtitle", "A film by Noor Haddad"), Control::colour("text", "Text", "#f4efe6"), Control::colour("rule", "Rule", "#c9a96e")],
        build: |c| {
            let k = c.k();
            let d = c.dur;
            let mut m = TextMotion { exit: Some(Reveal::new(RevealKind::Fade, TextUnit::Line)), exit_duration: 0.6, ..Default::default() };
            if let Some(a) = animator::preset("focus_in", TextUnit::Character, 0.0, 1.6) {
                m.animators.push(a);
            }
            let tw = c.measure(&c.text("title"), FONT, 300, 110.0 * k, 12.0)[0];
            vec![
                spaced(text("Title", c.text("title"), 110.0 * k, c.colour("text"), 300, 0.0, -20.0 * k, Anchor::Centre, 0.0, d, m), 12.0),
                shape("Rule", json!([{"type": "line", "from": [-tw * 0.3, 0], "to": [tw * 0.3, 0]}, {"type": "trim", "start": k2(0.9, json!(0.5), 1.8, json!(0), "out"), "end": k2(0.9, json!(0.5), 1.8, json!(1), "out")}, {"type": "stroke", "colour": c.colour("rule"), "width": 2.0 * k.max(0.5), "opacity": in_out(0.9, 1.0, d, 0.5, json!(0), json!(1), "linear")}]), 0.0, 60.0 * k, 0.0, d),
                spaced(text("Subtitle", c.text("subtitle"), 30.0 * k, c.colour("text"), 400, 0.0, 110.0 * k, Anchor::Centre, 1.4, d, reveal(RevealKind::Fade, TextUnit::Word, 0.8, RevealKind::Fade)), 10.0),
            ]
        },
    });
    v.push(Template {
        id: "title.split",
        name: "Split Reveal",
        category: Category::Title,
        description: "Two bars slide apart to reveal a title between them, which tracks out slightly as it holds",
        tags: "title reveal bars split dramatic sport promo",
        duration: 4.0,
        controls: vec![Control::text("title", "Title", "GAME DAY"), Control::colour("bars", "Bars", "#ff5c7a"), Control::colour("text", "Text", "#ffffff")],
        build: |c| {
            let k = c.k();
            let d = c.dur;
            let size = 140.0 * k;
            let tw = c.measure(&c.text("title"), FONT, 900, size, 6.0)[0] + 120.0 * k;
            let bar = |dir: f64| json!({"type": "group", "position": in_out(0.1, 0.6, d, 0.4, json!([0, 0]), json!([0, dir * size * 0.62]), "in_out"), "items": [{"type": "rect", "size": [tw, 18.0 * k]}, {"type": "fill", "colour": c.colour("bars")}]});
            let mut m = reveal(RevealKind::Fade, TextUnit::Line, 0.3, RevealKind::Fade);
            m.animators.push(animator::Animator { unit: TextUnit::Character, range: Some(animator::Range { amount: animator::Keyed::Keys(vec![vec![json!(0.3), json!(0.0)], vec![json!(d), json!(1.0), json!("linear")]]), ..Default::default() }), wiggle: None, position: None, rotation: None, scale: None, opacity: None, colour: None, blur: None, tracking: Some(6.0) });
            vec![
                spaced(text("Title", c.text("title"), size, c.colour("text"), 900, 0.0, 0.0, Anchor::Centre, 0.25, d, m), 6.0),
                shape("Bars", json!([bar(-1.0), bar(1.0)]), 0.0, 0.0, 0.0, d),
            ]
        },
    });
    v.push(Template {
        id: "title.badge",
        name: "Seal Badge",
        category: Category::Title,
        description: "A scalloped seal pops in behind a short word, turning slowly",
        tags: "title badge seal sale new sticker stamp label",
        duration: 4.0,
        controls: vec![Control::text("word", "Word", "NEW"), Control::text("small", "Small text", "this week"), Control::colour("badge", "Badge", "#ffb020"), Control::colour("text", "Text", "#101418")],
        build: |c| {
            let k = c.k();
            let d = c.dur;
            let o = studio_vector::presets::Options { colour: c.colour("badge"), accent: c.colour("badge"), size: 420.0 * k, width: 14.0 * k, speed: 0.6 };
            let badge = studio_vector::presets::make("badge", &o).unwrap_or(json!({"items": []}));
            vec![
                Layer::Shape(ShapeLayer { name: "Badge".into(), doc: json!({"items": [{"type": "group", "opacity": in_out(0.0, 0.01, d, 0.4, json!(0), json!(1), "linear"), "rotation": "=t * 8", "items": badge["items"].clone()}]}), x: 0.0, y: 0.0, start: 0.0, duration: d }),
                text("Word", c.text("word"), 110.0 * k, c.colour("text"), 900, 0.0, -20.0 * k, Anchor::Centre, 0.3, d, reveal(RevealKind::Pop, TextUnit::Character, 0.5, RevealKind::Zoom)),
                text("Small", c.text("small"), 30.0 * k, c.colour("text"), 600, 0.0, 60.0 * k, Anchor::Centre, 0.5, d, reveal(RevealKind::Fade, TextUnit::Word, 0.4, RevealKind::Fade)),
            ]
        },
    });
    v.push(Template {
        id: "title.circle_text",
        name: "Round About",
        category: Category::Title,
        description: "Words running round a slowly turning circle around a centre word",
        tags: "title circle round path text spinning stamp logo",
        duration: 6.0,
        controls: vec![Control::text("ring", "Ring text", "STUDIO • MADE BY HAND • STUDIO • MADE BY HAND • "), Control::text("centre", "Centre", "2026"), Control::colour("text", "Text", "#ffffff")],
        build: |c| {
            let k = c.k();
            let d = c.dur;
            vec![
                text("Centre", c.text("centre"), 80.0 * k, c.colour("text"), 800, 0.0, 0.0, Anchor::Centre, 0.2, d, reveal(RevealKind::Pop, TextUnit::Character, 0.5, RevealKind::Fade)),
                on_path(spaced(text("Ring", c.text("ring"), 34.0 * k, c.colour("text"), 700, 0.0, 0.0, Anchor::Centre, 0.0, d, reveal(RevealKind::Fade, TextUnit::Character, 1.0, RevealKind::Fade)), 12.0), json!({"circle": 230.0 * k}), json!({"keys": [[0, 0.0], [d, -0.12, "linear"]]})),
            ]
        },
    });
}

fn callouts(v: &mut Vec<Template>) {
    v.push(Template {
        id: "callout.pointer",
        name: "Pointer",
        category: Category::Callout,
        description: "A pulsing dot on the point, a leader line drawing out from it, and a label box at its end",
        tags: "callout pointer label annotation leader line tag product feature",
        duration: 5.0,
        controls: vec![Control::point("point", "Point", [-200.0, 80.0]), Control::text("label", "Label", "Hand-stitched seams"), Control::choice("side", "Side", &["right", "left"], "right"), Control::colour("accent", "Accent", "#21c7a8"), Control::colour("text", "Text", "#ffffff")],
        build: |c| {
            let k = c.k();
            let d = c.dur;
            let p = c.point("point");
            let dir = if c.text("side") == "left" { -1.0 } else { 1.0 };
            let size = 34.0 * k;
            let tw = c.measure(&c.text("label"), FONT, 700, size, 0.0)[0] + 50.0 * k;
            let elbow = [p[0] + dir * 120.0 * k, p[1] - 120.0 * k];
            let end = [elbow[0] + dir * 80.0 * k, elbow[1]];
            let box_c = [end[0] + dir * tw / 2.0, end[1]];
            vec![
                shape("Pointer", json!([
                    {"type": "group", "items": [{"type": "circle", "radius": format!("={} * fract(t / 1.4)", 40.0 * k)}, {"type": "stroke", "colour": c.colour("accent"), "width": 3.0 * k, "opacity": "=1 - fract(t / 1.4)"}]},
                    {"type": "group", "scale": k2(0.0, json!(0), 0.3, json!(1), "overshoot"), "items": [{"type": "circle", "radius": 10.0 * k}, {"type": "fill", "colour": c.colour("accent")}]},
                    {"type": "group", "items": [{"type": "path", "points": [[0, 0], [elbow[0] - p[0], elbow[1] - p[1]], [end[0] - p[0], end[1] - p[1]]]}, {"type": "trim", "end": in_out(0.2, 0.7, d, 0.4, json!(0), json!(1), "out")}, {"type": "stroke", "colour": c.colour("accent"), "width": 3.0 * k, "join": "round"}]},
                    {"type": "group", "position": [box_c[0] - p[0], box_c[1] - p[1]], "scale": in_out(0.6, 0.95, d, 0.35, json!([0, 1]), json!([1, 1]), "out"), "items": [{"type": "rect", "size": [tw, size * 1.8], "roundness": 8.0 * k}, {"type": "fill", "colour": c.colour("accent")}]},
                ]), p[0], p[1], 0.0, d),
                text("Label", c.text("label"), size, c.colour("text"), 700, box_c[0], box_c[1], Anchor::Centre, 0.85, d, reveal(RevealKind::Fade, TextUnit::Character, 0.4, RevealKind::Fade)),
            ]
        },
    });
    v.push(Template {
        id: "callout.circle",
        name: "Ring It",
        category: Category::Callout,
        description: "A hand-drawn circle loops round the point and a note appears beside it",
        tags: "callout circle highlight hand drawn marker note look here",
        duration: 4.0,
        controls: vec![Control::point("point", "Point", [0.0, 0.0]), Control::number("size", "Circle size", 260.0, 40.0, 1200.0), Control::text("note", "Note", "look here!"), Control::colour("ink", "Ink", "#ff5c7a")],
        build: |c| {
            let k = c.k();
            let d = c.dur;
            let p = c.point("point");
            let s = c.num("size") * k;
            let o = studio_vector::presets::Options { colour: c.colour("ink"), accent: c.colour("ink"), size: s, width: 9.0 * k, speed: 0.7 };
            let ring = studio_vector::presets::make("circle_draw", &o).unwrap_or(json!({"items": []}));
            vec![
                Layer::Shape(ShapeLayer { name: "Ring".into(), doc: json!({"items": [{"type": "group", "opacity": in_out(0.0, 0.01, d, 0.4, json!(0), json!(1), "linear"), "items": ring["items"].clone()}]}), x: p[0], y: p[1], start: 0.0, duration: d }),
                text("Note", c.text("note"), 44.0 * k, c.colour("ink"), 800, p[0] + s * 0.55, p[1] - s * 0.55, Anchor::Left, 0.6, d, reveal(RevealKind::Pop, TextUnit::Word, 0.4, RevealKind::Fade)),
            ]
        },
    });
    v.push(Template {
        id: "callout.arrow",
        name: "Arrow Note",
        category: Category::Callout,
        description: "A curved arrow draws itself towards the point from a short note",
        tags: "callout arrow note annotation point hand drawn",
        duration: 4.0,
        controls: vec![Control::point("point", "Point", [200.0, 60.0]), Control::text("note", "Note", "new!"), Control::colour("ink", "Ink", "#ffffff")],
        build: |c| {
            let k = c.k();
            let d = c.dur;
            let p = c.point("point");
            let start = [p[0] - 260.0 * k, p[1] - 160.0 * k];
            let mid = [p[0] - 200.0 * k, p[1] - 10.0 * k];
            let tip = [p[0] - 30.0 * k, p[1] - 10.0 * k];
            vec![
                shape("Arrow", json!([
                    {"type": "path", "points": [start, mid, tip], "smooth": true},
                    {"type": "path", "points": [[tip[0] - 26.0 * k, tip[1] - 22.0 * k], tip, [tip[0] - 30.0 * k, tip[1] + 18.0 * k]]},
                    {"type": "trim", "end": in_out(0.2, 0.9, d, 0.4, json!(0), json!(1), "out"), "mode": "sequence"},
                    {"type": "stroke", "colour": c.colour("ink"), "width": 7.0 * k, "cap": "round", "join": "round"},
                ]), 0.0, 0.0, 0.0, d),
                text("Note", c.text("note"), 50.0 * k, c.colour("ink"), 800, start[0], start[1] - 40.0 * k, Anchor::Centre, 0.0, d, reveal(RevealKind::Pop, TextUnit::Character, 0.4, RevealKind::Fade)),
            ]
        },
    });
}

fn social(v: &mut Vec<Template>) {
    v.push(Template {
        id: "social.subscribe",
        name: "Subscribe Button",
        category: Category::Social,
        description: "A button pops in, a cursor taps it, it turns grey and says subscribed, with a little bell",
        tags: "social subscribe button youtube cta call to action click bell follow",
        duration: 4.0,
        controls: vec![Control::text("label", "Label", "SUBSCRIBE"), Control::text("done", "After the tap", "SUBSCRIBED"), Control::colour("button", "Button", "#e62117"), Control::colour("text", "Text", "#ffffff")],
        build: |c| {
            let k = c.k();
            let d = c.dur;
            let size = 44.0 * k;
            let w = c.measure(&c.text("label"), FONT, 800, size, 4.0)[0].max(c.measure(&c.text("done"), FONT, 800, size, 4.0)[0]) + 110.0 * k;
            let h = 100.0 * k;
            let y = c.h / 2.0 - 0.2 * c.h;
            let tap = 1.4;
            vec![
                shape("Button", json!([
                    {"type": "group", "scale": json!({"keys": [[0, 0], [0.4, 1, "overshoot"], [tap - 0.05, 1], [tap + 0.05, 0.92], [tap + 0.25, 1, "overshoot"], [d - 0.35, 1], [d, 0, "in"]]}), "items": [
                        {"type": "rect", "size": [w, h], "roundness": 14.0 * k},
                        {"type": "fill", "colour": json!({"keys": [[tap, c.colour("button")], [tap + 0.15, "#606060", "linear"]]})},
                    ]},
                    {"type": "group", "position": json!({"keys": [[0.5, [w * 0.7, h * 1.4]], [tap - 0.05, [w * 0.25, h * 0.15], "in_out"], [tap + 0.6, [w * 0.25, h * 0.15]], [tap + 1.1, [w * 0.9, h * 1.2], "in_out"]]}), "opacity": json!({"keys": [[0.45, 0], [0.6, 1], [tap + 0.9, 1], [tap + 1.1, 0]]}), "items": [
                        {"type": "path", "points": [[0, 0], [0, 46.0 * k], [12.0 * k, 35.0 * k], [22.0 * k, 56.0 * k], [30.0 * k, 52.0 * k], [20.0 * k, 32.0 * k], [36.0 * k, 32.0 * k]], "closed": true},
                        {"type": "fill", "colour": "#ffffff"},
                        {"type": "stroke", "colour": "#101418", "width": 3.0 * k},
                    ]},
                ]), 0.0, y, 0.0, d),
                spaced(text("Label", c.text("label"), size, c.colour("text"), 800, 0.0, y, Anchor::Centre, 0.25, tap + 0.1, TextMotion { reveal: Some(Reveal::new(RevealKind::Fade, TextUnit::Line)), reveal_duration: 0.2, ..Default::default() }), 4.0),
                spaced(text("Done", c.text("done"), size, c.colour("text"), 800, 0.0, y, Anchor::Centre, tap + 0.1, d, reveal(RevealKind::Pop, TextUnit::Line, 0.3, RevealKind::Fade)), 4.0),
            ]
        },
    });
    v.push(Template {
        id: "social.handle",
        name: "Handle Slide",
        category: Category::Social,
        description: "A round icon spins in and an @handle slides out of it",
        tags: "social handle username instagram tiktok follow icon",
        duration: 4.0,
        controls: vec![Control::text("handle", "Handle", "@studio.makes"), Control::text("icon", "Icon letter", "S"), Control::colour("icon_colour", "Icon", "#c86bff"), Control::colour("text", "Text", "#ffffff")],
        build: |c| {
            let k = c.k();
            let d = c.dur;
            let r = 54.0 * k;
            let size = 46.0 * k;
            let tw = c.measure(&c.text("handle"), FONT, 700, size, 0.0)[0];
            let x0 = -(tw + r * 2.4) / 2.0;
            let y = -c.h / 2.0 + 0.18 * c.h;
            vec![
                shape("Icon", json!([{"type": "group", "rotation": in_out(0.0, 0.5, d, 0.35, json!(-180), json!(0), "out"), "scale": in_out(0.0, 0.5, d, 0.35, json!(0), json!(1), "overshoot"), "items": [{"type": "circle", "radius": r}, {"type": "fill", "gradient": {"type": "linear", "from": [-r, -r], "to": [r, r], "stops": [c.colour("icon_colour"), "#ffb020"]}}]}]), x0 + r, y, 0.0, d),
                text("Icon letter", c.text("icon"), 56.0 * k, "#ffffff".into(), 900, x0 + r, y, Anchor::Centre, 0.3, d, reveal(RevealKind::Pop, TextUnit::Line, 0.3, RevealKind::Fade)),
                text("Handle", c.text("handle"), size, c.colour("text"), 700, x0 + r * 2.4, y, Anchor::Left, 0.45, d, reveal(RevealKind::SlideLeft, TextUnit::Character, 0.6, RevealKind::Fade)),
            ]
        },
    });
}

fn transitions(v: &mut Vec<Template>) {
    v.push(Template {
        id: "transition.diagonal",
        name: "Diagonal Sweep",
        category: Category::Transition,
        description: "Three angled colour bands sweep across and cover the frame at the middle, then clear: put it over a cut",
        tags: "transition wipe diagonal sweep bands colour cover cut",
        duration: 1.2,
        controls: vec![Control::colour("a", "Colour 1", "#7c5cff"), Control::colour("b", "Colour 2", "#21c7a8"), Control::colour("c", "Colour 3", "#101418")],
        build: |c| {
            let d = c.dur;
            let span = (c.w * c.w + c.h * c.h).sqrt() * 1.6;
            let band = |col: String, delay: f64| json!({"type": "group", "rotation": -24, "items": [
                {"type": "rect", "size": [span, span], "position": {"keys": [[delay, [-span * 1.05, 0]], [d * 0.5, [0, 0], "in_out"], [d - 0.1 + delay * 0.0, [span * 1.05, 0], "in_out"]]}},
                {"type": "fill", "colour": col},
            ]});
            vec![shape("Bands", json!([band(c.colour("a"), 0.0), band(c.colour("b"), d * 0.06), band(c.colour("c"), d * 0.12)]), 0.0, 0.0, 0.0, d)]
        },
    });
    v.push(Template {
        id: "transition.circle",
        name: "Iris Burst",
        category: Category::Transition,
        description: "A circle bursts from the centre to fill the frame, then opens out from the middle like an iris",
        tags: "transition circle iris burst zoom cover cut",
        duration: 1.0,
        controls: vec![Control::colour("colour", "Colour", "#ffb020"), Control::point("centre", "Centre", [0.0, 0.0])],
        build: |c| {
            let d = c.dur;
            let r = (c.w * c.w + c.h * c.h).sqrt() * 0.6 + c.point("centre")[0].abs().max(c.point("centre")[1].abs());
            vec![shape("Iris", json!([
                {"type": "circle", "radius": json!({"keys": [[0, 0], [d * 0.5, r, "in"]]})},
                {"type": "circle", "radius": json!({"keys": [[d * 0.5, 0], [d, r, "out"]]})},
                {"type": "merge", "mode": "subtract"},
                {"type": "fill", "colour": c.colour("colour")},
            ]), c.point("centre")[0], c.point("centre")[1], 0.0, d)]
        },
    });
    v.push(Template {
        id: "transition.bars",
        name: "Shutter Bars",
        category: Category::Transition,
        description: "Horizontal bars close in from alternate sides over the cut and slide away",
        tags: "transition bars shutter stripes slide cover cut",
        duration: 1.0,
        controls: vec![Control::colour("colour", "Colour", "#101418"), Control::number("bars", "Bars", 6.0, 2.0, 16.0)],
        build: |c| {
            let d = c.dur;
            let n = c.num("bars").round().clamp(2.0, 16.0) as usize;
            let bh = c.h / n as f64;
            let items: Vec<Value> = (0..n)
                .map(|i| {
                    let side = if i % 2 == 0 { -1.0 } else { 1.0 };
                    let delay = i as f64 * d * 0.03;
                    json!({"type": "group", "items": [
                        {"type": "rect", "size": [c.w + 4.0, bh + 2.0], "position": {"keys": [[delay, [side * c.w, -c.h / 2.0 + bh * (i as f64 + 0.5)]], [d * 0.5, [0, -c.h / 2.0 + bh * (i as f64 + 0.5)], "out"], [d * 0.55 + delay, [0, -c.h / 2.0 + bh * (i as f64 + 0.5)]], [d, [-side * c.w, -c.h / 2.0 + bh * (i as f64 + 0.5)], "in"]]}},
                        {"type": "fill", "colour": c.colour("colour")},
                    ]})
                })
                .collect();
            vec![shape("Bars", Value::Array(items), 0.0, 0.0, 0.0, d)]
        },
    });
}

fn infographics(v: &mut Vec<Template>) {
    v.push(Template {
        id: "info.stat",
        name: "Big Number",
        category: Category::Infographic,
        description: "A number counting up inside a ring that fills to a share, with a label under it",
        tags: "infographic stat number counter percent ring kpi progress",
        duration: 5.0,
        controls: vec![Control::number("value", "Value", 87.0, -1e12, 1e12), Control::text("suffix", "After the number", "%"), Control::text("prefix", "Before the number", ""), Control::number("share", "Ring fill (0-1)", 0.87, 0.0, 1.0), Control::text("label", "Label", "of viewers stay to the end"), Control::colour("ring", "Ring", "#21c7a8"), Control::colour("text", "Text", "#ffffff")],
        build: |c| {
            let k = c.k();
            let d = c.dur;
            let r = 170.0 * k;
            let share = c.num("share").clamp(0.0, 1.0);
            vec![
                shape("Ring", json!([{"type": "group", "opacity": in_out(0.0, 0.2, d, 0.4, json!(0), json!(1), "linear"), "items": [
                    {"type": "circle", "radius": r}, {"type": "stroke", "colour": c.colour("ring"), "width": 22.0 * k, "opacity": 0.2},
                    {"type": "arc", "radius": r, "start": 0, "end": k2(0.2, json!(0.01), 1.6, json!(360.0 * share), "out")},
                    {"type": "stroke", "colour": c.colour("ring"), "width": 22.0 * k, "cap": "round"},
                ]}]), 0.0, -40.0 * k, 0.0, d),
                Layer::Text(TextLayer { name: "Number".into(), text: format!("{}", c.num("value")), font: FONT.into(), weight: 800, italic: false, size: 96.0 * k, colour: c.colour("text"), letter_spacing: 0.0, anchor: Anchor::Centre, x: 0.0, y: -40.0 * k, start: 0.2, duration: d - 0.2, motion: Some(reveal(RevealKind::Fade, TextUnit::Line, 0.2, RevealKind::Fade)), counter: Some(Counter { from: 0.0, to: c.num("value"), start: 0.0, duration: 1.4, decimals: if c.num("value").fract().abs() > 1e-9 { 1 } else { 0 }, prefix: c.text("prefix"), suffix: c.text("suffix"), separator: true }), path: None }),
                text("Label", c.text("label"), 34.0 * k, c.colour("text"), 500, 0.0, r + 40.0 * k, Anchor::Centre, 0.9, d, reveal(RevealKind::Rise, TextUnit::Word, 0.6, RevealKind::Fade)),
            ]
        },
    });
    v.push(Template {
        id: "info.progress",
        name: "Progress Bar",
        category: Category::Infographic,
        description: "A rounded bar filling to a share with its percentage counting along on top",
        tags: "infographic progress bar percent loading goal fill",
        duration: 4.0,
        controls: vec![Control::number("share", "Share (0-1)", 0.64, 0.0, 1.0), Control::text("label", "Label", "Funded"), Control::colour("fill", "Fill", "#7c5cff"), Control::colour("track", "Track", "#ffffff"), Control::colour("text", "Text", "#ffffff")],
        build: |c| {
            let k = c.k();
            let d = c.dur;
            let w = 900.0 * k;
            let h = 34.0 * k;
            let s = c.num("share").clamp(0.0, 1.0);
            let x0 = -w / 2.0;
            vec![
                shape("Bar", json!([{"type": "group", "opacity": in_out(0.0, 0.25, d, 0.4, json!(0), json!(1), "linear"), "items": [
                    {"type": "group", "items": [{"type": "rect", "size": [w, h], "roundness": h / 2.0}, {"type": "fill", "colour": c.colour("track"), "opacity": 0.18}]},
                    {"type": "group", "position": [x0, 0], "scale": k2(0.3, json!([0.001, 1]), 1.7, json!([s.max(0.001), 1]), "out"), "items": [{"type": "rect", "position": [w / 2.0, 0], "size": [w, h], "roundness": h / 2.0}, {"type": "fill", "gradient": {"type": "linear", "from": [0, 0], "to": [w, 0], "stops": [c.colour("fill"), "#21c7a8"]}}]},
                ]}]), 0.0, 0.0, 0.0, d),
                text("Label", c.text("label"), 36.0 * k, c.colour("text"), 700, x0, -h * 1.6, Anchor::Left, 0.2, d, reveal(RevealKind::Rise, TextUnit::Word, 0.5, RevealKind::Fade)),
                Layer::Text(TextLayer { name: "Percent".into(), text: format!("{:.0}%", s * 100.0), font: FONT.into(), weight: 800, italic: false, size: 36.0 * k, colour: c.colour("text"), letter_spacing: 0.0, anchor: Anchor::Right, x: -x0, y: -h * 1.6, start: 0.3, duration: d - 0.3, motion: Some(reveal(RevealKind::Fade, TextUnit::Line, 0.2, RevealKind::Fade)), counter: Some(Counter { from: 0.0, to: s * 100.0, start: 0.0, duration: 1.4, decimals: 0, prefix: String::new(), suffix: "%".into(), separator: false }), path: None }),
            ]
        },
    });
    v.push(Template {
        id: "info.timeline",
        name: "Timeline",
        category: Category::Infographic,
        description: "A line drawing across with milestones popping on in turn, each with its date and words",
        tags: "infographic timeline history milestones dates steps roadmap",
        duration: 6.0,
        controls: vec![Control::data("events", "Events (date, words)", json!([["2019", "First sketch"], ["2021", "Prototype"], ["2023", "Launch"], ["2026", "One million"]])), Control::colour("line", "Line", "#ffffff"), Control::colour("accent", "Dots", "#ffb020"), Control::colour("text", "Text", "#ffffff")],
        build: |c| {
            let k = c.k();
            let d = c.dur;
            let rows: Vec<(String, String)> = match c.values.get("events").cloned().unwrap_or_else(|| json!([["2019", "First sketch"], ["2021", "Prototype"], ["2023", "Launch"], ["2026", "One million"]])) {
                Value::Array(a) => a.iter().map(|r| match r {
                    Value::Array(x) => (x.first().map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string())).unwrap_or_default(), x.get(1).and_then(Value::as_str).unwrap_or("").to_owned()),
                    Value::Object(o) => (o.get("date").or(o.get("label")).and_then(Value::as_str).unwrap_or("").to_owned(), o.get("text").or(o.get("words")).and_then(Value::as_str).unwrap_or("").to_owned()),
                    other => (other.as_str().unwrap_or("").to_owned(), String::new()),
                }).collect(),
                _ => Vec::new(),
            };
            let n = rows.len().max(1);
            let w = c.w * 0.78;
            let x0 = -w / 2.0;
            let build = 2.0;
            let mut items = vec![json!({"type": "group", "items": [{"type": "line", "from": [x0, 0], "to": [-x0, 0]}, {"type": "trim", "end": k2(0.1, json!(0), 0.1 + build, json!(1), "in_out")}, {"type": "stroke", "colour": c.colour("line"), "width": 4.0 * k, "opacity": 0.6}]})];
            let mut layers = Vec::new();
            for (i, (date, words)) in rows.iter().enumerate() {
                let x = if n == 1 { 0.0 } else { x0 + w * i as f64 / (n - 1) as f64 };
                let at = 0.1 + build * if n == 1 { 0.5 } else { i as f64 / (n - 1) as f64 };
                items.push(json!({"type": "group", "position": [x, 0], "scale": k2(at, json!(0), at + 0.35, json!(1), "overshoot"), "items": [{"type": "circle", "radius": 16.0 * k}, {"type": "fill", "colour": c.colour("accent")}, {"type": "stroke", "colour": c.colour("line"), "width": 4.0 * k}]}));
                let up = if i % 2 == 0 { -1.0 } else { 1.0 };
                layers.push(text(&format!("{date} date"), date.clone(), 44.0 * k, c.colour("accent"), 800, x, up * 60.0 * k, Anchor::Centre, at.min(d - 0.4), d, reveal(RevealKind::Rise, TextUnit::Character, 0.4, RevealKind::Fade)));
                if !words.is_empty() {
                    layers.push(text(&format!("{date} words"), words.clone(), 28.0 * k, c.colour("text"), 500, x, up * 110.0 * k, Anchor::Centre, (at + 0.15).min(d - 0.4), d, reveal(RevealKind::Fade, TextUnit::Word, 0.4, RevealKind::Fade)));
                }
            }
            let mut out = vec![shape("Line", json!([{"type": "group", "opacity": in_out(0.0, 0.01, d, 0.4, json!(0), json!(1), "linear"), "items": items}]), 0.0, 0.0, 0.0, d)];
            out.extend(layers);
            out
        },
    });
    v.push(Template {
        id: "info.list",
        name: "Checklist",
        category: Category::Infographic,
        description: "Points appearing one after another, each ticked off as it lands",
        tags: "infographic list bullet points checklist steps tips agenda",
        duration: 6.0,
        controls: vec![Control::text("items", "Points (a line each)", "Plan the shots\nLight the scene\nRoll camera"), Control::text("heading", "Heading", "Today"), Control::colour("tick", "Ticks", "#21c7a8"), Control::colour("text", "Text", "#ffffff")],
        build: |c| {
            let k = c.k();
            let d = c.dur;
            let lines: Vec<String> = c.text("items").lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect();
            let gap = 80.0 * k;
            let top = -gap * (lines.len() as f64) / 2.0;
            let x0 = -c.w * 0.25;
            let mut out = vec![text("Heading", c.text("heading"), 60.0 * k, c.colour("text"), 800, x0, top - gap * 0.8, Anchor::Left, 0.0, d, reveal(RevealKind::Rise, TextUnit::Word, 0.5, RevealKind::Fade))];
            let mut items = Vec::new();
            for (i, l) in lines.iter().enumerate() {
                let at = 0.4 + 0.45 * i as f64;
                let y = top + gap * (i as f64 + 0.5);
                items.push(json!({"type": "group", "position": [x0 + 18.0 * k, y], "items": [
                    {"type": "group", "scale": k2(at, json!(0), at + 0.3, json!(1), "overshoot"), "items": [{"type": "circle", "radius": 20.0 * k}, {"type": "fill", "colour": c.colour("tick")}]},
                    {"type": "path", "points": [[-9.0 * k, 0], [-2.0 * k, 8.0 * k], [10.0 * k, -8.0 * k]]},
                    {"type": "trim", "end": k2(at + 0.2, json!(0), at + 0.5, json!(1), "out")},
                    {"type": "stroke", "colour": "#ffffff", "width": 5.0 * k, "cap": "round", "join": "round"},
                ]}));
                out.push(text(&format!("Point {}", i + 1), l.clone(), 40.0 * k, c.colour("text"), 600, x0 + 60.0 * k, y, Anchor::Left, at.min(d - 0.4), d, reveal(RevealKind::SlideLeft, TextUnit::Word, 0.45, RevealKind::Fade)));
            }
            out.insert(0, shape("Ticks", json!([{"type": "group", "opacity": in_out(0.0, 0.01, d, 0.4, json!(0), json!(1), "linear"), "items": items}]), 0.0, 0.0, 0.0, d));
            out
        },
    });
}

/// A chart look from the common chart controls.
fn look(c: &Ctx) -> Look {
    let k = c.k();
    let mut l = Look { size: [c.w * 0.62, c.h * 0.5], centre: [0.0, c.h * 0.06], duration: c.dur, text_size: 30.0 * k, ..Default::default() };
    let colour = c.colour("colour");
    if !colour.is_empty() {
        l.palette.retain(|p| !p.eq_ignore_ascii_case(&colour));
        l.palette.insert(0, colour);
    }
    l.text = c.colour("text");
    l.prefix = c.text("prefix");
    l.suffix = c.text("suffix");
    l.decimals = c.num("decimals").round().clamp(0.0, 6.0) as u32;
    l
}

fn chart_controls(data: Value, extra: Vec<Control>) -> Vec<Control> {
    let mut v = vec![Control::text("title", "Title", "Weekly visits"), Control::data("data", "Data (label, value)", data), Control::colour("colour", "Main colour", "#7c5cff"), Control::colour("text", "Text", "#ffffff"), Control::text("prefix", "Before values", ""), Control::text("suffix", "After values", ""), Control::number("decimals", "Decimals", 0.0, 0.0, 6.0)];
    v.extend(extra);
    v
}

fn with_title(c: &Ctx, mut layers: Vec<Layer>) -> Vec<Layer> {
    let k = c.k();
    let t = c.text("title");
    if !t.is_empty() {
        layers.push(text("Chart title", t, 48.0 * k, c.colour("text"), 800, 0.0, -c.h * 0.36, Anchor::Centre, 0.0, c.dur, reveal(RevealKind::Rise, TextUnit::Word, 0.5, RevealKind::Fade)));
    }
    layers
}

fn chart_templates(v: &mut Vec<Template>) {
    let week = json!([["Mon", 120], ["Tue", 180], ["Wed", 150], ["Thu", 240], ["Fri", 310], ["Sat", 280], ["Sun", 200]]);
    v.push(Template { id: "chart.bar", name: "Bar Chart", category: Category::Chart, description: "Bars growing one after another with their values counting up and labels under them", tags: "chart bar column graph data statistics compare", duration: 6.0, controls: chart_controls(week.clone(), vec![]), build: |c| with_title(c, charts::bar(&c.data("data"), &look(c))) });
    v.push(Template { id: "chart.line", name: "Line Chart", category: Category::Chart, description: "A line drawing itself through the values, points popping on as it reaches them, over a soft area", tags: "chart line graph trend growth data over time", duration: 6.0, controls: chart_controls(week, vec![]), build: |c| with_title(c, charts::line(&c.data("data"), &look(c))) });
    v.push(Template {
        id: "chart.pie",
        name: "Donut Chart",
        category: Category::Chart,
        description: "A donut sweeping round slice by slice with a legend of shares and the total counting in the middle",
        tags: "chart pie donut share percent breakdown legend",
        duration: 6.0,
        controls: chart_controls(json!([["Video", 46], ["Photo", 28], ["Audio", 16], ["Text", 10]]), vec![Control::number("hole", "Hole (0 = pie)", 0.58, 0.0, 0.9), Control::text("total_label", "Under the total", "total")]),
        build: |c| with_title(c, charts::pie(&c.data("data"), &look(c), c.num("hole"), &c.text("total_label"))),
    });
    v.push(Template {
        id: "chart.map",
        name: "Route Map",
        category: Category::Chart,
        description: "Places pinned on a latitude and longitude grid, joined in order by travelling dashed arcs",
        tags: "chart map route travel places cities pins locations journey",
        duration: 7.0,
        controls: chart_controls(json!([["Lisbon", -9.14, 38.72, 0], ["Madrid", -3.7, 40.42, 0], ["Paris", 2.35, 48.86, 0], ["Berlin", 13.4, 52.52, 0]]), vec![Control::toggle("route", "Join in order", true)]),
        build: |c| with_title(c, charts::map(&c.data("data"), &look(c), None, c.flag("route"))),
    });
}

fn end_cards(v: &mut Vec<Template>) {
    v.push(Template {
        id: "end.thanks",
        name: "Thanks For Watching",
        category: Category::EndCard,
        description: "A closing line with two framed boxes for next videos and a follow prompt",
        tags: "end card outro thanks watching next video subscribe screen",
        duration: 8.0,
        controls: vec![Control::text("line", "Line", "Thanks for watching"), Control::text("prompt", "Prompt", "Watch next"), Control::colour("frame", "Frames", "#ffffff"), Control::colour("text", "Text", "#ffffff")],
        build: |c| {
            let k = c.k();
            let d = c.dur;
            let (bw, bh) = (560.0 * k, 315.0 * k);
            let frame = |x: f64, delay: f64| json!({"type": "group", "position": [x, 70.0 * k], "items": [
                {"type": "rect", "size": [bw, bh], "roundness": 16.0 * k},
                {"type": "trim", "end": k2(delay, json!(0), delay + 0.8, json!(1), "out")},
                {"type": "stroke", "colour": c.colour("frame"), "width": 5.0 * k},
            ]});
            vec![
                shape("Frames", json!([{"type": "group", "opacity": in_out(0.0, 0.01, d, 0.5, json!(0), json!(1), "linear"), "items": [frame(-bw * 0.56, 0.4), frame(bw * 0.56, 0.55)]}]), 0.0, 0.0, 0.0, d),
                text("Line", c.text("line"), 70.0 * k, c.colour("text"), 800, 0.0, -c.h * 0.3, Anchor::Centre, 0.0, d, reveal(RevealKind::Rise, TextUnit::Word, 0.6, RevealKind::Fade)),
                spaced(text("Prompt", c.text("prompt"), 30.0 * k, c.colour("text"), 600, 0.0, 70.0 * k + bh / 2.0 + 50.0 * k, Anchor::Centre, 1.0, d, reveal(RevealKind::Fade, TextUnit::Character, 0.5, RevealKind::Fade)), 16.0),
            ]
        },
    });
}

fn build() -> Vec<Template> {
    let mut v = Vec::new();
    lower_thirds(&mut v);
    titles(&mut v);
    callouts(&mut v);
    social(&mut v);
    transitions(&mut v);
    infographics(&mut v);
    chart_templates(&mut v);
    end_cards(&mut v);
    v
}

pub fn all() -> &'static [Template] {
    static ALL: OnceLock<Vec<Template>> = OnceLock::new();
    ALL.get_or_init(build)
}
