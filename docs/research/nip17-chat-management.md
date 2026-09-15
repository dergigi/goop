# NIP-17 chat management and delivery indicators

Research date: 2026-09-15. Implemented after this research: Nospeak-compatible archiving and a local Leave/Rejoin with explicit user confirmation.

## Findings

### Archiving has existing Nostr-backed implementations

Nospeak syncs archives as an account-signed `kind:30000` event with the public tag `d=dm-archive`. Its content is a NIP-44 self-encrypted tag array: `p` plus the other user's hex public key for direct chats, or `e` plus its conversation identifier for groups. The current reader treats the newest remote list as authoritative, including removals. This differs from the older design document's proposed union merge, which would not reliably propagate unarchiving. [Nospeak ArchiveSyncService](https://github.com/psic4t/nospeak/blob/e94a4647caf81ac44f75b28c772dd44e0b9ca262/src/lib/core/ArchiveSyncService.ts)

Its group identifiers are the first 16 hex characters of SHA-256 over sorted participant hex keys, including self. Goop's existing local room identifier must not simply be exported as a compatible Nospeak identifier. [Nospeak ConversationRepository](https://github.com/psic4t/nospeak/blob/e94a4647caf81ac44f75b28c772dd44e0b9ca262/src/lib/db/ConversationRepository.ts)

This is concrete client prior art, not an archive schema standardized by NIP-17. NIP-51 standardizes list mechanics, but describes kind 30000 as follow sets; using group hashes in `e` tags is a client convention, not a standard event reference. [NIP-51](https://github.com/nostr-protocol/nips/blob/master/51.md)

A second option is encrypted application data. Amethyst signs `AmethystSettings` app-data events and self-encrypts their JSON with NIP-44, then restores preferences by decrypting its own events. This is prior art for private settings synchronization, not evidence of a shared archive format. [Amethyst AppSpecificState](https://github.com/vitorpamplona/amethyst/blob/e57b0e07540e2038e9eab0324e0a9efab3e9395c/amethyst/src/main/java/com/vitorpamplona/amethyst/model/nip78AppSpecific/AppSpecificState.kt)

NIP-78 provides kind 30078 for application-specific data, with a `d` identifier. It gives us a Nostr-native transport, but cross-client compatibility still requires agreement on the encrypted payload and conversation IDs. [NIP-78](https://github.com/nostr-protocol/nips/blob/master/78.md)

### Leaving is different from archiving

NIP-17 defines a room by its participants: the author plus the `p` tags. Removing a participant creates a new room. There are no administrators, invitations, bans, or standardized leave control messages. A local client can stop displaying or notifying about a conversation; it cannot force other clients to stop sending encrypted copies. Deletion is separate: the specification discusses recipient deletion of outer gift wraps and optional gift-wrapped deletion messages, neither of which guarantees every copy disappears. [NIP-17](https://github.com/nostr-protocol/nips/blob/master/17.md)

NIP-29 does define a kind-9022 leave request for relay-managed groups. That depends on a relay's group membership model and does not apply to Goop's existing NIP-17 participant-set conversations. [NIP-29](https://github.com/nostr-protocol/nips/blob/master/29.md)

The original Coop discussion asks for categorization, archiving and deletion, but proposes no agreed event schema. [Coop #50](https://github.com/lumehq/coop/issues/50), [original comment](https://github.com/lumehq/coop/issues/47#issuecomment-2912084711)

## Design findings

1. Offer reversible **Archive / Unarchive**, preserving history and providing an Archived view. Decide explicitly whether new messages unarchive a conversation.
2. Prefer the existing Nospeak convention if cross-client archive synchronization is the priority. Validate direct-chat and group-ID fixtures against Nospeak before claiming compatibility; preserve unknown entries when editing the shared list.
3. Otherwise use a versioned, self-encrypted NIP-78 document with canonical participant identities. Describe it as Goop settings sync, not a standard archive protocol.
4. Persist pending local changes and merge remote state before publishing. Handle offline edits, concurrent devices, explicit empty lists and account changes. Keep participant identifiers inside encrypted content.
5. For NIP-17 groups, offer **Stop following** or clearly explain that **Leave** only hides/silences the room locally. A peer-visible leave notification would need a documented extension and cooperating clients. Do not send one or delete history as a side effect of archiving.

## Delivery checkmarks

Amethyst tracks relay acknowledgements for each recipient's gift wrap. A single check means a copy was accepted somewhere; double checks require at least one relay acceptance for every other participant, excluding the sender's self-copy. This is not a device receipt or a read receipt. Goop's new indicators follow these rules and keep the detailed relay reports accessible. [Amethyst ChatDeliveryTracker](https://github.com/vitorpamplona/amethyst/blob/e57b0e07540e2038e9eab0324e0a9efab3e9395c/commons/src/commonMain/kotlin/com/vitorpamplona/amethyst/commons/relayClient/chatDelivery/ChatDeliveryTracker.kt)

## Implemented behavior

- Archive/Unarchive uses Nospeak's encrypted list and exact conversation-ID mapping, including the truncated group hash. Unknown tags are preserved when editing; legacy `e`/npub entries are recognized.
- Archived chats stay archived when messages arrive and remain accessible under Archived. New archive edits are persisted per account before synchronization, then applied to the newest retrieved remote list. Remote removals and explicit empty lists are meaningful.
- Sync polls while connected and wakes after edits. Signer refusals pause synchronization until an explicit Retry sync or new edit. Pending local edits survive restart. Concurrent publications still follow Nostr's last-write-wins behavior; this is not a conflict-free protocol.
- Leave locally also archives the group but stores its notification suppression only on this device. It preserves history and refuses new sends until Rejoin. Its confirmation explains that it cannot prevent peers sending messages.
- Tests cover direct and group IDs, unknown/legacy tags, account isolation, pending changes across restart, local Leave/Rejoin persistence, and two local clients exchanging encrypted archive state through a test relay. A live Nospeak-device test remains pending.
