//! Wwise sound bank objects (`HIRC`, bank version 88 as used by TPP) and a
//! small offline mixer for events whose actions play single sounds.
//!
//! Event names are not stored in the bank: an event's id is the FNV-1 hash of
//! its lower-case name ([`short_id`]). TPP's player weapon shots are named
//! `sfx_w_p_<sound set>` + `_m` (unsuppressed) or `_sup` + `_active` /
//! `_nonact`, the sound set coming from `EquipParameters`'
//! `receiverParamSetsSound` (e.g. `sfx_w_p_hg01_sup_active` for the WU
//! S.Pistol).
//!
//! What is read:
//! * Events (type 4): `u32` action count, action ids.
//! * Actions (type 3): `u16` type (high byte: 0x01 stop, 0x04 play, 0x08 set
//!   pitch, 0x0A set volume), `u32` target, `u8` is-bus, properties (`u8`
//!   count, ids, 4-byte values; 0x0E delay ms, 0x0F fade ms), ranged
//!   properties (`u8` count, 9 bytes each), then for set pitch / volume a
//!   fade curve, a value meaning and the `f32` value (cents / dB).
//! * Sounds (type 2): the source (`u32` plugin, `u32` stream type, `u32`
//!   wem id, `u32` file id, in-bank offset and size when the stream type is
//!   0, `u8` bits), then the node's base parameters: override-FX flag, FX
//!   count (and the FX entries), bus id, parent id, two priority flags and
//!   the properties (0x00 volume dB, 0x02 pitch cents).

use crate::sbp::{Pcm, SbpError, SoundBank};
use std::collections::HashMap;

/// Wwise short id of a name: FNV-1 (32-bit) of the lower-case name.
pub fn short_id(name: &str) -> u32 {
    name.to_ascii_lowercase()
        .bytes()
        .fold(2166136261u32, |h, b| h.wrapping_mul(16777619) ^ b as u32)
}

const TYPE_SOUND: u8 = 2;
const TYPE_ACTION: u8 = 3;
const TYPE_EVENT: u8 = 4;

/// The `HIRC` objects of a bank: id -> (type, payload).
pub struct Hirc<'a> {
    objects: HashMap<u32, (u8, &'a [u8])>,
}

/// One sound an event plays.
#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub sound: u32,
    pub wem: u32,
    pub volume_db: f32,
    pub pitch_cents: f32,
    pub delay: f32,
    /// Stop time and fade-out length (seconds), if the event stops it.
    pub stop: Option<(f32, f32)>,
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn f32_at(b: &[u8], at: usize) -> Option<f32> {
    u32_at(b, at).map(f32::from_bits)
}

/// Properties `(id, raw 4-byte value)` at `at`; returns them and the offset
/// after the list.
fn props(b: &[u8], at: usize) -> Option<(Vec<(u8, u32)>, usize)> {
    let n = *b.get(at)? as usize;
    let ids = b.get(at + 1..at + 1 + n)?;
    let vals = at + 1 + n;
    let list = ids
        .iter()
        .enumerate()
        .map(|(k, &id)| Some((id, u32_at(b, vals + 4 * k)?)))
        .collect::<Option<Vec<_>>>()?;
    Some((list, vals + 4 * n))
}

fn prop(list: &[(u8, u32)], id: u8) -> Option<u32> {
    list.iter().find(|p| p.0 == id).map(|p| p.1)
}

impl<'a> Hirc<'a> {
    /// The objects of the first bank in `data` (an `.sbp` or `.bnk`).
    pub fn parse(data: &'a [u8]) -> Result<Self, SbpError> {
        let at = data
            .windows(4)
            .position(|w| w == b"HIRC")
            .ok_or(SbpError::NoIndex)?;
        let count = u32_at(data, at + 8).ok_or(SbpError::NoIndex)? as usize;
        let mut objects = HashMap::with_capacity(count);
        let mut p = at + 12;
        for _ in 0..count {
            let (Some(&t), Some(size), Some(id)) = (data.get(p), u32_at(data, p + 1), u32_at(data, p + 5)) else {
                break;
            };
            let size = size as usize;
            let Some(body) = data.get(p + 9..p + 5 + size) else { break };
            objects.insert(id, (t, body));
            p += 5 + size;
        }
        Ok(Self { objects })
    }

    pub fn len(&self) -> usize {
        self.objects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    pub fn has_event(&self, id: u32) -> bool {
        matches!(self.objects.get(&id), Some((TYPE_EVENT, _)))
    }

    /// `(wem, volume dB, pitch cents)` of a Sound object.
    fn sound(&self, id: u32) -> Option<(u32, f32, f32)> {
        let &(TYPE_SOUND, b) = self.objects.get(&id)? else { return None };
        let wem = u32_at(b, 8)?;
        let stream = u32_at(b, 4)?;
        let mut at = if stream == 0 { 24 } else { 16 } + 1;
        at += 1; // override parent FX
        let fx = *b.get(at)? as usize;
        at += 1;
        if fx > 0 {
            at += 1 + fx * 7; // bypass bits, (index, id, share set, rendered)
        }
        at += 4 + 4 + 2; // override bus, parent, priority flags
        let (list, _) = props(b, at)?;
        let vol = prop(&list, 0x00).map(f32::from_bits).unwrap_or(0.0);
        let pitch = prop(&list, 0x02).map(f32::from_bits).unwrap_or(0.0);
        Some((wem, vol, pitch))
    }

    /// The sounds event `id` plays, with what its actions do to them.
    /// Actions on containers or buses are left out.
    pub fn event_layers(&self, id: u32) -> Option<Vec<Layer>> {
        let &(TYPE_EVENT, b) = self.objects.get(&id)? else { return None };
        let n = u32_at(b, 0)? as usize;
        let mut plays = Vec::new();
        let mut stops = HashMap::new();
        let mut pitch = HashMap::new();
        let mut volume = HashMap::new();
        for k in 0..n {
            let Some(&(TYPE_ACTION, a)) = self.objects.get(&u32_at(b, 4 + 4 * k)?) else { continue };
            let kind = u16::from_le_bytes([a[0], a[1]]) >> 8;
            let target = u32_at(a, 2)?;
            let (list, after) = props(a, 7)?;
            let ranged = *a.get(after)? as usize;
            let rest = after + 1 + ranged * 9;
            let ms = |id| prop(&list, id).unwrap_or(0) as f32 / 1000.0;
            match kind {
                0x04 => plays.push((target, ms(0x0E))),
                0x01 => {
                    stops.insert(target, (ms(0x0E), ms(0x0F)));
                }
                0x08 => {
                    pitch.insert(target, f32_at(a, rest + 2)?);
                }
                0x0A => {
                    volume.insert(target, f32_at(a, rest + 2)?);
                }
                _ => {}
            }
        }
        Some(
            plays
                .into_iter()
                .filter_map(|(sound, delay)| {
                    let (wem, vol, cents) = self.sound(sound)?;
                    Some(Layer {
                        sound,
                        wem,
                        volume_db: vol + volume.get(&sound).copied().unwrap_or(0.0),
                        pitch_cents: cents + pitch.get(&sound).copied().unwrap_or(0.0),
                        delay,
                        stop: stops.get(&sound).copied(),
                    })
                })
                .collect(),
        )
    }
}

/// Mix `layers` from `bank` into 48 kHz stereo: each layer resampled for its
/// pitch, delayed, scaled by its volume and faded out at its stop time; the
/// result is scaled down if it would clip (the game mixes with headroom).
pub fn mix(bank: &SoundBank, layers: &[Layer]) -> Result<Pcm, SbpError> {
    const RATE: u32 = 48000;
    let mut out: Vec<[f32; 2]> = Vec::new();
    for l in layers {
        let src = bank.pcm(l.wem)?;
        let ch = src.channels.max(1) as usize;
        let frames = src.samples.len() / ch;
        let at = |i: usize, c: usize| src.samples[i * ch + c.min(ch - 1)] as f32 / 32768.0;
        let gain = 10f32.powf(l.volume_db / 20.0);
        let step = src.sample_rate as f64 / RATE as f64 * 2f64.powf(l.pitch_cents as f64 / 1200.0);
        let start = (l.delay * RATE as f32) as usize;
        let mut pos = 0.0f64;
        let mut k = 0usize;
        while (pos as usize) + 1 < frames {
            let t = (start + k) as f32 / RATE as f32;
            let mut g = gain;
            if let Some((stop, fade)) = l.stop {
                if t >= stop + fade {
                    break;
                }
                if t > stop && fade > 0.0 {
                    g *= 1.0 - (t - stop) / fade;
                }
            }
            let i = pos as usize;
            let fr = (pos - i as f64) as f32;
            let idx = start + k;
            if out.len() <= idx {
                out.resize(idx + 1, [0.0; 2]);
            }
            for c in 0..2 {
                out[idx][c] += (at(i, c) * (1.0 - fr) + at(i + 1, c) * fr) * g;
            }
            k += 1;
            pos += step;
        }
    }
    let peak = out.iter().flatten().fold(0f32, |m, v| m.max(v.abs()));
    let scale = if peak > 0.9 { 0.9 / peak } else { 1.0 };
    Ok(Pcm {
        channels: 2,
        sample_rate: RATE,
        samples: out
            .iter()
            .flatten()
            .map(|v| (v * scale * 32767.0).clamp(-32768.0, 32767.0) as i16)
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_ids() {
        // Bank and event ids checked against TPP's common_bank_01.
        assert_eq!(short_id("common_bank_01"), 1296109055);
        assert_eq!(short_id("system_SE_bank_02"), 3482987163);
        assert_eq!(short_id("sfx_w_p_hg01_sup_active"), 4082435284);
    }

    #[test]
    fn reads_an_event() {
        // HIRC with one sound (wem 7, -6 dB, -100 cents), a play action
        // delayed 50 ms and an event.
        let mut sound = Vec::new();
        sound.extend_from_slice(&0x0001_0001u32.to_le_bytes()); // PCM
        sound.extend_from_slice(&0u32.to_le_bytes()); // in bank
        sound.extend_from_slice(&7u32.to_le_bytes()); // wem
        sound.extend_from_slice(&[0; 12]); // file id, offset, size
        sound.push(0); // bits
        sound.extend_from_slice(&[0, 0]); // override FX, FX count
        sound.extend_from_slice(&[0; 8]); // bus, parent
        sound.extend_from_slice(&[0, 0]); // priority flags
        sound.extend_from_slice(&[2, 0x00, 0x02]);
        sound.extend_from_slice(&(-6.0f32).to_le_bytes());
        sound.extend_from_slice(&(-100.0f32).to_le_bytes());
        let mut action = vec![0x03, 0x04];
        action.extend_from_slice(&100u32.to_le_bytes());
        action.extend_from_slice(&[0, 1, 0x0E]);
        action.extend_from_slice(&50u32.to_le_bytes());
        action.push(0);
        let mut event = 1u32.to_le_bytes().to_vec();
        event.extend_from_slice(&200u32.to_le_bytes());
        let mut hirc = b"HIRC\0\0\0\0".to_vec();
        hirc.extend_from_slice(&3u32.to_le_bytes());
        for (t, id, body) in [(2u8, 100u32, &sound), (3, 200, &action), (4, 300, &event)] {
            hirc.push(t);
            hirc.extend_from_slice(&(body.len() as u32 + 4).to_le_bytes());
            hirc.extend_from_slice(&id.to_le_bytes());
            hirc.extend_from_slice(body);
        }
        let h = Hirc::parse(&hirc).unwrap();
        assert!(h.has_event(300));
        let layers = h.event_layers(300).unwrap();
        assert_eq!(
            layers,
            vec![Layer {
                sound: 100,
                wem: 7,
                volume_db: -6.0,
                pitch_cents: -100.0,
                delay: 0.05,
                stop: None
            }]
        );
    }
}
