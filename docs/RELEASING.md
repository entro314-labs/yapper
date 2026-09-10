# Releasing Windbag

Single branch, plain semver, one channel per tag. The pipeline itself lives in
the shared `entro314-labs/tauri-release-kit` repo (a reusable `workflow_call`
workflow); [`.github/workflows/release.yml`](../.github/workflows/release.yml)
here is a thin caller that fills in Windbag's inputs. The local half — version
bump, changelog roll, tag, push — is driven by `@entro314labs/release-kit`
(`pnpm release`), configured in [`release.config.json`](../release.config.json).
Fixes to the pipeline land in the kit once and every app picks them up.

## Where releases live

Releases and the updater manifests are published to the **public**
`entro314-labs/windbag-releases` mirror, not to this repo — the same split
every other app here uses. This repo has to stay public regardless: GitHub
Pages serves `docs/client-metadata.json` at the URL the AT Protocol treats as
Windbag's identity, so a private repo would break Sign in with Bluesky, not
the updater. The mirror keeps six platforms' worth of binaries and the rolling
channel tags out of the repo people actually read.

| Channel | Tag shape      | Manifest                                            |
| ------- | -------------- | --------------------------------------------------- |
| stable  | `vX.Y.Z`       | `releases/latest/download/latest.json`              |
| beta    | `vX.Y.Z-beta.N`| `releases/download/latest-beta/latest.json`         |
| alpha   | `vX.Y.Z-alpha.N` | `releases/download/latest-alpha/latest.json`      |

Those three shapes are the contract between the two repos, and
`src-tauri/src/update.rs` has tests that pin them.

## The ritual

```bash
# 1. CHANGELOG.md needs an [Unreleased] section describing the release —
#    the pipeline refuses a tag with no matching section, and that section
#    becomes both the GitHub release body and the updater's release notes.

# 2. One command: preflight checks, the version write (package.json +
#    tauri.conf.json + Cargo.toml/lock via release.config.json), changelog
#    roll, release commit, annotated tag, push.
pnpm release patch
pnpm release minor
pnpm release patch --dry-run   # print every step, execute nothing
```

The tag push triggers `release.yml`: a preflight gate runs `pnpm check` and
`pnpm knip` on the tagged commit, then the kit builds all six platform legs,
signs the updater artifacts, publishes the release and `latest.json` on the
mirror, and updates the Homebrew cask.

## One-time setup

Nothing below is in the repo, and the pipeline is inert until all of it exists:

1. **The releases mirror.** A public `entro314-labs/windbag-releases` repo.
2. **Secrets** on this repo:
   - `TAURI_SIGNING_PRIVATE_KEY` — the contents of `~/.tauri/windbag.key`. Its
     public half is already in `tauri.conf.json`; **lose the private key and no
     existing install can ever be updated again.**
   - `RELEASES_TOKEN` — write access to the mirror.
   - `HOMEBREW_TAP_TOKEN` — write access to `entro314-labs/homebrew-tap`.

macOS runners bill 10× on a private repo. The kit accepts `macos_arm_runner` /
`macos_intel_runner` inputs pointing at a self-hosted Apple Silicon label (the
other apps here use `macbook`); wire those in once a runner exists, and never
on a public repo whose workflows run for fork PRs.

## A partially failed release

Re-dispatch the workflow with the **same** tag and `build_targets` set to just
the failed legs. The run reuses the existing draft release, so the successful
legs' assets are kept and only the failed legs cost runner minutes again.

## Local builds

`bundle.createUpdaterArtifacts` is on, so `pnpm tauri:build` now needs
`TAURI_SIGNING_PRIVATE_KEY` in the environment to sign what it produces.
`pnpm tauri:dev` is unaffected. The `tauri:build:{macos,windows,linux}` scripts
use the same per-OS overlay configs the release legs do, which is the only way
a local build produces the same artifact set as CI.
