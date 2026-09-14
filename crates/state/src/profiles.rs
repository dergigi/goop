use std::collections::BTreeSet;

use instant::Duration;
use nostr_sdk::prelude::*;

/// Discover each author's outbox before requesting their latest profile.
/// Authors without a known outbox fall back to the client's read relays.
pub async fn subscribe_profiles(
    client: &Client,
    public_keys: impl IntoIterator<Item = PublicKey>,
) -> anyhow::Result<()> {
    let authors: BTreeSet<_> = public_keys.into_iter().collect();
    if authors.is_empty() {
        return Ok(());
    }

    // Separate filters preserve fallback for unknown authors in a mixed batch
    // and request the latest event per person, rather than a shared batch limit.
    let filters: Vec<_> = authors
        .into_iter()
        .map(|author| Filter::new().kind(Kind::Metadata).author(author).limit(1))
        .collect();
    let opts = SubscribeAutoCloseOptions::default()
        .exit_policy(ReqExitPolicy::ExitOnEOSE)
        .timeout(Some(Duration::from_secs(10)));

    // Passing filters (not a relay map) enables the SDK's on-demand discovery.
    client.subscribe(filters).close_on(opts).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use nostr_gossip_memory::prelude::*;
    use nostr_sdk::local_relay::MockRelay;

    #[tokio::test]
    async fn discovers_outbox_and_keeps_fallback_for_another_author() {
        let bootstrap = MockRelay::run().await.unwrap();
        let outbox = MockRelay::run().await.unwrap();
        let author = Keys::generate();
        let fallback_author = Keys::generate();
        let profile = EventBuilder::new(
            Kind::Metadata,
            r#"{"name":"Outbox only","picture":"https://example.com/avatar.png"}"#,
        )
        .finalize(&author)
        .unwrap();
        let fallback_profile = EventBuilder::new(Kind::Metadata, r#"{"name":"Bootstrap only"}"#)
            .finalize(&fallback_author)
            .unwrap();
        let relays = EventBuilder::new(Kind::RelayList, "")
            .tag(Tag::custom(
                "r",
                [outbox.url().await.to_string(), "write".into()],
            ))
            .finalize(&author)
            .unwrap();
        outbox.add_event(profile.clone()).await.unwrap();
        bootstrap.add_event(relays).await.unwrap();
        bootstrap.add_event(fallback_profile.clone()).await.unwrap();

        let client = Client::builder()
            .database(nostr_memory::MemoryDatabase::unbounded())
            .gossip(NostrGossipMemory::unbounded())
            .gossip_config(
                GossipConfig::default()
                    // Only the test uses unencrypted loopback relays.
                    .allowed(GossipAllowedRelays {
                        local: true,
                        without_tls: true,
                        ..Default::default()
                    })
                    .sync_initial_timeout(Duration::from_millis(500))
                    .sync_idle_timeout(Duration::from_secs(1))
                    .fetch_timeout(Duration::from_secs(2))
                    .no_background_refresh(),
            )
            .build();
        client
            .add_relay(bootstrap.url().await)
            .and_connect()
            .await
            .unwrap();
        let mut notifications = client.notifications();
        tokio::time::timeout(Duration::from_secs(20), async {
            subscribe_profiles(&client, [author.public_key(), fallback_author.public_key()])
                .await
                .unwrap();
            let mut expected = BTreeSet::from([profile.id, fallback_profile.id]);
            while let Some(notification) = notifications.next().await {
                if let ClientNotification::Event { event, .. } = notification {
                    expected.remove(&event.id);
                    if expected.is_empty() {
                        return;
                    }
                }
            }
            panic!("notification stream ended before both profiles arrived");
        })
        .await
        .expect("both outbox and fallback profiles should arrive");
        client.shutdown().await;
    }
}
