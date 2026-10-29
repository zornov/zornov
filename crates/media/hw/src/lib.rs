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

/// An AV1 decoder on the video hardware: 8-bit 4:2:0 pictures out.
pub struct Av1Decoder {
    #[cfg(target_os = "macos")]
    inner: vt_dec::Decoder,
}

impl Av1Decoder {
    /// Whether this machine's video hardware decodes AV1 at all.
    pub fn supported() -> bool {
        #[cfg(target_os = "macos")]
        {
            vt_dec::Decoder::av1_supported()
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    /// From the stream's `av1C` record and coded size. `Err` says why there
    /// is no hardware decoder (the caller keeps Studio's own): 10-bit and
    /// 12-bit streams are refused because the pictures leave as 8-bit, as
    /// are monochrome and non-4:2:0 streams.
    pub fn new(av1c: &[u8], width: u32, height: u32) -> Result<Av1Decoder, String> {
        let Some(&b2) = av1c.get(2) else { return Err("av1C record too short".into()) };
        if b2 & 0x40 != 0 {
            return Err(format!("{}-bit AV1 is not decoded on the video hardware (8-bit only)", if b2 & 0x20 != 0 { 12 } else { 10 }));
        }
        if b2 & 0x10 != 0 || (b2 >> 3) & 1 != 1 || (b2 >> 2) & 1 != 1 {
            return Err("only 4:2:0 AV1 is decoded on the video hardware".into());
        }
        #[cfg(target_os = "macos")]
        {
            vt_dec::Decoder::new_av1(av1c, width, height).map(|inner| Av1Decoder { inner }).ok_or_else(|| "no AV1 video hardware".into())
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (width, height);
            Err("no AV1 video hardware".into())
        }
    }

    /// One temporal unit (no temporal delimiter) in; the pictures that are ready out.
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

    /// Drops what is inside, keeping the session, before decoding on from
    /// a sync sample elsewhere in the stream.
    pub fn reset(&mut self) {
        #[cfg(target_os = "macos")]
        self.inner.reset();
    }
}

#[cfg(test)]
#[path = "../../encode/tests/data/svtav1_160x96_15f.rs"]
mod av1_stream;

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

    /// Splits a low-overhead OBU stream into temporal units without their
    /// temporal delimiters (the way MP4 stores them).
    fn av1_samples(stream: &[u8]) -> Vec<Vec<u8>> {
        let mut units: Vec<Vec<u8>> = Vec::new();
        let mut pos = 0;
        while pos < stream.len() {
            let h = stream[pos];
            let hdr = 1 + (h & 4 != 0) as usize;
            let (mut size, mut n) = (0usize, 0);
            for i in 0..8 {
                let b = stream[pos + hdr + i];
                size |= ((b & 0x7f) as usize) << (i * 7);
                n += 1;
                if b & 0x80 == 0 {
                    break;
                }
            }
            let end = pos + hdr + n + size;
            match (h >> 3) & 0xf {
                2 => units.push(Vec::new()),
                15 => {}
                _ => units.last_mut().expect("a temporal delimiter first").extend_from_slice(&stream[pos..end]),
            }
            pos = end;
        }
        units
    }

    fn av1c_of(sample0: &[u8]) -> Vec<u8> {
        let seq_len = 2 + sample0[1] as usize;
        assert_eq!(sample0[0] >> 3 & 0xf, 1, "first unit starts with the sequence header");
        studio_encode::mux::av1c_from_sequence_header(&sample0[..seq_len]).unwrap().av1c
    }

    fn psnr(a: &[u8], b: &[u8]) -> f64 {
        let se: f64 = a.iter().zip(b).map(|(&x, &y)| (x as f64 - y as f64).powi(2)).sum();
        if se == 0.0 { f64::INFINITY } else { 10.0 * (255.0f64 * 255.0 * a.len() as f64 / se).log10() }
    }

    #[test]
    fn av1_hardware_matches_software() {
        let units = av1_samples(crate::av1_stream::STREAM);
        assert_eq!(units.len(), 15);
        let av1c = av1c_of(&units[0]);
        if !Av1Decoder::supported() {
            eprintln!("no AV1 hardware here; skipped");
            return;
        }
        let mut hw = Av1Decoder::new(&av1c, 160, 96).expect("a hardware AV1 decoder");
        let mut got = Vec::new();
        for (i, u) in units.iter().enumerate() {
            got.extend(hw.decode(u, i as i64).unwrap());
        }
        got.extend(hw.flush());
        assert_eq!(got.len(), 15, "every picture comes back");
        got.sort_by_key(|d| d.sample);

        let mut sw = studio_av1::Decoder::new();
        let mut want = Vec::new();
        for (i, u) in units.iter().enumerate() {
            want.extend(sw.decode_pts(u, i as i64).unwrap());
        }
        want.extend(sw.flush());
        assert_eq!(want.len(), 15);
        want.sort_by_key(|p| p.pts);

        let to8 = |p: &Vec<u16>| p.iter().map(|&v| v as u8).collect::<Vec<u8>>();
        let mut worst = f64::INFINITY;
        let mut exact = true;
        for (h, s) in got.iter().zip(&want) {
            assert_eq!((h.width, h.height), (s.width, s.height));
            assert_eq!(h.y.len(), 160 * 96);
            assert_eq!(h.u.len(), 80 * 48);
            for (a, b) in [(&h.y, &s.planes[0]), (&h.u, &s.planes[1]), (&h.v, &s.planes[2])] {
                let b = to8(b);
                exact &= *a == b;
                worst = worst.min(psnr(a, &b));
            }
        }
        eprintln!("AV1 hardware vs software, 15 frames: exact = {exact}, worst plane PSNR = {worst:.2} dB");
        assert!(worst > 40.0, "hardware and software decodes agree (worst PSNR {worst:.2} dB)");
    }

    #[test]
    fn av1_ten_bit_is_refused() {
        // av1C: profile 0, level 4, high_bitdepth, 4:2:0.
        let err = Av1Decoder::new(&[0x81, 0x04, 0x4c, 0x00], 160, 96).err().expect("refused");
        assert!(err.contains("10-bit"), "{err}");
        let err = Av1Decoder::new(&[0x81, 0x04, 0x6c, 0x00], 160, 96).err().expect("refused");
        assert!(err.contains("12-bit"), "{err}");
    }
}
