//! Minimal `.dds` writer: block-compressed data passed through (BC1/BC2/BC3)
//! or uncompressed 32-bit BGRA, with a full or partial mip chain.

/// Pixel data of a DDS image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DdsFormat {
    Bc1,
    Bc2,
    Bc3,
    /// 32-bit, bytes in B, G, R, A order.
    Bgra8,
}

impl DdsFormat {
    fn four_cc(self) -> Option<&'static [u8; 4]> {
        match self {
            Self::Bc1 => Some(b"DXT1"),
            Self::Bc2 => Some(b"DXT3"),
            Self::Bc3 => Some(b"DXT5"),
            Self::Bgra8 => None,
        }
    }

    /// Bytes for a `width` x `height` image.
    pub fn image_size(self, width: u32, height: u32) -> usize {
        let blocks = (width.div_ceil(4) * height.div_ceil(4)) as usize;
        match self {
            Self::Bc1 => blocks * 8,
            Self::Bc2 | Self::Bc3 => blocks * 16,
            Self::Bgra8 => (width * height * 4) as usize,
        }
    }
}

/// A DDS file for `mips` (largest first; each the exact size of its level).
pub fn write_dds(format: DdsFormat, width: u32, height: u32, mips: &[Vec<u8>]) -> Vec<u8> {
    const CAPS: u32 = 0x1;
    const HEIGHT: u32 = 0x2;
    const WIDTH: u32 = 0x4;
    const PITCH: u32 = 0x8;
    const PIXELFORMAT: u32 = 0x1000;
    const MIPMAPCOUNT: u32 = 0x20000;
    const LINEARSIZE: u32 = 0x80000;
    let mut h = Vec::with_capacity(128);
    let mut put = |v: u32| h.extend_from_slice(&v.to_le_bytes());
    let compressed = format.four_cc().is_some();
    put(u32::from_le_bytes(*b"DDS "));
    put(124);
    put(CAPS | HEIGHT | WIDTH | PIXELFORMAT | MIPMAPCOUNT | if compressed { LINEARSIZE } else { PITCH });
    put(height);
    put(width);
    put(if compressed { format.image_size(width, height) as u32 } else { width * 4 });
    put(0); // depth
    put(mips.len() as u32);
    for _ in 0..11 {
        put(0);
    }
    // Pixel format.
    put(32);
    match format.four_cc() {
        Some(cc) => {
            put(0x4); // FOURCC
            put(u32::from_le_bytes(*cc));
            for _ in 0..5 {
                put(0);
            }
        }
        None => {
            put(0x41); // RGB | ALPHAPIXELS
            put(0);
            put(32);
            put(0x00FF_0000);
            put(0x0000_FF00);
            put(0x0000_00FF);
            put(0xFF00_0000);
        }
    }
    put(0x1000 | if mips.len() > 1 { 0x40_0008 } else { 0 }); // TEXTURE | COMPLEX | MIPMAP
    for _ in 0..4 {
        put(0);
    }
    let mut out = h;
    for m in mips {
        out.extend_from_slice(m);
    }
    out
}

/// Box-filtered half-size image (RGBA8, row-major).
pub fn downsample(px: &[[u8; 4]], width: u32, height: u32) -> (Vec<[u8; 4]>, u32, u32) {
    let (w2, h2) = ((width / 2).max(1), (height / 2).max(1));
    let mut out = Vec::with_capacity((w2 * h2) as usize);
    for y in 0..h2 {
        for x in 0..w2 {
            let mut sum = [0u32; 4];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let sx = (x * 2 + dx).min(width - 1);
                let sy = (y * 2 + dy).min(height - 1);
                let p = px[(sy * width + sx) as usize];
                for k in 0..4 {
                    sum[k] += p[k] as u32;
                }
            }
            out.push([(sum[0] / 4) as u8, (sum[1] / 4) as u8, (sum[2] / 4) as u8, (sum[3] / 4) as u8]);
        }
    }
    (out, w2, h2)
}

/// BGRA bytes of an RGBA image.
pub fn rgba_to_bgra(px: &[[u8; 4]]) -> Vec<u8> {
    px.iter().flat_map(|p| [p[2], p[1], p[0], p[3]]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_is_128_bytes_and_data_follows() {
        let mip = vec![7u8; 8];
        let f = write_dds(DdsFormat::Bc1, 4, 4, &[mip]);
        assert_eq!(&f[0..4], b"DDS ");
        assert_eq!(f.len(), 128 + 8);
        assert_eq!(&f[84..88], b"DXT1");
    }
}
