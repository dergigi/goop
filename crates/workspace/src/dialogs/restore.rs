use instant::Duration;

use anyhow::Error;
use device::DeviceRegistry;
use gpui::prelude::FluentBuilder;
use gpui::{
    AppContext, Context, Entity, IntoElement, ParentElement, Render, SharedString, Styled,
    Subscription, Task, Window, div,
};
use nostr_sdk::prelude::*;
use theme::ActiveTheme;
use ui::button::{Button, ButtonVariants};
use ui::input::{Input, InputEvent, InputState};
use ui::{WindowExtension, v_flex};

#[derive(Debug)]
pub struct RestoreEncryption {
    /// Secret key input
    key_input: Entity<InputState>,

    /// Error message
    error: Entity<Option<SharedString>>,

    /// Async tasks
    tasks: Vec<Task<Result<(), Error>>>,

    /// Event subscription
    _subscription: Option<Subscription>,
}

impl RestoreEncryption {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let key_input = cx.new(|cx| InputState::new(window, cx).masked(true));
        let error = cx.new(|_| None);

        let subscription =
            cx.subscribe_in(&key_input, window, |this, _input, event, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.restore(window, cx);
                };
            });

        Self {
            key_input,
            error,
            tasks: vec![],
            _subscription: Some(subscription),
        }
    }

    fn restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let device = DeviceRegistry::global(cx);
        let content = self.key_input.read(cx).value();

        if !content.is_empty() {
            self.set_error("The secret key cannot be empty.", cx);
        }

        let Ok(secret) = SecretKey::parse(&content) else {
            self.set_error("The secret key is invalid.", cx);
            return;
        };

        device.update(cx, |this, cx| {
            this.set_announcement(Keys::new(secret), cx);
        });

        // Close the current modal
        window.close_modal(cx);
    }

    fn set_error<S>(&mut self, message: S, cx: &mut Context<Self>)
    where
        S: Into<SharedString>,
    {
        // Update error message
        self.error.update(cx, |this, cx| {
            *this = Some(message.into());
            cx.notify();
        });

        // Clear the error message after 3 secs
        self.tasks.push(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(3)).await;

            this.update(cx, |this, cx| {
                this.error.update(cx, |this, cx| {
                    *this = None;
                    cx.notify();
                });
            })?;

            Ok(())
        }));
    }
}

impl Render for RestoreEncryption {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .gap_2()
            .text_sm()
            .child(
                v_flex()
                    .gap_1()
                    .text_sm()
                    .text_color(cx.theme().text_muted)
                    .child("Secret Key")
                    .child(Input::new(&self.key_input)),
            )
            .child(
                Button::new("restore")
                    .label("Restore")
                    .primary()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.restore(window, cx);
                    })),
            )
            .when_some(self.error.read(cx).as_ref(), |this, error| {
                this.child(
                    div()
                        .text_xs()
                        .text_center()
                        .text_color(cx.theme().text_danger)
                        .child(error.clone()),
                )
            })
    }
}
