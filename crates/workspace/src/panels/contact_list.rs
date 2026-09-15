use std::collections::HashSet;

use anyhow::Error;
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString, Styled, Subscription,
    Task, TextAlign, Window, div, rems,
};
use instant::Duration;
use nostr_sdk::prelude::*;
use person::PersonRegistry;
use smallvec::{SmallVec, smallvec};
use state::NostrRegistry;
use theme::ActiveTheme;
use ui::avatar::Avatar;
use ui::button::{Button, ButtonVariants};
use ui::dock::{Panel, PanelEvent};
use ui::input::{Input, InputEvent, InputState};
use ui::scroll::ScrollableElement;
use ui::{Disableable, IconName, Sizable, StyledExt, WindowExtension, h_flex, v_flex};

pub fn init(window: &mut Window, cx: &mut App) -> Entity<ContactListPanel> {
    cx.new(|cx| ContactListPanel::new(window, cx))
}

#[derive(Debug)]
pub struct ContactListPanel {
    name: SharedString,
    focus_handle: FocusHandle,

    /// Npub input
    input: Entity<InputState>,

    /// Whether the panel is updating
    updating: bool,

    /// Error message
    error: Option<SharedString>,

    /// All contacts
    contacts: HashSet<PublicKey>,

    /// Event subscriptions
    _subscriptions: SmallVec<[Subscription; 1]>,

    /// Background tasks
    tasks: Vec<Task<Result<(), Error>>>,
}

impl ContactListPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("npub1..."));
        let mut subscriptions = smallvec![];

        subscriptions.push(
            // Subscribe to user's input events
            cx.subscribe_in(&input, window, move |this, _input, event, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.add(window, cx);
                }
            }),
        );

        // Run at the end of current cycle
        cx.defer_in(window, |this, window, cx| {
            this.load(window, cx);
        });

        Self {
            name: "Contact List".into(),
            focus_handle: cx.focus_handle(),
            input,
            updating: false,
            contacts: HashSet::new(),
            error: None,
            _subscriptions: subscriptions,
            tasks: vec![],
        }
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

            let contact_list: HashSet<PublicKey> = client
                .database()
                .query(filter)
                .await?
                .into_iter()
                .next()
                .map(|event| event.tags.public_keys().collect())
                .unwrap_or_default();

            Ok(contact_list)
        });

        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            let public_keys = task.await?;

            // Update state
            this.update(cx, |this, cx| {
                this.contacts.extend(public_keys);
                cx.notify();
            })?;

            Ok(())
        }));
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.input.read(cx).value().to_string();

        if let Ok(public_key) = PublicKey::parse(&value) {
            if self.contacts.insert(public_key) {
                self.input.update(cx, |this, cx| {
                    this.set_value("", window, cx);
                });
                cx.notify();
            }
        } else {
            self.set_error("The public key is invalid.", window, cx);
        }
    }

    fn remove(&mut self, public_key: &PublicKey, cx: &mut Context<Self>) {
        self.contacts.remove(public_key);
        cx.notify();
    }

    fn set_error<E>(&mut self, error: E, window: &mut Window, cx: &mut Context<Self>)
    where
        E: Into<SharedString>,
    {
        self.error = Some(error.into());
        cx.notify();

        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;

            // Clear the error message after a delay
            this.update(cx, |this, cx| {
                this.error = None;
                cx.notify();
            })?;

            Ok(())
        }));
    }

    fn set_updating(&mut self, updating: bool, cx: &mut Context<Self>) {
        self.updating = updating;
        cx.notify();
    }

    pub fn update(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.contacts.is_empty() {
            self.set_error("Add at least one contact.", window, cx);
            return;
        };

        let nostr = NostrRegistry::global(cx);
        let client = nostr.read(cx).client();
        let signer = nostr.read(cx).signer();

        // Get contacts
        let contacts: Vec<Contact> = self
            .contacts
            .iter()
            .map(|public_key| Contact::new(*public_key))
            .collect();

        // Set updating state
        self.set_updating(true, cx);

        let task: Task<Result<(), Error>> = cx.background_spawn(async move {
            // Construct contact list event builder
            let event = ContactListBuilder::new(contacts)
                .finalize_async(&signer)
                .await?;

            // Set contact list
            client.send_event(&event).to_nip65().await?;

            Ok(())
        });

        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            match task.await {
                Ok(_) => {
                    this.update_in(cx, |this, window, cx| {
                        this.set_updating(false, cx);
                        this.load(window, cx);

                        window.push_notification("Update successful", cx);
                    })?;
                }
                Err(e) => {
                    this.update_in(cx, |this, window, cx| {
                        this.set_updating(false, cx);
                        this.set_error(e.to_string(), window, cx);
                    })?;
                }
            };

            Ok(())
        }));
    }

    fn render_list_items(&mut self, cx: &mut Context<Self>) -> Vec<impl IntoElement> {
        let persons = PersonRegistry::global(cx);
        let mut items = Vec::new();

        for (ix, public_key) in self.contacts.iter().enumerate() {
            let profile = persons.read(cx).get(public_key, cx);

            items.push(
                h_flex()
                    .id(ix)
                    .group("")
                    .flex_1()
                    .w_full()
                    .h_8()
                    .px_2()
                    .justify_between()
                    .rounded(cx.theme().radius)
                    .hover(|this| this.bg(cx.theme().ghost_element_hover))
                    .child(
                        h_flex()
                            .gap_2()
                            .text_sm()
                            .child(Avatar::new(profile.avatar()).small())
                            .child(profile.name()),
                    )
                    .child(
                        Button::new("remove_{ix}")
                            .icon(IconName::Close)
                            .xsmall()
                            .ghost()
                            .invisible()
                            .group_hover("", |this| this.visible())
                            .on_click({
                                let public_key = public_key.to_owned();
                                cx.listener(move |this, _ev, _window, cx| {
                                    this.remove(&public_key, cx);
                                })
                            }),
                    ),
            )
        }

        items
    }

    fn render_empty(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .h_20()
            .justify_center()
            .border_2()
            .border_dashed()
            .border_color(cx.theme().border)
            .rounded(cx.theme().radius_lg)
            .text_sm()
            .text_align(TextAlign::Center)
            .child(SharedString::from("Add contacts to get started."))
    }
}

impl Panel for ContactListPanel {
    fn panel_id(&self) -> SharedString {
        self.name.clone()
    }

    fn title(&self, _cx: &App) -> AnyElement {
        self.name.clone().into_any_element()
    }
}

impl EventEmitter<PanelEvent> for ContactListPanel {}

impl Focusable for ContactListPanel {
    fn focus_handle(&self, _: &App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ContactListPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .p_3()
            .gap_3()
            .w_full()
            .overflow_y_scrollbar()
            .child(
                v_flex()
                    .gap_2()
                    .flex_1()
                    .w_full()
                    .text_sm()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().text_muted)
                            .child("New contact:"),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .w_full()
                                    .child(Input::new(&self.input).small().cleanable(true))
                                    .child(
                                        Button::new("add")
                                            .icon(IconName::Plus)
                                            .tooltip("Add contact")
                                            .ghost()
                                            .size(rems(2.))
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.add(window, cx);
                                            })),
                                    ),
                            )
                            .when_some(self.error.as_ref(), |this, error| {
                                this.child(
                                    div()
                                        .italic()
                                        .text_xs()
                                        .text_color(cx.theme().text_danger)
                                        .child(error.clone()),
                                )
                            }),
                    )
                    .map(|this| {
                        if self.contacts.is_empty() {
                            this.child(self.render_empty(window, cx))
                        } else {
                            this.child(
                                v_flex()
                                    .gap_1()
                                    .flex_1()
                                    .w_full()
                                    .children(self.render_list_items(cx)),
                            )
                        }
                    })
                    .child(
                        Button::new("submit")
                            .icon(IconName::CheckCircle)
                            .label("Update")
                            .primary()
                            .font_semibold()
                            .loading(self.updating)
                            .disabled(self.updating)
                            .on_click(cx.listener(move |this, _ev, window, cx| {
                                this.update(window, cx);
                            })),
                    ),
            )
    }
}
