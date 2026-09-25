use gpui::Action;
use nostr_sdk::prelude::*;
use serde::Deserialize;

#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = chat, no_json)]
pub enum Command {
    Find,
    ChangeSubject(String),
    ToggleBackup,
    Copy(PublicKey),
    ViewProfile(PublicKey),
    CopyMessage(EventId),
    Reply(EventId),
    Relays(PublicKey),
    Njump(PublicKey),
    Trace(EventId),
    Rebroadcast(EventId),
}

#[derive(Action, Clone, Default, PartialEq, Eq, Deserialize)]
#[action(namespace = chat, no_json)]
pub struct ShowConnectionStatus;

/// Focus the active chat's message composer from anywhere in the workspace.
#[derive(Action, Clone, Default, PartialEq, Eq, Deserialize)]
#[action(namespace = chat, no_json)]
pub struct FocusComposer;
