// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Relighting on the machine: a key light from a direction and colour
//! laid over a picture, using the surface the depth map describes (its
//! slopes give each pixel a facing, so a face lit from the left brightens
//! on its left cheek and falls off on the right). Without a depth map the
//! picture's own soft brightness stands in for the surface.
//!
//! Also the backdrop changes that are a matter of light and colour: time
//! of day ([`TimeOfDay`]) and season ([`Season`]), each a key light plus a
//! grade. These change the light, not what is in the picture; Codex (from
//! the app) repaints (snow on the ground, lit windows at night).

use crate::frame::{box_mean, Frame, Mask};

/// A light to add.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Light {
    /// Where it comes from, degrees round the frame: 0 = from the right,
    /// 90 = from above, 180 = from the left, 270 = from below.
    pub azimuth: f32,
    /// Degrees above the picture plane: 0 = grazing, 90 = from the camera.
    pub elevation: f32,
    /// Linear RGB colour (1, 1, 1 = white).
    pub colour: [f32; 3],
    /// 0..2: how strongly it shapes the picture.
    pub strength: f32,
    /// 0..1: how much of the old light stays where the new one does not
    /// reach.
    pub ambient: f32,
}

impl Default for Light {
    fn default() -> Self {
        Light { azimuth: 135.0, elevation: 35.0, colour: [1.0, 0.95, 0.88], strength: 0.8, ambient: 0.55 }
    }
}

impl Light {
    /// A direction by name: left, right, top, bottom, top-left, front...
    pub fn from_direction(name: &str) -> Option<(f32, f32)> {
        let n = name.trim().to_ascii_lowercase().replace(['_', ' '], "-");
        Some(match n.as_str() {
            "right" => (0.0, 25.0),
            "top-right" | "upper-right" => (45.0, 35.0),
            "top" | "above" | "overhead" => (90.0, 40.0),
            "top-left" | "upper-left" => (135.0, 35.0),
            "left" => (180.0, 25.0),
            "bottom-left" => (225.0, 25.0),
            "bottom" | "below" | "under" => (270.0, 25.0),
            "bottom-right" => (315.0, 25.0),
            "front" | "camera" => (90.0, 80.0),
            "back" | "behind" | "rim" => (90.0, 5.0),
            _ => return None,
        })
    }
}

/// A colour by name or `#rrggbb`, as linear-ish RGB multipliers.
pub fn colour_by_name(name: &str) -> Option<[f32; 3]> {
    let n = name.trim().to_ascii_lowercase();
    if let Some(hex) = n.strip_prefix('#').filter(|h| h.len() == 6) {
        let v = u32::from_str_radix(hex, 16).ok()?;
        return Some([((v >> 16) & 255) as f32 / 255.0, ((v >> 8) & 255) as f32 / 255.0, (v & 255) as f32 / 255.0]);
    }
    Some(match n.as_str() {
        "white" | "daylight" | "neutral" => [1.0, 1.0, 1.0],
        "warm" | "tungsten" | "candle" => [1.0, 0.72, 0.45],
        "golden" | "gold" | "sunset" => [1.0, 0.68, 0.35],
        "orange" => [1.0, 0.55, 0.2],
        "red" => [1.0, 0.25, 0.2],
        "pink" | "magenta" => [1.0, 0.4, 0.75],
        "purple" | "violet" => [0.65, 0.4, 1.0],
        "blue" | "moonlight" | "moon" => [0.55, 0.7, 1.0],
        "cool" | "overcast" => [0.82, 0.9, 1.0],
        "cyan" | "teal" => [0.4, 0.9, 1.0],
        "green" => [0.45, 1.0, 0.5],
        "neon" => [0.9, 0.3, 1.0],
        _ => return None,
    })
}

/// Surface facing per pixel from a depth map (1 = near): unit normals.
fn normals(depth: &Mask, relief: f32) -> Vec<[f32; 3]> {
    let (w, h) = (depth.width, depth.height);
    // A little smoothing so the facing follows forms, not texture.
    let d = box_mean(&depth.data, w, h, 2);
    let at = |x: isize, y: isize| d[(y.clamp(0, h as isize - 1) as usize) * w + x.clamp(0, w as isize - 1) as usize];
    let k = relief * w.max(h) as f32 / 8.0;
    let mut n = Vec::with_capacity(w * h);
    for y in 0..h as isize {
        for x in 0..w as isize {
            // Nearer is towards the camera: the surface rises with depth.
            let dx = (at(x + 1, y) - at(x - 1, y)) * 0.5 * k;
            let dy = (at(x, y + 1) - at(x, y - 1)) * 0.5 * k;
            let (nx, ny, nz) = (-dx, -dy, 1.0);
            let l = (nx * nx + ny * ny + nz * nz).sqrt();
            n.push([nx / l, ny / l, nz / l]);
        }
    }
    n
}

/// `f` relit by `light`. `depth` (same size, 1 = near) gives the shapes;
/// without it the picture's blurred brightness does.
pub fn relight(f: &Frame, depth: Option<&Mask>, light: &Light) -> Frame {
    let (w, h) = (f.width, f.height);
    let surface = match depth {
        Some(d) if (d.width, d.height) == (w, h) => d.clone(),
        Some(d) => d.resized(w, h),
        None => Mask::new(w, h, box_mean(&f.luma(), w, h, (w.max(h) / 40).max(2))),
    };
    let ns = normals(&surface, if depth.is_some() { 1.0 } else { 0.5 });
    let (az, el) = (light.azimuth.to_radians(), light.elevation.to_radians());
    // Screen y grows downwards; "from above" is negative y.
    let l = [az.cos() * el.cos(), -az.sin() * el.cos(), el.sin()];
    let mut out = f.clone();
    let s = light.strength.clamp(0.0, 2.0);
    let amb = light.ambient.clamp(0.0, 1.0);
    // Nearer things catch more of a key light placed near the subject.
    for (i, p) in out.rgba.chunks_exact_mut(4).enumerate() {
        let n = ns[i];
        let lambert = (n[0] * l[0] + n[1] * l[1] + n[2] * l[2]).max(0.0);
        let near = if depth.is_some() { 0.6 + 0.4 * surface.data[i] } else { 1.0 };
        for c in 0..3 {
            let v = p[c] as f32 / 255.0;
            let lit = v * (amb + s * lambert * near * light.colour[c] * 1.2);
            // Soft shoulder: highlights roll off instead of clipping.
            let lit = lit / (1.0 + (lit - 1.0).max(0.0));
            p[c] = (lit.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
    }
    out
}

/// A grade: lift (shadows), gain (highlights), saturation and a tint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grade {
    pub gain: [f32; 3],
    pub lift: [f32; 3],
    pub saturation: f32,
    pub exposure: f32,
}

impl Default for Grade {
    fn default() -> Self {
        Grade { gain: [1.0; 3], lift: [0.0; 3], saturation: 1.0, exposure: 1.0 }
    }
}

/// `f` with `g` applied.
pub fn grade(f: &Frame, g: &Grade) -> Frame {
    let mut out = f.clone();
    for p in out.rgba.chunks_exact_mut(4) {
        let mut v: [f32; 3] = std::array::from_fn(|c| p[c] as f32 / 255.0 * g.exposure);
        let y = 0.2126 * v[0] + 0.7152 * v[1] + 0.0722 * v[2];
        for c in 0..3 {
            v[c] = y + (v[c] - y) * g.saturation;
            v[c] = g.lift[c] + v[c] * (g.gain[c] - g.lift[c]);
            p[c] = (v[c].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
    }
    out
}

/// Times of day a backdrop can be moved to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeOfDay {
    Dawn,
    Morning,
    Noon,
    GoldenHour,
    Sunset,
    BlueHour,
    Night,
}

impl TimeOfDay {
    pub fn parse(s: &str) -> Option<TimeOfDay> {
        Some(match s.trim().to_ascii_lowercase().replace(['_', '-'], " ").as_str() {
            "dawn" | "sunrise" | "first light" => TimeOfDay::Dawn,
            "morning" => TimeOfDay::Morning,
            "noon" | "midday" | "day" | "daytime" | "afternoon" => TimeOfDay::Noon,
            "golden hour" | "golden" | "late afternoon" => TimeOfDay::GoldenHour,
            "sunset" | "dusk" | "evening" => TimeOfDay::Sunset,
            "blue hour" | "twilight" => TimeOfDay::BlueHour,
            "night" | "midnight" | "moonlight" | "nighttime" => TimeOfDay::Night,
            _ => return None,
        })
    }

    pub const NAMES: &'static str = "dawn, morning, noon, golden hour, sunset, blue hour, night";

    fn look(self) -> (Light, Grade) {
        let l = |az: f32, el: f32, colour: [f32; 3], strength: f32, ambient: f32| Light { azimuth: az, elevation: el, colour, strength, ambient };
        let g = |gain: [f32; 3], lift: [f32; 3], saturation: f32, exposure: f32| Grade { gain, lift, saturation, exposure };
        match self {
            TimeOfDay::Dawn => (l(20.0, 12.0, [1.0, 0.78, 0.7], 0.6, 0.6), g([1.0, 0.92, 0.95], [0.03, 0.02, 0.05], 0.85, 0.9)),
            TimeOfDay::Morning => (l(30.0, 35.0, [1.0, 0.95, 0.85], 0.5, 0.7), g([1.02, 1.0, 0.97], [0.0, 0.0, 0.01], 1.0, 1.0)),
            TimeOfDay::Noon => (l(90.0, 70.0, [1.0, 1.0, 1.0], 0.45, 0.75), g([1.03, 1.03, 1.03], [0.0; 3], 1.05, 1.05)),
            TimeOfDay::GoldenHour => (l(160.0, 15.0, [1.0, 0.7, 0.38], 0.85, 0.5), g([1.08, 0.95, 0.75], [0.02, 0.01, 0.0], 1.12, 1.0)),
            TimeOfDay::Sunset => (l(170.0, 6.0, [1.0, 0.5, 0.28], 0.9, 0.42), g([1.05, 0.8, 0.7], [0.04, 0.01, 0.04], 1.1, 0.85)),
            TimeOfDay::BlueHour => (l(160.0, 4.0, [0.6, 0.7, 1.0], 0.4, 0.45), g([0.75, 0.85, 1.05], [0.01, 0.02, 0.06], 0.8, 0.7)),
            TimeOfDay::Night => (l(110.0, 50.0, [0.55, 0.68, 1.0], 0.35, 0.35), g([0.55, 0.65, 0.9], [0.0, 0.01, 0.04], 0.55, 0.55)),
        }
    }
}

/// `f` moved to time of day `t` (depth optional).
pub fn time_of_day(f: &Frame, depth: Option<&Mask>, t: TimeOfDay, amount: f32) -> Frame {
    let (light, g) = t.look();
    let done = grade(&relight(f, depth, &light), &g);
    mix(f, &done, amount)
}

/// Seasons a backdrop can be moved to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Season {
    Spring,
    Summer,
    Autumn,
    Winter,
}

impl Season {
    pub fn parse(s: &str) -> Option<Season> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "spring" => Season::Spring,
            "summer" => Season::Summer,
            "autumn" | "fall" => Season::Autumn,
            "winter" => Season::Winter,
            _ => return None,
        })
    }

    pub const NAMES: &'static str = "spring, summer, autumn, winter";
}

/// `f` moved to season `s`: foliage greens shifted (fresh, deep, turned
/// to orange and red, or faded to grey-brown with a cold, bright light).
pub fn season(f: &Frame, s: Season, amount: f32) -> Frame {
    let mut out = f.clone();
    for p in out.rgba.chunks_exact_mut(4) {
        let (h, sat, v) = rgb_to_hsv(p[0], p[1], p[2]);
        // Foliage: yellow-green to green-cyan.
        let foliage = smooth(60.0, 85.0, h) * (1.0 - smooth(160.0, 185.0, h)) * smooth(0.12, 0.3, sat);
        let (nh, ns, nv) = match s {
            Season::Spring => (h + (100.0 - h) * 0.3 * foliage, sat * (1.0 + 0.15 * foliage), v * (1.0 + 0.06 * foliage)),
            Season::Summer => (h + (115.0 - h) * 0.2 * foliage, sat * (1.0 + 0.25 * foliage), v),
            Season::Autumn => {
                // Different leaves turn differently: the brightness picks.
                let to = 18.0 + 28.0 * v;
                (h + (to - h) * 0.85 * foliage, sat * (1.0 + 0.2 * foliage), v * (1.0 - 0.05 * foliage))
            }
            Season::Winter => (h + (35.0 - h) * 0.6 * foliage, sat * (1.0 - 0.7 * foliage), v * (1.0 - 0.1 * foliage)),
        };
        let (r, g, b) = hsv_to_rgb(nh.rem_euclid(360.0), ns.clamp(0.0, 1.0), nv.clamp(0.0, 1.0));
        p[0] = r;
        p[1] = g;
        p[2] = b;
    }
    let g = match s {
        Season::Spring => Grade { gain: [1.0, 1.02, 1.0], saturation: 1.05, ..Grade::default() },
        Season::Summer => Grade { gain: [1.05, 1.02, 0.95], saturation: 1.1, exposure: 1.04, ..Grade::default() },
        Season::Autumn => Grade { gain: [1.06, 0.98, 0.88], lift: [0.02, 0.01, 0.0], saturation: 1.05, ..Grade::default() },
        Season::Winter => Grade { gain: [0.95, 1.0, 1.08], lift: [0.06, 0.07, 0.09], saturation: 0.7, exposure: 1.06 },
    };
    mix(f, &grade(&out, &g), amount)
}

fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `a` → `b` by `t` (0..1).
pub fn mix(a: &Frame, b: &Frame, t: f32) -> Frame {
    let t = t.clamp(0.0, 1.0);
    let mut out = b.clone();
    for (o, x) in out.rgba.chunks_exact_mut(4).zip(a.rgba.chunks_exact(4)) {
        for c in 0..3 {
            o[c] = (x[c] as f32 * (1.0 - t) + o[c] as f32 * t + 0.5) as u8;
        }
    }
    out
}

pub fn rgb_to_hsv(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (h, if max == 0.0 { 0.0 } else { d / max }, max)
}

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let q = |v: f32| ((v + m) * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
    (q(r), q(g), q(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dome: near in the middle.
    fn dome(w: usize, h: usize) -> Mask {
        let mut d = vec![0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let (dx, dy) = (x as f32 / w as f32 - 0.5, y as f32 / h as f32 - 0.5);
                d[y * w + x] = (1.0 - (dx * dx + dy * dy).sqrt() * 2.0).max(0.0);
            }
        }
        Mask::new(w, h, d)
    }

    #[test]
    fn a_light_from_the_left_brightens_the_left() {
        let f = Frame::filled(40, 40, [120, 120, 120, 255]);
        let d = dome(40, 40);
        let (az, el) = Light::from_direction("left").unwrap();
        let g = relight(&f, Some(&d), &Light { azimuth: az, elevation: el, colour: [1.0, 1.0, 1.0], strength: 1.0, ambient: 0.5 });
        let left = g.px(12, 20)[0];
        let right = g.px(28, 20)[0];
        assert!(left > right + 20, "left {left} right {right}");
        let warm = relight(&f, Some(&d), &Light { colour: colour_by_name("warm").unwrap(), ..Light::default() });
        let p = warm.px(15, 15);
        assert!(p[0] > p[2], "{p:?}");
        assert_eq!(colour_by_name("#ff8000"), Some([1.0, 128.0 / 255.0, 0.0]));
    }

    #[test]
    fn night_is_darker_and_bluer_autumn_turns_leaves() {
        let f = Frame::filled(16, 16, [150, 150, 150, 255]);
        let n = time_of_day(&f, None, TimeOfDay::parse("night").unwrap(), 1.0);
        let p = n.px(8, 8);
        assert!(p[0] < 120 && p[2] > p[0], "{p:?}");
        let leaves = Frame::filled(8, 8, [60, 140, 50, 255]);
        let a = season(&leaves, Season::parse("fall").unwrap(), 1.0);
        let q = a.px(4, 4);
        assert!(q[0] > q[1], "autumn leaves {q:?}");
        let (h, s, v) = rgb_to_hsv(60, 140, 50);
        assert_eq!(hsv_to_rgb(h, s, v), (60, 140, 50));
    }
}
