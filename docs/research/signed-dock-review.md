# Signed dock review

Reviewed on 2026-09-16 after Reya recommended the dock implementation from Signed.

## Source snapshots

- [Reya's message](https://njump.to/nevent1qqsvq8mu88f733fvrc2rexzkxkh2s5t6xvps76kd7w4583jstn8r0pqzyqfxzqalmhyd7fttdc9tl4ln097gphxya2y00shc0h2pqs3qknt97ntfjy3): recommends copying Signed's latest dock implementation for more control and fixes, without enumerating the bugs.
- [Signed](https://git.reya.info/reya/signed), HEAD `a85b5ee87b7378491eae856d92555ae0d63c7518`.
- Signed pins [GPUI Component](https://github.com/longbridge/gpui-component/tree/39c2c86dbee7ad445591462f8675f74082e10828) at `39c2c86dbee7ad445591462f8675f74082e10828`.
- Goop reviewed at `c4e5ead`, following 2.10.0 publication.

## What the code actually is

There are three layers:

1. **GPUI**, from Zed: native rendering, input, focus, and application state.
2. **GPUI Component / gpui-base**: reusable widgets and a dock engine that manages panels and layouts.
3. **Signed's dock crate**: custom renderers and interaction choices, built on that engine.

Signed's `crates/dock/Cargo.toml` explicitly describes it as a skin over the upstream dock. Its `lib.rs` re-exports the shared `DockArea`, `DockLayout`, `Panel`, and related types; its own files supply tab bars, resize controls, tiles, and window controls. This is ordinary Rust source code, not a special generated layout format. Source inspection does not establish whether individual portions were written by hand or with AI assistance.

Goop has its own local dock implementation in `crates/ui/src/dock` (roughly 3,200 lines including tests). Signed has roughly 1,800 lines of dock presentation code plus the external engine. Those numbers are not a like-for-like size or complexity comparison.

## Useful differences

| Area | Evidence in Signed or its pinned engine | Relevance to Goop |
| --- | --- | --- |
| Panel activation | `gpui-base/dock/active.rs` tracks stable panel identities and reconciles state changes, rather than relying on delayed indexes into a mutable list. Engine tests cover insertion, removal, reselecting the active tab, dragging between groups, and collapse/expand. | A stronger foundation for changes to tabs and split panes. Goop currently has separate index-based activation paths; most application panels do not override activation callbacks, so this alone is not evidence of an observed user-facing bug. |
| Layout and sizing | Engine tests cover split proportions, explicit sizes surviving the first layout pass, and serialization of actual rendered dimensions. | Useful regression coverage for resizing and split views. Goop's short-window composer and sidebar focus fixes must remain covered too. |
| Empty groups | Signed returns an empty element for empty tab groups, removes an emptied side dock, and includes a render smoke test covering removal of the last bottom panel. | Overlaps Goop fixes for empty docks and last-tab behavior. Preserve Goop's automatic Welcome tab. |
| Resize interaction | Signed reads current dock state while dragging a closed dock open, then emits a layout-change event on mouse release. | Avoids using stale open/closed state during a drag and supports persisting the final size. Applicability needs a targeted Goop reproduction. |
| Appearance versus engine | Separate `DockAreaRenderer`, `TabGroupRenderer`, and `TilesRenderer` hooks. | Makes it possible to retain Goop's visual design while obtaining shared engine maintenance. |

The upstream tests are concrete evidence of covered scenarios, not proof that every corresponding Goop path is broken. This review did not run Signed or compile its dependency graph.

## Why copying the files is insufficient

- Signed uses GPUI revision `7960b2a7c9568e90fbe0727332149e5b2a5fd57a`; Goop is locked to `bce0c5785bfd9172c939aca4083fd70bc4930927`. Compatibility must be established explicitly.
- The panel API differs: Goop returns a string identity and a title from `&App`; Signed uses the shared engine's identity, `BasePanel`, panel handles, and a mutable title-rendering API.
- Signed's renderers depend on its UI/theme and window-control components. They are not standalone files that can replace Goop's dock module unchanged.
- Goop has application-specific tab restoration, close-all/next/previous commands, account-scoped cleanup, focus recovery after hiding a sidebar, composer focus, right-pane profile routing, and a Welcome fallback. These must survive a migration.
- Signed's macOS tab bar also acts as a title bar and handles traffic-light spacing. Goop's current separate title bar should not change accidentally.

## Recommendation

Evaluate the pinned shared dock engine in an isolated prototype, keeping Goop's presentation and behavior. Use regression tests for tab moves, active-tab identity, empty groups, composer visibility in short windows, sidebar hide/show shortcuts, reopened tabs, logout cleanup, and profile placement before considering a replacement.

For an immediate bug, first reproduce it and port a narrow fix where possible. A wholesale transplant is not justified by the message alone. No production code or dependency versions were changed in this review.
