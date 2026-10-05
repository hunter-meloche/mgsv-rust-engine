//! Export Snake's own body (body, face and bionic arm from the user's MGSV
//! install) as a Skyrim SE skinned mesh, `Data\meshes\snake\snake_body.nif`,
//! worn on the player's skeleton while Snake mode is on.
//!
//! Skinning: every Snake bone maps to a Skyrim bone through the retarget
//! table (a Snake bone that drives no Skyrim bone uses its nearest ancestor
//! that does), restricted to the core bones present in the in-game
//! third-person skeleton (fingers, toes' children and twists are not there,
//! so their weights fold into their parents). The bind pose is the Skyrim
//! skeleton as the retarget poses it for Snake's rest pose, in the space of
//! Snake's mesh converted to Skyrim axes and units; so when the plugin poses
//! the skeleton like Snake, the mesh follows.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use fox_archive::Vfs;
use fox_data::Fmdl;
use skyrim_data::nif_write::{self, Shape, SkinBone, SkinnedShape};

use crate::skyrim::SkyrimRetarget;
use crate::skyrim_export::{
    cross, diffuse_dds, has_cutout, material_presets, material_reflectance, material_texture, material_texture_code,
    normalize, pbr_normal_dds, to_sky, ExportReport,
};
use crate::{read_any, snake_rig, SNAKE_ARM, SNAKE_BODY, SNAKE_FACE};

const UNITS_PER_METER: f32 = 70.0;

/// Skyrim mesh path of Snake's body (relative to `Data\meshes`).
pub const BODY_MESH: &str = r"snake\snake_body.nif";
/// Snake's arms for the first-person skeleton.
pub const ARMS_MESH: &str = r"snake\snake_arms_1p.nif";

/// Which skeleton a body mesh is bound to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BodyView {
    /// The whole body on the third-person skeleton's core bones.
    ThirdPerson,
    /// The arms (clavicles down to the fingers) on the first-person
    /// skeleton, which has finger and twist bones; the rest is cut away.
    FirstPersonArms,
}

/// Bones the first-person view poses (the plugin's `IsArmBone`).
fn is_arm_bone(name: &str) -> bool {
    ["Clavicle", "UpperArm", "Upperarm", "Forearm", "Hand", "Finger"]
        .iter()
        .any(|p| name.contains(p))
}

/// Skyrim bones the body may be skinned to: the ones the in-game
/// third-person skeleton has (and the plugin poses). The toes are not in it
/// at runtime, so Snake's toes fold into the feet.
const CORE_BONES: [&str; 20] = [
    "NPC Pelvis [Pelv]",
    "NPC Spine [Spn0]",
    "NPC Spine1 [Spn1]",
    "NPC Spine2 [Spn2]",
    "NPC Neck [Neck]",
    "NPC Head [Head]",
    "NPC L Clavicle [LClv]",
    "NPC L UpperArm [LUar]",
    "NPC L Forearm [LLar]",
    "NPC L Hand [LHnd]",
    "NPC R Clavicle [RClv]",
    "NPC R UpperArm [RUar]",
    "NPC R Forearm [RLar]",
    "NPC R Hand [RHnd]",
    "NPC L Thigh [LThg]",
    "NPC L Calf [LClf]",
    "NPC L Foot [Lft ]",
    "NPC R Thigh [RThg]",
    "NPC R Calf [RClf]",
    "NPC R Foot [Rft ]",
];

/// A material's Skyrim texture paths (diffuse, normal), glossiness and
/// whether it cuts out with the diffuse alpha.
#[derive(Clone, Debug)]
pub struct BodyMaterial {
    pub textures: Vec<String>,
    pub glossiness: f32,
    pub cutout: bool,
}

impl Default for BodyMaterial {
    fn default() -> Self {
        Self { textures: Vec::new(), glossiness: 30.0, cutout: false }
    }
}

/// What was learnt about each texture file written (they are shared by
/// materials and parts): a normal map's glossiness, a diffuse's cutout.
#[derive(Default)]
struct TextureInfo {
    glossiness: HashMap<std::path::PathBuf, f32>,
    cutout: HashMap<std::path::PathBuf, bool>,
}

/// Snake's body bound to the Skyrim skeleton.
pub struct Body {
    pub retarget: SkyrimRetarget,
    /// Retarget bone index of each mesh bone.
    pub core: Vec<usize>,
    pub bones: Vec<SkinBone>,
    pub shapes: Vec<SkinnedShape>,
    /// (Snake part model, mesh index) of each shape.
    pub sources: Vec<(&'static str, usize)>,
}

/// Write Snake's body mesh and textures under `data_dir` (Skyrim's `Data`),
/// using the character skeleton of the Skyrim install at `skyrim_dir`.
pub fn export_snake_body(vfs: &Vfs, skyrim_dir: &Path, data_dir: &Path, report: &mut ExportReport) -> Result<()> {
    let tex_dir = data_dir.join("textures").join("snake");
    let mesh_dir = data_dir.join("meshes").join("snake");
    std::fs::create_dir_all(&tex_dir)?;
    std::fs::create_dir_all(&mesh_dir)?;
    let mut info = TextureInfo::default();
    for (view, file, root) in [
        (BodyView::ThirdPerson, "snake_body.nif", "snake_body"),
        (BodyView::FirstPersonArms, "snake_arms_1p.nif", "snake_arms_1p"),
    ] {
        let mut notes = Vec::new();
        let body = build_body(
            vfs,
            skyrim_dir,
            view,
            &mut |fmdl| material_textures(vfs, fmdl, "snake", &tex_dir, report, &mut info),
            &mut notes,
        )?;
        report.notes.extend(notes);
        let path = mesh_dir.join(file);
        std::fs::write(&path, nif_write::write_skinned_nif(root, &body.bones, &body.shapes))?;
        report.notes.push(format!(
            "{root}: {} shapes, {} vertices, {} bones",
            body.shapes.len(),
            body.shapes.iter().map(|s| s.shape.positions.len()).sum::<usize>(),
            body.bones.len()
        ));
        report.files.push(path);
    }
    // Body textures of earlier exports that are no longer used (they are
    // named after their contents' settings).
    for entry in std::fs::read_dir(&tex_dir)?.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("snake_") && name.ends_with(".dds") && !report.files.contains(&path) {
            std::fs::remove_file(&path)?;
            report.notes.push(format!("removed old {name}"));
        }
    }
    Ok(())
}

/// Bind Snake's body to the Skyrim skeleton of the install at `skyrim_dir`;
/// `textures(fmdl)` returns each material's texture paths.
///
/// Both views use the bind pose of the retargeted third-person skeleton:
/// the plugin writes the same local transforms to the first-person arm
/// bones, so each vertex keeps its place relative to its bones.
pub fn build_body(
    vfs: &Vfs,
    skyrim_dir: &Path,
    view: BodyView,
    textures: &mut dyn FnMut(&Fmdl) -> Result<Vec<BodyMaterial>>,
    notes: &mut Vec<String>,
) -> Result<Body> {
    let (skel, _) = snake_rig(vfs)?;
    let nif = skyrim_data::Nif::parse(&skyrim_data::read_install_file(skyrim_dir, skyrim_data::CHARACTER_SKELETON)?)?;
    let retarget = SkyrimRetarget::new(&nif, &skel)?;
    let links = retarget.bone_links();
    let model = retarget.model_transforms(&fox_anim::Pose::rest(&skel));

    // The mesh's bones: the core bones, in retarget (parent-first) order.
    let mut core: Vec<usize> = Vec::new();
    for (i, (name, _, _)) in links.iter().enumerate() {
        let wanted = match view {
            BodyView::ThirdPerson => CORE_BONES.contains(&name.as_str()),
            BodyView::FirstPersonArms => is_arm_bone(name),
        };
        if wanted && !core.contains(&i) {
            core.push(i);
        }
    }
    let nif_index: HashMap<usize, u8> = core.iter().enumerate().map(|(k, &i)| (i, k as u8)).collect();
    let pelvis = match view {
        BodyView::ThirdPerson => core
            .iter()
            .position(|&i| links[i].0 == "NPC Pelvis [Pelv]")
            .context("no pelvis in the retarget table")? as u8,
        BodyView::FirstPersonArms => 0,
    };
    // Snake bone -> first retarget bone it drives.
    let mut by_source: HashMap<usize, usize> = HashMap::new();
    for (i, (_, _, source)) in links.iter().enumerate() {
        if let Some(s) = source {
            by_source.entry(*s).or_insert(i);
        }
    }
    // Snake bone -> mesh bone (None: no mesh bone above it, i.e. outside
    // the arms in the first-person view).
    let fallback = match view {
        BodyView::ThirdPerson => Some(pelvis),
        BodyView::FirstPersonArms => None,
    };
    let to_mesh_bone = |snake_bone: usize| -> Option<u8> {
        let mut cur = Some(snake_bone);
        while let Some(s) = cur {
            if let Some(&o) = by_source.get(&s) {
                let mut out = Some(o);
                while let Some(b) = out {
                    if let Some(&k) = nif_index.get(&b) {
                        return Some(k);
                    }
                    out = links[b].1;
                }
                return fallback;
            }
            cur = skel.bones[s].parent;
        }
        fallback
    };
    // First person keeps the vertices weighted mostly below the clavicles.
    let limb: Vec<bool> = core.iter().map(|&i| !links[i].0.contains("Clavicle")).collect();

    let bones: Vec<SkinBone> = core
        .iter()
        .map(|&i| {
            let (name, q, t) = &model[i];
            let m = fox_anim::glam::Mat3::from_quat(*q);
            SkinBone {
                name: name.clone(),
                rotation: [m.row(0).to_array(), m.row(1).to_array(), m.row(2).to_array()],
                translation: t.to_array(),
            }
        })
        .collect();

    let mut shapes = Vec::new();
    let mut sources = Vec::new();
    for (path, part) in [(SNAKE_BODY, "body"), (SNAKE_FACE, "face"), (SNAKE_ARM, "arm")] {
        if view == BodyView::FirstPersonArms && part == "face" {
            continue;
        }
        let fmdl = Fmdl::parse(&read_any(vfs, path)?)?;
        let skinned = fox_anim::SkinnedModel::bind(&fmdl, &skel).map_err(anyhow::Error::msg)?;
        // Rest positions in the skeleton's space: the face and arm models are
        // authored around their own bone positions, so the raw vertices are
        // offset (15 and 62 cm); Snake's own skinning at rest places them.
        let rest = skinned.skin(&fox_anim::Pose::rest(&skel));
        let textures = textures(&fmdl)?;
        for (i, ((mesh, sm), rest_positions)) in fmdl.meshes.iter().zip(&skinned.meshes).zip(&rest).enumerate() {
            if fmdl.mesh_hidden(i) {
                continue;
            }
            if mesh.positions.len() > u16::MAX as usize {
                notes.push(format!("snake {part} mesh {i}: too many vertices; skipped"));
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
            // Up to four (mesh bone, weight), merged per bone, normalised;
            // `keep` marks the vertices this view shows.
            let mut keep = vec![true; sm.influences.len()];
            let weights: Vec<[(u8, f32); 4]> = sm
                .influences
                .iter()
                .enumerate()
                .map(|(v, inf)| {
                    let mut acc: Vec<(u8, f32)> = Vec::new();
                    let total: f32 = inf.iter().map(|x| x.weight).sum();
                    let mut on_limb = 0.0;
                    for x in inf {
                        let Some(b) = to_mesh_bone(x.bone as usize) else { continue };
                        if limb[b as usize] {
                            on_limb += x.weight;
                        }
                        match acc.iter_mut().find(|(k, _)| *k == b) {
                            Some(e) => e.1 += x.weight,
                            None => acc.push((b, x.weight)),
                        }
                    }
                    acc.sort_by(|a, b| b.1.total_cmp(&a.1));
                    acc.truncate(4);
                    let sum: f32 = acc.iter().map(|e| e.1).sum();
                    let mut out = [(0u8, 0.0f32); 4];
                    for (k, (b, w)) in acc.iter().enumerate() {
                        out[k] = (*b, if sum > 0.0 { w / sum } else { 0.0 });
                    }
                    if sum <= 0.0 {
                        out[0] = (pelvis, 1.0);
                    }
                    if view == BodyView::FirstPersonArms && (sum <= 0.0 || on_limb < 0.5 * total) {
                        keep[v] = false;
                    }
                    out
                })
                .collect();
            let positions: Vec<[f32; 3]> = rest_positions
                .iter()
                .map(|p| {
                    let s = to_sky(p.to_array());
                    [s[0] * UNITS_PER_METER, s[1] * UNITS_PER_METER, s[2] * UNITS_PER_METER]
                })
                .collect();
            // Fox front faces wind the other way from Skyrim's.
            let triangles: Vec<[u16; 3]> = mesh
                .indices
                .chunks_exact(3)
                .map(|t| [t[0], t[2], t[1]])
                .filter(|t| t.iter().all(|&k| keep[k as usize]))
                .collect();
            if triangles.is_empty() {
                continue;
            }
            // Only the vertices the kept triangles use.
            let mut used = vec![false; positions.len()];
            for t in &triangles {
                for &k in t {
                    used[k as usize] = true;
                }
            }
            let mut remap = vec![u16::MAX; positions.len()];
            let mut next = 0u16;
            for (k, u) in used.iter().enumerate() {
                if *u {
                    remap[k] = next;
                    next += 1;
                }
            }
            let material = textures.get(mesh.material_instance as usize).cloned().unwrap_or_default();
            fn pick<T: Copy>(v: &[T], used: &[bool]) -> Vec<T> {
                v.iter().zip(used).filter(|(_, u)| **u).map(|(x, _)| *x).collect()
            }
            shapes.push(SkinnedShape {
                shape: Shape {
                    name: format!("snake_{part}:{i}"),
                    positions: pick(&positions, &used),
                    normals: pick(&normals, &used),
                    tangents: pick(&tangents, &used),
                    bitangents: pick(&bitangents, &used),
                    uvs: pick(&mesh.uvs[0], &used),
                    triangles: triangles.iter().map(|t| t.map(|k| remap[k as usize])).collect(),
                    textures: material.textures,
                    glossiness: material.glossiness,
                    specular_strength: 1.0,
                    alpha_test: material.cutout.then_some(128),
                    // A cut-out decal reaching past its texture (the emblem
                    // patch, UVs -0.49..1.07) would repeat it: MGSV clamps.
                    clamp_uv: material.cutout
                        && mesh.uvs[0].iter().flatten().any(|&c| !(-0.01..=1.01).contains(&c)),
                    effect: None,
                },
                weights: pick(&weights, &used),
            });
            sources.push((path, i));
        }
    }
    Ok(Body {
        retarget,
        core,
        bones,
        shapes,
        sources,
    })
}

/// Base colour and normal map DDS for every material of `fmdl`; returns the
/// texture paths (diffuse, normal) and glossiness per material. Files are
/// named after the MGSV textures (path codes) and reflectance, so textures
/// shared by several materials or parts are written once (`info` remembers
/// what each one gives its materials).
fn material_textures(
    vfs: &Vfs,
    fmdl: &Fmdl,
    stem: &str,
    tex_dir: &Path,
    report: &mut ExportReport,
    info: &mut TextureInfo,
) -> Result<Vec<BodyMaterial>> {
    let presets = material_presets(vfs);
    if presets.is_empty() {
        report.notes.push("material presets not found; default reflectance used".into());
    }
    let mut out = Vec::new();
    for mi in 0..fmdl.material_instances.len() {
        let mut slots = vec![String::new(); 2];
        let mut material = BodyMaterial::default();
        if let Some(pc) = material_texture_code(fmdl, mi, "Base_Tex_SRGB") {
            let name = format!("{stem}_{:012x}_d.dds", pc & 0xFFFF_FFFF_FFFF);
            let path = tex_dir.join(&name);
            if !report.files.contains(&path) {
                match material_texture(vfs, fmdl, mi, "Base_Tex_SRGB") {
                    Some(Ok(base)) => {
                        std::fs::write(&path, diffuse_dds(&base)?)?;
                        report.files.push(path.clone());
                        info.cutout.insert(path.clone(), has_cutout(&base));
                    }
                    Some(Err(e)) => report.notes.push(format!("{stem} material {mi} base colour: {e}")),
                    None => {}
                }
            }
            if report.files.contains(&path) {
                slots[0] = format!("textures\\snake\\{name}");
                material.cutout = info.cutout.get(&path).copied().unwrap_or(false);
            }
        }
        // The normal map carries the specular mask; a material with only a
        // specular map gets a flat one.
        let normal_pc = material_texture_code(fmdl, mi, "NormalMap_Tex_NRM");
        let spec_pc = material_texture_code(fmdl, mi, "SpecularMap_Tex_LIN");
        if let Some(pc) = normal_pc.or(spec_pc) {
            let spec_pc = spec_pc.unwrap_or(0);
            let reflectance = material_reflectance(fmdl, mi, &presets);
            let name = format!(
                "{stem}_{}{:012x}_{:04x}_{:03}_n.dds",
                if normal_pc.is_some() { "" } else { "flat_" },
                pc & 0xFFFF_FFFF_FFFF,
                spec_pc & 0xFFFF,
                (reflectance * 1000.0).round() as u32
            );
            let path = tex_dir.join(&name);
            if !report.files.contains(&path) {
                let normal = material_texture(vfs, fmdl, mi, "NormalMap_Tex_NRM");
                let spec = material_texture(vfs, fmdl, mi, "SpecularMap_Tex_LIN");
                match (normal.transpose(), spec.transpose()) {
                    (Ok(normal), Ok(spec)) => {
                        let (dds, g) = pbr_normal_dds(normal.as_ref(), spec.as_ref(), reflectance)?;
                        std::fs::write(&path, dds)?;
                        report.files.push(path.clone());
                        info.glossiness.insert(path.clone(), g);
                        report.notes.push(format!("{name}: reflectance {reflectance:.3}, glossiness {g:.0}"));
                    }
                    (Err(e), _) | (_, Err(e)) => {
                        report.notes.push(format!("{stem} material {mi} normal/specular map: {e}"))
                    }
                }
            }
            if report.files.contains(&path) {
                slots[1] = format!("textures\\snake\\{name}");
                material.glossiness = info.glossiness.get(&path).copied().unwrap_or(material.glossiness);
            }
        }
        material.textures = slots;
        out.push(material);
    }
    Ok(out)
}
