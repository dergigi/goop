# Changelog

All notable changes to Goop are documented here.

The format follows [Keep a Changelog 1.0.0](https://keepachangelog.com/en/1.0.0/),
and releases follow [Semantic Versioning 2.0.0](https://semver.org/).

## [Unreleased]

### Added

- Instant local conversation and profile search with Cmd/Ctrl+K and Cmd/Ctrl+P, keyboard navigation, and discoverable shortcuts.
- Search, tab management, inbox/request navigation, composer focus, and reload keyboard shortcuts.
- Application version and build revision in Settings.
- Resumable NIP-17 history backfill, full rescans, history progress, failure details, and explicit recovery from configured general relays.
- Persistent outgoing messages with independent recipient/self-copy delivery tracking and retries after interruptions.

### Changed

- Profile links use njump.to.
- Message requests have a clear title and an Accept button.
- Visible profiles receive priority loading, cached metadata refreshes through outbox discovery, and avatars share an image cache.

### Removed

- **Breaking:** Plain secret-key identity login and identity backup menu; identity login requires an external signer.
- Panel zoom controls from overflow menus.

### Fixed

- Shift+Enter inserts a newline without sending the message.
- Search Escape restores the previous focus; closed tabs remain restorable after all tabs close and are cleared on sign-out.
- Accepted and previously active conversations retain their inbox classification during loading and across restarts.
- Saved signer connections retry temporary connection failures without signing the user out.
- History downloads no longer wait for signer throughput; rescans recover gaps, and recovery requests made during a scan are queued visibly.
- Transient decryption failures retry with backoff; explicit signer refusals stay paused across restarts until manually retried.
- History scans recover from relay authentication requests and reserve decryption capacity for live messages.
- Relay acknowledgement tracking begins before publication, and outgoing work stays isolated across account changes.

### Security

- Validate NIP-17 seals and rumor identifiers before caching, isolate incoming caches by account and local provenance, and route reactions through their target messages.
- Require local provenance for history checkpoints instead of trusting unrelated database records.

## [1.1.0] - 2026-09-14

First release of the Goop fork. Earlier Coop releases retain their upstream history.

### Added

- Markdown rendering in chat messages, with an appearance setting to enable or disable it.
- Account-published Blossom upload servers with the local service as a fallback.
- Cmd/Ctrl+, to open Settings.
- Collapsible Contacts and Results sections, startup loading indicators, and a search clear button.
- Goop application identity, branding, icons, and purple accents.

### Fixed

- Preserve Markdown line breaks and indentation and correct mention/link offsets after media extraction.

[Unreleased]: https://github.com/dergigi/goop/compare/v1.1.0...HEAD
[1.1.0]: https://github.com/dergigi/goop/releases/tag/v1.1.0
