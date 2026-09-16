# Startup and history performance review

Reviewed after 2.10.0, following reports of intermittent macOS beachballs during loading.

## Changes

- Bound the background-to-UI signal queue to 256 entries. Producers await capacity instead of accumulating an unlimited queue. The foreground consumer yields after 32 events or 4 ms of processing, allowing input and painting between bursts. This is a cooperative budget, not a hard maximum for an individual event.
- Load cached chat messages by the existing account and room tags instead of parsing and signature-verifying every conversation each time a tab opens. Older cache records remain supported. Reactions still require a separate account-wide reaction query; target matching includes locally queued outgoing messages.
- Parse loaded message media and mentions on the background executor, then insert into the panel in batches of 64 with a yield between batches. Each panel retains one history-loading task, replacing an obsolete reload.
- Count unread messages by visiting only candidates at or after the read position, without allocating a filtered copy of the complete conversation history.
- Cache the effective block list as an immutable shared set, rebuilt on load or mutation. Rendering and unread checks no longer parse all public/private block tags repeatedly. Pending edits, self exclusion, account isolation, and persistence retain their existing semantics.

## Validation

The chat, chat UI, and workspace suites cover cache provenance/account isolation, outgoing-target reactions, blocked authors, persistence, gallery, keyboard behavior, and status rendering. Added regressions check:

- Room-scoped loading matches the previous result, including reactions to outgoing-only messages, without parsing an unrelated malformed chat record or indexing unrelated plaintext.
- A 100,000-message conversation with 10 unread messages checks exactly 10 visibility candidates; manual unread remains supported.
- Block snapshots are shared between reads, remain immutable across edits, and reconstruct correctly after restart.

Validation passed: 66 chat tests (65 in the combined run plus the newly added snapshot regression), 22 chat UI tests, and 36 workspace tests. The optimized macOS application build passed with `cargo build --locked --release -p goop --features gpui_macos/runtime_shaders`. A plain `cargo check` initially failed because this machine lacks the offline Metal compiler; the production build uses the repository's configured runtime-shader feature.

These are operation-count and correctness checks, not measured end-to-end startup latency or RSS improvements. No captured stack trace yet establishes the cause of the reported beachball.

## Remaining investigation

1. Capture a macOS process sample during the actual stall; measure time to interactive and resident memory with a representative large account, before and after these changes. Do not include message content or credentials in diagnostic logs.
2. Database initialization still uses foreground `block_on`. Move it behind an asynchronous startup state with proper error/retry handling.
3. Read-position, Inbox, archive, and moderation persistence still contain synchronous filesystem writes. Move writes off the foreground thread without losing ordering or durability on logout/quit.
4. Add a backward-compatible reaction-target index and page large conversation histories; current startup still indexes all cached message text for search.
5. Coalesce contact-triggered room refreshes and profile/status invalidations. Profile dispatch also needs a cooperative foreground budget under sustained traffic.
