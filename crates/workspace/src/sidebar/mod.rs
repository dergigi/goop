use chat::{ChatRegistry, Room, RoomKind};
use chrono::{DateTime, Local, NaiveDate};
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
use ui::menu::{DropdownMenu, ContextMenuExt};
use ui::{WindowExtension, IconName, Selectable, Sizable, StyledExt, h_flex, v_flex};
pub(crate) mod entry;
mod list_filter;
use list_filter::ChatFilter;

#[derive(gpui::Action, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[action(namespace = sidebar, no_json)]
pub(crate) enum ChatAction { MarkAllRead, SetRead(u64, bool), Pin(u64, bool), Archive(u64, bool), Leave(u64, bool) }

#[derive(Clone, Copy, PartialEq, Eq)]
enum DateGroup { Inbox, Requests, Archive, Pinned, Today, Yesterday, LastWeek, Older }
impl DateGroup {
    fn label(self) -> &'static str {
        match self { Self::Inbox => "Inbox", Self::Requests => "Requests", Self::Archive => "Archive", Self::Pinned => "Pinned", Self::Today => "Today", Self::Yesterday => "Yesterday",
            Self::LastWeek => "Last 7 Days", Self::Older => "Older" }
    }
}
fn date_group(date: NaiveDate, today: NaiveDate) -> DateGroup {
    match today.signed_duration_since(date).num_days() {
        ..=0 => DateGroup::Today,
        1 => DateGroup::Yesterday,
        2..=6 => DateGroup::LastWeek,
        _ => DateGroup::Older,
    }
}
enum SidebarRow { Heading(DateGroup), Chat(Entity<Room>), Empty }

#[derive(Default)]
struct RequestBadge {
    owner: Option<nostr_sdk::prelude::PublicKey>,
    seen: std::collections::HashSet<u64>,
}
impl RequestBadge {
    fn update(&mut self, owner: Option<nostr_sdk::prelude::PublicKey>, requests: &[u64], viewing: bool) -> bool {
        if self.owner != owner { self.owner = owner; self.seen.clear(); }
        if owner.is_none() { return false; }
        if viewing { self.seen.extend(requests.iter().copied()); }
        requests.iter().any(|id| !self.seen.contains(id))
    }
}

pub struct Sidebar {
    focus_handle: FocusHandle,
    scroll_handle: UniformListScrollHandle,
    filter: Entity<RoomKind>,
    draft_indicators: Entity<chat_ui::DraftIndicators>,
    show_blocked: bool,
    list_filter: ChatFilter,
    active_room: Option<u64>,
    blocked_users: Entity<crate::panels::blocked_users::BlockedUsers>,
    request_badge: RequestBadge,
    _subscriptions: Vec<Subscription>,
    _date_refresh: gpui::Task<()>,
}
impl Sidebar {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let draft_indicators = chat_ui::DraftIndicators::global(cx);
        Self::load_draft_indicators(cx);
        let chat = ChatRegistry::global(cx);
        let subscriptions = vec![
            cx.observe(&NostrRegistry::global(cx), |_, _, cx| {
                Self::load_draft_indicators(cx);
                cx.notify();
            }),
            cx.observe(&draft_indicators, |_, _, cx| cx.notify()),
            cx.observe(&chat, |_, _, cx| cx.notify()),
            cx.observe(&person::PersonRegistry::global(cx), |_, _, cx| cx.notify()),
        ];
        Self {
            focus_handle: cx.focus_handle(),
            scroll_handle: UniformListScrollHandle::new(),
            draft_indicators,
            filter: cx.new(|_| RoomKind::Ongoing),
            show_blocked: false,
            list_filter: ChatFilter::All,
            active_room: None,
            blocked_users: crate::panels::blocked_users::init(cx),
            request_badge: RequestBadge::default(),
            _subscriptions: subscriptions,
            _date_refresh: cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(std::time::Duration::from_secs(60)).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() { break; }
                }
            }),
        }
    }
    fn load_draft_indicators(cx: &mut App) {
        if let Some(owner) = NostrRegistry::global(cx).read(cx).current_user() {
            chat_ui::DraftIndicators::global(cx).update(cx, |drafts, cx| drafts.load_account(owner, cx));
        }
    }
    pub fn set_active_room(&mut self, room: Option<u64>, cx: &mut Context<Self>) {
        if self.active_room != room {
            self.active_room = room;
            cx.notify();
        }
    }

    pub fn set_filter(&mut self, kind: RoomKind, window: &mut Window, cx: &mut Context<Self>) {
        self.show_blocked = false;
        self.filter.update(cx, |filter, _| *filter = kind);
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }
    pub fn show_blocked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_blocked = true;
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }
    fn current_filter(&self, kind: &RoomKind, cx: &Context<Self>) -> bool {
        !self.show_blocked && self.filter.read(cx) == kind
    }

    pub(crate) fn chat_action(&mut self, action: &ChatAction, window: &mut Window, cx: &mut Context<Self>) {
        let action = action.clone();
        let visible_rooms: Vec<_> = self.filtered_rooms(self.filter.read(cx), cx).iter()
            .map(|room| room.read(cx).id).collect();
        let apply = move |window: &mut Window, cx: &mut App| {
            let result = ChatRegistry::global(cx).update(cx, |chat, cx| match action {
                ChatAction::SetRead(id, value) => chat.set_room_read(id, value, cx),
                ChatAction::MarkAllRead => chat.mark_rooms_read(&visible_rooms, cx),
                ChatAction::Pin(id, value) => chat.set_pinned(id, value, cx),
                ChatAction::Archive(id, value) => chat.set_archived(id, value, cx),
                ChatAction::Leave(id, value) => chat.leave_locally(id, value, cx),
            });
            if let Err(error) = result { window.push_notification(ui::notification::Notification::error(error.to_string()), cx); }
        };
        if matches!(action, ChatAction::Leave(_, true)) {
            window.open_modal(cx, move |modal, _, _| {
                let apply = apply.clone();
                modal.confirm().title("Leave this group locally?")
                    .button_props(ui::modal::ModalButtonProps::default().ok_text("Leave locally").cancel_text("Cancel"))
                    .child("Goop will hide this group and silence its notifications on this device. Other participants can still send messages. History is kept; use Rejoin in Archived to return.")
                    .on_ok(move |_, window, cx| { apply(window, cx); true })
            });
        } else { apply(window, cx); }
    }

    fn render_footer(&self, cx: &Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .flex_shrink_0()
            .border_t_1()
            .border_color(cx.theme().border)
            .p_2()
            .gap_1()
            .justify_between()
            .child(
                Button::new("new-group")
                    .icon(IconName::Group)
                    .label("New Group")
                    .small()
                    .ghost()
                    .tooltip("New Group")
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(crate::Command::NewGroup), cx)
                    }),
            )
            .child(
                h_flex().gap_1().flex_shrink_0()
                    .child(
                        Button::new("sidebar-archive")
                            .icon(IconName::Archive).small().ghost().tooltip("Archived chats")
                            .selected(self.current_filter(&RoomKind::Archived, cx))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.set_filter(RoomKind::Archived, window, cx)
                            })),
                    )
                    .child(Button::new("sidebar-blocked").icon(IconName::Block).small().ghost().tooltip("Blocked users").selected(self.show_blocked)
                        .on_click(cx.listener(|this,_,window,cx| { this.focus_handle.focus(window,cx); window.dispatch_action(Box::new(crate::Command::ShowBlockedUsers),cx); })))
                    .child(
                        Button::new("sidebar-help")
                            .icon(IconName::Help).small().ghost().tooltip("Help")
                            .dropdown_menu_with_anchor(gpui::Anchor::BottomRight, |menu, _, _| {
                                menu.menu("Usage Guide", Box::new(crate::Command::UsageGuide))
                                    .menu("Agent Guide", Box::new(crate::Command::SetUpAgents))
                                    .separator()
                                    .menu("Keyboard Shortcuts", Box::new(crate::Command::KeyboardShortcuts))
                            }),
                    )
                    .child(
                        Button::new("sidebar-settings")
                            .icon(IconName::Settings).small().ghost().tooltip("Settings")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.focus_handle.focus(window, cx);
                                window.dispatch_action(Box::new(crate::Command::ShowSettings), cx)
                            })),
                    ),
            )
    }

    fn filtered_rooms(&self, kind: &RoomKind, cx: &App) -> Vec<Entity<Room>> {
        ChatRegistry::global(cx).read(cx).rooms(kind, cx).into_iter()
            .filter(|room| {
                if self.list_filter == ChatFilter::All { return true; }
                let room = room.read(cx);
                let has_draft = NostrRegistry::global(cx).read(cx).current_user()
                    .is_some_and(|owner| self.draft_indicators.read(cx).has_draft(owner, room.id));
                self.list_filter.matches(room.is_group(), room.display_member(cx).self_identifies_as_bot(), has_draft)
            }).collect()
    }

    fn toggle_list_filter(&mut self, list_filter: ChatFilter, window: &mut Window, cx: &mut Context<Self>) {
        self.list_filter = self.list_filter.toggle(list_filter);
        self.scroll_handle.scroll_to_item(0, gpui::ScrollStrategy::Top);
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    fn grouped_rows(&self, cx: &Context<Self>) -> Vec<SidebarRow> {
        let chat = ChatRegistry::global(cx);
        let today = Local::now().date_naive();
        let mut rows = Vec::new();
        let mut previous = None;
        let mut rooms = self.filtered_rooms(self.filter.read(cx), cx);
        if rooms.is_empty() {
            let heading = match self.filter.read(cx) {
                RoomKind::Archived => DateGroup::Archive,
                RoomKind::Request => DateGroup::Requests,
                _ => DateGroup::Inbox,
            };
            rows.push(SidebarRow::Heading(heading));
            if !chat.read(cx).loading() && !NostrRegistry::global(cx).read(cx).identity_loading() {
                rows.push(SidebarRow::Empty);
            }
            return rows;
        }
        if self.current_filter(&RoomKind::Archived, cx) {
            if !rooms.is_empty() { rows.push(SidebarRow::Heading(DateGroup::Archive)); }
            rows.extend(rooms.into_iter().map(SidebarRow::Chat));
            return rows;
        }
        rooms.sort_by_key(|room| !chat.read(cx).is_pinned(room.read(cx)));
        for room in rooms {
            let date = i64::try_from(room.read(cx).created_at.as_secs()).ok()
                .and_then(|secs| DateTime::from_timestamp(secs, 0))
                .map(|date| date.with_timezone(&Local).date_naive());
            let group = if chat.read(cx).is_pinned(room.read(cx)) { DateGroup::Pinned }
                else { date.map(|date| date_group(date, today)).unwrap_or(DateGroup::Older) };
            if previous != Some(group) { rows.push(SidebarRow::Heading(group)); previous = Some(group); }
            rows.push(SidebarRow::Chat(room));
        }
        rows
    }

    fn render_list_items(
        &self,
        range: Range<usize>,
        cx: &Context<Self>,
    ) -> Vec<impl IntoElement + use<>> {
        let rows = self.grouped_rows(cx);

        rows
            .get(range.clone())
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(ix, row)| {
                let item = match row {
                    SidebarRow::Heading(group) => {
                        let focus_handle = self.focus_handle.clone();
                        return h_flex().h_9().w_full().px_2().justify_between()
                            .text_sm().text_color(cx.theme().text_muted)
                            .child(group.label())
                            .when(range.start + ix == 0, |heading| heading.child(h_flex().gap_0p5()
                                .when(NostrRegistry::global(cx).read(cx).current_user()
                                    .is_some_and(|owner| self.draft_indicators.read(cx).has_drafts_outside(owner, self.active_room)), |filters| filters.child(
                                    Button::new("filter-drafts").icon(IconName::Pencil)
                                        .xsmall().ghost().selected(self.list_filter == ChatFilter::Drafts)
                                        .tooltip(if self.list_filter == ChatFilter::Drafts { "Show all chats" } else { "Show drafts" })
                                        .on_click(cx.listener(|this, _, window, cx| this.toggle_list_filter(ChatFilter::Drafts, window, cx)))
                                ))
                                .child(Button::new("filter-bots").icon(IconName::Robot)
                                    .xsmall().ghost().selected(self.list_filter == ChatFilter::Bots)
                                    .tooltip(if self.list_filter == ChatFilter::Bots { "Show all chats" } else { "Show bots" })
                                    .on_click(cx.listener(|this, _, window, cx| this.toggle_list_filter(ChatFilter::Bots, window, cx))))
                                .child(Button::new("filter-people").icon(IconName::User)
                                    .xsmall().ghost().selected(self.list_filter == ChatFilter::People)
                                    .tooltip(if self.list_filter == ChatFilter::People { "Show all chats" } else { "Show non-bots" })
                                    .on_click(cx.listener(|this, _, window, cx| this.toggle_list_filter(ChatFilter::People, window, cx))))
                                .child(Button::new("chat-list-menu").icon(IconName::EllipsisVertical)
                                    .xsmall().ghost().tooltip("Chat list actions")
                                    .dropdown_menu(move |menu, _, _| {
                                        menu.action_context(focus_handle.clone())
                                            .menu("Mark all as read", Box::new(ChatAction::MarkAllRead))
                                    })
                            ))).into_any_element();
                    },
                    SidebarRow::Chat(item) => item,
                    SidebarRow::Empty => return h_flex().h_9().w_full().px_2()
                        .text_sm().text_color(cx.theme().text_muted)
                        .child(self.list_filter.empty_message()).into_any_element(),
                };
                let room = item.read(cx);
                let room_clone = item.clone();
                let public_key = room.display_member(cx).public_key();
                let handler = cx.listener(move |_this, _ev, window, cx| {
                    ChatRegistry::global(cx).update(cx, |s, cx| {
                        s.emit_room(&room_clone, window, cx);
                    });
                });

                let id = room.id;
                let archived = chat::ChatRegistry::global(cx).read(cx).is_archived(room);
                let left = chat::ChatRegistry::global(cx).read(cx).has_left(room);
                let group = room.is_group();
                let pinned = ChatRegistry::global(cx).read(cx).is_pinned(room);
                let entry = RoomEntry::new(range.start + ix)
                    .room_id(room.id)
                    .unread_count(ChatRegistry::global(cx).read(cx).unread_count(room.id))
                    .name(room.display_name(cx))
                    .avatar(room.display_image(cx))
                    .public_key(public_key)
                    .kind(room.kind)
                    .created_at(room.created_at.to_ago())
                    .has_draft(NostrRegistry::global(cx).read(cx).current_user()
                        .is_some_and(|owner| self.draft_indicators.read(cx).has_draft(owner, room.id)))
                    .on_click(handler);
                let actions = h_flex().gap_1()
                    .child(Button::new(("pin-chat", id)).icon(IconName::Pin).xsmall().ghost()
                        .selected(pinned).tooltip(if pinned { "Unpin chat" } else { "Pin chat" })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.chat_action(&ChatAction::Pin(id, !pinned), window, cx);
                        })))
                    .when(!left, |row| row.child(Button::new(("archive-chat", id))
                        .icon(IconName::Archive).xsmall().ghost()
                        .tooltip(if archived { "Unarchive chat" } else { "Archive chat" })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.chat_action(&ChatAction::Archive(id, !archived), window, cx);
                        }))));
                let entry = entry.actions(actions);
                let unread = ChatRegistry::global(cx).read(cx).has_unread(id);
                let focus_handle = self.focus_handle.clone();
                div().child(entry).context_menu_with_id(("chat-context-menu", id), move |menu, _, cx| {
                    let owner = NostrRegistry::global(cx).read(cx).current_user();
                    let muted = ChatRegistry::global(cx).read(cx).is_muted(public_key);
                    let blocked = ChatRegistry::global(cx).read(cx).is_blocked(public_key);
                    menu.action_context(focus_handle.clone())
                        .when(!group, |menu| menu
                            .item(ui::menu::PopupMenuItem::new("View profile")
                                .on_click(move |_, window, cx| {
                                    // Open after dismissal restores focus to the sidebar.
                                    window.defer(cx, move |window, cx| {
                                        let profile = crate::panels::person_profile::init(public_key, window, cx);
                                        crate::Workspace::add_panel(profile, ui::dock::DockPlacement::Right, window, cx);
                                    });
                                }))
                            .separator())
                        .menu_with_icon(if pinned { "Unpin" } else { "Pin" }, IconName::Pin, Box::new(ChatAction::Pin(id, !pinned)))
                        .when(!left, |menu| menu.menu_with_icon(if archived { "Unarchive" } else { "Archive" }, IconName::Archive,
                            Box::new(ChatAction::Archive(id, !archived))))
                        .separator()
                        .menu_with_icon(if unread { "Mark as read" } else { "Mark as unread" }, if unread { IconName::Check } else { IconName::Inbox },
                            Box::new(ChatAction::SetRead(id, unread)))
                        .when(!group && owner.is_some_and(|owner| owner != public_key), |menu| {
                            [
                                (if muted { "Unmute" } else { "Mute" }, IconName::Mute, 0),
                                (if blocked { "Unblock" } else { "Block" }, IconName::Block, 1),
                                ("Report", IconName::Flag, 2),
                            ].into_iter().fold(menu.separator(), |menu, (label, icon, action)| {
                                menu.item(ui::menu::PopupMenuItem::new(label).icon(icon)
                                    .on_click(move |_, window, cx| {
                                        // Let the context menu restore focus before opening a modal.
                                        window.defer(cx, move |window, cx| {
                                            if NostrRegistry::global(cx).read(cx).current_user() != owner { return; }
                                            match action {
                                                0 => crate::dialogs::moderation::mute(public_key, window, cx),
                                                1 => crate::dialogs::moderation::block(public_key, window, cx),
                                                _ => {
                                                    let name = person::PersonRegistry::global(cx).read(cx)
                                                        .get(&public_key, cx).name();
                                                    crate::dialogs::report::open(public_key, name, window, cx);
                                                }
                                            }
                                        });
                                    }))
                            })
                        })
                        .when(group, |menu| menu.separator().menu(if left { "Rejoin" } else { "Leave locally…" },
                            Box::new(ChatAction::Leave(id, !left))))
                }).into_any_element()
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
        let owner = nostr.read(cx).current_user();
        if self.request_badge.owner != owner
            || (self.list_filter == ChatFilter::Drafts && !owner.is_some_and(|owner| self.draft_indicators.read(cx).has_drafts_outside(owner, self.active_room)))
        {
            self.list_filter = ChatFilter::All;
        }

        let requests: Vec<_> = chat.read(cx).rooms(&RoomKind::Request, cx)
            .iter().map(|room| room.read(cx).id).collect();
        let viewing_requests = self.current_filter(&RoomKind::Request, cx);
        let visible_requests: Vec<_> = self.filtered_rooms(&RoomKind::Request, cx)
            .iter().map(|room| room.read(cx).id).collect();
        self.request_badge.update(owner, &visible_requests, viewing_requests);
        let new_requests = self.request_badge.update(owner, &requests, false);
        let unread_inbox = chat.read(cx).rooms(&RoomKind::Ongoing, cx).iter()
            .any(|room| chat.read(cx).has_unread(room.read(cx).id));
        let show_hints = window.is_window_active() && window.modifiers().secondary();

        v_flex()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::chat_action))
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
                            "note-to-self",
                            "Note to self",
                            IconName::Book,
                            crate::Command::NoteToSelf,
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
                    .w_full()
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .pb_2()
                    .px_2()
                    .gap_2()
                    .justify_center()
                    .child(div().relative().flex_1().child(
                        Button::new("all")
                            .map(|this| {
                                if self.current_filter(&RoomKind::Ongoing, cx) {
                                    this.icon(IconName::InboxFill)
                                } else {
                                    this.icon(IconName::Inbox)
                                }
                            })
                            .label("Inbox")
                            .when(unread_inbox, |button| button.child(
                                div().size_1().rounded_full().bg(cx.theme().cursor)))
                            .small()
                            .tooltip("All ongoing conversations")
                            .ghost_alt()
                            .font_semibold()
                            .w_full()
                            .selected(self.current_filter(&RoomKind::Ongoing, cx))
                            .on_click(cx.listener(|this, _ev, window, cx| {
                                this.set_filter(RoomKind::Ongoing, window, cx);
                            })),
                    ).when(show_hints, |view| {
                        view.when_some(ui::Kbd::binding_for_action(&crate::Command::ShowInbox, None, window), |view, hint| {
                            view.child(hint.absolute().top_neg_2().right_1())
                        })
                    }))
                    .child(div().relative().flex_1().child(
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
                            .w_full()
                            .selected(self.current_filter(&RoomKind::Request, cx))
                            .when(new_requests, |this| {
                                this.child(div().size_1().rounded_full().bg(cx.theme().cursor))
                            })
                            .on_click(cx.listener(|this, _ev, window, cx| {
                                this.set_filter(RoomKind::default(), window, cx);
                            })),
                    ).when(show_hints, |view| {
                        view.when_some(ui::Kbd::binding_for_action(&crate::Command::ShowRequests, None, window), |view, hint| {
                            view.child(hint.absolute().top_neg_2().right_1())
                        })
                    })),
            )
            .child(v_flex().w_full().flex_1().min_h_0().gap_1().map(|this| {
                if self.show_blocked {
                    return this.child(self.blocked_users.clone());
                }
                this.child(
                    uniform_list(
                        "rooms",
                        self.grouped_rows(cx).len(),
                        cx.processor(|this, range, _window, cx| this.render_list_items(range, cx)),
                    )
                    .track_scroll(&self.scroll_handle)
                    .flex_1()
                    .h_full()
                    .px_2(),
                )
                .child(Scrollbar::vertical(&self.scroll_handle))
            }))
            .child(self.render_footer(cx))
    }
}

#[cfg(test)]
mod date_tests {
    use super::*;
    #[test]
    fn request_badge_tracks_unseen_requests_and_resets_for_accounts() {
        use nostr_sdk::prelude::Keys;
        let owner = Some(Keys::generate().public_key());
        let mut badge = RequestBadge::default();
        assert!(!badge.update(owner, &[], false));
        assert!(badge.update(owner, &[1], false));
        assert!(!badge.update(owner, &[], false)); // removed/archived before viewing
        assert!(badge.update(owner, &[1], false));
        assert!(!badge.update(owner, &[1], true)); // viewed requests
        assert!(!badge.update(owner, &[1], false)); // other chat activity cannot re-light it
        assert!(badge.update(owner, &[1, 2], false));
        assert!(!badge.update(owner, &[1], false)); // new request accepted elsewhere
        assert!(badge.update(Some(Keys::generate().public_key()), &[1], false));
        assert!(!badge.update(None, &[1], false));
    }

    #[test]
    fn groups_use_calendar_dates_and_non_overlapping_week_boundaries() {
        let today = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        for (days, expected) in [(0, DateGroup::Today), (1, DateGroup::Yesterday),
            (2, DateGroup::LastWeek), (6, DateGroup::LastWeek), (7, DateGroup::Older),
            (40, DateGroup::Older), (-1, DateGroup::Today)] {
            assert!(date_group(today - chrono::Duration::days(days), today) == expected);
        }
    }
}
