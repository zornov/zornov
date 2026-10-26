// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Studio's own looks, defined as grade parameters.
//!
//! Each look is baked into a 33³ `.cube` in the [`LutSpace::Rec709`]
//! signal space (BT.709 primaries, BT.1886 encoding, output clamped to
//! 0..1) by `cargo run -p studio-colour --example gen_looks`, which writes
//! `crates/picture/colour/looks/<slug>.cube`. The parameters below are the source
//! of truth; the shipped files are checked against them by a test.

use crate::basic::Basic;
use crate::grade::Grade;
use crate::lut::{Lut3d, LutSpace};
use crate::transfer::Transfer;
use crate::wheels::{Wheel, Wheels};

/// Lattice size of shipped looks.
pub const LOOK_SIZE: usize = 33;

pub struct Look {
    pub slug: &'static str,
    pub title: &'static str,
    pub grade: fn() -> Grade,
}

fn warm_afternoon() -> Grade {
    Grade {
        basic: Basic { temperature: 7600.0, contrast: 0.12, saturation: 1.06, ..Basic::default() },
        wheels: Wheels {
            lift: Wheel { hue: 200.0, amount: 0.10, luma: 0.0 },
            gain: Wheel { hue: 40.0, amount: 0.15, luma: 0.02 },
            ..Wheels::default()
        },
        ..Grade::default()
    }
}

fn blue_hour() -> Grade {
    Grade {
        basic: Basic {
            exposure: -0.25,
            temperature: 5200.0,
            contrast: 0.2,
            blacks: -0.2,
            saturation: 0.85,
            ..Basic::default()
        },
        wheels: Wheels { gamma: Wheel { hue: 225.0, amount: 0.25, luma: -0.05 }, ..Wheels::default() },
        ..Grade::default()
    }
}

fn faded_print() -> Grade {
    let mut g = Grade {
        basic: Basic { fade: 0.45, contrast: -0.1, highlights: -0.3, saturation: 0.82, ..Basic::default() },
        wheels: Wheels { gain: Wheel { hue: 50.0, amount: 0.1, luma: -0.03 }, ..Wheels::default() },
        ..Grade::default()
    };
    g.curves.master = vec![[0.0, 0.0], [0.3, 0.27], [0.7, 0.73], [1.0, 1.0]];
    g
}

fn dusk_contrast() -> Grade {
    Grade {
        basic: Basic { contrast: 0.25, vibrance: 0.2, shadows: -0.1, ..Basic::default() },
        wheels: Wheels {
            lift: Wheel { hue: 195.0, amount: 0.25, luma: 0.0 },
            gain: Wheel { hue: 35.0, amount: 0.2, luma: 0.0 },
            ..Wheels::default()
        },
        ..Grade::default()
    }
}

fn silver_mono() -> Grade {
    let mut g = Grade { basic: Basic { saturation: 0.0, contrast: 0.3, ..Basic::default() }, ..Grade::default() };
    g.curves.master = vec![[0.0, 0.02], [0.25, 0.2], [0.75, 0.8], [1.0, 1.0]];
    g
}

pub const LOOKS: &[Look] = &[
    Look { slug: "warm-afternoon", title: "Warm Afternoon", grade: warm_afternoon },
    Look { slug: "blue-hour", title: "Blue Hour", grade: blue_hour },
    Look { slug: "faded-print", title: "Faded Print", grade: faded_print },
    Look { slug: "dusk-contrast", title: "Dusk Contrast", grade: dusk_contrast },
    Look { slug: "silver-mono", title: "Silver Mono", grade: silver_mono },
];

impl Look {
    /// Bakes the look into a Rec. 709 / BT.1886 3D LUT.
    pub fn bake(&self, size: usize) -> Lut3d {
        let p = (self.grade)().prepare();
        let t = LutSpace::Rec709.transfer();
        debug_assert_eq!(t, Transfer::Bt1886);
        Lut3d::from_fn(size, |e| {
            let lin = t.decode_rgb(e);
            t.encode_rgb(p.apply_rgb(lin)).map(|v| v.clamp(0.0, 1.0))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lut::parse_cube;

    #[test]
    fn shipped_looks_match_their_parameters() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("looks");
        for look in LOOKS {
            let path = dir.join(format!("{}.cube", look.slug));
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|_| panic!("missing {path:?}; run the gen_looks example"));
            let file = parse_cube(&text).unwrap();
            let shipped = file.lut3d.unwrap();
            assert_eq!(shipped.size, LOOK_SIZE);
            let baked = look.bake(LOOK_SIZE);
            for (a, b) in shipped.data.iter().zip(&baked.data) {
                for i in 0..3 {
                    assert!((a[i] - b[i]).abs() < 2e-5, "{} out of date", look.slug);
                }
            }
        }
    }
}
