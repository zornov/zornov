# studio-model

The project data model for Zornov: what a project *is*, how it changes, and
how it is stored and exchanged. No pixels or samples here. `render` reads
the model to draw frames; `app` changes it only through edits.

```
Project
├── settings        canvas, frame rate, background, colour space, sample rate
├── media[]         MediaItem: path (+ project-relative), probed info, caches
├── sequences[]     Sequence
│   ├── lanes[]     top→bottom: Visual…, exactly one Spine, Audio…
│   ├── clips[]     all lanes' clips, sorted by (lane, start, id)
│   ├── transitions[]  between adjacent spine clips
│   └── markers[]
├── presets[]       saved settings bundles (JSON payloads)
└── next_id         shared id counter
```

Times are `i64` microseconds (`Micros`). `FrameRate::snap` /
`snap_floor` / `frame_to_micros` round to the frame grid explicitly.

## For `render`: reading a frame

```rust
let seq = project.sequence(id).unwrap();
// Lanes are top→bottom; draw bottom-up so the top lane lands last.
for lane in seq.lanes.iter().rev().filter(|l| l.kind != LaneKind::Audio && !l.hidden) {
    let Some(clip) = seq.clip_at(lane.id, t) else { continue };
    if !clip.enabled || clip.is_gap() { continue }
    let local = t - clip.start;                    // clip-relative time
    let src = clip.source_time_at(t);              // speed, curves, reverse, freeze
    let xf = clip.transform_at(local);             // keyframes applied
    for fx in clip.effects.iter().filter(|e| e.enabled) {
        let params = clip.effect_params_at(fx, local); // missing params = fx defaults
    }
    let grade = clip.grade_params_at(local);
    for m in &clip.masks { let mp = clip.mask_params_at(m, local); }
    // then: blend with clip.blend; motion presets in clip.motion are applied on top.
}
```

* **Sources.** `ClipSource::Media` (with `video` / `audio` stream flags),
  `Sequence` (a compound clip: render the nested sequence at `src`
  recursively), `Generator` (solid, gradient, text, shape, adjustment;
  parameters by name in `params`, text in `text` runs), `Gap` (nothing).
* **Transitions.** `seq.transitions` join `left` → `right` on the spine.
  `Transition::split()` gives `(pre, post)`: the transition covers
  `[cut − pre, cut + post)` where `cut = right.start`. During it, the left
  clip keeps playing past its end and the right clip starts early; both
  sides have the media for it (an invariant). Use
  `clip.source_time_extended(t)` for the part outside a clip's edges.
* **Audio.** Clip `audio` (gain dB, pan, fades, effects, `muted`), lane
  `volume_db` / `pan` / `muted` / `solo`. Keyframable paths `audio.volume_db`
  and `audio.pan` via `clip.property_at(path, local)`.
* **Keyframes.** `clip.keyframes` maps a property path to a `Track`.
  `property_at` returns the track value when there is one, otherwise the
  static value. Paths: `transform.position|scale_x|scale_y|rotation|anchor|
  opacity|crop_left|crop_right|crop_top|crop_bottom`, `audio.volume_db|pan`,
  `effect.<id>.<param>`, `audio_effect.<id>.<param>`, `mask.<id>.<param>`,
  `grade.<param>`, `generator.<param>`. Colours interpolate in linear light.
* **Units.** Transform position is canvas pixels from the canvas centre;
  scale 1 = source fitted inside the canvas; rotation in degrees clockwise;
  crop as fractions per edge. Colours (`Rgba`) are sRGB-encoded, straight
  alpha.

## For `app`: changing the project

Every change is an `Edit` applied through a `History` (or
`Project::apply` directly, which returns the inverse):

```rust
let mut history = History::new();
history.apply(&mut project, "Split", Edit::Split { seq, at: playhead, clips: None })?;

// A drag is one undo step; every mouse move applies for live feedback.
history.begin("Move clip");
history.apply(&mut project, "", Edit::Move { seq, clips: sel.clone(), delta, to_lane: None })?;
history.commit();          // or history.cancel(&mut project) on Escape

history.undo(&mut project)?;
history.redo(&mut project)?;
```

* **All or nothing.** An edit that fails returns `EditError` and leaves the
  project untouched: `NotFound`, `Locked` (lane locked), `Invalid` (with a
  reason to show), `NoHandles` (not enough media), `Invariant` (would
  break the project). After every edit the invariants are checked
  (`Project::check_invariants`), in release builds too.
* **Primitives vs operations.** `InsertClip`, `SetClip`, `SetLane`, … change
  one object; use `SetClip` for plain attribute changes (transform, name,
  enabled, label, fades, blend). Operations (`Insert`, `Overwrite`, `Split`,
  `Trim`, `RippleDelete`, `Lift`, `Move`, `Reorder`, `Slip`, `Slide`, `Roll`,
  `Duplicate`, `Paste`, `PasteAttributes`, `DetachAudio`, `FreezeFrame`,
  `SetSpeed`, `AddTransition`, effects/masks/grade/keyframes/markers,
  lanes, `CreateCompound`/`ExpandCompound`, …) implement editor behaviour
  and clean up after themselves (merging spine gaps, dropping a trailing
  gap, shortening or removing transitions that no longer fit).
* **Ids.** One counter for everything. To know a new object's id in
  advance, take it from `project.alloc_id()` and put it in the edit;
  `0` lets the edit pick. Undo never rewinds the counter, so compare
  projects with `content_eq` rather than `==` after undo.
* **The spine** is packed from 0 with no holes; empty space is a gap clip
  (`Clip::gap`). Close a gap with `RippleDelete` of the gap clip. Lanes
  above/below are free placement with no overlaps.
* **Linked clips** (`link`, e.g. picture + detached sound) move, trim,
  slip, delete and ripple together. `group` is a selection group only.
* **Snapping** is separate from edits: `SnapTargets::collect(seq,
  Some(playhead), &dragged)` then `snap` / `snap_span` to get a corrected
  delta, then apply a normal edit. Transition limits for UI sliders:
  `project.max_transition_duration(seq, left, align)`.
* **Clipboard.** `Clipboard::copy(&project, seq, &ids)` (carries the media
  items, serialisable with `to_json` for the system clipboard), then
  `Edit::Paste`. `Edit::PasteAttributes` with an `AttrSet` for
  attributes-only paste.
* `Edit` is serde-serialisable (`{"op": "trim", …}`), handy for logging or
  scripting.

## Files

```rust
let folder = file::ProjectFolder::create("~/Movies/My Video")?; // + media/ cache/ backups/
folder.save(&project)?;                         // atomic: temp file, fsync, rename
let loaded = folder.load()?;                    // migrates, validates, re-finds moved media
folder.save_with(&loaded.project, &loaded.extra)?; // keep unknown top-level fields

let mut autosave = file::autosave::Autosave::new(); // 30 s idle, keeps 20
autosave.changed(Instant::now());               // after each history change
autosave.poll(Instant::now(), &folder, &project)?;  // from the frame loop
file::autosave::load_latest_backup(&folder)?;   // crash recovery
```

`project.json` is `{"format": "studio-project", "version": 1, "project":
{…}}`. Every struct keeps unknown JSON fields in its `extra` map and
writes them back, so older builds do not destroy newer data; clips also
have a free-form `meta` map for tools. Format changes go through
`file::migrate` (`migrate_vN_to_vN+1` functions; see that module's docs).

## Interchange

```rust
let (otio_json, report) = interchange::otio::export(&project, seq)?;
let imported = interchange::otio::import(&mut project, &text)?;   // or fcpxml::import
history.apply(&mut project, "Import", imported.edit)?;            // one undo step
let (edl, report) = interchange::edl::export(&project, seq, Default::default())?;
```

Every call returns a `Report` of what could not be carried over; show it
to the user. OTIO export embeds the full Zornov objects in
`metadata.studio`, so Zornov → OTIO → Zornov is lossless. FCPXML import
covers 1.9+ spine, connected clips, titles, gaps, compound clips,
transitions and markers. EDL export is CMX3600 for the spine (cuts,
dissolves, M2 speed lines, drop-frame for 29.97/59.94).

## Design notes

* Clip timing is `start`, `duration`, `source_in` and `speed`; the source
  out point is derived (`source_out()`), so duration and out point can
  never disagree. Freeze frames are speed 0.
* Keyframe times are clip-relative. Start trims and splits shift tracks so
  animation stays on the same frames; speed changes rescale them.
* Dependencies: `serde`/`serde_json` (file format, with exact float
  round-trips) and `roxmltree` (FCPXML parsing); `proptest` for tests.

## Tests

`cargo test -p studio-model`: unit tests per module; `tests/edits.rs`
(every edit applied, undone and redone); `tests/properties.rs` (random
edit sequences keep invariants, failed edits change nothing, full undo and
redo, file reload); `tests/file.rs` (round trip, unknown fields, atomic
writes, moved folders, autosave); `tests/interchange.rs` (OTIO round
trip and foreign OTIO, FCPXML, EDL).
