# Moderation interoperability

Checked against upstream sources on 2026-09-15.

- [NIP-51](https://github.com/nostr-protocol/nips/blob/master/51.md) defines kind 10000 mute lists, public tags, and self-encrypted private tags. A `p` tag identifies a user. Kind 10006 blocks relays, not accounts.
- [Amethyst MuteListState](https://github.com/vitorpamplona/amethyst/blob/main/amethyst/src/main/java/com/vitorpamplona/amethyst/model/nip51Lists/muteList/MuteListState.kt) adds hidden users with `isPrivate = true` and removes them through MuteListEvent.
- [Amethyst MuteListEvent](https://github.com/vitorpamplona/amethyst/blob/main/quartz/src/commonMain/kotlin/com/vitorpamplona/quartz/nip51Lists/muteList/MuteListEvent.kt) uses kind 10000.
- [Amethyst private-tag encryption](https://github.com/vitorpamplona/amethyst/blob/main/quartz/src/commonMain/kotlin/com/vitorpamplona/quartz/nip51Lists/encryption/PrivateTagsInContent.kt) writes NIP-44-encrypted JSON tag arrays. Empty decrypted content is also accepted by Goop.
- [NIP-56](https://github.com/nostr-protocol/nips/blob/master/56.md) defines public kind-1984 reports and seven reasons: spam, impersonation, malware, nudity, profanity, illegal, and other.

Goop calls the account-list action **Block**, reserving **Mute** for local timed notification suppression. It imports public and private user entries, preserves unrelated tags, and adds new blocks privately. Unblocking removes the user's entries from both halves. It does not apply hashtag, word, or event entries as user blocks, but preserves them.

Sync discovers the latest list before editing, preserves pending local edits across discovery, and requires a relay acknowledgement before marking an edit synced. Read failures are not interpreted as an empty remote list. Signer refusals/timeouts pause retries. Account changes stop the old worker and retain separate persisted state. Legacy NIP-04 private lists produce an actionable error and are never overwritten.

Tests use local relay fixtures to check private/public merge, another client's updates, unknown-tag preservation, denied reads, rejected writes, and legacy-list preservation. Additional tests cover account isolation, local restart, mute expiry, direct versus group filtering, and blocked-author unread counts. No live accounts are reported or blocked by tests.
