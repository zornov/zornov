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
    let hints = [("split", key("split")), ("duplicate", key("duplicate")), ("marker", key("marker")), ("delete", key("delete"))];
    let facts = Facts { inside, grouped, linked, many, picture_with_sound, media_clip, all_on, visual_one, can_paste: h.clips.is_some(), can_paste_look: h.look.is_some(), one: ids.len() == 1 };
    let entries = entries(&facts);
    kit::popover(ctx, open_id(), anchor, kit::Side::Below, 232.0, |ui| {
        let hint = |k: &str| hints.iter().find(|h| h.0 == k).map_or("", |h| h.1.as_str()).to_owned();
        let draw = |ui: &mut egui::Ui, e: &Entry, act: &mut Option<Act>| {
            let shortcut = e.hint.map(&hint).unwrap_or_default();
            let mut m = MenuItem::new(e.label).icon(e.icon).enabled(e.enabled).shortcut(&shortcut);
            if e.danger {
                m = m.danger();
            }
            if m.show(ui).clicked() {
                *act = Some(e.act);
            }
        };
        let mut last: Option<&str> = None;
        for section in SECTIONS {
            let group: Vec<&Entry> = entries.iter().filter(|e| e.section == *section).collect();
            if group.is_empty() {
                continue;
            }
            if last.is_some() {
                kit::menu_separator(ui);
            }
            last = Some(section.name());
            match section.name() {
                // Plain rows.
                "" => group.iter().for_each(|e| draw(ui, e, &mut act)),
                // A heading that opens its rows in place.
                name => {
                    let gid = egui::Id::unique(("clip-menu-group", name));
                    let open = ui.ctx().data(|d| d.get_temp::<bool>(gid)).unwrap_or(false);
                    if MenuItem::new(name).icon(section.icon()).shortcut(if open { "▾" } else { "▸" }).show(ui).clicked() {
                        ui.ctx().data_mut(|d| d.insert_temp(gid, !open));
                    }
                    if open {
                        ui.indent(gid, |ui| group.iter().for_each(|e| draw(ui, e, &mut act)));
                    }
                }
            }
        }
    });
    let Some(act) = act else { return };
    kit::set_open(ctx, open_id(), false);
    run(app, ctx, &ids, playhead, act);
}

/// What is true of the selection, for which rows the menu shows.
struct Facts {
    inside: bool,
    grouped: bool,
    linked: bool,
    many: bool,
    one: bool,
    picture_with_sound: bool,
    media_clip: bool,
    all_on: bool,
    visual_one: bool,
    can_paste: bool,
    can_paste_look: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Section {
    Edit,
    Arrange,
    Link,
    Audio,
    Time,
    Look,
    Clip,
    Delete,
}

const SECTIONS: &[Section] = &[Section::Edit, Section::Arrange, Section::Link, Section::Audio, Section::Time, Section::Look, Section::Clip, Section::Delete];

impl Section {
    /// The heading a section folds under; empty for plain rows.
    fn name(self) -> &'static str {
        match self {
            Section::Arrange => "Arrange",
            Section::Link => "Group and link",
            Section::Audio => "Audio",
            Section::Time => "Speed and time",
            Section::Look => "Look",
            _ => "",
        }
    }
    fn icon(self) -> Icon {
        match self {
            Section::Arrange => Icon::Stack,
            Section::Link => Icon::LinkSimple,
            Section::Audio => Icon::SpeakerHigh,
            Section::Time => Icon::Repeat,
            _ => Icon::Palette,
        }
    }
}

struct Entry {
    section: Section,
    label: &'static str,
    icon: Icon,
    /// A keymap id whose key is shown beside the row.
    hint: Option<&'static str>,
    enabled: bool,
    danger: bool,
    act: Act,
}

/// The rows for a selection: what cannot apply to it is left out, what is
/// only unavailable for now (nothing to paste, playhead outside the clip)
/// is shown dimmed.
fn entries(f: &Facts) -> Vec<Entry> {
    let e = |section, label, icon, hint, enabled, act| Entry { section, label, icon, hint, enabled, danger: false, act };
    let mut v = vec![
        e(Section::Edit, "Split at playhead", Icon::Scissors, Some("split"), f.inside, Act::Split),
        e(Section::Edit, "Duplicate", Icon::Copy, Some("duplicate"), true, Act::Duplicate),
        e(Section::Edit, "Copy", Icon::Copy, None, true, Act::Copy),
        e(Section::Edit, "Cut", Icon::Scissors, None, true, Act::Cut),
        e(Section::Edit, "Paste at playhead", Icon::DownloadSimple, None, f.can_paste, Act::Paste),
    ];
    if f.visual_one {
        v.push(e(Section::Arrange, "Bring to front", Icon::Stack, None, true, Act::Restack(Stack::Top)));
        v.push(e(Section::Arrange, "Bring forward", Icon::Stack, None, true, Act::Restack(Stack::Forward)));
        v.push(e(Section::Arrange, "Send backward", Icon::Stack, None, true, Act::Restack(Stack::Backward)));
        v.push(e(Section::Arrange, "Send to back", Icon::Stack, None, true, Act::Restack(Stack::Bottom)));
    }
    if f.many {
        v.push(e(Section::Link, "Group", Icon::Stack, Some("group"), true, Act::Group));
        v.push(e(Section::Link, "Link", Icon::LinkSimple, Some("link"), true, Act::Link));
        v.push(e(Section::Link, "Nest into a sequence", Icon::Stack, None, true, Act::Nest));
    }
    if f.grouped {
        v.push(e(Section::Link, "Ungroup", Icon::Stack, Some("ungroup"), true, Act::Ungroup));
    }
    if f.linked {
        v.push(e(Section::Link, "Unlink", Icon::LinkSimple, Some("unlink"), true, Act::Unlink));
    }
    if f.picture_with_sound {
        v.push(e(Section::Audio, "Detach sound", Icon::SpeakerHigh, None, true, Act::Detach));
    }
    if f.media_clip {
        v.push(e(Section::Time, "Freeze frame at playhead", Icon::Pause, Some("freeze"), f.inside, Act::Freeze));
        v.push(e(Section::Time, "Reverse", Icon::Repeat, Some("reverse"), true, Act::Reverse));
    }
    if f.one {
        v.push(e(Section::Look, "Copy look", Icon::Palette, Some("copy-look"), true, Act::CopyLook));
        v.push(e(Section::Look, "Paste look", Icon::Palette, Some("paste-look"), f.can_paste_look, Act::PasteLook));
    }
    v.push(e(Section::Clip, if f.all_on { "Disable" } else { "Enable" }, Icon::Eye, None, true, Act::Enabled(!f.all_on)));
    v.push(e(Section::Clip, "Add marker", Icon::Flag, Some("marker"), true, Act::Marker));
    v.push(e(Section::Clip, "Show in Inspector", Icon::Faders, None, true, Act::Inspector));
    v.push(Entry { section: Section::Delete, label: "Delete", icon: Icon::Trash, hint: Some("delete"), enabled: true, danger: true, act: Act::Delete });
    v
}

/// A keyboard action that is otherwise a clip-menu row (group, link, freeze,
/// reverse, look copy/paste) run on the selection.
pub fn key_action(app: &mut StudioApp, ctx: &egui::Context, id: &str) {
    let ids = app.editor.selection.clone();
    let act = match id {
        "group" => Act::Group,
        "ungroup" => Act::Ungroup,
        "link" => Act::Link,
        "unlink" => Act::Unlink,
        "freeze" => Act::Freeze,
        "reverse" => Act::Reverse,
        "copy-look" => Act::CopyLook,
        "paste-look" => Act::PasteLook,
        _ => return,
    };
    let many = ids.len() > 1;
    let one = ids.len() == 1;
    let why = match act {
        _ if ids.is_empty() => Some("Select a clip first"),
        Act::Group | Act::Link if !many => Some("Select two or more clips"),
        Act::CopyLook | Act::PasteLook if !one => Some("Select one clip"),
        Act::PasteLook if !held(ctx).look.is_some() => Some("Nothing to paste: copy a look first"),
        _ => None,
    };
    if let Some(w) = why {
        kit::push(ctx, w, None, Some(Icon::Info));
        return;
    }
    let at = app.editor.playhead;
    run(app, ctx, &ids, at, act);
}

/// ⌘C / ⌘X on the selection (the same as the clip menu's Copy and Cut).
pub fn copy_selection(app: &mut StudioApp, ctx: &egui::Context, cut: bool) {
    let ids = app.editor.selection.clone();
    if ids.is_empty() {
        kit::push(ctx, if cut { "Select a clip to cut" } else { "Select a clip to copy" }, None, Some(Icon::Info));
        return;
    }
    let at = app.editor.playhead;
    run(app, ctx, &ids, at, if cut { Act::Cut } else { Act::Copy });
}

/// ⌘V: paste at the playhead, or say there is nothing to paste.
pub fn paste_selection(app: &mut StudioApp, ctx: &egui::Context) {
    if !can_paste(ctx) {
        kit::push(ctx, "Nothing to paste: copy a clip first", None, Some(Icon::Info));
        return;
    }
    let at = app.editor.playhead;
    paste_at(app, ctx, at);
}

/// Whether the menu has copied clips to paste.
pub fn can_paste(ctx: &egui::Context) -> bool {
    held(ctx).clips.is_some()
}

/// Paste the clips the menu copied at `at`; the new clips become the selection.
pub fn paste_at(app: &mut StudioApp, ctx: &egui::Context, at: Time) {
    let seq = app.doc.seq();
    if let Some(cb) = held(ctx).clips {
        let old: Vec<u32> = app.doc.project.clips.iter().map(|c| c.id).collect();
        if app.doc.apply("Paste", m::Edit::Paste { seq, at, clipboard: Box::new(cb), mode: m::PasteMode::Insert }) {
            app.editor.selection = app.doc.project.clips.iter().map(|c| c.id).filter(|id| !old.contains(id)).collect();
            let n = app.editor.selection.len();
            super::toast_undo(&mut app.editor, ctx, if n == 1 { "Pasted clip".to_owned() } else { format!("Pasted {n} clips") }, Icon::DownloadSimple);
        }
    }
}

#[derive(Clone, Copy)]
enum Act {
    Split,
    Duplicate,
    Marker,
    Delete,
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
        Act::Split => super::run(app, ctx, Action::Split),
        Act::Duplicate => super::run(app, ctx, Action::Duplicate),
        Act::Marker => super::run(app, ctx, Action::Marker),
        Act::Delete => super::run(app, ctx, Action::Delete),
        Act::Copy | Act::Cut => {
            match m::Clipboard::copy(&app.doc.model, seq, &clips) {
                Ok(cb) => {
                    let mut h = held(ctx);
                    h.clips = Some(cb);
                    hold(ctx, h);
                    if matches!(act, Act::Cut) {
                        app.editor.selection = ids.to_vec();
                        // "Cut" is said only once the clips are really gone.
                        if !super::delete_selected(&mut app.editor, &mut app.doc, ctx, true) {
                            kit::push(ctx, "Copied, but could not remove the clips", None, Some(Icon::Warning));
                        }
                    } else {
                        kit::push(ctx, if ids.len() == 1 { "Copied clip".to_owned() } else { format!("Copied {} clips", ids.len()) }, None, Some(Icon::Copy));
                    }
                }
                Err(e) => {
                    kit::push(ctx, format!("Could not copy: {e}"), None, Some(Icon::Warning));
                }
            }
        }
        Act::Paste => paste_at(app, ctx, playhead),
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
            let Some(op) = ids.first().and_then(|id| restack_op(&app.doc.project, *id, how)) else {
                kit::push(ctx, "Nothing to restack against here", None, Some(Icon::Warning));
                return;
            };
            match restack(app, ids[0], &op) {
                Ok(done) => {
                    super::toast_undo(&mut app.editor, ctx, done, Icon::Stack);
                }
                Err(e) => {
                    kit::push(ctx, e, None, Some(Icon::Warning));
                }
            }
        }
    }
}

/// Move a clip to the lane the `restack_op` asks for, adding a lane when
/// none is free there: one plain edit, one undo step.
fn restack(app: &mut StudioApp, id: u32, op: &serde_json::Value) -> Result<String, String> {
    use crate::view::LaneKind;
    let (lane, new_at) = {
        let p = &app.doc.project;
        let c = p.clip(id).ok_or("That clip is gone")?;
        let idx = |l: u32| p.lanes.iter().position(|x| x.id == l);
        let spine = p.spine();
        let spine_i = spine.and_then(idx).unwrap_or(p.lanes.len());
        let free = |l: u32| !p.clips.iter().any(|o| o.id != id && o.lane == l && o.start < c.end() && c.start < o.end());
        let visual = |i: usize| p.lanes.get(i).is_some_and(|l| l.kind == LaneKind::Visual && !l.locked && Some(l.id) != spine);
        let lane_of = |o: u64| p.clip(o as u32).map(|c| c.lane);
        if let Some(o) = op.get("in_front_of").and_then(|v| v.as_u64()) {
            let oi = lane_of(o).and_then(idx).ok_or("That clip has no lane")?;
            ((0..oi).rev().find(|&i| visual(i) && free(p.lanes[i].id)).map(|i| p.lanes[i].id), oi)
        } else if let Some(o) = op.get("behind").and_then(|v| v.as_u64()) {
            let ol = lane_of(o).ok_or("That clip has no lane")?;
            if Some(ol) == spine {
                return Err("Nothing goes behind the main lane".into());
            }
            let oi = idx(ol).ok_or("That clip has no lane")?;
            ((oi + 1..spine_i).find(|&i| visual(i) && free(p.lanes[i].id)).map(|i| p.lanes[i].id), oi + 1)
        } else if op.get("to").and_then(|v| v.as_str()) == Some("top") {
            let busy = (0..spine_i).filter(|&i| p.lanes.get(i).is_some_and(|l| l.kind == LaneKind::Visual) && !free(p.lanes[i].id)).min();
            let top = busy.unwrap_or(spine_i);
            ((0..top).rev().find(|&i| visual(i) && free(p.lanes[i].id)).map(|i| p.lanes[i].id), top)
        } else {
            (spine_i.checked_sub(1).filter(|&i| visual(i) && free(p.lanes[i].id)).map(|i| p.lanes[i].id), spine_i)
        }
    };
    if lane.is_some_and(|l| app.doc.project.clip(id).is_some_and(|c| c.lane == l)) {
        return Err("Already there".into());
    }
    app.doc.begin_batch("Restack");
    let lane_id = match lane {
        Some(l) => l,
        None => {
            let lid = app.doc.alloc_id();
            let seq = app.doc.seq();
            if !app.doc.apply("Add lane", m::Edit::AddLane { seq, lane: m::Lane::new(m::LaneId(lid), m::LaneKind::Visual, "Overlay"), index: Some(new_at) }) {
                let why = app.doc.error.take().unwrap_or_else(|| "Could not add a lane".into());
                app.doc.rollback_batch();
                return Err(why);
            }
            lid as u32
        }
    };
    let e = super::cutting::move_edit(app, vec![m::ClipId(id as u64)], 0, Some(m::LaneId(lane_id as u64)));
    let ok = app.doc.apply("Restack", e);
    if !ok {
        let why = app.doc.error.take().unwrap_or_else(|| "Could not move the clip".into());
        app.doc.rollback_batch();
        return Err(why);
    }
    app.doc.commit_batch();
    Ok("Restacked clip".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn menu_rows(f: &Facts) -> Vec<&'static str> {
        entries(f).iter().map(|e| e.label).collect()
    }

    fn facts() -> Facts {
        Facts { inside: true, grouped: false, linked: false, many: false, one: true, picture_with_sound: false, media_clip: false, all_on: true, visual_one: true, can_paste: false, can_paste_look: false }
    }

    #[test]
    fn rows_that_cannot_apply_are_left_out() {
        let one = menu_rows(&facts());
        for gone in ["Group", "Ungroup", "Link", "Unlink", "Nest into a sequence", "Detach sound", "Reverse"] {
            assert!(!one.contains(&gone), "{gone} is hidden for one plain clip");
        }
        let many = menu_rows(&Facts { many: true, one: false, visual_one: false, ..facts() });
        assert!(many.contains(&"Group") && many.contains(&"Link") && many.contains(&"Nest into a sequence"));
        assert!(!many.contains(&"Bring to front"), "stacking needs one clip");
        // Only unavailable for now: shown, dimmed.
        let paste = entries(&facts()).into_iter().find(|e| e.label == "Paste at playhead").unwrap();
        assert!(!paste.enabled);
    }

    #[test]
    fn each_verb_appears_once_and_uses_the_menu_wording() {
        let all = menu_rows(&Facts { many: true, grouped: true, linked: true, picture_with_sound: true, media_clip: true, can_paste: true, can_paste_look: true, ..facts() });
        let mut seen = std::collections::HashSet::new();
        assert!(all.iter().all(|l| seen.insert(*l)), "no verb twice: {all:?}");
        assert!(all.contains(&"Add marker") && all.contains(&"Delete") && all.contains(&"Split at playhead"));
        assert!(!all.contains(&"Add marker at playhead"), "one wording for the verb");
    }

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

    /// Restack is a plain edit: one undo step, authored by the person, and
    /// a lane is added when none is free.
    #[test]
    fn restack_is_one_plain_undo_step() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let _serial = crate::ui_tests::serial();
        let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
        let mut s = crate::agent::Session::new(crate::services::project::new_custom("Stack", 320, 180, 24.0));
        s.edit(&[json!({"op": "card", "colour": "#ff0000", "at": 0.0, "duration": 3.0, "lane": "overlay", "width": 100, "height": 100}), json!({"op": "card", "colour": "#00ff00", "at": 0.0, "duration": 3.0, "lane": "overlay", "width": 100, "height": 100})], "cards", false).unwrap();
        app.doc = s.doc;
        let lane_pos = |app: &StudioApp, id: u32| app.doc.project.lanes.iter().position(|l| Some(l.id) == app.doc.project.clip(id).map(|c| c.lane)).unwrap();
        let ids: Vec<u32> = app.doc.project.clips.iter().filter(|c| c.name.contains("card")).map(|c| c.id).collect();
        let (a, b) = if lane_pos(&app, ids[0]) < lane_pos(&app, ids[1]) { (ids[0], ids[1]) } else { (ids[1], ids[0]) };
        let steps = app.doc.history_len();
        let op = restack_op(&app.doc.project, a, Stack::Bottom).expect("an op");
        let done = restack(&mut app, a, &op).expect("restacked");
        assert_eq!(done, "Restacked clip");
        assert!(lane_pos(&app, a) > lane_pos(&app, b));
        assert_eq!(app.doc.history_len(), steps + 1, "one undo step, lane included");
        assert!(app.doc.can_undo());
        assert!(app.doc.undo().is_some());
        assert!(lane_pos(&app, a) < lane_pos(&app, b), "undo puts it back");
    }
}
