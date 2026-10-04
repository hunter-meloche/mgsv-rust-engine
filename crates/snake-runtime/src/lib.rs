//! Loads everything Snake needs from a local MGSV: TPP install: archives,
//! skeleton and rig, motion clips, model parts and weapons. Shared by the
//! `mgsv` tools and the C API (`snake-ffi`).

use std::path::Path;

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
    pub model: fox_anim::SkinnedModel,
    pub rest: fox_anim::Pose,
    pub stats: snake_core::WeaponStats,
    pub rig: snake_core::WeaponRig,
    /// Where it rides when not in the hands (bone and offset); hidden if none.
    pub back: Option<(usize, (fox_anim::Quat, fox_anim::Vec3))>,
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
        let weapons = WEAPONS
            .iter()
            .map(|def| load_weapon(vfs, def, &equip_src, &skel, &rig, &clips))
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
    model: &'static str,
    cnp: &'static str,
    motion: &'static str,
    clips: snake_core::WeaponClips<'static>,
    /// Body connection point (and bone) it rides on when not in the hands.
    holster: Option<(&'static str, &'static str)>,
    color: [f32; 3],
}

pub const WEAPONS: [WeaponDef; 2] = [
    // AM MRS-4 assault rifle.
    WeaponDef {
        id: "WP_West_ar_010",
        model: "/Assets/tpp/pack/collectible/primary/EQP_WP_West_ar_010.fpk:/Assets/tpp/weapon/asw/Scenes/ar00_main0_aw6.fmdl",
        cnp: "/Assets/tpp/pack/collectible/primary/EQP_WP_West_ar_010.fpk:/Assets/tpp/weapon/asw/Scenes/ar00_main0_aw6.fcnp",
        motion: "/Assets/tpp/pack/player/motion/equip/receiver/pl_rcvr_ar00.fpk:/Assets/tpp/motion/mtar/player2/Receiver/pl_rcvr_ar00.mtar",
        clips: snake_core::WeaponClips {
            grids: [
                Some("snapasr_s_fre_pose_25"),
                Some("snapasr_q_fre_pose_25"),
                Some("snapasr_c_fre_pose_35"),
            ],
            reload: "snapar00_0_s_mag_chg",
            fire: Some("snapar00_aw1_s_fre"),
        },
        holster: Some(("SKL_300_ASRROOT", "CNP_ASRROOT")),
        color: [0.22, 0.22, 0.22],
    },
    // WU S.Pistol (tranquilizer).
    WeaponDef {
        id: "WP_West_thg_010",
        model: "/Assets/tpp/pack/collectible/secondary/EQP_WP_West_thg_010.fpk:/Assets/tpp/weapon/asw/Scenes/hg01_main3_aw0.fmdl",
        cnp: "/Assets/tpp/pack/collectible/secondary/EQP_WP_West_thg_010.fpk:/Assets/tpp/weapon/asw/Scenes/hg01_main3_aw0.fcnp",
        motion: "/Assets/tpp/pack/player/motion/equip/receiver/pl_rcvr_hg01.fpk:/Assets/tpp/motion/mtar/player2/Receiver/pl_rcvr_hg01.mtar",
        clips: snake_core::WeaponClips {
            grids: [
                Some("snaphg01_s_fre_pose_25"),
                Some("snaphg01_q_fre_pose_25"),
                Some("snaphg01_c_fre_pose_35"),
            ],
            reload: "snaphg01_s_fre_rld",
            fire: Some("snaphg01_s_fre1"),
        },
        holster: None,
        color: [0.28, 0.27, 0.25],
    },
];

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
    skel: &fox_anim::Skeleton,
    rig: &fox_anim::HumanRig,
    clips: &snake_core::ClipLibrary,
) -> Result<LoadedWeapon> {
    let stats = snake_core::WeaponStats::from_equip_parameters(equip_src, def.id)
        .map_err(anyhow::Error::msg)?;
    let body = fox_data::fcnp::Fcnp::parse(&read_any(vfs, SNAKE_CNP)?)?;
    let gun_cnp = fox_data::fcnp::Fcnp::parse(&read_any(vfs, def.cnp)?)?;
    let bone = |n: &str| {
        skel.find(fox_hash::str_code32(n.as_bytes()))
            .with_context(|| format!("no bone {n}"))
    };
    let (_, muzzle) = cnp(&gun_cnp, "CNP_MUZZLE_FLASH")?;
    let weapon_rig = snake_core::WeaponRig::calibrate(
        rig,
        skel,
        clips,
        def.clips.clone(),
        bone("SKL_023_RHAND")?,
        cnp(&body, "CNP_RIGHT_HAND")?,
        muzzle,
    )
    .map_err(anyhow::Error::msg)?;
    let model = fox_data::Fmdl::parse(&read_any(vfs, def.model)?)?;
    let gun_skel = fox_anim::Skeleton::from_fmdl(&model);
    let skinned = fox_anim::SkinnedModel::bind(&model, &gun_skel).map_err(anyhow::Error::msg)?;
    let holster = match def.holster {
        Some((b, c)) => Some((bone(b)?, cnp(&body, c)?)),
        None => None,
    };
    Ok(LoadedWeapon {
        model: skinned,
        rest: fox_anim::Pose::rest(&gun_skel),
        stats,
        rig: weapon_rig,
        back: holster,
        color: fox_anim::Vec3::from_array(def.color),
    })
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
