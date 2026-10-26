// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The dressed front of a building: bays of windows set into real
//! openings (reveal, frame, glass, sill, lintel), doors and entrances,
//! pilasters or fins, string courses, balconies, shutters, timber frames,
//! cornices, all parameterised by the style's [`Look`].

use super::geom::{bx, column, hull, prism, pt, tube};
use super::{Ctx, Mat, Style};
use std::f32::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Win {
    Rect,
    Arched,
    Pointed,
    Slit,
    Ribbon,
    Curtain,
    Lattice,
    Band,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cornice {
    None,
    Classical,
    Deco,
    Slab,
    Eaves,
    Pagoda,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Roof {
    Flat,
    Gable,
    Hip,
    Mansard,
    Stepped,
    Pagoda,
    Tower,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Door {
    Portal,
    Arched,
    Slab,
    Glass,
    Gate,
    Plank,
}

/// How a style dresses a wall.
#[derive(Clone, Debug)]
pub struct Look {
    pub style: Style,
    pub wall: Mat,
    pub trim: Mat,
    pub frame: Mat,
    pub glass: Mat,
    pub roof: Mat,
    pub door: Mat,
    pub accent: Mat,
    pub metal: Mat,
    pub storey: f32,
    /// Ground storey height as a multiple of a storey.
    pub ground: f32,
    pub bay: f32,
    pub win_w: f32,
    pub win_h: f32,
    pub sill: f32,
    pub window: Win,
    pub thick: f32,
    pub pilasters: bool,
    pub fins: bool,
    pub courses: bool,
    pub timber: bool,
    pub shutters: bool,
    pub jetty: f32,
    pub balcony: f32,
    pub cornice: Cornice,
    pub roofk: Roof,
    pub doork: Door,
}

impl Look {
    pub fn of(style: Style) -> Look {
        let base = Look {
            style,
            wall: Mat::new([222, 205, 170], 0.85, "stone"),
            trim: Mat::new([240, 232, 215], 0.7, "stone"),
            frame: Mat::new([245, 242, 235], 0.5, "plastic_matte"),
            glass: Mat::glass([120, 150, 170]),
            roof: Mat::new([105, 95, 92], 0.8, "stone"),
            door: Mat::new([70, 45, 30], 0.6, "wood_varnished"),
            accent: Mat::new([160, 120, 70], 0.5, "wood"),
            metal: Mat::metal([55, 58, 62], 0.45),
            storey: 380.0,
            ground: 1.25,
            bay: 330.0,
            win_w: 0.42,
            win_h: 0.62,
            sill: 0.2,
            window: Win::Rect,
            thick: 36.0,
            pilasters: false,
            fins: false,
            courses: true,
            timber: false,
            shutters: false,
            jetty: 0.0,
            balcony: 0.0,
            cornice: Cornice::Classical,
            roofk: Roof::Flat,
            doork: Door::Portal,
        };
        match style {
            Style::Classical => Look { pilasters: true, balcony: 0.25, window: Win::Rect, ..base },
            Style::ArtDeco => Look {
                wall: Mat::new([206, 190, 150], 0.7, "stone"),
                trim: Mat::new([226, 214, 178], 0.6, "stone"),
                frame: Mat::new([60, 55, 50], 0.4, "steel"),
                glass: Mat::glass([70, 95, 120]),
                accent: Mat::metal([212, 175, 70], 0.3),
                roof: Mat::metal([70, 110, 100], 0.4),
                door: Mat::metal([212, 175, 70], 0.35),
                storey: 360.0,
                ground: 1.35,
                bay: 300.0,
                win_w: 0.3,
                win_h: 0.76,
                sill: 0.12,
                fins: true,
                cornice: Cornice::Deco,
                roofk: Roof::Stepped,
                doork: Door::Glass,
                ..base
            },
            Style::Brutalist => Look {
                wall: Mat::new([150, 148, 142], 0.95, "concrete"),
                trim: Mat::new([138, 136, 130], 0.95, "concrete"),
                frame: Mat::metal([38, 38, 40], 0.5),
                glass: Mat::glass([60, 78, 86]),
                roof: Mat::new([110, 108, 104], 0.95, "concrete"),
                door: Mat::metal([50, 52, 55], 0.5),
                storey: 340.0,
                ground: 1.2,
                bay: 420.0,
                win_w: 0.55,
                win_h: 0.42,
                sill: 0.3,
                window: Win::Ribbon,
                thick: 95.0,
                fins: true,
                courses: false,
                cornice: Cornice::Slab,
                doork: Door::Slab,
                ..base
            },
            Style::ModernGlass => Look {
                wall: Mat::new([190, 192, 196], 0.5, "concrete"),
                trim: Mat::new([200, 202, 205], 0.5, "concrete"),
                frame: Mat::metal([120, 126, 134], 0.3),
                glass: Mat::glass([78, 120, 150]),
                roof: Mat::new([90, 92, 96], 0.8, "concrete"),
                door: Mat::glass([100, 140, 165]),
                storey: 330.0,
                ground: 1.5,
                bay: 160.0,
                window: Win::Curtain,
                thick: 24.0,
                courses: false,
                cornice: Cornice::Slab,
                doork: Door::Glass,
                ..base
            },
            Style::Medieval => Look {
                wall: Mat::new([196, 184, 158], 0.95, "stone"),
                trim: Mat::new([150, 140, 125], 0.9, "stone"),
                frame: Mat::new([84, 58, 38], 0.8, "wood"),
                glass: Mat::glass([90, 105, 100]),
                roof: Mat::new([138, 72, 52], 0.85, "ceramic"),
                door: Mat::new([68, 46, 30], 0.8, "wood"),
                accent: Mat::new([84, 58, 38], 0.8, "wood"),
                storey: 300.0,
                ground: 1.0,
                bay: 260.0,
                win_w: 0.28,
                win_h: 0.36,
                sill: 0.32,
                window: Win::Pointed,
                thick: 55.0,
                timber: true,
                shutters: true,
                jetty: 42.0,
                courses: false,
                cornice: Cornice::Eaves,
                roofk: Roof::Gable,
                doork: Door::Arched,
                ..base
            },
            Style::Oriental => Look {
                wall: Mat::new([236, 226, 204], 0.9, "stone"),
                trim: Mat::new([60, 50, 44], 0.7, "wood"),
                frame: Mat::new([110, 30, 26], 0.5, "wood_varnished"),
                glass: Mat::glass([150, 140, 110]),
                roof: Mat::new([62, 68, 74], 0.7, "ceramic"),
                door: Mat::new([150, 35, 30], 0.5, "wood_varnished"),
                accent: Mat::metal([200, 160, 60], 0.35),
                storey: 340.0,
                ground: 1.1,
                bay: 340.0,
                win_w: 0.55,
                win_h: 0.55,
                window: Win::Lattice,
                thick: 40.0,
                courses: false,
                cornice: Cornice::Pagoda,
                roofk: Roof::Pagoda,
                doork: Door::Gate,
                ..base
            },
            Style::Futuristic => Look {
                wall: Mat::new([226, 232, 240], 0.3, "ceramic"),
                trim: Mat::metal([150, 160, 175], 0.25),
                frame: Mat::metal([90, 100, 115], 0.25),
                glass: Mat::glass([30, 50, 80]),
                roof: Mat::metal([120, 130, 145], 0.3),
                door: Mat::glass([40, 70, 110]),
                accent: Mat::glowing([80, 220, 255], 4.0),
                storey: 300.0,
                ground: 1.5,
                bay: 200.0,
                win_w: 0.8,
                win_h: 0.3,
                sill: 0.35,
                window: Win::Band,
                thick: 30.0,
                courses: false,
                cornice: Cornice::Slab,
                roofk: Roof::Tower,
                doork: Door::Glass,
                ..base
            },
            Style::Rustic => Look {
                wall: Mat::new([150, 130, 105], 0.95, "stone"),
                trim: Mat::new([112, 76, 46], 0.8, "wood"),
                frame: Mat::new([112, 76, 46], 0.8, "wood"),
                glass: Mat::glass([100, 120, 120]),
                roof: Mat::new([120, 86, 56], 0.9, "wood"),
                door: Mat::new([90, 58, 34], 0.8, "wood"),
                accent: Mat::new([112, 76, 46], 0.8, "wood"),
                storey: 290.0,
                ground: 1.0,
                bay: 250.0,
                win_w: 0.3,
                win_h: 0.4,
                sill: 0.3,
                thick: 48.0,
                shutters: true,
                courses: true,
                cornice: Cornice::Eaves,
                roofk: Roof::Gable,
                doork: Door::Plank,
                ..base
            },
        }
    }

    /// Height of storey `s` (the ground storey is taller).
    pub fn storey_h(&self, s: usize) -> f32 {
        if s == 0 { self.storey * self.ground } else { self.storey }
    }

    /// Height of the first `n` storeys.
    pub fn height_of(&self, n: usize) -> f32 {
        (0..n).map(|s| self.storey_h(s)).sum()
    }
}

/// What a facade call is asked for.
#[derive(Clone, Copy)]
pub struct Face {
    /// The front gets door, pilasters, balconies; plain sides do not.
    pub dressed: bool,
    pub door_bay: Option<usize>,
    /// A shop window and sign on the ground storey instead of small windows.
    pub shop: bool,
    /// Cornice along the top.
    pub top: bool,
    /// Upper storeys project forward where the style does that (a front or a back, not a side).
    pub jet: bool,
}

impl Face {
    pub fn front() -> Face {
        Face { dressed: true, door_bay: None, shop: false, top: true, jet: true }
    }
    pub fn side() -> Face {
        Face { dressed: false, door_bay: None, shop: false, top: true, jet: false }
    }
}

pub(super) struct Opening {
    pub x0: f32,
    pub x1: f32,
    pub sill: f32,
    pub head: f32,
}

fn arc(cx: f32, cy: f32, r: f32, a0: f32, a1: f32, n: usize) -> Vec<[f32; 2]> {
    (0..=n)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / n as f32;
            [cx + r * a.cos(), cy + r * a.sin()]
        })
        .collect()
}

/// The two halves of an arch head (right spring to apex, apex to left
/// spring), round or pointed, with the radius taken in by `inset`.
fn arch_halves(pointed: bool, xl: f32, xr: f32, spring: f32, inset: f32) -> (Vec<[f32; 2]>, Vec<[f32; 2]>) {
    let w = xr - xl;
    let xc = (xl + xr) / 2.0;
    if pointed {
        (arc(xl, spring, w - inset, 0.0, PI / 3.0, 6), arc(xr, spring, w - inset, 2.0 * PI / 3.0, PI, 6))
    } else {
        (arc(xc, spring, w / 2.0 - inset, 0.0, PI / 2.0, 6), arc(xc, spring, w / 2.0 - inset, PI / 2.0, PI, 6))
    }
}

/// The wall of one storey as boxes around the openings.
pub(super) fn tile(cx: &mut Ctx, x0: f32, x1: f32, u: [f32; 2], z: [f32; 2], ops: &[Opening]) {
    let mut at = x0;
    let m = cx.look.wall;
    for o in ops {
        if o.x0 > at + 0.5 {
            bx(cx.parts.slot("walls", m), [at, o.x0], u, z, -1.0);
        }
        if o.sill > u[0] + 0.5 {
            bx(cx.parts.slot("walls", m), [o.x0, o.x1], [u[0], o.sill], z, -1.0);
        }
        if o.head < u[1] - 0.5 {
            bx(cx.parts.slot("walls", m), [o.x0, o.x1], [o.head, u[1]], z, -1.0);
        }
        at = o.x1;
    }
    if x1 > at + 0.5 {
        bx(cx.parts.slot("walls", m), [at, x1], u, z, -1.0);
    }
}

/// Glass in a frame, lit or dark, or broken out.
fn broken(cx: &mut Ctx) -> bool {
    let p = (cx.wear - 0.25).max(0.0) * 0.6;
    cx.chance(p)
}

fn pane(cx: &mut Ctx, x: [f32; 2], u: [f32; 2], z: f32) {
    if broken(cx) {
        return;
    }
    if cx.chance(cx.lit) {
        let warm = Mat::glowing([255, 214, 140], 2.2);
        bx(cx.parts.slot("lit windows", warm), x, u, [z, z + 3.0], 0.0);
    } else {
        let glass = cx.look.glass;
        bx(cx.parts.slot("window glass", glass), x, u, [z, z + 3.0], 0.0);
    }
}

/// One window in an opening `w` wide between `sill` and `head`, in a wall
/// whose outer face is at `z` and which is `t` thick.
#[allow(clippy::too_many_arguments)]
pub(super) fn window(cx: &mut Ctx, kind: Win, xc: f32, w: f32, sill: f32, head: f32, z: f32, t: f32, dress: bool, floor: usize) {
    let l = cx.look.clone();
    let (xl, xr) = (xc - w / 2.0, xc + w / 2.0);
    let fr = if l.style == Style::Brutalist { 10.0 } else { 7.0 };
    let zf = z + t * 0.45;
    let frame = l.frame;
    let h = head - sill;
    match kind {
        Win::Arched | Win::Pointed => {
            let pointed = kind == Win::Pointed;
            let spring = if pointed { head - w * 0.866 } else { head - w / 2.0 };
            let (ar, al) = arch_halves(pointed, xl, xr, spring, 0.0);
            // The corners above the arch filled with wall.
            let mut right = vec![[xr, head]];
            right.extend(ar.iter().cloned());
            let mut left = vec![[xl, head]];
            left.extend(al.iter().cloned());
            for poly in [right, left] {
                prism(cx.parts.slot("walls", l.wall), &poly, [z, z + t]);
            }
            // Glass in the arch.
            let (ir, il) = arch_halves(pointed, xl, xr, spring, fr);
            let mut outline = vec![[xl + fr, sill + fr], [xr - fr, sill + fr]];
            outline.extend(ir.iter().cloned());
            outline.extend(il.iter().cloned());
            if !broken(cx) {
                if cx.chance(cx.lit) {
                    prism(cx.parts.slot("lit windows", Mat::glowing([255, 214, 140], 2.2)), &outline, [z + t * 0.55, z + t * 0.55 + 3.0]);
                } else {
                    prism(cx.parts.slot("window glass", l.glass), &outline, [z + t * 0.55, z + t * 0.55 + 3.0]);
                }
            }
            // A frame band round the arch, jambs and a sill.
            let mut band: Vec<[f32; 2]> = Vec::new();
            band.extend(ar.iter().cloned());
            band.extend(al.iter().cloned());
            band.extend(il.iter().rev().cloned());
            band.extend(ir.iter().rev().cloned());
            prism(cx.parts.slot("window frames", frame), &band, [zf, zf + 8.0]);
            for (a, b) in [(xl, xl + fr), (xr - fr, xr)] {
                bx(cx.parts.slot("window frames", frame), [a, b], [sill, spring], [zf, zf + 8.0], 0.0);
            }
            bx(cx.parts.slot("window frames", frame), [xl, xr], [sill, sill + fr], [zf, zf + 8.0], 0.0);
            if cx.detail >= 2 {
                bx(cx.parts.slot("window frames", frame), [xc - 2.5, xc + 2.5], [sill, head - fr], [zf, zf + 6.0], 0.0);
                bx(cx.parts.slot("window frames", frame), [xl, xr], [spring - 3.0, spring + 3.0], [zf, zf + 6.0], 0.0);
            }
            if dress && !pointed {
                bx(cx.parts.slot("sills and lintels", l.trim), [xc - 9.0, xc + 9.0], [head, head + 22.0], [z - 8.0, z + t * 0.5], -1.0);
            }
            bx(cx.parts.slot("sills and lintels", l.trim), [xl - 8.0, xr + 8.0], [sill - 9.0, sill], [z - 10.0, z + t * 0.6], -1.0);
        }
        Win::Slit => {
            let (a, b) = (xc - w * 0.16, xc + w * 0.16);
            pane(cx, [a, b], [sill, head], z + t * 0.6);
            for (x0, x1) in [(a - fr, a), (b, b + fr)] {
                bx(cx.parts.slot("window frames", frame), [x0, x1], [sill - fr, head + fr], [zf, zf + 8.0], 0.0);
            }
            bx(cx.parts.slot("window frames", frame), [a - fr, b + fr], [sill - fr, sill], [zf, zf + 8.0], 0.0);
            bx(cx.parts.slot("window frames", frame), [a - fr, b + fr], [head, head + fr], [zf, zf + 8.0], 0.0);
        }
        Win::Lattice => {
            pane(cx, [xl + fr, xr - fr], [sill + fr, head - fr], z + t * 0.7);
            bx(cx.parts.slot("window frames", frame), [xl, xl + fr], [sill, head], [zf, zf + 10.0], 0.0);
            bx(cx.parts.slot("window frames", frame), [xr - fr, xr], [sill, head], [zf, zf + 10.0], 0.0);
            bx(cx.parts.slot("window frames", frame), [xl, xr], [sill, sill + fr], [zf, zf + 10.0], 0.0);
            bx(cx.parts.slot("window frames", frame), [xl, xr], [head - fr, head], [zf, zf + 10.0], 0.0);
            let n = if cx.detail >= 3 { 6 } else { 4 };
            for i in 1..n {
                let x = xl + (xr - xl) * i as f32 / n as f32;
                bx(cx.parts.slot("window frames", frame), [x - 2.0, x + 2.0], [sill, head], [zf + 2.0, zf + 8.0], 0.0);
                let u = sill + (head - sill) * i as f32 / n as f32;
                bx(cx.parts.slot("window frames", frame), [xl, xr], [u - 2.0, u + 2.0], [zf + 2.0, zf + 8.0], 0.0);
            }
        }
        Win::Band => {
            bx(cx.parts.slot("window frames", frame), [xl - 4.0, xr + 4.0], [sill - 6.0, head + 6.0], [z - 4.0, z + t * 0.5], -1.0);
            let glow = l.accent;
            if cx.chance(cx.lit * 0.6 + 0.35) {
                bx(cx.parts.slot("glow strips", glow), [xl, xr], [sill + h * 0.3, head - h * 0.3], [z - 6.0, z - 3.0], 0.0);
            }
            bx(cx.parts.slot("window glass", l.glass), [xl, xr], [sill, head], [z - 3.0, z], 0.0);
        }
        _ => {
            pane(cx, [xl + fr, xr - fr], [sill + fr, head - fr], z + t * 0.55);
            for (x0, x1) in [(xl, xl + fr), (xr - fr, xr)] {
                bx(cx.parts.slot("window frames", frame), [x0, x1], [sill, head], [zf, zf + 8.0], 0.0);
            }
            bx(cx.parts.slot("window frames", frame), [xl, xr], [sill, sill + fr], [zf, zf + 8.0], 0.0);
            bx(cx.parts.slot("window frames", frame), [xl, xr], [head - fr, head], [zf, zf + 8.0], 0.0);
            if cx.detail >= 2 && w > 70.0 {
                bx(cx.parts.slot("window frames", frame), [xc - 2.5, xc + 2.5], [sill, head], [zf, zf + 6.0], 0.0);
                if h > 120.0 {
                    let u = sill + h * 0.62;
                    bx(cx.parts.slot("window frames", frame), [xl, xr], [u - 2.5, u + 2.5], [zf, zf + 6.0], 0.0);
                }
            }
            if cx.detail >= 3 && w > 90.0 {
                for k in [0.25f32, 0.75] {
                    bx(cx.parts.slot("window frames", frame), [xl + w * k - 1.5, xl + w * k + 1.5], [sill, head], [zf + 1.0, zf + 5.0], 0.0);
                }
            }
        }
    }
    if dress && matches!(kind, Win::Rect | Win::Slit | Win::Lattice) && l.style != Style::Brutalist {
        let ov = 9.0;
        bx(cx.parts.slot("sills and lintels", l.trim), [xl - ov, xr + ov], [sill - 9.0, sill], [z - 12.0, z + t * 0.6], -1.0);
        bx(cx.parts.slot("sills and lintels", l.trim), [xl - ov, xr + ov], [head, head + 11.0], [z - 6.0, z + t * 0.6], -1.0);
        if l.style == Style::Classical && floor % 2 == 1 && cx.detail >= 2 {
            prism(cx.parts.slot("sills and lintels", l.trim), &[[xl - 12.0, head + 11.0], [xr + 12.0, head + 11.0], [xc, head + 11.0 + w * 0.2]], [z - 10.0, z + t * 0.3]);
        }
    }
    if dress && l.shutters && kind != Win::Band {
        let pw = (w * 0.5).min(60.0);
        for (a, b) in [(xl - pw - 3.0, xl - 3.0), (xr + 3.0, xr + pw + 3.0)] {
            if cx.chance(cx.wear * 0.25) {
                continue;
            }
            let tilt = if cx.chance(cx.wear * 0.4) { 14.0 } else { 0.0 };
            bx(cx.parts.slot("shutters", l.accent), [a, b], [sill, head - tilt], [z - 4.0, z + 6.0], -1.0);
        }
    }
    if dress && l.style == Style::Rustic && floor == 0 && cx.detail >= 2 {
        bx(cx.parts.slot("window boxes", l.accent), [xl - 6.0, xr + 6.0], [sill - 26.0, sill - 6.0], [z - 30.0, z - 4.0], -1.0);
        let leaf = Mat::new([60, 110, 50], 0.9, "leaf");
        bx(cx.parts.slot("plants", leaf), [xl, xr], [sill - 6.0, sill + 8.0], [z - 26.0, z - 8.0], 0.0);
    }
    if dress && cx.chance(cx.wear * 0.7) {
        let stain = Mat::new([48, 44, 40], 1.0, "concrete");
        bx(cx.parts.slot("stains", stain), [xc - w * 0.3, xc + w * 0.3], [sill - 120.0, sill - 12.0], [z - 12.5, z - 12.0], 0.0);
    }
}

fn balcony(cx: &mut Ctx, xc: f32, w: f32, u: f32, z: f32) {
    let l = cx.look.clone();
    let (x0, x1) = (xc - w / 2.0, xc + w / 2.0);
    let depth = 70.0;
    bx(cx.parts.slot("balconies", l.trim), [x0, x1], [u - 14.0, u + 4.0], [z - depth, z + 4.0], -1.0);
    let rail = l.metal;
    let top = u + 98.0;
    bx(cx.parts.slot("balcony rails", rail), [x0 + 2.0, x1 - 2.0], [top - 5.0, top], [z - depth + 2.0, z - depth + 8.0], 0.0);
    bx(cx.parts.slot("balcony rails", rail), [x0 + 2.0, x1 - 2.0], [u + 6.0, u + 11.0], [z - depth + 2.0, z - depth + 8.0], 0.0);
    for (a, b) in [(x0 + 2.0, x0 + 8.0), (x1 - 8.0, x1 - 2.0)] {
        bx(cx.parts.slot("balcony rails", rail), [a, b], [u + 4.0, top], [z - depth + 2.0, z - depth + 8.0], 0.0);
        bx(cx.parts.slot("balcony rails", rail), [a, b], [top - 5.0, top], [z - depth + 2.0, z - 4.0], 0.0);
    }
    if cx.detail >= 2 {
        let step = if cx.detail >= 3 { 14.0 } else { 26.0 };
        let mut x = x0 + 16.0;
        while x < x1 - 14.0 {
            bx(cx.parts.slot("balcony rails", rail), [x, x + 3.0], [u + 11.0, top - 5.0], [z - depth + 3.0, z - depth + 6.0], 0.0);
            x += step;
        }
    }
}

pub(super) fn door(cx: &mut Ctx, xc: f32, w: f32, h: f32, z: f32, t: f32, dressed: bool) {
    let l = cx.look.clone();
    let (xl, xr) = (xc - w / 2.0, xc + w / 2.0);
    let fr = 12.0;
    let zf = z + t * 0.4;
    match l.doork {
        Door::Glass => {
            bx(cx.parts.slot("door frames", l.frame), [xl, xl + fr], [0.0, h], [zf, zf + 10.0], 0.0);
            bx(cx.parts.slot("door frames", l.frame), [xr - fr, xr], [0.0, h], [zf, zf + 10.0], 0.0);
            bx(cx.parts.slot("door frames", l.frame), [xl, xr], [h - fr, h], [zf, zf + 10.0], 0.0);
            bx(cx.parts.slot("door frames", l.frame), [xc - 3.0, xc + 3.0], [0.0, h - fr], [zf, zf + 10.0], 0.0);
            bx(cx.parts.slot("window glass", l.glass), [xl + fr, xr - fr], [4.0, h - fr], [zf + 3.0, zf + 6.0], 0.0);
            for s in [-1.0f32, 1.0] {
                bx(cx.parts.slot("door handles", l.metal), [xc + s * 14.0 - 2.0, xc + s * 14.0 + 2.0], [h * 0.4, h * 0.4 + 45.0], [zf - 6.0, zf - 2.0], 0.0);
            }
            bx(cx.parts.slot("canopies", l.metal), [xl - 30.0, xr + 30.0], [h + 14.0, h + 26.0], [z - 130.0, z + 4.0], -1.0);
        }
        Door::Slab => {
            bx(cx.parts.slot("doors", l.door), [xl + fr, xr - fr], [0.0, h - fr], [zf, zf + 14.0], -1.0);
            bx(cx.parts.slot("canopies", l.trim), [xl - 70.0, xr + 70.0], [h + 8.0, h + 50.0], [z - 160.0, z + 4.0], -1.0);
            bx(cx.parts.slot("door handles", l.metal), [xc - 3.0, xc + 3.0], [h * 0.45, h * 0.45 + 40.0], [zf - 8.0, zf - 2.0], 0.0);
        }
        Door::Gate => {
            for s in [(xl + fr, xc - 1.5), (xc + 1.5, xr - fr)] {
                bx(cx.parts.slot("doors", l.door), [s.0, s.1], [0.0, h - fr], [zf, zf + 12.0], -1.0);
                if cx.detail >= 2 {
                    for r in 0..3 {
                        for c in 0..3 {
                            let (sx, su) = (s.0 + (s.1 - s.0) * (c as f32 + 0.5) / 3.0, (h - fr) * (r as f32 + 0.6) / 3.2);
                            bx(cx.parts.slot("door studs", l.accent), [sx - 4.0, sx + 4.0], [su - 4.0, su + 4.0], [zf - 3.0, zf], 0.0);
                        }
                    }
                }
            }
            for x in [(xl, xl + fr), (xr - fr, xr)] {
                column(cx.parts.slot("columns", l.frame), (x.0 + x.1) / 2.0, z - 20.0, 0.0, h + 40.0, 15.0, 15.0, 12);
            }
            bx(cx.parts.slot("lintels", l.trim), [xl - 30.0, xr + 30.0], [h + 30.0, h + 60.0], [z - 50.0, z + 10.0], -1.0);
        }
        Door::Arched | Door::Plank => {
            let arch = l.doork == Door::Arched;
            let spring = if arch { h - w / 2.0 } else { h };
            bx(cx.parts.slot("doors", l.door), [xl + fr, xr - fr], [0.0, spring], [zf, zf + 12.0], -1.0);
            if arch {
                let (ar, al) = arch_halves(false, xl, xr, spring, fr);
                let mut leaf = vec![[xl + fr, spring], [xr - fr, spring]];
                leaf.extend(ar.iter().cloned());
                leaf.extend(al.iter().cloned());
                prism(cx.parts.slot("doors", l.door), &leaf, [zf, zf + 12.0]);
                let (ar, al) = arch_halves(false, xl, xr, spring, 0.0);
                let mut right = vec![[xr, h]];
                right.extend(ar.iter().cloned());
                let mut left = vec![[xl, h]];
                left.extend(al.iter().cloned());
                for poly in [right, left] {
                    prism(cx.parts.slot("walls", l.wall), &poly, [z, z + t]);
                }
            }
            if cx.detail >= 2 {
                for k in [0.25f32, 0.7] {
                    bx(cx.parts.slot("door straps", l.metal), [xl + fr, xr - fr], [spring * k - 6.0, spring * k + 6.0], [zf - 3.0, zf], 0.0);
                }
                bx(cx.parts.slot("door handles", l.metal), [xc + w * 0.22 - 4.0, xc + w * 0.22 + 4.0], [h * 0.4, h * 0.4 + 10.0], [zf - 8.0, zf - 2.0], 0.0);
            }
            bx(cx.parts.slot("steps", l.trim), [xl - 20.0, xr + 20.0], [0.0, 16.0], [z - 60.0, z], -1.0);
        }
        Door::Portal => {
            bx(cx.parts.slot("doors", l.door), [xl + fr, xc - 1.5], [0.0, h - fr], [zf, zf + 12.0], -1.0);
            bx(cx.parts.slot("doors", l.door), [xc + 1.5, xr - fr], [0.0, h - fr], [zf, zf + 12.0], -1.0);
            if cx.detail >= 2 {
                for (a, b) in [(xl + fr + 8.0, xc - 9.0), (xc + 9.0, xr - fr - 8.0)] {
                    bx(cx.parts.slot("door panels", l.door), [a, b], [h * 0.1, h * 0.46], [zf - 3.0, zf], 0.0);
                    bx(cx.parts.slot("door panels", l.door), [a, b], [h * 0.52, h - fr - 18.0], [zf - 3.0, zf], 0.0);
                }
            }
            bx(cx.parts.slot("door frames", l.trim), [xl, xl + fr], [0.0, h], [z - 10.0, z + t * 0.6], -1.0);
            bx(cx.parts.slot("door frames", l.trim), [xr - fr, xr], [0.0, h], [z - 10.0, z + t * 0.6], -1.0);
            bx(cx.parts.slot("door frames", l.trim), [xl, xr], [h - fr, h], [z - 10.0, z + t * 0.6], -1.0);
            for k in 0..3 {
                let e = 24.0 + k as f32 * 20.0;
                bx(cx.parts.slot("steps", l.trim), [xl - e, xr + e], [0.0, 48.0 - 16.0 * k as f32], [z - 30.0 - e, z + 4.0], -1.0);
            }
            bx(cx.parts.slot("door handles", l.metal), [xc - 14.0, xc - 10.0], [h * 0.42, h * 0.42 + 12.0], [zf - 8.0, zf - 2.0], 0.0);
            if dressed && l.pilasters {
                for x in [xl - 18.0, xr + 18.0] {
                    column(cx.parts.slot("columns", l.trim), x, z - 62.0, 58.0, h + 40.0, 16.0, 14.0, 14);
                    bx(cx.parts.slot("columns", l.trim), [x - 22.0, x + 22.0], [48.0, 58.0], [z - 84.0, z - 40.0], -1.0);
                    bx(cx.parts.slot("columns", l.trim), [x - 22.0, x + 22.0], [h + 40.0, h + 52.0], [z - 84.0, z - 40.0], -1.0);
                }
                bx(cx.parts.slot("columns", l.trim), [xl - 44.0, xr + 44.0], [h + 52.0, h + 78.0], [z - 90.0, z - 34.0], -1.0);
                prism(cx.parts.slot("columns", l.trim), &[[xl - 50.0, h + 78.0], [xr + 50.0, h + 78.0], [xc, h + 78.0 + (xr - xl + 100.0) * 0.14]], [z - 94.0, z - 40.0]);
            }
        }
    }
}

/// The front or a side: a dressed wall from `x0` for `w` across, its outer
/// face at `z`, `floors` storeys. The ground is height 0.
pub fn facade(cx: &mut Ctx, x0: f32, w: f32, z: f32, floors: usize, o: Face) {
    let l = cx.look.clone();
    let n = ((w / l.bay).round() as usize).max(1);
    let bw = w / n as f32;
    let t = l.thick;
    let door_bay = o.door_bay.unwrap_or(n / 2).min(n - 1);
    let mut y = 0.0;
    for s in 0..floors {
        let h = l.storey_h(s);
        let (y0, y1) = (y, y + h);
        y = y1;
        let jz = z - if o.jet { l.jetty * s as f32 } else { 0.0 };
        let ground = s == 0;
        match l.window {
            Win::Curtain => {
                curtain_storey(cx, x0, w, jz, y0, y1, o, ground, door_bay, n, bw);
                continue;
            }
            Win::Ribbon => ribbon_storey(cx, x0, w, jz, y0, y1, o, ground, door_bay, n, bw),
            _ => {
                struct W {
                    xc: f32,
                    w: f32,
                    sill: f32,
                    head: f32,
                    door: bool,
                }
                let mut ws: Vec<W> = Vec::new();
                for i in 0..n {
                    let xc = x0 + (i as f32 + 0.5) * bw;
                    if ground && o.dressed && i == door_bay {
                        let dw = (bw * 0.5).clamp(90.0, 170.0);
                        let dh = (h * 0.74).min(300.0);
                        ws.push(W { xc, w: dw, sill: 0.0, head: dh, door: true });
                        continue;
                    }
                    if ground && o.shop {
                        ws.push(W { xc, w: bw * 0.84, sill: h * 0.14, head: h * 0.78, door: false });
                        continue;
                    }
                    let ww = (bw * l.win_w).max(40.0);
                    let wh = h * l.win_h;
                    let sill = y0 + h * l.sill + if ground { h * 0.08 } else { 0.0 };
                    let head = if matches!(l.window, Win::Arched | Win::Pointed) { sill + wh.max(ww * 1.3) } else { sill + wh };
                    ws.push(W { xc, w: if l.window == Win::Slit { ww * 0.7 } else { ww }, sill, head: head.min(y1 - 28.0), door: false });
                }
                let ops: Vec<Opening> = ws.iter().map(|k| Opening { x0: k.xc - k.w / 2.0, x1: k.xc + k.w / 2.0, sill: k.sill, head: k.head }).collect();
                tile(cx, x0, x0 + w, [y0, y1], [jz, jz + t], &ops);
                for k in &ws {
                    if k.door {
                        door(cx, k.xc, k.w, k.head, jz, t, o.dressed);
                    } else if o.shop && ground {
                        shop_window(cx, k.xc, k.w, k.sill, k.head, jz, t);
                    } else {
                        window(cx, l.window, k.xc, k.w, k.sill, k.head, jz, t, o.dressed, s);
                        if o.dressed && s > 0 && l.balcony > 0.0 && s + 1 < floors && cx.chance(l.balcony * 0.6) {
                            balcony(cx, k.xc, k.w * 1.5, y0, jz);
                        }
                    }
                }
            }
        }
        if l.courses && s + 1 < floors {
            bx(cx.parts.slot("string courses", l.trim), [x0 - 6.0, x0 + w + 6.0], [y1 - 7.0, y1 + 7.0], [jz - 10.0, jz + t], -1.0);
        }
        if l.style == Style::Brutalist {
            bx(cx.parts.slot("slab edges", l.trim), [x0 - 10.0, x0 + w + 10.0], [y1 - 28.0, y1], [jz - 40.0, jz + t], -1.0);
        }
        if l.timber {
            timber(cx, x0, w, jz, y0, y1, n, bw, t, s);
        }
        if l.cornice == Cornice::Pagoda {
            eave(cx, x0, w, jz, y1, t);
        }
        if l.jetty > 0.0 && o.jet && s + 1 < floors {
            let k = ((w / 90.0) as usize).max(1);
            for i in 0..=k {
                let x = x0 + 20.0 + (w - 40.0) * i as f32 / k as f32;
                bx(cx.parts.slot("timber frame", l.frame), [x - 8.0, x + 8.0], [y1 - 34.0, y1], [jz - l.jetty, jz - 2.0], -1.0);
            }
        }
    }
    let top = y;
    if o.dressed && l.pilasters {
        pilasters(cx, x0, z, n, bw, l.height_of(1), top);
    }
    if l.fins {
        fins(cx, x0, z, n, bw, top, o.dressed);
    }
    if l.timber {
        bx(cx.parts.slot("plinth", l.trim), [x0 - 4.0, x0 + w + 4.0], [0.0, 40.0], [z - 6.0, z + t], -1.0);
    } else if o.dressed && matches!(l.style, Style::Classical | Style::Rustic | Style::ArtDeco) {
        bx(cx.parts.slot("plinth", l.trim), [x0 - 6.0, x0 + w + 6.0], [0.0, 46.0], [z - 12.0, z + t], -1.0);
    }
    if cx.wear > 0.15 {
        bx(cx.parts.slot("grime", Mat::new([40, 36, 32], 1.0, "concrete")), [x0, x0 + w], [0.0, 30.0 + cx.wear * 70.0], [z - 12.4, z - 12.2], 0.0);
    }
    if o.top {
        cornice(cx, x0, w, z, top, t, o.dressed);
    }
    if cx.wear > 0.3 {
        patches(cx, x0, w, z, top);
    }
}

fn shop_window(cx: &mut Ctx, xc: f32, w: f32, sill: f32, head: f32, z: f32, t: f32) {
    let l = cx.look.clone();
    let (xl, xr) = (xc - w / 2.0, xc + w / 2.0);
    let zf = z + t * 0.4;
    pane(cx, [xl + 8.0, xr - 8.0], [sill + 8.0, head - 8.0], z + t * 0.55);
    for (a, b) in [(xl, xl + 8.0), (xr - 8.0, xr)] {
        bx(cx.parts.slot("window frames", l.frame), [a, b], [sill, head], [zf, zf + 10.0], 0.0);
    }
    bx(cx.parts.slot("window frames", l.frame), [xl, xr], [sill, sill + 8.0], [zf, zf + 10.0], 0.0);
    bx(cx.parts.slot("window frames", l.frame), [xl, xr], [head - 8.0, head], [zf, zf + 10.0], 0.0);
    bx(cx.parts.slot("awnings", l.accent), [xl - 6.0, xr + 6.0], [head + 4.0, head + 18.0], [z - 90.0, z + 2.0], -1.0);
    let sign = Mat::glowing([255, 190, 90], 1.8);
    bx(cx.parts.slot("signs", sign), [xl + 10.0, xr - 10.0], [head + 24.0, head + 52.0], [z - 8.0, z - 4.0], 0.0);
}

#[allow(clippy::too_many_arguments)]
fn curtain_storey(cx: &mut Ctx, x0: f32, w: f32, z: f32, y0: f32, y1: f32, o: Face, ground: bool, door_bay: usize, n: usize, bw: f32) {
    let l = cx.look.clone();
    let t = l.thick;
    let spandrel = 62.0;
    bx(cx.parts.slot("slab edges", l.wall), [x0 - 4.0, x0 + w + 4.0], [y0 - 4.0, y0 + 26.0], [z - 14.0, z + t], -1.0);
    bx(cx.parts.slot("spandrel panels", Mat::metal([62, 74, 86], 0.3)), [x0, x0 + w], [y0 + 26.0, y0 + spandrel], [z - 4.0, z + 4.0], 0.0);
    let m = l.frame;
    let (g0, g1) = (y0 + spandrel, y1 - 4.0);
    for i in 0..n {
        let (a, b) = (x0 + i as f32 * bw, x0 + (i + 1) as f32 * bw);
        if ground && o.dressed && i == door_bay {
            door(cx, (a + b) / 2.0, bw.min(190.0), (g1 - y0).min(300.0), z, t, true);
            continue;
        }
        pane(cx, [a + 3.0, b - 3.0], [g0, g1], z + 2.0);
        bx(cx.parts.slot("mullions", m), [a - 3.0, a + 3.0], [g0, g1], [z - 8.0, z + 6.0], 0.0);
    }
    bx(cx.parts.slot("mullions", m), [x0 + w - 3.0, x0 + w + 3.0], [g0, g1], [z - 8.0, z + 6.0], 0.0);
    bx(cx.parts.slot("mullions", m), [x0, x0 + w], [g1 - 3.0, g1 + 1.0], [z - 8.0, z + 6.0], 0.0);
    if cx.detail >= 2 {
        bx(cx.parts.slot("mullions", m), [x0, x0 + w], [(g0 + g1) / 2.0 - 2.0, (g0 + g1) / 2.0 + 2.0], [z - 6.0, z + 5.0], 0.0);
    }
}

#[allow(clippy::too_many_arguments)]
fn ribbon_storey(cx: &mut Ctx, x0: f32, w: f32, z: f32, y0: f32, y1: f32, o: Face, ground: bool, door_bay: usize, n: usize, bw: f32) {
    let l = cx.look.clone();
    let t = l.thick;
    let h = y1 - y0;
    let sill = y0 + h * l.sill;
    let head = sill + h * l.win_h;
    let pier = (bw * 0.18).max(40.0);
    let mut ops: Vec<Opening> = Vec::new();
    let mut at = x0 + pier;
    for i in 0..n {
        let b1 = x0 + (i as f32 + 1.0) * bw - if i + 1 == n { pier } else { 0.0 };
        let (a0, a1) = (at, if i + 1 == n { b1 } else { b1 - pier * 0.5 });
        if ground && o.dressed && i == door_bay {
            let dw = (bw * 0.45).clamp(100.0, 180.0);
            let xc = (a0 + a1) / 2.0;
            ops.push(Opening { x0: xc - dw / 2.0, x1: xc + dw / 2.0, sill: y0, head: y0 + (h * 0.72).min(290.0) });
        } else {
            ops.push(Opening { x0: a0, x1: a1, sill, head });
        }
        at = b1 + pier * 0.5;
    }
    tile(cx, x0, x0 + w, [y0, y1], [z, z + t], &ops);
    for (i, op) in ops.iter().enumerate() {
        let xc = (op.x0 + op.x1) / 2.0;
        if ground && o.dressed && i == door_bay {
            door(cx, xc, op.x1 - op.x0, op.head - y0, z, t, true);
        } else {
            window(cx, Win::Ribbon, xc, op.x1 - op.x0, op.sill, op.head, z, t, o.dressed, 0);
            let step = bw * 0.5;
            let mut x = op.x0 + step;
            while x < op.x1 - 10.0 {
                bx(cx.parts.slot("window frames", l.frame), [x - 4.0, x + 4.0], [op.sill, op.head], [z + t * 0.6, z + t * 0.6 + 8.0], 0.0);
                x += step;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn timber(cx: &mut Ctx, x0: f32, w: f32, z: f32, y0: f32, y1: f32, n: usize, bw: f32, t: f32, s: usize) {
    let m = cx.look.frame;
    for i in 0..=n {
        let x = x0 + i as f32 * bw;
        bx(cx.parts.slot("timber frame", m), [x - 8.0, x + 8.0], [y0, y1], [z - 5.0, z + t * 0.5], -1.0);
    }
    bx(cx.parts.slot("timber frame", m), [x0 - 8.0, x0 + w + 8.0], [y0, y0 + 16.0], [z - 5.0, z + t * 0.5], -1.0);
    bx(cx.parts.slot("timber frame", m), [x0 - 8.0, x0 + w + 8.0], [y1 - 16.0, y1], [z - 5.0, z + t * 0.5], -1.0);
    if s > 0 || n > 2 {
        for i in 0..n {
            if cx.chance(0.6) {
                let (a, b) = (x0 + i as f32 * bw + 8.0, x0 + (i + 1) as f32 * bw - 8.0);
                let (u0, u1) = (y0 + 16.0, y1 - 16.0);
                let flip = cx.chance(0.5);
                let (xa, xb, d) = if flip { (b, a, -12.0) } else { (a, b, 12.0) };
                prism(cx.parts.slot("timber frame", m), &[[xa, u0], [xa + d, u0], [xb, u1], [xb - d, u1]], [z - 3.0, z + t * 0.4]);
            }
        }
    }
}

fn eave(cx: &mut Ctx, x0: f32, w: f32, z: f32, u: f32, t: f32) {
    let l = cx.look.clone();
    hull(cx.parts.slot("roof tiles", l.roof), ([x0 - 100.0, x0 + w + 100.0], [z - 130.0, z + t + 30.0]), ([x0 - 10.0, x0 + w + 10.0], [z + 10.0, z + t]), [u - 10.0, u + 70.0]);
    bx(cx.parts.slot("beams", l.trim), [x0 - 90.0, x0 + w + 90.0], [u - 24.0, u - 10.0], [z - 120.0, z + t + 20.0], -1.0);
    let k = ((w / 110.0) as usize).max(1);
    for i in 0..=k {
        let x = x0 + 30.0 + (w - 60.0) * i as f32 / k as f32;
        bx(cx.parts.slot("brackets", l.accent), [x - 12.0, x + 12.0], [u - 44.0, u - 24.0], [z - 70.0, z - 10.0], -1.0);
    }
}

fn pilasters(cx: &mut Ctx, x0: f32, z: f32, n: usize, bw: f32, from: f32, top: f32) {
    let m = cx.look.trim;
    let wall = cx.look.wall;
    for i in 0..=n {
        let x = x0 + i as f32 * bw;
        let half = 17.0;
        let (a, b) = if i == 0 { (x - 4.0, x + 2.0 * half) } else if i == n { (x - 2.0 * half, x + 4.0) } else { (x - half, x + half) };
        let zr = [z - 17.0, z + 4.0];
        bx(cx.parts.slot("pilasters", m), [a, b], [from, top - 20.0], zr, -1.0);
        bx(cx.parts.slot("pilasters", m), [a - 4.0, b + 4.0], [from, from + 14.0], [zr[0] - 3.0, zr[1]], -1.0);
        bx(cx.parts.slot("pilasters", m), [a - 6.0, b + 6.0], [top - 20.0, top - 8.0], [zr[0] - 5.0, zr[1]], -1.0);
        bx(cx.parts.slot("pilasters", m), [a - 9.0, b + 9.0], [top - 8.0, top], [zr[0] - 8.0, zr[1]], -1.0);
        if cx.detail >= 3 {
            for k in 1..4 {
                let fx = a + (b - a) * k as f32 / 4.0;
                bx(cx.parts.slot("pilaster flutes", wall), [fx - 1.5, fx + 1.5], [from + 20.0, top - 28.0], [zr[0] - 0.8, zr[0] + 0.2], 0.0);
            }
        }
    }
}

fn fins(cx: &mut Ctx, x0: f32, z: f32, n: usize, bw: f32, top: f32, dressed: bool) {
    let l = cx.look.clone();
    let proj = if l.style == Style::Brutalist { 48.0 } else if l.style == Style::ArtDeco { 44.0 } else { 26.0 };
    let wid = if l.style == Style::Brutalist { 34.0 } else if l.style == Style::ArtDeco { 30.0 } else { 20.0 };
    for i in 0..=n {
        let x = x0 + i as f32 * bw;
        let rise = if l.style == Style::ArtDeco && dressed { 70.0 + (i % 2) as f32 * 40.0 } else { 0.0 };
        bx(cx.parts.slot("fins", l.trim), [x - wid / 2.0, x + wid / 2.0], [0.0, top + rise], [z - proj, z + 4.0], -1.0);
    }
    if l.style == Style::ArtDeco && dressed && cx.detail >= 2 {
        let mut u = l.height_of(1);
        while u < top - 40.0 {
            for i in 0..n {
                let xc = x0 + (i as f32 + 0.5) * bw;
                prism(cx.parts.slot("gilt spandrels", l.accent), &[[xc - bw * 0.3, u], [xc, u + 26.0], [xc + bw * 0.3, u], [xc + bw * 0.3, u + 8.0], [xc, u + 34.0], [xc - bw * 0.3, u + 8.0]], [z - 3.0, z + 1.0]);
            }
            u += l.storey;
        }
    }
}

fn cornice(cx: &mut Ctx, x0: f32, w: f32, z: f32, top: f32, t: f32, dressed: bool) {
    let l = cx.look.clone();
    let m = l.trim;
    let (a, b) = (x0 - 8.0, x0 + w + 8.0);
    match l.cornice {
        Cornice::Classical => {
            bx(cx.parts.slot("cornice", m), [a, b], [top, top + 26.0], [z - 8.0, z + t], -1.0);
            bx(cx.parts.slot("cornice", m), [a - 6.0, b + 6.0], [top + 26.0, top + 50.0], [z - 46.0, z + t], -1.0);
            bx(cx.parts.slot("cornice", m), [a - 12.0, b + 12.0], [top + 50.0, top + 62.0], [z - 56.0, z + t], -1.0);
            if cx.detail >= 2 {
                let mut x = a;
                while x < b - 12.0 {
                    if !cx.chance(cx.wear * 0.4) {
                        bx(cx.parts.slot("dentils", m), [x, x + 10.0], [top + 14.0, top + 26.0], [z - 34.0, z - 24.0], 0.0);
                    }
                    x += 22.0;
                }
            }
            if dressed && cx.detail >= 3 {
                bx(cx.parts.slot("balustrade", m), [a - 6.0, b + 6.0], [top + 62.0, top + 72.0], [z - 36.0, z - 6.0], -1.0);
                let mut x = a;
                while x < b - 14.0 {
                    column(cx.parts.slot("balustrade", m), x + 7.0, z - 21.0, top + 72.0, top + 128.0, 7.0, 5.5, 8);
                    x += 24.0;
                }
                bx(cx.parts.slot("balustrade", m), [a - 6.0, b + 6.0], [top + 128.0, top + 138.0], [z - 36.0, z - 6.0], -1.0);
            } else {
                bx(cx.parts.slot("parapet", l.wall), [a, b], [top + 62.0, top + 120.0], [z - 14.0, z + t * 0.8], -1.0);
            }
        }
        Cornice::Deco => {
            for (k, (inset, h)) in [(0.0f32, 30.0f32), (14.0, 22.0), (28.0, 18.0)].iter().enumerate() {
                let u0 = top + [0.0, 30.0, 52.0][k];
                bx(cx.parts.slot("cornice", m), [a + inset, b - inset], [u0, u0 + h], [z - 30.0 + inset, z + t], -1.0);
            }
            bx(cx.parts.slot("cornice", l.accent), [a, b], [top + 6.0, top + 10.0], [z - 31.0, z - 29.0], 0.0);
        }
        Cornice::Slab => {
            bx(cx.parts.slot("cornice", m), [a - 14.0, b + 14.0], [top, top + 34.0], [z - 55.0, z + t], -1.0);
            if l.style == Style::ModernGlass {
                bx(cx.parts.slot("parapet", l.frame), [a, b], [top + 34.0, top + 44.0], [z - 8.0, z + 4.0], 0.0);
            }
        }
        Cornice::Eaves | Cornice::Pagoda | Cornice::None => {
            bx(cx.parts.slot("cornice", m), [a, b], [top, top + 14.0], [z - 14.0, z + t], -1.0);
        }
    }
}

/// Lighter and darker patches where plaster or paint has gone.
fn patches(cx: &mut Ctx, x0: f32, w: f32, z: f32, top: f32) {
    let wall = cx.look.wall;
    let lighter = Mat { rgb: [0, 1, 2].map(|i| wall.rgb[i].saturating_add(18)), ..wall };
    let darker = Mat { rgb: [0, 1, 2].map(|i| wall.rgb[i].saturating_sub(42)), ..wall };
    let count = (cx.wear * w / 90.0 * (top / 400.0)).round() as usize;
    for _ in 0..count {
        let x = cx.rng.range(x0 + 20.0, (x0 + w - 60.0).max(x0 + 21.0));
        let u = cx.rng.range(60.0, (top - 120.0).max(70.0));
        let (pw, ph) = (cx.rng.range(30.0, 90.0), cx.rng.range(24.0, 70.0));
        let light = cx.chance(0.5);
        let (m, name) = if light { (lighter, "plaster patches") } else { (darker, "damp patches") };
        bx(cx.parts.slot(name, m), [x, x + pw], [u, u + ph], [z - 13.0, z - 12.0], 0.0);
    }
}

/// A fire escape: landings, rails, a ladder and zig-zag stairs hung on the
/// wall at `z`, from the first floor up.
pub fn fire_escape(cx: &mut Ctx, xc: f32, z: f32, floors: usize) {
    let l = cx.look.clone();
    let m = Mat::metal([38, 40, 42], 0.55);
    let wd = 190.0;
    for s in 1..floors {
        let u = l.height_of(s) - 6.0;
        bx(cx.parts.slot("fire escape", m), [xc - wd / 2.0, xc + wd / 2.0], [u - 6.0, u], [z - 110.0, z - 6.0], 0.0);
        let top = u + 100.0;
        bx(cx.parts.slot("fire escape", m), [xc - wd / 2.0, xc + wd / 2.0], [top - 4.0, top], [z - 110.0, z - 105.0], 0.0);
        for x in [xc - wd / 2.0, xc + wd / 2.0 - 4.0] {
            bx(cx.parts.slot("fire escape", m), [x, x + 4.0], [u, top], [z - 110.0, z - 106.0], 0.0);
        }
        bx(cx.parts.slot("fire escape", m), [xc - wd / 2.0, xc - wd / 2.0 + 4.0], [top - 4.0, top], [z - 110.0, z - 6.0], 0.0);
        bx(cx.parts.slot("fire escape", m), [xc + wd / 2.0 - 4.0, xc + wd / 2.0], [top - 4.0, top], [z - 110.0, z - 6.0], 0.0);
        if cx.detail >= 2 {
            let mut x = xc - wd / 2.0 + 14.0;
            while x < xc + wd / 2.0 - 10.0 {
                bx(cx.parts.slot("fire escape", m), [x, x + 2.5], [u, top - 4.0], [z - 109.0, z - 107.0], 0.0);
                x += 22.0;
            }
        }
        if s > 1 {
            let u_lo = l.height_of(s - 1) - 6.0;
            let (xa, xb) = if s % 2 == 0 { (xc - wd / 2.0 + 10.0, xc - wd / 2.0 + 60.0) } else { (xc + wd / 2.0 - 60.0, xc + wd / 2.0 - 10.0) };
            let steps = 8;
            for k in 0..steps {
                let f = k as f32 / steps as f32;
                let uu = u_lo + (u - u_lo) * f;
                bx(cx.parts.slot("fire escape", m), [xa, xb], [uu, uu + 3.0], [z - 100.0 + f * 80.0, z - 100.0 + f * 80.0 + 14.0], 0.0);
            }
            tube(cx.parts.slot("fire escape", m), pt(xa, u_lo + 80.0, z - 100.0), pt(xa, u + 80.0, z - 18.0), 2.5, 2.5, 6);
        }
    }
    let u1 = l.height_of(1);
    for x in [xc + wd / 2.0 - 40.0, xc + wd / 2.0 - 16.0] {
        bx(cx.parts.slot("fire escape", m), [x, x + 3.0], [u1 - 220.0, u1 - 6.0], [z - 60.0, z - 56.0], 0.0);
    }
    for k in 0..8 {
        let u = u1 - 210.0 + k as f32 * 26.0;
        bx(cx.parts.slot("fire escape", m), [xc + wd / 2.0 - 40.0, xc + wd / 2.0 - 13.0], [u, u + 3.0], [z - 60.0, z - 56.0], 0.0);
    }
}
