//! `.mtar` motion archives and the GANI2 animation clips inside them.
//!
//! Format knowledge follows the GPL-3.0 "Fox Engine MTAR Tools" Blender add-on
//! by mctrollin (binary templates for MTAR/GANI); this is an independent
//! implementation.
//!
//! * Header (32 bytes): version (201403250 for TPP), clip count, unit (track)
//!   count, segment count, shader node/unit counts, motion-point unit count,
//!   flags (`0x1000` = "mini" format used by TPP, `0x4000` = has skeleton
//!   list), offset of the shared common-info section.
//! * Clip table (32 bytes per clip in the mini format): path hash (QAR layout,
//!   extension `gani`), track data offset, sizes/offsets stored divided by 16,
//!   motion event offset.
//! * Common info: a linked list of nodes `(StrCode32 name, data size, next
//!   offset relative to the node, pad)`. `LayoutTrack` describes every track
//!   unit once (bone name hash, segment types); `SkeletonList` holds the bone
//!   names as NUL-separated strings.
//! * Each clip: a mini header (frame count, named float parameters, one flag
//!   byte per unit, then one 4-byte segment header per segment: bit size and a
//!   24-bit data offset relative to the header itself), followed by keyframes.
//! * Keyframes: an initial value, then — unless the unit is static —
//!   `(u8 frame delta, value)` pairs until the deltas reach the frame count.
//!   Rotations are bit-packed axis-angle quaternions (12/13/15-bit
//!   components); vectors and floats are byte-aligned 32-bit floats or a
//!   Fox-specific 16-bit float.

#[derive(Debug, thiserror::Error)]
pub enum MtarError {
    #[error("not a TPP mtar (version {0})")]
    BadVersion(u32),
    #[error("old-format (non-mini) mtar is not supported")]
    OldFormat,
    #[error("truncated at {0:#x}")]
    Truncated(usize),
    #[error("{0}")]
    Invalid(String),
}

type Result<T> = std::result::Result<T, MtarError>;

pub const VERSION_TPP: u32 = 201_403_250;
pub const FLAG_MINI: u16 = 0x1000;
pub const FLAG_HAS_SKEL_LIST: u16 = 0x4000;

const NODE_LAYOUT_TRACK: u32 = 1_337_830_127;
const NODE_SKELETON_LIST: u32 = 2_447_659_851;

/// Track unit flags (per clip).
pub const UNIT_LOOP: u8 = 0x1;
pub const UNIT_HERMITE: u8 = 0x2;
pub const UNIT_STATIC: u8 = 0x4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentType {
    Quat,
    Float,
    Vector2,
    Vector3,
    Vector4,
    QuatDiff,
    VectorDiff,
}

impl SegmentType {
    fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => Self::Quat,
            1 => Self::Float,
            2 => Self::Vector2,
            3 => Self::Vector3,
            4 => Self::Vector4,
            5 => Self::QuatDiff,
            6 => Self::VectorDiff,
            _ => return None,
        })
    }
    pub fn is_rotation(self) -> bool {
        matches!(self, Self::Quat | Self::QuatDiff)
    }
}

#[derive(Clone, Debug)]
pub struct MtarEntry {
    pub hash: u64,
    pub tracks_offset: u32,
    pub tracks_size: u32,
    pub motion_point_offset: u32,
    pub motion_point_size: u32,
    pub motion_events_offset: u32,
}

/// One track unit from the shared layout: usually one bone.
#[derive(Clone, Debug)]
pub struct LayoutUnit {
    /// `StrCode32` of the bone (or channel) name.
    pub name: u32,
    pub segments: Vec<SegmentType>,
}

#[derive(Clone, Debug)]
pub struct Mtar {
    pub version: u32,
    pub flags: u16,
    pub unit_count: u16,
    pub segment_count: u16,
    pub entries: Vec<MtarEntry>,
    pub layout: Vec<LayoutUnit>,
    /// Bone names from the skeleton list (when present).
    pub skeleton: Vec<String>,
}

/// A decoded keyframe: frame number (absolute) and value (1–4 components;
/// quaternions are `[x, y, z, w]`).
#[derive(Clone, Debug, PartialEq)]
pub struct Keyframe {
    pub frame: u32,
    pub value: [f32; 4],
}

#[derive(Clone, Debug)]
pub struct Channel {
    pub kind: SegmentType,
    pub bits: u8,
    pub keys: Vec<Keyframe>,
}

#[derive(Clone, Debug)]
pub struct Track {
    pub name: u32,
    pub flags: u8,
    pub channels: Vec<Channel>,
}

#[derive(Clone, Debug)]
pub struct Clip {
    pub frame_count: u32,
    /// Named float parameters (`StrCode32` name, value), e.g. slope data.
    pub params: Vec<(u32, f32)>,
    pub tracks: Vec<Track>,
    /// Motion events (markers the game reacts to, e.g. a magazine leaving or
    /// entering the gun during a reload).
    pub events: Vec<MotionEvent>,
}

/// A motion event: a named marker at one or more frames.
///
/// Layout (at the clip entry's motion event offset): `u32` section hash,
/// `u32` track count, `u32` track offsets (from the section). Each track:
/// `u32` name (`StrCode32`, e.g. `Normal`), `u32` event count, `u32` event
/// offsets (from the track). Each event: `u32` name (`StrCode32`), a flags
/// byte (low nibble: frame count; `0x40`: 16-bit frames, `0x80`: 8-bit), three
/// more bytes (parameter counts), then the frames, each followed by an
/// all-ones pad of the same width.
#[derive(Clone, Debug, PartialEq)]
pub struct MotionEvent {
    pub track: u32,
    pub name: u32,
    pub frames: Vec<u16>,
}

struct R<'a>(&'a [u8]);

impl R<'_> {
    fn b<const N: usize>(&self, at: usize) -> Result<[u8; N]> {
        self.0
            .get(at..at + N)
            .map(|s| s.try_into().unwrap())
            .ok_or(MtarError::Truncated(at))
    }
    fn u8(&self, at: usize) -> Result<u8> {
        Ok(self.b::<1>(at)?[0])
    }
    fn u16(&self, at: usize) -> Result<u16> {
        Ok(u16::from_le_bytes(self.b(at)?))
    }
    fn u32(&self, at: usize) -> Result<u32> {
        Ok(u32::from_le_bytes(self.b(at)?))
    }
    fn u64(&self, at: usize) -> Result<u64> {
        Ok(u64::from_le_bytes(self.b(at)?))
    }
    fn f32(&self, at: usize) -> Result<f32> {
        Ok(f32::from_le_bytes(self.b(at)?))
    }
    /// Little-endian bit read (up to 32 bits) starting at `bit` (absolute).
    fn bits(&self, bit: usize, n: u32) -> Result<u32> {
        if n == 0 {
            return Ok(0);
        }
        let byte = bit / 8;
        let shift = (bit % 8) as u32;
        let need = (shift + n).div_ceil(8) as usize;
        let mut raw = [0u8; 8];
        raw[..need].copy_from_slice(
            self.0
                .get(byte..byte + need)
                .ok_or(MtarError::Truncated(byte))?,
        );
        let v = u64::from_le_bytes(raw) >> shift;
        Ok((v & ((1u64 << n) - 1)) as u32)
    }
}

/// Fox animation half float (custom exponent bias, no infinities/NaNs).
pub fn anim_half(v: u16) -> f32 {
    let v = v as u32;
    let mut bits = v & 0x7c00;
    if bits > 0 {
        bits = (bits + 0x1dc00) << 13;
    }
    bits |= ((v & 0x8000) << 16) | ((v & 0x3ff) << 13);
    f32::from_bits(bits)
}

/// Decode a bit-packed axis-angle quaternion: angle and two axis components
/// at `n` bits each, the third axis component derived, then three sign bits.
fn read_quat(r: &R, bit: &mut usize, n: u32) -> Result<[f32; 4]> {
    let mut take = |k: u32| -> Result<u32> {
        let v = r.bits(*bit, k)?;
        *bit += k as usize;
        Ok(v)
    };
    let (a, b, c) = (take(n)?, take(n)?, take(n)?);
    let (sx, sy, sz) = (take(1)?, take(1)?, take(1)?);
    let d = ((1u32 << n) - 1) as f32;
    let half_theta = a as f32 / d * std::f32::consts::FRAC_PI_2;
    let (mut x, mut y) = (b as f32 / d, c as f32 / d);
    let mut z = 1.0 - x - y;
    let len = (x * x + y * y + z * z).sqrt();
    if len > 0.0 {
        x /= len;
        y /= len;
        z /= len;
    }
    if sx != 0 {
        x = -x;
    }
    if sy != 0 {
        y = -y;
    }
    if sz != 0 {
        z = -z;
    }
    let (s, cth) = half_theta.sin_cos();
    Ok([s * x, s * y, s * z, cth])
}

fn read_scalar(r: &R, at: &mut usize, bits: u8) -> Result<f32> {
    match bits {
        16 => {
            let v = anim_half(r.u16(*at)?);
            *at += 2;
            Ok(v)
        }
        32 => {
            let v = r.f32(*at)?;
            *at += 4;
            Ok(v)
        }
        0 => Ok(0.0),
        other => Err(MtarError::Invalid(format!(
            "unsupported component size {other}"
        ))),
    }
}

fn read_value(r: &R, kind: SegmentType, bits: u8, at: &mut usize) -> Result<[f32; 4]> {
    let n = match kind {
        SegmentType::Float => 1,
        SegmentType::Vector2 => 2,
        SegmentType::Vector3 | SegmentType::VectorDiff => 3,
        SegmentType::Vector4 => 4,
        _ => unreachable!("rotations are bit-packed"),
    };
    let mut v = [0f32; 4];
    for c in v.iter_mut().take(n) {
        *c = read_scalar(r, at, bits)?;
    }
    Ok(v)
}

/// A zero frame delta is legal (a repeated key) but would let corrupt data
/// loop forever, so cap the key count.
fn guard(keys: &[Keyframe], frames: u32) -> Result<()> {
    if keys.len() > frames as usize * 4 + 16 {
        return Err(MtarError::Invalid(
            "too many keyframes for the clip length".into(),
        ));
    }
    Ok(())
}

/// Decode one channel's keyframes starting at byte `at`.
fn read_channel(
    r: &R,
    kind: SegmentType,
    bits: u8,
    flags: u8,
    frames: u32,
    at: usize,
) -> Result<Vec<Keyframe>> {
    let animated = flags & UNIT_STATIC == 0;
    let mut keys = Vec::new();
    let mut frame = 0u32;
    if kind.is_rotation() {
        if !matches!(bits, 12 | 13 | 15) {
            return Err(MtarError::Invalid(format!(
                "quaternion with {bits}-bit components"
            )));
        }
        let mut bit = at * 8;
        keys.push(Keyframe {
            frame,
            value: read_quat(r, &mut bit, bits as u32)?,
        });
        while animated && frame < frames {
            let delta = r.bits(bit, 8)?;
            bit += 8;
            frame += delta;
            guard(&keys, frames)?;
            let mut q = read_quat(r, &mut bit, bits as u32)?;
            // Keep consecutive keys in the same hemisphere for interpolation.
            let prev = keys.last().unwrap().value;
            if prev.iter().zip(&q).map(|(a, b)| a * b).sum::<f32>() < 0.0 {
                q = q.map(|c| -c);
            }
            keys.push(Keyframe { frame, value: q });
        }
    } else {
        let mut p = at;
        keys.push(Keyframe {
            frame,
            value: read_value(r, kind, bits, &mut p)?,
        });
        while animated && frame < frames {
            let delta = r.u8(p)? as u32;
            p += 1;
            frame += delta;
            guard(&keys, frames)?;
            keys.push(Keyframe {
                frame,
                value: read_value(r, kind, bits, &mut p)?,
            });
        }
    }
    if animated && frame != frames {
        return Err(MtarError::Invalid(format!(
            "frame deltas sum to {frame}, clip has {frames}"
        )));
    }
    Ok(keys)
}

impl Mtar {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let r = R(data);
        let version = r.u32(0)?;
        if version != VERSION_TPP {
            return Err(MtarError::BadVersion(version));
        }
        let count = r.u32(4)? as usize;
        let unit_count = r.u16(8)?;
        let segment_count = r.u16(10)?;
        let flags = r.u16(18)?;
        let common = r.u32(20)? as usize;
        if flags & FLAG_MINI == 0 {
            return Err(MtarError::OldFormat);
        }

        let entries = (0..count)
            .map(|i| {
                let at = 32 + i * 32;
                Ok(MtarEntry {
                    hash: r.u64(at)?,
                    tracks_offset: r.u32(at + 8)?,
                    tracks_size: r.u16(at + 12)? as u32 * 16,
                    motion_point_offset: r.u16(at + 14)? as u32 * 16,
                    motion_point_size: r.u16(at + 16)? as u32 * 16,
                    motion_events_offset: r.u32(at + 24)?,
                })
            })
            .collect::<Result<_>>()?;

        let mut layout = Vec::new();
        let mut skeleton = Vec::new();
        let mut node = common;
        for _ in 0..64 {
            let (name, size, next) = (
                r.u32(node)?,
                r.u32(node + 4)? as usize,
                r.u32(node + 8)? as usize,
            );
            let body = node + 16;
            match name {
                NODE_LAYOUT_TRACK => {
                    let units = r.u32(body)? as usize;
                    for u in 0..units {
                        let ua = body + r.u32(body + 20 + u * 4)? as usize;
                        let seg_n = r.u8(ua + 4)? as usize;
                        let segments = (0..seg_n)
                            .map(|s| {
                                let t = r.u8(ua + 8 + s * 8 + 6)? & 0x0f;
                                SegmentType::from_u8(t)
                                    .ok_or_else(|| MtarError::Invalid(format!("segment type {t}")))
                            })
                            .collect::<Result<_>>()?;
                        layout.push(LayoutUnit {
                            name: r.u32(ua)?,
                            segments,
                        });
                    }
                }
                NODE_SKELETON_LIST => {
                    let bytes = data
                        .get(body..body + size)
                        .ok_or(MtarError::Truncated(body))?;
                    skeleton = bytes
                        .split(|&b| b == 0)
                        .filter(|s| !s.is_empty())
                        .map(|s| String::from_utf8_lossy(s).into_owned())
                        .collect();
                }
                _ => {}
            }
            if next == 0 {
                break;
            }
            node += next;
        }
        if layout.len() != unit_count as usize {
            return Err(MtarError::Invalid(format!(
                "layout has {} units, header says {unit_count}",
                layout.len()
            )));
        }
        Ok(Self {
            version,
            flags,
            unit_count,
            segment_count,
            entries,
            layout,
            skeleton,
        })
    }

    /// Decode clip `index`.
    pub fn clip(&self, data: &[u8], index: usize) -> Result<Clip> {
        let r = R(data);
        let e = self
            .entries
            .get(index)
            .ok_or_else(|| MtarError::Invalid(format!("no clip {index}")))?;
        let at = e.tracks_offset as usize;
        let frame_count = r.u32(at)?;
        let param_count = r.u8(at + 5)? as usize;
        let params = (0..param_count)
            .map(|k| Ok((r.u32(at + 8 + k * 8)?, r.f32(at + 12 + k * 8)?)))
            .collect::<Result<_>>()?;
        let units = self.layout.len();
        let flags_at = at + 8 + param_count * 8;
        let seg_base = (flags_at + units + 3) & !3;

        let mut tracks = Vec::with_capacity(units);
        let mut seg = 0usize;
        for (u, unit) in self.layout.iter().enumerate() {
            let flags = r.u8(flags_at + u)?;
            let mut channels = Vec::with_capacity(unit.segments.len());
            for &kind in &unit.segments {
                let h = seg_base + seg * 4;
                let header = r.u32(h)?;
                let bits = (header & 0xff) as u8;
                let data_at = h + (header >> 8) as usize;
                let keys =
                    read_channel(&r, kind, bits, flags, frame_count, data_at).map_err(|err| {
                        MtarError::Invalid(format!(
                            "unit {u} ({:#x}) segment {seg}: {err}",
                            unit.name
                        ))
                    })?;
                channels.push(Channel { kind, bits, keys });
                seg += 1;
            }
            tracks.push(Track {
                name: unit.name,
                flags,
                channels,
            });
        }
        // Best effort: a few NPC clips use another event layout; their
        // animation still loads.
        let events = if e.motion_events_offset == 0 {
            Vec::new()
        } else {
            read_events(&r, e.motion_events_offset as usize).unwrap_or_default()
        };
        Ok(Clip {
            frame_count,
            params,
            tracks,
            events,
        })
    }
}

fn read_events(r: &R, at: usize) -> Result<Vec<MotionEvent>> {
    let tracks = r.u32(at + 4)? as usize;
    if tracks > 64 {
        return Err(MtarError::Invalid(format!("{tracks} motion event tracks")));
    }
    let mut out = Vec::new();
    for t in 0..tracks {
        let ta = at + r.u32(at + 8 + 4 * t)? as usize;
        let track = r.u32(ta)?;
        let n = r.u32(ta + 4)? as usize;
        if n > 256 {
            return Err(MtarError::Invalid(format!("{n} motion events")));
        }
        for k in 0..n {
            let ea = ta + r.u32(ta + 8 + 4 * k)? as usize;
            let name = r.u32(ea)?;
            let flags = r.u8(ea + 4)?;
            let count = (flags & 0x0f) as usize;
            let wide = flags & 0x40 != 0;
            let frames = (0..count)
                .map(|f| {
                    if wide {
                        r.u16(ea + 8 + f * 4)
                    } else {
                        r.u8(ea + 8 + f * 2).map(u16::from)
                    }
                })
                .collect::<Result<_>>()?;
            out.push(MotionEvent { track, name, frames });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motion_events() {
        // From TPP's pl_rcvr_ar00.mtar: one track ("Normal") with an 8-bit
        // point event (frame 24) and a 16-bit range event (frames 28-63).
        let mut d = vec![0u8; 0];
        d.extend_from_slice(&0x0bfe_2cf6u32.to_le_bytes());
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&12u32.to_le_bytes());
        // track at 12
        d.extend_from_slice(&0xc8f4_fcb6u32.to_le_bytes());
        d.extend_from_slice(&2u32.to_le_bytes());
        d.extend_from_slice(&16u32.to_le_bytes());
        d.extend_from_slice(&28u32.to_le_bytes());
        // event at 12 + 16
        d.extend_from_slice(&[0xb1, 0xde, 0x0d, 0xec, 0x81, 0, 0, 1, 0x18, 0xff, 0, 0]);
        // event at 12 + 28
        d.extend_from_slice(&[0x3a, 0xd5, 0xe0, 0xff, 0x42, 0, 0, 1, 0x1c, 0, 0xff, 0xff, 0x3f, 0, 0xff, 0xff]);
        let ev = read_events(&R(&d), 0).unwrap();
        assert_eq!(ev.len(), 2);
        assert_eq!((ev[0].name, ev[0].frames.clone()), (0xec0d_deb1, vec![24]));
        assert_eq!((ev[1].name, ev[1].frames.clone()), (0xffe0_d53a, vec![28, 63]));
        assert_eq!(ev[1].track, 0xc8f4_fcb6);
    }

    #[test]
    fn anim_half_values() {
        assert_eq!(anim_half(0), 0.0);
        // Sign bit only flips the sign.
        assert_eq!(anim_half(0x8000 | 0x3c00), -anim_half(0x3c00));
        assert!(anim_half(0x3c00) > 0.0);
    }

    #[test]
    fn identity_quaternion() {
        // angle 0 → w = 1 regardless of axis.
        let data = [0u8; 8];
        let mut bit = 0;
        let q = read_quat(&R(&data), &mut bit, 15).unwrap();
        assert_eq!(bit, 48);
        assert!((q[3] - 1.0).abs() < 1e-6);
    }
}
