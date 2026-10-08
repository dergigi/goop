use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

pub const DOWNLOAD_BYTES: u64 = 487_170_055;
const URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8.tar.bz2";
const SHA256: &str = "5793d0fd397c5778d2cf2126994d58e9d56b1be7c04d13c7a15bb1b4eafb16bf";
const ATTRIBUTION: &str = "Parakeet TDT 0.6B v3 by NVIDIA. CC BY 4.0.\nhttps://huggingface.co/nvidia/parakeet-tdt-0.6b-v3\nhttps://creativecommons.org/licenses/by/4.0/\nINT8 ONNX conversion distributed by the sherpa-onnx project.\nhttps://github.com/k2-fsa/sherpa-onnx\n";
const FILES: [&str; 4] = [
    "encoder.int8.onnx",
    "decoder.int8.onnx",
    "joiner.int8.onnx",
    "tokens.txt",
];

/// The OS releases this lock after a crash. Separate Goop processes cannot
/// remove a loaded model or overwrite each other's interrupted downloads.
pub(crate) fn lock(root: &Path) -> Result<fs::File> {
    fs::create_dir_all(root)?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("model.lock"))?;
    lock.try_lock()
        .context("Dictation is busy. Try again in a moment")?;
    Ok(lock)
}

pub fn directory(root: &Path) -> PathBuf {
    root.join("parakeet-v3-int8")
}
pub fn ready(root: &Path) -> bool {
    let dir = directory(root);
    fs::read_to_string(dir.join("verified.sha256")).is_ok_and(|s| s == SHA256)
        && FILES
            .iter()
            .all(|f| fs::metadata(dir.join(f)).is_ok_and(|m| m.is_file() && m.len() > 0))
}
pub(crate) fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        bail!("Cancelled");
    }
    Ok(())
}

/// Download into a staging directory. Only an entirely verified model is installed.
pub fn install(root: &Path, cancel: &AtomicBool, mut progress: impl FnMut(u64)) -> Result<()> {
    check_cancel(cancel)?;
    let _lock = lock(root)?;
    if ready(root) {
        return Ok(());
    }
    fs::create_dir_all(root)?;
    let staging = root.join("parakeet-v3-download");
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir(&staging)?;
    let result = (|| {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(20))
            .timeout(std::time::Duration::from_secs(15))
            .build()?;
        let mut response = client
            .get(URL)
            .send()
            .context("Could not download dictation. Check your connection and try again")?
            .error_for_status()
            .context("Dictation download is unavailable. Please try again later")?;
        let archive = staging.join("model.tar.bz2");
        let mut file = fs::File::create(&archive)?;
        let mut hash = Sha256::new();
        let mut received = 0u64;
        let mut reported = 0;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            check_cancel(cancel)?;
            let n = response
                .read(&mut buffer)
                .context("Dictation download was interrupted. Please try again")?;
            if n == 0 {
                break;
            }
            received += n as u64;
            if received > DOWNLOAD_BYTES {
                bail!("Unexpected dictation model size");
            }
            hash.update(&buffer[..n]);
            file.write_all(&buffer[..n])
                .context("Could not save dictation model. Check free disk space")?;
            if received - reported >= 1_000_000 {
                progress(received);
                reported = received;
            }
        }
        file.sync_all()?;
        drop(file);
        if received != DOWNLOAD_BYTES || format!("{:x}", hash.finalize()) != SHA256 {
            bail!("Dictation model download was incomplete or corrupt. Please retry");
        }
        check_cancel(cancel)?;
        progress(DOWNLOAD_BYTES);
        extract(&archive, &staging, cancel)?;
        fs::remove_file(archive)?;
        fs::write(staging.join("ATTRIBUTION.txt"), ATTRIBUTION)?;
        fs::write(staging.join("verified.sha256"), SHA256)?;
        check_cancel(cancel)?;
        let destination = directory(root);
        if destination.exists() {
            fs::remove_dir_all(&destination)?;
        }
        fs::rename(&staging, destination)?;
        Ok(())
    })();
    if staging.exists() {
        let _ = fs::remove_dir_all(staging);
    }
    result
}

fn extract(archive: &Path, target: &Path, cancel: &AtomicBool) -> Result<()> {
    let reader = bzip2::read::BzDecoder::new(fs::File::open(archive)?);
    let mut tar = tar::Archive::new(reader);
    let mut found = std::collections::HashSet::new();
    for entry in tar.entries()? {
        check_cancel(cancel)?;
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !FILES.contains(&name) {
            continue;
        }
        if !entry.header().entry_type().is_file()
            || !found.insert(name.to_owned())
            || entry.size() > 1_500_000_000
        {
            bail!("Invalid dictation model archive");
        }
        // Never use archive paths or follow archive symlinks.
        let mut output = fs::File::create(target.join(name))?;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            check_cancel(cancel)?;
            let n = entry.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            output.write_all(&buffer[..n])?;
        }
        output.sync_all()?;
    }
    if found.len() != FILES.len() {
        bail!("Dictation model archive is missing files");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn incomplete_download_is_not_ready() {
        let root = tempfile::tempdir().unwrap();
        let dir = directory(root.path());
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("verified.sha256"), SHA256).unwrap();
        assert!(!ready(root.path()));
        for name in FILES {
            fs::write(dir.join(name), b"model").unwrap();
        }
        assert!(ready(root.path()));
        fs::write(dir.join(FILES[0]), []).unwrap();
        assert!(!ready(root.path()));
    }

    fn archive(root: &Path, names: &[&str], symlink: bool) -> PathBuf {
        let path = root.join("test.tar.bz2");
        let encoder = bzip2::write::BzEncoder::new(
            fs::File::create(&path).unwrap(),
            bzip2::Compression::fast(),
        );
        let mut archive = tar::Builder::new(encoder);
        for name in names {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            if symlink {
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_link_name("/tmp/not-a-model").unwrap();
                header.set_size(0);
                header.set_cksum();
                archive.append_data(&mut header, name, &[][..]).unwrap();
            } else {
                header.set_size(5);
                header.set_cksum();
                archive
                    .append_data(&mut header, name, &b"model"[..])
                    .unwrap();
            }
        }
        archive.into_inner().unwrap().finish().unwrap();
        path
    }
    #[test]
    fn extraction_requires_all_files_and_rejects_duplicates_and_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("output");
        fs::create_dir(&target).unwrap();
        let cancel = AtomicBool::new(false);
        let complete = archive(root.path(), &FILES, false);
        extract(&complete, &target, &cancel).unwrap();
        assert_eq!(fs::read(target.join("tokens.txt")).unwrap(), b"model");
        let missing = archive(root.path(), &FILES[..3], false);
        assert!(extract(&missing, &target, &cancel).is_err());
        let duplicate = archive(root.path(), &[FILES[0], FILES[0]], false);
        assert!(extract(&duplicate, &target, &cancel).is_err());
        let symlink = archive(root.path(), &FILES[..1], true);
        assert!(extract(&symlink, &target, &cancel).is_err());
    }
    #[test]
    fn cancellation_does_not_install_or_extract() {
        let root = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(true);
        assert!(install(root.path(), &cancel, |_| panic!("should not download")).is_err());
        let path = archive(root.path(), &FILES, false);
        assert!(extract(&path, root.path(), &cancel).is_err());
        assert!(!ready(root.path()));
        assert!(!root.path().join("encoder.int8.onnx").exists());
    }
    #[test]
    #[ignore = "downloads 487 MB; requires GOOP_DOWNLOAD_ROOT"]
    fn downloads_verified_model() {
        let root = PathBuf::from(std::env::var("GOOP_DOWNLOAD_ROOT").unwrap());
        install(&root, &AtomicBool::new(false), |_| {}).unwrap();
        assert!(ready(&root));
        assert!(!root.join("parakeet-v3-download").exists());
    }

    #[test]
    fn model_lock_protects_loaded_files_and_releases_on_drop() {
        let root = tempfile::tempdir().unwrap();
        let lease = lock(root.path()).unwrap();
        assert!(lock(root.path()).is_err());
        drop(lease);
        assert!(lock(root.path()).is_ok());
    }
}
