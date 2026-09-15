use crate::Command;
use gpui::prelude::FluentBuilder;
use gpui::{
    Action, App, InteractiveElement, ParentElement, StatefulInteractiveElement, Styled, Window,
    div, px,
};
use theme::ActiveTheme;
use ui::dock::{CloseAllPanels, ClosePanel, NextPanel, PreviousPanel, ReopenClosedPanel};
use ui::{Kbd, StyledExt, WindowExtension, h_flex, v_flex};

pub const HELP_CONTEXT: &str = "Workspace && !Input";

pub fn open(window: &mut Window, cx: &mut App) {
    window.open_modal(cx, |modal, window, cx| {
        let groups: Vec<(&str, Vec<(&str, Box<dyn Action>)>)> = vec![
            ("Navigation", vec![
                ("New Chat", Box::new(Command::NewConversation)),
                ("Note to self", Box::new(Command::NoteToSelf)),
                ("New Group", Box::new(Command::NewGroup)),
                ("Search conversations", Box::new(Command::SearchConversations)),
                ("Search profiles", Box::new(Command::SearchProfiles)),
                ("Find in current chat", Box::new(Command::Search)),
                ("Toggle sidebar", Box::new(Command::ToggleSidebar)),
                ("Inbox", Box::new(Command::ShowInbox)),
                ("Requests", Box::new(Command::ShowRequests)),
                ("Focus message box", Box::new(Command::FocusComposer)),
            ]),
            ("Tabs", vec![
                ("Close tab", Box::new(ClosePanel)),
                ("Close all tabs", Box::new(CloseAllPanels)),
                ("Reopen closed tab", Box::new(ReopenClosedPanel)),
                ("Next tab", Box::new(NextPanel)),
                ("Previous tab", Box::new(PreviousPanel)),
            ]),
            ("Profile and relays", vec![
                ("Profile", Box::new(Command::ShowProfile)),
                ("Contacts", Box::new(Command::ShowContactList)),
                ("Messaging relays", Box::new(Command::ShowMessaging)),
                ("Gossip relays", Box::new(Command::ShowRelayList)),
            ]),
            ("Application", vec![
                ("Settings", Box::new(Command::ShowSettings)),
                ("Reload", Box::new(Command::RefreshMessagingRelays)),
                ("Keyboard shortcuts", Box::new(Command::KeyboardShortcuts)),
            ]),
        ];
        modal.title("Keyboard Shortcuts").show_close(true).width(px(540.)).child(
            v_flex().id("shortcut-list").max_h(px(520.)).overflow_y_scroll().gap_4()
                .children(groups.into_iter().map(|(title, actions)| {
                    v_flex().gap_2()
                        .child(div().font_semibold().text_sm().child(title))
                        .children(actions.into_iter().map(|(label, action)| {
                            h_flex().gap_4().justify_between().text_sm()
                                .child(label)
                                .when_some(Kbd::binding_for_action(action.as_ref(), Some("Workspace"), window), |row, keys| row.child(keys))
                        }))
                }))
                .child(v_flex().gap_2().text_sm()
                    .child(div().font_semibold().child("While typing"))
                    .child("Enter sends a message; Shift+Enter adds a new line.")
                    .child("In search, use ↑ / ↓ to choose and Enter to open. Esc returns to your previous view.")
                    .child("In chat find, Enter / Shift+Enter move to the next / previous match.")
                    .child(div().text_color(cx.theme().text_muted).child("? opens this window when you aren't typing."))))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn question_mark_is_not_a_shortcut_inside_inputs() {
        let predicate = gpui::KeyBindingContextPredicate::parse(HELP_CONTEXT).unwrap();
        let workspace = gpui::KeyContext::parse("Workspace").unwrap();
        let input = gpui::KeyContext::parse("Input").unwrap();
        assert!(predicate.depth_of(&[workspace.clone()]).is_some());
        assert!(predicate.depth_of(&[workspace, input]).is_none());
    }
}
