//! Locomotion controller: stances, gaits, starts/stops, stance changes.
//!
//! Movement comes from the clips' own root motion (so feet do not slide).
//! The controller picks clips the way the game's naming lays them out:
//!
//! * idles `snapnon_<stance>_idl_<foot>`
//! * loops `..._wk_lp_*`, `..._rn_lp_*`, `..._dh_lp`, prone `c_fst_lp_r`
//! * starts `..._<gait>_st_<l|r><0|45|90|135|180>_<foot>`: start moving with a
//!   turn of that many degrees to the left/right, from the `<foot>` idle
//! * stops `..._<gait>_ed_*_<foot>`, ending in the `<foot>` idle
//! * stance changes `..._idl_s2q_*` etc.
//!
//! Stops are chosen by matching foot positions against the current pose.

use std::sync::Arc;

use fox_anim::{HumanRig, Pose, Quat, Skeleton, Vec3};
use fox_data::mtar::Clip;

use crate::motion::{ClipLibrary, MotionPlayer};
use crate::weapon::{WeaponRig, WeaponStats};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stance {
    Stand,
    Crouch,
    Prone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Gait {
    Idle,
    Walk,
    Run,
    Dash,
}

/// Which idle variant the character is in (and which start clips fit it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Foot {
    L,
    R,
}

impl Foot {
    fn code(self) -> &'static str {
        match self {
            Foot::L => "l",
            Foot::R => "r",
        }
    }
}

/// One tick of player input.
#[derive(Clone, Copy, Debug, Default)]
pub struct Input {
    /// Movement stick: x right, y forward, relative to the camera; length 0..1.
    pub move_x: f32,
    pub move_y: f32,
    /// Camera heading (radians about +Y; 0 looks along +Z).
    pub camera_yaw: f32,
    /// Hold to dash (stand) / run.
    pub sprint: bool,
    /// Hold to walk slowly regardless of stick magnitude.
    pub walk: bool,
    /// Pressed this tick: toggle crouch.
    pub crouch: bool,
    /// Pressed this tick: toggle prone.
    pub prone: bool,
    /// Hold to raise the weapon and aim along the camera.
    pub aim: bool,
    /// Hold to fire (while aiming).
    pub fire: bool,
    /// Pressed this tick: reload.
    pub reload: bool,
    /// Pressed this tick: dive forward to prone (while running or dashing).
    pub dive: bool,
    /// Camera pitch (radians, up positive): where the weapon aims.
    pub aim_pitch: f32,
    /// Pressed this tick: switch to the weapon in this slot.
    pub switch_to: Option<usize>,
}

/// MGSV's dive from a run to prone. Its plain sibling `snapdiv_s_rn_f`
/// ends mirrored, in a prone pose with no idle; this one ends in
/// `snapnon_c_idl_l`.
pub const DIVE: &str = "snapdiv_s_rn_f_int";

/// A shot fired this tick, in world space.
#[derive(Clone, Copy, Debug)]
pub struct Shot {
    pub origin: Vec3,
    pub direction: Vec3,
    pub speed: f32,
    pub gravity: f32,
    /// MGSV damage per hit (see `WeaponStats`): lethal and sleep.
    pub lethal_damage: f32,
    pub sleep_damage: f32,
}

/// A carried weapon and its runtime state.
pub struct Armed {
    pub stats: WeaponStats,
    pub rig: WeaponRig,
    pub ammo: u32,
    pub reserve: u32,
    /// Aim grid clips for stand, crouch, prone.
    grids: [Option<Arc<Clip>>; 3],
    /// Reload clips: partial / empty magazine, stand / crouch.
    reload_partial: [Option<Arc<Clip>>; 2],
    reload_empty: [Option<Arc<Clip>>; 2],
    fire_clip: Option<Arc<Clip>>,
    cooldown: f32,
    reloading: Option<Reload>,
    /// The magazine's place relative to the left hand while it is out of
    /// the gun (taken when it comes out).
    magazine_in_hand: Option<(Quat, Vec3)>,
    /// Seconds into the fire clip, after a shot.
    firing: Option<f32>,
    fire_held: bool,
    /// Draw and holster clips (upper body), if any.
    draw_clip: Option<Arc<Clip>>,
    holster_clip: Option<Arc<Clip>>,
    /// The gun is in the hand (else in its holster).
    drawn: bool,
    /// Drawing or holstering, and a motion being faded out after it was
    /// cut short (with its remaining weight).
    handling: Option<Handling>,
    fading: Option<(Handling, f32)>,
    /// Seconds since the gun last changed hands: its transform blends from
    /// where it was over `SWAP_BLEND`.
    since_swap: f32,
}

/// The gun's transform blends from the holster to the hand (and back) over
/// this long after it changes hands: MGSV's clips put the hand within a few
/// centimetres of the holstered gun, not exactly on it.
const SWAP_BLEND: f32 = 0.12;

/// Reaching for the holstered gun before the draw clip (which starts with
/// the hand on it) plays.
const REACH: f32 = 0.15;

/// Drawing the gun from its holster, or putting it back: an upper-body clip
/// and the time the gun changes hands.
struct Handling {
    clip: Arc<Clip>,
    t: f32,
    length: f32,
    swap_at: f32,
    /// Drawing (else holstering).
    draw: bool,
    swapped: bool,
}

impl Handling {
    /// `hand_over`: the clip frame where the gun changes hands (else the
    /// clip's event, else halfway).
    fn new(clip: Arc<Clip>, draw: bool, hand_over: Option<f32>) -> Self {
        let length = clip.frame_count as f32 / crate::FPS;
        let event = if draw {
            crate::weapon::EVENT_WEAPON_TO_HAND
        } else {
            crate::weapon::EVENT_WEAPON_TO_HOLSTER
        };
        let swap_at = hand_over
            .or_else(|| {
                clip.events
                    .iter()
                    .find(|e| e.name == event)
                    .and_then(|e| e.frames.first())
                    .map(|&f| f as f32)
            })
            .map_or(length * 0.5, |f| f / crate::FPS);
        // A draw clip starts with the hand on the holstered gun: reach for
        // it first (holding the first frame), then play.
        let t = if draw { -REACH } else { 0.0 };
        Self { clip, t, length, swap_at, draw, swapped: false }
    }

    /// Seconds into the clip (0 while reaching).
    fn clip_time(&self) -> f32 {
        self.t.max(0.0)
    }

    /// The clip's frame in the standing aim: a draw ends there, a holster
    /// starts there.
    fn aim_frame(&self) -> f32 {
        if self.draw { self.clip.frame_count as f32 } else { 0.0 }
    }

    /// Weight on the upper body: in from whatever the body is doing (the
    /// reach, or quickly from the aim when holstering), out at the end into
    /// the aim (drawing) or locomotion.
    fn weight(&self) -> f32 {
        let rise = if self.draw { (self.t + REACH) / REACH } else { self.t / 0.08 };
        let edge = rise.min((self.length - self.t) / 0.12).clamp(0.0, 1.0);
        edge * edge * (3.0 - 2.0 * edge)
    }

    /// Nearly done: the aim beneath shows through.
    fn finishing(&self) -> bool {
        self.t >= self.length - 0.12
    }
}

/// A reload in progress: its clip and the times (seconds) of its magazine
/// events.
struct Reload {
    clip: Arc<Clip>,
    t: f32,
    length: f32,
    magazine_out: f32,
    magazine_in: f32,
    refilled: bool,
}

impl Reload {
    fn new(clip: Arc<Clip>) -> Self {
        let length = clip.frame_count as f32 / crate::FPS;
        let at = |name: u32| {
            clip.events
                .iter()
                .find(|e| e.name == name)
                .and_then(|e| e.frames.first())
                .map(|&f| f as f32 / crate::FPS)
        };
        let magazine_in = at(crate::weapon::EVENT_MAGAZINE_IN).unwrap_or(length * 0.6);
        let magazine_out = at(crate::weapon::EVENT_MAGAZINE_OUT)
            .unwrap_or(magazine_in * 0.4)
            .min(magazine_in);
        Self {
            clip,
            t: 0.0,
            length,
            magazine_out,
            magazine_in,
            refilled: false,
        }
    }

    /// Weight of the reload motion: eased in and out at the clip's ends.
    fn weight(&self) -> f32 {
        let edge = (self.t / 0.2).min((self.length - self.t) / 0.25).clamp(0.0, 1.0);
        edge * edge * (3.0 - 2.0 * edge)
    }
}

impl Armed {
    pub fn reloading(&self) -> bool {
        self.reloading.is_some()
    }

    /// Reload progress 0..1, if reloading.
    pub fn reload_progress(&self) -> Option<f32> {
        self.reloading.as_ref().map(|r| (r.t / r.length).clamp(0.0, 1.0))
    }

    /// The reload clip for the current magazine and stance.
    fn reload_clip(&self, stance: Stance) -> Option<Arc<Clip>> {
        let slot = if stance == Stance::Crouch { 1 } else { 0 };
        let (first, second) = if self.ammo == 0 {
            (&self.reload_empty, &self.reload_partial)
        } else {
            (&self.reload_partial, &self.reload_empty)
        };
        first[slot]
            .clone()
            .or_else(|| first[0].clone())
            .or_else(|| second[slot].clone())
            .or_else(|| second[0].clone())
    }

    /// Put rounds in the magazine (from the reserve unless `unlimited`).
    fn refill(&mut self, unlimited: bool) {
        let mut take = self.stats.magazine.saturating_sub(self.ammo);
        if !unlimited {
            take = take.min(self.reserve);
            self.reserve -= take;
        }
        self.ammo += take;
    }
}

fn stance_slot(s: Stance) -> usize {
    match s {
        Stance::Stand => 0,
        Stance::Crouch => 1,
        Stance::Prone => 2,
    }
}

/// Clip naming for one weapon set (`snapnon` = unarmed) in
/// `player2_resident.mtar`.
#[derive(Clone, Debug)]
pub struct ClipNames {
    pub set: &'static str,
}

impl Default for ClipNames {
    fn default() -> Self {
        Self { set: "snapnon" }
    }
}

fn stance_code(s: Stance) -> &'static str {
    match s {
        Stance::Stand => "s",
        Stance::Crouch => "q",
        Stance::Prone => "c",
    }
}

impl ClipNames {
    fn name(&self, rest: &str) -> String {
        format!("{}_{rest}", self.set)
    }

    /// Gait code used in clip names; crouch has no dash, prone only crawls.
    fn gait_code(stance: Stance, gait: Gait) -> Option<&'static str> {
        Some(match (stance, gait) {
            (_, Gait::Idle) => return None,
            (Stance::Prone, _) => "fst",
            (Stance::Stand, Gait::Walk) | (Stance::Crouch, Gait::Walk) => "wk",
            (Stance::Stand, Gait::Run) | (Stance::Crouch, _) => "rn",
            (Stance::Stand, Gait::Dash) => "dh",
        })
    }

    pub fn idle(&self, stance: Stance, foot: Foot) -> String {
        match stance {
            Stance::Prone => self.name("c_idl_l"),
            s => self.name(&format!("{}_idl_{}", stance_code(s), foot.code())),
        }
    }

    pub fn looping(&self, stance: Stance, gait: Gait) -> String {
        match (stance, gait) {
            (_, Gait::Idle) => self.idle(stance, Foot::L),
            (Stance::Stand, Gait::Walk) => self.name("s_wk_lp_vr1_l"),
            (Stance::Stand, Gait::Run) => self.name("s_rn_lp_vr1_l"),
            (Stance::Stand, Gait::Dash) => self.name("s_dh_lp"),
            (Stance::Crouch, Gait::Walk) => self.name("q_wk_lp_l"),
            (Stance::Crouch, _) => self.name("q_rn_lp_l"),
            (Stance::Prone, _) => self.name("c_fst_lp_r"),
        }
    }

    /// Start clips for turning `angle` degrees (positive = left) from the
    /// `foot` idle, best first.
    pub fn starts(&self, stance: Stance, gait: Gait, angle: f32, foot: Foot) -> Vec<String> {
        let Some(g) = Self::gait_code(stance, gait) else {
            return Vec::new();
        };
        let s = stance_code(stance);
        if stance == Stance::Prone {
            return if angle.abs() > 112.5 {
                vec![self.name("c_fst_st_180_l")]
            } else {
                vec![self.name("c_fst_st_l")]
            };
        }
        let step = ((angle.abs() / 45.0).round() as i32 * 45).min(180);
        let side = if angle >= 0.0 { "l" } else { "r" };
        [foot, if foot == Foot::L { Foot::R } else { Foot::L }]
            .iter()
            .map(|f| self.name(&format!("{s}_{g}_st_{side}{step}_{}", f.code())))
            .collect()
    }

    /// Stop clips for a gait, each with the idle variant it ends in.
    pub fn stops(&self, stance: Stance, gait: Gait) -> Vec<(String, Foot)> {
        let both = |base: &str| {
            vec![
                (self.name(&format!("{base}_l")), Foot::L),
                (self.name(&format!("{base}_r")), Foot::R),
            ]
        };
        match (stance, gait) {
            (_, Gait::Idle) => Vec::new(),
            (Stance::Stand, Gait::Walk) => both("s_wk_ed_vr1"),
            (Stance::Stand, Gait::Run) => both("s_rn_ed_vr1"),
            (Stance::Stand, Gait::Dash) => both("s_dh_ed"),
            (Stance::Crouch, Gait::Walk) => both("q_wk_ed"),
            (Stance::Crouch, _) => both("q_rn_ed"),
            (Stance::Prone, _) => vec![(self.name("c_fst_ed_l"), Foot::L)],
        }
    }

    pub fn transition(&self, from: Stance, to: Stance) -> Option<String> {
        use Stance::*;
        let rest = match (from, to) {
            (Stand, Crouch) => "q_idl_s2q_r",
            (Crouch, Stand) => "q_idl_q2s_l",
            (Stand, Prone) => "c_idl_s2c_l",
            (Prone, Stand) => "c_idl_c2s_l",
            (Crouch, Prone) => "c_idl_q2c_l",
            (Prone, Crouch) => "c_idl_c2q_l",
            _ => return None,
        };
        Some(self.name(rest))
    }

    /// Stance changes on the move (both feet's variants where MGSV has
    /// them): crouch-walking or running down to prone (a slide forward into
    /// the crawl), and from the crawl up into a crouched or standing run.
    /// MGSV has none for standing <-> crouching: it blends between the
    /// stances' loops.
    pub fn moving_transition(&self, to: Stance, gait: Gait) -> Vec<String> {
        let both = |base: &str| vec![self.name(&format!("{base}_l")), self.name(&format!("{base}_r"))];
        match (to, gait) {
            (Stance::Prone, Gait::Walk) => both("c_qwk2c"),
            (Stance::Prone, _) => both("c_qrn2c"),
            (Stance::Crouch, _) => both("c_c2qrn"),
            (Stance::Stand, _) => vec![self.name("c_c2srn_l")],
        }
    }

    /// Clips the controller cannot work without.
    pub fn required(&self) -> Vec<String> {
        let mut v = Vec::new();
        for stance in [Stance::Stand, Stance::Crouch, Stance::Prone] {
            v.push(self.idle(stance, Foot::L));
            for gait in [Gait::Walk, Gait::Run, Gait::Dash] {
                v.push(self.looping(stance, gait));
            }
        }
        v.dedup();
        v
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Idle,
    /// Playing a start clip that leads into a loop.
    Start,
    Loop,
    /// Playing a stop clip that ends in the `foot` idle.
    Stop {
        foot: Foot,
    },
    /// Playing a stance-change clip toward `to`.
    Changing {
        to: Stance,
    },
    /// Weapon raised: strafing locomotion, body follows the camera.
    Aim,
}

/// Crossfade time between locomotion loops (seconds).
const FADE: f32 = 0.25;

pub struct Controller {
    /// World position of the character root (on the ground).
    pub position: Vec3,
    /// Facing (radians about +Y; 0 faces +Z).
    pub yaw: f32,
    pub stance: Stance,
    pub gait: Gait,
    foot: Foot,
    state: State,
    player: MotionPlayer,
    names: ClipNames,
    /// Carried weapons and the one in use.
    pub weapons: Vec<Armed>,
    pub current: Option<usize>,
    /// 0..1 blend of the aiming upper body.
    aim_weight: f32,
    /// Aim direction relative to the body (radians).
    aim_yaw: f32,
    aim_pitch: f32,
    /// Accumulated recoil (radians of camera pitch) for the host to apply.
    pub recoil: f32,
    pending_shots: u32,
    /// 0..1 blend of the weapon raised for a reload while not aiming.
    ready_weight: f32,
    /// Reloads never use up the reserve.
    pub unlimited_magazines: bool,
    /// Tempo of all of Snake's motion: 1.5 plays every animation (and so
    /// moves, turns, changes stance, raises the gun and reloads) half again
    /// as fast. The fire rate stays the weapon's rounds per minute.
    pub tempo: f32,
    /// The current weapon's magazine (root space) and whether it is in the
    /// left hand; see [`Controller::track_magazine`].
    magazine: Option<(Quat, Vec3, bool)>,
    /// Foley the controller makes itself (raising the gun, switching
    /// weapons), until taken.
    foley: Vec<crate::foley::Foley>,
    /// The weapon to switch to once the current one is holstered.
    pending_switch: Option<usize>,
    /// After a stance change while aiming: the previous stance's aim grid
    /// slot and its remaining weight (the upper body blends across).
    aim_slot_fade: Option<(usize, f32)>,
}

/// Time to blend the aiming upper body from one stance's aim to the next.
const AIM_STANCE_FADE: f32 = 0.3;

impl Controller {
    pub fn new(clips: &ClipLibrary, names: ClipNames) -> Result<Self, String> {
        let missing: Vec<_> = names
            .required()
            .into_iter()
            .filter(|n| clips.get(n).is_none())
            .collect();
        if !missing.is_empty() {
            return Err(format!("missing clips: {}", missing.join(", ")));
        }
        let idle = names.idle(Stance::Stand, Foot::L);
        Ok(Self {
            position: Vec3::ZERO,
            yaw: 0.0,
            stance: Stance::Stand,
            gait: Gait::Idle,
            foot: Foot::L,
            state: State::Idle,
            player: MotionPlayer::new(&idle, clips.get(&idle).unwrap(), true),
            names,
            weapons: Vec::new(),
            current: None,
            aim_weight: 0.0,
            aim_yaw: 0.0,
            aim_pitch: 0.0,
            recoil: 0.0,
            pending_shots: 0,
            ready_weight: 0.0,
            unlimited_magazines: false,
            tempo: 1.0,
            magazine: None,
            foley: Vec::new(),
            pending_switch: None,
            aim_slot_fade: None,
        })
    }

    /// Add a weapon (full magazine); the first one added is selected.
    pub fn equip(
        &mut self,
        clips: &ClipLibrary,
        stats: WeaponStats,
        rig: WeaponRig,
    ) -> Result<(), String> {
        let get = |n: &str| clips.get(n).ok_or_else(|| format!("missing clip {n}"));
        let grid = |slot: usize| -> Result<Option<Arc<Clip>>, String> {
            rig.grids[slot].as_ref().map(|g| get(&g.clip)).transpose()
        };
        let reload = |names: &[Option<String>; 2]| -> Result<[Option<Arc<Clip>>; 2], String> {
            Ok([
                names[0].as_deref().map(get).transpose()?,
                names[1].as_deref().map(get).transpose()?,
            ])
        };
        let armed = Armed {
            grids: [grid(0)?, grid(1)?, grid(2)?],
            reload_partial: reload(&rig.reload.partial)?,
            reload_empty: reload(&rig.reload.empty)?,
            magazine_in_hand: None,
            fire_clip: rig.fire.as_deref().map(get).transpose()?,
            draw_clip: rig.draw.as_deref().map(get).transpose()?,
            holster_clip: rig.holster_clip.as_deref().map(get).transpose()?,
            ammo: stats.magazine,
            reserve: stats.reserve,
            stats,
            rig,
            cooldown: 0.0,
            reloading: None,
            firing: None,
            fire_held: false,
            drawn: false,
            handling: None,
            fading: None,
            since_swap: SWAP_BLEND,
        };
        self.weapons.push(armed);
        if self.current.is_none() {
            self.current = Some(self.weapons.len() - 1);
        }
        Ok(())
    }

    /// The weapon in use.
    pub fn weapon(&self) -> Option<&Armed> {
        self.current.and_then(|i| self.weapons.get(i))
    }

    fn weapon_mut(&mut self) -> Option<&mut Armed> {
        self.current.and_then(|i| self.weapons.get_mut(i))
    }

    /// The locomotion clip playing (for probes and debugging).
    pub fn clip_name(&self) -> &str {
        self.player.current_name()
    }

    pub fn aiming(&self) -> bool {
        self.state == State::Aim
    }

    fn push_handling_foley(&mut self) {
        use crate::foley::{Foley, FoleyKind};
        let action = self.foley_action();
        for kind in [FoleyKind::RattleWeapon, FoleyKind::RattleSuit] {
            self.foley.push(Foley { kind, left: false, action });
        }
    }

    /// Foley the controller made itself since the last call: raising the
    /// gun to aim and switching weapons rattle the gear.
    pub fn take_foley(&mut self) -> Vec<crate::foley::Foley> {
        std::mem::take(&mut self.foley)
    }

    /// Motion events (name hashes) the current clip passed since the last
    /// call (see [`crate::foley`]).
    pub fn take_motion_events(&mut self) -> Vec<u32> {
        self.player.take_crossed()
    }

    /// Whether the current clip marks its own foot contacts.
    pub fn clip_has_foot_events(&self) -> bool {
        self.player.current_has_event(crate::foley::event("left_foot_ground"))
            || self.player.current_has_event(crate::foley::event("right_foot_ground"))
    }

    /// The movement Snake's foley sounds are chosen for (MGSV's action
    /// switch values).
    pub fn foley_action(&self) -> crate::foley::FoleyAction {
        use crate::foley::FoleyAction as A;
        match (self.stance, self.gait, &self.state) {
            (_, _, State::Changing { to: Stance::Prone }) => A::Cp,
            (Stance::Prone, _, _) => A::Cr,
            (Stance::Crouch, Gait::Run | Gait::Dash, _) => A::RnC,
            (Stance::Crouch, _, _) => A::WkC,
            (Stance::Stand, Gait::Dash, _) => A::Dh,
            (Stance::Stand, Gait::Run, _) => A::Rn,
            (Stance::Stand, _, _) => A::Wk,
        }
    }

    /// The clip currently playing (for debugging / HUD).
    pub fn current_clip(&self) -> &str {
        self.player.current_name()
    }

    /// Normalised progress of the current clip (for debugging).
    pub fn clip_progress(&self) -> f32 {
        self.player.progress()
    }

    /// The character's world rotation.
    pub fn rotation(&self) -> Quat {
        Quat::from_rotation_y(self.yaw)
    }

    fn play(&mut self, clips: &ClipLibrary, name: &str, looping: bool, fade: f32) -> bool {
        match clips.get(name) {
            Some(clip) => {
                if name != self.player.current_name() {
                    self.player.play(name, clip, looping, fade);
                }
                true
            }
            None => false,
        }
    }

    /// Enter a locomotion loop at the frame whose limb positions best match
    /// the current pose (a start or another loop rarely ends where the loop
    /// begins; entering at frame 0 rewinds the stride, visibly).
    fn play_loop_matched(&mut self, rig: &HumanRig, clips: &ClipLibrary, name: &str) -> bool {
        let Some(clip) = clips.get(name) else { return false };
        if name == self.player.current_name() {
            return true;
        }
        let mut start = 0.0;
        if let Ok(cur) = self.player.frame(rig) {
            let mut best = f32::MAX;
            for f in 0..clip.frame_count {
                if let Ok(frame) = rig.sample_frame(&clip, f as f32) {
                    let d = rig.target_distance(&cur, &frame);
                    if d < best {
                        best = d;
                        start = f as f32;
                    }
                }
            }
        }
        self.player.play_at(name, clip, true, FADE, start);
        true
    }

    /// Advance one tick. Movement slides along `world`'s obstacles.
    pub fn update(
        &mut self,
        rig: &HumanRig,
        clips: &ClipLibrary,
        collision: &dyn crate::world::Collision,
        dt: f32,
        input: &Input,
    ) {
        let dt = dt * self.tempo;
        // Desired movement in world space. Fox space: Y up, a heading of 0
        // faces +Z, and the right-hand side of that heading is -X.
        let (s, c) = input.camera_yaw.sin_cos();
        let forward = Vec3::new(s, 0.0, c);
        let right = Vec3::new(-c, 0.0, s);
        let world = right * input.move_x + forward * input.move_y;
        let amount = world.length().min(1.0);
        let moving = amount >= 0.15;
        let want_yaw = world.x.atan2(world.z);
        let gait = if !moving {
            Gait::Idle
        } else if input.sprint {
            Gait::Dash
        } else if input.walk || amount < 0.6 {
            Gait::Walk
        } else {
            Gait::Run
        };

        // Diving: running or dashing (standing or crouched), Snake dives
        // forward and lands prone. MGSV's dive from a run ends exactly in
        // the prone idle; the stance changes when it finishes.
        if input.dive
            && moving
            && matches!(self.stance, Stance::Stand | Stance::Crouch)
            && matches!(self.state, State::Start | State::Loop)
            && self.play(clips, DIVE, false, 0.15)
        {
            self.state = State::Changing { to: Stance::Prone };
            self.gait = Gait::Idle;
        }

        // Dashing while moving crouched rises straight into the dash (MGSV:
        // the dash button stands Snake up without stopping), blending from
        // the crouched stride into the matching dash frame.
        if input.sprint
            && moving
            && self.stance == Stance::Crouch
            && matches!(self.state, State::Start | State::Loop)
        {
            self.stance = Stance::Stand;
            let name = self.names.looping(Stance::Stand, Gait::Dash);
            if self.play_loop_matched(rig, clips, &name) {
                self.gait = Gait::Dash;
                self.state = State::Loop;
            }
        }

        // Stance changes.
        if !matches!(self.state, State::Changing { .. }) {
            let target = if input.prone {
                Some(if self.stance == Stance::Prone {
                    Stance::Stand
                } else {
                    Stance::Prone
                })
            } else if input.crouch {
                Some(match self.stance {
                    Stance::Stand => Stance::Crouch,
                    Stance::Crouch => Stance::Stand,
                    Stance::Prone => Stance::Crouch,
                })
            } else {
                None
            };
            if let Some(to) = target {
                self.change_stance(rig, clips, to, moving, gait);
            }
        }

        // Switching weapons: the one in the hands goes back in its holster
        // first, then the other comes out if Snake is still aiming.
        if let Some(i) = input.switch_to.filter(|&i| i < self.weapons.len()) {
            if self.current != Some(i) {
                if let Some(w) = self.weapon_mut() {
                    w.reloading = None;
                    w.magazine_in_hand = None;
                    w.firing = None;
                }
                self.pending_switch = Some(i);
            } else {
                self.pending_switch = None;
            }
        }
        if let Some(i) = self.pending_switch {
            if self.weapon().is_none_or(|w| !w.drawn && w.handling.is_none()) {
                self.current = Some(i);
                self.pending_switch = None;
                // Handling the gear: suit and weapon rattle.
                self.push_handling_foley();
            }
        }
        let slot = stance_slot(self.stance);
        let can_aim =
            self.pending_switch.is_none() && self.weapon().is_some_and(|w| w.grids[slot].is_some());
        // The gun comes out to aim or reload and goes back afterwards.
        let reloading = self.weapon().is_some_and(Armed::reloading);
        let out = (input.aim && can_aim) || (reloading && self.pending_switch.is_none());
        // Not while changing stance: the draw and holster are laid onto the
        // stance's aim, and the stance is only settled at the end (drawing
        // while crouching down raised the arms to standing height).
        let changing = matches!(self.state, State::Changing { .. });
        self.update_handling(dt, (!changing).then_some(out));
        self.update_weapon(dt, input);
        let aim_now = input.aim && can_aim && !matches!(self.state, State::Changing { .. });
        if aim_now {
            if self.state != State::Aim {
                // Bringing the gun up.
                self.push_handling_foley();
            }
            self.state = State::Aim;
        } else if self.state == State::Aim {
            // Lower the weapon: back to plain locomotion.
            if moving {
                let name = self.names.looping(self.stance, gait);
                self.play_loop_matched(rig, clips, &name);
                self.gait = gait;
                self.state = State::Loop;
            } else {
                self.foot = Foot::L;
                self.enter_idle(clips);
            }
        }
        if let Some((_, weight)) = self.aim_slot_fade.as_mut() {
            *weight -= dt / AIM_STANCE_FADE;
        }
        if self.aim_slot_fade.is_some_and(|(_, weight)| weight <= 0.0) {
            self.aim_slot_fade = None;
        }
        let target_w = if self.state == State::Aim { 1.0 } else { 0.0 };
        self.aim_weight += (target_w - self.aim_weight).clamp(-6.0 * dt, 6.0 * dt);
        // A reload raises the weapon even when not aiming (to the aim grid's
        // centre: the aim angles relax to straight ahead).
        let reloading = self.weapon().is_some_and(Armed::reloading);
        let ready = if reloading { 1.0 } else { 0.0 };
        self.ready_weight += (ready - self.ready_weight).clamp(-5.0 * dt, 5.0 * dt);
        if self.state != State::Aim {
            let relax = (-6.0 * dt).exp();
            self.aim_yaw *= relax;
            self.aim_pitch *= relax;
        }

        match self.state {
            State::Aim => {
                // Body turns toward the camera; the rest is taken by the aim grid.
                let diff = wrap_angle(input.camera_yaw - self.yaw);
                let rate = if self.stance == Stance::Prone {
                    2.0
                } else {
                    10.0
                };
                self.yaw = wrap_angle(self.yaw + diff.clamp(-rate * dt, rate * dt));
                self.aim_yaw = wrap_angle(input.camera_yaw - self.yaw);
                self.aim_pitch = input.aim_pitch;
                let name = match (self.stance, moving) {
                    (Stance::Prone, false) => {
                        self.gait = Gait::Idle;
                        "snaprdy_c_fre0_idl_0".to_string()
                    }
                    (Stance::Prone, true) => {
                        self.gait = Gait::Walk;
                        format!("snapasr_c_fre0_wk_lp_{}", strafe_dir(want_yaw - self.yaw))
                    }
                    (stance, true) => {
                        let s = if stance == Stance::Crouch { "q" } else { "s" };
                        let g = if input.walk || amount < 0.6 {
                            "wk"
                        } else {
                            "rn"
                        };
                        self.gait = if g == "wk" { Gait::Walk } else { Gait::Run };
                        format!(
                            "snaprdy_{s}_fre0_{g}_lp_{}",
                            strafe_dir(want_yaw - self.yaw)
                        )
                    }
                    (stance, false) => {
                        let s = if stance == Stance::Crouch { "q" } else { "s" };
                        self.gait = Gait::Idle;
                        format!("snaprdy_{s}_fre0_idl_0")
                    }
                };
                self.play(clips, &name, true, 0.2);
            }
            State::Changing { to } => {
                if self.player.finished() || self.player.progress() > 0.95 {
                    self.stance = to;
                    self.foot = Foot::L;
                    // Still moving (a moving transition, or set off during
                    // one): straight into the new stance's loop.
                    let name = self.names.looping(to, gait);
                    if moving && self.play_loop_matched(rig, clips, &name) {
                        self.gait = gait;
                        self.state = State::Loop;
                    } else {
                        self.enter_idle(clips);
                    }
                }
            }
            State::Idle => {
                if moving {
                    self.begin_moving(clips, gait, want_yaw);
                }
            }
            State::Start => {
                if !moving {
                    self.begin_stop(rig, clips);
                } else if self.player.finished() || self.player.progress() > 0.9 {
                    let name = self.names.looping(self.stance, gait);
                    self.play_loop_matched(rig, clips, &name);
                    self.gait = gait;
                    self.state = State::Loop;
                }
            }
            State::Loop => {
                if !moving {
                    self.begin_stop(rig, clips);
                } else {
                    if gait != self.gait {
                        let name = self.names.looping(self.stance, gait);
                        self.play_loop_matched(rig, clips, &name);
                        self.gait = gait;
                    }
                    let rate = match self.stance {
                        Stance::Stand => 7.0,
                        Stance::Crouch => 5.0,
                        Stance::Prone => 2.0,
                    };
                    let diff = wrap_angle(want_yaw - self.yaw);
                    self.yaw = wrap_angle(self.yaw + diff.clamp(-rate * dt, rate * dt));
                }
            }
            State::Stop { foot } => {
                if moving && self.player.progress() > 0.3 {
                    self.foot = foot;
                    self.begin_moving(clips, gait, want_yaw);
                } else if self.player.finished() || self.player.progress() > 0.95 {
                    self.foot = foot;
                    self.enter_idle(clips);
                }
            }
        }

        // Root motion, in the character's frame, moves and turns it.
        let (dr, dp) = self.player.advance(rig, dt);
        let delta = self.rotation() * Vec3::new(dp.x, 0.0, dp.z);
        let radius = if self.stance == Stance::Prone {
            0.35
        } else {
            0.3
        };
        let height = match self.stance {
            Stance::Stand => 1.8,
            Stance::Crouch => 1.1,
            Stance::Prone => 0.5,
        };
        self.position = collision.slide(self.position, delta, radius, height);
        let f = dr * Vec3::Z;
        self.yaw = wrap_angle(self.yaw + f.x.atan2(f.z));
    }

    /// Change stance to `to`. On the move (or aiming) Snake does not stop,
    /// as in MGSV: standing <-> crouching switches at once and blends into
    /// the new stance's loop (and aim); down to prone and up from it play
    /// MGSV's moving transitions, which carry on into the new stance's loop.
    /// Standing still, the stance-change clips play.
    fn change_stance(&mut self, rig: &HumanRig, clips: &ClipLibrary, to: Stance, moving: bool, gait: Gait) {
        let from = self.stance;
        let aiming = self.state == State::Aim;
        let walking = moving && (aiming || matches!(self.state, State::Start | State::Loop));
        let upright = |s: Stance| s != Stance::Prone;
        if upright(from) && upright(to) && (walking || aiming) {
            self.stance = to;
            if aiming {
                let slots = (stance_slot(from), stance_slot(to));
                if slots.0 != slots.1 {
                    self.aim_slot_fade = Some((slots.0, 1.0));
                }
            } else {
                let gait = if to == Stance::Crouch && gait == Gait::Dash { Gait::Run } else { gait };
                let name = self.names.looping(to, gait);
                if self.play_loop_matched(rig, clips, &name) {
                    self.gait = gait;
                    self.state = State::Loop;
                }
            }
            return;
        }
        if walking {
            // The variant whose first frame is closest to the current feet.
            let move_gait = if from == Stance::Prone { Gait::Run } else { gait };
            let current = self.player.frame(rig).ok();
            let best = self
                .names
                .moving_transition(to, move_gait)
                .into_iter()
                .filter_map(|name| {
                    let clip = clips.get(&name)?;
                    let d = match &current {
                        Some(cur) => rig.target_distance(cur, &rig.sample_frame(&clip, 0.0).ok()?),
                        None => 0.0,
                    };
                    Some((d, name))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((_, name)) = best {
                if self.play(clips, &name, false, 0.2) {
                    self.state = State::Changing { to };
                    return;
                }
            }
        }
        match self.names.transition(from, to) {
            Some(name) if self.play(clips, &name, false, 0.2) => {
                self.state = State::Changing { to };
                self.gait = Gait::Idle;
            }
            _ => self.stance = to,
        }
    }

    fn enter_idle(&mut self, clips: &ClipLibrary) {
        let name = self.names.idle(self.stance, self.foot);
        if !self.play(clips, &name, true, 0.2) {
            self.foot = Foot::L;
            let name = self.names.idle(self.stance, Foot::L);
            self.play(clips, &name, true, 0.2);
        }
        self.gait = Gait::Idle;
        self.state = State::Idle;
    }

    fn begin_moving(&mut self, clips: &ClipLibrary, gait: Gait, want_yaw: f32) {
        let angle = wrap_angle(want_yaw - self.yaw).to_degrees();
        for name in self.names.starts(self.stance, gait, angle, self.foot) {
            if self.play(clips, &name, false, 0.15) {
                self.gait = gait;
                self.state = State::Start;
                return;
            }
        }
        // No start clip: turn to face and go straight into the loop.
        self.yaw = want_yaw;
        let name = self.names.looping(self.stance, gait);
        self.play(clips, &name, true, FADE);
        self.gait = gait;
        self.state = State::Loop;
    }

    /// Pick the stop clip whose first frame best matches the current feet.
    fn begin_stop(&mut self, rig: &HumanRig, clips: &ClipLibrary) {
        let current = self.player.frame(rig).ok();
        let best = self
            .names
            .stops(self.stance, self.gait)
            .into_iter()
            .filter_map(|(name, foot)| {
                let clip = clips.get(&name)?;
                let d = match &current {
                    Some(cur) => rig.target_distance(cur, &rig.sample_frame(&clip, 0.0).ok()?),
                    None => 0.0,
                };
                Some((d, name, foot))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        match best {
            Some((_, name, foot)) => {
                self.play(clips, &name, false, 0.2);
                self.state = State::Stop { foot };
                self.gait = Gait::Idle;
            }
            None => self.enter_idle(clips),
        }
    }

    fn update_weapon(&mut self, dt: f32, input: &Input) {
        let aiming = self.state == State::Aim;
        // Out of the holster and up.
        let in_hand = self
            .weapon()
            .is_some_and(|w| w.drawn && w.handling.as_ref().is_none_or(Handling::finishing));
        let aim_ready = self.aim_weight > 0.8 && in_hand;
        self.recoil *= (-8.0 * dt).exp();
        let mut shots = 0;
        let mut kick = 0.0;
        let unlimited = self.unlimited_magazines;
        let stance = self.stance;
        // Real seconds: the tempo does not change the fire rate.
        let real_dt = dt / self.tempo.max(0.01);
        // The weapon comes up before the reload motion starts.
        let raised = self.aim_weight.max(self.ready_weight) > 0.9 && in_hand;
        if let Some(w) = self.weapon_mut() {
            w.cooldown -= real_dt;
            if let Some(t) = w.firing.as_mut() {
                *t += dt;
                let len = w
                    .fire_clip
                    .as_ref()
                    .map_or(0.0, |c| c.frame_count as f32 / crate::FPS);
                if *t >= len {
                    w.firing = None;
                }
            }
            let can_reload = |w: &Armed| w.ammo < w.stats.magazine && (unlimited || w.reserve > 0);
            if let Some(r) = w.reloading.as_mut() {
                if raised || r.t > 0.0 {
                    r.t += dt;
                }
                // Rounds go in with the magazine (the clip's MAGAZINE_IN).
                let refill = !r.refilled && r.t >= r.magazine_in;
                let done = r.t >= r.length;
                if refill {
                    r.refilled = true;
                }
                if refill || (done && !r.refilled) {
                    w.refill(unlimited);
                }
                if done {
                    w.reloading = None;
                    w.magazine_in_hand = None;
                }
            } else if input.reload && can_reload(w) {
                w.reloading = w.reload_clip(stance).map(Reload::new);
            }
            let trigger = input.fire && (w.stats.full_auto || !w.fire_held);
            w.fire_held = input.fire;
            if aiming && aim_ready && trigger && w.reloading.is_none() && w.cooldown <= 0.0 {
                if w.ammo > 0 {
                    w.ammo -= 1;
                    w.cooldown = 60.0 / w.stats.rounds_per_minute.max(1.0);
                    w.firing = w.fire_clip.as_ref().map(|_| 0.0);
                    shots += 1;
                    // Placeholder kick until the receiver wobble fields are decoded.
                    kick += 0.6f32.to_radians();
                } else if unlimited || w.reserve > 0 {
                    w.reloading = w.reload_clip(stance).map(Reload::new);
                }
            }
        }
        self.pending_shots += shots;
        self.recoil += kick;
    }
    /// Shots fired since the last call, leaving the muzzle of the gun in
    /// `pose` (root space) along the aim direction, in world space.
    ///
    /// If the barrel pokes through an obstacle (the character is pressed
    /// against a wall), shots start at the obstacle instead of beyond it.
    pub fn take_shots(&mut self, pose: &Pose, world: &dyn crate::world::Collision) -> Vec<Shot> {
        let n = std::mem::take(&mut self.pending_shots);
        let Some(w) = self.weapon() else {
            return Vec::new();
        };
        let (gr, gp) = w.rig.gun_transform(pose);
        let mut muzzle = self.position + self.rotation() * (gp + gr * w.rig.muzzle);
        let centre = Vec3::new(self.position.x, muzzle.y, self.position.z);
        if let Some(hit) = world.raycast(centre, muzzle) {
            muzzle = hit.point + (centre - muzzle).normalize_or_zero() * 0.02;
        }
        let heading = self.yaw + self.aim_yaw;
        let (cp, sp) = (self.aim_pitch.cos(), self.aim_pitch.sin());
        let direction = Vec3::new(heading.sin() * cp, sp, heading.cos() * cp);
        (0..n)
            .map(|_| Shot {
                origin: muzzle,
                direction,
                speed: w.stats.bullet_speed,
                gravity: w.stats.bullet_gravity,
                lethal_damage: w.stats.lethal_damage,
                sleep_damage: w.stats.sleep_damage,
            })
            .collect()
    }

    /// Whether the gun is in the hand (else holstered).
    pub fn weapon_in_hand(&self) -> bool {
        self.weapon().is_some_and(|w| w.drawn)
    }

    /// Draw the current weapon (`out` true) or put it back in its holster
    /// (false; None: carry on with what is under way): MGSV's draw and
    /// holster clips play on the upper body and the gun changes hands at
    /// their events. Prone (the clips are standing ones) and for weapons
    /// without clips it changes hands at once.
    fn update_handling(&mut self, dt: f32, out: Option<bool>) {
        let prone = self.stance == Stance::Prone;
        let mut holstering = false;
        let Some(w) = self.weapon_mut() else { return };
        w.since_swap += dt;
        if let Some((_, weight)) = w.fading.as_mut() {
            *weight -= dt / 0.1;
        }
        if w.fading.as_ref().is_some_and(|(_, weight)| *weight <= 0.0) {
            w.fading = None;
        }
        if let Some(h) = w.handling.as_mut() {
            h.t += dt;
            if !h.swapped && h.t >= h.swap_at {
                h.swapped = true;
                w.drawn = h.draw;
                w.since_swap = 0.0;
            }
        }
        if w.handling.as_ref().is_some_and(|h| h.t >= h.length) {
            w.handling = None;
        }
        let heading_out = w.handling.as_ref().map_or(w.drawn, |h| h.draw);
        let Some(out) = out else { return };
        if out == heading_out {
            return;
        }
        // Turning back: fade the motion out from where it is.
        if let Some(h) = w.handling.take() {
            let weight = h.weight();
            w.fading = Some((h, weight));
        }
        if w.drawn == out {
            return;
        }
        let clip = if out { &w.draw_clip } else { &w.holster_clip };
        match clip.clone().filter(|_| !prone) {
            Some(clip) => {
                let hand_over = w.rig.hand_over[if out { 0 } else { 1 }];
                w.handling = Some(Handling::new(clip, out, hand_over));
                holstering = !out;
            }
            None => {
                w.drawn = out;
                w.since_swap = 0.0;
            }
        }
        if holstering {
            self.push_handling_foley();
        }
    }

    /// Weapon `i`'s transform in `pose`'s space and whether it is in the
    /// hand: in the hand, in its holster, or between the two just after it
    /// changed hands. None when it is holstered and has no holster, or its
    /// holster hides it.
    pub fn gun_place(&self, i: usize, pose: &Pose) -> Option<(Quat, Vec3, bool)> {
        let w = self.weapons.get(i)?;
        let current = self.current == Some(i);
        let in_hand = current && w.drawn;
        let hand = w.rig.gun_transform(pose);
        let holster = w.rig.holstered(pose);
        let s = if current { (w.since_swap / SWAP_BLEND).clamp(0.0, 1.0) } else { 1.0 };
        if !in_hand && w.rig.holster_hides && s >= 1.0 {
            return None;
        }
        let (from, to) = if in_hand { (holster, hand) } else { (Some(hand), holster?) };
        let s = s * s * (3.0 - 2.0 * s);
        let (r, p) = match from {
            Some((fr, fp)) if s < 1.0 => (fr.slerp(to.0, s), fp.lerp(to.1, s)),
            _ => to,
        };
        Some((r, p, in_hand))
    }

    /// Follow the current weapon's magazine for `pose` (call after each
    /// [`Controller::pose`]): in the gun at its `CNP_AMMO`, or, between the
    /// reload's magazine-out and magazine-in events, in the left hand where
    /// it was when it came out.
    pub fn track_magazine(&mut self, pose: &Pose) {
        self.magazine = None;
        let Some((gr, gp, _)) = self.current.and_then(|i| self.gun_place(i, pose)) else {
            return;
        };
        let Some(w) = self.current.and_then(|i| self.weapons.get_mut(i)) else {
            return;
        };
        let Some(m) = w.rig.magazine else { return };
        let in_gun = (gr * m.cnp.0, gp + gr * m.cnp.1);
        let out = w
            .reloading
            .as_ref()
            .is_some_and(|r| r.t >= r.magazine_out && r.t < r.magazine_in);
        if !out {
            w.magazine_in_hand = None;
            self.magazine = Some((in_gun.0, in_gun.1, false));
            return;
        }
        let (lr, lp) = (pose.rotations[m.left_hand_bone], pose.positions[m.left_hand_bone]);
        let off = *w.magazine_in_hand.get_or_insert_with(|| {
            let li = lr.inverse();
            (li * in_gun.0, li * (in_gun.1 - lp))
        });
        self.magazine = Some((lr * off.0, lp + lr * off.1, true));
    }

    /// The current weapon's magazine in root space and whether it is in the
    /// left hand (from the last [`Controller::track_magazine`]).
    pub fn magazine(&self) -> Option<(Quat, Vec3, bool)> {
        self.magazine
    }

    /// Skeleton pose in the character's root space.
    pub fn pose(&self, rig: &HumanRig, skel: &Skeleton) -> Result<Pose, String> {
        let mut frame = self.player.frame(rig)?;
        if let (Some(w), Some(upper)) = (self.weapon(), rig.mask("Upper")) {
            let slot = stance_slot(self.stance);
            // The stance's aim pose, blended from the previous stance's just
            // after a stance change.
            let aim_at = |slot: usize, pitch: f32| -> Result<Option<fox_anim::RigFrame>, String> {
                match (&w.rig.grids[slot], &w.grids[slot]) {
                    (Some(grid), Some(clip)) => w.rig.aim_frame(rig, grid, clip, self.aim_yaw, pitch).map(Some),
                    _ => Ok(None),
                }
            };
            let aim_blended = |pitch: f32| -> Result<Option<fox_anim::RigFrame>, String> {
                let aim = aim_at(slot, pitch)?;
                Ok(match (aim, self.aim_slot_fade) {
                    (Some(aim), Some((from, weight))) => match aim_at(from, pitch)? {
                        Some(old) => Some(rig.blend(&aim, &old, weight.clamp(0.0, 1.0))),
                        None => Some(aim),
                    },
                    (aim, _) => aim,
                })
            };
            let raised = self.aim_weight.max(self.ready_weight);
            if raised > 0.0 {
                if let Some(aim) = aim_blended(self.aim_pitch + self.recoil * 0.5)? {
                    frame = rig.layer(&frame, &aim, upper, raised);
                }
            }
            // Drawing or holstering (and a cut-short one fading out). MGSV's
            // clips are standing ones that end (draw) or start (holster) in
            // the standing aim: they are applied as their change from that
            // frame onto the current stance's aim, so crouched they start
            // and end on the crouched aim instead of forcing standing arms.
            let fading = w.fading.as_ref().map(|(h, weight)| (h, h.weight().min(*weight)));
            let handling = w.handling.as_ref().map(|h| (h, h.weight()));
            let mut aim_now = None;
            for (h, weight) in fading.into_iter().chain(handling) {
                if weight <= 0.0 {
                    continue;
                }
                if aim_now.is_none() {
                    aim_now = Some(match aim_blended(self.aim_pitch)? {
                        Some(aim) => aim,
                        None => aim_at(0, self.aim_pitch)?.unwrap_or_else(|| frame.clone()),
                    });
                }
                let aim = aim_now.as_ref().expect("set above");
                let reference = rig.sample_frame(&h.clip, h.aim_frame())?;
                let now = rig.sample_frame(&h.clip, h.clip_time() * crate::FPS)?;
                let over = rig.add_delta(aim, &reference, &now, upper, 1.0);
                frame = rig.layer(&frame, &over, upper, weight);
            }
            // Per-shot motion, applied as a change from the clip's first
            // frame so it works at any aim angle.
            if let (Some(t), Some(fire)) = (w.firing, &w.fire_clip) {
                let reference = rig.sample_frame(fire, 0.0)?;
                let now = rig.sample_frame(fire, t * crate::FPS)?;
                frame = rig.add_delta(&frame, &reference, &now, upper, self.aim_weight);
            }
            // The reload, applied as its change from its first frame (every
            // reload clip starts and ends in its stance's aim pose): it
            // starts and ends exactly on the current pose, at any aim angle
            // and in stances without their own clip.
            if let Some(r) = &w.reloading {
                let reference = rig.sample_frame(&r.clip, 0.0)?;
                let now = rig.sample_frame(&r.clip, r.t * crate::FPS)?;
                frame = rig.add_delta(&frame, &reference, &now, upper, r.weight());
            }
        }
        Ok(rig.solve(skel, &frame))
    }
}

/// Strafe clip direction label (degrees, 45-degree steps, left positive).
fn strafe_dir(rel: f32) -> i32 {
    let mut dir = ((wrap_angle(rel).to_degrees() / 45.0).round() as i32) * 45;
    if dir == -180 {
        dir = 180;
    }
    dir
}

fn wrap_angle(a: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    (a + std::f32::consts::PI).rem_euclid(tau) - std::f32::consts::PI
}
