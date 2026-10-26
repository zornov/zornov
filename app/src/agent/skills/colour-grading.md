---
name: colour-grading
use: Use when colour or light needs work: exposure, matching, looks, log, HDR.
topics: colour, color, colours, colors, palette, gradients, gradient, saturation, light, exposure, blacks, whites, contrast, brightness, quality, bits, hdr, alpha, luma, banding, grading, grade, aces, ocio, interchange, conform, edl, fcpxml, formats, proxies, multicam, timecode, post, post-production, frames, previews
tools: scopes, colour, see, frame, contact_sheet, match_colour, copy_look, apply, effect, fx_layer, conform, media_manage, card, check, review
---
**Order**
1. Identify the source: camera log footage needs its input space (`colour {action: "input", media, space}`; `spaces` lists them) and a view (`colour {action: "view", view: "aces"}`), or it looks flat and its highlights clip. Ordinary footage needs neither.
2. Measure before touching: `scopes` at a moment (black p1, median, white p99, blown-out share, cast in shadows / mids / highlights, skin-line share). Healthy: black p1 2-8 percent, white p99 90-98 percent, under 1 percent blown out, median 35-55 percent.
3. Fix exposure first (`colour {action: "grade", clip, params: {exposure, ...}}`), then balance (temperature, tint), then contrast, then the look. Secondary corrections as nodes with a qualifier or a power window (skin, sky, foliage); track a window that must follow a face.
4. Match shots that cut together: grade the hero shot first, then `match_colour` the others `to` it (strength 60-80 percent when they are meant to differ a little); only like with like. `copy_look` passes one clip's look to another.
5. The look: a ready look with `apply` from `catalogue {panel: "looks"}`, or a LUT (`colour {action: "lut"}`); one look across the film, intensity to taste.
6. Graphics: one accent colour over neutrals, a near-black and an off-white, never pure; backgrounds as a gradient `card` of two close colours.
7. HDR delivery: `colour {action: "output", space: "rec2020_pq", hdr_peak}` then `colour {action: "hdr"}` for the numbers. Exchange with other suites: `conform` (EDL, FCPXML, OTIO). Heavy originals: `media_manage` (proxies, consolidate).
8. Measure again: `scopes` with `compare` against the neighbouring shot, and look at both sides of every cut.

**Checks**
- Shots cut together are within about 5 percent of each other on median and black, with the same mid cast name.
- Skin sits on the skin line: off it reads sick (green) or sunburnt (magenta).
- No large area clipped to pure black or white; text over the grade still reads.

**Mistakes**
- Grading before fixing exposure, or matching a sunset to a green field (it turns the field orange).
- Matching shots whose difference is the story (day to night, a flashback).
- Saturating everything: keep big areas muted and small accents saturated.
- Judging banding or sharpness from a light preview: ask `frame` for `detail: true`.

## Reference: Post-production

Colour management, grading, formats, interchange and media management,
as a finishing house works. Tools: `colour`, `conform`, `media_manage`,
and the `export` presets for masters.

**How colour flows.** Every source is decoded from what it is (its input
colour space) into Studio's working space: scene-linear light, BT.709
primaries kept unbounded (wide-gamut colours survive as negative values,
nothing clips), with grading done in ACEScct on AP1 primaries. At the end
of the frame a view transform turns scene light into display light, then
the output space encodes it (Rec.709, P3, Rec.2020 PQ or HLG). So:
- Camera log footage: `colour {"action": "input", "media": [3, 4], "space": "slog3_sgamut3cine"}`
  (`colour {"action": "spaces"}` lists every camera space: S-Log3, ARRI
  LogC3/LogC4, V-Log, Canon Log 1/2/3, Apple Log, RED Log3G10, F-Log/F-Log2,
  DaVinci Intermediate, Blackmagic Film Gen 5, ACEScct, Cineon). A bare
  curve name takes the camera's own gamut (`"logc3"`).
- Then a view: `colour {"action": "view", "view": "aces"}` (ACES 1.3:
  the Rec.709/sRGB, P3 or HDR output transform follows the output space;
  `"aces:2000"` sets the PQ peak). Without a view, log footage looks flat
  until graded and highlights clip.
- HDR delivery: `colour {"action": "output", "space": "rec2020_pq", "hdr_peak": 1000}`
  then `colour {"action": "hdr"}` for MaxCLL / MaxFALL, nits percentiles
  and how much lies outside Rec.709 / P3. 1.0 working = 203 cd/m².
- A studio OCIO config: `colour {"action": "ocio", "config": "/path/config.ocio"}`,
  then inputs `"ocio:ARRI LogC3 (EI800)"` and views
  `"ocio:sRGB - Display/ACES 1.0 - SDR Video"`. Configs with ACES 2.0
  output transforms are read, but those views are refused (Studio has
  ACES 1.3); use Studio's own `aces` view or an ACES 1.x config.

**Grading.** `colour {"action": "grade", "clip": 7, "params": {...}}`
sets any grade parameter (null removes one): `exposure` (stops),
`contrast`, `temperature` (K), `tint`, `saturation`, `highlights`,
`shadows`, `whites`, `blacks`; wheels `lift_hue|amount|luma`, `gamma_*`,
`gain_*`, `offset_*` (or `wheel_mode: "log"` with `shadows_*`,
`midtones_*`, `highlights_*`); curves as point lists in 0..1 (`curve_master`,
`curve_red|green|blue`: `[[0,0],[0.25,0.2],[1,1]]`; `hue_vs_hue`,
`hue_vs_sat`, `hue_vs_lum`, `lum_vs_sat`, `sat_vs_sat` with hue as a
fraction of a turn); eight-band HSL `hsl_<red|orange|yellow|green|aqua|blue|purple|magenta>_<hue|sat|light>`;
an exact ASC CDL `cdl_slope_r|g|b`, `cdl_offset_*`, `cdl_power_*`,
`cdl_sat` (it replaces the four-way wheels). `lut` applies a `.cube` /
`.3dl`; `lut` with `export` bakes a grade into one for set or another
suite (`.3dl` or `.cube`, Rec.709 in and out).
Nodes are secondary corrections after the grade, in order:
`colour {"action": "node", "clip": 7, "qualifier": "skin", "params": {"exposure": 0.3, "saturation": 0.9}}`
(qualifier presets skin, sky, foliage, red…magenta, highlights,
midtones, shadows, or `{hue, hue_width, sat_low, sat_high, lum_low, lum_high, …}`);
a power window `"window": {"shape": "ellipse", "x": 0.5, "y": 0.4, "width": 0.3, "height": 0.4, "feather": 0.1}`
makes a mask that shapes only the node: track it to follow a face with
`track {"clip": 7, "mask": <mask id>, "point": [x, y]}`. `mix`,
`key_blur`, `window_invert`, `node: n` edits node n, `remove: true`.
Shot matching is the edit op `match_colour`. The stills gallery keeps a
grade with its picture: `colour {"action": "stills", "save": "Day ext A", "clip": 7}`,
then `{"apply": "Day ext A", "clips": [9, 12]}`.
`cdl` imports `.cdl` / `.cc` / `.ccc` from set (or slope/offset/power/sat)
and exports a `.ccc` of the given clips.

**Formats.** Studio plays these as they are (no conversion): H.264, HEVC,
Apple ProRes 422 / HQ / LT / Proxy / 4444 / 4444 XQ in MOV at full 10/12-bit
precision with PCM sound, OpenEXR (none, RLE, ZIP), DPX (8/10/12/16-bit,
Cineon log decoded), TIFF (8/16-bit, float) and 16-bit PNG, alone or as
frame sequences: `import` a pattern like `/plates/sh010.####.exr` (or
`%04d`) or any frame of one. Masters (`export` presets): `ProRes 422 HQ`,
`ProRes 4444`, `ProRes Proxy` (24-bit PCM, timecode from 01:00:00:00,
reel name), `EXR sequence (ACES)` (ACES2065-1 half float, the ACES
container), `DPX sequence` (10-bit), `TIFF sequence`, `PNG 16-bit sequence`,
`DNxHR LB`, `DNxHR SQ`, `DNxHR HQ` (8-bit 4:2:2), `DNxHR HQX`
(10-bit 4:2:2) and `DNxHR 444` (10-bit 4:4:4), written by Studio itself
in MOV with the same sound and timecode (4:2:2 needs an even width).
Not available: DNxHD/DNxHR decoding without conversion,
EXR PIZ / DWA / B44 compression (re-save as ZIP), alpha in EXR/ProRes
4444 playback (read, not drawn yet).

**Timecode, reels, rates.** Source timecode and reel names are read from
camera MOVs (their timecode track), DPX and EXR headers; see or fix them
with `media_manage {"action": "info"}` / `{"action": "set", "media": 3,
"timecode": "14:22:10:05", "reel": "A001C003"}`. Drop-frame (`;`) applies
to 29.97 and 59.94; 23.976 never drops. EDLs and conform use them.

**Interchange.** `conform {"action": "export", "path": "/out/reel1.edl"}`
(CMX3600: source timecode, reels, dissolves, speed `M2`, ASC CDL, LOC
markers; overlays are listed as not carried), `.fcpxml` (FCPXML 1.10 for other editing suites: lanes, titles, compound clips, dissolves,
markers, speed) or `.otio`. `conform {"action": "import", "path": ...}`
reads EDL, FCPXML and OTIO as a new sequence; EDL reels not in the
project become offline media: `conform {"action": "relink", "folder":
"/Volumes/RAID/day1"}` finds them by name, reel and timecode and keeps the
same source frames. AAF is not supported: ask for EDL, FCPXML or OTIO.

**Media management.** Heavy originals: `media_manage {"action":
"proxies", "scale": 2}` makes ProRes 422 LT proxy files (kept with the project)
and switches the viewer to them; `use_proxies {"on": false}` goes back.
Export always reads the originals; a proxy stands in when an original is
missing. `consolidate {"folder": ...}` copies every used file (frame
sequences, proxies, LUTs) into one folder and relinks. Multicam:
`media_manage {"action": "multicam", "media": [3, 4, 5], "sync": "sound"}`
lines the angles up by their sound (or `"timecode"`), builds the
multicam sequence and puts its clip on the main lane; cut between angles
with `{"action": "angle", "clip": 21, "at": 12.5, "angle": 2}`.

## Reference: Colour

- **Measure, then grade**: `scopes` before and after a grade. Healthy
  footage: black p1 2-8 %, white p99 90-98 %, under 1 % blown out
  (more is fine only for a window or a light source), median 35-55 %
  for a normal scene. Shots cut together should land within about 5 %
  of each other on median and black, with the same mid cast name;
  `compare` says by how much they differ. Skin belongs on the skin line:
  a face that sits off it reads sick (green) or sunburnt (magenta).
- **Matching shots** (`match_colour`): when shots from two cameras, two
  times of day or a phone and a camera cut together and jump, match the
  odd ones `to` the hero shot (the one already graded or the best
  looking): it moves their colour and light to the reference's, then
  their own grade applies on top. Grade the hero first, then match the
  rest; check with `scopes` / `compare` on both sides of a cut. Use
  `strength` 60-80 % when the shots are meant to differ a little (a
  warm interior after a cool exterior). Do not match shots whose
  difference is the story (day to night, a flashback, a dream), or a
  shot to one with different content (a sunset to a green field: it
  turns the field orange); match like with like.
- **Palette**: one accent colour (two at most) over neutrals: a
  near-black (#0e0f14, not pure #000), an off-white (#f4f4f6, not pure
  #fff), and two or three greys. The accent marks what matters: the key
  number, the button, the winner. `check`: `palette_sprawl`.
- **Contrast**: text must read: light on dark or dark on light, never
  mid on mid. Saturated colour on saturated colour vibrates (red on
  blue, pink on green): put a neutral between them.
- **Harmony**: analogous colours (neighbours on the wheel) feel calm;
  complementary (opposites) feel energetic, so use one as the accent and
  the other sparingly. Brand colours win over theory.
- **Temperature**: warm (orange, red) advances and excites; cool (blue,
  teal) recedes and calms. Grade footage warm for human stories, cool
  for tech; keep a sequence consistent.
- **Saturation** is a volume knob: a little goes a long way. Fully
  saturated backgrounds tire the eye; keep big areas muted and the small
  accents saturated.
- **Gradients**: two close colours (same hue, 10-25% apart in brightness)
  look rich; two far colours look like a 2005 web button. A dark gradient
  backdrop (`card` with `gradient: ["#1b1f3b", "#05060c"]`, `angle: 90`)
  beats a flat colour. `radial: true` makes a soft spotlight behind the
  hero.
- **How Studio makes them clean** (so you do not have to fight it):
  colours mix in OKLab, where equal steps look equal (no bright plateau
  that suddenly falls off, no grey middle between two hues); the ramp
  eases smoothly through every stop with no crease where it starts, ends
  or meets a stop (the seams the eye sees in a plain linear ramp); a
  colour fading to transparent keeps its colour; radial light falls off
  with a long soft tail and no edge; a fixed fine grain hides banding.
  More colours in the `gradient` list (3-6) give a richer ramp: add an
  in-between colour to steer the middle (e.g. `["#2a1b5c", "#1a1240",
  "#05060c"]`), never a different hue just to have one.
- **Where 8-bit video still bands**: very dark, very smooth ramps across
  the whole frame lose to 8-bit and to every platform's re-encoding. Keep
  some brightness range in a backdrop (not near-black to near-black),
  let texture or footage break up big flat areas, and keep glows small.

## Reference: Light: exposure, blacks, whites, contrast

- **Exposure** (stops): +1 doubles the light. Fix under- or
  over-exposed footage first, before any look.
- **Blacks and whites**: real footage should touch near-black in its
  shadows and near-white in its highlights, without large areas clipped
  to pure black or pure white (detail is gone there for good). A frame
  that is all mid-grey looks washed out: raise contrast. A frame with
  big pure-white areas looks blown: lower highlights or exposure.
- **Contrast** shapes mood: high for punch and drama, low for soft and
  nostalgic. Change it gently (±10-25).
- Match shots that cut together: the same brightness and warmth from
  shot to shot (`copy_look` from one to the next), or the cut jumps.

## Reference: Colour quality, bits, HDR, alpha, luma

- Studio composites in linear light at high precision and exports 8-bit
  Rec.709 (the standard for social and web), dithered so gradients do
  not band. Flat colours come out exact.
- **8-bit banding**: very dark, very smooth gradients and big soft glows
  are where 8-bit shows steps. Avoid huge low-contrast glows on near-black;
  a little texture (grain, a light leak) hides banding.
- **HDR** footage (very bright highlights) is tone-mapped to SDR; do not
  push exposure on it, and avoid pure-white text over HDR highlights.
- **Alpha** is a picture's transparency (a title, a cut-out, a sticker
  has it; footage does not). **Luma** is brightness. Mattes and keys use
  one or the other.
- **Previews are light**: `frame` and `contact_sheet` pictures are small
  JPEGs (512-1024 px). Judge layout, motion, colour and readability from
  them; for small text, thin lines, edges and banding ask for
  `detail: true` before deciding anything is wrong or right. Never judge
  the final picture's sharpness from a light preview, and never let a
  blurry preview talk you into sharpening.

## Reference: tool `colour`

Colour management and grading, as in a grading suite. action spaces: the working space, view, output, every media's input colour space and what can be chosen. input {media, space}: the colour space footage was shot in (camera log: slog3_sgamut3cine, logc3_awg3, logc4_awg4, vlog_vgamut, clog3_cinemagamut, applelog_rec2020, log3g10_rwg, flog2_rec2020, davinci_intermediate_dwg, bmdfilm_gen5_bmdwg; rec709, srgb, acescg, aces2065-1, cineon; ocio:<space>; auto = what the file says). view {view: none | aces | aces:<peak nits> | ocio:<display>/<view>} sets the display transform. output {space: rec709 | p3 | rec2020_pq | rec2020_hlg, hdr_peak}. ocio {config}: load an OpenColorIO config. grade {clip(s), params, lut, reset}: any grade parameter (exposure, contrast, wheels lift_/gamma_/gain_/offset_hue|amount|luma, curves as [x,y] points, hsl_<band>_hue|sat|light, cdl_*). node {clip, node?, params, qualifier: preset (skin, sky, foliage, highlights…) or {hue, hue_width, sat_low…}, window: {shape, x, y, width, height, feather} or a mask id, window_invert, mix, key_blur, remove}: secondary corrections in order; track a window's mask with the track tool. lut {clip(s), path, intensity} applies a .cube/.3dl; lut {clip, export: path, size} bakes the grade. cdl {clip(s), slope, offset, power, sat | path: .cdl/.cc/.ccc} or {clips, export: path}. hdr {t | from, to, count}: MaxCLL, MaxFALL, nits, gamut excursions. stills {save: name, clip, t} | {apply: name, clips} | {remove} | list. See guide topic post.

## Reference: tool `conform`

Exchange timelines with other editing and finishing systems and relink media. action import {path: .edl | .fcpxml | .otio, rate?}: a new sequence (EDL reels become offline media until relinked; ASC CDLs and LOC markers come in). export {path, format?: edl | fcpxml | otio, start_timecode?}: the open sequence (EDLs carry source timecode, reels, CDLs and markers; FCPXML 1.10 for other editing suites). relink {folder, media?, by_timecode?}: find offline or missing media under a folder by file name, reel and timecode. offline: what still needs a file. AAF is not supported (exchange AAF timelines as EDL, FCPXML or OTIO).

## Reference: tool `media_manage`

Media management. info {media?}: codec, size, fps, source timecode, reel, input colour, proxy, offline. set {media, timecode?, reel?}. proxies {media?, scale: 2 | 4, again?, use?}: make Apple ProRes 422 LT proxy files and use them (export always reads the originals). use_proxies {on}. forget_proxies {media?}. consolidate {folder, all?}: copy every used file (sequences, proxies, LUTs) into one folder and relink. multicam {media: [angles], sync: sound | timecode, audio: angle, name, place?, at?}: sync angles and put a multicam clip on the timeline. angle {clip, angle, at?}: cut the multicam clip to an angle at a time (without at: the whole clip).

## Reference: tool `scopes`

The video scopes at time `t` as numbers: luma percentiles (black, median, white), blown-out, crushed and out-of-broadcast-range shares, the colour cast in shadows, mids and highlights, saturation (mean, 95th percentile, oversaturated share), the overall tint and how much sits on the skin line. `compare`: a second moment, with the differences (to match one shot to another by measurement). `picture`: also the RGB parade and vectorscope as one JPEG.
