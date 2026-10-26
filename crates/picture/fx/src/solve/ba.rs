// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Bundle adjustment: Levenberg–Marquardt over camera poses, points and
//! (optionally) the focal length, with the points eliminated by the Schur
//! complement and a Huber loss against bad tracks.

use super::linalg::{self as la, M3, V3};
use super::Pose;

/// One observation: camera index, point index, measured position
/// (pixels from the picture centre).
#[derive(Clone, Copy, Debug)]
pub struct Obs {
    pub cam: usize,
    pub point: usize,
    pub uv: [f64; 2],
}

/// What may move.
#[derive(Clone, Copy, Debug)]
pub struct Free {
    /// Cameras before this index stay put (the gauge).
    pub first_free_cam: usize,
    pub focal: bool,
    /// Cameras only turn (nodal pan/tilt): their centres stay put.
    pub rotation_only: bool,
}

/// Projection of camera-space point `xc` with focal `f`.
#[inline]
pub fn project(f: f64, xc: V3) -> Option<[f64; 2]> {
    (xc[2] > 1e-9).then(|| [f * xc[0] / xc[2], f * xc[1] / xc[2]])
}

fn huber(r2: f64, delta: f64) -> (f64, f64) {
    // (cost, weight)
    let r = r2.sqrt();
    if r <= delta { (r2, 1.0) } else { (2.0 * delta * r - delta * delta, delta / r) }
}

/// Total robust cost.
pub fn cost(cams: &[Pose], pts: &[V3], f: f64, obs: &[Obs], delta: f64) -> f64 {
    obs.iter()
        .map(|o| {
            let xc = cams[o.cam].apply(pts[o.point]);
            match project(f, xc) {
                Some(p) => huber((p[0] - o.uv[0]).powi(2) + (p[1] - o.uv[1]).powi(2), delta).0,
                None => delta * delta * 100.0,
            }
        })
        .sum()
}

/// Adjusts `cams`, `pts` and `f` in place. Returns the final RMS error in
/// pixels.
pub fn adjust(cams: &mut [Pose], pts: &mut [V3], f: &mut f64, obs: &[Obs], free: Free, iterations: usize, delta: f64) -> f64 {
    let nc = cams.len();
    let np = pts.len();
    let cam_dof = if free.rotation_only { 3 } else { 6 };
    let free_cams: Vec<usize> = (free.first_free_cam..nc).collect();
    let cam_slot: Vec<Option<usize>> = (0..nc).map(|c| free_cams.iter().position(|&x| x == c)).collect();
    let nf = free_cams.len() * cam_dof + usize::from(free.focal);
    let fslot = free_cams.len() * cam_dof;
    let mut by_point: Vec<Vec<usize>> = vec![Vec::new(); np];
    for (k, o) in obs.iter().enumerate() {
        by_point[o.point].push(k);
    }
    let mut lambda = 1e-3;
    let mut cur = cost(cams, pts, *f, obs, delta);
    for _ in 0..iterations {
        // Normal equations.
        let mut u = vec![0.0; nf * nf];
        let mut gc = vec![0.0; nf];
        let mut v = vec![[[0.0; 3]; 3]; np];
        let mut gp = vec![[0.0; 3]; np];
        // W per observation (camera rows × 3) and the focal row.
        let mut wc: Vec<[[f64; 3]; 6]> = vec![[[0.0; 3]; 6]; obs.len()];
        let mut wf: Vec<[f64; 3]> = vec![[0.0; 3]; obs.len()];
        for (k, o) in obs.iter().enumerate() {
            let cam = &cams[o.cam];
            let rx = la::mv(&cam.r, pts[o.point]);
            let xc = la::add(rx, cam.t);
            if xc[2] <= 1e-6 {
                continue;
            }
            let (x, y, z) = (xc[0], xc[1], xc[2]);
            let p = [*f * x / z, *f * y / z];
            let r = [p[0] - o.uv[0], p[1] - o.uv[1]];
            let (_, wt) = huber(r[0] * r[0] + r[1] * r[1], delta);
            // d(uv)/d(xc)
            let dp = [[*f / z, 0.0, -*f * x / (z * z)], [0.0, *f / z, -*f * y / (z * z)]];
            // Camera: the whole pose turned by δω (left-multiplied), then
            // moved by δt: xc' = exp(δω)·xc + δt.
            let sk = la::skew(xc);
            let mut jc = [[0.0f64; 6]; 2];
            for row in 0..2 {
                for c in 0..3 {
                    // dxc/dδω = -[xc]×
                    jc[row][c] = -(0..3).map(|k2| dp[row][k2] * sk[k2][c]).sum::<f64>();
                    jc[row][3 + c] = dp[row][c];
                }
            }
            let jp: [[f64; 3]; 2] = std::array::from_fn(|row| std::array::from_fn(|c| (0..3).map(|k2| dp[row][k2] * cam.r[k2][c]).sum()));
            let jf = [x / z, y / z];
            let i = o.point;
            for a in 0..3 {
                for b in 0..3 {
                    v[i][a][b] += wt * (jp[0][a] * jp[0][b] + jp[1][a] * jp[1][b]);
                }
                gp[i][a] += wt * (jp[0][a] * r[0] + jp[1][a] * r[1]);
            }
            if let Some(s) = cam_slot[o.cam] {
                let base = s * cam_dof;
                for a in 0..cam_dof {
                    for b in 0..cam_dof {
                        u[(base + a) * nf + base + b] += wt * (jc[0][a] * jc[0][b] + jc[1][a] * jc[1][b]);
                    }
                    gc[base + a] += wt * (jc[0][a] * r[0] + jc[1][a] * r[1]);
                    for c in 0..3 {
                        wc[k][a][c] = wt * (jc[0][a] * jp[0][c] + jc[1][a] * jp[1][c]);
                    }
                }
                if free.focal {
                    for a in 0..cam_dof {
                        let val = wt * (jc[0][a] * jf[0] + jc[1][a] * jf[1]);
                        u[(base + a) * nf + fslot] += val;
                        u[fslot * nf + base + a] += val;
                    }
                }
            }
            if free.focal {
                u[fslot * nf + fslot] += wt * (jf[0] * jf[0] + jf[1] * jf[1]);
                gc[fslot] += wt * (jf[0] * r[0] + jf[1] * r[1]);
                for c in 0..3 {
                    wf[k][c] = wt * (jf[0] * jp[0][c] + jf[1] * jp[1][c]);
                }
            }
        }
        // LM loop: try steps with growing damping until the cost drops.
        let mut improved = false;
        for _try in 0..8 {
            let mut s = u.clone();
            for d in 0..nf {
                s[d * nf + d] *= 1.0 + lambda;
                s[d * nf + d] += 1e-9;
            }
            let mut rhs: Vec<f64> = gc.iter().map(|g| -g).collect();
            let mut vinv = vec![[[0.0; 3]; 3]; np];
            for i in 0..np {
                let mut vi = v[i];
                for d in 0..3 {
                    vi[d][d] = vi[d][d] * (1.0 + lambda) + 1e-9;
                }
                let Some(inv) = la::inv3(&vi) else { continue };
                vinv[i] = inv;
                // Rows of W for this point: (slot index, 3-vector).
                let rows: Vec<(usize, [f64; 3])> = by_point[i]
                    .iter()
                    .flat_map(|&k| {
                        let o = obs[k];
                        let mut r = Vec::new();
                        if let Some(sl) = cam_slot[o.cam] {
                            for a in 0..cam_dof {
                                r.push((sl * cam_dof + a, wc[k][a]));
                            }
                        }
                        if free.focal {
                            r.push((fslot, wf[k]));
                        }
                        r
                    })
                    .collect();
                // Merge rows sharing a slot (the focal row repeats).
                let mut merged: Vec<(usize, [f64; 3])> = Vec::with_capacity(rows.len());
                for (sl, w) in rows {
                    match merged.iter_mut().find(|m| m.0 == sl) {
                        Some(m) => {
                            for c in 0..3 {
                                m.1[c] += w[c];
                            }
                        }
                        None => merged.push((sl, w)),
                    }
                }
                let wv: Vec<(usize, [f64; 3])> = merged.iter().map(|(sl, w)| (*sl, std::array::from_fn(|c| (0..3).map(|k2| w[k2] * inv[k2][c]).sum()))).collect();
                for (a, wva) in &wv {
                    for (b, wb) in &merged {
                        s[a * nf + b] -= wva[0] * wb[0] + wva[1] * wb[1] + wva[2] * wb[2];
                    }
                    rhs[*a] += wva[0] * gp[i][0] + wva[1] * gp[i][1] + wva[2] * gp[i][2];
                }
            }
            let dc = if nf > 0 {
                match la::cholesky_solve(&s, &rhs, nf) {
                    Some(d) => d,
                    None => {
                        lambda *= 10.0;
                        continue;
                    }
                }
            } else {
                Vec::new()
            };
            // Back-substitute points.
            let mut new_pts = pts.to_vec();
            for i in 0..np {
                let mut b = [-gp[i][0], -gp[i][1], -gp[i][2]];
                for &k in &by_point[i] {
                    let o = obs[k];
                    if let Some(sl) = cam_slot[o.cam] {
                        for a in 0..cam_dof {
                            for c in 0..3 {
                                b[c] -= wc[k][a][c] * dc[sl * cam_dof + a];
                            }
                        }
                    }
                    if free.focal {
                        for c in 0..3 {
                            b[c] -= wf[k][c] * dc[fslot];
                        }
                    }
                }
                let d = la::mv(&vinv[i], b);
                new_pts[i] = la::add(pts[i], d);
            }
            let mut new_cams = cams.to_vec();
            for (s_i, &c) in free_cams.iter().enumerate() {
                let base = s_i * cam_dof;
                let dw = [dc[base], dc[base + 1], dc[base + 2]];
                let r = la::mul(&la::rodrigues(dw), &cams[c].r);
                // Rotating about the camera centre keeps the centre when
                // only rotation is free.
                // Turning the whole pose keeps the centre when only rotation is
                // free (t' = exp(δω)·t).
                let turned = la::mv(&la::rodrigues(dw), cams[c].t);
                let t = if free.rotation_only { turned } else { la::add(turned, [dc[base + 3], dc[base + 4], dc[base + 5]]) };
                new_cams[c] = Pose { r, t };
            }
            let new_f = if free.focal { (*f + dc[fslot]).max(1.0) } else { *f };
            let c2 = cost(&new_cams, &new_pts, new_f, obs, delta);
            if c2 < cur {
                cams.copy_from_slice(&new_cams);
                pts.copy_from_slice(&new_pts);
                *f = new_f;
                let rel = (cur - c2) / cur.max(1e-12);
                cur = c2;
                lambda = (lambda * 0.3).max(1e-9);
                improved = rel > 1e-7;
                break;
            }
            lambda *= 10.0;
        }
        if !improved {
            break;
        }
    }
    rms(cams, pts, *f, obs)
}

/// Root-mean-square reprojection error, pixels.
pub fn rms(cams: &[Pose], pts: &[V3], f: f64, obs: &[Obs]) -> f64 {
    if obs.is_empty() {
        return 0.0;
    }
    let s: f64 = obs
        .iter()
        .map(|o| match project(f, cams[o.cam].apply(pts[o.point])) {
            Some(p) => (p[0] - o.uv[0]).powi(2) + (p[1] - o.uv[1]).powi(2),
            None => 1e4,
        })
        .sum();
    (s / obs.len() as f64).sqrt()
}

/// Refines one camera pose against fixed points (perspective-n-point).
pub fn refine_pose(pose: &mut Pose, f: f64, pts: &[(V3, [f64; 2])], iterations: usize, delta: f64, rotation_only: bool) -> f64 {
    if pts.is_empty() {
        return f64::INFINITY;
    }
    let mut cams = vec![*pose];
    let points: Vec<V3> = pts.iter().map(|p| p.0).collect();
    let obs: Vec<Obs> = pts.iter().enumerate().map(|(i, p)| Obs { cam: 0, point: i, uv: p.1 }).collect();
    let mut cur = cost(&cams, &points, f, &obs, delta);
    let mut lambda = 1e-3;
    let dof = if rotation_only { 3 } else { 6 };
    for _ in 0..iterations {
        let mut h = [[0.0f64; 6]; 6];
        let mut g = [0.0f64; 6];
        let cam = cams[0];
        for o in &obs {
            let rx = la::mv(&cam.r, points[o.point]);
            let xc = la::add(rx, cam.t);
            if xc[2] <= 1e-6 {
                continue;
            }
            let (x, y, z) = (xc[0], xc[1], xc[2]);
            let r = [f * x / z - o.uv[0], f * y / z - o.uv[1]];
            let (_, wt) = huber(r[0] * r[0] + r[1] * r[1], delta);
            let dp = [[f / z, 0.0, -f * x / (z * z)], [0.0, f / z, -f * y / (z * z)]];
            let sk = la::skew(xc);
            let mut j = [[0.0f64; 6]; 2];
            for row in 0..2 {
                for c in 0..3 {
                    j[row][c] = -(0..3).map(|k| dp[row][k] * sk[k][c]).sum::<f64>();
                    j[row][3 + c] = dp[row][c];
                }
            }
            for a in 0..dof {
                for b in 0..dof {
                    h[a][b] += wt * (j[0][a] * j[0][b] + j[1][a] * j[1][b]);
                }
                g[a] += wt * (j[0][a] * r[0] + j[1][a] * r[1]);
            }
        }
        let mut ok = false;
        for _ in 0..8 {
            let mut a = vec![0.0; dof * dof];
            for i in 0..dof {
                for k in 0..dof {
                    a[i * dof + k] = h[i][k];
                }
                a[i * dof + i] = h[i][i] * (1.0 + lambda) + 1e-9;
            }
            let b: Vec<f64> = g[..dof].iter().map(|v| -v).collect();
            let Some(d) = la::cholesky_solve(&a, &b, dof) else {
                lambda *= 10.0;
                continue;
            };
            let dw = [d[0], d[1], d[2]];
            let rr = la::mul(&la::rodrigues(dw), &cam.r);
            let turned = la::mv(&la::rodrigues(dw), cam.t);
            let t = if rotation_only { turned } else { la::add(turned, [d[3], d[4], d[5]]) };
            let cand = [Pose { r: rr, t }];
            let c2 = cost(&cand, &points, f, &obs, delta);
            if c2 < cur {
                let rel = (cur - c2) / cur.max(1e-12);
                cams[0] = cand[0];
                cur = c2;
                lambda = (lambda * 0.3).max(1e-9);
                ok = rel > 1e-8;
                break;
            }
            lambda *= 10.0;
        }
        if !ok {
            break;
        }
    }
    *pose = cams[0];
    rms(&cams, &points, f, &obs)
}

/// Triangulates a point from two or more views (linear, homogeneous).
pub fn triangulate(views: &[(Pose, [f64; 2])], f: f64) -> Option<V3> {
    if views.len() < 2 {
        return None;
    }
    let mut rows = Vec::with_capacity(views.len() * 2);
    for (pose, uv) in views {
        let (x, y) = (uv[0] / f, uv[1] / f);
        let p: [[f64; 4]; 3] = std::array::from_fn(|r| [pose.r[r][0], pose.r[r][1], pose.r[r][2], pose.t[r]]);
        rows.push((0..4).map(|c| x * p[2][c] - p[0][c]).collect::<Vec<f64>>());
        rows.push((0..4).map(|c| y * p[2][c] - p[1][c]).collect::<Vec<f64>>());
    }
    let h = la::null_vector(&rows);
    if h[3].abs() < 1e-12 {
        return None;
    }
    let x = [h[0] / h[3], h[1] / h[3], h[2] / h[3]];
    // In front of every camera.
    views.iter().all(|(p, _)| p.apply(x)[2] > 0.0).then_some(x)
}

/// The angle (radians) between the rays from two camera centres to `x`.
pub fn parallax(a: &Pose, b: &Pose, x: V3) -> f64 {
    let (ra, rb) = (la::unit(la::sub(x, a.centre())), la::unit(la::sub(x, b.centre())));
    la::dot(ra, rb).clamp(-1.0, 1.0).acos()
}

#[allow(dead_code)]
fn _assert_m3(_: M3) {}
