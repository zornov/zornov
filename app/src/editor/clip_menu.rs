// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The right-click menu of a clip, on the timeline and on the canvas: the
//! edits an editor expects there (cut, copy, paste, duplicate, delete,
//! split, group, link, detach sound, freeze, reverse, copy and paste a
//! look, nest) and a way to the Inspector. The AI has an op for each of
//! these (`group`, `link`, `freeze`, `copy_look`, `sequence nest`…); this is
//! the same edit by hand.

use super::Action;
use crate::app::StudioApp;
use crate::icons::Icon;
use crate::kit::{self, MenuItem};
use crate::view::Time;
use eframe::egui::{self, Pos2, Rect, Vec2};
use studio_model as m;

fn open_id() -> egui::Id {
    egui::Id::new("clip-menu")
}

fn at_id() -> egui::Id {
    egui::Id::new("clip-menu-at")
}

/// Opens the menu at `pos` for the clips selected (the clip under the
/// pointer first becomes the selection, unless it is already in it).
pub fn open(app: &mut StudioApp, ctx: &egui::Context, clip: u32, pos: Pos2) {
    if !app.editor.selection.contains(&clip) {
        app.editor.selection = vec![clip];
    }
    app.editor.selected_cut = None;
    ctx.data_mut(|d| d.insert_temp(at_id(), pos));
    kit::set_open(ctx, open_id(), true);
}

/// What the menu copied: clips for paste, one clip's look for paste look.
#[derive(Clone, Default)]
struct Held {
    clips: Option<m::Clipboard>,
    look: Option<m::Clip>,
}

fn held(ctx: &egui::Context) -> Held {
    ctx.data(|d| d.get_temp::<Held>(egui::Id::new("clip-menu-held"))).unwrap_or_default()
}

fn hold(ctx: &egui::Context, h: Held) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("clip-menu-held"), h));
}

/// Draws the menu while it is open. Call once a frame.
pub fn show(app: &mut StudioApp, ctx: &egui::Context) {
    if !kit::is_open(ctx, open_id()) {
        return;
    }
    let Some(pos) = ctx.data(|d| d.get_temp::<Pos2>(at_id())) else { return };
    let ids: Vec<u32> = app.editor.selection.iter().copied().filter(|id| app.doc.project.clip(*id).is_some()).collect();
    if ids.is_empty() {
        kit::set_open(ctx, open_id(), false);
        return;
    }
    let anchor = Rect::from_min_size(pos, Vec2::splat(1.0));
    let model = app.doc.model.active();
    let playhead = app.editor.playhead;
    let first = model.clip(m::ClipId(ids[0] as u64)).cloned();
    let inside = ids.iter().filter_map(|id| app.doc.project.clip(*id)).any(|c| playhead > c.start && playhead < c.end());
    let grouped = ids.iter().any(|id| model.clip(m::ClipId(*id as u64)).is_some_and(|c| c.group.is_some()));
    let linked = ids.iter().any(|id| model.clip(m::ClipId(*id as u64)).is_some_and(|c| c.link.is_some()));
    let many = ids.len() > 1;
    let picture_with_sound = ids.len() == 1 && first.as_ref().is_some_and(|c| matches!(&c.source, m::ClipSource::Media(r) if r.video && r.audio));
    let media_clip = ids.len() == 1 && first.as_ref().is_some_and(|c| matches!(c.source, m::ClipSource::Media(_) | m::ClipSource::Sequence(_)));
    let all_on = ids.iter().all(|id| model.clip(m::ClipId(*id as u64)).is_some_and(|c| c.enabled));
    let visual_one = ids.len() == 1 && app.doc.project.clip(ids[0]).is_some_and(|c| c.kind != crate::view::ClipKind::Audio);
    let h = held(ctx);
    let mut act: Option<Act> = None;
    let key = |id: &str| crate::keymap::hint(&app.prefs, id);
    let (split_k, dup_k, marker_k, delete_k) = (key("split"), key("duplicate"), key("marker"), key("delete"));
    kit::popover(ctx, open_id(), anchor, kit::Side::Below, 232.0, |ui| {
        let item = |ui: &mut egui::Ui, label: &str, shortcut: &str, icon: Icon, enabled: bool| {
            let mut m = MenuItem::new(label).icon(icon).enabled(enabled);
            if !shortcut.is_empty() {
                m = m.shortcut(shortcut);
            }
            m.show(ui).clicked()
        };
        if item(ui, "Split at playhead", &split_k, Icon::Scissors, inside) {
            act = Some(Act::Do(Action::Split));
        }
        if item(ui, "Duplicate", &dup_k, Icon::Copy, true) {
            act = Some(Act::Do(Action::Duplicate));
        }
        if item(ui, "Copy", "", Icon::Copy, true) {
            act = Some(Act::Copy);
        }
        if item(ui, "Cut", "", Icon::Scissors, true) {
            act = Some(Act::Cut);
        }
        if item(ui, "Paste at playhead", "", Icon::DownloadSimple, h.clips.is_some()) {
            act = Some(Act::Paste);
        }
        kit::menu_separator(ui);
        if item(ui, if all_on { "Disable" } else { "Enable" }, "", Icon::Eye, true) {
            act = Some(Act::Enabled(!all_on));
        }
        if item(ui, "Group", "", Icon::Stack, many) {
            act = Some(Act::Group);
        }
        if item(ui, "Ungroup", "", Icon::Stack, grouped) {
            act = Some(Act::Ungroup);
        }
        if item(ui, "Link", "", Icon::LinkSimple, many) {
            act = Some(Act::Link);
        }
        if item(ui, "Unlink", "", Icon::LinkSimple, linked) {
            act = Some(Act::Unlink);
        }
        if item(ui, "Nest into a sequence", "", Icon::Stack, many) {
            act = Some(Act::Nest);
        }
        kit::menu_separator(ui);
        if item(ui, "Bring to front", "", Icon::Stack, visual_one) {
            act = Some(Act::Restack(Stack::Top));
        }
        if item(ui, "Bring forward", "", Icon::Stack, visual_one) {
            act = Some(Act::Restack(Stack::Forward));
        }
        if item(ui, "Send backward", "", Icon::Stack, visual_one) {
            act = Some(Act::Restack(Stack::Backward));
        }
        if item(ui, "Send to back", "", Icon::Stack, visual_one) {
            act = Some(Act::Restack(Stack::Bottom));
        }
        kit::menu_separator(ui);
        if item(ui, "Detach sound", "", Icon::SpeakerHigh, picture_with_sound) {
            act = Some(Act::Detach);
        }
        if item(ui, "Freeze frame at playhead", "", Icon::Pause, media_clip && inside) {
            act = Some(Act::Freeze);
        }
        if item(ui, "Reverse", "", Icon::Repeat, media_clip) {
            act = Some(Act::Reverse);
        }
        kit::menu_separator(ui);
        if item(ui, "Copy look", "", Icon::Palette, ids.len() == 1) {
            act = Some(Act::CopyLook);
        }
        if item(ui, "Paste look", "", Icon::Palette, h.look.is_some()) {
            act = Some(Act::PasteLook);
        }
        if item(ui, "Add marker at playhead", &marker_k, Icon::Flag, true) {
            act = Some(Act::Do(Action::Marker));
        }
        if item(ui, "Show in Inspector", "", Icon::Faders, true) {
            act = Some(Act::Inspector);
        }
        kit::menu_separator(ui);
        if MenuItem::new("Delete").icon(Icon::Trash).shortcut(&delete_k).danger().show(ui).clicked() {
            act = Some(Act::Do(Action::Delete));
        }
    });
    let Some(act) = act else { return };
    kit::set_open(ctx, open_id(), false);
    run(app, ctx, &ids, playhead, act);
}

enum Act {
    Do(Action),
    Copy,
    Cut,
    Paste,
    Enabled(bool),
    Group,
    Ungroup,
    Link,
    Unlink,
    Nest,
    Detach,
    Freeze,
    Reverse,
    CopyLook,
    PasteLook,
    Inspector,
    Restack(Stack),
}

#[derive(Clone, Copy)]
enum Stack {
    Top,
    Forward,
    Backward,
    Bottom,
}

/// The `restack` op for a move in the stack, from who overlaps the clip in
/// time on the lanes above and below it. `None`: nothing to do.
fn restack_op(p: &crate::view::Project, id: u32, how: Stack) -> Option<serde_json::Value> {
    use serde_json::json;
    let c = p.clip(id)?;
    let idx = |l: u32| p.lanes.iter().position(|x| x.id == l);
    let me = idx(c.lane)?;
    let over = |o: &&crate::view::Clip| o.id != id && o.start < c.end() && c.start < o.end() && p.lane(o.lane).is_some_and(|l| l.kind == crate::view::LaneKind::Visual || Some(l.id) == p.spine());
    match how {
        Stack::Top => Some(json!({"op": "restack", "clip": id, "to": "top"})),
        Stack::Bottom => Some(json!({"op": "restack", "clip": id, "to": "bottom"})),
        Stack::Forward => {
            let o = p.clips.iter().filter(over).filter_map(|o| idx(o.lane).filter(|i| *i < me).map(|i| (i, o.id))).max_by_key(|x| x.0)?;
            Some(json!({"op": "restack", "clip": id, "in_front_of": o.1}))
        }
        Stack::Backward => {
            let spine = p.spine();
            let o = p.clips.iter().filter(over).filter(|o| Some(o.lane) != spine).filter_map(|o| idx(o.lane).filter(|i| *i > me).map(|i| (i, o.id))).min_by_key(|x| x.0)?;
            Some(json!({"op": "restack", "clip": id, "behind": o.1}))
        }
    }
}

fn run(app: &mut StudioApp, ctx: &egui::Context, ids: &[u32], playhead: Time, act: Act) {
    let seq = app.doc.seq();
    let clips: Vec<m::ClipId> = ids.iter().map(|id| m::ClipId(*id as u64)).collect();
    match act {
        Act::Do(a) => super::run(app, ctx, a),
        Act::Copy | Act::Cut => {
            if let Ok(cb) = m::Clipboard::copy(&app.doc.model, seq, &clips) {
                let mut h = held(ctx);
                h.clips = Some(cb);
                hold(ctx, h);
                kit::push(ctx, "Copied", None, Some(Icon::Copy));
                if matches!(act, Act::Cut) {
                    super::run(app, ctx, Action::Delete);
                }
            }
        }
        Act::Paste => {
            if let Some(cb) = held(ctx).clips {
                let old: Vec<u32> = app.doc.project.clips.iter().map(|c| c.id).collect();
                if app.doc.apply("Paste", m::Edit::Paste { seq, at: playhead, clipboard: Box::new(cb), mode: m::PasteMode::Insert }) {
                    app.editor.selection = app.doc.project.clips.iter().map(|c| c.id).filter(|id| !old.contains(id)).collect();
                }
            }
        }
        Act::Enabled(on) => {
            let edits: Vec<m::Edit> = clips
                .iter()
                .filter_map(|id| app.doc.model.active().clip(*id).cloned())
                .map(|mut c| {
                    c.enabled = on;
                    m::Edit::SetClip { seq, clip: Box::new(c) }
                })
                .collect();
            app.doc.apply(if on { "Enable clips" } else { "Disable clips" }, m::Edit::Batch { edits });
        }
        Act::Group => {
            let group = m::GroupId(app.doc.alloc_id());
            app.doc.apply("Group", m::Edit::Group { seq, clips, group });
        }
        Act::Ungroup => {
            app.doc.apply("Ungroup", m::Edit::Ungroup { seq, clips });
        }
        Act::Link => {
            let link = m::LinkId(app.doc.alloc_id());
            app.doc.apply("Link", m::Edit::Link { seq, clips, link });
        }
        Act::Unlink => {
            app.doc.apply("Unlink", m::Edit::Unlink { seq, clips });
        }
        Act::Nest => {
            let new_sequence = m::SequenceId(app.doc.alloc_id());
            let new_clip = m::ClipId(app.doc.alloc_id());
            if app.doc.apply("Nest", m::Edit::CreateCompound { seq, clips, name: "Nest".into(), new_sequence, new_clip }) {
                app.editor.selection = vec![new_clip.0 as u32];
            }
        }
        Act::Detach => {
            if let Some(&clip) = clips.first() {
                let new_clip = m::ClipId(app.doc.alloc_id());
                app.doc.apply("Detach sound", m::Edit::DetachAudio { seq, clip, new_clip });
            }
        }
        Act::Freeze => {
            if let Some(&clip) = clips.first() {
                let new_clip = m::ClipId(app.doc.alloc_id());
                app.doc.apply("Freeze frame", m::Edit::FreezeFrame { seq, clip, at: playhead, duration: crate::view::SEC, new_clip });
            }
        }
        Act::Reverse => {
            app.doc.apply("Reverse", m::Edit::Reverse { seq, clips });
        }
        Act::CopyLook => {
            if let Some(c) = clips.first().and_then(|id| app.doc.model.active().clip(*id).cloned()) {
                let mut h = held(ctx);
                h.look = Some(c);
                hold(ctx, h);
                kit::push(ctx, "Look copied", None, Some(Icon::Palette));
            }
        }
        Act::PasteLook => {
            if let Some(source) = held(ctx).look {
                let attrs = m::AttrSet { effects: true, grade: true, blend: true, masks: true, ..Default::default() };
                app.doc.apply("Paste look", m::Edit::PasteAttributes { seq, targets: clips, source: Box::new(source), attrs });
            }
        }
        Act::Inspector => app.editor.open_panel(crate::dock::Panel::Inspector),
        Act::Restack(how) => {
            let Some(op) = ids.first().and_then(|id| restack_op(&app.doc.project, *id, how)) else { return };
            let doc = std::mem::replace(&mut app.doc, crate::agent::Session::default().doc);
            let mut s = crate::agent::Session::new(doc);
            let r = s.edit(&[op], "Restack", false);
            app.doc = s.doc;
            if let Err(e) = r {
                kit::push(ctx, e, None, Some(Icon::Warning));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn restack_moves_a_layer_through_the_stack() {
        let mut s = crate::agent::Session::new(crate::services::project::new_custom("Stack", 320, 180, 24.0));
        s.edit(
            &[
                json!({"op": "card", "colour": "#ff0000", "at": 0.0, "duration": 3.0, "lane": "overlay", "width": 100, "height": 100}),
                json!({"op": "card", "colour": "#00ff00", "at": 0.0, "duration": 3.0, "lane": "overlay", "width": 100, "height": 100}),
            ],
            "cards",
            false,
        )
        .expect("two cards");
        let lanes = |s: &crate::agent::Session, id: u32| s.doc.project.lanes.iter().position(|l| Some(l.id) == s.doc.project.clip(id).map(|c| c.lane)).unwrap();
        let ids: Vec<u32> = s.doc.project.clips.iter().filter(|c| c.name.contains("card")).map(|c| c.id).collect();
        assert_eq!(ids.len(), 2);
        let (a, b) = if lanes(&s, ids[0]) < lanes(&s, ids[1]) { (ids[0], ids[1]) } else { (ids[1], ids[0]) };
        // `a` is above `b`. Send a to the back: it ends up below b.
        let op = restack_op(&s.doc.project, a, Stack::Bottom).expect("an op");
        s.edit(&[op], "back", false).expect("restack");
        assert!(lanes(&s, a) > lanes(&s, b), "a is now under b: a {} b {} of {:?}", lanes(&s, a), lanes(&s, b), s.doc.project.lanes.iter().map(|l| (l.id, l.kind)).collect::<Vec<_>>());
        // Bring it forward again, over b.
        let op = restack_op(&s.doc.project, a, Stack::Forward).expect("an op");
        s.edit(&[op], "forward", false).expect("restack");
        assert!(lanes(&s, a) < lanes(&s, b), "a is over b again");
    }
}
