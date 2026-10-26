// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! HLS playlists (RFC 8216): master and media playlists, variant and audio
//! choice, keys and maps. Only what is needed to fetch the segments.

use crate::error::{Error, Result};
use crate::types::Quality;
use crate::url::join;

#[derive(Debug, Clone, Default)]
pub struct Variant {
    pub uri: String,
    pub bandwidth: u64,
    pub width: u32,
    pub height: u32,
    pub codecs: String,
    pub audio: Option<String>,
    pub frame_rate: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct Rendition {
    pub kind: String,
    pub group: String,
    pub name: String,
    pub lang: Option<String>,
    pub uri: Option<String>,
    pub default: bool,
    pub autoselect: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Master {
    pub variants: Vec<Variant>,
    pub renditions: Vec<Rendition>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyMethod {
    None,
    Aes128,
    /// SAMPLE-AES and friends: protected.
    Protected(String),
}

#[derive(Debug, Clone)]
pub struct KeyInfo {
    pub method: KeyMethod,
    pub uri: Option<String>,
    pub iv: Option<[u8; 16]>,
}

#[derive(Debug, Clone)]
pub struct MapInfo {
    pub uri: String,
    pub range: Option<(u64, u64)>,
    /// The key in force where the map was declared (the init segment can be encrypted too).
    pub key: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct Segment {
    pub uri: String,
    pub dur: f64,
    /// Inclusive byte range.
    pub range: Option<(u64, u64)>,
    pub key: Option<usize>,
    pub map: Option<usize>,
    pub disc: bool,
    pub seq: u64,
}

#[derive(Debug, Clone, Default)]
pub struct Media {
    pub segments: Vec<Segment>,
    pub keys: Vec<KeyInfo>,
    pub maps: Vec<MapInfo>,
    pub endlist: bool,
    pub i_frames_only: bool,
    pub target: f64,
}

pub enum Playlist {
    Master(Master),
    Media(Media),
}

/// `KEY=value,KEY2="quoted, value"` into pairs.
pub fn attr_list(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && (b[i] == b',' || b[i].is_ascii_whitespace()) {
            i += 1;
        }
        let ks = i;
        while i < b.len() && b[i] != b'=' && b[i] != b',' {
            i += 1;
        }
        let key = s[ks..i].trim().to_ascii_uppercase();
        if i >= b.len() || b[i] != b'=' {
            continue;
        }
        i += 1;
        let val;
        if i < b.len() && b[i] == b'"' {
            i += 1;
            let vs = i;
            while i < b.len() && b[i] != b'"' {
                i += 1;
            }
            val = s[vs..i].to_owned();
            i += 1;
        } else {
            let vs = i;
            while i < b.len() && b[i] != b',' {
                i += 1;
            }
            val = s[vs..i].trim().to_owned();
        }
        if !key.is_empty() {
            out.push((key, val));
        }
    }
    out
}

fn get<'a>(a: &'a [(String, String)], k: &str) -> Option<&'a str> {
    a.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
}

fn hex16(s: &str) -> Option<[u8; 16]> {
    let h = s.trim().trim_start_matches("0x").trim_start_matches("0X");
    if h.len() > 32 || h.is_empty() || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let padded = format!("{h:0>32}");
    let mut out = [0u8; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&padded[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

fn byterange(s: &str, prev_end: Option<u64>) -> Option<(u64, u64)> {
    let (n, o) = match s.split_once('@') {
        Some((n, o)) => (n.trim().parse::<u64>().ok()?, o.trim().parse::<u64>().ok()?),
        None => (s.trim().parse::<u64>().ok()?, prev_end?),
    };
    (n > 0).then(|| (o, o + n - 1))
}

/// The resolution "WxH".
fn resolution(s: &str) -> (u32, u32) {
    s.split_once(['x', 'X']).and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?))).unwrap_or((0, 0))
}

pub fn parse(base: &str, text: &str) -> Result<Playlist> {
    let text = text.trim_start_matches('\u{feff}');
    if !text.trim_start().starts_with("#EXTM3U") {
        return Err(Error::Bad("that is not a playlist (it does not start with #EXTM3U)".into()));
    }
    let abs = |u: &str| join(base, u).ok_or_else(|| Error::Bad(format!("the playlist points at an address Studio cannot use: {u}")));
    let mut master = Master::default();
    let mut media = Media::default();
    let mut is_master = false;
    let mut pending_variant: Option<Variant> = None;
    let mut dur: Option<f64> = None;
    let mut pending_range: Option<(u64, u64)> = None;
    let mut last_end: Option<u64> = None;
    let mut key: Option<usize> = None;
    let mut map: Option<usize> = None;
    let mut disc = false;
    let mut seq: u64 = 0;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('#') {
            let (tag, val) = rest.split_once(':').unwrap_or((rest, ""));
            match tag {
                "EXT-X-STREAM-INF" => {
                    is_master = true;
                    let a = attr_list(val);
                    let (w, h) = get(&a, "RESOLUTION").map(resolution).unwrap_or((0, 0));
                    pending_variant = Some(Variant {
                        uri: String::new(),
                        bandwidth: get(&a, "AVERAGE-BANDWIDTH").or(get(&a, "BANDWIDTH")).and_then(|v| v.parse().ok()).unwrap_or(0),
                        width: w,
                        height: h,
                        codecs: get(&a, "CODECS").unwrap_or("").to_owned(),
                        audio: get(&a, "AUDIO").map(str::to_owned),
                        frame_rate: get(&a, "FRAME-RATE").and_then(|v| v.parse().ok()),
                    });
                }
                "EXT-X-MEDIA" => {
                    is_master = true;
                    let a = attr_list(val);
                    master.renditions.push(Rendition {
                        kind: get(&a, "TYPE").unwrap_or("").to_ascii_uppercase(),
                        group: get(&a, "GROUP-ID").unwrap_or("").to_owned(),
                        name: get(&a, "NAME").unwrap_or("").to_owned(),
                        lang: get(&a, "LANGUAGE").map(str::to_owned),
                        uri: get(&a, "URI").map(abs).transpose()?,
                        default: get(&a, "DEFAULT").is_some_and(|v| v.eq_ignore_ascii_case("YES")),
                        autoselect: get(&a, "AUTOSELECT").is_some_and(|v| v.eq_ignore_ascii_case("YES")),
                    });
                }
                "EXTINF" => dur = val.split(',').next().and_then(|d| d.trim().parse::<f64>().ok()),
                "EXT-X-BYTERANGE" => pending_range = byterange(val, last_end),
                "EXT-X-TARGETDURATION" => media.target = val.trim().parse().unwrap_or(0.0),
                "EXT-X-MEDIA-SEQUENCE" => seq = val.trim().parse().unwrap_or(0),
                "EXT-X-ENDLIST" => media.endlist = true,
                "EXT-X-I-FRAMES-ONLY" => media.i_frames_only = true,
                "EXT-X-DISCONTINUITY" => disc = true,
                "EXT-X-KEY" => {
                    let a = attr_list(val);
                    let m = get(&a, "METHOD").unwrap_or("NONE").to_ascii_uppercase();
                    let format = get(&a, "KEYFORMAT").unwrap_or("identity").to_owned();
                    let method = match m.as_str() {
                        "NONE" => KeyMethod::None,
                        "AES-128" if format.eq_ignore_ascii_case("identity") => KeyMethod::Aes128,
                        other => KeyMethod::Protected(format!("{other} with key format {format}")),
                    };
                    if method == KeyMethod::None {
                        key = None;
                    } else {
                        media.keys.push(KeyInfo { method, uri: get(&a, "URI").and_then(|u| join(base, u).or_else(|| Some(u.to_owned()))), iv: get(&a, "IV").and_then(hex16) });
                        key = Some(media.keys.len() - 1);
                    }
                }
                "EXT-X-MAP" => {
                    let a = attr_list(val);
                    if let Some(u) = get(&a, "URI") {
                        media.maps.push(MapInfo { uri: abs(u)?, range: get(&a, "BYTERANGE").and_then(|r| byterange(r, None)), key });
                        map = Some(media.maps.len() - 1);
                    }
                }
                _ => {}
            }
            continue;
        }
        // A URI line.
        if let Some(mut v) = pending_variant.take() {
            v.uri = abs(line)?;
            master.variants.push(v);
        } else if let Some(d) = dur.take() {
            let range = pending_range.take();
            last_end = range.map(|r| r.1 + 1);
            media.segments.push(Segment { uri: abs(line)?, dur: d, range, key, map, disc, seq });
            seq += 1;
            disc = false;
        }
    }
    if is_master && !master.variants.is_empty() {
        Ok(Playlist::Master(master))
    } else if is_master && media.segments.is_empty() {
        Err(Error::Bad("the playlist lists no streams".into()))
    } else {
        Ok(Playlist::Media(media))
    }
}

impl Media {
    /// Why this stream is protected, if it is.
    pub fn protection(&self) -> Option<String> {
        self.keys.iter().find_map(|k| match &k.method {
            KeyMethod::Protected(why) => Some(format!("This stream is protected ({why}). Studio does not work around copy protection (DRM), so it cannot be downloaded.")),
            _ => None,
        })
    }

    pub fn seconds(&self) -> f64 {
        self.segments.iter().map(|s| s.dur).sum()
    }
}

fn is_h264(c: &str) -> bool {
    c.contains("avc1") || c.contains("avc3")
}

fn audio_only(v: &Variant) -> bool {
    v.height == 0 && v.width == 0 && (v.codecs.is_empty() || !(v.codecs.contains("avc") || v.codecs.contains("hvc") || v.codecs.contains("hev") || v.codecs.contains("av01") || v.codecs.contains("vp09")))
}

/// Plain words for a variant: "1280x720, 2.4 Mbit/s".
pub fn describe(v: &Variant) -> String {
    let mut parts = Vec::new();
    if v.height > 0 {
        parts.push(format!("{}x{}", v.width, v.height));
    } else if audio_only(v) {
        parts.push("sound only".into());
    }
    if let Some(f) = v.frame_rate.filter(|f| *f > 0.0) {
        parts.push(format!("{} fps", (f * 100.0).round() / 100.0));
    }
    if v.bandwidth > 0 {
        parts.push(format!("{:.1} Mbit/s", v.bandwidth as f64 / 1e6));
    }
    if parts.is_empty() { "the only stream".into() } else { parts.join(", ") }
}

/// The variant to take for `q`.
pub fn choose_variant(m: &Master, q: Quality) -> Option<usize> {
    let all: Vec<usize> = (0..m.variants.len()).collect();
    let video: Vec<usize> = all.iter().copied().filter(|&i| !audio_only(&m.variants[i])).collect();
    let rank = |i: usize| {
        let v = &m.variants[i];
        (v.height, is_h264(&v.codecs) as u8, v.bandwidth)
    };
    match q {
        Quality::AudioOnly => all.iter().copied().filter(|&i| audio_only(&m.variants[i])).min_by_key(|&i| m.variants[i].bandwidth).or_else(|| video.iter().copied().min_by_key(|&i| m.variants[i].bandwidth)),
        Quality::Best => {
            let pool = if video.is_empty() { &all } else { &video };
            pool.iter().copied().max_by_key(|&i| rank(i))
        }
        Quality::Lowest => {
            let pool = if video.is_empty() { &all } else { &video };
            pool.iter().copied().min_by_key(|&i| (m.variants[i].bandwidth, rank(i)))
        }
        Quality::Height(h) => {
            let pool = if video.is_empty() { &all } else { &video };
            let under: Vec<usize> = pool.iter().copied().filter(|&i| m.variants[i].height <= h).collect();
            if under.is_empty() {
                pool.iter().copied().min_by_key(|&i| rank(i))
            } else {
                under.into_iter().max_by_key(|&i| rank(i))
            }
        }
    }
}

/// The separate sound playlist that goes with `v`, if its sound is not in
/// the variant itself.
pub fn choose_audio<'a>(m: &'a Master, v: &Variant, lang: Option<&str>) -> Option<&'a Rendition> {
    let group = v.audio.as_deref()?;
    let mut c: Vec<&Rendition> = m.renditions.iter().filter(|r| r.kind == "AUDIO" && r.group == group && r.uri.is_some()).collect();
    if c.is_empty() {
        return None;
    }
    let score = |r: &&Rendition| {
        let l = lang.is_some_and(|l| r.lang.as_deref().is_some_and(|x| x.to_ascii_lowercase().starts_with(&l.to_ascii_lowercase())));
        (l as u8, r.default as u8, r.autoselect as u8)
    };
    c.sort_by_key(|r| std::cmp::Reverse(score(r)));
    c.into_iter().next()
}

/// Every variant, described, best first.
pub fn available(m: &Master) -> Vec<String> {
    let mut idx: Vec<usize> = (0..m.variants.len()).collect();
    idx.sort_by_key(|&i| std::cmp::Reverse((m.variants[i].height, m.variants[i].bandwidth)));
    let mut out: Vec<String> = Vec::new();
    for i in idx {
        let d = describe(&m.variants[i]);
        if !out.contains(&d) {
            out.push(d);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MASTER: &str = "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"English\",LANGUAGE=\"en\",DEFAULT=YES,URI=\"audio/en.m3u8\"\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"Deutsch\",LANGUAGE=\"de\",URI=\"audio/de.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360,CODECS=\"avc1.4d401e,mp4a.40.2\",AUDIO=\"aud\"\nlow/index.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=2800000,RESOLUTION=1280x720,CODECS=\"avc1.64001f,mp4a.40.2\",AUDIO=\"aud\",FRAME-RATE=29.97\nhigh/index.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=64000,CODECS=\"mp4a.40.2\"\nonly-audio.m3u8\n";

    #[test]
    fn a_master_playlist_is_read_and_chosen_from() {
        let Playlist::Master(m) = parse("https://e.com/v/master.m3u8", MASTER).unwrap() else { panic!() };
        assert_eq!(m.variants.len(), 3);
        assert_eq!(m.variants[1].uri, "https://e.com/v/high/index.m3u8");
        assert_eq!(m.variants[1].height, 720);
        let best = choose_variant(&m, Quality::Best).unwrap();
        assert_eq!(m.variants[best].height, 720);
        assert_eq!(m.variants[choose_variant(&m, Quality::Lowest).unwrap()].height, 360);
        assert_eq!(m.variants[choose_variant(&m, Quality::Height(480)).unwrap()].height, 360);
        assert_eq!(m.variants[choose_variant(&m, Quality::Height(1080)).unwrap()].height, 720);
        assert_eq!(m.variants[choose_variant(&m, Quality::Height(144)).unwrap()].height, 360);
        assert_eq!(m.variants[choose_variant(&m, Quality::AudioOnly).unwrap()].uri, "https://e.com/v/only-audio.m3u8");
        let a = choose_audio(&m, &m.variants[best], None).unwrap();
        assert_eq!(a.uri.as_deref(), Some("https://e.com/v/audio/en.m3u8"));
        let de = choose_audio(&m, &m.variants[best], Some("de")).unwrap();
        assert_eq!(de.uri.as_deref(), Some("https://e.com/v/audio/de.m3u8"));
        assert_eq!(describe(&m.variants[best]), "1280x720, 29.97 fps, 2.8 Mbit/s");
        assert_eq!(available(&m)[0], "1280x720, 29.97 fps, 2.8 Mbit/s");
    }

    #[test]
    fn a_media_playlist_keeps_keys_maps_ranges_and_breaks() {
        let t = "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXT-X-MEDIA-SEQUENCE:7\n#EXT-X-KEY:METHOD=AES-128,URI=\"k.bin\",IV=0x000102030405060708090a0b0c0d0e0f\n#EXT-X-MAP:URI=\"init.mp4\",BYTERANGE=\"720@0\"\n#EXTINF:6.0,\n#EXT-X-BYTERANGE:1000@720\na.m4s\n#EXTINF:5.5,\n#EXT-X-BYTERANGE:900\na.m4s\n#EXT-X-DISCONTINUITY\n#EXT-X-KEY:METHOD=NONE\n#EXTINF:4,\nb.ts\n#EXT-X-ENDLIST\n";
        let Playlist::Media(m) = parse("https://e.com/p/i.m3u8", t).unwrap() else { panic!() };
        assert_eq!(m.segments.len(), 3);
        assert!(m.endlist);
        assert_eq!(m.segments[0].seq, 7);
        assert_eq!(m.segments[0].range, Some((720, 1719)));
        assert_eq!(m.segments[1].range, Some((1720, 2619)));
        assert!(m.segments[2].disc && m.segments[2].key.is_none());
        let k = &m.keys[m.segments[0].key.unwrap()];
        assert_eq!(k.method, KeyMethod::Aes128);
        assert_eq!(k.uri.as_deref(), Some("https://e.com/p/k.bin"));
        assert_eq!(k.iv.unwrap()[15], 15);
        assert_eq!(m.maps[0].range, Some((0, 719)));
        assert!((m.seconds() - 15.5).abs() < 1e-9);
        assert!(m.protection().is_none());
        let drm = "#EXTM3U\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"skd://abc\",KEYFORMAT=\"com.apple.streamingkeydelivery\"\n#EXTINF:6,\na.ts\n";
        let Playlist::Media(m) = parse("https://e.com/i.m3u8", drm).unwrap() else { panic!() };
        assert!(m.protection().unwrap().contains("copy protection"));
        assert!(parse("https://e.com/", "<html>").is_err());
    }
}
