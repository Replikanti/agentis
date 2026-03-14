# Agentis — Feature Ideas Backlog

Raw idea pool for future phases. Each idea is independent.
Priority and grouping into phases happens separately.

---

## 1. Agent Introspection

**What:** A language-level primitive that lets an agent query its own state —
CB remaining, fitness trajectory, generation number, ancestor failure reasons.

**Why:** Right now agents are blind to their own history within an evolution run.
An agent that *knows* it has 30 CB left and that three ancestors failed on
the same validation can make a fundamentally different decision than one
that doesn't.

**Sketch:**
```
let budget = introspect.cb_remaining;
let history = introspect.lineage_failures;
if history.contains("timeout") {
    // switch strategy
}
```

**Depends on:** Library (Phase 10) already stores population data.
Introspection exposes it at runtime.

---

## 2. Composite Memory (Cross-Generation Knowledge)

**What:** A `memo` block that persists distilled knowledge across generations
within an evolution run. Not full state — compressed learnings.

**Why:** Evolution currently discards everything except the winning code.
Meta-knowledge like "classification prompts work better with few-shot
examples" is lost every generation. Memo lets the lineage accumulate
wisdom.

**Sketch:**
```
memo "classification-strategy" {
    "few-shot outperforms zero-shot for categories > 5";
    "temperature 0.3 reduces hallucination on structured output";
}

agent classifier(text: string) -> Category {
    let hints = recall("classification-strategy");
    let result = prompt("Classify: {hints}", text) -> Category;
    return result;
}
```

**Depends on:** Library export/import (Phase 10 M43).

---

## 3. Delegated Execution with Contracts

**What:** `delegate(agent, task, contract)` — one agent assigns work to another
with explicit expectations: output schema, CB cap, minimum fitness, timeout.

**Why:** Current agents are monolithic. Real-world problems decompose into
sub-tasks. Delegation lets an orchestrator agent spawn specialist agents
without managing their internals — only the contract matters.

**Sketch:**
```
let result = delegate(summarizer, article, {
    schema: Summary,
    cb_max: 50,
    fitness_min: 0.7,
    timeout: 30s
});
```

**Depends on:** Colony model (Phase 8) for distributed execution.

---

## 4. Negation / Anti-Constraints

**What:** `avoid("pattern", context)` — a post-hoc semantic check that rejects
output matching an anti-pattern. Different from `validate` which checks
structure; `avoid` checks meaning.

**Why:** Validation catches "wrong shape". Avoid catches "wrong content".
An agent generating a product description should `avoid("competitor names")`
or `avoid("unsubstantiated health claims")`. This is a guardrail primitive,
not a filter.

**Sketch:**
```
agent writer(brief: string) -> Article {
    let draft = prompt("Write article", brief) -> string;
    avoid draft {
        "hallucinated statistics",
        "competitor brand names",
        "first person voice"
    };
    return draft;
}
```

**Depends on:** Prompt infrastructure (already exists). Avoid is essentially
a typed negative-validation prompt call.

---

## 5. Agent-to-Agent Messaging

**What:** Named channels for agents to communicate within a colony. Not
delegation (structured contract) — messaging is fire-and-forget signals.

**Why:** Colony workers currently run in isolation. Sometimes an agent
discovers something useful for another agent's task — a channel lets it
broadcast without blocking.

**Sketch:**
```
channel findings: string;

agent scout(domain: string) -> Report {
    let intel = prompt("Research {domain}") -> string;
    emit findings <- intel;
    return Report { domain, intel };
}

agent analyst() -> Summary {
    let data = listen findings timeout 10s;
    return prompt("Summarize findings", data) -> Summary;
}
```

**Depends on:** Colony model (Phase 8).

---

## 6. Composable Agent Pipelines

**What:** Pipe operator for chaining agents: `input |> agent_a |> agent_b`.
Output of one agent is input to the next.

**Why:** Reduces boilerplate for sequential processing. Instead of manually
wiring outputs to inputs, the language handles it. Encourages building
small, focused agents.

**Sketch:**
```
let result = raw_text
    |> cleaner
    |> classifier
    |> summarizer;
```

**Depends on:** Type system for input/output matching between agents.

---

## 7. Adaptive Fitness Functions

**What:** Fitness functions that evolve alongside agents. Instead of static
`validate { score > 70 }`, the fitness threshold adjusts based on
population performance.

**Why:** Static thresholds are arbitrary. If the whole population scores 90+,
a threshold of 70 isn't selecting for anything. Adaptive fitness raises
the bar as the population improves.

**Sketch:**
```
evolve "solver" adaptive {
    // threshold = population_mean + 1 * stddev
    // automatically tightens each generation
    fitness adaptive(mean + 1σ);
}
```

**Depends on:** Arena (Phase 7), fitness metrics.

---

## 8. Budget Prediction ✅ (Phase 13, v0.9.0)

**What:** Before executing a prompt, estimate its CB cost based on input
size, prompt complexity, and historical data from similar calls.

**Why:** Agents currently spend CB and discover they're broke mid-execution.
Prediction lets an agent decide *before* a prompt call whether it can
afford it, and choose a cheaper strategy if not.

**Sketch:**
```
let cost = estimate_cb(prompt_text, input_size);
if introspect.cb_remaining < cost * 1.5 {
    // use cheaper approach
}
```

**Depends on:** Introspection (idea #1), CB system (already exists).

---

## 9. Agent Package Registry

**What:** `agentis publish` / `agentis install` — share agents as
content-addressed modules. Import by hash or by name@version.

**Why:** The content-addressed DAG already makes agents portable. A registry
turns that into an ecosystem. Someone builds a great classifier agent —
others import it by hash.

**Sketch:**
```
agentis publish classifier --tag v1
agentis install @replikanti/classifier@v1

// in code:
import classifier from "sha256:abc123...";
```

**Depends on:** VCS model (already exists), content-addressed storage.

---

## 10. MCP / Tool Use Integration

**What:** First-class support for calling external tools via Model Context
Protocol or similar. Not sandboxed file I/O — structured tool invocation.

**Why:** Agents that can only prompt are limited. An agent that can also
call a database, hit an API, or use a calculator becomes drastically
more capable. MCP is becoming the standard for this.

**Sketch:**
```
tool database = mcp("postgres://...");
tool search = mcp("https://search.api/mcp");

agent researcher(topic: string) -> Report {
    let results = search.query(topic);
    let stored = database.insert(results);
    return prompt("Synthesize", results) -> Report;
}
```

**Depends on:** Sandboxed I/O (already exists), network whitelisting.

---

## 11. Structured Evolution Log (Agent-Readable)

**What:** A tracing format designed for agents, not humans. Structured
JSON/DAG log of every generation: what was tried, what failed, why,
what mutated into what.

**Why:** Current `--trace` is human-readable. An agent doing meta-evolution
(evolving its own evolution strategy) needs machine-readable history.

**Sketch:**
```
agentis evolve solver --trace-format dag

// produces .agentis/traces/solver-run-001.json
// {generations: [{id, parent, code_hash, fitness, cb_spent, failure_reason}]}
```

**Depends on:** Arena (Phase 7), colony observability (Phase 8 M34).

---

## 12. Confidence Primitive ✅ (Phase 13, v0.9.0)

**What:** `confidence(claim, context) -> float` as a built-in that's more
than just prompting "how sure are you". Uses calibrated techniques:
sampling multiple responses, measuring agreement, checking against
known facts in memo.

**Why:** Self-reported LLM confidence is unreliable. A calibrated
confidence primitive that uses ensemble sampling gives agents a real
signal for decision-making.

**Sketch:**
```
let answer = prompt("What is the capital of X?", data) -> string;
let conf = confidence(answer, data, samples: 5);
if conf < 0.8 {
    let answer = prompt("Re-examine carefully", data) -> string;
}
```

**Depends on:** Prompt infrastructure, CB system (multiple samples cost CB).

---

## 13. Interop: Execute Foreign Code

**What:** `exec` block that runs Python/JS/shell inside the sandbox,
returning typed results to Agentis.

**Why:** Some things shouldn't be prompted — math, data parsing, file
transformations. Instead of asking an LLM to compute a hash, just
compute it. Hybrid execution: LLM for reasoning, code for computation.

**Sketch:**
```
let hash = exec python {
    import hashlib
    result = hashlib.sha256(b"hello").hexdigest()
};
// hash is now a string in Agentis
```

**Depends on:** Sandbox (already exists).

---

## 14. Warm-Start from External Context

**What:** Feed an evolution run with external context — a document, a
codebase, a dataset — that all agents in the population can reference.

**Why:** Currently agents start with only their prompt and code. Real tasks
need grounding in external data. Warm-start injects that context into
the evolution environment.

**Sketch:**
```
agentis evolve solver --context ./dataset.csv --context ./spec.md
```

**Depends on:** Smart seeding (Phase 10 M40).

---

## 15. Multi-Model Arena

**What:** Evolve the same agent across different LLM backends simultaneously.
Claude, Ollama, Gemini — let them compete in the same arena.

**Why:** Different models have different strengths. A multi-model arena
discovers which model is best for which task, and the winning lineage
carries its model preference.

**Sketch:**
```
agentis evolve solver --backends claude,ollama,gemini
// arena tracks model alongside fitness
```

**Depends on:** Multi-backend support (already exists), arena (Phase 7).

---

## Review Notes & Prioritization Guidance

**Implemented in Phase 11 (v0.7.0):** Ideas #1 (Introspection) and #2 (Composite Memory).
**Implemented in Phase 12 (v0.8.0):** Portable Agent Identity (#9 partially — content-addressed bundles with identity hashes, export/import).
**Implemented in Phase 13 (v0.9.0):** Ideas #8 (Budget Prediction) and #12 (Confidence Primitive).

**Recommended next priorities (post Phase 13):**
1. Idea #3 — Delegated Execution with Contracts (multi-agent decomposition)
2. Idea #6 — Composable Agent Pipelines (pipe operator chaining)
3. Idea #10 — MCP / Tool Use Integration (external tool invocation)

**Defer until later:**
- Idea #5 — Agent-to-Agent Messaging: risk of uncontrolled side-channels.
  Implement after Contracts + typed channels are proven.
- Idea #4 — Negation/Anti-Constraints: expensive (LLM call per avoid).
  Better as a library pattern first, promote to primitive if adoption is high.
- Idea #13 — Foreign Code Interop: security/sandboxing complexity.
  Needs capability-based sandbox first.
- Idea #15 — Multi-Model Arena: high engineering cost, lower urgency.

**Future extensions tracked (not yet separate ideas):**
- `recall("key", filter: ...)` and `recall("key", sort: "fitness_desc")` — advanced recall
- Typed memo (`memo Strategy "key" { field: type = value }`)
- Memo author/lineage_id tagging for cross-run sharing
- `introspect.ancestor_successes` histogram (parallel to failure histogram)
