---
name: capture-live
use: Use when recording or going live: camera, mic, screen, voice-over, teleprompter, mixer.
topics: capture, record, recording, live, stream, streaming, teleprompter, prompter, ingest, voiceover, voice-over, mixer, switcher, replay
tools: capture, voices, listen, narrate, import, media, notes, check
---
**Order**
1. `capture {action: "devices"}` lists what is connected and the permission state; it never turns anything on. Open a camera, microphone, screen or window only when the person's own words asked for it (`user_approved: true`), never "to check". If system access is missing, tell them what is missing. To rehearse without hardware use the `test:` sources (`test:bars`, `test:counter`, `test:tone:440`).
2. Recording: `capture {action: "record", inputs: [{label, video, audio}]}`. Voice-over: `{action: "voiceover", mic, at}` records against the timeline. Teleprompter: `{action: "prompter", do: "load", scene}` from the story's script.
3. Live: `{action: "live", do: "open", ...}` for the vision mixer (sources, switch, graphics, outputs); rehearse with a test output before going live.
4. Card ingest and backups: `{action: "cards"}` lists mounted cards; copy and verify before formatting anything.
5. Separately recorded sound: `{action: "sync", video, audio}` aligns it to the picture.
6. After recording: `import`, `listen` for faults (clipping, hum), then edit (skill footage-edit).

**Checks**
- Levels peak under -6 dB while recording; headphones monitoring if available.
- Free disk space for the planned length.
- The file exists and plays before the take is declared good.

**Mistakes**
- Starting a live output without a rehearsal.
- Formatting a card before the backup is verified.
- Assuming a device is available; list it first.

## Reference: Capture and live production

One tool, `capture`, with an `action`. It records cameras, microphones,
screens and windows, puts a voice-over on the timeline, runs a
teleprompter, switches live sources like a vision mixer (with titles,
transitions, a program recording, ISO recordings of every source, RTMP /
SRT streaming and a replay buffer), copies camera cards safely and lines
separately recorded sound up with picture.

**Real devices and permissions.** A camera, microphone, screen or window
is opened only when the person asked for it: pass `user_approved: true`
only when their own words in this conversation asked you to use that
device or to stream. The system must also have given Studio access:
`capture {action: devices}` lists what is connected and the state of
camera / microphone / screen access (listing never asks anything and
never turns a camera on). If access has not been given, tell the person
what is missing; ask the system's question only when they want to use the
device: `capture {action: permissions, request: "camera", user_approved: true}`
(a dialog appears for them). Never record the screen or a camera "to
check". For trying things, the `test:` sources need nothing and are
exactly like real ones to everything downstream: pictures `test:bars`,
`test:counter` (the frame number is coded along the top; `test:counter:3`
for another colour), `test:ramp`, `test:red` / `green` / `blue` / `black`
/ `white` / `grey`; sound `test:tone:440`, `test:voice` (speech-like),
`test:noise:7`, `test:silence` (`@0.3` after any sets the level).

**Recording.** `{action: "record", inputs: [{label: "cam a", video:
"camera:…", audio: "mic:default"}, {label: "screen", video: "screen:1",
cursor: true}, {label: "lav", audio: "mic:…"}], seconds: 30, name: "take",
to_timeline: true, at: 0}`. Every input is its own file in the project's
`Recordings` folder (a picture source with its sound: `.mp4` H.264 + AAC;
a microphone alone: 24-bit `.wav`), all on one clock, so multicam files
line up by their start. Screens and windows get a highlight ring that
follows the pointer and a ripple on every click; the pointer's path is
kept beside the video (`….cursor.json`) for zooming to the action. With
`seconds`, test sources run on a simulated clock (a minute takes a moment)
and real devices in real time; without `seconds` the recording runs until
`{action: "stop", session}` (`status` shows levels with peak hold and a
clip flag). `to_timeline` puts each file on a lane of its own name at `at`.
Files are playable only once finished: always `stop`.

**Voice-over.** `{action: "voiceover", mic: "mic:default", at: 12.5,
seconds: 20, count_in: 3, bpm: 100, gain: 1, latency_ms: 0}` counts in,
records, trims the silence at both ends, writes `Voice-over/vo-001.wav`
and places the take on the `voice-over` lane starting where the first
word was spoken, ready to cut like any clip. The reply tells you the peak
level and whether it clipped (record again if it did, or under -30 dB).
`click: true` plays the count-in on the speakers. `latency_ms` moves the
take earlier by the microphone's delay. Without `seconds` it records until
`stop` (the person speaks while the picture plays in the app). Dialogue
replacement against picture is the ADR desk in the sound tools, not this.

**Teleprompter.** `{action: "prompter", do: "load", scene: "all"}` loads
the dialogue and narration of the story's scenes (speaker names shown as
headings, directions dimmed, neither waited for) or `text: "…"` (a line in
brackets is a direction). `wpm` sets the pace (default 150), `mode`:
`pace` (scrolls at speaking pace; a breath between paragraphs), `follow`
(follows the speaker by what live transcription hears) or `manual`.
`do: "play"` / `"pause"` / `"advance"` (seconds) / `"jump"` (a word) /
`"back"` (to the start of the paragraph), `"settings"` (`mirror: true` for
a prompter glass, `size`, `wpm`), `"frame"` (a picture of the prompter to
look at), `"status"`. Voice-follow: `do: "hear", words: […]` feeds what a
transcription heard (tolerant of slips, skipped and extra words; it holds
and reports `lost: true` when the speaker is off script) and `do:
"listen", media: <recording>` (or `mic:`) transcribes a rolling window of
sound as if live (needs the speech model installed) and moves the
prompter by it.

**Live production (the vision mixer).** `{action: "live", do: "open",
sources: [{name: "wide", video: "camera:…", audio: "mic:…"}, {name:
"close", video: …}], width: 1280, height: 720, fps: 30, replay_secs: 30,
record_program: true, iso: true}` returns a `session`. Test-only
productions run on a simulated clock: `do: "run", seconds: 2` makes time
pass (titles and transitions advance with it); with real devices or a
`stream` they run on the wall clock by themselves. Then, each with
`session`:

- `cut` / `take` with `to`: a source (name or number), `"black"`, a saved
  scene, `{pip: {main, inset, corner, scale}}`, `{split: [a, b]}` or
  `{grid: [...]}`; `transition`: `cut`, `mix`, `dip`, `wipe[_left|_up|_down]`,
  `push[_right|_up|_down]` over `ms`. `take` without `to` takes the preview
  to program (the old program waits in preview, as on a hardware mixer);
  `preview` sets the preview; `scene` saves a named layout.
- `title` (`kind`: `lower` with `text` and `sub`, `centre`, `bug`; `ms`
  fade) and `title_off`. Sound follows the picture and crossfades with it;
  `mute` / `unmute` / `gain` per source.
- `frame` (the program), `multiview` (preview and program large, every
  source small with red / green tally borders), `state`.
- `stream` with `url`: `rtmp://host/app/key` (YouTube, Twitch and any RTMP
  server; not `rtmps://` yet) or `srt://host:port?streamid=…&latency=120`
  (no passphrase yet). Only with the person's say-so (`user_approved`).
  It reconnects after a break and resumes on a keyframe; a slow network
  drops pictures up to the next keyframe, never stalls the show. Stream
  keys are never shown in answers. `stop_stream` ends them. `status` shows each
  stream (sent, dropped, reconnects) and, for a show on the wall clock,
  `behind_ms`: more than a few hundred means this computer cannot keep up
  (use a smaller picture or a lower frame rate).
- `replay_save` (`seconds`) saves the last stretch of program as an
  `.mp4` without re-encoding; `replay_play` (`seconds`, `speed`, `as`)
  makes it a source you can take to program (slow motion at 0.5).
- `close` finishes everything: the program recording, every ISO file and
  the streams; `import: true` / `to_timeline: true` brings the files
  into the project (the program on a lane `program`, each ISO on `iso …`),
  where a live cut can be re-cut from the ISOs afterwards.

**Cards and backups.** `{action: "cards"}` lists mounted camera cards
(DCIM, XDROOT, PRIVATE, BPAV…). `{action: "ingest", from: "/Volumes/A001",
to: "/Volumes/Work/Footage", backup: "/Volumes/Backup/Footage", import:
true}` reads each file off the card once, writes it to every destination,
then reads every copy back and compares its SHA-256 to what came off the
card; a manifest (`ingest-manifest.json`, `sha256sums.txt`) sits beside
each copy. A copy that differs is reported `MISMATCH` (and never silently
replaces an existing different file: the new copy keeps a `-2` name). A
backup on the card's own drive is refused. Nothing on the card is changed.
Later, `{action: "verify", dir}` re-checks a folder against its manifest.
Do not tell the person a card is safe to wipe unless `ok` is true and the
backup copy exists. (Cameras connected by cable as a camera rather than as
a disk are not read yet; mount them as storage.)

**Syncing separately recorded sound.** `{action: "sync", video: <camera
clip id>, audio: <recorder file's media>, apply: true}` compares the
camera's own sound with the recorder's by cross-correlation (a clap or any
shared room sound), reports `offset_s`, `confidence` (under 8: not found,
nothing is placed) and clock drift in ppm, and with `apply` places the
good sound exactly against the picture (on lane `synced audio`, as long as
the clip), mutes the camera's own sound and links the two.

## Reference: tool `capture`

Capture and live production. action: devices (cameras, microphones, screens, windows, test sources, and whether the system has given access) | permissions (status; request: camera|microphone|screen asks the person, only when they asked) | record (several inputs at once, each to its own file: a camera or screen with its sound is an .mp4, a microphone a .wav; screens carry a cursor highlight and keep the pointer's path; seconds runs it, without seconds it keeps recording until stop; to_timeline places the files) | stop | status (levels, progress) | voiceover (record the microphone onto the timeline at `at` with a count-in, level, clip warning; the take lands on the "voice-over" lane) | prompter (do: load a scene or text, play, advance, hear, jump, settings (mirror, size, wpm), listen (voice-follow from a recording or the microphone), frame (a picture of the prompter), status) | live (a vision mixer: do: open with sources, take / cut / preview with a transition, scene, title, mute, run, frame, multiview, stream to rtmp:// or srt://, replay_save, replay_play, close; program and every source (ISO) can be recorded) | ingest (copy a card or folder to the working drive and to a backup drive, each file checked by SHA-256 against the card; import: true adds it to the project) | cards | verify (re-check an ingested folder) | sync (line separately recorded sound up with a camera clip by their common sound; apply: true places it and mutes the camera's own). Real devices need user_approved: true, which only the person's own request gives; test: sources need nothing and are for trying things (guide topic "capture").
