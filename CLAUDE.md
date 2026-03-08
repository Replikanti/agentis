# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Agentis is an AI-native programming language fused with a Version Control System (VCS). Code is represented as a binary, hashed DAG stored in `.agentis/objects/` (content-addressable storage). Designed as an operating system for AI agents — humans write prompts, agents work in Agentis.

## Tech Stack

- **Language:** Rust (only `sha2` crate as external dependency)
- **No frameworks:** No SQLite, no Tokio, no serde — pure vanilla Rust
- **License:** MIT

## Architecture

```
src/
  main.rs         # CLI (init, commit, run, branch, switch, log)
  lexer.rs        # Tokenizer
  ast.rs          # AST types + manual binary serialization
  parser.rs       # Recursive descent parser (Pratt precedence)
  storage.rs      # SHA-256 content-addressed object store
  evaluator.rs    # Tree-walking interpreter + Cognitive Budget
  refs.rs         # Branch/reference management (genesis-first)
  error.rs        # Unified AgentisError type
```

Pipeline: source → lexer → parser → AST → binary serialization → SHA-256 hash → `.agentis/objects/`

## Key Concepts

- **Genesis branch:** Default branch (never `main` or `master`)
- **Cognitive Budget (CB):** Execution fuel — arithmetic=1, lookup=1, call=5, prompt=50. Exceeding raises `CognitiveOverload`
- **AI-native constructs:** `agent` (isolated pure execution), `prompt` (typed LLM call, mock in Phase 1), `validate` (runtime predicates), `explore` (semantic branching)
- **Static types:** TypeScript-style with inference, structural typing, mandatory annotations on signatures

## Build & Run

```bash
cargo build                    # Build
cargo test                     # Run all tests (176)
cargo test <test_name>         # Run a single test
cargo clippy                   # Lint

cargo run -- init              # Initialize .agentis/ with genesis branch
cargo run -- commit file.ag    # Parse, store AST, update current branch
cargo run -- run genesis       # Execute code from branch
cargo run -- branch            # List branches
cargo run -- branch <name>     # Create new branch
cargo run -- switch <name>     # Switch to a different branch
cargo run -- log               # Show commit log
```
