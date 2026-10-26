Extra AAC test streams for studio-aac, produced on macOS with Apple's
encoder (`afconvert -d aac|aach|aacp`) from synthetic 1-second signals
(log sweep + tones + clicks, noise on the right channel). Each `.pcm` is
Apple's own decode (`afconvert -d LEF32`), interleaved f32le, priming
trimmed when the container signals it (ADTS: untrimmed).

| file | content |
|---|---|
| lc48_256.m4a | LC stereo 48 kHz, 256 kb/s |
| lc8_mono.m4a | LC mono 8 kHz, 16 kb/s |
| lc32.adts | LC stereo 32 kHz in ADTS, 96 kb/s |
| he48.m4a | HE-AAC v1 stereo 48 kHz (24 kHz core) |
| he32m.m4a | HE-AAC v1 mono 32 kHz (16 kHz core) |
| he22m.m4a | HE-AAC v1 mono 22.05 kHz (11.025 kHz core) |
| hev2_48.m4a | HE-AAC v2 48 kHz (24 kHz mono core + PS) |
| hev2_44_40k.m4a | HE-AAC v2 44.1 kHz, 40 kb/s |
| ff_kbd44.m4a | LC stereo 44.1 kHz from FFmpeg's encoder (KBD windows, M/S, intensity), 128 kb/s |
| ff_pns44.m4a | LC stereo 44.1 kHz from FFmpeg's encoder with PNS (noise substitution), 64 kb/s |

The two `ff_*` streams come from FFmpeg's AAC *encoder* (test material only:
Apple's encoder never emits KBD windows or PNS); their references are still
Apple's decode. PNS noise is random by definition, so `ff_pns44` is compared
by spectral band energies rather than waveform SNR.
