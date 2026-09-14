//! Resumable history from the account's current inbox relays only.
use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use futures::StreamExt;
use nostr_sdk::prelude::*;
use serde::{Deserialize, Serialize};

use super::{LOCAL_KEYS, Signal};

const PAGE_SIZE: usize = 256;
const MAX_PAGE_SIZE: usize = 4096;
const WRAP_OVERLAP: u64 = 2 * 24 * 60 * 60;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelayHistory {
    pub received: usize,
    pub pages: usize,
    pub oldest: Option<Timestamp>,
    pub done: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Checkpoint {
    revision: u64,
    /// Time of the last completed head scan (not a message's randomized timestamp).
    head: u64,
    until: u64,
    complete: bool,
}

fn checkpoint_key(user: PublicKey, relay: &RelayUrl) -> String {
    format!("goop-history-v1:{user}:{relay}")
}

async fn read_checkpoint(client: &Client, key: &str) -> Result<Option<Checkpoint>> {
    Ok(client
        .database()
        .query(
            Filter::new()
                .kind(Kind::ApplicationSpecificData)
                .identifier(key),
        )
        .await?
        .into_iter()
        .filter_map(|event| serde_json::from_str::<Checkpoint>(&event.content).ok())
        .max_by_key(|checkpoint| checkpoint.revision))
}

async fn save_checkpoint(client: &Client, key: &str, checkpoint: &mut Checkpoint) -> Result<()> {
    let old = client
        .database()
        .query(
            Filter::new()
                .kind(Kind::ApplicationSpecificData)
                .identifier(key),
        )
        .await?;
    checkpoint.revision += 1;
    // Replaceable events with equal timestamps may reject the newer checkpoint.
    // These events are local only; advance monotonically even within one second.
    let created_at = old
        .iter()
        .map(|event| event.created_at.as_secs().saturating_add(1))
        .max()
        .unwrap_or(0)
        .max(Timestamp::now().as_secs());
    let event = EventBuilder::new(
        Kind::ApplicationSpecificData,
        serde_json::to_string(checkpoint)?,
    )
    .custom_created_at(Timestamp::from(created_at))
    .tag(Tag::identifier(key))
    .finalize_async(&*LOCAL_KEYS)
    .await?;
    // Persist the replacement before deleting previous revisions.
    if !client.database().save_event(&event).await?.is_success() {
        bail!("Could not persist history progress");
    }
    let ids: Vec<_> = old.into_iter().map(|event| event.id).collect();
    if !ids.is_empty() {
        client.database().delete(Filter::new().ids(ids)).await?;
    }
    Ok(())
}

struct PageSubscription(Client, SubscriptionId);
impl Drop for PageSubscription {
    fn drop(&mut self) {
        let client = self.0.clone();
        let id = self.1.clone();
        // Also release relay subscriptions if a scan is cancelled on sign-out.
        async_utility::task::spawn(async move {
            let _ = client.unsubscribe(&id).await;
        });
    }
}

/// Require an explicit EOSE. A timeout/disconnection is never an empty page.
async fn fetch_page(
    client: &Client,
    relay: &RelayUrl,
    user: PublicKey,
    until: u64,
    since: Option<u64>,
    limit: usize,
) -> Result<Vec<Event>> {
    let id = SubscriptionId::new(format!("goop-history-{}", SubscriptionId::generate()));
    let _subscription = PageSubscription(client.clone(), id.clone());
    let mut notifications = client.notifications();
    let mut filter = Filter::new()
        .kind(Kind::GiftWrap)
        .pubkey(user)
        .until(Timestamp::from(until))
        .limit(limit);
    if let Some(since) = since {
        filter = filter.since(Timestamp::from(since));
    }
    let result = async_utility::time::timeout(Some(Duration::from_secs(30)), async {
        let output = client
            .subscribe(ReqTarget::single(relay, [filter]))
            .with_id(id.clone())
            .await?;
        if output.success.is_empty() {
            bail!("Relay did not accept the history subscription");
        }
        let mut events = BTreeMap::new();
        while let Some(notification) = notifications.next().await {
            if let ClientNotification::Message { relay_url, message } = notification {
                if &relay_url != relay {
                    continue;
                }
                match *message {
                    RelayMessage::Event {
                        subscription_id,
                        event,
                    } if subscription_id.as_ref() == &id => {
                        events.insert(event.id, event.into_owned());
                    }
                    RelayMessage::EndOfStoredEvents(sub_id) if sub_id.as_ref() == &id => {
                        return Ok(events.into_values().collect());
                    }
                    RelayMessage::Closed {
                        subscription_id,
                        message,
                    } if subscription_id.as_ref() == &id => bail!("{message}"),
                    _ => {}
                }
            }
        }
        Err(anyhow!("Relay notification stream closed"))
    })
    .await;
    // Subscription is manual so timeouts cannot masquerade as successful EOSE.
    let _ = client.unsubscribe(&id).await;
    result.ok_or_else(|| anyhow!("History request timed out; progress retained"))?
}

/// Keep the oldest timestamp inclusive on the next page to avoid cutting through
/// a group of messages sharing a timestamp. Widen saturated boundary requests.
fn advance(page: &[Event], until: u64, limit: usize) -> Result<(Option<u64>, usize)> {
    let Some(oldest) = page.iter().map(|e| e.created_at.as_secs()).min() else {
        return Ok((None, PAGE_SIZE));
    };
    if oldest < until {
        return Ok((Some(oldest), PAGE_SIZE));
    }
    if page.len() >= limit {
        if limit >= MAX_PAGE_SIZE {
            bail!("Relay history is stuck at a timestamp boundary; retry later");
        }
        return Ok((Some(until), limit * 2));
    }
    Ok((until.checked_sub(1), PAGE_SIZE))
}

pub(super) async fn scan_relay(
    client: &Client,
    user: PublicKey,
    relay: RelayUrl,
    queue: &super::DecryptQueue,
    signals: &flume::Sender<Signal>,
    force: bool,
) -> Result<()> {
    let mut progress = RelayHistory::default();
    signals
        .send_async(Signal::History(relay.clone(), progress.clone()))
        .await?;
    let result: Result<()> = async {
        client.add_relay(&relay).and_connect().await?;
        let now = Timestamp::now().as_secs();
        let key = checkpoint_key(user, &relay);
        let previous = read_checkpoint(client, &key).await?;
        let mut checkpoint = previous.clone().unwrap_or(Checkpoint {
            revision: 0,
            head: now,
            until: now,
            complete: false,
        });
        // Catch up arrivals since the last launch before resuming the old cursor.
        // Gift-wrap timestamps are randomized, so overlap the previous head.
        if previous.is_some() {
            let since = checkpoint.head.saturating_sub(WRAP_OVERLAP);
            let mut cursor = Some(now);
            let mut limit = PAGE_SIZE;
            while let Some(until) = cursor {
                if until < since {
                    break;
                }
                let page = fetch_page(client, &relay, user, until, Some(since), limit).await?;
                ingest(
                    client,
                    page.as_slice(),
                    &relay,
                    queue,
                    signals,
                    &mut progress,
                )
                .await?;
                (cursor, limit) = advance(&page, until, limit)?;
            }
            checkpoint.head = now;
            save_checkpoint(client, &key, &mut checkpoint).await?;
        }
        if force {
            checkpoint.complete = false;
        }
        let mut limit = PAGE_SIZE;
        while !checkpoint.complete {
            let page = fetch_page(client, &relay, user, checkpoint.until, None, limit).await?;
            ingest(client, &page, &relay, queue, signals, &mut progress).await?;
            let (next, next_limit) = advance(&page, checkpoint.until, limit)?;
            checkpoint.complete = next.is_none();
            if let Some(next) = next {
                checkpoint.until = next;
            }
            limit = next_limit;
            save_checkpoint(client, &key, &mut checkpoint).await?;
        }
        Ok(())
    }
    .await;
    progress.done = result.is_ok();
    progress.error = result.as_ref().err().map(ToString::to_string);
    signals.send_async(Signal::History(relay, progress)).await?;
    result
}

async fn ingest(
    client: &Client,
    page: &[Event],
    relay: &RelayUrl,
    queue: &super::DecryptQueue,
    signals: &flume::Sender<Signal>,
    progress: &mut RelayHistory,
) -> Result<()> {
    for event in page {
        // Raw ciphertext survives signer failure and application restarts.
        client.database().save_event(event).await?;
        queue.enqueue(event.clone(), false).await?;
    }
    progress.pages += 1;
    progress.received += page.len();
    if let Some(oldest) = page.iter().map(|event| event.created_at).min() {
        progress.oldest = Some(
            progress
                .oldest
                .map_or(oldest, |previous| previous.min(oldest)),
        );
    }
    signals
        .send_async(Signal::History(relay.clone(), progress.clone()))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(time: u64) -> Event {
        EventBuilder::new(Kind::GiftWrap, "ciphertext")
            .custom_created_at(Timestamp::from(time))
            .finalize(&Keys::generate())
            .unwrap()
    }
    #[test]
    fn short_pages_still_continue_and_overlap_oldest_timestamp() {
        assert_eq!(
            advance(&[event(90), event(80)], 100, PAGE_SIZE).unwrap(),
            (Some(80), PAGE_SIZE)
        );
        assert_eq!(
            advance(&[event(80)], 80, PAGE_SIZE).unwrap(),
            (Some(79), PAGE_SIZE)
        );
        assert_eq!(advance(&[], 79, PAGE_SIZE).unwrap(), (None, PAGE_SIZE));
    }
    #[test]
    fn full_timestamp_boundary_is_widened_not_skipped() {
        assert_eq!(
            advance(&[event(80), event(80)], 80, 2).unwrap(),
            (Some(80), 4)
        );
        assert!(advance(&vec![event(80); MAX_PAGE_SIZE], 80, MAX_PAGE_SIZE).is_err());
    }
    #[test]
    fn checkpoints_are_account_and_relay_specific() {
        let a = Keys::generate().public_key();
        let b = Keys::generate().public_key();
        let relay = RelayUrl::parse("wss://example.com").unwrap();
        assert_ne!(checkpoint_key(a, &relay), checkpoint_key(b, &relay));
        assert_ne!(
            checkpoint_key(a, &relay),
            checkpoint_key(a, &RelayUrl::parse("wss://other.example.com").unwrap())
        );
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use nostr_sdk::local_relay::LocalRelay;

    #[tokio::test]
    async fn backfills_past_relay_cap_and_resumes_from_persisted_cursor() {
        let relay = LocalRelay::builder()
            .max_filter_limit(3)
            .max_query_results(3)
            .build();
        relay.run().await.unwrap();
        let url = relay.url().await;
        let client = Client::builder()
            .database(nostr_memory::MemoryDatabase::unbounded())
            .build();
        let user = Keys::generate().public_key();
        let mut expected = std::collections::BTreeSet::new();
        for time in [100, 100, 101, 102, 103, 104, 105, 106] {
            let event = EventBuilder::new(Kind::GiftWrap, "ciphertext")
                .tag(Tag::public_key(user))
                .custom_created_at(Timestamp::from(time))
                .finalize(&Keys::generate())
                .unwrap();
            expected.insert(event.id);
            relay.add_event(event).await.unwrap();
        }
        let (queue, receiver) = super::super::DecryptQueue::new();
        let (signals, _rx) = flume::unbounded();
        // Simulate a restart partway through history.
        let key = checkpoint_key(user, &url);
        let mut checkpoint = Checkpoint {
            revision: 0,
            head: 106,
            until: 103,
            complete: false,
        };
        save_checkpoint(&client, &key, &mut checkpoint)
            .await
            .unwrap();
        tokio::time::timeout(
            Duration::from_secs(10),
            scan_relay(&client, user, url.clone(), &queue, &signals, false),
        )
        .await
        .unwrap()
        .unwrap();
        let received: std::collections::BTreeSet<_> = receiver
            .history
            .try_iter()
            .map(|(event, _)| event.id)
            .collect();
        assert_eq!(received, expected);
        assert!(
            read_checkpoint(&client, &key)
                .await
                .unwrap()
                .unwrap()
                .complete
        );
        assert_eq!(
            client
                .database()
                .query(Filter::new().kind(Kind::GiftWrap).pubkey(user))
                .await
                .unwrap()
                .len(),
            expected.len()
        );
        // Repeating the scan cannot enqueue duplicate decryptions.
        scan_relay(&client, user, url, &queue, &signals, true)
            .await
            .unwrap();
        assert!(receiver.history.is_empty());
        assert!(receiver.interactive.is_empty());
        client.shutdown().await;
        relay.shutdown();
    }
}
