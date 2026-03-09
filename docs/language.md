# Agentis Language Reference

## Types

### Primitives

| Type | Literal | Notes |
|------|---------|-------|
| `int` | `42`, `-1`, `0` | 64-bit signed integer |
| `float` | `3.14`, `0.5` | 64-bit floating point |
| `string` | `"hello"` | UTF-8, escapes: `\n \t \r \\ \"` |
| `bool` | `true`, `false` | |
| `void` | — | Implicit return of statements |

### Compound types

| Type | Syntax | Notes |
|------|--------|-------|
| `list<T>` | `[1, 2, 3]` | Ordered, heterogeneous at runtime |
| `map<K,V>` | `map_of(k1, v1, k2, v2)` | Ordered key-value pairs |
| Struct | `type Name { field: T }` | User-defined, named structural types |
| `agent_handle` | — | Returned by `spawn`, consumed by `await` |

### Type annotations

Required on function/agent parameters and return types. Omitted on `let`
(inferred from the right-hand side).

```
fn add(a: int, b: int) -> int { ... }
agent scanner(url: string) -> Report { ... }
let x = 42;                    // inferred as int
```

Generic types use angle brackets: `list<int>`, `map<string, float>`.

---

## Declarations

### `fn` — Pure function

```
fn name(param: type, ...) -> return_type {
    // body
}
```

Functions share the caller's scope and budget. Not isolated.

### `agent` — Isolated execution unit

```
agent name(param: type, ...) -> return_type {
    // body
}
```

Agents run in **isolated scope** — they cannot read or modify the caller's
variables. When called directly, they execute synchronously. When `spawn`ed,
they run on a separate thread with their own budget copy.

### `type` — Struct definition

```
type Report {
    title: string,
    confidence: float
}
```

Defines a named structural type. Fields are accessed with dot notation:
`report.title`. Used as return types for `prompt` to get structured LLM output.

### `import` — Load code by content hash

```
import "abc123...";                  // import everything
import "abc123..." as utils;         // namespaced: utils.func_name
import "abc123..." { func1, Type1 }; // selective
```

Loads a previously committed program from the object store by its SHA-256 hash.
Cycle detection prevents infinite recursion.

---

## Statements

### `let` — Variable binding

```
let name = expression;
```

Immutable binding (no reassignment). Type is inferred.

### `return` — Early exit from function/agent

```
return expression;
return;              // returns void
```

### `cb` — Cognitive Budget override

```
cb 500;
```

Sets the execution budget to the given value. Typically used at the start of
an agent body to cap its resource usage.

---

## Expressions

### Arithmetic

`+`, `-`, `*`, `/` on `int` and `float`. Mixed int/float promotes to float.
String concatenation with `+`. Division by zero raises `DivisionByZero`.

### Comparison

`==`, `!=`, `<`, `>`, `<=`, `>=` — works on int, float, string, bool.
Mixed int/float comparisons are supported.

### Unary

`-` (negation on int/float), `!` (logical not on bool).

### `if` / `else`

```
let result = if condition {
    // then branch
} else {
    // else branch
};
```

Both branches are expressions (return the last value). `else` is optional.
Truthiness: `false`, `0`, `""`, `[]` are falsy; everything else is truthy.

### Field access

```
result.title
result.confidence
```

Works on struct values. Raises `UndefinedField` if the field doesn't exist.

### List and map literals

```
let items = [1, 2, 3];
let pairs = map_of("a", 1, "b", 2);
```

### Function/agent call

```
let result = my_function(arg1, arg2);
let report = my_agent(data);
```

Arity is checked at runtime. CB cost: 5 per call.

---

## AI-Native Constructs

### `prompt` — Typed LLM call

```
let result = prompt("instruction", input_expr) -> ReturnType;
```

Sends instruction + input to the configured LLM backend. The response is
parsed and coerced to `ReturnType`. For struct types, the LLM receives
field names and types in the prompt.

**CB cost: 50.** This is intentionally expensive — it forces agents to batch
work into few, large prompts rather than many trivial ones.

Requires the `Prompt` capability.

### `validate` — Runtime predicates

```
validate target_expr {
    predicate1,
    predicate2,
    ...
};
```

Evaluates each predicate (must return `bool`). If any predicate is `false`,
raises `ValidationFailed` with the predicate index. If all pass, returns the
target value. CB cost: 1 per predicate.

### `explore` — Evolutionary branching

```
explore "branch-name" {
    // body
}
```

Runs the body in **full isolation** (saved/restored environment and budget).
- If the body completes successfully → a VCS branch is created.
- If any error occurs (validation failure, budget exhaustion, etc.) →
  environment is silently rolled back, no side effects.

This is natural selection for code paths. Use `validate` inside `explore`
to set survival criteria.

Requires the `VcsWrite` capability. CB cost: 1 to enter.

### `spawn` / `await` — Parallel agent execution

```
let handle = spawn agent_name(args...);
let result = await(handle);
let result = await_timeout(handle, 5000);  // ms
```

`spawn` launches an agent on a new OS thread with its own budget copy.
Returns an `agent_handle`. `await` blocks until the agent completes and
returns its result. `await_timeout` raises `CognitiveOverload` if the
agent doesn't finish in time.

Max concurrent agents: 16 (configurable).
CB cost: 10 for spawn, 5 for await (call cost).

---

## Built-in Functions

| Function | Args | Returns | CB | Cap | Notes |
|----------|------|---------|----|-----|-------|
| `print(...)` | any | void | 5 | Stdout | Variadic, space-separated |
| `len(x)` | string/list/map | int | 5 | — | |
| `push(list, item)` | list, any | list | 5 | — | Returns new list |
| `get(coll, key)` | list+int / map+any | any | 5 | — | Index or key lookup |
| `map_of(k,v,...)` | even number | map | 5 | — | Construct map from pairs |
| `typeof(x)` | any | string | 5 | — | Returns type name |
| `file_read(path)` | string | string | 15 | FileRead | Sandboxed to `.agentis/sandbox/` |
| `file_write(path, content)` | string, string | void | 15 | FileWrite | Sandboxed |
| `http_get(url)` | string | string | 30 | NetConnect | Domain-whitelisted |
| `http_post(url, body)` | string, string | string | 30 | NetConnect | Domain-whitelisted |
| `await(handle)` | agent_handle | any | 5 | — | Block until agent completes |
| `await_timeout(handle, ms)` | agent_handle, int | any | 5 | — | Timeout in milliseconds |

---

## Cognitive Budget (CB)

Every operation costs CB. When budget reaches zero, execution halts with
`CognitiveOverload`.

| Operation | Cost |
|-----------|------|
| Arithmetic, comparison, lookup, unary | 1 |
| Variable binding (`let`) | 1 |
| If/else branch | 1 |
| List/map literal construction | 1 |
| Validate (per predicate) | 1 |
| Explore enter | 1 |
| Function/agent call | 5 |
| Spawn | 10 |
| File I/O (`file_read`, `file_write`) | 10 + 5 (call) |
| HTTP (`http_get`, `http_post`) | 25 + 5 (call) |
| Prompt (LLM call) | 50 |

Override the budget with `cb <amount>;` at the start of a scope. Spawned
agents inherit the parent's current budget as their own.

---

## Capabilities

Operations that affect the outside world require explicit capability grants.
Without a grant, the operation raises `CapabilityDenied`.

| Capability | Guards |
|------------|--------|
| `Prompt` | `prompt()` LLM calls |
| `FileRead` | `file_read()` |
| `FileWrite` | `file_write()` |
| `NetConnect` | `http_get()`, `http_post()` |
| `NetListen` | Inbound network connections |
| `VcsRead` | Reading from object store |
| `VcsWrite` | `explore` branch creation |
| `Stdout` | `print()` |

Capabilities use unforgeable tokens (SHA-256 HMAC with per-registry secret).
They can be granted and revoked at runtime. Spawned agents receive all
capabilities by default.

---

## Comments

Line comments only: `// comment until end of line`

---

## Operator Precedence

From lowest to highest:

1. `==`, `!=`
2. `<`, `>`, `<=`, `>=`
3. `+`, `-`
4. `*`, `/`
5. Unary `-`, `!`
6. `.` (field access)
7. `()` (call)

---

## Error Diagnostics

Errors report a **DAG context path**, not line/column numbers. Source `.ag`
files are transient input — the canonical representation is the hashed AST.

```
agent "scanner" -> fn "process":
  undefined variable: x
```

Errors propagate context as they unwind through function/agent call chains
and explore blocks.
