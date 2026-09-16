---
layout: default
title: Usage guide
---

# Goop usage guide

A native NIP-17 client for you and your agents.

Designed for people who love the keyboard.
This guide covers Goop 2.1.

If NIP-17 isn’t good enough for you, give [WhiteNoise](https://www.whitenoise.chat/) a try.

[Agent Guide](#agent-guide) · [Keyboard shortcuts](#keyboard-shortcuts)

## Connect your signer

Choose **Connect with QR code**, scan it in a compatible NIP-46 signer (such as Amber), and approve. Goop remembers the connection for next time. **Escape** cancels; expired codes can be replaced with **New code**.

Alternatively, paste a `bunker://` connection URL from your signer, or choose **Scan bunker QR** to scan it with your computer’s camera.

Allow Goop to access your camera, then hold your signer’s bunker QR code in the preview. Use the camera selector for an external webcam. A successful scan turns the camera off and fills the URL field; press **Continue** to connect. **Escape** or **Paste URL instead** cancels scanning.

Camera images are decoded on your device, never uploaded or saved. Scanning also stops when you leave the screen, close Goop, or reach the two-minute timeout. If access is denied, enable Goop’s camera permission in system settings and try again. If no camera is available, you can still paste the URL. Sandboxed Linux packages may need camera access enabled in their app permissions.

Only bunker connection QR codes are accepted here; an `nsec` private key or a website QR code is not a signer connection.

### Linux credential storage

If login reports a missing Secret portal or another system credential-store error, choose **Use local credential file**, then connect with a QR code or bunker URL. This explicitly switches Goop to `~/.config/goop/credentials/signer.json` (or your XDG config directory). The file is unencrypted, with permissions limited to your user account: directory `700`, file `600`. It contains the signer connection and Goop’s client key, not your account’s signing key. Keep it private.

Goop preserves accessible credentials when switching. If an existing account’s client key is unavailable, restore access to the system credential store first; Goop will not silently generate a replacement. Logging out removes the saved login from the selected store while retaining the client key and local history.

## Emoji and reactions

Use the composer’s emoji button to browse or search by name, shortcode, or emoji. Categories and skin-tone controls include an **All** option for mixed-tone variants. Choose an emoji to insert it at the cursor.

On a message, choose **Add reaction** to open the same picker; the three quick reactions remain available. Search and press **Enter** to choose, or press **Down** to move into the grid and navigate with arrow keys. **Escape** cancels and returns focus to the composer. The catalogue uses [Unicode 17.0 emoji data](https://docs.rs/emojis/0.9.0/emojis/); appearance follows your operating system’s emoji font.

## Log out

Open the account menu in the title bar and choose **Log out**, immediately after **Settings**. Confirm to disconnect the signer and forget the saved login on this device. **Cancel** keeps you signed in. Local chat history and encryption keys are kept; reconnect your signer to log in again.

## Profile links

Profiles include a **njump.to** link and a homepage link labeled with its address (for example, **dergigi.com**) when the person has set a valid homepage. Hover over the address to see the full URL.

## Agent Guide

Get updates from your agents and reply to them in Goop. Each agent needs its own
Nostr identity and a NIP-17 messaging integration. Give the agent your public key
(`npub`) as its contact, then accept its first message in **Requests**.

### OpenClaw [TESTED]

Copy this prompt and send it to your OpenClaw agent:

```text
Use clawhub.ai/dergigi/nihao to create a nostr identity for yourself &
after that use github.com/fabianfabian/openclaw-nostr-nip17
to set yourself up with NIP-17. DM me when you’re done.
My nostr identity is PASTE_YOUR_NPUB_HERE
```

Replace `PASTE_YOUR_NPUB_HERE` with your full Goop public key (`npub1…`) before
sending the prompt. If you have a NIP-05 address, you can instead write
`My NIP-05 is whatever@domain.com`, using your own address.

### Hermes [UNTESTED]

Install the community [Hermes NIP-17 platform plugin](https://github.com/boto-coder/hermes-nostr-platform)
using its setup instructions. Give it the agent’s own identity, add your `npub` to
`NOSTR_ALLOWED_PUBKEYS`, and keep the Hermes gateway running to receive messages
and reply. Set `NOSTR_HOME_CHANNEL` to your `npub` if you also want scheduled
notifications delivered to Goop.

The plugin publishes the agent’s profile and DM relay list. For now, include your
Goop identity’s DM relays in the plugin’s configured relays: its current
[send path only uses existing relay connections](https://github.com/boto-coder/hermes-nostr-platform/blob/82951dd917bdd153b7a5ef3cf159b0b5dfc626c3/adapter.py#L284),
even when it discovers additional recipient relays.

### NullClaw [UNTESTED]

[NullClaw’s Nostr setup](https://github.com/nullclaw/nullclaw#nostr-channel-setup)
includes a NIP-17 channel. Install its `nak` dependency, run the onboarding wizard,
and supply your `npub` as the owner. The channel publishes the agent’s DM relays and
can respond to Goop text messages while NullClaw is running.

### Other harnesses [UNTESTED]

For harnesses without a verified messaging plugin, [Bray](https://github.com/forgesworn/bray)
provides NIP-17 messaging tools through MCP, and
[Agent Messenger](https://github.com/Sortis-AI/agent-messenger) provides command-line
send/listen tools and an optional agent runner. Both need additional relay and
receive-loop setup. These are building blocks for custom integrations; see the
[detailed review](https://github.com/dergigi/goop/blob/master/docs/research/nip17-agent-harnesses.md)
for the findings by harness.

### Check your setup

1. Confirm that the agent has its own `npub` and published DM relay list.
2. Send a short message from the agent to your `npub`.
3. Accept it in Goop if it appears in **Requests**, then reply.
4. Check that the agent receives the reply and can answer it.

OpenClaw has been tested with Goop. The other integrations have been reviewed
from documentation and source but have not been tested with Goop. See the
[detailed NIP-17 integration review](https://github.com/dergigi/goop/blob/master/docs/research/nip17-agent-harnesses.md)
for implementation findings and sources.

## Keyboard shortcuts

Use **Cmd** on macOS or **Ctrl** on Windows/Linux unless noted otherwise.

- **?** — open Keyboard Shortcuts when you aren’t typing (also available in Help).
- **Cmd/Ctrl+B** — show or hide the conversation sidebar.
- **Cmd/Ctrl+K** — search loaded conversations and message contents, including message requests; names match first.
- **Cmd/Ctrl+P** — instantly search loaded profiles by name, Nostr address, or public key.
- **↑ / ↓**, **Enter**, **Esc** — choose a quick-search result, open it, or return to your previous focus.
- **Cmd/Ctrl+F** — find text in the current chat; **Enter / Shift+Enter** move between matching messages, and **Esc** closes the find bar and restores focus.
- **Cmd/Ctrl+1 / 2 / 3** — open Inbox, open Requests, or focus the current chat’s message box. Hold Cmd/Ctrl to reveal the numbered hints.
- **Cmd/Ctrl+N** (or **Cmd/Ctrl+T**) — open New Chat to start a direct or group conversation.
- **Cmd/Ctrl+Shift+N** — open New Group.
- **Cmd/Ctrl+W**, **Cmd/Ctrl+Shift+W**, **Cmd/Ctrl+Shift+T** — close the current tab, close all tabs, or restore a closed tab.
- **Ctrl+Tab / Ctrl+Shift+Tab** (all platforms) — next / previous tab.
- **Cmd/Ctrl+R** — reload messaging and rescan history.
- **Cmd/Ctrl+Shift+C** — open Contacts.
- **Cmd/Ctrl+Shift+P** — open your Profile.
- **Cmd/Ctrl+Shift+D** — open Connection Status.
- **Cmd/Ctrl+Shift+M** — open Messaging Relays.
- **Cmd/Ctrl+Shift+G** — open Gossip Relays.
- **Cmd/Ctrl+,** — open settings.
- **Enter / Shift+Enter** — send a message / insert a newline.

Quick search filters an in-memory snapshot of loaded items without querying relays as you type. Opening a profile can refresh its details. The sidebar buttons and account menu also expose the quick-search shortcuts.

## App menus

- **Chats:** start chats and groups, open Note to Self, find messages, pin or archive the active chat, mark it read or unread, and leave a group locally. Chat-specific actions are available when a chat is open; Leave requires a group and confirmation.
- **Account:** your profile, contacts, blocked users, connection status, relay configuration, and Log Out.
- **View:** sidebar visibility, Inbox, Requests, message-box focus, reload, and full screen.

The top-right controls are **Relays**, **Connection Status** (pulse icon), and **Message History**. Relays has moved out of the sidebar footer.

## Connection status and recovery

Click the live status beside your avatar or the connection-status icon immediately left of Message History in the top-right corner, press **Cmd/Ctrl+Shift+D**, or choose **Connection status** from the account menu. Hover over the icon for a live summary. Relays appear as compact rows: a green dot means connected, a shield means authenticated, and a yellow warning triangle opens the full error with a Copy error button. Hover over a row for connection and history details. The sidepane refreshes automatically and separates signer connection, messaging relays, history/decryption, and outgoing messages.

- **Signer unavailable:** open your signer, check pending requests, and use **Reconnect signer** if needed.
- **Relay disconnected or authentication failed:** check the named relay and signer. **Reconnect messaging relays** retries connections; **Manage messaging relays** opens your relay configuration.
- **History incomplete:** **Resume history** continues from saved checkpoints. The compact Message History menu offers a full rescan and a broader relay search; both are also available in Connection Status.
- **Decryption failures:** **Retry decryption** retries failed messages. Previously declined requests resume only when you explicitly retry.
- **Outgoing messages:** delayed sends retry automatically; paused sends require **Retry pending sends**. Counts include your own encrypted copy. A relay accepting a message is not a read receipt.

**Copy status** copies the displayed diagnostic text, without message bodies or secret keys. It can include relay URLs and error details. Authentication results are shown when observed during this session; a connected socket alone does not establish successful authentication or delivery. A completed history scan only covers what its relays retain.

## Reporting an account

Choose **Report** at the bottom of the profile pane (or **Report user** in request screening), select a reason, and optionally add an explanation. Nothing is selected by default. Reports and explanations are **public**, signed by your account. **Automatically block reported users** is enabled by default in **Settings → General**. When enabled, Goop blocks the person after a relay accepts your report; turn it off to report without blocking.

If automatic blocking fails, the confirmation says the report was sent and explains how to retry blocking. Changing accounts while the report is being sent skips automatic blocking.

Goop waits for relay acknowledgement before confirming submission and tells you when only some relays accepted the report. Signing or delivery errors stay visible with **Copy error**, and your inputs are preserved. Retrying an unchanged report reuses its signed event; changing the reason or explanation creates a new report.

## Muting and blocking

The profile pane keeps three red actions at the bottom, with profile details and **Chat** / **Search** above:

- **Mute:** pause notifications from this person for 1 hour, 8 hours, 1 day, 1 week, or until you unmute them. This applies to their messages in direct and group chats, on this device and account. Messages and unread counts still appear. Open **Unmute** to resume notifications or change the duration.
- **Block:** after confirmation, hide their direct chats from Inbox, Requests, and Archived. Their messages and reactions in shared groups are hidden, and their messages do not trigger notifications or unread counts. Shared groups remain available. Direct messaging is disabled until you unblock them; existing history is preserved.
- **Report:** publish a signed report with a reason and optional public explanation. By default, Goop also blocks the person after the report is accepted. Disable **Automatically block reported users** in Settings → General to report without blocking.

The **Blocked users** button beside Archive in the sidebar footer shows blocked users in the left sidebar, just like Archive. The Account menu opens the same list. Open a profile or choose **Unblock** to restore visibility. The list includes users blocked in compatible clients, even if you have never chatted with them in Goop.

Blocks use private NIP-44-encrypted `p` entries in the standard NIP-51 kind-10000 list, as Amethyst does. Existing public/private entries and unrelated list tags are preserved. Local changes take effect immediately; the Blocked users view shows pending sync, errors, **Copy error**, and **Retry sync**. Declined or timed-out signer requests wait for an explicit retry. Muting is device-local and expires automatically.

Legacy NIP-04-encrypted lists are preserved but cannot currently be edited by Goop. If encountered, the view explains that the list needs to be updated in a NIP-44-compatible client first; the local block still applies. Blocking cannot prevent another client from sending encrypted messages to your relays.
