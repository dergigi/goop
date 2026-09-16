//! Ordered, coalesced local JSON writes. No disk I/O or serialization in save().
use std::{
    any::Any,
    collections::{BTreeMap, VecDeque},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, LazyLock, Mutex},
    time::Duration,
};

use futures::channel::oneshot;
use serde::{Serialize, de::DeserializeOwned};

type SaveResult = Result<(), String>;
type Writer = dyn Fn(&Path, &dyn Snapshot) -> io::Result<()> + Send + Sync;

trait Snapshot: Send + Sync {
    fn json(&self) -> io::Result<Vec<u8>>;
    fn as_any(&self) -> &dyn Any;
}
impl<T: Serialize + Send + Sync + 'static> Snapshot for T {
    fn json(&self) -> io::Result<Vec<u8>> {
        serde_json::to_vec(self).map_err(io::Error::other)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct Pending {
    revision: u64,
    value: Arc<dyn Snapshot>,
    queued: bool,
    in_flight: bool,
    error: Option<String>,
}
struct Waiter {
    remaining: BTreeMap<PathBuf, u64>,
    error: Option<String>,
    reply: oneshot::Sender<SaveResult>,
}
#[derive(Default)]
struct State {
    revision: u64,
    pending: BTreeMap<PathBuf, Pending>,
    queue: VecDeque<PathBuf>,
    waiters: Vec<Waiter>,
    notifications: BTreeMap<PathBuf, String>,
    stopping: bool,
    closing: bool,
}
#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}
struct Owner(Arc<Shared>);
impl Drop for Owner {
    fn drop(&mut self) {
        self.0.state.lock().unwrap().stopping = true;
        self.0.wake.notify_one();
    }
}

#[derive(Clone)]
pub struct Persistence(Arc<Owner>);
impl std::fmt::Debug for Persistence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Persistence")
    }
}

pub fn global() -> &'static Persistence {
    static WRITER: LazyLock<Persistence> =
        LazyLock::new(|| Persistence::new().expect("Could not start local-state writer"));
    &WRITER
}

impl Persistence {
    pub fn new() -> io::Result<Self> {
        Self::with_writer(Arc::new(|path, value| atomic_write(path, &value.json()?)))
    }

    fn with_writer(writer: Arc<Writer>) -> io::Result<Self> {
        let shared = Arc::new(Shared::default());
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("goop-local-state".into())
            .spawn(move || run(worker, writer))?;
        Ok(Self(Arc::new(Owner(shared))))
    }

    /// Reopening an account sees its newest accepted snapshot even if its disk
    /// write is still pending. Successful snapshots are released from memory.
    pub fn load<T: DeserializeOwned + Clone + Default + 'static>(
        &self,
        path: &Path,
    ) -> io::Result<T> {
        let value = self
            .0
            .0
            .state
            .lock()
            .unwrap()
            .pending
            .get(path)
            .map(|p| p.value.clone());
        if let Some(value) = value {
            return value
                .as_any()
                .downcast_ref::<T>()
                .cloned()
                .ok_or_else(|| io::Error::other("Local-state type mismatch"));
        }
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(T::default()),
            Err(error) => Err(error),
        }
    }

    /// Ownership of a snapshot makes serialization safe on the worker. Repeated
    /// updates replace a queued snapshot, never grow a per-file task chain.
    pub fn save<T: Serialize + Send + Sync + 'static>(
        &self,
        path: &Path,
        value: T,
    ) -> io::Result<()> {
        let mut state = self.0.0.state.lock().unwrap();
        if state.stopping || state.closing {
            return Err(io::Error::other("Local-state writer has stopped"));
        }
        state.revision += 1;
        let revision = state.revision;
        let queued = state.pending.get(path).is_some_and(|p| p.queued);
        let error = state.pending.get(path).and_then(|p| p.error.clone());
        state.pending.insert(
            path.to_owned(),
            Pending {
                revision,
                value: Arc::new(value),
                queued: true,
                in_flight: false,
                error,
            },
        );
        if !queued {
            state.queue.push_back(path.to_owned());
        }
        self.0.0.wake.notify_one();
        Ok(())
    }

    /// Wait until every snapshot accepted before this call has reached disk (or
    /// a newer replacement has). Failed files get another attempt, not a silent OK.
    pub fn flush(&self) -> impl Future<Output = SaveResult> + Send + 'static {
        let (reply, result) = oneshot::channel();
        let mut state = self.0.0.state.lock().unwrap();
        let remaining = state
            .pending
            .iter()
            .map(|(path, p)| (path.clone(), p.revision))
            .collect::<BTreeMap<_, _>>();
        if remaining.is_empty() {
            let _ = reply.send(Ok(()));
        } else {
            retry_failed(&mut state);
            state.waiters.push(Waiter {
                remaining,
                error: None,
                reply,
            });
            self.0.0.wake.notify_one();
        }
        async move {
            result
                .await
                .unwrap_or_else(|_| Err("Local-state writer stopped before saving".into()))
        }
    }

    /// Last-resort shutdown barrier. The worker never needs the UI executor.
    /// Normal Quit should use flush().await while the window remains responsive.
    pub fn flush_blocking(&self) -> SaveResult {
        futures::executor::block_on(self.flush())
    }

    /// Reject late producers before the final shutdown barrier so they cannot
    /// enqueue more state after the flush snapshot was taken.
    pub fn shutdown_blocking(&self) -> SaveResult {
        self.0.0.state.lock().unwrap().closing = true;
        self.flush_blocking()
    }

    /// Background failures are delivered to the UI once per changed error/file.
    pub fn take_errors(&self) -> Vec<String> {
        std::mem::take(&mut self.0.0.state.lock().unwrap().notifications)
            .into_values()
            .collect()
    }
}

fn retry_failed(state: &mut State) {
    for (path, pending) in &mut state.pending {
        if pending.error.is_some() && !pending.queued && !pending.in_flight {
            pending.queued = true;
            state.queue.push_back(path.clone());
        }
    }
}

fn run(shared: Arc<Shared>, writer: Arc<Writer>) {
    loop {
        let (path, revision, value) = {
            let mut state = shared.state.lock().unwrap();
            loop {
                if let Some(path) = state.queue.pop_front() {
                    let pending = state.pending.get_mut(&path).unwrap();
                    pending.queued = false;
                    pending.in_flight = true;
                    break (path, pending.revision, pending.value.clone());
                }
                if state.stopping {
                    return;
                }
                let (next, timeout) = shared
                    .wake
                    .wait_timeout(state, Duration::from_secs(5))
                    .unwrap();
                state = next;
                if timeout.timed_out() {
                    retry_failed(&mut state);
                }
            }
        };
        // Never hold the scheduler lock while serializing, writing, or syncing.
        let result = writer(&path, &*value).map_err(|error| error.to_string());
        let mut state = shared.state.lock().unwrap();
        let current = state
            .pending
            .get(&path)
            .is_some_and(|p| p.revision == revision);
        match &result {
            Ok(()) => {
                if current {
                    state.pending.remove(&path);
                    state.notifications.remove(&path);
                }
            }
            Err(error) if current => {
                let pending = state.pending.get_mut(&path).unwrap();
                let changed = pending.error.as_ref() != Some(error);
                pending.in_flight = false;
                pending.error = Some(error.clone());
                if changed {
                    state.notifications.insert(path.clone(), error.clone());
                }
            }
            Err(_) => {} // A newer queued snapshot will get its own attempt.
        }
        let mut waiting = Vec::new();
        for mut waiter in state.waiters.drain(..) {
            if waiter
                .remaining
                .get(&path)
                .is_some_and(|target| *target <= revision)
            {
                match &result {
                    Ok(()) => {
                        waiter.remaining.remove(&path);
                    }
                    Err(error) if current => {
                        waiter.remaining.remove(&path);
                        waiter.error.get_or_insert_with(|| error.clone());
                    }
                    Err(_) => {}
                }
            }
            if waiter.remaining.is_empty() {
                let _ = waiter.reply.send(waiter.error.map_or(Ok(()), Err));
            } else {
                waiting.push(waiter);
            }
        }
        state.waiters = waiting;
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("Local-state path has no parent"))?;
    std::fs::create_dir_all(dir)?;
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    };

    #[test]
    fn coalesces_slow_writes_and_reopens_latest_state_without_waiting_for_disk() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("alice.json");
        let second = dir.path().join("bob.json");
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release = Mutex::new(release_rx);
        let count = Arc::new(AtomicUsize::new(0));
        let calls = count.clone();
        let persistence = Persistence::with_writer(Arc::new(move |path, value| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                started_tx.send(()).unwrap();
                release.lock().unwrap().recv().unwrap();
            }
            atomic_write(path, &value.json()?)
        }))
        .unwrap();
        persistence.save(&first, 0_u64).unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        for value in 1..=1_000_u64 {
            persistence.save(&first, value).unwrap();
        }
        persistence.save(&second, 42_u64).unwrap();
        assert_eq!(persistence.load::<u64>(&first).unwrap(), 1_000);
        assert_eq!(persistence.load::<u64>(&second).unwrap(), 42);
        {
            let state = persistence.0.0.state.lock().unwrap();
            assert_eq!(state.pending.len(), 2);
            assert_eq!(state.queue.len(), 2);
        }
        let flush = persistence.flush();
        release_tx.send(()).unwrap();
        futures::executor::block_on(flush).unwrap();
        assert_eq!(std::fs::read_to_string(first).unwrap(), "1000");
        assert_eq!(std::fs::read_to_string(second).unwrap(), "42");
        assert_eq!(count.load(Ordering::SeqCst), 3);
        assert!(persistence.0.0.state.lock().unwrap().pending.is_empty());
    }

    #[test]
    fn failed_save_preserves_previous_file_and_retries_the_latest_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        atomic_write(&path, b"7").unwrap();
        let failing = Arc::new(AtomicBool::new(true));
        let fail = failing.clone();
        let persistence = Persistence::with_writer(Arc::new(move |path, value| {
            if fail.load(Ordering::SeqCst) {
                return Err(io::Error::other("disk unavailable"));
            }
            atomic_write(path, &value.json()?)
        }))
        .unwrap();
        persistence.save(&path, 8_u64).unwrap();
        assert_eq!(
            persistence.flush_blocking().unwrap_err(),
            "disk unavailable"
        );
        assert_eq!(persistence.take_errors(), vec!["disk unavailable"]);
        assert!(persistence.flush_blocking().is_err());
        assert!(persistence.take_errors().is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "7");
        assert_eq!(persistence.load::<u64>(&path).unwrap(), 8);
        persistence.save(&path, 9_u64).unwrap();
        assert!(persistence.flush_blocking().is_err());
        failing.store(false, Ordering::SeqCst);
        persistence.flush_blocking().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "9");
        assert!(persistence.0.0.state.lock().unwrap().pending.is_empty());
    }

    #[test]
    fn simultaneous_flushes_do_not_queue_a_retry_that_is_already_in_flight() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release = Mutex::new(release_rx);
        let persistence = Persistence::with_writer(Arc::new(move |path, value| {
            if count.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(io::Error::other("retry me"));
            }
            started_tx.send(()).unwrap();
            release.lock().unwrap().recv().unwrap();
            atomic_write(path, &value.json()?)
        }))
        .unwrap();
        persistence.save(&path, 5_u64).unwrap();
        assert!(persistence.flush_blocking().is_err());
        let first = persistence.flush();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let second = persistence.flush();
        assert!(persistence.0.0.state.lock().unwrap().queue.is_empty());
        release_tx.send(()).unwrap();
        futures::executor::block_on(first).unwrap();
        futures::executor::block_on(second).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn shutdown_drains_accepted_changes_and_rejects_late_producers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let persistence = Persistence::new().unwrap();
        persistence.save(&path, 10_u64).unwrap();
        persistence.shutdown_blocking().unwrap();
        assert!(persistence.save(&path, 11_u64).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "10");
    }

    #[test]
    fn an_obsolete_failed_write_does_not_discard_its_newer_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release = Mutex::new(release_rx);
        let first = AtomicBool::new(true);
        let persistence = Persistence::with_writer(Arc::new(move |path, value| {
            if first.swap(false, Ordering::SeqCst) {
                started_tx.send(()).unwrap();
                release.lock().unwrap().recv().unwrap();
                return Err(io::Error::other("old snapshot failed"));
            }
            atomic_write(path, &value.json()?)
        }))
        .unwrap();
        persistence.save(&path, 1_u64).unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let earlier_barrier = persistence.flush();
        persistence.save(&path, 2_u64).unwrap();
        let latest_barrier = persistence.flush();
        release_tx.send(()).unwrap();
        futures::executor::block_on(earlier_barrier).unwrap();
        futures::executor::block_on(latest_barrier).unwrap();
        assert!(persistence.take_errors().is_empty());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "2");
    }

    #[test]
    fn a_failed_file_does_not_end_shutdown_before_other_files_finish() {
        use futures::FutureExt;
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.json");
        let good = dir.path().join("good.json");
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release = Mutex::new(release_rx);
        let persistence = Persistence::with_writer(Arc::new(move |path, value| {
            if path.file_name().unwrap() == "bad.json" {
                return Err(io::Error::other("bad file"));
            }
            started_tx.send(()).unwrap();
            release.lock().unwrap().recv().unwrap();
            atomic_write(path, &value.json()?)
        }))
        .unwrap();
        persistence.save(&bad, 1_u64).unwrap();
        persistence.save(&good, 2_u64).unwrap();
        let mut flush = Box::pin(persistence.flush());
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(flush.as_mut().now_or_never().is_none());
        release_tx.send(()).unwrap();
        assert_eq!(futures::executor::block_on(flush).unwrap_err(), "bad file");
        assert_eq!(std::fs::read_to_string(good).unwrap(), "2");
    }

    #[test]
    fn dropping_the_owner_does_not_cancel_accepted_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let persistence = Persistence::new().unwrap();
        persistence.save(&path, 25_u64).unwrap();
        let flush = persistence.flush();
        drop(persistence);
        futures::executor::block_on(flush).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "25");
    }

    #[test]
    fn serialization_runs_on_worker_and_files_are_private() {
        struct Probe(Arc<Mutex<Option<std::thread::ThreadId>>>);
        impl Serialize for Probe {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                *self.0.lock().unwrap() = Some(std::thread::current().id());
                serializer.serialize_u64(1)
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let thread = Arc::new(Mutex::new(None));
        let persistence = Persistence::new().unwrap();
        persistence.save(&path, Probe(thread.clone())).unwrap();
        persistence.flush_blocking().unwrap();
        assert_ne!(thread.lock().unwrap().unwrap(), std::thread::current().id());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o077,
                0
            );
        }
    }
}
