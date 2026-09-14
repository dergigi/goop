use std::collections::HashSet;
use std::ops::Range;

use crate::sidebar::entry::RoomEntry;
use anyhow::Error;
use chat::{ChatRegistry, Room, RoomKind};
use common::{DebouncedDelay, goop_cache};
use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, Subscription,
    Task, Window, uniform_list,
};
use instant::Duration;
use nostr_sdk::prelude::*;
use person::PersonRegistry;
use smallvec::{SmallVec, smallvec};
use state::{FIND_DELAY, IMAGE_CACHE_SIZE, NostrRegistry};
use theme::ActiveTheme;
use ui::button::{Button, ButtonVariants};
use ui::input::{Input, InputEvent, InputState};
use ui::notification::Notification;
use ui::{Disableable, IconName, Selectable, Sizable, StyledExt, WindowExtension, v_flex};

const INPUT_PLACEHOLDER: &str = "Find a contact or paste a Nostr address";

/// Contact selection and discovery for new direct or group conversations.
struct NewChat {
    /// Find input state
    find_input: Entity<InputState>,

    /// Debounced delay for find input
    find_debouncer: DebouncedDelay<Self>,

    /// Find results
    find_results: Entity<Option<Vec<PublicKey>>>,

    /// Async find operation
    find_task: Option<Task<Result<(), Error>>>,

    /// Selected public keys
    selected_pkeys: Entity<HashSet<PublicKey>>,

    contacts_expanded: bool,
    results_expanded: bool,

    /// User's contacts
    contact_list: Entity<Option<Vec<PublicKey>>>,

    /// Async tasks
    tasks: SmallVec<[Task<Result<(), Error>>; 1]>,

    /// Event subscriptions
    _subscriptions: SmallVec<[Subscription; 1]>,
}

impl NewChat {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let contact_list = cx.new(|_| None);
        let selected_pkeys = cx.new(|_| HashSet::new());
        let find_results = cx.new(|_| None);
        let find_input = cx.new(|cx| InputState::new(window, cx).placeholder(INPUT_PLACEHOLDER));

        let mut subscriptions = smallvec![];
        subscriptions.push(cx.observe(&NostrRegistry::global(cx), |_, _, cx| cx.notify()));

        subscriptions.push(
            // Subscribe to find input events
            cx.subscribe_in(&find_input, window, |this, state, event, window, cx| {
                let delay = Duration::from_millis(FIND_DELAY);

                match event {
                    InputEvent::PressEnter { .. } => {
                        this.search(window, cx);
                    }
                    InputEvent::Change => {
                        if state.read(cx).value().is_empty() {
                            // Clear results when input is empty
                            this.reset(window, cx);
                        } else {
                            // Run debounced search
                            this.find_debouncer
                                .fire_new(delay, window, cx, |this, window, cx| {
                                    this.debounced_search(window, cx)
                                });
                        }
                        cx.notify();
                    }
                    InputEvent::Focus => {
                        this.get_contact_list(window, cx);
                    }
                    _ => {}
                };
            }),
        );

        Self {
            find_input,
            find_debouncer: DebouncedDelay::new(),
            find_results,
            find_task: None,
            contacts_expanded: true,
            results_expanded: true,
            contact_list,
            selected_pkeys,
            tasks: smallvec![],
            _subscriptions: subscriptions,
        }
    }

    /// Get the contact list.
    fn get_contact_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let nostr = NostrRegistry::global(cx);
        let client = nostr.read(cx).client();

        let Some(public_key) = nostr.read(cx).current_user() else {
            return;
        };

        let task: Task<Result<HashSet<PublicKey>, Error>> = cx.background_spawn(async move {
            let filter = Filter::new()
                .author(public_key)
                .kind(Kind::ContactList)
                .limit(1);

            let contacts: HashSet<PublicKey> = client
                .database()
                .query(filter)
                .await?
                .into_iter()
                .next()
                .map(|event| event.tags.public_keys().collect())
                .unwrap_or_default();

            Ok(contacts)
        });

        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            match task.await {
                Ok(contacts) => {
                    this.update(cx, |this, cx| {
                        this.set_contact_list(contacts, cx);
                    })?;
                }
                Err(e) => {
                    cx.update(|window, cx| {
                        window.push_notification(
                            Notification::error(e.to_string()).autohide(false),
                            cx,
                        );
                    })?;
                }
            };

            Ok(())
        }));
    }

    /// Set the contact list with new contacts.
    fn set_contact_list<I>(&mut self, contacts: I, cx: &mut Context<Self>)
    where
        I: IntoIterator<Item = PublicKey>,
    {
        self.contact_list.update(cx, |this, cx| {
            *this = Some(contacts.into_iter().collect());
            cx.notify();
        });
    }

    /// Trigger the debounced search
    fn debounced_search(&self, window: &mut Window, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn_in(window, async move |this, cx| {
            this.update_in(cx, |this, window, cx| {
                this.search(window, cx);
            })
            .ok();
        })
    }

    /// Search
    fn search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Get query
        let query = self.find_input.read(cx).value();

        // Return if the query is empty
        if query.is_empty() {
            return;
        }

        // Block the input until the search completes
        self.set_finding(true, window, cx);

        // Create the search task
        let nostr = NostrRegistry::global(cx);
        let find_users = nostr.read(cx).search(&query, cx);

        // Run task in the main thread
        self.find_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = find_users.await;

            // Update the UI with the search results
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(people) => this.set_results(people, cx),
                    Err(error) => {
                        window.push_notification(Notification::error(error.to_string()), cx)
                    }
                };
                this.set_finding(false, window, cx);
            })?;

            Ok(())
        }));
    }

    /// Set the results of the search
    fn set_results(&mut self, results: Vec<PublicKey>, cx: &mut Context<Self>) {
        self.find_results.update(cx, |this, cx| {
            *this = Some(results);
            cx.notify();
        });
    }

    /// Set the finding status
    fn set_finding(&mut self, status: bool, _window: &mut Window, cx: &mut Context<Self>) {
        // Disable the input to prevent duplicate requests
        self.find_input.update(cx, |this, cx| {
            this.set_loading(status, cx);
        });
        // Set the search status
        cx.notify();
    }

    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Cancel delayed searches as well as any request already in flight.
        self.find_debouncer = DebouncedDelay::new();
        // Clear all search results
        self.find_results.update(cx, |this, cx| {
            *this = None;
            cx.notify();
        });

        // Clear all selected public keys
        self.selected_pkeys.update(cx, |this, cx| {
            this.clear();
            cx.notify();
        });

        // Reset the search status
        self.set_finding(false, window, cx);

        // Cancel the current search task
        self.find_task = None;
        cx.notify();
    }

    /// Select a public key in the picker.
    fn select(&mut self, public_key: &PublicKey, cx: &mut Context<Self>) {
        self.selected_pkeys.update(cx, |this, cx| {
            if this.contains(public_key) {
                this.remove(public_key);
            } else {
                this.insert(public_key.to_owned());
            }
            cx.notify();
        });
    }

    /// Check if a public key is selected in the picker.
    fn is_selected(&self, public_key: &PublicKey, cx: &App) -> bool {
        self.selected_pkeys.read(cx).contains(public_key)
    }

    /// Get all selected public keys in the picker.
    fn get_selected(&self, cx: &Context<Self>) -> HashSet<PublicKey> {
        self.selected_pkeys.read(cx).clone()
    }

    fn create_room(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(public_key) = NostrRegistry::global(cx).read(cx).current_user() else {
            return;
        };
        let receivers = self.get_selected(cx);
        if receivers.is_empty() {
            return;
        }
        let room = cx.new(|_| {
            Room::new(public_key, receivers)
                .organize(&public_key)
                .kind(RoomKind::Ongoing)
        });
        window.close_modal(cx);
        ChatRegistry::global(cx).update(cx, |chat, cx| chat.emit_room(&room, window, cx));
    }

    /// Render the contact list
    fn render_results(
        &self,
        range: Range<usize>,
        cx: &Context<Self>,
    ) -> Vec<impl IntoElement + use<>> {
        let persons = PersonRegistry::global(cx);

        // Get the contact list
        let Some(results) = self.find_results.read(cx) else {
            return vec![];
        };

        // Map the contact list to a list of elements
        results
            .get(range.clone())
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(ix, public_key)| {
                let selected = self.is_selected(public_key, cx);
                let profile = persons.read(cx).get(public_key, cx);
                let pkey_clone = public_key.to_owned();
                let handler = cx.listener(move |this, _ev, _window, cx| {
                    this.select(&pkey_clone, cx);
                });

                RoomEntry::new(range.start + ix)
                    .public_key(*public_key)
                    .name(profile.name())
                    .avatar(profile.avatar())
                    .on_click(handler)
                    .selected(selected)
                    .into_any_element()
            })
            .collect()
    }

    /// Render the contact list
    fn render_contacts(
        &self,
        range: Range<usize>,
        cx: &Context<Self>,
    ) -> Vec<impl IntoElement + use<>> {
        let persons = PersonRegistry::global(cx);

        // Get the contact list
        let Some(contacts) = self.contact_list.read(cx) else {
            return vec![];
        };

        // Map the contact list to a list of elements
        contacts
            .get(range.clone())
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(ix, public_key)| {
                let selected = self.is_selected(public_key, cx);
                let profile = persons.read(cx).get(public_key, cx);
                let pkey_clone = public_key.to_owned();
                let handler = cx.listener(move |this, _ev, _window, cx| {
                    this.select(&pkey_clone, cx);
                });

                RoomEntry::new(range.start + ix)
                    .public_key(*public_key)
                    .name(profile.name().trim())
                    .avatar(profile.avatar())
                    .on_click(handler)
                    .selected(selected)
                    .into_any_element()
            })
            .collect()
    }
}

pub fn open(window: &mut Window, cx: &mut App) {
    let view = cx.new(|cx| NewChat::new(window, cx));
    let input = view.read(cx).find_input.clone();
    window.open_modal(cx, move |modal, _, _| {
        modal
            .width(gpui::px(560.))
            .show_close(true)
            .title("New Chat")
            .child(view.clone())
    });
    window.defer(cx, move |window, cx| {
        input.update(cx, |input, cx| input.focus(window, cx))
    });
}

impl Render for NewChat {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .h(gpui::px(440.))
            .gap_3()
            .image_cache(goop_cache("new-chat", IMAGE_CACHE_SIZE))
            .child(Input::new(&self.find_input))
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_3()
                    .when_some(self.find_results.read(cx).as_ref(), |this, results| {
                        this.child(
                            v_flex()
                                .gap_1()
                                .when(self.results_expanded, |this| this.flex_1())
                                .border_b_1()
                                .border_color(cx.theme().border_variant)
                                .child(
                                    Button::new("toggle-results")
                                        .label("Results")
                                        .icon(if self.results_expanded {
                                            IconName::CaretDown
                                        } else {
                                            IconName::CaretRight
                                        })
                                        .transparent()
                                        .small()
                                        .w_full()
                                        .justify_start()
                                        .font_semibold()
                                        .text_color(cx.theme().text_muted)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.results_expanded = !this.results_expanded;
                                            cx.notify();
                                        })),
                                )
                                .when(self.results_expanded, |this| {
                                    this.child(
                                        uniform_list(
                                            "rooms",
                                            results.len(),
                                            cx.processor(move |this, range, _window, cx| {
                                                this.render_results(range, cx)
                                            }),
                                        )
                                        .flex_1()
                                        .h_full(),
                                    )
                                }),
                        )
                    })
                    .when_some(self.contact_list.read(cx).as_ref(), |this, contacts| {
                        this.child(
                            v_flex()
                                .gap_1()
                                .when(self.contacts_expanded, |this| this.flex_1())
                                .child(
                                    Button::new("toggle-contacts")
                                        .label("Contacts")
                                        .icon(if self.contacts_expanded {
                                            IconName::CaretDown
                                        } else {
                                            IconName::CaretRight
                                        })
                                        .transparent()
                                        .small()
                                        .w_full()
                                        .justify_start()
                                        .font_semibold()
                                        .text_color(cx.theme().text_muted)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.contacts_expanded = !this.contacts_expanded;
                                            cx.notify();
                                        })),
                                )
                                .when(self.contacts_expanded, |this| {
                                    this.child(
                                        uniform_list(
                                            "contacts",
                                            contacts.len(),
                                            cx.processor(|this, range, _window, cx| {
                                                this.render_contacts(range, cx)
                                            }),
                                        )
                                        .flex_1()
                                        .h_full(),
                                    )
                                }),
                        )
                    }),
            )
            .child(
                Button::new("create-chat")
                    .label(if self.selected_pkeys.read(cx).len() > 1 {
                        "Create Group DM"
                    } else {
                        "Start Chat"
                    })
                    .primary()
                    .disabled(self.selected_pkeys.read(cx).is_empty())
                    .on_click(cx.listener(|this, _, window, cx| this.create_room(window, cx))),
            )
    }
}
