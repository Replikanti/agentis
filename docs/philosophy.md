# Everything Is Prompt

In Ruby, everything is an object. In Agentis, **everything is a prompt**.

## Why No Stdlib

There is no `string.split()`. No `list.filter()`. No `sort()`. No regex.
No stdlib.

If an agent needs to split a string, it asks the LLM. If it needs to filter
a list, it asks the LLM. If it needs to extract emails from text, it asks
the LLM. Every data transformation is a prompt.

```
// Python: emails = re.findall(r'[\w.]+@[\w.]+', text)
// Agentis:
let emails = prompt("Extract all email addresses", text) -> list<string>;
```

The LLM handles edge cases that no regex ever will. It understands context,
intent, and ambiguity. The LLM is the standard library.

## Why CB Exists

Cognitive Budget is **evolutionary pressure**.

A prompt costs 50 CB. A well-designed agent does one prompt that extracts,
transforms, and validates in a single call. A badly-designed agent wastes
budget on 20 micro-prompts doing trivial string operations.

CB doesn't exist to save money. It exists to force agents to think in
batches — to compose large, efficient prompts instead of simulating
imperative string manipulation through an LLM. The constraint shapes
better agent design.

```
// Bad: 3 prompts = 150 CB
let raw = prompt("extract names", text) -> list<string>;
let sorted = prompt("sort these names", raw) -> list<string>;
let first = prompt("get the first item", sorted) -> string;

// Good: 1 prompt = 50 CB
let first = prompt("Extract all names from this text, sort alphabetically, return only the first", text) -> string;
```

## Why Validate

LLM output is non-deterministic. The same prompt can return different results.
`validate` is the answer.

```
let result = prompt("Rate this 1-100", text) -> int;
validate result {
    result > 0,
    result <= 100
};
```

The agent proposes, the predicates dispose. If validation fails, the value
doesn't propagate. Inside `explore`, validation failure means the branch
dies — only validated solutions survive.

`validate` is not error handling. It's a **fitness function**.

## Why Explore

`explore` is natural selection for code paths.

```
explore "approach-a" {
    let sol = solver(problem);
    validate sol { sol.score > 70 };
}

explore "approach-b" {
    let sol = solver(problem);
    validate sol { sol.score > 70 };
}
```

Each block runs in total isolation. Success creates a VCS branch. Failure
is silently discarded — no error, no side effect. Check which approaches
survived with `agentis branch`.

This is not manual branching. You don't choose which branch wins. The
validation predicates decide. Survival of the fittest.

## Why Agents Are Pure

Agents run in **isolated scope**. They cannot read or modify the caller's
variables. When spawned, they run on separate threads with their own budget.

This enables fearless concurrency. Three spawned agents cannot corrupt each
other's state. They execute independently, and the caller awaits their
results. There are no race conditions because there is no shared mutable state.

Purity also makes agents reproducible — given the same inputs and the same
LLM responses, an agent produces the same outputs.

## Why Content-Addressed

Source `.ag` files are a transient input format. After `agentis commit`, the
code exists as a hashed binary AST in `.agentis/objects/`. The hash IS the
identity.

This means:
- No filenames to manage. Import by hash.
- No merge conflicts. Different code = different hash.
- No text diffs. The unit of change is a program, not a line.
- Built-in integrity. Every read is verified.
- Natural deduplication. Same subtree = same hash = stored once.

Errors don't say "line 42, column 5" because there is no line 42. The code
is a DAG of hashed nodes. Errors reference the DAG path:
`agent "scanner" → fn "process": undefined variable: x`.

## Comparison

| Task | Python | Agentis |
|------|--------|---------|
| Split a string | `s.split(",")` | `prompt("split by comma", s) -> list<string>` |
| Filter a list | `[x for x in lst if x > 5]` | `prompt("keep items > 5", lst) -> list<int>` |
| Parse emails | `re.findall(...)` | `prompt("extract emails", text) -> list<string>` |
| Sort data | `sorted(data)` | `prompt("sort ascending", data) -> list<int>` |
| Import code | `from utils import func` | `import "sha256hash" { func };` |
| Branch code | `git checkout -b feature` | `explore "feature" { ... }` |

Every row in the left column is a deterministic stdlib call. Every row in the
right column is an LLM call. That's the point.

## When NOT to Use Agentis

Agentis is the wrong tool when you need:

- **Tight loops.** If you need to iterate 10,000 times, each iteration costs
  CB. Agentis is not for number crunching.
- **Deterministic string parsing.** If you need to split a CSV the exact same
  way every time, use Python. LLMs are probabilistic.
- **Sub-millisecond latency.** Every prompt is a network call (2-20 seconds).
  Agentis programs are slow by design.
- **Large data volumes.** Sending 10MB through an LLM prompt is wasteful and
  expensive. Agentis works best with small, meaningful inputs.
- **Programmer productivity.** If writing `prompt("split", s)` instead of
  `s.split(",")` annoys you, that's intentional. Agentis forces you to
  rethink your data pipeline in terms of AI operations, not string manipulation.

Agentis is for **AI orchestration** — coordinating multiple agents that
reason about data, make decisions, and produce structured outputs. If your
problem is "make an AI do a complex task with validation and branching,"
Agentis is built for that. If your problem is "parse this CSV," use awk.
