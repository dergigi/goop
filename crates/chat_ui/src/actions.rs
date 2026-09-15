use gpui::Action;
use nostr_sdk::prelude::*;
use serde::Deserialize;

#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = chat, no_json)]
pub enum Command {
    Find,
    Insert(&'static str),
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
