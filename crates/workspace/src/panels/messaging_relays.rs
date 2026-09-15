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
use smallvec::{SmallVec, smallvec};
use state::{NostrRegistry, StateEvent};
use theme::ActiveTheme;
use ui::button::{Button, ButtonVariants};
use ui::dock::{Panel, PanelEvent};
use ui::input::{Input, InputEvent, InputState};
use ui::{Disableable, IconName, Sizable, StyledExt, WindowExtension, divider, h_flex, v_flex};

const MSG: &str = "Messaging Relays are relays that hosted all your messages. \
                   Other users will find your relays and send messages to it.";

pub fn init(window: &mut Window, cx: &mut App) -> Entity<MessagingRelayPanel> {
    cx.new(|cx| MessagingRelayPanel::new(window, cx))
}

#[derive(Debug)]
pub struct MessagingRelayPanel {
    name: SharedString,
    focus_handle: FocusHandle,

    /// Relay URL input
    input: Entity<InputState>,

    /// Whether the panel is updating
    updating: bool,
    loading: bool,
    dirty: bool,
    owner: Option<PublicKey>,

    /// Error message
    error: Option<SharedString>,

    /// All relays
    relays: HashSet<RelayUrl>,

    /// Event subscriptions
    _subscriptions: SmallVec<[Subscription; 1]>,

    /// Background tasks
    tasks: Vec<Task<Result<(), Error>>>,
}

impl MessagingRelayPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("wss://example.com"));
        let mut subscriptions = smallvec![];

        subscriptions.push(
            // Subscribe to user's input events
            cx.subscribe_in(&input, window, move |this, _input, event, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.add(window, cx);
                }
            }),
        );

        subscriptions.push(cx.subscribe_in(&NostrRegistry::global(cx), window,
            |this, _, event, window, cx| {
                if matches!(event, StateEvent::SignerChanged | StateEvent::NoSigner) {
                    let owner = NostrRegistry::global(cx).read(cx).current_user();
                    if owner != this.owner {
                        this.tasks.clear();
                        this.relays.clear();
                        this.dirty = false;
                        this.updating = false;
                        this.loading = false;
                        this.error = None;
                        this.owner = owner;
                    }
                    this.load(window, cx);
                    cx.notify();
                }
            }));

        // Run at the end of current cycle
        cx.defer_in(window, |this, window, cx| {
            this.load(window, cx);
        });

        Self {
            name: "Update Messaging Relays".into(),
            focus_handle: cx.focus_handle(),
            input,
            updating: false,
            loading: false,
            dirty: false,
            owner: None,
            relays: HashSet::new(),
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

        self.owner = Some(public_key);
        if self.dirty || self.loading { return; }
        self.loading = true;
        cx.notify();
        let task: Task<Result<Vec<RelayUrl>, Error>> = cx.background_spawn(async move {
            let filter = Filter::new().kind(Kind::InboxRelays).author(public_key).limit(1);
            let mut newest = client.database().query(filter.clone()).await?
                .into_iter().next();
            if let Ok(events) = client.fetch_events(filter).timeout(Duration::from_secs(10)).await {
                for event in events {
                    if newest.as_ref().is_none_or(|old| event.created_at > old.created_at
                        || (event.created_at == old.created_at && event.id < old.id)) {
                        newest = Some(event);
                    }
                }
            }
            Ok(newest.map(|event| nip17::extract_relay_list(&event).collect()).unwrap_or_default())
        });
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.owner != Some(public_key)
                    || NostrRegistry::global(cx).read(cx).current_user() != Some(public_key) { return; }
                this.loading = false;
                match result {
                    Ok(relays) if !this.dirty => this.relays = relays.into_iter().collect(),
                    Err(error) => this.error = Some(error.to_string().into()),
                    _ => {}
                }
                cx.notify();
            })?;
            Ok(())
        }));
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.input.read(cx).value().to_string();

        if !value.starts_with("ws") {
            self.set_error("The relay URL is invalid.", window, cx);
            return;
        }

        if let Ok(url) = RelayUrl::parse(&value) {
            if self.relays.insert(url) {
                self.dirty = true;
                self.input.update(cx, |this, cx| {
                    this.set_value("", window, cx);
                });
                cx.notify();
            }
        } else {
            self.set_error("The relay URL is invalid.", window, cx);
        }
    }

    fn remove(&mut self, url: &RelayUrl, cx: &mut Context<Self>) {
        self.dirty = true;
        self.relays.remove(url);
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

    pub fn set_relays(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.relays.is_empty() {
            self.set_error("Add at least one relay.", window, cx);
            return;
        };

        let nostr = NostrRegistry::global(cx);
        let client = nostr.read(cx).client();
        let signer = nostr.read(cx).signer();

        // Construct event tags
        let tags: Vec<Tag> = self
            .relays
            .iter()
            .map(|relay| Tag::from(Nip17Tag::Relay(relay.to_owned())))
            .collect();

        // Set updating state
        self.set_updating(true, cx);

        let task: Task<Result<(), Error>> = cx.background_spawn(async move {
            // Construct nip17 event builder
            let event = EventBuilder::new(Kind::InboxRelays, "")
                .tags(tags)
                .finalize_async(&signer)
                .await?;

            // Set messaging relays
            client.send_event(&event).to_nip65().await?;

            Ok(())
        });

        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            match task.await {
                Ok(_) => {
                    this.update_in(cx, |this, window, cx| {
                        this.set_updating(false, cx);
                        this.dirty = false;
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
        let mut items = Vec::new();

        for url in self.relays.iter() {
            items.push(
                h_flex()
                    .id(SharedString::from(url.to_string()))
                    .group("")
                    .flex_1()
                    .w_full()
                    .h_8()
                    .px_2()
                    .justify_between()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().secondary_background)
                    .text_color(cx.theme().secondary_foreground)
                    .child(div().text_sm().child(SharedString::from(url.to_string())))
                    .child(
                        Button::new("remove_{ix}")
                            .icon(IconName::Close)
                            .xsmall()
                            .ghost()
                            .invisible()
                            .group_hover("", |this| this.visible())
                            .on_click({
                                let url = url.to_owned();
                                cx.listener(move |this, _ev, _window, cx| {
                                    this.remove(&url, cx);
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
            .child(if NostrRegistry::global(cx).read(cx).identity_loading() {
                "Connecting to signer…"
            } else if self.loading {
                "Loading relays…"
            } else { "Please add some relays." })
    }
}

impl Panel for MessagingRelayPanel {
    fn panel_id(&self) -> SharedString {
        self.name.clone()
    }

    fn title(&self, _cx: &App) -> AnyElement {
        self.name.clone().into_any_element()
    }
}

impl EventEmitter<PanelEvent> for MessagingRelayPanel {}

impl Focusable for MessagingRelayPanel {
    fn focus_handle(&self, _: &App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MessagingRelayPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .p_3()
            .gap_3()
            .w_full()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().text_muted)
                    .child(SharedString::from(MSG)),
            )
            .child(divider(cx))
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
                            .child(SharedString::from("Relays:")),
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
                                            .tooltip("Add relay")
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
                        if self.relays.is_empty() {
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
                            .disabled(self.updating || self.loading || NostrRegistry::global(cx).read(cx).current_user().is_none())
                            .on_click(cx.listener(move |this, _ev, window, cx| {
                                this.set_relays(window, cx);
                            })),
                    ),
            )
    }
}
