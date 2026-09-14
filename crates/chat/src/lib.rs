use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, hash_map};
#[cfg(test)]
use std::sync::LazyLock;
use std::sync::{Arc, RwLock};

use anyhow::{Error, anyhow};
use common::EventExt;
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
use cache::RumorCache;
mod decryption;
mod history;
mod outgoing;
#[cfg(test)]
mod test_signer;
pub use history::RelayHistory;
use outgoing::{OutgoingMessage, OutgoingQueue};

mod message;
mod room;

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
    Decrypted(EventId, Result<NewMessage, FailedMessage>),
    History(RelayUrl, RelayHistory),
    Outgoing(OutgoingMessage),
    OutgoingError(String),
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
    history_error: Option<String>,
    last_history: Option<Instant>,
    queue: Option<DecryptQueue>,
    history_task: Option<Task<Result<(), Error>>>,
    decrypt_task: Option<Task<Result<(), Error>>>,
    retry_task: Option<Task<Result<(), Error>>>,
    outgoing_task: Option<Task<Result<(), Error>>>,
    outgoing: Option<OutgoingQueue>,
    incoming: Option<RumorCache>,
    outgoing_reports: HashMap<EventId, Vec<SendReport>>,

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
        let (tx, rx) = flume::unbounded::<Signal>();
        let mut subscriptions = smallvec![];

        subscriptions.push(
            // Subscribe to the signer event
            cx.subscribe(&nostr, |this, _nostr, event, cx| {
                if event.signer_changed() {
                    this.reset(cx);
                    this.handle_notifications(cx);
                    this.get_metadata(cx);
                    this.get_rooms(cx);
                } else if matches!(event, StateEvent::NoSigner) {
                    this.reset(cx);
                };
            }),
        );

        let device = device::DeviceRegistry::global(cx);
        subscriptions.push(cx.subscribe(&device, |this, device, _, cx| {
            if let Some(queue) = &this.outgoing {
                queue.set_encryption_signer(device.read(cx).signer(cx));
            }
        }));

        // Run at the end of the current cycle
        cx.defer_in(window, |this, _window, cx| {
            this.get_rooms(cx);
        });

        Self {
            rooms: vec![],
            room_index: HashMap::new(),
            trash: cx.new(|_| BTreeSet::default()),
            seen: Arc::new(RwLock::new(HashMap::default())),
            event_map: Arc::new(RwLock::new(HashMap::default())),
            history: BTreeMap::new(),
            history_running: false,
            history_error: None,
            last_history: None,
            queue: None,
            history_task: None,
            decrypt_task: None,
            retry_task: None,
            outgoing_task: None,
            outgoing: None,
            incoming: None,
            outgoing_reports: HashMap::new(),
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
            while let Ok(signal) = rx.recv_async().await {
                this.update(cx, |this, cx| {
                    match signal {
                        Signal::InboxReady => this.get_messages(cx),
                        Signal::Outgoing(message) => {
                            this.outgoing_reports
                                .insert(message.id(), message.reports());
                            let mut incoming = NewMessage::new(message.id(), message.rumor);
                            incoming.historical = true;
                            this.new_message(incoming, cx);
                        }
                        Signal::OutgoingError(error) => cx.emit(ChatEvent::Error(error)),
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
        let encryption = device::DeviceRegistry::global(cx).read(cx).signer(cx);
        let (queue, wake) = OutgoingQueue::new(client, owner, encryption);
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

    pub fn outgoing_reports(&self, id: &EventId) -> Option<Vec<SendReport>> {
        self.outgoing_reports.get(id).cloned()
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
        self.start_history(false, cx);
    }

    /// Resume history automatically on opening/scanning a chat, with a cooldown.
    pub fn ensure_history(&mut self, cx: &mut Context<Self>) {
        if self
            .last_history
            .is_none_or(|last| last.elapsed() >= Duration::from_secs(60))
        {
            self.start_history(false, cx);
        }
    }

    /// Explicitly retry incomplete relays and check for additional older history.
    pub fn load_older_history(&mut self, cx: &mut Context<Self>) {
        self.start_history(true, cx);
    }

    fn start_history(&mut self, force: bool, cx: &mut Context<Self>) {
        if self.history_running {
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
                queue.enqueue(event, false).await?;
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
            let mut scans = futures::stream::iter(relays)
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
        let status = if self.history_running {
            "Loading history"
        } else if pending > 0 {
            "Decrypting messages"
        } else if errors > 0 {
            "History incomplete"
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
        self.history_running || self.pending_messages() > 0
    }

    /// Get a weak reference to a room by its ID
    pub fn room(&self, id: &u64, _cx: &App) -> Option<WeakEntity<Room>> {
        self.room_index.get(id).map(|room| room.downgrade())
    }

    /// Get all rooms based on the filter.
    pub fn rooms(&self, filter: &RoomKind, cx: &App) -> Vec<Entity<Room>> {
        self.rooms
            .iter()
            .filter(|room| &room.read(cx).kind == filter)
            .cloned()
            .collect()
    }

    /// Count the number of rooms based on the filter.
    pub fn count(&self, filter: &RoomKind, cx: &App) -> usize {
        self.rooms
            .iter()
            .filter(|room| &room.read(cx).kind == filter)
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

    /// Add a new room to the start of list.
    pub fn add_room<I>(&mut self, room: I, cx: &mut Context<Self>)
    where
        I: Into<Room>,
    {
        let nostr = NostrRegistry::global(cx);
        let Some(public_key) = nostr.read(cx).current_user() else {
            return;
        };

        let room: Room = room.into().organize(&public_key);
        let room_id = room.id;
        let entity = cx.new(|_| room);

        self.room_index.insert(room_id, entity.clone());
        self.rooms.insert(0, entity);

        cx.emit(ChatEvent::Ping);
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
        self.history_error = None;
        self.history_task = None;
        self.decrypt_task = None;
        self.retry_task = None;
        if let Some(queue) = &self.outgoing {
            queue.stop();
        }
        self.outgoing_task = None;
        self.outgoing = None;
        self.incoming = None;
        self.outgoing_reports.clear();
        self.notification_listener = None;
        self.signal_consumer = None;
        self.queue = None;
        self.history_running = false;
        self.last_history = None;
        self.history.clear();
        self.seen = Arc::default();
        self.event_map = Arc::default();
        let (tx, rx) = flume::unbounded();
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
                    if new_room.created_at > this.created_at {
                        *this = new_room;
                        cx.notify();
                    }
                });
            } else {
                let new_room_id = new_room.id;
                let entity = cx.new(|_| new_room);
                self.room_index.insert(new_room_id, entity.clone());
                self.rooms.push(entity);

                let new_index = self.rooms.len();
                room_map.insert(new_room_id, new_index);
            }
        }
    }

    /// Load all rooms from the database.
    pub fn get_rooms(&mut self, cx: &mut Context<Self>) {
        let task = self.get_rooms_task(cx);

        self.tasks.push(cx.spawn(async move |this, cx| {
            match task.await {
                Ok(rooms) => {
                    this.update(cx, |this, cx| {
                        this.extend_rooms(rooms, cx);
                        this.sort(cx);
                    })?;
                }
                Err(e) => {
                    this.update(cx, |_, cx| {
                        cx.emit(ChatEvent::Error(e.to_string()));
                    })?;
                }
            };

            Ok(())
        }));
    }

    /// Create a task to load rooms from the database
    fn get_rooms_task(&self, cx: &App) -> Task<Result<HashSet<Room>, Error>> {
        let nostr = NostrRegistry::global(cx);
        let client = nostr.read(cx).client();
        let signer = nostr.read(cx).signer();

        let cache = self.incoming.clone();
        cx.background_spawn(async move {
            let public_key = signer.get_public_key_async().await?;

            // Query the latest contact list (previously `NostrDatabaseExt::contacts_public_keys`)
            let filter = Filter::new()
                .author(public_key)
                .kind(Kind::ContactList)
                .limit(1);

            let contacts: HashSet<PublicKey> = client
                .database()
                .query(filter)
                .await
                .unwrap_or_default()
                .into_iter()
                .next()
                .map(|event| event.tags.public_keys().collect())
                .unwrap_or_default();

            let messages = match cache {
                Some(cache) => cache.all().await?,
                None => vec![],
            };
            let mut grouped: HashMap<u64, Vec<UnsignedEvent>> = HashMap::new();
            for rumor in messages {
                if cache::is_chat(rumor.kind) {
                    grouped.entry(rumor.uniq_id()).or_default().push(rumor);
                }
            }

            let mut rooms = HashSet::with_capacity(grouped.len());

            for (_id, messages) in grouped.into_iter() {
                let latest = messages.iter().max_by_key(|m| m.created_at).unwrap();
                let room = Room::from(latest).organize(&public_key);

                let user_sent = messages.iter().any(|m| m.pubkey == public_key);
                let is_contact = room.members.iter().any(|k| contacts.contains(k));

                let room = if user_sent || is_contact {
                    room.kind(RoomKind::Ongoing)
                } else {
                    room
                };

                rooms.insert(room);
            }

            Ok(rooms)
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
