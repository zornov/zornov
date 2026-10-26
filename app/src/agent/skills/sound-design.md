---
name: sound-design
use: Use when sound matters: effects, ambience, dialogue, mixing, loudness, subtitles, dubbing.
topics: foley-making, foley-sounds, sfx-models, making-sounds, animal-sounds, local-sfx, footsteps-made, singing, sing, scat, vocals, sung-voices, sound, sounds-design, sfx, audio, mix, mixing, silence, sound-editing, audio-editing, cutting-audio, chopping, crossfades, ducking, eq, compression, j-cuts, music-editing, film-sound, film-mix, re-recording, surround, loudness, stems, adr, subtitles, dubbing, localization, localisation, m-and-e, dialogue, cutting, dub, music, sound-design
tools: listen, sfx, sing, add, foley, audio, animate, narrate, voices, mix, loudness, stems, dialogue, adr, subtitles, dub, ai, catalogue, music, split, trim, move, slip, detach_audio, j_cut, l_cut, check, review
---
**Order**
1. `listen` first: to each source (faults such as clipping, hum, rumble, with times and a fix) and later to the mix (`listen {mix: true}`: how far under the voice the music sits).
2. Layers, top to bottom: voice (about -16 LUFS, always understandable), then effects short and under the music, then music as a bed dipped 10-14 dB under speech (`narrate` does the dip; otherwise key the music's `volume` with `animate`: down 0.15 s before the line, back up 0.4 s after).
3. Clean the voice: `ai {voice_cleanup}`, then `audio` effects: eq (`high_pass` 80 Hz, a little presence), compressor (ratio 3-4), `de_esser` for harsh voices, `gate` for hiss. Fill every gap with room tone, never digital silence.
4. Effects: give each moment its own sound (`sfx` makes foley and effects from a description with no file: skill `sound-design` part Foley; original music: skill `music-beat-sync` part Scoring; `add {query, kind}` takes an unused one each time; `sfx` makes a variant). Whoosh as long as the move, starting with it; impact on the frame of contact; riser ending exactly on the drop (`sfx` with `ends_at`). Foley for characters: `foley`.
5. Every place gets a bed (`sfx` kinds stream, rain, wind, birds, forest, night...) about 20 dB under the voice, changed when the place changes.
6. Edit sound: cut dialogue in the pauses, music on a downbeat, join with a 30-150 ms `equal_power` crossfade, end on the song's last hit or fade 2-4 s (`audio` `fade_out`). 4 ms ramps on hard edges are automatic.
7. Film mix and delivery: set each lane's role (dialogue, music, effects, background) before `mix`; `dialogue` for level and match; `adr` for lines that cannot be saved; `loudness` last, to the destination's spec; `stems` for DX / MX / FX / M&E; `subtitles` and `dub` for other languages.

**Checks**
- `check` reports `voice_buried`, `sfx_overuse`, `music_cut_off`, clipping and loudness; fix each.
- Loudness is measured on the export, not guessed: about -14 LUFS social, -16 speech-led, -23 broadcast EBU R 128, true peak under -1 dB.
- Picture and sound line up within a frame or two after any move.

**Mistakes**
- A whoosh on every move: 3-5 cues in a short video.
- Narration buried because levels were set by hand: leave `narrate`'s leveling alone unless `listen` says otherwise.
- Normalising before the mix is final, or limiting stems.
- An M&E with silent footsteps: recreate foley and effects in the FX stem.

## Reference: The sound department

A film mix, its deliveries and its other languages, with seven tools
(one undo step each; the window's Sound panel, ⇧X, works the same mix).

**Mix** (`mix`). `show` first: every lane with its role (dialogue,
music, effects, ambience), the bus it ends up in, fader, pan, sends,
inserts and surround position; the buses; the master; the layout.
- `setup` adds the film buses: DX (lightly compressed and de-essed), MX,
  FX, BG and a Reverb return. Lanes go to the bus of their role; the
  main lane is production dialogue unless told otherwise. Set a role:
  `{"action": "lane", "lane": "Car", "role": "effects"}`.
- Lane: `volume`, `pan`, `mute`, `solo`, `bus`, `send: {bus: "Reverb",
  level: -14}`, inserts (`effect` + `params`, same kinds as clip sound:
  eq, compressor, limiter, gate, de_esser, reverb with a `room` preset,
  delay), `surround: {x, y, z, width, centre, lfe}`, `object: true`.
  Buses and the master take `volume`, `mute`, `solo` and inserts; the
  master also `ceiling`, `limiter`, `ducking`.
- `layout`: `stereo`, `5.1` or `7.1`. In surround, dialogue sits in the
  centre speaker, music spreads across the front without it, ambience
  wraps the room, effects go where `surround` puts them (x −1 left..1
  right, y −1 back..1 front; the middle of the room is everywhere).
  `lfe` sends lows to the sub. The stereo export is a true stereo mix,
  not a fold-down.
- `automate {lane | bus, param: volume | pan | x | y | z | send (+ bus),
  keys: [[seconds, value], ...]}` rides a fader or flies a sound through
  the room (a car passing: x from −1 to 1, y from 1 to −1).
- `meters {at, duration}`: peak per lane, bus and channel, and loudness
  (integrated, short-term, momentary, true peak) over a stretch.

**Delivery** (`loudness`, `stems`).
- `loudness {target}` measures the delivered mix against a spec:
  `ebu_r128` −23 LUFS / −1 dBTP, `atsc_a85` −24 / −2, `netflix` −27
  dialogue-gated / −2, `amazon` −24 / −2, `streaming` −14 / −1,
  `podcast` −16 / −1. `normalise: true` sets the master gain and the
  limiter ceiling to meet it in one step and measures again; `passes`
  and `problems` say where it stands.
- `stems {path, layout, separate_bg, adm}` writes 24-bit Broadcast WAVs:
  DX, MX, FX, M&E, the full mix, and per dubbed language its DX and mix.
  Stems add up to the mix before its limiter. `adm: true` also writes an
  ADM file (bed plus a moving object per `object` lane) for immersive
  mastering.
- `export` (MP4) carries a stereo track, a 5.1 one too when the mix is in
  surround, the same for every dubbed language (tagged, alternates in
  the player's menu), every subtitle track, and `.srt` / `.vtt` beside it.

**Dialogue** (`dialogue`, `adr`).
- `dialogue {action: level, clips, target}` brings each phrase to the
  target (−24 LUFS default; −27 film, −16 online) as the clip's volume
  keys. `{action: match, clip, reference}` gives a pickup or ADR line
  the reference's EQ, level and room.
- `adr`: `add {start, end, character, text, reason}` or `from_subtitles`
  builds the cue list; `sheet {path}` writes the cue sheet (timecode in
  and out); `beeps {path}` the three-beep guide; `take {cue, file}`
  brings a recorded take in, lined up with the production line (`warp:
  true` word by word), matched to the production voice, on the ADR lane,
  with the production line dipped under it (`replace: false` keeps it).
  In the window, Record plays the picture from 4 s before the line with
  a 3-2-1 streamer and records the microphone.

**Other languages** (`subtitles`, `dub`).
- `subtitles {action: make, lang}` transcribes the dialogue and times it
  by the spec (`netflix`: 42 characters a line, 2 lines, 17 characters a
  second, 5/6 to 7 s, 2 frames apart, snapped to cuts; `bbc`, `social`).
  `check` lists every rule broken, `conform` fixes timing, `speakers
  {names: {"1": "MAYA"}}` says who speaks each line, `edit` changes one.
- `translate {from, lang, texts}`: give one translation per subtitle,
  in order (you are the translator: keep each line about as long as the
  original so it reads in the same time; the reply lists lines that read
  too fast). Without `texts` it uses the OpenAI API with the person's key.
- `burn_in {lang}` puts styled captions in the picture; `export {lang,
  path, format}` writes SRT or VTT; `import {path, lang}` reads one.
- `dub`: `characters` lists who speaks; `voice {character, lang, voice}`
  picks each character's voice per language (voices of the service
  chosen in Settings: see `voices`); `make {lang, fit: phrases}` speaks
  every subtitle of that language and fits it to when the original actor
  speaks (`phrases` follows the original's pauses, for lips). The
  language then has its own mix: music and effects (M&E) with the dub.
  Lines in `reword` do not fit even at 25% faster: shorten their
  translation and make them again (`cues: [n]`). Each dubbed line has a
  words file beside it for lip sync.

## Reference: Sound

- **Layers**: dialogue or narration on top (about -16 LUFS, always
  understandable), music under it (about -10 dB as a bed, dipped 10-14 dB
  under speech; `narrate` does this), effects in between and short.
- **Choosing music**: match the tone and tempo to the cut (fast cuts
  want 120+ BPM; calm pieces 70-100). One track per video, or a clear
  change at a section. Mind the licence: CC BY needs a credit (in the
  end card or description); prefer the library or CC0.
- **Choosing effects**: whoosh for movement and transitions (as long as
  the move), hit / boom for landings and drops, riser into a drop
  (ending exactly on it), clicks and pops for UI and small arrivals,
  ambience (room, street, nature) under real places. `sfx` `describe`
  makes any sound in words.
- **Restraint**: 3-5 cues in a short video; not every movement needs a
  sound (`check`: `sfx_overuse`). Effects sit under the music, never
  harsh; two cues within a tenth of a second blur into mush.
- **Silence** is a tool: drop everything for half a second before the
  big moment.
- Beats: `listen` gives the tempo, beats and drops; cut and land hits on
  them, but readability and story come before the grid.

## Reference: Sound editing

The craft of cutting, chopping, fading and shaping sound, with Zornov's
tools (`split`, `trim`, `move`, `slip`, `animate` `volume` / `pan`,
`audio` for fades, curves and effects, `listen` to hear what you did).

**Cutting and chopping**
- Cut dialogue in the pauses, never mid-word: `listen` with words gives
  where each word starts and ends; cut 0.05-0.1 s into the silence.
- Cut music on the **downbeat** of a bar (the "one"), and remove or
  repeat whole bars or phrases (4 or 8 bars) so the rhythm never
  stumbles; `listen` gives the beats and the bar length.
- **Shortening a song** to fit: keep the intro's start and the real
  ending, remove whole sections from the middle, and join at matching
  downbeats with a 30-150 ms crossfade. **Lengthening**: repeat a chorus
  or a loopable section the same way. Never just chop the end off.
- **Endings**: end on the song's own last hit (a button ending) or fade
  out over 2-4 s; a song cut dead mid-phrase sounds like a mistake
  (`check`: `music_cut_off`).
- Zornov puts a 4 ms ramp on every hard sound edge so cuts never click,
  except where a clip simply continues into the next piece of the same
  recording.
- **J and L cuts** (split edits): start the next shot's sound under the
  end of the last picture (J), or let a sound carry on over the next
  picture (L): `trim` the audio clip separately from its picture
  (`detach_audio` first). They make cuts feel natural.
- **Room tone**: a gap in dialogue must not drop to digital silence;
  fill it with a quiet stretch of the same room.
- **Sync**: picture and sound must line up within a frame or two;
  check claps, hits and lips after moving things.

**Fades and crossfades**
- `audio` `fade_in` / `fade_out` (seconds) with a `curve`:
  - `equal_power` for crossfades (no dip in loudness at the middle),
  - `s_curve` for smooth starts and ends of music,
  - `fast` (quick drop then a tail) to get out of the way,
  - `slow` (holds, then drops) to let something ring out,
  - `linear` rarely: it sounds like it drops too early.
- **Crossfade** two sounds by overlapping them on two sound lanes
  (0.5-2 s for music and ambience, 30-150 ms inside a music edit) with a
  `fade_out` on the first and a `fade_in` on the second, both
  `equal_power`.
- Short fades (5-20 ms) on dialogue edits; 1-3 s for music in and out;
  ambience fades longer than you think (2-5 s).

**Levels and ducking**
- Targets: about -14 LUFS for social video, -16 for podcasts and
  speech-led pieces, peaks under -1 dB. `check` measures the mix.
- Voice on top: narration and dialogue clear and steady; music dipped
  10-14 dB under speech (`narrate` does it; otherwise key the music's
  `volume` with `animate`: down 0.15 s before the line, back up 0.4 s
  after). `check`: `voice_buried`.
- Ride levels with `animate` `volume` rather than a static compromise: a
  swell into a drop, a dip under a line, a fade into silence.
- **Pan** to place sounds: centre for voice and music; a whoosh can travel
  left to right with the motion it belongs to.

**Shaping with effects** (`audio` `effect` with `params`)
- **eq**: for voice, `high_pass` 80 Hz (removes rumble), cut `low_mid`
  2-4 dB if boxy or muddy, a little `presence` (+2-3 dB) for clarity,
  `air` for sparkle; for music under a voice, cut `presence` 2-4 dB so
  the words sit in that space.
- **compressor** on voice: threshold around -18 dB, ratio 3-4, attack
  10 ms, release 100 ms, a little makeup: steadier, closer, more present.
  Do not squash music that is already mastered.
- **de_esser** on harsh voices (freq around 6-7 kHz).
- **gate** to quiet hiss and room noise between words (gentle range,
  -20 dB, not a hard cut).
- **limiter** on the loudest moments: ceiling -1 dB.
- **reverb** gives a place: a small `mix` (0.1-0.2) glues a voice into a
  scene; big, long reverbs are a special effect (a tail on the last
  word, a dream); never on a voice that must be understood clearly.
- **delay** for echo effects and transitions, sparingly.
- **pitch_shift**: a riser up, a slowed-down drop, a voice effect; for
  speed changes keep `preserve_pitch` true unless the chipmunk or the
  slow-motion groan is the point.
- `ai` `voice_cleanup` removes noise from recorded dialogue.

**Sound design**
- Layer a big moment: a low boom + a mid hit + a high sparkle, short and
  together, rather than one loud effect.
- A riser ends exactly on the drop; a reversed sound (`reversed: true`)
  makes a pull-in before a hit.
- Whooshes as long as the move, starting with it; impacts on the frame
  of contact, not after.
- Silence before the big moment (0.3-0.6 s) makes it hit harder than any
  effect.

## Reference: Film sound: mixing, delivery and other languages

How a re-recording mixer, a dialogue editor and a localisation team
would do it (the tools: `mix`, `loudness`, `stems`, `dialogue`, `adr`,
`subtitles`, `dub`; guide topic `mix`).

**The mix is built from stems.** Every sound belongs to one of dialogue
(DX), music (MX), effects and foley (FX) or backgrounds (BG); each stem
has its bus, and the mix is the sum of the buses. Set every lane's role
before mixing: a wrong role puts a line of dialogue in the M&E.
- Dialogue first and in the centre: it carries the story and everything
  else is balanced against it. In 5.1 it lives in the centre speaker;
  music keeps out of the centre, ambience wraps the surrounds, effects
  follow the picture (a car passing flies across, an off-screen sound
  sits off-screen). Panning dialogue away from the centre is rare and
  deliberate (a voice from off-screen).
- The LFE is for weight (explosions, impacts, low drones), sent on
  purpose; nothing important may live only there (many systems drop it).
- One shared reverb return per kind of space, fed by sends, glues a
  scene better than a reverb on every clip. Match each line's room to
  where it is said (`room`: car, small_room, corridor, church…).
- Ride the faders through a scene (automation) rather than setting one
  compromise level: the music comes up in the gaps and steps back under
  lines; a background swells in at a cut to a new place.

**Dialogue editing.** Level phrase by phrase so a whisper and a shout
both read (`dialogue level`), then compress gently on the DX bus (2-3:1)
and de-ess. Fill every gap with room tone. A pickup or ADR line must
sound like the line before it: match EQ, level and room (`dialogue
match`) and check by ear across the cut.

**ADR.** Cue only what cannot be saved (noise under a line, a
performance or line change, a missing line); note the reason. The actor
hears three beeps a second apart and speaks where the fourth would be,
watching the picture. Sync to the production line, word by word when the
performance drifts; then match the room. Lips must stay within a frame.

**Loudness and delivery.** Mix to the spec of where it is going and
normalise in one step at the end: broadcast EBU R 128 (−23 LUFS, −1
dBTP), US broadcast ATSC A/85 (−24 LKFS, −2 dBTP), Netflix (−27 LKFS
measured on the dialogue, −2 dBTP: keep the dialogue level steady and
let the music and effects be dynamic around it), streaming platforms
around −14. Theatrical mixes are mixed at a calibrated room level, not
to a LUFS number; deliver the near-field (home) mix separately. Keep the
loudness range sensible for home viewing (roughly 10-20 LU).
- Deliver stems with the mix: DX, MX, FX, M&E. They must add up to the
  mix (no limiter on stems). An M&E must be *filled*: production
  effects and foley recorded under the dialogue disappear with it, so
  recreate them (foley, effects) in the FX stem, or the foreign version
  has silent footsteps.

**Subtitles.** Two lines at most, 42 characters a line (16 for Chinese
and Korean, 13 for Japanese), broken at a phrase boundary and balanced,
17 characters a second for adults, at least 5/6 s and at most 7 s on
screen, 2 frames between subtitles, in and out points on shot changes
when close. One speaker's line per subtitle when possible; speaker names
or dashes when two speak. SDH adds sound descriptions [door slams] and
speaker IDs; forced subtitles only translate on-screen foreign text and
foreign dialogue.

**Translation and dubbing.** Translate for the ear and the eye, not word
for word: a subtitle reads in the time the line is spoken, a dubbed line
is spoken in the time the mouth moves. Keep names, honorifics and
register; adapt jokes and idioms. For dubbing, the translated line must
fit the original's length and pauses (within about 10-25 %); open
vowels and closed lips (p, b, m) should land on the picture's mouths in
close-ups. Each character keeps one voice per language across the film,
cast for age, energy and timbre. A language's mix is the M&E plus its
dialogue; check it alone, then against the original.

## Reference: tool `mix`

The film mix: lanes into buses (dialogue DX, music MX, effects FX, ambience BG, aux returns like a shared reverb) into the master. action: show (default: every lane with role, bus, fader, pan, sends, effects, surround position; buses; master; layout; delivery), setup (adds DX/MX/FX/BG and a Reverb return), lane {lane, role: dialogue|music|effects|ambience, bus, volume dB, pan, mute, solo, send: {bus, level dB, pre}, unsend: bus, effect + params, remove_effect, surround: {x -1 left..1 right, y -1 back..1 front, z 0..1 height, width degrees, centre, lfe dB}, object: true (moves as an object in ADM export)}, bus {bus (name/id) or add: {kind, name}, volume, mute, solo, effect + params, remove_effect, remove}, master {volume, ceiling dBTP, limiter, ducking, effect + params, remove_effect}, layout {layout: stereo|5.1|7.1}, automate {lane or bus, param: volume|pan|x|y|z|send (+ bus), keys: [[seconds, value], ...] (replaces), clear: true}, meters {at, duration} (peak and LUFS per bus and channel over a stretch). Effects: eq (bass, low_mid, mid, presence, air dB; high_pass, low_pass Hz), compressor, limiter, gate, de_esser, reverb (room: booth, car, small_room, living_room, bathroom, office, corridor, hall, church, warehouse, cave, outdoor), delay, pitch_shift.

## Reference: tool `adr`

ADR (re-recorded dialogue). list; add {start, end, character, text, reason}; from_subtitles {from, to, characters, reason, lang}: cues for those lines; set {cue, status: to do|recorded|approved, notes, text}; remove {cue}; sheet {path}: the cue sheet as CSV with timecode; beeps {cue, path}: a WAV of three beeps a second apart ending one second before the line, to play to the actor; take {cue, file, warp: word-by-word sync, match: voice-match to the production, replace: dip the production line under it}: the take lined up with the production line and laid on the ADR lane.

## Reference: tool `subtitles`

Subtitles and captions per language. list; make {lang, spec: netflix|bbc|social, translate: true for Whisper's English}: from the dialogue with word timings, segmented by the spec's line length (42 chars, 2 lines for Netflix), reading speed (17 cps), 5/6-7 s and 2-frame gaps, snapped to shot changes; from_captions {lang}: the caption clips on the timeline as a track; import {path, lang}; export {lang, path, format: srt|vtt}; show {lang}; edit {lang, cue, text, start, end, speaker}; speakers {lang, names: {cue: name}}; conform {lang, spec}; check {lang, spec}: every rule broken; translate {from, lang, texts: [one per subtitle]} (your translation, timing kept; without texts the OpenAI API with the person's key); burn_in {lang, style}: styled caption clips in the picture; remove {lang}. Exports carry every track as a subtitle track and write .srt/.vtt beside the file.

## Reference: tool `dub`

Dubbing into other languages. characters: who speaks (from subtitles) and their voices per language; voice {character, lang, voice, pace}: the voice a character speaks a language with (a voice of the speech service chosen in Settings; see voices); make {lang, fit: span|phrases|natural, cues, characters}: every subtitle of that language spoken by its character's voice, fitted to when the original actor speaks (phrases follows the original's pauses, for lips), laid on the language's dub lane; the language then gets its own mix (M&E + dub) and audio track in exports; lines that cannot fit within 25% are reported to reword. list: dubbed languages.

## Reference: Foley, effects and singing made in Zornov
**Order**
1. List the sounds the scene needs and when each happens (spot sounds at moments, beds under places).
2. `sfx` each one: `describe` in words ("a match strike", "distant period traffic", "café murmur", "pig oinking") or `kind` with settings. They are made here by physical models, so they always work. Several sounds in one description become one passage; "then" puts them in order; separate ops give finer control.
3. Place with `at`, or `ends_at` to land the sound's main moment (a riser's peak, a match catching); the reply states the moment. One-shot effects keep their length; beds and passages fill `duration`.
4. Levels: effects arrive loud (peak near -2 dBFS), beds quiet. Foreground action -6 to -12 dB, beds about -20 dB under a voice, distant things -14 and lower. Use `variant` for another take instead of repeating one file.
5. `listen` the mix; say plainly when a shot rests on a synthesised sound.

**Sung parts**: `sing {"voices": [{"voice": "soprano", "notes": "E5:0.5b:doo G5:0.5b:bee C6:1b:bap r:0.5b A5:0.5b:da"}, {"voice": "tenor", "notes": "r:1b C4:0.5b:doo E4:0.5b:bee G4:1b:bop"}], "bpm": 110, "at": 4}`. Notes are `pitch:length:syllable` (lengths in seconds, or beats with `b`; `r:length` is a rest); or a list of `{pitch, start, dur, syl}`. Each voice is its own clip and lane, with every syllable's time in the reply, so a mouth can follow it (`puppet` with `voice` and the clip). Voices: soprano, alto, tenor, bass; notes far outside a range are reported. `vibrato`, `breath`, `pan` 0-1 and -1..1.

**Checks**
- The reply names the models used and the take; footsteps differ step to step; passages have all the layers you named.
- A sung part: pitch and timing are exact, the tone is plainly synthetic, it sings syllables not words; tell the person so.

**Mistakes**
- Identical effects repeated: vary with `variant`, `size`, `speed`.
- Beds as loud as the action: set `volume`.
- Using `sfx` footsteps for a character on screen: `foley` lays steps on the frames its feet land.

### What exists
Impacts (thud, slam, bang, smash, clang), knocks, scrapes and squeaks, creaks, match strike (scratch, hiss, flame that burns down), lighter, torch ignition, fire crackle, water pouring, dripping, splashing, bubbling and boiling, rain, wind and howls, engines idling or revving, a car passing (Doppler), traffic (`distant`; `period` adds bulb horns and hooves), horns, hooves, crowd murmur, a café (murmur with cups, cutlery, chairs, espresso machine), applause, clocks, switches, locks, doors (open, slam, bell), typing and typewriters, machine hum, servos, camera shutter, phones, heartbeat, breathing, whooshes, risers (with a `tail`), booms, interface sounds, thunder, explosions, gunshots, bells, sirens, sci-fi zaps, glass clinks and breaks, paper, cloth, sizzling.

### Animals and footsteps
Animals: pig (oink, squeal), horse (neigh, snort), hen (cluck, cackle), sheep (baa, lamb), dog (bark, growl, whine, howl), cat (meow, purr), cow, duck, rooster, crow, owl, frog, lion, elephant, donkey, mouse. Settings `species` and `call`, or words ("dog barking"); `size` lowers or raises the voice; `count` repeats; each `variant` is a different individual.
Footsteps: `kind: "footsteps"` with `surface` (grass, leaves, dirt, gravel, stone, wood, snow, sand, mud, water, metal, carpet, tile), `shoe` (boots, sneakers, heels, bare), `count` or `duration`, `speed`, `size` for weight.
Settings in general: `distance`, `size`, `speed`, `count`, `surface`, `shoe`, `material` (wood, metal, glass, stone, plastic, ceramic), `period`, `intensity`, `tail`.

### Recipes
- A match lighting a cigarette on a noir street: `sfx "distant period traffic"` for the scene (bed, low), `sfx "match strike"` at the moment, `sfx "footsteps on wet stone in heels"` as they walk away.
- A café: one `"cafe murmur"` under the scene; spot `"glasses clinking"` and `"door bell"` where they happen.
- A farm: hen, pig, horse, sheep each at its shot, with a wind bed.
- A transition: a whoosh as long as the move; a riser ending on the drop (`ends_at`), a boom on it.
