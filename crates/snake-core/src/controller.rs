//! Locomotion controller: stances, gaits, stance changes and movement.
//!
//! Movement comes from the clips' own root motion (so feet do not slide);
//! the controller only picks clips, crossfades between them and turns the
//! character toward the requested direction.

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

/// Which clip plays for each stance/gait and stance change. Defaults are the
/// unarmed (`snapnon`) clips from `player2_resident.mtar`.
#[derive(Clone, Debug)]
pub struct ClipNames {
    pub locomotion: Vec<((Stance, Gait), &'static str)>,
    pub transitions: Vec<((Stance, Stance), &'static str)>,
}

impl Default for ClipNames {
    fn default() -> Self {
        use Gait::*;
        use Stance::*;
        Self {
            locomotion: vec![
                ((Stand, Idle), "snapnon_s_idl_l"),
                ((Stand, Walk), "snapnon_s_wk_lp_vr1_l"),
                ((Stand, Run), "snapnon_s_rn_lp_vr1_l"),
                ((Stand, Dash), "snapnon_s_dh_lp"),
                ((Crouch, Idle), "snapnon_q_idl_l"),
                ((Crouch, Walk), "snapnon_q_wk_lp_l"),
                ((Crouch, Run), "snapnon_q_rn_lp_l"),
                ((Prone, Idle), "snapnon_c_idl_l"),
                ((Prone, Walk), "snapnon_c_fst_lp_r"),
            ],
            transitions: vec![
                ((Stand, Crouch), "snapnon_q_idl_s2q_r"),
                ((Crouch, Stand), "snapnon_q_idl_q2s_l"),
                ((Stand, Prone), "snapnon_c_idl_s2c_l"),
                ((Prone, Stand), "snapnon_c_idl_c2s_l"),
                ((Crouch, Prone), "snapnon_c_idl_q2c_l"),
                ((Prone, Crouch), "snapnon_c_idl_c2q_l"),
            ],
        }
    }
}

impl ClipNames {
    /// The clip for a stance and gait, falling back to slower gaits.
    pub fn locomotion(&self, stance: Stance, gait: Gait) -> &'static str {
        let order = [Gait::Dash, Gait::Run, Gait::Walk, Gait::Idle];
        let start = order.iter().position(|g| *g == gait).unwrap_or(3);
        order[start..]
            .iter()
            .find_map(|g| {
                self.locomotion
                    .iter()
                    .find(|(k, _)| *k == (stance, *g))
                    .map(|(_, n)| *n)
            })
            .unwrap_or("snapnon_s_idl_l")
    }

    pub fn transition(&self, from: Stance, to: Stance) -> Option<&'static str> {
        self.transitions
            .iter()
            .find(|(k, _)| *k == (from, to))
            .map(|(_, n)| *n)
    }

    /// Every clip name referenced, for loading.
    pub fn all(&self) -> Vec<&'static str> {
        self.locomotion
            .iter()
            .map(|(_, n)| *n)
            .chain(self.transitions.iter().map(|(_, n)| *n))
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Locomotion,
    /// Playing a stance-change clip toward `to`.
    Changing {
        to: Stance,
    },
}

/// Crossfade time between locomotion clips (seconds).
const FADE: f32 = 0.25;

pub struct Controller {
    /// World position of the character root (on the ground).
    pub position: Vec3,
    /// Facing (radians about +Y; 0 faces +Z).
    pub yaw: f32,
    pub stance: Stance,
    pub gait: Gait,
    state: State,
    player: MotionPlayer,
    names: ClipNames,
}

impl Controller {
    pub fn new(clips: &ClipLibrary, names: ClipNames) -> Result<Self, String> {
        let missing: Vec<_> = names
            .all()
            .into_iter()
            .filter(|n| clips.get(n).is_none())
            .collect();
        if !missing.is_empty() {
            return Err(format!("missing clips: {}", missing.join(", ")));
        }
        let idle = names.locomotion(Stance::Stand, Gait::Idle);
        Ok(Self {
            position: Vec3::ZERO,
            yaw: 0.0,
            stance: Stance::Stand,
            gait: Gait::Idle,
            state: State::Locomotion,
            player: MotionPlayer::new(idle, clips.get(idle).unwrap(), true),
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

    pub fn update(&mut self, rig: &HumanRig, clips: &ClipLibrary, dt: f32, input: &Input) {
        // Stance changes (not while already changing).
        if self.state == State::Locomotion {
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
            if let Some(to) = target.filter(|t| *t != self.stance) {
                if let Some(name) = self.names.transition(self.stance, to) {
                    if let Some(clip) = clips.get(name) {
                        self.player.play(name, clip, false, 0.15);
                        self.state = State::Changing { to };
                        self.gait = Gait::Idle;
                    }
                } else {
                    self.stance = to;
                }
            }
        }

        // Desired movement in world space. Fox space: Y up, a heading of 0
        // faces +Z, and the right-hand side of that heading is -X.
        let (s, c) = input.camera_yaw.sin_cos();
        let forward = Vec3::new(s, 0.0, c);
        let right = Vec3::new(-c, 0.0, s);
        let world = right * input.move_x + forward * input.move_y;
        let amount = world.length().min(1.0);

        match self.state {
            State::Changing { to } => {
                if self.player.finished() || self.player.progress() > 0.95 {
                    self.stance = to;
                    self.state = State::Locomotion;
                    self.set_gait(clips, Gait::Idle, 0.2);
                }
            }
            State::Locomotion => {
                let gait = if amount < 0.15 {
                    Gait::Idle
                } else if input.sprint {
                    Gait::Dash
                } else if input.walk || amount < 0.6 {
                    Gait::Walk
                } else {
                    Gait::Run
                };
                if gait != self.gait {
                    self.set_gait(clips, gait, FADE);
                }
                // Turn toward the movement direction.
                if gait != Gait::Idle {
                    let want = world.x.atan2(world.z);
                    let rate = match self.stance {
                        Stance::Stand => 7.0,
                        Stance::Crouch => 5.0,
                        Stance::Prone => 2.0,
                    };
                    let diff = wrap_angle(want - self.yaw);
                    self.yaw = wrap_angle(self.yaw + diff.clamp(-rate * dt, rate * dt));
                }
            }
        }

        // Root motion, in the character's frame, moves it through the world.
        let (dr, dp) = self.player.advance(rig, dt);
        let facing = self.rotation();
        self.position += facing * Vec3::new(dp.x, 0.0, dp.z);
        // Stance changes may turn the body (e.g. rolling onto the back).
        if matches!(self.state, State::Changing { .. }) {
            let f = dr * Vec3::Z;
            self.yaw = wrap_angle(self.yaw + f.x.atan2(f.z));
        }
    }

    fn set_gait(&mut self, clips: &ClipLibrary, gait: Gait, fade: f32) {
        self.gait = gait;
        let name = self.names.locomotion(self.stance, gait);
        if name != self.player.current_name() {
            if let Some(clip) = clips.get(name) {
                self.player.play(name, clip, true, fade);
            }
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
