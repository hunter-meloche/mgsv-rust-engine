//! `.ftex` textures: a small header with a mip table, and the pixel data in
//! sibling `N.ftexs` files (the largest mips in the highest-numbered file).
//!
//! Layout (little-endian; FtexTool by Atvaark documents the format):
//! * header: magic `FTEX`, version (f32, 2.03/2.04), pixel format (u16),
//!   width, height, depth (u16), mip count (u8), ... ; the mip table starts
//!   at 0x40 with 16-byte entries `(u32 offset, u32 size, u32 stored size,
//!   u8 mip index, u8 ftexs file number, u16 chunk count)`;
//! * a mip in an `.ftexs` starts at its offset with `chunk count` 8-byte
//!   chunk headers `(u16 stored size, u16 size, u32 offset)`; the offset is
//!   relative to the mip's start and its top bit marks a chunk stored as is,
//!   otherwise the chunk is zlib-compressed.

use std::io::Read;

#[derive(Debug, thiserror::Error)]
pub enum FtexError {
    #[error("not an ftex file")]
    BadMagic,
    #[error("truncated at {0:#x}")]
    Truncated(usize),
    #[error("{0}")]
    Invalid(String),
}

type Result<T> = std::result::Result<T, FtexError>;

/// Pixel formats used by TPP textures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    /// 32-bit, bytes in R, G, B, A order.
    Rgba8,
    Bc1,
    Bc2,
    Bc3,
    Other(u16),
}

impl PixelFormat {
    fn from_id(id: u16) -> Self {
        match id {
            0 => Self::Rgba8,
            2 => Self::Bc1,
            3 => Self::Bc2,
            4 => Self::Bc3,
            n => Self::Other(n),
        }
    }

    /// Bytes for a `width` x `height` image.
    pub fn image_size(self, width: u32, height: u32) -> Option<usize> {
        let blocks = (width.div_ceil(4) * height.div_ceil(4)) as usize;
        match self {
            Self::Rgba8 => Some((width * height * 4) as usize),
            Self::Bc1 => Some(blocks * 8),
            Self::Bc2 | Self::Bc3 => Some(blocks * 16),
            Self::Other(_) => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Mip {
    pub offset: u32,
    pub size: u32,
    pub stored_size: u32,
    pub index: u8,
    /// Which `N.ftexs` file holds this mip.
    pub file: u8,
    pub chunks: u16,
}

#[derive(Clone, Debug)]
pub struct Ftex {
    pub version: f32,
    pub format: PixelFormat,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    /// Largest first.
    pub mips: Vec<Mip>,
}

fn u16_at(b: &[u8], at: usize) -> Result<u16> {
    b.get(at..at + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or(FtexError::Truncated(at))
}

fn u32_at(b: &[u8], at: usize) -> Result<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or(FtexError::Truncated(at))
}

impl Ftex {
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.get(0..4) != Some(b"FTEX") {
            return Err(FtexError::BadMagic);
        }
        let mip_count = *data.get(0x10).ok_or(FtexError::Truncated(0x10))? as usize;
        let mut mips = Vec::with_capacity(mip_count);
        for i in 0..mip_count {
            let at = 0x40 + i * 16;
            mips.push(Mip {
                offset: u32_at(data, at)?,
                size: u32_at(data, at + 4)?,
                stored_size: u32_at(data, at + 8)?,
                index: *data.get(at + 12).ok_or(FtexError::Truncated(at + 12))?,
                file: *data.get(at + 13).ok_or(FtexError::Truncated(at + 13))?,
                chunks: u16_at(data, at + 14)?,
            });
        }
        mips.sort_by_key(|m| m.index);
        Ok(Self {
            version: f32::from_bits(u32_at(data, 4)?),
            format: PixelFormat::from_id(u16_at(data, 8)?),
            width: u16_at(data, 0x0A)? as u32,
            height: u16_at(data, 0x0C)? as u32,
            depth: u16_at(data, 0x0E)? as u32,
            mips,
        })
    }

    /// Size of mip level `level`.
    pub fn mip_dims(&self, level: usize) -> (u32, u32) {
        ((self.width >> level).max(1), (self.height >> level).max(1))
    }

    /// The pixel data of each mip, largest first. `part(n)` returns the
    /// contents of `n.ftexs` (None if missing); mips from a missing part
    /// end the list.
    pub fn read_mips(&self, mut part: impl FnMut(u8) -> Option<Vec<u8>>) -> Result<Vec<Vec<u8>>> {
        let mut files: std::collections::HashMap<u8, Option<Vec<u8>>> = Default::default();
        let mut out = Vec::new();
        for mip in &self.mips {
            let file = files.entry(mip.file).or_insert_with(|| part(mip.file));
            let Some(file) = file.as_deref() else {
                break;
            };
            out.push(read_mip(file, mip)?);
        }
        Ok(out)
    }
}

fn read_mip(file: &[u8], mip: &Mip) -> Result<Vec<u8>> {
    let base = mip.offset as usize;
    let mut out = Vec::with_capacity(mip.size as usize);
    for c in 0..mip.chunks as usize {
        let at = base + c * 8;
        let stored = u16_at(file, at)? as usize;
        let size = u16_at(file, at + 2)? as usize;
        let off = u32_at(file, at + 4)?;
        let raw = off & 0x8000_0000 != 0;
        let start = base + (off & 0x7FFF_FFFF) as usize;
        let bytes = file
            .get(start..start + stored)
            .ok_or(FtexError::Truncated(start))?;
        // Sizes are u16, so a 64 KiB chunk is stored as 0.
        let size = if size == 0 { 0x10000 } else { size };
        if raw || stored == size {
            out.extend_from_slice(bytes);
        } else {
            let mut d = flate2::read::ZlibDecoder::new(bytes);
            let before = out.len();
            d.read_to_end(&mut out)
                .map_err(|e| FtexError::Invalid(format!("chunk {c}: {e}")))?;
            if out.len() - before != size {
                return Err(FtexError::Invalid(format!(
                    "chunk {c}: {} bytes, expected {size}",
                    out.len() - before
                )));
            }
        }
    }
    if out.len() != mip.size as usize {
        return Err(FtexError::Invalid(format!(
            "mip {}: {} bytes, expected {}",
            mip.index,
            out.len(),
            mip.size
        )));
    }
    Ok(out)
}

/// Decode an image to RGBA8 (row-major, top row first).
pub fn decode_rgba(format: PixelFormat, width: u32, height: u32, data: &[u8]) -> Result<Vec<[u8; 4]>> {
    let (w, h) = (width as usize, height as usize);
    let need = format
        .image_size(width, height)
        .ok_or_else(|| FtexError::Invalid(format!("unsupported pixel format {format:?}")))?;
    if data.len() < need {
        return Err(FtexError::Truncated(data.len()));
    }
    let mut out = vec![[0u8; 4]; w * h];
    if format == PixelFormat::Rgba8 {
        for (o, px) in out.iter_mut().zip(data.chunks_exact(4)) {
            *o = [px[0], px[1], px[2], px[3]];
        }
        return Ok(out);
    }
    let block_size = if format == PixelFormat::Bc1 { 8 } else { 16 };
    let bw = w.div_ceil(4);
    for (i, block) in data[..need].chunks_exact(block_size).enumerate() {
        let (bx, by) = ((i % bw) * 4, (i / bw) * 4);
        let mut px = [[0u8; 4]; 16];
        match format {
            PixelFormat::Bc1 => decode_color(block, true, &mut px),
            PixelFormat::Bc2 => {
                decode_color(&block[8..], false, &mut px);
                for (k, p) in px.iter_mut().enumerate() {
                    let nibble = (block[k / 2] >> ((k % 2) * 4)) & 0xF;
                    p[3] = nibble * 17;
                }
            }
            _ => {
                decode_color(&block[8..], false, &mut px);
                decode_alpha(&block[..8], &mut px);
            }
        }
        for (k, p) in px.iter().enumerate() {
            let (x, y) = (bx + k % 4, by + k / 4);
            if x < w && y < h {
                out[y * w + x] = *p;
            }
        }
    }
    Ok(out)
}

fn rgb565(c: u16) -> [u32; 3] {
    let r = ((c >> 11) & 0x1F) as u32;
    let g = ((c >> 5) & 0x3F) as u32;
    let b = (c & 0x1F) as u32;
    [(r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2)]
}

/// BC1 colour block (also the colour half of BC2/BC3, which always use the
/// four-colour mode).
fn decode_color(b: &[u8], bc1: bool, px: &mut [[u8; 4]; 16]) {
    let c0 = u16::from_le_bytes([b[0], b[1]]);
    let c1 = u16::from_le_bytes([b[2], b[3]]);
    let (a, z) = (rgb565(c0), rgb565(c1));
    let mix = |wa: u32, wb: u32, d: u32| -> [u8; 4] {
        [
            ((a[0] * wa + z[0] * wb) / d) as u8,
            ((a[1] * wa + z[1] * wb) / d) as u8,
            ((a[2] * wa + z[2] * wb) / d) as u8,
            255,
        ]
    };
    let palette = if !bc1 || c0 > c1 {
        [mix(1, 0, 1), mix(0, 1, 1), mix(2, 1, 3), mix(1, 2, 3)]
    } else {
        [mix(1, 0, 1), mix(0, 1, 1), mix(1, 1, 2), [0, 0, 0, 0]]
    };
    let bits = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    for (k, p) in px.iter_mut().enumerate() {
        *p = palette[((bits >> (k * 2)) & 3) as usize];
    }
}

/// BC3/BC4 alpha block.
fn decode_alpha(b: &[u8], px: &mut [[u8; 4]; 16]) {
    let (a0, a1) = (b[0] as u32, b[1] as u32);
    let mut table = [0u32; 8];
    table[0] = a0;
    table[1] = a1;
    if a0 > a1 {
        for i in 1..7 {
            table[i + 1] = ((7 - i as u32) * a0 + i as u32 * a1) / 7;
        }
    } else {
        for i in 1..5 {
            table[i + 1] = ((5 - i as u32) * a0 + i as u32 * a1) / 5;
        }
        table[6] = 0;
        table[7] = 255;
    }
    let mut bits = 0u64;
    for (i, &v) in b[2..8].iter().enumerate() {
        bits |= (v as u64) << (8 * i);
    }
    for (k, p) in px.iter_mut().enumerate() {
        p[3] = table[((bits >> (k * 3)) & 7) as usize] as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bc1_block_decodes_endpoints() {
        // c0 = pure red, c1 = pure blue, all texels index 0 except the
        // second (index 1).
        let block = [0x00, 0xF8, 0x1F, 0x00, 0b0000_0100, 0, 0, 0];
        let img = decode_rgba(PixelFormat::Bc1, 4, 4, &block).unwrap();
        assert_eq!(img[0], [255, 0, 0, 255]);
        assert_eq!(img[1], [0, 0, 255, 255]);
    }

    #[test]
    fn bc3_alpha_interpolates() {
        let mut block = [0u8; 16];
        block[0] = 255;
        block[1] = 0;
        // texel 0 -> index 1 (a1 = 0), texel 1 -> index 0 (255)
        block[2] = 0b0000_0001;
        let img = decode_rgba(PixelFormat::Bc3, 4, 4, &block).unwrap();
        assert_eq!(img[0][3], 0);
        assert_eq!(img[1][3], 255);
    }
}
