# Agentis

An AI-native programming language fused with a Version Control System.
Code is a binary, hashed DAG — not text files. The LLM is the standard library.

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/Replikanti/agentis/main/install.sh | sh
```

Or download a binary directly from [Releases](https://github.com/Replikanti/agentis/releases).

| Platform | Binary |
|----------|--------|
| Linux x86_64 | `agentis-linux-x86_64` |
| Linux aarch64 | `agentis-linux-aarch64` |
| macOS x86_64 | `agentis-macos-x86_64` |
| macOS Apple Silicon | `agentis-macos-aarch64` |

## Quick Start

```bash
agentis init                          # creates .agentis/ with config
# Edit .agentis/config — uncomment your LLM backend (claude, ollama, API)
agentis go examples/fast-demo.ag      # first run — output in 3-8 seconds
```

## Everything Is Prompt

In Ruby, everything is an object. In Agentis, **everything is a prompt**.

There is no stdlib. No `string.split()`. No `list.filter()`. If an agent
needs to split a string, it asks the LLM. The LLM is the standard library.

```
// Extract emails — no regex, no stdlib, just a prompt
let emails = prompt("Extract all email addresses", text) -> list<string>;

// Classify with typed output + validation
agent classifier(text: string) -> Category {
    cb 200;
    let result = prompt("Classify this text", text) -> Category;
    validate result {
        result.confidence > 0.5
    };
    return result;
}

// Evolutionary branching — survive or die
explore "approach-a" {
    let sol = solver(problem);
    validate sol { sol.score > 70 };
}
```

## LLM Backends

Configure in `.agentis/config` (created by `agentis init`):

| Backend | Config | Cost |
|---------|--------|------|
| **Claude CLI** (recommended) | `llm.backend = cli` | Flat-rate subscription |
| **Ollama** (local) | `llm.backend = cli`, `llm.command = ollama` | Free |
| **Anthropic API** | `llm.backend = http` | Per-token |
| **Gemini CLI** | `llm.backend = cli`, `llm.command = gemini` | Flat-rate |
| **Mock** (default) | `llm.backend = mock` | No LLM needed |

## CLI

```bash
agentis init                  # Create .agentis/ with genesis branch
agentis go <file.ag>          # Commit + run in one step
agentis go <file.ag> --trace  # Same, with verbose trace output
agentis doctor                # Pre-flight environment check
agentis commit <file.ag>      # Parse and store AST
agentis run <branch>          # Execute code from a branch
agentis branch                # List branches
agentis branch <name>         # Create new branch
agentis switch <name>         # Switch branch
agentis log                   # Show commit history
```

## Why Agentis?

- **AI-native.** Designed for agents. `prompt` is a language primitive, not a library call.
- **Cognitive Budget.** Every operation costs fuel. Prevents runaway agents. Forces efficient prompt design.
- **Evolutionary branching.** `explore` blocks fork execution — success creates a branch, failure is silently discarded.
- **Content-addressed code.** SHA-256 hashed AST. No merge conflicts. Import by hash.
- **Sandboxed I/O.** File operations are jailed to `.agentis/sandbox/`. Network calls require domain whitelisting.
- **Zero bloat.** Vanilla Rust. `sha2` + `ureq` only.

## Docs

- [Language Reference](docs/language.md) — syntax, types, built-ins, CB costs
- [VCS Model](docs/vcs.md) — content-addressed storage, commits, branches
- [Philosophy](docs/philosophy.md) — why everything is prompt
- [Examples](examples/README.md) — 6 programs from hello world to evolutionary branching

## License

MIT
