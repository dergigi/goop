use std::path::PathBuf;

use anyhow::{Error, anyhow};
use gpui::AsyncApp;
use instant::Duration;

use crate::NostrRegistry;
#[cfg(not(target_arch = "wasm32"))]
use gpui_tokio::Tokio;
use mime_guess::from_path;
use nostr_blossom::prelude::*;
use nostr_sdk::prelude::*;

#[cfg(not(target_arch = "wasm32"))]
pub async fn upload(fallback: Url, path: PathBuf, cx: &AsyncApp) -> Result<Url, Error> {
    let content_type = from_path(&path).first_or_octet_stream().to_string();
    let data = smol::fs::read(path).await?;
    let (client, user, signer) = cx.update(|cx| {
        let registry = NostrRegistry::global(cx);
        let registry = registry.read(cx);
        (
            registry.client(),
            registry.current_user(),
            registry.signer(),
        )
    });
    let user = user.ok_or_else(|| anyhow!("Sign in before uploading media"))?;

    Tokio::spawn(cx, async move {
        let servers = upload_servers(load_media_servers(&client, user).await, fallback);
        try_upload_servers(servers, |server| {
            let data = data.clone();
            let content_type = content_type.clone();
            let signer = signer.clone();
            async move {
                if signer.get_public_key_async().await? != user {
                    return Err(anyhow!("Account changed during upload; please try again"));
                }
                let client = BlossomClient::new(server);
                let blob = client
                    .upload_blob(data, Some(content_type), None, Some(&signer))
                    .await?;
                Ok(blob.url)
            }
        })
        .await
    })
    .await
    .map_err(|e| anyhow!("Upload error: {e}"))?
}

#[cfg(target_arch = "wasm32")]
pub async fn upload(_server: Url, _path: PathBuf, _cx: &AsyncApp) -> Result<Url, Error> {
    Err(anyhow!("File upload not supported on web"))
}

/// Load the newest BUD-03 list, retaining cached preferences when relays are unavailable.
pub(crate) async fn load_media_servers(client: &Client, user: PublicKey) -> Vec<Url> {
    let filter = Filter::new()
        .author(user)
        .kind(Kind::Custom(10063))
        .limit(1);
    let mut newest = client
        .database()
        .query(filter.clone())
        .await
        .ok()
        .and_then(|events| events.into_iter().next());
    match client
        .fetch_events(filter)
        .timeout(Duration::from_secs(10))
        .await
    {
        Ok(events) => {
            for event in events {
                if newest.as_ref().is_none_or(|old| {
                    event.created_at > old.created_at
                        || (event.created_at == old.created_at && event.id < old.id)
                }) {
                    newest = Some(event);
                }
            }
        }
        Err(error) => log::warn!("Could not refresh media servers: {error}"),
    }
    newest
        .as_ref()
        .map(media_servers_from_event)
        .unwrap_or_default()
}

fn media_servers_from_event(event: &Event) -> Vec<Url> {
    let mut servers = Vec::new();
    for tag in event.tags.iter() {
        let fields = tag.as_slice();
        if fields.first().map(String::as_str) != Some("server") {
            continue;
        }
        if let Some(server) = fields.get(1).and_then(|url| Url::parse(url).ok()) {
            if matches!(server.scheme(), "http" | "https")
                && server.host_str().is_some()
                && server.username().is_empty()
                && server.password().is_none()
                && !servers.contains(&server)
            {
                servers.push(server);
            }
        }
    }
    servers
}

fn upload_servers(servers: Vec<Url>, fallback: Url) -> Vec<Url> {
    if servers.is_empty() {
        vec![fallback]
    } else {
        servers
    }
}

#[cfg(not(target_arch = "wasm32"))]
async fn try_upload_servers<F, Fut>(servers: Vec<Url>, mut upload: F) -> Result<Url, Error>
where
    F: FnMut(Url) -> Fut,
    Fut: std::future::Future<Output = Result<Url, Error>>,
{
    use futures::{FutureExt, select_biased};
    let mut errors = Vec::new();
    for server in servers {
        let attempt = upload(server.clone()).fuse();
        let timeout = FutureExt::fuse(smol::Timer::after(Duration::from_secs(120)));
        futures::pin_mut!(attempt, timeout);
        let result = select_biased! {
            result = attempt => result,
            _ = timeout => Err(anyhow!("Upload timed out")),
        };
        match result {
            Ok(url) => return Ok(url),
            Err(error) => errors.push(format!("{server}: {error}")),
        }
    }
    Err(anyhow!("Media upload failed: {}", errors.join("; ")))
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn server_tags_keep_preference_order_and_skip_invalid_duplicates() {
        let event = EventBuilder::new(Kind::Custom(10063), "ignored")
            .tags([
                Tag::custom("server", ["https://primary.example"]),
                Tag::custom("server", ["file:///tmp/media"]),
                Tag::custom("server", ["https://primary.example/"]),
                Tag::custom("server", ["not a URL"]),
                Tag::custom("server", ["https://user:password@example.com"]),
                Tag::custom("other", ["https://ignored.example"]),
                Tag::custom("server", ["https://secondary.example/"]),
                Tag::custom("server", Vec::<String>::new()),
            ])
            .finalize(&Keys::generate())
            .unwrap();
        let servers = media_servers_from_event(&event);
        assert_eq!(
            servers.iter().map(Url::as_str).collect::<Vec<_>>(),
            ["https://primary.example/", "https://secondary.example/"]
        );
    }

    #[test]
    fn fallback_is_only_used_without_published_servers() {
        let fallback = Url::parse("https://blossom.band").unwrap();
        let primary = Url::parse("https://primary.example").unwrap();
        assert_eq!(
            upload_servers(vec![], fallback.clone()),
            vec![fallback.clone()]
        );
        assert_eq!(
            upload_servers(vec![primary.clone()], fallback),
            vec![primary]
        );
    }

    #[test]
    fn upload_tries_servers_in_order_and_stops_at_success() {
        let servers = [
            "https://first.example",
            "https://second.example",
            "https://third.example",
        ]
        .map(|url| Url::parse(url).unwrap())
        .to_vec();
        let mut attempts = Vec::new();
        let result = smol::block_on(try_upload_servers(servers.clone(), |server| {
            attempts.push(server.clone());
            std::future::ready(if attempts.len() == 1 {
                Err(anyhow!("Unavailable"))
            } else {
                Ok(server)
            })
        }))
        .unwrap();
        assert_eq!(attempts, servers[..2]);
        assert_eq!(result, servers[1]);
    }

    #[test]
    fn upload_reports_all_failures_without_using_an_unlisted_server() {
        let server = Url::parse("https://private.example").unwrap();
        let result = smol::block_on(try_upload_servers(vec![server], |_| {
            std::future::ready(Err(anyhow!("Forbidden")))
        }))
        .unwrap_err();
        assert!(
            result
                .to_string()
                .contains("https://private.example/: Forbidden")
        );
    }
}
