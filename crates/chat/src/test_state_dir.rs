//! Keep test directories alive until all queued local-state writes reach disk.
pub struct StateDir(tempfile::TempDir);
impl StateDir {
    pub fn new() -> Self {
        Self(tempfile::tempdir().unwrap())
    }
    pub fn path(&self) -> &std::path::Path {
        self.0.path()
    }
}
impl Drop for StateDir {
    fn drop(&mut self) {
        common::persistence::global()
            .flush_blocking()
            .expect("flush test state before deleting its directory");
    }
}
