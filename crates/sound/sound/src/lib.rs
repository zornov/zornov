// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Mixing and audio processing for Studio, one layer above the `audio`
//! crate's device I/O.
//!
//! Everything here works on **f32 planar blocks** (one slice per channel,
//! 256–1024 frames) and is **allocation-free once built**: every processor
//! takes its sample rate and maximum block size in its constructor, sizes its
//! state there, and its `process` never allocates. Build processors on a
//! control thread, run them on the audio or render thread.
//!
//! Modules, roughly in signal order:
//!
//! - [`automation`]: keyframe curves evaluated per sample.
//! - [`resample`]: windowed-sinc rate conversion with selectable quality.
//! - [`eq`]: RBJ biquads and the five-band parametric equaliser.
//! - [`dynamics`]: compressor, true-peak limiter, gate, de-esser.
//! - [`reverb`], [`delay`]: algorithmic plate reverb and stereo delay.
//! - [`pitch`]: WSOLA time stretch and streaming pitch shift.
//! - [`voice`]: the voice-style catalogue built from the units above.
//! - [`vad`], [`ducking`]: speech detection and music ducking.
//! - [`loudness`]: ITU-R BS.1770-4 metering and normalisation.
//! - [`beats`]: onset, tempo and beat grid analysis.
//! - [`dialogue`]: phrase leveling and voice matching between takes.
//! - [`align`]: lining ADR and dubbed lines up with a guide (offset, warp).
//! - [`wav`], [`adm`]: delivery WAV / BWF files, and ADM BWF objects.
//! - [`peaks`]: multi-resolution waveform peak caches.
//! - [`recorder`]: voice-over recorder logic with count-in and meters.
//! - [`surround`], [`multi`]: speaker layouts, the surround panner,
//!   multichannel buffers, per-pair effects and the linked limiter.
//! - [`mixer`]: the clip → lane → bus → master graph that ties it all together.
//!
//! Times on the timeline are [`Micros`] (microseconds, `i64`), the same unit
//! the project model uses; inside the DSP everything is in frames at the
//! mixer's sample rate.

// No `unsafe` in the crate. Test builds relax this to `deny` for one
// test-only module: a counting allocator that proves the render path never
// allocates.
#![cfg_attr(not(test), forbid(unsafe_code))]
#![cfg_attr(test, deny(unsafe_code))]
#![warn(missing_docs)]

pub mod adm;
pub mod align;
pub mod automation;
pub mod beats;
pub mod buffer;
pub mod delay;
pub mod dialogue;
pub mod ducking;
pub mod dynamics;
pub mod eq;
pub mod fft;
pub mod loudness;
pub mod mixer;
pub mod multi;
pub mod peaks;
pub mod pitch;
pub mod recorder;
pub mod resample;
pub mod reverb;
pub mod surround;
pub mod vad;
pub mod voice;
pub mod wav;

pub use automation::{Curve, Ease, FadeShape, Fades, Key};
pub use buffer::{Effect, StereoBuf, db_to_gain, gain_to_db};
pub use loudness::{LoudnessMeter, LoudnessReport, LoudnessTarget, measure, normalise_gain_db};
pub use mixer::{AudioSource, BusKind, BusSpec, ClipSpec, EffectSpec, LaneRole, LaneSpec, MasterSpec, MemorySource, Meters, MixPlan, Mixer, SendSpec, SpeedSpec, SurroundSpec, measure_source, mix_offline, mix_offline_layout};
pub use multi::MultiBuf;
pub use surround::{Layout, PanShape, Position};
pub use resample::{Quality, Resampler};
pub use voice::{VoiceChain, VoiceGroup, VoiceStyle};


/// Microseconds on the timeline, the project model's time unit.
pub type Micros = i64;

/// One second in [`Micros`].
pub const SECOND: Micros = 1_000_000;

/// Frames at `rate` for a timeline duration, rounded to nearest.
pub fn micros_to_frames(t: Micros, rate: u32) -> i64 {
    ((t as i128 * rate as i128 + SECOND as i128 / 2).div_euclid(SECOND as i128)) as i64
}

/// Timeline time of a frame index at `rate`, rounded to nearest microsecond.
pub fn frames_to_micros(frames: i64, rate: u32) -> Micros {
    let r = rate.max(1) as i128;
    ((frames as i128 * SECOND as i128 + r / 2).div_euclid(r)) as Micros
}

/// Counts this thread's heap allocations in test builds, so tests can prove
/// the render path never allocates (tests run in parallel, so a global count
/// would see other tests' work).
#[cfg(test)]
#[allow(unsafe_code)]
pub(crate) mod alloc_count {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    pub struct Counting;

    thread_local! {
        static COUNT: Cell<usize> = const { Cell::new(0) };
    }

    fn bump() {
        // `try_with` fails only while the thread is being torn down.
        let _ = COUNT.try_with(|c| c.set(c.get() + 1));
    }

    // SAFETY: every call is delegated unchanged to the system allocator; the
    // counter is a plain thread-local cell with a const initialiser, so
    // touching it never allocates or re-enters this allocator.
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            bump();
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            unsafe { System.dealloc(ptr, layout) }
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            bump();
            unsafe { System.realloc(ptr, layout, new_size) }
        }
    }

    #[global_allocator]
    static GLOBAL: Counting = Counting;

    /// Allocations made so far on the calling thread.
    pub fn count() -> usize {
        COUNT.with(|c| c.get())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_conversions_round_trip() {
        for t in [0, 1, 999_999, SECOND, 123_456_789, -SECOND] {
            let f = micros_to_frames(t, 48_000);
            let back = frames_to_micros(f, 48_000);
            assert!((back - t).abs() <= 11, "{t} -> {f} -> {back}");
        }
        assert_eq!(micros_to_frames(SECOND, 44_100), 44_100);
        assert_eq!(frames_to_micros(48_000, 48_000), SECOND);
    }
}
