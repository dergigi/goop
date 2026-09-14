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

### Claude Code, Codex, Cursor, and other MCP harnesses

An MCP server gives your running agent tools to send and read NIP-17 messages.
[Bray](https://github.com/forgesworn/bray) provides `dm-send`, `dm-read`, and
`dm-conversation`, with support for a separate signer or an agent key file.
Follow its identity setup, then add the server using your harness’s instructions:

| Harness | Where to connect the NIP-17 tools |
| --- | --- |
| Claude Code | [Add a local MCP server](https://code.claude.com/docs/en/mcp). |
| Codex | [Configure an MCP server](https://developers.openai.com/codex/mcp/) in the CLI or IDE extension. |
| Cursor | [Add an MCP server](https://cursor.com/docs/mcp). |
| OpenCode | [Configure a local MCP server](https://opencode.ai/docs/mcp-servers/). |
| Gemini CLI | [Add an MCP server](https://geminicli.com/docs/tools/mcp-server/). |
| Cline | [Connect an MCP server](https://docs.cline.bot/mcp/mcp-overview). |
| Goose | [Add a custom MCP extension](https://github.com/aaif-goose/goose/blob/main/documentation/docs/getting-started/using-extensions.md). |
| Hermes | [Add an MCP server](https://hermes-agent.nousresearch.com/docs/user-guide/features/mcp/). |
| Windsurf / Cascade | [Configure MCP tools](https://docs.windsurf.com/windsurf/cascade/mcp) for your installed agent. |

Ask the agent to send a short test message to your `npub`, then check for your reply.
MCP tools alone do not keep an agent listening while its session is closed; an
always-available conversation needs a running channel or a message-checking loop.

**Relay setup matters:** configure the agent to receive on its published DM relays
and send to the DM relays advertised by your Goop identity. Bray’s current
[send implementation](https://github.com/forgesworn/bray/blob/a46eda90b9b5d3f060a8a0f59b821a7df7f13201/src/social/dm.ts#L100)
needs explicit recipient DM relays for reliable routing when the automatically
selected relays differ. Treat the first send-and-reply test as part of setup.

### Pi and command-line workflows

[Agent Messenger](https://github.com/Sortis-AI/agent-messenger) provides NIP-17
`am send` and `am listen` commands. It also includes an optional listener and agent
runner for processing incoming messages. Configure the relay lists explicitly for
your two identities before using it with Goop.

[Pi supports skills and extensions](https://pi.dev/docs/latest/extensions), so a
skill can teach it to use those commands. For [Aider](https://aider.chat/docs/scripting.html),
a wrapper can connect its command-line mode to the message listener. These paths
need custom setup; they are not built-in Goop integrations.

### NullClaw

[NullClaw’s Nostr setup](https://github.com/nullclaw/nullclaw#nostr-channel-setup)
includes a NIP-17 channel. Install its `nak` dependency, run the onboarding wizard,
and supply your `npub` as the owner. The channel publishes the agent’s DM relays and
can respond to Goop text messages while NullClaw is running.

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
