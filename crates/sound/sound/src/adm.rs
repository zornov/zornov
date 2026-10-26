// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Object-based audio as an ADM Broadcast Wave file (ITU-R BS.2076 Audio
//! Definition Model in an `axml` chunk, with the `chna` track map of
//! ITU-R BS.2088 / EBU Tech 3285 s7): a channel bed plus mono objects that
//! move through the room. ADM is an open ITU standard, so writing it needs
//! no licence; immersive renderers and mastering tools read it. Every
//! definition is written into the file (no reliance on the common
//! definitions library). Positions are cartesian: X −1 left..1 right,
//! Y −1 back..1 front, Z 0 ear level..1 top, the same square the surround
//! panner uses.
//!
//! Not checked against a commercial immersive renderer: it follows the
//! standard's structure (programme → content → object → pack → channel →
//! stream → track, block formats with `rtime`/`duration`), and the tests
//! check that structure.

use crate::surround::Layout;
use crate::wav::{Bext, SampleFormat, write_with};

/// One moving mono object.
#[derive(Clone, Debug, PartialEq)]
pub struct Object {
    /// Name shown in renderers ("Car pass", "Dragon").
    pub name: String,
    /// The audio, at the file rate.
    pub samples: Vec<f32>,
    /// Position over time: `(seconds, x, y, z)`, sorted; held after the last.
    pub path: Vec<(f64, f32, f32, f32)>,
    /// Size of the object, 0 point .. 1 whole room.
    pub width: f32,
}

/// Speaker labels and positions (azimuth, elevation) of a bed in crate
/// channel order, as BS.2051 names them.
fn bed_speakers(layout: Layout) -> &'static [(&'static str, f32, f32)] {
    match layout {
        Layout::Stereo => &[("M+030", 30.0, 0.0), ("M-030", -30.0, 0.0)],
        Layout::Surround51 => &[("M+030", 30.0, 0.0), ("M-030", -30.0, 0.0), ("M+000", 0.0, 0.0), ("LFE1", 0.0, -30.0), ("M+110", 110.0, 0.0), ("M-110", -110.0, 0.0)],
        Layout::Surround71 => &[("M+030", 30.0, 0.0), ("M-030", -30.0, 0.0), ("M+000", 0.0, 0.0), ("LFE1", 0.0, -30.0), ("M+090", 90.0, 0.0), ("M-090", -90.0, 0.0), ("M+135", 135.0, 0.0), ("M-135", -135.0, 0.0)],
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn time(secs: f64) -> String {
    // hh:mm:ss.zzzzz with 5 decimals (BS.2076 time format).
    let secs = secs.max(0.0);
    let h = (secs / 3600.0).floor();
    let m = ((secs - h * 3600.0) / 60.0).floor();
    let s = secs - h * 3600.0 - m * 60.0;
    format!("{:02}:{:02}:{:08.5}", h as u64, m as u64, s)
}

/// The ADM XML and `chna` body for a bed of `layout` (tracks 1..=n) and
/// `objects` (the following tracks) lasting `duration` seconds.
pub fn metadata(layout: Layout, objects: &[Object], duration: f64, title: &str, rate: u32) -> (String, Vec<u8>) {
    let bed = bed_speakers(layout);
    let mut x = String::new();
    x.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<ebuCoreMain xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns=\"urn:ebu:metadata-schema:ebuCore_2016\" schema=\"EBU_CORE_20161222.xsd\" xml:lang=\"en\">\n<coreMetadata><format><audioFormatExtended version=\"ITU-R_BS.2076-2\">\n");
    let content_objects: Vec<String> = (0..=objects.len()).map(|i| format!("AO_{:04X}", 0x1001 + i)).collect();
    x.push_str(&format!("<audioProgramme audioProgrammeID=\"APR_1001\" audioProgrammeName=\"{}\" start=\"{}\" end=\"{}\">\n<audioContentIDRef>ACO_1001</audioContentIDRef>\n</audioProgramme>\n", esc(title), time(0.0), time(duration)));
    x.push_str("<audioContent audioContentID=\"ACO_1001\" audioContentName=\"Mix\">\n");
    for o in &content_objects {
        x.push_str(&format!("<audioObjectIDRef>{o}</audioObjectIDRef>\n"));
    }
    x.push_str("</audioContent>\n");
    // Bed object.
    let mut uid = 1u32;
    let bed_uids: Vec<String> = (0..bed.len()).map(|_| {
        let u = format!("ATU_{uid:08X}");
        uid += 1;
        u
    }).collect();
    x.push_str(&format!("<audioObject audioObjectID=\"AO_1001\" audioObjectName=\"Bed {}\" start=\"{}\" duration=\"{}\">\n<audioPackFormatIDRef>AP_00011001</audioPackFormatIDRef>\n", layout.id(), time(0.0), time(duration)));
    for u in &bed_uids {
        x.push_str(&format!("<audioTrackUIDRef>{u}</audioTrackUIDRef>\n"));
    }
    x.push_str("</audioObject>\n");
    let obj_uids: Vec<String> = objects.iter().map(|_| {
        let u = format!("ATU_{uid:08X}");
        uid += 1;
        u
    }).collect();
    for (i, o) in objects.iter().enumerate() {
        x.push_str(&format!("<audioObject audioObjectID=\"{}\" audioObjectName=\"{}\" start=\"{}\" duration=\"{}\">\n<audioPackFormatIDRef>AP_{:08X}</audioPackFormatIDRef>\n<audioTrackUIDRef>{}</audioTrackUIDRef>\n</audioObject>\n", content_objects[i + 1], esc(&o.name), time(0.0), time(duration), 0x0003_1001 + i as u32, obj_uids[i]));
    }
    // Bed pack: DirectSpeakers (type 0001).
    x.push_str("<audioPackFormat audioPackFormatID=\"AP_00011001\" audioPackFormatName=\"Bed\" typeLabel=\"0001\" typeDefinition=\"DirectSpeakers\">\n");
    for k in 0..bed.len() {
        x.push_str(&format!("<audioChannelFormatIDRef>AC_{:08X}</audioChannelFormatIDRef>\n", 0x0001_1001 + k as u32));
    }
    x.push_str("</audioPackFormat>\n");
    for (k, (label, az, el)) in bed.iter().enumerate() {
        let id = 0x0001_1001 + k as u32;
        x.push_str(&format!("<audioChannelFormat audioChannelFormatID=\"AC_{id:08X}\" audioChannelFormatName=\"{label}\" typeLabel=\"0001\" typeDefinition=\"DirectSpeakers\">\n<audioBlockFormat audioBlockFormatID=\"AB_{id:08X}_00000001\">\n<speakerLabel>{label}</speakerLabel>\n<position coordinate=\"azimuth\">{az:.1}</position>\n<position coordinate=\"elevation\">{el:.1}</position>\n<position coordinate=\"distance\">1.0</position>\n</audioBlockFormat>\n</audioChannelFormat>\n"));
    }
    // Object packs and moving channel formats (type 0003).
    for (i, o) in objects.iter().enumerate() {
        let id = 0x0003_1001 + i as u32;
        x.push_str(&format!("<audioPackFormat audioPackFormatID=\"AP_{id:08X}\" audioPackFormatName=\"{}\" typeLabel=\"0003\" typeDefinition=\"Objects\">\n<audioChannelFormatIDRef>AC_{id:08X}</audioChannelFormatIDRef>\n</audioPackFormat>\n", esc(&o.name)));
        x.push_str(&format!("<audioChannelFormat audioChannelFormatID=\"AC_{id:08X}\" audioChannelFormatName=\"{}\" typeLabel=\"0003\" typeDefinition=\"Objects\">\n", esc(&o.name)));
        let mut path = o.path.clone();
        if path.is_empty() {
            path.push((0.0, 0.0, 1.0, 0.0));
        }
        for (b, w) in path.iter().enumerate() {
            let start = if b == 0 { 0.0 } else { w.0 };
            let end = path.get(b + 1).map_or(duration, |n| n.0).max(start);
            x.push_str(&format!("<audioBlockFormat audioBlockFormatID=\"AB_{id:08X}_{:08X}\" rtime=\"{}\" duration=\"{}\">\n<cartesian>1</cartesian>\n<position coordinate=\"X\">{:.4}</position>\n<position coordinate=\"Y\">{:.4}</position>\n<position coordinate=\"Z\">{:.4}</position>\n<width>{:.3}</width>\n<jumpPosition>0</jumpPosition>\n</audioBlockFormat>\n", b + 1, time(start), time(end - start), w.1.clamp(-1.0, 1.0), w.2.clamp(-1.0, 1.0), w.3.clamp(-1.0, 1.0), o.width.clamp(0.0, 1.0)));
        }
        x.push_str("</audioChannelFormat>\n");
    }
    // Streams and tracks: one each per channel format.
    let all: Vec<u32> = (0..bed.len() as u32).map(|k| 0x0001_1001 + k).chain((0..objects.len() as u32).map(|i| 0x0003_1001 + i)).collect();
    for id in &all {
        x.push_str(&format!("<audioStreamFormat audioStreamFormatID=\"AS_{id:08X}\" audioStreamFormatName=\"PCM_{id:08X}\" formatLabel=\"0001\" formatDefinition=\"PCM\">\n<audioChannelFormatIDRef>AC_{id:08X}</audioChannelFormatIDRef>\n<audioTrackFormatIDRef>AT_{id:08X}_01</audioTrackFormatIDRef>\n</audioStreamFormat>\n"));
        x.push_str(&format!("<audioTrackFormat audioTrackFormatID=\"AT_{id:08X}_01\" audioTrackFormatName=\"PCM_{id:08X}\" formatLabel=\"0001\" formatDefinition=\"PCM\">\n<audioStreamFormatIDRef>AS_{id:08X}</audioStreamFormatIDRef>\n</audioTrackFormat>\n"));
    }
    let uids: Vec<&String> = bed_uids.iter().chain(&obj_uids).collect();
    for (k, (u, id)) in uids.iter().zip(&all).enumerate() {
        let pack = if k < bed.len() { "AP_00011001".to_owned() } else { format!("AP_{id:08X}") };
        x.push_str(&format!("<audioTrackUID UID=\"{u}\" sampleRate=\"{rate}\" bitDepth=\"24\">\n<audioTrackFormatIDRef>AT_{id:08X}_01</audioTrackFormatIDRef>\n<audioPackFormatIDRef>{pack}</audioPackFormatIDRef>\n</audioTrackUID>\n"));
    }
    x.push_str("</audioFormatExtended></format></coreMetadata></ebuCoreMain>\n");
    // chna: numTracks, numUIDs, then 40-byte entries.
    let mut chna = Vec::new();
    let n = all.len() as u16;
    chna.extend_from_slice(&n.to_le_bytes());
    chna.extend_from_slice(&n.to_le_bytes());
    for (k, (u, id)) in uids.iter().zip(&all).enumerate() {
        chna.extend_from_slice(&(k as u16 + 1).to_le_bytes());
        chna.extend_from_slice(u.as_bytes());
        chna.extend_from_slice(format!("AT_{id:08X}_01").as_bytes());
        let pack = if k < bed.len() { "AP_00011001".to_owned() } else { format!("AP_{id:08X}") };
        chna.extend_from_slice(pack.as_bytes());
        chna.push(0);
    }
    (x, chna)
}

/// A complete ADM BWF file: the bed's channels (crate order) then one
/// track per object, 24-bit at `rate`.
pub fn write(bed: &[Vec<f32>], layout: Layout, objects: &[Object], rate: u32, title: &str) -> Vec<u8> {
    let frames = bed.iter().map(Vec::len).chain(objects.iter().map(|o| o.samples.len())).max().unwrap_or(0);
    let duration = frames as f64 / rate.max(1) as f64;
    let (xml, chna) = metadata(layout, objects, duration, title, rate);
    let mut chans: Vec<Vec<f32>> = bed.to_vec();
    chans.resize(layout.channels(), Vec::new());
    for o in objects {
        chans.push(o.samples.clone());
    }
    for c in &mut chans {
        c.resize(frames, 0.0);
    }
    // Tracks stay in ADM order (the chna map says what each is), so no WAV
    // speaker mask or reordering.
    write_with(&chans, rate, SampleFormat::Pcm24, None, Some(&Bext { description: title.into(), originator: "Zornov".into(), time_reference: 0 }), &[(*b"chna", chna), (*b"axml", xml.into_bytes())])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bed_and_a_moving_object() {
        let bed: Vec<Vec<f32>> = (0..6).map(|_| vec![0.1; 4800]).collect();
        let obj = Object { name: "Car <pass>".into(), samples: vec![0.2; 9600], path: vec![(0.0, -1.0, 1.0, 0.0), (0.1, 1.0, -1.0, 0.5)], width: 0.1 };
        let bytes = write(&bed, Layout::Surround51, &[obj], 48_000, "Reel 1");
        let p = crate::wav::parse(&bytes).unwrap();
        assert_eq!(p.chans.len(), 7);
        assert_eq!(p.chans[0].len(), 9600);
        let chna = &p.chunks.iter().find(|c| &c.0 == b"chna").unwrap().1;
        assert_eq!(u16::from_le_bytes([chna[0], chna[1]]), 7);
        assert_eq!(chna.len(), 4 + 7 * 40);
        assert_eq!(&chna[4 + 6 * 40 + 2..4 + 6 * 40 + 14], b"ATU_00000007");
        assert_eq!(&chna[4 + 6 * 40 + 28..4 + 6 * 40 + 39], b"AP_00031001");
        let xml = String::from_utf8(p.chunks.iter().find(|c| &c.0 == b"axml").unwrap().1.clone()).unwrap();
        assert!(xml.contains("Car &lt;pass&gt;"));
        assert!(xml.contains("<speakerLabel>LFE1</speakerLabel>"));
        assert_eq!(xml.matches("<audioBlockFormat audioBlockFormatID=\"AB_00031001_").count(), 2);
        assert!(xml.contains("rtime=\"00:00:00.10000\" duration=\"00:00:00.10000\""), "second block runs to the end");
        assert_eq!(xml.matches("<audioTrackUID ").count(), 7);
        // Every reference has a definition.
        for id in ["AP_00011001", "AC_00011006", "AS_00031001", "AT_00031001_01", "AO_1002"] {
            assert!(xml.matches(id).count() >= 2, "{id}");
        }
    }
}
