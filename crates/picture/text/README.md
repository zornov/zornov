# studio-text

Text for Zornov: font discovery and fallback, shaping and layout, the
styling stack, a catalogue of text styles, animated text templates, and
captions with word timings and subtitle export. Pure Rust, CPU
rasterisation; output is `studio_colour::Image` (linear-light,
premultiplied RGBA f32) so `render` and `fx` composite it directly.

## Using it

```rust
use studio_text::*;

let engine = TextEngine::discover();            // system + bundled fonts
let style = TextStyle::new(FontRequest::bold("Inter"), 64.0, Fill::Solid(Rgba::WHITE))
    .with_stroke(Stroke::new(Rgba::BLACK, 2.0))
    .with_shadow(Shadow::new(Rgba::BLACK, 6.0, 4.0, 90.0).opacity(0.7));
let block = TextBlock::plain("Hello, world").with_layout(LayoutParams::default().wrap(900.0));

// Into its own layer (plus where it sits relative to the layout box):
let layout = engine.layout(&block, &style, &Canvas::HD);
let raster = engine.paint(&layout, &style, &Hooks::NONE);

// Or straight onto a frame, centred at a pixel position:
let mut frame = Image::new(1920, 1080);
engine.render_onto(&mut frame, &block, &style, [960.0, 540.0], &Hooks::NONE);
```

Lengths in styles are **canvas units**: 1 unit = 1/1080 of the canvas
height, so a 40-unit title looks the same at 1080p and 4K. Font size is
5..300 units; letter and line spacing are percentages of the font size.
Colours are typed as sRGB (`Rgba::hex("#FFD23F")`) and converted once at
paint time.

## Modules

| module      | contents |
|-------------|----------|
| `font`      | `FontLibrary`: system fonts (fontdb), the bundled library (from the content folder's `catalog.json`, lazily loaded), user imports; CSS-style family/weight/italic matching with synthetic bold/italic; per-script fallback chains and emoji fallback |
| `emoji`     | Emoji drawn from the bundled OpenMoji / Twemoji PNG sets (deterministic, sharp at title sizes); colour fonts remain the fallback |
| `layout`    | Shaping (rustybuzz) and layout: bidi, script itemisation, line breaking (UAX #14), alignment incl. justify, vertical text, letter/line spacing, per-span styles, text on an arc (−360..360°), fit-to-box, max lines, per-character / word / line boxes |
| `style`     | `TextStyle` (fill: solid / gradient / image; multiple strokes; shadow; glow; background box; extrusion; colour cycle; underline / strike; case; opacity), `SpanStyle`, `LayoutParams`, `TextBlock`, `Canvas` |
| `raster`    | zeno analytic coverage at the final pixel size with sub-pixel phases, mask and path caches, blur, rounded rects, affine image drawing, compositing |
| `paint`     | The painter: background → extrusion → shadow → glow → strokes → face, driven by per-unit `Hooks` (offset / scale / rotation / opacity / colour per character, word or line) |
| `anim`      | Reveal presets (fade, rise, drop, pop, slide, type, spin, bounce, zoom, flip, spring) with stagger and order; loop motions (wave, pulse, jitter, sway, shimmer, wobble) |
| `scene`     | Animated multi-layer scenes: text / rect / ellipse / line layers, keyframe tracks with easing, intro / outro phases that stretch to any clip length, `Scene::frame` and `Scene::render` |
| `styles`    | 70 text styles in 10 groups (`styles::catalogue()`, `find`, `search`) |
| `templates` | 46 animated templates in 8 groups (`templates::all()`, `find`, `search`); also shipped as `templates/*.json` |
| `captions`  | `CaptionTrack`: cues with word timings and speakers; 14 caption styles (karaoke, word pop, boxed lines, bottom bar, speech bubble, typed reveal…); segmentation by pauses / sentences / reading speed; balanced two-line breaking; safe margins; `Cue::retime` (edit text, keep timings); find & replace, filler removal; SRT / VTT / ASS export and SRT / VTT import |
| `png_io`    | PNG read / write for review sheets and golden tests |

## Animation hooks

`Hooks` carries up to three callbacks `(index, count) -> UnitPose` for
characters, words and lines. The painter evaluates them once per unit and
applies the combined transform to every stage (strokes, shadows and glows
move with the glyphs). `fx` uses the same `anim::stagger` shape so text
animations in both crates agree.

## Templates

A template is a `Scene`: fields the user edits, layers positioned as
canvas fractions, keyframe tracks in seconds within the intro or outro
phase, and per-layer reveals. Play it with
`scene.render_onto(&engine, &mut frame, t, clip_duration, &values)`;
a 5 s design plays correctly in a 12 s clip because the intro runs from
the start, the outro ends at the end and the middle holds.

`render` can also call `Scene::frame` to get each layer as an image plus
its affine transform and composite on the GPU.

## Captions

```rust
let words = vec![Word::new("Hello", 0.0, 0.4), Word::new("world", 0.45, 0.9)];
let mut track = CaptionTrack::from_words(words, captions::find("caption.sunlit").unwrap().clone(), |_| None);
track.render_onto(&engine, &mut frame, 0.5);
std::fs::write("out.srt", track.to_srt())?;
std::fs::write("out.ass", track.to_ass(&Canvas::HD))?;
```

## Review and tests

- `cargo run --release --example contact_sheet` writes `review/styles.png`,
  `review/templates-*.png` and `review/captions.png`.
- `cargo run --example gen_templates` rewrites `templates/*.json`
  (a test keeps the JSON in step with the Rust builders).
- `cargo test` runs unit tests and the golden-image tests in
  `tests/golden.rs` (one PNG per feature in `tests/golden/`; set
  `UPDATE_GOLDEN=1` after an intended change). Both need the bundled
  content folder (`STUDIO_CONTENT_DIR`, default `~/Movies/Studio/Content`,
  an old folder name); without it the golden tests pass vacuously.
