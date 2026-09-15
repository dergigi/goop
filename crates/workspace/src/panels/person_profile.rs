use gpui::{AnyElement, App, AppContext, Context, Entity, EventEmitter, FocusHandle,
    Focusable, IntoElement, ParentElement, Render, SharedString, Styled, Window};
use nostr_sdk::prelude::PublicKey;
use ui::dock::{Panel, PanelEvent};
use ui::scroll::ScrollableElement;
use ui::{v_flex, h_flex, IconName, Sizable, StyledExt};
use ui::button::{Button, ButtonVariants};
use theme::ActiveTheme;
use crate::dialogs::screening::{self, Screening};

pub fn init(key: PublicKey, window: &mut Window, cx: &mut App) -> Entity<PersonProfile> {
    let details = screening::init(key, window, cx);
    cx.new(|cx| PersonProfile { key, details, focus_handle: cx.focus_handle() })
}

pub struct PersonProfile {
    key: PublicKey,
    details: Entity<Screening>,
    focus_handle: FocusHandle,
}

impl Panel for PersonProfile {
    fn panel_id(&self) -> SharedString { format!("profile-{}", self.key.to_hex()).into() }
    fn title(&self, _: &App) -> AnyElement { "Profile".into_any_element() }
}
impl EventEmitter<PanelEvent> for PersonProfile {}
impl Focusable for PersonProfile {
    fn focus_handle(&self, _: &App) -> FocusHandle { self.focus_handle.clone() }
}
impl Render for PersonProfile {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex().size_full().p_4().gap_6().overflow_y_scrollbar()
            .child(self.details.clone())
            .child(h_flex().justify_center().gap_6().pb_2().children(
                [("Chat", IconName::Chat, false), ("Search", IconName::Search, true)]
                    .into_iter().map(|(label, icon, search)| {
                        v_flex().items_center().gap_2()
                            .child(Button::new(label).icon(icon).large().ghost()
                                .w(gpui::px(56.)).h(gpui::px(44.)).rounded_full()
                                .bg(cx.theme().elevated_surface_background)
                                .tooltip(if search { "Find in this chat" } else { "Open chat" })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    window.dispatch_action(Box::new(crate::Command::OpenProfileChat(this.key, search)), cx);
                                })))
                            .child(gpui::div().text_sm().font_semibold().child(label))
                    })
            ))
    }
}
