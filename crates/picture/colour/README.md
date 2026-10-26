# studio-colour

Colour science for Studio: transfer functions, colour spaces, grading
tools, LUTs, automatic corrections and scopes. No runtime dependencies
(`naga` is a dev-dependency used only to validate the WGSL in tests).

## Image type

`Image` is linear-light, premultiplied RGBA `f32`, in the working
primaries (Rec. 709 by default; set `Grade::working`). 1.0 is diffuse
white and HDR values above 1 are kept. `render` converts its frame
buffers to and from this type.

## Using it

```rust
use studio_colour::{Grade, FrameInfo, Image};

let mut grade = Grade::default();          // exact identity
grade.basic.exposure = 0.5;                // stops
grade.basic.temperature = 4800.0;          // light was warmer → cooler picture
grade.wheels.gain.luma = 0.1;
grade.curves.master = vec![[0.0, 0.0], [0.5, 0.55], [1.0, 1.0]];
grade.apply(&mut image, FrameInfo { frame: 42 });

// Per-pixel only (no spatial tools), e.g. to bake a LUT:
let p = grade.prepare();
let out = p.apply_rgb([0.18, 0.18, 0.18]);
```

## Modules

| module      | contents |
|-------------|----------|
| `transfer`  | sRGB, BT.709 OETF, BT.1886, PQ (ST 2084), HLG (BT.2100) incl. OOTF, ACEScct, ACEScc |
| `primaries` | BT.709/601/2020, P3-D65, AP0/AP1; RGB↔XYZ; Bradford; Planckian locus, CCT/Duv; white-balance matrix |
| `ycbcr`     | BT.601/709/2020 Y′CbCr, limited/full range quantisation |
| `space`     | grading space (AP1 + ACEScct, normalised so 0 = black, 1 = diffuse white); hue plane |
| `basic`     | exposure, white balance, contrast, highlights/shadows/whites/blacks, fade, saturation, vibrance; parameter blocks for sharpen/vignette/grain |
| `spatial`   | clarity, dehaze (dark channel), sharpen, vignette, grain, blur helpers |
| `cdl`       | ASC CDL v1.2 (clamped and unclamped forms) |
| `wheels`    | lift/gamma/gain/offset → CDL; shadows/midtones/highlights masks; intensity |
| `spline`, `curves` | monotone, shape-preserving and periodic Hermite splines; master/RGB and hue/lum/sat curves |
| `hsl`       | 8 overlapping hue bands |
| `lut`       | `.cube` read/write (1D + 3D), tetrahedral interpolation, LUT input spaces, skin protection |
| `auto`      | auto white balance (grey-world + white-patch), auto levels, match colour (Oklab mean/std transfer) |
| `scopes`    | downscale, luma waveform, RGB parade, vectorscope (+75 % targets, skin line), histogram |
| `grade`     | `Grade` (all parameters), the fixed processing order, `GradeProcessor` |
| `gpu`       | WGSL sources (`shaders/`), uniform packing, LUT texels, scope params |
| `looks`     | Studio's own looks as grade parameters |

Every formula and parameter mapping that is our own design (tone-range
weights, fade, vibrance, wheel → CDL mapping, tonal masks, HSL weights,
skin mask, vignette, grain, dehaze details) is documented in the module
docs next to the code.

## Processing order

Un-premultiply → dehaze → levels → match colour → white balance and
exposure → grading space → contrast / tone ranges / fade → wheels →
curves → HSL → saturation / vibrance → linear → LUT → clarity → sharpen →
vignette → grain → premultiply. Stages at their defaults are skipped,
so the default grade is a bit-exact identity.

## GPU

`gpu::grade_shader_source()` is a compute shader (`cs_grade`) for the whole
per-pixel path plus vignette and grain; `gpu::pack_grade` fills its
uniform. `gpu::scopes_shader_source()` has `cs_downscale` and `cs_scopes`
(atomic counters). Bindings and buffer layouts are documented in
`src/gpu.rs` and at the top of each `.wgsl` file. Dehaze, clarity and
sharpen are CPU-only for now. The shaders are validated with naga in
tests but have not been executed on a GPU in this crate's tests.

## Looks

`looks/*.cube` are 33³ LUTs (BT.709 primaries, BT.1886-encoded in and
out) baked from the parameters in `src/looks.rs`. Regenerate with:

```
cargo run -p studio-colour --example gen_looks
```

A test fails if the shipped files no longer match their parameters.

## Tests

`cargo test -p studio-colour` covers: bit-exact identity at defaults;
ASC CDL against values evaluated from the v1.2 equations; tetrahedral LUT
reference points (e.g. a lattice of r·g interpolates to exactly
min(r, g)); monotone curves never inverting (randomised); transfer
function reference points and round trips; published BT.709 and
AP0→AP1 matrices; CCT/Duv round trips; tone-range monotonicity for
every control combination; scopes, auto tools and WGSL validation.
