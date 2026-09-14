# DM reliability release checklist

Release Goop for macOS, Windows, and Linux after the remaining DM improvements and validation are complete. This is the follow-up to the [Dark Wisp review](dark-wisp-nip17.md). Source inspection and unit tests alone do not establish equivalent real-world reliability.

The agreed scope remains NIP-17, external identity signers, and history from the account's current inbox relays. Do not add NIP-04 or separate history relays as coverage workarounds.

## Implemented and checked

- [x] Relay acknowledgement tracking starts before publication; queued transport is not mistaken for acceptance (`28ce4fe`).
- [x] Validate seal structure, signature, author consistency, and supplied rumor IDs before caching (`9131af0`).
- [x] Reserve signer capacity for live messages and explicit retries during history backfill (`070e0bd`).
- [x] Keep history requests alive through AUTH-required responses, with failure/checkpoint recovery tests (`a4eaab3`).
- [x] Persist outgoing intents before signing and wraps before publishing. Restore unfinished sends, preserve IDs, and retry failed relays independently (`8aea9a0`).
- [x] Attempt the sender copy independently of recipient delivery; display partial delivery and retry controls.
- [x] Keep outgoing jobs and queued plaintext account-scoped and separate from the relay database. Stop work on account changes and freeze in-flight signing identity (`988b4a3`).

The outgoing implementation is covered by disk-backed restart tests, controlled relay rejection, signer interruption, account-switch cancellation, and prevention of relay-database queue injection. The chat suite currently has 29 passing tests. The desktop application passes `cargo check --locked -p goop --features gpui_macos/runtime_shaders` on the development Mac. It has not yet been rebuilt and installed with these changes.

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
- [ ] Build and install a release candidate on the development Mac for interactive testing.
- [ ] Build and smoke-test the macOS, Windows, and Linux artifacts through the release workflow; verify retained user data and queue files across upgrade.
- [ ] Set the release version, document the changes and any known limitations, publish artifacts, and verify downloads/update metadata.

Do not publish solely because the implementation checklist is complete. The release should follow the interoperability, recovery, and platform checks above.

Incoming cache migration preserves raw encrypted events and rebuilds verified account-scoped records by replaying them through the signer. Legacy plaintext records lack reliable account/provenance information and are not read. The first upgraded startup can therefore require extra decryption work. Regression tests cover legacy replay, untrusted cache records, account isolation, concurrent duplicate wraps, and group reactions arriving before their target.

Signer refusal recovery is covered for incoming worker restarts and outgoing queue restarts/reconnection, with one request before refusal and no additional signing until explicit retry. The state suite has 6 passing tests, including typed signer-error classification. No real external-signer or cross-client interactive test has been claimed.

History checkpoints now require the persistent local provenance key as well as their account/relay identifier. Old or unrelated signed database records cannot mark history complete or replace trusted progress. Untrusted legacy checkpoints are ignored, causing a fresh scan of the existing inbox relays; retained ciphertext and the verified rumor cache prevent message loss/duplication during this rescan. Tests cover checkpoint injection and persistence/private permissions of the local cache key.
