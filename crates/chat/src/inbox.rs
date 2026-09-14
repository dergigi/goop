//! Account-local, durable evidence that a conversation belongs in Inbox.
use anyhow::Result;
use nostr_sdk::prelude::PublicKey;
use std::{
    collections::BTreeSet,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub(super) struct Inbox {
    path: PathBuf,
    rooms: BTreeSet<u64>,
}
impl Inbox {
    pub fn open(root: &Path, owner: PublicKey) -> Result<Self> {
        let path = root
            .join("goop-inbox-v1")
            .join(format!("{}.json", owner.to_hex()));
        let rooms = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeSet::new(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self { path, rooms })
    }
    pub fn contains(&self, room: u64) -> bool {
        self.rooms.contains(&room)
    }
    pub fn remember(&mut self, rooms: impl IntoIterator<Item = u64>) -> Result<()> {
        let mut updated = self.rooms.clone();
        updated.extend(rooms);
        if updated == self.rooms {
            return Ok(());
        }
        let dir = self.path.parent().unwrap();
        std::fs::create_dir_all(dir)?;
        let mut temp = tempfile::NamedTempFile::new_in(dir)?;
        temp.write_all(&serde_json::to_vec(&updated)?)?;
        temp.as_file().sync_all()?;
        temp.persist(&self.path).map_err(|e| e.error)?;
        #[cfg(unix)]
        std::fs::File::open(dir)?.sync_all()?;
        self.rooms = updated;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use nostr_sdk::prelude::Keys;
    #[test]
    fn acceptance_survives_restart_and_is_isolated_between_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let alice = Keys::generate().public_key();
        let bob = Keys::generate().public_key();
        let mut inbox = Inbox::open(dir.path(), alice).unwrap();
        inbox.remember([7, 9]).unwrap();
        inbox.remember([7]).unwrap();
        let restarted = Inbox::open(dir.path(), alice).unwrap();
        assert!(restarted.contains(7));
        assert!(restarted.contains(9));
        assert!(!Inbox::open(dir.path(), bob).unwrap().contains(7));
    }
}
