use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, RwLock};

use anyhow::Error;
use gpui::{App, AppContext, Context, Entity, Global, Task, Window};
use instant::{Duration, Instant};
use nostr_sdk::prelude::*;
use smallvec::{SmallVec, smallvec};
use state::NostrRegistry;

mod person;

pub use person::*;

pub fn init(window: &mut Window, cx: &mut App) {
    PersonRegistry::set_global(cx.new(|cx| PersonRegistry::new(window, cx)), cx);
}

struct GlobalPersonRegistry(Entity<PersonRegistry>);

impl Global for GlobalPersonRegistry {}

#[derive(Debug, Clone)]
enum Dispatch {
    Person(Person),
    Relays(Event),
}

/// Person Registry
#[derive(Debug)]
pub struct PersonRegistry {
    /// Collection of all persons (user profiles)
    persons: HashMap<PublicKey, Entity<Person>>,

    /// Last metadata request for each public key
    seen: RwLock<HashMap<PublicKey, Instant>>,
    pending: Arc<RwLock<HashSet<PublicKey>>>,

    /// Sender for requesting metadata
    sender: flume::Sender<PublicKey>,

    /// Tasks for asynchronous operations
    tasks: SmallVec<[Task<()>; 4]>,
}

impl PersonRegistry {
    /// Retrieve the global person registry
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalPersonRegistry>().0.clone()
    }

    /// Set the global person registry instance
    fn set_global(state: Entity<Self>, cx: &mut App) {
        cx.set_global(GlobalPersonRegistry(state));
    }

    /// Create a new person registry instance
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let nostr = NostrRegistry::global(cx);
        let client = nostr.read(cx).client();

        // Channel for communication between nostr and gpui
        let (tx, rx) = flume::bounded::<Dispatch>(100);
        let (metadata_tx, metadata_rx) = flume::unbounded::<PublicKey>();

        let mut tasks = smallvec![];

        let pending = Arc::new(RwLock::new(HashSet::new()));
        let notifications_tx = tx.clone();
        let client2 = client.clone();
        tasks.push(cx.background_spawn(async move {
            Self::handle_notifications(&client2, &notifications_tx).await;
        }));

        // Disk lookup has its own worker: slow relays cannot hold cached avatars hostage.
        let (network_tx, network_rx) = flume::unbounded();
        let cache_client = client.clone();
        let cache_tx = tx.clone();
        tasks.push(cx.background_spawn(async move {
            dispatch_requests(&cache_client, metadata_rx, network_tx, &cache_tx).await;
        }));
        // Coalesce startup lookups and pace batches, not just concurrent requests.
        // A fast relay can exhaust its request budget with four serial workers.
        let executor = cx.background_executor().clone();
        let network_client = client.clone();
        let network_updates = tx.clone();
        let network_pending = pending.clone();
        tasks.push(cx.background_spawn(async move {
            while let Ok(first) = network_rx.recv_async().await {
                executor.timer(Duration::from_millis(200)).await;
                let authors = profile_batch(first, &network_rx);
                let rate_limited =
                    match fetch_profiles(&network_client, &authors, &network_updates).await {
                        Ok(rate_limited) => rate_limited,
                        Err(error) => {
                            log::warn!("Could not refresh visible profiles: {error}");
                            true // Back off on setup failures as well.
                        }
                    };
                // Retain pending keys during the cooldown so redraws cannot queue
                // duplicate work while a relay is refusing requests.
                executor.timer(profile_batch_delay(rate_limited)).await;
                let mut pending = network_pending.write().unwrap();
                for author in authors {
                    pending.remove(&author);
                }
            }
        }));

        tasks.push(cx.spawn(async move |this, cx| {
            let mut budget = common::UiWorkBudget::default();
            while let Ok(event) = rx.recv_async().await {
                this.update(cx, |this, cx| {
                    match event {
                        Dispatch::Person(person) => {
                            this.insert(person, cx);
                        }
                        Dispatch::Relays(event) => {
                            this.set_messaging_relays(&event, cx);
                        }
                    };
                })
                .ok();
                budget.checkpoint(cx.background_executor()).await;
            }
        }));

        // Load all user profiles from the database
        cx.defer_in(window, |this, _window, cx| {
            this.load(cx);
        });

        Self {
            persons: HashMap::new(),
            seen: RwLock::new(HashMap::new()),
            pending,
            sender: metadata_tx,
            tasks,
        }
    }

    /// Handle nostr notifications
    async fn handle_notifications(client: &Client, tx: &flume::Sender<Dispatch>) {
        let mut notifications = client.notifications();
        let mut processed: HashSet<EventId> = HashSet::new();

        while let Some(notification) = notifications.next().await {
            let ClientNotification::Message { message, .. } = notification else {
                // Skip if the notification is not a message
                continue;
            };

            if let RelayMessage::Event { event, .. } = *message {
                // Ignore history traffic before deduplication. Contact-list fanout
                // must not block delivery of profile events already on the stream.
                if !matches!(
                    event.kind,
                    Kind::Metadata | Kind::InboxRelays
                ) {
                    continue;
                }
                // Skip if the event has already been processed
                if !processed.insert(event.id) {
                    continue;
                }

                match event.kind {
                    Kind::Metadata => {
                        let Ok(person) = Person::from_metadata_event(&event) else {
                            continue;
                        };
                        if tx.send_async(Dispatch::Person(person)).await.is_err() {
                            log::warn!("PersonRegistry channel closed, dropping metadata event");
                        }
                    }
                    Kind::InboxRelays => {
                        tx.send_async(Dispatch::Relays(event.into_owned()))
                            .await
                            .ok();
                    }
                    _ => {}
                }
            }
        }
    }

    /// Load all user profiles from the database
    fn load(&mut self, cx: &mut Context<Self>) {
        let nostr = NostrRegistry::global(cx);
        let client = nostr.read(cx).client();

        let task: Task<Result<Vec<Person>, Error>> = cx.background_spawn(async move {
            let filter = Filter::new().kind(Kind::Metadata).limit(200);
            let events = client.database().query(filter).await?;
            let persons = events
                .into_iter()
                .filter_map(|event| Person::from_metadata_event(&event).ok())
                .collect();

            Ok(persons)
        });

        self.tasks.push(cx.spawn(async move |this, cx| {
            if let Ok(persons) = task.await {
                this.update(cx, |this, cx| {
                    this.bulk_insert(persons, cx);
                })
                .ok();
            }
        }));
    }

    /// Set messaging relays for a person
    fn set_messaging_relays(&mut self, event: &Event, cx: &mut App) {
        let urls: Vec<RelayUrl> = nip17::extract_relay_list(event).collect();

        if let Some(person) = self.persons.get(&event.pubkey) {
            person.update(cx, |person, cx| {
                person.set_messaging_relays(urls);
                cx.notify();
            });
        } else {
            let person = Person::new(event.pubkey, Metadata::default()).with_messaging_relays(urls);
            self.insert(person, cx);
        }
    }

    /// Insert batch of persons
    fn bulk_insert(&mut self, persons: Vec<Person>, cx: &mut Context<Self>) {
        for person in persons.into_iter() {
            self.insert(person, cx);
        }
        cx.notify();
    }

    /// Insert or update a person
    pub fn insert(&mut self, person: Person, cx: &mut App) {
        let public_key = person.public_key();

        let changed = match self.persons.get(&public_key) {
            Some(this) => this.update(cx, |this, cx| {
                let changed = this.merge_metadata(&person);
                if changed {
                    cx.notify();
                }
                changed
            }),
            None => {
                self.persons.insert(public_key, cx.new(|_| person));
                true
            }
        };
        if changed {
            cx.refresh_windows();
        }
    }

    /// Refresh a visible profile immediately, without the background batch delay.
    /// Deliver cached and streamed metadata separately so the UI can paint early.
    pub fn refresh(
        &mut self,
        public_key: PublicKey,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), Error>> {
        let client = NostrRegistry::global(cx).read(cx).client();
        self.seen
            .write()
            .unwrap()
            .insert(public_key, Instant::now());
        cx.spawn(async move |this, cx| {
            let filter = Filter::new()
                .kind(Kind::Metadata)
                .author(public_key)
                .limit(1);
            if let Some(person) = cached_profile(&client, public_key).await? {
                this.update(cx, |this, cx| this.insert(person, cx))?;
            }
            // Filter targets retain automatic outbox discovery.
            let mut stream = client
                .stream_events(filter)
                .timeout(Duration::from_secs(10))
                .await?;
            while let Some((_, event)) = stream.next().await {
                if let Ok(event) = event
                    && let Ok(person) = Person::from_metadata_event(&event)
                {
                    this.update(cx, |this, cx| this.insert(person, cx))?;
                }
            }
            Ok(())
        })
    }

    /// Snapshot loaded profiles without scheduling database or relay requests.
    pub fn loaded(&self, cx: &App) -> Vec<Person> {
        self.persons
            .values()
            .map(|person| person.read(cx).clone())
            .collect()
    }

    /// Get single person by public key
    pub fn get(&self, public_key: &PublicKey, cx: &App) -> Person {
        // Render cached metadata immediately, but still refresh it on first use
        // this session and periodically afterward. A cached profile without a
        // picture must not prevent us from discovering a newer one on its outbox.
        let has_metadata = self
            .persons
            .get(public_key)
            .is_some_and(|person| person.read(cx).has_metadata());
        let refresh_after = Duration::from_secs(if has_metadata { 60 * 60 } else { 30 });

        let public_key = *public_key;

        let should_request = {
            let mut seen = self.seen.write().unwrap();
            if seen
                .get(&public_key)
                .is_some_and(|last| last.elapsed() < refresh_after)
            {
                false
            } else {
                seen.insert(public_key, Instant::now());
                true
            }
        };
        if should_request && self.pending.write().unwrap().insert(public_key) {
            let sender = self.sender.clone();

            // Spawn background task to request metadata
            cx.background_spawn(async move {
                if let Err(e) = sender.send_async(public_key).await {
                    log::warn!("Failed to send public key for metadata request: {e}");
                }
            })
            .detach();
        }

        // Return a temporary profile with default metadata
        self.persons
            .get(&public_key)
            .map(|person| person.read(cx).clone())
            .unwrap_or_else(|| Person::new(public_key, Metadata::default()))
    }
}

/// Query a visible author directly, including profiles outside the startup prewarm.
async fn cached_profile(client: &Client, public_key: PublicKey) -> Result<Option<Person>, Error> {
    let events = client
        .database()
        .query(
            Filter::new()
                .kind(Kind::Metadata)
                .author(public_key)
                .limit(1),
        )
        .await?;
    Ok(events
        .iter()
        .find_map(|event| Person::from_metadata_event(event).ok()))
}

const PROFILE_BATCH_SIZE: usize = 64;

fn profile_batch(first: PublicKey, requests: &flume::Receiver<PublicKey>) -> HashSet<PublicKey> {
    let mut authors = HashSet::from([first]);
    while authors.len() < PROFILE_BATCH_SIZE {
        let Ok(author) = requests.try_recv() else {
            break;
        };
        authors.insert(author);
    }
    authors
}

fn profile_batch_delay(rate_limited: bool) -> Duration {
    // Leave room for discovery, messages and account-list requests on relays
    // with small per-IP budgets. This only paces background profile refreshes.
    Duration::from_secs(if rate_limited { 120 } else { 60 })
}

async fn fetch_profiles(
    client: &Client,
    authors: &HashSet<PublicKey>,
    tx: &flume::Sender<Dispatch>,
) -> Result<bool, Error> {
    fetch_profiles_with_allowed_relays(client, authors, tx, GossipAllowedRelays::default()).await
}

async fn fetch_profiles_with_allowed_relays(
    client: &Client,
    authors: &HashSet<PublicKey>,
    tx: &flume::Sender<Dispatch>,
    allowed: GossipAllowedRelays,
) -> Result<bool, Error> {
    if authors.is_empty() {
        return Ok(false);
    }
    let read_relays = client
        .relays()
        .with_capabilities(RelayCapabilities::READ)
        .await;
    let discovery_relays = client
        .relays()
        .with_capabilities(RelayCapabilities::READ | RelayCapabilities::DISCOVERY)
        .await;
    let mut limited_relays = HashSet::new();
    // Use one grouped discovery filter. Automatic gossip's negentropy fallback
    // can generate one filter per cached relay list even for a batched query.
    let discovery = Filter::new()
        .kind(Kind::RelayList)
        .authors(authors.iter().copied());
    let targets: Vec<_> = discovery_relays
        .into_keys()
        .map(|relay| (relay, vec![discovery.clone()]))
        .collect();
    if !targets.is_empty() {
        collect_profile_events(client, targets, tx, &mut limited_relays).await?;
    }

    // Keep fallback on configured read relays, including authors with no outbox.
    let mut targets: HashMap<RelayUrl, HashSet<PublicKey>> = read_relays
        .into_keys()
        .filter(|relay| !limited_relays.contains(relay))
        .map(|relay| (relay, authors.clone()))
        .collect();
    // The database retains valid cached relay lists when discovery is offline.
    let lists = client.database().query(discovery).await?;
    let mut latest = HashMap::new();
    for event in lists {
        if event.verify().is_ok() {
            let entry = latest.entry(event.pubkey).or_insert(event.clone());
            if event.created_at > entry.created_at {
                *entry = event;
            }
        }
    }
    for (author, event) in latest {
        let relays: BTreeSet<_> = nip65::extract_relay_list(&event)
            .filter(|(relay, usage)| {
                *usage != Some(RelayMetadata::Read)
                    && allowed.is_allowed(relay)
                    && !limited_relays.contains(relay)
            })
            .map(|(relay, _)| relay.clone())
            .collect();
        // Bound the number of outboxes queried per author.
        for relay in relays.into_iter().take(3) {
            if let Err(error) = client
                .add_relay(&relay)
                .capabilities(RelayCapabilities::GOSSIP)
                .and_connect()
                .await
            {
                log::warn!("Could not connect profile relay: {error}");
                continue;
            }
            targets.entry(relay).or_default().insert(author);
        }
    }
    let targets = targets
        .into_iter()
        .map(|(relay, authors)| {
            // Metadata is replaceable; a shared limit(1) would omit other authors.
            (
                relay,
                vec![Filter::new().kind(Kind::Metadata).authors(authors)],
            )
        })
        .collect();
    collect_profile_events(client, targets, tx, &mut limited_relays).await?;
    Ok(!limited_relays.is_empty())
}

async fn collect_profile_events(
    client: &Client,
    targets: Vec<(RelayUrl, Vec<Filter>)>,
    tx: &flume::Sender<Dispatch>,
    limited_relays: &mut HashSet<RelayUrl>,
) -> Result<(), Error> {
    if targets.is_empty() {
        return Ok(());
    }
    let mut stream = client
        .stream_events(ReqTarget::manual(targets))
        .timeout(Duration::from_secs(10))
        .await?;
    while let Some((relay, event)) = stream.next().await {
        match event {
            Ok(event) => {
                if event.kind == Kind::RelayList && event.verify().is_ok() {
                    client.database().save_event(&event).await?;
                }
                if let Ok(person) = Person::from_metadata_event(&event) {
                    tx.send_async(Dispatch::Person(person)).await?;
                }
            }
            Err(error) => {
                if error.to_string().contains("rate-limited:") {
                    limited_relays.insert(relay);
                }
            }
        }
    }
    Ok(())
}

async fn dispatch_requests(
    cache_client: &Client,
    metadata_rx: flume::Receiver<PublicKey>,
    network_tx: flume::Sender<PublicKey>,
    cache_tx: &flume::Sender<Dispatch>,
) {
    while let Ok(public_key) = metadata_rx.recv_async().await {
        match cached_profile(&cache_client, public_key).await {
            Ok(Some(person)) => {
                let _ = cache_tx.send_async(Dispatch::Person(person)).await;
            }
            Ok(None) => {}
            Err(error) => log::warn!("Could not read cached profile: {error}"),
        }
        if network_tx.send(public_key).is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod loading_tests {
    use super::*;
    #[test]
    fn startup_batches_are_bounded_and_deduplicate_profiles() {
        let (tx, rx) = flume::unbounded();
        let first = Keys::generate().public_key();
        tx.send(first).unwrap();
        for _ in 0..PROFILE_BATCH_SIZE {
            tx.send(Keys::generate().public_key()).unwrap();
        }
        let batch = profile_batch(first, &rx);
        assert_eq!(batch.len(), PROFILE_BATCH_SIZE);
        assert!(batch.contains(&first));
        assert_eq!(rx.len(), 1);
    }

    #[tokio::test]
    async fn startup_profiles_fit_in_two_relay_queries() {
        use nostr_gossip_memory::prelude::NostrGossipMemory;
        use nostr_sdk::local_relay::LocalRelay;
        tokio::time::timeout(Duration::from_secs(10), async {
            let relay = LocalRelay::builder().queries_per_minute(2).build();
            relay.run().await.unwrap();
            let client = Client::builder()
                .database(nostr_memory::MemoryDatabase::unbounded())
                .gossip(NostrGossipMemory::unbounded())
                .gossip_config(GossipConfig::default().no_background_refresh())
                .build();
            let mut authors = HashSet::new();
            for _ in 0..PROFILE_BATCH_SIZE {
                let keys = Keys::generate();
                authors.insert(keys.public_key());
                relay.add_event(profile(&keys, 1)).await.unwrap();
                // Simulate a returning user with cached discovery data. The
                // SDK's automatic fallback must not refresh these one by one.
                client
                    .database()
                    .save_event(
                        &EventBuilder::new(Kind::RelayList, "")
                            .finalize(&keys)
                            .unwrap(),
                    )
                    .await
                    .unwrap();
            }
            client
                .add_relay(relay.url().await)
                .and_connect()
                .await
                .unwrap();
            let (tx, rx) = flume::unbounded();
            assert!(!fetch_profiles(&client, &authors, &tx).await.unwrap());
            let found: HashSet<_> = rx
                .try_iter()
                .filter_map(|item| match item {
                    Dispatch::Person(person) => Some(person.public_key()),
                    _ => None,
                })
                .collect();
            assert_eq!(found, authors);
            // Both query tokens are now spent. A subsequent batch must signal
            // cooldown, not silently discard the relay's rejection.
            assert!(fetch_profiles(&client, &authors, &tx).await.unwrap());
            client.shutdown().await;
            relay.shutdown();
        })
        .await
        .expect("a batch should not require a query per author");
    }

    #[tokio::test]
    async fn profile_batch_keeps_outbox_discovery_and_unknown_author_fallback() {
        use nostr_gossip_memory::prelude::*;
        use nostr_sdk::local_relay::MockRelay;
        tokio::time::timeout(Duration::from_secs(20), async {
            let bootstrap = MockRelay::run().await.unwrap();
            let outbox = MockRelay::run().await.unwrap();
            let known = Keys::generate();
            let unknown = Keys::generate();
            outbox.add_event(profile(&known, 1)).await.unwrap();
            bootstrap.add_event(profile(&unknown, 1)).await.unwrap();
            bootstrap
                .add_event(
                    EventBuilder::new(Kind::RelayList, "")
                        .tag(Tag::custom(
                            "r",
                            [outbox.url().await.to_string(), "write".into()],
                        ))
                        .finalize(&known)
                        .unwrap(),
                )
                .await
                .unwrap();
            let client = Client::builder()
                .database(nostr_memory::MemoryDatabase::unbounded())
                .gossip(NostrGossipMemory::unbounded())
                .gossip_config(
                    GossipConfig::default()
                        .allowed(GossipAllowedRelays {
                            local: true,
                            without_tls: true,
                            ..Default::default()
                        })
                        .sync_initial_timeout(Duration::from_millis(500))
                        .sync_idle_timeout(Duration::from_secs(1))
                        .fetch_timeout(Duration::from_secs(2))
                        .no_background_refresh(),
                )
                .build();
            client
                .add_relay(bootstrap.url().await)
                .and_connect()
                .await
                .unwrap();
            let (tx, rx) = flume::unbounded();
            let authors = HashSet::from([known.public_key(), unknown.public_key()]);
            assert!(
                !fetch_profiles_with_allowed_relays(
                    &client,
                    &authors,
                    &tx,
                    GossipAllowedRelays {
                        local: true,
                        without_tls: true,
                        ..Default::default()
                    }
                )
                .await
                .unwrap()
            );
            let found: HashSet<_> = rx
                .try_iter()
                .filter_map(|item| match item {
                    Dispatch::Person(person) => Some(person.public_key()),
                    _ => None,
                })
                .collect();
            assert_eq!(found, authors);
            client.shutdown().await;
        })
        .await
        .expect("batched profiles should arrive from both relays");
    }

    fn profile(keys: &Keys, time: u64) -> Event {
        EventBuilder::new(
            Kind::Metadata,
            r#"{"name":"Visible person","picture":"https://example.com/avatar.png"}"#,
        )
        .custom_created_at(Timestamp::from(time))
        .finalize(keys)
        .unwrap()
    }
    #[tokio::test]
    async fn visible_cache_lookup_finds_profiles_outside_startup_prewarm() {
        let client = Client::builder()
            .database(nostr_memory::MemoryDatabase::unbounded())
            .build();
        let oldest = Keys::generate();
        client
            .database()
            .save_event(&profile(&oldest, 1))
            .await
            .unwrap();
        for time in 2..=201 {
            client
                .database()
                .save_event(&profile(&Keys::generate(), time))
                .await
                .unwrap();
        }
        let prewarm = client
            .database()
            .query(Filter::new().kind(Kind::Metadata).limit(200))
            .await
            .unwrap();
        assert!(
            !prewarm
                .iter()
                .any(|event| event.pubkey == oldest.public_key())
        );
        let cached = cached_profile(&client, oldest.public_key())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cached.name().as_ref(), "Visible person");
        assert_eq!(cached.avatar().as_ref(), "https://example.com/avatar.png");
        client.shutdown().await;
    }

    #[tokio::test]
    async fn cached_profiles_do_not_wait_for_queued_network_lookups() {
        tokio::time::timeout(Duration::from_secs(2), async {
            let client = Client::builder()
                .database(nostr_memory::MemoryDatabase::unbounded())
                .build();
            let first = Keys::generate();
            let second = Keys::generate();
            for keys in [&first, &second] {
                client
                    .database()
                    .save_event(&profile(keys, 1))
                    .await
                    .unwrap();
            }
            let (requests, rx) = flume::unbounded();
            let (network, network_rx) = flume::unbounded();
            let (updates, update_rx) = flume::unbounded();
            let worker_client = client.clone();
            let worker = tokio::spawn(async move {
                dispatch_requests(&worker_client, rx, network, &updates).await
            });
            for keys in [&first, &second] {
                requests.send(keys.public_key()).unwrap();
                let Dispatch::Person(person) = update_rx.recv_async().await.unwrap() else {
                    panic!("Expected profile");
                };
                assert_eq!(person.public_key(), keys.public_key());
            }
            // Neither network request has been serviced, but both cached profiles arrived.
            assert_eq!(network_rx.len(), 2);
            drop(requests);
            worker.await.unwrap();
            client.shutdown().await;
        })
        .await
        .unwrap();
    }
}
