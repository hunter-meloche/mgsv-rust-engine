//! `.fmdl` models: skeleton, meshes, materials and vertex buffers.
//!
//! Layout (little-endian; see FMDL Studio by BobDoleOwndU, MIT, for the
//! community documentation of this format):
//! * header: magic `FMDL`, version (f32), section-info offset, block flags,
//!   section 0/1 block counts, section 0/1 offset and length;
//! * section info: section 0 entries `(u16 type, u16 count, u32 offset)`,
//!   section 1 entries `(u32 type, u32 offset, u32 length)`;
//! * section 0 holds fixed-size tables (bones, meshes, formats, ...);
//! * section 1 holds material vectors, the vertex/index buffer and strings.
//!
//! TPP models usually name things by `StrCode64`/`PathCode64` hashes (blocks
//! 21/22) rather than by strings; [`Fmdl::name`] handles both.

use crate::NameTable;

#[derive(Debug, thiserror::Error)]
pub enum FmdlError {
    #[error("not an fmdl file")]
    BadMagic,
    #[error("truncated at {0:#x}")]
    Truncated(usize),
    #[error("{0}")]
    Invalid(String),
}

type Result<T> = std::result::Result<T, FmdlError>;

#[derive(Clone, Debug)]
pub struct Bone {
    pub name: u16,
    pub parent: i16,
    pub bounding_box: u16,
    pub local_position: [f32; 4],
    pub world_position: [f32; 4],
}

#[derive(Clone, Debug)]
pub struct MeshGroup {
    pub name: u16,
    pub invisible: bool,
    pub parent: i16,
}

/// Meshes `meshes` belong to mesh group `group` (section 0 block 2).
#[derive(Clone, Debug)]
pub struct MeshGroupEntry {
    pub group: u16,
    pub meshes: std::ops::Range<usize>,
}

#[derive(Clone, Debug)]
pub struct MaterialInstance {
    pub name: u16,
    pub material: u16,
    pub textures: std::ops::Range<usize>,
    pub parameters: std::ops::Range<usize>,
}

/// `(name index, path/reference index)` pairs used by textures and parameters.
#[derive(Clone, Copy, Debug)]
pub struct NamePair {
    pub name: u16,
    pub reference: u16,
}

#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub material_instance: u16,
    /// Index into [`Fmdl::bone_groups`]: maps this mesh's bone indices to
    /// model bones.
    pub bone_group: u16,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 4]>,
    pub tangents: Vec<[f32; 4]>,
    pub colors: Vec<[u8; 4]>,
    /// Weights as stored (0..=255 per influence).
    pub bone_weights: Vec<[u8; 4]>,
    /// Indices into this mesh's bone group.
    pub bone_indices: Vec<[u8; 4]>,
    pub uvs: [Vec<[f32; 2]>; 4],
    pub indices: Vec<u16>,
}

#[derive(Clone, Debug, Default)]
pub struct Fmdl {
    pub version: f32,
    pub bones: Vec<Bone>,
    pub mesh_groups: Vec<MeshGroup>,
    pub mesh_group_entries: Vec<MeshGroupEntry>,
    pub material_instances: Vec<MaterialInstance>,
    pub bone_groups: Vec<Vec<u16>>,
    pub textures: Vec<NamePair>,
    pub material_parameters: Vec<NamePair>,
    pub materials: Vec<NamePair>,
    pub bounding_boxes: Vec<([f32; 4], [f32; 4])>,
    pub meshes: Vec<Mesh>,
    pub strings: Vec<String>,
    pub str_code64s: Vec<u64>,
    pub path_code64s: Vec<u64>,
    pub material_vectors: Vec<[f32; 4]>,
}

struct R<'a>(&'a [u8]);

impl R<'_> {
    fn b<const N: usize>(&self, at: usize) -> Result<[u8; N]> {
        self.0
            .get(at..at + N)
            .map(|s| s.try_into().unwrap())
            .ok_or(FmdlError::Truncated(at))
    }
    fn u8(&self, at: usize) -> Result<u8> {
        Ok(self.b::<1>(at)?[0])
    }
    fn u16(&self, at: usize) -> Result<u16> {
        Ok(u16::from_le_bytes(self.b(at)?))
    }
    fn i16(&self, at: usize) -> Result<i16> {
        Ok(self.u16(at)? as i16)
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
    fn v4(&self, at: usize) -> Result<[f32; 4]> {
        Ok([
            self.f32(at)?,
            self.f32(at + 4)?,
            self.f32(at + 8)?,
            self.f32(at + 12)?,
        ])
    }
    fn half(&self, at: usize) -> Result<f32> {
        Ok(half_to_f32(self.u16(at)?))
    }
}

/// IEEE 754 binary16 to f32.
pub fn half_to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) as u32) << 31;
    let exp = ((h >> 10) & 0x1f) as u32;
    let mant = (h & 0x3ff) as u32;
    let bits = match exp {
        0 if mant == 0 => sign,
        0 => {
            // Subnormal: normalise.
            let mut e = 127 - 15 + 1;
            let mut m = mant;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            sign | ((e as u32) << 23) | ((m & 0x3ff) << 13)
        }
        0x1f => sign | 0x7f80_0000 | (mant << 13),
        _ => sign | ((exp + 127 - 15) << 23) | (mant << 13),
    };
    f32::from_bits(bits)
}

/// Vertex attribute ids used by the vertex format table.
mod attr {
    pub const POSITION: u8 = 0;
    pub const BONE_WEIGHTS: u8 = 1;
    pub const NORMAL: u8 = 2;
    pub const COLOR: u8 = 3;
    pub const BONE_INDICES: u8 = 7;
    pub const UV0: u8 = 8;
    pub const UV3: u8 = 0xB;
    pub const TANGENT: u8 = 0xE;
}

impl Fmdl {
    /// The mesh group mesh `mesh` belongs to.
    pub fn mesh_group_of(&self, mesh: usize) -> Option<u16> {
        self.mesh_group_entries
            .iter()
            .find(|e| e.meshes.contains(&mesh))
            .map(|e| e.group)
    }

    /// Whether mesh `mesh` sits in a mesh group flagged invisible (or under
    /// one): helper geometry the game does not draw.
    pub fn mesh_hidden(&self, mesh: usize) -> bool {
        let mut cur = self.mesh_group_of(mesh).map(usize::from);
        let mut depth = 0;
        while let Some(g) = cur {
            let Some(mg) = self.mesh_groups.get(g) else { break };
            if mg.invisible {
                return true;
            }
            depth += 1;
            cur = (mg.parent >= 0 && depth < 16).then_some(mg.parent as usize);
        }
        false
    }

    /// The names (`names(name id)`) of mesh group `group` and its
    /// ancestors, innermost first.
    pub fn mesh_group_path(&self, group: u16, names: &dyn Fn(u16) -> String) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = Some(group as usize);
        while let Some(g) = cur {
            let Some(mg) = self.mesh_groups.get(g) else { break };
            out.push(names(mg.name));
            cur = (mg.parent >= 0 && out.len() < 16).then_some(mg.parent as usize);
        }
        out
    }

    pub fn parse(data: &[u8]) -> Result<Self> {
        let r = R(data);
        if r.b::<4>(0)? != *b"FMDL" {
            return Err(FmdlError::BadMagic);
        }
        let version = r.f32(4)?;
        let info_offset = r.u64(8)? as usize;
        let s0_count = r.u32(32)? as usize;
        let s1_count = r.u32(36)? as usize;
        let s0 = r.u32(40)? as usize;
        let s1 = r.u32(48)? as usize;

        // Section tables: type -> (count, absolute offset) / (offset, length).
        let mut s0_blocks = std::collections::HashMap::new();
        for i in 0..s0_count {
            let at = info_offset + i * 8;
            s0_blocks.insert(
                r.u16(at)?,
                (r.u16(at + 2)? as usize, s0 + r.u32(at + 4)? as usize),
            );
        }
        let mut s1_blocks = std::collections::HashMap::new();
        for i in 0..s1_count {
            let at = info_offset + s0_count * 8 + i * 12;
            s1_blocks.insert(
                r.u32(at)?,
                (s1 + r.u32(at + 4)? as usize, r.u32(at + 8)? as usize),
            );
        }
        let block = |t: u16| s0_blocks.get(&t).copied().unwrap_or((0, 0));
        let table = |t: u16, size: usize| -> Vec<usize> {
            let (n, at) = block(t);
            (0..n).map(|i| at + i * size).collect()
        };

        let mut m = Fmdl {
            version,
            ..Default::default()
        };

        for at in table(0, 48) {
            m.bones.push(Bone {
                name: r.u16(at)?,
                parent: r.i16(at + 2)?,
                bounding_box: r.u16(at + 4)?,
                local_position: r.v4(at + 16)?,
                world_position: r.v4(at + 32)?,
            });
        }
        for at in table(1, 8) {
            m.mesh_groups.push(MeshGroup {
                name: r.u16(at)?,
                invisible: r.u16(at + 2)? != 0,
                parent: r.i16(at + 4)?,
            });
        }
        for at in table(2, 32) {
            let (count, first) = (r.u16(at + 6)? as usize, r.u16(at + 8)? as usize);
            m.mesh_group_entries.push(MeshGroupEntry {
                group: r.u16(at + 4)?,
                meshes: first..first + count,
            });
        }
        for at in table(4, 16) {
            let (tc, pc) = (r.u8(at + 6)? as usize, r.u8(at + 7)? as usize);
            let (ft, fp) = (r.u16(at + 8)? as usize, r.u16(at + 10)? as usize);
            m.material_instances.push(MaterialInstance {
                name: r.u16(at)?,
                material: r.u16(at + 4)?,
                textures: ft..ft + tc,
                parameters: fp..fp + pc,
            });
        }
        for at in table(5, 68) {
            let n = (r.u16(at + 2)? as usize).min(32);
            m.bone_groups.push(
                (0..n)
                    .map(|k| r.u16(at + 4 + k * 2))
                    .collect::<Result<_>>()?,
            );
        }
        let pairs = |t: u16| -> Result<Vec<NamePair>> {
            table(t, 4)
                .into_iter()
                .map(|at| {
                    Ok(NamePair {
                        name: r.u16(at)?,
                        reference: r.u16(at + 2)?,
                    })
                })
                .collect()
        };
        m.textures = pairs(6)?;
        m.material_parameters = pairs(7)?;
        m.materials = pairs(8)?;
        for at in table(13, 32) {
            m.bounding_boxes.push((r.v4(at)?, r.v4(at + 16)?));
        }
        m.path_code64s = table(21, 8)
            .into_iter()
            .map(|at| r.u64(at))
            .collect::<Result<_>>()?;
        m.str_code64s = table(22, 8)
            .into_iter()
            .map(|at| r.u64(at))
            .collect::<Result<_>>()?;

        if let Some(&(at, len)) = s1_blocks.get(&0) {
            m.material_vectors = (0..len / 16)
                .map(|i| r.v4(at + i * 16))
                .collect::<Result<_>>()?;
        }
        if let Some(&(at, _)) = s1_blocks.get(&3) {
            for si in table(12, 8) {
                let (len, off) = (r.u16(si + 2)? as usize, r.u32(si + 4)? as usize);
                let bytes = data
                    .get(at + off..at + off + len)
                    .ok_or(FmdlError::Truncated(at + off))?;
                m.strings.push(String::from_utf8_lossy(bytes).into_owned());
            }
        }

        // Meshes and their buffers.
        let buffer = s1_blocks.get(&2).map(|&(at, _)| at);
        let buffer_offsets: Vec<usize> = table(14, 16)
            .into_iter()
            .map(|at| r.u32(at + 8).map(|o| o as usize))
            .collect::<Result<_>>()?;
        let format_infos = table(9, 8);
        let mesh_formats = table(10, 8);
        let vertex_formats = table(11, 4);
        for (i, at) in table(3, 48).into_iter().enumerate() {
            let mut mesh = Mesh {
                material_instance: r.u16(at + 4)?,
                bone_group: r.u16(at + 6)?,
                ..Default::default()
            };
            let vertex_count = r.u16(at + 10)? as usize;
            let first_index = r.u32(at + 16)? as usize;
            let index_count = r.u32(at + 20)? as usize;
            if let (Some(buf), Some(&fi)) = (buffer, format_infos.get(i)) {
                let first_mesh_format = r.u16(fi + 4)? as usize;
                let (vf_first, vf_count) = (r.u16(fi + 6)? as usize, r.u8(fi + 1)? as usize);
                let mf = |k: usize| -> Result<(usize, usize)> {
                    let at = *mesh_formats.get(first_mesh_format + k).ok_or_else(|| {
                        FmdlError::Invalid(format!("mesh {i}: missing mesh format {k}"))
                    })?;
                    Ok((r.u8(at + 2)? as usize, r.u32(at + 4)? as usize))
                };
                let buf_off = |k: usize| -> Result<usize> {
                    buffer_offsets
                        .get(k)
                        .map(|o| buf + o)
                        .ok_or_else(|| FmdlError::Invalid(format!("missing buffer offset {k}")))
                };

                // Stream 0: positions (float3).
                let (_, pos_off) = mf(0)?;
                let base = buf_off(0)? + pos_off;
                for v in 0..vertex_count {
                    let p = base + v * 12;
                    mesh.positions
                        .push([r.f32(p)?, r.f32(p + 4)?, r.f32(p + 8)?]);
                }

                // Stream 1: interleaved attributes.
                let attrs: Vec<(u8, usize)> = (vf_first..vf_first + vf_count)
                    .filter_map(|k| vertex_formats.get(k))
                    .map(|&vf| Ok((r.u8(vf)?, r.u16(vf + 2)? as usize)))
                    .collect::<Result<_>>()?;
                if attrs.iter().any(|(t, _)| *t != attr::POSITION) {
                    let (stride, off) = mf(1)?;
                    let base = buf_off(1)? + off;
                    for v in 0..vertex_count {
                        let vb = base + v * stride;
                        for &(t, o) in &attrs {
                            let p = vb + o;
                            match t {
                                attr::BONE_WEIGHTS => mesh.bone_weights.push(r.b(p)?),
                                attr::NORMAL => mesh.normals.push([
                                    r.half(p)?,
                                    r.half(p + 2)?,
                                    r.half(p + 4)?,
                                    r.half(p + 6)?,
                                ]),
                                attr::COLOR => mesh.colors.push(r.b(p)?),
                                attr::BONE_INDICES => mesh.bone_indices.push(r.b(p)?),
                                attr::UV0..=attr::UV3 => mesh.uvs[(t - attr::UV0) as usize]
                                    .push([r.half(p)?, r.half(p + 2)?]),
                                attr::TANGENT => mesh.tangents.push([
                                    r.half(p)?,
                                    r.half(p + 2)?,
                                    r.half(p + 4)?,
                                    r.half(p + 6)?,
                                ]),
                                _ => {}
                            }
                        }
                    }
                }

                // Stream 2: triangle indices.
                let base = buf_off(2)? + first_index * 2;
                mesh.indices = (0..index_count)
                    .map(|k| r.u16(base + k * 2))
                    .collect::<Result<_>>()?;
            }
            m.meshes.push(mesh);
        }
        Ok(m)
    }

    /// Name for a name index: the string table when present, otherwise the
    /// `StrCode64` table resolved through `names` (or the hex hash).
    pub fn name(&self, index: u16, names: &NameTable) -> String {
        let i = index as usize;
        if !self.strings.is_empty() {
            return self.strings.get(i).cloned().unwrap_or_default();
        }
        match self.str_code64s.get(i) {
            Some(&h) => names
                .get(h & 0xFFFF_FFFF_FFFF)
                .map(str::to_string)
                .unwrap_or_else(|| format!("{h:#x}")),
            None => format!("#{i}"),
        }
    }

    /// Basic consistency checks: indices in range, positions finite, bone
    /// references valid.
    pub fn validate(&self) -> Result<()> {
        for (i, m) in self.meshes.iter().enumerate() {
            let n = m.positions.len();
            if let Some(bad) = m.indices.iter().find(|&&x| x as usize >= n) {
                return Err(FmdlError::Invalid(format!(
                    "mesh {i}: index {bad} >= {n} vertices"
                )));
            }
            if m.positions.iter().flatten().any(|f| !f.is_finite()) {
                return Err(FmdlError::Invalid(format!("mesh {i}: non-finite position")));
            }
            if let Some(group) = self.bone_groups.get(m.bone_group as usize) {
                if let Some(bad) = m
                    .bone_indices
                    .iter()
                    .flatten()
                    .find(|&&b| (b as usize) >= group.len().max(1))
                {
                    return Err(FmdlError::Invalid(format!(
                        "mesh {i}: bone index {bad} outside group of {}",
                        group.len()
                    )));
                }
            }
        }
        for (i, b) in self.bones.iter().enumerate() {
            if b.parent >= self.bones.len() as i16 {
                return Err(FmdlError::Invalid(format!(
                    "bone {i}: parent {} out of range",
                    b.parent
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::half_to_f32;

    #[test]
    fn halves() {
        assert_eq!(half_to_f32(0x3c00), 1.0);
        assert_eq!(half_to_f32(0xc000), -2.0);
        assert_eq!(half_to_f32(0x0000), 0.0);
        assert_eq!(half_to_f32(0x7bff), 65504.0);
        assert!((half_to_f32(0x0001) - 5.960_464_5e-8).abs() < 1e-12);
    }
}
