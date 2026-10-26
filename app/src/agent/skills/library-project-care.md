---
name: library-project-care
use: Use when organising assets or protecting work: find, relink, licences, backups.
topics: library, assets, safety, backup, backups, rights, licences, licenses, health, recovery, archive
tools: assets, project_care, media, search, import, save, open, new_project, undo, redo, check
---
**Order**
1. Find by what is in it: `assets {action: "find", query}` searches names, labels, notes, spoken words, on-screen lettering and what pictures show (`index` looks into new media; `similar` is "more like this"). Organise with `rate`, `tag`, `bin_create`.
2. Missing media: `assets {action: "missing"}` lists what cannot be found; `relink` looks by content under another name or place. Do not recreate a file that can be found.
3. Rights: every asset can carry a licence (`assets {action: "licence", asset, licence, author, credit}`). `rights_check` compares the timeline with the licences and returns blocked and warning items and the credit lines for the end card or description; `export` runs it too.
4. Safety: `project_care` has `health` (missing files, disk, backups, rights), `snapshot` and `restore`, `backup` and `restore_backup`, `collect` (a standalone copy), `clean` (says what would go first) and `recover`. Run `health` before long sessions and before delivering; `snapshot` before a risky change.
5. Never delete or overwrite the person's original media; work on copies. `save`, `new_project`, `open` for the project file; `undo` / `redo` for edits (`edit` is one undo step).

**Checks**
- Nothing offline in the project before export.
- Licences that require credit are credited.
- A snapshot exists before a risky operation.

**Mistakes**
- Replacing missing media with something similar instead of relinking.
- Cleaning or deleting what holds the person's ratings, tags and licences: the library database is never cleaned away.
- Ignoring a licence expiry.

## Reference: Media library and project safety

Two tools: `assets` (every picture, clip, sound and generated file Studio has seen, across all projects) and `project_care` (history, backups, health). Both work for the AI and for the person from the same data.

**The library.** One database in Studio's data folder (`Library/assets.db`; it holds the person's ratings, tags and licences, so it is never cleaned away). Anything in the open project is in it already when you call `assets`. `assets {"action":"add","paths":[folder or files]}` brings in more. A file is known by its content as well as its path, so a moved or renamed file is found again.

- Look: `assets` (overview), `info`, `find`, `similar`, `bins`, `bin`, `duplicates`, `missing`.
- Organise: `rate` (stars 0-5), `favourite`, `tag` / `untag` (kind: tag, keyword, person, place), `note`, `bin_create` (name `"Footage/Day 1"`), `bin_add`, `bin_remove`, `bin_delete`. A **smart bin** is a saved search: `bin_create` with `rule` (any `find` fields, e.g. `{"min_rating":4,"kind":"video","query":"beach"}`) and it keeps itself filled. Deleting a bin never touches the assets.
- Name things by id, file name or path (`asset`, `assets`); `media` takes the project's own media ids.
- Metadata read from the file: camera, lens, shutter, aperture, ISO, place (Exif; QuickTime make/model/location), when it was shot, start timecode. `find` filters: `camera`, `lens`, `location`, `source` (imported, generated, downloaded, bundled), `licence`, `min_seconds`, `max_seconds`.

**Finding by what is in it.** `find {"query":"..."}` searches names, labels, notes, spoken words, on-screen lettering and what pictures show, and tries words of near meaning ("night" also finds "dark", "crowd" finds "group", "car" finds "vehicle"). Each result says where it matched (`matched`: field, and seconds for speech). Footage only has insides after it is looked into: `assets {"action":"index"}` describes a few frames of each picture and clip (colour, light, people, close-up or wide, lettering), groups faces, gives every picture and sound a signature for `similar` ("more like this"), and with `speech: true` writes out what is said (needs the speech model). The app does this for new media in the background. `index` with `all: true` does everything not yet looked into. Be straight about the limits: it does not name objects or actions, it does not read the words of titles (it only notices lettering), and faces are grouped by how they look, not identified. `people` lists the groups; `name_person {"person":3,"name":"Meera"}` names one and every asset with that face gets the person label, so `find {"person":"Meera"}` works. A good search order: `find` with words, then narrow with filters, then `similar` on the best hit.

**Relinking and missing media.** `missing` lists files the project or library cannot find; `relink` looks for them by content (same file under another name or place) in the usual places and connected drives, or in `search` folders, and repoints the project's media (one undo step) and the library. The project keeps working with clips untouched.

**Rights and licences.** Every asset can carry a licence: `licence {"asset":..., "licence":"cc-by","author":"...","credit":"...","url":"...","expires":"2027-03-31","source":"...","cost":"...","terms":"..."}`. Kinds: owned, licensed, royalty-free, permission, public-domain, cc0, open, cc-by, cc-by-sa, cc-by-nc, cc-by-nd, editorial, rights-managed, generated, unknown (`licences` lists them with what each allows). Studio's own bundled content brings its licence and credit line along. `rights_check` compares the timeline with the licences and returns `blocked` (expired, non-commercial or editorial licence on a commercial film, missing file), `warning` (no licence recorded, expires within 30 days, no-derivatives, share-alike, credit missing) and the **credit lines** to put in the end credits or description. **`export` runs this check itself**: a blocked asset stops the export with the reason and fix; pass `commercial: false` if the film is not commercial, or `ignore_rights: true` only when the person says to; warnings and credits come back in the export reply as `rights`. Record where footage came from when you bring it in (`download`, `generate` and `import` do not know its licence). `expiring {"days":60}` lists licences ending soon.

**Keeping the project safe.** `project_care`:
- `health`: missing files, files that live outside the project or on another drive (offline if unplugged), sizes, disk space, whether autosave and backups exist, rights problems. Run it before long sessions and before delivering.
- `history`: autosaves (every 30 s after a change, newest 20 kept), kept snapshots, restore points. `snapshot {"label":"before recut"}` keeps a moment autosave will never remove. `restore {"which":0}` (a number, `"latest"`, or part of a name) puts a snapshot back; the project as it was is kept first as a restore point (`restore {"which":"before-restore"}` undoes a restore).
- `backup {"to":"/Volumes/Drive/Backups","keep":5}`: project file, notes and history as a dated snapshot, and each media file stored once by content, so repeats copy only what changed; verified by size; the newest `keep` snapshots stay and media nothing needs is dropped. `backups {"to":...}` lists them; `restore_backup {"from":...,"into":newfolder}` rebuilds the project with its media.
- `collect {"to":newfolder}`: a standalone copy of the project with every file it uses (media, fonts, LUTs, models), the project pointing at its own copies; the original is untouched; `only_used` leaves out unused media. `archive` adds a SHA-256 for every file; `verify {"folder":...}` checks it later; `relocate {"folder":...}` repoints a moved archive.
- `clean`: says what would go (`apply: true` does it). It removes only the project's regenerable caches, autosaves beyond the newest N, and shared cache files untouched for a month, and never media, never a file any project names (proxies, waveforms, thumbnails the project points at stay), never kept snapshots.
- `recover`: windows that ended without a clean quit and autosaves newer than the saved file. The app itself reopens a crashed project at the playhead and selection it had, on the next start.

Never delete or overwrite the person's original media. Work on copies, and run `health` and `rights_check` before delivering.

## Reference: tool `assets`

The media library across every project: files, bins, ratings, labels, rights, and search by what is in them. `action`: overview (default), add (paths: files or folders into the library), find (query: words for names, labels, notes, spoken words, on-screen lettering and what pictures show, matched by meaning too; plus filters kind, min_rating, favourite, tag, keyword, person, place, bin, project, camera, lens, source, licence, min_seconds, max_seconds, missing, sort), similar (asset: more like this picture, clip or sound), info (asset), rate (asset(s), stars 0-5), favourite (asset(s), on), tag / untag (asset(s), values; kind: tag, keyword, person or place), note (asset, text), bins, bin (name: its contents), bin_create (name like "Footage/Day 1"; rule: a find as a smart bin that keeps itself filled), bin_add / bin_remove (name, assets), bin_delete, duplicates (near: also look-alikes), missing, relink (search: folders to look in; default this Mac's usual places and connected drives; fixes the project's files too), index (look into assets so find can see inside them: pictures, faces, sound, with speech true to write out what is said; limit), people / name_person (person: group number, name), licence (asset(s), licence, author, source, credit, url, expires YYYY-MM-DD, cost, terms), rights_check (what the timeline uses against its licences; commercial default true), licences (the kinds), expiring (days), forget (asset(s): leaves the library, not the disk). Assets are named by id, file name or path; `media` takes the project's media ids.

## Reference: tool `project_care`

Keeps the project safe. `action`: health (default: missing files, offline drives, sizes, disk space, backups), history (autosaves and kept snapshots, newest first), snapshot (label: keep this moment), restore (which: number from history, "latest", or part of a name; the project as it is now is kept first, so a restore can be undone), backup (to: a folder or drive; media is stored once by content so repeats copy only what changed; keep: snapshots to keep, default 5; media false for the project file only), backups (to: what is there), restore_backup (from: the backup folder, into: a new folder, snapshot), collect (to: a new folder; copies the project with every file it uses; only_used, with_cache), archive (as collect, plus a checksum for every file), verify (folder: an archive), relocate (folder: an archive that was moved), clean (what a cleanup would remove; apply true to do it; it removes only project caches nothing refers to, autosaves beyond keep_autosaves, and shared cache files untouched for shared_cache_days; never media, never anything a project names), recover (sessions that ended without a clean quit, and autosaves newer than the saved file).
