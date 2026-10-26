// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Small linear algebra for the solver: 3x3 matrices (rows), poses
//! (a turn and a shift) and the handful of vector helpers the mesh crate
//! does not have.

pub use crate::{V3, add, cross, dot, length, normalise, scale, sub};
pub use studio_mesh::rig::{Q_ID, Quat, q_conj, q_mul, q_normalise, q_rotate, q_to_mat};

/// Rows.
pub type M3 = [V3; 3];

pub const M_ZERO: M3 = [[0.0; 3]; 3];
pub const M_ID: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

pub fn neg(a: V3) -> V3 {
    [-a[0], -a[1], -a[2]]
}

pub fn len2(a: V3) -> f32 {
    dot(a, a)
}

pub fn mad(a: V3, b: V3, k: f32) -> V3 {
    [a[0] + b[0] * k, a[1] + b[1] * k, a[2] + b[2] * k]
}

pub fn vmin(a: V3, b: V3) -> V3 {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])]
}

pub fn vmax(a: V3, b: V3) -> V3 {
    [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])]
}

/// A unit vector, or `fallback` when `a` is too short to have a direction.
pub fn normalise_or(a: V3, fallback: V3) -> V3 {
    let l = length(a);
    if l > 1e-9 { scale(a, 1.0 / l) } else { fallback }
}

/// Two unit vectors that with `n` make a right-handed frame.
pub fn tangents(n: V3) -> (V3, V3) {
    let a = if n[0].abs() < 0.57 { [1.0, 0.0, 0.0] } else if n[1].abs() < 0.57 { [0.0, 1.0, 0.0] } else { [0.0, 0.0, 1.0] };
    let t1 = normalise(cross(n, a));
    (t1, cross(n, t1))
}

pub fn m_vec(m: &M3, v: V3) -> V3 {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}

pub fn m_t(m: &M3) -> M3 {
    std::array::from_fn(|r| std::array::from_fn(|c| m[c][r]))
}

pub fn m_mul(a: &M3, b: &M3) -> M3 {
    std::array::from_fn(|r| std::array::from_fn(|c| a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c]))
}

pub fn m_add(a: &M3, b: &M3) -> M3 {
    std::array::from_fn(|r| std::array::from_fn(|c| a[r][c] + b[r][c]))
}

pub fn m_scale(a: &M3, k: f32) -> M3 {
    std::array::from_fn(|r| std::array::from_fn(|c| a[r][c] * k))
}

pub fn m_diag(d: V3) -> M3 {
    [[d[0], 0.0, 0.0], [0.0, d[1], 0.0], [0.0, 0.0, d[2]]]
}

/// The matrix of `v ×`.
pub fn m_skew(v: V3) -> M3 {
    [[0.0, -v[2], v[1]], [v[2], 0.0, -v[0]], [-v[1], v[0], 0.0]]
}

pub fn m_inverse(m: &M3) -> M3 {
    let c0 = cross(m[1], m[2]);
    let det = dot(m[0], c0);
    if det.abs() < 1e-30 {
        return M_ZERO;
    }
    let c1 = cross(m[2], m[0]);
    let c2 = cross(m[0], m[1]);
    let k = 1.0 / det;
    [[c0[0] * k, c1[0] * k, c2[0] * k], [c0[1] * k, c1[1] * k, c2[1] * k], [c0[2] * k, c1[2] * k, c2[2] * k]]
}

/// `R · M · Rᵀ` for a turn `q`: a body-frame tensor in the world.
pub fn m_rotate(q: Quat, m: &M3) -> M3 {
    let r = q_to_mat(q);
    m_mul(&m_mul(&r, m), &m_t(&r))
}

/// The eigenvectors (columns of the returned turn) and eigenvalues of a
/// symmetric matrix, by Jacobi sweeps: a body's principal axes.
pub fn eigen_sym(m: &M3) -> (M3, V3) {
    let mut a = *m;
    let mut v = M_ID;
    for _ in 0..24 {
        let off = a[0][1].abs() + a[0][2].abs() + a[1][2].abs();
        if off < 1e-12 {
            break;
        }
        for (p, q) in [(0usize, 1usize), (0, 2), (1, 2)] {
            if a[p][q].abs() < 1e-15 {
                continue;
            }
            let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
            let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
            let t = if theta == 0.0 { 1.0 } else { t };
            let c = 1.0 / (t * t + 1.0).sqrt();
            let s = t * c;
            let mut j = M_ID;
            j[p][p] = c;
            j[q][q] = c;
            j[p][q] = s;
            j[q][p] = -s;
            a = m_mul(&m_mul(&m_t(&j), &a), &j);
            v = m_mul(&v, &j);
        }
    }
    (v, [a[0][0], a[1][1], a[2][2]])
}

/// A turn from rotation-matrix rows.
pub fn q_from_mat(r: &M3) -> Quat {
    studio_mesh::rig::q_from_axes([r[0][0], r[1][0], r[2][0]], [r[0][1], r[1][1], r[2][1]], [r[0][2], r[1][2], r[2][2]])
}

/// The turn by `w` radians a second over `h` seconds applied to `q`.
pub fn q_integrate(q: Quat, w: V3, h: f32) -> Quat {
    let dq = q_mul([w[0], w[1], w[2], 0.0], q);
    q_normalise([q[0] + 0.5 * h * dq[0], q[1] + 0.5 * h * dq[1], q[2] + 0.5 * h * dq[2], q[3] + 0.5 * h * dq[3]])
}

/// The rotation vector (axis × angle) of a turn, the short way round.
pub fn q_log(q: Quat) -> V3 {
    let q = if q[3] < 0.0 { [-q[0], -q[1], -q[2], -q[3]] } else { q };
    let s = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2]).sqrt();
    if s < 1e-7 {
        return [2.0 * q[0], 2.0 * q[1], 2.0 * q[2]];
    }
    let angle = 2.0 * s.atan2(q[3]);
    scale([q[0], q[1], q[2]], angle / s)
}

/// Where a body or a collider is: `p` is its origin, `q` its turn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub p: V3,
    pub q: Quat,
}

impl Default for Pose {
    fn default() -> Pose {
        Pose::IDENTITY
    }
}

impl Pose {
    pub const IDENTITY: Pose = Pose { p: [0.0; 3], q: Q_ID };

    pub fn new(p: V3, q: Quat) -> Pose {
        Pose { p, q }
    }

    pub fn at(p: V3) -> Pose {
        Pose { p, q: Q_ID }
    }

    /// A local point in the world.
    pub fn apply(&self, v: V3) -> V3 {
        add(self.p, q_rotate(self.q, v))
    }

    pub fn rotate(&self, v: V3) -> V3 {
        q_rotate(self.q, v)
    }

    /// A world point in the local frame.
    pub fn unapply(&self, v: V3) -> V3 {
        q_rotate(q_conj(self.q), sub(v, self.p))
    }

    pub fn unrotate(&self, v: V3) -> V3 {
        q_rotate(q_conj(self.q), v)
    }

    /// `self ∘ other`: `other` is in this frame.
    pub fn then(&self, other: &Pose) -> Pose {
        Pose { p: self.apply(other.p), q: q_normalise(q_mul(self.q, other.q)) }
    }

    pub fn inverse(&self) -> Pose {
        let qi = q_conj(self.q);
        Pose { p: neg(q_rotate(qi, self.p)), q: qi }
    }
}

/// An axis-aligned box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub lo: V3,
    pub hi: V3,
}

impl Aabb {
    pub const EMPTY: Aabb = Aabb { lo: [f32::INFINITY; 3], hi: [f32::NEG_INFINITY; 3] };

    pub fn around(c: V3, r: f32) -> Aabb {
        Aabb { lo: [c[0] - r, c[1] - r, c[2] - r], hi: [c[0] + r, c[1] + r, c[2] + r] }
    }

    pub fn grow(&mut self, p: V3) {
        self.lo = vmin(self.lo, p);
        self.hi = vmax(self.hi, p);
    }

    pub fn union(&self, o: &Aabb) -> Aabb {
        Aabb { lo: vmin(self.lo, o.lo), hi: vmax(self.hi, o.hi) }
    }

    pub fn fattened(&self, m: f32) -> Aabb {
        Aabb { lo: [self.lo[0] - m, self.lo[1] - m, self.lo[2] - m], hi: [self.hi[0] + m, self.hi[1] + m, self.hi[2] + m] }
    }

    pub fn overlaps(&self, o: &Aabb) -> bool {
        (0..3).all(|k| self.lo[k] <= o.hi[k] && o.lo[k] <= self.hi[k])
    }

    pub fn centre(&self) -> V3 {
        scale(add(self.lo, self.hi), 0.5)
    }

    pub fn size(&self) -> V3 {
        sub(self.hi, self.lo)
    }

    pub fn is_empty(&self) -> bool {
        (0..3).any(|k| self.lo[k] > self.hi[k])
    }
}

/// The closest points between segments `p0 p1` and `q0 q1`: the
/// parameters along each (0..1).
pub fn segment_segment(p0: V3, p1: V3, q0: V3, q1: V3) -> (f32, f32) {
    let d1 = sub(p1, p0);
    let d2 = sub(q1, q0);
    let r = sub(p0, q0);
    let a = dot(d1, d1);
    let e = dot(d2, d2);
    let f = dot(d2, r);
    if a <= 1e-12 && e <= 1e-12 {
        return (0.0, 0.0);
    }
    if a <= 1e-12 {
        return (0.0, (f / e).clamp(0.0, 1.0));
    }
    let c = dot(d1, r);
    if e <= 1e-12 {
        return ((-c / a).clamp(0.0, 1.0), 0.0);
    }
    let b = dot(d1, d2);
    let denom = a * e - b * b;
    let mut s = if denom > 1e-12 * a * e { ((b * f - c * e) / denom).clamp(0.0, 1.0) } else { 0.0 };
    let mut t = (b * s + f) / e;
    if t < 0.0 {
        t = 0.0;
        s = (-c / a).clamp(0.0, 1.0);
    } else if t > 1.0 {
        t = 1.0;
        s = ((b - c) / a).clamp(0.0, 1.0);
    }
    (s, t)
}

pub fn closest_on_segment(a: V3, b: V3, p: V3) -> V3 {
    let ab = sub(b, a);
    let l2 = dot(ab, ab);
    if l2 < 1e-12 {
        return a;
    }
    mad(a, ab, (dot(sub(p, a), ab) / l2).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eigen_of_a_turned_diagonal_finds_its_axes() {
        let q = q_normalise([0.3, 0.2, -0.4, 0.8]);
        let m = m_rotate(q, &m_diag([1.0, 2.0, 5.0]));
        let (v, d) = eigen_sym(&m);
        let mut s = d;
        s.sort_by(f32::total_cmp);
        assert!((s[0] - 1.0).abs() < 1e-4 && (s[1] - 2.0).abs() < 1e-4 && (s[2] - 5.0).abs() < 1e-4, "{d:?}");
        // V D Vᵀ rebuilds it.
        let back = m_mul(&m_mul(&v, &m_diag(d)), &m_t(&v));
        for r in 0..3 {
            for c in 0..3 {
                assert!((back[r][c] - m[r][c]).abs() < 1e-4);
            }
        }
    }

    #[test]
    fn pose_round_trips() {
        let p = Pose::new([1.0, 2.0, 3.0], q_normalise([0.1, 0.7, 0.2, 0.6]));
        let v = [0.3, -2.0, 5.0];
        let back = p.unapply(p.apply(v));
        assert!(length(sub(back, v)) < 1e-5);
        let i = p.then(&p.inverse());
        assert!(length(i.p) < 1e-5);
        assert!(length(q_log(i.q)) < 1e-4);
    }
}
