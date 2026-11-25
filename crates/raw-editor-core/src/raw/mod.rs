//! RAW handling: sensor-data decoding for the GPU pipeline (`loader`), Adobe
//! DCP colour profiles (`dcp`), and the embedded-JPEG cache tiers behind the
//! library grid and culling views (`jpeg`, `processor`).
use std::path::Path;

pub mod dcp;
pub mod exif;
pub mod jpeg;
pub mod loader;
pub mod processor; // Phase 28: Multi-tier cache processor
#[cfg(test)]
mod tests;

/// File extensions the importer accepts, lower-case.
///
/// Bayer-sensor formats only. rawler can decode more (`rawler::decoders::
/// supported_extensions`), but the debayer pass assumes a 2×2 CFA: Fuji
/// X-Trans RAF and Sigma Foveon X3F would import fine and then develop as
/// garbage, so they stay out until the pipeline can render them.
pub const RAW_EXTENSIONS: &[&str] = &[
    "nef", "nrw", // Nikon
    "cr2", "cr3", // Canon
    "arw",        // Sony
    "dng",        // Adobe / Leica / Pentax / phones
    "orf",        // OM System / Olympus
    "rw2", "rwl", // Panasonic / Leica
    "pef",        // Pentax
    "srw",        // Samsung
];

/// True when `path` has one of [`RAW_EXTENSIONS`] (case-insensitive).
pub fn is_supported_raw(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| RAW_EXTENSIONS.iter().any(|known| ext.eq_ignore_ascii_case(known)))
}

#[cfg(test)]
mod extension_tests {
    use super::*;

    #[test]
    fn supported_raw_matches_any_case_and_rejects_the_rest() {
        assert!(is_supported_raw(Path::new("/c/DSC_0001.NEF")));
        assert!(is_supported_raw(Path::new("/c/IMG_0001.cr3")));
        assert!(is_supported_raw(Path::new("/c/P1000001.Rw2")));
        assert!(!is_supported_raw(Path::new("/c/DSC_0001.JPG")));
        assert!(!is_supported_raw(Path::new("/c/DSCF0001.RAF")));
        assert!(!is_supported_raw(Path::new("/c/NEF")));
        assert!(!is_supported_raw(Path::new("/c/archive.nef.xmp")));
    }
}
