//! The rifle hanging on its sling at Snake's left hip (`SKL_300_ASRROOT`,
//! which carries the holstered rifle), simulated as MGSV simulates it rather
//! than fixed to the waist (fixed, crouching tips it out behind him).
//!
//! Parameters from MGSV's `sna0_main0_asr.sim` (`SimOnPhysics`
//! "sna0_main0_asr"): one 5 kg body on a ball-socket joint at the bone,
//! centred 13.6 cm below and behind it (the rifle's centre of mass), angular
//! damping 0.8; the joint keeps the body's reference direction within 35
//! degrees of the waist's (down and 28 degrees back), twist stopped; it
//! collides with the left thigh (and the chest, upper arm and head, which a
//! hanging rifle does not reach); `SimInertialControl` coefficient 0: none
//! of the character's own movement reaches it as inertia, it swings only
//! with his posture.
//!
//! Here the centre of mass is a particle on a fixed distance from the
//! animated pivot, stepped at 120 Hz in the character's root space (all of
//! the root's movement carries it), and the bone is turned from its
//! animated direction to the simulated one.

use fox_anim::glam::{Quat, Vec3};
use fox_anim::{Pose, Skeleton};

/// The body's centre in the bone's frame (MGSV's `bodyOffsetPos`).
const CENTRE: Vec3 = Vec3::new(0.000222, -0.107529, -0.08332);
/// The joint's reference directions (MGSV's `refA` on the waist and `refB`
/// on the rifle, both in the rest pose's frame) and its limit.
const REF_WAIST: Vec3 = Vec3::new(0.0, -0.882947, -0.469472);
const REF_RIFLE: Vec3 = Vec3::new(0.321393, -0.55667, -0.766044);
const LIMIT: f32 = 35.0;
const DAMPING: f32 = 0.8; // per second
/// MGSV's rifle capsule radius and the left thigh's hit capsule radius.
const RADIUS: f32 = 0.03;
const THIGH_RADIUS: f32 = 0.105;
const STEP: f32 = 1.0 / 120.0;
const GRAVITY: Vec3 = Vec3::new(0.0, -9.8, 0.0);

pub struct SlingSim {
    bone: usize,
    waist: usize,
    thigh: (usize, usize),
    /// The limit's axis for the centre's direction: the waist's reference
    /// turned as the rifle's reference turns into the centre's direction.
    axis: Vec3,
    x: Vec3,
    prev: Vec3,
    pivot: Vec3,
    leftover: f32,
    ready: bool,
}

impl SlingSim {
    /// None if `skel` has no rifle sling bone.
    pub fn new(skel: &Skeleton) -> Option<Self> {
        let find = |n: &str| skel.find(fox_hash::str_code32(n.as_bytes()));
        let centre = CENTRE.normalize();
        Some(Self {
            bone: find("SKL_300_ASRROOT")?,
            waist: find("SKL_000_WAIST")?,
            thigh: (find("SKL_030_LTHIGH")?, find("SKL_031_LLEG")?),
            axis: Quat::from_rotation_arc(REF_RIFLE.normalize(), centre) * REF_WAIST.normalize(),
            x: Vec3::ZERO,
            prev: Vec3::ZERO,
            pivot: Vec3::ZERO,
            leftover: 0.0,
            ready: false,
        })
    }

    /// Advance by `dt` seconds and turn the sling bone in `pose` (root
    /// space) to the simulated rifle.
    pub fn step(&mut self, pose: &mut Pose, dt: f32) {
        let rot = pose.rotations[self.bone];
        let pivot = pose.positions[self.bone];
        let animated = pivot + rot * CENTRE;
        let len = CENTRE.length();
        if !self.ready || (pivot - self.pivot).length() > 0.5 {
            (self.x, self.prev, self.pivot, self.ready) = (animated, animated, pivot, true);
        }
        let axis = (pose.rotations[self.waist] * self.axis).normalize();
        let (ta, tb) = (pose.positions[self.thigh.0], pose.positions[self.thigh.1]);

        self.leftover = (self.leftover + dt.clamp(0.0, 0.25)).min(0.25);
        let steps = (self.leftover / STEP).floor() as usize;
        self.leftover -= steps as f32 * STEP;
        let start = self.pivot;
        let keep = (-DAMPING * STEP).exp();
        for s in 0..steps {
            // Carried by all of the pivot's movement (no inertia).
            let at = start.lerp(pivot, (s + 1) as f32 / steps as f32);
            let carry = at - self.pivot;
            self.pivot = at;
            self.x += carry;
            self.prev += carry;
            let v = (self.x - self.prev) * keep;
            self.prev = self.x;
            self.x += v + GRAVITY * STEP * STEP;
            for _ in 0..4 {
                self.solve(axis, ta, tb, len);
            }
        }
        self.pivot = pivot;

        let from = (animated - pivot).normalize_or_zero();
        let to = (self.x - pivot).normalize_or_zero();
        if from != Vec3::ZERO && to != Vec3::ZERO {
            pose.rotations[self.bone] = (Quat::from_rotation_arc(from, to) * rot).normalize();
        }
    }

    fn solve(&mut self, axis: Vec3, ta: Vec3, tb: Vec3, len: f32) {
        // On its distance from the pivot, within the joint's cone.
        let mut dir = (self.x - self.pivot).normalize_or(axis);
        let angle = axis.angle_between(dir);
        let max = LIMIT.to_radians();
        if angle > max {
            let (turn, _) = Quat::from_rotation_arc(axis, dir).to_axis_angle();
            dir = Quat::from_axis_angle(turn, max) * axis;
        }
        self.x = self.pivot + dir * len;
        // Out of the left thigh and above the ground (the root is on it).
        let ab = tb - ta;
        let t = ((self.x - ta).dot(ab) / ab.length_squared().max(1e-8)).clamp(0.0, 1.0);
        let off = self.x - (ta + ab * t);
        let min = THIGH_RADIUS + RADIUS;
        if off.length() < min && off.length() > 1e-6 {
            self.x = ta + ab * t + off * (min / off.length());
        }
        self.x.y = self.x.y.max(RADIUS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fox_anim::skeleton::Bone;

    /// A waist 1 m up with the sling bone at Snake's left hip and a left leg.
    fn skeleton() -> Skeleton {
        let bone = |name: &str, parent: Option<usize>, p: Vec3| Bone {
            name: fox_hash::str_code32(name.as_bytes()),
            parent,
            rest_local: Vec3::ZERO,
            rest_model: p,
        };
        Skeleton {
            bones: vec![
                bone("SKL_000_WAIST", None, Vec3::new(0.0, 1.0, 0.0)),
                bone("SKL_300_ASRROOT", Some(0), Vec3::new(0.207, 1.02, 0.178)),
                bone("SKL_030_LTHIGH", Some(0), Vec3::new(0.096, 0.905, 0.028)),
                bone("SKL_031_LLEG", Some(2), Vec3::new(0.096, 0.476, 0.006)),
            ],
        }
    }

    #[test]
    fn hangs_within_its_cone_and_follows_a_lean() {
        let skel = skeleton();
        let mut sim = SlingSim::new(&skel).unwrap();
        let mut pose = Pose::rest(&skel);
        for _ in 0..240 {
            pose = Pose::rest(&skel);
            sim.step(&mut pose, 1.0 / 60.0);
        }
        // Standing: hanging down, within 35 degrees of the waist's reference.
        let centre = |pose: &Pose| pose.rotations[1] * CENTRE.normalize();
        assert!(centre(&pose).y < -0.6, "{:?}", centre(&pose));
        assert!(centre(&pose).angle_between(sim.axis).to_degrees() <= LIMIT + 0.5);
        // Leaning 60 degrees forward (a crouch): fixed to the waist the
        // rifle would tip back and up; it still hangs down.
        let lean = Quat::from_rotation_x(60f32.to_radians());
        for _ in 0..240 {
            pose = Pose::rest(&skel);
            for k in 0..2 {
                pose.rotations[k] = lean;
            }
            pose.positions[1] = Vec3::new(0.0, 1.0, 0.0) + lean * (Vec3::new(0.207, 1.02, 0.178) - Vec3::new(0.0, 1.0, 0.0));
            sim.step(&mut pose, 1.0 / 60.0);
        }
        let fixed = (lean * CENTRE.normalize()).y;
        assert!(centre(&pose).y < fixed - 0.2, "{} vs fixed {fixed}", centre(&pose).y);
    }
}
