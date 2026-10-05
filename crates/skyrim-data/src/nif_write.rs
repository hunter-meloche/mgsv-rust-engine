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

/// `BSLightingShaderProperty` (default shader type) using texture set block
/// `texture_set`; `skinned` sets the Skinned flag skinned meshes need.
fn lighting_shader(s: &Shape, texture_set: i32, skinned: bool) -> Vec<u8> {
    let mut m = W::default();
    m.u32(0); // shader type
    m.i32(-1); // name
    m.u32(0); // extra data
    m.i32(-1); // controller
    m.u32(SHADER_FLAGS_1 | if skinned { 0x2 } else { 0 });
    m.u32(SHADER_FLAGS_2);
    for v in [0.0, 0.0, 1.0, 1.0] {
        m.f32(v); // uv offset, uv scale
    }
    m.i32(texture_set);
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
    m.0
}

/// `BSShaderTextureSet` with 9 slots.
fn texture_set(s: &Shape) -> Vec<u8> {
    let mut t = W::default();
    t.u32(9);
    for k in 0..9 {
        t.sized_string(s.textures.get(k).map_or("", String::as_str));
    }
    t.0
}

/// One vertex's packed position, bitangent, UV, normal and tangent (the
/// first 28 bytes of an SSE vertex).
fn packed_vertex(w: &mut W, s: &Shape, v: usize) {
    let p = s.positions[v];
    let n = s.normals.get(v).copied().unwrap_or([0.0, 0.0, 1.0]);
    let t = s.tangents.get(v).copied().unwrap_or([1.0, 0.0, 0.0]);
    let bt = s.bitangents.get(v).copied().unwrap_or([0.0, 1.0, 0.0]);
    let uv = s.uvs.get(v).copied().unwrap_or([0.0, 0.0]);
    w.f32(p[0]);
    w.f32(p[1]);
    w.f32(p[2]);
    w.f32(bt[0]);
    w.u16(f32_to_f16(uv[0]));
    w.u16(f32_to_f16(uv[1]));
    for k in n {
        w.u8(unit_byte(k));
    }
    w.u8(unit_byte(bt[1]));
    for k in t {
        w.u8(unit_byte(k));
    }
    w.u8(unit_byte(bt[2]));
}

/// A bone of a skinned mesh: its name (matched to the skeleton the mesh is
/// worn on) and its bind transform in the mesh's space (`v' = R v + t`,
/// `rotation` row-major).
#[derive(Clone, Debug)]
pub struct SkinBone {
    pub name: String,
    pub rotation: [[f32; 3]; 3],
    pub translation: [f32; 3],
}

/// A mesh part skinned to [`SkinBone`]s: up to four `(bone, weight)` per
/// vertex (weights summing to 1; unused slots weight 0).
#[derive(Clone, Debug, Default)]
pub struct SkinnedShape {
    pub shape: Shape,
    pub weights: Vec<[(u8, f32); 4]>,
}

/// Vertex layout of skinned shapes: 10 dwords; UV at 4, normal at 5, tangent
/// at 6, skin data at 7; vertex | uv | normal | tangent | skinned (as the
/// vanilla `malebody_1.nif`).
const SKINNED_VERTEX_DESC: u64 = 0x0005_B000_7065_040A;

/// A NIF of skinned meshes in the SSE layout (as vanilla body meshes): an
/// `NiNode` root, one `NiNode` per bone at its bind transform, and per shape
/// a vertex-less `BSTriShape`, a `BSDismemberSkinInstance` (one body-part
/// partition), `NiSkinData` (inverse bind transforms and weights), an
/// `NiSkinPartition` holding the vertices, a lit shader and a texture set.
pub fn write_skinned_nif(root_name: &str, bones: &[SkinBone], shapes: &[SkinnedShape]) -> Vec<u8> {
    let mut strings = vec![root_name.to_string()];
    strings.extend(bones.iter().map(|b| b.name.clone()));
    strings.extend(shapes.iter().map(|s| s.shape.name.clone()));
    let types = [
        "NiNode",
        "BSTriShape",
        "BSDismemberSkinInstance",
        "NiSkinData",
        "NiSkinPartition",
        "BSLightingShaderProperty",
        "BSShaderTextureSet",
    ];
    let first_shape = 1 + bones.len() as i32;
    let mut blocks: Vec<(u16, Vec<u8>)> = Vec::new();

    let mut root = W::default();
    root.av_object(0);
    root.u32((bones.len() + shapes.len()) as u32);
    for i in 0..bones.len() + shapes.len() {
        root.i32(if i < bones.len() { 1 + i as i32 } else { first_shape + 6 * (i - bones.len()) as i32 });
    }
    root.u32(0); // effects
    blocks.push((0, root.0));

    for (i, b) in bones.iter().enumerate() {
        let mut n = W::default();
        n.i32(1 + i as i32);
        n.u32(0);
        n.i32(-1);
        n.u32(0x0008_000E);
        for v in b.translation {
            n.f32(v);
        }
        for row in b.rotation {
            for v in row {
                n.f32(v);
            }
        }
        n.f32(1.0);
        n.i32(-1); // collision
        n.u32(0); // children
        n.u32(0); // effects
        blocks.push((0, n.0));
    }

    for (k, sk) in shapes.iter().enumerate() {
        let s = &sk.shape;
        let base = first_shape + 6 * k as i32;
        let nv = s.positions.len();
        // BSTriShape: geometry lives in the skin partition.
        let mut t = W::default();
        t.av_object(1 + bones.len() as i32 + k as i32);
        let (c, r) = bounding_sphere(&s.positions);
        for v in c {
            t.f32(v);
        }
        t.f32(r);
        t.i32(base + 1); // skin
        t.i32(base + 4); // shader
        t.i32(-1); // alpha
        t.0.extend_from_slice(&SKINNED_VERTEX_DESC.to_le_bytes());
        t.u16(0);
        t.u16(0);
        t.u32(0);
        t.u32(0); // particle data
        blocks.push((1, t.0));

        // Bones this shape uses (all of them: one partition, identity map).
        let mut d = W::default();
        d.i32(base + 2); // skin data
        d.i32(base + 3); // skin partition
        d.i32(0); // skeleton root
        d.u32(bones.len() as u32);
        for i in 0..bones.len() {
            d.i32(1 + i as i32);
        }
        d.u32(1); // partitions
        d.u16(0x0101); // part flags (editor visible | start net boneset)
        d.u16(32); // body part: body
        blocks.push((2, d.0));

        // NiSkinData: identity skin transform, per bone the inverse bind
        // transform, a bone-space bound and the vertex weights.
        let mut sd = W::default();
        for v in [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] {
            sd.f32(v);
        }
        sd.u32(bones.len() as u32);
        sd.u8(1); // has vertex weights
        for (bi, b) in bones.iter().enumerate() {
            let r = b.rotation;
            let inv = [
                [r[0][0], r[1][0], r[2][0]],
                [r[0][1], r[1][1], r[2][1]],
                [r[0][2], r[1][2], r[2][2]],
            ];
            let it = [
                -(inv[0][0] * b.translation[0] + inv[0][1] * b.translation[1] + inv[0][2] * b.translation[2]),
                -(inv[1][0] * b.translation[0] + inv[1][1] * b.translation[1] + inv[1][2] * b.translation[2]),
                -(inv[2][0] * b.translation[0] + inv[2][1] * b.translation[1] + inv[2][2] * b.translation[2]),
            ];
            for row in inv {
                for v in row {
                    sd.f32(v);
                }
            }
            for v in it {
                sd.f32(v);
            }
            sd.f32(1.0);
            let influenced: Vec<(u16, f32)> = (0..nv)
                .filter_map(|v| {
                    sk.weights[v]
                        .iter()
                        .find(|(i, w)| *i as usize == bi && *w > 0.0)
                        .map(|(_, w)| (v as u16, *w))
                })
                .collect();
            let local: Vec<[f32; 3]> = influenced
                .iter()
                .map(|(v, _)| {
                    let p = s.positions[*v as usize];
                    [
                        inv[0][0] * p[0] + inv[0][1] * p[1] + inv[0][2] * p[2] + it[0],
                        inv[1][0] * p[0] + inv[1][1] * p[1] + inv[1][2] * p[2] + it[1],
                        inv[2][0] * p[0] + inv[2][1] * p[1] + inv[2][2] * p[2] + it[2],
                    ]
                })
                .collect();
            let (bc, br) = bounding_sphere(&local);
            for v in bc {
                sd.f32(v);
            }
            sd.f32(br);
            sd.u16(influenced.len() as u16);
            for (v, w) in influenced {
                sd.u16(v);
                sd.f32(w);
            }
        }
        blocks.push((3, sd.0));

        // NiSkinPartition: the vertices (with weights as halves and bone
        // indices), then one partition over all of them.
        let mut sp = W::default();
        sp.u32(1);
        sp.u32((nv * 40) as u32);
        sp.u32(40);
        sp.0.extend_from_slice(&SKINNED_VERTEX_DESC.to_le_bytes());
        for v in 0..nv {
            packed_vertex(&mut sp, s, v);
            for (_, w) in sk.weights[v] {
                sp.u16(f32_to_f16(w));
            }
            for (i, _) in sk.weights[v] {
                sp.u8(i);
            }
        }
        sp.u16(nv as u16);
        sp.u16(s.triangles.len() as u16);
        sp.u16(bones.len() as u16);
        sp.u16(0); // strips
        sp.u16(4); // weights per vertex
        for i in 0..bones.len() {
            sp.u16(i as u16);
        }
        sp.u8(1); // vertex map
        for v in 0..nv {
            sp.u16(v as u16);
        }
        sp.u8(1); // vertex weights
        for v in 0..nv {
            for (_, w) in sk.weights[v] {
                sp.f32(w);
            }
        }
        sp.u8(1); // faces
        for t in &s.triangles {
            for &i in t {
                sp.u16(i);
            }
        }
        sp.u8(1); // bone indices
        for v in 0..nv {
            for (i, _) in sk.weights[v] {
                sp.u8(i);
            }
        }
        sp.u8(0); // LOD level
        sp.u8(0); // global VB
        sp.0.extend_from_slice(&SKINNED_VERTEX_DESC.to_le_bytes());
        for t in &s.triangles {
            for &i in t {
                sp.u16(i);
            }
        }
        blocks.push((4, sp.0));

        blocks.push((5, lighting_shader(s, base + 5, true)));
        blocks.push((6, texture_set(s)));
    }
    finish_nif(&types, &blocks, &strings)
}

/// Header, blocks and footer of a NIF with the given block types and strings.
fn finish_nif(types: &[&str], blocks: &[(u16, Vec<u8>)], strings: &[String]) -> Vec<u8> {
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
    for (t, _) in blocks {
        w.u16(*t);
    }
    for (_, b) in blocks {
        w.u32(b.len() as u32);
    }
    w.u32(strings.len() as u32);
    w.u32(strings.iter().map(String::len).max().unwrap_or(0) as u32);
    for s in strings {
        w.sized_string(s);
    }
    w.u32(0); // groups
    for (_, b) in blocks {
        w.0.extend_from_slice(b);
    }
    w.u32(1); // roots
    w.i32(0);
    w.0
}

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

        blocks.push((2, lighting_shader(s, shape_block + 2, false)));
        blocks.push((3, texture_set(s)));
    }

    finish_nif(&types, &blocks, &strings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skinned_nif_layout() {
        let bone = |name: &str, z: f32| SkinBone {
            name: name.into(),
            rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            translation: [0.0, 0.0, z],
        };
        let shape = SkinnedShape {
            shape: Shape {
                name: "body".into(),
                positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 10.0], [0.0, 1.0, 20.0]],
                triangles: vec![[0, 1, 2]],
                ..Default::default()
            },
            weights: vec![[(0, 1.0), (0, 0.0), (0, 0.0), (0, 0.0)]; 3],
        };
        let data = write_skinned_nif("Snake", &[bone("NPC Pelvis [Pelv]", 0.0), bone("NPC Spine [Spn0]", 10.0)], &[shape]);
        let nif = crate::Nif::parse(&data).unwrap();
        let kinds: Vec<&str> = nif.blocks.iter().map(|b| b.0.as_str()).collect();
        assert_eq!(
            kinds,
            [
                "NiNode", "NiNode", "NiNode", "BSTriShape", "BSDismemberSkinInstance", "NiSkinData",
                "NiSkinPartition", "BSLightingShaderProperty", "BSShaderTextureSet"
            ]
        );
        assert_eq!(nif.blocks[3].1.len(), 120); // as vanilla skinned BSTriShape
        assert_eq!(nif.blocks[4].1.len(), 4 * 4 + 2 * 4 + 4 + 4);
        // partition: header 20 + 3 vertices x 40 + partition data
        let part = 10 + 2 * 2 + 1 + 3 * 2 + 1 + 3 * 4 * 4 + 1 + 6 + 1 + 3 * 4 + 2 + 8 + 6;
        assert_eq!(nif.blocks[6].1.len(), 20 + 3 * 40 + part);
    }

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
