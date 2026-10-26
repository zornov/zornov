---
name: plugins-automation
use: Use when work repeats: macros, batch runs, plugins, shortcuts.
topics: plugins, plugin, ofx, openfx, macros, macro, automation, batch-runs, shortcuts, workspaces, batch
tools: plugins, automation, repeat, edit, layout, made
---
**Order**
1. If the same op repeats inside one edit, use `repeat` (`{op: "repeat", times, do: [...]}`, with `{i*0.5}` style expressions) rather than many calls.
2. If a sequence of steps repeats across the work, record a macro: `automation {action: "record", name}` ... `stop`, then `run` it; `macros` lists them.
3. Many projects: `automation {action: "batch", macro | ops, export, projects}`; try on one project first.
4. Extra effects, generators, transitions, importers and exporters come from plugins: `plugins` lists, searches, installs, enables and disables them. Install only after the person says yes. Installed effects and transitions are used by name with `apply` and `transition`. Prefer a built-in way when one exists.
5. Shortcuts and workspaces are part of `automation` (`shortcuts`, `workspaces`) and `layout`.

**Checks**
- A batch is run on one project and looked at before the rest.
- A macro does what its description says.

**Mistakes**
- Batch-exporting before one test export has been checked.
- Installing a plugin for something a built-in op already does.

## Reference: Plugins and automation

**Plugins** add effects, generators, transitions, importers, exporters and
panels. They are WebAssembly built with `studio-plugin-api`, sandboxed (no
files, no network, a memory cap and a time limit), or OpenFX bundles from
other vendors (native code, each in its own process, off until the person
turns it on).

- When nothing built in does a job, `plugins {"action": "search",
  "query": "film burn light leak"}` looks through what can be installed;
  `plugins {"action": "list"}` shows what is installed and available.
- Installing brings someone else's code in: ask the person first. In the
  window Zornov asks them when you call `plugins {"action": "install",
  "id": ...}`; from a shell, install only what they asked for.
- Installed effects and transitions are in the catalogue like built-in
  ones (Effects ▸ Plugins, Cuts ▸ Plugins): `{"op": "apply", "item":
  "Duotone", "clips": [4]}`, `{"op": "transition", "clip": 4, "item":
  "toolkit:iris"}`, and their settings with the `effect` op. Their ids are
  `plugin:item` (`ofx:<identifier>` for OpenFX).
- `import` hands files Zornov cannot read to a plugin that can (a `.ppm`
  becomes a picture in the media bin, a `.markers` list becomes markers).
  `plugins {"action": "export", "plugin", "exporter"}` writes the project
  in a plugin's format; `plugins {"action": "panel", "plugin", "panel",
  "button"}` presses a panel's button (its changes are one undo step).
- **Write one when nothing fits.** `plugins {"action": "create", "id":
  "warm-vignette", "name": "Warm vignette", "description": "...",
  "params": [{"id": "amount", "label": "Amount", "kind": "float", "min":
  0, "max": 1, "default": 0.7}], "pixel": "let (dx, dy) = (x / w - 0.5, y
  / h - 0.5); let k = 1.0 - p.float(\"amount\") * (dx * dx + dy *
  dy).sqrt(); [px[0] * k, px[1] * k, px[2] * k, px[3]]"}`. The body is
  Rust for `fn pixel(px: Px, x, y, w, h: f32, ctx: &Context, p: &Params)
  -> Px`: `px` is premultiplied linear RGBA at pixel centre (x, y),
  `ctx.time` seconds, parameters by id (`p.float`, `p.int`, `p.bool`,
  `p.choice`, `p.colour` (premultiplied), `p.point` (0..1)). Kinds:
  float, int, bool, choice (`options`), colour, point. Add `"animated":
  true` when it changes over time, `"wgsl"` with `fn effect(pos:
  vec2<f32>) -> vec4<f32>` for the GPU (`tex(pos)` samples, `u.p[i]` the
  parameters, `u.time`, `u.size`). Zornov builds it with Rust on this Mac,
  runs it on a test picture, installs it and answers with what to apply;
  a compile error comes back for you to fix. `source` instead of `pixel`
  takes a whole `lib.rs` (several effects, a transition, a panel: see
  docs/plugins.md). It needs the person's consent like an install.

**Macros** record edits once and replay them. `automation {"action":
"record", "clips": [4]}`, make the changes with `edit` (a grade, effects,
a transform, keyframes), then `automation {"action": "stop", "name":
"Night look"}`. `automation {"action": "run", "name": "Night look",
"clips": "all"}` puts the same change on every picture clip (`main`,
`video`, `titles`, `sound` or ids); a macro recorded on one clip runs
once per clip, one recorded on several maps them in timeline order. The
person records macros in the window too (Library ▸ AI ▸ All tools, or
⌥⌘R) and can run yours.

**Batch**: `automation {"action": "batch", "macro": "Night look",
"export": "Social 1080p", "projects": ["/path/a", "/path/b"]}` opens each
project, runs the macro (and/or `ops`), exports and saves; it asks the
person first in the window. Without `projects` it works on this one.

**Shortcuts and workspaces** are the person's: `automation {"action":
"shortcuts"}` and `{"action": "workspaces"}` list them (Edit, Colour,
Audio, VFX and their own); they change them in Settings ▸ Shortcuts and
the Layout menu. Switching workspaces and moving panels is the `layout`
tool (next section).

## Reference: tool `plugins`

Plugins: effects, generators, transitions, importers, exporters and panels from other people (sandboxed WebAssembly) or OpenFX. action: list (installed and available), search {query} (what could do a job), install {id or path} (only after the person says yes; the window asks them), enable / disable / uninstall {id}, create {id, name, description, pixel: body of fn pixel(px, x, y, w, h, ctx, p) -> Px, params: [{id,label,kind,min,max,default}], wgsl?} or {id, source: a whole lib.rs} to write and build an effect plugin when nothing fits (needs Rust on this Mac), import {path}, export {plugin, exporter, path?}, panel {plugin, panel, button?, values?}, ofx {on?} (load OpenFX plugins; native code, ask first), add_index {url}, refresh. Installed effects and transitions are used with the edit ops apply and transition, by name or as plugin:item.

## Reference: tool `automation`

Macros and batch work. action: record {clips: the clips the change is made on} then make the changes with edit, stop {name, description} saves it; run {name, clips: ids or all, main, video, titles, sound} replays it (one clip recorded: once per clip; several: in timeline order); macros lists them; delete {name}. batch {macro? and/or ops?, export? (a preset name or export args), clips?, projects?: [project folders]}: the same work on many clips, or on every project listed, each saved. shortcuts and workspaces list the person's keyboard shortcuts and panel layouts (Edit, Colour, Audio, VFX).
