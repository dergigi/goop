# Flathub Submission for Goop

This directory contains the files needed to submit Goop to Flathub.

## Prerequisites

- Flatpak installed
- `flatpak-builder` installed
- Rust/Cargo installed (for vendoring)

## Quick Start

Run the preparation script from the repo root:

```bash
./script/prepare-flathub.sh
```

This will:
1. Vendor all Rust dependencies (crates.io + git)
2. Generate the metainfo.xml with proper release info
3. Create `com.dergigi.goop.yml` - the Flatpak manifest for Flathub

## Files Generated

| File | Purpose |
|------|---------|
| `com.dergigi.goop.yml` | Main Flatpak manifest (submit this to Flathub) |
| `com.dergigi.goop.metainfo.xml` | AppStream metadata with release info |
| `vendor.tar.gz` | Vendored Rust dependencies |
| `cargo-config.toml` | Cargo configuration for offline builds |
| `release-info.xml` | Release info snippet for metainfo |

## Testing Locally

Before submitting to Flathub, test the build:

```bash
cd flathub

# Build and install locally
flatpak-builder --user --install --force-clean build com.dergigi.goop.yml

# Test the app
flatpak run com.dergigi.goop

# Run the Flathub linter (must pass!)
flatpak run --command=flatpak-builder-lint org.flatpak.Builder manifest com.dergigi.goop.yml
flatpak run --command=flatpak-builder-lint org.flatpak.Builder repo repo
```

## Submitting to Flathub

### 1. Prepare Your Release

Ensure you have:
- [ ] Committed all changes
- [ ] Tagged the release: `git tag -a v1.0.0 -m "Release v1.0.0"`
- [ ] Pushed the tag: `git push origin v1.0.0`
- [ ] Run `./script/prepare-flathub.sh` to regenerate files

### 2. Fork and Submit

```bash
# Fork https://github.com/flathub/flathub on GitHub first

# Clone your fork (use the new-pr branch!)
git clone --branch=new-pr git@github.com:YOUR_USERNAME/flathub.git
cd flathub

# Create a new branch
git checkout -b com.dergigi.goop

# Copy ONLY the manifest file from your project
cp /path/to/goop/flathub/com.dergigi.goop.yml .

# Commit and push
git add com.dergigi.goop.yml
git commit -m "Add com.dergigi.goop"
git push origin com.dergigi.goop
```

### 3. Open Pull Request

1. Go to your fork on GitHub
2. Click "Compare & pull request"
3. **Important:** Set base branch to `new-pr` (not `master`!)
4. Fill in the PR template
5. Submit and wait for review

## What Happens Next?

1. Flathub's automated CI will build your app
2. A maintainer will review your submission
3. Once approved, a new repo `flathub/com.dergigi.goop` will be created
4. You'll get write access to maintain the app
5. Future updates: Push new commits to `flathub/com.dergigi.goop`

## Updating the App

To release a new version:

1. Update version in workspace `Cargo.toml`
2. Tag the new release: `git tag -a v1.0.0 -m "Release v1.0.0"`
3. Push the tag: `git push origin v1.0.0`
4. Run `./script/prepare-flathub.sh` to regenerate
5. Clone the flathub repo: `git clone https://github.com/flathub/com.dergigi.goop.git`
6. Update the manifest with new commit/tag and hashes
7. Submit PR to `flathub/com.dergigi.goop`

## Troubleshooting

### Build fails with "network access not allowed"
- Make sure `CARGO_NET_OFFLINE=true` is set in the manifest
- Ensure `vendor.tar.gz` is properly extracted before building

### Linter complains about metainfo
- Ensure `com.dergigi.goop.metainfo.xml` has at least one `<release>` entry
- Check that screenshots are accessible URLs

### Missing dependencies
- If new git dependencies are added, re-run `prepare-flathub.sh`
- The script vendors all dependencies from `Cargo.lock`

## Resources

- [Flathub Submission Docs](https://docs.flathub.org/docs/for-app-authors/submission)
- [Flatpak Manifest Reference](https://docs.flatpak.org/en/latest/manifests.html)
- [AppStream Metainfo Guide](https://www.freedesktop.org/software/appstream/docs/chap-Metadata.html)
