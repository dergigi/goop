//! Native credential storage, with an explicitly selected Linux file alternative.
use anyhow::Result;
#[cfg(target_os = "linux")]
use gpui::AppContext;
use gpui::{App, Task};

type Credential = (String, Vec<u8>);

pub fn read(cx: &App, service: &str) -> Task<Result<Option<Credential>>> {
    #[cfg(target_os = "linux")]
    match local_enabled() {
        Ok(true) => {
            let service = service.to_owned();
            return cx.background_spawn(async move { file::read(&local_path(), &service) });
        }
        Err(error) => return Task::ready(Err(error)),
        Ok(false) => {}
    }
    cx.read_credentials(service)
}

pub fn write(cx: &App, service: &str, username: &str, secret: &[u8]) -> Task<Result<()>> {
    #[cfg(target_os = "linux")]
    match local_enabled() {
        Ok(true) => {
            let (service, username, secret) =
                (service.to_owned(), username.to_owned(), secret.to_vec());
            return cx.background_spawn(async move {
                file::update(&local_path(), &service, Some((username, secret)))
            });
        }
        Err(error) => return Task::ready(Err(error)),
        Ok(false) => {}
    }
    cx.write_credentials(service, username, secret)
}

pub fn delete(cx: &App, service: &str) -> Task<Result<()>> {
    #[cfg(target_os = "linux")]
    match local_enabled() {
        Ok(true) => {
            let service = service.to_owned();
            return cx.background_spawn(async move { file::update(&local_path(), &service, None) });
        }
        Err(error) => return Task::ready(Err(error)),
        Ok(false) => {}
    }
    cx.delete_credentials(service)
}

#[cfg(target_os = "linux")]
fn local_path() -> std::path::PathBuf {
    common::config_dir().join("credentials").join("signer.json")
}

#[cfg(target_os = "linux")]
pub fn local_enabled() -> Result<bool> {
    match std::fs::symlink_metadata(local_path()) {
        Ok(_) => Ok(true), // Invalid files fail closed when read; never silently switch backends.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// Called only after explicit user selection. Copy accessible credentials without
/// deleting the native originals. Never replace a known account's missing key.
#[cfg(target_os = "linux")]
pub fn enable_local(cx: &App, known_account: bool) -> Task<Result<()>> {
    let master = cx.read_credentials(crate::MASTER_KEYRING);
    let user = cx.read_credentials(crate::USER_KEYRING);
    cx.background_spawn(async move {
        let master = master.await;
        let user = user.await;
        let entries = file::migration_entries(master, user, known_account)?;
        file::create(&local_path(), entries)
    })
}

#[cfg(any(target_os = "linux", all(test, unix)))]
mod file {
    use super::*;
    use anyhow::{Context, bail};
    use std::{
        collections::BTreeMap,
        fs::{self, File, OpenOptions},
        io::Write,
        os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
        path::Path,
        sync::Mutex,
    };

    type Entries = BTreeMap<String, Credential>;
    static LOCK: Mutex<()> = Mutex::new(());

    fn private(metadata: &fs::Metadata, directory: bool) -> Result<()> {
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
            || if directory {
                !metadata.is_dir()
            } else {
                !metadata.is_file()
            }
        {
            bail!(
                "Local credential storage must be owned by you, with directory permissions 700 and file permissions 600"
            );
        }
        Ok(())
    }

    fn directory(path: &Path) -> Result<()> {
        let parent = path.parent().context("Missing credential directory")?;
        private(&fs::symlink_metadata(parent)?, true)
    }

    fn load(path: &Path) -> Result<Entries> {
        directory(path)?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        private(&file.metadata()?, false)?;
        serde_json::from_reader(file)
            .context("Cannot read local signer credentials; the file was left unchanged")
    }

    fn save(path: &Path, entries: &Entries) -> Result<()> {
        directory(path)?;
        let parent = path.parent().unwrap();
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?; // Created with mode 600.
        serde_json::to_writer(temporary.as_file_mut(), entries)?;
        temporary.flush()?;
        temporary.as_file().sync_all()?;
        temporary.persist(path).map_err(|e| e.error)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    }

    pub(super) fn migration_entries(
        master: Result<Option<Credential>>,
        user: Result<Option<Credential>>,
        known_account: bool,
    ) -> Result<Entries> {
        let (master, user) = if known_account {
            (master?, user?)
        } else {
            (master.unwrap_or(None), user.unwrap_or(None))
        };
        if master.is_none() && (known_account || user.is_some()) {
            bail!(
                "The existing signer connection key is unavailable. Restore access to your system credential store before changing storage."
            );
        }
        let mut entries = Entries::new();
        if let Some(value) = master {
            entries.insert(crate::MASTER_KEYRING.into(), value);
        }
        if let Some(value) = user {
            entries.insert(crate::USER_KEYRING.into(), value);
        }
        Ok(entries)
    }

    pub(super) fn create(path: &Path, entries: Entries) -> Result<()> {
        let _guard = LOCK.lock().unwrap();
        let parent = path.parent().context("Missing credential directory")?;
        fs::create_dir_all(parent.parent().context("Missing config directory")?)?;
        match fs::DirBuilder::new().mode(0o700).create(parent) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        directory(path)?;
        // Do not overwrite an existing selection, even if malformed or a symlink.
        match fs::symlink_metadata(path) {
            Ok(_) => {
                load(path)?;
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        save(path, &entries)
    }

    pub(super) fn read(path: &Path, service: &str) -> Result<Option<Credential>> {
        let _guard = LOCK.lock().unwrap();
        Ok(load(path)?.remove(service))
    }

    pub(super) fn update(path: &Path, service: &str, value: Option<Credential>) -> Result<()> {
        let _guard = LOCK.lock().unwrap();
        let mut entries = load(path)?;
        if let Some(value) = value {
            entries.insert(service.into(), value);
        } else {
            entries.remove(service);
        }
        save(path, &entries)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::fs::{PermissionsExt, symlink};
        #[test]
        fn reconnect_and_logout_preserve_connection_key() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("credentials/signer.json");
            create(&path, Entries::new()).unwrap();
            let key = ("client".into(), vec![42; 32]);
            update(&path, crate::MASTER_KEYRING, Some(key.clone())).unwrap();
            update(
                &path,
                crate::USER_KEYRING,
                Some(("bunker".into(), b"bunker://example".to_vec())),
            )
            .unwrap();
            assert_eq!(
                read(&path, crate::MASTER_KEYRING).unwrap(),
                Some(key.clone())
            );
            assert!(read(&path, crate::USER_KEYRING).unwrap().is_some());
            assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
            assert_eq!(
                fs::metadata(path.parent().unwrap()).unwrap().mode() & 0o777,
                0o700
            );
            update(&path, crate::USER_KEYRING, None).unwrap();
            assert!(read(&path, crate::USER_KEYRING).unwrap().is_none());
            assert_eq!(read(&path, crate::MASTER_KEYRING).unwrap(), Some(key));
        }
        #[test]
        fn malformed_or_exposed_files_are_not_replaced() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("credentials/signer.json");
            create(&path, Entries::new()).unwrap();
            fs::write(&path, b"invalid").unwrap();
            assert!(update(&path, "user", None).is_err());
            assert_eq!(fs::read(&path).unwrap(), b"invalid");
            fs::write(&path, b"{}").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(read(&path, "user").is_err());
            assert!(create(&path, Entries::new()).is_err());
        }
        #[test]
        fn symlink_file_and_directory_are_rejected() {
            let dir = tempfile::tempdir().unwrap();
            let target = dir.path().join("target");
            fs::write(&target, b"untouched").unwrap();
            let path = dir.path().join("credentials/signer.json");
            create(&path, Entries::new()).unwrap();
            fs::remove_file(&path).unwrap();
            symlink(&target, &path).unwrap();
            assert!(read(&path, "user").is_err());
            assert!(update(&path, "user", None).is_err());
            assert_eq!(fs::read(&target).unwrap(), b"untouched");
            let linked = dir.path().join("linked");
            symlink(path.parent().unwrap(), &linked).unwrap();
            assert!(create(&linked.join("signer.json"), Entries::new()).is_err());
        }
        #[test]
        fn migration_handles_missing_portal_without_rotating_existing_identity() {
            let unavailable = || Err(anyhow::anyhow!("Portal unavailable"));
            assert!(
                migration_entries(unavailable(), unavailable(), false)
                    .unwrap()
                    .is_empty()
            );
            assert!(migration_entries(unavailable(), unavailable(), true).is_err());
            assert!(migration_entries(Ok(None), Ok(None), true).is_err());
            assert!(
                migration_entries(unavailable(), Ok(Some(("bunker".into(), vec![1]))), false)
                    .is_err()
            );
            let key = ("client".into(), vec![7; 32]);
            assert_eq!(
                migration_entries(Ok(Some(key.clone())), unavailable(), false)
                    .unwrap()
                    .get(crate::MASTER_KEYRING),
                Some(&key)
            );
        }
        #[test]
        fn simultaneous_saves_keep_both_credentials() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("credentials/signer.json");
            create(&path, Entries::new()).unwrap();
            std::thread::scope(|scope| {
                for service in [crate::MASTER_KEYRING, crate::USER_KEYRING] {
                    let path = &path;
                    scope.spawn(move || {
                        update(path, service, Some(("name".into(), vec![1]))).unwrap()
                    });
                }
            });
            assert_eq!(load(&path).unwrap().len(), 2);
        }
    }
}
