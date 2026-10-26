---
name: music-beat-sync
use: Use when a film is built to music: choosing a track, beats, drops, montages.
topics: scoring, score, composing, composition, original-music, music-making, film-scoring, cues, cue-points, underscore, music, beats, beat, beat-sync, montage, music-video, drop, drops, tempo, bpm
tools: listen, analyse, music, catalogue, search, add, place, split, trim, fx_layer, sfx, animate, audio, marker, notes, contact_sheet, check
---
**Order**
0. No fitting track, or music that must hit moments in the picture: `music` composes it (skill `music-beat-sync` part Scoring).
1. Choose the track by tone and tempo: fast cuts want 120+ BPM, calm pieces 70-100. `catalogue {panel: "audio", query, mood, min_seconds}` ranks tracks; prefer the library or CC0, and note any credit a licence needs. One track per mood; change it where the mood turns.
2. `listen` to it: sections (main, peak, low, quiet), drops, breaks, bpm, bar length, beats, energy. Write the beat map in `notes` (time of each section, the first drop, the bar length).
3. Scout every source clip (`contact_sheet {media}`, `see {media}`) and pick the strongest moment for each slot.
4. Lay shots in order with durations of 1, 2 or 4 beats, faster in the peaks, slower in the quiet sections; put the film's biggest moment on the first drop.
5. Hits on drops: `fx_layer` (zoom blur, RGB split, shake, flash) 0.15-0.3 s on the cut, its strength animated to zero; punch-ins by `animate` scale on beats. Riser into the drop: `sfx {kind: "riser", ends_at}`.
6. Fit the music to the picture: `music` fits a track to the length with a fade at the end; to shorten a song keep the start and real ending, remove whole bars from the middle and join on matching downbeats with a short crossfade.
7. Under speech the music dips 10-14 dB (skill sound-design).

**Checks**
- Cuts land on beats where a sentence or action ends near one; readability and story come before the grid.
- The ending is the song's own last hit or a 2-4 s fade, never a dead cut (`music_cut_off`).
- `listen {mix: true}` shows the voice still clear.

**Mistakes**
- Cutting on every beat: it becomes a metronome. Vary 1, 2 and 4 beat shots.
- Chopping the end of a song to fit the length.
- Several tracks fighting for one section; `check` flags one track under a film whose notes name several moods.
- Flashes and shakes on every beat: only the biggest drops.

## Reference: Recipe: a music video

**Music video** — `listen` to the song (sections, drops, bar) → write the
beat map in `notes` → `contact_sheet` with `media` for each source →
`place` shots in order with durations of 1, 2 or 4 beats, faster in the
peaks, the strongest shot on the first drop → `fx_layer` hits on the
drops → `check`, `script`, `contact_sheet` → `export`.

## Reference: Scoring: original music made in Studio
**Order**
1. Read the cut first (`summary`, `listen`, markers): where does the story turn, where is the biggest moment, where does it end? Those times are the cues.
2. Say what it is in a composer's words: genre, mood, who plays. `"noir jazz with sax and double bass, rainy night"`, `"hopeful acoustic folk, slow build"`, `"dark slow score"`, `"minimal electronic on a beat grid"`. Named instruments take the lead, the bass or the chords.
3. Call `music` (it composes here, always, with nothing to set up): `genre` or `describe`, `instruments`, `cues`, `tail`; `bpm` and `key` only when they matter; `duration` and `at` for a stretch (default: the whole video). With only a `mood` it takes a fitting library track first and composes when the library has none.
4. Read the reply: genre, key, tempo, band, where the last chord falls, loudness, and where each cue landed (on a downbeat, on a beat, or between beats when no tempo fit). Then `listen` or `check`. Another take of the same words: `variant: 1, 2`.
5. Under narration it sits low and dips while someone speaks; set `volume` to override. Leave a beat of silence before the first spoken line.

**Cues** are times on the project timeline: `{"at": 2.0, "kind": "hit"}`, or `"hit": 2.0`, `"resolve": 14.5`, `"riser": 8`.
- `hit`: cymbal, low boom and a chord stab exactly on the time; the band swells into it.
- `resolve`: the harmony lands home on the time (the bar before leans on the dominant): a reveal, calm after trouble.
- `riser`: a synth riser climbing to the time, then a hit. `drop`: everything stops for the beat before, then a hit. `swell`: strings build to the time and cut.
The grid bends a little so each cue falls on a beat (more when no `bpm` was given); a given `bpm` is kept unless cues force a small bend.

**Fit**: the piece is exactly as long as asked; the last chord falls at length minus `tail` (about 8 percent, 1.5-4 s) and rings out into silence, so it needs no fade. To end on a picture cut, make `duration` end there.

**Checks**
- The reply's cue list matches the times you asked for.
- `check` reports no `music_cut_off` or `voice_buried`; music sits about -10 dB, lower under speech.

**Mistakes**
- Asking for a real recording's performance: say this is composed and synthesised (musical and in tune, tight to picture, not a recorded band); good for underscore, beds, stings and temp score.
- Cue times relative to the music instead of the timeline when `at` is set: cues are timeline seconds.
- Same words every time: change the words or use `variant` for another take.

### Genres, moods and instruments
Genres (any near word works): jazz (noir, swing, lounge), folk (acoustic, country, ballad), cinematic (score, orchestral, epic), dark (ominous, horror, tense, suspense), electronic (minimal, techno, house, synthwave), ambient (calm, drone, meditation), lofi (chill, hip hop), rock, piano (classical, ballad, lullaby), pop (upbeat, corporate), playful (quirky, comedy, circus).
Mood words: dark, sad, tense, noir (minor, slower); hopeful, happy, bright (major); slow, calm, fast, driving, upbeat (tempo); build, rising, fading (shape); sparse, minimal (thinner); epic, big (fuller).
Instruments: piano, electric piano, organ, acoustic or nylon guitar, harp, double bass, electric bass, synth bass, sax, trumpet, trombone, horn, flute, clarinet, violin, cello, strings, pad, choir, synth lead, synth pluck, bells, marimba, vibraphone; drums, brushes, or "no drums".

### Calls
`music {"genre": "noir jazz with sax and double bass", "duration": 40, "cues": [{"at": 2.0, "kind": "hit"}, {"at": 34.0, "kind": "resolve"}]}`
`music {"describe": "hopeful acoustic folk, slow build", "bpm": 84, "key": "G major"}`
`music {"genre": "dark slow score", "tail": 4}`
