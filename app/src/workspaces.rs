// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Workspaces: the editor's panels laid out for a task. Six come with
//! Studio (Edit, Colour, Audio, Effects, Motion, Review); the person can
//! save the current layout under a name of their own (or over a built-in
//! one), kept in `<Studio>/Workspaces.json`. Layouts saved under the earlier
//! names (VFX, Motion graphics, Production) are read as the renamed ones.
//!
//! A workspace sets only what can be dragged anyway: the whole dock tree
//! (which panels are where, open and how big), the Library section, whether
//! the scopes show over the picture and which Inspector tab a selected clip
//! opens on. Nothing is locked. Files from before docking (no `dock`) are
//! read as the old fixed layout with their sizes.
//!
//! One click: the tabs in the top bar (a dropdown when the window is narrow)
//! and View ▸ Workspaces; `⌥1`–`⌥6` from the keyboard (Settings ▸ Shortcuts
//! changes them). [`switch`] does everything (and tells a screen reader);
//! [`apply`] only moves the panels.

use serde::{Deserialize, Serialize};

use crate::app::StudioApp;
use crate::editor::library::RailItem;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub name: String,
    #[serde(default)]
    pub about: String,
    pub library_open: bool,
    pub library_w: f32,
    pub inspector_open: bool,
    pub inspector_w: f32,
    pub timeline_open: bool,
    /// None: the editor's own height.
    pub timeline_h: Option<f32>,
    /// The whole layout (`crate::dock`). The fields above are kept from it
    /// for an older Studio, and used when a file has no tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dock: Option<crate::dock::Tree>,
    /// The library section shown (Media, Effects, Looks, Audio...).
    pub rail: String,
    /// Whether the video scopes are showing.
    #[serde(default)]
    pub scopes: bool,
    /// The Inspector tab a selected clip opens on (Adjust, Audio, Animation…);
    /// empty leaves it as it is.
    #[serde(default)]
    pub tab: String,
}

fn ws(name: &str, about: &str, library: (bool, f32), inspector: (bool, f32), timeline: (bool, Option<f32>), rail: &str) -> Workspace {
    Workspace {
        name: name.into(),
        about: about.into(),
        library_open: library.0,
        library_w: library.1,
        inspector_open: inspector.0,
        inspector_w: inspector.1,
        timeline_open: timeline.0,
        timeline_h: timeline.1,
        dock: Some(crate::dock::Tree::legacy(library, inspector, timeline)),
        rail: rail.into(),
        scopes: false,
        tab: String::new(),
    }
}

fn with(mut w: Workspace, scopes: bool, tab: &str) -> Workspace {
    w.scopes = scopes;
    w.tab = tab.into();
    w
}

/// `w` with `p` docked at `zone` of `target` in its tree.
fn docking(mut w: Workspace, p: crate::dock::Panel, target: Option<crate::dock::Panel>, zone: crate::dock::Zone) -> Workspace {
    if let Some(t) = w.dock.as_mut() {
        let lay = crate::editor::dock_ui::fallback_layout(t);
        t.dock(p, target, zone, &lay);
    }
    w
}

/// `w` with `p`'s tab showing in its stack.
fn showing(mut w: Workspace, p: crate::dock::Panel) -> Workspace {
    if let Some(t) = w.dock.as_mut() {
        t.activate(p);
    }
    w
}

/// The workspaces that come with Studio. Names and descriptions are English
/// here and translated where they are shown.
pub fn builtin() -> Vec<Workspace> {
    // Six dock trees are built; the top bar asks several times a frame.
    static BUILT: std::sync::OnceLock<Vec<Workspace>> = std::sync::OnceLock::new();
    BUILT.get_or_init(build_in).clone()
}

fn build_in() -> Vec<Workspace> {
    use crate::dock::{Panel, Zone};
    vec![
        ws("Edit", "Cut and arrange: a big timeline with your media beside it.", (true, 340.0), (true, 300.0), (true, Some(300.0)), "Media"),
        // The scopes docked under the colour controls, and a timeline tall
        // enough for a few tracks.
        docking(with(ws("Colour", "Grade and match shots: looks, colour controls and scopes.", (true, 280.0), (true, 380.0), (true, Some(240.0)), "Looks"), true, "Adjust"), Panel::Scopes, Some(Panel::Inspector), Zone::Bottom),
        // The mixer beside the timeline, so levels can be set while the clips go by.
        docking(with(ws("Audio", "Mix sound: audio tools, a tall timeline and level controls.", (true, 320.0), (true, 300.0), (true, Some(460.0)), "Audio"), false, "Audio"), Panel::Sound, Some(Panel::Timeline), Zone::Right),
        with(ws("Effects", "Effects, masks and tracking on a large viewer.", (true, 320.0), (true, 400.0), (true, Some(220.0)), "Effects"), false, "Adjust"),
        with(ws("Motion", "Titles, shapes and animation with keyframes close at hand.", (true, 320.0), (true, 340.0), (true, Some(280.0)), "Motions"), false, "Animation"),
        // The Assistant a tab with the Library (showing); Review notes,
        // versions and the team a tab with the Inspector.
        showing(showing(docking(docking(ws("Review", "Plan and review: your media and the Assistant, a short timeline.", (true, 380.0), (true, 300.0), (true, Some(200.0)), "AI"), Panel::Review, Some(Panel::Inspector), Zone::Tab), Panel::Ai, Some(Panel::Library), Zone::Tab), Panel::Ai), Panel::Review),
    ]
}

/// The names the built-in workspaces had before, and what they are now.
const RENAMED: [(&str, &str); 3] = [("VFX", "Effects"), ("Motion graphics", "Motion"), ("Production", "Review")];

/// `name` as the built-in workspaces call it now (an earlier name becomes the new one).
fn canonical(name: &str) -> &str {
    let name = name.trim();
    RENAMED.iter().find(|(old, _)| old.eq_ignore_ascii_case(name)).map_or(name, |(_, new)| new)
}

/// The icon of a built-in workspace (the generic one for the person's own).
pub fn icon(name: &str) -> crate::icons::Icon {
    use crate::icons::Icon;
    match name.to_ascii_lowercase().as_str() {
        "edit" => Icon::Scissors,
        "colour" => Icon::Palette,
        "audio" => Icon::Faders,
        "effects" | "vfx" => Icon::MagicWand,
        "motion" | "motion graphics" => Icon::Path,
        "review" | "production" => Icon::FilmSlate,
        _ => Icon::SquaresFour,
    }
}

/// Keyboard actions (Settings ▸ Shortcuts) and the built-in workspace each runs.
pub const ACTIONS: [(&str, &str); 6] = [
    ("workspace-edit", "Edit"),
    ("workspace-colour", "Colour"),
    ("workspace-audio", "Audio"),
    ("workspace-vfx", "Effects"),
    ("workspace-motion", "Motion"),
    ("workspace-production", "Review"),
];

/// The workspace a shortcut action stands for (the Effects, Motion and Review keys keep their action ids).
pub fn for_action(action: &str) -> Option<&'static str> {
    ACTIONS.iter().find(|(a, _)| *a == action).map(|(_, n)| *n)
}

fn path() -> std::path::PathBuf {
    studio_paths::studio().join("Workspaces.json")
}

fn read_saved() -> Vec<Workspace> {
    std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// How long `all` goes on with what it read; the top bar asks every frame.
const KEEP: std::time::Duration = std::time::Duration::from_secs(1);

static ALL: std::sync::Mutex<Option<(std::time::Instant, Vec<Workspace>)>> = std::sync::Mutex::new(None);

/// Drops the kept list, after this process changed the file.
fn forget() {
    *ALL.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Built-in workspaces (as the person changed them) and their own.
pub fn all() -> Vec<Workspace> {
    let mut slot = ALL.lock().unwrap_or_else(|e| e.into_inner());
    match &*slot {
        Some((at, v)) if at.elapsed() < KEEP => v.clone(),
        _ => {
            let v = merged(&builtin(), read_saved());
            *slot = Some((std::time::Instant::now(), v.clone()));
            v
        }
    }
}

/// The built-in workspaces with the person's changes to them, then their own.
fn merged(builtin: &[Workspace], own: Vec<Workspace>) -> Vec<Workspace> {
    let is = |o: &Workspace, b: &Workspace| canonical(&o.name).eq_ignore_ascii_case(&b.name);
    let mut out: Vec<Workspace> = builtin
        .iter()
        .map(|b| {
            own.iter().find(|o| is(o, b)).map_or_else(
                || b.clone(),
                |o| {
                    // A layout kept under an earlier name is the renamed workspace's.
                    let mut w = o.clone();
                    if !o.name.eq_ignore_ascii_case(&b.name) && RENAMED.iter().any(|(old, _)| old.eq_ignore_ascii_case(&o.name)) {
                        w.name = b.name.clone();
                    }
                    w
                },
            )
        })
        .collect();
    out.extend(own.into_iter().filter(|o| !builtin.iter().any(|b| is(o, b))));
    out
}

pub fn find(name: &str) -> Option<Workspace> {
    all().into_iter().find(|w| w.name.eq_ignore_ascii_case(canonical(name)))
}

/// Saves (or replaces) a workspace.
pub fn save(w: Workspace) -> Result<(), String> {
    if w.name.trim().is_empty() {
        return Err("a workspace needs a name".into());
    }
    let mut w = w;
    w.name = canonical(&w.name).to_owned();
    let mut own = read_saved();
    own.retain(|o| !canonical(&o.name).eq_ignore_ascii_case(&w.name));
    own.push(w);
    forget();
    if let Some(d) = path().parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(path(), serde_json::to_vec_pretty(&own).unwrap_or_default()).map_err(|e| format!("could not save the workspace: {e}"))
}

/// Removes a saved workspace (a built-in one goes back to how it came).
pub fn delete(name: &str) -> Result<(), String> {
    let mut own = read_saved();
    let n = own.len();
    own.retain(|o| !canonical(&o.name).eq_ignore_ascii_case(canonical(name)));
    if own.len() == n {
        return Err(format!("no saved workspace \"{name}\""));
    }
    forget();
    std::fs::write(path(), serde_json::to_vec_pretty(&own).unwrap_or_default()).map_err(|e| e.to_string())
}

fn rail(name: &str) -> RailItem {
    RailItem::ALL.into_iter().find(|r| r.name().eq_ignore_ascii_case(name)).unwrap_or(RailItem::Media)
}

/// The Library section a workspace shows (for the editor to open on).
pub fn rail_of(w: &Workspace) -> RailItem {
    rail(&w.rail)
}

/// The window's layout now, as a workspace called `name`.
/// The window's layout now, as a workspace called `name`.
pub fn current(app: &StudioApp, name: &str) -> Workspace {
    let ed = &app.editor;
    let ((library_open, library_w), (inspector_open, inspector_w), (timeline_open, timeline_h)) = ed.dock.legacy_fields();
    Workspace {
        name: name.trim().to_owned(),
        about: String::new(),
        library_open,
        library_w,
        inspector_open,
        inspector_w,
        timeline_open,
        timeline_h,
        dock: Some(ed.dock.clone()),
        rail: ed.rail.name().to_owned(),
        scopes: false,
        tab: String::new(),
    }
}

/// The layout a workspace stands for: its tree, or (a file from before
/// docking) the fixed layout with its sizes.
pub fn tree_of(w: &Workspace) -> crate::dock::Tree {
    match &w.dock {
        Some(t) => t.clone().sanitized(),
        None => crate::dock::Tree::legacy((w.library_open, w.library_w), (w.inspector_open, w.inspector_w), (w.timeline_open, w.timeline_h)),
    }
}

/// Lays the window out as `w` (and remembers it as the layout).
pub fn apply(app: &mut StudioApp, w: &Workspace) {
    let ed = &mut app.editor;
    // The timeline's zoom and scroll belong to the project, not the layout:
    // switching must not throw them away (`set_tree` asks for a refit). A
    // timeline still following the window keeps fitting by itself.
    let (pps, scroll, keep) = (ed.px_per_sec, ed.tl_scroll, !ed.auto_fit);
    ed.set_tree(tree_of(w));
    if keep {
        ed.px_per_sec = pps;
        ed.tl_scroll = scroll;
        ed.fit_pending = false;
    }
    ed.rail = rail(&w.rail);
    app.prefs.workspace = w.name.clone();
}

/// [`apply`], and also the scopes and the Inspector tab, and a spoken
/// "Colour workspace" for a screen reader.
pub fn switch(app: &mut StudioApp, ctx: &eframe::egui::Context, w: &Workspace) {
    apply(app, w);
    crate::editor::dock_ui::reset_key_scope(ctx);
    // Scopes docked in the tree show there; otherwise over the picture.
    if !app.editor.dock.contains(crate::dock::Panel::Scopes) {
        crate::editor::scopes::set_on(ctx, w.scopes);
    }
    ctx.data_mut(|d| {
        if w.tab.is_empty() {
            d.remove::<String>(tab_id());
        } else {
            d.insert_temp(tab_id(), w.tab.clone());
        }
    });
    let shown = crate::i18n::t(&w.name).into_owned();
    // The layout changing is the feedback; only a screen reader is told.
    crate::a11y::announce(ctx, crate::i18n::tr_args("workspace-switched", &[("name", shown.into())]).into_owned());
}

fn tab_id() -> eframe::egui::Id {
    eframe::egui::Id::new("workspace-inspector-tab")
}

/// The Inspector tab the last workspace asked for, until it has been shown.
pub fn take_tab(ctx: &eframe::egui::Context) -> Option<String> {
    ctx.data(|d| d.get_temp::<String>(tab_id()))
}

/// The Inspector found the tab and showed it.
pub fn tab_shown(ctx: &eframe::egui::Context) {
    ctx.data_mut(|d| d.remove::<String>(tab_id()));
}

/// The workspace used last (by name), if it still exists.
pub fn saved(app: &StudioApp) -> Option<Workspace> {
    find(&app.prefs.workspace)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `all` as it was written before the file was kept.
    fn old_all(own: Vec<Workspace>) -> Vec<Workspace> {
        let mut out: Vec<Workspace> = build_in().into_iter().map(|b| own.iter().find(|o| o.name.eq_ignore_ascii_case(&b.name)).cloned().unwrap_or(b)).collect();
        out.extend(own.into_iter().filter(|o| !build_in().iter().any(|b| b.name.eq_ignore_ascii_case(&o.name))));
        out
    }

    #[test]
    fn the_merged_list_is_what_all_always_was() {
        let mine = |n: &str, w: f32| Workspace { name: n.into(), library_w: w, ..build_in().remove(0) };
        for own in [vec![], vec![mine("colour", 111.0)], vec![mine("Mine", 222.0), mine("EDIT", 333.0), mine("Other", 444.0)]] {
            assert_eq!(merged(&builtin(), own.clone()), old_all(own));
        }
        assert_eq!(builtin(), build_in(), "the kept built-ins are the made ones");
    }

    #[test]
    fn the_kept_list_is_dropped_when_this_process_changes_it() {
        let first = all();
        assert_eq!(first, all(), "the same list within the second");
        forget();
        assert_eq!(all(), first, "read again, and the same");
    }

    #[test]
    fn builtins_cover_the_tasks_and_name_real_library_sections() {
        let names: Vec<String> = builtin().into_iter().map(|w| w.name).collect();
        assert_eq!(names, ["Edit", "Colour", "Audio", "Effects", "Motion", "Review"]);
        for w in builtin() {
            assert_eq!(rail(&w.rail).name(), w.rail, "{}", w.name);
        }
    }

    #[test]
    fn every_builtin_has_an_action_an_icon_and_a_translation() {
        for w in builtin() {
            assert!(ACTIONS.iter().any(|(_, n)| *n == w.name), "{} has a shortcut action", w.name);
            assert_ne!(icon(&w.name), crate::icons::Icon::SquaresFour, "{} has its own icon", w.name);
            for l in studio_i18n::Lang::ALL {
                for text in [&w.name, &w.about] {
                    let id = studio_i18n::id_for_source(text).unwrap_or_else(|| panic!("`{text}` is not in en.ftl"));
                    assert!(studio_i18n::catalogue(l).contains(id) || studio_i18n::is_pending(id), "{id} in {}", l.tag());
                }
            }
        }
    }

    #[test]
    fn each_builtin_differs_and_keeps_the_timeline() {
        let all = builtin();
        for (i, a) in all.iter().enumerate() {
            assert!(a.timeline_open, "{} keeps the timeline", a.name);
            for b in &all[i + 1..] {
                assert_ne!((a.library_w, a.inspector_open, a.inspector_w, a.timeline_h, &a.rail), (b.library_w, b.inspector_open, b.inspector_w, b.timeline_h, &b.rail), "{} and {}", a.name, b.name);
            }
        }
    }

    #[test]
    fn colour_has_scopes_looks_and_the_adjust_tab() {
        let c = builtin().into_iter().find(|w| w.name == "Colour").unwrap();
        assert!(c.scopes);
        assert_eq!(c.rail, "Looks");
        assert_eq!(c.tab, "Adjust");
    }

    #[test]
    fn shortcut_actions_find_their_workspace() {
        assert_eq!(for_action("workspace-motion"), Some("Motion"));
        assert_eq!(for_action("workspace-vfx"), Some("Effects"));
        assert_eq!(for_action("workspace-nope"), None);
    }

    #[test]
    fn layouts_saved_under_the_earlier_names_are_the_renamed_workspaces() {
        let mine = |n: &str, w: f32| Workspace { name: n.into(), library_w: w, ..build_in().remove(0) };
        let own = vec![mine("VFX", 111.0), mine("motion graphics", 222.0), mine("Production", 333.0), mine("Mine", 444.0)];
        let all = merged(&builtin(), own);
        let names: Vec<&str> = all.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names, ["Edit", "Colour", "Audio", "Effects", "Motion", "Review", "Mine"], "no duplicates, no orphans");
        assert_eq!([all[3].library_w, all[4].library_w, all[5].library_w], [111.0, 222.0, 333.0], "their changes are kept");
        // An old name still finds it.
        assert_eq!(canonical("VFX"), "Effects");
        assert_eq!(canonical("Mine"), "Mine");
    }

    #[test]
    fn old_saved_files_without_the_new_fields_still_load() {
        let json = r#"[{"name":"Mine","library_open":false,"library_w":300.0,"inspector_open":true,"inspector_w":330.0,"timeline_open":true,"timeline_h":250.0,"rail":"Media"}]"#;
        let w: Vec<Workspace> = serde_json::from_str(json).unwrap();
        assert!(!w[0].scopes && w[0].tab.is_empty() && w[0].dock.is_none());
        // A file from before docking is the fixed layout with its sizes.
        let t = tree_of(&w[0]);
        assert_eq!(t, crate::dock::Tree::legacy((false, 300.0), (true, 330.0), (true, Some(250.0))));
        assert_eq!(t.legacy_fields(), ((false, 300.0), (true, 330.0), (true, Some(250.0))));
    }

    #[test]
    fn builtin_trees_match_their_task_and_round_trip() {
        use crate::dock::Panel;
        for w in builtin() {
            let t = tree_of(&w);
            assert!(t.is_showing(Panel::Viewer) && t.is_open(Panel::Timeline), "{}", w.name);
            // The old fields agree with the tree, for an older Studio.
            assert_eq!(t.legacy_fields().0 .0, w.library_open, "{}", w.name);
            assert_eq!(t.legacy_fields().1 .0, w.inspector_open, "{}", w.name);
            let json = serde_json::to_string(&w).unwrap();
            assert_eq!(serde_json::from_str::<Workspace>(&json).unwrap(), w);
        }
        let by = |n: &str| tree_of(&builtin().into_iter().find(|w| w.name == n).unwrap());
        let colour = by("Colour");
        assert!(colour.is_showing(Panel::Scopes) && colour.find(Panel::Scopes).unwrap()[..2] == colour.find(Panel::Inspector).unwrap()[..2], "scopes under the inspector");
        // The mixer beside the timeline, both showing.
        let audio = by("Audio");
        assert!(audio.is_showing(Panel::Sound) && audio.is_showing(Panel::Timeline) && audio.find(Panel::Sound) != audio.find(Panel::Timeline));
        assert_eq!(audio.find(Panel::Sound).unwrap()[..1], audio.find(Panel::Timeline).unwrap()[..1], "in the timeline's zone");
        let review = by("Review");
        assert!(review.is_showing(Panel::Review) && review.find(Panel::Review) == review.find(Panel::Inspector), "review notes with the Inspector");
        assert!(review.is_showing(Panel::Ai) && review.find(Panel::Ai) == review.find(Panel::Library), "the Assistant with the Library");
        assert_eq!(by("Edit"), crate::dock::Tree::legacy((true, 340.0), (true, 300.0), (true, Some(300.0))));
    }

    /// Switching workspace keeps the selection, the playhead and a zoomed
    /// timeline where they were.
    #[test]
    fn switching_workspace_keeps_selection_playhead_and_timeline_view() {
        let ctx = eframe::egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let _serial = crate::ui_tests::serial();
        let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
        app.editor.px_per_sec = 91.0;
        app.editor.tl_scroll = eframe::egui::vec2(333.0, 12.0);
        app.editor.auto_fit = false;
        app.editor.selection = vec![3, 4];
        app.editor.playhead = 5 * crate::view::SEC;
        for w in build_in() {
            switch(&mut app, &ctx, &w);
            assert!(crate::kit::recent(&ctx).iter().all(|t| !t.text.contains("orkspace")), "no toast for {}", w.name);
            assert_eq!(app.editor.px_per_sec, 91.0, "zoom after {}", w.name);
            assert_eq!(app.editor.tl_scroll, eframe::egui::vec2(333.0, 12.0), "scroll after {}", w.name);
            assert!(!app.editor.fit_pending, "no refit after {}", w.name);
            assert_eq!(app.editor.playhead, 5 * crate::view::SEC, "playhead after {}", w.name);
            assert_eq!(app.editor.selection, vec![3, 4], "selection after {}", w.name);
        }
    }
}
