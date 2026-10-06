//! Renders Snake's foley ([`snake_core::foley::Foley`]) from the user's
//! MGSV sound bank, the way MGSV's `SoundTppPlayer.lua` sets it up:
//!
//! * footsteps / hands: `FOOT_GROUND_L` .. `ARM_LEAVE_R` with
//!   `player_type_switch` = `nom` (Snake's fatigues) and `foot_step_switch`
//!   = surface symbol (`SOILA`, `ROCKA`, `WOODA`, `IRONA`, `GRASA`, `GRAVA`,
//!   `SANDA`, ...) + `W` when wet + `_` + action (`Wk`, `WkC`, `Rn`, `RnC`,
//!   `Dh`, `cr`, `cp`);
//! * suit and gear rattle: `sfx_p_rtl_suit_nom_wk` with `rattle_suit_switch`
//!   = action;
//! * weapon rattle: `RATTLE_WEAPON` with `rattle_weapon_switch` = weapon
//!   symbol (`ar00` assault rifles, `sr00` sniper rifles, `mg01`, `ms00`,
//!   `sd00`; handguns and SMGs have none) + `_` + action;
//! * the chest landing (dive): `Ref_P_Chest_Ground`, rattle `cp`.
//!
//! Containers are resolved with [`fox_data::wwise::Hirc::render_layers`]
//! (random containers pick a different recording each time).

use anyhow::{Context, Result};
use fox_archive::Vfs;
use fox_data::sbp::{Pcm, SoundBank};
use fox_data::wwise::{short_id, Hirc, Switches};
use snake_core::foley::{Foley, FoleyKind};

use crate::read_any;

/// MGSV's footstep surfaces (`SoundTppPlayer.lua` symbols).
pub const SURFACES: [&str; 13] =
    ["SOILA", "ROCKAW", "ROCKA", "CLOTA", "GRASA", "GRAVA", "WOODA", "IRONA", "FENCA", "TIN_A", "IRONB", "GLASB", "SANDA"];

pub struct FoleyBank {
    hircs: Vec<Hirc<'static>>,
    bank: SoundBank<'static>,
    seed: u64,
}

impl FoleyBank {
    /// The common bank (player foley lives there), kept for the session.
    pub fn load(vfs: &Vfs) -> Result<Self> {
        let data: &'static [u8] = Box::leak(read_any(vfs, crate::COMMON_BANK)?.into_boxed_slice());
        let hircs = Hirc::parse_all(data);
        anyhow::ensure!(!hircs.is_empty(), "no sound objects in {}", crate::COMMON_BANK);
        let bank = SoundBank::parse(data).context("common bank")?;
        Ok(Self { hircs, bank, seed: 0x2545_F491_4F6C_DD1D })
    }

    /// The Wwise event and switches MGSV posts for `f` on `surface`.
    fn event(f: &Foley, surface: &str, wet: bool, weapon: Option<&str>) -> Option<(&'static str, Vec<(String, String)>)> {
        let act = f.action.name();
        let side = |l: &'static str, r: &'static str| if f.left { l } else { r };
        let footstep = |event: &'static str| {
            let value = format!("{surface}{}_{act}", if wet { "W" } else { "" });
            Some((event, vec![("player_type_switch".into(), "nom".into()), ("foot_step_switch".into(), value)]))
        };
        match f.kind {
            FoleyKind::FootGround => footstep(side("FOOT_GROUND_L", "FOOT_GROUND_R")),
            FoleyKind::FootLeave => footstep(side("FOOT_LEAVE_L", "FOOT_LEAVE_R")),
            FoleyKind::HandGround => footstep(side("ARM_GROUND_L", "ARM_GROUND_R")),
            FoleyKind::HandLeave => footstep(side("ARM_LEAVE_L", "ARM_LEAVE_R")),
            FoleyKind::RattleSuit => Some((
                "sfx_p_rtl_suit_nom_wk",
                vec![("player_type_switch".into(), "nom".into()), ("rattle_suit_switch".into(), act.to_ascii_lowercase())],
            )),
            FoleyKind::RattleWeapon => {
                let symbol = weapon?;
                Some(("RATTLE_WEAPON", vec![("rattle_weapon_switch".into(), format!("{symbol}_{}", act.to_ascii_lowercase()))]))
            }
            FoleyKind::ChestGround => Some((
                "Ref_P_Chest_Ground",
                vec![("player_type_switch".into(), "nom".into()), ("rattle_suit_switch".into(), "cp".into())],
            )),
        }
    }

    /// `f` as 48 kHz stereo, or None if MGSV has no sound for it (e.g. a
    /// handgun's weapon rattle).
    pub fn render(&mut self, f: &Foley, surface: &str, wet: bool, weapon: Option<&str>) -> Option<Pcm> {
        let (event, switches) = Self::event(f, surface, wet, weapon)?;
        let id = short_id(event);
        let hirc = self.hircs.iter().find(|h| h.has_event(id))?;
        let switches: Switches = switches.iter().map(|(g, v)| (short_id(g), short_id(v))).collect();
        let seed = &mut self.seed;
        let mut pick = |n: usize| {
            *seed ^= *seed << 13;
            *seed ^= *seed >> 7;
            *seed ^= *seed << 17;
            (*seed % n.max(1) as u64) as usize
        };
        let layers = hirc.render_layers(id, &switches, &mut pick)?;
        if layers.is_empty() {
            return None;
        }
        fox_data::wwise::mix(&self.bank, &layers).ok()
    }
}

/// MGSV's weapon rattle symbol for a weapon's sound set (`ar00` for the
/// MRS-4's), None for weapons that do not rattle (handguns, SMGs).
pub fn weapon_symbol(sound_set: &str) -> Option<&'static str> {
    match sound_set.get(..2)? {
        "ar" => Some("ar00"),
        "sr" => Some("sr00"),
        "mg" => Some("mg01"),
        "ms" => Some("ms00"),
        "sd" => Some("sd00"),
        _ => None,
    }
}
