//! `.fcnp` connection points: named attachment transforms on a model
//! (`CNP_BARREL`, `CNP_STOCK`, hand grips on characters, ...), each parented
//! to one of the model's bones.
//!
//! Layout (worked out from retail files):
//!
//! * Header (32 bytes): `u32 0`, `u32` first node offset, `u32` file size,
//!   `StrCode32` model name, `u32` name string offset (relative to the hash).
//! * Nodes (0x60 bytes, linked): `StrCode32` name, `u32` name offset
//!   (relative to the node), `u32 0`, `u32 0x30`, `u32 0x30`, 2 × `u32 0`,
//!   `i32` previous-node offset, `i32` next-node offset (0 ends the list),
//!   `u32` parameter-block offset (relative to the node), 2 × `u32` pad;
//!   then position (float4), rotation quaternion (float4), scale (float4).
//! * Parameter block: `u32 count`, then `(StrCode32 key, u32 key offset,
//!   StrCode32 value, u32 value offset)`; each string offset is relative to
//!   the hash before it. The `Parent` key names the bone.

#[derive(Debug, thiserror::Error)]
pub enum FcnpError {
    #[error("truncated at {0:#x}")]
    Truncated(usize),
    #[error("bad node chain at {0:#x}")]
    Chain(usize),
}

type Result<T> = std::result::Result<T, FcnpError>;

#[derive(Clone, Debug)]
pub struct ConnectPoint {
    pub name: String,
    pub position: [f32; 3],
    /// Quaternion `[x, y, z, w]`.
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    /// `(key, value)` parameters, e.g. `("Parent", "SKL_000_ROOT")`.
    pub params: Vec<(String, String)>,
}

impl ConnectPoint {
    pub fn parent(&self) -> Option<&str> {
        self.params
            .iter()
            .find(|(k, _)| k == "Parent")
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Clone, Debug)]
pub struct Fcnp {
    pub model: String,
    pub points: Vec<ConnectPoint>,
}

struct R<'a>(&'a [u8]);

impl R<'_> {
    fn u32(&self, at: usize) -> Result<u32> {
        self.0
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .ok_or(FcnpError::Truncated(at))
    }
    fn f32(&self, at: usize) -> Result<f32> {
        Ok(f32::from_bits(self.u32(at)?))
    }
    fn cstr(&self, at: usize) -> Result<String> {
        let tail = self.0.get(at..).ok_or(FcnpError::Truncated(at))?;
        let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
        Ok(String::from_utf8_lossy(&tail[..end]).into_owned())
    }
    /// String at an offset stored at `at`, relative to `base`.
    fn rel_str(&self, base: usize, at: usize) -> Result<String> {
        self.cstr(base + self.u32(at)? as usize)
    }
}

impl Fcnp {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let r = R(data);
        let model = r.rel_str(0x0c, 0x10)?;
        let mut points = Vec::new();
        let mut at = r.u32(4)? as usize;
        while at != 0 {
            if points.len() > 4096 {
                return Err(FcnpError::Chain(at));
            }
            let name = r.rel_str(at, at + 4)?;
            let v = |o: usize| r.f32(at + o);
            let params_at = at + r.u32(at + 0x24)? as usize;
            let mut params = Vec::new();
            let count = r.u32(params_at)? as usize;
            for k in 0..count.min(256) {
                let e = params_at + 4 + k * 16;
                params.push((r.rel_str(e, e + 4)?, r.rel_str(e + 8, e + 12)?));
            }
            points.push(ConnectPoint {
                name,
                position: [v(0x30)?, v(0x34)?, v(0x38)?],
                rotation: [v(0x40)?, v(0x44)?, v(0x48)?, v(0x4c)?],
                scale: [v(0x50)?, v(0x54)?, v(0x58)?],
                params,
            });
            let next = r.u32(at + 0x20)? as i32;
            if next == 0 {
                break;
            }
            let n = at as i64 + next as i64;
            if n <= at as i64 || n as usize >= data.len() {
                return Err(FcnpError::Chain(at));
            }
            at = n as usize;
        }
        Ok(Self { model, points })
    }
}
