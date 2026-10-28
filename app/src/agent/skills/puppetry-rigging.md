---
name: puppetry-rigging
use: Use when a 2D character must move, talk or act: pins, rigs, skeletons, cuts, faces.
topics: puppetry, visemes, viseme, phonemes, phoneme, mouth-chart, jaw, tongue, puppet, rig, rigs, rigging, scissor, scissoring, cut-lines, acting, life, dialogue, facing, talking, conversation, animals, lip-sync, lipsync, mouth, blink, eyes, gaze, skeleton, skeletons, bones, bone, ik, pose, poses, walk-cycle, parts, swap, mocap, motion-capture, turnaround, character, characters, characters-library, mask-puppet, blinks, library, lips, separate-limbs
tools: puppet, lip_sync, narrate, scissor, skeleton, pose, ik, aim, parts, swap, retarget, character, mocap, mask_puppet, foley, shadow, light_match, set, animate, frame, contact_sheet, check, review
---
**Order**
1. One character, one layer: each character is its own transparent picture (`generate` with `transparent: true`, or `cut_out`). Never reuse one cast sheet for several characters. Register a recurring one with `character` so every shot uses the same look, rig and poses.
2. Choose the rig by what it must do. Loose `puppet` pins (with `rig`: biped, quadruped, bird, fish, plant, object) for quick life, cloth, hair, a backdrop's breeze. A `skeleton` (`build`) when it walks, reaches or performs: bones keep limbs their length, joints bend the right way, `ik` plants a foot.
3. Cut what must move apart (`scissor`, or `auto_cut`): between the legs, under the arms, round a tail, along the lips with `fill: "mouth"`. Read the rig reply; draw the cuts it missed. Never walk a character whose legs touch, never talk a face whose mouth is not cut.
4. Facing: read which way the reply says it faces; `set flip: true` if wrong. Talkers face each other and keep their sides for the scene.
5. Act in windows, one `act` call per stretch of seconds (a later call over the same seconds replaces it): `breathe` / `idle` under everything, `talk` for the length of a line with the line's `voice` clip (the mouth follows it), listeners `listen` / `nod` / `laugh` / `think`, feelings `surprise` / `scared` / `sad` / `celebrate`, travel `walk` / `run` / `sneak` / `hop` / `swim` with `travel`. Add `blink: true` and `look_at` for every face.
6. Performance: block poses first (`pose` keys), then in-betweens; anticipate and let limbs settle. Parts (`parts`, `swap`) when the head must turn or mouth shapes swap. `mocap` is a start: lower `strength` for animals, push the key poses on a cartoon.
7. Footsteps: `foley` on the frames the feet land. Ground the character in the place: `shadow`, `light_match`.
8. Look at a frame mid-line and mid-blink, and at the widest stride.

**Checks**
- The speaker's head moves, eyes and beak point at the listener, feet stay planted unless it walks.
- At the poses where parts are furthest apart (widest stride, mouth fully open) no gap shows the background.
- `check` and `review` for `faces_away`, `slides`, `same_acting`, `moves_alike`.

**Mistakes**
- A character that only slides or scales across the frame: it is a sticker. Give it a body that moves.
- Every character on the same act and key times: each has its own tempo; the listener reacts after the speaker.
- A cut that runs across a joint detaches the limb; end cuts where parts join.
- Redrawing a character per shot from a new prompt: change face, colours, proportions.

## Reference: Characters: life, facing and dialogue

A character that only slides or scales across the frame is a sticker.
Give every character on screen a body that moves, and stage them so they
read as talking to each other.

- **One character, one layer.** Generate each character on its own with
  `transparent: true` (or `cut_out` it from a picture). Never reuse one
  cast sheet for several characters: copies share one pose and one
  facing.
- **Rig it, then act.** `puppet` with `"rig": "bird"` (`biped`,
  `quadruped`, `bird`, `fish`, `plant`, `object`) places named pins on
  the silhouette (head, body, tail, feet, hands, top), with a hip and a
  knee above each foot so a step bends the leg instead of dragging the
  body. Name pins yourself (`left_knee`, `tail_tip`…) for more joints:
  acts read the names. `"act"` bakes motion into them: `breathe` and
  `idle` under everything; `talk` while it speaks; `listen`, `nod`,
  `shake_head`, `look`, `think`, `point`, `wave` in conversation;
  `surprise`, `laugh`, `scared`, `sad`, `celebrate` for feelings; `peck`,
  `wag`, `swim` for animals; `hop`, `walk`, `run`, `sneak` (`travel` px
  moves it across); `sway` for plants. Acts combine in a list, run for
  `from`/`duration` seconds of the clip, and `speed` sets the tempo; each
  character moves in its own time, so two doing the same act are never
  in step. One `act` call per window: a later call over the same seconds
  replaces it, so split a clip into windows (a line each) and give each
  window its own acts.
- **Nothing stands frozen.** Every character on screen breathes or idles
  at least; whoever speaks gets `talk` for the length of the line, and
  the listeners react (`listen`, `nod`, `laugh`…) rather than holding
  still.
- **Faces that talk and look.** Whoever speaks gets `"voice"`: its
  mouth follows the line (`"act": ["talk"], "voice": <line clip>`), and
  it blinks. Every character on screen blinks (`"blink": true`) and looks
  at whoever is talking (`"look_at": <speaker>`); the speaker looks at the
  one it talks to. A still face reads as a doll. Look at a frame mid-line
  and mid-blink: if the mouth or eyes moved the wrong spot, correct them
  with `"face": {"eyes": [[x, y], …], "mouth": [x, y]}`.
- **Separate what moves apart.** A rig bends one connected mesh, so
  anything that touches moves as one attached blob: a stepping leg drags
  the other, a raised arm pulls the chest, an opening mouth stretches the
  cheeks. Scissor the character (`scissor`, or `"auto_cut": true`): cut
  between the legs from the crotch down, under each arm, around a tail
  (root left joined), and along the lips with `"fill": "mouth"` so the gap
  shows a dark inside. Rigging a biped or quadruped already cuts legs,
  arms and a drawn mouth; read the reply, and draw the cuts it missed.
  Never walk a character whose legs touch without cutting them, and never
  talk a face whose mouth is not cut.
- **Facing.** The `puppet` reply says which way a character faces on
  the canvas. If it is wrong, `set` `"flip": true` mirrors the layer
  (its pins and motion mirror with it); rig with `"facing"` when the
  silhouette can't tell.
- **Dialogue staging.** Characters who talk face each other: the one on
  the left faces right, the one on the right faces left, and they keep
  those sides for the whole scene (the 180° rule: crossing it makes them
  seem to turn away). The speaker looks at the listener, never out of
  the frame, unless the line is meant for the audience. A character that
  walks faces where it walks.
- **Check it.** Look at frames from the middle of a line (`frame`): the
  speaker's head is moving, eyes and beak point at the listener, feet
  stay planted unless it walks.

## Reference: Character animation: skeletons, parts and performance

- **Bones before pins.** A character that walks, reaches or performs
  gets a skeleton (`skeleton` `build`), not loose pins: bones keep
  limbs their length, joints bend one way (knees back, elbows forward)
  and a foot can be placed while the leg follows (`ik`). Pins stay for
  cloth, hair, flames and a backdrop's breeze.
- **Pose to pose, then in-betweens.** Block the story poses first
  (`pose` `keys`: the contact, the reach, the hit), each readable as a
  silhouette, then let the curves in-between. Hold a pose a few frames
  before a big move (anticipation) and let limbs settle after it
  (overlap: the forearm lands a beat after the upper arm, the tail
  after the hips). Save the poses a character returns to (`save`) and
  reuse them (`apply`) so it moves like itself all film.
- **Walks have weight.** A walk is two steps a second for an adult
  (`speed` 0.8 for a heavy or old one, 1.3 for a child), the body
  lowest at the contact and highest as the legs pass; leave out `travel` and
  the reply gives the `travel` (px) that keeps the planted foot still. A run leans forward and leaves the
  ground; a sneak crouches and slows. Never slide a character across
  the frame without its legs going.
- **Plant the feet.** In a scene where a character stands and gestures,
  key the feet with a live `ik` handle on each so the body can lean,
  bob and turn while the feet stay put; let go (`off`) before it walks.
- **Scissor before you pose.** Bones pull the picture around them, and
  where two limbs or a limb and the body touch in the drawing the mesh
  joins them. Cut the mesh along what must part (`scissor`; `skeleton`
  `build` does the legs, arms and mouth for a biped or quadruped): the
  gap between the legs from the crotch down, the slit under each arm, the
  lips (`fill: "mouth"` for its inside; stop short of the corners), a
  tail's sides. The picture stays whole at rest and opens only when the
  sides move, so check the poses where parts are furthest apart (the
  widest stride, the mouth fully open) and fill any gap that shows the
  background. Cuts end where parts join: a cut that runs across a joint
  detaches the limb.
- **Parts for performance.** Split a character into parts (`parts`)
  when it must turn its head, swap a hand pose or lip sync on separate
  mouth shapes; a part layer turns rigidly with its bone, and each
  limb keeps a round cap at its joint so turns open no gap. Order them
  with bone `depth` (the far arm behind the body, the near one in
  front).
- **Swap, don't stretch.** Mouth shapes, blinks and hand poses read
  best as drawings swapped on the frame (`swap`), the way 2D studios do it; make the shapes from the character
  itself (`character` `generate` `mouths`) so they match its style.
- **One character, one look.** Every shot of a character comes from its
  library entry (`character` `use`), made from the same reference and
  description, with its rig and poses; a character redrawn per shot
  from a new prompt changes face, colours and proportions, and the
  audience notices.
- **Motion capture is a start, not the end.** Captured motion
  (`mocap`) is real weight and timing, but it is a person's: on an
  animal, lower `strength` and keep the spine's turn small; on a
  cartoon, push the key poses further after capture (`pose` on top) and
  cut the jitter with more `smooth`. Check it with `frame` at the
  extremes: a knee bent backwards or an arm through the body is a
  mapping to fix (`map`, `swap_sides`, `mirror`).

## Reference: Puppet pins

**Puppet pins** (the Puppet tool): `puppet` places pins on a
layer (picture, video, title, shape) and bends it by moving them; the
mesh follows only what the layer draws, so an arm bends without
dragging the body beside it. Points are canvas pixels from the centre
at the moment `at` (default the clip's start), a name of the layer's
box (`top_left`, `centre`…) or `{"u": 0..1, "v": 0..1}` across it.
Kinds: `position` (holds its point where you move it; two or more bend,
one alone moves the whole layer), `bend` (`rotate` degrees and `scale` %
around it), `starch` (`amount` 0–100 % stiffens within `extent` %),
`overlap` (`amount` −100…100: that part in front where folds cross).

    {"op": "puppet", "clip": 7, "pins": [{"name": "body", "at": [0, 60]}, {"name": "hand", "at": [140, -40]}]}
    {"op": "puppet", "clip": 7, "pin": "hand", "keys": [[0, [140, -40]], {"t": 0.6, "by": [30, -90], "ease": "overshoot"}, [1.2, [140, -40]]]}

A key moves the pin to `to` (canvas pixels) or `by` pixels from where it
was placed; `rotate`, `scale`, `amount`, `extent` key the other
settings. `pin` + `to` / `by` without keys sets it (a key at `at` once
it is animated). `remove`: a pin or `"all"`. Keys show on the timeline
and in `inspect` (`pins`); look with `frame` to check the bend.

## Reference: Rigs and acting

**Rigs and acting**: `"rig": "bird"` (biped, quadruped, bird, fish,
plant, object) places named pins on the character's silhouette (its
transparency, plain backdrop or cut-out); `"facing": "left"` when it
can't tell. `"act"` bakes life into those pins: `breathe`, `idle`,
`talk`, `listen`, `nod`, `shake_head`, `look`, `think`, `point`, `wave`,
`surprise`, `laugh`, `scared`, `sad`, `celebrate`, `peck`, `wag`, `hop`,
`walk`, `run`, `sneak`, `swim`, `sway`, one or a list, over `from` / `duration` seconds of the clip at
`speed`; `travel` px moves it across while it walks. The reply says which
way it faces on the canvas; `set` `"flip": true` turns it round. See the
craft book's `puppetry`. `"rig": "backdrop"` pins a whole background
(ground held, top and sides free) for `"act": "breeze"`: a place that
lives. Staging a story: the craft book's `staging`.

    {"op": "puppet", "clip": 7, "rig": "bird", "act": ["idle", "peck"]}
    {"op": "puppet", "clip": 9, "act": ["breathe", "talk"], "from": 2, "duration": 3.5}

## Reference: Scissoring

**Scissoring** (cut lines): a puppet or skeleton bends one connected mesh,
so where parts touch they drag each other: a stepping leg stretches the
other leg, an opening mouth pulls the cheeks. A cut is a path across the
picture along which the mesh splits; the two sides then move on their own,
the picture stays whole at rest and opens only where they move apart.
Cut between the legs (from the crotch down), along the lips (stop short
of the corners so they stay joined), under an arm, around a tail (leave
its root joined). `"cuts"` on `puppet` or `skeleton`, or the `scissor` op,
draws them: `[{"name": "mouth", "path": [[x, y], …], "fill": "mouth"}]`
(points like pins: canvas pixels, `{"u", "v"}`; at least two). `fill`
shows something in the gap that opens: `"mouth"` (a mouth's dark inside),
a colour (`"#601010"`), `"auto"` (the picture's own colour at the cut,
darkened) or `"none"`. `"auto_cut": true` finds them from the silhouette
and the face: narrow gaps between limbs, the notches at the crotch and
armpits, the lip line (snapped to a drawn mouth when the picture has
one), the borders between a skeleton's parts. **Rigging a biped or
quadruped (`puppet` `rig`, `skeleton` `build`) cuts its legs, arms and
mouth by default** (`"auto_cut": false` to skip); a mouth that was only
estimated, with no drawn lip line to find, is proposed in the reply and
not applied (`"auto": ["mouth"]` applies it). `scissor` also moves
(`cut` + `path` / `by`), re-fills (`fill`), renames, switches (`enabled`)
and `remove`s one (an id, a name or `"all"`), or `list`s them; `auto` takes
`true`, `legs`, `arms`, `mouth`, `gaps` (any narrow gap), `parts`, `all`.
Cuts need something to move the sides: pins (`puppet`) or bones
(`skeleton`). Where a cut ends the mesh stays joined, so it splits only as far as it
runs: to part two pieces completely, run it out to empty space or the
picture's edge; to leave a joint (hip, shoulder, mouth corner), end it
short of it.

    {"op": "puppet", "clip": 7, "rig": "biped"}
    {"op": "scissor", "clip": 7, "cuts": [{"name": "mouth", "path": [[-12, 52], [12, 52]], "fill": "mouth"}]}
    {"op": "scissor", "clip": 7, "auto": ["legs", "arms"]}

## Reference: Faces

**Faces**: `"voice"` (the clip of a spoken line) moves the mouth with
that line, opening on its syllables, shaped by its words (wide for "ee",
round for "oo", shut for m, b, p) and shut in its pauses; the window
becomes where the line plays. `"blink": true` (or the average seconds
between blinks: 4 natural, 2 nervous, 8 calm) blinks at irregular
times; `"look_at"` (a clip, or `camera`, `left`, `right`, `up`, `down`)
turns the eyes to it. `"act": "talk"` with a `voice` blinks too. A
one-picture character gets face pins (`face eye1`, `face eye1 lid`…,
`face mouth`, `face lip`) found in its picture: eyes from pupils in their
whites, the mouth from the dark line below them. The reply says where
they went: look with `frame`, and correct them with `"face": {"eyes":
[[x, y], …], "mouth": [x, y], "mouth_width": px}` (canvas pixels). Face
pins ride on the head, so acts and face motion go on in any order. A
character made of parts: layers parented to it named `mouth closed`,
`mouth open`, `mouth wide`, `mouth round`, `eye`, `lid`, `pupil` (or
`"parts": {"mouths": {"open": id, …}, "eyes": [ids], "lids": [ids],
"pupils": [ids]}`): mouths swap, lids show for a blink (else the eyes
squash), pupils slide.

    {"op": "puppet", "clip": 9, "act": ["breathe", "talk"], "voice": 31, "look_at": 12}
    {"op": "puppet", "clip": 12, "act": "listen", "from": 2, "duration": 3.5, "blink": true, "look_at": 9}

## Reference: Skeletons

**Skeletons** (bones): a
character picture gets real bones with parents that bend it (each bone
pulls the picture around it, measured through the picture, so one leg
moves without the other) and carry its part layers. `skeleton` with
`"build": kind` (biped, quadruped, bird, fish, snake, plant, object)
draws bones on the silhouette (transparency, plain backdrop, cut-out,
or the object SAM finds in the middle of the picture),
named by role: `hips` (the root), `spine`, `neck`, `head`,
`upper_arm.front` / `forearm.front` (`.back` for the far side),
`thigh.front` / `shin.front`, a quadruped's `front_leg.upper` /
`.lower` and `back_leg.…`, `tail.1`…, `trunk.1`…; knees and elbows get
joint limits that bend the right way for where it faces (`"facing"`:
right, left or camera, as it looks on the canvas). Draw your own with
`bones` (`[{"name", "parent", "head": point, "tail": point, "min",
"max", "depth"}]`); `bone` + `min` / `max` (degrees from rest),
`depth` (−1 behind … 1 in front where parts fold over), `deform: false`
(a control bone), `rename`, `parent`; `remove` a bone or `"all"`. A
point is `[x, y]` canvas pixels, `{"u", "v"}` across the layer, or a
bone's name (its tip). `act` (on `skeleton`, or `puppet` on a clip with
bones) bakes motions into bone keys on top of the pose at `from`:
`idle`, `breathe`, `walk`, `run`, `sneak`, `hop`, `jump`, `celebrate`,
`wave`, `point`, `nod`, `shake_head`, `look`, `talk`, `listen`, `think`,
`sad`, `scared`, `surprise`, `laugh`, `peck`, `wag`, `swim`, `slither`,
`sway`, `breeze`; legs swing in turn with knees folding through the
step, arms against them, the body bobbing. `"travel": "auto"` moves the
layer at the gait's own pace so the feet do not slide (a number: px).

    {"op": "skeleton", "clip": 7, "build": "biped", "facing": "right"}
    {"op": "skeleton", "clip": 7, "act": ["walk"], "from": 0, "duration": 4, "travel": "auto"}

`pose` sets bones now (`"bones": {"upper_arm.front": 120, "head": -10}`
degrees from rest, clockwise; `"key": true` for keys; `offset` moves a
bone by `[dx, dy]` px, the root moves the body), pose-to-pose `keys`
(`[{"t": 0.5, "bones": {…}}, {"t": 1.2, "apply": "contact"}]`), the
pose library (`save` a name, `apply` it, or `"Mia/wave"` from the
character library), `copy` (returns the pose) and `paste`, `mirror`
(front and back limbs trade), `reset` (rest). `ik` places a hand or
foot and the limb follows: `"bone": "front foot"` (or the last bone,
`chain` bones up, default 2) `"to"` a point, baked into the bones'
rotations (`"key": true` for a key), within their limits; `flip` bends
the joint the other way. With `keys` (`[[t, point], …]`) or
`"live": true` the handle stays on: the foot holds its keyed target
(planted feet, a hand on a table) whatever the body does; `off` lets go.
Handle settings (they make it live): `pole` (a point or bone name the
middle joint bends toward: knees forward, elbows back; `false` clears
it), `stretch` (`true`, or the longest the limb may grow, as a factor,
`1.5`: it lengthens to cover a far target instead of stopping short;
`false` rigid), `soft` (0..0.5: the limb eases into straight over the
last part of its reach, no snapping knee), and `pin` (`true` plants the
end where it is now, a foot on the ground while the body moves; `false`
lets go). A target past the reach, with no stretch, gets a straight limb
pointing at it. Joint limits are the bones' `min` / `max` (`skeleton`
`bone` … `min`, `max`): the solver bends the way that respects them.
`aim` turns one bone to face a point or another bone (a head to a
speaker, an eye to a ball, a gun arm to a target): `{"op": "aim",
"clip": 7, "bone": "head", "to": "front hand", "limit": 50}`, with
`keys` / `live` / `blend` like `ik`, and `limit` the most it may turn
(degrees each way from rest). `retarget` moves one rig's animation onto
another of other proportions: `{"op": "retarget", "from": 3, "clip": 7}`
keys every mapped bone of clip 7 to point the way its partner in clip 3
does (partners by role: `thigh.front` to `thigh.front`, `upper_arm.L`
to `upper_arm.front`; `map` `{target bone: source bone}` adds or fixes
pairs), with the root's travel scaled to the leg length and a rig
facing the other way mirrored; `start` / `duration` pick the part of
the source to take.

    {"op": "pose", "clip": 7, "keys": [{"t": 0, "bones": {"upper_arm.front": 10}}, {"t": 0.6, "bones": {"upper_arm.front": 150, "forearm.front": -30}}]}
    {"op": "ik", "clip": 7, "bone": "front hand", "to": [220, -40], "key": true}

## Reference: Characters made of parts

**Characters made of parts**: `parts` splits the rigged picture into
part layers (head, torso, arms, legs, tail; with `face`, the default,
the eyes and mouth too, outlined by SAM where it can, the face under
them patched), each on its own
lane above the rig, parented to it and following its bone; the rig
layer hides (opacity 0) and carries them, so `pose`, `act`, `ik` and
`mocap` on it move the parts. Bone `depth` orders them. **Swap sets**
(mouth shapes, hand poses, eye states): `swap`
with `set` and `add` (`{"open": picture, "wide": picture}`: files or
media; a picture the rig's size lines up with it, a smaller one goes
`place` (a point) at `width` of the rig's width) hangs each option on a
bone (`bone`, else the head for mouths and eyes); `keys` (`[[t,
option], …]`, seconds of the rig clip) or `show` switch them on the
frame. The face layers are named as lip sync looks for them ("mouth
closed", "mouth open", "eye front"), so `puppet` `voice` and `blink` on
the rig drive them.

## Reference: Character library

**Character library** (one look across shots): `character` with
`create` (a name), `from` (the reference: a clip, media or file),
`kind` and `description` (its look in words) keeps it in the project's
`Characters` folder, with the clip's rig and pose library. `generate`
makes more pictures from the reference so they keep its design:
`"turnaround"` (side, back, three-quarter views) or `{"views": […],
"poses": ["running", "sitting"], "expressions": ["happy", "angry"],
"mouths": ["closed", "open", "wide", "round"], "hands": […]}`
(`estimate: true` says what it would make, sending nothing; `provider`, `aspect` pass
through to generate). `use` (`"front"`, `"side"`, `"pose/running"`…) puts one on
the timeline (`at`, `duration`, `lane`, `x`, `y`), rigged when it is
the reference; `mouths_on` (a rig clip) makes its mouths that rig's
mouth swap set; `save_rig` (a clip) stores a rig; `list`.

## Reference: Motion capture

**Motion capture**: `mocap` drives a skeleton from a video of a person
(`video`: a clip, media or file; `from` / `duration` seconds of it, at
`rate` fps, default 15; keys from the rig clip's `at`). Body points
come from the pose model, smoothed (`smooth` 0–1) with hidden points
bridged; each limb drives the bone of its role. A bone whose rest
matches the person's limb (hanging arms, standing legs) takes its
direction; one that does not (a dog's level spine, a quadruped's legs
from the arms) takes its turn from the first frame. `strength` scales
the turns (0.6 for an animal's stiffer legs), `mirror` flips the
person, `swap_sides` puts their left on the back limbs, `root`
(full, vertical, none) moves the body with their hips (scaled by leg
length), `map` (`{"tail.1": "left_forearm"}`) overrides by bone. The
reply says which bones followed which limb and how many frames found a
person: film the whole body in good light.

    {"op": "mocap", "clip": 7, "video": 12, "smooth": 0.6, "mirror": true}
    {"op": "mocap", "clip": 9, "video": 12, "strength": 0.6, "root": "vertical"}

## Reference: Mask pins

**Mask pins**: `mask_puppet` does the same to a mask's outline (`mask`
1…; the shape becomes a path first), and `point` (1…, clockwise from
the top) with `to` / `by` / `keys` moves one point of it. A mask cuts
the picture before the puppet bends it, so a layer's mask always
follows its puppet. **Corner pin**: `corner_pin` puts the layer's box
corners anywhere (perspective: a screen replacement, a sign on a wall),
`skew` in degrees slants it; `keys` per corner animate them.

## Reference: Lip sync: real mouth sync for any character

**Order**
1. Make the line first: `narrate {say, speaker: <character clip>}` speaks it and syncs the speaker's mouth in one step (`scene` takes `speaker` too). Phonemes are timed on the sound and kept beside the file, so later syncs reuse them. A recorded or imported line has no phonemes yet: `lip_sync` hears its words (speech recognition) or reads the words file beside it.
2. `lip_sync {clip: <character>, audio: <line clip>}`. Leave `audio` out and the voice clip under the character is used. It works on:
   - a rigged 3D model: face shape keys (jaw open, lip close, pucker, stretch, roll, tongue, blinks, brows) found by name in any case (`Mouth_Open`, `jawOpen`, `viseme_aa`…), or one key per viseme, plus jaw, tongue and eye bones. Keys land on `generator.shape.*` and `generator.bone.*`: edit them in the timeline and graph like any keys;
   - a puppet with face pins (`puppet {rig…}` or `"face"`): lips follow the same timing;
   - a character of parts: name a drawing per viseme (`mouth rest`, `mouth MBP`, `mouth FV`, `mouth TH`, `mouth L`, `mouth R`, `mouth WOO`, `mouth CHSHJ`, `mouth SZ`, `mouth AA`, `mouth AE`, `mouth AH`, `mouth EE`, `mouth IH`, `mouth OH`, `mouth UH`, `mouth ER`, `mouth etc`). A smaller chart works: a missing viseme shows its nearest drawing (the plain `closed/open/wide/round` four work);
   - an animal: a jaw bone (or `jawOpen`) opens softly with the sound; lips are left alone.
3. Shape it: `intensity` (1 natural; 0.5 mumbling; 1.4 theatrical), `emotion` (happy widens and smiles, sad droops, angry shows teeth, surprised drops the jaw), `lead` (the mouth leads the sound, 0.04 to 0.08 s; 0.06 default), `offset` (seconds later/earlier), `extras: false` for no blinks, brows or eye movement.
4. Run `lip_sync {clip, audio, check: true}`: it names a mouth moving in silence, a mouth still while speaking, a missed or too-short M/B/P closure and a mouth off the voice by more than 2 frames, each with its fix (usually `offset` or a re-sync). It also runs after every sync.

**Checks**
- Look at three frames: an M/B/P (lips pressed, even with the jaw open), an `ee` (wide) and an `oo` (round).
- Lip closures hold at least 2 frames at the project rate; fast speech is not over-articulated (short sounds open less).
- The jaw (opening) and the lips (closure, round, wide) are separate channels: a rig with both keeps them apart.

**Mistakes**
- Syncing before the line is final: re-sync after retiming or trimming the voice, since keys are laid for the audio they were made from.
- A face with no mouth keys, jaw bone or mouth pins: the reply says what is missing; rig it or add a ready character first.
- Hand-keyed mouths left under a new sync: a sync replaces the mouth keys inside the line's window only.
- Offsetting by eye: use the check's `offset`, it measures the lag.
