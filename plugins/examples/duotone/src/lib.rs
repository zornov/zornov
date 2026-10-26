// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Example Studio plugin: duotone. Maps the brightness of each pixel onto
//! a ramp between two colours, the poster look. The same maths runs on the
//! CPU (below) and on the GPU (`duotone.wgsl`).

use studio_plugin_api::*;

pub struct Duotone;

impl Effect for Duotone {
    fn info(&self) -> EffectInfo {
        EffectInfo::new("duotone", "Duotone")
            .description("Two-colour poster look: shadows in one colour, highlights in another.")
            .category(Category::Stylise)
            .param(Param::colour("shadows", "Shadows", [0.05, 0.02, 0.25, 1.0]))
            .param(Param::colour("highlights", "Highlights", [1.0, 0.55, 0.3, 1.0]))
            .param(Param::float("contrast", "Contrast", 0.25, 4.0, 1.2))
            .param(Param::float("mix", "Mix", 0.0, 1.0, 1.0))
            .wgsl(include_str!("duotone.wgsl"))
    }

    fn apply(&self, _ctx: &Context, frame: &mut Frame, p: &Params) {
        let (lo, hi) = (p.colour("shadows"), p.colour("highlights"));
        let (k, m) = (p.float("contrast"), p.float("mix"));
        for px in frame.pixels.iter_mut() {
            let a = px[3];
            if a <= 1e-6 {
                continue;
            }
            let l = ((0.2126 * px[0] + 0.7152 * px[1] + 0.0722 * px[2]) / a).clamp(0.0, 1.0);
            let t = ((l - 0.5) * k + 0.5).clamp(0.0, 1.0);
            let s = t * t * (3.0 - 2.0 * t);
            for c in 0..3 {
                let tone = (lo[c] + (hi[c] - lo[c]) * s) * a;
                px[c] += (tone - px[c]) * m;
            }
        }
    }
}

pub fn plugin() -> Plugin {
    Plugin::new("duotone", "Duotone", "1.0.0")
        .author("Studio examples")
        .description("A two-colour poster effect, on the CPU and the GPU.")
        .license("Apache-2.0")
        .effect(Duotone)
}

export_plugin!(plugin);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn black_becomes_the_shadow_colour_and_white_the_highlight() {
        let defs = Duotone.info().params;
        let packed = defaults(&defs);
        let mut px = vec![[0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0, 1.0]];
        let mut f = Frame { width: 2, height: 1, pixels: &mut px };
        plugin().run_effect(0, &Context::default(), &mut f, &packed).unwrap();
        assert!((px[0][2] - 0.25).abs() < 1e-5, "{:?}", px[0]);
        assert!((px[1][0] - 1.0).abs() < 1e-5, "{:?}", px[1]);
    }
}
