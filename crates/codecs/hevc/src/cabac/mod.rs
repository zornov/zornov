// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! CABAC: arithmetic decoding engine, context tables and syntax element decoding.

pub mod engine;
#[cfg(any(test, feature = "synth"))]
pub mod specenc;
pub mod syntax;
pub mod tables;

pub use engine::{CabacEngine, Ctx};
pub use tables::NUM_CTX;

use crate::error::{Error, Result};

/// Bin source used in bitstream *synthesis* mode (feature `synth`): every bin
/// the parser asks for is chosen pseudo-randomly and simultaneously encoded
/// with the reference CABAC encoder, producing a valid bitstream that decodes
/// to exactly the same syntax.
#[cfg(feature = "synth")]
pub struct SynthBins {
    pub enc: specenc::SpecEncoder,
    pub rng: u64,
    /// Probability (in 1/256) of a context-coded bin being 1.
    pub p_bin: u32,
    /// Probability (in 1/256) of a bypass bin being 1.
    pub p_bypass: u32,
    /// Probability (in 1/256) of choosing pcm_flag = 1 when allowed.
    pub p_pcm: u32,
    /// CTUs remaining in the current slice segment.
    pub remaining_ctus: usize,
    /// Bit offsets (in `enc.bits`) where each substream after the first starts.
    pub substream_starts: Vec<usize>,
}

#[cfg(feature = "synth")]
impl SynthBins {
    pub fn new(seed: u64, ctus: usize) -> SynthBins {
        SynthBins {
            enc: specenc::SpecEncoder::new(),
            rng: seed.wrapping_mul(0x9E3779B97F4A7C15) | 1,
            p_bin: 128,
            p_bypass: 100,
            p_pcm: 80,
            remaining_ctus: ctus,
            substream_starts: Vec::new(),
        }
    }
    pub fn rand(&mut self) -> u32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 24) as u32
    }
    fn chance(&mut self, p256: u32) -> u32 {
        ((self.rand() & 255) < p256) as u32
    }
}

/// Arithmetic decoder plus the context variables of the current slice segment.
pub struct Cabac<'a> {
    pub e: CabacEngine<'a>,
    pub ctx: [Ctx; NUM_CTX],
    data: &'a [u8],
    raw_bit_pos: usize,
    #[cfg(feature = "synth")]
    pub synth: Option<Box<SynthBins>>,
}

impl<'a> Cabac<'a> {
    pub fn new(data: &'a [u8], start: usize, slice_qp: i32, init_type: usize) -> Self {
        Cabac {
            e: CabacEngine::new(data, start),
            ctx: engine::init_contexts(slice_qp, init_type),
            data,
            raw_bit_pos: 0,
            #[cfg(feature = "synth")]
            synth: None,
        }
    }

    #[inline(always)]
    pub fn bin(&mut self, idx: usize) -> u32 {
        #[cfg(feature = "synth")]
        if let Some(s) = self.synth.as_mut() {
            let b = s.chance(s.p_bin);
            s.enc.encode(&mut self.ctx[idx], b);
            return b;
        }
        self.e.decode_bin(&mut self.ctx[idx])
    }

    #[inline(always)]
    pub fn bypass(&mut self) -> u32 {
        #[cfg(feature = "synth")]
        if let Some(s) = self.synth.as_mut() {
            let b = s.chance(s.p_bypass);
            s.enc.encode_bypass(b);
            return b;
        }
        self.e.decode_bypass()
    }

    #[inline(always)]
    pub fn bypass_bits(&mut self, n: u32) -> u32 {
        #[cfg(feature = "synth")]
        if self.synth.is_some() {
            let mut v = 0;
            for _ in 0..n {
                v = (v << 1) | self.bypass();
            }
            return v;
        }
        self.e.decode_bypass_bits(n)
    }

    #[cfg(feature = "synth")]
    fn synth_terminate(&mut self, v: u32) -> u32 {
        let s = self.synth.as_mut().expect("synth");
        s.enc.encode_terminate(v);
        if v == 1 {
            s.enc.align_zero();
        }
        v
    }

    /// pcm_flag (terminating bin).
    pub fn pcm_flag(&mut self) -> bool {
        #[cfg(feature = "synth")]
        if let Some(s) = self.synth.as_mut() {
            let v = s.chance(s.p_pcm);
            return self.synth_terminate(v) != 0;
        }
        self.e.decode_terminate() != 0
    }

    /// end_of_slice_segment_flag (terminating bin).
    pub fn end_of_slice_segment_flag(&mut self) -> bool {
        #[cfg(feature = "synth")]
        if let Some(s) = self.synth.as_mut() {
            s.remaining_ctus = s.remaining_ctus.saturating_sub(1);
            let v = (s.remaining_ctus == 0) as u32;
            return self.synth_terminate(v) != 0;
        }
        self.e.decode_terminate() != 0
    }

    /// end_of_subset_one_bit followed by byte_alignment() and re-initialisation
    /// of the arithmetic decoder (9.3.2.5).
    pub fn end_of_subset(&mut self) -> Result<()> {
        #[cfg(feature = "synth")]
        if self.synth.is_some() {
            self.synth_terminate(1);
            let s = self.synth.as_mut().expect("synth");
            s.enc.restart();
            let pos = s.enc.bits.len();
            s.substream_starts.push(pos);
            return Ok(());
        }
        if self.e.decode_terminate() != 1 {
            return Err(Error::Invalid("end_of_subset_one_bit"));
        }
        let pos = self.e.aligned_byte_after_terminate();
        if pos >= self.data.len() {
            return Err(Error::UnexpectedEof);
        }
        self.e.reinit(pos);
        Ok(())
    }

    /// end_of_subset_one_bit followed by byte_alignment(): returns the byte
    /// position of the next substream without re-initialising (WPP rows
    /// decoded in parallel start at their entry points).
    pub fn end_of_subset_pos(&mut self) -> Result<usize> {
        if self.e.decode_terminate() != 1 {
            return Err(Error::Invalid("end_of_subset_one_bit"));
        }
        Ok(self.e.aligned_byte_after_terminate())
    }

    /// Starts reading raw PCM sample bits after pcm_flag == 1.
    pub fn pcm_begin(&mut self) -> Result<()> {
        #[cfg(feature = "synth")]
        if self.synth.is_some() {
            return Ok(());
        }
        let pos = self.e.aligned_byte_after_terminate();
        if pos >= self.data.len() {
            return Err(Error::UnexpectedEof);
        }
        self.raw_bit_pos = pos * 8;
        Ok(())
    }

    /// Reads `n` (<= 16) raw PCM bits.
    pub fn pcm_bits(&mut self, n: u32) -> Result<u32> {
        #[cfg(feature = "synth")]
        if let Some(s) = self.synth.as_mut() {
            let v = s.rand() & ((1u32 << n) - 1);
            s.enc.put_raw(v, n);
            return Ok(v);
        }
        let mut v = 0u32;
        for _ in 0..n {
            let byte = *self.data.get(self.raw_bit_pos >> 3).ok_or(Error::UnexpectedEof)?;
            let bit = (byte >> (7 - (self.raw_bit_pos & 7))) & 1;
            v = (v << 1) | bit as u32;
            self.raw_bit_pos += 1;
        }
        Ok(v)
    }

    /// Re-initialises the arithmetic decoder after the PCM samples.
    pub fn pcm_end(&mut self) {
        #[cfg(feature = "synth")]
        if let Some(s) = self.synth.as_mut() {
            s.enc.align_zero();
            s.enc.restart();
            return;
        }
        self.e.reinit(self.raw_bit_pos.div_ceil(8));
    }

    /// True if the decoder has read well past the end of the data.
    pub fn overrun(&self) -> bool {
        #[cfg(feature = "synth")]
        if self.synth.is_some() {
            return false;
        }
        self.e.overrun() > 4
    }
}
