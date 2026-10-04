//! The world the player moves and shoots in, behind a small trait so any
//! host (the test range in `mgsv play`, Skyrim through the bridge) can supply
//! its own collision.

use fox_anim::Vec3;

/// What a ray hit.
#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub point: Vec3,
    pub normal: Vec3,
    /// Host-defined id of the thing hit (e.g. a box index), if any.
    pub id: Option<usize>,
}

pub trait Collision {
    /// First hit along the segment `from -> to`.
    fn raycast(&self, from: Vec3, to: Vec3) -> Option<Hit>;

    /// Move an upright character capsule (feet at `feet`) horizontally by
    /// `delta`, sliding along obstacles. Returns the new feet position.
    fn slide(&self, feet: Vec3, delta: Vec3, _radius: f32, _height: f32) -> Vec3 {
        feet + delta
    }
}

/// An endless flat floor at `y = height`.
pub struct FlatGround {
    pub height: f32,
}

fn ray_plane(from: Vec3, to: Vec3, height: f32) -> Option<Hit> {
    let (a, b) = (from.y - height, to.y - height);
    if a >= 0.0 && b < 0.0 {
        let t = a / (a - b);
        Some(Hit {
            point: from.lerp(to, t),
            normal: Vec3::Y,
            id: None,
        })
    } else {
        None
    }
}

impl Collision for FlatGround {
    fn raycast(&self, from: Vec3, to: Vec3) -> Option<Hit> {
        ray_plane(from, to, self.height)
    }
}

/// Axis-aligned box.
#[derive(Clone, Copy, Debug)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub fn new(center: Vec3, size: Vec3) -> Self {
        Self {
            min: center - size * 0.5,
            max: center + size * 0.5,
        }
    }

    /// Segment intersection (slab method): parameter in 0..1 and normal.
    pub fn ray(&self, from: Vec3, to: Vec3) -> Option<(f32, Vec3)> {
        let d = to - from;
        let (mut t0, mut t1) = (0.0f32, 1.0f32);
        let mut normal = Vec3::ZERO;
        for axis in 0..3 {
            let (o, dd, lo, hi) = (from[axis], d[axis], self.min[axis], self.max[axis]);
            if dd.abs() < 1e-9 {
                if o < lo || o > hi {
                    return None;
                }
                continue;
            }
            let (mut a, mut b) = ((lo - o) / dd, (hi - o) / dd);
            let mut n = Vec3::ZERO;
            n[axis] = -dd.signum();
            if a > b {
                std::mem::swap(&mut a, &mut b);
            }
            if a > t0 {
                t0 = a;
                normal = n;
            }
            t1 = t1.min(b);
            if t0 > t1 {
                return None;
            }
        }
        (normal != Vec3::ZERO).then_some((t0, normal))
    }
}

/// Boxes on a flat floor: a reference world for tests and the test range.
#[derive(Clone, Debug, Default)]
pub struct BoxWorld {
    pub floor: f32,
    pub boxes: Vec<Aabb>,
}

impl Collision for BoxWorld {
    fn raycast(&self, from: Vec3, to: Vec3) -> Option<Hit> {
        let mut best: Option<(f32, Hit)> =
            ray_plane(from, to, self.floor).map(|h| ((h.point - from).length(), h));
        let len = (to - from).length();
        for (i, b) in self.boxes.iter().enumerate() {
            if let Some((t, n)) = b.ray(from, to) {
                let d = t * len;
                if best.as_ref().is_none_or(|(bd, _)| d < *bd) {
                    best = Some((
                        d,
                        Hit {
                            point: from.lerp(to, t),
                            normal: n,
                            id: Some(i),
                        },
                    ));
                }
            }
        }
        best.map(|(_, h)| h)
    }

    fn slide(&self, feet: Vec3, delta: Vec3, radius: f32, height: f32) -> Vec3 {
        let mut p = feet + Vec3::new(delta.x, 0.0, delta.z);
        // Push the capsule's circle (in XZ) out of every box it overlaps
        // vertically; a few passes settle corners.
        for _ in 0..4 {
            let mut moved = false;
            for b in &self.boxes {
                if b.max.y <= p.y + 0.05 || b.min.y >= p.y + height {
                    continue;
                }
                let closest = Vec3::new(
                    p.x.clamp(b.min.x, b.max.x),
                    0.0,
                    p.z.clamp(b.min.z, b.max.z),
                );
                let flat = Vec3::new(p.x, 0.0, p.z);
                let off = flat - closest;
                let dist = off.length();
                if dist < radius {
                    if dist > 1e-6 {
                        p += off / dist * (radius - dist);
                    } else {
                        // Centre inside the box: leave by the nearest face.
                        let exits = [
                            (p.x - b.min.x, Vec3::new(-1.0, 0.0, 0.0)),
                            (b.max.x - p.x, Vec3::X),
                            (p.z - b.min.z, Vec3::new(0.0, 0.0, -1.0)),
                            (b.max.z - p.z, Vec3::Z),
                        ];
                        let (d, n) = exits
                            .into_iter()
                            .min_by(|a, b| a.0.total_cmp(&b.0))
                            .unwrap();
                        p += n * (d + radius);
                    }
                    moved = true;
                }
            }
            if !moved {
                break;
            }
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wall() -> BoxWorld {
        BoxWorld {
            floor: 0.0,
            boxes: vec![Aabb::new(
                Vec3::new(0.0, 1.0, 2.0),
                Vec3::new(4.0, 2.0, 0.5),
            )],
        }
    }

    #[test]
    fn rays_hit_the_nearest_surface() {
        let w = wall();
        let h = w
            .raycast(Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 1.0, 10.0))
            .unwrap();
        assert!(
            (h.point.z - 1.75).abs() < 1e-4
                && h.normal == Vec3::new(0.0, 0.0, -1.0)
                && h.id == Some(0)
        );
        let h = w
            .raycast(Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, -1.0, 1.0))
            .unwrap();
        assert!(h.id.is_none() && h.point.y.abs() < 1e-4);
    }

    #[test]
    fn capsules_slide_along_walls() {
        let w = wall();
        // Walk diagonally into the wall: blocked in z, free in x.
        let p = w.slide(Vec3::new(0.0, 0.0, 1.2), Vec3::new(0.5, 0.0, 0.5), 0.3, 1.8);
        assert!((p.z - 1.45).abs() < 1e-3, "{p}");
        assert!((p.x - 0.5).abs() < 1e-3, "{p}");
    }
}
