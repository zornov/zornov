// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Rooms: boards or tatami underfoot, walls with real window and door
//! openings, skirting and mouldings, a ceiling that belongs to the style,
//! and furniture grouped the way the style lives (table and chairs, sofa,
//! shelves, fireplace, lamps, rug).

use super::facade::{Opening, Win, door, tile, window};
use super::geom::{ball, bx, column, hull, prism, pt, tube};
use super::{Ctx, Mat, Spec, Style};

fn wood_for(style: Style) -> Mat {
    match style {
        Style::Classical => Mat::new([120, 78, 46], 0.45, "wood_varnished"),
        Style::ArtDeco => Mat::new([70, 44, 30], 0.35, "wood_varnished"),
        Style::Brutalist => Mat::new([140, 138, 132], 0.9, "concrete"),
        Style::ModernGlass => Mat::new([196, 170, 130], 0.5, "wood"),
        Style::Medieval => Mat::new([92, 62, 40], 0.85, "wood"),
        Style::Oriental => Mat::new([112, 36, 28], 0.5, "wood_varnished"),
        Style::Futuristic => Mat::new([210, 216, 224], 0.25, "ceramic"),
        Style::Rustic => Mat::new([130, 92, 58], 0.85, "wood"),
    }
}

fn cloth_for(style: Style) -> Mat {
    match style {
        Style::Classical => Mat::new([120, 40, 46], 0.9, "velvet"),
        Style::ArtDeco => Mat::new([30, 84, 84], 0.8, "velvet"),
        Style::Brutalist => Mat::new([70, 74, 80], 0.95, "fabric"),
        Style::ModernGlass => Mat::new([60, 70, 84], 0.9, "fabric"),
        Style::Medieval => Mat::new([110, 40, 36], 0.95, "fabric"),
        Style::Oriental => Mat::new([180, 150, 80], 0.95, "fabric"),
        Style::Futuristic => Mat::new([230, 235, 245], 0.3, "plastic"),
        Style::Rustic => Mat::new([140, 100, 70], 0.95, "fabric"),
    }
}

fn table(cx: &mut Ctx, x: f32, z: f32, w: f32, d: f32, h: f32) {
    let wood = wood_for(cx.look.style);
    bx(cx.parts.slot("table", wood), [x - w / 2.0, x + w / 2.0], [h - 8.0, h], [z - d / 2.0, z + d / 2.0], -1.0);
    for (sx, sz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        bx(cx.parts.slot("table", wood), [x + sx * (w / 2.0 - 12.0) - 4.0, x + sx * (w / 2.0 - 12.0) + 4.0], [0.0, h - 8.0], [z + sz * (d / 2.0 - 12.0) - 4.0, z + sz * (d / 2.0 - 12.0) + 4.0], -1.0);
    }
    bx(cx.parts.slot("table", wood), [x - w / 2.0 + 14.0, x + w / 2.0 - 14.0], [h - 24.0, h - 8.0], [z - d / 2.0 + 14.0, z + d / 2.0 - 14.0], 0.0);
}

fn chair(cx: &mut Ctx, x: f32, z: f32, face: f32) {
    // face: +1 looks +z, -1 looks -z
    let wood = wood_for(cx.look.style);
    let cloth = cloth_for(cx.look.style);
    bx(cx.parts.slot("chairs", wood), [x - 22.0, x + 22.0], [44.0, 50.0], [z - 22.0, z + 22.0], -1.0);
    bx(cx.parts.slot("chair cushions", cloth), [x - 19.0, x + 19.0], [50.0, 56.0], [z - 19.0, z + 19.0], -1.0);
    for (sx, sz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        bx(cx.parts.slot("chairs", wood), [x + sx * 19.0 - 2.5, x + sx * 19.0 + 2.5], [0.0, 44.0], [z + sz * 19.0 - 2.5, z + sz * 19.0 + 2.5], 0.0);
    }
    bx(cx.parts.slot("chairs", wood), [x - 22.0, x + 22.0], [50.0, 98.0], [z - face * 22.0 - 2.5, z - face * 22.0 + 2.5], -1.0);
}

fn sofa(cx: &mut Ctx, x: f32, z: f32, w: f32, face: f32) {
    let cloth = cloth_for(cx.look.style);
    let wood = wood_for(cx.look.style);
    let d = 90.0;
    bx(cx.parts.slot("sofa", cloth), [x - w / 2.0, x + w / 2.0], [14.0, 44.0], [z - d / 2.0, z + d / 2.0], -1.0);
    bx(cx.parts.slot("sofa", cloth), [x - w / 2.0, x + w / 2.0], [44.0, 92.0], [z - face * d / 2.0 - 10.0, z - face * d / 2.0 + 10.0], -1.0);
    for sx in [-1.0f32, 1.0] {
        bx(cx.parts.slot("sofa", cloth), [x + sx * (w / 2.0 - 9.0) - 9.0, x + sx * (w / 2.0 - 9.0) + 9.0], [44.0, 66.0], [z - d / 2.0, z + d / 2.0], -1.0);
    }
    let n = (w / 70.0) as usize;
    for i in 0..n.max(2) {
        let xx = x - w / 2.0 + 18.0 + (w - 36.0) * (i as f32 + 0.5) / n.max(2) as f32;
        bx(cx.parts.slot("sofa cushions", cloth), [xx - (w - 36.0) / n.max(2) as f32 / 2.0 + 1.0, xx + (w - 36.0) / n.max(2) as f32 / 2.0 - 1.0], [44.0, 56.0], [z - d / 2.0 + 8.0, z + d / 2.0 - 4.0], -1.0);
    }
    for sx in [-1.0f32, 1.0] {
        bx(cx.parts.slot("sofa", wood), [x + sx * (w / 2.0 - 6.0) - 3.0, x + sx * (w / 2.0 - 6.0) + 3.0], [0.0, 14.0], [z - d / 2.0 + 6.0, z - d / 2.0 + 12.0], 0.0);
        bx(cx.parts.slot("sofa", wood), [x + sx * (w / 2.0 - 6.0) - 3.0, x + sx * (w / 2.0 - 6.0) + 3.0], [0.0, 14.0], [z + d / 2.0 - 12.0, z + d / 2.0 - 6.0], 0.0);
    }
}

fn shelf(cx: &mut Ctx, x: f32, z: f32, w: f32, h: f32, face: f32) {
    let wood = wood_for(cx.look.style);
    let d = 34.0;
    for sx in [-1.0f32, 1.0] {
        bx(cx.parts.slot("shelves", wood), [x + sx * (w / 2.0 - 2.0) - 2.0, x + sx * (w / 2.0 - 2.0) + 2.0], [0.0, h], [z - d / 2.0, z + d / 2.0], -1.0);
    }
    bx(cx.parts.slot("shelves", wood), [x - w / 2.0, x + w / 2.0], [0.0, h], [z + face * (d / 2.0) * -1.0 - 1.5 * face, z + face * (d / 2.0) * -1.0 + 1.5 * face], -1.0);
    let rows = (h / 50.0) as usize;
    let spines = [Mat::new([140, 40, 36], 0.8, "leather"), Mat::new([40, 76, 120], 0.8, "leather"), Mat::new([200, 180, 120], 0.8, "fabric"), Mat::new([48, 100, 70], 0.8, "leather")];
    for r in 0..=rows {
        let u = r as f32 * (h - 4.0) / rows.max(1) as f32;
        bx(cx.parts.slot("shelves", wood), [x - w / 2.0, x + w / 2.0], [u, u + 4.0], [z - d / 2.0, z + d / 2.0], -1.0);
        if r < rows && cx.detail >= 2 {
            let mut xx = x - w / 2.0 + 6.0;
            let mut k = 0;
            while xx < x + w / 2.0 - 14.0 {
                let bw = cx.rng.range(4.0, 9.0);
                let bh = cx.rng.range(24.0, 40.0);
                if cx.chance(0.85) {
                    let m = spines[(k + r) % spines.len()];
                    let name = format!("books {}", (k + r) % spines.len() + 1);
                    bx(cx.parts.slot(&name, m), [xx, xx + bw], [u + 4.0, u + 4.0 + bh], [z - 11.0, z + 11.0], 0.0);
                }
                xx += bw + 0.5;
                k += 1;
            }
        }
    }
}

fn floor_lamp(cx: &mut Ctx, x: f32, z: f32) {
    let metal = cx.look.metal;
    let glow = Mat::glowing([255, 220, 160], 2.5);
    column(cx.parts.slot("lamps", metal), x, z, 0.0, 6.0, 20.0, 18.0, 12);
    column(cx.parts.slot("lamps", metal), x, z, 6.0, 150.0, 2.5, 2.5, 8);
    column(cx.parts.slot("lamp shades", glow), x, z, 150.0, 185.0, 22.0, 30.0, 14);
}

fn rug(cx: &mut Ctx, x: f32, z: f32, w: f32, d: f32) {
    let cloth = cloth_for(cx.look.style);
    bx(cx.parts.slot("rug", cloth), [x - w / 2.0, x + w / 2.0], [0.0, 2.5], [z - d / 2.0, z + d / 2.0], 0.0);
    let border = Mat { rgb: [0, 1, 2].map(|i| cloth.rgb[i].saturating_add(46)), ..cloth };
    for (a, b, c, e) in [(x - w / 2.0, x + w / 2.0, z - d / 2.0 + 10.0, z - d / 2.0 + 16.0), (x - w / 2.0, x + w / 2.0, z + d / 2.0 - 16.0, z + d / 2.0 - 10.0)] {
        bx(cx.parts.slot("rug border", border), [a + 10.0, b - 10.0], [2.5, 3.2], [c, e], 0.0);
    }
    for (a, b) in [(x - w / 2.0 + 10.0, x - w / 2.0 + 16.0), (x + w / 2.0 - 16.0, x + w / 2.0 - 10.0)] {
        bx(cx.parts.slot("rug border", border), [a, b], [2.5, 3.2], [z - d / 2.0 + 10.0, z + d / 2.0 - 10.0], 0.0);
    }
}

fn fireplace(cx: &mut Ctx, x: f32, z_wall: f32, towards: f32) {
    // towards: +1 if the room is at +z from the wall, -1 if at -z.
    let l = cx.look.clone();
    let stone = if matches!(l.style, Style::Medieval | Style::Rustic) { Mat::new([128, 122, 112], 0.95, "stone") } else { l.trim };
    let wd = 170.0;
    let zf = z_wall;
    let (a, b) = if towards > 0.0 { (zf, zf + 44.0) } else { (zf - 44.0, zf) };
    bx(cx.parts.slot("fireplace", stone), [x - wd / 2.0, x - wd / 2.0 + 28.0], [0.0, 120.0], [a, b], -1.0);
    bx(cx.parts.slot("fireplace", stone), [x + wd / 2.0 - 28.0, x + wd / 2.0], [0.0, 120.0], [a, b], -1.0);
    bx(cx.parts.slot("fireplace", stone), [x - wd / 2.0 - 10.0, x + wd / 2.0 + 10.0], [120.0, 140.0], [a - towards * 8.0, b + towards * 8.0], -1.0);
    bx(cx.parts.slot("hearth", stone), [x - wd / 2.0 - 20.0, x + wd / 2.0 + 20.0], [0.0, 6.0], [a - towards * 40.0, b], -1.0);
    bx(cx.parts.slot("firebox", Mat::new([22, 20, 20], 1.0, "concrete")), [x - wd / 2.0 + 28.0, x + wd / 2.0 - 28.0], [6.0, 112.0], [if towards > 0.0 { zf } else { zf - 12.0 }, if towards > 0.0 { zf + 12.0 } else { zf }], 0.0);
    let fire = Mat::glowing([255, 120, 40], 3.0);
    hull(cx.parts.slot("fire", fire), ([x - 40.0, x + 40.0], [zf + towards * 14.0 - 14.0, zf + towards * 14.0 + 14.0]), ([x - 6.0, x + 6.0], [zf + towards * 14.0 - 4.0, zf + towards * 14.0 + 4.0]), [8.0, 60.0]);
    bx(cx.parts.slot("chimney breast", stone), [x - wd / 2.0 - 10.0, x + wd / 2.0 + 10.0], [140.0, 300.0], [a + towards * 0.0, b], -1.0);
}

pub(crate) fn room(cx: &mut Ctx, spec: &Spec) {
    let l = cx.look.clone();
    let w = spec.size.max(300.0);
    let d = spec.depth.unwrap_or(w * 0.85);
    let h = (l.storey * 0.88).max(240.0);
    let t = 22.0;
    let variant = spec.variant.clone().unwrap_or_default();
    let (hw, hd) = (w / 2.0, d / 2.0);
    let wood = wood_for(l.style);
    let floor_m = if matches!(l.style, Style::Brutalist) { Mat::new([128, 126, 120], 0.95, "concrete") } else if l.style == Style::Futuristic { Mat::new([60, 66, 76], 0.2, "ceramic") } else { wood };
    // Floor: boards of three shades, or tatami, or a slab.
    bx(cx.parts.slot("floor slab", Mat::new([92, 88, 82], 0.95, "concrete")), [-hw - t, hw + t], [-30.0, -2.0], [-hd - t, hd + t], -1.0);
    match l.style {
        Style::Oriental => {
            let tatami = Mat::new([196, 190, 140], 0.95, "fabric");
            let border = Mat::new([40, 50, 40], 0.9, "fabric");
            let (tw, td) = (95.0, 190.0);
            let mut x = -hw;
            let mut flip = false;
            while x < hw - 4.0 {
                let mut z = -hd;
                while z < hd - 4.0 {
                    let (a, b) = if flip { (td, tw) } else { (tw, td) };
                    let (x1, z1) = ((x + a).min(hw), (z + b).min(hd));
                    bx(cx.parts.slot("tatami", tatami), [x, x1 - 1.0], [0.0, 5.0], [z, z1 - 1.0], 0.0);
                    bx(cx.parts.slot("tatami edges", border), [x, x1 - 1.0], [5.0, 5.8], [z, z + 3.0], 0.0);
                    z = z1;
                }
                x += tw;
                flip = !flip;
            }
        }
        Style::Brutalist | Style::Futuristic => {
            bx(cx.parts.slot("floor", floor_m), [-hw, hw], [-2.0, 4.0], [-hd, hd], -1.0);
        }
        _ => {
            let n = (w / 24.0) as usize;
            let bw = w / n as f32;
            for i in 0..n {
                let shade = (cx.rng.next() * 3.0) as usize;
                let m = Mat { rgb: [0, 1, 2].map(|k| (floor_m.rgb[k] as i32 + (shade as i32 - 1) * 10).clamp(0, 255) as u8), ..floor_m };
                let name = if shade == 1 { "floor boards".to_string() } else { format!("floor boards {}", shade + 1) };
                bx(cx.parts.slot(&name, m), [-hw + i as f32 * bw, -hw + (i as f32 + 1.0) * bw - 1.2], [0.0, 5.0], [-hd, hd], 0.0);
            }
        }
    }
    // Back wall with windows (exterior toward +z).
    let wallm = l.wall;
    let mut back = cx.child();
    {
        let b = &mut back;
        let nwin = if l.window == Win::Curtain { 0 } else { ((w / (l.bay * 1.4)).round() as usize).clamp(1, 4) };
        if l.window == Win::Curtain || l.style == Style::Futuristic {
            // Glass wall.
            let bays = (w / 150.0).round().max(2.0) as usize;
            let bwid = w / bays as f32;
            for i in 0..bays {
                let (a, c) = (-hw + i as f32 * bwid, -hw + (i as f32 + 1.0) * bwid);
                bx(b.parts.slot("window glass", l.glass), [a + 3.0, c - 3.0], [20.0, h - 14.0], [t * 0.5, t * 0.5 + 3.0], 0.0);
                bx(b.parts.slot("mullions", l.frame), [a - 3.0, a + 3.0], [0.0, h], [t * 0.3, t * 0.3 + 10.0], 0.0);
            }
            bx(b.parts.slot("mullions", l.frame), [hw - 3.0, hw + 3.0], [0.0, h], [t * 0.3, t * 0.3 + 10.0], 0.0);
            bx(b.parts.slot("mullions", l.frame), [-hw, hw], [h - 14.0, h], [t * 0.3, t * 0.3 + 10.0], 0.0);
            bx(b.parts.slot("mullions", l.frame), [-hw, hw], [0.0, 20.0], [t * 0.3, t * 0.3 + 10.0], 0.0);
        } else {
            let mut ops = Vec::new();
            let ww = (w / (nwin as f32 + 0.6) * 0.6).clamp(90.0, 190.0);
            for i in 0..nwin {
                let xc = -hw + w * (i as f32 + 0.5) / nwin as f32;
                let arched = matches!(l.window, Win::Arched | Win::Pointed);
                let head = if arched { 120.0 + ww * 1.3 } else { h * 0.82 };
                ops.push(Opening { x0: xc - ww / 2.0, x1: xc + ww / 2.0, sill: 85.0, head: head.min(h - 20.0) });
            }
            tile(b, -hw, hw, [0.0, h], [0.0, t], &ops);
            for o in &ops {
                window(b, l.window, (o.x0 + o.x1) / 2.0, o.x1 - o.x0, o.sill, o.head, 0.0, t, false, 0);
            }
        }
    }
    cx.parts.absorb(back.parts, 180.0, [0.0, 0.0, hd + t]);
    // Left wall with a door (exterior toward -x), right wall plain.
    let mut left = cx.child();
    {
        let b = &mut left;
        let dw = 100.0;
        let dh = (h * 0.78).min(240.0);
        let xc = -d * 0.25;
        let ops = [Opening { x0: xc - dw / 2.0, x1: xc + dw / 2.0, sill: 0.0, head: dh }];
        tile(b, -hd, hd, [0.0, h], [0.0, t], &ops);
        door(b, xc, dw, dh, 0.0, t, false);
        // Architrave on the room side.
        let inner = t + 4.0;
        bx(b.parts.slot("architraves", l.trim), [xc - dw / 2.0 - 10.0, xc - dw / 2.0], [0.0, dh + 10.0], [t - 2.0, inner], -1.0);
        bx(b.parts.slot("architraves", l.trim), [xc + dw / 2.0, xc + dw / 2.0 + 10.0], [0.0, dh + 10.0], [t - 2.0, inner], -1.0);
        bx(b.parts.slot("architraves", l.trim), [xc - dw / 2.0 - 10.0, xc + dw / 2.0 + 10.0], [dh, dh + 10.0], [t - 2.0, inner], -1.0);
    }
    cx.parts.absorb(left.parts, 90.0, [-hw, 0.0, 0.0]);
    bx(cx.parts.slot("walls", wallm), [hw, hw + t], [0.0, h], [-hd, hd], -1.0);
    if variant == "closed" {
        bx(cx.parts.slot("walls", wallm), [-hw, hw], [0.0, h], [-hd - t, -hd], -1.0);
    }
    // Skirting and mouldings round the room.
    let trim = l.trim;
    for (x0, x1, z0, z1) in [(-hw, hw, hd - 6.0, hd), (hw - 6.0, hw, -hd, hd), (-hw, -hw + 6.0, -hd, hd)] {
        bx(cx.parts.slot("skirting", trim), [x0, x1], [0.0, 16.0], [z0, z1], -1.0);
        if matches!(l.style, Style::Classical | Style::ArtDeco) {
            bx(cx.parts.slot("crown moulding", trim), [x0, x1], [h - 14.0, h], [z0, z1], -1.0);
        }
    }
    // Ceiling.
    if variant != "cutaway" {
        ceiling(cx, w, d, h, t);
    }
    // Wall treatments.
    match l.style {
        Style::Classical | Style::ArtDeco => {
            let gilt = l.accent;
            for (i, x) in [-w * 0.3, 0.0, w * 0.3].iter().enumerate() {
                let _ = i;
                bx(cx.parts.slot("wall panels", trim), [x - 60.0, x + 60.0], [20.0, 120.0], [hd - 12.0, hd - 6.0], -1.0);
                bx(cx.parts.slot("panel inlays", gilt), [x - 50.0, x + 50.0], [30.0, 110.0], [hd - 13.0, hd - 12.0], 0.0);
            }
            fireplace(cx, hw - 4.0 - 90.0, -hd + 0.0, 1.0);
        }
        Style::Medieval | Style::Rustic => {
            fireplace(cx, hw - 130.0, hd - 6.0, -1.0);
        }
        Style::Oriental => {
            // Shoji screen on the right wall.
            let frame = l.frame;
            let paper = Mat::new([240, 232, 210], 0.9, "fabric");
            bx(cx.parts.slot("shoji paper", paper), [hw - 8.0, hw - 6.0], [20.0, h - 40.0], [-hd * 0.6, hd * 0.6], 0.0);
            let mut z = -hd * 0.6;
            while z <= hd * 0.6 + 1.0 {
                bx(cx.parts.slot("shoji frames", frame), [hw - 10.0, hw - 4.0], [20.0, h - 40.0], [z - 2.0, z + 2.0], 0.0);
                z += hd * 1.2 / 6.0;
            }
            let mut u = 20.0;
            while u <= h - 40.0 {
                bx(cx.parts.slot("shoji frames", frame), [hw - 10.0, hw - 4.0], [u - 2.0, u + 2.0], [-hd * 0.6, hd * 0.6], 0.0);
                u += (h - 60.0) / 4.0;
            }
        }
        Style::Brutalist | Style::ModernGlass | Style::Futuristic => {}
    }
    // Furniture by how the style lives.
    let sl = cx.look.style;
    match sl {
        Style::Oriental => {
            table(cx, 0.0, 0.0, 130.0, 90.0, 34.0);
            for (x, z) in [(-100.0f32, 0.0f32), (100.0, 0.0), (0.0, -80.0), (0.0, 80.0)] {
                bx(cx.parts.slot("cushions", cloth_for(sl)), [x - 30.0, x + 30.0], [5.0, 14.0], [z - 30.0, z + 30.0], -1.0);
            }
        }
        Style::Medieval | Style::Rustic => {
            table(cx, 0.0, 20.0, 260.0, 90.0, 76.0);
            for i in 0..3 {
                let x = -90.0 + i as f32 * 90.0;
                bx(cx.parts.slot("benches", wood), [x - 36.0, x + 36.0], [38.0, 44.0], [-48.0 + 20.0, -26.0 + 20.0], -1.0);
                bx(cx.parts.slot("benches", wood), [x - 36.0, x + 36.0], [38.0, 44.0], [26.0 + 20.0, 48.0 + 20.0], -1.0);
            }
            // Candle holders / lanterns on the table.
            let glow = Mat::glowing([255, 190, 90], 3.0);
            column(cx.parts.slot("candles", Mat::new([230, 220, 190], 0.6, "ceramic")), 0.0, 20.0, 76.0, 100.0, 4.0, 4.0, 8);
            ball(cx.parts.slot("flames", glow), pt(0.0, 104.0, 20.0), 5.0, 1.4, 8);
            shelf(cx, -hw * 0.55, hd - 24.0, 150.0, 190.0, -1.0);
        }
        _ => {
            rug(cx, 0.0, 10.0, w * 0.5, d * 0.45);
            sofa(cx, 0.0, hd * 0.55, w * 0.4, -1.0);
            table(cx, 0.0, -10.0, 110.0, 60.0, 38.0);
            chair(cx, -w * 0.28, -hd * 0.25, 1.0);
            chair(cx, w * 0.28, -hd * 0.25, 1.0);
            shelf(cx, hw * 0.58, hd - 24.0, 150.0, 200.0, -1.0);
            floor_lamp(cx, -hw + 60.0, hd - 60.0);
        }
    }
    if cx.look.style == Style::Classical || cx.look.style == Style::ArtDeco {
        chandelier(cx, 0.0, 0.0, h);
    }
    // Pictures on the right wall.
    for i in 0..2 {
        let z = -hd * 0.35 + i as f32 * hd * 0.7;
        let gilt = l.accent;
        bx(cx.parts.slot("picture frames", gilt), [hw - 8.0, hw - 2.0], [140.0, 210.0], [z - 36.0, z + 36.0], -1.0);
        bx(cx.parts.slot("pictures", Mat::new([90 + 40 * i as u8, 110, 130 - 30 * i as u8], 0.7, "fabric")), [hw - 9.0, hw - 8.0], [148.0, 202.0], [z - 28.0, z + 28.0], 0.0);
    }
    // Curtains at the window (cloth, left and right).
    if l.window != Win::Curtain && cx.detail >= 2 {
        let cm = cloth_for(sl);
        for sx in [-1.0f32, 1.0] {
            bx(cx.parts.slot("curtains", cm), [sx * (w * 0.24) - 16.0 + sx * 0.0, sx * (w * 0.24) + 16.0], [30.0, h - 30.0], [hd - 16.0, hd - 8.0], -1.0);
        }
        bx(cx.parts.slot("curtain rod", l.metal), [-w * 0.3, w * 0.3], [h - 34.0, h - 30.0], [hd - 14.0, hd - 10.0], 0.0);
    }
    if cx.wear > 0.2 {
        bx(cx.parts.slot("dust and grime", Mat::new([60, 54, 48], 1.0, "concrete")), [-hw + 20.0, hw - 20.0], [5.1, 5.4], [-hd + 20.0, hd - 20.0], 0.0);
    }
    let _ = (prism as fn(&mut crate::poly::Poly, &[[f32; 2]], [f32; 2]), tube as fn(&mut crate::poly::Poly, [f32; 3], [f32; 3], f32, f32, usize));
}

fn chandelier(cx: &mut Ctx, x: f32, z: f32, h: f32) {
    let metal = Mat::metal([200, 165, 80], 0.3);
    let glow = Mat::glowing([255, 230, 170], 3.5);
    tube(cx.parts.slot("chandelier", metal), pt(x, h, z), pt(x, h - 50.0, z), 3.0, 3.0, 6);
    column(cx.parts.slot("chandelier", metal), x, z, h - 66.0, h - 50.0, 26.0, 8.0, 14);
    for i in 0..6 {
        let a = std::f32::consts::TAU * i as f32 / 6.0;
        let (px, pz) = (x + a.cos() * 40.0, z + a.sin() * 40.0);
        tube(cx.parts.slot("chandelier", metal), pt(x, h - 62.0, z), pt(px, h - 70.0, pz), 2.0, 2.0, 5);
        ball(cx.parts.slot("chandelier bulbs", glow), pt(px, h - 80.0, pz), 7.0, 1.3, 8);
    }
}

fn ceiling(cx: &mut Ctx, w: f32, d: f32, h: f32, t: f32) {
    let l = cx.look.clone();
    let (hw, hd) = (w / 2.0, d / 2.0);
    let ceil = Mat::new([232, 228, 218], 0.95, "concrete");
    match l.style {
        Style::Brutalist => {
            let c = l.wall;
            bx(cx.parts.slot("ceiling", c), [-hw - t, hw + t], [h, h + 30.0], [-hd - t, hd + t], -1.0);
            // Waffle slab: a grid of beams under it.
            let n = 5;
            for i in 0..=n {
                let x = -hw + w * i as f32 / n as f32;
                bx(cx.parts.slot("ceiling beams", c), [x - 10.0, x + 10.0], [h - 24.0, h], [-hd, hd], -1.0);
                let z = -hd + d * i as f32 / n as f32;
                bx(cx.parts.slot("ceiling beams", c), [-hw, hw], [h - 24.0, h], [z - 10.0, z + 10.0], -1.0);
            }
        }
        Style::Medieval | Style::Rustic | Style::Oriental => {
            bx(cx.parts.slot("ceiling", if l.style == Style::Oriental { Mat::new([220, 205, 170], 0.9, "wood") } else { ceil }), [-hw - t, hw + t], [h, h + 20.0], [-hd - t, hd + t], -1.0);
            let beam = if l.style == Style::Oriental { l.frame } else { wood_for(l.style) };
            let n = (w / 150.0).round().max(3.0) as usize;
            for i in 0..=n {
                let x = -hw + w * i as f32 / n as f32;
                bx(cx.parts.slot("ceiling beams", beam), [x - 12.0, x + 12.0], [h - 26.0, h], [-hd, hd], -1.0);
            }
            if l.style == Style::Oriental {
                let m = (d / 150.0).round().max(2.0) as usize;
                for i in 0..=m {
                    let z = -hd + d * i as f32 / m as f32;
                    bx(cx.parts.slot("ceiling beams", beam), [-hw, hw], [h - 16.0, h], [z - 8.0, z + 8.0], -1.0);
                }
            }
        }
        Style::Futuristic | Style::ModernGlass => {
            bx(cx.parts.slot("ceiling", ceil), [-hw - t, hw + t], [h, h + 20.0], [-hd - t, hd + t], -1.0);
            let glow = Mat::glowing([230, 245, 255], 3.0);
            let n = 3;
            for i in 0..n {
                let z = -hd + d * (i as f32 + 0.5) / n as f32;
                bx(cx.parts.slot("light strips", glow), [-hw * 0.7, hw * 0.7], [h - 4.0, h], [z - 8.0, z + 8.0], 0.0);
            }
        }
        _ => {
            bx(cx.parts.slot("ceiling", ceil), [-hw - t, hw + t], [h, h + 20.0], [-hd - t, hd + t], -1.0);
            // A ceiling rose and a coffered border.
            column(cx.parts.slot("ceiling rose", l.trim), 0.0, 0.0, h - 6.0, h, 56.0, 56.0, 20);
            column(cx.parts.slot("ceiling rose", l.trim), 0.0, 0.0, h - 12.0, h - 6.0, 34.0, 34.0, 20);
            bx(cx.parts.slot("ceiling beams", l.trim), [-hw + 40.0, hw - 40.0], [h - 8.0, h], [-hd + 40.0, -hd + 50.0], -1.0);
            bx(cx.parts.slot("ceiling beams", l.trim), [-hw + 40.0, hw - 40.0], [h - 8.0, h], [hd - 50.0, hd - 40.0], -1.0);
        }
    }
}
