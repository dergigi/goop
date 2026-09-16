use chat::ChatRegistry;
use common::crash_report::{self, CrashReport};
use gpui::{
    App, AppContext, Context, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    Styled, Subscription, Task, Window, div, px,
};
use nostr_sdk::prelude::*;
use state::NostrRegistry;
use theme::ActiveTheme;
use ui::button::{Button, ButtonVariants};
use ui::notification::Notification;
use ui::scroll::ScrollableElement;
use ui::{Disableable, Sizable, WindowExtension, h_flex, v_flex};

pub fn offer(window: &mut Window, cx: &mut App) {
    let reports = crash_report::pending();
    if reports.is_empty() {
        return;
    }
    window.push_notification(
        Notification::new()
            .title("Goop encountered an error")
            .message("A local crash report is available. Send it to Goop?")
            .autohide(false)
            .action(move |_, _, cx| {
                let reports = reports.clone();
                Button::new("review-crash-report")
                    .label("Review report")
                    .small()
                    .on_click(cx.listener(move |notice, _, window, cx| {
                        notice.dismiss(window, cx);
                        open(reports.clone(), window, cx);
                    }))
            }),
        cx,
    );
}

fn open(reports: Vec<CrashReport>, window: &mut Window, cx: &mut App) {
    let form = cx.new(|cx| {
        let subscriptions = vec![
            cx.observe(&NostrRegistry::global(cx), |_, _, cx| cx.notify()),
            cx.observe(&ChatRegistry::global(cx), |_, _, cx| cx.notify()),
        ];
        let text = reports
            .iter()
            .map(|r| r.text.as_str())
            .collect::<Vec<_>>()
            .join("\n---\n\n");
        CrashForm {
            reports,
            text,
            sending: false,
            error: None,
            task: None,
            _subscriptions: subscriptions,
        }
    });
    window.open_modal(cx, move |modal, window, cx| {
        let busy = form.read(cx).sending;
        modal
            .title("Crash report")
            .width(px(520.).min(window.viewport_size().width - px(32.)))
            .margin_top(px(16.))
            .show_close(!busy)
            .keyboard(!busy)
            .overlay_closable(false)
            .child(form.clone())
    });
}

struct CrashForm {
    reports: Vec<CrashReport>,
    text: String,
    sending: bool,
    error: Option<String>,
    task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}
impl CrashForm {
    fn dismiss(&self) -> bool {
        self.reports.iter().all(|report| report.dismiss().is_ok())
    }
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sending {
            return;
        }
        let Some(owner) = NostrRegistry::global(cx).read(cx).current_user() else {
            return;
        };
        let target = PublicKey::parse(state::GOOP_NPUB).expect("valid Goop public key");
        let candidate = cx.new(|_| {
            chat::Room::new(owner, [target])
                .organize(&owner)
                .kind(chat::RoomKind::Ongoing)
        });
        let registry = ChatRegistry::global(cx);
        let room = registry
            .read(cx)
            .room(&candidate.read(cx).id, cx)
            .and_then(|r| r.upgrade())
            .unwrap_or(candidate);
        let task = room
            .read(cx)
            .rumor(self.text.clone(), [], false, cx)
            .and_then(|rumor| room.read(cx).send(rumor, cx));
        let Some(task) = task else {
            self.error = Some("Messaging is still connecting. Try again in a moment.".into());
            cx.notify();
            return;
        };
        self.sending = true;
        self.error = None;
        cx.notify();
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                this.sending = false;
                match result {
                    Ok(_) => {
                        let removed = this.dismiss();
                        window.close_modal(cx);
                        window.push_notification(
                            Notification::success(if removed {
                                "Crash report queued for delivery to Goop."
                            } else {
                                "Crash report queued. The local report could not be removed."
                            }),
                            cx,
                        );
                        if NostrRegistry::global(cx).read(cx).current_user() == Some(owner) {
                            registry
                                .update(cx, |registry, cx| registry.emit_room(&room, window, cx));
                        }
                    }
                    Err(_) => {
                        this.error = Some(
                            "Could not queue the report. It is saved locally; you can try again."
                                .into(),
                        );
                        cx.notify();
                    }
                }
            })
            .ok();
        }));
    }
}
impl Render for CrashForm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let connected = NostrRegistry::global(cx).read(cx).current_user().is_some();
        let mut view = v_flex().gap_3().w_full()
            .child("Send these diagnostics to Goop in an encrypted DM?")
            .child(div().text_sm().text_color(cx.theme().text_muted)
                .child("Only the report below is sent. No message contents, credentials, or application logs."))
            .child(div().id("crash-report-preview").max_h((window.viewport_size().height - px(280.)).max(px(64.)).min(px(280.)))
                .overflow_y_scrollbar().p_3().rounded_md().bg(cx.theme().elevated_surface_background)
                .text_sm().child(SharedString::from(self.text.clone())));
        if !connected {
            view = view.child(
                div()
                    .text_sm()
                    .child("Connect your signer to send the report."),
            );
        }
        if let Some(error) = &self.error {
            view = view.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().danger_foreground)
                    .child(error.clone()),
            );
        }
        view.child(
            h_flex()
                .justify_end()
                .gap_2()
                .child(
                    Button::new("dismiss-crash-report")
                        .label("Dismiss")
                        .ghost()
                        .disabled(self.sending)
                        .on_click(cx.listener(|this, _, window, cx| {
                            if this.dismiss() {
                                window.close_modal(cx);
                            } else {
                                this.error = Some("Could not remove the local report.".into());
                                cx.notify();
                            }
                        })),
                )
                .child(
                    Button::new("send-crash-report")
                        .label("Send to Goop")
                        .primary()
                        .loading(self.sending)
                        .disabled(self.sending || !connected)
                        .on_click(cx.listener(|this, _, window, cx| this.send(window, cx))),
                ),
        )
    }
}
