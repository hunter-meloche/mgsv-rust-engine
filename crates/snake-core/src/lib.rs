//! Snake's player systems, independent of any renderer or host engine.
//!
//! * [`MotionPlayer`]: plays `.gani` clips on the human rig with crossfades
//!   and extracts root motion (looping clips wrap seamlessly).
//! * [`Controller`]: locomotion: stand / crouch / prone stances, walk, run
//!   and dash gaits, start / stop clips with turns, stance changes,
//!   root-motion-driven movement, aiming (strafe loops + aim pose grid on
//!   the upper body), firing and reloading.
//! * [`weapon`]: weapon stats from the game's tables, gun attachment,
//!   ballistics.
//! * [`world`]: the pluggable [`Collision`] interface (raycasts, capsule
//!   sliding) and a reference box world.
//!
//! The host feeds an [`Input`] each tick and reads back the character's
//! world transform and skeleton [`fox_anim::Pose`].

pub mod controller;
pub mod motion;
pub mod weapon;
pub mod world;

pub use controller::{Armed, ClipNames, Controller, Foot, Gait, Input, Shot, Stance};
pub use motion::{ClipLibrary, MotionPlayer};
pub use weapon::{AimGrid, Ballistics, WeaponClips, WeaponRig, WeaponStats};
pub use world::{Aabb, BoxWorld, Collision, FlatGround, Hit};

/// Animation frames per second.
pub const FPS: f32 = 30.0;
