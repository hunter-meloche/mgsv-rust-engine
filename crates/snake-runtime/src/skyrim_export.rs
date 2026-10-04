//! Export Snake's weapons from the user's MGSV install as Skyrim SE assets:
//! `Data\meshes\snake\<id>.nif` and `Data\textures\snake\<id>_<material>_{d,n}.dds`.
//! Generated locally from the user's own game files; nothing is shipped.
//!
//! Geometry is converted to Skyrim axes and units (`(-x, z, y) * 70`, the
//! same mapping as the skeleton retarget), so the mesh's frame is the gun
//! frame of [`snake_core::WeaponRig::gun_transform`] seen from Skyrim.
//! Textures: the base colour is passed through block-compressed; the
//! two-channel normal map (x in alpha, y in green) is rebuilt as a full
//! normal with the specular map's red channel as Skyrim's specular mask.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use fox_archive::Vfs;
use fox_data::ftex::{self, PixelFormat};
use fox_data::{Fmdl, Ftex};
use skyrim_data::dds::{self, DdsFormat};
use skyrim_data::nif_write::{self, Shape};

use crate::{read_any, WEAPONS};

const UNITS_PER_METER: f32 = 70.0;

fn to_sky(v: [f32; 3]) -> [f32; 3] {
    [-v[0], v[2], v[1]]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-6 { [v[0] / l, v[1] / l, v[2] / l] } else { [0.0, 0.0, 1.0] }
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// Skyrim mesh path (relative to `Data\meshes`) of weapon `id`.
pub fn mesh_path(id: &str) -> String {
    format!("snake\\{}.nif", id.to_ascii_lowercase())
}

/// A texture of an `.fmdl` material slot: header and decoded-size mips.
struct Texture {
    ftex: Ftex,
    mips: Vec<Vec<u8>>,
}

fn read_texture(vfs: &Vfs, path_code: u64) -> Result<Texture> {
    let stem = path_code & ((1u64 << 51) - 1);
    let read = |ext: &str| {
        vfs.lookup_hash(((fox_hash::extension_id(ext) as u64) << 51) | stem)
            .and_then(|r| vfs.read_ref(r).ok())
    };
    let header = read("ftex").with_context(|| format!("texture {path_code:#x} not found"))?;
    let ftex = Ftex::parse(&header)?;
    let mips = ftex.read_mips(|n| read(&format!("{n}.ftexs")))?;
    anyhow::ensure!(!mips.is_empty(), "texture {path_code:#x}: no mip data");
    Ok(Texture { ftex, mips })
}

/// Texture of material `mi` in slot `slot` (e.g. `Base_Tex_SRGB`).
fn material_texture(vfs: &Vfs, m: &Fmdl, mi: usize, slot: &str) -> Option<Result<Texture>> {
    let want = fox_hash::str_code64(slot.as_bytes()) & 0xFFFF_FFFF_FFFF;
    let inst = m.material_instances.get(mi)?;
    let param = m.material_parameters.get(inst.textures.clone())?.iter().find(|p| {
        m.str_code64s.get(p.name as usize).map(|h| h & 0xFFFF_FFFF_FFFF) == Some(want)
            || m.strings.get(p.name as usize).map(String::as_str) == Some(slot)
    })?;
    let tex = m.textures.get(param.reference as usize)?;
    let pc = *m.path_code64s.get(tex.reference as usize)?;
    Some(read_texture(vfs, pc))
}

fn dds_format(f: PixelFormat) -> Option<DdsFormat> {
    match f {
        PixelFormat::Bc1 => Some(DdsFormat::Bc1),
        PixelFormat::Bc2 => Some(DdsFormat::Bc2),
        PixelFormat::Bc3 => Some(DdsFormat::Bc3),
        _ => None,
    }
}

/// Diffuse DDS: block-compressed data passed through, else decoded BGRA.
fn diffuse_dds(t: &Texture) -> Result<Vec<u8>> {
    if let Some(f) = dds_format(t.ftex.format) {
        return Ok(dds::write_dds(f, t.ftex.width, t.ftex.height, &t.mips));
    }
    let mut mips = Vec::new();
    for (i, m) in t.mips.iter().enumerate() {
        let (w, h) = t.ftex.mip_dims(i);
        mips.push(dds::rgba_to_bgra(&ftex::decode_rgba(t.ftex.format, w, h, m)?));
    }
    Ok(dds::write_dds(DdsFormat::Bgra8, t.ftex.width, t.ftex.height, &mips))
}

/// Normal DDS (BGRA, alpha = specular mask) from the two-channel normal
/// map and the specular map's red channel.
fn normal_dds(normal: &Texture, specular: Option<&Texture>) -> Result<Vec<u8>> {
    let mut mips = Vec::new();
    for (i, m) in normal.mips.iter().enumerate() {
        let (w, h) = normal.ftex.mip_dims(i);
        let mut px = ftex::decode_rgba(normal.ftex.format, w, h, m)?;
        // Specular mask at the same level (nearest sample if sizes differ).
        let spec = specular.and_then(|s| {
            let level = s.mips.len().min(i + 1) - 1;
            let (sw, sh) = s.ftex.mip_dims(level);
            ftex::decode_rgba(s.ftex.format, sw, sh, &s.mips[level])
                .ok()
                .map(|p| (p, sw, sh))
        });
        for (k, p) in px.iter_mut().enumerate() {
            let x = p[3] as f32 / 127.5 - 1.0;
            let y = p[1] as f32 / 127.5 - 1.0;
            let z = (1.0 - x * x - y * y).max(0.0).sqrt();
            let mask = match &spec {
                // MGSV's specular map is bright (red averages ~230 on the
                // MRS-4); scaled to the range of vanilla weapon masks.
                Some((s, sw, sh)) => {
                    let (px_x, px_y) = ((k as u32 % w) * sw / w, (k as u32 / w) * sh / h);
                    (s[(px_y * sw + px_x) as usize][0] as u32 * 3 / 10) as u8
                }
                None => 40,
            };
            let b = |v: f32| ((v.clamp(-1.0, 1.0) + 1.0) * 127.5).round() as u8;
            *p = [b(x), b(y), b(z), mask];
        }
        mips.push(dds::rgba_to_bgra(&px));
    }
    Ok(dds::write_dds(DdsFormat::Bgra8, normal.ftex.width, normal.ftex.height, &mips))
}

/// What [`export_weapons`] wrote.
#[derive(Debug, Default)]
pub struct ExportReport {
    pub files: Vec<PathBuf>,
    pub notes: Vec<String>,
}

/// Write every weapon in [`WEAPONS`] under `data_dir` (Skyrim's `Data`).
pub fn export_weapons(vfs: &Vfs, data_dir: &Path) -> Result<ExportReport> {
    let mut report = ExportReport::default();
    for def in &WEAPONS {
        export_weapon(vfs, def.id, def.model, data_dir, &mut report)
            .with_context(|| format!("exporting {}", def.id))?;
    }
    Ok(report)
}

fn export_weapon(vfs: &Vfs, id: &str, model: &str, data_dir: &Path, report: &mut ExportReport) -> Result<()> {
    let m = Fmdl::parse(&read_any(vfs, model)?)?;
    let stem = id.to_ascii_lowercase();
    let tex_dir = data_dir.join("textures").join("snake");
    let mesh_dir = data_dir.join("meshes").join("snake");
    std::fs::create_dir_all(&tex_dir)?;
    std::fs::create_dir_all(&mesh_dir)?;

    // Textures per material.
    let mut material_textures = Vec::new();
    for mi in 0..m.material_instances.len() {
        let mut slots = vec![String::new(); 2];
        match material_texture(vfs, &m, mi, "Base_Tex_SRGB") {
            Some(Ok(base)) => {
                let path = tex_dir.join(format!("{stem}_{mi}_d.dds"));
                std::fs::write(&path, diffuse_dds(&base)?)?;
                slots[0] = format!("textures\\snake\\{stem}_{mi}_d.dds");
                report.files.push(path);
            }
            Some(Err(e)) => report.notes.push(format!("{id} material {mi} base colour: {e}")),
            None => {}
        }
        match material_texture(vfs, &m, mi, "NormalMap_Tex_NRM") {
            Some(Ok(normal)) => {
                let spec = material_texture(vfs, &m, mi, "SpecularMap_Tex_LIN").and_then(Result::ok);
                let path = tex_dir.join(format!("{stem}_{mi}_n.dds"));
                std::fs::write(&path, normal_dds(&normal, spec.as_ref())?)?;
                slots[1] = format!("textures\\snake\\{stem}_{mi}_n.dds");
                report.files.push(path);
            }
            Some(Err(e)) => report.notes.push(format!("{id} material {mi} normal map: {e}")),
            None => {}
        }
        material_textures.push(slots);
    }

    // One shape per mesh, in Skyrim axes and units.
    let mut shapes = Vec::new();
    for (i, mesh) in m.meshes.iter().enumerate() {
        if mesh.positions.len() > u16::MAX as usize {
            report.notes.push(format!("{id}: mesh {i} has too many vertices; skipped"));
            continue;
        }
        let normals: Vec<[f32; 3]> = mesh
            .normals
            .iter()
            .map(|n| normalize(to_sky([n[0], n[1], n[2]])))
            .collect();
        let mut tangents = Vec::with_capacity(normals.len());
        let mut bitangents = Vec::with_capacity(normals.len());
        for (k, n) in normals.iter().enumerate() {
            let t4 = mesh.tangents.get(k).copied().unwrap_or([1.0, 0.0, 0.0, 1.0]);
            let t = normalize(to_sky([t4[0], t4[1], t4[2]]));
            let sign = if t4[3] < 0.0 { -1.0 } else { 1.0 };
            let b = cross(*n, t);
            tangents.push(t);
            bitangents.push([b[0] * sign, b[1] * sign, b[2] * sign]);
        }
        shapes.push(Shape {
            name: format!("{stem}:{i}"),
            positions: mesh
                .positions
                .iter()
                .map(|p| {
                    let s = to_sky(*p);
                    [s[0] * UNITS_PER_METER, s[1] * UNITS_PER_METER, s[2] * UNITS_PER_METER]
                })
                .collect(),
            normals,
            tangents,
            bitangents,
            uvs: mesh.uvs[0].clone(),
            triangles: mesh
                .indices
                .chunks_exact(3)
                .map(|t| [t[0], t[1], t[2]])
                .collect(),
            textures: material_textures
                .get(mesh.material_instance as usize)
                .cloned()
                .unwrap_or_default(),
            glossiness: 80.0,
            specular_strength: 1.0,
        });
    }
    let path = mesh_dir.join(format!("{stem}.nif"));
    std::fs::write(&path, nif_write::write_static_nif(&stem, &shapes))?;
    report.files.push(path);
    Ok(())
}
