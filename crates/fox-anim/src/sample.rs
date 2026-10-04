//! Sampling decoded `.gani` channels.

use fox_data::mtar::{Channel, SegmentType};
use glam::{Quat, Vec4};

/// Value of a channel at `frame` (fractional), interpolated linearly between
/// keyframes (spherically for rotations). Clamps outside the clip.
pub fn sample(ch: &Channel, frame: f32) -> [f32; 4] {
    let keys = &ch.keys;
    let Some(first) = keys.first() else {
        return [0.0, 0.0, 0.0, 1.0];
    };
    if keys.len() == 1 || frame <= first.frame as f32 {
        return first.value;
    }
    let i = keys.partition_point(|k| k.frame as f32 <= frame);
    if i >= keys.len() {
        return keys[keys.len() - 1].value;
    }
    let (a, b) = (&keys[i - 1], &keys[i]);
    let span = (b.frame - a.frame) as f32;
    let t = if span > 0.0 {
        (frame - a.frame as f32) / span
    } else {
        1.0
    };
    if matches!(ch.kind, SegmentType::Quat | SegmentType::QuatDiff) {
        Quat::from_array(a.value)
            .slerp(Quat::from_array(b.value), t)
            .to_array()
    } else {
        Vec4::from_array(a.value)
            .lerp(Vec4::from_array(b.value), t)
            .to_array()
    }
}
