//! A read-only view over every QAR archive in a TPP install.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::dictionary::PathDictionary;
use crate::qar::{QarArchive, QarEntry};
use crate::{Error, Result};

/// Locate the game install.
///
/// Order: `MGSV_GAME_DIR`, then the current directory and its ancestors
/// (the repository is meant to be cloned inside the game folder).
pub fn find_game_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("MGSV_GAME_DIR").map(PathBuf::from) {
        if is_game_dir(&dir) {
            return Some(dir);
        }
    }
    let cwd = std::env::current_dir().ok()?;
    cwd.ancestors()
        .find(|d| is_game_dir(d))
        .map(Path::to_path_buf)
}

pub fn is_game_dir(dir: &Path) -> bool {
    dir.join("mgsvtpp.exe").is_file() && dir.join("master").is_dir()
}

/// Archives in priority order, highest first. Patches override the base
/// game; `00.dat` (scripts, boot data) overrides the bulk chunks.
pub fn tpp_archive_paths(game_dir: &Path) -> Vec<PathBuf> {
    let master = game_dir.join("master");
    let mut out = Vec::new();

    // Patch folders: master/1/<NAME>/0/*.dat, newest name last alphabetically.
    if let Ok(rd) = std::fs::read_dir(master.join("1")) {
        let mut patches: Vec<_> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        patches.sort();
        for p in patches.into_iter().rev() {
            push_sorted_dats(&p.join("0"), &mut out);
        }
    }
    push_sorted_dats(&master.join("0"), &mut out);
    for name in ["data1.dat"] {
        out.push(master.join(name));
    }
    for i in 0..=4 {
        out.push(master.join(format!("chunk{i}.dat")));
    }
    for i in 0..=4 {
        out.push(master.join(format!("texture{i}.dat")));
    }
    // Hash-named archives (later content drops).
    if let Ok(rd) = std::fs::read_dir(&master) {
        let mut extra: Vec<_> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension().is_some_and(|e| e == "dat")
                    && p.file_stem()
                        .and_then(|s| s.to_str())
                        .is_some_and(|s| s.len() == 16 && s.chars().all(|c| c.is_ascii_hexdigit()))
            })
            .collect();
        extra.sort();
        out.extend(extra);
    }
    out.retain(|p| p.is_file());
    out
}

fn push_sorted_dats(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        let mut v: Vec<_> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "dat"))
            .collect();
        v.sort();
        out.extend(v);
    }
}

#[derive(Clone, Copy, Debug)]
pub struct EntryRef {
    pub archive: u32,
    pub entry: u32,
}

pub struct Vfs {
    pub game_dir: PathBuf,
    pub archives: Vec<QarArchive>,
    /// Highest-priority location of each entry hash.
    index: HashMap<u64, EntryRef>,
    pub dictionary: PathDictionary,
}

impl Vfs {
    pub fn open(game_dir: impl AsRef<Path>) -> Result<Self> {
        let game_dir = game_dir.as_ref().to_path_buf();
        if !is_game_dir(&game_dir) {
            return Err(Error::NotGameDir(game_dir));
        }
        let mut archives = Vec::new();
        for p in tpp_archive_paths(&game_dir) {
            match QarArchive::open(&p) {
                Ok(a) => archives.push(a),
                Err(Error::BadMagic(_)) => continue,
                Err(e) => return Err(e),
            }
        }
        let mut index = HashMap::new();
        for (ai, a) in archives.iter().enumerate() {
            for (ei, e) in a.entries.iter().enumerate() {
                index.entry(e.hash).or_insert(EntryRef {
                    archive: ai as u32,
                    entry: ei as u32,
                });
            }
        }
        Ok(Self {
            game_dir,
            archives,
            index,
            dictionary: PathDictionary::new(),
        })
    }

    pub fn entry(&self, r: EntryRef) -> (&QarArchive, &QarEntry) {
        let a = &self.archives[r.archive as usize];
        (a, &a.entries[r.entry as usize])
    }

    /// All unique entries, highest priority wins.
    pub fn iter(&self) -> impl Iterator<Item = (u64, EntryRef)> + '_ {
        self.index.iter().map(|(h, r)| (*h, *r))
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    pub fn lookup_hash(&self, hash: u64) -> Option<EntryRef> {
        self.index.get(&hash).copied()
    }

    /// Find a file by path (`/Assets/tpp/...lua`, `/init.lua`).
    pub fn lookup(&self, path: &str) -> Option<EntryRef> {
        self.lookup_hash(fox_hash::path_code64_ext(path))
    }

    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        let r = self
            .lookup(path)
            .ok_or_else(|| Error::NotFound(path.to_string()))?;
        let (a, e) = self.entry(r);
        a.read(e)
    }

    pub fn read_ref(&self, r: EntryRef) -> Result<Vec<u8>> {
        let (a, e) = self.entry(r);
        a.read(e)
    }
}
