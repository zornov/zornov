// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! View (display) transforms: the last colour step of a frame, turning
//! scene-linear working values into what a display should show.
//!
//! * [`ViewTransform::None`]: working values go to the output as they are
//!   (display-referred editing; values above 1 clip in SDR).
//! * [`ViewTransform::Aces`]: the ACES 1.3 output transform for the
//!   output's display (Rec.709/sRGB, P3, Rec.2020 PQ at a chosen peak, HLG).
//! * [`ViewTransform::Ocio`]: a display / view of an OpenColorIO config.
//!   Its result (the display's code values) is turned back into the linear
//!   values the output encoder expects for the current output space, so the
//!   file or screen ends up with exactly OCIO's code values; pick an OCIO
//!   display that matches the output space.
//!
//! Each is baked into a 65³ LUT with an ACEScct shaper (cached per output
//! space) and applied with the grade shader's LUT stage, on the GPU or CPU.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use studio_colour::aces::{DisplayTarget, VIEW_LUT_SIZE, bake_view};
use studio_colour::lut::Lut3d;
use studio_colour::math::Mat3;
use studio_colour::primaries::rgb_to_rgb;
use studio_colour::transfer::{acescct_to_lin, lin_to_acescct};
use studio_colour::{CubeFile, LutSettings, LutSpace, Primaries, Transfer};

use crate::output::OutputSpace;

/// A frame's view transform.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum ViewTransform {
    #[default]
    None,
    /// ACES 1.3 output transform; `hdr_peak` is the PQ peak in cd/m².
    Aces { hdr_peak: u16 },
    /// An OCIO display / view registered with [`register_ocio_view`].
    Ocio(u32),
}

impl ViewTransform {
    pub fn is_none(self) -> bool {
        self == ViewTransform::None
    }

    /// Parses a project setting: `none`, `aces`, `aces:2000` (PQ peak),
    /// or `ocio:<display>/<view>` with the project's OCIO config.
    pub fn parse(spec: &str, config: Option<&std::path::Path>, hdr_peak: Option<u32>) -> Result<ViewTransform, String> {
        let s = spec.trim();
        let peak = hdr_peak.unwrap_or(1000).clamp(400, 10000) as u16;
        match s.to_ascii_lowercase().as_str() {
            "" | "none" | "off" | "raw" | "standard" => return Ok(ViewTransform::None),
            "aces" | "aces1.3" | "aces13" | "aces 1.3" => return Ok(ViewTransform::Aces { hdr_peak: peak }),
            _ => {}
        }
        if let Some(n) = s.strip_prefix("aces:") {
            let p: u16 = n.trim().parse().map_err(|_| format!("\"{s}\": the ACES HDR peak is a number of cd/m² (1000, 2000, 4000)"))?;
            return Ok(ViewTransform::Aces { hdr_peak: p.clamp(400, 10000) });
        }
        if let Some(dv) = s.strip_prefix("ocio:") {
            let cfg = config.ok_or("an ocio: view needs an OCIO config on the project")?;
            let (display, view) = dv.split_once('/').ok_or_else(|| format!("\"{s}\": write ocio:<display>/<view>"))?;
            // Checked once per config / display / view (scenes ask for
            // their canvas every frame).
            type Known = Mutex<HashMap<(PathBuf, String), Result<u32, String>>>;
            static KNOWN: OnceLock<Known> = OnceLock::new();
            let key = (cfg.to_path_buf(), dv.to_string());
            if let Some(r) = KNOWN.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
                return r.clone().map(ViewTransform::Ocio);
            }
            let r = register_ocio_view(cfg, display.trim(), view.trim());
            KNOWN.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner()).insert(key, r.clone());
            return r.map(ViewTransform::Ocio);
        }
        Err(format!("unknown view transform \"{s}\"; use none, aces, aces:<peak nits> or ocio:<display>/<view>"))
    }
}

/// The ACES display target for an output space.
pub fn display_target(space: OutputSpace, hdr_peak: u16) -> DisplayTarget {
    match space {
        OutputSpace::SrgbDisplay | OutputSpace::Rec709 => DisplayTarget::Rec709,
        OutputSpace::DisplayP3 => DisplayTarget::P3D65,
        OutputSpace::Rec2020Pq => DisplayTarget::Rec2020Pq(hdr_peak),
        OutputSpace::Rec2020Hlg => DisplayTarget::Rec2020Hlg,
        // Scene-referred output gets no view; callers check first.
        OutputSpace::AcesCct => DisplayTarget::Rec709,
    }
}

struct OcioView {
    config: PathBuf,
    display: String,
    view: String,
}

fn ocio_views() -> &'static Mutex<Vec<OcioView>> {
    static V: OnceLock<Mutex<Vec<OcioView>>> = OnceLock::new();
    V.get_or_init(|| Mutex::new(Vec::new()))
}

/// Checks and registers an OCIO display / view; the id is stable for the
/// same triple.
pub fn register_ocio_view(config: &std::path::Path, display: &str, view: &str) -> Result<u32, String> {
    let cfg = crate::colour_space::ocio_config(config)?;
    cfg.working_view(display, view)?;
    let mut g = ocio_views().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = g.iter().position(|v| v.config == config && v.display == display && v.view == view) {
        return Ok(i as u32);
    }
    g.push(OcioView { config: config.to_path_buf(), display: display.to_string(), view: view.to_string() });
    Ok((g.len() - 1) as u32)
}

/// Output code values → the working-linear values the output encoder
/// turns back into those codes (its exact inverse).
pub fn decode_output(space: OutputSpace, code: [f32; 3]) -> [f32; 3] {
    let lin = match space {
        OutputSpace::SrgbDisplay | OutputSpace::DisplayP3 => Transfer::Srgb.decode_rgb(code),
        OutputSpace::Rec709 => Transfer::Bt1886.decode_rgb(code),
        OutputSpace::Rec2020Pq => Transfer::Pq.decode_rgb(code),
        OutputSpace::Rec2020Hlg => code.map(|v| {
            let scene = Transfer::Hlg.decode(v);
            let d = if scene < 0.0 { -(-scene).powf(1.2) } else { scene.powf(1.2) };
            d * 1000.0 / 203.0
        }),
        OutputSpace::AcesCct => Transfer::AcesCct.decode_rgb(code),
    };
    let m: Mat3 = space.from_working().inverse().unwrap_or(Mat3::IDENTITY);
    m.apply(lin)
}

/// The baked LUT of a view for an output space (cached).
pub fn view_lut(view: ViewTransform, space: OutputSpace) -> Result<Option<Arc<CubeFile>>, String> {
    type Cache = Mutex<HashMap<(ViewTransform, OutputSpace), Arc<CubeFile>>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    if view.is_none() || space.is_scene_referred() {
        return Ok(None);
    }
    let cache = CACHE.get_or_init(Default::default);
    if let Some(c) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&(view, space)) {
        return Ok(Some(c.clone()));
    }
    let lut = match view {
        ViewTransform::None => return Ok(None),
        ViewTransform::Aces { hdr_peak } => bake_view(display_target(space, hdr_peak), VIEW_LUT_SIZE),
        ViewTransform::Ocio(id) => {
            let (cfg, display, view_name) = {
                let g = ocio_views().lock().unwrap_or_else(|e| e.into_inner());
                let v = g.get(id as usize).ok_or("unknown OCIO view")?;
                (v.config.clone(), v.display.clone(), v.view.clone())
            };
            let config = crate::colour_space::ocio_config(&cfg)?;
            let p = config.working_view(&display, &view_name)?;
            let ap1 = Primaries::ACES_AP1;
            let to_working = rgb_to_rgb(&ap1, &Primaries::BT709);
            let to_ap1 = rgb_to_rgb(&Primaries::BT709, &ap1);
            Lut3d::from_fn(VIEW_LUT_SIZE, |cct| {
                let lin = to_working.apply(cct.map(acescct_to_lin));
                let out = decode_output(space, p.apply(lin));
                to_ap1.apply(out).map(lin_to_acescct)
            })
        }
    };
    let cube = Arc::new(CubeFile { title: Some(format!("{view:?}")), lut1d: None, lut3d: Some(lut) });
    cache.lock().unwrap_or_else(|e| e.into_inner()).insert((view, space), cube.clone());
    Ok(Some(cube))
}

/// The view as a grade (LUT stage only), or `None` for no view.
pub fn view_grade(view: ViewTransform, space: OutputSpace) -> Option<studio_colour::Grade> {
    let cube = view_lut(view, space).ok().flatten()?;
    let mut g = studio_colour::Grade::default();
    g.lut = Some(LutSettings { lut: cube, space: LutSpace::AcesCct, intensity: 1.0, skin_protect: 0.0 });
    Some(g)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_output_inverts_the_encoder() {
        for space in [OutputSpace::SrgbDisplay, OutputSpace::Rec709, OutputSpace::DisplayP3, OutputSpace::Rec2020Pq, OutputSpace::Rec2020Hlg, OutputSpace::AcesCct] {
            for lin in [[0.18f32, 0.18, 0.18], [0.5, 0.3, 0.2], [0.9, 0.9, 0.9]] {
                let m = space.from_working();
                let code = m.apply(lin).map(|v| space.encode(v));
                let back = decode_output(space, code);
                for k in 0..3 {
                    assert!((back[k] - lin[k]).abs() < 2e-3, "{space:?} {lin:?} -> {back:?}");
                }
            }
        }
    }

    #[test]
    fn aces_view_grade() {
        let g = view_grade(ViewTransform::Aces { hdr_peak: 1000 }, OutputSpace::Rec709).unwrap();
        let p = g.prepare();
        let out = p.apply_rgb([0.18, 0.18, 0.18]);
        let want = DisplayTarget::Rec709.view([0.18; 3]);
        assert!((out[1] - want[1]).abs() < 2e-3, "{out:?} vs {want:?}");
        assert!(view_grade(ViewTransform::None, OutputSpace::Rec709).is_none());
    }

    #[test]
    fn parse_names() {
        assert_eq!(ViewTransform::parse("ACES", None, None).unwrap(), ViewTransform::Aces { hdr_peak: 1000 });
        assert_eq!(ViewTransform::parse("aces:2000", None, None).unwrap(), ViewTransform::Aces { hdr_peak: 2000 });
        assert_eq!(ViewTransform::parse("none", None, None).unwrap(), ViewTransform::None);
        assert!(ViewTransform::parse("ocio:sRGB/ACES", None, None).is_err());
        assert!(ViewTransform::parse("filmic", None, None).is_err());
    }
}
