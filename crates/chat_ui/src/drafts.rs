//! Account-scoped composer drafts, written by the shared background writer.
use std::{
    collections::BTreeSet,
    io,
    path::{Path, PathBuf},
};

use common::persistence::Persistence;
use nostr_sdk::prelude::{EventId, PublicKey};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Snapshot {
    pub text: String,
    pub replies: BTreeSet<EventId>,
}

pub(super) struct Draft {
    pub path: PathBuf,
    snapshot: Snapshot,
    edited: bool,
}

impl Draft {
    pub fn new(root: &Path, owner: PublicKey, room: u64) -> Self {
        Self {
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
