use super::*;
use speech::{Event, Session};

#[derive(Default, PartialEq)]
enum Phase {
    #[default]
    Idle,
    Setup,
    Downloading,
    Preparing,
    Recording,
    Processing,
    Failed,
}
#[derive(Default)]
pub(super) struct Dictation {
    phase: Phase,
    session: Option<Session>,
    task: Option<Task<()>>,
    owner: Option<PublicKey>,
    seconds: u64,
    level: f32,
    downloaded: u64,
    download: bool,
    error: String,
    retryable: bool,
    send_guard: Option<ComposerSnapshot>,
}
type ComposerSnapshot = (drafts::Snapshot, Vec<String>);

impl Dictation {
    pub(super) fn active(&self) -> bool {
        self.phase != Phase::Idle
    }
    pub(super) fn recording(&self) -> bool {
        self.phase == Phase::Recording
    }
    fn accepts_result(&self, owner: Option<PublicKey>) -> bool {
        self.active() && self.owner.is_some() && self.owner == owner
    }
    fn should_send(&self, current: &ComposerSnapshot, focused: bool) -> bool {
        focused && self.send_guard.as_ref() == Some(current)
    }
}
fn model_root() -> PathBuf {
    common::support_dir().join("speech")
}

/// Append to the current draft, including edits made while recognition ran.
fn append_transcript(current: &str, transcript: &str) -> String {
    let separator = if current.is_empty() || current.ends_with(char::is_whitespace) {
        ""
    } else {
        " "
    };
    format!("{current}{separator}{}", transcript.trim())
}
impl ChatPanel {
    pub(super) fn escape_chat(&mut self, action: &ui::input::Escape, window: &mut Window, cx: &mut Context<Self>) {
        if self.dictation.recording() && self.input.read(cx).focus_handle(cx).is_focused(window) {
            self.finish_dictation(false, cx);
        } else {
            self.escape_find(action, window, cx);
        }
    }
    pub fn toggle_dictation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dictation.recording() {
            if self.uploading { return; }
            self.focus_composer(window, cx);
            self.finish_dictation(true, cx);
        } else if !self.dictation.active() && !self.uploading {
            let Some(room) = self.room.upgrade() else { return };
            let registry = ChatRegistry::global(cx).read(cx);
            if registry.has_left(room.read(cx)) || registry.room_blocked(room.read(cx)) {
                return;
            }
            self.focus_composer(window, cx);
            self.start_dictation(false, window, cx);
        }
    }
    fn dictation_guard(&self, cx: &App) -> ComposerSnapshot {
        (
            self.draft_snapshot(cx),
            self.attachments
                .read(cx)
                .iter()
                .map(|a| a.file.url.to_string())
                .collect(),
        )
    }
    fn start_dictation(&mut self, download: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !download && !speech::model::ready(&model_root()) {
            self.dictation.phase = Phase::Setup;
            cx.notify();
            return;
        }
        let Some(owner) = NostrRegistry::global(cx).read(cx).displayed_user() else {
            return;
        };
        let (session, events) = match Session::start(model_root(), download) {
            Ok(result) => result,
            Err(error) => {
                window.push_notification(Notification::error(error.to_string()), cx);
                return;
            }
        };
        self.dictation = Dictation {
            session: Some(session),
            owner: Some(owner),
            download,
            phase: if download {
                Phase::Downloading
            } else {
                Phase::Preparing
            },
            ..Default::default()
        };
        self.focus_composer(window, cx);
        self.dictation.task = Some(cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = events.recv_async().await {
                let terminal = matches!(
                    event,
                    Event::Ready
                        | Event::Transcript(_)
                        | Event::Failed {
                            retryable: false,
                            ..
                        }
                );
                if this
                    .update_in(cx, |this, window, cx| {
                        this.dictation_event(event, window, cx)
                    })
                    .is_err()
                {
                    break;
                }
                if terminal {
                    break;
                }
            }
        }));
        cx.notify();
    }
    fn dictation_event(&mut self, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        if !self
            .dictation
            .accepts_result(NostrRegistry::global(cx).read(cx).displayed_user())
        {
            self.dictation = Dictation::default();
            cx.notify();
            return;
        }
        match event {
            Event::Notice(message) => {
                self.dictation.send_guard = None;
                window.push_notification(message, cx);
            }
            Event::Download(bytes) => self.dictation.downloaded = bytes,
            Event::Ready => {
                self.dictation = Dictation::default();
                window.push_notification("Dictation is ready. Press the microphone to start", cx);
            }
            Event::Preparing => self.dictation.phase = Phase::Preparing,
            Event::Recording { seconds, level } => {
                // A final capture update can arrive after Finish was clicked.
                if self.dictation.phase != Phase::Processing {
                    self.dictation.phase = Phase::Recording;
                }
                self.dictation.seconds = seconds;
                self.dictation.level = level;
            }
            Event::Processing => self.dictation.phase = Phase::Processing,
            Event::Failed { message, retryable } => {
                self.dictation.phase = Phase::Failed;
                self.dictation.error = message;
                self.dictation.retryable = retryable;
            }
            Event::Transcript(text) => {
                let focused = window.is_window_active()
                    && self.input.read(cx).focus_handle(cx).is_focused(window);
                let send = self
                    .dictation
                    .should_send(&self.dictation_guard(cx), focused);
                let value = append_transcript(&self.input.read(cx).value(), &text);
                self.dictation = Dictation::default();
                self.input
                    .update(cx, |input, cx| input.set_value(value, window, cx));
                self.persist_draft(window, cx);
                if send {
                    self.send_text_message(window, cx);
                }
            }
        }
        cx.notify();
    }
    pub(super) fn finish_dictation(&mut self, send: bool, cx: &mut Context<Self>) {
        self.dictation.send_guard = send.then(|| self.dictation_guard(cx));
        if let Some(session) = &self.dictation.session {
            session.finish();
        }
        self.dictation.phase = Phase::Processing;
        cx.notify();
    }
    pub(super) fn render_dictation(&self, disabled: bool, cx: &Context<Self>) -> AnyElement {
        let d = &self.dictation;
        if !d.active() {
            return Button::new("dictation")
                .icon(IconName::Microphone)
                .tooltip(if cfg!(target_os = "macos") { "Dictate (⌘D)" } else { "Dictate (Ctrl+D)" })
                .ghost()
                .large()
                .disabled(disabled)
                .on_click(
                    cx.listener(|this, _, window, cx| this.start_dictation(false, window, cx)),
                )
                .into_any_element();
        }
        let cancel = Button::new("dictation-cancel")
            .icon(IconName::Close)
            .tooltip("Discard dictation")
            .ghost()
            .on_click(cx.listener(|this, _, _, cx| {
                this.dictation = Dictation::default();
                cx.notify();
            }));
        let mut row = h_flex()
            .w_full()
            .min_w_0()
            .gap_2()
            .px_2()
            .py_1()
            .rounded(cx.theme().radius)
            .bg(cx.theme().text_accent.opacity(0.14))
            .child(cancel);
        if d.phase == Phase::Setup {
            return row
                .child(div().flex_1().text_sm().child("Local dictation · 487 MB"))
                .child(
                    Button::new("dictation-download")
                        .icon(IconName::Download)
                        .label("Download")
                        .secondary()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.start_dictation(true, window, cx)
                        })),
                )
                .into_any_element();
        }
        if d.phase == Phase::Recording {
            row = row
                .child(h_flex().gap_1().items_center().children((0..12).map(|i| {
                    div()
                        .w(px(3.))
                        .h(px(6. + (i % 4) as f32 * 3.))
                        .rounded(px(2.))
                        .bg(cx
                            .theme()
                            .text_accent
                            .opacity(if i as f32 / 12. < d.level.sqrt() {
                                1.
                            } else {
                                0.18
                            }))
                })))
                .child(div().text_sm().child(format!(
                    "{:02}:{:02} / 5:00",
                    d.seconds / 60,
                    d.seconds % 60
                )))
                .child(div().flex_1())
                .child(
                    Button::new("dictation-insert")
                        .icon(IconName::Pencil)
                        .tooltip("Transcribe into draft (Esc)")
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| this.finish_dictation(false, cx))),
                )
                .child(
                    Button::new("dictation-send")
                        .icon(IconName::PaperPlaneFill)
                        .tooltip("Transcribe and send (Enter)")
                        .primary()
                        .disabled(disabled)
                        .on_click(cx.listener(|this, _, _, cx| this.finish_dictation(true, cx))),
                );
        } else if d.phase == Phase::Failed {
            row = row
                .child(div().flex_1().min_w_0().text_sm().child(d.error.clone()))
                .child(
                    Button::new("dictation-retry")
                        .icon(IconName::Refresh)
                        .tooltip("Retry dictation")
                        .ghost()
                        .on_click(cx.listener(|this, _, window, cx| {
                            if this.dictation.retryable {
                                if let Some(session) = &this.dictation.session {
                                    session.retry();
                                }
                                this.dictation.phase = Phase::Processing;
                                cx.notify();
                            } else {
                                this.start_dictation(this.dictation.download, window, cx);
                            }
                        })),
                );
        } else {
            let label = match d.phase {
                Phase::Downloading if d.downloaded == speech::model::DOWNLOAD_BYTES => {
                    "Installing dictation…".into()
                }
                Phase::Downloading => format!(
                    "Downloading local speech model (parakeet-v3)… {}%",
                    d.downloaded * 100 / speech::model::DOWNLOAD_BYTES
                ),
                Phase::Preparing => "Preparing dictation…".into(),
                _ => "Transcribing…".into(),
            };
            row = row
                .child(div().flex_1().text_sm().child(label))
                .child(Button::new("dictation-working").loading(true).ghost());
        }
        row.into_any_element()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transcription_preserves_edits_and_whitespace() {
        assert_eq!(append_transcript("", " Hello. "), "Hello.");
        assert_eq!(
            append_transcript("Typed while recording", "More."),
            "Typed while recording More."
        );
        assert_eq!(
            append_transcript("A thought\n", "Another."),
            "A thought\nAnother."
        );
    }

    #[test]
    fn late_results_require_same_account_and_explicit_unchanged_send_intent() {
        let owner = nostr_sdk::prelude::Keys::generate().public_key();
        let another = nostr_sdk::prelude::Keys::generate().public_key();
        let original = (
            drafts::Snapshot {
                text: "existing draft".into(),
                ..Default::default()
            },
            vec![],
        );
        let mut dictation = Dictation {
            phase: Phase::Processing,
            owner: Some(owner),
            ..Default::default()
        };
        assert!(dictation.accepts_result(Some(owner)));
        assert!(!dictation.accepts_result(Some(another)));
        assert!(!dictation.accepts_result(None));
        assert!(!dictation.should_send(&original, true)); // Pencil/default completion only inserts.
        dictation.send_guard = Some(original.clone());
        assert!(dictation.should_send(&original, true));
        assert!(!dictation.should_send(&original, false)); // User moved to another chat/window.
        let mut edited = original.clone();
        edited.0.text.push_str(" edited during transcription");
        assert!(!dictation.should_send(&edited, true));
        let mut attached = original.clone();
        attached.1.push("https://example.com/photo".into());
        assert!(!dictation.should_send(&attached, true));
        dictation = Dictation::default(); // Cancel or chat teardown.
        assert!(!dictation.accepts_result(Some(owner)));
    }
}
