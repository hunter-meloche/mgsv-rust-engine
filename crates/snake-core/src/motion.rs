//! Clip playback with crossfades and root motion.

use std::collections::HashMap;
use std::sync::Arc;

use fox_anim::{HumanRig, Quat, RigFrame, Vec3};
use fox_data::mtar::Clip;

use crate::FPS;

/// Decoded clips by short name (e.g. `snapnon_s_wk_lp_vr1_l`).
#[derive(Clone, Default)]
pub struct ClipLibrary {
    clips: HashMap<String, Arc<Clip>>,
}

impl ClipLibrary {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, name: impl Into<String>, clip: Clip) {
        self.clips.insert(name.into(), Arc::new(clip));
    }

    pub fn get(&self, name: &str) -> Option<Arc<Clip>> {
        self.clips.get(name).cloned()
    }

    pub fn len(&self) -> usize {
        self.clips.len()
    }

    pub fn is_empty(&self) -> bool {
        self.clips.is_empty()
    }
}

#[derive(Clone)]
struct Playing {
    name: String,
    clip: Arc<Clip>,
    /// Current time in frames.
    time: f32,
    looping: bool,
}

impl Playing {
    fn length(&self) -> f32 {
        self.clip.frame_count as f32
    }

    /// Advance by `frames`, returning the root motion covered (in the root's
    /// frame at the old time). Looping clips wrap; others clamp at the end.
    fn advance(&mut self, rig: &HumanRig, frames: f32) -> (Quat, Vec3) {
        let len = self.length();
        let mut total = (Quat::IDENTITY, Vec3::ZERO);
        let mut remaining = frames;
        while remaining > 1e-6 && len > 0.0 {
            let to = (self.time + remaining).min(len);
            total = compose(total, rig.root_motion(&self.clip, self.time, to));
            remaining -= to - self.time;
            self.time = to;
            if self.time >= len {
                if !self.looping {
                    break;
                }
                // The root is at the anchor (identity) at the last frame, so
                // the next cycle continues from there.
                self.time = 0.0;
            }
        }
        total
    }
}

/// Apply motion `b` after motion `a` (both expressed in their start frames).
fn compose(a: (Quat, Vec3), b: (Quat, Vec3)) -> (Quat, Vec3) {
    ((a.0 * b.0).normalize(), a.1 + a.0 * b.1)
}

/// Plays one clip at a time, crossfading from the previous one.
#[derive(Clone)]
pub struct MotionPlayer {
    current: Playing,
    /// Previous clip and the fade progress (elapsed, duration) in seconds.
    previous: Option<(Playing, f32, f32)>,
    /// Playback speed multiplier.
    pub speed: f32,
}

impl MotionPlayer {
    pub fn new(name: &str, clip: Arc<Clip>, looping: bool) -> Self {
        Self {
            current: Playing {
                name: name.into(),
                clip,
                time: 0.0,
                looping,
            },
            previous: None,
            speed: 1.0,
        }
    }

    /// Switch to a clip, crossfading over `fade` seconds.
    pub fn play(&mut self, name: &str, clip: Arc<Clip>, looping: bool, fade: f32) {
        let next = Playing {
            name: name.into(),
            clip,
            time: 0.0,
            looping,
        };
        let prev = std::mem::replace(&mut self.current, next);
        self.previous = (fade > 0.0).then_some((prev, 0.0, fade));
    }

    /// Like [`MotionPlayer::play`], starting `time` (frames) into the clip.
    pub fn play_at(&mut self, name: &str, clip: Arc<Clip>, looping: bool, fade: f32, time: f32) {
        self.play(name, clip, looping, fade);
        self.current.time = time;
    }

    pub fn current_name(&self) -> &str {
        &self.current.name
    }

    /// Normalised progress of the current clip (0..1).
    pub fn progress(&self) -> f32 {
        let len = self.current.length();
        if len > 0.0 {
            (self.current.time / len).min(1.0)
        } else {
            1.0
        }
    }

    /// A non-looping clip has reached its last frame.
    pub fn finished(&self) -> bool {
        !self.current.looping && self.current.time >= self.current.length()
    }

    fn fade_weight(&self) -> f32 {
        match &self.previous {
            Some((_, elapsed, dur)) => (elapsed / dur).clamp(0.0, 1.0),
            None => 1.0,
        }
    }

    /// Advance by `dt` seconds; returns the blended root motion.
    pub fn advance(&mut self, rig: &HumanRig, dt: f32) -> (Quat, Vec3) {
        let frames = dt * FPS * self.speed;
        let cur = self.current.advance(rig, frames);
        let Some((prev, elapsed, dur)) = &mut self.previous else {
            return cur;
        };
        let old = prev.advance(rig, frames);
        *elapsed += dt;
        let w = (*elapsed / *dur).clamp(0.0, 1.0);
        let done = *elapsed >= *dur;
        if done {
            self.previous = None;
        }
        (old.0.slerp(cur.0, w), old.1.lerp(cur.1, w))
    }

    /// The blended rig frame at the current time.
    pub fn frame(&self, rig: &HumanRig) -> Result<RigFrame, String> {
        let cur = rig.sample_frame(&self.current.clip, self.current.time)?;
        match &self.previous {
            Some((prev, _, _)) => {
                let old = rig.sample_frame(&prev.clip, prev.time)?;
                Ok(rig.blend(&old, &cur, self.fade_weight()))
            }
            None => Ok(cur),
        }
    }
}
