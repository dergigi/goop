use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, anyhow};
use futures::StreamExt;
use nostr_sdk::prelude::*;
use state::UniversalSigner;

use super::{FailedMessage, NewMessage, Signal, extract_rumor};

#[derive(Debug, Default)]
struct QueueState {
    pending: HashSet<EventId>,
    attempted: HashSet<EventId>,
}

struct EnqueuePermit {
    state: Arc<Mutex<QueueState>>,
    id: EventId,
    armed: bool,
}
impl Drop for EnqueuePermit {
    fn drop(&mut self) {
        if self.armed {
            self.state.lock().unwrap().pending.remove(&self.id);
        }
    }
}

const HISTORY_CAPACITY: usize = 256;
const INTERACTIVE_CAPACITY: usize = 64;
const HISTORY_CONCURRENCY: usize = 3;

pub(super) struct DecryptReceivers {
    pub(super) history: flume::Receiver<(Event, bool)>,
    pub(super) interactive: flume::Receiver<(Event, bool)>,
}

/// Reserve one of four signer slots for live messages and explicit retries.
/// History retains three slots so neither workload can starve the other.
#[derive(Debug, Clone)]
pub(super) struct DecryptQueue {
    history_sender: flume::Sender<(Event, bool)>,
    interactive_sender: flume::Sender<(Event, bool)>,
    state: Arc<Mutex<QueueState>>,
    loaded: Arc<AtomicUsize>,
}

impl DecryptQueue {
    pub fn new() -> (Self, DecryptReceivers) {
        let (history_sender, history) = flume::bounded(HISTORY_CAPACITY);
        let (interactive_sender, interactive) = flume::bounded(INTERACTIVE_CAPACITY);
        (
            Self {
                history_sender,
                interactive_sender,
                state: Arc::default(),
                loaded: Arc::default(),
            },
            DecryptReceivers {
                history,
                interactive,
            },
        )
    }

    pub async fn enqueue(&self, event: Event, retry: bool) -> Result<()> {
        self.enqueue_inner(event, retry, true).await
    }

    pub async fn enqueue_live(&self, event: Event) -> Result<()> {
        self.enqueue_inner(event, false, false).await
    }

    async fn enqueue_inner(&self, event: Event, retry: bool, historical: bool) -> Result<()> {
        let id = event.id;
        {
            let mut state = self.state.lock().unwrap();
            if state.pending.contains(&id) || (!retry && state.attempted.contains(&id)) {
                return Ok(());
            }
            state.pending.insert(id);
        }
        let mut permit = EnqueuePermit {
            state: self.state.clone(),
            id,
            armed: true,
        };
        // Retry priority must not turn an old message into a live notification.
        let sender = if retry || !historical {
            &self.interactive_sender
        } else {
            &self.history_sender
        };
        if sender.send_async((event, historical)).await.is_err() {
            return Err(anyhow!("Message loading stopped"));
        }
        permit.armed = false;
        Ok(())
    }

    pub fn pending(&self) -> usize {
        self.state.lock().unwrap().pending.len()
    }
    pub fn loaded(&self) -> usize {
        self.loaded.load(Ordering::Relaxed)
    }

    pub async fn run(
        &self,
        receivers: DecryptReceivers,
        client: Client,
        signer: UniversalSigner,
        signals: flume::Sender<Signal>,
    ) -> Result<()> {
        let decrypt_job = |(event, historical): (Event, bool)| {
            let client = &client;
            let signer = &signer;
            async move {
                let mut result = Err(anyhow!("Signer unavailable"));
                // A transient signer disconnect gets one automatic retry. Further
                // failures stay on disk and can be retried explicitly/reconnected.
                for attempt in 0..2 {
                    result = async_utility::time::timeout(
                        Some(Duration::from_secs(30)),
                        extract_rumor(client, signer, &event),
                    )
                    .await
                    .unwrap_or_else(|| Err(anyhow!("Signer decryption timed out")));
                    if result.is_ok() {
                        break;
                    }
                    if attempt == 0 {
                        async_utility::time::sleep(Duration::from_secs(2)).await;
                    }
                }
                (event, historical, result)
            }
        };
        let history = receivers
            .history
            .into_stream()
            .map(decrypt_job)
            .buffer_unordered(HISTORY_CONCURRENCY);
        let interactive = receivers
            .interactive
            .into_stream()
            .map(decrypt_job)
            .buffer_unordered(1);
        let mut jobs = futures::stream::select(interactive, history);
        while let Some((event, historical, result)) = jobs.next().await {
            {
                let mut state = self.state.lock().unwrap();
                state.pending.remove(&event.id);
                state.attempted.insert(event.id);
            }
            let result = match result {
                Ok(rumor) => {
                    self.loaded.fetch_add(1, Ordering::Relaxed);
                    let mut message = NewMessage::new(event.id, rumor);
                    message.historical = historical;
                    Ok(message)
                }
                Err(error) => Err(FailedMessage::new(&event, error.to_string())),
            };
            signals
                .send_async(Signal::Decrypted(event.id, result))
                .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn cancelling_a_blocked_enqueue_does_not_prevent_retry() {
        let (queue, receiver) = DecryptQueue::new();
        for i in 0..HISTORY_CAPACITY {
            let event = EventBuilder::new(Kind::GiftWrap, i.to_string())
                .finalize(&Keys::generate())
                .unwrap();
            queue.enqueue(event, false).await.unwrap();
        }
        let event = EventBuilder::new(Kind::GiftWrap, "retry me")
            .finalize(&Keys::generate())
            .unwrap();
        let cloned_queue = queue.clone();
        let cloned_event = event.clone();
        let blocked = tokio::spawn(async move { cloned_queue.enqueue(cloned_event, false).await });
        tokio::time::timeout(Duration::from_secs(1), async {
            while queue.pending() != 257 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        blocked.abort();
        let _ = blocked.await;
        assert_eq!(queue.pending(), 256);
        receiver.history.recv_async().await.unwrap();
        queue.enqueue(event.clone(), false).await.unwrap();
        assert!(
            receiver
                .history
                .try_iter()
                .any(|(queued, _)| queued.id == event.id)
        );
    }

    #[tokio::test]
    async fn failed_decryption_can_be_retried_without_duplicate_jobs() {
        let client = Client::builder()
            .database(nostr_memory::MemoryDatabase::unbounded())
            .build();
        let signer = UniversalSigner::new(Keys::generate());
        let event = EventBuilder::new(Kind::GiftWrap, "invalid ciphertext")
            .finalize(&Keys::generate())
            .unwrap();
        let (queue, receiver) = DecryptQueue::new();
        let (tx, rx) = flume::unbounded();
        let worker_queue = queue.clone();
        let worker =
            tokio::spawn(async move { worker_queue.run(receiver, client, signer, tx).await });
        queue.enqueue(event.clone(), false).await.unwrap();
        queue.enqueue(event.clone(), false).await.unwrap();
        let first = tokio::time::timeout(Duration::from_secs(5), rx.recv_async())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(first, Signal::Decrypted(id, Err(_)) if id == event.id));
        assert_eq!(queue.pending(), 0);
        assert!(rx.is_empty());
        queue.enqueue(event.clone(), false).await.unwrap();
        assert_eq!(queue.pending(), 0);
        queue.enqueue(event.clone(), true).await.unwrap();
        let second = tokio::time::timeout(Duration::from_secs(5), rx.recv_async())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(second, Signal::Decrypted(id, Err(_)) if id == event.id));
        assert_eq!(queue.pending(), 0);
        worker.abort();
    }
    #[derive(Debug)]
    struct GatedSigner {
        keys: Keys,
        blocked: HashSet<PublicKey>,
        started: flume::Sender<()>,
        release: flume::Receiver<()>,
        active: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
    }

    impl AsyncGetPublicKey for GatedSigner {
        type Error = <Keys as AsyncGetPublicKey>::Error;
        fn get_public_key_async(
            &self,
        ) -> futures::future::BoxFuture<'_, std::result::Result<PublicKey, Self::Error>> {
            self.keys.get_public_key_async()
        }
    }

    impl AsyncSignEvent for GatedSigner {
        type Error = <Keys as AsyncSignEvent>::Error;
        fn sign_event_async(
            &self,
            event: UnsignedEvent,
        ) -> futures::future::BoxFuture<'_, std::result::Result<Event, Self::Error>> {
            self.keys.sign_event_async(event)
        }
    }

    impl AsyncNip44 for GatedSigner {
        type Error = <Keys as AsyncNip44>::Error;
        fn nip44_encrypt_async<'a>(
            &'a self,
            key: &'a PublicKey,
            content: &'a str,
        ) -> futures::future::BoxFuture<'a, std::result::Result<String, Self::Error>> {
            self.keys.nip44_encrypt_async(key, content)
        }
        fn nip44_decrypt_async<'a>(
            &'a self,
            key: &'a PublicKey,
            content: &'a str,
        ) -> futures::future::BoxFuture<'a, std::result::Result<String, Self::Error>> {
            Box::pin(async move {
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.peak.fetch_max(active, Ordering::SeqCst);
                if self.blocked.contains(key) {
                    self.started.send_async(()).await.unwrap();
                    self.release.recv_async().await.unwrap();
                }
                let result = self.keys.nip44_decrypt_async(key, content).await;
                self.active.fetch_sub(1, Ordering::SeqCst);
                result
            })
        }
    }

    async fn gift_wrap(sender: &Keys, recipient: &Keys, content: &str) -> Event {
        let rumor = EventBuilder::new(Kind::PrivateDirectMessage, content)
            .tag(Tag::public_key(recipient.public_key()))
            .finalize_unsigned(sender.public_key());
        nip59::GiftWrapBuilder::new(recipient.public_key(), rumor)
            .finalize_async(sender)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn live_messages_and_retries_finish_while_history_signer_requests_are_blocked() {
        tokio::time::timeout(Duration::from_secs(5), async {
            let client = Client::builder()
                .database(nostr_memory::MemoryDatabase::unbounded())
                .build();
            let sender = Keys::generate();
            let recipient = Keys::generate();
            let (queue, receivers) = DecryptQueue::new();
            let mut blocked = HashSet::new();
            for i in 0..12 {
                let event = gift_wrap(&sender, &recipient, &format!("history {i}")).await;
                blocked.insert(event.pubkey);
                queue.enqueue(event, false).await.unwrap();
            }
            let live = gift_wrap(&sender, &recipient, "live").await;
            let retry = gift_wrap(&sender, &recipient, "retry").await;
            let (started_tx, started_rx) = flume::unbounded();
            let (release_tx, release_rx) = flume::unbounded();
            let peak = Arc::new(AtomicUsize::new(0));
            let signer = UniversalSigner::new(GatedSigner {
                keys: recipient,
                blocked,
                started: started_tx,
                release: release_rx,
                active: Arc::default(),
                peak: peak.clone(),
            });
            let (tx, rx) = flume::unbounded();
            let worker_queue = queue.clone();
            let worker =
                tokio::spawn(async move { worker_queue.run(receivers, client, signer, tx).await });
            // All historical slots are occupied by signer requests that cannot finish.
            for _ in 0..HISTORY_CONCURRENCY {
                started_rx.recv_async().await.unwrap();
            }
            queue.enqueue_live(live.clone()).await.unwrap();
            queue.enqueue_live(live.clone()).await.unwrap();
            queue.enqueue(retry.clone(), true).await.unwrap();
            for (expected, historical) in [(live.id, false), (retry.id, true)] {
                match rx.recv_async().await.unwrap() {
                    Signal::Decrypted(id, Ok(message)) => {
                        assert_eq!(id, expected);
                        assert_eq!(message.historical, historical);
                    }
                    other => panic!("Unexpected result: {other:?}"),
                }
            }
            assert_eq!(queue.pending(), 12);
            assert_eq!(queue.loaded(), 2);
            assert!(rx.is_empty());
            // Release history and verify it still makes progress without duplicates.
            for _ in 0..12 {
                release_tx.send_async(()).await.unwrap();
            }
            let mut completed = HashSet::new();
            for _ in 0..12 {
                match rx.recv_async().await.unwrap() {
                    Signal::Decrypted(id, Ok(message)) => {
                        assert!(message.historical);
                        assert!(completed.insert(id));
                    }
                    other => panic!("Unexpected result: {other:?}"),
                }
            }
            assert_eq!(queue.pending(), 0);
            assert_eq!(queue.loaded(), 14);
            assert_eq!(peak.load(Ordering::SeqCst), 4);
            worker.abort();
            let _ = worker.await;
        })
        .await
        .expect("live messages must not wait for blocked history");
    }

    #[tokio::test]
    async fn a_full_history_queue_does_not_block_live_enqueue() {
        let (queue, receivers) = DecryptQueue::new();
        let keys = Keys::generate();
        for i in 0..HISTORY_CAPACITY {
            let event = EventBuilder::new(Kind::GiftWrap, i.to_string())
                .finalize(&keys)
                .unwrap();
            queue.enqueue(event, false).await.unwrap();
        }
        let live = EventBuilder::new(Kind::GiftWrap, "live")
            .finalize(&keys)
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), queue.enqueue_live(live.clone()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(receivers.interactive.try_recv().unwrap().0.id, live.id);
        assert_eq!(receivers.history.len(), HISTORY_CAPACITY);
    }
}
