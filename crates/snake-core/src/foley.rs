//! Snake's foley: footsteps, hands on the ground, suit and gear rattle,
//! weapon rattle and the chest landing on a dive, as MGSV triggers them.
//!
//! MGSV's sound setup (`/Assets/tpp/sound/scripts/motion/setup.lua` and
//! `chara/SoundTppPlayer.lua`) turns motion events into Wwise events:
//! foot / arm ground and leave (`FOOT_GROUND_L` .. `ARM_LEAVE_R`, switched
//! by the surface and the action), `RATTLE_SUIT`, `RATTLE_WEAPON` and the
//! chest landing (`Ref_P_Chest_Ground`). This module turns Snake's clips'
//! motion events into [`Foley`] requests; the host renders and plays them.
//!
//! Many of MGSV's locomotion clips (the loops) carry no motion events in
//! the archive (the game's motion graph supplies them), so for clips
//! without foot events the contacts are found in the pose instead: a foot
//! coming down to its lowest height marks a step (and the suit and weapon
//! rattle with it, as MGSV's clips pair them), a foot lifting off a leave.

use fox_anim::{Pose, Skeleton};

/// StrCode32 of a motion event name.
pub fn event(name: &str) -> u32 {
    fox_hash::str_code32(name.as_bytes())
}

/// What makes the sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum FoleyKind {
    FootGround = 0,
    FootLeave = 1,
    HandGround = 2,
    HandLeave = 3,
    RattleSuit = 4,
    RattleWeapon = 5,
    ChestGround = 6,
}

/// The movement: MGSV's action switch values (walk, crouched walk, run,
/// crouched run, dash, crawl, getting down prone).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum FoleyAction {
    Wk = 0,
    WkC = 1,
    Rn = 2,
    RnC = 3,
    Dh = 4,
    Cr = 5,
    Cp = 6,
}

impl FoleyAction {
    /// The switch value suffix (`SOILA_Rn`, `rattle_suit_switch` = `rn`;
    /// Wwise ids ignore case).
    pub fn name(self) -> &'static str {
        match self {
            Self::Wk => "Wk",
            Self::WkC => "WkC",
            Self::Rn => "Rn",
            Self::RnC => "RnC",
            Self::Dh => "Dh",
            Self::Cr => "cr",
            Self::Cp => "cp",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Foley {
    pub kind: FoleyKind,
    pub left: bool,
    pub action: FoleyAction,
}

/// Motion events (StrCode32) that make sounds, and what they make.
fn kind_of(name: u32) -> Option<(FoleyKind, bool)> {
    use FoleyKind::*;
    const TABLE: [(&str, FoleyKind, bool); 13] = [
        ("left_foot_ground", FootGround, true),
        ("right_foot_ground", FootGround, false),
        ("left_foot_leave", FootLeave, true),
        ("right_foot_leave", FootLeave, false),
        ("left_hand_ground", HandGround, true),
        ("right_hand_ground", HandGround, false),
        ("left_arm_ground", HandGround, true),
        ("right_arm_ground", HandGround, false),
        ("left_leg_ground", FootGround, true),
        ("right_leg_ground", FootGround, false),
        ("rattle_suit", RattleSuit, true),
        ("rattle_weapon", RattleWeapon, true),
        // The dive's landing slide: the chest hits the ground.
        ("slide", ChestGround, true),
    ];
    TABLE.iter().find(|(n, _, _)| event(n) == name).map(|&(_, k, l)| (k, l))
}

/// Foot contact detection for clips without foot events.
pub struct FoleyTracker {
    feet: [usize; 2],
    /// Recent lowest height of each foot (model space, m), slowly rising.
    low: [f32; 2],
    down: [bool; 2],
    ready: bool,
}

/// A foot within this of its lowest height is planted; lifted again above
/// `LIFT`.
const PLANT: f32 = 0.025;
const LIFT: f32 = 0.06;

impl FoleyTracker {
    pub fn new(skel: &Skeleton) -> Option<Self> {
        let find = |n: &str| skel.find(event(n));
        Some(Self {
            feet: [find("SKL_032_LFOOT")?, find("SKL_042_RFOOT")?],
            low: [0.0; 2],
            down: [true; 2],
            ready: false,
        })
    }

    /// This tick's foley from the motion events the clip passed (`crossed`)
    /// and, if the clip has no foot events of its own, the feet in `pose`.
    pub fn update(
        &mut self,
        crossed: &[u32],
        clip_has_feet: bool,
        moving: bool,
        action: FoleyAction,
        pose: &Pose,
        dt: f32,
        out: &mut Vec<Foley>,
    ) {
        for &name in crossed {
            if let Some((kind, left)) = kind_of(name) {
                out.push(Foley { kind, left, action });
            }
        }
        let heights = [pose.positions[self.feet[0]].y, pose.positions[self.feet[1]].y];
        if !self.ready {
            self.low = heights;
            self.ready = true;
        }
        for k in 0..2 {
            // The floor of each foot's path: follows it down at once, back
            // up slowly (stance changes move it).
            self.low[k] = if heights[k] < self.low[k] { heights[k] } else { self.low[k] + 0.05 * dt };
            if clip_has_feet || !moving {
                self.down[k] = heights[k] < self.low[k] + LIFT;
                continue;
            }
            let left = k == 0;
            if !self.down[k] && heights[k] < self.low[k] + PLANT {
                self.down[k] = true;
                out.push(Foley { kind: FoleyKind::FootGround, left, action });
                // MGSV's clips rattle the suit and the gun with each step.
                out.push(Foley { kind: FoleyKind::RattleSuit, left, action });
                out.push(Foley { kind: FoleyKind::RattleWeapon, left, action });
            } else if self.down[k] && heights[k] > self.low[k] + LIFT {
                self.down[k] = false;
                out.push(Foley { kind: FoleyKind::FootLeave, left, action });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fox_anim::glam::Vec3;
    use fox_anim::skeleton::Bone;

    /// A pose with the feet at `left` / `right` heights.
    fn pose_with_feet(skel: &Skeleton, left: f32, right: f32) -> Pose {
        let mut pose = Pose::rest(skel);
        pose.positions = vec![Vec3::ZERO; skel.bones.len()];
        pose.positions[0].y = left;
        pose.positions[1].y = right;
        pose
    }

    fn skeleton() -> Skeleton {
        let bone = |n: &str| Bone { name: event(n), parent: None, rest_local: Vec3::ZERO, rest_model: Vec3::ZERO };
        Skeleton { bones: vec![bone("SKL_032_LFOOT"), bone("SKL_042_RFOOT")] }
    }

    #[test]
    fn clip_events_make_foley() {
        let skel = skeleton();
        let mut t = FoleyTracker::new(&skel).unwrap();
        let mut out = Vec::new();
        let pose = pose_with_feet(&skel, 0.1, 0.1);
        t.update(&[event("left_foot_ground"), event("rattle_suit"), event("slide"), 12345], true, true, FoleyAction::Rn, &pose, 1.0 / 60.0, &mut out);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], Foley { kind: FoleyKind::FootGround, left: true, action: FoleyAction::Rn });
        assert_eq!(out[2].kind, FoleyKind::ChestGround);
    }

    #[test]
    fn steps_found_in_the_pose() {
        let skel = skeleton();
        let mut t = FoleyTracker::new(&skel).unwrap();
        let mut steps = 0;
        let mut leaves = 0;
        // Two seconds of running: each foot swings 0.1 - 0.35 m, out of phase.
        for i in 0..120 {
            let ph = i as f32 / 60.0 * std::f32::consts::TAU * 1.4;
            let h = |p: f32| 0.1 + 0.25 * (0.5 - 0.5 * p.cos()).powf(1.5);
            let pose = pose_with_feet(&skel, h(ph), h(ph + std::f32::consts::PI));
            let mut out = Vec::new();
            t.update(&[], false, true, FoleyAction::Rn, &pose, 1.0 / 60.0, &mut out);
            steps += out.iter().filter(|f| f.kind == FoleyKind::FootGround).count();
            leaves += out.iter().filter(|f| f.kind == FoleyKind::FootLeave).count();
        }
        // 1.4 cycles a second for two seconds, two feet: about 5-6 steps.
        assert!((4..=7).contains(&steps), "{steps} steps");
        assert!((4..=7).contains(&leaves), "{leaves} leaves");
    }
}
