// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Finding a way on foot: an A* search over a grid laid on the world's
//! floors, where a cell counts only if a standing character fits there
//! (walls and furniture pushed out of, slopes and steps within the
//! character's limits, water no deeper than wading). Floors are kept
//! apart by height, so the way up a staircase, a doorway into a room and
//! round a pond all come out.

use crate::math::V3;
use crate::walk::Params;
use crate::world::World;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

#[derive(Clone, Copy, Debug)]
pub struct NavParams {
    /// Metres per grid cell.
    pub cell: f32,
    /// Search no further than this beyond the box of start and goal.
    pub margin: f32,
    /// Give up after this many cells.
    pub max_cells: usize,
}

impl Default for NavParams {
    fn default() -> NavParams {
        NavParams { cell: 0.5, margin: 25.0, max_cells: 400_000 }
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Item {
    f: f32,
    key: (i32, i32, i32),
}
impl Eq for Item {}
impl Ord for Item {
    fn cmp(&self, o: &Item) -> Ordering {
        o.f.total_cmp(&self.f)
    }
}
impl PartialOrd for Item {
    fn partial_cmp(&self, o: &Item) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

/// The floor a character standing at (x, z) with feet near `y` would be
/// on, if that spot is somewhere to stand.
fn stand(w: &World, p: &Params, x: f32, z: f32, y: f32) -> Option<f32> {
    let g = w.ground_at(x, z, y + p.step + 0.01)?;
    if g.y < y - 1.2 || g.normal[1] < p.max_slope.to_radians().cos() {
        return None;
    }
    let out = w.push_out([x, g.y, z], p.radius, g.y + p.step, g.y + p.height);
    if out[0].abs() > 1e-3 || out[1].abs() > 1e-3 {
        return None;
    }
    // Overhead room to stand.
    if w.ceiling_above(x, z, g.y + 0.2).is_some_and(|c| c < g.y + p.height) {
        return None;
    }
    // Not out of one's depth.
    if let Some(l) = w.water_at(x, z)
        && l - g.y > p.height * 0.55
    {
        return None;
    }
    Some(g.y)
}

/// Whether one can walk the straight line between two standing points.
fn clear_line(w: &World, p: &Params, a: V3, b: V3) -> bool {
    let d = [b[0] - a[0], b[2] - a[2]];
    let l = (d[0] * d[0] + d[1] * d[1]).sqrt();
    let n = (l / 0.25).ceil().max(1.0) as usize;
    let mut y = a[1];
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let (x, z) = (a[0] + d[0] * t, a[2] + d[1] * t);
        match stand(w, p, x, z, y) {
            Some(ny) if (ny - y).abs() <= 0.3 => y = ny,
            _ => return false,
        }
    }
    true
}

/// A walkable path from `from` to `to` (feet positions), as waypoints
/// including the start, smoothed to as few as keep it walkable. `None`
/// when there is no way.
pub fn find_path(w: &World, from: V3, to: V3, p: &Params, np: &NavParams) -> Option<Vec<V3>> {
    let c = np.cell;
    let (min_x, max_x) = (from[0].min(to[0]) - np.margin, from[0].max(to[0]) + np.margin);
    let (min_z, max_z) = (from[2].min(to[2]) - np.margin, from[2].max(to[2]) + np.margin);
    let cell_of = |x: f32, z: f32| (((x - min_x) / c).round() as i32, ((z - min_z) / c).round() as i32);
    let pos_of = |ix: i32, iz: i32| (min_x + ix as f32 * c, min_z + iz as f32 * c);
    let level = |y: f32| (y * 2.0).round() as i32;
    let start_y = stand(w, p, from[0], from[2], from[1] + 0.2).unwrap_or(from[1]);
    let goal_y = stand(w, p, to[0], to[2], to[1] + 0.2).unwrap_or(to[1]);
    let (sx, sz) = cell_of(from[0], from[2]);
    let (gx, gz) = cell_of(to[0], to[2]);
    let start = (sx, sz, level(start_y));
    let goal_xz = (gx, gz);
    let mut open = BinaryHeap::new();
    let mut best: HashMap<(i32, i32, i32), (f32, f32, (i32, i32, i32))> = HashMap::new();
    best.insert(start, (0.0, start_y, start));
    open.push(Item { f: 0.0, key: start });
    let h = |ix: i32, iz: i32| (((ix - gx) as f32).powi(2) + ((iz - gz) as f32).powi(2)).sqrt() * c;
    let mut closed = 0usize;
    let mut found: Option<(i32, i32, i32)> = None;
    while let Some(Item { key, .. }) = open.pop() {
        let (g, y, _) = best[&key];
        let (ix, iz, _) = key;
        // Near enough to the goal cell (and its floor).
        if (ix - gx).abs() <= 1 && (iz - gz).abs() <= 1 && (y - goal_y).abs() < 0.9 {
            found = Some(key);
            break;
        }
        closed += 1;
        if closed > np.max_cells {
            break;
        }
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
            let (nx, nz) = (ix + dx, iz + dz);
            let (x, z) = pos_of(nx, nz);
            if x < min_x - c || x > max_x + c || z < min_z - c || z > max_z + c {
                continue;
            }
            let Some(ny) = stand(w, p, x, z, y) else { continue };
            // A diagonal must not cut a blocked corner.
            if dx != 0 && dz != 0 {
                let (ax, az) = pos_of(ix + dx, iz);
                let (bx, bz) = pos_of(ix, iz + dz);
                if stand(w, p, ax, az, y).is_none() || stand(w, p, bx, bz, y).is_none() {
                    continue;
                }
            }
            let nk = (nx, nz, level(ny));
            let step = if dx != 0 && dz != 0 { 1.414 } else { 1.0 } * c;
            let wet = w.water_at(x, z).map_or(0.0, |l| (l - ny).max(0.0)) * 2.0;
            let cost = g + step + (ny - y).abs() * 1.5 + wet;
            if best.get(&nk).is_none_or(|b| cost < b.0) {
                best.insert(nk, (cost, ny, key));
                open.push(Item { f: cost + h(nx, nz), key: nk });
            }
        }
    }
    let end = found?;
    let mut path = vec![[to[0], goal_y, to[2]]];
    let mut k = end;
    loop {
        let (_, y, parent) = best[&k];
        let (x, z) = pos_of(k.0, k.1);
        if k == start {
            path.push([from[0], start_y, from[2]]);
            break;
        }
        path.push([x, y, z]);
        k = parent;
    }
    path.reverse();
    let _ = goal_xz;
    // String pulling: skip ahead while the straight line stays walkable.
    let mut out = vec![path[0]];
    let mut i = 0;
    while i + 1 < path.len() {
        let mut j = path.len() - 1;
        while j > i + 1 && !clear_line(w, p, path[i], path[j]) {
            j -= 1;
        }
        out.push(path[j]);
        i = j;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collide::Solid;
    use crate::terrain::Terrain;

    fn field() -> World {
        let mut w = World::new("t");
        w.terrain = Some(Terrain::flat([200.0, 200.0], 1.0));
        w
    }

    #[test]
    fn a_straight_way_is_two_points_and_a_wall_is_gone_round() {
        let mut w = field();
        w.extra_solids.push(Solid::Box { centre: [0.0, 1.5, 10.0], half: [8.0, 1.5, 0.3], yaw: 0.0 });
        w.finish();
        let p = Params::default();
        let n = NavParams::default();
        let open = find_path(&w, [30.0, 0.0, 0.0], [30.0, 0.0, 20.0], &p, &n).unwrap();
        assert_eq!(open.len(), 2, "{open:?}");
        let round = find_path(&w, [0.0, 0.0, 0.0], [0.0, 0.0, 20.0], &p, &n).unwrap();
        assert!(round.len() >= 3, "{round:?}");
        // Every leg is walkable and the way clears the wall's end.
        let widest = round.iter().map(|q| q[0].abs()).fold(0.0f32, f32::max);
        assert!(widest > 8.0, "{widest}");
        for pair in round.windows(2) {
            assert!(clear_line(&w, &p, pair[0], pair[1]));
        }
    }

    #[test]
    fn it_finds_the_door_and_the_stairs_and_says_no_when_there_is_no_way() {
        let (b, solids_only) = (crate::kit::house(8.0, 6.0, 2, 3), ());
        let _ = solids_only;
        let mut w = field();
        w.extra_solids = b.solids;
        w.finish();
        let p = Params::default();
        let n = NavParams::default();
        // From the street to the first floor: through the door and up the stairs.
        let path = find_path(&w, [0.0, 0.0, -12.0], [-2.85, 3.0, 2.3], &p, &n).expect("a way in");
        let last = path.last().unwrap();
        assert!((last[1] - 3.0).abs() < 0.3, "ends upstairs: {last:?}");
        // The way goes through the front wall at the door (a gap).
        let enters = path.windows(2).any(|s| s[0][2] < -3.2 && s[1][2] >= -3.05);
        assert!(enters, "{path:?}");
        // A box of walls with no door: no way.
        let mut sealed = field();
        for (c, h) in [([0.0, 1.5, -5.0], [5.0, 1.5, 0.25]), ([0.0, 1.5, 5.0], [5.0, 1.5, 0.25]), ([-5.0, 1.5, 0.0], [0.25, 1.5, 5.0]), ([5.0, 1.5, 0.0], [0.25, 1.5, 5.0])] {
            sealed.extra_solids.push(Solid::Box { centre: c, half: h, yaw: 0.0 });
        }
        sealed.finish();
        assert!(find_path(&sealed, [0.0, 0.0, -20.0], [0.0, 0.0, 0.0], &p, &NavParams { max_cells: 30_000, ..n }).is_none());
    }
}
