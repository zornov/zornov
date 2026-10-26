# studio-vector-raster

Zornov's vector rasterizer: a `Scene` of fills, strokes and layers in user
space, drawn at any size by the CPU (`render`, `render_f32`) or the GPU
(`gpu::GpuRaster`, WGSL compute).

## How it draws

1. **Prepare** (`prepare.rs`, shared by both renderers). Paths are mapped to
   device pixels and flattened to line segments (0.01 px tolerance by
   default); curves that cannot reach the canvas are dropped, and curves
   wholly left of it become y-monotone chords (they still decide the
   winding to their right). Strokes become fills in the path's own space
   (`stroke.rs`: kurbo's stroker for even strokes, a union of oriented
   pieces for variable-width profiles). Paints are resolved to device
   space. Layers with effects, opacity masks, patterns and mesh gradients
   become **prepasses**: images drawn first and then used as paints.
2. **Coverage** is the exact area of each pixel inside the path. Each
   segment adds, for every row it crosses, its height in that row times
   the part of the pixel to its right, integrated exactly along the
   segment (`Φ(u) = ∫ clamp(u, 0, 1)`); a running sum along the row gives
   each pixel's signed winding area, which non-zero clamps and even-odd
   reflects. For simple polygons this equals a polygon-clipping oracle to
   2·10⁻⁵ (`tests/exact.rs`).
3. **CPU** (`cpu.rs`): rows in bands of 32, bands in parallel; each band
   composites every draw and isolated group in order into floating-point
   pixels. Paint evaluation in `shade.rs`, blend formulas in `blend.rs`
   (the W3C modes), effects in `filter.rs` (separable Gaussian, box
   passes for large deviations, dilation, shadows and glows).
4. **GPU** (`gpu/`): the CPU bins segments into 16×16 tiles with each
   tile's 16 row backdrops (`encode.rs`); one workgroup per tile runs the
   tile's commands per pixel (`fine.wgsl`: the same area integral, paints,
   blends and a small layer stack); effects run as compute passes
   (`filter.wgsl`). GPU and CPU agree to within one 8-bit level.

## Tests

* `tests/reference.rs`: SVG art (shapes, fill rules, strokes with joins,
  caps and dashes, gradients with spread modes, groups, all 16 blend
  modes, clip paths, masks, blur, patterns) drawn here and by resvg, at 1×
  and 3×, compared pixel by pixel.
* `tests/exact.rs`: exact coverage against polygon clipping, disc areas at
  scales from 0.5× to 64×, an 8K render's edges, gradient values, stroke
  areas, effect energy, clips and masks, freeform and mesh gradients.
* `tests/gpu.rs`: the GPU renderer against the CPU renderer on all of it.

## Benchmark

`cargo bench -p studio-vector-raster`: a 2,000-shape illustration on a
1920×1080 artboard at 1080p, 4K and 8K, CPU and GPU, with resvg's CPU
renderer for scale.

Measured on an Apple M4 Max shared with many other builds (load average
above 300, so absolute times are pessimistic; the ratios hold):

| | 1080p | 4K | 8K |
|---|---|---|---|
| CPU (this crate) | 44 ms | 81 ms | 210 ms |
| GPU (this crate, with readback) | 54 ms | 82 ms | 146 ms |
| resvg CPU | 93 ms | 193 ms | 464 ms |
