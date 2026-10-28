---
name: review-before-export
use: Use when an edit looks finished: look, listen, check, review, fix, report honestly.
topics: review, director, lifeless, viewer, repetition, repeated, honesty, honest, report, reporting, overdoing, restraint, checklist, too-much, quality-check
tools: see, frame, contact_sheet, listen, check, review, script, summary, inspect, graph, scopes, notes, undo
---
**Order**
1. Look across the whole edit: `contact_sheet` for the film, `frame` at the middle of key moments (`contact_sheet` with `every: 0.05` over fast moves, `detail: true` for small text and edges), `see` for a line a shot. Listen where sound matters: `listen {mix: true}`.
2. `check`: gaps, clips past their footage, dark stretches, sound past the picture, clipping, loudness, titles cut off or unreadable, overdoing (`effects_piled`, `transition_every_cut`, `flash_overuse`, `strobing`, `sfx_overuse`, `too_many_fonts`, `palette_sprawl`). Fix each, then check again.
3. `review`: watches the film as the audience will. Act on every note: one place held (`place_held`), even cuts, cuts mid-sentence or off the beat, gliding characters (`slides`), the same acting everywhere, a spoken thing not shown (`unshown`), no ambience, long still stretches, stiff or jittering motion, faked changes.
4. Overdoing, frame by frame: one focal point; one or two effects per layer; one or two transition kinds, mostly straight cuts; two fonts; flashes only on the biggest hits; sound effects only where something lands; every line stays long enough to read; it works with the sound off.
5. Fix, look again, `check` and `review` again after the last change. A film is finished when `review` has no notes or every remaining one is a choice you can name.
6. Report in three parts: verified by looking (frames or sheets you saw), applied but not looked at (set per the `edit` read-back), and failed or left out. Name every workaround for what it is.

**Checks**
- Nothing remains unlooked since the last `edit`.
- Every `3d_` note is fixed or listed as a set-aside choice.
- The report matches the read-back, not your intention.

**Mistakes**
- "Nothing to fix" while `review` still has notes.
- Passing `check` and `review` taken as "it looks good": it only means no fault was found.
- Calling a cross-fade a morph, or a still slid across a character walking.
- Looking only at stills 0.7 s apart and missing a snap.

## Reference: The director's review

`check` passes a film a viewer still finds dead: every clip can be
correct while the whole is one place, one pose and one rhythm. Before
calling anything done, watch it the way the audience will: `review`.
It returns notes with times, clips and fixes; a film is finished when it
has none, or when every one left is a choice you can name.

- **The place changes.** One backdrop framed one way for minutes
  (`place_held`), or filling a third of the film (`place_reused`), reads
  as a slideshow. Each beat gets its own view: another part of the
  place, closer or wider, a new angle, the light or weather moved on.
- **Shots are cut, not metered.** Thirty shots of exactly twenty
  seconds (`even_cuts`) beat like a clock. Cut where the story turns;
  never in the middle of a sentence (`cut_mid_sentence`); under music,
  land a cut on a beat where a sentence ends near one (`cuts_off_beat`).
- **Characters walk; they do not glide.** A layer moved across the
  ground while no foot stays planted (`slides`) is a sticker being
  dragged. A walk plants one foot, carries the body over it, then the
  other; the body travels only as far as the steps carry it.
- **Everyone acts for themselves.** The same keyed performance pasted
  into scene after scene (`same_acting`), every character moving on the
  same key times (`moves_alike`), or one move for most of the cast
  (`one_move`) is the "everyone moving alike" a viewer notices at once.
  Each character has its own tempo, and reacts after the one who leads.
- **What is said is seen.** When the voice says "a little fish circled
  the last puddle", a fish and a puddle are on screen then (`unshown`).
  The review matches the voice's words against the names of what is on
  screen (layers, files, titles, clip notes, markers), and knows a
  character by the name the story gives it ("Molly the rabbit"). Name
  layers for what they show; show the thing, cut to it, or have someone
  point at it.
- **Places sound.** Every place has a bed of its own (`no_ambience`),
  and the bed changes when the place does (`same_ambience`).
- **The picture moves on.** From a contact sheet of the cut, `review`
  finds long spans where the picture barely changes (`still_picture`)
  and frames minutes apart that look the same (`looks_same`).
- **Motion has weight.** Big moves at one constant speed that stop dead
  (`stiff_motion`) look mechanical; ease them in and out and let parts
  follow through. A position that turns back every few frames (`jitter`)
  reads as a glitch. A run of near-identical layers swapped one after
  another, or two pictures cross-faded, is a change faked, not made
  (`faked_change`).

Passing `check` and `review` means no fault was found, not that the film
is good. Say what you saw. Report in three parts: verified by looking
(frames or a contact sheet you looked at), applied but not looked at
(the `edit` read-back shows it set), and failed or left out. Name every
workaround for what it is: "faked with 26 models swapped every 0.1 s",
"a cross-fade, not a morph", "a still picture slid across". Never call a
work-around by the name of the thing it stands in for, and never say an
edit stuck until the read-back shows it.

## Reference: Overdoing: the checklist

Before you say it is done, ask of every frame:
- Is there **one** focal point? Remove what competes.
- At most **one or two effects** on any layer, and is each one doing a
  job? (`check`: `effects_piled`.)
- At most **one or two transition types**, mostly straight cuts.
- At most **two fonts**, **one or two accent colours**.
- Flashes, shakes and glitches: **only on the biggest hits**.
- Sound effects: **only where something lands or moves big**.
- Does every line stay on screen long enough to read?
- Does it still work with the sound off?

## Reference: tool `check`

Find what is wrong before exporting: gaps in the main lane, clips running past their footage, titles off the canvas or too wide, unknown effects, audio clipping / loudness (peak dBFS, LUFS), black, dark or flat frames. Each problem has a time, a clip and a fix. `accept`: findings that are intended, [{"kind": "3d_buried", "clip": 12, "why": "the boat sits in the water"}] (without clip: that kind everywhere); they are kept with the project (accepted.json) and listed under `accepted`, not as problems, so they stop blocking. `clear: true` forgets them all.

## Reference: tool `review`

The director's review: the whole film watched as a viewer watches it, for what makes it lifeless though check finds nothing. Notes, each with times, clips and a fix: one backdrop held shot after shot or filling much of the film, every shot the same length, cuts in the middle of a sentence or off the music's beat, characters gliding over the ground with no foot planted, one keyed performance copied between shots or stamped on every character, everyone in a shot doing the same thing, sentences naming things (animals, places, objects, characters) that are not on screen while they are said (matched against layer, file and title names, clip notes and markers), places with no sound of their own or one bed under many places, and from a contact sheet of the cut: long spans where the picture barely changes and frames minutes apart that look the same; and from the keys, motion that is stiff (big constant-speed moves that stop dead), jittery, or a change faked by swapping near-identical layers or cross-fading two pictures. Run it before you say the film is done and act on every note. A note that is intended can be accepted (`accept`, as for check) so it no longer blocks; `check {clear: true}` forgets acceptances.
