// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `scopes`: the video scopes as numbers (and, on request, as a picture):
//! where black, mid and white sit, what blows out or crushes, the cast in
//! shadows, mids and highlights, saturation and the skin line. `compare`
//! reads a second moment and says how far apart the two are: matching
//! shots by measurement, not by eye.

use super::{Content, Session, f, micros};
use crate::editor::scopes as ui_scopes;
use serde_json::{Value, json};
use studio_colour::scopes::{self, Readings};

fn r3(v: f32) -> f64 {
    (v as f64 * 1000.0).round() / 1000.0
}

/// Readings as the AI reads them: percentages of the signal.
pub fn readings_json(r: &Readings) -> Value {
    let pct = |v: f32| (v as f64 * 1000.0).round() / 10.0;
    let zone = |c: Option<[f32; 3]>| match c {
        Some(c) => json!({"rgb_offset": c.map(r3), "cast": ui_scopes::cast_name(c).unwrap_or("neutral")}),
        None => Value::Null,
    };
    json!({
        "luma_percent": {"p1": pct(r.luma[0]), "p5": pct(r.luma[1]), "median": pct(r.luma[2]), "p95": pct(r.luma[3]), "p99": pct(r.luma[4]), "mean": pct(r.luma_mean)},
        "blown_out_percent": pct(r.clipped),
        "crushed_percent": pct(r.crushed),
        "outside_broadcast_range_percent": pct(r.outside_legal),
        "shadows": zone(r.cast[0]),
        "mids": zone(r.cast[1]),
        "highlights": zone(r.cast[2]),
        "saturation_percent": {"mean": pct(r.saturation[0]), "p95": pct(r.saturation[1])},
        "oversaturated_percent": pct(r.oversaturated),
        "tint": {"hue_degrees": r.tint.0.round(), "strength_percent": pct(r.tint.1)},
        "near_skin_line_percent": pct(r.near_skin_line),
    })
}

impl Session {
    fn scope_readings(&mut self, t: f64) -> Result<(Readings, studio_colour::image::Image), String> {
        let img = self.frames.rgba(&self.doc, micros(t), 512)?;
        let sampled = ui_scopes::sample(&img.pixels, img.width, img.height, 256);
        Ok((scopes::readings(&sampled, &ui_scopes::settings(256)), sampled))
    }

    pub(super) fn scopes_tool(&mut self, a: &Value) -> Result<Vec<Content>, String> {
        let t = f(a, "t").ok_or("scopes needs \"t\" (seconds on the timeline)")?;
        let (r, img) = self.scope_readings(t)?;
        let mut out = json!({"t": t, "readings": readings_json(&r), "summary": ui_scopes::words(&r).join("; ")});
        if let Some(t2) = f(a, "compare") {
            let (r2, _) = self.scope_readings(t2)?;
            let d = |x: f32, y: f32| ((x - y) as f64 * 1000.0).round() / 10.0;
            let mut zones = serde_json::Map::new();
            for (k, name) in ["shadows", "mids", "highlights"].iter().enumerate() {
                if let (Some(a0), Some(b0)) = (r.cast[k], r2.cast[k]) {
                    zones.insert((*name).into(), json!({"rgb_offset_difference": [d(a0[0], b0[0]), d(a0[1], b0[1]), d(a0[2], b0[2])]}));
                }
            }
            out["compare"] = json!({
                "t": t2,
                "readings": readings_json(&r2),
                "difference": {
                    "note": "this moment minus the compared one, in percent of the signal; to match them move this one by the opposite",
                    "black_p1": d(r.luma[0], r2.luma[0]),
                    "median": d(r.luma[2], r2.luma[2]),
                    "white_p99": d(r.luma[4], r2.luma[4]),
                    "saturation_mean": d(r.saturation[0], r2.saturation[0]),
                    "zones": zones,
                }
            });
        }
        let mut res = vec![Content::Text(out.to_string())];
        if a.get("picture").and_then(Value::as_bool).unwrap_or(false) {
            res.insert(0, Content::Jpeg(studio_content::jpeg::encode(&sheet(&img), 80)));
        }
        Ok(res)
    }
}

/// Parade and vectorscope side by side on black, as one picture.
fn sheet(img: &studio_colour::image::Image) -> studio_media::rgba::RgbaImage {
    let s = ui_scopes::settings(256);
    let parade = ui_scopes::picture(img, 1, &s);
    let vector = ui_scopes::picture(img, 2, &s);
    let (pw, ph) = (parade.size[0] as u32, parade.size[1] as u32);
    let (vw, vh) = (vector.size[0] as u32, vector.size[1] as u32);
    let (w, h) = (pw + vw + 12, ph.max(vh) + 8);
    let mut out = studio_media::rgba::RgbaImage { width: w, height: h, pixels: vec![0; (w * h * 4) as usize] };
    for p in out.pixels.chunks_exact_mut(4) {
        p.copy_from_slice(&[12, 12, 14, 255]);
    }
    let mut put = |pic: &eframe::egui::ColorImage, ox: u32, oy: u32| {
        for y in 0..pic.size[1] as u32 {
            for x in 0..pic.size[0] as u32 {
                let c = pic[(x as usize, y as usize)];
                let a = c.a() as u32;
                let i = (((oy + y) * w + ox + x) * 4) as usize;
                for (k, v) in [c.r(), c.g(), c.b()].into_iter().enumerate() {
                    // The scope colours are premultiplied: add them on.
                    out.pixels[i + k] = (out.pixels[i + k] as u32 * (255 - a) / 255 + v as u32).min(255) as u8;
                }
            }
        }
    };
    put(&parade, 4, 4);
    put(&vector, pw + 8, 4);
    out
}

#[cfg(all(test, feature = "render"))]
mod tests {
    use super::*;
    use crate::services;

    #[test]
    fn scopes_read_a_card_and_compare_two_moments() {
        let mut s = Session::new(services::project::new_custom("Scopes", 320, 180, 30.0));
        s.edit(&[json!({"op": "card", "colour": "#ffffff", "at": 0, "duration": 1}), json!({"op": "card", "colour": "#203040", "at": 1, "duration": 1})], "cards", false).unwrap();
        let out = s.scopes_tool(&json!({"t": 0.5, "compare": 1.5, "picture": true})).unwrap();
        assert!(matches!(out[0], Content::Jpeg(_)));
        let Content::Text(txt) = &out[1] else { panic!() };
        let v: Value = serde_json::from_str(txt).unwrap();
        assert!(v["readings"]["blown_out_percent"].as_f64().unwrap() > 90.0, "{v}");
        assert!(v["compare"]["difference"]["median"].as_f64().unwrap() > 50.0, "{v}");
        assert_eq!(v["compare"]["readings"]["mids"], Value::Null);
        assert!(v["compare"]["readings"]["shadows"]["cast"].as_str().unwrap().contains("blue"), "{v}");
    }
}
