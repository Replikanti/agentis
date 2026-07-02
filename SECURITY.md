# Security

Agentis runs autonomous agents that can call LLMs, message each other, execute
code, and touch the filesystem. This document describes the containment model
an operator relies on, and how to report a vulnerability.

## Containment model

- **Capabilities are opt-in per daemon.** An agent gets exactly the capability
  flags its daemon was launched with: `--enable-exec` (shell execution),
  `--enable-messaging` (emit/listen), `--enable-migration`,
  `--enable-replication`. Without the flag, the corresponding builtin returns a
  capability-denied error. The watchdog can degrade a misbehaving agent to
  deny-exec, which overrides an earlier `--enable-exec`.
- **Sandboxed I/O.** Agent file operations are jailed to `.agentis/sandbox/`.
- **Cognitive Budget.** Every operation costs fuel from a bounded budget
  (`cb N;`); a runaway agent hits `CognitiveOverload` and stops rather than
  looping forever.
- **Cryptographic identity.** Agents hold Ed25519 keypairs; inter-agent
  messages are signed, peers verify on first use, and decision chains are
  signed.
- **Graduated autonomy.** Production federations built on Agentis gate external
  writes on a measured four-tier confidence ladder — observe-only by default,
  terminal actions (merge, publish) only at the top tier with additional
  opt-in gates. The normative contract is
  [ADR-0001](https://github.com/Replikanti/agentis-colonies/blob/main/doc/adr/ADR-0001-confidence-tiers.md)
  in the Apache-2.0 colonies repo, alongside its own
  [SECURITY.md](https://github.com/Replikanti/agentis-colonies/blob/main/SECURITY.md).

## Update integrity

`agentis update` downloads releases over HTTPS from this repository;
`agentis update --verify-sig` additionally verifies the release's Ed25519
signature. The `install.sh` one-liner installs the latest release binary — read
it before piping to `sh` if that is your policy, or download a binary and its
checksum from the Releases page manually.

## Operator responsibilities

- Treat LLM backends and forge tokens as secrets: they live in `.agentis/config`
  and colony-local config files, never in `.ag` source.
- An agent's `prompt()` inputs may contain untrusted text (issue bodies, web
  content). Capability flags and the confidence ladder bound what a steered
  agent can do — leave `--enable-exec` off for agents that do not need it.

## Reporting a vulnerability

Use GitHub's private vulnerability reporting ("Report a vulnerability" under
this repository's **Security** tab) rather than a public issue. Reports
touching the proprietary runtime internals are routed to the maintainers
through the same channel.
