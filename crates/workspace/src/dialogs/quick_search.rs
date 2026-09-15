//! Local quick navigation. Build searchable text once when opening; typing does no I/O.
use std::collections::HashMap;
use std::sync::Arc;

use chat::{ChatRegistry, Room, RoomKind, SearchMessage};
use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    ScrollStrategy, SharedString, StatefulInteractiveElement, Styled, Subscription,
    UniformListScrollHandle, Window, div, px, uniform_list,
};
use nostr_sdk::prelude::*;
use person::{Person, PersonRegistry};
use theme::ActiveTheme;
use ui::input::{Input, InputEvent, InputState};
use ui::modal::ModalButtonProps;
use ui::{WindowExtension, h_flex, v_flex};

#[derive(Clone)]
enum Target {
    Conversation(Entity<Room>),
    Profile(PublicKey),
    NoteToSelf,
}

struct Entry {
    name: SharedString,
    detail: SharedString,
    text: String,
    messages: Vec<Arc<SearchMessage>>,
    target: Target,
}

fn profile_text(person: &Person) -> String {
    let metadata = person.metadata();
    format!(
        "{} {} {} {} {}",
        person.name(),
        metadata.name.unwrap_or_default(),
        metadata.nip05.unwrap_or_default(),
        person.public_key().to_hex(),
        person.public_key().to_bech32().unwrap_or_default()
    )
    .to_lowercase()
}

fn matches(text: &str, query: &str) -> bool {
    query.split_whitespace().all(|word| text.contains(word))
}


#[derive(Debug, PartialEq, Eq)]
struct SearchResult {
    entry: usize,
    message: Option<usize>,
}

fn search(entries: &[Entry], query: &str) -> Vec<SearchResult> {
    let query = query.to_lowercase();
    let terms: Vec<_> = query.split_whitespace().collect();
    let mut names = Vec::new();
    let mut contents = Vec::new();
    for (entry, item) in entries.iter().enumerate() {
        if matches(&item.text, &query) {
            names.push(SearchResult { entry, message: None });
        } else if let Some(message) = item.messages.iter().position(|message| {
            terms.iter().all(|term| item.text.contains(term) || message.normalized.contains(term))
        }) {
            contents.push(SearchResult { entry, message: Some(message) });
        }
    }
    names.extend(contents);
    names
}

fn snippet(content: &str, query: &str) -> String {
    let query = query.to_lowercase();
    let terms: Vec<_> = query.split_whitespace().collect();
    let words: Vec<_> = content.split_whitespace().collect();
    let hit = words.iter().position(|word| {
        let word = word.to_lowercase();
        terms.iter().any(|term| word.contains(term))
    }).unwrap_or(0);
    let start = hit.saturating_sub(5);
    let end = (start + 24).min(words.len());
    let text = words[start..end].join(" ");
    let truncated = text.chars().count() > 180;
    format!("{}{}{}", if start > 0 { "…" } else { "" },
        text.chars().take(180).collect::<String>(),
        if truncated || end < words.len() { "…" } else { "" })
}

pub fn open(profiles: bool, window: &mut Window, cx: &mut App) {
    let owner = state::NostrRegistry::global(cx).read(cx).current_user();
    let mut people: HashMap<_, _> = PersonRegistry::global(cx)
        .read(cx)
        .loaded(cx)
        .into_iter()
        .map(|person| (person.public_key(), person))
        .collect();
    if let Some(owner) = owner {
        people.entry(owner).or_insert_with(|| Person::from(owner));
    }
    let mut entries = Vec::new();
    if profiles {
        for person in people.values() {
            entries.push(Entry {
                name: person.name(),
                detail: if Some(person.public_key()) == owner { "Yourself".into() }
                    else { person.public_key().to_bech32().unwrap_or_default().into() },
                text: format!("{} {}", profile_text(person),
                    if Some(person.public_key()) == owner { "note to self yourself" } else { "" }),
                messages: Vec::new(),
                target: Target::Profile(person.public_key()),
            });
        }
        entries.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then(a.text.cmp(&b.text))
        });
    } else {
        let chat = ChatRegistry::global(cx);
        let mut rooms = chat.read(cx).rooms(&RoomKind::Ongoing, cx);
        rooms.extend(chat.read(cx).rooms(&RoomKind::Request, cx));
        rooms.sort_by_key(|room| std::cmp::Reverse(room.read(cx).created_at));
        if let Some(owner) = owner {
            let self_room = chat.read(cx).rooms(&RoomKind::Ongoing, cx).into_iter()
                .chain(chat.read(cx).rooms(&RoomKind::Archived, cx))
                .find(|room| room.read(cx).members() == [owner]);
            entries.push(Entry {
                name: "Note to self".into(), detail: "Yourself".into(),
                text: format!("note to self yourself {}", profile_text(&people[&owner])),
                messages: self_room.as_ref().map(|room| chat.read(cx).search_messages(room.read(cx).id)).unwrap_or_default(),
                target: Target::NoteToSelf,
            });
        }
        for room in rooms {
            let data = room.read(cx);
            if owner.is_some_and(|owner| data.members() == [owner]) { continue; }
            let members: Vec<_> = data
                .members()
                .iter()
                .map(|key| {
                    people
                        .get(key)
                        .cloned()
                        .unwrap_or_else(|| Person::from(*key))
                })
                .collect();
            let name = data.subject.clone().unwrap_or_else(|| {
                members
                    .iter()
                    .take(if data.is_group() { 2 } else { 1 })
                    .map(|p| p.name().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
                    .into()
            });
            let text = format!(
                "{} {}",
                name,
                members
                    .iter()
                    .map(profile_text)
                    .collect::<Vec<_>>()
                    .join(" ")
            )
            .to_lowercase();
            entries.push(Entry {
                name,
                text,
                messages: chat.read(cx).search_messages(data.id),
                detail: if data.kind == RoomKind::Request {
                    "Message request"
                } else {
                    "Inbox"
                }
                .into(),
                target: Target::Conversation(room.clone()),
            });
        }
    }
    let view = cx.new(|cx| QuickSearch::new(entries, profiles, window, cx));
    window.open_modal(cx, move |modal, _, _| {
        modal
            .width(px(560.))
            .show_close(false)
            .child(view.clone())
    });
}

struct QuickSearch {
    entries: Vec<Entry>,
    results: Vec<SearchResult>,
    query: String,
    selected: usize,
    input: Entity<InputState>,
    scroll: UniformListScrollHandle,
    _subscription: Subscription,
}

impl QuickSearch {
    fn new(entries: Vec<Entry>, profiles: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx
            .new(|cx| InputState::new(window, cx).placeholder(if profiles { "Search profiles…" } else { "Search conversations and messages…" }));
        let subscription =
            cx.subscribe_in(
                &input,
                window,
                |this, input, event, window, cx| match event {
                    InputEvent::Change => {
                        let query = input.read(cx).value().to_lowercase();
                        this.results = search(&this.entries, &query);
                        this.query = query;
                        this.selected = 0;
                        this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => this.confirm(window, cx),
                    _ => {}
                },
            );
        cx.defer_in(window, |this, window, cx| {
            this.input.update(cx, |input, cx| input.focus(window, cx))
        });
        Self {
            results: search(&entries, ""),
            query: String::new(),
            entries,
            selected: 0,
            input,
            scroll: UniformListScrollHandle::new(),
            _subscription: subscription,
        }
    }

    fn move_selection(&mut self, down: bool, cx: &mut Context<Self>) {
        if self.results.is_empty() {
            return;
        }
        self.selected = if down {
            (self.selected + 1).min(self.results.len() - 1)
        } else {
            self.selected.saturating_sub(1)
        };
        self.scroll
            .scroll_to_item(self.selected, ScrollStrategy::Top);
        cx.notify();
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.results.get(self.selected) else {
            return;
        };
        let target = self.entries[ix.entry].target.clone();
        window.close_modal(cx);
        match target {
            Target::NoteToSelf => super::new_chat::open_self(window, cx),
            Target::Profile(key) => {
                window.dispatch_action(Box::new(crate::Command::OpenProfile(key)), cx);
            }
            Target::Conversation(room) => {
                let data = room.read(cx);
                let request =
                    data.kind == RoomKind::Request && settings::AppSettings::get_screening(cx);
                let id = data.id;
                let key = data.members().first().copied();
                ChatRegistry::global(cx).update(cx, |chat, cx| chat.emit_room(&room, window, cx));
                if request && let Some(key) = key {
                    let view = super::screening::init(key, window, cx);
                    window.open_modal(cx, move |modal, _, _| {
                        modal
                            .confirm()
                            .title("Message request")
                                .child("This person wants to start a conversation with you.")
                            .child(view.clone())
                            .button_props(
                                ModalButtonProps::default()
                                    .cancel_text("Ignore")
                                    .ok_text("Accept"),
                            )
                            .on_ok(move |_, _, cx| {
                                ChatRegistry::global(cx)
                                    .update(cx, |chat, cx| chat.accept_room(id, cx))
                            })
                            .on_cancel(|_, window, cx| {
                                window.dispatch_action(Box::new(ui::dock::ClosePanel), cx);
                                true
                            })
                    });
                }
            }
        }
    }
}

impl Render for QuickSearch {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .pt_3()
            .gap_2()
            .capture_action(cx.listener(|this, _: &ui::input::MoveUp, _, cx| {
                this.move_selection(false, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &ui::input::MoveDown, _, cx| {
                this.move_selection(true, cx);
                cx.stop_propagation();
            }))
            .child(div().pb_2().border_b_1().border_color(cx.theme().border)
                .child(Input::new(&self.input).appearance(false)))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().text_muted)
                    .child("↑ ↓ to choose · Enter to open · Esc to return"),
            )
            .when(self.results.is_empty(), |view| {
                view.child(div().p_4().child("No matching loaded items"))
            })
            .child(
                uniform_list(
                    "quick-search-results",
                    self.results.len(),
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        range
                            .map(|ix| {
                                let result = &this.results[ix];
                                let entry = &this.entries[result.entry];
                                let detail = result.message.map(|message| {
                                    SharedString::from(format!("{} · {}", entry.detail,
                                        snippet(&entry.messages[message].content, &this.query)))
                                }).unwrap_or_else(|| entry.detail.clone());
                                h_flex()
                                    .id(ix)
                                    .w_full()
                                    .p_2()
                                    .gap_2()
                                    .rounded_md()
                                    .when(ix == this.selected, |row| {
                                        row.bg(cx.theme().elevated_surface_background)
                                    })
                                    .child(
                                        v_flex()
                                            .min_w_0()
                                            .child(div().truncate().child(entry.name.clone()))
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_xs()
                                                    .text_color(cx.theme().text_muted)
                                                    .child(detail),
                                            ),
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.selected = ix;
                                        this.confirm(window, cx);
                                    }))
                            })
                            .collect()
                    }),
                )
                .track_scroll(&self.scroll)
                .h(px(320.)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_search_matches_names_addresses_keys_and_multiple_terms() {
        let key = Keys::generate().public_key();
        let person = Person::new(
            key,
            Metadata::new()
                .name("alice")
                .display_name("Alice Example")
                .nip05("alice@example.com"),
        );
        let text = profile_text(&person);
        assert!(matches(&text, "example alice"));
        assert!(matches(&text, "alice@example.com"));
        assert!(matches(&text, &key.to_bech32().unwrap()));
        assert!(matches(&text, &key.to_hex()));
        assert!(matches(&text, ""));
        assert!(!matches(&text, "alice missing"));
    }
    fn entry(name: &str, messages: &[&str]) -> Entry {
        Entry {
            name: name.to_owned().into(), detail: "Inbox".into(), text: name.to_lowercase(),
            messages: messages.iter().map(|content| Arc::new(SearchMessage {
                author: Keys::generate().public_key(),
                content: (*content).into(), normalized: content.to_lowercase().into(),
                created_at: Timestamp::from(0),
            })).collect(),
            target: Target::Profile(Keys::generate().public_key()),
        }
    }

    #[test]
    fn names_rank_before_contents_and_each_conversation_appears_once() {
        let entries = vec![entry("Bob", &["Alice sent the invoice", "Alice replied"]),
            entry("Alice", &["Hello"]), entry("Alice group", &[]), entry("Carol", &["Invoice due"])];
        assert_eq!(search(&entries, "ALICE"), vec![
            SearchResult { entry: 1, message: None }, SearchResult { entry: 2, message: None },
            SearchResult { entry: 0, message: Some(0) },
        ]);
        assert_eq!(search(&entries, "bob invoice"), vec![SearchResult { entry: 0, message: Some(0) }]);
        assert_eq!(search(&entries, "   ").len(), entries.len());
        assert!(search(&entries, "missing").is_empty());
    }

    #[test]
    fn content_terms_must_match_one_message_and_snippets_preserve_unicode() {
        assert!(search(&[entry("Alice", &["invoice", "tomorrow"])], "invoice tomorrow").is_empty());
        let text = "intro one two three four five six seven eight Café 東京 invoice details";
        let preview = snippet(text, "CAFÉ");
        assert!(preview.contains("Café 東京"));
        assert!(preview.starts_with('…'));
    }

}
