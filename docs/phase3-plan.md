# Agentis Phase 3: Live Agent Runtime

## Vision

Phase 1 = language + VCS. Phase 2 = cloud-native runtime. Phase 3 = **agents
actually work autonomously** — real LLM calls, real I/O, multi-agent coordination.
"Operating system for AI agents" becomes a functioning reality, not just a mock.

## Tech Stack Rules

- Language: Rust
- New dependencies (strictly limited):
  - `ureq` — synchronous, minimal HTTP client (no async, no Tokio)
  - No `reqwest`, no `hyper`, no heavy async runtimes
- JSON: Hand-rolled minimal JSON builder/parser utility (`json.rs`, ~150 LOC).
  No `serde`, no `serde_json`. Reused across LLM requests, config parsing,
  and module metadata.
- No async frameworks. Use `std::thread`, `std::net`, `std::sync`.
- Security: All file paths MUST be canonicalized (`std::fs::canonicalize()`)
  to prevent `../` traversal attacks outside the sandbox.

## Execution Model Clarification

**WASM is a compilation target, not an execution path in Phase 3.**

Phase 2 produced a WASM compiler backend (`compiler.rs`) that generates `.wasm`
binaries. However, Phase 2 explicitly forbids heavy WASM runtimes (no wasmtime,
no wasmer, no wasmi). Therefore:

- **All Phase 3 runtime features (LLM calls, I/O, spawn/await) run through the
  tree-walking interpreter** (`evaluator.rs`). This is the primary execution path.
- The WASM compiler remains a **build artifact generator** — useful for
  distributing pre-compiled modules or running in external WASM runtimes that
  the user provides.
- The WASM import section (Phase 2: `cap_check`, `cap_revoke`, `host_print`)
  defines the **contract** that any future WASM host must implement. Phase 3
  does not implement such a host.
- If WASM execution becomes a goal, it belongs in a future phase with an
  explicit decision on which runtime to allow.

## Milestones

- [x] M11: Pluggable LLM Backend (prompt stops being a mock)
- [x] M12: Capability-Gated I/O (real file read/write + HTTP via OCap)
- [x] M13: Module System (import/export across Agentis repositories)
- [x] M14: Multi-Agent Orchestration (message passing, parallel agents)

### M11: Pluggable LLM Backend

`prompt` finally calls a real LLM. Configuration via `.agentis/config`.

Deliverable: `llm.rs`, `json.rs`, changes to `evaluator.rs` and `main.rs`

**Prerequisite — JSON utility (`json.rs`):**

Before implementing the HTTP backend, build a minimal safe JSON builder/parser.
This prevents raw `format!()` string injection vulnerabilities when constructing
API requests. The utility is reused across M11 (LLM requests/responses),
M12 (config parsing), and M13 (module metadata).

Covers: string escaping, numbers, booleans, null, arrays, objects.
Does NOT cover: streaming, comments, trailing commas.

**LlmBackend trait:**

```rust
trait LlmBackend {
    fn complete(&self, instruction: &str, input: &str, return_type: &TypeAnnotation)
        -> Result<Value, LlmError>;
}
```

Implementations:
- `MockBackend` — existing deterministic stub values (for tests, default)
- `CliBackend` — spawns `claude` CLI as subprocess (flat-rate subscription,
  no per-token billing). Recommended for development.
- `HttpBackend` — HTTPS via `ureq` to API endpoint (per-token billing)

**Configuration** (`.agentis/config`, simple `key = value` format):

```
# Option 1: CLI backend — Claude (flat-rate, recommended)
llm.backend = cli
llm.command = claude
llm.args = -p --output-format text
llm.model = claude-sonnet-4-20250514
llm.max_retries = 2

# Option 1b: CLI backend — Gemini
# llm.backend = cli
# llm.command = gemini
# llm.args = -p
# llm.model = gemini-2.5-pro

# Option 1c: CLI backend — any tool accepting stdin
# llm.backend = cli
# llm.command = my-tool
# llm.args = --json --stdin

# Option 2: HTTP API backend (per-token billing)
# llm.backend = http
# llm.endpoint = https://api.anthropic.com/v1/messages
# llm.model = claude-sonnet-4-20250514
# llm.api_key_env = ANTHROPIC_API_KEY
# llm.max_retries = 2
```

Config parsing uses hand-rolled line parser (not TOML, not JSON). Format:
`key = value`, `#` comments, no nesting. Sufficient for all Phase 3 needs.

**Defensive JSON parsing:** LLMs can hallucinate structure — the parser must
never panic on malformed output. Strategy:
1. Parse JSON response using `json.rs` utility
2. Validate parsed values against declared return type
3. On type mismatch: re-prompt with error feedback (up to `llm.max_retries`
   attempts, default 2). Each retry costs 50 CB.
4. After retries exhausted → `EvalError::TypeError` with detail on what
   the LLM returned vs. what was expected
5. Retry attempts logged visibly: `[LLM retry 1/2: type mismatch — expected
   int, got string]`

CB cost: 50 per prompt attempt (including each retry). A prompt with 2 retries
costs 150 CB total. Real HTTP round-trip cost is visible in the budget.

### M12: Capability-Gated I/O

The OCap system from Phase 2 finally guards real operations, not just declarations.

Deliverable: `io.rs`, changes to `evaluator.rs` and `capabilities.rs`

**Operations:**

| Operation    | Capability | Builtin                             |
|--------------|------------|-------------------------------------|
| Read file    | FileRead   | `file_read(path) -> string`         |
| Write file   | FileWrite  | `file_write(path, content) -> void` |
| HTTP GET     | NetConnect | `http_get(url) -> string`           |
| HTTP POST    | NetConnect | `http_post(url, body) -> string`    |

HTTP operations use the same `ureq` dependency from M11.

**Sandboxing:**
- FileRead/FileWrite restricted to `.agentis/sandbox/` directory
- All paths canonicalized via `std::fs::canonicalize()` before any I/O.
  Resolved path must start with the canonical sandbox prefix — otherwise
  `EvalError::General("path outside sandbox")`. This prevents `../` traversal.
- NetConnect restricted to domain whitelist (configurable in `.agentis/config`)
- Every operation goes through `require_cap()` — no grant = `CapabilityDenied`

**Execution path:** All I/O runs through the tree-walking interpreter only.
The WASM import section from Phase 2 defines the host function contract but
Phase 3 does not implement a WASM host. `io.rs` is designed as a standalone
module so a future WASM host can call the same sandboxing logic.

**CB costs:**

| Operation  | Cost |
|------------|------|
| file_read  | 10   |
| file_write | 10   |
| http_get   | 25   |
| http_post  | 25   |

### M13: Module System

Import/export code across Agentis repositories. Code is content-addressed —
import is a hash, not a path.

Deliverable: extensions to `ast.rs`, `parser.rs`, `evaluator.rs`

**Syntax:**

```
import "sha256hash" as utils;    // import entire program under alias
import "sha256hash" { scan };    // import specific function
```

**Mechanism:**
- Import = load AST from object store by hash
- If hash doesn't exist locally → fetch via M7's existing TCP sync protocol
  (`network::sync_push_pull`). No new network code needed.
- Hash verification is automatic: content-addressed storage means the hash
  IS the name. If `store.load(hash)` succeeds, integrity is guaranteed by
  the SHA-256 check in `storage.rs`. No separate signature needed.
- Imported functions/agents registered in evaluator scope
- No cyclic imports (DAG topological ordering — detect cycles at import
  resolution time, error immediately)

**Peer configuration** (`.agentis/config`):

```
sync.peers = 192.168.1.10:9461, 10.0.0.5:9461
```

**New AST node:**
- `ImportDecl { hash: String, alias: Option<String>, names: Option<Vec<String>> }`

### M14: Multi-Agent Orchestration

Agents run in isolation and communicate via message passing.

Deliverable: `orchestration.rs`, changes to `evaluator.rs`, `ast.rs`, `parser.rs`

**Syntax:**

```
agent coordinator() {
    let a = spawn scanner("url1");     // starts agent, returns handle
    let b = spawn scanner("url2");
    let result_a = await(a);           // blocks until completion
    let result_b = await(b);
    return merge(result_a, result_b);
}
```

**Mechanism:**
- `spawn(agent, args...)` — runs agent in isolated `std::thread::spawn`,
  returns `AgentHandle`
- `await(handle)` — blocks via `JoinHandle<Result<Value, EvalError>>`,
  returns result or propagates error
- Each agent has its own: scope, CB, capability set, snapshot manager
- Parent agent can pass capabilities to spawned agent (delegation)
- `AgentHandle` is a new Value variant wrapping
  `Arc<Mutex<Option<JoinHandle<Result<Value, EvalError>>>>>`

**CB rules:**
- `spawn` costs 10 CB from parent budget
- Spawned agent gets its own budget (from `cb` declaration or default)
- Parent budget is not charged for child's work

**Fork bomb prevention:** A spawned agent's `spawn` calls also cost 10 CB from
its own budget. Since every agent has a finite budget and `spawn` is not free,
recursive spawning is bounded by CB exhaustion. Additional safeguard: global
`max_concurrent_agents` limit (default 16, configurable in `.agentis/config`).
Exceeding it → `EvalError::General("agent limit exceeded")`.

**Error propagation:**
- If spawned agent fails → `await` returns the `EvalError`
- Timeout: optional `await_timeout(handle, ms)` — `CognitiveOverload` on exceed

## Implementation Order

Recommended sequence within Phase 3:

1. **`json.rs`** — JSON builder/parser utility (prerequisite for M11 and M12)
2. **Config parser** — `.agentis/config` reader (prerequisite for M11)
3. **M11** — LLM backend (MockBackend first, then HttpBackend)
4. **M12** — I/O with sandboxing (file ops first, HTTP ops reuse ureq from M11)
5. **M13** — Module system (builds on existing storage + network)
6. **M14** — Multi-agent orchestration (builds on everything above)

## Success Criteria

Phase 3 is complete when:
1. `prompt` calls a real LLM API and returns typed results
2. Agents can read/write files and make HTTP calls — all through OCap
3. Code can be imported by hash from another repository
4. Multiple agents run in parallel via `spawn`/`await`
5. Full end-to-end: `agentis init && agentis commit prog.ag && agentis run genesis`
   with a real LLM
