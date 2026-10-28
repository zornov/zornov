# The editor's craft book (design basics)

The short rules every layout, shot and cut follows: layering, placement,
anchors, opacity, shadows, shapes, cuts, fades, flashes, markers and
searching. How to do each kind of work (stories, characters, 3D, sound,
colour, titles, vector, mattes, effects, export, review) is in the
skills: `skill {name}` loads one, and the old topic words
(`colour`, `sound`, `typography`, `puppetry`, `overdoing`...) still open
the skill that took them over. `guide` with `topic: "craft"` lists these
chapters.

The one rule over all of them: **restraint**. Every effect, flash, sound,
font, colour and transition must earn its place. When in doubt, leave it
out. One strong idea done well beats ten done at once.

## Layering

How Zornov stacks pictures:
- **Lanes higher up draw over lanes lower down.** The main lane is the
  bottom of the picture; overlay lanes above it are drawn in order, the
  top lane last (in front). Sound lanes do not draw.
- **New things go on top** of whatever is showing at their time. Build a
  frame from the back to the front: background first, then the picture,
  then boxes, then text on them, then the small accents.
- **To put something behind**: `card` with `behind: <clip>`, or `restack`
  (`clip`, then `behind: <clip>`, `in_front_of: <clip>`, or `to: "top"` /
  `"bottom"`). Never guess a lane number to get an order: lane numbers
  are names, not depths.
- A full-frame opaque layer (a card with no size, footage that fills the
  frame) hides **everything** below it. `check` reports `hidden_layer`.
- **3D models stay on one side of a full-frame card.** A fade or cover card
  between 3D lanes splits the scene: a model on a lane above the card shows
  through it while the rest of the world fades. Put a fade or cover above
  every 3D lane (`restack`, `to: "top"`), a backdrop below them all.
  `check` reports `3d_split_by_2d`.
- Inside a merged clip the same rules hold among its layers; the merged
  clip as a whole sits at its own lane.
- After `merge`, `unmerge`, `flatten`, `matte` or a deletion, **ids
  change**: take the ids the result names, or look again with `summary`
  / `inspect`. An id from before is stale; using it is an error, never a
  guess.

The depth of a frame, back to front: background (gradient, blurred
footage, texture) → the hero (footage, product, screen) → supporting
boxes and frames → text → accents (arrows, highlights, stickers) →
anything that sweeps across everything (a flash, a light leak).

## Placement and composition

- Coordinates are canvas pixels from the centre, y down. The title safe
  area is the middle 90% of width and height: keep text inside it; keep
  important things away from the bottom 15% on vertical video (the app's
  own buttons sit there).
- **Rule of thirds** for heroes and faces; **centre** for single big
  statements; **left-aligned** stacks for lists.
- **Alignment**: things that belong together share an edge or a centre
  line. Use `align` and `distribute` instead of eyeballing numbers.
- **Spacing**: equal gaps between equal things; more space around what
  matters most. Crowded frames read as cheap.
- **Visual hierarchy**: one thing is the most important on each frame.
  Make it the biggest, brightest or most saturated; everything else
  steps back (smaller, darker, greyer, softer).
- `pick` tells what is under a point; `on_screen` (in `inspect`) gives a
  clip's real box; use them instead of guessing where things landed.

## Anchor points, scale, rotation, crop

- The **anchor** is the point a layer scales and turns around (default:
  its centre). `set` `anchor: "left"` makes a bar grow from its left
  edge; `"bottom"` makes a stack grow up; a corner makes a card swing
  like a door. Setting the anchor never moves the picture.
- **Scale** footage at most to 100% of its own pixels; past that it goes
  soft (`check`: `upscaled`). Graphics (cards, shapes, text) are drawn at
  any size sharply. Animate `width`/`height` of a box, not `scale_x`,
  when the box has round corners (scale squashes them).
- **Rotation** in degrees, clockwise. Small tilts (2-8°) feel lively;
  big ones need a reason. Rotation keys turn the short or long way as
  written: 350 → 10 goes all the way back through 180.
- **Crop** (`crop_left|right|top|bottom`, percent) trims a picture's
  edges without scaling: for picture-in-picture, for removing black bars,
  for reveals (animate a crop from 100 to 0).

## Opacity, visibility and blending

- **Opacity** fades a layer. Text at less than 100% opacity looks weak:
  fade it in and out, but read it at full strength.
- **enabled: false** hides a clip without deleting it; a lane can be
  `hide`, `mute`, `solo`, `lock`.
- **Blend modes**: `screen` and `add` for light (flares, glows, light
  leaks over footage: black vanishes); `multiply` for shadows and dark
  textures (white vanishes); `overlay` / `soft_light` for texture over a
  picture. Normal for everything else. A blend is a deliberate choice,
  not a decoration.
- **Mattes**: `matte` shows a clip only through the shape (alpha) or
  brightness (luma) of the clip above it: footage inside big type, a
  picture revealed by a growing shape.

## Shadows, depth and glows

- A **drop shadow** lifts a card, a cut-out or a title off what is
  behind it (`card` `shadow: true`, or `effect` `drop_shadow`: distance
  0.5-2, softness 1-4, opacity 0.3-0.6, falling down: angle 90). Zornov
  draws it in three layers as real shadows fall (a tight dark one where
  it meets the ground, a softer one, a wide faint one), so it fades out
  with no edge. Soft and subtle; a hard black offset shadow looks dated.
- **Clean, not harsh**: soft edges come from Gaussian blur (big softness,
  low opacity), never from a hard shape at partial opacity; a shadow or
  glow should be felt more than seen. On a dark background a shadow
  barely shows: separate with a lighter outline or a faint glow instead.
- Light comes from one direction: every shadow in a frame falls the same
  way.
- **Glow** is for light sources (neon, a screen, a highlight), small and
  brief. A glow on everything is a smear. Glow, god rays, bloom and
  Gaussian blur are heavy to render: use them on small layers or for a
  moment.
- **Blur for depth**: a blurred, darkened background makes the sharp
  layer pop (Gaussian blur 1-3% on the background footage, exposure
  -0.5). Blur moving things only with motion blur-like effects
  (directional blur along the move), briefly.

## Shapes and boxes

- `card` with `width`/`height` is a box, and its `shape` field picks
  rectangle, ellipse, polygon (`sides`), star, arrow or line; `roundness`
  0-1 (0.1-0.2 is the modern look; 1 is a pill); `stroke` +
  `stroke_width` (pixels) for an outline:
  `{"op": "card", "shape": "rectangle", "width": 600, "height": 160, "roundness": 0.15, "colour": "#1b1f2a"}`.
- The `shape` op draws vector layers: one plain shape with `type` (rect,
  ellipse, polygon, star), `width`, `height`, `roundness`, `colour`,
  `stroke`; anything more with `items` (shapes, operators and paint in
  order) or a `preset` (skill `titles-typography` has the item list):
  `{"op": "shape", "type": "rect", "width": 400, "height": 120, "roundness": 0.2, "colour": "#ff3366", "at": 1}`.
- Boxes behind text: padding of at least the text height on each side;
  a soft shadow or a 1-2 px lighter outline separates a box from its
  background.
- Consistency: the same roundness and the same shadow for every box in a
  video.

## Cuts and transitions

- Most cuts are **straight cuts**, on the beat or on an action. A cut
  on movement hides itself; a cut between two similar shapes or
  movements (a match cut) feels clever.
- **Transitions** mark a change: of place, time, topic or energy. Pick
  one or two that suit the tone and repeat them. `check`:
  `transition_every_cut`, `transition_grab_bag`.
  - Dissolve: time passing, calm. Dip to black: an ending, a pause.
  - Whip pan, zoom through, spin: energy, between fast shots.
  - Wipe / push / slide: lists and comparisons, side by side.
  - Glitch cut: tech, hard energy, rarely.
- A transition is short: 0.2-0.5 s for energetic, 0.6-1 s for calm.
- **J and L cuts**: let the next shot's sound start before its picture
  (or the last shot's sound run on) so cuts feel smooth.

## Fades

- Fade picture in from black at the very start only if the opening is
  calm; a hook starts on a picture, not black.
- Fade music out at the end (1-3 s, `fade_out`); fade every sound in and
  out by a few frames so nothing clicks.
- Fade text in and out quickly (0.2-0.4 s); slow text fades look sleepy.

## Flashes and surprise

- A **flash** (a brief white or bright frame, `card` white for 2-4
  frames with fast opacity keys) punctuates the single biggest hit. One
  or two per video. Never three within a second: that strobes and can
  trigger seizures (`check`: `strobing`, `flash_overuse`).
- **Surprise** comes from contrast: quiet then loud, still then fast,
  small then huge, dark then bright. A surprise works once; set it up
  (slow down, hold, drop the music) and then break it. Repeated, it is
  noise.
- Camera shake, glitch and RGB split: a few frames on a hit, not a look
  for a whole scene.

## Markers

- Mark the plan before building: the hook, each section, the drops,
  where the voice says the key words (`marker` with `note`). Then build
  to the markers; `summary` lists them.
- Mark problems you will come back to ("fix colour here"); remove them
  when done. Markers are for you and the user, not decoration.

## Searching

- `search` looks through **everything Zornov knows across every
  project** at once: titles and on-screen text, what narration says and
  when, markers, media files and where they are used, notes, saved moves
  and designs. Use it before making something again: the line that worked
  last time, the footage already downloaded, the move made for a hook.
- `catalogue` with `query` as a list of plain words searches effects,
  looks, motions, text styles, transitions, assets and sounds at once:
  ask for what it does ("glow", "paper texture", "whoosh", "typewriter"),
  not a brand name. `panel` limits it to one kind; `details: true` shows
  settings.
- Stickers and assets: search by subject and mood; prefer one consistent
  set over a mix.
- Before downloading media, look in the project and the library first.
