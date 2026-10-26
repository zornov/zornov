---
name: footage-edit
use: Use when cutting the person's footage: rough cut, pacing, b-roll, music, captions.
topics: footage, editing, editorial, cutting, trim, trimming, sequences, takes, dailies, recipes, moves, creative, time, speed, freeze, stretching, ramps, analyse, catalogue, sounds, sound-library, drops, sound
tools: search_media, see, listen, analyse, frame, contact_sheet, media, import, download, make_playable, place, add, insert, overwrite, replace, split, trim, delete, move, reorder, ripple, roll, slip, slide, j_cut, l_cut, text_cut, speed_ramp, smooth_motion, reframe, music, sfx, narrate, audio, transition, marker, editorial, catalogue, search, summary, script, notes, plan, check, review, ai
---
**Search before generating.** When the film needs b-roll, a real animal, place, object or sound that the person did not supply, `search_media {query, kind: video | image | audio}` finds it (library first, then the web); `fetch` the best and cut it in. Generate stills only for something original or when nothing found fits. Say what was found and what was made.

**Order**
1. Read what exists: `notes` (your earlier plan), `summary` or `script` (what is on the timeline), `media` (what was imported). New footage that will not preview: `make_playable`. A link: `download`.
2. Know the footage before cutting. `see {media}` scouts it a line per second; `listen {media}` hears speech, music and faults; `analyse` finds shot changes, silences and beats; `ai {captions}` gives the spoken words with times. A talking clip is cut by its words, not by eye.
3. Write the plan in `notes`: what it is in one sentence, who it is for, the hook (the strongest picture or line goes first), the sections with durations that add up to the target, the music mood, the caption style. For several parts, post `plan` as well.
4. Rough cut, story order first: `place` (or `insert` / `overwrite` with `src_in` / `src_out` when you know the moment) the best moments, one section per `edit`. Talking footage: `editorial {ask: transcript}` then `text_cut` the fillers and false starts; dead air: `analyse` silences, then `split` and `delete` the pauses, last one first.
5. Pace it. Cut where the story turns, never mid-word or mid-sentence; let a reaction run a beat longer than feels safe; vary shot lengths (no clock-like even cuts). Land picture cuts on a beat only when a sentence ends near one. `j_cut` / `l_cut` make dialogue cuts natural. `speed_ramp` for one emphasised moment, then `smooth_motion`.
6. Cover and enrich. B-roll over a talking head hides a cut and shows what is said: footage first, then `catalogue` / `search` (library), then `generate` (see skill generated-media), never abstract filler. Vertical delivery from widescreen: `reframe`.
7. Sound: `music` under the whole film (one track per mood), voice on top, effects only where something lands (`sfx`, `add`); see skills music-beat-sync and sound-design.
8. Captions and titles: `ai {captions}`, styled and timed by skill titles-typography.
9. Look and listen, then finish with skill review-before-export.

**Checks**
- `frame` or `contact_sheet` across the whole edit; `listen {mix: true}` where the voice and music meet.
- `read_back` after every `edit`: an op under `unchanged` did nothing.
- `check` then `review`; act on every note.
- It works with the sound off, and the first two seconds are the strongest.

**Mistakes**
- Cutting a long narration into equal chunks, or cutting mid-word: use the transcript's word times (`listen` with words) and cut in the pause.
- Placing before looking: a clip placed without `see` often has the wrong half. Scout first.
- A whoosh on every cut and five transition kinds: one or two kinds, mostly straight cuts, a few sound cues.
- Fixing a flaw by adding effects. Trim, cover or re-cut instead.
- Reporting from intent: report from `read_back` and from frames you looked at.

## Reference: Footage and sound

- `analyse` finds shot changes, pauses and beats in the source files,
  in timeline seconds per clip. Captions (`ai` `captions`) give the spoken
  words with their times.
- **The library, as an inventory.** `catalogue` with no arguments says what
  Zornov's content library holds: counts by kind, category, mood and sound
  kind (hit, boom, swell, swoosh, long), which packs are installed, downloaded
  only as an index, or available, and how much needs a credit line in the film's credits. `catalogue {panel: "audio", query, kind, mood, min_seconds,
  max_seconds, fresh}` ranks sounds and music by name, tags, mood and what each
  sounds like, and returns a few varied candidates with kind, length and mood;
  items the project already plays are marked `used_in_project` (`fresh: true`
  leaves them out). `search` and `assets {action: find}` look in the library
  too.
- **Variety.** Placing a sound or track the project already plays adds a note
  with unused alternatives. `add {query: "whoosh", kind: "swoosh"}` takes a
  different unused one each time (`same: true` repeats on purpose); `sfx` makes
  a new variant of the same kind and length each time (`variant` repeats one);
  `music` prefers a track the project does not play. `check` and `review` flag
  one sound played too often, one music track under a film whose notes name
  several moods, and one kind of sound from start to end.
- `catalogue` lists what can be applied or added, per panel (`effects`
  with their settings, `looks`, `motions`, `text`, `transitions`,
  `assets`, `audio`, `media`), with an optional `query` (a word: `neon`,
  `frame`, `hud`, `impact`, `explosion`). When nothing matches, it widens
  to what editors mean by the word and says so. Sound searches are ranked
  by name, folder and what each sound measurably is: every result has
  `sounds_like` (hit, boom, swell, swoosh) and its length. `listen` with
  `item` describes one in detail; `sfx` makes one to measure.
- Lottie animations (`.json`, `.lottie`: animated icons, stickers, lower
  thirds from Lottie libraries) import like video with transparency and play
  over what is below. `export {"preset": "Lottie"}` (or a `.json` /
  `.lottie` path) writes the timeline as one: pictures, titles, cards and
  shapes with their motion exactly as it plays; video, sound, effects and
  masks are left out and listed.
- layered PSD documents: `import` brings a PSD as its merged picture, or with
  `layers: true` as one merged clip of its layers (each on its own lane,
  placed, blended and faded as in the document; `unmerge` opens it, so a
  title, a product and a background animate apart). The reply lists the
  layers (with their text for type layers) and what did not come over
  (adjustment layers, layer styles, clipping).

## Reference: Recipes

**Vertical short from widescreen** — `new_project` 1080×1920 → `import`
→ `place` with `fit: "fill_picture"` → `reframe` → `check` → `export`
`Vertical`.

**Cut the dead air** — `analyse` `silences` → `split` at both edges of
each pause → `delete` the pieces (ripple closes the gaps), last pause
first.

**Music under the voice** — `add` a track on a sound lane → `set`
`volume` −14 → `trim` its end to the picture → `fade_out` 2.

**Title card** — `card` black on the main lane for 2 s, a `text` over it,
`apply` a motion (`catalogue motions`).

## Reference: Cutting like a film editor

The tools film editors use, as ops. Times are
seconds: `src_*` are seconds into the footage, `at` / `rec_out` / `from`
/ `to` are timeline seconds. Each op is one undo step inside its `edit`.

**Source to record (three- and four-point edits).** `insert` splices in
(later material moves right), `overwrite` records over, `fit_to_fill`
changes the speed so the source range fills the record range exactly,
`replace` (with marks) swaps the footage under `at` keeping the clip's
length, place and settings, the source mark `src_in` landing on `at`.
Give any three of `src_in`, `src_out`, `at`, `rec_out`: both source marks
and `rec_out` backtime (the edit ends at `rec_out`); both record marks and
one source mark use the record length. No record mark: the end of the
main lane. `{"op": "insert", "media": "S12_T3", "src_in": 14.2, "src_out":
18.9, "at": 42}`. Patching: `lane` takes the picture (default main;
`video: false` for none), `audio_lane` puts the sound on its own linked
clip there (default: the sound rides in the picture clip), `audio: false`
leaves it out. `sequence` instead of `media` cuts another sequence in as a
nest. `source_edit {mode}` takes the mode as a field.

**Trimming.** `ripple {clip, edge: start|end, by}` trims and moves
everything after (and keeps sync-locked lanes in step); `roll {clip, by}`
moves the cut after a clip; `slip {clip, by}` changes what a clip shows,
`slide {clip, by}` moves it between its neighbours; `extend {clip, edge,
to}` rolls the cut at that edge to a time (an extend edit). Several
edges at once: `trim_sides {sides: [{clip, edge, kind: ripple|roll|plain},
...], by}`; the outgoing side on one lane and the incoming side on
another is an asymmetric trim (clip ids only in `sides`). Split edits:
`j_cut {clip, by}` makes the next shot heard `by` seconds before it is
seen (its sound starts under this clip), `l_cut {clip, by}` lets this
clip's sound run `by` seconds under the next shot; `split_edit {clip, by,
part: audio|video}` moves just one half of the cut either way. Sound still
inside the picture clips is detached first and stays linked.

**Ranges and lanes.** `lift {from, to, lanes}` takes the range out and
leaves the space; `extract {from, to, lanes}` takes it out and closes up.
Without `lanes`, every targeted lane. `target {lane, on}` (un)targets
lanes; `sync_lock {lane, on}` makes a lane follow ripples made on other
lanes (inserts, extracts, ripple trims, text cuts): a music bed or
overlay that must stay in sync with the main lane gets a sync lock.
Without a sync lock, lanes other than the edited one stay where they are
(linked sound always follows its picture).

**Sequences.** `sequence {action: new | duplicate | switch | rename |
delete | nest, name, sequence, clips}`: keep cuts side by side (duplicate
before a big change, switch back), nest clips into a sequence of their
own (`nest` with `clips`), cut a sequence into another with
`insert {sequence}`. `editorial {ask: sequences}` lists them.

**Organising a big edit.** `take {media, scene, take, camera, circled,
rating, note}` keeps the script supervisor's facts on a file; `take
{auto: true}` reads them from names like `S12_T3`, `Sc4-Tk2_CamB`,
`12A-3`. `string_out {scene, circled, media, gap}` lays every take (of a
scene, or only circled ones) end to end in a new sequence, in scene and
take order, each with a marker naming it; `selects {takes: [{media, from,
to, label}], name}` makes a selects reel of chosen moments. Both open the
new sequence (`open: false` to stay); switch back with `sequence`.

**Script-based editing.** `editorial {ask: find_takes, line: "I told you
I was at the docks"}` lists every take that says the line, with source
times, how closely it says it, scene/take (circled first) and whether the
cut already uses it: cut the best in with `insert` / `overwrite`, or put
them all in a `selects` reel to compare readings. **Text-based editing**:
`editorial {ask: transcript, clip}` gives a clip's lines, numbered; then
`text_cut {clip, remove: ["um basically", "you know"]}` takes those words
out of the picture and sound (and sync-locked lanes) and closes up;
`lines: [3, 4]` drops whole lines, `keep: [1, 5]` keeps only those (a
paper edit), `ranges: [[a, b]]` cuts source seconds. Word times inside a
line are shared out by length, so check the cut by ear on short words.

**Speed ramps.** `speed_ramp {clip, speeds: [1, 0.25, 1]}` (even steps
through the clip's footage) or `points: [[0, 1], [0.4, 0.2], [1, 1.5]]`
(position 0..1 through the footage, speed). The clip keeps its footage;
slow parts last longer and later clips follow. Then `smooth_motion` on
the clip fills the slow parts with optical-flow in-betweens so the ramp
does not judder.

**Nudges and sync.** `nudge {clips, by, linked}` moves clips by any
amount, a fraction of a frame included (sound can sit between frames:
`by: 0.01`). `linked: false` moves them without their linked picture or
sound (linked selection off), leaving the link in place, now out of sync
by `by`: for slipping sound into sync with picture. `move` takes
`linked: false` the same way.

**Markers.** `marker {at, name, colour: red|orange|yellow|green|cyan|
blue|purple|white, note, duration, clip}`: a coloured marker with a note
(on a clip when `clip` is given). The index reads them back.

**Looking things up**, with the `editorial` tool: `match_frame {t}` (the
media and second on screen, to cut in from the same moment), 
`reverse_match {media, t}` (where a source moment is already used),
`gaps`, `duplicates` (the same footage twice, a flash frame repeated),
`index {kind}` (every clip, marker, transition and effect in time
order), `lanes`, `takes {scene}`.

## Reference: Creative moves that work

- **Hit the cut**: a 0.15–0.3 s `fx_layer` (zoom blur, RGB split, shake,
  flash) on a cut, its strength animated to zero.
- **Transitions with intent**: whip / zoom / spin between energetic shots
  (0.2–0.4 s), dissolve / dip between calm ones (0.6–1.2 s).
- **Punch-ins**: `animate` `scale` `relative` `[[0, 1.12], [0.2, 1]]` on
  beats, or `[[0, 1], [3, 1.08]]` across a shot for a slow push.
- **Speed ramps**: `split`, then `set` `speed` 0.4 on the moment before a
  drop and back to 1 at it.
- **Readable titles**: light on dark or dark on light; or `backing` on the
  title (any colour of text then reads, the accent stays yours); keep
  vertical titles between 8% and 78% of the height.
- **Match cuts**: `match` from the outgoing clip (`by: "shape"`), look at
  the sheet, `place` the best one, verify with `frame` on both sides,
  fine-tune with `roll` / `slip`.

## Reference: Time: speed, stretching, freezes

- **Speed ramps** (`animate` `speed`): slow into a moment, fast out of it;
  ramp over at least 0.2 s (a jump in speed lurches). Slow motion needs
  footage shot fast; 30 fps footage at 0.3x stutters (use it for a beat,
  not a scene), unless the in-betweens are made: `smooth_motion` on the
  slowed or ramped clip makes them with RIFE optical flow (a cross-fade
  with `method: "blend"`), so every frame is new. Use it for slow motion
  of motion the eye follows (a jump, a pour, a turn of the head, a car
  passing); not on footage already shot fast (60 fps and up at 0.5x needs
  nothing: it says so), on cuts inside the slowed part (it morphs one shot
  into the next: split at the cut first), or on fast chaotic motion
  (water spray, fire, crowds) where flow tears; there a cross-fade or the
  plain stutter is better. Do it last: it bakes the slowed part into new
  footage (the clip keeps its timing, its sound is detached and stays),
  so trim and ramp first; `remove: true` takes it back.
- **Stretching** footage to fit a gap (a speed of 0.8-1.25) is invisible;
  past that it shows.
- **Freeze frames** (`freeze`) stop a moment for a caption or a joke;
  hold 0.5-2 s, with a sound or a zoom to mark it.
- **Time drops** (the music's drop, the moment the beat comes back):
  land the biggest visual change exactly on it: the reveal, the cut to
  the hero, the title hit. Build tension into it (a riser, a slow
  push-in, quieter picture) and release on it. `listen` finds the drops.

## Reference: tool `import`

Bring media files (video, audio, images, layered PSD documents, Lottie animations .json / .lottie, which play with their transparency) into the project; `to_timeline` also lays them on the main lane in order. A PSD comes in as its merged picture; with `layers: true` each layer becomes a picture clip on its own lane, where it sat (the document fitted to the canvas), with its opacity, blend mode and visibility, merged into one clip named after the file (`unmerge` opens it; `merge: false` leaves the layers apart) at `at` for `duration` seconds (default 5).

## Reference: tool `download`

Bring media from the internet into the project: a link straight to a video, sound or picture file, or a web page whose video or sound is openly exposed. For a page Zornov looks through its video and audio elements, Open Graph and player tags, VideoObject data, alternate links, oEmbed and the HLS (.m3u8) and DASH (.mpd) playlists named in the page or its scripts, then downloads the best version and, for a playlist, joins it into one MP4 (quality picks another: best, lowest, audio for sound only, or a height such as 720). Media that is protected (DRM), hidden behind a site's signature or sign-in, or not exposed at all is refused with the reason; Zornov never logs in for the person or works around protection, so tell them plainly and ask for a file or another link. Saved in the project's Downloads folder and added to the media bin; with to_timeline also laid on the main lane. Only use media the person has the right to use.

## Reference: tool `search_media`

Find real pictures, video, sound and 3D before making any. The library answers first (`library`), then the web like a search engine (picture and video result pages), plus Openverse (pictures, sound), Wikimedia Commons (pictures, video, sound) and Poly Haven (3D models, textures, HDRIs). Fields: query, kind (image, video, audio, 3d, texture, hdri), max (default 8, at most 20), thumbs (default true). The answer is a numbered list (title, source, size or seconds, author, source page) and one numbered sheet of thumbnails: look at the sheet, pick, then `search_media {fetch: N, name, to_timeline}` brings result N into the project through the same path as `download` (a video page is read for its video) and keeps the page it came from with the file. A Poly Haven model comes as a glTF folder to open with the 3D tools; a texture comes as colour, normal and ARM maps; an HDRI as a sky file. `unavailable` names a source that did not answer; the others still did. Use found pictures as references (`image`) for generation and modelling. Say which media were found and which were made.

## Reference: tool `editorial`

The look-ups an editor makes while cutting. ask: match_frame {t, lane} (the source frame on screen: media and second, to cut in from the same moment), reverse_match {media, t} (where that source moment is used), gaps {min}, duplicates {min} (the same footage used twice), index {kind: clip|marker|markers|transition|effect|gap} (every clip, marker, transition and effect in time order), sequences (every cut and nest), lanes (sync locks, targeting), takes {scene} (scene, take, camera, circled, rating, note per file), transcript {clip} (the lines a clip says, numbered, for text_cut), find_takes {line} (every take that says a script line, best and circled first, with source times).

## Reference: tool `search`

Search everything Zornov knows across every project on this Mac at once: on-screen text and titles, what narration says (and when), markers, media files and which projects use them, the AI editors' notes, saved moves and designs. Every word must match (prefixes count); best matches first. Use it to find where something was said or shown, reuse a move or a design, or find footage used before.

## Reference: tool `catalogue`

What the library has: pictures (stickers, emoji, shapes, backgrounds), sounds and music, motions, effects, looks, text styles, transitions. `query` as a list of words (or no panel) searches pictures, sounds and motions for every word in one call: ["bird", "broken heart", "wedding"]. With a panel and one query, that panel in detail.

## Reference: tool `see`

Zornov looks for you and says what it sees, in words (a few dozen tokens a line instead of hundreds for a picture): light (brightness, blown-out, pure black), contrast, dominant colours and how vivid, faces (how many, how close, where), where the subject sits, camera motion (still, pans which way, how fast), sharpness, black bars, lettering on screen, and what is playing there. No arguments: one line per main-lane shot. t: one moment. clips: those clips. media (+ from, to, every): footage before it is used, one line per step (times in the file). place + t: the backdrop read for staging (characters left out, or the clips in without): where the far land ends, which parts are ground, spots to stand on with the height a character there should have, and pieces (rocks, trunks, grass) a character can stand behind (foreground cuts one out). regions + t: what the backdrop is made of (sky, water, ground, path, trees, rocks, mountains, buildings, people, animals): how much, where, how far, so effects and particles can go inside one. Use frame / contact_sheet only when you need the picture itself.

## Reference: tool `listen`

Hear a song, a sound effect or a clip's sound, described as an editor hears it. Songs: an energy line, sections (quiet / medium / loud with times), drops and breaks, bpm, beats, bar length. Short sounds: their shape (hard hit, long-tail boom, swell, swoosh), attack, tail, brightness. Always, under heard: what it is by ear, with times (speech, music, speech over music, applause, noise, silence), faults (clipping, clicks, hum, rumble), the voice (pitch, pace, how roomy) and loudness against delivery targets (LUFS, true peak), in plain words (sounds_like). With mix: true, the timeline mix from..to: the same, plus how far under the voice the music sits while it speaks and where it masks the words. Times are in the file, or on the timeline for a clip.

## Reference: tool `script`

The whole timeline read like an edit decision list: lane by lane, one line per clip with its time, source range, speed, scale, position, looks, effects, keys and sound; transitions under the clip they leave; markers; the start of your notes. Ends with the revision; with since (a revision you already read) only the lines that changed.
