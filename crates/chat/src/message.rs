use std::hash::Hash;
use std::ops::Range;

use common::{EventExt, NostrParser, extract_and_remove_media_urls};
use gpui::{SharedString, SharedUri};
use nostr_sdk::prelude::*;

/// Rendered message.
#[derive(Debug, Clone)]
pub struct Message {
    pub id: EventId,
    /// Author's public key
    pub author: PublicKey,
    /// The content/text of the message
    pub content: String,
    /// List of media URLs in the message
    pub media: Vec<SharedUri>,
    /// Message created time as unix timestamp
    pub created_at: Timestamp,
    /// List of mentioned public keys in the message
    pub mentions: Vec<Mention>,
    /// List of event of the message this message is a reply to
    pub replies_to: Vec<EventId>,
}

impl From<&Event> for Message {
    fn from(val: &Event) -> Self {
        let replies_to = extract_reply_ids(&val.tags);
        let (media, string) = extract_and_remove_media_urls(&val.content);
        let mentions = extract_mentions(&string);

        Self {
            id: val.id,
            author: val.pubkey,
            content: string,
            media,
            created_at: val.created_at,
            mentions,
            replies_to,
        }
    }
}

impl From<&UnsignedEvent> for Message {
    fn from(val: &UnsignedEvent) -> Self {
        let replies_to = extract_reply_ids(&val.tags);
        let (media, string) = extract_and_remove_media_urls(&val.content);
        let mentions = extract_mentions(&string);

        Self {
            // Event ID must be known
            id: val.id.unwrap(),
            author: val.pubkey,
            content: string,
            media,
            created_at: val.created_at,
            mentions,
            replies_to,
        }
    }
}

impl From<&NewMessage> for Message {
    fn from(val: &NewMessage) -> Self {
        let replies_to = extract_reply_ids(&val.rumor.tags);
        let (media, string) = extract_and_remove_media_urls(&val.rumor.content);
        let mentions = extract_mentions(&string);

        Self {
            // Event ID must be known
            id: val.rumor.id.unwrap(),
            author: val.rumor.pubkey,
            content: string,
            media,
            created_at: val.rumor.created_at,
            mentions,
            replies_to,
        }
    }
}

impl Eq for Message {}

impl PartialEq for Message {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Ord for Message {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.created_at
            .cmp(&other.created_at)
            .then_with(|| self.id.cmp(&other.id))
    }
}

impl PartialOrd for Message {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Hash for Message {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

/// New message.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct NewMessage {
    pub room: u64,
    pub gift_wrap: EventId,
    pub historical: bool,
    pub rumor: UnsignedEvent,
}

impl NewMessage {
    pub fn new(gift_wrap: EventId, rumor: UnsignedEvent) -> Self {
        let room = rumor.uniq_id();

        Self {
            room,
            gift_wrap,
            historical: false,
            rumor,
        }
    }
}

/// Trash message.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FailedMessage {
    pub event_id: EventId,
    pub raw_event: SharedString,
    pub reason: SharedString,
}

impl FailedMessage {
    pub fn new<T>(event: &Event, reason: T) -> Self
    where
        T: Into<SharedString>,
    {
        Self {
            event_id: event.id,
            raw_event: SharedString::from(event.as_json()),
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Mention {
    pub public_key: PublicKey,
    pub range: Range<usize>,
}

impl Mention {
    pub fn new(public_key: PublicKey, range: Range<usize>) -> Self {
        Self { public_key, range }
    }
}

/// Extracts all mentions (public keys) from a content string.
fn extract_mentions(content: &str) -> Vec<Mention> {
    let parser = NostrParser::new();
    let tokens = parser.parse(content);

    tokens
        .filter_map(|token| match token.value {
            Nip21::Pubkey(public_key) => Some(Mention::new(public_key, token.range)),
            Nip21::Profile(profile) => Some(Mention::new(profile.public_key, token.range)),
            _ => None,
        })
        .collect()
}

/// Extracts all reply (ids) from the event tags.
fn extract_reply_ids(inner: &Tags) -> Vec<EventId> {
    let mut replies_to = vec![];

    for tag in inner.iter().filter(|tag| tag.kind() == "e") {
        if let Some(id) = tag.content().and_then(|id| EventId::parse(id).ok()) {
            replies_to.push(id);
        }
    }

    for tag in inner.iter().filter(|tag| tag.kind() == "q") {
        if let Some(id) = tag.content().and_then(|id| EventId::parse(id).ok()) {
            replies_to.push(id);
        }
    }

    replies_to
}

#[cfg(test)]
mod ordering_tests {
    use super::*;
    #[test]
    fn messages_in_the_same_second_are_not_deduplicated() {
        let keys = Keys::generate();
        let first = EventBuilder::new(Kind::PrivateDirectMessage, "first")
            .custom_created_at(Timestamp::from(100))
            .finalize(&keys)
            .unwrap();
        let second = EventBuilder::new(Kind::PrivateDirectMessage, "second")
            .custom_created_at(Timestamp::from(100))
            .finalize(&keys)
            .unwrap();
        let messages =
            std::collections::BTreeSet::from([Message::from(&first), Message::from(&second)]);
        assert_eq!(messages.len(), 2);
    }
}
