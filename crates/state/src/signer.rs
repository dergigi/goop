use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use nostr_connect::client::AuthUrlHandler;
use nostr_sdk::prelude::*;

/// Stable retry decisions derived from signer error types, not arbitrary relay text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignerFailure {
    Rejected,
    Cancelled,
    Disconnected,
    Timeout,
    Other,
}

impl fmt::Display for SignerFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Rejected => "signer rejected the operation",
            Self::Cancelled => "signer operation cancelled",
            Self::Disconnected => "signer disconnected",
            Self::Timeout => "signer operation timed out",
            Self::Other => "signer operation failed",
        })
    }
}
impl Error for SignerFailure {}

impl SignerFailure {
    pub fn classify(mut error: &(dyn Error + 'static)) -> Self {
        loop {
            if let Some(failure) = error.downcast_ref::<Self>() {
                return *failure;
            }
            if let Some(error) = error.downcast_ref::<nostr_connect::error::Error>() {
                use nostr_connect::error::ErrorKind;
                match error.kind() {
                    ErrorKind::Rejected => return Self::refusal(&error.to_string()),
                    ErrorKind::Timeout => return Self::Timeout,
                    _ => {}
                }
            }
            if let Some(error) = error.downcast_ref::<nostr_sdk::error::Error>() {
                use nostr_sdk::error::ErrorKind;
                match error.kind() {
                    ErrorKind::Timeout => return Self::Timeout,
                    ErrorKind::Transport | ErrorKind::State => return Self::Disconnected,
                    _ => {}
                }
            }
            if let Some(error) = error.downcast_ref::<std::io::Error>() {
                use std::io::ErrorKind;
                match error.kind() {
                    ErrorKind::TimedOut => return Self::Timeout,
                    ErrorKind::ConnectionAborted
                    | ErrorKind::ConnectionReset
                    | ErrorKind::NotConnected
                    | ErrorKind::BrokenPipe => return Self::Disconnected,
                    _ => {}
                }
            }
            match error.source() {
                Some(source) => error = source,
                None => return Self::Other,
            }
        }
    }

    fn refusal(message: &str) -> Self {
        // These transports return cancellation as a remote rejection with free text.
        if message.to_ascii_lowercase().contains("cancel") {
            Self::Cancelled
        } else {
            Self::Rejected
        }
    }

    pub fn requires_retry(self) -> bool {
        matches!(self, Self::Rejected | Self::Cancelled)
    }
}

#[derive(Debug)]
pub struct UniversalSignerError(Box<dyn Error + Send + Sync + 'static>);

impl fmt::Display for UniversalSignerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Error for UniversalSignerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&*self.0)
    }
}

impl UniversalSignerError {
    pub fn new<E>(err: E) -> Self
    where
        E: Error + Send + Sync + 'static,
    {
        UniversalSignerError(Box::new(err))
    }
}

#[derive(Clone, Debug)]
pub struct UniversalSigner {
    inner: Arc<RwLock<Arc<SignerSession>>>,
}

#[derive(Debug)]
struct SignerSession {
    signer: RwLock<Option<Arc<dyn InnerSigner>>>,
    active: AtomicBool,
}

impl SignerSession {
    fn signer(&self) -> Result<Arc<dyn InnerSigner>, UniversalSignerError> {
        self.ensure_active()?;
        self.signer
            .read()
            .expect("RwLock poisoned")
            .clone()
            .ok_or_else(|| UniversalSignerError::new(SignerFailure::Disconnected))
    }

    fn ensure_active(&self) -> Result<(), UniversalSignerError> {
        if self.active.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(UniversalSignerError::new(SignerFailure::Disconnected))
        }
    }
}

impl UniversalSigner {
    pub fn new<T>(signer: T) -> Self
    where
        T: AsyncGetPublicKey + AsyncSignEvent + AsyncNip44 + 'static,
        <T as AsyncGetPublicKey>::Error: Error + Send + Sync + 'static,
        <T as AsyncSignEvent>::Error: Error + Send + Sync + 'static,
        <T as AsyncNip44>::Error: Error + Send + Sync + 'static,
    {
        Self {
            inner: Arc::new(RwLock::new(Arc::new(SignerSession {
                signer: RwLock::new(Some(Arc::new(InnerSignerImpl(signer)))),
                active: AtomicBool::new(true),
            }))),
        }
    }

    /// Freeze the current signer for account-bound work. Later account switches
    /// update the shared signer without changing this snapshot's identity.
    pub fn snapshot(&self) -> Self {
        Self {
            inner: Arc::new(RwLock::new(
                self.inner.read().expect("RwLock poisoned").clone(),
            )),
        }
    }

    /// Revoke the session, including frozen snapshots held by account workers.
    pub fn disconnect(&self) {
        let session = self.inner.read().expect("RwLock poisoned");
        session.active.store(false, Ordering::SeqCst);
        session.signer.write().expect("RwLock poisoned").take();
    }

    /// Swap the inner signer in-place. All clones see the new signer.
    pub fn swap_inner<T>(&self, new_signer: T)
    where
        T: AsyncGetPublicKey + AsyncSignEvent + AsyncNip44 + 'static,
        <T as AsyncGetPublicKey>::Error: Error + Send + Sync + 'static,
        <T as AsyncSignEvent>::Error: Error + Send + Sync + 'static,
        <T as AsyncNip44>::Error: Error + Send + Sync + 'static,
    {
        *self.inner.write().expect("RwLock poisoned") = Arc::new(SignerSession {
            signer: RwLock::new(Some(Arc::new(InnerSignerImpl(new_signer)))),
            active: AtomicBool::new(true),
        });
    }
}

trait InnerSigner: fmt::Debug + Send + Sync + 'static {
    fn get_public_key_async(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<PublicKey, UniversalSignerError>> + Send + '_>>;
    fn sign_event_async(
        &self,
        unsigned: UnsignedEvent,
    ) -> Pin<Box<dyn Future<Output = Result<Event, UniversalSignerError>> + Send + '_>>;
    fn nip44_encrypt_async<'a>(
        &'a self,
        public_key: &'a PublicKey,
        content: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, UniversalSignerError>> + Send + 'a>>;
    fn nip44_decrypt_async<'a>(
        &'a self,
        public_key: &'a PublicKey,
        payload: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, UniversalSignerError>> + Send + 'a>>;
}

// Pairing may wait indefinitely, but individual signer operations must finish
// or return a typed timeout so queued messages can recover.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

async fn signer_request<T, E: Error + Send + Sync + 'static>(
    request: impl Future<Output = Result<T, E>> + Send,
    timeout: std::time::Duration,
) -> Result<T, UniversalSignerError> {
    use futures::{FutureExt, future::{select, Either}};
    match select(request.boxed(), async move { smol::Timer::after(timeout).await }.boxed()).await {
        Either::Left((result, _)) => result.map_err(UniversalSignerError::new),
        Either::Right(_) => Err(UniversalSignerError::new(SignerFailure::Timeout)),
    }
}

#[derive(Debug)]
struct InnerSignerImpl<T>(T);

impl<T> InnerSigner for InnerSignerImpl<T>
where
    T: AsyncGetPublicKey + AsyncSignEvent + AsyncNip44 + Send + Sync + 'static,
    <T as AsyncGetPublicKey>::Error: Error + Send + Sync + 'static,
    <T as AsyncSignEvent>::Error: Error + Send + Sync + 'static,
    <T as AsyncNip44>::Error: Error + Send + Sync + 'static,
{
    fn get_public_key_async(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<PublicKey, UniversalSignerError>> + Send + '_>> {
        Box::pin(async move {
            signer_request(AsyncGetPublicKey::get_public_key_async(&self.0), REQUEST_TIMEOUT).await
        })
    }

    fn sign_event_async(
        &self,
        unsigned: UnsignedEvent,
    ) -> Pin<Box<dyn Future<Output = Result<Event, UniversalSignerError>> + Send + '_>> {
        Box::pin(async move {
            signer_request(AsyncSignEvent::sign_event_async(&self.0, unsigned), REQUEST_TIMEOUT).await
        })
    }

    fn nip44_encrypt_async<'a>(
        &'a self,
        public_key: &'a PublicKey,
        content: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, UniversalSignerError>> + Send + 'a>> {
        Box::pin(async move {
            signer_request(AsyncNip44::nip44_encrypt_async(&self.0, public_key, content), REQUEST_TIMEOUT).await
        })
    }

    fn nip44_decrypt_async<'a>(
        &'a self,
        public_key: &'a PublicKey,
        payload: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, UniversalSignerError>> + Send + 'a>> {
        Box::pin(async move {
            signer_request(AsyncNip44::nip44_decrypt_async(&self.0, public_key, payload), REQUEST_TIMEOUT).await
        })
    }
}

impl AsyncGetPublicKey for UniversalSigner {
    type Error = UniversalSignerError;

    fn get_public_key_async(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<PublicKey, Self::Error>> + Send + '_>> {
        let inner = self.inner.read().expect("RwLock poisoned").clone();
        Box::pin(async move {
            inner.ensure_active()?;
            let result = inner.signer()?.get_public_key_async().await;
            inner.ensure_active()?;
            result
        })
    }
}

impl AsyncSignEvent for UniversalSigner {
    type Error = UniversalSignerError;

    fn sign_event_async(
        &self,
        unsigned: UnsignedEvent,
    ) -> Pin<Box<dyn Future<Output = Result<Event, Self::Error>> + Send + '_>> {
        let inner = self.inner.read().expect("RwLock poisoned").clone();
        Box::pin(async move {
            inner.ensure_active()?;
            let result = inner.signer()?.sign_event_async(unsigned).await;
            inner.ensure_active()?;
            result
        })
    }
}

impl AsyncNip44 for UniversalSigner {
    type Error = UniversalSignerError;

    fn nip44_encrypt_async<'a>(
        &'a self,
        public_key: &'a PublicKey,
        content: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, Self::Error>> + Send + 'a>> {
        let inner = self.inner.read().expect("RwLock poisoned").clone();
        Box::pin(async move {
            inner.ensure_active()?;
            let result = inner
                .signer()?
                .nip44_encrypt_async(public_key, content)
                .await;
            inner.ensure_active()?;
            result
        })
    }

    fn nip44_decrypt_async<'a>(
        &'a self,
        public_key: &'a PublicKey,
        payload: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, Self::Error>> + Send + 'a>> {
        let inner = self.inner.read().expect("RwLock poisoned").clone();
        Box::pin(async move {
            inner.ensure_active()?;
            let result = inner
                .signer()?
                .nip44_decrypt_async(public_key, payload)
                .await;
            inner.ensure_active()?;
            result
        })
    }
}

#[derive(Debug, Clone)]
pub struct GoopAuthUrlHandler;

impl AuthUrlHandler for GoopAuthUrlHandler {
    fn on_auth_url(
        &self,
        auth_url: Url,
    ) -> Pin<Box<dyn Future<Output = Result<(), nostr_connect::error::Error>> + Send + '_>> {
        Box::pin(async move {
            webbrowser::open(auth_url.as_str()).unwrap();
            Ok(())
        })
    }
}

#[cfg(test)]
mod failure_tests {
    #[test]
    fn request_deadline_is_typed_and_success_preserves_result() {
        use super::*;
        smol::block_on(async {
            let error = signer_request(
                std::future::pending::<Result<(), std::io::Error>>(),
                std::time::Duration::from_millis(5),
            ).await.unwrap_err();
            assert_eq!(SignerFailure::classify(&error), SignerFailure::Timeout);
            let value = signer_request(async { Ok::<_, std::io::Error>(42) }, REQUEST_TIMEOUT).await.unwrap();
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn logout_revokes_snapshots_without_breaking_a_new_login() {
        use super::*;
        smol::block_on(async {
            let signer = UniversalSigner::new(Keys::generate());
            let old = signer.snapshot();
            let public_key = old.get_public_key_async().await.unwrap();
            let pending = old.get_public_key_async();
            signer.disconnect();
            assert!(pending.await.is_err());
            assert!(old.get_public_key_async().await.is_err());
            assert!(
                old.nip44_encrypt_async(&public_key, "secret")
                    .await
                    .is_err()
            );
            assert!(
                old.sign_event_async(
                    EventBuilder::new(Kind::TextNote, "test").finalize_unsigned(public_key)
                )
                .await
                .is_err()
            );
            let next = Keys::generate();
            signer.swap_inner(next.clone());
            assert_eq!(
                signer.get_public_key_async().await.unwrap(),
                next.public_key()
            );
            assert!(old.get_public_key_async().await.is_err());
        });
    }

    use super::*;
    #[test]
    fn classifies_wrapped_signer_errors_without_treating_relay_rejection_as_consent() {
        use nostr_connect::error::{Error as ConnectError, ErrorKind};
        for (kind, message, expected) in [
            (ErrorKind::Rejected, "denied", SignerFailure::Rejected),
            (
                ErrorKind::Rejected,
                "user cancelled",
                SignerFailure::Cancelled,
            ),
            (ErrorKind::Timeout, "timed out", SignerFailure::Timeout),
        ] {
            let error = UniversalSignerError::new(ConnectError::with_static_message(kind, message));
            assert_eq!(SignerFailure::classify(&error), expected);
        }
        let disconnected =
            UniversalSignerError::new(std::io::Error::from(std::io::ErrorKind::NotConnected));
        assert_eq!(
            SignerFailure::classify(&disconnected),
            SignerFailure::Disconnected
        );
        let relay_error = nostr_sdk::error::Error::with_static_message(
            nostr_sdk::error::ErrorKind::Rejected,
            "relay rejected event",
        );
        assert_eq!(SignerFailure::classify(&relay_error), SignerFailure::Other);
        assert!(!SignerFailure::classify(&relay_error).requires_retry());
    }
}
