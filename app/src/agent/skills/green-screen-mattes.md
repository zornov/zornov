---
name: green-screen-mattes
use: Use when a subject must leave its background: green screen, cut-outs, roto, masks.
topics: keying, key, cut-out, cutout, roto, rotoscope, background, matte, mattes, mask, masks, cut_out, chroma, stabilize, track, tracking
tools: keying, cut_out, roto, mask, matte, mask_puppet, corner_pin, track, apply, effect, foreground, frame, check, place
---
**Order**
1. Pick the method by the footage. Green or blue screen, white or black background (a black background for fire and flares can use the `screen` blend instead): a key (`keying`, or a key effect from `catalogue {panel: "effects"}`). No screen: `cut_out` (people need no subject; anything else gets a point on it, two or three for a long thing, `exclude` what keeps getting caught, or a `box`). A shape you must edit or feather by hand, or something too fast to matte cleanly: `roto`. A window, rectangle or circle: a plain `mask`.
2. Key on clean, evenly lit footage. Pull a solid core and an edge matte separately, suppress spill but restore its brightness, then wrap the new background's light a few pixels onto the subject.
3. Look at the matte as well as the comp, at a moment of fast motion with `frame` and `detail: true`: hair and hands are where it fails.
4. Put the subject into the new place: the background below, the subject above, text behind a person when the title goes on the lane under the cut-out; match the subject's light and shadow to the place (skill vfx-compositing).
5. Blurring a face or plate: copy the footage to the lane above, blur the copy, `mask` it over the face, `track` the face on the original with `follow: "mask"`.

**Checks**
- Edges at 100 percent in motion; no green fringe, no chopped hair.
- A subject half out of frame looks chopped at the edge: use another method.
- The mask follows: confirm after any retime.

**Mistakes**
- Confusing keying (remove a colour) with keyframes (values over time).
- Keying uneven footage with one pass; use a clean plate or garbage-matte the far areas.
- Cutting out what a simple mask does, or a whole shot that is already a clean key.
- Moving a mask by eye at a few moments; `track` instead (skill vfx-compositing).

## Reference: Keying and tracking

- **Keying** (chroma key, luma key, difference key in `catalogue
  effects`) removes a colour or brightness to make a cut-out: green
  screen, a white background, a black background for fire and flares
  (or use `screen` blend for those). Key on clean, evenly lit footage;
  check edges with `detail: true`.
- Do not confuse keying (removing a colour from a picture) with
  keyframes (values over time) or with arcs (curved position paths).
- **Cut-outs** (`cut_out`): when there is no green screen. People:
  leave `subject` out. Anything else: pick it with a point on it (two or
  three for a long or thin thing), `exclude` what keeps getting caught
  (a chair the person sits on), or a `box` round it. Use one for a
  person in front of a new background or a graphic, text *behind* a
  person (title on the lane under the cut-out, the original clip under
  both), a subject over a blurred or darkened copy of its own shot, or
  `invert` to clear a person out of a room shot. Check edges at a
  moment of fast motion with `frame` (`detail: true`): hair and hands
  are where it fails. Do not cut out what a mask does more simply (a
  window, a rectangle, a circle), a whole shot that is already a clean
  key, or a subject that is half out of frame (the edge looks chopped).
- **Roto** (`roto`) when the outline must be a shape you can edit or
  feather by hand (a glow round a product, a colour change on one car,
  an effect inside a moving thing), or to cut out something too fast or
  blurred for a clean matte. A roto mask is a mask: `invert`, put an
  effect on the clip (it stays inside), or move a point that slips
  with `mask_puppet`.
- **Tracking** (`track`): when a title, arrow, sticker, blur or mask
  must stay on something that moves in the shot, track it; never key a
  layer by eye at a few moments (it slides off between them). Pick a
  point with detail (an eye, a logo corner, a window edge), not a plain
  wall or sky; `"point": "face"` finds the largest face. `radius` is how
  much picture around the point moves with it: about the size of the
  object (a face 60–120 px, a logo 40 px). Use a **plane** (four corners)
  for flat surfaces seen in perspective (a phone or TV screen, a sign, a
  wall, a book cover) and attach with `follow: "corners"`: the insert
  then bends with the surface. Point tracks attach with `position`
  (labels, arrows: they should not turn) or `transform` (stickers on a
  turning object, a hat on a head).
- Read the track's `confidence` and `weak_at`: below 50 % or a lost
  stretch means it slipped (a hand crossed it, motion blur, it left the
  frame). Look at those moments with `frame`; put it right with `fix` at
  the moment it is wrong, which re-tracks from there and moves every
  follower with it. A track that is lost half the time is the wrong
  point: start again on a better one.
- Do not track a still shot (a tripod or a photo: just place the
  layer), a subject that leaves the frame for good (cut, or let the
  label go with a fade before it leaves), or text that should stay
  readable over a fast whip (a follower moves as fast as the subject:
  give it a fade-out instead).
- **Blurring a face or a plate**: put the same footage on the lane
  above at the same moment (`place` with `lane`, `at` and `source_in` as
  the clip has them), blur that copy, `mask` it over the face, then
  `track` the face on the original with `attach` the copy and `follow:
  "mask"`: the blurred patch stays on the face.
- `reframe` is different: it follows faces and subjects to move the crop
  of footage in a vertical (or tighter) frame.
- **Stabilizing** (`stabilize`): `smooth` takes shake out of a handheld
  or walking shot and keeps its intended move (a pan, a walk); `lock`
  removes all camera motion, for a shot meant to be on a tripod
  (interviews, products, a static wide). Stabilizing zooms in to hide the
  moving edges: keep `max_zoom` near 1.1 (10 %) for 1080p so the picture
  stays sharp; it eases the correction when it would need more. A long
  `smoothness` (2–4 s) makes a floating, gimbal-like move; short (0.3 s)
  only kills the jitter. Lock on a subject (`track`: a tracked point)
  when the subject should stay still while the background moves.
- Do not stabilize footage whose shake is the point (action, found
  footage, a punch of energy), a shot already on a tripod or gimbal
  (`see` says "still"; the report's "shake before" under 1 px means
  there was nothing to take out), or a shot with a big fast move (a
  whip pan): it zooms in far for nothing. Stabilize before reframing,
  tracking or animating a clip's position: those keys sit on top.

## Reference: Cut-outs and roto

**Cut-outs and roto** (local models, nothing leaves the Mac):
`cut_out` removes what is behind the subject so the lanes below show
through. With nothing picked it keeps the **people** (MODNet: hair-fine
edges, every person in the shot); `subject: "object"` with `point` /
`points` (`[x, y]` canvas pixels on it at `at`), `exclude` (points that
are not it) or `box` (`[[x0, y0], [x1, y1]]` round it) keeps any object
(SAM 2.1), carried through the whole clip. `invert: true` keeps
everything but the subject (remove a person, keep the room); `enabled`
turns it off and on; `remove: true` takes it away. It takes about a
second a frame for objects (less for people), so cut out only the clip
that needs it, trimmed first. `roto` picks the same way and makes a
