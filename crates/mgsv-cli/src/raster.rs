//! A small software rasterizer for the `mgsv play` test window: z-buffered,
//! flat-shaded triangles. Development tool only; the real renderer will be
//! the host engine's (Skyrim via the SKSE bridge, or a GPU backend).

use fox_anim::glam::{Mat4, Vec3, Vec4};

pub struct Raster {
    pub width: usize,
    pub height: usize,
    pub color: Vec<u32>,
    depth: Vec<f32>,
}

impl Raster {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            color: vec![0; width * height],
            depth: vec![f32::INFINITY; width * height],
        }
    }

    pub fn clear(&mut self, sky_top: Vec3, sky_bottom: Vec3) {
        for y in 0..self.height {
            let t = y as f32 / self.height as f32;
            let c = rgb(sky_top.lerp(sky_bottom, t));
            self.color[y * self.width..(y + 1) * self.width].fill(c);
        }
        self.depth.fill(f32::INFINITY);
    }

    /// Draw world-space triangles `(a, b, c)` with a base colour each, lit by
    /// a directional light (two-sided), or unlit (emissive) with `None`.
    pub fn draw(
        &mut self,
        view_proj: &Mat4,
        light: Option<Vec3>,
        tris: impl Iterator<Item = ([Vec3; 3], Vec3)>,
    ) {
        let (w, h) = (self.width as f32, self.height as f32);
        for (p, base) in tris {
            let n = (p[1] - p[0]).cross(p[2] - p[0]);
            let len = n.length();
            if len < 1e-12 {
                continue;
            }
            let lit = light.map_or(1.0, |l| 0.35 + 0.65 * (n / len).dot(l).abs());
            let col = rgb(base * lit);
            let clip: [Vec4; 3] = p.map(|v| *view_proj * v.extend(1.0));
            // Reject triangles touching the near plane (simple; the camera
            // keeps the character well in front).
            if clip.iter().any(|c| c.w < 0.05) {
                continue;
            }
            let s: [Vec3; 3] = clip.map(|c| {
                let ndc = c.truncate() / c.w;
                Vec3::new((ndc.x + 1.0) * 0.5 * w, (1.0 - ndc.y) * 0.5 * h, ndc.z)
            });
            self.fill(&s, col);
        }
    }

    fn fill(&mut self, s: &[Vec3; 3], col: u32) {
        let area = edge(s[0], s[1], s[2]);
        if area.abs() < 1e-6 {
            return;
        }
        let min_x = s.iter().map(|v| v.x).fold(f32::MAX, f32::min).max(0.0) as usize;
        let max_x = (s
            .iter()
            .map(|v| v.x)
            .fold(f32::MIN, f32::max)
            .min(self.width as f32 - 1.0)) as isize;
        let min_y = s.iter().map(|v| v.y).fold(f32::MAX, f32::min).max(0.0) as usize;
        let max_y = (s
            .iter()
            .map(|v| v.y)
            .fold(f32::MIN, f32::max)
            .min(self.height as f32 - 1.0)) as isize;
        if max_x < 0 || max_y < 0 {
            return;
        }
        for y in min_y..=max_y as usize {
            for x in min_x..=max_x as usize {
                let p = Vec3::new(x as f32 + 0.5, y as f32 + 0.5, 0.0);
                let w0 = edge(s[1], s[2], p) / area;
                let w1 = edge(s[2], s[0], p) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = w0 * s[0].z + w1 * s[1].z + w2 * s[2].z;
                let i = y * self.width + x;
                if z < self.depth[i] {
                    self.depth[i] = z;
                    self.color[i] = col;
                }
            }
        }
    }
}

fn edge(a: Vec3, b: Vec3, p: Vec3) -> f32 {
    (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)
}

pub fn rgb(c: Vec3) -> u32 {
    let c = (c.clamp(Vec3::ZERO, Vec3::ONE) * 255.0).round();
    ((c.x as u32) << 16) | ((c.y as u32) << 8) | c.z as u32
}

/// Save the colour buffer as a PNG (for automated checks of the renderer).
pub fn save_png(r: &Raster, path: &std::path::Path) -> std::io::Result<()> {
    use std::io::Write;
    let mut raw = Vec::with_capacity((r.width * 3 + 1) * r.height);
    for y in 0..r.height {
        raw.push(0); // filter: none
        for &c in &r.color[y * r.width..(y + 1) * r.width] {
            raw.extend_from_slice(&[(c >> 16) as u8, (c >> 8) as u8, c as u8]);
        }
    }
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(&raw)?;
    let idat = z.finish()?;
    let mut out = vec![0x89u8, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut chunk = |kind: &[u8], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let crc = crc32(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(r.width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(r.height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &idat);
    chunk(b"IEND", &[]);
    std::fs::write(path, out)
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

impl Raster {
    /// Draw a small screen-centre crosshair.
    pub fn crosshair(&mut self, color: u32) {
        let (cx, cy) = (self.width / 2, self.height / 2);
        for d in 4..10 {
            for (x, y) in [(cx + d, cy), (cx - d, cy), (cx, cy + d), (cx, cy - d)] {
                self.color[y * self.width + x] = color;
            }
        }
    }
}
