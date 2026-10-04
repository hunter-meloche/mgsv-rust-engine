//! `.mtar` motion archives: a table of animation clips (`.gani`).
//!
//! Layout as observed in the retail files (no community documentation):
//! * 32-byte header: magic `0x0C012B72`, entry count (u32), two u16 fields,
//!   flags (u32 at 0x10), offset of the end of the entry table (u32 at 0x14);
//! * `count` entries of 32 bytes from 0x20:
//!   `u64 path hash, u32 clip offset, u16 frames, u16 frames (repeat),
//!    u32 track count, u32 0, u32 secondary offset, u32 0`.
//!
//! The path hash uses the QAR layout (extension id in the top 13 bits), so
//! names resolve through the same path dictionary.

#[derive(Debug, thiserror::Error)]
pub enum MtarError {
    #[error("not an mtar file")]
    BadMagic,
    #[error("truncated at {0:#x}")]
    Truncated(usize),
}

pub const MAGIC: u32 = 0x0C01_2B72;

#[derive(Clone, Debug)]
pub struct MtarEntry {
    pub hash: u64,
    pub offset: u32,
    pub frames: u16,
    pub frames2: u16,
    pub tracks: u32,
    pub secondary_offset: u32,
}

#[derive(Clone, Debug)]
pub struct Mtar {
    pub flags: u32,
    pub table_end: u32,
    pub header_u16: [u16; 2],
    pub entries: Vec<MtarEntry>,
}

impl Mtar {
    pub fn parse(data: &[u8]) -> Result<Self, MtarError> {
        let u16_at = |at: usize| -> Result<u16, MtarError> {
            data.get(at..at + 2)
                .map(|s| u16::from_le_bytes(s.try_into().unwrap()))
                .ok_or(MtarError::Truncated(at))
        };
        let u32_at = |at: usize| -> Result<u32, MtarError> {
            data.get(at..at + 4)
                .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
                .ok_or(MtarError::Truncated(at))
        };
        let u64_at = |at: usize| -> Result<u64, MtarError> {
            data.get(at..at + 8)
                .map(|s| u64::from_le_bytes(s.try_into().unwrap()))
                .ok_or(MtarError::Truncated(at))
        };
        if u32_at(0)? != MAGIC {
            return Err(MtarError::BadMagic);
        }
        let count = u32_at(4)? as usize;
        let entries = (0..count)
            .map(|i| {
                let at = 0x20 + i * 32;
                Ok(MtarEntry {
                    hash: u64_at(at)?,
                    offset: u32_at(at + 8)?,
                    frames: u16_at(at + 12)?,
                    frames2: u16_at(at + 14)?,
                    tracks: u32_at(at + 16)?,
                    secondary_offset: u32_at(at + 24)?,
                })
            })
            .collect::<Result<_, MtarError>>()?;
        Ok(Self {
            flags: u32_at(0x10)?,
            table_end: u32_at(0x14)?,
            header_u16: [u16_at(8)?, u16_at(10)?],
            entries,
        })
    }
}
