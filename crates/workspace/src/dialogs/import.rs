use anyhow::{Error, anyhow};
use gpui::prelude::FluentBuilder;
use gpui::{
    AppContext, Context, Entity, IntoElement, ParentElement, Render, SharedString, Styled,
    Subscription, Task, Window, div,
};
use instant::Duration;
use nostr_connect::prelude::*;
use state::{GoopAuthUrlHandler, NostrRegistry, USER_KEYRING};
use theme::ActiveTheme;
use ui::button::{Button, ButtonVariants};
use ui::input::{Input, InputEvent, InputState};
use ui::{Disableable, StyledExt, WindowExtension, divider, v_flex};

#[derive(Debug)]
pub struct ImportIdentity {
    /// Secret key input
    key_input: Entity<InputState>,

    /// Password input (if required)
    pass_input: Entity<InputState>,

    /// Error message
    error: Entity<Option<SharedString>>,

    /// Whether the user is currently loading
    loading: bool,

    /// Async tasks
    tasks: Vec<Task<Result<(), Error>>>,

    /// Input subscription
    _subscription: Option<Subscription>,
}

impl ImportIdentity {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let key_input = cx.new(|cx| InputState::new(window, cx).placeholder("nsec or bunker://"));
        let pass_input = cx.new(|cx| InputState::new(window, cx).masked(true));
        let error = cx.new(|_| None);

        let input_subscription =
            cx.subscribe_in(&key_input, window, |this, _input, event, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.login(window, cx);
                };
            });

        Self {
            key_input,
            pass_input,
            error,
            loading: false,
            tasks: vec![],
            _subscription: Some(input_subscription),
        }
    }

    fn login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.key_input.read(cx).value();
        let password = self.pass_input.read(cx).value();

        // Set loading state
        self.set_loading(true, cx);

        if value.starts_with("ncryptsec1") {
            self.ncryptsec(value, password, window, cx);
            return;
        }

        if value.starts_with("bunker://") {
            match NostrConnectUri::parse(value) {
                Ok(uri) => {
                    self.bunker(uri, window, cx);
                }
                Err(e) => {
                    self.set_error(e.to_string(), cx);
                }
            }
            return;
        }

        if let Ok(secret) = SecretKey::parse(&value) {
            let keys = Keys::new(secret);
            let nostr = NostrRegistry::global(cx);

            // Update the signer
            nostr.update(cx, |this, cx| {
                this.set_signer(keys, cx);
            });
        } else {
            self.set_error("Invalid key", cx);
        }
    }

    fn ncryptsec<S>(&mut self, content: S, pwd: S, window: &mut Window, cx: &mut Context<Self>)
    where
        S: Into<String>,
    {
        let nostr = NostrRegistry::global(cx);
        let content: String = content.into();
        let password: String = pwd.into();

        if password.is_empty() {
            self.set_error("Password is required", cx);
            return;
        }

        let Ok(enc) = EncryptedSecretKey::from_bech32(&content) else {
            self.set_error("Secret Key is invalid", cx);
            return;
        };

        // Decrypt in the background to ensure it doesn't block the UI
        let task = cx.background_spawn(async move {
            if let Ok(content) = enc.decrypt(&password) {
                Ok(Keys::new(content))
            } else {
                Err(anyhow!("Invalid password"))
            }
        });

        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            match task.await {
                Ok(keys) => {
                    nostr.update_in(cx, |this, window, cx| {
                        this.set_signer(keys, cx);
                        window.close_modal(cx);
                    })?;
                }
                Err(e) => {
                    this.update(cx, |this, cx| {
                        this.set_error(e.to_string(), cx);
                    })?;
                }
            }
            Ok(())
        }));
    }

    fn bunker(&mut self, uri: NostrConnectUri, window: &mut Window, cx: &mut Context<Self>) {
        let nostr = NostrRegistry::global(cx);
        let master_keys = nostr.read(cx).get_master_key(cx);
        let password = uri.to_string();
        let save = cx.write_credentials(USER_KEYRING, "bunker", password.as_bytes());

        self.tasks.push(cx.spawn_in(window, async move |_this, cx| {
            let keys = master_keys.await;
            let timeout = Duration::from_secs(30);

            // Construct the nostr connect signer
            let mut signer = NostrConnect::new(uri, keys, timeout, None)?;

            // Handle auth url with the default browser
            signer.auth_url_handler(GoopAuthUrlHandler);

            nostr.update(cx, |this, cx| {
                this.set_signer(signer, cx);
                cx.background_spawn(async move { save.await.ok() }).detach();
                cx.notify();
            });

            Ok(())
        }));
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn proxy(&mut self, cx: &mut Context<Self>) {
        let nostr = NostrRegistry::global(cx);
        nostr.update(cx, |this, cx| {
            this.connect_proxy(cx);
        });
    }

    fn set_loading(&mut self, status: bool, cx: &mut Context<Self>) {
        self.loading = status;
        cx.notify();
    }

    fn set_error<S>(&mut self, message: S, cx: &mut Context<Self>)
    where
        S: Into<SharedString>,
    {
        self.set_loading(false, cx);

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

impl Render for ImportIdentity {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        const BUNKER_WARN: &str = "Nostr Connect will usually take more time to get all your messages. Please keep your session open until you see all your messages.";
        const KEY_WARN: &str = "Goop won't store your identity key on the local device. You need to re-login again in the next session. You can use Nostr Connect for persistent login.";

        let is_wasm = cfg!(target_arch = "wasm32");
        let require_password = self.key_input.read(cx).value().starts_with("ncryptsec1");
        let key_warning = self.key_input.read(cx).value().starts_with("nsec1") || require_password;
        let bunker_warning = self.key_input.read(cx).value().starts_with("bunker://");

        v_flex()
            .size_full()
            .gap_4()
            .text_sm()
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        v_flex()
                            .gap_1()
                            .text_color(cx.theme().text_muted)
                            .child("Continue with existing key or bunker connection")
                            .child(Input::new(&self.key_input)),
                    )
                    .when(require_password, |this| {
                        this.child(
                            v_flex()
                                .gap_1()
                                .text_color(cx.theme().text_muted)
                                .child("Decrypt Password:")
                                .child(Input::new(&self.pass_input)),
                        )
                    })
                    .when(bunker_warning, |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().text_warning)
                                .child(div().child(BUNKER_WARN)),
                        )
                    })
                    .when(key_warning, |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().text_warning)
                                .child(div().child(KEY_WARN)),
                        )
                    }),
            )
            .child(
                Button::new("login")
                    .label("Continue")
                    .primary()
                    .font_semibold()
                    .loading(self.loading)
                    .disabled(self.loading)
                    .on_click(cx.listener(move |this, _ev, window, cx| {
                        this.login(window, cx);
                    })),
            )
            .child(divider(cx))
            .when(!is_wasm, |this| {
                this.child(
                    Button::new("proxy")
                        .label("Connect via Web Extension (Experimental)")
                        .ghost_alt()
                        .loading(self.loading)
                        .disabled(self.loading)
                        .on_click(cx.listener(move |this, _ev, _window, cx| {
                            this.proxy(cx);
                        })),
                )
            })
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
