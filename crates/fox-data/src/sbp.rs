//! `.sbp` sound packages: an `SBPL` header pointing at a Wwise sound bank
//! (`bnk`) and its streamed data (`stp`). Only what is needed to pull the
//! in-memory sounds out of the bank is read: the `DIDX` index (u32 id,
//! offset, size) and the `DATA` block holding RIFF `.wem` files.

#[derive(Debug, thiserror::Error)]
pub enum SbpError {
    #[error("no sound bank index in this file")]
    NoIndex,
    #[error("sound {0} not in the bank")]
    Missing(u32),
    #[error("sound {0}: {1}")]
    Format(u32, String),
}

/// One decoded sound: interleaved 16-bit PCM.
#[derive(Clone, Debug)]
pub struct Pcm {
    pub channels: u16,
    pub sample_rate: u32,
    pub samples: Vec<i16>,
}

impl Pcm {
    /// A standard 16-bit PCM `.wav` file.
    pub fn to_wav(&self) -> Vec<u8> {
        let data_len = (self.samples.len() * 2) as u32;
        let block = self.channels as u32 * 2;
        let mut w = Vec::with_capacity(44 + data_len as usize);
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data_len).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes()); // PCM
        w.extend_from_slice(&self.channels.to_le_bytes());
        w.extend_from_slice(&self.sample_rate.to_le_bytes());
        w.extend_from_slice(&(self.sample_rate * block).to_le_bytes());
        w.extend_from_slice(&(block as u16).to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&data_len.to_le_bytes());
        for s in &self.samples {
            w.extend_from_slice(&s.to_le_bytes());
        }
        w
    }
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay.get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// The in-memory sounds of the first bank in an `.sbp` (or a bare `.bnk`).
pub struct SoundBank<'a> {
    data: &'a [u8],
    /// id -> (offset, size) within `data[data_start..]`
    index: Vec<(u32, u32, u32)>,
    data_start: usize,
}

impl<'a> SoundBank<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, SbpError> {
        let didx = find(data, b"DIDX", 0).ok_or(SbpError::NoIndex)?;
        let len = u32_at(data, didx + 4).ok_or(SbpError::NoIndex)? as usize;
        let index = (0..len / 12)
            .filter_map(|k| {
                let at = didx + 8 + 12 * k;
                Some((u32_at(data, at)?, u32_at(data, at + 4)?, u32_at(data, at + 8)?))
            })
            .collect();
        let data_block = find(data, b"DATA", didx + 8 + len).ok_or(SbpError::NoIndex)?;
        Ok(Self {
            data,
            index,
            data_start: data_block + 8,
        })
    }

    /// Ids of the in-memory sounds.
    pub fn ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.index.iter().map(|e| e.0)
    }

    /// The raw `.wem` file of sound `id`.
    pub fn wem(&self, id: u32) -> Result<&'a [u8], SbpError> {
        let &(_, off, size) = self
            .index
            .iter()
            .find(|e| e.0 == id)
            .ok_or(SbpError::Missing(id))?;
        let start = self.data_start + off as usize;
        self.data
            .get(start..start + size as usize)
            .ok_or_else(|| SbpError::Format(id, "truncated".into()))
    }

    /// Sound `id` decoded, if it is stored as 16-bit PCM (most TPP sound
    /// effects are) or Wwise IMA ADPCM; Vorbis ones are not supported.
    pub fn pcm(&self, id: u32) -> Result<Pcm, SbpError> {
        let w = self.wem(id)?;
        let bad = |m: &str| SbpError::Format(id, m.to_string());
        if w.get(0..4) != Some(b"RIFF") || w.get(8..12) != Some(b"WAVE") {
            return Err(bad("not a RIFF/WAVE file"));
        }
        let fmt = find(w, b"fmt ", 12).ok_or_else(|| bad("no fmt chunk"))?;
        let tag = u16_at(w, fmt + 8).ok_or_else(|| bad("short fmt"))?;
        let channels = u16_at(w, fmt + 10).ok_or_else(|| bad("short fmt"))?;
        let sample_rate = u32_at(w, fmt + 12).ok_or_else(|| bad("short fmt"))?;
        let bits = u16_at(w, fmt + 22).ok_or_else(|| bad("short fmt"))?;
        let adpcm = tag == 2 && bits == 4;
        if !((tag == 1 || tag == 0xFFFE) && bits == 16) && !adpcm {
            return Err(bad(&format!("codec {tag:#x}/{bits} bit is not 16-bit PCM or IMA ADPCM")));
        }
        let data = find(w, b"data", fmt).ok_or_else(|| bad("no data chunk"))?;
        let len = u32_at(w, data + 4).ok_or_else(|| bad("short data"))? as usize;
        let bytes = w.get(data + 8..data + 8 + len).ok_or_else(|| bad("truncated data"))?;
        if adpcm {
            let block = u16_at(w, fmt + 20).ok_or_else(|| bad("short fmt"))? as usize;
            return Ok(Pcm {
                channels,
                sample_rate,
                samples: decode_wwise_ima(bytes, channels.max(1) as usize, block),
            });
        }
        Ok(Pcm {
            channels,
            sample_rate,
            samples: bytes
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                .collect(),
        })
    }
}

const IMA_STEPS: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107,
    118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963,
    1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894,
    6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794,
    32767,
];
const IMA_INDEX: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];

/// Wwise IMA ADPCM: blocks of `block` bytes holding one sub-block per
/// channel (`block / channels` bytes: i16 history sample, u8 step index,
/// u8 padding, then 4-bit codes low nibble first), 64 samples each. The
/// header sample only seeds the predictor.
fn decode_wwise_ima(bytes: &[u8], channels: usize, block: usize) -> Vec<i16> {
    let sub = block / channels;
    if sub <= 4 {
        return Vec::new();
    }
    let per_block = (sub - 4) * 2;
    let mut out = Vec::new();
    for frame in bytes.chunks_exact(block) {
        let mut decoded = vec![0i16; per_block * channels];
        for ch in 0..channels {
            let b = &frame[ch * sub..(ch + 1) * sub];
            let mut sample = i16::from_le_bytes([b[0], b[1]]) as i32;
            let mut index = (b[2] as i32).clamp(0, 88);
            for (k, byte) in b[4..].iter().enumerate() {
                for (half, code) in [byte & 0x0F, byte >> 4].into_iter().enumerate() {
                    let step = IMA_STEPS[index as usize];
                    let mut diff = step >> 3;
                    if code & 1 != 0 {
                        diff += step >> 2;
                    }
                    if code & 2 != 0 {
                        diff += step >> 1;
                    }
                    if code & 4 != 0 {
                        diff += step;
                    }
                    sample = if code & 8 != 0 { sample - diff } else { sample + diff }.clamp(-32768, 32767);
                    index = (index + IMA_INDEX[(code & 7) as usize]).clamp(0, 88);
                    decoded[(k * 2 + half) * channels + ch] = sample as i16;
                }
            }
        }
        out.extend_from_slice(&decoded);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ima_block_decodes_64_samples() {
        // One mono block: history 0, step index 0, codes 0x7 (largest
        // positive step) then 0xF (largest negative).
        let mut block = vec![0u8, 0, 0, 0];
        block.extend(std::iter::repeat(0xF7).take(32));
        let out = decode_wwise_ima(&block, 1, 36);
        assert_eq!(out.len(), 64);
        assert!(out[0] > 0, "first code steps up: {}", out[0]);
        assert!(out[1] < out[0], "second code steps down");
    }

    #[test]
    fn wav_round_trip_header() {
        let pcm = Pcm {
            channels: 1,
            sample_rate: 44100,
            samples: vec![0, 1000, -1000],
        };
        let wav = pcm.to_wav();
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(wav.len(), 44 + 6);
        assert_eq!(u32_at(&wav, 24), Some(44100));
    }
}
