//! Secondary motion of the strap hanging at Snake's back (`SKL_310_BELT_A`
//! .. `SKL_313_BELT_D`), which MGSV simulates rather than animates.
//!
//! Parameters from MGSV's own simulation of it,
//! `sna0_main0_body.sim` (`SimOnPhysics` "sna0_main0_body"): four capsule
//! links (0.2, 0.2, 0.2 and 0.8 kg; linear damping 0.1, angular 0.8) on
//! ball-socket joints, the first limited to 35 degrees from hanging slightly
//! back, the others to 90 degrees from their parent link; it collides with
//! capsules on the waist and both thighs and shins; and a
//! `SimInertialControl` coefficient of 0.6 (60% of the character's own
//! movement reaches the strap as inertia, the rest carries it along).
//!
//! Here it is a position-based chain of five particles (the four joints
//! and the tip) stepped at 120 Hz in world space, with the top joint pinned
//! to the animated waist; the result is written back into the pose as the
//! four bones' rotations and positions.

use fox_anim::glam::{Quat, Vec3};
use fox_anim::{Pose, Skeleton};

const BONES: [&str; 4] = ["SKL_310_BELT_A", "SKL_311_BELT_B", "SKL_312_BELT_C", "SKL_313_BELT_D"];
/// Particle masses (kg): joints B, C, D, tip (MGSV's link masses; the
/// heavy last link is a buckle).
const MASS: [f32; 4] = [0.2, 0.2, 0.2, 0.8];
/// Tip below the last joint (m): MGSV's last link is centred 2.2 cm below
/// it and 2.3 cm in half-length.
const TIP: f32 = 0.0457;
const INERTIA: f32 = 0.6;
const DAMPING: f32 = 0.9; // per second, of the particles' velocity
const FIRST_LIMIT: f32 = 35.0;
const LIMIT: f32 = 90.0;
/// MGSV's reference direction of the first joint, in the waist's frame:
/// down and slightly back.
const FIRST_REF: Vec3 = Vec3::new(0.0, -0.998027, -0.062791);
const RADIUS: f32 = 0.024; // link radius (MGSV: 0.023 - 0.025)
const STEP: f32 = 1.0 / 120.0;
const GRAVITY: Vec3 = Vec3::new(0.0, -9.8, 0.0);

/// A collision capsule between two bones' joints (or around one).
struct Capsule {
    a: usize,
    b: usize,
    radius: f32,
}

pub struct StrapSim {
    bones: [usize; 4],
    waist: usize,
    capsules: Vec<Capsule>,
    lengths: [f32; 4],
    x: [Vec3; 5],
    prev: [Vec3; 5],
    anchor: Vec3,
    leftover: f32,
    ready: bool,
}

impl StrapSim {
    /// None if `skel` has no strap.
    pub fn new(skel: &Skeleton) -> Option<Self> {
        let find = |n: &str| skel.find(fox_hash::str_code32(n.as_bytes()));
        let bones = [find(BONES[0])?, find(BONES[1])?, find(BONES[2])?, find(BONES[3])?];
        let waist = find("SKL_000_WAIST")?;
        let rest = |i: usize| skel.bones[i].rest_model;
        let mut lengths = [TIP; 4];
        for k in 0..3 {
            lengths[k] = (rest(bones[k + 1]) - rest(bones[k])).length();
        }
        let mut capsules = Vec::new();
        // MGSV's hit bodies: thighs (r 0.117) and shins (r 0.1), hip to
        // knee and knee to ankle.
        for (from, to, radius) in [
            ("SKL_030_LTHIGH", "SKL_031_LLEG", 0.117),
            ("SKL_031_LLEG", "SKL_032_LFOOT", 0.1),
            ("SKL_040_RTHIGH", "SKL_041_RLEG", 0.117),
            ("SKL_041_RLEG", "SKL_042_RFOOT", 0.1),
        ] {
            if let (Some(a), Some(b)) = (find(from), find(to)) {
                capsules.push(Capsule { a, b, radius });
            }
        }
        Some(Self {
            bones,
            waist,
            capsules,
            lengths,
            x: [Vec3::ZERO; 5],
            prev: [Vec3::ZERO; 5],
            anchor: Vec3::ZERO,
            leftover: 0.0,
            ready: false,
        })
    }

    /// Joint and tip positions of the animated pose (model space).
    fn animated(&self, pose: &Pose) -> [Vec3; 5] {
        let p = |k: usize| pose.positions[self.bones[k]];
        let tip = p(3) + pose.rotations[self.bones[3]] * Vec3::new(0.0, -TIP, 0.0);
        [p(0), p(1), p(2), p(3), tip]
    }

    /// Advance by `dt` seconds with the character root at `root` (world
    /// position, on the ground) turned by `turn`, and write the strap into
    /// `pose` (model space).
    pub fn step(&mut self, pose: &mut Pose, root: Vec3, turn: Quat, dt: f32) {
        let to_world = |p: Vec3| root + turn * p;
        let animated = self.animated(pose);
        let world: Vec<Vec3> = animated.iter().map(|&p| to_world(p)).collect();
        // Start (or restart after a jump, e.g. a teleport) hanging as animated.
        if !self.ready || (world[0] - self.anchor).length() > 1.0 {
            for k in 0..5 {
                self.x[k] = world[k];
                self.prev[k] = world[k];
            }
            self.anchor = world[0];
            self.ready = true;
        }
        let waist_rot = turn * pose.rotations[self.waist];
        let first_ref = (waist_rot * FIRST_REF).normalize();
        let waist_pos = to_world(pose.positions[self.waist]);
        let waist_axis = waist_rot * Vec3::Y;
        let waist_centre = waist_pos + waist_rot * Vec3::new(0.0, -0.064, 0.072);
        let capsules: Vec<(Vec3, Vec3, f32)> = self
            .capsules
            .iter()
            .map(|c| (to_world(pose.positions[c.a]), to_world(pose.positions[c.b]), c.radius))
            .chain(std::iter::once((waist_centre - waist_axis * 0.06, waist_centre + waist_axis * 0.06, 0.18)))
            .collect();
        let ground = root.y;

        // Fixed steps, the anchor moving linearly over them.
        self.leftover = (self.leftover + dt.clamp(0.0, 0.25)).min(0.25);
        let steps = (self.leftover / STEP).floor() as usize;
        self.leftover -= steps as f32 * STEP;
        let start = self.anchor;
        for s in 0..steps {
            let anchor = start.lerp(world[0], (s + 1) as f32 / steps as f32);
            // The share of the anchor's movement the strap is carried by.
            let carry = (anchor - self.x[0]) * (1.0 - INERTIA);
            self.x[0] = anchor;
            self.prev[0] = anchor;
            let keep = (-DAMPING * STEP).exp();
            for k in 1..5 {
                self.x[k] += carry;
                self.prev[k] += carry;
                let v = (self.x[k] - self.prev[k]) * keep;
                self.prev[k] = self.x[k];
                self.x[k] += v + GRAVITY * STEP * STEP;
            }
            for _ in 0..6 {
                self.solve(first_ref, &capsules, ground);
            }
        }
        self.anchor = world[0];

        // Back into the pose: each bone turned from its animated direction
        // to the simulated one, joints where the chain has them.
        let inv = turn.inverse();
        let sim: Vec<Vec3> = self.x.iter().map(|&p| inv * (p - root)).collect();
        for k in 0..4 {
            let from = (animated[k + 1] - animated[k]).normalize_or_zero();
            let to = (sim[k + 1] - sim[k]).normalize_or_zero();
            if from == Vec3::ZERO || to == Vec3::ZERO {
                continue;
            }
            let b = self.bones[k];
            pose.rotations[b] = (Quat::from_rotation_arc(from, to) * pose.rotations[b]).normalize();
            if k > 0 {
                pose.positions[b] = sim[k];
            }
        }
    }

    fn solve(&mut self, first_ref: Vec3, capsules: &[(Vec3, Vec3, f32)], ground: f32) {
        let inv_mass = |k: usize| if k == 0 { 0.0 } else { 1.0 / MASS[k - 1] };
        for k in 0..4 {
            // Length.
            let d = self.x[k + 1] - self.x[k];
            let len = d.length();
            if len > 1e-6 {
                let (wa, wb) = (inv_mass(k), inv_mass(k + 1));
                let corr = d * ((len - self.lengths[k]) / (len * (wa + wb)));
                self.x[k] += corr * wa;
                self.x[k + 1] -= corr * wb;
            }
            // Swing limit: the first link about MGSV's reference, the others
            // about the link above.
            let (reference, limit) = if k == 0 {
                (first_ref, FIRST_LIMIT)
            } else {
                ((self.x[k] - self.x[k - 1]).normalize_or_zero(), LIMIT)
            };
            let dir = (self.x[k + 1] - self.x[k]).normalize_or_zero();
            if reference != Vec3::ZERO && dir != Vec3::ZERO {
                let angle = reference.angle_between(dir);
                let max = limit.to_radians();
                if angle > max {
                    let limited = Quat::from_rotation_arc(reference, dir);
                    let (axis, _) = limited.to_axis_angle();
                    let clamped = Quat::from_axis_angle(axis, max) * reference;
                    self.x[k + 1] = self.x[k] + clamped * self.lengths[k];
                }
            }
        }
        // Collisions: out of the body capsules and above the ground.
        for k in 1..5 {
            for &(a, b, r) in capsules {
                let ab = b - a;
                let t = if ab.length_squared() > 1e-8 {
                    ((self.x[k] - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let closest = a + ab * t;
                let off = self.x[k] - closest;
                let dist = off.length();
                let min = r + RADIUS;
                if dist < min && dist > 1e-6 {
                    self.x[k] = closest + off * (min / dist);
                }
            }
            if self.x[k].y < ground + RADIUS {
                self.x[k].y = ground + RADIUS;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fox_anim::skeleton::Bone;

    /// A waist and the strap hanging behind it, 15 cm links.
    fn skeleton() -> Skeleton {
        let bone = |name: &str, parent: Option<usize>, y: f32| Bone {
            name: fox_hash::str_code32(name.as_bytes()),
            parent,
            rest_local: Vec3::ZERO,
            rest_model: Vec3::new(0.03, y, -0.13),
        };
        Skeleton {
            bones: vec![
                Bone { rest_model: Vec3::new(0.0, 1.0, 0.0), ..bone("SKL_000_WAIST", None, 0.0) },
                bone("SKL_310_BELT_A", Some(0), 0.95),
                bone("SKL_311_BELT_B", Some(1), 0.80),
                bone("SKL_312_BELT_C", Some(2), 0.65),
                bone("SKL_313_BELT_D", Some(3), 0.50),
            ],
        }
    }

    fn rest_pose(skel: &Skeleton) -> Pose {
        let mut pose = Pose::rest(skel);
        pose.positions = skel.bones.iter().map(|b| b.rest_model).collect();
        pose
    }

    #[test]
    fn hangs_still_at_rest() {
        let skel = skeleton();
        let mut sim = StrapSim::new(&skel).unwrap();
        let mut pose = rest_pose(&skel);
        for _ in 0..120 {
            pose = rest_pose(&skel);
            sim.step(&mut pose, Vec3::ZERO, Quat::IDENTITY, 1.0 / 60.0);
        }
        // Still hanging straight down from its anchor, links unstretched.
        let tip_drop = pose.positions[1].y - pose.positions[4].y;
        assert!((tip_drop - 0.45).abs() < 0.01, "{tip_drop}");
        assert!((pose.positions[4].z - pose.positions[1].z).abs() < 0.02);
    }

    #[test]
    fn trails_behind_when_running() {
        let skel = skeleton();
        let mut sim = StrapSim::new(&skel).unwrap();
        // Running forward (+Z) at 4 m/s: the strap's lower end swings back.
        let mut root = Vec3::ZERO;
        let mut pose = rest_pose(&skel);
        for _ in 0..30 {
            root.z += 4.0 / 60.0;
            pose = rest_pose(&skel);
            sim.step(&mut pose, root, Quat::IDENTITY, 1.0 / 60.0);
        }
        let back = pose.positions[1].z - pose.positions[4].z;
        assert!(back > 0.05, "tip only {back} m behind the anchor");
        // Within the joint limits: the first link at most 35 degrees off.
        let first = (pose.positions[2] - pose.positions[1]).normalize();
        assert!(first.angle_between(FIRST_REF.normalize()).to_degrees() < 36.0);
    }
}
