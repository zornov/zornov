// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! DASH manifests (MPD): the representations of the main period and the
//! list of pieces to fetch for each (SegmentTemplate with a number, a time
//! or a timeline, SegmentList, SegmentBase / a plain BaseURL).

use crate::error::{Error, Result};
use crate::types::Quality;
use crate::url::join;
use crate::xml::{self, Node};

#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub url: String,
    pub range: Option<(u64, u64)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    /// One file holds it all (SegmentBase, or just a BaseURL).
    Single(Chunk),
    Segments { init: Option<Chunk>, segs: Vec<Chunk> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Video,
    Audio,
    Other,
}

#[derive(Debug, Clone)]
pub struct Rep {
    #[cfg_attr(not(test), allow(dead_code))]
    pub id: String,
    pub kind: Kind,
    pub bandwidth: u64,
    pub width: u32,
    pub height: u32,
    #[allow(dead_code)]
    pub codecs: String,
    pub lang: Option<String>,
    pub protected: bool,
    pub trick: bool,
    pub frame_rate: Option<String>,
    pub plan: Plan,
}

#[derive(Debug, Clone)]
pub struct Mpd {
    pub reps: Vec<Rep>,
    pub dynamic: bool,
    /// Periods in the manifest (only the longest is used).
    pub periods: usize,
}

/// "PT1H2M3.5S" in seconds.
pub fn iso_duration(s: &str) -> Option<f64> {
    let s = s.trim();
    let rest = s.strip_prefix('P')?;
    let (date, time) = rest.split_once('T').unwrap_or((rest, ""));
    let mut total = 0.0;
    let mut num = String::new();
    for c in date.chars() {
        match c {
            '0'..='9' | '.' => num.push(c),
            'Y' => total += num.parse::<f64>().ok()? * 365.0 * 86400.0,
            'M' => total += num.parse::<f64>().ok()? * 30.0 * 86400.0,
            'W' => total += num.parse::<f64>().ok()? * 7.0 * 86400.0,
            'D' => total += num.parse::<f64>().ok()? * 86400.0,
            _ => return None,
        }
        if !c.is_ascii_digit() && c != '.' {
            num.clear();
        }
    }
    for c in time.chars() {
        match c {
            '0'..='9' | '.' => num.push(c),
            'H' => total += num.parse::<f64>().ok()? * 3600.0,
            'M' => total += num.parse::<f64>().ok()? * 60.0,
            'S' => total += num.parse::<f64>().ok()?,
            _ => return None,
        }
        if !c.is_ascii_digit() && c != '.' {
            num.clear();
        }
    }
    Some(total)
}

/// `$Number$`, `$Number%05d$`, `$Time$`, `$Bandwidth$`, `$RepresentationID$`, `$$`.
pub fn substitute(t: &str, id: &str, bandwidth: u64, number: u64, time: u64) -> String {
    let mut out = String::new();
    let parts: Vec<&str> = t.split('$').collect();
    for (i, p) in parts.iter().enumerate() {
        if i % 2 == 0 {
            out.push_str(p);
            continue;
        }
        if p.is_empty() {
            out.push('$');
            continue;
        }
        let (name, fmt) = p.split_once('%').map_or((*p, None), |(n, f)| (n, Some(f)));
        let value = match name {
            "RepresentationID" => {
                out.push_str(id);
                continue;
            }
            "Number" => number,
            "Bandwidth" => bandwidth,
            "Time" => time,
            _ => {
                // Not a variable: leave it as written.
                out.push('$');
                out.push_str(p);
                out.push('$');
                continue;
            }
        };
        let width = fmt.and_then(|f| f.strip_suffix('d')).and_then(|w| w.trim_start_matches('0').parse::<usize>().ok()).unwrap_or(0);
        out.push_str(&format!("{value:0width$}"));
    }
    out
}

#[derive(Default, Clone)]
struct Template {
    media: Option<String>,
    init: Option<String>,
    start_number: Option<u64>,
    timescale: Option<u64>,
    duration: Option<u64>,
    pto: Option<u64>,
    timeline: Option<Vec<(Option<u64>, u64, i64)>>,
}

fn template_over(t: &mut Template, n: &Node) {
    if let Some(v) = n.attr("media") {
        t.media = Some(v.to_owned());
    }
    if let Some(v) = n.attr("initialization") {
        t.init = Some(v.to_owned());
    }
    if let Some(v) = n.attr("startNumber").and_then(|v| v.parse().ok()) {
        t.start_number = Some(v);
    }
    if let Some(v) = n.attr("timescale").and_then(|v| v.parse().ok()) {
        t.timescale = Some(v);
    }
    if let Some(v) = n.attr("duration").and_then(|v| v.parse().ok()) {
        t.duration = Some(v);
    }
    if let Some(v) = n.attr("presentationTimeOffset").and_then(|v| v.parse().ok()) {
        t.pto = Some(v);
    }
    if let Some(tl) = n.kid("SegmentTimeline") {
        t.timeline = Some(tl.kids("S").filter_map(|s| Some((s.attr("t").and_then(|v| v.parse().ok()), s.attr("d")?.parse().ok()?, s.attr("r").and_then(|v| v.parse().ok()).unwrap_or(0)))).collect());
    }
}

fn range_of(s: &str) -> Option<(u64, u64)> {
    let (a, b) = s.split_once('-')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

fn base_url(cur: &str, n: &Node) -> String {
    match n.kid("BaseURL").map(|b| b.text.trim()).filter(|t| !t.is_empty()) {
        Some(b) => join(cur, b).unwrap_or_else(|| cur.to_owned()),
        None => cur.to_owned(),
    }
}

fn has_prop(n: &Node, scheme_part: &str) -> bool {
    n.children.iter().any(|c| matches!(c.name.as_str(), "EssentialProperty" | "SupplementalProperty") && c.attr("schemeIdUri").is_some_and(|s| s.to_ascii_lowercase().contains(scheme_part)))
        || n.kids("Role").any(|r| r.attr("value").is_some_and(|v| v.eq_ignore_ascii_case("trick")))
}

pub fn parse(manifest_url: &str, text: &str) -> Result<Mpd> {
    let root = xml::parse(text).map_err(|e| Error::Bad(format!("the manifest cannot be read: {e}")))?;
    if root.name != "MPD" {
        return Err(Error::Bad("that is not a DASH manifest (no MPD element)".into()));
    }
    let dynamic = root.attr("type").is_some_and(|t| t.eq_ignore_ascii_case("dynamic"));
    let total = root.attr("mediaPresentationDuration").and_then(iso_duration);
    let periods: Vec<&Node> = root.kids("Period").collect();
    if periods.is_empty() {
        return Err(Error::Bad("the manifest has no periods".into()));
    }
    // Period lengths: its own duration, else up to the next one, else to the end.
    let starts: Vec<f64> = periods.iter().scan(0.0, |acc, p| {
        if let Some(s) = p.attr("start").and_then(iso_duration) {
            *acc = s;
        }
        let at = *acc;
        *acc += p.attr("duration").and_then(iso_duration).unwrap_or(0.0);
        Some(at)
    }).collect();
    let lens: Vec<Option<f64>> = periods.iter().enumerate().map(|(i, p)| p.attr("duration").and_then(iso_duration).or_else(|| starts.get(i + 1).map(|n| n - starts[i]).or(total.map(|t| t - starts[i])).filter(|d| *d > 0.0))).collect();
    let main = (0..periods.len()).max_by(|&a, &b| lens[a].unwrap_or(0.0).partial_cmp(&lens[b].unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(0);
    let period = periods[main];
    let plen = lens[main];
    let b0 = base_url(manifest_url, &root);
    let b1 = base_url(&b0, period);
    let mut reps = Vec::new();
    for aset in period.kids("AdaptationSet") {
        let b2 = base_url(&b1, aset);
        let aset_mime = aset.attr("mimeType").unwrap_or("");
        let aset_type = aset.attr("contentType").unwrap_or("");
        let aset_protected = aset.kid("ContentProtection").is_some();
        let aset_trick = has_prop(aset, "trickmode");
        for rep in aset.kids("Representation") {
            let mime = rep.attr("mimeType").unwrap_or(aset_mime);
            let codecs = rep.attr("codecs").or(aset.attr("codecs")).unwrap_or("").to_owned();
            let ctype = if !aset_type.is_empty() { aset_type.to_owned() } else { mime.split('/').next().unwrap_or("").to_owned() };
            let kind = match ctype.as_str() {
                "video" => Kind::Video,
                "audio" => Kind::Audio,
                "" => {
                    if codecs.contains("avc") || codecs.contains("hvc") || codecs.contains("hev") || codecs.contains("av01") || codecs.contains("vp0") || codecs.contains("vp9") {
                        Kind::Video
                    } else if codecs.contains("mp4a") || codecs.contains("opus") || codecs.contains("ac-3") || codecs.contains("ec-3") || codecs.contains("flac") {
                        Kind::Audio
                    } else {
                        Kind::Other
                    }
                }
                _ => Kind::Other,
            };
            if kind == Kind::Other {
                continue;
            }
            let id = rep.attr("id").unwrap_or("").to_owned();
            let bandwidth = rep.attr("bandwidth").and_then(|v| v.parse().ok()).unwrap_or(0);
            let b3 = base_url(&b2, rep);
            let mut tmpl = Template::default();
            for n in [period, aset, rep] {
                if let Some(t) = n.kid("SegmentTemplate") {
                    template_over(&mut tmpl, t);
                }
            }
            let plan = if let Some(media) = tmpl.media.clone() {
                let ts = tmpl.timescale.unwrap_or(1).max(1);
                let start_number = tmpl.start_number.unwrap_or(1);
                let mut times: Vec<(u64, u64)> = Vec::new(); // (number, time)
                if let Some(tl) = &tmpl.timeline {
                    let mut t = 0u64;
                    let mut n = start_number;
                    let end = plen.map(|p| (p * ts as f64) as u64 + tmpl.pto.unwrap_or(0));
                    for (i, (st, d, r)) in tl.iter().enumerate() {
                        if let Some(st) = st {
                            t = *st;
                        } else if i == 0 {
                            t = tmpl.pto.unwrap_or(0);
                        }
                        let repeats = if *r >= 0 {
                            *r as u64
                        } else {
                            // Until the next entry starts, or the period ends.
                            let stop = tl.get(i + 1).and_then(|x| x.0).or(end).ok_or_else(|| Error::Unsupported("This is a live stream with no end; Studio saves recorded videos, not live ones.".into()))?;
                            (stop.saturating_sub(t) / (*d).max(1)).saturating_sub(1)
                        };
                        for _ in 0..=repeats {
                            times.push((n, t));
                            n += 1;
                            t += d;
                            if times.len() > 200_000 {
                                return Err(Error::Bad("the manifest lists an unreasonable number of segments".into()));
                            }
                        }
                    }
                } else {
                    let d = tmpl.duration.ok_or_else(|| Error::Bad("the manifest gives no segment length".into()))?.max(1);
                    let secs = plen.or(total).ok_or_else(|| Error::Unsupported("This is a live stream with no end; Studio saves recorded videos, not live ones.".into()))?;
                    let count = ((secs * ts as f64) / d as f64).ceil() as u64;
                    if count > 200_000 {
                        return Err(Error::Bad("the manifest lists an unreasonable number of segments".into()));
                    }
                    for i in 0..count {
                        times.push((start_number + i, tmpl.pto.unwrap_or(0) + i * d));
                    }
                }
                let seg = |n: u64, t: u64| -> Result<Chunk> {
                    let u = substitute(&media, &id, bandwidth, n, t);
                    Ok(Chunk { url: join(&b3, &u).ok_or_else(|| Error::Bad(format!("a segment address cannot be used: {u}")))?, range: None })
                };
                let segs = times.iter().map(|&(n, t)| seg(n, t)).collect::<Result<Vec<_>>>()?;
                let init = match &tmpl.init {
                    Some(i) => {
                        let u = substitute(i, &id, bandwidth, 0, 0);
                        Some(Chunk { url: join(&b3, &u).ok_or_else(|| Error::Bad(format!("an init address cannot be used: {u}")))?, range: None })
                    }
                    None => None,
                };
                Plan::Segments { init, segs }
            } else if let Some(list) = [rep, aset, period].iter().find_map(|n| n.kid("SegmentList")) {
                let init = list.kid("Initialization").and_then(|i| {
                    let url = i.attr("sourceURL").and_then(|u| join(&b3, u)).unwrap_or_else(|| b3.clone());
                    Some(Chunk { url, range: i.attr("range").and_then(range_of) })
                });
                let segs: Vec<Chunk> = list
                    .kids("SegmentURL")
                    .filter_map(|s| {
                        let url = match s.attr("media") {
                            Some(m) => join(&b3, m)?,
                            None => b3.clone(),
                        };
                        Some(Chunk { url, range: s.attr("mediaRange").and_then(range_of) })
                    })
                    .collect();
                if segs.is_empty() {
                    continue;
                }
                Plan::Segments { init, segs }
            } else if crate::url::is_http(&b3) && b3 != manifest_url {
                // SegmentBase, or just a BaseURL: the whole file.
                Plan::Single(Chunk { url: b3.clone(), range: None })
            } else {
                continue;
            };
            reps.push(Rep {
                id,
                kind,
                bandwidth,
                width: rep.attr("width").or(aset.attr("width")).and_then(|v| v.parse().ok()).unwrap_or(0),
                height: rep.attr("height").or(aset.attr("height")).and_then(|v| v.parse().ok()).unwrap_or(0),
                codecs,
                lang: aset.attr("lang").or(rep.attr("lang")).map(str::to_owned),
                protected: aset_protected || rep.kid("ContentProtection").is_some(),
                trick: aset_trick || has_prop(rep, "trickmode"),
                frame_rate: rep.attr("frameRate").or(aset.attr("frameRate")).map(str::to_owned),
                plan,
            });
        }
    }
    Ok(Mpd { reps, dynamic, periods: periods.len() })
}

pub fn describe(r: &Rep) -> String {
    let mut parts = Vec::new();
    match r.kind {
        Kind::Video if r.height > 0 => parts.push(format!("{}x{}", r.width, r.height)),
        Kind::Audio => parts.push("sound".into()),
        _ => {}
    }
    if let Some(f) = r.frame_rate.as_deref().and_then(|f| match f.split_once('/') {
        Some((a, b)) => Some(a.parse::<f64>().ok()? / b.parse::<f64>().ok()?),
        None => f.parse().ok(),
    }) {
        parts.push(format!("{} fps", (f * 100.0).round() / 100.0));
    }
    if r.bandwidth > 0 {
        parts.push(format!("{:.1} Mbit/s", r.bandwidth as f64 / 1e6));
    }
    if parts.is_empty() { "the only stream".into() } else { parts.join(", ") }
}

/// Every video representation described, best first.
pub fn available(m: &Mpd) -> Vec<String> {
    let mut v: Vec<&Rep> = m.reps.iter().filter(|r| r.kind == Kind::Video && !r.trick).collect();
    v.sort_by_key(|r| std::cmp::Reverse((r.height, r.bandwidth)));
    let mut out: Vec<String> = Vec::new();
    for r in v {
        let d = describe(r);
        if !out.contains(&d) {
            out.push(d);
        }
    }
    out
}

/// The video and sound representations to take.
pub fn choose<'a>(m: &'a Mpd, q: Quality, lang: Option<&str>) -> Result<(Option<&'a Rep>, Option<&'a Rep>)> {
    let usable = |k: Kind| -> Vec<&'a Rep> { m.reps.iter().filter(|r| r.kind == k && !r.trick).collect() };
    let (video, audio) = (usable(Kind::Video), usable(Kind::Audio));
    if video.is_empty() && audio.is_empty() {
        return Err(Error::NotExposed("The manifest lists no video or sound Studio can take.".into()));
    }
    let open_v: Vec<&Rep> = video.iter().copied().filter(|r| !r.protected).collect();
    let open_a: Vec<&Rep> = audio.iter().copied().filter(|r| !r.protected).collect();
    if open_v.is_empty() && open_a.is_empty() {
        return Err(Error::Protected("This video is protected (the manifest asks for DRM, such as Widevine, PlayReady or FairPlay). Studio does not work around copy protection, so it cannot be downloaded.".into()));
    }
    let rank = |r: &&Rep| (r.height, r.bandwidth);
    let v = if q == Quality::AudioOnly {
        None
    } else {
        match q {
            Quality::Lowest => open_v.iter().copied().min_by_key(|r| (r.bandwidth, rank(r))),
            Quality::Height(h) => {
                let under: Vec<&Rep> = open_v.iter().copied().filter(|r| r.height <= h).collect();
                if under.is_empty() { open_v.iter().copied().min_by_key(rank) } else { under.into_iter().max_by_key(rank) }
            }
            _ => open_v.iter().copied().max_by_key(rank),
        }
    };
    let score = |r: &&Rep| {
        let l = lang.is_some_and(|l| r.lang.as_deref().is_some_and(|x| x.to_ascii_lowercase().starts_with(&l.to_ascii_lowercase())));
        (l as u8, if q == Quality::Lowest { u64::MAX - r.bandwidth } else { r.bandwidth })
    };
    let a = open_a.iter().copied().max_by_key(score);
    if v.is_none() && a.is_none() {
        return Err(Error::Protected("This video is protected (the manifest asks for DRM). Studio does not work around copy protection, so it cannot be downloaded.".into()));
    }
    Ok((v, a))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_and_templates() {
        assert_eq!(iso_duration("PT1H2M3.5S"), Some(3723.5));
        assert_eq!(iso_duration("PT10S"), Some(10.0));
        assert_eq!(iso_duration("P1DT1M"), Some(86460.0));
        assert_eq!(iso_duration("nope"), None);
        assert_eq!(substitute("seg-$RepresentationID$-$Number%05d$.m4s?b=$Bandwidth$&$$", "v1", 900, 7, 0), "seg-v1-00007.m4s?b=900&$");
        assert_eq!(substitute("t$Time$.m4s", "x", 0, 0, 12345), "t12345.m4s");
    }

    const TEMPLATE: &str = r#"<MPD type="static" mediaPresentationDuration="PT20S" xmlns="urn:mpeg:dash:schema:mpd:2011">
<BaseURL>media/</BaseURL>
<Period>
 <AdaptationSet contentType="video" mimeType="video/mp4" segmentAlignment="true">
  <SegmentTemplate timescale="1000" duration="4000" startNumber="1" initialization="$RepresentationID$/init.mp4" media="$RepresentationID$/s$Number$.m4s"/>
  <Representation id="v360" bandwidth="500000" width="640" height="360" codecs="avc1.4d401e"/>
  <Representation id="v720" bandwidth="2500000" width="1280" height="720" codecs="avc1.64001f" frameRate="30000/1001"/>
 </AdaptationSet>
 <AdaptationSet contentType="audio" lang="en" mimeType="audio/mp4">
  <Representation id="a1" bandwidth="128000" codecs="mp4a.40.2">
   <SegmentTemplate timescale="48000" initialization="a/init.mp4" media="a/$Time$.m4s"><SegmentTimeline><S t="0" d="192000" r="-1"/></SegmentTimeline></SegmentTemplate>
  </Representation>
 </AdaptationSet>
</Period></MPD>"#;

    #[test]
    fn templates_numbers_and_timelines() {
        let m = parse("https://cdn.test/vod/manifest.mpd", TEMPLATE).unwrap();
        assert_eq!(m.reps.len(), 3);
        let v = m.reps.iter().find(|r| r.id == "v720").unwrap();
        let Plan::Segments { init, segs } = &v.plan else { panic!() };
        assert_eq!(init.as_ref().unwrap().url, "https://cdn.test/vod/media/v720/init.mp4");
        assert_eq!(segs.len(), 5);
        assert_eq!(segs[4].url, "https://cdn.test/vod/media/v720/s5.m4s");
        let a = m.reps.iter().find(|r| r.id == "a1").unwrap();
        let Plan::Segments { segs, .. } = &a.plan else { panic!() };
        assert_eq!(segs.len(), 5);
        assert_eq!(segs[2].url, "https://cdn.test/vod/media/a/384000.m4s");
        let (v, a) = choose(&m, Quality::Best, None).unwrap();
        assert_eq!(v.unwrap().id, "v720");
        assert_eq!(a.unwrap().id, "a1");
        assert_eq!(choose(&m, Quality::Height(400), None).unwrap().0.unwrap().id, "v360");
        assert!(choose(&m, Quality::AudioOnly, None).unwrap().0.is_none());
        assert_eq!(describe(v.unwrap()), "1280x720, 29.97 fps, 2.5 Mbit/s");
    }

    #[test]
    fn lists_bases_and_protection() {
        let list = r#"<MPD mediaPresentationDuration="PT8S"><Period><AdaptationSet mimeType="video/mp4"><Representation id="1" bandwidth="1" height="240" width="320">
<BaseURL>v.mp4</BaseURL><SegmentList duration="4"><Initialization range="0-99"/><SegmentURL mediaRange="100-999"/><SegmentURL mediaRange="1000-1999"/></SegmentList></Representation>
<Representation id="2" bandwidth="2" height="480" width="640"><BaseURL>whole.mp4</BaseURL><SegmentBase indexRange="0-50"/></Representation></AdaptationSet></Period></MPD>"#;
        let m = parse("https://c.test/d/m.mpd", list).unwrap();
        let Plan::Segments { init, segs } = &m.reps[0].plan else { panic!() };
        assert_eq!(init.as_ref().unwrap().range, Some((0, 99)));
        assert_eq!(segs[1], Chunk { url: "https://c.test/d/v.mp4".into(), range: Some((1000, 1999)) });
        assert_eq!(m.reps[1].plan, Plan::Single(Chunk { url: "https://c.test/d/whole.mp4".into(), range: None }));
        let drm = r#"<MPD mediaPresentationDuration="PT8S"><Period><AdaptationSet mimeType="video/mp4"><ContentProtection schemeIdUri="urn:uuid:edef8ba9-79d6-4ace-a3c8-27dcd51d21ed"/><Representation id="1" bandwidth="1"><BaseURL>v.mp4</BaseURL></Representation></AdaptationSet></Period></MPD>"#;
        let m = parse("https://c.test/m.mpd", drm).unwrap();
        assert!(matches!(choose(&m, Quality::Best, None), Err(Error::Protected(_))));
        let live = r#"<MPD type="dynamic"><Period><AdaptationSet mimeType="video/mp4"><Representation id="1"><SegmentTemplate media="s$Number$.m4s" duration="2"/></Representation></AdaptationSet></Period></MPD>"#;
        assert!(matches!(parse("https://c.test/m.mpd", live), Err(Error::Unsupported(_))));
    }
}
