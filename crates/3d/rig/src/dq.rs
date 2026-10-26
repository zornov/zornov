// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dual-quaternion skinning (Kavan et al.): bones' rigid motions blended
//! as dual quaternions instead of matrices, so a twisted forearm or a bent
//! knee keeps its volume where linear blending collapses it ("candy
//! wrapper"). A bone's stretch (squash and stretch, scaled bones) is not
//! rigid: it is split off ([`Bone::from_affine`]), blended linearly in the
//! rest pose, then the rigid part is applied by the blended dual
//! quaternion. 2D skins use the same code with z = 0 and turns about z.

/// A quaternion `[x, y, z, w]`.
pub type Q = [f64; 4];

pub fn q_mul(a: Q, b: Q) -> Q {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

fn q_conj(q: Q) -> Q {
    [-q[0], -q[1], -q[2], q[3]]
}

fn q_dot(a: Q, b: Q) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]
}

/// The rotation of an orthonormal matrix (rows) as a quaternion.
pub fn q_from_matrix(m: [[f64; 3]; 3]) -> Q {
    let tr = m[0][0] + m[1][1] + m[2][2];
    let q = if tr > 0.0 {
        let s = (tr + 1.0).sqrt() * 2.0;
        [(m[2][1] - m[1][2]) / s, (m[0][2] - m[2][0]) / s, (m[1][0] - m[0][1]) / s, 0.25 * s]
    } else if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
        let s = (1.0 + m[0][0] - m[1][1] - m[2][2]).sqrt() * 2.0;
        [0.25 * s, (m[0][1] + m[1][0]) / s, (m[0][2] + m[2][0]) / s, (m[2][1] - m[1][2]) / s]
    } else if m[1][1] > m[2][2] {
        let s = (1.0 + m[1][1] - m[0][0] - m[2][2]).sqrt() * 2.0;
        [(m[0][1] + m[1][0]) / s, 0.25 * s, (m[1][2] + m[2][1]) / s, (m[0][2] - m[2][0]) / s]
    } else {
        let s = (1.0 + m[2][2] - m[0][0] - m[1][1]).sqrt() * 2.0;
        [(m[0][2] + m[2][0]) / s, (m[1][2] + m[2][1]) / s, 0.25 * s, (m[1][0] - m[0][1]) / s]
    };
    let l = q_dot(q, q).sqrt().max(1e-12);
    q.map(|x| x / l)
}

fn rotate(q: Q, p: [f64; 3]) -> [f64; 3] {
    let r = q_mul(q_mul(q, [p[0], p[1], p[2], 0.0]), q_conj(q));
    [r[0], r[1], r[2]]
}

/// A rigid motion as a unit dual quaternion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dq {
    pub real: Q,
    pub dual: Q,
}

impl Dq {
    pub const IDENTITY: Dq = Dq { real: [0.0, 0.0, 0.0, 1.0], dual: [0.0; 4] };

    /// Turn `q`, then move by `t`.
    pub fn new(q: Q, t: [f64; 3]) -> Dq {
        let dual = q_mul([t[0], t[1], t[2], 0.0], q).map(|x| x * 0.5);
        Dq { real: q, dual }
    }

    pub fn translation(&self) -> [f64; 3] {
        let t = q_mul(self.dual.map(|x| x * 2.0), q_conj(self.real));
        [t[0], t[1], t[2]]
    }

    pub fn apply(&self, p: [f64; 3]) -> [f64; 3] {
        let r = rotate(self.real, p);
        let t = self.translation();
        [r[0] + t[0], r[1] + t[1], r[2] + t[2]]
    }
}

/// Blends rigid motions by weight (the shortest way round each, then
/// normalised).
pub fn blend(parts: &[(Dq, f64)]) -> Dq {
    let Some(pivot) = parts.iter().max_by(|a, b| a.1.total_cmp(&b.1)).map(|p| p.0.real) else { return Dq::IDENTITY };
    let (mut r, mut d) = ([0.0; 4], [0.0; 4]);
    for (q, w) in parts {
        let w = if q_dot(q.real, pivot) < 0.0 { -w } else { *w };
        for k in 0..4 {
            r[k] += q.real[k] * w;
            d[k] += q.dual[k] * w;
        }
    }
    let l = q_dot(r, r).sqrt();
    if l < 1e-12 {
        return Dq::IDENTITY;
    }
    Dq { real: r.map(|x| x / l), dual: d.map(|x| x / l) }
}

/// A bone's motion split for skinning: its stretch in the rest pose, then
/// a rigid motion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bone {
    pub rigid: Dq,
    /// Rest-space stretch (identity for a rigid bone).
    pub stretch: [[f64; 3]; 3],
}

impl Bone {
    /// Splits `p ↦ m·p + t` (rows of `m`) into rotation and stretch
    /// (`m = R·S`, polar decomposition).
    pub fn from_affine(m: [[f64; 3]; 3], t: [f64; 3]) -> Bone {
        let r = polar(m);
        // S = Rᵀ·M
        let s: [[f64; 3]; 3] = std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| r[k][i] * m[k][j]).sum()));
        Bone { rigid: Dq::new(q_from_matrix(r), t), stretch: s }
    }
}

/// The rotation nearest `m` (iterated average with its inverse
/// transpose).
fn polar(m: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut r = m;
    for _ in 0..30 {
        let Some(inv) = inverse(r) else { break };
        // (R + R⁻ᵀ) / 2
        let next: [[f64; 3]; 3] = std::array::from_fn(|i| std::array::from_fn(|j| 0.5 * (r[i][j] + inv[j][i])));
        let diff: f64 = (0..3).flat_map(|i| (0..3).map(move |j| (i, j))).map(|(i, j)| (next[i][j] - r[i][j]).abs()).sum();
        r = next;
        if diff < 1e-12 {
            break;
        }
    }
    r
}

fn inverse(m: [[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let c = |i: usize, j: usize| {
        let (a, b) = ((i + 1) % 3, (i + 2) % 3);
        let (x, y) = ((j + 1) % 3, (j + 2) % 3);
        m[a][x] * m[b][y] - m[a][y] * m[b][x]
    };
    let det = m[0][0] * c(0, 0) + m[0][1] * c(0, 1) + m[0][2] * c(0, 2);
    if det.abs() < 1e-14 {
        return None;
    }
    Some(std::array::from_fn(|i| std::array::from_fn(|j| c(j, i) / det)))
}

/// Where point `p` (rest pose) goes under bones blended by `weights`
/// (`(bone, weight)`): stretch blended linearly, motion as dual
/// quaternions.
pub fn skin(p: [f64; 3], bones: &[Bone], weights: &[(usize, f64)]) -> [f64; 3] {
    let total: f64 = weights.iter().filter(|w| w.0 < bones.len()).map(|w| w.1).sum();
    if total <= 1e-12 {
        return p;
    }
    let mut s = [[0.0; 3]; 3];
    let mut parts = Vec::with_capacity(weights.len());
    for &(b, w) in weights {
        let Some(bone) = bones.get(b) else { continue };
        let w = w / total;
        for i in 0..3 {
            for j in 0..3 {
                s[i][j] += bone.stretch[i][j] * w;
            }
        }
        parts.push((bone.rigid, w));
    }
    let q = [0, 1, 2].map(|i| s[i][0] * p[0] + s[i][1] * p[1] + s[i][2] * p[2]);
    blend(&parts).apply(q)
}

/// A 2D map (`x' = m0 x + m1 y + m2`, `y' = m3 x + m4 y + m5`) as a
/// skinning bone, turning about z.
pub fn bone_2d(m: [f64; 6]) -> Bone {
    Bone::from_affine([[m[0], m[1], 0.0], [m[3], m[4], 0.0], [0.0, 0.0, 1.0]], [m[2], m[5], 0.0])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn_z(deg: f64) -> Q {
        let h = deg.to_radians() / 2.0;
        [0.0, 0.0, h.sin(), h.cos()]
    }

    #[test]
    fn a_dual_quaternion_turns_and_moves_like_its_matrix() {
        let d = Dq::new(turn_z(90.0), [1.0, 2.0, 3.0]);
        let p = d.apply([1.0, 0.0, 0.0]);
        assert!((p[0] - 1.0).abs() < 1e-12 && (p[1] - 3.0).abs() < 1e-12 && (p[2] - 3.0).abs() < 1e-12, "{p:?}");
        assert!((d.translation()[1] - 2.0).abs() < 1e-12);
        let m = [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
        let q = q_from_matrix(m);
        assert!((q_dot(q, turn_z(90.0)).abs() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn blending_two_bones_keeps_volume_where_linear_blending_collapses() {
        // A joint at the origin; one bone still, one turned a half turn
        // about the bone's own axis (x): the classic candy wrapper.
        let still = Bone { rigid: Dq::IDENTITY, stretch: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] };
        let h = 170f64.to_radians() / 2.0;
        let twisted = Bone { rigid: Dq::new([h.sin(), 0.0, 0.0, h.cos()], [0.0; 3]), ..still };
        let p = [0.0, 1.0, 0.0];
        let q = skin(p, &[still, twisted], &[(0, 0.5), (1, 0.5)]);
        let r = (q[1] * q[1] + q[2] * q[2]).sqrt();
        assert!((r - 1.0).abs() < 1e-9, "dual quaternions keep the radius: {q:?}");
        // Linear blending of the same two: nearly collapsed.
        let lin = {
            let a = p;
            let b = rotate(twisted.rigid.real, p);
            [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0, (a[2] + b[2]) / 2.0]
        };
        assert!((lin[1] * lin[1] + lin[2] * lin[2]).sqrt() < 0.1);
    }

    #[test]
    fn stretched_bones_split_into_stretch_and_motion() {
        // A 2D bone along x turned 30° and stretched twice along itself.
        let (s, c) = 30f64.to_radians().sin_cos();
        let m = [2.0 * c, -s, 5.0, 2.0 * s, c, 1.0];
        let b = bone_2d(m);
        let p = skin([1.0, 1.0, 0.0], &[b], &[(0, 1.0)]);
        let want = [m[0] + m[1] + m[2], m[3] + m[4] + m[5]];
        assert!((p[0] - want[0]).abs() < 1e-9 && (p[1] - want[1]).abs() < 1e-9, "{p:?} vs {want:?}");
        assert!((b.stretch[0][0] - 2.0).abs() < 1e-9 && (b.stretch[1][1] - 1.0).abs() < 1e-9);
        assert_eq!(skin([3.0, 4.0, 5.0], &[b], &[]), [3.0, 4.0, 5.0]);
    }
}
