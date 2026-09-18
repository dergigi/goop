use std::rc::Rc;

use chat::RoomKind;
use gpui::prelude::FluentBuilder;
use gpui::{
    App, ClickEvent, InteractiveElement, IntoElement, ParentElement as _, RenderOnce, SharedString,
    StatefulInteractiveElement, Styled, Window, div,
};
use nostr_sdk::prelude::*;
use settings::AppSettings;
use theme::ActiveTheme;
use ui::avatar::Avatar;
use ui::dock::ClosePanel;
use ui::modal::ModalButtonProps;
use ui::{Icon, IconName, Selectable, Sizable, StyledExt, WindowExtension, h_flex};

use crate::dialogs::screening;

#[derive(IntoElement)]
pub struct RoomEntry {
    ix: usize,
    actions: Option<gpui::AnyElement>,
    unread_count: usize,
    room_id: Option<u64>,
    public_key: Option<PublicKey>,
    name: Option<SharedString>,
    avatar: Option<SharedString>,
    created_at: Option<SharedString>,
    has_draft: bool,
    kind: Option<RoomKind>,
    selected: bool,
    highlighted: bool,
    #[allow(clippy::type_complexity)]
    handler: Option<Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>>,
}

impl RoomEntry {
    pub fn new(ix: usize) -> Self {
        Self {
            ix,
            actions: None,
            unread_count: 0,
            room_id: None,
            public_key: None,
            name: None,
            avatar: None,
            created_at: None,
            has_draft: false,
            kind: None,
            handler: None,
            selected: false,
            highlighted: false,
        }
    }

    pub fn actions(mut self, actions: impl IntoElement) -> Self {
        self.actions = Some(actions.into_any_element());
        self
    }

    pub fn unread_count(mut self, count: usize) -> Self {
        self.unread_count = count;
        self
    }

    pub fn room_id(mut self, id: u64) -> Self {
        self.room_id = Some(id);
        self
    }

    pub fn public_key(mut self, public_key: PublicKey) -> Self {
        self.public_key = Some(public_key);
        self
    }

    pub fn name(mut self, name: impl Into<SharedString>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn avatar(mut self, avatar: impl Into<SharedString>) -> Self {
        self.avatar = Some(avatar.into());
        self
    }

    pub fn created_at(mut self, created_at: impl Into<SharedString>) -> Self {
        self.created_at = Some(created_at.into());
        self
    }

    pub fn has_draft(mut self, has_draft: bool) -> Self {
        self.has_draft = has_draft;
        self
    }

    pub fn kind(mut self, kind: RoomKind) -> Self {
        self.kind = Some(kind);
        self
    }

    pub fn highlighted(mut self, highlighted: bool) -> Self {
        self.highlighted = highlighted;
        self
    }

    fn is_message_request(&self) -> bool {
        self.kind == Some(RoomKind::Request) && self.room_id.is_some() && self.public_key.is_some()
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.handler = Some(Rc::new(handler));
        self
    }
}

impl Selectable for RoomEntry {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl RenderOnce for RoomEntry {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let screening = AppSettings::get_screening(cx) && self.is_message_request();

        let public_key = self.public_key;
        let is_selected = self.is_selected();

        h_flex()
            .id(self.ix)
            .group("chat-row")
            .h_9()
            .w_full()
            .px_1p5()
            .gap_2()
            .text_sm()
            .rounded(cx.theme().radius)
            .when(self.highlighted, |row| row.bg(cx.theme().element_active))
            .when_some(self.avatar, |this, avatar| {
                this.child(Avatar::new(avatar).small().flex_shrink_0())
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .when_some(self.name, |this, name| {
                        this.child(
                            h_flex()
                                .flex_1()
                                .justify_between()
                                .line_clamp(1)
                                .text_ellipsis()
                                .truncate()
                                .font_medium()
                                .child(name)
                                .when(is_selected, |this| {
                                    this.child(
                                        Icon::new(IconName::CheckCircle)
                                            .small()
                                            .text_color(cx.theme().icon_accent),
                                    )
                                }),
                        )
                    })
                    .child(
                        h_flex()
                            .gap_1p5()
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(cx.theme().text_placeholder)
                            .when_some(self.actions, |this, actions| this.child(
                                h_flex().gap_1().invisible()
                                    .group_hover("chat-row", |style| style.visible())
                                    .child(actions)))
                            .when_some(self.created_at, |this, created_at| this.child(
                                h_flex().gap_1p5().flex_shrink_0()
                                    // Fixed slots keep the time and hover actions aligned,
                                    // regardless of unread count or timestamp length.
                                    .child(h_flex().w_6().flex_shrink_0().justify_center()
                                        .when(self.unread_count > 0, |slot| slot.child(
                                            h_flex().h_4().min_w_4().px_0p5().rounded_full()
                                                .justify_center().items_center()
                                                .bg(cx.theme().cursor).text_color(gpui::white())
                                                .text_xs().font_semibold()
                                                .child(if self.unread_count > 99 { "99+".to_owned() }
                                                    else { self.unread_count.to_string() })
                                        )))
                                    .child(h_flex().w_10().flex_shrink_0().justify_end()
                                        .when(self.has_draft, |slot| slot.child(
                                            div().id("draft-indicator").flex().items_center()
                                                .child(Icon::new(IconName::Pencil).xsmall())
                                                .tooltip(|window, cx| ui::tooltip::Tooltip::new("Draft message", window, cx).into())
                                        ))
                                        .when(!self.has_draft, |slot| slot.child(created_at))))),
                    ),
            )
            .hover(|this| this.bg(cx.theme().elevated_surface_background))
            .when_some(self.handler, |this, handler| {
                this.on_click(move |event, window, cx| {
                    handler(event, window, cx);

                    if let Some(public_key) = public_key
                        && screening
                    {
                        let room_id = self.room_id;
                        let screening = screening::init(public_key, window, cx);

                        window.open_modal(cx, move |this, _window, _cx| {
                            this.confirm()
                                .title("Message request")
                                .child("This person wants to start a conversation with you.")
                                .child(screening.clone())
                                .button_props(
                                    ModalButtonProps::default()
                                        .cancel_text("Ignore")
                                        .ok_text("Accept"),
                                )
                                .on_ok(move |_, _, cx| {
                                    room_id.is_some_and(|id| {
                                        chat::ChatRegistry::global(cx)
                                            .update(cx, |chat, cx| chat.accept_room(id, cx))
                                    })
                                })
                                .on_cancel(move |_event, window, cx| {
                                    window.dispatch_action(Box::new(ClosePanel), cx);
                                    true
                                })
                        });
                    }
                })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_explicit_message_requests_are_screened() {
        let key = Keys::generate().public_key();
        assert!(!RoomEntry::new(0).public_key(key).is_message_request());
        assert!(
            !RoomEntry::new(0)
                .public_key(key)
                .room_id(1)
                .kind(RoomKind::Ongoing)
                .is_message_request()
        );
        assert!(
            !RoomEntry::new(0)
                .public_key(key)
                .kind(RoomKind::Request)
                .is_message_request()
        );
        assert!(
            RoomEntry::new(0)
                .public_key(key)
                .room_id(1)
                .kind(RoomKind::Request)
                .is_message_request()
        );
    }
}
