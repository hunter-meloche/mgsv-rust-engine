use glam::Vec3;

use fox_data::Fmdl;

#[derive(Clone, Debug)]
pub struct Bone {
    /// `StrCode32` of the bone name (the low 32 bits of its `StrCode64`).
    pub name: u32,
    pub parent: Option<usize>,
    /// Rest translation relative to the parent.
    pub rest_local: Vec3,
    /// Rest position in model space.
    pub rest_model: Vec3,
}

#[derive(Clone, Debug, Default)]
pub struct Skeleton {
    /// Parents precede children.
    pub bones: Vec<Bone>,
}

impl Skeleton {
    pub fn from_fmdl(m: &Fmdl) -> Self {
        let v3 = |v: [f32; 4]| Vec3::new(v[0], v[1], v[2]);
        let bones = m
            .bones
            .iter()
            .map(|b| Bone {
                name: m.str_code64s.get(b.name as usize).copied().unwrap_or(0) as u32,
                parent: usize::try_from(b.parent).ok(),
                rest_local: v3(b.local_position),
                rest_model: v3(b.world_position),
            })
            .collect();
        Self { bones }
    }

    pub fn find(&self, name: u32) -> Option<usize> {
        self.bones.iter().position(|b| b.name == name)
    }
}
