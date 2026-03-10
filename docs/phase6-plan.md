# Phase 6: Resurrection & Developer Experience

## Vision

Phases 1-5 built a safe, AI-native runtime. Phase 6 makes it **usable**.

Agentis agents can now be interrupted and resumed without losing state.
Developers get an interactive REPL, a test runner, and richer error context.
No new builtins. No async. No language changes.

## Design Principle: DAG-Native, Not Line-Native

Agentis errors reference DAG node paths, not line:column. Code lives in
content-addressed objects, not text files. Phase 6 improves error context
by showing **what** failed (AST expression, agent name, hashes) — not
**where** in a text file. If a source `.ag` file is available from the
most recent commit, the error may include a code fragment as a hint, but
the canonical reference is always the DAG path.

## Milestones

- [x] M23: Snapshot CLI + Resurrection
- [ ] M24: REPL (`agentis repl`)
- [ ] M25: Test Runner (`agentis test`)
- [ ] M26: Rich Error Context

### M23: Snapshot CLI + Resurrection

Deliverable: changes to `main.rs`, `evaluator.rs`, `snapshot.rs`

Expose existing snapshot infrastructure via CLI. Agents recover state
after crash/restart. CB penalty on resurrection: evolutionary pressure
against fragile agents.

**CLI commands:**

```bash
agentis snapshot list              # list all snapshots (hash, budget, output lines)
agentis snapshot show <hash>       # show snapshot details (variables, budget, output)
agentis repl --resume <hash>       # start REPL with restored state (needs M24)
```

**Snapshot listing:**

```
HASH         CB        OUTPUT  SCOPES
a1b2c3d4e5f6 7340/10000  3 lines  2
f6e5d4c3b2a1 4200/10000  0 lines  1
```

**Resurrection flow:**

1. Load program from current branch (re-registers functions/types/agents).
2. Restore `MemorySnapshot` — scopes, budget, output.
3. Apply CB penalty: restored budget = `snapshot_budget * 0.7` (30% tax).
4. Drop into REPL (M24) or execute further statements.

The 30% CB tax means resurrected agents have less fuel than before death.
Agents that die repeatedly converge to zero budget. This is intentional:
resilient agents that checkpoint wisely survive; fragile agents don't.

**Implementation:**

- `SnapshotManager` already has `save`, `load`, `history`, `rollback_to`.
- Add `list_all()` method that returns `Vec<(Hash, MemorySnapshot)>`.
- Snapshots are already content-addressed and syncable via P2P.
- `MemorySnapshot` captures: scopes (variables), budget, output buffer.
- Functions/types/agents come from the program, not the snapshot.

**What snapshots do NOT capture** (by design):

- Function/type definitions (re-loaded from program on resurrection)
- Capability grants (re-granted by CLI flags / config)
- LLM backend / I/O context (runtime configuration, not agent state)
- Active agent threads (transient; AgentHandles serialize as Void)

### M24: REPL (`agentis repl`)

Deliverable: changes to `main.rs`, minor changes to `parser.rs`

Interactive evaluator. Parse one statement at a time, evaluate, print
result, keep state. The REPL is the natural interface for development
and for resumed agents (M23).

**Usage:**

```
$ agentis repl
agentis> let x = 5;
5
agentis> let y = x * 3;
15
agentis> print(y);
15
agentis> fn double(n: int) -> int { return n * 2; }
agentis> double(y)
30
agentis> prompt("greet", "hello") -> string
[llm] requesting mock  ...
[llm] received (0.0s)
"mock"
agentis> .budget
CB: 9895/10000
agentis> .snapshot
Snapshot saved: a1b2c3d4e5f6
agentis> .exit
```

**Dot-commands** (REPL meta-commands, not language constructs):

| Command | Action |
|---------|--------|
| `.exit` | Quit REPL |
| `.budget` | Show remaining CB / initial budget |
| `.snapshot` | Manually save snapshot |
| `.output` | Show accumulated output buffer |
| `.help` | Show available dot-commands |

**Implementation:**

- `Evaluator` already supports incremental `eval_statement()` calls
  with persistent state (env, functions, types, budget, output).
- Parser needs a `parse_single_declaration()` or `parse_repl_input()`
  method that parses one statement/declaration from a line.
- Multi-line detection: if input ends with `{` and braces aren't
  balanced, prompt for continuation lines (`...>` prompt).
- Read from stdin line by line (`std::io::stdin().read_line()`).
- Bare expressions (no `let`, no `;`) evaluate and print the result.
- REPL state: full Evaluator with VCS, I/O, LLM, tracer, audit.
- Each statement is a transaction boundary (snapshot persisted if
  persistence is enabled).

**REPL + Resume flow:**

```bash
agentis repl --resume a1b2c3d4e5f6
```

1. Load program from current branch.
2. First pass: register functions/types/agents.
3. Restore snapshot (scopes, budget * 0.7, output).
4. Print restored state summary.
5. Enter interactive loop.

**What the REPL is NOT:**

- Not a debugger (no breakpoints, no step-through).
- Not a notebook (no cell persistence — use `.snapshot` for that).
- Not a shell (no `!command` or subprocess execution).

### M25: Test Runner (`agentis test`)

Deliverable: changes to `main.rs`

Run a program and report `validate` outcomes as test results. Explore
branches are treated as test branches — each one either passes or fails.

**Usage:**

```bash
agentis test examples/explore.ag       # test one file
agentis test examples/*.ag             # test multiple files
agentis test examples/ --fail-fast     # stop on first failure
agentis test examples/ --verbose       # show validate predicate details
```

**Output:**

```
examples/explore.ag
  explore "approach-a" .............. PASS
  explore "approach-b" .............. FAIL
    validate predicate #1: FAIL (got false)

examples/classify.ag
  (no explore blocks)
  validate .......................... PASS (3 predicates)

Results: 2 passed, 1 failed, 3 total
```

**Semantics:**

- Parse and execute the program normally (same as `agentis go`).
- Collect all `explore` block outcomes (success/failure).
- Collect all `validate` results (each is a test assertion).
- Report in a structured format with exit code:
  - Exit 0: all passed.
  - Exit 1: any failure.
- `--fail-fast`: stop after first failing file.
- `--verbose`: show individual predicate results (uses tracer verbose).

**Implementation:**

- Reuse `cmd_go` logic but suppress normal output.
- `Evaluator` already tracks `explore_branches` (successes).
- Failed explores produce `EvalError` with context — capture these.
- `validate` failures produce `EvalError::ValidationFailed` with
  `predicate_index` and `detail` — capture and report.
- Need to catch errors per-explore rather than aborting the whole
  program. This means wrapping each explore evaluation in error
  handling that records the outcome.

**What tests are NOT:**

- Not a new language construct (no `test` keyword).
- Not a framework (no setup/teardown, no mocking builtins).
- Tests are just programs with `validate` and `explore` blocks.

### M26: Rich Error Context

Deliverable: changes to `evaluator.rs`, `ast.rs`

Improve runtime error messages with DAG-native context. Show what
expression failed, what types were involved, and what agent was running.

**Current errors:**

```
runtime error: missing capability: pii_transmit
```

**After M26:**

```
runtime error in agent "summarizer":
  capability denied: pii_transmit
  at: prompt("Summarize this data", <input>) -> Summary
  input length: 1524 chars
  PII detected: email, phone

  Hint: grant PiiTransmit via --grant-pii or config pii_transmit = allow
```

**Approach:**

Add an optional `ErrorContext` struct to key error sites in the evaluator.
Not on every expression — only on high-value error points:

| Error site | Context added |
|------------|---------------|
| `eval_prompt` | instruction text, input length, return type, PII result |
| `eval_call` (arity) | function name, expected vs got params |
| `eval_call` (undefined) | name, similar names if any |
| `eval_validate` | predicate index, target value type, predicate expression |
| `eval_explore` (failure) | explore name, error from inner block |
| Capability denied | which capability, where to grant it |
| CB exhaustion | remaining budget, cost of operation, agent name |

**Implementation:**

- New struct `ErrorDetail` in `evaluator.rs` (not a language-level type):
  ```rust
  struct ErrorDetail {
      agent_name: Option<String>,
      expression_desc: String,  // "prompt(...) -> Type"
      hints: Vec<String>,       // actionable suggestions
  }
  ```
- Attach to `EvalError::InContext` or new variant `EvalError::Detailed`.
- Display implementation shows structured multi-line error.
- No changes to AST structure — context is built at evaluation time
  from available information (current agent name, expression fields).

**What this is NOT:**

- Not line:column source mapping (Agentis is DAG-native).
- Not a full error recovery system (errors still abort execution).
- Not exhaustive — only key error sites get rich context.

## Implementation Order

1. **M23** — snapshots CLI (foundation for resume)
2. **M24** — REPL (needs M23 for `--resume`, but usable standalone)
3. **M25** — test runner (independent, can parallel with M24)
4. **M26** — rich errors (independent, improves all prior milestones)

M24 and M25 can be developed in parallel — they don't depend on each
other. M26 improves the experience of both but isn't a prerequisite.

## Success Criteria

Phase 6 is complete when:
1. `agentis snapshot list` shows all persisted snapshots
2. `agentis repl` provides interactive evaluation with persistent state
3. `agentis repl --resume <hash>` restores from snapshot with CB penalty
4. `agentis test *.ag` reports validate/explore outcomes with exit codes
5. Runtime errors show agent name, expression context, and actionable hints
6. Zero new builtins. Zero new language syntax. Zero async.

## What Phase 6 Does NOT Include

- **No `publish()` builtin.** Shared memory = `file_write` + P2P sync.
- **No message passing builtins.** Communication = write to sandbox, sync.
- **No event loop or async runtime.** Sync Rust, `std::thread` only.
- **No distributed colony.** Single-instance focus. Colony is Phase 7+.
- **No debugger.** REPL is interactive evaluation, not step-through.
- **No line:column in errors.** DAG-native context, not text-file positions.
