# Writing Zornov plugins

Zornov plugins add **effects**, **generators**, **transitions**,
**importers**, **exporters** and **panels**. They are written in Rust with
the `studio-plugin-api` crate and compiled to WebAssembly
(`wasm32-unknown-unknown`). Zornov runs them in a sandbox, so a plugin
can be installed from anyone without giving it the run of the computer.

Zornov also loads existing **OpenFX** (OFX) image effect plugins; see
[OpenFX](#openfx) at the end.

Two complete examples live in `plugins/examples/`:

- `duotone`: one effect, with CPU code and a WGSL shader for the GPU.
- `toolkit`: a generator (Plasma), a transition (Iris), two importers
  (Netpbm pictures, marker lists), an exporter (clip list as CSV) and a
  panel (Shot list).

Both ship with Zornov (Library ▸ AI ▸ Available) and are built into
`plugins/dist/`.

## Quick start

```bash
rustup target add wasm32-unknown-unknown
cargo new --lib my-effect
```

`Cargo.toml`:

```toml
[package]
name = "my-effect"
version = "1.0.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
studio-plugin-api = { path = "/path/to/studio/crates/plugin-api" }

[profile.release]
opt-level = "s"
lto = true
panic = "abort"
strip = true
```

`src/lib.rs`:

```rust
use studio_plugin_api::*;

struct Invert;

impl Effect for Invert {
    fn info(&self) -> EffectInfo {
        EffectInfo::new("invert", "Invert")
            .description("Turns the picture into its negative.")
            .category(Category::Stylise)
            .param(Param::float("amount", "Amount", 0.0, 1.0, 1.0))
    }

    fn apply(&self, _ctx: &Context, frame: &mut Frame, p: &Params) {
        let k = p.float("amount");
        for px in frame.pixels.iter_mut() {
            for c in 0..3 {
                // Premultiplied: the negative of c is alpha - c.
                px[c] += (px[3] - 2.0 * px[c]) * k;
            }
        }
    }
}

fn plugin() -> Plugin {
    Plugin::new("invert", "Invert", "1.0.0")
        .author("You")
        .description("A negative.")
        .license("Apache-2.0")
        .effect(Invert)
}

export_plugin!(plugin);
```

Build and install:

```bash
cargo build --release --target wasm32-unknown-unknown
```

The module is `target/wasm32-unknown-unknown/release/my_effect.wasm`.
Install it from the AI library (Library ▸ AI ▸ Installed ▸ Install from
file…), ask the AI ("install ~/my_effect.wasm"), or copy it to
`~/Movies/Studio/Plugins/invert/plugin.wasm` and restart. The effect is
then in **Effects ▸ Plugins**, its parameters in the inspector, and the
AI can apply it as `invert:invert` or "Invert".

## What a plugin is

One plugin is one `.wasm` module. [`Plugin`] says who it is (`id`, `name`,
`version`, `author`, `description`, `license`, `homepage`) and lists its
items. The plugin id is lower case letters, digits, `-` and `_`; item ids
the same. In projects an effect, generator or transition is stored as
`<plugin id>:<item id>` (`duotone:duotone`), so **never rename an id or a
parameter id once people use it**: their projects would lose the effect
(it is skipped, settings kept, until a plugin with that id is back).

### Pictures

Every picture is linear-light, premultiplied RGBA `f32` (`Px = [f32; 4]`),
rows top to bottom, the same as Zornov's own effects. Values above 1 are
highlights; do not produce negative or non-finite values (Zornov replaces
them, but the result will not be what you meant). The output has the
input's size: an effect changes `frame.pixels` in place.

`Frame` has `width`, `height`, `pixels`, `get(x, y)`, `set(x, y, px)`,
`sample(x, y)` (bilinear, clamped, pixel centres at +0.5) and
`snapshot()` (a copy, for reading neighbours while writing).

`Context` has `time` (seconds since the clip started), `frame_rate`,
`seed` (per clip, so two clips with the same effect differ) and, for
transitions, `progress` (0 to 1, strictly inside: Zornov itself shows the
outgoing picture at 0 and the incoming one at 1). Effects must be pure
functions of their inputs, parameters and context: Zornov renders frames
in any order, on several threads, and may run several copies of a plugin.

### Parameters

```rust
Param::float("radius", "Radius", 0.0, 50.0, 8.0)   // slider min..max, default
Param::int("count", "Count", 1, 12, 4)
Param::bool("invert", "Invert", false)
Param::choice("shape", "Shape", &["Circle", "Square"], 0)
Param::colour("tint", "Tint", [1.0, 0.8, 0.6, 1.0]) // linear RGB, straight alpha
Param::point("centre", "Centre", [0.5, 0.5])        // 0..1, top-left origin
```

`.fixed()` makes a parameter non-animatable. Every other parameter can be
keyframed in Zornov; the plugin always sees the value at the frame being
rendered. Read them with `p.float`, `p.int`, `p.bool`, `p.choice` (index),
`p.colour` (premultiplied) and `p.point`. Up to 16 parameters per item.

### Effects, generators, transitions

- `Effect`: `info()` and `apply(ctx, frame, params)`.
- `Generator`: same shape, `generate`. A generator draws over its input
  (Zornov puts it on an empty layer for a picture of its own); give it a
  `mix`/opacity parameter as the built-in ones have. It shows up with the
  effects under Plugins.
- `Transition`: `apply(ctx, a, b, params)`; `a` is the outgoing picture
  and receives the result, `b` the incoming one. It is in **Cuts ▸
  Plugins**.

`EffectInfo` also takes `.category(...)` (where it is grouped),
`.animated()` (the result depends on time or seed; otherwise Zornov may
reuse a frame while nothing changes), `.wgsl(body)` and `.gpu_only()`.

### GPU shaders

Give an item `.wgsl(include_str!("my.wgsl"))` and Zornov's GPU renderer
runs the shader instead of the CPU code (the CPU code is still used for
thumbnails, by the CPU renderer and as the reference). You write only the
body; Zornov puts its prelude in front:

- Effects and generators: write `fn effect(pos: vec2<f32>) -> vec4<f32>`.
  `pos` is in pixels (centres at +0.5). `tex(pos)` samples the input,
  `u.size` is the frame size, `u.time` the clip time, `u.seed` the seed
  (as raw bits; `seed_u()` gives the `u32`), `u.p[i]` parameter `i` as a
  `vec4` (numbers, switches and choices in `.x`, points in `.xy`, colours
  premultiplied in all four). Helpers: `luma`, `pct_h`, `hash2`, `noise1`,
  `sstep`, `srgb_encode`, `srgb_decode`, `sample_mirror`.
- Transitions: write `fn transition(pos: vec2<f32>, t: f32) -> vec4<f32>`;
  `tex_a(pos)`, `tex_b(pos)`, `t` is the progress, `ease(i, t)` an easing
  by index.

The binding layout (for reference): effects use group 0 bindings 0 input
texture, 1 sampler, 2 output `rgba16float` storage texture, 3 uniforms
(`size`, `time`, `seed`, `p: array<vec4<f32>, 16>`); transitions 0 A, 1 B,
2 sampler, 3 output, 4 uniforms with `progress` in place of `time`. The
full preludes are `crates/picture/fx/src/wgsl/common.wgsl` and
`transition_common.wgsl`. Zornov checks the shader with naga when the
plugin loads: a shader that does not compile is reported in the plugin's
details and the CPU code is used. `.gpu_only()` items (no CPU code) are
turned off when their shader fails and pass the picture through on the
CPU renderer, so prefer giving CPU code.

### Importers

```rust
impl Importer for Pnm {
    fn info(&self) -> ToolInfo {
        ToolInfo::new("pnm", "Netpbm picture").extension("ppm").extension("pgm")
    }
    fn import(&self, name: &str, bytes: &[u8]) -> Result<Imported, String> { ... }
}
```

Zornov sends files whose type it does not read itself to the first
enabled importer for their extension (from the media import, a drop, or
the AI's `import`). Return `Imported::Image { width, height, rgba8 }`
(straight-alpha sRGB, saved as a PNG in the project's `Imported` folder and
added to the media bin) or `Imported::Ops(json)`: the AI's edit ops as a
JSON array, applied as one undo step (markers, an edit list, captions).
See the agent guide (`guide {"topic": "ops"}`) for the ops.

### Exporters

`Exporter::export(&project) -> Result<Vec<u8>, String>` gets the project
as JSON and returns the file's bytes; `ToolInfo::extension` is the file
type. The project is:

```json
{"name": "Trailer", "canvas": [1920, 1080], "fps": 24, "duration": 92.5,
 "clips": [{"id": 3, "name": "Opening", "kind": "video", "lane": 0, "lane_id": 7,
            "start": 0.0, "dur": 4.0, "text": "", "effects": ["duotone:duotone"],
            "media": "/path/to/opening.mp4"}],
 "markers": [{"at": 12.0, "note": "Drop"}]}
```

Times are seconds. `lane` is 0 for the main lane, 1, 2, ... for overlays
upwards and -1, -2, ... for sound lanes downwards.

### Panels

A panel is a small column of widgets in the AI library (Installed). Zornov
calls `Panel::update(&event, &project)` when it is opened and whenever a
button is pressed or a slider or toggle changes; you return what to show
and, optionally, edit ops to apply:

```rust
fn update(&self, event: &PanelEvent, project: &Json) -> PanelView {
    let n = project.get("clips").items().len();
    let mut view = PanelView {
        widgets: vec![
            Widget::Heading("Clips".into()),
            Widget::Label(format!("{n} clips")),
            Widget::Button { id: "mark".into(), label: "Mark each".into() },
        ],
        ..Default::default()
    };
    if event.button.as_deref() == Some("mark") {
        view.ops = Some(/* [{"op": "marker", "at": ..., "note": ...}] */ Json::Arr(vec![]));
        view.message = Some("Marked".into());
    }
    view
}
```

Widgets: `Heading`, `Label`, `Slider { id, label, min, max, value }`,
`Toggle { id, label, value }`, `Button { id, label }`, `Separator`.
`event.values` has every slider and toggle by id, `event.selection` the
selected clip ids. Panels keep no state between calls: everything they
need comes in the event and the project.

### JSON

`studio_plugin_api::Json` is a small JSON value with `parse`, `to_json`,
`get`, `at`, `items`, `as_f64`, `as_str`, `as_bool`, and `Json::obj().with
(key, value)` to build objects; plugins need no other crates. You may use
other crates that build for `wasm32-unknown-unknown` without WASI.

### Logging

`studio_plugin_api::log("...")` writes to the plugin's log (shown in its
details and to the AI). Errors from your code are worth logging: a
panicking plugin call fails that call only and leaves the picture as it
was.

## Testing

Plugins are plain Rust, so test them natively with `cargo test`: build a
`Plugin` and call `run_effect`, `run_generator`, `run_transition`,
`run_import`, `run_export` and `run_panel` with `defaults(&params)` or
your own packed values (see the tests in `plugins/examples/*/src/lib.rs`).
`crates/plugins/plugins/tests/examples.rs` shows the examples installed into a
scratch folder and run through Zornov's own effect lookups. To try a
plugin on a scratch project without opening a window, give Zornov a
scratch home (so nothing lands in your own Zornov folder) and send it
calls on stdin, one JSON object per line:

```bash
HOME=/tmp/plugin-home zornov agent --headless --project /tmp/plugin-test --out /tmp/plugin-out - < calls.jsonl
```

with `calls.jsonl` holding, for example,
`{"tool": "plugins", "args": {"action": "install", "id": "duotone"}}`,
an `edit` that makes a card, an `edit` with `{"op": "apply", "item":
"Duotone", "clips": [ID]}`, and `{"tool": "frame", "args": {"t": 1}}` to
look at the result.

## The sandbox

A plugin may import exactly one function, `studio.log`. There is no WASI:
no files, sockets, clock, randomness, environment or threads. Each
instance has a memory cap (1.5 GB, enough for a 4K transition) and each
call a time limit (20 s). A plugin that traps, runs out of time or asks
for too much memory fails that call (the picture passes through
unchanged); after eight failures its items stop running until it is
loaded again. Modules that import anything else are refused at install.

Build for `wasm32-unknown-unknown`. `wasm32-wasip1` modules import WASI
and are refused.

## Versions and the ABI

`studio-plugin-api` is versioned on its own (1.0.0). Every 1.x release
keeps **ABI 1**; Zornov refuses a module whose `studio_abi` it does not
know and says which side to update. The ABI, for people writing plugins
without the crate (any language that targets wasm32):

| export | signature | |
|---|---|---|
| `memory` | | linear memory |
| `studio_abi` | `() -> i32` | 1 |
| `studio_alloc` | `(size: i32) -> i32` | 16-byte aligned block |
| `studio_free` | `(ptr: i32, size: i32)` | |
| `studio_manifest` | `() -> i64` | UTF-8 JSON (below) as `ptr << 32 \| len` |
| `studio_effect` | `(index, ctx, pixels, w, h, params, n) -> i32` | in place; 0 = ok |
| `studio_generate` | same | |
| `studio_transition` | `(index, ctx, a, b, w, h, params, n) -> i32` | result in `a` |
| `studio_import` | `(index, name, name_len, data, len) -> i64` | blob |
| `studio_export` | `(index, data, len) -> i64` | blob |
| `studio_panel` | `(index, data, len) -> i64` | JSON |

`ctx` points at four 32-bit words: time (f32), frame rate (f32), seed
(u32), progress (f32). `pixels`, `a`, `b` point at `w * h` RGBA `f32`
pixels; `params` at `n` `vec4<f32>`s packed as for shaders. The host
allocates every input with `studio_alloc` and frees it after the call;
results returned as `ptr << 32 | len` are freed by the host with
`studio_free`. A blob is `[u32 little-endian header length][header
JSON][payload]`: importers answer `{"kind": "image", "width", "height"}`
with RGBA8 payload, `{"kind": "ops", "ops": [...]}`, or `{"kind": "error",
"message"}`; exporters `{"ok": true}` with the file as payload, or `{"ok":
false, "message"}`. Panels get `{"event": {"button", "values",
"selection"}, "project": {...}}` and answer `{"widgets": [...], "ops"?,
"message"?}` or `{"error"}`.

The manifest:

```json
{"abi": 1, "api": "1.0.0", "id": "toolkit", "name": "Toolkit", "version": "1.0.0",
 "author": "", "description": "", "license": "Apache-2.0", "homepage": "",
 "effects": [{"id": "x", "name": "X", "description": "", "category": "stylise",
              "animated": false, "cpu": true, "wgsl": "fn effect(...)...",
              "params": [{"id": "amount", "label": "Amount", "kind": "float",
                          "min": 0, "max": 1, "default": 0.5, "animatable": true}]}],
 "generators": [], "transitions": [],
 "importers": [{"id": "pnm", "name": "Netpbm", "description": "", "extensions": ["ppm"]}],
 "exporters": [], "panels": []}
```

## Publishing

A plugin index is a JSON file on an https server:

```json
{"plugins": [{"id": "film-grain", "name": "Film grain", "version": "1.2.0",
              "description": "Organic 16 and 35 mm grain.", "author": "You",
              "license": "MIT", "provides": "1 effect", "items": ["Film grain"],
              "url": "https://example.com/film-grain-1.2.0.wasm",
              "sha256": "<64 hex digits of the module>"}]}
```

People add an index from the AI ("add the plugin index https://..."), or
in `~/Movies/Studio/Plugins/sources.json` as `{"indexes": ["https://..."]}`.
Its plugins appear under Available; a download is installed only if its
SHA-256 matches. Newer versions offer an update.

## Plugins the AI writes

When nothing fits, Zornov's AI can write an effect itself (`plugins
{"action": "create"}`): it supplies the body of a per-pixel function and
its parameters (or a whole `lib.rs`), and Zornov writes a crate under
`~/Movies/Studio/Plugin sources/<id>/` against a copy of this API, builds
it offline with the Rust toolchain on the Mac, runs every item on a test
picture and installs it. The source stays there to read, change and
rebuild. This needs Rust and the wasm target on the Mac; without them the
AI says so.

## OpenFX

Zornov hosts OFX image effect plugins (`.ofx.bundle`) from
`/Library/OFX/Plugins`, `~/Library/OFX/Plugins`, `OFX_PLUGIN_PATH` and
folders listed in `~/Movies/Studio/Plugins/ofx.json`. Loading them is off
until the person turns it on (Library ▸ AI ▸ Available ▸ OpenFX, or the
AI with their consent), because OFX plugins are native code from their
vendors and are **not sandboxed**. Each bundle runs in its own worker
process (`zornov ofx-worker <bundle>`), so a plugin that crashes or hangs
takes only its worker down; it is restarted up to three times, then left
off until Zornov restarts, and the picture passes through.

Supported: the OFX Image Effect API 1.4 in the filter, general (one
source), generator and transition contexts; CPU rendering of whole frames
in 32-bit float, 16-bit or 8-bit RGBA; the property, image effect,
parameter, memory, multithread and message suites; double, integer,
boolean, choice, RGB/RGBA, 2D/3D (points when their double type says so)
and string parameters. OFX plugins become `ofx:<identifier>` effects,
generators and transitions with their parameters in the inspector.

Not supported (plugins that need these fail to load or render and are
listed with the reason): overlays and custom interacts, GPU rendering
(OpenGL, Metal, CUDA, OpenCL), tiles and multi-resolution, reading other
frames than the current one (temporal effects), parametric parameters,
animated parameters inside the plugin (Zornov animates a parameter by
sending its value for each frame), the progress, timeline and dialog
suites. Zornov works in linear light; frames go to OFX plugins sRGB-encoded
(most expect display-referred pixels) unless the plugin is listed under
`linear` in `ofx.json`.

Tested with a bundle written against the C ABI in
`crates/plugins/plugins/tests/ofx_fixture`. Commercial bundles vary in what they
need from a host. FxFactory's bundle, for example, loads and runs in its
worker on this Mac but offers no effects without the FxFactory app and
its products installed; GPU-only plugins (many commercial ones) will not
render here.

## Automation

Macros, batch runs and workspaces are not plugins but work with them; see
the agent guide (`guide {"topic": "plugins"}`): record edits once and
replay them on other clips (Library ▸ AI ▸ All tools, ⌥⌘R to record,
⌥⌘M to run the last one), run a macro and an export over many projects
(`automation {"action": "batch"}`), set keyboard shortcuts in Settings ▸
Shortcuts (clashes are flagged), and switch workspaces (Edit, Colour,
Audio, VFX, or your own) from the Layout menu or ⌥1-⌥4.
