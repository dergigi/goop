//! Account-scoped composer drafts, written by the shared background writer.
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::{Path, PathBuf},
};

use common::persistence::Persistence;
use gpui::{App, AppContext, Context, Entity, Global};
use nostr_sdk::prelude::{EventId, PublicKey};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Snapshot {
    pub text: String,
    pub replies: BTreeSet<EventId>,
}

pub(super) struct Draft {
    pub path: PathBuf,
    pub owner: PublicKey,
    pub room: u64,
    snapshot: Snapshot,
    edited: bool,
}

impl Draft {
    pub fn new(root: &Path, owner: PublicKey, room: u64) -> Self {
        Self {
            owner,
            room,
            path: root
                .join("drafts")
                .join(owner.to_hex())
                .join(format!("{room}.json")),
            snapshot: Snapshot::default(),
            edited: false,
        }
    }

    pub fn save(&mut self, snapshot: Snapshot, writer: &Persistence) -> io::Result<()> {
        if snapshot == self.snapshot {
            return Ok(());
        }
        self.edited = true;
        // Only remember an accepted snapshot, so a rejected enqueue can be retried.
        writer.save(&self.path, snapshot.clone())?;
        self.snapshot = snapshot;
        Ok(())
    }

    pub fn restore(&mut self, snapshot: &Snapshot, current: &Snapshot) -> bool {
        if self.edited || current != &Snapshot::default() {
            return false;
        }
        self.snapshot = snapshot.clone();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr_sdk::prelude::Keys;

    #[test]
    fn disk_scan_preserves_live_edits_and_keeps_accounts_separate() {
        let owner = Keys::generate().public_key();
        let other = Keys::generate().public_key();
        let mut indicators = DraftIndicators::default();
        // A draft was cleared, and another was typed, while startup loading ran.
        indicators.values.insert((owner, 1), false);
        indicators.values.insert((owner, 2), true);
        indicators.merge_loaded([
            ((owner, 1), true), ((owner, 2), false), ((owner, 3), true),
        ].into());
        assert!(!indicators.has_draft(owner, 1));
        assert!(indicators.has_draft(owner, 2));
        assert!(indicators.has_draft(owner, 3));
        assert!(!indicators.has_draft(other, 3));
    }

    #[test]
    fn drafts_survive_restart_and_are_isolated_by_account_and_chat() {
        let dir = tempfile::tempdir().unwrap();
        let owner = Keys::generate().public_key();
        let mut draft = Draft::new(dir.path(), owner, 1);
        let snapshot = Snapshot {
            text: "  a long draft\nwith whitespace 🧡\n".into(),
            replies: [EventId::from_byte_array([0; 32])].into(),
        };
        let writer = Persistence::new().unwrap();
        draft.save(snapshot.clone(), &writer).unwrap();
        writer.flush_blocking().unwrap();
        drop(writer);
        let restarted = Persistence::new().unwrap();
        assert_eq!(restarted.load::<Snapshot>(&draft.path).unwrap(), snapshot);
        for other in [
            Draft::new(dir.path(), owner, 2),
            Draft::new(dir.path(), Keys::generate().public_key(), 1),
        ] {
            assert_eq!(
                restarted.load::<Snapshot>(&other.path).unwrap(),
                Snapshot::default()
            );
        }
        draft.save(Snapshot::default(), &restarted).unwrap();
        restarted.flush_blocking().unwrap();
        assert_eq!(
            restarted.load::<Snapshot>(&draft.path).unwrap(),
            Snapshot::default()
        );
    }

    #[test]
    fn pending_writes_survive_reopening_and_restore_never_overwrites_edits() {
        let dir = tempfile::tempdir().unwrap();
        let writer = Persistence::new().unwrap();
        let owner = Keys::generate().public_key();
        let mut draft = Draft::new(dir.path(), owner, 1);
        let text = Snapshot {
            text: "new input".into(),
            ..Snapshot::default()
        };
        draft.save(text.clone(), &writer).unwrap();
        assert_eq!(writer.load::<Snapshot>(&draft.path).unwrap(), text);
        assert!(!draft.restore(&Snapshot::default(), &text));
        draft.save(Snapshot::default(), &writer).unwrap();
        assert!(!draft.restore(&text, &Snapshot::default()));
        let mut fresh = Draft::new(dir.path(), owner, 2);
        assert!(!fresh.restore(&Snapshot::default(), &text));
        assert!(fresh.restore(&text, &Snapshot::default()));
        writer.flush_blocking().unwrap();
    }

    #[test]
    fn failed_disk_write_retains_latest_draft_for_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("not-a-directory");
        std::fs::write(&root, "occupied").unwrap();
        let writer = Persistence::new().unwrap();
        let mut draft = Draft::new(&root, Keys::generate().public_key(), 1);
        let snapshot = Snapshot {
            text: "don't lose me".into(),
            ..Snapshot::default()
        };
        draft.save(snapshot.clone(), &writer).unwrap();
        assert!(writer.flush_blocking().is_err());
        assert_eq!(writer.load::<Snapshot>(&draft.path).unwrap(), snapshot);
        std::fs::remove_file(&root).unwrap();
        writer.flush_blocking().unwrap();
        let restarted = Persistence::new().unwrap();
        assert_eq!(restarted.load::<Snapshot>(&draft.path).unwrap(), snapshot);
    }
}


#[derive(Default)]
pub struct DraftIndicators {
    values: BTreeMap<(PublicKey, u64), bool>,
    loaded: BTreeSet<PublicKey>,
}

struct GlobalDraftIndicators(Entity<DraftIndicators>);
impl Global for GlobalDraftIndicators {}

impl DraftIndicators {
    pub fn global(cx: &mut App) -> Entity<Self> {
        if !cx.has_global::<GlobalDraftIndicators>() {
            let indicators = cx.new(|_| Self::default());
            cx.set_global(GlobalDraftIndicators(indicators));
        }
        cx.global::<GlobalDraftIndicators>().0.clone()
    }

    pub fn has_draft(&self, owner: PublicKey, room: u64) -> bool {
        self.values.get(&(owner, room)).copied().unwrap_or(false)
    }

    pub(super) fn update_draft(&mut self, owner: PublicKey, room: u64, text: &str, cx: &mut Context<Self>) {
        let present = !text.trim().is_empty();
        if self.values.insert((owner, room), present) != Some(present) {
            cx.notify();
        }
    }

    pub fn load_account(&mut self, owner: PublicKey, cx: &mut Context<Self>) {
        if !self.loaded.insert(owner) { return; }
        let load = cx.background_executor().spawn(async move {
            let directory = common::support_dir().join("drafts").join(owner.to_hex());
            let mut values = BTreeMap::new();
            let entries = match std::fs::read_dir(directory) {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(values),
                Err(error) => return Err(error),
            };
            for entry in entries {
                let path = entry?.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") { continue; }
                let Some(room) = path.file_stem().and_then(|name| name.to_str()).and_then(|name| name.parse::<u64>().ok()) else { continue; };
                match common::persistence::global().load::<Snapshot>(&path) {
                    Ok(draft) => { values.insert((owner, room), !draft.text.trim().is_empty()); }
                    Err(error) => log::warn!("Could not read a draft indicator: {error}"),
                }
            }
            Ok::<_, io::Error>(values)
        });
        cx.spawn(async move |this, cx| {
            let result = load.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(values) => {
                    this.merge_loaded(values);
                    cx.notify();
                }
                Err(error) => {
                    this.loaded.remove(&owner);
                    log::warn!("Could not load draft indicators: {error}");
                }
            });
        }).detach();
    }

    fn merge_loaded(&mut self, values: BTreeMap<(PublicKey, u64), bool>) {
        for (key, value) in values {
            // Live edits (including clearing a draft) take precedence over the disk scan.
            self.values.entry(key).or_insert(value);
        }
    }
}
