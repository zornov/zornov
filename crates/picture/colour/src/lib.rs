// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Studio colour science: transfer functions, colour spaces, grading tools,
//! LUTs, automatic corrections and scopes.
//!
//! Everything works on [`Image`], a linear premultiplied RGBA `f32` image.
//! Grading tools run in a log-like grading space (ACEScct on AP1
//! primaries, see [`space`]) and convert back to linear afterwards. The
//! CPU implementation is the reference; [`gpu`] provides WGSL sources for
//! the per-pixel parts and the scope computations.
//!
//! The usual entry point is [`Grade`]: fill in the parameters, then call
//! [`Grade::apply`] (whole image) or [`Grade::prepare`] for a per-pixel
//! [`GradeProcessor`]. Defaults are an exact identity.

pub mod aces;
pub mod auto;
pub mod basic;
pub mod camera;
pub mod cdl;
pub mod curves;
pub mod gpu;
pub mod grade;
pub mod hdr;
pub mod hsl;
pub mod image;
pub mod looks;
pub mod lut;
pub mod math;
pub mod ocio;
pub mod primaries;
pub mod qualifier;
pub mod scopes;
pub mod space;
pub mod spatial;
pub mod spline;
pub mod transfer;
pub mod wheels;
pub mod ycbcr;
pub mod yaml;

pub use auto::{ColourMatch, Levels, auto_levels, auto_white_balance};
pub use aces::DisplayTarget;
pub use basic::Basic;
pub use camera::{CameraSpace, Gamut, LogCurve};
pub use cdl::Cdl;
pub use curves::Curves;
pub use grade::{FrameInfo, Grade, GradeProcessor};
pub use hsl::Hsl;
pub use image::Image;
pub use lut::{CubeFile, LutSettings, LutSpace, parse_3dl, parse_cube, read_lut, write_3dl, write_cube, write_lut};
pub use primaries::Primaries;
pub use qualifier::Qualifier;
pub use transfer::Transfer;
pub use wheels::{Wheel, WheelMode, Wheels};
pub use ycbcr::{Range, YCbCrMatrix};

/// Deterministic helpers shared by unit tests.
#[cfg(test)]
pub(crate) mod test_util {
    /// Small linear congruential generator for randomised tests.
    pub struct Lcg(pub u64);

    impl Lcg {
        /// Uniform in 0..1.
        pub fn next(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((self.0 >> 40) as f32) / (1u64 << 24) as f32
        }
    }
}
