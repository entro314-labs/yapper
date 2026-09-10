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

## What is already wired

Nothing here needs doing again — it is written down because losing any of it
breaks the pipeline in a way the error messages will not explain.

- **`entro314-labs/windbag-releases`** — the public mirror releases and the
  `latest.json` manifests are published to.
- **`TAURI_SIGNING_PRIVATE_KEY`** — the minisign key at `~/.tauri/windbag.key`,
  whose public half is in `src-tauri/tauri.conf.json`. **That local file is the
  only copy: lose it and no installed copy of Windbag can ever be updated
  again, on any channel, forever.** Back it up somewhere that is not this
  machine. There is deliberately no `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`
  secret — the key is passwordless, and the kit sets that variable to the empty
  string, which is what a passwordless key expects.
- **`RELEASES_TOKEN` / `HOMEBREW_TAP_TOKEN`** — currently both hold the same
  broad classic PAT, which is org-wide (`repo`, `admin:org`, `delete_repo`) and
  lives in a *public* repo's Actions secrets. It works, and it is more access
  than either job needs. When convenient, replace them with two fine-grained
  PATs carrying only `Contents: Read and write`, one on `windbag-releases` and
  one on `homebrew-tap`:

  ```sh
  gh secret set RELEASES_TOKEN     --repo entro314-labs/yapper   # scoped to windbag-releases
  gh secret set HOMEBREW_TAP_TOKEN --repo entro314-labs/yapper   # scoped to homebrew-tap
  ```

- **GitHub Pages, from `/docs` on `main`.** Not part of releasing, but it shares
  this repo's fate: it serves `client-metadata.json` at the URL the AT Protocol
  treats as Windbag's identity. **This repo cannot be made private** — Sign in
  with Bluesky would stop working for every user the moment it were.

macOS runners bill 10× on a private repo; this one is public, so they bill at
the standard rate and the `macos_arm_runner` / `macos_intel_runner`
self-hosted overrides the other apps here use are not needed. Never point them
at a self-hosted runner while this repo is public and its workflows run for
fork PRs.

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
