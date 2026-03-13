# Phase 11 — Agent Memory & Introspection

**Codename:** The Remembering
**Version target:** v0.7.0
**Theme:** Agents that know themselves and learn from their ancestors.

---

## Motivation

Through Phase 10, Agentis agents are powerful but amnesiac. Each generation
starts from zero knowledge about what was tried before. The library stores
*winning code* but not *why it won* or *what failed*. An agent cannot inspect
its own CB budget at runtime or access its lineage history.

Phase 11 gives agents two capabilities that change everything:

1. **Memory** — persist and recall distilled knowledge across generations.
2. **Introspection** — query own runtime state (CB, lineage, generation).

Together, these let agents make informed decisions instead of blind ones.

---

## Milestones

- [ ] M44: Introspection Runtime Object
- [ ] M45: Lineage History (Failures + Successes)
- [ ] M46: Memo Store
- [ ] M47: Memo Garbage Collection & Size Limits
- [ ] M48: Introspection-Aware Evolution Strategy

### M44: Introspection Runtime Object

**Goal:** Expose a read-only `introspect` object available at runtime.

**Fields:**
```
introspect.cb_remaining    -> int       // CB left in current execution
introspect.cb_spent        -> int       // CB consumed so far
introspect.generation      -> int       // current generation number (0 = seed)
introspect.lineage_id      -> string    // hash of ancestor chain
introspect.arena_size      -> int       // agents in current arena round
```

**Implementation:**
- Inject `introspect` as a `Value::Struct("Introspect", fields)` into the
  evaluator environment before execution.
- **Dynamic fields:** `cb_remaining` and `cb_spent` must be read live from
  the evaluator's budget state, not from a static struct snapshot. Implement
  as a special case in `eval_field_access` (evaluator.rs) — when the object
  is `"Introspect"` and the field is `cb_remaining` or `cb_spent`, read
  directly from `self.budget` / `self.spent`.
- Static fields (`generation`, `lineage_id`, `arena_size`) are injected once
  from the evolution context before agent execution.
- Outside of `evolve`, generation = 0, lineage_id = "genesis", arena_size = 0.
- Read-only: agent cannot mutate introspect values (no reassignment in parser).

**Tests:**
- Agent reads `cb_remaining` and gets correct value after prompt calls.
- `cb_spent` increases after operations.
- Agent outside evolution sees generation = 0.
- Introspect fields are immutable (write attempt → error).

**CB cost:** 0 (introspection is free — it's reading own state, not prompting).

---

### M45: Lineage History (Failures + Successes)

**Goal:** Extend `introspect` with full ancestor history — both failures
and successes — plus a summary object for quick decision-making.

**Fields:**
```
introspect.ancestor_failures  -> list<AncestorRecord>   // cap: 10
introspect.ancestor_successes -> list<AncestorRecord>    // cap: 3

AncestorRecord {
    generation:     int,
    outcome:        string,    // "validation_failed" | "cb_exhausted" | "timeout" | "survived"
    fitness_score:  float,     // fitness at end of run
    code_hash:      string,    // SHA-256 of the AST
    elapsed_ms:     int,       // wall-clock time this ancestor ran
}

introspect.lineage_summary -> LineageSummary

LineageSummary {
    total_ancestors:    int,
    success_count:      int,
    failure_count:      int,
    avg_fitness:        float,    // mean fitness across lineage
}
```

**Note on `failure_reasons_histogram`:** The original plan included a
`map<string, int>` histogram. Deferred — requires iterating ancestor
records at injection time and building a map. Agents can compute this
themselves from `ancestor_failures` using `get()` and `len()`. If the
pattern proves common, promote to a built-in field later.

**Implementation:**
- Extend `LineageEntry` in `evolve.rs` with: `outcome: String`,
  `elapsed_ms: u64`, and `cb_spent: u64` fields. Currently only stores
  `score`, `parent_hash`, `generation`, `prompt_count`, `mutations`.
- Evolution loop: record failure reason when a variant fails (validation,
  CB exhaustion, timeout). Record "survived" for passing variants.
- Serialize ancestor records into lineage metadata alongside checkpoints.
- **Checkpoint version bump:** Increment `VERSION` in `checkpoint.rs`
  from 1 to 2. Add backward compatibility: `from_bytes()` must still
  read version 1 checkpoints (without ancestor data) gracefully.
- Cap failures at 10, successes at 3 (successes are rarer, fewer needed).
- Compute `LineageSummary` on injection (derived, not stored separately).
- Inject into Introspect struct before execution.

**Tests:**
- After 3 failed generations, 4th generation sees 3 failure records.
- Cap at 10 failures: 15 failures → only last 10 visible.
- Successful ancestor appears in ancestor_successes.
- lineage_summary.avg_fitness matches manual calculation.
- Outside evolution: all lists empty, summary zeroed.
- Version 1 checkpoint loads without ancestor data (backward compat).

**CB cost:** 0.

---

### M46: Memo Store

**Goal:** Introduce `memo_write`, `recall`, and `recall_latest` as built-in
functions for cross-generation knowledge persistence.

**Design decision:** Implement as built-in functions in `eval_call()`
(evaluator.rs), NOT as new AST nodes. This avoids changes to the parser,
lexer, and AST binary serialization. Same pattern as `file_read`,
`file_write`, `push`, `get`.

**Syntax:**
```
// Write a memo entry
memo_write("classifier-strategy", "few-shot outperforms zero-shot for categories > 5");

// Read all entries for a key (newest first)
let all_hints = recall("classifier-strategy");

// Read only the most recent entry (or ""/null if missing)
let latest = recall_latest("classifier-strategy");
```

**Semantics:**
- `memo_write(key, value)` appends an entry to `.agentis/memo/{key}.jsonl`.
- Each entry is a JSON line: `{"generation":N,"value":"...","timestamp":T}`.
- `recall(key)` returns a `list<string>` of all values, newest first.
- `recall_latest(key)` returns the most recent value as a string, or `""`.
- Memo entries survive across generations within the same evolution run.
- Memo entries are NOT automatically shared across different evolve runs
  (but can be via library export, Phase 10 M43 — future extension).

**Implementation:**
- Add `"memo_write"`, `"recall"`, `"recall_latest"` arms in `eval_call()`.
- Storage: `.agentis/memo/` directory, one `.jsonl` file per key.
  Key is sanitized (alphanumeric + hyphens only) to avoid path traversal.
- Atomic writes: write to tmp file + rename (same pattern as library.rs).
- Memo write costs 1 CB per call.
- Memo recall costs 0 CB (reading is free).
- **Per-generation write limit:** Max 20 memo writes per key per generation.
  Beyond 20: writes are silently dropped and a warning is logged to trace.
- **Large entry guard:** Individual values capped at 10 KB. Values
  exceeding this are truncated with a `[truncated]` suffix.

**Tests:**
- Write memo in generation 1, recall in generation 2 → entries present.
- Recall nonexistent key → empty list.
- `recall_latest` on empty key → empty string.
- Memo entries accumulate (gen 1 writes 2, gen 3 writes 1 → 3 total).
- CB correctly deducted on write (1 per call).
- 21st write in same generation/key → silently dropped, warning in trace.
- Two parallel workers writing to same key → no corruption (atomic write).
- Entry >10 KB → truncated with suffix.
- Key with special characters → rejected with error.

---

### M47: Memo Garbage Collection & Size Limits

**Goal:** Prevent memo from growing unbounded.

**Rules:**
- Max 100 entries per key.
- Max 50 keys per evolution run.
- Total memo store max 10 MB (configurable via `--memo-max-size`).
- When limit hit: oldest entries evicted first (FIFO).
- `agentis memo list` — show all keys and entry counts.
- `agentis memo clear [key]` — manual cleanup.
- `agentis memo stats` — show total size, key count, largest keys.

**Implementation:**
- Size check on every `memo_write` call.
- FIFO eviction within key.
- CLI subcommand `memo` added to match/dispatch in `main.rs`
  (same pattern as `lib`, `colony`, `snapshot` subcommands).
- Config option in `.agentis/config`: `memo.max_size = 10MB`.
- Override via CLI: `agentis evolve solver --memo-max-size 50MB`.

**Tests:**
- Write 101 entries to one key → oldest evicted, 100 remain.
- Exceed 50 keys → error with message.
- `agentis memo list` shows correct output.
- `agentis memo clear` removes entries.
- `agentis memo stats` shows correct size and key count.
- Custom `--memo-max-size 1MB` triggers eviction at lower threshold.
- Config file `memo.max_size` is respected when CLI flag absent.

---

### M48: Introspection-Aware Evolution Strategy

**Goal:** Demonstrate the power of introspection + memo combined.
Ship a new example that uses both features in a real evolution run.

**Example: `examples/self-improving-classifier.ag`**

An agent that:
1. Reads `introspect.ancestor_failures` to see what went wrong before.
2. Checks `introspect.lineage_summary` for overall trajectory.
3. Recalls memo entries about which prompt strategies worked.
4. Makes budget-aware decisions based on `introspect.cb_remaining`.
5. Writes new memo entries about what it discovered.

**Parser limitations respected:**
- No `let x = if ... { } else { };` (if/else is not an expression).
- No reassignment (`x = new_value` not supported).
- No closures/lambdas (no `.any(f => ...)` syntax).
- No string interpolation (no `"{variable}"` — use `+` concatenation).
- No `else if` shorthand (use `else { if ... { } }` or early-return pattern).

```
type Category {
    label: string,
    confidence: float
}

// Strategy selection via early-return pattern (no if-else expression)
fn select_strategy(
    cb_left: int,
    fail_count: int,
    total: int,
    avg_fitness: float,
    hints: string
) -> string {
    // Low budget: zero-shot, no frills
    if cb_left < 80 {
        return "Classify this text into a category. Be brief.";
    }

    // Catastrophic trajectory: >70% ancestors failed — start fresh
    if total > 0 {
        if fail_count * 10 > total * 7 {
            return "Ignore all previous approaches. Classify this text using the simplest possible method.";
        }
    }

    // Strong lineage: try ambitious approach
    if avg_fitness > 0.8 {
        return "Classify with detailed reasoning and sub-categories.";
    }

    // Default with hints from memo
    if len(hints) > 0 {
        return "Classify this text. Strategy hints: " + hints;
    }

    return "Classify this text into a category.";
}

agent classifier(text: string) -> Category {
    cb 200;

    // Read introspection data
    let cb_left = introspect.cb_remaining;
    let summary = introspect.lineage_summary;
    let fail_count = summary.failure_count;
    let total = summary.total_ancestors;
    let avg_fitness = summary.avg_fitness;

    // Recall memo from previous generations
    let hints = recall_latest("classifier-strategy");

    // Select strategy based on introspection
    let instruction = select_strategy(cb_left, fail_count, total, avg_fitness, hints);

    let result = prompt(instruction, text) -> Category;

    validate result {
        result.confidence > 0.6
    };

    // Record what worked for future generations
    let gen = introspect.generation;
    memo_write("classifier-strategy",
        "gen=" + to_string(gen) + " conf=" + to_string(result.confidence)
    );

    return result;
}

explore "positive-review" {
    let r = classifier("This product is amazing, best purchase ever!");
    validate r { r.label == "positive" };
}

explore "negative-review" {
    let r = classifier("Terrible quality, broke after one day.");
    validate r { r.label == "negative" };
}
```

**What this demonstrates:**
- **Introspection:** Agent reads its own CB, generation, and lineage summary.
- **Memory:** Agent reads and writes memo across generations.
- **Budget-awareness:** Strategy degrades gracefully when CB is low.
- **Lineage-awareness:** Strategy adapts based on ancestor failure ratio.
- **No unsupported syntax:** Uses early-return pattern, string concatenation,
  built-in function calls only.

**Tests:**
- Run evolve with 5 generations. Verify memo accumulates entries.
- Verify later generations reference ancestor data.
- Force low-CB scenario → verify agent takes zero-shot path.
- Compare fitness trajectory with vs without memo (expect faster convergence).

---

## File changes (estimated)

```
src/evaluator.rs    — introspect injection, dynamic cb fields,
                      memo_write/recall/recall_latest built-ins,
                      per-gen write limits, entry truncation,
                      atomic writes                              +350 lines
src/evolve.rs       — failure+success recording in LineageEntry,
                      ancestor history collection, summary       +150 lines
src/checkpoint.rs   — version bump 1→2, ancestor serialization,
                      backward compat for v1 checkpoints          +80 lines
src/main.rs         — agentis memo subcommand (list/clear/stats),
                      --memo-max-size flag, introspect injection
                      in cmd_go / cmd_evolve                     +120 lines
examples/           — self-improving-classifier.ag                +60 lines
docs/phase11-plan.md — this document                               —
```

**Total estimate:** ~760 lines of Rust + example.

---

## Non-goals for Phase 11

- **Agent-to-agent messaging** — needs colony work first (future phase).
- **Delegation with contracts** — depends on messaging (future phase).
- **External tool integration (MCP)** — orthogonal, separate phase.
- **Adaptive fitness** — can layer on top of introspection later.
- **Package registry** — ecosystem concern, not language concern.
- **New AST nodes** — memo/recall implemented as built-in functions, not
  new parser/AST constructs. Keeps scope minimal.
- **`else if` syntax** — parser currently requires `else { if ... { } }`.
  Desirable but out of scope. Tracked as parser enhancement for future phase.
- **String interpolation** — would benefit memo writes. Deferred.
- **Closures/lambdas** — would enable `.any()` / `.filter()` patterns.
  Significant parser+evaluator work. Deferred.
- **`failure_reasons_histogram`** — agents can compute from ancestor list.
  Promote to built-in if pattern proves common.

---

## Sequencing

```
M44 (introspect) → M45 (lineage history) → M46 (memo store) → M47 (memo GC) → M48 (example)
     |                    |                       |                   |               |
   1-2 hrs             2-3 hrs                 3-4 hrs            1-2 hrs          1-2 hrs
```

All milestones are sequential. M45 extends M44. M46 is independent of M45
but benefits from it. M47 hardens M46. M48 ties everything together.

---

## Success Criteria

Phase 11 is done when:

1. An agent can read its own CB state (live, not snapshot), generation number, and arena size at runtime.
2. An agent can see why its ancestors failed *and* what its successful ancestors looked like.
3. An agent can access `lineage_summary` for aggregate stats (avg fitness, counts).
4. An agent can write and read memo entries that survive across generations.
5. `recall_latest` and `recall` work correctly.
6. Memo store has configurable size limits and GC (FIFO eviction).
7. A shipped example demonstrates budget-aware, lineage-aware self-improving behavior using only supported syntax.
8. `agentis memo list`, `agentis memo clear`, and `agentis memo stats` work.
9. Checkpoint format v2 stores ancestor history with backward compat for v1.
10. All existing tests still pass.
