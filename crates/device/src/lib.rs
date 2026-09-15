use std::cell::Cell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as AnyhowContext, Error, anyhow};
use gpui::{
    App, AppContext, Context, Entity, EventEmitter, Global, IntoElement, ParentElement,
    SharedString, Styled, Subscription, Task, Window, div, relative,
};
use instant::Duration;
use nostr_sdk::prelude::*;
use person::PersonRegistry;
use settings::AppSettings;
use smallvec::{SmallVec, smallvec};
use state::{Announcement, CLIENT_NAME, NostrRegistry, UniversalSigner};
use theme::ActiveTheme;
use ui::avatar::Avatar;
use ui::button::Button;
use ui::notification::{Notification, NotificationKind};
use ui::{Disableable, Sizable, StyledExt, WindowExtension, h_flex, v_flex};

const IDENTIFIER: &str = "goop:device";

pub fn init(window: &mut Window, cx: &mut App) {
    DeviceRegistry::set_global(cx.new(|cx| DeviceRegistry::new(window, cx)), cx);
}

struct GlobalDeviceRegistry(Entity<DeviceRegistry>);

impl Global for GlobalDeviceRegistry {}

/// Device event.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum DeviceEvent {
    /// A new encryption signer has been set
    Set,
    /// User have not setup encryption key
    NotSet,
    /// The device is requesting an encryption key
    Requesting,
    /// An error occurred
    Error(SharedString),
}

impl DeviceEvent {
    pub fn error<T>(error: T) -> Self
    where
        T: Into<SharedString>,
    {
        Self::Error(error.into())
    }
}

/// Device Registry
///
/// NIP-4e: https://github.com/nostr-protocol/nips/blob/per-device-keys/4e.md
#[derive(Debug)]
pub struct DeviceRegistry {
    /// Whether there is a pending request for encryption key approval
    pub pending_request: bool,

    /// Whether an announcement has been made for this device
    pub announcement_existed: Arc<AtomicBool>,

    /// Signer
    signer: Entity<Option<UniversalSigner>>,

    /// Async tasks
    tasks: Vec<Task<Result<(), Error>>>,

    /// Event subscription
    _subscriptions: SmallVec<[Subscription; 2]>,
}

impl EventEmitter<DeviceEvent> for DeviceRegistry {}

impl DeviceRegistry {
    /// Retrieve the global device registry state
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalDeviceRegistry>().0.clone()
    }

    /// Set the global device registry instance
    fn set_global(state: Entity<Self>, cx: &mut App) {
        cx.set_global(GlobalDeviceRegistry(state));
    }

    /// Create a new device registry instance
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let nostr = NostrRegistry::global(cx);
        let settings = AppSettings::global(cx);

        let signer = cx.new(|_| None);
        let mut subscriptions = smallvec![];

        subscriptions.push(
            // Subscribe to nostr state events
            cx.observe(&settings, move |this, settings, cx| {
                if settings.read(cx).is_nip4e_enabled(cx) {
                    this.get_announcement(cx);
                };
            }),
        );

        subscriptions.push(
            // Observe the user signer
            cx.subscribe_in(&nostr, window, move |this, _nostr, event, window, cx| {
                if matches!(event, state::StateEvent::NoSigner) {
                    this.tasks.clear();
                    this.pending_request = false;
                    this.announcement_existed = Arc::new(AtomicBool::new(false));
                    this.signer.update(cx, |signer, cx| {
                        if let Some(signer) = signer.take() { signer.disconnect(); }
                        cx.notify();
                    });
                    cx.notify();
                }
                if event.signer_changed() {
                    this.tasks.clear();
                    this.handle_notifications(window, cx);
                }
                if event.signer_changed() && settings.read(cx).is_nip4e_enabled(cx) {
                    this.get_announcement(cx);
                }
            }),
        );

        cx.defer_in(window, |this, window, cx| {
            this.handle_notifications(window, cx);
        });

        Self {
            signer,
            pending_request: false,
            announcement_existed: Arc::new(AtomicBool::new(false)),
            tasks: vec![],
            _subscriptions: subscriptions,
        }
    }

    fn handle_notifications(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let nostr = NostrRegistry::global(cx);
        let client = nostr.read(cx).client();
        let signer = nostr.read(cx).signer();

        let announcement_existed = self.announcement_existed.clone();
        let (tx, rx) = flume::bounded::<Event>(100);

        self.tasks.push(cx.background_spawn(async move {
            let mut notifications = client.notifications();
            let mut processed_events = HashSet::new();
            let current_user = signer.get_public_key_async().await.ok();

            while let Some(notification) = notifications.next().await {
                if let ClientNotification::Message { message, .. } = notification
                    && let RelayMessage::Event { event, .. } = *message
                {
                    if !processed_events.insert(event.id) {
                        // Skip if the event has already been processed
                        continue;
                    }

                    match event.kind {
                        Kind::Custom(10044) if current_user == Some(event.pubkey) => {
                            announcement_existed.store(true, Ordering::Relaxed);
                            tx.send_async(event.into_owned()).await?;
                        }
                        Kind::Custom(4454) if current_user == Some(event.pubkey) => {
                            tx.send_async(event.into_owned()).await?;
                        }
                        Kind::Custom(4455) if current_user == Some(event.pubkey) => {
                            tx.send_async(event.into_owned()).await?;
                        }
                        _ => {}
                    }
                }
            }

            Ok(())
        }));

        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = rx.recv_async().await {
                match event.kind {
                    Kind::Custom(10044) => {
                        this.update_in(cx, |this, _window, cx| {
                            this.set_encryption(&event, cx);
                        })?;
                    }
                    // New request event from other device
                    Kind::Custom(4454) => {
                        this.update_in(cx, |this, window, cx| {
                            this.ask_for_approval(event, window, cx);
                        })?;
                    }
                    // New response event from the master device
                    Kind::Custom(4455) => {
                        this.update(cx, |this, cx| {
                            this.extract_encryption(event, cx);
                        })?;
                    }
                    _ => {}
                }
            }
            Ok(())
        }));
    }

    /// Set whether there is a pending request for encryption key approval
    fn set_pending_request(&mut self, pending: bool, cx: &mut Context<Self>) {
        self.pending_request = pending;
        cx.notify();
    }

    /// Get the signer
    pub fn signer(&self, cx: &App) -> Option<UniversalSigner> {
        self.signer.read(cx).clone()
    }

    /// Set the decoupled encryption key for the current user
    fn set_signer(&mut self, new_signer: Keys, cx: &mut Context<Self>) {
        self.signer.update(cx, |this, cx| {
            *this = Some(UniversalSigner::new(new_signer));
            cx.notify();
        });
        cx.emit(DeviceEvent::Set);
    }

    /// Backup the encryption's secret key to a file
    pub fn backup(&self, path: PathBuf, cx: &App) -> Task<Result<(), Error>> {
        let nostr = NostrRegistry::global(cx);
        let client = nostr.read(cx).client();
        let signer = nostr.read(cx).signer();

        cx.background_spawn(async move {
            let keys = get_keys(&client, &signer).await?;
            let content = keys.secret_key().to_bech32()?;

            if cfg!(target_arch = "wasm32") {
                return Err(anyhow!("Not supported"));
            }

            #[cfg(not(target_arch = "wasm32"))]
            smol::fs::write(path, &content).await?;

            Ok(())
        })
    }

    /// Get device announcement for current user
    pub fn get_announcement(&mut self, cx: &mut Context<Self>) {
        let nostr = NostrRegistry::global(cx);
        let client = nostr.read(cx).client();
        let signer = nostr.read(cx).signer();

        self.tasks.push(cx.background_spawn(async move {
            let opts = SubscribeAutoCloseOptions::default().exit_policy(ReqExitPolicy::ExitOnEOSE);
            let current_user = signer.get_public_key_async().await?;

            // Construct the filter for the device announcement event
            let filter = Filter::new()
                .kind(Kind::Custom(10044))
                .author(current_user)
                .limit(1);

            client
                .subscribe(filter)
                .close_on(opts)
                .with_id(SubscriptionId::new("nip4e"))
                .await?;

            Ok(())
        }));

        let announcement_existed = self.announcement_existed.clone();

        self.tasks.push(cx.spawn(async move |this, cx| {
            // Wait for 5 seconds
            cx.background_executor().timer(Duration::from_secs(5)).await;

            // Then check if the msg relays have been found
            if announcement_existed.load(Ordering::Acquire) {
                return Ok(());
            }

            this.update(cx, |_this, cx| {
                cx.emit(DeviceEvent::NotSet);
            })?;

            Ok(())
        }));
    }

    /// Create a new device signer and announce it to user's relay list
    pub fn set_announcement(&mut self, keys: Keys, cx: &mut Context<Self>) {
        let task = self.create_encryption(keys, cx);

        self.tasks.push(cx.spawn(async move |this, cx| {
            match task.await {
                Ok(keys) => {
                    this.update(cx, |this, cx| {
                        this.set_signer(keys, cx);
                        this.wait_for_request(cx);
                    })?;
                }
                Err(e) => {
                    this.update(cx, |_this, cx| {
                        cx.emit(DeviceEvent::error(e.to_string()));
                    })?;
                }
            }
            Ok(())
        }));
    }

    /// Create new encryption key and announce it to user's relay list
    fn create_encryption(&self, keys: Keys, cx: &App) -> Task<Result<Keys, Error>> {
        let nostr = NostrRegistry::global(cx);
        let client = nostr.read(cx).client();
        let signer = nostr.read(cx).signer();

        let secret = keys.secret_key().to_secret_hex();
        let n = keys.public_key();

        cx.background_spawn(async move {
            // Construct an announcement event
            let event = EventBuilder::new(Kind::Custom(10044), "")
                .tags(vec![
                    Tag::custom("n", vec![n]),
                    Tag::custom("client", vec![CLIENT_NAME]),
                ])
                .finalize_async(&signer)
                .await?;

            // Publish announcement
            client
                .send_event(&event)
                .to_nip65()
                .ack_policy(AckPolicy::none())
                .await?;

            // Save device keys to the database
            set_keys(&client, &signer, &secret).await?;

            Ok(keys)
        })
    }

    /// Set encryption key from the announcement event
    fn set_encryption(&mut self, event: &Event, cx: &mut Context<Self>) {
        let nostr = NostrRegistry::global(cx);
        let client = nostr.read(cx).client();
        let signer = nostr.read(cx).signer();

        let announcement = Announcement::from(event);
        let device_pubkey = announcement.public_key();

        // Get encryption key from the database and compare with the announcement
        let task: Task<Result<Keys, Error>> = cx.background_spawn(async move {
            let keys = get_keys(&client, &signer).await?;

            // Compare the public key from the announcement with the one from the database
            if keys.public_key() != device_pubkey {
                return Err(anyhow!("The encryption key doesn't match the announcement."));
            };

            Ok(keys)
        });

        self.tasks.push(cx.spawn(async move |this, cx| {
            if let Ok(keys) = task.await {
                this.update(cx, |this, cx| {
                    this.set_signer(keys, cx);
                    this.wait_for_request(cx);
                })?;
            } else {
                this.update(cx, |this, cx| {
                    this.request(cx);
                })?;
            }
            Ok(())
        }));
    }

    /// Wait for encryption key requests from now on
    fn wait_for_request(&mut self, cx: &mut Context<Self>) {
        let nostr = NostrRegistry::global(cx);
        let client = nostr.read(cx).client();
        let signer = nostr.read(cx).signer();

        self.tasks.push(cx.background_spawn(async move {
            let public_key = signer.get_public_key_async().await?;
            let id = SubscriptionId::new("dekey-requests");

            // Construct a filter for encryption key requests
            let filter = Filter::new()
                .kind(Kind::Custom(4454))
                .author(public_key)
                .since(Timestamp::now());

            // Subscribe to the device key requests on user's write relays
            client.subscribe(vec![filter]).with_id(id).await?;

            Ok(())
        }));
    }

    /// Request encryption keys from other device
    pub fn request(&mut self, cx: &mut Context<Self>) {
        let nostr = NostrRegistry::global(cx);
        let client = nostr.read(cx).client();
        let signer = nostr.read(cx).signer();

        let app_keys_task = get_or_init_app_keys(cx);

        let task: Task<Result<Option<Event>, Error>> = cx.background_spawn(async move {
            let app_keys = app_keys_task.await?;
            let app_pubkey = app_keys.public_key();
            let public_key = signer.get_public_key_async().await?;

            // Construct a filter to get the latest approval event
            let filter = Filter::new()
                .kind(Kind::Custom(4455))
                .author(public_key)
                .pubkey(app_pubkey)
                .limit(1);

            match client.database().query(filter).await?.into_iter().next() {
                // Found an approval event
                Some(event) => Ok(Some(event)),
                // No approval event found, construct a request event
                None => {
                    // Construct an event for device key request
                    let event = EventBuilder::new(Kind::Custom(4454), "")
                        .tags(vec![
                            Tag::custom("P", vec![app_pubkey]),
                            Tag::custom("client", vec![CLIENT_NAME]),
                        ])
                        .finalize_async(&signer)
                        .await?;

                    // Send the event to write relays
                    client.send_event(&event).to_nip65().await?;

                    Ok(None)
                }
            }
        });

        self.tasks.push(cx.spawn(async move |this, cx| {
            match task.await {
                Ok(Some(event)) => {
                    this.update(cx, |this, cx| {
                        this.extract_encryption(event, cx);
                    })?;
                }
                Ok(None) => {
                    this.update(cx, |this, cx| {
                        this.wait_for_approval(cx);
                    })?;
                }
                Err(e) => {
                    this.update(cx, |_this, cx| {
                        cx.emit(DeviceEvent::error(e.to_string()));
                    })?;
                }
            };
            Ok(())
        }));
    }

    /// Wait for encryption key approvals
    fn wait_for_approval(&mut self, cx: &mut Context<Self>) {
        let nostr = NostrRegistry::global(cx);
        let client = nostr.read(cx).client();
        let signer = nostr.read(cx).signer();

        cx.emit(DeviceEvent::Requesting);

        self.tasks.push(cx.background_spawn(async move {
            let public_key = signer.get_public_key_async().await?;

            // Construct a filter for device key requests
            let filter = Filter::new()
                .kind(Kind::Custom(4455))
                .author(public_key)
                .since(Timestamp::now());

            // Subscribe to the device key requests on user's write relays
            client.subscribe(filter).await?;

            Ok(())
        }));
    }

    /// Parse the approval event to get encryption key then set it
    fn extract_encryption(&mut self, event: Event, cx: &mut Context<Self>) {
        let app_keys_task = get_or_init_app_keys(cx);

        let task: Task<Result<Keys, Error>> = cx.background_spawn(async move {
            let app_keys = app_keys_task.await?;
            let master = event
                .tags
                .iter()
                .find(|tag| tag.kind() == "P")
                .and_then(|tag| tag.content())
                .and_then(|content| PublicKey::parse(content).ok())
                .context("Invalid event tags")?;

            let payload = event.content.as_str();
            let decrypted = app_keys.nip44_decrypt_async(&master, payload).await?;

            let secret = SecretKey::from_hex(&decrypted)?;
            let keys = Keys::new(secret);

            Ok(keys)
        });

        self.tasks.push(cx.spawn(async move |this, cx| {
            match task.await {
                Ok(keys) => {
                    this.update(cx, |this, cx| {
                        this.set_signer(keys, cx);
                    })?;
                }
                Err(e) => {
                    this.update(cx, |_this, cx| {
                        cx.emit(DeviceEvent::error(e.to_string()));
                    })?;
                }
            }
            Ok(())
        }));
    }

    /// Approve requests for device keys from other devices
    fn approve(&mut self, event: &Event, window: &mut Window, cx: &mut Context<Self>) {
        let nostr = NostrRegistry::global(cx);
        let client = nostr.read(cx).client();
        let signer = nostr.read(cx).signer();

        // Get user's write relays
        let event = event.clone();
        let id: SharedString = event.id.to_hex().into();

        let task: Task<Result<(), Error>> = cx.background_spawn(async move {
            // Get device keys
            let keys = get_keys(&client, &signer).await?;
            let secret = keys.secret_key().to_secret_hex();

            // Extract the target public key from the event tags
            let target = event
                .tags
                .iter()
                .find(|tag| tag.kind() == "P")
                .and_then(|tag| tag.content())
                .and_then(|content| PublicKey::parse(content).ok())
                .context("Target is not a valid public key")?;

            // Encrypt the device keys with the user's signer
            let payload = keys.nip44_encrypt_async(&target, &secret).await?;

            // Construct the response event
            //
            // P tag: the current device's public key
            // p tag: the requester's public key
            let event = EventBuilder::new(Kind::Custom(4455), payload)
                .tags(vec![
                    Tag::custom("P", vec![keys.public_key().to_hex()]),
                    Tag::public_key(target),
                ])
                .finalize_async(&signer)
                .await?;

            // Send the response event to the user's relay list
            client.send_event(&event).to_nip65().await?;

            Ok(())
        });

        self.tasks.push(cx.spawn_in(window, async move |_this, cx| {
            match task.await {
                Ok(_) => {
                    cx.update(|window, cx| {
                        window.clear_notification_by_id::<DeviceNotification>(id, cx);
                    })
                    .ok();
                }
                Err(e) => {
                    cx.update(|window, cx| {
                        window.push_notification(
                            Notification::error(e.to_string()).autohide(false),
                            cx,
                        );
                    })
                    .ok();
                }
            };

            Ok(())
        }));
    }

    /// Handle encryption request
    fn ask_for_approval(&mut self, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        // Ignore if there is already a pending request
        if self.pending_request {
            return;
        }
        self.set_pending_request(true, cx);

        // Show notification
        let notification = self.notification(event, cx);
        window.push_notification(notification, cx);
    }

    /// Build a notification for the encryption request.
    fn notification(&self, event: Event, cx: &Context<Self>) -> Notification {
        const MSG: &str = "You've requested an encryption key from another device. \
                           Approve to allow Goop to share the key with that device.";

        let request = Announcement::from(&event);
        let persons = PersonRegistry::global(cx);
        let profile = persons.read(cx).get(&request.public_key(), cx);

        let entity = cx.entity().downgrade();
        let loading = Rc::new(Cell::new(false));
        let key = SharedString::from(event.id.to_hex());

        Notification::new()
            .type_id::<DeviceNotification>(key)
            .autohide(false)
            .with_kind(NotificationKind::Info)
            .title("Encryption Key Request")
            .content(move |_this, _window, cx| {
                v_flex()
                    .gap_2()
                    .text_sm()
                    .child(
                        div()
                            .text_sm()
                            .line_height(relative(1.25))
                            .child(SharedString::from(MSG)),
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                v_flex()
                                    .gap_1()
                                    .text_sm()
                                    .child(
                                        div()
                                            .font_semibold()
                                            .text_xs()
                                            .text_color(cx.theme().text_muted)
                                            .child(SharedString::from("From:")),
                                    )
                                    .child(
                                        div()
                                            .h_8()
                                            .w_full()
                                            .px_2()
                                            .rounded(cx.theme().radius)
                                            .bg(cx.theme().elevated_surface_background)
                                            .child(
                                                h_flex()
                                                    .gap_2()
                                                    .child(Avatar::new(profile.avatar()).xsmall())
                                                    .child(profile.name()),
                                            ),
                                    ),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .text_sm()
                                    .child(
                                        div()
                                            .font_semibold()
                                            .text_xs()
                                            .text_color(cx.theme().text_muted)
                                            .child(SharedString::from("Client:")),
                                    )
                                    .child(
                                        div()
                                            .h_8()
                                            .w_full()
                                            .px_2()
                                            .rounded(cx.theme().radius)
                                            .bg(cx.theme().elevated_surface_background)
                                            .child(request.client_name()),
                                    ),
                            ),
                    )
                    .into_any_element()
            })
            .action(move |_this, _window, _cx| {
                let view = entity.clone();
                let event = event.clone();

                Button::new("approve")
                    .label("Approve")
                    .loading(loading.get())
                    .disabled(loading.get())
                    .on_click({
                        let loading = Rc::clone(&loading);
                        move |_ev, window, cx| {
                            // Set loading state to true
                            loading.set(true);
                            // Process to approve the request
                            view.update(cx, |this, cx| {
                                this.approve(&event, window, cx);
                            })
                            .ok();
                        }
                    })
            })
    }
}

struct DeviceNotification;

/// Get or create new app keys (async, returns a task)
fn get_or_init_app_keys(cx: &App) -> Task<Result<Keys, Error>> {
    let read = cx.read_credentials(CLIENT_NAME);

    cx.spawn(async move |cx| {
        if let Ok(Some((_, secret))) = read.await
            && let Ok(keys) = SecretKey::from_slice(&secret).map(Keys::new)
        {
            return Ok(keys);
        }

        // No stored keys found or invalid — generate new ones
        let keys = Keys::generate();
        let user = keys.public_key().to_hex();
        let secret = keys.secret_key().to_secret_bytes();

        cx.update(|cx| {
            let write = cx.write_credentials(CLIENT_NAME, &user, &secret);
            cx.background_spawn(async move {
                if let Err(e) = write.await {
                    log::error!("Keyring not available or panic: {e}")
                }
            })
            .detach();
        });

        Ok(keys)
    })
}

/// Encrypt and store device keys in the local database.
async fn set_keys(client: &Client, signer: &UniversalSigner, secret: &str) -> Result<(), Error> {
    let public_key = signer.get_public_key_async().await?;
    let content = signer.nip44_encrypt_async(&public_key, secret).await?;

    // Construct the application data event
    let event = EventBuilder::new(Kind::ApplicationSpecificData, content)
        .tag(Tag::identifier(IDENTIFIER))
        .finalize_async(signer)
        .await?;

    // Save the event to the database
    client.database().save_event(&event).await?;

    Ok(())
}

/// Get device keys from the local database.
async fn get_keys(client: &Client, signer: &UniversalSigner) -> Result<Keys, Error> {
    let public_key = signer.get_public_key_async().await?;

    let filter = Filter::new()
        .kind(Kind::ApplicationSpecificData)
        .identifier(IDENTIFIER)
        .author(public_key);

    if let Some(event) = client.database().query(filter).await?.first() {
        let content = signer
            .nip44_decrypt_async(&public_key, &event.content)
            .await?;

        let secret = SecretKey::parse(&content)?;
        let keys = Keys::new(secret);

        Ok(keys)
    } else {
        Err(anyhow!("Key not found"))
    }
}
