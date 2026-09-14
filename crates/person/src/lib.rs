use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use anyhow::Error;
use gpui::{App, AppContext, Context, Entity, Global, Task, Window};
use instant::{Duration, Instant};
use nostr_sdk::prelude::*;
use smallvec::{SmallVec, smallvec};
use state::{Announcement, NostrRegistry};

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
    Announcement(Event),
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
        // Reserve bounded parallelism for visible people; no idle batching timer.
        for _ in 0..4 {
            let client = client.clone();
            let requests = network_rx.clone();
            let tx = tx.clone();
            let pending = pending.clone();
            tasks.push(cx.background_spawn(async move {
                while let Ok(public_key) = requests.recv_async().await {
                    if let Err(error) = fetch_profile(&client, public_key, &tx).await {
                        log::warn!("Could not refresh visible profile: {error}");
                    }
                    pending.write().unwrap().remove(&public_key);
                }
            }));
        }

        tasks.push(cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv_async().await {
                this.update(cx, |this, cx| {
                    match event {
                        Dispatch::Person(person) => {
                            this.insert(person, cx);
                        }
                        Dispatch::Announcement(event) => {
                            this.set_announcement(&event, cx);
                        }
                        Dispatch::Relays(event) => {
                            this.set_messaging_relays(&event, cx);
                        }
                    };
                })
                .ok();
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
                    Kind::Metadata | Kind::InboxRelays | Kind::Custom(10044)
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
                    Kind::Custom(10044) => {
                        tx.send_async(Dispatch::Announcement(event.into_owned()))
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

    /// Set profile encryption keys announcement
    fn set_announcement(&mut self, event: &Event, cx: &mut App) {
        let announcement = Announcement::from(event);

        if let Some(person) = self.persons.get(&event.pubkey) {
            person.update(cx, |person, cx| {
                person.set_announcement(announcement);
                cx.notify();
            });
        } else {
            let person =
                Person::new(event.pubkey, Metadata::default()).with_announcement(announcement);
            self.insert(person, cx);
        }
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

async fn fetch_profile(
    client: &Client,
    public_key: PublicKey,
    tx: &flume::Sender<Dispatch>,
) -> Result<(), Error> {
    let filter = Filter::new()
        .kind(Kind::Metadata)
        .author(public_key)
        .limit(1);
    // Filter-based targeting preserves the SDK's automatic outbox discovery.
    let mut stream = client
        .stream_events(filter)
        .timeout(Duration::from_secs(10))
        .await?;
    while let Some((_, event)) = stream.next().await {
        if let Ok(event) = event
            && let Ok(person) = Person::from_metadata_event(&event)
        {
            tx.send_async(Dispatch::Person(person)).await?;
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
