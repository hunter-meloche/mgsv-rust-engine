//! `.frig` rigs: how animation rig units drive skeleton bones.
//!
//! Format knowledge follows mctrollin's GPL-3.0 "Fox Engine MTAR Tools"
//! (FRIG binary template); independent implementation.
//!
//! * Header (32 bytes): name (StrCode64), version (102), rig unit count,
//!   segment count, file size, bone list offset, mask definition offset.
//! * Rig definition: one absolute offset per unit, then unit definitions:
//!   `u32 type, i16 track count, i16 bone count, i16 parent bone,
//!   i16 parent unit, u32 pad`, followed by type-specific skeleton and
//!   segment indices (and, for IK units, a chain plane normal).
//! * Mask definition: layers of per-unit weights (12-char names).
//! * Bone list: `(u32 rig index, StrCode32 name)` pairs.

#[derive(Debug, thiserror::Error)]
pub enum FrigError {
    #[error("unsupported frig version {0}")]
    Version(u32),
    #[error("truncated at {0:#x}")]
    Truncated(usize),
    #[error("unknown rig unit type {0}")]
    UnitType(u32),
}

type Result<T> = std::result::Result<T, FrigError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitType {
    Root = 1,
    Orientation = 2,
    TwoBone = 3,
    LocalOrientation = 4,
    LocalTransform = 5,
    ThreeBoneLikeTwoBone = 6,
    Transform = 7,
    Arm = 8,
    LocalTransformSrt = 9,
    AnimalLeg = 10,
    MultiLocalOrientation = 11,
    TwoBoneTrans = 12,
}

impl UnitType {
    fn from_u32(v: u32) -> Result<Self> {
        use UnitType::*;
        Ok(match v {
            1 => Root,
            2 => Orientation,
            3 => TwoBone,
            4 => LocalOrientation,
            5 => LocalTransform,
            6 => ThreeBoneLikeTwoBone,
            7 => Transform,
            8 => Arm,
            9 => LocalTransformSrt,
            10 => AnimalLeg,
            11 => MultiLocalOrientation,
            12 => TwoBoneTrans,
            other => return Err(FrigError::UnitType(other)),
        })
    }
}

/// One rig unit. Skeleton indices refer to the rig's bone list; segment
/// indices refer to the segments of the matching animation track unit.
#[derive(Clone, Debug)]
pub struct RigUnit {
    pub kind: UnitType,
    pub track_count: i16,
    pub bone_count: i16,
    pub parent_unit: i16,
    /// Bones this unit drives, in chain order (root → effector for IK).
    pub bones: Vec<i16>,
    pub segments: Vec<i16>,
    /// IK chain plane normal (two-bone / arm units).
    pub plane_normal: Option<[f32; 4]>,
}

#[derive(Clone, Debug)]
pub struct MaskLayer {
    pub name: String,
    pub weights: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct Frig {
    pub name: u64,
    pub segment_count: u32,
    pub units: Vec<RigUnit>,
    pub masks: Vec<MaskLayer>,
    /// `(rig index, StrCode32 bone name)`.
    pub bones: Vec<(u32, u32)>,
}

struct R<'a>(&'a [u8]);

impl R<'_> {
    fn b<const N: usize>(&self, at: usize) -> Result<[u8; N]> {
        self.0
            .get(at..at + N)
            .map(|s| s.try_into().unwrap())
            .ok_or(FrigError::Truncated(at))
    }
    fn i16(&self, at: usize) -> Result<i16> {
        Ok(i16::from_le_bytes(self.b(at)?))
    }
    fn u32(&self, at: usize) -> Result<u32> {
        Ok(u32::from_le_bytes(self.b(at)?))
    }
    fn u64(&self, at: usize) -> Result<u64> {
        Ok(u64::from_le_bytes(self.b(at)?))
    }
    fn f32(&self, at: usize) -> Result<f32> {
        Ok(f32::from_le_bytes(self.b(at)?))
    }
    fn i16s(&self, at: usize, n: usize) -> Result<Vec<i16>> {
        (0..n).map(|k| self.i16(at + k * 2)).collect()
    }
    fn v4(&self, at: usize) -> Result<[f32; 4]> {
        Ok([
            self.f32(at)?,
            self.f32(at + 4)?,
            self.f32(at + 8)?,
            self.f32(at + 12)?,
        ])
    }
}

impl Frig {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let r = R(data);
        let version = r.u32(8)?;
        if version != 102 {
            return Err(FrigError::Version(version));
        }
        let unit_count = r.u32(12)? as usize;
        let segment_count = r.u32(16)?;
        let bone_list = r.u32(24)? as usize;
        let mask_def = r.u32(28)? as usize;

        let mut units = Vec::with_capacity(unit_count);
        for i in 0..unit_count {
            let at = r.u32(32 + i * 4)? as usize;
            let kind = UnitType::from_u32(r.u32(at)?)?;
            let body = at + 16;
            // (bones, segments, plane normal) by type; IK units carry 16
            // unknown bytes, then the plane normal, then indices.
            use UnitType::*;
            let (bones, segments, plane_normal) = match kind {
                Root => (vec![], r.i16s(body, 2)?, None),
                Orientation | LocalOrientation => {
                    (vec![r.i16(body)?], vec![r.i16(body + 2)?], None)
                }
                Transform | LocalTransform => (vec![r.i16(body)?], r.i16s(body + 2, 2)?, None),
                ThreeBoneLikeTwoBone => (
                    r.i16s(body + 32, 3)?,
                    r.i16s(body + 38, 2)?,
                    Some(r.v4(body + 16)?),
                ),
                Arm => {
                    // chain a, b, c, segments a, b, c, effector
                    let mut bones = r.i16s(body + 32, 3)?;
                    bones.push(r.i16(body + 44)?);
                    (bones, r.i16s(body + 38, 3)?, Some(r.v4(body + 16)?))
                }
                TwoBone => {
                    // chain a, b, segments a, b, effector
                    let mut bones = r.i16s(body + 32, 2)?;
                    bones.push(r.i16(body + 40)?);
                    (bones, r.i16s(body + 36, 2)?, Some(r.v4(body + 16)?))
                }
                LocalTransformSrt => (vec![r.i16(body)?], vec![], None),
                AnimalLeg => (r.i16s(body + 24, 4)?, vec![], None),
                MultiLocalOrientation => (vec![r.i16(body)?], vec![r.i16(body + 2)?], None),
                TwoBoneTrans => (r.i16s(body + 16, 2)?, vec![], None),
            };
            units.push(RigUnit {
                kind,
                track_count: r.i16(at + 4)?,
                bone_count: r.i16(at + 6)?,
                parent_unit: r.i16(at + 10)?,
                bones,
                segments,
                plane_normal,
            });
        }

        let mut masks = Vec::new();
        if mask_def != 0 {
            let mask_units = r.u32(mask_def)? as usize;
            let layers = r.u32(mask_def + 4)? as usize;
            for l in 0..layers {
                let at = mask_def + r.u32(mask_def + 8 + l * 4)? as usize;
                let name_bytes = data.get(at + 4..at + 16).ok_or(FrigError::Truncated(at))?;
                let name = String::from_utf8_lossy(name_bytes)
                    .trim_end_matches('\0')
                    .to_string();
                let weights = (0..mask_units)
                    .map(|k| r.f32(at + 16 + k * 4))
                    .collect::<Result<_>>()?;
                masks.push(MaskLayer { name, weights });
            }
        }

        let bone_count = r.u32(bone_list)? as usize;
        let bones = (0..bone_count)
            .map(|k| Ok((r.u32(bone_list + 4 + k * 8)?, r.u32(bone_list + 8 + k * 8)?)))
            .collect::<Result<_>>()?;

        Ok(Self {
            name: r.u64(0)?,
            segment_count,
            units,
            masks,
            bones,
        })
    }
}
