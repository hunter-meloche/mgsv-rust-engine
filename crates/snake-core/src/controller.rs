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

use fox_anim::{HumanRig, Pose, Quat, Skeleton, Vec3};

use crate::motion::{ClipLibrary, MotionPlayer};

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
        })
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

        match self.state {
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

    /// Skeleton pose in the character's root space.
    pub fn pose(&self, rig: &HumanRig, skel: &Skeleton) -> Result<Pose, String> {
        Ok(rig.solve(skel, &self.player.frame(rig)?))
    }
}

fn wrap_angle(a: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    (a + std::f32::consts::PI).rem_euclid(tau) - std::f32::consts::PI
}
