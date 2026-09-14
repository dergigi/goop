//! Local quick navigation. Build searchable text once when opening; typing does no I/O.
use std::collections::HashMap;

use chat::{ChatRegistry, Room, RoomKind};
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
}

struct Entry {
    name: SharedString,
    detail: SharedString,
    text: String,
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

pub fn open(profiles: bool, window: &mut Window, cx: &mut App) {
    let people: HashMap<_, _> = PersonRegistry::global(cx)
        .read(cx)
        .loaded(cx)
        .into_iter()
        .map(|person| (person.public_key(), person))
        .collect();
    let mut entries = Vec::new();
    if profiles {
        for person in people.values() {
            entries.push(Entry {
                name: person.name(),
                detail: person.public_key().to_bech32().unwrap_or_default().into(),
                text: profile_text(person),
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
        for room in rooms {
            let data = room.read(cx);
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
    let view = cx.new(|cx| QuickSearch::new(entries, window, cx));
    window.open_modal(cx, move |modal, _, _| {
        modal
            .width(px(560.))
            .show_close(true)
            .title(if profiles {
                "Search profiles"
            } else {
                "Search conversations"
            })
            .child(view.clone())
    });
}

struct QuickSearch {
    entries: Vec<Entry>,
    results: Vec<usize>,
    selected: usize,
    input: Entity<InputState>,
    scroll: UniformListScrollHandle,
    _subscription: Subscription,
}

impl QuickSearch {
    fn new(entries: Vec<Entry>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx
            .new(|cx| InputState::new(window, cx).placeholder("Search by name, address, or npub…"));
        let subscription =
            cx.subscribe_in(
                &input,
                window,
                |this, input, event, window, cx| match event {
                    InputEvent::Change => {
                        let query = input.read(cx).value().to_lowercase();
                        this.results = this
                            .entries
                            .iter()
                            .enumerate()
                            .filter_map(|(ix, entry)| matches(&entry.text, &query).then_some(ix))
                            .collect();
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
            results: (0..entries.len()).collect(),
            entries,
            selected: 0,
            input,
            scroll: UniformListScrollHandle::new(),
            _subscription: subscription,
        }
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.results.get(self.selected) else {
            return;
        };
        let target = self.entries[*ix].target.clone();
        window.close_modal(cx);
        match target {
            Target::Profile(key) => {
                let view = super::screening::init(key, window, cx);
                window.open_modal(cx, move |modal, _, _| {
                    modal.title("Profile").show_close(true).child(view.clone())
                });
            }
            Target::Conversation(room) => {
                let data = room.read(cx);
                let request = data.kind == RoomKind::Request && settings::AppSettings::get_screening(cx);
                let id = data.id;
                let key = data.members().first().copied();
                ChatRegistry::global(cx).update(cx, |chat, cx| chat.emit_room(&room, window, cx));
                if request && let Some(key) = key {
                    let view = super::screening::init(key, window, cx);
                    window.open_modal(cx, move |modal, _, _| {
                        modal
                            .confirm()
                            .title("Message request")
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
            .gap_2()
            .capture_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "up" | "down" => {
                        if !this.results.is_empty() {
                            this.selected = if event.keystroke.key == "up" {
                                this.selected.saturating_sub(1)
                            } else {
                                (this.selected + 1).min(this.results.len() - 1)
                            };
                            this.scroll
                                .scroll_to_item(this.selected, ScrollStrategy::Top);
                            cx.notify();
                        }
                        cx.stop_propagation();
                        window.prevent_default();
                    }
                    _ => {}
                }
            }))
            .child(Input::new(&self.input))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().text_muted)
                    .child("Loaded items only · ↑ ↓ to choose · Enter to open · Esc to return"),
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
                                let entry = &this.entries[this.results[ix]];
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
                                                    .child(entry.detail.clone()),
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
}
