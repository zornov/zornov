<p align="center">
  <img alt="Zornov: the free, open-source film studio" src="docs/images/banner.png" width="100%">
</p>

<p align="center">
  <a href="LICENSE"><img alt="Licence: AGPL-3.0-or-later" src="https://img.shields.io/badge/licence-AGPL--3.0--or--later-DC1F2C?style=flat-square"></a>
  <a href="LICENSING.md"><img alt="Commercial licence available" src="https://img.shields.io/badge/commercial%20licence-available-3A3A42?style=flat-square"></a>
  <img alt="Written in Rust" src="https://img.shields.io/badge/written%20in-Rust-3A3A42?style=flat-square">
  <img alt="Platforms: macOS, Windows, Linux" src="https://img.shields.io/badge/platforms-macOS%20%C2%B7%20Windows%20%C2%B7%20Linux-3A3A42?style=flat-square">
</p>

<p align="center">
  <a href="https://zornov.com/download"><b>Download</b></a> ·
  <a href="https://zornov.com">Website</a> ·
  <a href="#made-in-zornov">Films</a> ·
  <a href="#the-ai-editor">AI editor</a> ·
  <a href="#what-it-does">Features</a> ·
  <a href="#build-and-run">Build</a> ·
  <a href="#documentation">Docs</a>
</p>

<br>

<p align="center">
  <img alt="A fast motion-design edit: a parallax title over an astronaut, a grid of clips, tracked labels on a dancer, a speed ramp, a spinning video cube and a flame-filled title" src="docs/images/films/keyframes.webp" width="100%">
  <br>
  <sub>1,344 keyframes in one edit, made in Zornov.</sub>
</p>

Zornov is a free, open-source film studio. Edit, grade, animate puppets, build 3D worlds, compose the score and deliver, all in one app. An AI director can work on the same timeline while you do, and you can take over anywhere.

## Made in Zornov

Every film here was edited, animated, scored and exported in Zornov, from public-domain footage or from scratch.

<table>
  <tr>
    <td width="50%"><img alt="A city edit cut on the beat: trams, a night market, subways and a light show" src="docs/images/films/edit.webp"></td>
    <td width="50%"><img alt="Drummers, dancers and a rocket launch cut to the beats of music composed in the app" src="docs/images/films/sound.webp"></td>
  </tr>
  <tr>
    <td align="center"><sub><b>Edit</b>: city nights, cut on the beat</sub></td>
    <td align="center"><sub><b>Sound</b>: music composed in the app, every cut on a beat</sub></td>
  </tr>
  <tr>
    <td><img alt="A figure walking down a rainy night street with glowing lamps and wet reflections" src="docs/images/films/3d.webp"></td>
    <td><img alt="A puppet character with labelled pins, bones, scissor cuts, blinks and lip sync" src="docs/images/films/puppet.webp"></td>
  </tr>
  <tr>
    <td align="center"><sub><b>3D</b>: a rainy street, rendered in Zornov</sub></td>
    <td align="center"><sub><b>Puppets</b>: every puppet tool, labelled as it works</sub></td>
  </tr>
  <tr>
    <td><img alt="A space mission trailer: launch, orbit, Earth and return" src="docs/images/films/director.webp"></td>
    <td><img alt="A cycling clip that opens from a vertical frame to widescreen and back" src="docs/images/films/deliver.webp"></td>
  </tr>
  <tr>
    <td align="center"><sub><b>AI director</b>: a mission trailer, cut by AI through Zornov's tools</sub></td>
    <td align="center"><sub><b>Deliver</b>: one film, tall and wide</sub></td>
  </tr>
</table>

## The editor

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/editor-dark.png">
  <img alt="The Zornov editor: media, player, details and a timeline with captions, eight shots and a score" src="docs/images/editor-light.png" width="100%">
</picture>

## The AI editor

<img alt="The AI editor planning a cut to the music, next to the timeline it is editing" src="docs/images/ai-dark.png" width="100%">

Tell it what you want, and it works on your timeline live, with more than 80 tools: it can see, listen, cut, model, animate, light, score, check and export.

- **Crew mode** splits a film across an Editor, Animator, Sound designer, VFX artist and Colourist working in parallel.
- **Plans you can follow.** Bigger jobs start with a plan. You approve checkpoints, correct it mid-flight, or stop it.
- **Your edits and its edits never collide**, and each side has its own undo.
- **Skills and lessons.** It loads how-to packs for each kind of work and learns from your corrections. You can edit or delete what it learned in Settings.
- **Checks before export:** overlaps, titles over faces, flat lighting, floating objects, sliding feet, bad lip sync and more, each with the exact fix.
- **Real media first.** It looks in your library before it generates anything.
- **Careful with outside content.** Web pages, transcripts and shared notes are treated as material, never as instructions, and anything that sends data out or spends money asks you first.

The AI editor uses the model you connect: Codex with your ChatGPT sign-in, Claude Code, or an API key. What you ask it, and what it looks at to answer, goes to that provider. The rest of Zornov works without it, and offline.

## What it does

<img alt="The start screen: new projects by frame shape, templates drawn as the timelines they set up, and recent projects" src="docs/images/start-dark.png" width="100%">

### Edit
- A magnetic timeline with lanes, markers, groups, nesting, ripple, roll, slip and slide.
- Source/record editing, J and L cuts, three-point edits, takes and string-outs.
- Keyframes and curves, speed ramps, time remapping, freeze frames and reverse.
- Titles, lower thirds, callouts, charts and 3D text, edited in place on the canvas.

### Picture
- Native decoding and encoding of H.264, HEVC, AV1, VP9, ProRes, DNxHR, AAC, Opus and Vorbis, in MP4, MOV, MXF and WebM.
- A GPU compositor working in 16-bit float, with blend modes, masks, mattes, tracking, stabilising and corner pinning.
- Colour grading with scopes, LUTs, HDR and shot matching.
- Keying, rotoscoping and cut-outs. Restore, denoise, upscale and smooth slow motion.

### 3D
- A full modelling workspace: extrude, inset, bevel, loop cut, knife and bridge, modifiers, UVs, materials and sculpting.
- Characters with fingers, eyes, lashes, teeth and hair, fitted clothes, rigs and walks with planted feet.
- Physics: rigid bodies, ragdolls, cloth, hair, fracture, fluids and smoke, baked to the timeline.
- A real-time renderer and a path tracer with weather, fog, light beams and wet surfaces.
- Walkable worlds with terrain, plants, water and sky, recorded as camera shots. Procedural streets and interiors in eight styles.
- glTF, OBJ, FBX, STL and PLY import.

### Animation
- Puppets with pins, bones and scissor cuts, so limbs and lips move apart.
- Lip sync, blinks, acting beats, drivers and squash and stretch.
- Motion presets, stagger, expressions and particles.
- Vector illustration with paths, pathfinder, type and trace.

### Sound
- A mixer with buses, loudness, stems, dialogue cleanup and ADR, and an effects chain on every clip.
- Original music composed in the app in 11 genres, fitted to the exact length and cue points.
- About 50 physically modelled sound effects, animal voices, footsteps on 13 surfaces, and sung vocals.
- Narration with the system's voices, or ElevenLabs and Fish Audio if you add their keys.

## Download

Get Zornov for macOS, Windows or Linux from **[zornov.com/download](https://zornov.com/download)**. It updates itself from inside the app.

## Build and run

Zornov is written in Rust (edition 2024).

```bash
cargo build --release -p studio-app
./target/release/zornov
```

To make a signed macOS app bundle:

```bash
cargo xtask bundle
```

See [`docs/releasing.md`](docs/releasing.md) for signing, notarisation and the update feed.

<details>
<summary><b>Linux in a container</b></summary>

<br>

To build and test the Linux version without installing anything else, use the container in [`docker/Dockerfile`](docker/Dockerfile). Open the repository in VS Code or GitHub Codespaces and choose **Reopen in Container**, or run:

```bash
docker build -t zornov-linux docker
docker run --rm -it -v "$PWD:/work" zornov-linux cargo test --workspace
```

The container has a software graphics driver, so the GPU tests run without a graphics card. It can also check that the Windows code compiles; see [`CONTRIBUTING.md`](CONTRIBUTING.md).

</details>

<details>
<summary><b>Headless use</b></summary>

<br>

Zornov can be driven without a window, for scripts, tests and automation. Every film above was made this way:

```bash
zornov agent --headless --project path/to/project --out path/to/out -
```

Send one JSON line per tool call on standard input, for example `{"tool": "frame", "args": {"t": 2.0}}`.

</details>

## Documentation

| Guide | What it covers |
|---|---|
| [AI crew](docs/ai-crew.md) | How the AI editor and its crew work |
| [Plugins](docs/plugins.md) | Writing plugins: effects, generators, transitions, importers, exporters and panels |
| [Accessibility](docs/accessibility.md) | Using Zornov without a mouse, with a screen reader, at large sizes and in any light |
| [Localisation](docs/localisation.md) | The interface languages and how to add translations |
| [Onboarding, help and workspaces](docs/onboarding-help-workspaces.md) | First run, built-in help and layouts |
| [Accounts and cloud](docs/accounts-and-cloud.md) | Optional accounts, credits, the cloud and the marketplace (off by default) |
| [Models](docs/models.md) | The AI models Zornov can download when you ask, and their licences |
| [Automation security](docs/automation-security.md) | How issues, pull requests and automation are kept safe |
| [Releasing](docs/releasing.md) | Signing, notarisation, release packages and updates |

<details>
<summary><b>Repository layout</b></summary>

<br>

The app is in [`app/`](app): the interface, the editor, the 3D workspace and the AI editor. Its engines are libraries in `crates/`, grouped by what they do:

| Folder | What it holds |
|---|---|
| `crates/codecs/` | Video and audio codecs: H.264, HEVC, AV1, VP9, ProRes, DNxHR, AAC, Opus |
| `crates/media/` | Containers, decoding, encoding, delivery, capture and web media |
| `crates/picture/` | The GPU renderer, colour, effects, text, motion graphics, Lottie |
| `crates/paint/` | The paint and photo engine (layers, brushes, PSD, RAW, HEIF) |
| `crates/vector/` | Vector illustration, path operations, tracing |
| `crates/3d/` | Meshes, modelling, rigs, physics, worlds, 3D quality checks |
| `crates/sound/` | Audio engine, mixing, synthesis, speech and lip sync |
| `crates/ai/` | AI models and the inference engine |
| `crates/project/` | The project model, content library, production, languages, updates |
| `crates/plugins/` | The plugin host and its API |
| `crates/business/` | Accounts, plans and the cloud server (off by default) |
| `crates/dev/` | Build tasks (`cargo xtask`), the shared test kit and the text checks |

Outside `crates/`:

| Folder | What it holds |
|---|---|
| `docs/` | The guides above, and the README's pictures and films |
| `docker/` | The Linux build container (also the dev container) |
| `plugins/` | Example plugins (`examples/`) and their built `.wasm` files (`dist/`), bundled with the app |
| `packaging/` | Release configuration, the app icons, macOS entitlements and the Linux desktop entry |
| `testdata/` | Small shared media, audio and image files used by the codec and app tests |
| `tools/` | Stand-alone developer tools (`pack-content`) |

</details>

## Contributing

Contributions are welcome. Start with [`CONTRIBUTING.md`](CONTRIBUTING.md). [`GOVERNANCE.md`](GOVERNANCE.md) explains how decisions are made, and [`SECURITY.md`](SECURITY.md) how to report a security problem privately.

## Licence

Zornov is open source under the **GNU Affero General Public License v3.0 or later** ([`LICENSE`](LICENSE)). You can use, study, change and share it. If you distribute it, or offer a changed version to people over a network, you share your source under the same licence.

For products or services that can't follow the AGPL, Zornov is also available under a **commercial licence** from Ashrox LLC; see [`LICENSING.md`](LICENSING.md).

Copyright 2026 Ashrox LLC and contributors. Third-party credits are in [`ATTRIBUTIONS.md`](ATTRIBUTIONS.md), and model licences are in [`docs/models.md`](docs/models.md). The films above use public-domain footage from NASA, the Library of Congress (Carol M. Highsmith) and Wikimedia Commons contributors.
