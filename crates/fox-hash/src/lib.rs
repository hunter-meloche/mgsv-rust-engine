//! Hash functions used throughout the Fox Engine (MGSV: TPP).
//!
//! * [`str_code64`] / [`str_code32`]: the engine's general string hash
//!   (`StrCode64`/`StrCode32` in Lua, entity and property names in `.fox2`).
//! * [`path_code64`]: the 50-bit path hash used for QAR (`.dat`) archive
//!   entries, combined with a 13-bit extension id by [`path_code64_ext`].

mod city;

pub use city::{city_hash64, city_hash64_with_seed, city_hash64_with_seeds};

const SEED0: u64 = 0x9ae1_6a3b_2f90_404f;

/// Bit 50 of a QAR path hash: set for paths outside `/Assets/` (and for
/// `/Assets/tpptest*`).
pub const PATH_META_FLAG: u64 = 1 << 50;
/// Low 50 bits of a QAR path hash.
pub const PATH_HASH_MASK: u64 = PATH_META_FLAG - 1;

/// `StrCode64`: CityHash64 of the NUL-terminated string, masked to 48 bits.
pub fn str_code64(text: &[u8]) -> u64 {
    let mut bytes = Vec::with_capacity(text.len() + 1);
    bytes.extend_from_slice(text);
    bytes.push(0);
    let seed1 = match text.first() {
        Some(&b0) => ((b0 as u64) << 16).wrapping_add(text.len() as u64),
        None => 0,
    };
    city_hash64_with_seeds(&bytes, SEED0, seed1) & 0xFFFF_FFFF_FFFF
}

/// `StrCode32`: the low 32 bits of [`str_code64`].
pub fn str_code32(text: &[u8]) -> u32 {
    str_code64(text) as u32
}

/// Hash of a path without its extension, as stored in QAR entry ids.
///
/// Everything after the first `.` is dropped. `/Assets/` is stripped; paths
/// outside it get [`PATH_META_FLAG`].
pub fn path_code64(path: &str) -> u64 {
    let stem = match path.find('.') {
        Some(i) => &path[..i],
        None => path,
    };
    path_code64_raw(stem)
}

fn path_code64_raw(text: &str) -> u64 {
    let mut meta = true;
    let mut text = text;
    if let Some(rest) = text.strip_prefix("/Assets/") {
        meta = rest.starts_with("tpptest");
        text = rest;
    }
    let text = text.trim_start_matches('/').as_bytes();

    // seed1 is the last (up to) eight bytes, reversed, read little-endian.
    let mut seed1 = [0u8; 8];
    for (j, &b) in text.iter().rev().take(8).enumerate() {
        seed1[j] = b;
    }
    let hash = city_hash64_with_seeds(text, SEED0, u64::from_le_bytes(seed1)) & PATH_HASH_MASK;
    if meta { hash | PATH_META_FLAG } else { hash }
}

/// 13-bit type id of a file extension (without the leading dot).
pub fn extension_id(ext: &str) -> u16 {
    (path_code64_raw(ext) & 0x1FFF) as u16
}

/// Full QAR entry id: `extension_id << 51 | path_code64`.
pub fn path_code64_ext(path: &str) -> u64 {
    let path = path.replace('\\', "/");
    let (stem, ext) = match path.find('.') {
        Some(i) => (&path[..i], &path[i + 1..]),
        None => (path.as_str(), ""),
    };
    let type_id = if KNOWN_EXTENSIONS.contains(&ext) {
        extension_id(ext) as u64
    } else {
        0
    };
    (type_id << 51) | path_code64_raw(stem)
}

/// Extension for a 13-bit type id, if it is one the engine is known to use.
pub fn extension_from_id(id: u16) -> Option<&'static str> {
    use std::sync::OnceLock;
    static TABLE: OnceLock<std::collections::HashMap<u16, &'static str>> = OnceLock::new();
    TABLE
        .get_or_init(|| {
            KNOWN_EXTENSIONS
                .iter()
                .map(|e| (extension_id(e), *e))
                .collect()
        })
        .get(&id)
        .copied()
}

/// File extensions that appear in TPP QAR archives. Multi-part extensions
/// (`eng.lng`, `1.ftexs`) are hashed as a whole.
pub const KNOWN_EXTENSIONS: &[&str] = &[
    "1.ftexs", "1.nav2", "2.ftexs", "3.ftexs", "4.ftexs", "5.ftexs", "6.ftexs", "ag.evf", "aia",
    "aib", "aibc", "aig", "aigc", "aim", "aip", "ait", "atsh", "bnd", "bnk", "cc.evf", "clo",
    "csnav", "dat", "des", "dnav", "dnav2", "eng.lng", "ese", "evb", "evf", "fag", "fage", "fago",
    "fagp", "fagx", "fclo", "fcnp", "fcnpx", "fdes", "fdmg", "ffnt", "fmdl", "fmdlb", "fmtt",
    "fnt", "fova", "fox", "fox2", "fpk", "fpkd", "fpkl", "frdv", "fre.lng", "frig", "frt", "fsd",
    "fsm", "fsml", "fsop", "fstb", "ftex", "fv2", "fx.evf", "fxp", "gani", "geom", "ger.lng",
    "gpfp", "grxla", "grxoc", "gskl", "htre", "info", "ita.lng", "jpn.lng", "json", "lad", "ladb",
    "lani", "las", "lba", "lng", "lpsh", "lua", "mas", "mbl", "mog", "mtar", "mtl", "nav2", "nta",
    "obr", "obrb", "param", "parts", "path", "pftxs", "ph", "phep", "phsd", "por.lng", "qar",
    "rbs", "rdb", "rdf", "rnav", "rus.lng", "sad", "sand", "sani", "sbp", "sd.evf", "sdf", "sim",
    "simep", "snav", "spa.lng", "spch", "sub", "subp", "tgt", "tre2", "txt", "uia", "uif", "uig",
    "uigb", "uil", "uilb", "utxl", "veh", "vfx", "vfxbin", "vfxdb", "vnav", "vo.evf", "vpc", "wem",
    "wmv", "xml",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_ids_are_unique() {
        let mut ids: Vec<_> = KNOWN_EXTENSIONS.iter().map(|e| extension_id(e)).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), KNOWN_EXTENSIONS.len());
    }

    #[test]
    fn ext_roundtrip() {
        for e in KNOWN_EXTENSIONS {
            assert_eq!(extension_from_id(extension_id(e)), Some(*e));
        }
    }

    #[test]
    fn path_hash_layout() {
        let h = path_code64_ext("/Assets/tpp/script/lib/TppMain.lua");
        assert_eq!(h >> 51, extension_id("lua") as u64);
        assert_eq!(h & PATH_META_FLAG, 0);
        assert_ne!(path_code64("/init.lua") & PATH_META_FLAG, 0);
    }
}
