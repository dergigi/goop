use gpui::Action;
use nostr_sdk::prelude::*;
use serde::Deserialize;
use settings::SignerKind;

#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = chat, no_json)]
pub enum Command {
    Find,
    Insert(&'static str),
    ChangeSubject(String),
    ChangeSigner(SignerKind),
    ToggleBackup,
    Copy(PublicKey),
    Relays(PublicKey),
    Njump(PublicKey),
    Trace(EventId),
}
