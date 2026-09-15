use chat::ChatRegistry;
use gpui::{App, ParentElement, Styled, Window};
use nostr_sdk::prelude::PublicKey;
use person::PersonRegistry;
use state::NostrRegistry;
use ui::button::{Button, ButtonVariants};
use ui::scroll::ScrollableElement;
use ui::{WindowExtension, v_flex};

pub fn mute(key: PublicKey, window: &mut Window, cx: &mut App) {
    let owner = NostrRegistry::global(cx).read(cx).current_user();
    let muted = ChatRegistry::global(cx).read(cx).is_muted(key);
    let name = PersonRegistry::global(cx).read(cx).get(&key, cx).name();
    window.open_modal(cx, move |modal, window, _| {
        modal.title(format!("Mute {name}")).margin_top(gpui::px(16.)).width(gpui::px(420.).min(window.viewport_size().width - gpui::px(32.)))
            .child("Pause notifications from this person on this device. Messages and unread counts will still appear.")
            .child(v_flex().gap_2().max_h((window.viewport_size().height - gpui::px(190.)).max(gpui::px(80.))).overflow_y_scrollbar().children([
                ("For 1 hour", Some(3600)), ("For 8 hours", Some(28800)),
                ("For 1 day", Some(86400)), ("For 1 week", Some(604800)),
                ("Until I unmute", Some(u64::MAX)), ("Unmute", None),
            ].into_iter().filter(move |(_, duration)| duration.is_some() || muted).map(move |(label, seconds)| {
                Button::new(label).label(label).ghost().on_click(move |_, window, cx| {
                    if owner != NostrRegistry::global(cx).read(cx).current_user() { window.close_modal(cx); return; }
                    let result = ChatRegistry::global(cx).update(cx, |chat,cx| chat.mute_user(key,seconds,cx));
                    match result {
                        Ok(()) => { window.close_modal(cx); }
                        Err(error) => { window.push_notification(ui::notification::Notification::error(error.to_string()),cx); }
                    }
                })
            })))
    });
}
pub fn block(key: PublicKey, window: &mut Window, cx: &mut App) {
    let owner = NostrRegistry::global(cx).read(cx).current_user();
    let blocked = ChatRegistry::global(cx).read(cx).is_blocked(key);
    let name = PersonRegistry::global(cx).read(cx).get(&key, cx).name();
    window.open_modal(cx, move |modal, _, _| {
        modal.confirm().title(format!("{} {name}?", if blocked { "Unblock" } else { "Block" }))
            .button_props(ui::modal::ModalButtonProps::default().ok_text(if blocked { "Unblock" } else { "Block" }).cancel_text("Cancel"))
            .child(if blocked {
                "Their chats and messages will become visible again. Goop will also remove them from your synced Nostr block list."
            } else {
                "Their direct chats will disappear from Inbox and Requests, and their messages in shared groups will be hidden. Goop will silence their notifications and add them privately to your synced Nostr block list. They can still send messages. You can unblock them from Blocked users in the sidebar."
            })
            .on_ok(move |_, window, cx| {
                if owner != NostrRegistry::global(cx).read(cx).current_user() { return true; }
                match ChatRegistry::global(cx).update(cx, |chat,cx| chat.block_user(key,!blocked,cx)) {
                    Ok(()) => true,
                    Err(error) => { window.push_notification(ui::notification::Notification::error(error.to_string()),cx); false }
                }
            })
    });
}
