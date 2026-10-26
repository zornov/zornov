// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Where a character is, bone by bone, at a moment: the key points (soles,
//! hands, head, eyes, gaze) in the world of the canvas, which of the feet
//! are down and which are sliding, and how far each hand reaches.
//!
//! The world here is the 3D layer's own: x right, y down, z away, in canvas
//! pixels. A model's point goes through the clip's turn about the vertical
//! (`rotation_y`), its scale and its place, exactly as the renderer draws
//! it. The posed frames come from the same rig evaluation the renderer
//! uses; this module only reads them.

use super::body::{Body, Limb, Side};
use crate::rig::{Rig, Xf};
use crate::{V3, add, dot, length, normalise, scale, sub};

/// The clip's place in the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipXf {
    /// `transform.position` x, y and `position_z`.
    pub pos: V3,
    /// `transform.rotation_y`, degrees.
    pub yaw_deg: f32,
    /// Uniform scale (`scale_x`).
    pub scale: f32,
}

impl ClipXf {
    pub fn world(&self, m: V3) -> V3 {
        let (s, c) = self.yaw_deg.to_radians().sin_cos();
        let k = self.scale;
        [self.pos[0] + k * (m[0] * c - m[2] * s), self.pos[1] + k * m[1], self.pos[2] + k * (m[0] * s + m[2] * c)]
    }

    pub fn world_dir(&self, m: V3) -> V3 {
        let (s, c) = self.yaw_deg.to_radians().sin_cos();
        [m[0] * c - m[2] * s, m[1], m[0] * s + m[2] * c]
    }

    /// A world point in the model's own pixels.
    pub fn model(&self, w: V3) -> V3 {
        let (s, c) = self.yaw_deg.to_radians().sin_cos();
        let (dx, dy, dz) = ((w[0] - self.pos[0]) / self.scale, (w[1] - self.pos[1]) / self.scale, (w[2] - self.pos[2]) / self.scale);
        [dx * c + dz * s, dy, -dx * s + dz * c]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FootPoint {
    pub leg: String,
    pub ankle: V3,
    pub toe: V3,
    /// The ground under the ankle: the ankle lowered by its rest height.
    pub sole: V3,
    /// Horizontal px/s over the neighbouring samples (filled by [`annotate`]).
    pub speed: f32,
    /// Standing on the ground (filled by [`annotate`]).
    pub contact: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HandPoint {
    pub side: Side,
    pub wrist: V3,
    /// The far end of the hand.
    pub tip: V3,
    pub shoulder: V3,
    pub elbow: V3,
    /// Shoulder to wrist, and the arm's length (both bones).
    pub reach: f32,
    pub arm_length: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Points {
    pub t: f32,
    pub root: V3,
    pub head: V3,
    pub eyes: V3,
    /// Unit, the way the eyes (or the head) face.
    pub gaze: V3,
    pub feet: Vec<FootPoint>,
    pub hands: Vec<HandPoint>,
    /// Every bone: name, head, tail.
    pub bones: Vec<(String, V3, V3)>,
}

/// The key points from posed frames.
pub fn points(rig: &Rig, body: &Body, world: &[Xf], xf: &ClipXf, t: f32) -> Points {
    let k = xf.scale;
    let tail = |i: usize| world[i].apply([0.0, rig.bones[i].length(), 0.0]);
    let w = |m: V3| xf.world(m);
    let bones: Vec<(String, V3, V3)> = rig.bones.iter().enumerate().map(|(i, b)| (b.name.clone(), w(world[i].pos), w(tail(i)))).collect();
    let rest = rig.rest_world();
    let sole_rest = body.sole_height(rig);
    let mut feet = Vec::new();
    for leg in &body.legs {
        let a = world[leg.end].pos;
        // The ankle's height above the sole at rest, in the world.
        let ankle_h = ((dot(rest[leg.end].pos, body.up) - sole_rest).abs()) * k;
        let ankle = w(a);
        let toe = w(leg.tip.map_or_else(|| tail(leg.end), tail));
        feet.push(FootPoint { leg: leg.name.clone(), ankle, toe, sole: sub(ankle, scale(body.up, ankle_h)), speed: 0.0, contact: false });
    }
    let mut hands = Vec::new();
    for arm in &body.arms {
        let (sh, el, wr) = (w(world[arm.upper].pos), w(world[arm.lower].pos), w(world[arm.end].pos));
        hands.push(HandPoint { side: arm.side, wrist: wr, tip: w(tail(arm.end)), shoulder: sh, elbow: el, reach: length(sub(wr, sh)), arm_length: Body::limb_length(rig, arm) * k });
    }
    let head = body.head.map_or(w(world[body.hips].pos), |h| w(world[h].pos));
    // Eyes: their middle; a head without eyes looks from above its centre.
    let eye_m = if body.eyes.is_empty() {
        body.head.map_or(head, |h| w(add(world[h].pos, scale(sub(tail(h), world[h].pos), 0.55))))
    } else {
        let sum = body.eyes.iter().map(|&e| world[e].pos).fold([0.0; 3], add);
        w(scale(sum, 1.0 / body.eyes.len() as f32))
    };
    let gaze_bone = body.eyes.first().copied().or(body.head);
    let gaze = gaze_bone.map_or(xf.world_dir(body.fwd), |b| {
        let rest_rot = rest[b].rot;
        let r = crate::rig::q_mul(world[b].rot, crate::rig::q_conj(rest_rot));
        xf.world_dir(crate::rig::q_rotate(r, body.fwd))
    });
    Points { t, root: w(world[body.root].pos), head, eyes: eye_m, gaze: normalise(gaze), feet, hands, bones }
}

/// Fills in each foot's speed and whether it is on the ground, over a
/// series of samples. A foot is down when it is as low as the lowest foot
/// has been around that moment and not rising, which holds on slopes and
/// steps where a fixed ground line would not.
pub fn annotate(series: &mut [Points], ell: f32, up_y: f32) {
    let n = series.len();
    if n == 0 {
        return;
    }
    let nf = series[0].feet.len();
    for li in 0..nf {
        for i in 0..n {
            let (a, b) = (i.saturating_sub(1), (i + 1).min(n - 1));
            let dt = (series[b].t - series[a].t).max(1e-4);
            let (pa, pb) = (series[a].feet[li].ankle, series[b].feet[li].ankle);
            series[i].feet[li].speed = ((pb[0] - pa[0]).powi(2) + (pb[2] - pa[2]).powi(2)).sqrt() / dt;
        }
    }
    // The ground under each foot: the lowest this foot has been within half
    // a second either side.
    let times: Vec<f32> = series.iter().map(|p| p.t).collect();
    let window = |i: usize| -> (usize, usize) {
        let mut a = i;
        while a > 0 && times[i] - times[a - 1] <= 0.5 {
            a -= 1;
        }
        let mut b = i;
        while b + 1 < n && times[b + 1] - times[i] <= 0.5 {
            b += 1;
        }
        (a, b)
    };
    for li in 0..nf {
        for i in 0..n {
            let (a, b) = window(i);
            let low = (a..=b).map(|j| up_y * series[j].feet[li].ankle[1]).fold(f32::INFINITY, f32::min);
            // Ground for a body that is on one level: the lowest of all feet.
            let all_low = (a..=b).flat_map(|j| series[j].feet.iter().map(|f| up_y * f.ankle[1])).fold(f32::INFINITY, f32::min);
            let here = up_y * series[i].feet[li].ankle[1];
            let ground = low.min(all_low + 0.12 * ell);
            // The lowest foot at that moment, close to the ground, is the one holding the body up.
            let lowest_now = series[i].feet.iter().map(|f| up_y * f.ankle[1]).fold(f32::INFINITY, f32::min);
            series[i].feet[li].contact = here <= ground + 0.02 * ell || (here <= lowest_now + 1e-3 && here <= all_low + 0.08 * ell);
        }
    }
}

/// The compact words for a hand's reach.
pub fn stretch_of(h: &HandPoint) -> f32 {
    if h.arm_length > 1e-6 { h.reach / h.arm_length } else { 0.0 }
}

pub fn limb_by_name<'a>(body: &'a Body, name: &str) -> Option<&'a Limb> {
    body.legs.iter().chain(body.arms.iter()).find(|l| l.name.eq_ignore_ascii_case(name))
}
