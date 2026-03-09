# Agentis Phase 4: First Contact

## Vision

Phases 1–3 built the engine. Phase 4 makes it **visible**. Someone downloads
Agentis, reads a page of docs, runs an example with their own LLM (flat-rate
CLI, local ollama, or API), and understands what "everything is prompt" means
in practice.

No new language features. No stdlib. The language is deliberately minimal —
it's an orchestration layer for AI, not a general-purpose programming language.

## Design Principle: Everything Is Prompt

In Ruby, everything is an object. In Agentis, **everything is a prompt**.

There is no `string.split()`. There is no `list.filter()`. There is no stdlib.
If an agent needs to split a string, it asks the LLM. If it needs to filter a
list, it asks the LLM. Every data transformation is a prompt.

This is not a limitation — it's the core design:

- **CB cost is the constraint.** 50 CB per prompt forces agents to batch work
  into few, large prompts instead of many trivial ones. A well-designed agent
  does one prompt that extracts, transforms, and validates. A badly-designed
  agent burns budget on 20 micro-prompts. CB is evolutionary pressure.
- **No determinism needed.** If determinism matters, use `validate`. The agent
  proposes, the predicates dispose. `explore` branches survive or die.
- **AI understands context.** `prompt("extract emails from this text", data)`
  handles edge cases that no regex ever will. The LLM is the stdlib.

## Tech Stack Rules

- No new dependencies. Phase 4 is docs, examples, and wiring.
- No stdlib. If someone asks "how do I sort a list?" the answer is `prompt`.
- No line/column error reporting. Code is a DAG of hashed AST nodes, not text
  files. Source `.ag` files are a transient input format for `agentis commit`.

## Milestones

- [ ] M15: Runtime Trace (see what agents are doing)
- [ ] M16: One-Step Workflow + AST-Native Diagnostics
- [ ] M17: Example Suite (demonstrate "everything is prompt")
- [ ] M18: Documentation (language reference, philosophy, getting started)

### M15: Runtime Trace

When an agent runs, the user sees nothing except `print()` output. For a demo
(and for debugging), execution needs to be observable.

Deliverable: changes to `evaluator.rs`, `main.rs`

**Trace events** (written to stderr, not stdout):

```
[agent scanner]       entered, CB=1000
[prompt]              "Analyze this page" → Report
[llm]                 backend=cli, command=claude
[llm]                 response: { title: "...", confidence: 0.92 }
[validate]            2 predicates: ✓ ✓
[spawn scanner]       agent=scanner, CB=1000, handle=#1
[await #1]            completed, result=Report { ... }
[explore "feature"]   entered, CB=500
[explore "feature"]   ✓ branch created
[CB]                  remaining: 340/1000
```

**Verbosity levels** (`.agentis/config`):

```
trace.level = quiet    # nothing (default, current behavior)
trace.level = normal   # agent enter/exit, prompt calls, explore outcomes
trace.level = verbose  # everything including LLM responses, CB changes
```

Implementation: a `Tracer` struct passed to `Evaluator`, called at key points.
No new dependencies — `eprintln!` with formatting. Tracer is a simple trait
so tests can capture trace output.

**CB cost:** Zero. Tracing is infrastructure, not computation.

### M16: One-Step Workflow + AST-Native Diagnostics

Two improvements to the CLI experience.

Deliverable: changes to `main.rs`, `evaluator.rs`, `error.rs`

**Part A — `agentis go <file>`:**

Combines `commit` + `run` in one step:

```bash
agentis go example.ag          # commit to current branch, then run it
agentis go example.ag --trace  # same, with trace.level=verbose
```

This is the demo command. Write a `.ag` file, run it, see what happens.
Internally: parse → store → update branch → load → typecheck → eval.
Same pipeline as `commit` + `run`, just one command.

**Part B — AST-native diagnostics:**

Errors currently say: `undefined variable: x`. They should say:

```
Error in agent "scanner" → statement #3 → call "process":
  undefined variable: x

  Node: fn_decl scanner [abc123def4...]
  Parent: program [9f8e7d6c5b...]
```

The error traces a path through the DAG — declaration name → statement index →
expression kind → node hash. No line numbers, no column numbers. The DAG IS
the source. If you want to inspect the node, you look it up by hash.

Implementation: `EvalError` variants get an optional `context: Vec<String>`
field for the trace path. `eval_*` methods push context as they descend.

**Part C — Config templates:**

`agentis init` creates `.agentis/config` with commented-out templates:

```
# LLM Backend — uncomment ONE section:

# --- Claude CLI (flat-rate, recommended) ---
# llm.backend = cli
# llm.command = claude
# llm.args = -p --output-format text

# --- Ollama (local, free) ---
# llm.backend = cli
# llm.command = ollama
# llm.args = run llama3

# --- Anthropic API (per-token) ---
# llm.backend = http
# llm.endpoint = https://api.anthropic.com/v1/messages
# llm.model = claude-sonnet-4-20250514
# llm.api_key_env = ANTHROPIC_API_KEY

# --- Gemini CLI ---
# llm.backend = cli
# llm.command = gemini
# llm.args = -p

# Agent limits
# max_concurrent_agents = 16

# Trace
# trace.level = quiet
```

New users see all options immediately. Uncomment and go.

### M17: Example Suite

5 programs demonstrating "everything is prompt" in practice. Each stored in
`examples/` as `.ag` files with inline comments.

Deliverable: `examples/` directory with `.ag` files

**Example 1: `hello.ag` — First contact**

```
// Simplest possible Agentis program.
// The LLM generates the greeting — even "hello world" is a prompt.
let greeting = prompt("Say hello to the world in a creative way", "world") -> string;
print(greeting);
```

**Example 2: `classify.ag` — Type-safe LLM output**

```
type Category {
    label: string,
    confidence: float
}

agent classifier(text: string) -> Category {
    cb 200;
    let result = prompt("Classify this text into a category", text) -> Category;
    validate result {
        len(result.label) > 0,
        result.confidence > 0.5,
        result.confidence <= 1.0
    };
    return result;
}

let r = classifier("The stock market crashed today");
print("Label:", r.label, "Confidence:", r.confidence);
```

**Example 3: `parallel.ag` — Multi-agent orchestration**

```
type Summary { title: string, key_points: string }

agent summarizer(topic: string) -> Summary {
    cb 200;
    let result = prompt("Summarize the key aspects of this topic", topic) -> Summary;
    validate result { len(result.title) > 0 };
    return result;
}

// Spawn 3 agents in parallel — each gets its own budget and scope
let h1 = spawn summarizer("quantum computing");
let h2 = spawn summarizer("gene editing");
let h3 = spawn summarizer("renewable energy");

// Await all results
let s1 = await(h1);
let s2 = await(h2);
let s3 = await(h3);

print("=== Summaries ===");
print(s1.title, "-", s1.key_points);
print(s2.title, "-", s2.key_points);
print(s3.title, "-", s3.key_points);
```

**Example 4: `explore.ag` — Evolutionary branching**

```
type Solution { approach: string, score: int }

agent solver(problem: string) -> Solution {
    cb 300;
    let result = prompt("Propose a solution and rate it 1-100", problem) -> Solution;
    validate result {
        result.score > 0,
        result.score <= 100
    };
    return result;
}

// explore forks execution — success creates a branch, failure is discarded
explore "approach-a" {
    let sol = solver("How to reduce latency in distributed systems?");
    validate sol { sol.score > 70 };
    print("Approach A:", sol.approach, "Score:", sol.score);
}

explore "approach-b" {
    let sol = solver("How to reduce latency in distributed systems?");
    validate sol { sol.score > 70 };
    print("Approach B:", sol.approach, "Score:", sol.score);
}

// Branches that survive have validated solutions.
// Branches that fail are silently discarded.
// Check with: agentis branch
```

**Example 5: `pipeline.ag` — Data pipeline (everything is prompt)**

```
// No stdlib. No string.split(). No list.filter().
// The LLM IS the data processor.

type Email { address: string, domain: string }
type Report { total: int, domains: string }

agent extract_emails(text: string) -> list<Email> {
    cb 200;
    return prompt("Extract all email addresses from this text, return as list of {address, domain}", text) -> list<Email>;
}

agent analyze(emails: list<Email>) -> Report {
    cb 200;
    return prompt("Count total emails and list unique domains, return as {total, domains}", emails) -> Report;
}

let raw = file_read("data.txt");
let emails = extract_emails(raw);
let report = analyze(emails);
print("Found", report.total, "emails across domains:", report.domains);
```

### M18: Documentation

Three documents. No fluff, no marketing. Technical reference for people who
want to understand and use Agentis.

Deliverable: `docs/` markdown files

**Document 1: `docs/language.md` — Language Reference**

Complete syntax and semantics:
- Declarations: `fn`, `agent`, `type`, `import`
- Statements: `let`, `return`, `cb`
- Expressions: arithmetic, comparison, if/else, calls, field access, literals
- AI constructs: `prompt`, `validate`, `explore`, `spawn`/`await`
- Types: `int`, `float`, `string`, `bool`, `void`, `list<T>`, `map<K,V>`, structs
- Builtins: `print`, `len`, `push`, `get`, `map_of`, `typeof`, `file_read`,
  `file_write`, `http_get`, `http_post`, `await`, `await_timeout`
- Cognitive Budget: cost table, `cb` override, `CognitiveOverload`
- Capabilities: what each CapKind guards

**Document 2: `docs/vcs.md` — VCS Model**

How code-as-DAG works:
- Source `.ag` is transient — committed to binary AST, then irrelevant
- Content-addressed storage: SHA-256 of serialized AST → object hash
- Commit = { tree_hash, parent_hash, timestamp }
- Branches = refs pointing to commit hashes
- `explore` creates branches automatically (success) or discards (failure)
- `import "hash"` loads code by content address
- P2P sync: HAVE/WANT/DATA/DONE over TCP
- Why there are no merge conflicts (AST-level dedup, not text-level diff)

**Document 3: `docs/philosophy.md` — Everything Is Prompt**

The design rationale:
- Why no stdlib: the LLM is the standard library
- Why CB exists: evolutionary pressure on agent design
- Why validate: runtime contracts on non-deterministic outputs
- Why explore: survival of the fittest, not manual branching
- Why agents are pure: isolation enables fearless concurrency
- Why content-addressed: reproducibility without text files
- Comparison: "In Python you import pandas. In Agentis you prompt."

## Implementation Order

1. **M15** (Runtime Trace) — foundation for debugging + demos
2. **M16** (Workflow + Diagnostics) — `agentis go`, better errors, config templates
3. **M17** (Examples) — needs M15+M16 to be demonstrable
4. **M18** (Docs) — written last, after examples prove the patterns work

## Success Criteria

Phase 4 is complete when:
1. `agentis init && agentis go example.ag --trace` shows a full agent run
   with visible LLM calls, validation, and CB tracking
2. Errors identify the DAG node and declaration path, not source line numbers
3. 5 example programs run with any LLM backend (CLI/local/HTTP)
4. A new user can read the docs and understand what Agentis is, why it exists,
   and how to write their first agent — in under 10 minutes
