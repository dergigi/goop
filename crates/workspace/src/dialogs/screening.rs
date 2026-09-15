use std::collections::HashMap;

use anyhow::Error;
use common::TimestampExt;
use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, Context, Div, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, Styled, Subscription, Task, Window, div, px, relative, uniform_list,
};
use instant::Duration;
use nostr_sdk::prelude::*;
use person::{Person, PersonRegistry, shorten_pubkey};
use smallvec::{SmallVec, smallvec};
use state::{BOOTSTRAP_RELAYS, NostrAddress, NostrRegistry, TIMEOUT};
use theme::ActiveTheme;
use ui::avatar::Avatar;
use ui::button::{Button, ButtonVariants};
use ui::indicator::Indicator;
use ui::{Disableable, Icon, IconName, Sizable, StyledExt, WindowExtension, h_flex, v_flex};

pub fn init(public_key: PublicKey, window: &mut Window, cx: &mut App) -> Entity<Screening> {
    cx.new(|cx| Screening::new(public_key, window, cx))
}

/// Screening
pub struct Screening {
    /// Public Key of the person being screened.
    public_key: PublicKey,

    /// Whether the person's address is verified.
    verified: Option<bool>,
    verifying_address: Option<Nip05Address>,
    verification_task: Option<Task<()>>,
    profile_loading: bool,
    activity_loading: bool,

    /// Whether the person is followed by current user.
    followed: Option<bool>,

    /// Last time the person was active.
    last_active: Option<Timestamp>,

    /// All mutual contacts of the person being screened.
    mutual_contacts: Vec<PublicKey>,
    mutual_loading: bool,

    /// Async tasks
    tasks: SmallVec<[Task<()>; 3]>,

    /// Subscriptions
    _subscriptions: SmallVec<[Subscription; 1]>,
}

impl Screening {
    pub fn new(public_key: PublicKey, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = smallvec![];

        subscriptions.push(cx.on_release_in(window, move |this, window, cx| {
            this.tasks.clear();
            window.close_all_modals(cx);
        }));

        cx.defer_in(window, |this, _window, cx| {
            this.load_profile(cx);
            this.check_contact(cx);
            this.check_wot(cx);
            this.check_last_activity(cx);
        });

        Self {
            public_key,
            verified: None,
            verifying_address: None,
            verification_task: None,
            profile_loading: true,
            activity_loading: true,
            followed: None,
            last_active: None,
            mutual_contacts: vec![],
            mutual_loading: true,
            tasks: smallvec![],
            _subscriptions: subscriptions,
        }
    }

    fn load_profile(&mut self, cx: &mut Context<Self>) {
        self.profile_loading = true;
        let task = PersonRegistry::global(cx)
            .update(cx, |persons, cx| persons.refresh(self.public_key, cx));
        self.tasks.push(cx.spawn(async move |this, cx| {
            if let Err(error) = task.await {
                log::warn!("Could not refresh request profile: {error}");
            }
            this.update(cx, |this, cx| {
                this.profile_loading = false;
                cx.notify();
            })
            .ok();
        }));
    }

    fn check_contact(&mut self, cx: &mut Context<Self>) {
        let nostr = NostrRegistry::global(cx);
        let client = nostr.read(cx).client();
        let public_key = self.public_key;

        let Some(current_user) = nostr.read(cx).current_user() else {
            return;
        };

        let task: Task<Result<bool, Error>> = cx.background_spawn(async move {
            // Check if user is in contact list
            let filter = Filter::new()
                .author(current_user)
                .kind(Kind::ContactList)
                .limit(1);

            let followed = client
                .database()
                .query(filter)
                .await
                .unwrap_or_default()
                .into_iter()
                .next()
                .map(|event| event.tags.public_keys().any(|k| k == public_key))
                .unwrap_or(false);

            Ok(followed)
        });

        self.tasks.push(cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or(false);

            this.update(cx, |this, cx| {
                this.followed = Some(result);
                cx.notify();
            })
            .ok();
        }));
    }

    fn check_wot(&mut self, cx: &mut Context<Self>) {
        let nostr = NostrRegistry::global(cx);
        let client = nostr.read(cx).client();
        let public_key = self.public_key;

        let Some(current_user) = nostr.read(cx).current_user() else {
            return;
        };

        let task: Task<Result<Vec<PublicKey>, Error>> = cx.background_spawn(async move {
            // Check mutual contacts
            let filter = Filter::new().kind(Kind::ContactList).pubkey(public_key);
            let own_contacts = client
                .database()
                .query(
                    Filter::new()
                        .kind(Kind::ContactList)
                        .author(current_user)
                        .limit(1),
                )
                .await?;
            let followed: std::collections::HashSet<_> = own_contacts
                .iter()
                .flat_map(|event| event.tags.public_keys())
                .collect();
            let mut mutual_contacts = vec![];

            if let Ok(events) = client.database().query(filter).await {
                for event in events
                    .into_iter()
                    .filter(|ev| followed.contains(&ev.pubkey) && ev.pubkey != public_key)
                {
                    mutual_contacts.push(event.pubkey);
                }
            }

            Ok(mutual_contacts)
        });

        self.tasks.push(cx.spawn(async move |this, cx| {
            match task.await {
                Ok(contacts) => {
                    this.update(cx, |this, cx| {
                        this.mutual_contacts = contacts;
                        this.mutual_loading = false;
                        cx.notify();
                    })
                    .ok();
                }
                Err(e) => {
                    log::error!("Failed to fetch mutual contacts: {}", e);
                    this.update(cx, |this, cx| {
                        this.mutual_loading = false;
                        cx.notify();
                    })
                    .ok();
                }
            };
        }));
    }

    fn check_last_activity(&mut self, cx: &mut Context<Self>) {
        let nostr = NostrRegistry::global(cx);
        let client = nostr.read(cx).client();
        let public_key = self.public_key;

        let task: Task<Option<Timestamp>> = cx.background_spawn(async move {
            let filter = Filter::new().author(public_key).limit(1);
            let mut activity: Option<Timestamp> = None;

            // Construct target for subscription
            let target: HashMap<&str, Vec<Filter>> = BOOTSTRAP_RELAYS
                .into_iter()
                .map(|relay| (relay, vec![filter.clone()]))
                .collect();

            if let Ok(mut stream) = client
                .stream_events(target)
                .timeout(Duration::from_secs(TIMEOUT))
                .await
            {
                while let Some((_url, event)) = stream.next().await {
                    if let Ok(event) = event {
                        activity = Some(
                            activity.map_or(event.created_at, |old| old.max(event.created_at)),
                        );
                    }
                }
            }

            activity
        });

        self.tasks.push(cx.spawn(async move |this, cx| {
            let result = task.await;

            this.update(cx, |this, cx| {
                this.last_active = result;
                this.activity_loading = false;
                cx.notify();
            })
            .ok();
        }));
    }

    fn verify_identifier(&mut self, cx: &mut Context<Self>) {
        let http_client = cx.http_client();
        let public_key = self.public_key;

        // Skip if the user doesn't have a NIP-05 identifier
        let Some(address) = self.address(cx) else {
            return;
        };

        self.verifying_address = Some(address.clone());
        self.verified = None;
        let task: Task<Result<bool, Error>> =
            cx.background_spawn(async move { address.verify(&http_client, &public_key).await });

        self.verification_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await.unwrap_or(false);

            this.update(cx, |this, cx| {
                this.verified = Some(result);
                cx.notify();
            })
            .ok();
        }));
    }

    fn profile(&self, cx: &Context<Self>) -> Person {
        let persons = PersonRegistry::global(cx);
        persons.read(cx).get(&self.public_key, cx)
    }

    fn address(&self, cx: &Context<Self>) -> Option<Nip05Address> {
        self.profile(cx)
            .metadata()
            .nip05
            .and_then(|addr| Nip05Address::parse(&addr).ok())
    }

    fn open_njump(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Ok(bech32) = self.profile(cx).public_key().to_bech32();
        cx.open_url(&format!("https://njump.to/{bech32}"));
    }

    fn confirm_report(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity().downgrade();
        let name = self.profile(cx).name();
        let public_key = self.public_key.to_bech32().unwrap();
        window.open_modal(cx, move |modal, _, _| {
            let view = view.clone();
            modal.confirm()
                .title("Report user?")
                .button_props(ui::modal::ModalButtonProps::default()
                    .ok_text("Send public report").cancel_text("Cancel"))
                .child(v_flex().gap_2().text_sm()
                    .child(format!("Report {name} for impersonation?"))
                    .child(public_key.clone())
                    .child("This publishes a public report signed by your account. Other clients may use it to filter this person."))
                .on_ok(move |_, window, cx| {
                    view.update(cx, |view, cx| view.report(window, cx)).ok();
                    true
                })
        });
    }

    fn report(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let nostr = NostrRegistry::global(cx);
        let client = nostr.read(cx).client();
        let signer = nostr.read(cx).signer();
        let public_key = self.public_key;

        let task: Task<Result<(), Error>> = cx.background_spawn(async move {
            let tag = Tag::from(Nip56Tag::PublicKey {
                public_key,
                report: Report::Impersonation,
            });

            let event = EventBuilder::new(Kind::Reporting, "")
                .tag(tag)
                .finalize_async(&signer)
                .await?;

            // Send the report to the public relays
            client.send_event(&event).to(BOOTSTRAP_RELAYS).await?;

            Ok(())
        });

        self.tasks.push(cx.spawn_in(window, async move |_, cx| {
            if task.await.is_ok() {
                cx.update(|window, cx| {
                    window.close_modal(cx);
                    window.push_notification("Report submitted successfully", cx);
                })
                .ok();
            }
        }));
    }

    fn mutual_contacts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let contacts = self.mutual_contacts.clone();

        window.open_modal(cx, move |this, _window, _cx| {
            let contacts = contacts.clone();
            let total = contacts.len();

            this.title("Mutual contacts").child(
                v_flex().gap_1().pb_2().child(
                    uniform_list("contacts", total, move |range, _window, cx| {
                        let persons = PersonRegistry::global(cx);
                        let mut items = Vec::with_capacity(total);

                        for ix in range {
                            let Some(contact) = contacts.get(ix) else {
                                continue;
                            };
                            let profile = persons.read(cx).get(contact, cx);

                            items.push(
                                h_flex()
                                    .h_11()
                                    .w_full()
                                    .px_2()
                                    .gap_1p5()
                                    .rounded(cx.theme().radius)
                                    .text_sm()
                                    .hover(|this| this.bg(cx.theme().elevated_surface_background))
                                    .child(Avatar::new(profile.avatar()).small())
                                    .child(profile.name()),
                            );
                        }

                        items
                    })
                    .h(px(300.)),
                ),
            )
        });
    }
}

impl Render for Screening {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        const CONTACT: &str = "This person is one of your contacts.";
        const NOT_CONTACT: &str = "This person is not one of your contacts.";
        const NO_ACTIVITY: &str = "No public activity found on the relays checked.";
        const RELAY_INFO: &str = "Only checked on public relays; may be inaccurate.";
        const NO_MUTUAL: &str = "No mutual contacts found.";
        const NIP05_MATCH: &str = "The address matches the user's public key.";
        const NIP05_NOT_MATCH: &str = "Could not verify this address.";
        const NO_NIP05: &str = "No address found in the available profile.";

        let address = self.address(cx);
        if address != self.verifying_address {
            self.verification_task = None;
            self.verifying_address = None;
            self.verified = None;
            if address.is_some() {
                self.verify_identifier(cx);
            }
        }
        let profile = self.profile(cx);
        let shorten_pubkey = shorten_pubkey(self.public_key, 8);

        let last_active = if self.activity_loading {
            None
        } else {
            Some(self.last_active.is_some())
        };
        let mutuals = self.mutual_contacts.len();
        let mutuals_str = format!(
            "{} mutual contact{}",
            mutuals,
            if mutuals == 1 { "" } else { "s" }
        );

        v_flex()
            .gap_4()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().text_muted)
                    .child("This person wants to start a conversation with you."),
            )
            .child(
                v_flex()
                    .gap_3()
                    .items_center()
                    .justify_center()
                    .text_center()
                    .child(Avatar::new(profile.avatar()).large())
                    .when(self.profile_loading, |this| {
                        this.child(
                            h_flex().gap_2().child(Indicator::new().small()).child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().text_muted)
                                    .child("Loading profile…"),
                            ),
                        )
                    })
                    .when(
                        !self.profile_loading && profile.metadata() == Metadata::default(),
                        |this| {
                            this.child(
                                Button::new("retry-profile")
                                    .label("Retry loading profile")
                                    .small()
                                    .ghost()
                                    .on_click(cx.listener(|this, _, _, cx| this.load_profile(cx))),
                            )
                        },
                    )
                    .child(
                        div()
                            .font_semibold()
                            .line_height(relative(1.25))
                            .child(profile.name()),
                    ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        h_flex()
                            .p_1()
                            .flex_1()
                            .h_7()
                            .justify_center()
                            .rounded_full()
                            .bg(cx.theme().elevated_surface_background)
                            .text_sm()
                            .truncate()
                            .text_ellipsis()
                            .text_center()
                            .line_height(relative(1.))
                            .child(
                                Button::new("copy-request-key")
                                    .label(shorten_pubkey)
                                    .small()
                                    .ghost()
                                    .tooltip("Copy public key")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let Ok(npub) = this.public_key.to_bech32();
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                            npub,
                                        ));
                                    })),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("njump")
                                    .icon(IconName::Link)
                                    .label("njump.to")
                                    .secondary()
                                    .small()
                                    .rounded()
                                    .on_click(cx.listener(move |this, _e, window, cx| {
                                        this.open_njump(window, cx);
                                    })),
                            )
                            .child(
                                Button::new("report")
                                    .tooltip("Report user for impersonation")
                                    .label("Report user…")
                                    .icon(IconName::Flag)
                                    .small()
                                    .warning()
                                    .rounded()
                                    .on_click(cx.listener(move |this, _e, window, cx| {
                                        this.confirm_report(window, cx);
                                    })),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .gap_3()
                    .child(
                        h_flex()
                            .items_start()
                            .gap_2()
                            .text_sm()
                            .child(status_badge(self.followed, cx))
                            .child(
                                v_flex().text_sm().child("Contact").child(
                                    div()
                                        .line_clamp(1)
                                        .text_color(cx.theme().text_muted)
                                        .child({
                                            if self.followed == Some(true) {
                                                SharedString::from(CONTACT)
                                            } else {
                                                SharedString::from(if self.followed.is_none() {
                                                    "Checking contacts…"
                                                } else {
                                                    NOT_CONTACT
                                                })
                                            }
                                        }),
                                ),
                            ),
                    )
                    .child(
                        h_flex()
                            .items_start()
                            .gap_2()
                            .text_sm()
                            .child(status_badge(last_active, cx))
                            .child(
                                v_flex()
                                    .text_sm()
                                    .child(
                                        h_flex().gap_0p5().child("Public activity").child(
                                            Button::new("active")
                                                .icon(IconName::Info)
                                                .xsmall()
                                                .ghost()
                                                .rounded()
                                                .tooltip(RELAY_INFO),
                                        ),
                                    )
                                    .child(
                                        div()
                                            .w_full()
                                            .line_clamp(1)
                                            .text_color(cx.theme().text_muted)
                                            .map(|this| {
                                                if let Some(t) = self.last_active {
                                                    this.child(SharedString::from(format!(
                                                        "Last active: {}.",
                                                        t.to_human_time()
                                                    )))
                                                } else {
                                                    this.child(SharedString::from(
                                                        if self.activity_loading {
                                                            "Checking public activity…"
                                                        } else {
                                                            NO_ACTIVITY
                                                        },
                                                    ))
                                                }
                                            }),
                                    ),
                            ),
                    )
                    .child(
                        h_flex()
                            .items_start()
                            .gap_2()
                            .child(status_badge(
                                if self.profile_loading || address.is_some() {
                                    self.verified
                                } else {
                                    Some(false)
                                },
                                cx,
                            ))
                            .child(
                                v_flex()
                                    .text_sm()
                                    .child({
                                        if let Some(addr) = self.address(cx) {
                                            SharedString::from(format!("Address: {}", addr))
                                        } else {
                                            SharedString::from("Profile address (NIP-05)")
                                        }
                                    })
                                    .child(
                                        div()
                                            .line_clamp(1)
                                            .text_color(cx.theme().text_muted)
                                            .child({
                                                if self.address(cx).is_some() {
                                                    if self.verified == Some(true) {
                                                        SharedString::from(NIP05_MATCH)
                                                    } else {
                                                        SharedString::from(
                                                            if self.verified.is_none() {
                                                                "Verifying address…"
                                                            } else {
                                                                NIP05_NOT_MATCH
                                                            },
                                                        )
                                                    }
                                                } else {
                                                    SharedString::from(if self.profile_loading {
                                                        "Waiting for profile…"
                                                    } else {
                                                        NO_NIP05
                                                    })
                                                }
                                            }),
                                    ),
                            ),
                    )
                    .child(
                        h_flex()
                            .items_start()
                            .gap_2()
                            .child(status_badge(
                                if self.mutual_loading {
                                    None
                                } else {
                                    Some(mutuals > 0)
                                },
                                cx,
                            ))
                            .child(
                                h_flex()
                                    .text_sm()
                                    .child(
                                        div()
                                            .line_clamp(1)
                                            .text_color(cx.theme().text_muted)
                                            .child({
                                                if mutuals > 0 {
                                                    SharedString::from(mutuals_str)
                                                } else {
                                                    SharedString::from(if self.mutual_loading {
                                                        "Checking mutual contacts…"
                                                    } else {
                                                        NO_MUTUAL
                                                    })
                                                }
                                            }),
                                    )
                                    .child(
                                        Button::new("mutuals")
                                            .icon(IconName::Info)
                                            .xsmall()
                                            .ghost()
                                            .rounded()
                                            .disabled(mutuals == 0)
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.mutual_contacts(window, cx);
                                            })),
                                    ),
                            ),
                    ),
            )
    }
}

fn status_badge(status: Option<bool>, cx: &App) -> Div {
    h_flex()
        .size_6()
        .justify_center()
        .flex_shrink_0()
        .map(|this| {
            if let Some(status) = status {
                this.child(Icon::new(IconName::CheckCircle).small().text_color({
                    if status {
                        cx.theme().icon_accent
                    } else {
                        cx.theme().icon_muted
                    }
                }))
            } else {
                this.child(Indicator::new().small())
            }
        })
}
