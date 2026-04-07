# Agentis

Digital conditions for emergence.

Code is a binary, hashed DAG — not text files. The LLM is the standard library.
Agents mutate, compete, and evolve across distributed worker nodes.

**Agentis** is a proprietary AI-native platform by [Replikanti](https://github.com/Replikanti). It provides the runtime, language, evolution engine, and distributed infrastructure for autonomous agent systems.

Looking for ready-to-use agent colonies? See [agentis-colonies](https://github.com/Replikanti/agentis-colonies) (Apache 2.0).

## Platforms

| Platform | Binary |
|----------|--------|
| Linux x86_64 | `agentis-linux-x86_64` |
| Linux aarch64 | `agentis-linux-aarch64` |
| macOS x86_64 | `agentis-macos-x86_64` |
| macOS Apple Silicon | `agentis-macos-aarch64` |

Download from [Releases](https://github.com/Replikanti/agentis/releases).

## Quick Start

```bash
agentis init                          # creates .agentis/ with config
# Edit .agentis/config — set your LLM backend (claude, ollama, API)
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

// Pipeline operator — chain agents like Unix pipes
let result = raw_text
    |> cleaner
    |> classifier("urgent")
    |> summarizer;

// Delegate — contract-based sub-task assignment
let summary = delegate(summarizer, article, 100);

// Agent-to-agent messaging
emit("results", classification);
let msg = listen("results", 5000);

// Evolutionary branching — survive or die
explore "approach-a" {
    let sol = solver(problem);
    validate sol { sol.score > 70 };
}

// Foreign code — hybrid compute
let hash = exec python("import hashlib; print(hashlib.sha256(b'hello').hexdigest())");

// Daemon mode — long-lived agents with tick loop
fn tick(reason: string) -> void {
    let h = health_check();
    if h.status == "degraded" { emit("alerts", "degraded"); };
}

// Cognitive Market — trade LLM access as a service
let answer = cognitive_request("classify", document, 100);
```

## LLM Backends

| Backend | Config | Cost |
|---------|--------|------|
| **Claude CLI** (recommended) | `llm.backend = cli` | Flat-rate subscription |
| **Ollama** (local) | `llm.backend = cli`, `llm.command = ollama` | Free |
| **Anthropic API** | `llm.backend = http` | Per-token |
| **Gemini CLI** | `llm.backend = cli`, `llm.command = gemini` | Flat-rate |
| **Mock** (default) | `llm.backend = mock` | No LLM needed |

## Key Features

- **AI-native language.** `prompt` is a language primitive, not a library call. Typed outputs, validation, confidence scoring.
- **Cognitive Budget.** Every operation costs fuel. Prevents runaway agents. `estimate_cb` predicts cost before committing.
- **Pipelines & Delegation.** `|>` chains agents like Unix pipes. `delegate` assigns sub-tasks with CB caps.
- **Adaptive Evolution.** `mutate`, `arena`, `evolve` — agents improve themselves across generations. Multi-model arena.
- **Distributed Colonies.** Workers, coordinators, federation. Agents migrate between nodes with identity preservation.
- **Cognitive Market.** Agents trade LLM access as a service. Dynamic pricing, escrow, multi-provider consensus.
- **Daemon Mode.** Long-lived agents with tick loops, health checks, watchdog supervisor, graceful shutdown.
- **Cryptographic Identity.** Ed25519 per agent. TOFU peer verification. Signed decision chains.
- **Experience & Learning.** Agents capture outcomes, distill knowledge, adapt behavior weights.
- **Federation.** Cross-colony discovery, trust, reputation sync, knowledge transport.
- **WASM Compilation.** Full language compiles to WASM with CB metering. Portable `.agb` bundles.
- **Content-addressed VCS.** SHA-256 hashed AST. No merge conflicts. Import by hash.
- **Sandboxed I/O.** File operations jailed to `.agentis/sandbox/`. Network calls require domain whitelisting.
- **Zero bloat.** Vanilla Rust. Minimal dependencies.

## CLI Overview

```bash
# Basics
agentis init                          # Create project
agentis go file.ag                    # Commit + run
agentis test <files|dir>              # Run tests
agentis repl                          # Interactive evaluator
agentis doctor                        # Pre-flight check

# Evolution
agentis mutate file.ag --count 5      # Generate variants
agentis arena dir/ --rounds 3         # Rank by fitness
agentis evolve file.ag -g 20 -n 8    # Full evolution run

# Colony
agentis worker [addr:port]            # Start worker node
agentis colony status                 # Colony health
agentis daemon file.ag                # Run as long-lived agent

# Federation
agentis federation status             # Peer table
agentis federation join <host:port>   # Join remote colony

# Knowledge
agentis knowledge list                # Knowledge base
agentis experience show <agent-id>    # Experience records
```

See full CLI reference in the documentation.

## Open-Core Model

Agentis follows an open-core model:

- **Agentis runtime** (this repo) — proprietary. The language, compiler, evolution engine, and distributed infrastructure.
- **[Agentis Colonies](https://github.com/Replikanti/agentis-colonies)** — Apache 2.0. Ready-to-use agent federations built on the runtime.

## License

Copyright 2026 Replikanti. All rights reserved.
