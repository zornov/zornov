// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `fonts`: the installed system fonts and Studio's bundled ones, searched
//! by name, category and look, with a small preview on request. A title
//! then takes the family by name: `text` / `set` with `font`, `weight`,
//! `italic`, and `font` inside a run.
//!
//! * no arguments: what is available, by category, and how to ask;
//! * `query` / `category`: matching families (a free-text request such as
//!   "art deco", "handwritten marker" or "heavy slab" works), best first;
//! * `family`: one family in detail (weights, tags, which face answers);
//! * `preview: true` (or a sample text): the first matches drawn as a picture,
//!   row by row in the order of the list.

use super::Content;
use serde_json::{Value, json};
use studio_text::{FontHit, FontRequest, FontSource};

const DEFAULT_LIMIT: usize = 20;
const MAX_LIMIT: usize = 60;
/// Rows in one preview picture.
const PREVIEW_ROWS: usize = 8;

fn source_name(s: FontSource) -> &'static str {
    match s {
        FontSource::System => "system",
        FontSource::Bundled => "bundled",
        FontSource::User => "imported",
    }
}

fn row(h: &FontHit) -> Value {
    let e = &h.entry;
    json!({
        "family": e.family,
        "category": e.category,
        "tags": e.tags.iter().filter(|t| *t != e.category).collect::<Vec<_>>(),
        "weights": e.weights,
        "italic": e.italic,
        "from": source_name(e.source),
    })
}

/// The tool: JSON text, plus a picture when a preview is asked for.
pub fn tool(a: &Value) -> Result<Vec<Content>, String> {
    let engine = crate::text_look::engine();
    let lib = &engine.fonts;
    let limit = a.get("limit").and_then(Value::as_u64).map_or(DEFAULT_LIMIT, |n| (n as usize).clamp(1, MAX_LIMIT));
    let text_of = |k: &str| a.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    let (query, category, family) = (text_of("query"), text_of("category"), text_of("family"));
    let source = text_of("source").map(str::to_lowercase);
    let weight = a.get("weight").and_then(Value::as_f64).map_or(400, |w| w.clamp(100.0, 900.0) as u16);
    let italic = a.get("italic").and_then(Value::as_bool).unwrap_or(false);
    // `preview` is true, or the sample text to draw.
    let (want_preview, sample) = match a.get("preview") {
        Some(Value::Bool(b)) => (*b, None),
        Some(Value::String(s)) if !s.trim().is_empty() => (true, Some(s.trim().to_owned())),
        _ => (false, text_of("sample").map(str::to_owned)),
    };
    let want_preview = want_preview || sample.is_some();

    // One family in detail.
    if let Some(fam) = family {
        let hit = lib.find_fonts(fam, None, 200).into_iter().find(|h| h.entry.family.eq_ignore_ascii_case(fam));
        let Some(hit) = hit else {
            let near = lib.similar_families(fam, 6);
            return Ok(vec![Content::Text(
                json!({
                    "family": fam,
                    "installed": false,
                    "close": near,
                    "note": "no such family is installed or bundled; a title would not draw in it (text with font refuses it). Search by look instead: fonts with query, e.g. \"art deco\", \"script\", \"slab\", \"mono\"",
                })
                .to_string(),
            )]);
        };
        let face = engine.resolved(&FontRequest::new(&hit.entry.family, weight, italic));
        let mut v = row(&hit);
        v["installed"] = json!(true);
        v["faces"] = json!(hit.entry.faces);
        if !hit.entry.licence.is_empty() {
            v["licence"] = json!(hit.entry.licence);
        }
        if let Some(f) = face {
            v["answers_for"] = json!({"weight": weight, "italic": italic, "face": format!("{} {}{}", f.family, f.weight, if f.italic { " italic" } else { "" }), "synthetic_bold": f.synthetic_bold, "synthetic_italic": f.synthetic_italic});
        }
        v["use"] = json!(format!("{{\"op\": \"text\", \"text\": \"...\", \"font\": \"{}\", \"weight\": {}}}", hit.entry.family, weight));
        let mut out = vec![Content::Text(v.to_string())];
        if want_preview {
            let s = sample.clone().unwrap_or_else(|| "Aa Quick brown fox 0123".to_owned());
            // The weights it has, one row each.
            let mut rows: Vec<(String, FontRequest)> = Vec::new();
            let ws: Vec<u16> = if hit.entry.weights.len() > 4 { vec![300, 400, 700, 900] } else { hit.entry.weights.clone() };
            for w in ws {
                rows.push((format!("{} {w}", hit.entry.family), FontRequest::new(&hit.entry.family, w, false)));
            }
            if hit.entry.italic {
                rows.push((format!("{} italic", hit.entry.family), FontRequest::new(&hit.entry.family, 400, true)));
            }
            if let Some(j) = sheet(rows.into_iter().map(|(_, r)| r).collect(), &s) {
                out.push(Content::Jpeg(j));
            }
        }
        return Ok(out);
    }

    // Overview.
    if query.is_none() && category.is_none() && source.is_none() {
        let counts = lib.category_counts();
        let mut examples = serde_json::Map::new();
        for (c, _) in &counts {
            let hits: Vec<String> = lib.find_fonts("", Some(c), 4).into_iter().map(|h| h.entry.family).collect();
            examples.insert((*c).to_owned(), json!(hits));
        }
        let mut v = json!({
            "families": counts.iter().map(|(_, n)| n).sum::<usize>(),
            "by_category": counts.iter().map(|(c, n)| (c.to_string(), json!(n))).collect::<serde_json::Map<_, _>>(),
            "examples": examples,
            "looks": "art deco, blackletter, pixel, stencil, futuristic, western, horror, comic, elegant, retro, slab, rounded, condensed, script, handwritten, mono; also bold, light, italic, variable",
            "ask": "fonts {query: \"art deco\"} or {category: \"script\"} or {query: \"heavy slab\", preview: true} lists matches; {family: \"Name\"} shows one family; preview draws them",
            "use": "text / set take font (family), weight (100-900) and italic; a run in runs takes font too: {\"op\": \"text\", \"text\": \"GRAND OPENING\", \"font\": \"Poiret One\", \"weight\": 400}",
        });
        if want_preview {
            let picks: Vec<FontRequest> = counts.iter().filter_map(|(c, _)| lib.find_fonts("", Some(c), 1).into_iter().next()).map(|h| FontRequest::new(&h.entry.family, 400, false)).collect();
            let mut out = vec![Content::Text(v.to_string())];
            if let Some(j) = sheet(picks, sample.as_deref().unwrap_or("Aa Quick brown fox")) {
                out.push(Content::Jpeg(j));
            }
            return Ok(out);
        }
        v["note"] = json!("add a query to narrow it down");
        return Ok(vec![Content::Text(v.to_string())]);
    }

    // A search.
    let mut hits = lib.find_fonts(query.unwrap_or(""), category, if source.is_some() { 400 } else { limit.max(PREVIEW_ROWS) });
    if let Some(src) = &source {
        let keep = match src.as_str() {
            "system" | "installed" => FontSource::System,
            "bundled" | "studio" => FontSource::Bundled,
            "imported" | "user" => FontSource::User,
            other => return Err(format!("source \"{other}\": system, bundled or imported")),
        };
        hits.retain(|h| h.entry.source == keep);
    }
    let total = hits.len();
    hits.truncate(limit);
    let asked = [query, category].iter().flatten().copied().collect::<Vec<_>>().join(" ");
    let mut v = json!({"query": asked, "matches": hits.iter().map(row).collect::<Vec<_>>()});
    if hits.is_empty() {
        let w = studio_text::directory::parse_query(&asked);
        v["note"] = json!(format!(
            "no installed or bundled family matches \"{asked}\". {}Try a broader look (display, script, serif, elegant, retro) or fonts with no arguments for what is available. Do not draw lettering by hand as outlines: pick the nearest family and set it with text font",
            if w.words.is_empty() { String::new() } else { format!("(\"{}\" is read as part of a family name.) ", w.words.join(" ")) }
        ));
        let near: Vec<String> = asked.split_whitespace().flat_map(|w| lib.similar_families(w, 3)).take(6).collect();
        if !near.is_empty() {
            v["close"] = json!(near);
        }
    } else {
        if total > hits.len() {
            v["more"] = json!(format!("{} more match; narrow the query or raise limit (up to {MAX_LIMIT})", total - hits.len()));
        }
        v["use"] = json!("text / set with font: \"<family>\", weight, italic; preview: true draws the first matches");
    }
    let mut out = Vec::new();
    if want_preview && !hits.is_empty() {
        let picks: Vec<(String, FontRequest)> = hits.iter().take(PREVIEW_ROWS).map(|h| (h.entry.family.clone(), FontRequest::new(&h.entry.family, pick_weight(&h.entry.weights), false))).collect();
        v["preview"] = json!(format!("rows are the first {} matches in order; each row shows {}", picks.len(), if sample.is_some() { "your sample" } else { "the family's name" }));
        let rows: Vec<(String, FontRequest)> = picks;
        let j = match &sample {
            Some(s) => sheet(rows.into_iter().map(|(_, r)| r).collect(), s),
            None => sheet_named(rows),
        };
        out.push(Content::Text(v.to_string()));
        if let Some(j) = j {
            out.push(Content::Jpeg(j));
        }
        return Ok(out);
    }
    out.push(Content::Text(v.to_string()));
    Ok(out)
}

/// The regular weight when the family has it, else the nearest to it.
fn pick_weight(ws: &[u16]) -> u16 {
    ws.iter().copied().min_by_key(|w| w.abs_diff(400)).unwrap_or(400)
}

const ROW_W: u32 = 720;
const ROW_H: u32 = 84;

/// Rows of `sample`, one per request, as a JPEG.
fn sheet(rows: Vec<FontRequest>, sample: &str) -> Option<Vec<u8>> {
    stack(rows.into_iter().map(|r| (r, sample.to_owned())).collect())
}

/// Rows each showing its own family name.
fn sheet_named(rows: Vec<(String, FontRequest)>) -> Option<Vec<u8>> {
    stack(rows.into_iter().map(|(name, r)| (r, name)).collect())
}

fn stack(rows: Vec<(FontRequest, String)>) -> Option<Vec<u8>> {
    if rows.is_empty() {
        return None;
    }
    let engine = crate::text_look::engine();
    let n = rows.len() as u32;
    let mut srgb = vec![0u8; (ROW_W * ROW_H * n * 4) as usize];
    for (i, (req, text)) in rows.iter().enumerate() {
        let (img, _) = engine.specimen(req, text, ROW_W, ROW_H - 2);
        let f = studio_fx::Image { width: img.width as usize, height: img.height as usize, pixels: img.pixels };
        let bytes = f.to_srgb8();
        let off = (i as u32 * ROW_H * ROW_W * 4) as usize;
        srgb[off..off + bytes.len()].copy_from_slice(&bytes);
        // A hairline between rows.
        let rule = off + ((ROW_H - 2) * ROW_W * 4) as usize;
        for p in srgb[rule..rule + (2 * ROW_W * 4) as usize].chunks_exact_mut(4) {
            p.copy_from_slice(&[150, 150, 150, 255]);
        }
    }
    let img = studio_media::RgbaImage { width: ROW_W, height: ROW_H * n, pixels: srgb };
    Some(studio_content::jpeg::encode(&img, 80))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(parts: &[Content]) -> Value {
        match &parts[0] {
            Content::Text(t) => serde_json::from_str(t).expect("json"),
            _ => panic!("text first"),
        }
    }

    #[test]
    fn overview_lists_categories_and_how_to_ask() {
        let r = tool(&json!({})).expect("fonts");
        let v = text(&r);
        assert!(v["families"].as_u64().unwrap_or(0) >= 2, "{v}");
        assert!(v["by_category"].is_object() && v["ask"].is_string(), "{v}");
        assert!(v["use"].as_str().unwrap_or("").contains("font"), "{v}");
    }

    #[test]
    fn searches_by_name_and_category_and_says_when_nothing_matches() {
        // Manrope is bundled with the app, so this holds on every machine.
        let v = text(&tool(&json!({"query": "manrope"})).expect("fonts"));
        assert_eq!(v["matches"][0]["family"], "Manrope", "{v}");
        let none = text(&tool(&json!({"query": "zzqx nothing like this"})).expect("fonts"));
        assert_eq!(none["matches"].as_array().map(Vec::len), Some(0), "{none}");
        assert!(none["note"].as_str().unwrap_or("").contains("no installed"), "{none}");
        let mono = text(&tool(&json!({"category": "mono", "limit": 5})).expect("fonts"));
        assert!(mono["matches"].as_array().is_some_and(|m| m.iter().all(|f| f["category"] == "mono")), "{mono}");
    }

    #[test]
    fn family_detail_and_preview_picture() {
        let r = tool(&json!({"family": "Manrope", "preview": true})).expect("fonts");
        let v = text(&r);
        assert_eq!(v["installed"], true, "{v}");
        assert!(v["weights"].as_array().is_some_and(|w| !w.is_empty()));
        assert!(r.iter().any(|p| matches!(p, Content::Jpeg(j) if j.len() > 500)), "no preview picture");
        let missing = text(&tool(&json!({"family": "Manrop"})).expect("fonts"));
        assert_eq!(missing["installed"], false);
        assert!(missing["close"].as_array().is_some_and(|c| c.iter().any(|f| f == "Manrope")), "{missing}");
    }

    #[test]
    fn a_search_preview_is_one_picture_in_list_order() {
        let r = tool(&json!({"query": "manrope", "preview": "Hello"})).expect("fonts");
        assert!(r.iter().any(|p| matches!(p, Content::Jpeg(_))));
        assert!(text(&r)["preview"].as_str().unwrap_or("").contains("your sample"));
    }
}
