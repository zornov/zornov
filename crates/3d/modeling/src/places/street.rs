// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Streets (road, kerbs, sidewalks, marked crossings, lamps, hydrants,
//! trees, rows of buildings), bridges, fences and stairs.

use super::facade::{Face, Roof, fire_escape};
use super::geom::{ball, bx, column, extrude, hull, prism, prism_x, pt, tube};
use super::mass::{block, roof};
use super::{Ctx, Mat, Spec, Style};
use std::f32::consts::{PI, TAU};

fn wall_tints(style: Style) -> &'static [[u8; 3]] {
    match style {
        Style::Classical => &[[222, 205, 170], [200, 190, 175], [214, 180, 150], [226, 214, 196]],
        Style::ArtDeco => &[[206, 190, 150], [186, 170, 140], [220, 206, 176], [170, 160, 140]],
        Style::Brutalist => &[[150, 148, 142], [128, 126, 122], [164, 160, 150]],
        Style::ModernGlass => &[[190, 192, 196], [170, 175, 182], [205, 205, 200]],
        Style::Medieval => &[[196, 184, 158], [210, 190, 150], [180, 170, 150], [200, 172, 140]],
        Style::Oriental => &[[236, 226, 204], [226, 210, 180], [240, 232, 214]],
        Style::Futuristic => &[[226, 232, 240], [200, 210, 225], [235, 238, 245]],
        Style::Rustic => &[[150, 130, 105], [168, 146, 118], [136, 120, 98], [160, 150, 130]],
    }
}

/// One building of a row, facing -z, centred on the origin at ground level.
fn row_building(cx: &mut Ctx, w: f32, d: f32, floors: usize, tint: usize, shop: bool, end: bool, escape: bool) -> Ctx {
    let mut c = cx.child();
    let tints = wall_tints(c.look.style);
    let rgb = tints[tint % tints.len()];
    c.look.wall.rgb = rgb;
    let front = Face { shop, ..Face::front() };
    let top = block(&mut c, w, d, floors, 0.0, front, true, end);
    let rk = if c.look.style == Style::Classical && c.chance(0.4) { Roof::Mansard } else if c.look.roofk == Roof::Stepped || c.look.roofk == Roof::Pagoda || c.look.roofk == Roof::Tower { Roof::Flat } else { c.look.roofk };
    roof(&mut c, rk, w, d, top);
    if escape && floors >= 3 {
        let z = -d / 2.0;
        fire_escape(&mut c, w * 0.18, z, floors.min(8));
    }
    // Different wall colours need different parts: name them by tint.
    let wall = c.look.wall;
    for s in c.parts.slots.iter_mut() {
        if s.mat == wall && tint > 0 {
            s.name = format!("{} {}", s.name, tint + 1);
        }
    }
    c
}

pub(crate) fn street(cx: &mut Ctx, spec: &Spec) {
    let l = cx.look.clone();
    let len = spec.size.max(1500.0);
    let road = spec.width.unwrap_or(700.0);
    let walk = 320.0;
    let kerb = 16.0;
    let asphalt = Mat::new([58, 60, 64], 0.95, "concrete");
    let paving = Mat::new([160, 156, 148], 0.9, "concrete");
    let kerb_m = Mat::new([186, 182, 174], 0.8, "stone");
    let (z0, z1) = (-len / 2.0, len / 2.0);
    // Road, with a little crown, and the ground beyond the sidewalks.
    bx(cx.parts.slot("road", asphalt), [-road / 2.0, road / 2.0], [-30.0, -1.0], [z0 - 200.0, z1 + 200.0], 0.0);
    for s in [-1.0f32, 1.0] {
        let x0 = s * road / 2.0;
        let x1 = s * (road / 2.0 + walk);
        bx(cx.parts.slot("kerbs", kerb_m), [x0 - s * 0.0, x0 + s * 24.0], [-30.0, kerb], [z0 - 200.0, z1 + 200.0], -1.0);
        bx(cx.parts.slot("sidewalk", paving), [x0 + s * 24.0, x1], [-30.0, kerb - 1.0], [z0 - 200.0, z1 + 200.0], 0.0);
        // Paving joints.
        if cx.detail >= 2 {
            let joint = Mat::new([112, 108, 100], 1.0, "concrete");
            let mut z = z0 - 160.0;
            while z < z1 + 200.0 {
                bx(cx.parts.slot("paving joints", joint), [x0 + s * 24.0, x1], [kerb - 1.0, kerb - 0.4], [z, z + 3.0], 0.0);
                z += 110.0;
            }
        }
    }
    // Markings: dashed centre line, crossings at both ends, stop lines.
    let white = Mat::new([226, 226, 220], 0.8, "concrete");
    let yellow = Mat::new([228, 190, 60], 0.8, "concrete");
    let mut z = z0;
    while z < z1 - 120.0 {
        bx(cx.parts.slot("road markings", white), [-6.0, 6.0], [-1.0, 0.0], [z, z + 200.0], 0.0);
        z += 400.0;
    }
    for zc in [z0 + 260.0, z1 - 260.0] {
        let mut x = -road / 2.0 + 40.0;
        while x < road / 2.0 - 60.0 {
            bx(cx.parts.slot("road markings", white), [x, x + 36.0], [-1.0, 0.0], [zc - 80.0, zc + 80.0], 0.0);
            x += 76.0;
        }
        bx(cx.parts.slot("road markings", white), [-road / 2.0 + 20.0, -4.0], [-1.0, 0.0], [zc - 150.0, zc - 130.0], 0.0);
        bx(cx.parts.slot("road markings", yellow), [road / 2.0 - 20.0, road / 2.0 - 8.0], [-1.0, 0.0], [zc - 150.0, zc + 150.0], 0.0);
    }
    // Manholes and drains.
    let iron = Mat::metal([50, 50, 52], 0.6);
    for k in 0..(len / 1300.0) as usize + 1 {
        let z = z0 + 500.0 + k as f32 * 1300.0 + cx.rng.range(-80.0, 80.0);
        column(cx.parts.slot("manholes", iron), cx.rng.range(-120.0, 120.0), z, 0.0, 3.0, 36.0, 36.0, 14);
    }
    for s in [-1.0f32, 1.0] {
        let mut z = z0 + 300.0;
        while z < z1 {
            bx(cx.parts.slot("drains", iron), [s * (road / 2.0 - 40.0) - 18.0, s * (road / 2.0 - 40.0) + 18.0], [-1.0, 1.0], [z, z + 60.0], 0.0);
            z += 900.0;
        }
    }
    // Rows of buildings on both sides.
    let floors_base = spec.floors.unwrap_or(match l.style {
        Style::ModernGlass | Style::ArtDeco => 9,
        Style::Brutalist => 6,
        Style::Classical => 5,
        Style::Oriental => 2,
        Style::Medieval | Style::Rustic => 3,
        Style::Futuristic => 10,
    });
    let shops_ok = matches!(l.style, Style::Classical | Style::ArtDeco | Style::ModernGlass | Style::Brutalist);
    let escapes_ok = matches!(l.style, Style::Classical | Style::ArtDeco | Style::Brutalist | Style::Rustic);
    for (side, yaw) in [(-1.0f32, -90.0f32), (1.0, 90.0)] {
        let mut z = z0;
        let mut i = 0;
        while z < z1 - 300.0 {
            let w = (l.bay * cx.rng.range(2.6, 5.4)).clamp(700.0, 1700.0);
            let w = w.min(z1 - z);
            if w < 500.0 {
                break;
            }
            let depth = cx.rng.range(850.0, 1150.0);
            let floors = ((floors_base as f32 + cx.rng.range(-2.0, 2.0)).round() as usize).clamp(2, 40);
            let tint = (cx.rng.next() * 4.0) as usize;
            let shop = shops_ok && cx.chance(0.5);
            let esc = escapes_ok && cx.chance(0.35);
            let end = z + w >= z1 - 320.0 || i == 0;
            let b = row_building(cx, w, depth, floors, tint, shop, end, esc);
            // Front faces the road: the building's -z looks toward x = 0.
            let x = side * (road / 2.0 + walk + depth / 2.0 - 4.0);
            cx.parts.absorb(b.parts, yaw, [x, 0.0, z + w / 2.0]);
            z += w;
            i += 1;
        }
        // Curbside furniture on this side.
        furniture(cx, side, road, walk, z0, z1);
    }
}

fn furniture(cx: &mut Ctx, side: f32, road: f32, walk: f32, z0: f32, z1: f32) {
    let l = cx.look.clone();
    let pole = l.metal;
    let xl = side * (road / 2.0 + 70.0);
    let lamp_glow = Mat::glowing([255, 220, 150], 3.0);
    let mut z = z0 + 400.0;
    let mut k = 0;
    while z < z1 {
        // Lamp post: base, shaft, arm, head.
        column(cx.parts.slot("lamp posts", pole), xl, z, 0.0, 30.0, 14.0, 10.0, 10);
        column(cx.parts.slot("lamp posts", pole), xl, z, 30.0, 520.0, 7.0, 5.0, 10);
        tube(cx.parts.slot("lamp posts", pole), pt(xl, 520.0, z), pt(xl - side * 70.0, 560.0, z), 4.0, 3.5, 8);
        bx(cx.parts.slot("lamp heads", pole), [xl - side * 100.0, xl - side * 56.0], [552.0, 566.0], [z - 14.0, z + 14.0], -1.0);
        bx(cx.parts.slot("lamps", lamp_glow), [xl - side * 96.0, xl - side * 60.0], [546.0, 552.0], [z - 10.0, z + 10.0], 0.0);
        z += 900.0 + 80.0 * (k % 2) as f32;
        k += 1;
    }
    // Fire hydrants, bollards, a bench and bins; trees in pits.
    let red = Mat::new([180, 40, 34], 0.5, "painted_metal");
    let mut z = z0 + 900.0;
    while z < z1 - 400.0 {
        let x = xl + side * 20.0;
        column(cx.parts.slot("hydrants", red), x, z, 0.0, 62.0, 13.0, 11.0, 10);
        column(cx.parts.slot("hydrants", red), x, z, 62.0, 70.0, 15.0, 8.0, 10);
        tube(cx.parts.slot("hydrants", red), pt(x - 18.0, 40.0, z), pt(x + 18.0, 40.0, z), 5.0, 5.0, 8);
        z += 2100.0;
    }
    let wood = Mat::new([120, 84, 52], 0.8, "wood");
    let mut z = z0 + 1500.0;
    while z < z1 - 400.0 {
        let x = side * (road / 2.0 + walk * 0.55);
        bx(cx.parts.slot("benches", wood), [x - 20.0, x + 20.0], [44.0, 50.0], [z - 80.0, z + 80.0], -1.0);
        bx(cx.parts.slot("benches", wood), [x + side * 20.0 - 2.0, x + side * 20.0 + 2.0], [50.0, 100.0], [z - 80.0, z + 80.0], -1.0);
        for dz in [-70.0f32, 70.0] {
            bx(cx.parts.slot("benches", pole), [x - 18.0, x + 18.0], [16.0, 44.0], [z + dz - 3.0, z + dz + 3.0], 0.0);
        }
        z += 2300.0;
    }
    // Trees (not on every street): a trunk and a crown, a planter pit.
    if !matches!(l.style, Style::Brutalist | Style::Futuristic) {
        let mut z = z0 + 700.0;
        let leaf = Mat::new([68, 112, 56], 0.9, "leaf");
        let bark = Mat::new([84, 64, 46], 0.9, "bark");
        while z < z1 - 400.0 {
            let x = side * (road / 2.0 + walk * 0.72);
            bx(cx.parts.slot("tree pits", Mat::new([92, 82, 70], 1.0, "concrete")), [x - 50.0, x + 50.0], [kerb_top(), kerb_top() + 4.0], [z - 50.0, z + 50.0], -1.0);
            tube(cx.parts.slot("tree trunks", bark), pt(x, 16.0, z), pt(x + cx.rng.range(-8.0, 8.0), 300.0, z), 11.0, 7.0, 8);
            let r = cx.rng.range(110.0, 160.0);
            ball(cx.parts.slot("tree crowns", leaf), pt(x, 400.0, z), r, 0.85, 12);
            z += 1500.0;
        }
    }
    let _ = (walk, TAU);
}

fn kerb_top() -> f32 {
    15.0
}

// ---------------------------------------------------------------- bridge

pub(crate) fn bridge(cx: &mut Ctx, spec: &Spec) {
    let l = cx.look.clone();
    let len = spec.size.max(1200.0);
    let wd = spec.width.unwrap_or(620.0);
    let variant = spec.variant.clone().unwrap_or_else(|| match l.style {
        Style::Classical | Style::Medieval | Style::Oriental => "arch",
        Style::ArtDeco => "suspension",
        Style::Futuristic => "cable_stayed",
        _ => "beam",
    }.into());
    let stone = l.wall;
    let trim = l.trim;
    let water = Mat::new([56, 92, 108], 0.08, "water");
    let road = Mat::new([60, 62, 66], 0.95, "concrete");
    let mark = Mat::new([226, 226, 220], 0.8, "concrete");
    let n_arch = ((len / 700.0).round() as usize).clamp(2, 9);
    let span = len / n_arch as f32;
    let height = match variant.as_str() {
        "arch" => span * 0.5 + 220.0,
        "suspension" | "cable_stayed" => 560.0,
        _ => 440.0,
    };
    // River and banks.
    bx(cx.parts.slot("water", water), [-len / 2.0 - 900.0, len / 2.0 + 900.0], [-260.0, -30.0], [-wd * 1.6, wd * 1.6], 0.0);
    for s in [-1.0f32, 1.0] {
        bx(cx.parts.slot("banks", Mat::new([100, 92, 74], 1.0, "terrain")), [s * (len / 2.0 + 20.0), s * (len / 2.0 + 900.0)], [-260.0, height - 60.0], [-wd * 1.6, wd * 1.6], -1.0);
    }
    // Deck and surface.
    let deck_t = 36.0;
    match variant.as_str() {
        "arch" => {
            for i in 0..n_arch {
                let (xa, xb) = (-len / 2.0 + i as f32 * span, -len / 2.0 + (i as f32 + 1.0) * span);
                let xc = (xa + xb) / 2.0;
                let pier = (span * 0.12).max(40.0);
                let r = span / 2.0 - pier / 2.0;
                let rise = r * 0.92;
                let n = 14;
                let outline = arch_block(xa, xb, xc, r, rise, height - deck_t, n);
                prism(cx.parts.slot("arch spans", stone), &outline, [-wd / 2.0, wd / 2.0]);
                // Arch ring (voussoirs) proud of the face.
                let ring = arch_ring(xc, r, rise, 34.0, n);
                prism(cx.parts.slot("arch rings", trim), &ring, [-wd / 2.0 - 8.0, -wd / 2.0 + 14.0]);
                prism(cx.parts.slot("arch rings", trim), &ring, [wd / 2.0 - 14.0, wd / 2.0 + 8.0]);
                // Keystone.
                bx(cx.parts.slot("arch rings", trim), [xc - 16.0, xc + 16.0], [rise + 6.0, rise + 58.0], [-wd / 2.0 - 12.0, -wd / 2.0 + 14.0], -1.0);
                bx(cx.parts.slot("arch rings", trim), [xc - 16.0, xc + 16.0], [rise + 6.0, rise + 58.0], [wd / 2.0 - 14.0, wd / 2.0 + 12.0], -1.0);
                // Cutwater on piers.
                if i + 1 < n_arch {
                    prism(cx.parts.slot("cutwaters", stone), &[[xb - pier / 2.0, -240.0], [xb + pier / 2.0, -240.0], [xb + pier / 2.0, height * 0.35], [xb, height * 0.42], [xb - pier / 2.0, height * 0.35]], [-wd / 2.0 - 40.0, wd / 2.0 + 40.0]);
                }
            }
            bx(cx.parts.slot("string course", trim), [-len / 2.0, len / 2.0], [height - deck_t - 4.0, height - deck_t + 10.0], [-wd / 2.0 - 14.0, wd / 2.0 + 14.0], -1.0);
        }
        _ => {
            // Girder deck on piers.
            bx(cx.parts.slot("deck", stone), [-len / 2.0 - 40.0, len / 2.0 + 40.0], [height - deck_t, height], [-wd / 2.0, wd / 2.0], -1.0);
            for s in [-0.4f32, 0.0, 0.4] {
                bx(cx.parts.slot("girders", l.metal), [-len / 2.0, len / 2.0], [height - deck_t - 60.0, height - deck_t], [s * wd - 10.0, s * wd + 10.0], -1.0);
            }
            let piers = (len / 800.0) as usize;
            for k in 1..=piers {
                let x = -len / 2.0 + len * k as f32 / (piers + 1) as f32;
                bx(cx.parts.slot("piers", stone), [x - 36.0, x + 36.0], [-260.0, height - deck_t - 60.0], [-wd * 0.3, wd * 0.3], -1.0);
                bx(cx.parts.slot("piers", stone), [x - 56.0, x + 56.0], [height - deck_t - 90.0, height - deck_t - 60.0], [-wd * 0.46, wd * 0.46], -1.0);
            }
        }
    }
    bx(cx.parts.slot("roadway", road), [-len / 2.0 - 40.0, len / 2.0 + 40.0], [height, height + 8.0], [-wd / 2.0 + 60.0, wd / 2.0 - 60.0], -1.0);
    bx(cx.parts.slot("footways", Mat::new([150, 146, 138], 0.9, "concrete")), [-len / 2.0 - 40.0, len / 2.0 + 40.0], [height, height + 14.0], [-wd / 2.0, -wd / 2.0 + 60.0], -1.0);
    bx(cx.parts.slot("footways", Mat::new([150, 146, 138], 0.9, "concrete")), [-len / 2.0 - 40.0, len / 2.0 + 40.0], [height, height + 14.0], [wd / 2.0 - 60.0, wd / 2.0], -1.0);
    let mut x = -len / 2.0;
    while x < len / 2.0 {
        bx(cx.parts.slot("road markings", mark), [x, x + 160.0], [height + 8.0, height + 9.0], [-5.0, 5.0], 0.0);
        x += 360.0;
    }
    // Parapets or railings with posts and lamps.
    for s in [-1.0f32, 1.0] {
        let z = s * (wd / 2.0 - 16.0);
        if matches!(l.style, Style::Classical | Style::Medieval | Style::Oriental | Style::Rustic) && variant == "arch" {
            bx(cx.parts.slot("parapets", stone), [-len / 2.0 - 40.0, len / 2.0 + 40.0], [height + 14.0, height + 110.0], [z - 18.0, z + 18.0], -1.0);
            bx(cx.parts.slot("parapets", trim), [-len / 2.0 - 40.0, len / 2.0 + 40.0], [height + 110.0, height + 124.0], [z - 24.0, z + 24.0], -1.0);
        } else {
            bx(cx.parts.slot("railings", l.metal), [-len / 2.0 - 40.0, len / 2.0 + 40.0], [height + 100.0, height + 108.0], [z - 4.0, z + 4.0], 0.0);
            bx(cx.parts.slot("railings", l.metal), [-len / 2.0 - 40.0, len / 2.0 + 40.0], [height + 18.0, height + 24.0], [z - 3.0, z + 3.0], 0.0);
            let mut x = -len / 2.0;
            while x <= len / 2.0 + 1.0 {
                bx(cx.parts.slot("railings", l.metal), [x - 3.0, x + 3.0], [height + 14.0, height + 108.0], [z - 3.0, z + 3.0], 0.0);
                x += 56.0;
            }
        }
        let glow = Mat::glowing([255, 220, 150], 3.0);
        let mut x = -len / 2.0 + 150.0;
        while x < len / 2.0 {
            column(cx.parts.slot("lamp posts", l.metal), x, z, height + 14.0, height + 330.0, 6.0, 4.0, 8);
            ball(cx.parts.slot("lamps", glow), pt(x, height + 345.0, z), 14.0, 1.1, 8);
            x += 600.0;
        }
    }
    match variant.as_str() {
        "suspension" => {
            for s in [-1.0f32, 1.0] {
                let tx = s * len * 0.3;
                let top = height + 700.0;
                for zz in [-wd / 2.0 + 20.0, wd / 2.0 - 20.0] {
                    bx(cx.parts.slot("towers", stone), [tx - 34.0, tx + 34.0], [-200.0, top], [zz - 28.0, zz + 28.0], -1.0);
                    bx(cx.parts.slot("towers", trim), [tx - 44.0, tx + 44.0], [top, top + 24.0], [zz - 38.0, zz + 38.0], -1.0);
                }
                for u in [height + 220.0, height + 520.0] {
                    bx(cx.parts.slot("towers", stone), [tx - 30.0, tx + 30.0], [u, u + 40.0], [-wd / 2.0 + 20.0, wd / 2.0 - 20.0], -1.0);
                }
            }
            let tx = len * 0.3;
            let top = height + 700.0;
            for zz in [-wd / 2.0 + 20.0, wd / 2.0 - 20.0] {
                // Main cable: parabola between the tower tops, anchored at the ends.
                let mut prev = pt(-len / 2.0, height + 20.0, zz);
                let tops = [(-len / 2.0, height + 20.0), (-tx, top), (tx, top), (len / 2.0, height + 20.0)];
                let pts = cable_points(&tops, (height + 40.0) as f32, 40);
                for p in pts {
                    let q = pt(p.0, p.1, zz);
                    tube(cx.parts.slot("main cables", l.metal), prev, q, 7.0, 7.0, 6);
                    prev = q;
                }
                let mut x = -tx;
                while x <= tx {
                    let u = cable_height(&tops, x, height + 40.0);
                    tube(cx.parts.slot("hangers", l.metal), pt(x, u, zz), pt(x, height + 14.0, zz), 1.8, 1.8, 5);
                    x += 70.0;
                }
            }
        }
        "cable_stayed" => {
            let top = height + 760.0;
            for zz in [-wd / 2.0 + 20.0, wd / 2.0 - 20.0] {
                tube(cx.parts.slot("towers", l.trim), pt(0.0, -200.0, zz), pt(0.0, top, zz), 30.0, 16.0, 10);
                for k in 0..10 {
                    let u = top - 40.0 - k as f32 * 26.0;
                    for s in [-1.0f32, 1.0] {
                        let x = s * (140.0 + k as f32 * (len / 2.0 - 160.0) / 10.0);
                        tube(cx.parts.slot("cables", l.accent), pt(0.0, u, zz), pt(x, height + 14.0, zz), 2.6, 2.6, 5);
                    }
                }
            }
        }
        _ => {}
    }
}

fn cable_points(knots: &[(f32, f32); 4], _deck: f32, n: usize) -> Vec<(f32, f32)> {
    let (x0, x1) = (knots[0].0, knots[3].0);
    (1..=n).map(|i| {
        let x = x0 + (x1 - x0) * i as f32 / n as f32;
        (x, cable_height(knots, x, 0.0))
    }).collect()
}

/// Height of the main cable at x: straight to the tower tops, sagging between them.
fn cable_height(k: &[(f32, f32); 4], x: f32, low: f32) -> f32 {
    let _ = low;
    let (a, b) = (k[1], k[2]);
    if x < a.0 {
        let t = (x - k[0].0) / (a.0 - k[0].0);
        let sag = 1.0 - (1.0 - t).powi(2);
        k[0].1 + (a.1 - k[0].1) * (0.5 * t + 0.5 * sag)
    } else if x > b.0 {
        let t = (k[3].0 - x) / (k[3].0 - b.0);
        let sag = 1.0 - (1.0 - t).powi(2);
        k[3].1 + (b.1 - k[3].1) * (0.5 * t + 0.5 * sag)
    } else {
        let t = (x - a.0) / (b.0 - a.0);
        let low = a.1 - (a.1 - 120.0 - 560.0).max(0.0) * 0.0 - 520.0;
        let sag = 4.0 * t * (1.0 - t);
        a.1 + (low - a.1) * sag
    }
}

/// The wall above an arch: the outline from the left pier up and over.
fn arch_block(xa: f32, xb: f32, xc: f32, r: f32, rise: f32, top: f32, n: usize) -> Vec<[f32; 2]> {
    let mut o = vec![[xa, -240.0], [xc - r, -240.0], [xc - r, 0.0]];
    for i in 0..=n {
        let a = PI - PI * i as f32 / n as f32;
        o.push([xc + r * a.cos(), rise * a.sin()]);
    }
    o.push([xc + r, -240.0]);
    o.push([xb, -240.0]);
    o.push([xb, top]);
    o.push([xa, top]);
    o
}

fn arch_ring(xc: f32, r: f32, rise: f32, w: f32, n: usize) -> Vec<[f32; 2]> {
    let mut o = Vec::new();
    for i in 0..=n {
        let a = PI * i as f32 / n as f32;
        o.push([xc + (r + w) * a.cos(), (rise + w) * a.sin()]);
    }
    for i in (0..=n).rev() {
        let a = PI * i as f32 / n as f32;
        o.push([xc + r * a.cos(), rise * a.sin()]);
    }
    o
}

// ----------------------------------------------------------------- fence

pub(crate) fn fence(cx: &mut Ctx, spec: &Spec) {
    let l = cx.look.clone();
    let len = spec.size.max(300.0);
    let h = spec.depth.unwrap_or(match l.style {
        Style::Medieval => 230.0,
        Style::Brutalist => 240.0,
        _ => 160.0,
    });
    let gate = spec.variant.as_deref() != Some("no_gate");
    let post_gap = 220.0;
    let n = ((len / post_gap).round() as usize).max(2);
    let gap = len / n as f32;
    let gate_at = n / 2;
    let rail = l.metal;
    let wood = l.accent;
    for i in 0..=n {
        let x = -len / 2.0 + i as f32 * gap;
        let lean = if cx.chance(cx.wear * 0.2) { 6.0 } else { 0.0 };
        match l.style {
            Style::Classical | Style::ArtDeco => {
                bx(cx.parts.slot("posts", l.wall), [x - 20.0, x + 20.0], [0.0, h + 40.0], [-20.0, 20.0], -1.0);
                bx(cx.parts.slot("posts", l.trim), [x - 26.0, x + 26.0], [h + 40.0, h + 56.0], [-26.0, 26.0], -1.0);
                hull(cx.parts.slot("post caps", l.trim), ([x - 22.0, x + 22.0], [-22.0, 22.0]), ([x - 5.0, x + 5.0], [-5.0, 5.0]), [h + 56.0, h + 86.0]);
            }
            Style::Medieval => {
                tube(cx.parts.slot("palisade", wood), pt(x - lean, 0.0, 0.0), pt(x, h, 0.0), 14.0, 12.0, 8);
                tube(cx.parts.slot("palisade", wood), pt(x, h, 0.0), pt(x, h + 40.0, 0.0), 12.0, 1.0, 8);
            }
            Style::Brutalist => bx(cx.parts.slot("posts", l.wall), [x - 24.0, x + 24.0], [0.0, h + 10.0], [-24.0, 24.0], -1.0),
            Style::ModernGlass | Style::Futuristic => {
                bx(cx.parts.slot("posts", l.frame), [x - 5.0, x + 5.0], [0.0, h], [-5.0, 5.0], -1.0);
                if l.style == Style::Futuristic {
                    bx(cx.parts.slot("glow strips", l.accent), [x - 2.0, x + 2.0], [h * 0.2, h * 0.9], [-7.0, -5.0], 0.0);
                }
            }
            Style::Oriental => {
                column(cx.parts.slot("posts", l.frame), x, 0.0, 0.0, h + 20.0, 11.0, 11.0, 10);
                ball(cx.parts.slot("posts", l.accent), pt(x, h + 26.0, 0.0), 11.0, 1.0, 8);
            }
            Style::Rustic => {
                bx(cx.parts.slot("posts", wood), [x - 8.0 + lean, x + 8.0 + lean], [0.0, h + 10.0], [-8.0, 8.0], -1.0);
            }
        }
        if i < n && !(gate && i == gate_at) {
            let (a, b) = (x + 24.0, x + gap - 24.0);
            infill(cx, a, b, h, i);
        }
    }
    // Rails for the styles that have them.
    let (xa, xb) = (-len / 2.0, len / 2.0);
    let gate_x = (-len / 2.0 + gate_at as f32 * gap, -len / 2.0 + (gate_at as f32 + 1.0) * gap);
    let rail_runs: Vec<(f32, f32)> = if gate { vec![(xa, gate_x.0), (gate_x.1, xb)] } else { vec![(xa, xb)] };
    for (a, b) in &rail_runs {
        match l.style {
            Style::Classical | Style::ArtDeco | Style::Rustic | Style::Oriental => {
                bx(cx.parts.slot("rails", if l.style == Style::Rustic { wood } else { rail }), [*a, *b], [h * 0.82, h * 0.82 + 8.0], [-3.0, 3.0], 0.0);
                bx(cx.parts.slot("rails", if l.style == Style::Rustic { wood } else { rail }), [*a, *b], [h * 0.2, h * 0.2 + 8.0], [-3.0, 3.0], 0.0);
            }
            Style::Medieval => bx(cx.parts.slot("rails", wood), [*a, *b], [h * 0.45, h * 0.45 + 10.0], [8.0, 18.0], -1.0),
            Style::ModernGlass | Style::Futuristic => bx(cx.parts.slot("rails", l.frame), [*a, *b], [h, h + 6.0], [-4.0, 4.0], 0.0),
            Style::Brutalist => {}
        }
    }
    if gate {
        let (ga, gb) = (gate_x.0 + 30.0, gate_x.1 - 30.0);
        let mid = (ga + gb) / 2.0;
        for (a, b) in [(ga, mid - 3.0), (mid + 3.0, gb)] {
            bx(cx.parts.slot("gate", rail), [a, a + 6.0], [14.0, h - 10.0], [-3.0, 3.0], 0.0);
            bx(cx.parts.slot("gate", rail), [b - 6.0, b], [14.0, h - 10.0], [-3.0, 3.0], 0.0);
            bx(cx.parts.slot("gate", rail), [a, b], [14.0, 20.0], [-3.0, 3.0], 0.0);
            bx(cx.parts.slot("gate", rail), [a, b], [h - 16.0, h - 10.0], [-3.0, 3.0], 0.0);
            let mut x = a + 18.0;
            while x < b - 12.0 {
                bx(cx.parts.slot("gate", rail), [x, x + 3.0], [20.0, h - 16.0], [-2.0, 2.0], 0.0);
                x += 18.0;
            }
        }
        bx(cx.parts.slot("hinges", rail), [ga - 4.0, ga + 4.0], [h * 0.25, h * 0.25 + 12.0], [-6.0, 6.0], 0.0);
        bx(cx.parts.slot("hinges", rail), [gb - 4.0, gb + 4.0], [h * 0.7, h * 0.7 + 12.0], [-6.0, 6.0], 0.0);
    }
    // A footing along the ground.
    bx(cx.parts.slot("footing", Mat::new([120, 116, 108], 0.95, "concrete")), [xa - 30.0, xb + 30.0], [-18.0, 0.0], [-34.0, 34.0], -1.0);
}

fn infill(cx: &mut Ctx, a: f32, b: f32, h: f32, i: usize) {
    let l = cx.look.clone();
    let rail = l.metal;
    let wood = l.accent;
    match l.style {
        Style::Classical | Style::ArtDeco => {
            let mut x = a;
            while x < b {
                if !cx.chance(cx.wear * 0.15) {
                    bx(cx.parts.slot("bars", rail), [x, x + 4.0], [h * 0.12, h * 0.9], [-2.0, 2.0], 0.0);
                    hull(cx.parts.slot("finials", if l.style == Style::ArtDeco { l.accent } else { rail }), ([x - 3.0, x + 7.0], [-4.0, 4.0]), ([x + 1.0, x + 3.0], [-0.5, 0.5]), [h * 0.9, h * 0.9 + 22.0]);
                }
                x += 22.0;
            }
        }
        Style::Rustic => {
            let mut x = a;
            while x < b {
                if !cx.chance(cx.wear * 0.2) {
                    let tilt = if cx.chance(cx.wear * 0.3) { 8.0 } else { 0.0 };
                    prism(cx.parts.slot("pickets", wood), &[[x, 0.0 + 6.0], [x + 14.0, 6.0], [x + 14.0 + tilt, h * 0.85], [x + 7.0 + tilt, h * 0.85 + 12.0], [x + tilt, h * 0.85]], [-3.0, 3.0]);
                }
                x += 24.0;
            }
        }
        Style::Brutalist => bx(cx.parts.slot("panels", l.wall), [a, b], [12.0, h], [-14.0, 14.0], -1.0),
        Style::ModernGlass | Style::Futuristic => {
            if !cx.chance(cx.wear * 0.25) {
                bx(cx.parts.slot("glass panels", l.glass), [a, b], [h * 0.1, h - 6.0], [-1.5, 1.5], 0.0);
            }
        }
        Style::Oriental => {
            bx(cx.parts.slot("lattice", l.frame), [a, b], [h * 0.12, h * 0.16], [-3.0, 3.0], 0.0);
            let mut x = a + 6.0;
            while x < b - 6.0 {
                bx(cx.parts.slot("lattice", l.frame), [x, x + 4.0], [h * 0.16, h * 0.8], [-3.0, 3.0], 0.0);
                x += 24.0;
            }
            bx(cx.parts.slot("lattice", l.frame), [a, b], [h * 0.46, h * 0.5], [-3.0, 3.0], 0.0);
        }
        Style::Medieval => {
            let _ = (wood, i);
        }
    }
}

// ----------------------------------------------------------------- stairs

pub(crate) fn stairs(cx: &mut Ctx, spec: &Spec) {
    let l = cx.look.clone();
    let height = spec.size.max(100.0);
    let rise = 18.0;
    let going = 28.0;
    let n = (height / rise).round().max(2.0) as usize;
    let r = height / n as f32;
    let wd = spec.width.unwrap_or(190.0);
    let variant = spec.variant.clone().unwrap_or_else(|| if l.style == Style::Medieval { "spiral".into() } else { "straight".into() });
    let tread = if matches!(l.style, Style::Rustic | Style::Medieval) { Mat::new([128, 92, 58], 0.85, "wood") } else if l.style == Style::Futuristic { Mat::glowing([80, 220, 255], 2.5) } else { l.trim };
    let stringer = l.wall;
    let rail = l.metal;
    match variant.as_str() {
        "spiral" => {
            let steps = (n as f32 * 1.0) as usize;
            let rr = wd * 0.7;
            tube(cx.parts.slot("newel", l.wall), pt(0.0, 0.0, 0.0), pt(0.0, height + 90.0, 0.0), 18.0, 18.0, 12);
            for k in 0..steps {
                let a0 = TAU * k as f32 / (steps as f32 * 0.8);
                let a1 = a0 + TAU / (steps as f32 * 0.8) * 1.04;
                let u0 = k as f32 * r;
                let outline = vec![[0.0, 0.0], [rr * a0.cos(), rr * a0.sin()], [rr * ((a0 + a1) / 2.0).cos() * 1.02, rr * ((a0 + a1) / 2.0).sin() * 1.02], [rr * a1.cos(), rr * a1.sin()]];
                extrude(cx.parts.slot("treads", tread), &outline, [u0, u0 + r], |a, b, d| pt(a, d, b));
                tube(cx.parts.slot("balusters", rail), pt(rr * a0.cos() * 0.97, u0 + r, rr * a0.sin() * 0.97), pt(rr * a0.cos() * 0.97, u0 + r + 90.0, rr * a0.sin() * 0.97), 2.2, 2.2, 5);
            }
            // Handrail as a helix of short tubes.
            let seg = steps * 2;
            let mut prev = None;
            for k in 0..=seg {
                let f = k as f32 / seg as f32;
                let a = TAU * f * steps as f32 / (steps as f32 * 0.8);
                let p = pt(rr * a.cos() * 0.97, f * steps as f32 * r + r + 90.0, rr * a.sin() * 0.97);
                if let Some(q) = prev {
                    tube(cx.parts.slot("handrails", l.accent), q, p, 4.0, 4.0, 6);
                }
                prev = Some(p);
            }
        }
        _ => {
            let turn = variant == "turn";
            let flight = if turn { n / 2 } else { n };
            let mut x_off = 0.0;
            let mut z_off = 0.0;
            let mut u = 0.0;
            let mut sign = 1.0f32;
            for fl in 0..(if turn { 2 } else { 1 }) {
                let cnt = if fl == 0 { flight } else { n - flight };
                let len = cnt as f32 * going;
                let _ = x_off;
                // Stringers: sloped slabs either side.
                for sx in [-1.0f32, 1.0] {
                    let x = x_off + sx * (wd / 2.0 + 5.0);
                    let o = [[0.0, u], [len, u + cnt as f32 * r], [len, u + cnt as f32 * r - 40.0], [0.0, u - 40.0]];
                    let pts: Vec<[f32; 2]> = o.iter().map(|p| [z_off + sign * p[0], p[1]]).collect();
                    prism_x(cx.parts.slot("stringers", stringer), &pts, [x - 5.0, x + 5.0]);
                }
                for k in 0..cnt {
                    let z0 = z_off + sign * k as f32 * going;
                    let zr = [z0.min(z0 + sign * going), z0.max(z0 + sign * going)];
                    let top = u + (k as f32 + 1.0) * r;
                    bx(cx.parts.slot("treads", tread), [x_off - wd / 2.0, x_off + wd / 2.0], [top - 5.0, top], [zr[0] - 2.0, zr[1] + 2.0], -1.0);
                    bx(cx.parts.slot("risers", stringer), [x_off - wd / 2.0, x_off + wd / 2.0], [top - r, top - 5.0], [zr[0] + if sign > 0.0 { 0.0 } else { going - 3.0 }, zr[0] + if sign > 0.0 { 3.0 } else { going }], 0.0);
                }
                // Balustrade on both sides.
                for sx in [-1.0f32, 1.0] {
                    let x = x_off + sx * (wd / 2.0 - 2.0);
                    let a = pt(x, u + 90.0, z_off);
                    let b = pt(x, u + cnt as f32 * r + 90.0, z_off + sign * len);
                    tube(cx.parts.slot("handrails", l.accent), a, b, 4.5, 4.5, 8);
                    let m = (len / 34.0) as usize;
                    for j in 0..=m {
                        let f = j as f32 / m.max(1) as f32;
                        let (zz, uu) = (z_off + sign * len * f, u + cnt as f32 * r * f);
                        if l.style == Style::ModernGlass || l.style == Style::Futuristic {
                            if j % 6 == 0 {
                                bx(cx.parts.slot("balusters", l.frame), [x - 2.0, x + 2.0], [uu, uu + 90.0], [zz - 2.0, zz + 2.0], 0.0);
                            }
                        } else if cx.detail >= 2 || j % 2 == 0 {
                            bx(cx.parts.slot("balusters", rail), [x - 1.8, x + 1.8], [uu + 4.0, uu + 90.0], [zz - 1.8, zz + 1.8], 0.0);
                        }
                    }
                    bx(cx.parts.slot("newel posts", l.wall), [x - 8.0, x + 8.0], [u, u + 120.0], [z_off - 8.0, z_off + 8.0], -1.0);
                }
                u += cnt as f32 * r;
                z_off += sign * len;
                if turn && fl == 0 {
                    // Landing, then turn back on the other side.
                    bx(cx.parts.slot("landing", tread), [x_off - wd / 2.0 - 10.0, x_off + wd * 1.5 + 14.0], [u - 8.0, u], [z_off, z_off + sign * wd + 20.0], -1.0);
                    x_off += wd + 14.0;
                    z_off += sign * wd + 20.0;
                    sign = -sign;
                }
            }
        }
    }
    bx(cx.parts.slot("foot slab", Mat::new([120, 116, 108], 0.95, "concrete")), [-wd, wd * 2.0 + 30.0], [-16.0, 0.0], [-80.0, n as f32 * going * 1.4], -1.0);
}
