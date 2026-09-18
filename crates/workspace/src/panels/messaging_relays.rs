use std::collections::HashSet;

use anyhow::{Error, ensure};
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RelayPurpose { Messaging, PrivateStorage }

impl RelayPurpose {
    fn title(self) -> &'static str {
        match self { Self::Messaging => "Update Messaging Relays", Self::PrivateStorage => "Private Storage Relays" }
    }
    fn description(self) -> &'static str {
        match self {
            Self::Messaging => "Relays where other people send your messages.",
            Self::PrivateStorage => "Sync encrypted drafts across your devices. Leave this list empty to keep drafts local.",
        }
    }
}

pub fn init(window: &mut Window, cx: &mut App) -> Entity<RelayUrlListPanel> {
    cx.new(|cx| RelayUrlListPanel::new(RelayPurpose::Messaging, window, cx))
}

pub fn init_private_storage(window: &mut Window, cx: &mut App) -> Entity<RelayUrlListPanel> {
    cx.new(|cx| RelayUrlListPanel::new(RelayPurpose::PrivateStorage, window, cx))
}

#[derive(Debug)]
pub struct RelayUrlListPanel {
    name: SharedString,
    purpose: RelayPurpose,
    ready: bool,
    newest: Option<Timestamp>,
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

impl RelayUrlListPanel {
    fn new(purpose: RelayPurpose, window: &mut Window, cx: &mut Context<Self>) -> Self {
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
                        this.ready = false;
                        this.newest = None;
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
            name: purpose.title().into(),
            purpose,
            ready: false,
            newest: None,
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
        let purpose = self.purpose;
        let signer = nostr.read(cx).signer().snapshot();
        let task: Task<Result<(Vec<RelayUrl>, Option<Timestamp>), Error>> = cx.background_spawn(async move {
            if purpose == RelayPurpose::PrivateStorage {
                let event = state::private_storage::latest(&client, public_key).await?;
                return match event {
                    Some(event) => Ok((state::private_storage::decode(&event, public_key, &signer).await?, Some(event.created_at))),
                    None => Ok((vec![], None)),
                };
            }
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
            Ok((newest.as_ref().map(|event| nip17::extract_relay_list(event).collect()).unwrap_or_default(), newest.map(|event| event.created_at)))
        });
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.owner != Some(public_key)
                    || NostrRegistry::global(cx).read(cx).current_user() != Some(public_key) { return; }
                this.loading = false;
                match result {
                    Ok((relays, newest)) if !this.dirty => {
                        this.relays = relays.into_iter().collect();
                        this.newest = newest;
                        this.ready = true;
                        this.error = None;
                    },
                    Err(error) => this.error = Some(error.to_string().into()),
                    _ => {}
                }
                cx.notify();
            })?;
            Ok(())
        }));
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ready || self.updating { return; }
        let value = self.input.read(cx).value().trim().to_string();

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
        if !self.ready || self.updating { return; }
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
        if !self.ready || self.updating || self.loading { return; }
        if self.purpose == RelayPurpose::Messaging && self.relays.is_empty() {
            self.set_error("Add at least one relay.", window, cx);
            return;
        }
        let nostr = NostrRegistry::global(cx);
        let Some(owner) = nostr.read(cx).current_user().filter(|owner| Some(*owner) == self.owner) else { return; };
        let client = nostr.read(cx).client();
        let signer = nostr.read(cx).signer().snapshot();
        let purpose = self.purpose;
        let previous = self.newest;
        let mut relays: Vec<_> = self.relays.iter().cloned().collect();
        relays.sort();
        self.error = None;
        self.set_updating(true, cx);

        let task: Task<Result<Timestamp, Error>> = cx.background_spawn(async move {
            ensure!(signer.get_public_key_async().await? == owner, "Signer changed");
            let now = Timestamp::now();
            ensure!(previous.is_none_or(|timestamp| timestamp <= now), "Relay list has a future timestamp; try again shortly");
            let created_at = previous.map(|timestamp| Timestamp::from(now.as_secs().max(timestamp.as_secs() + 1))).unwrap_or(now);
            let event = match purpose {
                RelayPurpose::PrivateStorage => state::private_storage::build(owner, &relays, created_at, &signer).await?,
                RelayPurpose::Messaging => EventBuilder::new(Kind::InboxRelays, "")
                    .tags(relays.into_iter().map(|relay| Tag::from(Nip17Tag::Relay(relay))))
                    .custom_created_at(created_at).finalize_async(&signer).await?,
            };
            // Do not activate a list that no relay accepted.
            let output = client.send_event(&event).to_nip65().save_into_database(false).await?;
            ensure!(output.success.values().any(|status| status.is_ack()), "No relay accepted the relay list");
            client.database().save_event(&event).await?;
            Ok(created_at)
        });
        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                if this.owner != Some(owner) || NostrRegistry::global(cx).read(cx).current_user() != Some(owner) { return; }
                this.set_updating(false, cx);
                match result {
                    Ok(created_at) => {
                        this.dirty = false;
                        this.newest = Some(created_at);
                        if purpose == RelayPurpose::PrivateStorage {
                            chat::ChatRegistry::global(cx).update(cx, |chat, cx| chat.retry_drafts(cx));
                        }
                        window.push_notification("Relays updated", cx);
                    }
                    Err(error) => { this.error = Some(error.to_string().into()); cx.notify(); }
                }
            })?;
            Ok(())
        }));
    }

    fn render_list_items(&mut self, cx: &mut Context<Self>) -> Vec<impl IntoElement> {
        let mut items = Vec::new();

        let mut relays: Vec<_> = self.relays.iter().collect();
        relays.sort();
        for url in relays {
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
                            .disabled(self.updating || !self.ready)
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
            } else if self.purpose == RelayPurpose::PrivateStorage { "Drafts stay on this device." }
            else { "Please add some relays." })
    }
}

impl Panel for RelayUrlListPanel {
    fn panel_id(&self) -> SharedString {
        self.name.clone()
    }

    fn title(&self, _cx: &App) -> AnyElement {
        self.name.clone().into_any_element()
    }
}

impl EventEmitter<PanelEvent> for RelayUrlListPanel {}

impl Focusable for RelayUrlListPanel {
    fn focus_handle(&self, _: &App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for RelayUrlListPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .p_3()
            .gap_3()
            .w_full()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().text_muted)
                    .child(SharedString::from(self.purpose.description())),
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
                                    .child(Input::new(&self.input).small().cleanable(true).disabled(!self.ready || self.updating))
                                    .child(
                                        Button::new("add")
                                            .icon(IconName::Plus)
                                            .tooltip("Add relay")
                                            .disabled(!self.ready || self.updating)
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
                    .when(!self.ready && !self.loading && self.error.is_some(), |view| view.child(
                        Button::new("retry-load").icon(IconName::Reset).label("Load relays").ghost()
                            .on_click(cx.listener(|this, _, window, cx| this.load(window, cx)))))
                    .child(
                        Button::new("submit")
                            .icon(IconName::CheckCircle)
                            .label("Update")
                            .primary()
                            .font_semibold()
                            .loading(self.updating)
                            .disabled(self.updating || self.loading || !self.ready || !self.dirty || NostrRegistry::global(cx).read(cx).current_user().is_none())
                            .on_click(cx.listener(move |this, _ev, window, cx| {
                                this.set_relays(window, cx);
                            })),
                    ),
            )
    }
}
