//! Read-only diagnostics with explicit, narrowly scoped recovery actions.
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use chat::ChatRegistry;
use futures::{FutureExt, StreamExt, stream::BoxStream};
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, ClipboardItem, Context, Entity, EventEmitter, FocusHandle,
    Focusable, IntoElement, ParentElement, Render, SharedString, Styled, Subscription, Task,
    Window,
};
use nostr_sdk::prelude::*;
use state::{NostrRegistry, USER_GIFTWRAP};
use theme::ActiveTheme;
use ui::button::{Button, ButtonVariants};
use ui::dock::{Panel, PanelEvent};
use ui::scroll::ScrollableElement;
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
) -> anyhow::Result<Vec<RelayState>> {
    let events = client
        .database()
        .query(Filter::new().kind(Kind::InboxRelays).author(owner).limit(1))
        .await?;
    let urls: BTreeSet<RelayUrl> = events
        .into_iter()
        .next()
        .map(|event| nip17::extract_relay_list(&event).collect())
        .unwrap_or_default();
    watches.retain(|url, _| urls.contains(url));
    let mut result = Vec::new();
    for url in urls {
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
    Ok(result)
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
) -> Option<String> {
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
            .any(|r| r.auth == Some("Authentication failed") || r.closed.is_some())
        {
            "Messaging relay needs attention"
        } else if relays
            .iter()
            .any(|r| r.auth == Some("Waiting for signer authentication"))
        {
            "Waiting for relay authentication…"
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
            return None;
        }
    } else {
        "Checking messaging relays…"
    };
    Some(message.into())
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
                    this.error = None;
                }
                cx.notify();
            }),
        );
        let task = cx.spawn(async move |this, cx| {
            let mut sampled_owner = None;
            let mut watches = BTreeMap::new();
            loop {
                let (owner, client) = this.read_with(cx, |this, cx| {
                    (this.owner, NostrRegistry::global(cx).read(cx).client())
                })?;
                if sampled_owner != owner {
                    watches.clear();
                    sampled_owner = owner;
                }
                if let Some(owner) = owner {
                    let (next_watches, result) = cx
                        .background_spawn(async move {
                            let result = sample(&client, owner, &mut watches).await;
                            (watches, result)
                        })
                        .await;
                    watches = next_watches;
                    this.update(cx, |this, cx| {
                        if this.owner != Some(owner) {
                            return;
                        }
                        match result {
                            Ok(relays) => {
                                this.relays = Some(relays);
                                this.error = None;
                            }
                            Err(error) => {
                                this.relays = None;
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
            owner,
            relays: None,
            error: None,
            focus: cx.focus_handle(),
            _task: task,
            _subscriptions: subscriptions,
        }
    })
}

pub struct ConnectionStatus {
    owner: Option<PublicKey>,
    relays: Option<Vec<RelayState>>,
    error: Option<String>,
    focus: FocusHandle,
    _task: Task<anyhow::Result<()>>,
    _subscriptions: Vec<Subscription>,
}

impl ConnectionStatus {
    pub fn summary(&self, cx: &App) -> String {
        let nostr = NostrRegistry::global(cx);
        let nostr = nostr.read(cx);
        if let Some(summary) = connection_summary(
            nostr.identity_loading(),
            nostr.signer_connection_error().is_some(),
            self.owner.is_some(),
            self.relays.as_deref(),
            self.error.is_some(),
        ) {
            return summary;
        }
        let relays = self
            .relays
            .as_ref()
            .expect("connection summary covers unchecked relays");
        let connected = relays
            .iter()
            .filter(|r| r.status == Some(RelayStatus::Connected))
            .count();
        if connected == 0 {
            return "Messaging relays disconnected".into();
        }
        let chat = ChatRegistry::global(cx);
        let chat = chat.read(cx);
        let delivery = Delivery::collect(chat.delivery_reports());
        if chat.outgoing_error().is_some() {
            return "Outgoing queue needs attention".into();
        }
        if delivery.paused > 0 {
            return format!("{} sends paused", delivery.paused);
        }
        if delivery.failed > 0 {
            return format!("{} sends delayed", delivery.failed);
        }
        if connected < relays.len() {
            return format!("{connected}/{} messaging relays connected", relays.len());
        }
        if chat.history_error().is_some()
            || chat.history_relays().values().any(|r| r.error.is_some())
        {
            return "History incomplete".into();
        }
        if chat.history_running() {
            return "Downloading message history…".into();
        }
        if chat.pending_messages() > 0 {
            return format!("Decrypting {} messages…", chat.pending_messages());
        }
        if chat.count_trash_messages(cx) > 0 {
            return "Some messages could not be decrypted".into();
        }
        if delivery.preparing > 0 {
            return format!("Preparing {} messages for signing…", delivery.preparing);
        }
        if delivery.pending > 0 {
            return format!("Sending {} messages…", delivery.pending);
        }
        if chat.loading() {
            return "Loading conversation list…".into();
        }
        "Messaging relays connected".into()
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
        for (reason, count) in chat.decryption_failures(cx) {
            sections.push(("Decryption".into(), format!("{count} messages: {reason}")));
        }
        let delivery = Delivery::collect(chat.delivery_reports());
        sections.push(("Outgoing messages".into(), format!("{} pending · {} paused · {} delayed\nDelayed sends retry automatically; paused sends wait for your explicit retry. Counts include your own encrypted copy. Relay acceptance is not a read receipt.", delivery.pending, delivery.paused, delivery.failed)));
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
    fn title(&self, _: &App) -> AnyElement {
        "Connection status".into_any_element()
    }
}
impl EventEmitter<PanelEvent> for ConnectionStatus {}
impl Focusable for ConnectionStatus {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for ConnectionStatus {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let details = self.details(cx);
        let copy = details
            .iter()
            .map(|(heading, detail)| format!("{heading}\n{detail}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        let relay_urls: BTreeSet<String> = self.relays.as_ref().into_iter().flatten()
            .map(|relay| relay.url.to_string()).collect();
        let mut details = details.into_iter();
        let signer_detail = details.next().map(|(_, text)| text).unwrap_or_default();
        let details: Vec<_> = details.filter(|(heading, _)| !relay_urls.contains(heading)).collect();
        let signed_in = self.owner.is_some();
        let nostr = NostrRegistry::global(cx);
        let signer_needs_retry = nostr.read(cx).identity_loading()
            || nostr.read(cx).signer_connection_error().is_some()
            || !signed_in;
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
        v_flex().size_full().min_h_0().p_4().gap_4().overflow_y_scrollbar()
            .child(h_flex().gap_2().flex_wrap()
                .child(Button::new("copy-status").label("Copy status").small().ghost()
                    .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))))
                .child(Button::new("retry-status-signer").label("Reconnect signer").small().ghost().disabled(!signer_needs_retry)
                    .on_click(|_, _, cx| NostrRegistry::global(cx).update(cx, |nostr, cx| nostr.retry_signer(cx)))))
            .child(v_flex().gap_2()
                .child(h_flex().gap_2().child(Icon::new(IconName::UserKey).small()).child("Signer"))
                .child(gpui::div().text_sm().text_color(cx.theme().text_muted).child(signer_detail)))
            .when(signed_in && !relay_urls.is_empty(), |view| view.child(
                v_flex().gap_2()
                    .child(h_flex().gap_2().child(Icon::new(IconName::Relay).small()).child("Messaging relays"))
                    .children(self.relays.as_ref().into_iter().flatten().map(|relay| relay_card(relay, chat.history_relays().get(&relay.url).map(|progress| {
                        format!("{} messages received · {}", progress.received,
                            if progress.error.is_some() { "Scan incomplete" } else if progress.done { "History checked" } else { "Loading history…" })
                    }), cx)))
            ))
            .children(details.into_iter().map(|(heading, detail)| v_flex().gap_1().child(heading)
                .child(gpui::div().text_sm().text_color(cx.theme().text_muted).child(detail))))
            .child(h_flex().gap_2().flex_wrap()
                .child(Button::new("manage-status-relays").label("Manage messaging relays").small().ghost()
                    .on_click(|_, window, cx| {
                        let panel = super::messaging_relays::init(window, cx);
                        crate::Workspace::add_panel(panel, ui::dock::DockPlacement::Right, window, cx);
                    }))
                .child(Button::new("manage-status-gossip").label("Manage gossip relays").small().ghost()
                    .on_click(|_, window, cx| {
                        let panel = super::relay_list::init(window, cx);
                        crate::Workspace::add_panel(panel, ui::dock::DockPlacement::Right, window, cx);
                    }))
                .child(Button::new("retry-status-relays").label("Reconnect messaging relays").small().ghost().disabled(!signed_in || !relays_found)
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
            .child(h_flex().gap_2().flex_wrap()
                .child(Button::new("status-rescan").label("Resume history").small().ghost().disabled(!signed_in || history_running)
                    .on_click(|_, _, cx| ChatRegistry::global(cx).update(cx, |chat, cx| chat.resume_history(cx))))
                .child(Button::new("status-full-rescan").label("Rescan all history").small().ghost().disabled(!signed_in || history_running)
                    .on_click(|_, _, cx| ChatRegistry::global(cx).update(cx, |chat, cx| chat.load_older_history(cx))))
                .child(Button::new("status-broaden").label("Search other relays").small().ghost().disabled(!signed_in || history_running)
                    .on_click(|_, _, cx| ChatRegistry::global(cx).update(cx, |chat, cx| chat.search_other_relays(cx))))
                .child(Button::new("status-decrypt").label("Retry decryption").small().ghost().disabled(!signed_in || !decrypt_failed)
                    .on_click(|_, _, cx| ChatRegistry::global(cx).update(cx, |chat, cx| chat.retry_failed_messages(cx))))
                .child(Button::new("status-send").label("Retry pending sends").small().ghost().disabled(!signed_in || !send_pending)
                    .on_click(|_, _, cx| ChatRegistry::global(cx).read(cx).retry_outgoing())))
            .child(gpui::div().text_xs().text_color(cx.theme().text_muted)
                .child("Retrying decryption or sends also resumes requests you previously declined. Your signer may ask for approval again."))
    }
}

fn relay_card(relay: &RelayState, history: Option<String>, cx: &App) -> impl IntoElement {
    let (icon, label, color) = match relay.status {
        Some(RelayStatus::Connected) => (IconName::CheckCircle, "Connected", cx.theme().text_accent),
        Some(RelayStatus::Initialized | RelayStatus::Pending | RelayStatus::Connecting) =>
            (IconName::Loader, "Connecting", cx.theme().text_warning),
        Some(RelayStatus::Sleeping) => (IconName::Moon, "Sleeping", cx.theme().text_muted),
        Some(RelayStatus::Banned) => (IconName::Block, "Disabled", cx.theme().text_danger),
        Some(RelayStatus::Shutdown) => (IconName::CloseCircle, "Shut down", cx.theme().text_muted),
        _ => (IconName::CloseCircle, "Disconnected", cx.theme().text_warning),
    };
    let auth = match relay.auth {
        Some("Authenticated") => Some((IconName::Shield, "Authenticated", cx.theme().text_accent)),
        Some("Authentication failed") => Some((IconName::Warning, "Auth failed", cx.theme().text_danger)),
        Some("Waiting for signer authentication") => Some((IconName::UserKey, "Awaiting signer", cx.theme().text_warning)),
        _ => None,
    };
    let badge = |icon: IconName, label: &'static str, color| h_flex()
        .gap_1().px_2().py_1().rounded_full().text_xs().text_color(color)
        .bg(cx.theme().surface_background)
        .child(Icon::new(icon).xsmall()).child(label);
    v_flex().w_full().min_w_0().flex_shrink_0().p_3().gap_2()
        .rounded(cx.theme().radius).bg(cx.theme().elevated_surface_background)
        .child(gpui::div().min_w_0().truncate().text_sm()
            .child(relay.url.as_str().trim_start_matches("wss://").trim_start_matches("ws://").trim_end_matches('/').to_owned()))
        .child(h_flex().gap_2().flex_wrap()
            .child(badge(icon, label, color))
            .when_some(auth, |row, (icon, label, color)| row.child(badge(icon, label, color))))
        .when_some(history, |card, progress| card.child(
            gpui::div().text_xs().text_color(cx.theme().text_muted).child(progress)))
        .when_some(relay.closed.clone(), |card, reason| card.child(
            h_flex().items_start().gap_2().text_sm().text_color(cx.theme().text_danger)
                .child(Icon::new(IconName::Warning).small())
                .child(gpui::div().min_w_0().child(reason))))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signer_and_account_state_take_priority_over_old_relay_data() {
        let old = [RelayState {
            url: RelayUrl::parse("wss://example.com").unwrap(),
            status: Some(RelayStatus::Connected),
            auth: Some("Authentication failed"),
            closed: None,
        }];
        assert_eq!(
            connection_summary(false, false, false, Some(&old), false).as_deref(),
            Some("Connect your signer")
        );
        assert_eq!(
            connection_summary(true, false, true, Some(&old), false).as_deref(),
            Some("Connecting to signer…")
        );
        assert_eq!(
            connection_summary(true, true, true, Some(&old), false).as_deref(),
            Some("Signer unavailable")
        );
    }

    #[test]
    fn unchecked_missing_and_disconnected_relays_are_distinct() {
        assert_eq!(
            connection_summary(false, false, true, None, false).as_deref(),
            Some("Checking messaging relays…")
        );
        assert_eq!(
            connection_summary(false, false, true, Some(&[]), false).as_deref(),
            Some("No messaging relays found")
        );
        let mut relay = RelayState {
            url: RelayUrl::parse("wss://example.com").unwrap(),
            status: Some(RelayStatus::Disconnected),
            auth: None,
            closed: None,
        };
        assert_eq!(
            connection_summary(false, false, true, Some(&[relay.clone()]), false).as_deref(),
            Some("Messaging relays disconnected")
        );
        relay.status = Some(RelayStatus::Connected);
        relay.closed = Some("auth-required: authenticate first".into());
        assert_eq!(
            connection_summary(false, false, true, Some(&[relay]), false).as_deref(),
            Some("Messaging relay needs attention")
        );
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
