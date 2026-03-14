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

// Budget-aware decisions — estimate cost, gauge confidence
agent classifier(text: string) -> string {
    cb 500;
    let cost = estimate_cb("analyze deeply", text);
    let conf = confidence("extract facts", text, 3);
    if cost > introspect.cb_remaining * 4 / 10 {
        return prompt("classify briefly", text) -> string;
    };
    if conf.agreement < 65 / 100 {
        return prompt("classify with examples", text) -> string;
    };
    return prompt("analyze deeply", text) -> string;
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
# Basics
agentis init                          # Create .agentis/ with genesis branch
agentis init --secure                 # Locked-down config (PII denied, audit on)
agentis go file.ag                    # Commit + run in one step
agentis go file.ag --fitness          # Run + print fitness report
agentis commit file.ag                # Parse and store AST
agentis run <branch>                  # Execute code from a branch
agentis branch [name]                 # List or create branches
agentis switch <name>                 # Switch branch
agentis log                           # Show commit history
agentis doctor                        # Pre-flight environment check

# Development
agentis test <files|dir>              # Run validate/explore tests
agentis repl                          # Interactive evaluator
agentis repl --resume <hash>          # Resume from snapshot (30% CB penalty)
agentis compile <branch>              # Compile branch to WASM binary
agentis snapshot list|show <hash>     # Inspect persisted snapshots

# Evolution
agentis mutate file.ag --count 5      # Generate mutated variants
agentis arena dir/ --rounds 3         # Rank variants by fitness
agentis evolve file.ag -g 20 -n 8    # Evolve: 20 generations, pop 8
agentis evolve file.ag --resume <hash> -g 10 -n 8  # Resume from checkpoint
agentis evolve file.ag --resume-from agent.agb      # Resume from bundle
agentis evolve file.ag --backup-to /backups         # Auto-backup on new best
agentis evolve file.ag --adaptive-budget             # Dynamic per-lineage budgets
agentis evolve file.ag --seed-from-lib "email"       # Warm-start from library
agentis lineage evolved/variant.ag    # Trace ancestry to seed

# Library
agentis lib add file.ag --tag "v1"    # Add variant to library
agentis lib list [--tag T]            # List entries (optionally filtered)
agentis lib search "email"            # Fuzzy search by description/tag
agentis lib show <hash-or-tag>        # Show entry details
agentis lib export --out b.alib --all # Export library bundle
agentis lib import bundle.alib        # Import library bundle

# Identity & Portability
agentis identity hash                 # Identity from HEAD checkpoint
agentis identity hash file.ag         # Seed-only identity
agentis identity show                 # Identity card (hash, gen, drift hint)
agentis identity verify agent.agb     # Verify bundle integrity (PASS/FAIL)
agentis identity diff a.agb b.agb     # Compare two bundles
agentis export --out agent.agb        # Export portable .agb bundle
agentis export --out a.agb --include-memos --tag stable
agentis import agent.agb              # Import bundle (checkpoint + memos + lineage)
agentis import agent.agb --memo-conflict replace    # Overwrite local memos

# Colony (distributed)
agentis worker [addr:port]            # Start colony worker node
agentis arena dir/ --workers h1:9462,h2:9462        # Distributed arena
agentis colony status --workers W     # Worker health table
agentis colony history [--limit N]    # Checkpoint chain
agentis colony best [--min-score 0.9] # Find best checkpoint
agentis colony gc [--older-than 7d]   # Garbage-collect checkpoints
agentis sync <host:port>              # Sync objects with remote peer
agentis serve [addr:port]             # Listen for incoming sync

# Budget & Confidence
agentis stats                         # Prompt cost statistics
agentis stats --json                  # Stats as JSON
agentis stats --per-identity          # Stats grouped by instruction hash

# Memory & Audit
agentis memo list                     # Show memo keys and entry counts
agentis memo stats                    # Memo store size and key count
agentis memo clear [key]              # Manual cleanup
agentis audit [--pii-only] [--last N] # View prompt audit log

# Maintenance
agentis version                       # Show current version
agentis update                        # Self-update to latest release
```

## Why Agentis?

- **AI-native.** Designed for agents. `prompt` is a language primitive, not a library call.
- **Cognitive Budget.** Every operation costs fuel. Prevents runaway agents. Forces efficient prompt design.
- **Budget Prediction & Confidence.** `estimate_cb` predicts cost before committing. `confidence` samples the LLM N times and measures agreement. Agents decide strategy based on what they can afford and how much to trust.
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
