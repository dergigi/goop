//! Controlled external-signer refusal used by restart/retry regressions.
use futures::future::BoxFuture;
use nostr_sdk::prelude::*;
use state::SignerFailure;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[derive(Debug, Clone)]
pub(crate) struct RefusingSigner {
    pub keys: Keys,
    pub refused: Arc<AtomicBool>,
    pub calls: Arc<AtomicUsize>,
    failure: SignerFailure,
}
impl RefusingSigner {
    pub fn new(keys: Keys) -> Self {
        Self {
            keys,
            refused: Arc::new(AtomicBool::new(true)),
            calls: Arc::default(),
            failure: SignerFailure::Rejected,
        }
    }
    pub fn with_failure(mut self, failure: SignerFailure) -> Self {
        self.failure = failure;
        self
    }
    fn check(&self) -> Result<(), SignerFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.refused.load(Ordering::SeqCst) {
            Err(self.failure)
        } else {
            Ok(())
        }
    }
}
impl AsyncGetPublicKey for RefusingSigner {
    type Error = SignerFailure;
    fn get_public_key_async(&self) -> BoxFuture<'_, Result<PublicKey, Self::Error>> {
        Box::pin(async { Ok(self.keys.public_key()) })
    }
}
impl AsyncSignEvent for RefusingSigner {
    type Error = SignerFailure;
    fn sign_event_async(&self, event: UnsignedEvent) -> BoxFuture<'_, Result<Event, Self::Error>> {
        Box::pin(async move {
            self.check()?;
            self.keys
                .sign_event_async(event)
                .await
                .map_err(|_| SignerFailure::Other)
        })
    }
}
impl AsyncNip44 for RefusingSigner {
    type Error = SignerFailure;
    fn nip44_encrypt_async<'a>(
        &'a self,
        key: &'a PublicKey,
        content: &'a str,
    ) -> BoxFuture<'a, Result<String, Self::Error>> {
        Box::pin(async move {
            self.check()?;
            self.keys
                .nip44_encrypt_async(key, content)
                .await
                .map_err(|_| SignerFailure::Other)
        })
    }
    fn nip44_decrypt_async<'a>(
        &'a self,
        key: &'a PublicKey,
        content: &'a str,
    ) -> BoxFuture<'a, Result<String, Self::Error>> {
        Box::pin(async move {
            self.check()?;
            self.keys
                .nip44_decrypt_async(key, content)
                .await
                .map_err(|_| SignerFailure::Other)
        })
    }
}
