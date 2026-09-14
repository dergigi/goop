use gpui::http_client::Url;
use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, Context, Entity, IntoElement, ParentElement, Render, SharedString, Styled,
    Subscription, Window, div, px,
};
use settings::AppSettings;
use state::NostrRegistry;
use theme::{ActiveTheme, Theme, ThemeMode};
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
    fn set_theme_mode(mode: ThemeMode, window: &mut Window, cx: &mut App) {
        AppSettings::update_theme_mode(mode, cx);
        Theme::change(mode, Some(window), cx);
    }
}

impl Render for Preferences {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        const SCREENING: &str = "Show an screening dialog to verify the unknown sender.";
        const AVATAR: &str = "Hide all avatar pictures to improve performance.";
        const MODE: &str = "Use the selected light or dark theme, or to follow the OS.";
        const NIP4E: &str = "Use a dedicated key to encrypt and decrypt messages.";
        const RESET: &str = "Reset the theme to the default one.";

        let servers = NostrRegistry::global(cx).read(cx).media_servers().to_vec();
        let screening = AppSettings::get_screening(cx);
        let render_markdown = AppSettings::get_render_markdown(cx);
        let hide_avatar = AppSettings::get_hide_avatar(cx);
        let nip4e = AppSettings::get_nip4e(cx);
        let theme_mode = AppSettings::get_theme_mode(cx);

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
                        Switch::new("avatar")
                            .label("Hide user avatar")
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
                                    .child(div().text_sm().child(SharedString::from("Mode")))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().text_muted)
                                            .child(SharedString::from(MODE)),
                                    ),
                            )
                            .child(
                                Button::new("theme-mode")
                                    .label(theme_mode.name())
                                    .ghost_alt()
                                    .small()
                                    .dropdown_menu(|this, _window, _cx| {
                                        this.item(PopupMenuItem::new("Light").on_click(
                                            |_, window, cx| {
                                                Self::set_theme_mode(ThemeMode::Light, window, cx);
                                            },
                                        ))
                                        .item(
                                            PopupMenuItem::new("Dark").on_click(|_, window, cx| {
                                                Self::set_theme_mode(ThemeMode::Dark, window, cx);
                                            }),
                                        )
                                    }),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .justify_between()
                            .child(
                                v_flex()
                                    .child(div().text_sm().child(SharedString::from("Reset theme")))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().text_muted)
                                            .child(SharedString::from(RESET)),
                                    ),
                            )
                            .child(
                                Button::new("reset")
                                    .label("Reset")
                                    .ghost_alt()
                                    .small()
                                    .on_click(move |_ev, window, cx| {
                                        AppSettings::global(cx).update(cx, |this, cx| {
                                            this.reset_theme(window, cx);
                                        })
                                    }),
                            ),
                    ),
            )
            .child(
                GroupBox::new()
                    .id("experiments")
                    .title("Experiments")
                    .fill()
                    .child(
                        Switch::new("nip4e")
                            .label("Decoupling Encryption Key")
                            .description(NIP4E)
                            .checked(nip4e)
                            .on_click(move |_, _window, cx| {
                                AppSettings::update_nip4e(!nip4e, cx);
                            }),
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
    }
}
