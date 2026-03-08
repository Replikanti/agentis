# Agentis Phase 1: Zero-Dependency AI-native Language & Integrated VCS

## Vision

Agentis is a system where the programming language and VCS are fused into a single
entity. Code is a hashed DAG of AST nodes. Every function and expression is a
content-addressed binary object stored in `.agentis/objects/`. This eliminates
text-based merge conflicts and allows AI agents to manipulate, branch, and test
code programmatically with 100% determinism.

## Tech Stack

| Component      | Choice                                          |
|----------------|-------------------------------------------------|
| Language       | Rust                                            |
| Storage        | `std::fs` (content-addressed `.agentis/objects/`) |
| Hashing        | `sha2` (SHA-256) — only external dependency     |
| Serialization  | Manual binary `to_bytes`/`from_bytes`            |
| Execution      | Tree-walking interpreter + Cognitive Budget     |

## Type System

Statically typed, TypeScript-style:

- **Primitive types:** `int`, `float`, `string`, `bool`
- **Collections:** `list<T>`, `map<K, V>`
- **User-defined types:**
  ```
  type Category {
      label: string,
      confidence: float
  }
  ```
- **Type inference** where unambiguous (`let x = 5` → `int`)
- **Structural typing** — two types with the same shape are compatible
- **Mandatory annotations** on `fn`/`agent` signatures and `prompt` return types

## AI-native Constructs

### `agent`

Like `fn`, but with an isolated execution context and its own Cognitive Budget.
Agents are pure — they cannot mutate outer state. Side effects only through
`prompt`.

```
agent scanner(url: string) -> Report {
    cb 1000;   // declare this agent's Cognitive Budget
    let data = fetch(url);
    let result = prompt("Analyze this page", data) -> Report;
    validate result {
        result.confidence > 0.8
    }
    return result;
}
```

- Own CB, declared via `cb <amount>;` or falls back to a default
- No access to mutable outer state (pure)
- Can call `prompt`, `validate`, other agents, and regular `fn`

### `prompt`

Typed LLM call with schema-enforced output. In Phase 1, implemented as a **mock**
(returns deterministic stub data matching the declared return type) to avoid
external API dependencies and token costs.

```
let result = prompt("Classify this text", input) -> Category;
```

- Return type is mandatory — output is validated against it
- CB cost: 50 (configurable) — prompt is an expensive operation
- Phase 2+: pluggable LLM backends via configuration

### `validate`

Runtime contract — a set of boolean predicates that must all hold. If any fails,
raises `EvalError::ValidationFailed` with detail on which predicate failed.

```
validate result {
    result.confidence > 0.8,
    result.category != "unknown"
}
```

- Typically used after `prompt` to verify LLM output
- CB cost: 1 per predicate

### `explore`

Semantic branching — forks execution in an isolated context and automatically
creates a VCS branch on success.

```
explore "feature-name" {
    // runs with snapshot of current state
    // own scope, isolated from outer context
    let improved = prompt("Refactor this", code) -> Code;
    validate improved { improved.score > code.score }
}
// if block succeeds → .agentis/refs/heads/feature-name is created
// if block fails (error, validation, CB exhaustion) → nothing is persisted
```

- **Success** = branch automatically created with the resulting AST state
- **Failure** (error, validation fail, CB exhausted) = no side effects, no branch
- Replaces traditional git branch + merge — merges happen at AST hash level,
  not text level

## Milestones

- [ ] M1: Lexer
- [ ] M2: AST & Binary Serialization
- [ ] M3: Parser (Recursive Descent)
- [ ] M4: Content-Addressed Storage (VCS Core)
- [ ] M5: Execution Engine & Cognitive Budget
- [ ] M6: Reference Management & CLI

### M1: Lexer (`lexer.rs`)

Tokenize all language constructs:
- Keywords: `fn`, `let`, `if`, `else`, `return`, `true`, `false`, `type`
- AI-native keywords: `agent`, `prompt`, `validate`, `explore`, `cb`
- Type keywords: `int`, `float`, `string`, `bool`, `list`, `map`
- Identifiers, integer/float/string literals
- Operators: `+`, `-`, `*`, `/`, `=`, `==`, `!=`, `<`, `>`, `<=`, `>=`, `->`, `.`
- Delimiters: `(`, `)`, `{`, `}`, `,`, `;`, `:`
- Comments: `//` line comments

**Deliverable:** `src/lexer.rs` + unit tests.

### M2: AST & Binary Serialization (`ast.rs`)

Define AST node enums:
- `Program`, `FnDecl`, `AgentDecl`, `TypeDecl`
- `LetStmt`, `ReturnStmt`, `ExprStmt`, `CbStmt`
- `IfExpr`, `CallExpr`, `BinaryExpr`, `UnaryExpr`
- `Identifier`, `IntLiteral`, `FloatLiteral`, `StringLiteral`, `BoolLiteral`
- `ExploreBlock`, `PromptExpr`, `ValidateExpr`
- Type annotation nodes: `TypeAnnotation`, `GenericType`

Implement `to_bytes(&self) -> Vec<u8>` and `from_bytes(&[u8]) -> Result<Self>`
manually for every node. Manual serialization gives full control over the binary
format — critical because SHA-256 hashes are computed from these bytes, and any
format instability would break content addressing.

**Deliverable:** `src/ast.rs` with full binary round-trip serialization + tests.

### M3: Parser — Recursive Descent (`parser.rs`)

Turn token stream into typed AST. Standard recursive descent:
- Operator precedence via Pratt parsing or explicit precedence levels
- Type annotation parsing (`name: type`, `-> ReturnType`)
- AI-native construct parsing (`agent`, `prompt ... -> Type`, `validate`, `explore`)
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
- Static type checking before execution
- `agent` execution with isolated scope and own CB
- `prompt` as mock (returns deterministic stub data matching return type)
- `validate` as runtime predicate checker
- `explore` as isolated execution context with branch creation on success

Cognitive Budget (CB):
- Every operation deducts from a per-execution budget
- Cost table: arithmetic=1, comparison=1, variable lookup=1, function call=5,
  prompt=50, validate predicate=1
- `EvalError::CognitiveOverload` when budget hits zero
- Agents declare own budget via `cb` statement; default fallback for those without
- `EvalError::ValidationFailed` when validate predicates fail

**Deliverable:** `src/evaluator.rs` + tests (including CB exhaustion and
validation failure tests).

### M6: Reference Management & CLI (`refs.rs`, `main.rs`)

References:
- `.agentis/refs/heads/<branch>` — file containing root hash
- Default branch: **`genesis`** (never `main` or `master`)
- Branch operations: create, switch, list
- `explore` blocks create branches automatically on success

CLI via `std::env::args`:
```
agentis init                  # Create .agentis/ structure + genesis branch
agentis commit <root_hash>    # Update current branch head
agentis run <branch>          # Execute code from branch's root hash
agentis branch <name>         # Create new branch from current head
agentis log                   # Show commit history (hash chain)
```

**Deliverable:** `src/refs.rs` + `src/main.rs` tying everything together.

## Execution Model Roadmap

- **Phase 1:** Tree-walking interpreter (simple, easy debugging)
- **Phase 2:** Bytecode compilation + stack-based VM
- **Phase 3:** LLVM backend or WASM target

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
1. You can write Agentis code with type annotations, parse it, store the AST as
   content-addressed objects, and execute it from a branch reference
2. `agent`, `prompt` (mock), `validate`, and `explore` work as specified
3. Cognitive Budget prevents runaway execution
4. All operations are deterministic — same input always produces same hash,
   same AST, same result
5. `agentis init && agentis commit <hash> && agentis run genesis` works end-to-end
