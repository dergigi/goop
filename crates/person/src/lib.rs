use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

use anyhow::Error;
use common::EventExt;
use gpui::{App, AppContext, Context, Entity, Global, Task, Window};
use instant::{Duration, Instant};
use nostr_sdk::prelude::*;
use smallvec::{SmallVec, smallvec};
use state::{Announcement, NostrRegistry, TIMEOUT, subscribe_profiles};

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

    /// Set of public keys that have been seen
    seen: RwLock<HashMap<PublicKey, Instant>>,

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

        let client2 = client.clone();
        tasks.push(cx.background_spawn(async move {
            Self::handle_notifications(&client2, &tx).await;
        }));

        let client3 = client.clone();
        tasks.push(cx.background_spawn(async move {
            Self::handle_requests(&client3, &metadata_rx).await;
        }));

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
                    Kind::ContactList => {
                        let public_keys = event.extract_public_keys();
                        if let Err(e) = subscribe_profiles(client, public_keys).await {
                            log::warn!("Failed to get metadata for contact list: {e}");
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

    /// Handle request for metadata
    async fn handle_requests(client: &Client, rx: &flume::Receiver<PublicKey>) {
        let mut batch: HashSet<PublicKey> = HashSet::new();

        loop {
            match flume::Selector::new()
                .recv(rx, |result| result.ok())
                .wait_timeout(Duration::from_secs(TIMEOUT))
            {
                Ok(Some(public_key)) => {
                    batch.insert(public_key);
                    // Process the batch if it's full
                    if batch.len() >= 20
                        && let Err(e) = subscribe_profiles(client, std::mem::take(&mut batch)).await
                    {
                        log::warn!("Failed to get metadata batch: {e}");
                    }
                }
                _ => {
                    if !batch.is_empty()
                        && let Err(e) = subscribe_profiles(client, std::mem::take(&mut batch)).await
                    {
                        log::warn!("Failed to get metadata batch: {e}");
                    }
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

        match self.persons.get(&public_key) {
            Some(this) => {
                this.update(cx, |this, cx| {
                    this.merge_metadata(&person);
                    cx.notify();
                });
            }
            None => {
                self.persons.insert(public_key, cx.new(|_| person));
            }
        }
        cx.refresh_windows();
    }

    /// Get single person by public key
    pub fn get(&self, public_key: &PublicKey, cx: &App) -> Person {
        if let Some(person) = self.persons.get(public_key)
            && person.read(cx).has_metadata()
        {
            return person.read(cx).clone();
        }

        let public_key = *public_key;

        let should_request = {
            let mut seen = self.seen.write().unwrap();
            if seen
                .get(&public_key)
                .is_some_and(|last| last.elapsed() < Duration::from_secs(30))
            {
                false
            } else {
                seen.insert(public_key, Instant::now());
                true
            }
        };
        if should_request {
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
