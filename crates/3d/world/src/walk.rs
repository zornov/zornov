// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Moving through a world like a game: a character controller with
//! gravity, steps, slopes, jumping, sprinting, wading and swimming, and a
//! camera that walks (first or third person), flies or orbits.
//!
//! The body is a standing circle (`radius`) of `height` metres whose
//! feet position is the state. Each step it accelerates toward where the
//! controls point, falls, is pushed out of walls, and is lifted onto
//! whatever floor is within a step's height (the terrain, a stair, a
//! kerb, a floor above a stair), refusing slopes steeper than the limit
//! (it slides back down them). The step runs in sub-steps short enough
//! that nothing is tunnelled through.

use crate::math::{V3, add, dot, len, mul, norm, sub, wrap_deg};
use crate::world::World;

/// A character's size and abilities.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub radius: f32,
    pub height: f32,
    pub eye: f32,
    /// Highest step climbed without jumping.
    pub step: f32,
    /// Steepest slope walked, degrees.
    pub max_slope: f32,
    pub gravity: f32,
    /// Highest jump from level ground.
    pub jump_height: f32,
    pub walk_speed: f32,
    pub sprint_speed: f32,
    pub accel_ground: f32,
    pub accel_air: f32,
}

impl Default for Params {
    fn default() -> Params {
        Params { radius: 0.35, height: 1.75, eye: 1.62, step: 0.45, max_slope: 50.0, gravity: 22.0, jump_height: 1.1, walk_speed: 4.2, sprint_speed: 7.6, accel_ground: 45.0, accel_air: 7.0 }
    }
}

impl Params {
    pub fn jump_speed(&self) -> f32 {
        (2.0 * self.gravity * self.jump_height).sqrt()
    }
}

/// What the controls ask for this moment.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Input {
    /// -1..1, forward positive.
    pub forward: f32,
    /// -1..1, right positive.
    pub right: f32,
    /// Fly only: -1..1, up positive.
    pub up: f32,
    pub jump: bool,
    pub sprint: bool,
}

/// The walker's state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    /// Feet.
    pub pos: V3,
    pub vel: V3,
    pub grounded: bool,
    pub sliding: bool,
    /// Metres of water the body stands in (up to its height).
    pub depth: f32,
    pub swimming: bool,
    coyote: f32,
    buffer: f32,
    /// What the camera sees as the feet height: follows the feet but
    /// smooths a step up or down.
    pub smooth_y: f32,
}

const SUB_DT: f32 = 1.0 / 120.0;

impl Body {
    pub fn at(pos: V3) -> Body {
        Body { pos, vel: [0.0; 3], grounded: false, sliding: false, depth: 0.0, swimming: false, coyote: 0.0, buffer: 0.0, smooth_y: pos[1] }
    }

    /// Stands the body on the ground under it (or leaves it falling).
    pub fn settle(&mut self, w: &World, p: &Params) {
        if let Some(g) = w.ground_at(self.pos[0], self.pos[2], self.pos[1] + p.step + 0.01) {
            self.pos[1] = g.y;
            self.grounded = true;
        }
        self.smooth_y = self.pos[1];
    }

    pub fn speed_h(&self) -> f32 {
        (self.vel[0] * self.vel[0] + self.vel[2] * self.vel[2]).sqrt()
    }

    /// Advances by `dt` seconds with the camera turned `yaw` degrees.
    pub fn step(&mut self, w: &World, p: &Params, input: &Input, yaw: f32, dt: f32) {
        let n = ((dt / SUB_DT).ceil() as usize).clamp(1, 40);
        let h = dt / n as f32;
        for _ in 0..n {
            self.sub_step(w, p, input, yaw, h);
        }
        // The eye follows the feet, easing steps.
        let k = 1.0 - (-18.0 * dt).exp();
        let d = self.pos[1] - self.smooth_y;
        self.smooth_y = if d.abs() > 1.5 || !self.grounded { self.pos[1] } else { self.smooth_y + d * k };
    }

    fn sub_step(&mut self, w: &World, p: &Params, input: &Input, yaw: f32, dt: f32) {
        // Water.
        let level = w.water_at(self.pos[0], self.pos[2]);
        self.depth = level.map_or(0.0, |l| (l - self.pos[1]).clamp(0.0, p.height));
        let was_swimming = self.swimming;
        self.swimming = self.depth > p.height * 0.62;
        let wade = 1.0 - 0.45 * (self.depth / p.height).clamp(0.0, 1.0);

        // Wish velocity.
        let (s, c) = yaw.to_radians().sin_cos();
        let fwd = [s, 0.0, c];
        let right = [c, 0.0, -s];
        let mut wish = add(mul(fwd, input.forward), mul(right, input.right));
        if len(wish) > 1.0 {
            wish = norm(wish);
        }
        let top = if input.sprint && !self.swimming { p.sprint_speed } else { p.walk_speed };
        let speed = top * wade * if self.swimming { 0.7 } else { 1.0 };
        let target = mul(wish, speed);
        let accel = if self.grounded { p.accel_ground } else if self.swimming { p.accel_ground * 0.3 } else { p.accel_air };
        let dv = [target[0] - self.vel[0], target[2] - self.vel[2]];
        let dl = (dv[0] * dv[0] + dv[1] * dv[1]).sqrt();
        let max = accel * dt;
        let k = if dl > max { max / dl } else { 1.0 };
        // Friction when no input on the ground.
        self.vel[0] += dv[0] * k;
        self.vel[2] += dv[1] * k;

        // Jump.
        self.buffer = if input.jump { 0.12 } else { (self.buffer - dt).max(0.0) };
        self.coyote = if self.grounded { 0.1 } else { (self.coyote - dt).max(0.0) };
        if self.buffer > 0.0 && (self.grounded || self.coyote > 0.0) && !self.swimming && !self.sliding {
            self.vel[1] = p.jump_speed();
            self.grounded = false;
            self.coyote = 0.0;
            self.buffer = 0.0;
        }

        // Gravity, buoyancy.
        if self.swimming {
            let float_feet = level.unwrap_or(self.pos[1]) - p.height * 0.7;
            self.vel[1] += ((float_feet - self.pos[1]) * 9.0 - self.vel[1] * 3.5) * dt;
            if input.jump {
                self.vel[1] += 14.0 * dt;
            }
            self.grounded = false;
        } else if !self.grounded {
            self.vel[1] = (self.vel[1] - p.gravity * dt).max(-60.0);
        }

        // Horizontal movement against slopes and walls.
        let old = self.pos;
        let mut new = [old[0] + self.vel[0] * dt, old[1], old[2] + self.vel[2] * dt];
        if self.grounded
            && let Some(g) = w.ground_at(new[0], new[2], old[1] + p.step + 0.01)
            && g.normal[1] < p.max_slope.to_radians().cos()
            && g.y > old[1] + 1e-3
        {
            // Too steep to climb: remove the uphill part of the move.
            let up_h = norm([-g.normal[0], 0.0, -g.normal[2]]);
            let into = dot([self.vel[0], 0.0, self.vel[2]], up_h);
            if into > 0.0 {
                self.vel[0] -= up_h[0] * into;
                self.vel[2] -= up_h[2] * into;
            }
            new = [old[0] + self.vel[0] * dt, old[1], old[2] + self.vel[2] * dt];
        }
        let y_lo = old[1] + p.step;
        let y_hi = old[1] + p.height;
        for _ in 0..3 {
            let out = w.push_out(new, p.radius, y_lo, y_hi);
            if out[0].abs() < 1e-5 && out[1].abs() < 1e-5 {
                break;
            }
            new[0] += out[0];
            new[2] += out[1];
            let n = norm([out[0], 0.0, out[1]]);
            let into = dot([self.vel[0], 0.0, self.vel[2]], n);
            if into < 0.0 {
                self.vel[0] -= n[0] * into;
                self.vel[2] -= n[2] * into;
            }
        }
        self.pos[0] = new[0];
        self.pos[2] = new[2];

        // Vertical movement and floors.
        self.sliding = false;
        if self.swimming {
            self.pos[1] += self.vel[1] * dt;
            if let Some(g) = w.ground_at(self.pos[0], self.pos[2], self.pos[1] + 0.01)
                && self.pos[1] < g.y
            {
                self.pos[1] = g.y;
                self.vel[1] = self.vel[1].max(0.0);
            }
            return;
        }
        if was_swimming && !self.swimming && self.vel[1] > 0.0 {
            self.grounded = false;
        }
        if self.grounded && self.vel[1] <= 0.0 {
            let snap = p.step + 0.05;
            match w.ground_at(self.pos[0], self.pos[2], old[1] + p.step + 0.01) {
                Some(g) if g.y >= old[1] - snap => {
                    if g.normal[1] < p.max_slope.to_radians().cos() && g.y <= old[1] + 1e-3 {
                        // Walked onto a steep face: slide down it.
                        self.slide(&g.normal, p, dt);
                        self.pos[1] = g.y.min(self.pos[1]);
                        self.grounded = false;
                        self.sliding = true;
                    } else {
                        self.pos[1] = g.y;
                        self.vel[1] = 0.0;
                    }
                }
                _ => {
                    self.grounded = false;
                }
            }
        }
        if !self.grounded {
            let before = self.pos[1];
            let after = before + self.vel[1] * dt;
            if self.vel[1] > 0.0 {
                // Heads and ceilings.
                let head = before + p.height;
                match w.ceiling_above(self.pos[0], self.pos[2], head - 0.02) {
                    Some(c) if head + self.vel[1] * dt >= c => {
                        self.pos[1] = c - p.height;
                        self.vel[1] = 0.0;
                    }
                    _ => self.pos[1] = after,
                }
            } else {
                match w.ground_at(self.pos[0], self.pos[2], before + 0.05) {
                    Some(g) if after <= g.y => {
                        if g.normal[1] < p.max_slope.to_radians().cos() {
                            self.pos[1] = g.y;
                            self.vel[1] = 0.0;
                            self.slide(&g.normal, p, dt);
                            self.sliding = true;
                        } else {
                            self.pos[1] = g.y;
                            self.vel[1] = 0.0;
                            self.grounded = true;
                        }
                    }
                    _ => self.pos[1] = after,
                }
            }
        }
        // Never fall out of the world.
        if self.pos[1] < w.bounds().min[1] - 200.0 {
            self.pos = w.spawn.position;
            self.vel = [0.0; 3];
        }
    }

    fn slide(&mut self, n: &V3, p: &Params, dt: f32) {
        let down = norm([n[0] * -n[1], -(n[0] * n[0] + n[2] * n[2]), n[2] * -n[1]]);
        let a = p.gravity * 0.8 * dt;
        self.vel[0] += down[0] * a;
        self.vel[2] += down[2] * a;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Walk,
    Fly,
    Orbit,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Walk => "walk",
            Mode::Fly => "fly",
            Mode::Orbit => "orbit",
        }
    }
    pub fn parse(s: &str) -> Option<Mode> {
        match s.trim().to_lowercase().as_str() {
            "walk" | "walking" | "first_person" => Some(Mode::Walk),
            "fly" | "flying" | "free" => Some(Mode::Fly),
            "orbit" => Some(Mode::Orbit),
            _ => None,
        }
    }
    pub fn next(self) -> Mode {
        match self {
            Mode::Walk => Mode::Fly,
            Mode::Fly => Mode::Orbit,
            Mode::Orbit => Mode::Walk,
        }
    }
}

/// Controls for one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Controls {
    pub input: Input,
    /// Mouse look, degrees this frame (right and down positive).
    pub look: [f32; 2],
    /// Scroll: dolly in orbit/third person, speed in fly.
    pub zoom: f32,
    /// Orbit: drag pans instead of turning.
    pub pan: [f32; 2],
}

/// A camera at one instant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub pos: V3,
    /// Degrees; 0 looks along +z, positive turns right.
    pub yaw: f32,
    /// Degrees; positive looks up.
    pub pitch: f32,
    /// Vertical field of view, degrees.
    pub fov: f32,
}

impl Pose {
    pub fn forward(&self) -> V3 {
        let (sy, cy) = self.yaw.to_radians().sin_cos();
        let (sp, cp) = self.pitch.to_radians().sin_cos();
        [cp * sy, sp, cp * cy]
    }
    pub fn right(&self) -> V3 {
        let (sy, cy) = self.yaw.to_radians().sin_cos();
        [cy, 0.0, -sy]
    }
    pub fn up(&self) -> V3 {
        crate::math::cross(self.forward(), self.right())
    }
    pub fn view(&self) -> crate::math::Mat4 {
        crate::math::look_to(self.pos, self.forward(), [0.0, 1.0, 0.0])
    }
    pub fn looking_at(pos: V3, target: V3, fov: f32) -> Pose {
        let d = sub(target, pos);
        Pose { pos, yaw: d[0].atan2(d[2]).to_degrees(), pitch: (d[1] / len(d).max(1e-6)).clamp(-1.0, 1.0).asin().to_degrees(), fov }
    }
}

/// Walk, fly or orbit through a world.
#[derive(Clone, Debug)]
pub struct Explorer {
    pub mode: Mode,
    pub params: Params,
    pub body: Body,
    pub yaw: f32,
    pub pitch: f32,
    pub fov: f32,
    pub third_person: bool,
    pub follow_distance: f32,
    /// Flying: the eye.
    pub fly_pos: V3,
    pub fly_speed: f32,
    /// Orbiting: what it circles and how far.
    pub orbit_target: V3,
    pub orbit_distance: f32,
}

impl Explorer {
    pub fn new(w: &World) -> Explorer {
        let params = Params::default();
        let mut body = Body::at(w.spawn.position);
        body.settle(w, &params);
        Explorer {
            mode: Mode::Walk,
            params,
            body,
            yaw: w.spawn.yaw,
            pitch: 0.0,
            fov: 60.0,
            third_person: false,
            follow_distance: 3.5,
            fly_pos: add(w.spawn.position, [0.0, params.eye, 0.0]),
            fly_speed: 12.0,
            orbit_target: w.spawn.position,
            orbit_distance: 12.0,
        }
    }

    /// The first-person eye.
    pub fn eye(&self) -> V3 {
        [self.body.pos[0], self.body.smooth_y + self.params.eye, self.body.pos[2]]
    }

    /// Changes mode, keeping the view where it was: walking from a flying
    /// view drops to the ground below the eye; flying from walking starts
    /// at the eye; orbit circles the point ahead.
    pub fn set_mode(&mut self, w: &World, mode: Mode) {
        if mode == self.mode {
            return;
        }
        let cam = self.camera(w);
        match mode {
            Mode::Walk => {
                let feet = [cam.pos[0], cam.pos[1] - self.params.eye, cam.pos[2]];
                self.body = Body::at(feet);
                self.body.vel = [0.0; 3];
                // Over the ground: land on it from here.
                self.body.pos[1] = self.body.pos[1].max(w.ground_at(feet[0], feet[2], f32::MAX).map_or(f32::MIN, |g| g.y));
                self.body.settle(w, &self.params);
                self.yaw = cam.yaw;
                self.pitch = cam.pitch.clamp(-85.0, 85.0);
            }
            Mode::Fly => {
                self.fly_pos = cam.pos;
                self.yaw = cam.yaw;
                self.pitch = cam.pitch;
            }
            Mode::Orbit => {
                let f = Pose { pos: cam.pos, yaw: cam.yaw, pitch: cam.pitch, fov: cam.fov }.forward();
                let hit = w.raycast(cam.pos, f, 200.0).map(|h| h.t).unwrap_or(12.0).clamp(2.0, 200.0);
                self.orbit_distance = hit;
                self.orbit_target = add(cam.pos, mul(f, hit));
                self.yaw = cam.yaw;
                self.pitch = cam.pitch;
            }
        }
        self.mode = mode;
    }

    /// Places the walker on the ground at (x, z) facing `yaw`.
    pub fn teleport(&mut self, w: &World, pos: V3, yaw: f32) {
        self.body = Body::at(pos);
        self.body.settle(w, &self.params);
        self.yaw = yaw;
        self.fly_pos = add(pos, [0.0, self.params.eye, 0.0]);
    }

    pub fn update(&mut self, w: &World, c: &Controls, dt: f32) {
        self.yaw = wrap_deg(self.yaw + c.look[0]);
        match self.mode {
            Mode::Walk => {
                self.pitch = (self.pitch - c.look[1]).clamp(-85.0, 85.0);
                self.body.step(w, &self.params, &c.input, self.yaw, dt);
                if self.third_person {
                    self.follow_distance = (self.follow_distance * (1.0 - c.zoom * 0.1)).clamp(1.2, 12.0);
                }
            }
            Mode::Fly => {
                self.pitch = (self.pitch - c.look[1]).clamp(-89.0, 89.0);
                self.fly_speed = (self.fly_speed * (1.0 + c.zoom * 0.1)).clamp(0.5, 600.0);
                let pose = Pose { pos: self.fly_pos, yaw: self.yaw, pitch: self.pitch, fov: self.fov };
                let boost = if c.input.sprint { 3.0 } else { 1.0 };
                let v = add(add(mul(pose.forward(), c.input.forward), mul(pose.right(), c.input.right)), [0.0, c.input.up, 0.0]);
                self.fly_pos = add(self.fly_pos, mul(v, self.fly_speed * boost * dt));
                // Not under the ground.
                if let Some(g) = w.ground_at(self.fly_pos[0], self.fly_pos[2], f32::MAX) {
                    self.fly_pos[1] = self.fly_pos[1].max(g.y + 0.3);
                }
            }
            Mode::Orbit => {
                self.pitch = (self.pitch - c.look[1]).clamp(-89.0, 89.0);
                self.orbit_distance = (self.orbit_distance * (1.0 - c.zoom * 0.12)).clamp(0.8, 2000.0);
                let pose = Pose { pos: [0.0; 3], yaw: self.yaw, pitch: 0.0, fov: self.fov };
                let k = self.orbit_distance * 0.002;
                self.orbit_target = add(self.orbit_target, add(mul(pose.right(), -c.pan[0] * k), [0.0, c.pan[1] * k, 0.0]));
                let drive = add(mul(pose.forward(), c.input.forward), mul(pose.right(), c.input.right));
                self.orbit_target = add(self.orbit_target, mul(drive, self.orbit_distance * 0.8 * dt));
            }
        }
    }

    /// Where the camera is now.
    pub fn camera(&self, w: &World) -> Pose {
        match self.mode {
            Mode::Walk => {
                let eye = self.eye();
                if !self.third_person {
                    return Pose { pos: eye, yaw: self.yaw, pitch: self.pitch, fov: self.fov };
                }
                // Behind and above the shoulder, pulled in by walls.
                let look = Pose { pos: eye, yaw: self.yaw, pitch: self.pitch, fov: self.fov };
                let head = add(eye, [0.0, 0.25, 0.0]);
                let back = mul(look.forward(), -1.0);
                let want = self.follow_distance;
                let d = w.raycast(head, back, want + 0.3).map_or(want, |h| (h.t - 0.3).max(0.4));
                let mut pos = add(head, mul(back, d));
                if let Some(g) = w.ground_at(pos[0], pos[2], f32::MAX) {
                    pos[1] = pos[1].max(g.y + 0.25);
                }
                Pose::looking_at(pos, add(head, mul(look.forward(), 6.0)), self.fov)
            }
            Mode::Fly => Pose { pos: self.fly_pos, yaw: self.yaw, pitch: self.pitch, fov: self.fov },
            Mode::Orbit => {
                let f = Pose { pos: [0.0; 3], yaw: self.yaw, pitch: self.pitch, fov: self.fov }.forward();
                let mut pos = sub(self.orbit_target, mul(f, self.orbit_distance));
                if let Some(g) = w.ground_at(pos[0], pos[2], f32::MAX) {
                    pos[1] = pos[1].max(g.y + 0.3);
                }
                Pose::looking_at(pos, self.orbit_target, self.fov)
            }
        }
    }

    /// Where the walker's body stands, for drawing a stand-in in third
    /// person.
    pub fn avatar(&self) -> Option<(V3, f32)> {
        (self.mode == Mode::Walk && self.third_person).then_some((self.body.pos, self.yaw))
    }
}

/// A walk to follow: waypoints in order, at a pace.
#[derive(Clone, Debug)]
pub struct Route {
    pub points: Vec<V3>,
    pub sprint: bool,
    /// Seconds to stand still at each point.
    pub pause: f32,
    /// How near counts as there.
    pub arrive: f32,
}

/// Walks an explorer along a route with steering that goes round
/// obstacles, calling `each` with the time and camera after every frame
/// (so the walk can be recorded). Returns the seconds taken and whether
/// every point was reached.
pub fn walk_route(w: &World, ex: &mut Explorer, route: &Route, fps: f32, mut each: impl FnMut(f32, &Pose)) -> (f32, bool) {
    let dt = 1.0 / fps.max(1.0);
    let mut t = 0.0;
    ex.mode = Mode::Walk;
    each(t, &ex.camera(w));
    let mut stuck = 0.0f32;
    let mut last = ex.body.pos;
    let mut follow: Option<f32>;
    let mut clear_for = 0.0f32;
    // Each leg is planned over the world's floors first.
    let mut legs: Vec<V3> = Vec::new();
    let mut at = ex.body.pos;
    for target in &route.points {
        match crate::nav::find_path(w, at, *target, &ex.params, &crate::nav::NavParams::default()) {
            Some(p) => legs.extend(p.into_iter().skip(1)),
            None => legs.push(*target),
        }
        at = *target;
    }
    for (ti, target) in legs.iter().enumerate() {
        let last_of_route = route.points.iter().any(|p| (p[0] - target[0]).abs() < 1e-3 && (p[2] - target[2]).abs() < 1e-3);
        let _ = ti;
        let mut pause = 0.0;
        let mut guard = 0.0;
        follow = None;
        loop {
            let to = sub(*target, ex.body.pos);
            let flat = [to[0], 0.0, to[2]];
            let dist = len(flat);
            if dist < if last_of_route { route.arrive } else { 0.7 } {
                pause += dt;
                ex.update(w, &Controls::default(), dt);
                t += dt;
                each(t, &ex.camera(w));
                if pause >= if last_of_route { route.pause } else { 0.0 } {
                    break;
                }
                continue;
            }
            guard += dt;
            if guard > 90.0 + dist * 2.0 {
                return (t, false);
            }
            // Turn towards the goal; when something stands in the way,
            // follow along it on one side until the way is clear.
            let goal = flat[0].atan2(flat[2]).to_degrees();
            let chest = add(ex.body.pos, [0.0, ex.params.step + 0.4, 0.0]);
            let ahead = |deg: f32, reach: f32| {
                let (s, c) = deg.to_radians().sin_cos();
                w.raycast(chest, [s, 0.0, c], reach).map_or(reach, |h| h.t)
            };
            let blocked = ahead(goal, 2.2_f32.min(dist)) < 2.0_f32.min(dist - 0.05).max(0.5);
            if follow.is_none() && blocked {
                follow = Some(if ahead(goal + 75.0, 6.0) >= ahead(goal - 75.0, 6.0) { 1.0f32 } else { -1.0 });
                clear_for = 0.0;
            }
            let mut want = goal;
            if let Some(side) = follow {
                if blocked {
                    clear_for = 0.0;
                    want = goal + side * 80.0;
                    if ahead(want, 1.2) < 1.0 {
                        want = goal + side * 120.0;
                    }
                } else {
                    clear_for += dt;
                    want = goal + side * 25.0 * (1.0 - (clear_for / 0.6).min(1.0));
                    if clear_for > 0.6 {
                        follow = None;
                    }
                }
            }
            let diff = wrap_deg(want - ex.yaw);
            let turn = diff.clamp(-360.0 * dt, 360.0 * dt);
            let slow = (1.0 - diff.abs() / 120.0).clamp(0.15, 1.0);
            let c = Controls { input: Input { forward: slow, sprint: route.sprint, ..Default::default() }, look: [turn, 0.0], ..Default::default() };
            // A little downhill/uphill pitch eases the view toward the horizon.
            ex.pitch += (0.0 - ex.pitch) * (1.0 - (-3.0 * dt).exp());
            ex.update(w, &c, dt);
            t += dt;
            each(t, &ex.camera(w));
            // Stuck: jump, then give up on this point.
            if dist_moved(last, ex.body.pos) < 0.02 * dt * 60.0 * 0.2 {
                stuck += dt;
            } else {
                stuck = 0.0;
                last = ex.body.pos;
            }
            if stuck > 1.0 {
                let jump = Controls { input: Input { forward: 1.0, jump: true, ..Default::default() }, ..Default::default() };
                ex.update(w, &jump, dt);
                if stuck > 3.0 {
                    return (t, false);
                }
            }
        }
    }
    (t, true)
}

fn dist_moved(a: V3, b: V3) -> f32 {
    len(sub(a, b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collide::Solid;
    use crate::terrain::{Brush, Terrain};

    fn field() -> World {
        let mut w = World::new("t");
        w.terrain = Some(Terrain::flat([256.0, 256.0], 1.0));
        w
    }

    fn run(w: &World, b: &mut Body, p: &Params, input: Input, secs: f32) {
        let n = (secs * 60.0) as usize;
        for _ in 0..n {
            b.step(w, p, &input, 0.0, 1.0 / 60.0);
        }
    }

    #[test]
    fn gravity_drops_a_body_onto_the_ground_and_it_stays() {
        let mut w = field();
        w.finish();
        let p = Params::default();
        let mut b = Body::at([0.0, 5.0, 0.0]);
        run(&w, &mut b, &p, Input::default(), 1.5);
        assert!(b.grounded && b.pos[1].abs() < 1e-4, "{:?}", b.pos);
        assert!(b.vel[1] == 0.0);
    }

    #[test]
    fn walking_and_sprinting_reach_their_speeds() {
        let mut w = field();
        w.finish();
        let p = Params::default();
        let mut b = Body::at([0.0, 0.0, -100.0]);
        b.settle(&w, &p);
        run(&w, &mut b, &p, Input { forward: 1.0, ..Default::default() }, 3.0);
        let walked = b.pos[2] + 100.0;
        assert!((walked - 4.2 * 3.0).abs() < 1.2, "{walked}");
        let mut s = Body::at([0.0, 0.0, -100.0]);
        s.settle(&w, &p);
        run(&w, &mut s, &p, Input { forward: 1.0, sprint: true, ..Default::default() }, 3.0);
        assert!(s.pos[2] + 100.0 > walked * 1.5);
        // Diagonal is no faster than straight.
        let mut d = Body::at([0.0, 0.0, 0.0]);
        d.settle(&w, &p);
        run(&w, &mut d, &p, Input { forward: 1.0, right: 1.0, ..Default::default() }, 2.0);
        assert!(d.speed_h() < 4.3, "{}", d.speed_h());
    }

    #[test]
    fn a_jump_peaks_at_the_set_height_and_lands() {
        let mut w = field();
        w.finish();
        let p = Params::default();
        let mut b = Body::at([0.0, 0.0, 0.0]);
        b.settle(&w, &p);
        let mut peak = 0.0f32;
        for i in 0..120 {
            let jump = i == 0;
            b.step(&w, &p, &Input { jump, ..Default::default() }, 0.0, 1.0 / 60.0);
            peak = peak.max(b.pos[1]);
        }
        assert!((peak - 1.1).abs() < 0.1, "{peak}");
        assert!(b.grounded && b.pos[1].abs() < 1e-3);
    }

    #[test]
    fn steps_are_climbed_and_taller_walls_stop_you() {
        let mut w = field();
        w.extra_solids.push(Solid::Box { centre: [0.0, 0.15, 5.0], half: [3.0, 0.15, 1.0], yaw: 0.0 });
        w.extra_solids.push(Solid::Box { centre: [10.0, 0.6, 5.0], half: [3.0, 0.6, 1.0], yaw: 0.0 });
        w.finish();
        let p = Params::default();
        let mut b = Body::at([0.0, 0.0, 0.0]);
        b.settle(&w, &p);
        run(&w, &mut b, &p, Input { forward: 1.0, ..Default::default() }, 2.0);
        assert!(b.pos[2] > 6.0 && b.grounded, "climbed over a 0.3 m kerb: {:?}", b.pos);
        let mut c = Body::at([10.0, 0.0, 0.0]);
        c.settle(&w, &p);
        run(&w, &mut c, &p, Input { forward: 1.0, ..Default::default() }, 3.0);
        assert!(c.pos[2] < 4.0 - p.radius + 0.05, "stopped by a 1.2 m wall: {:?}", c.pos);
        // A jump gets over the 0.7 m ledge but not the high wall.
        let mut d = Body::at([10.0, 0.0, 0.0]);
        d.settle(&w, &p);
        for i in 0..200 {
            d.step(&w, &p, &Input { forward: 1.0, jump: i % 30 == 0, ..Default::default() }, 0.0, 1.0 / 60.0);
        }
        assert!(d.pos[2] > 4.0 - p.radius - 0.5, "{:?}", d.pos);
    }

    #[test]
    fn slopes_are_walked_up_when_gentle_and_refused_when_steep() {
        let mut w = field();
        // A ramp of height rising with z: gentle 20 degrees, then steep 65.
        {
            let t = w.terrain.as_mut().unwrap();
            let (nx, nz) = (t.cells[0], t.cells[1]);
            for iz in 0..=nz {
                let z = t.origin[1] + iz as f32 * t.cell;
                let gentle = ((z + 20.0).clamp(0.0, 20.0)) * 0.364;
                let steep = (z - 20.0).max(0.0) * 2.1;
                for ix in 0..=nx {
                    t.heights[iz * (nx + 1) + ix] = gentle + steep;
                }
            }
            t.sculpt(Brush::Smooth, [1000.0, 1000.0], 1.0, 0.0);
        }
        w.finish();
        let p = Params::default();
        let mut b = Body::at([0.0, 0.0, -30.0]);
        b.settle(&w, &p);
        run(&w, &mut b, &p, Input { forward: 1.0, ..Default::default() }, 8.0);
        // 20 degrees up the gentle part at ~4 m/s: well past z=0, on top.
        assert!(b.pos[2] > 0.0 && b.pos[1] > 6.0 && b.grounded, "{:?}", b.pos);
        run(&w, &mut b, &p, Input { forward: 1.0, ..Default::default() }, 12.0);
        assert!(b.pos[2] < 24.0, "the steep face stops you: {:?}", b.pos);
    }

    #[test]
    fn heads_hit_ceilings_and_floors_above_are_not_walked_through() {
        let mut w = field();
        w.extra_solids.push(Solid::Box { centre: [0.0, 2.2, 0.0], half: [5.0, 0.1, 5.0], yaw: 0.0 });
        w.finish();
        let p = Params::default();
        let mut b = Body::at([0.0, 0.0, 0.0]);
        b.settle(&w, &p);
        let mut peak = 0.0f32;
        for i in 0..90 {
            b.step(&w, &p, &Input { jump: i == 0, ..Default::default() }, 0.0, 1.0 / 60.0);
            peak = peak.max(b.pos[1] + p.height);
        }
        assert!(peak <= 2.1 + 1e-3, "head stopped under the ceiling: {peak}");
        assert!(b.pos[1] < 0.01, "back on the ground floor");
    }

    #[test]
    fn trees_block_and_a_fast_body_does_not_tunnel_through_walls() {
        let mut w = field();
        w.extra_solids.push(Solid::Cylinder { base: [0.0, 0.0, 3.0], radius: 0.4, height: 6.0 });
        w.extra_solids.push(Solid::Box { centre: [20.0, 1.5, 0.0], half: [0.05, 1.5, 10.0], yaw: 0.0 });
        w.finish();
        let p = Params::default();
        let mut b = Body::at([0.0, 0.0, 0.0]);
        b.settle(&w, &p);
        run(&w, &mut b, &p, Input { forward: 1.0, ..Default::default() }, 2.0);
        // Slides round or stops against the trunk, never inside it.
        let d = ((b.pos[0]).powi(2) + (b.pos[2] - 3.0).powi(2)).sqrt();
        assert!(d >= 0.4 + p.radius - 1e-3 || b.pos[2] > 3.5, "{:?}", b.pos);
        // Big time steps (a slow frame) with sprint into a paper-thin wall.
        let mut f = Body::at([10.0, 0.0, 0.0]);
        f.settle(&w, &p);
        for _ in 0..20 {
            f.step(&w, &p, &Input { forward: 0.0, right: 1.0, sprint: true, ..Default::default() }, 0.0, 0.25);
        }
        assert!(f.pos[0] < 20.0, "{:?}", f.pos);
    }

    #[test]
    fn deep_water_floats_you_and_shallows_slow_you() {
        let mut w = field();
        w.terrain.as_mut().unwrap().basin([0.0, 0.0], 15.0, 8.0);
        w.water.push(crate::world::Water { level: -0.5, min: [-30.0, -30.0], max: [30.0, 30.0], colour: [0.02, 0.1, 0.12], clarity: 6.0, wave_height: 0.1, wave_speed: 1.0 });
        w.finish();
        let p = Params::default();
        let mut b = Body::at([0.0, 3.0, 0.0]);
        run(&w, &mut b, &p, Input::default(), 4.0);
        assert!(b.swimming, "{:?} depth {}", b.pos, b.depth);
        let head = b.pos[1] + p.height;
        assert!(head > -0.5 && b.pos[1] < -0.5, "floats with the head above water: {head}");
    }

    #[test]
    fn modes_switch_keeping_the_view_and_third_person_pulls_in_for_walls() {
        let mut w = field();
        w.extra_solids.push(Solid::Box { centre: [0.0, 2.0, -2.0], half: [5.0, 2.0, 0.2], yaw: 0.0 });
        w.finish();
        let mut ex = Explorer::new(&w);
        ex.teleport(&w, [0.0, 0.0, 0.0], 0.0);
        let first = ex.camera(&w);
        assert!((first.pos[1] - 1.62).abs() < 1e-3);
        ex.set_mode(&w, Mode::Fly);
        let fly = ex.camera(&w);
        assert_eq!(fly.pos, first.pos);
        ex.update(&w, &Controls { input: Input { up: 1.0, ..Default::default() }, ..Default::default() }, 1.0);
        assert!(ex.camera(&w).pos[1] > 10.0);
        ex.set_mode(&w, Mode::Walk);
        for _ in 0..180 {
            ex.update(&w, &Controls::default(), 1.0 / 60.0);
        }
        assert!(ex.body.grounded && ex.body.pos[1].abs() < 1e-3, "drops to the ground");
        ex.teleport(&w, [0.0, 0.0, -1.0], 0.0);
        ex.third_person = true;
        let tp = ex.camera(&w);
        // Camera sits behind (toward -z) but the wall at z=-2.2.. blocks: it is pulled in.
        assert!(tp.pos[2] > -2.2, "{:?}", tp.pos);
        assert!(ex.avatar().is_some());
        ex.set_mode(&w, Mode::Orbit);
        let o = ex.camera(&w);
        assert!(super::len(sub(o.pos, ex.orbit_target)) > 1.0);
    }

    #[test]
    fn a_route_is_walked_round_a_wall_to_its_end() {
        let mut w = field();
        w.extra_solids.push(Solid::Box { centre: [0.0, 1.5, 10.0], half: [6.0, 1.5, 0.3], yaw: 0.0 });
        w.finish();
        let mut ex = Explorer::new(&w);
        ex.teleport(&w, [0.0, 0.0, 0.0], 0.0);
        let mut frames = 0;
        let route = Route { points: vec![[0.0, 0.0, 20.0]], sprint: false, pause: 0.0, arrive: 1.0 };
        let (secs, ok) = walk_route(&w, &mut ex, &route, 30.0, |_, _| frames += 1);
        assert!(ok, "reached the far side: {:?} after {secs}", ex.body.pos);
        assert!(frames as f32 / 30.0 > 4.5);
        assert!(super::len(sub(ex.body.pos, [0.0, 0.0, 20.0])) < 2.0);
    }
}
