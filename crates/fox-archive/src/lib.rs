//! Fox Engine archive formats.

pub mod crypto;
pub mod dictionary;
pub mod fpk;
pub mod qar;
pub mod vfs;

pub use dictionary::PathDictionary;
pub use fpk::{Fpk, FpkEntry, FpkKind};
pub use qar::{QarArchive, QarEntry};
pub use vfs::{EntryRef, Vfs, find_game_dir};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("bad magic, expected {0}")]
    BadMagic(&'static str),
    #[error("truncated data")]
    Truncated,
    #[error("not found: {0}")]
    NotFound(String),
    #[error("{0} is not an MGSV: TPP install (expected mgsvtpp.exe and master/)")]
    NotGameDir(std::path::PathBuf),
}

pub type Result<T> = std::result::Result<T, Error>;
