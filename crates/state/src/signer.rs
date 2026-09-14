use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
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
            #[cfg(not(target_arch = "wasm32"))]
            if let Some(error) = error.downcast_ref::<browser_signer_proxy::Error>() {
                use browser_signer_proxy::ErrorKind;
                match error.kind() {
                    ErrorKind::Rejected => return Self::refusal(&error.to_string()),
                    ErrorKind::Timeout => return Self::Timeout,
                    ErrorKind::State => return Self::Disconnected,
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
    inner: Arc<RwLock<Arc<dyn InnerSigner>>>,
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
            inner: Arc::new(RwLock::new(Arc::new(InnerSignerImpl(signer)))),
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

    /// Swap the inner signer in-place. All clones see the new signer.
    pub fn swap_inner<T>(&self, new_signer: T)
    where
        T: AsyncGetPublicKey + AsyncSignEvent + AsyncNip44 + 'static,
        <T as AsyncGetPublicKey>::Error: Error + Send + Sync + 'static,
        <T as AsyncSignEvent>::Error: Error + Send + Sync + 'static,
        <T as AsyncNip44>::Error: Error + Send + Sync + 'static,
    {
        *self.inner.write().expect("RwLock poisoned") = Arc::new(InnerSignerImpl(new_signer));
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
            AsyncGetPublicKey::get_public_key_async(&self.0)
                .await
                .map_err(UniversalSignerError::new)
        })
    }

    fn sign_event_async(
        &self,
        unsigned: UnsignedEvent,
    ) -> Pin<Box<dyn Future<Output = Result<Event, UniversalSignerError>> + Send + '_>> {
        Box::pin(async move {
            AsyncSignEvent::sign_event_async(&self.0, unsigned)
                .await
                .map_err(UniversalSignerError::new)
        })
    }

    fn nip44_encrypt_async<'a>(
        &'a self,
        public_key: &'a PublicKey,
        content: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, UniversalSignerError>> + Send + 'a>> {
        Box::pin(async move {
            AsyncNip44::nip44_encrypt_async(&self.0, public_key, content)
                .await
                .map_err(UniversalSignerError::new)
        })
    }

    fn nip44_decrypt_async<'a>(
        &'a self,
        public_key: &'a PublicKey,
        payload: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, UniversalSignerError>> + Send + 'a>> {
        Box::pin(async move {
            AsyncNip44::nip44_decrypt_async(&self.0, public_key, payload)
                .await
                .map_err(UniversalSignerError::new)
        })
    }
}

impl UniversalSigner {
    #[allow(dead_code)]
    fn with_inner<R>(&self, f: impl FnOnce(&dyn InnerSigner) -> R) -> R {
        let guard = self.inner.read().expect("RwLock poisoned");
        f(&**guard)
    }
}

impl AsyncGetPublicKey for UniversalSigner {
    type Error = UniversalSignerError;

    fn get_public_key_async(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<PublicKey, Self::Error>> + Send + '_>> {
        let inner = self.inner.read().expect("RwLock poisoned").clone();
        Box::pin(async move { inner.get_public_key_async().await })
    }
}

impl AsyncSignEvent for UniversalSigner {
    type Error = UniversalSignerError;

    fn sign_event_async(
        &self,
        unsigned: UnsignedEvent,
    ) -> Pin<Box<dyn Future<Output = Result<Event, Self::Error>> + Send + '_>> {
        let inner = self.inner.read().expect("RwLock poisoned").clone();
        Box::pin(async move { inner.sign_event_async(unsigned).await })
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
        Box::pin(async move { inner.nip44_encrypt_async(public_key, content).await })
    }

    fn nip44_decrypt_async<'a>(
        &'a self,
        public_key: &'a PublicKey,
        payload: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, Self::Error>> + Send + 'a>> {
        let inner = self.inner.read().expect("RwLock poisoned").clone();
        Box::pin(async move { inner.nip44_decrypt_async(public_key, payload).await })
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
