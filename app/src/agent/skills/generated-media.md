---
name: generated-media
use: Use when something is missing: generate or change pictures, animate a still, restore.
topics: generated, generate, generation, stills, no-key, generative, enhance, restore, restoration, upscale, restyle, relight, outpaint, extend, inpaint, looks, consistency, variations
tools: search_media, generate, enhance, search, catalogue, import, download, cut_out, parallax, three_d, animate, frame, check
---
**Search before generating.** A real animal, place, object, texture or b-roll shot exists already: `search_media {query, kind: image | video}` (library, then the web, plus Openverse, Wikimedia Commons and Poly Haven), look at the sheet, `fetch` the best one. Generate when the person wants something original or stylised, when nothing found fits, or when a set must match. Say which pictures were found and which were made.

**Order**
1. Look first: footage, the library (`catalogue`, `search`), then make. Only generate what was asked for or what the film cannot do without.
2. Pictures from words: `generate {make: "picture", prompt, transparent, aspect}`, one item or a batch (skill picture-batches). Video, music and sound effects from words are not available: make the pictures and move them, and score from the library.
3. Make a still move: `generate {task: "animate", motion}` (a camera move through the picture's depth), or `parallax` / `three_d` on separate layers, or keyframes (skill motion-animation).
4. Change what is there: `generate {task, clip | image}` with `restyle`, `relight`, `time_of_day`, `season`, `outpaint`, `inpaint`, `upscale`, `between`. `best_of: 3` keeps the best of three takes. `{restore: clip}` undoes a result.
5. Restore and improve footage on this Mac: `enhance` (auto, dust, scratches, deflicker, denoise, faces, upscale, sharpen, `old_film`). Long local work says how long first (`estimate: true`).
6. Keep characters and places the same: `save_look`, then `look`; every generated file keeps its recipe (`{again: media id}`, `count` for variations).
7. Look at the result in frame, then continue.

**Checks**
- The generated picture matches the project's style and aspect; the subject is separate when it must act.
- Say what is generated and what is real; never present a stand-in as the real thing.
- Hands, text and faces in generated pictures are inspected at `detail: true`.

**Mistakes**
- Offering to set something up for video or music generation: say plainly it is not available.
- Re-generating a character per shot: its look is saved.
- Upscaling or sharpening to fix a blurry preview.
- Using a generated scene with everyone painted in: nothing in it can act (skill story-film).

## Reference: Generating and changing media

- `generate` makes what no file or library item has: a picture, by
  Codex's own image generation (about a minute each), saved in `Generated/` and imported (`to_timeline` to
  lay it down): `make: "picture"`; `transparent: true` makes the subject
  alone with alpha; `count` makes several. Work from a picture with
  `image`: a file, a media id, or `{"frame": 12.5}` for the program
  frame; it keeps a character or style. Only generate what was asked
  for. `{"list": true}` says what is available.
- Video from words is not available yet. Make it move in Zornov
  instead: `task: "animate"` makes a camera move through the picture's
  depth, or generate the pictures and move them (guide topic
  `generated`). Music, sound effects and singing are made by Zornov
  itself and always work: `music` composes original music with cue points
  (skill `music-beat-sync` part Scoring), `sfx` makes foley and effects and `sing` sings
  (skill `sound-design` part Foley); the library (`catalogue`) is free and instant
  for real recordings. Say plainly that a generated video clip is not
  available; do not suggest setting anything up for it.
- `generate` with a `task` changes or animates what is there, by the best
  provider here (`provider`: `codex` for pictures on the person's ChatGPT, `local` on this Mac; the reply says who made it
  and, for a fallback, what Codex would add). Work on `clip`
  (a video clip's frames or a still clip's picture: the result goes back
  into the clip with its framing kept, `{"restore": clip}` undoes it) or
  on `image` (a title, card, shape or nested clip works too: it is
  rendered over its span with what lies under it, and the result takes
  its place). A result made from `image` lands on the timeline by itself
  (at `at`, or at the moment of `{"frame": t}`; `to_timeline: false`
  keeps it in the media bin). Tasks: `animate` (a still into a clip by a camera move
  through its depth, `motion` push_in, pan_left,
  orbit_left, dolly_zoom…, `seconds`), `between` (`image` to
  `end_image`), `restyle` (`prompt` and/or `style_ref`; it copies
  the reference's colour and light, on a picture Codex redraws it), `relight` (`direction`, `colour`), `time_of_day` (`time`),
  `season` (`season`), `outpaint` (`right: 0.5`, `wider: 1.5`, `aspect:
  "21:9"`: a pan that runs off a backdrop gets more backdrop; the old
  part stays where it was and the grown sides open for the move),
  `inpaint` (`region` [x, y, w, h] shares, `prompt`), `upscale`
  (`scale`, `height`). `best_of: 3` on a picture task by
  Codex makes three takes, scores each (composition kept, detail,
  clipping, closeness to the reference or look) and keeps the best; the
  others are in `Generated/Takes` with their scores. `enhance` restores a clip or picture in one
  pass on this Mac: `steps` (auto, dust, scratches, deflicker, deblock,
  denoise, fade, faces, clean, upscale, sharpen), `old_film: true`,
  `scale` or `height` (Real-ESRGAN), `fps` (RIFE in-betweens to 48, 50,
  60); `{"remove": clip}` puts the original back. Long local work says
  how long first (`estimate: true`) and needs `long: true` past 15
  minutes.
- Keep characters and places the same across generations with looks:
  `generate {"save_look": "fern", "refs": [pictures], "words": "a red fox
  with a white-tipped tail", "seed": 7}` once, then `look: "fern"` on any
  generation adds its pictures, words and seed. The story bible's cast
  (look, references) and places (light, backdrop) are looks too.
  `{"looks": true}` lists them. Every generated file keeps its recipe
  beside it: `{"again": <media id>}` makes it again, `count: 3` makes
  variations (new seeds), and any other key changes one thing
  (`{"again": 12, "season": "winter"}`).

## Reference: Generated scenes in motion

What no file or library item has, Zornov makes: `generate` with `make:
"picture"`, by Codex's own image generation (about a minute a picture). Video from words is not available yet, so the
motion is made here, in the edit, and that is often better because it is
editable.

- **Real things are generated, not drawn.** A person, a face, a
  product, a place or an animal is never built from flat cards and
  ovals: that reads as clip art. Generate it, then design around it.
- **Separate the layers.** Generate the backdrop on its own (no subject)
  and the subject with `transparent: true` (alone, with alpha), or
  generate one picture and cut the subject out (`cut_out`). Separate
  layers are what make depth, light and motion possible.
- **Keep it the same subject.** Pass the first picture as `image` when
  making the next one (another pose, angle or expression): the same
  face, product and style across shots.
- **Make it move through time.** Stage the layers in 3D: the backdrop
  far (`three_d`, `z`), the subject near, with `parallax` or a camera
  move (a slow `push_in` or a 10-20° `orbit`, eased), a key light and an
  ambient light, a soft shadow. Animate the subject too: a turn in
  (`motion` presets), a slow drift or scale, a second pose cut on a beat
  or crossfaded. No still holds the screen for more than about 3 s
  without something moving.
- **Check it.** Look at the first, middle and last frames (`frame`): the
  move must show, the subject must stay clean at its edges (no halo from
  the cut-out), and the light must come from one side consistently.
- **The route when something is missing** (never stop and hand it back):
  library first (`catalogue`: the 3D Fluent emoji, photos, motion
  clips), then `generate`, then building with shapes, gradients, light
  and 3D for graphic (not realistic) designs. If the missing piece is
  video from words, say in one line after the job is done that it is not
  available yet, not instead of it.

## Reference: Generative media: change what is there

A film studio does not only shoot new material; it fixes, extends and
reworks what it has. `generate` with a `task` and `enhance` do that,
each by the best provider here (Codex for pictures, or this Mac for
free), and every result keeps its recipe so it can be made
again or varied.

- **Restore before you grade, grade before you restyle.** Old or rough
  footage goes through `enhance` first (`old_film: true` for archive
  film: dust, scratches, flicker, fading; `steps: ["deblock",
  "denoise"]` for over-compressed web video), then the grade, then any
  restyle. Restyling noise only paints the noise.
- **Upscale the shot, not the timeline.** Upscale only clips that are
  smaller than the canvas and on screen long enough to be looked at;
  2× is usually enough (a 960×540 clip in a 1080p edit). A face that
  fills the frame wants `faces: true`. Check a frame at 100 % after.
- **Frame rate is a look.** Interpolating 24 fps drama to 60 makes it
  look like television; use `fps` for screen recordings, sport and slow
  motion, not for a film's own shots.
- **Extend for the move, not the still.** When a pan, push or reframe
  runs off a backdrop, `outpaint` the backdrop on the side the camera
  goes (`right: 0.5` for a pan to the right): the picture keeps its
  place and the new side opens for the move. Extend a little past what
  the move needs; a model's invented edge is weakest furthest out.
- **Fill small, repaint big.** `inpaint` on this Mac takes out wires,
  specks and small signs cleanly; anything larger (a person, a car)
  needs Codex and a prompt for what belongs there.
- **Light continuity first.** When shots of one scene were made or shot
  at different times, `relight` them to one key direction and colour
  (the place's light in the story bible), or move a backdrop to the
  scene's `time_of_day`, before matching colour. Light from one side
  across a scene matters more than exact colour.
- **One look per film.** `restyle` with a `style_ref` (a frame of the
  look you want) on every shot of a sequence, at the same `strength`,
  so the whole sequence moves together; a restyle in words alone
  drifts from shot to shot.
- **Keep the cast the same.** Save each character and place as a look
  (pictures from more than one angle, the words that describe them, a
  seed) and pass `look` on every generation that shows them. The story
  bible's cast and places already count. Generate a new shot of a
  character from their look's picture, not from words alone.
- **Animate a still with intent.** `animate` on this Mac moves the
  camera through the picture's depth: a slow `push_in` for tension, a
  `pan` to reveal, an `orbit` for scale, a `dolly_zoom` for unease, 4 to
  6 s. When something in the picture must move by itself (water, hair,
  a crowd), that needs a video model, which is not available yet: say
  so plainly and keep the camera move.
- **Say what was used, not what it cost.** Name who made it when it matters. Never mention cost, tokens or keys unless the person asks; local work over 15 minutes says how long first.

## Reference: tool `generate`

Make a picture from words with Codex's image generation: make: picture. Video from words is not available yet: make the pictures and move them (three_d, parallax, keyframes). Music, sound and singing are made by Zornov itself, with nothing to set up: edit op music composes original music, edit op sfx makes foley and effects, edit op sing sings. From words, or from pictures: a file, a media id, or {"frame": seconds} for the program frame (restyle a still, the same product in a new shot); transparent: true makes the subject alone for layering in 3D. Saved in the project's Generated folder and added to the media bin; with to_timeline also laid on the main lane. list: true says what is available. Only for what the person asked to have made. With task it changes or animates what is there, by Codex (pictures) or this Mac (provider; auto picks the best here and says what Codex would add): animate (a still into a clip by a camera move through its depth: motion push_in, pan_left, orbit_left…), between (image to end_image: a morph), restyle (style_ref and/or prompt; pictures or a clip), relight (direction, colour), time_of_day (time), season (season), outpaint (left/right/top/bottom shares, wider, taller or aspect: fixes pans that run off a backdrop), inpaint (region [x,y,w,h] and prompt), upscale (scale or height). A clip goes back into that clip, framing kept (restore: clip puts the original back). look: a saved character or place (save_look with refs, words, seed; the story bible's cast and places count) keeps generations matching. again: a media id makes it again from its recipe, count for variations.

Several pictures: `items: [{name, prompt, aspect, transparent, image}]` makes them all at the same time on Codex image workers (4 at once; `workers` up to 8, or the STUDIO_IMAGE_WORKERS setting). Plan every picture first and send one batch, never one by one. It returns a `job` at once; each finished picture lands in the media bin and is announced after your next tool calls (`pictures_ready`); `job: N` shows progress and every picture's state, `wait: true` waits for all, `cancel: true` stops pictures not started. A failed picture is retried once with a reworded request and reports its real reason.

## Reference: tool `enhance`

Restore and improve a clip or a picture in one pass, on this Mac by default (free): steps auto (default: deblock and denoise where needed) or a list of dust, scratches, deflicker, deblock, denoise, fade (faded or colour-cast prints), faces (face clean-up), clean (Real-ESRGAN clean-up at the same size), upscale, sharpen; old_film: true runs dust, scratches, deflicker, denoise and fade. scale (1-4) or height upscales with Real-ESRGAN (a licence-safe local model; install it in Settings ▸ AI models, bicubic until then); fps makes a higher frame rate with RIFE in-betweens. A clip gets the result in place (timing kept; remove: clip puts the original back). estimate: true says how long it will take.
