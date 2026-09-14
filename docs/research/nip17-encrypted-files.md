# Encrypted chat attachments

Implemented on 2026-09-15. This replaces Goop's inherited unencrypted chat attachment upload path. Public profile pictures use a separately named public upload function.

## Wire format

Goop follows [NIP-17 file messages](https://github.com/nostr-protocol/nips/blob/master/17.md#file-message) and the format in [Dark Wisp's EncryptedMedia.kt at dcd8eb7](https://github.com/barrydeen/dark-wisp-android/blob/dcd8eb7e015fd5bb62b32df45b76a7ffbfea5175/app/src/main/kotlin/com/darkwisp/app/nostr/EncryptedMedia.kt):

- AES-256-GCM, fresh random 32-byte key and 12-byte nonce for each file, 128-bit authentication tag appended to the ciphertext, no additional authenticated data.
- Blossom receives ciphertext as `application/octet-stream`. The logged-in signer authorizes uploads so account-whitelisted servers can accept them. The storage server can associate uploads with that account, but receives no decryption keys. Servers must accept arbitrary binary blobs; media-only servers may reject encrypted attachments with HTTP 415.
- One kind-15 rumor per file contains its URL and `file-type`, `encryption-algorithm`, `decryption-key`, `decryption-nonce`, `x`, `ox`, and `size` tags. Keys and nonces are hexadecimal. Participant, subject, and reply tags come from the conversation.
- `size` is the original plaintext length, matching Dark Wisp. Receiving also accepts the ciphertext length emitted by earlier Goop versions; hashes and AES-GCM authentication are verified in either case.
- Text accompanying files is a separate kind-14 message. Every rumor enters the durable outgoing queue and is sealed and gift-wrapped for each recipient and the sender. File metadata is not added to outer gift-wrap tags.
- Receiving verifies metadata, the ciphertext hash, AES-GCM authentication, and the original hash before displaying decrypted bytes. Missing or invalid encryption metadata never falls back to the ordinary URL image loader.
- Decrypted previews stay in memory. Saving plaintext to disk requires the user's explicit Save action. Downloads and uploads are bounded to 100 MB of plaintext and use HTTPS.

## Validation

Tests cover randomized ciphertext, key/nonce uniqueness, round trips, tampering, wrong keys, truncated ciphertext, invalid metadata, a NIST AES-256-GCM reference vector, and rejection of invalid kind-15 messages by the ordinary media renderer. A queue test persists a kind-15 message, reloads it, constructs gift wraps for the recipient and sender, unwraps both, and recovers the original file. It also checks that outer events expose neither the URL nor decryption tags.

The full state, chat, chat UI, and workspace test suites pass. This is source-format compatibility with Dark Wisp, not a claim of a live device-to-device Dark Wisp test.

Upload authorization was checked against [Dark Wisp's BlossomRepository.kt at dcd8eb7](https://github.com/barrydeen/dark-wisp-android/blob/dcd8eb7e015fd5bb62b32df45b76a7ffbfea5175/app/src/main/kotlin/com/darkwisp/app/repo/BlossomRepository.kt): encrypt locally, sign with the account, and PUT ciphertext to `/upload` as `application/octet-stream`. Goop retains its published server preference policy instead of adding an unlisted server after failures. A regression fixture covers Dark Wisp's original-size tags and legacy Goop ciphertext-size tags.

## Existing uploads

This change cannot encrypt files already uploaded in plaintext. Removing an attachment from a draft removes its reference, not the blob on the server. Pasted links to existing public images retain their existing visibility.
