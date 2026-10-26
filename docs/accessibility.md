# Accessibility

Studio is built to be used without a mouse, with a screen reader, in strong
light or none, at large sizes, and by people who see colour differently.

## Screen readers

egui's AccessKit support exposes the interface to VoiceOver (macOS), Narrator
and NVDA (Windows) and Orca (Linux). What Studio adds:

* **Every shared control has a name.** `kit` widgets (buttons, icon buttons,
  toggles, segmented controls, chips, menus, sliders, fields, sections, tiles)
  report their label in the language of the interface. A test fails if a
  button, checkbox, slider or field reaches the accessibility tree without
  one (`ui_tests.rs`).
* **The timeline is browsable.** Each clip is a list item named
  "*name*, *kind* clip on *lane*, starts at *time*, lasts *n* seconds, selected".
  A clip that receives keyboard focus is selected and the playhead moves to it.
* **Regions.** The Library, Viewer, Inspector and Timeline (and the Sound,
  Scopes, Review and Record panels when docked) are landmarks. `F6` and
  `⇧F6` move between them; the region is announced and outlined. `⇧F10` on a
  region opens its "Move panel to…" menu: every place it can be docked is a
  named button ("Dock left · Viewer"), so panels move without a pointer.
* **Spoken updates.** Toasts (undo, redo, split, delete, moved, saved, export
  finished…), keyboard navigation results and workspace changes are put in a
  live region (`a11y::announce`), so a screen reader says them. Turn this off in
  Settings ▸ General ▸ Accessibility ▸ Spoken updates.
* **"Where am I?"** `⌥.` says the playhead time, how many clips are selected,
  and whether it is playing.

## Keyboard

Everything can be done from the keyboard. `Tab` and `⇧Tab` move between
controls, `Space` or `Enter` activate, `Esc` closes a popover or dialog.

| Keys | Does |
|---|---|
| `Space` `J` `K` `L` | play/pause, back, stop, forward |
| `←` `→` / `⇧←` `⇧→` | one frame / one second |
| `↑` `↓` | previous / next edit point |
| `[` `]` | previous / next clip on the lane |
| `⌥↑` `⌥↓` | the clip on the lane above / below the playhead |
| `⌥←` `⌥→` (`⇧` for a second) | nudge the selected clips one frame |
| `S` `M` `I` `O` | split, marker, in point, out point |
| `⌘Z` `⇧⌘Z` | undo, redo |
| `F6` `⇧F6` | next / previous region |
| `F1` | help (on the focused region's article if one has focus) |
| `⌥1`–`⌥6` | workspaces |
| `⇧F10` | the focused region's Move panel menu (dock it somewhere else) |
| `⌥⌘L` `⌥⌘I` `⌥⌘T` | show or hide the Library, Inspector, Timeline |
| `⌥.` | say where the playhead is |
| `.` `,` | the layer below / above at the playhead (the canvas) |
| `⌥,` | list the layers at the playhead |
| `⌘⌥←` `⌘⌥→` `⌘⌥↑` `⌘⌥↓` (`⇧` for 5 %) | move the selected layer 1 % of the picture |
| `⌘K` | the AI editor |

The tour, tips and help all work from the keyboard (`→` `←` `Esc`, `F1`).

## Themes

Settings ▸ General:

* **High contrast** (`access.rs`): black or white grounds, pure text, strong
  borders, a saturated accent, thicker focus rings and hairlines. All text
  pairs reach 7:1 (WCAG AAA). Works with the dark and light themes.
* **Colour vision**: *Red-green safe* and *Blue-yellow safe* palettes for the
  four clip colours and for warning and error colours. Tests simulate protan,
  deutan and tritan vision (Machado et al., 2009) and require the colours to
  stay apart (ΔE ≥ 18). Meaning is also carried by icons, labels and shapes
  (clip names, lane names, a warning icon), never by colour alone.
* **Interface size** 80–200 %, applied to everything.
* **Reduce motion** turns off hover, panel and popover animations.

Text contrast in every mode is checked by `access::tests`.

## Known gaps

* The preview image has no description for a screen reader; the AI editor's
  `see` tool and the clip list are the way to know what is on screen.
* Reduce motion is a setting; it does not yet follow the operating system's
  preference automatically.
* The text editor widget does not shape Arabic and Persian (see
  `docs/localisation.md`).
