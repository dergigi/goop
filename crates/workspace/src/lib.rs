use std::sync::Arc;

use ::settings::AppSettings;
use anyhow::Error;
use auto_update::AutoUpdater;
use chat::{ChatEvent, ChatRegistry};
use common::{GoopImageCache, download_dir};
use device::{DeviceEvent, DeviceRegistry};
use gpui::prelude::FluentBuilder;
use gpui::{
    Action, App, AppContext, Axis, Context, Entity, InteractiveElement, IntoElement,
    KeyBinding, ParentElement, Render, SharedString, Styled, Subscription, Task, Window, div,
    image_cache, px,
};
use nostr_sdk::prelude::*;
use person::{PersonRegistry, shorten_pubkey};
use serde::Deserialize;
use smallvec::{SmallVec, smallvec};
use state::{IMAGE_CACHE_SIZE, NostrRegistry, StateEvent};
use theme::{ActiveTheme, Appearance, SIDEBAR_WIDTH, Theme};
use ui::avatar::Avatar;
use ui::button::{Button, ButtonVariants};
use ui::dock::{
    CloseAllPanels, ClosePanel, DockArea, DockEvent, DockItem, DockPlacement, NextPanel, PanelView,
    PreviousPanel, ReopenClosedPanel,
};
use ui::menu::{DropdownMenu, PopupMenuItem};
use ui::notification::{Notification, NotificationKind};
use ui::{Icon, IconName, Root, Sizable, TitleBar, WindowExtension, h_flex, v_flex};

use crate::dialogs::import::ImportIdentity;
use crate::dialogs::restore::RestoreEncryption;
use crate::dialogs::settings;
use crate::panels::{contact_list, greeter, messaging_relays, profile, relay_list};
use crate::sidebar::Sidebar;

mod build_info;
mod dialogs;
mod panels;
mod sidebar;

gpui::actions!(workspace, [FindInChat, ToggleChatPin, ToggleChatArchive, MarkChatRead, MarkChatUnread, LeaveChat]);

enum ChatMenuOperation { TogglePin, ToggleArchive, MarkRead, MarkUnread, Leave }

pub fn init(window: &mut Window, cx: &mut App) -> Entity<Workspace> {
    let modifier = if cx.theme().platform.is_mac() {
        "cmd"
    } else {
        "ctrl"
    };
    cx.bind_keys([
        KeyBinding::new(
            "?",
            Command::KeyboardShortcuts,
            Some(dialogs::shortcuts::HELP_CONTEXT),
        ),
        KeyBinding::new(&format!("{modifier}-,"), Command::ShowSettings, None),
        KeyBinding::new(&format!("{modifier}-f"), FindInChat, None),
        KeyBinding::new(&format!("{modifier}-b"), Command::ToggleSidebar, None),
        KeyBinding::new(&format!("{modifier}-k"), Command::SearchConversations, None),
        KeyBinding::new(&format!("{modifier}-p"), Command::SearchProfiles, None),
        KeyBinding::new(&format!("{modifier}-shift-p"), Command::ShowProfile, None),
        KeyBinding::new(&format!("{modifier}-shift-c"), Command::ShowContactList, None),
        KeyBinding::new(&format!("{modifier}-shift-m"), Command::ShowMessaging, None),
        KeyBinding::new(&format!("{modifier}-shift-g"), Command::ShowRelayList, None),
        KeyBinding::new(&format!("{modifier}-shift-d"), Command::ShowConnectionStatus, None),
        KeyBinding::new(
            &format!("{modifier}-r"),
            Command::RefreshMessagingRelays,
            None,
        ),
        KeyBinding::new(&format!("{modifier}-1"), Command::ShowInbox, None),
        KeyBinding::new(&format!("{modifier}-2"), Command::ShowRequests, None),
        KeyBinding::new(&format!("{modifier}-3"), Command::FocusComposer, None),
        KeyBinding::new(&format!("{modifier}-t"), Command::NewConversation, None),
        KeyBinding::new(&format!("{modifier}-n"), Command::NewConversation, None),
        KeyBinding::new(&format!("{modifier}-shift-n"), Command::NewGroup, None),
        KeyBinding::new(&format!("{modifier}-shift-s"), Command::NoteToSelf, None),
        KeyBinding::new(&format!("{modifier}-w"), ClosePanel, None),
        KeyBinding::new(&format!("{modifier}-shift-w"), CloseAllPanels, None),
        KeyBinding::new(&format!("{modifier}-shift-t"), ReopenClosedPanel, None),
        KeyBinding::new("ctrl-tab", NextPanel, None),
        KeyBinding::new("ctrl-shift-tab", PreviousPanel, None),
    ]);

    cx.new(|cx| Workspace::new(window, cx))
}

struct DeviceNotifcation;
struct MsgRelayNotification;

#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = workspace, no_json)]
pub enum Command {
    About,
    MinimizeWindow,
    ToggleFullScreen,
    Search,
    SearchConversations,
    SearchProfiles,
    ShowInbox,
    ShowRequests,
    FocusComposer,
    NewConversation,
    NoteToSelf,
    NewGroup,
    UsageGuide,
    KeyboardShortcuts,
    SetUpAgents,
    ToggleSidebar,
    Update,
    RefreshMessagingRelays,
    LoadOlderHistory,
    SearchOtherRelays,
    RetryDecryption,
    BackupEncryption,
    ImportEncryption,
    RefreshEncryption,
    ResetEncryption,
    ShowRelayList,
    ShowMessaging,
    ShowConnectionStatus,
    ShowProfile,
    OpenProfile(PublicKey),
    ShowBlockedUsers,
    OpenProfileChat(PublicKey, bool),
    ShowSettings,
    Logout,
    ShowContactList,
}

pub struct Workspace {
    sidebar: Entity<Sidebar>,
    connection_status: Entity<panels::connection_status::ConnectionStatus>,
    /// App's Dock Area
    dock: Entity<DockArea>,
    pending_profile_search: Option<u64>,

    /// App's Image Cache
    image_cache: Entity<GoopImageCache>,

    /// Async tasks
    tasks: Vec<Task<Result<(), Error>>>,

    /// Event subscriptions
    _subscriptions: SmallVec<[Subscription; 6]>,
}

impl Workspace {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let chat = ChatRegistry::global(cx);
        let device = DeviceRegistry::global(cx);
        let nostr = NostrRegistry::global(cx);

        let sidebar = cx.new(|cx| Sidebar::new(window, cx));
        let dock = cx.new(|cx| DockArea::new(window, cx));
        let image_cache = GoopImageCache::new(IMAGE_CACHE_SIZE, cx);

        let connection_status = panels::connection_status::init(cx);
        let mut subscriptions = smallvec![];
        subscriptions.push(cx.observe(&connection_status, |_, _, cx| cx.notify()));
        subscriptions.push(cx.observe(&nostr, |_, _, cx| cx.notify()));
        subscriptions.push(cx.subscribe_in(&dock, window, |_, _, event, window, cx| {
            if matches!(event, DockEvent::LayoutChanged) {
                // Wait until close/drag/close-all has finished changing the live tree.
                cx.defer_in(window, |this, window, cx| {
                    if this.dock.read(cx).center_is_empty(cx) {
                        let welcome = greeter::init(window, cx);
                        this.dock.update(cx, |dock, cx| {
                            dock.add_panel(Arc::new(welcome), DockPlacement::Center, window, cx);
                        });
                    }
                });
            }
        }));
        subscriptions.push(cx.observe(&chat, |_, _, cx| cx.notify()));

        subscriptions.push(
            // Observe system appearance and update theme
            cx.observe_window_appearance(window, |_this, window, cx| {
                if AppSettings::get_appearance(cx) == Appearance::System {
                    Theme::sync_system_appearance(Some(window), cx);
                }
            }),
        );

        subscriptions.push(
            // Subscribe to the nostr events
            cx.subscribe_in(&nostr, window, move |this, _state, event, window, cx| {
                match event {
                    StateEvent::SignerChanged => {
                        this.dock.update(cx, |dock, _| dock.clear_closed_panels());
                        window.close_all_modals(cx);
                    }
                    StateEvent::NoSigner => {
                        window.close_all_modals(cx);
                        DockArea::close_all(&this.dock, window, cx);
                        this.dock.update(cx, |dock, _| dock.clear_closed_panels());
                        this.import_identity(window, cx);
                    }
                    _ => {}
                };
            }),
        );

        subscriptions.push(
            // Observe all events emitted by the device registry
            cx.subscribe_in(&device, window, |_this, _device, event, window, cx| {
                match event {
                    DeviceEvent::Requesting => {
                        const MSG: &str =
                            "Open your other client and approve the encryption key request.";

                        let note = Notification::new()
                            .id::<DeviceNotifcation>()
                            .autohide(false)
                            .title("Wait for approval")
                            .message(MSG)
                            .with_kind(NotificationKind::Info);

                        window.push_notification(note, cx);
                    }
                    DeviceEvent::NotSet => {
                        const MSG: &str =
                            "You haven't set up an encryption key yet. Would you like to create one?";

                        let note = Notification::new()
                            .id::<DeviceNotifcation>()
                            .message(MSG)
                            .with_kind(NotificationKind::Info)
                            .action(|_this, _window, _cx| {
                                Button::new("retry").label("Retry").on_click(
                                    move |_this, window, cx| {
                                        let device = DeviceRegistry::global(cx);
                                        device.update(cx, |this, cx| {
                                            this.set_announcement(Keys::generate(), cx);
                                        });
                                        window.clear_notification::<DeviceNotifcation>(cx);
                                    },
                                )
                            });

                        window.push_notification(note, cx);
                    }
                    DeviceEvent::Set => {
                        let note = Notification::new()
                            .id::<DeviceNotifcation>()
                            .message("Your encryption key has been set.")
                            .with_kind(NotificationKind::Success);

                        window.push_notification(note, cx);
                    }
                    DeviceEvent::Error(error) => {
                        window.push_notification(Notification::error(error).autohide(false), cx);
                    }
                };
            }),
        );

        subscriptions.push(
            // Observe all events emitted by the chat registry
            cx.subscribe_in(&chat, window, move |this, chat, ev, window, cx| {
                match ev {
                    ChatEvent::InboxRelayNotFound => {
                        const MSG: &str = "No messaging relays were found. Goop cannot receive messages.";

                        window.push_notification(
                            Notification::warning(MSG)
                                .id::<MsgRelayNotification>()
                                .autohide(false)
                                .action(|_this, _window, _cx| {
                                    Button::new("retry").label("Retry").on_click(
                                        move |_this, window, cx| {
                                            let chat = ChatRegistry::global(cx);
                                            chat.update(cx, |this, cx| {
                                                this.get_metadata(cx);
                                            });
                                            window.clear_notification::<MsgRelayNotification>(cx);
                                        },
                                    )
                                }),
                            cx,
                        );
                    }
                    ChatEvent::OpenProfile(public_key) => {
                        let public_key = *public_key;
                        // Let the context menu finish dismissing before moving focus.
                        cx.defer_in(window, move |this, window, cx| {
                            this.on_command(&Command::OpenProfile(public_key), window, cx);
                        });
                    }
                    ChatEvent::OpenRoom(id) => {
                        if let Some(room) = chat.read(cx).room(id, cx) {
                            this.dock.update(cx, |this, cx| {
                                this.add_panel(
                                    Arc::new(chat_ui::init(room, window, cx)),
                                    DockPlacement::Center,
                                    window,
                                    cx,
                                );
                            });
                            if this.pending_profile_search == Some(*id) {
                                this.pending_profile_search = None;
                                for panel in this.dock.read(cx).active_panels(cx) {
                                    if panel.panel_id(cx).as_ref() == id.to_string()
                                        && let Ok(chat) = panel.view().downcast::<chat_ui::ChatPanel>()
                                    {
                                        chat.update(cx, |chat, cx| chat.focus_find(window, cx));
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    ChatEvent::CloseRoom(..) => {
                        this.dock.update(cx, |this, cx| {
                            // Force focus to the tab panel
                            this.focus_tab_panel(window, cx);

                            // Dispatch the close panel action
                            cx.defer_in(window, |_, window, cx| {
                                window.dispatch_action(Box::new(ClosePanel), cx);
                                window.close_all_modals(cx);
                            });
                        });
                    }
                    ChatEvent::Error(error) => {
                        window.push_notification(Notification::error(error).autohide(false), cx);
                    }
                    _ => {}
                };
            }),
        );

        cx.defer_in(window, |this, window, cx| {
            let dock = this.dock.downgrade();
            let greeter = Arc::new(greeter::init(window, cx));
            let tabs = DockItem::tabs(vec![greeter], None, &dock, window, cx);
            let center = DockItem::split(Axis::Vertical, vec![tabs], &dock, window, cx);

            let sidebar = DockItem::panel(Arc::new(this.sidebar.clone()));
            this.dock.update(cx, |this, cx| {
                this.set_center(center, window, cx);
                this.set_left_dock(sidebar, Some(SIDEBAR_WIDTH), true, window, cx);
            });
        });

        Self {
            sidebar,
            connection_status,
            dock,
            pending_profile_search: None,
            image_cache,
            tasks: vec![],
            _subscriptions: subscriptions,
        }
    }

    /// Add panel to the dock
    pub fn add_panel<P>(panel: P, placement: DockPlacement, window: &mut Window, cx: &mut App)
    where
        P: PanelView,
    {
        if let Some(root) = window.root::<Root>().flatten()
            && let Ok(workspace) = root.read(cx).view().clone().downcast::<Self>()
        {
            workspace.update(cx, |this, cx| {
                this.dock.update(cx, |this, cx| {
                    this.add_panel(Arc::new(panel), placement, window, cx);
                });
            });
        }
    }

    /// Handle command events
    fn cycle_tabs(&self, backwards: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.dock.read(cx).active_tab_group(window, cx) {
            tab.update(cx, |tab, cx| tab.cycle(backwards, window, cx));
        }
    }

    fn on_command(&mut self, command: &Command, window: &mut Window, cx: &mut Context<Self>) {
        match command {
            Command::MinimizeWindow => window.minimize_window(),
            Command::ToggleFullScreen => window.toggle_fullscreen(),
            Command::About => {
                window.open_modal(cx, |modal, _, _| {
                    modal.title("About Goop").show_close(true).child(
                        v_flex()
                            .gap_2()
                            .child(h_flex().child(build_info::version_link()))
                            .child(h_flex().child(build_info::build_link()))
                            .child("A native NIP-17 client for you and your agents.")
                            .child(
                                Button::new("about-source")
                                    .label("Goop on GitHub")
                                    .ghost()
                                    .on_click(|_, _, cx| {
                                        cx.open_url("https://github.com/dergigi/goop")
                                    }),
                            ),
                    )
                });
            }

            Command::ToggleSidebar => {
                self.dock.update(cx, |dock, cx| {
                    dock.toggle_dock(DockPlacement::Left, window, cx)
                });
            }
            Command::ShowInbox | Command::ShowRequests => {
                if !self.dock.read(cx).is_dock_open(DockPlacement::Left, cx) {
                    self.dock.update(cx, |dock, cx| {
                        dock.toggle_dock(DockPlacement::Left, window, cx)
                    });
                }
                let kind = if matches!(command, Command::ShowInbox) {
                    chat::RoomKind::Ongoing
                } else {
                    chat::RoomKind::Request
                };
                self.sidebar
                    .update(cx, |sidebar, cx| sidebar.set_filter(kind, window, cx));
            }
            Command::FocusComposer | Command::Search => {
                if matches!(command, Command::Search)
                    && ui::Root::read(window, cx).has_active_modals()
                {
                    return;
                }
                let mut panels = if matches!(command, Command::Search) {
                    Vec::new()
                } else {
                    self.dock.read(cx).active_panels(cx)
                };
                if let Some(tab) = self.dock.read(cx).active_tab_group(window, cx)
                    && let Some(panel) = tab.read(cx).active_panel(cx)
                {
                    panels.insert(0, panel);
                }
                if let Some(chat) = panels
                    .into_iter()
                    .find_map(|panel| panel.view().downcast::<chat_ui::ChatPanel>().ok())
                {
                    chat.update(cx, |chat, cx| {
                        if matches!(command, Command::Search) {
                            chat.focus_find(window, cx);
                        } else {
                            chat.focus_composer(window, cx);
                        }
                    });
                }
            }
            Command::SearchConversations | Command::SearchProfiles => {
                dialogs::quick_search::open(matches!(command, Command::SearchProfiles), window, cx);
            }
            Command::NewConversation => {
                dialogs::new_chat::open(window, cx);
            }
            Command::NoteToSelf => dialogs::new_chat::open_self(window, cx),
            Command::NewGroup => dialogs::new_chat::open_group(window, cx),
            Command::UsageGuide => cx.open_url("https://dergigi.com/goop/"),
            Command::KeyboardShortcuts => dialogs::shortcuts::open(window, cx),
            Command::SetUpAgents => {
                cx.open_url("https://dergigi.com/goop/#agent-guide");
            }
            Command::ShowSettings => {
                let view = settings::init(window, cx);

                window.open_modal(cx, move |this, _window, _cx| {
                    this.width(px(520.))
                        .show_close(true)
                        .pb_2()
                        .title("Preferences")
                        .child(view.clone())
                });
            }
            Command::ShowProfile => {
                let nostr = NostrRegistry::global(cx);

                if let Some(public_key) = nostr.read(cx).current_user() {
                    self.dock.update(cx, |this, cx| {
                        this.add_panel(
                            Arc::new(profile::init(public_key, window, cx)),
                            DockPlacement::Right,
                            window,
                            cx,
                        );
                    });
                }
            }
            Command::OpenProfileChat(public_key, search) => {
                let Some(owner) = NostrRegistry::global(cx).read(cx).current_user() else { return; };
                let candidate = cx.new(|_| chat::Room::new(owner, [*public_key])
                    .organize(&owner).kind(chat::RoomKind::Ongoing));
                let chat = ChatRegistry::global(cx);
                let room = chat.read(cx).room(&candidate.read(cx).id, cx)
                    .and_then(|room| room.upgrade()).unwrap_or(candidate);
                self.pending_profile_search = search.then_some(room.read(cx).id);
                chat.update(cx, |chat, cx| chat.emit_room(&room, window, cx));
            }
            Command::ShowBlockedUsers => {
                if !self.dock.read(cx).is_dock_open(DockPlacement::Left, cx) {
                    self.dock.update(cx, |dock, cx| dock.toggle_dock(DockPlacement::Left, window, cx));
                }
                self.sidebar.update(cx, |sidebar, cx| sidebar.show_blocked(window, cx));
            }
            Command::OpenProfile(public_key) => {
                self.dock.update(cx, |dock, cx| {
                    dock.add_panel(
                        Arc::new(panels::person_profile::init(*public_key, window, cx)),
                        DockPlacement::Right,
                        window,
                        cx,
                    );
                });
            }
            Command::ShowContactList => {
                self.dock.update(cx, |this, cx| {
                    this.add_panel(
                        Arc::new(contact_list::init(window, cx)),
                        DockPlacement::Right,
                        window,
                        cx,
                    );
                });
            }
            Command::ShowConnectionStatus => {
                let panel = self.connection_status.clone();
                self.dock.update(cx, |dock, cx| dock.add_panel(Arc::new(panel), DockPlacement::Right, window, cx));
            }
            Command::ShowMessaging => {
                self.dock.update(cx, |this, cx| {
                    this.add_panel(
                        Arc::new(messaging_relays::init(window, cx)),
                        DockPlacement::Right,
                        window,
                        cx,
                    );
                });
            }
            Command::LoadOlderHistory => {
                ChatRegistry::global(cx).update(cx, |chat, cx| chat.load_older_history(cx))
            }
            Command::SearchOtherRelays => {
                ChatRegistry::global(cx).update(cx, |chat, cx| chat.search_other_relays(cx))
            }
            Command::RetryDecryption => {
                ChatRegistry::global(cx).update(cx, |chat, cx| chat.retry_failed_messages(cx))
            }
            Command::RefreshMessagingRelays => {
                let chat = ChatRegistry::global(cx);
                // Trigger a refresh of the chat registry
                chat.update(cx, |this, cx| {
                    this.reload(cx);
                });
            }
            Command::ShowRelayList => {
                self.dock.update(cx, |this, cx| {
                    this.add_panel(
                        Arc::new(relay_list::init(window, cx)),
                        DockPlacement::Right,
                        window,
                        cx,
                    );
                });
            }
            Command::RefreshEncryption => {
                let device = DeviceRegistry::global(cx);
                device.update(cx, |this, cx| {
                    this.get_announcement(cx);
                });
            }
            Command::Logout => {
                window.open_modal(cx, |modal, _, _| {
                    modal.confirm()
                        .title("Log out?")
                        .button_props(ui::modal::ModalButtonProps::default()
                            .ok_text("Log out")
                            .ok_variant(ui::button::ButtonVariant::Danger)
                            .cancel_text("Cancel"))
                        .child("Disconnect your signer and forget the saved login on this device. Your local chat history will be kept. You’ll need to connect your signer again to log in.")
                        .on_ok(|_, window, cx| {
                            let task = NostrRegistry::global(cx).update(cx, |state, cx| state.logout(cx));
                            window.spawn(cx, async move |cx| {
                                if let Err(error) = task.await {
                                    cx.update(|window, cx| {
                                        window.push_notification(Notification::error(format!("Could not log out: {error}")).autohide(false), cx);
                                    }).ok();
                                }
                            }).detach();
                            true
                        })
                });
            }
            Command::ResetEncryption => {
                self.confirm_reset_encryption(window, cx);
            }
            Command::BackupEncryption => {
                let device = DeviceRegistry::global(cx).downgrade();
                let save_dialog = cx.prompt_for_new_path(download_dir(), Some("encryption.txt"));

                self.tasks.push(cx.spawn_in(window, async move |_this, cx| {
                    // Get the output path from the save dialog
                    let output_path = match save_dialog.await {
                        Ok(Ok(Some(path))) => path,
                        Ok(Ok(None)) | Err(_) => return Ok(()),
                        Ok(Err(error)) => {
                            cx.update(|window, cx| {
                                let message = format!("Failed to pick save location: {error:#}");
                                let note = Notification::error(message).autohide(false);
                                window.push_notification(note, cx);
                            })?;
                            return Ok(());
                        }
                    };

                    // Get the backup task
                    let backup =
                        device.read_with(cx, |this, cx| this.backup(output_path.clone(), cx))?;

                    // Run the backup task
                    backup.await?;

                    // Open the backup file with the system's default application
                    cx.update(|_window, cx| {
                        cx.open_with_system(output_path.as_path());
                    })?;

                    Ok(())
                }));
            }
            Command::ImportEncryption => {
                self.import_encryption(window, cx);
            }
            Command::Update => {
                // No-op on managed distribution channels (Flatpak/Snap) where
                // the in-app updater is never initialized.
                if let Some(auto_updater) = AutoUpdater::try_global(cx) {
                    auto_updater.update(cx, |this, cx| {
                        this.updater.update(cx, |updater, cx| {
                            updater.check(cx);
                        });
                    });
                }
            }
        }
    }

    fn confirm_reset_encryption(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        const ENC_MSG: &str = "An encryption key is a special key used to encrypt and decrypt your messages. \
                               Your identity is completely decoupled from all encryption processes to protect your privacy.";

        const ENC_WARN: &str = "By resetting your encryption key, you will lose access to \
                                all your previously encrypted messages. This action cannot be undone.";

        let device = DeviceRegistry::global(cx);
        let ent = device.downgrade();

        window.open_modal(cx, move |this, _window, cx| {
            let ent = ent.clone();

            this.confirm()
                .show_close(true)
                .title("Reset Encryption Key")
                .child(
                    v_flex()
                        .gap_1()
                        .text_sm()
                        .child(SharedString::from(ENC_MSG))
                        .child(
                            div()
                                .italic()
                                .text_color(cx.theme().text_danger)
                                .child(SharedString::from(ENC_WARN)),
                        ),
                )
                .on_ok(move |_ev, _window, cx| {
                    ent.update(cx, |this, cx| {
                        this.set_announcement(Keys::generate(), cx);
                    })
                    .ok();
                    // true to close modal
                    true
                })
        });
    }

    fn import_encryption(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let restore = cx.new(|cx| RestoreEncryption::new(window, cx));
        window.open_modal(cx, move |this, _window, _cx| {
            this.width(px(420.))
                .title("Restore Encryption")
                .child(restore.clone())
        });
    }

    fn import_identity(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let import = cx.new(|cx| ImportIdentity::new(window, cx));

        window.open_modal(cx, move |this, _window, _cx| {
            this.width(px(450.))
                .show_close(false)
                .overlay_closable(false)
                .keyboard(false)
                .title("Connect Your Signer")
                .child(import.clone())
        });
    }

    fn titlebar_left(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let nostr = NostrRegistry::global(cx);
        let displayed_user = nostr.read(cx).displayed_user();

        h_flex()
            .flex_shrink_0()
            .gap_2()
            .when_some(displayed_user.as_ref(), |this, public_key| {
                let persons = PersonRegistry::global(cx);
                let profile = persons.read(cx).get(public_key, cx);
                let avatar = profile.avatar();
                let name = profile.name();

                this.child(
                    Button::new("current-user")
                        .child(Avatar::new(avatar.clone()).xsmall())
                        .small()
                        .caret()
                        .compact()
                        .transparent()
                        .dropdown_menu(move |this, _window, cx| {
                            let avatar = avatar.clone();
                            let name = name.clone();

                            this.min_w(px(256.))
                                .item(PopupMenuItem::element(move |_window, cx| {
                                    h_flex()
                                        .gap_1p5()
                                        .text_xs()
                                        .text_color(cx.theme().text_muted)
                                        .child(Avatar::new(avatar.clone()).xsmall())
                                        .child(name.clone())
                                }))
                                .separator()
                                .menu(
                                    "Search conversations",
                                    Box::new(Command::SearchConversations),
                                )
                                .menu("Search profiles", Box::new(Command::SearchProfiles))
                                .menu("Toggle sidebar", Box::new(Command::ToggleSidebar))
                                .menu("Connection status", Box::new(Command::ShowConnectionStatus))
                                .separator()
                                .menu_with_icon(
                                    "Profile",
                                    IconName::Profile,
                                    Box::new(Command::ShowProfile),
                                )
                                .menu_with_icon(
                                    "Contact List",
                                    IconName::Book,
                                    Box::new(Command::ShowContactList),
                                )
                                // Only offer in-app updates when auto-update is
                                // enabled (managed channels update themselves).
                                .when(AutoUpdater::is_available(cx), |this| {
                                    this.separator().menu_with_icon(
                                        "Check for Updates",
                                        IconName::Device,
                                        Box::new(Command::Update),
                                    )
                                })
                                .menu_with_icon(
                                    "Settings",
                                    IconName::Settings,
                                    Box::new(Command::ShowSettings),
                                )
                                .separator()
                                .menu_element_with_icon(
                                    Icon::new(IconName::Door).text_color(cx.theme().text_danger),
                                    Box::new(Command::Logout),
                                    |_, cx| div().text_color(cx.theme().text_danger).child("Log out"),
                                )
                        }),
                )
            })
            .child(
                Button::new("connection-status-summary")
                    .label(self.connection_status.read(cx).summary(cx))
                    .tooltip("Connection status and recovery")
                    .small().ghost()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.on_command(&Command::ShowConnectionStatus, window, cx);
                    })),
            )
    }

    fn active_chat_panel(&self, window: &Window, cx: &App) -> Option<Entity<chat_ui::ChatPanel>> {
        let mut panels = self.dock.read(cx).active_panels(cx);
        if let Some(tab) = self.dock.read(cx).active_tab_group(window, cx)
            && let Some(panel) = tab.read(cx).active_panel(cx) {
            panels.insert(0, panel);
        }
        panels.into_iter().find_map(|panel| panel.view().downcast::<chat_ui::ChatPanel>().ok())
    }

    fn chat_menu_action(&mut self, action: ChatMenuOperation, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = self.active_chat_panel(window, cx) else { return; };
        let Some(room) = panel.read(cx).room() else { return; };
        let room = room.read(cx);
        let chat = ChatRegistry::global(cx);
        let chat = chat.read(cx);
        let action = match action {
            ChatMenuOperation::TogglePin => sidebar::ChatAction::Pin(room.id, !chat.is_pinned(room)),
            ChatMenuOperation::ToggleArchive => sidebar::ChatAction::Archive(room.id, !chat.is_archived(room)),
            ChatMenuOperation::MarkRead => sidebar::ChatAction::SetRead(room.id, true),
            ChatMenuOperation::MarkUnread => sidebar::ChatAction::SetRead(room.id, false),
            ChatMenuOperation::Leave if room.is_group() => sidebar::ChatAction::Leave(room.id, true),
            _ => return,
        };
        self.sidebar.update(cx, |sidebar, cx| sidebar.chat_action(&action, window, cx));
    }

    fn titlebar_right(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let chat = ChatRegistry::global(cx);
        let nip4e_enabled = AppSettings::get_nip4e(cx);
        let nostr = NostrRegistry::global(cx);

        let public_key = nostr.read(cx).current_user();
        let persons = PersonRegistry::global(cx);
        let announcement = public_key.and_then(|key| persons.read(cx).get(&key, cx).announcement());
        let shortcut = if cx.theme().platform.is_mac() { "⌘⇧D" } else { "Ctrl+Shift+D" };
        let status = self.connection_status.read(cx).summary(cx);

        // Update status is only shown when auto-update is available. On
        // managed distribution channels (Flatpak/Snap) no updater exists, so
        // nothing is rendered.
        let updater_status = AutoUpdater::try_global(cx).and_then(|updater| {
            let updater = updater.read(cx);
            (!updater.idle(cx)).then(|| updater.status(cx))
        });

        h_flex()
            .when(!cx.theme().platform.is_mac(), |this| this.pr_2())
            .gap_2()
            .when_some(updater_status, |this, status| {
                this.child(div().text_xs().italic().child(status))
            })
            .when(nip4e_enabled && public_key.is_some(), |this| {
                this.child(
                    Button::new("key")
                        .icon(IconName::UserKey)
                        .tooltip("Decoupled encryption key")
                        .small()
                        .ghost()
                        .dropdown_menu(move |this, _window, _cx| {
                            this.min_w(px(260.))
                                .label("Encryption Key")
                                .when_some(announcement.as_ref(), |this, announcement| {
                                    let name = announcement.client_name();
                                    let pkey = shorten_pubkey(announcement.public_key(), 8);

                                    this.item(PopupMenuItem::element(move |_window, cx| {
                                        h_flex()
                                            .gap_1()
                                            .text_sm()
                                            .child(
                                                Icon::new(IconName::Device)
                                                    .small()
                                                    .text_color(cx.theme().icon_muted),
                                            )
                                            .child(name.clone())
                                    }))
                                    .item(
                                        PopupMenuItem::element(move |_window, cx| {
                                            h_flex()
                                                .gap_1()
                                                .text_sm()
                                                .child(
                                                    Icon::new(IconName::UserKey)
                                                        .small()
                                                        .text_color(cx.theme().icon_muted),
                                                )
                                                .child(SharedString::from(pkey.clone()))
                                        }),
                                    )
                                })
                                .separator()
                                .menu_with_icon(
                                    "Export Encryption Key",
                                    IconName::Shield,
                                    Box::new(Command::BackupEncryption),
                                )
                                .menu_with_icon(
                                    "Restore from secret key",
                                    IconName::Usb,
                                    Box::new(Command::ImportEncryption),
                                )
                                .separator()
                                .menu_with_icon(
                                    "Reload",
                                    IconName::Refresh,
                                    Box::new(Command::RefreshEncryption),
                                )
                                .menu_with_icon(
                                    "Reset",
                                    IconName::Warning,
                                    Box::new(Command::ResetEncryption),
                                )
                        }),
                )
            })
            .child(
                Button::new("titlebar-relays")
                    .icon(IconName::Relay).small().ghost().tooltip("Relays")
                    .dropdown_menu(|menu, _, _| {
                        menu.menu("Messaging Relays", Box::new(Command::ShowMessaging))
                            .menu("Gossip Relays", Box::new(Command::ShowRelayList))
                    }),
            )
            .child(
                Button::new("connection-status")
                    .icon(IconName::Activity)
                    .tooltip(format!("Connection status ({shortcut})\n{status}"))
                    .small().ghost()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.on_command(&Command::ShowConnectionStatus, window, cx);
                    })),
            )
            .when(public_key.is_some(), |bar| bar.child(
                Button::new("inbox")
                    .icon(IconName::Inbox)
                    .tooltip("Message history")
                    .small()
                    .ghost()
                    .dropdown_menu(move |this, _window, cx| {
                        let chat = chat.read(cx);
                        let summary = if chat.history_running() { "Loading message history…" }
                            else if chat.history_error().is_some() || chat.history_relays().values().any(|relay| relay.error.is_some()) { "History incomplete" }
                            else if chat.pending_messages() > 0 { "Decrypting messages…" }
                            else { "Search for older messages" };
                        this.min_w(px(240.))
                            .label("Message history")
                            .label(summary)
                            .separator()
                            .menu_with_icon("Rescan all history", IconName::History, Box::new(Command::LoadOlderHistory))
                            .menu_with_icon("Search other relays", IconName::Search, Box::new(Command::SearchOtherRelays))
                            .separator()
                            .menu_with_icon("Connection status", IconName::Activity, Box::new(Command::ShowConnectionStatus))
                    }),
            ))
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active_room = self.active_chat_panel(window, cx).and_then(|panel| panel.read(cx).room());
        let chat_actions_available = active_room.is_some() && !Root::read(window, cx).has_active_modals();
        let group_active = active_room.is_some_and(|room| room.read(cx).is_group());
        let modal_layer = Root::render_modal_layer(window, cx);
        let notification_layer = Root::render_notification_layer(window, cx);

        div()
            .id("workspace")
            .key_context("Workspace")
            .on_modifiers_changed(|_, window, _| window.refresh())
            .on_action(cx.listener(Self::on_command))
            .when(chat_actions_available, |view| view
                .on_action(cx.listener(|this, _: &FindInChat, window, cx| {
                    if let Some(panel) = this.active_chat_panel(window, cx) {
                        panel.update(cx, |chat, cx| chat.focus_find(window, cx));
                    }
                }))
                .on_action(cx.listener(|this, _: &ToggleChatPin, window, cx| this.chat_menu_action(ChatMenuOperation::TogglePin, window, cx)))
                .on_action(cx.listener(|this, _: &ToggleChatArchive, window, cx| this.chat_menu_action(ChatMenuOperation::ToggleArchive, window, cx)))
                .on_action(cx.listener(|this, _: &MarkChatRead, window, cx| this.chat_menu_action(ChatMenuOperation::MarkRead, window, cx)))
                .on_action(cx.listener(|this, _: &MarkChatUnread, window, cx| this.chat_menu_action(ChatMenuOperation::MarkUnread, window, cx)))
                .when(group_active, |view| view.on_action(cx.listener(|this, _: &LeaveChat, window, cx| this.chat_menu_action(ChatMenuOperation::Leave, window, cx)))))
            .on_action(cx.listener(|this, _: &CloseAllPanels, window, cx| {
                DockArea::close_all(&this.dock, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ReopenClosedPanel, window, cx| {
                this.dock
                    .update(cx, |dock, cx| dock.reopen_closed(window, cx))
            }))
            .on_action(cx.listener(|this, _: &ClosePanel, window, cx| {
                if let Some(tab) = this.dock.read(cx).active_tab_group(window, cx) {
                    tab.update(cx, |tab, cx| {
                        if let Some(panel) = tab.active_panel(cx) {
                            tab.remove_panel(&panel, window, cx);
                        }
                    });
                }
            }))
            .on_action(
                cx.listener(|this, _: &NextPanel, window, cx| this.cycle_tabs(false, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &PreviousPanel, window, cx| {
                    this.cycle_tabs(true, window, cx)
                }),
            )
            .relative()
            .size_full()
            .child(
                image_cache(self.image_cache.clone())
                    .relative()
                    .size_full()
                    .child(
                        v_flex()
                            .size_full()
                            // Title Bar
                            .child(
                                TitleBar::new()
                                    .child(self.titlebar_left(cx))
                                    .child(self.titlebar_right(cx)),
                            )
                            // Main
                            .child(div().flex_1().min_h_0().w_full().child(self.dock.clone())),
                    ),
            )
            // Notifications
            .children(notification_layer)
            // Modals
            .children(modal_layer)
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub mod qr_camera;
