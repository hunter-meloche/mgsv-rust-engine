//! Fox Engine animation playback.
//!
//! * [`Skeleton`]: bones of an `.fmdl` (Fox rest poses carry translations
//!   only; every bone's rest rotation is identity).
//! * [`sample`]: evaluate a decoded `.gani` channel at any time.
//! * [`HumanRig`]: the `human_finger.frig` rig solve that turns the 18 player
//!   track units into a skeleton pose (FK units plus arm/leg IK).
//! * [`SkinnedModel`]: linear-blend skinning of `.fmdl` meshes by a pose.

pub mod rig;
pub mod sample;
pub mod skeleton;
pub mod skin;

pub use glam;
pub use glam::{Quat, Vec3};
pub use rig::{HumanRig, Pose, RigFrame};
pub use skeleton::Skeleton;
pub use skin::SkinnedModel;
