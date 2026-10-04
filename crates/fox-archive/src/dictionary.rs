//! Path dictionaries for turning QAR hashes back into names.
//!
//! QAR entries only store hashes. Names are recovered by hashing candidate
//! paths (community dictionaries, strings harvested from game data) and
//! matching.

use std::collections::HashMap;
use std::path::Path;

use fox_hash::{PATH_HASH_MASK, extension_from_id, path_code64};

#[derive(Default)]
pub struct PathDictionary {
    by_hash: HashMap<u64, String>,
}

impl PathDictionary {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one candidate path. Returns true if it was new.
    pub fn insert(&mut self, path: &str) -> bool {
        let path = path.trim();
        if path.is_empty() {
            return false;
        }
        let stem = match path.find('.') {
            Some(i) => &path[..i],
            None => path,
        };
        let h = path_code64(stem) & PATH_HASH_MASK;
        if self.by_hash.contains_key(&h) {
            return false;
        }
        self.by_hash.insert(h, stem.to_string());
        true
    }

    pub fn load_file(&mut self, path: impl AsRef<Path>) -> std::io::Result<usize> {
        let text = std::fs::read_to_string(path)?;
        Ok(text.lines().filter(|l| self.insert(l)).count())
    }

    pub fn len(&self) -> usize {
        self.by_hash.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_hash.is_empty()
    }

    pub fn contains(&self, entry_hash: u64) -> bool {
        self.by_hash.contains_key(&(entry_hash & PATH_HASH_MASK))
    }

    /// Resolve a full QAR entry hash. Unknown paths become the hex hash, and
    /// unknown extensions become `_unknown`, so the result is always usable
    /// as a file name. The bool says whether both parts were resolved.
    pub fn resolve(&self, entry_hash: u64) -> (String, bool) {
        let ext = extension_from_id((entry_hash >> 51) as u16);
        let stem = self.by_hash.get(&(entry_hash & PATH_HASH_MASK));
        let found = stem.is_some() && ext.is_some();
        let stem = stem
            .cloned()
            .unwrap_or_else(|| format!("{:x}", entry_hash & PATH_HASH_MASK));
        let name = match ext {
            Some(e) => format!("{stem}.{e}"),
            None => format!("{stem}._unknown"),
        };
        (name, found)
    }
}
