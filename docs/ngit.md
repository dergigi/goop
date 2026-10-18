# Nostr mirror

Goop's Git history, branches, and tags are mirrored on `relay.ngit.dev` and `gitnostr.com`, using [ngit](https://ngit.dev). Browse the repository on [GitWorkshop](https://gitworkshop.dev/npub1dergggklka99wwrs92yz8wdjs952h2ux2ha2ed598ngwu9w7a6fsh9xzpc/relay.ngit.dev/goop).

```sh
git clone nostr://npub1dergggklka99wwrs92yz8wdjs952h2ux2ha2ed598ngwu9w7a6fsh9xzpc/relay.ngit.dev/goop
```

Install ngit before cloning. GitHub remains the release and existing issue/PR host. Git mirroring does not copy GitHub issue discussions, pull-request discussions, or release binaries.

## Maintainer checkout

- `origin`: GitHub, preserving the existing release scripts.
- `nostr`: the Nostr repository, backed by both GRASP hosts.
- `upstream`: Coop.
- Local signer alias: `ngit-goop`, a dedicated Amber connection stored in the OS credential store.

After publishing commits to GitHub, update the mirror:

```sh
git fetch origin
git push nostr refs/remotes/origin/master:refs/heads/master
```

Repeat the explicit branch push for other published branches. For a published release tag, use its full ref, for example:

```sh
git push nostr refs/tags/v2.15.1:refs/tags/v2.15.1
```

The signer must be online. These pushes publish signed repository state and update both GRASP hosts. A direct GitHub push does not automatically update Nostr. After each release, mirror the release tag and the final master commit, including packaging and validation updates.

Use `ngit repo` to inspect the announcement and `ngit sync` to retry a GRASP host that missed a signed update. Avoid force pushes and `git push --mirror`, which can delete refs or publish local-only branches. Fetch and reconcile any changes made through Nostr before pushing over them.

The initial mirror was verified against all five GitHub branches and all published tags on 2026-10-18. Both hosts returned matching object IDs.
