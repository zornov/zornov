// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Photoshop documents (PSD / PSB) as pictures and as layers.
//!
//! [`flatten`] writes the document's merged image as a PNG: what `convert`
//! hands Studio for a `.psd` dropped anywhere. [`extract`] writes every
//! visible-or-hidden pixel layer as its own PNG (its own bounds, its layer
//! mask in the alpha) and says where each sits in the document, with its
//! opacity, blend mode, visibility and group, bottom of the stack first:
//! what the `import` tool's `layers: true` lays out as clips.
//!
//! The reader is EffectCraft's (`studio-psd`, vendored; see NOTICE). What
//! Studio cannot carry over (adjustment layers, layer styles, clipping
//! masks, group blend modes, live text) is listed in `notes`, and the
//! pixels Photoshop drew for it are kept where there are any.

use std::path::{Path, PathBuf};
use studio_psd::{Node, Psd};

/// One layer, ready to place.
#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub name: String,
    pub png: PathBuf,
    /// Bounds in document pixels.
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    /// 0..100, the group's opacity and the fill opacity folded in.
    pub opacity: f32,
    /// Studio's blend mode name.
    pub blend: &'static str,
    pub visible: bool,
    /// The groups it is in, outermost first.
    pub groups: Vec<String>,
    /// What a type layer says (its pixels are the rendered text).
    pub text: Option<String>,
}

/// A document taken apart.
#[derive(Clone, Debug)]
pub struct Extracted {
    pub width: u32,
    pub height: u32,
    /// Bottom of the stack first.
    pub layers: Vec<Layer>,
    /// What did not come over as it was.
    pub notes: Vec<String>,
}

/// Photoshop's blend key as Studio's blend mode (`None`: no match).
pub fn blend(key: &str) -> Option<&'static str> {
    Some(match key {
        "norm" | "pass" => "normal",
        "mul " => "multiply",
        "scrn" => "screen",
        "over" => "overlay",
        "sLit" => "soft_light",
        "hLit" => "hard_light",
        "div " => "colour_dodge",
        "idiv" => "colour_burn",
        "dark" => "darken",
        "lite" => "lighten",
        "diff" => "difference",
        "smud" => "exclusion",
        "hue " => "hue",
        "sat " => "saturation",
        "colr" => "colour",
        "lum " => "luminosity",
        "lddg" => "add",
        _ => return None,
    })
}

/// Whether `path` is a Photoshop document (by its first bytes).
/// Photoshop documents and large documents.
pub const EXTENSIONS: &[&str] = &["psd", "psb"];

pub fn is_psd(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 4];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok() && &head == b"8BPS"
}

fn open(path: &Path) -> Result<Psd, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !studio_psd::is_psd(&bytes) {
        return Err(format!("{} is not a Photoshop document", path.display()));
    }
    Psd::parse(bytes).map_err(|e| format!("{}: {e}", path.display()))
}

fn write_png(path: &Path, px: &studio_psd::Pixels) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), px.width, px.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    let bytes: Vec<u8> = px.data.iter().flat_map(|p| p.map(|c| (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)).collect();
    w.write_image_data(&bytes).map_err(|e| e.to_string())
}

/// The merged image of `psd` as a PNG at `out`.
pub fn flatten(psd: &Path, out: &Path) -> Result<(), String> {
    let doc = open(psd)?;
    let px = doc.composite().map_err(|e| format!("{}: {e}", psd.display()))?;
    write_png(out, &px)
}

/// A file name for a layer: its number and name.
fn file_name(n: usize, name: &str) -> String {
    let clean: String = name.chars().map(|c| if c.is_alphanumeric() || " -_".contains(c) { c } else { '-' }).collect();
    let clean = clean.trim().chars().take(60).collect::<String>();
    format!("{n:02} {}.png", if clean.is_empty() { "layer" } else { &clean })
}

/// Every pixel layer of `psd` as PNGs in `dir`.
pub fn extract(psd: &Path, dir: &Path) -> Result<Extracted, String> {
    let doc = open(psd)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut notes = doc.warnings.clone();
    // The tree, top first, walked into a flat list with each layer's
    // groups, visibility and opacity.
    struct At {
        index: usize,
        groups: Vec<String>,
        visible: bool,
        opacity: f32,
    }
    fn walk(doc: &Psd, nodes: &[Node], groups: &[String], visible: bool, opacity: f32, out: &mut Vec<At>, notes: &mut Vec<String>) {
        for n in nodes {
            match n {
                Node::Layer(i) => out.push(At { index: *i, groups: groups.to_vec(), visible, opacity }),
                Node::Group { layer, children } => {
                    let g = &doc.layers[*layer];
                    if !matches!(g.blend_key(), "pass" | "norm") {
                        notes.push(format!("group \"{}\": its blend mode ({}) applies to each layer on its own", g.name, g.blend_key().trim()));
                    }
                    let mut inner = groups.to_vec();
                    inner.push(g.name.clone());
                    walk(doc, children, &inner, visible && !g.hidden, opacity * f32::from(g.opacity) / 255.0, out, notes);
                }
            }
        }
    }
    let mut order = Vec::new();
    walk(&doc, &doc.tree(), &[], true, 1.0, &mut order, &mut notes);
    let mut layers = Vec::new();
    for (k, at) in order.iter().rev().enumerate() {
        let l = &doc.layers[at.index];
        if l.adjustment.is_some() {
            notes.push(format!("\"{}\" is an adjustment layer: not brought over (use a look or grade on the clip)", l.name));
            continue;
        }
        if !l.has_pixels() || l.rect.is_empty() {
            continue;
        }
        if l.clipping {
            notes.push(format!("\"{}\" was clipped to the layer below: placed unclipped", l.name));
        }
        if l.effects.is_some() {
            notes.push(format!("\"{}\" has layer styles (shadows, strokes…): not drawn", l.name));
        }
        let blend_mode = blend(l.blend_key()).unwrap_or_else(|| {
            notes.push(format!("\"{}\": blend mode {} has no match in Zornov, so it is normal", l.name, l.blend_key().trim()));
            "normal"
        });
        let px = match doc.layer_pixels(at.index, false) {
            Ok(px) if px.width > 0 && px.height > 0 => px,
            Ok(_) => continue,
            Err(e) => {
                notes.push(format!("\"{}\" could not be read: {e}", l.name));
                continue;
            }
        };
        let png = dir.join(file_name(k + 1, &l.name));
        write_png(&png, &px)?;
        layers.push(Layer {
            name: l.name.clone(),
            png,
            left: l.rect.left,
            top: l.rect.top,
            width: px.width,
            height: px.height,
            opacity: (at.opacity * f32::from(l.opacity) / 255.0 * f32::from(l.fill_opacity) / 255.0 * 100.0).clamp(0.0, 100.0),
            blend: blend_mode,
            visible: at.visible && !l.hidden,
            groups: at.groups.clone(),
            text: l.text.as_ref().map(|t| t.text.clone()),
        });
    }
    if layers.is_empty() {
        return Err(format!("{} has no pixel layers (import it without layers for its merged image)", psd.display()));
    }
    Ok(Extracted { width: doc.width, height: doc.height, layers, notes })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use studio_psd::Rect;
    use studio_psd::write::{WDoc, WLayer, write};

    /// A 200x100 document: a red background, a half-transparent blue
    /// square in a group, a hidden green bar on screen, a type-free top.
    pub fn sample(path: &Path) {
        let mut d = WDoc::new(200, 100);
        d.layers = vec![
            WLayer::solid("Background", Rect::new(0, 0, 200, 100), [1.0, 0.0, 0.0, 1.0]),
            WLayer::group_end(),
            WLayer::solid("Square", Rect::new(20, 10, 40, 40), [0.0, 0.0, 1.0, 1.0]).with_blend(b"scrn").with_opacity(128),
            WLayer::group("Shapes", true, *b"pass").with_opacity(255),
            WLayer { hidden: true, ..WLayer::solid("Bar", Rect::new(0, 80, 200, 20), [0.0, 1.0, 0.0, 1.0]) },
        ];
        d.composite = Some(vec![[1.0, 0.0, 0.0, 1.0]; 200 * 100]);
        std::fs::write(path, write(&d)).unwrap();
    }

    #[test]
    fn layers_come_out_bottom_first_with_their_place_and_looks() {
        let dir = std::env::temp_dir().join(format!("studio-psd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let psd = dir.join("poster.psd");
        sample(&psd);
        let x = extract(&psd, &dir.join("layers")).unwrap();
        assert_eq!((x.width, x.height), (200, 100));
        let names: Vec<&str> = x.layers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["Background", "Square", "Bar"]);
        let sq = &x.layers[1];
        assert_eq!((sq.left, sq.top, sq.width, sq.height), (20, 10, 40, 40));
        assert_eq!(sq.blend, "screen");
        assert!((sq.opacity - 50.2).abs() < 0.5, "{}", sq.opacity);
        assert_eq!(sq.groups, ["Shapes"]);
        assert!(!x.layers[2].visible && x.layers[0].visible);
        let img = image_size(&sq.png);
        assert_eq!(img, (40, 40));
        flatten(&psd, &dir.join("flat.png")).unwrap();
        assert_eq!(image_size(&dir.join("flat.png")), (200, 100));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn image_size(p: &Path) -> (u32, u32) {
        let d = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(p).unwrap()));
        let r = d.read_info().unwrap();
        (r.info().width, r.info().height)
    }

    #[test]
    fn blend_keys_map_to_studio_names() {
        assert_eq!(blend("mul "), Some("multiply"));
        assert_eq!(blend("lddg"), Some("add"));
        assert_eq!(blend("hMix"), None);
        assert_eq!(file_name(3, "Logo / final?"), "03 Logo - final-.png");
    }
}
