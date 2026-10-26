---
name: picture-batches
use: Use when several pictures must be generated at once: backdrops, characters, props.
topics: batch, batches, pictures-batch, parallel-pictures, image-workers
tools: search_media, generate, import, media, notes, see, frame, contact_sheet
---
**Search before batching.** For real-world subjects (animals, places, objects, textures, b-roll) run `search_media` first and fetch what fits; put only what could not be found, or what must match a set, in the batch. Say which were found and which were made.

**Order**
1. Plan every picture the film needs before generating any: name, prompt, aspect ratio, whether it needs `transparent: true` (a character or prop that will sit over a place). Write the list in `notes`.
2. Send them as one batch: `generate {items: [{name, prompt, aspect, transparent, image}]}`. They are made at the same time by image workers (4 at once; `workers` up to 8), so twenty pictures take about as long as five. Never one by one.
3. The call returns a `job` at once: keep working (story, sound, timing) while they are made. Each finished picture lands in the media bin and is announced in the reply of your next tool calls (`pictures_ready`, with its media id). `generate {job: N}` shows progress; `wait: true` waits for all.
4. A picture that failed was already retried once with a reworded request; its entry says why. Change the wording (drop what the filter refuses) and send only the failed ones as a new batch. Never start a second batch for pictures the first covers.
5. Keep sets consistent: reference pictures (`image`) and a saved look (`save_look` / `look`) so a character or place stays the same across items.
6. Look at the finished set together (`contact_sheet` or `frame` of the media) before building on it.

**Checks**
- The set matches in style, colour and lighting; characters are on a transparent background.
- Every item the plan listed arrived; nothing extra was made.
- The aspect matches where it will be used (16:9 backdrops, square or tall for props).

**Mistakes**
- Making pictures one at a time and waiting for each.
- A new prompt per shot for the same character: use its saved look.
- Waiting idle on the job: work on the edit while it runs.
- Generating only what was asked for; extra pictures cost time.

## Reference: Generating pictures in one batch

- Pictures go in ONE batch: plan every picture the film needs first (with
  its aspect and `transparent`), then request them together, never one by
  one: `generate {"items": [{"name": "Harbour", "prompt": "...", "aspect":
  "16:9"}, {"name": "Captain", "prompt": "...", "transparent": true}]}`.
  They are made at the same time by Codex image workers (4 at once;
  `workers` up to 8), so twenty pictures take about as long as five. The
  call returns a `job` at once: keep working on the edit while they are
  made. Each finished picture lands in the media bin and is announced in
  the reply of your next tool calls (`pictures_ready`, with its media id);
  `generate {"job": N}` shows progress and every picture's state, and
  `wait: true` waits for all. A picture that fails was already retried
  once with a reworded request; if it still fails its entry says why, so
  change the wording (or drop what the filter refuses) and send only the
  failed ones as a new batch. Never start a second batch for pictures the
  first already covers.
