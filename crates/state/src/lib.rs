use std::collections::HashMap;

use anyhow::{Error, anyhow};
use common::config_dir;
use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task, Window};
use gpui_tokio::Tokio;
use instant::Duration;
use nostr_connect::prelude::*;
use nostr_gossip_memory::prelude::*;
use nostr_lmdb::prelude::*;
use nostr_sdk::prelude::*;

mod blossom;
pub mod encrypted_file;
mod constants;
pub mod credentials;
mod nip05;
mod profiles;
pub mod private_storage;
mod signer;

pub use blossom::*;
pub use constants::*;
pub use nip05::*;
pub use profiles::subscribe_profiles;
pub use signer::{GoopAuthUrlHandler, SignerFailure, UniversalSigner};

pub fn init(window: &mut Window, cx: &mut App) {
    // rustls uses the `aws_lc_rs` provider by default
    // This only errors if the default provider has already
    // been installed. We can ignore this `Result`.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();

    // Initialize the tokio runtime
    gpui_tokio::init(cx);

    NostrRegistry::set_global(cx.new(|cx| NostrRegistry::new(window, cx)), cx);
}

struct GlobalNostrRegistry(Entity<NostrRegistry>);

impl Global for GlobalNostrRegistry {}

/// Signer event.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum StateEvent {
    /// The state is busy
    Busy,
    /// User has no signer
    NoSigner,
    /// The signer has changed
    SignerChanged,
    /// An error occurred
    Error(String),
}

impl StateEvent {
    pub fn signer_changed(&self) -> bool {
        matches!(self, StateEvent::SignerChanged)
    }

    pub fn error<T>(error: T) -> Self
    where
        T: Into<String>,
    {
        Self::Error(error.into())
    }
}

/// Nostr Registry
#[derive(Debug)]
pub struct NostrRegistry {
    /// Nostr client
    client: Client,

    /// Universal signer
    signer: UniversalSigner,

    /// Current user's public key
    current_user: Option<PublicKey>,

    /// True until saved credentials and their signer have finished resolving.
    identity_loading: bool,
    remembered_user: Option<PublicKey>,
    connection_error: Option<String>,
    pending_signer: Option<UniversalSigner>,
    bunker: Option<NostrConnect>,
    logging_out: bool,
    connection_task: Option<Task<Result<(), Error>>>,
    credential_task: Option<Task<Result<(), Error>>>,

    media_servers: Vec<Url>,
    media_servers_task: Option<Task<()>>,

    /// Tasks for asynchronous operations
    tasks: Vec<Task<Result<(), Error>>>,
}

impl EventEmitter<StateEvent> for NostrRegistry {}

impl NostrRegistry {
    /// Retrieve the global nostr state
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalNostrRegistry>().0.clone()
    }

    /// Set the global nostr instance
    fn set_global(state: Entity<Self>, cx: &mut App) {
        cx.set_global(GlobalNostrRegistry(state));
    }

    /// Create a new nostr instance
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let signer = UniversalSigner::new(Keys::generate());
        let authenticator = SignerAuthenticator::new(signer.clone());

        // Construct the nostr lmdb instance
        let database = cx.foreground_executor().block_on(async move {
            NostrLmdb::open(config_dir().join("nostr"))
                .await
                .expect("Failed to initialize database")
        });

        // Construct the nostr client
        let client = ClientBuilder::default()
            .database(database)
            .authenticator(authenticator)
            .gossip(NostrGossipMemory::unbounded())
            .gossip_config(GossipConfig::default().no_background_refresh())
            .connect_timeout(Duration::from_secs(10))
            .sleep_when_idle(SleepWhenIdle::Enabled {
                timeout: Duration::from_secs(600),
            })
            .build();

        // Connect to bootstrap relays after the window is ready
        cx.defer_in(window, |this, _window, cx| {
            this.connect_bootstrap_relays(cx);

            this.get_user_credential(cx);
        });

        Self {
            client,
            signer,
            current_user: None,
            identity_loading: true,
            remembered_user: std::fs::read_to_string(config_dir().join("last-signer-pubkey"))
                .ok()
                .and_then(|value| PublicKey::parse(value.trim()).ok()),
            connection_error: None,
            pending_signer: None,
            bunker: None,
            logging_out: false,
            connection_task: None,
            credential_task: None,
            media_servers: Vec::new(),
            media_servers_task: None,
            tasks: vec![],
        }
    }

    /// Get the nostr client
    pub fn client(&self) -> Client {
        self.client.clone()
    }

    /// Get the current signer
    pub fn signer(&self) -> UniversalSigner {
        self.signer.clone()
    }

    /// Get the current user's public key
    pub fn current_user(&self) -> Option<PublicKey> {
        self.current_user
    }

    pub fn identity_loading(&self) -> bool {
        self.identity_loading
    }

    /// Display-only remembered identity; never authorizes account operations.
    pub fn displayed_user(&self) -> Option<PublicKey> {
        self.current_user.or(self.remembered_user)
    }

    /// Offer setup after startup settles when there is no known account or
    /// active connection attempt. A credential-store error must not strand a
    /// first-time user behind an unavailable profile menu.
    pub fn needs_signer_setup(&self) -> bool {
        !self.identity_loading && self.displayed_user().is_none() && self.pending_signer.is_none()
    }

    pub fn signer_connection_error(&self) -> Option<&str> {
        self.connection_error.as_deref()
    }

    pub fn retry_signer(&mut self, cx: &mut Context<Self>) {
        if self.logging_out { return; }
        if let Some(signer) = self.pending_signer.clone() {
            self.begin_signer_connection(signer, cx);
        } else {
            self.get_user_credential(cx);
        }
    }

    fn finish_identity_loading(&mut self, cx: &mut Context<Self>) {
        self.identity_loading = false;
        if self.current_user.is_none() {
            cx.emit(StateEvent::NoSigner);
        }
        cx.notify();
    }

    pub fn media_servers(&self) -> &[Url] {
        &self.media_servers
    }

    pub fn refresh_media_servers(&mut self, cx: &mut Context<Self>) {
        let Some(user) = self.current_user else {
            return;
        };
        let client = self.client();
        let task = cx.background_spawn(async move { load_media_servers(&client, user).await });
        self.media_servers_task = Some(cx.spawn(async move |this, cx| {
            let servers = task.await;
            this.update(cx, |this, cx| {
                if this.current_user == Some(user) {
                    this.media_servers = servers;
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// Forget only the login, preserving the master key and account history.
    pub fn logout(&mut self, cx: &mut Context<Self>) -> Task<Result<(), Error>> {
        if self.logging_out {
            return Task::ready(Err(anyhow!("Log out is already in progress")));
        }
        self.logging_out = true;
        let credentials = credentials::read(cx, USER_KEYRING);
        cx.spawn(async move |this, cx| {
            let result: Result<(), Error> = async {
                common::persistence::global().flush().await
                    .map_err(|error| anyhow!("Could not save local state before logout: {error}"))?;
                // Keychain deletion reports an error for absent entries on macOS.
                if credentials.await?.is_some() {
                    this.update(cx, |_, cx| credentials::delete(cx, USER_KEYRING))?.await?;
                }
                Ok(())
            }.await;
            this.update(cx, |this, cx| {
                this.logging_out = false;
                result?;
                match std::fs::remove_file(config_dir().join("last-signer-pubkey")) {
                    Ok(()) => {},
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                    Err(error) => return Err(error.into()),
                }
                this.credential_task = None;
                this.connection_task = None;
                this.pending_signer = None;
                this.tasks.clear();
                if let Some(bunker) = this.bunker.take() {
                    Tokio::spawn(cx, async move { bunker.shutdown().await }).detach();
                }
                this.signer.disconnect();
                this.current_user = None;
                this.remembered_user = None;
                this.identity_loading = false;
                this.connection_error = None;
                this.media_servers.clear();
                this.media_servers_task = None;
                cx.emit(StateEvent::NoSigner);
                cx.notify();
                Ok(())
            })?
        })
    }

    pub fn set_bunker(&mut self, signer: NostrConnect, cx: &mut Context<Self>) {
        self.bunker = Some(signer.clone());
        self.set_signer(signer, cx);
    }

    /// Update the signer
    pub fn set_signer<T>(&mut self, new_signer: T, cx: &mut Context<Self>)
    where
        T: AsyncGetPublicKey + AsyncSignEvent + AsyncNip44 + 'static,
        <T as AsyncGetPublicKey>::Error: std::error::Error + Send + Sync + 'static,
        <T as AsyncSignEvent>::Error: std::error::Error + Send + Sync + 'static,
        <T as AsyncNip44>::Error: std::error::Error + Send + Sync + 'static,
    {
        self.begin_signer_connection(UniversalSigner::new(new_signer), cx);
    }

    fn begin_signer_connection(&mut self, signer: UniversalSigner, cx: &mut Context<Self>) {
        if self.logging_out { return; }
        self.connection_task = None;
        self.pending_signer = Some(signer.clone());
        self.identity_loading = true;
        self.connection_error = None;
        cx.notify();
        let executor = cx.background_executor().clone();
        self.connection_task = Some(cx.spawn(async move |this, cx| {
            loop {
                use futures::{FutureExt, pin_mut, select};
                let request = signer.get_public_key_async().fuse();
                let timeout = executor.timer(Duration::from_secs(30)).fuse();
                pin_mut!(request, timeout);
                let result: Result<PublicKey, Error> = select! {
                    result = request => result.map_err(Into::into),
                    _ = timeout => Err(SignerFailure::Timeout.into()),
                };
                match result {
                    Ok(public_key) => {
                        this.update(cx, |this, cx| {
                            this.signer.swap_inner(signer.clone());
                            this.current_user = Some(public_key);
                            this.remembered_user = Some(public_key);
                            this.identity_loading = false;
                            this.connection_error = None;
                            this.pending_signer = None;
                            this.media_servers.clear();
                            this.refresh_media_servers(cx);
                            cx.emit(StateEvent::SignerChanged);
                            cx.notify();
                        })?;
                        // Only a public display hint; signer credentials stay in Keychain.
                        if let Err(error) = std::fs::create_dir_all(config_dir()).and_then(|_| {
                            std::fs::write(
                                config_dir().join("last-signer-pubkey"),
                                public_key.to_hex(),
                            )
                        }) {
                            log::warn!("Could not remember the displayed identity: {error}");
                        }
                        return Ok(());
                    }
                    Err(error) => {
                        let retry = reconnect_automatically(error.as_ref());
                        this.update(cx, |this, cx| {
                            this.identity_loading = retry;
                            if this.connection_error.is_none() {
                                cx.emit(StateEvent::error(error.to_string()));
                            }
                            this.connection_error = Some(if retry {
                                "Signer unavailable. Retrying the saved connection…".into()
                            } else {
                                format!("Could not connect to signer: {error}")
                            });
                            // A failed connection is not a sign-out: retain account and chats.
                            cx.notify();
                        })?;
                        if !retry {
                            return Ok(());
                        }
                        executor.timer(Duration::from_secs(15)).await;
                        this.update(cx, |this, cx| {
                            this.identity_loading = true;
                            cx.notify();
                        })?;
                    }
                }
            }
        }));
    }

    /// Connect to the bootstrapping relays
    fn connect_bootstrap_relays(&mut self, cx: &mut Context<Self>) {
        let client = self.client();

        let task: Task<Result<(), Error>> = cx.background_spawn(async move {
            // Add indexer relay to the relay pool
            for url in INDEXER_RELAYS.into_iter() {
                client
                    .add_relay(url)
                    .capabilities(RelayCapabilities::DISCOVERY)
                    .await?;
            }

            // Add bootstrap relay to the relay pool
            for url in BOOTSTRAP_RELAYS.into_iter() {
                client.add_relay(url).await?;
            }

            // Connect to all added relays
            client.connect().await;

            Ok(())
        });

        self.tasks.push(cx.spawn(async move |this, cx| {
            if let Err(e) = task.await {
                this.update(cx, |_this, cx| {
                    cx.emit(StateEvent::error(e.to_string()));
                })?;
            }
            Ok(())
        }));
    }

    /// Check the user's credential and set the signer if valid
    fn get_user_credential(&mut self, cx: &mut Context<Self>) {
        let user_keyring = credentials::read(cx, USER_KEYRING);
        self.identity_loading = true;
        self.connection_error = None;
        self.connection_task = None;
        self.pending_signer = None;
        cx.notify();

        self.credential_task = Some(cx.spawn(async move |this, cx| {
            let result: Result<(), Error> = async {
                match user_keyring.await? {
                    Some((_username, secret)) => {
                        let content = String::from_utf8(secret)?;

                        if content.starts_with("bunker://") {
                            let keys = this.update(cx, |this, cx| this.get_master_key(cx, false))?.await?;
                            let timeout = Duration::from_secs(30);
                            let uri = NostrConnectUri::parse(content)?;

                            // Construct the nostr connect signer
                            let mut signer = NostrConnect::new(uri, keys, timeout, None)?;

                            // Handle auth url with the default browser
                            signer.auth_url_handler(GoopAuthUrlHandler);

                            this.update(cx, |this, cx| {
                                this.set_bunker(signer, cx);
                                cx.notify();
                            })?;
                        } else if content == "proxy" {
                            // Preserve the old credential and account data until the
                            // user explicitly connects a replacement signer.
                            this.update(cx, |this, cx| {
                                this.remembered_user = None;
                                this.identity_loading = false;
                                this.connection_error = Some("Browser-extension login has been removed. Connect your signer using a bunker:// URL.".into());
                                cx.emit(StateEvent::NoSigner);
                                cx.notify();
                            })?;
                        } else {
                            return Err(anyhow!("Unrecognized saved identity format"));
                        }
                    }
                    None => {
                        this.update(cx, |this, cx| {
                            if this.remembered_user.is_some() || this.current_user.is_some() {
                                this.identity_loading = false;
                                this.connection_error = Some("Saved signer credentials are unavailable. Check Keychain access and retry.".into());
                                cx.notify();
                            } else {
                                this.finish_identity_loading(cx);
                            }
                        })?;
                    }
                }
                Ok(())
            }
            .await;
            if let Err(error) = result {
                this.update(cx, |this, cx| {
                    this.identity_loading = false;
                    this.connection_error = Some(format!("Could not restore saved signer: {error}"));
                    if this.needs_signer_setup() {
                        cx.emit(StateEvent::NoSigner);
                    }
                    cx.notify();
                })?;
            }
            Ok(())
        }));
    }

    /// Get the master key that used for Nostr Connect
    pub fn get_master_key(&self, cx: &App, create_if_missing: bool) -> Task<Result<Keys, Error>> {
        let task = credentials::read(cx, MASTER_KEYRING);
        let create_if_missing = create_if_missing && self.remembered_user.is_none();
        cx.spawn(async move |cx| {
            let saved = task.await?.map(|(_, secret)| secret);
            let (keys, created) = connection_keys(saved, create_if_missing)?;
            if created {
                let save = cx.update(|cx| {
                    credentials::write(
                        cx, MASTER_KEYRING,
                        &keys.public_key().to_hex(),
                        &keys.secret_key().to_secret_bytes(),
                    )
                });
                save.await?;
            }
            Ok(keys)
        })
    }

    /// Get the public key of a NIP-05 address
    pub fn query_address(&self, addr: Nip05Address, cx: &App) -> Task<Result<PublicKey, Error>> {
        let http_client = cx.http_client();

        cx.background_spawn(async move {
            let profile = addr.profile(&http_client).await?;
            let public_key = profile.public_key;

            Ok(public_key)
        })
    }

    /// Perform a WoT (via Vertex) search for a given query.
    pub fn wot_search(&self, query: &str, cx: &App) -> Task<Result<Vec<PublicKey>, Error>> {
        let client = self.client();
        let query = query.to_string();
        let signer = self.signer.clone();

        cx.background_spawn(async move {
            // Construct a vertex request event
            let event = EventBuilder::new(Kind::Custom(5315), "")
                .tags(vec![
                    Tag::custom("param", vec!["search", &query]),
                    Tag::custom("param", vec!["limit", "10"]),
                ])
                .finalize_async(&signer)
                .await?;

            // Send the event to vertex relays
            let output = client.send_event(&event).to(WOT_RELAYS).await?;

            // Construct a filter to get the response or error from vertex
            let filter = Filter::new()
                .kinds(vec![Kind::Custom(6315), Kind::Custom(7000)])
                .event(output.id().to_owned());

            // Construct target for subscription
            let target: HashMap<&str, Vec<Filter>> = WOT_RELAYS
                .into_iter()
                .map(|relay| (relay, vec![filter.clone()]))
                .collect();

            // Stream events from the wot relays
            let mut stream = client
                .stream_events(target)
                .timeout(Duration::from_secs(TIMEOUT))
                .await?;

            while let Some((_url, res)) = stream.next().await {
                if let Ok(event) = res {
                    match event.kind {
                        Kind::Custom(6315) => {
                            let content: serde_json::Value = serde_json::from_str(&event.content)?;
                            let pubkeys: Vec<PublicKey> = content
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(|item| item.as_object())
                                .filter_map(|obj| obj.get("pubkey").and_then(|v| v.as_str()))
                                .filter_map(|pubkey_str| PublicKey::parse(pubkey_str).ok())
                                .collect();

                            return Ok(pubkeys);
                        }
                        Kind::Custom(7000) => {
                            return Err(anyhow!("Search error"));
                        }
                        _ => {}
                    }
                }
            }

            Err(anyhow!("No results for query: {query}"))
        })
    }
}

fn connection_keys(saved: Option<Vec<u8>>, create_if_missing: bool) -> Result<(Keys, bool), Error> {
    match saved {
        Some(secret) => Ok((Keys::new(SecretKey::from_slice(&secret)?), false)),
        None if create_if_missing => Ok((Keys::generate(), true)),
        None => Err(anyhow!(
            "Saved signer connection key is unavailable; check Keychain access and retry"
        )),
    }
}

fn reconnect_automatically(error: &(dyn std::error::Error + 'static)) -> bool {
    matches!(
        SignerFailure::classify(error),
        SignerFailure::Disconnected | SignerFailure::Timeout
    )
}

#[cfg(test)]
mod reconnect_tests {
    use super::*;
    #[test]
    fn restoring_never_replaces_missing_or_invalid_connection_keys() {
        assert!(connection_keys(None, false).is_err());
        assert!(connection_keys(Some(vec![0; 5]), false).is_err());
        assert!(connection_keys(Some(vec![0; 5]), true).is_err());
        let existing = Keys::generate();
        for allow_creation in [false, true] {
            let (keys, created) = connection_keys(
                Some(existing.secret_key().to_secret_bytes().to_vec()),
                allow_creation,
            )
            .unwrap();
            assert!(!created);
            assert_eq!(keys.public_key(), existing.public_key());
        }
        assert!(connection_keys(None, true).unwrap().1);
    }

    #[test]
    fn transient_failures_retry_but_refusals_require_user_action() {
        assert!(reconnect_automatically(&SignerFailure::Timeout));
        assert!(reconnect_automatically(&SignerFailure::Disconnected));
        assert!(!reconnect_automatically(&SignerFailure::Rejected));
        assert!(!reconnect_automatically(&SignerFailure::Cancelled));
        assert!(!reconnect_automatically(&SignerFailure::Other));
    }
}
