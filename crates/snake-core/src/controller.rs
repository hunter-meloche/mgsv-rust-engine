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
    /// Camera pitch (radians, up positive): where the weapon aims.
    pub aim_pitch: f32,
    /// Pressed this tick: switch to the weapon in this slot.
    pub switch_to: Option<usize>,
}

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
}

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
            ammo: stats.magazine,
            reserve: stats.reserve,
            stats,
            rig,
            cooldown: 0.0,
            reloading: None,
            firing: None,
            fire_held: false,
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

    pub fn aiming(&self) -> bool {
        self.state == State::Aim
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
                match self.names.transition(self.stance, to) {
                    Some(name) if self.play(clips, &name, false, 0.2) => {
                        self.state = State::Changing { to };
                        self.gait = Gait::Idle;
                    }
                    _ => self.stance = to,
                }
            }
        }

        if let Some(i) = input.switch_to.filter(|&i| i < self.weapons.len()) {
            if self.current != Some(i) {
                if let Some(w) = self.weapon_mut() {
                    w.reloading = None;
                    w.magazine_in_hand = None;
                    w.firing = None;
                }
                self.current = Some(i);
            }
        }
        self.update_weapon(dt, input);
        let slot = stance_slot(self.stance);
        let can_aim = self.weapon().is_some_and(|w| w.grids[slot].is_some());
        let aim_now = input.aim && can_aim && !matches!(self.state, State::Changing { .. });
        if aim_now {
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
                    self.enter_idle(clips);
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
        let aim_ready = self.aim_weight > 0.8;
        self.recoil *= (-8.0 * dt).exp();
        let mut shots = 0;
        let mut kick = 0.0;
        let unlimited = self.unlimited_magazines;
        let stance = self.stance;
        // Real seconds: the tempo does not change the fire rate.
        let real_dt = dt / self.tempo.max(0.01);
        // The weapon comes up before the reload motion starts.
        let raised = self.aim_weight.max(self.ready_weight) > 0.9;
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

    /// Whether the gun should be drawn in the hand (else holstered).
    pub fn weapon_in_hand(&self) -> bool {
        self.weapon()
            .is_some_and(|w| self.aim_weight > 0.05 || w.reloading())
    }

    /// Follow the current weapon's magazine for `pose` (call after each
    /// [`Controller::pose`]): in the gun at its `CNP_AMMO`, or, between the
    /// reload's magazine-out and magazine-in events, in the left hand where
    /// it was when it came out.
    pub fn track_magazine(&mut self, pose: &Pose) {
        self.magazine = None;
        let Some(w) = self.current.and_then(|i| self.weapons.get_mut(i)) else {
            return;
        };
        let Some(m) = w.rig.magazine else { return };
        let (gr, gp) = w.rig.gun_transform(pose);
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
            let raised = self.aim_weight.max(self.ready_weight);
            if raised > 0.0 {
                if let (Some(grid), Some(clip)) = (&w.rig.grids[slot], &w.grids[slot]) {
                    let aim = w.rig.aim_frame(
                        rig,
                        grid,
                        clip,
                        self.aim_yaw,
                        self.aim_pitch + self.recoil * 0.5,
                    )?;
                    frame = rig.layer(&frame, &aim, upper, raised);
                }
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
