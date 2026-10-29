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
        v.push(e(Section::Link, "Group", Icon::Stack, None, true, Act::Group));
        v.push(e(Section::Link, "Link", Icon::LinkSimple, None, true, Act::Link));
        v.push(e(Section::Link, "Nest into a sequence", Icon::Stack, None, true, Act::Nest));
    }
    if f.grouped {
        v.push(e(Section::Link, "Ungroup", Icon::Stack, None, true, Act::Ungroup));
    }
    if f.linked {
        v.push(e(Section::Link, "Unlink", Icon::LinkSimple, None, true, Act::Unlink));
    }
    if f.picture_with_sound {
        v.push(e(Section::Audio, "Detach sound", Icon::SpeakerHigh, None, true, Act::Detach));
    }
    if f.media_clip {
        v.push(e(Section::Time, "Freeze frame at playhead", Icon::Pause, None, f.inside, Act::Freeze));
        v.push(e(Section::Time, "Reverse", Icon::Repeat, None, true, Act::Reverse));
    }
    if f.one {
        v.push(e(Section::Look, "Copy look", Icon::Palette, None, true, Act::CopyLook));
        v.push(e(Section::Look, "Paste look", Icon::Palette, None, f.can_paste_look, Act::PasteLook));
    }
    v.push(e(Section::Clip, if f.all_on { "Disable" } else { "Enable" }, Icon::Eye, None, true, Act::Enabled(!f.all_on)));
    v.push(e(Section::Clip, "Add marker", Icon::Flag, Some("marker"), true, Act::Marker));
    v.push(e(Section::Clip, "Show in Inspector", Icon::Faders, None, true, Act::Inspector));
    v.push(Entry { section: Section::Delete, label: "Delete", icon: Icon::Trash, hint: Some("delete"), enabled: true, danger: true, act: Act::Delete });
    v
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
}
