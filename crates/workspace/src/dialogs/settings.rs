use gpui::http_client::Url;
use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, Context, Entity, IntoElement, ParentElement, Render, SharedString, Styled,
    Subscription, Window, div, px,
};
use settings::AppSettings;
use state::NostrRegistry;
use theme::{ActiveTheme, Appearance, Theme};
use ui::button::{Button, ButtonVariants};
use ui::group_box::{GroupBox, GroupBoxVariants};
use ui::input::{Input, InputState};
use ui::menu::{DropdownMenu, PopupMenuItem};
use ui::notification::Notification;
use ui::switch::Switch;
use ui::{IconName, Sizable, WindowExtension, h_flex, v_flex};

pub fn init(window: &mut Window, cx: &mut App) -> Entity<Preferences> {
    cx.new(|cx| Preferences::new(window, cx))
}

pub struct Preferences {
    file_input: Entity<InputState>,
    _subscription: Subscription,
}

impl Preferences {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let server = AppSettings::get_file_server(cx);
        let file_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(server.to_string())
                .placeholder("https://myblossom.com")
        });

        let registry = NostrRegistry::global(cx);
        let subscription = cx.observe(&registry, |_, _, cx| cx.notify());
        registry.update(cx, |registry, cx| registry.refresh_media_servers(cx));
        Self {
            file_input,
            _subscription: subscription,
        }
    }

    /// Update the file server (blossom) URL
    fn update_file_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.file_input.read(cx).value();

        match Url::parse(&value) {
            Ok(url) => {
                AppSettings::update_file_server(url, cx);
            }
            Err(e) => {
                window.push_notification(Notification::error(e.to_string()).autohide(false), cx);
            }
        }
    }

    /// Set the theme mode (light or dark)
    fn set_appearance(mode: Appearance, window: &mut Window, cx: &mut App) {
        AppSettings::update_appearance(mode, cx);
        Theme::change(mode.resolve(window.appearance()), Some(window), cx);
    }
}

impl Render for Preferences {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        const SCREENING: &str = "Show a screening dialog to verify unknown senders.";
        const AVATAR: &str = "Hide all avatar pictures to improve performance.";
        const MODE: &str = "Follow your system appearance, or choose light or dark.";

        let servers = NostrRegistry::global(cx).read(cx).media_servers().to_vec();
        let screening = AppSettings::get_screening(cx);
        let auto_block_reports = AppSettings::get_auto_block_reports(cx);
        let render_markdown = AppSettings::get_render_markdown(cx);
        let hide_avatar = AppSettings::get_hide_avatar(cx);
        let appearance = AppSettings::get_appearance(cx);

        v_flex()
            .gap_4()
            .child(
                GroupBox::new()
                    .id("general")
                    .title("General")
                    .fill()
                    .child(
                        Switch::new("screening")
                            .label("Screening")
                            .description(SCREENING)
                            .checked(screening)
                            .on_click(move |_, _window, cx| {
                                AppSettings::update_screening(!screening, cx);
                            }),
                    )
                    .child(
                        Switch::new("auto-block-reports")
                            .label("Automatically block reported users")
                            .description("Block users after a relay accepts your report. You can unblock them from Blocked users.")
                            .checked(auto_block_reports)
                            .on_click(move |_, _, cx| AppSettings::update_auto_block_reports(!auto_block_reports, cx)),
                    )
                    .child(
                        Switch::new("avatar")
                            .label("Hide user avatars")
                            .description(AVATAR)
                            .checked(hide_avatar)
                            .on_click(move |_, _window, cx| {
                                AppSettings::update_hide_avatar(!hide_avatar, cx);
                            }),
                    ),
            )
            .child(
                GroupBox::new()
                    .id("appearance")
                    .title("Appearance")
                    .fill()
                    .child(
                        Switch::new("render-markdown")
                            .label("Render Markdown")
                            .description(
                                "Format chat messages with bold, italic, code, links, and lists.",
                            )
                            .checked(render_markdown)
                            .on_click(move |_, _window, cx| {
                                AppSettings::update_render_markdown(!render_markdown, cx);
                                cx.refresh_windows();
                            }),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .justify_between()
                            .child(
                                v_flex()
                                    .child(div().text_sm().child(SharedString::from("Appearance")))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().text_muted)
                                            .child(SharedString::from(MODE)),
                                    ),
                            )
                            .child(
                                Button::new("theme-mode")
                                    .label(appearance.name())
                                    .ghost_alt()
                                    .small()
                                    .dropdown_menu(|this, _window, _cx| {
                                        this.item(PopupMenuItem::new("System").on_click(|_, window, cx| { Self::set_appearance(Appearance::System, window, cx); }))
                                        .item(PopupMenuItem::new("Light").on_click(
                                            |_, window, cx| {
                                                Self::set_appearance(Appearance::Light, window, cx);
                                            },
                                        ))
                                        .item(
                                            PopupMenuItem::new("Dark").on_click(|_, window, cx| {
                                                Self::set_appearance(Appearance::Dark, window, cx);
                                            }),
                                        )
                                    }),
                            ),
                    ),
            )
            .child(
                GroupBox::new()
                    .id("media")
                    .title("Media Upload Service")
                    .fill()
                    .when(!servers.is_empty(), |this| {
                        this.child(v_flex().gap_2()
                            .child(div().text_xs().text_color(cx.theme().text_muted)
                                .child("Your published media servers, in upload preference order:"))
                            .children(servers.iter().enumerate().map(|(index, server)| {
                                div().text_sm().child(format!("{}. {}", index + 1, server))
                            })))
                    })
                    .when(servers.is_empty(), |this| this.child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(Input::new(&self.file_input).text_xs().small())
                                    .child(
                                        Button::new("update-file-server")
                                            .icon(IconName::Check)
                                            .ghost()
                                            .size_8()
                                            .on_click(cx.listener(move |this, _ev, window, cx| {
                                                this.update_file_server(window, cx)
                                            })),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .italic()
                                    .text_color(cx.theme().text_placeholder)
                                    .child(SharedString::from("Blossom fallback when your account has no media servers.")),
                            ),
                    )),
            )
            .child(
                h_flex().w_full().gap_2()
                    .child(h_flex().flex_1().min_w_0().flex_wrap().gap_1().text_xs().text_color(cx.theme().text_muted)
                        .child(crate::build_info::version_link())
                        .child("·")
                        .child(crate::build_info::build_link()))
                    .child(Button::new("report-bug").icon(IconName::Bug).label("Report a bug")
                        .small().ghost().on_click(|_, window, cx| {
                            window.close_modal(cx);
                            window.defer(cx, |window, cx| {
                                let key = nostr_sdk::prelude::PublicKey::parse(state::GOOP_NPUB)
                                    .expect("Goop project npub must be valid");
                                window.dispatch_action(Box::new(crate::Command::OpenProfileChat(key, false)), cx);
                            });
                        })),
            )
    }
}
