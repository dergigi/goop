use std::cmp::Ordering;
use std::hash::{Hash, Hasher};

use anyhow::Error;
use common::EventExt;
use gpui::{App, AppContext, Context, EventEmitter, SharedString, Task};
use instant::Duration;
use itertools::Itertools;
use nostr_sdk::prelude::*;
use person::{Person, PersonRegistry};
use settings::{RoomConfig, SignerKind};
use state::{NostrRegistry, TIMEOUT};

use crate::NewMessage;

#[derive(Debug, Clone)]
pub struct SendReport {
    pub receiver: PublicKey,
    pub queued: bool,
    pub accepted: bool,
    pub self_copy: bool,
    pub gift_wrap_id: Option<EventId>,
    pub error: Option<SharedString>,
    pub output: Option<Output<EventId, EventSendStatus>>,
}

impl SendReport {
    pub fn new(receiver: PublicKey) -> Self {
        Self {
            receiver,
            queued: false,
            accepted: false,
            self_copy: false,
            gift_wrap_id: None,
            error: None,
            output: None,
        }
    }

    /// Set the gift wrap ID.
    pub fn gift_wrap_id(mut self, gift_wrap_id: EventId) -> Self {
        self.gift_wrap_id = Some(gift_wrap_id);
        self
    }

    /// Set the output.
    pub fn output(mut self, output: Output<EventId, EventSendStatus>) -> Self {
        self.output = Some(output);
        self
    }

    /// Set the error message.
    pub fn error<T>(mut self, error: T) -> Self
    where
        T: Into<SharedString>,
    {
        self.error = Some(error.into());
        self
    }

    /// Returns true if the send is pending.
    pub fn pending(&self) -> bool {
        self.queued
            || (self.error.is_none()
                && self
                    .output
                    .as_ref()
                    .is_some_and(|o| o.success.is_empty() && o.failed.is_empty()))
    }

    /// Returns true if the send was successful.
    pub fn success(&self) -> bool {
        self.accepted
            || self.error.is_none()
                && self
                    .output
                    .as_ref()
                    .is_some_and(|o| o.success.values().any(EventSendStatus::is_ack))
    }

    /// Returns true if the send failed.
    pub fn failed(&self) -> bool {
        self.error.is_some()
            || self
                .output
                .as_ref()
                .is_some_and(|o| o.success.is_empty() && !o.failed.is_empty())
    }
}

/// Room event.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RoomEvent {
    /// Incoming message.
    Incoming(NewMessage),
    /// Reloads the current room's messages.
    Reload,
}

/// Room kind.
#[derive(Clone, Copy, Hash, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum RoomKind {
    #[default]
    Request,
    Ongoing,
}

#[derive(Debug, Clone)]
pub struct Room {
    /// Conversation ID
    pub id: u64,

    /// The timestamp of the last message in the room
    pub created_at: Timestamp,

    /// Subject of the room
    pub subject: Option<SharedString>,

    /// All members of the room
    pub(super) members: Vec<PublicKey>,

    /// Kind
    pub kind: RoomKind,

    /// Configuration
    config: RoomConfig,
}

impl Ord for Room {
    fn cmp(&self, other: &Self) -> Ordering {
        self.created_at.cmp(&other.created_at)
    }
}

impl PartialOrd for Room {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Room {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Hash for Room {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl Eq for Room {}

impl EventEmitter<RoomEvent> for Room {}

impl From<&UnsignedEvent> for Room {
    fn from(val: &UnsignedEvent) -> Self {
        let id = val.uniq_id();
        let created_at = val.created_at;
        let members = val.extract_public_keys();
        let subject = val
            .tags
            .iter()
            .find(|tag| tag.kind() == "subject")
            .and_then(|tag| tag.content().map(|s| s.to_owned().into()));

        Room {
            id,
            created_at,
            subject,
            members,
            kind: RoomKind::default(),
            config: RoomConfig::new(),
        }
    }
}

impl From<UnsignedEvent> for Room {
    fn from(val: UnsignedEvent) -> Self {
        Room::from(&val)
    }
}

impl Room {
    /// Constructs a new room with the given receiver and tags.
    pub fn new<T>(author: PublicKey, receivers: T) -> Self
    where
        T: IntoIterator<Item = PublicKey>,
    {
        // Map receiver public keys to tags
        let tags = Tags::from_list(receivers.into_iter().map(Tag::public_key).collect());

        // Construct an unsigned event for a direct message
        //
        // WARNING: never sign this event
        let mut event = EventBuilder::new(Kind::PrivateDirectMessage, "")
            .tags(tags)
            .finalize_unsigned(author);

        // Ensure that the ID is set
        event.ensure_id();

        Room::from(&event)
    }

    /// Organizes the members of the room by moving the target member to the end.
    ///
    /// Always call this function to ensure the current user is at the end of the list.
    pub fn organize(mut self, target: &PublicKey) -> Self {
        if let Some(index) = self.members.iter().position(|member| member == target) {
            let member = self.members.remove(index);
            self.members.push(member);
        }
        self
    }

    /// Sets the kind of the room and returns the modified room
    pub fn kind(mut self, kind: RoomKind) -> Self {
        self.kind = kind;
        self
    }

    /// Sets this room is ongoing conversation
    pub fn set_ongoing(&mut self, cx: &mut Context<Self>) {
        self.kind = RoomKind::Ongoing;
        cx.notify();
    }

    /// Updates the creation timestamp of the room
    pub fn set_created_at(&mut self, created_at: impl Into<Timestamp>, cx: &mut Context<Self>) {
        self.created_at = created_at.into();
        cx.notify();
    }

    /// Updates the subject of the room
    pub fn set_subject<T>(&mut self, subject: T, cx: &mut Context<Self>)
    where
        T: Into<SharedString>,
    {
        self.subject = Some(subject.into());
        cx.notify();
    }

    /// Updates the signer kind config for the room
    pub fn set_signer_kind(&mut self, kind: &SignerKind, cx: &mut Context<Self>) {
        self.config.set_signer_kind(kind);
        cx.notify();
    }

    /// Updates the backup config for the room
    pub fn set_backup(&mut self, cx: &mut Context<Self>) {
        self.config.toggle_backup();
        cx.notify();
    }

    /// Returns the config of the room
    pub fn config(&self) -> &RoomConfig {
        &self.config
    }

    /// Returns the members of the room
    pub fn members(&self) -> &[PublicKey] {
        &self.members
    }

    /// Checks if the room has more than two members (group)
    pub fn is_group(&self) -> bool {
        self.members.len() > 2
    }

    /// Gets the display name for the room
    pub fn display_name(&self, cx: &App) -> SharedString {
        if let Some(value) = self.subject.clone() {
            value
        } else {
            self.merged_name(cx)
        }
    }

    /// Gets the display image for the room
    pub fn display_image(&self, cx: &App) -> SharedString {
        if !self.is_group() {
            self.display_member(cx).avatar()
        } else {
            SharedString::from("brand/group.png")
        }
    }

    /// Get a member to represent the room
    ///
    /// Display member is always different from the current user.
    pub fn display_member(&self, cx: &App) -> Person {
        let persons = PersonRegistry::global(cx);
        persons.read(cx).get(&self.members[0], cx)
    }

    /// Merge the names of the first two members of the room.
    fn merged_name(&self, cx: &App) -> SharedString {
        let persons = PersonRegistry::global(cx);

        if self.is_group() {
            let profiles: Vec<Person> = self
                .members
                .iter()
                .map(|public_key| persons.read(cx).get(public_key, cx))
                .collect();

            let mut name = profiles
                .iter()
                .take(2)
                .map(|p| p.name())
                .collect::<Vec<_>>()
                .join(", ");

            if profiles.len() > 3 {
                name = format!("{}, +{}", name, profiles.len() - 2);
            }

            SharedString::from(name)
        } else {
            self.display_member(cx).name()
        }
    }

    /// Push a new message to the current room
    pub fn push_message(&mut self, message: NewMessage, cx: &mut Context<Self>) {
        let created_at = message.rumor.created_at;
        let new_message = created_at > self.created_at;

        // Emit the incoming message event
        cx.emit(RoomEvent::Incoming(message));

        if new_message {
            self.set_created_at(created_at, cx);
        }
    }

    /// Emits a signal to reload the current room's messages.
    pub fn emit_refresh(&mut self, cx: &mut Context<Self>) {
        cx.emit(RoomEvent::Reload);
    }

    /// Get gossip relays for each member
    pub fn connect(&self, cx: &App) -> Task<Result<(), Error>> {
        let nostr = NostrRegistry::global(cx);
        let client = nostr.read(cx).client();
        let members = self.members().to_vec();

        cx.background_spawn(async move {
            let opts = SubscribeAutoCloseOptions::default()
                .exit_policy(ReqExitPolicy::ExitOnEOSE)
                .timeout(Some(Duration::from_secs(TIMEOUT)));

            let tasks: Vec<_> = members
                .into_iter()
                .map(|public_key| {
                    let client = client.clone();
                    async move {
                        let inbox = Filter::new()
                            .author(public_key)
                            .kind(Kind::InboxRelays)
                            .limit(1);

                        let announcement = Filter::new()
                            .author(public_key)
                            .kind(Kind::Custom(10044))
                            .limit(1);

                        client
                            .subscribe(vec![inbox, announcement])
                            .close_on(opts)
                            .await
                    }
                })
                .collect();

            for result in futures::future::join_all(tasks).await {
                result?;
            }

            Ok(())
        })
    }

    /// Get all messages belonging to the room
    pub fn get_messages(&self, cx: &App) -> Task<Result<Vec<UnsignedEvent>, Error>> {
        let nostr = NostrRegistry::global(cx);
        let client = nostr.read(cx).client();
        let room_id = self.id;
        let outgoing = crate::ChatRegistry::global(cx).read(cx).outgoing_queue();

        cx.background_spawn(async move {
            let filter = Filter::new()
                .kind(Kind::ApplicationSpecificData)
                .custom_tag(SingleLetterTag::LOWERCASE_R, room_id.to_string());

            let mut messages: Vec<_> = client
                .database()
                .query(filter)
                .await?
                .into_iter()
                .filter_map(|event| UnsignedEvent::from_json(&event.content).ok())
                .sorted_by_key(|message| message.created_at)
                .collect();

            if let Some(outgoing) = outgoing {
                messages.extend(outgoing.messages(room_id).await?);
                messages.sort_by_key(|message| (message.created_at, message.id));
                messages.dedup_by_key(|message| message.id);
            }
            Ok(messages)
        })
    }

    // Construct a rumor event for direct message
    pub fn rumor<S, I>(
        &self,
        content: S,
        replies: I,
        reaction: bool,
        cx: &App,
    ) -> Option<UnsignedEvent>
    where
        S: Into<String>,
        I: IntoIterator<Item = EventId>,
    {
        let kind = if reaction {
            Kind::Reaction
        } else {
            Kind::PrivateDirectMessage
        };

        let content: String = content.into();
        let replies: Vec<EventId> = replies.into_iter().collect();

        let persons = PersonRegistry::global(cx);
        let nostr = NostrRegistry::global(cx);

        // Get current user's public key
        let sender = nostr.read(cx).current_user()?;

        // Construct event's tags
        let mut tags = vec![];

        // Add subject tag if present
        if let Some(value) = self.subject.as_ref() {
            tags.push(Tag::custom("subject", vec![value.to_string()]));
        }

        // Add all reply tags
        for id in replies.into_iter() {
            tags.push(Tag::event(id))
        }

        // Add all receiver tags (no intermediate allocation)
        for public_key in self.members.iter().filter(|pk| *pk != &sender) {
            let member = persons.read(cx).get(public_key, cx);
            tags.push(Tag::from(Nip01Tag::PublicKey {
                public_key: member.public_key(),
                relay_hint: member.messaging_relay_hint(),
            }));
        }

        // Construct a direct message rumor event
        // WARNING: never sign and send this event to relays
        let mut event = EventBuilder::new(kind, content)
            .tags(tags)
            .finalize_unsigned(sender);

        // Ensure that the ID is set
        event.ensure_id();

        Some(event)
    }

    /// Persist an outgoing intent; the account worker owns signing and delivery.
    pub fn send(
        &self,
        rumor: UnsignedEvent,
        cx: &App,
    ) -> Option<Task<Result<Vec<SendReport>, Error>>> {
        let nostr = NostrRegistry::global(cx);
        let owner = nostr.read(cx).current_user()?;
        let queue = crate::ChatRegistry::global(cx).read(cx).outgoing_queue()?;
        let persons = PersonRegistry::global(cx);
        let destinations = self
            .members
            .iter()
            .copied()
            .filter(|key| *key != owner)
            .chain(std::iter::once(owner))
            .map(|key| {
                let person = persons.read(cx).get(&key, cx);
                crate::outgoing::Destination::new(
                    key,
                    person.announcement().map(|a| a.public_key()),
                    key == owner,
                )
            })
            .collect();
        let kind = self.config.signer_kind().clone();
        Some(cx.background_spawn(async move {
            let message = crate::outgoing::OutgoingMessage::new(owner, rumor, kind, destinations)?;
            queue.enqueue(message).await
        }))
    }
}

#[cfg(test)]
mod delivery_tests {
    use super::*;
    use nostr_gossip_memory::prelude::*;
    use nostr_sdk::local_relay::MockRelay;

    #[tokio::test]
    async fn immediate_inbox_acknowledgement_is_in_the_send_report() {
        let inbox = MockRelay::run().await.unwrap();
        let discovery = MockRelay::run().await.unwrap();
        let recipient = Keys::generate();
        discovery
            .add_event(
                InboxRelayList::new([inbox.url().await])
                    .finalize(&recipient)
                    .unwrap(),
            )
            .await
            .unwrap();
        let client = Client::builder()
            .gossip(NostrGossipMemory::unbounded())
            .gossip_config(
                GossipConfig::default()
                    .allowed(GossipAllowedRelays {
                        local: true,
                        without_tls: true,
                        ..Default::default()
                    })
                    .no_background_refresh(),
            )
            .build();
        client
            .add_relay(discovery.url().await)
            .capabilities(RelayCapabilities::DISCOVERY)
            .and_connect()
            .await
            .unwrap();
        let sender = Keys::generate();
        let rumor = EventBuilder::new(Kind::PrivateDirectMessage, "fast acknowledgement")
            .tag(Tag::public_key(recipient.public_key()))
            .finalize_unsigned(sender.public_key());
        let event = nip59::GiftWrapBuilder::new(recipient.public_key(), rumor)
            .finalize_async(&sender)
            .await
            .unwrap();
        // No UI listener or post-send registration: the relay responds immediately.
        let mut destination =
            crate::outgoing::Destination::new(recipient.public_key(), None, false);
        destination.wrap = Some(event.clone());
        crate::outgoing::publish(&client, &mut destination).await;
        let job = crate::outgoing::OutgoingMessage::new(
            sender.public_key(),
            EventBuilder::new(Kind::PrivateDirectMessage, "test")
                .finalize_unsigned(sender.public_key()),
            settings::SignerKind::User,
            vec![destination],
        )
        .unwrap();
        let report = job.reports().remove(0);
        assert!(report.success());
        assert!(!report.pending());
        assert!(!report.failed());
        let output = report.output.unwrap();
        assert_eq!(output.success.len(), 1);
        assert!(output.success.contains_key(&inbox.url().await));
        assert!(output.failed.is_empty());
        client.shutdown().await;
    }

    #[test]
    fn preparation_errors_are_failed_not_unknown() {
        let report = SendReport::new(Keys::generate().public_key()).error("Signer unavailable");
        assert!(report.failed());
        assert!(!report.success());
        assert!(!report.pending());
    }
}
