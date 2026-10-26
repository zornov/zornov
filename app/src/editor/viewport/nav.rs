// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The editor's own eye in the 3D workspace, apart from any scene camera:
//! where it looks from, what it looks at, and the moves a 3D application
//! makes with it (orbit, pan, dolly, zoom to the pointer, fly, walk,
//! frame, the axis views, perspective and orthographic, smooth moves
//! between views).
//!
//! World units are the canvas's: x right and y down in canvas pixels from
//! the canvas centre, z away from the default camera. The eye is held as
//! a point it turns round (the focus point) with a heading, a pitch and a
//! distance, so orbiting and framing never drift. Heading 0 looks into
//! the canvas (+z), a positive heading turns right; a positive pitch looks
//! down from above.

use serde::{Deserialize, Serialize};
use studio_model::space as sp;

/// The orthographic view is drawn as a very long lens from very far off
/// (the renderer has one projection), so it is flat to within a few
/// percent at any depth that matters.
pub const ORTHO_FAR: f64 = 40.0;

/// Nearest the eye goes to its focus point (canvas pixels).
pub const MIN_DISTANCE: f64 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Nav {
    /// The point the eye orbits and looks at.
    pub target: [f64; 3],
    /// Degrees; 0 looks along +z, positive turns right.
    pub yaw: f64,
    /// Degrees in -90..=90; positive looks down.
    pub pitch: f64,
    /// From the eye to the target, canvas pixels.
    pub distance: f64,
    /// Focal length on a 36 mm frame.
    pub lens: f64,
    pub ortho: bool,
}

impl Default for Nav {
    fn default() -> Nav {
        Nav { target: [0.0; 3], yaw: 0.0, pitch: 0.0, distance: 2667.0, lens: 50.0, ortho: false }
    }
}

/// The named views.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
}

impl Axis {
    pub const ALL: [Axis; 6] = [Axis::Front, Axis::Back, Axis::Left, Axis::Right, Axis::Top, Axis::Bottom];

    pub fn name(self) -> &'static str {
        match self {
            Axis::Front => "front",
            Axis::Back => "back",
            Axis::Left => "left",
            Axis::Right => "right",
            Axis::Top => "top",
            Axis::Bottom => "bottom",
        }
    }

    pub fn parse(s: &str) -> Option<Axis> {
        Axis::ALL.into_iter().find(|a| a.name() == s.trim().to_lowercase())
    }

    /// Heading and pitch looking along this axis.
    pub fn angles(self) -> (f64, f64) {
        match self {
            Axis::Front => (0.0, 0.0),
            Axis::Back => (180.0, 0.0),
            // From the left side, looking right (+x).
            Axis::Left => (90.0, 0.0),
            Axis::Right => (-90.0, 0.0),
            Axis::Top => (0.0, 90.0),
            Axis::Bottom => (0.0, -90.0),
        }
    }
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn mul(a: [f64; 3], k: f64) -> [f64; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn len(a: [f64; 3]) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// Wraps degrees into -180..180.
pub fn wrap(deg: f64) -> f64 {
    let d = (deg + 180.0).rem_euclid(360.0) - 180.0;
    if d == -180.0 { 180.0 } else { d }
}

impl Nav {
    /// A three-quarter view of a canvas `width` x `height` pixels, a
    /// little above and to the left, the whole canvas in sight.
    pub fn three_quarter(width: f64, height: f64) -> Nav {
        let mut n = Nav { yaw: 28.0, pitch: 18.0, ..Nav::default() };
        n.frame([0.0; 3], 0.5 * (width * width + height * height).sqrt(), width, width / height.max(1.0));
        n
    }

    /// Straight on, as the default camera sees the canvas.
    pub fn front(width: f64) -> Nav {
        Nav { distance: sp::default_zoom(width), ..Nav::default() }
    }

    /// Unit direction the eye looks.
    pub fn forward(&self) -> [f64; 3] {
        let (sy, cy) = self.yaw.to_radians().sin_cos();
        let (sp_, cp) = self.pitch.to_radians().sin_cos();
        [cp * sy, sp_, cp * cy]
    }

    /// Unit screen-right direction (always level).
    pub fn right(&self) -> [f64; 3] {
        let (sy, cy) = self.yaw.to_radians().sin_cos();
        [cy, 0.0, -sy]
    }

    /// Unit screen-down direction.
    pub fn down(&self) -> [f64; 3] {
        cross(self.forward(), self.right())
    }

    /// Rows: right, down, forward (as `View::axes`).
    pub fn axes(&self) -> sp::Mat3 {
        [self.right(), self.down(), self.forward()]
    }

    /// Where the eye is.
    pub fn position(&self) -> [f64; 3] {
        sub(self.target, mul(self.forward(), self.distance))
    }

    /// The lens as the renderer's zoom for a canvas `width` wide.
    pub fn zoom(&self, width: f64) -> f64 {
        sp::zoom_from_focal_length(self.lens, width)
    }

    /// The view to render and draw with. Orthographic is a lens from far
    /// off with the same size at the target.
    pub fn view(&self, width: f64) -> sp::View {
        let k = if self.ortho { ORTHO_FAR } else { 1.0 };
        sp::View { position: sub(self.target, mul(self.forward(), self.distance * k)), axes: self.axes(), zoom: self.zoom(width) * k }
    }

    /// World pixels per canvas pixel at the target's depth.
    pub fn world_per_pixel(&self, width: f64) -> f64 {
        self.distance / self.zoom(width).max(1e-9)
    }

    /// Turns round the target.
    pub fn orbit(&mut self, d_yaw: f64, d_pitch: f64) {
        self.yaw = wrap(self.yaw + d_yaw);
        self.pitch = (self.pitch + d_pitch).clamp(-90.0, 90.0);
    }

    /// Turns round `pivot` (the selection, say): the target swings with
    /// the eye so what is under the pivot stays put on screen.
    pub fn orbit_about(&mut self, pivot: [f64; 3], d_yaw: f64, d_pitch: f64) {
        let before = self.axes();
        let eye = self.position();
        self.orbit(d_yaw, d_pitch);
        let after = self.axes();
        // The eye's offset from the pivot, in the old view's frame, kept
        // in the new one.
        let rel = sub(eye, pivot);
        let local = sp::apply(&before, rel);
        let eye2 = add(pivot, sp::apply(&sp::transpose(&after), local));
        self.target = add(eye2, mul(self.forward(), self.distance));
    }

    /// Slides across the view by canvas pixels (the world under the
    /// pointer follows it at the target's depth).
    pub fn pan(&mut self, dx: f64, dy: f64, width: f64) {
        let k = self.world_per_pixel(width);
        let shift = add(mul(self.right(), -dx * k), mul(self.down(), -dy * k));
        self.target = add(self.target, shift);
    }

    /// Moves the eye nearer (factor < 1) or further from the target.
    pub fn dolly(&mut self, factor: f64) {
        self.distance = (self.distance * factor).clamp(MIN_DISTANCE, 1e7);
    }

    /// Dollies towards the point under `at` (canvas pixels from the
    /// screen centre), so what is under the pointer stays under it.
    pub fn zoom_at(&mut self, factor: f64, at: [f64; 2], width: f64) {
        let k = self.world_per_pixel(width);
        let under = add(self.target, add(mul(self.right(), at[0] * k), mul(self.down(), at[1] * k)));
        let before = self.distance;
        self.dolly(factor);
        let f = self.distance / before;
        self.target = add(under, mul(sub(self.target, under), f));
    }

    /// Moves eye and target together: `by` is right, down and forward in
    /// the eye's frame (canvas pixels).
    pub fn travel(&mut self, by: [f64; 3]) {
        let w = add(add(mul(self.right(), by[0]), mul(self.down(), by[1])), mul(self.forward(), by[2]));
        self.target = add(self.target, w);
    }

    /// Walking: forward and right stay level, `up` is straight up.
    pub fn walk(&mut self, forward: f64, right: f64, up: f64) {
        let (sy, cy) = self.yaw.to_radians().sin_cos();
        let level = [sy, 0.0, cy];
        let w = add(add(mul(level, forward), mul(self.right(), right)), [0.0, -up, 0.0]);
        self.target = add(self.target, w);
    }

    /// Looks round from where the eye is (first person): the target moves.
    pub fn look(&mut self, d_yaw: f64, d_pitch: f64) {
        let eye = self.position();
        self.yaw = wrap(self.yaw + d_yaw);
        self.pitch = (self.pitch + d_pitch).clamp(-89.0, 89.0);
        self.target = add(eye, mul(self.forward(), self.distance));
    }

    /// Fits a sphere at `centre` of `radius` in a view of `aspect`
    /// (width / height), keeping the direction.
    pub fn frame(&mut self, centre: [f64; 3], radius: f64, width: f64, aspect: f64) {
        let zoom = self.zoom(width);
        // Half the narrower side of the view, in canvas pixels.
        let half = 0.5 * width.min(width / aspect.max(1e-6));
        let r = radius.max(1.0) * 1.15;
        self.target = centre;
        self.distance = (zoom * r / half.max(1.0)).max(MIN_DISTANCE);
        if !self.ortho {
            // Not inside the sphere.
            self.distance = self.distance.max(r * 1.05);
        }
    }

    /// Looks along an axis; the axis views are orthographic, as in 3D
    /// applications.
    pub fn set_axis(&mut self, a: Axis) {
        let (y, p) = a.angles();
        self.yaw = y;
        self.pitch = p;
        self.ortho = true;
    }

    /// The eye of a scene camera: same place and direction, the target a
    /// lens's distance ahead (or at the camera's point of interest).
    pub fn from_view(v: &sp::View, width: f64, poi: Option<[f64; 3]>) -> Nav {
        let f = v.axes[2];
        let pitch = f[1].clamp(-1.0, 1.0).asin().to_degrees();
        let yaw = f[0].atan2(f[2]).to_degrees();
        let distance = poi.map(|p| len(sub(p, v.position))).filter(|d| *d > MIN_DISTANCE).unwrap_or(v.zoom);
        let lens = sp::focal_length_from_zoom(v.zoom, width).clamp(4.0, 2000.0);
        Nav { target: add(v.position, mul(f, distance)), yaw, pitch, distance, lens, ortho: false }
    }

    /// In between `a` (t = 0) and `b` (t = 1): the short way round, the
    /// distance on a log scale, eased.
    pub fn lerp(a: &Nav, b: &Nav, t: f64) -> Nav {
        let t = t.clamp(0.0, 1.0);
        if t >= 1.0 {
            return *b;
        }
        let e = t * t * (3.0 - 2.0 * t);
        let l = |x: f64, y: f64| x + (y - x) * e;
        Nav {
            target: [l(a.target[0], b.target[0]), l(a.target[1], b.target[1]), l(a.target[2], b.target[2])],
            yaw: wrap(a.yaw + wrap(b.yaw - a.yaw) * e),
            pitch: l(a.pitch, b.pitch),
            distance: (a.distance.max(1e-6).ln() + (b.distance.max(1e-6).ln() - a.distance.max(1e-6).ln()) * e).exp(),
            lens: l(a.lens, b.lens),
            ortho: if t >= 1.0 { b.ortho } else { a.ortho && b.ortho },
        }
    }

    /// Screen point (canvas pixels from the centre of a view `width`
    /// wide) of a world point, and its depth; `None` behind the eye.
    pub fn project(&self, p: [f64; 3], width: f64) -> Option<([f64; 2], f64)> {
        self.view(width).project(p).map(|(c, d)| ([c.x, c.y], d))
    }
}

/// A smooth move between two views over `seconds`.
#[derive(Clone, Copy, Debug)]
pub struct Glide {
    pub from: Nav,
    pub to: Nav,
    pub start: f64,
    pub seconds: f64,
}

impl Glide {
    /// The view at `now`, and whether the move is over.
    pub fn at(&self, now: f64) -> (Nav, bool) {
        let t = ((now - self.start) / self.seconds.max(1e-6)).clamp(0.0, 1.0);
        (Nav::lerp(&self.from, &self.to, t), t >= 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        len(sub(a, b)) < 1e-6 * (1.0 + len(a))
    }

    #[test]
    fn the_front_view_is_the_default_camera() {
        let n = Nav::front(1920.0);
        let v = n.view(1920.0);
        let d = sp::View::default_for(1920.0);
        assert!(close(v.position, d.position));
        assert!((v.zoom - d.zoom).abs() < 1e-6 * d.zoom);
        for r in 0..3 {
            assert!(close(v.axes[r], d.axes[r]));
        }
    }

    #[test]
    fn axes_stay_square_at_every_angle_including_straight_down() {
        for (y, p) in [(0.0, 90.0), (37.0, -90.0), (-120.0, 45.0), (180.0, 0.0)] {
            let n = Nav { yaw: y, pitch: p, ..Nav::default() };
            let a = n.axes();
            for i in 0..3 {
                assert!((len(a[i]) - 1.0).abs() < 1e-9);
                for j in 0..i {
                    let d = a[i][0] * a[j][0] + a[i][1] * a[j][1] + a[i][2] * a[j][2];
                    assert!(d.abs() < 1e-9, "{y} {p}");
                }
            }
        }
        // From the top the eye is above (y negative is up) the target.
        let mut n = Nav::default();
        n.set_axis(Axis::Top);
        assert!(n.position()[1] < -1000.0);
        assert!(n.ortho);
        // From the right it sits on +x.
        n.set_axis(Axis::Right);
        assert!(n.position()[0] > 1000.0);
    }

    #[test]
    fn orbit_pan_dolly_and_zoom_to_the_pointer() {
        let mut n = Nav::front(1000.0);
        let d0 = n.distance;
        n.orbit(90.0, 0.0);
        assert!((len(sub(n.position(), n.target)) - d0).abs() < 1e-6);
        // Looking +x now, from -x.
        assert!(n.position()[0] < -d0 * 0.99);
        n.pan(100.0, 0.0, 1000.0);
        // Dragging right moves the world right: the target goes left on
        // screen (towards -right).
        assert!(n.target[2] > 0.0);
        let t0 = n.target;
        n.dolly(0.5);
        assert!((n.distance - d0 * 0.5).abs() < 1e-6);
        assert!(close(n.target, t0));
        // Zooming at the screen centre is a dolly.
        let mut m = Nav::front(1000.0);
        m.zoom_at(0.5, [0.0, 0.0], 1000.0);
        assert!(close(m.target, [0.0; 3]));
        // Zooming at a point keeps that point under the pointer.
        let mut m = Nav::front(1000.0);
        let world = [200.0, 100.0, 0.0];
        let before = m.project(world, 1000.0).unwrap().0;
        m.zoom_at(0.5, before, 1000.0);
        let after = m.project(world, 1000.0).unwrap().0;
        assert!((before[0] - after[0]).abs() < 1e-6 && (before[1] - after[1]).abs() < 1e-6, "{before:?} {after:?}");
    }

    #[test]
    fn looking_round_keeps_the_eye_and_walking_stays_level() {
        let mut n = Nav { yaw: 10.0, pitch: 30.0, ..Nav::default() };
        let eye = n.position();
        n.look(45.0, -10.0);
        assert!(close(n.position(), eye));
        let y = n.position()[1];
        n.walk(500.0, 120.0, 0.0);
        assert!((n.position()[1] - y).abs() < 1e-6);
        n.walk(0.0, 0.0, 50.0);
        assert!((n.position()[1] - (y - 50.0)).abs() < 1e-6);
    }

    #[test]
    fn orbiting_about_a_pivot_keeps_it_on_screen() {
        let mut n = Nav::three_quarter(1920.0, 1080.0);
        let pivot = [300.0, -50.0, 200.0];
        let before = n.project(pivot, 1920.0).unwrap().0;
        n.orbit_about(pivot, 30.0, -12.0);
        let after = n.project(pivot, 1920.0).unwrap().0;
        assert!((before[0] - after[0]).abs() < 1e-6 && (before[1] - after[1]).abs() < 1e-6, "{before:?} {after:?}");
    }

    #[test]
    fn framing_fits_and_a_camera_round_trips() {
        let mut n = Nav { yaw: -40.0, pitch: 20.0, ..Nav::default() };
        n.frame([100.0, 0.0, 500.0], 300.0, 1920.0, 16.0 / 9.0);
        // The sphere's edge is inside the view's height.
        let up = [100.0, -300.0, 500.0];
        let p = n.project(up, 1920.0).unwrap().0;
        assert!(p[1].abs() < 540.0);
        let v = n.view(1920.0);
        let back = Nav::from_view(&v, 1920.0, Some(n.target));
        assert!((back.yaw - n.yaw).abs() < 1e-6 && (back.pitch - n.pitch).abs() < 1e-6);
        assert!(close(back.target, n.target));
        assert!((back.lens - n.lens).abs() < 1e-6);
    }

    #[test]
    fn orthographic_keeps_size_at_the_target_and_glides_end_where_asked() {
        let mut n = Nav::front(1000.0);
        let w = [100.0, 0.0, 0.0];
        let persp = n.project(w, 1000.0).unwrap().0;
        n.ortho = true;
        let ortho = n.project(w, 1000.0).unwrap().0;
        assert!((persp[0] - ortho[0]).abs() < 1e-6);
        // Far behind and in front look nearly the same size.
        let deep = n.project([100.0, 0.0, 1000.0], 1000.0).unwrap().0;
        assert!((deep[0] - ortho[0]).abs() / ortho[0] < 0.05);
        let a = Nav { yaw: 170.0, ..Nav::default() };
        let b = Nav { yaw: -170.0, distance: 100.0, ..Nav::default() };
        let mid = Nav::lerp(&a, &b, 0.5);
        // The short way round: through 180, not 0.
        assert!(mid.yaw.abs() > 170.0);
        let g = Glide { from: a, to: b, start: 1.0, seconds: 0.25 };
        assert_eq!(g.at(2.0), (b, true));
        assert!(!g.at(1.1).1);
    }
}
