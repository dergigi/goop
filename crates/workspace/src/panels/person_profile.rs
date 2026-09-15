use crate::dialogs::screening::{self, Screening};
use chat::ChatRegistry;
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable,
    IntoElement, ParentElement, Render, SharedString, Styled, Window,
};
use nostr_sdk::prelude::PublicKey;
use state::NostrRegistry;
use theme::ActiveTheme;
use ui::button::{Button, ButtonVariants};
use ui::dock::{Panel, PanelEvent};
use ui::scroll::ScrollableElement;
use ui::{IconName, Sizable, StyledExt, h_flex, v_flex};

pub fn init(key: PublicKey, window: &mut Window, cx: &mut App) -> Entity<PersonProfile> {
    let details = screening::init(key, window, cx);
    details.update(cx, |details, _| details.show_report = false);
    cx.new(|cx| PersonProfile {
        key,
        details,
        focus_handle: cx.focus_handle(),
        _subscription: cx.observe(&ChatRegistry::global(cx), |_, _, cx| cx.notify()),
    })
}

pub struct PersonProfile {
    _subscription: gpui::Subscription,
    key: PublicKey,
    details: Entity<Screening>,
    focus_handle: FocusHandle,
}

impl Panel for PersonProfile {
    fn panel_id(&self) -> SharedString {
        format!("profile-{}", self.key.to_hex()).into()
    }
    fn title(&self, _: &App) -> AnyElement {
        "Profile".into_any_element()
    }
}
impl EventEmitter<PanelEvent> for PersonProfile {}
impl Focusable for PersonProfile {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
impl Render for PersonProfile {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .min_h_0()
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .p_4()
                    .gap_6()
                    .overflow_y_scrollbar()
                    .child(self.details.clone())
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .justify_center()
                            .gap_6()
                            .pb_2()
                            .children(
                                [
                                    ("Chat", IconName::Chat, false),
                                    ("Search", IconName::Search, true),
                                ]
                                .into_iter()
                                .map(|(label, icon, search)| {
                                    v_flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            Button::new(label)
                                                .icon(icon)
                                                .large()
                                                .ghost()
                                                .w(gpui::px(56.))
                                                .h(gpui::px(44.))
                                                .rounded_full()
                                                .bg(cx.theme().elevated_surface_background)
                                                .tooltip(if search {
                                                    "Find in this chat"
                                                } else {
                                                    "Open chat"
                                                })
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        window.dispatch_action(
                                                            Box::new(
                                                                crate::Command::OpenProfileChat(
                                                                    this.key, search,
                                                                ),
                                                            ),
                                                            cx,
                                                        );
                                                    },
                                                )),
                                        )
                                        .child(gpui::div().text_sm().font_semibold().child(label))
                                }),
                            ),
                    ),
            )
            .when(
                NostrRegistry::global(cx)
                    .read(cx)
                    .current_user()
                    .is_some_and(|owner| owner != self.key),
                |view| {
                    let chat = ChatRegistry::global(cx).read(cx);
                    let muted = chat.is_muted(self.key);
                    let blocked = chat.is_blocked(self.key);
                    view.child(
                        h_flex()
                            .flex_shrink_0()
                            .justify_center()
                            .gap_6()
                            .p_4()
                            .children(
                                [
                                    (if muted { "Unmute" } else { "Mute" }, IconName::Mute, 0),
                                    (
                                        if blocked { "Unblock" } else { "Block" },
                                        IconName::Block,
                                        1,
                                    ),
                                    ("Report", IconName::Flag, 2),
                                ]
                                .into_iter()
                                .map(|(label, icon, action)| {
                                    v_flex()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            Button::new(label)
                                                .icon(icon)
                                                .large()
                                                .danger()
                                                .w(gpui::px(56.))
                                                .h(gpui::px(44.))
                                                .rounded_full()
                                                .tooltip(label)
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| match action {
                                                        0 => crate::dialogs::moderation::mute(
                                                            this.key, window, cx,
                                                        ),
                                                        1 => crate::dialogs::moderation::block(
                                                            this.key, window, cx,
                                                        ),
                                                        _ => this.details.update(
                                                            cx,
                                                            |details, cx| {
                                                                details.confirm_report(window, cx)
                                                            },
                                                        ),
                                                    },
                                                )),
                                        )
                                        .child(
                                            gpui::div()
                                                .text_sm()
                                                .font_semibold()
                                                .text_color(cx.theme().text_danger)
                                                .child(label),
                                        )
                                }),
                            ),
                    )
                },
            )
    }
}
