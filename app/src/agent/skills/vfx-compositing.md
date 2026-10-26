---
name: vfx-compositing
use: Use when a shot needs compositing, tracking, stabilising, clean-up or a new sky.
topics: vfx, visual-effects, compositing, comp, nodes, matchmove, camera-tracking, cleanup, clean-plate, sky, light-wrap, grain, tracking, track, stabilize, stabilise, stabilizing, node-kinds, comp-nodes, keying-deluxe, compositor
tools: comp, keying, remove_object, clean_plate, sky, wire, paint, depth_of_field, grain_match, lens, motion_blur, matchmove, place_on_ground, export_camera, import_camera, track, stabilize, smooth_motion, reframe, shadow, light_match, match_colour, frame, check, polygon_mask, parts_check
---
**Order**
1. Decide layers or nodes. Titles, overlays and grades belong in the layer stack; a shot with a key, a clean-up, a sky, CG or several passes belongs in a node graph (`comp`), where every step can be viewed alone (`output`).
2. Stabilise first when the plate shakes (`stabilize`: `smooth` keeps an intended move, `lock` removes all motion; keep `max_zoom` near 1.1), then track, then animate on top.
3. Track what must stay on something that moves (`track`: a point with detail, `radius` about the object's size, a plane of four corners for a flat surface seen in perspective with `follow: "corners"`). Read `confidence` and `weak_at`; `fix` at the moment it slips.
4. Remove things: `remove_object` uses what is really behind in other frames; `clean_plate` for a static background; `wire` for wires; `paint` only on frames that need it; fill a little beyond the object (`grow`) and restore grain.
5. Insert things: key or cut out (skill green-screen-mattes), then match the plate: black level and colour (`match_colour`), grain (`grain_match`), edge softness, motion blur (`motion_blur`), lens (`lens`: undistort, comp, distort), depth (`depth_of_field`), contact shadow and `light_match`.
6. 3D objects in a moving shot: `matchmove` needs parallax (a dolly, crane or handheld move); mask moving people out of the solve, keep the error under a pixel, `place_on_ground`.
7. A new sky changes the light: relight the foreground toward it, add haze at the horizon, keep the sun on the same side as the plate's shadows (`sky`).
8. Check at 100 percent and in motion, scrubbing through.

**Checks**
- A comp is finished when nobody can find it: edges, light, grain, motion and lens all match the plate.
- The track holds through the whole shot, including the ends; a clean-up does not boil or slide.
- Contact shadows exist; nothing floats.

**Mistakes**
- Stabilising a tripod shot or footage whose shake is the point.
- Tracking a still shot or a subject that leaves the frame for good.
- One key pass for both core and hair.
- A cut-out sharp against a uniformly blurred background; let blur grow with distance.

## Reference: Visual effects

The layer stack stays the way to do simple work. For a shot that needs
real compositing (a key with a solid core and fine hair, an object
painted out, a new sky, CG that must sit in the plate) give the clip a
**node graph**: `comp`. The graph replaces the clip's picture with its
output; on a comp layer (`"shot": true`, an adjustment clip above
everything) it works on the whole shot below. People see the same graph
in the Compositor (⇧C in the timeline).

    {"op": "comp", "clip": 7, "add": [
      {"name": "key", "kind": "keyer", "params": {"screen": "Green", "core_shrink": 2}},
      {"name": "bg", "kind": "read", "in": [], "data": {"path": "/plates/street.mp4"}, "params": {"fit": "Fill"}},
      {"name": "wrap", "kind": "light_wrap", "in": ["key", "bg"], "params": {"intensity": 0.5}}]}

- `add` nodes (each `kind`, optional `name`, `in` the inputs by name or
  id — `"input"` is the clip's own picture — `params`, `data`, `keys`,
  `mix`, `mask`); a node without `in` is fed by the current output, and
  the last node added becomes the output. `set` changes nodes
  (`{"node": "key", "params": {…}}`), `remove` takes them out (what read
  them reads their input instead), `output` picks the node shown,
  `reset` starts over, `delete` takes the graph off the clip.
- Parameters: numbers, true/false, option names, colours `"#rrggbb"`;
  point parameters are fractions of the picture (0..1) or canvas pixels
  from the centre. `keys` animate them: `{"gain": [[1.0, 1], [2.5, 3]]}`
  (timeline seconds). Shapes, strokes and wires in `data` are canvas
  pixels from the centre (at `at`), turned into the clip's picture.
- Every node has `mix` (0 = its input untouched) and `mask` (a node
  whose alpha limits it). Channels travel with the picture: RGBA, depth
  (`set_depth` from a depth pass), motion (`motion_vectors`) and object
  ids (`id_from` mattes). Results are cached by content, and the
  per-pixel nodes run on the GPU.
- The node kinds with their parameters: guide topic `nodes`. Inputs
  (`input`, `read` files and other clips' footage, `constant`, `ramp`,
  `noise`), merge (`merge` over/under/plus/screen/in/out/atop/mask/
  stencil…, `keymix`, `switch`), transform (`transform` with motion blur
  from its keys, `corner_pin`, `crop`, `track` a 2D track), colour
  (`grade`, `colour_match` to a plate, `invert`, `clamp`), filters
  (`blur`, `median`, `effect` any effect from the browser), keying
  (`keyer`, `despill`, `light_wrap`, `edge_extend`), mattes (`roto`,
  `erode`, `matte_refine`, `luma_matte`, `depth_matte`, `id_matte`,
  `copy_alpha`), channels (`shuffle`, `set_depth`, `id_from`,
  `motion_vectors`, `premult`), lens (`lens_distort`, `zdefocus`,
  `vector_blur`, `grain`, `grain_match`), cleanup (`inpaint`,
  `temporal_fill`, `clean_plate`, `wire_remove`, `paint`,
  `sky_replace`).

Ready-made graphs (each adds its nodes after what the clip's graph
already does, and answers with the whole graph):

- `keying` `{clip, screen: "green"|"blue"|"#rrggbb", background: clip
  id or file, clean_plate: "auto" or a file, garbage: [[x, y], …],
  light_wrap, core, edge_band, spill, despill, restore, fix_edges}`: a
  colour-difference key with a solid core matte and a detailed edge
  matte, the screen subtracted from the edges (hair, motion blur), spill
  removed with the brightness kept. With `background` it comps over it
  with light wrap; without, the clip turns transparent there.
- `remove_object` `{clip, box: [[x0, y0], [x1, y1]] | outline: [[x,
  y], …] | the clip's cut-out, method: "auto"|"inpaint", range, camera:
  "locked"}`: paints the object out with the background nearby frames
  show (aligned to this one), inpainting what no frame shows. Cut a
  moving subject out first (`cut_out`) and remove_object uses its
  matte. Nothing here is a learned model: the fill is classical.
- `clean_plate` `{clip, samples, camera: "moving"}`: bakes the
  background alone (median of frames across the clip) to a PNG; use it
  as keying's clean plate or in a `read`.
- `sky` `{clip, sky: file|clip, matte: "cut_out", relight, haze, wrap,
  edge}`: finds the sky, rebuilds its edge through leaves and hair,
  puts the new sky in and relights the foreground towards it.
- `wire` `{clip, from, to | points, width, keys}`: wire and rig removal
  from the picture either side, keeping texture.
- `paint` `{clip, strokes: [{tool: colour|clone|reveal|erase|blur,
  points, radius, softness, opacity, colour, offset, frame_offset,
  from, to}]}`: strokes that last from `from` to `to` (timeline
  seconds); clone from an `offset` or another frame.
- `depth_of_field` `{clip, focus: [x, y] | distance, size, range}`:
  bakes an AI depth pass of the clip and blurs by depth.
- `grain_match` `{clip, reference}`, `lens` `{clip, k1, k2, chroma,
  mode}`, `motion_blur` `{clip, shutter}`.

**Matchmove** (3D camera tracking): `matchmove` `{clip, focal_mm,
sensor_mm, nodal, ignore: "cut_out", export}` follows a few hundred
corners through a moving shot, solves the camera (lens too when not
given) and adds a camera clip keyed every frame above the footage; 3D
layers, models and text then stick to the scene. It reports the error
in pixels (under 1 is good) and says when the shot is a pan from one
spot (nodal). `place_on_ground` `{clip, point: [x, y], at, lift, lie}`
stands a 3D layer or model where that screen point meets the solved
ground. `export_camera` `{clip, path: .chan|.json|.obj, units}` writes the camera for other 3D and compositing tools (`.chan`: Y up, rotation ZXY,
vertical field of view) or the point cloud; `import_camera` `{path,
at, units}` brings a camera in.

## Reference: Visual effects: compositing, matchmove and cleanup

A visual effect is finished when nobody can find it. Judge every comp
by the plate: edges, light, grain, motion and lens must all match it.

- **Layers or nodes.** Titles, simple overlays and grades belong in the
  layer stack. A shot with a key, a clean-up, a sky, CG or several
  passes belongs in a node graph (`comp`): the order of operations is
  visible and every step can be checked on its own (`output`).
- **Keying.** Pull the key in two parts: a solid core matte (shrunk
  in, softened) and an edge matte that keeps hair and motion blur;
  never one key trying to do both. Subtract the screen from the edges,
  suppress spill but restore its brightness, and look at the matte, not
  the comp (`output: "Matte"`, then `"Status"`). Garbage-matte what is
  far from the subject. Uneven screens key better against a clean
  plate. Then light wrap: the background's light bleeding a few pixels
  onto the subject is what makes it sit in the shot.
- **Matching.** A comp lies until its black level, contrast and colour
  match the plate (`colour_match` to the plate), its grain matches
  (`grain_match`), its edges are as soft as the plate's focus, its
  motion is as blurred as the plate's (`vector_blur`), and it is
  distorted by the same lens (`lens_distort`: undistort the plate,
  comp, distort again). Check at 100 % and in motion.
- **Depth.** Depth of field and haze come from distance. A depth pass
  (`depth_of_field`) puts the blur where a lens would; keep the subject
  in focus and let the background fall off gently, never a cut-out
  sharp against a uniform blur.
- **Matchmove.** A camera solve needs parallax: a dolly, crane or
  handheld move. A pan from one spot solves nodal (things stick at any
  depth but cannot be walked around). Mask moving people out of the
  solve (`ignore: "cut_out"`), keep the error under a pixel, and place
  objects on the ground the solve found (`place_on_ground`); a floating
  object is the classic tell. Give inserted objects a contact shadow.
- **Cleanup.** Remove with what is really behind: the same place in
  other frames (`remove_object`), then texture synthesis for what no
  frame shows. Fill a little beyond the object (`grow`) so its edge
  goes too, restore grain, and scrub through: a cleanup that boils or
  slides is worse than the object. Wires come out from the picture
  either side; paint strokes live only on the frames that need them.
- **Sky.** A new sky changes the light: relight the foreground toward
  it, add haze at the horizon, and wrap its light into the edges; keep
  the sun on the same side as the shadows in the plate.

## Reference: Tracking

**Tracking** (motion tracking on footage): `track` follows a point
(`point`: `[x, y]` canvas pixels at `at`, or `"face"`; `radius`: how
much picture around it moves with it) or a flat surface (`plane`: four
corners in order around it) through the whole clip, forwards and
backwards from `at`, and keeps the track on the clip with a confidence
per frame. `attach` (another clip) with `follow` makes that layer
follow it: `position` (moves with it), `transform` (also turns and
scales), `corners` (its corner pin lands on a plane's corners: screen
and sign inserts) or `mask` (`mask` 1…: one of its masks moves with it,
to blur or brighten a moving thing). The answer gives the track's
`confidence`, the moments it was weakest (`weak_at`) and where it is at
five moments: look there with `frame`. When it slipped, `fix` puts it
right at `at` (`[x, y]`, or four corners) and re-tracks from there; the
followers move with it. `retrack: true` re-tracks from `at` as it is.

    {"op": "track", "clip": 4, "point": [310, -120], "at": 2.0, "radius": 50, "name": "logo", "attach": "$1", "follow": "position"}
    {"op": "track", "clip": 4, "plane": [[-300, -200], [280, -180], [290, 160], [-310, 150]], "at": 1.0, "attach": 9, "follow": "corners"}
    {"op": "track", "clip": 4, "track": 1, "fix": [325, -110], "at": 3.4}

## Reference: Stabilizing

**Stabilizing**: `stabilize` measures the camera's shake and takes it
out with keys on the clip's position, rotation and scale: `mode`
`smooth` (keeps the intended move; `smoothness` seconds, default 1) or
`lock` (no camera motion at all). It zooms just enough to hide the
moving edges, up to `max_zoom` (default 1.15; false never zooms) and
eases the correction rather than zoom further. `rotation: false` leaves
the turn alone; `track` (a tracked point on the clip) holds that
subject still instead of the background; `remove: true` undoes it. The
answer says how much shake there was before and after.

    {"op": "stabilize", "clip": 4, "mode": "smooth", "smoothness": 1.5}
