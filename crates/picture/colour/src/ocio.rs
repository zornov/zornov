// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! OpenColorIO configs, read and applied in Rust (no OCIO library).
//!
//! [`Config::parse`] reads a v1 or v2 `.ocio` file: roles, colour spaces
//! (scene- and display-referred), displays and views (including shared
//! views and view transforms), file rules are ignored. [`Config::processor`]
//! builds a CPU [`Processor`] between two colour spaces;
//! [`Config::view_processor`] one for a display / view pair.
//!
//! Supported transforms: `MatrixTransform`, `ExponentTransform`,
//! `ExponentWithLinearTransform`, `LogTransform`, `LogAffineTransform`,
//! `LogCameraTransform`, `CDLTransform`, `RangeTransform`,
//! `AllocationTransform`, `FileTransform` (`.cube`, `.3dl`, `.spi1d`,
//! `.spi3d`), `ColorSpaceTransform`, `GroupTransform`, the
//! `ACES_GamutComp13` fixed function, and the `BuiltinTransform` styles
//! listed in [`builtin_names`] (ACES spaces, camera inputs, displays and
//! the ACES 1.x output transforms). Anything else — ACES 2.0 output
//! transforms, looks, named transforms, other fixed functions — makes
//! the processor fail with a message naming it, never a silent identity.

use crate::aces::DisplayTarget;
use crate::camera::{Gamut, LogCurve};
use crate::lut::{Lut1d, Lut3d, read_lut};
use crate::math::Mat3;
use crate::primaries::{Primaries, rgb_to_rgb};
use crate::transfer::{Transfer, hlg_oetf, pq_eotf, pq_inverse_eotf};
use crate::yaml::{self, Yaml};
use std::path::{Path, PathBuf};
use std::sync::Arc;

type V3 = [f64; 3];

/// A parsed transform (before direction and file loading are resolved).
#[derive(Clone, Debug, PartialEq)]
pub struct Transform {
    /// The tag without `Transform` (`Matrix`, `Group`, `Builtin`, …).
    pub kind: String,
    /// Its fields.
    pub node: Yaml,
}

impl Transform {
    fn from_yaml(y: &Yaml) -> Option<Transform> {
        let tag = y.tag()?;
        Some(Transform { kind: tag.trim_end_matches("Transform").to_string(), node: y.untagged().clone() })
    }

    fn inverse(&self) -> bool {
        self.node.get("direction").and_then(Yaml::as_str) == Some("inverse")
    }

    fn f(&self, k: &str, d: f64) -> f64 {
        self.node.get(k).and_then(Yaml::as_f64).unwrap_or(d)
    }

    /// A per-channel parameter: a number or a 3/4-list.
    fn f3(&self, k: &str, d: f64) -> V3 {
        match self.node.get(k).map(Yaml::numbers) {
            Some(v) if v.len() >= 3 => [v[0], v[1], v[2]],
            Some(v) if v.len() == 1 => [v[0]; 3],
            _ => [d; 3],
        }
    }

    fn s(&self, k: &str) -> Option<&str> {
        self.node.get(k).and_then(Yaml::as_str)
    }
}

/// A colour space of a config.
#[derive(Clone, Debug, PartialEq)]
pub struct ColourSpace {
    pub name: String,
    pub aliases: Vec<String>,
    pub family: String,
    pub encoding: String,
    pub description: String,
    pub is_data: bool,
    /// Lives in the display-referred reference space (v2 `display_colorspaces`).
    pub display_referred: bool,
    pub to_reference: Option<Transform>,
    pub from_reference: Option<Transform>,
}

/// A view of a display.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub name: String,
    /// A plain colour space view.
    pub colorspace: Option<String>,
    /// Or a view transform plus a display colour space.
    pub view_transform: Option<String>,
    pub display_colorspace: Option<String>,
    pub looks: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Display {
    pub name: String,
    pub views: Vec<View>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ViewTransform {
    pub name: String,
    pub from_scene: Option<Transform>,
    pub to_scene: Option<Transform>,
    pub from_display: Option<Transform>,
    pub to_display: Option<Transform>,
}

/// A loaded OCIO config.
#[derive(Clone, Debug)]
pub struct Config {
    pub name: String,
    pub version: f64,
    pub roles: Vec<(String, String)>,
    pub spaces: Vec<ColourSpace>,
    pub displays: Vec<Display>,
    pub view_transforms: Vec<ViewTransform>,
    pub default_view_transform: Option<String>,
    /// Folders FileTransform `src` paths are looked up in.
    pub search_paths: Vec<PathBuf>,
}

impl Config {
    /// Reads a `.ocio` file.
    pub fn load(path: &Path) -> Result<Config, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        Config::parse(&text, &dir).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Parses config text; `dir` is the config's folder (for LUT files).
    pub fn parse(text: &str, dir: &Path) -> Result<Config, String> {
        let y = yaml::parse(text)?;
        if y.get("colorspaces").is_none() {
            return Err("not an OCIO config (no colorspaces)".into());
        }
        let version = y.get("ocio_profile_version").and_then(Yaml::as_f64).unwrap_or(1.0);
        let roles = y.get("roles").map(|r| r.as_map().iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect()).unwrap_or_default();
        let mut search_paths = Vec::new();
        for p in y.get("search_path").map(Yaml::strings).unwrap_or_default() {
            for part in p.split(':').filter(|s| !s.is_empty()) {
                search_paths.push(dir.join(part));
            }
        }
        search_paths.push(dir.to_path_buf());
        let space = |c: &Yaml, display: bool| -> ColourSpace {
            let t = |k: &str| c.get(k).and_then(Transform::from_yaml);
            let (to, from) = if display {
                (t("to_display_reference"), t("from_display_reference"))
            } else {
                (t("to_scene_reference").or_else(|| t("to_reference")), t("from_scene_reference").or_else(|| t("from_reference")))
            };
            ColourSpace {
                name: c.get("name").and_then(Yaml::as_str).unwrap_or("").to_string(),
                aliases: c.get("aliases").map(Yaml::strings).unwrap_or_default(),
                family: c.get("family").and_then(Yaml::as_str).unwrap_or("").to_string(),
                encoding: c.get("encoding").and_then(Yaml::as_str).unwrap_or("").to_string(),
                description: c.get("description").and_then(Yaml::as_str).unwrap_or("").trim().to_string(),
                is_data: c.get("isdata").and_then(Yaml::as_bool).unwrap_or(false),
                display_referred: display,
                to_reference: to,
                from_reference: from,
            }
        };
        let mut spaces: Vec<ColourSpace> = y.get("colorspaces").map(|s| s.as_seq().iter().map(|c| space(c, false)).collect()).unwrap_or_default();
        if let Some(d) = y.get("display_colorspaces") {
            spaces.extend(d.as_seq().iter().map(|c| space(c, true)));
        }
        let view = |v: &Yaml| View {
            name: v.get("name").and_then(Yaml::as_str).unwrap_or("").to_string(),
            colorspace: v.get("colorspace").and_then(Yaml::as_str).map(str::to_owned),
            view_transform: v.get("view_transform").and_then(Yaml::as_str).map(str::to_owned),
            display_colorspace: v.get("display_colorspace").and_then(Yaml::as_str).map(str::to_owned),
            looks: v.get("looks").and_then(Yaml::as_str).map(str::to_owned),
        };
        let shared: Vec<View> = y.get("shared_views").map(|s| s.as_seq().iter().map(view).collect()).unwrap_or_default();
        let mut displays = Vec::new();
        for (name, list) in y.get("displays").map(Yaml::as_map).unwrap_or(&[]) {
            let mut views = Vec::new();
            for v in list.as_seq() {
                if v.tag() == Some("Views") {
                    for n in v.strings() {
                        if let Some(sv) = shared.iter().find(|s| s.name == n) {
                            views.push(sv.clone());
                        }
                    }
                } else {
                    views.push(view(v));
                }
            }
            for v in &mut views {
                if v.display_colorspace.as_deref() == Some("<USE_DISPLAY_NAME>") {
                    v.display_colorspace = Some(name.clone());
                }
            }
            displays.push(Display { name: name.clone(), views });
        }
        let view_transforms = y
            .get("view_transforms")
            .map(|s| {
                s.as_seq()
                    .iter()
                    .map(|v| {
                        let t = |k: &str| v.get(k).and_then(Transform::from_yaml);
                        ViewTransform {
                            name: v.get("name").and_then(Yaml::as_str).unwrap_or("").to_string(),
                            from_scene: t("from_scene_reference").or_else(|| t("from_reference")),
                            to_scene: t("to_scene_reference").or_else(|| t("to_reference")),
                            from_display: t("from_display_reference"),
                            to_display: t("to_display_reference"),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Config {
            name: y.get("name").and_then(Yaml::as_str).unwrap_or("").to_string(),
            version,
            roles,
            spaces,
            displays,
            view_transforms,
            default_view_transform: y.get("default_view_transform").and_then(Yaml::as_str).map(str::to_owned),
            search_paths,
        })
    }

    /// A colour space by name, alias or role (case-insensitive).
    pub fn space(&self, name: &str) -> Option<&ColourSpace> {
        let n = name.trim();
        let eq = |a: &str| a.eq_ignore_ascii_case(n);
        if let Some(s) = self.spaces.iter().find(|s| eq(&s.name) || s.aliases.iter().any(|a| eq(a))) {
            return Some(s);
        }
        let role = self.roles.iter().find(|(r, _)| eq(r))?;
        self.spaces.iter().find(|s| s.name.eq_ignore_ascii_case(&role.1) || s.aliases.iter().any(|a| a.eq_ignore_ascii_case(&role.1)))
    }

    /// The colour space a role points at.
    pub fn role(&self, role: &str) -> Option<&ColourSpace> {
        let r = self.roles.iter().find(|(k, _)| k == role)?;
        self.space(&r.1)
    }

    /// The config's colour space equal to Studio's working space (linear
    /// light, BT.709 primaries, D65): found by its usual names, or `None`.
    pub fn working_space(&self) -> Option<&ColourSpace> {
        const NAMES: [&str; 10] = [
            "lin_rec709_srgb",
            "lin_rec709",
            "lin_srgb",
            "Linear Rec.709 (sRGB)",
            "Utility - Linear - sRGB",
            "Utility - Linear - Rec.709",
            "linear_srgb",
            "Linear sRGB",
            "srgb_linear",
            "Linear BT.709",
        ];
        NAMES.iter().find_map(|n| self.space(n))
    }

    fn view_transform(&self, name: &str) -> Option<&ViewTransform> {
        self.view_transforms.iter().find(|v| v.name.eq_ignore_ascii_case(name))
    }

    /// Ops taking `cs` to its reference space.
    fn to_reference(&self, cs: &ColourSpace, depth: usize) -> Result<Vec<Op>, String> {
        if cs.is_data {
            return Ok(Vec::new());
        }
        match (&cs.to_reference, &cs.from_reference) {
            (Some(t), _) => self.compile(t, false, depth),
            (None, Some(t)) => self.compile(t, true, depth),
            (None, None) => Ok(Vec::new()),
        }
    }

    fn from_reference(&self, cs: &ColourSpace, depth: usize) -> Result<Vec<Op>, String> {
        if cs.is_data {
            return Ok(Vec::new());
        }
        match (&cs.from_reference, &cs.to_reference) {
            (Some(t), _) => self.compile(t, false, depth),
            (None, Some(t)) => self.compile(t, true, depth),
            (None, None) => Ok(Vec::new()),
        }
    }

    /// Scene reference → display reference through the default view
    /// transform (OCIO's bridge between the two reference spaces).
    fn scene_to_display(&self, depth: usize) -> Result<Vec<Op>, String> {
        let vt = self
            .default_view_transform
            .as_deref()
            .and_then(|n| self.view_transform(n))
            .or_else(|| self.view_transforms.first())
            .ok_or("this config has display colour spaces but no view transform to reach them")?;
        self.vt_forward(vt, depth)
    }

    fn vt_forward(&self, vt: &ViewTransform, depth: usize) -> Result<Vec<Op>, String> {
        match (&vt.from_scene, &vt.to_display, &vt.from_display, &vt.to_scene) {
            (Some(t), _, _, _) => self.compile(t, false, depth),
            (None, _, _, Some(t)) => self.compile(t, true, depth),
            // A display-to-display view transform applied after the
            // default scene bridge.
            (None, Some(t), _, None) => self.compile(t, false, depth),
            (None, None, Some(t), None) => self.compile(t, true, depth),
            _ => Ok(Vec::new()),
        }
    }

    fn convert_ops(&self, src: &ColourSpace, dst: &ColourSpace, depth: usize) -> Result<Vec<Op>, String> {
        if depth > 16 {
            return Err("colour space transforms nest too deeply".into());
        }
        // Data (non-colour) spaces pass values through, as in OCIO.
        if src.is_data || dst.is_data {
            return Ok(Vec::new());
        }
        let mut ops = self.to_reference(src, depth + 1)?;
        match (src.display_referred, dst.display_referred) {
            (false, true) => ops.extend(self.scene_to_display(depth + 1)?),
            (true, false) => {
                let fwd = self.scene_to_display(depth + 1)?;
                ops.extend(invert_ops(fwd)?);
            }
            _ => {}
        }
        ops.extend(self.from_reference(dst, depth + 1)?);
        Ok(ops)
    }

    /// A processor from colour space `src` to `dst` (names, aliases or roles).
    pub fn processor(&self, src: &str, dst: &str) -> Result<Processor, String> {
        let s = self.space(src).ok_or_else(|| format!("no colour space \"{src}\" in {}", self.label()))?;
        let d = self.space(dst).ok_or_else(|| format!("no colour space \"{dst}\" in {}", self.label()))?;
        Ok(Processor { ops: self.convert_ops(s, d, 0)? })
    }

    /// A processor from `src` through `display` / `view`, ending in that
    /// display's encoded values.
    pub fn view_processor(&self, src: &str, display: &str, view: &str) -> Result<Processor, String> {
        let s = self.space(src).ok_or_else(|| format!("no colour space \"{src}\" in {}", self.label()))?;
        let d = self.displays.iter().find(|d| d.name.eq_ignore_ascii_case(display)).ok_or_else(|| format!("no display \"{display}\"; displays: {}", self.displays.iter().map(|d| d.name.as_str()).collect::<Vec<_>>().join(", ")))?;
        let v = d.views.iter().find(|v| v.name.eq_ignore_ascii_case(view)).ok_or_else(|| format!("display \"{}\" has no view \"{view}\"; views: {}", d.name, d.views.iter().map(|v| v.name.as_str()).collect::<Vec<_>>().join(", ")))?;
        if v.looks.as_deref().is_some_and(|l| !l.is_empty()) {
            return Err(format!("view \"{}\" uses looks, which Zornov does not apply", v.name));
        }
        if let Some(cs) = &v.colorspace {
            let dst = self.space(cs).ok_or_else(|| format!("view \"{}\" names unknown colour space \"{cs}\"", v.name))?;
            return Ok(Processor { ops: self.convert_ops(s, dst, 0)? });
        }
        let vt_name = v.view_transform.as_deref().ok_or_else(|| format!("view \"{}\" has neither a colour space nor a view transform", v.name))?;
        let vt = self.view_transform(vt_name).ok_or_else(|| format!("unknown view transform \"{vt_name}\""))?;
        let dcs_name = v.display_colorspace.as_deref().unwrap_or(&d.name);
        let dcs = self.space(dcs_name).ok_or_else(|| format!("unknown display colour space \"{dcs_name}\""))?;
        let mut ops = self.to_reference(s, 1)?;
        if s.display_referred {
            ops.extend(invert_ops(self.scene_to_display(1)?)?);
        }
        ops.extend(self.vt_forward(vt, 1)?);
        ops.extend(self.from_reference(dcs, 1)?);
        Ok(Processor { ops })
    }

    fn label(&self) -> String {
        if self.name.is_empty() { "the config".into() } else { format!("config \"{}\"", self.name) }
    }

    fn find_file(&self, src: &str) -> Option<PathBuf> {
        let p = Path::new(src);
        if p.is_absolute() {
            return p.exists().then(|| p.to_path_buf());
        }
        self.search_paths.iter().map(|d| d.join(src)).find(|f| f.exists())
    }

    fn compile(&self, t: &Transform, inverse: bool, depth: usize) -> Result<Vec<Op>, String> {
        let inv = inverse ^ t.inverse();
        let ops = match t.kind.as_str() {
            "Group" => {
                let mut ops = Vec::new();
                let kids: Vec<Transform> = t.node.get("children").map(|c| c.as_seq().iter().filter_map(Transform::from_yaml).collect()).unwrap_or_default();
                if inv {
                    for k in kids.iter().rev() {
                        ops.extend(self.compile(k, true, depth + 1)?);
                    }
                } else {
                    for k in &kids {
                        ops.extend(self.compile(k, false, depth + 1)?);
                    }
                }
                return Ok(ops);
            }
            "ColorSpace" => {
                let (a, b) = (t.s("src").unwrap_or(""), t.s("dst").unwrap_or(""));
                let (a, b) = if inv { (b, a) } else { (a, b) };
                let s = self.space(a).ok_or_else(|| format!("ColorSpaceTransform: unknown \"{a}\""))?;
                let d = self.space(b).ok_or_else(|| format!("ColorSpaceTransform: unknown \"{b}\""))?;
                return self.convert_ops(s, d, depth + 1);
            }
            "File" => {
                let src = t.s("src").ok_or("FileTransform without src")?;
                let path = self.find_file(src).ok_or_else(|| format!("LUT file \"{src}\" not found next to the config"))?;
                let lut = load_lut_file(&path)?;
                let mut v = Vec::new();
                if let Some(l1) = lut.0 {
                    v.push(Op::Lut1(Arc::new(l1), false));
                }
                if let Some(l3) = lut.1 {
                    v.push(Op::Lut3(Arc::new(l3)));
                }
                if inv {
                    return invert_ops(v);
                }
                return Ok(v);
            }
            "Builtin" => builtin(t.s("style").unwrap_or(""))?,
            "Matrix" => {
                let m = t.node.get("matrix").map(Yaml::numbers).unwrap_or_default();
                let off = t.node.get("offset").map(Yaml::numbers).unwrap_or_default();
                let mm = if m.len() >= 16 {
                    Mat3([[m[0], m[1], m[2]], [m[4], m[5], m[6]], [m[8], m[9], m[10]]])
                } else if m.len() == 9 {
                    Mat3([[m[0], m[1], m[2]], [m[3], m[4], m[5]], [m[6], m[7], m[8]]])
                } else {
                    Mat3::IDENTITY
                };
                let o = if off.len() >= 3 { [off[0], off[1], off[2]] } else { [0.0; 3] };
                vec![Op::Matrix(mm, o)]
            }
            "Exponent" => {
                let e = t.f3("value", 1.0);
                let style = t.s("style").unwrap_or("clamp").to_string();
                vec![Op::Exponent(e, style)]
            }
            "ExponentWithLinear" => vec![Op::MonCurve { gamma: t.f3("gamma", 1.0), offset: t.f3("offset", 0.0), inverse: false }],
            "Log" => vec![Op::LogAffine { base: t.f("base", 2.0), log_slope: [1.0; 3], log_offset: [0.0; 3], lin_slope: [1.0; 3], lin_offset: [0.0; 3], inverse: false }],
            "LogAffine" => vec![Op::LogAffine {
                base: t.f("base", 2.0),
                log_slope: t.f3("log_side_slope", 1.0),
                log_offset: t.f3("log_side_offset", 0.0),
                lin_slope: t.f3("lin_side_slope", 1.0),
                lin_offset: t.f3("lin_side_offset", 0.0),
                inverse: false,
            }],
            "LogCamera" => {
                let base = t.f("base", 2.0);
                let p = LogCam {
                    base,
                    log_slope: t.f3("log_side_slope", 1.0),
                    log_offset: t.f3("log_side_offset", 0.0),
                    lin_slope: t.f3("lin_side_slope", 1.0),
                    lin_offset: t.f3("lin_side_offset", 0.0),
                    lin_break: t.f3("lin_side_break", 0.0),
                    linear_slope: t.node.get("linear_slope").map(|_| t.f3("linear_slope", 1.0)),
                };
                vec![Op::LogCamera(Box::new(p), false)]
            }
            "CDL" => {
                let style_noclamp = t.s("style").is_some_and(|s| s.to_ascii_lowercase().contains("noclamp"));
                vec![Op::Cdl { slope: t.f3("slope", 1.0), offset: t.f3("offset", 0.0), power: t.f3("power", 1.0), sat: t.f("sat", 1.0), clamp: !style_noclamp, inverse: false }]
            }
            "Range" => {
                let g = |k: &str| t.node.get(k).and_then(Yaml::as_f64);
                vec![Op::Range { min_in: g("min_in_value"), max_in: g("max_in_value"), min_out: g("min_out_value"), max_out: g("max_out_value") }]
            }
            "Allocation" => {
                let vars = t.node.get("vars").map(Yaml::numbers).unwrap_or_default();
                let lg2 = t.s("allocation") == Some("lg2");
                let (lo, hi) = (vars.first().copied().unwrap_or(if lg2 { -10.0 } else { 0.0 }), vars.get(1).copied().unwrap_or(if lg2 { 6.0 } else { 1.0 }));
                let off = vars.get(2).copied().unwrap_or(0.0);
                vec![Op::Allocation { lg2, lo, hi, offset: off, inverse: false }]
            }
            "FixedFunction" => {
                let style = t.s("style").unwrap_or("");
                if style.eq_ignore_ascii_case("ACES_GamutComp13") {
                    let p = t.node.get("params").map(Yaml::numbers).unwrap_or_default();
                    let gc = if p.len() >= 7 { GamutComp { lim: [p[0], p[1], p[2]], thr: [p[3], p[4], p[5]], pwr: p[6] } } else { GamutComp::ACES13 };
                    vec![Op::GamutComp(gc, false)]
                } else {
                    return Err(format!("FixedFunctionTransform style \"{style}\" is not supported"));
                }
            }
            other => return Err(format!("{other}Transform is not supported")),
        };
        if inv { invert_ops(ops) } else { Ok(ops) }
    }
}

/// LogCameraTransform parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct LogCam {
    base: f64,
    log_slope: V3,
    log_offset: V3,
    lin_slope: V3,
    lin_offset: V3,
    lin_break: V3,
    linear_slope: Option<V3>,
}

impl LogCam {
    fn seg(&self, c: usize) -> (f64, f64) {
        let lb = self.base.ln();
        let at = self.lin_slope[c] * self.lin_break[c] + self.lin_offset[c];
        let log_at = self.log_slope[c] * at.max(1e-30).ln() / lb + self.log_offset[c];
        let slope = self.linear_slope.map(|s| s[c]).unwrap_or(self.log_slope[c] * self.lin_slope[c] / (at * lb));
        (slope, log_at - slope * self.lin_break[c])
    }

    fn fwd(&self, x: f64, c: usize) -> f64 {
        if x <= self.lin_break[c] {
            let (s, o) = self.seg(c);
            s * x + o
        } else {
            self.log_slope[c] * (self.lin_slope[c] * x + self.lin_offset[c]).max(1e-30).ln() / self.base.ln() + self.log_offset[c]
        }
    }

    fn inv(&self, y: f64, c: usize) -> f64 {
        let (s, o) = self.seg(c);
        let y_break = s * self.lin_break[c] + o;
        if y <= y_break {
            (y - o) / s
        } else {
            (self.base.powf((y - self.log_offset[c]) / self.log_slope[c]) - self.lin_offset[c]) / self.lin_slope[c]
        }
    }
}

/// ACES Reference Gamut Compression parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GamutComp {
    pub lim: V3,
    pub thr: V3,
    pub pwr: f64,
}

impl GamutComp {
    /// The ACES 1.3 defaults (cyan, magenta, yellow).
    pub const ACES13: GamutComp = GamutComp { lim: [1.147, 1.264, 1.312], thr: [0.815, 0.803, 0.880], pwr: 1.2 };

    /// Applies (or reverses) the compression to AP1 linear values.
    pub fn apply(&self, rgb: V3, inverse: bool) -> V3 {
        let ach = rgb[0].max(rgb[1]).max(rgb[2]);
        if ach == 0.0 {
            return rgb;
        }
        let mut out = rgb;
        for c in 0..3 {
            let dist = (ach - rgb[c]) / ach.abs();
            let (thr, lim, p) = (self.thr[c], self.lim[c], self.pwr);
            let scale = (lim - thr) / (((1.0 - thr) / (lim - thr)).powf(-p) - 1.0).powf(1.0 / p);
            let cd = if dist < thr {
                dist
            } else if !inverse {
                let nd = (dist - thr) / scale;
                thr + scale * nd / (1.0 + nd.powf(p)).powf(1.0 / p)
            } else {
                let nd = (dist - thr) / scale;
                if nd >= 1.0 { dist } else { thr + scale * (-(nd.powf(p) / (nd.powf(p) - 1.0))).powf(1.0 / p) }
            };
            out[c] = ach - cd * ach.abs();
        }
        out
    }
}

/// One step of a processor.
#[derive(Clone, Debug)]
enum Op {
    Matrix(Mat3, V3),
    Exponent(V3, String),
    MonCurve { gamma: V3, offset: V3, inverse: bool },
    LogAffine { base: f64, log_slope: V3, log_offset: V3, lin_slope: V3, lin_offset: V3, inverse: bool },
    LogCamera(Box<LogCam>, bool),
    Cdl { slope: V3, offset: V3, power: V3, sat: f64, clamp: bool, inverse: bool },
    Range { min_in: Option<f64>, max_in: Option<f64>, min_out: Option<f64>, max_out: Option<f64> },
    Allocation { lg2: bool, lo: f64, hi: f64, offset: f64, inverse: bool },
    Lut1(Arc<Lut1d>, bool),
    Lut3(Arc<Lut3d>),
    Curve(LogCurve, bool),
    Transfer(Transfer, bool),
    /// PQ with linear 1.0 = 100 cd/m² (OCIO's convention).
    Pq100(bool),
    /// HLG for a 1000 cd/m² display, linear 1.0 = 100 cd/m².
    Hlg1000(bool),
    AcesOutput(DisplayTarget, bool),
    GamutComp(GamutComp, bool),
}

fn invert_ops(ops: Vec<Op>) -> Result<Vec<Op>, String> {
    let mut out = Vec::with_capacity(ops.len());
    for op in ops.into_iter().rev() {
        out.push(match op {
            Op::Matrix(m, o) => {
                let inv = m.inverse().ok_or("a matrix in the config cannot be inverted")?;
                let no = inv.mul_vec64(o).map(|v| -v);
                Op::Matrix(inv, no)
            }
            Op::Exponent(e, s) => Op::Exponent(e.map(|v| 1.0 / v), s),
            Op::MonCurve { gamma, offset, inverse } => Op::MonCurve { gamma, offset, inverse: !inverse },
            Op::LogAffine { base, log_slope, log_offset, lin_slope, lin_offset, inverse } => Op::LogAffine { base, log_slope, log_offset, lin_slope, lin_offset, inverse: !inverse },
            Op::LogCamera(p, i) => Op::LogCamera(p, !i),
            Op::Cdl { slope, offset, power, sat, clamp, inverse } => Op::Cdl { slope, offset, power, sat, clamp, inverse: !inverse },
            Op::Range { min_in, max_in, min_out, max_out } => Op::Range { min_in: min_out, max_in: max_out, min_out: min_in, max_out: max_in },
            Op::Allocation { lg2, lo, hi, offset, inverse } => Op::Allocation { lg2, lo, hi, offset, inverse: !inverse },
            Op::Lut1(l, i) => Op::Lut1(l, !i),
            Op::Lut3(_) => return Err("a 3D LUT in the config would have to be inverted, which Zornov does not do".into()),
            Op::Curve(c, i) => Op::Curve(c, !i),
            Op::Transfer(t, i) => Op::Transfer(t, !i),
            Op::Pq100(i) => Op::Pq100(!i),
            Op::Hlg1000(i) => Op::Hlg1000(!i),
            Op::AcesOutput(t, i) => {
                if t.is_hdr() {
                    return Err("inverting an ACES HDR output transform is not supported".into());
                }
                Op::AcesOutput(t, !i)
            }
            Op::GamutComp(g, i) => Op::GamutComp(g, !i),
        });
    }
    Ok(out)
}

fn powm(x: f64, e: f64, style: &str) -> f64 {
    match style {
        "mirror" => x.signum() * x.abs().powf(e),
        "pass_thru" if x < 0.0 => x,
        "linear" if x < 0.0 => x,
        _ => x.max(0.0).powf(e),
    }
}

fn moncurve_fwd(x: f64, g: f64, o: f64) -> f64 {
    // Encoded → linear (OCIO's forward direction), with a linear toe
    // that meets the power segment with equal value at the break.
    if g <= 1.0 || o == 0.0 {
        return x.max(0.0).powf(g);
    }
    let brk = o / (g - 1.0);
    let slope = ((brk + o) / (1.0 + o)).powf(g) / brk;
    if x <= brk { x * slope } else { ((x + o) / (1.0 + o)).powf(g) }
}

fn moncurve_inv(y: f64, g: f64, o: f64) -> f64 {
    if g <= 1.0 || o == 0.0 {
        return y.max(0.0).powf(1.0 / g);
    }
    let brk = o / (g - 1.0);
    let slope = ((brk + o) / (1.0 + o)).powf(g) / brk;
    let yb = brk * slope;
    if y <= yb { y / slope } else { (1.0 + o) * y.powf(1.0 / g) - o }
}

fn lut1_inverse(l: &Lut1d, y: f64, c: usize) -> f64 {
    let n = l.data.len();
    let v: Vec<f64> = l.data.iter().map(|p| p[c] as f64).collect();
    let (lo, hi) = (l.domain_min[c] as f64, l.domain_max[c] as f64);
    let increasing = v[n - 1] >= v[0];
    let pos = |k: f64| lo + (hi - lo) * k / (n - 1) as f64;
    for i in 0..n - 1 {
        let (a, b) = (v[i], v[i + 1]);
        let inside = if increasing { y >= a && y <= b } else { y <= a && y >= b };
        if inside {
            let f = if (b - a).abs() < 1e-12 { 0.0 } else { (y - a) / (b - a) };
            return pos(i as f64 + f);
        }
    }
    let below = if increasing { y < v[0] } else { y > v[0] };
    if below { lo } else { hi }
}

impl Op {
    fn apply(&self, c: V3) -> V3 {
        match self {
            Op::Matrix(m, o) => {
                let r = m.mul_vec64(c);
                [r[0] + o[0], r[1] + o[1], r[2] + o[2]]
            }
            Op::Exponent(e, s) => [0, 1, 2].map(|i| powm(c[i], e[i], s)),
            Op::MonCurve { gamma, offset, inverse } => [0, 1, 2].map(|i| if *inverse { moncurve_inv(c[i], gamma[i], offset[i]) } else { moncurve_fwd(c[i], gamma[i], offset[i]) }),
            Op::LogAffine { base, log_slope, log_offset, lin_slope, lin_offset, inverse } => [0, 1, 2].map(|i| {
                if *inverse {
                    (base.powf((c[i] - log_offset[i]) / log_slope[i]) - lin_offset[i]) / lin_slope[i]
                } else {
                    log_slope[i] * (lin_slope[i] * c[i] + lin_offset[i]).max(f64::MIN_POSITIVE).ln() / base.ln() + log_offset[i]
                }
            }),
            Op::LogCamera(p, inverse) => [0, 1, 2].map(|i| if *inverse { p.inv(c[i], i) } else { p.fwd(c[i], i) }),
            Op::Cdl { slope, offset, power, sat, clamp, inverse } => {
                let luma = |v: V3| 0.2126 * v[0] + 0.7152 * v[1] + 0.0722 * v[2];
                let cl = |v: f64| if *clamp { v.clamp(0.0, 1.0) } else { v };
                if !*inverse {
                    let v = [0, 1, 2].map(|i| {
                        let x = c[i] * slope[i] + offset[i];
                        if *clamp { cl(x).powf(power[i]) } else { powm(x, power[i], "mirror") }
                    });
                    let l = luma(v);
                    v.map(|x| cl(l + sat * (x - l)))
                } else {
                    let l = luma(c);
                    let v = c.map(|x| l + (x - l) / sat);
                    [0, 1, 2].map(|i| {
                        let x = if *clamp { cl(v[i]).powf(1.0 / power[i]) } else { powm(v[i], 1.0 / power[i], "mirror") };
                        cl((x - offset[i]) / slope[i])
                    })
                }
            }
            Op::Range { min_in, max_in, min_out, max_out } => c.map(|x| match (min_in, max_in, min_out, max_out) {
                (Some(a), Some(b), Some(p), Some(q)) => (p + (x - a) * (q - p) / (b - a)).clamp(p.min(*q), p.max(*q)),
                _ => {
                    let mut y = x;
                    if let (Some(a), Some(p)) = (min_in, min_out) {
                        y = (y - a + p).max(*p);
                    }
                    if let (Some(b), Some(q)) = (max_in, max_out) {
                        y = (y - b + q).min(*q);
                    }
                    y
                }
            }),
            Op::Allocation { lg2, lo, hi, offset, inverse } => c.map(|x| {
                if *lg2 {
                    if *inverse { 2f64.powf(x * (hi - lo) + lo) - offset } else { ((x + offset).max(1e-30).log2() - lo) / (hi - lo) }
                } else if *inverse {
                    x * (hi - lo) + lo
                } else {
                    (x - lo) / (hi - lo)
                }
            }),
            Op::Lut1(l, inverse) => {
                if *inverse {
                    [0, 1, 2].map(|i| lut1_inverse(l, c[i], i))
                } else {
                    l.sample(c.map(|v| v as f32)).map(|v| v as f64)
                }
            }
            Op::Lut3(l) => l.sample(c.map(|v| v as f32)).map(|v| v as f64),
            Op::Curve(k, inverse) => c.map(|v| if *inverse { k.encode(v as f32) as f64 } else { k.decode(v as f32) as f64 }),
            Op::Transfer(t, inverse) => c.map(|v| if *inverse { t.encode(v as f32) as f64 } else { t.decode(v as f32) as f64 }),
            Op::Pq100(inverse) => c.map(|v| if *inverse { pq_inverse_eotf((v * 100.0) as f32) as f64 } else { pq_eotf(v as f32) as f64 / 100.0 }),
            Op::Hlg1000(inverse) => {
                if !*inverse {
                    // Display linear (1.0 = 100 cd/m²) → HLG signal.
                    let d = c.map(|v| (v * 100.0 / 1000.0).max(0.0));
                    let yd = 0.2627 * d[0] + 0.6780 * d[1] + 0.0593 * d[2];
                    if yd <= 0.0 {
                        return [0.0; 3];
                    }
                    let k = yd.powf((1.0 - 1.2) / 1.2);
                    d.map(|v| hlg_oetf((v * k) as f32) as f64)
                } else {
                    let s = c.map(|v| Transfer::Hlg.decode(v as f32) as f64);
                    let ys = 0.2627 * s[0] + 0.6780 * s[1] + 0.0593 * s[2];
                    let k = ys.max(0.0).powf(0.2);
                    s.map(|v| v * k * 1000.0 / 100.0)
                }
            }
            Op::AcesOutput(t, inverse) => {
                let p = match t {
                    DisplayTarget::Rec709 => Primaries::BT709,
                    DisplayTarget::P3D65 => Primaries::P3_D65,
                    _ => Primaries::BT2020,
                };
                if !*inverse {
                    let out = t.output_transform(c);
                    let scale = if t.is_hdr() { 1.0 / 100.0 } else { 1.0 };
                    p.rgb_to_xyz().mul_vec64(out.map(|v| v * scale))
                } else {
                    let rgb = p.xyz_to_rgb().mul_vec64(c);
                    t.inverse_output_transform(rgb)
                }
            }
            Op::GamutComp(g, inverse) => {
                let m = rgb_to_rgb(&Primaries::ACES_AP0, &Primaries::ACES_AP1);
                let ap1 = m.mul_vec64(c);
                let out = g.apply(ap1, *inverse);
                m.inverse().expect("ap1").mul_vec64(out)
            }
        }
    }
}

/// Loads a LUT for a FileTransform: (1D, 3D).
fn load_lut_file(path: &Path) -> Result<(Option<Lut1d>, Option<Lut3d>), String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "cube" | "3dl" => {
            let f = read_lut(path)?;
            Ok((f.lut1d, f.lut3d))
        }
        "spi1d" => {
            let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            parse_spi1d(&text).map(|l| (Some(l), None)).map_err(|e| format!("{}: {e}", path.display()))
        }
        "spi3d" => {
            let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            parse_spi3d(&text).map(|l| (None, Some(l))).map_err(|e| format!("{}: {e}", path.display()))
        }
        other => Err(format!("{}: LUT format .{other} is not supported (use .cube, .3dl, .spi1d or .spi3d)", path.display())),
    }
}

/// Sony Pictures Imageworks `.spi1d`.
pub fn parse_spi1d(text: &str) -> Result<Lut1d, String> {
    let (mut from, mut comps, mut data) = ((0.0f32, 1.0f32), 1usize, Vec::new());
    let mut in_data = false;
    for l in text.lines() {
        let l = l.trim();
        if l.is_empty() {
            continue;
        }
        if l.starts_with('{') {
            in_data = true;
            continue;
        }
        if l.starts_with('}') {
            break;
        }
        if in_data {
            let v: Vec<f32> = l.split_whitespace().filter_map(|s| s.parse().ok()).collect();
            data.push(match v.len() {
                1 => [v[0]; 3],
                n if n >= 3 => [v[0], v[1], v[2]],
                _ => return Err(format!("bad spi1d row {l:?}")),
            });
            continue;
        }
        let mut p = l.split_whitespace();
        match p.next() {
            Some("From") => {
                let a: Vec<f32> = p.filter_map(|s| s.parse().ok()).collect();
                if a.len() == 2 {
                    from = (a[0], a[1]);
                }
            }
            Some("Components") => comps = p.next().and_then(|s| s.parse().ok()).unwrap_or(1),
            _ => {}
        }
    }
    let _ = comps;
    if data.len() < 2 {
        return Err("spi1d has no table".into());
    }
    Ok(Lut1d { data, domain_min: [from.0; 3], domain_max: [from.1; 3] })
}

/// Sony Pictures Imageworks `.spi3d`.
pub fn parse_spi3d(text: &str) -> Result<Lut3d, String> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let _header = lines.next();
    let sizes: Vec<usize> = lines.next().ok_or("spi3d: no size line")?.split_whitespace().filter_map(|s| s.parse().ok()).collect();
    let n = *sizes.first().ok_or("spi3d: bad size")?;
    if !(2..=256).contains(&n) {
        return Err(format!("spi3d: unsupported size {n}"));
    }
    let mut data = vec![[0.0f32; 3]; n * n * n];
    for l in lines {
        let v: Vec<f32> = l.split_whitespace().filter_map(|s| s.parse().ok()).collect();
        if v.len() < 6 {
            continue;
        }
        let (r, g, b) = (v[0] as usize, v[1] as usize, v[2] as usize);
        if r < n && g < n && b < n {
            data[(b * n + g) * n + r] = [v[3], v[4], v[5]];
        }
    }
    Ok(Lut3d { size: n, data, domain_min: [0.0; 3], domain_max: [1.0; 3] })
}

fn gamut_to_ap0(g: Gamut) -> Op {
    Op::Matrix(rgb_to_rgb(&g.primaries(), &Primaries::ACES_AP0), [0.0; 3])
}

fn xyz_d65_to(p: &Primaries) -> Op {
    Op::Matrix(p.xyz_to_rgb(), [0.0; 3])
}

/// AP0 / AP1 → CIE XYZ with D65 white (Bradford).
fn aces_to_xyz_d65(p: &Primaries) -> Op {
    let xyz = crate::primaries::bradford(p.white, crate::primaries::D65).mul(&p.rgb_to_xyz());
    Op::Matrix(xyz, [0.0; 3])
}

/// Styles [`builtin`] understands.
pub fn builtin_names() -> Vec<&'static str> {
    vec![
        "IDENTITY",
        "ACEScct_to_ACES2065-1",
        "ACEScc_to_ACES2065-1",
        "ACEScg_to_ACES2065-1",
        "ARRI_ALEXA-LOGC-EI800-AWG_to_ACES2065-1",
        "ARRI_LOGC4_to_ACES2065-1",
        "SONY_SLOG3-SGAMUT3_to_ACES2065-1",
        "SONY_SLOG3-SGAMUT3.CINE_to_ACES2065-1",
        "PANASONIC_VLOG-VGAMUT_to_ACES2065-1",
        "CANON_CLOG2-CGAMUT_to_ACES2065-1",
        "CANON_CLOG3-CGAMUT_to_ACES2065-1",
        "RED_REDLOG3G10-RWG_to_ACES2065-1",
        "APPLE_LOG_to_ACES2065-1",
        "CURVE - CANON_CLOG2_to_LINEAR",
        "CURVE - CANON_CLOG3_to_LINEAR",
        "CURVE - APPLE_LOG_to_LINEAR",
        "CURVE - ST-2084_to_LINEAR",
        "CURVE - LINEAR_to_ST-2084",
        "UTILITY - ACES-AP0_to_CIE-XYZ-D65_BFD",
        "UTILITY - ACES-AP1_to_CIE-XYZ-D65_BFD",
        "UTILITY - ACES-AP1_to_LINEAR-REC709_BFD",
        "DISPLAY - CIE-XYZ-D65_to_sRGB",
        "DISPLAY - CIE-XYZ-D65_to_REC.1886-REC.709",
        "DISPLAY - CIE-XYZ-D65_to_G2.2-REC.709",
        "DISPLAY - CIE-XYZ-D65_to_G2.6-P3-D65",
        "DISPLAY - CIE-XYZ-D65_to_DisplayP3",
        "DISPLAY - CIE-XYZ-D65_to_ST2084-P3-D65",
        "DISPLAY - CIE-XYZ-D65_to_REC.2100-PQ",
        "DISPLAY - CIE-XYZ-D65_to_REC.2100-HLG-1000nit",
        "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-VIDEO_1.0",
        "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-VIDEO-P3lim_1.1",
        "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - HDR-VIDEO-1000nit-15nit-REC2020lim_1.1",
        "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - HDR-VIDEO-2000nit-15nit-REC2020lim_1.1",
        "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - HDR-VIDEO-4000nit-15nit-REC2020lim_1.1",
        "ACES-LMT - ACES 1.3 Reference Gamut Compression",
    ]
}

fn camera(curve: LogCurve, gamut: Gamut) -> Vec<Op> {
    vec![Op::Curve(curve, false), gamut_to_ap0(gamut)]
}

fn builtin(style: &str) -> Result<Vec<Op>, String> {
    let s = style.trim();
    let base = s.trim_end_matches(" - MIRROR NEGS");
    let ap1 = Primaries::ACES_AP1;
    let ap0 = Primaries::ACES_AP0;
    let ap1_to_ap0 = Op::Matrix(rgb_to_rgb(&ap1, &ap0), [0.0; 3]);
    let mirror = s.ends_with("MIRROR NEGS");
    let enc = |t: Transfer| Op::Transfer(t, true);
    let display = |p: Primaries, t: Op| vec![xyz_d65_to(&p), t];
    Ok(match base {
        "IDENTITY" | "pass_thru" => Vec::new(),
        "ACEScct_to_ACES2065-1" => vec![Op::Curve(LogCurve::AcesCct, false), ap1_to_ap0],
        "ACEScc_to_ACES2065-1" => vec![Op::Curve(LogCurve::AcesCc, false), ap1_to_ap0],
        "ACEScg_to_ACES2065-1" => vec![ap1_to_ap0],
        "ARRI_ALEXA-LOGC-EI800-AWG_to_ACES2065-1" => camera(LogCurve::LogC3, Gamut::ArriWideGamut3),
        "ARRI_LOGC4_to_ACES2065-1" => camera(LogCurve::LogC4, Gamut::ArriWideGamut4),
        "SONY_SLOG3-SGAMUT3_to_ACES2065-1" => camera(LogCurve::SLog3, Gamut::SGamut3),
        "SONY_SLOG3-SGAMUT3.CINE_to_ACES2065-1" => camera(LogCurve::SLog3, Gamut::SGamut3Cine),
        "PANASONIC_VLOG-VGAMUT_to_ACES2065-1" => camera(LogCurve::VLog, Gamut::VGamut),
        "CANON_CLOG2-CGAMUT_to_ACES2065-1" => camera(LogCurve::CanonLog2, Gamut::CinemaGamut),
        "CANON_CLOG3-CGAMUT_to_ACES2065-1" => camera(LogCurve::CanonLog3, Gamut::CinemaGamut),
        "RED_REDLOG3G10-RWG_to_ACES2065-1" | "RED_LOG3G10-RWG_to_ACES2065-1" => camera(LogCurve::Log3G10, Gamut::RedWideGamut),
        "APPLE_LOG_to_ACES2065-1" => camera(LogCurve::AppleLog, Gamut::Rec2020),
        "CURVE - CANON_CLOG2_to_LINEAR" => vec![Op::Curve(LogCurve::CanonLog2, false)],
        "CURVE - CANON_CLOG3_to_LINEAR" => vec![Op::Curve(LogCurve::CanonLog3, false)],
        "CURVE - APPLE_LOG_to_LINEAR" => vec![Op::Curve(LogCurve::AppleLog, false)],
        "CURVE - ST-2084_to_LINEAR" => vec![Op::Pq100(false)],
        "CURVE - LINEAR_to_ST-2084" => vec![Op::Pq100(true)],
        "UTILITY - ACES-AP0_to_CIE-XYZ-D65_BFD" => vec![aces_to_xyz_d65(&ap0)],
        "UTILITY - ACES-AP1_to_CIE-XYZ-D65_BFD" => vec![aces_to_xyz_d65(&ap1)],
        "UTILITY - ACES-AP1_to_LINEAR-REC709_BFD" => vec![Op::Matrix(rgb_to_rgb(&ap1, &Primaries::BT709), [0.0; 3])],
        "DISPLAY - CIE-XYZ-D65_to_sRGB" => display(Primaries::BT709, enc(Transfer::Srgb)),
        "DISPLAY - CIE-XYZ-D65_to_REC.1886-REC.709" => display(Primaries::BT709, enc(Transfer::Bt1886)),
        "DISPLAY - CIE-XYZ-D65_to_G2.2-REC.709" => display(Primaries::BT709, Op::Exponent([1.0 / 2.2; 3], if mirror { "mirror".into() } else { "clamp".into() })),
        "DISPLAY - CIE-XYZ-D65_to_G2.6-P3-D65" => display(Primaries::P3_D65, Op::Exponent([1.0 / 2.6; 3], if mirror { "mirror".into() } else { "clamp".into() })),
        "DISPLAY - CIE-XYZ-D65_to_DisplayP3" => display(Primaries::P3_D65, enc(Transfer::Srgb)),
        "DISPLAY - CIE-XYZ-D65_to_ST2084-P3-D65" => display(Primaries::P3_D65, Op::Pq100(true)),
        "DISPLAY - CIE-XYZ-D65_to_REC.2100-PQ" => display(Primaries::BT2020, Op::Pq100(true)),
        "DISPLAY - CIE-XYZ-D65_to_REC.2100-HLG-1000nit" => vec![xyz_d65_to(&Primaries::BT2020), Op::Hlg1000(false)],
        "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-VIDEO_1.0" => vec![Op::AcesOutput(DisplayTarget::Rec709, false)],
        "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-VIDEO-P3lim_1.1" | "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-VIDEO-P3_1.0" => vec![Op::AcesOutput(DisplayTarget::P3D65, false)],
        "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - HDR-VIDEO-1000nit-15nit-REC2020lim_1.1" | "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - HDR-VIDEO-1000nit-15nit-P3lim_1.1" => {
            vec![Op::AcesOutput(DisplayTarget::Rec2020Pq(1000), false)]
        }
        "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - HDR-VIDEO-2000nit-15nit-REC2020lim_1.1" | "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - HDR-VIDEO-2000nit-15nit-P3lim_1.1" => {
            vec![Op::AcesOutput(DisplayTarget::Rec2020Pq(2000), false)]
        }
        "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - HDR-VIDEO-4000nit-15nit-REC2020lim_1.1" | "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - HDR-VIDEO-4000nit-15nit-P3lim_1.1" => {
            vec![Op::AcesOutput(DisplayTarget::Rec2020Pq(4000), false)]
        }
        "ACES-LMT - ACES 1.3 Reference Gamut Compression" => vec![Op::GamutComp(GamutComp::ACES13, false)],
        other if other.contains("_2.0") && other.starts_with("ACES-OUTPUT") => {
            return Err(format!("BuiltinTransform \"{other}\" is an ACES 2.0 output transform; Zornov implements the ACES 1.3 ones (choose an ACES 1.x config or Zornov's own ACES view)"));
        }
        other => return Err(format!("BuiltinTransform \"{other}\" is not supported")),
    })
}

/// A compiled chain of operations.
#[derive(Clone, Debug, Default)]
pub struct Processor {
    ops: Vec<Op>,
}

impl Processor {
    /// Applies the chain to one RGB triple.
    pub fn apply(&self, c: [f32; 3]) -> [f32; 3] {
        self.apply64(c.map(|v| v as f64)).map(|v| v as f32)
    }

    pub fn apply64(&self, c: V3) -> V3 {
        self.ops.iter().fold(c, |v, op| op.apply(v))
    }

    pub fn is_identity(&self) -> bool {
        self.ops.is_empty()
    }

    /// Appends another processor.
    pub fn then(mut self, o: Processor) -> Processor {
        self.ops.extend(o.ops);
        self
    }

    /// AP0 linear → Studio working (BT.709 linear, D65).
    pub fn aces_to_working() -> Processor {
        Processor { ops: vec![Op::Matrix(rgb_to_rgb(&Primaries::ACES_AP0, &Primaries::BT709), [0.0; 3])] }
    }

    /// Bakes the chain over the 0..1 cube.
    pub fn bake(&self, size: usize) -> Lut3d {
        Lut3d::from_fn(size, |c| self.apply(c))
    }
}

impl Config {
    /// colour space → Studio working space. Uses the config's linear
    /// Rec.709 space when it has one, else its ACES interchange space plus
    /// Studio's own AP0 → BT.709 matrix.
    pub fn to_working(&self, src: &str) -> Result<Processor, String> {
        if let Some(w) = self.working_space() {
            return self.processor(src, &w.name);
        }
        if let Some(a) = self.role("aces_interchange").or_else(|| self.space("ACES2065-1")) {
            return Ok(self.processor(src, &a.name)?.then(Processor::aces_to_working()));
        }
        Err(format!("{} has no linear Rec.709 or ACES2065-1 colour space to reach Zornov's working space", self.label()))
    }

    /// Studio working space → `dst`.
    pub fn from_working(&self, dst: &str) -> Result<Processor, String> {
        if let Some(w) = self.working_space() {
            return self.processor(&w.name, dst);
        }
        if let Some(a) = self.role("aces_interchange").or_else(|| self.space("ACES2065-1")) {
            let m = Processor { ops: vec![Op::Matrix(rgb_to_rgb(&Primaries::BT709, &Primaries::ACES_AP0), [0.0; 3])] };
            return Ok(m.then(self.processor(&a.name, dst)?));
        }
        Err(format!("{} has no linear Rec.709 or ACES2065-1 colour space", self.label()))
    }

    /// Studio working space → display / view, as the display's code values.
    pub fn working_view(&self, display: &str, view: &str) -> Result<Processor, String> {
        if let Some(w) = self.working_space() {
            return self.view_processor(&w.name, display, view);
        }
        let a = self.role("aces_interchange").or_else(|| self.space("ACES2065-1")).ok_or("no working space in the config")?;
        let m = Processor { ops: vec![Op::Matrix(rgb_to_rgb(&Primaries::BT709, &Primaries::ACES_AP0), [0.0; 3])] };
        Ok(m.then(self.view_processor(&a.name, display, view)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"
ocio_profile_version: 2
name: test
roles:
  scene_linear: ACEScg
  aces_interchange: ACES2065-1
  color_timing: ACEScct
default_view_transform: ACES 1.0 - SDR Video
displays:
  sRGB - Display:
    - !<View> {name: ACES 1.0 - SDR Video, view_transform: ACES 1.0 - SDR Video, display_colorspace: sRGB - Display}
    - !<View> {name: Raw, colorspace: Raw}
view_transforms:
  - !<ViewTransform>
    name: ACES 1.0 - SDR Video
    from_scene_reference: !<BuiltinTransform> {style: ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-VIDEO_1.0}
display_colorspaces:
  - !<ColorSpace>
    name: sRGB - Display
    from_display_reference: !<BuiltinTransform> {style: DISPLAY - CIE-XYZ-D65_to_sRGB}
colorspaces:
  - !<ColorSpace>
    name: ACES2065-1
  - !<ColorSpace>
    name: ACEScg
    to_scene_reference: !<BuiltinTransform> {style: ACEScg_to_ACES2065-1}
  - !<ColorSpace>
    name: ACEScct
    to_scene_reference: !<BuiltinTransform> {style: ACEScct_to_ACES2065-1}
  - !<ColorSpace>
    name: Raw
    isdata: true
  - !<ColorSpace>
    name: Linear Rec.709 (sRGB)
    aliases: [lin_rec709]
    from_scene_reference: !<MatrixTransform> {matrix: [2.52168618674388, -1.13413098823972, -0.387555198504164, 0, -0.276479914229922, 1.37271908766826, -0.096239173438334, 0, -0.0153780649660342, -0.152975335867399, 1.16835340083343, 0, 0, 0, 0, 1]}
  - !<ColorSpace>
    name: ARRI LogC3 (EI800)
    to_scene_reference: !<GroupTransform>
      children:
        - !<LogCameraTransform> {base: 10, log_side_slope: 0.247189638318671, log_side_offset: 0.385536998692443, lin_side_slope: 5.55555555555556, lin_side_offset: 0.0522722750251688, lin_side_break: 0.0105909904954696, direction: inverse}
        - !<MatrixTransform> {matrix: [0.680205505106279, 0.236136601606481, 0.0836578932872398, 0, 0.0854149797421404, 1.01747087860704, -0.102885858349182, 0, 0.00205652166929683, -0.0625625003847921, 1.06050597871549, 0, 0, 0, 0, 1]}
  - !<ColorSpace>
    name: sRGB Encoded Rec.709 (sRGB)
    from_scene_reference: !<GroupTransform>
      children:
        - !<MatrixTransform> {matrix: [2.52168618674388, -1.13413098823972, -0.387555198504164, 0, -0.276479914229922, 1.37271908766826, -0.096239173438334, 0, -0.0153780649660342, -0.152975335867399, 1.16835340083343, 0, 0, 0, 0, 1]}
        - !<ExponentWithLinearTransform> {gamma: 2.4, offset: 0.055, direction: inverse}
  - !<ColorSpace>
    name: Gamma 2.2 AP1
    to_scene_reference: !<GroupTransform>
      children:
        - !<ExponentTransform> {value: 2.2, style: mirror}
        - !<ColorSpaceTransform> {src: ACEScg, dst: ACES2065-1}
"#;

    fn cfg() -> Config {
        Config::parse(CONFIG, Path::new(".")).unwrap()
    }

    fn close(a: [f32; 3], b: [f32; 3], eps: f32) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() <= eps * b[i].abs().max(1.0))
    }

    #[test]
    fn structure() {
        let c = cfg();
        assert_eq!(c.spaces.len(), 9);
        assert_eq!(c.role("color_timing").unwrap().name, "ACEScct");
        assert_eq!(c.working_space().unwrap().name, "Linear Rec.709 (sRGB)");
        assert_eq!(c.displays[0].views.len(), 2);
        assert!(c.space("LIN_REC709").is_some());
    }

    #[test]
    fn logc_matches_builtin_curve() {
        let c = cfg();
        let p = c.to_working("ARRI LogC3 (EI800)").unwrap();
        let code = LogCurve::LogC3.encode(0.18);
        let out = p.apply([code; 3]);
        // A neutral stays neutral and decodes to 18 %.
        assert!(close(out, [0.18; 3], 2e-3), "{out:?}");
    }

    #[test]
    fn srgb_encoding_round_trip() {
        let c = cfg();
        let to = c.from_working("sRGB Encoded Rec.709 (sRGB)").unwrap();
        let back = c.to_working("sRGB Encoded Rec.709 (sRGB)").unwrap();
        for v in [[0.0f32, 0.0, 0.0], [0.18, 0.18, 0.18], [0.5, 0.2, 0.9], [1.0, 1.0, 1.0]] {
            let e = to.apply(v);
            assert!(close(back.apply(e), v, 1e-4));
            let want = Transfer::Srgb.encode_rgb(v);
            assert!(close(e, want, 1e-4), "{e:?} vs {want:?}");
        }
        // ColorSpaceTransform and Exponent inside a group.
        let g = c.to_working("Gamma 2.2 AP1").unwrap();
        let w = g.apply([0.5; 3]);
        assert!(close(w, [0.5f32.powf(2.2); 3], 2e-3), "{w:?}");
    }

    #[test]
    fn display_view_matches_studio_aces() {
        let c = cfg();
        let p = c.working_view("sRGB - Display", "ACES 1.0 - SDR Video").unwrap();
        let out = p.apply([0.18; 3]);
        let studio = DisplayTarget::Rec709.view([0.18; 3]);
        let want = Transfer::Srgb.encode_rgb(studio);
        assert!(close(out, want, 2e-3), "{out:?} vs {want:?}");
        let raw = c.working_view("sRGB - Display", "Raw").unwrap();
        assert_eq!(raw.apply([0.3, 0.4, 0.5]), [0.3, 0.4, 0.5]);
    }

    #[test]
    fn unsupported_is_an_error() {
        let bad = CONFIG.replace("SDR-VIDEO_1.0", "SDR-100nit-REC709_2.0");
        let c = Config::parse(&bad, Path::new(".")).unwrap();
        let e = c.working_view("sRGB - Display", "ACES 1.0 - SDR Video").unwrap_err();
        assert!(e.contains("ACES 2.0"), "{e}");
    }

    #[test]
    fn log_camera_inverse() {
        let p = LogCam { base: 10.0, log_slope: [0.25; 3], log_offset: [0.4; 3], lin_slope: [5.0; 3], lin_offset: [0.05; 3], lin_break: [0.01; 3], linear_slope: None };
        for x in [-0.01, 0.0, 0.005, 0.01, 0.18, 4.0] {
            let y = p.fwd(x, 0);
            assert!((p.inv(y, 0) - x).abs() < 1e-9, "{x}");
        }
        // Continuous at the break.
        assert!((p.fwd(0.01 - 1e-9, 0) - p.fwd(0.01 + 1e-9, 0)).abs() < 1e-6);
    }

    #[test]
    fn gamut_compression() {
        let g = GamutComp::ACES13;
        let out = g.apply([1.0, -0.2, 0.1], false);
        assert!(out[1] > -0.2, "pulled in {out:?}");
        let back = g.apply(out, true);
        assert!((back[1] + 0.2).abs() < 1e-6);
        let inside = g.apply([0.5, 0.4, 0.3], false);
        assert_eq!(inside, [0.5, 0.4, 0.3]);
    }

    #[test]
    fn spi_luts() {
        let l = parse_spi1d("Version 1\nFrom 0.0 1.0\nLength 3\nComponents 1\n{\n0\n0.25\n1\n}\n").unwrap();
        assert!((l.sample([0.5; 3])[0] - 0.25).abs() < 1e-6);
        assert!((lut1_inverse(&l, 0.25, 0) - 0.5).abs() < 1e-9);
        let l3 = parse_spi3d("SPILUT 1.0\n3 3\n2 2 2\n0 0 0 0 0 0\n1 0 0 1 0 0\n0 1 0 0 1 0\n1 1 0 1 1 0\n0 0 1 0 0 1\n1 0 1 1 0 1\n0 1 1 0 1 1\n1 1 1 1 1 1\n").unwrap();
        assert_eq!(l3.at(1, 0, 1), [1.0, 0.0, 1.0]);
    }

    /// Reads a real config named by `STUDIO_OCIO_CONFIG` (for example the
    /// ACES studio config) and checks every input colour space resolves.
    #[test]
    #[ignore]
    fn real_config() {
        let Ok(path) = std::env::var("STUDIO_OCIO_CONFIG") else { return };
        let c = Config::load(Path::new(&path)).unwrap();
        let mut ok = 0;
        for s in &c.spaces {
            match c.to_working(&s.name) {
                Ok(p) => {
                    ok += 1;
                    if s.name.contains("LogC3") || s.name.contains("S-Log3") || s.name.contains("V-Log") || s.name.contains("Apple Log") {
                        let curve = LogCurve::from_name(&s.name.split_whitespace().next().unwrap().replace("S-Log3", "slog3")).or_else(|| LogCurve::from_name(&s.name.replace(" ", "")));
                        if let Some(curve) = curve {
                            let out = p.apply([curve.encode(0.18); 3]);
                            println!("{}: grey -> {out:?}", s.name);
                            assert!((out[1] - 0.18).abs() < 0.005, "{}: {out:?}", s.name);
                        }
                    }
                }
                Err(e) => println!("{}: {e}", s.name),
            }
        }
        // Studio's own camera spaces agree with the config's.
        let table = [
            ("ARRI LogC3 (EI800)", "logc3_awg3"),
            ("ARRI LogC4", "logc4_awg4"),
            ("BMDFilm WideGamut Gen5", "bmdfilm_gen5_bmdwg"),
            ("DaVinci Intermediate WideGamut", "davinci_intermediate_dwg"),
            ("V-Log V-Gamut", "vlog_vgamut"),
            ("Log3G10 REDWideGamutRGB", "log3g10_rwg"),
            ("S-Log3 S-Gamut3.Cine", "slog3_sgamut3cine"),
            ("Apple Log", "applelog_rec2020"),
            ("ACEScct", "acescct_ap1"),
        ];
        for (name, ours) in table {
            let p = c.to_working(name).unwrap();
            let cam = crate::camera::CameraSpace::parse(ours).unwrap();
            let m = rgb_to_rgb(&cam.gamut.primaries(), &Primaries::BT709);
            for code in [[0.4f32, 0.4, 0.4], [0.5, 0.35, 0.3], [0.2, 0.3, 0.45], [0.7, 0.65, 0.6]] {
                let a = p.apply(code);
                let b = m.apply(code.map(|v| cam.curve.decode(v)));
                let err = (0..3).map(|i| (a[i] - b[i]).abs() / b[i].abs().max(0.05)).fold(0f32, f32::max);
                println!("{name} {code:?}: config {a:?} studio {b:?} rel err {err:.4}");
                // Neutrals agree exactly; saturated colours differ by the
                // config's CAT02 round trip through the ACES white.
                assert!(err < 0.03, "{name}");
            }
        }
        println!("{ok} of {} colour spaces resolve", c.spaces.len());
        assert!(ok * 2 > c.spaces.len());
    }
}
