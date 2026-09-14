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
    room_id: Option<u64>,
    public_key: Option<PublicKey>,
    name: Option<SharedString>,
    avatar: Option<SharedString>,
    created_at: Option<SharedString>,
    kind: Option<RoomKind>,
    selected: bool,
    #[allow(clippy::type_complexity)]
    handler: Option<Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>>,
}

impl RoomEntry {
    pub fn new(ix: usize) -> Self {
        Self {
            ix,
            room_id: None,
            public_key: None,
            name: None,
            avatar: None,
            created_at: None,
            kind: None,
            handler: None,
            selected: false,
        }
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

    pub fn kind(mut self, kind: RoomKind) -> Self {
        self.kind = Some(kind);
        self
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
        let hide_avatar = AppSettings::get_hide_avatar(cx);
        let screening = AppSettings::get_screening(cx);

        let public_key = self.public_key;
        let is_selected = self.is_selected();

        h_flex()
            .id(self.ix)
            .h_9()
            .w_full()
            .px_1p5()
            .gap_2()
            .text_sm()
            .rounded(cx.theme().radius)
            .when(!hide_avatar, |this| {
                this.when_some(self.avatar, |this, avatar| {
                    this.child(Avatar::new(avatar).small().flex_shrink_0())
                })
            })
            .child(
                div()
                    .flex_1()
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
                            .when_some(self.created_at, |this, created_at| this.child(created_at)),
                    ),
            )
            .hover(|this| this.bg(cx.theme().elevated_surface_background))
            .when_some(self.handler, |this, handler| {
                this.on_click(move |event, window, cx| {
                    handler(event, window, cx);

                    if let Some(public_key) = public_key
                        && self.kind != Some(RoomKind::Ongoing)
                        && screening
                    {
                        let room_id = self.room_id;
                        let screening = screening::init(public_key, window, cx);

                        window.open_modal(cx, move |this, _window, _cx| {
                            this.confirm()
                                .title("Message request")
                                .child(screening.clone())
                                .button_props(
                                    ModalButtonProps::default()
                                        .cancel_text("Ignore")
                                        .ok_text("Accept"),
                                )
                                .on_ok(move |_, _, cx| {
                                    room_id.is_some_and(|id| chat::ChatRegistry::global(cx)
                                        .update(cx, |chat, cx| chat.accept_room(id, cx)))
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
