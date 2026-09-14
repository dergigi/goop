use chat::{ChatEvent, ChatRegistry, RoomKind};
use common::{TimestampExt, goop_cache};
use entry::RoomEntry;
use gpui::prelude::FluentBuilder;
use gpui::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, UniformListScrollHandle, Window, div, uniform_list,
};
use state::{IMAGE_CACHE_SIZE, NostrRegistry};
use std::ops::Range;
use theme::{ActiveTheme};
use ui::button::{Button, ButtonVariants};
use ui::dock::{Panel, PanelEvent};
use ui::scroll::Scrollbar;
use ui::{IconName, Selectable, Sizable, StyledExt, h_flex, v_flex};
pub(crate) mod entry;

pub struct Sidebar {
    focus_handle: FocusHandle,
    scroll_handle: UniformListScrollHandle,
    filter: Entity<RoomKind>,
    new_requests: bool,
    _subscriptions: Vec<Subscription>,
}
impl Sidebar {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let chat = ChatRegistry::global(cx);
        let subscriptions = vec![
            cx.observe(&NostrRegistry::global(cx), |_, _, cx| cx.notify()),
            cx.observe(&chat, |_, _, cx| cx.notify()),
            cx.subscribe(&chat, |this, _, event, cx| {
                if event == &ChatEvent::Ping {
                    this.new_requests = true;
                    cx.notify();
                }
            }),
        ];
        Self {
            focus_handle: cx.focus_handle(),
            scroll_handle: UniformListScrollHandle::new(),
            filter: cx.new(|_| RoomKind::Ongoing),
            new_requests: false,
            _subscriptions: subscriptions,
        }
    }
    pub fn set_filter(&mut self, kind: RoomKind, window: &mut Window, cx: &mut Context<Self>) {
        self.filter.update(cx, |filter, _| *filter = kind);
        self.new_requests = false;
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }
    fn current_filter(&self, kind: &RoomKind, cx: &Context<Self>) -> bool {
        self.filter.read(cx) == kind
    }

    fn render_list_items(
        &self,
        range: Range<usize>,
        cx: &Context<Self>,
    ) -> Vec<impl IntoElement + use<>> {
        let chat = ChatRegistry::global(cx);
        let rooms = chat.read(cx).rooms(self.filter.read(cx), cx);

        rooms
            .get(range.clone())
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(ix, item)| {
                let room = item.read(cx);
                let room_clone = item.clone();
                let public_key = room.display_member(cx).public_key();
                let handler = cx.listener(move |_this, _ev, window, cx| {
                    ChatRegistry::global(cx).update(cx, |s, cx| {
                        s.emit_room(&room_clone, window, cx);
                    });
                });

                RoomEntry::new(range.start + ix)
                    .room_id(room.id)
                    .name(room.display_name(cx))
                    .avatar(room.display_image(cx))
                    .public_key(public_key)
                    .kind(room.kind)
                    .created_at(room.created_at.to_ago())
                    .on_click(handler)
                    .into_any_element()
            })
            .collect()
    }
}
impl Panel for Sidebar {
    fn panel_id(&self) -> SharedString {
        "Sidebar".into()
    }
}

impl EventEmitter<PanelEvent> for Sidebar {}

impl Focusable for Sidebar {
    fn focus_handle(&self, _: &App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let nostr = NostrRegistry::global(cx);
        let chat = ChatRegistry::global(cx);
        let logged_in = nostr.read(cx).current_user().is_some();
        let restoring = nostr.read(cx).identity_loading();
        let loading = restoring || (chat.read(cx).loading() && logged_in);

        let total_rooms = chat.read(cx).count(self.filter.read(cx), cx);

        v_flex()
            .track_focus(&self.focus_handle)
            .image_cache(goop_cache("sidebar", IMAGE_CACHE_SIZE))
            .size_full()
            .gap_2()
            .child(
                v_flex().px_2().gap_1().children(
                    [
                        (
                            "new-chat",
                            "New Chat",
                            IconName::Plus,
                            crate::Command::NewConversation,
                        ),
                        (
                            "search",
                            "Search",
                            IconName::Search,
                            crate::Command::SearchConversations,
                        ),
                    ]
                    .into_iter()
                    .map(|(id, label, icon, action)| {
                        let shortcut = ui::Kbd::binding_for_action(&action, None, window);
                        h_flex()
                            .id(id)
                            .w_full()
                            .h_8()
                            .px_3()
                            .gap_2()
                            .rounded(cx.theme().radius)
                            .text_sm()
                            .text_color(cx.theme().text_muted)
                            .hover(|style| {
                                style
                                    .bg(cx.theme().ghost_element_hover)
                                    .text_color(cx.theme().text)
                            })
                            .child(ui::Icon::new(icon).small())
                            .child(div().flex_1().child(label))
                            .when_some(shortcut, |row, shortcut| row.child(shortcut))
                            .on_click(move |_, window, cx| {
                                window.dispatch_action(Box::new(action.clone()), cx)
                            })
                    }),
                ),
            )
            .child(
                h_flex()
                    .px_2()
                    .gap_2()
                    .justify_center()
                    .child(
                        Button::new("all")
                            .map(|this| {
                                if self.current_filter(&RoomKind::Ongoing, cx) {
                                    this.icon(IconName::InboxFill)
                                } else {
                                    this.icon(IconName::Inbox)
                                }
                            })
                            .label("Inbox")
                            .small()
                            .tooltip("All ongoing conversations")
                            .ghost_alt()
                            .font_semibold()
                            .flex_1()
                            .selected(self.current_filter(&RoomKind::Ongoing, cx))
                            .on_click(cx.listener(|this, _ev, window, cx| {
                                this.set_filter(RoomKind::Ongoing, window, cx);
                            })),
                    )
                    .child(
                        Button::new("requests")
                            .map(|this| {
                                if self.current_filter(&RoomKind::Request, cx) {
                                    this.icon(IconName::FistbumpFill)
                                } else {
                                    this.icon(IconName::Fistbump)
                                }
                            })
                            .label("Requests")
                            .small()
                            .tooltip("Incoming new conversations")
                            .ghost_alt()
                            .font_semibold()
                            .flex_1()
                            .selected(!self.current_filter(&RoomKind::Ongoing, cx))
                            .when(self.new_requests, |this| {
                                this.child(div().size_1().rounded_full().bg(cx.theme().cursor))
                            })
                            .on_click(cx.listener(|this, _ev, window, cx| {
                                this.set_filter(RoomKind::default(), window, cx);
                            })),
                    ),
            )
            .when(!loading && total_rooms == 0, |this| {
                this.child(
                    div().w_full().px_2().child(
                        v_flex()
                            .p_3()
                            .h_24()
                            .w_full()
                            .border_2()
                            .border_dashed()
                            .border_color(cx.theme().border_variant)
                            .rounded(cx.theme().radius_lg)
                            .items_center()
                            .justify_center()
                            .text_center()
                            .child(div().text_sm().font_semibold().child("No conversations"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().text_muted)
                                    .child("Start a conversation with someone to get started."),
                            ),
                    ),
                )
            })
            .child(v_flex().size_full().flex_1().gap_1().map(|this| {
                this.child(
                    uniform_list(
                        "rooms",
                        total_rooms,
                        cx.processor(|this, range, _window, cx| this.render_list_items(range, cx)),
                    )
                    .track_scroll(&self.scroll_handle)
                    .flex_1()
                    .h_full()
                    .px_2(),
                )
                .child(Scrollbar::vertical(&self.scroll_handle))
            }))
    }
}
