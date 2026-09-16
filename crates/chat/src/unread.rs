//! Device-local read positions. These are never published as read receipts.
use anyhow::Result;
use nostr_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use std::{collections::{BTreeMap, BTreeSet}, io::Write, path::{Path, PathBuf}};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadPosition {
    timestamp: Timestamp,
    ids: BTreeSet<EventId>,
    #[serde(default)]
    marked_unread: bool,
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
        self.count_filtered(room, incoming, |_| true)
    }
    /// Filter only unread candidates; never copy the room's complete history.
    pub fn count_filtered(
        &self,
        room: u64,
        incoming: &BTreeSet<(Timestamp, EventId)>,
        mut visible: impl FnMut(&EventId) -> bool,
    ) -> usize {
        let default = ReadPosition::default();
        let read = self.positions.get(&room).unwrap_or(&default);
        let count = incoming.range((read.timestamp, EventId::from_byte_array([0; 32]))..)
            .filter(|(time, id)| (*time > read.timestamp || !read.ids.contains(id)) && visible(id))
            .count();
        count.max(usize::from(read.marked_unread))
    }
    #[cfg(test)]
    pub fn has_unread(&self, room: u64, incoming: &ReadPosition) -> bool {
        incoming.has_unread(self.positions.get(&room).unwrap_or(&ReadPosition::default()))
    }
    pub fn mark(&mut self, room: u64, position: &ReadPosition) -> Result<bool> {
        self.mark_many(&[(room, position.clone())])
    }
    pub fn mark_many(&mut self, positions: &[(u64, ReadPosition)]) -> Result<bool> {
        if !positions.iter().any(|(room, position)|
            self.positions.get(room).is_some_and(|read| read.marked_unread)
                || position.has_unread(self.positions.get(room).unwrap_or(&ReadPosition::default()))) {
            return Ok(false);
        }
        let mut updated = self.positions.clone();
        for (room, position) in positions {
            let read = updated.entry(*room).or_default();
            read.merge(position);
            read.marked_unread = false;
        }
        self.persist(updated)
    }
    pub fn mark_unread(&mut self, room: u64) -> Result<bool> {
        let mut updated = self.positions.clone();
        updated.entry(room).or_default().marked_unread = true;
        self.persist(updated)
    }
    fn persist(&mut self, updated: BTreeMap<u64, ReadPosition>) -> Result<bool> {
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
    fn filtering_only_visits_unread_candidates_in_large_histories() {
        let root = tempfile::tempdir().unwrap();
        let mut store = ReadStore::open(root.path(), Keys::generate().public_key()).unwrap();
        let incoming: BTreeSet<_> = (1..=100_000).map(|time| (Timestamp::from(time), id(1))).collect();
        let mut position = ReadPosition::default();
        position.note(Timestamp::from(99_990), id(1));
        store.mark(1, &position).unwrap();
        let mut visited = 0;
        assert_eq!(store.count_filtered(1, &incoming, |_| { visited += 1; true }), 10);
        assert_eq!(visited, 10);
        assert_eq!(store.count_filtered(1, &incoming, |_| false), 0);
        store.mark_unread(1).unwrap();
        assert_eq!(store.count_filtered(1, &incoming, |_| false), 1);
    }

    #[test]
    fn manual_unread_survives_restart_and_clears_for_self_chats_too() {
        let root = tempfile::tempdir().unwrap();
        let owner = Keys::generate().public_key();
        let mut store = ReadStore::open(root.path(), owner).unwrap();
        assert!(store.mark_unread(1).unwrap());
        let mut store = ReadStore::open(root.path(), owner).unwrap();
        assert_eq!(store.count(1, &BTreeSet::new()), 1);
        store.mark_many(&[(1, ReadPosition::default())]).unwrap();
        assert_eq!(store.count(1, &BTreeSet::new()), 0);
        let messages = BTreeSet::from([(Timestamp::from(10), id(1)), (Timestamp::from(11), id(2))]);
        store.mark_unread(1).unwrap();
        assert_eq!(store.count(1, &messages), 2);
        let legacy: ReadPosition = serde_json::from_value(serde_json::json!({"timestamp": 0, "ids": []})).unwrap();
        assert!(!legacy.marked_unread);
    }

    #[test]
    fn marking_a_list_read_persists_without_touching_other_rooms_or_future_messages() {
        let root = tempfile::tempdir().unwrap();
        let owner = Keys::generate().public_key();
        let mut store = ReadStore::open(root.path(), owner).unwrap();
        let mut position = ReadPosition::default(); position.note(Timestamp::from(10), id(1));
        assert!(store.mark_many(&[(1, position.clone()), (2, position.clone())]).unwrap());
        let mut store = ReadStore::open(root.path(), owner).unwrap();
        assert!(!store.has_unread(1, &position));
        assert!(!store.has_unread(2, &position));
        assert!(store.has_unread(3, &position));
        assert!(!store.mark_many(&[(1, position.clone()), (2, position.clone())]).unwrap());
        position.note(Timestamp::from(10), id(2));
        assert!(store.has_unread(1, &position));
    }

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
