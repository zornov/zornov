---
name: physics-3d
use: Use when 3D things must act physically: fall, stack, collide, swing, break, be thrown, ragdoll, jiggle.
topics: physics, physics-3d, rigid-body, rigid-bodies, gravity, collision, collisions, colliders, fall, falling, drop, throw, push, knock, topple, tip, stack, stacking, dominoes, bowling, pendulum, hinge, joint, joints, rope, chain, spring, door, ragdoll, knocked-down, jiggle, secondary-motion, follow-through, tail, ears, antennae, feathers, coat-tails, settle, resting, floating, sinking, mass, weight, material, destruction, shatter, break, breaking, glass, vehicle, crash
tools: body, joint, drop, throw, push, break, settle, ragdoll, jiggle, bake_physics, physics_world, simulate, model, mesh, asset, check, review, frame
---
**Order**
1. Build real solids first (skill `3d-assets-scenes`): closed models with thickness. Physics measures the solid's volume, so a flat card or an open shell has no weight and collides like paper (`3d_flat`, `3d_open`: fix with `mesh` step `solid`).
2. Scale: Zornov works it out (people about 1.7 m, worlds at their own scale, else 475 px to the metre). If the scene is something else (a toy table, a city), say it once: `physics_world {px_per_metre}`. Gravity is real (9.81 m/s²) at that scale, so small things fall fast and big things slowly.
3. New models rest on what is under them when placed (`rest: false` on `model` to float it on purpose). Put the ground, floor or table in first, then what stands on it.
4. Say what each thing is: `body {clip, type, material}`. Props that move are `dynamic`; scenery is `static` by default; animated clips and characters push things out of their way (kinematic) without being moved. A material sets density (mass from the real volume), friction, bounce and whether it breaks: glass, porcelain, ceramic, ice, plaster shatter when hit hard.
5. Joints for what is held: `joint {clip, kind: hinge, at_point, axis, limits}` for doors, lids, pendulums; `rope`/`chain`/`spring` for hanging things; `slider` for drawers; `fixed` to glue two together; `breaks` (N) to snap.
6. Make it happen: `drop`, `throw {toward}` (lands on the target), `push {direction, strength, point}`, `break {at, point}`, `ragdoll` for a character that falls, `bake_physics` for the whole scene from its settings. All of it bakes into keys (and pieces for breaks): scrub, cut and render without simulating again. Doing it again replaces the run from its start.
7. Characters: a hard hit knocks a character loose (it staggers and falls; `react: false` to stop that); `ragdoll {strength}` from 0 (limp) to 1 (holds its pose a moment). `jiggle` gives tails, ears, antennae, ponytails, coat tails and feather bones follow-through after the body moves; `flesh` 0..1 shakes loose skin.
8. `frame` the result at the start, the middle and the end, from the side as well as the front; `check` reports `3d_floating`, `3d_sunk` and `3d_tips` with the clip and the amount; `settle` (no clips) fixes them all.

**Checks**
- Every prop rests on something or is held by a joint; nothing sinks into what it touches.
- Heavy things fall at the same speed as light ones; what differs is how they hit (a steel ball dents through a pile, a foam ball bounces off).
- Contacts happen where they should: a glass breaks on the floor, not in the air; a domino row falls one by one.
- The reply of each run says when everything came to rest, and lists hard hits: put sounds there (`sfx`).

**Mistakes**
- Keying a fall or a bounce by hand: it looks weightless. Use `drop`/`throw`.
- A thing that should hang (a lamp, a sign) left floating: `joint` it (`rope` from the ceiling) or mark it `body {type: none}` if it is meant to fly.
- Two models in one place (a cup inside the table): `settle` it.
- Walls of a room or a floor too thin: give scenery real thickness, or the fast things may skim through its edges.
- A rigged character dropped with `drop`: that moves it as one stiff piece. Use `ragdoll`.

## Reference: physics ops

| op | fields |
|---|---|
| `body` | `clip`/`clips`, `type` (auto, dynamic, static, kinematic, none), `material`, `mass` (kg), `collider` (auto, box, sphere, capsule, hull, parts, mesh), `friction` (0-2), `bounce` (0-1), `breakable` |
| `joint` | `clip`, `to` (else the world), `kind` (hinge, ball, slider, fixed, spring, rope, rod), `at_point`, `axis`, `limits`, `motor` [speed, torque], `spring` [Hz, damping], `breaks`, `remove` |
| `drop` | `clip`/`clips`, `at`, `duration` (default 4), `height` (px lifted first), `spin` [x, y, z] deg/s, `floor` (false, or a y) |
| `throw` | `clip`, `velocity` [x, y, z] px/s, or `toward` (clip or point) with `time` (s in the air), `spin`, `at`, `duration` |
| `push` | `clip`, `direction` + `strength` (px/s), or `from` (clip or point), `point` (where on it), `at`, `duration` |
| `break` | `clip`, `at`, `point`, `pieces` (default 18), `when_hit` (true: only when something hits it hard), `by` (the clip that hits) |
| `settle` | `clips` (default: whatever floats, sinks or tips), `at`, `duration` |
| `ragdoll` | `clip`, `at`, `duration`, `strength`, `fade` (s), `push` [x, y, z] px/s, `point` |
| `jiggle` | `clip`, `bones` (names, or auto), `stiffness` 0-1, `damping` 0-1, `gravity`, `drag`, `flesh` 0-1, `at`, `duration` |
| `bake_physics` | `at`, `duration` (default 5), `clips` (extra dynamic), `react`, `floor` |
| `physics_world` | `px_per_metre`, `gravity` (m/s²), `rest_on_place` |

Colliders: `auto` picks the simplest that fits (a ball, a turned box, one convex hull, or convex parts for concave things such as a chair or a mug); `mesh` is the exact triangles (scenery). Open and paper-thin models are made solid for collision.

## Reference: examples

- A stack that is knocked over: build the boxes resting on each other, `body {clips, type: dynamic, material: wood}`, `throw {clip: ball, toward: <the middle box>}`.
- Dominoes: tiles standing in a row (a little closer than their height), `push {clip: first, direction: [1, 0, 0], point: <near its top>}`.
- A chair that tips: placed leaning, `settle {clip}`, or `push` its back.
- A bowling shot: pins and a heavy ball `body {material: rubber, mass: 6}`, `throw {velocity}`; the pins are dynamic wood.
- Cloth over furniture: `simulate {kind: cloth, make: tablecloth, colliders: [table]}`; a cape or coat on a walking character: `simulate {kind: cloth, make: cape, on}`.
- A glass that shatters: `body {material: glass}` then `drop {height}`; it breaks where it lands.
- A character hit by a ball: the ball `throw {toward: character}` inside `bake_physics` (characters react); or `ragdoll {push}`.
- A dog's tail and ears in a run: `rig {template: quadruped}`, `act`, then `jiggle`.
