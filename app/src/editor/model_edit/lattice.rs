// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Editing a lattice modifier's cage by hand: its control points drawn
//! over the model, picked with clicks (Shift adds) or A, and dragged in
//! the view (or moved exactly with typed offsets in the Inspector); the
//! model bends as they move. Enter or Esc ends it. A drag is one undo
//! step.

use super::draw::Space;
use super::{Stage, with};
use crate::app::StudioApp;
use crate::editor::viewport::ui::Pane;
use eframe::egui::{self, Color32, Key, Modifiers, Pos2, Stroke};
use studio_mesh::V3;
use studio_modeling::modifiers::{Lattice, Modifier};

/// A lattice cage being edited.
#[derive(Clone, Debug, PartialEq)]
pub struct LatticeEdit {
    /// The modifier's place in the open part's stack.
    pub modifier: usize,
    pub picked: Vec<[usize; 3]>,
    /// While dragging: where it started and the moves then.
    drag: Option<(Pos2, Vec<([usize; 3], V3)>)>,
}

/// The cage's points: (index, where it is now), part space.
pub fn points(l: &Lattice) -> Vec<([usize; 3], V3)> {
    let res = [l.res[0].clamp(2, 6), l.res[1].clamp(2, 6), l.res[2].clamp(2, 6)];
    let (lo, hi) = (l.min.unwrap_or([0.0; 3]), l.max.unwrap_or([0.0; 3]));
    let mut out = Vec::new();
    for k in 0..res[2] {
        for j in 0..res[1] {
            for i in 0..res[0] {
                let ijk = [i, j, k];
                let base: V3 = std::array::from_fn(|a| lo[a] + (hi[a] - lo[a]) * ijk[a] as f32 / (res[a] - 1) as f32);
                let d = l.moves.iter().filter(|(p, _)| *p == ijk).fold([0.0; 3], |s, (_, d)| studio_mesh::add(s, *d));
                out.push((ijk, studio_mesh::add(base, d)));
            }
        }
    }
    out
}

fn lattice_of(s: &super::Session, i: usize) -> Option<Lattice> {
    match s.part().modifiers.get(i)? {
        Modifier::Lattice(l) => Some(l.clone()),
        _ => None,
    }
}

fn set_lattice(s: &mut super::Session, i: usize, l: Lattice) {
    let p = s.part;
    if let Some(Modifier::Lattice(x)) = s.asset.parts[p].modifiers.get_mut(i) {
        *x = l;
    }
}

/// Starts editing the cage of the open part's modifier `i` (its box is
/// fixed to the part's size first, so what is drawn is what bends).
pub fn start(app: &mut StudioApp, ctx: &egui::Context, i: usize) -> Result<(), String> {
    if super::stage() != Stage::Edit {
        super::enter(app, ctx, Stage::Edit)?;
    }
    let Some(s) = with(|s| s.clone()) else { return Err("no model is open".into()) };
    let mut l = lattice_of(&s, i).ok_or("that modifier is not a lattice")?;
    if l.min.is_none() || l.max.is_none() {
        let (lo, hi) = s.poly().bounds();
        let pad: V3 = std::array::from_fn(|a| (hi[a] - lo[a]).max(1.0) * 0.02);
        l.min = Some(std::array::from_fn(|a| lo[a] - pad[a]));
        l.max = Some(std::array::from_fn(|a| hi[a] + pad[a]));
        with(|cur| {
            if let Some(cur) = cur {
                set_lattice(cur, i, l.clone());
                cur.changed();
            }
        });
        super::commit(app, "Lattice cage")?;
    }
    with(|cur| {
        if let Some(cur) = cur {
            cur.lattice = Some(LatticeEdit { modifier: i, picked: Vec::new(), drag: None });
        }
    });
    Ok(())
}

pub fn stop() {
    with(|s| {
        if let Some(s) = s {
            s.lattice = None;
        }
    });
}

pub fn on() -> bool {
    with(|s| s.as_ref().is_some_and(|s| s.lattice.is_some()))
}

/// Keys while editing a cage: A picks every point, Alt+A none, Enter or
/// Esc ends it.
pub fn keys(ctx: &egui::Context) -> bool {
    if !on() {
        return false;
    }
    let pressed = |m: Modifiers, k: Key| ctx.input_mut(|i| i.modifiers.alt == m.alt && i.consume_key(m, k));
    if pressed(Modifiers::NONE, Key::Escape) || pressed(Modifiers::NONE, Key::Enter) {
        stop();
        return true;
    }
    if pressed(Modifiers::ALT, Key::A) {
        with(|s| {
            if let Some(e) = s.as_mut().and_then(|s| s.lattice.as_mut()) {
                e.picked.clear();
            }
        });
        return true;
    }
    if pressed(Modifiers::NONE, Key::A) {
        with(|s| {
            if let Some(s) = s
                && let Some(i) = s.lattice.as_ref().map(|e| e.modifier)
                && let Some(l) = lattice_of(s, i)
            {
                let all: Vec<[usize; 3]> = points(&l).into_iter().map(|p| p.0).collect();
                if let Some(e) = s.lattice.as_mut() {
                    e.picked = all;
                }
            }
        });
        return true;
    }
    false
}

/// Draws the cage.
pub fn overlay(app: &StudioApp, painter: &egui::Painter, pane: &Pane) {
    let Some(s) = with(|s| s.clone()) else { return };
    let Some(e) = &s.lattice else { return };
    let Some(l) = lattice_of(&s, e.modifier) else { return };
    let Some(space) = Space::of(app, &s) else { return };
    let pts = points(&l);
    let at = |ijk: [usize; 3]| pts.iter().find(|p| p.0 == ijk).map(|p| p.1);
    let res = [l.res[0].clamp(2, 6), l.res[1].clamp(2, 6), l.res[2].clamp(2, 6)];
    let line = Stroke::new(1.0, Color32::from_rgb(140, 200, 255));
    for (ijk, q) in &pts {
        for a in 0..3 {
            if ijk[a] + 1 < res[a] {
                let mut n = *ijk;
                n[a] += 1;
                if let (Some(sa), Some(nb)) = (pane.to_screen(space.world(*q)), at(n).and_then(|w| pane.to_screen(space.world(w)))) {
                    painter.line_segment([sa, nb], line);
                }
            }
        }
    }
    for (ijk, q) in &pts {
        if let Some(p) = pane.to_screen(space.world(*q)) {
            let picked = e.picked.contains(ijk);
            painter.rect_filled(egui::Rect::from_center_size(p, egui::vec2(7.0, 7.0)), 1.0, if picked { super::draw::PICKED } else { Color32::from_rgb(140, 200, 255) });
        }
    }
    let th = crate::theme::tokens(painter.ctx());
    painter.text(pane.rect.center_bottom() - egui::vec2(0.0, 30.0), egui::Align2::CENTER_BOTTOM, crate::i18n::t("Lattice: click points (Shift adds, A all), drag to bend · Enter or Esc ends"), crate::theme::caption(), th.text);
}

/// Picks and drags cage points. Returns whether the pointer was the cage's.
pub fn interact(app: &mut StudioApp, ctx: &egui::Context, pane: &Pane, resp: &egui::Response) -> bool {
    let Some(s) = with(|s| s.clone()) else { return false };
    let Some(e) = s.lattice.clone() else { return false };
    let Some(l) = lattice_of(&s, e.modifier) else {
        stop();
        return false;
    };
    let Some(space) = Space::of(app, &s) else { return true };
    let pts = points(&l);
    let pointer = ctx.input(|i| i.pointer.hover_pos());
    let nearest = |at: Pos2| -> Option<[usize; 3]> {
        pts.iter().filter_map(|(ijk, q)| pane.to_screen(space.world(*q)).map(|p| (p.distance(at), *ijk))).filter(|(d, _)| *d < 12.0).min_by(|a, b| a.0.total_cmp(&b.0)).map(|x| x.1)
    };
    let shift = ctx.input(|i| i.modifiers.shift);
    if resp.clicked()
        && let Some(p) = pointer
    {
        let hit = nearest(p);
        with(|cur| {
            if let Some(le) = cur.as_mut().and_then(|c| c.lattice.as_mut()) {
                match (hit, shift) {
                    (Some(h), true) => {
                        if let Some(i) = le.picked.iter().position(|x| *x == h) {
                            le.picked.remove(i);
                        } else {
                            le.picked.push(h);
                        }
                    }
                    (Some(h), false) => le.picked = vec![h],
                    (None, false) => le.picked.clear(),
                    (None, true) => {}
                }
            }
        });
    }
    if resp.drag_started()
        && let Some(p) = ctx.input(|i| i.pointer.press_origin())
        && let Some(h) = nearest(p)
    {
        let picked = if e.picked.contains(&h) { e.picked.clone() } else { vec![h] };
        let moves = l.moves.clone();
        with(|cur| {
            if let Some(le) = cur.as_mut().and_then(|c| c.lattice.as_mut()) {
                le.picked = picked;
                le.drag = Some((p, moves));
            }
        });
        if !app.doc.in_batch() {
            app.doc.begin_batch("Move lattice points");
        }
    }
    let dragging = with(|s| s.as_ref().and_then(|s| s.lattice.as_ref()).and_then(|e| e.drag.clone()));
    if let (Some((from, base)), Some(p)) = (dragging, pointer) {
        let picked = with(|s| s.as_ref().and_then(|s| s.lattice.as_ref()).map(|e| e.picked.clone())).unwrap_or_default();
        let anchor = pts.iter().find(|q| picked.first() == Some(&q.0)).map(|q| q.1).unwrap_or([0.0; 3]);
        let w = space.world(anchor);
        let depth = pane.view.view(w)[2].max(1.0);
        let wpp = depth / pane.view.zoom / pane.scale as f64;
        let d = p - from;
        let dw: [f64; 3] = std::array::from_fn(|j| (pane.view.axes[0][j] * d.x as f64 + pane.view.axes[1][j] * d.y as f64) * wpp);
        let dl = studio_mesh::sub(space.to_local([w[0] + dw[0], w[1] + dw[1], w[2] + dw[2]]), space.to_local(w));
        let mut nl = l.clone();
        nl.moves = base.clone();
        for ijk in &picked {
            nl.moves.push((*ijk, dl));
        }
        // One move per point.
        let mut merged: Vec<([usize; 3], V3)> = Vec::new();
        for (ijk, v) in nl.moves {
            match merged.iter_mut().find(|m| m.0 == ijk) {
                Some(m) => m.1 = studio_mesh::add(m.1, v),
                None => merged.push((ijk, v)),
            }
        }
        nl.moves = merged;
        with(|cur| {
            if let Some(cur) = cur {
                set_lattice(cur, e.modifier, nl);
            }
        });
        super::preview(app);
        ctx.request_repaint();
    }
    if ctx.input(|i| i.pointer.any_released()) && with(|s| s.as_ref().and_then(|s| s.lattice.as_ref()).is_some_and(|e| e.drag.is_some())) {
        with(|cur| {
            if let Some(le) = cur.as_mut().and_then(|c| c.lattice.as_mut()) {
                le.drag = None;
            }
        });
        let _ = super::commit(app, "Move lattice points");
        if app.doc.in_batch() {
            app.doc.commit_batch();
        }
    }
    true
}
