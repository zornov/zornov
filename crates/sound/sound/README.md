# studio-sound

Mixing and audio processing for Zornov, one layer above `studio-audio`'s
device I/O. `render` uses it to mix a sequence for export; `app` uses the
same graph for live monitoring, plus the meters, loudness, beat analysis,
waveform caches and the voice-over recorder.

```
MixPlan ──Mixer::new──▶ Mixer ──render / render_multi──▶ stereo or N-channel blocks
  clips: source → speed/rate (sinc + WSOLA) → volume·fades·mute → effects → pan
  lanes: Σ clips → effects → fader (automation) → ducking → pan / surround panner
         → its bus (by id, or the bus of its role's kind) or the master; sends → aux buses
  buses: Σ lanes and sends → effects (per channel pair) → gain → mute/solo → Σ master
         (delay-compensated against the slowest bus)
  master: gain → effects → LFE low-pass (surround) → linked true-peak limiter → loudness → out
```

A plan has a `Layout` (`Stereo`, `Surround51` L R C LFE Ls Rs, `Surround71`
L R C LFE Lss Rss Lrs Rrs). With no buses and a stereo layout the graph is
exactly the old clip → lane → master one. `MixPlan::stem(kinds)` keeps
only the lanes of some stems (DX / MX / FX / BG) with the limiter off, so
stems add up to the mix.

All DSP is `f32`, planar, block based (any block up to the `max_block` the
mixer was built with; longer requests are split). Everything is sized at
construction and the render path never allocates (a test proves it with a
counting allocator). Build on a control thread, run on the audio or render
thread.

Times are `Micros` (microseconds, `i64`), the model's unit; inside the DSP
everything is frames at the mixer's rate. `micros_to_frames` /
`frames_to_micros` convert.

## Modules

| module | what |
|---|---|
| `mixer` | `MixPlan` → `Mixer`; `AudioSource`, `MemorySource`, `EffectSpec`, `mix_offline`, `measure_source` |
| `automation` | `Curve` of `Key`s with `Ease`, sampled per frame; `FadeShape`, `Fades` |
| `resample` | streaming windowed-sinc `Resampler` (`Quality::Fast/Good/High/Best`), one-shot `resample` |
| `eq` | RBJ `Biquad`, five-band `ParametricEq` with HP/LP, `response_db` for the inspector curve |
| `dynamics` | `Compressor`, true-peak `Limiter`, `Gate`, `DeEsser`, `EnvelopeFollower`, `TruePeak` |
| `reverb`, `delay` | Dattorro-style plate `Reverb` (`ReverbParams::{ROOM,PLATE,HALL}`, twelve film rooms in `ROOMS` / `ReverbParams::room`), `StereoDelay` (ping-pong, tempo sync) |
| `pitch` | WSOLA `Stretcher`, `PitchShifter` (±24 st, length preserved), `stretch`, `dominant_frequency` |
| `voice` | 35 `VoiceStyle`s in six `VoiceGroup`s, each a `VoiceChain` of the units above |
| `vad`, `ducking` | speech detector `Vad`, `speech_segments`; `Ducker` gain ramp |
| `loudness` | BS.1770-4 `LoudnessMeter` (stereo, or any layout with `with_layout` / `process_multi`), `measure`, `measure_layout`, `measure_within` (dialogue gating), `LoudnessTarget` (EBU R128, ATSC A/85, Netflix dialogue-gated, Amazon, streaming, podcast: target, true-peak ceiling, tolerance), `normalise_gain_db`, `test_vectors` |
| `beats` | `analyse` → `BeatAnalysis` (bpm, beats, onsets, downbeats, confidence), `BeatGrid`, `snap_to` |
| `peaks` | multi-resolution `PeakCache` for waveform drawing |
| `recorder` | `VoiceOverRecorder` state machine (count-in, meter, take), `DeviceFeed` over `studio_audio::AudioIn` |
| `buffer` | `StereoBuf`, the `Effect` trait, `db_to_gain`, `pan_gains` |
| `surround` | `Layout` (stereo, 5.1, 7.1: channel names, ITU azimuths, BS.1770 weights, WAV order and mask, Lo/Ro fold-down), the pairwise (2-D VBAP) panner `source_gains` from a room `Position` with `PanShape` (width, centre, LFE send) |
| `multi` | `MultiBuf`, `MultiFx` (an effect chain per channel pair, centre alone, LFE left out), `MultiLimiter` (linked look-ahead true peak over N channels), `LfeFilter` |
| `dialogue` | phrase leveling (`phrases`, `level_keys`), voice fingerprints and matching (`profile`, `match_voice`: five-band EQ, level, room) |
| `align` | `offset` (envelope cross-correlation), `warp_map` (DTW over band energies), `conform` (piecewise WSOLA), `fit` (one stretch within limits): ADR and dub sync |
| `wav`, `adm` | delivery WAV / BWF writer and reader (16/24-bit, float, extensible with speaker masks, `bext`), ADM BWF (ITU-R BS.2076 `axml` + `chna`) with a bed and moving objects |

## For `render`: mixing a sequence for export

Build a `MixPlan` from the model, one `ClipSpec` per audio-bearing clip and
one `LaneSpec` per audio lane, then either run the `Mixer` block by block or
call `mix_offline`.

```rust
use studio_sound::*;

// One source per clip: decode (or memory-map) the clip's audio at its own
// rate. `MemorySource` is the in-memory one; `render` implements
// `AudioSource` over its decoder cache so long clips stream.
struct Decoded { /* ... */ }
impl AudioSource for Decoded {
    fn rate(&self) -> u32 { 48_000 }
    fn channels(&self) -> usize { 2 }                 // 1 = mono content (constant-power pan)
    fn frames(&self) -> Option<u64> { None }
    fn read(&mut self, frame: u64, l: &mut [f32], r: &mut [f32]) -> usize { /* ... */ 0 }
}

let mut plan = MixPlan::default();
plan.lanes.push(LaneSpec { role: LaneRole::Dialogue, ..LaneSpec::new(lane_id, LaneRole::Dialogue) });

let mut clip = ClipSpec::new(clip_id, lane_id, start_us, duration_us, Box::new(source));
clip.source_in = source_in_us;                        // for reversed clips: where playback begins
clip.speed = SpeedSpec::Constant(2.0);                // or SpeedSpec::Varying(Curve) in clip time
clip.preserve_pitch = true;                           // WSOLA; false = varispeed
clip.volume_db = Curve::new(keys, 0.0);               // keys in clip-relative µs, values in dB
clip.pan = Curve::constant(-0.3);
clip.fade_in = 500_000; clip.fade_shape = FadeShape::EqualPower;
clip.gain_offset_db = normalise_gain_db(&report, LoudnessTarget::Streaming.lufs(), f64::INFINITY);
clip.effects = vec![EffectSpec::Compressor(dynamics::CompressorParams::VOICE), EffectSpec::Voice { style: VoiceStyle::Newsroom, amount: 1.0 }];
plan.clips.push(clip);

plan.master.limiter = Some(dynamics::LimiterParams { ceiling_db: -1.0, ..Default::default() });
plan.master.ducking = Some(ducking::DuckingParams::default());   // music lanes under dialogue lanes

// Whole export in one call (drops the master latency, so frame 0 is `start`):
let (left, right) = mix_offline(plan, 48_000, 0, end_us, Quality::Best);
```

Streaming instead of one call, e.g. to interleave with video encoding:

```rust
let mut mixer = Mixer::new(48_000, 1024, Quality::Best, plan);
let latency = mixer.master_latency();   // limiter look-ahead etc., in frames
mixer.seek(0);
let mut block = StereoBuf::new(1024);
// render `latency` extra frames and discard them from the front, then:
mixer.render(&mut block);               // block.left() / block.right()
let report = mixer.loudness().report(); // integrated LUFS, range, true peak of the export
```

`Mixer::end()` is the time after which the output is silent (clip ends plus
effect tails). For an export-level "normalise to −14 / −16 / −23 LUFS",
render once, read `report.integrated_lufs`, and apply
`normalise_gain_db(&report, target.lufs(), target.ceiling_dbtp())` to
`plan.master.volume_db` for the real pass (or leave the limiter to catch
peaks by passing `f64::INFINITY` as the ceiling).

### Mapping the model

- `AudioProps.volume_db` and its `audio.volume_db` keyframe track →
  `ClipSpec::volume_db` (`Curve::new`, times relative to the clip start;
  `Key::eased` with `Ease::{Linear,Hold,In,Out,InOut}`). Same for pan.
- `Fade { duration, curve }` → `fade_in`/`fade_out` + `FadeShape`:
  `Linear→Linear`, `EqualPower→EqualPower`, `SCurve→SCurve`,
  `Exponential→Slow`, `Logarithmic→Fast`.
- `Clip.speed` → `SpeedSpec` (`Constant` for a plain factor, `Varying` for
  speed curves, sampled from the model's `rate_at`); `Clip.reversed` →
  `reversed`, with `source_in` set to the source time of the clip's first
  frame (the out point for a reversed clip).
- `Lane.muted/solo/volume_db/pan` → `LaneSpec`; `Lane.kind == Audio` lanes
  and visual lanes whose clips carry audio both become lanes here. The
  lane role (dialogue / music / other) comes from the user's lane setting.
- Model audio effects (an id plus parameters) → `EffectSpec` via
  `EffectSpec::kind_id()`; each variant's params struct lists ranges and
  defaults in its docs.

## For `app`: live monitoring and the inspector

**Playback.** Keep one `Mixer` behind the lock the audio thread renders
under. The audio callback (or the mix-ahead thread feeding
`studio_audio::AudioOut`) calls `render_interleaved` for each block;
`seek` on playhead jumps; `meters()` after each block for the UI. On edits,
build a new `MixPlan` and call `Mixer::update(plan)`: engines whose
structure (source rate, speed, direction, effects) did not change keep
their state, so a volume tweak during playback does not cut a reverb tail.
`update` may allocate, so it is a control-thread call made under the same
lock. Compensate `master_latency()` when lining audio up with the picture.

```rust
let mut mixer = Mixer::new(rate, 512, Quality::High, plan);
mixer.seek(playhead_us);
// audio thread:
mixer.render_interleaved(&mut out);           // out.len() / 2 frames
let m = mixer.meters();                       // lane_peaks (plan order), master_peak,
                                              // limiter_reduction_db, momentary_lufs,
                                              // ducking_db, speech
```

**Inspector.** `ParametricEq::response_db(freq)` draws the EQ curve;
`Compressor::gain_reduction_db` etc. read the meters of a standalone unit.
`VoiceStyle::ALL`, `name()`, `description()`, `group()` fill the voice
picker; `VoiceStyle::from_id` restores a saved choice.

**Loudness.** `measure_source(&mut source)` gives a clip's `LoudnessReport`
for the readout and for "Normalise": store the resulting
`normalise_gain_db` in the clip (`gain_offset_db`). `LoudnessTarget::ALL`
lists the menu entries with labels.

**Waveforms.** `PeakCache::build_stereo(l, r, rate, 256)` once per media
item (or `new` + `append` while importing), then
`query(ch, start_frame, end_frame, &mut peaks)` with one `Peak` per pixel at
any zoom; `memory_bytes()` for the cache budget.

**Beats.** `beats::analyse(l, r, rate)` → `BeatAnalysis { bpm, beats,
onsets, downbeats, confidence }`; `grid()` for snapping and
`snap_to(markers, t, tolerance)` for marker snapping. Onset detection and
the tempo estimate come from `studio_audio::beats` (re-exported).

**Voice-over.** Open the device with `studio_audio::AudioIn`, make a
`VoiceOverRecorder::new(rate, RecorderParams { count_in_beats: 3, start:
playhead, .. })`, call `start()`, then each tick `DeviceFeed::pump` (or
`feed(samples)`), read `state()` for the 3-2-1 overlay and `meter()` for the
input meter, mix `render_clicks(out)` into the monitor output during the
count-in, and `stop()` returns a `Take { samples, rate, start, duration }`
to drop on a new lane. `peaks()` is a growing waveform for the lane while
recording.

**Ducking preview.** `vad::speech_segments(mono, rate, VadParams::default())`
marks speech regions on a dialogue clip offline.

## Design notes

- The resampler is a Kaiser-windowed sinc at 256 phases with linear
  interpolation; the cut-off follows the lower Nyquist, so downsampling
  rejects aliasing. Same-rate clips at speed 1 bypass it and are bit-exact.
- Speed is split between WSOLA (0.1×–10×, keeps pitch) and varispeed
  (resampler ratio) so any speed in 0.1×–100× works; with pitch
  preservation off, speeds above 32× also go through the stretcher.
- Latency from clip effects (pitch shift) is compensated by priming the
  chain from before the playhead on every (re)start, so clips stay on
  their frame. Lane and master latency is reported, not hidden.
- Loudness follows ITU-R BS.1770-4 / EBU R 128: K-weighting (the published
  48 kHz coefficients fall out of the analogue prototype), 400 ms blocks at
  75 % overlap, −70 LUFS absolute and −10 LU relative gates, histogram
  based so the meter never allocates. `loudness::test_vectors` generates
  compliance-style signals at any rate; the tests check them.
- The compressor follows the feed-forward log-domain design (Giannoulis,
  Massberg & Reiss, JAES 2012); the limiter is look-ahead with a sliding
  minimum and 4× oversampled true peak; the reverb is a Dattorro plate.

## Tests

`cargo test -p studio-sound` (93 tests): filters against analytic
responses, dynamics against steady tones and bursts, resampling against
sines and aliasing probes, WSOLA pitch and length, loudness against
generated BS.1770 vectors and the published K-weighting coefficients, beat
tracking on a synthetic drum pattern, the mixer against sample-exact
expectations (keyframes landing on their sample, fades, mute/solo, pan law,
block-size independence, seeking, other sample rates, speed with and
without pitch preservation, reverse, latency alignment, ducking, limiter,
loudness of a mix, in-place updates, buses with gain, mute and solo,
pre- and post-fader sends, pan automation, stems summing to the mix,
surround placement of dialogue, music and ambience, bus latency
compensation), the panner's power and speaker positions, the linked
limiter, leveling and voice matching, ADR offset and word-by-word warp,
WAV and ADM structure, and an allocation count of zero over a demanding
render.
