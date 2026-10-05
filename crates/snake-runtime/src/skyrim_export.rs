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

/// `format` is the expected pixel format, used only when the `.ftex` header
/// is missing and the texture is rebuilt from its `.ftexs` parts.
fn read_texture(vfs: &Vfs, path_code: u64, format: PixelFormat) -> Result<Texture> {
    let stem = path_code & ((1u64 << 51) - 1);
    let read = |ext: &str| {
        vfs.lookup_hash(((fox_hash::extension_id(ext) as u64) << 51) | stem)
            .and_then(|r| vfs.read_ref(r).ok())
    };
    let Some(header) = read("ftex") else {
        let parts: Vec<(u8, Vec<u8>)> = (1..=6u8)
            .filter_map(|n| read(&format!("{n}.ftexs")).map(|d| (n, d)))
            .collect();
        anyhow::ensure!(!parts.is_empty(), "texture {path_code:#x} not found");
        let (ftex, mips) = Ftex::from_parts(format, &parts)
            .with_context(|| format!("texture {path_code:#x} (no .ftex header)"))?;
        return Ok(Texture { ftex, mips });
    };
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
    // TPP normal maps are BC3 (x in alpha, y in green); the rest BC1.
    let format = if slot.starts_with("NormalMap") { PixelFormat::Bc3 } else { PixelFormat::Bc1 };
    Some(read_texture(vfs, pc, format))
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
    // The muzzle flash is its own mesh, attached beside the gun: an
    // additive shape inside the gun's own node makes Skyrim draw the whole
    // gun in the transparent pass (it turned pale while flashing).
    write_muzzle_flash_texture(vfs, data_dir, &mut report)?;
    let flash_nif =
        nif_write::write_static_nif("muzzle_flash", &muzzle_flash_shapes([0.0; 3], to_sky([0.0, 0.0, 1.0])));
    for name in ["muzzle_flash.nif", "muzzle_flash_1p.nif"] {
        let flash = data_dir.join("meshes").join("snake").join(name);
        std::fs::create_dir_all(flash.parent().unwrap())?;
        std::fs::write(&flash, &flash_nif)?;
        report.files.push(flash);
    }
    for def in &WEAPONS {
        export_weapon(vfs, def.id, def.model, data_dir, &mut report)
            .with_context(|| format!("exporting {}", def.id))?;
        if let Some((bank, ids)) = def.shot_sound {
            for (k, &id) in ids.iter().enumerate() {
                let path = sound_path(data_dir, def.id, k);
                export_sound(vfs, bank, id, &path).with_context(|| format!("{} shot sound {id}", def.id))?;
                report.files.push(path);
            }
        }
    }
    Ok(report)
}

/// Where the plugin looks for variation `k` of weapon `id`'s gunshot:
/// `<id>_shot.wav`, `<id>_shot1.wav`, `<id>_shot2.wav`, ...
pub fn sound_path(data_dir: &Path, id: &str, k: usize) -> PathBuf {
    let suffix = if k == 0 { String::new() } else { k.to_string() };
    data_dir
        .join("SKSE")
        .join("Plugins")
        .join("snake_sounds")
        .join(format!("{}_shot{suffix}.wav", id.to_ascii_lowercase()))
}

/// Write sound `id` of the `.sbp` at `bank` (an archive path) as a `.wav`.
pub fn export_sound(vfs: &Vfs, bank: &str, id: u32, out: &Path) -> Result<()> {
    let data = read_any(vfs, bank)?;
    let pcm = fox_data::sbp::SoundBank::parse(&data)?.pcm(id)?;
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(out, pcm.to_wav())?;
    Ok(())
}

/// MGSV's muzzle flame (2x2 flipbook, BC3), used for every weapon's flash.
const MUZZLE_FLASH_TEXTURE: &str = "/Assets/tpp/effect/vfx_pic/fire/fx_mzffir01_hh_alp_clp.ftex";
const MUZZLE_FLASH_DDS: &str = r"textures\snake\muzzle_flash.dds";

/// Name prefix of the flash shapes (`<prefix><cell>`), toggled by the plugin.
pub const MUZZLE_FLASH_PREFIX: &str = "muzzle_flash:";

fn write_muzzle_flash_texture(vfs: &Vfs, data_dir: &Path, report: &mut ExportReport) -> Result<()> {
    let path = data_dir.join("textures").join("snake").join("muzzle_flash.dds");
    if report.files.contains(&path) {
        return Ok(());
    }
    let header = vfs.read(MUZZLE_FLASH_TEXTURE)?;
    let ftex = Ftex::parse(&header)?;
    let stem = MUZZLE_FLASH_TEXTURE.trim_end_matches(".ftex");
    let mips = ftex.read_mips(|n| vfs.read(&format!("{stem}.{n}.ftexs")).ok())?;
    std::fs::write(&path, diffuse_dds(&Texture { ftex, mips })?)?;
    report.files.push(path);
    Ok(())
}

/// Skyrim mesh path of the muzzle flash (relative to `Data\meshes`); it is
/// built at its origin along +Y, the gun frame's barrel axis in Skyrim axes.
pub const MUZZLE_FLASH_MESH: &str = r"snake\muzzle_flash.nif";

/// Muzzle of `muzzle` (Fox gun frame, metres) in the exported gun mesh's
/// frame (Skyrim axes and units).
pub fn muzzle_in_mesh(muzzle: [f32; 3]) -> [f32; 3] {
    let m = to_sky(muzzle);
    [m[0] * UNITS_PER_METER, m[1] * UNITS_PER_METER, m[2] * UNITS_PER_METER]
}

/// Flash shapes at the muzzle `m` (Skyrim units) along the barrel `axis`:
/// two crossed flame quads plus a front burst, one shape per flipbook cell.
fn muzzle_flash_shapes(m: [f32; 3], axis: [f32; 3]) -> Vec<Shape> {
    let a = normalize(axis);
    let helper = if a[2].abs() < 0.9 { [0.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0] };
    let u = normalize(cross(a, helper));
    let v = cross(a, u);
    let (length, width, burst) = (26.0f32, 13.0f32, 9.0f32);
    let at = |base: [f32; 3], d1: [f32; 3], s1: f32, d2: [f32; 3], s2: f32| {
        [
            base[0] + d1[0] * s1 + d2[0] * s2,
            base[1] + d1[1] * s1 + d2[1] * s2,
            base[2] + d1[2] * s1 + d2[2] * s2,
        ]
    };
    (0..4)
        .map(|cell| {
            let (u0, v0) = ((cell % 2) as f32 * 0.5, (cell / 2) as f32 * 0.5);
            let mut sh = Shape {
                name: format!("{MUZZLE_FLASH_PREFIX}{cell}"),
                effect: Some(nif_write::Effect {
                    texture: MUZZLE_FLASH_DDS.to_string(),
                    emissive_multiple: 2.0,
                }),
                ..Default::default()
            };
            // Flame quads: base at the muzzle (bottom of the cell), tip
            // along the barrel (top of the cell).
            for side in [u, v] {
                let base = sh.positions.len() as u16;
                for (along, across, uv) in [
                    (-2.0, -width / 2.0, [u0, v0 + 0.5]),
                    (-2.0, width / 2.0, [u0 + 0.5, v0 + 0.5]),
                    (length, width / 2.0, [u0 + 0.5, v0]),
                    (length, -width / 2.0, [u0, v0]),
                ] {
                    sh.positions.push(at(m, a, along, side, across));
                    sh.uvs.push(uv);
                    sh.normals.push(cross(a, side));
                    sh.tangents.push(a);
                    sh.bitangents.push(side);
                }
                sh.triangles.push([base, base + 1, base + 2]);
                sh.triangles.push([base, base + 2, base + 3]);
            }
            // Front burst facing down the barrel.
            let base = sh.positions.len() as u16;
            for (x, y, uv) in [
                (-burst, -burst, [u0, v0 + 0.5]),
                (burst, -burst, [u0 + 0.5, v0 + 0.5]),
                (burst, burst, [u0 + 0.5, v0]),
                (-burst, burst, [u0, v0]),
            ] {
                sh.positions.push(at(at(m, a, 1.0, u, x), v, y, a, 0.0));
                sh.uvs.push(uv);
                sh.normals.push(a);
                sh.tangents.push(u);
                sh.bitangents.push(v);
            }
            sh.triangles.push([base, base + 1, base + 2]);
            sh.triangles.push([base, base + 2, base + 3]);
            sh
        })
        .collect()
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
                // Fox and Skyrim front faces wind in opposite directions
                // (checked against vanilla meshes: their triangles face
                // along the vertex normals, Fox's face against them).
                .map(|t| [t[0], t[2], t[1]])
                .collect(),
            textures: material_textures
                .get(mesh.material_instance as usize)
                .cloned()
                .unwrap_or_default(),
            glossiness: 80.0,
            specular_strength: 1.0,
            effect: None,
        });
    }
    // A second copy for the first-person skeleton: Skyrim caches models by
    // path, so another instance needs another file.
    let nif = nif_write::write_static_nif(&stem, &shapes);
    for suffix in ["", "_1p"] {
        let path = mesh_dir.join(format!("{stem}{suffix}.nif"));
        std::fs::write(&path, &nif)?;
        report.files.push(path);
    }
    Ok(())
}
