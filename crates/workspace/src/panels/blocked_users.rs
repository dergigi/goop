use chat::ChatRegistry;
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable,
    IntoElement, ParentElement, Render, SharedString, Styled, Subscription, Window, div,
};
use person::PersonRegistry;
use nostr_sdk::prelude::ToBech32;
use theme::ActiveTheme;
use ui::avatar::Avatar;
use ui::button::{Button, ButtonVariants};
use ui::dock::{Panel, PanelEvent};
use ui::scroll::ScrollableElement;
use ui::{IconName, Sizable, h_flex, v_flex};

pub fn init(cx: &mut App) -> Entity<BlockedUsers> {
    cx.new(|cx| BlockedUsers {
        focus: cx.focus_handle(),
        _subscriptions: vec![
            cx.observe(&ChatRegistry::global(cx), |_, _, cx| cx.notify()),
            cx.observe(&PersonRegistry::global(cx), |_, _, cx| cx.notify()),
        ],
    })
}
pub struct BlockedUsers {
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}
impl Panel for BlockedUsers {
    fn panel_id(&self) -> SharedString {
        "blocked-users".into()
    }
    fn title(&self, _: &App) -> AnyElement {
        "Blocked users".into_any_element()
    }
}
impl EventEmitter<PanelEvent> for BlockedUsers {}
impl Focusable for BlockedUsers {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for BlockedUsers {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chat = ChatRegistry::global(cx).read(cx);
        let keys = chat.blocked_users();
        let error = chat.moderation_error.clone();
        let pending = chat.moderation_pending();
        v_flex().size_full().min_h_0().p_2().gap_1().overflow_y_scrollbar()
            .child(div().px_2().py_2().text_sm().text_color(cx.theme().text_muted).child("Blocked users"))
            .when(pending, |view| view.child(div().px_2().text_xs().text_color(cx.theme().text_muted).child("Waiting to sync…")))
            .when_some(error, |view, error| view.child(v_flex().gap_2()
                .child(div().text_sm().text_color(cx.theme().text_danger).child(error.clone()))
                .child(h_flex().gap_2()
                    .child(Button::new("retry-blocks").label("Retry sync").small().ghost().on_click(|_,_,cx| ChatRegistry::global(cx).update(cx,|chat,cx| chat.retry_moderation(cx))))
                    .child(Button::new("copy-block-error").icon(IconName::Copy).small().ghost().tooltip("Copy error").on_click(move |_,_,cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(error.clone())))))))
            .when(keys.is_empty(), |view| view.child(div().px_2().text_sm().text_color(cx.theme().text_muted).child("No blocked users.")))
            .children(keys.into_iter().map(|key| {
                let profile = PersonRegistry::global(cx).read(cx).get(&key,cx);
                h_flex().min_w_0().w_full().gap_1()
                    .child(Button::new(format!("profile-{key}"))
                        .flex_1().min_w_0().truncate_label().small().ghost()
                        .tooltip(key.to_bech32().unwrap())
                        .child(h_flex().min_w_0().gap_2()
                            .child(Avatar::new(profile.avatar()).small())
                            .child(div().min_w_0().truncate().child(profile.name())))
                        .on_click(move |_,window,cx| window.dispatch_action(Box::new(crate::Command::OpenProfile(key)),cx)))
                    .child(Button::new(format!("unblock-{key}")).label("Unblock").small().ghost()
                        .on_click(move |_,window,cx| crate::dialogs::moderation::block(key,window,cx)))
            }))
    }
}
