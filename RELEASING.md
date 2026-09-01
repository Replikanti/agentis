# Releasing the `agentis` installer crate

The `crate/` directory is a thin **open-core installer** for Agentis: a signpost
stub plus `[package.metadata.binstall]` overrides that point `cargo binstall
agentis` at the prebuilt binaries published on this repo's Releases. No private
core source is ever published.

## Automated publishing

`.github/workflows/publish-crate.yml` publishes the crate to crates.io whenever
a new GitHub Release is published here — which happens automatically when the
`agentis-core` release pipeline (`release.yml`, on a `v*` tag) cross-publishes
the platform binaries to this repo. The workflow:

1. resolves the version from the release tag (`v1.29.1` → `1.29.1`);
2. refuses to run unless the matching `agentis-linux-x86_64` release asset exists
   (so the crate is never published ahead of its binaries);
3. bumps `crate/Cargo.toml` to that version;
4. skips if that version is already on crates.io (safe to re-run);
5. `cargo publish --allow-dirty`es, then syncs the bump back to `main` **via a
   pull request** (main is protected, so the workflow cannot push to it
   directly). The sync is best-effort and non-fatal: the crate is already
   published by then, so if the PR cannot auto-merge it is simply left for a
   human — the release is never blocked. The committed `crate/Cargo.toml`
   version is cosmetic anyway; each run re-derives the version from the release
   tag.

### One-time setup (required)

The publish step is **inert until** the repository secret `CARGO_REGISTRY_TOKEN`
is set. Add it under **Settings → Secrets and variables → Actions → New
repository secret**:

- Name: `CARGO_REGISTRY_TOKEN`
- Value: a crates.io API token (crates.io → Account Settings → API Tokens) with
  the `publish-new` and `publish-update` scopes.

Until then the workflow runs but logs a notice and skips the publish, so it does
no harm.

## Manual publishing

Run the workflow by hand from the Actions tab (**Publish crate** →
*Run workflow*) with a `version` input (e.g. `1.29.1`), or locally:

```bash
cd crate
# set crate/Cargo.toml version to match the release you want to track
cargo login            # crates.io token
cargo publish --dry-run
cargo publish
```

Always keep `crate/Cargo.toml`'s version equal to the agentis-core release whose
binaries `cargo binstall agentis` should fetch (the binstall `pkg-url` templates
on `v{ version }`).
