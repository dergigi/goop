//! Device-local read positions. These are never published as read receipts.
use anyhow::Result;
use nostr_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use std::{collections::{BTreeMap, BTreeSet}, io::Write, path::{Path, PathBuf}};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadPosition {
    timestamp: Timestamp,
    ids: BTreeSet<EventId>,
}
impl ReadPosition {
    pub fn note(&mut self, timestamp: Timestamp, id: EventId) {
        if timestamp > self.timestamp { self.timestamp = timestamp; self.ids.clear(); }
        if timestamp == self.timestamp { self.ids.insert(id); }
    }
    pub fn has_unread(&self, read: &Self) -> bool {
        !self.ids.is_empty() && (self.timestamp > read.timestamp
            || (self.timestamp == read.timestamp && !self.ids.is_subset(&read.ids)))
    }
    fn merge(&mut self, other: &Self) {
        for id in &other.ids { self.note(other.timestamp, *id); }
    }
}
#[derive(Debug)]
pub(super) struct ReadStore {
    path: PathBuf,
    positions: BTreeMap<u64, ReadPosition>,
}
impl ReadStore {
    pub fn open(root: &Path, owner: PublicKey) -> Result<Self> {
        let path = root.join("goop-read-v1").join(format!("{}.json", owner.to_hex()));
        let positions = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self { path, positions })
    }
    pub fn count(&self, room: u64, incoming: &BTreeSet<(Timestamp, EventId)>) -> usize {
        let default = ReadPosition::default();
        let read = self.positions.get(&room).unwrap_or(&default);
        incoming.range((read.timestamp, EventId::from_slice(&[0; 32]).unwrap())..)
            .filter(|(time, id)| *time > read.timestamp || !read.ids.contains(id)).count()
    }
    #[cfg(test)]
    pub fn has_unread(&self, room: u64, incoming: &ReadPosition) -> bool {
        incoming.has_unread(self.positions.get(&room).unwrap_or(&ReadPosition::default()))
    }
    pub fn mark(&mut self, room: u64, position: &ReadPosition) -> Result<bool> {
        if !position.has_unread(self.positions.get(&room).unwrap_or(&ReadPosition::default())) {
            return Ok(false);
        }
        let mut updated = self.positions.clone();
        updated.entry(room).or_default().merge(position);
        if updated == self.positions { return Ok(false); }
        let dir = self.path.parent().unwrap();
        std::fs::create_dir_all(dir)?;
        let mut temp = tempfile::NamedTempFile::new_in(dir)?;
        temp.write_all(&serde_json::to_vec(&updated)?)?;
        temp.as_file().sync_all()?;
        temp.persist(&self.path).map_err(|e| e.error)?;
        self.positions = updated;
        Ok(true)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn id(n: u8) -> EventId { EventId::from_slice(&[n; 32]).unwrap() }
    #[test]
    fn unread_counts_deduplicate_and_clear_at_the_read_position() {
        let root = tempfile::tempdir().unwrap();
        let mut store = ReadStore::open(root.path(), Keys::generate().public_key()).unwrap();
        let mut messages = BTreeSet::from([(Timestamp::from(10), id(1)), (Timestamp::from(11), id(2))]);
        messages.insert((Timestamp::from(11), id(2)));
        assert_eq!(store.count(1, &messages), 2);
        let mut read = ReadPosition::default(); read.note(Timestamp::from(10), id(1));
        store.mark(1, &read).unwrap(); assert_eq!(store.count(1, &messages), 1);
        read.note(Timestamp::from(11), id(2)); store.mark(1, &read).unwrap();
        assert_eq!(store.count(1, &messages), 0);
        messages.insert((Timestamp::from(11), id(3)));
        assert_eq!(store.count(1, &messages), 1);
    }
    #[test]
    fn read_positions_survive_restart_and_same_second_arrivals() {
        let root = tempfile::tempdir().unwrap();
        let owner = Keys::generate().public_key();
        let other = Keys::generate().public_key();
        let mut store = ReadStore::open(root.path(), owner).unwrap();
        let mut incoming = ReadPosition::default();
        incoming.note(Timestamp::from(10), id(2));
        assert!(store.has_unread(1, &incoming));
        assert!(store.mark(1, &incoming).unwrap());
        assert!(!store.mark(1, &incoming).unwrap());
        let mut store = ReadStore::open(root.path(), owner).unwrap();
        assert!(!store.has_unread(1, &incoming));
        assert!(ReadStore::open(root.path(), other).unwrap().has_unread(1, &incoming));
        incoming.note(Timestamp::from(9), id(3));
        assert!(!store.has_unread(1, &incoming));
        incoming.note(Timestamp::from(10), id(1));
        assert!(store.has_unread(1, &incoming));
        store.mark(1, &incoming).unwrap();
        incoming.note(Timestamp::from(11), id(4));
        assert!(store.has_unread(1, &incoming));
        assert!(store.has_unread(2, &incoming));
    }
}
