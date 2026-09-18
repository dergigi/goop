//! Account-scoped, local-only outgoing intents and prepared gift wraps.
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{Result, bail};
use futures::{FutureExt, lock::Mutex};
use nostr_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use state::{SignerFailure, UniversalSigner};

use super::{SendReport, Signal};
use common::EventExt;

const STORE_TAG: &str = "goop-outgoing-v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Destination {
    pub receiver: PublicKey,
    pub self_copy: bool,
    pub wrap: Option<Event>,
    pub accepted: BTreeSet<RelayUrl>,
    pub failed: BTreeMap<RelayUrl, String>,
    pub error: Option<String>,
}

impl Destination {
    pub fn new(receiver: PublicKey, self_copy: bool) -> Self {
        Self {
            receiver,
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
    // Read old queue records without silently publishing their experimental wraps.
    #[serde(default, rename = "signer_kind", skip_serializing_if = "Option::is_none")]
    legacy_signer_kind: Option<String>,
    pub destinations: Vec<Destination>,
    revision: u64,
    #[serde(default)]
    paused: bool,
}

impl OutgoingMessage {
    pub fn new(
        owner: PublicKey,
        rumor: UnsignedEvent,
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
            legacy_signer_kind: None,
            destinations,
            revision: 0,
            paused: false,
        })
    }

    fn uses_removed_encryption(&self) -> bool {
        self.legacy_signer_kind.as_deref().is_some_and(|kind| kind != "User")
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
                report.paused = self.paused && !destination.complete();
                report.queued = !self.paused && !destination.complete();
                report.accepted = !destination.accepted.is_empty();
                report.error = destination.error.clone().map(Into::into);
                if report.paused && report.error.is_none() {
                    // A refusal stops the whole message, including unattempted copies.
                    report.error = self.destinations.iter().filter(|other| other.wrap.is_none()).find_map(|other| {
                        other.error.as_ref().map(|reason| {
                            format!("This copy is waiting because another copy could not be prepared. {reason}").into()
                        })
                    });
                }
                if self.uses_removed_encryption() && !destination.complete() {
                    report.paused = true;
                    report.queued = false;
                    report.error = Some("This queued message uses removed experimental encryption. Send a new message to use NIP-17.".into());
                }
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
    {
        std::fs::File::open(&dir)?.sync_all()?;
        std::fs::File::open(root)?.sync_all()?;
        if let Some(parent) = root.parent() {
            std::fs::File::open(parent)?.sync_all()?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub(super) struct OutgoingQueue {
    client: Client,
    root: PathBuf,
    owner: PublicKey,
    wake: flume::Sender<()>,
    enqueue_lock: Arc<Mutex<()>>,
    active: Arc<AtomicBool>,
    resume_requested: Arc<AtomicBool>,
    rebroadcast_requested: Arc<RwLock<BTreeSet<EventId>>>,
    retry_requested: Arc<RwLock<BTreeSet<EventId>>>,
}

impl OutgoingQueue {
    pub fn new(
        client: Client,
        owner: PublicKey,
    ) -> (Self, flume::Receiver<()>) {
        let (wake, receiver) = flume::bounded(1);
        (
            Self {
                client,
                root: common::config_dir().join(STORE_TAG),
                owner,
                wake,
                enqueue_lock: Arc::default(),
                active: Arc::new(AtomicBool::new(true)),
                resume_requested: Arc::default(),
                rebroadcast_requested: Arc::default(),
                retry_requested: Arc::default(),
            },
            receiver,
        )
    }

    pub fn stop(&self) {
        self.active.store(false, Ordering::SeqCst);
        self.wake();
    }

    fn ensure_active(&self) -> Result<()> {
        if !self.active.load(Ordering::SeqCst) {
            bail!("Outgoing account is inactive");
        }
        Ok(())
    }

    pub async fn all_messages(&self) -> Result<Vec<UnsignedEvent>> {
        self.ensure_active()?;
        Ok(load(&self.root, self.owner)
            .await?
            .into_iter()
            .map(|job| job.rumor)
            .collect())
    }

    pub async fn messages(&self, room: u64) -> Result<Vec<UnsignedEvent>> {
        self.ensure_active()?;
        Ok(load(&self.root, self.owner)
            .await?
            .into_iter()
            .filter(|job| job.rumor.uniq_id() == room || job.rumor.kind == Kind::Reaction)
            .map(|job| job.rumor)
            .collect())
    }

    pub fn retry(&self) {
        self.resume_requested.store(true, Ordering::SeqCst);
        self.wake();
    }

    pub fn retry_message(&self, id: EventId) {
        self.retry_requested.write().unwrap().insert(id);
        self.wake();
    }

    pub async fn rebroadcast(&self, id: EventId) -> Result<()> {
        self.ensure_active()?;
        let message = load(&self.root, self.owner).await?.into_iter()
            .find(|message| message.id() == id)
            .ok_or_else(|| anyhow::anyhow!("Original outgoing message is unavailable on this device"))?;
        if message.uses_removed_encryption() {
            bail!("This message uses removed experimental encryption and cannot be rebroadcast. Send a new message to use NIP-17.");
        }
        if message.destinations.iter().any(|destination| destination.wrap.is_none()) {
            bail!("Message is still being prepared; retry it from Delivery status first");
        }
        self.ensure_active()?;
        self.rebroadcast_requested.write().unwrap().insert(id);
        self.wake();
        Ok(())
    }

    fn wake(&self) {
        let _ = self.wake.try_send(());
    }

    pub async fn enqueue(&self, mut message: OutgoingMessage) -> Result<Vec<SendReport>> {
        if message.owner != self.owner {
            bail!("Outgoing account changed");
        }
        let _guard = self.enqueue_lock.lock().await;
        self.ensure_active()?;
        if let Some(existing) = load(&self.root, self.owner)
            .await?
            .into_iter()
            .find(|job| job.id() == message.id())
        {
            self.wake();
            return Ok(existing.reports());
        }
        // Persist intent before requesting signatures, clearing the composer, or publishing.
        save(&self.root, &mut message).await?;
        self.wake();
        Ok(message.reports())
    }

    pub async fn run(
        &self,
        signer: UniversalSigner,
        wake: flume::Receiver<()>,
        signals: flume::Sender<Signal>,
    ) -> Result<()> {
        let mut announced = BTreeMap::new();
        let mut last_error = None;
        while self.active.load(Ordering::SeqCst) {
            match self.process(&signer, &signals, &mut announced).await {
                Ok(()) => {
                    if last_error.take().is_some() {
                        signals.send_async(Signal::OutgoingRecovered).await?;
                    }
                },
                Err(error) => {
                    self.ensure_active()?;
                    let error = error.to_string();
                    if last_error.as_ref() != Some(&error) {
                        signals
                            .send_async(Signal::OutgoingError(error.clone()))
                            .await?;
                        last_error = Some(error);
                    }
                }
            }
            let next = wake.recv_async().fuse();
            let timer = async_utility::time::sleep(Duration::from_secs(30)).fuse();
            futures::pin_mut!(next, timer);
            futures::select! { result = next => { result?; }, _ = timer => {} }
        }
        Ok(())
    }

    async fn process(
        &self,
        signer: &UniversalSigner,
        signals: &flume::Sender<Signal>,
        announced: &mut BTreeMap<EventId, u64>,
    ) -> Result<()> {
        self.ensure_active()?;
        let resume = self.resume_requested.swap(false, Ordering::SeqCst);
        for mut message in load(&self.root, self.owner).await? {
            if message.uses_removed_encryption() {
                if announced.get(&message.id()) != Some(&message.revision) {
                    signals.send_async(Signal::Outgoing(message.clone())).await?;
                    announced.insert(message.id(), message.revision);
                }
                continue;
            }
            if self.rebroadcast_requested.write().unwrap().remove(&message.id()) {
                // Only the worker mutates persisted jobs. Keep the original
                // rumors and signed wraps; requeue exactly this message.
                for destination in &mut message.destinations {
                    destination.accepted.clear();
                    destination.failed.clear();
                    destination.error = None;
                }
                message.paused = false;
                save(&self.root, &mut message).await?;
            }
            let retry_this = self.retry_requested.write().unwrap().remove(&message.id());
            if (resume || retry_this) && message.paused {
                message.paused = false;
                save(&self.root, &mut message).await?;
            }
            self.ensure_active()?;
            // Restore only this account's conversation state from its local queue.
            if announced.get(&message.id()) != Some(&message.revision) {
                signals
                    .send_async(Signal::Outgoing(message.clone()))
                    .await?;
                announced.insert(message.id(), message.revision);
            }
            if message.complete() || message.paused {
                continue;
            }
            for index in 0..message.destinations.len() {
                if message.destinations[index].complete() {
                    continue;
                }
                self.ensure_active()?;
                if message.destinations[index].wrap.is_none() {
                    let result =
                        async_utility::time::timeout(Some(Duration::from_secs(30)), async {
                            let signing_owner = signer.get_public_key_async().await?;
                            if signing_owner != self.owner {
                                bail!("Outgoing account changed");
                            }
                            prepare(signer, &message, index)
                                .await
                        })
                        .await
                        .unwrap_or_else(|| {
                            Err(std::io::Error::from(std::io::ErrorKind::TimedOut).into())
                        });
                    self.ensure_active()?;
                    match result {
                        Ok(wrap) => {
                            message.destinations[index].wrap = Some(wrap);
                            message.destinations[index].error = None;
                        }
                        Err(error) => {
                            let failure = SignerFailure::classify(error.as_ref());
                            message.paused = failure.requires_retry();
                            message.destinations[index].error = Some(preparation_error(&error));
                        }
                    }
                    // A prepared wrap must survive a crash before it can leave this machine.
                    save(&self.root, &mut message).await?;
                }
                if message.destinations[index].wrap.is_some() {
                    self.ensure_active()?;
                    publish(&self.client, &mut message.destinations[index]).await;
                    save(&self.root, &mut message).await?;
                }
                signals
                    .send_async(Signal::Outgoing(message.clone()))
                    .await?;
                announced.insert(message.id(), message.revision);
                if message.paused {
                    break;
                }
            }
        }
        Ok(())
    }
}

fn preparation_error(error: &anyhow::Error) -> String {
    let failure = SignerFailure::classify(error.as_ref());
    if failure == SignerFailure::Timeout {
        "Your signer did not respond in time. Goop will retry automatically. Open your signer and approve the request when prompted.".into()
    } else if failure.requires_retry() {
        format!("The signer did not complete the request. Sending stopped. Check your signer, then retry. Details: {error:#}")
    } else {
        format!("Could not prepare the encrypted message; Goop will retry. Details: {error:#}")
    }
}

async fn prepare(
    signer: &UniversalSigner,
    message: &OutgoingMessage,
    index: usize,
) -> Result<Event> {
    let destination = &message.destinations[index];
    Ok(nip59::GiftWrapBuilder::new(destination.receiver, message.rumor.clone())
        .finalize_async(signer)
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
        let (mut queue, _) = OutgoingQueue::new(client, owner);
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
            .map(|key| Destination::new(key, false))
            .chain([Destination::new(owner.public_key(), true)])
            .collect();
        OutgoingMessage::new(owner.public_key(), rumor, destinations).unwrap()
    }

    #[test]
    fn preparation_timeouts_identify_signer_and_automatic_retry() {
        for error in [
            anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::TimedOut)),
            anyhow::Error::new(SignerFailure::Timeout).context("Preparing recipient copy"),
        ] {
            let reason = preparation_error(&error);
            assert!(reason.contains("Your signer did not respond in time"));
            assert!(reason.contains("Goop will retry automatically"));
            assert!(reason.contains("approve the request when prompted"));
            assert!(!SignerFailure::classify(error.as_ref()).requires_retry());
        }
        let unrelated = preparation_error(&anyhow::anyhow!("Invalid recipient"));
        assert!(!unrelated.contains("Your signer did not respond"));
        assert!(unrelated.contains("Invalid recipient"));
    }

    #[test]
    fn preparation_errors_preserve_remote_reason_and_context() {
        let error = anyhow::Error::new(SignerFailure::Rejected)
            .context("nip44_encrypt is not permitted")
            .context("Preparing recipient copy");
        let reason = preparation_error(&error);
        assert!(reason.contains("nip44_encrypt is not permitted"));
        assert!(reason.contains("Preparing recipient copy"));
        assert!(reason.contains("Sending stopped"));
        assert!(!reason.contains("Goop will retry"));
    }

    #[tokio::test]
    async fn legacy_identity_queue_ignores_alternate_recipient_keys() {
        let owner = Keys::generate();
        let recipient = Keys::generate();
        let original = message(&owner, &[recipient.public_key()]);
        let mut json = serde_json::to_value(&original).unwrap();
        json["signer_kind"] = "User".into();
        json["destinations"][0]["announcement"] = serde_json::to_value(Keys::generate().public_key()).unwrap();
        let restored: OutgoingMessage = serde_json::from_value(json).unwrap();
        assert!(!restored.uses_removed_encryption());
        let wrap = prepare(&UniversalSigner::new(owner.clone()), &restored, 0).await.unwrap();
        let unwrapped = nip59::extract_rumor(&recipient, &wrap).unwrap();
        assert_eq!(unwrapped.sender, owner.public_key());
        assert_eq!(unwrapped.rumor.content, original.rumor.content);
        assert!(serde_json::to_value(original).unwrap().get("signer_kind").is_none());
    }

    #[tokio::test]
    async fn experimental_queue_records_are_preserved_and_never_retried_or_rebroadcast() {
        let owner = Keys::generate();
        let recipient = Keys::generate();
        let client = Client::default();
        for kind in ["Auto", "Encryption"] {
            let dir = tempfile::tempdir().unwrap();
            let queue = queue(client.clone(), owner.public_key(), dir.path());
            let mut json = serde_json::to_value(message(&owner, &[recipient.public_key()])).unwrap();
            json["signer_kind"] = kind.into();
            let mut legacy: OutgoingMessage = serde_json::from_value(json).unwrap();
            // Even an already-prepared wrap must not escape through retry/rebroadcast.
            legacy.destinations[0].wrap = Some(prepare(&UniversalSigner::new(owner.clone()), &legacy, 0).await.unwrap());
            save(dir.path(), &mut legacy).await.unwrap();
            let before = std::fs::read(dir.path().join(owner.public_key().to_hex()).join(format!("{}.json", legacy.id()))).unwrap();
            let (tx, rx) = flume::unbounded();
            queue.retry();
            // A revoked signer proves no signing is attempted while restoring the queue.
            let signer = UniversalSigner::new(owner.clone());
            signer.disconnect();
            queue.process(&signer, &tx, &mut BTreeMap::new()).await.unwrap();
            let Signal::Outgoing(restored) = rx.recv().unwrap() else { panic!("missing restored message"); };
            assert!(restored.reports().iter().all(|r| r.paused && r.error.is_some()));
            assert!(queue.rebroadcast(legacy.id()).await.unwrap_err().to_string().contains("removed experimental encryption"));
            let after = std::fs::read(dir.path().join(owner.public_key().to_hex()).join(format!("{}.json", legacy.id()))).unwrap();
            assert_eq!(before, after);
        }
        client.shutdown().await;
    }

    #[tokio::test]
    async fn encrypted_file_keys_survive_restart_and_stay_inside_gift_wraps() {
        let owner = Keys::generate();
        let receiver = Keys::generate();
        let plaintext = b"private image fixture";
        let mut encrypted = state::encrypted_file::EncryptedFile::encrypt(plaintext, "image/png".into()).unwrap();
        encrypted.file.url = Url::parse("https://example.com/ciphertext").unwrap();
        let rumor = EventBuilder::new(Kind::Custom(15), encrypted.file.url.to_string())
            .tags(encrypted.file.tags()).tag(Tag::public_key(receiver.public_key()))
            .finalize_unsigned(owner.public_key());
        let original = OutgoingMessage::new(owner.public_key(), rumor,
            vec![Destination::new(receiver.public_key(), false),
                 Destination::new(owner.public_key(), true)]).unwrap();
        let client = Client::builder().database(nostr_memory::MemoryDatabase::unbounded()).build();
        let dir = tempfile::tempdir().unwrap();
        let queue = queue(client.clone(), owner.public_key(), dir.path());
        queue.enqueue(original).await.unwrap();
        let stored = load(dir.path(), owner.public_key()).await.unwrap().remove(0);
        for (index, recipient) in [&receiver, &owner].into_iter().enumerate() {
            let wrap = prepare(&UniversalSigner::new(owner.clone()), &stored, index).await.unwrap();
            assert_eq!(wrap.kind, Kind::GiftWrap);
            assert!(!wrap.as_json().contains("decryption-key"));
            assert!(!wrap.as_json().contains(encrypted.file.url.as_str()));
            assert!(wrap.tags.iter().all(|tag| tag.kind() != "k"));
            let unwrapped = nip59::extract_rumor(recipient, &wrap).unwrap();
            assert_eq!(unwrapped.rumor.kind, Kind::Custom(15));
            let file = state::encrypted_file::EncryptedFile::from_tags(&unwrapped.rumor.content, &unwrapped.rumor.tags).unwrap();
            assert_eq!(file.decrypt(&encrypted.ciphertext).unwrap(), plaintext);
            let rendered = crate::Message::from(&unwrapped.rumor);
            assert!(rendered.media.is_empty());
            assert!(rendered.encrypted_file.unwrap().is_ok());
        }
        client.shutdown().await;
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
            second.retry_message(rumor_id);
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
            let previous_revision = restored.revision;
            second.rebroadcast(rumor_id).await.unwrap();
            second.process(&UniversalSigner::new(UnavailableSigner(Keys::generate())),
                &signals, &mut announced).await.unwrap();
            let rebroadcast = load(dir.path(), saved.owner).await.unwrap().remove(0);
            assert!(rebroadcast.complete());
            assert!(rebroadcast.revision > previous_revision);
            assert_eq!(rebroadcast.rumor, restored.rumor);
            assert_eq!(rebroadcast.destinations.iter().map(|d| d.wrap.clone()).collect::<Vec<_>>(),
                restored.destinations.iter().map(|d| d.wrap.clone()).collect::<Vec<_>>());
            assert!(second.rebroadcast(EventId::from_byte_array([0; 32])).await.is_err());
            second.stop();
            assert!(second.rebroadcast(rumor_id).await.is_err());
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
    async fn refusal_survives_restart_and_only_explicit_retry_resumes_signing() {
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
        let client = client(&discovery.url().await).await;
        let first = queue(client.clone(), owner.public_key(), dir.path());
        let controlled = crate::test_signer::RefusingSigner::new(owner.clone());
        let signer = UniversalSigner::new(controlled.clone());
        let original = message(&owner, &[recipient.public_key()]);
        let id = original.id();
        first.enqueue(original).await.unwrap();
        let (signals, _rx) = flume::unbounded();
        first
            .process(&signer, &signals, &mut BTreeMap::new())
            .await
            .unwrap();
        assert_eq!(controlled.calls.load(Ordering::SeqCst), 1);
        let saved = load(dir.path(), owner.public_key())
            .await
            .unwrap()
            .remove(0);
        assert!(saved.paused);
        assert!(saved.reports().iter().all(|r| r.paused && !r.pending()));
        assert!(saved.reports().iter().all(|r| r.error.as_ref().is_some_and(|error|
            error.contains("signer rejected the operation"))));
        assert!(saved.reports().iter().any(|r| r.error.as_ref().is_some_and(|error|
            error.contains("another copy"))));
        assert!(saved.destinations.iter().all(|d| d.wrap.is_none()));
        first
            .process(&signer, &signals, &mut BTreeMap::new())
            .await
            .unwrap();
        let restarted = queue(client.clone(), owner.public_key(), dir.path());
        // A signer reconnect must not reverse the user's refusal.
        controlled.refused.store(false, Ordering::SeqCst);
        restarted
            .process(&signer, &signals, &mut BTreeMap::new())
            .await
            .unwrap();
        assert_eq!(controlled.calls.load(Ordering::SeqCst), 1);
        let mut other = message(&owner, &[Keys::generate().public_key()]);
        other.paused = true;
        let other_id = other.id();
        restarted.enqueue(other).await.unwrap();
        restarted.retry_message(id);
        restarted
            .process(&signer, &signals, &mut BTreeMap::new())
            .await
            .unwrap();
        let jobs = load(dir.path(), owner.public_key()).await.unwrap();
        assert!(jobs.iter().find(|job| job.id() == other_id).unwrap().paused);
        let saved = jobs.iter().find(|job| job.id() == id).unwrap();
        assert_eq!(saved.id(), id);
        assert!(!saved.paused);
        assert!(saved.complete());
        client.shutdown().await;
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
        let room = saved.rumor.uniq_id();
        assert_eq!(first.messages(room).await.unwrap().len(), 1);
        let other_account = queue(first_client.clone(), recipient.public_key(), dir.path());
        assert!(other_account.messages(room).await.unwrap().is_empty());
        assert!(
            first_client
                .database()
                .query(Filter::new().kind(Kind::ApplicationSpecificData))
                .await
                .unwrap()
                .is_empty(),
            "queued plaintext must not enter the shared relay cache"
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
    #[tokio::test]
    async fn queue_error_is_reported_until_storage_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let owner = Keys::generate();
        let client = Client::builder().database(nostr_memory::MemoryDatabase::unbounded()).build();
        let account_dir = dir.path().join(owner.public_key().to_hex());
        std::fs::write(&account_dir, "not a directory").unwrap();
        let (mut queue, wake) = OutgoingQueue::new(client.clone(), owner.public_key());
        queue.root = dir.path().to_owned();
        let (tx, rx) = flume::unbounded();
        let worker = queue.clone();
        let task = tokio::spawn(async move { worker.run(UniversalSigner::new(owner), wake, tx).await });
        let error = tokio::time::timeout(Duration::from_secs(5), rx.recv_async()).await.unwrap().unwrap();
        assert!(matches!(error, Signal::OutgoingError(_)));
        std::fs::remove_file(&account_dir).unwrap();
        std::fs::create_dir(&account_dir).unwrap();
        queue.retry();
        let recovered = tokio::time::timeout(Duration::from_secs(5), rx.recv_async()).await.unwrap().unwrap();
        assert!(matches!(recovered, Signal::OutgoingRecovered));
        queue.stop();
        task.await.unwrap().unwrap();
        client.shutdown().await;
    }

    #[derive(Debug)]
    struct PausingSigner {
        signer: UniversalSigner,
        started: flume::Sender<()>,
        resume: flume::Receiver<()>,
    }
    impl AsyncGetPublicKey for PausingSigner {
        type Error = <UniversalSigner as AsyncGetPublicKey>::Error;
        fn get_public_key_async(
            &self,
        ) -> futures::future::BoxFuture<'_, std::result::Result<PublicKey, Self::Error>> {
            self.signer.get_public_key_async()
        }
    }
    impl AsyncSignEvent for PausingSigner {
        type Error = <UniversalSigner as AsyncSignEvent>::Error;
        fn sign_event_async(
            &self,
            event: UnsignedEvent,
        ) -> futures::future::BoxFuture<'_, std::result::Result<Event, Self::Error>> {
            Box::pin(async move {
                self.started.send_async(()).await.unwrap();
                self.resume.recv_async().await.unwrap();
                self.signer.sign_event_async(event).await
            })
        }
    }
    impl AsyncNip44 for PausingSigner {
        type Error = <UniversalSigner as AsyncNip44>::Error;
        fn nip44_encrypt_async<'a>(
            &'a self,
            key: &'a PublicKey,
            content: &'a str,
        ) -> futures::future::BoxFuture<'a, std::result::Result<String, Self::Error>> {
            self.signer.nip44_encrypt_async(key, content)
        }
        fn nip44_decrypt_async<'a>(
            &'a self,
            key: &'a PublicKey,
            content: &'a str,
        ) -> futures::future::BoxFuture<'a, std::result::Result<String, Self::Error>> {
            self.signer.nip44_decrypt_async(key, content)
        }
    }

    #[tokio::test]
    async fn account_stop_during_preparation_leaves_intent_without_publishing() {
        tokio::time::timeout(Duration::from_secs(5), async {
            let dir = tempfile::tempdir().unwrap();
            let owner = Keys::generate();
            let client = Client::builder()
                .database(nostr_memory::MemoryDatabase::unbounded())
                .build();
            let queue = queue(client.clone(), owner.public_key(), dir.path());
            let original = message(&owner, &[Keys::generate().public_key()]);
            queue.enqueue(original.clone()).await.unwrap();
            let (started_tx, started_rx) = flume::bounded(1);
            let (resume_tx, resume_rx) = flume::bounded(1);
            let signer = UniversalSigner::new(PausingSigner {
                signer: UniversalSigner::new(owner.clone()),
                started: started_tx,
                resume: resume_rx,
            });
            let worker_queue = queue.clone();
            let (signals, _rx) = flume::unbounded();
            let worker = tokio::spawn(async move {
                worker_queue
                    .process(&signer, &signals, &mut BTreeMap::new())
                    .await
            });
            started_rx.recv_async().await.unwrap();
            queue.stop();
            resume_tx.send_async(()).await.unwrap();
            assert!(
                worker
                    .await
                    .unwrap()
                    .unwrap_err()
                    .to_string()
                    .contains("inactive")
            );
            assert!(queue.enqueue(original).await.is_err());
            let retained = load(dir.path(), owner.public_key()).await.unwrap();
            assert_eq!(retained.len(), 1);
            assert!(
                retained[0]
                    .destinations
                    .iter()
                    .all(|d| d.wrap.is_none() && d.accepted.is_empty())
            );
            client.shutdown().await;
        })
        .await
        .expect("stopped preparation must not continue delivering");
    }

    #[tokio::test]
    async fn outgoing_signer_snapshot_does_not_follow_account_switches() {
        let original = Keys::generate();
        let replacement = Keys::generate();
        let signer = UniversalSigner::new(original.clone());
        let snapshot = signer.snapshot();
        signer.swap_inner(replacement.clone());
        assert_eq!(
            signer.get_public_key_async().await.unwrap(),
            replacement.public_key()
        );
        assert_eq!(
            snapshot.get_public_key_async().await.unwrap(),
            original.public_key()
        );
        let unsigned =
            EventBuilder::new(Kind::Seal, "test").finalize_unsigned(original.public_key());
        let event = snapshot.sign_event_async(unsigned).await.unwrap();
        assert_eq!(event.pubkey, original.public_key());
        event.verify().unwrap();
    }
}
