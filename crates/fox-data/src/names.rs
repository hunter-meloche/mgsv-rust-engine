//! Reverse lookup for `StrCode64` hashes (class names, property names, and
//! string values). Names come from dictionaries, from the `.fox2` string
//! tables themselves, and from identifiers harvested from the game executable.

use std::collections::HashMap;
use std::path::Path;

#[derive(Default, Clone)]
pub struct NameTable {
    by_hash: HashMap<u64, String>,
}

impl NameTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, name: &str) -> u64 {
        let h = fox_hash::str_code64(name.as_bytes());
        self.by_hash.entry(h).or_insert_with(|| name.to_string());
        h
    }

    pub fn get(&self, hash: u64) -> Option<&str> {
        self.by_hash.get(&hash).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.by_hash.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_hash.is_empty()
    }

    /// One name per line.
    pub fn load_dictionary(&mut self, path: impl AsRef<Path>) -> std::io::Result<usize> {
        let text = std::fs::read_to_string(path)?;
        let before = self.len();
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            self.insert(line);
        }
        Ok(self.len() - before)
    }

    /// Add every identifier-like ASCII run (`[A-Za-z_][A-Za-z0-9_]{2,}`) in a
    /// binary, e.g. the reflection names in mgsvtpp.exe.
    pub fn harvest_identifiers(&mut self, data: &[u8]) -> usize {
        let before = self.len();
        let mut start = None;
        for (i, &b) in data.iter().enumerate() {
            let ident = b.is_ascii_alphanumeric() || b == b'_';
            match (start, ident) {
                (None, true) if !b.is_ascii_digit() => start = Some(i),
                (Some(s), false) => {
                    if i - s >= 3 && i - s <= 96 {
                        // SAFETY of from_utf8: bytes are ASCII by construction.
                        self.insert(std::str::from_utf8(&data[s..i]).unwrap());
                    }
                    start = None;
                }
                _ => {}
            }
        }
        self.len() - before
    }
}
