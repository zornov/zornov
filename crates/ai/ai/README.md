# studio-ai

Local AI features for Studio, built on the in-house `studio-nn` inference
engine. Everything runs on the user's machine; the only network access is
the model download the user starts. See [`docs/models.md`](../../docs/models.md) for every
model, its source and licence.

## Layout

| Module | What it does |
|---|---|
| `registry` | Every model: upstream URLs (pinned revisions), SHA-256, sizes, licence. |
| `store` | The models folder `~/Movies/Studio/Models`: status, verify, **download** (system `curl`, `.part` + resume, checksum, `.tar.gz` unpacking), remove. |
| `jobs` | Worker threads for inference: progress, messages, cancel, panic isolation, a list for the jobs popover. |
| `cache` | Results on disk keyed by (media hash, model, parameters); `media_hash` hashes size + three 1 MiB samples. |
| `engine` | Shared, compiled `studio-nn` sessions (Metal when available, else CPU). |
| `frame` | `Frame` (RGBA8), `Mask` (f32 0..1), the `FrameSource` trait clips are read through, guided filter, rotated crops. |
| `features::*` | The features below. |
| `objects` | The object map of a piece of footage or a picture: what is in it, where in every frame, how far and how big (depth to metres), colours, text, faces and poses, relations (on, holding, in front of), when each thing enters and leaves, camera motion apart from object motion, free space for titles, and moment search by description. Cached by content hash. Built from the open-vocabulary detector, depth, text reader, face and pose models and the motion tracker; tests drive it with scripted sensors. |

## Features

| Feature | Module | Model | Without the model |
|---|---|---|---|
| Captions (word timestamps, language detection, translate to English, SRT) | `transcribe` | Whisper tiny/base/small | `ModelMissing` |
| Remove background (video) | `matting` | MODNet + guided-filter edges + motion-adaptive temporal smoothing | `ModelMissing` |
| Object cut-out (click / box, track through clip) | `segment` | SAM 2.1 tiny | `ModelMissing` |
| Voice cleanup (strength slider, offline to WAV) | `denoise` | DeepFilterNet 3 | spectral gate |
| Smooth slow motion | `interpolate` | RIFE 4.9 | frame blending |
| Depth maps (temporally stable) | `depth` | Depth Anything V2 small | `ModelMissing` |
| Faces (+ tracker) and body pose | `faces` | BlazeFace, BlazePose | `ModelMissing` |
| Open-vocabulary detection (find anything named, from text) | `detect` | OWLv2 base | `ModelMissing` |
| Picture-text embeddings (find a moment by description, name a crop) | `embed` | SigLIP base | `ModelMissing` |
| Text in the picture (Latin script) | `ocr` | PaddleOCR | `ModelMissing` |
| Scene cuts (hard cuts, fades, flash-proof) | `scenes` | none | — |
| Auto reframe (9:16, 1:1, …) | `reframe` | faces if installed | saliency + motion |
| Silence / filler / repeat proposals | `silence` | transcript for fillers | silences still work |
| Beat sync (beats, onsets, snapping, clip fitting) | `beats` | none (`studio-audio`) | — |

`features::Feature::availability(&store)` says for each feature whether it
is ready, running on a fallback, or needs a model — the AI panel uses it to
show a download button instead of an error. `Error::ModelMissing(id)`
formats as a user-facing message with the model's name and size.

Edits are proposals: scene cuts, reframe crop paths and silence/filler cuts
are returned as data for the user to review and accept. Explicit commands
(remove background, voice cleanup) produce their output directly.

## Using it

```rust
use studio_ai::{features::transcribe::*, CacheKey, Jobs, ModelStore, ResultCache};

let store = ModelStore::default();
let jobs = Jobs::new(1);
let cache = ResultCache::new(ResultCache::default_dir());

// Off the UI thread, cached per (media, model, options):
let whisper = Whisper::load_best(&store)?; // Err(ModelMissing(..)) → show download button
let opts = TranscribeOptions::default();
let key = CacheKey::new(studio_ai::media_hash(&clip_path)?, whisper.model.key(), opts.cache_params());
let job = jobs.spawn_cached("Captions", cache, key, move |ctx| whisper.transcribe(&mono, rate, &opts, ctx));
// each UI frame: job.progress(), job.message(), job.cancel(); when finished: job.take_result()
```

Downloading a model (only on a user click):

```rust
let h = jobs.spawn("Download Whisper", move |ctx| store.download(ModelId::WhisperTiny, ctx));
```

Video features read frames through `FrameSource` (the render crate
implements it over its decoders) and hand results to a sink closure, so a
whole clip's mattes or depth maps never have to sit in memory at once.

## Tests

```sh
cargo test -p studio-ai                      # unit tests, no models needed
cargo test --release -p studio-ai --test real_models -- --ignored --nocapture
```

The real-model tests skip (with a message) any model that is not
installed. They synthesise their inputs (speech from macOS `say`, test
patterns); set `STUDIO_AI_TEST_PHOTOS` to a folder with `melvin.jpg`,
`chawla.jpg` and `aldrin.jpg` (public-domain NASA photos, not shipped) to
also run the face, pose and matting checks on real photographs, and
`STUDIO_AI_TEST_OUT` to dump the masks as PNGs.

Rough timings on an Apple-silicon laptop (default backend): Whisper tiny
≈ 7× real time; MODNet 0.2 s per 2 MP frame; SAM 2.1 encoder 1.4 s per
frame, each extra prompt 0.08 s; RIFE 0.13 s at 320×240; DeepFilterNet
≈ 30× real time; face detection 8 ms, pose 70 ms per frame.

## Implementation notes

- Whisper: greedy decoding with the standard timestamp rules (paired,
  monotonic timestamps, timestamp-probability rule, first timestamp ≤ 1 s),
  segment-based seeking, no-speech skipping and a repetition guard. Word
  times come from DTW over the cross-attention of the alignment heads in
  `generation_config.json`, median-filtered and z-normalised. The decoder
  runs without a KV cache (simplest export); fine for tiny/base, slower for
  small.
- DeepFilterNet: features and STFT follow the reference implementation
  (Vorbis window, ERB bands, exponential mean / unit normalisation with
  α = 0.99). The exported networks' outputs lag by `conv_lookahead` = 2
  frames, which is compensated (measured: +12.6 dB SNR at 3 dB input; the
  uncompensated version gives +7 dB). Long files run in 30 s chunks with
  1 s warm-up; chunk seams were checked to be inaudible in SNR terms.
- SAM 2.1: this export only returns the three multi-mask candidates, so the
  best-rated one is always used. Box-only prompts pass one padding point
  (label −10). Tracking re-prompts each frame from the previous mask.
- BlazePose's detector takes [0, 1] input while BlazeFace takes [−1, 1]
  (determined by testing on photos).
