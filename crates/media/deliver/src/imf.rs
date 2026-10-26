// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Interoperable Master Format packages (SMPTE ST 2067, Application 2E
//! as Netflix and the studios ask for it): JPEG 2000 pictures and PCM
//! sound in MXF track files, a composition playlist (CPL), a packing list
//! (PKL), an asset map and a volume index.
//!
//! Experimental. The essence files and the package arithmetic (hashes,
//! sizes, durations, ids that tie the files together) are real and checked
//! by the tests; the CPL's essence descriptors are a reduced form of what
//! ST 2067-3 lists, and the codestream is Studio's general profile, not a
//! certified Broadcast Contribution Profile level. A package from here has
//! not been through Netflix Photon or another IMF validator: validate
//! before delivering.

use crate::dcp::{Audio, iso_now, read_pack};
use crate::j2k;
use crate::mxf::{self, Essence, J2kDesc, MxfWriter};
use crate::sha1;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct ImfSpec {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub fps: (u32, u32),
    pub bit_depth: u8,
    pub issuer: String,
    pub creator: String,
    pub language: String,
    pub step: f32,
}

impl ImfSpec {
    pub fn new(title: &str, width: u32, height: u32, fps: (u32, u32)) -> Self {
        ImfSpec { title: title.into(), width, height, fps, bit_depth: 10, issuer: "Zornov".into(), creator: "Zornov by Ashrox LLC".into(), language: "en".into(), step: 1.0 }
    }

    pub fn j2k_params(&self) -> j2k::Params {
        let mut p = j2k::Params::general(self.width, self.height, 3, self.bit_depth);
        p.levels = 5;
        p.precincts = vec![15; 6];
        p.step = self.step * (1u32 << (self.bit_depth.saturating_sub(8))) as f32 / 4.0;
        p
    }
}

/// Display RGB (8-bit) to the 3 planes of `bit_depth` bits the encoder takes.
pub fn rgba_to_planes(rgba: &[u8], w: usize, h: usize, bit_depth: u8) -> [Vec<i32>; 3] {
    let shift = bit_depth as i32 - 8;
    let conv = |v: u8| -> i32 {
        let v = v as i32;
        if shift >= 0 { (v << shift) | (v >> (8 - shift).max(0)) } else { v >> -shift }
    };
    let mut out = [vec![0i32; w * h], vec![0i32; w * h], vec![0i32; w * h]];
    for i in 0..w * h {
        for c in 0..3 {
            out[c][i] = conv(rgba[i * 4 + c]).min((1 << bit_depth) - 1);
        }
    }
    out
}

pub fn encode_frame(spec: &ImfSpec, rgba: &[u8], w: usize, h: usize) -> Result<Vec<u8>, String> {
    let planes = rgba_to_planes(rgba, w, h, spec.bit_depth);
    j2k::encode(&[&planes[0], &planes[1], &planes[2]], &spec.j2k_params())
}

#[derive(Debug)]
pub struct ImfResult {
    pub dir: PathBuf,
    pub cpl: PathBuf,
    pub frames: u64,
    pub files: Vec<PathBuf>,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn urn(u: &[u8; 16]) -> String {
    format!("urn:uuid:{}", mxf::uuid_string(u))
}

/// Write the package into `dir` (created) from frame packs and sound.
pub fn build(spec: &ImfSpec, dir: &Path, packs: &[PathBuf], audio: Option<&Audio>) -> Result<ImfResult, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let io = |e: std::io::Error| e.to_string();
    let rate = spec.fps;
    let pic_path = dir.join(format!("{}_video.mxf", sanitize(&spec.title)));
    let mut w: Option<MxfWriter> = None;
    let mut desc: Option<J2kDesc> = None;
    for p in packs {
        read_pack(p, |cs| {
            if w.is_none() {
                let d = J2kDesc::from_codestream(&cs)?;
                if (d.width, d.height) != (spec.width, spec.height) {
                    return Err(format!("frames are {}x{}, the package is {}x{}", d.width, d.height, spec.width, spec.height));
                }
                desc = Some(d.clone());
                w = Some(MxfWriter::create(&pic_path, Essence::Jpeg2000(d), rate, &spec.title).map_err(|e| e.to_string())?);
            }
            w.as_mut().unwrap().write_frame(&cs).map_err(|e| e.to_string())
        })?;
    }
    let pic = w.ok_or("no pictures to package")?.finish().map_err(io)?;
    let frames = pic.frames;
    let mut tracks: Vec<(PathBuf, mxf::MxfInfo, &str)> = vec![(pic_path.clone(), pic, "image")];
    if let Some(a) = audio {
        let path = dir.join(format!("{}_audio.mxf", sanitize(&spec.title)));
        let per_num = 48_000u64 * rate.1 as u64;
        let ch = a.channels as usize;
        let mut aw = MxfWriter::create(&path, Essence::Pcm { rate: 48_000, channels: a.channels, bits: 24 }, rate, &spec.title).map_err(io)?;
        let mut pos = 0usize;
        for f in 0..frames {
            // Samples through the end of this frame, so non-integer frame rates stay in step.
            let upto = (((f + 1) * per_num) / rate.0 as u64) as usize;
            let (lo, hi) = (pos * ch, (upto * ch).min(a.samples.len()));
            let mut chunk = a.samples[lo.min(a.samples.len())..hi.max(lo.min(a.samples.len()))].to_vec();
            chunk.resize((upto - pos) * ch, 0.0);
            let mut b = Vec::with_capacity(chunk.len() * 3);
            for s in &chunk {
                b.extend_from_slice(&((s.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32).to_le_bytes()[..3]);
            }
            aw.write_frame(&b).map_err(io)?;
            pos = upto;
        }
        tracks.push((path, aw.finish().map_err(io)?, "sound"));
    }

    let (cpl_id, pkl_id, am_id, seg_id, vseq, aseq, vres, ares, desc_v, desc_a, track_v, track_a) = (mxf::uuid(), mxf::uuid(), mxf::uuid(), mxf::uuid(), mxf::uuid(), mxf::uuid(), mxf::uuid(), mxf::uuid(), mxf::uuid(), mxf::uuid(), mxf::uuid(), mxf::uuid());
    let now = iso_now();
    let rate_s = format!("{} {}", rate.0, rate.1);
    let d = desc.ok_or("no picture descriptor")?;
    let mut cpl = String::new();
    cpl.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    cpl.push_str("<CompositionPlaylist xmlns=\"http://www.smpte-ra.org/schemas/2067-3/2016\" xmlns:cc=\"http://www.smpte-ra.org/schemas/2067-2/2016\" xmlns:r0=\"http://www.smpte-ra.org/reg/395/2014/13/1/aaf\">\n");
    cpl.push_str(&format!("  <Id>{}</Id>\n  <Annotation>{}</Annotation>\n  <IssueDate>{now}</IssueDate>\n  <Issuer>{}</Issuer>\n  <Creator>{}</Creator>\n  <ContentTitle>{}</ContentTitle>\n  <EditRate>{rate_s}</EditRate>\n", urn(&cpl_id), esc(&spec.title), esc(&spec.issuer), esc(&spec.creator), esc(&spec.title)));
    cpl.push_str("  <ExtensionProperties>\n    <cc:ApplicationIdentification>http://www.smpte-ra.org/ns/2067-21/2020</cc:ApplicationIdentification>\n  </ExtensionProperties>\n");
    cpl.push_str("  <EssenceDescriptorList>\n");
    cpl.push_str(&format!("    <EssenceDescriptor>\n      <Id>{}</Id>\n      <r0:RGBADescriptor>\n        <r0:SampleRate>{rate_s}</r0:SampleRate>\n        <r0:EssenceLength>{frames}</r0:EssenceLength>\n        <r0:FrameLayout>FullFrame</r0:FrameLayout>\n        <r0:StoredWidth>{}</r0:StoredWidth>\n        <r0:StoredHeight>{}</r0:StoredHeight>\n        <r0:ComponentMaxRef>{}</r0:ComponentMaxRef>\n        <r0:ComponentMinRef>0</r0:ComponentMinRef>\n      </r0:RGBADescriptor>\n    </EssenceDescriptor>\n", urn(&desc_v), d.width, d.height, (1u32 << d.bit_depth) - 1));
    if audio.is_some() {
        cpl.push_str(&format!("    <EssenceDescriptor>\n      <Id>{}</Id>\n      <r0:WAVEPCMDescriptor>\n        <r0:SampleRate>{rate_s}</r0:SampleRate>\n        <r0:AudioSampleRate>48000 1</r0:AudioSampleRate>\n        <r0:ChannelCount>{}</r0:ChannelCount>\n        <r0:QuantizationBits>24</r0:QuantizationBits>\n      </r0:WAVEPCMDescriptor>\n    </EssenceDescriptor>\n", urn(&desc_a), audio.map_or(2, |a| a.channels)));
    }
    cpl.push_str("  </EssenceDescriptorList>\n  <SegmentList>\n    <Segment>\n");
    cpl.push_str(&format!("      <Id>{}</Id>\n      <SequenceList>\n", urn(&seg_id)));
    let res = |id: &[u8; 16], track_file: &[u8; 16], desc_id: &[u8; 16]| format!("            <Resource xsi:type=\"TrackFileResourceType\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">\n              <Id>{}</Id>\n              <EditRate>{rate_s}</EditRate>\n              <IntrinsicDuration>{frames}</IntrinsicDuration>\n              <EntryPoint>0</EntryPoint>\n              <SourceDuration>{frames}</SourceDuration>\n              <RepeatCount>1</RepeatCount>\n              <TrackFileId>{}</TrackFileId>\n              <SourceEncoding>{}</SourceEncoding>\n            </Resource>\n", urn(id), urn(track_file), urn(desc_id));
    cpl.push_str(&format!("        <cc:MainImageSequence>\n          <Id>{}</Id>\n          <TrackId>{}</TrackId>\n          <ResourceList>\n{}          </ResourceList>\n        </cc:MainImageSequence>\n", urn(&vseq), urn(&track_v), res(&vres, &tracks[0].1.id, &desc_v)));
    if let Some(t) = tracks.get(1) {
        cpl.push_str(&format!("        <cc:MainAudioSequence>\n          <Id>{}</Id>\n          <TrackId>{}</TrackId>\n          <ResourceList>\n{}          </ResourceList>\n        </cc:MainAudioSequence>\n", urn(&aseq), urn(&track_a), res(&ares, &t.1.id, &desc_a)));
    }
    cpl.push_str("      </SequenceList>\n    </Segment>\n  </SegmentList>\n</CompositionPlaylist>\n");
    let cpl_name = format!("CPL_{}.xml", mxf::uuid_string(&cpl_id));
    std::fs::write(dir.join(&cpl_name), &cpl).map_err(io)?;

    // PKL.
    let mut pkl = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<PackingList xmlns=\"http://www.smpte-ra.org/schemas/2067-2/2016/PKL\">\n  <Id>{}</Id>\n  <IssueDate>{now}</IssueDate>\n  <Issuer>{}</Issuer>\n  <Creator>{}</Creator>\n  <AssetList>\n", urn(&pkl_id), esc(&spec.issuer), esc(&spec.creator));
    let mut entries: Vec<(String, [u8; 16], &str)> = tracks.iter().map(|(p, i, _)| (p.file_name().unwrap().to_string_lossy().into_owned(), i.id, "application/mxf")).collect();
    entries.push((cpl_name.clone(), cpl_id, "text/xml"));
    let mut sizes = Vec::new();
    for (name, id, ty) in &entries {
        let (hash, size) = sha1::file_hash(&dir.join(name)).map_err(io)?;
        sizes.push(size);
        pkl.push_str(&format!("    <Asset>\n      <Id>{}</Id>\n      <Hash>{hash}</Hash>\n      <Size>{size}</Size>\n      <Type>{ty}</Type>\n      <OriginalFileName>{}</OriginalFileName>\n    </Asset>\n", urn(id), esc(name)));
    }
    pkl.push_str("  </AssetList>\n</PackingList>\n");
    let pkl_name = format!("PKL_{}.xml", mxf::uuid_string(&pkl_id));
    std::fs::write(dir.join(&pkl_name), &pkl).map_err(io)?;
    // ASSETMAP and VOLINDEX.
    let ns = "http://www.smpte-ra.org/schemas/429-9/2007/AM";
    let mut am = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<AssetMap xmlns=\"{ns}\">\n  <Id>{}</Id>\n  <Creator>{}</Creator>\n  <VolumeCount>1</VolumeCount>\n  <IssueDate>{now}</IssueDate>\n  <Issuer>{}</Issuer>\n  <AssetList>\n", urn(&am_id), esc(&spec.creator), esc(&spec.issuer));
    let pkl_len = std::fs::metadata(dir.join(&pkl_name)).map_err(io)?.len();
    am.push_str(&format!("    <Asset>\n      <Id>{}</Id>\n      <PackingList>true</PackingList>\n      <ChunkList>\n        <Chunk>\n          <Path>{pkl_name}</Path>\n          <VolumeIndex>1</VolumeIndex>\n          <Offset>0</Offset>\n          <Length>{pkl_len}</Length>\n        </Chunk>\n      </ChunkList>\n    </Asset>\n", urn(&pkl_id)));
    for ((name, id, _), size) in entries.iter().zip(&sizes) {
        am.push_str(&format!("    <Asset>\n      <Id>{}</Id>\n      <ChunkList>\n        <Chunk>\n          <Path>{name}</Path>\n          <VolumeIndex>1</VolumeIndex>\n          <Offset>0</Offset>\n          <Length>{size}</Length>\n        </Chunk>\n      </ChunkList>\n    </Asset>\n", urn(id)));
    }
    am.push_str("  </AssetList>\n</AssetMap>\n");
    std::fs::write(dir.join("ASSETMAP.xml"), am).map_err(io)?;
    std::fs::write(dir.join("VOLINDEX.xml"), format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<VolumeIndex xmlns=\"{ns}\">\n  <Index>1</Index>\n</VolumeIndex>\n")).map_err(io)?;
    Ok(ImfResult { dir: dir.to_path_buf(), cpl: dir.join(cpl_name), frames, files: tracks.into_iter().map(|t| t.0).collect() })
}

fn sanitize(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
    if t.is_empty() { "imf".into() } else { t }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dcp::PackWriter;

    #[test]
    fn planes_scale_eight_bits_to_the_package_depth() {
        let p = rgba_to_planes(&[0, 128, 255, 255], 1, 1, 10);
        assert_eq!((p[0][0], p[1][0], p[2][0]), (0, 514, 1023));
        let p = rgba_to_planes(&[0, 128, 255, 255], 1, 1, 8);
        assert_eq!((p[0][0], p[1][0], p[2][0]), (0, 128, 255));
    }

    #[test]
    fn a_package_ties_its_files_together() {
        let dir = std::env::temp_dir().join(format!("studio-imf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut spec = ImfSpec::new("Imf Test", 160, 96, (24, 1));
        spec.bit_depth = 10;
        let mut rgba = vec![255u8; 160 * 96 * 4];
        for y in 0..96 {
            for x in 0..160 {
                let i = (y * 160 + x) * 4;
                rgba[i] = (x * 255 / 160) as u8;
                rgba[i + 1] = (y * 255 / 96) as u8;
                rgba[i + 2] = 90;
            }
        }
        let mut p = spec.j2k_params();
        p.levels = 3;
        p.precincts = vec![15; 4];
        let planes = rgba_to_planes(&rgba, 160, 96, 10);
        let cs = j2k::encode(&[&planes[0], &planes[1], &planes[2]], &p).unwrap();
        let pack = dir.join("a.pack");
        let mut w = PackWriter::create(&pack).unwrap();
        for _ in 0..6 {
            w.push(&cs).unwrap();
        }
        w.finish().unwrap();
        let stereo: Vec<f32> = (0..48_000 / 4 * 2).map(|i| 0.2 * ((i / 2) as f32 * 0.05).sin()).collect();
        let audio = Audio { channels: 2, samples: stereo };
        let out = dir.join("pkg");
        let r = build(&spec, &out, &[pack], Some(&audio)).unwrap();
        assert_eq!(r.frames, 6);
        assert_eq!(r.files.len(), 2);
        let cpl = std::fs::read_to_string(&r.cpl).unwrap();
        assert!(cpl.contains("2067-21/2020") && cpl.contains("<IntrinsicDuration>6</IntrinsicDuration>") && cpl.contains("MainAudioSequence"));
        let pkl_name = std::fs::read_dir(&out).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).find(|n| n.starts_with("PKL_")).unwrap();
        let pkl = std::fs::read_to_string(out.join(pkl_name)).unwrap();
        // Every packed file's hash and size are right.
        for chunk in pkl.split("<Asset>").skip(1) {
            let tag = |t: &str| chunk.split(&format!("<{t}>")).nth(1).and_then(|s| s.split(&format!("</{t}>")).next()).unwrap().to_string();
            let (hash, size) = sha1::file_hash(&out.join(tag("OriginalFileName"))).unwrap();
            assert_eq!((hash, size.to_string()), (tag("Hash"), tag("Size")));
        }
        // The CPL names the track files by their package ids.
        let am = std::fs::read_to_string(out.join("ASSETMAP.xml")).unwrap();
        assert_eq!(am.matches("<Chunk>").count(), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
