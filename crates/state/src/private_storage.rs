//! NIP-37 private-storage relay lists (kind 10013).
use anyhow::{Result, ensure};
use nostr_sdk::prelude::*;
use std::{collections::BTreeSet, time::Duration};

use crate::UniversalSigner;

pub const RELAY_LIST_KIND: Kind = Kind::Custom(10013);

pub async fn latest(client: &Client, owner: PublicKey) -> Result<Option<Event>> {
    let filter = Filter::new().author(owner).kind(RELAY_LIST_KIND).limit(1);
    let cached = client.database().query(filter.clone()).await?;
    let remote = client
        .fetch_events(filter)
        .timeout(Duration::from_secs(10))
        .await;
    let remote = match remote {
        Ok(events) => events,
        Err(_) if !cached.is_empty() => BTreeSet::new(),
        Err(error) => return Err(error.into()),
    };
    Ok(cached.into_iter().chain(remote).max_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| b.id.cmp(&a.id))
    }))
}

pub async fn decode(
    event: &Event,
    owner: PublicKey,
    signer: &UniversalSigner,
) -> Result<Vec<RelayUrl>> {
    ensure!(
        event.kind == RELAY_LIST_KIND && event.pubkey == owner,
        "Invalid private-storage relay list"
    );
    event.verify()?;
    let plaintext = signer.nip44_decrypt_async(&owner, &event.content).await?;
    let tags: Vec<Vec<String>> = serde_json::from_str(&plaintext)?;
    Ok(tags
        .iter()
        .filter(|tag| tag.first().is_some_and(|name| name == "relay"))
        .filter_map(|tag| tag.get(1).and_then(|url| RelayUrl::parse(url).ok()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect())
}

pub async fn build(
    owner: PublicKey,
    relays: &[RelayUrl],
    created_at: Timestamp,
    signer: &UniversalSigner,
) -> Result<Event> {
    ensure!(
        signer.get_public_key_async().await? == owner,
        "Signer changed"
    );
    let tags: Vec<_> = relays
        .iter()
        .map(|url| vec!["relay".to_owned(), url.to_string()])
        .collect();
    let content = signer
        .nip44_encrypt_async(&owner, &serde_json::to_string(&tags)?)
        .await?;
    Ok(EventBuilder::new(RELAY_LIST_KIND, content)
        .custom_created_at(created_at)
        .finalize_async(signer)
        .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn private_list_round_trips_and_empty_list_disables_sync() {
        let keys = Keys::generate();
        let owner = keys.public_key();
        let signer = UniversalSigner::new(keys.clone());
        let urls = vec![RelayUrl::parse("wss://private.example.com").unwrap()];
        let event = build(owner, &urls, Timestamp::now(), &signer)
            .await
            .unwrap();
        assert!(event.tags.is_empty());
        assert!(!event.content.contains("private.example.com"));
        assert_eq!(decode(&event, owner, &signer).await.unwrap(), urls);
        assert!(
            decode(&event, Keys::generate().public_key(), &signer)
                .await
                .is_err()
        );
        let empty = build(owner, &[], Timestamp::now(), &signer).await.unwrap();
        assert!(decode(&empty, owner, &signer).await.unwrap().is_empty());
        assert!(
            build(
                Keys::generate().public_key(),
                &urls,
                Timestamp::now(),
                &signer
            )
            .await
            .is_err()
        );
    }
}
