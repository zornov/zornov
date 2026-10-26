// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Deciding what to draw: frustum culling of terrain chunks, objects and
//! cells of scattered copies; a level of detail for each by distance;
//! and thinning of far grass and flowers by a fixed per-copy rank so
//! nothing flickers. The result is a list of batches, one per (asset,
//! level), each holding the raw instance data for one instanced draw.

use crate::math::{Frustum, V3, dist};
use crate::world::World;

/// How to cull.
#[derive(Clone, Copy, Debug)]
pub struct CullParams {
    /// Where the viewer is (levels of detail and thinning follow it).
    pub eye: V3,
    /// What is seen: the camera's view, or a light's box for shadows.
    pub frustum: Frustum,
    /// Scales the distances where levels change (1 normal, <1 sooner).
    pub lod_bias: f32,
    /// 0..1 multiplier on how many far copies are kept.
    pub density: f32,
    /// Terrain levels begin at these distances.
    pub terrain_lod: [f32; 3],
    /// Seconds into the shot (where performing characters are).
    pub time: f32,
}

/// Raw instances of one asset at one level.
#[derive(Clone, Debug, Default)]
pub struct Batch {
    pub asset: usize,
    pub lod: usize,
    pub instances: Vec<[f32; 8]>,
}

#[derive(Clone, Debug, Default)]
pub struct DrawList {
    pub batches: Vec<Batch>,
    /// Terrain chunks to draw: (column, row, level).
    pub chunks: Vec<(usize, usize, usize)>,
    /// Characters in view that act, as they are now.
    pub performers: Vec<crate::world::Performer>,
    pub stats: CullStats,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CullStats {
    pub considered: usize,
    pub drawn: usize,
    pub triangles: usize,
    pub chunks: usize,
}

/// How much of the far crowd stays at `d`: all inside 60 % of the asset's
/// range, none at the range.
pub fn density_at(d: f32, range: f32) -> f32 {
    if range <= 0.0 {
        return 1.0;
    }
    let (a, b) = (range * 0.6, range);
    if d <= a { 1.0 } else if d >= b { 0.0 } else { 1.0 - (d - a) / (b - a) }
}

fn batch_for(list: &mut DrawList, index: &mut std::collections::HashMap<(usize, usize), usize>, asset: usize, lod: usize) -> usize {
    *index.entry((asset, lod)).or_insert_with(|| {
        list.batches.push(Batch { asset, lod, instances: Vec::new() });
        list.batches.len() - 1
    })
}

/// Everything of `w` that `p` says is worth drawing.
pub fn cull(w: &World, p: &CullParams) -> DrawList {
    let mut list = DrawList::default();
    let mut index = std::collections::HashMap::new();
    if let Some(t) = &w.terrain {
        let [cx, cz] = t.chunks();
        for j in 0..cz {
            for i in 0..cx {
                let b = t.chunk_bounds(i, j);
                list.stats.considered += 1;
                if !p.frustum.aabb(&b) {
                    continue;
                }
                // Distance to the nearest point of the chunk.
                let near = [p.eye[0].clamp(b.min[0], b.max[0]), p.eye[1].clamp(b.min[1], b.max[1]), p.eye[2].clamp(b.min[2], b.max[2])];
                let lod = t.lod_for(dist(near, p.eye) / p.lod_bias.max(0.05), &p.terrain_lod);
                list.chunks.push((i, j, lod));
                list.stats.chunks += 1;
                list.stats.triangles += (crate::terrain::CHUNK / crate::terrain::LOD_STRIDES[lod]).pow(2) * 2;
            }
        }
    }
    for (oi, o) in w.objects.iter().enumerate() {
        let a = &w.assets[o.asset];
        list.stats.considered += 1;
        if w.is_performer(o)
            && let Some(pf) = w.performer_state(oi, p.time)
        {
            let b = a.bounds.placed(pf.inst.pos, pf.inst.yaw, pf.inst.scale);
            if p.frustum.aabb(&b) {
                list.stats.drawn += 1;
                list.stats.triangles += a.lods[0].triangles();
                list.performers.push(pf);
            }
            continue;
        }
        let b = a.bounds.placed(o.inst.pos, o.inst.yaw, o.inst.scale);
        if !p.frustum.aabb(&b) {
            continue;
        }
        let d = dist(b.centre(), p.eye);
        if a.cull_distance > 0.0 && d > a.cull_distance * p.lod_bias.max(0.05) {
            continue;
        }
        let lod = a.lod_at(d / p.lod_bias.max(0.05));
        let k = batch_for(&mut list, &mut index, o.asset, lod);
        list.batches[k].instances.push(o.inst.raw());
        list.stats.drawn += 1;
        list.stats.triangles += a.lods[lod].triangles();
    }
    for s in &w.scatters {
        let a = &w.assets[s.asset];
        let range = a.cull_distance * p.lod_bias.max(0.05);
        let g = &s.grid;
        for cell in &g.cells {
            if cell.1 == 0 {
                continue;
            }
            list.stats.considered += cell.1 as usize;
            if !p.frustum.aabb(&cell.2) {
                continue;
            }
            // Nearest the cell comes to the viewer.
            let near = [p.eye[0].clamp(cell.2.min[0], cell.2.max[0]), p.eye[1].clamp(cell.2.min[1], cell.2.max[1]), p.eye[2].clamp(cell.2.min[2], cell.2.max[2])];
            let dn = dist(near, p.eye);
            if range > 0.0 && dn > range {
                continue;
            }
            for k in cell.0..cell.0 + cell.1 {
                let inst = &s.instances[g.order[k as usize] as usize];
                let d = dist(inst.pos, p.eye);
                if range > 0.0 {
                    if d > range {
                        continue;
                    }
                    let keep = density_at(d, range) * p.density;
                    if inst.rank as f32 / 255.0 >= keep {
                        continue;
                    }
                }
                let r = a.bounds.radius() * inst.scale;
                if !p.frustum.sphere([inst.pos[0], inst.pos[1] + a.bounds.centre()[1] * inst.scale, inst.pos[2]], r) {
                    continue;
                }
                let lod = a.lod_at(d / p.lod_bias.max(0.05));
                let bi = batch_for(&mut list, &mut index, s.asset, lod);
                list.batches[bi].instances.push(inst.raw());
                list.stats.drawn += 1;
                list.stats.triangles += a.lods[lod].triangles();
            }
        }
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::Asset;
    use crate::math::{mat_mul, look_to, perspective};
    use crate::terrain::Terrain;
    use crate::world::Instance;

    fn params(eye: V3, forward: V3) -> CullParams {
        let view = look_to(eye, forward, [0.0, 1.0, 0.0]);
        let proj = perspective(70f32.to_radians(), 16.0 / 9.0, 0.1, 400.0);
        CullParams { eye, frustum: Frustum::from_view_proj(&mat_mul(&proj, &view)), lod_bias: 1.0, density: 1.0, terrain_lod: [60.0, 140.0, 300.0], time: 0.0 }
    }

    fn forest(n: usize) -> World {
        let mut w = World::new("forest");
        w.terrain = Some(Terrain::flat([512.0, 512.0], 1.0));
        let tree = studio_mesh::flora::make("pine", 8.0, 1);
        let a = w.add_asset(Asset::from_studio("pine", &tree, 1.0, &[0.35, 0.1]));
        let grass = studio_mesh::flora::make("grass", 0.5, 1);
        let g = w.add_asset(Asset::from_studio("grass", &grass, 1.0, &[]).with_cull(40.0));
        let mut rng = crate::noise::Rng::new(5);
        let trees: Vec<Instance> = (0..n).map(|_| Instance { pos: [rng.range(-250.0, 250.0), 0.0, rng.range(-250.0, 250.0)], yaw: rng.range(0.0, 360.0), scale: rng.range(0.8, 1.3), rank: (rng.next() * 255.0) as u8 }).collect();
        w.add_scatter("pines", a, trees, true);
        let blades: Vec<Instance> = (0..n * 8).map(|_| Instance { pos: [rng.range(-250.0, 250.0), 0.0, rng.range(-250.0, 250.0)], yaw: 0.0, scale: 1.0, rank: (rng.next() * 255.0) as u8 }).collect();
        w.add_scatter("grass", g, blades, false);
        w.finish();
        w
    }

    #[test]
    fn only_what_is_in_view_is_kept_and_far_trees_use_coarser_levels() {
        let w = forest(3000);
        let looking = cull(&w, &params([0.0, 2.0, -250.0], [0.0, 0.0, 1.0]));
        let away = cull(&w, &params([0.0, 2.0, -250.0], [0.0, 0.0, -1.0]));
        assert!(looking.stats.drawn > 100, "{:?}", looking.stats);
        assert!(away.stats.drawn == 0, "{:?}", away.stats);
        let total: usize = w.terrain.as_ref().unwrap().chunks().iter().product();
        assert!(looking.stats.chunks < total * 4 / 5 && away.stats.chunks < looking.stats.chunks / 2, "{} {} of {total}", looking.stats.chunks, away.stats.chunks);
        // The same eye: nearer trees use finer levels than farther ones.
        let tree_batches: Vec<&Batch> = looking.batches.iter().filter(|b| b.asset == 0).collect();
        assert!(tree_batches.iter().any(|b| b.lod == 0) && tree_batches.iter().any(|b| b.lod >= 1), "levels in use: {:?}", tree_batches.iter().map(|b| (b.lod, b.instances.len())).collect::<Vec<_>>());
        let all_tris: usize = w.triangle_count();
        assert!(looking.stats.triangles * 3 < all_tris, "{} of {}", looking.stats.triangles, all_tris);
    }

    #[test]
    fn grass_thins_with_distance_and_the_same_copies_stay() {
        let w = forest(500);
        let p = params([0.0, 2.0, 0.0], [0.0, 0.0, 1.0]);
        let a = cull(&w, &p);
        let grass = |l: &DrawList| l.batches.iter().filter(|b| b.asset == 1).map(|b| b.instances.len()).sum::<usize>();
        // Nothing beyond its range is drawn.
        for b in a.batches.iter().filter(|b| b.asset == 1) {
            for i in &b.instances {
                assert!(dist([i[0], i[1], i[2]], p.eye) <= 40.0 + 1e-3);
            }
        }
        let mut lean = p;
        lean.density = 0.5;
        let b = cull(&w, &lean);
        assert!(grass(&b) < grass(&a) && grass(&b) > 0, "{} vs {}", grass(&b), grass(&a));
        // Moving the eye a hair keeps (almost) the same ones.
        let c = cull(&w, &params([0.0, 2.0, 0.05], [0.0, 0.0, 1.0]));
        assert!((grass(&c) as i64 - grass(&a) as i64).abs() * 20 < grass(&a) as i64 + 20);
        assert!(density_at(10.0, 40.0) == 1.0 && density_at(40.0, 40.0) == 0.0 && density_at(32.0, 40.0) < 1.0);
    }
}
