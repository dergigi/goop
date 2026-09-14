# Goop

Goop is Gigi's fork of [Coop](https://git.reya.info/reya/coop), a simple, fast, and reliable nostr client for secure messaging across all platforms.

[Download the latest release](https://github.com/dergigi/goop/releases/latest).

Goop is designed for people who love the keyboard and would rather reach for a shortcut than a mouse.

### Keyboard shortcuts

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

### Versioning and changelog

Goop follows [Semantic Versioning 2.0.0](https://semver.org/) and [Keep a Changelog 1.0.0](https://keepachangelog.com/en/1.0.0/). See [CHANGELOG.md](CHANGELOG.md) for released and upcoming changes, and the [release guide](docs/releasing.md) for compatibility rules and the release process.

Upstream: https://git.reya.info/reya/coop.git

### License

Copyright (C) 2025 Ren Amamiya & other Coop contributors

Copyright (C) 2026 Gigi & other Goop contributors

This program is free software: you can redistribute it and/or modify it under the terms of the GNU General Public License as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.

This program is distributed in the hope that it will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General Public License for more details.

You should have received a copy of the GNU General Public License along with this program. If not, see https://www.gnu.org/licenses/.
