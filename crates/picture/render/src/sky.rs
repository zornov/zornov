// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Skies and environments for 3D scenes: a daylight sky with the sun where
//! the time of day puts it (Preetham, Shirley and Smits, "A Practical
//! Analytic Model for Daylight", 1999: the Perez sky luminance and colour
//! distributions driven by atmospheric turbidity), a sun disk that reddens
//! low in the sky, a ground below the horizon, haze over distance; or a
//! panoramic HDR picture (Radiance `.hdr`, equirectangular) lighting the
//! scene from every direction.
//!
//! Directions are Studio's world: y down (up is −y), z away from the
//! camera (north), x right (east).

use crate::space::{V3, add, dot, length, normalise, scale};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// A sky or environment at one instant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyDesc {
    /// Unit direction sunlight travels (from the sun towards the ground).
    pub sun: V3,
    /// Haze: 2 crystal clear – 10 thick haze.
    pub turbidity: f32,
    /// Overall brightness of the sky dome (sun included).
    pub strength: f32,
    /// Linear RGB of the ground (what light below the horizon is).
    pub ground: V3,
    /// Drawn behind the models (else they sit on transparency).
    pub background: bool,
    /// Aerial perspective: how much distance fades into the sky colour
    /// (0 none – 1 thick), per 10 000 px.
    pub haze: f32,
    /// A panoramic picture instead of the daylight model (see [`hdri`]);
    /// 0 for none.
    pub hdri: u64,
    /// Turn of the picture about the vertical, degrees.
    pub hdri_turn: f32,
}

impl Default for SkyDesc {
    fn default() -> SkyDesc {
        SkyDesc { sun: normalise([0.35, 0.7, 0.6]), turbidity: 3.0, strength: 1.0, ground: [0.22, 0.2, 0.17], background: true, haze: 0.0, hdri: 0, hdri_turn: 0.0 }
    }
}

/// The sun's direction of travel for an hour of the day (0–24, local solar
/// time), a day of the year (1–365) and a latitude (degrees north), with
/// north `heading` degrees clockwise from straight ahead (+z).
pub fn sun_for_time(hour: f32, day: f32, latitude: f32, heading: f32) -> V3 {
    let decl = (23.44f32).to_radians() * ((360.0 / 365.0) * (day - 81.0)).to_radians().sin();
    let lat = latitude.to_radians();
    let ha = ((hour - 12.0) * 15.0).to_radians();
    let elev = (lat.sin() * decl.sin() + lat.cos() * decl.cos() * ha.cos()).clamp(-1.0, 1.0).asin();
    // Azimuth from north, clockwise (east = 90°).
    let az = {
        let y = -ha.sin() * decl.cos();
        let x = decl.sin() * lat.cos() - decl.cos() * lat.sin() * ha.cos();
        y.atan2(x)
    };
    let az = az + heading.to_radians();
    // Towards the sun: north +z, east +x, up −y.
    let to_sun = [elev.cos() * az.sin(), -elev.sin(), elev.cos() * az.cos()];
    scale(to_sun, -1.0)
}

/// The sun's elevation above the horizon, degrees.
pub fn elevation(sky: &SkyDesc) -> f32 {
    // Light travelling down (+y) means the sun is up.
    sky.sun[1].clamp(-1.0, 1.0).asin().to_degrees()
}

fn perez(theta: f32, gamma: f32, c: &[f32; 5]) -> f32 {
    (1.0 + c[0] * (c[1] / theta.cos().max(0.01)).exp()) * (1.0 + c[2] * (c[3] * gamma).exp() + c[4] * gamma.cos() * gamma.cos())
}

/// Linear sRGB from CIE xyY.
fn xyy_to_rgb(x: f32, y: f32, lum: f32) -> V3 {
    let yy = y.max(1e-4);
    let xx = x * lum / yy;
    let zz = (1.0 - x - y) * lum / yy;
    [3.2406 * xx - 1.5372 * lum - 0.4986 * zz, -0.9689 * xx + 1.8758 * lum + 0.0415 * zz, 0.0557 * xx - 0.2040 * lum + 1.0570 * zz]
}

/// Sky colour seen looking along `dir` (linear RGB; the sun disk not
/// included — see [`sun_colour`]).
pub fn radiance(sky: &SkyDesc, dir: V3) -> V3 {
    if sky.hdri != 0
        && let Some(h) = hdri(sky.hdri)
    {
        return scale(h.sample(dir, sky.hdri_turn), sky.strength);
    }
    let d = normalise(dir);
    let up = -d[1];
    if up < 0.0 {
        // Below the horizon: the ground lit by the sky, darker straight down.
        let horizon = daylight(sky, normalise([d[0], -0.001, d[2]]));
        let sun_up = sky.sun[1].max(0.0);
        let lit = 0.25 + 0.75 * sun_up;
        let k = (1.0 + up * 3.0).clamp(0.0, 1.0);
        let g: V3 = std::array::from_fn(|i| sky.ground[i] * lit * sky.strength * 0.9);
        return add(scale(horizon, k * 0.6), scale(g, 1.0 - k * 0.6));
    }
    daylight(sky, d)
}

/// The daylight model above the horizon.
fn daylight(sky: &SkyDesc, d: V3) -> V3 {
    let t = sky.turbidity.clamp(1.7, 12.0);
    let to_sun = scale(sky.sun, -1.0);
    let sun_up = (-to_sun[1]).clamp(-1.0, 1.0);
    // Night: a dim blue.
    if sun_up < -0.1 {
        return scale([0.004, 0.006, 0.012], sky.strength);
    }
    let theta_s = sun_up.max(0.02).acos();
    let theta = (-d[1]).clamp(0.001, 1.0).acos();
    let gamma = dot(d, to_sun).clamp(-1.0, 1.0).acos();
    // Zenith luminance (kcd/m²) and chromaticity from turbidity.
    let chi = (4.0 / 9.0 - t / 120.0) * (std::f32::consts::PI - 2.0 * theta_s);
    let yz = ((4.0453 * t - 4.9710) * chi.tan() - 0.2155 * t + 2.4192).max(0.0);
    let ts = theta_s;
    let (t2, ts2, ts3) = (t * t, ts * ts, ts * ts * ts);
    let xz = (0.00166 * ts3 - 0.00375 * ts2 + 0.00209 * ts) * t2 + (-0.02903 * ts3 + 0.06377 * ts2 - 0.03202 * ts + 0.00394) * t + (0.11693 * ts3 - 0.21196 * ts2 + 0.06052 * ts + 0.25886);
    let yzc = (0.00275 * ts3 - 0.00610 * ts2 + 0.00317 * ts) * t2 + (-0.04214 * ts3 + 0.08970 * ts2 - 0.04153 * ts + 0.00516) * t + (0.15346 * ts3 - 0.26756 * ts2 + 0.06670 * ts + 0.26688);
    let cy = [0.1787 * t - 1.4630, -0.3554 * t + 0.4275, -0.0227 * t + 5.3251, 0.1206 * t - 2.5771, -0.0670 * t + 0.3703];
    let cx = [-0.0193 * t - 0.2592, -0.0665 * t + 0.0008, -0.0004 * t + 0.2125, -0.0641 * t - 0.8989, -0.0033 * t + 0.0452];
    let cyc = [-0.0167 * t - 0.2608, -0.0950 * t + 0.0092, -0.0079 * t + 0.2102, -0.0441 * t - 1.6537, -0.0109 * t + 0.0529];
    let f0 = |c: &[f32; 5]| perez(0.0, theta_s, c);
    let lum = yz * perez(theta, gamma, &cy) / f0(&cy);
    let x = xz * perez(theta, gamma, &cx) / f0(&cx);
    let y = yzc * perez(theta, gamma, &cyc) / f0(&cyc);
    // Scaled so a clear noon sky lights a white card about as the default
    // lights do; dimmer as the sun sinks.
    let twilight = ((sun_up + 0.1) / 0.2).clamp(0.0, 1.0);
    let rgb = xyy_to_rgb(x, y, lum * 0.045 * twilight);
    rgb.map(|v| v.max(0.0) * sky.strength)
}

/// The sun's colour and strength (linear RGB) as it reaches the ground:
/// white high up, golden then red as it sinks, gone below the horizon.
pub fn sun_colour(sky: &SkyDesc) -> V3 {
    if sky.hdri != 0 {
        return [0.0; 3];
    }
    // Sine of the sun's height above the horizon.
    let elev = sky.sun[1];
    if elev <= -0.02 {
        return [0.0; 3];
    }
    // Air mass along the light's path; red survives long paths best.
    let m = 1.0 / (elev.max(0.0) + 0.15 * (93.885 - elev.max(0.0).asin().to_degrees()).powf(-1.253)).max(1e-3);
    let t = sky.turbidity.clamp(1.7, 12.0);
    let tau = [0.08, 0.14, 0.28].map(|k: f32| k * (0.55 + 0.1 * t));
    let fade = ((elev + 0.02) / 0.06).clamp(0.0, 1.0);
    std::array::from_fn(|i| (-tau[i] * m).exp() * 1.15 * fade * sky.strength)
}

/// How far (cosine) the sun disk reaches from its centre.
pub const SUN_COS: f32 = 0.999_96;

/// Haze colour and how much of a point `dist` px away is hidden by it.
pub fn haze(sky: &SkyDesc, dir: V3, dist: f32) -> (V3, f32) {
    if sky.haze <= 0.0 {
        return ([0.0; 3], 0.0);
    }
    let k = 1.0 - (-dist * sky.haze / 10_000.0).exp();
    let flat = normalise([dir[0], -0.05, dir[2]]);
    (radiance(sky, flat), k)
}

/// Light arriving on a surface facing `n` from the whole sky (no sun
/// disk), from nine spherical-harmonic numbers worked out once per sky.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Irradiance {
    sh: [[f32; 3]; 9],
}

impl Irradiance {
    pub fn of(sky: &SkyDesc) -> Irradiance {
        let mut sh = [[0.0f32; 3]; 9];
        // A fixed spiral of directions over the sphere.
        let n = 512;
        for i in 0..n {
            let z = 1.0 - 2.0 * (i as f32 + 0.5) / n as f32;
            let r = (1.0 - z * z).max(0.0).sqrt();
            let phi = i as f32 * 2.399_963;
            let d = [r * phi.cos(), z, r * phi.sin()];
            let c = radiance(sky, d);
            let b = basis(d);
            for k in 0..9 {
                for ch in 0..3 {
                    sh[k][ch] += c[ch] * b[k] * (4.0 * std::f32::consts::PI / n as f32);
                }
            }
        }
        // Convolved with the cosine lobe (Ramamoorthi and Hanrahan).
        let a = [std::f32::consts::PI, 2.094_395, 2.094_395, 2.094_395, 0.785_398, 0.785_398, 0.785_398, 0.785_398, 0.785_398];
        for k in 0..9 {
            for ch in 0..3 {
                sh[k][ch] *= a[k] / std::f32::consts::PI;
            }
        }
        Irradiance { sh }
    }

    /// Irradiance over π (what a white diffuse surface reflects).
    pub fn at(&self, n: V3) -> V3 {
        let b = basis(normalise(n));
        let mut out = [0.0f32; 3];
        for k in 0..9 {
            for ch in 0..3 {
                out[ch] += self.sh[k][ch] * b[k];
            }
        }
        out.map(|v| v.max(0.0))
    }
}

fn basis(d: V3) -> [f32; 9] {
    let (x, y, z) = (d[0], d[1], d[2]);
    [0.282_095, 0.488_603 * y, 0.488_603 * z, 0.488_603 * x, 1.092_548 * x * y, 1.092_548 * y * z, 0.315_392 * (3.0 * z * z - 1.0), 1.092_548 * x * z, 0.546_274 * (x * x - y * y)]
}

// ------------------------------------------------------------------ HDRI

/// A panoramic HDR picture (equirectangular, linear RGB).
#[derive(Clone, Debug, PartialEq)]
pub struct Hdri {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<[f32; 3]>,
}

impl Hdri {
    /// The colour seen looking along `dir` (picture turned `turn` degrees).
    pub fn sample(&self, dir: V3, turn: f32) -> V3 {
        let (u, v) = Hdri::uv(dir, turn);
        let x = ((u * self.width as f32) as usize).min(self.width.saturating_sub(1));
        let y = ((v * self.height as f32) as usize).min(self.height.saturating_sub(1));
        self.pixels.get(y * self.width + x).copied().unwrap_or([0.0; 3])
    }

    /// Picture coordinates (0..1) for a direction: the top row is up (−y),
    /// the middle column looks ahead (+z).
    pub fn uv(dir: V3, turn: f32) -> (f32, f32) {
        let d = normalise(dir);
        let phi = d[0].atan2(d[2]) + turn.to_radians();
        let u = (phi / std::f32::consts::TAU + 0.5).rem_euclid(1.0);
        let v = (d[1].clamp(-1.0, 1.0)).acos() / std::f32::consts::PI;
        (u, 1.0 - v)
    }

    /// The direction a picture point looks along.
    pub fn dir(u: f32, v: f32, turn: f32) -> V3 {
        let phi = (u - 0.5) * std::f32::consts::TAU - turn.to_radians();
        let theta = (1.0 - v) * std::f32::consts::PI;
        let y = theta.cos();
        let r = theta.sin();
        [r * phi.sin(), y, r * phi.cos()]
    }

    /// Reads a Radiance `.hdr` (RGBE, flat or run-length encoded).
    pub fn read_hdr(bytes: &[u8]) -> Result<Hdri, String> {
        let mut at = 0;
        let mut line = || -> Result<String, String> {
            let start = at;
            while at < bytes.len() && bytes[at] != b'\n' {
                at += 1;
            }
            let s = String::from_utf8_lossy(&bytes[start..at]).into_owned();
            at += 1;
            if at > bytes.len() + 1 {
                return Err("the HDR file ends early".into());
            }
            Ok(s)
        };
        let first = line()?;
        if !first.starts_with("#?") {
            return Err("not a Radiance HDR file".into());
        }
        loop {
            let l = line()?;
            if l.is_empty() {
                break;
            }
            if l.starts_with("FORMAT=") && !l.contains("32-bit_rle_rgbe") {
                return Err(format!("HDR format {l} is not read (only RGBE)"));
            }
        }
        let size = line()?;
        let parts: Vec<&str> = size.split_whitespace().collect();
        if parts.len() != 4 || parts[0] != "-Y" || parts[2] != "+X" {
            return Err(format!("HDR layout \"{size}\" is not read (only -Y h +X w)"));
        }
        let h: usize = parts[1].parse().map_err(|_| "bad HDR height")?;
        let w: usize = parts[3].parse().map_err(|_| "bad HDR width")?;
        if w == 0 || h == 0 || w * h > 64_000_000 {
            return Err("bad HDR size".into());
        }
        let mut px = vec![[0.0f32; 3]; w * h];
        let mut scan = vec![[0u8; 4]; w];
        let rgbe = |e: [u8; 4]| -> [f32; 3] {
            if e[3] == 0 {
                return [0.0; 3];
            }
            let f = 2f32.powi(e[3] as i32 - 136);
            [e[0] as f32 * f, e[1] as f32 * f, e[2] as f32 * f]
        };
        for y in 0..h {
            let b = |i: usize| -> Result<u8, String> { bytes.get(i).copied().ok_or_else(|| "the HDR file ends early".to_string()) };
            if (8..0x8000).contains(&w) && b(at)? == 2 && b(at + 1)? == 2 && b(at + 2)? & 0x80 == 0 {
                // New-style run-length: four channels, each in runs.
                at += 4;
                for ch in 0..4 {
                    let mut x = 0;
                    while x < w {
                        let c = b(at)?;
                        at += 1;
                        if c > 128 {
                            let n = (c - 128) as usize;
                            let v = b(at)?;
                            at += 1;
                            for k in 0..n.min(w - x) {
                                scan[x + k][ch] = v;
                            }
                            x += n;
                        } else {
                            let n = c as usize;
                            for k in 0..n.min(w - x) {
                                scan[x + k][ch] = b(at + k)?;
                            }
                            at += n;
                            x += n;
                        }
                    }
                }
            } else {
                for s in scan.iter_mut() {
                    *s = [b(at)?, b(at + 1)?, b(at + 2)?, b(at + 3)?];
                    at += 4;
                }
            }
            for x in 0..w {
                px[y * w + x] = rgbe(scan[x]);
            }
        }
        Ok(Hdri { width: w, height: h, pixels: px })
    }

    /// The average colour (what an ambient light should be).
    pub fn average(&self) -> V3 {
        let mut acc = [0.0f64; 3];
        let mut wsum = 0.0f64;
        for y in 0..self.height {
            // Rows near the poles cover less of the sphere.
            let w = (((y as f64 + 0.5) / self.height as f64) * std::f64::consts::PI).sin();
            for x in 0..self.width {
                let p = self.pixels[y * self.width + x];
                for c in 0..3 {
                    acc[c] += p[c] as f64 * w;
                }
                wsum += w;
            }
        }
        acc.map(|v| (v / wsum.max(1e-9)) as f32)
    }
}

type HdriCache = Mutex<HashMap<u64, Arc<Hdri>>>;

fn hdris() -> &'static HdriCache {
    static C: OnceLock<HdriCache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The key a picture file goes by.
pub fn hdri_key(path: &str) -> u64 {
    path.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)) | 1
}

/// Loads a panorama (kept by its key); PNG and JPEG pictures are read as
/// ordinary pictures (sRGB), `.hdr` as light.
pub fn load_hdri(path: &str) -> Result<u64, String> {
    let key = hdri_key(path);
    if hdris().lock().unwrap_or_else(|e| e.into_inner()).contains_key(&key) {
        return Ok(key);
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let img = if bytes.starts_with(b"#?") {
        Hdri::read_hdr(&bytes).map_err(|e| format!("{path}: {e}"))?
    } else {
        let rgba = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) { studio_media::png::decode(&bytes).map_err(|e| format!("{path}: {e:?}"))? } else { studio_media::jpeg::decode(&bytes).map_err(|e| format!("{path}: {e}"))? };
        let lin = |v: u8| studio_mesh::srgb_to_linear(v);
        Hdri { width: rgba.width as usize, height: rgba.height as usize, pixels: rgba.pixels.chunks_exact(4).map(|p| [lin(p[0]), lin(p[1]), lin(p[2])]).collect() }
    };
    hdris().lock().unwrap_or_else(|e| e.into_inner()).insert(key, Arc::new(img));
    Ok(key)
}

/// A loaded panorama by key.
pub fn hdri(key: u64) -> Option<Arc<Hdri>> {
    hdris().lock().unwrap_or_else(|e| e.into_inner()).get(&key).cloned()
}

/// Registers a panorama made in memory (tests, generated skies).
pub fn register_hdri(key: u64, img: Hdri) {
    hdris().lock().unwrap_or_else(|e| e.into_inner()).insert(key, Arc::new(img));
}

/// The brightness of a colour (Rec. 709 luminance).
pub fn luminance(c: V3) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// Length helper re-exported for callers.
pub fn len(v: V3) -> f32 {
    length(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sun_rises_in_the_east_and_the_sky_is_blue_at_noon_and_red_at_dusk() {
        let noon = sun_for_time(12.0, 172.0, 40.0, 0.0);
        let morning = sun_for_time(8.0, 172.0, 40.0, 0.0);
        let evening = sun_for_time(19.5, 172.0, 40.0, 0.0);
        assert!(noon[1] > 0.8, "high at noon (light travels down): {noon:?}");
        // Morning light comes from the east (+x), so it travels towards −x.
        assert!(morning[0] < -0.3, "{morning:?}");
        assert!(evening[0] > 0.3);
        let sky = SkyDesc { sun: noon, ..Default::default() };
        let up = radiance(&sky, [0.0, -1.0, 0.0]);
        assert!(up[2] > up[0], "blue overhead: {up:?}");
        let low = SkyDesc { sun: sun_for_time(19.15, 172.0, 40.0, 0.0), ..Default::default() };
        let sc = sun_colour(&low);
        let hi = sun_colour(&sky);
        assert!(sc[0] > sc[2] * 1.5, "red sun at dusk: {sc:?}");
        assert!(hi[2] > sc[2], "{hi:?}");
        // Irradiance: more light on a surface facing up than down.
        let irr = Irradiance::of(&sky);
        assert!(luminance(irr.at([0.0, -1.0, 0.0])) > luminance(irr.at([0.0, 1.0, 0.0])));
    }

    #[test]
    fn hdr_pictures_read_flat_and_run_length() {
        // A 2×1 flat RGBE file: one white (1.0) pixel, one black.
        let mut f = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 1 +X 2\n".to_vec();
        f.extend_from_slice(&[128, 128, 128, 129, 0, 0, 0, 0]);
        let h = Hdri::read_hdr(&f).unwrap();
        assert_eq!((h.width, h.height), (2, 1));
        assert!((h.pixels[0][0] - 1.0).abs() < 1e-3 && h.pixels[1] == [0.0; 3]);
        // Directions and picture coordinates agree.
        for d in [[0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.3, -0.8, -0.5]] {
            let d = normalise(d);
            let (u, v) = Hdri::uv(d, 30.0);
            let back = Hdri::dir(u, v, 30.0);
            assert!(length(crate::space::sub(back, d)) < 1e-3, "{d:?} {back:?}");
        }
    }
}
