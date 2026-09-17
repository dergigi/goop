//! One displayed reaction per rumor, regardless of delivery or reload count.
use std::collections::{BTreeMap, HashSet};

use gpui::SharedString;
use nostr_sdk::prelude::*;

#[derive(Default)]
pub(super) struct Reactions {
    seen: HashSet<EventId>,
    by_message: BTreeMap<EventId, Vec<(SharedString, PublicKey)>>,
}

impl Reactions {
    pub fn insert(&mut self, event: &UnsignedEvent) -> bool {
        if event.kind != Kind::Reaction { return false; }
        let (Some(id), Some(target)) = (event.id, event.tags.event_ids().last()) else {
            return false;
        };
        // Use the inner rumor ID: relay acknowledgements, self copies, and
        // separate gift wraps can all refer to this same reaction.
        if !self.seen.insert(id) { return false; }
        self.by_message.entry(target).or_default()
            .push((SharedString::from(&event.content), event.pubkey));
        true
    }

    pub fn get(&self, target: &EventId) -> &[(SharedString, PublicKey)] {
        self.by_message.get(target).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn clear(&mut self) {
        self.seen.clear();
        self.by_message.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reaction(author: PublicKey, target: EventId, emoji: &str) -> UnsignedEvent {
        let mut event = EventBuilder::new(Kind::Reaction, emoji)
            .tag(Tag::event(target)).finalize_unsigned(author);
        event.ensure_id();
        event
    }

    #[test]
    fn local_send_relay_updates_and_history_replay_count_once() {
        let author = Keys::generate().public_key();
        let target = EventId::from_byte_array([1; 32]);
        let event = reaction(author, target, "🤙");
        let mut reactions = Reactions::default();
        assert!(reactions.insert(&event)); // Local outgoing intent.
        for _ in 0..6 { assert!(!reactions.insert(&event)); } // Relay progress/self copies.
        for _ in 0..3 { assert!(!reactions.insert(&event)); } // History reloads.
        assert_eq!(reactions.get(&target), &[(SharedString::from("🤙"), author)]);
        reactions.clear(); // Block/unblock rebuilds must clear both indexes.
        assert!(reactions.insert(&event));
        assert_eq!(reactions.get(&target).len(), 1);
    }

    #[test]
    fn distinct_authors_emojis_and_message_targets_remain_independent() {
        let alice = Keys::generate().public_key();
        let bob = Keys::generate().public_key();
        let first = EventId::from_byte_array([1; 32]);
        let second = EventId::from_byte_array([2; 32]);
        let mut reactions = Reactions::default();
        for event in [reaction(alice, first, "🧡"), reaction(bob, first, "🧡"),
            reaction(alice, first, "👀"), reaction(alice, second, "🧡")] {
            assert!(reactions.insert(&event));
        }
        assert_eq!(reactions.get(&first).len(), 3);
        assert_eq!(reactions.get(&second).len(), 1);
    }

    #[test]
    fn uses_the_same_last_target_as_the_chat_cache_and_ignores_incomplete_events() {
        let author = Keys::generate().public_key();
        let first = EventId::from_byte_array([1; 32]);
        let target = EventId::from_byte_array([2; 32]);
        let mut event = EventBuilder::new(Kind::Reaction, "👀")
            .tags([Tag::event(first), Tag::event(target), Tag::event(target)])
            .finalize_unsigned(author);
        let mut reactions = Reactions::default();
        assert!(!reactions.insert(&event));
        event.ensure_id();
        assert!(reactions.insert(&event));
        assert!(reactions.get(&first).is_empty());
        assert_eq!(reactions.get(&target).len(), 1);
        let mut no_target = EventBuilder::new(Kind::Reaction, "👀").finalize_unsigned(author);
        no_target.ensure_id();
        assert!(!reactions.insert(&no_target));
    }
}
