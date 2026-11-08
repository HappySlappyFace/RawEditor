//! The model files, where they come from, and installing them.
//!
//! The upstream projects publish PyTorch checkpoints; ONNX Runtime needs
//! ONNX, so `tools/models/export.py` converts them once and the results are
//! attached to this repo's `models-v1` GitHub release, together with the
//! Apache-2.0 licence and a NOTICE. The app downloads them only when the
//! user clicks to (the only network access besides lensfun and the camera
//! profiles), checks every byte against the SHA-256 recorded here, and only
//! then moves the file into place — a partial or tampered download never
//! becomes a model.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::Progress;

/// Where the converted models are published.
pub const RELEASE_URL: &str =
    "https://github.com/HappySlappyFace/RawEditor/releases/download/models-v1/";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelId {
    /// SAM 2.1 hiera-tiny: click-to-select AI masks.
    Segment,
    /// PMRID: raw-domain AI denoise.
    Denoise,
}

#[derive(Debug, Clone, Copy)]
pub struct ModelFile {
    pub name: &'static str,
    pub sha256: &'static str,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct Model {
    pub id: ModelId,
    pub title: &'static str,
    pub purpose: &'static str,
    pub licence: &'static str,
    pub upstream: &'static str,
    pub files: &'static [ModelFile],
}

pub const SEGMENT: Model = Model {
    id: ModelId::Segment,
    title: "SAM 2.1 (hiera-tiny)",
    purpose: "AI masks: click to select",
    licence: "Apache-2.0, © Meta Platforms, Inc.",
    upstream: "https://github.com/facebookresearch/sam2",
    files: &[
        ModelFile {
            name: "sam2.1_hiera_tiny_encoder.onnx",
            sha256: "91846fd406e3d637a2c47e77c08f63ae7e555e016f7be98e44631c260a20a908",
            bytes: 109_384_566,
        },
        ModelFile {
            name: "sam2.1_hiera_tiny_decoder.onnx",
            sha256: "91927de13a029da213aaa6afab7e0822a374bb3e4585aaa99d2ad0298b707145",
            bytes: 16_509_635,
        },
    ],
};

pub const DENOISE: Model = Model {
    id: ModelId::Denoise,
    title: "PMRID",
    purpose: "AI denoise (raw domain)",
    licence: "Apache-2.0, © Megvii Inc.",
    upstream: "https://github.com/MegEngine/PMRID",
    files: &[ModelFile {
        name: "pmrid.onnx",
        sha256: "c2191a4095ea9c23ac15a4f684a7bad1e9530c49b4cccadde1886d24846624af",
        bytes: 4_165_323,
    }],
};

pub const ALL: [Model; 2] = [SEGMENT, DENOISE];

impl ModelId {
    pub fn model(self) -> &'static Model {
        match self {
            ModelId::Segment => &SEGMENT,
            ModelId::Denoise => &DENOISE,
        }
    }
}

impl Model {
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.bytes).sum()
    }
}

/// The models folder. `RAWEDITOR_MODELS_DIR` overrides it (tests, and
/// developers running freshly exported files).
pub fn dir() -> PathBuf {
    std::env::var_os("RAWEDITOR_MODELS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(crate::core::paths::models_dir)
}

pub fn file_path(file: &ModelFile) -> PathBuf {
    dir().join(file.name)
}

/// Installed = every file present at its recorded size. The hash was checked
/// when it was downloaded; re-hashing 100 MB on every query would not be
/// free, and this runs from `view`.
pub fn is_installed(id: ModelId) -> bool {
    id.model().files.iter().all(|f| {
        std::fs::metadata(file_path(f)).map(|m| m.len() == f.bytes).unwrap_or(false)
    })
}

/// Delete a model's files. Missing files are not an error.
pub fn remove(id: ModelId) -> Result<(), String> {
    for f in id.model().files {
        match std::fs::remove_file(file_path(f)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("could not remove {}: {e}", f.name)),
        }
    }
    Ok(())
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        // Per-read inactivity, not a whole-transfer limit: a 100 MB file on a
        // slow link may legitimately take minutes.
        .timeout_recv_body(Some(Duration::from_secs(60)))
        .timeout_connect(Some(Duration::from_secs(30)))
        .https_only(true)
        .user_agent(format!("RawEditor/{} (AI models)", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// Download every missing file of `id`. Blocking; reports bytes to
/// `progress` and stops (deleting the partial file) when it is cancelled.
pub fn install(id: ModelId, progress: &Progress) -> Result<(), String> {
    let model = id.model();
    let dir = dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    progress.set_total(model.total_bytes());
    let agent = agent();
    for f in model.files {
        let dest = file_path(f);
        if verify_file(&dest, f).is_ok() {
            progress.add(f.bytes);
            continue;
        }
        let url = format!("{RELEASE_URL}{}", f.name);
        let response = agent.get(&url).call().map_err(|e| format!("download failed: {e}"))?;
        let mut reader = response
            .into_body()
            .into_with_config()
            .limit(f.bytes + 1)
            .reader();
        receive(&mut reader, &dest, f, progress)?;
        tracing::info!("AI models: installed {} from {url}", f.name);
    }
    Ok(())
}

/// Stream `reader` into `dest` through a `.part` file, hashing as it goes;
/// the file is renamed into place only if size and hash match.
fn receive(reader: &mut impl Read, dest: &Path, f: &ModelFile, progress: &Progress) -> Result<(), String> {
    let part = dest.with_extension("part");
    let result = (|| {
        let mut out = std::fs::File::create(&part).map_err(|e| format!("could not write {}: {e}", part.display()))?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1 << 16];
        let mut got = 0u64;
        loop {
            if progress.cancelled() {
                return Err(super::CANCELLED.to_string());
            }
            let n = reader.read(&mut buf).map_err(|e| format!("download failed: {e}"))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n]).map_err(|e| format!("could not write {}: {e}", part.display()))?;
            got += n as u64;
            progress.add(n as u64);
        }
        out.sync_all().map_err(|e| format!("could not write {}: {e}", part.display()))?;
        check(got, &hex(&hasher.finalize()), f)?;
        std::fs::rename(&part, dest).map_err(|e| format!("could not install {}: {e}", f.name))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

fn check(bytes: u64, sha256: &str, f: &ModelFile) -> Result<(), String> {
    if bytes != f.bytes {
        return Err(format!("{}: expected {} bytes, got {bytes}", f.name, f.bytes));
    }
    if sha256 != f.sha256 {
        return Err(format!("{}: checksum mismatch — the download is corrupt or not the published file", f.name));
    }
    Ok(())
}

/// Hash a file on disk against its record.
pub fn verify_file(path: &Path, f: &ModelFile) -> Result<(), String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let bytes = std::io::copy(&mut file, &mut hasher).map_err(|e| e.to_string())?;
    check(bytes, &hex(&hasher.finalize()), f)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(data: &[u8]) -> ModelFile {
        let sha = hex(&Sha256::digest(data));
        ModelFile { name: "test.onnx", sha256: Box::leak(sha.into_boxed_str()), bytes: data.len() as u64 }
    }

    #[test]
    fn a_matching_download_is_moved_into_place() {
        let dir = std::env::temp_dir().join(format!("rawed-models-ok-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let data = b"not really a model".repeat(1000);
        let f = record(&data);
        let dest = dir.join(f.name);
        let progress = Progress::default();
        receive(&mut &data[..], &dest, &f, &progress).unwrap();
        assert!(verify_file(&dest, &f).is_ok());
        assert!(!dest.with_extension("part").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_corrupt_or_short_download_never_becomes_a_model() {
        let dir = std::env::temp_dir().join(format!("rawed-models-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let data = b"the published bytes".repeat(100);
        let f = record(&data);
        let dest = dir.join(f.name);
        let progress = Progress::default();

        let mut tampered = data.clone();
        tampered[7] ^= 1;
        assert!(receive(&mut &tampered[..], &dest, &f, &progress).unwrap_err().contains("checksum"));
        assert!(receive(&mut &data[..10], &dest, &f, &progress).unwrap_err().contains("bytes"));
        assert!(!dest.exists() && !dest.with_extension("part").exists());

        progress.cancel();
        assert_eq!(receive(&mut &data[..], &dest, &f, &progress).unwrap_err(), crate::ml::CANCELLED);
        assert!(!dest.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn every_file_is_served_from_the_release() {
        for m in ALL {
            assert!(!m.files.is_empty());
            for f in m.files {
                assert_eq!(f.sha256.len(), 64);
                assert!(f.name.ends_with(".onnx"));
            }
        }
        assert!(RELEASE_URL.starts_with("https://github.com/") && RELEASE_URL.ends_with('/'));
    }
}
