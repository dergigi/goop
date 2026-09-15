use anyhow::Error;
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
use ui::{Disableable, StyledExt, divider, v_flex};

#[derive(Debug)]
pub struct ImportIdentity {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    scanner: Option<Entity<super::qr_scanner::QrScanner>>,
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    scan_subscription: Option<Subscription>,
    /// Bunker connection URI
    key_input: Entity<InputState>,

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
        let key_input = cx.new(|cx| InputState::new(window, cx).placeholder("bunker://"));
        let error = cx.new(|_| None);

        let input_subscription =
            cx.subscribe_in(&key_input, window, |this, _input, event, window, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.login(window, cx);
                };
            });

        Self {
            #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
            scanner: None,
            #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
            scan_subscription: None,
            key_input,
            error,
            loading: false,
            tasks: vec![],
            _subscription: Some(input_subscription),
        }
    }

    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    fn scan(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading || self.scanner.is_some() { return; }
        let scanner = cx.new(|cx| super::qr_scanner::QrScanner::new(window,cx));
        self.scan_subscription = Some(cx.subscribe_in(&scanner,window,|this,_,event,window,cx| {
            if let super::qr_scanner::ScanEvent::Scanned(url) = event {
                this.key_input.update(cx,|input,cx| input.set_value(url.clone(),window,cx));
                this.error.update(cx,|error,cx| { *error=None; cx.notify(); });
            }
            this.scanner = None;
            this.scan_subscription = None;
            this.key_input.update(cx,|input,cx| input.focus(window,cx));
            cx.notify();
        }));
        self.scanner = Some(scanner);
        cx.notify();
    }

    fn login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading { return; }
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        if self.scanner.is_some() { return; }
        let value = self.key_input.read(cx).value();
        self.set_loading(true, cx);
        match NostrConnectUri::parse(value.trim()) {
            Ok(uri @ NostrConnectUri::Bunker { .. }) => self.bunker(uri, window, cx),
            _ => self.set_error(
                "Enter a bunker:// connection from your signer application",
                cx,
            ),
        }
    }

    fn bunker(&mut self, uri: NostrConnectUri, window: &mut Window, cx: &mut Context<Self>) {
        let nostr = NostrRegistry::global(cx);
        let master_keys = nostr.read(cx).get_master_key(cx, true);
        let password = uri.to_string();
        let save = cx.write_credentials(USER_KEYRING, "bunker", password.as_bytes());

        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result: Result<(), Error> = async {
                let keys = master_keys.await?;
                save.await?;
                let timeout = Duration::from_secs(30);

                // Construct the nostr connect signer
                let mut signer = NostrConnect::new(uri, keys, timeout, None)?;

                // Handle auth url with the default browser
                signer.auth_url_handler(GoopAuthUrlHandler);

                nostr.update(cx, |this, cx| {
                    this.set_signer(signer, cx);
                    cx.notify();
                });

                Ok(())
            }
            .await;
            if let Err(error) = result {
                this.update_in(cx, |this, _, cx| this.set_error(error.to_string(), cx))?;
            }
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
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        if let Some(scanner) = &self.scanner { return scanner.clone().into_any_element(); }
        const BUNKER_WARN: &str = "Nostr Connect will usually take more time to get all your messages. Please keep your session open until you see all your messages.";
        let is_wasm = cfg!(target_arch = "wasm32");
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
                            .child("Connect with your signer application")
                            .child(Input::new(&self.key_input)),
                    )
                    .when(bunker_warning, |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().text_warning)
                                .child(div().child(BUNKER_WARN)),
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
            .map(|view| {
                #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
                let view = view.child(Button::new("scan-bunker-qr").icon(ui::IconName::Scan).label("Scan QR code")
                    .ghost_alt().disabled(self.loading).on_click(cx.listener(|this,_,window,cx| this.scan(window,cx))));
                view
            })
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
            }).into_any_element()
    }
}
