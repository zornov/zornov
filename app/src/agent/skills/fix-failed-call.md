---
name: fix-failed-call
use: Use when a tool call fails, is refused, does nothing or gives a wrong result.
topics: mistakes, lessons, trials, errors, failed, failure, troubleshooting, unchanged, refused
tools: capabilities, guide, edit_schema, howto, skill, inspect, summary, undo, make_playable, job, notes, check
---
**Order**
1. Read the error whole. An unknown field is refused with the right ones; a bad value says what is valid. Fix the call and run it again; do not retry the same call unchanged.
2. If it ran but changed nothing, the op is listed under `unchanged` in `read_back`. Say so and fix it: wrong clip reference, a time outside the clip, a property the clip does not have (`inspect` shows what it has).
3. Fields you are unsure of: `edit_schema` (ops), `guide {topic: "tool:<name>"}` (a tool's fields), or `capabilities {query}` (what does this and with what).
4. Media that will not preview or play: `make_playable`. A long job: `job` shows its state. Stale ids after merge, unmerge, flatten or delete: use the ids the last result names.
5. A call that did the wrong thing: `undo` takes back the whole last call; then redo it smaller.
6. Before saying Studio cannot do something, or reaching for a workaround, call `capabilities` with what you want and load the matching skill.
7. When you find a fix that was not obvious, save it: `skill {do: "learn", skill, lesson, kind: "fixed"}`, tagged to the skill the job belonged to.

**Checks**
- The retry changed the read-back the way you meant.
- A workaround is named for what it is in the final report.

**Mistakes**
- Giving up or asking permission for something a tool can do.
- Working around a failed `narrate` with another voice: say it failed.
- Piling effects to hide a result that was wrong; undo and redo it properly.
- Guessing field names; ask `edit_schema`.

## Reference: Mistakes we have seen (do not repeat them)

From real runs of the AI editors on this app:
- **A progress bar that sat still for most of a second, then jumped**:
  an `expo_in` curve on a fill. Fills and arrivals start fast and settle
  (`power2_out`). It was also scaled with `scale_x`, which squashed its
  round ends: animate `width` with `anchor: "left"`.
- **A zoom blur, a glow and a white flash ballooning to 2.6x height all
  on that bar**: it became a smear. One effect on a moving thing.
- **Only still frames were checked**, 0.7 s apart, so a 2-frame snap was
  never seen. Look at the move (`every: 0.05`) and run `check`.
- **Bars drawn under their tracks; a background drawn over the
  pictures**: stacking order guessed from lane numbers. Build back to
  front; use `restack` / `behind`.
- **Scenes unmerged and re-merged five times to change one thing**,
  layers padded by hand for transitions, a scene exported and
  re-imported as footage: edit inside merged clips by id, transitions
  between merged clips need no padding, `flatten` makes footage.
- **Stale ids after unmerge**: the old ids pointed at nothing visible.
  Use the ids each result gives.
- **Arcs on x and rotation**: arcs are for position only.
- **Relative opacity of 100**: relative values are factors (1.0 = as is).
- **A whoosh on every move** (20 effects in 35 s) and **five transition
  kinds** in one short video: restraint.
- **Narration that read the screen back and filled gaps with hype**
  ("Watch it fly!", "Just keep creating!"): say what the picture cannot,
  in the user's words, between the cuts.
- **Narration buried under music** because levels were set by hand:
  `narrate` levels the voice and dips the music; leave that alone unless
  `listen` says otherwise.
- **A stand-in "screen recording" built from cards** when the user's
  footage was missing: ask for the footage first (one short question
  before starting), and never present a stand-in as the real thing.
- **Flat, plain frames next to a designed competitor**: what made the
  difference was a dimmed or gradient background, a soft shadow under
  titles and cards, a framed picture-in-picture and one accent line or
  shape. Design the frame, not just the content.
