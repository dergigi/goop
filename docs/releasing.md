# Versioning and releases

Goop follows [Semantic Versioning 2.0.0](https://semver.org/) and maintains
[CHANGELOG.md](../CHANGELOG.md) using [Keep a Changelog 1.0.0](https://keepachangelog.com/en/1.0.0/).

## Compatibility contract

For this desktop application, the public compatibility contract covers documented
login methods, supported messaging protocols and interoperability, documented
command-line/deep-link interfaces, and access to existing accounts, settings, and
locally stored messages after an upgrade. Internal Rust crates are implementation
details, not a stable library API. Visual layout and wording are not API guarantees.

- **MAJOR**: incompatible changes to that contract, including removing a supported
  login method or requiring a manual, incompatible data migration.
- **MINOR**: backward-compatible features or deprecations; reset PATCH to zero.
- **PATCH**: backward-compatible bug fixes without new features.

Automatic migrations that preserve access to existing data are compatible. Record
breaking changes and migration instructions explicitly. The unreleased removal of
plain-secret-key login requires the next stable release to increment MAJOR from
1.1.0 under this policy. Do not select a smaller bump just because most changes are fixes.

The workspace version in `Cargo.toml` is authoritative; application crates inherit
it. Tags use `vMAJOR.MINOR.PATCH` (the `v` is a Git convention, not part of SemVer).
Prereleases may use identifiers such as `2.0.0-rc.1` and are marked as prereleases
on GitHub. A commit revision distinguishes local development builds; they are not
new published releases. Never move a published version tag or replace its artifacts.

## Maintaining the changelog

Add human-readable entries to `Unreleased` as changes land. Use the applicable
Added, Changed, Deprecated, Removed, Fixed, and Security categories, omit empty
categories, and describe user impact rather than copying the Git log.

Keep releases newest first, use ISO `YYYY-MM-DD` release dates, and maintain the
version links at the bottom. Preserve published entries; clarify them only to
correct documentation. Historical standalone files in `release-notes/` are archives;
new GitHub release bodies come from the matching changelog section.

## Cutting a release

1. Complete the [DM release validation checklist](research/dm-release-checklist.md).
2. Choose the appropriate version. Move completed `Unreleased` entries into a
   dated section such as `## [2.0.0] - YYYY-MM-DD`, leaving an `Unreleased` heading.
   Update comparison links and document any required migration steps. Use the
   actual publication date; adjust it before publishing if the draft is delayed.
3. Run `./script/release <version>` from the repository root. It validates the
   version and dated changelog entry before changing files, updates the workspace
   version and lockfile, then commits and pushes the release tag. Review its commit
   selection carefully. This starts the draft-release workflow.
4. The workflow checks that tag, manifest version, and dated changelog entry agree
   before building all platforms. It extracts the release body from the changelog
   and marks prereleases automatically.
5. Review the draft, installers, checksums, release date, and upgrade behavior before
   publishing. Once published, corrections to the software require a new version.

A manual workflow dispatch with `create_release: false` remains available for
build-only validation without requiring a finalized changelog entry.
