// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Hardware video codecs through the operating system: the media engine
//! in the machine does the work, so the CPU is left to decode and render.
//! On macOS that is VideoToolbox, bound here by hand. Elsewhere (for now)
//! nothing is available and callers keep using Studio's own codecs.

// On every platform: macOS speaks, the others say they have no system voices.
pub mod speech;
#[cfg(target_os = "macos")]
mod vt;
#[cfg(target_os = "macos")]
mod vt_dec;

/// One decoded picture: 8-bit 4:2:0 planes (video range), and the sample
/// number it came from (pictures may come out of decode order).
pub struct Decoded {
    pub sample: i64,
    pub width: u32,
    pub height: u32,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
    pub chroma_width: u32,
    pub chroma_height: u32,
}

/// An H.264 decoder on the video hardware.
pub struct H264Decoder {
    #[cfg(target_os = "macos")]
    inner: vt_dec::Decoder,
}

impl H264Decoder {
    /// From the stream's avcC record; `None` when there is no hardware for
    /// it (the caller keeps Studio's own decoder).
    pub fn new(avcc: &[u8]) -> Option<H264Decoder> {
        #[cfg(target_os = "macos")]
        {
            vt_dec::Decoder::new(avcc).map(|inner| H264Decoder { inner })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = avcc;
            None
        }
    }

    /// One AVCC sample in; the pictures that are ready out.
    pub fn decode(&mut self, data: &[u8], sample: i64) -> Result<Vec<Decoded>, String> {
        #[cfg(target_os = "macos")]
        {
            self.inner.decode(data, sample)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (data, sample);
            Err("no hardware decoder".into())
        }
    }

    /// The pictures still inside.
    pub fn flush(&mut self) -> Vec<Decoded> {
        #[cfg(target_os = "macos")]
        {
            self.inner.flush()
        }
        #[cfg(not(target_os = "macos"))]
        {
            Vec::new()
        }
    }

    /// Drops what is inside, keeping the (warm) session, before decoding on
    /// from a sync sample elsewhere in the stream.
    pub fn reset(&mut self) {
        #[cfg(target_os = "macos")]
        self.inner.reset();
    }
}

/// An HEVC decoder on the video hardware (8-bit pictures out, whatever
/// the stream's depth: enough for the preview).
pub struct HevcDecoder {
    #[cfg(target_os = "macos")]
    inner: vt_dec::Decoder,
}

impl HevcDecoder {
    /// From the stream's VPS / SPS / PPS and NAL length size; `None` when
    /// there is no hardware for it.
    pub fn new(parameter_sets: &[Vec<u8>], nal_length_size: usize) -> Option<HevcDecoder> {
        #[cfg(target_os = "macos")]
        {
            vt_dec::Decoder::new_hevc(parameter_sets, nal_length_size as i32).map(|inner| HevcDecoder { inner })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (parameter_sets, nal_length_size);
            None
        }
    }

    /// One length-prefixed sample in; the pictures that are ready out.
    pub fn decode(&mut self, data: &[u8], sample: i64) -> Result<Vec<Decoded>, String> {
        #[cfg(target_os = "macos")]
        {
            self.inner.decode(data, sample)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (data, sample);
            Err("no hardware decoder".into())
        }
    }

    /// The pictures still inside.
    pub fn flush(&mut self) -> Vec<Decoded> {
        #[cfg(target_os = "macos")]
        {
            self.inner.flush()
        }
        #[cfg(not(target_os = "macos"))]
        {
            Vec::new()
        }
    }

    /// Drops what is inside, keeping the (warm) session, before decoding on
    /// from a sync sample elsewhere in the stream.
    pub fn reset(&mut self) {
        #[cfg(target_os = "macos")]
        self.inner.reset();
    }
}

/// One encoded picture: AVCC-framed NAL units (4-byte lengths).
pub struct Encoded {
    pub data: Vec<u8>,
    pub keyframe: bool,
}

/// An H.264 encoder on the video hardware.
/// A hardware HEVC Main 10 encoder: 10-bit pictures, so smooth gradients
/// and soft shadows keep their steps four times finer than 8-bit H.264.
pub struct HevcEncoder {
    #[cfg(target_os = "macos")]
    inner: vt::Encoder,
}

impl HevcEncoder {
    /// A 10-bit encoder for `width`×`height` 16-bit RGBA pictures, or
    /// `None` when this machine has none.
    pub fn new_main10(width: u32, height: u32, fps_num: u32, fps_den: u32, bitrate_kbps: u32, keyframe_every: u32) -> Option<HevcEncoder> {
        #[cfg(target_os = "macos")]
        {
            vt::Encoder::open(vt::Codec::HevcMain10, width, height, fps_num, fps_den, bitrate_kbps, keyframe_every).map(|inner| HevcEncoder { inner })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (width, height, fps_num, fps_den, bitrate_kbps, keyframe_every);
            None
        }
    }

    /// The hvcC record (VPS, SPS, PPS) for the container.
    pub fn hvcc(&self) -> Vec<u8> {
        #[cfg(target_os = "macos")]
        {
            self.inner.hvcc()
        }
        #[cfg(not(target_os = "macos"))]
        {
            Vec::new()
        }
    }

    /// Hands one 16-bit RGBA picture to the hardware.
    pub fn encode_rgba16(&mut self, rgba: &[u16]) -> Result<Vec<Encoded>, String> {
        #[cfg(target_os = "macos")]
        {
            self.inner.encode_rgba16(rgba)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = rgba;
            Err("no hardware encoder".into())
        }
    }

    /// Everything still in the hardware.
    pub fn finish(&mut self) -> Result<Vec<Encoded>, String> {
        #[cfg(target_os = "macos")]
        {
            self.inner.finish()
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(Vec::new())
        }
    }
}

pub struct H264Encoder {
    #[cfg(target_os = "macos")]
    inner: vt::Encoder,
}

impl H264Encoder {
    /// A hardware encoder for `width`×`height` RGBA pictures, or `None`
    /// when this machine has none (the caller falls back to software).
    pub fn new(width: u32, height: u32, fps_num: u32, fps_den: u32, bitrate_kbps: u32, keyframe_every: u32) -> Option<H264Encoder> {
        #[cfg(target_os = "macos")]
        {
            vt::Encoder::new(width, height, fps_num, fps_den, bitrate_kbps, keyframe_every).map(|inner| H264Encoder { inner })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (width, height, fps_num, fps_den, bitrate_kbps, keyframe_every);
            None
        }
    }

    /// The avcC record (SPS and PPS) for the container.
    pub fn avcc(&self) -> Vec<u8> {
        #[cfg(target_os = "macos")]
        {
            self.inner.avcc()
        }
        #[cfg(not(target_os = "macos"))]
        {
            Vec::new()
        }
    }

    /// Hands one RGBA picture to the hardware; returns the pictures it has
    /// finished, in order (it may hold a few back).
    pub fn encode_rgba(&mut self, rgba: &[u8]) -> Result<Vec<Encoded>, String> {
        #[cfg(target_os = "macos")]
        {
            self.inner.encode_rgba(rgba)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = rgba;
            Err("no hardware encoder".into())
        }
    }

    /// Everything still in the hardware.
    pub fn finish(&mut self) -> Result<Vec<Encoded>, String> {
        #[cfg(target_os = "macos")]
        {
            self.inner.finish()
        }
        #[cfg(not(target_os = "macos"))]
        {
            Ok(Vec::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardware_round_trip() {
        let Some(mut enc) = H264Encoder::new(320, 240, 30, 1, 2000, 10) else {
            eprintln!("no hardware codec here; skipped");
            return;
        };
        let avcc = enc.avcc();
        let mut samples = Vec::new();
        for i in 0..30u32 {
            // A grey that steps up each frame.
            let g = (40 + i * 5) as u8;
            let px: Vec<u8> = (0..320 * 240).flat_map(|_| [g, g, g, 255]).collect();
            samples.extend(enc.encode_rgba(&px).unwrap());
        }
        samples.extend(enc.finish().unwrap());
        assert_eq!(samples.len(), 30);
        let mut dec = H264Decoder::new(&avcc).expect("a hardware decoder for our own stream");
        let mut got = Vec::new();
        for (i, s) in samples.iter().enumerate() {
            got.extend(dec.decode(&s.data, i as i64).unwrap());
        }
        got.extend(dec.flush());
        assert_eq!(got.len(), 30, "every picture comes back");
        got.sort_by_key(|d| d.sample);
        assert_eq!((got[0].width, got[0].height), (320, 240));
        let mean = |d: &Decoded| d.y.iter().map(|&v| v as f64).sum::<f64>() / d.y.len() as f64;
        assert!(mean(&got[29]) > mean(&got[0]) + 50.0, "brightness steps up: {} -> {}", mean(&got[0]), mean(&got[29]));
    }
}
