---
name: vector-drawing
use: Use when the work is drawn: logos, icons, illustration, lettering, shape morphs.
topics: vector, vector-design, illustration, illustrate, svg, pdf, eps, pathfinder, trace, logo, logos, logo-design, icons, icon, infographics, infographic, artboard, artboards, wordmark, lettering, palettes, morph, morphing, shape-morph, tween, shape-tween
tools: vector, morph, shape, text, card, generate, animate, frame, check
---
**Order**
1. Decide the idea in a sentence and the constraints: where it will be seen, how small, one colour or full colour, light or dark background.
2. Open or create a document with `vector {doc, ops}` (a `.svart` name in the project's Vector folder; an .svg, .pdf, .eps or image opens as a new one). Ops run in order: shapes (rect, ellipse, polygon, star, ...), paths (pen, curve), type (`text`, `area_text`, `path_text`), `pathfinder` (unite, minus_front...) to combine, `repeat` for live constructions.
3. Build from simple geometry on a grid: big shapes first, then combine, then details. Colour with a few swatches (`swatch`, `harmony`); one accent over neutrals.
4. Lay out with `align` and `distribute`; text stays live type until the final outline step.
5. Tracing a picture (`trace`) gives a start, not a finish; clean the paths.
6. Look: render at the size it will really be used, and at a very small size for icons and logos. Export (`export` to .svg, .pdf, .eps, png) or `place` it on the timeline with `place: {at, duration}`.
7. Morphing one outline into another: the `morph` op (`clip`, `to`, `at`, `duration` about 1 s, `ease`), which matches the outlines point for point. Never fake it with scale or opacity. Keep both shapes about the same size and place; 0.6-1.2 s with ease_in_out reads as one motion.

**Checks**
- A logo works in one colour, on light and dark, and at 16 px.
- Strokes have one weight system; corners one roundness; equal gaps are equal.
- Paths closed before filling; no stray points.

**Mistakes**
- Many colours and gradients where one accent would do.
- Outlined text before the wording is final.
- A "morph" faked with a cross-fade or a scale: use the `morph` op, and say so if you could not.

## Reference: Vector illustration

The `vector` tool makes and edits vector art: logos, icons, illustrations,
infographics, lettering, patterns. A document (`.svart`, in the project's
`Vector` folder) has artboards, layers, groups and art; every object has
an id (`summary` lists them with their boxes and paints). Units are
document points; an artboard the size of the canvas maps one point to one
canvas pixel. The art is drawn exactly (true area coverage, curves kept as
curves), so it is sharp at any size: export it at 4× or 8×, place it on
the timeline and it draws at the output's own resolution.

Call: `vector {"doc": "logo", "new": {"width": 1000, "height": 1000}, "ops":
[…], "look": true}`. Ops run in order and the document is saved after
each call; `look` returns a picture of the artboard. `{"op": "help"}`
lists every op with its fields.

**Paint** is a colour (`"#e8573f"`, `"tomato"`), a gradient
(`{"linear": {"from": [x, y], "to": [x, y], "stops": ["#ff512f",
"#f9d423"]}}`, `"radial"` with `center`, `radius`, optional `focal`;
`"conic"` with `center`, `angle`; stops may be `[offset, colour]`;
`"space": "oklab"` for even, bright blends), `{"freeform": [[x, y,
"#c"], …]}`, or `{"swatch": "brand"}` (a global swatch: change it once,
everything using it follows). The flat form works too:
`{"type": "radial", "center": [x, y], "radius": r, "stops": [[0,
"#fff"], [1, "#0000"]]}` (or `"type": "linear"` with `from`, `to`). The
full form, as `get` shows a gradient, is `{"type": "gradient", "kind":
"radial", "c0": {"x", "y"}, "r0": 0, "c1": {"x", "y"}, "r1": r,
"stops": [{"offset": 0, "color": "#fff"}, …]}`: c0 / r0 the focal
circle, c1 / r1 the outer one. Any op that makes art takes `fill`,
`stroke`, `stroke_width`, `join`, `cap`, `dashes`, `dash_offset`, `profile` (variable
width: taper, taper_both, bulge, pinch, brush), `opacity`, `blend`,
`effects`, `name`, `parent` (a layer or group id). `fills` / `strokes`
(lists) stack several in one object: a fill under two strokes makes an
outlined badge in one shape.

**Drawing**: `rect` (x, y, w, h, radius or radii, corners round /
inverted_round / chamfer), `ellipse`, `circle`, `polygon`, `star`
(inner radius optional: straight arms), `spiral`, `arc`, `line`, `grid`,
`polar_grid`, `pie`; `path` with SVG `d`, `points` (+ `closed`,
`smooth`) or `anchors` [{at, in, out}] (the pen); `curve` (smooth through
points, `corners` by index), `pencil` (freehand, fitted), `shaper` (a
rough drawing becomes a clean shape). Edit points with `anchor`,
`insert_anchor`, `delete_anchor`, `simplify`, `smooth`, `add_anchors`,
`reverse`, `join`.

**Type**: `text` (point type at x, y), `area_text` (rect, columns,
gutter; flows around objects that have `wrap`), `path_text` (along a
path or an object); `font`, `weight`, `italic`, `size`, `leading`,
`tracking` (thousandths of an em), `case` (upper, small_caps, title),
`features` (`["liga", "ss01", "onum", "-kern"]`), `align` (left, center,
right, justify), `runs` for mixed styles. `char_style`, `para_style`,
`type_style`; `fonts`, `glyphs`, `font_features` to choose.
`outline_text` turns type into paths (for logos: always outline the
final wordmark).

**Combining**: `pathfinder` with `mode` unite, minus_front, minus_back,
intersect, exclude, divide, trim, merge, crop, outline; `shape_builder`
(merge or delete the regions under points), `live_paint` (fill the faces
overlapping outlines make), `compound` (holes), `offset_path`,
`outline_stroke`, `knife`, `eraser`, `scissors`, `blob` (blob brush),
`expand` (live shapes, type, effects, repeats, blends and envelopes into
plain paths).

**Live constructions** stay editable: `repeat` (radial: count, radius,
center; grid: rows, cols, gap; mirror: at, angle), `blend` (two shapes,
steps, optional spine), `envelope` (warp arc, arc_lower, arc_upper,
arch, bulge, shell_lower, shell_upper, flag, wave, fish, rise, fisheye,
inflate, squeeze, twist with bend −1..1; or mesh, free corners, shape),
`perspective_grid` + `perspective` (art on the left, right or floor
plane), `symbol` (art becomes reusable) + `instance`. Effects:
drop_shadow, inner_shadow, outer_glow, inner_glow, blur (raster, scaled
with the art), offset, roughen, zig_zag, pucker_bloat, twist,
round_corners, transform copies, warp (on the outline).

**Layout**: `align` (how: left, center, right, top, middle, bottom; to:
"artboard" or an id), `distribute` (axis, spacing), `group`, `ungroup`,
`clip` (the frontmost shape clips the rest), `mask` (opacity mask from
art), `arrange`, `transform` (translate, rotate, scale, skew, flip_x,
about each object's centre), `place` (x, y, anchor), `resize`, `guide`,
`grid_settings`, `snap`. `chart` makes infographic charts (bar, column,
line, pie, donut) from `data` [[label, value], …] in a `rect`, with
`colors`, `labels`, `title`.

**Colour**: `swatch` (global), `harmony` (base, rule complementary,
split_complementary, analogous, triadic, tetradic, square,
monochromatic, shades, tints; `swatches: true` adds them), `recolor`
(map old to new), `contrast`.

**Images**: `image` (file, x, y, w, h), `trace` (an image object or file
to vector shapes; preset: 3 colors, 6 colors, 16 colors, black and white
logo, line art, sketched art, silhouettes, technical drawing, high
fidelity photo, low fidelity photo, flat logo, shades of gray).

**Files**: `export` to .svg, .pdf (one page per artboard, vector), .eps,
.png (`scale`: 4 makes an 8K picture of a 1080p artboard), .svart;
`import` places another file's art; `doc` can open .svg, .pdf, .eps.

**On the timeline**: `place: {"at": 2, "duration": 5, "x": 0, "y": 0}`
puts the artboard on the timeline as a clip (centred on the canvas, one
point per canvas pixel; `fit`: contain or cover). Animate any property
with `key` (id, property as a JSON pointer such as `/opacity`,
`/appearance/items/0/paint`, `/shape/radius`, `/path`, or `@move`
[dx, dy], `@rotate` degrees, `@scale`; t in seconds from the clip's
start; ease linear, ease_in, ease_out, ease_in_out, hold, spring or
[x1, y1, x2, y2]); paths morph between keys. `bind` ties a property to a
clip parameter so the timeline's keyframes drive it. The clip redraws
when the document changes.

**How to design with it**
1. Start the document at the size it is for (1000 × 1000 for a logo or
   icon, the canvas for video). Set swatches first (a `harmony`, then
   global swatches), so colour stays consistent and is easy to change.
2. Build from few, clean shapes: live shapes and pathfinder beat many
   loose paths; group by meaning and name the groups.
3. `look` after each step and judge it like a designer: balance, weight,
   spacing, contrast. Fix what you see, then look again. `check` reports
   art off the artboard, small or low-contrast type and hairlines.
4. For logos and icons: outline the type, unite overlaps, look at it
   small (`look` with `width: 64`) and in one colour (`recolor` to
   black) before calling it done; export SVG and PDF.
5. For video: keep the art on its own layer, animate with `key` (enter
   with `@scale` and `/opacity` over 0.4–0.8 s with ease_out), then
   `place` it and check the frame in the edit.

## Reference: Morphing shapes

A morph really turns one outline into another: a circle becomes a
triangle, a logo becomes a word, an O becomes a disc. Do not fake it with
scale or opacity; use the `morph` op, which matches the two outlines point
for point:

* both are resampled to the same number of points, with corners (turns
  over 30°) kept as points so a triangle's tips land exactly;
* the start point and direction that move least are chosen, and opposite
  winding is reversed, so the shape never twists or turns inside out;
* several subpaths (letters, holes) are paired by place and size; one
  with no partner grows out of, or shrinks into, a point.

`edit` op `morph`: `clip`, `to`, `at` (timeline seconds, default the clip
start), `duration` (1), `ease` (any `animate` curve; ease_in_out).

* **Shape layer**: `to` is shape items (`{"type": "polygon", "points": 3,
  "size": 400}`), `{"clip": id}` (another shape layer's shapes),
  `{"text": "HELLO", "size": 220, "font", "weight"}` (type outlines) or
  SVG path data. The layer gets a `morph` item before its paint and a
  `generator.morph` parameter keyed 0 → 1; retime or re-ease it with
  `keys` on `generator.morph`, or drive it with an expression. Inside a
  shape document, `{"type": "morph", "to": [...], "amount": 0.5}` does the
  same by hand.
* **Vector art clip**: `object` (id or name; default the topmost) morphs
  into `to` (another object's id or name, path data, `{"text"}`, or a
  shape `{"op": "ellipse"…}`); the target is hidden unless
  `keep_target`; solid fills cross-fade too. In the vector tool the same
  op is `{"op": "morph", "id", "to", "t", "duration", "ease"}`.
* **Mask**: `mask` (1…) and `to` as `{"shape": "star", "position",
  "size", "rotation"}`, `{"mask": n}` or `{"points": [[x, y], …]}` (0..1
  of the layer). The mask becomes a path whose matched points are keyed.

Good morphs: keep both shapes about the same size and place, or move the
layer while it morphs; 0.6–1.2 s with ease_in_out reads as one motion;
morph type into type of similar length; let a strong shape hold for a
beat before and after.

## Reference: Vector design: logos, icons, illustration, infographics

**Logos.** A mark has to work at 16 pixels and on a billboard, in one
colour and in full colour. Build it from a few geometric shapes (circles,
rounded rectangles, arcs) united with `pathfinder`, not many hand-placed
points. Keep one idea: a monogram, a symbol, or a wordmark, rarely all
three. Optical balance beats arithmetic: round and pointed shapes reach
a little past the guide lines that flat shapes stop at (overshoot about
1–3% of the height), and a circle looks smaller than a square of the
same width. Space the letters of a wordmark by eye (`tracking`, then
kern pairs with runs), outline the type when it is final, and unite
overlaps so the file has no hidden pieces. Before calling it done: look
at it at 64 pixels wide, recolour it to black on white and white on
black, and check it reads; deliver SVG and PDF (and PNG at 1×, 2×, 4×).

**Icons.** Draw on a fixed grid (24 or 48 units) with one stroke weight
for the whole set (2 on 24 is common), the same corner radius and the
same cap and join everywhere. Keep at least the stroke width between
lines so small sizes don't fill in. Fill-style icons use the same
silhouette weight. Snap anchors to whole or half units so edges stay
crisp at 1×. A set is consistent before it is clever.

**Illustration.** Plan values before colours: a clear light, middle and
dark separate the subject from the ground. Choose a palette from a
`harmony` (three to five colours plus a near-black and a near-white),
save it as global swatches and use only those. Give the piece one shape
language (round and soft, or angular and sharp) and vary sizes (one
big, a few medium, many small). Use gradients for light, not as
decoration; `oklab` space keeps blends bright. Shadows and highlights
can be shapes clipped to the form (`clip`) in a multiply or screen
blend. Texture with `roughen` or a blurred `inner_shadow` sparingly.

**Infographics.** One message per chart. Sort bars, label directly
(not with a legend), start bars at zero, and keep gridlines faint or
gone. Category colours from one harmony; one accent colour for the
point being made, the rest muted. Type: one family, two or three sizes,
numbers in tabular figures (`features: ["tnum"]`). Leave generous
margins; align everything to a grid (`align`, `distribute`). Check
contrast (`contrast` of text on its background: at least 4.5:1).

**For video.** Art on the timeline is drawn at the output's resolution,
so build it at canvas size and never rasterise it first. Animate the
parts, not the whole: enter in a sequence (`key` `@scale` from 0.8 with
`/opacity` from 0 over 0.5 s, ease_out, each part 0.08 s after the
last), morph paths between keys of the same anchor count, and let lines
draw on with dash offset keys. Keep moving art away from the title-safe
edges.

## Reference: Shape morphing

A morph is one shape becoming another, and the eye judges it by whether
it stays one thing the whole way.

- **Match, then move.** The points of both shapes must correspond:
  the same count, the same direction, starting where the two are
  closest, corners pinned to corners. Badly matched points make the
  shape twist, fold through itself or swim; `morph` does the matching.
- **Similar size and place.** A morph between shapes of very different
  size reads as a scale move with a wobble. Line them up first, or
  move the layer with the morph so the change of shape is the event.
- **One motion.** 0.6–1.2 s with ease_in_out; hold the start shape and
  the end shape for a beat each. Faster than 0.3 s reads as a cut.
- **Count the parts.** A circle into a triangle is clean; a word into a
  logo pairs letters with pieces by place and size, and spare pieces
  grow from a point. Fewer, bolder parts morph best: a five-letter word
  into a five-letter word, a mark into its wordmark's initial.
- **Colour with shape.** Cross-fade the fill over the same time and
  ease, or keep one colour so only the outline changes.
- **Not a dissolve.** If the two shapes have nothing in common (a face
  into a skyline), a morph looks like a melt; a match cut or a wipe
  usually serves better.

## Reference: tool `vector`

Vector illustration: draw, edit and lay out vector art in a document (doc: a .svart name in the project's Vector folder; .svg, .pdf, .eps or an image open as a new one), with ops applied in order: shapes (rect, ellipse, circle, polygon, star, spiral, arc, line, grid), paths (path d / points / anchors: the pen; curve; pencil; shaper), type (text, area_text with columns and wrap, path_text; type_style, char_style, para_style, OpenType features, glyphs), paint (fill/stroke as colours, gradients linear/radial/conic/freeform, swatches; style, appearance with several fills and strokes, effects: drop/inner shadow, glows, blur, offset, roughen, zig_zag, pucker_bloat, twist, round_corners, warp), pathfinder (unite, minus_front, intersect, exclude, divide, trim, merge, crop, outline), shape_builder, live_paint, offset_path, outline_stroke, simplify, knife, eraser, scissors, blob, width_profile, expand, outline_text, trace (bitmap to vectors), repeat (radial, grid, mirror), blend, envelope (warps, mesh), perspective, symbol / instance, align, distribute, group, clip, mask, charts for infographics (chart: bar, column, line, pie, donut), harmony and recolor, animation keys on any property, export (svg, pdf, eps, png at any scale). {"op": "help"} lists every op and its fields. look: true returns a picture of the artboard to judge; place puts the art on the timeline as a clip (at, duration, x, y, fit, params) that draws it sharp at any resolution and plays its animation.
