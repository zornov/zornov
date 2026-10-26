---
name: project-team
use: Use when others are involved: comments, versions, sharing, roles, the person's taste.
topics: team, comments, versions, share, collab, roles, locks, layout, workspaces, panels, taste, project
tools: comments, versions, share, team, layout, taste, undo, redo, open, save, new_project, notes, made
---
**Order**
1. Review notes: `comments {action: "list"}` gives open notes with the clips under them. Fix each with `edit`, look, then `resolve` it with a note saying what changed.
2. Versions: `versions` (list, save, compare, restore, branch, switch): `save` a named version before a large change; name versions for what they are.
3. Sharing and roles: `share` works through files the person chooses (package, import_notes, merge, link, sync, status); `team` shows roles and locks. A locked lane or clip belongs to someone else: do not override it.
4. The person's taste: follow what comes with each request; save a lasting preference with `taste {remember, kind}` (`everywhere` for taste in general). A correction mid-run is a rule, so also record a lesson (`skill {do: "learn"}`).
5. Window and housekeeping: `layout` for workspaces and panels; `made` lists saved moves and designs; `undo` / `redo`, `save`.

**Checks**
- Every comment resolved has a note saying what changed.
- A version exists before a big restructure.
- You never change something another member holds a lock on.

**Mistakes**
- Resolving a comment without changing anything.
- Overwriting someone's version instead of making a new one.
- Saving a one-off wish as a lasting taste.

## Reference: Working with a team

A project can have review notes, named versions, other people and
copies elsewhere. All of it lives in files in the project folder, so the
project must be saved (`--project` or `save`). Four tools:

**`comments`: review notes.** A note is pinned to a time (`at`, seconds;
`until` for a stretch), and maybe to a `clip` (the note then follows the
clip when it moves) and a `region` of the frame (`[x0, y0, x1, y1]`,
canvas pixels from the centre, y down: the units of `pick` and `place`).
Notes have threads (`reply`) and are `open` or `resolved`. Ids read like
`sam-3`. `list` gives the open notes in time order, each with `under`: the
clips at that moment (or its own clip) with their lanes and times, so you
can act without another call. Notes you write are signed with the
person's name plus "(AI)".

To work through notes ("fix every open note"):

1. `comments` `{}`: the open notes with what is under each.
2. For each: look (`frame` at its time, `listen` for sound notes), make the
   change with `edit`, look again.
3. `comments` `{"action": "resolve", "id": "sam-3", "note": "lifted the
   exposure 0.4 stops on clip 12"}`: say what changed, in a few words.
   A note you cannot or should not do ("make it feel more like the
   trailer" with no trailer): `reply` with a question instead of resolving.
4. `comments` `{}` again: nothing open, or only what you replied to.

`export` `{"format": "csv" | "markdown" | "edl" | "json", "path"}` writes
the list (EDL markers open in other editing suites). `import` `{"path"}`
reads notes back: Studio's own, a CSV with time and note columns, an EDL
with `LOC` markers, or plain lines `00:01:23:12 the music is late`; notes
already there are skipped.

**`versions`.** `save` `{"name", "note"}` before a big change or a hand-off;
`list`; `compare` `{"from": "v2", "to": "now"}` names every clip added,
removed, moved, trimmed and which properties changed (old → new);
`restore` `{"version"}` goes back as one undo step; `branch` `{"version"
or "now", "name"}` puts that cut beside the current one as another
sequence and makes it active (an alternate cut: "the 30-second version");
`switch` `{"name"}` moves between cuts.

**`team`: roles and locks.** `show` says who you are working for and what
they may do. Owners manage the team (`set` `{"name", "role"}`, `remove`);
editors edit; reviewers only add notes: every edit of theirs is refused
with the reason, so for a reviewer, write notes instead of editing. An
editor `lock`s a lane or a clip they are working on (`{"lane": id}` or
`{"clip": id}`); nobody else's edit may change it, including a ripple that
would shift it. An edit refused with "locked by Kim" means: leave that
part, or add a note for Kim.

**`share`.** `package` writes a review package (a 540p proxy render, the
notes as CSV and Markdown, the version, a notes template) into the
project's Review folder for the person to send; `import_notes` `{"path"}`
reads what came back. `merge` `{"path"}` takes in another copy of this
project (handed over on a drive, say): a three-way merge from the newest
version both copies have (or `base`), one undo step; what both changed
differently keeps yours and is listed under `conflicts`. `link`
`{"folder"}` connects to a shared folder (a network drive or a synced
folder) where several people work on one project; `sync` exchanges
edits and notes with it (theirs merged in, yours published); `status`
says whether others published since. Nothing is sent anywhere except
into that folder, and only when asked.

## Reference: tool `comments`

Review notes from the team, pinned to a time (and maybe a clip, which the note follows, or a region of the frame): threads with replies, resolved or open. list (default: open notes, each with the clips under it), add {note, at | clip, region, until}, reply {id, note}, resolve {id or [ids] or "all", note: what you did}, reopen, edit, delete, export {format: csv|markdown|edl|json, path}, import {path: a notes file or a review package that came back}. To work through notes: list, fix each with edit, look, then resolve it with a note saying what changed.

## Reference: tool `versions`

Named versions of the project: list, save {name, note}, compare {from, to: version id/name or "now"} (clips added, removed, moved, trimmed and which properties changed, old → new), restore {version} (one undo step), branch {version or now, name} (that cut joins the project as another sequence, and becomes active), switch {name} between cuts.

## Reference: tool `share`

Working with others, all through files the person chooses: package {path} writes a review package (a 540p proxy render, the notes, the version) to send; import_notes {path} reads the notes that came back; merge {path, base} takes in another copy of this project (a three-way merge, one undo step; conflicts keep yours and are listed); link {folder} connects to a shared folder (network drive, synced folder) where several people sync; sync exchanges changes and notes with it; status.
