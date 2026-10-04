//! Skyrim Special Edition data readers, used to bridge Snake into Skyrim
//! (e.g. the skeleton's rest pose for retargeting). Reads the user's own
//! install; nothing is embedded.

pub mod bsa;
pub mod nif;

pub use bsa::Bsa;
pub use nif::Nif;

/// Read a file (e.g. `meshes\\actors\\character\\character assets\\skeleton.nif`)
/// from the BSAs in a Skyrim install's `Data` folder.
pub fn read_install_file(
    skyrim_dir: &std::path::Path,
    inner: &str,
) -> Result<Vec<u8>, bsa::BsaError> {
    let data = skyrim_dir.join("Data");
    let mut archives: Vec<_> = std::fs::read_dir(&data)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("bsa")))
        .collect();
    archives.sort();
    for a in archives {
        let Ok(bsa) = Bsa::open(&a) else { continue };
        if let Some(e) = bsa.find(inner) {
            return bsa.read(e);
        }
    }
    Err(bsa::BsaError::Io(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("{inner} not in {}", data.display()),
    )))
}

/// Skyrim's character skeleton path inside the archives.
pub const CHARACTER_SKELETON: &str = "meshes\\actors\\character\\character assets\\skeleton.nif";
