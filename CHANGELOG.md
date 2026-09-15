# Changelog

All notable changes to Goop are documented here.

The format follows [Keep a Changelog 1.0.0](https://keepachangelog.com/en/1.0.0/),
and releases follow [Semantic Versioning 2.0.0](https://semver.org/).

## [Unreleased]

## [2.5.0] - 2026-09-15

### Added

- Add a vertical three-dot menu on the first chat-list heading with “Mark all as read” for the current list.
- Add right-click Pin/Unpin, Archive/Unarchive, and Mark as read/Mark as unread actions to chat rows. Manual unread marks persist on this device and remain visible on an open chat until it is refocused or displays new incoming messages.

- Add “View profile” to one-to-one chat context menus, opening the profile in the right sidepane.

- Add Chat and Search buttons at the bottom of profiles; Search opens the conversation and focuses find-in-chat.

### Changed

- Put profile public keys on their own full-width row with a dedicated copy button.
- Give the message composer a distinct rounded background and roomier padding. It grows with new lines and wrapped text, keeping its controls aligned to the bottom.

### Fixed

- Keep the composer visible in short windows by letting message history shrink and scroll; limit draft growth to the available window height.

## [2.4.0] - 2026-09-15

Goop 2.4 adds Note to self, Amethyst-compatible pinned chats, and local unread counts, with smoother keyboard navigation and a quieter sidebar. Pin compatibility is checked against Amethyst's source and relay fixtures; live cross-client testing remains pending.

### Added

- Open Note to self from the sidebar or Cmd/Ctrl+Shift+S. Find your account by typing “self” in conversation search, profile search, or New Chat.
- Pin chats above the date groups and sync pins through Amethyst's self-encrypted kind-30078 `AmethystSettings` document, preserving other settings fields.
- Show blue unread-count badges on chats and an Inbox dot when an Inbox chat has unread incoming messages. Read positions persist on this device; sent messages and reactions do not count. Opening a chat in the active window marks its loaded messages read. Existing history starts unread until viewed; no read receipts are published.

### Changed

- Show pin and archive buttons only while hovering a chat row, replacing the overflow button. Group Leave/Rejoin remains available by right-clicking the row.
- Move Archived into the footer between Relays and Help, widen the default sidebar to 280 px, and add a divider below Inbox/Requests.
- Align chat actions against a fixed-width time column, keeping them steady as duration labels change.
- Use “Start a new chat” and “Search past conversations” on Get Started. Replace the README shortcut list with a link to the user manual.

### Fixed

- Focus the message box when opening or switching to a chat, including an already-open Note to self tab.
- Open profile-search results in the right sidepane and show conversation-request wording only in request dialogs.
- Restore sidebar focus before opening Settings so clicking the gear after Help works.
- Show the Requests dot only for unseen requests still in the request list; ordinary chat activity cannot trigger it.
- Restore Welcome after the final center tab is closed, including Close All Tabs.
- Refresh unread counts when new messages leave chat ordering unchanged and when the application regains focus.

## [2.3.0] - 2026-09-15

Goop 2.3 adds interoperable chat archiving, local group leave/rejoin, clearer delivery indicators and safer reporting. Archive compatibility is tested against Nospeak’s source format and local relay fixtures; live cross-app testing remains pending.

### Added

- Archive and unarchive chats using Nospeak-compatible, self-encrypted `kind:30000` / `dm-archive` lists, with an Archived view and retryable cross-device sync.
- Leave group chats locally after confirmation, preserving history and suppressing notifications on this device. Rejoin from Archived; other clients may still send messages.
- Group conversations by Today, Yesterday, Last 7 Days, and Older using the local calendar date of their latest message.
- Show single and double checkmarks for relay delivery, following Amethyst's per-recipient acknowledgement semantics. These are not read receipts.
- Add Rebroadcast to sent messages, reusing their original stored signed gift wraps without creating new messages.

### Fixed

- Reload open messaging and gossip relay panels after signer connection, with loading states and protection for unsaved edits.
- Label the report action with a flag icon and require confirmation identifying the person, reason, and public nature of the report.

## [2.2.0] - 2026-09-14

Goop 2.2 encrypts new chat attachments before uploading them and adds message-content search, drag-and-drop attachments, and notification copying.

Encrypted attachments require a server that accepts binary blobs and a client that supports NIP-17 kind-15 file messages. Dark Wisp compatibility has been checked against its source and regression fixtures; live cross-client testing remains pending.

### Security

- Encrypt chat attachments locally with per-file AES-256-GCM keys before Blossom uploads. Send keys and metadata only inside NIP-17 kind-15 gift-wrapped messages; authenticate and decrypt received files locally.
- Authorize encrypted attachment uploads with the logged-in signer for account-whitelisted Blossom servers. Servers can identify the uploader but cannot decrypt the file. Public profile-picture uploads remain separate.
- Previously uploaded unencrypted attachments remain unencrypted; removing an attachment from a draft does not delete its server copy.

### Added

- Copy notification titles and messages to the clipboard directly from the notification popover.
- Cmd/Ctrl+K searches loaded message contents as well as names, with name matches first and snippets for content matches.
- Drop files onto a chat to attach them to the draft, using the same upload flow as the + picker; multiple files upload in order.
- A pinned sidebar footer with New Group, Relays, Help, and Settings; Cmd/Ctrl+Shift+N opens group creation directly.
- Hold Cmd on macOS or Ctrl on Windows/Linux to show numbered hints on Inbox, Requests, and the message box.
- Shortcuts for Contacts (Cmd/Ctrl+Shift+C), Profile (Cmd/Ctrl+Shift+P), Messaging Relays (Cmd/Ctrl+Shift+M), and Gossip Relays (Cmd/Ctrl+Shift+G), shown in menus and shortcut help.
- Open the agent messaging guide from “Set up your agents to message you” in Get Started.

### Changed

- Display modifier-held navigation hints as standard shortcut keycaps overlaid on their targets, without reserving layout space.
- Highlight the chat with a labeled file drop zone and show the current upload filename and queued attachment count by the composer.
- Simplify conversation and profile search dialogs with descriptive placeholders and no title bar or close button.
- Align delivery-status recipients in padded rows and move retry into the dialog footer; allow error details to size to their content.
- Open the online usage guide from Help → Usage Guide.

### Fixed

- Accept encrypted attachments with Dark Wisp's original-file `size` tag and emit the same size convention; retain support for earlier Goop attachments.
- Give draft attachments a working remove button with its own hit area; clicking the thumbnail opens a large image preview instead of removing it.
- Refresh open delivery-status dialogs as recipients progress, including relay results, queued/paused states, and the retry button.
- Include newly added icons in incremental builds so the Agent Guide robot icon appears.

## [2.1.0] - 2026-09-14

Goop 2.1 makes the desktop interface calmer and easier to use with a keyboard.
Existing identities, settings, and message storage remain compatible.

### Added

- Open keyboard shortcut help with ? outside inputs or through Help. Get Started now offers New Chat, Search Conversations, and Keyboard Shortcuts with their bindings.
- Navigate New Chat with Up/Down and Enter, matching conversation search. Enter selects recipients in group mode.
- Find in the current chat with Cmd/Ctrl+F or its toolbar button. Matching text is highlighted; Enter and Shift+Enter move between matching messages, and Escape restores focus. Search covers decrypted messages already available in the chat and updates as history arrives.
- Native File, Edit, View, Window, and Help menus, standard editing and window actions, and an About dialog showing the version and build.
- A Message history toolbar button that shows or hides history status and recovery controls.

### Changed

- Replace the theme collection with fixed Light and Dark palettes and a System preference, enabled by default. Existing theme choices migrate to System while other settings are retained.
- Use one resizable left sidebar for Inbox/Requests, toggled with Cmd/Ctrl+B. Profile, contact, and relay settings open in the right dock without replacing other configuration tabs.
- Replace the sidebar search box with New Chat and Search actions with shortcut badges. Inbox and Requests keep their labels and positions. Cmd/Ctrl+K opens conversation search; Cmd/Ctrl+P searches cached profiles.
- Move contact selection and group creation into New Chat, opened with Cmd/Ctrl+N or Cmd/Ctrl+T. Typing filters contacts locally; explicit name@domain addresses, npubs, and nprofiles resolve directly without global relay text search.
- Rename the broader relay action to “Broaden message scan to other relays” and place it after “Retry failed decryptions”.

### Fixed

- Show the profile menu before the loading indicator in the title bar.
- Hide a sidebar when its final tab closes, including its resize handle and empty toggle. Reopening a tab restores its sidebar.
- Selecting a person in New Chat immediately opens the existing conversation or starts a new chat, without a message-request dialog. Group recipient selection is available through New group chat.
- Find contacts by the names already displayed in New Chat, without failed relay searches. Preserve recipient selections while changing the filter.
- Show conversation loading once in the title bar; remove the duplicate sidebar overlay.
- Remove the unnecessary “Loaded items only” search-footer label.

**Validation and limitations:** Regression coverage includes contact-query classification,
local filtering, literal and Unicode chat matching, match navigation, Markdown
highlighting, and existing messaging recovery. Full interactive GUI smoke tests on
every platform remain incomplete. Find does not search messages the app has not
retrieved and decrypted. Existing relay-retention and cross-client limitations from
2.0.0 still apply. macOS bundles are ad-hoc signed, not notarized; Windows installers
do not carry a developer certificate.

## [2.0.0] - 2026-09-14

Goop 2.0 focuses on reliable NIP-17 messaging and fast keyboard navigation.

**Upgrading:** Identity login now requires an external signer. If you previously
used a plain secret key, configure that identity in your signer before upgrading.
Keep an independent backup of your identity; Goop no longer offers identity backup
or secret-key login. Existing bunker connections are retained and retry temporary
connection failures.

The first launch may take longer while Goop rebuilds verified message caches from
retained encrypted events. If conversations are missing, use **Message History →
Rescan all history**, or **Search other configured relays** for additional copies.
Broader searches query your published general relays and add network traffic;
outgoing messages still use inbox relays. NIP-04 is not supported.

**Known limitations:** History coverage depends on what relays retain and return;
“History checked” is not proof of complete account history. Timestamp ties under
restrictive relay limits remain a coverage edge case. Full cross-client validation
of encrypted attachments and groups, and GUI smoke tests on every target platform,
remain incomplete; this release does not claim parity with Dark Wisp.

**Downloads:** Choose arm64 for Apple Silicon/ARM or x64 for Intel/AMD. macOS and
Windows installers lack developer-certificate signing; macOS is not notarized.
Local Snap packages require `snap install --dangerous --classic <file.snap>`.
`SHA256SUMS` lists checksums for all downloads.

### Added

- Semantic Versioning policy, a Keep a Changelog history, and release validation that uses changelog entries for release notes.
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

[Unreleased]: https://github.com/dergigi/goop/compare/v2.4.0...HEAD
[2.4.0]: https://github.com/dergigi/goop/compare/v2.3.0...v2.4.0
[2.3.0]: https://github.com/dergigi/goop/compare/v2.2.0...v2.3.0
[2.2.0]: https://github.com/dergigi/goop/compare/v2.1.0...v2.2.0
[2.1.0]: https://github.com/dergigi/goop/compare/v2.0.0...v2.1.0
[2.0.0]: https://github.com/dergigi/goop/compare/v1.1.0...v2.0.0
[1.1.0]: https://github.com/dergigi/goop/releases/tag/v1.1.0
