---
name: export-delivery
use: Use when the film is final: platform presets, QC, background renders, masters, cutdowns.
topics: delivery, deliver, finishing, qc, queue, mastering, dcp, imf, nodes, bundle, cutdown, cutdowns, displays, display, aspect, formats-delivery, export, formats
tools: export, export_status, queue, qc, deliver, bundle, cutdown, check, review, listen, loudness, scopes, new_project
---
**Order**
1. Finish skill review-before-export first; export is never the first look.
2. Choose the target: a platform spec (`deliver` with no arguments lists them: youtube, youtube-shorts, instagram-reel, tiktok, netflix, ...), a master preset (`export` presets such as ProRes), or the quick `export {preset}`. Vertical 9:16 for Reels, TikTok and Shorts, 16:9 for screens, 1:1 or 4:5 for feeds.
3. For a different aspect, build or reframe for it (`new_project` size, `reframe`) before exporting, not after.
4. Check without rendering: `qc {preflight: true}` reads the mix's loudness and captions against speech. Fix what it reports.
5. Render: `export` waits for the file and reports its peak and loudness, and runs `qc` on it. Long or many jobs: `queue add` (background; `export_status` or `queue` shows progress). Another computer's nodes can help.
6. Deliverables: `bundle` builds the folder (master, versions, captions, report); `cutdown` makes 15, 30 and 60 second versions as projects of their own.
7. Read the QC verdict: PASS, PASS with warnings, FAIL. Say what failed and fix it; never say a delivery is ready while QC says FAIL.

**Checks**
- Size, frame rate, loudness and true peak match the chosen spec.
- Captions exist when speech does and the spec asks for them.
- The file plays: open the result and look at the first and last frames.

**Mistakes**
- Exporting before the last `check`.
- Normalising loudness by guess: use the measured value from the export reply.
- Delivering a cutdown that is the master trimmed at the end; recut the story.
- The photosensitive flash test is a screening aid, not a certification; say so.

## Reference: Finishing and delivery

Everything after the cut: checking, rendering in the background, masters
for a platform or a cinema, the files that travel with a film.

**`qc` checks a finished file** (`export` runs it for you; `qc: false`
turns that off). It decodes every picture and all the sound and reports
`PASS`, `PASS with warnings` or `FAIL`, each finding with a time, a
severity and a fix, and writes `<file>.qc.json` / `.qc.txt` beside the
file. What it measures: picture levels (luma 16-235, chroma 16-240, RGB
beyond -5/+105 %, as a share of each frame), **photosensitive flashes**
(ITU-R BT.1702 / Ofcom: more than 3 flashes in any second over a quarter
of the screen, general and saturated red; plus regular stripes), black
and frozen frames, letterbox bars, size / shape / frame rate against the
spec, true peak, integrated loudness against the target, clipping,
dropouts to digital silence inside the programme, a dead channel,
inverted phase, DC offset, missing sound, missing captions (speech and no
captions), picture and sound of different lengths. `spec` judges by a
delivery spec; `preflight: true` (or no file) checks the project without
rendering: the mix's loudness, captions against speech. Say what failed
and fix it; do not tell the user a delivery is ready while QC says FAIL.
The flash test is a screening aid, not a certified Harding FPA; its
pattern test is approximate (stripes finer than 1/64 of the width are not
seen).

**`deliver` renders to a spec** and checks it. Without arguments it lists
the specs: `youtube`, `youtube-4k`, `youtube-shorts`, `instagram-reel`,
`instagram-feed`, `tiktok` (loudness -14 LUFS / -1 dBTP, their sizes and
length limits), `netflix` (dialogue-gated -27 LKFS, -2 dBTP, strict
levels and flashes, captions), `broadcast-r128` (1920 x 1080 at 25 fps,
-23 LUFS, R 103 legal levels), `broadcast-atsc` (29.97 fps, -24 LKFS),
`web-preview` (720p), and the packages `dcp-smpte`, `dcp-interop`,
`imf-netflix`. The numbers are the published specs as Zornov knew them in
2026: say so, and tell the user to check the platform's current page for
anything contractual. A file spec makes an MP4; the QC report comes back
in the answer.

- **DCP** (`dcp-smpte` / `dcp-interop`): a folder with the picture track
  file (JPEG 2000 in MXF, 12-bit X'Y'Z', DCI white and gamma 2.6, frame
  capped at the DCI 250 Mbit/s), the sound track file (24-bit PCM 48 kHz,
  6 channels: the stereo mix in L R, silence in the rest), CPL, PKL,
  ASSETMAP, VOLINDEX. Frame rates 24, 25, 30, 48, 50, 60 (4K up to 30).
  Flat / scope follows the project's shape inside the 2K container. The
  JPEG 2000 encoder is Zornov's own (written from the standard; standard
  JPEG 2000 decoders read it), software only, about 2-4 s per 2K frame per
  machine: **give a feature to render nodes** (`distributed: true`). Not
  done: encryption / KDMs, signatures, subtitle tracks, 3D. Not tested on
  a cinema server: have it checked in a DCP player before sending it.
- **IMF** (`imf-netflix`, App 2E): MXF track files, CPL, PKL, ASSETMAP.
  Experimental: its CPL descriptors are reduced and the codestream is
  Zornov's general profile, so run Netflix Photon over it before delivery.

**`queue` renders in the background.** `queue add` (with `spec` or
`preset`, `priority`, `qc`, `bundle`, `distributed`) puts the saved
project in the render queue; the Zornov window draws the queue while it
is open (the jobs popover shows it with pause, resume, cancel and "run
first"); from a shell `zornov queue run` draws it, and `queue run` does
the same inside an agent session. `pause` / `resume` / `cancel` /
`priority` / `retry` / `clear` manage it from anywhere; jobs finish with
a notice (`notices`) and a desktop notification. A render is cut into
pieces of 8 s (`chunk_secs`) each starting on a keyframe; finished pieces
stay on disk, so a pause or a crash loses at most the piece in flight,
and the pieces are joined without re-encoding (the picture is the encoder's
own bytes). Sound is mixed once for the whole.
`save_preset` / `presets` / `add` with `from_preset` keep favourite
settings.

**Render nodes.** On another computer on the network run `zornov node`;
it prints an address and a pairing token. `queue add_node {addr, token}`
here (`queue discover` finds nodes, `queue nodes` shows who answers).
A job with `distributed: true` then shares its pieces with every node that
answers (and this Mac), spreading them as machines free up and
re-sending a piece to another machine if one fails. Everything draws with
Zornov's software H.264 encoder in that case, so the pieces join. The
project's files are sent once (checked by fingerprint, so a second render
sends only what changed); media outside the project folder travels too.
Fonts, LUTs and other files a project points at by absolute path must be
inside the project folder or exist on the node. The link is authenticated
by the token but not encrypted, and a paired computer can make the node open
files on its disk through a project's media paths: pair only your own
machines, on a network you trust.

**`bundle` is the deliverables folder**: `master/` (rendered and checked),
`proxy/` (720p), `audio/` (the full mix as 24-bit WAV; `stems/` from the
sound desk when `<project>/stems` has them, otherwise dialogue / music /
effects by lane), `captions/` (.srt and .vtt from the caption clips),
`pictures/` (thumbnails and a poster frame chosen by contrast and
detail), `cutdowns/` and a `manifest.json` with every file's size and
SHA-256, plus a README. Whatever could not be made is under `missing`
(no captions, no sound...): tell the user.

**`cutdown` makes a shorter version** (15, 30, 60 s) as a project of its
own in `cutdowns/`; the user's project is not touched. Main-lane shots are
scored (speech, titles, markers, opening and closing) and the least
valuable dropped until the length fits, then the rest shortened;
overlays and sound follow, music ends with a fade. Give `keep` / `drop`
(clip ids) when you know better, `vertical: true` for a 1080 x 1920
reframe that follows the subject, `render: true` for the file. Open the
cutdown and look at it (`contact_sheet`): the plan is arithmetic, the
judgement is yours. `bundle` with `cutdowns: [15, 30, 60]` and
`verticals: true` does all of them.

The order that works: `check` -> fix -> `deliver {spec}` (or `queue add`
for long ones) -> read the QC findings, fix, render again -> `bundle`.

## Reference: Displays and delivery

- **Aspect**: 16:9 (1920x1080) for YouTube and screens, 9:16
  (1080x1920) for Reels, TikTok and Shorts, 1:1 or 4:5 for feeds. Build
  for the destination; reframe, do not just letterbox.
- People watch on phones, often without sound: big type, captions for
  speech, the story clear from the picture.
- Export presets carry the right size and frame rate; keep the
  project's frame rate (do not mix 24/25/30 without reason).

## Reference: tool `export`

Render a file. Presets: Social 1080p, 4K master, Vertical, GIF, Intermediate, PNG sequence, HEVC 10-bit (smooth gradients, hardware), Audio only (M4A), masters (ProRes 422 HQ, ProRes 4444, ProRes Proxy, EXR / DPX / TIFF / PNG 16-bit sequences, DNxHR LB, DNxHR SQ, DNxHR HQ, DNxHR HQX, DNxHR 444: QuickTime with sound and timecode), Lottie (or dotLottie; also any path ending .json or .lottie): a vector animation for the web and apps, its pictures, titles, cards and shapes with their motion baked from Zornov; the reply lists what Lottie cannot hold (video, sound, effects, masks). Waits for the file unless `wait` is false, for at most 45 seconds: a longer render answers with {"export": id, "still_rendering": true} and goes on in the background; poll export_status {"export": id} until it says done.

## Reference: tool `queue`

The render queue: exports that run in the background, in priority order, with pause and resume, saved presets and notices, optionally shared with render nodes on the local network. action: add {name, spec | preset, path, range [from, to] seconds, qc (default true), bundle, priority (higher first), distributed (share chunks with nodes), chunk_secs, from_preset} / list / pause {job} / resume {job} / cancel {job} / retry {job} / priority {job, priority} / clear (finished) / run {wait} (draw the queue here until empty) / notices / presets / save_preset {name, spec, preset, qc, bundle, distributed, priority} / nodes (render computers and whether they answer) / add_node {addr, token, name} / remove_node {name} / discover. A job renders the saved project. A render node is another computer running `zornov node`; it prints its address and a pairing token.

## Reference: tool `qc`

Automated quality control of a finished video file (default: the last export), with a report: picture levels (legal range 16-235, RGB gamut), photosensitive-epilepsy flashes (ITU-R BT.1702 / Ofcom: more than 3 flashes a second), black and frozen frames, letterbox bars, aspect and size against the spec, sound true peak, integrated loudness against the target, clipping, dropouts, dead channels, phase, missing sound, missing captions. Each finding has a time, a severity (fail / warning) and a fix. `spec` judges it by a delivery spec (youtube, tiktok, netflix, broadcast-r128 ...). `preflight` (or no file at all) checks the project without a file: the mix's loudness, captions against speech, canvas. The report is written beside the file as .qc.json and .qc.txt. Run it before you say a delivery is ready, and fix every fail.

## Reference: tool `deliver`

Render for a platform or for mastering, check it, report. No arguments (or list): the specs with their notes: youtube, youtube-4k, youtube-shorts, instagram-reel, instagram-feed, tiktok, netflix, broadcast-r128, broadcast-atsc, dcp-smpte, dcp-interop (a cinema package: JPEG 2000 in MXF, CPL, PKL, ASSETMAP; unsigned, unencrypted), imf-netflix (IMF package, experimental), web-preview. With spec: renders the project to that spec (size, rate, loudness judged), runs QC and returns the verdict. `queue: true` adds it to the render queue instead; `distributed` shares it with render nodes; `bundle` also makes the deliverables bundle.

## Reference: tool `bundle`

The deliverables bundle in one folder with a manifest (sizes and SHA-256 of every file) and a README: master (rendered, QC'd), proxy (720p review copy), the full mix as 24-bit WAV and stems (the sound desk's stems folder when it made them, otherwise dialogue / music / effects by lane), captions as .srt and .vtt, thumbnails and a poster frame (the best of 24 sampled by contrast and detail), and cutdowns. `cutdowns: [15, 30, 60]` makes those lengths (and `verticals: true` their 9:16 reframes). Each part can be turned off (master, proxy, audio, stems, captions, thumbnails, poster). What could not be made is listed under `missing` and in the README.

## Reference: tool `cutdown`

A shorter version of the programme (15, 30, 60 s...) as a project of its own in cutdowns/ (yours is never changed). Main-lane shots are scored (speech, titles, markers, the opening and closing), the least valuable are dropped until the length fits, the rest shortened if needed, and sound and overlays follow the cuts (music ends with a fade). `keep` / `drop`: clip ids you insist on or refuse. `vertical`: also 1080x1920 with the shots filled and reframed on the subject. `render: true` writes the video (with QC). Look at the result (open it, contact_sheet) and rerun with keep / drop until it is the version you would ship.
