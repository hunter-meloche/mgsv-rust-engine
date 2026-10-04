//! QAR (`SQAR`) archives: the `.dat` files in `master/` and `mgo/`.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use memmap2::Mmap;

use crate::crypto::{self, Layer1};
use crate::{Error, Result};

const MAGIC: u32 = 0x5241_5153; // "SQAR"
const HEADER_XOR: [u32; 4] = [0x4144_1043, 0x11C2_2050, 0xD056_08C3, 0x532C_7319];
const ENTRY_HEADER_SIZE: u64 = 32;

#[derive(Clone, Debug)]
pub struct QarEntry {
    /// `extension_id << 51 | path hash`; see [`fox_hash::path_code64_ext`].
    pub hash: u64,
    /// Absolute offset of the entry data in the archive file.
    pub data_offset: u64,
    pub stored_size: u32,
    pub uncompressed_size: u32,
    pub md5: [u8; 16],
    /// Layer-2 magic, or 0 when the entry only uses layer 1.
    pub encryption: u32,
    pub key: u32,
}

impl QarEntry {
    pub fn is_compressed(&self) -> bool {
        self.stored_size != self.uncompressed_size
    }
    pub fn extension_id(&self) -> u16 {
        (self.hash >> 51) as u16
    }
    pub fn path_hash(&self) -> u64 {
        self.hash & fox_hash::PATH_HASH_MASK
    }
}

pub struct QarArchive {
    path: PathBuf,
    map: Mmap,
    pub flags: u32,
    pub version: u32,
    pub entries: Vec<QarEntry>,
}

fn u32_at(b: &[u8], at: usize) -> Result<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
        .ok_or(Error::Truncated)
}

impl QarArchive {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = File::open(&path)?;
        // SAFETY: the game files are treated as read-only; nothing in this
        // process writes to them while mapped.
        let map = unsafe { Mmap::map(&file)? };
        let b = &map[..];

        if u32_at(b, 0)? != MAGIC {
            return Err(Error::BadMagic("SQAR"));
        }
        let flags = u32_at(b, 4)? ^ HEADER_XOR[0];
        let file_count = u32_at(b, 8)? ^ HEADER_XOR[1];
        let _unknown_count = u32_at(b, 12)? ^ HEADER_XOR[2];
        let _block_file_end = u32_at(b, 16)? ^ HEADER_XOR[3];
        let _first_file_offset = u32_at(b, 20)? ^ HEADER_XOR[0];
        let version = u32_at(b, 24)? ^ HEADER_XOR[0];
        let block_shift = if flags & 0x800 != 0 { 12 } else { 10 };

        let mut entries = Vec::with_capacity(file_count as usize);
        let mut v2_xor: u32 = 0xA2C1_8EC3;
        for i in 0..file_count as usize {
            let at = 32 + i * 8;
            let (raw1, raw2) = (u32_at(b, at)?, u32_at(b, at + 4)?);
            let (o1, o2) = (i * 8, i * 8 + 4);
            let (lo, hi) = if version != 2 {
                (
                    raw1 ^ HEADER_XOR[(i + o1 / 5) % 4],
                    raw2 ^ HEADER_XOR[(i + o2 / 5) % 4],
                )
            } else {
                let lo = raw1 ^ HEADER_XOR[(v2_xor.wrapping_add((o1 / 5) as u32) % 4) as usize];
                let hi = raw2 ^ HEADER_XOR[(v2_xor.wrapping_add((o2 / 5) as u32) % 4) as usize];
                let rot = (hi / 256) % 19;
                v2_xor ^= lo.rotate_right(rot);
                (lo, hi)
            };
            let section = (hi as u64) << 32 | lo as u64;
            let offset = (section >> 40) << block_shift;
            entries.push(Self::read_entry_header(b, offset, version)?);
        }

        Ok(Self {
            path,
            map,
            flags,
            version,
            entries,
        })
    }

    fn read_entry_header(b: &[u8], offset: u64, version: u32) -> Result<QarEntry> {
        let o = offset as usize;
        let hash_lo = u32_at(b, o)? ^ HEADER_XOR[0];
        let hash_hi = u32_at(b, o + 4)? ^ HEADER_XOR[0];
        let size1 = u32_at(b, o + 8)? ^ HEADER_XOR[1];
        let size2 = u32_at(b, o + 12)? ^ HEADER_XOR[2];
        let md5_words = [
            u32_at(b, o + 16)? ^ HEADER_XOR[3],
            u32_at(b, o + 20)? ^ HEADER_XOR[0],
            u32_at(b, o + 24)? ^ HEADER_XOR[0],
            u32_at(b, o + 28)? ^ HEADER_XOR[1],
        ];
        let mut md5 = [0u8; 16];
        for (k, w) in md5_words.iter().enumerate() {
            md5[k * 4..k * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
        let hash = (hash_hi as u64) << 32 | hash_lo as u64;
        let (uncompressed_size, stored_size) = if version != 2 {
            (size1, size2)
        } else {
            (size2, size1)
        };
        let data_offset = offset + ENTRY_HEADER_SIZE;

        let mut entry = QarEntry {
            hash,
            data_offset,
            stored_size,
            uncompressed_size,
            md5,
            encryption: 0,
            key: 0,
        };
        if stored_size >= 8 {
            let d = data_offset as usize;
            let mut head = b.get(d..d + 8).ok_or(Error::Truncated)?.to_vec();
            Layer1::new(version, hash, &md5).apply(&mut head, 0);
            let magic = u32::from_le_bytes(head[0..4].try_into().unwrap());
            if crypto::layer2_header_size(magic) != 0 {
                entry.encryption = magic;
                entry.key = u32::from_le_bytes(head[4..8].try_into().unwrap());
            }
        }
        Ok(entry)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Raw stored bytes of an entry, before any decoding.
    pub fn raw(&self, entry: &QarEntry) -> Result<&[u8]> {
        let start = entry.data_offset as usize;
        self.map
            .get(start..start + entry.stored_size as usize)
            .ok_or(Error::Truncated)
    }

    /// Fully decoded contents of an entry.
    pub fn read(&self, entry: &QarEntry) -> Result<Vec<u8>> {
        let mut data = self.raw(entry)?.to_vec();
        Layer1::new(self.version, entry.hash, &entry.md5).apply(&mut data, 0);

        if entry.encryption != 0 {
            let header = crypto::layer2_header_size(entry.encryption);
            data.drain(..header);
            crypto::layer2_apply(&mut data, entry.key);
        }

        if entry.is_compressed() {
            let mut out = Vec::with_capacity(entry.uncompressed_size as usize);
            flate2::read::ZlibDecoder::new(&data[..]).read_to_end(&mut out)?;
            data = out;
        }
        Ok(data)
    }
}
