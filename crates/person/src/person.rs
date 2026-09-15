use std::cmp::Ordering;
use std::hash::{Hash, Hasher};

use gpui::SharedString;
use nostr_sdk::prelude::*;

/// Person
#[derive(Debug, Clone)]
pub struct Person {
    /// Public Key
    public_key: PublicKey,

    /// Metadata (profile)
    metadata: Metadata,

    /// None identifies a placeholder that still needs metadata.
    metadata_timestamp: Option<Timestamp>,


    /// Messaging relays
    messaging_relays: Vec<RelayUrl>,
}

impl PartialEq for Person {
    fn eq(&self, other: &Self) -> bool {
        self.public_key == other.public_key
    }
}

impl Eq for Person {}

impl PartialOrd for Person {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Person {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name().cmp(&other.name())
    }
}

impl Hash for Person {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.public_key.hash(state)
    }
}

impl From<PublicKey> for Person {
    fn from(public_key: PublicKey) -> Self {
        Self::new(public_key, Metadata::default())
    }
}

impl Person {
    pub fn new(public_key: PublicKey, metadata: Metadata) -> Self {
        Self {
            public_key,
            metadata_timestamp: (metadata != Metadata::default()).then(Timestamp::now),
            metadata,
            messaging_relays: vec![],
        }
    }

    pub(crate) fn from_metadata_event(event: &Event) -> anyhow::Result<Self> {
        let mut person = Self::new(event.pubkey, Metadata::from_json(&event.content)?);
        person.metadata_timestamp = Some(event.created_at);
        Ok(person)
    }

    pub(crate) fn has_metadata(&self) -> bool {
        self.metadata_timestamp.is_some()
    }

    /// Merge cached/relay metadata without replacing a newer profile or its relay data.
    pub(crate) fn merge_metadata(&mut self, incoming: &Self) -> bool {
        if incoming.metadata_timestamp.is_none()
            || incoming.metadata_timestamp < self.metadata_timestamp
        {
            return false;
        }
        let changed = self.metadata != incoming.metadata;
        self.metadata = incoming.metadata.clone();
        self.metadata_timestamp = incoming.metadata_timestamp;
        changed
    }

    /// Build profile messaging relays
    pub fn with_messaging_relays<I>(mut self, relays: I) -> Self
    where
        I: IntoIterator<Item = RelayUrl>,
    {
        self.messaging_relays = relays.into_iter().collect();
        self
    }

    /// Get profile public key
    pub fn public_key(&self) -> PublicKey {
        self.public_key
    }

    /// Get profile metadata
    pub fn metadata(&self) -> Metadata {
        self.metadata.clone()
    }

    /// Get profile messaging relays
    pub fn messaging_relays(&self) -> &Vec<RelayUrl> {
        &self.messaging_relays
    }

    /// Get relay hint for messaging relay list
    pub fn messaging_relay_hint(&self) -> Option<RelayUrl> {
        self.messaging_relays.first().cloned()
    }

    /// Get profile avatar
    pub fn avatar(&self) -> SharedString {
        self.metadata()
            .picture
            .as_ref()
            .filter(|picture| !picture.is_empty())
            .map(|picture| picture.into())
            .unwrap_or_else(|| "brand/avatar.png".into())
    }

    /// Get profile name
    pub fn name(&self) -> SharedString {
        if let Some(display_name) = self.metadata().display_name.as_ref()
            && !display_name.is_empty()
        {
            return SharedString::from(display_name.trim());
        }

        if let Some(name) = self.metadata().name.as_ref()
            && !name.is_empty()
        {
            return SharedString::from(name.trim());
        }

        SharedString::from(shorten_pubkey(self.public_key(), 4))
    }

    /// Set profile metadata
    pub fn set_metadata(&mut self, metadata: Metadata) {
        self.metadata = metadata;
        self.metadata_timestamp = Some(Timestamp::now());
    }

    /// Set profile messaging relays
    pub fn set_messaging_relays<I>(&mut self, relays: I)
    where
        I: IntoIterator<Item = RelayUrl>,
    {
        self.messaging_relays = relays.into_iter().collect();
    }
}

/// Shorten a [`PublicKey`] to a string with the first and last `len` characters
///
/// Ex. `00000000:00000002`
pub fn shorten_pubkey(public_key: PublicKey, len: usize) -> String {
    let Ok(pubkey) = public_key.to_bech32();

    format!(
        "{}...{}",
        &pubkey[0..(len + 1)],
        &pubkey[pubkey.len() - len..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(public_key: PublicKey, picture: Option<&str>, timestamp: u64) -> Person {
        let mut metadata = Metadata::default();
        metadata.picture = picture.map(str::to_owned);
        let mut person = Person::new(public_key, metadata);
        person.metadata_timestamp = Some(Timestamp::from(timestamp));
        person
    }

    #[test]
    fn cached_metadata_fills_relay_placeholder_without_losing_relays() {
        let key = Keys::generate().public_key();
        let relays = vec![RelayUrl::parse("wss://relay.example.com").unwrap()];
        let mut placeholder = Person::from(key).with_messaging_relays(relays.clone());
        assert!(!placeholder.has_metadata());
        assert!(placeholder.merge_metadata(&profile(
            key,
            Some("https://example.com/avatar.png"),
            10
        )));
        assert!(placeholder.has_metadata());
        assert_eq!(placeholder.messaging_relays, relays);
        assert_eq!(
            placeholder.metadata.picture.as_deref(),
            Some("https://example.com/avatar.png")
        );
    }

    #[test]
    fn late_cache_or_relay_response_cannot_replace_newer_avatar() {
        let key = Keys::generate().public_key();
        let mut current = profile(key, Some("https://example.com/new.png"), 20);
        assert!(!current.merge_metadata(&profile(key, Some("https://example.com/old.png"), 10)));
        assert!(!current.merge_metadata(&Person::from(key)));
        assert_eq!(
            current.metadata.picture.as_deref(),
            Some("https://example.com/new.png")
        );
    }

    #[test]
    fn newer_profile_can_remove_avatar() {
        let key = Keys::generate().public_key();
        let mut current = profile(key, Some("https://example.com/avatar.png"), 10);
        assert!(current.merge_metadata(&profile(key, None, 20)));
        assert!(current.metadata.picture.is_none());
        assert!(current.has_metadata());
    }
}
