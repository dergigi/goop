use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, hash_map};
#[cfg(test)]
use std::sync::LazyLock;
use std::sync::{Arc, RwLock};

use anyhow::{Error, anyhow};
use decryption::DecryptQueue;
use futures::StreamExt;
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use gpui::{
    App, AppContext, Context, Entity, EventEmitter, Global, Subscription, Task, WeakEntity, Window,
};
use instant::{Duration, Instant};
use nostr_sdk::prelude::*;
use smallvec::{SmallVec, smallvec};
use state::{NostrRegistry, StateEvent, USER_GIFTWRAP, UniversalSigner};
mod cache;
mod search;
pub use search::SearchMessage;
use cache::RumorCache;
mod decryption;
mod history;
mod inbox;
mod unread;
pub use unread::ReadPosition;
mod archive;
mod moderation;
mod outgoing;
#[cfg(test)]
mod test_signer;
#[cfg(test)]
mod test_state_dir;
pub use history::RelayHistory;
use outgoing::{OutgoingMessage, OutgoingQueue};

mod message;
mod room;
mod room_loader;

pub use message::*;
pub use room::*;

/// Test-only local provenance key; production uses its persisted cache key.
#[cfg(test)]
static LOCAL_KEYS: LazyLock<Keys> = LazyLock::new(Keys::generate);

pub fn init(window: &mut Window, cx: &mut App) {
    ChatRegistry::set_global(cx.new(|cx| ChatRegistry::new(window, cx)), cx);
}

struct GlobalChatRegistry(Entity<ChatRegistry>);

impl Global for GlobalChatRegistry {}

/// Chat event.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChatEvent {
    /// An event to open a room by its ID
    OpenRoom(u64),
    /// Show a message author in the workspace profile sidepane.
    OpenProfile(PublicKey),
    /// An event to close a room by its ID
    CloseRoom(u64),
    /// An event to notify UI about a new chat request
    Ping,
    /// No Inbox Relays found, the app is not ready to subscribe messages
    InboxRelayNotFound,
    /// An error occurred
    Error(String),
}

/// Channel signal.
#[derive(Debug, Clone)]
enum Signal {
    /// Inbox Relays found, the app is ready to subscribe messages
    InboxReady,
    ContactsChanged,
    Decrypted(EventId, Result<NewMessage, FailedMessage>),
    History(RelayUrl, RelayHistory),
    Outgoing(OutgoingMessage),
    OutgoingError(String),
    OutgoingRecovered,
}

/// Chat Registry
#[derive(Debug)]
pub struct ChatRegistry {
    /// Chat rooms
    rooms: Vec<Entity<Room>>,

    /// O(1) room lookup by room ID
    room_index: HashMap<u64, Entity<Room>>,

    /// Events that failed to unwrap for any reason
    trash: Entity<BTreeSet<FailedMessage>>,

    /// Tracking events seen on which relays in the current session
    seen: Arc<RwLock<HashMap<EventId, HashSet<RelayUrl>>>>,

    /// Mapping of unwrapped event ids to their gift wrap event ids
    event_map: Arc<RwLock<HashMap<EventId, EventId>>>,

    history: BTreeMap<RelayUrl, RelayHistory>,
    history_running: bool,
    pending_history: Option<bool>,
    history_error: Option<String>,
    last_history: Option<Instant>,
    queue: Option<DecryptQueue>,
    history_task: Option<Task<Result<(), Error>>>,
    decrypt_task: Option<Task<Result<(), Error>>>,
    retry_task: Option<Task<Result<(), Error>>>,
    outgoing_task: Option<Task<Result<(), Error>>>,
    outgoing: Option<OutgoingQueue>,
    moderation: Option<moderation::ModerationStore>,
    moderation_task: Option<Task<()>>,
    pub moderation_error: Option<String>,
    archives: Option<archive::ArchiveStore>,
    archive_task: Option<Task<()>>,
    pub archive_error: Option<String>,
    incoming: Option<RumorCache>,
    inbox: Option<inbox::Inbox>,
    reads: Option<unread::ReadStore>,
    contacts: HashSet<PublicKey>,
    classification_ready: bool,
    room_reload: room_loader::ReloadState,
    room_load_task: Option<Task<Result<(), Error>>>,
    outgoing_reports: HashMap<EventId, Vec<SendReport>>,
    outgoing_error: Option<String>,

    /// Channel for sending signals to the UI.
    signal_tx: flume::Sender<Signal>,

    /// Channel for receiving signals from the UI.
    signal_rx: flume::Receiver<Signal>,

    /// Async tasks
    tasks: SmallVec<[Task<Result<(), Error>>; 2]>,

    /// Notification listener task (cancelled on signer change)
    notification_listener: Option<Task<Result<(), Error>>>,

    /// Signal consumer task (cancelled on signer change)
    signal_consumer: Option<Task<Result<(), Error>>>,

    /// Fuzzy matcher for room search (cached; intentionally excluded from Debug)
    #[allow(dead_code)]
    matcher: CachedMatcher,

    /// Subscriptions
    _subscriptions: SmallVec<[Subscription; 2]>,
}

/// Wrapper to provide Debug for SkimMatcherV2
struct CachedMatcher(SkimMatcherV2);

impl std::fmt::Debug for CachedMatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CachedMatcher { .. }")
    }
}

impl std::ops::Deref for CachedMatcher {
    type Target = SkimMatcherV2;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl EventEmitter<ChatEvent> for ChatRegistry {}

impl ChatRegistry {
    /// Retrieve the global chat registry state
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalChatRegistry>().0.clone()
    }

    /// Set the global chat registry instance
    fn set_global(state: Entity<Self>, cx: &mut App) {
        cx.set_global(GlobalChatRegistry(state));
    }

    /// Create a new chat registry instance
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let nostr = NostrRegistry::global(cx);
        let (tx, rx) = flume::bounded::<Signal>(256);
        let mut subscriptions = smallvec![];

        subscriptions.push(
            // Subscribe to the signer event
            cx.subscribe(&nostr, |this, _nostr, event, cx| {
                if event.signer_changed() {
                    this.reset(cx);
                    this.handle_notifications(cx);
                    this.start_archives(cx);
                    this.start_moderation(cx);
                    this.get_metadata(cx);
                    this.get_rooms(cx);
                } else if matches!(event, StateEvent::NoSigner) {
                    this.reset(cx);
                };
            }),
        );

        // Run at the end of the current cycle
        cx.defer_in(window, |this, _window, cx| {
            this.get_rooms(cx);
            if this.archives.is_none() { this.start_archives(cx); }
            if this.moderation.is_none() { this.start_moderation(cx); }
        });

        Self {
            rooms: vec![],
            room_index: HashMap::new(),
            trash: cx.new(|_| BTreeSet::default()),
            seen: Arc::new(RwLock::new(HashMap::default())),
            event_map: Arc::new(RwLock::new(HashMap::default())),
            history: BTreeMap::new(),
            history_running: false,
            pending_history: None,
            history_error: None,
            last_history: None,
            queue: None,
            history_task: None,
            decrypt_task: None,
            retry_task: None,
            outgoing_task: None,
            outgoing: None,
            moderation: None,
            moderation_task: None,
            moderation_error: None,
            archives: None,
            archive_task: None,
            archive_error: None,
            incoming: None,
            inbox: None,
            reads: None,
            contacts: HashSet::new(),
            classification_ready: false,
            room_reload: room_loader::ReloadState::default(),
            room_load_task: None,
            outgoing_reports: HashMap::new(),
            outgoing_error: None,
            matcher: CachedMatcher(SkimMatcherV2::default()),
            signal_rx: rx,
            signal_tx: tx,
            tasks: smallvec![],
            notification_listener: None,
            signal_consumer: None,
            _subscriptions: subscriptions,
        }
    }

    /// Route live deliveries into reserved capacity alongside downloaded history.
    fn handle_notifications(&mut self, cx: &mut Context<Self>) {
        let nostr = NostrRegistry::global(cx);
        let client = nostr.read(cx).client();
        let signer = nostr.read(cx).signer();
        let Some(user) = nostr.read(cx).current_user() else {
            return;
        };
        self.reads = match unread::ReadStore::open(&common::config_dir(), user) {
            Ok(reads) => Some(reads),
            Err(error) => { cx.emit(ChatEvent::Error(format!("Could not load read state: {error}"))); None }
        };
        self.inbox = match inbox::Inbox::open(common::config_dir(), user) {
            Ok(inbox) => Some(inbox),
            Err(error) => {
                cx.emit(ChatEvent::Error(format!(
                    "Could not load Inbox state: {error}"
                )));
                return;
            }
        };
        let (queue, receiver) = DecryptQueue::new();
        self.queue = Some(queue.clone());
        let decrypt_queue = queue.clone();
        let cache = match RumorCache::open(client.clone(), user) {
            Ok(cache) => cache,
            Err(error) => {
                cx.emit(ChatEvent::Error(format!(
                    "Could not open message cache: {error}"
                )));
                return;
            }
        };
        self.incoming = Some(cache.clone());
        let signer = signer.snapshot();
        let signals = self.signal_tx.clone();
        self.decrypt_task = Some(cx.background_spawn(async move {
            decrypt_queue.run(receiver, cache, signer, signals).await
        }));
        let seen = self.seen.clone();
        let tx = self.signal_tx.clone();
        self.notification_listener = Some(cx.background_spawn(async move {
            let mut notifications = client.notifications();
            while let Some(notification) = notifications.next().await {
                let ClientNotification::Message { message, relay_url } = notification else {
                    continue;
                };
                if let RelayMessage::Event {
                    event,
                    subscription_id,
                } = *message
                {
                    if event.kind == Kind::ContactList && event.pubkey == user {
                        tx.send_async(Signal::ContactsChanged).await?;
                    }
                    if event.kind == Kind::InboxRelays && event.pubkey == user {
                        tx.send_async(Signal::InboxReady).await?;
                    }
                    if event.kind == Kind::GiftWrap
                        && event.tags.public_keys().any(|key| key == user)
                    {
                        seen.write()
                            .unwrap()
                            .entry(event.id)
                            .or_default()
                            .insert(relay_url);
                        // History pages are persisted/enqueued by the page worker.
                        if subscription_id.as_ref().as_str() == USER_GIFTWRAP {
                            client.database().save_event(&event).await?;
                            queue.enqueue_live(event.into_owned()).await?;
                        }
                    }
                }
            }
            Ok(())
        }));
        let rx = self.signal_rx.clone();
        self.signal_consumer = Some(cx.spawn(async move |this, cx| {
            let mut budget = common::UiWorkBudget::default();
            while let Ok(signal) = rx.recv_async().await {
                this.update(cx, |this, cx| {
                    match signal {
                        Signal::ContactsChanged => this.get_rooms(cx),
                        Signal::InboxReady => this.get_messages(cx),
                        Signal::Outgoing(message) => {
                            this.outgoing_reports
                                .insert(message.id(), message.reports());
                            let mut incoming = NewMessage::new(message.id(), message.rumor);
                            incoming.historical = true;
                            this.new_message(incoming, cx);
                        }
                        Signal::OutgoingError(error) => {
                            this.outgoing_error = Some(error.clone());
                            cx.emit(ChatEvent::Error(error));
                        }
                        Signal::OutgoingRecovered => this.outgoing_error = None,
                        Signal::History(relay, progress) => {
                            this.history.insert(relay, progress);
                        }
                        Signal::Decrypted(id, result) => {
                            this.trash.update(cx, |trash, cx| {
                                trash.retain(|failed| failed.event_id != id);
                                if let Err(failed) = &result {
                                    trash.insert(failed.clone());
                                }
                                cx.notify();
                            });
                            if let Ok(message) = result {
                                if let Some(rumor_id) = message.rumor.id {
                                    this.event_map.write().unwrap().insert(rumor_id, id);
                                }
                                this.new_message(message, cx);
                            }
                        }
                    }
                    cx.notify();
                })?;
                budget.checkpoint(cx.background_executor()).await;
            }
            Ok(())
        }));
        self.start_outgoing(cx);
    }

    fn start_outgoing(&mut self, cx: &mut Context<Self>) {
        let nostr = NostrRegistry::global(cx);
        let Some(owner) = nostr.read(cx).current_user() else {
            return;
        };
        let client = nostr.read(cx).client();
        let signer = nostr.read(cx).signer().snapshot();
        let (queue, wake) = OutgoingQueue::new(client, owner);
        self.outgoing = Some(queue.clone());
        let signals = self.signal_tx.clone();
        self.outgoing_task =
            Some(cx.background_spawn(async move { queue.run(signer, wake, signals).await }));
    }

    pub(crate) fn incoming_cache(&self) -> Option<RumorCache> {
        self.incoming.clone()
    }

    pub(crate) fn outgoing_queue(&self) -> Option<OutgoingQueue> {
        self.outgoing.clone()
    }

    /// Snapshot already loaded message text without disk or relay access.
    pub fn has_unread(&self, room: u64) -> bool { self.unread_count(room) > 0 }
    pub fn unread_count(&self, room: u64) -> usize {
        match (&self.reads, &self.incoming) {
            (Some(reads), Some(cache)) => match &self.moderation {
                Some(store) => cache.unread_count_filtered(room, reads, &store.blocked_snapshot()),
                None => cache.unread_count(room, reads),
            },
            _ => 0,
        }
    }
    pub fn set_room_read(&mut self, room: u64, read: bool, cx: &mut Context<Self>) -> Result<(), Error> {
        anyhow::ensure!(self.room_index.contains_key(&room), "Conversation not found");
        let cache = self.incoming.as_ref().ok_or_else(|| anyhow!("Connect your account before changing read state"))?;
        let reads = self.reads.as_mut().ok_or_else(|| anyhow!("Read state is unavailable"))?;
        let changed = if read { reads.mark_many(&cache.read_positions(&[room]))? }
            else { reads.mark_unread(room)? };
        if changed { cx.notify(); }
        Ok(())
    }
    pub fn mark_list_read(&mut self, filter: &RoomKind, cx: &mut Context<Self>) -> Result<(), Error> {
        let rooms: Vec<_> = self.rooms(filter, cx).iter().map(|room| room.read(cx).id).collect();
        let cache = self.incoming.as_ref().ok_or_else(|| anyhow!("Connect your account before marking chats read"))?;
        let positions = cache.read_positions(&rooms);
        let reads = self.reads.as_mut().ok_or_else(|| anyhow!("Read state is unavailable"))?;
        if reads.mark_many(&positions)? { cx.notify(); }
        Ok(())
    }
    pub fn mark_read(&mut self, owner: PublicKey, room: u64, position: &ReadPosition, cx: &mut Context<Self>) {
        if NostrRegistry::global(cx).read(cx).current_user() != Some(owner) { return; }
        if let Some(reads) = &mut self.reads {
            match reads.mark(room, position) {
                Ok(true) => cx.notify(),
                Ok(false) => {},
                Err(error) => cx.emit(ChatEvent::Error(format!("Could not save read state: {error}"))),
            }
        }
    }
    pub fn search_messages(&self, room: u64) -> Vec<Arc<SearchMessage>> {
        self.incoming.as_ref().map(|cache| cache.search_messages(room).into_iter().filter(|message| !self.is_blocked(message.author)).collect()).unwrap_or_default()
    }

    pub fn outgoing_reports(&self, id: &EventId) -> Option<Vec<SendReport>> {
        self.outgoing_reports.get(id).cloned()
    }

    fn start_moderation(&mut self, cx: &mut Context<Self>) {
        let nostr = NostrRegistry::global(cx).read(cx);
        let Some(owner) = nostr.current_user() else { return; };
        let client = nostr.client();
        let signer = nostr.signer().snapshot();
        let (store, wake) = match moderation::ModerationStore::open(&common::config_dir(), owner) {
            Ok(value) => value,
            Err(error) => { self.moderation_error = Some(error.to_string()); return; }
        };
        self.moderation = Some(store.clone());
        self.moderation_task = Some(cx.spawn(async move |this, cx| {
            loop {
                if !store.paused.load(std::sync::atomic::Ordering::SeqCst) {
                    let (sync_store, client, signer) = (store.clone(), client.clone(), signer.clone());
                    use futures::FutureExt;
                    let sync = cx.background_spawn(async move { sync_store.sync(&client, &signer).await });
                    let timeout = cx.background_executor().timer(std::time::Duration::from_secs(60));
                    let result = match futures::future::select(sync.boxed(), timeout.boxed()).await {
                        futures::future::Either::Left((result, _)) => result,
                        futures::future::Either::Right(_) => Err(anyhow!(state::SignerFailure::Timeout)),
                    };
                    if let Err(error) = &result {
                        if state::SignerFailure::classify(error.as_ref()).requires_retry() {
                            store.paused.store(true, std::sync::atomic::Ordering::SeqCst);
                        }
                    }
                    if this.update(cx, |this, cx| {
                        this.moderation_error = result.err().map(|error| error.to_string());
                        cx.notify();
                    }).is_err() { break; }
                }
                use futures::{FutureExt, select_biased};
                let request = wake.recv_async().fuse();
                let timer = cx.background_executor().timer(std::time::Duration::from_secs(30)).fuse();
                futures::pin_mut!(request, timer);
                select_biased! { _ = request => {}, _ = timer => {} }
            }
        }));
        cx.notify();
    }
    pub fn blocked_users(&self) -> BTreeSet<PublicKey> { self.moderation.as_ref().map(|store| store.blocked()).unwrap_or_default() }
    pub fn is_blocked(&self, key: PublicKey) -> bool { self.moderation.as_ref().is_some_and(|store| store.is_blocked(key)) }
    pub fn room_blocked(&self, room: &Room) -> bool { self.moderation.as_ref().is_some_and(|store| moderation::hides_room(&store.blocked_snapshot(), room.members())) }
    pub fn is_muted(&self, key: PublicKey) -> bool { self.moderation.as_ref().is_some_and(|store| store.muted(key)) }
    pub fn moderation_pending(&self) -> bool { self.moderation.as_ref().is_some_and(|store| store.pending()) }
    pub fn mute_user(&mut self, key: PublicKey, seconds: Option<u64>, cx: &mut Context<Self>) -> Result<(), Error> {
        self.moderation.as_ref().ok_or_else(|| anyhow!("Connect your signer before muting"))?.mute(key, seconds)?;
        cx.notify(); Ok(())
    }
    pub fn block_user(&mut self, key: PublicKey, blocked: bool, cx: &mut Context<Self>) -> Result<(), Error> {
        self.moderation.as_ref().ok_or_else(|| anyhow!("Connect your signer before blocking"))?.block(key, blocked)?;
        cx.notify(); Ok(())
    }
    pub fn retry_moderation(&mut self, cx: &mut Context<Self>) {
        if let Some(store) = &self.moderation { store.retry(); } else { self.start_moderation(cx); }
    }
    fn start_archives(&mut self, cx: &mut Context<Self>) {
        let nostr = NostrRegistry::global(cx).read(cx);
        let Some(owner) = nostr.current_user() else { return; };
        let client = nostr.client();
        let signer = nostr.signer().snapshot();
        let (store, wake) = match archive::ArchiveStore::open(&common::config_dir(), owner) {
            Ok(value) => value,
            Err(error) => { self.archive_error = Some(error.to_string()); return; }
        };
        self.archives = Some(store.clone());
        self.archive_task = Some(cx.spawn(async move |this, cx| {
            loop {
                if !store.paused.load(std::sync::atomic::Ordering::SeqCst) {
                    let (sync_store, client, signer) = (store.clone(), client.clone(), signer.clone());
                    let result = cx.background_spawn(async move { sync_store.sync(&client, &signer).await }).await;
                    if let Err(error) = &result {
                        if state::SignerFailure::classify(error.as_ref()).requires_retry() {
                            store.paused.store(true, std::sync::atomic::Ordering::SeqCst);
                        }
                    }
                    if this.update(cx, |this, cx| {
                        this.archive_error = result.err().map(|error| error.to_string());
                        cx.notify();
                    }).is_err() { break; }
                }
                use futures::{FutureExt, select_biased};
                let request = wake.recv_async().fuse();
                let timer = cx.background_executor().timer(std::time::Duration::from_secs(30)).fuse();
                futures::pin_mut!(request, timer);
                select_biased! { _ = request => {}, _ = timer => {} }
            }
        }));
        cx.notify();
    }
    pub fn retry_archives(&mut self, cx: &mut Context<Self>) {
        if let Some(store) = &self.archives { store.retry(); } else { self.start_archives(cx); }
    }
    pub fn is_pinned(&self, room: &Room) -> bool {
        self.archives.as_ref().is_some_and(|store| store.is_pinned(room.members()))
    }
    pub fn set_pinned(&mut self, id: u64, pinned: bool, cx: &mut Context<Self>) -> Result<(), Error> {
        let room = self.room_index.get(&id).ok_or_else(|| anyhow!("Conversation not found"))?.read(cx);
        let store = self.archives.as_ref().ok_or_else(|| anyhow!("Connect your signer before pinning"))?;
        store.pin(room.members(), pinned)?;
        cx.notify();
        Ok(())
    }
    pub fn is_archived(&self, room: &Room) -> bool {
        self.archives.as_ref().is_some_and(|store| store.contains(room.members()) || store.has_left(room.members()))
    }
    pub fn notifications_muted(&self, author: PublicKey, members: &[PublicKey]) -> bool {
        if self.is_blocked(author) || self.is_muted(author) { return true; }
        self.archives.as_ref().is_some_and(|store| store.has_left(members))
    }
    pub fn has_left(&self, room: &Room) -> bool {
        self.archives.as_ref().is_some_and(|store| store.has_left(room.members()))
    }
    pub fn set_archived(&mut self, id: u64, archived: bool, cx: &mut Context<Self>) -> Result<(), Error> {
        let room = self.room_index.get(&id).ok_or_else(|| anyhow!("Conversation not found"))?.read(cx);
        let store = self.archives.as_ref().ok_or_else(|| anyhow!("Connect your signer before archiving"))?;
        store.set(room.members(), archived)?;
        cx.notify();
        Ok(())
    }
    pub fn leave_locally(&mut self, id: u64, left: bool, cx: &mut Context<Self>) -> Result<(), Error> {
        let room = self.room_index.get(&id).ok_or_else(|| anyhow!("Conversation not found"))?.read(cx);
        anyhow::ensure!(room.is_group(), "Leave locally is available for group chats");
        let store = self.archives.as_ref().ok_or_else(|| anyhow!("Connect your signer before leaving"))?;
        store.leave(room.members(), left)?;
        cx.notify();
        Ok(())
    }

    pub fn rebroadcast(&self, id: EventId, cx: &App) -> Task<Result<(), anyhow::Error>> {
        let queue = self.outgoing.clone();
        cx.background_spawn(async move {
            let queue = queue.ok_or_else(|| anyhow::anyhow!("Connect your signer before rebroadcasting"))?;
            queue.rebroadcast(id).await
        })
    }

    pub fn retry_outgoing(&self) {
        if let Some(queue) = &self.outgoing {
            queue.retry();
        }
    }

    pub fn get_metadata(&mut self, cx: &mut Context<Self>) {
        let nostr = NostrRegistry::global(cx);
        let client = nostr.read(cx).client();

        let Some(public_key) = nostr.read(cx).current_user() else {
            return;
        };

        self.tasks.push(cx.spawn(async move |this, cx| {
            // Subscribe to metadata from relays
            let opts = SubscribeAutoCloseOptions::default().exit_policy(ReqExitPolicy::ExitOnEOSE);

            let msg_relays = Filter::new()
                .kind(Kind::InboxRelays)
                .author(public_key)
                .limit(1);

            let contact_list = Filter::new()
                .kind(Kind::ContactList)
                .author(public_key)
                .limit(1);

            _ = client
                .subscribe(vec![msg_relays, contact_list])
                .close_on(opts)
                .await;

            // Give relays time to respond
            cx.background_executor().timer(Duration::from_secs(5)).await;

            // Verify inbox relays were received
            let filter = Filter::new()
                .kind(Kind::InboxRelays)
                .author(public_key)
                .limit(1);

            let found = client
                .database()
                .query(filter)
                .await
                .unwrap_or_default()
                .into_iter()
                .next()
                .is_some();

            if !found {
                this.update(cx, |_this, cx| {
                    cx.emit(ChatEvent::InboxRelayNotFound);
                })?;
            } else {
                this.update(cx, |this, cx| this.ensure_history(cx))?;
            }

            Ok(())
        }));
    }

    fn get_messages(&mut self, cx: &mut Context<Self>) {
        self.start_history(false, false, cx);
    }

    /// Resume history automatically on opening/scanning a chat, with a cooldown.
    pub fn ensure_history(&mut self, cx: &mut Context<Self>) {
        if self
            .last_history
            .is_none_or(|last| last.elapsed() >= Duration::from_secs(60))
        {
            self.start_history(false, false, cx);
        }
    }

    /// Explicitly rescan the entire retained history, including previously scanned gaps.
    /// Resume interrupted history using saved checkpoints without forcing a full rescan.
    pub fn resume_history(&mut self, cx: &mut Context<Self>) {
        self.start_history(false, false, cx);
    }

    pub fn load_older_history(&mut self, cx: &mut Context<Self>) {
        self.start_history(true, false, cx);
    }

    pub fn search_other_relays(&mut self, cx: &mut Context<Self>) {
        self.start_history(true, true, cx);
    }

    fn start_history(&mut self, force: bool, include_general: bool, cx: &mut Context<Self>) {
        if self.history_running {
            if force {
                // Preserve an explicit recovery request while a scan is active.
                self.pending_history =
                    Some(self.pending_history.unwrap_or(false) || include_general);
                cx.notify();
            }
            return;
        }
        let Some(queue) = self.queue.clone() else {
            return;
        };
        let nostr = NostrRegistry::global(cx);
        let Some(user) = nostr.read(cx).current_user() else {
            return;
        };
        let client = nostr.read(cx).client();
        let signals = self.signal_tx.clone();
        self.history_running = true;
        self.history_error = None;
        self.last_history = Some(Instant::now());
        self.history.clear();
        cx.notify();
        let task = cx.background_spawn(async move {
            // Replay locally saved ciphertext, including failures from an earlier
            // session, before advancing the persisted network checkpoint.
            let cached = client
                .database()
                .query(Filter::new().kind(Kind::GiftWrap).pubkey(user))
                .await?;
            for event in cached {
                queue.schedule(event.id);
            }
            let event = client
                .database()
                .query(Filter::new().kind(Kind::InboxRelays).author(user).limit(1))
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| anyhow!("No inbox relays found"))?;
            let relays: BTreeSet<RelayUrl> = nip17::extract_relay_list(&event).collect();
            if relays.is_empty() {
                return Err(anyhow!("No inbox relays configured"));
            }
            for relay in &relays {
                client.add_relay(relay).await?;
                signals
                    .send_async(Signal::History(relay.clone(), RelayHistory::default()))
                    .await?;
            }
            // Keep live traffic separate; old history has its own resumable scans.
            let live = Filter::new()
                .kind(Kind::GiftWrap)
                .pubkey(user)
                .since(Timestamp::from(
                    Timestamp::now().as_secs().saturating_sub(2 * 24 * 60 * 60),
                ));
            let targets: HashMap<_, _> = relays
                .iter()
                .cloned()
                .map(|relay| (relay, live.clone()))
                .collect();
            let id = SubscriptionId::new(USER_GIFTWRAP);
            let _ = client.unsubscribe(&id).await;
            client.subscribe(targets).with_id(id).await?;
            let mut history_relays: Vec<_> = relays.iter().cloned().collect();
            if include_general {
                history_relays.extend(history::general_relays(&client, user, &relays).await?);
            }
            // Inbox relays are scheduled first; at most two scans run at once.
            let mut scans = futures::stream::iter(history_relays)
                .map(|relay| history::scan_relay(&client, user, relay, &queue, &signals, force))
                .buffer_unordered(2);
            while let Some(result) = scans.next().await {
                if let Err(error) = result {
                    log::warn!("History relay failed: {error}");
                }
            }
            Ok(())
        });
        self.history_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.history_running = false;
                if let Err(error) = &result {
                    this.history_error = Some(error.to_string());
                    cx.emit(ChatEvent::Error(error.to_string()));
                }
                if let Some(include_general) = this.pending_history.take() {
                    this.start_history(true, include_general, cx);
                }
                cx.notify();
            })?;
            result
        }));
    }

    pub fn retry_failed_messages(&mut self, cx: &mut Context<Self>) {
        let Some(queue) = self.queue.clone() else {
            return;
        };
        let events: Vec<_> = self
            .trash
            .read(cx)
            .iter()
            .filter_map(|failed| Event::from_json(failed.raw_event.as_ref()).ok())
            .collect();
        self.retry_task = Some(cx.background_spawn(async move {
            for event in events {
                queue.enqueue(event, true).await?;
            }
            Ok(())
        }));
        cx.notify();
    }

    pub fn decryption_failures(&self, cx: &App) -> BTreeMap<String, usize> {
        let mut reasons = BTreeMap::new();
        for failure in self.trash.read(cx).iter() {
            *reasons.entry(failure.reason.to_string()).or_default() += 1;
        }
        reasons
    }

    /// Current-account delivery reports for diagnostics; contains no message text.
    pub fn delivery_reports(&self) -> impl Iterator<Item = &Vec<SendReport>> {
        self.outgoing_reports.values()
    }

    pub fn outgoing_error(&self) -> Option<&str> { self.outgoing_error.as_deref() }

    pub fn history_error(&self) -> Option<&str> {
        self.history_error.as_deref()
    }

    pub fn history_relays(&self) -> &BTreeMap<RelayUrl, RelayHistory> {
        &self.history
    }
    pub fn history_running(&self) -> bool {
        self.history_running
    }
    pub fn pending_messages(&self) -> usize {
        self.queue.as_ref().map_or(0, |queue| queue.pending())
    }
    pub fn history_summary(&self, cx: &App) -> String {
        let loaded = self.queue.as_ref().map_or(0, |queue| queue.loaded());
        let pending = self.pending_messages();
        let received: usize = self.history.values().map(|relay| relay.received).sum();
        let errors = self
            .history
            .values()
            .filter(|relay| relay.error.is_some())
            .count();
        let failed = self.count_trash_messages(cx);
        if let Some(error) = &self.history_error {
            return format!("History incomplete · {error} · {pending} pending · {failed} failed");
        }
        let status = if self.pending_history == Some(true) {
            "Loading history · broader relay search queued"
        } else if self.pending_history.is_some() {
            "Loading history · full rescan queued"
        } else if self.history_running {
            "Loading history"
        } else if pending > 0 {
            "Decrypting messages"
        } else if errors > 0 {
            "History incomplete"
        } else if self.last_history.is_none() {
            "History not scanned yet"
        } else {
            "History checked"
        };
        format!(
            "{status} · {received} received · {loaded} loaded · {pending} pending · {failed} failed · {errors} relay errors"
        )
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.get_metadata(cx);
        self.retry_outgoing();
        self.load_older_history(cx);
        self.retry_failed_messages(cx);
        self.get_rooms(cx);
    }

    pub fn loading(&self) -> bool {
        (self.inbox.is_some() && !self.classification_ready)
            || self.history_running
            || self.pending_messages() > 0
    }

    /// Get a weak reference to a room by its ID
    pub fn room(&self, id: &u64, _cx: &App) -> Option<WeakEntity<Room>> {
        self.room_index.get(id).map(|room| room.downgrade())
    }

    /// Get all rooms based on the filter.
    pub fn rooms(&self, filter: &RoomKind, cx: &App) -> Vec<Entity<Room>> {
        self.rooms
            .iter()
            .filter(|_| *filter != RoomKind::Request || self.classification_ready)
            .filter(|room| {
                let room = room.read(cx);
                if self.room_blocked(room) { return false; }
                if *filter == RoomKind::Archived { self.is_archived(room) }
                else { &room.kind == filter && !self.is_archived(room) }
            })
            .cloned()
            .collect()
    }

    /// Count the number of rooms based on the filter.
    pub fn count(&self, filter: &RoomKind, cx: &App) -> usize {
        self.rooms
            .iter()
            .filter(|_| *filter != RoomKind::Request || self.classification_ready)
            .filter(|room| {
                let room = room.read(cx);
                if self.room_blocked(room) { return false; }
                if *filter == RoomKind::Archived { self.is_archived(room) }
                else { &room.kind == filter && !self.is_archived(room) }
            })
            .count()
    }

    /// Count the number of messages seen by a given relay.
    pub fn count_messages(&self, relay_url: &RelayUrl) -> usize {
        self.seen
            .read()
            .unwrap()
            .values()
            .filter(|s| s.contains(relay_url))
            .count()
    }

    /// Count the number of trash messages.
    pub fn count_trash_messages(&self, cx: &App) -> usize {
        self.trash.read(cx).len()
    }

    /// Get the trash messages entity.
    pub fn trash(&self) -> Entity<BTreeSet<FailedMessage>> {
        self.trash.clone()
    }

    /// Get the relays that have seen a given rumor id.
    pub fn rumor_seen_on(&self, id: &EventId) -> Option<HashSet<RelayUrl>> {
        self.event_map
            .read()
            .unwrap()
            .get(id)
            .map(|id| self.seen_on(id))
    }

    /// Get the relays that have seen a given gift wrap id.
    pub fn seen_on(&self, id: &EventId) -> HashSet<RelayUrl> {
        self.seen
            .read()
            .unwrap()
            .get(id)
            .cloned()
            .unwrap_or_default()
    }

    /// Accepting is local to this account and does not send a message or follow anyone.
    pub fn accept_room(&mut self, id: u64, cx: &mut Context<Self>) -> bool {
        let Some(inbox) = &mut self.inbox else {
            return false;
        };
        if let Err(error) = inbox.remember([id]) {
            cx.emit(ChatEvent::Error(format!(
                "Could not save acceptance: {error}"
            )));
            return false;
        }
        if let Some(room) = self.room_index.get(&id) {
            room.update(cx, |room, cx| room.set_ongoing(cx));
        }
        cx.notify();
        true
    }

    /// Add a new room to the start of list.
    pub fn add_room<I>(&mut self, room: I, cx: &mut Context<Self>)
    where
        I: Into<Room>,
    {
        let nostr = NostrRegistry::global(cx);
        let Some(public_key) = nostr.read(cx).current_user() else {
            return;
        };

        let mut room: Room = room.into().organize(&public_key);
        if self
            .inbox
            .as_ref()
            .is_some_and(|inbox| inbox.contains(room.id))
            || room.members.iter().any(|key| self.contacts.contains(key))
        {
            room.kind = RoomKind::Ongoing;
        }
        let room_id = room.id;
        if room.kind == RoomKind::Ongoing {
            self.accept_room(room_id, cx);
        }
        let entity = cx.new(|_| room);

        self.room_index.insert(room_id, entity.clone());
        self.rooms.insert(0, entity);

        if !self.room_index.get(&room_id).is_some_and(|room| self.has_left(room.read(cx))) { cx.emit(ChatEvent::Ping); }
        cx.notify();
    }

    /// Emit an open room event.
    ///
    /// If the room is new, add it to the registry.
    pub fn emit_room(&mut self, room: &Entity<Room>, window: &mut Window, cx: &mut Context<Self>) {
        // Get the room's ID.
        let id = room.read(cx).id;

        // If the room is new, add it to the registry and index.
        if let hash_map::Entry::Vacant(e) = self.room_index.entry(id) {
            let entity = room.to_owned();
            e.insert(entity.clone());
            self.rooms.insert(0, entity);
        }

        // Emit the open room event deferred to avoid re-entrant reads
        cx.defer_in(window, move |_this, _window, cx| {
            cx.emit(ChatEvent::OpenRoom(id));
        });
    }

    /// Close a room.
    pub fn close_room(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.room_index.contains_key(&id) {
            self.room_index.remove(&id);
            self.rooms.retain(|r| r.read(cx).id != id);
            cx.defer_in(window, move |_this, _window, cx| {
                cx.emit(ChatEvent::CloseRoom(id));
            });
        }
    }

    /// Sort rooms by their created at. Only notifies if order changed.
    pub fn sort(&mut self, cx: &mut Context<Self>) {
        let before: Vec<_> = self.rooms.iter().map(|ev| ev.read(cx).id).collect();
        self.rooms.sort_by_key(|ev| Reverse(ev.read(cx).created_at));
        let after: Vec<_> = self.rooms.iter().map(|ev| ev.read(cx).id).collect();
        if before != after {
            cx.notify();
        }
    }

    /// Finding rooms based on a query.
    pub fn find(&self, query: &str, cx: &App) -> Vec<Entity<Room>> {
        if let Ok(public_key) = PublicKey::parse(query) {
            self.rooms
                .iter()
                .filter(|room| room.read(cx).members.contains(&public_key))
                .cloned()
                .collect()
        } else {
            self.rooms
                .iter()
                .filter(|room| {
                    self.matcher
                        .fuzzy_match(room.read(cx).display_name(cx).as_ref(), query)
                        .is_some()
                })
                .cloned()
                .collect()
        }
    }

    /// Reset the registry.
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.tasks.clear();
        self.room_load_task = None;
        self.room_reload = room_loader::ReloadState::default();
        self.history_error = None;
        self.history_task = None;
        self.decrypt_task = None;
        self.retry_task = None;
        if let Some(queue) = &self.outgoing {
            queue.stop();
        }
        self.outgoing_task = None;
        self.outgoing = None;
        if let Some(store) = self.moderation.take() { store.stop(); }
        self.moderation_task = None;
        self.moderation_error = None;
        if let Some(store) = self.archives.take() { store.stop(); }
        self.archive_task = None;
        self.archive_error = None;
        self.incoming = None;
        self.inbox = None;
        self.reads = None;
        self.contacts.clear();
        self.classification_ready = false;
        self.outgoing_reports.clear();
        self.outgoing_error = None;
        self.notification_listener = None;
        self.signal_consumer = None;
        self.queue = None;
        self.history_running = false;
        self.pending_history = None;
        self.last_history = None;
        self.history.clear();
        self.seen = Arc::default();
        self.event_map = Arc::default();
        let (tx, rx) = flume::bounded(256);
        self.signal_tx = tx;
        self.signal_rx = rx;
        self.rooms.clear();
        self.room_index.clear();
        self.trash.update(cx, |this, cx| {
            this.clear();
            cx.notify();
        });
        cx.notify();
    }

    /// Extend the registry with new rooms.
    fn extend_rooms(&mut self, rooms: HashSet<Room>, cx: &mut Context<Self>) {
        let mut room_map: HashMap<u64, usize> = self
            .rooms
            .iter()
            .enumerate()
            .map(|(idx, room)| (room.read(cx).id, idx))
            .collect();

        for new_room in rooms.into_iter() {
            // Check if we already have a room with this ID
            if let Some(&index) = room_map.get(&new_room.id) {
                self.rooms[index].update(cx, |this, cx| {
                    this.merge_loaded(new_room);
                    cx.notify();
                });
            } else {
                let new_room_id = new_room.id;
                let entity = cx.new(|_| new_room);
                self.room_index.insert(new_room_id, entity.clone());
                self.rooms.push(entity);

                let new_index = self.rooms.len() - 1;
                room_map.insert(new_room_id, new_index);
            }
        }
    }

    /// Serialize room scans, coalescing requests received during the current scan.
    pub fn get_rooms(&mut self, cx: &mut Context<Self>) {
        if NostrRegistry::global(cx).read(cx).current_user().is_none()
            || !self.room_reload.request() {
            return;
        }
        let mut task = self.get_rooms_task(cx);
        self.room_load_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let result = task.await;
                let next = this.update(cx, |this, cx| {
                    match result {
                        Ok(loaded) => this.apply_loaded_rooms(loaded, cx),
                        Err(error) => cx.emit(ChatEvent::Error(error.to_string())),
                    }
                    this.room_reload.finish().then(|| this.get_rooms_task(cx))
                })?;
                match next {
                    Some(next) => task = next,
                    None => break,
                }
            }
            Ok(())
        }));
    }

    fn apply_loaded_rooms(&mut self, loaded: room_loader::LoadedRooms, cx: &mut Context<Self>) {
        let room_loader::LoadedRooms { mut rooms, contacts } = loaded;
        self.contacts = contacts;
        self.classification_ready = true;
        if let Some(inbox) = &mut self.inbox {
            let established = rooms
                .iter()
                .filter(|room| room.kind == RoomKind::Ongoing)
                .map(|room| room.id)
                .chain(self.rooms.iter().filter_map(|room| {
                    let room = room.read(cx);
                    (room.kind == RoomKind::Ongoing
                        || room
                            .members
                            .iter()
                            .any(|key| self.contacts.contains(key)))
                    .then_some(room.id)
                }));
            if let Err(error) = inbox.remember(established) {
                cx.emit(ChatEvent::Error(format!(
                    "Could not save Inbox state: {error}"
                )));
            }
            rooms = rooms
                .into_iter()
                .map(|mut room| {
                    if inbox.contains(room.id) {
                        room.kind = RoomKind::Ongoing;
                    }
                    room
                })
                .collect();
        }
        // Contacts arriving after live messages also promote existing rooms.
        for room in &self.rooms {
            room.update(cx, |room, cx| {
                if room.members.iter().any(|key| self.contacts.contains(key)) {
                    room.set_ongoing(cx);
                }
            });
        }
        self.extend_rooms(rooms, cx);
        self.sort(cx);
        cx.notify();
    }

    /// Create a task to load rooms from the database
    fn get_rooms_task(&self, cx: &App) -> Task<Result<room_loader::LoadedRooms, Error>> {
        let nostr = NostrRegistry::global(cx);
        let client = nostr.read(cx).client();
        let owner = nostr.read(cx).current_user();
        let outgoing = self.outgoing.clone();
        let cache = self.incoming.clone();
        cx.background_spawn(async move {
            room_loader::load(client, owner.ok_or_else(|| anyhow!("No account"))?, cache, outgoing).await
        })
    }

    /// Parse a nostr event into a message and push it to the belonging room
    ///
    /// If the room doesn't exist, it will be created.
    /// Updates room ordering based on the most recent messages.
    pub fn new_message(&mut self, mut message: NewMessage, cx: &mut Context<Self>) {
        let nostr = NostrRegistry::global(cx);

        let Some(public_key) = nostr.read(cx).current_user() else {
            return;
        };

        let mut refresh_reactions = false;
        if let Some(cache) = &self.incoming {
            cache.note(&message.rumor);
            if message.rumor.kind == Kind::Reaction {
                let Some(room) = cache.reaction_room(&message.rumor) else {
                    return;
                };
                message.room = room;
                if !self.room_index.contains_key(&room) {
                    return;
                }
            } else if cache::is_chat(message.rumor.kind) {
                refresh_reactions = message.rumor.id.is_some_and(|id| cache.has_reactions(id));
            } else {
                return;
            }
        } else {
            return;
        }

        if message.rumor.pubkey == public_key {
            self.accept_room(message.room, cx);
        }

        match self.room_index.get(&message.room).cloned() {
            Some(room) => {
                room.update(cx, |this, cx| {
                    if this.kind == RoomKind::Request && message.rumor.pubkey == public_key {
                        this.set_ongoing(cx);
                    }
                    this.push_message(message, cx);
                    if refresh_reactions {
                        this.emit_refresh(cx);
                    }
                });
                self.sort(cx);
            }
            None => {
                // Push the new room to the front of the list
                self.add_room(message.rumor, cx);
            }
        }
        // Unread counts and timestamps can change without reordering rooms.
        cx.notify();
    }

    /// Trigger a refresh of the opened chat rooms by their IDs
    pub fn refresh_rooms(&mut self, ids: &[u64], cx: &mut Context<Self>) {
        for room in self.rooms.iter() {
            if ids.contains(&room.read(cx).id) {
                room.update(cx, |this, cx| {
                    this.emit_refresh(cx);
                });
            }
        }
    }
}

/// Unwraps a gift-wrapped event and processes its contents.
async fn extract_rumor(
    cache: &RumorCache,
    signer: &UniversalSigner,
    gift_wrap: &Event,
) -> Result<(UnsignedEvent, bool), Error> {
    gift_wrap.verify()?;
    if gift_wrap.kind != Kind::GiftWrap
        || !gift_wrap.tags.public_keys().any(|key| key == cache.owner)
    {
        return Err(anyhow!("Gift wrap is not addressed to this account"));
    }
    // Try to get cached rumor first
    if let Ok(rumor) = cache.get(gift_wrap.id).await {
        return Ok((rumor, true));
    }

    // Try to unwrap with the available signer
    let unwrapped = try_unwrap_with(signer, gift_wrap).await?;
    let mut rumor = unwrapped.rumor;

    // Verify rumor author matches the seal sender (as per mobile implementation)
    if rumor.pubkey != unwrapped.sender {
        return Err(anyhow!("Rumor author does not match seal sender"));
    }

    // A supplied ID must match the payload; only synthesize an absent ID.
    rumor.verify_id()?;
    rumor.ensure_id();

    let inserted = cache.put(gift_wrap.id, &rumor).await?;
    Ok((rumor, !inserted))
}

/// Attempts to unwrap a gift wrap event with a given signer.
async fn try_unwrap_with(
    signer: &UniversalSigner,
    gift_wrap: &Event,
) -> Result<UnwrappedGift, Error> {
    // Get the sealed event
    let seal = signer
        .nip44_decrypt_async(&gift_wrap.pubkey, &gift_wrap.content)
        .await?;

    // Verify the sealed event
    let seal: Event = Event::from_json(seal)?;
    seal.verify()?;
    if seal.kind != Kind::Seal || !seal.tags.is_empty() {
        return Err(anyhow!(
            "Invalid NIP-17 seal: expected kind 13 with no tags"
        ));
    }

    // Get the rumor event
    let rumor = signer
        .nip44_decrypt_async(&seal.pubkey, &seal.content)
        .await?;

    let rumor = UnsignedEvent::from_json(rumor)?;

    Ok(UnwrappedGift {
        sender: seal.pubkey,
        rumor,
    })
}

#[cfg(test)]
mod validation_tests {
    use super::*;

    // Build every layer explicitly so malformed inner events still have valid
    // outer encryption and signatures, as they could on a real relay.
    async fn wrap_seal(recipient: &Keys, seal: &Event) -> Event {
        let ephemeral = Keys::generate();
        let content = ephemeral
            .nip44_encrypt_async(&recipient.public_key(), &seal.as_json())
            .await
            .unwrap();
        EventBuilder::new(Kind::GiftWrap, content)
            .tag(Tag::public_key(recipient.public_key()))
            .finalize(&ephemeral)
            .unwrap()
    }

    async fn seal_rumor(
        sender: &Keys,
        recipient: &Keys,
        rumor: &UnsignedEvent,
        kind: Kind,
        tags: Vec<Tag>,
    ) -> Event {
        let content = sender
            .nip44_encrypt_async(&recipient.public_key(), &rumor.as_json())
            .await
            .unwrap();
        EventBuilder::new(kind, content)
            .tags(tags)
            .finalize(sender)
            .unwrap()
    }

    #[tokio::test]
    async fn rejects_malformed_inner_events_before_caching() {
        let sender = Keys::generate();
        let recipient = Keys::generate();
        let signer = UniversalSigner::new(recipient.clone());
        let client = Client::builder()
            .database(nostr_memory::MemoryDatabase::unbounded())
            .build();
        let cache = RumorCache::with_keys(client.clone(), recipient.public_key(), Keys::generate());
        let rumor = EventBuilder::new(Kind::PrivateDirectMessage, "hello")
            .tag(Tag::public_key(recipient.public_key()))
            .finalize_unsigned(sender.public_key());
        let mut wrong_id = rumor.clone();
        wrong_id.id = Some(EventId::from_byte_array([0; 32]));
        let mut wrong_author = rumor.clone();
        wrong_author.pubkey = Keys::generate().public_key();
        wrong_author.id = None;
        let mut invalid_signature =
            seal_rumor(&sender, &recipient, &rumor, Kind::Seal, vec![]).await;
        invalid_signature.sig = EventBuilder::new(Kind::Seal, "different content")
            .finalize(&sender)
            .unwrap()
            .sig;
        let cases = [
            (
                "wrong seal kind",
                seal_rumor(&sender, &recipient, &rumor, Kind::TextNote, vec![]).await,
            ),
            (
                "tagged seal",
                seal_rumor(
                    &sender,
                    &recipient,
                    &rumor,
                    Kind::Seal,
                    vec![Tag::public_key(recipient.public_key())],
                )
                .await,
            ),
            (
                "wrong rumor ID",
                seal_rumor(&sender, &recipient, &wrong_id, Kind::Seal, vec![]).await,
            ),
            (
                "wrong rumor author",
                seal_rumor(&sender, &recipient, &wrong_author, Kind::Seal, vec![]).await,
            ),
            ("invalid seal signature", invalid_signature),
        ];
        for (name, seal) in cases {
            let wrap = wrap_seal(&recipient, &seal).await;
            assert!(
                extract_rumor(&cache, &signer, &wrap).await.is_err(),
                "{name}"
            );
            assert!(cache.get(wrap.id).await.is_err(), "cached {name}");
        }
        client.shutdown().await;
    }

    #[tokio::test]
    async fn raw_history_rebuilds_legacy_cache_and_duplicate_wraps_are_identified() {
        let sender = Keys::generate();
        let recipient = Keys::generate();
        let signer = UniversalSigner::new(recipient.clone());
        let client = Client::builder()
            .database(nostr_memory::MemoryDatabase::unbounded())
            .build();
        let cache = RumorCache::with_keys(client.clone(), recipient.public_key(), Keys::generate());
        let mut rumor = EventBuilder::new(Kind::PrivateDirectMessage, "retained history")
            .tag(Tag::public_key(recipient.public_key()))
            .finalize_unsigned(sender.public_key());
        rumor.ensure_id();
        let seal = seal_rumor(&sender, &recipient, &rumor, Kind::Seal, vec![]).await;
        let first = wrap_seal(&recipient, &seal).await;
        let second = wrap_seal(&recipient, &seal).await;
        let legacy = EventBuilder::new(Kind::ApplicationSpecificData, rumor.as_json())
            .tags([Tag::identifier(first.id), Tag::custom("k", ["14"])])
            .finalize(&*LOCAL_KEYS)
            .unwrap();
        client.database().save_event(&legacy).await.unwrap();
        client.database().save_event(&first).await.unwrap();
        assert!(cache.get(first.id).await.is_err());
        let raw = client
            .database()
            .query(
                Filter::new()
                    .kind(Kind::GiftWrap)
                    .pubkey(recipient.public_key()),
            )
            .await
            .unwrap();
        assert_eq!(raw.len(), 1);
        assert_eq!(
            extract_rumor(&cache, &signer, raw.first().unwrap())
                .await
                .unwrap(),
            (rumor.clone(), false)
        );
        assert_eq!(
            extract_rumor(&cache, &signer, &second).await.unwrap(),
            (rumor.clone(), true)
        );
        assert_eq!(cache.all().await.unwrap(), vec![rumor]);
        // A matching cache alias does not bypass outer signature/recipient validation.
        let mut forged = first.clone();
        forged.content.push('x');
        assert!(extract_rumor(&cache, &signer, &forged).await.is_err());
        let other = RumorCache::with_keys(
            client.clone(),
            Keys::generate().public_key(),
            Keys::generate(),
        );
        assert!(extract_rumor(&other, &signer, &first).await.is_err());
        client.shutdown().await;
    }

    #[tokio::test]
    async fn valid_rumors_with_or_without_ids_are_cached_with_canonical_ids() {
        let sender = Keys::generate();
        let recipient = Keys::generate();
        let signer = UniversalSigner::new(recipient.clone());
        let client = Client::builder()
            .database(nostr_memory::MemoryDatabase::unbounded())
            .build();
        let cache = RumorCache::with_keys(client.clone(), recipient.public_key(), Keys::generate());
        for include_id in [false, true] {
            let mut rumor = EventBuilder::new(Kind::PrivateDirectMessage, "valid message")
                .tag(Tag::public_key(recipient.public_key()))
                .finalize_unsigned(sender.public_key());
            if include_id {
                rumor.ensure_id();
            } else {
                rumor.id = None;
            }
            let expected_id = rumor.compute_id();
            let seal = seal_rumor(&sender, &recipient, &rumor, Kind::Seal, vec![]).await;
            let wrap = wrap_seal(&recipient, &seal).await;
            let (result, _) = extract_rumor(&cache, &signer, &wrap).await.unwrap();
            assert_eq!(result.id, Some(expected_id));
            assert_eq!(cache.get(wrap.id).await.unwrap(), result);
        }
        client.shutdown().await;
    }
}
