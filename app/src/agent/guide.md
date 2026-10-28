# Zornov for agents

Zornov is a video editor built to be driven by you. Everything a person
can do in the window is a tool call here, in plain words and units, and
you can look at, listen to and read the work at every step.

## Your senses

- **See (start here)**: `see` is Zornov looking for you and saying what it
  sees, in a line per shot: light, clipping, contrast, colours, faces and
  how close, where the subject sits, camera motion, sharpness, bars,
  lettering, and what is playing there. `see` with `media` scouts footage
  before it is used, a line per second. A line costs a few dozen tokens; a
  picture costs hundreds, so look at pictures only when a line is not
  enough (composition, a match cut, a final check).
- **Read a place**: `see` with `regions: true` and `t` says what the
  backdrop is made of (sky, water, ground, path, trees, rocks, mountains,
  buildings, people, animals): how much of the frame each takes, where,
  how far, and its box in canvas px, plus where the far land ends and
  where the light is. A scene-parsing model (UperNet, 238 MB, fetched the
  first time) read against the depth map; cached per backdrop. `see`
  with `place` adds a `made of` line once the model is here. Use it
  before putting effects, particles or characters into a place (see
  Living places).
- **Know the objects**: `objects` names what is in footage and pictures
  and keeps it as a map (read once per file, cached by content): each
  thing with an id, label, a box in every frame, how far and how big in
  metres, colours, text on it, faces and facing, when it comes in and
  leaves, how it moves apart from the camera. `objects` lists a shot;
  `where {object, t}`, `when`, `around` (on, holding, in front, behind),
  `find {query}` (the moment that matches a description), `space` (clear
  room for a title) and `mask` (cut an object out, with ready `cut_out`
  and `track` ops). Boxes are canvas px. See the skill `objects-in-footage`.
- **Look**: `frame` (one moment) and `contact_sheet` (many moments in
  one picture). Both take `media` to look at a file before it is used;
  the times they print are seconds into the file, ready for `place`.
- **Frame against the guides**: the viewer's overlays (rule of thirds,
  golden ratio, centre cross, title / action safe, aspect guides 2.39,
  1.85, 4:3, 1:1, 9:16) are named on every request when the person has
  them on ("viewer overlays on: rule of thirds"). `frame` then draws them
  onto the picture and says where each figure sits against them (eyes on
  the upper third line, body on the left third, power points, headroom,
  lead room ahead of where it looks, inside title safe, cut by the 2.39
  frame); `see` with `t` says the same in a line per figure. Ask for
  guides yourself with `guides: ["thirds", "2.39"]` (`false` for none).
  `check` flags poor framing: a head cut in a wide shot or sinking under
  too much headroom, a figure looking into the near edge with no lead
  room, feet cut at the ankles or legs at the knees, a hand clipped by
  the edge, tangents (an edge just touching the frame or another figure).
- **Compare and review shots**: `compare_cameras` frames two to six
  options ("low angle close-up on Pip", "wide on the group, high angle")
  on the same moment, side by side, with the framing of each, and
  changes nothing; make the one you choose with the `shot` op.
  `animatic` lays out every camera's shot in order (first, middle, last
  moment) with its lens and moves: the storyboard of the cut.
- **Listen**: `listen` to a song, a sound effect or a clip. A song comes
  back as sections (main, peak, low, quiet) with times, its drops and
  breaks, bpm, bar length, beats and an energy line (▁▂▃▅▇). A short
  sound comes back as its shape (a hard hit, a boom with a tail, a swell,
  a swoosh), attack, tail and brightness. Cut to what you hear: put the
  biggest moment of the picture on the song's first drop. Every answer
  also has `heard`: what the sound *is*, half a second at a time
  (speech, music, speech over music, applause, noise, silence), its
  faults (clipping, clicks, mains hum, rumble, with times and a fix), the
  voice (pitch, pace, how roomy), loudness against the platforms (-14
  LUFS for social, -16 voice and podcasts, -23 broadcast; true peak under
  -1 dB), and `sounds_like`, all of it in a sentence or three. Speech
  under loud music is easy to miss by ear: transcription (`ai`
  `captions`) is sure of it. `listen` with `mix: true` (and `from` / `to`) hears the
  timeline's mix: the same, plus how far under the voice the music sits
  while someone speaks (15 dB or more is clear; under 9 dB it fights the
  words) and where it masks the consonants, with what to do.
- **Read**: `script` is the whole timeline as an edit decision list;
  `summary` the same as data; `inspect` one clip with every property and
  every keyframe. `notes` is your notebook, kept with the project: write
  the brief, the beat map and your plan there, and read it first when
  you come back.
- **Curves**: `graph` reads a clip's animation the way an animator
  reads the graph editor: per curve a sparkline (▁▃▆█), samples in the
  property's units, its keys with their curves and handles, the fastest
  moment, and faults: overshoot past the keys, a kink (the speed jumps
  at a key: a hitch the eye catches), holds. Check a move with it after
  `animate`, then reshape with `keys`: `curve` per key, or exact `out`
  / `in` handles (`[time fraction, value fraction]` of the move; a value
  fraction past 1 overshoots).
- **Measure**: `scopes` at a moment: luma percentiles (black p1,
  median, white p99), blown-out, crushed and out-of-broadcast shares,
  the cast in shadows, mids and highlights (RGB offsets and a name),
  saturation and the share on the skin line. `compare` another moment
  for the differences: grade one shot until its median, black, white
  and mid cast sit within a few percent of the other's. `picture: true`
  adds the parade and vectorscope.
- **Find**: `match` finds the moments in your footage that look like a
  frame (a clip's last frame, or a file at a time), by shape (where the
  strong lines are), look (colour and light) or both: candidates for
  match cuts and "more like this", with a sheet of them to look at.
- **Judge**: `check` finds what a professional would not ship: gaps,
  clips past their footage, black or dark stretches, sound running past
  the picture, clipping and loudness, and titles judged on the rendered
  picture (cut off, under the TikTok / Reels / Shorts buttons, hard to
  read, over faces or busy detail, on top of each other), lettering that
  came with the footage (a source title card), and a video with no sound
  at all. Each problem has a time, a clip and a fix.
- **Direct**: `review` watches the whole film as a viewer would, for what
  makes it lifeless when `check` finds nothing wrong: one backdrop held
  shot after shot, every shot the same length, cuts mid-sentence or off
  the beat, characters gliding with no foot planted, one performance
  copied between shots or stamped on everyone, sentences naming things
  that are not on screen while they are said, places with no sound of
  their own, and (from a contact sheet of the cut) long spans where the
  picture barely changes or frames minutes apart that look the same.
  Each note has times, clips and a fix.

## How a project is built

- A **project** has a canvas (width × height, fps) and one timeline.
- The timeline has **lanes**, stacked like layers: overlay lanes on top
  (titles, stickers, effect layers, picture-in-picture), the **main** lane
  (the story), sound lanes below.
- **The main lane is magnetic**: its clips sit end to end from 0. A new
  length or speed there ripples everything after it (a speed ramp is
  `split` then `set` `speed` 0.4). To change order use `move` `to` or
  `reorder`; `trim` there takes `ripple: true`. Overlay lanes hold clips
  anywhere.
- Times are **seconds**; positions are canvas pixels from the centre,
  **positive y down** (on 1080×1920 the top edge is y −960).

## Talking about clips

Wherever an op takes `clip` or `clips`:

| you write | it means |
|---|---|
| `12` | clip 12 |
| `"$1"`, `"$2"`, `"$last"` | the first, second, latest clip made earlier in this same `edit` call |
| `"@4.2"` | the main-lane clip playing at 4.2 s |
| `"music@4.2"`, `"overlay@4.2"` | the clip on that lane at 4.2 s |
| `"Night market"` | the clip with that name or title text |
| `"lane:main"`, `"all"` | every clip on a lane, every clip (in `clips`) |
| `{"lane": "main", "from": 2, "to": 8, "kind": "video"}` | the clips in a range (in `clips`) |

Media, lanes and library items are taken by name too (a near miss
returns suggestions). A field an op does not take is refused with the
list of what it takes; nothing is ever silently ignored.

## Properties: one vocabulary

`inspect` (with `at`) also says where the clip is on screen: its box,
corners and anchor point in canvas pixels; `pick` names the layers under a
point, topmost first. `inspect` lists every property of a clip by name with its value now and
its keyframes. `set` changes any of them and `animate` keys any of them,
always in the same units:

- `x`, `y` (px from centre), `scale`, `scale_x`, `scale_y`, `opacity`,
  `crop_left|right|top|bottom` (percent), `rotation` (degrees),
  `anchor_x`, `anchor_y`
- `volume` (dB), `pan` (−100…100), `fade_in`, `fade_out` (s), `muted`
- `exposure` (stops), `contrast`, `saturation`, `temperature`, `tint`
  (−100…100), other grade settings as `grade.<name>`
- each effect's settings as `<effect>.<setting>` (`zoom_blur.amount`; a
  second of the same effect `zoom_blur#2.amount`), masks as
  `mask1.<setting>`, title and shape settings as `generator.<setting>`
- 3D (after `three_d`): `z` (px, away from the viewer), `rotation_x`,
  `rotation_y` (degrees; `rotation` is z), `orientation_x|y|z`, `scale_z`
  (percent), `anchor_z`, `material.<setting>`; a camera's `zoom` (px),
  `focal_length` (mm), `fov` (degrees), `focus_distance`, `aperture`,
  `blur_level`, `poi_x|y|z` (what it aims at); a light's `intensity`,
  `colour`, `cone_angle`, `cone_feather`, `radius`, `falloff`,
  `shadow_darkness`, `glow`, `flare`, `beam`, `flicker`, `poi_x|y|z`; a
  model's `wetness` (0-1) and `emissive_strength` (a multiplier: glow
  brighter than white blooms); the air and look as `generator.<field>`
  (`generator.fog_density`, `generator.exposure`…)
- not animatable: `blend` (normal, screen, multiply, overlay, add…),
  `reversed`, `enabled`, `flip` (mirror left-right: true/false; `flip_y`
  upside down), `text`, `style`, `text_size`, `colour`, `name`

## Motion

`animate` keys any property above, in the same units, and gives you the
control of an animator:

- **Keys**: `[t, value]`, or `{"t": 0.4, "v": 120, "ease": "snap"}` to
  shape the move that starts at that key. Values are numbers, `#rrggbb`
  colours (`generator.colour` animates a title's colour) or `[x, y]`.
  Times are from the clip's start; `"timeline": true` takes timeline
  times, so keys can sit straight on beats from `listen`.
- **Curves** (`ease` for all keys, or per key): smooth, linear, hold,
  ease_in_out, ease_in, ease_out, overshoot (past the target and back:
  pops), anticipate (pulls back first), snap (arrives at once: hits),
  whip (slow then fast: into a cut). `keys` sets exact bezier handles on
  any key (`"out": [0.1, 0.9]`, fractions of the move's time and change;
  past 1 overshoots).
- **The GSAP curves**, as real keyframe curves (GSAP spellings work):
  elastic (rings around the target and settles: `"elastic.out(1.2,
  0.4)"`, amplitude and period), spring (a softer elastic), bounce
  (lands like a ball), back (`"back(2.5)"`: how far past), expo, circ,
  sine, power1..power4, steps(n) (stop-motion, ticking numbers). Each
  takes `_in`, `_out` (the default) or `_in_out`: `bounce_in_out`,
  `"power3.inOut"`. Springy arrivals: scale `[[0, 0], [0.6, 100]]` with
  `elastic`; drops: y with `bounce`.
- **Motion paths**: position moves curve instead of going straight.
  `"path": "smooth"` curves through every point; `"arc": 0.3` bows each
  move sideways by that much of its length (negative: the other way; a
  list, one per move). A key's own `"path": {"in": [dx, dy], "out": [dx,
  dy]}` sets exact handles. The clip travels the curve at the speed its
  `ease` says, overshoots along it, and `keys` with `"path"` reshapes it.
- **Stagger** (`stagger` op): one move on many clips, each after the
  last: `{"op": "stagger", "clips": [ids], "each": 0.08, "from":
  "centre", "animate": {"properties": {"scale": [[0, 0], [0.5, 100]],
  "opacity": [[0, 0], [0.3, 100]]}, "ease": "back"}}`. Keys are timed
  for the first clip; the others shift. `from`: start, end, centre,
  edges, random, or a place in the list. `by: "x"`, `"y"` or
  `"distance"` (from `origin`, the centre by default) staggers by where
  clips sit on the picture: a grid ripples out from a point. `amount`
  is the whole spread instead of `each`; `spread: "expo_in"` bunches
  the delays. `use_move` instead of `animate` staggers a saved move.
- **Several properties at once**: `"properties": {"scale": [...],
  "opacity": [...], "rotation": [...]}`.
- **relative**: scale and opacity values are factors of the clip's own
  value (`[[0, 1.15], [0.2, 1]]`), the rest offsets.
- **pulse**: `{"at": [beat times], "peak": 1.12, "decay": 0.25}` (with
  `relative`) punches in on every beat you pass.
- **wiggle**: `{"amount": 3, "per_second": 6}` hand-held wobble around
  the value (rotation, x, y, scale…), baked as keys. For a wobble that
  runs the whole clip or sits on top of a keyed move, use `modifier`
  noise instead: nothing to bake, and later keys keep it.
- **Modifiers** (`modifier` op): procedural layers on a property's
  keys, like curve modifiers. `{"op": "modifier", "clip": 4,
  "property": "rotation", "add": "noise", "amount": 4, "per_second":
  1.5}` wobbles ±4° around whatever rotation is keyed (or its value now).
  `add`: **noise** (`amount` in the property's units, `per_second`,
  `octaves` 1-8 for jitter, `seed`, `from`/`to` seconds to confine it),
  **cycles** (repeats the keys: `after` / `before` hold, repeat,
  offset (each loop carries on from the last: a spin that keeps
  spinning, a walk that keeps walking), mirror (ping-pong); `count`, 0
  for ever; needs two keys), **stepped** (`per_second` new poses: 12 is
  "on twos", 8 is choppy stop-motion), **limits** (`min`, `max` in the
  property's units), **expression** (`expr`, in stored units: opacity
  and scale 1 = 100 %, degrees, pixels; names `value`, `t` seconds, `i`
  axis, `pi`, `tau`; sin, cos, abs, min, max, clamp, lerp, smoothstep,
  noise, random…). One of each per property; adding again replaces it;
  `"remove": "noise"` or `"all"`. `inspect` lists them under
  `modifiers`. Loop a 1 s spin for ever: animate rotation `[[0, 0], [1,
  360]]` linear, then `cycles` with `after: "offset"`.
- **Drivers** (`driver` op): a property follows other properties live,
  nothing baked: move the source and it follows. Follow one:
  `{"op": "driver", "clip": 5, "property": "rotation", "from": {"clip":
  3, "property": "rotation"}, "multiply": 0.5, "add": 10, "delay":
  0.2}` (`add` in the driven property's units, `multiply` a plain
  factor, `delay` seconds behind: a follower that lags). Or a formula:
  `"vars": {"a": {"clip": 3, "property": "x"}}, "expr": "sin(a / 100) *
  50"`. Formulas work in stored units (opacity and scale are factors,
  1 = 100 %; degrees; pixels from the centre) and may use `value` (the
  property as keyed), `t` (seconds), `i` (axis). `x` and `y` drive one
  axis each; any keyable property drives or is driven (3D, effect and
  generator settings, materials, model layers). `remove: true` frees
  it; `inspect` lists `drivers`. Use drivers for relationships that
  must hold when things change (a shadow under a moving ball, a dial's
  needle and its number, gears turning together, a whole rig echoing
  one control); keys for one-off moves; `parent` when a layer should
  simply ride along.
- **Physics** (`physics` op): layers fall, bounce, tumble and stack
  as rigid bodies, baked into position and rotation keys (edit them
  like any keys afterwards). `{"op": "physics", "clips": [ids], "at":
  1.0, "duration": 2.5}` drops them onto the bottom edge. `bounce` 0..1
  (0.35), `friction` (0.5), `gravity` px/s² (about two frame heights),
  `floor` (a y in canvas pixels, or false), `walls` / `ceiling` (true:
  the frame's edges hold them in), `static` (clips they land on that
  stay put: a shelf, a logo), `push` (`[vx, vy]` px/s, or per clip
  `{"4": [300, -900]}`: thrown up and across), `spin` (deg/s), `shape`
  (box, or circle for round things, per clip too). Place the layers
  where they start (above the frame, tilted) before the run. Flat,
  unparented layers only.
- **speed** keys ramp the playback rate inside one clip:
  `{"property": "speed", "keys": [[0, 1], [0.8, 0.35], [1.6, 1]]}`.

`keys` edits single keyframes afterwards: remove, move, re-curve.

**Motion presets**: `motion` sets a clip's `entry`, `exit` and `loop`
(names from `catalogue motions`, each with its own `duration` and
`params`); they stack with keyframes. **Looks**: `look_strength`
(0-100%) is animatable; stack looks with `fx_layer`.

**Merge**: `merge` makes several clips one clip (a box and its title, a
cluster of stickers, a whole scene): it moves, animates and takes effects
as a unit and turns around its own middle; `unmerge` opens it.

**Layers without taking them apart.** Never unmerge just to change one
thing, and never export and re-import to combine layers:
- `inspect` on a merged clip lists what it holds (ids, times inside).
- Any op on a clip inside a merged clip works in place by its id (`set`,
  `animate`, `effect`, `keys`, `delete`…); `"inside": <merged clip>` on
  `text`, `card`, `add`… builds new layers in it. Times inside are the
  merged clip's own (0 = its start).
- Transitions between merged clips need no padding: past their contents
  their first and last pictures hold.
- `flatten` renders a merged clip once to a video that plays in its place
  (keeps its position, animation, effects and transitions): for heavy
  scenes that should play smoothly, or to use a built scene as footage.
  Opaque: anything it showed through turns black. `unflatten` brings the
  live layers back.
- `align` (`to`: left, centre, right, top, middle, bottom; `margin`;
  `relative: "first"` lines up on the first clip) and `distribute`
  (`axis`: horizontal or vertical) lay clips out by their real boxes.
- `lane` `lock` keeps a lane from being changed; `hide`, `mute`, `solo`.
- `parent` (`clip` or `clips`, `to`: the clip to follow, null to let go)
  makes clips follow another's position, scale and rotation, as in layered motion tools: a label rides its box, stickers orbit a logo, a whole cluster
  moves by animating one layer. Linking never makes anything jump; the
  followers keep their own animation on top.
- `matte` (`clip`, `with`: the clip above it, by default the nearest;
  `mode`: alpha, alpha_inverted, luma, luma_inverted) shows a clip only
  through the shape or brightness of the one above: footage inside big
  text, a picture revealed by a growing shape, a wipe through a gradient.
  The two become one merged clip; animate the matte inside it by its id.
  `remove: true` takes the matte off.

**Your own moves and designs**: `save_move` keeps everything animated on
a clip as a named move; the `use_move` op puts it on other clips, timed
to each one and relative to its own scale and position. `save_design`
keeps a group of clips (layers, timing, media, text, animation); the
`use_design` op places it anywhere, moved and with new text. `made`
lists them. They are kept for every project: build a vocabulary.

**3D** (3D layers, cameras and lights): `three_d` puts a
layer in space. The canvas is the plane z = 0; z is pixels away from
the viewer, so a layer at z = 1.39 × the canvas width (the lens's
distance, `zoom`) shows at half size. Rotation X tips the top away,
Y turns the right edge away, `rotation` is Z as in 2D. With no camera
3D layers are seen straight on (a 50 mm lens); `camera` adds one (aimed
at the canvas centre unless `look_at` says otherwise) and moves it:
`orbit` (degrees round what it looks at; `vertical` too), `pan` / `tilt`
(degrees, turning in place), `dolly` / `push_in` / `pull_out` (px along
its view), `truck` (sideways), `pedestal` (up), `roll`, `zoom` (`to` mm
or `by` a factor), `dolly_zoom`, `shake`; each with `from` / `duration`
(s into the camera clip) and `ease`. `light` adds parallel (sun), spot,
point or ambient light; lights fall only on 3D layers, and once there is
one, a layer facing away from every light is dark: add an `ambient` one.
Shadows need a light with `shadows: true` and layers with
`casts_shadows: true`. `extrude` (px) gives text and shapes thickness.
Depth sorting is automatic among 3D layers next to each other on the
lanes; a 2D layer, adjustment or matte between them draws by its lane.
3D motion presets for `motion` (any layer, 3D or not): entries
`turn_in`, `tilt_in`, `card_flip`, `swivel`, `fly_in`, `fall_back`;
loops `spin_3d`, `rock_3d`, `hover_3d`. `parallax` spreads layers in
depth (furthest first), keeps their size on screen, and moves a camera
past them: the 2.5D move. Between clips, the `flip` (card flip) and
`cube` transitions turn in 3D. `inspect` shows a clip's `space`. Read
`guide {"topic": "3d"}` (lenses, moves, light, how much is too much)
before building a 3D shot. Anything with volume (a head, a product, a
creature, a rock) is a real 3D model, not flat layers: see 3D models
(`guide {"topic": "models"}`).

    {"op": "three_d", "clip": 7, "rotation_y": -25, "extrude": 30, "casts_shadows": true}
    {"op": "camera", "focal_length": 35, "move": {"type": "orbit", "by": 30, "duration": 4, "ease": "ease_in_out"}}
    {"op": "light", "type": "spot", "position": [-500, -400, -900], "look_at": [0, 0, 0], "shadows": true}
    {"op": "parallax", "clips": ["background", "photo", "title"], "move": "push_in"}
    {"op": "motion", "clip": 7, "entry": "turn_in"}
    {"op": "animate", "clip": 7, "property": "rotation_y", "keys": [[0, -90], [0.8, 0]], "ease": "back.out"}

**path mask** with its points keyed every `every` frames (`detail`
points round the outline, `feather` px): editable afterwards, point by
point, with `mask_puppet` (`mask`, `point`, `keys`), and usable like
any mask (`invert`, effects inside it, a track matte for others).

    {"op": "cut_out", "clip": 4}
    {"op": "cut_out", "clip": 4, "subject": "object", "point": [120, -40], "exclude": [[300, 0]], "at": 1.5}
    {"op": "roto", "clip": 4, "box": [[-80, -200], [140, 220]], "at": 2.0, "every": 2}

## Ops for `edit`

One `edit` call is a list of ops applied as **one undo step, all or
nothing**; `"dry_run": true` checks a plan without changing anything.
Each op is `{"op": "<name>", …fields}`.

**Putting things on the timeline**

| op | fields | does |
|---|---|---|
| `place` | `media`, `at`, `lane`, `source_in`, `source_out` or `duration`, `speed`, `fit`, `x`, `y`, `volume`, `muted` | a piece of a file: which part, where, how fast, how framed (`fit`: `fit` / `fill` / `fill_picture`, which ignores letterbox bars). Footage used = duration × speed |
| `add` | `item` (or `query` + `kind`, `mood`, `min_seconds`, `max_seconds`, `same`: an unused sound or track that fits), `at`, `lane`, `panel`, `duration`, `x`, `y`, `scale`, `fit`, `opacity`, `rotation`, `volume` | a library item (music, sound effect, sticker, emoji, frame, overlay, background, shape) placed and sized in one op; a `duration` longer than a motion graphic loops it |
| `scene` | `say` (what it says), `show` (things to see), `title` (short words on screen), `where` (a place for the background), `mood`, `voice` (a voice name, or false for a silent caption), `duration`, `at` | one beat of a video, built by Zornov: `say` is spoken and the scene lasts as long as it; pictures from the library are sized and laid out to fill the frame, the first at once and the others as the voice names them, kept moving; the spoken words appear as captions; a whoosh on the cut; it follows the scenes before it |
| `narrate` | `say`, `at`, `voice`, `rate` (words a minute, 175), `volume` | speaks a line on the voice lane with the service chosen in Settings (Mac voices, Fish Audio, ElevenLabs or OpenAI: `voices` says which; `voice` is one of that service's voices); returns its words with their times. If it fails, say so: never work around it with another voice |
| `music` | `genre` or `describe` ("noir jazz with sax and double bass", "hopeful acoustic folk", "minimal electronic", "dark slow score"), `mood`, `instruments`, `bpm`, `key` ("D minor"), `duration` (default: the whole video), `at`, `cues` (`[{"at": 2.0, "kind": "hit"}]` on the project timeline: hit, resolve, riser, drop, swell), `tail`, `variant`, `volume`; or `item` / only `mood` for a library track | composes original music here in any genre, key and tempo, played by synthesised instruments, exactly as long as asked, hitting each cue on a beat and ending on a last chord that rings out into the tail; the reply says the key, tempo, band and where each cue landed. With only a `mood` it takes a fitting library track under the whole video (a fade at the end) and composes when the library has nothing; under narration it sits low and dips while someone speaks (skill `music-beat-sync` part Scoring) |
| `text` | `text`, `at`, `duration`, `style`, `x`, `y`, `size`, `colour`, `lane`, `backing` | a title; `backing: true` (or `{"colour": "black", "opacity": 70}`) puts a box sized to the text under it, so it reads on any picture; `backing: false` removes every box, the style's own plate too |
| `sfx` | `kind` (riser, whoosh, boom, hit; a place's bed: stream, waterfall, rain, wind, birds, crickets, fire, waves, forest, night, up to 120 s; or any foley such as footsteps, match, traffic, murmur, door, fire, thunder, an animal) or `describe` (any sound in words: "footsteps on gravel, distant period traffic and a café murmur", "pig oinking", "rain on a tin roof"), settings `surface`, `shoe`, `count`, `species`, `call`, `distance`, `size`, `speed`, `material`, `period`, `tail`, `intensity`, `at` or `ends_at` (where its peak or main moment lands), `duration`, `volume`, `lane`, `variant` | makes the sound to measure and places it (`variant: 1, 2…` for a different take): physical models made here, no file needed: a riser that peaks exactly on the drop (`ends_at`) with a tail, a whoosh as long as the transition, a match that strikes and burns, footsteps on fourteen surfaces, animal voices, passages of several sounds in one (skill `sound-design` part Foley) |
| `sing` | `voices` (`[{"voice": "soprano", "notes": "E5:0.5:doo G5:0.5:bee C6:1:bap r:0.5", "pan", "vibrato"}]`; or `notes` and `voice` for one), `bpm` (lengths as `1b`), `at`, `volume`, `lane` | sings a part: pitched notes with syllables (doo, bee, bap, la), one stem and lane per voice, with each syllable's time for lip sync; a synthetic voice, exact in pitch and rhythm, plainly artificial in tone, syllables not words (skill `sound-design` part Foley) |
| `foley` | `clip` (a puppet that has acted), `surface`, `size`, `character`, `volume`, `lane`; or `kind` (splash, rustle, thud, flap, swish) with `at` | footsteps (paddles, landings, taps) on the frames the puppet's feet land, made for the ground (its scene's place, or `surface`: grass, leaves, dirt, gravel, stone, wood, snow, sand, mud, water, metal, carpet, tile, wet pavement) and its size; laying again replaces them. Or a one-off action sound at a moment |
| `motion` | `clip`/`clips`, `entry`, `exit`, `loop` | motion presets, all three at once |
| `merge` / `unmerge` | `clips`, `name` / `clip` | several clips as one; edit inside it by the inner ids, or `"inside"` |
| `flatten` / `unflatten` | `clip` (merged) | render a merged clip to one video in place / back to live layers |
| `align` / `distribute` | `clips`, `to`, `margin`, `relative` / `axis` | line clips up on the canvas or each other / space them evenly |
| `parent` | `clip`/`clips`, `to` (null lets go), `at` | clips follow another clip's position, scale and rotation |
| `matte` | `clip`, `with`, `mode`, `remove` | a clip shows only through the shape or brightness of the clip above |
| `restack` | `clip`/`clips`, `in_front_of` or `behind` (a clip), or `to`: top / bottom | changes what is drawn over what (lanes higher up are in front) |
| `audio` | `clip`/`clips`, `effect` (eq, compressor, limiter, gate, de_esser, reverb, delay, pitch_shift) with `params`, `remove` (a kind, or true), `fade_in` / `fade_out` (s) with `curve` (linear, equal_power, s_curve, slow, fast), `preserve_pitch` | the sound side of a clip; volume and pan are keyed with `animate` (`volume` dB, `pan`) |
| `use_move` | `move`, `clip`/`clips`, `fit`, `at` | a move you saved (`stagger` puts it on many, one after another) |
| `use_design` | `design`, `at`, `x`, `y`, `text` | a design you saved |
| `card` | `colour`, `at`, `duration`, `lane`, optional `width`, `height`, `x`, `y`, `opacity`, `roundness`, `gradient` (`["#from", "#to"]`, with `angle`, or `radial: true` and `radius`), `shape` (rectangle, ellipse, polygon with `sides`, star, arrow, line), `stroke` + `stroke_width` (px outline), `shadow` (true, or `{"distance", "softness", "opacity", "angle"}`), `behind` (a clip it goes under) | a colour or gradient clip over the frame (backdrops, title cards, a beat of darkness, a white flash), or with a size a box or shape anywhere; it goes on top of what shows at its time unless `behind` says otherwise. Afterwards `set` / `animate` take its `colour`, `width` and `height` (pixels) and `roundness` |
| `shape` | `items` or `preset`, `inputs`, `at`, `duration`, `lane`, `x`, `y`, `name`, `behind`, `extrude`; or `clip` + `patch` / `add` / `items` / `inputs` | a shape layer: shapes (also `curve`: roses, spirographs, L-systems, Fourier epicycles, any formula; skill `3d-assets-scenes`, part `math shapes`), operators (merge, trim, offset, wiggle, zigzag, round corners, repeater, morph…) and paint, all animatable (see Motion graphics) |
| `text_3d` | `text`, `size`, `depth`, `bevel`, `colour`, `side_colour`, `metallic`, `roughness`, `font`, `weight`, `at`, `duration`, `x`, `y`, `z`, `rotation` | extruded, bevelled 3D letters, lit by the scene |
| `template` | `list` / `search` / `category`; `id`, `controls`, `at`, `duration`, `x`, `y`, `name`; or `clip` + `controls` | a motion graphics template (lower third, title, callout, transition, infographic, chart, end card) as one clip with controls to fill in again |
| `chart` | `kind` (bar, line, pie, donut, map), `data`, `title`, `colour`, `text`, `prefix`, `suffix`, `decimals`, `hole`, `route`, `at`, `duration` | a chart animated from data |
| `fx_layer` | `item`, `at`, `duration`, `params`, `lane`, `animate` | an effect or look over everything below for a stretch (hits on cuts); `animate`: `{"amount": [[0, 0.8], [0.25, 0]]}` |

**Cutting and arranging**

| op | fields | does |
|---|---|---|
| `split` | `at`, `clips` | cuts clips under `at` |
| `trim` | `clip`, `start`, `end`, `ripple` | moves a clip's edges |
| `delete` | `clips`, `ripple` (default true) | removes clips |
| `move` | `clips`, `by` or `to`, `lane` | moves clips in time or to another lane |
| `reorder` | `clip`, `before` | moves a main-lane clip before another (end if none) |
| `roll` | `clip`, `by` | moves the cut after a clip: one gets longer, the next shorter |
| `slide` | `clip`, `by` | moves a clip between its neighbours, which give and take |
| `slip` | `clip`, `by` | plays a different part of the source in the same place |
| `duplicate` | `clips` | copies clips in right after themselves |
| `freeze` | `clip`, `at`, `duration` | holds the frame at `at` |
| `replace` | `clip`, `media` | swaps the footage, keeping timing and look |
| `detach_audio` | `clip`, `linked` | the sound becomes its own clip |
| `unlink`, `link`, `group`, `ungroup` | `clips` | how clips move together |

**Look and feel**

| op | fields | does |
|---|---|---|
| `set` | `clip` (or `clips`: many at once), any properties, `fit`, `speed`, `duration`, `start`, `backing`, `anchor`, `at` | changes clips. `anchor`: where scale and rotation turn, by name (`top_left`, `centre`, `bottom_right`…) or `[x, y]` pixels from the layer's centre; the picture stays where it is. `at` (seconds): an animated property gets a key at that moment instead of being outvoted by its keys |
| `repeat` | `times` (or `for`: a list of items), `do`: the ops, optional `let` (lists or numbers every turn shares), `as` | writes ops out many times inside the edit: no script needed. In `do`, `"{expr}"` is worked out: `i` (0, 1, 2…), `n`, each item's fields, lists as `h[i]`, `+ - * / %` and brackets; a whole-string `"{i*0.5}"` stays a number, `"bar {i+1}"` becomes text. Repeats nest; `$last` counts every clip made |
| `animate` | `clip` or `clips`, `property`, `keys`, `ease`, `relative`, `path`, `arc`; or `curve` (a path from mathematics: a preset such as lissajous, rose, spirograph, heart, or formulas, or a 3D `attractor`) / `formula` (a formula of t) | keyframes a property (replaces its keys); position moves bend with `path: "smooth"` or `arc`; along a curve or by a formula the keys are written for you (skill `3d-assets-scenes`, part `math shapes`) |
| `stagger` | `clips` (in order), `each` or `amount`, `from`, `by`, `origin`, `spread`, `animate` or `use_move` | one move on many clips, each a little after the last (GSAP's stagger), as real keys on every clip |
| `driver` | `clip`, `property`, `from` (+ `multiply`, `add`, `delay`) or `vars` + `expr`, `remove`, `enabled` | the property follows other properties live (drivers): no keys baked |
| `expression` | `clip`, `property`, `expr`, `remove` | a formula on a property (wiggle, loop_out, value_at, links with prop…; see Motion graphics) |
| `time_remap` | `clip`, `keys` (`[clip s, source s]`), `expr`, `remove` | which source second plays when: holds, reversals, ramps by keys or formula |
| `physics` | `clips`, `at`, `duration`, `bounce`, `friction`, `gravity`, `floor`, `walls`, `ceiling`, `static`, `push`, `spin`, `shape` | rigid-body fall, bounce and stacking, baked into position and rotation keys |
| `modifier` | `clip`, `property`, `add` (noise, cycles, stepped, limits, expression) or `remove`, and that kind's settings | procedural layers on a property's keys: wobble, loop, hold poses, clamp, a formula; one per kind, kept when keys change |
| `keys` | `clip`, `property` (or `"all"`: every animated property at once), `remove`, `move`, `curve` | edits keyframes: `move` `[[from, to]]` retimes, `curve` `[{"t", "curve"}]` sets how a key eases into the next (smooth, linear, hold, ease_in_out, ease_in, ease_out, overshoot, anticipate, snap, whip; or `out` / `in` handles) |
| `apply` | `item`, `clips`, `panel`, `params`, `duration` | an effect, look, motion or transition |
| `effect` | `clip`, `effect` (name or place), `remove`, `enabled`, `params` (each effect's own setting names; `blur` means a blur's `radius`, `strength` its `amount`), `move_to`; or `item` (+ `params`) to add one, as `apply` | one effect in a clip's stack |
| `copy_look` | `from`, `clips`, `parts` (transform, blend, audio, effects, grade, masks) | copies parts of one clip onto others |
| `mask` | `clip`, `shape` (rectangle, ellipse, line, mirror, star, heart), `params`, `invert`; or `mask` (1…) to change, `remove`; or `region` (sky, water, ground, path, trees, rocks, mountains, buildings, people, animals; also river, foliage, road…) with `t`, `feather` (0.01), `expand`, `invert`, `backdrop` | shapes what shows; `region` traces that part of the backdrop into path masks (outlines, holes cut out) so the clip shows only there |
| `morph` | `clip`, `to` (shape items, `{"clip": id}`, `{"text"}`, path data; for a mask `{"shape"}`, `{"mask": n}`, `{"points"}`), `at`, `duration` (1), `ease`; `mask` (1…) or `object` (vector art) | one outline really becomes another, points matched (equal resampling, corners kept, least-motion start, winding and subpaths paired); keyed on the timeline — see topic `morph` |
| `particles` | `preset`, `at`, `duration`, `region`, `t`, `lane`, `behind` (a clip: goes just under it), `params` (`amount`, `size`, `speed`, `direction`, `gravity`, `wind`, `turbulence`, `colour`, `opacity`, `ground`, `horizon`, `on_ground`, `depth`, `look`, `emitter`, `area_a`, `area_b`, `seed`) | a clear layer of rain, snow, leaves, dust, fireflies, sparks, smoke, mist, splashes, bubbles, confetti, magic dust, clouds, glints, stars or embers; with `region` they start in that part of the backdrop and show only there (see Living places) |
| `living` | `recipes` (water, foliage, clouds, rays, dusk, night, day_to_night, night_to_day, rain, storm, snow, mist, fog, leaves, fireflies, dust, embers, magic), `backdrop`, `at`, `duration`, `ramp` (`[from, to]` seconds into the layers), `front` (mist in front of the characters) | brings a still backdrop to life, a layer per recipe, each masked to its region (see Living places) |
| `puppet` | `clip`, `pins` (`[{"name", "at": [x, y], "kind"}]`), `pin` + `to` / `by` / `rotate` / `scale` / `amount` / `extent` / `keys`, `remove`, `density`, `enabled`, `at`, `rig`, `facing`, `act`, `from`, `duration`, `speed`, `travel`, `voice`, `blink`, `look_at`, `face`, `parts`, `cuts`, `auto_cut` | pins that bend a layer; rigs and acts a character, its mouth, blinks and eyes; `rig` also cuts the legs, arms and mouth apart (see Puppet pins, Scissoring) |
| `lip_sync` | `clip` (the character), `audio` (the line's clip; the voice lane clip under the character if left out), `from`, `duration`, `intensity` (1 natural), `emotion`, `lead` (s, default 0.06), `offset` (s later), `extras` (false: no blinks, brows, eye movement), `check` (true: read the mouth back against the voice and list problems with fixes) | real mouth sync from the line's phonemes (dictionary + rules, laid on the audio): visemes blend, lead the sound and hold M/B/P shut; drives a 3D face's shape keys and jaw/tongue/eye bones (keys on `generator.shape.*`, editable), a puppet's mouth pins, or a drawn mouth chart (`mouth rest`, `mouth MBP`, `mouth AA`…); `narrate` takes `speaker` to do it for the character that says the line |
| `skeleton` | `clip`, `build` (biped, quadruped, bird, fish, snake, plant, object), `facing`, `bones` (`[{"name", "parent", "head", "tail", "min", "max", "depth"}]`), `bone` + `min` / `max` / `depth` / `deform` / `rename` / `parent`, `remove`, `enabled`, `falloff`, `act`, `from`, `duration`, `speed`, `travel` (px or "auto"), `cuts`, `auto_cut`, `at` | bones that bend a character and carry its parts; motions on them; `build` also cuts the legs, arms and mouth apart (see Skeletons, Scissoring) |
| `scissor` | `clip`, `cuts` (`[{"name", "path": [point, …], "fill"}]`), `auto` (true, legs, arms, mouth, gaps, parts, all), `cut` (id or name) + `path` / `by` / `fill` / `name` / `enabled`, `remove` (id, name, "all"), `list`, `at` | cut lines that split the mesh a puppet or skeleton bends, so legs, arms and lips move apart instead of stretching together; a fill shows in the gap |
| `pose` | `clip`, `at`, `bones` (`{name: degrees}`), `offset`, `key`, `keys` (`[{"t", "bones" / "apply"}]`), `save`, `apply`, `copy`, `paste`, `mirror`, `reset` | poses, pose keys and the pose library |
| `ik` | `clip`, `bone` (or "front hand", "back foot") / `handle`, `to`, `by`, `chain`, `flip`, `pole`, `stretch`, `soft`, `pin`, `key`, `live`, `keys` (`[[t, point]]`), `blend`, `off`, `at` | a hand or foot to a point, the limb following (baked or live) |
| `parts` | `clip`, `face`, `at` | splits a rigged picture into part layers on its bones |
| `aim` | `clip` (the rig), `bone`, `to` (point or bone), `limit` (degrees each way), `keys`, `live`, `blend`, `key`, `off`, `at` | turn a bone to face a point (head, eye, arm): a one-bone `ik` |
| `retarget` | `clip` (takes the motion), `from` (the animated rig), `map` (`{target bone: source bone}`), `start`, `duration` | one rig's animation onto another of other proportions (pairs by bone role, travel scaled to leg length) |
| `swap` | `clip` (the rig), `set`, `add` (`{option: picture}`), `bone`, `place`, `width`, `keys` (`[[t, option]]`), `show`, `list`, `at` | swap sets: mouth shapes, hand poses, eye states |
| `character` | `create` / `name`, `from`, `kind`, `description`, `generate` ("turnaround" or `{"views", "poses", "expressions", "mouths", "hands"}`), `estimate`, `add` (`{"kind", "name", "picture"}`), `use`, `at`, `duration`, `lane`, `x`, `y`, `mouths_on`, `save_rig`, `list` | the project's character library: one look across shots |
| `mocap` | `clip` (the rig), `video`, `from`, `duration`, `rate`, `smooth`, `mirror`, `swap_sides`, `strength`, `root` (full, vertical, none), `map`, `at` | a filmed person drives the skeleton |
| `mask_puppet` | `clip`, `mask` (1…), the same pin fields, `point` (1…) + `to` / `by` / `keys` | bends or animates a mask's outline (it becomes a path) |
| `corner_pin` | `clip`, `top_left`, `top_right`, `bottom_right`, `bottom_left` (`[x, y]` or `{"by": [dx, dy]}`), `skew` (degrees), `keys` per corner, `reset`, `remove`, `at` | perspective and skew (a true perspective pin: screens, signs, a picture on a wall) |
| `three_d` | `clip`/`clips`, `on` (false: back to 2D), `z`, `rotation_x`, `rotation_y`, `rotation`, `orientation` `[x, y, z]`, `scale_z`, `anchor_z`, `auto_orient` (off, path, camera), `look_at` (`[x, y, z]` or a clip), `extrude` (px), `casts_shadows`, `accepts_lights`, `accepts_shadows`, `material` (`ambient`, `diffuse`, `specular` %, `shininess`, `side_shade` %) | puts a layer in 3D space (see 3D) |
| `camera` | `clip` (else the camera at `at`, or a new one), `new`, `at`, `duration`, `type` (one_node, two_node), `position` `[x, y, z]`, `look_at`, `zoom` / `focal_length` (mm, or "anamorphic 40") / `fov`, `lens_kit`, `sensor`, `squeeze`, `breathing` %, `bokeh`, `distortion` %, `fringe` %, `dof`, `focus` (px or a clip), `aperture`, `blur_level`, `roll`, `auto_orient`, `move` / `moves` (`{"type", "by", "from", "duration", "ease", "vertical", "around"}`; also rack_focus, follow_focus, dolly_path, crane, steadicam, handheld + `profile`, drone + `style`, follow) | adds, aims and moves a camera on a real lens |
| `light` | `clip` (else a new one), `type` (parallel, spot, point, ambient), `position`, `look_at`, `colour`, `intensity` %, `cone_angle`, `cone_feather` %, `radius`, `falloff`, `shadows`, `shadow_darkness` %, how it shows itself: `glow`, `flare` (lens streak), `beam` (cone or shaft in the air), `source_size` (m), `flicker` (0-1) with `flicker_speed` (Hz), `flame` with `flame_height`, `flame_width` (m) and `flame_tongues`, and `atmosphere` (this light carries the scene's air and look; set it with `atmosphere`) | adds or changes a light |
| `atmosphere` | `preset` (neon_rain, candlelit, foggy_dawn, snowy_dusk, desert_noon, overcast_mist, storm, ember_smoke, noir_haze, clean_film; `list` describes them) or fields: `ground` (world y of the floor, or "auto"), `metre`, `fog` (false clears) with `fog_density`, `fog_falloff`, `fog_base`, `fog_colour`, `fog_anisotropy`, `fog_range`, `fog_patchiness`; `weather` (a word, or up to 3 `{kind, amount, wind [x, y, z], size, speed, colour, splash, zone}`: rain, snow, dust, embers, sparks, leaves, ash, smoke); the look: `tonemap` (filmic, soft, none), `auto_exposure`, `exposure` (stops), `contrast`, `saturation`, `temperature`, `tint`, `richness`, `shadow_tint` and `highlight_tint` (+ `_amount`), `bloom_strength`, `bloom_threshold`, `bloom_knee`, `bloom_radius`, `vignette`, `grain`, `halation`, `fringe`, `reflection`, `air_light`; `clip` (the carrier), `at`, `duration`, `remove` | the scene's air and look on one light clip (made if none, lights nothing); a preset replaces the air, fields change it |
| `shot` | `say` (words: "slow push in on Pip") or `on`, `size`, `angle`, `move`, `speed`, `lens`, `shallow`, `dutch`, `yaw`, `profile`, `focus_to`; `at`, `duration`, `clip` | a camera framing subjects by name at a shot size, then moving (see Cinematography) |
| `lighting` | `preset` (three_point, rembrandt, low_key, golden_hour, motivated…) or `mood`, `on`, `at`, `duration`, `intensity` %, `colour` (key), `replace`, `glow`, `flare`, `beam`, `flame`, `flicker` (how its lamps show themselves; firelight flickers and neon glows by default), `atmosphere` (true: the air that goes with it, or a preset name) | a whole lighting set-up round a subject |
| `previs` | `shots` (words or shot fields with `duration`; default the story's shot list), `cast`, `set`, `at` | stand-ins, a plain set and a camera per shot |
| `parallax` | `clips` (furthest first), `depth` (px), `move` (truck, push_in, orbit… or `{"type", "by"}`), `amount`, `ease`, `overscan` % | layers spread in depth and a camera moving past them |
| `body` | `clip`/`clips`, `type` (auto, dynamic, static, kinematic, none), `material` (wood, oak, steel, glass, rubber, stone, plastic, flesh, fabric, ice … 45 of them), `mass` (kg), `collider` (auto, box, sphere, capsule, hull, parts, mesh), `friction`, `bounce`, `breakable` | what a 3D model is physically; the reply gives its mass (solid volume × density) and collider. Scenery is static and props stay put until simulated; `dynamic` ones move in `bake_physics` (skill `physics-3d`) |
| `joint` | `clip`, `to` (a clip; none: the world), `kind` (hinge, ball, slider, fixed, spring, rope, rod), `at_point` `[x, y, z]`, `axis`, `limits` (degrees, px for a slider), `motor` (`[speed, torque]`), `spring` (`[Hz, damping]`), `breaks` (N), `remove` | doors, pendulums, wheels, drawers, chains, lamps on cords |
| `drop` / `throw` / `push` | `clip`/`clips`, `at`, `duration`, `floor`; drop: `height`, `spin`; throw: `velocity` (px/s) or `toward` (a clip or point) + `time`, `spin`; push: `direction` + `strength` (px/s) or `from`, `point` | physics with the whole scene (it lands, bounces, knocks things over, breaks what is fragile), baked into keys |
| `break` | `clip`, `at`, `point`, `pieces`, `when_hit`, `by` | shatters a model into pieces that fly on and land (glass, china, plaster break by themselves when hit hard) |
| `settle` | `clips` (none: everything floating, sunk or about to tip), `at` | drops models for real until they stop and leaves them there |
| `ragdoll` | `clip` (a rigged character), `at`, `duration`, `strength` (0 limp … 1 holds its pose), `fade`, `push` `[x, y, z]` px/s, `point` | the character goes loose: falls, is knocked over, collapses (bone keys) |
| `jiggle` | `clip` (rigged), `bones` (default every tail, ear, antenna, hair, coat tail, feather bone), `stiffness`, `damping`, `gravity`, `drag`, `flesh` (0..1: loose skin shakes), `at`, `duration` | secondary motion: follow-through and settle after the body moves |
| `bake_physics` | `at`, `duration`, `clips` (extra dynamic ones), `react` (characters knocked loose, default true), `floor` | the whole scene from its physics settings: props, joints, breaks, characters pushing and pushed |
| `physics_world` | `px_per_metre`, `gravity` (m/s², 9.81), `rest_on_place` | the scene's scale and gravity; new models rest on what is under them unless `rest_on_place` is false (or the `model` op has `rest: false`) |
| `transition` | `clip` (before the cut), `item`, `duration`, `align`, `remove`, `keep_timing` | the transition into the next clip; keeps the timing (uses spare footage), trims only with `keep_timing: false` |
| `reframe` | `clips`, `smoothing` | keeps the subject in frame in a crop (vertical from wide); set `scale` / `fit` first |
| `track` | `clip`, `at`; `point` (`[x, y]` or `"face"`) with `radius`, or `plane` (four corners); `motion` (translation, similarity, perspective), `name`; `track` + `fix` / `retrack` / `remove` / `name`; `attach` (a clip) with `follow` (position, transform, corners, mask) and `mask`; `detach` (+ `clear`) on a follower | tracks a point or a surface in the footage and makes other layers follow it (see Tracking) |
| `stabilize` | `clip`, `mode` (smooth, lock), `smoothness` (s), `rotation`, `scale`, `max_zoom`, `track`, `remove` | takes camera shake out (see Stabilizing) |
| `smooth_motion` | `clip`, `method` (flow, blend), `quality` (full: the footage's own size, slower), `estimate`, `long`, `remove` | in-between frames for a slowed or ramped clip (RIFE optical flow), so slow motion does not stutter; about half a second a new frame at 720p: `estimate: true` first for long clips |
| `match_colour` | `clip` / `clips`, `to` (the reference shot), `strength` (0-100 %), `remove` | moves shots' colour and light to the reference's (shot matching; their own grade still applies on top) |
| `shadow` | `clip` / `clips` (or `at`: every character showing then); `angle` (where it falls on screen: 0 right, 90 towards the camera, 270 back), `length` (× the character's height), `softness` (× its height), `opacity`, `contact` (the dark patch under the feet) 0-100 %, `colour`, `feet` ([u, v] of its picture), `ground` (y px: stays on that line while it jumps), `enabled`, `remove` | a soft shadow on the ground under a character, made from its own bent picture so it walks and acts with it; with no settings given, all of them are read from the backdrop's light (see Characters in their backdrop) |
| `light_match` | `clip` / `clips` (or `at`), `amount` (0-200 %, default 100), `rim` (0-200 %, or false), `remove` | the character lit like the place it stands in: darker in shade, brighter in sun, tinted by the light around it, and a rim of the scene's key light on the edges facing it (`rim_light` effect) |
| `foreground` | `piece` (a number from `see` with `place`) or `point` / `points` / `box` (canvas px), `exclude`, `at`, `hides` (the characters it goes over; default those it overlaps), `backdrop`; `clip` + `remove` | cuts a rock, trunk or grass out of the backdrop (SAM and the depth map) onto a lane over the characters, so they can stand behind it; it moves with the backdrop |
| `cut_out` | `clip`; `subject` (people, object, face), `point` / `points`, `exclude`, `box`, `at`; `invert`, `smoothing`, `enabled`, `remove` | removes the background (keeps people or a picked object) so lanes below show through (see Cut-outs) |
| `roto` | `clip`, `point` / `points`, `exclude`, `box`, `at`, `every` (frames), `detail` (points), `feather` (px), `invert` | a path mask round a picked object, keyed through the clip |

**Structure**

| op | fields | does |
|---|---|---|
| `lane` | `action` (add, rename, lock, unlock, hide, show, mute, unmute, solo, unsolo, move, remove), `lane`, `name`, `kind`, `position`, `to` | lanes |
| `marker` | `at`, `note`; or `remove` (a time), `move` (`[from, to]`) | markers |
| `keyframe` | `clip`, `property`, `time`, `value` | one key in model units (prefer `animate`) |
| `raw` | `edit` | a model edit, for the few things above does not cover (`edit_schema`) |

## Keeping your context small

- `summary` and `script` end with the revision; next time pass `since`
  with it and only what changed comes back.
- `frame` does not send a picture again when nothing on screen changed
  since you got it (`force` sends it anyway).
- `catalogue` leaves effect settings out unless you ask (`details`) or
  narrow to a few.
- `see` before `frame`; `check` remembers what it already measured, so
  running it again after a small change is quick.

## Good habits

- Look and listen before cutting; `check` and look again, then `review`
  and act on every note (review again after), before saying it is done.
  "Nothing to fix" is never the answer while `review` has notes: fix
  them or say plainly which you left and why.
- Fewer round trips: calls sent together in one script run at once, so
  look, `check` and `listen` in one go, and an `edit` with its `frame`
  after it. Put everything you can foresee into one `edit` (`"$last"`
  names what an earlier op made); `read_back` is the state, so no
  `summary` after it. Send several downloads or pictures together
  (`items` for pictures), never one at a time. Do not guess links.
- Read `read_back` after every `edit`: it is what is on the timeline now.
  An op listed under `unchanged` did nothing; say so, or fix it. Report
  in three parts: seen (looked at in a frame or sheet), set but not
  looked at, failed. Passing checks is not looking good.
- One `edit` with many ops is one undo step; `undo` takes back the whole
  call. If an op is refused the message says why and nothing changed.
- `export` waits for the file and reports its peak and loudness.
- `place`, `set`, `animate` are ops inside `edit`, not tools of their own:
  `edit` `{"ops": [{"op": "place", …}]}`.

## The window's panels

The editor window is panels docked in a tree, the way the person arranged
them: splits side by side or stacked, and tab stacks (several panels in
one place, one showing). Panels: `library`, `viewer`, `inspector`,
`timeline`, `sound` (mixer, loudness, dialogue, subtitles), `scopes`,
`review` (notes, versions, team), `record` (camera, voice-over,
prompter, live), `editorial` (timeline index, takes, script lines) and
`ai` (you, the AI editor). Review, Record and Editorial can also float in
a window inside the editor, and any panel but the Viewer can pop out into
a window of the system's own (another screen). Only in the Zornov window,
not from a shell.

- `layout {}` tells you every panel (`open`, `showing` when it is the tab
  in front, `docked`, `floating`), the whole `tree`, the workspace and the
  workspaces there are.
- `layout {"action": "open", "panel": "scopes"}` and `"close"` show or
  hide one (the Viewer always stays). An open panel behind another tab
  comes to the front.
- `layout {"action": "dock", "panel": "scopes", "target": "inspector",
  "zone": "bottom"}` puts it under the Inspector; zones `left`, `right`,
  `top`, `bottom`, or `tab` to stack it with the target. `"target":
  "window"` docks along a whole edge of the window. The panel takes about
  half the target (less when it wants less); the others keep their place.
- `"float"` takes Review, Record or Editorial out into a window inside the
  editor. `"pop_out"` puts any panel but the Viewer in a window of its
  own; `"dock_back"` (or closing that window) puts it back where it was.
  Your work goes on wherever your own panel is, even while it is closed.
- `"workspace"` with `"name"` switches (Edit, Colour, Audio, VFX, Motion
  graphics, Production, or one the person saved); `"save_workspace"`
  keeps the window as it is now under a name; `"reset"` goes back to
  Zornov's own layout. `"set"` with a `tree` from `layout {}` lays out a
  whole tree at once.

The layout is the person's: change it when they ask ("put the scopes next
to the viewer", "a colour layout"), or when a panel you are using with
them is closed (open the Sound panel before talking them through a mix).
Say what you moved; Layout ▸ Reset layout puts Zornov's own back.

## From a shell

`zornov agent --project DIR TOOL '{"json": "args"}'` runs one tool on a
project folder (created if empty) and saves after every change; undo and
redo carry across calls. `zornov agent --project DIR -` reads calls
(`{"tool": …, "args": …}`, one per line or as one array) from stdin.
Pictures are written as JPEG files and their paths printed.
`zornov mcp --project DIR` is the same as an MCP server.
