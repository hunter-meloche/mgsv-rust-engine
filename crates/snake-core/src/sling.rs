//! The rifle hanging on its sling at Snake's left hip (`SKL_300_ASRROOT`,
//! which carries the holstered rifle): it keeps the angle it is mounted at
//! on the waist (MGSV's `CNP_ASRROOT`: muzzle down and back, the stock at
//! the hip) and swings a little with Snake's movement, mostly side to side.
//!
//! MGSV simulates the bone (`sna0_main0_asr.sim`: one 5 kg body on a
//! ball-socket joint, its centre 13.6 cm from the joint, angular damping
//! 0.8, colliding with the left thigh). Left to hang under gravity, the
//! rifle stands up from the hip and its stock reaches into the shoulder
//! whenever Snake leans to run or crouch, which MGSV's rifle does not do;
//! here the body is held to its mounted angle by a spring instead, free to
//! swing 15 degrees side to side but only 8 degrees fore and aft.
//!
//! The centre of mass is a particle at a fixed distance from the animated
//! joint, stepped at 120 Hz in world space (the joint's movement swings
//! it), kept out of the left thigh, and the bone is turned from its mounted
//! direction to the simulated one.

use fox_anim::glam::{Quat, Vec3};
use fox_anim::{Pose, Skeleton};

/// The body's centre from the joint, in the bone's (the waist's) frame
/// (MGSV's `bodyOffsetPos`).
const CENTRE: Vec3 = Vec3::new(0.000222, -0.107529, -0.08332);
/// Swing allowed away from the mounted angle: side to side (about the
/// waist's forward axis) and fore and aft (about its lateral axis).
const SIDE_LIMIT: f32 = 15.0;
const PITCH_LIMIT: f32 = 8.0;
/// Pull back to the mounted angle (1/s^2: about 1.7 swings a second) and
/// damping (1/s).
const STIFFNESS: f32 = 120.0;
const DAMPING: f32 = 3.0;
/// MGSV's rifle capsule radius and the left thigh's hit capsule radius.
const RADIUS: f32 = 0.03;
const THIGH_RADIUS: f32 = 0.105;
const STEP: f32 = 1.0 / 120.0;

pub struct SlingSim {
    bone: usize,
    thigh: (usize, usize),
    x: Vec3,
    prev: Vec3,
    pivot: Vec3,
    leftover: f32,
    ready: bool,
}

/// Side and fore-aft angles of a downward direction `d` (bone frame).
fn angles(d: Vec3) -> (f32, f32) {
    (d.x.atan2(-d.y), d.z.atan2(-d.y))
}

impl SlingSim {
    /// None if `skel` has no rifle sling bone.
    pub fn new(skel: &Skeleton) -> Option<Self> {
        let find = |n: &str| skel.find(fox_hash::str_code32(n.as_bytes()));
        Some(Self {
            bone: find("SKL_300_ASRROOT")?,
            thigh: (find("SKL_030_LTHIGH")?, find("SKL_031_LLEG")?),
            x: Vec3::ZERO,
            prev: Vec3::ZERO,
            pivot: Vec3::ZERO,
            leftover: 0.0,
            ready: false,
        })
    }

    /// Advance by `dt` seconds with the character root at `root` (world
    /// position) turned by `turn`, and turn the sling bone in `pose` (root
    /// space) to the simulated rifle.
    pub fn step(&mut self, pose: &mut Pose, root: Vec3, turn: Quat, dt: f32) {
        let to_world = |p: Vec3| root + turn * p;
        let frame = turn * pose.rotations[self.bone];
        let pivot = to_world(pose.positions[self.bone]);
        let mounted = pivot + frame * CENTRE;
        if !self.ready || (pivot - self.pivot).length() > 1.0 {
            (self.x, self.prev, self.pivot, self.ready) = (mounted, mounted, pivot, true);
        }
        let (ta, tb) = (to_world(pose.positions[self.thigh.0]), to_world(pose.positions[self.thigh.1]));

        self.leftover = (self.leftover + dt.clamp(0.0, 0.25)).min(0.25);
        let steps = (self.leftover / STEP).floor() as usize;
        self.leftover -= steps as f32 * STEP;
        let start = self.pivot;
        let keep = (-DAMPING * STEP).exp();
        for s in 0..steps {
            let f = (s + 1) as f32 / steps as f32;
            self.pivot = start.lerp(pivot, f);
            let target = self.pivot + frame * CENTRE;
            let v = (self.x - self.prev) * keep;
            self.prev = self.x;
            self.x += v + (target - self.x) * STIFFNESS * STEP * STEP;
            self.solve(frame, ta, tb);
        }
        self.pivot = pivot;

        let from = frame * CENTRE.normalize();
        let to = (self.x - pivot).normalize_or_zero();
        if to != Vec3::ZERO {
            let world = Quat::from_rotation_arc(from, to) * frame;
            pose.rotations[self.bone] = (turn.inverse() * world).normalize();
        }
    }

    fn solve(&mut self, frame: Quat, ta: Vec3, tb: Vec3) {
        // Out of the left thigh.
        let ab = tb - ta;
        let t = ((self.x - ta).dot(ab) / ab.length_squared().max(1e-8)).clamp(0.0, 1.0);
        let off = self.x - (ta + ab * t);
        let min = THIGH_RADIUS + RADIUS;
        if off.length() < min && off.length() > 1e-6 {
            self.x = ta + ab * t + off * (min / off.length());
        }
        // At its distance from the joint, within its swing limits.
        let inv = frame.inverse();
        let d = (inv * (self.x - self.pivot)).normalize_or(CENTRE.normalize());
        let (side0, pitch0) = angles(CENTRE);
        let (side, pitch) = angles(d);
        let side = side.clamp(side0 - SIDE_LIMIT.to_radians(), side0 + SIDE_LIMIT.to_radians());
        let pitch = pitch.clamp(pitch0 - PITCH_LIMIT.to_radians(), pitch0 + PITCH_LIMIT.to_radians());
        let d = Vec3::new(side.tan(), -1.0, pitch.tan()).normalize();
        self.x = self.pivot + frame * d * CENTRE.length();
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

    fn angles_of(pose: &Pose, lean: Quat) -> (f32, f32) {
        let d = lean.inverse() * pose.rotations[1] * CENTRE.normalize();
        let ((s, p), (s0, p0)) = (angles(d), angles(CENTRE));
        ((s - s0).to_degrees(), (p - p0).to_degrees())
    }

    #[test]
    fn keeps_its_mount_and_swings_mostly_sideways() {
        let skel = skeleton();
        let mut sim = SlingSim::new(&skel).unwrap();
        // Standing still: at its mounted angle (the thigh nudges it out a
        // little).
        let mut pose = Pose::rest(&skel);
        for _ in 0..120 {
            pose = Pose::rest(&skel);
            sim.step(&mut pose, Vec3::ZERO, Quat::IDENTITY, 1.0 / 60.0);
        }
        let (side, pitch) = angles_of(&pose, Quat::IDENTITY);
        assert!(side.abs() < 4.0 && pitch.abs() < 2.0, "{side} {pitch}");
        // Leaning 60 degrees forward (a crouch): it leans with the waist
        // rather than standing up into the shoulder.
        let lean = Quat::from_rotation_x(60f32.to_radians());
        let (mut max_side, mut max_pitch) = (0.0f32, 0.0f32);
        let mut root = Vec3::ZERO;
        for i in 0..240 {
            pose = Pose::rest(&skel);
            for k in 0..2 {
                pose.rotations[k] = lean;
            }
            pose.positions[1] = Vec3::new(0.0, 1.0, 0.0) + lean * Vec3::new(0.207, 0.02, 0.178);
            // Running with a side-to-side sway.
            root.z += 3.5 / 60.0;
            root.x = 0.04 * (i as f32 * 0.4).sin();
            sim.step(&mut pose, root, Quat::IDENTITY, 1.0 / 60.0);
            let (s, p) = angles_of(&pose, lean);
            max_side = max_side.max(s.abs());
            max_pitch = max_pitch.max(p.abs());
        }
        assert!(max_side > 1.0, "no swing ({max_side})");
        assert!(max_side <= SIDE_LIMIT + 0.5 && max_pitch <= PITCH_LIMIT + 0.5, "{max_side} {max_pitch}");
    }
}
