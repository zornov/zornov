// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Fragmented MP4: a minimal in-test fMP4 writer exercising the
//! `mvex/trex` + `moof/traf/tfhd/tfdt/trun` paths.

use std::io::Cursor;

use studio_mp4::{Codec, ColorInfo, Demuxer, TrackKind};

fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}

fn full(kind: &[u8; 4], version: u8, flags: u32, body: &[u8]) -> Vec<u8> {
    let mut b = ((version as u32) << 24 | flags).to_be_bytes().to_vec();
    b.extend_from_slice(body);
    bx(kind, &b)
}

fn be(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_be_bytes()).collect()
}

fn cat(parts: &[Vec<u8>]) -> Vec<u8> {
    parts.concat()
}

const ONE: u32 = 0x10000;
const SYNC: u32 = 0x0200_0000; // depends_on = 2 (I-frame)
const NON_SYNC: u32 = 0x0101_0000; // depends_on = 1, non-sync

fn matrix(rot90: bool) -> Vec<u8> {
    if rot90 {
        be(&[0, ONE, 0, (-(ONE as i32)) as u32, 0, 0, 0, 0, 0x4000_0000])
    } else {
        be(&[ONE, 0, 0, 0, ONE, 0, 0, 0, 0x4000_0000])
    }
}

fn tkhd(id: u32, rot90: bool, w: u32, h: u32) -> Vec<u8> {
    let mut b = be(&[0, 0, id, 0, 0, 0, 0, 0, 0]);
    b.extend(matrix(rot90));
    b.extend(be(&[w << 16, h << 16]));
    full(b"tkhd", 0, 3, &b)
}

fn mdhd(timescale: u32, lang: u16) -> Vec<u8> {
    let mut b = be(&[0, 0, timescale, 0]);
    b.extend(lang.to_be_bytes());
    b.extend([0, 0]);
    full(b"mdhd", 0, 0, &b)
}

fn hdlr(kind: &[u8; 4]) -> Vec<u8> {
    let mut b = be(&[0]);
    b.extend_from_slice(kind);
    b.extend(be(&[0, 0, 0]));
    b.push(0);
    full(b"hdlr", 0, 0, &b)
}

fn empty_tables() -> Vec<u8> {
    cat(&[
        full(b"stts", 0, 0, &be(&[0])),
        full(b"stsc", 0, 0, &be(&[0])),
        full(b"stsz", 0, 0, &be(&[0, 0])),
        full(b"stco", 0, 0, &be(&[0])),
    ])
}

fn avc1_entry() -> Vec<u8> {
    let mut b = vec![0u8; 6];
    b.extend([0, 1]); // data_reference_index
    b.extend([0u8; 16]);
    b.extend(320u16.to_be_bytes());
    b.extend(240u16.to_be_bytes());
    b.extend(be(&[0x0048_0000, 0x0048_0000, 0]));
    b.extend(1u16.to_be_bytes());
    b.extend([0u8; 32]);
    b.extend(24u16.to_be_bytes());
    b.extend((-1i16).to_be_bytes());
    let avcc = [1, 77, 0, 30, 0xff, 0xe1, 0, 4, 0x67, 77, 0, 30, 1, 0, 2, 0x68, 0xce];
    b.extend(bx(b"avcC", &avcc));
    b.extend(bx(b"pasp", &be(&[4, 3])));
    b.extend(bx(b"colr", &[b'n', b'c', b'l', b'x', 0, 1, 0, 1, 0, 1, 0x80]));
    b.extend(bx(b"btrt", &be(&[1000, 900_000, 500_000])));
    bx(b"avc1", &b)
}

fn mp4a_entry() -> Vec<u8> {
    let mut b = vec![0u8; 6];
    b.extend([0, 1]);
    b.extend([0u8; 8]); // version, revision, vendor
    b.extend(2u16.to_be_bytes());
    b.extend(16u16.to_be_bytes());
    b.extend([0u8; 4]);
    b.extend(be(&[44100 << 16]));
    // ES_Descriptor > DecoderConfigDescriptor > DecoderSpecificInfo (AAC LC 44.1k stereo).
    let dsi = [0x05, 0x02, 0x12, 0x10];
    let mut dcd = vec![0x04, 13 + dsi.len() as u8, 0x40, 0x15, 0, 0, 0];
    dcd.extend(be(&[128_000, 128_000]));
    dcd.extend(dsi);
    let mut es = vec![0x03, 3 + dcd.len() as u8, 0, 1, 0];
    es.extend(dcd);
    b.extend(full(b"esds", 0, 0, &es));
    bx(b"mp4a", &b)
}

fn trak(id: u32, kind: &[u8; 4], entry: Vec<u8>, timescale: u32, edits: Option<Vec<u8>>) -> Vec<u8> {
    let stbl = bx(b"stbl", &cat(&[full(b"stsd", 0, 0, &cat(&[be(&[1]), entry])), empty_tables()]));
    let minf = bx(b"minf", &stbl);
    let mdia = bx(b"mdia", &cat(&[mdhd(timescale, 0x15c7), hdlr(kind), minf]));
    let video = kind == b"vide";
    let mut parts = vec![tkhd(id, video, if video { 320 } else { 0 }, if video { 240 } else { 0 })];
    if let Some(e) = edits {
        parts.push(bx(b"edts", &e));
    }
    parts.push(mdia);
    bx(b"trak", &cat(&parts))
}

fn trex(id: u32, duration: u32, size: u32, flags: u32) -> Vec<u8> {
    full(b"trex", 0, 0, &be(&[id, 1, duration, size, flags]))
}

/// Payload bytes of sample `k`: `size` copies of `k`.
fn payload(k: u8, size: usize) -> Vec<u8> {
    vec![k; size]
}

struct Built {
    file: Vec<u8>,
    video: Vec<Vec<u8>>,
    audio: Vec<Vec<u8>>,
}

fn build() -> Built {
    let ftyp = bx(b"ftyp", &cat(&[b"iso6".to_vec(), be(&[0]), b"iso6".to_vec(), b"avc1".to_vec()]));
    // Single unbounded edit skipping the composition delay of 512.
    let elst = full(b"elst", 0, 0, &be(&[1, 0, 512, ONE]));
    let moov = bx(
        b"moov",
        &cat(&[
            full(b"mvhd", 0, 0, &cat(&[be(&[0, 0, 1000, 0, ONE]), vec![0; 80]])),
            trak(1, b"vide", avc1_entry(), 12800, Some(elst)),
            trak(2, b"soun", mp4a_entry(), 44100, None),
            bx(b"mvex", &cat(&[trex(1, 512, 0, NON_SYNC), trex(2, 1024, 0, 0)])),
        ]),
    );

    let mut file = cat(&[ftyp, moov]);
    let mut video = Vec::new();
    let mut audio = Vec::new();

    // Fragment 1: video uses default-base-is-moof + explicit sizes and CTOs;
    // audio uses an absolute base_data_offset and tfhd default sizes.
    {
        let v_sizes = [30u32, 20, 10, 12];
        let v_cto = [512u32, 1536, 0, 0];
        let a_count = 3u32;
        let moof_start = file.len() as u64;
        let make = |v_off: u32, a_base: u64| {
            let mut trun_v = be(&[4, v_off, SYNC]);
            for i in 0..4 {
                trun_v.extend(be(&[v_sizes[i], v_cto[i]]));
            }
            let traf_v = bx(
                b"traf",
                &cat(&[
                    full(b"tfhd", 0, 0x2_0000, &be(&[1])),
                    full(b"tfdt", 1, 0, &0u64.to_be_bytes()),
                    full(b"trun", 0, 0x01 | 0x04 | 0x200 | 0x800, &trun_v),
                ]),
            );
            let mut tfhd_a = be(&[2]);
            tfhd_a.extend(a_base.to_be_bytes());
            tfhd_a.extend(be(&[1024, 10]));
            let traf_a = bx(
                b"traf",
                &cat(&[
                    full(b"tfhd", 0, 0x01 | 0x08 | 0x10, &tfhd_a),
                    full(b"tfdt", 0, 0, &be(&[0])),
                    full(b"trun", 0, 0x01, &be(&[a_count, 0])),
                ]),
            );
            bx(b"moof", &cat(&[full(b"mfhd", 0, 0, &be(&[1])), traf_v, traf_a]))
        };
        let moof_len = make(0, 0).len() as u64;
        let mdat_payload = moof_start + moof_len + 8;
        let v_bytes: u64 = v_sizes.iter().map(|&s| s as u64).sum();
        let moof = make((mdat_payload - moof_start) as u32, mdat_payload + v_bytes);
        let mut mdat = Vec::new();
        for (k, &s) in v_sizes.iter().enumerate() {
            video.push(payload(k as u8 + 1, s as usize));
            mdat.extend(video.last().unwrap());
        }
        for k in 0..a_count {
            audio.push(payload(0x80 + k as u8, 10));
            mdat.extend(audio.last().unwrap());
        }
        file.extend(moof);
        file.extend(bx(b"mdat", &mdat));
    }

    // Fragment 2: video relies on tfhd defaults and continues dts without
    // tfdt; audio's trun has no data_offset, so its data follows the previous
    // traf's data (ISO base offset rule without default-base-is-moof).
    {
        let moof_start = file.len() as u64;
        let a_sizes = [5u32, 6];
        let make = |v_off: u32| {
            let traf_v = bx(
                b"traf",
                &cat(&[
                    full(b"tfhd", 0, 0x08 | 0x10 | 0x20, &be(&[1, 512, 7, NON_SYNC])),
                    full(b"trun", 0, 0x01 | 0x04 | 0x800, &be(&[2, v_off, SYNC, 512, 512])),
                ]),
            );
            let traf_a = bx(
                b"traf",
                &cat(&[
                    full(b"tfhd", 0, 0, &be(&[2])),
                    full(b"tfdt", 1, 0, &3072u64.to_be_bytes()),
                    full(b"trun", 0, 0x200, &be(&[2, a_sizes[0], a_sizes[1]])),
                ]),
            );
            bx(b"moof", &cat(&[full(b"mfhd", 0, 0, &be(&[2])), traf_v, traf_a]))
        };
        let moof_len = make(0).len() as u64;
        let moof = make((moof_len + 8) as u32);
        let mut mdat = Vec::new();
        for k in 0..2u8 {
            video.push(payload(0x10 + k, 7));
            mdat.extend(video.last().unwrap());
        }
        for (k, &s) in a_sizes.iter().enumerate() {
            audio.push(payload(0x90 + k as u8, s as usize));
            mdat.extend(audio.last().unwrap());
        }
        assert_eq!(moof_start + moof.len() as u64, file.len() as u64 + moof_len);
        file.extend(moof);
        file.extend(bx(b"mdat", &mdat));
    }
    Built { file, video, audio }
}

#[test]
fn fragmented_mp4_expands_into_sample_tables() {
    let built = build();
    let mut dmx = Demuxer::new(Cursor::new(built.file.clone())).unwrap();
    let movie = dmx.movie().clone();
    assert!(movie.fragmented);
    assert_eq!(&movie.brand, b"iso6");
    assert_eq!(movie.tracks.len(), 2);

    let v = movie.video().unwrap();
    assert_eq!((v.id, v.kind, v.timescale), (1, TrackKind::Video, 12800));
    assert_eq!((v.width, v.height, v.rotation, v.flip), (320, 240, 90, false));
    assert_eq!(v.language, "eng");
    assert_eq!(v.pixel_aspect, Some((4, 3)));
    assert_eq!(
        v.color,
        Some(ColorInfo::Nclx { primaries: 1, transfer: 1, matrix: 1, full_range: true })
    );
    assert_eq!(v.bitrate.map(|b| b.avg), Some(500_000));
    let Codec::Avc { config } = &v.codec else { panic!("not avc") };
    assert_eq!((config.profile_idc, config.level_idc, config.nal_length_size), (77, 30, 4));

    let dts: Vec<i64> = v.samples.iter().map(|s| s.dts).collect();
    let pts: Vec<i64> = v.samples.iter().map(|s| s.pts).collect();
    let sync: Vec<bool> = v.samples.iter().map(|s| s.sync).collect();
    assert_eq!(dts, [0, 512, 1024, 1536, 2048, 2560]);
    assert_eq!(pts, [512, 2048, 1024, 1536, 2560, 3072]);
    assert_eq!(sync, [true, false, false, false, true, false]);
    assert_eq!(v.sdtp, [0x20, 0x10, 0x10, 0x10, 0x20, 0x10]);
    assert_eq!(v.edit_offset, 512);
    assert_eq!(v.presentation_order(), [0, 2, 3, 1, 4, 5]);
    // Unbounded edit: media end (3072) minus media_time (512).
    assert!((v.duration_secs - 2560.0 / 12800.0).abs() < 1e-12);
    assert!((movie.duration_secs - v.duration_secs).abs() < 1e-12);
    for (s, want) in v.samples.iter().zip(&built.video) {
        assert_eq!(&dmx.read_sample(s).unwrap(), want);
    }

    let sp = v.sample_for_time(1536.0 / 12800.0).unwrap();
    assert_eq!((sp.sync_index, sp.target_index), (0, 1));
    let sp = v.sample_for_time(2100.0 / 12800.0).unwrap();
    assert_eq!((sp.sync_index, sp.target_index), (4, 4));

    let a = movie.audio().unwrap();
    assert_eq!(a.kind, TrackKind::Audio);
    assert_eq!(
        a.codec,
        Codec::Aac { asc: vec![0x12, 0x10], object_type: 2, sample_rate: 44100, channels: 2 }
    );
    assert_eq!(a.bitrate.map(|b| b.avg), Some(128_000));
    let dts: Vec<i64> = a.samples.iter().map(|s| s.dts).collect();
    assert_eq!(dts, [0, 1024, 2048, 3072, 4096]);
    assert!(a.samples.iter().all(|s| s.sync && s.duration == 1024));
    for (s, want) in a.samples.iter().zip(&built.audio) {
        assert_eq!(&dmx.read_sample(s).unwrap(), want);
    }
    assert_eq!(a.samples.len(), built.audio.len());
}

#[test]
fn fragmented_mp4_rejects_bad_fragments() {
    let built = build();
    // Truncating inside a box must fail; cutting between fragments leaves a
    // valid file with fewer samples (as with an interrupted live stream).
    let mut boundaries = Vec::new();
    let mut pos = 0;
    while pos < built.file.len() {
        boundaries.push(pos);
        pos += u32::from_be_bytes(built.file[pos..pos + 4].try_into().unwrap()) as usize;
    }
    for cut in built.file.len() - 300..built.file.len() {
        let r = Demuxer::new(Cursor::new(built.file[..cut].to_vec()));
        if boundaries.contains(&cut) {
            // Only a cut after a complete moof+mdat pair is valid.
            if let Ok(d) = r {
                assert_eq!(d.movie().video().unwrap().samples.len(), 4);
            }
        } else {
            assert!(r.is_err(), "cut {cut}");
        }
    }
    // boundaries: ftyp, moov, moof1, mdat1, moof2, mdat2.
    let after_first = boundaries[4];
    let d = Demuxer::new(Cursor::new(built.file[..after_first].to_vec())).unwrap();
    assert_eq!(d.movie().audio().unwrap().samples.len(), 3);
    // Redirect the first traf to an unknown track id.
    let mut f = built.file.clone();
    let tfhd = f.windows(4).position(|w| w == b"tfhd").unwrap();
    f[tfhd + 8..tfhd + 12].copy_from_slice(&9u32.to_be_bytes());
    assert!(Demuxer::new(Cursor::new(f)).is_err());
    // A huge trun sample count without per-sample data.
    let mut f = built.file.clone();
    let trun = f.windows(4).rposition(|w| w == b"trun").unwrap();
    f[trun + 8..trun + 12].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(Demuxer::new(Cursor::new(f)).is_err());
}
