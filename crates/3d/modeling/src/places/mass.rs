// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Whole buildings: four walls dressed by the style, a lit-from-inside
//! core behind the glass, and a roof that belongs to the style (parapet
//! and plant, gable with chimneys, mansard with dormers, stepped setbacks
//! with a spire, tiered eaves, a tapering tower).

use super::facade::{Face, Roof, Win, facade, fire_escape};
use super::geom::{ball, bx, column, hull, prism, pt, tube};
use super::{Ctx, Mat, Spec, Style};

/// How many storeys a height holds.
fn floors_for(cx: &Ctx, spec: &Spec) -> usize {
    if let Some(f) = spec.floors {
        return f.clamp(1, 80);
    }
    let l = &cx.look;
    let mut n = 1;
    while l.height_of(n + 1) <= spec.size * 1.04 && n < 80 {
        n += 1;
    }
    n
}

fn default_width(cx: &Ctx, spec: &Spec) -> f32 {
    if let Some(w) = spec.width {
        return w.max(200.0);
    }
    let bays = match cx.look.style {
        Style::Classical => 5.0,
        Style::ArtDeco => 4.0,
        Style::Brutalist => 4.0,
        Style::ModernGlass => 8.0,
        Style::Medieval => 3.0,
        Style::Oriental => 3.0,
        Style::Futuristic => 5.0,
        Style::Rustic => 3.0,
    };
    cx.look.bay * bays
}

fn interior_mat() -> Mat {
    Mat::new([46, 40, 36], 0.95, "concrete")
}

/// Four dressed walls and a core: `floors` storeys of `w` x `d` with the
/// ground at height `base`. Returns the height of the top.
#[allow(clippy::too_many_arguments)]
pub(crate) fn block(cx: &mut Ctx, w: f32, d: f32, floors: usize, base: f32, front: Face, with_cornice: bool, sides: bool) -> f32 {
    let l = cx.look.clone();
    let t = l.thick;
    let mut f = front;
    f.top = with_cornice;
    let mut c = cx.child();
    super::facade::facade(&mut c, -w / 2.0, w, -d / 2.0, floors, f);
    cx.parts.absorb(c.parts, 0.0, [0.0, -base, 0.0]);
    let mut side = Face::side();
    side.top = with_cornice;
    let mut c = cx.child();
    facade(&mut c, -w / 2.0, w, 0.0, floors, Face { jet: true, ..side });
    cx.parts.absorb(c.parts, 180.0, [0.0, -base, d / 2.0]);
    let ws = (d - 2.0 * t).max(60.0);
    for (yaw, x) in [(90.0f32, -w / 2.0), (-90.0, w / 2.0)].into_iter().filter(|_| sides) {
        let mut c = cx.child();
        facade(&mut c, -ws / 2.0, ws, 0.0, floors, side);
        cx.parts.absorb(c.parts, yaw, [x, -base, 0.0]);
    }
    if !sides {
        // Party walls: plain, so a neighbour's lower roof never shows the dark core.
        let top = base + l.height_of(floors);
        for sx in [-1.0f32, 1.0] {
            let x = sx * (w / 2.0 - t / 2.0);
            bx(cx.parts.slot("walls", l.wall), [x - t / 2.0, x + t / 2.0], [base, top], [-d / 2.0 + t, d / 2.0 - t], -1.0);
        }
    }
    // The core, storey by storey, set back behind the glass; corner fill under a jetty.
    let mut y = base;
    let dark = interior_mat();
    for s in 0..floors {
        let h = l.storey_h(s);
        let j = l.jetty * s as f32;
        bx(cx.parts.slot("interior", dark), [-w / 2.0 + t, w / 2.0 - t], [y, y + h], [-d / 2.0 - j + t + 1.0, d / 2.0 + j - t - 1.0], 0.0);
        if j > 0.0 {
            for sx in [-1.0f32, 1.0] {
                let x = sx * (w / 2.0 - t / 2.0);
                for sz in [-1.0f32, 1.0] {
                    bx(cx.parts.slot("walls", l.wall), [x - t / 2.0, x + t / 2.0], [y, y + h], [sz * (d / 2.0 + j) - if sz < 0.0 { 0.0 } else { t }, sz * (d / 2.0) + if sz < 0.0 { t } else { 0.0 }], -1.0);
                }
            }
        }
        y += h;
    }
    y
}

pub(crate) fn building(cx: &mut Ctx, spec: &Spec) {
    if cx.look.style == Style::Futuristic && spec.kind != "facade" {
        return futuristic_tower(cx, spec);
    }
    let floors = floors_for(cx, spec);
    let w = default_width(cx, spec);
    let d = spec.depth.unwrap_or(w * 0.8).max(200.0);
    let l = cx.look.clone();
    let shop = matches!(l.style, Style::Classical | Style::Brutalist | Style::ModernGlass) && floors <= 4 && cx.chance(0.3);
    let roof_kind = if l.style == Style::Classical && cx.chance(0.5) { Roof::Mansard } else { l.roofk };
    match roof_kind {
        Roof::Pagoda => pagoda(cx, w, d, floors),
        Roof::Stepped => stepped(cx, w, d, floors),
        _ => {
            let top = block(cx, w, d, floors, 0.0, Face { shop, ..Face::front() }, true, true);
            roof(cx, roof_kind, w, d, top);
            ground_pad(cx, w, d);
        }
    }
}

fn ground_pad(cx: &mut Ctx, w: f32, d: f32) {
    let m = Mat::new([112, 108, 100], 0.95, "concrete");
    bx(cx.parts.slot("foundation", m), [-w / 2.0 - 60.0, w / 2.0 + 60.0], [-24.0, 0.0], [-d / 2.0 - 120.0, d / 2.0 + 60.0], -1.0);
}

pub(crate) fn facade_only(cx: &mut Ctx, spec: &Spec) {
    let floors = floors_for(cx, spec);
    let w = default_width(cx, spec);
    let l = cx.look.clone();
    let d = 140.0f32.max(l.thick * 2.5);
    let shop = floors <= 4 && cx.chance(0.25) && l.window != Win::Curtain;
    facade(cx, -w / 2.0, w, -d / 2.0, floors, Face { shop, ..Face::front() });
    // A thin box behind, returns at the ends and a back wall so it is a place, not a card.
    let top = l.height_of(floors);
    let t = l.thick;
    let dark = interior_mat();
    bx(cx.parts.slot("interior", dark), [-w / 2.0 + t, w / 2.0 - t], [0.0, top], [-d / 2.0 + t + 1.0, d / 2.0 - t], 0.0);
    bx(cx.parts.slot("walls", l.wall), [-w / 2.0, w / 2.0], [0.0, top], [d / 2.0 - t, d / 2.0], -1.0);
    for sx in [-1.0f32, 1.0] {
        let x = sx * (w / 2.0 - t / 2.0);
        bx(cx.parts.slot("walls", l.wall), [x - t / 2.0, x + t / 2.0], [0.0, top], [-d / 2.0 + t, d / 2.0], -1.0);
    }
    roof(cx, if l.roofk == Roof::Gable { Roof::Gable } else { Roof::Flat }, w, d, top);
    ground_pad(cx, w, d);
    let _ = spec;
}

/// A roof for the walls that end at height `top`.
pub(crate) fn roof(cx: &mut Ctx, kind: Roof, w: f32, d: f32, top: f32) {
    let l = cx.look.clone();
    match kind {
        Roof::Gable => {
            let ov = 46.0;
            let rise = if l.style == Style::Medieval { w * 0.36 } else { w * 0.24 };
            let th = 16.0;
            let (hw, hd) = (w / 2.0 + ov, d / 2.0 + ov + 20.0);
            // Two sloped slabs, a ridge, and wall gables under them.
            for s in [-1.0f32, 1.0] {
                prism(cx.parts.slot("roof", l.roof), &[[s * hw, top - 6.0], [0.0, top + rise], [0.0, top + rise + th], [s * hw, top - 6.0 + th]], [-hd, hd]);
            }
            bx(cx.parts.slot("roof", l.roof), [-14.0, 14.0], [top + rise, top + rise + th + 8.0], [-hd - 4.0, hd + 4.0], -1.0);
            for (z0, z1) in [(-d / 2.0, -d / 2.0 + l.thick), (d / 2.0 - l.thick, d / 2.0)] {
                prism(cx.parts.slot("walls", l.wall), &[[-w / 2.0, top], [w / 2.0, top], [0.0, top + rise - 2.0]], [z0, z1]);
            }
            if cx.detail >= 2 {
                // Battens along the slope.
                let n = (hd * 2.0 / 38.0) as usize;
                for s in [-1.0f32, 1.0] {
                    for i in 0..n {
                        let z = -hd + 12.0 + i as f32 * 38.0;
                        prism(cx.parts.slot("roof battens", l.roof), &[[s * hw, top - 6.0 + th], [0.0, top + rise + th], [0.0, top + rise + th + 3.0], [s * hw, top - 3.0 + th]], [z, z + 4.0]);
                    }
                }
            }
            if matches!(l.style, Style::Medieval | Style::Rustic) {
                chimney(cx, w * 0.28, 0.0, top + rise * 0.55, rise);
                dormers(cx, w, d, top, rise, hw);
            }
        }
        Roof::Mansard => {
            let rise = 150.0;
            hull(cx.parts.slot("roof", l.roof), ([-w / 2.0 - 10.0, w / 2.0 + 10.0], [-d / 2.0 - 10.0, d / 2.0 + 10.0]), ([-w / 2.0 + 90.0, w / 2.0 - 90.0], [-d / 2.0 + 90.0, d / 2.0 - 90.0]), [top, top + rise]);
            bx(cx.parts.slot("roof", l.roof), [-w / 2.0 + 90.0, w / 2.0 - 90.0], [top + rise, top + rise + 10.0], [-d / 2.0 + 90.0, d / 2.0 - 90.0], -1.0);
            // Dormers on the front slope.
            let n = ((w - 200.0) / l.bay).round().max(1.0) as usize;
            for i in 0..n {
                let x = -w / 2.0 + 100.0 + (w - 200.0) * (i as f32 + 0.5) / n as f32 + 30.0;
                let xc = x - 30.0;
                bx(cx.parts.slot("dormers", l.wall), [xc - 38.0, xc + 38.0], [top + 10.0, top + 100.0], [-d / 2.0 + 28.0, -d / 2.0 + 74.0], -1.0);
                prism(cx.parts.slot("roof", l.roof), &[[xc - 46.0, top + 100.0], [xc + 46.0, top + 100.0], [xc, top + 140.0]], [-d / 2.0 + 22.0, -d / 2.0 + 80.0]);
                bx(cx.parts.slot("window glass", l.glass), [xc - 24.0, xc + 24.0], [top + 28.0, top + 84.0], [-d / 2.0 + 25.0, -d / 2.0 + 28.0], 0.0);
                bx(cx.parts.slot("window frames", l.frame), [xc - 30.0, xc + 30.0], [top + 22.0, top + 90.0], [-d / 2.0 + 24.0, -d / 2.0 + 26.0], 0.0);
            }
            chimney(cx, -w * 0.3, d * 0.15, top + rise, 90.0);
            chimney(cx, w * 0.3, d * 0.15, top + rise, 90.0);
        }
        Roof::Hip => {
            hull(cx.parts.slot("roof", l.roof), ([-w / 2.0 - 40.0, w / 2.0 + 40.0], [-d / 2.0 - 40.0, d / 2.0 + 40.0]), ([-w * 0.2, w * 0.2], [-d * 0.1, d * 0.1]), [top, top + w * 0.2]);
        }
        _ => flat_roof(cx, w, d, top),
    }
}

fn chimney(cx: &mut Ctx, x: f32, z: f32, base: f32, h: f32) {
    let l = cx.look.clone();
    bx(cx.parts.slot("chimneys", l.trim), [x - 28.0, x + 28.0], [base - 40.0, base + h * 0.5], [z - 28.0, z + 28.0], -1.0);
    bx(cx.parts.slot("chimneys", l.trim), [x - 36.0, x + 36.0], [base + h * 0.5, base + h * 0.5 + 16.0], [z - 36.0, z + 36.0], -1.0);
    for dx in [-14.0f32, 14.0] {
        column(cx.parts.slot("chimney pots", Mat::new([150, 82, 56], 0.8, "ceramic")), x + dx, z, base + h * 0.5 + 16.0, base + h * 0.5 + 44.0, 9.0, 8.0, 8);
    }
}

fn dormers(cx: &mut Ctx, w: f32, d: f32, top: f32, rise: f32, hw: f32) {
    let l = cx.look.clone();
    let n = 2;
    for i in 0..n {
        let z = -d / 2.0 + d * (i as f32 + 0.5) / n as f32 * 0.9 + d * 0.05;
        let side = if i % 2 == 0 { -1.0f32 } else { 1.0 };
        let x = side * w * 0.2;
        let base = top + rise * 0.28;
        bx(cx.parts.slot("dormers", l.wall), [x - 36.0, x + 36.0], [base, base + 90.0], [z - 40.0, z + 40.0], -1.0);
        prism(cx.parts.slot("roof", l.roof), &[[x - 46.0, base + 90.0], [x + 46.0, base + 90.0], [x, base + 140.0]], [z - 46.0, z + 46.0]);
        let _ = hw;
        let face = if side < 0.0 { x - 37.0 } else { x + 37.0 };
        let _ = face;
        bx(cx.parts.slot("window glass", l.glass), [x - 22.0, x + 22.0], [base + 16.0, base + 74.0], [z - 41.5, z - 40.0], 0.0);
    }
}

/// Parapet, bulkhead, water tank, vents and aerials on a flat roof.
fn flat_roof(cx: &mut Ctx, w: f32, d: f32, top: f32) {
    let l = cx.look.clone();
    bx(cx.parts.slot("roof deck", l.roof), [-w / 2.0, w / 2.0], [top, top + 12.0], [-d / 2.0, d / 2.0], -1.0);
    // Parapets on the sides that have none from a cornice.
    let t = l.thick;
    bx(cx.parts.slot("parapet", l.wall), [-w / 2.0, w / 2.0], [top + 12.0, top + 52.0], [d / 2.0 - t, d / 2.0], -1.0);
    // Stair bulkhead.
    let (bw, bd) = (w * 0.16, d * 0.2);
    bx(cx.parts.slot("rooftop plant", l.trim), [w * 0.25 - bw / 2.0, w * 0.25 + bw / 2.0], [top + 12.0, top + 150.0], [d * 0.1 - bd / 2.0, d * 0.1 + bd / 2.0], -1.0);
    bx(cx.parts.slot("rooftop plant", l.metal), [w * 0.25 - 22.0, w * 0.25 + 22.0], [top + 12.0, top + 110.0], [d * 0.1 - bd / 2.0 - 3.0, d * 0.1 - bd / 2.0], 0.0);
    // Water tank on legs.
    let tx = -w * 0.2;
    column(cx.parts.slot("water tank", Mat::new([120, 90, 62], 0.8, "wood")), tx, 0.0, top + 70.0, top + 190.0, 52.0, 52.0, 16);
    column(cx.parts.slot("water tank", Mat::new([120, 90, 62], 0.8, "wood")), tx, 0.0, top + 190.0, top + 222.0, 52.0, 8.0, 16);
    for (lx, lz) in [(-38.0f32, -38.0f32), (38.0, -38.0), (-38.0, 38.0), (38.0, 38.0)] {
        bx(cx.parts.slot("rooftop plant", l.metal), [tx + lx - 3.0, tx + lx + 3.0], [top + 12.0, top + 72.0], [lz - 3.0, lz + 3.0], 0.0);
    }
    // Vents and air-conditioning units.
    let n = cx.rng.range(2.0, 5.0) as usize;
    for _ in 0..n {
        let (x, z) = (cx.rng.range(-w * 0.4, w * 0.1), cx.rng.range(-d * 0.35, d * 0.35));
        let (sx, sz, sh) = (cx.rng.range(30.0, 70.0), cx.rng.range(30.0, 60.0), cx.rng.range(30.0, 60.0));
        bx(cx.parts.slot("rooftop plant", l.metal), [x - sx / 2.0, x + sx / 2.0], [top + 12.0, top + 12.0 + sh], [z - sz / 2.0, z + sz / 2.0], -1.0);
        column(cx.parts.slot("rooftop plant", l.metal), x, z, top + 12.0 + sh, top + 22.0 + sh, 14.0, 14.0, 10);
    }
    // Antenna.
    tube(cx.parts.slot("rooftop plant", l.metal), pt(w * 0.3, top + 150.0, d * 0.1), pt(w * 0.3, top + 300.0, d * 0.1), 3.0, 1.5, 6);
    if l.style == Style::ModernGlass {
        // Mechanical penthouse with louvres.
        bx(cx.parts.slot("rooftop plant", l.frame), [-w * 0.3, w * 0.1], [top + 12.0, top + 190.0], [-d * 0.2, d * 0.2], -1.0);
        for i in 0..8 {
            let u = top + 40.0 + i as f32 * 16.0;
            bx(cx.parts.slot("rooftop plant", l.metal), [-w * 0.3 + 6.0, w * 0.1 - 6.0], [u, u + 5.0], [-d * 0.2 - 3.0, -d * 0.2], 0.0);
        }
    }
}

/// Setbacks that step in as the building rises, then a spire.
fn stepped(cx: &mut Ctx, w: f32, d: f32, floors: usize) {
    let l = cx.look.clone();
    let tiers = if floors >= 12 { 4 } else if floors >= 6 { 3 } else { 2 };
    let mut remaining = floors;
    let mut base = 0.0;
    let (mut cw, mut cd) = (w, d);
    for k in 0..tiers {
        let n = if k + 1 == tiers { remaining } else { ((remaining as f32) * if k == 0 { 0.55 } else { 0.5 }).round().max(1.0) as usize };
        let front = if k == 0 { Face::front() } else { Face { door_bay: None, shop: false, ..Face::front() } };
        let top = tier(cx, cw, cd, n, base, front, k);
        remaining = remaining.saturating_sub(n);
        // Roof slab of this tier where the next steps in.
        if k + 1 < tiers {
            bx(cx.parts.slot("roof deck", l.roof), [-cw / 2.0, cw / 2.0], [top, top + 10.0], [-cd / 2.0, cd / 2.0], -1.0);
        }
        base = top;
        cw *= 0.72;
        cd *= 0.78;
        if remaining == 0 {
            break;
        }
    }
    // Crown and spire.
    let crown = Mat::metal([212, 175, 70], 0.3);
    hull(cx.parts.slot("crown", l.roof), ([-cw / 2.0, cw / 2.0], [-cd / 2.0, cd / 2.0]), ([-cw * 0.2, cw * 0.2], [-cd * 0.2, cd * 0.2]), [base, base + cw * 0.5]);
    tube(cx.parts.slot("spire", crown), pt(0.0, base + cw * 0.5, 0.0), pt(0.0, base + cw * 0.5 + cw * 1.1, 0.0), 14.0, 1.5, 8);
    ball(cx.parts.slot("spire", crown), pt(0.0, base + cw * 0.5 + cw * 0.15, 0.0), 18.0, 1.0, 10);
    ground_pad(cx, w, d);
}

fn tier(cx: &mut Ctx, w: f32, d: f32, floors: usize, base: f32, front: Face, k: usize) -> f32 {
    let top = block(cx, w, d, floors, base, front, true, true);
    let _ = k;
    top
}

/// Tiers that narrow, each with its own tiled eaves, on a raised platform.
fn pagoda(cx: &mut Ctx, w: f32, d: f32, floors: usize) {
    let l = cx.look.clone();
    let tiers = floors.clamp(1, 7);
    let stone = Mat::new([170, 164, 152], 0.95, "stone");
    bx(cx.parts.slot("platform", stone), [-w / 2.0 - 140.0, w / 2.0 + 140.0], [0.0, 46.0], [-d / 2.0 - 220.0, d / 2.0 + 140.0], -1.0);
    for k in 0..3 {
        let e = 30.0 + k as f32 * 26.0;
        bx(cx.parts.slot("platform", stone), [-70.0 - e, 70.0 + e], [k as f32 * 15.0, 15.0 + k as f32 * 15.0 - 1.0], [-d / 2.0 - 220.0 - 30.0 + k as f32 * 4.0 - e * 0.0, -d / 2.0 - 190.0 - 10.0 + k as f32 * 15.0], -1.0);
    }
    let (mut cw, mut cd) = (w, d);
    let mut base = 46.0;
    for k in 0..tiers {
        let front = if k == 0 { Face::front() } else { Face { door_bay: None, shop: false, ..Face::front() } };
        let top = block(cx, cw, cd, 1, base, front, false, true);
        // Eaves over this tier (the facade pass added them to its own faces; add a full hip over the corners).
        let o = 110.0;
        hull(cx.parts.slot("roof tiles", l.roof), ([-cw / 2.0 - o, cw / 2.0 + o], [-cd / 2.0 - o, cd / 2.0 + o]), ([-cw / 2.0 + 20.0, cw / 2.0 - 20.0], [-cd / 2.0 + 20.0, cd / 2.0 - 20.0]), [top - 10.0, top + 70.0]);
        // Upturned corner tips.
        for (sx, sz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
            let (cxp, czp) = (sx * (cw / 2.0 + o), sz * (cd / 2.0 + o));
            tube(cx.parts.slot("roof tiles", l.roof), pt(cxp - sx * 24.0, top - 8.0, czp - sz * 24.0), pt(cxp + sx * 8.0, top + 26.0, czp + sz * 8.0), 7.0, 2.0, 6);
        }
        // Red columns at the corners.
        for (sx, sz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
            column(cx.parts.slot("columns", l.frame), sx * (cw / 2.0 - 4.0), sz * (cd / 2.0 - 4.0), base, top - 10.0, 14.0, 14.0, 12);
        }
        base = top + 70.0;
        cw *= 0.82;
        cd *= 0.82;
    }
    // Finial.
    let gold = l.accent;
    tube(cx.parts.slot("finial", gold), pt(0.0, base - 20.0, 0.0), pt(0.0, base + 180.0, 0.0), 10.0, 2.0, 8);
    for k in 0..5 {
        column(cx.parts.slot("finial", gold), 0.0, 0.0, base + k as f32 * 24.0, base + k as f32 * 24.0 + 6.0, 24.0 - k as f32 * 3.5, 24.0 - k as f32 * 3.5, 14);
    }
}

/// A tapering tower of glass drums with glowing bands, rings and a spire.
fn futuristic_tower(cx: &mut Ctx, spec: &Spec) {
    let l = cx.look.clone();
    let floors = floors_for(cx, spec);
    let r0 = spec.width.unwrap_or(l.bay * 3.0) / 2.0;
    let tiers = (floors / 5).clamp(3, 9);
    let per = floors as f32 / tiers as f32;
    let metal = l.trim;
    let glass = l.glass;
    // Podium with a ring of lit arches.
    column(cx.parts.slot("podium", l.wall), 0.0, 0.0, 0.0, 120.0, r0 * 1.7, r0 * 1.6, 40);
    column(cx.parts.slot("podium", metal), 0.0, 0.0, 120.0, 134.0, r0 * 1.75, r0 * 1.75, 40);
    let mut base = 134.0;
    let mut r = r0;
    for k in 0..tiers {
        let h = per * l.storey;
        let r1 = r * (0.86 + 0.06 * (k as f32 / tiers as f32));
        column(cx.parts.slot("interior", interior_mat()), 0.0, 0.0, base, base + h, r1 - 10.0, r - 10.0, 36);
        column(cx.parts.slot("window glass", glass), 0.0, 0.0, base, base + h, r1, r, 36);
        // Ribs.
        let ribs = 14;
        for i in 0..ribs {
            let a = std::f32::consts::TAU * i as f32 / ribs as f32;
            let (xa, za) = (a.cos(), a.sin());
            tube(cx.parts.slot("ribs", l.wall), pt(xa * (r + 4.0), base, za * (r + 4.0)), pt(xa * (r1 + 4.0), base + h, za * (r1 + 4.0)), 7.0, 6.0, 6);
        }
        // Rings: a glowing band per storey and a solid collar at the top.
        let n = per.round() as usize;
        for s in 0..n {
            let f = (s as f32 + 0.5) / n as f32;
            let rr = r + (r1 - r) * f;
            let u = base + h * f;
            if cx.detail >= 2 || s % 2 == 0 {
                column(cx.parts.slot("glow bands", l.accent), 0.0, 0.0, u - 5.0, u + 5.0, rr + 5.0, rr + 5.0, 36);
            }
        }
        column(cx.parts.slot("collars", metal), 0.0, 0.0, base + h - 10.0, base + h + 12.0, r1 + 22.0, r1 + 22.0, 36);
        // A ring balcony every other tier.
        if k % 2 == 1 {
            column(cx.parts.slot("balconies", metal), 0.0, 0.0, base + h + 12.0, base + h + 22.0, r1 + 70.0, r1 + 70.0, 36);
            column(cx.parts.slot("balcony rails", l.metal), 0.0, 0.0, base + h + 22.0, base + h + 84.0, r1 + 68.0, r1 + 68.0, 36);
        }
        base += h + 12.0;
        r = r1;
    }
    column(cx.parts.slot("crown", metal), 0.0, 0.0, base, base + 70.0, r, r * 0.7, 36);
    tube(cx.parts.slot("spire", l.metal), pt(0.0, base + 70.0, 0.0), pt(0.0, base + 70.0 + r * 3.0, 0.0), 10.0, 1.5, 8);
    ball(cx.parts.slot("glow bands", l.accent), pt(0.0, base + 70.0 + r * 3.0, 0.0), 14.0, 1.0, 10);
    // Skybridge struts and a landing pad collar.
    for i in 0..3 {
        let a = std::f32::consts::TAU * i as f32 / 3.0 + 0.4;
        tube(cx.parts.slot("struts", l.metal), pt(a.cos() * r0 * 1.5, 134.0, a.sin() * r0 * 1.5), pt(a.cos() * r0 * 0.8, base * 0.45, a.sin() * r0 * 0.8), 9.0, 5.0, 8);
    }
    let _ = fire_escape;
}
