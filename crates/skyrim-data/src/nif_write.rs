//! Writer for simple static Skyrim SE meshes: an `NiNode` root with one
//! `BSTriShape` per part, each with a `BSLightingShaderProperty` and a
//! `BSShaderTextureSet`. Layouts follow vanilla SSE files (NIF 20.2.0.7,
//! user version 12, BS version 100), e.g. `meshes\weapons\iron\longsword.nif`:
//! 28-byte vertices (position + bitangent x as floats, UV as halves, normal
//! and tangent as bytes carrying bitangent y and z).

/// One mesh part. Positions in Skyrim units; normals, tangents and
/// bitangents unit length; UVs with v down (DirectX).
#[derive(Clone, Debug, Default)]
pub struct Shape {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub tangents: Vec<[f32; 3]>,
    pub bitangents: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub triangles: Vec<[u16; 3]>,
    /// Texture paths relative to `Data` (e.g. `textures\snake\gun_d.dds`):
    /// diffuse, normal (+ specular mask in alpha), then the other slots.
    pub textures: Vec<String>,
    pub glossiness: f32,
    pub specular_strength: f32,
    /// Draw as an unlit additive effect (`BSEffectShaderProperty` +
    /// additive `NiAlphaProperty`) with this source texture instead of the
    /// lit material, e.g. a muzzle flash.
    pub effect: Option<Effect>,
}

/// An unlit, additive, double-sided effect material.
#[derive(Clone, Debug)]
pub struct Effect {
    pub texture: String,
    pub emissive_multiple: f32,
}

#[derive(Default)]
struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn sized_string(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.0.extend_from_slice(s.as_bytes());
    }
    fn export_string(&mut self, s: &str) {
        self.u8(s.len() as u8 + 1);
        self.0.extend_from_slice(s.as_bytes());
        self.u8(0);
    }
    /// NiObjectNET + NiAVObject fields (identity transform, no extra data,
    /// controller or collision).
    fn av_object(&mut self, name: i32) {
        self.i32(name);
        self.u32(0); // extra data
        self.i32(-1); // controller
        self.u32(0x0008_000E); // flags, as in vanilla meshes
        for v in [0.0, 0.0, 0.0] {
            self.f32(v); // translation
        }
        for v in [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] {
            self.f32(v); // rotation
        }
        self.f32(1.0); // scale
        self.i32(-1); // collision object
    }
}

/// IEEE half from f32 (round to nearest, no NaN handling needed for UVs).
pub fn f32_to_f16(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xFF) as i32 - 127 + 15;
    let mant = bits & 0x7F_FFFF;
    if exp <= 0 {
        if exp < -10 {
            return sign;
        }
        let m = (mant | 0x80_0000) >> (1 - exp);
        return sign | ((m + 0x1000) >> 13) as u16;
    }
    if exp >= 31 {
        return sign | 0x7C00;
    }
    let h = ((exp as u32) << 10) | (mant >> 13);
    let round = (mant >> 12) & 1;
    sign | (h + round) as u16
}

fn unit_byte(v: f32) -> u8 {
    ((v.clamp(-1.0, 1.0) + 1.0) * 127.5).round() as u8
}

fn bounding_sphere(points: &[[f32; 3]]) -> ([f32; 3], f32) {
    if points.is_empty() {
        return ([0.0; 3], 0.0);
    }
    let mut lo = points[0];
    let mut hi = points[0];
    for p in points {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
    let r = points
        .iter()
        .map(|p| ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2)).sqrt())
        .fold(0.0f32, f32::max);
    (c, r)
}

/// Shape flags of vanilla weapons: specular, receive/cast shadows, z test;
/// z write and environment-map light fade.
const SHADER_FLAGS_1: u32 = 0x8240_0301;
const SHADER_FLAGS_2: u32 = 0x0000_8001;
/// Vertex layout: 7 dwords; UV at dword 4, normal at 5, tangent at 6;
/// attributes vertex | uv | normal | tangent.
const VERTEX_DESC: u64 = 0x0001_B000_0065_0407;

/// A NIF file with `shapes` under an `NiNode` named `root_name`.
pub fn write_static_nif(root_name: &str, shapes: &[Shape]) -> Vec<u8> {
    let mut strings = vec![root_name.to_string()];
    strings.extend(shapes.iter().map(|s| s.name.clone()));
    // The root is a plain NiNode: these meshes are attached to an actor's
    // skeleton by hand, outside Skyrim's own attach path that keeps a
    // BSFadeNode's fade state current.
    let types = [
        "NiNode",
        "BSTriShape",
        "BSLightingShaderProperty",
        "BSShaderTextureSet",
        "BSEffectShaderProperty",
        "NiAlphaProperty",
    ];

    let mut blocks: Vec<(u16, Vec<u8>)> = Vec::new();
    // Root.
    let mut root = W::default();
    root.av_object(0);
    root.u32(shapes.len() as u32);
    for i in 0..shapes.len() {
        root.i32(1 + 3 * i as i32);
    }
    root.u32(0); // effects
    blocks.push((0, root.0));

    for (i, s) in shapes.iter().enumerate() {
        let shape_block = 1 + 3 * i as i32;
        // BSTriShape
        let mut b = W::default();
        b.av_object(1 + i as i32);
        let (c, r) = bounding_sphere(&s.positions);
        for v in c {
            b.f32(v);
        }
        b.f32(r);
        b.i32(-1); // skin
        b.i32(shape_block + 1); // shader property
        b.i32(if s.effect.is_some() { shape_block + 2 } else { -1 }); // alpha property
        b.0.extend_from_slice(&VERTEX_DESC.to_le_bytes());
        b.u16(s.triangles.len() as u16);
        b.u16(s.positions.len() as u16);
        b.u32((s.positions.len() * 28 + s.triangles.len() * 6) as u32);
        for v in 0..s.positions.len() {
            let p = s.positions[v];
            let n = s.normals.get(v).copied().unwrap_or([0.0, 0.0, 1.0]);
            let t = s.tangents.get(v).copied().unwrap_or([1.0, 0.0, 0.0]);
            let bt = s.bitangents.get(v).copied().unwrap_or([0.0, 1.0, 0.0]);
            let uv = s.uvs.get(v).copied().unwrap_or([0.0, 0.0]);
            b.f32(p[0]);
            b.f32(p[1]);
            b.f32(p[2]);
            b.f32(bt[0]);
            b.u16(f32_to_f16(uv[0]));
            b.u16(f32_to_f16(uv[1]));
            for k in n {
                b.u8(unit_byte(k));
            }
            b.u8(unit_byte(bt[1]));
            for k in t {
                b.u8(unit_byte(k));
            }
            b.u8(unit_byte(bt[2]));
        }
        for t in &s.triangles {
            for &k in t {
                b.u16(k);
            }
        }
        b.u32(0); // particle data size
        blocks.push((1, b.0));

        if let Some(fx) = &s.effect {
            // BSEffectShaderProperty, laid out as in vanilla effect meshes.
            let mut e = W::default();
            e.i32(-1); // name
            e.u32(0); // extra data
            e.i32(-1); // controller
            e.u32(0x8000_0000); // flags 1: z test
            e.u32(0x0000_0010); // flags 2: double sided
            for v in [0.0, 0.0, 1.0, 1.0] {
                e.f32(v); // uv offset, uv scale
            }
            e.sized_string(&fx.texture);
            e.u8(3); // texture clamp: wrap
            e.u8(0); // lighting influence: unlit
            e.u8(0); // env map min lod
            e.u8(0);
            for v in [1.0, 1.0, 1.0, 1.0] {
                e.f32(v); // falloff start/stop angle, start/stop opacity
            }
            for v in [1.0, 1.0, 1.0, 1.0] {
                e.f32(v); // emissive colour
            }
            e.f32(fx.emissive_multiple);
            e.f32(100.0); // soft falloff depth
            e.sized_string(""); // greyscale texture
            blocks.push((4, e.0));
            // NiAlphaProperty: blend src alpha + one (additive), no z sort.
            let mut a = W::default();
            a.i32(-1);
            a.u32(0);
            a.i32(-1);
            a.u16(0x100D);
            a.u8(0);
            blocks.push((5, a.0));
            continue;
        }

        // BSLightingShaderProperty (default shader type).
        let mut m = W::default();
        m.u32(0); // shader type
        m.i32(-1); // name
        m.u32(0); // extra data
        m.i32(-1); // controller
        m.u32(SHADER_FLAGS_1);
        m.u32(SHADER_FLAGS_2);
        for v in [0.0, 0.0, 1.0, 1.0] {
            m.f32(v); // uv offset, uv scale
        }
        m.i32(shape_block + 2); // texture set
        for v in [0.0, 0.0, 0.0, 1.0] {
            m.f32(v); // emissive colour, emissive multiple
        }
        m.u32(3); // texture clamp: wrap s, wrap t
        m.f32(1.0); // alpha
        m.f32(0.0); // refraction strength
        m.f32(s.glossiness);
        for v in [1.0, 1.0, 1.0] {
            m.f32(v); // specular colour
        }
        m.f32(s.specular_strength);
        m.f32(0.3); // lighting effect 1
        m.f32(2.0); // lighting effect 2
        blocks.push((2, m.0));

        // BSShaderTextureSet (9 slots).
        let mut t = W::default();
        t.u32(9);
        for k in 0..9 {
            t.sized_string(s.textures.get(k).map_or("", String::as_str));
        }
        blocks.push((3, t.0));
    }

    let mut w = W::default();
    w.0.extend_from_slice(b"Gamebryo File Format, Version 20.2.0.7\n");
    w.u32(0x1402_0007);
    w.u8(1);
    w.u32(12);
    w.u32(blocks.len() as u32);
    w.u32(100);
    w.export_string("mgsv-rust-engine");
    w.export_string("");
    w.export_string("");
    w.u16(types.len() as u16);
    for t in types {
        w.sized_string(t);
    }
    for (t, _) in &blocks {
        w.u16(*t);
    }
    for (_, b) in &blocks {
        w.u32(b.len() as u32);
    }
    w.u32(strings.len() as u32);
    w.u32(strings.iter().map(String::len).max().unwrap_or(0) as u32);
    for s in &strings {
        w.sized_string(s);
    }
    w.u32(0); // groups
    for (_, b) in &blocks {
        w.0.extend_from_slice(b);
    }
    w.u32(1); // roots
    w.i32(0);
    w.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats() {
        assert_eq!(f32_to_f16(0.0), 0);
        assert_eq!(f32_to_f16(1.0), 0x3C00);
        assert_eq!(f32_to_f16(0.5), 0x3800);
        assert_eq!(f32_to_f16(-2.0), 0xC000);
    }

    #[test]
    fn written_nif_reads_back() {
        let shape = Shape {
            name: "part".into(),
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            triangles: vec![[0, 1, 2]],
            textures: vec!["textures\\a.dds".into()],
            glossiness: 50.0,
            specular_strength: 1.0,
            ..Default::default()
        };
        let data = write_static_nif("Gun", &[shape]);
        let nif = crate::Nif::parse(&data).unwrap();
        assert_eq!(nif.blocks.len(), 4);
        assert_eq!(nif.blocks[1].0, "BSTriShape");
        assert_eq!(nif.blocks[1].1.len(), 0x74 + 3 * 28 + 6 + 4);
        assert_eq!(nif.blocks[2].1.len(), 100);
        let root = nif.root().unwrap();
        assert_eq!(root.name, "Gun");
        assert_eq!(nif.strings, vec!["Gun", "part"]);
    }
}
