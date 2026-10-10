//! Loads everything Snake needs from a local MGSV: TPP install: archives,
//! skeleton and rig, motion clips, model parts and weapons. Shared by the
//! `mgsv` tools and the C API (`snake-ffi`).

use std::path::Path;

pub mod foley;
pub mod skyrim;
pub mod skyrim_body;
pub mod skyrim_export;

use anyhow::{Context, Result};
use fox_archive::{Fpk, Vfs};

/// Open the game's archives and load path dictionaries from `dictionaries`
/// (the repo's `dictionaries/` folder) so clips resolve by name.
pub fn open_game(game_dir: &Path, dictionaries: &Path) -> Result<Vfs> {
    let mut vfs = Vfs::open(game_dir).with_context(|| format!("opening {}", game_dir.display()))?;
    for name in [
        "qar_dictionary.txt",
        "harvested_paths.txt",
        "mtar_tools/mtar_dictionary.txt",
    ] {
        let p = dictionaries.join(name);
        if p.is_file() {
            vfs.dictionary.load_file(&p)?;
        }
    }
    Ok(vfs)
}

/// A weapon ready to use: stats, rig attachment, model.
pub struct LoadedWeapon {
    /// Equipment id, e.g. `WP_West_ar_010`.
    pub id: &'static str,
    /// Display name, e.g. `AM MRS-4`.
    pub name: &'static str,
    pub model: fox_anim::SkinnedModel,
    pub rest: fox_anim::Pose,
    pub stats: snake_core::WeaponStats,
    /// Attachment, clips and holster (`rig.holster`).
    pub rig: snake_core::WeaponRig,
    /// Meshes of Snake's body model that show the gun in its closed holster
    /// (the pistol grip at the back of the thigh holster): drawn only while
    /// the gun is in there.
    pub holster_meshes: Vec<usize>,
    pub color: fox_anim::Vec3,
}

/// Everything needed to run and draw Snake.
pub struct SnakeAssets {
    pub skel: fox_anim::Skeleton,
    pub rig: fox_anim::HumanRig,
    pub clips: snake_core::ClipLibrary,
    /// Body, face, bionic arm with a base colour each.
    pub parts: Vec<(fox_anim::SkinnedModel, fox_anim::Vec3)>,
    pub weapons: Vec<LoadedWeapon>,
}

impl SnakeAssets {
    pub fn load(vfs: &Vfs) -> Result<Self> {
        let (skel, rig) = snake_rig(vfs)?;
        let clips = player_clip_library(vfs)?;
        let mut parts = Vec::new();
        for (path, color) in [
            (SNAKE_BODY, fox_anim::Vec3::new(0.47, 0.45, 0.34)),
            (SNAKE_FACE, fox_anim::Vec3::new(0.80, 0.62, 0.52)),
            (SNAKE_ARM, fox_anim::Vec3::new(0.38, 0.38, 0.40)),
        ] {
            let model = fox_data::Fmdl::parse(&read_any(vfs, path)?)?;
            parts.push((
                fox_anim::SkinnedModel::bind(&model, &skel).map_err(anyhow::Error::msg)?,
                color,
            ));
        }
        let equip_src = String::from_utf8_lossy(&read_any(vfs, EQUIP_PARAMETERS)?).into_owned();
        let damage_src = String::from_utf8_lossy(&read_any(vfs, DAMAGE_PARAMETERS)?).into_owned();
        let weapons = WEAPONS
            .iter()
            .map(|def| load_weapon(vfs, def, &equip_src, &damage_src, &skel, &rig, &clips))
            .collect::<Result<_>>()?;
        Ok(Self {
            skel,
            rig,
            clips,
            parts,
            weapons,
        })
    }

    /// A controller with every loaded weapon equipped.
    pub fn controller(&self) -> Result<snake_core::Controller> {
        let mut ctl = snake_core::Controller::new(&self.clips, snake_core::ClipNames::default())
            .map_err(anyhow::Error::msg)?;
        for w in &self.weapons {
            ctl.equip(&self.clips, w.stats.clone(), w.rig.clone())
                .map_err(anyhow::Error::msg)?;
        }
        Ok(ctl)
    }
}

/// Read a file from the archives, or from inside a package with `PKG:INNER`.
pub fn read_any(vfs: &Vfs, path: &str) -> Result<Vec<u8>> {
    if let Some((pkg, inner)) = path
        .split_once(".fpkd:")
        .map(|(a, b)| (format!("{a}.fpkd"), b))
        .or_else(|| {
            path.split_once(".fpk:")
                .map(|(a, b)| (format!("{a}.fpk"), b))
        })
    {
        let data = vfs.read(&pkg)?;
        let p = Fpk::parse(&data)?;
        let e = p
            .entries
            .iter()
            .find(|e| e.clean_path() == inner)
            .with_context(|| format!("{inner} not in {pkg}"))?;
        return Ok(p.read(e)?);
    }
    Ok(vfs.read(path)?)
}

pub const HUMAN_RIG: &str =
    "/Assets/tpp/pack/mission2/common/mis_com_chico_gz.fpk:/Assets/tpp/rig/frig/human_finger.frig";

pub const SNAKE_ARM: &str = "/Assets/tpp/pack/player/fova/plfova_sna0_arm0_v00.fpk:/Assets/tpp/chara/sna/Scenes/sna0_arm0_cov.fmdl";

pub const SNAKE_FACE: &str = "/Assets/tpp/pack/player/fova/plfova_sna0_face0_v00.fpk:/Assets/tpp/chara/sna/Scenes/sna0_face0_cov.fmdl";

pub const SNAKE_BODY: &str = "/Assets/tpp/pack/player/parts/plparts_normal.fpk:/Assets/tpp/chara/sna/Scenes/sna0_main0_def.fmdl";

pub fn snake_rig(vfs: &Vfs) -> Result<(fox_anim::Skeleton, fox_anim::HumanRig)> {
    let rig = fox_data::frig::Frig::parse(&read_any(vfs, HUMAN_RIG)?)?;
    let skel = fox_anim::Skeleton::from_fmdl(&fox_data::Fmdl::parse(&read_any(vfs, SNAKE_BODY)?)?);
    let human = fox_anim::HumanRig::new(&rig, &skel).map_err(anyhow::Error::msg)?;
    Ok((skel, human))
}

pub const PLAYER_RESIDENT: &str = "/Assets/tpp/pack/player/motion/player2_resident_motion.fpk:/Assets/tpp/motion/mtar/player2/player2_resident.mtar";

/// Every clip of the player's resident motion archive, by short name.
/// A weapon Snake can carry in `mgsv play`: equip id, assembled model,
/// receiver motion archive and clip names.
pub struct WeaponDef {
    id: &'static str,
    /// Display name, as MGSV's HUD shows it.
    name: &'static str,
    model: &'static str,
    /// The magazine's own model (MGSV keeps it separate; it sits on the
    /// gun's `CNP_AMMO` and comes out during reloads).
    magazine: Option<&'static str>,
    cnp: &'static str,
    motion: &'static str,
    clips: snake_core::WeaponClips<'static>,
    /// Where it rides when not in the hands.
    holster: Option<Holster>,
    /// The point of the gun (its frame, metres) held at the hand's
    /// connection point. The pistol's origin is on its bore line, and held
    /// there the right hand closed round the slide; its grip is lower.
    grip: [f32; 3],
    color: [f32; 3],
}

/// A weapon's place on Snake's body when not in the hands.
pub enum Holster {
    /// A bone and the body's connection point on it.
    Cnp(&'static str, &'static str),
    /// A closed holster on Snake's body (the body mesh skinned to this
    /// bone): the gun sits inside it, hidden, upright as MGSV's draw clip
    /// takes it out (Snake's thigh holster has a helper bone but no
    /// connection point).
    Enclosed(&'static str),
}

pub const WEAPONS: [WeaponDef; 2] = [
    // AM MRS-4 assault rifle.
    WeaponDef {
        id: "WP_West_ar_010",
        name: "AM MRS-4",
        model: "/Assets/tpp/pack/collectible/primary/EQP_WP_West_ar_010.fpk:/Assets/tpp/weapon/asw/Scenes/ar00_main0_aw6.fmdl",
        magazine: Some("/Assets/tpp/pack/collectible/common/col_common_tpp.fpk:/Assets/tpp/weapon/amo/Scenes/am00_main0_def.fmdl"),
        cnp: "/Assets/tpp/pack/collectible/primary/EQP_WP_West_ar_010.fpk:/Assets/tpp/weapon/asw/Scenes/ar00_main0_aw6.fcnp",
        motion: "/Assets/tpp/pack/player/motion/equip/receiver/pl_rcvr_ar00.fpk:/Assets/tpp/motion/mtar/player2/Receiver/pl_rcvr_ar00.mtar",
        clips: snake_core::WeaponClips {
            grids: [
                Some("snapasr_s_fre_pose_25"),
                Some("snapasr_q_fre_pose_25"),
                Some("snapasr_c_fre_pose_35"),
            ],
            // A quick magazine change with rounds left; the full reload
            // (with the charging handle) when empty.
            reload: snake_core::ReloadNames {
                partial: [Some("snapar00_0_s_mag_chg"), None],
                empty: [Some("snapar00_aw0_s_fre_rld"), Some("snapar00_aw0_c_fre_rld_std")],
            },
            fire: Some("snapar00_aw1_s_fre"),
            // Off the sling up to the aim, and back.
            draw: Some("snapasr_s_fre0_st"),
            holster: Some("snapasr_s_fre0_ed"),
        },
        holster: Some(Holster::Cnp("SKL_300_ASRROOT", "CNP_ASRROOT")),
        grip: [0.0, 0.0, 0.0],
        color: [0.22, 0.22, 0.22],
    },
    // WU S.Pistol (tranquilizer).
    WeaponDef {
        id: "WP_West_thg_010",
        name: "WU S.PISTOL",
        model: "/Assets/tpp/pack/collectible/secondary/EQP_WP_West_thg_010.fpk:/Assets/tpp/weapon/asw/Scenes/hg01_main3_aw0.fmdl",
        magazine: Some("/Assets/tpp/pack/collectible/secondary/EQP_WP_West_thg_010.fpk:/Assets/tpp/weapon/hag/Scenes/hg01_ammo0_def.fmdl"),
        cnp: "/Assets/tpp/pack/collectible/secondary/EQP_WP_West_thg_010.fpk:/Assets/tpp/weapon/asw/Scenes/hg01_main3_aw0.fcnp",
        motion: "/Assets/tpp/pack/player/motion/equip/receiver/pl_rcvr_hg01.fpk:/Assets/tpp/motion/mtar/player2/Receiver/pl_rcvr_hg01.mtar",
        clips: snake_core::WeaponClips {
            grids: [
                Some("snaphg01_s_fre_pose_25"),
                Some("snaphg01_q_fre_pose_25"),
                Some("snaphg01_c_fre_pose_35"),
            ],
            // Slide locked back when empty.
            reload: snake_core::ReloadNames {
                partial: [Some("snaphg01_s_fre_rld"), Some("snaphg01_c_fre_rld_std")],
                empty: [Some("snaphg01_s_fre_emp_rld"), Some("snaphg01_c_fre_emp_rld")],
            },
            fire: Some("snaphg01_s_fre1"),
            // Out of the thigh holster up to the aim, and back.
            draw: Some("snaphg01_s_fre0_st"),
            holster: Some("snaphg01_s_fre0_ed"),
        },
        holster: Some(Holster::Enclosed("SKL_534_RHOL_HLP")),
        // The web of the hand under the slide's rear, the fist round the
        // grip (checked with `mgsv hand-view`).
        grip: [0.0, -0.03, -0.02],
        color: [0.28, 0.27, 0.25],
    },
];

/// Damage of every attack (`TppDamage.ReloadDamageParameter`).
pub const DAMAGE_PARAMETERS: &str = "/Assets/tpp/level_asset/damage/ParameterTables/DamageParameterTables.lua";

/// The always-loaded sound bank with TPP's common sound effects, including
/// every player weapon's shot events.
pub const COMMON_BANK: &str = "/Assets/tpp/sound/asset/common_bank_01.sbp";

pub const EQUIP_PARAMETERS: &str =
    "/Assets/tpp/level_asset/weapon/ParameterTables/parts/EquipParameters.lua";

/// Player clips by short name: the resident set plus the rifle receiver's.
pub fn player_clip_library(vfs: &Vfs) -> Result<snake_core::ClipLibrary> {
    let mut lib = snake_core::ClipLibrary::new();
    let paths = std::iter::once(PLAYER_RESIDENT).chain(WEAPONS.iter().map(|w| w.motion));
    for path in paths {
        let data = read_any(vfs, path)?;
        let m = fox_data::mtar::Mtar::parse(&data)?;
        for i in 0..m.entries.len() {
            let Ok(c) = m.clip(&data, i) else { continue };
            let name = vfs.dictionary.resolve(m.entries[i].hash).0;
            let short = name
                .rsplit('/')
                .next()
                .unwrap_or(&name)
                .trim_end_matches(".gani");
            lib.insert(short, c);
        }
    }
    Ok(lib)
}

/// One weapon: stats from the game's tables, rig attachment calibrated on its
/// aim grid, model, and holster point.
pub fn load_weapon(
    vfs: &Vfs,
    def: &WeaponDef,
    equip_src: &str,
    damage_src: &str,
    skel: &fox_anim::Skeleton,
    rig: &fox_anim::HumanRig,
    clips: &snake_core::ClipLibrary,
) -> Result<LoadedWeapon> {
    let mut stats = snake_core::WeaponStats::from_equip_parameters(equip_src, def.id)
        .map_err(anyhow::Error::msg)?;
    stats.apply_damage_parameters(damage_src).map_err(anyhow::Error::msg)?;
    let body = fox_data::fcnp::Fcnp::parse(&read_any(vfs, SNAKE_CNP)?)?;
    let gun_cnp = fox_data::fcnp::Fcnp::parse(&read_any(vfs, def.cnp)?)?;
    let bone = |n: &str| {
        skel.find(fox_hash::str_code32(n.as_bytes()))
            .with_context(|| format!("no bone {n}"))
    };
    let (_, muzzle) = cnp(&gun_cnp, "CNP_MUZZLE_FLASH")?;
    let mut weapon_rig = snake_core::WeaponRig::calibrate(
        rig,
        skel,
        clips,
        def.clips.clone(),
        bone("SKL_023_RHAND")?,
        cnp(&body, "CNP_RIGHT_HAND")?,
        muzzle,
    )
    .map_err(anyhow::Error::msg)?;
    weapon_rig.grip = fox_anim::Vec3::from(def.grip);
    if def.magazine.is_some() {
        if let Ok(ammo) = cnp(&gun_cnp, "CNP_AMMO") {
            weapon_rig.magazine = Some(snake_core::MagazineRig {
                cnp: ammo,
                left_hand_bone: bone("SKL_013_LHAND")?,
            });
        }
    }
    let model = fox_data::Fmdl::parse(&read_any(vfs, def.model)?)?;
    let gun_skel = fox_anim::Skeleton::from_fmdl(&model);
    let skinned = fox_anim::SkinnedModel::bind(&model, &gun_skel).map_err(anyhow::Error::msg)?;
    let mut holster_meshes = Vec::new();
    weapon_rig.holster = match def.holster {
        Some(Holster::Cnp(b, c)) => Some((bone(b)?, cnp(&body, c)?)),
        Some(Holster::Enclosed(b)) => {
            let b = bone(b)?;
            let on_bone = weapon_rig.holster_from_draw(rig, skel, clips, b).map_err(anyhow::Error::msg)?;
            let gun = skinned.skin(&fox_anim::Pose::rest(&gun_skel)).concat();
            let body = fox_data::Fmdl::parse(&read_any(vfs, SNAKE_BODY)?)?;
            weapon_rig.holster_hides = true;
            let (place, meshes) = inside_holster(&body, skel, b, on_bone, &gun).with_context(|| format!("{}: holster", def.id))?;
            holster_meshes = meshes;
            Some((b, place))
        }
        None => None,
    };
    weapon_rig.fit_hand_over(rig, skel, clips).map_err(anyhow::Error::msg)?;
    Ok(LoadedWeapon {
        id: def.id,
        name: def.name,
        model: skinned,
        rest: fox_anim::Pose::rest(&gun_skel),
        stats,
        rig: weapon_rig,
        holster_meshes,
        color: fox_anim::Vec3::from_array(def.color),
    })
}

/// The gun's place on holster bone `bone` moved into the holster: the body
/// mesh with the most vertices skinned to `bone`. In the rest pose the top
/// 12 cm of the gun (`gun`: vertices in its own frame), turned as `on_bone`
/// turns it, is centred on the top 12 cm of the holster, 2 cm below its
/// top edge. Also returns the body meshes in mesh groups under the
/// holster's own group: the holstered gun MGSV models into the holster.
fn inside_holster(
    body: &fox_data::Fmdl,
    skel: &fox_anim::Skeleton,
    bone: usize,
    on_bone: (fox_anim::Quat, fox_anim::Vec3),
    gun: &[fox_anim::Vec3],
) -> Result<((fox_anim::Quat, fox_anim::Vec3), Vec<usize>)> {
    use fox_anim::Vec3;
    let skinned_to_bone = |m: &fox_data::fmdl::Mesh| -> Vec<Vec3> {
        let group = body.bone_groups.get(m.bone_group as usize);
        m.positions
            .iter()
            .zip(m.bone_weights.iter().zip(&m.bone_indices))
            .filter(|(_, (w, ix))| {
                (0..4).any(|k| w[k] > 0 && group.and_then(|g| g.get(ix[k] as usize)) == Some(&(bone as u16)))
            })
            .map(|(p, _)| Vec3::from_array(*p))
            .collect()
    };
    let (holster_mesh, holster) = body
        .meshes
        .iter()
        .map(|m| skinned_to_bone(m))
        .enumerate()
        .max_by_key(|(_, v)| v.len())
        .filter(|(_, v)| !v.is_empty())
        .context("no mesh skinned to the holster bone")?;
    let group = body.mesh_group_of(holster_mesh);
    let under_holster = |mesh: usize| {
        let mut cur = body.mesh_group_of(mesh).filter(|g| Some(*g) != group);
        for _ in 0..16 {
            let Some(g) = cur else { return false };
            if Some(g) == group {
                return true;
            }
            cur = body.mesh_groups.get(g as usize).and_then(|mg| u16::try_from(mg.parent).ok());
        }
        false
    };
    let contents = (0..body.meshes.len()).filter(|&m| under_holster(m)).collect();
    // Top edge and the centre of the top 12 cm (x, z) of a vertex set.
    let top = |vs: &[Vec3]| {
        let y = vs.iter().fold(f32::MIN, |a, v| a.max(v.y));
        let upper: Vec<&Vec3> = vs.iter().filter(|v| v.y > y - 0.12).collect();
        let c = upper.iter().fold(Vec3::ZERO, |a, v| a + **v) / upper.len() as f32;
        Vec3::new(c.x, y, c.z)
    };
    // Rest pose: bones unrotated at their rest positions.
    let bone_rest = skel.bones[bone].rest_model;
    let (r, t) = on_bone;
    let placed: Vec<Vec3> = gun.iter().map(|v| bone_rest + t + r * *v).collect();
    let (h, g) = (top(&holster), top(&placed));
    let shift = Vec3::new(h.x - g.x, h.y - 0.02 - g.y, h.z - g.z);
    Ok(((r, t + shift), contents))
}

pub const SNAKE_CNP: &str = "/Assets/tpp/pack/player/parts/plparts_normal.fpk:/Assets/tpp/chara/sna/Scenes/sna0_main0_def.fcnp";

pub const RIFLE_CNP: &str = "/Assets/tpp/pack/collectible/primary/EQP_WP_West_ar_010.fpk:/Assets/tpp/weapon/asw/Scenes/ar00_main0_aw6.fcnp";

pub fn cnp(f: &fox_data::fcnp::Fcnp, name: &str) -> Result<(fox_anim::Quat, fox_anim::Vec3)> {
    let c = f
        .points
        .iter()
        .find(|c| c.name == name)
        .with_context(|| format!("no {name} in {}", f.model))?;
    Ok((
        fox_anim::Quat::from_array(c.rotation),
        fox_anim::Vec3::from_array(c.position),
    ))
}
