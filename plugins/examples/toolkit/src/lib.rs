// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Example Studio plugin with one of every other kind of item:
//!
//! - Plasma: an animated generator (CPU and WGSL).
//! - Iris: a circular transition (CPU and WGSL).
//! - Netpbm picture: imports `.ppm`, `.pgm` and `.pbm` files.
//! - Marker list: imports `.markers` text files (`seconds note` per line)
//!   as timeline markers.
//! - Clip list (CSV): exports every clip as a spreadsheet row.
//! - Shot list: a panel that counts the shots and can mark each one.

use studio_plugin_api::*;

// ------------------------------------------------------------ generator

pub struct Plasma;

impl Generator for Plasma {
    fn info(&self) -> EffectInfo {
        EffectInfo::new("plasma", "Plasma")
            .description("Flowing colour plasma, for backgrounds.")
            .param(Param::float("scale", "Scale", 0.1, 8.0, 1.5))
            .param(Param::float("speed", "Speed", 0.0, 6.0, 1.0))
            .param(Param::colour("colour", "Colour", [0.2, 0.6, 1.0, 1.0]))
            .param(Param::float("mix", "Opacity", 0.0, 1.0, 1.0))
            .animated()
            .wgsl(include_str!("plasma.wgsl"))
    }

    fn generate(&self, ctx: &Context, frame: &mut Frame, p: &Params) {
        let (scale, speed, col, m) = (p.float("scale"), p.float("speed"), p.colour("colour"), p.float("mix"));
        let h = frame.height as f32;
        let t = ctx.time * speed;
        let w = frame.width;
        for (i, px) in frame.pixels.iter_mut().enumerate() {
            let (x, y) = (((i % w) as f32 + 0.5) / h * scale, ((i / w) as f32 + 0.5) / h * scale);
            let r = ((x - 0.5) * (x - 0.5) + (y - 0.5) * (y - 0.5)).sqrt();
            let v = (x * 6.0 + t).sin() + ((y * 5.0 - t) * 1.3).sin() + ((x + y) * 4.0 + t * 0.7).sin() + (r * 9.0 - t * 1.6).sin();
            let k = 0.5 + 0.5 * (v * std::f32::consts::FRAC_PI_2).sin();
            let mut over = [0.0; 4];
            for c in 0..3 {
                over[c] = col[c] * (0.25 + 0.75 * k) + 0.08 * (1.0 - k);
            }
            over[3] = col[3];
            for c in 0..4 {
                let o = over[c] + px[c] * (1.0 - over[3]);
                px[c] += (o - px[c]) * m;
            }
        }
    }
}

// ----------------------------------------------------------- transition

pub struct Iris;

impl Transition for Iris {
    fn info(&self) -> EffectInfo {
        EffectInfo::new("iris", "Iris")
            .description("The next shot opens in a growing circle.")
            .param(Param::point("center", "Centre", [0.5, 0.5]))
            .param(Param::float("softness", "Softness", 0.0, 20.0, 2.0))
            .wgsl(include_str!("iris.wgsl"))
    }

    fn apply(&self, ctx: &Context, a: &mut Frame, b: &Frame, p: &Params) {
        let (w, h) = (a.width as f32, a.height as f32);
        let c = p.point("center");
        let (cx, cy) = (c[0] * w, c[1] * h);
        let corner = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)].iter().map(|(x, y)| ((cx - x) * (cx - x) + (cy - y) * (cy - y)).sqrt()).fold(0.0f32, f32::max);
        let soft = (p.float("softness") * 0.01 * h).max(0.5);
        let r = ctx.progress * (corner + soft);
        let wi = a.width;
        for (i, px) in a.pixels.iter_mut().enumerate() {
            let (x, y) = ((i % wi) as f32 + 0.5, (i / wi) as f32 + 0.5);
            let d = ((x - cx) * (x - cx) + (y - cy) * (y - cy)).sqrt();
            let k = ((r - d) / soft).clamp(0.0, 1.0);
            let q = b.pixels[i];
            for ch in 0..4 {
                px[ch] += (q[ch] - px[ch]) * k;
            }
        }
    }
}

// ------------------------------------------------------------ importers

pub struct Netpbm;

impl Importer for Netpbm {
    fn info(&self) -> ToolInfo {
        ToolInfo::new("netpbm", "Netpbm picture").description("Portable pixmap, graymap and bitmap pictures (P1-P6).").extension("ppm").extension("pgm").extension("pbm").extension("pnm")
    }

    fn import(&self, _name: &str, bytes: &[u8]) -> Result<Imported, String> {
        decode_netpbm(bytes)
    }
}

/// Reads any Netpbm picture into straight sRGB RGBA8.
pub fn decode_netpbm(b: &[u8]) -> Result<Imported, String> {
    let mut i = 0;
    let token = |i: &mut usize| -> Result<String, String> {
        loop {
            while *i < b.len() && b[*i].is_ascii_whitespace() {
                *i += 1;
            }
            if *i < b.len() && b[*i] == b'#' {
                while *i < b.len() && b[*i] != b'\n' {
                    *i += 1;
                }
                continue;
            }
            break;
        }
        let s = *i;
        while *i < b.len() && !b[*i].is_ascii_whitespace() {
            *i += 1;
        }
        if s == *i { Err("the file ends early".into()) } else { Ok(String::from_utf8_lossy(&b[s..*i]).into_owned()) }
    };
    let magic = token(&mut i)?;
    let num = |s: String| s.parse::<u32>().map_err(|_| format!("bad number `{s}`"));
    let w = num(token(&mut i)?)?;
    let h = num(token(&mut i)?)?;
    if w == 0 || h == 0 || w > 16384 || h > 16384 {
        return Err(format!("unsupported size {w}x{h}"));
    }
    let max = if magic == "P1" || magic == "P4" { 1 } else { num(token(&mut i)?)?.max(1) };
    let n = (w * h) as usize;
    let chans = match magic.as_str() {
        "P3" | "P6" => 3,
        "P1" | "P2" | "P4" | "P5" => 1,
        other => return Err(format!("not a Netpbm picture ({other})")),
    };
    let mut vals: Vec<u32> = Vec::with_capacity(n * chans);
    match magic.as_str() {
        "P1" | "P2" | "P3" => {
            for _ in 0..n * chans {
                if magic == "P1" {
                    // Bitmaps may run the digits together.
                    while i < b.len() && !matches!(b[i], b'0' | b'1') {
                        i += 1;
                    }
                    vals.push(*b.get(i).ok_or("the file ends early")? as u32 - b'0' as u32);
                    i += 1;
                } else {
                    vals.push(num(token(&mut i)?)?);
                }
            }
        }
        "P4" => {
            i += 1;
            let row = (w as usize).div_ceil(8);
            for y in 0..h as usize {
                for x in 0..w as usize {
                    let byte = *b.get(i + y * row + x / 8).ok_or("the file ends early")?;
                    vals.push(((byte >> (7 - x % 8)) & 1) as u32);
                }
            }
        }
        _ => {
            i += 1;
            let wide = max > 255;
            for k in 0..n * chans {
                let v = if wide {
                    let o = i + 2 * k;
                    u16::from_be_bytes([*b.get(o).ok_or("the file ends early")?, *b.get(o + 1).ok_or("the file ends early")?]) as u32
                } else {
                    *b.get(i + k).ok_or("the file ends early")? as u32
                };
                vals.push(v);
            }
        }
    }
    let to8 = |v: u32| ((v.min(max) as f32 / max as f32) * 255.0 + 0.5) as u8;
    let mut rgba = Vec::with_capacity(n * 4);
    for p in 0..n {
        let (r, g, bl) = if chans == 3 {
            (to8(vals[p * 3]), to8(vals[p * 3 + 1]), to8(vals[p * 3 + 2]))
        } else {
            // Bitmaps: 1 is black.
            let v = if max == 1 && (magic == "P1" || magic == "P4") { 255 - to8(vals[p]) } else { to8(vals[p]) };
            (v, v, v)
        };
        rgba.extend_from_slice(&[r, g, bl, 255]);
    }
    Ok(Imported::Image { width: w, height: h, rgba8: rgba })
}

pub struct MarkerList;

impl Importer for MarkerList {
    fn info(&self) -> ToolInfo {
        ToolInfo::new("markers", "Marker list").description("A text file with `seconds note` (or `mm:ss note`) on each line, as timeline markers.").extension("markers")
    }

    fn import(&self, _name: &str, bytes: &[u8]) -> Result<Imported, String> {
        let text = std::str::from_utf8(bytes).map_err(|_| "the file is not text")?;
        let mut ops = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (t, note) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
            let secs = parse_time(t).ok_or_else(|| format!("line {}: `{t}` is not a time", n + 1))?;
            ops.push(Json::obj().with("op", "marker").with("at", secs).with("note", note.trim()));
        }
        if ops.is_empty() {
            return Err("no markers in the file".into());
        }
        Ok(Imported::Ops(Json::Arr(ops)))
    }
}

/// `12.5`, `1:02.5` or `1:00:02`.
fn parse_time(s: &str) -> Option<f64> {
    s.split(':').try_fold(0.0, |acc, part| part.parse::<f64>().ok().filter(|v| *v >= 0.0).map(|v| acc * 60.0 + v))
}

// ------------------------------------------------------------- exporter

pub struct ClipCsv;

impl Exporter for ClipCsv {
    fn info(&self) -> ToolInfo {
        ToolInfo::new("clip_csv", "Clip list (CSV)").description("Every clip as a spreadsheet row: id, name, kind, lane, start, end, length, effects.").extension("csv")
    }

    fn export(&self, project: &Json) -> Result<Vec<u8>, String> {
        let mut out = String::from("id,name,kind,lane,start,end,duration,effects\n");
        let field = |s: &str| if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_owned() };
        for c in project.get("clips").items() {
            let start = c.get("start").as_f64().unwrap_or(0.0);
            let dur = c.get("dur").as_f64().unwrap_or(0.0);
            let fx: Vec<&str> = c.get("effects").items().iter().filter_map(|e| e.as_str().or(e.get("kind").as_str())).collect();
            out.push_str(&format!(
                "{},{},{},{},{:.3},{:.3},{:.3},{}\n",
                c.get("id").as_f64().unwrap_or(0.0) as i64,
                field(c.get("name").as_str().unwrap_or("")),
                field(c.get("kind").as_str().unwrap_or("")),
                c.get("lane").as_f64().unwrap_or(0.0) as i64,
                start,
                start + dur,
                dur,
                field(&fx.join("; ")),
            ));
        }
        Ok(out.into_bytes())
    }
}

// ---------------------------------------------------------------- panel

pub struct ShotList;

impl Panel for ShotList {
    fn info(&self) -> ToolInfo {
        ToolInfo::new("shot_list", "Shot list").description("Counts the shots on the main lane and marks each one.")
    }

    fn update(&self, event: &PanelEvent, project: &Json) -> PanelView {
        let min = event.value("min_len").unwrap_or(0.0) as f64;
        let shots: Vec<&Json> = project.get("clips").items().iter().filter(|c| c.get("lane").as_f64() == Some(0.0) && c.get("dur").as_f64().unwrap_or(0.0) >= min).collect();
        let total: f64 = shots.iter().filter_map(|c| c.get("dur").as_f64()).sum();
        let mut view = PanelView {
            widgets: vec![
                Widget::Heading("Shot list".into()),
                Widget::Label(format!("{} shots on the main lane, {:.1} s in all", shots.len(), total)),
                Widget::Slider { id: "min_len".into(), label: "Shortest shot to count (s)".into(), min: 0.0, max: 10.0, value: min as f32 },
                Widget::Separator,
                Widget::Button { id: "mark".into(), label: "Mark each shot".into() },
            ],
            ..Default::default()
        };
        if event.button.as_deref() == Some("mark") {
            let ops: Vec<Json> = shots
                .iter()
                .enumerate()
                .map(|(i, c)| Json::obj().with("op", "marker").with("at", c.get("start").as_f64().unwrap_or(0.0)).with("note", format!("Shot {}: {}", i + 1, c.get("name").as_str().unwrap_or(""))))
                .collect();
            view.message = Some(format!("Marked {} shots", ops.len()));
            view.ops = Some(Json::Arr(ops));
        }
        view
    }
}

pub fn plugin() -> Plugin {
    Plugin::new("toolkit", "Toolkit", "1.0.0")
        .author("Studio examples")
        .description("A generator, a transition, two importers, an exporter and a panel: one of each kind of plugin item.")
        .license("Apache-2.0")
        .generator(Plasma)
        .transition(Iris)
        .importer(Netpbm)
        .importer(MarkerList)
        .exporter(ClipCsv)
        .panel(ShotList)
}

export_plugin!(plugin);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn netpbm_ascii_and_binary() {
        let Imported::Image { width, height, rgba8 } = decode_netpbm(b"P3\n# c\n2 1\n255\n255 0 0  0 0 255\n").unwrap() else { panic!() };
        assert_eq!((width, height), (2, 1));
        assert_eq!(&rgba8[..8], &[255, 0, 0, 255, 0, 0, 255, 255]);
        let mut p5 = b"P5 1 1 255\n".to_vec();
        p5.push(128);
        let Imported::Image { rgba8, .. } = decode_netpbm(&p5).unwrap() else { panic!() };
        assert_eq!(rgba8, vec![128, 128, 128, 255]);
        let Imported::Image { rgba8, .. } = decode_netpbm(b"P1 2 1 10").unwrap() else { panic!() };
        assert_eq!((rgba8[0], rgba8[4]), (0, 255));
        assert!(decode_netpbm(b"P9 1 1").is_err());
    }

    #[test]
    fn markers_and_csv() {
        let Imported::Ops(ops) = MarkerList.import("a.markers", b"# list\n1.5 Intro\n1:02 Chorus\n").unwrap() else { panic!() };
        assert_eq!(ops.at(1).get("at").as_f64(), Some(62.0));
        let project = Json::parse(r#"{"clips":[{"id":3,"name":"A, b","kind":"video","lane":0,"start":1,"dur":2,"effects":["blur"]}]}"#).unwrap();
        let csv = String::from_utf8(ClipCsv.export(&project).unwrap()).unwrap();
        assert!(csv.contains("3,\"A, b\",video,0,1.000,3.000,2.000,blur"), "{csv}");
        let v = ShotList.update(&PanelEvent { button: Some("mark".into()), ..Default::default() }, &project);
        assert_eq!(v.ops.unwrap().items().len(), 1);
    }
}
