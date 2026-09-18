//! Local-first NIP-37 message drafts. The relay copy contains an unsigned kind-14
//! rumor encrypted to ourselves; it is never sent to the conversation's recipients.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Result, ensure};
use common::EventExt;
use nostr_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use state::UniversalSigner;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub text: String,
    pub replies: BTreeSet<EventId>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Local {
    snapshot: Snapshot,
    members: Vec<PublicKey>,
    identifier: String,
    pending: bool,
    revision: u64,
    newest: Option<(Timestamp, EventId)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Seen {
    stamp: (Timestamp, EventId),
    room: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Data {
    rooms: BTreeMap<u64, Local>,
    seen: BTreeMap<String, Seen>,
}

#[derive(Clone, Debug)]
pub(super) struct DraftStore {
    owner: PublicKey,
    root: PathBuf,
    data: Arc<RwLock<Data>>,
    edited_at: Arc<RwLock<BTreeMap<u64, Instant>>>,
    active: Arc<AtomicBool>,
    pub paused: Arc<AtomicBool>,
    wake: flume::Sender<()>,
    relay_list: Arc<RwLock<Option<(EventId, Vec<RelayUrl>)>>>,
}

fn newer(a: (Timestamp, EventId), b: (Timestamp, EventId)) -> bool {
    a.0 > b.0 || (a.0 == b.0 && a.1 < b.1)
}

fn path(root: &Path, owner: PublicKey, room: u64) -> PathBuf {
    root.join("drafts")
        .join(owner.to_hex())
        .join(format!("{room}.json"))
}

impl DraftStore {
    pub fn open(root: &Path, owner: PublicKey) -> Result<(Self, flume::Receiver<()>)> {
        let mut data: Data = common::persistence::global()
            .load(&root.join("draft-sync").join(format!("{owner}.json")))?;
        for (room, local) in &mut data.rooms {
            let disk: Snapshot = common::persistence::global().load(&path(root, owner, *room))?;
            if local.pending {
                common::persistence::global()
                    .save(&path(root, owner, *room), local.snapshot.clone())?;
            } else if disk != local.snapshot {
                local.snapshot = disk;
                local.pending = true;
                local.revision += 1;
            }
        }
        let (wake, receiver) = flume::bounded(1);
        Ok((
            Self {
                owner,
                root: root.into(),
                data: Arc::new(RwLock::new(data)),
                edited_at: Arc::default(),
                active: Arc::new(AtomicBool::new(true)),
                paused: Arc::default(),
                wake,
                relay_list: Arc::default(),
            },
            receiver,
        ))
    }

    pub fn stop(&self) {
        let _guard = self.data.write().unwrap();
        self.active.store(false, Ordering::SeqCst);
    }

    fn ensure_active(&self) -> Result<()> {
        ensure!(self.active.load(Ordering::SeqCst), "Draft account changed");
        Ok(())
    }

    fn persist(&self, data: &Data) -> Result<()> {
        self.ensure_active()?;
        common::persistence::global().save(
            &self
                .root
                .join("draft-sync")
                .join(format!("{}.json", self.owner)),
            data.clone(),
        )?;
        Ok(())
    }

    pub fn snapshot(&self, room: u64) -> Option<Snapshot> {
        self.data
            .read()
            .unwrap()
            .rooms
            .get(&room)
            .map(|local| local.snapshot.clone())
    }

    pub fn indicators(&self) -> Vec<(u64, bool)> {
        self.data.read().unwrap().rooms.iter()
            .map(|(room, local)| (*room, !local.snapshot.text.trim().is_empty())).collect()
    }

    pub fn rooms(&self) -> Vec<(Vec<PublicKey>, Snapshot)> {
        self.data
            .read()
            .unwrap()
            .rooms
            .values()
            .map(|local| (local.members.clone(), local.snapshot.clone()))
            .collect()
    }

    /// Called after the local composer save. Keep unsent edits durable even when
    /// offline or when the signer declines a background request.
    pub fn stage(&self, members: &[PublicKey], snapshot: Snapshot) -> Result<()> {
        let room = crate::Room::new(self.owner, members.iter().copied()).id;
        let mut data = self.data.write().unwrap();
        self.stage_locked(&mut data, room, members, snapshot)
    }

    fn stage_locked(
        &self,
        data: &mut Data,
        room: u64,
        members: &[PublicKey],
        snapshot: Snapshot,
    ) -> Result<()> {
        self.ensure_active()?;
        if data
            .rooms
            .get(&room)
            .is_some_and(|local| local.snapshot == snapshot)
        {
            return Ok(());
        }
        // An untouched empty composer must not erase a remote draft before discovery.
        if !data.rooms.contains_key(&room) && snapshot == Snapshot::default() {
            return Ok(());
        }
        let mut updated = data.clone();
        let local = updated.rooms.entry(room).or_insert_with(|| Local {
            snapshot: Snapshot::default(),
            members: members.to_vec(),
            identifier: format!("goop-{}", Keys::generate().public_key()),
            pending: false,
            revision: 0,
            newest: None,
        });
        local.snapshot = snapshot.clone();
        local.pending = true;
        local.revision += 1;
        self.persist(&updated)?;
        common::persistence::global().save(&path(&self.root, self.owner, room), snapshot)?;
        *data = updated;
        self.edited_at.write().unwrap().insert(room, Instant::now());
        let _ = self.wake.try_send(());
        Ok(())
    }

    pub fn import_local(&self, members: &[PublicKey]) -> Result<()> {
        let room = crate::Room::new(self.owner, members.iter().copied()).id;
        // Only migrate once. Holding the lock until the disk read prevents a
        // concurrent incoming draft from replacing an older local-only draft.
        let mut data = self.data.write().unwrap();
        if data.rooms.contains_key(&room) {
            return Ok(());
        }
        let snapshot = common::persistence::global().load(&path(&self.root, self.owner, room))?;
        if snapshot != Snapshot::default() {
            self.stage_locked(&mut data, room, members, snapshot)?;
        }
        Ok(())
    }

    pub fn retry(&self) {
        self.paused.store(false, Ordering::SeqCst);
        let _ = self.wake.try_send(());
    }

    /// Read the same encrypted kind-10013 list as Amethyst. Never fall back to
    /// publishing draft metadata on a user's public or messaging relays.
    async fn relays(&self, client: &Client, signer: &UniversalSigner) -> Result<Vec<RelayUrl>> {
        let latest = state::private_storage::latest(client, self.owner).await?;
        let mut relays = BTreeSet::new();
        if let Some(event) = latest {
            event.verify()?;
            let cached = self.relay_list.read().unwrap().clone();
            if let Some((_, urls)) = cached.filter(|(id, _)| *id == event.id) {
                relays.extend(urls);
            } else {
                relays.extend(state::private_storage::decode(&event, self.owner, signer).await?);
                *self.relay_list.write().unwrap() =
                    Some((event.id, relays.iter().cloned().collect()));
            }
        }
        Ok(relays.into_iter().collect())
    }

    pub async fn sync(&self, client: &Client, signer: &UniversalSigner) -> Result<bool> {
        self.ensure_active()?;
        ensure!(
            signer.get_public_key_async().await? == self.owner,
            "Draft signer changed"
        );
        let relays = self.relays(client, signer).await?;
        if relays.is_empty() {
            return Ok(false);
        }
        for url in &relays {
            client
                .add_relay(url)
                .capabilities(RelayCapabilities::GOSSIP)
                .and_connect()
                .await?;
        }
        let filter = Filter::new().author(self.owner).kind(Kind::Custom(31234));
        let targets = ReqTarget::manual(relays.iter().map(|url| (url, vec![filter.clone()])));
        let remote = client
            .fetch_events(targets)
            .timeout(Duration::from_secs(10))
            .await?;
        let mut events: Vec<_> = remote.into_iter().collect();
        // Oldest first lets a deletion find the room from a previous event.
        events.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        let mut receive_error = None;
        for event in events {
            if let Err(error) = self.receive(&event, signer).await {
                // A broken draft must not hold up the rest of the queue. Stop on
                // signer failures so refusal never causes repeated prompts.
                if state::SignerFailure::classify(error.as_ref()) != state::SignerFailure::Other {
                    return Err(error);
                }
                receive_error = Some(error);
            }
        }
        let pending: Vec<_> = self
            .data
            .read()
            .unwrap()
            .rooms
            .iter()
            .filter(|(_, local)| local.pending)
            .map(|(room, local)| (*room, local.clone()))
            .collect();
        for (room, local) in pending {
            if self
                .edited_at
                .read()
                .unwrap()
                .get(&room)
                .is_some_and(|time| time.elapsed() < Duration::from_secs(3))
            {
                continue;
            }
            self.publish(client, signer, &relays, room, local).await?;
        }
        if let Some(error) = receive_error {
            return Err(error);
        }
        Ok(true)
    }

    async fn receive(&self, event: &Event, signer: &UniversalSigner) -> Result<()> {
        ensure!(
            event.pubkey == self.owner && event.kind == Kind::Custom(31234),
            "Invalid draft author or kind"
        );
        event.verify()?;
        let Some(identifier) = event.tags.identifier() else {
            return Ok(());
        };
        let stamp = (event.created_at, event.id);
        if self
            .data
            .read()
            .unwrap()
            .seen
            .get(&identifier)
            .is_some_and(|seen| !newer(stamp, seen.stamp))
        {
            return Ok(());
        }
        // Ignore unsupported kinds without asking the signer to decrypt them.
        if !event.content.is_empty()
            && !event.tags.iter().any(|tag| {
                tag.as_slice().first().is_some_and(|s| s == "k") && tag.content() == Some("14")
            })
        {
            return Ok(());
        }
        if event
            .tags
            .expiration()
            .is_some_and(|expiration| expiration <= Timestamp::now())
        {
            return Ok(());
        }
        let decoded = if event.content.is_empty() {
            None
        } else {
            let plaintext = signer
                .nip44_decrypt_async(&self.owner, &event.content)
                .await?;
            Some(decode(&plaintext, self.owner)?)
        };
        let mut data = self.data.write().unwrap();
        self.ensure_active()?;
        let mut updated = data.clone();
        let room = decoded
            .as_ref()
            .map(|(rumor, _)| rumor.uniq_id())
            .or_else(|| updated.seen.get(&identifier).and_then(|seen| seen.room));
        updated
            .seen
            .insert(identifier.clone(), Seen { stamp, room });
        if let Some(room) = room {
            let existing = updated.rooms.get(&room);
            if existing.is_none_or(|local| {
                !local.pending && local.newest.is_none_or(|old| newer(stamp, old))
            }) {
                let (members, snapshot) = match decoded {
                    Some((rumor, snapshot)) => (rumor.extract_public_keys(), snapshot),
                    None => (
                        existing
                            .map(|local| local.members.clone())
                            .unwrap_or_default(),
                        Snapshot::default(),
                    ),
                };
                // Preserve a pre-sync local draft (including drafts from older Goop).
                let disk: Snapshot =
                    common::persistence::global().load(&path(&self.root, self.owner, room))?;
                if existing.is_none() && disk != Snapshot::default() {
                    updated.rooms.insert(
                        room,
                        Local {
                            snapshot: disk,
                            members,
                            identifier,
                            pending: true,
                            revision: 1,
                            newest: Some(stamp),
                        },
                    );
                } else {
                    updated.rooms.insert(
                        room,
                        Local {
                            snapshot: snapshot.clone(),
                            members,
                            identifier,
                            pending: false,
                            revision: 0,
                            newest: Some(stamp),
                        },
                    );
                    common::persistence::global()
                        .save(&path(&self.root, self.owner, room), snapshot)?;
                }
            }
        }
        self.persist(&updated)?;
        *data = updated;
        Ok(())
    }

    async fn publish(
        &self,
        client: &Client,
        signer: &UniversalSigner,
        relays: &[RelayUrl],
        room: u64,
        local: Local,
    ) -> Result<()> {
        let now = Timestamp::now();
        let previous = self
            .data
            .read()
            .unwrap()
            .seen
            .get(&local.identifier)
            .map(|seen| seen.stamp);
        if previous.is_some_and(|stamp| now <= stamp.0) {
            return Ok(());
        }
        let content = if local.snapshot.text.trim().is_empty() {
            String::new()
        } else {
            let rumor = encode(self.owner, &local.members, &local.snapshot);
            signer
                .nip44_encrypt_async(&self.owner, &rumor.as_json())
                .await?
        };
        let event = EventBuilder::new(Kind::Custom(31234), content)
            .tag(Tag::identifier(&local.identifier))
            .tag(Tag::parse(["k", "14"])?)
            .tag(Tag::expiration(Timestamp::from(
                now.as_secs() + 90 * 24 * 60 * 60,
            )))
            .custom_created_at(now)
            .finalize_async(signer)
            .await?;
        self.ensure_active()?;
        // Don't publish a snapshot superseded while waiting for signer approval.
        if self
            .data
            .read()
            .unwrap()
            .rooms
            .get(&room)
            .is_none_or(|current| current.revision != local.revision)
        {
            return Ok(());
        }
        let output = client
            .send_event(&event)
            .to(relays.iter())
            .ack_policy(AckPolicy::all())
            .await?;
        ensure!(
            output.success.values().any(|status| status.is_ack()),
            "No private-storage relay accepted the draft"
        );
        let mut data = self.data.write().unwrap();
        self.ensure_active()?;
        let mut updated = data.clone();
        let stamp = (event.created_at, event.id);
        updated.seen.insert(
            local.identifier,
            Seen {
                stamp,
                room: Some(room),
            },
        );
        if let Some(current) = updated.rooms.get_mut(&room) {
            current.newest = Some(stamp);
            if current.revision == local.revision {
                current.pending = false;
            }
        }
        self.persist(&updated)?;
        *data = updated;
        Ok(())
    }
}

fn encode(owner: PublicKey, members: &[PublicKey], snapshot: &Snapshot) -> UnsignedEvent {
    let mut recipients: BTreeSet<_> = members
        .iter()
        .copied()
        .filter(|key| *key != owner)
        .collect();
    if recipients.is_empty() {
        recipients.insert(owner);
    }
    let mut rumor = EventBuilder::new(Kind::PrivateDirectMessage, snapshot.text.clone())
        .tags(recipients.into_iter().map(Tag::public_key))
        .tags(snapshot.replies.iter().map(|id| Tag::event(*id)))
        .finalize_unsigned(owner);
    // Amethyst restores the unsigned rumor as an Event and uses its ID.
    rumor.ensure_id();
    rumor
}

fn decode(json: &str, owner: PublicKey) -> Result<(UnsignedEvent, Snapshot)> {
    let rumor = UnsignedEvent::from_json(json)?;
    ensure!(
        rumor.pubkey == owner && rumor.kind == Kind::PrivateDirectMessage,
        "Invalid message draft"
    );
    ensure!(
        rumor.tags.public_keys().next().is_some(),
        "Draft has no recipients"
    );
    let snapshot = Snapshot {
        text: rumor.content.clone(),
        replies: rumor.tags.event_ids().collect(),
    };
    Ok((rumor, snapshot))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_state_dir::StateDir;

    fn text(value: &str) -> Snapshot {
        Snapshot {
            text: value.into(),
            ..Snapshot::default()
        }
    }

    fn ready(store: &DraftStore) {
        store.edited_at.write().unwrap().clear();
    }

    async fn wrapped(
        keys: &Keys,
        identifier: &str,
        members: &[PublicKey],
        snapshot: Snapshot,
        seconds: u64,
    ) -> Event {
        let content = if snapshot == Snapshot::default() {
            String::new()
        } else {
            keys.nip44_encrypt_async(
                &keys.public_key(),
                &encode(keys.public_key(), members, &snapshot).as_json(),
            )
            .await
            .unwrap()
        };
        EventBuilder::new(Kind::Custom(31234), content)
            .tag(Tag::identifier(identifier))
            .tag(Tag::parse(["k", "14"]).unwrap())
            .custom_created_at(Timestamp::from(seconds))
            .finalize(keys)
            .unwrap()
    }

    #[test]
    fn rumor_round_trip_preserves_recipients_replies_and_whitespace() {
        let owner = Keys::generate().public_key();
        let peer = Keys::generate().public_key();
        let third = Keys::generate().public_key();
        let snapshot = Snapshot {
            text: "  hello 🧡\n\n".into(),
            replies: [EventId::from_byte_array([1; 32])].into(),
        };
        for members in [vec![owner], vec![peer, owner], vec![third, peer, owner]] {
            let rumor = encode(owner, &members, &snapshot);
            let (restored, result) = decode(&rumor.as_json(), owner).unwrap();
            assert_eq!(result, snapshot);
            assert_eq!(restored.uniq_id(), crate::Room::new(owner, members).id);
            assert!(rumor.id.is_some());
            assert!(!rumor.as_json().contains("\"sig\""));
            // Amethyst serializes its unsigned events with an empty sig field.
            let mut amethyst = serde_json::to_value(&rumor).unwrap();
            amethyst["sig"] = "".into();
            assert_eq!(decode(&amethyst.to_string(), owner).unwrap().1, snapshot);
        }
        assert!(decode(&encode(owner, &[peer], &snapshot).as_json(), peer).is_err());
    }

    #[tokio::test]
    async fn incoming_amethyst_style_draft_updates_and_deletes_without_resurrection() {
        let root = StateDir::new();
        let keys = Keys::generate();
        let owner = keys.public_key();
        let peer = Keys::generate().public_key();
        let signer = UniversalSigner::new(keys.clone());
        let (store, _) = DraftStore::open(root.path(), owner).unwrap();
        let room = crate::Room::new(owner, [peer]).id;
        let initial = wrapped(&keys, "amethyst-random-uuid", &[peer], text("hello"), 100).await;
        store.receive(&initial, &signer).await.unwrap();
        assert_eq!(store.snapshot(room), Some(text("hello")));
        let edit = wrapped(
            &keys,
            "amethyst-random-uuid",
            &[peer],
            text("hello again"),
            101,
        )
        .await;
        store.receive(&edit, &signer).await.unwrap();
        assert_eq!(store.snapshot(room), Some(text("hello again")));
        let deletion = wrapped(
            &keys,
            "amethyst-random-uuid",
            &[peer],
            Snapshot::default(),
            102,
        )
        .await;
        store.receive(&deletion, &signer).await.unwrap();
        store.receive(&initial, &signer).await.unwrap();
        assert_eq!(store.snapshot(room), Some(Snapshot::default()));
        // An older independent draft for the same conversation cannot resurrect it.
        let old = wrapped(&keys, "older-draft", &[peer], text("old"), 99).await;
        store.receive(&old, &signer).await.unwrap();
        assert_eq!(store.snapshot(room), Some(Snapshot::default()));
        let (restarted, _) = DraftStore::open(root.path(), owner).unwrap();
        assert_eq!(restarted.snapshot(room), Some(Snapshot::default()));
        store.stop();
        assert!(store.stage(&[peer], text("after logout")).is_err());
    }

    #[tokio::test]
    async fn incoming_drafts_and_deletions_do_not_overwrite_local_edits() {
        let root = StateDir::new();
        let keys = Keys::generate();
        let peer = Keys::generate().public_key();
        let signer = UniversalSigner::new(keys.clone());
        let (store, _) = DraftStore::open(root.path(), keys.public_key()).unwrap();
        let room = crate::Room::new(keys.public_key(), [peer]).id;
        let initial = wrapped(&keys, "shared", &[peer], text("remote"), 100).await;
        store.receive(&initial, &signer).await.unwrap();
        store.stage(&[peer], text("typing locally")).unwrap();
        let deletion = wrapped(&keys, "shared", &[peer], Snapshot::default(), 101).await;
        store.receive(&deletion, &signer).await.unwrap();
        assert_eq!(store.snapshot(room), Some(text("typing locally")));
        let (restarted, _) = DraftStore::open(root.path(), keys.public_key()).unwrap();
        assert!(restarted.data.read().unwrap().rooms[&room].pending);
        assert_eq!(restarted.snapshot(room), Some(text("typing locally")));
    }

    #[tokio::test]
    async fn legacy_local_draft_survives_remote_discovery() {
        let root = StateDir::new();
        let keys = Keys::generate();
        let peer = Keys::generate().public_key();
        let room = crate::Room::new(keys.public_key(), [peer]).id;
        common::persistence::global()
            .save(
                &path(root.path(), keys.public_key(), room),
                text("legacy input"),
            )
            .unwrap();
        let (store, _) = DraftStore::open(root.path(), keys.public_key()).unwrap();
        store
            .receive(
                &wrapped(&keys, "amethyst", &[peer], text("remote"), 100).await,
                &UniversalSigner::new(keys),
            )
            .await
            .unwrap();
        assert_eq!(store.snapshot(room), Some(text("legacy input")));
        assert!(store.data.read().unwrap().rooms[&room].pending);
    }

    #[tokio::test]
    async fn sync_uses_private_relay_list_and_preserves_refused_sends() {
        use nostr_sdk::local_relay::MockRelay;
        tokio::time::timeout(Duration::from_secs(25), async {
            let public = MockRelay::run().await.unwrap();
            let private = MockRelay::run().await.unwrap();
            let keys = Keys::generate();
            let owner = keys.public_key();
            let peer = Keys::generate().public_key();
            let signer = UniversalSigner::new(keys.clone());
            let client = Client::builder()
                .database(nostr_memory::MemoryDatabase::unbounded())
                .build();
            client
                .add_relay(public.url().await)
                .and_connect()
                .await
                .unwrap();
            let root = StateDir::new();
            let (store, _) = DraftStore::open(root.path(), owner).unwrap();
            store.stage(&[peer], text("private message")).unwrap();
            ready(&store);
            // No private list means no publication, even on a connected local relay.
            assert!(!store.sync(&client, &signer).await.unwrap());
            let tags = serde_json::json!([["relay", private.url().await]]).to_string();
            let content = keys.nip44_encrypt_async(&owner, &tags).await.unwrap();
            public
                .add_event(
                    EventBuilder::new(Kind::Custom(10013), content)
                        .finalize(&keys)
                        .unwrap(),
                )
                .await
                .unwrap();
            let refusing = crate::test_signer::RefusingSigner::new(keys.clone());
            assert!(
                store
                    .sync(&client, &UniversalSigner::new(refusing.clone()))
                    .await
                    .is_err()
            );
            let room = crate::Room::new(owner, [peer]).id;
            assert!(store.data.read().unwrap().rooms[&room].pending);
            assert!(store.sync(&client, &signer).await.unwrap());
            assert!(!store.data.read().unwrap().rooms[&room].pending);
            let filter = Filter::new().author(owner).kind(Kind::Custom(31234));
            let exposed = client
                .fetch_events(ReqTarget::single(public.url().await, [filter.clone()]))
                .timeout(Duration::from_secs(2))
                .await
                .unwrap();
            assert!(
                exposed.is_empty(),
                "draft metadata must not leak onto public relays"
            );
            let events = client
                .fetch_events(ReqTarget::single(private.url().await, [filter]))
                .timeout(Duration::from_secs(2))
                .await
                .unwrap();
            assert_eq!(events.len(), 1);
            let event = events.iter().next().unwrap();
            assert!(!event.content.contains("private message"));
            let content = keys
                .nip44_decrypt_async(&owner, &event.content)
                .await
                .unwrap();
            assert_eq!(decode(&content, owner).unwrap().1, text("private message"));
            // A second device restores the same draft.
            let other_root = StateDir::new();
            let (other, _) = DraftStore::open(other_root.path(), owner).unwrap();
            other.sync(&client, &signer).await.unwrap();
            assert_eq!(other.snapshot(room), Some(text("private message")));
            // Clearing/sending replaces the same draft with a tombstone.
            tokio::time::sleep(Duration::from_millis(1100)).await;
            store.stage(&[peer], Snapshot::default()).unwrap();
            ready(&store);
            store.sync(&client, &signer).await.unwrap();
            other.sync(&client, &signer).await.unwrap();
            assert_eq!(other.snapshot(room), Some(Snapshot::default()));
            assert!(!store.data.read().unwrap().rooms[&room].pending);
            client.shutdown().await;
        })
        .await
        .unwrap();
    }
}
