# DM reliability release checklist

Release Goop for macOS, Windows, and Linux after the remaining DM improvements and validation are complete. This is the follow-up to the [Dark Wisp review](dark-wisp-nip17.md). Source inspection and unit tests alone do not establish equivalent real-world reliability.

The agreed scope remains NIP-17, external identity signers, and history from the account's current inbox relays, with an explicit recovery search of the account's published general-relay list. Do not add NIP-04 or separate history relays as coverage workarounds.

## Implemented and checked

- [x] Relay acknowledgement tracking starts before publication; queued transport is not mistaken for acceptance (`28ce4fe`).
- [x] Validate seal structure, signature, author consistency, and supplied rumor IDs before caching (`9131af0`).
- [x] Reserve signer capacity for live messages and explicit retries during history backfill (`070e0bd`).
- [x] Keep history requests alive through AUTH-required responses, with failure/checkpoint recovery tests (`a4eaab3`).
- [x] Persist outgoing intents before signing and wraps before publishing. Restore unfinished sends, preserve IDs, and retry failed relays independently (`8aea9a0`).
- [x] Attempt the sender copy independently of recipient delivery; display partial delivery and retry controls.
- [x] Keep outgoing jobs and queued plaintext account-scoped and separate from the relay database. Stop work on account changes and freeze in-flight signing identity (`988b4a3`).

The outgoing implementation is covered by disk-backed restart tests, controlled relay rejection, signer interruption, account-switch cancellation, and prevention of relay-database queue injection. The chat suite currently has 38 passing tests. The desktop application passes `cargo check --locked -p goop --features gpui_macos/runtime_shaders` on the development Mac. A development candidate containing these changes has been rebuilt and installed on the development Mac; see the validation record below.

## Remaining implementation and regression coverage

- [x] Classify signer rejection/cancellation separately from disconnects and timeouts. Persist paused incoming wraps and outgoing intents across restarts; only explicit retry resumes refused work. Transport cancellation text is reported as cancellation when supplied; otherwise it is a rejection.
- [x] Scope decrypted incoming messages to the account and locally signed cache provenance; deduplicate by validated rumor ID across gift wraps, including after restart.
- [x] Store actual rumor kinds; route reactions through their target message, retain reactions arriving before their targets, and reject unsupported payloads before room creation.
- [ ] Expose current inbox relay connection, authentication, history, and delivery failures in a useful diagnostic view.
- [ ] Extend history tests for disconnects during a page, inbox-list changes during a scan, and timestamp ties under a server-imposed cap smaller than the request. Do not label query exhaustion as proof of complete account history.
- [ ] Validate consistent profile refresh and avatars across sidebar, tabs, and conversations during backfill. Measure missing metadata separately from failed image downloads or view refreshes.
- [ ] Check encrypted-file and group-message interoperability, including recipient tags, replies, and reactions; close the relevant gaps before claiming DM feature parity.

## Release validation

- [ ] Exercise Goop against Dark Wisp and another NIP-17 client with equivalent accounts, retained messages, inbox configuration, and external signers. Include new conversations, groups, self-copies, old history, replies, reactions, and attachments.
- [ ] Exercise signer refusal/reconnection, unavailable relays, partial delivery, forced application exit, reopening the queue, and account switching.
- [ ] Measure cold/warm startup and live-message latency during a large history import. Check outgoing-queue disk/memory overhead with a large completed history.
- [x] Build and install a release candidate on the development Mac for interactive testing.
- [ ] Build and smoke-test the macOS, Windows, and Linux artifacts through the release workflow; verify retained user data and queue files across upgrade.
- [x] Set the release version, document the changes and any known limitations, publish artifacts, and verify downloads/update metadata.

Do not publish solely because the implementation checklist is complete. The release should follow the interoperability, recovery, and platform checks above.

Incoming cache migration preserves raw encrypted events and rebuilds verified account-scoped records by replaying them through the signer. Legacy plaintext records lack reliable account/provenance information and are not read. The first upgraded startup can therefore require extra decryption work. Regression tests cover legacy replay, untrusted cache records, account isolation, concurrent duplicate wraps, and group reactions arriving before their target.

Signer refusal recovery is covered for incoming worker restarts and outgoing queue restarts/reconnection, with one request before refusal and no additional signing until explicit retry. The state suite has 6 passing tests, including typed signer-error classification. No real external-signer or cross-client interactive test has been claimed.

History checkpoints now require the persistent local provenance key as well as their account/relay identifier. Old or unrelated signed database records cannot mark history complete or replace trusted progress. Untrusted legacy checkpoints are ignored, causing a fresh scan of the existing inbox relays; retained ciphertext and the verified rumor cache prevent message loss/duplication during this rescan. Tests cover checkpoint injection and persistence/private permissions of the local cache key.

## Development candidate validation

- Installed Goop 1.1.0 build `d47b65d` in `/Applications/Goop.app` with the new incoming cache, persistent signer pauses, and trusted history checkpoints.
- `cargo test --locked -p chat --lib`: 29 passed.
- `cargo test --locked -p state --lib`: 6 passed.
- Full desktop check and optimized build passed with `--features gpui_macos/runtime_shaders`.
- Ad hoc bundle signature verified; installed binary matches the built binary. Previous bundle preserved under `dist/previous-install-iia4rh3c/Goop.app`.
- The running app was left open; interactive verification requires quitting and reopening it.
- CI now explicitly runs chat/state recovery tests on macOS, Windows, and Linux; default workspace tests previously covered only the desktop package.
- [Six-platform installer validation](https://github.com/dergigi/goop/actions/runs/34866724438) started at `62284dc` with draft-release creation disabled. Build and platform smoke-test results are not yet claimed.
- Diagnostics, additional history boundary/disconnection cases, avatar/performance measurements, encrypted-file interoperability, real external-signer/cross-client exercises, and release publication remain open. This development install is not the final release candidate.

## Additional development checks

- History downloads now save ciphertext and queue IDs without waiting for signer throughput. A regression downloads 600 wraps without an active decrypt worker.
- “Rescan all history” revisits previously scanned ranges; “Search other configured relays” additionally scans the account’s published general relays, with two concurrent scans and duplicate suppression. Sending still uses inbox relays. Requests made during a scan are queued visibly.
- Transient decryption failures retry with backoff; signer refusals remain paused until explicit retry. The history menu groups failures by reason.
- Local conversation/profile quick search uses loaded snapshots and performs no database or relay queries while typing. Matching tests and the desktop compile check pass; interactive keyboard/focus checks remain part of candidate testing.

## Goop 2.0.0 publication

[Goop 2.0.0](https://github.com/dergigi/goop/releases/tag/v2.0.0) is published for all six platform/architecture targets. See the [validation record](../releases/2.0.0-validation.md) for tests, installer checks, Mac signing corrections, checksums, and update metadata verification. Unchecked interoperability, performance, and GUI smoke-test items above remain open and are disclosed as limitations; publication does not establish Dark Wisp parity.
