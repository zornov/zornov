---
name: 3d-assets-scenes
use: Use when the film needs real 3D: models, assets, scenes, cameras, lights, renders.
topics: 3d, three-d, models, model, mesh, meshes, modeling, modelling, 3d-models, asset, assets-3d, sculpting, sculpt, character-3d, face, faces, head, anatomy, rigging-3d, 3d-animation, animation-3d, simulation-3d, cloth, hair, fur, smoke, fire, particles-3d, destruction, shatter, water, ocean, soft-body, worlds, world, environment, sky, terrain, landscape, path-tracing, render, rendering, passes, hdri, space, camera, cameras, lights, lighting, parallax, perspective, lens, 3d-workspace, viewport, hero-objects, critic, character, characters, environments, heads, lenses, lip-sync, mocap, motion-capture, particles, poses, posing, rigging, rigs, visemes, walk, walk-cycle, wardrobe, clothes, clothing, outfit, props-in-hand, hair-3d, math, maths, mathematics, math-shapes, formula-shapes, implicit, implicit-surface, sdf, gyroid, tpms, lattice, lattices, minimal-surface, parametric, parametric-surface, fractal, fractals, mandelbulb, menger, knot, knots, torus-knot, seashell, mobius, klein, height-field, heightfield, l-system, lsystem, phyllotaxis, golden-angle, attractor, lorenz, epicycles, fourier, superformula, warp, deform, twist, bend, taper, spirograph, rose-curve, lissajous
tools: search_media, model, mesh, asset, build_place, combine, uncombine, generate, three_d, camera, light, lighting, parallax, shot, rig, pose, act, simulate, world, render3d, previs, compare_cameras, animatic, viewport, frame, contact_sheet, check, review, notes, animate, body, settle, wear, hold
---
**Search before modelling.** Look for CC0 models, textures and skies first: `search_media {query, kind: "3d" | "texture" | "hdri"}` (Poly Haven), then fetch what fits and change it with `mesh`. Model from scratch only when nothing fits. Photos found with `search_media {kind: image}` (a real side view and front view) are the references for `generate {task: "model3d", images}` or for building by hand. Say what was found and what was modelled or made.

**Order**
0. A 3D person or creature: `model` `shape: "character"` (`preset`, `age`, `style`, `figure`; `dressed: false` to dress it yourself), then `wear` clothes, hats, shoes, hair and beards (each a separate named part over a full body; `remove` or swap any), `hold` props in its hands (the fingers close, a flame lights the scene). `guide {"topic": "wardrobe"}` has the how-to.
1. Anything with volume (an object, product, vehicle, character, creature, logo, building) is built as a real 3D model. Never fake volume with stacked cards or text switched to `three_d`.
2. Write `notes` first: `Parts: body, lid, hinge, light strip, ...`. The 3D critic (run by `check` and `review`) reads the request and your notes for the parts the subject has, and `review` stays NOT DONE while any `3d_` note is open.
3. Look in the person's kept models before building: `catalogue {panel: "assets", query}`; if one fits, `add` it by `item` and change it with `mesh`.
4. Reference: `generate` front, side and three-quarter views (plain or transparent background, named "<object> reference front"). The critic compares your model's outline with them.
5. A place (building, facade, street, interior, bridge, fence, stairs) is ONE model: `build_place {kind, style, wear}` in classical, art_deco, brutalist, modern_glass, medieval, oriental, futuristic or rustic; many clips already placed become one with `combine {clips | like | all}`, and `uncombine` takes parts back out. Never a cube per pilaster (`3d_many_primitives`).
6. Make the form. For ANY single object (an animal, a person, a vehicle, furniture, a building, food, a plant, a product or device, a toy, a sculpture, a stylised or cartoon thing): `generate` ONE clean picture of it (the whole object, a three-quarter view from slightly above, one subject on a plain background or `transparent: true`, evenly lit, no cast shadow, nothing else in frame), then `generate {task: "model3d", images: [that picture]}`. A real image-to-3D model makes the textured mesh on this Mac as a background job (about a minute): the call returns a job; `generate {job, wait: true}` waits and the model lands as a clip, kept in My Models. If the result is bad, fix the PICTURE (regenerate it clearer, whole, plain background, even light) and run it again; at most 2 attempts, then keep the better one and say what is off. Never carve again. A raw file: `{task: "model3d", file, class}`. From parts: the `asset` op for hard-surface things built of distinct parts: named parts (prim, size, at, bevel, subdivide, modifiers, material, bone) or a `kit`. Blended `model` `parts` and `mesh` steps (subdivide, sculpt, stroke, bevel, material, texture) refine any model afterwards.
7. Solid: every model is closed with thickness; `mesh` step `solid` closes holes and thickens open sheets (`3d_open`, `3d_flat`). Physics (falling, resting, colliding, breaking: skill `physics-3d`) weighs the real volume.
8. Edges and detail: bevel about 1 percent of the size with subdivide 1 on each part; detail last (boolean for vents, mirror, array).
9. Moving parts: a `bone` with its hinge pivot per part, then `pose` and key the bone. A state change is a pose on the same model clip, never a second clip swapped in.
10. Materials per part (layered names such as painted_metal, glass, wood, skin, or colour/metallic/roughness/transmission); `emissive` for anything that glows.
11. Light and lens: `lighting` three-point with `shadows` on the key; `camera` with a slow `move` and `dof` on the model; `render3d {mode: "path"}` for finals. For mood, glowing lamps and flames, fog, rain and wet surfaces load `lighting-mood`. Characters: `rig` then `act`/`pose`; cloth, smoke, destruction: `simulate`; landscapes and skies: `world`.
12. Compare against the references from front, side and three-quarter views; fix proportions first, then run `check` and `review` and fix every `3d_` note.

**Checks**
- `frame` from several sides after every few mesh steps, not once at the end.
- Every part the subject has exists and is named for what it is.
- Notes the request really asks to set aside (a blocky low-poly look) are written as `3d-ok: kind - reason` in `notes`, and the final answer lists them.

**Mistakes**
- Stacking primitives without bevel, with the stock grey material, flat light and a static camera: the critic names each (`3d_no_bevel`, `3d_default_material`, `3d_flat_light`, `3d_no_camera_move`).
- Sculpting fine detail on a coarse surface: block big forms first, subdivide, then details.
- A second model clip standing in for a state change (`3d_state_swap`).
- Judging only one angle; a face needs the front and the side (`rotation_y` 90).

## Reference: 3D models

`model` adds a real 3D model: triangles with materials, seen through the
camera and lit by the lights like a 3D layer (with no lights in the
scene it gets a key, fill and rim light of its own), with a depth buffer
so models hide each other where they overlap. Either a shape to work
from (`shape`: sphere, ico_sphere, cube, cylinder, cone, torus,
capsule, plane, or `head`: an anatomical human head and neck with
eyes, lips and ears, `size` px from chin to crown, to sculpt into the
person you want; `size` px or `[x, y, z]`; `detail` 1-6, default 4), or
blended `parts` (below) or a file
(`file`: .glb, .gltf, .obj, .fbx, .stl, .ply, a name from the project's Downloads, or a web link, which is downloaded first (a github.com blob link works); `size` fits its largest side. `download` and `import` of a model file add it the same way. The
clip's 3D transform places it (`position` `[x, y, z]`, `rotation`
`[x, y, z]` degrees, `scale`, and every 3D property `set` / `animate`
takes: turn it with `rotation_y` keys). `colour`, `metallic` (0-1),
`roughness` (0 mirror – 1 matte), `emissive`, `opacity`, `subsurface`
(0-1: light under the surface, for skin, wax, marble, leaves) and
`smooth` (0 faceted – 1 smooth shading) are properties too; on a model
with several materials `colour` changes them all (use the `material`
step with `index` for one part).

**Blended parts** (`model` with `parts`) build organic forms the way
clay is pushed together: each part melts into the ones before it over
`blend` px, or carves them (`subtract: true`). Parts: `shape`
(ellipsoid, box, capsule, cylinder, torus), `at` `[x, y, z]`, `size`
(px or `[x, y, z]`), `turn` `[x, y, z]`, `round` (box edges), capsules
`from` / `to` / `radius`, and their own `colour`, `roughness`,
`metallic`, `subsurface`. A torso is a few ellipsoids blended 40 px; a
limb a capsule; a mug a cylinder with a smaller one subtracted and a
torus handle; a creature's eye socket a subtracted ellipsoid. A part
can also be a formula (`math`: an implicit solid or preset such as
`gyroid`, `blob`, `mandelbulb`) that melts, carves or cuts like the
rest. Then sculpt the result with `mesh`. `model` with `clip`
and `export` writes .glb or .obj.

**Shapes from mathematics** (`model` with `math`): lattices and minimal
surfaces, formula solids, fractals, parametric surfaces (shells, Möbius
strips, Klein bottles, vases) with thickness and cut-out patterns,
knotted tubes, height fields, L-system plants, phyllotaxis and
attractor trails, presets or formulas of your own; shapes that move with
`t` are baked to play. `skill {"name": "3d-assets-scenes", "part": "math shapes"}` has every field and preset.

`mesh` changes a model's shape and surface: `{"op": "mesh", "clip": 5,
"steps": [{"do": …}, …]}` (one undo step), or one step's fields at the
top. Points are in the model's own pixels before its layer's transform,
centred on the model: x right, y down, z away (the face looking at the
camera is at negative z). The result lists the box (`box_min`,
`box_max`), so you always know where the surface is.

| step | fields | does |
|---|---|---|
| `subdivide` | `levels` (1-4) | every triangle becomes four and the surface rounds off; do it before sculpting detail (a level-5 ico_sphere is 20 000 triangles; each level ×4) |
| `sculpt` | `brush`, `at` `[x, y, z]`, `radius` px, `strength` (px; 0-1 for smooth / flatten), `by` (grab), `symmetry` (default on: mirrored across x = 0) | `inflate` pushes out (negative: in), `grab` moves by `by`, `smooth`, `flatten`, `pinch` (sharpens), `crease` (a groove; negative a ridge) |
| `stroke` | as sculpt with `path` (a list of points) | the brush dragged along a line: brows, lips, eyelids, wrinkles |
| `bevel` | `width` px (default 1.2% of the size), `segments` (1-8, default 3), `angle` (edges bending more than this, default 50), `profile` (0 flat chamfer - 1) | rounds every sharp edge so it catches light; do it on the closed shapes, before `subdivide` |
| `smooth` | `iterations`, `strength`, `region` | evens out the whole surface or a region without shrinking it |
| `symmetrize` | `keep` (right, left) | mirrors one half onto the other |
| `extrude` | `region`, `distance` px | pushes faces out with new sides (limbs, a nose from a box) |
| `displace` | `amount` px, `size` px, `octaves`, `seed`, `region` | noise in and out: rock, bark, skin texture at small sizes |
| `twist` / `bend` / `taper` | `axis` (x, y up-down, z), `degrees` (twist, bend), `toward` (bend), `from` / `to` (taper scales), `refine` | space warps of the whole model; triangles, materials and rig kept; a coarse model is split first (part `math shapes`) |
| `warp` | `math` (`"[dx, dy, dz]"`, formula space, the half size as 1) or `amount` + `size` + `seed` (noise) | every point moved by a formula or by smooth noise through the form |
| `repeat` | `count` (`[x, y, z]`, or with `around: true` / `radius` copies round `axis`), `spacing` | copies in a grid or a ring |
| `move` | `by`, `turn` `[x, y, z]` deg, `scale`, `pivot`, `region` | moves part (or all) of the model |
| `add` | `shape`, `size`, `detail`, `at`, `turn`, material fields | another shape into the same model (eyes, buttons, handles) |
| `boolean` | `mode` (union, subtract, intersect), `with` (a shape `{"shape", "size", "at"}` or a model clip) | cuts and joins (a hole, a socket, a bevelled edge) |
| `material` | `colour`, `metallic`, `roughness`, `emissive`, `opacity`, `subsurface`, `name`; with `region`: a new material on that part; with `index`: that material | the surface; per part: lips, irises, nails |
| `texture` | `image` (PNG/JPEG), `projection` (box, sphere, cylinder, planar), `flip` (v, u), `index` | a picture on the surface (generate one with `generate` first: skin, wood, labels); a model's own texture file that sits upside down takes `flip: "v"` |
| `uv` | `projection`, or `flip` (v, u, both) alone | texture coordinates again, or the texture turned over |
| `decimate` | `triangles` | fewer triangles, same shape (heavy imports) |
| `normals` | `angle` (degrees) | sharp edges where faces meet at more than `angle` |
| `delete` | `region` | removes faces |
| `mirror`, `array`, `fit` | `axis`; `count` + `offset`; `size` | a mirrored copy; copies in a row; centre and size |

A `region` is a word (`all`, `top`, `bottom`, `left`, `right`,
`front`, `back`: faces looking that way) or `{"sphere": {"at": [x, y,
z], "radius": r}}`, `{"box": {"min": [..], "max": [..]}}`, `{"side":
{"axis": "x", "above": 0}}`, `{"facing": {"direction": [x, y, z],
"degrees": 45}}`, `{"material": n}`.

**Hand modelling** (the same operations a person has in the 3D
workspace's edit mode, on the model's polygon cage; the model stays
editable by hand afterwards): give a step a `select`, or the op
`"edit": true`, and `do` any operation below. The pick carries from step
to step. `select`: `mode` (vertex, edge, face), `faces` (top, bottom,
left, right, front, back, all; face numbers; `{"facing": [x, y, z],
"degrees"}`, `{"box": {"min", "max"}}`, `{"side": {"axis", "above"}}`,
`{"material": n}`, `{"nearest": [x, y, z]}`, plus `"grow": n`), `edges`
(sharp, boundary, seams, creased, all; `[[a, b], …]`; `{"sharper": deg}`,
`{"loop": [x, y, z]}`, `{"ring": [x, y, z]}`, `{"outline": <faces>}`),
`verts` (all, numbers, `{"near": [x, y, z], "radius"}`, `{"box"}`), `add:
true` to keep the earlier pick. Lengths are model px; defaults suit the
model's size. `{"do": "operations"}` lists every operation with its
settings; `{"do": "describe"}` gives the cage, the box, open edges, the
pick, the modifier stack and the material slots. Models from triangles
(imports, `model` shapes) are joined back into quads first.

| step | fields | does |
|---|---|---|
| `extrude` / `extrude_region` / `extrude_normals` / `extrude_individual` | `distance` px | picked faces pulled out (as a region, along each corner's normal, or each face alone); open edges into a new band |
| `inset` / `poke` / `solidify` | `thickness`, `depth`, `individual`; `offset`; `thickness` | a rim inside the picked faces; a point in each; thickness for a sheet |
| `bevel` / `bevel_vertices` | `width` px, `segments`, `profile` | rounded or chamfered picked edges or corners |
| `loop_cut` / `edge_slide` / `vertex_slide` / `subdivide` | `edge` [a, b] or the picked edge, `cuts`, `slide`; `factor`; `factor`, `toward`; `cuts`, `smooth` | loops across a ring of quads, slid; picked edges split into grids |
| `knife` / `bisect` | `point`, `normal`; `clear` (none, front, back), `fill` | new edges along a plane across the picked faces; cut and clear one side |
| `merge` / `dissolve` / `delete` / `split` / `separate` / `duplicate` | `at` (centre, cursor, first, last, collapse, distance); `what`; `by` (selection, material, loose_parts) | weld, remove keeping the surface, remove, disconnect, into new model clips, copy |
| `connect` / `make_face` / `fill` / `grid_fill` / `bridge` | `segments`, `twist` | split faces between corners; a face from vertices or a picked hole; close holes; join two loops |
| `spin` / `screw` / `mirror` / `symmetrize` | `degrees`, `steps`, `axis`, `cursor` [x, y, z]; `turns`, `height`; `axis`; `axis`, `direction` | lathe a profile, a thread or spring, flip the pick, make the whole symmetric |
| `shrink_fatten` / `to_sphere` / `smooth_vertices` / `randomize` / `selection_to_cursor` | `distance`; `factor`; `iterations`; `amount`, `seed` | move picked vertices along normals, toward a sphere, relax, jitter, onto a point |
| `mark_seam` / `mark_sharp` / `crease` / `mark_sharp_by_angle` | `clear`; `amount` 0-1; `angle`, `crease`, `seam` | texture seams, hard shading, subdivision creases |
| `unwrap` / `project_uvs` / `pack_uvs` / `reset_uvs` / `clear_seams` | `method` (seams, angle, smart), `margin`; `kind` (box, planar, front, cylinder, sphere, view) | texture layouts |
| `recalc_outside` / `recalc_inside` / `flip_normals` / `shade_smooth` / `shade_flat` / `assign_material` | `slot` | which way faces point; smooth or faceted; a material slot on the pick |
| `triangulate` / `tris_to_quads` / `merge_by_distance` / `fill_holes` / `limited_dissolve` / `fix_t_junctions` / `decimate` | `angle`, `distance`, `sides`, `ratio` | clean up |
| `select` / `select_loop` / `select_ring` / `select_linked` / `select_more` / `select_less` / `select_similar` / `select_non_manifold` / `select_sharp` / `select_mirror` / `hide` / `reveal` | the `select` fields; `by`, `threshold`; `axis` | change the pick (and what is hidden) |
| `add_cube` / `add_plane` / `add_grid` / `add_circle` / `add_uv_sphere` / `add_ico_sphere` / `add_quad_sphere` / `add_cylinder` / `add_cone` / `add_torus` / `add_head` / `add_curve` | size settings, `cursor` [x, y, z] | another shape into the same mesh |
| `modifier` | `add` {"kind", fields}, `remove` n, `enable` n + `on`, `move` n + `to` m, `apply` n, `index` n + `set` {fields} | the non-destructive stack: subdivide, mirror, array, radial, bevel, solidify, boolean (`with` a part), displace, smooth, weld, triangulate, decimate, remesh, shrinkwrap, lattice, bend, twist, taper, wave, cast, weighted_normals |
| `material` | `slot`, `colour`, `metallic`, `roughness`, `transmission`, `subsurface`, `emissive`, `smart` (a layered material by name), `name`, `assign` | the slot's material, put on the picked faces |
| `part` | `name` | work on another part of an asset |

    {"op": "mesh", "clip": "Cup", "steps": [{"do": "inset", "select": {"faces": "top"}, "thickness": 12}, {"do": "extrude_region", "distance": -180}, {"do": "bevel", "select": {"edges": "sharp"}, "width": 3, "segments": 2}, {"do": "modifier", "add": {"kind": "subdivide", "levels": 2}}, {"do": "material", "select": {"faces": "all"}, "colour": "#e8e2d6", "roughness": 0.25}]}
    {"op": "mesh", "clip": 7, "edit": true, "steps": [{"do": "loop_cut", "select": {"edges": {"nearest": [100, 0, 0]}}, "cuts": 3}, {"do": "select", "edges": {"loop": [100, 40, 0]}}, {"do": "edge_slide", "factor": 0.3}]}

Sculpt like a sculptor: block the big forms first on a coarse surface
(a sphere stretched to the skull's proportions, grab the jaw and chin,
push in the eye sockets), subdivide, then the secondary forms (nose,
brow, cheekbones, lips), subdivide again, then details with small
brushes and strokes; look (`frame`) after every few steps from the
front and from the side (`rotation_y` 90). Read the craft chapter
`guide {"topic": "sculpting"}` before a character or a face.

    {"op": "model", "shape": "sphere", "size": [300, 380, 320], "detail": 4, "colour": "#d8a48f", "roughness": 0.55, "name": "Head"}
    {"op": "mesh", "clip": "Head", "steps": [{"do": "sculpt", "brush": "grab", "at": [0, 150, -60], "by": [0, 40, -20], "radius": 120}, {"do": "subdivide"}, {"do": "sculpt", "brush": "inflate", "at": [55, -40, -140], "radius": 35, "strength": -14}]}
    {"op": "model", "shape": "head", "size": 420, "detail": 5, "colour": "#c99a80", "name": "Face"}
    {"op": "model", "name": "Mug", "parts": [{"shape": "cylinder", "size": [160, 200, 160], "round": 8}, {"shape": "cylinder", "at": [0, -14, 0], "size": [136, 200, 136], "subtract": true, "blend": 4}, {"shape": "torus", "at": [95, 0, 0], "turn": [90, 0, 0], "size": [110, 22, 110], "blend": 10}], "colour": "#e8e2d6", "roughness": 0.25}
    {"op": "model", "file": "/path/to/chair.glb", "position": [0, 100, 300]}
    {"op": "animate", "clip": "Head", "property": "rotation_y", "keys": [[0, -30], [4, 30]], "ease": "ease_in_out"}

### Hero objects: the recipe

**Look in the person's kept models first.** Every 3D model you make is kept
for every project (Library, Assets, Models), with its parts, materials and
rig. Before modelling a character, building, prop or place, run `catalogue`
with `{"panel": "assets", "query": "castle"}`; if one fits, `add` it by
`item` and change it with `mesh` instead of building it again. Carrying on
with a kept model updates that entry; a different thing gets its own.

A hero object (a product, a vehicle, a character, a piece of furniture, a
building: whatever the film is about) is designed, not stacked. `check`
and `review` run a 3D critic over every model that stays on screen;
`review` stays NOT DONE while any `3d_` note is open, and your final
answer must list the ones you left. The critic reads the request and your
`notes` for the parts the subject has (a car's wheels, windscreen and
headlights; a wardrobe's doors, handles and legs; a creature's wings and
tail; a lid, a button, a strip of light), so **write a line in `notes`
first: `Parts: body, lid, hinge, light strip, earbuds`**, then build every
one. Follow this order:

1. **Reference first.** `generate` the front, side and three-quarter views
   (plain background, or `transparent: true`), named with "reference" and
   the object ("case reference front"). The critic renders your model from
   eight sides and compares its outline with them (`3d_reference_mismatch`
   under 50% overlap). Look at them before you build.
2. **Make the form, one of two ways.**
   - **From a picture (any object).** `generate` with `task: "model3d"` and
     `images` (one picture; with several views the front one is used: a
     name ending in front, or `{"image": …, "side": "front"}`). A picture
     with a plain or transparent background is used as it is; one with a
     background is cut to its subject first. A real image-to-3D model
     reads it and a surface comes out for any kind of object (animals,
     people, vehicles, furniture, buildings, food, plants, products,
     devices, toys, sculptures, stylised and cartoon things). That surface
     is repaired, rebuilt as even quads (`faces`, default 10000), unwrapped
     and its colour baked into a texture (`texture`, default 1024) with a
     PBR material, all on this Mac. It runs as a background job: the reply
     is a job number, `generate {job}` shows progress, `wait: true` waits,
     `cancel: true` stops it, and the model clip lands when it is done.
     What makes a good picture: the whole object, one subject, plain
     background, even light with no hard shadow or glare, a three-quarter
     view from a little above, no hands or props holding it. Thin parts
     (legs, spokes, antennae) come out only as thick as the model can
     resolve; transparent or mirror-like things come out as solid shapes
     with their colour painted on. If the result is bad, change the
     PICTURE (regenerate it, adjust the framing or light) and run it again,
     at most 2 attempts in all, then keep the better and say what is off.
     Do not carve again. `method: "carve"` (silhouettes of front, side and
     back views cut a solid) is only for simple hard-surface shapes the
     views fully describe; it is also the automatic fallback when the model
     cannot run. A raw model file (a scan, a download) is cleaned up the
     same way: `{"task": "model3d", "file": "/path/raw.glb", "class":
     "product"}` (class prop, product, character, creature, vehicle,
     architecture, environment sets the quad budget) goes through repair,
     quad remesh, unwrap and texture transfer until it clears the quality
     bars.
   - **From blended parts (creatures, animals, organic forms).** `model`
     with `parts`: ellipsoids for the masses, `cone` (`from`, `to`,
     `radius`, `radius_to`) for beaks, horns, legs, tails and claws, thin
     ellipsoids (`fan`, `feather`, `comb`, `ear`) for crests and fins, a
     `path` of points for necks and tentacles, each with `blend` and its own
     `colour`. The result is one closed, smooth solid with a material per
     part, ready for `rig`, `mesh` sculpting and physics. A rooster:

         {"op": "model", "name": "Rooster", "detail": 5, "parts": [
           {"shape": "ellipsoid", "at": [0, 0, 0], "size": [260, 220, 180], "colour": "#c46a2b"},
           {"shape": "ellipsoid", "at": [-120, -150, 0], "size": [110, 120, 100], "blend": 40, "colour": "#d9822b"},
           {"shape": "cone", "from": [-170, -160, 0], "to": [-225, -150, 0], "radius": 18, "radius_to": 3, "colour": "#e8b830"},
           {"shape": "comb", "at": [-120, -225, 0], "size": [80, 50, 14], "blend": 10, "colour": "#d0201c"},
           {"shape": "fan", "at": [150, -120, 0], "size": [140, 220, 24], "turn": [0, 0, -25], "blend": 30, "colour": "#1f3a2e"},
           {"shape": "cone", "from": [-20, 90, 30], "to": [-30, 210, 30], "radius": 12, "radius_to": 8, "colour": "#e8b830"},
           {"shape": "cone", "from": [-20, 90, -30], "to": [-30, 210, -30], "radius": 12, "radius_to": 8, "colour": "#e8b830"}]}

   - **From parts.** The `asset` op, for anything hard-surface or made of
     distinct parts. Each part is `{"name", "prim" (box, cylinder, sphere,
     torus, cone, tube, plane), "size", "at", "turn", "bevel" (px),
     "subdivide" (levels), "modifiers" ([{"kind": "mirror"|"array"|
     "radial"|"solidify"|"bevel"|"subdivide"|"boolean"|"bend"|"taper"|
     "twist"|"displace"…}]), "material", "bone"}`. A `kit` (rock, tree,
     pine, terrain, building, crate, barrel, column, stairs, bolt, pipe,
     table, chair) starts from a built asset you add parts to. Name every
     part for what it is ("wheel front left", "windscreen", "door left",
     "light strip"): the part's material and bone carry that name, which
     is how `3d_missing_part` sees it.
   - **Places and architecture: one model, never a cube per pilaster.**
     `build_place {"kind": "street", "style": "art_deco", "wear": 0.3}`
     makes a building, facade, street, interior, bridge, fence or stairs
     as ONE model clip with named parts (walls, window frames, window
     glass, cornice, kerbs, lamp posts...). Styles: classical, art_deco,
     brutalist, modern_glass, medieval, oriental, futuristic, rustic.
     Models already placed as many clips become one with `combine
     {"clips": [ids]}` (or `"like"`, or `"all": true`); `uncombine`
     takes parts back out. To walk through a place use `build_world`.
     `3d_many_primitives` flags eight or more primitive clips forming
     one place.
3. **Edges and detail.** `bevel` 1% of the size with `subdivide` 1 on each
   part (`3d_no_bevel`, `3d_faceted`); detail last: `modifiers` boolean for
   vents and ports, mirror for symmetry, array/radial for repeats;
   `mesh` `sculpt`, `displace` on a generated or imported form.
4. **Moving parts with pivots, in the one model.** Give the part a `bone`:
   `{"head": [x, y, z], "tail": [x, y, z], "parent": "body"}` with the head
   on the hinge line (a lid's back edge, a door's side); every part is
   bound whole to its own bone (the rest to `root`). On a model you made
   another way: `rig` with `bones` and `rigid`: true. Then `pose` the bone
   (`{"t": 8, "bones": {"lid": [-100, 0, 0]}}`) and key it back. A state
   change (case opens, door swings, wheels turn) is a pose on the same
   model clip, never a second model clip standing in (`3d_state_swap`);
   to show the object again later, keep the one clip.
5. **Layered PBR materials.** `material` per part: a smart layered one by
   name (painted_metal, brushed_aluminium, chrome, steel, gold, copper,
   car_paint, plastic, plastic_matte, rubber, leather, fabric, denim,
   velvet, wood, wood_varnished, concrete, stone, marble, ceramic, glass,
   water, gem, skin, scales, leaf, bark, terrain…) with an optional
   `colour`, or `{"colour", "metallic", "roughness", "transmission",
   "emissive"}`. Where the request describes light (a strip, a screen,
   headlights, an LED) that part gets `emissive` in the light's colour
   (`3d_missing_emissive`); the stock grey is `3d_default_material`.
6. **Light and lens.** `lighting` three-point (or a key, a rim and an
   ambient `light`), `shadows` on the key (`3d_default_light`,
   `3d_flat_light`). `camera` with a `move` (slow orbit or push in) and
   `dof` focused on the model with a modest `aperture`
   (`3d_no_camera_move`, `3d_no_dof`). `render3d` `mode: "path"` for finals.
7. **Check against the reference.** `frame` from the front, side and
   three-quarter beside the reference pictures; fix proportions first.
   Then `check` and `review`; fix every `3d_` note.

    {"op": "asset", "name": "Sports car", "duration": 8, "parts": [
      {"name": "body", "size": [400, 100, 180], "bevel": 8, "subdivide": 1, "material": "car_paint", "colour": "#a01018"},
      {"name": "wheel front left", "prim": "cylinder", "size": [60, 30, 60], "at": [130, 55, -90], "turn": [90, 0, 0], "bevel": 2, "material": "rubber"},
      {"name": "windscreen", "size": [90, 50, 150], "at": [60, -70, 0], "bevel": 2, "material": {"colour": "#a8c8d8", "transmission": 1}},
      {"name": "headlight left", "size": [14, 14, 30], "at": [200, -5, -55], "material": {"colour": "#ffffff", "emissive": "#fff4d0"}},
      {"name": "door left", "size": [100, 70, 6], "at": [0, -5, -92], "bevel": 2, "material": "car_paint", "bone": {"head": [50, 0, -92], "tail": [50, -60, -92]}}]}
    {"op": "pose", "clip": "Sports car", "keys": [{"t": 1, "bones": {"door left": [0, 0, 0]}}, {"t": 2, "bones": {"door left": [0, 0, 70]}}]}
    {"tool": "generate", "task": "model3d", "images": ["/p/rooster reference.png"], "size": 400, "name": "Rooster"}
    {"tool": "generate", "job": 1, "wait": true}

If the request itself asks for a flat, low-poly or blocky look, set those
notes aside by writing a line in `notes`: `3d-ok: primitive, no_bevel - the
request asks for a blocky low-poly look`. Only style kinds can be set aside
(primitive, no_bevel, faceted, default_material, default_light, flat_light,
no_camera_move, no_dof, reference_mismatch); missing parts, swaps and
unposed hinges never can. The final answer says what was set aside.

## Reference: 3D animation: rigs, poses and motion

`{"op": "model", "shape": "character", "size": 500}` adds a person ready
to animate: a standing figure in an A-pose with the head base for a face,
dressed (`top`, `trousers`, `shoes`, `skin` colours; `build` slim,
average, athletic, heavy; `shoulders`, `hips`, `head_size`; `face`
false for an egg head), with bones, skin weights, eye bones and face
shape keys already made. Any other model can be rigged with `rig`.

| op | fields | does |
|---|---|---|
| `rig` | `clip`; `template` (humanoid, quadruped, chain with `count` + `start`), or `bones` (`[{"name", "parent", "head", "tail"}]`, model px); `face` (shape keys on a head base); `constraint` (`{"kind": "ik", "bone", "chain", "target", "pole"}`, `look_at`, `limit` with `min`/`max` degrees, `copy_rotation` with `from`); `skinning` (`linear`, or `dual_quaternion`: twisted forearms and bent knees keep their volume instead of pinching); `remove`; `list` | bones fitted to the model and the skin bound automatically (each vertex to the bones it can see, smoothed at the joints); or the rig in words |
| `rig` with `rigid`: true | `bones` as above | each separate piece of the model follows the one bone nearest its middle, whole, with no blending: a lid on its hinge bone, a wheel on its axle; use it for hard objects, the default smooth skin for bodies |
| `pose` | `clip`, `keys` (`[{"t", "bones", "shapes", "reach", "look_at", "ease"}]`) or one pose's fields with `t`; `ease` smooth, linear, ease, hold | bones in degrees `{"upper_arm.L": [x, y, z]}`, shape keys `{"smile": 1}`, IK `{"hand.R": [x, y, z]}` (false lets go), `look_at` `[x, y, z]` or `{"clip": id}`: keyed at seconds into the clip; Zornov fills in between |
| `act` | `clip`, `motion` (walk, run, idle, wave, jump, nod, shake_head, talk, dance, sneak) or `mocap` (a .bvh or rigged .glb file; `take`) or `action` (one the model brought); `at`, `speed`, `energy`, `cycle`, `loop`, `travel` (walks carry the layer forward; `duration`), `blend_to` + `over` `[t0, t1]`, `stop` | motion: procedural cycles, motion capture retargeted onto the rig by bone role, a blend from one into another |

Bones are named by role: `hips`, `spine`, `chest`, `neck`, `head`,
`shoulder.L`, `upper_arm.L`, `forearm.L`, `hand.L`, `thigh.L`, `shin.L`,
`foot.L`, `toe.L` (and `.R`; L is the character's own left, +x),
`eye.L`/`eye.R`. A bone's degrees `[x, y, z]`: x swings it forward (+)
or back, y twists it along itself, z swings it sideways — mirrored on
`.R` bones, so `upper_arm.L` `[0, 0, 80]` and `upper_arm.R` `[0, 0, -80]`
raise both arms out. Knees and elbows bend with x (shins negative,
forearms positive). Positions (`reach`, `look_at`) are in the model's own
pixels: the character is centred, y down, facing −z (the camera).

IK handles on a humanoid: `foot.L`, `foot.R`, `hand.L`, `hand.R` (plant
a foot on a step, put a hand on a table), and `look` for the head (with
the eyes). Face keys use the names face capture and avatars use
(`jawOpen`, `eyeBlinkLeft`, `mouthSmileLeft`, `mouthPucker`,
`browInnerUp`…, and visemes `viseme_aa`, `viseme_E`, `viseme_I`,
`viseme_O`, `viseme_U`, `viseme_PP`, `viseme_FF`…); short words work in
`pose` (`smile`, `blink`, `frown`, `aa`, `oh`, `mbp`). A glTF/GLB with a
skeleton, morph targets and animations comes in rigged with its
animations as actions (`act` `action`), and `model` `export` .glb keeps
the rig and every action.

    {"op": "model", "shape": "character", "size": 520, "top": "#7a3b2e", "name": "Hero"}
    {"op": "act", "clip": "Hero", "motion": "walk", "duration": 4}
    {"op": "pose", "clip": "Hero", "keys": [{"t": 3.5, "shapes": {"smile": 0}}, {"t": 4, "shapes": {"smile": 1}, "look_at": [200, -180, -600]}]}
    {"op": "act", "clip": "Hero", "mocap": "/path/to/jump.bvh", "at": 4}
    {"op": "rig", "clip": "Rope", "template": "chain", "count": 8, "start": "top"}

## Reference: 3D simulation

`simulate` runs physics once and bakes it: rigid bodies become ordinary
keys; everything else a cache the clip plays (scrub freely, it never
simulates again). `at` and `duration` are seconds; `floor` is a world y
(default the bottom of the frame; false for none); `colliders` are model
clips things bump into (an animated character collides as it moves).

| kind | fields | does |
|---|---|---|
| `rigid` | `clips`, `static`, `colliders`, `bounce`, `friction`, `push` / `spin` (`[x, y, z]` or `{clip: [x, y, z]}`), `gravity` | models fall, tumble, stack and knock each other over |
| `shatter` | `clip`, `pieces`, `at` (when), `impact` `[x, y, z]` (model px), `strength`, `by` (a clip that breaks it on contact) | Voronoi pieces (broken faces get an "Inside" material) fly apart and land |
| `cloth` | `make` (cape with `on`: a character, flag, curtain, tablecloth) or `clip`; `fabric` (silk, cotton, denim, canvas, leather, rubber, flag), `pin` (top, left…, none), `pins` (points), `wind` `[x, y, z]`, `gusts`, `colour`, `position` | hangs, drapes, blows; a cape rides on the shoulders and swings with the walk |
| `soft` | `clip`, `stiffness`, `squash`, `damping`, `clusters`, `push` | jelly, rubber, squash and stretch |
| `hair` | `clip`, `region` (as `mesh`), `count`, `length`, `curl`, `comb`, `stiffness`, `colour`, `wind` | strands grown on the region that follow the model and swing |
| `particles` | `preset` (sparks, fountain, rain, snow, dust, confetti, embers, magic, debris, bubbles), `position` `[x, y, z]`, `size`, `rate`, `burst`, `life`, `velocity`, `colour`, `wind`, `emit_from` (a model's surface), `emit_for` | particles that collide with the floor and `colliders` |
| `smoke`, `fire` | `position`, `size`, `resolution`, `smoke`, `heat`, `swirl`, `wind`, `colour`, `density`, `fire`, `colliders` | a plume on a grid: hot air rises and curls, fire glows |
| `water` | `size`, `drops` (`[{"at": [x, z], "t"}]`), `wakes` (`[{"from", "to", "t0", "t1"}]`), `position` | a pond that ripples |
| `ocean` | `size`, `height`, `wavelength`, `wind` `[x, z]`, `position` | open sea swell |

    {"op": "simulate", "kind": "rigid", "clips": [4, 5, 6], "spin": {"5": [0, 0, 200]}, "duration": 3}
    {"op": "simulate", "kind": "cloth", "make": "cape", "on": "Hero", "fabric": "silk", "colour": "#8b0000"}
    {"op": "simulate", "kind": "shatter", "clip": "Vase", "by": "Ball", "pieces": 24}
    {"op": "simulate", "kind": "fire", "position": [0, 150, 0], "size": 300, "duration": 5}

## Reference: 3D worlds and final renders

`world` builds a set: `sky` (`{"time": 17.5 or "golden_hour", "day",
"latitude", "heading", "turbidity" 2 clear – 10 hazy, "strength",
"haze" (distance fades into the sky), "ground", "background"}`: a
daylight sky with the sun where the time puts it, lighting everything,
its colour going gold and red low down), or `hdri` (a .hdr panorama
lighting from every direction; `hdri_turn`); `terrain` (`{"style":
hills, mountains, dunes, plains, canyon, island, "size", "height",
"cells", "seed"}`: ground painted grass, rock, snow and sand by slope
and height); `scatter` (`[{"kind": pine, tree, palm, bush, rock, grass,
flower, "count", "size", "max_slope", "min_height", "max_height"
(fractions of the land's height), "clumping"}]`) on that terrain or
`on` a model.

`render3d` says how the camera draws models: `mode` path (light
bounces: soft shadows, colour bleeding, glass and water that bend
light, glowing surfaces lighting the room, skin and hair, smoke and
fire lit through) or raster (fast, the default); `samples` for the
final render (64; 256 for clean glass and interiors), `preview_samples`
(8, while editing), `bounces`, `denoise`, `motion_blur` (true or the
shutter 0-1); the camera's depth of field (`camera` `focus`,
`aperture`) blurs path-traced renders too. `passes` (`{"at", "out"}`)
writes beauty, depth, normal, albedo and object-id PNGs for compositing.
Glass and water: `mesh` `material` with `transmission` 1 and `ior`
1.5 (water 1.33).

    {"op": "world", "sky": {"time": "golden_hour", "haze": 0.4}, "terrain": {"style": "hills"}, "scatter": [{"kind": "pine", "count": 300}, {"kind": "rock", "count": 80}, {"kind": "grass", "count": 2000}]}
    {"op": "render3d", "mode": "path", "samples": 128, "motion_blur": 0.5}
    {"op": "render3d", "passes": {"at": 2.0, "out": "/tmp/shot12_passes"}}

## Reference: The 3D workspace

The tool `viewport` is the Player as a 3D space, the same one the person
sees: what you do there shows on their screen. The editor's eye is not a
camera: moving it changes nothing in the project until you put a camera
there. Use it to look round a 3D shot from any side, to place things by
where they show, and to set cameras up by eye.

- **Read** (`viewport` with no action): the eye (`position`, `target`,
  `yaw`, `pitch`, `distance`, `lens_mm`, `orthographic`), what it looks
  through, and every object in the world at `t` (models, 3D layers, 2D
  layers as cards, cameras, lights) with `at`, `size` and `on_screen`
  (`x`, `y` as 0..1 of the view, `in_view`, or "behind the view").
- **See**: `{"action": "picture"}` (or `"picture": true` on any action)
  sends the view as a picture with the floor grid, camera frustums,
  lights and outlines drawn in. Shading: `solid` (grey models, light from
  the view), `material` (materials, light from the view), `rendered`
  (the scene's own lights), `wireframe`, `xray`.
- **Move the eye**: `orbit` (`yaw`, `pitch` degrees; `about`: `selection`
  with `clips`, or `cursor`), `pan` (`x`, `y` px), `dolly` (`factor` < 1
  is closer), `fly` / `walk` (`forward`, `right`, `up` px), `look`
  (turn where it stands), `set` (`target`, `yaw`, `pitch`, `distance`,
  `lens`, `orthographic`, or `position` + `look_at`), `view` (`axis`:
  front, back, left, right, top, bottom: orthographic, as in 3D apps;
  `perspective`, `orthographic`, `camera`), `frame` (`clips`, or all).
- **Cameras**: `look_through` (`camera`, or the active one), `editor`
  (back to the eye), `camera_to_view` (the camera then sees exactly the
  eye's view: position, aim and lens; keyed at `t` if animated; a camera
  is added when there is none), `view_from_camera`, `lock_camera`
  (`on`: navigating while looking through moves the camera), and
  `record_path` (`camera`, `keys`: waypoints `{t, position, look_at}`,
  `{t, view: "saved name"}` or `{t, target, yaw, pitch, distance}`; the
  camera gets a key every other frame, smooth between waypoints).
- **Saved views**: `bookmark` / `go` / `forget` (`name`).
- **Objects**: `pick` (`x`, `y` as 0..1 of the view: what is drawn
  there), `select` (`clips` or `x`, `y`: selects in the app too), `move`
  (`clips`, `by` `[x, y, z]` px), `rotate` (`by` `[x, y, z]` degrees),
  `scale` (`by` factor or `[x, y, z]`); `space` world, view (right, down,
  away from the eye) or local; `pivot` median, individual or cursor;
  `snap`. Each is one undo step and keys animated properties at `t`. A
  2D layer moved in depth or turned becomes a 3D layer. `hide`, `unhide`,
  `lock`, `unlock` (`clips`) act in the viewport only. `cursor` (`at`)
  places the 3D cursor.
- **2.5D**: `spread` (`px`) shows 2D layers as cards spread in depth by
  stacking order (lowest furthest), to see and arrange stills and video
  in space; `options` (`cards`, `grid`, `overlays`, `frame`, `quad`,
  `snap`, `snap_step`, `snap_angle`, `tool`, `orient`, `pivot`,
  `floor_y`, `speed`).

Work like a cinematographer: frame the subject, orbit to find the angle,
`picture` to look, `camera_to_view`, then `frame` (the tool) through the
camera to check the shot itself.

    {"action": "frame", "clips": ["Head"]}
    {"action": "orbit", "yaw": 35, "pitch": 12, "picture": true}
    {"action": "camera_to_view"}
    {"action": "record_path", "keys": [{"t": 0, "position": [0, -200, -2600], "look_at": [0, 0, 0]}, {"t": 4, "position": [1400, -350, -900], "look_at": [0, 0, 200]}]}

In the app: the 3D button on the Player toolbar (or Shift-Tab over the
Player) turns the workspace on. Middle drag or Alt-drag orbits (round the
selection), Shift with it pans, the wheel zooms to the pointer; on a
trackpad two fingers orbit, Shift pans, a pinch zooms. Right drag looks,
W A S D Q E fly while it is held; `~` fly mode, Shift-`~` walk mode. 1, 3,
7 front, right, top (Cmd for the opposite), 5 perspective/orthographic, 0
through the camera, `.` frames the selection, Home everything. The view
gizmo's balls snap to an axis; dragging it orbits. Click picks (Shift
adds), G R S Q move, rotate, scale, select; Cmd flips snapping while
dragging; a right click places the 3D cursor. The header has the shading
modes, tools, axes (world, local, view), pivots, snapping, the View menu
(cameras, lock, record a fly-through, views, quad view, what shows,
spread, lens, saved views) and the outliner (lanes as collections with
their eye and lock, each object with a viewport eye and lock). While the
view moves the picture renders small and refines to full size when it
stops; scrubbing the timeline updates the view.

## Reference: 3D: space, cameras and light

3D is for depth the story needs: a photo wall the camera drifts past, a
title that stands up in a room, a product turning to show its side. It
is not a way to make flat things busy.
- **Parallax is the workhorse.** Three or four layers at different depths
  (backdrop far, subject mid, title near) and a slow camera move: `truck`
  a little sideways, or `push_in`. The near layers slide faster than the
  far ones, which is what makes the eye read depth. Spread them by about
  the lens distance (`parallax` does this and keeps each layer's size);
  the backdrop needs overscan or a move shows its edges. Every camera
  move grows a full-frame picture or colour backdrop just enough to keep
  its edges out of frame and says so ("backdrop N scaled up"); a backdrop
  that does not fill the frame at the start is left alone, so start full.
- **Camera language.** A push in is attention ("look at this"); a pull
  out is reveal or ending; a truck or orbit is exploration; a pedestal up
  is lift, ending high. Motivate every move. One move per shot, eased in
  and out (`ease_in_out`), slower than you think: 3-6 s for 20-40° of
  orbit, a push of a quarter of the distance (about 650 px for the
  default camera on a 1920 px canvas; under a tenth reads as no move at
  all). Compare the first and last frames: the move must show. A camera
  that never stops moving is a screensaver.
- **Lenses.** 35-50 mm feels natural; 24 mm and wider exaggerates depth
  and distorts edges (energy, scale); 85 mm and longer flattens and
  isolates. A `dolly_zoom` is a single moment of unease: once, if ever.
- **Depth of field** sells scale and focus, but keep the subject sharp:
  focus on it (`focus: <clip>`), a modest `aperture`. Heavy blur on
  everything is mud. Rack focus (animate `focus_distance`) only when
  attention really moves.
- **Light.** One key light (a spot from the upper left, aimed at the
  subject) plus a soft `ambient` (20-35 %) so nothing goes black. Warm
  key, cool ambient reads cinematic. Shadows only where they explain
  space: a card over a wall, text over a floor; `shadow_darkness` 40-60 %.
  Lit layers darken by default: check the frame after adding light.
- **Turns.** A layer seen edge-on (rotation 90°) disappears: keep text
  within ±60° of the viewer while it must be read, and let it settle
  facing front. 3D entries (`turn_in`, `card_flip`, `swivel`, `fly_in`)
  are entries, like any other: one style per video, short (0.4-0.8 s).
- **Extrude** text and logos for a solid, sculpted look: thickness about a
  tenth of the letter height; a side shade of 50-60 %. Too thick reads as
  a smear.
- **Not overdoing.** One 3D idea per scene. No spinning everything, no
  constant orbit, no extreme lenses for their own sake. If a 2D move says
  it, use the 2D move.

## Reference: Sculpting and modeling

Anything with volume is a **model** (`model`, `mesh`), never a pile of
flat layers. A face built from hundreds of flat circles, ovals and cards
in 3D space looks like paper cut-outs the moment the camera turns, and it
cannot take light. One model, sculpted, can.

- **Start from the closest base.** A face or a person's head starts from
  `shape: "head"` (anatomy already right: skull, jaw, cheekbones, eye
  sockets with eyes and lids, nose, lips, ears, neck); make it someone
  with brushes (a broader jaw, fuller lips, a longer nose, deeper-set
  eyes) rather than building it from a ball. Bodies, animals, products
  and soft forms start from blended `parts`. A real object that exists
  as a model file (a chair, a car) is imported.
- **Look first.** For a real subject, `generate` a reference image (front
  view, and a side view) and look at it before you sculpt: proportions
  are where models go wrong.
- **Big to small.** Block the masses on a coarse surface (an ico_sphere
  at detail 3-4), check the silhouette from the front and the side, then
  `subdivide` and do the secondary forms, then subdivide again for
  details. Detail on a coarse surface is lumpy; masses on a dense one are
  slow and wobbly.
- **Brush sizes follow the form.** Big forms: radius a third to half of
  the model. Secondary forms (nose, brow, cheekbones, lips): a tenth to a
  fifth. Details (nostrils, eyelid edges, lip line): a twentieth, with
  `stroke` along a path. Strength around a tenth to a fifth of the radius;
  several gentle passes beat one hard one.
- **Symmetry on** for faces, bodies and products; turn it off only for
  the last asymmetric touches (a smile's corner, a scar).
- **A human head** (front of the face towards -z, size 300 px tall):
  the skull is an egg, wider at the back and top, about 0.75 as wide as
  tall and 0.85 as deep. The eyes sit halfway down the head, one eye's
  width apart, in sockets pushed in about a fifth of the head's depth
  from the front plane; the brow ridge stands out above them. The nose
  bottom is halfway between eyes and chin; the mouth a third of the way
  from nose to chin; ears between the brow and nose bottom lines, on the
  sides just behind the middle. The jaw narrows from below the ears to
  the chin; the neck is a cylinder about half the head's width, set
  behind the jaw. The cheekbones are the widest part of the face. Eyes
  are spheres (`add`, a white sclera with a separate dark iris
  `material` region), set in the sockets so the lids cover their top.
- **Materials.** Skin: roughness 0.45-0.6, metallic 0, `subsurface`
  0.5-0.7 (without it skin looks like plastic), a warm colour;
  lips slightly redder and smoother; eyes roughness 0.1 for the
  highlight. Metal: metallic 1 with roughness 0.2-0.4. Plastic:
  metallic 0, roughness 0.3-0.5. A `texture` from a generated image
  adds pores, freckles, wood grain or a label.
- **Light it.** A key light above to one side, a weaker fill on the
  other side, a rim from behind; an ambient light keeps shadows from
  going black. Shadows (`shadows: true` on the key) ground it.
- **Turn it.** Show volume with motion: `animate` `rotation_y` slowly
  (20-40 degrees over several seconds), or a camera `orbit`. Look at
  the frames from several angles before calling it done.
- **Imports.** A real model file (`model` `file`) beats sculpting a
  complicated object from scratch: chairs, cars, buildings, animals.

### Hero objects: designed, not stacked

Two rounded boxes with one carved from the other is a blob, not a product.
What separates a designed hero object from it, and what the 3D critic
looks for:
- **Reconstructed surfaces.** A model made from a picture is judged for
  what that method gets wrong, and each note says its fix:
  `3d_holes` (open edges: part of the object was lost or the picture was
  cropped), `3d_floating_pieces` (a shadow, second object or background
  left a loose piece) and `3d_smeared_texture` (stretched or streaked
  colour: glare or hard shadow in the picture, or too small a texture).
  The fix is always the picture first: regenerate it whole, one subject,
  plain background, even light, then `generate` `task: "model3d"` again;
  at most 2 attempts, then keep the better and say what is off.
- **Parts.** A real object is several pieces, and which pieces depends on
  what it is: a device has a body, buttons, a screen or a light strip; a car
  has wheels, glass, lights, doors; a wardrobe has doors, handles, legs; a
  creature has a head, limbs, a tail, wings; a building has walls, a roof,
  windows, a door. List them in `notes` ("Parts: body, lid, light strip")
  from the brief before building; build every one as its own part of the
  one model, named for what it is (`asset` parts, or `generate` task
  `model3d` from one clean reference picture for an organic or any single-object form).
- **Edges.** Nothing real has a razor edge: a bevel about 1% of the size
  (3 segments) catches a highlight along every edge, and that line is what
  makes a render read as a made thing. Bevel first, then subdivide.
- **Moving parts.** A lid is a piece on a bone whose pivot lies on the hinge
  line; opening is a rotation about it, keyed with `pose`. Never swap in a
  second model of the open state: the viewer sees a cut instead of the
  thing moving.
- **Materials.** Each part its own: matte body, polished or brushed metal,
  glass, rubber. A light that is described is an emissive material, with
  light cast from it. Default grey plastic on a hero is the clearest tell.
- **Light and lens.** Key, rim and fill (a hero lit by one light has no
  edge); a slow camera move and shallow depth of field that keep the model
  sharp and soften the room.
- **Compare.** Generate reference views first and check the model against
  them from the same angles; proportions are where models go wrong.
- **Say what is open.** If a note stands at the end, the answer lists it.
  "Check and review passed" is only true when they did.

## Reference: 3D animation: rigs and acting

A 3D character is one rigged model that moves by its bones, never a
stack of parts nudged frame by frame. Start from `shape: "character"`
(rigged, skinned, a face with shape keys) or `rig` a model you made.
- **Pose to pose.** Block the story poses first with `pose` (the
  extremes: crouch, leap, land, look), half a second to a second apart,
  look at them as stills, then let Zornov fill in between; add a
  breakdown pose where an arc needs one. Hold a strong pose 6-12 frames
  so it reads.
- **Weight and balance.** The hips carry the body: they lead a walk and
  drop on every footfall. A standing pose puts the weight over one foot,
  the hips tilted and the shoulders tilted the other way (contrapposto).
  Nothing is symmetric: one arm forward, the head turned a little.
- **Arcs and overlap.** Heads, hands and feet travel on arcs. The body
  starts a move, the arms and head follow a few frames later and settle
  after it stops (key the follow-through a little later than the main
  move). A cape, hair or a tail lags further still: simulate it.
- **Eyes lead.** The eyes (and then the head) turn to a thing before the
  body does: `look_at` the target a few frames ahead of the turn. Blink
  (`blink` 1 for 3-4 frames) on a turn or a change of thought, not on a
  timer.
- **Cycles.** `act` walk/run/sneak are in-place cycles with travel so the
  feet do not slide; plant a foot on a step with `reach` (`foot.L`).
  Change speed with `speed` (and the travel follows), mood with `energy`
  (0.6 tired, 1.4 eager). Blend walk into run over half a second
  (`blend_to`), not instantly.
- **Mocap.** A captured take (`act` `mocap`) carries real timing: use it
  for long natural motion, then add a pose on top (bone keys add to the
  capture) to aim a look or fix a contact.
- **Faces.** Speech is visemes timed to the words (`viseme_aa` open
  vowels, `viseme_O`/`viseme_U` rounded, `viseme_PP` for m, b, p closing
  fully, `viseme_FF` for f, v), lips leading the sound by a frame or two,
  most shapes at 60-80 % (full shapes look shouted). Emotion lives in the
  brows and the mouth corners (`browInnerUp` worry, `mouthSmile` warmth,
  `mouthFrown`); keep a little asymmetry.

## Reference: Simulation: cloth, hair, smoke, destruction

Simulate what nobody could key by hand; key what the story needs to
control. Bake once, then edit with the keys and caches it leaves.
- **Rigid bodies.** Real weight comes from gravity about two frame heights
  a second squared (the default), a little bounce (0.2-0.4; rubber 0.7)
  and friction so things settle. A stack falls when something hits it:
  start them at rest and push the striker. Check the end frame: nothing
  should hover or sink.
- **Breaking.** More, smaller pieces near the impact (`impact`), bigger
  ones away; 15-40 pieces read as glass or pottery, 8-12 as stone. Let a
  thrown object break it (`by`) so the timing is the hit's. Dust or
  debris particles at the same moment sell it.
- **Cloth.** Pick the fabric: silk flows and ripples, cotton drapes,
  denim and leather hold folds. Pin what holds it (a flag's pole edge, a
  curtain's rail, a cape's shoulders). Wind with gusts beats steady wind;
  a gentle 200-400 px/s moves a curtain, 600+ streams a flag.
- **Hair and fur.** Short stiff fur barely moves; long hair lags and
  swings past a head turn by 6-10 frames. Groom it combed the way gravity
  pulls; a few thousand strands read as hair at normal framing.
- **Particles.** Fewer, well-timed particles beat a blizzard: sparks a
  burst of 200-400 that cools to orange, snow drifting slowly (low
  gravity, swirl), rain fast and thin. Let them land on the set (the
  floor and `colliders`).
- **Smoke and fire.** Fire is hot and fast, rising in tongues with dark
  smoke above; steam is white and fades fast; dust hangs low. More
  `resolution` for close-ups (64-96), less for distant plumes. Light it
  with the scene's sun: smoke takes the light's colour.
- **Water.** A pond reacts to what touches it (drops, a wading foot, a
  boat's path as a wake); the sea always moves (swell from the wind).
  Water is glass with `ior` 1.33: render it path traced.

## Reference: Worlds and final renders

A 3D set is ground, sky, things on the ground, and light; shoot it like
a location.
- **Time of day is the mood.** Morning is clear and cool, noon flat and
  harsh (avoid it for drama), golden hour warm with long shadows, dusk
  red and low, blue hour cold and quiet. Set the sky's `time` before
  anything else; its sun lights the scene and colours the shadows.
- **Haze makes distance.** A little `haze` (0.2-0.5) fades far hills
  into the sky and gives a landscape scale; none makes it look like a
  model railway.
- **Ground and scatter.** Choose the terrain to the story (hills for a
  journey, mountains for awe, dunes for isolation). Scatter in layers:
  a few big trees, more bushes and rocks, many grass tufts; clump them;
  keep trees off steep slopes and high peaks. Keep the camera's path
  clear.
- **Path tracing for finals.** Raster is for blocking and preview; switch
  the camera to `render3d` `mode: "path"` for the shots that ship: light
  bounces between surfaces, shadows soften with distance, glass and
  water bend light, glowing things light the room. 64 samples for most
  shots, 128-256 for glass, interiors and dark scenes; the denoiser
  cleans the rest. Motion blur (shutter 0.5) for anything fast.
- **Passes.** When a shot will be composited (fog, glows, a matte for a
  title behind a character), write `passes`: depth for fog and focus,
  normals for relighting, ids for mattes.

## Reference: tool `viewport`

The 3D workspace, used as a person uses it (the Player shows it too): `get` reads the editor's eye (position, target, yaw, pitch, distance, lens, orthographic), what it looks through, shading, saved views, and every object in the world (models, 3D layers, 2D layers as cards, cameras, lights) with where it sits and where it shows on screen (x, y as fractions of the view). Move the eye: `orbit` (yaw, pitch; about: selection with clips, or cursor), `pan` (x, y canvas pixels), `dolly` (factor; < 1 goes closer; x, y fractions to zoom towards), `fly` / `walk` (forward, right, up pixels), `look` (yaw, pitch from where it stands), `set` (target, yaw, pitch, distance, lens, orthographic; or position + look_at), `view` (axis: front, back, left, right, top, bottom, perspective, orthographic, camera), `frame` (clips, or everything). Cameras: `look_through` (camera, or the active one), `editor`, `camera_to_view` (camera; the camera then sees the eye's view, keyed if animated), `view_from_camera`, `lock_camera` (on: navigating moves the camera), `record_path` (camera, keys: [{t, position, look_at} | {t, view: saved name} | {t, target, yaw, pitch, distance}], keyed every other frame). Views: `bookmark` / `go` / `forget` (name). Objects: `pick` / `select` (x, y fractions of the view, or clips), `move` (clips, by [x, y, z] px; space world, view or local), `rotate` (clips, by [x, y, z] degrees), `scale` (clips, by factor or [x, y, z]); pivot median, individual or cursor; snap. `hide` / `unhide` / `lock` / `unlock` (clips, viewport only), `cursor` (at [x, y, z]), `shading` (mode: solid, material, rendered, wireframe, xray), `spread` (px between 2D layers in depth), `options` (cards, grid, overlays, frame, quad, snap, snap_step, snap_angle, tool, orient, pivot, floor_y, speed), `on` / `off`. `picture`: the view as a JPEG with grid, frustums, lights and outlines (or picture: true with any action). Units: canvas pixels from the centre, y down (negative y is up), z away.

## Reference: math shapes

**What it is.** Mathematics as a way to make form and motion, written in Studio's formula language (the one `modifier` and `driver` use: `+ - * / ^ %`, `c ? a : b`, `name = …;` bindings, `sin cos atan2 sqrt abs min max clamp smoothstep mix fract mod noise`…, plus the shape functions below). Formulas are safe: no loops, capped sizes and iterations; every reply says how many evaluations it made, the grid it sampled and whether the solid is closed.

**How**
1. Pick the kind of mathematics that makes the form (table in Reference): a solid by a formula (`implicit`), a surface by u and v (`parametric`), a tube along a curve (`tube`), ground (`height`), growth (`lsystem`, `phyllotaxis`), a trail (`attractor`), a fractal (an implicit preset). Start from a preset and change its settings by name; write a formula of your own when no preset fits.
2. `model {"math": …, "size": px, "detail": 1-6}`: a real 3D model, closed where it can be (lattices are cut to their box; open sheets get a thin shell), with normals and texture coordinates. Colour and finish as for any model (`colour`, `metallic`, `roughness`, `emissive`…).
3. To melt a formula into clay, carve with it or cut by it, give a blended part `math` (implicit kinds only): `model {"parts": [{"shape": "ellipsoid", "size": 220}, {"math": {"preset": "gyroid", "cells": 3, "solid": 1}, "size": 240, "intersect": true}]}`.
4. Warp any model (made by math or not, rigged or not): `mesh` steps `twist`, `bend`, `taper`, `warp` (noise, or a formula offset), `repeat` (a grid, or round an axis). A coarse model is split first so it bends smoothly.
5. Motion: a shape whose formula reads `t` (or that `morph`s to another) is baked to a frame cache over the clip and plays; `animate {"curve": …}` moves a clip along a curve (2D, or 3D with `z` or an `attractor`), `orient: true` turns it along the way; `animate {"formula": "…"}` keys any number by a formula of t.
6. Flat curves: a shape layer item `{"type": "curve", …}` (roses, spirographs, L-systems, Fourier epicycles), stroked or filled like any shape; its settings animate and its formulas see `t`, so it morphs as it plays.
7. Light it (`lighting`, or `light` with shadows), move the camera, then look: `contact_sheet` or `see` from two or three angles; `check` before export.

**Checks**
- The reply's `closed` is true for solids; `evals` and `grid` are under the caps; `notes` say when a cap lowered the resolution or the frame rate of a bake.
- A lattice reads at the size it is seen: 2 to 4 `cells` across for a hero object, more for a texture of holes.
- Look at a moving shape at three times (`contact_sheet` with `times`).

**Mistakes**
- A formula that is positive everywhere in its domain has no surface ("no surface" error): check the sign (solid where it is 0 or less) and `bounds`.
- `v` and `u` are the surface parameters; in curves `u` is the parameter and `theta` the polar angle; `t` is always time.
- Formula space is the mathematician's: x right, y away, z up (Studio's y is down). Warp steps take the model's own axes (`axis: "y"` is up-down).
- An implicit shape that moves keeps its triangles and follows the surface; one that splits in two or merges as `t` runs stretches instead.

**Meshed or drawn directly.** An implicit solid or fractal is meshed by default (`render: "mesh"`: surface nets at `detail`, closed, with normals and texture coordinates: what materials, rigs, physics, booleans, mesh steps and exports need). `render: "direct"` instead draws it straight from its formula: each pixel's ray is sphere traced through the field on the GPU (the CPU without one), so a Mandelbulb or a gyroid keeps exact detail however close the camera comes, and a formula that reads `t` moves live with no bake. It is lit, shadowed, fogged and depth-sorted with the other models. What it cannot do: it is not triangles, so physics, rigs, collisions, booleans, mesh steps, exports and path-traced renders use its stand-in, a coarse mesh made with it (which also casts its shadows); it takes no texture. Switch a model either way with `mesh {"do": "render", "mode": "direct" | "mesh", "detail"}` (back to mesh remeshes it from the formula). Parametric surfaces, tubes, height fields and growth are always meshes; flat `curve` items are always live paths, worked out as they are drawn.

**Fields and presets**

**`math` on `model`** (also on a blended part, and on `mesh {"do": "add", "math", "size", "at"}`): a preset name, or an object with one of these fields naming its kind. Every other number in it is a setting, bound by name in front of the formula (`{"implicit": "sphere(x, y, z, rad) + 0.1 * noise3(x * 5, y * 5, z * 5)", "rad": 0.7}`); `"t": 2` evaluates at a time; `"bake": false` keeps a moving shape still.

| kind | fields | |
|---|---|---|
| `implicit` | (with `render` direct or mesh on the model op) formula of `x y z t r` (or a list, worked through by `weight`), `bounds` (half side of the domain, or `[x, y, z]`; default 1), `clip` (`box` or `ball`: what it is cut to), `weight` (a formula of t for a list), `morph` `{"to": …, "over": [t0, t1]}` | solid where the formula ≤ 0; `size` spans the domain |
| `parametric` | `"[x, y, z]"` of `u v t` (or three formulas, or `{"x","y","z"}`), `u`, `v` (ranges, numbers or formulas: `[0, "tau"]`), `coords` (`cartesian`, `cylindrical`: r, theta, z; `spherical`: r, theta, phi), `resolution` (n or `[nu, nv]`), `thickness` (px; 0 a bare sheet), `cut` (holes where it is true; `u`, `v` 0..1 over the surface, also `x y z`, raw `pu pv`, `t`), `morph` | seams and poles weld shut |
| `tube` | `"[x, y, z]"` of `u t`, `u`, `radius` (curve units, or a formula of u), `sides`, `resolution` | closed curves close, open ones are capped |
| `height` | formula of `x y t r`, giving the height up; `bounds` (half side, or `[x, y]`), `resolution`, `depth` (px of block under the lowest point) | a closed block of ground |
| `lsystem` | preset name or `{"axiom", "rules": {"F": "F[+F]F[-F]F"}}`; `iterations`, `angle`, `width`, `taper`, `shrink`, `droop`, `seed`, `sides`, `leaves`, `leaf_colour` | turtle: `F G` draw, `f` move, `+ -` turn, `& ^` pitch, `\ /` roll, `[ ]` branch, `!` thin, `L` leaf |
| `phyllotaxis` | `disc` (sunflower), `ball`, `cone` (pinecone), `dome` (cactus); `count`, `angle` (137.508, the golden angle), `seed_size`, `core`, `seed_colour`, `core_colour` | |
| `attractor` | preset name or `{"dx", "dy", "dz"}` (formulas of x y z t); `start`, `dt`, `steps`, `skip`, `radius`, `sides`, `every` | Runge–Kutta, capped at 200 000 steps |

Presets (settings in brackets): implicit `gyroid`, `schwarz_p`, `diamond`, `neovius`, `lidinoid` [cells, thickness, solid: 0 a sheet, 1 one network], `strut_lattice` [cells, width], `foam` [cells, width, seed], `blob` [radius, amount, frequency, speed, seed], `rock`, `ripple_sphere` [waves, amount, speed], `metaballs` [size, speed, threshold], `superquadric` [p], `heart`; fractals `mandelbulb` [power, iterations], `mandelbox` [scale, iterations], `menger` [iterations 1-7], `sierpinski` [iterations]. Parametric `torus` [major, minor], `sphere`, `swirl_torus` (r = 3 + sin u + cos v, theta = 2u, z = sin v + 2 cos u) [a, turns], `mobius` [width, twists], `klein`, `seashell` [turns], `nautilus` [turns, growth, open], `horn`, `vase` [waves, bulge, tall], `dini`, `enneper`, `helicoid`, `catenoid` [c, span], `supershape` [m, n1, n2, n3, m2, k1, k2, k3], `boy`. Tubes `torus_knot` [p, q, major, minor], `trefoil`, `figure_eight_knot`, `lissajous_knot`, `helix`, `spiral`. Heights `waves`, `ripples`, `dunes`, `terrain`, `saddle`, `egg_crate`, `sinc`. L-systems `tree`, `bush`, `plant`, `fern`, `weed`, `pine`. Attractors `lorenz`, `rossler`, `aizawa`, `thomas`, `halvorsen`, `chen`.

Shape functions in formulas: `sphere(x,y,z,r)`, `box(x,y,z,a,b,c)` (half sides), `torus(x,y,z,R,r)`, `cylinder(x,y,z,r,h)`, `length(…)`, `smin(a,b,k)`, `smax(a,b,k)` (smooth union / intersection; `max(a, -b)` subtracts), `gyroid`, `schwarz_p`, `schwarz_d`, `neovius`, `lidinoid` of (x, y, z), `noise2`, `noise3`, `fbm(x,y,z,octaves,seed)`, `voronoi(x,y[,z])`, `voronoi_edge(…)`, `mandelbulb(x,y,z,power,iters)`, `mandelbox`, `menger`, `sierpinski` (iterations capped at 30), `superformula(angle,m,n1,n2,n3)`, `sinh cosh tanh cbrt mix`. Domain tricks are formulas too: repeat with `x = mod(x + 0.5, 1) - 0.5;` in front, twist with `a = z * 2; x2 = x * cos(a) - y * sin(a); …`.

Examples:

    {"op": "model", "math": {"preset": "gyroid", "cells": 3, "thickness": 0.25}, "size": 320, "colour": "#d8b25a", "metallic": 1, "roughness": 0.3}
    {"op": "model", "math": {"preset": "swirl_torus"}, "size": 300, "colour": "#3b82f6"}
    {"op": "model", "math": {"preset": "sphere", "cut": "fract(u * 16) < 0.55 || fract(v * 10) < 0.5", "thickness": 3}, "size": 260}
    {"op": "model", "math": {"implicit": "sphere(x, y, z, 0.6 + 0.08 * sin(6 * atan2(y, x) + 3 * t))", "bounds": 1}, "duration": 4}
    {"op": "model", "math": {"preset": "mandelbulb", "power": 8, "iterations": 12}, "size": 400, "detail": 5}
    {"op": "model", "math": {"lsystem": "tree", "iterations": 6, "angle": 24}, "size": 500, "colour": "#6b4a2f"}
    {"op": "model", "math": {"phyllotaxis": "cone", "count": 400}, "size": 220}
    {"op": "model", "math": {"preset": "torus", "morph": {"to": "klein", "over": [0, 3]}}, "duration": 4}
    {"op": "mesh", "clip": 5, "steps": [{"do": "twist", "axis": "y", "degrees": 180}, {"do": "taper", "from": 1, "to": 0.5}, {"do": "bend", "degrees": 40, "toward": "x"}]}
    {"op": "mesh", "clip": 5, "do": "warp", "math": "[0.1 * sin(5 * z), 0.1 * cos(5 * z), 0]"}
    {"op": "mesh", "clip": 5, "do": "repeat", "count": 8, "around": true, "radius": 300}
    {"op": "animate", "clip": 7, "property": "position", "curve": {"preset": "lissajous", "a": 3, "b": 2, "size": 500, "orient": true, "loops": 2}}
    {"op": "animate", "clip": 7, "property": "position", "curve": {"attractor": "lorenz", "size": 400}}
    {"op": "animate", "clip": 7, "property": "rotation_y", "formula": "360 * t / 4 + 10 * sin(t * tau)"}
    {"op": "shape", "items": [{"type": "curve", "preset": "rose", "n": 7, "d": 3, "size": 500}, {"type": "stroke", "colour": "#f0c", "width": 3}]}
    {"op": "shape", "items": [{"type": "curve", "fourier": {"of": "heart", "terms": 16, "period": 6}, "size": 500}, {"type": "stroke", "colour": "white", "width": 2}]}

**`curve` items and `animate` curves** (studio's shape layers): `preset` with its settings (rose [n, d], lissajous [a, b, delta], heart, spiral [turns], log_spiral [growth, turns], fermat_spiral, superformula [m, n1, n2, n3], spirograph [big, small, pen], epitrochoid, cardioid, astroid, deltoid, butterfly, lemniscate, maurer_rose [n, step], wave [cycles, amount, speed]; implicit cassini [a, b], folium, devil, heart_implicit; L-systems koch, dragon, hilbert, sierpinski, gosper, levy, plant, fern, square_tiles), or `x` + `y` (formulas of `u`, `t`), `polar` (r of `theta`, `t`), `u` (range), `samples` (up to 20 000); `implicit` (f of `x y t r` = 0) with `bounds`, `resolution` (up to 600); `lsystem` `{axiom, rules, iterations, angle, draw}`; `fourier` `{of: points (px) | an SVG path | a preset | a curve, terms (up to 500), epicycles (circles, arm and trace, default true), period (s), trace}`. `size` fits the curve (px), `scale` is px per unit (default 100), `smooth: true` bends it through its points. On `animate`, the curve also takes `duration`, `start`, `loops`, `keys` (up to 600), `centre`, `orient`; with `z` (and `x`, `y` formulas of u) or an `attractor` it is a 3D path keying x, y and z.

**Caps**: formula grids at most 400 a side and 60 million evaluations a build (a finer `detail` is made coarser and said so); surfaces 512 × 512 samples; L-systems 200 000 symbols and 60 000 drawn points; phyllotaxis 20 000 seeds; attractors 200 000 steps; fractal iterations 30; a bake at most 240 frames and 30 million vertex positions (a lower frame rate, played smoothly between); formula animations 600 keys.
