---
name: titles-typography
use: Use when words go on screen: titles, lower thirds, captions, kinetic type, charts.
topics: typography, fonts, font, type, titles, title, mograph, motion-graphics, motion-design, templates, template, lower-thirds, lower-third, kinetic, kinetic-type, charts, chart, infographics-motion, text-animators, animators, 3d-text, text-3d, expressions, expression, time-remap, shapes, shape, shape-layers, captions, infographics, logo
tools: text, text_3d, fonts, template, chart, shape, card, ai, animate, motion, stagger, expression, set, apply, transition, frame, contact_sheet, check, review, catalogue
---
**Order**
1. Say what the words are for and take them from the person: names, claims and numbers exactly as given, never invented. Captions come from speech: `ai {captions}` gives words with times.
2. Templates first for the standard pieces (lower thirds, titles, callouts, end cards, infographics): `template`. One lower-third design per video, in the same place every time. Charts from the person's data: `chart` (bar, line, pie, map).
3. Plain titles: `text` with a `style` from `catalogue {panel: "text"}`. Two styles at most: one display face for headlines, one plain face for the rest. Headline 2-3 times the body size; at least 40 px on 1080p for anything meant to be read, 60 or more for social; under seven words a line.
4. Motion: entrances 0.3-0.6 s with an ease out, exits 0.25-0.4 s with an ease in; stagger letters 0.03-0.06 s, words 0.08-0.15 s, in reading order; the box or line first, then the name, then the role. Kinetic type: one idea per moment, one emphasis.
5. Hold time: about 0.3 s per word fully on screen (at least 1.2 s for a sentence, 0.8 s for a one to three word label), counted from when the whole line has arrived. Pace comes from motion and cuts, never from taking text away early.
6. Safe placement: inside the title-safe area (7-10 percent in), never over a face or mouth (`canvas_layout` view `fit` finds free space, view `safe` gives the box), never under the platform buttons for vertical video; give text over busy picture a backing (`backing`) or a darkened area. Vertical titles stay between 8 and 78 percent of the height.
7. 3D letters and logos: `text_3d` with depth 20-35 percent of the letter height, a bevel 3-5 percent, a glossy or metallic face, one key light and a slow turn of 10-30 degrees.
8. Layers: build back to front (background, picture, boxes, text, accents); put things behind with `restack` or `behind`, never a guessed lane.

**Checks**
- `check` judges titles on the rendered picture: cut off, under platform buttons, hard to read, over faces, overlapping; also `too_many_fonts`, `palette_sprawl`.
- Look at the held frame, not only the entrance; use `detail: true` for small text.
- It reads with the sound off.

**Mistakes**
- All caps for a sentence, stretched or squashed type, mid-grey on mid-grey, red on blue.
- Easing in on an entrance, or exits that crawl.
- Five effects on one title: one effect on a moving thing.
- Narration that reads the screen back; say what the picture cannot.

## Reference: Motion graphics

Zornov does motion design with layers, keys and expressions, for you and in the app
(the viewer toolbar's shapes button: templates, shape presets, charts, 3D
text; the Inspector: template controls, shape items, text animators).

**Shape layers** (`shape` op): a list of items worked through in order,
like a recipe. Shapes add outlines; operators change the outlines made
so far; `fill` / `stroke` paint **every outline made so far in their
group** as it is at that point, over what was painted before. So to
colour two shapes differently, put each with its paint in its own
`group`: a group with paint of its own draws itself; one without hands
its outlines to the group around it. Coordinates are
canvas pixels from the clip's centre (y down); the clip's own `x`, `y`,
scale and rotation move the whole layer.

- Shapes: `rect` (`size`, `roundness`), `ellipse` / `circle` (`size` or
  `radius`), `polygon` (`points`, `radius`), `star` (`points`, `radius`,
  `inner`), `line` (`from`, `to`), `arc` and `pie` (`radius`, `inner` for
  a ring, `start`, `end`: degrees from 12 o'clock, clockwise), `path`
  (`points` + `closed` / `smooth`, or SVG `"d": "M0 0 C …"`, or
  `vertices` with `in` / `out` handles). Each takes `position`.
- Paint: `fill` (`colour`, `opacity`, `rule`: nonzero / evenodd,
  `blend`), `stroke` (`colour`, `width`, `cap`, `join`, `dash`: `[on,
  off]`, `dash_offset`). Either takes a `gradient`: `{"type": "linear",
  "from", "to", "stops": ["#f60", "#fc0"]}` or `{"type": "radial",
  "centre", "radius", "stops": [[0, "#fff"], [1, "#0000"]]}`.
- Operators: `merge` (`mode`: union, subtract (the first minus the
  rest), intersect, exclude), `trim` (`start`, `end` 0..1, `offset`
  turns; `mode`: together, or sequence = all paths as one line), `offset`
  (`amount` px, + grows, - shrinks; `join`), `round_corners` (`radius`),
  `wiggle` (`size` px, `detail` points per 100 px, `per_second`,
  `correlation` 0..1, `seed`), `zigzag` (`size`, `ridges`, `points`:
  corner / smooth), `pucker` (`amount` -1..1: pucker to bloat), `twist`
  (`angle`), `morph` (`to`: a shape or list of shapes, `amount` 0..1:
  key it to morph), `repeater` (`copies`, `offset`, `position`,
  `rotation`, `scale` per copy, `anchor`, `start_opacity`,
  `end_opacity`, `composite`: above / below), `transform`.
- `group`: `items`, `position`, `anchor`, `scale` (factor or [x, y]),
  `rotation`, `skew`, `skew_axis`, `opacity`, `blend`.
- **Every number, point and colour animates**: `{"keys": [[0, 0], [0.6,
  1, "out"]], "loop": "cycle"}` (seconds into the clip; eases linear,
  smooth, in, out, in_out, hold, overshoot, anticipate, bounce,
  elastic; loop cycle / pingpong), `{"from": 0, "to": 1, "start": 0.2,
  "duration": 0.5, "ease": "out"}`, `"=formula"` (the expression
  language below: `t`, `progress` 0..1 through the clip, `i` the axis,
  the clip's inputs by name), or `"$name"`: an **input**, a setting of
  the clip (`"inputs": {"grow": 0}` when made) that keys, drives and
  takes expressions like any property: `animate` `generator.grow` (or
  just `grow`).
- `"preset"`: ring (fills to input `progress`), check, cross, burst,
  badge, spinner, dots, underline, circle_draw, arrow, pulse, blob,
  frame, bars, wipe, confetti; with `colour`, `accent`, `size`, `width`
  (line px), `speed` (seconds).
- Change one: `{"op": "shape", "clip": 7, "patch": [{"path":
  "/items/1/colour", "value": "#ff3366"}, {"path": "/items/3",
  "remove": true}]}`, `"add": [items]`, `"items"` (replace),
  `"inputs"`. The reply says where it draws.
- `{"op": "shape", "lottie": "/path/icon.json", "at": 2}` brings a
  Lottie file's shape layers in as shape layers you can edit (`.json`
  or `.lottie`; what has no equivalent is listed).
- `"extrude": {"depth": 60, "bevel": 6, "colour", "side_colour",
  "metallic", "roughness"}` makes the closed outlines a lit 3D model
  instead (logos, badges).

    {"op": "shape", "at": 1, "duration": 3, "items": [
      {"type": "circle", "radius": 160}, {"type": "circle", "radius": 110}, {"type": "merge", "mode": "subtract"},
      {"type": "fill", "gradient": {"type": "linear", "from": [-160, 0], "to": [160, 0], "stops": ["#7c5cff", "#21c7a8"]}},
      {"type": "trim", "end": {"keys": [[0, 0], [0.8, 1, "out"]]}},
      {"type": "stroke", "colour": "white", "width": 6},
      {"type": "repeater", "copies": 3, "position": [0, 0], "scale": [1.25, 1.25], "end_opacity": 0}]}

**Text animators** (on `text` and `set`): `"animator"` adds one,
`"animators": [...]` replaces them (`[]` none). A preset by name
(highlight_sweep, wave_jump, focus_in, tracking_in, jitter, float,
drop_out; with `unit`, `start`, `duration`) or your own: `{"unit":
"letter", "range": {"start": 0, "end": 0.3, "offset": {"keys": [[0,
-0.3], [1.5, 1]]}, "shape": "smooth"}, "position": [0, -40], "scale":
1.3, "colour": "#ffd166", "blur": 12, "opacity": 0, "rotation": 20,
"tracking": 30}`. The values are where a fully picked letter, word or
line ends up; the **range** picks a stretch (fractions of the text,
each a number or keys; `shape` square, ramp_up, ramp_down, triangle,
round, smooth; `order` forward, backward, centre, random; `amount`), a
**wiggle** (`{"per_second", "correlation", "min", "max", "mode":
"intersect" | "add"}`) makes the pick wander. They add to `reveal`,
`exit` and `loop`. Blur works on one unit kind per title.

**Rich text, paths, kerning** (on `text` and `set`): `"runs": [{"text":
"Big ", "weight": 900, "colour": "#ffcc00"}, {"text": "news", "italic":
true, "size": 0.6, "baseline_shift": 18, "tracking": 40}]` styles parts
of one title (bold, italic, underline, strikethrough, colour, size × the
title's, weight 100–900 (variable fonts use any value), tracking %,
baseline_shift px up, font). `"path": {"circle": 260}` (or `{"arc": r,
"degrees"}`, `{"wave": height, "width", "waves"}`, `{"d": "M…"}`,
`{"points": [[x, y], …]}`, with `offset`, `reverse`, `perpendicular`)
sets the letters along it; key `generator.path_start` 0..1 to run them
along; `null` straightens. `"kerning": false` drops the font's pair
kerning.

**3D text** (`text_3d` op): `{"op": "text_3d", "text": "GOLD", "size":
200, "depth": 60, "bevel": 6, "colour": "#e8c060", "side_colour":
"#8a6a20", "metallic": 1, "roughness": 0.25, "font", "weight", "at",
"duration", "x", "y", "z", "rotation": [x, y, z]}`: real extruded,
bevelled letters, a model lit by the scene's lights and seen by its
camera (add a light and a camera move for the classic logo reveal; turn
it with `animate` rotation_y).

**Expressions** (`expression` op): `{"op": "expression", "clip": 4,
"property": "rotation", "expr": "wiggle(2, 15)"}`; `"remove": true`.
A formula reading other clips (`prop`) becomes a driver, otherwise an
expression modifier. Safe by design (no loops, no files), worked out per
axis in stored units (opacity and scale 1 = 100 %, degrees, pixels):

- Statements: `amp = 20; freq = 3` then the result (`let` / `var`
  allowed); `c ? a : b`, `&& || !`, `[x, y]` (each axis its own),
  `value[0]` / `value[1]`, `// comments`.
- Names: `value`, `t` (clip seconds in a modifier, timeline seconds in a
  driver), `i`, `num_keys`, `pi`, `tau`.
- `wiggle(per_second, amount, octaves, seed)`, `loop_out("cycle" |
  "pingpong" | "offset" | "continue", n)` and `loop_in(…)` (`loopOut` spellings work too), `value_at(t)`, `velocity()`,
  `key_time(n)`, `key_value(n)`, `inertia(amount, per_second, decay)`
  (an overshoot that rings out after each key), `linear(x, a, b, v1,
  v2)`, `ease`, `ease_in`, `ease_out`, `posterize(x, steps)`, `noise`,
  `random(x, min, max)`, `prop("Clip name" or id, "rotation" | "x" |
  "position" | any path, axis, time)`, and the maths (sin, atan2, hypot,
  clamp, lerp, smoothstep, min/max…).
- Examples: a bounce after the last key `inertia(0.05, 3, 6)`; a pulse
  `value * (1 + 0.05 * sin(t * tau * 2))`; a needle following a dial
  `prop("Dial", "rotation") * -1`; a clock tick `floor(t) * 6`; a loop
  of the keys `loop_out("pingpong")`; motion on twos `value_at(posterize(t, 12))`.

**Time remapping** (`time_remap` op): which second of the footage plays
when. `{"op": "time_remap", "clip": 3, "keys": [[0, 0], [1, 4], [3, 4],
[4, 0]]}` (clip second → source second: a fast run, a hold, back to the
start), `"expr": "t * 0.5"` (a formula over the remap, driven live),
`"remove": true`. Speed settings are ignored while it is on.

**Templates** (`template` op): designed pieces with controls to fill
in, placed as one clip (its layers inside) that remembers its controls.
`{"op": "template", "list": true}` (or `"search": "news"`, `"category":
"chart"`) lists them with their controls. Place one: `{"op":
"template", "id": "lower_third.bar", "controls": {"name": "Ada
Lovelace", "role": "Mathematician", "accent": "#7c5cff"}, "at": 3,
"duration": 5}`; fill it in again any time with `{"op": "template",
"clip": 12, "controls": {"role": "Countess"}}` (time, place, effects
and keys on the clip stay). Lower thirds (bar, news, underline, pill),
titles (kinetic, focus, split, badge, circle_text), callouts (pointer,
circle, arrow: `point` is where it points), social (subscribe, handle),
transitions (diagonal, circle, bars: lay them over a cut, centred on
it), infographics (stat, progress, timeline, list), charts, end cards.

**Charts** (`chart` op): `{"op": "chart", "kind": "bar", "data":
[["Mon", 120], ["Tue", 180]], "title": "Weekly visits", "colour":
"#21c7a8", "prefix": "$", "suffix": "k", "decimals": 1, "at": 2,
"duration": 6}`. Kinds: bar (grow in turn, values counting up), line
(draws on, points pop), pie / donut (`hole` 0..0.9; sweeps round, a
legend of shares, the total counting), map (`[["Lisbon", -9.14, 38.72],
…]` longitude, latitude: pins popping, joined in order by travelling
arcs; `route: false` pins only). Data may also be `[{"label",
"value", "colour"}]` or text with `label, value` a line. Charts are
templates: fill one in again with new data through `template` + `clip`.

**Lottie**: shape layers export as real Lottie shapes (`export
{"preset": "Lottie"}`), every frame's outlines and paint keyed, so
they play anywhere exactly as here.

To turn one shape into another (a circle into a triangle, a logo into a
word), use the `morph` op; topic `morph`.

## Reference: Typography and fonts

- **Two styles at most**: one display face for headlines (bold, tight),
  one plain face for everything else. `check`: `too_many_fonts`.
- **Hierarchy by size and weight**, not by colour: headline 2-3 times the
  body size.
- **Readable**: at least 40 px on 1080p for any line meant to be read;
  60+ for social; short lines (under 7 words); never all caps for more
  than a few words; never stretch or squash type.
- **Time to read**: about 0.3 s per word fully on screen (at least 1.2 s
  for a sentence), counted from when it has finished arriving.
- Text never sits over busy detail or faces: give it a backing box, a
  darkened area, or move it.
- Numbers and claims in the user's words; never invented.

## Reference: Motion graphics: shapes, type in motion, templates, charts

- **Templates first** for the standard pieces (lower thirds, titles,
  callouts, end cards, charts): they are designed, timed and fill in
  again cleanly; build from shape layers only for what no template
  does. One lower-third design per video, the same placement every time.
- **Timing**: entrances 0.3-0.6 s with an ease out (things arrive and
  settle), exits 0.25-0.4 s with an ease in (they leave and accelerate
  away), and a hold long enough to read (0.3 s a word after it has
  arrived, 2 s minimum for a name). Overshoot for friendly brands,
  plain ease out for serious ones. Never ease in on an entrance.
- **Stagger**: letters 0.03-0.06 s apart, words 0.08-0.15 s, list items
  0.3-0.5 s; the stagger runs the way the eye reads. Elements of one
  piece arrive in order of importance: the box or line first, then the
  name, then the role.
- **Lower thirds**: inside the title-safe area (7-10 % in from the left
  and bottom), never over a face or a mouth, two lines at most (name
  bold, role lighter and smaller); on screen 4-6 s, early in the
  person's first answer, not again for the same person.
- **Shapes**: strokes 2-8 px at 1080p; one corner roundness for every
  shape in a piece; trim paths to draw lines on (0.5-1 s, ease out);
  repeaters for rhythm (dots, bars, rings) rather than copies; wiggle
  paths and wiggle() small (3-10 px, 0.5-2 per second) unless the look
  is deliberately nervous; a merge subtract cuts clean holes (rings,
  letters through a bar).
- **Kinetic type**: one idea per moment; the animator does the
  emphasis (a highlight sweep over the key word, a jump on the beat),
  not five effects at once. Blur-in (focus_in) reads cinematic, pop and
  wave read playful. Text on a path for round badges and stamps, not
  for paragraphs. Tracking-in (letters gathering) suits titles over 1 s.
- **Rich text**: stress one word with weight or colour, not both plus a
  size jump; a baseline shift for footnote marks and units (m², ×10³).
- **3D text and logos**: depth 20-35 % of the letter height, a bevel
  3-5 % so the edges catch light, a metallic or glossy face, one key
  light from above-front and a slow camera move or a turn of 10-30°
  (rotation_y). A full spin is a cliché.
- **Charts**: say one thing (the title says it); values counting up as
  bars grow; bars start at zero; a pie or donut for parts of one whole
  (2-6 slices, largest first), never for change over time (a line).
  Label values directly instead of with gridlines when there are few.
  Map routes in the order travelled; pins appear as the route reaches
  them. Never invent numbers: the data the user gave, in their units.
- **Expressions vs keys**: keys for a designed move; expressions for
  behaviour that should keep going or hold whatever happens
  (`wiggle`, `loop_out("cycle")`, a needle that follows a dial with
  `prop`, `inertia` for a settle after the last key). Keep amounts
  small and say what a formula does when you add one.
- **Transitions**: graphic wipes lie centred on the cut and cover it at
  their middle; use one kind per video, in the brand's colours.

## Reference: Text that moves

**Text that moves** (`text` or `set` on a title):
- `reveal`: an entrance per letter, word or line: fade, rise, drop, pop,
  slide_left, slide_right, type, spin, bounce, zoom, flip, spring
  (`{"kind": "pop", "unit": "letter", "duration": 0.6, "overlap": 0.5,
  "order": "centre"}`; orders forward, backward, centre, random).
- `exit`: the same kinds, played out at the end of the clip.
- `loop` while it is up: wave, pulse, jitter, sway, shimmer, wobble
  (`{"kind": "wave", "unit": "word", "speed": 1.5, "amount": 0.6}`).
- `unit_motion`: your own per-letter motion, one or a list: keys over
  each letter's progress 0..1 for `x`, `y` (in font sizes), `scale`,
  `scale_x`, `scale_y`, `rotation`, `opacity`, staggered by `unit`,
  `order`, `overlap`, with an `ease` (linear, smooth, in, out, in_out,
  overshoot, anticipate, elastic, bounce, hold) over `duration` seconds
  from `start`. All of these stack on one title.
