use std::time::Duration;

use anyhow::{Result, ensure};
use gpui::{App, AppContext, Context, Entity, Global, Task, Window};
use semver::Version;
use serde::Deserialize;
use smol::io::AsyncReadExt;

const RELEASE_API: &str = "https://api.github.com/repos/dergigi/goop/releases/latest";
const RELEASE_PAGE: &str = "https://github.com/dergigi/goop/releases/tag/";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvailableRelease {
    pub version: Version,
    pub url: String,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
}

fn newer_release(bytes: &[u8], current: &Version) -> Result<Option<AvailableRelease>> {
    let release: Release = serde_json::from_slice(bytes)?;
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let version = Version::parse(
        release
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&release.tag_name),
    )?;
    if !version.pre.is_empty() || version.cmp_precedence(current).is_le() {
        return Ok(None);
    }
    Ok(Some(AvailableRelease {
        version,
        url: format!("{RELEASE_PAGE}{}", release.tag_name),
    }))
}

struct GlobalReleaseChecker(Entity<ReleaseChecker>);
impl Global for GlobalReleaseChecker {}

/// Checks release metadata only. Installation is left to the user or package manager.
pub struct ReleaseChecker {
    available: Option<AvailableRelease>,
    checking: bool,
}

pub fn init(window: &mut Window, cx: &mut App) {
    let checker = cx.new(|cx: &mut Context<ReleaseChecker>| {
        cx.defer_in(window, |_, _, cx| {
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_secs(120))
                    .await;
                loop {
                    let Ok(task) = this.update(cx, |this, cx| this.check(cx)) else {
                        break;
                    };
                    if let Some(task) = task {
                        let _ = task.await;
                    }
                    cx.background_executor()
                        .timer(Duration::from_secs(6 * 60 * 60))
                        .await;
                }
            })
            .detach();
        });
        ReleaseChecker {
            available: None,
            checking: false,
        }
    });
    cx.set_global(GlobalReleaseChecker(checker));
}

impl ReleaseChecker {
    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalReleaseChecker>()
            .map(|global| global.0.clone())
    }

    pub fn available(&self) -> Option<&AvailableRelease> {
        self.available.as_ref()
    }

    /// Returns no task if a request is already running.
    pub fn check(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<Task<Result<Option<AvailableRelease>>>> {
        if self.checking {
            return None;
        }
        self.checking = true;
        let client = cx.http_client();
        let executor = cx.background_executor().clone();
        let request = cx.background_spawn(async move {
            smol::future::or(
                async move {
                    let request = gpui::http_client::Request::builder()
                        .uri(RELEASE_API)
                        .header("User-Agent", concat!("Goop/", env!("CARGO_PKG_VERSION")))
                        .header("Accept", "application/vnd.github+json")
                        .body(().into())?;
                    let mut response = client.send(request).await?;
                    ensure!(
                        response.status().is_success(),
                        "Release check returned {}",
                        response.status()
                    );
                    let mut bytes = Vec::new();
                    const MAX_BYTES: u64 = 1024 * 1024;
                    response
                        .body_mut()
                        .take(MAX_BYTES + 1)
                        .read_to_end(&mut bytes)
                        .await?;
                    ensure!(
                        bytes.len() as u64 <= MAX_BYTES,
                        "Release response is too large"
                    );
                    newer_release(&bytes, &Version::parse(env!("CARGO_PKG_VERSION"))?)
                },
                async move {
                    executor.timer(Duration::from_secs(20)).await;
                    anyhow::bail!("Release check timed out")
                },
            )
            .await
        });
        Some(cx.spawn(async move |this, cx| {
            let result = request.await;
            if let Err(error) = &result {
                log::warn!("Could not check for updates: {error}");
            }
            this.update(cx, |this, cx| {
                this.checking = false;
                // A temporary network failure must not hide a known update.
                if let Ok(available) = &result {
                    this.available = available.clone();
                }
                cx.notify();
            })?;
            result
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(
        tag: &str,
        draft: bool,
        prerelease: bool,
        current: &str,
    ) -> Result<Option<AvailableRelease>> {
        newer_release(
            serde_json::json!({"tag_name": tag, "draft": draft, "prerelease": prerelease})
                .to_string()
                .as_bytes(),
            &Version::parse(current).unwrap(),
        )
    }

    #[test]
    fn compares_versions_numerically_and_builds_release_link() {
        let found = release("v2.12.0", false, false, "2.9.0").unwrap().unwrap();
        assert_eq!(found.version, Version::parse("2.12.0").unwrap());
        assert_eq!(
            found.url,
            "https://github.com/dergigi/goop/releases/tag/v2.12.0"
        );
        assert!(release("v2.9.0", false, false, "2.11.0").unwrap().is_none());
        assert!(
            release("v2.11.0", false, false, "2.11.0")
                .unwrap()
                .is_none()
        );
        assert!(
            release("v2.11.0+build2", false, false, "2.11.0+build1")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn ignores_unpublished_and_prerelease_versions() {
        assert!(release("v3.0.0", true, false, "2.11.0").unwrap().is_none());
        assert!(release("v3.0.0", false, true, "2.11.0").unwrap().is_none());
        assert!(
            release("v3.0.0-beta.1", false, false, "2.11.0")
                .unwrap()
                .is_none()
        );
        assert!(
            release("v3.0.0", false, false, "3.0.0-beta.1")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn malformed_metadata_is_an_error() {
        assert!(release("garbage", false, false, "2.11.0").is_err());
        assert!(
            newer_release(
                br#"{"message":"API rate limit exceeded"}"#,
                &Version::new(2, 11, 0)
            )
            .is_err()
        );
    }
}
