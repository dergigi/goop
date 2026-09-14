---
layout: default
title: Usage guide
---

# Goop usage guide

Goop is a NIP-17 messaging client for private, encrypted conversations on Nostr, designed for people who love the keyboard.
This guide covers Goop 2.1.

If NIP-17 isn’t good enough for you, give [WhiteNoise](https://www.whitenoise.chat/) a try.

[Keyboard shortcuts](#keyboard-shortcuts) · [Agent Guide](#agent-guide)

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

## Agent Guide

Your agents can send you updates and receive your replies as private NIP-17 messages.
Goop is where you read and reply; the agent runs in its own harness, with a Nostr
messaging integration and its own identity.

1. Set up a NIP-17 integration for your agent’s harness.
2. Configure it to message your Nostr public key (`npub`).
3. Open the agent’s message in Goop. If it appears in Requests, choose **Accept** to move the conversation to your inbox.

### OpenClaw

Use the [OpenClaw NIP-17 plugin](https://github.com/fabianfabian/openclaw-nostr-nip17).
Follow its setup instructions to connect an agent identity and configure pairing
or allowed senders. It also supports separate identities for multiple agents.

Publish the agent identity’s DM relay list so Goop can discover where to send
replies. The plugin’s setup guide covers this requirement; it does not publish
that list automatically.

### Other harnesses

Use a NIP-17 messaging integration for your harness. Setup depends on the harness;
Goop receives the resulting messages as ordinary conversations.
