use anyhow::{anyhow, bail};
use settings::AppSettings;
use chat::ChatRegistry;
use futures::{FutureExt, future::Either};
use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, ClipboardItem, Context, Entity, ParentElement, Render, SharedString, Styled,
    Subscription, Task, Window, div, px,
};
use nostr_sdk::prelude::*;
use state::{BOOTSTRAP_RELAYS, NostrRegistry};
use theme::ActiveTheme;
use ui::button::{Button, ButtonVariants};
use ui::input::{Input, InputEvent, InputState};
use ui::scroll::ScrollableElement;
use ui::notification::Notification;
use ui::{Disableable, Icon, IconName, Sizable, WindowExtension, h_flex, v_flex};

const REASONS: [(Report, &str, &str); 7] = [
    (Report::Spam, "Spam or scam", "Unsolicited messages, scams, or fraud"),
    (
        Report::Impersonation,
        "Impersonation",
        "Pretending to be someone else",
    ),
    (Report::Malware, "Malware", "Malicious files or software"),
    (Report::Nudity, "Nudity", "Sexual or explicit imagery"),
    (
        Report::Profanity,
        "Profanity",
        "Offensive language or hateful speech",
    ),
    (Report::Illegal, "Illegal", "Allegedly illegal content"),
    (
        Report::Other,
        "Other",
        "Another reason, such as a scam or fraud",
    ),
];

fn reason_icon(reason: &Report) -> IconName {
    match reason {
        Report::Spam => IconName::MailX,
        Report::Impersonation => IconName::Mask,
        Report::Malware => IconName::Bug,
        Report::Nudity => IconName::EyeOff,
        Report::Profanity => IconName::MessageWarning,
        Report::Illegal => IconName::Gavel,
        Report::Other => IconName::Flag,
    }
}

fn report_builder(
    target: PublicKey,
    reason: Option<Report>,
    explanation: &str,
) -> anyhow::Result<EventBuilder> {
    let reason = reason.ok_or_else(|| anyhow!("Choose a reason before sending a report."))?;
    Ok(
        EventBuilder::new(Kind::Reporting, explanation.trim()).tag(Tag::from(
            Nip56Tag::PublicKey {
                public_key: target,
                report: reason,
            },
        )),
    )
}

fn delivery_result(
    accepted: usize,
    total: usize,
    errors: impl IntoIterator<Item = String>,
) -> anyhow::Result<String> {
    if accepted == 0 {
        let errors = errors.into_iter().collect::<Vec<_>>().join("\n");
        bail!(
            "No relay confirmed receipt of your report.{}",
            if errors.is_empty() {
                String::new()
            } else {
                format!("\n{errors}")
            }
        );
    }
    if accepted < total {
        Ok(format!(
            "Report accepted by {accepted} of {total} relays. The remaining relays did not confirm receipt."
        ))
    } else {
        Ok("Public report accepted by relays.".into())
    }
}

async fn publish(client: Client, event: Event) -> anyhow::Result<String> {
    let relays = BOOTSTRAP_RELAYS
        .iter()
        .copied()
        .map(RelayUrl::parse)
        .collect::<Result<Vec<_>, _>>()?;
    publish_to(&client, &event, &relays).await
}

async fn publish_to(client: &Client, event: &Event, relays: &[RelayUrl]) -> anyhow::Result<String> {
    let output = client
        .send_event(event)
        .to(relays)
        .ack_policy(AckPolicy::all())
        .await?;
    delivery_result(
        output
            .success
            .values()
            .filter(|status| status.is_ack())
            .count(),
        relays.len(),
        output
            .failed
            .into_iter()
            .map(|(url, reason)| format!("{url}: {reason}")),
    )
}

// A report failure must never trigger blocking. A block failure must never
// turn a published report into a failed report that the user might resubmit.
fn finish_report(
    result: anyhow::Result<String>,
    auto_block: bool,
    same_account: bool,
    block: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<String> {
    let mut message = result?;
    if auto_block {
        if !same_account {
            message.push_str(" Automatic blocking was skipped because your account changed. Return to the reporting account to block this user.");
        } else {
            match block() {
                Ok(()) => message.push_str(" This user is blocked locally. Block-list sync status is available in Blocked users."),
                Err(error) => message.push_str(&format!(" The report was sent, but automatic blocking failed: {error}. Use Block on their profile to retry blocking.")),
            }
        }
    }
    Ok(message)
}

pub fn open(target: PublicKey, name: SharedString, window: &mut Window, cx: &mut App) {
    let form = cx.new(|cx| ReportForm::new(target, name, window, cx));
    window.open_modal(cx, move |modal, window, cx| {
        let busy = form.read(cx).sending;
        modal
            .title("Report user")
            .width(px(480.).min(window.viewport_size().width - px(32.)))
            .margin_top(px(16.))
            .show_close(!busy)
            .keyboard(!busy)
            .overlay_closable(false)
            .child(form.clone())
    });
}

struct ReportForm {
    target: PublicKey,
    name: SharedString,
    owner: Option<PublicKey>,
    reason: Option<Report>,
    explanation: Entity<InputState>,
    explanation_expanded: bool,
    signed: Option<Event>,
    sending: bool,
    submitted_auto_block: bool,
    error: Option<String>,
    success: Option<String>,
    task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl ReportForm {
    fn new(
        target: PublicKey,
        name: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let explanation = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Add an explanation…")
                .auto_grow(1, 3)
        });
        let mut subscriptions = vec![cx.subscribe(&explanation, |this: &mut Self, _, event, cx| {
            if matches!(event, InputEvent::Change) && !this.sending {
                this.signed = None;
                this.error = None;
                cx.notify();
            }
        })];
        subscriptions.push(cx.observe(&AppSettings::global(cx), |_, _, cx| cx.notify()));
        Self {
            target,
            name,
            owner: NostrRegistry::global(cx).read(cx).current_user(),
            reason: None,
            explanation,
            explanation_expanded: false,
            signed: None,
            sending: false,
            submitted_auto_block: false,
            error: None,
            success: None,
            task: None,
            _subscriptions: subscriptions,
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sending || self.success.is_some() {
            return;
        }
        let nostr = NostrRegistry::global(cx);
        let Some(owner) = self
            .owner
            .filter(|owner| nostr.read(cx).current_user() == Some(*owner))
        else {
            self.error = Some(
                "Your account changed or disconnected. Close this dialog and try again.".into(),
            );
            cx.notify();
            return;
        };
        let builder = match report_builder(
            self.target,
            self.reason.clone(),
            &self.explanation.read(cx).value(),
        ) {
            Ok(builder) => builder,
            Err(error) => {
                self.error = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        let signer = nostr.read(cx).signer().snapshot();
        let client = nostr.read(cx).client();
        let cached = self.signed.clone();
        let auto_block = AppSettings::get_auto_block_reports(cx);
        self.submitted_auto_block = auto_block;
        self.sending = true;
        self.error = None;
        cx.notify();
        let signing = cx.background_spawn(async move {
            if let Some(event) = cached {
                return Ok(event);
            }
            let event = builder.finalize_async(&signer).await?;
            if event.pubkey != owner {
                bail!("Signer returned a different account. No report was published.");
            }
            event.verify()?;
            anyhow::Ok(event)
        });
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result: anyhow::Result<String> = async {
                let timeout = cx.background_executor().timer(std::time::Duration::from_secs(30));
                let event = match futures::future::select(signing.boxed(), timeout.boxed()).await {
                    Either::Left((result, _)) => result?,
                    Either::Right(_) => bail!("Your signer did not respond in time. Open it, check pending requests, and retry."),
                };
                this.update(cx, |this, cx| {
                    if NostrRegistry::global(cx).read(cx).current_user() != Some(owner) {
                        bail!("Your account changed. No report was published. Close this dialog and try again.");
                    }
                    this.signed = Some(event.clone());
                    Ok(())
                })??;
                let sending = cx.background_spawn(publish(client, event));
                let timeout = cx.background_executor().timer(std::time::Duration::from_secs(45));
                match futures::future::select(sending.boxed(), timeout.boxed()).await {
                    Either::Left((result, _)) => result,
                    Either::Right(_) => Err(anyhow!("Report delivery timed out. Receipt is unconfirmed; retrying will reuse the same report.")),
                }
            }.await;
            this.update_in(cx, |this, window, cx| {
                this.sending = false;
                let same_account = NostrRegistry::global(cx).read(cx).current_user() == Some(owner);
                let mut block_warning = auto_block && !same_account;
                let result = finish_report(result, auto_block, same_account, || {
                    let result = ChatRegistry::global(cx).update(cx, |chat, cx| {
                        if chat.is_blocked(this.target) { Ok(()) } else { chat.block_user(this.target, true, cx) }
                    });
                    block_warning = result.is_err();
                    result
                });
                match result {
                    Ok(message) => {
                        this.success = Some(message.clone());
                        let notice = if block_warning {
                            Notification::warning(message).autohide(false)
                        } else {
                            Notification::success(message)
                        };
                        window.close_modal(cx);
                        window.push_notification(notice, cx);
                    },
                    Err(error) => this.error = Some(error.to_string()),
                }
                cx.notify();
            }).ok();

        }));
    }
}

impl Render for ReportForm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let auto_block = if self.sending { self.submitted_auto_block } else { AppSettings::get_auto_block_reports(cx) };
        v_flex().gap_3().text_sm()
            .max_h((window.viewport_size().height - px(112.)).max(px(80.))).overflow_y_scrollbar()
            .child(format!("Report {}", self.name))
            .child(div().text_xs().text_color(cx.theme().text_muted).child(self.target.to_bech32().unwrap()))
            .child("This publishes a report signed by your account. The selected reason and explanation are public.")
            .child(v_flex().w_full().gap_1().flex_shrink_0().children(REASONS.iter().map(|(reason, label, description)| {
                let selected = self.reason.as_ref() == Some(reason);
                let reason = reason.clone();
                Button::new(*label)
                    .ghost().align_left().w_full().h_8().px_3().flex_shrink_0()
                    .when(selected, |button| button.primary())
                    .child(h_flex().w_full().min_w_0().gap_2()
                        .child(Icon::new(reason_icon(&reason)).small().flex_shrink_0())
                        .child(div().flex_1().min_w_0().truncate().child(*label))
                        .child(div().size_4().flex_shrink_0()
                            .when(selected, |view| view.child(Icon::new(IconName::Check).small()))))
                    .tooltip(*description).disabled(self.sending)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.reason.as_ref() != Some(&reason) { this.signed = None; }
                        this.reason = Some(reason.clone()); this.error = None; cx.notify();
                    }))
            })))
            .child(Button::new("toggle-report-explanation")
                .label(if self.explanation_expanded { "Hide explanation" } else if self.explanation.read(cx).value().is_empty() { "Add an explanation (optional)" } else { "Edit explanation" })
                .icon(if self.explanation_expanded { ui::IconName::ChevronDown } else { ui::IconName::Plus })
                .align_left().small().ghost().disabled(self.sending)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.explanation_expanded = !this.explanation_expanded;
                    if this.explanation_expanded {
                        this.explanation.update(cx, |input, cx| input.focus(window, cx));
                    }
                    cx.notify();
                })))
            .when(self.explanation_expanded, |view| view
                .child(Input::new(&self.explanation).disabled(self.sending)))
            .when_some(self.error.clone(), |view, error| view.child(v_flex().gap_2()
                .child(div().text_color(cx.theme().text_warning).child(error.clone()))
                .child(Button::new("copy-report-error").label("Copy error").small().ghost()
                    .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(error.clone()))))))
            .when(self.sending, |view| view.child("Sending report… Your signer may ask for approval."))
            .child(h_flex().gap_2().justify_end()
                .child(Button::new("cancel-report").label("Cancel").ghost().disabled(self.sending)
                    .on_click(|_, window, cx| window.close_modal(cx)))
                .child(Button::new("send-report").label(if auto_block { "Block & Report" } else { "Send Report" })
                    .primary().loading(self.sending).disabled(self.reason.is_none() || self.sending)
                    .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx)))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr_sdk::local_relay::{LocalRelay, WritePolicy, WritePolicyResult};

    #[test]
    fn auto_block_requires_report_success_preference_and_same_account() {
        let never = || -> anyhow::Result<()> { panic!("must not block") };
        assert!(finish_report(Err(anyhow!("relay rejected")), true, true, never).is_err());
        assert_eq!(finish_report(Ok("Accepted".into()), false, true, never).unwrap(), "Accepted");
        assert!(finish_report(Ok("Accepted".into()), true, false, never).unwrap().contains("account changed"));
        let mut calls = 0;
        let message = finish_report(Ok("Accepted by 1 of 2 relays".into()), true, true, || { calls += 1; Ok(()) }).unwrap();
        assert_eq!(calls, 1);
        assert!(message.contains("blocked locally"));
        let message = finish_report(Ok("Accepted".into()), true, true, || Err(anyhow!("disk full"))).unwrap();
        assert!(message.contains("report was sent"));
        assert!(message.contains("disk full"));
    }

    #[derive(Debug)]
    struct RejectReports;
    impl WritePolicy for RejectReports {
        fn admit_event<'a>(
            &'a self,
            _: &'a Event,
            _: &'a std::net::SocketAddr,
        ) -> futures::future::BoxFuture<'a, WritePolicyResult> {
            Box::pin(async {
                WritePolicyResult::reject(
                    MachineReadablePrefix::Blocked,
                    "Reports disabled on this relay",
                )
            })
        }
    }

    #[tokio::test]
    async fn real_relay_rejections_fail_and_partial_acceptance_is_reported() {
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            let good = LocalRelay::builder().build();
            let bad = LocalRelay::builder().write_policy(RejectReports).build();
            good.run().await.unwrap();
            bad.run().await.unwrap();
            let good_url = good.url().await;
            let bad_url = bad.url().await;
            let client = Client::default();
            client.add_relay(&good_url).and_connect().await.unwrap();
            client.add_relay(&bad_url).and_connect().await.unwrap();
            let owner = Keys::generate();
            let event = report_builder(
                Keys::generate().public_key(),
                Some(Report::Spam),
                "Unsolicited messages",
            )
            .unwrap()
            .finalize(&owner)
            .unwrap();
            assert!(
                publish_to(&client, &event, &[bad_url.clone()])
                    .await
                    .is_err()
            );
            let result = publish_to(&client, &event, &[good_url.clone(), bad_url])
                .await
                .unwrap();
            assert!(result.contains("1 of 2"), "{result}");
            // Retrying the exact signed event also receives a valid acknowledgement.
            assert!(publish_to(&client, &event, &[good_url]).await.is_ok());
            client.shutdown().await;
        })
        .await
        .expect("local relay acknowledgements should finish promptly");
    }

    #[test]
    fn reason_is_required_and_each_choice_uses_its_nip56_tag() {
        let owner = Keys::generate().public_key();
        let target = Keys::generate().public_key();
        assert!(report_builder(target, None, "").is_err());
        for (reason, _, _) in REASONS {
            let expected = reason.as_str().to_owned();
            let event = report_builder(target, Some(reason), "  Evidence for this report.  ")
                .unwrap()
                .finalize_unsigned(owner);
            assert_eq!(event.kind, Kind::Reporting);
            assert_eq!(event.content, "Evidence for this report.");
            assert_eq!(
                event.tags.iter().next().unwrap().as_slice(),
                &["p".to_owned(), target.to_hex(), expected]
            );
        }
    }
    #[test]
    fn missing_acknowledgements_fail_and_partial_acceptance_is_explicit() {
        assert!(
            delivery_result(0, 2, [])
                .unwrap_err()
                .to_string()
                .contains("No relay confirmed")
        );
        let error = delivery_result(0, 2, ["wss://example.com: rejected".into()])
            .unwrap_err()
            .to_string();
        assert!(error.contains("rejected"));
        assert!(delivery_result(1, 2, []).unwrap().contains("1 of 2"));
        assert!(delivery_result(2, 2, []).is_ok());
    }
}
