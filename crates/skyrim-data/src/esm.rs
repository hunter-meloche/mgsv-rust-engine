//! Minimal reader for Skyrim plugin files (`.esm`/`.esp`, TES4 format): walks
//! every record and yields its type, form ID and editor ID (`EDID`), used to
//! find vanilla forms by name instead of hard-coding IDs.
//!
//! Layout: records and groups share a 24-byte header. A group is
//! `GRUP, size (incl. header), label, type, stamp, unknown` and contains
//! records and groups; a record is `type, data size, flags, form ID, stamp,
//! version, unknown` followed by its fields (`type, u16 size, data`; an
//! `XXXX` field gives the next field's u32 size). Records with flag
//! 0x00040000 store `u32 size` + zlib data.

use std::io::Read;

#[derive(Debug, thiserror::Error)]
pub enum EsmError {
    #[error("truncated at {0:#x}")]
    Truncated(usize),
    #[error("record {0:08X}: {1}")]
    Decompress(u32, String),
}

/// One record: its type (e.g. `ALCH`), form ID and editor ID.
#[derive(Clone, Debug)]
pub struct RecordInfo {
    pub kind: [u8; 4],
    pub form_id: u32,
    pub editor_id: String,
}

fn u32_at(b: &[u8], at: usize) -> Result<u32, EsmError> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
        .ok_or(EsmError::Truncated(at))
}

fn u16_at(b: &[u8], at: usize) -> Result<u16, EsmError> {
    b.get(at..at + 2)
        .map(|s| u16::from_le_bytes(s.try_into().unwrap()))
        .ok_or(EsmError::Truncated(at))
}

/// The `EDID` of a record's field data.
fn editor_id(fields: &[u8]) -> Result<String, EsmError> {
    let mut p = 0;
    let mut big: Option<usize> = None;
    while p + 6 <= fields.len() {
        let kind = &fields[p..p + 4];
        let size = big.take().unwrap_or(u16_at(fields, p + 4)? as usize);
        let data = fields.get(p + 6..p + 6 + size).ok_or(EsmError::Truncated(p))?;
        if kind == b"XXXX" {
            big = Some(u32_at(data, 0)? as usize);
        } else if kind == b"EDID" {
            return Ok(String::from_utf8_lossy(data).trim_end_matches('\0').to_string());
        }
        p += 6 + size;
    }
    Ok(String::new())
}

/// Every record of the given types (all types if `types` is empty).
pub fn records(data: &[u8], types: &[&[u8; 4]]) -> Result<Vec<RecordInfo>, EsmError> {
    let mut out = Vec::new();
    let mut p = 0;
    while p + 24 <= data.len() {
        let kind: [u8; 4] = data[p..p + 4].try_into().unwrap();
        if &kind == b"GRUP" {
            p += 24; // descend: the group's contents follow its header
            continue;
        }
        let size = u32_at(data, p + 4)? as usize;
        let flags = u32_at(data, p + 8)?;
        let form_id = u32_at(data, p + 12)?;
        let body = data.get(p + 24..p + 24 + size).ok_or(EsmError::Truncated(p))?;
        p += 24 + size;
        if !types.is_empty() && !types.iter().any(|t| **t == kind) {
            continue;
        }
        let fields = if flags & 0x0004_0000 != 0 && body.len() >= 4 {
            let mut v = Vec::with_capacity(u32_at(body, 0)? as usize);
            flate2::read::ZlibDecoder::new(&body[4..])
                .read_to_end(&mut v)
                .map_err(|e| EsmError::Decompress(form_id, e.to_string()))?;
            v
        } else {
            body.to_vec()
        };
        out.push(RecordInfo {
            kind,
            form_id,
            editor_id: editor_id(&fields)?,
        });
    }
    Ok(out)
}
