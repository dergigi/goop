//! Background room discovery and coalescing of repeated reload requests.
use std::collections::{HashMap, HashSet};

use anyhow::Result;
use common::EventExt;
use nostr_sdk::prelude::*;

use crate::{Room, RoomKind, cache::{self, RumorCache}, outgoing::OutgoingQueue};

#[derive(Debug, Default)]
pub(super) enum ReloadState {
    #[default]
    Idle,
    Loading,
    RequestedAgain,
}

impl ReloadState {
    /// True only when the caller should start a worker.
    pub fn request(&mut self) -> bool {
        match self {
            Self::Idle => { *self = Self::Loading; true }
            _ => { *self = Self::RequestedAgain; false }
        }
    }

    /// Finish one attempt, including failed attempts. Preserve a request that
    /// arrived while it was running, but collapse any burst into one retry.
    pub fn finish(&mut self) -> bool {
        let again = matches!(self, Self::RequestedAgain);
        *self = if again { Self::Loading } else { Self::Idle };
        again
    }
}

pub(super) struct LoadedRooms {
    pub rooms: HashSet<Room>,
    pub contacts: HashSet<PublicKey>,
}

pub(super) async fn load(
    client: Client,
    owner: PublicKey,
    cache: Option<RumorCache>,
    outgoing: Option<OutgoingQueue>,
) -> Result<LoadedRooms> {
    // A query failure must not replace the current contacts with an empty set.
    let contacts = client.database().query(
        Filter::new().author(owner).kind(Kind::ContactList).limit(1),
    ).await?.into_iter().next()
        .map(|event| event.tags.public_keys().collect()).unwrap_or_default();

    let mut summaries = RoomSummaries::default();
    if let Some(cache) = &cache {
        // all() already indexes incoming rumors. Do not index them a second time.
        for rumor in cache.all().await? {
            summaries.note(&rumor, owner);
        }
    }
    if let Some(outgoing) = outgoing {
        for rumor in outgoing.all_messages().await? {
            if let Some(cache) = &cache { cache.note(&rumor); }
            summaries.note(&rumor, owner);
        }
    }
    Ok(LoadedRooms { rooms: summaries.finish(owner, &contacts), contacts })
}

/// Keep only room metadata and evidence that the owner has sent a message.
/// Message bodies and reactions do not need to survive room classification.
#[derive(Default)]
struct RoomSummaries(HashMap<u64, (Room, bool)>);

impl RoomSummaries {
    fn note(&mut self, rumor: &UnsignedEvent, owner: PublicKey) {
        if !cache::is_chat(rumor.kind) { return; }
        let sent = rumor.pubkey == owner;
        self.0.entry(rumor.uniq_id()).and_modify(|(latest, user_sent)| {
            *user_sent |= sent;
            if rumor.created_at >= latest.created_at { *latest = Room::from(rumor); }
        }).or_insert_with(|| (Room::from(rumor), sent));
    }

    fn finish(self, owner: PublicKey, contacts: &HashSet<PublicKey>) -> HashSet<Room> {
        self.0.into_values().map(|(room, user_sent)| {
            let room = room.organize(&owner);
            if user_sent || room.members.iter().any(|key| contacts.contains(key)) {
                room.kind(RoomKind::Ongoing)
            } else { room }
        }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesces_bursts_without_losing_requests_during_a_reload() {
        let mut state = ReloadState::default();
        assert!(state.request());
        for _ in 0..100 { assert!(!state.request()); }
        assert!(state.finish());
        assert!(!state.request()); // Another request during the follow-up scan.
        assert!(state.finish());
        assert!(!state.finish());
        assert!(state.request()); // Success or failure both return to idle.
        assert!(!state.finish());
    }

    #[test]
    fn summaries_preserve_latest_metadata_and_older_outgoing_evidence() {
        let owner = Keys::generate().public_key();
        let peer = Keys::generate().public_key();
        let stranger = Keys::generate().public_key();
        let event = |author, recipient, time, subject: &str| {
            let mut event = EventBuilder::new(Kind::PrivateDirectMessage, "body")
                .tags([Tag::public_key(recipient), Tag::custom("subject", [subject])])
                .custom_created_at(Timestamp::from(time)).finalize_unsigned(author);
            event.ensure_id(); event
        };
        let old_outgoing = event(owner, peer, 1, "old");
        let incoming = event(peer, owner, 2, "new");
        let request = event(stranger, owner, 3, "request");
        let mut reaction = incoming.clone(); reaction.kind = Kind::Reaction;
        let mut summaries = RoomSummaries::default();
        for event in [&incoming, &old_outgoing, &request, &reaction] { summaries.note(event, owner); }
        assert_eq!(summaries.0.len(), 2);
        let rooms = summaries.finish(owner, &HashSet::new());
        let ongoing = rooms.iter().find(|room| room.id == incoming.uniq_id()).unwrap();
        assert_eq!(ongoing.kind, RoomKind::Ongoing);
        assert_eq!(ongoing.created_at, incoming.created_at);
        assert_eq!(ongoing.subject.as_deref(), Some("new"));
        assert_eq!(ongoing.members.last(), Some(&owner));
        assert_eq!(rooms.iter().find(|room| room.id == request.uniq_id()).unwrap().kind, RoomKind::Request);
        let mut summaries = RoomSummaries::default();
        summaries.note(&request, owner);
        assert_eq!(summaries.finish(owner, &HashSet::from([stranger])).iter().next().unwrap().kind, RoomKind::Ongoing);
    }
}
