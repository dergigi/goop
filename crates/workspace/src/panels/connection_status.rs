//! Read-only diagnostics with explicit, narrowly scoped recovery actions.
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use chat::ChatRegistry;
use futures::{FutureExt, StreamExt, stream::BoxStream};
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, ClipboardItem, Context, Entity, EventEmitter, FocusHandle,
    Focusable, InteractiveElement, StatefulInteractiveElement, IntoElement, ParentElement, Render, SharedString, Styled, Subscription, Task,
    Window,
};
use nostr_sdk::prelude::*;
use state::{NostrRegistry, USER_GIFTWRAP};
use theme::ActiveTheme;
use ui::button::{Button, ButtonVariants};
use ui::dock::{Panel, PanelEvent};
use ui::scroll::{Scrollbar, ScrollableElement};
use ui::{Disableable, Icon, IconName, Sizable, WindowExtension, h_flex, v_flex};

#[derive(Clone, Debug, PartialEq, Eq)]
struct RelayState {
    url: RelayUrl,
    status: Option<RelayStatus>,
    auth: Option<&'static str>,
    closed: Option<String>,
}

struct RelayWatch {
    stream: BoxStream<'static, RelayNotification>,
    auth: Option<&'static str>,
    closed: Option<String>,
}

impl RelayWatch {
    fn observe(&mut self, event: RelayNotification) {
        match event {
            RelayNotification::Authenticated => self.auth = Some("Authenticated"),
            RelayNotification::AuthenticationFailed => self.auth = Some("Authentication failed"),
            RelayNotification::RelayStatus { status } if status != RelayStatus::Connected => {
                self.auth = None;
                self.closed = None;
            }
            RelayNotification::Message { message } => match *message {
                RelayMessage::Auth { .. } => self.auth = Some("Waiting for signer authentication"),
                RelayMessage::Closed {
                    subscription_id,
                    message,
                } if subscription_id.as_str() == USER_GIFTWRAP => {
                    self.closed = Some(message.to_string());
                }
                RelayMessage::Event {
                    subscription_id, ..
                }
                | RelayMessage::EndOfStoredEvents(subscription_id)
                    if subscription_id.as_str() == USER_GIFTWRAP =>
                {
                    self.closed = None
                }
                _ => {}
            },
            _ => {}
        }
    }
}

async fn sample(
    client: &Client,
    owner: PublicKey,
    watches: &mut BTreeMap<RelayUrl, RelayWatch>,
    history_urls: BTreeSet<RelayUrl>,
) -> anyhow::Result<(Vec<RelayState>, Vec<RelayState>)> {
    let events = client
        .database()
        .query(Filter::new().kind(Kind::InboxRelays).author(owner).limit(1))
        .await?;
    let urls: BTreeSet<RelayUrl> = events
        .into_iter()
        .next()
        .map(|event| nip17::extract_relay_list(&event).collect())
        .unwrap_or_default();
    let all_urls: BTreeSet<_> = urls.union(&history_urls).cloned().collect();
    watches.retain(|url, _| all_urls.contains(url));
    let mut result = Vec::new();
    for url in all_urls {
        let relay = client.relay(&url).await?;
        let mut state = RelayState {
            url: url.clone(),
            status: relay.as_ref().map(Relay::status),
            auth: None,
            closed: None,
        };
        if let Some(relay) = relay {
            let watch = watches.entry(url).or_insert_with(|| RelayWatch {
                stream: relay
                    .notifications()
                    .filter(|event| {
                        futures::future::ready(match event {
                            RelayNotification::Event { .. } => false,
                            RelayNotification::Message { message } => match message.as_ref() {
                                RelayMessage::Event {
                                    subscription_id, ..
                                } => subscription_id.as_str() == USER_GIFTWRAP,
                                RelayMessage::Ok { .. } | RelayMessage::Notice { .. } => false,
                                _ => true,
                            },
                            _ => true,
                        })
                    })
                    .boxed(),
                auth: None,
                closed: None,
            });
            // Bound the work per tick even on a busy relay.
            for _ in 0..256 {
                let Some(Some(event)) = watch.stream.next().now_or_never() else {
                    break;
                };
                watch.observe(event);
            }
            state.status = Some(relay.status());
            state.auth = watch.auth;
            state.closed = watch.closed.clone();
        }
        result.push(state);
    }
    Ok(result.into_iter().partition(|relay| urls.contains(&relay.url)))
}

#[derive(Default, Debug, PartialEq, Eq)]
struct Delivery {
    pending: usize,
    preparing: usize,
    paused: usize,
    failed: usize,
    reasons: BTreeSet<String>,
}
impl Delivery {
    fn collect<'a>(reports: impl Iterator<Item = &'a Vec<chat::SendReport>>) -> Self {
        let mut result = Self::default();
        for reports in reports {
            let outstanding: Vec<_> = reports.iter().filter(|report| !report.success()).collect();
            if outstanding.is_empty() {
                continue;
            }
            if outstanding.iter().any(|r| r.paused) {
                result.paused += 1;
            } else if outstanding.iter().any(|r| r.failed()) {
                result.failed += 1;
            } else {
                result.pending += 1;
                if outstanding.iter().any(|r| r.gift_wrap_id.is_none()) {
                    result.preparing += 1;
                }
            }
            for report in outstanding {
                if let Some(error) = &report.error {
                    result.reasons.insert(error.to_string());
                }
                if let Some(output) = &report.output {
                    for (url, error) in &output.failed {
                        result.reasons.insert(format!("{url}: {error}"));
                    }
                }
            }
        }
        result
    }
}

fn connection_summary(
    loading: bool,
    signer_error: bool,
    signed_in: bool,
    relays: Option<&[RelayState]>,
    error: bool,
) -> String {
    let message = if signer_error {
        "Signer unavailable"
    } else if loading {
        "Connecting to signer…"
    } else if !signed_in {
        "Connect your signer"
    } else if error {
        "Connection status unavailable"
    } else if let Some(relays) = relays {
        if relays.is_empty() {
            "No messaging relays found"
        } else if relays
            .iter()
            .all(|r| r.status != Some(RelayStatus::Connected))
        {
            if relays.iter().any(|r| {
                matches!(
                    r.status,
                    Some(RelayStatus::Initialized | RelayStatus::Pending | RelayStatus::Connecting)
                )
            }) {
                "Connecting to messaging relays…"
            } else {
                "Messaging relays disconnected"
            }
        } else {
            let connected = relays.iter().filter(|r| r.status == Some(RelayStatus::Connected)).count();
            return format!("{connected}/{} relays connected", relays.len());
        }
    } else {
        "Checking messaging relays…"
    };
    message.into()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConnectionHealth { Connected, Connecting, Attention, Offline }

fn connection_health(loading: bool, signer_error: bool, signed_in: bool,
    relays: Option<&[RelayState]>, error: bool) -> ConnectionHealth {
    if signer_error { return ConnectionHealth::Attention; }
    if loading { return ConnectionHealth::Connecting; }
    if !signed_in { return ConnectionHealth::Offline; }
    if error { return ConnectionHealth::Attention; }
    match relays {
        Some(relays) if relays.iter().any(|relay| relay.status == Some(RelayStatus::Connected)) => ConnectionHealth::Connected,
        Some(relays) if relays.iter().any(|relay| matches!(relay.status,
            Some(RelayStatus::Initialized | RelayStatus::Pending | RelayStatus::Connecting))) => ConnectionHealth::Connecting,
        Some(_) => ConnectionHealth::Attention,
        None => ConnectionHealth::Connecting,
    }
}

fn status_viewport(content: impl IntoElement, scroll: &gpui::ScrollHandle) -> impl IntoElement {
    gpui::div().size_full().min_h_0().min_w_0().relative().overflow_hidden()
        .child(gpui::div().id("connection-status-scroll").size_full().overflow_y_scroll()
            .track_scroll(scroll).child(content))
        .child(Scrollbar::vertical(scroll))
}

pub fn init(cx: &mut App) -> Entity<ConnectionStatus> {
    cx.new(|cx| {
        let nostr = NostrRegistry::global(cx);
        let owner = nostr.read(cx).current_user();
        let mut subscriptions = vec![cx.observe(&ChatRegistry::global(cx), |_, _, cx| cx.notify())];
        subscriptions.push(
            cx.observe(&nostr, |this: &mut ConnectionStatus, nostr, cx| {
                let owner = nostr.read(cx).current_user();
                if owner != this.owner {
                    this.owner = owner;
                    this.relays = None;
                    this.history_relays.clear();
                    this.error = None;
                }
                cx.notify();
            }),
        );
        let task = cx.spawn(async move |this, cx| {
            let mut sampled_owner = None;
            let mut watches = BTreeMap::new();
            loop {
                let (owner, client, history_urls) = this.read_with(cx, |this, cx| {
                    (this.owner, NostrRegistry::global(cx).read(cx).client(),
                        ChatRegistry::global(cx).read(cx).history_relays().keys().cloned().collect())
                })?;
                if sampled_owner != owner {
                    watches.clear();
                    sampled_owner = owner;
                }
                if let Some(owner) = owner {
                    let (next_watches, result) = cx
                        .background_spawn(async move {
                            let result = sample(&client, owner, &mut watches, history_urls).await;
                            (watches, result)
                        })
                        .await;
                    watches = next_watches;
                    this.update(cx, |this, cx| {
                        if this.owner != Some(owner) {
                            return;
                        }
                        match result {
                            Ok((relays, history_relays)) => {
                                this.relays = Some(relays);
                                this.history_relays = history_relays;
                                this.error = None;
                            }
                            Err(error) => {
                                this.relays = None;
                                this.history_relays.clear();
                                this.error = Some(error.to_string());
                            }
                        }
                        cx.notify();
                    })?;
                }
                cx.background_executor().timer(Duration::from_secs(2)).await;
            }
            #[allow(unreachable_code)]
            Ok(())
        });
        ConnectionStatus {
            show_troubleshooting: false,
            scroll: gpui::ScrollHandle::new(),
            owner,
            relays: None,
            history_relays: Vec::new(),
            error: None,
            focus: cx.focus_handle(),
            _task: task,
            _subscriptions: subscriptions,
        }
    })
}

pub struct ConnectionStatus {
    show_troubleshooting: bool,
    scroll: gpui::ScrollHandle,
    owner: Option<PublicKey>,
    relays: Option<Vec<RelayState>>,
    history_relays: Vec<RelayState>,
    error: Option<String>,
    focus: FocusHandle,
    _task: Task<anyhow::Result<()>>,
    _subscriptions: Vec<Subscription>,
}

impl ConnectionStatus {
    pub fn indicator(&self, cx: &App) -> impl IntoElement + use<> {
        let nostr = NostrRegistry::global(cx);
        let nostr = nostr.read(cx);
        let health = connection_health(nostr.identity_loading(), nostr.signer_connection_error().is_some(),
            self.owner.is_some(), self.relays.as_deref(), self.error.is_some());
        let color = match health {
            ConnectionHealth::Connected => gpui::rgb(0x22a34a).into(),
            ConnectionHealth::Attention => cx.theme().text_warning,
            ConnectionHealth::Connecting | ConnectionHealth::Offline => cx.theme().icon_muted,
        };
        gpui::div().size(gpui::px(6.)).flex_shrink_0().rounded_full().bg(color)
    }

    pub fn summary(&self, cx: &App) -> String {
        let nostr = NostrRegistry::global(cx);
        let nostr = nostr.read(cx);
        connection_summary(
            nostr.identity_loading(),
            nostr.signer_connection_error().is_some(),
            self.owner.is_some(),
            self.relays.as_deref(),
            self.error.is_some(),
        )
    }

    fn details(&self, cx: &App) -> Vec<(String, String)> {
        let nostr = NostrRegistry::global(cx);
        let nostr = nostr.read(cx);
        let signer = if let Some(error) = nostr.signer_connection_error() {
            error.to_owned()
        } else if nostr.identity_loading() {
            "Waiting for the signer to confirm your identity. Open your signer and check for a request.".into()
        } else if self.owner.is_some() {
            "Identity connected. Signing or decryption requests may still need approval in your signer.".into()
        } else {
            "Connect your signer to send and receive messages.".into()
        };
        let mut sections = vec![("Signer".into(), signer)];
        if self.owner.is_none() {
            return sections;
        }
        if let Some(error) = &self.error {
            sections.push((
                "Messaging relays".into(),
                format!("Could not check relay status: {error}"),
            ));
        } else if let Some(relays) = &self.relays {
            if relays.is_empty() {
                sections.push((
                    "Messaging relays".into(),
                    "No messaging relays found yet. Check your relay list or add a relay to receive messages.".into(),
                ));
            }
            for relay in relays {
                let mut text = match relay.status {
                    Some(RelayStatus::Connected) => "Connected",
                    Some(
                        RelayStatus::Initialized | RelayStatus::Pending | RelayStatus::Connecting,
                    ) => "Connecting…",
                    Some(RelayStatus::Disconnected) => "Disconnected · waiting to reconnect",
                    Some(RelayStatus::Terminated) => "Disconnected · reconnect to resume",
                    Some(RelayStatus::Banned) => "Relay disabled",
                    Some(RelayStatus::Sleeping) => "Connection sleeping",
                    Some(RelayStatus::Shutdown) => "Connection shut down",
                    None => "Not connected",
                }
                .to_owned();
                if let Some(auth) = relay.auth {
                    text.push_str(&format!(" · {auth}"));
                }
                if let Some(error) = &relay.closed {
                    text.push_str(&format!("\nMessage subscription closed: {error}"));
                }
                sections.push((relay.url.to_string(), text));
            }
        } else {
            sections.push((
                "Messaging relays".into(),
                "Checking configured relays…".into(),
            ));
        }
        let chat = ChatRegistry::global(cx);
        let chat = chat.read(cx);
        sections.push(("Message history".into(), chat.history_summary(cx)));
        for (url, progress) in chat.history_relays() {
            if let Some(error) = &progress.error {
                sections.push((format!("History: {url}"), error.clone()));
            }
        }
        sections.push(("History coverage".into(), "A finished scan only covers messages retained by those relays. It does not prove your full history was recovered.".into()));
        if !chat.decryption_failures(cx).is_empty() {
            sections.push(("Decryption".into(), String::new()));
        }
        let delivery = Delivery::collect(chat.delivery_reports());
        sections.push(("Outgoing messages".into(), String::new()));
        if delivery.preparing > 0 {
            sections.push(("Message signing".into(), format!("{} messages still need an encrypted, signed copy. Check your signer for approval requests.", delivery.preparing)));
        }
        if let Some(error) = chat.outgoing_error() {
            sections.push(("Outgoing queue error".into(), error.into()));
        }
        for reason in delivery.reasons {
            sections.push(("Delivery error".into(), reason));
        }
        sections
    }
}

impl Panel for ConnectionStatus {
    fn panel_id(&self) -> SharedString {
        "connection-status".into()
    }
    fn title(&self, cx: &App) -> AnyElement {
        let title = match self.relays.as_deref() {
            Some(relays) => {
                let connected = relays.iter().filter(|relay| relay.status == Some(RelayStatus::Connected)).count();
                format!("Connection status: {connected}/{}", relays.len())
            }
            None => "Connection status".into(),
        };
        h_flex().gap_2().items_center().child(self.indicator(cx)).child(title).into_any_element()
    }
}
impl EventEmitter<PanelEvent> for ConnectionStatus {}
impl Focusable for ConnectionStatus {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for ConnectionStatus {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let details = self.details(cx);
        let relay_urls: BTreeSet<String> = self.relays.as_ref().into_iter().flatten()
            .map(|relay| relay.url.to_string()).collect();
        let mut details = details.into_iter();
        let signer_detail = details.next().map(|(_, text)| text).unwrap_or_default();
        let details: Vec<_> = details.filter(|(heading, _)| !relay_urls.contains(heading) && !heading.starts_with("History: ")).collect();
        let signed_in = self.owner.is_some();
        let nostr = NostrRegistry::global(cx);
        let signer = nostr.read(cx);
        let signer_needs_setup = signer.needs_signer_setup();
        let reconnecting = signer.identity_loading() && signer.signer_connection_error().is_none();
        let signer_needs_retry = !signer_needs_setup
            && (signer.identity_loading() || signer.signer_connection_error().is_some() || !signed_in);
        let signer_connected = signed_in && !signer_needs_setup && !signer_needs_retry;
        let chat = ChatRegistry::global(cx);
        let chat = chat.read(cx);
        let delivery = Delivery::collect(chat.delivery_reports());
        let decrypt_failed = chat.count_trash_messages(cx) > 0;
        let history_running = chat.history_running();
        let relays_found = self
            .relays
            .as_ref()
            .is_some_and(|relays| !relays.is_empty());
        let send_pending = delivery.pending + delivery.paused + delivery.failed > 0
            || chat.outgoing_error().is_some();
        let content = v_flex().w_full().min_w_0().p_4().gap_3().flex_shrink_0()
            .child(v_flex().gap_2()
                .child(h_flex().gap_2().child(Icon::new(IconName::UserKey).small()).child("Signer"))
                .child(gpui::div().text_sm().text_color(cx.theme().text_muted).child(signer_detail))
                .when(signer_needs_setup || signer_needs_retry, |view| view.child(v_flex().w_full().gap_2()
                    .when(signer_needs_setup, |view| view.child(
                        Button::new("setup-status-signer").label("Connect your signer").small().primary()
                            .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::Command::ConnectSigner), cx))))
                    .when(signer_needs_retry, |view| view.child(status_action("retry-status-signer", "Reconnect signer", IconName::Refresh, None, window).disabled(reconnecting)
                        .on_click(|_, _, cx| NostrRegistry::global(cx).update(cx, |nostr, cx| nostr.retry_signer(cx))))))))
            .when(signed_in && !relay_urls.is_empty(), |view| view.child(
                v_flex().gap_2()
                    .child(h_flex().gap_2().child(Icon::new(IconName::Relay).small()).child("Messaging relays"))
                    .children(self.relays.as_ref().into_iter().flatten().map(|relay| relay_row(relay, chat.history_relays().get(&relay.url).map(|progress| {
                        format!("{} messages received · {}", progress.received,
                            if progress.error.is_some() { "Scan incomplete" } else if progress.done { "History checked" } else { "Loading history…" })
                    }), chat.history_relays().get(&relay.url).and_then(|progress| progress.error.clone()), cx)))
            ))
            .children(details.into_iter().map(|(heading, detail)| {
                let history = heading == "Message history";
                v_flex().gap_1().when(!history, |view| view.child(heading.clone()))
                    .child(if history {
                        history_progress(chat.history_status(cx), history_running, cx).into_any_element()
                    } else if heading == "Decryption" {
                        v_flex().w_full().min_w_0()
                            .children(chat.decryption_failures(cx).into_iter().enumerate().map(|(index, (reason, count))| {
                                status_counter("decryption-counter", index, reason, count, cx)
                            })).into_any_element()
                    } else if heading == "Outgoing messages" {
                        v_flex().w_full().min_w_0()
                            .children([("Pending", delivery.pending), ("Paused", delivery.paused), ("Delayed", delivery.failed)]
                                .into_iter().enumerate().map(|(index, (label, count))| {
                                    status_counter("outgoing-counter", index, label.into(), count, cx)
                                }))
                            .child(gpui::div().mt_1().text_sm().text_color(cx.theme().text_muted)
                                .child("Delayed sends retry automatically; paused sends wait for your explicit retry. Counts include your own encrypted copy. Relay acceptance is not a read receipt."))
                            .into_any_element()
                    } else {
                        gpui::div().text_sm().text_color(cx.theme().text_muted).child(detail).into_any_element()
                    })
                    .when(history, |view| view.children(chat.history_relays().iter()
                        .filter(|(url, _)| !relay_urls.contains(&url.to_string()))
                        .map(|(url, progress)| {
                            let state = self.history_relays.iter().find(|relay| &relay.url == url).cloned()
                                .unwrap_or_else(|| RelayState { url: url.clone(), status: None, auth: None, closed: None });
                            relay_row(&state, Some(format!("{} messages received · {}", progress.received,
                                if progress.error.is_some() { "Scan incomplete" } else if progress.done { "History checked" } else { "Loading history…" })),
                                progress.error.clone(), cx).into_any_element()
                        })))
            }))
            .child(h_flex().child(Button::new("toggle-status-troubleshooting")
                .icon(if self.show_troubleshooting { IconName::CaretDown } else { IconName::CaretRight })
                .label("Troubleshooting").small().ghost()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.show_troubleshooting = !this.show_troubleshooting;
                    cx.notify();
                }))))
            .when(self.show_troubleshooting, |view| view.child(v_flex().gap_3()
            .child(v_flex().w_full().gap_1()
                .child(status_action("manage-status-relays", "Manage messaging relays", IconName::Relay, Some(crate::Command::ShowMessaging), window)
                    .on_click(|_, window, cx| {
                        let panel = super::messaging_relays::init(window, cx);
                        crate::Workspace::add_panel(panel, ui::dock::DockPlacement::Right, window, cx);
                    }))
                .child(status_action("manage-status-gossip", "Manage gossip relays", IconName::Group, Some(crate::Command::ShowRelayList), window)
                    .on_click(|_, window, cx| {
                        let panel = super::relay_list::init(window, cx);
                        crate::Workspace::add_panel(panel, ui::dock::DockPlacement::Right, window, cx);
                    }))
                .child(status_action("retry-status-relays", "Reconnect messaging relays", IconName::Refresh, None, window).disabled(!signed_in || !relays_found)
                    .on_click(cx.listener(|this, _, window, cx| {
                        let urls: Vec<_> = this.relays.as_ref().into_iter().flatten().map(|r| r.url.clone()).collect();
                        let client = NostrRegistry::global(cx).read(cx).client();
                        let task = cx.background_spawn(async move {
                            for url in urls {
                                client.add_relay(&url).await?;
                                if let Some(relay) = client.relay(&url).await? { relay.disconnect(); relay.connect(); }
                            }
                            anyhow::Ok(())
                        });
                        cx.spawn_in(window, async move |_, cx| {
                            if let Err(error) = task.await { cx.update(|window, cx| window.push_notification(error.to_string(), cx)).ok(); }
                        }).detach();
                    }))))
            .child(v_flex().w_full().gap_1()
                .child(status_action("status-rescan", "Resume history", IconName::History, None, window).disabled(!signed_in || history_running)
                    .on_click(|_, _, cx| ChatRegistry::global(cx).update(cx, |chat, cx| chat.resume_history(cx))))
                .child(status_action("status-full-rescan", "Rescan all history", IconName::Reset, Some(crate::Command::LoadOlderHistory), window).disabled(!signed_in || history_running)
                    .on_click(|_, _, cx| ChatRegistry::global(cx).update(cx, |chat, cx| chat.load_older_history(cx))))
                .child(status_action("status-broaden", "Scan other relays", IconName::Search, Some(crate::Command::SearchOtherRelays), window).disabled(!signed_in || history_running)
                    .on_click(|_, _, cx| ChatRegistry::global(cx).update(cx, |chat, cx| chat.search_other_relays(cx))))
                .child(status_action("status-decrypt", "Retry decryption", IconName::UserKey, Some(crate::Command::RetryDecryption), window).disabled(!signed_in || !decrypt_failed)
                    .on_click(|_, _, cx| ChatRegistry::global(cx).update(cx, |chat, cx| chat.retry_failed_messages(cx))))
                .child(status_action("status-send", "Retry pending sends", IconName::PaperPlaneFill, None, window).disabled(!signed_in || !send_pending)
                    .on_click(|_, _, cx| ChatRegistry::global(cx).read(cx).retry_outgoing()))
                .when(signer_connected, |view| view.child(
                    status_action("disconnect-status-signer", "Disconnect signer", IconName::Door, Some(crate::Command::Logout), window)
                        .danger()
                        .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::Command::Logout), cx)))))
            .child(gpui::div().text_xs().text_color(cx.theme().text_muted)
                .child("Retrying decryption or sends also resumes requests you previously declined. Your signer may ask for approval again."))));
        status_viewport(content, &self.scroll)
    }
}

fn history_progress(progress: chat::HistoryStatus, history_running: bool, cx: &App) -> impl IntoElement {
    let counters = [("Received", progress.received), ("Loaded", progress.loaded),
        ("Pending", progress.pending), ("Failed", progress.failed), ("Relay errors", progress.relay_errors)];
    v_flex().w_full().min_w_0().flex_shrink_0()
        .child(h_flex().id("history-phase").w_full().h_6().flex_shrink_0().gap_2()
            .tooltip(move |window, cx| ui::tooltip::Tooltip::new(progress.status, window, cx).into())
            .child(gpui::div().flex_1().min_w_0().truncate().child("Message history"))
            .child(gpui::div().size_4().flex_shrink_0()
                .when(history_running || progress.pending > 0, |view| view.child(
                    ui::indicator::Indicator::new().small().color(cx.theme().text_muted)))))
        .children(counters.into_iter().enumerate().map(|(index, (label, count))| {
            status_counter("history-counter", index, label.into(), count, cx)
        }))
        .when_some(progress.error, |view, error| view.child(gpui::div().text_sm().text_color(cx.theme().text_muted).child(error)))
}

/// Keep diagnostics in stable rows, with full details available on hover.
fn status_counter(id: &'static str, index: usize, label: String, count: usize, cx: &App) -> impl IntoElement {
    let tooltip = format!("{label}: {count}");
    h_flex().id((id, index)).w_full().min_w_0().h_6().flex_shrink_0().gap_2()
        .text_sm().text_color(cx.theme().text_muted)
        .tooltip(move |window, cx| ui::tooltip::Tooltip::new(tooltip.clone(), window, cx).into())
        .child(gpui::div().flex_1().min_w_0().truncate().child(label))
        .child(gpui::div().flex_shrink_0().max_w(gpui::relative(0.5)).truncate().text_right().child(count.to_string()))
}

/// Same row proportions as the sidebar's primary actions. Read shortcuts from
/// the keymap so platform-specific badges only appear for bound actions.
fn status_action(
    id: &'static str,
    label: &'static str,
    icon: IconName,
    action: Option<crate::Command>,
    window: &Window,
) -> Button {
    let shortcut = action.and_then(|action| ui::Kbd::binding_for_action(&action, None, window));
    Button::new(id).ghost().align_left().w_full().h_8().px_3()
        .child(h_flex().w_full().min_w_0().gap_2()
            .child(Icon::new(icon).small().flex_shrink_0())
            .child(gpui::div().flex_1().min_w_0().truncate().child(label))
            .when_some(shortcut, |row, shortcut| row.child(shortcut)))
}

fn relay_errors(relay: &RelayState, history_error: Option<&str>) -> Vec<String> {
    let mut errors = Vec::new();
    if relay.auth == Some("Authentication failed") {
        errors.push("Relay authentication failed.".to_owned());
    }
    if let Some(error) = &relay.closed {
        errors.push(error.clone());
    }
    if let Some(error) = history_error {
        if !errors.iter().any(|existing| existing == error) {
            errors.push(error.to_owned());
        }
    }
    errors
}

fn relay_row(relay: &RelayState, history: Option<String>, history_error: Option<String>, cx: &App) -> impl IntoElement {
    let (connection, color) = match relay.status {
        Some(RelayStatus::Connected) => ("Connected", gpui::rgb(0x22a34a).into()),
        Some(RelayStatus::Initialized | RelayStatus::Pending | RelayStatus::Connecting) =>
            ("Connecting…", cx.theme().text_warning),
        Some(RelayStatus::Sleeping) => ("Sleeping", cx.theme().icon_muted),
        Some(RelayStatus::Banned) => ("Disabled", cx.theme().icon_muted),
        Some(RelayStatus::Shutdown) => ("Shut down", cx.theme().icon_muted),
        None => ("Connection status not observed", cx.theme().icon_muted),
        _ => ("Disconnected", cx.theme().icon_muted),
    };
    let errors = relay_errors(relay, history_error.as_deref());
    let error = (!errors.is_empty()).then(|| errors.join("\n\n"));
    let url = relay.url.to_string();
    let tooltip = format!("{url}\n{connection}{}{}",
        relay.auth.map(|auth| format!(" · {auth}")).unwrap_or_default(),
        history.map(|history| format!("\n{history}")).unwrap_or_default());
    h_flex().id(SharedString::from(format!("relay-status-{url}")))
        .w_full().min_w_0().flex_shrink_0().min_h(gpui::px(28.)).py_1().gap_2()
        .tooltip(move |window, cx| ui::tooltip::Tooltip::new(tooltip.clone(), window, cx).into())
        .child(gpui::div().size(gpui::px(6.)).flex_shrink_0().rounded_full().bg(color))
        .child(gpui::div().flex_1().min_w_0().truncate().text_sm()
            .child(url.trim_start_matches("wss://").trim_start_matches("ws://").trim_end_matches('/').to_owned()))
        .when(relay.auth == Some("Authenticated"), |row| row.child(
            gpui::div().id("authenticated").flex_shrink_0()
                .tooltip(|window, cx| ui::tooltip::Tooltip::new("Authenticated", window, cx).into())
                .child(Icon::new(IconName::Shield).xsmall().text_color(cx.theme().icon_muted))))
        .when(relay.auth == Some("Waiting for signer authentication"), |row| row.child(
            gpui::div().id("auth-pending").flex_shrink_0()
                .tooltip(|window, cx| ui::tooltip::Tooltip::new("Waiting for signer authentication", window, cx).into())
                .child(Icon::new(IconName::UserKey).xsmall().text_color(cx.theme().text_warning))))
        .when_some(error, |row, error| row.child(
            Button::new("relay-error")
                .icon(Icon::new(IconName::WarningTriangle).text_color(cx.theme().text_warning))
                .xsmall().ghost().tooltip("Show relay error")
                .on_click(move |_, window, cx| {
                    let error = error.clone();
                    let url = url.clone();
                    window.open_modal(cx, move |modal, _, _| {
                        let copy = format!("{url}\n\n{error}");
                        modal.title("Relay error").show_close(true).width(gpui::px(520.))
                            .child(v_flex().gap_3()
                                .child(gpui::div().text_sm().child(url.clone()))
                                .child(v_flex().max_h(gpui::px(320.)).overflow_y_scrollbar()
                                    .child(gpui::div().text_sm().child(error.clone())))
                                .child(Button::new("copy-relay-error").icon(IconName::Copy).label("Copy error").small().ghost()
                                    .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy.clone())))))
                    });
                })))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn relay_error_details_preserve_full_reasons_without_duplicate_history_errors() {
        let reason = "ERROR: auth-required: requested filter requires authentication\nOpen your signer to approve.";
        let relay = RelayState {
            url: RelayUrl::parse("wss://example.com").unwrap(),
            status: Some(RelayStatus::Connected),
            auth: Some("Authentication failed"),
            closed: Some(reason.into()),
        };
        assert_eq!(relay_errors(&relay, Some(reason)), vec!["Relay authentication failed.", reason]);
        let mut healthy = relay.clone();
        healthy.auth = Some("Authenticated");
        healthy.closed = None;
        assert!(relay_errors(&healthy, None).is_empty());
        assert_eq!(relay_errors(&healthy, Some("History connection timed out")), vec!["History connection timed out"]);
    }

    #[test]
    fn signer_and_account_state_take_priority_over_old_relay_data() {
        let old = [RelayState {
            url: RelayUrl::parse("wss://example.com").unwrap(),
            status: Some(RelayStatus::Connected),
            auth: Some("Authentication failed"),
            closed: None,
        }];
        assert_eq!(
            connection_summary(false, false, false, Some(&old), false).as_str(),
            "Connect your signer"
        );
        assert_eq!(
            connection_summary(true, false, true, Some(&old), false).as_str(),
            "Connecting to signer…"
        );
        assert_eq!(
            connection_summary(true, true, true, Some(&old), false).as_str(),
            "Signer unavailable"
        );
    }

    #[test]
    fn unchecked_missing_and_disconnected_relays_are_distinct() {
        assert_eq!(
            connection_summary(false, false, true, None, false).as_str(),
            "Checking messaging relays…"
        );
        assert_eq!(
            connection_summary(false, false, true, Some(&[]), false).as_str(),
            "No messaging relays found"
        );
        let mut relay = RelayState {
            url: RelayUrl::parse("wss://example.com").unwrap(),
            status: Some(RelayStatus::Disconnected),
            auth: None,
            closed: None,
        };
        assert_eq!(
            connection_summary(false, false, true, Some(&[relay.clone()]), false).as_str(),
            "Messaging relays disconnected"
        );
        relay.status = Some(RelayStatus::Connected);
        relay.closed = Some("auth-required: authenticate first".into());
        assert_eq!(
            connection_summary(false, false, true, Some(&[relay]), false).as_str(),
            "1/1 relays connected"
        );
    }

    #[tokio::test]
    async fn history_relays_are_sampled_without_changing_the_messaging_list() {
        let client = ClientBuilder::default().database(nostr_memory::MemoryDatabase::unbounded()).build();
        let owner = Keys::generate();
        let inbox = RelayUrl::parse("wss://inbox.example.com").unwrap();
        let history = RelayUrl::parse("wss://history.example.com").unwrap();
        let event = EventBuilder::new(Kind::InboxRelays, "")
            .tag(Tag::parse(["relay", inbox.as_str()]).unwrap())
            .finalize(&owner).unwrap();
        client.database().save_event(&event).await.unwrap();
        client.add_relay(&inbox).await.unwrap();
        client.add_relay(&history).await.unwrap();
        let mut watches = BTreeMap::new();
        let (messaging, extra) = sample(&client, owner.public_key(), &mut watches,
            BTreeSet::from([inbox.clone(), history.clone()])).await.unwrap();
        assert_eq!(messaging.iter().map(|r| r.url.clone()).collect::<Vec<_>>(), vec![inbox.clone()]);
        assert_eq!(extra.iter().map(|r| r.url.clone()).collect::<Vec<_>>(), vec![history]);
        assert!(extra[0].status.is_some());
        assert_ne!(extra[0].status, Some(RelayStatus::Connected));
        assert!(extra[0].auth.is_none());
        let (_, extra) = sample(&client, owner.public_key(), &mut watches, BTreeSet::new()).await.unwrap();
        assert!(extra.is_empty());
        assert_eq!(watches.keys().cloned().collect::<Vec<_>>(), vec![inbox]);
        client.shutdown().await;
    }

    #[test]
    fn connected_count_survives_individual_auth_and_subscription_errors() {
        let mut relays = (0..6).map(|i| RelayState {
            url: RelayUrl::parse(&format!("wss://relay{i}.example.com")).unwrap(),
            status: Some(RelayStatus::Connected), auth: None, closed: None,
        }).collect::<Vec<_>>();
        relays[0].auth = Some("Authentication failed");
        relays[0].closed = Some("auth-required".into());
        relays[5].status = Some(RelayStatus::Disconnected);
        assert_eq!(connection_summary(false, false, true, Some(&relays), false).as_str(), "5/6 relays connected");
        assert!(!relay_errors(&relays[0], None).is_empty());
    }

    #[test]
    fn authentication_failure_clears_on_reconnect_and_success() {
        let mut watch = RelayWatch {
            stream: futures::stream::empty().boxed(),
            auth: None,
            closed: None,
        };
        watch.observe(RelayNotification::AuthenticationFailed);
        assert_eq!(watch.auth, Some("Authentication failed"));
        watch.observe(RelayNotification::RelayStatus {
            status: RelayStatus::Disconnected,
        });
        assert_eq!(watch.auth, None);
        watch.observe(RelayNotification::Authenticated);
        assert_eq!(watch.auth, Some("Authenticated"));
    }

    #[test]
    fn only_live_message_subscription_errors_affect_receiving_status() {
        let mut watch = RelayWatch {
            stream: futures::stream::empty().boxed(),
            auth: None,
            closed: None,
        };
        for id in ["profile-lookup", USER_GIFTWRAP] {
            watch.observe(RelayNotification::Message {
                message: Box::new(RelayMessage::Closed {
                    subscription_id: std::borrow::Cow::Owned(SubscriptionId::new(id)),
                    message: "restricted".into(),
                }),
            });
            assert_eq!(watch.closed.is_some(), id == USER_GIFTWRAP);
        }
        watch.observe(RelayNotification::Message {
            message: Box::new(RelayMessage::EndOfStoredEvents(std::borrow::Cow::Owned(
                SubscriptionId::new(USER_GIFTWRAP),
            ))),
        });
        assert_eq!(watch.closed, None);
    }

    #[test]
    fn delivery_counts_messages_once_and_keeps_self_copy_failures() {
        let key = Keys::generate().public_key();
        let mut sent = chat::SendReport::new(key);
        sent.accepted = true;
        let mut paused = chat::SendReport::new(key);
        paused.paused = true;
        let mut failed = chat::SendReport::new(key).error("self copy rejected");
        failed.self_copy = true;
        let reports = vec![
            vec![sent.clone()],
            vec![paused, failed.clone()],
            vec![sent, failed],
            vec![chat::SendReport::new(key)],
        ];
        let result = Delivery::collect(reports.iter());
        assert_eq!((result.pending, result.paused, result.failed), (1, 1, 1));
        assert_eq!(result.reasons.len(), 1);
    }
}

#[cfg(test)]
mod health_tests {
    use super::*;
    #[test]
    fn partial_relay_connectivity_stays_positive_but_signer_failure_takes_priority() {
        let relays = [RelayState { url: RelayUrl::parse("wss://example.com").unwrap(),
            status: Some(RelayStatus::Connected), auth: None, closed: None },
            RelayState { url: RelayUrl::parse("wss://other.example.com").unwrap(),
            status: Some(RelayStatus::Disconnected), auth: None, closed: None }];
        assert_eq!(connection_health(false, false, true, Some(&relays), false), ConnectionHealth::Connected);
        assert_eq!(connection_health(false, true, true, Some(&relays), false), ConnectionHealth::Attention);
        assert_eq!(connection_health(true, false, false, None, false), ConnectionHealth::Connecting);
        assert_eq!(connection_health(false, false, false, None, false), ConnectionHealth::Offline);
        assert_eq!(connection_health(false, false, true, Some(&relays[1..]), false), ConnectionHealth::Attention);
        assert_eq!(connection_health(false, false, true, None, false), ConnectionHealth::Connecting);
    }
}

#[cfg(all(test, feature = "test-support"))]
mod scroll_tests {
    use super::*;
    use gpui::{point, px, size, TestAppContext};

    struct ScrollHarness { scroll: gpui::ScrollHandle }
    impl Render for ScrollHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            status_viewport(v_flex().w_full().children((0..40).map(|index| {
                gpui::div().h(px(40.)).flex_shrink_0().child(format!("Relay {index}"))
            })), &self.scroll)
        }
    }

    struct HistoryHarness { count: usize, scroll: gpui::ScrollHandle }
    impl Render for HistoryHarness {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            status_viewport(v_flex().child(history_progress(chat::HistoryStatus {
                status: if self.count == usize::MAX { "Loading history · broader relay search queued" } else { "Loading history" },
                received: self.count, loaded: self.count, pending: self.count,
                failed: self.count, relay_errors: self.count, error: None,
            }, self.count > 0, cx)).child(gpui::div().h(px(400.)).flex_shrink_0()), &self.scroll)
        }
    }

    #[gpui::test]
    fn growing_history_counts_do_not_move_content_below_them(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        let scroll = gpui::ScrollHandle::new();
        let view = window.update(|_, cx| {
            theme::init(cx); ui::init(cx);
            cx.new(|_| HistoryHarness { count: 0, scroll: scroll.clone() })
        });
        for width in [240., 160.] {
            let draw = |_: &mut Window, _: &mut App| view.clone().into_any_element();
            window.update(|_, cx| view.update(cx, |view, cx| { view.count = 0; cx.notify(); }));
            window.draw(point(px(0.), px(0.)), size(px(width), px(240.)), draw);
            let original = scroll.max_offset().y;
            assert!(original > px(0.));
            for count in [9, 1051, 95774, usize::MAX] {
                window.update(|_, cx| view.update(cx, |view, cx| { view.count = count; cx.notify(); }));
                window.draw(point(px(0.), px(0.)), size(px(width), px(240.)), draw);
                assert_eq!(scroll.max_offset().y, original, "history height changed at {count} in a {width}px pane");
            }
        }
    }

    #[gpui::test]
    fn long_status_content_scrolls_in_short_viewport(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        let scroll = gpui::ScrollHandle::new();
        let view = window.update(|_, cx| {
            theme::init(cx); ui::init(cx);
            cx.new(|_| ScrollHarness { scroll: scroll.clone() })
        });
        let draw = |_: &mut Window, _: &mut App| view.clone().into_any_element();
        window.draw(point(px(0.), px(0.)), size(px(280.), px(240.)), draw);
        assert!(scroll.max_offset().y > px(1000.), "the content must overflow the viewport");
        scroll.set_offset(point(px(0.), -scroll.max_offset().y));
        window.draw(point(px(0.), px(0.)), size(px(280.), px(240.)), draw);
        assert!(scroll.offset().y < px(-1000.), "the bottom must be reachable");
        scroll.set_offset(point(px(0.), px(0.)));
        window.draw(point(px(0.), px(0.)), size(px(280.), px(240.)), draw);
        assert_eq!(scroll.offset().y, px(0.), "the header must remain reachable");
    }
}
