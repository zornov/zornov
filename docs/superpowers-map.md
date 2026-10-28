# Zornov superpowers map

What Zornov can already do but nobody (human or agent) can reach, what is broken underneath, what it cannot sense yet, and what to build so it becomes a creation engine that measures its own work.

**How this was built**
- A 10 s formula-built film (L-system hero tree and forest, SDF bear and bunnies, wind, flocks, keyed walks) was made end to end through the MCP.
- Five independent asset-analysis passes took that film apart: meshes, rigs, motion paths, physics expressions, and a rigging lab that rebuilt and tested skeletons. Each has verified numbers.
- The source at `6afccac` was then audited against every finding, with file:line references.
- One behaviour is proven by a probe test that runs: [`crates/3d/mesh/tests/superpowers_probe.rs`](../crates/3d/mesh/tests/superpowers_probe.rs).

Paths below are relative to the repo root.

---

## 0. The four truths

1. **Zornov already owns most of the hard engines. They are dark.** These exist, but no caller, skill or guide brings them into play:
   - Drivers with cross-clip `prop()` and delays.
   - A real gait planner with foot plants.
   - Geodesic skin weights; FABRIK, soft and stretch IK; contact pins.
   - A dual-contouring mesher.
   - GPU instancing and LOD.
   - An SDF ray-marching renderer.
   - `loop_out`, `inertia`, `velocity` in expressions.

   Agents rebuilt them badly by hand instead. The single biggest win is making what exists reachable.
2. **Its senses judge frames and things; they don't measure time, relationships or physics.** Today's checks are mostly "does it exist" and "is this frame bad". Missing:
   - velocity, acceleration and jerk;
   - distances between objects over time;
   - gravity in keyed arcs;
   - formation spacing;
   - mesh-vs-intent error;
   - skin-weight sanity;
   - whether a write kept what it was told.

   The evals inherit that blindness and also have scorer bugs, so the numbers mislead.
3. **Bake paths lose information silently.** Several things go wrong without a word:
   - Keys are thinned or dropped while the reply reports the requested count.
   - Formula paths are rescaled and re-centred per object, which collapses a flock.
   - Five coordinate spaces are used with no single source of truth: canvas px y-down, formula z-up, model space `432/longest`, native mesh, and a 1/16.8 bake frame.

   Every one of these cost the film a visible defect.
4. **Formulas can be the source of truth, not triangles.** The film's assets were recovered as formulas at tiny error:

   | Asset | Before | After | Mean error | Note |
   |---|---|---|---|---|
   | Hero tree | 320,096 triangles | 13 numbers | 0.036% | |
   | Bear | 27,620 triangles | 16–19 terms | 0.13–0.25% | |
   | Clouds | 50–60k triangles | 4–9 blobs | about 0.05% | |
   | Jungle bush | — | exact textbook L-system | — | one leaf × 420 |

   For these, the mesh is the approximation: the error maps show tessellation faceting, not formula error. Zornov should keep the formula, render it directly or mesh it adaptively, and measure everything against it.

---

## 1. Superpowers Zornov already has

### 1a. Live and reachable
| Superpower | Where |
|---|---|
| Expression language: `sin cos tan atan2 exp sqrt pow mod fract step clamp lerp mix smoothstep if ease noise random wiggle length`, ternaries, `value_at`, `velocity`, `key_time`, `loop_in/out`, `inertia`, `prop()` | `crates/project/model/src/expr.rs:137-211` |
| Drivers: any property follows any other clip's property, per axis, with delay; recursion depth 8 | `crates/project/model/src/driver.rs`, op `driver` in `app/src/agent/drivers.rs` |
| Gait planner: phase by distance, curved paths, uneven ground, planted feet via IK, speed profile with curvature limit, 11 gaits | `crates/3d/mesh/src/perform/gait.rs` (via `act` with `route`/`to`) |
| Simulation suite: rigid, fracture, cloth (XPBD), soft, hair (PBD), particles, smoke and fire (stable fluids), water and ocean; SI dynamics with ragdolls and secondary chains | `crates/3d/sim`, `app/src/physics3d` |
| Dual-quaternion skinning, 2-bone analytic IK with poles, look-at, limits, copy-rotation | `crates/3d/mesh/src/rig.rs:718-846`, `crates/3d/rig/src/dq.rs` |
| SDF direct sphere-tracing on the GPU (`render: "direct"`) | `crates/picture/render/src/direct.rs` |
| Asset critics: topology, UV, PBR, silhouette IoU, chamfer, Hausdorff, symmetry | `crates/3d/quality` |
| Foot contact and slide measured on rigs (`inspect` with `pose`, `char_foot_slide`) | `crates/3d/mesh/src/perform/query.rs:128-172`, `perform/check.rs:108-139` |
| Flight recorder, friction report, replay to regression fixture | `app/src/agent/trace.rs`, `trace_report.rs` |
| Its own ONNX/TFLite/safetensors runtime with Metal and NEON | `crates/ai/nn` |

### 1b. Dark: built, but no caller, no skill, or the wrong default
| Capability | Status | Where | What it would have fixed in the film |
|---|---|---|---|
| Geodesic skin weights | no caller | `crates/3d/rig/src/weights.rs:230-266` | Bear belly and crotch tearing (101 cross-side influences) |
| Weight smooth, normalise, limit, mirror, paint | no caller | `crates/3d/rig/src/weights.rs` | Weight repair the analysis had to write in Python |
| FABRIK, soft IK, stretch IK | not used by the 3D rig; long chains fall back to CCD with the pole ignored | `crates/3d/rig/src/ik.rs:105-220`; `mesh/src/rig.rs:833-845` | Tails, necks, plant branches |
| Contact pins | no callers | `crates/3d/rig/src/pin.rs` | Paw locks on keyed walks (slide went 1,082 px → 0 only after hand-built pins) |
| Dual-contouring mesher (keeps sharp features) | retopology only | `crates/3d/quality/src/remesh.rs` | SDF parts rounded to about one cell (3.1 px at default detail) |
| GPU instancing and LOD | `build_world` only; `model`/`world` ops bake every copy | `crates/picture/render/src/realtime/gpu.rs:127-134`; `crates/3d/world/src/asset.rs:83-142` | 420 identical leaves per bush, 20k baked per scatter rule |
| FBX, USDA and PLY → Poly importers | no caller in `app/src` | `crates/3d/modeling/src/formats.rs:693-900` | Interop |
| Drivers as a wind field | the op exists; no skill teaches it | `app/src/agent/drivers.rs`; mentioned only in `guide.md` | 80 independent sine expressions instead of one field |
| `act` walk with gait on a route | exists; the film's agent keyed legs by hand because the skills point at `pose` | `app/src/agent/perform.rs:294-479` | Bear stance slip of 35 units per step |

**Action:** a "dark capabilities" pass. For each row, wire a caller or default, teach it in the matching skill (`physics-3d`, `living-places`, `puppetry-rigging`, `3d-assets-scenes`, `motion-animation`), and add an eval that only passes if the capability is used.

---

## 2. Engine bugs and traps (proven, ranked)

| # | Bug | Proof | Effect seen in the film | Fix |
|---|---|---|---|---|
| B1 | **`template: quadruped` assumes the body lies along x, head at −x.** It never checks the long axis or facing. `centre_joints` pins x for midline bones, a humanoid assumption. | `crates/3d/mesh/src/autorig.rs:292-328`, `:208-259`. **Probe** `superpowers_probe.rs`: a body along z gets a spine across its width, and "left/right" front legs end up 180 px apart *along the body* (one at the head end, one at the tail). | Bear torso tore apart on every step | Detect the long axis with PCA of the vertices, facing from the head/tail mass and thickness asymmetry, and up from the ground contact. Template in a canonical frame, then rotate in. Refuse with a message when ambiguous. |
| B2 | **`rig` bones are in model px (`432/longest·(native−centre)`, axes remapped `(x,−z,y)`) with no conversion or explanation.** Imports use 648, blended parts aren't refit, implicit shapes are scaled by the domain box. | `app/src/agent/anim3d.rs:169-183`; `math/mod.rs:37-39,76-88`; `models.rs:474-496,732,749` | Rig built from mesh-native numbers landed shrunk inside the body: 4,552 far-weighted verts vs 25 baked | `rig` takes `space: "model" or "native" or "formula"` and converts. `inspect` returns the model frame (scale, centre, axis map). Reject bones outside the mesh hull with a winding-number test. |
| B3 | **The formula `curve` bake rescales to 300 px (400 in 3D) and re-centres every object on its own bounding box**, then places it on the static position. Arc-length resampling discards the formula's timing. | `app/src/agent/math_shapes.rs:1039-1098`; `crates/vector/vector/src/curve.rs:237-242` | Five birds collapsed into one (closest pair 0.004 px); butterflies' loops turned vertical | Keep the formula live as a driver or modifier by default. If baking, keep absolute units (`scale` path), keep timing, never subtract per-object centres, and report the transform applied. |
| B4 | **Silent key loss on write.** Non-finite samples are dropped while the reply prints the requested count; RDP thins pose layers then writes Auto, which can drift past tolerance; half-frame merges; clip-end clamps collapse keys; speed changes dedup by µs. | `math_shapes.rs:1011-1019`; `perform/pose.rs:55-120`; `editor/xform.rs:291`; `services/character.rs:237`; `ops.rs:1185-1189`; counts are pre-drop at `craft.rs:532`, `anim3d.rs:321` | Bear walk: 37 keys written, 34 kept; the ease-out keys at 9.15/9.45/9.75 s were lost | Every write op returns `written`, `kept` and `dropped` with reasons. Any loss over 0 is a warning in `read_back`. RDP must be evaluated with the interpolation actually written. |
| B5 | **`auto` interpolation forces zero velocity at the ends and at every per-axis extremum, and is never C2.** A 2D point stops at turning keys. | `crates/project/model/src/keyframe.rs:579-609,421` | Camera stopped dead twice (acceleration jumps of 540 px/s²); bear legs eased to zero at every key (mid-swing speed 9.5% of peak) | Add `interp: "spline"` (C2 natural or centripetal Catmull-Rom with tension/continuity/bias) and `"through"` (C1, no zeroing at interior keys). Make camera and dolly use it. Lint `auto` on dense motion. |
| B6 | **`constraints: [...]` is aliased to `constraint`, so a list fails.** Limits apply in list order, so a limit listed before IK doesn't constrain it. | `app/src/agent/args.rs:44`; `anim3d.rs:234-237`; `mesh/src/rig.rs:760-767` | Every rig needed one op per constraint | Accept arrays. Order the solve as IK → limits → look-at, or make the order explicit. |
| B7 | **`rigid: true` welds an L-system tree into one piece.** Parent end caps and child start caps collapse to the same point; leaf poles sit on it. `rigid` is ignored with `template`. | `math/surface.rs:470-484`; `growth.rs:183-190`; `autorig.rs:459-517`; `anim3d.rs:185-201` | Tree rig fell back to one bone | Emit L-system branch IDs per vertex at generation time and bind by ID, not by welding. Offer `rig: {template: "branches", depth}` (the rigging lab's 63-bone tree proves it). |
| B8 | **`.smodel` can't be loaded as a model.** Zornov's own source format is missing from `import::EXTENSIONS`. | `crates/3d/mesh/src/import.rs:19` | Rig tests had to use the baked twins | Add `.smodel` (and `.smesh` round-trip) to `model` loading. |
| B9 | **Leaves and scatters are baked copies.** There's no instancing in `model`/`world`, `detail` is ignored for plants, and tubes default to 6 sides with one ring per turtle step. | `math_shapes.rs:685-774`; `world3d.rs:154-205` | 270–350k triangle trees; bark decimated to a budget that fragments it | Route `model`/`world` scatters and L-system leaves through the instancing path. Add `sides`/`segments` by screen size. Subdivide rings along curved steps. |
| B10 | **L-system taper is double-applied at forks.** `!` and `[` both multiply width, so presets using `![` thin twice per level; widths step per segment. Roll `/` turns the opposite way to the standard turtle convention. | `growth.rs:182-190`; jungle bush decoding | "No tapering, sizes look unnatural"; agents fought this with hand-tuned `!` counts | One width model: continuous taper along the branch (the 0.96 per segment the hero tree used, with no extra drop at forks) plus a pipe-model rule at forks (r_parent² = Σ r_child²). Document roll handedness, or flip it to the standard. |
| B11 | **SDF meshing is surface nets with no thin-feature guard for formulas.** Corners are rounded to one cell; features under 1–2 cells vanish. The field is normalised by a median gradient, not a true distance. | `crates/3d/mesh/src/sdf.rs:345-445`; `math/field.rs:263-295,437-446` | Petals 0.42 thick and wings rely on luck; ear tips are the worst error (1.2–1.6% Hausdorff) | Adaptive octree, dual contouring with QEF for sharp features, and a thinness pass that raises local resolution where `|∇f|` changes fast. Show a `detail` estimate before building. |
| B12 | **3D layers composite as 2D lanes.** A model lane above a 2D card draws its own backdrop over it. | Observed: the bear on the top lane rendered above the black fade card on an empty sky and ground | First 1.6 s of the WIP export showed the bear on an empty world | Composite all 3D lanes into one world pass and treat lane order only for 2D layers. Or warn in `check` when a 3D model sits above a full-frame 2D layer. |
| B13 | **The parts union keeps buried interior faces** (bird, butterfly); the editor bakes UV-ellipsoids. | Mesh analysis: Hausdorff 5.2% total vs 0.77% visible on birds | Wasted triangles; wrong error stats | Run `remesh::shell` (already exists) on blended or unioned parts by default. |
| B14 | **Scatter grass blade scale differs about 18× between scatter layers.** | Mesh analysis: blades 1.1–3.4 units in two scatters vs 18–55 in the third | Inconsistent grass density look | Check `world.scatter` units against the terrain scale. Add a lint for prototype scale variance between layers sharing a name. |
| B15 | **Side naming is inconsistent.** Bear `.L` is +x; bird and butterfly `wingl` is −x. | Puppeting analysis | A shared mirror would flip one rig | One convention (subject's left = −x in model space). Validate names in `rig`. |
| B16 | **Sound synthesis falls back to an unrelated nearest model** ("bear huff" → *traffic*; footsteps sound like applause). `listen` on a clip analyses the raw source, not the processed clip. The mix "hum" detector false-positives on bass notes. | `crates/sound/synth/src/sfx.rs:2171`; film session | The bear huff was deleted; foley needed manual EQ guessing | Refuse with options instead of substituting an unrelated class. Add an animal vocal model (breath plus formant growl) and surface-aware footsteps (mass, surface, gait timing from `act`). `listen` gets `processed: true`. Make the hum detector harmonic-aware (a tonal bass note vs a 60/50 Hz series). |

---

## 3. New senses ("eyes") to build

Each sense is a measurement with a threshold, wired into `check`/`review` as a note kind, exposed to `inspect`, and backed by an eval. All are deterministic code with no model needed; the analysis passes already ran prototypes in Python.

### 3a. Time and motion
| Sense | Measures | Threshold idea | Proven on |
|---|---|---|---|
| **Kinematics profile** | Per property and per world point: velocity, acceleration, jerk, sampled at render rate | Dead stop: speed under 2% of segment peak at an interior key. Acceleration flip over 300 px/s² inside 1 frame. RMS jerk over a budget. | Camera stops (543 px/s² steps); bear stride jerk 13,433 → 2,020 °/s³ |
| **Speed continuity** | Position speed jumps across keys and clip boundaries | Δv over 15% of local speed | Bear 63.7 → 37 → 0 px/s instant steps |
| **Heading vs facing** | Travel direction vs yaw for every moving body | Over 8° crabbing | Bunnies off by 6–17° |
| **Squash and stretch timing** | Phase of scale vs vertical speed and contact | Stretch peaking at an apex (vy ≈ 0); C0 squash at contact | All three bunnies |
| **Rhythm and coupling** | Shared frequencies, phase links, gait phase order (lateral sequence) | Report families; flag non-physiological gaits | 80 sways = one 3-parameter template; bear gait phase order |
| **Cue sync** | Distance of motion events (gust peak, landing, cut) from audio cues and beats | Over 2 frames | Gust peaked 1.16 s after the 5.5 s rustle cue |

### 3b. Relationships between things
| Sense | Measures | Proven on |
|---|---|---|
| **Pairwise clearance over time** | Minimum 3D distance between every pair of moving bodies, using swept bounding spheres then mesh distance | Butterflies passing through each other (1.3 px) |
| **Formation integrity** | Spacing matrix over time vs intended offsets | V collapsed to 0.004 px |
| **Contact and slide (generalised)** | `inspect` feet logic extended to any rig (paws, hooves, hands) on keyed motion, not only `act` | Bear slide 35 units per stance |
| **Ground truth by sample** | Terrain height under every grounded point at 5 moments (exists for footing; extend to scatters and every hop landing) | Bunny buried 10 px |

### 3c. Physics plausibility on keyed motion
| Sense | Measures | Proven on |
|---|---|---|
| **Ballistic fit** | Fit g, take-off speed and hang time to airborne segments; compare to the scene's metres-per-px | Bunny g = 3,918 px/s² → 399 px/m vs bear speed implying another scale |
| **Drag and terminal velocity** | Falling things accelerate from rest and settle at v_t; speed independent of release time | Petals fall faster the later they release (r = 0.997) |
| **Size scaling** | Sway frequency ∝ size^-0.5, wingbeat ∝ 1/size, amplitude vs height | No size scaling within species |
| **Scale consensus** | One metres-per-px implied by every moving body | Bunny vs bear disagree |

### 3d. Geometry and assets
| Sense | Measures | Exists? | Proven on |
|---|---|---|---|
| **Signed error heat** | Signed surface distance asset ↔ formula or reference, as a per-vertex map | Unsigned aggregates only (`views.rs:51-93`) | Bear heat at snout, shoulders, ankles |
| **Faceting vs smooth intent** | Deviation of mesh from its own implicit or limit surface; facet angle by screen size | Facet share only below 3k triangles (`three_d_check.rs:373`) | Sun lat/long faceting grid |
| **Hidden interior** | Faces fully inside other parts (winding number) | Internal remesh only | Birds 5.2% → 0.77% |
| **Seams** | Discontinuity of normal or curvature where parts meet; blend-radius estimate | Absent | Wing roots, ears |
| **Part segmentation** | Submesh/material split plus geometric segmentation (convexity, SDF primitive membership) | Partial | Eyes, nose, belly, tail patch separated |
| **Taper continuity** | Radius profile along each branch; flag steps at forks | Absent | "No tapering" complaint |
| **Repetition and instancing** | Identical sub-meshes (hash on normalised vertices) | Absent | 420 identical bush leaves |
| **Scatter statistics** | Density map, nearest-neighbour histogram, KS distance vs intended rule, patchiness | Absent | Meadow patches following a hidden mask |
| **Determinism** | Is a generated asset an exact rule or jittered? | Absent | Trees decoded exactly; decimated ones not |

### 3e. Rigs
| Sense | Measures | Proven on |
|---|---|---|
| **Skin-weight audit** | Far influences (over 1.5× nearest-bone distance), cross-side, non-adjacent in hierarchy, weight on unrelated chains | Bear 101 cross-side, 543 non-adjacent |
| **Pose-sweep stretch** | Max edge elongation across all keyed poses plus extremes (FK + skin) | 5.53× worst edge |
| **Bone placement** | Bones inside the hull (winding number), chain along the limb medial axis | Native-px trap: bones inside the body |
| **IK reach budget** | Pinned limbs vs body motion: when does a pin pop? | Hips past about 8° lifts paws 25–46 px |

### 3f. Data integrity (the "did it keep what I said" sense)
- Write-op accounting: requested, written, kept, dropped (B4).
- A round-trip check on every bake: evaluate the baked keys vs the source formula and report max/RMS error.
- Coordinate-frame echo: every op that takes points says which frame it read them in.

---

## 4. New engines and capabilities ("hands")

### 4a. Formula-first assets (the biggest unlock)
- **Inverse procedural fitting: mesh → formula.** A `fit` op:
  - SDF: ellipsoid and capsule blobs plus `smin`, with mirror folds detected.
  - L-system: ring-by-ring decoding, symbol counts, per-depth tables for jittered trees.
  - Instanced scatter: prototypes plus a placement rule.

  Each gives an error report and a signed heat map. The analysis passes did this in Python with exact results on the hero tree (0.036%), jungle bush (0.067%), primitives (exact), and clouds (about 0.05%).
- **Keep the formula as the asset.** Render with `direct` sphere tracing where it pays; otherwise mesh adaptively (B11). Edits are parameter edits, LOD is free, files are bytes.
- **Generators record their own grammar.** When Zornov builds an L-system or SDF, store the recipe beside the mesh (`.smodel` has `recipe: null` today) so nothing ever needs fitting back.

### 4b. Motion engine
- **Keys → closed-form compression** (`compress` op): fit piecewise ballistic, sinusoidal, ease or linear forms. The motion pass did 6,249 keys → 61 formulas; the bear walk went 1,632 keys → 19 scalars at max error 0.5°.
- **Motion profiles:** arc-length parametrised paths with explicit speed curves (cruise, accel, decel, C2). The gait planner already has the profile maths (`gait.rs:400-475`); expose it to any path, camera included.
- **Orbit and polar camera rig:** radius, azimuth and height as monotone splines around a subject. The motion pass's camera fix halved peak acceleration with no stops.
- **Flock and formation system:** boids with formation slots (V, line, echelon), per-member phase offsets, and a collision radius. The current fallback is the curve bake, which destroys formations (B3).
- **Hop and jump generator:** ballistic arcs from height and length, with velocity-driven squash, anticipation, C1 landing, pitch from velocity, and yaw = heading. Spec is in the motion analysis `bunny_*.json → improved`.

### 4c. Physics as fields, not per-object sines
- **Scene wind field `w(x, z, t)`:** gust spectrum plus a gust front travelling at speed c, mean lean (drag ∝ U²), and turbulence. It's readable by expressions as `wind(x, z, t)` or `prop("Wind", ...)`, and by cloth, hair, particles and smoke. The physics pass's model is there: per-plant parameters drop from 6 to 1, and the gust sweeps the field with delays from −1.45 to +1.22 s.
- **Plant response from size:** w_n = w_ref·scale^-0.5, A = A_ref·scale^0.3 (elastic similarity).
- **Falling-leaf model:** flutter phase drives rocking, tumble, glide toward the tilt, and a 2w descent bob, with a from-rest ramp and terminal velocity. The petal pass's coupled model is buildable today: `cos` and `exp` exist (`expr.rs:139,147`).
- **Flap and glide** without the velocity snap: a smoothstep-windowed burst (754 → 3 °/s).
- **Secondary motion on rigs:** the jiggle chains already exist in `sim/dynamics/secondary.rs`. Default them on tails and ears.

### 4d. Rigging engine
- **Axis-aware autorig** (B1), **space-aware `rig`** (B2), **geodesic bind by default** with the weight audit (3e) and auto-repair (midline gate, strip unrelated chains, Laplacian smooth). The puppeting pass measured cross-side influences 101 → 0.
- **Branch skeletons from L-system IDs** (B7): one bone per segment to depth N, with wind driving per generation and a lag.
- **Pins and foot plants for keyed motion,** not only `act`:
  - `pin_paw_*` control bones with 2-bone IK and poles;
  - stance keys solved from root speed and duty factor.

  The rigging lab took slide from 1,082 px to 0.
- **Gait from root speed** for any keyed path: amplitude = asin(v·T·DF / 2L), duty factor about 0.62.

### 4e. Surfaces and look
- **True taper and pipe-model forks** (B10). **Instanced leaves** (B9).
- **Seam blending:** an automatic `smin` radius where parts meet, sized from the seam sense.
- **Fur and skin detail as displacement or normal maps from the noise term**, not geometry. Fur noise RMS was 0.33 units on the bear and non-periodic.
- **Terrain:** keep the generator. The recovered 32-term cosine series fits the shape but isn't the generator, which was broadband noise.

### 4f. Sound
- Animal vocal synthesis, surface-aware footsteps synced from `act`/gait contact events, and refusal instead of an unrelated fallback (B16).
- `listen` on the processed clip, and a harmonic-aware hum check.
- **Contact events as a sound bus:** every foot plant, hop landing and branch rustle peak emits an event that foley can follow automatically.

---

## 5. MCP and agent surface

What made a strong agent slow or wrong while building this film, observed first-hand:

| Friction | What happened | Fix |
|---|---|---|
| Replies too big | `viewport frame` returns every object in the scene (about 150 entries with positions and sizes) with the picture; `summary` lists every lane and clip | Scope by default (`objects: "in_view"`, `limit`). `summary` with `since`/`lanes`. Enforce the budget.rs caps on these tools too. |
| Lane order traps | `lane add` lands at the top; `move` takes an absolute index after removal; the top lane drew 3D over 2D (B12) | `lane add {after: id}`, `lane move {after/before: id}`, warnings for 3D above full-frame 2D |
| `check` is slow and opaque | Minutes with no progress; the user aborted it | Progress events, a time budget, `scope` (clips, kinds), incremental re-check by revision |
| `export` blocks | 166 s blocking; `wait: false` + `export_status` exists but isn't suggested | Default async for long exports; say so in the reply |
| GUI and MCP diverge | The headless MCP project and the open editor window differ until save/reopen | Attach the MCP to the running app (the trace shows an `attach` source) and make it the default when the app is open |
| Property names guessed | The #1 eval failure: `scale`, `transform.scale`, `opacity` %, `curve_r` | `edit_schema` per clip lists canonical keyable paths. Errors return "did you mean". `keys` of `inspect` should list valid paths. |
| Coordinates ambiguous | `region` fractions vs px; `rig` model px; formula z-up vs canvas y-down | One `frames` guide topic. Every point-taking field accepts `{space}`. Every reply echoes the frame used. |
| `inspect` lacks rig introspection | No `bones` section (head/tail, weights per bone) | `inspect {what: "rig"}` returns bones, constraints, a skin-weight summary and audit notes |
| `contact_sheet` can't solo | Needed full-scene sheets to see one animal | `clips: [ids]` solo/isolate on `frame`/`contact_sheet` |
| No "no generation" policy | The user banned generated media for this project; enforcement was by agent discipline only | A project policy `generation: off` the server enforces on `generate`/`ai`/`model3d` |
| Silent success | Write ops report requested counts (B4) | Every `read_back` carries a loss report |
| Rigs built blind | Bones are only visible as renders | `viewport` overlay `bones: true` and `weights: <bone>` heat |

**New tools:**
- `fit` (mesh → formula, with error map)
- `compress` (keys → formula)
- `measure` (any sense in §3 on demand: `kinematics`, `clearance`, `ballistics`, `weights`, `seams`, `scatter_stats`, `taper`)
- `field` (wind and other scene fields)
- `flock`
- `hop`
- `audit` (run all senses on a scope and return ranked fixes with verified numbers)

---

## 6. Evals: make the score mean something

### 6a. Scorer bugs (from `app/src/agent/eval.rs`)
- **`fix_colour` is 0 in both runs** because the colourist used `colour`, not `edit`, and `eval.rs:450` zeroes every check when no tool named `edit` ran. Compare project revisions instead.
- **`honest_animation` and `steer_title` are false fails:** `animated_clips` (`:242-255`) and the title read (`:341`) ignore nested sequences and 3D text. True score 100 each.
- **`music_video` beat alignment** scored with 0 beats detected. Assert that detection succeeded before grading.
- **Concurrent runs wipe each other** (`remove_dir_all` at `:358`). Use a unique folder per run, or a lock.

### 6b. Metric-graded tasks (the film is the fixture)
Each task grades with §3 senses, not existence:

| Task | Graded on |
|---|---|
| **Creature walk** | Foot slide px per stance, dead stops, heading error |
| **Wind forest** | Shared-field usage, gust-front delay spread, size scaling, cue sync |
| **Flock** | Formation spacing error, pairwise clearance |
| **Formula fidelity** | Build the hero tree from its 13 numbers; Hausdorff vs reference under 0.3% |
| **Hop** | Ballistic g consistency, squash phase, crabbing |
| **Rig** | Weight audit counts, pose-sweep stretch, bones inside hull |
| **Integrity** | Keys written = kept; bake round-trip error |

Run `studio-quality bench` and the eval in CI nightly, with history.

---

## 7. Own models

Today the runtime (`crates/ai/nn`) is Zornov's own, but all 15 models are third-party weights (`docs/models.md`).

**Unfair advantage:** every Zornov generator knows its ground truth. Rendering formula assets, rigs and motion produces unlimited, perfectly labelled, licence-clean data.

| Model | In → out | Replaces |
|---|---|---|
| Inverse procedural | picture or mesh → SDF/L-system parameters | slow fitting in `fit` |
| Rig placement | mesh → joints and facing | the axis-blind template (B1) |
| Motion critic | motion → "off" score, trained on §3 metrics as labels | look-only review |
| House taste | frame or clip → preference, from `taste` store plus logged corrections | generic judgement |

Path: generate the dataset from generators → train out of tree (torch) → export ONNX → load with `studio-nn`. Gate each behind a §6 metric so a model only ships when it beats the code path.

---

## 8. Order of work

1. **Integrity and traps** (days):
   - B4 key-loss reporting, B6 constraint arrays, B8 `.smodel` load, B12 3D-over-2D warning;
   - eval scorer fixes (6a);
   - lane `after/before`, property-name suggestions.
2. **Rig correctness** (1–2 weeks): B1 axis-aware autorig, B2 rig spaces, geodesic bind and weight audit, B7 branch IDs and branch skeleton, pins for keyed walks.
3. **Motion core** (1–2 weeks): B5 C2/through interpolation, kinematics/speed/heading senses, B3 live formula paths, hop generator, orbit camera.
4. **Fields** (1–2 weeks): scene wind field with expression access, plant size scaling, falling-leaf model, cue-sync sense.
5. **Formula-first geometry** (multi-week): recipe storage, adaptive DC meshing with thin-feature guard (B11), instancing everywhere (B9), taper model (B10), `fit` with signed heat, seam sense.
6. **Senses as a product** (ongoing): `measure`/`audit`, metric evals in CI, dark-capability wiring with skill updates.
7. **Own models** (after 5): dataset from generators, inverse-procedural and rig-placement models first.

---

## Appendix: film numbers this map rests on
| Area | Before → after (verified) |
|---|---|
| Hero tree | 320,096 triangles → 13 numbers, 0.036% mean |
| Bear | 27,620 triangles → 16–19 SDF terms, 0.13–0.25% mean |
| Clouds | 50–60k triangles → 4–9 blobs, about 0.05% mean |
| Motion | 6,249 keys → 61 formulas, max 0.2 px at keys |
| Petals | 6,646 keys → 70 expressions (15× fewer numbers) |
| Bear walk | 1,632 keys → 19 scalars, max 0.5° |
| Bear foot slide | 35.4 → 0.21 units per step (gait fix); 1,082 px → 0 (pins) |
| Bear skin | cross-side influences 101 → 0; worst edge stretch 5.53× → 4.74× |
| Camera | 2 dead stops → none; peak acceleration 397 → 194 px/s² |
| Birds | formation closest pair 0.004 px → 8.3 px (restored V) |
| Butterflies | closest pass 1.3 px → 60 px |
| Gust | peak moved onto the 5.5 s cue |
