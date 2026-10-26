// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `import` with `layers: true` for Photoshop documents: every pixel layer
//! becomes a picture clip on its own lane, where it sat in the document
//! (the document fitted to the canvas), with its opacity, blend mode and
//! visibility, merged into one clip named after the file (`unmerge` opens
//! it into the layers again; `merge: false` leaves them apart). One undo
//! step after the layers are added to the media bin.

use super::{Session, f};
use serde_json::{Value, json};
use std::path::Path;

impl Session {
    pub(super) fn import_psd_layers(&mut self, path: &Path, a: &Value) -> Result<Value, String> {
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Photoshop".into());
        // The layers go beside the document, in a folder of their own.
        let dir = path.with_file_name(format!("{stem} layers"));
        let x = crate::psd_import::extract(path, &dir)?;
        let pngs: Vec<_> = x.layers.iter().map(|l| l.png.clone()).collect();
        let report = crate::services::project::import_media(&mut self.doc, &pngs);
        if report.ids.len() != x.layers.len() {
            return Err(format!("the layers could not be added to the media bin: {}", report.failed.join("; ")));
        }
        let (cw, ch) = (self.doc.project.width as f64, self.doc.project.height as f64);
        let (dw, dh) = (x.width.max(1) as f64, x.height.max(1) as f64);
        // The whole document fits the canvas; scale 100 fits one picture.
        let doc_fit = (cw / dw).min(ch / dh);
        let at = f(a, "at").unwrap_or(0.0);
        let dur = f(a, "duration").unwrap_or(5.0).max(0.1);
        let mut ops = Vec::new();
        let mut lanes = Vec::new();
        for (i, (l, media)) in x.layers.iter().zip(&report.ids).enumerate() {
            let lane = format!("{stem} · {} {}", i + 1, l.name);
            let (w, h) = (l.width.max(1) as f64, l.height.max(1) as f64);
            let own_fit = (cw / w).min(ch / h);
            let cx = (l.left as f64 + w / 2.0 - dw / 2.0) * doc_fit;
            let cy = (l.top as f64 + h / 2.0 - dh / 2.0) * doc_fit;
            ops.push(json!({"op": "lane", "action": "add", "name": lane, "position": "top"}));
            ops.push(json!({"op": "place", "media": media, "at": at, "lane": lane, "duration": dur, "x": cx, "y": cy, "name": l.name}));
            ops.push(json!({"op": "set", "clip": "$last", "scale": 100.0 * doc_fit / own_fit, "opacity": l.opacity, "blend": l.blend, "enabled": l.visible}));
            lanes.push(lane);
        }
        let merge = a.get("merge").and_then(Value::as_bool).unwrap_or(true) && x.layers.len() > 1;
        if merge {
            let refs: Vec<String> = (1..=x.layers.len()).map(|i| format!("${i}")).collect();
            ops.push(json!({"op": "merge", "clips": refs, "name": stem}));
            // The merged clip sits on the first layer's lane: it keeps that
            // one, named after the file; the others are empty now.
            for lane in &lanes[1..] {
                ops.push(json!({"op": "lane", "action": "remove", "lane": lane}));
            }
            ops.push(json!({"op": "lane", "action": "rename", "lane": lanes[0], "name": stem}));
        }
        let mut v = self.edit(&ops, &format!("Import {stem} layers"), false)?;
        v["psd"] = json!({
            "file": path.display().to_string(),
            "size": [x.width, x.height],
            "layers": x.layers.iter().map(|l| json!({"name": l.name, "groups": l.groups, "visible": l.visible, "blend": l.blend, "opacity": (l.opacity * 10.0).round() / 10.0, "text": l.text})).collect::<Vec<_>>(),
            "merged": merge,
            "notes": x.notes,
        });
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_psd_comes_in_as_one_merged_clip_of_its_layers() {
        let dir = std::env::temp_dir().join(format!("studio-psd-agent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let psd = dir.join("Poster.psd");
        crate::psd_import::tests::sample(&psd);
        let mut s = Session::default();
        let lanes_before = s.doc.project.lanes.len();
        let parts = s.call("import", &json!({"paths": [psd.display().to_string()], "layers": true, "at": 1.0})).unwrap();
        let super::super::Content::Text(t) = &parts[0] else { panic!() };
        let v: Value = serde_json::from_str(t).unwrap();
        assert_eq!(v["psd"]["layers"].as_array().unwrap().len(), 3, "{v}");
        assert_eq!(v["psd"]["merged"], true);
        assert_eq!(s.doc.project.lanes.len(), lanes_before + 1, "one lane for the merged clip; the layer lanes go");
        assert!(s.doc.project.lanes.iter().any(|l| l.name == "Poster"));
        // One clip on the timeline, holding the three layers.
        let merged: Vec<_> = s.doc.model.sequences.iter().filter(|q| q.id != s.doc.seq()).collect();
        let inner = merged.iter().find(|q| q.clips.len() == 3).expect("a merged clip of three layers");
        let by_name = |n: &str| inner.clips.iter().find(|c| c.name == n).unwrap();
        let sq = by_name("Square");
        // 200x100 in 1920x1080: fitted 9.6x; the 40 px square sits at
        // (20,10)-(60,50), centre (40,30) → (-60,-20) from the centre.
        assert!((sq.transform.position.x - -576.0).abs() < 0.5 && (sq.transform.position.y - -192.0).abs() < 0.5, "{:?}", sq.transform.position);
        assert_eq!(format!("{:?}", sq.blend), "Screen");
        assert!(!by_name("Bar").enabled);
        // Undo takes the whole layout back in one step.
        assert_eq!(s.doc.project.clips.len(), 1);
        assert!(s.doc.undo().is_some());
        assert!(s.doc.project.clips.is_empty(), "one undo takes the layout back");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_layers_a_psd_imports_as_its_merged_picture() {
        let dir = std::env::temp_dir().join(format!("studio-psd-flat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let psd = dir.join("Flat.psd");
        crate::psd_import::tests::sample(&psd);
        let mut s = Session::default();
        let parts = s.call("import", &json!({"paths": [psd.display().to_string()]})).unwrap();
        let super::super::Content::Text(t) = &parts[0] else { panic!() };
        let v: Value = serde_json::from_str(t).unwrap();
        assert_eq!(v["media"].as_array().unwrap().len(), 1, "{v}");
        assert!(v["converted"][0]["to"].as_str().unwrap().ends_with("Flat (Studio).png"), "{v}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A real Photoshop file (TeX Live's tiger, when it is installed): its
    /// layers laid out by `layers: true` render as Photoshop's own merged
    /// image does, placed the same way (measured: best match at zero shift,
    /// 37.7 dB at full size; the rest is resampling the layer and the merged
    /// image from slightly different sizes).
    #[cfg(feature = "render")]
    #[test]
    fn a_real_psd_renders_the_same_layered_or_flat() {
        let src = Path::new("/usr/local/texlive/2025/texmf-dist/doc/latex/graphicxpsd/tigerpsdfmt.psd");
        if !src.is_file() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("studio-psd-real-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let psd = dir.join("Tiger.psd");
        std::fs::copy(src, &psd).unwrap();
        let render = |layers: bool| {
            let mut s = Session::default();
            s.call("import", &json!({"paths": [psd.display().to_string()], "layers": layers, "to_timeline": !layers, "duration": 2.0})).unwrap();
            if !layers {
                // The flat picture: fitted like any picture, as the layered one is.
                let id = s.doc.project.clips[0].id;
                s.edit(&[json!({"op": "set", "clip": id, "scale": 100})], "fit", false).unwrap();
            }
            let t = s.doc.project.clips[0].start as f64 / 1e6 + 0.5;
            s.frames.rgba(&s.doc, super::super::micros(t), 1920).unwrap()
        };
        let (a, b) = (render(true), render(false));
        assert_eq!((a.width, a.height), (b.width, b.height));
        let mse = a.pixels.iter().zip(&b.pixels).map(|(x, y)| (*x as f64 - *y as f64).powi(2)).sum::<f64>() / a.pixels.len() as f64;
        let psnr = 10.0 * (255.0f64 * 255.0 / mse.max(1e-9)).log10();
        eprintln!("tiger: layered vs flat render, PSNR {psnr:.1} dB");
        assert!(psnr > 35.0, "PSNR {psnr:.1} dB");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
