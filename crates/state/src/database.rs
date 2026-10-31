//! Bound LMDB reads before they reach its blocking worker pool.
use std::{collections::BTreeSet, future::Future, path::Path, pin::Pin, sync::Arc};

use futures::channel::oneshot;
use nostr_database::error::Error;
use nostr_lmdb::{
    NostrLmdb,
    prelude::{
        DatabaseEventStatus, Event, EventId, Features, Filter, NostrDatabase, SaveEventStatus,
        Timestamp,
    },
};
use smol::lock::Semaphore;

type ResultFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'a>>;

// LMDB defaults to 126 slots. Leave ample headroom and avoid starting hundreds
// of blocking queries when relay traffic and history loading overlap.
const CONCURRENT_READS: usize = 16;

#[derive(Debug, Clone)]
struct ReadGate(Arc<Semaphore>);

impl ReadGate {
    fn new(limit: usize) -> Self {
        Self(Arc::new(Semaphore::new(limit)))
    }

    async fn run<T, F>(&self, operation: F) -> Result<T, Error>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, Error>> + Send + 'static,
    {
        let permit = self.0.acquire_arc().await;
        let (send, receive) = oneshot::channel();
        // Dropping the caller does not stop LMDB's spawn_blocking query. Keep
        // its permit until the actual operation finishes, even after a timeout.
        smol::spawn(async move {
            let result = operation.await;
            drop(permit);
            let _ = send.send(result);
        })
        .detach();
        receive.await.map_err(Error::other)?
    }
}

#[derive(Debug)]
pub(super) struct Database {
    inner: Arc<NostrLmdb>,
    reads: ReadGate,
}

impl Database {
    pub(super) async fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        Ok(Self {
            inner: Arc::new(NostrLmdb::open(path).await?),
            reads: ReadGate::new(CONCURRENT_READS),
        })
    }
}

impl NostrDatabase for Database {
    fn backend(&self) -> &'static str {
        self.inner.backend()
    }
    fn features(&self) -> Features {
        self.inner.features()
    }
    fn save_event<'a>(&'a self, event: &'a Event) -> ResultFuture<'a, SaveEventStatus> {
        self.inner.save_event(event)
    }
    fn check_id<'a>(&'a self, id: &'a EventId) -> ResultFuture<'a, DatabaseEventStatus> {
        let db = self.inner.clone();
        let id = *id;
        Box::pin(self.reads.run(async move { db.check_id(&id).await }))
    }
    fn event_by_id<'a>(&'a self, id: &'a EventId) -> ResultFuture<'a, Option<Event>> {
        let db = self.inner.clone();
        let id = *id;
        Box::pin(self.reads.run(async move { db.event_by_id(&id).await }))
    }
    fn count(&self, filter: Filter) -> ResultFuture<'_, usize> {
        let db = self.inner.clone();
        Box::pin(self.reads.run(async move { db.count(filter).await }))
    }
    fn query(&self, filter: Filter) -> ResultFuture<'_, BTreeSet<Event>> {
        let db = self.inner.clone();
        Box::pin(self.reads.run(async move { db.query(filter).await }))
    }
    fn negentropy_items(&self, filter: Filter) -> ResultFuture<'_, Vec<(EventId, Timestamp)>> {
        let db = self.inner.clone();
        Box::pin(
            self.reads
                .run(async move { db.negentropy_items(filter).await }),
        )
    }
    fn delete(&self, filter: Filter) -> ResultFuture<'_, ()> {
        self.inner.delete(filter)
    }
    fn wipe(&self) -> ResultFuture<'_, ()> {
        self.inner.wipe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::future::join_all;
    use nostr_lmdb::prelude::{EventBuilder, FinalizeEvent, Keys, Kind};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[tokio::test]
    async fn bursts_wait_instead_of_exceeding_reader_budget() {
        let gate = ReadGate::new(4);
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let results = join_all((0..512).map(|_| {
            let active = active.clone();
            let peak = peak.clone();
            gate.run(async move {
                let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(count, Ordering::SeqCst);
                smol::Timer::after(Duration::from_millis(1)).await;
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            })
        }))
        .await;
        assert!(results.into_iter().all(|result| result.is_ok()));
        assert_eq!(peak.load(Ordering::SeqCst), 4);
        assert_eq!(active.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn cancellation_keeps_slot_until_work_finishes_and_errors_release_it() {
        let gate = ReadGate::new(1);
        let (started, ready) = oneshot::channel();
        let (finish, finished) = oneshot::channel();
        let first_gate = gate.clone();
        let caller = tokio::spawn(async move {
            first_gate
                .run(async move {
                    let _ = started.send(());
                    let _ = finished.await;
                    Err::<(), _>(Error::other(std::io::Error::other("test failure")))
                })
                .await
        });
        ready.await.unwrap();
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        assert!(gate.0.try_acquire_arc().is_none());
        let next = gate.run(async { Ok(42) });
        futures::pin_mut!(next);
        assert!(futures::poll!(&mut next).is_pending());
        finish.send(()).unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), next)
                .await
                .unwrap()
                .unwrap(),
            42
        );
        assert!(gate.0.try_acquire_arc().is_some());
        let failure = gate
            .run(async { Err::<(), _>(Error::other(std::io::Error::other("query failed"))) })
            .await;
        assert!(failure.unwrap_err().to_string().contains("query failed"));
        assert_eq!(gate.run(async { Ok(7) }).await.unwrap(), 7);
    }

    #[tokio::test]
    async fn lmdb_read_methods_share_budget_and_writes_still_work() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database {
            inner: Arc::new(
                NostrLmdb::builder(dir.path())
                    .max_readers(2)
                    .build()
                    .await
                    .unwrap(),
            ),
            reads: ReadGate::new(1),
        };
        let event = EventBuilder::new(Kind::TextNote, "reader regression")
            .finalize(&Keys::generate())
            .unwrap();
        assert!(db.save_event(&event).await.unwrap().is_success());
        let operations = (0..256).map(|i| {
            let db = &db;
            let id = event.id;
            async move {
                match i % 5 {
                    0 => assert_eq!(db.count(Filter::new()).await.unwrap(), 1),
                    1 => assert_eq!(db.query(Filter::new()).await.unwrap().len(), 1),
                    2 => assert_eq!(db.event_by_id(&id).await.unwrap().unwrap().id, id),
                    3 => assert_eq!(db.check_id(&id).await.unwrap(), DatabaseEventStatus::Saved),
                    _ => assert_eq!(db.negentropy_items(Filter::new()).await.unwrap().len(), 1),
                }
            }
        });
        tokio::time::timeout(Duration::from_secs(10), join_all(operations))
            .await
            .unwrap();
        db.delete(Filter::new().id(event.id)).await.unwrap();
        assert_eq!(db.count(Filter::new()).await.unwrap(), 0);
        db.wipe().await.unwrap();
    }
}
