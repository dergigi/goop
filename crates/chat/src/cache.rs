//! Verified plaintext cache: local provenance, account isolation, and rumor identity.
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::sync::{Arc, RwLock};

use anyhow::{Result, anyhow, bail};
use common::EventExt;
use futures::lock::Mutex;
use nostr_sdk::prelude::*;

pub(super) fn local_keys() -> Result<Keys> {
    #[cfg(test)]
    {
        Ok(crate::LOCAL_KEYS.clone())
    }
    #[cfg(not(test))]
    {
        load_local_keys(common::config_dir())
    }
}

fn load_local_keys(dir: &std::path::Path) -> Result<Keys> {
    // This is an internal cache-signing key, never an identity signer or relay credential.
    std::fs::create_dir_all(dir)?;
    let path = dir.join("rumor-cache-key-v1");
    if !path.exists() {
        let keys = Keys::generate();
        let mut temp = tempfile::NamedTempFile::new_in(dir)?;
        temp.write_all(&keys.secret_key().to_secret_bytes())?;
        temp.as_file().sync_all()?;
        if let Err(error) = temp.persist_noclobber(&path)
            && error.error.kind() != std::io::ErrorKind::AlreadyExists
        {
            return Err(error.error.into());
        }
        #[cfg(unix)]
        std::fs::File::open(dir)?.sync_all()?;
    }
    Ok(Keys::new(SecretKey::from_slice(&std::fs::read(path)?)?))
}

#[derive(Debug, Clone)]
pub(super) struct RumorCache {
    client: Client,
    pub owner: PublicKey,
    keys: Keys,
    write_lock: Arc<Mutex<()>>,
    rooms: Arc<RwLock<BTreeMap<EventId, u64>>>,
    reaction_targets: Arc<RwLock<BTreeSet<EventId>>>,
    search: Arc<RwLock<crate::search::MessageSearchIndex>>,
    authors: Arc<RwLock<BTreeMap<EventId, PublicKey>>>,
    incoming_positions: Arc<RwLock<BTreeMap<u64, BTreeSet<(Timestamp, EventId)>>>>,
}

impl RumorCache {
    pub fn open(client: Client, owner: PublicKey) -> Result<Self> {
        let keys = local_keys()?;
        Ok(Self::with_keys(client, owner, keys))
    }

    pub fn client(&self) -> Client {
        self.client.clone()
    }

    pub fn with_keys(client: Client, owner: PublicKey, keys: Keys) -> Self {
        Self {
            client,
            owner,
            keys,
            write_lock: Arc::default(),
            rooms: Arc::default(),
            reaction_targets: Arc::default(),
            search: Arc::default(),
            authors: Arc::default(),
            incoming_positions: Arc::default(),
        }
    }

    fn filter(&self) -> Filter {
        Filter::new()
            .kind(Kind::ApplicationSpecificData)
            .author(self.keys.public_key())
            .pubkey(self.owner)
    }

    fn parse(&self, event: &Event) -> Result<UnsignedEvent> {
        event.verify()?;
        let rumor = UnsignedEvent::from_json(&event.content)?;
        validate(&rumor, self.owner)?;
        Ok(rumor)
    }

    pub fn note(&self, rumor: &UnsignedEvent) {
        self.search.write().unwrap().insert(rumor);
        if let Some(id) = rumor.id {
            self.authors.write().unwrap().insert(id, rumor.pubkey);
            if is_chat(rumor.kind) {
                self.rooms.write().unwrap().insert(id, rumor.uniq_id());
                if rumor.pubkey != self.owner {
                    self.incoming_positions.write().unwrap().entry(rumor.uniq_id()).or_default()
                        .insert((rumor.created_at, id));
                }
            }
            if rumor.kind == Kind::Reaction {
                self.reaction_targets
                    .write()
                    .unwrap()
                    .extend(rumor.tags.event_ids());
            }
        }
    }

    pub fn read_positions(&self, rooms: &[u64]) -> Vec<(u64, crate::ReadPosition)> {
        let incoming = self.incoming_positions.read().unwrap();
        rooms.iter().map(|room| {
            let mut position = crate::ReadPosition::default();
            if let Some(messages) = incoming.get(room)
                && let Some((latest, _)) = messages.last() {
                for (time, id) in messages.iter().rev().take_while(|(time, _)| time == latest) {
                    position.note(*time, *id);
                }
            }
            (*room, position)
        }).collect()
    }

    pub fn unread_count(&self, room: u64, reads: &crate::unread::ReadStore) -> usize {
        let incoming = self.incoming_positions.read().unwrap();
        reads.count(room, incoming.get(&room).unwrap_or(&BTreeSet::new()))
    }

    pub fn unread_count_filtered(&self, room: u64, reads: &crate::unread::ReadStore, blocked: &BTreeSet<PublicKey>) -> usize {
        if blocked.is_empty() { return self.unread_count(room, reads); }
        let incoming = self.incoming_positions.read().unwrap();
        let authors = self.authors.read().unwrap();
        let visible = incoming.get(&room).into_iter().flatten().filter(|(_, id)| authors.get(id).is_none_or(|author| !blocked.contains(author))).copied().collect();
        reads.count(room, &visible)
    }

    pub fn search_messages(&self, room: u64) -> Vec<Arc<crate::SearchMessage>> {
        self.search.read().unwrap().snapshot(room)
    }

    pub fn reaction_room(&self, rumor: &UnsignedEvent) -> Option<u64> {
        rumor
            .tags
            .event_ids()
            .last()
            .and_then(|id| self.rooms.read().unwrap().get(&id).copied())
    }

    pub fn has_reactions(&self, id: EventId) -> bool {
        self.reaction_targets.read().unwrap().contains(&id)
    }

    pub async fn get(&self, wrap: EventId) -> Result<UnsignedEvent> {
        let records = self
            .client
            .database()
            .query(
                self.filter()
                    .custom_tag(SingleLetterTag::LOWERCASE_G, wrap.to_hex())
                    .limit(1),
            )
            .await?;
        let event = records
            .first()
            .ok_or_else(|| anyhow!("Rumor is not cached for this account"))?;
        let rumor = self.parse(event)?;
        self.note(&rumor);
        Ok(rumor)
    }

    /// Returns true for a new rumor, false for a different wrap of an existing rumor.
    pub async fn put(&self, wrap: EventId, rumor: &UnsignedEvent) -> Result<bool> {
        validate(rumor, self.owner)?;
        let _guard = self.write_lock.lock().await;
        let id = rumor.id.ok_or_else(|| anyhow!("Rumor has no ID"))?;
        let identifier = format!("goop-rumor-v1:{}:{id}", self.owner);
        let old = self
            .client
            .database()
            .query(self.filter().identifier(&identifier))
            .await?;
        let mut wraps: BTreeSet<String> = old
            .iter()
            .flat_map(|e| e.tags.iter())
            .filter(|tag| tag.kind() == "g")
            .filter_map(|tag| tag.content().map(str::to_owned))
            .collect();
        wraps.insert(wrap.to_hex());
        let mut tags = vec![
            Tag::identifier(identifier),
            Tag::public_key(self.owner),
            Tag::custom("k", [rumor.kind.to_string()]),
        ];
        if is_chat(rumor.kind) {
            tags.push(Tag::custom("r", [rumor.uniq_id().to_string()]));
        }
        tags.extend(wraps.into_iter().map(|id| Tag::custom("g", [id])));
        let timestamp = old
            .iter()
            .map(|e| e.created_at.as_secs().saturating_add(1))
            .max()
            .unwrap_or(0)
            .max(Timestamp::now().as_secs());
        let event = EventBuilder::new(Kind::ApplicationSpecificData, rumor.as_json())
            .tags(tags)
            .custom_created_at(Timestamp::from(timestamp))
            .finalize(&self.keys)?;
        if !self
            .client
            .database()
            .save_event(&event)
            .await?
            .is_success()
        {
            bail!("Could not persist decrypted message");
        }
        self.note(rumor);
        Ok(old.is_empty())
    }

    /// Refusals are local, account-scoped state and survive history replay/restart.
    pub async fn paused(&self, wrap: EventId) -> Result<Option<String>> {
        let records = self
            .client
            .database()
            .query(
                Filter::new()
                    .kind(Kind::Custom(30079))
                    .author(self.keys.public_key())
                    .pubkey(self.owner)
                    .identifier(format!("goop-decrypt-pause-v1:{}:{wrap}", self.owner)),
            )
            .await?;
        match records.first() {
            Some(record) => {
                record.verify()?;
                Ok((!record.content.is_empty()).then(|| record.content.clone()))
            }
            None => Ok(None),
        }
    }

    pub async fn set_paused(&self, wrap: EventId, reason: Option<&str>) -> Result<()> {
        let _guard = self.write_lock.lock().await;
        let identifier = format!("goop-decrypt-pause-v1:{}:{wrap}", self.owner);
        let old = self
            .client
            .database()
            .query(
                Filter::new()
                    .kind(Kind::Custom(30079))
                    .author(self.keys.public_key())
                    .pubkey(self.owner)
                    .identifier(&identifier),
            )
            .await?;
        if old.is_empty() && reason.is_none() {
            return Ok(());
        }
        let timestamp = old
            .iter()
            .map(|e| e.created_at.as_secs().saturating_add(1))
            .max()
            .unwrap_or(0)
            .max(Timestamp::now().as_secs());
        let event = EventBuilder::new(Kind::Custom(30079), reason.unwrap_or_default())
            .tags([Tag::identifier(identifier), Tag::public_key(self.owner)])
            .custom_created_at(Timestamp::from(timestamp))
            .finalize(&self.keys)?;
        if !self
            .client
            .database()
            .save_event(&event)
            .await?
            .is_success()
        {
            bail!("Could not save signer retry state");
        }
        Ok(())
    }

    pub async fn all(&self) -> Result<Vec<UnsignedEvent>> {
        let records = self.client.database().query(self.filter()).await?;
        let mut messages = BTreeMap::new();
        for record in records {
            let rumor = self.parse(&record)?;
            self.note(&rumor);
            messages.insert(rumor.id.unwrap(), rumor);
        }
        Ok(messages.into_values().collect())
    }
}

pub(super) fn is_chat(kind: Kind) -> bool {
    kind == Kind::PrivateDirectMessage || kind == Kind::Custom(15)
}

pub(super) fn validate(rumor: &UnsignedEvent, owner: PublicKey) -> Result<()> {
    rumor.verify_id()?;
    if rumor.id.is_none() {
        bail!("Rumor ID missing");
    }
    if !is_chat(rumor.kind) && rumor.kind != Kind::Reaction {
        bail!("Unsupported private event kind: {}", rumor.kind);
    }
    if is_chat(rumor.kind)
        && rumor.pubkey != owner
        && !rumor.tags.public_keys().any(|key| key == owner)
    {
        bail!("Account is not a participant in this message");
    }
    if rumor.kind == Kind::Reaction && rumor.tags.event_ids().next().is_none() {
        bail!("Reaction has no target message");
    }
    Ok(())
}

/// A reaction is part of its target's room, not a room inferred from reaction p-tags.
pub(super) fn for_room(messages: Vec<UnsignedEvent>, room: u64) -> Vec<UnsignedEvent> {
    let targets: BTreeSet<_> = messages
        .iter()
        .filter(|m| is_chat(m.kind) && m.uniq_id() == room)
        .filter_map(|m| m.id)
        .collect();
    let mut result: Vec<_> = messages
        .into_iter()
        .filter(|m| {
            (is_chat(m.kind) && m.uniq_id() == room)
                || (m.kind == Kind::Reaction
                    && m.tags
                        .event_ids()
                        .last()
                        .is_some_and(|id| targets.contains(&id)))
        })
        .collect();
    result.sort_by_key(|m| (m.created_at, m.id));
    result.dedup_by_key(|m| m.id);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(
        sender: &Keys,
        recipients: &[PublicKey],
        kind: Kind,
        content: &str,
    ) -> UnsignedEvent {
        let mut rumor = EventBuilder::new(kind, content)
            .tags(recipients.iter().copied().map(Tag::public_key))
            .finalize_unsigned(sender.public_key());
        rumor.ensure_id();
        rumor
    }

    #[test]
    fn unread_index_excludes_own_messages_and_reactions() {
        let owner = Keys::generate();
        let peer = Keys::generate();
        let cache = RumorCache::with_keys(Client::default(), owner.public_key(), Keys::generate());
        let root = tempfile::tempdir().unwrap();
        let reads = crate::unread::ReadStore::open(root.path(), owner.public_key()).unwrap();
        let mut own = EventBuilder::new(Kind::PrivateDirectMessage, "sent")
            .tag(Tag::public_key(peer.public_key())).finalize_unsigned(owner.public_key());
        own.ensure_id(); cache.note(&own);
        assert_eq!(cache.unread_count(own.uniq_id(), &reads), 0);
        let mut incoming = EventBuilder::new(Kind::PrivateDirectMessage, "received")
            .tag(Tag::public_key(owner.public_key())).finalize_unsigned(peer.public_key());
        incoming.ensure_id(); cache.note(&incoming); cache.note(&incoming);
        assert_eq!(cache.unread_count(incoming.uniq_id(), &reads), 1);
        let mut reaction = EventBuilder::new(Kind::Reaction, "+")
            .tag(Tag::event(incoming.id.unwrap())).finalize_unsigned(peer.public_key());
        reaction.ensure_id(); cache.note(&reaction);
        assert_eq!(cache.unread_count(incoming.uniq_id(), &reads), 1);
    }

    #[tokio::test]
    async fn cache_is_account_scoped_and_deduplicates_wraps_after_restart() {
        let client = Client::builder()
            .database(nostr_memory::MemoryDatabase::unbounded())
            .build();
        let local = Keys::generate();
        let alice = Keys::generate();
        let bob = Keys::generate();
        let sender = Keys::generate();
        let rumor = message(
            &sender,
            &[alice.public_key(), bob.public_key()],
            Kind::PrivateDirectMessage,
            "group",
        );
        let a = RumorCache::with_keys(client.clone(), alice.public_key(), local.clone());
        let b = RumorCache::with_keys(client.clone(), bob.public_key(), local.clone());
        let first = EventId::from_byte_array([1; 32]);
        let second = EventId::from_byte_array([2; 32]);
        let third = EventId::from_byte_array([3; 32]);
        assert!(a.put(first, &rumor).await.unwrap());
        assert!(b.all().await.unwrap().is_empty());
        assert!(b.get(first).await.is_err());
        let clone = a.clone();
        let (one, two) = futures::join!(a.put(second, &rumor), clone.put(third, &rumor));
        assert!(!one.unwrap());
        assert!(!two.unwrap());
        let restarted = RumorCache::with_keys(client.clone(), alice.public_key(), local);
        assert_eq!(restarted.all().await.unwrap(), vec![rumor.clone()]);
        for wrap in [first, second, third] {
            assert_eq!(restarted.get(wrap).await.unwrap(), rumor);
        }
        assert!(b.put(first, &rumor).await.unwrap());
        assert_eq!(b.all().await.unwrap(), vec![rumor]);
        client.shutdown().await;
    }

    #[tokio::test]
    async fn unsigned_provenance_and_legacy_records_are_not_trusted() {
        let client = Client::builder()
            .database(nostr_memory::MemoryDatabase::unbounded())
            .build();
        let owner = Keys::generate();
        let sender = Keys::generate();
        let cache = RumorCache::with_keys(client.clone(), owner.public_key(), Keys::generate());
        let rumor = message(
            &sender,
            &[owner.public_key()],
            Kind::PrivateDirectMessage,
            "hello",
        );
        let wrap = EventId::from_byte_array([1; 32]);
        for identifier in [
            wrap.to_hex(),
            format!("goop-rumor-v1:{}:{}", owner.public_key(), rumor.id.unwrap()),
        ] {
            let forged = EventBuilder::new(Kind::ApplicationSpecificData, rumor.as_json())
                .tags([
                    Tag::identifier(identifier),
                    Tag::public_key(owner.public_key()),
                    Tag::custom("g", [wrap.to_hex()]),
                    Tag::custom("k", ["14"]),
                ])
                .finalize(&sender)
                .unwrap();
            client.database().save_event(&forged).await.unwrap();
        }
        assert!(cache.all().await.unwrap().is_empty());
        assert!(cache.get(wrap).await.is_err());
        client.shutdown().await;
    }

    #[tokio::test]
    async fn group_reactions_wait_for_their_target_and_keep_the_target_room() {
        let client = Client::builder()
            .database(nostr_memory::MemoryDatabase::unbounded())
            .build();
        let owner = Keys::generate();
        let sender = Keys::generate();
        let third = Keys::generate();
        let cache = RumorCache::with_keys(client.clone(), owner.public_key(), Keys::generate());
        let target = message(
            &sender,
            &[owner.public_key(), third.public_key()],
            Kind::PrivateDirectMessage,
            "group",
        );
        let mut reaction = EventBuilder::new(Kind::Reaction, "+")
            .tags([
                Tag::event(target.id.unwrap()),
                Tag::public_key(sender.public_key()),
            ])
            .finalize_unsigned(third.public_key());
        reaction.ensure_id();
        assert_ne!(reaction.uniq_id(), target.uniq_id());
        cache
            .put(EventId::from_byte_array([1; 32]), &reaction)
            .await
            .unwrap();
        assert!(cache.reaction_room(&reaction).is_none());
        assert!(for_room(cache.all().await.unwrap(), target.uniq_id()).is_empty());
        cache
            .put(EventId::from_byte_array([2; 32]), &target)
            .await
            .unwrap();
        assert_eq!(cache.reaction_room(&reaction), Some(target.uniq_id()));
        assert!(cache.has_reactions(target.id.unwrap()));
        assert_eq!(
            for_room(cache.all().await.unwrap(), target.uniq_id()).len(),
            2
        );
        client.shutdown().await;
    }

    #[test]
    fn local_provenance_key_survives_restart_with_private_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let first = load_local_keys(dir.path()).unwrap();
        assert_eq!(
            load_local_keys(dir.path()).unwrap().public_key(),
            first.public_key()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join("rumor-cache-key-v1"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0);
        }
    }

    #[test]
    fn rejects_unsupported_events_and_messages_for_other_accounts() {
        let sender = Keys::generate();
        let owner = Keys::generate().public_key();
        assert!(
            validate(
                &message(&sender, &[owner], Kind::TextNote, "public note"),
                owner
            )
            .is_err()
        );
        assert!(
            validate(
                &message(&sender, &[], Kind::PrivateDirectMessage, "not yours"),
                owner
            )
            .is_err()
        );
        assert!(validate(&message(&sender, &[owner], Kind::Reaction, "+"), owner).is_err());
        assert!(validate(&message(&sender, &[owner], Kind::Custom(15), "file"), owner).is_ok());
    }
    #[test]
    fn blocked_authors_do_not_count_as_unread_and_unblocking_restores_them() {
        let owner = Keys::generate().public_key();
        let bob = Keys::generate().public_key(); let carol = Keys::generate().public_key();
        let client = Client::default();
        let cache = RumorCache::with_keys(client, owner, Keys::generate());
        let root = tempfile::tempdir().unwrap();
        let reads = crate::unread::ReadStore::open(root.path(),owner).unwrap();
        let message = |author| {
            let mut event = EventBuilder::new(Kind::PrivateDirectMessage,"hello")
                .tags([Tag::public_key(owner),Tag::public_key(bob),Tag::public_key(carol)])
                .finalize_unsigned(author);
            event.ensure_id(); event
        };
        let first = message(bob); let second = message(carol);
        let room = first.uniq_id(); assert_eq!(room,second.uniq_id());
        cache.note(&first); cache.note(&second); cache.note(&first);
        assert_eq!(cache.unread_count(room,&reads),2);
        assert_eq!(cache.unread_count_filtered(room,&reads,&BTreeSet::from([bob])),1);
        assert_eq!(cache.unread_count_filtered(room,&reads,&BTreeSet::from([bob,carol])),0);
        assert_eq!(cache.unread_count_filtered(room,&reads,&BTreeSet::new()),2);
        assert_eq!(cache.search_messages(room).iter().filter(|message| message.author != bob).count(),1);
    }

}
