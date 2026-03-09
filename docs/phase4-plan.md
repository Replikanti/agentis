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

**Who Agentis is NOT for:** If you need a tight loop running 10,000 times,
deterministic string parsing, or sub-millisecond latency — this is the wrong
tool. Agentis is not about programmer productivity. It's about AI orchestration.
If writing a prompt to split a string annoys you, that is intentional — it
forces you to rethink your data pipeline.

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
(and for debugging), execution needs to be observable. More critically: an LLM
call takes 2–20 seconds. A silent terminal during that time reads as "frozen"
to any user. This kills first impressions.

Deliverable: `trace.rs`, changes to `evaluator.rs`, `llm.rs`, `main.rs`

**Critical UX requirement — LLM wait indicator:**

Even at `trace.level = quiet`, the system MUST print a minimal progress
indicator to stderr before and after any LLM network call:

```
[llm] requesting claude-sonnet-4-20250514 ...
[llm] still waiting ... (5.0s)
[llm] received (7.8s)
```

The "still waiting" line repeats every 4 seconds if the LLM hasn't responded.
This is not optional. Without it, 60–80% of new users will Ctrl+C during
their first `agentis go` because the terminal appears frozen. No external
spinner crates — plain `eprint!` / `eprintln!` with `Instant::elapsed()`.
Implementation: spawn a timer thread that prints to stderr; cancel on response.

**Trace events** (written to stderr, not stdout):

```
[agent scanner]       entered, CB=1000
[prompt]              "Analyze this page" → Report
[llm]                 requesting claude ...
[llm]                 received (4.2s)
[llm]                 response: { title: "...", confidence: 0.92 }
[validate]            2 predicates: pass pass
[spawn scanner]       agent=scanner, CB=1000, handle=#1
[await #1]            completed, result=Report { ... }
[explore "feature"]   entered, CB=500
[explore "feature"]   branch created
[CB]                  remaining: 340/1000
```

**Verbosity levels** (`.agentis/config`):

```
trace.level = normal   # default — agent enter/exit, prompt calls, explore outcomes
trace.level = quiet    # only LLM wait indicators (the minimum)
trace.level = verbose  # everything including LLM responses, CB deltas
```

Note: default is `normal`, not `quiet`. New users need to see what's happening
under the hood to understand the execution model. They can opt into `quiet`
once they understand.

Implementation: a `Tracer` trait passed to `Evaluator`, called at key points.
No new dependencies — `eprintln!` with formatting. Trait allows tests to
capture trace output into a `Vec<String>` instead of stderr.

**CB cost:** Zero. Tracing is infrastructure, not computation.

### M16: One-Step Workflow + AST-Native Diagnostics

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

`agentis init` creates `.agentis/config` with a working default and
commented-out alternatives:

```
# LLM Backend — default is mock (no LLM needed).
# Uncomment ONE section below to use a real LLM:

llm.backend = mock

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

# --- xAI / Grok API ---
# llm.backend = http
# llm.endpoint = https://api.x.ai/v1/messages
# llm.model = grok-3
# llm.api_key_env = XAI_API_KEY

# Agent limits
# max_concurrent_agents = 16

# Trace (default: normal — shows agent lifecycle and prompt calls)
trace.level = normal
```

New users see all options immediately. Uncomment and go.

**Part D — `agentis doctor`:**

Pre-flight check that validates the environment:

```bash
$ agentis doctor
[ok] .agentis/ repository found
[ok] config loaded (llm.backend = cli)
[ok] claude found in PATH (/usr/local/bin/claude)
[ok] .agentis/sandbox/ exists (writable)
[!!] trace.level = quiet (consider 'normal' for debugging)
```

Checks:
- `.agentis/` exists and is valid
- Config parses without errors
- LLM backend is reachable: CLI command in PATH, or HTTP endpoint responds,
  or API key env var is set
- Sandbox directory exists and is writable
- Reports trace level as informational

No new dependencies. Just `std::process::Command` for `which`-style checks
and `std::fs` for directory validation. Prints human-readable summary to
stdout.

### M17: Example Suite

6 programs demonstrating "everything is prompt" in practice. Each stored in
`examples/` as `.ag` files with inline comments. **Ordered by execution time**
— the first example a user runs must produce output in under 8 seconds, even
on a slow local model.

Deliverable: `examples/` directory with `.ag` files + `examples/data.txt`

**Example 1: `fast-demo.ag` — Instant gratification (< 8s)**

```
// One prompt, tiny input, fast response.
// This is the first thing a new user should run.
cb 80;
let mood = prompt("Respond with exactly one word describing a mood", "") -> string;
print("Mood of the day:", mood);
```

Why first: single prompt, near-empty input, one-word output. Even ollama on
a laptop responds in 3–7 seconds. The user sees output fast and understands
the basic mechanic: prompt in, typed value out.

**Example 2: `hello.ag` — Everything is a prompt, even hello world**

```
// Even "hello world" is a prompt. There is no print("hello world").
// The LLM generates the greeting.
let greeting = prompt("Say hello to the world in a creative way", "world") -> string;
print(greeting);
```

**Example 3: `classify.ag` — Type-safe LLM output + validation**

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

**Example 4: `pipeline.ag` — Data pipeline (everything is prompt)**

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

Ships with `examples/data.txt`:

```
Contact us at sales@acme.com or support@acme.com.
For partnerships, reach out to partner@example.org.
Bug reports go to bugs@dev.example.org.
```

**Example 5: `parallel.ag` — Multi-agent orchestration**

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

**Example 6: `explore.ag` — Evolutionary branching**

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

The design rationale — short, direct, opinionated:
- Why no stdlib: the LLM is the standard library
- Why CB exists: evolutionary pressure on agent design
- Why validate: runtime contracts on non-deterministic outputs
- Why explore: survival of the fittest, not manual branching
- Why agents are pure: isolation enables fearless concurrency
- Why content-addressed: reproducibility without text files
- Comparison: "In Python you import pandas. In Agentis you prompt."
- Anti-patterns: when NOT to use Agentis (tight loops, deterministic parsing,
  sub-ms latency). This section is mandatory — honest scoping builds trust
  faster than marketing

## Implementation Order

1. **M15** (Runtime Trace) — foundation for debugging + demos
2. **M16** (Workflow + Diagnostics) — `agentis go`, better errors, config templates
3. **M17** (Examples) — needs M15+M16 to be demonstrable
4. **M18** (Docs) — written last, after examples prove the patterns work

## Success Criteria

Phase 4 is complete when:
1. `agentis go fast-demo.ag` produces visible output in under 8 seconds on
   a local model, with LLM wait indicator visible even in quiet mode
2. `agentis go example.ag --trace` shows a full agent run with visible LLM
   calls, validation, and CB tracking
3. `agentis doctor` validates the environment and reports problems clearly
4. Errors identify the DAG node and declaration path, not source line numbers
5. 6 example programs run with any LLM backend (CLI/local/HTTP)
6. A new user can read the docs and understand what Agentis is, why it exists,
   and how to write their first agent — in under 10 minutes
