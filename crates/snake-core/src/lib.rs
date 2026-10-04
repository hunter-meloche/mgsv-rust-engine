//! Snake's player systems, independent of any renderer or host engine.
//!
//! * [`MotionPlayer`]: plays `.gani` clips on the human rig with crossfades
//!   and extracts root motion (looping clips wrap seamlessly).
//! * [`Controller`]: locomotion: stand / crouch / prone stances, walk, run
//!   and dash gaits, stance-change transitions, root-motion-driven movement.
//!
//! The host feeds an [`Input`] each tick and reads back the character's
//! world transform and skeleton [`fox_anim::Pose`].

pub mod controller;
pub mod motion;

pub use controller::{ClipNames, Controller, Gait, Input, Stance};
pub use motion::{ClipLibrary, MotionPlayer};

/// Animation frames per second.
pub const FPS: f32 = 30.0;
