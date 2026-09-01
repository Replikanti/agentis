# agentis

Installer for [Agentis](https://github.com/Replikanti/agentis) — a runtime,
language, and evolution engine for autonomous agents. Agentis is **open-core**:
the runtime is a prebuilt, proprietary binary, so this crate does not contain
its source. It exists to make the prebuilt binary installable through the Cargo
toolchain.

## Install

Fetch the prebuilt binary (fastest, no compilation):

```bash
cargo binstall agentis
```

Or use the install script:

```bash
curl -fsSL https://raw.githubusercontent.com/Replikanti/agentis/main/install.sh | sh
```

Supported targets match the published release assets: Linux and macOS on
`x86_64` and `aarch64`. Windows and other targets are not covered (no release
asset), same as the install script.

## Note on `cargo install agentis`

Because Agentis is open-core, a **source** build cannot install the runtime.
Running `cargo install agentis` compiles a small stub binary that, when run,
prints these install instructions and exits — it is not the runtime. Use
`cargo binstall agentis` (above) to fetch the real prebuilt binary.

## License

Proprietary — see [LICENSE](LICENSE). Copyright 2026 Replikanti. All rights reserved.
