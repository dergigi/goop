//! Searchable plaintext stays in memory and follows the account cache's lifetime.
use std::collections::BTreeMap;
use std::sync::Arc;

use common::EventExt;
use nostr_sdk::prelude::*;

#[derive(Debug)]
pub struct SearchMessage {
    pub content: Arc<str>,
    pub normalized: Arc<str>,
    pub created_at: Timestamp,
}

#[derive(Debug, Default)]
pub(crate) struct MessageSearchIndex(BTreeMap<u64, BTreeMap<EventId, Arc<SearchMessage>>>);

impl MessageSearchIndex {
    pub fn insert(&mut self, rumor: &UnsignedEvent) {
        if !super::cache::is_chat(rumor.kind) || rumor.content.trim().is_empty() {
            return;
        }
        let Some(id) = rumor.id else { return };
        self.0.entry(rumor.uniq_id()).or_default().entry(id).or_insert_with(|| {
            Arc::new(SearchMessage {
                content: rumor.content.as_str().into(),
                normalized: rumor.content.to_lowercase().into(),
                created_at: rumor.created_at,
            })
        });
    }

    pub fn snapshot(&self, room: u64) -> Vec<Arc<SearchMessage>> {
        let mut messages: Vec<_> = self.0.get(&room).into_iter()
            .flat_map(|messages| messages.values().cloned()).collect();
        messages.sort_by_key(|message| std::cmp::Reverse(message.created_at));
        messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexes_each_message_once_and_keeps_rooms_separate() {
        let sender = Keys::generate();
        let peer = Keys::generate().public_key();
        let mut first = EventBuilder::new(Kind::PrivateDirectMessage, "Hello ALICE")
            .tag(Tag::public_key(peer)).finalize_unsigned(sender.public_key());
        first.ensure_id();
        let mut other = EventBuilder::new(Kind::PrivateDirectMessage, "Another room")
            .tag(Tag::public_key(Keys::generate().public_key())).finalize_unsigned(sender.public_key());
        other.ensure_id();
        let mut index = MessageSearchIndex::default();
        index.insert(&first);
        index.insert(&first);
        index.insert(&other);
        let messages = index.snapshot(first.uniq_id());
        assert_eq!(messages.len(), 1);
        assert_eq!(&*messages[0].normalized, "hello alice");
        assert_eq!(&*messages[0].content, "Hello ALICE");
        let mut reaction = first.clone();
        reaction.kind = Kind::Reaction;
        reaction.content = "👍".into();
        reaction.id = None;
        reaction.ensure_id();
        index.insert(&reaction);
        assert_eq!(index.snapshot(first.uniq_id()).len(), 1);
        assert!(MessageSearchIndex::default().snapshot(first.uniq_id()).is_empty());
    }
}
