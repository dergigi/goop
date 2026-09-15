//! Nospeak-compatible, self-encrypted kind-30000 `dm-archive` lists.
use anyhow::{Result, ensure};
use nostr_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use state::UniversalSigner;
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};

const D_TAG: &str = "dm-archive";

/// Nospeak uses the peer's npub for a DM, or the first 16 hex characters of
/// SHA-256(sorted, concatenated participant hex keys) for a group.
pub(super) fn conversation_id(members: &[PublicKey], owner: PublicKey) -> String {
    use sha2::{Digest, Sha256};
    let mut members = members.to_vec();
    members.push(owner);
    members.sort();
    members.dedup();
    let peers: Vec<_> = members.iter().filter(|key| **key != owner).collect();
    if peers.len() == 1 {
        return peers[0].to_bech32().unwrap();
    }
    let joined: String = members.iter().map(|key| key.to_hex()).collect();
    format!("{:x}", Sha256::digest(joined.as_bytes()))[..16].to_owned()
}

fn tag_id(tag: &[String]) -> Option<String> {
    match (tag.first()?.as_str(), tag.get(1)?) {
        ("p", key) => PublicKey::from_hex(key)
            .ok()
            .map(|key| key.to_bech32().unwrap()),
        ("e", id) => Some(id.clone()),
        _ => None,
    }
}
fn apply_edits(mut tags: Vec<Vec<String>>, edits: &BTreeMap<String, bool>) -> Vec<Vec<String>> {
    tags.retain(|tag| tag_id(tag).is_none_or(|id| !edits.contains_key(&id)));
    for (id, archived) in edits {
        if !archived {
            continue;
        }
        if let Ok(key) = PublicKey::from_bech32(id) {
            tags.push(vec!["p".into(), key.to_hex()]);
        } else {
            tags.push(vec!["e".into(), id.clone()]);
        }
    }
    tags
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Data {
    tags: Vec<Vec<String>>,
    #[serde(default)]
    left: std::collections::BTreeSet<String>,
    pending: BTreeMap<String, bool>,
    newest: Option<(Timestamp, EventId)>,
}

#[derive(Clone, Debug)]
pub(super) struct ArchiveStore {
    owner: PublicKey,
    path: PathBuf,
    data: Arc<RwLock<Data>>,
    active: Arc<AtomicBool>,
    pub paused: Arc<AtomicBool>,
    wake: flume::Sender<()>,
}
impl ArchiveStore {
    pub fn open(root: &Path, owner: PublicKey) -> Result<(Self, flume::Receiver<()>)> {
        let path = root.join("goop-archives-v1").join(format!("{owner}.json"));
        let data = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Data::default(),
            Err(error) => return Err(error.into()),
        };
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
        self.active.store(false, Ordering::SeqCst);
    }
    fn ensure_active(&self) -> Result<()> {
        ensure!(
            self.active.load(Ordering::SeqCst),
            "Archive account changed"
        );
        Ok(())
    }
    pub fn contains(&self, members: &[PublicKey]) -> bool {
        let id = conversation_id(members, self.owner);
        let data = self.data.read().unwrap();
        data.pending.get(&id).copied().unwrap_or_else(|| {
            data.tags
                .iter()
                .any(|tag| tag_id(tag).as_ref() == Some(&id))
        })
    }
    pub fn has_left(&self, members: &[PublicKey]) -> bool {
        self.data
            .read()
            .unwrap()
            .left
            .contains(&conversation_id(members, self.owner))
    }
    pub fn leave(&self, members: &[PublicKey], left: bool) -> Result<()> {
        self.ensure_active()?;
        let mut data = self.data.write().unwrap();
        let mut updated = data.clone();
        let id = conversation_id(members, self.owner);
        if left {
            updated.left.insert(id.clone());
        } else {
            updated.left.remove(&id);
        }
        updated.pending.insert(id, left);
        self.persist(&updated)?;
        *data = updated;
        self.retry();
        Ok(())
    }
    fn persist(&self, data: &Data) -> Result<()> {
        let dir = self.path.parent().unwrap();
        std::fs::create_dir_all(dir)?;
        let mut file = tempfile::NamedTempFile::new_in(dir)?;
        file.write_all(&serde_json::to_vec(data)?)?;
        file.as_file().sync_all()?;
        file.persist(&self.path).map_err(|e| e.error)?;
        Ok(())
    }
    pub fn set(&self, members: &[PublicKey], archived: bool) -> Result<()> {
        self.ensure_active()?;
        let mut data = self.data.write().unwrap();
        let mut updated = data.clone();
        updated
            .pending
            .insert(conversation_id(members, self.owner), archived);
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
            .kind(Kind::Custom(30000))
            .author(self.owner)
            .identifier(D_TAG)
            .limit(1);
        // Never overwrite an unknown remote list after a failed discovery request.
        let remote = client
            .fetch_events(filter.clone())
            .timeout(std::time::Duration::from_secs(10))
            .await?;
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
                let plaintext = signer
                    .nip44_decrypt_async(&self.owner, &event.content)
                    .await?;
                let tags: Vec<Vec<String>> = serde_json::from_str(&plaintext)?;
                self.ensure_active()?;
                let mut data = self.data.write().unwrap();
                let mut updated = data.clone();
                updated.tags = tags;
                updated.newest = Some(stamp);
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
            "Archive signer changed"
        );
        let tags = apply_edits(snapshot.tags, &snapshot.pending);
        let content = signer
            .nip44_encrypt_async(&self.owner, &serde_json::to_string(&tags)?)
            .await?;
        let now = Timestamp::now();
        // Addressable-event replacement needs a strictly newer timestamp.
        if let Some((previous, _)) = snapshot.newest {
            ensure!(
                now > previous,
                "Archive update is waiting for the next timestamp; retry shortly"
            );
        }
        let event = EventBuilder::new(Kind::Custom(30000), content)
            .tag(Tag::identifier(D_TAG))
            .custom_created_at(now)
            .finalize_async(signer)
            .await?;
        self.ensure_active()?;
        let output = client
            .send_event(&event)
            .ack_policy(AckPolicy::all())
            .await?;
        ensure!(
            output.success.values().any(|status| status.is_ack()),
            "No relay accepted archive settings"
        );
        self.ensure_active()?;
        let mut data = self.data.write().unwrap();
        let mut updated = data.clone();
        updated.tags = tags;
        updated.newest = Some((event.created_at, event.id));
        for (id, value) in snapshot.pending {
            if updated.pending.get(&id) == Some(&value) {
                updated.pending.remove(&id);
            }
        }
        self.persist(&updated)?;
        *data = updated;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nospeak_ids_and_private_list_tags_match() {
        let a = Keys::parse("0000000000000000000000000000000000000000000000000000000000000001")
            .unwrap()
            .public_key();
        let b = Keys::parse("0000000000000000000000000000000000000000000000000000000000000002")
            .unwrap()
            .public_key();
        let c = Keys::parse("0000000000000000000000000000000000000000000000000000000000000003")
            .unwrap()
            .public_key();
        assert_eq!(conversation_id(&[a, b], a), b.to_bech32().unwrap());
        assert_eq!(
            conversation_id(&[c, b, a, b], a),
            conversation_id(&[a, b], c)
        );
        assert_eq!(conversation_id(&[a, b, c], a), "b9ac474811d3bb83");
        let id = b.to_bech32().unwrap();
        let original = vec![
            vec!["e".into(), id.clone()],
            vec!["future".into(), "preserve".into()],
        ];
        let edited = apply_edits(original, &BTreeMap::from([(id.clone(), true)]));
        assert!(edited.contains(&vec!["p".into(), b.to_hex()]));
        assert!(!edited.contains(&vec!["e".into(), id.clone()]));
        assert_eq!(
            apply_edits(edited, &BTreeMap::from([(id, false)])),
            vec![vec!["future", "preserve"]]
        );
    }
    #[tokio::test]
    async fn encrypted_nospeak_list_round_trips_and_remote_unarchive_is_applied() {
        use nostr_sdk::local_relay::MockRelay;
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            let relay = MockRelay::run().await.unwrap();
            let owner = Keys::generate();
            let bob = Keys::generate().public_key();
            let carol = Keys::generate().public_key();
            let signer = UniversalSigner::new(owner.clone());
            let original = vec![
                vec!["p".to_owned(), bob.to_hex()],
                vec!["future".into(), "keep".into()],
            ];
            let content = signer
                .nip44_encrypt_async(
                    &owner.public_key(),
                    &serde_json::to_string(&original).unwrap(),
                )
                .await
                .unwrap();
            let event = EventBuilder::new(Kind::Custom(30000), content)
                .tag(Tag::identifier(D_TAG))
                .custom_created_at(Timestamp::from(Timestamp::now().as_secs() - 5))
                .finalize(&owner)
                .unwrap();
            relay.add_event(event).await.unwrap();
            let client = Client::builder()
                .database(nostr_memory::MemoryDatabase::unbounded())
                .build();
            client
                .add_relay(relay.url().await)
                .and_connect()
                .await
                .unwrap();
            let root = tempfile::tempdir().unwrap();
            let (store, _) = ArchiveStore::open(root.path(), owner.public_key()).unwrap();
            store.sync(&client, &signer).await.unwrap();
            assert!(store.contains(&[bob]));
            let other_root = tempfile::tempdir().unwrap();
            let (other, _) = ArchiveStore::open(other_root.path(), owner.public_key()).unwrap();
            other.sync(&client, &signer).await.unwrap();
            assert!(other.contains(&[bob]));
            store.set(&[bob], false).unwrap();
            store.set(&[carol], true).unwrap();
            store.sync(&client, &signer).await.unwrap();
            let event = client
                .fetch_events(
                    Filter::new()
                        .kind(Kind::Custom(30000))
                        .author(owner.public_key())
                        .identifier(D_TAG)
                        .limit(1),
                )
                .timeout(std::time::Duration::from_secs(2))
                .await
                .unwrap()
                .into_iter()
                .next()
                .unwrap();
            assert_eq!(event.tags.as_slice().len(), 1);
            assert!(!event.content.contains(&carol.to_hex()));
            let plaintext = signer
                .nip44_decrypt_async(&owner.public_key(), &event.content)
                .await
                .unwrap();
            let tags: Vec<Vec<String>> = serde_json::from_str(&plaintext).unwrap();
            assert!(tags.contains(&vec!["p".into(), carol.to_hex()]));
            assert!(!tags.contains(&vec!["p".into(), bob.to_hex()]));
            assert!(tags.contains(&vec!["future".into(), "keep".into()]));
            other.sync(&client, &signer).await.unwrap();
            assert!(!other.contains(&[bob]));
            assert!(other.contains(&[carol]));
            client.shutdown().await;
        })
        .await
        .unwrap();
    }

    #[test]
    fn pending_archive_and_unarchive_survive_restart_and_accounts_are_isolated() {
        let root = tempfile::tempdir().unwrap();
        let owner = Keys::generate().public_key();
        let peer = Keys::generate().public_key();
        let (store, _) = ArchiveStore::open(root.path(), owner).unwrap();
        store.set(&[peer], true).unwrap();
        let (restarted, _) = ArchiveStore::open(root.path(), owner).unwrap();
        assert!(restarted.contains(&[peer]));
        assert!(
            !ArchiveStore::open(root.path(), peer)
                .unwrap()
                .0
                .contains(&[owner])
        );
        restarted.leave(&[peer], true).unwrap();
        assert!(
            ArchiveStore::open(root.path(), owner)
                .unwrap()
                .0
                .has_left(&[peer])
        );
        restarted.leave(&[peer], false).unwrap();
        assert!(
            !ArchiveStore::open(root.path(), owner)
                .unwrap()
                .0
                .has_left(&[peer])
        );
        restarted.set(&[peer], false).unwrap();
        assert!(
            !ArchiveStore::open(root.path(), owner)
                .unwrap()
                .0
                .contains(&[peer])
        );
    }
}
