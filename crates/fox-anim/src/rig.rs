//! Rig solve: turn sampled rig-unit tracks into a skeleton pose.
//!
//! Spaces (Fox character space: Y up, forward +Z, the character's left +X):
//!
//! * Orientation / Transform units give a bone's rotation (and for the waist
//!   its position) in character space. Rest rotations are identity, so these
//!   are the bones' model-space rotations directly.
//! * Local units give a rotation relative to the parent bone.
//! * Arm units: shoulder rotation, hand IK target position, IK pole.
//!   Two-bone units: foot IK target position, IK pole. The pole quaternion's
//!   X axis is the bend direction of the middle joint (elbow / knee); the
//!   rig's chain-plane normal gives the rest bend plane.
//! * The root unit gives the character root's transform in the clip's
//!   *anchor space*: the root at the clip's last frame (both root segments
//!   reach identity / zero there). IK target positions are stored in anchor
//!   space too (a planted foot's target stays constant while the root moves);
//!   everything else is relative to the root. The pose is produced in root
//!   space; [`HumanRig::root`] gives the root itself, for root motion.

use fox_data::frig::{Frig, UnitType};
use fox_data::mtar::{Channel, Clip};
use glam::{Mat3, Quat, Vec3};

use crate::Skeleton;
use crate::sample::sample;

/// Bone poses in character space, indexed like [`Skeleton::bones`].
#[derive(Clone, Debug)]
pub struct Pose {
    pub rotations: Vec<Quat>,
    pub positions: Vec<Vec3>,
    /// Per IK chain: distance to the target divided by the chain length
    /// (> 1 means the target was out of reach and the limb is straightened).
    pub ik_reach: Vec<f32>,
}

#[derive(Clone, Debug)]
enum Drive {
    /// Rotation (and optionally position) in character space.
    World { rot: usize, pos: Option<usize> },
    /// Rotation relative to the parent.
    Local { rot: usize },
}

#[derive(Clone, Debug)]
struct IkChain {
    upper: usize,
    lower: usize,
    effector: usize,
    target: usize,
    pole: usize,
    /// Rest bend-plane normal.
    plane_normal: Vec3,
}

/// The `human_finger.frig` rig bound to one skeleton.
#[derive(Clone, Debug)]
pub struct HumanRig {
    segment_count: usize,
    /// Per skeleton bone.
    drives: Vec<Option<Drive>>,
    chains: Vec<IkChain>,
    /// `(rotation delta, translation delta)` segments of the root unit.
    root: Option<(usize, usize)>,
    /// Axis of the pole quaternion that points along the bend direction.
    pole_axis: Vec3,
}

impl HumanRig {
    /// Bind `frig` to `skel` by bone name. Fails if a bone the rig drives is
    /// missing from the skeleton.
    pub fn new(frig: &Frig, skel: &Skeleton) -> Result<Self, String> {
        let bone = |i: i16| -> Result<usize, String> {
            let &(_, name) = frig
                .bones
                .get(i as usize)
                .ok_or_else(|| format!("rig bone index {i} out of range"))?;
            skel.find(name)
                .ok_or_else(|| format!("skeleton has no bone {name:#010x}"))
        };
        let seg = |s: i16| s as usize;
        let mut drives = vec![None; skel.bones.len()];
        let mut chains = Vec::new();
        let mut root = None;
        for u in &frig.units {
            match u.kind {
                UnitType::Root => root = Some((seg(u.segments[0]), seg(u.segments[1]))),
                UnitType::Transform => {
                    drives[bone(u.bones[0])?] = Some(Drive::World {
                        rot: seg(u.segments[0]),
                        pos: Some(seg(u.segments[1])),
                    })
                }
                UnitType::Orientation => {
                    drives[bone(u.bones[0])?] = Some(Drive::World {
                        rot: seg(u.segments[0]),
                        pos: None,
                    })
                }
                UnitType::LocalOrientation => {
                    drives[bone(u.bones[0])?] = Some(Drive::Local {
                        rot: seg(u.segments[0]),
                    })
                }
                UnitType::MultiLocalOrientation => {
                    for k in 0..u.track_count {
                        drives[bone(u.bones[0] + k)?] = Some(Drive::Local {
                            rot: seg(u.segments[0] + k),
                        });
                    }
                }
                UnitType::Arm => {
                    // bones: shoulder, upper arm, forearm, hand;
                    // segments: shoulder rotation, hand target, pole.
                    drives[bone(u.bones[0])?] = Some(Drive::World {
                        rot: seg(u.segments[0]),
                        pos: None,
                    });
                    chains.push(IkChain {
                        upper: bone(u.bones[1])?,
                        lower: bone(u.bones[2])?,
                        effector: bone(u.bones[3])?,
                        target: seg(u.segments[1]),
                        pole: seg(u.segments[2]),
                        plane_normal: normal(u.plane_normal),
                    });
                }
                UnitType::TwoBone => {
                    // bones: thigh, leg, foot; segments: foot target, pole.
                    chains.push(IkChain {
                        upper: bone(u.bones[0])?,
                        lower: bone(u.bones[1])?,
                        effector: bone(u.bones[2])?,
                        target: seg(u.segments[0]),
                        pole: seg(u.segments[1]),
                        plane_normal: normal(u.plane_normal),
                    });
                }
                other => {
                    return Err(format!(
                        "{other:?} units are not supported by the human rig"
                    ));
                }
            }
        }
        if skel
            .bones
            .iter()
            .enumerate()
            .any(|(i, b)| b.parent.is_some_and(|p| p >= i))
        {
            return Err("skeleton bones are not ordered parent-first".into());
        }
        Ok(Self {
            segment_count: frig.segment_count as usize,
            drives,
            chains,
            root,
            pole_axis: Vec3::X,
        })
    }

    /// Pose `skel` with `clip` at `frame` (fractional frames, 30 per second).
    pub fn pose(&self, skel: &Skeleton, clip: &Clip, frame: f32) -> Result<Pose, String> {
        let channels: Vec<&Channel> = clip.tracks.iter().flat_map(|t| &t.channels).collect();
        if channels.len() != self.segment_count {
            return Err(format!(
                "clip has {} segments, rig expects {}",
                channels.len(),
                self.segment_count
            ));
        }
        let v = |s: usize| sample(channels[s], frame);
        let quat = |s: usize| Quat::from_array(v(s)).normalize();
        let vec3 = |s: usize| {
            let a = v(s);
            Vec3::new(a[0], a[1], a[2])
        };

        // IK targets: anchor space -> root space.
        let (root_rot, root_pos) = self.root_at(&channels, frame);
        let inv_root = root_rot.inverse();
        let target = |s: usize| inv_root * (vec3(s) - root_pos);

        let n = skel.bones.len();
        let mut rot = vec![Quat::IDENTITY; n];
        let mut pos = vec![Vec3::ZERO; n];
        // World rotations decided by an IK solve, keyed by bone.
        let mut solved: Vec<Option<Quat>> = vec![None; n];
        let mut ik_reach = Vec::with_capacity(self.chains.len());

        for (i, b) in skel.bones.iter().enumerate() {
            let (parent_rot, parent_pos) = match b.parent {
                Some(p) => (rot[p], pos[p]),
                None => (Quat::IDENTITY, Vec3::ZERO),
            };
            pos[i] = parent_pos + parent_rot * b.rest_local;
            rot[i] = parent_rot;
            match &self.drives[i] {
                Some(Drive::World { rot: r, pos: p }) => {
                    rot[i] = quat(*r);
                    if let Some(p) = p {
                        pos[i] = vec3(*p);
                    }
                }
                Some(Drive::Local { rot: r }) => rot[i] = parent_rot * quat(*r),
                None => {}
            }
            if let Some(q) = solved[i] {
                rot[i] = q;
            }
            if let Some(c) = self.chains.iter().find(|c| c.upper == i) {
                let (upper, lower, reach) = solve_two_bone(
                    skel,
                    c,
                    pos[i],
                    target(c.target),
                    quat(c.pole) * self.pole_axis,
                );
                rot[i] = upper;
                solved[c.lower] = Some(lower);
                ik_reach.push(reach);
            }
        }
        Ok(Pose {
            rotations: rot,
            positions: pos,
            ik_reach,
        })
    }

    /// Override which pole-quaternion axis is the bend direction (default
    /// +X). For experiments validating the rig convention.
    pub fn set_pole_axis(&mut self, axis: Vec3) {
        self.pole_axis = axis;
    }

    /// `(lower bone, effector)` skeleton indices of each IK chain, in the
    /// order of [`Pose::ik_reach`].
    pub fn ik_joints(&self) -> Vec<(usize, usize)> {
        self.chains.iter().map(|c| (c.lower, c.effector)).collect()
    }

    /// The character root at `frame` in the clip's anchor space (the root at
    /// the last frame): rotation and position.
    pub fn root(&self, clip: &Clip, frame: f32) -> (Quat, Vec3) {
        let channels: Vec<&Channel> = clip.tracks.iter().flat_map(|t| &t.channels).collect();
        self.root_at(&channels, frame)
    }

    /// Root motion from frame `from` to `to`, expressed in the root's own
    /// frame at `from`: (rotation delta, translation delta).
    pub fn root_motion(&self, clip: &Clip, from: f32, to: f32) -> (Quat, Vec3) {
        let (r0, p0) = self.root(clip, from);
        let (r1, p1) = self.root(clip, to);
        let inv = r0.inverse();
        ((inv * r1).normalize(), inv * (p1 - p0))
    }

    fn root_at(&self, channels: &[&Channel], frame: f32) -> (Quat, Vec3) {
        let Some((qs, vs)) = self.root else {
            return (Quat::IDENTITY, Vec3::ZERO);
        };
        let (Some(qc), Some(vc)) = (channels.get(qs), channels.get(vs)) else {
            return (Quat::IDENTITY, Vec3::ZERO);
        };
        let v = sample(vc, frame);
        (
            Quat::from_array(sample(qc, frame)).normalize(),
            Vec3::new(v[0], v[1], v[2]),
        )
    }
}

fn normal(n: Option<[f32; 4]>) -> Vec3 {
    n.map(|n| Vec3::new(n[0], n[1], n[2]).normalize_or_zero())
        .filter(|n| *n != Vec3::ZERO)
        .unwrap_or(Vec3::X)
}

/// Orthonormal frame from a bone direction and a bend-plane normal.
fn frame(dir: Vec3, normal: Vec3) -> Mat3 {
    let d = dir.normalize();
    let mut n = normal - d * d.dot(normal);
    if n.length_squared() < 1e-8 {
        n = d.any_orthonormal_vector();
    }
    let n = n.normalize();
    Mat3::from_cols(d, n, d.cross(n))
}

/// Rotation taking a rest bone frame onto a solved one.
fn frame_rotation(rest_dir: Vec3, rest_normal: Vec3, dir: Vec3, normal: Vec3) -> Quat {
    let m = frame(dir, normal) * frame(rest_dir, rest_normal).transpose();
    Quat::from_mat3(&m).normalize()
}

/// Two-bone IK in character space. Returns the world rotations of the upper
/// and lower bones and the reach ratio.
fn solve_two_bone(
    skel: &Skeleton,
    c: &IkChain,
    root: Vec3,
    target: Vec3,
    bend: Vec3,
) -> (Quat, Quat, f32) {
    let upper_rest = skel.bones[c.lower].rest_local;
    let lower_rest = skel.bones[c.effector].rest_local;
    let (l1, l2) = (upper_rest.length(), lower_rest.length());

    let to_target = target - root;
    let dist = to_target.length();
    let reach = dist / (l1 + l2);
    let line = if dist > 1e-6 {
        to_target / dist
    } else {
        upper_rest.normalize()
    };
    let d = dist.clamp((l1 - l2).abs() + 1e-4, l1 + l2 - 1e-4);

    // Bend direction: the pole's X axis, made perpendicular to the line.
    let mut b = bend - line * line.dot(bend);
    if b.length_squared() < 1e-8 {
        b = line.any_orthonormal_vector();
    }
    let b = b.normalize();
    let along = (l1 * l1 - l2 * l2 + d * d) / (2.0 * d);
    let h = (l1 * l1 - along * along).max(0.0).sqrt();
    let joint = root + line * along + b * h;
    let end = root + line * d;

    // Solved bend-plane normal, oriented like the rest one: in the rest pose
    // the bend direction is rest_dir x plane_normal.
    let normal = b.cross(line);
    let upper = frame_rotation(upper_rest, c.plane_normal, joint - root, normal);
    let lower = frame_rotation(lower_rest, c.plane_normal, end - joint, normal);
    (upper, lower, reach)
}
