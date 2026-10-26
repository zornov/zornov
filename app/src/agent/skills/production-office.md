---
name: production-office
use: Use when a breakdown, schedule, budget or task list is wanted.
topics: production, producer, breakdown, schedule, scheduling, stripboard, callsheet, call-sheet, budget, tasks, shotgrid, status
tools: production, story, plan, notes, summary
---
**Order**
1. A production office starts from a story: if the project has none, import the script first (`story {do: "import"}`, skill story-film). The breakdown reads scenes, so scenes and cast come first.
2. `production {do: "show"}` to see where things stand; `production {do: "settings"}` for dates, days off, shoot hours, general call and whether the film is animated.
3. `production {do: "breakdown"}` reads every scene for cast, locations, props, wardrobe, vehicles and animals; check it against the script before using it.
4. Schedule, then budget, then tasks, in that order: each uses the one before. Read each reply, fix what it flags (`max_locations_per_day`, conflicts, missing rates).
5. Keep the numbers the person gave: never invent rates, dates or counts. If an input is missing, say what is missing instead of guessing.

**Checks**
- The schedule's days add up to the film's scenes; no actor is booked beyond the limits in settings.
- The budget total equals the sum of its lines; report both.
- Tasks have an owner or are marked unowned.

**Mistakes**
- Scheduling before the breakdown is right: a missed prop or location moves the whole schedule.
- Presenting placeholder rates as real figures. Label every figure as an estimate from the person's settings.
- Rewriting the script to fit the schedule. The script is the person's; raise the conflict instead.

## Reference: Production office

The `production` tool is the producer's desk, kept beside the story in
the project (`production.json`; the script itself is never copied: scenes
and shots are the story's ids). It breaks the script down, schedules it,
budgets it and tracks the work. Every `do` returns plain text sheets you
can read and a short summary; `export` writes the same as PDF. Days count
from 1. Plan first (`story`), then:

1. **Break the script down.** `production {"do": "breakdown"}` reads each
   scene (action lines, shot actions and subjects) and tags what it needs:
   cast (numbered by first appearance, from the bible and the dialogue),
   the location, extras, props, wardrobe, make-up, vehicles, animals,
   stunts, practical effects (`sfx`), VFX, sound cues and music. It reads
   by keyword, so it misses oddly named things and sometimes over-reads:
   look at the sheets, then `tag` (`scene`, `category`, `name`, `qty`,
   `note`) what is missing and `untag` what is wrong. Characters of the
   story are never tagged as animals or props. Hand-made tags survive a
   second reading; `elements` lists everything with the scenes it is in.
   Scene length comes from its shots (or its lines) and sets the page
   count in eighths.
2. **Set the calendar.** `settings`: `start_date`, `days_off` (default sat
   and sun), `blackout` dates, `shoot_hours` (10) and
   `hours_per_screen_minute` (2.5: so a day shoots about 4 minutes of
   film), `general_call`, `max_locations_per_day`, `director`, `producer`,
   `hospital`, `office` (for the call sheet), `animated`.
3. **Schedule.** `schedule` (`auto` on first use) orders the scenes like a
   first AD: scenes at a location together, a location never split over
   days just to squeeze one more in, day and night apart, a day filled to
   what the crew can shoot, no actor on a date they cannot work. Read the
   stripboard it returns and its `warnings` (unscheduled scenes, too many
   locations, a day over capacity, mixed day and night, an unavailable
   actor in a scene). Adjust by hand: `{"move": "7", "to_day": 2,
   "index": 1}`, `{"set_day": 2, "call": "05:30", "notes": "weather cover:
   scene 9"}`. `availability {"cast": "Asha", "dates": ["2026-11-03"],
   "reschedule": true}` records dates an actor is away. `dood` is the
   day-out-of-days (SW start work, W, WF finish, SWF only day, H hold:
   paid between work days), the basis of cast costs.
4. **Call sheets.** `call_sheet {"day": 2, "pdf": true}` (or `path`):
   general call, estimated wrap, locations, scenes with start times, each
   actor's make-up and set call, what every department brings, notes,
   tomorrow. `export {"what": "all_call_sheets"}` writes every day. The
   PDFs use built-in fonts: Latin text only, so other scripts print as `?`.
5. **Animated or generated work: the pipeline.** `pipeline {"preset":
   "animation", "tasks": true}` (stages layout, animation, lighting,
   comp, sound; or preset `ai`: boards, generate, review, edit, sound).
   Each shot clears each stage in order, harder shots (many subjects,
   camera moves, effects in the scene) take longer, stages have a number
   of artists (`"artists": {"animation": 4}`), and it reports each shot's
   spans, the finish date and the bottleneck stage. `tasks: true` makes a
   task per shot and stage.
6. **Budget.** `budget {"template": "live_action_short" | "live_action_feature"
   | "animated_short" | "ai_film"}` builds accounts sized from the
   breakdown and schedule: cast by the days each actor works, a line per
   location, prop, costume, vehicle and VFX element, animation by shot
   second. The rates are indicative placeholders: replace them with real figures (`line` with `id`, `rate`, `qty`) and `scale` the whole template.
   `currency` sets the budget currency (any code; `rates {"INR": 0.012}`
   says what one unit is worth in it; line items can be in another
   currency; rupees group in lakhs). `fringe_pct` loads labour lines,
   `contingency_pct` is held back. Views: `top` (top sheet: budget,
   actual, estimate at completion, variance), `accounts` (every line with
   its id), `scenes` (cost per scene and shot, per second of film: a line
   that names a `shot`, `scene` or `element` goes there, the rest is
   shared by length). Record money with `actual {"line": id, "amount": …,
   "close": true}`; a closed line's actual is its final cost.
7. **Generation runs log themselves.** Every `generate` run is recorded
   with the project. Pass `shot` (or `scene`) to `generate` so a
   `generate` task and version of it are logged for that shot. Never
   mention money for these runs in your messages or notes.
8. **Tasks and status** (like ShotGrid or ftrack). A task is a step of a
   shot, scene, asset or the whole film, with a status, assignee, due
   date, notes and versions. Statuses: waiting, ready, in_progress,
   review, revisions, approved, final, blocked, omitted. `task {"entity":
   "shot", "target": "1A", "step": "animation"}` adds one;
   `asset_tasks` adds a design task for every character, prop, vehicle
   and place; `tasks` lists (filter by `status`, `step`, `assignee`,
   `shot`). `status {"task": "1A animation", "status": "in_progress"}`
   changes one (`task` is an id or `"1A animation"`; `note` adds a note);
   approving a stage wakes the stages that follow it. `version {"task":
   "1A animation", "note": "retimed"}` stamps a new version and links
   the clips the shot has on the timeline right now (bind or build the
   shot with `story` first), so a version always says which clips showed
   it. `dashboard` is what is done: percent overall, per step, per shot
   with the step each is at, who has what, what is overdue, blocked or
   waiting for review, and the recent activity.
9. **Work from the board, and keep it true.** When you take on a piece of
   work, set its task `in_progress`; when it is ready to look at, set it
   `review` with a note saying what to look at; when you cannot go on,
   `blocked` with a note. Report progress by reading `dashboard`, not
   from memory: say what is done, what is in review, what is blocked and
   what is next. Never mark `approved` or `final` for work you did
   yourself and the person has not looked at.

## Reference: tool `production`

The production office, kept with the project beside the story: breakdown, schedule, budget and task tracking. do: show (where things stand), settings (start_date, days_off, blackout, shoot_hours, hours_per_screen_minute, general_call, max_locations_per_day, director, producer, hospital, office, animated), breakdown (reads every scene for cast, locations, props, wardrobe, make-up, vehicles, animals, stunts, sfx, vfx, sound, music, extras; scene: one sheet; retag), elements, tag / untag (scene, category, name, qty, note: correct the keyword reading), schedule (auto: orders scenes into shooting days by location, day or night, what a day can shoot and actors' availability; move + to_day + index; set_day + call + notes; start_date), availability (cast, dates they cannot work, reschedule), dood (day out of days: SW W WF SWF H), call_sheet (day, pdf / path), pipeline (animation or generated work: preset animation or ai, artists: {stage: n}, tasks: true makes a task per shot and stage; when each shot clears layout, animation, lighting, comp, sound and the bottleneck), budget (template live_action_feature, live_action_short, animated_short or ai_film sized from the breakdown and schedule, scale, currency, rates {INR: 0.012}, fringe_pct, contingency_pct, view top / accounts / scenes: costs per scene and shot), account, line (add: account, desc, qty, unit, rate, currency, labour, scene, shot, element; or id to change; remove), actual (line, amount, currency, note, close),  tasks (filters: status, step, assignee, shot, entity), task (add: entity shot / scene / asset / project, target, step, assignee, due), status (task: an id or "1A layout", status waiting / ready / in_progress / review / revisions / approved / final / blocked / omitted, note, assignee, due: finishing a stage wakes the next), version (task, note, media: links the shot's clips on the timeline), asset_tasks, dashboard (what is done), export (what: breakdown, schedule, call_sheet, all_call_sheets, budget, pipeline, progress as PDF; path). Days count from 1. Mark your own work as you do it: set a task in_progress when you start it, review when it is ready to look at, and report progress from dashboard.
