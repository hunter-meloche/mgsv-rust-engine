//! Bethesda archives (`.bsa`) as used by Skyrim Special Edition (v105,
//! LZ4-frame compression; v104 zlib archives are read too).
//!
//! Layout: 36-byte header (`BSA\0`, version, folder-record offset, archive
//! flags, folder / file counts, name lengths, file flags), folder records
//! (`u64 hash, u32 count, u32 pad, u64 offset` in v105), per-folder blocks
//! (`bzstring` folder name when flag 0x1, then `u64 hash, u32 size, u32
//! offset` per file), then the file-name block (NUL-terminated, in order).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum BsaError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a BSA (or unsupported version {0})")]
    Format(u32),
    #[error("truncated at {0:#x}")]
    Truncated(usize),
    #[error("decompression failed: {0}")]
    Decompress(String),
}

const FLAG_DIR_NAMES: u32 = 0x1;
const FLAG_FILE_NAMES: u32 = 0x2;
const FLAG_COMPRESSED: u32 = 0x4;
const FLAG_EMBED_NAMES: u32 = 0x100;
const SIZE_COMPRESS_TOGGLE: u32 = 0x4000_0000;

#[derive(Clone, Debug)]
pub struct BsaEntry {
    /// Lower-case path with backslashes, e.g. `meshes\actors\...\skeleton.nif`.
    pub path: String,
    size: u32,
    offset: u32,
}

pub struct Bsa {
    file: PathBuf,
    version: u32,
    flags: u32,
    pub entries: Vec<BsaEntry>,
}

fn u32_at(b: &[u8], at: usize) -> Result<u32, BsaError> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
        .ok_or(BsaError::Truncated(at))
}

impl Bsa {
    pub fn open(path: &Path) -> Result<Self, BsaError> {
        let mut f = File::open(path)?;
        let mut header = [0u8; 36];
        f.read_exact(&mut header)?;
        let version = u32_at(&header, 4)?;
        if &header[0..4] != b"BSA\0" || !matches!(version, 104 | 105) {
            return Err(BsaError::Format(version));
        }
        let folder_offset = u32_at(&header, 8)? as u64;
        let flags = u32_at(&header, 12)?;
        let folders = u32_at(&header, 16)? as usize;
        let files = u32_at(&header, 20)? as usize;
        let folder_names_len = u32_at(&header, 24)? as usize;
        let file_names_len = u32_at(&header, 28)? as usize;
        let record = if version == 105 { 24 } else { 16 };
        // Everything up to the end of the file-name block.
        let dir_block = folders * record;
        let blocks = if flags & FLAG_DIR_NAMES != 0 {
            folders + folder_names_len
        } else {
            0
        } + files * 16;
        let meta_len = dir_block + blocks + file_names_len;
        let mut meta = vec![0u8; meta_len];
        f.seek(SeekFrom::Start(folder_offset))?;
        f.read_exact(&mut meta)?;

        let mut counts = Vec::with_capacity(folders);
        for i in 0..folders {
            counts.push(u32_at(&meta, i * record + 8)? as usize);
        }
        let mut p = dir_block;
        let mut raw = Vec::with_capacity(files);
        for &count in &counts {
            let mut folder = String::new();
            if flags & FLAG_DIR_NAMES != 0 {
                let len = *meta.get(p).ok_or(BsaError::Truncated(p))? as usize;
                let name = meta.get(p + 1..p + len).ok_or(BsaError::Truncated(p))?;
                folder = String::from_utf8_lossy(name)
                    .trim_end_matches('\0')
                    .to_string();
                p += 1 + len;
            }
            for _ in 0..count {
                raw.push((
                    folder.clone(),
                    u32_at(&meta, p + 8)?,
                    u32_at(&meta, p + 12)?,
                ));
                p += 16;
            }
        }
        let names: Vec<&[u8]> = if flags & FLAG_FILE_NAMES != 0 {
            meta[p..].split(|&b| b == 0).collect()
        } else {
            Vec::new()
        };
        let entries = raw
            .into_iter()
            .enumerate()
            .map(|(i, (folder, size, offset))| {
                let name = names
                    .get(i)
                    .map(|n| String::from_utf8_lossy(n).into_owned())
                    .unwrap_or_default();
                BsaEntry {
                    path: format!("{folder}\\{name}").to_ascii_lowercase(),
                    size,
                    offset,
                }
            })
            .collect();
        Ok(Self {
            file: path.to_path_buf(),
            version,
            flags,
            entries,
        })
    }

    pub fn find(&self, path: &str) -> Option<&BsaEntry> {
        let want = path.replace('/', "\\").to_ascii_lowercase();
        self.entries.iter().find(|e| e.path == want)
    }

    /// Read (and decompress) one file.
    pub fn read(&self, e: &BsaEntry) -> Result<Vec<u8>, BsaError> {
        let mut f = File::open(&self.file)?;
        let mut size = e.size & !SIZE_COMPRESS_TOGGLE & 0x3FFF_FFFF;
        let compressed = (self.flags & FLAG_COMPRESSED != 0) ^ (e.size & SIZE_COMPRESS_TOGGLE != 0);
        f.seek(SeekFrom::Start(e.offset as u64))?;
        if self.flags & FLAG_EMBED_NAMES != 0 {
            let mut len = [0u8; 1];
            f.read_exact(&mut len)?;
            f.seek(SeekFrom::Current(len[0] as i64))?;
            size -= 1 + len[0] as u32;
        }
        let mut data = vec![0u8; size as usize];
        f.read_exact(&mut data)?;
        if !compressed {
            return Ok(data);
        }
        let original = u32_at(&data, 0)? as usize;
        let body = &data[4..];
        let mut out = Vec::with_capacity(original);
        if self.version == 105 {
            lz4_flex::frame::FrameDecoder::new(body)
                .read_to_end(&mut out)
                .map_err(|e| BsaError::Decompress(e.to_string()))?;
        } else {
            flate2::read::ZlibDecoder::new(body)
                .read_to_end(&mut out)
                .map_err(|e| BsaError::Decompress(e.to_string()))?;
        }
        Ok(out)
    }
}
