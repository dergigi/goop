# Desktop UX direction: Paseo → Goop

Research date: 2026-09-14. Paseo source snapshot: `836f1a9c23f7c59a09c6112f2e73c3f875687e80`. Goop comparison baseline: `269174a` (after v2.0.0); first navigation implementation: `115542d`.

## Recommendation

Give Goop stable navigation, a coherent composer, and predictable keyboard focus. Paseo's most useful lessons are behavioral: hints reflect the action that actually runs, temporary search preserves the user's place, and layout adapts without making primary navigation change meaning. These fit Goop's existing GPUI architecture. A framework rewrite is unnecessary.

The user's Cursor screenshots reinforce one concrete choice: New Chat and Search should be ordinary navigation rows, with an icon, label, and shortcut badge. Inbox and Requests are persistent conversation categories. Opening search should not insert another category or remove their labels.

## Scope and evidence

This audit examines the pinned public Paseo source, including navigation, command-center behavior, keyboard routing, composer presentation, layout constants, theme definitions, loading placeholders, and relevant browser tests. It compares those with Goop's workspace, sidebar, quick-search dialog, chat composer, and shared UI primitives. Links below refer to the inspected source version.

The public Paseo homepage is a visual reference, not proof of runtime behavior: it presents a product mockup. The Cursor observations come from the user's screenshots; they do not establish whether shortcut visibility is driven by hover, selection, or focus. This is a source audit, not a completed hands-on accessibility, performance, or cross-platform usability study.

## Findings and application

### 1. Keep navigation stable

Paseo separates top-level navigation rows from workspace content. Its navigation rows pass action-derived shortcut keys into a shared row component. Labels and shortcut presentation therefore belong to reusable navigation components, rather than to the content being searched.[1]

Goop previously combined three functions in one sidebar: conversation categories, search, and contact selection for new conversations. Focusing its input enabled a separate search panel, inserted a leading magnifying-glass tab, and removed the Inbox/Requests labels. This made a focus change look like a navigation change.

**Implemented:** replace the input and redundant Chats/Profiles buttons with New Chat and Search rows. Search opens the existing local conversation-search modal. Inbox and Requests retain their positions, labels, and selection. Contact discovery and multi-recipient conversation creation move into a New Chat dialog. Cmd/Ctrl+F and Cmd/Ctrl+K open conversation search; Cmd/Ctrl+N and the existing Cmd/Ctrl+T open New Chat. Badges use GPUI's actual bindings through `Kbd`, including platform formatting.

The contact picker retains the previous discovery behavior, which may use relays. Conversation search remains an in-memory operation over loaded data. Those are different tasks and should remain understandable from their dialog titles and empty states.

### 2. Teach shortcuts at the point of use

Paseo's composer hint is conditional. It appears only for the active composer, when the input is empty and unfocused, and when a focus shortcut exists. The text does not intercept pointer events. Its style places it in the corner using a muted foreground, small text, and reduced opacity.[2]

This explains why the user's “⌘L to focus” example is effective: it offers the next relevant action, then disappears when no longer useful. Goop should adopt that behavior, rather than permanently printing shortcuts throughout every toolbar.

**Next:** add a quiet composer focus hint and Cmd/Ctrl+L as an alias for the existing Cmd/Ctrl+3. Derive the displayed binding from the registered action. Keep typed text, attachments, and reply previews clear of the hint. Verify contrast in both themes instead of copying Paseo's opacity literally.

### 3. Treat focus as application state

Paseo's command center handles Escape, Enter, and arrow navigation explicitly. Editing the query clears the previous active-result selection in the same state update. Closing the command center retrieves the previously focused element, retries restoring focus, and falls back to focusing the message input if restoration times out.[3]

Goop already has a modal focus-restoration mechanism and arrow/Enter handling in quick search. Reusing that modal for Cmd/Ctrl+F removes the need for a second sidebar-specific return-focus implementation. The next improvement is to test the whole interaction rather than infer it from individual handlers.

Acceptance scenarios:

- Open search from a partially written message, then Escape: restore the draft and its focus.
- Open search from Inbox or Requests, then Escape: preserve the selected category.
- Select another conversation: focus the destination appropriately instead of returning to the old chat.
- Close the source tab while a modal is open: use a valid fallback focus target.
- Repeat with IME composition and with multiple chat tabs open.

Goop's composer currently enables `clean_on_escape()`. Before making Escape a general focus-dismissal gesture there, remove the risk of silently clearing a draft. This is an identified Goop issue, not a claim that Paseo implements the same Escape policy.

### 4. Share one keyboard vocabulary

Paseo represents keyboard actions separately from focus scopes such as message input, editable controls, and command center. Its shortcut hook resolves the effective platform/runtime binding, including overrides. Its searchable help dialog also builds from effective bindings and shows when an action has been unassigned.[4]

Goop can adopt the principle with its existing action system. Menus, badges, tooltips, and a future Keyboard Shortcuts dialog should read the same bindings. Preserve user-established aliases while changing the preferred visible shortcut. Avoid inventing a second table of hardcoded glyphs.

A Goop help dialog is preferable to making the README the only way to discover profile search, tab restoration, or composer focus. User-customizable bindings can follow later; a consistent read-only help view is useful on its own.

### 5. Use a deliberate layout system

Paseo defines shared desktop header dimensions and an 820-unit maximum conversation/composer content width. Its desktop sidebar calculations reserve a minimum center width and clamp sidebar size according to the viewport. These are useful examples of coordinated geometry, not universal dimensions to copy.[5]

Goop should align the message column and composer, give the composer a clear boundary, and establish shared spacing for navigation rows, tabs, and secondary controls. Prototype a bounded reading column on wide windows, allowing code and media to use appropriate overflow behavior. Keep narrower windows usable and account for larger text and long translated labels.

Paseo also computes composer control density from available width, font scale, and control presence. It uses hysteresis around transitions to avoid flickering between layouts near a threshold.[6] Goop has fewer controls, but attachments, emoji, settings, and Send should have an intentional compact layout rather than crowding the text input.

### 6. Use semantic visual hierarchy

Paseo's actual theme definitions distinguish surfaces, normal and muted foregrounds, borders, and accents.[7] The useful lesson is semantic roles, not a particular gray palette. Goop already has semantic theme tokens and should build on them.

Use strong emphasis for the active conversation and primary action; quieter text for timestamps, shortcut hints, and supporting status. Avoid making every navigation control look like a large primary button. Preserve clear hover and keyboard-focus feedback as decoration is reduced.

### 7. Make loading informative without displacing useful content

Paseo includes a sidebar skeleton with repeated row geometry and a shared pulse animation.[8] This establishes that loading can be represented in place, but does not prove all its loading flows behave identically.

For Goop, cached conversations and identities should remain visible during refresh. Reserve placeholders for genuinely missing content. Separate initial loading, background history retrieval, and actionable connection failures. Keep detailed relay counters available in a status view rather than making them the dominant chat chrome. This is especially relevant to the user's earlier reports of apparent logout, unresolved avatars, and conversations appearing late.

### 8. Test the interaction, not just the component

Paseo has a browser test that submits a message, verifies the composer remains focused, then types the next message and checks the draft.[9] Its control-density tests are another useful pattern: responsiveness is behavior with acceptance criteria.

Goop's current sidebar change passes compilation and its existing local-search matching test. It has not yet received a running-app visual review. Before release, exercise the new navigation using mouse and keyboard on macOS, Windows, and Linux, including Escape restoration, direct/group creation, empty results, offline discovery failure, and narrow windows. Do not equate a successful Rust build with validated UI behavior.

## Ordered implementation plan

| Step | Deliverable | Completion criterion |
| --- | --- | --- |
| 1 — implemented | Stable New Chat/Search rows; separate picker; persistent Inbox/Requests | No sidebar search input or temporary third category; shortcut routes compile and search test passes |
| 2 | Composer container and contextual focus hint | Hint appears only when useful; Cmd/Ctrl+L and +3 agree; drafts survive dismissal |
| 3 | Shared shortcut help and focus behavior | Menu labels and help use real bindings; modal return-focus scenarios pass |
| 4 | Message/composer alignment and responsive spacing | Readable wide layout; usable narrow layout and larger text; no clipped primary actions |
| 5 | Calm loading/status presentation | Cached content persists; loading and offline states are distinguishable; recovery remains accessible |
| 6 | Profile/request presentation and final interaction review | Identity fallback, Accept/Ignore flow, keyboard traversal, and cross-platform smoke checks verified |

Implement and commit each step independently. Preserve NIP-17-only messaging, signer login, local search speed, and the existing relay/history recovery behavior. Paseo's agent workspaces, terminals, model selectors, and review workflow are outside this messenger's scope.

## Sources

1. [Sidebar navigation](https://github.com/getpaseo/paseo/blob/836f1a9c23f7c59a09c6112f2e73c3f875687e80/packages/app/src/components/sidebar/sidebar-nav-rows.tsx)

2. [Composer focus hint and rendering](https://github.com/getpaseo/paseo/blob/836f1a9c23f7c59a09c6112f2e73c3f875687e80/packages/app/src/composer/input/input.tsx)

3. [Command-center keyboard and focus lifecycle](https://github.com/getpaseo/paseo/blob/836f1a9c23f7c59a09c6112f2e73c3f875687e80/packages/app/src/command-center/command-center.tsx)

4. [Keyboard help built from effective bindings](https://github.com/getpaseo/paseo/blob/836f1a9c23f7c59a09c6112f2e73c3f875687e80/packages/app/src/components/keyboard-shortcuts-dialog.tsx)

5. [Shared layout dimensions](https://github.com/getpaseo/paseo/blob/836f1a9c23f7c59a09c6112f2e73c3f875687e80/packages/app/src/constants/layout.ts)

6. [Composer density calculation](https://github.com/getpaseo/paseo/blob/836f1a9c23f7c59a09c6112f2e73c3f875687e80/packages/app/src/composer/agent-controls/layout.ts)

7. [Semantic theme definitions](https://github.com/getpaseo/paseo/blob/836f1a9c23f7c59a09c6112f2e73c3f875687e80/packages/app/src/styles/theme.ts)

8. [Sidebar loading skeleton](https://github.com/getpaseo/paseo/blob/836f1a9c23f7c59a09c6112f2e73c3f875687e80/packages/app/src/components/sidebar-agent-list-skeleton.tsx)

9. [Composer focus interaction test](https://github.com/getpaseo/paseo/blob/836f1a9c23f7c59a09c6112f2e73c3f875687e80/packages/app/e2e/browser/composer-focus.spec.ts)

Additional inspected sources: [shortcut resolver hook](https://github.com/getpaseo/paseo/blob/836f1a9c23f7c59a09c6112f2e73c3f875687e80/packages/app/src/hooks/use-shortcut-keys.ts), [action and focus-scope types](https://github.com/getpaseo/paseo/blob/836f1a9c23f7c59a09c6112f2e73c3f875687e80/packages/app/src/keyboard/actions.ts), [sidebar width calculation](https://github.com/getpaseo/paseo/blob/836f1a9c23f7c59a09c6112f2e73c3f875687e80/packages/app/src/components/desktop-sidebar-layout.ts), [workspace result ranking](https://github.com/getpaseo/paseo/blob/836f1a9c23f7c59a09c6112f2e73c3f875687e80/packages/app/src/command-center/workspace-search.ts).
