# Agentis Phase 1: Zero-Dependency AI-native Language & Integrated VCS

## Vision

Agentis is a system where the programming language and VCS are fused into a single
entity. Code is a hashed DAG of AST nodes. Every function and expression is a
content-addressed binary object stored in `.agentis/objects/`. This eliminates
text-based merge conflicts and allows AI agents to manipulate, branch, and test
code programmatically with 100% determinism.

## Tech Stack

| Component      | Choice                                        |
|----------------|-----------------------------------------------|
| Language       | Rust                                          |
| Storage        | `std::fs` (content-addressed `.agentis/objects/`) |
| Hashing        | `sha2` (SHA-256) — only external dependency   |
| Networking     | `std::net` (raw TCP, deferred to Phase 2)     |
| Serialization  | Manual binary (see open decision OD-1)        |
| Execution      | Tree-walking interpreter + Cognitive Budget   |

## Milestones

### M1: Lexer (`lexer.rs`)

Tokenize all language constructs:
- Keywords: `fn`, `let`, `if`, `else`, `return`, `true`, `false`
- AI-native keywords: `agent`, `prompt`, `validate`, `explore`
- Identifiers, integer/string literals
- Operators: `+`, `-`, `*`, `/`, `=`, `==`, `!=`, `<`, `>`, `<=`, `>=`
- Delimiters: `(`, `)`, `{`, `}`, `,`, `;`, `:`
- Comments: `//` line comments

**Deliverable:** `src/lexer.rs` + unit tests.

### M2: AST & Binary Serialization (`ast.rs`)

Define AST node enums:
- `Program`, `FnDecl`, `AgentDecl`
- `LetStmt`, `ReturnStmt`, `ExprStmt`
- `IfExpr`, `CallExpr`, `BinaryExpr`, `UnaryExpr`
- `Identifier`, `IntLiteral`, `StringLiteral`, `BoolLiteral`
- `ExploreBlock` (semantic branching construct)
- `PromptExpr`, `ValidateExpr`

Implement `to_bytes(&self) -> Vec<u8>` and `from_bytes(&[u8]) -> Result<Self>`
manually for every node.

**Deliverable:** `src/ast.rs` with full binary round-trip serialization + tests.

### M3: Parser — Recursive Descent (`parser.rs`)

Turn token stream into AST. Standard recursive descent:
- Operator precedence via Pratt parsing or explicit precedence levels
- Error recovery: collect multiple errors rather than bail on first

**Deliverable:** `src/parser.rs` + tests covering all AST node types.

### M4: Content-Addressed Storage — VCS Core (`storage.rs`)

- Compute SHA-256 of serialized AST nodes
- Store in `.agentis/objects/<first-2-chars>/<rest-of-hash>` (Git-style fanout)
- Load by hash, verify integrity on read
- Recursive storage: a `FnDecl` stores hashes of its child nodes, not inline data

**Deliverable:** `src/storage.rs` with save/load/verify + tests.

### M5: Execution Engine & Cognitive Budget (`evaluator.rs`)

Tree-walking interpreter:
- Environment with lexical scoping
- Function calls, recursion
- Basic built-in functions: `print`, `len`, `type`

Cognitive Budget (CB):
- Every operation deducts from a per-execution budget
- Cost table: arithmetic=1, comparison=1, variable lookup=1, function call=5,
  memory allocation=dynamic (proportional to size)
- `EvalError::CognitiveOverload` when budget hits zero
- Budget is configurable per execution context

**Deliverable:** `src/evaluator.rs` + tests (including CB exhaustion tests).

### M6: Reference Management & CLI (`refs.rs`, `main.rs`)

References:
- `.agentis/refs/heads/<branch>` — file containing root hash
- Default branch: **`genesis`** (never `main` or `master`)
- Branch operations: create, switch, list

CLI via `std::env::args`:
```
agentis init                  # Create .agentis/ structure + genesis branch
agentis commit <root_hash>    # Update current branch head
agentis run <branch>          # Execute code from branch's root hash
agentis branch <name>         # Create new branch from current head
agentis log                   # Show commit history (hash chain)
```

**Deliverable:** `src/refs.rs` + `src/main.rs` tying everything together.

## Open Decisions

### OD-1: Manual binary serialization vs. serde

Manual `to_bytes`/`from_bytes` produces hundreds of lines of error-prone
boilerplate. `serde` + `bincode` would cut this dramatically without changing the
architecture.

**Recommendation:** Start manual for M2 to understand the binary format deeply.
Revisit if maintenance cost becomes a bottleneck.

### OD-2: Semantics of AI-native constructs

The keywords `agent`, `prompt`, `validate`, `explore` are tokenized (M1) and
parsed (M3), but their **runtime semantics** are not yet specified. These are the
core differentiator of Agentis:

- **`agent`:** How does an agent declaration differ from a function? What
  capabilities does it unlock?
- **`prompt`:** Is this a built-in that calls an LLM? Is it a typed construct
  with schema validation?
- **`validate`:** Runtime assertion? Type guard? Contract?
- **`explore`:** Semantic branching — does it fork execution? Create a VCS
  branch? Both?

**This must be specified before M3 parsing can be finalized.** Parser structure
depends on what these constructs look like syntactically.

### OD-3: Type system

For deterministic agent execution, at minimum we need:
- Basic types: `int`, `string`, `bool`, `list`, `map`
- Function signatures with parameter/return types
- Possibly: effect types for `prompt` (marks functions that call LLMs)

**Recommendation:** Design a minimal type system alongside M3. Can be gradually
enforced (warnings first, errors later).

### OD-4: Execution model evolution

Tree-walking interpreter is the simplest but slowest model. Acceptable for Phase 1
but should evolve:
- **Phase 2:** Bytecode compilation + VM
- **Phase 3:** JIT or ahead-of-time compilation

## Project Structure (Phase 1 target)

```
src/
  main.rs         # CLI entry point
  lexer.rs        # Tokenizer
  ast.rs          # AST types + binary serialization
  parser.rs       # Recursive descent parser
  storage.rs      # Content-addressed object store
  evaluator.rs    # Tree-walking interpreter + CB
  refs.rs         # Branch/reference management
  error.rs        # Unified error types
```

## Success Criteria

Phase 1 is complete when:
1. You can write Agentis code, parse it, store the AST as content-addressed
   objects, and execute it from a branch reference
2. Cognitive Budget prevents runaway execution
3. All operations are deterministic — same input always produces same hash,
   same AST, same result
4. `agentis init && agentis commit <hash> && agentis run genesis` works end-to-end
