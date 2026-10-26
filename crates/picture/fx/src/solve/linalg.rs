// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Small dense linear algebra for the camera solver: symmetric eigen
//! decomposition (Jacobi), 3×3 SVD, Cholesky solves and rotations.

pub type V3 = [f64; 3];
pub type M3 = [[f64; 3]; 3];

pub const I3: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn scale(a: V3, k: f64) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
pub fn norm(a: V3) -> f64 {
    dot(a, a).sqrt()
}
pub fn unit(a: V3) -> V3 {
    scale(a, 1.0 / norm(a).max(1e-300))
}
pub fn mul(a: &M3, b: &M3) -> M3 {
    std::array::from_fn(|r| std::array::from_fn(|c| (0..3).map(|k| a[r][k] * b[k][c]).sum()))
}
pub fn mv(m: &M3, v: V3) -> V3 {
    std::array::from_fn(|r| m[r][0] * v[0] + m[r][1] * v[1] + m[r][2] * v[2])
}
pub fn tr(m: &M3) -> M3 {
    std::array::from_fn(|r| std::array::from_fn(|c| m[c][r]))
}
pub fn det3(m: &M3) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}
pub fn skew(v: V3) -> M3 {
    [[0.0, -v[2], v[1]], [v[2], 0.0, -v[0]], [-v[1], v[0], 0.0]]
}

/// Rotation matrix of an axis-angle vector (Rodrigues).
pub fn rodrigues(w: V3) -> M3 {
    let th = norm(w);
    if th < 1e-12 {
        let k = skew(w);
        return std::array::from_fn(|r| std::array::from_fn(|c| I3[r][c] + k[r][c]));
    }
    let k = skew(scale(w, 1.0 / th));
    let k2 = mul(&k, &k);
    let (s, c) = th.sin_cos();
    std::array::from_fn(|r| std::array::from_fn(|cc| I3[r][cc] + s * k[r][cc] + (1.0 - c) * k2[r][cc]))
}

/// Axis-angle vector of a rotation matrix.
pub fn log_rot(r: &M3) -> V3 {
    let c = ((r[0][0] + r[1][1] + r[2][2] - 1.0) * 0.5).clamp(-1.0, 1.0);
    let th = c.acos();
    if th < 1e-9 {
        return [0.0; 3];
    }
    if (std::f64::consts::PI - th).abs() < 1e-6 {
        // Near 180°: axis from the largest diagonal.
        let i = (0..3).max_by(|&a, &b| r[a][a].total_cmp(&r[b][b])).unwrap_or(0);
        let mut v = [0.0; 3];
        v[i] = ((r[i][i] + 1.0) * 0.5).max(0.0).sqrt();
        for j in 0..3 {
            if j != i {
                v[j] = r[i][j] / (2.0 * v[i]).max(1e-12);
            }
        }
        return scale(unit(v), th);
    }
    let k = th / (2.0 * th.sin());
    [k * (r[2][1] - r[1][2]), k * (r[0][2] - r[2][0]), k * (r[1][0] - r[0][1])]
}

/// The nearest rotation to a 3×3 matrix (polar decomposition by SVD).
pub fn orthonormalise(m: &M3) -> M3 {
    let (u, _, v) = svd3(m);
    let mut r = mul(&u, &tr(&v));
    if det3(&r) < 0.0 {
        let mut u2 = u;
        for row in u2.iter_mut() {
            row[2] = -row[2];
        }
        r = mul(&u2, &tr(&v));
    }
    r
}

/// Eigen decomposition of a symmetric `n×n` matrix (row-major): values
/// ascending and the matching unit vectors (as rows).
pub fn sym_eigen(a: &[f64], n: usize) -> (Vec<f64>, Vec<Vec<f64>>) {
    let mut m = a.to_vec();
    let mut v = vec![0.0; n * n];
    for i in 0..n {
        v[i * n + i] = 1.0;
    }
    for _sweep in 0..100 {
        let off: f64 = (0..n).flat_map(|i| (0..n).filter(move |&j| j != i).map(move |j| (i, j))).map(|(i, j)| m[i * n + j].powi(2)).sum();
        if off < 1e-22 {
            break;
        }
        for p in 0..n {
            for q in p + 1..n {
                let apq = m[p * n + q];
                if apq.abs() < 1e-300 {
                    continue;
                }
                let theta = (m[q * n + q] - m[p * n + p]) / (2.0 * apq);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let t = if theta == 0.0 { 1.0 } else { t };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for k in 0..n {
                    let (mkp, mkq) = (m[k * n + p], m[k * n + q]);
                    m[k * n + p] = c * mkp - s * mkq;
                    m[k * n + q] = s * mkp + c * mkq;
                }
                for k in 0..n {
                    let (mpk, mqk) = (m[p * n + k], m[q * n + k]);
                    m[p * n + k] = c * mpk - s * mqk;
                    m[q * n + k] = s * mpk + c * mqk;
                }
                for k in 0..n {
                    let (vkp, vkq) = (v[k * n + p], v[k * n + q]);
                    v[k * n + p] = c * vkp - s * vkq;
                    v[k * n + q] = s * vkp + c * vkq;
                }
            }
        }
    }
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by(|&a, &b| m[a * n + a].total_cmp(&m[b * n + b]));
    let vals = idx.iter().map(|&i| m[i * n + i]).collect();
    let vecs = idx.iter().map(|&i| (0..n).map(|k| v[k * n + i]).collect()).collect();
    (vals, vecs)
}

/// The unit vector `x` minimising `|A x|` for the rows of `A`.
pub fn null_vector(rows: &[Vec<f64>]) -> Vec<f64> {
    let n = rows.first().map_or(0, |r| r.len());
    let mut ata = vec![0.0; n * n];
    for r in rows {
        for i in 0..n {
            for j in 0..n {
                ata[i * n + j] += r[i] * r[j];
            }
        }
    }
    sym_eigen(&ata, n).1.swap_remove(0)
}

/// SVD of a 3×3 matrix: `m = u · diag(s) · vᵀ`, singular values
/// descending.
pub fn svd3(m: &M3) -> (M3, V3, M3) {
    let mtm = mul(&tr(m), m);
    let flat: Vec<f64> = mtm.iter().flatten().copied().collect();
    let (vals, vecs) = sym_eigen(&flat, 3);
    // Descending.
    let order = [2usize, 1, 0];
    let mut v = [[0.0; 3]; 3];
    let mut s = [0.0; 3];
    for (k, &i) in order.iter().enumerate() {
        s[k] = vals[i].max(0.0).sqrt();
        for r in 0..3 {
            v[r][k] = vecs[i][r];
        }
    }
    let mut u = [[0.0; 3]; 3];
    for k in 0..3 {
        let col = mv(m, [v[0][k], v[1][k], v[2][k]]);
        let c = if s[k] > 1e-9 * s[0].max(1e-300) { scale(col, 1.0 / s[k]) } else { [0.0; 3] };
        for r in 0..3 {
            u[r][k] = c[r];
        }
    }
    // Complete u where a singular value (nearly) vanished: its column is
    // then noise, so take the cross product of the other two.
    if s[2] <= 1e-6 * s[0] {
        let c = cross([u[0][0], u[1][0], u[2][0]], [u[0][1], u[1][1], u[2][1]]);
        let c = unit(c);
        for r in 0..3 {
            u[r][2] = c[r];
        }
    }
    (u, s, v)
}

/// Solves `A x = b` for a symmetric positive-definite `A` (Cholesky).
/// Returns `None` when `A` is not positive definite.
pub fn cholesky_solve(a: &[f64], b: &[f64], n: usize) -> Option<Vec<f64>> {
    let mut l = vec![0.0; n * n];
    for i in 0..n {
        for j in 0..=i {
            let mut s = a[i * n + j];
            for k in 0..j {
                s -= l[i * n + k] * l[j * n + k];
            }
            if i == j {
                if s <= 0.0 || !s.is_finite() {
                    return None;
                }
                l[i * n + i] = s.sqrt();
            } else {
                l[i * n + j] = s / l[j * n + j];
            }
        }
    }
    let mut y = vec![0.0; n];
    for i in 0..n {
        let mut s = b[i];
        for k in 0..i {
            s -= l[i * n + k] * y[k];
        }
        y[i] = s / l[i * n + i];
    }
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let mut s = y[i];
        for k in i + 1..n {
            s -= l[k * n + i] * x[k];
        }
        x[i] = s / l[i * n + i];
    }
    Some(x)
}

/// Inverse of a symmetric positive-definite 3×3.
pub fn inv3(m: &M3) -> Option<M3> {
    let d = det3(m);
    if d.abs() < 1e-300 {
        return None;
    }
    let c = |r0: usize, c0: usize, r1: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
    Some([
        [c(1, 1, 2, 2) / d, -c(0, 1, 2, 2) / d, c(0, 1, 1, 2) / d],
        [-c(1, 0, 2, 2) / d, c(0, 0, 2, 2) / d, -c(0, 0, 1, 2) / d],
        [c(1, 0, 2, 1) / d, -c(0, 0, 2, 1) / d, c(0, 0, 1, 1) / d],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rodrigues_round_trips() {
        for w in [[0.1, -0.2, 0.3], [1.0, 0.5, -2.0], [0.0, 0.0, 3.0]] {
            let r = rodrigues(w);
            assert!((det3(&r) - 1.0).abs() < 1e-9);
            let back = log_rot(&r);
            let r2 = rodrigues(back);
            for i in 0..3 {
                for j in 0..3 {
                    assert!((r[i][j] - r2[i][j]).abs() < 1e-9);
                }
            }
        }
    }

    #[test]
    fn svd_reconstructs() {
        let m = [[2.0, -1.0, 0.5], [0.3, 1.5, -0.7], [1.1, 0.2, 0.9]];
        let (u, s, v) = svd3(&m);
        let back = mul(&mul(&u, &[[s[0], 0.0, 0.0], [0.0, s[1], 0.0], [0.0, 0.0, s[2]]]), &tr(&v));
        for i in 0..3 {
            for j in 0..3 {
                assert!((back[i][j] - m[i][j]).abs() < 1e-9);
            }
        }
        assert!(s[0] >= s[1] && s[1] >= s[2]);
    }

    #[test]
    fn cholesky_solves() {
        let a = [4.0, 1.0, 0.5, 1.0, 3.0, 0.2, 0.5, 0.2, 2.0];
        let b = [1.0, 2.0, 3.0];
        let x = cholesky_solve(&a, &b, 3).unwrap();
        for i in 0..3 {
            let s: f64 = (0..3).map(|j| a[i * 3 + j] * x[j]).sum();
            assert!((s - b[i]).abs() < 1e-12);
        }
    }
}
