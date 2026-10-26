// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Small linear-algebra and shaping helpers shared by the colour tools.

/// An RGB triple. Meaning (linear, encoded, grading space) depends on context.
pub type Rgb = [f32; 3];

/// 3×3 matrix stored row-major in `f64`, so that chains of conversions
/// (primaries → XYZ → adaptation → primaries) keep full precision before
/// they are applied to `f32` pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat3(pub [[f64; 3]; 3]);

impl Mat3 {
    pub const IDENTITY: Mat3 = Mat3([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);

    pub fn diag(d: [f64; 3]) -> Mat3 {
        Mat3([[d[0], 0.0, 0.0], [0.0, d[1], 0.0], [0.0, 0.0, d[2]]])
    }

    /// Builds a matrix whose columns are the three given vectors.
    pub fn from_columns(c0: [f64; 3], c1: [f64; 3], c2: [f64; 3]) -> Mat3 {
        Mat3([[c0[0], c1[0], c2[0]], [c0[1], c1[1], c2[1]], [c0[2], c1[2], c2[2]]])
    }

    pub fn mul(&self, o: &Mat3) -> Mat3 {
        let a = &self.0;
        let b = &o.0;
        let mut r = [[0.0; 3]; 3];
        for (i, row) in r.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
            }
        }
        Mat3(r)
    }

    pub fn mul_vec64(&self, v: [f64; 3]) -> [f64; 3] {
        let m = &self.0;
        [
            m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
            m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
            m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
        ]
    }

    /// Applies the matrix to an `f32` pixel.
    pub fn apply(&self, v: Rgb) -> Rgb {
        let r = self.mul_vec64([v[0] as f64, v[1] as f64, v[2] as f64]);
        [r[0] as f32, r[1] as f32, r[2] as f32]
    }

    pub fn determinant(&self) -> f64 {
        let m = &self.0;
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    }

    /// Inverse, or `None` when the matrix is (numerically) singular.
    pub fn inverse(&self) -> Option<Mat3> {
        let det = self.determinant();
        if det.abs() < 1e-14 {
            return None;
        }
        let m = &self.0;
        let inv = 1.0 / det;
        Some(Mat3([
            [
                (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * inv,
                (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * inv,
                (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * inv,
            ],
            [
                (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * inv,
                (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * inv,
                (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * inv,
            ],
            [
                (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * inv,
                (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * inv,
                (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * inv,
            ],
        ]))
    }

    /// True when every element is within `eps` of the identity.
    pub fn is_near_identity(&self, eps: f64) -> bool {
        (0..3).all(|i| (0..3).all(|j| (self.0[i][j] - Mat3::IDENTITY.0[i][j]).abs() <= eps))
    }

    /// Row-major `f32` copy, convenient for GPU uniforms.
    pub fn to_f32(&self) -> [[f32; 3]; 3] {
        let m = &self.0;
        [
            [m[0][0] as f32, m[0][1] as f32, m[0][2] as f32],
            [m[1][0] as f32, m[1][1] as f32, m[1][2] as f32],
            [m[2][0] as f32, m[2][1] as f32, m[2][2] as f32],
        ]
    }
}

/// Hermite smoothstep: 0 below `e0`, 1 above `e1`, C1-continuous between.
#[inline]
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[inline]
pub fn lerp3(a: Rgb, b: Rgb, t: f32) -> Rgb {
    [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)]
}

/// Weighted sum with the given luma weights.
#[inline]
pub fn dot3(a: Rgb, w: Rgb) -> f32 {
    a[0] * w[0] + a[1] * w[1] + a[2] * w[2]
}

/// Rec. 709 luma weights, used by the ASC CDL saturation operator and as the
/// default weighting for saturation-style operations in the grading space.
pub const REC709_LUMA: Rgb = [0.2126, 0.7152, 0.0722];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_round_trip() {
        let m = Mat3([[0.4, 0.3, 0.2], [0.2, 0.7, 0.1], [0.02, 0.1, 0.9]]);
        let p = m.mul(&m.inverse().unwrap());
        assert!(p.is_near_identity(1e-12));
    }

    #[test]
    fn smoothstep_ends() {
        assert_eq!(smoothstep(0.0, 1.0, -1.0), 0.0);
        assert_eq!(smoothstep(0.0, 1.0, 2.0), 1.0);
        assert!((smoothstep(0.0, 1.0, 0.5) - 0.5).abs() < 1e-7);
    }
}
