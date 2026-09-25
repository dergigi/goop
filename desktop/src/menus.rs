use gpui::{App, KeyBinding, Menu, MenuItem, OsAction, SystemMenuType, actions};
use ui::dock::{CloseAllPanels, ClosePanel, NextPanel, PreviousPanel, ReopenClosedPanel};
use ui::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
use workspace::{Command, FindInChat, ToggleChatPin, ToggleChatArchive, MarkChatRead, MarkChatUnread, LeaveChat};

actions!(goop, [Hide, HideOthers, ShowAll, BringToFront]);

pub fn init(cx: &mut App) {
    cx.on_action(|_: &Hide, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &BringToFront, cx| cx.activate(true));
    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("cmd-alt-h", HideOthers, None),
        KeyBinding::new("cmd-m", Command::MinimizeWindow, None),
        KeyBinding::new("ctrl-cmd-f", Command::ToggleFullScreen, None),
    ]);
    cx.set_menus(application_menus());
}

fn application_menus() -> Vec<Menu> {
    vec![
        Menu::new("Goop").items([
            MenuItem::action("About Goop", Command::About),
            MenuItem::separator(),
            MenuItem::action("Settings…", Command::ShowSettings),
            MenuItem::action("Check for Updates…", Command::Update),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Hide Goop", Hide),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit Goop", crate::Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("Close Tab", ClosePanel),
            MenuItem::action("Close All Tabs", CloseAllPanels),
            MenuItem::action("Reopen Closed Tab", ReopenClosedPanel),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", Undo, OsAction::Undo),
            MenuItem::os_action("Redo", Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", Cut, OsAction::Cut),
            MenuItem::os_action("Copy", Copy, OsAction::Copy),
            MenuItem::os_action("Paste", Paste, OsAction::Paste),
            MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
            MenuItem::separator(),
            MenuItem::action("Search Conversations…", Command::SearchConversations),
            MenuItem::action("Search Profiles…", Command::SearchProfiles),
        ]),
        Menu::new("View").items([
            MenuItem::action("Toggle Sidebar", Command::ToggleSidebar),
            MenuItem::action("Inbox", Command::ShowInbox),
            MenuItem::action("Requests", Command::ShowRequests),
            MenuItem::action("Focus Message Box", workspace::FocusComposer),
            MenuItem::action("Gallery", workspace::ShowGallery),
            MenuItem::separator(),
            MenuItem::action("Reload", Command::RefreshMessagingRelays),
            MenuItem::action("Toggle Full Screen", Command::ToggleFullScreen),
        ]),
        Menu::new("Chats").items([
            MenuItem::action("New Chat…", Command::NewConversation),
            MenuItem::action("New Group…", Command::NewGroup),
            MenuItem::action("Note to Self", Command::NoteToSelf),
            MenuItem::separator(),
            MenuItem::action("Find in Chat…", FindInChat),
            MenuItem::separator(),
            MenuItem::action("Pin / Unpin Chat", ToggleChatPin),
            MenuItem::action("Archive / Unarchive Chat", ToggleChatArchive),
            MenuItem::action("Mark as Read", MarkChatRead),
            MenuItem::action("Mark as Unread", MarkChatUnread),
            MenuItem::separator(),
            MenuItem::action("Leave Group Locally…", LeaveChat),
        ]),
        Menu::new("Account").items([
            MenuItem::action("Your Profile", Command::ShowProfile),
            MenuItem::action("Contacts", Command::ShowContactList),
            MenuItem::action("Blocked Users", Command::ShowBlockedUsers),
            MenuItem::separator(),
            MenuItem::action("Connection Status", Command::ShowConnectionStatus),
            MenuItem::action("Messaging Relays", Command::ShowMessaging),
            MenuItem::action("Gossip Relays", Command::ShowRelayList),
            MenuItem::action("Private Storage Relays", Command::ShowPrivateStorage),
            MenuItem::separator(),
            MenuItem::action("Log Out…", Command::Logout),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", Command::MinimizeWindow),
            MenuItem::separator(),
            MenuItem::action("Next Tab", NextPanel),
            MenuItem::action("Previous Tab", PreviousPanel),
            MenuItem::separator(),
            MenuItem::action("Bring All to Front", BringToFront),
        ]),
        Menu::new("Help").items([
            MenuItem::action("Keyboard Shortcuts", Command::KeyboardShortcuts),
            MenuItem::action("Usage Guide", Command::UsageGuide),
        ]),
    ]
}
