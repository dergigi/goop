use crate::Command;
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, div, svg,
};
use theme::ActiveTheme;
use ui::dock::{Panel, PanelEvent};
use ui::{Icon, IconName, Kbd, Sizable, StyledExt, h_flex, v_flex};

pub fn init(_window: &mut Window, cx: &mut App) -> Entity<GreeterPanel> {
    cx.new(|cx| GreeterPanel {
        focus_handle: cx.focus_handle(),
    })
}

pub struct GreeterPanel {
    focus_handle: FocusHandle,
}

impl Panel for GreeterPanel {
    fn panel_id(&self) -> SharedString {
        "Onboarding".into()
    }
    fn title(&self, cx: &App) -> AnyElement {
        div()
            .text_color(cx.theme().text_muted)
            .child("Welcome")
            .into_any_element()
    }
}
impl EventEmitter<PanelEvent> for GreeterPanel {}
impl Focusable for GreeterPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
impl Render for GreeterPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .p_4()
            .child(
                v_flex()
                    .w_full()
                    .max_w(gpui::px(420.))
                    .gap_6()
                    .child(
                        h_flex()
                            .gap_3()
                            .child(
                                svg()
                                    .path("brand/goop.svg")
                                    .size_12()
                                    .text_color(cx.theme().icon_muted),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .child(div().font_semibold().child("Welcome to Goop"))
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(cx.theme().text_muted)
                                            .child("A simple NIP-17 client that just works."),
                                    ),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .text_sm()
                                    .text_color(cx.theme().text_muted)
                                    .child("Get Started")
                                    .child(div().flex_1().h_px().bg(cx.theme().border)),
                            )
                            .children(
                                [
                                    (
                                        "set-up-agents",
                                        "Set up your agents to message you",
                                        IconName::Robot,
                                        Command::SetUpAgents,
                                    ),
                                    (
                                        "new-chat",
                                        "Start a new chat",
                                        IconName::Plus,
                                        Command::NewConversation,
                                    ),
                                    (
                                        "search",
                                        "Search past conversations",
                                        IconName::Search,
                                        Command::SearchConversations,
                                    ),
                                    (
                                        "shortcuts",
                                        "Keyboard Shortcuts",
                                        IconName::Keyboard,
                                        Command::KeyboardShortcuts,
                                    ),
                                ]
                                .into_iter()
                                .map(
                                    |(id, label, icon, action)| {
                                        h_flex()
                                            .id(id)
                                            .w_full()
                                            .h_9()
                                            .px_3()
                                            .gap_3()
                                            .rounded(cx.theme().radius)
                                            .hover(|row| row.bg(cx.theme().ghost_element_hover))
                                            .child(Icon::new(icon).small())
                                            .child(div().flex_1().text_sm().child(label))
                                            .when_some(
                                                Kbd::binding_for_action(
                                                    &action,
                                                    Some("Workspace"),
                                                    window,
                                                ),
                                                |row, keys| row.child(keys),
                                            )
                                            .on_click(move |_, window, cx| {
                                                window.dispatch_action(Box::new(action.clone()), cx)
                                            })
                                    },
                                ),
                            ),
                    ),
            )
    }
}
