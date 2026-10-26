# The AI editor as a production crew

Studio's AI editor (the AI panel) can work alone or as a crew. Either way it
uses the same tools on the open project, and one Undo takes back a request.

## Crew or solo

The chip next to the provider in the panel's header switches between
**Crew** and **Solo** (`Prefs::ai_crew`, off by default, so the editor starts solo).

- **Solo**: one agent does the whole request.
- **Crew**: a **director** reads the request, the project, the story
  structure and your taste, then posts a plan. Each step goes to one
  specialist:

  | role | owns | its `edit` ops (besides `set`, `keys`, `move`, `trim`, `delete`…) |
  |---|---|---|
  | Editor | structure, rhythm, titles | montage, place, split, transition, text, card, scene, narrate, music… |
  | Animator | everything that moves, characters, cameras, 3D | animate, puppet, motion, stagger, physics, three_d, camera, model… |
  | Sound designer | music, effects, ambience, mix | music, sfx, narrate, audio… |
  | Colourist | exposure, balance, matching, the look | effect, apply, copy_look, match_colour, fx_layer |
  | VFX artist | keys, roto, tracking, compositing | cut_out, roto, track, stabilize, mask, matte, corner_pin… |

  Every specialist is its own agent (Codex, Claude Code or the APIs, as
  chosen in Settings ▸ AI) with its own brief and its chapters of the craft
  book in its prompt. Steps that wait for nothing run side by side (up to
  three at once). The board keeps them apart: a clip one specialist holds
  (given in the plan, or touched by it) cannot be edited by another until
  its step ends, and an op another role owns is refused with who owns it
  (ops no role lists, such as ones added later, are open to everyone).
  Undo is refused to specialists (it would take back the others' work).
  When every step is done the director watches the whole film and either
  approves it or sends steps back with notes (two review rounds at most).

Code: `app/src/agent/crew.rs` (roles, `Board`, `CrewBrain`),
`brain.rs` (`Gate::call_as`), `attach.rs` (`studio mcp --attach SOCKET
--role ROLE`).

## Following and steering a run

- **The plan card** shows each step, who does it, its status (waiting,
  working, done, at a checkpoint, sent back, failed), the last word from
  it, its time, and for the plan the time left and tokens spent against
  the estimate. A solo agent posts its plan with the same `plan` tool, so
  solo runs read as a plan too.
- **Checkpoints** (◆): a step the director (or a solo agent) marks as a
  checkpoint waits for your approval before the steps that build on it
  start. Approve, or skip to send it back; words typed at a checkpoint go
  back with it. `Prefs::ai_checkpoints` turns the waiting off.
- **Pause / Resume** next to Stop: every agent holds at its next tool call.
- **Correct now** (⌘↵ while it works): your words reach every agent at its
  next tool call ("NOT RUN: the person interrupted with a correction…"),
  and steps that have not started get them in their brief. ↵ still queues
  a next request ("sends when this one finishes").

## Memory and taste

Each request starts with your taste in brackets, and the agents read it
before working:

- `<project>/AI/taste.json`: this project's likes, dislikes, rules, and
  what you corrected or rejected here.
- `<project>/AI/Style guide.md`: the project's style guide.
- `Studio/AI/taste.json`: notes for every project, plus recent corrections
  and rejections from all projects.

Corrections you send mid-run, steps you send back and changes you skip are
remembered automatically. Agents save lasting preferences with the `taste`
tool (`remember`, `kind`, `everywhere`), write the style guide with
`style_guide`, and `forget` a note by id.

Code: `app/src/agent/taste.rs`.

## Honesty and speed

- `capabilities` answers "can Studio do this, and with what?" from every
  tool, every `edit` op, the guide and the craft book, best first. Every
  brief says to call it before saying something cannot be done or reaching
  for a workaround.
- If an agent's answer says something is impossible and it never called
  `capabilities`, it is sent back once to check and do it properly
  (`crew::honest_turn`).
- A guide topic read twice in one request is answered from memory, and
  specialists get their chapters up front, so they rarely call `guide`.
- Codex runs Studio's tools with parallel calls allowed and a 15 minute
  tool timeout; the crew runs independent steps in parallel.
- Estimates: per-role averages of minutes and tokens (learned from finished
  plans in `Studio/AI/crew-stats.json`), along the plan's longest chain of
  waiting steps.

## Evaluation

`studio eval` runs real jobs headless, exactly as the panel does (the same
gate, crew, plan, corrections and taste), on projects it builds itself, and
scores the result:

```text
studio eval --out DIR [--provider codex|claude] [--model M] [--solo]
            [--tasks story_scene,music_video,fix_colour,honest_animation,steer_title]
            [--minutes 30]
```

| task | the job | scored on |
|---|---|---|
| story_scene | stage a fox and a rabbit meeting at dusk | length, characters animated, a place, sound, honesty, looked last |
| music_video | cut 5 clips to a song on the beat | length, number of cuts, every clip used, cuts within 90 ms of a beat, music to the end |
| fix_colour | a dark shot with a blue cast | the grey card neutral, exposure |
| honest_animation | make the cat wave and bounce | the cat animated, no "impossible" workaround |
| steer_title | a NIGHT RIDE title, corrected mid-run to DAWN RIDE | the correction reached it, new title, old title gone |

Results go to `DIR/results.json`; each run adds a line to
`DIR/history.jsonl`, and the summary shows the change since the last run.
Run it with `HOME` set to a scratch folder (Studio's folder then lives
there), `CODEX_HOME` pointing at your Codex sign-in, `STUDIO_CONTENT` at the
content library, and the current `codex` linked into the scratch home's
`.local/bin` (otherwise Studio may find an older Codex elsewhere, such as
Homebrew's, which refuses newer models):

```bash
mkdir -p /tmp/eval-home/.local/bin && ln -sf ~/.local/bin/codex /tmp/eval-home/.local/bin/codex
```

```bash
HOME=/tmp/eval-home CODEX_HOME=~/.codex STUDIO_CONTENT=~/content-build studio eval --out /tmp/evals
```

Unit tests run the whole crew offline with scripted agents
(`agent::eval::tests`), the board (`agent::crew::tests`), taste, and
capability search.
