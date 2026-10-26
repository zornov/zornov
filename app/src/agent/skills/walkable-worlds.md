---
name: walkable-worlds
use: Use when filming inside an enterable 3D place you build and walk through.
topics: enterable, walkable, walkable-worlds, world-building, level, scouting, interiors, interior, games, game-worlds, walk-through, first-person, locations
tools: build_world, edit_world, world_info, walk, stand, world, camera, shot, lighting, light, render3d, asset, model, frame, contact_sheet, check, review, notes
---
**Order**
1. Make one reference picture of the place at the hour you want (`generate`, a mood board) and write the place in `notes`: what it is, its size, the hour, where the camera will go. The picture is never pasted in as a backdrop; every tree, wall and rock is a model, so the camera can move anywhere.
2. `build_world` with a `description` or a `preset` (forest, meadow, desert, mountains, island, canyon, arctic, village, city, interior, scifi) and the `reference`. Units are metres, y up. Fill it in layers: ground, big shapes (hills, a lake, buildings), mid shapes (trees, rocks), small shapes (bushes, props), then light and fog.
3. Change it with `edit_world` (lists are added to; `list: true` shows what has numbers) instead of rebuilding. Real extra objects: `asset` and `model` (skill 3d-assets-scenes).
4. Scout with `world_info`: bounds, objects, ground heights, whether a spot is under water, and `path` (can a person walk from here to there, and how far). Ask before you route.
5. Shoot: `walk` a `route` at `speed` m/s (a stroll 1.2-1.6, a run 3-4; `smooth` 1 steady, 0.4 handheld urgency) with `pause` where something happens; the camera eye is recorded and keyed as a ready shot, `look_at` keeps a subject in view. `stand` places a camera like a person standing there. Characters in the world act by their `clip`.
6. Light for the hour (`sky` time, `lights` at night, a keyframed `generator.time_of_day` to run a day or a sunset), then `render3d {mode: "path"}` for finals. Cut between positions with `stand`, keeping the 180-degree line across the action.
7. Frame the first and last point of each route, then continue with the normal edit.

**Checks**
- The camera is never inside a hill or behind a wall: check with `world_info` ground heights and `path`.
- Scale reads right: a person 1.75 m, a door 2.2 m, a storey 2.8 m.
- A forest has more than trees (rock, deadwood, clearings with `near` and `radius`, fog).

**Mistakes**
- A billboard or pasted picture standing in for the main objects.
- Rebuilding the whole world for a small change.
- Routing without asking `world_info`, then filming a walk that goes through a wall.
- Skipping light: a world at the default hour looks flat.
- Expecting project lights and weather to reach a world: a world keeps its own sun, sky and lamps. It takes only the fog (density, thinning, colour) and the exposure, bloom, saturation and vignette of the scene's air and look clip (`atmosphere`, see `lighting-mood`); rain, snow, wet reflections, glow and flames are not drawn in it (a path-traced render of it draws them).

## Reference: Enterable worlds: build, explore, film

For scenes you move through (a forest path, a street, a flat, a base on
another planet) build a **walkable world**: real 3D with depth, not flat
cards and never billboards for the main things. Terrain is a sculpted
heightfield painted in layers; trees, rocks, grass and flowers are
scattered as thousands of instanced models with levels of detail; buildings
and rooms are made from modular kits with real walls, doors, windows,
floors and stairs you can climb; water reflects; the sky follows a time of
day. The same world is shown live in the 3D workspace (walk, fly or orbit
it), seen by the project's cameras, and path traced for finals. Units are
metres (the project's pixels are 100 to the metre, y down; the world ops
take metres, y up).

- `build_world`: `description` (free words: "a misty pine forest at dawn
  with a cabin", "a rainy-night city street", "a sci-fi hab on a dune
  planet", "a small apartment") or `preset` (forest, meadow, desert,
  mountains, island, canyon, arctic, village, city, interior, scifi), then
  anything to override: `size` (metres across), `seed`, `quality` ("high"
  uses the modeled tree and rock kits, "fast" low-poly ones), `terrain`
  (`{"style": hills, plains, mountains, dunes, canyon, island, valley,
  flat, "height": metres of relief}`), `sky` (`{"time": 17.5 | "dawn" |
  "golden_hour" | "dusk" | "night", "turbidity", "clouds" 0-1}`), `fog`,
  `water` (`{"kind": "lake", "at": [x, z], "radius"}` or `"sea"` or
  `"river"`), `scatter` (`[{"kind": pine, tree, palm, bush, rock, grass,
  flower, cactus, dead_tree, "count", "size", "on": "grass", "max_slope",
  "clumping", "near": [x, z], "radius"}]`), `buildings` (`[{"kind":
  house, apartment, tower, shop, warehouse, room, hall, hab, "at": [x,
  z], "yaw", "width", "depth", "floors", "furnish": ["table", "chair",
  "sofa", "bed", "shelf", "desk"]}]`), `roads` (`[{"from": [x, z], "to":
  [x, z], "width"}]`), `props` (`[{"kind": crate, barrel, table, chair,
  column, stairs, lamp, fence, car, sofa, bed, cactus, or a model file,
  "at": [x, z], "yaw", "size", "count", "spread", "lift"}]`), `characters`
  (`[{"name", "at": [x, z], "yaw", "height", "clip": idle, walk, run,
  wave, talk, dance, "walk_to": [[x, z], ...], "top": "#336699"}]`: they
  act and walk across the shot), `lights` (`[{"at": [x, z], "height",
  "colour", "intensity", "range"}]`), `sculpt` (`[{"brush": raise, lower,
  smooth, flatten, level, noise, "at": [x, z], "radius", "strength"}]`),
  `paint` (`[{"layer": grass, rock, dirt, snow, "at": [x, z], "radius"}]`),
  `spawn` ([x, y, z]), `without` (parts of the preset to leave out:
  scatter, buildings, props, characters, lights, roads, water), `look` (`{"exposure", "bloom", "foliage":
  "#rrggbb", "ground", "fog_colour"}`). It adds the world as a clip and,
  if the project has no camera, one at the start point at eye height.
- **Choose the look from a picture.** Generate or find a reference image of
  how the place should feel (`generate`), then pass it as `reference` (a file or the media id of a picture in the
  project): its sky, haze, ground and foliage colours and its light set the hour,
  fog and colours of real geometry. The picture itself is never placed in
  the world; the main objects are always models.
- `edit_world`: the lists above are *added to* (the preset's own trees, houses and so on are
  numbered items too: `list: true` shows them); `sky` (key by key),
  `water`, `terrain`, `fog`, `look`, `spawn` replace; `remove`
  (`{"props": [2, 3], "scatter": "all"}`; `list: true` shows what has
  numbers); `time_of_day` (hours or a name) sets the layer's own hour, which
  you can keyframe as `generator.time_of_day` to run a day or a sunset
  across a shot.
- `world_info`: the start point, bounds, objects with positions, water
  levels, the hour; `ground` (`[[x, z], ...]`) gives ground heights and
  whether a spot is under water; `path` (`{"from": [x, z], "to": [x,
  z]}`) says whether a person can walk from here to there and how far
  (through doors, up stairs, round walls and ponds). Ask before you route.
- `walk`: a person walks `route` (`[[x, z], {"at": [x, z], "pause": 2}]`)
  from `from` (or the start point) at `speed` m/s (`sprint`), round
  walls, through doorways and up stairs; the camera eye is recorded at
  `eye` height (1.65) with `fov`, smoothed (`smooth` 0 raw, 1 normal, 2
  steadier) and keyed into `camera` (or the project's) from `start`
  seconds: a ready timeline shot. `look_at` ([x, y, z]) keeps a subject in
  view while walking instead of looking ahead.
- `stand`: place a camera like a person standing there: `at` [x, z], `eye`
  (1.7), `look_at` ([x, y, z] or [x, z]) or `yaw`/`pitch`, `fov`, `t`.
  Walk to find the spots, `stand` to frame them; `camera` `move` and the
  usual 3D camera ops then work on it.
- Films: `render3d` `mode` path for the final of a world shot; characters
  in the world act by their `clip`; keyframe `generator.time_of_day`; use
  `light`, `shot` and the cinematography chapter's grammar on the world's
  cameras. Every frame comes from real geometry, so parallax, occlusion
  and depth of field are true from any position.

    {"op": "build_world", "description": "a misty pine forest at dawn with a cabin", "size": 300, "reference": "/tmp/forest_mood.png", "buildings": [{"kind": "house", "at": [20, 10], "floors": 1}], "characters": [{"name": "Hiker", "at": [0, 5], "clip": "walk", "walk_to": [[18, 8]]}]}
    {"op": "world_info", "ground": [[0, 0], [20, 10]], "path": {"from": [0, 5], "to": [22, 14]}}
    {"op": "walk", "route": [[0, 5], [12, 7], {"at": [22, 14], "pause": 1}], "speed": 1.6, "start": 0, "look_at": [20, 2, 10]}
    {"op": "stand", "at": [6, 3], "look_at": [20, 1.5, 10], "t": 8}
    {"op": "edit_world", "sky": {"time": "dusk"}, "props": [{"kind": "lamp", "at": [8, 8]}]}

## Reference: Locations to shoot in: enterable worlds

A walkable world is a location you can shoot from anywhere, so scout it
like one.
- **Reference first, geometry always.** Make one reference image of the
  place at the hour you want (the mood board), pass it as `reference`, and
  let the build take its colours and light. Never paste the picture in as
  a backdrop: every tree, wall and rock is a model, so the camera can move.
- **Fill it in layers.** Ground (terrain style and relief), big shapes
  (hills, a lake, buildings), mid shapes (trees, rocks), small shapes
  (bushes, grass, flowers, props), then light. A forest with only trees
  looks like a tree farm; add rock, deadwood, clearings with `near` and
  `radius`, and fog.
- **Scale by people.** A person is 1.75 m, a door 2.2 m, a storey 2.8 m.
  Keep buildings and props to those proportions and the world reads right.
- **Scout with `world_info`.** Ask for ground heights and walkability
  before placing cameras and routes; a camera inside a hill or behind a
  wall is the commonest failure.
- **Move like a person.** Walk at 1.2-1.6 m/s for a stroll (`speed`), 3-4
  for a run; keep `smooth` at 1 for steady-cam, 0.4 for handheld urgency.
  Hold a `pause` at the points where something happens. Looking ahead is
  natural; `look_at` a subject for a following shot.
- **Cut between positions with `stand`**, and keep the 180-degree line
  across the action as with any other set.
- **Light tells time.** Dawn and golden hour for warmth and long shadows,
  dusk and night for lit windows and lamps (add `lights`). Run the day with
  a keyframed `generator.time_of_day`.
