//! FPK / FPKD packages (`foxfpk win` / `foxfpkdwin`).
//!
//! `.fpk` holds asset files; `.fpkd` holds the matching data sets (`.fox2`,
//! scripts, parameter files). Both share one layout.

use md5::{Digest, Md5};

use crate::crypto;
use crate::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FpkKind {
    Fpk,
    Fpkd,
}

#[derive(Clone, Debug)]
pub struct FpkEntry {
    /// Path as stored. For a few entries the stored path is obfuscated; then
    /// `path_verified` is false and `md5` is the hash of the real path.
    pub path: String,
    pub path_verified: bool,
    pub md5: [u8; 16],
    pub offset: u32,
    pub size: u32,
}

impl FpkEntry {
    /// Path with drive prefixes (`Z:`) removed and `\` normalised to `/`.
    pub fn clean_path(&self) -> String {
        let p = self.path.replace('\\', "/");
        match p.find(':') {
            Some(i) => p[i + 1..].to_string(),
            None => p,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Fpk<'a> {
    data: &'a [u8],
    pub kind: FpkKind,
    pub entries: Vec<FpkEntry>,
    /// Other packages this one depends on.
    pub references: Vec<String>,
}

fn u32_at(b: &[u8], at: usize) -> Result<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
        .ok_or(Error::Truncated)
}

fn fpk_string(b: &[u8], at: usize) -> Result<String> {
    let off = u32_at(b, at)? as usize;
    let len = u32_at(b, at + 8)? as usize;
    let s = b.get(off..off + len).ok_or(Error::Truncated)?;
    Ok(String::from_utf8_lossy(s).into_owned())
}

pub fn is_fpk(data: &[u8]) -> bool {
    data.len() >= 48 && &data[0..6] == b"foxfpk" && &data[7..10] == b"win"
}

impl<'a> Fpk<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        if !is_fpk(data) {
            return Err(Error::BadMagic("foxfpk"));
        }
        let kind = if data[6] == b'd' {
            FpkKind::Fpkd
        } else {
            FpkKind::Fpk
        };
        let file_count = u32_at(data, 36)? as usize;
        let ref_count = u32_at(data, 40)? as usize;

        let mut entries = Vec::with_capacity(file_count);
        for i in 0..file_count {
            let at = 48 + i * 48;
            let offset = u32_at(data, at)?;
            let size = u32_at(data, at + 8)?;
            let path = fpk_string(data, at + 16)?;
            let md5: [u8; 16] = data
                .get(at + 32..at + 48)
                .ok_or(Error::Truncated)?
                .try_into()
                .unwrap();
            let path_verified = Md5::digest(path.as_bytes()).as_slice() == md5;
            entries.push(FpkEntry {
                path,
                path_verified,
                md5,
                offset,
                size,
            });
        }
        let refs_at = 48 + file_count * 48;
        let references = (0..ref_count)
            .map(|i| fpk_string(data, refs_at + i * 16))
            .collect::<Result<_>>()?;
        Ok(Self {
            data,
            kind,
            entries,
            references,
        })
    }

    /// Stored bytes of an entry.
    pub fn raw(&self, entry: &FpkEntry) -> Result<&'a [u8]> {
        let s = entry.offset as usize;
        self.data
            .get(s..s + entry.size as usize)
            .ok_or(Error::Truncated)
    }

    /// Contents of an entry, with script decryption applied where needed.
    pub fn read(&self, entry: &FpkEntry) -> Result<Vec<u8>> {
        let raw = self.raw(entry)?;
        let name = entry.clean_path();
        let file_name = name.rsplit('/').next().unwrap_or(&name);
        Ok(crypto::decrypt_fpk_script(raw, file_name).unwrap_or_else(|| raw.to_vec()))
    }
}
