// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Choosing a look from a reference picture. The AI makes (or is given)
//! a picture of how the world should feel; this reads its sky, ground and
//! foliage colours, its light and haze, and the builder dresses real
//! geometry to match. Nothing from the picture is placed in the world.

use crate::math::V3;
use studio_mesh::Mesh;

/// What a reference picture says about the look.
#[derive(Clone, Debug, PartialEq)]
pub struct LookHints {
    /// Linear RGB of the sky near the top of the picture.
    pub sky: V3,
    /// Linear RGB near the horizon (fog colour).
    pub horizon: V3,
    pub ground: V3,
    /// Linear RGB of green-ish (or leafy) areas; None when there are none.
    pub foliage: Option<V3>,
    /// Hour of the day it suggests.
    pub time: f32,
    /// 0 crisp .. 1 hazy.
    pub haze: f32,
    /// How bright the picture is, 0..1.
    pub brightness: f32,
    /// 0 clear .. 1 overcast.
    pub clouds: f32,
}

fn lin(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn mean(rgba: &[u8], w: usize, x0: usize, x1: usize, y0: usize, y1: usize, pick: impl Fn([f32; 3]) -> bool) -> Option<V3> {
    let (mut sum, mut n) = ([0.0f32; 3], 0.0f32);
    for y in y0..y1 {
        for x in x0..x1 {
            let i = (y * w + x) * 4;
            let c = [lin(rgba[i] as f32 / 255.0), lin(rgba[i + 1] as f32 / 255.0), lin(rgba[i + 2] as f32 / 255.0)];
            if pick(c) {
                for k in 0..3 {
                    sum[k] += c[k];
                }
                n += 1.0;
            }
        }
    }
    (n > 0.0).then(|| [sum[0] / n, sum[1] / n, sum[2] / n])
}

fn luma(c: V3) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// Reads the look of an 8-bit sRGB RGBA picture.
pub fn hints_from_rgba(rgba: &[u8], width: usize, height: usize) -> Option<LookHints> {
    if width < 8 || height < 8 || rgba.len() < width * height * 4 {
        return None;
    }
    let all = |_c: [f32; 3]| true;
    let top = mean(rgba, width, 0, width, 0, height / 4, all)?;
    let band = mean(rgba, width, 0, width, height * 3 / 8, height / 2, all)?;
    let low = mean(rgba, width, 0, width, height * 3 / 5, height, all)?;
    let foliage = mean(rgba, width, 0, width, height / 3, height, |c| c[1] > c[0] * 1.12 && c[1] > c[2] * 1.12 && luma(c) > 0.004);
    let whole = mean(rgba, width, 0, width, 0, height, all)?;
    let brightness = luma(whole).sqrt().clamp(0.0, 1.0);
    let sky_l = luma(top).sqrt();
    // Warmth of the horizon: red over blue.
    let warm = (band[0] / band[2].max(1e-4)).clamp(0.2, 5.0);
    let time = if sky_l < 0.12 {
        23.0
    } else if warm > 1.5 && sky_l < 0.7 {
        18.9
    } else if warm > 1.15 {
        17.0
    } else if sky_l < 0.3 {
        6.0
    } else {
        12.5
    };
    // Haze: little contrast between the top and the horizon band.
    let contrast = (luma(top) - luma(band)).abs() / (luma(top) + luma(band) + 1e-4);
    let haze = (1.0 - contrast * 3.0).clamp(0.0, 1.0);
    let grey = {
        let m = (top[0] + top[1] + top[2]) / 3.0;
        1.0 - ((top[0] - m).abs() + (top[1] - m).abs() + (top[2] - m).abs()) / (3.0 * m.max(1e-4)) * 2.0
    };
    Some(LookHints { sky: top, horizon: band, ground: low, foliage, time, haze, brightness, clouds: grey.clamp(0.0, 1.0) })
}

/// Moves the colours of materials whose names contain any of `names`
/// toward `target` (keeping their brightness), `amount` 0..1.
pub fn recolour(mesh: &mut Mesh, names: &[&str], target: V3, amount: f32) {
    for m in &mut mesh.materials {
        let n = m.name.to_lowercase();
        if names.iter().any(|k| n.contains(k)) {
            let l = luma([m.base_colour[0], m.base_colour[1], m.base_colour[2]]).max(1e-4);
            let lt = luma(target).max(1e-4);
            for k in 0..3 {
                let t = target[k] * l / lt;
                m.base_colour[k] += (t - m.base_colour[k]) * amount;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(w: usize, h: usize, sky: [u8; 3], ground: [u8; 3], tree: Option<[u8; 3]>) -> Vec<u8> {
        let mut px = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let c = if y < h / 2 { sky } else if tree.is_some() && x % 8 < 4 && y > h * 2 / 3 { tree.unwrap() } else { ground };
                px.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        px
    }

    #[test]
    fn a_blue_noon_a_golden_dusk_and_a_dark_night_read_differently() {
        let day = hints_from_rgba(&picture(64, 64, [90, 150, 230], [110, 120, 70], Some([40, 110, 40])), 64, 64).unwrap();
        let dusk = hints_from_rgba(&picture(64, 64, [230, 130, 70], [60, 40, 30], None), 64, 64).unwrap();
        let night = hints_from_rgba(&picture(64, 64, [8, 10, 24], [10, 10, 12], None), 64, 64).unwrap();
        assert!(day.time > 10.0 && day.time < 15.0, "{}", day.time);
        assert!(dusk.time > 16.0 && dusk.time < 20.0, "{}", dusk.time);
        assert!(night.time > 21.0 || night.time < 4.0, "{}", night.time);
        assert!(day.brightness > night.brightness);
        let f = day.foliage.unwrap();
        assert!(f[1] > f[0] && f[1] > f[2]);
        assert!(dusk.foliage.is_none());
        assert!(day.sky[2] > day.sky[0], "blue sky");
        assert!(dusk.sky[0] > dusk.sky[2], "warm sky");
        assert!(hints_from_rgba(&[0; 16], 2, 2).is_none());
    }

    #[test]
    fn recolouring_keeps_brightness_and_leaves_other_materials() {
        let mut m = Mesh::default();
        m.materials = vec![
            studio_mesh::Material { name: "Leaves".into(), base_colour: [0.1, 0.3, 0.05, 1.0], ..Default::default() },
            studio_mesh::Material { name: "Bark".into(), base_colour: [0.2, 0.1, 0.05, 1.0], ..Default::default() },
        ];
        let before = luma([0.1, 0.3, 0.05]);
        recolour(&mut m, &["leaf", "leaves"], [0.3, 0.2, 0.02], 1.0);
        let c = m.materials[0].base_colour;
        assert!(c[0] > c[1] * 1.2, "now autumn: {c:?}");
        assert!((luma([c[0], c[1], c[2]]) - before).abs() < 0.01);
        assert_eq!(m.materials[1].base_colour[0], 0.2);
    }
}
