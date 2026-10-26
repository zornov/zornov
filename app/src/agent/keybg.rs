// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `key_background`: removes a plain backdrop from a picture or clip.
//!
//! A white sticker border on a product shot, a white or black studio sheet,
//! a grey wall: the backdrop colour is picked from the picture's border
//! (`colour: "auto"`, the default) or given (`white`, `black`, `grey`,
//! `green`, `blue` or `#rrggbb`), and the chroma key is set on the clip
//! with a tolerance that fits how even the backdrop is. The reply says what
//! was found and every parameter, and what to change when the key eats a
//! pale detail (lower `tolerance`) or leaves backdrop behind (raise it).
//! The key itself (`studio_fx::keycolour`) weighs luma as well as colour
//! for a white, black or grey backdrop and takes the backdrop colour out of
//! soft edges, so hair and thin strokes do not keep a halo.

use super::{Session, f, micros};
use serde_json::Value;
use studio_fx::keycolour::{self, PlainBackground};
use studio_model as m;

const KIND: &str = "chroma_key";

/// A backdrop colour named in words or `#rrggbb`, as a straight linear
/// colour.
fn named(s: &str) -> Option<[f32; 3]> {
    let e = match s.trim().to_lowercase().as_str() {
        "white" => [1.0; 3],
        "black" => [0.0; 3],
        "grey" | "gray" => [0.5; 3],
        "green" => [0.05, 0.55, 0.1].map(studio_fx::image::linear_to_srgb),
        "blue" => [0.05, 0.1, 0.55].map(studio_fx::image::linear_to_srgb),
        other => {
            let k = super::craft::colour(other)?;
            [k.r() as f32 / 255.0, k.g() as f32 / 255.0, k.b() as f32 / 255.0]
        }
    };
    Some(e.map(studio_fx::image::srgb_to_linear))
}

fn rgba_of(linear: [f32; 3]) -> m::Rgba {
    m::Rgba::from_linear([linear[0] as f64, linear[1] as f64, linear[2] as f64, 1.0])
}

fn hex_of(linear: [f32; 3]) -> String {
    let e = linear.map(|v| (studio_fx::image::linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0).round() as u8);
    format!("#{:02x}{:02x}{:02x}", e[0], e[1], e[2])
}

impl Session {
    pub(super) fn key_background(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("key_background needs \"clip\": the picture or footage with the plain backdrop")? as u32;
        let vc = self.clip(id)?;
        let seq = self.doc.seq();
        let cid = m::ClipId(id as u64);
        let mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let existing = mc.effects.iter().find(|e| e.kind == KIND).cloned();
        if op.get("remove").and_then(Value::as_bool) == Some(true) {
            let e = existing.ok_or_else(|| format!("clip {id} has no background key"))?;
            let ok = self.doc.apply("Remove background key", m::Edit::RemoveEffect { seq, clip: cid, effect: e.id });
            self.check(ok, "")?;
            return Ok(format!("clip {id} shows its whole picture again"));
        }
        // Which colour: given, or found at the border.
        let want = op.get("colour").or_else(|| op.get("color")).and_then(Value::as_str).unwrap_or("auto");
        let at = f(op, "at").map(micros).unwrap_or(vc.start + vc.dur / 2).clamp(vc.start, vc.start + vc.dur - 1);
        let found: Option<PlainBackground> = {
            let (w, h, px) = crate::services::cutout::clip_frame(&self.doc, id, at, 480)?;
            let img = studio_fx::Image::from_srgb8(w, h, &px);
            keycolour::detect_plain_background(&img)
        };
        let (colour, how, tol0, soft0) = if want.eq_ignore_ascii_case("auto") {
            let b = found.ok_or("the border of this picture is not one plain colour, so there is no backdrop to pick: say which colour to remove (colour: \"white\", \"black\", \"grey\", \"green\" or \"#rrggbb\"), or cut the subject out with cut_out (AI) or roto if the backdrop is busy")?;
            let how = format!("picked from the border: {} ({}), {:.0}% of the border pixels, {}", b.name(), b.hex(), b.share * 100.0, if b.spread < 0.04 { "very even" } else if b.spread < 0.12 { "slightly uneven" } else { "uneven (shaded or noisy)" });
            (b.colour, how, b.tolerance(), b.softness())
        } else {
            let c = named(want).ok_or_else(|| format!("colour \"{want}\": auto, white, black, grey, green, blue or #rrggbb"))?;
            // Tolerance for a colour given by name: from the border when it
            // matches that colour, else a middle value.
            let (t, s) = match found.filter(|b| keycolour::distance(b.colour, c) < 0.25) {
                Some(b) => (b.tolerance(), b.softness()),
                None => (0.2, 0.12),
            };
            (c, format!("{want} ({}), as given", hex_of(c)), t, s)
        };
        if op.get("detect_only").and_then(Value::as_bool) == Some(true) {
            return Ok(format!("backdrop {how}; would key with tolerance {tol0:.2}, softness {soft0:.2} (nothing changed)"));
        }
        let tol = f(op, "tolerance").map(|v| v as f32).unwrap_or(tol0).clamp(0.0, 1.0);
        let soft = f(op, "softness").map(|v| v as f32).unwrap_or(soft0).clamp(0.0, 1.0);
        let spill = f(op, "spill").map(|v| v as f32).unwrap_or(0.6).clamp(0.0, 1.0);
        let feather = f(op, "feather").map(|v| v as f32).unwrap_or(0.1).clamp(0.0, 2.0);
        let choke = f(op, "choke").map(|v| v as f32).unwrap_or(0.0).clamp(-2.0, 2.0);
        let mut e = match existing {
            Some(e) => e,
            None => m::Effect::new(m::EffectId(self.doc.alloc_id()), KIND),
        };
        e.enabled = true;
        e.params.insert("key".into(), m::Value::Colour(rgba_of(colour)));
        for (k, v) in [("tolerance", tol), ("softness", soft), ("spill", spill), ("feather", feather), ("choke", choke), ("amount", 1.0)] {
            e.params.insert(k.into(), m::Value::Scalar(v as f64));
        }
        e.params.insert("show_matte".into(), m::Value::Bool(op.get("show_matte").and_then(Value::as_bool).unwrap_or(false)));
        let had = mc.effects.iter().any(|x| x.id == e.id);
        let edit = if had { m::Edit::SetEffect { seq, clip: cid, effect: e } } else { m::Edit::AddEffect { seq, clip: cid, stack: m::Stack::Video, effect: e, index: Some(0) } };
        let ok = self.doc.apply("Key background", edit);
        self.check(ok, "")?;
        let neutral = keycolour::neutrality(colour) > 0.5;
        Ok(format!(
            "clip {id}: backdrop {how}. chroma_key set: key {}, tolerance {tol:.2} (how far from the backdrop still goes), softness {soft:.2} (ramp above it), spill {spill:.2} ({}), feather {feather:.2}% of the height, choke {choke:.2}. {}If a pale detail went with the backdrop, lower tolerance; if backdrop is left, raise tolerance or softness; show_matte: true shows the matte. The clip is transparent where the backdrop was: put what goes behind it on the lane below",
            hex_of(colour),
            if neutral { "takes the backdrop colour out of soft edges, so no light or dark halo" } else { "removes the screen colour from edges" },
            if neutral { "A white, black or grey backdrop is keyed by brightness and colour together, so dark or pale details are kept as long as they differ from it by more than the tolerance. " } else { "" },
        ))
    }
}

/// What a key effect takes, for the reply to applying one: every setting
/// with its range and default, how to set them, and the right tool for a
/// plain backdrop. `None` for any other effect.
pub(super) fn key_hint(found: &str) -> Option<String> {
    use studio_fx::params::{ParamKind, ParamValue};
    let id = found.trim().to_lowercase().replace([' ', '-'], "_");
    if !matches!(id.as_str(), "chroma_key" | "luma_key" | "difference_key") {
        return None;
    }
    let e = studio_fx::effects::find(&id)?;
    let settings: Vec<String> = e
        .params
        .iter()
        .map(|d| {
            let (range, default) = match (&d.kind, &d.default) {
                (ParamKind::Float { min, max }, ParamValue::Float(v)) => (format!("{min}..{max}"), format!("{v}")),
                (ParamKind::Int { min, max }, ParamValue::Int(v)) => (format!("{min}..{max}"), format!("{v}")),
                (ParamKind::Bool, ParamValue::Bool(b)) => ("true or false".to_owned(), b.to_string()),
                (ParamKind::Choice(opts), ParamValue::Choice(i)) => (opts.join(" | "), opts.get(*i as usize).copied().unwrap_or("").to_owned()),
                (ParamKind::Colour, ParamValue::Colour(c)) => ("a colour, #rrggbb".to_owned(), hex_of([c[0], c[1], c[2]])),
                _ => (String::new(), String::new()),
            };
            format!("{} ({range}; default {default})", d.id)
        })
        .collect();
    let how = match id.as_str() {
        "chroma_key" => "key is the backdrop colour (any colour: green, blue, or white, black, grey); tolerance how far from it still goes (about 0.1 for a clean white sheet, 0.35 for an uneven screen); softness the ramp above it; spill removes the backdrop colour from edges. For a plain white, black or grey backdrop key_background picks the colour from the border and sets these for you",
        "luma_key" => "mode picks what goes: Darks removes dark areas (a black backdrop), Brights removes bright ones (a white backdrop), set it by name; threshold is the share of the brightness range removed from that end (0.1 = the top or bottom tenth), softness the ramp. For a white or black backdrop key_background does it from the border colour",
        _ => "threshold and softness say how far from the clean plate a pixel must differ to stay; needs a clean background plate of the same size as the aux input",
    };
    Some(format!("{} settings (change with effect {{clip, effect: \"{id}\", params: {{name: value}}}}): {}. {how}", e.name, settings.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_are_named_or_hex() {
        assert_eq!(named("white"), Some([1.0; 3]));
        assert_eq!(named("black"), Some([0.0; 3]));
        let g = named("grey").expect("grey");
        assert!(g[0] > 0.1 && g[0] < 0.3, "{g:?}");
        let h = named("#ff0000").expect("hex");
        assert!(h[0] > 0.99 && h[1] < 0.01);
        assert_eq!(named("notacolour"), None);
        assert_eq!(hex_of([1.0; 3]), "#ffffff");
        let h = key_hint("Chroma key").expect("a key effect");
        assert!(h.contains("tolerance") && h.contains("key_background"), "{h}");
        let l = key_hint("luma key").expect("a key effect");
        assert!(l.contains("Darks | Brights") && l.contains("Brights removes bright"), "{l}");
        assert!(key_hint("blur").is_none());
    }
}
