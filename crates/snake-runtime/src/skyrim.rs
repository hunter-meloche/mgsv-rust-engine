//! Retargeting Snake's pose onto Skyrim's character skeleton.
//!
//! Spaces: Fox model space (Y up, forward +Z, left +X, meters) and Skyrim
//! actor space (Z up, forward +Y, right +X, game units ~ 70 per meter). The
//! basis change is `sky = (-x, z, y)` (a proper rotation).
//!
//! Fox rest rotations are identity, so Snake's solved model rotations are
//! deltas from his T-pose rest. Skyrim's skeleton rests in an A-pose with
//! arbitrary bone frames. For every mapped bone an *aligned rest* is built
//! once: the Skyrim rest frame swung so the bone points where Snake's rest
//! bone points (direction to the child joint). Each frame the target model
//! rotation is `C * snake_rotation * C^-1 * aligned_rest`, converted to
//! local rotations relative to the (also retargeted) parent.

use anyhow::{Context, Result, bail};
use fox_anim::glam::{Mat3, Quat, Vec3};
use fox_anim::{Pose, Skeleton};

/// Meters to Skyrim units.
pub const UNITS_PER_METER: f32 = 70.0;

fn fox_to_sky(v: Vec3) -> Vec3 {
    Vec3::new(-v.x, v.z, v.y)
}

/// The basis change as a rotation (applied as `C q C^-1`).
fn basis() -> Quat {
    Quat::from_mat3(&Mat3::from_cols(
        Vec3::new(-1.0, 0.0, 0.0),
        Vec3::new(0.0, 0.0, 1.0),
        Vec3::new(0.0, 1.0, 0.0),
    ))
}

/// Skyrim bone -> Snake bone, plus the joints used to align rest directions
/// (Skyrim child, Snake child).
const MAP: &[(&str, &str, Option<(&str, &str)>)] = &[
    ("NPC COM [COM ]", "SKL_000_WAIST", None),
    (
        "NPC Spine [Spn0]",
        "SKL_001_SPINE",
        Some(("NPC Spine1 [Spn1]", "SKL_002_CHEST")),
    ),
    (
        "NPC Spine1 [Spn1]",
        "SKL_001_SPINE",
        Some(("NPC Spine2 [Spn2]", "SKL_002_CHEST")),
    ),
    (
        "NPC Spine2 [Spn2]",
        "SKL_002_CHEST",
        Some(("NPC Neck [Neck]", "SKL_003_NECK")),
    ),
    (
        "NPC Neck [Neck]",
        "SKL_003_NECK",
        Some(("NPC Head [Head]", "SKL_004_HEAD")),
    ),
    ("NPC Head [Head]", "SKL_004_HEAD", None),
    (
        "NPC L Clavicle [LClv]",
        "SKL_010_LSHLD",
        Some(("NPC L UpperArm [LUar]", "SKL_011_LUARM")),
    ),
    (
        "NPC L UpperArm [LUar]",
        "SKL_011_LUARM",
        Some(("NPC L Forearm [LLar]", "SKL_012_LFARM")),
    ),
    (
        "NPC L Forearm [LLar]",
        "SKL_012_LFARM",
        Some(("NPC L Hand [LHnd]", "SKL_013_LHAND")),
    ),
    (
        "NPC L Hand [LHnd]",
        "SKL_013_LHAND",
        Some(("NPC L Finger20 [LF20]", "SKL_107_LF31")),
    ),
    (
        "NPC R Clavicle [RClv]",
        "SKL_020_RSHLD",
        Some(("NPC R UpperArm [RUar]", "SKL_021_RUARM")),
    ),
    (
        "NPC R UpperArm [RUar]",
        "SKL_021_RUARM",
        Some(("NPC R Forearm [RLar]", "SKL_022_RFARM")),
    ),
    (
        "NPC R Forearm [RLar]",
        "SKL_022_RFARM",
        Some(("NPC R Hand [RHnd]", "SKL_023_RHAND")),
    ),
    (
        "NPC R Hand [RHnd]",
        "SKL_023_RHAND",
        Some(("NPC R Finger20 [RF20]", "SKL_207_RF31")),
    ),
    (
        "NPC L Thigh [LThg]",
        "SKL_030_LTHIGH",
        Some(("NPC L Calf [LClf]", "SKL_031_LLEG")),
    ),
    (
        "NPC L Calf [LClf]",
        "SKL_031_LLEG",
        Some(("NPC L Foot [Lft ]", "SKL_032_LFOOT")),
    ),
    (
        "NPC L Foot [Lft ]",
        "SKL_032_LFOOT",
        Some(("NPC L Toe0 [LToe]", "SKL_033_LTOE")),
    ),
    ("NPC L Toe0 [LToe]", "SKL_033_LTOE", None),
    (
        "NPC R Thigh [RThg]",
        "SKL_040_RTHIGH",
        Some(("NPC R Calf [RClf]", "SKL_041_RLEG")),
    ),
    (
        "NPC R Calf [RClf]",
        "SKL_041_RLEG",
        Some(("NPC R Foot [Rft ]", "SKL_042_RFOOT")),
    ),
    (
        "NPC R Foot [Rft ]",
        "SKL_042_RFOOT",
        Some(("NPC R Toe0 [RToe]", "SKL_043_RTOE")),
    ),
    ("NPC R Toe0 [RToe]", "SKL_043_RTOE", None),
];

/// Fingers: Skyrim finger n (thumb 0 .. pinky 4), joints 0-2 -> Snake bones.
const FINGERS: [[&str; 3]; 5] = [
    ["101_{s}F10", "102_{s}F11", "103_{s}F12"],
    ["104_{s}F21", "105_{s}F22", "106_{s}F23"],
    ["107_{s}F31", "108_{s}F32", "109_{s}F33"],
    ["111_{s}F41", "112_{s}F42", "113_{s}F43"],
    ["114_{s}F51", "115_{s}F52", "116_{s}F53"],
];

#[derive(Clone, Debug)]
struct Bone {
    name: String,
    /// Index of the parent in `bones`, if the parent is also output.
    parent: Option<usize>,
    /// Model rotation of the nearest unmapped ancestor chain (rest), used
    /// when `parent` is None.
    parent_rest_model: Quat,
    rest_local_rot: Quat,
    rest_local_pos: Vec3,
    /// Snake skeleton bone driving it (None: keeps the rest local rotation).
    source: Option<usize>,
    /// Aligned rest model rotation (Skyrim rest swung to Snake's rest).
    aligned: Quat,
}

/// Retargeting table for one Skyrim skeleton.
#[derive(Clone, Debug)]
pub struct SkyrimRetarget {
    bones: Vec<Bone>,
    waist: usize,
}

/// One output bone: Skyrim node name and its new local transform.
#[derive(Clone, Copy, Debug)]
pub struct LocalPose {
    pub rotation: Quat,
    /// For bones Snake drives: local translation placing the joint where
    /// Snake's is (the COM: pelvis height and sway); None keeps the rest.
    pub translation: Option<Vec3>,
}

impl SkyrimRetarget {
    /// Build from Skyrim's `skeleton.nif` (parsed) and Snake's skeleton.
    pub fn new(nif: &skyrim_data::Nif, snake: &Skeleton) -> Result<Self> {
        let parents = nif.parents();
        let local = |block: usize| -> (Quat, Vec3) {
            let n = nif.node(block).unwrap();
            // Stored rows are the matrix rows (v' = M v); verified on the
            // rest pose (left side at -X, toes forward, feet on the ground).
            let m = Mat3::from_cols_array_2d(&n.rotation).transpose();
            (
                Quat::from_mat3(&m).normalize(),
                Vec3::from_array(n.translation),
            )
        };
        // Rest model transforms of every node.
        let mut model: Vec<Option<(Quat, Vec3)>> = vec![None; nif.nodes.len()];
        fn compute(
            b: usize,
            parents: &[Option<usize>],
            local: &dyn Fn(usize) -> (Quat, Vec3),
            model: &mut Vec<Option<(Quat, Vec3)>>,
        ) -> (Quat, Vec3) {
            if let Some(m) = model[b] {
                return m;
            }
            let (lr, lt) = local(b);
            let m = match parents[b] {
                Some(p) => {
                    let (pr, pt) = compute(p, parents, local, model);
                    ((pr * lr).normalize(), pt + pr * lt)
                }
                None => (lr, lt),
            };
            model[b] = Some(m);
            m
        }
        let snake_bone = |n: &str| snake.find(fox_hash::str_code32(n.as_bytes()));
        let snake_rest = |i: usize| snake.bones[i].rest_model;
        let block_of = |name: &str| nif.find(name).map(|n| n.block);

        // Output list: mapped bones plus fingers, in an order where parents
        // come first (MAP is listed parent-first; fingers after hands).
        let mut table: Vec<(String, Option<usize>, Option<(String, usize)>)> = Vec::new();
        for (sky, sn, align) in MAP {
            if *sky == "NPC Spine [Spn0]" {
                // Thighs hang off the pelvis: keep it at rest under the COM.
                table.push(("NPC Pelvis [Pelv]".to_string(), None, None));
            }
            let src = snake_bone(sn).with_context(|| format!("Snake bone {sn}"))?;
            let align = match align {
                Some((sky_child, sn_child)) => Some((
                    sky_child.to_string(),
                    snake_bone(sn_child).with_context(|| format!("Snake bone {sn_child}"))?,
                )),
                None => None,
            };
            table.push((sky.to_string(), Some(src), align));
        }
        for (side, s) in [("L", "L"), ("R", "R")] {
            let base = if side == "L" { 0 } else { 100 };
            for (f, joints) in FINGERS.iter().enumerate() {
                for (j, pat) in joints.iter().enumerate() {
                    let num: u32 = pat[..3].parse().unwrap();
                    let sn = format!("SKL_{:03}_{}", num + base, &pat[4..].replace("{s}", s));
                    let sky = format!("NPC {side} Finger{f}{j} [{side}F{f}{j}]");
                    let src = snake_bone(&sn).with_context(|| format!("Snake bone {sn}"))?;
                    let align = (j < 2).then(|| {
                        let next = joints[j + 1];
                        let num: u32 = next[..3].parse().unwrap();
                        let sn_child =
                            format!("SKL_{:03}_{}", num + base, &next[4..].replace("{s}", s));
                        (
                            format!("NPC {side} Finger{f}{} [{side}F{f}{}]", j + 1, j + 1),
                            snake_bone(&sn_child).unwrap_or(src),
                        )
                    });
                    table.push((sky, Some(src), align));
                }
            }
        }
        // Twist bones: kept at rest relative to their (retargeted) parents.
        for tw in [
            "NPC L UpperarmTwist1 [LUt1]",
            "NPC L UpperarmTwist2 [LUt2]",
            "NPC L ForearmTwist1 [LLt1]",
            "NPC L ForearmTwist2 [LLt2]",
            "NPC R UpperarmTwist1 [RUt1]",
            "NPC R UpperarmTwist2 [RUt2]",
            "NPC R ForearmTwist1 [RLt1]",
            "NPC R ForearmTwist2 [RLt2]",
        ] {
            if block_of(tw).is_some() {
                table.push((tw.to_string(), None, None));
            }
        }

        let mut bones: Vec<Bone> = Vec::with_capacity(table.len());
        for (name, source, align) in table {
            let block =
                block_of(&name).with_context(|| format!("Skyrim skeleton has no {name}"))?;
            let (rest_local_rot, rest_local_pos) = local(block);
            let (rest_model_rot, rest_model_pos) = compute(block, &parents, &local, &mut model);
            // Local transforms are relative to the direct Skyrim parent, which
            // must itself be an output bone (only the COM hangs off an
            // untouched node, NPC Root).
            let parent = parents[block]
                .and_then(|a| bones.iter().position(|b| block_of(&b.name) == Some(a)));
            if parent.is_none() && name != "NPC COM [COM ]" {
                bail!("{name}: its Skyrim parent is not retargeted");
            }
            let parent_rest_model = parents[block].map_or(Quat::IDENTITY, |p| {
                compute(p, &parents, &local, &mut model).0
            });
            let aligned = match (&align, source) {
                (Some((sky_child, sn_child)), Some(src)) => {
                    let child = block_of(sky_child)
                        .with_context(|| format!("Skyrim skeleton has no {sky_child}"))?;
                    let sky_dir = (compute(child, &parents, &local, &mut model).1 - rest_model_pos)
                        .normalize_or_zero();
                    let snake_dir =
                        fox_to_sky(snake_rest(*sn_child) - snake_rest(src)).normalize_or_zero();
                    if sky_dir == Vec3::ZERO || snake_dir == Vec3::ZERO {
                        rest_model_rot
                    } else {
                        (Quat::from_rotation_arc(sky_dir, snake_dir) * rest_model_rot).normalize()
                    }
                }
                _ => rest_model_rot,
            };
            bones.push(Bone {
                name,
                parent,
                parent_rest_model,
                rest_local_rot,
                rest_local_pos,
                source,
                aligned,
            });
        }
        let waist = snake_bone("SKL_000_WAIST").context("Snake waist")?;
        if bones.is_empty() {
            bail!("nothing to retarget");
        }
        Ok(Self { bones, waist })
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.bones.iter().map(|b| b.name.as_str())
    }

    pub fn len(&self) -> usize {
        self.bones.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bones.is_empty()
    }

    /// Local transforms for every output bone, for Snake's `pose` (root
    /// space), in the order of [`Self::names`].
    pub fn apply(&self, pose: &Pose) -> Vec<LocalPose> {
        let c = basis();
        let ci = c.inverse();
        let mut model = Vec::with_capacity(self.bones.len());
        let mut model_pos: Vec<Vec3> = Vec::with_capacity(self.bones.len());
        let mut out = Vec::with_capacity(self.bones.len());
        for b in &self.bones {
            let parent_model = b.parent.map_or(b.parent_rest_model, |p| model[p]);
            let parent_pos = b.parent.map_or(Vec3::ZERO, |p| model_pos[p]);
            let target = match b.source {
                Some(s) => (c * pose.rotations[s] * ci * b.aligned).normalize(),
                None => (parent_model * b.rest_local_rot).normalize(),
            };
            model.push(target);
            let rotation = (parent_model.inverse() * target).normalize();
            // Joints driven by Snake sit where Snake's are (his proportions,
            // so his own mesh skins onto this skeleton without the limbs
            // pivoting about Skyrim's joints); the COM carries his pelvis
            // height and sway (NPC Root is at the actor's origin). Other
            // bones keep their rest offsets.
            let snake_pos = match b.source {
                Some(_) if b.name == "NPC COM [COM ]" => {
                    Some(fox_to_sky(pose.positions[self.waist]) * UNITS_PER_METER)
                }
                Some(s) => Some(fox_to_sky(pose.positions[s]) * UNITS_PER_METER),
                None => None,
            };
            let translation = snake_pos.map(|p| parent_model.inverse() * (p - parent_pos));
            model_pos.push(snake_pos.unwrap_or(parent_pos + parent_model * b.rest_local_pos));
            out.push(LocalPose {
                rotation,
                translation,
            });
        }
        out
    }

    /// Transform of an object Snake holds at `held` (root space, e.g. the
    /// gun from `WeaponRig::gun_transform`) relative to Skyrim bone `bone`,
    /// in Skyrim axes and units, for an object whose own geometry is
    /// converted the same way. None if `bone` is not driven by Snake.
    ///
    /// The bone's model rotation is `C R C^-1 A` (A its alignment), so the
    /// held object's local rotation is `A^-1 C (R^-1 G) C^-1`.
    pub fn attach_local(&self, pose: &Pose, bone: &str, held: (Quat, Vec3)) -> Option<(Quat, Vec3)> {
        let b = self.bones.iter().find(|b| b.name == bone)?;
        let s = b.source?;
        let (hr, hp) = (*pose.rotations.get(s)?, *pose.positions.get(s)?);
        let inv = hr.inverse();
        let (q_rel, t_rel) = (inv * held.0, inv * (held.1 - hp));
        let c = basis();
        let ai = b.aligned.inverse();
        Some((
            (ai * c * q_rel * c.inverse()).normalize(),
            ai * (fox_to_sky(t_rel) * UNITS_PER_METER),
        ))
    }

    /// Model-space joint positions of the retargeted Skyrim skeleton (for
    /// checks): rest local translations under the retargeted rotations.
    pub fn joint_positions(&self, pose: &Pose) -> Vec<(String, Vec3)> {
        self.model_transforms(pose)
            .into_iter()
            .map(|(name, _, pos)| (name, pos))
            .collect()
    }

    /// Model-space rotation and position of every output bone for `pose`
    /// (Skyrim axes and units; the space of Snake's root converted by
    /// `(-x, z, y) * 70`): rest local translations under the retargeted
    /// rotations, as the plugin poses the in-game skeleton.
    pub fn model_transforms(&self, pose: &Pose) -> Vec<(String, Quat, Vec3)> {
        let locals = self.apply(pose);
        let mut model: Vec<(Quat, Vec3)> = Vec::with_capacity(self.bones.len());
        let mut out = Vec::new();
        for (b, l) in self.bones.iter().zip(&locals) {
            let (pr, pt) = match b.parent {
                Some(p) => model[p],
                None => (b.parent_rest_model, Vec3::ZERO),
            };
            let t = l.translation.unwrap_or(b.rest_local_pos);
            let m = ((pr * l.rotation).normalize(), pt + pr * t);
            model.push(m);
            out.push((b.name.clone(), m.0, m.1));
        }
        out
    }

    /// Each output bone's name, parent (index among the output bones) and
    /// driving Snake bone.
    pub fn bone_links(&self) -> Vec<(String, Option<usize>, Option<usize>)> {
        self.bones.iter().map(|b| (b.name.clone(), b.parent, b.source)).collect()
    }
}
