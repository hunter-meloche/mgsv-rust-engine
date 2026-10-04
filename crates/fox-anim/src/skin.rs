//! Linear-blend skinning of `.fmdl` meshes with a solved [`Pose`].
//!
//! Fox rest rotations are identity, so a bone's bind transform is a pure
//! translation by its rest model-space position, and skinning a vertex `v`
//! by bone `b` is `R_b * (v - rest_b) + P_b`.
//!
//! A model part (body, face, gear) carries its own bone list. Its bones are
//! bound to the driving skeleton by name; a bone the skeleton lacks rides
//! rigidly on its nearest bound ancestor.

use fox_data::Fmdl;
use glam::Vec3;

use crate::{Pose, Skeleton};

/// One vertex influence: skeleton bone, weight, and the vertex position
/// relative to that bone's rest position.
#[derive(Clone, Copy, Debug)]
pub struct Influence {
    pub bone: u16,
    pub weight: f32,
    pub offset: Vec3,
}

#[derive(Clone, Debug)]
pub struct SkinMesh {
    pub rest_positions: Vec<Vec3>,
    /// Up to four influences per vertex; weights sum to 1.
    pub influences: Vec<Vec<Influence>>,
    pub indices: Vec<u16>,
}

#[derive(Clone, Debug)]
pub struct SkinnedModel {
    pub meshes: Vec<SkinMesh>,
    /// Model bones that had no skeleton bone of their own.
    pub unbound_bones: usize,
}

impl SkinnedModel {
    pub fn bind(model: &Fmdl, skel: &Skeleton) -> Result<Self, String> {
        let names: Vec<u32> = model
            .bones
            .iter()
            .map(|b| model.str_code64s.get(b.name as usize).copied().unwrap_or(0) as u32)
            .collect();
        // Per model bone: (skeleton bone, rest position used for binding).
        let mut binding = Vec::with_capacity(model.bones.len());
        let mut unbound_bones = 0;
        for i in 0..model.bones.len() {
            let mut cur = Some(i);
            let mut found = None;
            while let Some(k) = cur {
                if let Some(s) = skel.find(names[k]) {
                    let w = model.bones[k].world_position;
                    found = Some((s as u16, Vec3::new(w[0], w[1], w[2])));
                    break;
                }
                cur = usize::try_from(model.bones[k].parent).ok();
            }
            if found.map(|f| f.0 as usize) != skel.find(names[i]) {
                unbound_bones += 1;
            }
            binding.push(found.ok_or_else(|| format!("model bone {i} has no bound ancestor"))?);
        }

        let mut meshes = Vec::with_capacity(model.meshes.len());
        for m in &model.meshes {
            let group = model
                .bone_groups
                .get(m.bone_group as usize)
                .ok_or_else(|| format!("bone group {} out of range", m.bone_group))?;
            let rest_positions: Vec<Vec3> =
                m.positions.iter().map(|p| Vec3::from_array(*p)).collect();
            let mut influences = Vec::with_capacity(rest_positions.len());
            for (v, &p) in rest_positions.iter().enumerate() {
                let mut inf = Vec::with_capacity(4);
                match (m.bone_weights.get(v), m.bone_indices.get(v)) {
                    (Some(w), Some(ix)) => {
                        for k in 0..4 {
                            if w[k] == 0 {
                                continue;
                            }
                            let model_bone = *group
                                .get(ix[k] as usize)
                                .ok_or_else(|| format!("bone index {} outside group", ix[k]))?
                                as usize;
                            let (bone, rest) = binding[model_bone];
                            inf.push(Influence {
                                bone,
                                weight: w[k] as f32,
                                offset: p - rest,
                            });
                        }
                    }
                    // Rigid mesh: the bone group's first bone.
                    _ => {
                        let model_bone = *group.first().ok_or("rigid mesh without bones")? as usize;
                        let (bone, rest) = binding[model_bone];
                        inf.push(Influence {
                            bone,
                            weight: 1.0,
                            offset: p - rest,
                        });
                    }
                }
                let total: f32 = inf.iter().map(|i| i.weight).sum();
                if total <= 0.0 {
                    return Err(format!("vertex {v} has no weights"));
                }
                inf.iter_mut().for_each(|i| i.weight /= total);
                influences.push(inf);
            }
            meshes.push(SkinMesh {
                rest_positions,
                influences,
                indices: m.indices.clone(),
            });
        }
        Ok(Self {
            meshes,
            unbound_bones,
        })
    }

    /// Skinned vertex positions of every mesh for `pose`.
    pub fn skin(&self, pose: &Pose) -> Vec<Vec<Vec3>> {
        self.meshes
            .iter()
            .map(|m| {
                m.influences
                    .iter()
                    .map(|inf| {
                        inf.iter()
                            .map(|i| {
                                let b = i.bone as usize;
                                (pose.rotations[b] * i.offset + pose.positions[b]) * i.weight
                            })
                            .sum()
                    })
                    .collect()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skeleton::Bone;
    use glam::Quat;

    #[test]
    fn skinning_follows_bone_transforms() {
        let skel = Skeleton {
            bones: vec![
                Bone {
                    name: 1,
                    parent: None,
                    rest_local: Vec3::ZERO,
                    rest_model: Vec3::ZERO,
                },
                Bone {
                    name: 2,
                    parent: Some(0),
                    rest_local: Vec3::Y,
                    rest_model: Vec3::Y,
                },
            ],
        };
        let mesh = SkinMesh {
            rest_positions: vec![Vec3::new(1.0, 1.0, 0.0)],
            influences: vec![vec![
                Influence {
                    bone: 0,
                    weight: 0.5,
                    offset: Vec3::new(1.0, 1.0, 0.0),
                },
                Influence {
                    bone: 1,
                    weight: 0.5,
                    offset: Vec3::new(1.0, 0.0, 0.0),
                },
            ]],
            indices: vec![],
        };
        let model = SkinnedModel {
            meshes: vec![mesh],
            unbound_bones: 0,
        };
        let rest = Pose::rest(&skel);
        assert!((model.skin(&rest)[0][0] - Vec3::new(1.0, 1.0, 0.0)).length() < 1e-6);
        // Rotate bone 1 by 90 degrees about Y: its half of the vertex swings to -Z.
        let mut posed = rest.clone();
        posed.rotations[1] = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let v = model.skin(&posed)[0][0];
        assert!((v - Vec3::new(0.5, 1.0, -0.5)).length() < 1e-5, "{v}");
    }
}
