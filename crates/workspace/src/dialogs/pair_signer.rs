//! Client-initiated NIP-46 pairing. Invitations are ephemeral; only the
//! authenticated, secret-free bunker connection is persisted.
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use futures::{
    FutureExt,
    future::{Either, select},
};
use gpui::prelude::FluentBuilder;
use gpui::{
    App, ClipboardItem, Context, EventEmitter, FocusHandle, InteractiveElement,
    IntoElement, ParentElement, Render, RenderImage, Styled, Subscription, Task, Window, div, img,
    px,
};
use nostr_connect::prelude::*;
use qrcode::{Color, QrCode};
use state::{GoopAuthUrlHandler, NOSTR_CONNECT_RELAY, NostrRegistry, USER_KEYRING};
use theme::ActiveTheme;
use ui::button::{Button, ButtonVariants};
use ui::scroll::ScrollableElement;
use ui::{Disableable, IconName, Sizable, h_flex, v_flex};

const PAIRING_TIMEOUT: Duration = Duration::from_secs(120);

fn invitation(keys: &Keys, relays: Vec<RelayUrl>) -> NostrConnectUri {
    let mut uri = NostrConnectUri::client(keys.public_key(), relays, "Goop");
    if let NostrConnectUri::Client { metadata, .. } = &mut uri {
        metadata.url = Some(Url::parse("https://goop.dergigi.com").unwrap());
    }
    uri
}

fn qr_pixels(text: &str) -> Result<image::RgbaImage> {
    let code = QrCode::new(text)?;
    let scale = 4;
    let side = (code.width() + 8) * scale;
    let mut image = image::RgbaImage::from_pixel(side as u32, side as u32, image::Rgba([255; 4]));
    for y in 0..code.width() {
        for x in 0..code.width() {
            if code[(x, y)] == Color::Dark {
                for dy in 0..scale {
                    for dx in 0..scale {
                        image.put_pixel(
                            ((x + 4) * scale + dx) as u32,
                            ((y + 4) * scale + dy) as u32,
                            image::Rgba([0, 0, 0, 255]),
                        );
                    }
                }
            }
        }
    }
    Ok(image)
}

/// The SDK authenticates the invitation secret before learning the account key.
/// Keep the signer alive after success, so its verified identity is reused.
async fn approve(
    signer: NostrConnect,
    timeout: Duration,
    cancel: flume::Receiver<()>,
) -> Result<(NostrConnect, NostrConnectUri)> {
    let handshake = async {
        signer.get_public_key_async().await?;
        let bunker = signer.bunker_uri().await?;
        Ok::<_, anyhow::Error>(bunker)
    };
    let interruption = async {
        let timer = async {
            smol::Timer::after(timeout).await;
        };
        match select(cancel.recv_async().boxed(), timer.boxed()).await {
            Either::Left(_) => "Pairing cancelled.",
            Either::Right(_) => "Pairing expired. Generate a new code.",
        }
    };
    let result = match select(handshake.boxed(), interruption.boxed()).await {
        Either::Left((result, _)) => result,
        Either::Right((reason, _)) => Err(anyhow!(reason)),
    };
    match result {
        Ok(bunker) => Ok((signer, bunker)),
        Err(error) => {
            signer.shutdown().await;
            Err(error)
        }
    }
}

pub enum PairEvent {
    Cancelled,
}

pub struct PairSigner {
    focus: FocusHandle,
    cancel: Option<flume::Sender<()>>,
    image: Option<Arc<RenderImage>>,
    link: Option<String>,
    signer: Option<NostrConnect>,
    error: Option<String>,
    saving: bool,
    task: Option<Task<()>>,
    _release: Subscription,
}
impl EventEmitter<PairEvent> for PairSigner {}

impl PairSigner {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let release = cx.on_release_in(window, |this: &mut Self, window, cx| this.stop(window, cx));
        cx.defer_in(window, |this, window, cx| this.start(window, cx));
        Self {
            focus,
            cancel: None,
            image: None,
            link: None,
            signer: None,
            error: None,
            saving: false,
            task: None,
            _release: release,
        }
    }

    fn clear_image(&mut self, window: &mut Window) {
        if let Some(image) = self.image.take() {
            let _ = window.drop_image(image);
        }
    }

    fn stop(&mut self, window: &mut Window, cx: &mut App) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.try_send(());
        }
        self.task = None;
        self.clear_image(window);
        self.link = None;
        if let Some(signer) = self.signer.take() {
            gpui_tokio::Tokio::spawn(cx, async move { signer.shutdown().await }).detach();
        }
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stop(window, cx);
        self.error = None;
        self.saving = false;
        let (cancel, cancelled) = flume::bounded(1);
        self.cancel = Some(cancel);
        let keys = NostrRegistry::global(cx).read(cx).get_master_key(cx, true);
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result: Result<()> = async {
                let keys = keys.await?;
                let uri = invitation(
                    &keys,
                    vec![
                        RelayUrl::parse(NOSTR_CONNECT_RELAY)?,
                        RelayUrl::parse("wss://relay.nsec.app")?,
                    ],
                );
                let link = uri.to_string();
                let pixels = qr_pixels(&link)?;
                let mut signer = NostrConnect::new(uri, keys, PAIRING_TIMEOUT, None)?;
                signer.auth_url_handler(GoopAuthUrlHandler);
                this.update(cx, |this, cx| {
                    this.signer = Some(signer.clone());
                    this.link = Some(link);
                    this.image = Some(Arc::new(RenderImage::new(smallvec::smallvec![
                        image::Frame::new(pixels)
                    ])));
                    cx.notify();
                })?;
                let approved = gpui_tokio::Tokio::spawn_result(
                    cx,
                    approve(signer, PAIRING_TIMEOUT, cancelled),
                )
                .await?;
                let (signer, bunker) = approved;
                // There is no credential write before authenticated approval.
                let save = this.update_in(cx, |this, window, cx| -> Result<_> {
                    if NostrRegistry::global(cx).read(cx).current_user().is_some() {
                        bail!("Your account changed. Start pairing again.");
                    }
                    this.saving = true;
                    this.clear_image(window);
                    this.link = None;
                    cx.notify();
                    Ok(cx.write_credentials(USER_KEYRING, "bunker", bunker.to_string().as_bytes()))
                })??;
                save.await?;
                this.update(cx, |this, cx| {
                    // Transfer ownership before SignerChanged closes this view.
                    this.signer = None;
                    NostrRegistry::global(cx).update(cx, |state, cx| state.set_bunker(signer, cx));
                })?;
                Ok(())
            }
            .await;
            if let Err(error) = result {
                this.update_in(cx, |this, window, cx| {
                    // The current task ends naturally; don't cancel it from itself.
                    this.clear_image(window);
                    this.link = None;
                    if let Some(signer) = this.signer.take() {
                        gpui_tokio::Tokio::spawn(cx, async move { signer.shutdown().await })
                            .detach();
                    }
                    this.saving = false;
                    this.error = Some(error.to_string());
                    cx.notify();
                })
                .ok();
            }
        }));
        cx.notify();
    }
}

impl Render for PairSigner {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let side = (window.viewport_size().height - px(240.))
            .min(px(280.))
            .max(px(180.));
        v_flex()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" && !this.saving {
                    cx.stop_propagation();
                    this.stop(window, cx);
                    cx.emit(PairEvent::Cancelled);
                }
            }))
            .gap_3()
            .items_center()
            .w_full()
            .max_h((window.viewport_size().height - px(120.)).max(px(120.)))
            .overflow_y_scrollbar()
            .child(div().text_sm().child(if self.saving {
                "Connecting…"
            } else if self.error.is_some() {
                "Pairing stopped"
            } else if self.image.is_some() {
                "Scan in Amber"
            } else {
                "Preparing code…"
            }))
            .when_some(self.image.clone(), |view, image| {
                view.child(img(image).w(side).h(side))
            })
            .when(self.image.is_some(), |view| {
                view.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().text_muted)
                        .child("Waiting for approval"),
                )
            })
            .when_some(self.error.clone(), |view, error| {
                view.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().text_warning)
                        .child(error),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .when_some(self.link.clone(), |row, link| {
                        row.child(
                            Button::new("copy-pairing-link")
                                .icon(IconName::Copy)
                                .small()
                                .ghost()
                                .tooltip("Copy pairing link")
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(link.clone()))
                                }),
                        )
                    })
                    .when(self.error.is_some(), |row| {
                        row.child(
                            Button::new("retry-pairing")
                                .label("New code")
                                .small()
                                .primary()
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.start(window, cx)),
                                ),
                        )
                    })
                    .child(
                        Button::new("cancel-pairing")
                            .label("Cancel")
                            .small()
                            .ghost()
                            .disabled(self.saving)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.stop(window, cx);
                                cx.emit(PairEvent::Cancelled);
                            })),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr_sdk::local_relay::LocalRelay;
    use nostr_sdk::prelude::RelayStatus;

    struct Allow;
    impl NostrConnectSignerActions for Allow {
        fn approve(&self, _: &PublicKey, _: &NostrConnectRequest) -> bool {
            true
        }
    }

    async fn listening(signer: &NostrConnect) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if signer
                    .status()
                    .await
                    .values()
                    .any(|status| *status == RelayStatus::Connected)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        })
        .await
        .unwrap();
    }

    #[test]
    fn qr_round_trip_preserves_pairing_secret_and_amber_metadata() {
        let keys = Keys::generate();
        let uri = invitation(
            &keys,
            vec![RelayUrl::parse("wss://relay.nsec.app").unwrap()],
        );
        let next = invitation(&keys, uri.relays().to_vec());
        assert_ne!(uri.secret(), next.secret());
        assert!(uri.secret().unwrap().len() >= 32);
        let text = uri.to_string();
        assert!(!text.contains(&keys.secret_key().to_secret_hex()));
        let gray = image::DynamicImage::ImageRgba8(qr_pixels(&text).unwrap()).into_luma8();
        let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(
            gray.width() as usize,
            gray.height() as usize,
            |x, y| gray.get_pixel(x as u32, y as u32)[0],
        );
        let grids = prepared.detect_grids();
        assert_eq!(grids.len(), 1);
        let (_, decoded) = grids[0].decode().unwrap();
        assert_eq!(decoded, text);
        let parsed = NostrConnectUri::parse(decoded).unwrap();
        assert_eq!(parsed, uri);
        let NostrConnectUri::Client { metadata, .. } = parsed else {
            panic!("expected client invitation");
        };
        assert_eq!(metadata.name, "Goop");
        assert_eq!(metadata.url.unwrap().as_str(), "https://goop.dergigi.com/");
    }

    #[tokio::test]
    async fn approved_pairing_learns_user_identity_and_produces_reconnectable_bunker() {
        tokio::time::timeout(Duration::from_secs(10), async {
            let relay = LocalRelay::builder().build();
            relay.run().await.unwrap();
            let url = relay.url().await;
            let keys = Keys::generate();
            let remote_key = Keys::generate();
            let account = Keys::generate();
            let uri = invitation(&keys, vec![url.clone()]);
            let signer =
                NostrConnect::new(uri.clone(), keys.clone(), Duration::from_secs(3), None).unwrap();
            let monitor = signer.clone();
            let (_cancel, cancelled) = flume::bounded(1);
            let wait = tokio::spawn(approve(signer, Duration::from_secs(4), cancelled));
            listening(&monitor).await;
            let remote = NostrConnectRemoteSigner::from_uri(
                uri.clone(),
                NostrConnectKeys::new(remote_key.clone(), account.clone()),
                None,
            )
            .unwrap();
            let server = tokio::spawn(async move { remote.serve(Allow).await });
            let (signer, bunker) = wait.await.unwrap().unwrap();
            assert_eq!(
                signer.get_public_key_async().await.unwrap(),
                account.public_key()
            );
            assert_eq!(
                bunker.remote_signer_public_key(),
                Some(&remote_key.public_key())
            );
            assert!(bunker.secret().is_none());
            assert!(!bunker.to_string().contains(uri.secret().unwrap()));
            signer.shutdown().await;
            server.abort();
            let _ = server.await;
            // Model the signer's approved persistent client permission on restart.
            let trusted = NostrConnectRemoteSigner::new(
                NostrConnectKeys::new(remote_key, account.clone()),
                [url],
                None,
                None,
            )
            .unwrap();
            let server = tokio::spawn(async move { trusted.serve(Allow).await });
            // This SDK test server has no readiness hook. Let its relay subscription
            // settle before sending the restart handshake (the relay is ephemeral).
            tokio::time::sleep(Duration::from_millis(100)).await;
            let restored = NostrConnect::new(
                NostrConnectUri::parse(bunker.to_string()).unwrap(),
                keys,
                Duration::from_secs(3),
                None,
            )
            .unwrap();
            assert_eq!(
                restored.get_public_key_async().await.unwrap(),
                account.public_key()
            );
            restored.shutdown().await;
            server.abort();
        })
        .await
        .expect("pairing and reconnect should finish promptly");
    }

    #[tokio::test]
    async fn incorrect_secret_cannot_pair_and_expiry_closes_connections() {
        tokio::time::timeout(Duration::from_secs(5), async {
            let relay = LocalRelay::builder().build();
            relay.run().await.unwrap();
            let keys = Keys::generate();
            let uri = invitation(&keys, vec![relay.url().await]);
            let signer =
                NostrConnect::new(uri.clone(), keys, Duration::from_secs(2), None).unwrap();
            let monitor = signer.clone();
            let (_cancel, cancelled) = flume::bounded(1);
            let wait = tokio::spawn(approve(signer, Duration::from_millis(500), cancelled));
            listening(&monitor).await;
            let mut wrong = uri;
            if let NostrConnectUri::Client { secret, .. } = &mut wrong {
                *secret = "wrong-secret".into();
            }
            let remote = NostrConnectRemoteSigner::from_uri(
                wrong,
                NostrConnectKeys::new(Keys::generate(), Keys::generate()),
                None,
            )
            .unwrap();
            let server = tokio::spawn(async move { remote.serve(Allow).await });
            let error = wait.await.unwrap().unwrap_err();
            assert!(error.to_string().contains("expired"));
            assert!(
                monitor
                    .status()
                    .await
                    .values()
                    .all(|status| *status != RelayStatus::Connected)
            );
            server.abort();
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn cancellation_stops_pairing_before_approval() {
        let relay = LocalRelay::builder().build();
        relay.run().await.unwrap();
        let keys = Keys::generate();
        let uri = invitation(&keys, vec![relay.url().await]);
        let signer = NostrConnect::new(uri, keys, Duration::from_secs(5), None).unwrap();
        let monitor = signer.clone();
        let (cancel, cancelled) = flume::bounded(1);
        let wait = tokio::spawn(approve(signer, Duration::from_secs(5), cancelled));
        listening(&monitor).await;
        cancel.send(()).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(1), wait)
            .await
            .unwrap()
            .unwrap();
        assert!(result.unwrap_err().to_string().contains("cancelled"));
        assert!(
            monitor
                .status()
                .await
                .values()
                .all(|status| *status != RelayStatus::Connected)
        );
    }
}
