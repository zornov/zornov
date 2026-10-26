---
name: pictures-retouch
use: Use when a still must be painted, retouched, composited or drawn frame by frame.
topics: pictures, picture, paint, painting, photo, photos, retouch, retouching, photo-editing, raster, drawing, brushes, skin, frequency-separation, dodge-burn, composite, drawn-animation, frame-by-frame, rotoscoping, onion-skin, compositing, rotoscope
tools: picture, generate, enhance, cut_out, colour, frame, contact_sheet, check
---
**Order**
1. Pick the intent: remove something, change something (sky, colour, skin), composite pictures, paint, or draw frame by frame. All of it happens in `picture`, not by stacking timeline layers.
2. Open or create the picture (`picture {ops: [{op: "new"} | {op: "open"}]}`; a file is only read, the picture is a copy). Work non-destructively: selections with the AI, edits on adjustment layers, real brushes.
3. Select first: `select` with `what` subject, sky, background, people (hair stays soft) or object (`point`, `exclude`, `box`), `refine: true` for hair and fur, `to: "mask"` to keep it as a mask. Work through masks, not erasing.
4. Retouch in the order that protects the picture: remove distractions, fix exposure and colour on adjustment layers, then local work (dodge and burn, frequency separation for skin) with soft low-opacity brushes, then sharpening last.
5. Composite: match black level, colour, grain and edge softness of each piece to the base; add a shadow where it touches.
6. Look after every few ops (`look: true` returns how the picture looks now; the picture is the judge, not the op list). Put it on the timeline with the `to_timeline` op (`at`, `duration`); calling it again updates the clips that show it.
7. Frame-by-frame drawing: a flipbook (`flipbook`, `fps`, `onion` skin of the neighbours); draw key drawings first, then in-betweens. Rotoscoping draws over a frame of footage (`from_video` makes a picture from a frame).

**Checks**
- Edges at 100 percent; skin keeps texture (no plastic smoothing).
- Each piece's light direction agrees with the base.
- The exported picture matches the project's aspect and size before `to_timeline`.

**Mistakes**
- Painting on the flattened picture instead of a new layer (`{op: "layer", action: "new"}`): the change cannot be changed later.
- Heavy smoothing and over-sharpening from a light preview.
- Compositing pieces from different lighting without relighting.

## Reference: Pictures: painting and photo editing

`picture` paints and edits pictures: the same pictures the Paint
workspace (⇧⌘P) shows, so the person sees what you do and you see what
they did. A call is `{"ops": [...], "look": true}`: the ops run on the
active picture as one step (all or nothing), the picture is saved in the
project's `Pictures` folder (`<name>.pcraft`, lossless: every layer,
mask, adjustment layer and smart object; `<name>.png` beside it), and
`look` (true or a size in px) returns how it looks now. Look after every
few ops; the picture is the judge, not the op list. `{"list": true}`
lists the open and saved pictures; `"picture": name` works on another.

Pictures are 8, 16 or 32-bit (`depth`), RGB, grey, CMYK or Lab, colour
managed, tiled: very large documents are fine. Coordinates are picture
pixels from the top-left; opacity, flow, hardness and strength are
0–100; colours are `#rrggbb`.

**Starting**: `{"op": "new", "name": "Poster", "width": 3000, "height":
2000, "background": "white" | "transparent" | "#rrggbb", "depth": 16}`;
`{"op": "open", "path": …}` reads PSD/PSB (with layers, masks,
adjustments, text, smart objects), TIFF, PNG, JPEG, WebP, HEIC, EXR,
camera raw (DNG, CR2/CR3, NEF, ARW, RAF, ORF, RW2…, developed), GIF,
OpenRaster, Krita and more: the file is only read, the picture is a copy
in the project. `{"op": "from_video", "media": "beach.mov", "at": 3.2}`
(or `"program": true` for the edit itself) makes a picture of a frame.
`place` puts a file in as a smart object that keeps its source.

**Selecting**: `{"op": "select", "what": …}` with `subject` (every main
object), `sky`, `background` (segmentation; `model: true` uses the local
depth and object models instead), `people` (portrait matting: hair stays
soft), or `object` with `point` [x, y], `points` / `exclude` (clicks on
and off it) or `box` [x0, y0, x1, y1] (the object model, SAM 2.1); also `rect` / `ellipse` (`rect` [x, y,
w, h]), `lasso` (`points` or SVG `path`), `wand` (`point`,
`tolerance`), `colour_range`, `all`, `none`, `invert`. `mode`: replace,
add, subtract, intersect. `feather` px; `refine: true` (or
`select.refineEdge` settings) for hair and fur; `to: "mask"` turns the
selection into the active layer's mask. Work through masks, not
erasing: a mask can be changed later.

**Painting**: `{"op": "paint", "tool": "brush", "feel": "watercolour",
"path": "M 100 400 C 300 200 500 600 700 400", "size": 40, "color":
"#c0392b", "opacity": 80, "taper": 0.3}`. The stroke is `points` ([[x,
y, pressure], …]), `path` (SVG path data: M L H V C S Q T A Z) or
`shape` ({"kind": "line" | "circle" | "ellipse" | "rect" | "polygon" |
"star" | "arc" | "spiral" | "wave", …}); `taper` thins the ends and
`pressure` [start, end] ramps it, as a pen does. Tools: brush, pixel
(hard aliased pencil), eraser, mixer (wet paint that picks up and mixes
colour: `wet`, `load`, `mix`), smudge (`strength`), blur, sharpen,
dodge / burn (`range`: shadows, midtones, highlights; `exposure`),
sponge, clone (`source` [x, y]), heal (`source`), spot_heal, remove,
history, pattern, replace_colour. Feels (brush presets): pencil,
hard_pencil, ink, calligraphy, airbrush, spray, watercolour, wash,
ink_wash, oil, gouache, acrylic, dry_brush, chalk, charcoal, pastel,
crayon, marker, soft, hard, spatter, stipple, grass, foliage, sparkle,
texture, rake; or any `preset` by name, or a full `brush` ({…}:
spacing, scattering, texture, dual brush, shape and colour dynamics).
`symmetry`: vertical, horizontal, dual, diagonal (or {mode, center,
rotation}); false turns it off. Paint on a new layer (`{"op": "layer",
"action": "new", "name": "Scarf"}`) so it can be changed.
`brush.presets.importAbr` imports brush files.

**Retouching**: `{"op": "remove", "what": "people"}` (or `point` /
`box` on the thing, or the current selection) takes it out and fills
the hole from its surroundings; `generative_fill` (`prompt`) paints the
selection from a description with the generative models instead, as a
masked layer (for new content: "a wooden bench", "more sky").
`spot_heal` strokes for blemishes and specks, `clone` / `heal` with a
`source` for repeating texture, `frequency_separation` (`radius`) for
skin, dodge and burn on a 50% grey Overlay layer for shape and light.

**Adjusting**: `{"op": "adjust", "kind": "curves", "points": [[0, 0],
[64, 50], [192, 210], [255, 255]]}` adds an adjustment layer (it stays
editable; `layer: false` changes the pixels instead). Kinds: levels,
curves, exposure, vibrance, hue_saturation, colour_balance,
brightness_contrast, black_white, photo_filter, channel_mixer,
selective_colour, gradient_map, colour_lookup, posterize, threshold,
invert, and on pixels shadows_highlights, hdr_toning, match_colour,
replace_colour, camera_raw (white balance, exposure, contrast,
highlights, shadows, whites, blacks, texture, clarity, dehaze,
vibrance, saturation, tone curve, HSL, colour grading, sharpening,
noise reduction, lens). Select first to adjust part of the picture: the
adjustment layer takes the selection as its mask.

**Layers**: `{"op": "layer", "action": …}`: new, duplicate, delete,
group, select (`layer`: id or name), set (`name`, `opacity`, `fill`,
`blend`, `visible`, `locks`), mask (`from`: selection, all, none,
transparency, apply, delete), clip (to the layer below), smart (into a
smart object), merge, front, back, up, down. Blend modes are the usual
27 (Multiply, Screen, Overlay, SoftLight, Color, Luminosity…).

**Everything else** is an engine command: `{"commands": "warp"}` finds
them with their parameters, and `{"op": "<id>", …params}` runs one.
Among them: `filter.liquify` (forward warp, pucker, bloat… strokes),
`edit.puppetWarp` (pins), `edit.perspectiveWarp`, `edit.transform`,
`edit.contentAwareScale`, `image.crop` / `image.imageSize` /
`image.canvasSize`, `file.automate.mergeToHdrPro` (bracketed
exposures), `file.automate.photomerge` (panoramas),
`edit.autoAlignLayers` then `edit.autoBlendLayers` with `method:
"stack"` (focus stacking), `layer.smartObjects.stackMode.median`
(people out of a burst), every blur, sharpen, noise, distort, render
and stylize filter, layer styles (`layer.layerStyle.dropShadow` …),
type (`type.create`), shapes and paths, channels, `history.newSnapshot`
/ `history.selectState`.

**Animation, frame by frame**: `{"op": "animate", "drawings": 8,
"fps": 12, "hold": 2}` makes the picture a flipbook (a `Frames` group
of drawings, one shown at a time; layers outside it show under every
drawing). `{"op": "frame", "go": 3}`, `"action"`: next, prev, add
(blank), copy (the current drawing, to trace and change), delete, hold
(`hold`: frames it stays, 1 = on ones, 2 = on twos; `all: true`). Paint
ops go to the current drawing. `{"op": "flipbook", "fps": 24, "onion":
{"before": 2, "after": 1, "opacity": 30}, "loop": true}`; `look` shows
the onion skin (earlier drawings red, later green). **Rotoscoping**:
`{"op": "trace", "media": "dancer.mov", "at": 2.0, "frames": 24,
"fps": 12}` puts the footage's frames under the drawings (a locked
`Reference` group, never part of the result).

**Onto the timeline**: `{"op": "to_timeline", "at": 4.0, "duration":
5}` places the picture (an animation lays its drawings in a row, timed
by their holds, merged into one clip; `loops`). After more painting,
`to_timeline` again updates the clips that show it (one undo step);
`again: true` places another copy. `export` writes PSD (layers kept),
TIFF, PNG, JPEG, WebP, EXR (`quality` for JPEG and WebP).

**Whole jobs from a brief** are a few calls: look, select, change on
layers, look again, refine. "Remove the people": `remove` with `what:
people`, look, `spot_heal` what remains. "Make it dusk": select the sky,
a gradient map or curves on it (deep blue to orange), a warm photo
filter and lowered exposure on the rest, a soft orange glow layer (Soft
Light) near the horizon, look. "Paint this character a scarf": select
the character (object at a point on them), a new layer clipped to
nothing, paint the scarf's shape with `feel: gouache` along a path,
shade with a darker colour on a Multiply layer, add highlights, look.

## Reference: Painting, photo retouching and drawn animation

**Work non-destructively.** Keep the photograph's own layer untouched:
retouch on a new layer above it (heal and clone with "sample all
layers"), adjust with adjustment layers, hide with masks instead of
erasing, and turn a placed picture into a smart object before scaling
or filtering it so the source stays sharp and the filter stays
editable. Name layers for what they do ("Sky darker", "Skin even").

**Selections decide quality.** A hard selection edge reads as a cut-out
pasted on. Feather 1–3 px for objects, refine the edge (smart radius)
for hair, fur and leaves, and decontaminate colours when a subject goes
onto a new background. Check a mask by looking at it over black and
over white.

**Removing things.** Select a little past the object (`expand` 4–10 px)
so no outline stays, fill from the surroundings, then look: repeated
patterns, smeared lines and doubled edges are the tells. Fix lines and
edges first with the clone stamp along the line, then texture with the
healing brush. Shadows and reflections of what was removed must go too.
New content (more sky, a missing object) is generative fill's job; check
it for light direction, grain and perspective, and blend its edge.

**Skin.** Frequency separation: even tone on the low layer (soft brush
or a light blur in a selection), blemishes on the high layer with the
healing brush. Keep pores: if the skin looks plastic, lower the low
layer's opacity. Dodge and burn on a 50% grey Overlay layer at low
exposure (5–10%), following the light already in the face; it is shape,
not make-up.

**Light and colour.** Grade from global to local: white balance and
exposure (camera raw) first, then contrast with curves, then local
light (dodge / burn, masked curves), colour last. A believable
time-of-day change moves everything together: sky colour, the colour of
the light on surfaces, the shadows' colour (blue at dusk, warm at golden
hour), contrast (lower and softer as the light goes) and practical
lights coming on. Vibrance before saturation; skin breaks first.

**Compositing.** Match the five things that sell a composite: light
direction, colour of light and shadow, contrast and black level, grain
and sharpness, perspective and scale (eye line on the horizon). Add a
contact shadow under anything that stands on a surface. Clip
adjustments to the one layer they belong to.

**Painting.** Block in big shapes and values with a large brush first,
then refine; squint to judge values. Shade on a Multiply layer and light
on a Screen or Color Dodge layer above the base colours, clipped to
them. Vary edges: sharp where the eye should go, soft and lost
elsewhere. Pressure and taper make lines alive; a constant-width line
reads as mechanical. Wet media (watercolour, oil with the mixer brush)
need the colour to pick up and mix: lower `load`, raise `wet`.

**Drawn animation.** Key poses first (the extremes of the action), then
breakdowns that show the arc, then in-betweens. On twos (each drawing
held two frames at 24 fps, or 12 fps on ones) is the classic hand-drawn
rate; fast actions go on ones. Squash and stretch keep the volume; the
onion skin shows spacing: closer drawings are slower. Hold a pose for
at least 6–8 frames so it reads. Rotoscoping: trace the essential lines
of the footage, not every wrinkle; keep line weight steady between
drawings or the line boils (a little boil is a style; a lot is noise).

**Print and screen.** Work in 16-bit for heavy grading (no banding),
32-bit for HDR merges; export PNG or 16-bit TIFF for further work, PSD
to keep layers, JPEG at quality 85–92 for the web.

## Reference: tool `picture`

Paint and edit pictures (raster painting and photo editing, the Paint workspace's pictures): `ops` run on the active picture as one step (all or nothing), it is saved in the project's Pictures folder, and `look` returns how it looks. Studio ops: new, open (a file is only read; the picture is a copy), picture (switch), save, export (psd/tif/png/jpg/webp/exr), place (a file as a smart object), select (what: subject, sky, background by segmentation; people by matting; object at point/points/exclude/box with the object model; rect, ellipse, lasso, wand, all, none, invert; mode, feather, refine for hair, to: mask), paint (tool: brush, pixel, eraser, mixer, smudge, blur, sharpen, dodge, burn, sponge, clone, heal, spot_heal, remove, history; feel: pencil, ink, airbrush, watercolour, oil, gouache, chalk, charcoal, pastel, crayon…; the stroke as points, path (SVG path data) or shape; size, color, opacity/flow/hardness 0..100, taper, symmetry), fill, gradient, remove (take out what/point/box or the selection and fill from the surroundings), generative_fill (paint the selection from a prompt), adjust (kind: levels, curves, hue_saturation, vibrance, selective_colour, colour_balance, exposure, camera_raw, shadows_highlights…; an adjustment layer unless layer: false), layer (new, duplicate, delete, group, select, set, mask, clip, smart, merge), frequency_separation, animate / frame / flipbook (frame-by-frame animation: drawings, holds, onion skin), trace (rotoscope: footage frames under the drawings), from_video (a frame as a new picture), to_timeline (the picture or the animation onto the timeline; again after more painting updates it), undo, redo, snapshot; and any engine command by id with its parameters (`commands` searches them: liquify, puppet warp, perspective warp, HDR merge, panorama, auto-blend for focus stacks, filters, content-aware fill and scale, camera raw…). Coordinates are picture pixels from the top-left.
