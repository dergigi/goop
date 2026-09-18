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
use ui::{Disableable, v_flex};
use ui::scroll::ScrollableElement;

pub struct ImportIdentity {
    show_bunker: bool,
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
                if matches!(event, InputEvent::Change) { cx.notify(); }
                if let InputEvent::PressEnter { .. } = event {
                    this.login(window, cx);
                };
            });

        cx.defer_in(window, |this, window, cx| this.pair(window, cx));
        Self {
            show_bunker: false,
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

    pub(crate) fn pairing_approved(&self, cx: &gpui::App) -> bool {
        self.pairing.as_ref().is_some_and(|pairing| pairing.read(cx).is_approved())
    }

    fn stop_pairing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pairing) = self.pairing.take() {
            pairing.update(cx, |pairing, cx| pairing.stop(window, cx));
        }
        self.pairing_subscription = None;
    }

    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    fn scan(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading || self.scanner.is_some() { return; }
        self.stop_pairing(window, cx);
        let scanner = cx.new(|cx| super::qr_scanner::QrScanner::new(window,cx));
        self.scan_subscription = Some(cx.subscribe_in(&scanner,window,|this,_,event,window,cx| {
            if let super::qr_scanner::ScanEvent::Scanned(url) = event {
                this.show_bunker = true;
                this.key_input.update(cx,|input,cx| input.set_value(url.clone(),window,cx));
                this.error.update(cx,|error,cx| { *error=None; cx.notify(); });
            }
            this.scanner = None;
            this.scan_subscription = None;
            if this.show_bunker {
                this.key_input.update(cx,|input,cx| input.focus(window,cx));
            } else {
                this.pair(window, cx);
            }
            cx.notify();
        }));
        self.scanner = Some(scanner);
        cx.notify();
    }

    fn pair(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading || self.pairing.is_some() { return; }
        let pairing = cx.new(|cx| super::pair_signer::PairSigner::new(window, cx));
        self.pairing_subscription = Some(cx.subscribe_in(&pairing, window, |this, _, event, window, cx| {
            match event {
                super::pair_signer::PairEvent::Saving(saving) => this.loading = *saving,
                super::pair_signer::PairEvent::Cancelled => {
                    this.pairing = None;
                    this.pairing_subscription = None;
                    this.show_bunker = true;
                    this.key_input.update(cx, |input, cx| input.focus(window, cx));
                }
            }
            cx.notify();
        }));
        self.pairing = Some(pairing);
        cx.notify();
    }

    fn login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading { return; }
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        if self.scanner.is_some() { return; }
        let value = self.key_input.read(cx).value();
        self.error.update(cx, |error, cx| { *error = None; cx.notify(); });
        self.set_loading(true, cx);
        match NostrConnectUri::parse(value.trim()) {
            Ok(uri @ NostrConnectUri::Bunker { .. }) => {
                self.stop_pairing(window, cx);
                self.bunker(uri, window, cx);
            },
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
                cx.update(|_, cx| state::credentials::write(cx, USER_KEYRING, "bunker", password.as_bytes()))?.await?;
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        if let Some(scanner) = &self.scanner { return scanner.clone().into_any_element(); }
        v_flex().w_full().gap_3().text_sm()
            .max_h((window.viewport_size().height - gpui::px(130.)).max(gpui::px(120.)))
            .overflow_y_scrollbar()
            .when_some(self.pairing.clone(), |view, pairing| view.child(pairing))
            .child(Button::new("use-bunker-url")
                .label("Use a bunker URL")
                .icon(if self.show_bunker { ui::IconName::CaretDown } else { ui::IconName::CaretRight })
                .ghost().align_left().w_full().disabled(self.loading)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.show_bunker = !this.show_bunker;
                    if this.show_bunker {
                        this.key_input.update(cx, |input, cx| input.focus(window, cx));
                    } else {
                        this.pair(window, cx);
                    }
                    cx.notify();
                })))
            .when(self.show_bunker, |view| view.child(v_flex().w_full().gap_3()
                .child(Input::new(&self.key_input))
                .child(Button::new("login").label("Connect").primary().w_full()
                    .loading(self.loading)
                    .disabled(self.loading || !matches!(NostrConnectUri::parse(self.key_input.read(cx).value().trim()), Ok(NostrConnectUri::Bunker { .. })))
                    .on_click(cx.listener(|this, _, window, cx| this.login(window, cx))))
                .when_some(self.error.read(cx).as_ref(), |view, error| view.child(
                    div().text_xs().text_color(cx.theme().text_danger).child(error.clone())))))
            .map(|view| {
                #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
                let view = view.child(Button::new("scan-bunker-qr").icon(ui::IconName::Scan).label("Scan signer QR")
                    .ghost().align_left().w_full().disabled(self.loading)
                    .on_click(cx.listener(|this, _, window, cx| this.scan(window, cx))));
                view
            })
            .when_some(NostrRegistry::global(cx).read(cx).signer_connection_error().map(str::to_owned), |view, error| view
                .child(div().text_color(cx.theme().text_warning).child(error))
                .child(Button::new("retry-saved-signer").label("Retry saved connection").ghost().disabled(self.loading)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.stop_pairing(window, cx);
                        this.show_bunker = true;
                        NostrRegistry::global(cx).update(cx, |state, cx| state.retry_signer(cx));
                        cx.notify();
                    }))))
            .into_any_element()
    }
}
