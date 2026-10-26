# studio-app

The Zornov desktop application (`zornov` binary): eframe/egui on wgpu.

## Layout of the crate

| module | what it holds |
|---|---|
| `theme` | the "Darkroom" identity: the single source of colour tokens (dark + light), spacing, radii, type scale (Manrope + JetBrains Mono), surface depth; pushes the tokens into egui's style |
| `icons` | Phosphor SVGs (MIT) bundled with `include_str!`, rasterised with resvg at the exact pixel size they are drawn |
| `kit/` | shared components, built once: buttons, icon buttons, switch, slider, number field (expressions: `+10`, `*2`), field row, section card, segmented control, chips, menu button, popover/menu/modal, colour well + picker, tile grid/list, empty state, toasts, motion timing |
| `gallery` | hidden screen (⇧⌘G or `--screen gallery`) with every component in every state |
| `screens/` | Start, Export dialog, Settings |
| `editor/` | the editor: layout + splitters (`mod.rs`), top bar, Library, Viewer, Inspector, Timeline, and `render_view` (frames from `studio-render`) |
| `doc` | the open document: a `studio-model` project + `History` + the view the screens draw from |
| `bridge` | model → view projection, view diff → model `Edit`s, sample projects → model |
| `view` | plain view structs, sample projects (generated footage), formatting helpers |
| `catalogue` | Library lists from `studio-fx` (effects, transitions, motions) and `studio-colour` (looks) |
| `thumbs` | background workers for thumbnails, Library previews (real fx/colour on a sample picture) and the simple viewer frames |
| `media_io` | probing and decoding real files with `mp4`/`h264`/`hevc`/`media` |
| `files` | save/open project folders (`studio-model::file`), autosave, media import, OTIO/FCPXML |
| `export_run` | export through `studio-render`'s exporter into `encode` (H.264/MP4) and `media` (CLV, PNG, GIF) |
| `prefs` | preferences and remembered layout (eframe storage) |

## Data flow

The model is the only source of truth. `Doc::apply` sends a model
operation (`Split`, `Trim`, `Reorder`, `Move`, `RippleDelete`, …);
`Doc::edit` runs a closure over a copy of the view and `bridge::diff`
turns the difference into primitive edits (`SetClip`, `InsertClip`,
`SetLane`, `SetSettings`, …). Drags are one history transaction
(`apply_gesture` / `edit_gesture`, committed on pointer release, Esc
cancels). After every change the view is rebuilt at the playhead time, so
keyframed values show their animated value; editing a keyframed property
writes a key at the playhead.

The viewer shows frames from `studio-render` (`Previewer` over a
`ModelScene`; titles via the `text` crate, installed as render's text
renderer). Until the first rendered frame arrives, or when Settings →
Playback → Preview engine is "Simple", a built-in compositor draws the
clips. The built-in compositor always computes layer placement for the
on-canvas handles and click-to-pick.

`render` is a default cargo feature (it brings in `studio-render`, `studio-text` and
`studio-encode`). `--no-default-features` builds the app with the simple preview only and no
export.

## Development flags

```
zornov --screen start|editor|export|settings|gallery --theme dark|light
       --size 1440x900 --project demo|empty|one|big --select N
       --shot out.png [--frames N] [--scroll Y]
       --tabs=N --hover=X,Y --script=path --media=a.mp4,b.mov
       --layout-min --layout-max --collapsed --rail=effects --list
       --dock=scopes:viewer:right,sound:timeline:tab,review:window:left
       --simple-preview --settings-shortcuts --settings-about
```

`--shot` renders, waits for background pictures, saves a PNG of the window
and exits; it never reads or writes saved preferences, never opens file
dialogs, does not take window focus and ignores real keyboard and pointer
input (only `--script` / `--tabs` / `--hover` input reaches it). `--script` replays input (`frame move|down|up|click x y [shift|cmd|alt]`,
`frame key NAME [mods]`) to check drags and shortcuts in screenshots.

## Tests

`cargo test -p studio-app` (use `CARGO_TARGET_DIR=target-app`): theme
contrast (all text ≥ 4.5:1), icon rasterising, number expressions, model
round trips through the bridge and history, gestures as one undo step,
refused edits change nothing, save/load, probing and decoding the test
media (H.264, HEVC), and an export that renders, encodes and decodes back.

## Licences of bundled assets

`assets/fonts/` Manrope and JetBrains Mono (SIL OFL 1.1, licence files
alongside), `assets/icons/` Phosphor Icons (MIT, `Phosphor-LICENSE.txt`).
They are listed in Settings → About.
