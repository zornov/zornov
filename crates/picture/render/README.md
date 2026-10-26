# studio-render

The frame compositor: turns a timeline at a time into pixels. Resolves the
active clips, decodes their sources through an LRU byte-budgeted cache,
converts everything into a linear-light premultiplied working space, runs
effects, grades and masks, places each layer with its transform and
motion pose, blends with the W3C formulas, and converts to the output
space with dithering. Two backends run the same pipeline: the GPU (wgpu
compute passes in WGSL) for the viewer and export, and the CPU as the
reference and fallback. Golden and agreement tests keep them within
tolerance.

```
Scene (model adapter / StaticScene)
  └─ resolve(t)         stacking, transitions, adjustment clips, motion poses
       └─ per layer     decode cache → convert to working space → crop
                        → effects (fx) → grade (colour) → masks → blur
                        → minify → transform → opacity → blend
  └─ output             working → output space → dither → RGBA8 / RGBA16
```

## Using it

```rust
use std::sync::Arc;
use studio_render::{best_renderer, DecodeCache, SourceRegistry, Sources};
use studio_render::model_scene::{ModelScene, register_media};
use studio_render::preview::{Previewer, PreviewRequest};

let registry = SourceRegistry::new();
register_media(&project, &registry);                 // media id → file path
let cache = DecodeCache::new(512 << 20);             // byte budget, shared
let renderer = best_renderer(Sources::new(registry.clone(), cache.clone()));

// Viewer: newest request wins, stale ones are dropped, the size asked for
// is the size rendered (capped at the canvas, never lowered on its own).
let preview = Previewer::new(renderer);
preview.set_scene(Arc::new(ModelScene::new(project.clone())));
preview.request(PreviewRequest { time: playhead, size: viewer_px, sequence: n, output: Default::default() });
if let Some(frame) = preview.poll() { /* frame.rgba is encoded RGBA8 */ }

// Export: frames in order from a worker pool with back-pressure.
let report = studio_render::export::Exporter::run(scene, settings, progress, make_renderer, |frame| encoder.push(frame));

// Hit-testing: which layer is under a canvas point.
let hit = studio_render::hit::layer_at(&scene, &mut sources, [x, y], t);
```

The renderer reads a [`Scene`](src/scene.rs): clips with every animated
value already evaluated at `t`. [`model_scene::ModelScene`](src/model_scene.rs)
(feature `model`, on by default) adapts a `studio_model::Project`;
`StaticScene` builds one in code for tests, benchmarks and hover previews.

## Modules

| module | contents |
|---|---|
| `scene` | `Scene` trait, `ClipDesc`, `TransitionDesc`, `Canvas`, `StaticScene` |
| `resolve` | draw list for one instant: order, transitions, adjustment clips, motion poses |
| `source`, `yuv` | `MediaSource` (MP4/MOV via the in-repo `mp4`, `h264`, `hevc`; PNG via `media`), `DecodeCache` (LRU by bytes, keyed by media and frame index), decoded pictures and their conversion |
| `colour_space` | H.273 code points → transfer / primaries / matrix, decode tables, HDR → SDR tone map |
| `frame`, `filter` | the working picture (`Frame`), blur, rotation, comparison helpers |
| `transform` | placement (position, scale, rotation, anchor, crop, flips) + motion pose → affine; transformed bounds and hit-testing |
| `blend` | W3C Compositing and Blending Level 1 modes on un-premultiplied colour |
| `mask` | shapes as signed distance fields, Gaussian feather by `erf`, expansion, invert, opacity, add / subtract / intersect |
| `effects` | adapters to `studio_fx` (effects, transitions, motion presets) and `studio_colour` (grades), built-in generators (solid, gradient, shapes) |
| `text` | `TextRenderer` hook for the `text` crate; a stub draws nothing until it is installed |
| `compositor` | `FrameRenderer` trait and the CPU reference |
| `gpu` | the wgpu backend: context, texture pool, every pass as a compute shader |
| `output` | working → sRGB / BT.709 / Display P3 / BT.2020 PQ / HLG, triangular dither, RGBA8 and RGBA16 |
| `preview` | render thread that always draws the newest request |
| `export` | worker pool, contiguous chunks per worker, in-order delivery, bounded look-ahead |
| `hit` | `layer_at(point, time)` and `layer_bounds` through each layer's real transformed rectangle |

## Working space and colour

Linear light, BT.709 primaries, premultiplied alpha; `f32` on the CPU,
`rgba16float` on the GPU; 1.0 is diffuse white and values above 1 are kept
until output. Video decodes by its H.273 description: BT.601 / 709 / 2020
matrices, limited or full range, 8 to 16 bits; sRGB, BT.709 (through the
BT.1886 display EOTF, which is what a viewer sees), pure gamma, PQ and HLG
transfer; primaries other than 709 go through a Bradford-adapted matrix.
PQ and HLG sources are tone mapped for SDR outputs with a documented knee
curve (identity to 0.8, exponential roll-off to 2.0 on the largest
channel, so hue is kept) in `colour_space::tone_map`. HLG uses the
BT.2100 system gamma of 1.2 at 1000 cd/m², applied per channel so the CPU
and GPU stay identical. The decode domain is clamped to −0.1..1.2 on both
paths (limited-range super-blacks and super-whites pass).

Masks live in the layer's own normalised space, so they move with the
layer. Feather is the Gaussian CDF of the signed distance, which is the
exact blur of a straight edge at one `erf` per pixel. Blend modes follow
the W3C formulas on un-premultiplied colour; `add` is the unclamped linear
dodge. Dither is ±1 LSB triangular noise from a position/channel/frame
hash, so scrubbing and export agree and so do the two backends.

## GPU backend

Every pass is an 8×8 compute shader (`src/gpu/shaders/`): Y′CbCr planes
upload as `r8uint`/`r16uint` textures and convert in `convert.wgsl`;
`composite.wgsl` inverse-maps each canvas pixel into the layer and blends;
`mask.wgsl`, `resample.wgsl` (half, blur, rotate), `fill.wgsl`,
`output.wgsl` (both depths). The `fx` crate's effect and transition shaders
and the `colour` crate's grade shader run as they are, with explicit
bind-group layouts. Anything without a shader (effects or transitions the
`fx` crate only implements on the CPU, grades with dehaze / clarity /
sharpen, unknown transitions) falls back: the picture is read back,
processed on the CPU and uploaded again, so output is always complete.
Uploads of CPU pictures go through staging buffers in the command
encoder, never `write_texture`, because a write would run before the
passes already recorded.

`GpuContext::new()` opens the best adapter; `GpuContext::from_device`
wraps the application's existing egui device so textures could stay on
the GPU. `STUDIO_RENDER_NO_GPU=1` forces the CPU path (tests, benchmark).

## Tests

`cargo test -p studio-render`:

- unit tests per module (blend identities, W3C soft-light reference
  points, mask SDFs, feather vs Gaussian CDF, transform inverses and
  hit-testing, cache eviction, dither statistics, output round trips,
  tone map continuity, f16 rounding);
- `tests/golden.rs`: 102 golden images (96×54, `tests/golden/`): every
  blend mode, every mask shape and combine rule, transforms (fit, scale,
  rotate, position, anchor, crop, flip, minify, magnify, opacity, all at
  once, portrait source, motion entry / exit / rest / loop), every
  transition at two moments plus the unknown-kind fallback, generators,
  an adjustment clip, a nested scene, the text hook with and without a
  renderer. Tolerance 3/255 on 99.5 % of pixels. Regenerate with
  `RENDER_BLESS=1 cargo test -p studio-render --test golden`;
- `tests/agreement.rs`: CPU vs GPU on a busy scene at three times and
  three sizes, stage by stage (each feature alone), output encoding at
  both depths, an HDR 10-bit PQ source inside a nested scene. Tolerance
  1 % of the pixel's magnitude with 0.5 % outliers (quantising effects
  flip at boundaries because the GPU holds `f16`). Skipped without a GPU;
- `tests/shaders.rs`: every WGSL module validates with naga, including
  the `fx` and `colour` shaders this crate runs;
- `tests/decode.rs`: real H.264 / HEVC fixtures against reference YUV
  dumps, in random seek order, through the cache.

## Benchmark

`cargo bench -p studio-render`: 1920×1080 at 30 fps, three stacked clips
(full frame; half size, rotated, screen blend; small, 85 % opacity) with
two shader-backed effects each (posterise + chromatic aberration, wave +
RGB split, pixelate + edge detect). Sources are procedural 1080p test
patterns, generated on the CPU every frame in place of decoding; their
cost is the first row. Measured on an Apple M4 Max (14 cores), release
profile, 60 frames:

| backend | case | ms/frame | fps |
|---|---|---:|---:|
| (source) | three 1080p test pictures | 3.2 | 310 |
| gpu (Apple M4 Max) | 1080p, rgba8 out | 10.5 | 95 |
| gpu (Apple M4 Max) | 1080p, f32 readback | 12.2 | 82 |
| gpu (Apple M4 Max) | 960×540 preview, rgba8 out | 4.5 | 221 |
| cpu (14 threads) | 1080p, rgba8 out | 331.1 | 3 |
| cpu (14 threads) | 960×540 preview, rgba8 out | 313.3 | 3 |

The GPU numbers include uploading three 1080p pictures, a synchronous
readback of the encoded frame, and the source generation. The target
(real time for this case on an M1-class laptop) leaves about 3× headroom
on this machine; it has not been measured on an M1. The CPU path runs
the `fx` crate's effects at source resolution whatever the target size,
which is why the preview row is no faster: it is the reference, not the
viewer path.

## Not done / limits

- Time effects (`echo`, `strobe`, `frame_blend`, `posterise_time`) get
  no frame history yet (`EffectContext::history` is empty), and
  `difference_key` gets no clean plate.
- Motion presets are drawn at one pose per frame; the `fx` crate's
  shutter poses (motion blur for presets) are not sampled.
- Text waits for the `text` crate: install its `TextRenderer` with
  `text::set_text_renderer`; until then text layers draw nothing.
- The GPU reads every finished frame back to the CPU. Keeping the output
  texture on the GPU for egui is the next step once `app` shares its
  device (`GpuContext::from_device`).
- HDR output spaces (PQ, HLG) encode but are not yet wired to an HDR
  export container.
