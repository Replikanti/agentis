# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Agentis is a programming language fused with a Version Control System (VCS). Code is represented as a binary, hashed DAG stored in `.agentis/objects/` (content-addressable storage), not as plain text files.

## Tech Stack

- **Language:** Rust (zero-dependency, only `sha2` crate for integrity)
- **No frameworks:** No SQLite, no Tokio — pure vanilla Rust
- **License:** MIT

## Architecture

The system is a hybrid compiler + Git-internals pipeline:

1. **Lexer/Parser** — source code → AST
2. **Hashing** — every AST node gets a SHA-256 hash
3. **Storage** — nodes saved as binary objects (content-addressable)
4. **Interpreter** — executes AST directly, enforcing Cognitive Budget (CB)
5. **P2P Sync** — code sync over raw TCP sockets

## Key Concepts

- **Genesis branch:** The default/root branch (replaces `main`)
- **Cognitive Budget (CB):** Execution fuel system that prevents runaway computation. Math ops cost 1 CB, function calls cost 5 CB, memory allocation scales by size. Exceeding budget raises `CognitiveOverload`.
- **Explore blocks:** Semantic branching mechanism (replaces traditional merge conflicts)

## Build & Run

```bash
cargo build            # Build the project
cargo test             # Run all tests
cargo test <test_name> # Run a single test
cargo run -- init      # Initialize a new repo with genesis branch
cargo run -- run genesis  # Execute code from genesis branch
```
