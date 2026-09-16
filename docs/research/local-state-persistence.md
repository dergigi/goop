# Local-state persistence

## Scope

Read positions, Inbox membership, archives/pins/left chats, blocks/mutes, and application settings now use `common::persistence`. Existing paths and JSON formats are preserved.

`save()` accepts an owned snapshot and returns immediately after queueing it. Serialization, directory creation, atomic file replacement, and filesystem sync all run on a dedicated thread. UI state updates immediately; a queued save is not yet a durable save.

## Ordering and memory

- A single worker orders disk writes. Each file has at most one queued snapshot plus the snapshot currently being written. New updates replace the queued snapshot, so bursts do not accumulate a chain of tasks or obsolete serialized files.
- Each accepted snapshot has a revision. Flush barriers wait for all revisions accepted before the barrier, or newer replacements. A failed file does not let a shutdown barrier skip other files that still need saving.
- Reopening an account consults pending snapshots before reading disk, preserving recent updates during fast account switches. Completed snapshots are released from memory.
- Archive and moderation shutdown serialize their active-state check with snapshot submission, preventing stopped store instances from submitting late writes.

## Failures and lifecycle

- Files are written to a private temporary file in the same directory, synced, then replaced atomically. On Unix the containing directory is synced too.
- Failed snapshots remain in memory. The worker retries when idle after five seconds, on a subsequent update, or on an explicit flush. Background errors appear as persistent notifications and duplicate unchanged errors are suppressed.
- Normal Quit and logout await a flush without blocking the UI. A failed flush prevents those actions from proceeding.
- Platform shutdown has a final synchronous barrier, with I/O still on the worker. This is intentional: the pinned GPUI version gives async quit hooks only 200 ms. The barrier rejects late producers, waits for all accepted files to be attempted, and logs failures if the platform is already terminating.
- Force termination, a crash, or a permanently unwritable disk can still prevent queued changes from reaching storage. Atomic replacement protects the previous complete file; it cannot make an unsuccessful write durable.
- Settings no longer silently ignore write errors or overwrite an unreadable settings file with defaults during startup.

## Validation

Controlled-worker tests cover delayed serialization/writes, 1,000 coalesced updates, account-file isolation, reopening pending state, obsolete in-flight failures, retry, simultaneous flushes, drain-on-drop, final shutdown, and one failed file alongside another still saving. Tests also verify serialization runs off the caller thread and Unix file permissions exclude other users.

The common, chat, settings, state, chat UI, and workspace suites passed (165 tests, with one crash subprocess fixture intentionally ignored). Representative store restart tests explicitly flush before reopening, and test directories stay alive until their queued writes finish.

This change targets writes. Some startup reads and LMDB initialization still run synchronously and remain candidates for profiling.
