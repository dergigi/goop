---
layout: default
title: Usage guide
---

# Goop usage guide

A simple NIP-17 client that just works.

Designed for people who love the keyboard.
This guide covers Goop 2.1.

If NIP-17 isn’t good enough for you, give [WhiteNoise](https://www.whitenoise.chat/) a try.

[Agent Guide](#agent-guide) · [Keyboard shortcuts](#keyboard-shortcuts)

## Agent Guide

Get updates from your agents and reply to them in Goop. Each agent needs its own
Nostr identity and a NIP-17 messaging integration. Give the agent your public key
(`npub`) as its contact, then accept its first message in **Requests**.

### OpenClaw

**Not on Nostr yet? Start with the [Nihao skill](https://clawhub.ai/dergigi/skills/nihao).**
Ask your OpenClaw agent to use it to set up its identity, profile, and DM relays.
Nihao prepares the identity; the channel plugin below handles conversations.

```sh
openclaw skills install @dergigi/nihao
```

Then install the [OpenClaw NIP-17 channel plugin](https://github.com/fabianfabian/openclaw-nostr-nip17)
and follow its configuration instructions. Use the agent’s identity for the plugin
and approve your own `npub` through pairing or the allowed-senders list. Keep the
OpenClaw gateway running so the agent can receive your messages and reply.

The plugin supports separate identities for multiple agents. Its current
[implementation publishes the agent’s DM relay list on startup](https://github.com/fabianfabian/openclaw-nostr-nip17/blob/bcbd54516be347a426acec3be5e74944e590d534/src/nip17-bus.ts#L252),
so check the installed version if relay discovery does not work.

### Hermes

Install the community [Hermes NIP-17 platform plugin](https://github.com/boto-coder/hermes-nostr-platform)
using its setup instructions. Give it the agent’s own identity, add your `npub` to
`NOSTR_ALLOWED_PUBKEYS`, and keep the Hermes gateway running to receive messages
and reply. Set `NOSTR_HOME_CHANNEL` to your `npub` if you also want scheduled
notifications delivered to Goop.

The plugin publishes the agent’s profile and DM relay list. For now, include your
Goop identity’s DM relays in the plugin’s configured relays: its current
[send path only uses existing relay connections](https://github.com/boto-coder/hermes-nostr-platform/blob/82951dd917bdd153b7a5ef3cf159b0b5dfc626c3/adapter.py#L284),
even when it discovers additional recipient relays.

### NullClaw

[NullClaw’s Nostr setup](https://github.com/nullclaw/nullclaw#nostr-channel-setup)
includes a NIP-17 channel. Install its `nak` dependency, run the onboarding wizard,
and supply your `npub` as the owner. The channel publishes the agent’s DM relays and
can respond to Goop text messages while NullClaw is running.

### Other harnesses

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

These third-party paths were reviewed on 2026-09-14 from their documentation and
source. They have not all been tested end to end with Goop. See the
[detailed NIP-17 integration review](https://github.com/dergigi/goop/blob/master/docs/research/nip17-agent-harnesses.md)
for implementation findings and sources.

## Keyboard shortcuts

Use **Cmd** on macOS or **Ctrl** on Windows/Linux unless noted otherwise.

- **?** — open Keyboard Shortcuts when you aren’t typing (also available in Help).
- **Cmd/Ctrl+B** — show or hide the conversation sidebar.
- **Cmd/Ctrl+K** — instantly search loaded conversations, including message requests.
- **Cmd/Ctrl+P** — instantly search loaded profiles by name, Nostr address, or public key.
- **↑ / ↓**, **Enter**, **Esc** — choose a quick-search result, open it, or return to your previous focus.
- **Cmd/Ctrl+F** — find text in the current chat; **Enter / Shift+Enter** move between matching messages, and **Esc** closes the find bar and restores focus.
- **Cmd/Ctrl+1 / 2 / 3** — open Inbox, open Requests, or focus the current chat’s message box.
- **Cmd/Ctrl+N** (or **Cmd/Ctrl+T**) — open New Chat to start a direct or group conversation.
- **Cmd/Ctrl+W**, **Cmd/Ctrl+Shift+W**, **Cmd/Ctrl+Shift+T** — close the current tab, close all tabs, or restore a closed tab.
- **Ctrl+Tab / Ctrl+Shift+Tab** (all platforms) — next / previous tab.
- **Cmd/Ctrl+R** — reload messaging and rescan history.
- **Cmd/Ctrl+,** — open settings.
- **Enter / Shift+Enter** — send a message / insert a newline.

Quick search filters an in-memory snapshot of loaded items without querying relays as you type. Opening a profile can refresh its details. The sidebar buttons and account menu also expose the quick-search shortcuts.
