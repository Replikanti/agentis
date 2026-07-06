# CLAUDE.md

## What this repo is

The public distribution frontstore for the Agentis binary. Users land here to install (`install.sh`), read the README, browse example `.ag` agents, and download prebuilt binaries from Releases. **No source code lives here** — binaries are built and uploaded to this repo's Releases by an automated release pipeline; this repo never builds them.

## Contents

- `README.md` — the product page: install one-liner, quick start, LLM backend config, examples
- `install.sh` — POSIX-sh installer: detects os/arch, resolves the latest release via the GitHub API, downloads the matching binary to `${AGENTIS_INSTALL_DIR:-/usr/local/bin}` (sudo only when the target is not writable). It currently checks only for a non-empty download — it does NOT yet verify the `.sha256` sidecar that every release ships; if you touch this script, adding fail-closed sidecar verification is the most valuable improvement
- `examples/*.ag` — curated subset of the examples that `agentis init` ships

## Release assets (produced externally)

Each release carries 8 assets: `agentis-{linux,macos}-{x86_64,aarch64}` plus a `.sha256` sidecar each (`sha256sum` format; verify with `sha256sum -c` on Linux, `shasum -a 256 -c` on macOS). Sidecars provide post-download integrity verification, not cryptographic non-repudiation.

## Workflow

- Never push directly to main; feature branch + PR, squash-merge.
- This is a PUBLIC repo: no internal hostnames, no absolute paths from private machines, no references to private repositories or clients in any committed content — README, code comments, issues, or PR text.
- Keep README claims externally verifiable (release links, download badges); substance over hype.
- `install.sh` must stay POSIX sh (dash-compatible) and portable across Linux + macOS.

## Checking changes

```bash
sh -n install.sh                # syntax
shellcheck install.sh           # lint, if available
```

For installer changes, also dry-run against the latest real release into a temp `--prefix`/dir before merging.
