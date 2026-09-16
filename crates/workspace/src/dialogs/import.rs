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
use ui::{Disableable, StyledExt, v_flex};

pub struct ImportIdentity {
    pairing: Option<Entity<super::pair_signer::PairSigner>>,
    pairing_subscription: Option<Subscription>,
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
    _state_subscription: Subscription,
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
            pairing: None,
            pairing_subscription: None,
            #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
            scanner: None,
            #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
            scan_subscription: None,
            key_input,
            error,
            loading: false,
            tasks: vec![],
            _subscription: Some(input_subscription),
            _state_subscription: cx.observe(&NostrRegistry::global(cx), |this, state, cx| {
                if state.read(cx).signer_connection_error().is_some() && !state.read(cx).identity_loading() {
                    this.loading = false;
                }
                cx.notify();
            }),
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

    fn pair(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading || self.pairing.is_some() { return; }
        let pairing = cx.new(|cx| super::pair_signer::PairSigner::new(window, cx));
        self.pairing_subscription = Some(cx.subscribe_in(&pairing, window, |this, _, _, window, cx| {
            this.pairing = None;
            this.pairing_subscription = None;
            this.key_input.update(cx, |input, cx| input.focus(window, cx));
            cx.notify();
        }));
        self.pairing = Some(pairing);
        cx.notify();
    }

    fn login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading || self.pairing.is_some() { return; }
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        if self.scanner.is_some() { return; }
        let value = self.key_input.read(cx).value();
        self.error.update(cx, |error, cx| { *error = None; cx.notify(); });
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

        self.tasks.push(cx.spawn_in(window, async move |this, cx| {
            let result: Result<(), Error> = async {
                let keys = master_keys.await?;
                cx.update(|_, cx| cx.write_credentials(USER_KEYRING, "bunker", password.as_bytes()))?.await?;
                let timeout = Duration::from_secs(30);

                // Construct the nostr connect signer
                let mut signer = NostrConnect::new(uri, keys, timeout, None)?;

                // Handle auth url with the default browser
                signer.auth_url_handler(GoopAuthUrlHandler);

                nostr.update(cx, |this, cx| {
                    this.set_bunker(signer, cx);
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
    }
}

impl Render for ImportIdentity {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(pairing) = &self.pairing { return pairing.clone().into_any_element(); }
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        if let Some(scanner) = &self.scanner { return scanner.clone().into_any_element(); }
        const BUNKER_WARN: &str = "Keep Goop and your signer connected while message history loads.";
        let bunker_warning = self.key_input.read(cx).value().starts_with("bunker://");

        v_flex()
            .size_full()
            .gap_4()
            .text_sm()
            .when_some(NostrRegistry::global(cx).read(cx).signer_connection_error().map(str::to_owned), |view, error| view
                .child(div().text_color(cx.theme().text_warning).child(error))
                .child(Button::new("retry-saved-signer").label("Retry saved connection").ghost()
                    .on_click(|_, _, cx| NostrRegistry::global(cx).update(cx, |state, cx| state.retry_signer(cx)))))
            .child(Button::new("connect-with-qr").label("Connect with QR code").icon(ui::IconName::Scan)
                .primary().disabled(self.loading).on_click(cx.listener(|this, _, window, cx| this.pair(window, cx))))
            .child(div().text_xs().text_color(cx.theme().text_muted).text_center().child("or use a bunker URL"))
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        v_flex()
                            .gap_1()
                            .text_color(cx.theme().text_muted)
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
                let view = view.child(Button::new("scan-bunker-qr").icon(ui::IconName::Scan).label("Scan bunker QR")
                    .ghost_alt().disabled(self.loading).on_click(cx.listener(|this,_,window,cx| this.scan(window,cx))));
                view
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
