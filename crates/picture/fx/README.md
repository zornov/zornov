# studio-fx

Effects, transitions, motion presets, speed ramps, stabilisation and
tracking for Studio. Plain Rust, no runtime dependencies. Everything in the
catalogue is our own design: names, ranges and defaults were chosen so a
beginner gets a pleasing result without touching a slider.

## Conventions

- **Pictures** are [`Image`](src/image.rs): linear-light, premultiplied RGBA
  `f32`, rows top to bottom. Values above 1.0 are allowed (highlights). The
  renderer converts to and from its own textures.
- **Lengths** are in **percent of frame height**, so a look chosen on a
  small preview matches the 4K export. Positions are normalised (0,0 top
  left, 1,1 bottom right).
- **Parameters** are declared, not coded: every item has a
  `&'static [ParamDef]` (id, label, kind, range, default, animatable, unit).
  The UI builds controls from it; values live in a `ParamSet`, which always
  holds a valid, in-range value for every parameter (`set` clamps, refuses
  wrong kinds and non-finite numbers). Parameter ids are stable and go into
  project files: never rename one.
- **Time** is passed in, never read from a clock: animated effects (noise,
  shake, flicker, glitch) are pure functions of `EffectContext::time` and a
  per-clip seed, so scrubbing and export agree.

## Using it

```rust
use studio_fx::{effects, transitions, motion, EffectContext, ParamValue};

// Effect
let blur = effects::find("gaussian_blur").unwrap();
let params = blur.defaults().with("radius", ParamValue::Float(2.0));
let out = blur.apply(&EffectContext::still(), &frame, &params);

// Time effects read earlier frames (most recent first).
let echo = effects::find("echo").unwrap();
let need = echo.history_needed(&echo.defaults(), 30.0); // frames to keep
let ctx = EffectContext { time: t, frame_rate: 30.0, seed: clip_seed, history: &previous, aux: None };

// Transition: exactly `a` at 0 and exactly `b` at 1.
let wipe = transitions::find("wipe").unwrap();
let mid = wipe.apply(&Default::default(), &a, &b, 0.5, &wipe.defaults());

// Motion: entry ends at rest, exit is the entry played backwards.
let pop = motion::find("pop").unwrap();
let pose = pop.pose(motion::Phase::Entry, 0.3, &pop.defaults());
let layer = motion::apply_pose(&frame, &pose); // reference CPU drawing
```

- `EffectDef::identity` names the parameter value that leaves the input
  untouched (every effect has one), so the UI can offer "bypass" and
  hover previews can fade in.
- Generators (`solid`, `gradient`, `noise`, `grid`, `checkerboard`,
  `countdown`, `bars`) draw over their input; apply them to an empty
  layer for the bare picture. `effects::generate::bars_tone` gives the
  matching 1 kHz tone samples.
- `chroma_key` and friends output a keyed (transparent) picture;
  `difference_key` reads its clean plate from `EffectContext::aux`.
- Motion blur: moving transitions (`slide`, `push`, `cover`, `reveal`)
  average 8 positions over a film-like 180-degree shutter (`motion_blur`
  parameter, CPU and GPU alike). For motion presets,
  `MotionDef::shutter_poses` / `loop_shutter_poses` return the poses
  across the open shutter (default `motion::DEFAULT_SHUTTER` = 180
  degrees) and `motion::apply_poses` averages them; the renderer can do the
  same with its own transform pass.
- Text motion: `motion::split_text` (characters, words, lines),
  `motion::stagger` and `motion::text_poses` run any in/out preset per unit
  with overlap and order (forward, backward, centre-out, shuffled).
- Speed: `speed::SpeedCurve` is an editable Bézier ramp (keys of position
  and speed; each segment has CSS-style control points; interpolated in
  log2 space). It maps output time to source time and back, and gives the
  clip's new duration. `speed::frames_around` + `blend_frames` do frame
  blending; optical-flow in-betweens come from the `ai` crate.
- Stabilise: push frames into `stabilise::Stabiliser` (FAST corners,
  pyramidal KLT with a forward-backward check, RANSAC similarity), then
  `solve(frame_rate)` gives one `FrameCorrection` per frame: a smoothed
  path (or a locked tripod view) limited by `max_crop`, with one zoom for
  the whole clip so framing does not pump.
- Track: `track::PointTracker` (KLT) and `track::BoxTracker` (normalised
  cross-correlation with a slowly updated template, searching three
  scales). `track::to_keyframes` turns samples into position/scale
  keyframes relative to the first frame, thinned to a tolerance.
- `testcard::test_card` is a procedural picture with gradients, saturated
  and skin colours, a highlight above white, fine stripes and a
  transparent corner; the tests use it and it suits hover previews.

## GPU shaders

Per-pixel effects and the simpler transitions ship WGSL compute shaders
(`EffectDef::wgsl`, `TransitionDef::wgsl`; sources in `src/wgsl/`, the
assembled modules in `shaders.rs`). Binding layout, group 0:

| binding | effects | transitions |
|---|---|---|
| 0 | input texture | outgoing texture A |
| 1 | linear clamp sampler | incoming texture B |
| 2 | output `rgba16float` storage texture | linear clamp sampler |
| 3 | uniforms | output storage texture |
| 4 | | uniforms |

Uniforms: `size: vec2<f32>`, `time` (effects) or `progress`
(transitions), `seed`, then `p: array<vec4<f32>, 16>`, one vec4 per
parameter in declaration order, filled by `ParamSet::pack_vec4`. Entry
point `main`, workgroup 8x8. The renderer copies A or B at progress 0 and
1 instead of running a transition shader. Known differences from the CPU
reference: `pixelate` averages a 4x4 sample grid per cell, `chroma_key`
leaves choke and feather to separate passes, transition shaders support
the linear, sine in-out and cubic in-out easings (others fall back to
smoothstep), and `luma_fade`'s noise map is a simpler pattern. All shaders
are validated with naga in the tests but have not yet been run on a GPU.

## Tests

`cargo test -p studio-fx` checks, for every catalogue entry: well-formed
parameter lists; identity at the declared zero-strength value; finite
output with every parameter at each end of its range (and every choice),
with and without history; tiny and HDR inputs; determinism; golden images
at defaults (`tests/golden/`, 96x54 PNGs, tolerance 3/255 on 99.5 % of
pixels); transitions exactly equal to their inputs at 0 and 1 and
continuous just inside; motion rest poses, exits mirroring entries, loops
at zero amount; speed-curve mapping and inversion; stabilisation and
tracking against synthetic footage with known motion; WGSL validation.

Regenerate goldens after a deliberate look change with
`FX_BLESS=1 cargo test -p studio-fx` and review the PNG diffs.
`cargo run --release -p studio-fx --example preview -- <dir>` and
`--example preview_transitions -- <dir>` write contact sheets of every
effect and transition; `--example catalogue` prints the tables below.

## Catalogue

### Effects


**Blur & focus**

| id | name | GPU | parameters |
|---|---|---|---|
| `gaussian_blur` | Gaussian blur |  | `radius` 0..20 = 1; `direction` [Both / Horizontal / Vertical] = Both, fixed |
| `box_blur` | Box blur |  | `radius` 0..20 = 1; `direction` [Both / Horizontal / Vertical] = Both, fixed |
| `directional_blur` | Directional blur | yes | `length` 0..30 = 4; `angle` -180..180 = 0 |
| `spin_blur` | Spin blur | yes | `angle` 0..90 = 8; `center` = (0.5, 0.5) |
| `zoom_blur` | Zoom blur | yes | `amount` 0..1 = 0.15; `center` = (0.5, 0.5) |
| `lens_blur` | Lens blur |  | `radius` 0..10 = 1.5; `shape` [Disc / Hexagon / Octagon / Star] = Hexagon, fixed; `rotation` -180..180 = 15; `boost` 0..8 = 2; `threshold` 0..2 = 0.7 |
| `tilt_shift` | Tilt-shift |  | `focus` 0..1 = 0.55; `width` 0..1 = 0.15; `falloff` 0..1 = 0.3; `angle` -90..90 = 0; `radius` 0..10 = 1.5; `saturation` 0..1 = 0.15; `mix` 0..1 = 1 |
| `background_fill` | Background blur fill |  | `blur` 0..20 = 4; `dim` 0..1 = 0.2; `zoom` 1..2 = 1.1; `mix` 0..1 = 1 |

**Light**

| id | name | GPU | parameters |
|---|---|---|---|
| `glow` | Glow |  | `threshold` 0..2 = 0.6; `radius` 0..20 = 3; `intensity` 0..4 = 1; `tint` = rgba(1, 1, 1, 1) |
| `bloom` | Bloom |  | `threshold` 0..2 = 0.8; `radius` 0..30 = 6; `intensity` 0..4 = 0.6; `warmth` -1..1 = 0.15 |
| `light_leak` | Light leak |  | `intensity` 0..2 = 0.6; `hue` 0..360 = 25; `spread` 0..180 = 30; `speed` 0..2 = 0.25; `scale` 0.2..3 = 1; `seed` 0..999 = 0, fixed |
| `lens_flare` | Lens flare |  | `position` = (0.28, 0.25); `intensity` 0..4 = 1; `size` 0.2..3 = 1; `tint` = rgba(1, 0.82, 0.62, 1); `streak` 0..2 = 0.6; `ghosts` 0..8 = 4, fixed; `seed` 0..999 = 0, fixed |
| `god_rays` | God rays |  | `center` = (0.5, 0.15); `length` 0..1 = 0.45; `intensity` 0..4 = 1; `threshold` 0..2 = 0.6; `tint` = rgba(1, 0.82, 0.62, 1) |
| `neon_edge` | Neon edges |  | `width` 0..2 = 0.25; `glow` 0..10 = 1.5; `intensity` 0..4 = 1.5; `hue` 0..360 = 300; `rainbow` = true, fixed; `background` 0..1 = 0.15; `mix` 0..1 = 1 |

**Stylise**

| id | name | GPU | parameters |
|---|---|---|---|
| `posterise` | Posterise | yes | `levels` 2..32 = 6; `mix` 0..1 = 1 |
| `halftone` | Halftone | yes | `cell` 0.2..10 = 1.2; `angle` -90..90 = 45; `mode` [Black / Colour] = Colour, fixed; `paper` = rgba(0.94, 0.92, 0.86, 1); `mix` 0..1 = 1 |
| `comic_ink` | Comic ink |  | `levels` 2..8 = 4, fixed; `edges` 0..2 = 1; `threshold` 0..1 = 0.15; `width` 0..2 = 0.15; `saturation` 0..1 = 0.3; `mix` 0..1 = 1 |
| `oil_paint` | Oil paint |  | `radius` 0..3 = 0.6; `mix` 0..1 = 1 |
| `sketch` | Sketch |  | `detail` 0..3 = 0.8; `darkness` 0..3 = 1.5; `shading` 0..1 = 0.35; `colour` 0..1 = 0; `paper` = rgba(0.95, 0.93, 0.88, 1); `mix` 0..1 = 1 |
| `emboss` | Emboss | yes | `angle` 0..360 = 135; `depth` 0..10 = 2; `colour` = false, fixed; `mix` 0..1 = 1 |
| `pixelate` | Pixelate | yes | `size` 0..20 = 2 |
| `mosaic` | Mosaic tiles |  | `size` 0.5..20 = 3; `grout` 0..0.5 = 0.08; `grout_colour` = rgba(0.12, 0.11, 0.1, 1); `variation` 0..1 = 0.25; `bevel` 0..1 = 0.5; `mix` 0..1 = 1 |
| `kaleidoscope` | Kaleidoscope | yes | `segments` 2..32 = 6, fixed; `angle` -360..360 = 0; `center` = (0.5, 0.5); `zoom` 0.25..4 = 1; `mix` 0..1 = 1 |
| `mirror` | Mirror | yes | `mode` [Left onto right / Right onto left / Top onto bottom / Bottom onto top / Four ways] = Left onto right, fixed; `position` 0..1 = 0.5; `mix` 0..1 = 1 |
| `edge_detect` | Edge detect | yes | `mode` [Light on dark / Colour edges / Dark on light] = Light on dark, fixed; `strength` 0..4 = 1; `mix` 0..1 = 1 |

**Distort**

| id | name | GPU | parameters |
|---|---|---|---|
| `wave` | Wave | yes | `amplitude` 0..20 = 1.5; `wavelength` 1..200 = 25; `angle` -180..180 = 90; `speed` -5..5 = 0.5; `phase` 0..360 = 0 |
| `ripple` | Ripple | yes | `center` = (0.5, 0.5); `amplitude` 0..10 = 1; `wavelength` 1..50 = 6; `speed` -5..5 = 1; `decay` 0..10 = 2 |
| `twirl` | Twirl | yes | `angle` -720..720 = 120; `radius` 0..150 = 45; `center` = (0.5, 0.5) |
| `bulge` | Bulge / pinch | yes | `amount` -1..1 = 0.5; `radius` 1..150 = 40; `center` = (0.5, 0.5) |
| `fisheye` | Fisheye / defish | yes | `amount` -1..1 = 0.5; `zoom` 0.5..2 = 1 |
| `glass` | Glass |  | `distortion` 0..5 = 1; `scale` 0.5..20 = 3; `mode` [Frosted / Reeded / Tiles] = Reeded, fixed |
| `noise_displace` | Noise displace |  | `amount` 0..20 = 2; `scale` 1..200 = 25; `speed` 0..5 = 0.5; `octaves` 1..6 = 3, fixed; `seed` 0..999 = 0, fixed |
| `camera_shake` | Camera shake |  | `amount` 0..20 = 1.5; `frequency` 0.1..30 = 6; `rotation` 0..20 = 1; `roughness` 0..1 = 0.5; `auto_zoom` = true, fixed; `seed` 0..999 = 0, fixed |

**Retro**

| id | name | GPU | parameters |
|---|---|---|---|
| `vhs` | VHS |  | `amount` 0..1 = 1; `bleed` 0..5 = 0.8; `tracking` 0..1 = 0.4; `scanlines` 0..1 = 0.35; `noise` 0..1 = 0.3; `seed` 0..999 = 0, fixed |
| `film` | Old film |  | `amount` 0..1 = 1; `weave` 0..1 = 0.3; `dust` 0..1 = 0.4; `scratches` 0..1 = 0.3; `flicker` 0..1 = 0.3; `grain` 0..1 = 0.3; `tint` 0..1 = 0.25; `vignette` 0..1 = 0.3; `seed` 0..999 = 0, fixed |
| `crt` | CRT screen |  | `amount` 0..1 = 1; `curvature` 0..1 = 0.3; `scanlines` 0..1 = 0.5; `mask` 0..1 = 0.4; `vignette` 0..1 = 0.4; `glow` 0..1 = 0.3 |
| `dither_8bit` | 8-bit dither |  | `pixel` 0..5 = 0.5; `palette` [Black & white / Handheld green / 16 colours / 64 colours / 512 colours] = 16 colours, fixed; `dither` 0..1 = 1; `mix` 0..1 = 1 |
| `chromatic_aberration` | Chromatic aberration | yes | `amount` 0..5 = 0.6; `center` = (0.5, 0.5) |
| `rgb_split` | RGB split | yes | `distance` 0..10 = 1; `angle` -180..180 = 0 |
| `glitch` | Glitch |  | `amount` 0..1 = 0.5; `block` 1..20 = 4; `slices` 0..1 = 0.6; `rgb` 0..1 = 0.5; `rate` 1..30 = 8; `seed` 0..999 = 0, fixed |

**Time**

| id | name | GPU | parameters |
|---|---|---|---|
| `echo` | Echo |  | `echoes` 1..16 = 4, fixed; `spacing` 1..10 = 2, fixed; `decay` 0..1 = 0.6; `mode` [Behind / Light trails / Lighten] = Behind, fixed; `amount` 0..1 = 1 |
| `strobe` | Strobe |  | `rate` 0.5..30 = 4; `duty` 0..1 = 0.5; `mode` [Colour / Transparent / Invert] = Colour, fixed; `colour` = rgba(1, 1, 1, 1); `amount` 0..1 = 1 |
| `frame_blend` | Frame blend |  | `frames` 2..16 = 3, fixed; `amount` 0..1 = 1 |
| `posterise_time` | Posterise time |  | `fps` 1..60 = 8 |

**Keying**

| id | name | GPU | parameters |
|---|---|---|---|
| `chroma_key` | Chroma key | yes | `key` = rgba(0.05, 0.55, 0.1, 1); `tolerance` 0..1 = 0.35; `softness` 0..1 = 0.15; `spill` 0..1 = 0.6; `feather` 0..2 = 0.1; `choke` -2..2 = 0; `show_matte` = false, fixed; `amount` 0..1 = 1 |
| `luma_key` | Luma key | yes | `threshold` 0..1 = 0.1; `softness` 0..1 = 0.1; `mode` [Darks / Brights] = Darks, fixed; `show_matte` = false, fixed; `amount` 0..1 = 1 |
| `difference_key` | Difference key |  | `threshold` 0..1 = 0.08; `softness` 0..1 = 0.08; `feather` 0..2 = 0.15; `show_matte` = false, fixed; `amount` 0..1 = 1 |

**Generators**

| id | name | GPU | parameters |
|---|---|---|---|
| `solid` | Solid colour | yes | `colour` = rgba(0.08, 0.09, 0.12, 1); `mix` 0..1 = 1 |
| `gradient` | Gradient | yes | `kind` [Linear / Radial / Angle] = Linear, fixed; `colour_a` = rgba(0.85, 0.32, 0.18, 1); `colour_b` = rgba(0.06, 0.08, 0.35, 1); `start` = (0.5, 0); `end` = (0.5, 1); `perceptual` = true, fixed; `mix` 0..1 = 1 |
| `noise` | Noise |  | `kind` [Clouds / Turbulence / Static] = Clouds, fixed; `scale` 1..200 = 30; `octaves` 1..8 = 5, fixed; `speed` 0..5 = 0.2; `contrast` 0..4 = 1; `colour_a` = rgba(0, 0, 0, 1); `colour_b` = rgba(1, 1, 1, 1); `seed` 0..999 = 0, fixed; `mix` 0..1 = 1 |
| `grid` | Grid | yes | `cell` 1..50 = 10; `width` 0.02..5 = 0.15; `line` = rgba(0.8, 0.8, 0.8, 1); `background` = rgba(0, 0, 0, 0); `mix` 0..1 = 1 |
| `checkerboard` | Checkerboard | yes | `size` 1..50 = 6; `colour_a` = rgba(0.8, 0.8, 0.8, 1); `colour_b` = rgba(0.25, 0.25, 0.25, 1); `angle` -180..180 = 0; `mix` 0..1 = 1 |
| `countdown` | Countdown |  | `start` 1..99 = 5, fixed; `seconds` 0.25..5 = 1, fixed; `ink` = rgba(0.95, 0.95, 0.95, 1); `accent` = rgba(0.9, 0.42, 0.12, 1); `background` = rgba(0.05, 0.05, 0.06, 1); `mix` 0..1 = 1 |
| `bars` | Bars and tone |  | `level` [75 % / 100 %] = 75 %, fixed; `tone_hz` 100..10000 = 1000, fixed; `tone_db` -60..0 = -20, fixed; `mix` 0..1 = 1 |

### Transitions

| id | name | GPU | parameters |
|---|---|---|---|
| `dissolve` | Dissolve | yes | `curve` [Linear / Smooth] = Smooth, fixed |
| `dip` | Dip to colour | yes | `colour` = rgba(0, 0, 0, 1); `hold` 0..0.8 = 0.1 |
| `wipe` | Wipe | yes | `angle` -180..180 = 0; `softness` 0..0.5 = 0.05; `easing` = Sine in-out, fixed |
| `clock_wipe` | Clock wipe | yes | `start` -180..180 = 0; `direction` [Clockwise / Anticlockwise] = Clockwise, fixed; `softness` 0..45 = 3; `center` = (0.5, 0.5); `easing` = Sine in-out, fixed |
| `iris` | Iris |  | `shape` [Circle / Rectangle / Diamond / Star] = Circle, fixed; `mode` [Open / Close] = Open, fixed; `softness` 0..0.3 = 0.02; `center` = (0.5, 0.5); `easing` = Cubic in-out, fixed |
| `slide` | Slide | yes | `direction` [Left / Right / Up / Down] = Left, fixed; `motion_blur` 0..1 = 0.5; `easing` = Cubic in-out, fixed |
| `push` | Push | yes | `direction` [Left / Right / Up / Down] = Left, fixed; `motion_blur` 0..1 = 0.5; `easing` = Cubic in-out, fixed |
| `cover` | Cover | yes | `direction` [Left / Right / Up / Down] = Left, fixed; `shadow` 0..1 = 0.4; `motion_blur` 0..1 = 0.5; `easing` = Cubic in-out, fixed |
| `reveal` | Reveal | yes | `direction` [Left / Right / Up / Down] = Left, fixed; `shadow` 0..1 = 0.4; `motion_blur` 0..1 = 0.5; `easing` = Cubic in-out, fixed |
| `zoom_through` | Zoom through |  | `amount` 1..8 = 3; `blur` 0..1 = 0.6; `easing` = Cubic in-out, fixed |
| `spin` | Spin |  | `turns` 0.25..4 = 1; `direction` [Clockwise / Anticlockwise] = Clockwise, fixed; `zoom` 0..1 = 0.4; `blur` 0..1 = 0.5; `easing` = Cubic in-out, fixed |
| `flip` | Flip |  | `axis` [Horizontal / Vertical] = Horizontal, fixed; `perspective` 0..1 = 0.6; `background` = rgba(0, 0, 0, 1); `easing` = Cubic in-out, fixed |
| `cube` | Cube |  | `direction` [Left / Right / Up / Down] = Left, fixed; `background` = rgba(0, 0, 0, 1); `easing` = Cubic in-out, fixed |
| `page_curl` | Page curl |  | `angle` -180..180 = 30; `radius` 0.01..0.3 = 0.08; `shadow` 0..1 = 0.5; `back` 0..1 = 0.7; `easing` = Sine in-out, fixed |
| `blur_dissolve` | Blur dissolve |  | `blur` 0..20 = 3 |
| `whip_pan` | Whip pan |  | `direction` [Left / Right / Up / Down] = Left, fixed; `blur` 0..30 = 12 |
| `glitch_cut` | Glitch cut |  | `intensity` 0..1 = 0.7 |
| `light_flash` | Light flash |  | `colour` = rgba(1, 0.94, 0.85, 1); `intensity` 0..8 = 1.5; `width` 0.05..1 = 0.4 |
| `luma_fade` | Luma fade | yes | `map` [Linear / Radial / Noise / Incoming brightness / Outgoing brightness] = Noise, fixed; `softness` 0..1 = 0.2; `angle` -180..180 = 0; `scale` 1..100 = 20; `invert` = false, fixed; `easing` = Linear, fixed |
| `shape_reveal` | Shape reveal |  | `shape` [Heart / Star / Hexagon / Circle / Triangle / Diamond] = Heart, fixed; `spin` -720..720 = 90; `softness` 0..0.3 = 0.01; `border` 0..5 = 0.6; `border_colour` = rgba(1, 1, 1, 1); `center` = (0.5, 0.5); `easing` = Cubic in-out, fixed |

### Motion presets

| id | name | kind | parameters |
|---|---|---|---|
| `fade` | Fade | InOut | `easing` = Linear, fixed |
| `rise` | Rise | InOut | `distance` 0..1 = 0.15; `fade` = true, fixed; `easing` = Quint out, fixed |
| `fall` | Fall | InOut | `distance` 0..1 = 0.15; `fade` = true, fixed; `easing` = Quint out, fixed |
| `drift_left` | Drift left | InOut | `distance` 0..1 = 0.15; `fade` = true, fixed; `easing` = Quint out, fixed |
| `drift_right` | Drift right | InOut | `distance` 0..1 = 0.15; `fade` = true, fixed; `easing` = Quint out, fixed |
| `zoom_in` | Zoom in | InOut | `start_scale` 0..1 = 0.5; `fade` = true, fixed; `easing` = Cubic out, fixed |
| `zoom_out` | Zoom out | InOut | `start_scale` 1..4 = 1.6; `fade` = true, fixed; `easing` = Cubic out, fixed |
| `pop` | Pop | InOut | `easing` = Back out, fixed |
| `spin_in` | Spin in | InOut | `angle` -720..720 = -180; `fade` = true, fixed; `easing` = Cubic out, fixed |
| `tumble` | Tumble | InOut | `distance` 0..1 = 0.15; `easing` = Bounce out, fixed |
| `swing` | Swing | InOut | `angle` -90..90 = 35; `easing` = Elastic out, fixed |
| `flip_x` | Flip X | InOut | `fade` = true, fixed; `easing` = Cubic out, fixed |
| `flip_y` | Flip Y | InOut | `fade` = true, fixed; `easing` = Cubic out, fixed |
| `squash` | Squash | InOut | `distance` 0..1 = 0.15; `easing` = Back out, fixed |
| `typewriter` | Typewriter | InOut | `steps` 2..64 = 16, fixed; `easing` = Linear, fixed |
| `wipe_reveal` | Wipe reveal | InOut | `direction` [Left to right / Right to left / Top down / Bottom up] = Left to right, fixed; `easing` = Cubic out, fixed |
| `blur_in` | Blur in | InOut | `blur` 0..10 = 3; `easing` = Cubic out, fixed |
| `float` | Float | Loop | `amount` 0..0.2 = 0.015; `period` 0.1..10 = 2 |
| `pulse` | Pulse | Loop | `amount` 0..0.5 = 0.06; `period` 0.1..10 = 0.8 |
| `sway` | Sway | Loop | `amount` 0..45 = 5; `period` 0.1..10 = 2 |
| `orbit` | Orbit | Loop | `amount` 0..0.2 = 0.02; `period` 0.1..10 = 2 |
| `jitter` | Jitter | Loop | `amount` 0..0.1 = 0.006; `period` 0.02..1 = 0.08; `seed` 0..999 = 0, fixed |
| `breathe` | Breathe | Loop | `amount` 0..0.3 = 0.03; `period` 0.1..10 = 4 |
| `wobble` | Wobble | Loop | `amount` 0..0.3 = 0.05; `period` 0.1..10 = 0.6 |
| `bounce` | Bounce | Loop | `amount` 0..0.3 = 0.04; `period` 0.1..10 = 0.7 |
| `spin` | Spin | Loop | `turns` -4..4 = 1; `period` 0.1..10 = 2 |
| `drift` | Wander | Loop | `amount` 0..0.2 = 0.02; `period` 0.1..10 = 2; `seed` 0..999 = 0, fixed |

### Speed presets

| id | name | keys (position: speed) |
|---|---|---|
| `constant` | Constant | 0: 1x, 1: 1x |
| `speed_up` | Speed up | 0: 0.4x, 1: 2.5x |
| `slow_down` | Slow down | 0: 2.5x, 1: 0.4x |
| `surge` | Surge | 0: 1x, 0.5: 3x, 1: 1x |
| `dramatic` | Dramatic | 0: 3x, 0.35: 0.2x, 0.65: 0.2x, 1: 3x |
| `spotlight` | Spotlight | 0: 1x, 0.3: 1x, 0.45: 0.25x, 0.75: 0.25x, 1: 1x |
| `rhythm` | Rhythm | 0: 2x, 0.25: 0.5x, 0.5: 2x, 0.75: 0.5x, 1: 2x |
| `rush_in` | Rush in | 0: 5x, 0.3: 1x, 1: 1x |
| `rush_out` | Rush out | 0: 1x, 0.7: 1x, 1: 5x |
