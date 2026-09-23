//! Keep the device's signer connection key in memory after a successful load.
//! Serialize callers so overlapping QR invitations cannot create different keys.
use std::future::Future;

use anyhow::Result;
use futures::lock::Mutex;
use nostr::prelude::Keys;

#[derive(Default)]
pub(crate) struct ConnectionKey(Mutex<Option<Keys>>);

impl std::fmt::Debug for ConnectionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConnectionKey { .. }")
    }
}

impl ConnectionKey {
    pub(crate) async fn get_or_load<F, Fut>(&self, load: F) -> Result<Keys>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Keys>>,
    {
        let mut cached = self.0.lock().await;
        if let Some(keys) = cached.as_ref() {
            return Ok(keys.clone());
        }
        // The loader must finish saving a newly created key before returning.
        // Failed reads/writes are not cached, so an explicit retry can succeed.
        let keys = load().await?;
        *cached = Some(keys.clone());
        Ok(keys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn overlapping_and_later_requests_only_load_once() {
        let cache = ConnectionKey::default();
        let loads = AtomicUsize::new(0);
        let load = || async {
            loads.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            Ok(Keys::generate())
        };
        let (first, second) = futures::join!(cache.get_or_load(load), cache.get_or_load(load));
        let third = cache.get_or_load(load).await.unwrap();
        assert_eq!(first.unwrap().public_key(), third.public_key());
        assert_eq!(second.unwrap().public_key(), third.public_key());
        assert_eq!(loads.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failed_load_can_be_retried() {
        let cache = ConnectionKey::default();
        assert!(cache.get_or_load(|| async { anyhow::bail!("Access denied") }).await.is_err());
        let keys = Keys::generate();
        let loaded = cache.get_or_load(|| async { Ok(keys.clone()) }).await.unwrap();
        assert_eq!(loaded.public_key(), keys.public_key());
    }
}
