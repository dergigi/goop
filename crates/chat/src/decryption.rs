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

/// Bound both network backpressure and concurrent remote-signer requests.
#[derive(Debug, Clone)]
pub(super) struct DecryptQueue {
    sender: flume::Sender<(Event, bool)>,
    state: Arc<Mutex<QueueState>>,
    loaded: Arc<AtomicUsize>,
}

impl DecryptQueue {
    pub fn new() -> (Self, flume::Receiver<(Event, bool)>) {
        let (sender, receiver) = flume::bounded(256);
        (
            Self {
                sender,
                state: Arc::default(),
                loaded: Arc::default(),
            },
            receiver,
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
        if self.sender.send_async((event, historical)).await.is_err() {
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
        receiver: flume::Receiver<(Event, bool)>,
        client: Client,
        signer: UniversalSigner,
        signals: flume::Sender<Signal>,
    ) -> Result<()> {
        let mut jobs = receiver
            .into_stream()
            .map(|(event, historical)| {
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
            })
            .buffer_unordered(4);
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
        for i in 0..256 {
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
        receiver.recv_async().await.unwrap();
        queue.enqueue(event.clone(), false).await.unwrap();
        assert!(receiver.try_iter().any(|(queued, _)| queued.id == event.id));
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
}
