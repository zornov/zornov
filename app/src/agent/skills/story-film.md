---
name: story-film
use: Use when turning a script or story into an animated film: cast, places, staging, dialogue.
topics: story, script, screenplay, shots, shot-list, storyboard, bible, foley, staging, planning, coverage, cinematography, cinematographer, framing, shot-sizes, lenses, scene, scenes, place, places, backdrop, cast, ambience, blocking, in-the-backdrop, ground-shadows, integration, 180, anamorphic, animatic, bokeh, compositing, contact-shadow, continuity, crane, dolly, dp, drone, eyeline, eyelines, focus, footsteps, foreground, golden-ratio, guides, handheld, headroom, jib, lead-room, light-match, lighting-setups, line, moods, occlusion, overlays, pacing, pasted, plan, previs, rack-focus, rigs, rim-light, shot, steadicam, storytelling, tangents, thirds, three-point
tools: search_media, story, plan, see, scene, narrate, voices, generate, character, puppet, scissor, skeleton, pose, parts, swap, living, particles, parallax, shadow, light_match, foreground, foley, sfx, music, transition, shot, compare_cameras, animatic, frame, contact_sheet, check, review, notes
---
**Search before generating.** Real places, animals, objects, textures and reference looks for the bible come from `search_media` first (library, then the web); use found pictures as `image` references for consistent generation. Generate characters and places that are original or stylised, or that must match across shots. Say what was found and what was made.

**Order**
1. Bring the script in: `story {do: "import", text | path}` (a Fountain screenplay keeps scene headings and cues; prose becomes a scene a paragraph). The person's words and times stay as written; change them only with `story {do: "line"}`.
2. Write the bible before any picture: `story {do: "set"}` with beats, places (backdrop, light, time of day, ambience bed, ground surface) and cast (look in words precise enough to draw again, voice, size against the others, personality, how it moves). The same character is the same picture, voice and size in every shot.
3. Make the pictures in one batch (skill picture-batches): a backdrop per place, a transparent character per cast member (`generate {items}`), each saved as a look so it stays the same.
4. Plan coverage: `story {do: "plan"}`, revise each shot (kind, framing, action, `side` of the 180 line, `looks`), then `story {do: "check"}` and fix everything it reports before building.
5. `story {do: "build"}` (`voices: true` speaks every line). It lays a stand-in per shot, a marker per scene and the place's sound bed.
6. Stage each shot over its stand-in, reading `story {do: "brief", shot}` first: read the place (`see {place: true, t}`), give each character one spot sized for its depth, rig and act it (skill puppetry-rigging), then `story {do: "bind", shot}`.
7. Make the place live and sound (skill living-places; `sfx` bed per place, quieter than the voice) and set characters into it: `shadow`, `light_match`, and a `foreground` piece where they pass behind something.
8. Cut to places with a transition and a change of bed and light; characters walk out of one place and into the next.
9. Look at the middle of every line and every place change, then skill review-before-export.

**Checks**
- `story {do: "check"}` before build; `check` after (`faces_away`, `figures_overlap`, `story_still`, `canvas_shows`, `uncovered_line`); `review` last, fix every note.
- Every spoken line shows something: the speaker talks, listeners react, the thing named is on screen.
- Frame a moment per place: feet on the ground with a shadow under them, the same light on character and place.

**Mistakes**
- Swapping animated characters for still pictures or full-frame panels. That is a slideshow; give the acting more (pins, acts, a better spot, a new pose) instead.
- Equal-length chunks of narration, three shots of one size in a row, crossing the 180 line off screen.
- A new picture of a character halfway through: use its saved look.
- One backdrop held for minutes: each beat gets another part of the place, a closer or wider view, or moved light.
- Calling a workaround by the name of the real thing. Say "a cross-fade, not a morph".

## Reference: Story: plan the film

A film with a script is planned before it is built, in the project's
**story** (the `story` tool), and built from the plan. Never cut a
narration into equal chunks and fill each with pictures: break it into
scenes and shots first.

- **Script** → `story {"do": "import", "text": "…"}` (or `"path"`: a
  `.fountain`, `.txt`, `.md`, `.srt`, `.vtt` or `.docx`; `"plan": true`
  also plans the shots; `"merge": true` puts the text over the story
  scene by scene, keeping shots and the bible; `"replace": true` over an
  existing story). Fountain keeps scene headings (`EXT. MEADOW - DAWN`
  makes the place and the time of day), character cues, parentheticals
  and `(V.O.)` narration; plain prose becomes a scene a paragraph (or a
  scene per `# Heading`) and a line a sentence; `NAME: words` is
  dialogue; Markdown headings are scenes. SRT and WebVTT keep each
  cue's start and length. `story {"do": "export", "format": "fountain" |
  "script" | "text" | "srt" | "shots", "path"}` writes it out (fountain
  carries shots and the bible so it reads back whole; script is the
  words, times and notes only).
- **Script times**: a line (and a scene, and a beat) can carry a start
  and a length. People type them in front of the words: `[0:12-0:18]
  words`, `0:12 words`, `[+4s] words`; the model keeps them as `start`
  and `duration`. A line without a time follows its shot (so it moves
  when the clips move) or, with no shot, is estimated from reading speed
  (2.6 words a second spoken, 3 read). `story {"do": "line"}` lists the
  lines with `[start-end]` (a `~` after it means worked out, not given);
  `story {"do": "line", "scene": "1", "line": 2, "start": "0:12", "end":
  "0:18"}` (or `duration`, or `time: "0:12-0:18"`; seconds or
  timecodes) sets one, `clear_time: true` lets it follow again, without
  `line` and with `text` it adds one (`after: n`), `remove: true` takes
  one out, `shift: 2.5` (with `from`, `to`) moves the given times, and
  `edits: [...]` does several. `plan` and `build` lay planned shots so
  the cuts fall on the given times (`story {"do": "retime"}` does only
  that); `build` with `voices: true` speaks a timed line at its time.
  Scenes take `start`, `notes`; lines take `note` (for the crew, never
  spoken).
- **Beats, scenes, places, cast** → `story {"do": "set", …}` patches by
  id: `beats: [{id, summary, purpose}]`; `places: [{id, backdrop (a
  media name or file), ambience (a bed: stream, forest, birds, night…),
  light, time_of_day, surface (grass, leaves, dirt, gravel, stone, wood,
  snow, sand, mud, water), interior, notes}]`; `cast: [{id, name, look,
  references, cast_item, voice, size (1 = the reference character),
  personality, moves (puppet acts, gait first), facing}]`; `scenes: [{id,
  heading, beat, place, time_of_day, cast, summary, lines, shots,
  after}]`; `shots: [{id, …}]` changes shots by id; `remove: true`
  deletes any of them; `narrator_voice` is the voice narration uses.
  Lines are `"FOX: words"`, plain words (narration) or `{kind, who,
  text, how}`.
- **Shots**: `{id, kind, subjects, framing, camera, duration, lines,
  action, side, crosses, looks, notes, after}`. `kind`: establishing,
  wide, two_shot, medium, over_the_shoulder, close_up, extreme_close_up,
  reaction, insert, pov. `camera`: static, pan, tilt, push_in, pull_out,
  track, crane, handheld, zoom. `lines` are the scene's line numbers
  (from 1) the shot plays under; `side` (`a` / `b`) is the camera's side
  of the 180° line; `looks` (left, right, camera) is where the main
  subject looks on screen.
- **Plan** → `story {"do": "plan"}` makes a first coverage for every
  scene without shots: where we are first, singles for each speaker with
  mirrored eyelines on one side of the line, reactions where a line
  lands, closer shots for feelings, wides for travel, each as long as its
  words. Then revise it: write each shot's action and framing, add the
  inserts and POVs the story calls for.
- **Check** → `story {"do": "check"}`: lines no shot shows, scenes with
  no place or no establishing shot, jump cuts, three of a size in a row,
  crossed lines, eyelines that miss, shots too short for their words,
  even chunks, characters with no look or voice, unknown names; on a
  built story also shots out of story order. Fix them before building.
- **Read the bible before staging** → `story {"do": "brief", "shot":
  "2C"}`: the place (backdrop, light, ambience, ground), who is in frame
  with their look, references, voice, moves and the height in pixels to
  stand at (from the shot size and their sizes), the lines with their
  voices, the framing and camera move for the shot size, and the shots
  either side (side of the line, eyelines).
- **Build** → `story {"do": "build"}` (one undo step; `scenes` picks
  some, `rebuild: true` lays them again, `voices: true` speaks each line
  with its character's voice): a stand-in for every shot on the main
  lane (the place's backdrop framed for the size and moved as its camera
  moves, or a slate card), a marker per scene and the place's bed under
  it. Stage the characters on each, then `story {"do": "bind", "shot":
  "2C"}` (every picture clip starting in its time, or `clips`).
- **In sync**: every `story` call reads the timeline: a built shot's
  start and length follow its clips (trimmed, moved, deleted); a new
  `duration` for a built shot trims its clips. `story` with no `do`
  shows the outline; `"scene": "2"` one scene with numbered lines.
- **Sounds tied to action**: `foley {clip}` lays footsteps for a
  puppet's walk, run, sneak or hop (paddles for swim, taps for peck) on
  the frames its feet land, for the ground of its scene's place (or
  `surface`) and the character's size; `puppet` with `"foley": true` (or
  a surface) does it as it acts. `foley {kind, at}`: splash, rustle,
  thud, flap, swish. Read the craft book's `planning` for coverage,
  the line, eyelines and pacing.

## Reference: Planning the film: script, coverage and continuity

A film is planned before it is built. Cutting a ten-minute narration into
twenty-second chunks, each with whatever pictures come to hand, gives a
slideshow in which nothing is shown on time and characters change from
one chunk to the next. Plan it the way a production does, in the
project's `story`, then build from the plan.

1. **Script.** Bring the screenplay or the narration in (`story` `do:
   import`). A Fountain screenplay keeps its scene headings, cues and
   (V.O.) narration; plain prose becomes a scene a paragraph and a line
   a sentence, so each sentence can be given to the shot that shows it.
2. **Beats.** Name the turns of the story (set-up, the call, the trial,
   the low point, the climax, home again) and give every scene the beat
   it serves. A scene that serves no beat is a candidate to cut.
3. **Places.** Every scene happens somewhere: a place with its backdrop,
   its light (direction, colour, quality), its time of day, its ambience
   bed and its ground. Scenes in the same place reuse the same backdrop,
   light and bed, so it reads as the same place; a new place changes all
   of them.
4. **The character bible.** Before anyone is staged, write each
   character down: their look in words precise enough to draw again,
   reference pictures or the cast item used for them, their voice, their
   size against the others (a heron at 1.6 stands 1.6 times the fox in
   every shot they share), their personality and how they move (their
   gait first: a rabbit hops, a fox walks or sneaks, a fish swims). Read
   the bible before every shot (`story` `do: brief`). The same
   character is the same picture, the same voice and the same size
   everywhere: never a new picture of them halfway through.
5. **Coverage.** Each scene opens on where we are (establishing or
   wide), then goes in to what matters: medium for what people do,
   close-ups for how it feels, a reaction where a line lands, an insert
   for the thing the words name (the key, the paw print, the water), a
   POV when we should see what a character sees. A conversation gets a
   two shot to set the geography, then a single (or an over-the-shoulder)
   for each speaker. Every line of narration and dialogue belongs to a
   shot that shows what it says: `uncovered_line` is the narration
   nobody sees.
6. **Variety.** No three shots of one size in a row; never the same size
   on the same subject twice in a row (a jump cut): change the size a
   step or the angle by 30° or more, or cut away between them.
7. **The 180° line.** In a scene, draw the line between the two who
   talk (or along the way someone travels) and keep the camera on one
   side of it (`side: "a"`): then the fox is always frame left looking
   right and the heron frame right looking left. Cross only on screen (a
   moving shot marked `crosses`) or through a neutral shot (straight down
   the line, an insert). Travel keeps its screen direction too: a
   journey that goes left to right keeps going left to right.
8. **Eyelines.** Two singles of characters looking at each other look
   opposite ways (`looks: right` / `looks: left`), at the same height
   when they are the same size, up and down by their difference when
   not. Leave looking room on the side they look to.
9. **Pacing.** A shot lasts as long as its words and its moment, not a
   fixed length: hold the wide that establishes and the moment that
   lands, quicken through action, let a reaction run a beat longer than
   feels safe. `story check` reports shots too short for their words,
   shots all one length (`monotonous_pacing`), flashes under 0.6 s and
   long static holds.
10. **Build, then stage.** `story build` lays a stand-in for every shot
    (the place's backdrop framed for the shot size and moved as the
    camera moves, or a slate), a marker per scene and the place's bed.
    Stage each shot over its stand-in from its brief, look at it, and
    `story bind` so the shot owns its clips. From then on the plan and
    the timeline follow each other: trim a shot's clip and the shot's
    length follows; change a built shot's length in the story and its
    clips are trimmed.
11. **Sound tied to action.** Footsteps land on the frames the feet do:
    a puppet that walks, runs, sneaks or hops gets `foley` (or
    `puppet` with `"foley": true`), made for the ground of its place
    (grass, leaves, gravel, stone, wood, snow, sand, mud, water) and
    weighted by the character's size. Spot sounds where things happen:
    a splash into water, a rustle through bushes, a thud when something
    falls, wings that flap, a swish past the lens.

## Reference: Staging a story scene

A story is told by what is seen. For every place and every line, decide
where everyone stands, what they do, what the place does and how it
sounds, and check it.

1. **Read the place first.** `see` with `place: true, t` reads the
   backdrop (characters left out): where the far land ends, which parts
   are ground, nine spots to stand on, each with the height a character
   standing there should have, and the pieces (rocks, trunks, grass)
   a character can stand behind. Frame it too, to see what the spots
   are (a rock, water, a log) and what the place offers: a bridge to
   cross, a trunk to hide behind, a stream to drink from.
2. **One spot each.** Feet on a spot, sized for its depth, a clear gap
   between characters; nobody's face covered. Further back is higher and
   smaller. `check` reports `figures_overlap` when one character hides
   another.
3. **Facing.** Characters who talk face each other (see "Characters:
   life, facing and dialogue"); `check` reports `faces_away`.
4. **Every line is shown.** For each line of narration or dialogue,
   decide what is seen while it is said: the character it is about does
   it (walk across with `travel`, peck, point, look, surprise), the
   speaker talks, the listeners react (listen, nod, shake_head, laugh,
   scared, sad, think). Different characters react differently, and
   each in its own time. `check` reports `story_still` when a voice
   speaks and nothing on screen acts.
5. **A place that lives.** A still backdrop reads as a photo. Read
   what it is made of (`see` with `regions: true, t`: sky, water,
   ground, path, trees, rocks...), then `living` with the recipes that
   fit it: `water` (ripples that flow and glints), `foliage` (sway),
   `clouds` (drift), `rays` (light that breathes). Each part moves only
   inside its own region, so the ground holds still. Weather and time
   tell story too: `rain`, `storm`, `snow`, `mist`, `leaves`,
   `fireflies`, `dusk`, `night`, `day_to_night`, with `ramp` to bring
   it in on a beat (the storm arrives as the trouble starts; night falls
   as the friends settle down). `particles` puts one effect where you
   want it (`region`, `behind` a character, or in front of everyone).
   `puppet` `"rig": "backdrop"` with `"act": "breeze"` still sways a
   whole picture whose parts the model cannot name; a separate sky or
   foreground layer moved at its own speed gives depth (`parallax`).
   Keep it subtle unless the moment is about the weather.
6. **A place that sounds.** Lay a bed under every place for as long as
   the scene stays there: `sfx` `kind` `stream`, `waterfall`, `rain`,
   `wind`, `birds`, `crickets`, `fire`, `waves`, `forest` or `night`
   (made here, no file or key), quietly under the voice (about −20 dB
   below it). Add spot sounds where things happen: footsteps on a walk,
   a splash, a creak. Change the bed when the place changes.
7. **Going somewhere new.** A new place is a new backdrop, not the same
   picture slid sideways. Characters leave the old one (walk out of
   frame) and enter the new one; mark the change with a transition and
   a change of bed and light. A backdrop that moves or zooms must keep
   filling the frame: `check` reports `canvas_shows` when its edge
   slides in.
8. **In the place, not on it.** Every character standing on the ground
   gets a `shadow` and a `light_match` once it is placed (see
   "Characters in their backdrop"); one standing behind a rock or trunk
   gets that piece as a `foreground` over it.
9. **Look.** Frame the middle of each line and each place change before
   calling it done.
9. **Review the film.** `review` watches all of it at once, as the
   audience will: run it last and act on every note (see "The
   director's review").

Never swap animated characters for still pictures: a generated scene
with everyone painted in, or a storyboard cut into full-frame panels,
is a slideshow, and nothing in it can act, react or walk. When the
acting looks weak, give it more: more pins, more acts, a better spot, a
new pose generated as its own transparent character. Keep each
character its own layer.

## Reference: Characters in their backdrop

A character generated on its own and laid over a generated place looks
cut out and pasted on until three things are right: it touches the
ground, it can be behind things, and the same light falls on it as on
the place. Do all three for every character that stands in a scene,
after it is placed and sized and before the acting is judged.

1. **A shadow.** `shadow` with the `clip` and nothing else reads the
   backdrop's light: a sun or glowing sky in the picture means the
   scene is lit from behind and shadows come towards the camera,
   spreading out from below the light, long when the light is low; a
   scene lit from the camera's side throws them back into the picture
   and away from the brighter side. Hard sun gives a dark, crisp
   shadow; an overcast or misty place a faint, very soft one; the colour
   is the scene's own shade colour. There is always a dark contact
   patch right under the feet: without it a character hovers. The
   shadow is drawn from the character's own bent picture, so it walks,
   hops and acts with the puppet; `ground: y` keeps it on a ground line
   while the character jumps (the shadow stays, fades and shifts as it
   rises). Characters standing in the same light get shadows falling
   the same way: shadow them in one op (`clips`, or `at`).
2. **Behind things.** `see` with `place` lists the pieces that could
   hide a character (a trunk, a rock, a bank of grass), with where to
   stand behind each. Put the character's feet there (higher up the
   frame than the piece's base, smaller for the distance), then
   `foreground` with `piece: <n>` cuts the piece from the backdrop and
   lays it over the character, so its legs or body pass behind the
   rock. A walk behind a trunk and out again sells the depth of a place
   more than any camera move. Cut only what the character passes
   behind; a piece over nobody does nothing but cost a layer. Frame it:
   a piece that grabbed a patch of ground behind the rock shows as a
   hard edge on the character, so cut again with `exclude` on that patch
   or a tighter `box`.
3. **The same light.** `light_match` darkens a character standing in
   shade, brightens one in sun, tints it a little towards the light and
   colour around it (a green glade, a warm dusk) and puts a rim of the
   scene's key light on the edges that face it, strongest when the
   scene is lit from behind. It keeps the character's own colours: at
   100 % it is a gentle move; 150-200 % for a night or a deep forest.
   Match again when a character walks from shade into sun (one
   `light_match` per clip, at the moment it stands there).

Check by framing a moment of each place with a character in it: the
feet sit on the ground with a dark line under them, the shadow falls
the way the shadows painted in the backdrop fall, and the character is
neither brighter nor more saturated than what is around it.

## Reference: Cinematography: shots, lenses, framing and light

The camera tells the story: where it stands is whose side we are on, how
close it is is how much we feel, when it moves is when something
changes. Choose every shot for what the moment needs, never for variety.
- **Shot sizes.** Wide (and extreme wide) says where and who is there;
  medium is conversation and action; close-up is feeling; extreme
  close-up is one detail that matters (eyes, a hand, a key). Open a scene
  wide, go closer as it tightens, come back wide to release it. Cut
  between sizes at least two steps apart (wide to close-up, not medium
  to medium-close), or it reads as a jump.
- **Angles.** Eye level is neutral. Low angle gives power, threat or
  heroism; high angle makes small, alone, watched; bird's-eye is fate or
  a map; a dutch tilt is unease, once. Over-the-shoulder ties two people
  together; a point of view puts us in someone's eyes.
- **Lenses.** Wide lenses (14-28 mm) open space and exaggerate depth and
  speed; normal (35-50 mm) is how we see; long (85-135 mm and more)
  flattens, isolates and spies. Faces look best from 50 to 100 mm; a
  wide lens close to a face distorts it (comedy, menace). A sensor
  smaller than full frame crops: the same mm frames tighter. Anamorphic
  (2× squeeze) is the big-screen look: wide view, oval bokeh, bent
  edges; vintage glass is softer and warmer. Pick one kit per film.
- **Focus.** Keep the eyes sharp. Shallow focus isolates a subject from a
  busy place; deep focus lets the audience look around. Rack focus moves
  attention from one subject to another: on a look, a line, a reveal,
  not as decoration; it takes about a second.
- **Composition.** Put eyes on the upper third line, the subject on a
  third (a power point where the lines cross is strongest); centre a
  figure only for symmetry, confrontation or a still, formal moment.
  Lead room: a figure looking or moving one way gets the space ahead of
  it (looking right: stand it on the left third); a figure boxed against
  the near edge feels trapped (sometimes that is the point). Headroom
  is small: 5-12% over the head in a medium shot; in a close-up cut
  across the forehead, never the chin. Never cut at a joint (ankles,
  knees, elbows, wrists): show the feet with floor under them, or cut
  mid-thigh, at the waist or the chest. Avoid tangents: an edge that just
  touches the frame or another figure makes the eye snag; overlap
  clearly or leave a clear gap. With an aspect guide on (2.39 for the
  cinema, 9:16 for phones), compose inside it: what falls outside will be
  cut. Check with `frame` (guides drawn in) and `check`.
- **Moves.** A move must mean something: push in is realisation or
  attention; pull out is reveal, loneliness, an ending; orbit is
  importance or two people circling; crane or drone up is release, scale,
  the end of a chapter; a truck alongside follows action; steadicam
  floats with someone through a place; handheld is urgency, a witness, a
  fight (calm for intimacy, nervous for fear, running for a chase). Slow
  is grand, fast is shock. One move per shot, eased in and out, and hold
  still when the moment needs the audience to look.
- **2.5D on stills.** A camera on flat pictures in space shows their
  edges and their flatness if it goes too far. Small moves (a push of a
  quarter of the distance, an orbit under 20°), backdrops that start
  full-frame, figures as cards facing the camera. `shot` and the camera
  op cut a move back rather than show the empty canvas, and say so; a
  wide establishing move needs a wider backdrop.
- **Lighting.** Light shapes and points. Three-point (key, fill, back) is
  the base: a key 30-60° to one side and above, fill on the other side at
  a third to a half of it, a back light to separate the figure from the
  background. The key-to-fill ratio is the mood: high key (little
  contrast) for comedy and joy, low key (deep shadow, one hard key) for
  noir, menace and mystery. Motivate the light: it should come from
  something in the picture (a window, the sun, a fire, a screen) in its
  colour; `lighting` with `motivated` reads the backdrop for it. Warm key
  and cool fill reads cinematic; golden hour is nostalgia and romance,
  blue hour and moonlight are quiet and lonely, under-light is horror.
  Flat pictures are lit evenly across, so on them light mostly sets
  brightness, colour and side; 3D models are modelled by it.
- **Previs.** Block a scene before you build it: stand-ins where the
  characters stand (`previs`), a camera per shot from the shot list,
  `animatic` to see the cut as a storyboard, `compare_cameras` to choose
  between framings. Keep the camera on one side of the line of action
  between two characters (the 180° rule) unless a shot crosses it in
  view.

## Reference: The shot op, a camera a cinematographer names

**Cinematography**: `shot` is a camera a cinematographer names. Say it
(`{"op": "shot", "say": "slow push in on Pip"}`) or give the parts:
`on` (subjects by id or name: 3D layers and models), `size`
(extreme_wide, wide, medium_wide, medium, medium_close_up, close_up,
extreme_close_up, two_shot, over_the_shoulder, insert, pov), `angle`
(eye_level, low, worms_eye, high, birds_eye, dutch), `move` (words: push
in, pull out, orbit, truck left / right, crane up / down, tilt, pan,
zoom in, crash zoom, dolly zoom, steadicam, handheld, drone, follow,
rack focus to X), `speed` (slow, fast), `lens` (mm), `shallow` (focus on
the eyes, background soft), `at`, `duration`, `clip` (re-frame a camera
you have). It makes a camera clip for the shot's time, places it so the
subject fills the frame at that size with the eyes on the upper third
and lead room on the side the figure looks, uses the usual lens for the
size (28 mm wide, 40 medium, 65 close-up) and the kit of the camera
already there, then moves. "the group" / "everyone" frames every figure
on screen. On flat pictures it keeps angles where cards still read (30°
aside, 35° up or down) and says so. A 2.5D shot never shows the empty
canvas: backdrops grow to cover the frame, and a move that would still
run past the pictures is cut back (and said). The camera op knows the
rest of the rig: `lens_kit` (spherical, vintage, anamorphic,
anamorphic_1.5, zoom, macro, ultra_wide, tele) or `focal_length` in
words ("anamorphic 40"), `sensor` (full_frame, super35, super16,
large_format, 65mm, imax, micro_four_thirds, aps_c, phone), `squeeze`
(anamorphic widens the view by it), `breathing` %, `bokeh` (round,
hexagon, octagon, oval, star: a shape other than round is baked as a
lens blur on each layer by its depth), `distortion` / `fringe` % (a
"Lens" adjustment layer over the shot). Camera moves beyond the basic
ones: `rack_focus` (`to`: a clip, point or distance), `follow_focus`
(`subject`), `dolly_path` (`path` of points, `aim`: keep, ahead, a
clip), `crane` / `jib` (`by` degrees of arm, `arm` px, `swing`),
`steadicam` (`by`, `subject`, `sway` %), `handheld` with a `profile`
(calm, documentary, shoulder, nervous, running), `drone` (`style`: rise,
reveal, flyover, orbit, pullback; `height`), `follow` (`subject`).
`lighting` puts up a whole set-up round a subject from where the camera
looks: `preset` three_point, rembrandt, butterfly, loop, split,
high_key, low_key (noir), rim, silhouette, golden_hour, blue_hour,
moonlight, firelight, neon, overcast, under, or `motivated` (the key
comes from the backdrop's brightest part in its colour, the fill from
the picture's own colour); or a `mood` (warm, cool, tense, romantic,
happy, scary, epic, night, party, natural). It switches off earlier
lights there (`replace: false` keeps them). `previs` blocks a scene:
stand-in mannequins (real 3D models) for every name with no clip, a
plain wall and floor when nothing fills the frame, and a camera per shot
from `shots` (words or shot fields with `duration`) or from the story's
shot list; then `animatic` to review it. Read `guide {"topic":
"cinematography"}` before shooting a scene.

    {"op": "shot", "say": "low angle close-up on Pip, slow push in"}
    {"op": "shot", "on": ["Pip", "Bo"], "size": "two_shot", "move": "slow orbit", "duration": 5}
    {"op": "camera", "lens_kit": "anamorphic", "focal_length": 40, "sensor": "super35", "move": {"type": "rack_focus", "to": {"clip": 9}, "from": 1.5, "duration": 1}}
    {"op": "camera", "move": {"type": "handheld", "profile": "nervous"}}
    {"op": "lighting", "preset": "motivated", "on": "Pip"}
    {"op": "previs", "shots": ["wide on Pip and Bo", "close-up on Pip", "slow push in on Bo"]}

## Reference: tool `story`

The production plan kept in the project: the script broken into beats, scenes and shots, each scene's place (backdrop, light, time of day, ambience, ground) and cast, and the character bible (look, reference pictures, cast item, voice, size, personality, moves). Plan with it BEFORE building: import the screenplay or narration, describe places and characters, plan coverage (every line in a shot, shot sizes varied, one side of the line, eyelines that meet, paced to the words), check, then build the timeline from it and stage each shot from its brief. do: show (outline; scene: one in full with numbered lines), cast (the bible), import (text or path: Fountain, plain text; plan: true), export (format fountain, text or shots; path), set (title, logline, narrator_voice, beats, places, cast, scenes with lines and shots, shots by id: patches by id, remove: true deletes), remove, plan (a first coverage per scene: revise it), check (craft problems with fixes), build (stand-ins on the timeline, one undo step: backdrops framed per shot, slates, markers, ambience beds; voices: true also speaks the lines), bind (shot + clips, or every picture clip starting in its time), brief (shot: everything staging it needs). Shot times are seconds; lines count from 1. The timeline is read back on every call: trimmed or moved shots update.

## Reference: tool `plan`

The plan the person follows while you work, shown as steps with checkpoints. Post it once for a job of several parts: {steps: [{title, checkpoint}]} (a director also gives each step role, task, clips, after). Then mark steps {start: n} and {done: n, note} as you go, or add a progress line with {note}. A checkpoint marked done waits for the person to approve it. A director reviewing calls {approve: true, summary} or {redo: [{step, note}]}. No arguments: the plan as it stands.
