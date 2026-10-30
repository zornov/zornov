// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The menus in the macOS menu bar. `editor::topbar` owns the menu data (the
//! rows of File, Edit, Clip, Project, View and Help, each with the thing it
//! does); the in-window menu row draws it elsewhere, and on macOS this builds
//! the application's NSMenu from the same rows. A pick from the menu bar is
//! queued as the same [`Do`] the in-window menu runs and performed on the next
//! frame by [`frame`], so a row does the same from either place.
//!
//! One owner per key: a row's key goes to the menu bar only when it has ⌘ (or
//! is a function key), so plain keys typed into a field are never taken, and
//! only while the row is enabled. When the menu bar owns a key the window never
//! sees it; when a row is disabled (a modal is open, the screen is not the
//! editor) AppKit leaves the key alone and the window's own handling takes it.

use crate::app::{Overlay, StudioApp};
use crate::dock::Panel;
use crate::editor::Action;
use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// What a menu row does.
#[derive(Clone)]
pub enum Do {
    Act(fn() -> Option<Action>),
    Copy,
    Cut,
    Paste,
    /// Group, ungroup, link, unlink, freeze, reverse, copy-look, paste-look.
    ClipKey(&'static str),
    Panel(Panel),
    /// A workspace, by name.
    Space(String),
    ResetLayout,
}

/// One menu row.
pub struct Item {
    pub what: Do,
    pub label: String,
    /// Its keys: an action's from the keymap, or a fixed key.
    pub shortcut: String,
    pub icon: Option<crate::icons::Icon>,
    pub enabled: bool,
    /// A checkmark, for rows that are on or off.
    pub checked: Option<bool>,
}

pub enum Row {
    Item(Item),
    Sep,
    Heading(&'static str),
    /// The "Save layout as…" name field: only the in-window menu has it.
    SaveLayout,
}

pub struct Menu {
    pub title: &'static str,
    pub rows: Vec<Row>,
}

impl Menu {
    pub fn items(&self) -> impl Iterator<Item = &Item> {
        self.rows.iter().filter_map(|r| if let Row::Item(i) = r { Some(i) } else { None })
    }
}

/// How the menu bar reaches the top bar's menu data and runner (set by the top bar).
struct Provider {
    model: fn(&StudioApp, &egui::Context) -> Vec<Menu>,
    perform: fn(&mut StudioApp, &egui::Context, Do),
}

static PROVIDER: OnceLock<Provider> = OnceLock::new();
static PENDING: Mutex<Vec<Do>> = Mutex::new(Vec::new());
static WINDOW: OnceLock<egui::Context> = OnceLock::new();
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Tells the menu bar where the menus come from. The top bar calls this each
/// frame; only the first call counts.
pub fn register(model: fn(&StudioApp, &egui::Context) -> Vec<Menu>, perform: fn(&mut StudioApp, &egui::Context, Do)) {
    PROVIDER.get_or_init(|| Provider { model, perform });
}

/// Whether the menus are in the menu bar (so the window leaves its own row out).
pub fn installed() -> bool {
    INSTALLED.load(Ordering::Relaxed)
}

/// Queues a pick from the menu bar and wakes the window.
fn push(what: Do) {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).push(what);
    if let Some(ctx) = WINDOW.get() {
        ctx.request_repaint();
    }
}

/// Once a frame, before the window's own key handling: runs what the menu bar
/// picked, then brings the menu bar up to date.
pub fn frame(app: &mut StudioApp, ctx: &egui::Context) {
    WINDOW.get_or_init(|| ctx.clone());
    drain(app, ctx);
    #[cfg(target_os = "macos")]
    native::sync(app, ctx);
}

/// Runs the queued picks. In a text field the editing rows (undo, cut, copy,
/// paste, select all) go to the field, as the keys did before the menu bar had them.
fn drain(app: &mut StudioApp, ctx: &egui::Context) {
    let Some(p) = PROVIDER.get() else { return };
    let picks = std::mem::take(&mut *PENDING.lock().unwrap_or_else(|e| e.into_inner()));
    for what in picks {
        if let Some(edit) = edit_of(&what).filter(|_| ctx.text_edit_focused()) {
            forward(ctx, edit);
            continue;
        }
        (p.perform)(app, ctx, what);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Edit {
    Undo,
    Redo,
    SelectAll,
    Cut,
    Copy,
    Paste,
}

fn edit_of(what: &Do) -> Option<Edit> {
    match what {
        Do::Cut => Some(Edit::Cut),
        Do::Copy => Some(Edit::Copy),
        Do::Paste => Some(Edit::Paste),
        Do::Act(f) => match f() {
            Some(Action::Undo) => Some(Edit::Undo),
            Some(Action::Redo) => Some(Edit::Redo),
            Some(Action::SelectAll) => Some(Edit::SelectAll),
            _ => None,
        },
        _ => None,
    }
}

/// Hands an editing row to the focused text field as the event its key makes.
fn forward(ctx: &egui::Context, edit: Edit) {
    use egui::{Event, Key, Modifiers};
    let key = |key: Key, modifiers: Modifiers| Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers };
    let event = match edit {
        Edit::Undo => key(Key::Z, Modifiers::COMMAND),
        Edit::Redo => key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT),
        Edit::SelectAll => key(Key::A, Modifiers::COMMAND),
        Edit::Cut => Event::Cut,
        Edit::Copy => Event::Copy,
        Edit::Paste => match clipboard_text() {
            Some(text) => Event::Paste(text),
            None => return,
        },
    };
    ctx.input_mut(|i| i.events.push(event));
}

#[cfg(target_os = "macos")]
fn clipboard_text() -> Option<String> {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
    // SAFETY: reading the general pasteboard's string from the main thread.
    unsafe { NSPasteboard::generalPasteboard().stringForType(NSPasteboardTypeString).map(|s| s.to_string()) }
}

#[cfg(not(target_os = "macos"))]
fn clipboard_text() -> Option<String> {
    None
}

const SHIFT: u64 = 1 << 17;
const CONTROL: u64 = 1 << 18;
const OPTION: u64 = 1 << 19;
const COMMAND: u64 = 1 << 20;

/// A row's keys ("⇧⌘S", "F1") as an NSMenuItem key equivalent and modifier
/// mask, if the menu bar should own them: only keys with ⌘, and function keys.
pub fn native_key(keys: &str) -> Option<(String, u64)> {
    let mut flags = 0;
    let mut rest = keys.trim();
    while let Some(c) = rest.chars().next() {
        flags |= match c {
            '⇧' => SHIFT,
            '⌃' => CONTROL,
            '⌥' => OPTION,
            '⌘' => COMMAND,
            _ => break,
        };
        rest = &rest[c.len_utf8()..];
    }
    let function = |n: u32| char::from_u32(0xF704 + n - 1).map(String::from);
    let key = match rest {
        "←" => "\u{F702}".to_owned(),
        "→" => "\u{F703}".to_owned(),
        "↑" => "\u{F700}".to_owned(),
        "↓" => "\u{F701}".to_owned(),
        "⌫" => "\u{8}".to_owned(),
        "⌦" => "\u{F728}".to_owned(),
        "⏎" | "↩" => "\r".to_owned(),
        "⎋" => "\u{1b}".to_owned(),
        "⇥" => "\t".to_owned(),
        "Space" => " ".to_owned(),
        f if f.len() > 1 && f.starts_with('F') && f[1..].parse::<u32>().is_ok_and(|n| (1..=12).contains(&n)) => {
            // A function key is the menu bar's with or without ⌘.
            return function(f[1..].parse().ok()?).map(|k| (k, flags));
        }
        one if one.chars().count() == 1 => one.to_lowercase(),
        _ => return None,
    };
    (flags & COMMAND != 0).then_some((key, flags))
}

/// The keys the menu bar owns, in menu order (the first row with a key keeps
/// it): the one place that decides, so no key is bound twice.
pub fn owned_keys<'a>(rows: impl IntoIterator<Item = &'a Item>, taken: &[(String, u64)]) -> Vec<Option<(String, u64)>> {
    let mut seen: Vec<(String, u64)> = taken.to_vec();
    rows.into_iter()
        .map(|i| {
            let k = native_key(&i.shortcut)?;
            if seen.contains(&k) {
                return None;
            }
            seen.push(k.clone());
            Some(k)
        })
        .collect()
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use objc2::rc::Retained;
    use objc2::runtime::Sel;
    use objc2::{declare_class, msg_send_id, mutability, sel, ClassType, DeclaredClass};
    use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem};
    use objc2_foundation::{MainThreadMarker, NSObject, NSString};
    use std::cell::RefCell;

    declare_class!(
        struct Picker;

        unsafe impl ClassType for Picker {
            type Super = NSObject;
            type Mutability = mutability::MainThreadOnly;
            const NAME: &'static str = "ZornovMenuPicker";
        }

        impl DeclaredClass for Picker {
            type Ivars = ();
        }

        unsafe impl Picker {
            #[method(picked:)]
            fn picked(&self, item: &NSMenuItem) {
                // SAFETY: reading the tag of the item AppKit just sent.
                let tag = unsafe { item.tag() };
                pick(tag);
            }
        }
    );

    /// The menu bar as last built.
    struct Installed {
        shape: String,
        /// Keeps the target alive (menu items do not).
        _picker: Retained<Picker>,
        items: Vec<Retained<NSMenuItem>>,
        dos: Vec<Do>,
    }

    thread_local! {
        static STATE: RefCell<Option<Installed>> = const { RefCell::new(None) };
    }

    fn pick(tag: isize) {
        let what = STATE.with(|s| s.try_borrow().ok().and_then(|s| s.as_ref().and_then(|s| s.dos.get(tag as usize).cloned())));
        if let Some(what) = what {
            push(what);
        }
    }

    fn ns(s: &str) -> Retained<NSString> {
        NSString::from_str(s)
    }

    fn make(mtm: MainThreadMarker, title: &str, action: Option<Sel>, key: &str, flags: u64) -> Retained<NSMenuItem> {
        // SAFETY: a plain item; `action` is a selector the target or the responder chain answers.
        let item = unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(mtm.alloc(), &ns(title), action, &ns(key)) };
        if !key.is_empty() {
            item.setKeyEquivalentModifierMask(NSEventModifierFlags(flags as usize));
        }
        item
    }

    /// A menu with `items`, and the item that holds it in the menu bar.
    fn submenu(mtm: MainThreadMarker, title: &str, items: &[Retained<NSMenuItem>]) -> (Retained<NSMenuItem>, Retained<NSMenu>) {
        // SAFETY: a plain menu made on the main thread.
        let menu = unsafe { NSMenu::initWithTitle(mtm.alloc(), &ns(title)) };
        // Enabled and checked are ours each frame, not the responder chain's.
        unsafe { menu.setAutoenablesItems(false) };
        for item in items {
            menu.addItem(item);
        }
        let top = make(mtm, title, None, "", 0);
        top.setSubmenu(Some(&menu));
        (top, menu)
    }

    fn system(mtm: MainThreadMarker, title: &str, action: Sel, key: &str, flags: u64) -> Retained<NSMenuItem> {
        make(mtm, title, Some(action), key, flags)
    }

    /// Whether the window is on a screen with the editor (the editor's rows mean something there).
    fn in_editor(app: &StudioApp) -> bool {
        use crate::cli::Screen;
        matches!(app.screen, Screen::Editor | Screen::Export | Screen::Settings)
    }

    /// Rows that work on every screen.
    fn everywhere(what: &Do) -> bool {
        matches!(what, Do::Act(f) if matches!(f(), Some(Action::Open | Action::Settings | Action::Help | Action::Tour | Action::Shortcuts | Action::CheckUpdates)))
    }

    fn shape(menus: &[Menu], settings: &Item) -> String {
        let mut s = format!("{}|{}|", settings.label, settings.shortcut);
        for m in menus {
            s.push_str(&format!("[{}]", m.title));
            for r in &m.rows {
                match r {
                    Row::Item(i) => s.push_str(&format!("{}|{}|{};", i.label, i.shortcut, i.checked.is_some())),
                    Row::Sep => s.push('-'),
                    Row::Heading(h) => s.push_str(h),
                    Row::SaveLayout => {}
                }
            }
        }
        s
    }

    pub fn sync(app: &StudioApp, ctx: &egui::Context) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let Some(p) = PROVIDER.get() else { return };
        let menus = (p.model)(app, ctx);
        // Settings is in the application menu on a Mac; File's own row loses its key to it.
        let settings = Item { what: Do::Act(|| Some(Action::Settings)), label: "Settings…".into(), shortcut: crate::keymap::hint(&app.prefs, "settings"), icon: None, enabled: true, checked: None };
        let flat: Vec<&Item> = std::iter::once(&settings).chain(menus.iter().flat_map(Menu::items)).collect();
        let shape = shape(&menus, &settings);
        let blocked = app.overlay != Overlay::None || crate::onboarding::tour_running(app);
        let editor = in_editor(app);
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            if state.as_ref().is_none_or(|s| s.shape != shape) {
                *state = Some(build(mtm, shape, &menus, &settings, &flat));
                INSTALLED.store(true, Ordering::Relaxed);
            }
            let Some(st) = state.as_ref() else { return };
            for (nsitem, item) in st.items.iter().zip(&flat) {
                let on = item.enabled && !blocked && (editor || everywhere(&item.what));
                // SAFETY: plain state setters on items this module made, on the main thread.
                unsafe {
                    nsitem.setEnabled(on);
                    nsitem.setState(isize::from(item.checked == Some(true)));
                }
            }
        });
    }

    fn build(mtm: MainThreadMarker, shape: String, menus: &[Menu], settings: &Item, flat: &[&Item]) -> Installed {
        let picker: Retained<Picker> = {
            let this = mtm.alloc::<Picker>().set_ivars(());
            // SAFETY: NSObject's init on a freshly allocated, initialised-ivar instance.
            unsafe { msg_send_id![super(this), init] }
        };
        // Keys the system rows below hold.
        let taken = [("q".to_owned(), COMMAND), ("h".to_owned(), COMMAND), ("h".to_owned(), COMMAND | OPTION), ("m".to_owned(), COMMAND)];
        let keys = owned_keys(flat.iter().copied(), &taken);
        let mut items = Vec::new();
        let mut dos = Vec::new();
        let mut row = |item: &Item, key: &Option<(String, u64)>| {
            let (k, f) = key.clone().unwrap_or_default();
            let ns_item = make(mtm, &crate::i18n::t(&item.label), Some(sel!(picked:)), &k, f);
            // SAFETY: the picker outlives the menu (it is kept in `Installed`).
            unsafe {
                ns_item.setTarget(Some(&picker));
                ns_item.setTag(dos.len() as isize);
            }
            dos.push(item.what.clone());
            items.push(ns_item.clone());
            ns_item
        };

        let mut keys = keys.iter();
        let settings_row = row(settings, keys.next().unwrap_or(&None));
        let app_items = [
            system(mtm, "About Zornov", sel!(orderFrontStandardAboutPanel:), "", 0),
            NSMenuItem::separatorItem(mtm),
            settings_row,
            NSMenuItem::separatorItem(mtm),
            system(mtm, "Hide Zornov", sel!(hide:), "h", COMMAND),
            system(mtm, "Hide Others", sel!(hideOtherApplications:), "h", COMMAND | OPTION),
            system(mtm, "Show All", sel!(unhideAllApplications:), "", 0),
            NSMenuItem::separatorItem(mtm),
            system(mtm, "Quit Zornov", sel!(terminate:), "q", COMMAND),
        ];
        // SAFETY: as above.
        let main = unsafe { NSMenu::initWithTitle(mtm.alloc(), &ns("")) };
        unsafe { main.setAutoenablesItems(false) };
        main.addItem(&submenu(mtm, "Zornov", &app_items).0);

        let window = submenu(
            mtm,
            "Window",
            &[system(mtm, "Minimize", sel!(performMiniaturize:), "m", COMMAND), system(mtm, "Zoom", sel!(performZoom:), "", 0)],
        );
        let mut help = None;
        for m in menus {
            let mut rows = Vec::new();
            for r in &m.rows {
                match r {
                    Row::Item(i) => rows.push(row(i, keys.next().unwrap_or(&None))),
                    Row::Sep => rows.push(NSMenuItem::separatorItem(mtm)),
                    Row::Heading(h) => {
                        let head = make(mtm, &crate::i18n::t(h), None, "", 0);
                        unsafe { head.setEnabled(false) };
                        rows.push(head);
                    }
                    Row::SaveLayout => {}
                }
            }
            let (top, menu) = submenu(mtm, &crate::i18n::t(m.title), &rows);
            if m.title == "Help" {
                // Help sits last, after Window.
                main.addItem(&window.0);
                help = Some(menu);
            }
            main.addItem(&top);
        }
        let app = NSApplication::sharedApplication(mtm);
        app.setMainMenu(Some(&main));
        // SAFETY: both menus are in the menu bar just set.
        unsafe {
            app.setWindowsMenu(Some(&window.1));
            app.setHelpMenu(help.as_deref());
        }
        Installed { shape, _picker: picker, items, dos }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(shortcut: &str) -> Item {
        Item { what: Do::ResetLayout, label: "x".into(), shortcut: shortcut.into(), icon: None, enabled: true, checked: None }
    }

    #[test]
    fn only_command_and_function_keys_are_the_menu_bars() {
        assert_eq!(native_key("⌘S"), Some(("s".into(), COMMAND)));
        assert_eq!(native_key("⇧⌘S"), Some(("s".into(), COMMAND | SHIFT)));
        assert_eq!(native_key("⌘,"), Some((",".into(), COMMAND)));
        assert_eq!(native_key("⌘\\"), Some(("\\".into(), COMMAND)));
        assert_eq!(native_key("F1"), Some(("\u{F704}".into(), 0)));
        // Keys typed into fields, and chords without ⌘, stay with the window.
        for plain in ["S", "M", "⌫", "Space", "←", "⌥1", "⌃⇧A", "", "⌘"] {
            assert_eq!(native_key(plain), None, "{plain}");
        }
    }

    #[test]
    fn a_key_is_bound_once_and_the_system_rows_keep_theirs() {
        let rows = [item("⌘S"), item("⌘S"), item("⌘Q"), item("⌘Z"), item("Space")];
        let owned = owned_keys(&rows, &[("q".into(), COMMAND)]);
        assert_eq!(owned, vec![Some(("s".into(), COMMAND)), None, None, Some(("z".into(), COMMAND)), None]);
    }

    #[test]
    fn editing_rows_are_recognised_for_text_fields() {
        assert_eq!(edit_of(&Do::Copy), Some(Edit::Copy));
        assert_eq!(edit_of(&Do::Act(|| Some(Action::Undo))), Some(Edit::Undo));
        assert_eq!(edit_of(&Do::Act(|| Some(Action::SelectAll))), Some(Edit::SelectAll));
        assert_eq!(edit_of(&Do::Act(|| Some(Action::Save))), None);
        assert_eq!(edit_of(&Do::ClipKey("group")), None);
    }

    /// A pick from the menu bar runs the action the in-window menu runs.
    #[test]
    fn a_pick_from_the_menu_bar_runs_its_action_on_the_next_frame() {
        let mut h = crate::ui_tests::Harness::new(&["--screen", "editor"]);
        // The first frame is where the top bar says where the menus come from.
        h.with_events(vec![]);
        let before = h.app.doc.project.markers.len();
        push(Do::Act(|| Some(Action::Marker)));
        assert_eq!(h.app.doc.project.markers.len(), before, "nothing runs until the frame");
        h.with_events(vec![]);
        assert_eq!(h.app.doc.project.markers.len(), before + 1);
        // Taken once.
        h.with_events(vec![]);
        assert_eq!(h.app.doc.project.markers.len(), before + 1);
        assert!(!installed(), "a headless run has no menu bar, so the window keeps its row");
    }
}
