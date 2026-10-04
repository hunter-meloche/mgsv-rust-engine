//! Minimal NIF reader for Skyrim SE files (version 20.2.0.7, BS version 100):
//! the header, string table, block sizes, and the node tree (`NiNode` and
//! subclasses) with each node's name, local transform and children. Other
//! block types are skipped using the header's block-size table.

#[derive(Debug, thiserror::Error)]
pub enum NifError {
    #[error("not a supported NIF: {0}")]
    Format(String),
    #[error("truncated at {0:#x}")]
    Truncated(usize),
}

type Result<T> = std::result::Result<T, NifError>;

#[derive(Clone, Debug)]
pub struct NifNode {
    pub block: usize,
    pub kind: String,
    pub name: String,
    pub translation: [f32; 3],
    /// Row-major 3x3 rotation as stored in the file.
    pub rotation: [[f32; 3]; 3],
    pub scale: f32,
    /// Block indices of child nodes (non-node children are dropped).
    pub children: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct Nif {
    pub version: u32,
    pub user_version: u32,
    pub bs_version: u32,
    pub block_types: Vec<String>,
    /// Block index -> node (None for non-node blocks).
    pub nodes: Vec<Option<NifNode>>,
}

struct R<'a> {
    b: &'a [u8],
    p: usize,
}

impl R<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        let s = self
            .b
            .get(self.p..self.p + n)
            .ok_or(NifError::Truncated(self.p))?;
        self.p += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }
    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }
    fn sized_string(&mut self) -> Result<String> {
        let n = self.u32()? as usize;
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
    fn short_string(&mut self) -> Result<String> {
        let n = self.u8()? as usize;
        Ok(String::from_utf8_lossy(self.take(n)?)
            .trim_end_matches('\0')
            .to_string())
    }
}

/// Block types read as nodes (NiNode layout).
const NODE_TYPES: &[&str] = &[
    "NiNode",
    "BSFadeNode",
    "BSLeafAnimNode",
    "BSTreeNode",
    "BSMultiBoundNode",
    "NiBillboardNode",
    "BSOrderedNode",
    "BSValueNode",
    "BSDamageStage",
    "BSBlastNode",
    "BSDebrisNode",
    "NiSwitchNode",
    "NiLODNode",
];

impl Nif {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let nl = data
            .iter()
            .position(|&b| b == b'\n')
            .ok_or_else(|| NifError::Format("no header line".into()))?;
        let line = String::from_utf8_lossy(&data[..nl]).into_owned();
        if !line.starts_with("Gamebryo File Format") {
            return Err(NifError::Format(line));
        }
        let mut r = R { b: data, p: nl + 1 };
        let version = r.u32()?;
        if version != 0x1402_0007 {
            return Err(NifError::Format(format!("version {version:#x}")));
        }
        let _endian = r.u8()?;
        let user_version = r.u32()?;
        let num_blocks = r.u32()? as usize;
        let bs_version = r.u32()?;
        let _author = r.short_string()?;
        if bs_version > 130 {
            let _unknown = r.u32()?;
        }
        let _process = r.short_string()?;
        let _export = r.short_string()?;
        if bs_version >= 103 {
            let _max_path = r.short_string()?;
        }
        let num_types = r.u16()? as usize;
        let block_types = (0..num_types)
            .map(|_| r.sized_string())
            .collect::<Result<Vec<_>>>()?;
        let type_of = (0..num_blocks)
            .map(|_| r.u16().map(|t| (t & 0x7FFF) as usize))
            .collect::<Result<Vec<_>>>()?;
        let sizes = (0..num_blocks)
            .map(|_| r.u32().map(|s| s as usize))
            .collect::<Result<Vec<_>>>()?;
        let num_strings = r.u32()? as usize;
        let _max_len = r.u32()?;
        let strings = (0..num_strings)
            .map(|_| r.sized_string())
            .collect::<Result<Vec<_>>>()?;
        let num_groups = r.u32()? as usize;
        for _ in 0..num_groups {
            r.u32()?;
        }

        let mut nodes = Vec::with_capacity(num_blocks);
        for (i, (&t, &size)) in type_of.iter().zip(&sizes).enumerate() {
            let start = r.p;
            let kind = block_types.get(t).cloned().unwrap_or_default();
            if NODE_TYPES.contains(&kind.as_str()) {
                let mut b = R {
                    b: &data[..start + size],
                    p: start,
                };
                let name_idx = b.i32()?;
                let name = usize::try_from(name_idx)
                    .ok()
                    .and_then(|i| strings.get(i).cloned())
                    .unwrap_or_default();
                let extra = b.u32()? as usize;
                b.take(extra * 4)?;
                let _controller = b.i32()?;
                let _flags = if bs_version > 26 {
                    b.u32()?
                } else {
                    b.u16()? as u32
                };
                let translation = [b.f32()?, b.f32()?, b.f32()?];
                let mut rotation = [[0f32; 3]; 3];
                for row in &mut rotation {
                    for v in row.iter_mut() {
                        *v = b.f32()?;
                    }
                }
                let scale = b.f32()?;
                if bs_version <= 34 {
                    let props = b.u32()? as usize;
                    b.take(props * 4)?;
                }
                let _collision = b.i32()?;
                let n = b.u32()? as usize;
                let children = (0..n)
                    .filter_map(|_| b.i32().ok())
                    .filter_map(|c| usize::try_from(c).ok())
                    .collect();
                nodes.push(Some(NifNode {
                    block: i,
                    kind,
                    name,
                    translation,
                    rotation,
                    scale,
                    children,
                }));
            } else {
                nodes.push(None);
            }
            r.p = start + size;
        }
        // Keep only node children.
        let is_node: Vec<bool> = nodes.iter().map(Option::is_some).collect();
        for n in nodes.iter_mut().flatten() {
            n.children
                .retain(|&c| is_node.get(c).copied().unwrap_or(false));
        }
        Ok(Self {
            version,
            user_version,
            bs_version,
            block_types,
            nodes,
        })
    }

    /// The first node (block 0 in skeleton files).
    pub fn root(&self) -> Option<&NifNode> {
        self.nodes.iter().flatten().next()
    }

    pub fn node(&self, block: usize) -> Option<&NifNode> {
        self.nodes.get(block)?.as_ref()
    }

    pub fn find(&self, name: &str) -> Option<&NifNode> {
        self.nodes.iter().flatten().find(|n| n.name == name)
    }

    /// Parent block of each node block.
    pub fn parents(&self) -> Vec<Option<usize>> {
        let mut parent = vec![None; self.nodes.len()];
        for n in self.nodes.iter().flatten() {
            for &c in &n.children {
                parent[c] = Some(n.block);
            }
        }
        parent
    }
}
