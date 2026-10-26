// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A light clip's parameters read as how it shows itself and, for the
//! clip that carries the scene's air, the air and look.
//!
//! On any light: `glow`, `flare`, `beam` (strengths, 0 = off),
//! `source_size` (metres), `flicker` (0..1) with `flicker_speed` (Hz), and
//! a flame: `flame` (on) with `flame_height`, `flame_width` and
//! `flame_tongues`.
//!
//! On the light with `atmosphere` on: `ground` (world y of the ground),
//! `metre`, the fog (`fog_density`, `fog_falloff`, `fog_base`,
//! `fog_colour`, `fog_anisotropy`, `fog_range`, `fog_patchiness`), up to
//! three weather layers (`weather_1`..`weather_3` naming the kind, with
//! `_amount`, `_wind_x`, `_wind_y`, `_wind_z`, `_size`, `_speed`, `_colour`,
//! `_splash` and `_zone` = "x0 y0 z0 x1 y1 z1"), the bloom (`bloom_strength`,
//! `bloom_threshold`, `bloom_knee`, `bloom_radius`) and the grade
//! (`exposure`, `contrast`, `saturation`, `tonemap`, `temperature`, `tint`,
//! `richness`, `shadow_tint`, `shadow_tint_amount`, `highlight_tint`,
//! `highlight_tint_amount`, `vignette`, `grain`, `halation`, `fringe`,
//! `auto_exposure`: false to leave the exposure exactly as written),
//! `reflection` and `air_light` (colours).

use super::*;
use studio_model::{ParamMap, Value};

/// How steady a flame-lit light is: 1 ± `amount`, from smooth noise of the
/// time at `speed` Hz, the same for every render of the same instant.
pub fn flicker(secs: f32, seed: u64, amount: f32, speed: f32) -> f32 {
    if amount <= 0.0 {
        return 1.0;
    }
    let s = (seed % 997) as f32 * 3.17;
    let t = secs * speed.max(0.1);
    let n = |x: f32| -> f32 {
        let i = x.floor();
        let f = x - i;
        let u = f * f * (3.0 - 2.0 * f);
        let h = |k: f32| ((k * 127.1 + s).sin() * 43758.547).fract().abs();
        h(i) + (h(i + 1.0) - h(i)) * u
    };
    let v = (n(t) + 0.5 * n(t * 2.3 + 11.0) + 0.25 * n(t * 5.1 + 3.0)) / 1.75;
    (1.0 + amount.clamp(0.0, 1.0) * (v * 2.0 - 1.0) * 0.8).max(0.05)
}

fn num(p: &ParamMap, k: &str) -> Option<f32> {
    p.get(k).and_then(Value::as_f64).map(|v| v as f32)
}

fn rgb(p: &ParamMap, k: &str) -> Option<[f32; 3]> {
    p.get(k).and_then(Value::as_colour).map(|c| {
        let l = c.to_linear();
        [l[0] as f32, l[1] as f32, l[2] as f32]
    })
}

/// What a light's parameters ask for, or `None` when it asks for nothing.
pub fn read(p: &ParamMap, id: u64, secs: f32) -> Option<LightLook> {
    let mut k = LightLook {
        glow: num(p, "glow").unwrap_or(0.0).max(0.0),
        flare: num(p, "flare").unwrap_or(0.0).max(0.0),
        beam: num(p, "beam").unwrap_or(0.0).max(0.0),
        size: num(p, "source_size").unwrap_or(0.0).max(0.0),
        time: secs,
        flame: None,
        atmos: None,
    };
    if matches!(p.get("flame"), Some(Value::Bool(true))) {
        k.flame = Some(Flame { height: num(p, "flame_height").unwrap_or(0.12).max(0.005), width: num(p, "flame_width").unwrap_or(0.05).max(0.003), tongues: num(p, "flame_tongues").unwrap_or(1.0), seed: (id % 89) as f32 / 89.0 });
    }
    if matches!(p.get("atmosphere"), Some(Value::Bool(true))) {
        k.atmos = Some(read_atmos(p));
    }
    (k.visible() || k.atmos.is_some()).then_some(k)
}

fn read_atmos(p: &ParamMap) -> Atmos {
    let mut a = Atmos { metre: num(p, "metre").unwrap_or(0.0).max(0.0), ground_y: num(p, "ground").unwrap_or(0.0), ..Default::default() };
    let density = num(p, "fog_density").unwrap_or(0.0);
    if density > 0.0 {
        let d = Fog::default();
        a.fog = Some(Fog {
            density,
            falloff: num(p, "fog_falloff").unwrap_or(d.falloff).max(0.0),
            base_y: num(p, "fog_base").unwrap_or(a.ground_y),
            colour: rgb(p, "fog_colour").unwrap_or(d.colour),
            anisotropy: num(p, "fog_anisotropy").unwrap_or(d.anisotropy),
            range: num(p, "fog_range").unwrap_or(d.range).max(1.0),
            patchiness: num(p, "fog_patchiness").unwrap_or(0.0).clamp(0.0, 1.0),
        });
    }
    for i in 0..MAX_WEATHER {
        let key = format!("weather_{}", i + 1);
        let Some(kind) = p.get(&key).and_then(Value::as_str).and_then(WeatherKind::parse) else { continue };
        let amount = num(p, &format!("{key}_amount")).unwrap_or(0.6);
        if amount <= 0.0 {
            continue;
        }
        let mut w = Weather::new(kind, amount);
        w.wind = [num(p, &format!("{key}_wind_x")).unwrap_or(0.0), num(p, &format!("{key}_wind_y")).unwrap_or(0.0), num(p, &format!("{key}_wind_z")).unwrap_or(0.0)];
        w.size = num(p, &format!("{key}_size")).unwrap_or(1.0);
        w.speed = num(p, &format!("{key}_speed")).unwrap_or(1.0);
        w.colour = rgb(p, &format!("{key}_colour"));
        if let Some(s) = num(p, &format!("{key}_splash")) {
            w.splash = s.clamp(0.0, 1.0);
        }
        if let Some(z) = p.get(&format!("{key}_zone")).and_then(Value::as_str) {
            let v: Vec<f32> = z.split(|c: char| c.is_whitespace() || c == ',').filter_map(|s| s.parse().ok()).collect();
            if v.len() == 6 {
                w.zone = Some(([v[0].min(v[3]), v[1].min(v[4]), v[2].min(v[5])], [v[0].max(v[3]), v[1].max(v[4]), v[2].max(v[5])]));
            }
        }
        a.weather[i] = Some(w);
    }
    let b = Bloom::default();
    a.bloom = Bloom { strength: num(p, "bloom_strength").unwrap_or(b.strength).max(0.0), threshold: num(p, "bloom_threshold").unwrap_or(b.threshold).max(0.0), knee: num(p, "bloom_knee").unwrap_or(b.knee), radius: num(p, "bloom_radius").unwrap_or(b.radius) };
    let g = Grade::default();
    let tint = |k: &str| -> [f32; 4] {
        match rgb(p, k) {
            Some(c) => [c[0], c[1], c[2], num(p, &format!("{k}_amount")).unwrap_or(0.5).clamp(0.0, 2.0)],
            None => [0.0; 4],
        }
    };
    a.grade = Grade {
        exposure: num(p, "exposure").unwrap_or(g.exposure).clamp(-6.0, 6.0),
        contrast: num(p, "contrast").unwrap_or(g.contrast).clamp(0.2, 3.0),
        saturation: num(p, "saturation").unwrap_or(g.saturation).clamp(0.0, 3.0),
        // A scene with a look is toned and metered unless it says not to:
        // nothing a lamp or a sky adds clips to white by default.
        tonemap: p.get("tonemap").and_then(Value::as_str).and_then(Tonemap::parse).unwrap_or(Tonemap::Filmic),
        temperature: num(p, "temperature").unwrap_or(0.0).clamp(-1.0, 1.0),
        tint: num(p, "tint").unwrap_or(0.0).clamp(-1.0, 1.0),
        richness: num(p, "richness").unwrap_or(0.0).clamp(0.0, 1.0),
        shadow_tint: tint("shadow_tint"),
        highlight_tint: tint("highlight_tint"),
        vignette: num(p, "vignette").unwrap_or(0.0).clamp(0.0, 1.0),
        grain: num(p, "grain").unwrap_or(0.0).clamp(0.0, 1.0),
        halation: num(p, "halation").unwrap_or(0.0).clamp(0.0, 1.0),
        fringe: num(p, "fringe").unwrap_or(0.0).clamp(0.0, 1.0),
        auto_exposure: !matches!(p.get("auto_exposure"), Some(Value::Bool(false))),
    };
    if let Some(c) = rgb(p, "reflection") {
        a.reflection = c;
    }
    if let Some(c) = rgb(p, "air_light") {
        a.ambient = c;
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flicker_is_steady_per_instant_and_moves() {
        assert_eq!(flicker(1.0, 3, 0.0, 7.0), 1.0);
        let times: Vec<f32> = (0..40).map(|i| i as f32 * 0.04).collect();
        let a: Vec<f32> = times.iter().map(|t| flicker(*t, 3, 0.5, 7.0)).collect();
        assert_eq!(a[5], flicker(times[5], 3, 0.5, 7.0));
        let (lo, hi) = a.iter().fold((9.0f32, 0.0f32), |(l, h), v| (l.min(*v), h.max(*v)));
        assert!(hi - lo > 0.15 && lo > 0.4 && hi < 1.6, "{lo} {hi}");
    }

    #[test]
    fn parameters_become_a_look() {
        let mut p = ParamMap::new();
        assert!(read(&p, 1, 0.0).is_none());
        p.insert("glow".into(), Value::Scalar(1.0));
        p.insert("flame".into(), Value::Bool(true));
        let k = read(&p, 7, 2.0).unwrap();
        assert!(k.glow == 1.0 && k.flame.is_some() && k.atmos.is_none() && k.time == 2.0);
        let mut q = ParamMap::new();
        q.insert("atmosphere".into(), Value::Bool(true));
        q.insert("fog_density".into(), Value::Scalar(0.03));
        q.insert("weather_1".into(), Value::Choice("rain".into()));
        q.insert("weather_1_amount".into(), Value::Scalar(0.7));
        q.insert("weather_2".into(), Value::Choice("snowfall".into()));
        q.insert("tonemap".into(), Value::Choice("filmic".into()));
        q.insert("weather_1_zone".into(), Value::Text("0 0 0, 10 -5 10".into()));
        let a = read(&q, 2, 0.0).unwrap().atmos.unwrap();
        assert!(a.fog.is_some_and(|f| f.density == 0.03));
        assert!(a.weather[0].is_some_and(|w| w.kind == WeatherKind::Rain && w.amount == 0.7 && w.zone.is_some()));
        assert!(a.weather[1].is_some_and(|w| w.kind == WeatherKind::Snow));
        assert_eq!(a.grade.tonemap, Tonemap::Filmic);
    }
}
