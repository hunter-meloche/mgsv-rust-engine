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
}

/// A shot fired this tick, in world space.
#[derive(Clone, Copy, Debug)]
pub struct Shot {
    pub origin: Vec3,
    pub direction: Vec3,
    pub speed: f32,
    pub gravity: f32,
}

/// The equipped weapon and its runtime state.
pub struct Armed {
    pub stats: WeaponStats,
    pub rig: WeaponRig,
    pub ammo: u32,
    pub reserve: u32,
    grids: [Arc<Clip>; 2],
    reload_clip: Arc<Clip>,
    cooldown: f32,
    /// Seconds into the reload clip, while reloading.
    reloading: Option<f32>,
    fire_held: bool,
}

impl Armed {
    pub fn reloading(&self) -> bool {
        self.reloading.is_some()
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
    pub weapon: Option<Armed>,
    /// 0..1 blend of the aiming upper body.
    aim_weight: f32,
    /// Aim direction relative to the body (radians).
    aim_yaw: f32,
    aim_pitch: f32,
    /// Accumulated recoil (radians of camera pitch) for the host to apply.
    pub recoil: f32,
    pending_shots: u32,
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
            weapon: None,
            aim_weight: 0.0,
            aim_yaw: 0.0,
            aim_pitch: 0.0,
            recoil: 0.0,
            pending_shots: 0,
        })
    }

    /// Equip a weapon (full magazine).
    pub fn equip(
        &mut self,
        clips: &ClipLibrary,
        stats: WeaponStats,
        rig: WeaponRig,
    ) -> Result<(), String> {
        let get = |n: &str| clips.get(n).ok_or_else(|| format!("missing clip {n}"));
        self.weapon = Some(Armed {
            grids: [get(rig.grid[0])?, get(rig.grid[1])?],
            reload_clip: get(rig.reload)?,
            ammo: stats.magazine,
            reserve: stats.reserve,
            stats,
            rig,
            cooldown: 0.0,
            reloading: None,
            fire_held: false,
        });
        Ok(())
    }

    pub fn aiming(&self) -> bool {
        self.state == State::Aim
    }

    /// The clip currently playing (for debugging / HUD).
    pub fn current_clip(&self) -> &str {
        self.player.current_name()
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

    pub fn update(&mut self, rig: &HumanRig, clips: &ClipLibrary, dt: f32, input: &Input) {
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

        self.update_weapon(dt, input);
        let can_aim = self.weapon.is_some() && self.stance != Stance::Prone;
        let aim_now = input.aim && can_aim && !matches!(self.state, State::Changing { .. });
        if aim_now {
            self.state = State::Aim;
        } else if self.state == State::Aim {
            // Lower the weapon: back to plain locomotion.
            if moving {
                let name = self.names.looping(self.stance, gait);
                self.play(clips, &name, true, FADE);
                self.gait = gait;
                self.state = State::Loop;
            } else {
                self.foot = Foot::L;
                self.enter_idle(clips);
            }
        }
        let target_w = if self.state == State::Aim { 1.0 } else { 0.0 };
        self.aim_weight += (target_w - self.aim_weight).clamp(-6.0 * dt, 6.0 * dt);

        match self.state {
            State::Aim => {
                // Body turns toward the camera; the rest is taken by the aim grid.
                let diff = wrap_angle(input.camera_yaw - self.yaw);
                self.yaw = wrap_angle(self.yaw + diff.clamp(-10.0 * dt, 10.0 * dt));
                self.aim_yaw = wrap_angle(input.camera_yaw - self.yaw);
                self.aim_pitch = input.aim_pitch;
                let s = if self.stance == Stance::Crouch {
                    "q"
                } else {
                    "s"
                };
                let name = if moving {
                    let rel = wrap_angle(want_yaw - self.yaw).to_degrees();
                    let mut dir = ((rel / 45.0).round() as i32) * 45;
                    if dir == -180 {
                        dir = 180;
                    }
                    let g = if input.walk || amount < 0.6 {
                        "wk"
                    } else {
                        "rn"
                    };
                    self.gait = if g == "wk" { Gait::Walk } else { Gait::Run };
                    format!("snaprdy_{s}_fre0_{g}_lp_{dir}")
                } else {
                    self.gait = Gait::Idle;
                    format!("snaprdy_{s}_fre0_idl_0")
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
                    self.play(clips, &name, true, FADE);
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
                        self.play(clips, &name, true, FADE);
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
        self.position += self.rotation() * Vec3::new(dp.x, 0.0, dp.z);
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
        self.recoil *= (-8.0 * dt).exp();
        let Some(w) = self.weapon.as_mut() else {
            return;
        };
        w.cooldown -= dt;
        if let Some(t) = w.reloading.as_mut() {
            *t += dt;
            if *t >= w.reload_clip.frame_count as f32 / crate::FPS {
                let take = (w.stats.magazine - w.ammo).min(w.reserve);
                w.ammo += take;
                w.reserve -= take;
                w.reloading = None;
            }
        } else if input.reload && w.ammo < w.stats.magazine && w.reserve > 0 {
            w.reloading = Some(0.0);
        }
        let trigger = input.fire && (w.stats.full_auto || !w.fire_held);
        w.fire_held = input.fire;
        if aiming && trigger && w.reloading.is_none() && w.cooldown <= 0.0 && self.aim_weight > 0.8
        {
            if w.ammo > 0 {
                w.ammo -= 1;
                w.cooldown = 60.0 / w.stats.rounds_per_minute.max(1.0);
                self.pending_shots += 1;
                // Placeholder kick until the receiver wobble fields are decoded.
                self.recoil += 0.6f32.to_radians();
            } else if w.reserve > 0 {
                w.reloading = Some(0.0);
            }
        }
    }

    /// Shots fired since the last call, leaving the muzzle of the gun in
    /// `pose` (root space) along the aim direction, in world space.
    pub fn take_shots(&mut self, pose: &Pose) -> Vec<Shot> {
        let n = std::mem::take(&mut self.pending_shots);
        let Some(w) = &self.weapon else {
            return Vec::new();
        };
        let (gr, gp) = w.rig.gun_transform(pose);
        let muzzle = self.position + self.rotation() * (gp + gr * w.rig.muzzle);
        let heading = self.yaw + self.aim_yaw;
        let (cp, sp) = (self.aim_pitch.cos(), self.aim_pitch.sin());
        let direction = Vec3::new(heading.sin() * cp, sp, heading.cos() * cp);
        (0..n)
            .map(|_| Shot {
                origin: muzzle,
                direction,
                speed: w.stats.bullet_speed,
                gravity: w.stats.bullet_gravity,
            })
            .collect()
    }

    /// Whether the gun should be drawn in the hand (else holstered).
    pub fn weapon_in_hand(&self) -> bool {
        self.weapon
            .as_ref()
            .is_some_and(|w| self.aim_weight > 0.05 || w.reloading())
    }

    /// Skeleton pose in the character's root space.
    pub fn pose(&self, rig: &HumanRig, skel: &Skeleton) -> Result<Pose, String> {
        let mut frame = self.player.frame(rig)?;
        if let (Some(w), Some(upper)) = (&self.weapon, rig.mask("Upper")) {
            if self.aim_weight > 0.0 {
                let grid = &w.grids[(self.stance == Stance::Crouch) as usize];
                let aim =
                    w.rig
                        .aim_frame(rig, grid, self.aim_yaw, self.aim_pitch + self.recoil * 0.5)?;
                frame = rig.layer(&frame, &aim, upper, self.aim_weight);
            }
            if let Some(t) = w.reloading {
                let len = w.reload_clip.frame_count as f32 / crate::FPS;
                let ramp = (t / 0.15).min((len - t) / 0.15).clamp(0.0, 1.0);
                let reload = rig.sample_frame(&w.reload_clip, t * crate::FPS)?;
                frame = rig.layer(&frame, &reload, upper, ramp);
            }
        }
        Ok(rig.solve(skel, &frame))
    }
}

fn wrap_angle(a: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    (a + std::f32::consts::PI).rem_euclid(tau) - std::f32::consts::PI
}
