# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Agentis is an AI-native programming language fused with a Version Control System (VCS). Code is represented as a binary, hashed DAG stored in `.agentis/objects/` (content-addressable storage). Designed as an operating system for AI agents — humans write prompts, agents work in Agentis.

## Tech Stack

- **Language:** Rust (`sha2` + `wasm-encoder` + `ureq` crates; `wasmparser` dev-only)
- **No frameworks:** No Tokio, no serde — minimal sync Rust
- **License:** MIT

## Architecture

```
src/
  main.rs           # CLI (init, commit, run, branch, switch, compile, sync, serve, worker, log, go, mutate, arena, evolve, lineage)
  arena.rs          # Arena runner (rank variants by fitness, table/JSON output)
  evolve.rs         # Evolution loop (generational mutation→arena→select, lineage tracking)
  fitness.rs        # Fitness scoring (FitnessReport, FitnessWeights, JSONL registry)
  mutation.rs       # Mutation engine (extract agents, mock/LLM mutations, source reconstruction)
  lexer.rs          # Tokenizer
  ast.rs            # AST types + manual binary serialization
  parser.rs         # Recursive descent parser (Pratt precedence) + error recovery
  typechecker.rs    # Static type checker (inference, structural typing)
  storage.rs        # SHA-256 content-addressed object store
  evaluator.rs      # Tree-walking interpreter + Cognitive Budget + OCap + collections
  json.rs           # Minimal JSON builder/parser (no serde)
  config.rs         # Config reader (.agentis/config, key = value format)
  llm.rs            # Pluggable LLM backend (MockBackend + CliBackend + HttpBackend)
  io.rs             # Capability-gated I/O (sandboxed file ops + whitelisted HTTP)
  compiler.rs       # WASM compiler backend (AST→WASM binary, CB metering)
  capabilities.rs   # Capability-Based Security (OCap) — unforgeable handles + PiiTransmit
  snapshot.rs       # Orthogonal Persistence — memory snapshots at transaction boundaries
  colony.rs         # Distributed Colony (ThreadPool, worker node, protocol encode/decode)
  network.rs        # Raw TCP P2P sync (binary HAVE/WANT/DATA/DONE + colony EVAL/RESULT/PING/PONG/AUTH)
  refs.rs           # Branch/reference management (genesis-first)
  pii.rs            # Internal PII scanner (guard, not builtin — never exposed to .ag code)
  audit.rs          # JSONL audit log for prompt() calls (.agentis/audit/prompts.jsonl)
  trace.rs          # Runtime tracing (quiet/normal/verbose)
  error.rs          # Unified AgentisError type
```

Pipeline: source → lexer → parser → AST → typechecker (warnings) → evaluator OR compiler
Storage: AST → binary serialization → SHA-256 hash → `.agentis/objects/`

## Key Concepts

- **Genesis branch:** Default branch (never `main` or `master`)
- **Cognitive Budget (CB):** Execution fuel — arithmetic=1, lookup=1, call=5, prompt=50. Exceeding raises `CognitiveOverload`
- **AI-native constructs:** `agent` (isolated pure execution), `prompt` (typed LLM call — pluggable backend, mock default), `validate` (runtime predicates), `explore` (semantic branching)
- **Static types:** TypeScript-style with inference, structural typing, mandatory annotations on signatures

## Build & Run

```bash
cargo build                    # Build
cargo test                     # Run all tests (582)
cargo test <test_name>         # Run a single test
cargo clippy                   # Lint

cargo run -- init              # Initialize .agentis/ with genesis branch
cargo run -- commit file.ag    # Parse, store AST, update current branch
cargo run -- run genesis       # Execute code from branch
cargo run -- branch            # List branches
cargo run -- branch <name>     # Create new branch
cargo run -- switch <name>     # Switch to a different branch
cargo run -- compile <branch>   # Compile branch to WASM binary
cargo run -- sync <host:port>  # Sync objects with remote peer
cargo run -- serve [addr:port] # Listen for incoming sync connections
cargo run -- worker [addr:port]               # Start colony worker node
cargo run -- worker 0.0.0.0:9462 --secret S   # Worker with auth
cargo run -- worker --max-concurrent 4        # Limit concurrent evals
cargo run -- log               # Show commit log
cargo run -- snapshot list     # List all persisted snapshots
cargo run -- snapshot show <h> # Show snapshot details (variables, budget, output)
cargo run -- repl              # Interactive evaluator (REPL)
cargo run -- repl --resume <h> # Resume REPL from snapshot (30% CB penalty)
cargo run -- test <files|dir>  # Run tests (validate/explore outcomes)
cargo run -- go file.ag --fitness              # Run + print fitness report
cargo run -- go file.ag --fitness --weights W  # Custom weights (cb,val,exp)
cargo run -- mutate file.ag --list-agents     # List agents and their instructions
cargo run -- mutate file.ag --count 5         # Generate 5 mutated variants
cargo run -- mutate file.ag --dry-run         # Preview mutations without writing
cargo run -- mutate file.ag --agent <name>    # Mutate only specific agent
cargo run -- mutate file.ag --out <dir>       # Write variants to directory
cargo run -- mutate file.ag --mutate-prompt T # Custom mutation template ({instruction})
cargo run -- arena dir/                      # Rank all .ag files by fitness
cargo run -- arena f1.ag f2.ag --rounds 5    # Run each 5 times, average scores
cargo run -- arena dir/ --top 3 --json       # Top 3 as JSON
cargo run -- arena dir/ --workers h1:9462,h2:9462  # Colony arena
cargo run -- arena dir/ --workers workers.txt --secret S
cargo run -- colony status --workers h1:9462,h2:9462  # Worker health
cargo run -- colony status --workers workers.txt --json
cargo run -- colony ping 10.0.0.1:9462                # Single ping
cargo run -- evolve file.ag -g 10 -n 8      # Evolution: 10 gens, pop 8
cargo run -- evolve file.ag --dry-run -g 10 -n 8  # Estimate cost
cargo run -- evolve file.ag -g 5 -n 4 --show-lineage --out evolved/
cargo run -- evolve file.ag -g 20 -n 8 --stop-on-stall 5
cargo run -- lineage evolved/variant.ag     # Trace ancestry to seed
```

## Phase 2 Features

- **WASM Compiler:** Compiles integer subset of AST to WASM binary with CB metering injected. OCap host imports declared in import section.
- **OCap Security:** SHA-256 unforgeable capability handles. 8 capability kinds (Prompt, FileRead, FileWrite, NetConnect, NetListen, VcsRead, VcsWrite, Stdout). Registry secret from `/dev/urandom`.
- **Orthogonal Persistence:** Snapshots at transaction boundaries (empty call stack). Content-addressed dedup via ObjectStore.
- **TCP P2P Sync:** Binary length-prefixed protocol. `sync_push_pull` (client), `sync_serve_once` (server).
- **Collections:** `[1, 2, 3]` list literals, `map_of(k, v, ...)` builtin. `push`, `get`, `len` builtins.
- **Static Type Checker:** Pre-evaluation type checking with inference. Reports as warnings.

## Phase 3 Features (complete)

- **Pluggable LLM Backend:** `prompt` calls real LLMs. Config in `.agentis/config`. Three backends: MockBackend (default), CliBackend (any CLI tool — flat-rate), HttpBackend (per-token API). Defensive JSON parsing with retry.
- **Capability-Gated I/O:** `file_read`/`file_write` sandboxed to `.agentis/sandbox/` with path canonicalization. `http_get`/`http_post` restricted to domain whitelist (`io.allowed_domains`). All ops go through OCap `require_cap()`. CB costs: file=10, http=25.
- **Module System:** `import "sha256hash";`, `import "hash" as alias;`, `import "hash" { name };`. Content-addressed imports from object store. Cyclic import detection. Transitive resolution.
- **Multi-Agent Orchestration:** `spawn agent(args)` runs agent in `std::thread`, returns `AgentHandle`. `await(handle)` blocks for result. `await_timeout(handle, ms)` with timeout. Fork bomb prevention via `max_concurrent_agents` (default 16). Spawn costs 10 CB.
- **JSON Utility:** Hand-rolled JSON builder/parser (`json.rs`). Safe string escaping, no serde.
- **Config System:** Simple `key = value` format in `.agentis/config`.

## Phase 5 Features (Data Guardians — complete)

- **PiiTransmit Capability:** New `CapKind::PiiTransmit` excluded from `grant_all()`. Must be explicitly granted via `--grant-pii` CLI flag or `pii_transmit = allow` in config.
- **Internal PII Guard:** `pii.rs` scans prompt inputs for email, phone, credit card, Czech birth number, IBAN, IPv4, SSN. Blocks prompt if PII detected without PiiTransmit. Zero CB cost.
- **Audit Log:** Every `prompt()` call logged to `.agentis/audit/prompts.jsonl` (JSONL). Fields: timestamp, agent name, instruction/input hashes, PII scan result, capability status, backend. Opt-in: enabled when `.agentis/audit/` directory exists.
- **Audit CLI:** `agentis audit` displays audit log table. Filters: `--last N`, `--pii-only`, `--agent <name>`, `--blocked`.
- **Secure Init:** `agentis init --secure` creates locked-down config (PiiTransmit denied, audit enabled, mock backend).

## Phase 6 Features (Resurrection & Developer Experience — complete)

- **Snapshot CLI:** `agentis snapshot list/show` — persistent snapshot registry (`.agentis/snapshots`), prefix hash matching, variable/output/budget inspection.
- **REPL:** `agentis repl` — interactive evaluator with dot-commands (`.exit`, `.budget`, `.snapshot`, `.output`, `.help`). Multi-line via brace balancing. `--resume <hash>` restores from snapshot with 30% CB penalty.
- **Test Runner:** `agentis test <files|dir>` — reports validate/explore outcomes. `--fail-fast`, `--verbose`. Exit code 0/1.
- **Rich Errors:** `ErrorDetail` struct with agent name, expression description, actionable hints. Enhanced: prompt PII errors, undefined functions (with "did you mean?"), arity mismatches, CB exhaustion, validate failures.

## Phase 7 Features (Agent Evolution — complete)

- **Fitness Metrics (M27):** `agentis go file.ag --fitness` reports composite fitness score. `FitnessReport` with CB efficiency, validate rate, explore rate, prompt count. `FitnessWeights` configurable via `--weights 0.3,0.5,0.2` or config. Dynamic weight redistribution when validates/explores absent. JSONL registry at `.agentis/fitness.jsonl`.
- **Mutation Engine (M29):** `agentis mutate file.ag` generates agent variants by mutating prompt instruction strings. Source-level string replacement (not AST rewrite). LLM-guided mutations with real backend; 8 deterministic perturbations with mock. Flags: `--count`, `--out`, `--agent`, `--mutate-prompt`, `--dry-run`, `--list-agents`.
- **Arena Runner (M28):** `agentis arena dir/` runs variants side by side, ranks by fitness. Supports `--rounds N` (multi-round averaging), `--top N`, `--json`, `--weights`. Sequential execution, quiet tracing. Error variants scored 0.0 with truncated error messages.
- **Evolution Loop (M30):** `agentis evolve file.ag -g G -n N` runs the full evolutionary loop: mutate → arena → select → repeat. Tournament selection (top K=N/2 survives). Per-generation JSONL lineage in `.agentis/fitness/`. Intermediate bests saved to `<out>/g{gen:02}-best.ag`. `--budget-cap`, `--stop-on-stall`, `--show-lineage`, `--dry-run`. `agentis lineage <file>` traces ancestry to seed.

## Phase 8 Features (Distributed Colony — complete)

- **Parallel Arena (M31):** `agentis arena dir/ --parallel [--threads N]` runs variants in parallel using a thread pool. Auto-detects CPU count. `agentis evolve ... --threads N` for parallel evolution. `colony.rs` provides `ThreadPool` with `mpsc` + worker threads.
- **Worker Node (M32):** `agentis worker [addr:port]` starts a colony worker that evaluates `.ag` programs remotely. Uses local LLM config (heterogeneous backends). `--secret` for shared-secret auth (SHA-256). `--max-concurrent N` for pipeline parallelism. Payload limits: source ≤1MB, output/error ≤4KB.
- **Colony Arena (M33):** `agentis arena dir/ --workers host1:port,host2:port [--secret S]` distributes evaluation across workers. Round-robin distribution. Graceful fallback to local on worker failure with reason (connection refused/auth failed/timeout/protocol error). `--workers workers.txt` reads from file. `colony.workers` and `colony.secret` in config.
- **Colony Observability (M34):** `agentis colony status [--workers W] [--json]` shows worker health table. `agentis colony ping addr:port` for single-worker diagnostics. Reports backend name, eval stats, busy status, roundtrip time.
- **Protocol Extension:** Same binary framing as sync. EVAL(0x05)/RESULT(0x06) for evaluation, PING(0x07)/PONG(0x08) for health, AUTH(0x09)/AUTH_OK(0x0A)/AUTH_FAIL(0x0B) for auth handshake.
