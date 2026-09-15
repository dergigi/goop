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
- **Cmd/Ctrl+Shift+M** — open Messaging Relays.
- **Cmd/Ctrl+Shift+G** — open Gossip Relays.
- **Cmd/Ctrl+,** — open settings.
- **Enter / Shift+Enter** — send a message / insert a newline.

Quick search filters an in-memory snapshot of loaded items without querying relays as you type. Opening a profile can refresh its details. The sidebar buttons and account menu also expose the quick-search shortcuts.
