use anyhow::Error;
use gpui::{
    AnyElement, App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable,
    IntoElement, ParentElement, Render, SharedString, Styled, Task, Window, div, svg,
};
use state::NostrRegistry;
use theme::ActiveTheme;
use ui::button::{Button, ButtonVariants};
use ui::dock::{DockPlacement, Panel, PanelEvent};
use ui::{Icon, IconName, Sizable, StyledExt, h_flex, v_flex};

use crate::panels::profile;
use crate::{Command, Workspace};

pub fn init(window: &mut Window, cx: &mut App) -> Entity<GreeterPanel> {
    cx.new(|cx| GreeterPanel::new(window, cx))
}

pub struct GreeterPanel {
    name: SharedString,
    focus_handle: FocusHandle,
    tasks: Vec<Task<Result<(), Error>>>,
}

impl GreeterPanel {
    fn new(_window: &mut Window, cx: &mut App) -> Self {
        Self {
            name: "Onboarding".into(),
            focus_handle: cx.focus_handle(),
            tasks: vec![],
        }
    }

    fn add_profile_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let nostr = NostrRegistry::global(cx);

        if let Some(public_key) = nostr.read(cx).current_user() {
            self.tasks.push(cx.spawn_in(window, async move |_this, cx| {
                cx.update(|window, cx| {
                    Workspace::add_panel(
                        profile::init(public_key, window, cx),
                        DockPlacement::Right,
                        window,
                        cx,
                    );
                })
                .ok();

                Ok(())
            }));
        }
    }
}

impl Panel for GreeterPanel {
    fn panel_id(&self) -> SharedString {
        self.name.clone()
    }

    fn title(&self, cx: &App) -> AnyElement {
        div()
            .child(
                svg()
                    .path("brand/goop.svg")
                    .size_4()
                    .text_color(cx.theme().text_muted),
            )
            .into_any_element()
    }
}

impl EventEmitter<PanelEvent> for GreeterPanel {}

impl Focusable for GreeterPanel {
    fn focus_handle(&self, _: &App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for GreeterPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        const TITLE: &str = "Welcome to Goop!";
        const DESCRIPTION: &str = "Chat Freely, Stay Private on Nostr.";

        h_flex()
            .size_full()
            .items_center()
            .justify_center()
            .p_2()
            .child(
                v_flex()
                    .h_full()
                    .w_112()
                    .gap_6()
                    .items_center()
                    .justify_center()
                    .child(
                        h_flex()
                            .mb_4()
                            .gap_2()
                            .w_full()
                            .child(
                                svg()
                                    .path("brand/goop.svg")
                                    .size_12()
                                    .text_color(cx.theme().icon_muted),
                            )
                            .child(
                                v_flex()
                                    .child(
                                        div()
                                            .font_semibold()
                                            .text_color(cx.theme().text)
                                            .child(SharedString::from(TITLE)),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().text_muted)
                                            .child(SharedString::from(DESCRIPTION)),
                                    ),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .w_full()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .w_full()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().text_muted)
                                    .child(SharedString::from("Get Started"))
                                    .child(div().flex_1().h_px().bg(cx.theme().border)),
                            )
                            .child(
                                v_flex()
                                    .gap_2()
                                    .w_full()
                                    .child(
                                        Button::new("profile")
                                            .icon(Icon::new(IconName::Profile))
                                            .label("Update profile")
                                            .ghost()
                                            .small()
                                            .justify_start()
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                this.add_profile_panel(window, cx)
                                            })),
                                    )
                                    .child(
                                        Button::new("theme")
                                            .icon(Icon::new(IconName::Moon))
                                            .label("Change theme")
                                            .ghost()
                                            .small()
                                            .justify_start()
                                            .on_click(cx.listener(move |_, _, _, cx| {
                                                cx.dispatch_action(&Command::ToggleTheme);
                                            })),
                                    ),
                            ),
                    ),
            )
    }
}
