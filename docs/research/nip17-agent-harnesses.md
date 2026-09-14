# NIP-17 messaging integrations for agent harnesses

Goop can be the human-facing inbox for an agent when the integration sends ordinary NIP-17 chat messages, delivers them to the recipient’s DM relays, and keeps a receive path running. The most direct documented setup among the reviewed projects is OpenClaw’s community NIP-17 channel. NullClaw also implements a persistent NIP-17 channel. Most coding harnesses instead expose extension mechanisms through which a separate messaging tool can be connected.

This review covers public documentation and implementation snapshots checked on 2026-09-14. It distinguishes implemented capabilities from inferred host compatibility. It is not an end-to-end certification of the listed integrations with Goop, a security audit, or a popularity ranking. The public usage guide selects the shortest practical setup paths from these findings.

## What a usable Goop integration needs

NIP-17 defines private conversations using NIP-44 encryption and NIP-59 gift wraps. For text chat, the unsigned inner message is kind 14 and the outer gift wrap is kind 1059. A recipient’s kind 10050 event advertises the relays on which they receive DMs. Correct encryption alone does not establish delivery: the sender must discover and publish to those inbox relays.[^1]

An agent should have its own identity, distinct from the person using Goop. That identity needs a profile for recognizability and a published inbox list for replies. The agent’s listener must actually subscribe to those advertised relays. A generated key and a profile are therefore only the first part of setup.

There are three useful integration shapes:

| Shape | Behavior | Consequence for Goop users |
| --- | --- | --- |
| Persistent channel | Receives a message, invokes the harness, sends its answer | Suited to ongoing conversations while the gateway runs |
| MCP tools | Lets an active agent call send/read functions | Good for updates and checking replies; host scheduling determines responsiveness |
| CLI plus wrapper | Sends or streams messages through a command-line process | Flexible, but the operator must connect the listener to the harness |

MCP host support establishes that a server can be connected, not that it has been tested with Goop or that an incoming message starts a new model turn. This distinction applies to all the MCP rows below.

## Harness comparison

| Harness | NIP-17 route found | Evidence and readiness |
| --- | --- | --- |
| OpenClaw | `fabianfabian/openclaw-nostr-nip17` | Dedicated persistent community channel; setup documented and relevant source reviewed |
| Claude Code | Bray or Nostr Agent Interface through local MCP; Agent Messenger’s runner also documents Claude | Host transport documented; messaging implementation reviewed; combination not exercised with Goop |
| Codex CLI / IDE | Local MCP messaging server | Official MCP connection and configuration documented; host compatibility inferred from that transport |
| Cursor | Local MCP messaging server | Official custom-server support; no Cursor-specific NIP-17 channel verified |
| OpenCode | Local MCP messaging server | Official local MCP configuration; no ready-made native NIP-17 channel verified in this review |
| Gemini CLI | MCP messaging server | Official command-based server configuration; same tool-driven receive limitation |
| Cline | MCP messaging server | Official MCP support; messaging supplied by the external server |
| Goose | Custom MCP extension | Official extension docs explicitly support arbitrary MCP servers |
| Hermes | MCP messaging server | Official MCP support; dedicated NIP-17 gateway proposal #16769 is closed without merging |
| Windsurf / Cascade | MCP messaging server | Official documentation now distinguishes legacy Cascade configuration from the newer Devin Local agent; use the instructions matching the installed host |
| Pi | NIP-17 CLI invoked through a skill or extension | Official extension model supports custom tools; the core site directs MCP users to extensions rather than promising built-in MCP |
| Aider | NIP-17 CLI/listener with a custom wrapper | Official one-shot command-line mode can be invoked by an external process; no packaged Aider-specific integration verified |
| NullClaw | Built-in NIP-17 channel using `nak` | Onboarding, inbox publication, receive path, and reply path present in source |
| Moltis | Current source implements gift-wrapped text DMs | Source is ahead of the published channel documentation; relay provisioning and release availability require verification before a simple install recommendation |

Harness references: Claude Code,[^7] Codex,[^8] Cursor,[^9] OpenCode,[^10] Gemini CLI,[^11] Cline,[^12] Goose,[^13] Hermes,[^14] Windsurf/Cascade,[^15] Pi,[^16] and Aider.[^17] Messaging implementations are examined separately below.

## OpenClaw: identity setup plus a persistent channel

Nihao is an identity bootstrap and health-check tool. Its OpenClaw skill installs the CLI and guides creation of a profile, general relay list, and DM inbox list. It is appropriate for an agent that is not on Nostr yet. It does not maintain a message listener or invoke the agent on receipt; the channel plugin supplies those functions.[^2]

The upstream community channel supports multiple identities and mappings from each account to an OpenClaw agent. Pairing and allowed-sender configuration belong to the channel setup. Operators should configure the agent identity, authorize their own public key, restart the gateway, and test both directions.[^3]

There is a material documentation discrepancy. The plugin README says it does not publish the agent’s DM relay list. At reviewed commit `bcbd54516be347a426acec3be5e74944e590d534`, `startNip17Bus` defaults `publishRelayList` to true and calls `publishOwnRelayList` during startup. Publication targets include configured relays and discovery relays. The usage guide follows that source evidence and asks users to check their installed version rather than repeat the README’s older claim.[^4]

Recipient delivery uses a separate kind-10050 lookup/cache implementation. It searches discovery relays and extracts `relay` tags, with fallback behavior if discovery fails. That is much closer to a complete messaging channel than a tool that only encrypts an event and publishes it to a fixed list. A successful gateway startup is still not proof that the announcement reached a relay or that the human’s inbox accepted a message.[^5]

Recommendation: lead the public guide with **Nihao when identity setup is needed**, followed by the upstream NIP-17 plugin. Link to upstream installation/configuration instructions instead of copying a large configuration block that will drift.

## MCP: broad host coverage, tool-driven operation

### Bray

Bray exposes NIP-17 send/read/conversation tools and supports a separate signer, encrypted keys, and key files. Its README documents a local MCP launch and tools for identity setup. This makes it a useful candidate across the MCP-capable harnesses, without requiring an individual Nostr plugin for every editor.[^6]

Its DM source builds kind-14 rumors and gift wraps. However, the automatic recipient routing in `handleDmSend` consults NIP-65 read relays and merges them with the sender’s write relays; it does not use the recipient’s kind-10050 list in that path. An explicit `relays` argument overrides this behavior. Operators should supply the recipient’s actual DM inbox relays and configure reads against the agent’s inbox. The public guide calls out this requirement instead of presenting automatic discovery as complete.[^18]

The same implementation makes a sender copy by constructing another rumor addressed to the sender. This changes the conversation’s participant tags rather than reusing the original rumor, so sent-history consistency with Goop needs verification. Receiving a text update can work without this being correct, but a complete history claim would be premature.[^18]

Recommendation: present Bray as a tool-based setup that requires a send-and-reply check. Do not equate a connected MCP server with an always-running conversational agent.

### Nostr Agent Interface and Nostr MCP Server

Austin Kelsay’s projects expose direct-message operations named `sendDmNip44`, `getDmInboxNip44`, and `decryptDmNip44`. Despite the narrower names, their source uses complete NIP-17 gift wraps through `createDirectMessage` and `decryptDirectMessage`. Nostr Agent Interface additionally provides CLI and HTTP modes, with MCP as an explicit mode.[^19]

The send/read tools use provided relay lists or fixed defaults. The inspected DM module does not discover recipient kind-10050 relays automatically. Each call also takes the private key as a tool argument. A local wrapper that owns the agent key and restricts the exposed operation is preferable to repeatedly passing that credential through model-generated arguments.[^20]

Recommendation: document this as an alternative for implementers in the research note, not the shortest default recipe in the public guide. Its interfaces are useful, but the additional relay and identity handling must be implemented deliberately.

### Host-specific boundaries

Claude Code and Codex both document registering command-based MCP servers. Codex supports configuration through its CLI and TOML files; the configured executable, arguments, environment, and optional working directory determine how the local process starts. Hosted products should not be assumed to launch a local stdio executable merely because the corresponding desktop or CLI host can.[^7][^8]

Cursor, OpenCode, Gemini CLI, Cline, Goose, and Hermes document external-tool integrations through MCP. Their settings formats and approval controls differ, so the public guide links each host’s own instructions. This avoids maintaining nine nearly identical examples with different configuration syntax.[^9][^10][^11][^12][^13][^14]

Hermes proposal #16769 contains a dedicated NIP-17 gateway design, including publication of an inbox list and sender authorization. The proposal is closed and has no merge timestamp. It is evidence of work on the integration, not evidence that a normal Hermes installation ships it. The guide therefore gives Hermes the verified MCP extension route.[^21]

## Command-line workflows

Agent Messenger (`am`) implements NIP-17 text sending and a streaming receive command. Its optional `am-ingest` service puts received messages into SQLite; `am-agent` invokes a configurable CLI and sends replies. The documented configuration uses Claude’s command-line interface, but the command and argument template are configurable.[^22]

The messaging source publishes through `config.relays`; it does not automatically select each recipient’s DM inbox from kind 10050 in the inspected send path. Its quick-start commands therefore need relay provisioning before they are a dependable Goop recipe. The agent runner also needs an explicit policy for which conversations it should process; a background process is a separate operational component from the coding agent itself.[^23]

For Pi, using a CLI from a skill fits the documented extensibility model. An extension can also connect a listener to the session, but that is implementation work rather than a verified, ready-made NIP-17 Pi package. For Aider, the documented one-shot `--message` mode offers a possible wrapper interface. These are architecture recommendations, not claims of tested packaged integrations.[^16][^17]

## Other implemented channels

NullClaw’s README provides an onboarding path using `nak`, asks for the owner public key, and configures DM relays. Its channel source publishes kind 10050, queries the recipient’s inbox list, wraps kind-14 messages, unwraps incoming messages, and defaults previously unseen recipients to NIP-17. Its send implementation is explicitly text-only, so attachment support should not be inferred.[^24]

Moltis’s inspected source receives kind-1059 events, unwraps kind-14 messages, and sends gift-wrapped replies. Its current published guide still describes NIP-17 as future work. The helper sends to connected relays, and the inspected connection/helper paths do not establish automatic kind-10050 provisioning. Because the documentation and code disagree, the project belongs in the research inventory with a version caveat rather than an unconditional public setup recommendation.[^25]

## Validation and maintenance

This review checked documentation, repository metadata, and relevant source paths. No external agent was installed, no agent identity was created, and no real NIP-17 messages were sent to validate these combinations. Implementation evidence is stronger than a README badge, but weaker than an interoperability test.

A reproducible follow-up should give each tested integration an isolated identity and record its version, advertised inbox relays, and authorized human key. Test agent-to-Goop delivery, Goop-to-agent delivery, a reply after restart, and delivery when the two identities use different inbox relays. For MCP setups, also record whether replies are received only on an explicit tool call or are delivered into an active session automatically.

The public guide should stay focused on successful setup: identity, integration, host connection, relay configuration, and a two-way test. Update the reviewed date and source pins whenever the recommended integrations change. Keep per-harness configuration details in upstream links unless a configuration has actually been exercised with Goop.

## Sources

All sources accessed 2026-09-14. Source snapshots are pinned where implementation claims depend on code.

[^1]: Nostr protocol contributors. [NIP-17: Private Direct Messages](https://github.com/nostr-protocol/nips/blob/master/17.md).
[^2]: dergigi. [Nihao OpenClaw skill, v0.12.3](https://clawhub.ai/dergigi/skills/nihao); [Nihao source](https://github.com/dergigi/nihao/tree/17766f58a96a1f4d7ac451c0e113dc1cc88526ad).
[^3]: fabianfabian and contributors. [OpenClaw NIP-17 plugin README](https://github.com/fabianfabian/openclaw-nostr-nip17/blob/bcbd54516be347a426acec3be5e74944e590d534/README.md), snapshot 2026-09-03.
[^4]: Same project. [NIP-17 bus and startup relay-list publication](https://github.com/fabianfabian/openclaw-nostr-nip17/blob/bcbd54516be347a426acec3be5e74944e590d534/src/nip17-bus.ts).
[^5]: Same project. [Recipient DM relay discovery/cache](https://github.com/fabianfabian/openclaw-nostr-nip17/blob/bcbd54516be347a426acec3be5e74944e590d534/src/relay-cache.ts).
[^6]: forgesworn and contributors. [Bray README](https://github.com/forgesworn/bray/blob/a46eda90b9b5d3f060a8a0f59b821a7df7f13201/README.md), snapshot 2026-09-14.
[^7]: Anthropic. [Connect Claude Code to tools via MCP](https://code.claude.com/docs/en/mcp).
[^8]: OpenAI. [Model Context Protocol for Codex](https://developers.openai.com/codex/mcp/).
[^9]: Cursor. [Model Context Protocol](https://cursor.com/docs/mcp).
[^10]: OpenCode. [MCP servers](https://opencode.ai/docs/mcp-servers/).
[^11]: Google. [MCP servers with Gemini CLI](https://geminicli.com/docs/tools/mcp-server/).
[^12]: Cline. [MCP overview](https://docs.cline.bot/mcp/mcp-overview).
[^13]: Goose maintainers. [Using Extensions](https://github.com/aaif-goose/goose/blob/main/documentation/docs/getting-started/using-extensions.md).
[^14]: Nous Research. [MCP integration for Hermes](https://hermes-agent.nousresearch.com/docs/user-guide/features/mcp/).
[^15]: Cognition. [Cascade MCP configuration](https://docs.windsurf.com/windsurf/cascade/mcp), redirected to Devin Desktop documentation.
[^16]: Pi maintainers. [Pi](https://pi.dev/) and [Extensions reference](https://pi.dev/docs/latest/extensions).
[^17]: Aider maintainers. [Scripting Aider](https://aider.chat/docs/scripting.html).
[^18]: forgesworn and contributors. [Bray DM routing and sender-copy implementation](https://github.com/forgesworn/bray/blob/a46eda90b9b5d3f060a8a0f59b821a7df7f13201/src/social/dm.ts); [NIP-17 wrapping](https://github.com/forgesworn/bray/blob/a46eda90b9b5d3f060a8a0f59b821a7df7f13201/src/nip17-wrap.ts).
[^19]: Austin Kelsay. [Nostr Agent Interface README](https://github.com/AustinKelsay/nostr-agent-interface/blob/f19c8478ca1437ee6c29dd2793acce9efc77126c/README.md), snapshot 2026-04-04.
[^20]: Austin Kelsay. [Agent Interface DM tools](https://github.com/AustinKelsay/nostr-agent-interface/blob/f19c8478ca1437ee6c29dd2793acce9efc77126c/dm/dm-tools.ts); [Nostr MCP Server DM tools](https://github.com/AustinKelsay/nostr-mcp-server/blob/010b6dcb5aa87d268e0182554e3f9b0055e8fdab/dm/dm-tools.ts).
[^21]: Nous Research/Hermes contributors. [NIP-17 gateway proposal #16769](https://github.com/NousResearch/hermes-agent/pull/16769), closed without merge as checked on 2026-09-14.
[^22]: Sortis-AI. [Agent Messenger README](https://github.com/Sortis-AI/agent-messenger/blob/da49fad838b7e9adf9a964290e990a16520299f8/README.md), snapshot 2026-04-01.
[^23]: Sortis-AI. [Message implementation](https://github.com/Sortis-AI/agent-messenger/blob/da49fad838b7e9adf9a964290e990a16520299f8/crates/am-core/src/message.rs); [Configurable agent runner](https://github.com/Sortis-AI/agent-messenger/blob/da49fad838b7e9adf9a964290e990a16520299f8/crates/am-agent/src/main.rs).
[^24]: NullClaw contributors. [Nostr setup](https://github.com/nullclaw/nullclaw/blob/d8a802fd967962be5d4f819ccd0cf1592a98f39c/README.md#nostr-channel-setup); [Nostr channel implementation](https://github.com/nullclaw/nullclaw/blob/d8a802fd967962be5d4f819ccd0cf1592a98f39c/src/channels/nostr.zig), snapshot 2026-07-13.
[^25]: Moltis contributors. [Published Nostr guide](https://docs.moltis.org/nostr.html); [Receive path](https://github.com/moltis-org/moltis/blob/9d3238c322708e9d57fe235ce1c2b43ccc33af62/crates/nostr/src/bus.rs); [Gift-wrap send/receive helper](https://github.com/moltis-org/moltis/blob/9d3238c322708e9d57fe235ce1c2b43ccc33af62/crates/nostr/src/gift_wrap.rs), snapshot 2026-09-14.
