//! Account-scoped, local-only outgoing intents and prepared gift wraps.
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use futures::{FutureExt, lock::Mutex};
use nostr_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use settings::SignerKind;
use state::{Announcement, UniversalSigner};

use super::{SendReport, Signal, set_rumor};

const STORE_TAG: &str = "goop-outgoing-v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Destination {
    pub receiver: PublicKey,
    pub announcement: Option<PublicKey>,
    pub self_copy: bool,
    pub wrap: Option<Event>,
    pub accepted: BTreeSet<RelayUrl>,
    pub failed: BTreeMap<RelayUrl, String>,
    pub error: Option<String>,
}

impl Destination {
    pub fn new(receiver: PublicKey, announcement: Option<PublicKey>, self_copy: bool) -> Self {
        Self {
            receiver,
            announcement,
            self_copy,
            wrap: None,
            accepted: BTreeSet::new(),
            failed: BTreeMap::new(),
            error: None,
        }
    }

    fn complete(&self) -> bool {
        !self.accepted.is_empty() && self.failed.is_empty() && self.error.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct OutgoingMessage {
    pub owner: PublicKey,
    pub rumor: UnsignedEvent,
    pub signer_kind: SignerKind,
    pub destinations: Vec<Destination>,
    revision: u64,
}

impl OutgoingMessage {
    pub fn new(
        owner: PublicKey,
        rumor: UnsignedEvent,
        signer_kind: SignerKind,
        destinations: Vec<Destination>,
    ) -> Result<Self> {
        if rumor.pubkey != owner || destinations.is_empty() {
            bail!("Invalid outgoing message owner or recipients");
        }
        rumor.verify_id()?;
        let mut rumor = rumor;
        rumor.ensure_id();
        Ok(Self {
            owner,
            rumor,
            signer_kind,
            destinations,
            revision: 0,
        })
    }

    pub fn id(&self) -> EventId {
        self.rumor.id.expect("outgoing rumor has an ID")
    }
    fn complete(&self) -> bool {
        self.destinations.iter().all(Destination::complete)
    }

    pub fn reports(&self) -> Vec<SendReport> {
        self.destinations
            .iter()
            .map(|destination| {
                let mut report = SendReport::new(destination.receiver);
                report.self_copy = destination.self_copy;
                report.queued = !destination.complete();
                report.accepted = !destination.accepted.is_empty();
                report.error = destination.error.clone().map(Into::into);
                if let Some(wrap) = &destination.wrap {
                    report.gift_wrap_id = Some(wrap.id);
                    let mut output = Output::new(wrap.id);
                    output.success.extend(
                        destination
                            .accepted
                            .iter()
                            .cloned()
                            .map(|url| (url, EventSendStatus::Sent)),
                    );
                    output.failed.extend(destination.failed.clone());
                    report.output = Some(output);
                }
                report
            })
            .collect()
    }
}

async fn load(root: &Path, owner: PublicKey) -> Result<Vec<OutgoingMessage>> {
    let dir = root.join(owner.to_hex());
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut messages = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let record: OutgoingMessage = serde_json::from_slice(&std::fs::read(&path)?)?;
        if record.owner != owner || record.rumor.pubkey != owner {
            bail!("Outgoing account mismatch");
        }
        record.rumor.verify_id()?;
        if record.rumor.id.is_none() {
            bail!("Outgoing message is missing its ID");
        }
        if path.file_stem().and_then(|name| name.to_str()) != Some(record.id().to_hex().as_str()) {
            bail!("Outgoing message filename does not match its ID");
        }
        messages.push(record);
    }
    messages.sort_by_key(|message| (message.rumor.created_at, message.id()));
    Ok(messages)
}

async fn save(root: &Path, record: &mut OutgoingMessage) -> Result<()> {
    // A separate local store is essential: relay events in the Nostr database
    // must never be interpreted as instructions to sign or send messages.
    let dir = root.join(record.owner.to_hex());
    std::fs::create_dir_all(&dir)?;
    record.revision += 1;
    let mut temp = tempfile::NamedTempFile::new_in(&dir)?;
    temp.write_all(&serde_json::to_vec(record)?)?;
    temp.as_file().sync_all()?;
    temp.persist(dir.join(format!("{}.json", record.id())))
        .map_err(|e| e.error)?;
    #[cfg(unix)]
    std::fs::File::open(&dir)?.sync_all()?;
    Ok(())
}

#[derive(Debug, Clone)]
pub(super) struct OutgoingQueue {
    client: Client,
    root: PathBuf,
    owner: PublicKey,
    wake: flume::Sender<()>,
    enqueue_lock: Arc<Mutex<()>>,
    encryption: Arc<RwLock<Option<UniversalSigner>>>,
}

impl OutgoingQueue {
    pub fn new(
        client: Client,
        owner: PublicKey,
        encryption: Option<UniversalSigner>,
    ) -> (Self, flume::Receiver<()>) {
        let (wake, receiver) = flume::bounded(1);
        (
            Self {
                client,
                root: common::config_dir().join(STORE_TAG),
                owner,
                wake,
                enqueue_lock: Arc::default(),
                encryption: Arc::new(RwLock::new(encryption)),
            },
            receiver,
        )
    }

    pub fn set_encryption_signer(&self, signer: Option<UniversalSigner>) {
        *self.encryption.write().unwrap() = signer;
        self.retry();
    }

    pub fn retry(&self) {
        let _ = self.wake.try_send(());
    }

    pub async fn enqueue(&self, mut message: OutgoingMessage) -> Result<Vec<SendReport>> {
        if message.owner != self.owner {
            bail!("Outgoing account changed");
        }
        let _guard = self.enqueue_lock.lock().await;
        if let Some(existing) = load(&self.root, self.owner)
            .await?
            .into_iter()
            .find(|job| job.id() == message.id())
        {
            self.retry();
            return Ok(existing.reports());
        }
        // Persist intent before requesting signatures, clearing the composer, or publishing.
        save(&self.root, &mut message).await?;
        self.retry();
        Ok(message.reports())
    }

    pub async fn run(
        &self,
        signer: UniversalSigner,
        wake: flume::Receiver<()>,
        signals: flume::Sender<Signal>,
    ) -> Result<()> {
        let mut announced = BTreeMap::new();
        loop {
            // This task is cancelled on account changes. Also check before each pass
            // because UniversalSigner can be swapped while a request is in flight.
            match self.process(&signer, &signals, &mut announced).await {
                Ok(()) => {}
                Err(error) => {
                    signals
                        .send_async(Signal::OutgoingError(error.to_string()))
                        .await?;
                }
            }
            let next = wake.recv_async().fuse();
            let timer = async_utility::time::sleep(Duration::from_secs(30)).fuse();
            futures::pin_mut!(next, timer);
            futures::select! { result = next => { result?; }, _ = timer => {} }
        }
    }

    async fn process(
        &self,
        signer: &UniversalSigner,
        signals: &flume::Sender<Signal>,
        announced: &mut BTreeMap<EventId, u64>,
    ) -> Result<()> {
        for mut message in load(&self.root, self.owner).await? {
            // Cached plaintext also restores conversations before any relay accepts a copy.
            if announced.get(&message.id()) != Some(&message.revision) {
                set_rumor(&self.client, message.id(), &message.rumor).await?;
                signals
                    .send_async(Signal::Outgoing(message.clone()))
                    .await?;
                announced.insert(message.id(), message.revision);
            }
            if message.complete() {
                continue;
            }
            for index in 0..message.destinations.len() {
                if message.destinations[index].complete() {
                    continue;
                }
                if signer.get_public_key_async().await? != self.owner {
                    bail!("Outgoing account changed");
                }
                if message.destinations[index].wrap.is_none() {
                    let encryption = self.encryption.read().unwrap().clone();
                    let result = async_utility::time::timeout(
                        Some(Duration::from_secs(30)),
                        prepare(&self.client, signer, encryption.as_ref(), &message, index),
                    )
                    .await
                    .unwrap_or_else(|| Err(anyhow!("Signer preparation timed out")));
                    match result {
                        Ok(wrap) => {
                            message.destinations[index].wrap = Some(wrap);
                            message.destinations[index].error = None;
                        }
                        Err(error) => {
                            message.destinations[index].error = Some(error.to_string());
                        }
                    }
                    // A prepared wrap must survive a crash before it can leave this machine.
                    save(&self.root, &mut message).await?;
                }
                if message.destinations[index].wrap.is_some() {
                    publish(&self.client, &mut message.destinations[index]).await;
                    save(&self.root, &mut message).await?;
                }
                signals
                    .send_async(Signal::Outgoing(message.clone()))
                    .await?;
                announced.insert(message.id(), message.revision);
            }
        }
        Ok(())
    }
}

async fn prepare(
    client: &Client,
    signer: &UniversalSigner,
    encryption: Option<&UniversalSigner>,
    message: &OutgoingMessage,
    index: usize,
) -> Result<Event> {
    let destination = &message.destinations[index];
    let mut announcement = destination.announcement;
    if !message.signer_kind.user() && announcement.is_none() {
        let events = client
            .database()
            .query(
                Filter::new()
                    .kind(Kind::Custom(10044))
                    .author(destination.receiver)
                    .limit(1),
            )
            .await?;
        announcement = events
            .first()
            .map(|event| Announcement::from(event).public_key());
    }
    if message.signer_kind.encryption() && (announcement.is_none() || encryption.is_none()) {
        bail!("Encryption key unavailable; waiting to retry");
    }
    let target = if message.signer_kind.user() {
        destination.receiver
    } else {
        announcement.unwrap_or(destination.receiver)
    };
    let signing = if !message.signer_kind.user() && announcement.is_some() {
        encryption.unwrap_or(signer)
    } else {
        signer
    };
    let mut tags = vec![Tag::custom("k", ["14"])];
    if target != destination.receiver {
        tags.push(Tag::public_key(destination.receiver));
    }
    Ok(nip59::GiftWrapBuilder::new(target, message.rumor.clone())
        .extra_tags(tags)
        .finalize_async(signing)
        .await?)
}

pub(super) async fn publish(client: &Client, destination: &mut Destination) {
    let event = destination
        .wrap
        .as_ref()
        .expect("prepared before publishing");
    let result = async_utility::time::timeout(Some(Duration::from_secs(30)), async {
        let request = client.send_event(event).ack_policy(AckPolicy::all());
        // Retry only failed relays once discovery has returned a destination set.
        if !destination.failed.is_empty() {
            let urls: Vec<_> = destination.failed.keys().cloned().collect();
            for url in &urls {
                client.add_relay(url).and_connect().await?;
            }
            request.to(urls).await
        } else {
            request.to_nip17().await
        }
    })
    .await;
    match result {
        Some(Ok(output)) => {
            for (url, status) in output.success {
                if status.is_ack() {
                    destination.accepted.insert(url.clone());
                    destination.failed.remove(&url);
                }
            }
            destination.failed.extend(output.failed);
            destination.error = if destination.accepted.is_empty() && destination.failed.is_empty()
            {
                Some("No relay accepted the message".into())
            } else {
                None
            };
        }
        Some(Err(error)) => destination.error = Some(error.to_string()),
        None => destination.error = Some("Relay delivery timed out; queued for retry".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr_gossip_memory::prelude::*;
    use nostr_sdk::local_relay::{LocalRelay, MockRelay, WritePolicy, WritePolicyResult};
    use std::sync::{
        Mutex as StdMutex,
        atomic::{AtomicBool, Ordering},
    };

    #[derive(Debug, Clone)]
    struct RecordingPolicy {
        accept: Arc<AtomicBool>,
        received: Arc<StdMutex<Vec<EventId>>>,
    }
    impl RecordingPolicy {
        fn new(accept: bool) -> Self {
            Self {
                accept: Arc::new(AtomicBool::new(accept)),
                received: Arc::default(),
            }
        }
    }
    impl WritePolicy for RecordingPolicy {
        fn admit_event<'a>(
            &'a self,
            event: &'a Event,
            _: &'a std::net::SocketAddr,
        ) -> futures::future::BoxFuture<'a, WritePolicyResult> {
            Box::pin(async move {
                self.received.lock().unwrap().push(event.id);
                if self.accept.load(Ordering::SeqCst) {
                    WritePolicyResult::Accept
                } else {
                    WritePolicyResult::reject(
                        MachineReadablePrefix::Error,
                        "temporarily unavailable",
                    )
                }
            })
        }
    }

    async fn client(discovery: &RelayUrl) -> Client {
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
                    .no_background_refresh(),
            )
            .build();
        client
            .add_relay(discovery)
            .capabilities(RelayCapabilities::DISCOVERY)
            .and_connect()
            .await
            .unwrap();
        client
    }

    fn queue(client: Client, owner: PublicKey, dir: &Path) -> OutgoingQueue {
        let (mut queue, _) = OutgoingQueue::new(client, owner, None);
        queue.root = dir.to_owned();
        queue
    }

    fn message(owner: &Keys, recipients: &[PublicKey]) -> OutgoingMessage {
        let rumor = EventBuilder::new(Kind::PrivateDirectMessage, "durable group message")
            .tags(recipients.iter().copied().map(Tag::public_key))
            .finalize_unsigned(owner.public_key());
        let destinations = recipients
            .iter()
            .copied()
            .map(|key| Destination::new(key, None, false))
            .chain([Destination::new(owner.public_key(), None, true)])
            .collect();
        OutgoingMessage::new(owner.public_key(), rumor, SignerKind::User, destinations).unwrap()
    }

    #[tokio::test]
    async fn restart_retries_only_failed_relays_with_identical_wraps_and_independent_self_copy() {
        tokio::time::timeout(Duration::from_secs(10), async {
            let dir = tempfile::tempdir().unwrap();
            let owner = Keys::generate();
            let bob = Keys::generate();
            let carol = Keys::generate();
            let good_policy = RecordingPolicy::new(true);
            let bad_policy = RecordingPolicy::new(false);
            let good = LocalRelay::builder()
                .write_policy(good_policy.clone())
                .build();
            let bad = LocalRelay::builder()
                .write_policy(bad_policy.clone())
                .build();
            good.run().await.unwrap();
            bad.run().await.unwrap();
            let good_url = good.url().await;
            let bad_url = bad.url().await;
            let discovery = MockRelay::run().await.unwrap();
            for (keys, urls) in [
                (&owner, vec![good_url.clone()]),
                (&bob, vec![good_url.clone(), bad_url.clone()]),
                (&carol, vec![bad_url.clone()]),
            ] {
                discovery
                    .add_event(InboxRelayList::new(urls).finalize(keys).unwrap())
                    .await
                    .unwrap();
            }
            let first_client = client(&discovery.url().await).await;
            let first = queue(first_client.clone(), owner.public_key(), dir.path());
            let message = message(&owner, &[bob.public_key(), carol.public_key()]);
            let rumor_id = message.id();
            first.enqueue(message).await.unwrap();
            let (signals, _rx) = flume::unbounded();
            first
                .process(
                    &UniversalSigner::new(owner.clone()),
                    &signals,
                    &mut BTreeMap::new(),
                )
                .await
                .unwrap();
            let saved = load(dir.path(), owner.public_key())
                .await
                .unwrap()
                .remove(0);
            let wraps: Vec<_> = saved
                .destinations
                .iter()
                .map(|d| d.wrap.as_ref().unwrap().id)
                .collect();
            assert!(!saved.complete());
            assert!(!saved.destinations[0].complete()); // Bob's second relay rejected.
            assert!(saved.destinations[1].accepted.is_empty()); // Carol entirely failed.
            assert!(saved.destinations[2].complete()); // Self-copy is independent.
            assert_eq!(good_policy.received.lock().unwrap().len(), 2);
            assert_eq!(bad_policy.received.lock().unwrap().len(), 2);
            first_client.shutdown().await;
            drop(first);

            bad_policy.accept.store(true, Ordering::SeqCst);
            let second_client = client(&discovery.url().await).await;
            let second = queue(second_client.clone(), owner.public_key(), dir.path());
            let mut announced = BTreeMap::new();
            second
                .process(
                    &UniversalSigner::new(UnavailableSigner(owner.clone())),
                    &signals,
                    &mut announced,
                )
                .await
                .unwrap();
            let restored = load(dir.path(), owner.public_key())
                .await
                .unwrap()
                .remove(0);
            assert_eq!(restored.id(), rumor_id);
            assert!(restored.complete());
            assert_eq!(
                restored
                    .destinations
                    .iter()
                    .map(|d| d.wrap.as_ref().unwrap().id)
                    .collect::<Vec<_>>(),
                wraps
            );
            assert_eq!(
                good_policy.received.lock().unwrap().len(),
                2,
                "accepted relays must not be resent"
            );
            let attempts = bad_policy.received.lock().unwrap().clone();
            assert_eq!(attempts, vec![wraps[0], wraps[1], wraps[0], wraps[1]]);
            assert!(
                load(dir.path(), Keys::generate().public_key())
                    .await
                    .unwrap()
                    .is_empty()
            );
            second
                .process(&UniversalSigner::new(owner), &signals, &mut announced)
                .await
                .unwrap();
            assert_eq!(
                bad_policy.received.lock().unwrap().len(),
                4,
                "completed messages must not resend"
            );
            second_client.shutdown().await;
            good.shutdown();
            bad.shutdown();
        })
        .await
        .expect("outgoing retries should finish");
    }

    #[derive(Debug)]
    struct UnavailableSigner(Keys);
    impl AsyncGetPublicKey for UnavailableSigner {
        type Error = <Keys as AsyncGetPublicKey>::Error;
        fn get_public_key_async(
            &self,
        ) -> futures::future::BoxFuture<'_, std::result::Result<PublicKey, Self::Error>> {
            self.0.get_public_key_async()
        }
    }
    impl AsyncSignEvent for UnavailableSigner {
        type Error = std::io::Error;
        fn sign_event_async(
            &self,
            _: UnsignedEvent,
        ) -> futures::future::BoxFuture<'_, std::result::Result<Event, Self::Error>> {
            Box::pin(async { Err(std::io::Error::other("signer disconnected")) })
        }
    }
    impl AsyncNip44 for UnavailableSigner {
        type Error = <Keys as AsyncNip44>::Error;
        fn nip44_encrypt_async<'a>(
            &'a self,
            key: &'a PublicKey,
            content: &'a str,
        ) -> futures::future::BoxFuture<'a, std::result::Result<String, Self::Error>> {
            self.0.nip44_encrypt_async(key, content)
        }
        fn nip44_decrypt_async<'a>(
            &'a self,
            key: &'a PublicKey,
            content: &'a str,
        ) -> futures::future::BoxFuture<'a, std::result::Result<String, Self::Error>> {
            self.0.nip44_decrypt_async(key, content)
        }
    }

    #[tokio::test]
    async fn intent_survives_signer_failure_before_wrap_preparation() {
        let dir = tempfile::tempdir().unwrap();
        let owner = Keys::generate();
        let recipient = Keys::generate();
        let inbox = MockRelay::run().await.unwrap();
        let discovery = MockRelay::run().await.unwrap();
        for keys in [&owner, &recipient] {
            discovery
                .add_event(
                    InboxRelayList::new([inbox.url().await])
                        .finalize(keys)
                        .unwrap(),
                )
                .await
                .unwrap();
        }
        let first_client = client(&discovery.url().await).await;
        let first = queue(first_client.clone(), owner.public_key(), dir.path());
        let original = message(&owner, &[recipient.public_key()]);
        let id = original.id();
        first.enqueue(original).await.unwrap();
        let (signals, _rx) = flume::unbounded();
        first
            .process(
                &UniversalSigner::new(UnavailableSigner(owner.clone())),
                &signals,
                &mut BTreeMap::new(),
            )
            .await
            .unwrap();
        let saved = load(dir.path(), owner.public_key())
            .await
            .unwrap()
            .remove(0);
        assert_eq!(saved.id(), id);
        assert!(
            saved
                .destinations
                .iter()
                .all(|d| d.wrap.is_none() && d.error.is_some())
        );
        first_client.shutdown().await;
        let second_client = client(&discovery.url().await).await;
        let second = queue(second_client.clone(), owner.public_key(), dir.path());
        second
            .process(
                &UniversalSigner::new(owner.clone()),
                &signals,
                &mut BTreeMap::new(),
            )
            .await
            .unwrap();
        let recovered = load(dir.path(), owner.public_key())
            .await
            .unwrap()
            .remove(0);
        assert_eq!(recovered.id(), id);
        assert!(recovered.complete());
        second_client.shutdown().await;
    }

    #[tokio::test]
    async fn relay_database_cannot_inject_outgoing_intents_and_disk_errors_are_returned() {
        let dir = tempfile::tempdir().unwrap();
        let owner = Keys::generate();
        let client = Client::builder()
            .database(nostr_memory::MemoryDatabase::unbounded())
            .build();
        let message = message(&owner, &[Keys::generate().public_key()]);
        let injected = EventBuilder::new(
            Kind::ApplicationSpecificData,
            serde_json::to_string(&message).unwrap(),
        )
        .tags([
            Tag::custom("k", [STORE_TAG]),
            Tag::public_key(owner.public_key()),
        ])
        .finalize(&Keys::generate())
        .unwrap();
        client.database().save_event(&injected).await.unwrap();
        assert!(
            load(dir.path(), owner.public_key())
                .await
                .unwrap()
                .is_empty()
        );
        let bad_path = dir.path().join("not-a-directory");
        std::fs::write(&bad_path, "file").unwrap();
        let queue = queue(client.clone(), owner.public_key(), &bad_path);
        assert!(queue.enqueue(message).await.is_err());
        client.shutdown().await;
    }
}
