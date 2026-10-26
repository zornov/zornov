# Onboarding, help and workspaces

## First run

* **Tour** (`onboarding.rs`): the first time an editor opens, nine steps point
  at the Library, Viewer, Inspector, Timeline, workspace tabs and Export, and
  explain the AI editor and help. `→`/`Enter` next, `←` back, `Esc` skip; each
  step is announced to a screen reader; steps whose panel is closed are
  skipped. "Replay the welcome tour" (Settings ▸ General ▸ Accessibility, the
  Help menu, or the end of a help article) brings it back. The tour never runs
  in `--shot` runs or the headless agent.
* **Tips**: one-line hints, one at a time, only when they apply (an empty
  timeline, a selected clip, the fourth edit…), each shown once, never within
  12 s of the previous one. "Got it" or "Turn off tips" dismisses; a setting
  turns them off for good.
* **Templates and sample projects** (`templates.rs`) on the home screen:
  * *Start from a template*: YouTube video, short film, music video,
    animated story, documentary, podcast, ad. Each sets the canvas and
    frame rate (an ad is 9:16, a short film 2.39:1 at 24 fps…), makes the
    lanes that kind of production uses, adds markers for its structure (hook
    / chapters / end screen; acts; verse and chorus at 120 bpm; scenes; cold
    open and parts; segments; hook / problem / solution / proof / call to
    action) and placeholder titles. Marker notes and title text are written in
    the language of the interface.
  * *Learn by doing*: four guided projects on the built-in sample footage with
    the first steps written as markers: basics, colour, titles and motion,
    audio.

## Help

`F1` (or Help ▸ Search help) opens a searchable help with 25 articles
(`help-<topic>-title`, `-keys`, `-body` in the catalogue, so they are
translated). Search matches every word of the query against titles, keywords
and text, in the interface language and English, with prefix matching and
character matching for Japanese and Chinese. With a region focused (`F6`),
F1 opens that region's article.

The AI editor reads the same articles through the **`howto`** tool
(`studio agent howto "how do I split a clip"`), which returns the best
articles as JSON; `guide` tells it to use `howto` for questions about Studio
itself. Add an article by adding a `Topic` in `help.rs` and the three
messages in `en.ftl` (the tests tell you what is missing in each language).

## Workspaces

`workspaces.rs` (the built-in six and the person's own saved layouts, kept in
`<Studio>/Workspaces.json`). One click in the top bar (icons, the names are
tooltips; narrower windows and the person's own layouts are in the Layout
menu) or `⌥1`–`⌥6` (Settings ▸ Shortcuts changes them):

| Workspace | Library | Inspector | Timeline | Also |
|---|---|---|---|---|
| Edit | Media | open | default | |
| Colour | Looks | wide, Adjust tab, Scopes docked under it | short | |
| Audio | Audio | Audio tab | tall, Sound panel a tab behind it | |
| VFX | Effects | Adjust tab | medium | |
| Motion graphics | Motions | Animation tab | medium | |
| Production | AI + wide Library | closed, Review docked in its place | short | |

A workspace sets only what the user can drag anyway (the whole dock tree:
which panels are where, open and how big; the Library section, the scopes
over the picture, the Inspector tab); nothing is locked and everything stays
adjustable. `workspaces::switch` also shows the scopes, opens the Inspector
tab and tells a screen reader (the toast is what it hears);
`workspaces::apply` only lays out the panels. A saved workspace keeps its
tree in `dock`; the old size and open fields are still written (from the
tree) for an older Studio, and a file without `dock` is read as the fixed
layout those fields describe. The layout is remembered in `prefs.dock` (and
`prefs.layout` from it), the name in `prefs.workspace`; an editor opens with
the Library section of the workspace last used.

## Docking

`dock.rs` is the layout: a tree of splits (side by side or stacked, each
child sized in points, as a ratio of the split, or filling what is left) and
tab stacks, any node collapsible. The default tree is the fixed layout Studio
had before (a test checks it rect for rect at several window sizes, and the
screenshots are pixel-identical). `editor/dock_ui.rs` draws it: a panel's
header (or its tab, in a stack of two or more) is the handle; dragging shows
where it will land (left, right, above, below a panel, into it as a tab, or
along the window's edge), Esc cancels, and the toast offers Undo. The same
moves are in each panel's Move menu (right-click the header or tab, `⇧F10`
on a region reached with `F6`, or Layout ▸ Move panel). Layout ▸ Panels
opens and closes panels (`⌥⌘L` `⌥⌘I` `⌥⌘T` for the Library, Inspector and
Timeline); Reset layout goes back to the default. Review, Record and
Editorial also float in a window inside the editor (a handle in it docks
them); the Sound panel takes the Timeline's place and the scopes sit over the
picture when they are not docked. Any panel but the Viewer can **pop out**
into a window of the system's own (Move menu ▸ Pop out): egui's immediate
viewports, so the panel draws from app state like a docked one; the tree
keeps it in `popped` with its window size and the neighbour it docks back
beside. Dock back in its bar, or closing the window, puts it back. Where a
platform (or a test or screenshot run) has no extra windows it shows as a
window inside the editor. The AI editor is a panel too (`Panel::Ai`): ⌘K and
the Library rail's AI item open it, by default as a tab with the Library;
its requests run on whether it is docked, hidden or popped out (the state is
`app.agent`, ticked every frame). The AI arranges panels with its `layout`
tool.

To add a built-in workspace: an entry in `builtin()`, one in `ACTIONS`, an icon
in `icon()`, a row in `SHORTCUTS` (`screens/settings.rs`) and `EDITOR_KEYS`
(`editor/mod.rs`), and its name and description in `en.ftl`.
