use std::collections::{HashMap, HashSet};
use std::ops::Range;

use anyhow::Error;
use chat::{ChatRegistry, Room, RoomKind};
use common::{DebouncedDelay, goop_cache};
use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    Styled, Subscription, Task, UniformListScrollHandle, Window, div, uniform_list,
};
use instant::Duration;
use nostr_sdk::prelude::*;
use person::{Person, PersonRegistry};
use state::{FIND_DELAY, IMAGE_CACHE_SIZE, NostrRegistry};
use theme::ActiveTheme;
use ui::button::{Button, ButtonVariants};
use ui::input::{Input, InputEvent, InputState};
use ui::{Disableable, Selectable, WindowExtension, v_flex};

use crate::sidebar::entry::RoomEntry;

#[derive(Debug, PartialEq, Eq)]
enum Query {
    Text,
    PublicKey(PublicKey),
    Address(Nip05Address),
    InvalidIdentifier,
}

/// Only explicit identifiers may trigger resolution. The SDK's NIP-05 parser
/// also accepts plain names as domains, so it must not classify arbitrary text.
fn classify_query(query: &str) -> Query {
    let query = query.trim();
    if let Ok(key) = PublicKey::parse(query) {
        return Query::PublicKey(key);
    }
    if query.starts_with("npub1") || query.starts_with("nprofile1") || query.starts_with("nostr:") {
        return Query::InvalidIdentifier;
    }
    if query.contains('@') && !query.chars().any(char::is_whitespace) {
        let mut parts = query.split('@');
        let name = parts.next().unwrap_or_default();
        let domain = parts.next().unwrap_or_default();
        if !name.is_empty()
            && !domain.is_empty()
            && parts.next().is_none()
            && !domain.contains(['/', '?', '#', ':'])
            && let Ok(address) = Nip05Address::parse(query)
        {
            return Query::Address(address);
        }
        return Query::InvalidIdentifier;
    }
    Query::Text
}

#[cfg(test)]
fn matches_contact(person: &Person, query: &str) -> bool {
    matches_contact_with_self(person, query, false)
}

fn matches_contact_with_self(person: &Person, query: &str, is_self: bool) -> bool {
    let metadata = person.metadata();
    let text = format!(
        "{} {} {} {} {} {}",
        if is_self { "note to self yourself" } else { "" },
        person.name(),
        metadata.name.unwrap_or_default(),
        metadata.nip05.unwrap_or_default(),
        person.public_key().to_hex(),
        person.public_key().to_bech32().unwrap_or_default()
    )
    .to_lowercase();
    query
        .to_lowercase()
        .split_whitespace()
        .all(|word| text.contains(word))
}

/// Keep navigation on the same person when results reorder; clamp like Cmd+K.
fn contact_selection(
    visible: &[PublicKey],
    current: Option<PublicKey>,
    down: Option<bool>,
) -> Option<PublicKey> {
    if visible.is_empty() {
        return None;
    }
    let index = current
        .and_then(|key| visible.iter().position(|item| *item == key))
        .unwrap_or(0);
    let index = match down {
        Some(true) => (index + 1).min(visible.len() - 1),
        Some(false) => index.saturating_sub(1),
        None => index,
    };
    Some(visible[index])
}

/// Local contact filtering, with direct resolution for explicit identifiers.
struct NewChat {
    input: Entity<InputState>,
    contacts: Vec<PublicKey>,
    visible: Vec<PublicKey>,
    highlighted: Option<PublicKey>,
    selected: HashSet<PublicKey>,
    group_mode: bool,
    resolved: Option<PublicKey>,
    error: Option<String>,
    contacts_loaded: bool,
    scroll: UniformListScrollHandle,
    debounce: DebouncedDelay<Self>,
    resolution: Option<Task<()>>,
    _contacts_task: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl NewChat {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Filter contacts or paste npub, nprofile, name@domain")
        });
        let subscriptions = vec![
            cx.observe(&PersonRegistry::global(cx), |this, _, cx| this.filter(cx)),
            cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
                InputEvent::Change => this.query_changed(window, cx),
                InputEvent::PressEnter { .. } => {
                    this.debounce = DebouncedDelay::new();
                    if let Some(key) = this.highlighted {
                        this.activate_contact(key, window, cx);
                    } else {
                        this.resolve(window, cx);
                    }
                }
                _ => {}
            }),
        ];
        let nostr = NostrRegistry::global(cx);
        let user = nostr.read(cx).current_user();
        let client = nostr.read(cx).client();
        let load = cx.background_spawn(async move {
            let Some(user) = user else {
                return Ok::<_, Error>(Vec::new());
            };
            let events = client
                .database()
                .query(Filter::new().author(user).kind(Kind::ContactList).limit(1))
                .await?;
            Ok(events
                .into_iter()
                .next()
                .map(|event| {
                    event
                        .tags
                        .public_keys()
                        .collect::<HashSet<_>>()
                        .into_iter()
                        .collect()
                })
                .unwrap_or_default())
        });
        let contacts_task = cx.spawn(async move |this, cx| {
            let result = load.await;
            _ = this.update(cx, |this, cx| {
                this.contacts_loaded = true;
                match result {
                    Ok(contacts) => this.contacts = contacts,
                    Err(_) => {
                        this.error =
                            Some("Could not load contacts. Close New Chat and try again.".into())
                    }
                }
                this.filter(cx);
            });
        });
        Self {
            input,
            contacts: Vec::new(),
            visible: Vec::new(),
            highlighted: None,
            selected: HashSet::new(),
            group_mode: false,
            resolved: None,
            error: None,
            contacts_loaded: false,
            scroll: UniformListScrollHandle::new(),
            debounce: DebouncedDelay::new(),
            resolution: None,
            _contacts_task: contacts_task,
            _subscriptions: subscriptions,
        }
    }

    fn filter(&mut self, cx: &mut Context<Self>) {
        // Snapshot lookup does no database or network work. Rendering visible
        // people below may refresh their metadata through the normal cache.
        let people: HashMap<_, _> = PersonRegistry::global(cx)
            .read(cx)
            .loaded(cx)
            .into_iter()
            .map(|person| (person.public_key(), person))
            .collect();
        let query = self.input.read(cx).value();
        let person = |key: &PublicKey| {
            people
                .get(key)
                .cloned()
                .unwrap_or_else(|| Person::from(*key))
        };
        let owner = NostrRegistry::global(cx).read(cx).current_user();
        let mut candidates = self.contacts.clone();
        if !self.group_mode && let Some(owner) = owner {
            candidates.retain(|key| *key != owner);
            candidates.insert(0, owner);
        }
        self.visible = candidates.into_iter()
            .filter(|key| matches_contact_with_self(&person(key), &query, Some(*key) == owner))
            .collect();
        self.visible
            .sort_by_cached_key(|key| (person(key).name().to_lowercase(), key.to_hex()));
        if let Some(key) = self.resolved {
            self.visible.retain(|other| *other != key);
            self.visible.insert(0, key);
        }
        self.highlighted = contact_selection(&self.visible, self.highlighted, None);
        cx.notify();
    }

    fn query_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.debounce = DebouncedDelay::new();
        self.resolution = None;
        self.resolved = None;
        self.highlighted = None;
        self.error = None;
        self.input
            .update(cx, |input, cx| input.set_loading(false, cx));
        self.filter(cx);
        self.scroll.scroll_to_item(0, gpui::ScrollStrategy::Top);
        match classify_query(&self.input.read(cx).value()) {
            Query::PublicKey(key) => {
                self.resolved = Some(key);
                self.filter(cx);
            }
            Query::Address(_) => {
                self.debounce.fire_new(
                    Duration::from_millis(FIND_DELAY),
                    window,
                    cx,
                    |_, window, cx| {
                        cx.spawn_in(window, async move |this, cx| {
                            _ = this.update_in(cx, |this, window, cx| this.resolve(window, cx));
                        })
                    },
                );
            }
            Query::Text | Query::InvalidIdentifier => {}
        }
    }

    fn resolve(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value();
        match classify_query(&query) {
            Query::Text => {} // Enter must never turn local filtering into a relay search.
            Query::InvalidIdentifier => {
                self.error = Some("Check the Nostr identifier or name@domain address.".into());
                cx.notify();
            }
            Query::PublicKey(key) => {
                self.resolved = Some(key);
                self.filter(cx);
            }
            Query::Address(address) => {
                self.resolution = None;
                self.error = None;
                self.input
                    .update(cx, |input, cx| input.set_loading(true, cx));
                let task = NostrRegistry::global(cx)
                    .read(cx)
                    .query_address(address, cx);
                self.resolution = Some(cx.spawn(async move |this, cx| {
                    let result = task.await;
                    _ = this.update(cx, |this, cx| {
                        if this.input.read(cx).value() != query { return; }
                        this.input.update(cx, |input, cx| input.set_loading(false, cx));
                        match result {
                            Ok(key) => this.resolved = Some(key),
                            Err(_) => this.error = Some("Could not resolve this address. Check it and press Enter to retry.".into()),
                        }
                        this.filter(cx);
                    });
                }));
            }
        }
    }

    fn move_selection(&mut self, down: bool, cx: &mut Context<Self>) {
        self.highlighted = contact_selection(&self.visible, self.highlighted, Some(down));
        if let Some(index) = self
            .highlighted
            .and_then(|key| self.visible.iter().position(|item| *item == key))
        {
            self.scroll.scroll_to_item(index, gpui::ScrollStrategy::Top);
        }
        cx.notify();
    }

    fn activate_contact(&mut self, key: PublicKey, window: &mut Window, cx: &mut Context<Self>) {
        self.highlighted = Some(key);
        if self.group_mode {
            if !self.selected.insert(key) {
                self.selected.remove(&key);
            }
            cx.notify();
        } else {
            self.selected.clear();
            self.selected.insert(key);
            self.create_room(window, cx);
        }
    }

    fn create_room(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(user) = NostrRegistry::global(cx).read(cx).current_user() else {
            return;
        };
        if self.selected.is_empty() {
            return;
        }
        let room = cx.new(|_| {
            Room::new(user, self.selected.clone())
                .organize(&user)
                .kind(RoomKind::Ongoing)
        });
        let chat = ChatRegistry::global(cx);
        let existing = chat
            .read(cx)
            .room(&room.read(cx).id, cx)
            .and_then(|room| room.upgrade());
        let room = existing.unwrap_or(room);
        window.close_modal(cx);
        chat.update(cx, |chat, cx| chat.emit_room(&room, window, cx));
    }

    fn render_contacts(
        &self,
        range: Range<usize>,
        cx: &Context<Self>,
    ) -> Vec<impl IntoElement + use<>> {
        let people = PersonRegistry::global(cx);
        self.visible
            .get(range.clone())
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(index, key)| {
                let key = *key;
                let person = people.read(cx).get(&key, cx);
                RoomEntry::new(range.start + index)
                    .public_key(key)
                    .name(if Some(key) == NostrRegistry::global(cx).read(cx).current_user() { "Note to self".into() } else { person.name() })
                    .avatar(person.avatar())
                    .selected(self.selected.contains(&key))
                    .highlighted(self.highlighted == Some(key))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.activate_contact(key, window, cx);
                    }))
                    .into_any_element()
            })
            .collect()
    }
}

pub fn open_self(window: &mut Window, cx: &mut App) {
    let Some(owner) = NostrRegistry::global(cx).read(cx).current_user() else { return; };
    let candidate = cx.new(|_| Room::new(owner, [owner]).organize(&owner).kind(RoomKind::Ongoing));
    let chat = ChatRegistry::global(cx);
    let room = chat.read(cx).room(&candidate.read(cx).id, cx)
        .and_then(|room| room.upgrade()).unwrap_or(candidate);
    chat.update(cx, |chat, cx| chat.emit_room(&room, window, cx));
}

pub fn open(window: &mut Window, cx: &mut App) {
    open_with_mode(false, window, cx);
}

pub fn open_group(window: &mut Window, cx: &mut App) {
    open_with_mode(true, window, cx);
}

fn open_with_mode(group_mode: bool, window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| {
        let mut view = NewChat::new(window, cx);
        view.group_mode = group_mode;
        view
    });
    let input = view.read(cx).input.clone();
    window.open_modal(cx, move |modal, _, _| {
        modal
            .width(gpui::px(560.))
            .show_close(true)
            .title(if group_mode { "New Group" } else { "New Chat" })
            .child(view.clone())
    });
    window.defer(cx, move |window, cx| {
        input.update(cx, |input, cx| input.focus(window, cx))
    });
}

impl Render for NewChat {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .capture_action(
                cx.listener(|this, _: &ui::input::MoveUp, _, cx| this.move_selection(false, cx)),
            )
            .capture_action(
                cx.listener(|this, _: &ui::input::MoveDown, _, cx| this.move_selection(true, cx)),
            )
            .h(gpui::px(440.))
            .gap_3()
            .image_cache(goop_cache("new-chat", IMAGE_CACHE_SIZE))
            .child(Input::new(&self.input))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().text_muted)
                    .child(if self.group_mode {
                        "↑ ↓ to choose · Enter to select · Esc to close"
                    } else {
                        "↑ ↓ to choose · Enter to open · Esc to return"
                    }),
            )
            .when_some(self.error.clone(), |view, error| {
                view.child(div().text_sm().child(error))
            })
            .when(
                self.contacts_loaded
                    && self.visible.is_empty()
                    && self.error.is_none()
                    && !self.input.read(cx).loading,
                |view| {
                    view.child(div().text_sm().text_color(cx.theme().text_muted).child(
                        if self.input.read(cx).value().trim().is_empty() {
                            "No contacts yet. Paste a Nostr identifier or name@domain address."
                        } else {
                            "No matching contacts."
                        },
                    ))
                },
            )
            .child(
                uniform_list(
                    "new-chat-contacts",
                    self.visible.len(),
                    cx.processor(|this, range, _, cx| this.render_contacts(range, cx)),
                )
                .track_scroll(&self.scroll)
                .flex_1()
                .min_h_0(),
            )
            .when(!self.selected.is_empty(), |view| {
                view.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().text_muted)
                        .child(format!("{} selected", self.selected.len())),
                )
            })
            .when(self.group_mode, |view| {
                view.child(
                    Button::new("create-chat")
                        .label("Create Group DM")
                        .primary()
                        .disabled(self.selected.len() < 2)
                        .on_click(cx.listener(|this, _, window, cx| this.create_room(window, cx))),
                )
            })
            .child(
                Button::new("group-mode")
                    .label(if self.group_mode {
                        "Cancel group"
                    } else {
                        "New group chat"
                    })
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.group_mode = !this.group_mode;
                        this.selected.clear();
                        cx.notify();
                    })),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_aliases_are_only_added_for_the_current_account() {
        let person = Person::new(Keys::generate().public_key(), Metadata::new().name("alice"));
        for query in ["self", "Yourself", "note to self", "alice self"] {
            assert!(matches_contact_with_self(&person, query, true));
            assert!(!matches_contact_with_self(&person, query, false));
        }
        assert!(matches_contact_with_self(&person, "alice", true));
    }

    #[test]
    fn keyboard_selection_survives_reordering_and_clamps_to_results() {
        let alice = Keys::generate().public_key();
        let bob = Keys::generate().public_key();
        assert_eq!(contact_selection(&[alice, bob], None, None), Some(alice));
        assert_eq!(
            contact_selection(&[alice, bob], Some(alice), Some(true)),
            Some(bob)
        );
        assert_eq!(
            contact_selection(&[alice, bob], Some(bob), Some(true)),
            Some(bob)
        );
        assert_eq!(
            contact_selection(&[alice, bob], Some(alice), Some(false)),
            Some(alice)
        );
        assert_eq!(
            contact_selection(&[bob, alice], Some(alice), None),
            Some(alice)
        );
        assert_eq!(contact_selection(&[bob], Some(alice), None), Some(bob));
        assert_eq!(contact_selection(&[], Some(alice), Some(true)), None);
    }

    #[test]
    fn ordinary_names_never_trigger_resolution() {
        for query in [
            "",
            "Alice",
            "Alice Smith",
            "alice.example",
            "@alice",
            "alice@",
            "alice@example.org@evil.org",
        ] {
            assert!(
                !matches!(
                    classify_query(query),
                    Query::Address(_) | Query::PublicKey(_)
                ),
                "{query}"
            );
        }
        assert!(matches!(
            classify_query(" alice@example.org "),
            Query::Address(_)
        ));
    }

    #[test]
    fn resolves_public_identifiers_without_global_search() {
        let key = Keys::generate().public_key();
        let npub = key.to_bech32().unwrap();
        let profile = Nip19Profile::new(key, []).to_bech32().unwrap();
        for query in [
            npub.clone(),
            format!("nostr:{npub}"),
            profile.clone(),
            format!("nostr:{profile}"),
            key.to_hex(),
        ] {
            assert_eq!(classify_query(&query), Query::PublicKey(key));
        }
        assert_eq!(classify_query("npub1invalid"), Query::InvalidIdentifier);
        assert_eq!(classify_query("nprofile1invalid"), Query::InvalidIdentifier);
    }

    #[test]
    fn filters_contacts_by_visible_name_case_insensitively() {
        let person = Person::new(
            Keys::generate().public_key(),
            Metadata::new()
                .name("alice")
                .display_name("Alice Smith")
                .nip05("alice@example.org"),
        );
        assert!(matches_contact(&person, &person.name().to_uppercase()));
        assert!(matches_contact(&person, "  "));
        assert!(matches_contact(&person, "SMITH alice"));
        assert!(matches_contact(&person, "example.org"));
        assert!(!matches_contact(&person, "not a matching contact name"));
    }
}
