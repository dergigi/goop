use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, LazyLock, RwLock};

pub use actions::*;
use anyhow::{Context as AnyhowContext, Error};
use chat::{ChatRegistry, Message, Room, RoomEvent, SendReport};
use common::{TimestampExt, goop_cache};
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, ClipboardItem, Context, Entity, EventEmitter, FocusHandle,
    Focusable, InteractiveElement, IntoElement, ListAlignment, ListOffset, ListState, MouseButton,
    ObjectFit, ParentElement, PathPromptOptions, Render, SharedString, SharedUri,
    StatefulInteractiveElement, Styled, StyledImage, Subscription, SystemNotification,
    SystemNotificationAction, Task, WeakEntity, Window, div, img, list, px, red, relative, svg,
    white,
};
use itertools::Itertools;
use nostr_sdk::prelude::*;
use person::{Person, PersonRegistry};
use regex::Regex;
use settings::{AppSettings, SignerKind};
use smallvec::{SmallVec, smallvec};
use state::{NostrRegistry, upload};
use theme::ActiveTheme;
use ui::avatar::Avatar;
use ui::button::{Button, ButtonVariants};
use ui::dock::{Panel, PanelEvent};
use ui::input::{Input, InputEvent, InputState};
use ui::menu::DropdownMenu;
use ui::notification::Notification;
use ui::scroll::Scrollbar;
use ui::{
    Disableable, Icon, IconName, InteractiveElementExt, Sizable, StyledExt, WindowExtension,
    h_flex, v_flex,
};

use crate::text::RenderedText;

const REACTION_EMOJIS: &[&str] = &["👍", "👎", "😄", "🎉", "😕", "❤️", "🚀", "👀"];
const COMPACT_REACTION_EMOJIS: &[&str] = &["👍", "❤️", "👀"];

/// Regex matching strings that consist entirely of emoji characters,
/// zero-width joiners, variation selectors, and keycap combiners.
static EMOJI_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\p{Emoji}\u{200D}\u{FE0F}\u{20E3}]+$").unwrap());

mod actions;
mod text;

pub fn init(room: WeakEntity<Room>, window: &mut Window, cx: &mut App) -> Entity<ChatPanel> {
    cx.new(|cx| ChatPanel::new(room, window, cx))
}

/// Chat Panel
pub struct ChatPanel {
    id: SharedString,
    focus_handle: FocusHandle,

    /// Chat Room
    room: WeakEntity<Room>,

    /// Message list state
    list_state: ListState,

    /// All messages (sorted by created_at)
    messages: Vec<Message>,

    /// O(1) message lookup by EventId
    message_index: HashMap<EventId, usize>,

    /// All reactions
    reactions: BTreeMap<EventId, Vec<(SharedString, PublicKey)>>,

    render_markdown: bool,

    /// Mapping message ids to their rendered texts
    rendered_texts_by_id: BTreeMap<EventId, RenderedText>,

    /// Mapping message (rumor event) ids to their reports
    reports_by_id: Arc<RwLock<BTreeMap<EventId, Vec<SendReport>>>>,

    /// Chat input state
    input: Entity<InputState>,

    /// Subject input state
    subject_input: Entity<InputState>,

    /// Subject bar visibility
    subject_bar: Entity<bool>,

    /// Replies to
    replies_to: Entity<HashSet<EventId>>,

    /// Media Attachment
    attachments: Entity<Vec<Url>>,

    /// Upload state
    uploading: bool,

    /// Async operations
    tasks: Vec<Task<Result<(), Error>>>,

    /// Event subscriptions
    subscriptions: SmallVec<[Subscription; 3]>,
}

impl ChatPanel {
    pub fn new(room: WeakEntity<Room>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Define attachments and replies_to entities
        let attachments = cx.new(|_| vec![]);
        let replies_to = cx.new(|_| HashSet::new());
        let reports_by_id = Arc::new(RwLock::new(BTreeMap::new()));

        // Define list of messages
        let messages = Vec::new();
        let list_state = ListState::new(messages.len(), ListAlignment::Bottom, px(1024.));
        list_state.set_scroll_handler(|event, window, cx| {
            if event.is_scrolled && event.visible_range.start == 0 {
                window.defer(cx, |_, cx| {
                    ChatRegistry::global(cx).update(cx, |chat, cx| chat.ensure_history(cx))
                });
            }
        });

        // Get room id and name
        let (id, name) = room
            .read_with(cx, |this, _cx| {
                let id = this.id.to_string().into();
                let name = this.display_name(cx);

                (id, name)
            })
            .unwrap_or(("Unknown".into(), "Message...".into()));

        // Define input state
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(format!("Message {}", name))
                .auto_grow(1, 20)
                .submit_on_enter(true)
                .clean_on_escape()
        });

        // Define subject input state
        let subject_input = cx.new(|cx| InputState::new(window, cx).placeholder("New subject..."));
        let subject_bar = cx.new(|_cx| false);

        // Define subscriptions
        let mut subscriptions = smallvec![];
        subscriptions.push(cx.observe(&ChatRegistry::global(cx), |_, _, cx| cx.notify()));

        subscriptions.push(
            // Subscribe the chat input event
            cx.subscribe_in(&input, window, move |this, _input, event, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = event {
                    this.send_text_message(window, cx);
                };
            }),
        );

        subscriptions.push(
            // Subscribe the subject input event
            cx.subscribe_in(
                &subject_input,
                window,
                move |this, _input, event, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.change_subject(window, cx);
                    };
                },
            ),
        );

        subscriptions.push(cx.observe(&AppSettings::global(cx), |this, _, cx| {
            let markdown = AppSettings::get_render_markdown(cx);
            if this.render_markdown != markdown {
                this.render_markdown = markdown;
                this.rendered_texts_by_id.clear();
                let scroll_top = this.list_state.logical_scroll_top();
                this.list_state
                    .splice(0..this.messages.len(), this.messages.len());
                this.list_state.scroll_to(scroll_top);
                cx.notify();
            }
        }));

        // Define all functions that will run after the current cycle
        cx.defer_in(window, |this, window, cx| {
            this.connect(cx);
            this.subscribe_room_events(window, cx);
            this.get_messages(window, cx);
            ChatRegistry::global(cx).update(cx, |chat, cx| chat.ensure_history(cx));
        });

        Self {
            focus_handle: cx.focus_handle(),
            id,
            messages,
            message_index: HashMap::new(),
            reactions: BTreeMap::new(),
            room,
            list_state,
            input,
            subject_input,
            subject_bar,
            replies_to,
            attachments,
            render_markdown: AppSettings::get_render_markdown(cx),
            rendered_texts_by_id: BTreeMap::new(),
            reports_by_id,
            uploading: false,
            subscriptions,
            tasks: vec![],
        }
    }

    /// Get messaging relays and announcement for each member
    fn connect(&mut self, cx: &mut Context<Self>) {
        if let Some(room) = self.room.upgrade() {
            let task = room.read(cx).connect(cx);
            self.tasks.push(task);
        }
    }

    /// Subscribe to room events
    fn subscribe_room_events(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(room) = self.room.upgrade() else {
            return;
        };

        self.subscriptions.push(cx.subscribe_in(
            &room,
            window,
            move |this, _room, event, window, cx| {
                match event {
                    RoomEvent::Incoming(message) => {
                        if message.rumor.kind == Kind::Reaction {
                            this.insert_reaction(&message.rumor, cx);
                        } else {
                            this.insert_message(message, false, cx);

                            if !message.historical && !window.is_window_active() {
                                cx.show_system_notification(SystemNotification {
                                    tag: "message".into(),
                                    title: "New Message".into(),
                                    body: "You have a new message.".into(),
                                    actions: vec![SystemNotificationAction {
                                        id: "open".into(),
                                        label: "Open".into(),
                                    }],
                                });
                            }
                        }
                    }
                    RoomEvent::Reload => {
                        // Defer to avoid re-entrant read on Room while
                        // emit_refresh holds a write lock (via refresh_rooms).
                        cx.defer_in(window, |this, window, cx| {
                            this.get_messages(window, cx);
                        });
                    }
                };
            },
        ));
    }

    /// Load all messages belonging to this room
    fn get_messages(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Ok(get_messages) = self.room.read_with(cx, |this, cx| this.get_messages(cx)) else {
            return;
        };

        self.tasks.push(cx.spawn(async move |this, cx| {
            let events = get_messages.await?;

            // Update message list
            this.update(cx, |this, cx| {
                this.insert_messages(&events, cx);
            })?;

            Ok(())
        }));
    }

    /// Get user input content and merged all attachments if available
    fn get_input_value(&self, cx: &Context<Self>) -> String {
        // Get input's value
        let mut content = self.input.read(cx).value().trim().to_string();

        // Get all attaches and merge its with message
        let attachments = self.attachments.read(cx);

        if !attachments.is_empty() {
            let urls = attachments
                .iter()
                .map(|url| url.to_string())
                .collect_vec()
                .join("\n");

            if content.is_empty() {
                content = urls;
            } else {
                content = format!("{content}\n{urls}");
            }
        }

        content
    }

    fn change_subject(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let subject = self.subject_input.read(cx).value();

        self.room
            .update(cx, |this, cx| {
                this.set_subject(subject, cx);
            })
            .ok();
    }

    fn send_text_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Get the message which includes all attachments
        let content = self.get_input_value(cx);

        // Get the replies to this message
        let replies: Vec<EventId> = self.replies_to.read(cx).iter().copied().collect();

        // Return if message is empty
        if content.trim().is_empty() {
            window.push_notification("Cannot send an empty message", cx);
            return;
        }

        // If replying to exactly one message with only a valid emoji,
        // send as a reaction instead of a text message
        if replies.len() == 1 && EMOJI_RE.is_match(&content) && self.attachments.read(cx).is_empty()
        {
            for reply in &replies {
                self.send_reaction(&content, reply, window, cx);
            }
            self.clear(window, cx);
            return;
        }

        self.send_message(&content, replies, false, window, cx);
    }

    fn send_reaction(
        &mut self,
        emoji: &str,
        target: &EventId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Return if emoji is empty
        if emoji.trim().is_empty() {
            window.push_notification("Cannot send an empty reaction", cx);
            return;
        }

        self.send_message(emoji, vec![*target], true, window, cx);
    }

    /// Send a message to all members of the chat
    fn send_message(
        &mut self,
        value: &str,
        replies: Vec<EventId>,
        reaction: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if value.trim().is_empty() {
            window.push_notification("Cannot send an empty message", cx);
            return;
        }

        let room = self.room.clone();
        let content = value.to_string();

        // Upgrade room and create rumor + send task in a single read lock
        let Some(room_entity) = room.upgrade() else {
            return;
        };

        // Create rumor and send task
        let (rumor, send_task) = match room_entity.read_with(cx, |room, cx| {
            let rumor = room.rumor(content.clone(), replies.clone(), reaction, cx)?;
            let send_task = room.send(rumor.clone(), cx)?;
            Some((rumor, send_task))
        }) {
            Some(pair) => pair,
            None => {
                window.push_notification("Failed to create message", cx);
                return;
            }
        };

        let id = rumor.id.expect("rumor must have an id");

        // Insert optimistic message and clear input
        if rumor.kind != Kind::Reaction {
            self.insert_message(&rumor, true, cx);
            self.clear(window, cx);
        } else {
            self.insert_reaction(&rumor, cx);
        }

        // Update reports
        self.insert_reports(id, vec![], cx);

        // Spawn a single task to await the send and update reports
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            let outputs = send_task.await;

            this.update(cx, |this, cx| {
                this.insert_reports(id, outputs, cx);
            })?;

            Ok(())
        }));
    }

    /// Clear the input field, attachments, and replies
    ///
    /// Only run after sending a message
    fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |this, cx| {
            this.set_value("", window, cx);
        });
        self.attachments.update(cx, |this, cx| {
            this.clear();
            cx.notify();
        });
        self.replies_to.update(cx, |this, cx| {
            this.clear();
            cx.notify();
        })
    }

    /// Insert reports
    fn insert_reports(&mut self, id: EventId, reports: Vec<SendReport>, cx: &mut Context<Self>) {
        self.reports_by_id
            .write()
            .unwrap()
            .entry(id)
            .or_default()
            .extend(reports);
        cx.notify();
    }

    /// Insert a message into the chat panel
    fn insert_message<E>(&mut self, m: E, scroll: bool, cx: &mut Context<Self>)
    where
        E: Into<Message>,
    {
        let msg: Message = m.into();

        if let Err(pos) = self.messages.binary_search(&msg) {
            self.messages.insert(pos, msg);
            // Rebuild message index after insertion (indices from pos to end shift)
            for (i, message) in self.messages.iter().enumerate().skip(pos) {
                self.message_index.insert(message.id, i);
            }
            self.list_state.splice(pos..pos, 1);

            if scroll {
                self.list_state.scroll_to(ListOffset {
                    item_ix: self.list_state.item_count(),
                    offset_in_item: px(0.0),
                });
            }

            cx.notify();
        }
    }

    /// Convert and insert a vector of nostr events into the chat panel
    fn insert_messages(&mut self, events: &[UnsignedEvent], cx: &mut Context<Self>) {
        for event in events.iter() {
            if event.kind == Kind::Reaction {
                self.insert_reaction(event, cx);
                continue;
            }
            // Bulk inserting messages, so no need to scroll to the latest message
            self.insert_message(event, false, cx);
        }
    }

    /// Insert a reaction into the chat panel
    fn insert_reaction(&mut self, event: &UnsignedEvent, cx: &mut Context<Self>) {
        if event.kind != Kind::Reaction {
            return;
        }

        for id in event.tags.event_ids() {
            self.reactions
                .entry(id)
                .or_default()
                .push((SharedString::from(&event.content), event.pubkey));
        }

        cx.notify();
    }

    /// Check if a message has any reports
    fn has_reports(&self, id: &EventId) -> bool {
        self.reports_by_id.read().unwrap().contains_key(id)
    }

    /// Clone reports for a message (used for modal display, not called during render)
    fn sent_reports(&self, id: &EventId) -> Option<Vec<SendReport>> {
        self.reports_by_id.read().unwrap().get(id).cloned()
    }

    /// Get a message by its ID (O(1) lookup)
    fn message(&self, id: &EventId) -> Option<&Message> {
        self.message_index
            .get(id)
            .and_then(|&ix| self.messages.get(ix))
    }

    /// Get a reaction by its target ID (returns reference, no allocation)
    fn reaction(&self, id: &EventId) -> &[(SharedString, PublicKey)] {
        self.reactions.get(id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Check if a message has any reactions
    fn has_reaction(&self, id: &EventId) -> bool {
        self.reactions.contains_key(id)
    }

    /// Scroll to a message by its ID
    fn scroll_to(&self, id: &EventId) {
        if let Some(ix) = self.messages.iter().position(|msg| &msg.id == id) {
            self.list_state.scroll_to_reveal_item(ix);
        }
    }

    fn copy_author(&self, public_key: &PublicKey, cx: &App) {
        let content = public_key.to_bech32().unwrap();
        let item = ClipboardItem::new_string(content);

        cx.write_to_clipboard(item);
    }

    fn copy_message(&self, id: &EventId, cx: &App) {
        let Some(message) = self.message(id) else {
            return;
        };
        let content = message.content.to_string();
        let item = ClipboardItem::new_string(content);

        cx.write_to_clipboard(item);
    }

    fn reply_to(&mut self, id: &EventId, cx: &mut Context<Self>) {
        if let Some(text) = self.message(id) {
            self.replies_to.update(cx, |this, cx| {
                this.insert(text.id);
                cx.notify();
            });
        }
    }

    fn remove_reply(&mut self, id: &EventId, cx: &mut Context<Self>) {
        self.replies_to.update(cx, |this, cx| {
            this.remove(id);
            cx.notify();
        });
    }

    fn upload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Get the user's configured blossom server
        let server = AppSettings::get_file_server(cx);

        // Ask user for file upload
        let path = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });

        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            this.update(cx, |this, cx| {
                this.set_uploading(true, cx);
            })?;

            let mut paths = path.await??.context("Not found")?;
            let path = paths.pop().context("No path")?;

            // Upload via blossom client
            match upload(server, path, cx).await {
                Ok(url) => {
                    this.update_in(cx, |this, _window, cx| {
                        this.add_attachment(url, cx);
                        this.set_uploading(false, cx);
                    })?;
                }
                Err(e) => {
                    this.update_in(cx, |this, window, cx| {
                        this.set_uploading(false, cx);
                        window.push_notification(
                            Notification::error(e.to_string()).autohide(false),
                            cx,
                        );
                    })?;
                }
            }

            Ok(())
        }));
    }

    fn set_uploading(&mut self, uploading: bool, cx: &mut Context<Self>) {
        self.uploading = uploading;
        cx.notify();
    }

    fn add_attachment(&mut self, url: Url, cx: &mut Context<Self>) {
        self.attachments.update(cx, |this, cx| {
            this.push(url);
            cx.notify();
        });
    }

    fn remove_attachment(&mut self, url: &Url, _window: &mut Window, cx: &mut Context<Self>) {
        self.attachments.update(cx, |this, cx| {
            if let Some(ix) = this.iter().position(|this| this == url) {
                this.remove(ix);
                cx.notify();
            }
        });
    }

    fn profile(&self, public_key: &PublicKey, cx: &App) -> Person {
        let persons = PersonRegistry::global(cx);
        persons.read(cx).get(public_key, cx)
    }

    fn on_command(&mut self, command: &Command, window: &mut Window, cx: &mut Context<Self>) {
        match command {
            Command::Insert(content) => {
                self.input.update(cx, |this, cx| {
                    let new_value = format!("{} {}", this.value(), content);
                    this.set_value(new_value, window, cx);
                });
            }
            Command::ChangeSubject(subject) => {
                if self
                    .room
                    .update(cx, |this, cx| {
                        this.set_subject(subject, cx);
                    })
                    .is_err()
                {
                    window.push_notification(Notification::error("Failed to change subject"), cx);
                }
            }
            Command::ChangeSigner(kind) => {
                let settings = AppSettings::global(cx);
                let is_nip4e_enabled = settings.read(cx).is_nip4e_enabled(cx);
                let is_force_nip4e = *kind == SignerKind::Encryption || *kind == SignerKind::Auto;

                if !is_nip4e_enabled && is_force_nip4e {
                    window.push_notification("Decoupling Encryption Key is not enabled", cx);
                    return;
                }

                if self
                    .room
                    .update(cx, |this, cx| {
                        this.set_signer_kind(kind, cx);
                    })
                    .is_err()
                {
                    window.push_notification(Notification::error("Failed to change signer"), cx);
                }
            }
            Command::ToggleBackup => {
                if self
                    .room
                    .update(cx, |this, cx| {
                        this.set_backup(cx);
                    })
                    .is_err()
                {
                    window.push_notification(Notification::error("Failed to toggle backup"), cx);
                }
            }
            Command::Copy(public_key) => {
                self.copy_author(public_key, cx);
            }
            Command::Relays(public_key) => {
                self.open_relays(public_key, window, cx);
            }
            Command::Njump(public_key) => {
                self.open_njump(public_key, cx);
            }
            Command::Trace(id) => {
                self.open_trace(id, window, cx);
            }
        }
    }

    fn open_trace(&mut self, id: &EventId, window: &mut Window, cx: &mut Context<Self>) {
        let chat = ChatRegistry::global(cx);
        let seen_on = chat.read(cx).rumor_seen_on(id);

        window.open_modal(cx, move |this, _window, cx| {
            this.title("Seen on").show_close(true).child(
                v_flex()
                    .gap_1()
                    .when_none(&seen_on, |this| {
                        this.child(
                            h_flex()
                                .h_10()
                                .justify_center()
                                .text_sm()
                                .bg(cx.theme().elevated_surface_background)
                                .rounded(cx.theme().radius)
                                .child("Message isn't traced yet"),
                        )
                    })
                    .when_some(seen_on.as_ref(), |this, relays| {
                        this.children({
                            let mut items = vec![];

                            for url in relays.iter() {
                                items.push(
                                    h_flex()
                                        .h_7()
                                        .px_2()
                                        .gap_2()
                                        .bg(cx.theme().elevated_surface_background)
                                        .rounded(cx.theme().radius)
                                        .text_sm()
                                        .child(div().size_1p5().rounded_full().bg(gpui::green()))
                                        .child(SharedString::from(url.to_string())),
                                );
                            }

                            items
                        })
                    }),
            )
        });
    }

    fn open_relays(&mut self, public_key: &PublicKey, window: &mut Window, cx: &mut Context<Self>) {
        let profile = self.profile(public_key, cx);

        window.open_modal(cx, move |this, _window, cx| {
            let relays = profile.messaging_relays();

            this.title("Messaging Relays")
                .show_close(true)
                .child(v_flex().gap_1().children({
                    let mut items = vec![];

                    for url in relays.iter() {
                        items.push(
                            h_flex()
                                .h_7()
                                .px_2()
                                .gap_2()
                                .bg(cx.theme().elevated_surface_background)
                                .rounded(cx.theme().radius)
                                .text_sm()
                                .child(div().size_1p5().rounded_full().bg(gpui::green()))
                                .child(SharedString::from(url.to_string())),
                        );
                    }

                    items
                }))
        });
    }

    fn open_njump(&mut self, public_key: &PublicKey, cx: &mut Context<Self>) {
        let content = format!("https://njump.me/{}", public_key.to_bech32().unwrap());
        cx.open_url(&content);
    }

    pub fn focus_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.focus(window, cx));
    }

    fn render_history_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let chat = ChatRegistry::global(cx);
        let chat = chat.read(cx);
        h_flex()
            .px_3()
            .py_1()
            .gap_2()
            .text_xs()
            .text_color(cx.theme().text_muted)
            .child(div().flex_1().child(chat.history_summary(cx)))
            .child(
                Button::new("older-history")
                    .label("Load older history")
                    .small()
                    .ghost()
                    .disabled(chat.history_running())
                    .on_click(|_, _, cx| {
                        ChatRegistry::global(cx).update(cx, |chat, cx| chat.load_older_history(cx))
                    }),
            )
            .child(
                Button::new("retry-decryption")
                    .label("Retry failed")
                    .small()
                    .ghost()
                    .disabled(chat.count_trash_messages(cx) == 0)
                    .on_click(|_, _, cx| {
                        ChatRegistry::global(cx)
                            .update(cx, |chat, cx| chat.retry_failed_messages(cx))
                    }),
            )
    }

    fn render_announcement(&self, cx: &Context<Self>) -> AnyElement {
        const MSG: &str =
            "This conversation is private. Only members can see each other's messages.";

        v_flex()
            .h_40()
            .w_full()
            .gap_3()
            .p_3()
            .items_center()
            .justify_center()
            .text_center()
            .text_xs()
            .text_color(cx.theme().text_placeholder)
            .line_height(relative(1.3))
            .child(
                svg()
                    .path("brand/goop.svg")
                    .size_12()
                    .text_color(cx.theme().ghost_element_active),
            )
            .child(MSG)
            .into_any_element()
    }

    fn render_warning(&self, ix: usize, content: SharedString, cx: &Context<Self>) -> AnyElement {
        div()
            .id(ix)
            .w_full()
            .py_2()
            .px_3()
            .child(
                h_flex()
                    .w_full()
                    .gap_3()
                    .text_sm()
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .size_8()
                            .justify_center()
                            .rounded_full()
                            .bg(cx.theme().warning_background)
                            .text_color(cx.theme().warning_foreground)
                            .child(Icon::new(IconName::Warning).small()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .w_full()
                            .flex_initial()
                            .overflow_hidden()
                            .child(content),
                    ),
            )
            .into_any_element()
    }

    fn is_group_start(&self, ix: usize) -> bool {
        // 5 minutes
        const GROUP_WINDOW: u64 = 300;

        if ix == 0 {
            return true;
        }

        if let Some(previous) = self.messages.get(ix - 1)
            && let Some(current) = self.messages.get(ix)
        {
            if current.author != previous.author {
                return true;
            }

            let gap = current
                .created_at
                .as_secs()
                .saturating_sub(previous.created_at.as_secs());

            return gap > GROUP_WINDOW;
        }

        false
    }

    fn render_message(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(message) = self.messages.get(ix) {
            let persons = PersonRegistry::global(cx);
            let show_author = self.is_group_start(ix);
            let text = self
                .rendered_texts_by_id
                .entry(message.id)
                .or_insert_with(|| {
                    RenderedText::new(
                        &message.content,
                        &message.mentions,
                        &persons,
                        self.render_markdown,
                        cx,
                    )
                })
                .element(ix.into(), window, cx);

            self.render_text_message(ix, message, text, show_author, cx)
        } else {
            self.render_warning(ix, SharedString::from("Message not found"), cx)
        }
    }

    fn render_text_message(
        &self,
        ix: usize,
        message: &Message,
        rendered_text: AnyElement,
        show_author: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let id = message.id;
        let author = self.profile(&message.author, cx);
        let pk = author.public_key();

        let replies = message.replies_to.as_slice();
        let has_replies = !replies.is_empty();
        let has_reactions = self.has_reaction(&id);
        let has_reports = self.has_reports(&id);

        // Hide avatar setting
        let hide_avatar = AppSettings::get_hide_avatar(cx);

        div()
            .id(ix)
            .group("")
            .relative()
            .w_full()
            .py_1()
            .px_3()
            .child(
                div()
                    .flex()
                    .gap_3()
                    .when(!hide_avatar, |this| {
                        if show_author {
                            this.child(
                                Avatar::new(author.avatar())
                                    .flex_shrink_0()
                                    .relative()
                                    .dropdown_menu(move |this, _window, _cx| {
                                        this.menu("Public Key", Box::new(Command::Copy(pk)))
                                            .menu("View Relays", Box::new(Command::Relays(pk)))
                                            .separator()
                                            .menu("View on njump.me", Box::new(Command::Njump(pk)))
                                    }),
                            )
                        } else {
                            this.child(div().flex_shrink_0().w(px(32.)))
                        }
                    })
                    .child(
                        v_flex()
                            .flex_1()
                            .w_full()
                            .flex_initial()
                            .overflow_hidden()
                            .when(show_author, |this| {
                                this.child(
                                    h_flex()
                                        .gap_2()
                                        .text_sm()
                                        .text_color(cx.theme().text_placeholder)
                                        .child(div().font_semibold().child(author.name()))
                                        .child(message.created_at.to_human_time())
                                        .when(has_reports, |this| {
                                            this.child(self.render_sent_reports(&id, cx))
                                        }),
                                )
                            })
                            .when(has_replies, |this| {
                                this.children(self.render_message_replies(replies, cx))
                            })
                            .child(rendered_text)
                            .child(self.render_media(&message.media, cx))
                            .when(has_reactions, |this| {
                                this.child(self.render_reactions(&id, cx))
                            }),
                    ),
            )
            .child(
                div()
                    .group_hover("", |this| this.bg(cx.theme().element_active))
                    .absolute()
                    .left_0()
                    .top_0()
                    .w(px(2.))
                    .h_full()
                    .bg(cx.theme().border_transparent),
            )
            .child(self.render_actions(&id, &pk, cx))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _, _window, cx| {
                    this.copy_message(&id, cx);
                }),
            )
            .on_double_click(cx.listener(move |this, _, _window, cx| {
                this.reply_to(&id, cx);
            }))
            .hover(|this| this.bg(cx.theme().surface_background))
            .into_any_element()
    }

    fn render_media(&self, media: &[SharedUri], cx: &Context<Self>) -> impl IntoElement {
        // No media: return empty div
        if media.is_empty() {
            return div();
        };

        // Single media item: render full-width image
        if media.len() == 1 {
            return div().child(
                img(media[0].clone())
                    .border_1()
                    .border_color(cx.theme().border_variant)
                    .h(px(250.))
                    .object_fit(ObjectFit::Cover)
                    .rounded(cx.theme().radius),
            );
        }

        // Multiple media items: render in a row
        div()
            .w_full()
            .flex_1()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap_2()
            .children({
                let mut items = vec![];

                for (ix, item) in media.iter().enumerate() {
                    items.push(
                        div()
                            .id(format!("media-{ix}"))
                            .flex_grow_0()
                            .flex_shrink_0()
                            .child(
                                img(item.clone())
                                    .h_32()
                                    .border_1()
                                    .border_color(cx.theme().border_variant)
                                    .rounded(cx.theme().radius),
                            ),
                    );
                }

                items
            })
    }

    fn render_message_replies(
        &self,
        replies: &[EventId],
        cx: &Context<Self>,
    ) -> impl IntoIterator<Item = impl IntoElement> {
        let mut items = Vec::with_capacity(replies.len());

        for (ix, id) in replies.iter().enumerate() {
            let Some(message) = self.message(id) else {
                continue;
            };
            let author = self.profile(&message.author, cx);

            items.push(
                div()
                    .id(ix)
                    .w_full()
                    .px_2()
                    .border_l_2()
                    .border_color(cx.theme().element_active)
                    .text_sm()
                    .child(div().font_semibold().child(author.name()))
                    .child(
                        div()
                            .w_full()
                            .text_ellipsis()
                            .line_clamp(1)
                            .child(SharedString::from(&message.content)),
                    )
                    .hover(|this| this.bg(cx.theme().elevated_surface_background))
                    .on_click({
                        let id = *id;
                        cx.listener(move |this, _event, _window, _cx| {
                            this.scroll_to(&id);
                        })
                    }),
            );
        }

        items
    }

    fn render_reactions(&self, id: &EventId, cx: &App) -> impl IntoElement {
        let current_user = NostrRegistry::global(cx).read(cx).current_user();
        let reactions = self.reaction(id);

        // Group reactions by emoji and collect authors for each
        let mut grouped: BTreeMap<SharedString, Vec<PublicKey>> = BTreeMap::new();
        for (emoji, author) in reactions {
            grouped.entry(emoji.clone()).or_default().push(*author);
        }

        h_flex()
            .mt_2()
            .gap_1()
            .children(grouped.into_iter().map(|(emoji, authors)| {
                let count = authors.len();
                let has_reacted = current_user
                    .map(|pk| authors.contains(&pk))
                    .unwrap_or(false);

                h_flex()
                    .gap_2()
                    .py_0p5()
                    .px_1()
                    .rounded(cx.theme().radius)
                    .text_xs()
                    .border_1()
                    .when(has_reacted, |this| {
                        this.text_color(cx.theme().secondary_foreground)
                            .bg(cx.theme().secondary_background)
                            .border_color(cx.theme().secondary_active)
                    })
                    .when(!has_reacted, |this| this.border_color(cx.theme().border))
                    .child(emoji)
                    .child(SharedString::from(count.to_string()))
            }))
    }

    fn render_sent_reports(&self, id: &EventId, cx: &App) -> impl IntoElement {
        let reports = self.sent_reports(id);

        let pending = reports
            .as_ref()
            .is_some_and(|reports| reports.is_empty() || reports.iter().any(|r| r.pending()));

        let success = reports
            .as_ref()
            .is_some_and(|reports| !reports.is_empty() && reports.iter().any(|r| r.success()));

        let failed = reports
            .as_ref()
            .is_some_and(|reports| !reports.is_empty() && reports.iter().all(|r| r.failed()));

        let label = if success {
            SharedString::from("• Sent")
        } else if failed {
            SharedString::from("• Error")
        } else if pending {
            SharedString::from("• Sending...")
        } else {
            SharedString::from("• Unknown")
        };

        div()
            .id(SharedString::from(id.to_hex()))
            .child(label)
            .when(failed, |this| this.text_color(cx.theme().text_danger))
            .when_some(reports, |this, reports| {
                this.when(!pending, |this| {
                    this.on_click(move |_e, window, cx| {
                        let reports = reports.clone();

                        window.open_modal(cx, move |this, _window, cx| {
                            this.title(SharedString::from("Sent Reports"))
                                .show_close(true)
                                .child(v_flex().gap_4().children({
                                    let mut items = Vec::with_capacity(reports.len());

                                    for report in reports.iter() {
                                        items.push(Self::render_report(report, cx))
                                    }

                                    items
                                }))
                        });
                    })
                })
            })
    }

    fn render_report(report: &SendReport, cx: &App) -> impl IntoElement {
        let persons = PersonRegistry::global(cx);
        let profile = persons.read(cx).get(&report.receiver, cx);
        let name = profile.name();
        let avatar = profile.avatar();

        v_flex()
            .gap_2()
            .w_full()
            .child(
                h_flex()
                    .gap_2()
                    .text_sm()
                    .child(SharedString::from("Sent to:"))
                    .child(
                        h_flex()
                            .gap_1()
                            .font_semibold()
                            .child(Avatar::new(avatar).small())
                            .child(name.clone()),
                    ),
            )
            .when_some(report.error.clone(), |this, error| {
                this.child(
                    h_flex()
                        .flex_wrap()
                        .justify_center()
                        .p_1()
                        .h_16()
                        .w_full()
                        .text_sm()
                        .rounded(cx.theme().radius)
                        .bg(cx.theme().warning_background)
                        .text_color(cx.theme().warning_foreground)
                        .child(div().flex_1().w_full().text_center().child(error)),
                )
            })
            .when_some(report.output.clone(), |this, output| {
                this.child(
                    v_flex()
                        .gap_2()
                        .w_full()
                        .children({
                            let mut items = Vec::with_capacity(output.failed.len());

                            for (url, msg) in output.failed.into_iter() {
                                items.push(
                                    v_flex()
                                        .gap_0p5()
                                        .p_1()
                                        .w_full()
                                        .rounded(cx.theme().radius)
                                        .bg(cx.theme().danger_background)
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_semibold()
                                                .line_height(relative(1.25))
                                                .child(SharedString::from(url.to_string())),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().danger_foreground)
                                                .line_height(relative(1.25))
                                                .child(SharedString::from(msg.to_string())),
                                        ),
                                )
                            }

                            items
                        })
                        .children({
                            let mut items = Vec::with_capacity(output.success.len());

                            for url in output.success.into_iter() {
                                items.push(
                                    v_flex()
                                        .gap_0p5()
                                        .p_1()
                                        .w_full()
                                        .rounded(cx.theme().radius)
                                        .bg(cx.theme().elevated_surface_background)
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_semibold()
                                                .line_height(relative(1.25))
                                                .child(SharedString::from(url.0.to_string())),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .line_height(relative(1.25))
                                                .child(SharedString::from("Successfully")),
                                        ),
                                )
                            }

                            items
                        }),
                )
            })
    }

    fn render_actions(
        &self,
        id: &EventId,
        public_key: &PublicKey,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .p_0p5()
            .gap_1()
            .invisible()
            .absolute()
            .right_4()
            .top_neg_2()
            .when(cx.theme().shadow, |this| this.shadow_sm())
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .children({
                let mut items = vec![];

                for emoji in COMPACT_REACTION_EMOJIS {
                    items.push(
                        Button::new(*emoji)
                            .label(*emoji)
                            .tooltip(*emoji)
                            .small()
                            .ghost()
                            .on_click({
                                let emoji = *emoji;
                                let id = *id;
                                cx.listener(move |this, _event, window, cx| {
                                    this.send_reaction(emoji, &id, window, cx);
                                })
                            }),
                    );
                }

                items
            })
            .child(div().flex_shrink_0().h_4().w_px().bg(cx.theme().border))
            .child(
                Button::new("reply")
                    .icon(IconName::Reply)
                    .tooltip("Reply")
                    .small()
                    .ghost()
                    .on_click({
                        let id = id.to_owned();
                        cx.listener(move |this, _event, _window, cx| {
                            this.reply_to(&id, cx);
                        })
                    }),
            )
            .child(
                Button::new("copy")
                    .icon(IconName::Copy)
                    .tooltip("Copy")
                    .small()
                    .ghost()
                    .on_click({
                        let id = id.to_owned();
                        cx.listener(move |this, _event, _window, cx| {
                            this.copy_message(&id, cx);
                        })
                    }),
            )
            .child(div().flex_shrink_0().h_4().w_px().bg(cx.theme().border))
            .child(
                Button::new("advance")
                    .icon(IconName::Ellipsis)
                    .small()
                    .ghost()
                    .dropdown_menu({
                        let public_key = *public_key;
                        let id = *id;
                        move |this, _window, _cx| {
                            this.menu("Copy author", Box::new(Command::Copy(public_key)))
                                .menu("Seen on", Box::new(Command::Trace(id)))
                        }
                    }),
            )
            .group_hover("", |this| this.visible())
    }

    fn render_attachment(&self, url: &Url, cx: &Context<Self>) -> impl IntoElement {
        div()
            .id(SharedString::from(url.to_string()))
            .relative()
            .w_16()
            .child(
                img(url.as_str())
                    .size_16()
                    .when(cx.theme().shadow, |this| this.shadow_lg())
                    .rounded(cx.theme().radius)
                    .object_fit(ObjectFit::ScaleDown),
            )
            .child(
                div()
                    .absolute()
                    .top_neg_2()
                    .right_neg_2()
                    .size_4()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(red())
                    .child(Icon::new(IconName::Close).size_2().text_color(white())),
            )
            .on_click({
                let url = url.clone();
                cx.listener(move |this, _, window, cx| {
                    this.remove_attachment(&url, window, cx);
                })
            })
    }

    fn render_attachment_list(
        &self,
        _window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoIterator<Item = impl IntoElement> {
        let mut items = vec![];

        for url in self.attachments.read(cx).iter() {
            items.push(self.render_attachment(url, cx));
        }

        items
    }

    fn render_reply(&self, id: &EventId, cx: &Context<Self>) -> impl IntoElement {
        if let Some(text) = self.message(id) {
            let persons = PersonRegistry::global(cx);
            let profile = persons.read(cx).get(&text.author, cx);

            div()
                .w_full()
                .pl_2()
                .border_l_2()
                .border_color(cx.theme().element_active)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .items_baseline()
                                .gap_1()
                                .text_xs()
                                .text_color(cx.theme().text_muted)
                                .child("Replying to:")
                                .child(
                                    div()
                                        .text_color(cx.theme().text_accent)
                                        .child(profile.name()),
                                ),
                        )
                        .child(
                            Button::new("remove-reply")
                                .icon(IconName::Close)
                                .xsmall()
                                .ghost()
                                .on_click({
                                    let id = text.id;
                                    cx.listener(move |this, _, _, cx| {
                                        this.remove_reply(&id, cx);
                                    })
                                }),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .text_sm()
                        .text_ellipsis()
                        .line_clamp(1)
                        .child(SharedString::from(&text.content)),
                )
        } else {
            div()
        }
    }

    fn render_reply_list(
        &self,
        _window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoIterator<Item = impl IntoElement> {
        let mut items = vec![];

        for id in self.replies_to.read(cx).iter() {
            items.push(self.render_reply(id, cx));
        }

        items
    }

    fn render_config_menu(&self, _window: &mut Window, cx: &Context<Self>) -> impl IntoElement {
        let (backup, signer_kind) = self
            .room
            .read_with(cx, |this, _cx| {
                (this.config().backup(), this.config().signer_kind().clone())
            })
            .ok()
            .unwrap_or((true, SignerKind::default()));

        Button::new("encryption")
            .icon(IconName::Settings2)
            .tooltip("Configuration")
            .ghost()
            .large()
            .dropdown_menu(move |this, _window, _cx| {
                let auto = matches!(signer_kind, SignerKind::Auto);
                let encryption = matches!(signer_kind, SignerKind::Encryption);
                let user = matches!(signer_kind, SignerKind::User);

                this.label("Signer")
                    .menu_with_check_and_disabled(
                        "Auto",
                        auto,
                        Box::new(Command::ChangeSigner(SignerKind::Auto)),
                        auto,
                    )
                    .menu_with_check_and_disabled(
                        "Decoupled Encryption Key",
                        encryption,
                        Box::new(Command::ChangeSigner(SignerKind::Encryption)),
                        encryption,
                    )
                    .menu_with_check_and_disabled(
                        "User Identity",
                        user,
                        Box::new(Command::ChangeSigner(SignerKind::User)),
                        user,
                    )
                    .separator()
                    .label("Backup")
                    .menu_with_check("Backup messages", backup, Box::new(Command::ToggleBackup))
            })
    }

    fn render_emoji_menu(&self, _window: &Window, _cx: &Context<Self>) -> impl IntoElement {
        Button::new("emoji")
            .icon(IconName::Emoji)
            .ghost()
            .large()
            .dropdown_menu_with_anchor(gpui::Anchor::BottomLeft, move |this, _window, _cx| {
                let menu = this.horizontal();
                REACTION_EMOJIS.iter().fold(menu, |this, emoji| {
                    this.menu(*emoji, Box::new(Command::Insert(emoji)))
                })
            })
    }
}

impl Panel for ChatPanel {
    fn panel_id(&self) -> SharedString {
        self.id.clone()
    }

    fn title(&self, cx: &App) -> AnyElement {
        self.room
            .read_with(cx, |this, cx| {
                let label = this.display_name(cx);
                let url = this.display_image(cx);

                h_flex()
                    .gap_1p5()
                    .child(Avatar::new(url).xsmall())
                    .child(label)
                    .into_any_element()
            })
            .unwrap_or(div().child("Unknown").into_any_element())
    }

    fn toolbar_buttons(&self, _window: &Window, _cx: &App) -> Vec<Button> {
        let subject_bar = self.subject_bar.clone();

        vec![
            Button::new("subject")
                .icon(IconName::Input)
                .tooltip("Change subject")
                .small()
                .ghost()
                .on_click(move |_ev, _window, cx| {
                    subject_bar.update(cx, |this, cx| {
                        *this = !*this;
                        cx.notify();
                    });
                }),
        ]
    }
}

impl EventEmitter<PanelEvent> for ChatPanel {}

impl Focusable for ChatPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ChatPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .image_cache(goop_cache(self.id.clone(), 100))
            .on_action(cx.listener(Self::on_command))
            .size_full()
            .child(self.render_history_controls(cx))
            .when(*self.subject_bar.read(cx), |this| {
                this.child(
                    h_flex()
                        .h_12()
                        .w_full()
                        .px_2()
                        .gap_2()
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .child(Input::new(&self.subject_input).text_sm().small())
                        .child(
                            Button::new("change")
                                .icon(IconName::CheckCircle)
                                .label("Change")
                                .secondary()
                                .disabled(self.uploading)
                                .on_click(cx.listener(move |this, _ev, window, cx| {
                                    this.change_subject(window, cx);
                                })),
                        ),
                )
            })
            .child(
                v_flex()
                    .flex_1()
                    .relative()
                    .map(|this| {
                        if self.messages.is_empty() {
                            this.child(
                                div()
                                    .size_full()
                                    .flex()
                                    .items_center()
                                    .justify_end()
                                    .child(self.render_announcement(cx)),
                            )
                        } else {
                            this.child(
                                list(
                                    self.list_state.clone(),
                                    cx.processor(move |this, ix, window, cx| {
                                        this.render_message(ix, window, cx)
                                    }),
                                )
                                .size_full(),
                            )
                        }
                    })
                    .child(Scrollbar::vertical(&self.list_state)),
            )
            .child(
                v_flex()
                    .flex_shrink_0()
                    .p_2()
                    .w_full()
                    .gap_1p5()
                    .children(self.render_attachment_list(window, cx))
                    .children(self.render_reply_list(window, cx))
                    .child(
                        h_flex()
                            .items_end()
                            .child(
                                Button::new("upload")
                                    .icon(IconName::Plus)
                                    .tooltip("Upload media")
                                    .loading(self.uploading)
                                    .disabled(self.uploading)
                                    .ghost()
                                    .large()
                                    .on_click(cx.listener(move |this, _ev, window, cx| {
                                        this.upload(window, cx);
                                    })),
                            )
                            .child(Input::new(&self.input).appearance(false).flex_1())
                            .child(
                                h_flex()
                                    .pl_1()
                                    .gap_1()
                                    .child(self.render_emoji_menu(window, cx))
                                    .child(self.render_config_menu(window, cx))
                                    .child(
                                        Button::new("send")
                                            .icon(IconName::PaperPlaneFill)
                                            .disabled(self.uploading)
                                            .ghost()
                                            .large()
                                            .on_click(cx.listener(move |this, _ev, window, cx| {
                                                this.send_text_message(window, cx);
                                            })),
                                    ),
                            ),
                    ),
            )
    }
}
