// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The world: terrain, shapes placed one by one (buildings, props,
//! characters), shapes scattered by the thousand (trees, rocks, grass),
//! water, lights, sky and air. Everything a renderer draws and a walker
//! bumps into.

use crate::asset::Asset;
use crate::collide::SolidGrid;
use crate::math::{Aabb, V3};
use crate::terrain::Terrain;
use std::sync::Arc;

/// One placed copy of an asset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Instance {
    pub pos: V3,
    /// Degrees about the vertical.
    pub yaw: f32,
    pub scale: f32,
    /// A fixed 0..255 draw order, so thinning with distance drops the
    /// same copies every frame.
    pub rank: u8,
}

impl Instance {
    pub fn at(pos: V3) -> Instance {
        Instance { pos, yaw: 0.0, scale: 1.0, rank: 0 }
    }
    /// The eight floats a renderer needs: position, scale, sin and cos of
    /// the turn, rank.
    pub fn raw(&self) -> [f32; 8] {
        let (s, c) = self.yaw.to_radians().sin_cos();
        [self.pos[0], self.pos[1], self.pos[2], self.scale, s, c, self.rank as f32 / 255.0, 0.0]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectKind {
    Prop,
    Building,
    Character,
}

/// A character's performance: a clip of the mesh's rig played from an
/// offset at a speed, looping.
#[derive(Clone, Debug, PartialEq)]
pub struct Performance {
    pub clip: String,
    pub offset: f32,
    pub speed: f32,
    /// Ground points to walk through, in order, from where it stands.
    pub route: Vec<[f32; 2]>,
    /// Metres per second along the route (0 stands still).
    pub pace: f32,
}

/// A character as it is at one moment: where it stands, which clip it is
/// in, and how far into the clip.
#[derive(Clone, Debug, PartialEq)]
pub struct Performer {
    pub object: usize,
    pub inst: Instance,
    pub clip: String,
    pub clip_time: f32,
}

/// An asset placed on its own.
#[derive(Clone, Debug)]
pub struct Placed {
    pub name: String,
    pub asset: usize,
    pub inst: Instance,
    pub kind: ObjectKind,
    /// Whether a walker bumps into it.
    pub collide: bool,
    pub performance: Option<Performance>,
}

/// Many copies of one asset, held in a grid of cells for culling.
#[derive(Clone, Debug)]
pub struct Scatter {
    pub name: String,
    pub asset: usize,
    pub instances: Vec<Instance>,
    pub grid: CellGrid,
    /// Whether walkers bump into the copies (trunks, boulders).
    pub collide: bool,
}

/// Instances sorted into square cells, each with its box.
#[derive(Clone, Debug, Default)]
pub struct CellGrid {
    pub size: f32,
    pub origin: [f32; 2],
    pub dims: [usize; 2],
    /// Per cell: first entry in `order`, count, and the box of its
    /// instances' shapes.
    pub cells: Vec<(u32, u32, Aabb)>,
    pub order: Vec<u32>,
}

pub const CELL: f32 = 16.0;

impl CellGrid {
    pub fn build(instances: &[Instance], local: &Aabb) -> CellGrid {
        if instances.is_empty() {
            return CellGrid::default();
        }
        let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
        for i in instances {
            lo[0] = lo[0].min(i.pos[0]);
            lo[1] = lo[1].min(i.pos[2]);
            hi[0] = hi[0].max(i.pos[0]);
            hi[1] = hi[1].max(i.pos[2]);
        }
        let dims = [(((hi[0] - lo[0]) / CELL) as usize + 1).min(4096), (((hi[1] - lo[1]) / CELL) as usize + 1).min(4096)];
        let cell_of = |p: V3| {
            let x = (((p[0] - lo[0]) / CELL) as usize).min(dims[0] - 1);
            let z = (((p[2] - lo[1]) / CELL) as usize).min(dims[1] - 1);
            z * dims[0] + x
        };
        let mut counts = vec![0u32; dims[0] * dims[1] + 1];
        for i in instances {
            counts[cell_of(i.pos) + 1] += 1;
        }
        for k in 1..counts.len() {
            counts[k] += counts[k - 1];
        }
        let mut fill = counts.clone();
        let mut order = vec![0u32; instances.len()];
        let mut cells = vec![(0u32, 0u32, Aabb::EMPTY); dims[0] * dims[1]];
        for (n, i) in instances.iter().enumerate() {
            let c = cell_of(i.pos);
            order[fill[c] as usize] = n as u32;
            fill[c] += 1;
            cells[c].2 = cells[c].2.union(&local.placed(i.pos, i.yaw, i.scale));
        }
        for (c, cell) in cells.iter_mut().enumerate() {
            cell.0 = counts[c];
            cell.1 = counts[c + 1] - counts[c];
        }
        CellGrid { size: CELL, origin: lo, dims, cells, order }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Water {
    pub level: f32,
    pub min: [f32; 2],
    pub max: [f32; 2],
    /// Colour of deep water (linear).
    pub colour: V3,
    /// Metres of water that hide the bottom.
    pub clarity: f32,
    pub wave_height: f32,
    pub wave_speed: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightKind {
    Point,
    Spot,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Light {
    pub kind: LightKind,
    pub position: V3,
    pub direction: V3,
    /// Linear RGB times intensity (candela-like; 1 lights a 1 m radius
    /// white).
    pub colour: V3,
    pub range: f32,
    /// Spot half-angle in degrees.
    pub cone: f32,
}

/// Daylight and the sky's look.
#[derive(Clone, Debug, PartialEq)]
pub struct Sky {
    /// Hours, 0-24.
    pub time: f32,
    pub day: f32,
    pub latitude: f32,
    /// Degrees clockwise from +z to north.
    pub heading: f32,
    pub turbidity: f32,
    pub strength: f32,
    pub ground: V3,
    /// A panorama (.hdr) lighting everything instead of the daylight model.
    pub hdri: Option<String>,
    pub hdri_turn: f32,
    /// 0 clear - 1 overcast.
    pub clouds: f32,
}

impl Default for Sky {
    fn default() -> Sky {
        Sky { time: 14.0, day: 172.0, latitude: 40.0, heading: 0.0, turbidity: 3.0, strength: 1.0, ground: [0.22, 0.2, 0.17], hdri: None, hdri_turn: 0.0, clouds: 0.25 }
    }
}

impl Sky {
    /// Direction sunlight travels, in the world (y up, so downward is
    /// negative y).
    pub fn sun_direction(&self) -> V3 {
        let decl = (23.44f32).to_radians() * ((360.0 / 365.0) * (self.day - 81.0)).to_radians().sin();
        let lat = self.latitude.to_radians();
        let ha = ((self.time - 12.0) * 15.0).to_radians();
        let elev = (lat.sin() * decl.sin() + lat.cos() * decl.cos() * ha.cos()).clamp(-1.0, 1.0).asin();
        let az = {
            let y = -ha.sin() * decl.cos();
            let x = decl.sin() * lat.cos() - decl.cos() * lat.sin() * ha.cos();
            y.atan2(x)
        } + self.heading.to_radians();
        // Towards the sun, then reversed.
        crate::math::mul([elev.cos() * az.sin(), elev.sin(), elev.cos() * az.cos()], -1.0)
    }
    /// Degrees of the sun above the horizon.
    pub fn sun_elevation(&self) -> f32 {
        (-self.sun_direction()[1]).clamp(-1.0, 1.0).asin().to_degrees()
    }
}

/// Air: fog and haze.
#[derive(Clone, Debug, PartialEq)]
pub struct Atmosphere {
    /// Fog thickness per metre at the ground.
    pub fog: f32,
    /// Fog thins with height: per metre.
    pub height_falloff: f32,
    /// Fog colour; None takes the sky at the horizon.
    pub colour: Option<V3>,
}

impl Default for Atmosphere {
    fn default() -> Atmosphere {
        Atmosphere { fog: 0.0012, height_falloff: 0.02, colour: None }
    }
}

/// How the picture is finished.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    /// Exposure in stops.
    pub exposure: f32,
    pub bloom: f32,
    pub ssao: f32,
    pub saturation: f32,
    pub vignette: f32,
}

impl Default for Look {
    fn default() -> Look {
        Look { exposure: 0.0, bloom: 0.35, ssao: 1.0, saturation: 1.0, vignette: 0.12 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spawn {
    pub position: V3,
    pub yaw: f32,
}

#[derive(Clone, Debug)]
pub struct World {
    pub name: String,
    pub seed: u32,
    pub terrain: Option<Terrain>,
    pub assets: Vec<Asset>,
    pub objects: Vec<Placed>,
    pub scatters: Vec<Scatter>,
    pub water: Vec<Water>,
    pub lights: Vec<Light>,
    pub sky: Sky,
    pub atmosphere: Atmosphere,
    pub look: Look,
    pub spawn: Spawn,
    /// Extra blockers not tied to an object (walls of a room, a fence).
    pub extra_solids: Vec<crate::collide::Solid>,
    pub(crate) solids: SolidGrid,
    /// Bumps whenever anything changes; renderers rebuild on it.
    pub revision: u64,
}

impl World {
    pub fn new(name: &str) -> World {
        World {
            name: name.into(),
            seed: 1,
            terrain: None,
            assets: Vec::new(),
            objects: Vec::new(),
            scatters: Vec::new(),
            water: Vec::new(),
            lights: Vec::new(),
            sky: Sky::default(),
            atmosphere: Atmosphere::default(),
            look: Look::default(),
            spawn: Spawn { position: [0.0, 1.0, 0.0], yaw: 0.0 },
            extra_solids: Vec::new(),
            solids: SolidGrid::default(),
            revision: 1,
        }
    }

    pub fn add_asset(&mut self, a: Asset) -> usize {
        self.assets.push(a);
        self.assets.len() - 1
    }

    pub fn place(&mut self, name: &str, asset: usize, kind: ObjectKind, inst: Instance, collide: bool) -> usize {
        self.objects.push(Placed { name: name.into(), asset, inst, kind, collide, performance: None });
        self.objects.len() - 1
    }

    pub fn add_scatter(&mut self, name: &str, asset: usize, instances: Vec<Instance>, collide: bool) {
        let grid = CellGrid::build(&instances, &self.assets[asset].bounds);
        self.scatters.push(Scatter { name: name.into(), asset, instances, grid, collide });
    }

    /// The ground height at (x, z): the terrain, else 0.
    pub fn ground_y(&self, x: f32, z: f32) -> f32 {
        self.terrain.as_ref().map_or(0.0, |t| t.height_at(x, z))
    }

    /// Rebuilds what depends on everything else (the blocker grid) and
    /// bumps the revision. Call after editing objects or scatters.
    pub fn finish(&mut self) {
        for s in &mut self.scatters {
            s.grid = CellGrid::build(&s.instances, &self.assets[s.asset].bounds);
        }
        self.solids = crate::collide::build_grid(self);
        self.revision += 1;
    }

    /// The box around everything.
    pub fn bounds(&self) -> Aabb {
        let mut b = self.terrain.as_ref().map_or(Aabb::EMPTY, |t| t.bounds());
        for o in &self.objects {
            b = b.union(&self.assets[o.asset].bounds.placed(o.inst.pos, o.inst.yaw, o.inst.scale));
        }
        for w in &self.water {
            b = b.union(&Aabb::new([w.min[0], w.level, w.min[1]], [w.max[0], w.level + 0.1, w.max[1]]));
        }
        if b.is_empty() { Aabb::new([-10.0, 0.0, -10.0], [10.0, 5.0, 10.0]) } else { b }
    }

    /// Whether an object is a character that acts (drawn posed, not from
    /// its shape at rest).
    pub fn is_performer(&self, o: &Placed) -> bool {
        o.performance.is_some() && self.assets[o.asset].rigged.is_some()
    }

    /// Where and how a performing character is at `t` seconds: it walks
    /// its route at its pace (turning to face the way, standing on the
    /// ground there) in its clip, then stands in "idle".
    pub fn performer_state(&self, object: usize, t: f32) -> Option<Performer> {
        let o = self.objects.get(object)?;
        let p = o.performance.as_ref()?;
        let mut inst = o.inst;
        let mut clip = p.clip.clone();
        let local = (t * p.speed + p.offset).max(0.0);
        if !p.route.is_empty() && p.pace > 0.0 {
            let mut travelled = (t * p.pace).max(0.0);
            let mut at = [o.inst.pos[0], o.inst.pos[2]];
            let mut arrived = true;
            for next in &p.route {
                let seg = ((next[0] - at[0]).powi(2) + (next[1] - at[1]).powi(2)).sqrt();
                if seg < 1e-4 {
                    continue;
                }
                let dir = [(next[0] - at[0]) / seg, (next[1] - at[1]) / seg];
                inst.yaw = dir[0].atan2(dir[1]).to_degrees();
                if travelled <= seg {
                    at = [at[0] + dir[0] * travelled, at[1] + dir[1] * travelled];
                    arrived = false;
                    break;
                }
                travelled -= seg;
                at = *next;
            }
            inst.pos[0] = at[0];
            inst.pos[2] = at[1];
            inst.pos[1] = self.ground_at(at[0], at[1], o.inst.pos[1] + 1.0).map_or(o.inst.pos[1], |g| g.y);
            if arrived {
                clip = "idle".into();
            }
        }
        Some(Performer { object, inst, clip, clip_time: local })
    }

    /// Triangles in all the finest levels, counting every copy.
    pub fn triangle_count(&self) -> usize {
        let obj: usize = self.objects.iter().map(|o| self.assets[o.asset].lods[0].triangles()).sum();
        let sc: usize = self.scatters.iter().map(|s| s.instances.len() * self.assets[s.asset].lods[0].triangles()).sum();
        let ter = self.terrain.as_ref().map_or(0, |t| t.cells[0] * t.cells[1] * 2);
        obj + sc + ter
    }

    pub fn shared_assets(&self) -> Arc<Vec<Asset>> {
        Arc::new(self.assets.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sun_is_overhead_at_noon_and_gone_at_night() {
        let mut s = Sky { time: 12.0, day: 172.0, latitude: 40.0, ..Sky::default() };
        let noon = s.sun_elevation();
        assert!(noon > 60.0, "{noon}");
        assert!(s.sun_direction()[1] < -0.8);
        s.time = 0.0;
        assert!(s.sun_elevation() < -20.0);
        s.time = 18.5;
        let low = s.sun_elevation();
        assert!((0.0..25.0).contains(&low), "{low}");
    }

    #[test]
    fn the_grid_sorts_every_instance_into_one_cell() {
        let mut inst = Vec::new();
        for i in 0..500 {
            inst.push(Instance { pos: [(i % 25) as f32 * 7.0, 0.0, (i / 25) as f32 * 9.0], yaw: i as f32, scale: 1.0, rank: (i % 256) as u8 });
        }
        let g = CellGrid::build(&inst, &Aabb::new([-1.0, 0.0, -1.0], [1.0, 3.0, 1.0]));
        assert_eq!(g.order.len(), 500);
        assert_eq!(g.cells.iter().map(|c| c.1 as usize).sum::<usize>(), 500);
        let mut seen = g.order.clone();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 500);
        for c in &g.cells {
            for k in c.0..c.0 + c.1 {
                let p = inst[g.order[k as usize] as usize].pos;
                assert!(c.2.contains(p) || c.2.contains([p[0], c.2.min[1], p[2]]));
            }
        }
    }
}
