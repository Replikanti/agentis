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
  main.rs           # CLI (init, commit, run, branch, switch, compile, sync, serve, log)
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
  capabilities.rs   # Capability-Based Security (OCap) — unforgeable handles
  snapshot.rs       # Orthogonal Persistence — memory snapshots at transaction boundaries
  network.rs        # Raw TCP P2P sync (binary HAVE/WANT/DATA/DONE protocol)
  refs.rs           # Branch/reference management (genesis-first)
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
cargo test                     # Run all tests (380)
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
cargo run -- log               # Show commit log
```

## Phase 2 Features

- **WASM Compiler:** Compiles integer subset of AST to WASM binary with CB metering injected. OCap host imports declared in import section.
- **OCap Security:** SHA-256 unforgeable capability handles. 8 capability kinds (Prompt, FileRead, FileWrite, NetConnect, NetListen, VcsRead, VcsWrite, Stdout). Registry secret from `/dev/urandom`.
- **Orthogonal Persistence:** Snapshots at transaction boundaries (empty call stack). Content-addressed dedup via ObjectStore.
- **TCP P2P Sync:** Binary length-prefixed protocol. `sync_push_pull` (client), `sync_serve_once` (server).
- **Collections:** `[1, 2, 3]` list literals, `map_of(k, v, ...)` builtin. `push`, `get`, `len` builtins.
- **Static Type Checker:** Pre-evaluation type checking with inference. Reports as warnings.

## Phase 3 Features (in progress)

- **Pluggable LLM Backend:** `prompt` calls real LLMs. Config in `.agentis/config`. Three backends: MockBackend (default), CliBackend (any CLI tool — flat-rate), HttpBackend (per-token API). Defensive JSON parsing with retry.
- **Capability-Gated I/O:** `file_read`/`file_write` sandboxed to `.agentis/sandbox/` with path canonicalization. `http_get`/`http_post` restricted to domain whitelist (`io.allowed_domains`). All ops go through OCap `require_cap()`. CB costs: file=10, http=25.
- **JSON Utility:** Hand-rolled JSON builder/parser (`json.rs`). Safe string escaping, no serde.
- **Config System:** Simple `key = value` format in `.agentis/config`.
