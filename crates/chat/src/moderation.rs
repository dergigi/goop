//! Account-scoped timed notification mutes and private NIP-51 user blocks.
use anyhow::{Result, ensure};
use futures::StreamExt;
use nostr_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use state::UniversalSigner;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Data {
    public: Vec<Vec<String>>,
    private: Vec<Vec<String>>,
    newest: Option<(Timestamp, EventId)>,
    pending: BTreeMap<PublicKey, bool>,
    mutes: BTreeMap<PublicKey, u64>,
    #[serde(skip)]
    blocked_index: Arc<BTreeSet<PublicKey>>,
}
impl Data {
    fn reindex(&mut self, owner: PublicKey) {
        let data = &*self;
        let mut keys: BTreeSet<_> = data
            .public
            .iter()
            .chain(&data.private)
            .filter_map(|tag| key(tag))
            .collect();
        for (key, value) in &data.pending {
            if *value {
                keys.insert(*key);
            } else {
                keys.remove(key);
            }
        }
        keys.remove(&owner);
        self.blocked_index = Arc::new(keys);
    }
}
fn key(tag: &[String]) -> Option<PublicKey> {
    (tag.first()?.as_str() == "p")
        .then(|| PublicKey::from_hex(tag.get(1)?).ok())
        .flatten()
}
fn edit(
    public: &mut Vec<Vec<String>>,
    private: &mut Vec<Vec<String>>,
    edits: &BTreeMap<PublicKey, bool>,
) {
    public.retain(|tag| key(tag).is_none_or(|key| !edits.contains_key(&key)));
    private.retain(|tag| key(tag).is_none_or(|key| !edits.contains_key(&key)));
    for (key, blocked) in edits {
        if *blocked {
            private.push(vec!["p".into(), key.to_hex()]);
        }
    }
}
pub(super) fn hides_room(blocked: &BTreeSet<PublicKey>, members: &[PublicKey]) -> bool {
    members.len() <= 2 && members.iter().any(|key| blocked.contains(key))
}

#[derive(Clone, Debug)]
pub(super) struct ModerationStore {
    owner: PublicKey,
    path: PathBuf,
    data: Arc<RwLock<Data>>,
    active: Arc<AtomicBool>,
    pub paused: Arc<AtomicBool>,
    wake: flume::Sender<()>,
}
impl ModerationStore {
    pub fn open(root: &Path, owner: PublicKey) -> Result<(Self, flume::Receiver<()>)> {
        let path = root
            .join("goop-moderation-v1")
            .join(format!("{owner}.json"));
        let mut data: Data = common::persistence::global().load(&path)?;
        data.reindex(owner);
        let (wake, receiver) = flume::bounded(1);
        Ok((
            Self {
                owner,
                path,
                data: Arc::new(RwLock::new(data)),
                active: Arc::new(AtomicBool::new(true)),
                paused: Arc::default(),
                wake,
            },
            receiver,
        ))
    }
    pub fn stop(&self) {
        // Serialize stop with persist's active check and snapshot submission.
        let _guard = self.data.write().unwrap();
        self.active.store(false, Ordering::SeqCst);
    }
    fn ensure_active(&self) -> Result<()> {
        ensure!(
            self.active.load(Ordering::SeqCst),
            "Blocked-list account changed"
        );
        Ok(())
    }
    fn persist(&self, data: &Data) -> Result<()> {
        self.ensure_active()?;
        common::persistence::global().save(&self.path, data.clone())?;
        Ok(())
    }
    pub fn blocked(&self) -> BTreeSet<PublicKey> {
        (*self.blocked_snapshot()).clone()
    }
    pub fn blocked_snapshot(&self) -> Arc<BTreeSet<PublicKey>> {
        self.data.read().unwrap().blocked_index.clone()
    }
    pub fn is_blocked(&self, key: PublicKey) -> bool {
        self.data.read().unwrap().blocked_index.contains(&key)
    }
    pub fn pending(&self) -> bool {
        !self.data.read().unwrap().pending.is_empty()
    }
    pub fn muted(&self, key: PublicKey) -> bool {
        self.data
            .read()
            .unwrap()
            .mutes
            .get(&key)
            .is_some_and(|until| *until > Timestamp::now().as_secs())
    }
    pub fn mute(&self, key: PublicKey, seconds: Option<u64>) -> Result<()> {
        self.ensure_active()?;
        let mut data = self.data.write().unwrap();
        let mut updated = data.clone();
        match seconds {
            Some(seconds) => {
                updated
                    .mutes
                    .insert(key, Timestamp::now().as_secs().saturating_add(seconds));
            }
            None => {
                updated.mutes.remove(&key);
            }
        }
        updated.reindex(self.owner);
        self.persist(&updated)?;
        *data = updated;
        Ok(())
    }
    pub fn block(&self, key: PublicKey, value: bool) -> Result<()> {
        self.ensure_active()?;
        ensure!(key != self.owner, "You cannot block yourself");
        let mut data = self.data.write().unwrap();
        let mut updated = data.clone();
        updated.pending.insert(key, value);
        updated.reindex(self.owner);
        self.persist(&updated)?;
        *data = updated;
        self.retry();
        Ok(())
    }
    pub fn retry(&self) {
        self.paused.store(false, Ordering::SeqCst);
        let _ = self.wake.try_send(());
    }
    pub async fn sync(&self, client: &Client, signer: &UniversalSigner) -> Result<()> {
        self.ensure_active()?;
        let filter = Filter::new()
            .kind(Kind::Custom(10000))
            .author(self.owner)
            .limit(1);
        // fetch_events intentionally swallows per-relay read failures. A replacement
        // list must not turn a denied/timed-out read into an empty successful read.
        let mut stream = client
            .stream_events(filter.clone())
            .timeout(std::time::Duration::from_secs(10))
            .await?;
        let mut remote = Vec::new();
        while let Some((url, result)) = stream.next().await {
            remote.push(result.map_err(|error| anyhow::anyhow!("Could not read the block list from {url}: {error}. Your existing list has not been replaced."))?);
        }
        let cached = client.database().query(filter).await?;
        let latest = remote.into_iter().chain(cached).max_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        if let Some(event) = latest {
            let known = self.data.read().unwrap().newest;
            let stamp = (event.created_at, event.id);
            if known.is_none_or(|old| stamp.0 > old.0 || (stamp.0 == old.0 && stamp.1 < old.1)) {
                event.verify()?;
                // Never replace an unreadable legacy list or silently discard its private entries.
                ensure!(
                    !event.content.contains("?iv="),
                    "Your block list uses legacy encryption. Update it in a NIP-44-compatible client first; Goop has preserved the original list."
                );
                let plaintext = if event.content.trim().is_empty() {
                    String::new()
                } else {
                    signer
                        .nip44_decrypt_async(&self.owner, &event.content)
                        .await?
                };
                let private = if plaintext.trim().is_empty() {
                    vec![]
                } else {
                    serde_json::from_str(&plaintext)?
                };
                self.ensure_active()?;
                let mut data = self.data.write().unwrap();
                let mut updated = data.clone();
                updated.public = event
                    .tags
                    .iter()
                    .map(|tag| tag.as_slice().to_vec())
                    .collect();
                updated.private = private;
                updated.newest = Some(stamp);
                updated.reindex(self.owner);
                self.persist(&updated)?;
                *data = updated;
            }
        }
        let snapshot = self.data.read().unwrap().clone();
        if snapshot.pending.is_empty() {
            return Ok(());
        }
        ensure!(
            signer.get_public_key_async().await? == self.owner,
            "Block-list signer changed"
        );
        let (mut public, mut private) = (snapshot.public, snapshot.private);
        edit(&mut public, &mut private, &snapshot.pending);
        let content = signer
            .nip44_encrypt_async(&self.owner, &serde_json::to_string(&private)?)
            .await?;
        let now = Timestamp::now();
        ensure!(
            snapshot.newest.is_none_or(|(previous, _)| now > previous),
            "Block-list update is waiting for the next timestamp"
        );
        let tags = public
            .iter()
            .map(Tag::parse)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let event = EventBuilder::new(Kind::Custom(10000), content)
            .tags(tags)
            .custom_created_at(now)
            .finalize_async(signer)
            .await?;
        event.verify()?;
        ensure!(event.pubkey == self.owner, "Block-list signer changed");
        self.ensure_active()?;
        let output = client
            .send_event(&event)
            .ack_policy(AckPolicy::all())
            .await?;
        ensure!(
            output.success.values().any(|status| status.is_ack()),
            "No relay accepted your blocked-user list. Local blocking is active; retry to sync."
        );
        self.ensure_active()?;
        let mut data = self.data.write().unwrap();
        let mut updated = data.clone();
        updated.public = public;
        updated.private = private;
        updated.newest = Some((event.created_at, event.id));
        for (key, value) in snapshot.pending {
            if updated.pending.get(&key) == Some(&value) {
                updated.pending.remove(&key);
            }
        }
        updated.reindex(self.owner);
        self.persist(&updated)?;
        *data = updated;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn block_index_snapshots_survive_updates_and_rebuild_on_restart() {
        let root = crate::test_state_dir::StateDir::new();
        let owner = Keys::generate().public_key();
        let peer = Keys::generate().public_key();
        let (store, _) = ModerationStore::open(root.path(), owner).unwrap();
        store.block(peer, true).unwrap();
        let blocked = store.blocked_snapshot();
        assert!(Arc::ptr_eq(&blocked, &store.blocked_snapshot()));
        assert!(store.is_blocked(peer));
        common::persistence::global().flush_blocking().unwrap();
        let (restarted, _) = ModerationStore::open(root.path(), owner).unwrap();
        assert!(restarted.is_blocked(peer));
        store.block(peer, false).unwrap();
        assert!(!store.is_blocked(peer));
        assert!(blocked.contains(&peer));
        assert!(!store.is_blocked(owner));
    }

    #[test]
    fn preserves_other_entries_and_moves_blocks_to_private() {
        let target = Keys::generate().public_key();
        let other = Keys::generate().public_key();
        let mut public = vec![
            vec!["p".into(), target.to_hex()],
            vec!["word".into(), "spam".into()],
        ];
        let mut private = vec![
            vec!["p".into(), other.to_hex(), "relay hint".into()],
            vec!["x".into(), "unknown".into()],
        ];
        let original = private.clone();
        edit(&mut public, &mut private, &BTreeMap::from([(target, true)]));
        assert_eq!(public, vec![vec!["word", "spam"]]);
        assert_eq!(&private[..2], &original);
        assert_eq!(private[2], vec!["p".to_string(), target.to_hex()]);
        edit(
            &mut public,
            &mut private,
            &BTreeMap::from([(target, false)]),
        );
        assert_eq!(private, original);
    }
    #[test]
    fn local_changes_persist_and_are_account_scoped() {
        let dir = crate::test_state_dir::StateDir::new();
        let owner = Keys::generate().public_key();
        let peer = Keys::generate().public_key();
        let (store, _) = ModerationStore::open(dir.path(), owner).unwrap();
        store.block(peer, true).unwrap();
        store.mute(peer, Some(3600)).unwrap();
        let (reopened, _) = ModerationStore::open(dir.path(), owner).unwrap();
        assert!(reopened.blocked().contains(&peer));
        assert!(reopened.muted(peer));
        store.mute(peer, Some(0)).unwrap();
        assert!(!store.muted(peer));
        store.mute(peer, Some(u64::MAX)).unwrap();
        assert!(store.muted(peer));
        store.mute(peer, None).unwrap();
        assert!(!store.muted(peer));
        let (other, _) = ModerationStore::open(dir.path(), peer).unwrap();
        assert!(other.blocked().is_empty());
        store.block(peer, false).unwrap();
        assert!(store.blocked().is_empty());
        assert!(store.block(owner, true).is_err());
        store.stop();
        assert!(store.block(peer, true).is_err());
    }
    #[tokio::test]
    async fn imports_public_and_private_lists_merges_edits_and_syncs_another_client() {
        use nostr_sdk::local_relay::MockRelay;
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            let relay = MockRelay::run().await.unwrap();
            let owner = Keys::generate();
            let bob = Keys::generate().public_key();
            let carol = Keys::generate().public_key();
            let dave = Keys::generate().public_key();
            let signer = UniversalSigner::new(owner.clone());
            let private = vec![
                vec!["p".to_owned(), carol.to_hex()],
                vec!["word".into(), "preserve".into()],
            ];
            let content = signer
                .nip44_encrypt_async(
                    &owner.public_key(),
                    &serde_json::to_string(&private).unwrap(),
                )
                .await
                .unwrap();
            let original = EventBuilder::new(Kind::Custom(10000), content)
                .tag(Tag::public_key(bob))
                .tag(Tag::parse(["future", "public"]).unwrap())
                .custom_created_at(Timestamp::from(Timestamp::now().as_secs() - 5))
                .finalize(&owner)
                .unwrap();
            relay.add_event(original).await.unwrap();
            let client = Client::builder()
                .database(nostr_memory::MemoryDatabase::unbounded())
                .build();
            client
                .add_relay(relay.url().await)
                .and_connect()
                .await
                .unwrap();
            let root = crate::test_state_dir::StateDir::new();
            let (store, _) = ModerationStore::open(root.path(), owner.public_key()).unwrap();
            // An edit made before initial sync must merge with both halves of the remote list.
            store.block(dave, true).unwrap();
            store.block(bob, false).unwrap();
            store.sync(&client, &signer).await.unwrap();
            assert_eq!(store.blocked(), BTreeSet::from([carol, dave]));
            assert!(!store.pending());
            let events = client
                .fetch_events(
                    Filter::new()
                        .kind(Kind::Custom(10000))
                        .author(owner.public_key())
                        .limit(1),
                )
                .timeout(std::time::Duration::from_secs(2))
                .await
                .unwrap();
            let event = events.into_iter().next().unwrap();
            assert_eq!(
                event
                    .tags
                    .iter()
                    .map(|tag| tag.as_slice().to_vec())
                    .collect::<Vec<_>>(),
                vec![vec!["future", "public"]]
            );
            assert!(!event.content.contains(&dave.to_hex()));
            let plaintext = signer
                .nip44_decrypt_async(&owner.public_key(), &event.content)
                .await
                .unwrap();
            let tags: Vec<Vec<String>> = serde_json::from_str(&plaintext).unwrap();
            assert!(tags.contains(&vec!["word".into(), "preserve".into()]));
            let other_root = crate::test_state_dir::StateDir::new();
            let (other, _) = ModerationStore::open(other_root.path(), owner.public_key()).unwrap();
            other.sync(&client, &signer).await.unwrap();
            assert_eq!(other.blocked(), store.blocked());
            // A newer event from another client removes both users, without an edit in Goop.
            let cleared = EventBuilder::new(Kind::Custom(10000), "")
                .custom_created_at(Timestamp::from(event.created_at.as_secs() + 1))
                .finalize(&owner)
                .unwrap();
            relay.add_event(cleared).await.unwrap();
            store.sync(&client, &signer).await.unwrap();
            assert!(store.blocked().is_empty());
            client.shutdown().await;
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn unreadable_private_list_is_never_overwritten() {
        use nostr_sdk::local_relay::MockRelay;
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            let relay = MockRelay::run().await.unwrap();
            let owner = Keys::generate();
            let peer = Keys::generate().public_key();
            let original = EventBuilder::new(Kind::Custom(10000), "legacy?iv=unreadable")
                .custom_created_at(Timestamp::from(Timestamp::now().as_secs() - 5))
                .finalize(&owner)
                .unwrap();
            relay.add_event(original.clone()).await.unwrap();
            let client = Client::builder()
                .database(nostr_memory::MemoryDatabase::unbounded())
                .build();
            client
                .add_relay(relay.url().await)
                .and_connect()
                .await
                .unwrap();
            let root = crate::test_state_dir::StateDir::new();
            let (store, _) = ModerationStore::open(root.path(), owner.public_key()).unwrap();
            store.block(peer, true).unwrap();
            assert!(
                store
                    .sync(&client, &UniversalSigner::new(owner.clone()))
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("legacy encryption")
            );
            assert!(store.pending());
            assert!(store.blocked().contains(&peer));
            let events = client
                .fetch_events(
                    Filter::new()
                        .kind(Kind::Custom(10000))
                        .author(owner.public_key()),
                )
                .timeout(std::time::Duration::from_secs(2))
                .await
                .unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(events.into_iter().next().unwrap().id, original.id);
            client.shutdown().await;
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn relay_rejection_keeps_local_block_pending_for_retry() {
        use nostr_sdk::local_relay::{LocalRelay, WritePolicy, WritePolicyResult};
        #[derive(Debug)]
        struct Reject;
        impl WritePolicy for Reject {
            fn admit_event<'a>(
                &'a self,
                _: &'a Event,
                _: &'a std::net::SocketAddr,
            ) -> futures::future::BoxFuture<'a, WritePolicyResult> {
                Box::pin(async {
                    WritePolicyResult::reject(MachineReadablePrefix::Blocked, "test rejection")
                })
            }
        }
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            let relay = LocalRelay::builder().write_policy(Reject).build();
            relay.run().await.unwrap();
            let client = Client::builder()
                .database(nostr_memory::MemoryDatabase::unbounded())
                .build();
            client
                .add_relay(relay.url().await)
                .and_connect()
                .await
                .unwrap();
            let owner = Keys::generate();
            let peer = Keys::generate().public_key();
            let root = crate::test_state_dir::StateDir::new();
            let (store, _) = ModerationStore::open(root.path(), owner.public_key()).unwrap();
            store.block(peer, true).unwrap();
            assert!(
                store
                    .sync(&client, &UniversalSigner::new(owner))
                    .await
                    .is_err()
            );
            assert!(store.pending());
            assert!(store.blocked().contains(&peer));
            client.shutdown().await;
        })
        .await
        .unwrap();
    }

    #[test]
    fn blocks_hide_direct_chats_but_keep_shared_groups() {
        let owner = Keys::generate().public_key();
        let peer = Keys::generate().public_key();
        let third = Keys::generate().public_key();
        let blocked = BTreeSet::from([peer]);
        assert!(hides_room(&blocked, &[owner, peer]));
        assert!(!hides_room(&blocked, &[owner, peer, third]));
        assert!(!hides_room(&blocked, &[owner]));
        assert!(!hides_room(&BTreeSet::new(), &[owner, peer]));
    }
    #[tokio::test]
    async fn a_denied_list_read_cannot_publish_an_empty_replacement() {
        use nostr_sdk::local_relay::{LocalRelay, QueryPolicy, QueryPolicyResult};
        #[derive(Debug)]
        struct Denied;
        impl QueryPolicy for Denied {
            fn admit_query<'a>(
                &'a self,
                _: &'a mut Filter,
                _: &'a std::net::SocketAddr,
            ) -> futures::future::BoxFuture<'a, QueryPolicyResult> {
                Box::pin(async {
                    QueryPolicyResult::reject(MachineReadablePrefix::Restricted, "test denial")
                })
            }
        }
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            let relay = LocalRelay::builder().query_policy(Denied).build();
            relay.run().await.unwrap();
            let client = Client::default();
            client
                .add_relay(relay.url().await)
                .and_connect()
                .await
                .unwrap();
            let owner = Keys::generate();
            let root = crate::test_state_dir::StateDir::new();
            let (store, _) = ModerationStore::open(root.path(), owner.public_key()).unwrap();
            store.block(Keys::generate().public_key(), true).unwrap();
            let refusing = crate::test_signer::RefusingSigner::new(owner);
            let error = store
                .sync(&client, &UniversalSigner::new(refusing.clone()))
                .await
                .unwrap_err();
            assert!(
                error.to_string().contains("Could not read the block list"),
                "{error}"
            );
            assert_eq!(refusing.calls.load(Ordering::SeqCst), 0);
            assert!(store.pending());
            client.shutdown().await;
        })
        .await
        .unwrap();
    }
}
