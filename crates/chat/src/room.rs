use std::cmp::Ordering;
use std::hash::{Hash, Hasher};

use anyhow::{Error, anyhow};
use common::EventExt;
use device::DeviceRegistry;
use gpui::{App, AppContext, Context, EventEmitter, SharedString, Task};
use instant::Duration;
use itertools::Itertools;
use nostr_sdk::prelude::*;
use person::{Person, PersonRegistry};
use settings::{RoomConfig, SignerKind};
use state::{NostrRegistry, TIMEOUT, UniversalSigner};

use crate::NewMessage;

const NO_DEKEY: &str = "User hasn't set up a decoupled encryption key yet.";
const USER_NO_DEKEY: &str = "You haven't set up a decoupled encryption key or it's not available.";

#[derive(Debug, Clone)]
pub struct SendReport {
    pub receiver: PublicKey,
    pub gift_wrap_id: Option<EventId>,
    pub error: Option<SharedString>,
    pub output: Option<Output<EventId, EventSendStatus>>,
}

impl SendReport {
    pub fn new(receiver: PublicKey) -> Self {
        Self {
            receiver,
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
        self.error.is_none()
            && self
                .output
                .as_ref()
                .is_some_and(|o| o.success.is_empty() && o.failed.is_empty())
    }

    /// Returns true if the send was successful.
    pub fn success(&self) -> bool {
        self.error.is_none()
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
        let room_id = self.id.to_string();

        cx.background_spawn(async move {
            let filter = Filter::new()
                .kind(Kind::ApplicationSpecificData)
                .custom_tag(SingleLetterTag::LOWERCASE_R, room_id);

            let messages = client
                .database()
                .query(filter)
                .await?
                .into_iter()
                .filter_map(|event| UnsignedEvent::from_json(&event.content).ok())
                .sorted_by_key(|message| message.created_at)
                .collect();

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

    /// Select the appropriate signer based on signer kind and available keys.
    fn select_signer(
        signer_kind: &SignerKind,
        has_announcement: bool,
        encryption_signer: &Option<UniversalSigner>,
        user_signer: &UniversalSigner,
    ) -> UniversalSigner {
        match signer_kind {
            SignerKind::Auto => {
                if has_announcement {
                    encryption_signer
                        .clone()
                        .unwrap_or_else(|| user_signer.clone())
                } else {
                    user_signer.clone()
                }
            }
            SignerKind::Encryption => encryption_signer
                .clone()
                .expect("encryption signer must be set"),
            SignerKind::User => user_signer.clone(),
        }
    }

    /// Send rumor event to all members's messaging relays
    pub fn send(&self, rumor: UnsignedEvent, cx: &App) -> Option<Task<Vec<SendReport>>> {
        let config = self.config.clone();

        let device = DeviceRegistry::global(cx);
        let encryption_signer = device.read(cx).signer(cx);

        let nostr = NostrRegistry::global(cx);
        let client = nostr.read(cx).client();
        let user_signer = nostr.read(cx).signer();
        let current_user = nostr.read(cx).current_user()?;

        // Get sender's profile
        let persons = PersonRegistry::global(cx);
        let sender = persons.read(cx).get(&current_user, cx);

        // Get all members (excluding sender)
        let members: Vec<Person> = self
            .members
            .iter()
            .filter(|public_key| public_key != &&sender.public_key())
            .map(|member| persons.read(cx).get(member, cx))
            .collect();

        Some(cx.background_spawn(async move {
            let signer_kind = config.signer_kind();
            let backup = config.backup();

            let mut sents = 0;
            let mut reports = Vec::new();

            // Process each member
            for member in members {
                let announcement = member.announcement();
                let public_key = member.public_key();

                // Handle encryption signer requirements
                if signer_kind.encryption() {
                    // Receiver didn't set up a decoupled encryption key
                    if announcement.is_none() {
                        reports.push(SendReport::new(public_key).error(NO_DEKEY));
                        continue;
                    }

                    // Sender didn't set up a decoupled encryption key
                    if encryption_signer.is_none() {
                        reports.push(SendReport::new(sender.public_key()).error(USER_NO_DEKEY));
                        continue;
                    }
                }

                // Determine the signer to use
                let signer = Self::select_signer(
                    signer_kind,
                    announcement.is_some(),
                    &encryption_signer,
                    &user_signer,
                );

                // Send the gift wrap event and collect the report
                match send_gift_wrap(&client, &signer, &member, &rumor, signer_kind).await {
                    Ok(report) => {
                        if report.success() {
                            sents += 1;
                        }
                        reports.push(report);
                    }
                    Err(error) => {
                        let report = SendReport::new(public_key).error(error.to_string());
                        reports.push(report);
                    }
                }
            }

            // Send backup to current user if needed
            if backup && sents >= 1 {
                let public_key = sender.public_key();

                // Determine the signer to use
                let signer = Self::select_signer(
                    signer_kind,
                    sender.announcement().is_some(),
                    &encryption_signer,
                    &user_signer,
                );

                match send_gift_wrap(&client, &signer, &sender, &rumor, signer_kind).await {
                    Ok(report) => reports.push(report),
                    Err(error) => {
                        let report = SendReport::new(public_key).error(error.to_string());
                        reports.push(report);
                    }
                }
            }

            reports
        }))
    }
}

// Helper function to send a gift-wrapped event
async fn send_gift_wrap(
    client: &Client,
    signer: &UniversalSigner,
    receiver: &Person,
    rumor: &UnsignedEvent,
    config: &SignerKind,
) -> Result<SendReport, Error> {
    let k_tag = Tag::custom("k", vec!["14"]);
    let mut extra_tags = vec![k_tag];

    // Determine the receiver public key based on the config
    let receiver = match config {
        SignerKind::Auto => {
            if let Some(announcement) = receiver.announcement().as_ref() {
                extra_tags.push(Tag::public_key(receiver.public_key()));
                announcement.public_key()
            } else {
                receiver.public_key()
            }
        }
        SignerKind::Encryption => {
            if let Some(announcement) = receiver.announcement().as_ref() {
                extra_tags.push(Tag::public_key(receiver.public_key()));
                announcement.public_key()
            } else {
                return Err(anyhow!("User has no encryption announcement"));
            }
        }
        SignerKind::User => receiver.public_key(),
    };

    // Construct the gift wrap event
    let event = nip59::GiftWrapBuilder::new(receiver, rumor.clone())
        .extra_tags(extra_tags)
        .finalize_async(signer)
        .await?;

    publish_gift_wrap(client, receiver, &event).await
}

async fn publish_gift_wrap(
    client: &Client,
    receiver: PublicKey,
    event: &Event,
) -> Result<SendReport, Error> {
    // The SDK subscribes to acknowledgements before publishing and handles AUTH retries.
    let report = client
        .send_event(event)
        .to_nip17()
        .ack_policy(AckPolicy::all())
        .await
        .map(|output| {
            SendReport::new(receiver)
                .gift_wrap_id(event.id)
                .output(output)
        })?;

    Ok(report)
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
        let report = publish_gift_wrap(&client, recipient.public_key(), &event)
            .await
            .unwrap();
        assert!(report.success());
        assert!(!report.pending());
        assert!(!report.failed());
        let output = report.output.unwrap();
        assert_eq!(output.success.len(), 1);
        assert!(output.success[&inbox.url().await].is_ack());
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
