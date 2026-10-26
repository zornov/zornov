// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A solved camera in Studio's world, and in and out of other programs.
//!
//! Studio's world: x right, y down, z away from the viewer, in canvas
//! pixels; a camera's rotation is X, then Y, then Z degrees (see
//! `studio_model::space`). [`orient`] levels the ground plane (y = 0 is
//! the ground, positive y below it), puts the origin where the first frame
//! looks at the ground, and scales the scene so the first camera stands
//! the default camera distance from it, so a canvas-sized layer at the
//! origin looks canvas-sized.
//!
//! Exchange formats: Nuke `.chan` (frame, translate, rotate in ZXY order,
//! vertical field of view; Y up, the camera looks down −Z), Studio's JSON,
//! and the point cloud as OBJ vertices.

use serde::{Deserialize, Serialize};

use super::linalg::{self as la, M3, V3};
use super::{Pose, Solved};

/// Where the solve sits in the world.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Frame3 {
    /// Rows: world x, y, z axes in solve coordinates.
    pub axes: M3,
    /// Solve-space origin of the world.
    pub origin: V3,
    /// World units per solve unit.
    pub scale: f64,
}

impl Frame3 {
    pub fn to_world(&self, x: V3) -> V3 {
        la::scale(la::mv(&self.axes, la::sub(x, self.origin)), self.scale)
    }

    /// A solve pose as a world pose.
    pub fn pose(&self, p: &Pose) -> Pose {
        let r = la::mul(&p.r, &la::tr(&self.axes));
        Pose::from_centre(r, self.to_world(p.centre()))
    }
}

/// How to place the scene.
#[derive(Clone, Debug, Default)]
pub struct OrientSettings {
    /// Track ids that lie on the ground (three or more); found by a plane
    /// fit when empty.
    pub ground: Vec<usize>,
    /// A track to put at the origin.
    pub origin: Option<usize>,
    /// Two tracks and their real distance (canvas pixels), for scale.
    pub distance: Option<(usize, usize, f64)>,
    /// Width of the canvas the camera is used on (for the default scale).
    pub canvas_width: f64,
}

fn plane_fit(pts: &[V3]) -> Option<(V3, V3)> {
    // Normal = eigenvector of the smallest eigenvalue of the covariance.
    if pts.len() < 3 {
        return None;
    }
    let n = pts.len() as f64;
    let c = la::scale(pts.iter().fold([0.0; 3], |a, p| la::add(a, *p)), 1.0 / n);
    let mut cov = [0.0; 9];
    for p in pts {
        let d = la::sub(*p, c);
        for i in 0..3 {
            for j in 0..3 {
                cov[i * 3 + j] += d[i] * d[j];
            }
        }
    }
    let (_, v) = la::sym_eigen(&cov, 3);
    Some((c, la::unit([v[0][0], v[0][1], v[0][2]])))
}

/// Finds the ground: the plane with the most points facing roughly the
/// first camera's down axis.
fn find_ground(s: &Solved, first: &Pose) -> Option<(V3, V3)> {
    let pts: Vec<V3> = s.points.iter().map(|p| p.position).collect();
    if pts.len() < 12 {
        return None;
    }
    let down = [first.r[1][0], first.r[1][1], first.r[1][2]];
    let spread = {
        let c = la::scale(pts.iter().fold([0.0; 3], |a, p| la::add(a, *p)), 1.0 / pts.len() as f64);
        let mut d: Vec<f64> = pts.iter().map(|p| la::norm(la::sub(*p, c))).collect();
        d.sort_by(f64::total_cmp);
        d[d.len() / 2]
    };
    let thr = spread * 0.04;
    let mut rng = crate::math::Rng::new(17);
    let mut best: Option<(V3, V3, usize)> = None;
    for _ in 0..600 {
        let (a, b, c) = (pts[rng.below(pts.len())], pts[rng.below(pts.len())], pts[rng.below(pts.len())]);
        let n = la::cross(la::sub(b, a), la::sub(c, a));
        if la::norm(n) < 1e-12 {
            continue;
        }
        let n = la::unit(n);
        // Ground-like: within 50° of the camera's down axis.
        if la::dot(n, down).abs() < 50f64.to_radians().cos() {
            continue;
        }
        let count = pts.iter().filter(|p| la::dot(la::sub(**p, a), n).abs() < thr).count();
        if best.as_ref().is_none_or(|b| count > b.2) {
            best = Some((a, n, count));
        }
    }
    let (a, n, count) = best?;
    if count < 8 {
        return None;
    }
    let inl: Vec<V3> = pts.iter().copied().filter(|p| la::dot(la::sub(*p, a), n).abs() < thr).collect();
    plane_fit(&inl)
}

/// Places a solve in Studio's world.
pub fn orient(s: &Solved, settings: &OrientSettings) -> Frame3 {
    let first = s.poses.iter().flatten().next().copied().unwrap_or(Pose::IDENTITY);
    let cam_c = first.centre();
    let fwd = [first.r[2][0], first.r[2][1], first.r[2][2]];
    let down_cam = [first.r[1][0], first.r[1][1], first.r[1][2]];
    let by_id = |id: usize| s.points.iter().find(|p| p.track == id).map(|p| p.position);
    let ground = if settings.ground.len() >= 3 {
        plane_fit(&settings.ground.iter().filter_map(|&i| by_id(i)).collect::<Vec<_>>())
    } else if s.nodal {
        None
    } else {
        find_ground(s, &first)
    };
    let (origin, down) = match ground {
        Some((c, n)) => {
            // Down points away from the camera's side of the ground.
            let n = if la::dot(la::sub(cam_c, c), n) > 0.0 { la::scale(n, -1.0) } else { n };
            // Origin: where the first view's centre ray meets the ground,
            // else the points' centre on it.
            let denom = la::dot(fwd, n);
            let hit = if denom.abs() > 1e-6 {
                let t = la::dot(la::sub(c, cam_c), n) / denom;
                (t > 0.0).then(|| la::add(cam_c, la::scale(fwd, t)))
            } else {
                None
            };
            let o = hit.filter(|h| la::norm(la::sub(*h, cam_c)) < 20.0 * la::norm(la::sub(c, cam_c)).max(1e-9)).unwrap_or(c);
            (o, n)
        }
        None => {
            // No ground: keep the first camera level, origin at the median
            // point depth straight ahead.
            let mut d: Vec<f64> = s.points.iter().map(|p| la::dot(la::sub(p.position, cam_c), fwd)).filter(|d| *d > 0.0).collect();
            d.sort_by(f64::total_cmp);
            let depth = d.get(d.len() / 2).copied().unwrap_or(1.0);
            (la::add(cam_c, la::scale(fwd, depth)), down_cam)
        }
    };
    let origin = settings.origin.and_then(by_id).unwrap_or(origin);
    // Forward: the first camera's view direction laid flat.
    let mut z = la::sub(fwd, la::scale(down, la::dot(fwd, down)));
    if la::norm(z) < 1e-6 {
        z = la::sub(down_cam, la::scale(down, la::dot(down_cam, down)));
    }
    let z = la::unit(z);
    let x = la::unit(la::cross(down, z));
    let axes = [x, down, z];
    let dist = la::norm(la::sub(cam_c, origin)).max(1e-9);
    let scale = match settings.distance.and_then(|(a, b, d)| Some((by_id(a)?, by_id(b)?, d))) {
        Some((a, b, d)) if la::norm(la::sub(a, b)) > 1e-12 => d / la::norm(la::sub(a, b)),
        _ => settings.canvas_width.max(1.0) * 1.388_888_9 / dist,
    };
    Frame3 { axes, origin, scale }
}

// ------------------------------------------------- Studio camera rotations

fn rot_x(deg: f64) -> M3 {
    let (s, c) = deg.to_radians().sin_cos();
    [[1.0, 0.0, 0.0], [0.0, c, s], [0.0, -s, c]]
}
fn rot_y(deg: f64) -> M3 {
    let (s, c) = deg.to_radians().sin_cos();
    [[c, 0.0, -s], [0.0, 1.0, 0.0], [s, 0.0, c]]
}
fn rot_z(deg: f64) -> M3 {
    let (s, c) = deg.to_radians().sin_cos();
    [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]
}

/// Studio's layer/camera rotation from X, Y, Z degrees (no orientation):
/// the same as `studio_model::space::rotation([0; 3], x, y, z)`.
pub fn studio_rotation(x: f64, y: f64, z: f64) -> M3 {
    la::mul(&la::mul(&rot_x(x), &rot_y(y)), &rot_z(z))
}

/// X, Y, Z degrees of a Studio rotation (as `studio_model::space::euler`).
pub fn studio_euler(m: &M3) -> [f64; 3] {
    let sy = (-m[0][2]).clamp(-1.0, 1.0);
    let ry = sy.asin();
    if sy.abs() < 0.999_999 {
        let rx = m[1][2].atan2(m[2][2]);
        let rz = m[0][1].atan2(m[0][0]);
        [rx.to_degrees(), ry.to_degrees(), (-rz).to_degrees()]
    } else {
        let rx = (-m[2][1]).atan2(m[1][1]);
        [rx.to_degrees(), ry.to_degrees(), 0.0]
    }
}

/// One frame of a Studio camera: position, rotation (X, Y, Z degrees) and
/// zoom (pixels).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CameraKey {
    pub frame: usize,
    pub position: V3,
    pub rotation: [f64; 3],
    pub zoom: f64,
}

/// A camera's per-frame keys in the world (Studio camera axes are the rows
/// of `pose.r`, its rotation matrix is their transpose).
pub fn camera_keys(s: &Solved, frame3: &Frame3, zoom_scale: f64) -> Vec<CameraKey> {
    let mut out = Vec::new();
    let mut prev: Option<[f64; 3]> = None;
    for (f, p) in s.poses.iter().enumerate() {
        let Some(p) = p else { continue };
        let w = frame3.pose(p);
        let mut rot = studio_euler(&la::tr(&w.r));
        // Unwrap so keys do not spin the long way round between frames.
        if let Some(pr) = prev {
            for i in 0..3 {
                while rot[i] - pr[i] > 180.0 {
                    rot[i] -= 360.0;
                }
                while rot[i] - pr[i] < -180.0 {
                    rot[i] += 360.0;
                }
            }
        }
        prev = Some(rot);
        out.push(CameraKey { frame: f, position: w.centre(), rotation: rot, zoom: s.focal * zoom_scale });
    }
    out
}

/// The world pose of a Studio camera key.
pub fn key_pose(k: &CameraKey) -> Pose {
    let r = la::tr(&studio_rotation(k.rotation[0], k.rotation[1], k.rotation[2]));
    Pose::from_centre(r, k.position)
}

// ------------------------------------------------------------------ Nuke chan

/// Studio world → Nuke world: y and z flip (a half turn about x).
const FLIP: M3 = [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]];

/// Nuke's ZXY rotation order: `R = Ry · Rx · Rz` (right-handed, degrees).
fn nuke_rot(rx: f64, ry: f64, rz: f64) -> M3 {
    let (sx, cx) = rx.to_radians().sin_cos();
    let (sy, cy) = ry.to_radians().sin_cos();
    let (sz, cz) = rz.to_radians().sin_cos();
    let mx = [[1.0, 0.0, 0.0], [0.0, cx, -sx], [0.0, sx, cx]];
    let my = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
    let mz = [[cz, -sz, 0.0], [sz, cz, 0.0], [0.0, 0.0, 1.0]];
    la::mul(&la::mul(&my, &mx), &mz)
}

fn nuke_euler(r: &M3) -> [f64; 3] {
    // R = Ry Rx Rz: r[1][2] = -sin(x).
    let sx = (-r[1][2]).clamp(-1.0, 1.0);
    let x = sx.asin();
    if sx.abs() < 0.999_999 {
        let y = r[0][2].atan2(r[2][2]);
        let z = r[1][0].atan2(r[1][1]);
        [x.to_degrees(), y.to_degrees(), z.to_degrees()]
    } else {
        let y = (-r[2][0]).atan2(r[0][0]);
        [x.to_degrees(), y.to_degrees(), 0.0]
    }
}

/// Camera keys as a Nuke `.chan` file (frames from `first_frame`), with
/// the vertical field of view of a `height`-pixel picture. Units: Studio
/// pixels times `units` (e.g. 0.01 for a scene in metres-ish).
pub fn to_chan(keys: &[CameraKey], height: f64, first_frame: i64, units: f64) -> String {
    let mut s = String::new();
    for k in keys {
        let pose = key_pose(k);
        // Camera-to-world in Studio (columns right, down, forward), then
        // into Nuke's axes (right, up, back).
        let c2w = la::tr(&pose.r);
        let col = |m: &M3, i: usize| [m[0][i], m[1][i], m[2][i]];
        let right = la::mv(&FLIP, col(&c2w, 0));
        let up = la::mv(&FLIP, la::scale(col(&c2w, 1), -1.0));
        let back = la::mv(&FLIP, la::scale(col(&c2w, 2), -1.0));
        let rn: M3 = std::array::from_fn(|r| [right[r], up[r], back[r]]);
        let e = nuke_euler(&rn);
        let p = la::scale(la::mv(&FLIP, k.position), units);
        let vfov = 2.0 * (height * 0.5 / k.zoom.max(1e-6)).atan().to_degrees();
        s.push_str(&format!(
            "{} {:.6} {:.6} {:.6} {:.6} {:.6} {:.6} {:.6}\n",
            first_frame + k.frame as i64,
            p[0],
            p[1],
            p[2],
            e[0],
            e[1],
            e[2],
            vfov
        ));
    }
    s
}

/// Reads a Nuke `.chan` file back into camera keys (frames counted from
/// the first line). `height` is the picture height for the field of view;
/// a missing field of view uses `default_zoom`.
pub fn from_chan(text: &str, height: f64, units: f64, default_zoom: f64) -> Result<Vec<CameraKey>, String> {
    let mut out = Vec::new();
    let mut first: Option<i64> = None;
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let v: Vec<f64> = line.split_whitespace().map(|t| t.parse::<f64>()).collect::<Result<_, _>>().map_err(|_| format!("line {}: not numbers", n + 1))?;
        if v.len() < 7 {
            return Err(format!("line {}: a .chan line has frame, tx, ty, tz, rx, ry, rz (and fov)", n + 1));
        }
        let frame = v[0].round() as i64;
        let f0 = *first.get_or_insert(frame);
        let rn = nuke_rot(v[4], v[5], v[6]);
        let col = |m: &M3, i: usize| [m[0][i], m[1][i], m[2][i]];
        let right = la::mv(&FLIP, col(&rn, 0));
        let down = la::scale(la::mv(&FLIP, col(&rn, 1)), -1.0);
        let fwd = la::scale(la::mv(&FLIP, col(&rn, 2)), -1.0);
        let axes: M3 = [right, down, fwd];
        let rot = studio_euler(&la::tr(&axes));
        let pos = la::scale(la::mv(&FLIP, [v[1], v[2], v[3]]), 1.0 / units.max(1e-12));
        let zoom = v.get(7).map_or(default_zoom, |vfov| height * 0.5 / (vfov.to_radians() * 0.5).tan().max(1e-9));
        out.push(CameraKey { frame: (frame - f0).max(0) as usize, position: pos, rotation: rot, zoom });
    }
    if out.is_empty() {
        return Err("the .chan file has no frames".into());
    }
    Ok(out)
}

/// A solve as Studio's JSON exchange file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CameraFile {
    pub format: String,
    pub width: f64,
    pub height: f64,
    pub frame_rate: f64,
    pub focal_pixels: f64,
    pub error_pixels: f64,
    pub nodal: bool,
    pub keys: Vec<CameraKey>,
    /// Point cloud in world units: `[x, y, z, track id]`.
    pub points: Vec<[f64; 4]>,
}

/// The point cloud as OBJ vertices (Y up, as most 3D programs read OBJ).
pub fn points_obj(points: &[[f64; 4]], units: f64) -> String {
    let mut s = String::from("# Studio camera solve point cloud\n");
    for p in points {
        let q = la::scale(la::mv(&FLIP, [p[0], p[1], p[2]]), units);
        s.push_str(&format!("v {:.6} {:.6} {:.6}\n", q[0], q[1], q[2]));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn studio_euler_round_trips() {
        for r in [[10.0, 20.0, 30.0], [-40.0, 5.0, 170.0], [0.0, -60.0, 0.0]] {
            let m = studio_rotation(r[0], r[1], r[2]);
            let e = studio_euler(&m);
            let m2 = studio_rotation(e[0], e[1], e[2]);
            for i in 0..3 {
                for j in 0..3 {
                    assert!((m[i][j] - m2[i][j]).abs() < 1e-9);
                }
            }
        }
    }

    #[test]
    fn chan_round_trips() {
        let keys = vec![
            CameraKey { frame: 0, position: [10.0, -50.0, -1200.0], rotation: [5.0, -12.0, 3.0], zoom: 1500.0 },
            CameraKey { frame: 1, position: [14.0, -52.0, -1190.0], rotation: [5.5, -11.0, 2.0], zoom: 1500.0 },
        ];
        let text = to_chan(&keys, 1080.0, 1001, 0.01);
        assert!(text.starts_with("1001 "));
        let back = from_chan(&text, 1080.0, 0.01, 1000.0).unwrap();
        for (a, b) in keys.iter().zip(&back) {
            assert_eq!(a.frame, b.frame);
            for i in 0..3 {
                assert!((a.position[i] - b.position[i]).abs() < 1e-3);
                assert!((a.rotation[i] - b.rotation[i]).abs() < 1e-4, "{:?} {:?}", a.rotation, b.rotation);
            }
            assert!((a.zoom - b.zoom).abs() < 1e-3);
        }
    }

    #[test]
    fn nuke_axes_for_a_level_camera() {
        // A Studio camera at rest looks +z (away); in Nuke it looks -z with
        // no rotation.
        let k = CameraKey { frame: 0, position: [0.0, 0.0, -100.0], rotation: [0.0, 0.0, 0.0], zoom: 1000.0 };
        let line = to_chan(&[k], 1000.0, 1, 1.0);
        let v: Vec<f64> = line.split_whitespace().map(|t| t.parse().unwrap()).collect();
        assert!((v[3] - 100.0).abs() < 1e-9, "{line}");
        assert!(v[4].abs() < 1e-9 && v[5].abs() < 1e-9 && v[6].abs() < 1e-9, "{line}");
        // A Studio camera turned +30° about Y (its right edge away, so it
        // looks left) is Nuke's +30° about Y (also looking left).
        let k = CameraKey { rotation: [0.0, 30.0, 0.0], ..k };
        let line = to_chan(&[k], 1000.0, 1, 1.0);
        let v: Vec<f64> = line.split_whitespace().map(|t| t.parse().unwrap()).collect();
        assert!((v[5] - 30.0).abs() < 1e-6, "{line}");
    }
}
