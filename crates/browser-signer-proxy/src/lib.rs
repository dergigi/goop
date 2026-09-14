use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use atomic_destructor::{AtomicDestroyer, AtomicDestructor};
use event_listener::Event as ShutdownEvent;
use nostr::prelude::*;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Value, json};
use smol::channel;
use smol::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use smol::lock::Mutex;
use smol::net::{TcpListener, TcpStream};
use uuid::Uuid;

mod error;
pub mod prelude;

pub use self::error::{Error, ErrorKind};

const DEFAULT_HTML: &str = include_str!("../index.html");
const JS: &str = include_str!("../proxy.js");

type PendingResponseMap = HashMap<Uuid, channel::Sender<Result<Value, String>>>;

#[derive(Debug, Deserialize)]
struct Message {
    id: Uuid,
    error: Option<String>,
    result: Option<Value>,
}

impl Message {
    fn into_result(self) -> Result<Value, String> {
        if let Some(error) = self.error {
            Err(error)
        } else {
            Ok(self.result.unwrap_or(Value::Null))
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum RequestMethod {
    GetPublicKey,
    SignEvent,
    Nip04Encrypt,
    Nip04Decrypt,
    Nip44Encrypt,
    Nip44Decrypt,
}

impl RequestMethod {
    fn as_str(&self) -> &str {
        match self {
            Self::GetPublicKey => "get_public_key",
            Self::SignEvent => "sign_event",
            Self::Nip04Encrypt => "nip04_encrypt",
            Self::Nip04Decrypt => "nip04_decrypt",
            Self::Nip44Encrypt => "nip44_encrypt",
            Self::Nip44Decrypt => "nip44_decrypt",
        }
    }
}

impl Serialize for RequestMethod {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize)]
struct RequestData {
    id: Uuid,
    method: RequestMethod,
    params: Value,
}

impl RequestData {
    #[inline]
    fn new(method: RequestMethod, params: Value) -> Self {
        Self {
            id: Uuid::new_v4(),
            method,
            params,
        }
    }
}

#[derive(Serialize)]
struct Requests<'a> {
    requests: &'a [RequestData],
}

impl<'a> Requests<'a> {
    #[inline]
    fn new(requests: &'a [RequestData]) -> Self {
        Self { requests }
    }

    #[inline]
    fn len(&self) -> usize {
        self.requests.len()
    }
}

/// Params for NIP-04 and NIP-44 encryption/decryption
#[derive(Serialize)]
struct CryptoParams<'a> {
    public_key: &'a PublicKey,
    content: &'a str,
}

impl<'a> CryptoParams<'a> {
    #[inline]
    fn new(public_key: &'a PublicKey, content: &'a str) -> Self {
        Self {
            public_key,
            content,
        }
    }
}

#[derive(Debug)]
struct ProxyState {
    /// Requests waiting to be picked up by browser
    pub outgoing_requests: Mutex<Vec<RequestData>>,
    /// Map of request ID to response sender
    pub pending_responses: Mutex<PendingResponseMap>,
    /// Last time the client asked for the pending requests
    pub last_pending_request: Arc<AtomicU64>,
}

/// Configuration options for [`BrowserSignerProxy`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserSignerProxyOptions {
    /// Request timeout for the signer extension. Default is 30 seconds.
    pub timeout: Duration,
    /// Proxy server IP address and port. Default is `127.0.0.1:7400`.
    pub addr: SocketAddr,
    /// Custom HTML page.
    // NOTE: not `Option` to move it between threads without reference counter
    pub custom_html: &'static str,
}

#[derive(Debug, Clone)]
struct InnerBrowserSignerProxy {
    /// Configuration options for the proxy
    options: BrowserSignerProxyOptions,
    /// Internal state of the proxy including request queues
    state: Arc<ProxyState>,
    /// Notification trigger for graceful shutdown
    shutdown: Arc<ShutdownEvent>,
    /// Flag to indicate if the server is shutdown
    is_shutdown: Arc<AtomicBool>,
    /// Flag indicating if the server is started
    is_started: Arc<AtomicBool>,
}

impl AtomicDestroyer for InnerBrowserSignerProxy {
    fn on_destroy(&self) {
        self.shutdown();
    }
}

impl InnerBrowserSignerProxy {
    #[inline]
    fn is_shutdown(&self) -> bool {
        self.is_shutdown.load(Ordering::SeqCst)
    }

    fn shutdown(&self) {
        // Mark the server as shutdown
        self.is_shutdown.store(true, Ordering::SeqCst);

        // Notify all waiters that the proxy is shutting down
        self.shutdown.notify(usize::MAX);
    }
}

/// Nostr Browser Signer Proxy
///
/// Proxy to use Nostr Browser signer (NIP-07) in native applications.
#[derive(Debug, Clone)]
pub struct BrowserSignerProxy {
    inner: AtomicDestructor<InnerBrowserSignerProxy>,
}

impl Default for BrowserSignerProxyOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            // 7 for NIP-07 and 400 because the NIP title is 40 bytes :)
            addr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 7400)),
            custom_html: "",
        }
    }
}

impl BrowserSignerProxyOptions {
    /// Sets the timeout duration.
    pub const fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Sets the IP address.
    pub const fn ip_addr(mut self, new_ip: IpAddr) -> Self {
        self.addr = SocketAddr::new(new_ip, self.addr.port());
        self
    }

    /// Sets the port number.
    pub const fn port(mut self, new_port: u16) -> Self {
        self.addr = SocketAddr::new(self.addr.ip(), new_port);
        self
    }

    /// Sets a custom html page.
    ///
    /// The page must include `/proxy.js` script (`<script src="/proxy.js"></script>`)
    /// which will handle communication with the server and update the element
    /// with id `nip07-proxy-status` with the status.
    pub const fn custom_html_page(mut self, custom_html: &'static str) -> Self {
        self.custom_html = custom_html;
        self
    }
}

impl BrowserSignerProxy {
    /// Construct a new browser signer proxy
    pub fn new(options: BrowserSignerProxyOptions) -> Self {
        let state = ProxyState {
            outgoing_requests: Mutex::new(Vec::new()),
            pending_responses: Mutex::new(HashMap::new()),
            last_pending_request: Arc::new(AtomicU64::new(0)),
        };

        Self {
            inner: AtomicDestructor::new(InnerBrowserSignerProxy {
                options,
                state: Arc::new(state),
                shutdown: Arc::new(ShutdownEvent::new()),
                is_shutdown: Arc::new(AtomicBool::new(false)),
                is_started: Arc::new(AtomicBool::new(false)),
            }),
        }
    }

    /// Indicates whether the server is currently running.
    #[inline]
    pub fn is_started(&self) -> bool {
        self.inner.is_started.load(Ordering::SeqCst)
    }

    /// Checks if there is an open browser tab ready to respond to requests by
    /// verifying the time since the last pending request.
    #[inline]
    pub fn is_session_active(&self) -> bool {
        current_time() - self.inner.state.last_pending_request.load(Ordering::SeqCst) < 2
    }

    /// Get the signer proxy webpage URL
    #[inline]
    pub fn url(&self) -> String {
        format!("http://{}", self.inner.options.addr)
    }

    /// Start the proxy server.
    ///
    /// If this is not called explicitly, the server will be automatically
    /// started on the first interaction with the signer.
    pub async fn start(&self) -> Result<(), Error> {
        // Ensure is not shutdown
        if self.inner.is_shutdown() {
            return Err(Error::shutdown());
        }

        // Mark the proxy as started and check if was already started
        let is_started: bool = self.inner.is_started.swap(true, Ordering::SeqCst);

        // Immediately return if already started
        if is_started {
            return Ok(());
        }

        let listener: TcpListener = match TcpListener::bind(self.inner.options.addr).await {
            Ok(listener) => listener,
            Err(e) => {
                // Undo the started flag if binding fails
                self.inner.is_started.store(false, Ordering::SeqCst);
                return Err(Error::from(e));
            }
        };

        let addr: SocketAddr = self.inner.options.addr;
        let state: Arc<ProxyState> = self.inner.state.clone();
        let custom_html: &'static str = self.inner.options.custom_html;
        let shutdown: Arc<ShutdownEvent> = self.inner.shutdown.clone();

        smol::spawn(async move {
            tracing::info!("Starting proxy server on {addr}");

            loop {
                // Race between accepting a new connection and shutdown signal
                let shutdown_listener = shutdown.listen();

                enum AcceptEvent {
                    Connection(Result<(TcpStream, SocketAddr), std::io::Error>),
                    Shutdown,
                }

                let event = smol::future::or(
                    async { AcceptEvent::Connection(listener.accept().await) },
                    async {
                        shutdown_listener.await;
                        AcceptEvent::Shutdown
                    },
                )
                .await;

                match event {
                    AcceptEvent::Connection(Ok((stream, _))) => {
                        let state: Arc<ProxyState> = state.clone();
                        let shutdown: Arc<ShutdownEvent> = shutdown.clone();

                        smol::spawn(async move {
                            let shutdown_listener = shutdown.listen();

                            smol::future::or(
                                async {
                                    handle_connection(stream, state, custom_html).await;
                                },
                                async {
                                    shutdown_listener.await;
                                    tracing::debug!(
                                        "Closing connection, proxy server is shutting down."
                                    );
                                },
                            )
                            .await;
                        })
                        .detach();
                    }
                    AcceptEvent::Connection(Err(e)) => {
                        tracing::error!("Failed to accept connection: {e}");
                    }
                    AcceptEvent::Shutdown => break,
                }
            }

            tracing::info!("Proxy server shut down.");
        })
        .detach();

        Ok(())
    }

    #[inline]
    async fn store_pending_response(&self, id: Uuid, tx: channel::Sender<Result<Value, String>>) {
        let mut pending_responses = self.inner.state.pending_responses.lock().await;
        pending_responses.insert(id, tx);
    }

    #[inline]
    async fn store_outgoing_request(&self, request: RequestData) {
        let mut outgoing_requests = self.inner.state.outgoing_requests.lock().await;
        outgoing_requests.push(request);
    }

    async fn request<T>(&self, method: RequestMethod, params: Value) -> Result<T, Error>
    where
        T: DeserializeOwned,
    {
        // Start the proxy if not already started
        self.start().await?;

        // Construct the request
        let request: RequestData = RequestData::new(method, params);

        // Create a bounded channel of size 1 as a oneshot replacement
        let (tx, rx) = channel::bounded::<Result<Value, String>>(1);

        // Store the response sender
        self.store_pending_response(request.id, tx).await;

        // Add to outgoing requests queue
        self.store_outgoing_request(request).await;

        // Wait for response with timeout
        let response = race_timeout(self.inner.options.timeout, rx.recv()).await;

        match response {
            Ok(Ok(res)) => Ok(serde_json::from_value(res)?),
            Ok(Err(error)) => Err(Error::generic(error)),
            Err(TimeoutError) => Err(Error::timeout()),
        }
    }

    #[inline]
    async fn _get_public_key(&self) -> Result<PublicKey, Error> {
        self.request(RequestMethod::GetPublicKey, json!({})).await
    }

    #[inline]
    async fn _sign_event(&self, event: UnsignedEvent) -> Result<Event, Error> {
        let event: Event = self
            .request(RequestMethod::SignEvent, serde_json::to_value(event)?)
            .await?;
        event.verify()?;
        Ok(event)
    }

    #[inline]
    async fn _nip04_encrypt(&self, public_key: &PublicKey, content: &str) -> Result<String, Error> {
        let params = CryptoParams::new(public_key, content);
        self.request(RequestMethod::Nip04Encrypt, serde_json::to_value(params)?)
            .await
    }

    #[inline]
    async fn _nip04_decrypt(&self, public_key: &PublicKey, content: &str) -> Result<String, Error> {
        let params = CryptoParams::new(public_key, content);
        self.request(RequestMethod::Nip04Decrypt, serde_json::to_value(params)?)
            .await
    }

    #[inline]
    async fn _nip44_encrypt(&self, public_key: &PublicKey, content: &str) -> Result<String, Error> {
        let params = CryptoParams::new(public_key, content);
        self.request(RequestMethod::Nip44Encrypt, serde_json::to_value(params)?)
            .await
    }

    #[inline]
    async fn _nip44_decrypt(&self, public_key: &PublicKey, content: &str) -> Result<String, Error> {
        let params = CryptoParams::new(public_key, content);
        self.request(RequestMethod::Nip44Decrypt, serde_json::to_value(params)?)
            .await
    }
}

impl AsyncGetPublicKey for BrowserSignerProxy {
    type Error = Error;

    #[inline]
    fn get_public_key_async(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<PublicKey, Self::Error>> + Send + '_>> {
        Box::pin(async move { self._get_public_key().await })
    }
}

impl AsyncSignEvent for BrowserSignerProxy {
    type Error = Error;

    #[inline]
    fn sign_event_async(
        &self,
        unsigned: UnsignedEvent,
    ) -> Pin<Box<dyn Future<Output = Result<Event, Self::Error>> + Send + '_>> {
        Box::pin(async move { self._sign_event(unsigned).await })
    }
}

impl AsyncNip04 for BrowserSignerProxy {
    type Error = Error;

    fn nip04_encrypt_async<'a>(
        &'a self,
        public_key: &'a PublicKey,
        content: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, Self::Error>> + Send + 'a>> {
        Box::pin(async move { self._nip04_encrypt(public_key, content).await })
    }

    fn nip04_decrypt_async<'a>(
        &'a self,
        public_key: &'a PublicKey,
        encrypted_content: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, Self::Error>> + Send + 'a>> {
        Box::pin(async move { self._nip04_decrypt(public_key, encrypted_content).await })
    }
}

impl AsyncNip44 for BrowserSignerProxy {
    type Error = Error;

    fn nip44_encrypt_async<'a>(
        &'a self,
        public_key: &'a PublicKey,
        content: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, Self::Error>> + Send + 'a>> {
        Box::pin(async move { self._nip44_encrypt(public_key, content).await })
    }

    fn nip44_decrypt_async<'a>(
        &'a self,
        public_key: &'a PublicKey,
        payload: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, Self::Error>> + Send + 'a>> {
        Box::pin(async move { self._nip44_decrypt(public_key, payload).await })
    }
}

// ── Minimal HTTP server ──────────────────────────────────────────────────

/// Handle a single HTTP connection.
async fn handle_connection(stream: TcpStream, state: Arc<ProxyState>, custom_html: &'static str) {
    let mut reader = BufReader::new(stream);

    // Read the request line
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).await.is_err() {
        return;
    }
    let request_line = request_line.trim_end().to_string();

    // Parse method, path, and HTTP version from request line
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        send_response(&mut reader, 400, "Bad Request", "", "").await;
        return;
    }
    let method = parts[0].to_uppercase();
    let path = parts[1].to_string();

    // Read headers until empty line
    let mut headers = Vec::new();
    let mut content_length: usize = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await.is_err() {
            return;
        }
        let line = line.trim_end().to_string();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("content-length:") {
            content_length = value.trim().parse().unwrap_or(0);
        } else if let Some(value) = line.strip_prefix("Content-Length:") {
            content_length = value.trim().parse().unwrap_or(0);
        }
        headers.push(line);
    }

    match (method.as_str(), path.as_str()) {
        // Serve the HTML proxy page
        ("GET", "/") => {
            let html = if custom_html.is_empty() {
                DEFAULT_HTML
            } else {
                custom_html
            };
            send_response(&mut reader, 200, "OK", "text/html", html).await;
        }
        // Serve the JS proxy script
        ("GET", "/proxy.js") => {
            send_response(&mut reader, 200, "OK", "application/javascript", JS).await;
        }
        // Browser polls this endpoint to get pending requests
        ("GET", "/api/pending") => {
            state
                .last_pending_request
                .store(current_time(), Ordering::SeqCst);

            let mut outgoing = state.outgoing_requests.lock().await;

            let requests = Requests::new(&outgoing);
            let json = match serde_json::to_string(&requests) {
                Ok(j) => j,
                Err(e) => {
                    tracing::error!("Failed to serialize pending requests: {e}");
                    send_response(&mut reader, 500, "Internal Server Error", "", "").await;
                    return;
                }
            };

            tracing::debug!("Sending {} pending requests to browser", requests.len());

            // Clear the outgoing requests after sending them
            outgoing.clear();

            send_response_cors_json(&mut reader, 200, "OK", &json).await;
        }
        // Receive response from browser extension
        ("POST", "/api/response") => {
            let mut body_bytes = vec![0u8; content_length];
            if content_length > 0 && reader.read_exact(&mut body_bytes).await.is_err() {
                send_response(&mut reader, 400, "Bad Request", "", "").await;
                return;
            }

            let message: Message = match serde_json::from_slice(&body_bytes) {
                Ok(json) => json,
                Err(e) => {
                    tracing::error!("Failed to parse response body: {e}");
                    send_response(&mut reader, 400, "Invalid JSON", "", "").await;
                    return;
                }
            };

            tracing::debug!("Received response from browser: {message:?}");

            let id: Uuid = message.id;
            let mut pending = state.pending_responses.lock().await;

            match pending.remove(&id) {
                Some(sender) => {
                    // Use try_send since we already hold the lock
                    let _ = sender.try_send(message.into_result());
                    tracing::info!("Forwarded response for request {id}");
                }
                None => tracing::warn!("No pending request found for {id}"),
            }

            send_response_cors(&mut reader, 200, "OK", "text/plain", "OK").await;
        }
        // CORS preflight
        ("OPTIONS", _) => {
            let response = "HTTP/1.1 200 OK\r\n\
                Access-Control-Allow-Origin: *\r\n\
                Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
                Access-Control-Allow-Headers: Content-Type\r\n\
                Content-Length: 0\r\n\
                Connection: close\r\n\
                \r\n";
            let _ = reader.get_mut().write_all(response.as_bytes()).await;
            let _ = reader.get_mut().flush().await;
        }
        // 404 - not found
        _ => {
            send_response(&mut reader, 404, "Not Found", "", "").await;
        }
    }
}

/// Write an HTTP response to the stream.
async fn send_response(
    stream: &mut (impl AsyncWriteExt + Unpin),
    status: u16,
    status_text: &str,
    content_type: &str,
    body: &str,
) {
    let mut response = format!("HTTP/1.1 {status} {status_text}\r\n");

    if !content_type.is_empty() {
        response.push_str(&format!("Content-Type: {content_type}\r\n"));
    }

    response.push_str(&format!("Content-Length: {}\r\n", body.len()));
    response.push_str("Access-Control-Allow-Origin: *\r\n");
    response.push_str("Connection: close\r\n");
    response.push_str("\r\n");
    response.push_str(body);

    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.flush().await;
}

/// Write a response with CORS headers and JSON content type.
async fn send_response_cors_json(
    stream: &mut (impl AsyncWriteExt + Unpin),
    status: u16,
    status_text: &str,
    body: &str,
) {
    let mut response = format!("HTTP/1.1 {status} {status_text}\r\n");
    response.push_str("Content-Type: application/json\r\n");
    response.push_str(&format!("Content-Length: {}\r\n", body.len()));
    response.push_str("Access-Control-Allow-Origin: *\r\n");
    response.push_str("Connection: close\r\n");
    response.push_str("\r\n");
    response.push_str(body);

    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.flush().await;
}

/// Write a response with CORS headers.
async fn send_response_cors(
    stream: &mut (impl AsyncWriteExt + Unpin),
    status: u16,
    status_text: &str,
    content_type: &str,
    body: &str,
) {
    let mut response = format!("HTTP/1.1 {status} {status_text}\r\n");

    if !content_type.is_empty() {
        response.push_str(&format!("Content-Type: {content_type}\r\n"));
    }

    response.push_str(&format!("Content-Length: {}\r\n", body.len()));
    response.push_str("Access-Control-Allow-Origin: *\r\n");
    response.push_str("Connection: close\r\n");
    response.push_str("\r\n");
    response.push_str(body);

    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.flush().await;
}

// ── Timeout helper ───────────────────────────────────────────────────────

/// An error indicating that an operation timed out.
#[derive(Debug)]
struct TimeoutError;

/// Races a channel receive against a duration.
///
/// Returns the channel value on success, or [`TimeoutError`] if the duration
/// elapses first or the channel is closed.
async fn race_timeout<T>(
    duration: Duration,
    recv: impl Future<Output = Result<T, channel::RecvError>>,
) -> Result<T, TimeoutError> {
    enum Event<T> {
        Value(T),
        ChannelClosed,
        Timeout,
    }

    let event = smol::future::or(
        async {
            match recv.await {
                Ok(value) => Event::Value(value),
                Err(_) => Event::ChannelClosed,
            }
        },
        async {
            smol::Timer::after(duration).await;
            Event::Timeout
        },
    )
    .await;

    match event {
        Event::Value(value) => Ok(value),
        Event::ChannelClosed | Event::Timeout => Err(TimeoutError),
    }
}

// ── Utility ──────────────────────────────────────────────────────────────

/// Gets the current time in seconds since the Unix epoch (1970-01-01). If the
/// time is before the epoch, returns 0.
#[inline]
fn current_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}
