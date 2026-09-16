//! Use the upstream portal fallback on Linux without changing GPUI's dependency.
use anyhow::Result;
#[cfg(target_os = "linux")]
use gpui::AppContext;
use gpui::{App, Task};

type Credential = (String, Vec<u8>);

pub fn read(cx: &App, service: &str) -> Task<Result<Option<Credential>>> {
    #[cfg(target_os = "linux")]
    {
        let service = service.to_owned();
        cx.background_spawn(async move {
            let keyring = linux::open().await?;
            linux::read(&keyring, &service).await
        })
    }
    #[cfg(not(target_os = "linux"))]
    cx.read_credentials(service)
}

pub fn write(cx: &App, service: &str, username: &str, secret: &[u8]) -> Task<Result<()>> {
    #[cfg(target_os = "linux")]
    {
        let (service, username, secret) =
            (service.to_owned(), username.to_owned(), secret.to_vec());
        cx.background_spawn(async move {
            let keyring = linux::open().await?;
            linux::write(&keyring, &service, &username, &secret).await
        })
    }
    #[cfg(not(target_os = "linux"))]
    cx.write_credentials(service, username, secret)
}

pub fn delete(cx: &App, service: &str) -> Task<Result<()>> {
    #[cfg(target_os = "linux")]
    {
        let service = service.to_owned();
        cx.background_spawn(async move {
            let keyring = linux::open().await?;
            linux::delete(&keyring, &service).await
        })
    }
    #[cfg(not(target_os = "linux"))]
    cx.delete_credentials(service)
}

#[cfg(any(target_os = "linux", all(test, unix)))]
mod linux {
    use super::*;
    use anyhow::Context;
    use oo7::Keyring;

    // GPUI used this label on Linux. Keep it and its attributes so an upgrade
    // reads existing credentials rather than creating a second client identity.
    const LABEL: &str = "zed-github-account";

    #[cfg(target_os = "linux")]
    pub(super) async fn open() -> Result<Keyring> {
        let keyring = Keyring::new().await?;
        keyring.unlock().await?;
        Ok(keyring)
    }

    pub(super) async fn read(keyring: &Keyring, service: &str) -> Result<Option<Credential>> {
        for item in keyring.search_items(&[("url", service)]).await? {
            if item.label().await? == LABEL {
                let attributes = item.attributes().await?;
                let username = attributes
                    .get("username")
                    .context("Saved credential has no username")?;
                item.unlock().await?;
                return Ok(Some((username.clone(), item.secret().await?.to_vec())));
            }
        }
        Ok(None)
    }

    pub(super) async fn write(
        keyring: &Keyring,
        service: &str,
        username: &str,
        secret: &[u8],
    ) -> Result<()> {
        keyring
            .create_item(
                LABEL,
                &[("url", service), ("username", username)],
                secret,
                true,
            )
            .await?;
        Ok(())
    }

    pub(super) async fn delete(keyring: &Keyring, service: &str) -> Result<()> {
        for item in keyring.search_items(&[("url", service)]).await? {
            if item.label().await? == LABEL {
                item.delete().await?;
            }
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[cfg(target_os = "linux")]
        #[test]
        #[ignore = "requires the isolated D-Bus fixture in Linux CI"]
        fn portal_fallback_on_private_bus() {
            smol::block_on(async {
                let mode = std::env::var("GOOP_CREDENTIAL_PORTAL_TEST")
                    .expect("Run with the private bus fixture");
                let previous = oo7_previous::Keyring::new().await.unwrap_err().to_string();
                let current = open().await.unwrap_err().to_string();
                if mode == "missing" {
                    assert!(
                        previous.contains("org.freedesktop.portal.Secret"),
                        "{previous}"
                    );
                    assert!(!previous.contains("goop-test-host-reached"), "{previous}");
                    assert!(current.contains("goop-test-host-reached"), "{current}");
                } else {
                    assert_eq!(mode, "denied");
                    assert!(current.contains("goop-test-portal-denied"), "{current}");
                    assert!(!current.contains("goop-test-host-reached"), "{current}");
                }
            });
        }

        #[test]
        fn reads_previous_encrypted_store_and_preserves_client_key_on_logout() {
            smol::block_on(async {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("test.keyring");
                let encryption_key = [17u8; 64];
                let client_key = [29u8; 32];
                let old = oo7_previous::file::UnlockedKeyring::load(
                    &path,
                    oo7_previous::Secret::from(encryption_key.to_vec()),
                )
                .await
                .unwrap();
                old.create_item(
                    LABEL,
                    &[("url", crate::MASTER_KEYRING), ("username", "client")],
                    client_key.to_vec(),
                    true,
                )
                .await
                .unwrap();
                old.create_item(
                    LABEL,
                    &[("url", crate::USER_KEYRING), ("username", "bunker")],
                    b"bunker://test".to_vec(),
                    true,
                )
                .await
                .unwrap();
                drop(old);
                let current =
                    Keyring::sandboxed_with_path(&path, oo7::Secret::blob(encryption_key))
                        .await
                        .unwrap();
                assert_eq!(
                    read(&current, crate::MASTER_KEYRING).await.unwrap(),
                    Some(("client".into(), client_key.to_vec()))
                );
                assert_eq!(
                    read(&current, crate::USER_KEYRING).await.unwrap(),
                    Some(("bunker".into(), b"bunker://test".to_vec()))
                );
                write(&current, crate::USER_KEYRING, "bunker", b"bunker://updated")
                    .await
                    .unwrap();
                drop(current);
                let reopened =
                    Keyring::sandboxed_with_path(&path, oo7::Secret::blob(encryption_key))
                        .await
                        .unwrap();
                assert_eq!(
                    read(&reopened, crate::USER_KEYRING)
                        .await
                        .unwrap()
                        .unwrap()
                        .1,
                    b"bunker://updated"
                );
                delete(&reopened, crate::USER_KEYRING).await.unwrap();
                assert!(
                    read(&reopened, crate::USER_KEYRING)
                        .await
                        .unwrap()
                        .is_none()
                );
                assert_eq!(
                    read(&reopened, crate::MASTER_KEYRING)
                        .await
                        .unwrap()
                        .unwrap()
                        .1,
                    client_key
                );
            });
        }

        #[test]
        fn cannot_read_encrypted_store_with_wrong_key() {
            smol::block_on(async {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("test.keyring");
                let keyring = Keyring::sandboxed_with_path(&path, oo7::Secret::blob([17u8; 64]))
                    .await
                    .unwrap();
                write(
                    &keyring,
                    crate::USER_KEYRING,
                    "bunker",
                    b"private credential",
                )
                .await
                .unwrap();
                drop(keyring);
                let original = std::fs::read(&path).unwrap();
                assert!(
                    !original
                        .windows(18)
                        .any(|window| window == b"private credential")
                );
                assert!(
                    Keyring::sandboxed_with_path(&path, oo7::Secret::blob([18u8; 64]))
                        .await
                        .is_err()
                );
                assert_eq!(std::fs::read(&path).unwrap(), original);
            });
        }

        #[test]
        fn respects_legacy_label_and_only_deletes_requested_service() {
            smol::block_on(async {
                let dir = tempfile::tempdir().unwrap();
                let keyring = Keyring::sandboxed_with_path(
                    dir.path().join("test.keyring"),
                    oo7::Secret::blob([17u8; 64]),
                )
                .await
                .unwrap();
                keyring
                    .create_item(
                        "another app",
                        &[("url", crate::USER_KEYRING), ("username", "other")],
                        b"other secret".as_slice(),
                        true,
                    )
                    .await
                    .unwrap();
                assert!(read(&keyring, crate::USER_KEYRING).await.unwrap().is_none());
                write(&keyring, crate::USER_KEYRING, "bunker", b"bunker://test")
                    .await
                    .unwrap();
                delete(&keyring, crate::USER_KEYRING).await.unwrap();
                let items = keyring
                    .search_items(&[("url", crate::USER_KEYRING)])
                    .await
                    .unwrap();
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].label().await.unwrap(), "another app");
            });
        }
    }
}
