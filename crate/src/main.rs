//! Installer signpost for Agentis.
//!
//! This is a stub binary, not the Agentis runtime. Agentis is open-core: the
//! runtime is a prebuilt, proprietary binary and its source is not published,
//! so a source build (`cargo install agentis`) can only produce this stub. Its
//! two jobs are (1) to give `cargo-binstall` a bin target named `agentis` to
//! resolve — see `[package.metadata.binstall]` in Cargo.toml — and (2) to point
//! anyone who runs a source build at the real install paths instead of leaving
//! them with a non-runtime binary on their PATH.

fn main() {
    eprintln!("agentis: this is the installer stub, not the Agentis runtime.");
    eprintln!();
    eprintln!("Agentis is open-core: the runtime is a prebuilt proprietary binary,");
    eprintln!("so `cargo install agentis` (a source build) cannot install it.");
    eprintln!();
    eprintln!("Install the real runtime with one of:");
    eprintln!();
    eprintln!("  cargo binstall agentis      # fetch the prebuilt binary from GitHub Releases");
    eprintln!("  curl -fsSL https://raw.githubusercontent.com/Replikanti/agentis/main/install.sh | sh");
    eprintln!();
    eprintln!("Or download a binary for your platform from:");
    eprintln!("  https://github.com/Replikanti/agentis/releases");
    std::process::exit(1);
}
