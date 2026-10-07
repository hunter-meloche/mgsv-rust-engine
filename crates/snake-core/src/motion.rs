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
    /// The motion events whose frames are passed (from inclusive, to
    /// exclusive) go to `crossed` (event name hashes).
    fn advance(&mut self, rig: &HumanRig, frames: f32, crossed: &mut Vec<u32>) -> (Quat, Vec3) {
        let len = self.length();
        let mut total = (Quat::IDENTITY, Vec3::ZERO);
        let mut remaining = frames;
        while remaining > 1e-6 && len > 0.0 {
            let to = (self.time + remaining).min(len);
            for e in &self.clip.events {
                for &f in &e.frames {
                    let f = f as f32;
                    if f >= self.time && f < to {
                        crossed.push(e.name);
                    }
                }
            }
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
    /// The current clip's fade in (elapsed, duration; seconds), if fading.
    fade: Option<(f32, f32)>,
    /// Clips still showing beneath it, oldest first, each with its own fade
    /// in: a clip started while another was still fading in fades from the
    /// whole blend, not just from the clip before it (which snapped).
    previous: Vec<(Playing, Option<(f32, f32)>)>,
    /// Playback speed multiplier.
    pub speed: f32,
    /// Motion events of the current clip passed since the last
    /// [`MotionPlayer::take_crossed`].
    crossed: Vec<u32>,
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
            fade: None,
            previous: Vec::new(),
            speed: 1.0,
            crossed: Vec::new(),
        }
    }

    /// Motion events (name hashes) the current clip passed since the last
    /// call.
    pub fn take_crossed(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.crossed)
    }

    /// Whether the current clip has a motion event named `name`.
    pub fn current_has_event(&self, name: u32) -> bool {
        self.current.clip.events.iter().any(|e| e.name == name && !e.frames.is_empty())
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
        if fade > 0.0 {
            let prev_fade = self.fade.take();
            self.previous.push((prev, prev_fade));
            // A few layers at most: the oldest has (nearly) faded out.
            if self.previous.len() > 3 {
                self.previous.remove(0);
            }
            self.fade = Some((0.0, fade));
        } else {
            self.previous.clear();
            self.fade = None;
        }
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

    fn weight(fade: Option<(f32, f32)>) -> f32 {
        match fade {
            Some((elapsed, dur)) => (elapsed / dur).clamp(0.0, 1.0),
            None => 1.0,
        }
    }

    /// Advance by `dt` seconds; returns the blended root motion.
    pub fn advance(&mut self, rig: &HumanRig, dt: f32) -> (Quat, Vec3) {
        let frames = dt * FPS * self.speed;
        let cur = self.current.advance(rig, frames, &mut self.crossed);
        if self.previous.is_empty() {
            self.fade = None;
            return cur;
        }
        // Root motion blended like the pose: oldest first.
        let mut acc: Option<(Quat, Vec3)> = None;
        for (layer, fade) in self.previous.iter_mut() {
            let m = layer.advance(rig, frames, &mut Vec::new());
            let w = Self::weight(*fade);
            acc = Some(match acc {
                Some(a) => (a.0.slerp(m.0, w), a.1.lerp(m.1, w)),
                None => m,
            });
            if let Some((elapsed, _)) = fade.as_mut() {
                *elapsed += dt;
            }
        }
        let w = Self::weight(self.fade);
        let out = match acc {
            Some(a) => (a.0.slerp(cur.0, w), a.1.lerp(cur.1, w)),
            None => cur,
        };
        if let Some((elapsed, _)) = self.fade.as_mut() {
            *elapsed += dt;
        }
        // A layer fully faded in hides everything older.
        if Self::weight(self.fade) >= 1.0 {
            self.previous.clear();
            self.fade = None;
        } else if let Some(k) = self.previous.iter().rposition(|(_, f)| Self::weight(*f) >= 1.0) {
            self.previous.drain(..k);
            self.previous[0].1 = None;
        }
        out
    }

    /// The blended rig frame at the current time.
    pub fn frame(&self, rig: &HumanRig) -> Result<RigFrame, String> {
        let cur = rig.sample_frame(&self.current.clip, self.current.time)?;
        let mut acc: Option<RigFrame> = None;
        for (layer, fade) in &self.previous {
            let f = rig.sample_frame(&layer.clip, layer.time)?;
            acc = Some(match acc {
                Some(a) => rig.blend(&a, &f, Self::weight(*fade)),
                None => f,
            });
        }
        Ok(match acc {
            Some(a) => rig.blend(&a, &cur, Self::weight(self.fade)),
            None => cur,
        })
    }
}
