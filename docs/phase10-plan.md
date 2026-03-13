# Phase 10: Collaborative Evolution & Budget Intelligence

## Vision

Phases 7–9 built a complete evolution pipeline — local, distributed,
checkpointed. But each `evolve` run starts from scratch. Good variants
discovered yesterday are forgotten today. Cognitive budget is spread
equally across promising lineages and dead ends. And when an LLM rate-
limit or a validation failure hits mid-run, the entire population can
degrade with no automatic recovery.

Phase 10 makes evolution **collaborative and intelligent**:

1. A persistent **library** of elite variants that survives across runs.
2. **Warm-start seeding** from the library so new runs begin where old
   runs left off.
3. **Per-lineage adaptive budget allocation** that starves dead ends and
   feeds winners.
4. **Event hooks** that react to stagnation, crashes, and breakthroughs
   without changing the language.
5. **Portable export/import** of library bundles for cross-machine and
   cross-team sharing.

No new builtins. No new language syntax. No new crate dependencies.
Phase 10 is a CLI-level intelligence layer on top of the existing
evolution engine.

## Design Principle: Memory Across Runs

Phases 7–9 give the evolution loop memory *within* a run (checkpoints,
lineage JSONL). Phase 10 adds memory *across* runs. The library is
the long-term memory; warm-start is the recall mechanism; adaptive
budget is the attention mechanism; hooks are the reflexes.

## Persistent Population Library

Elite variants live in `.agentis/library/`, a content-addressed store
independent from `.agentis/objects/` and `.agentis/colony/`.

```
.agentis/library/
  objects/          # content-addressed LibraryEntry blobs
    ab/
      cd1234...     # SHA-256 hash of serialized LibraryEntry
  index             # newline-delimited list of all entry hashes
  tags/
    email-parser    # file containing hash → named entry
    best-classifier # file containing hash → named entry
```

**Index** is a flat list of all entry hashes (one per line). Simple to
scan, append-only in practice. Rebuilt from `objects/` on corruption.

**Tags** work identically to colony checkpoint tags — a file containing
one hash.

## LibraryEntry

The fundamental data structure. One per stored elite variant.

```rust
pub struct LibraryEntry {
    // Identity
    pub source: String,               // full .ag source code
    pub source_hash: String,          // SHA-256 of source

    // Provenance
    pub seed_hash: String,            // hash of the original seed
    pub generation: u32,              // generation where this variant was produced
    pub evolution_run: Option<String>, // checkpoint hash of the run that produced it

    // Fitness
    pub fitness_score: f64,
    pub cb_efficiency: f64,
    pub validate_rate: f64,
    pub explore_rate: f64,
    pub prompt_count: u32,

    // Metadata
    pub description: String,          // LLM-generated 1–2 sentence summary
    pub tags: Vec<String>,            // user-assigned tags
    pub timestamp: u64,               // Unix millis when added to library
}
```

**Why store full source?** Library entries must be self-contained.
Unlike checkpoints (which store populations for resume), library entries
are individual variants that need to be immediately usable as seeds.
Typical entry size: 2–10 KB.

**Why an LLM-generated description?** `lib search` needs something to
match against. Source code alone is searchable but not human-friendly.
The description is generated once at `lib add` time using the configured
LLM backend (or a default template with mock backend). Cost: one prompt
call per entry — negligible compared to evolution runs. Users who want
manual control can use `--description "..."` to set it directly,
`--desc-from-file <path>` to read from a file, or `--no-desc` to store
an empty description (useful for batch imports or when no LLM backend
is configured).

## Binary Serialization

Same approach as checkpoints — hand-rolled, no serde.

```
Header:
  [4 bytes magic: "AGlb"]            # Agentis Library
  [u8 version: 1]

Identity:
  [u32 source_len][source bytes]
  [u32 source_hash_len][source_hash bytes]

Provenance:
  [u32 seed_hash_len][seed_hash bytes]
  [u32 generation]
  [u8 has_run][u32 run_hash_len][run_hash bytes if has_run]

Fitness:
  [f64 fitness_score]
  [f64 cb_efficiency]
  [f64 validate_rate]
  [f64 explore_rate]
  [u32 prompt_count]

Metadata:
  [u32 description_len][description bytes]
  [u32 tag_count]
    per tag:
      [u32 tag_len][tag bytes]
  [u64 timestamp_ms]
```

All multi-byte integers are little-endian. All f64 values are IEEE 754 LE
via `f64::to_le_bytes()`/`from_le_bytes()`. Strings are UTF-8.

## Warm-Start Seeding

When `--seed-from-lib` is specified, the evolution loop uses library
entries alongside (or instead of) the seed file to build the initial
population.

**Selection strategies:**

- **Tag match:** `--seed-from-lib "tag:email-parser"` — select entries
  with matching tag.
- **Top-K:** `--seed-top-k 5` — take the 5 highest-scoring entries from
  the matched set.
- **Warm-start probability:** `--warm-start-prob 0.4` — with 40%
  probability, mutate from a library entry instead of from the surviving
  population. Applied per variant during each generation.
- **Probability decay:** `--warm-start-decay <end>` — linearly decay
  warm-start probability from the initial value to `<end>` over the
  course of the run. For example, `--warm-start-prob 0.7 --warm-start-decay 0.1`
  starts at 70% library injection and decays to 10% by the final
  generation. Rationale: early generations benefit most from library
  memory; later generations should rely on the evolved population.

**How warm-start interacts with the evolution loop:**

```
for each generation:
    for each variant slot in population:
        if random() < warm_start_prob AND library has matching entries:
            parent = random choice from top-K library entries
        else:
            parent = tournament-selected from surviving population
        variant = mutate(parent)
```

This is a soft injection. The library provides a "suggestion pool" that
competes with the current population. If library entries are outdated or
irrelevant, natural selection filters them out within a few generations.

**Provenance tracking:** Each variant in the generation JSONL lineage
records its provenance type: `"seed-file"` (from the original seed),
`"population"` (tournament-selected from surviving population), or
`"library"` (warm-start injected from library, with the library entry
hash). This makes it easy to see how much of the final population
originated from library vs. organic evolution:

```
Gen  5: best=0.850  avg=0.720  prompts=2.1  (8 variants: 5 population, 3 library)
```

## Per-Lineage Budget Allocation

Each lineage (traced from its seed) gets a dynamic budget fraction
instead of equal sharing.

**Tracking:**

```rust
pub struct LineageBudget {
    pub seed_hash: String,
    pub allocated_fraction: f64,    // current share of total CB [0.0, 1.0]
    pub cumulative_cb: u64,         // CB spent so far on this lineage
    pub recent_scores: Vec<f64>,    // last N generation best scores
    pub stall_count: u32,           // consecutive gens without improvement
}
```

**Allocation rules:**

1. **Initial:** Equal allocation across all active lineages.
2. **Growing lineage** (Δscore > 0.01 over last 5 gens): increase
   allocation by 50%, up to `max_lineage_fraction` (default 0.5).
3. **Stalled lineage** (Δscore < 0.01 for 5 gens): reduce allocation
   to 1/3 of current.
4. **Dead lineage** (stall_count > hard cap): terminate entirely,
   redistribute budget to growing lineages.
5. **Normalization:** After each reallocation, normalize all fractions
   to sum to 1.0.

**CLI:**

```bash
agentis evolve seed.ag -g 20 -n 8 --adaptive-budget
agentis evolve seed.ag -g 20 -n 8 --adaptive-budget --max-lineage-fraction 0.6
agentis evolve seed.ag -g 20 -n 8 --adaptive-budget --lineage-stall-window 3
```

**How budget fractions translate to variant slots:**

Population size N is fixed. Budget fractions determine how many of the
N variant slots each lineage gets per generation. With 8 slots and two
lineages at 0.75/0.25: lineage A gets 6 slots, lineage B gets 2.
Rounding: floor + distribute remainders to highest-fraction lineages.

**Note:** In the current single-seed model, there is one lineage per
run. Per-lineage budget becomes meaningful when combined with
warm-start (multiple seeds from library) or when lineages diverge
significantly. The tracking infrastructure is built regardless — it's
lightweight and useful for observability even with one lineage.

## Event Hooks

Config-level hooks that fire in response to evolution events. Defined
in `.agentis/config`, not in the language.

**Supported events and actions:**

| Event | Trigger | Available actions |
|-------|---------|-------------------|
| `on_stagnation` | stall_count reaches threshold | `reduce_budget <fraction>`, `inject_library <count>`, `log <message>` |
| `on_new_best` | new best_ever_score found | `checkpoint`, `tag <name>`, `lib_add`, `log <message>` |
| `on_validation_fail` | validate block fails during evolution | `retry <count> temp+<delta>`, `switch_model <name>`, `log <message>` |
| `on_crash` | variant evaluation crashes (panic, timeout) | `checkpoint`, `log <message>`, `skip` |

**Config syntax:**

```
hooks.on_stagnation = reduce_budget 0.3
hooks.on_new_best = checkpoint tag=improved
hooks.on_validation_fail = retry 2 temp+0.2
hooks.on_crash = checkpoint
```

**Semantics:**

- `reduce_budget <fraction>`: multiply the stalled lineage's budget
  fraction by `<fraction>`.
- `inject_library <count>`: replace `<count>` weakest variants in the
  population with top library entries.
- `checkpoint`: store a checkpoint immediately.
- `tag <name>`: tag the most recent checkpoint.
- `lib_add`: add the current best variant to the library.
- `retry <count> temp+<delta>`: re-evaluate the failing variant up to
  `<count>` times, increasing LLM temperature by `<delta>` each retry.
- `switch_model <name>`: re-evaluate the failing variant with a
  different LLM model (by temporarily overriding `llm.model` in the
  backend config). Useful when rate-limits or hallucinations on one
  model can be resolved by falling back to another (e.g. Sonnet → Haiku).
  The switch applies only to the retry — subsequent variants use the
  original model.
- `log <message>`: append `<message>` to stderr.
- `skip`: silently skip the failing variant (score = 0.0).

**Implementation:** Hooks are parsed at `cmd_evolve` startup. Each hook
is a `(Event, Vec<Action>)` pair. The evolution loop checks for events
at defined points and executes matching actions. No shell execution,
no arbitrary commands — only the predefined action vocabulary above.

## Portable Library Export/Import

**Export:**

```bash
agentis lib export --tag "email-v3" --out bundle.alib
agentis lib export --top 5 --out best.alib
agentis lib export --all --out full-library.alib
```

**Bundle format (`.alib`):**

```
Header:
  [4 bytes magic: "ALIb"]           # Agentis Library Bundle
  [u8 version: 1]
  [u32 entry_count]

Entries:
  per entry:
    [u32 blob_len][serialized LibraryEntry bytes]
```

Simple concatenation of serialized entries with length prefixes.
No compression in v1 (entries are typically 2–10 KB; a 100-entry bundle
is ~500 KB uncompressed — acceptable). The version byte allows adding
optional compression in a future format revision if bundle sizes become
a concern. The bundle is self-contained — no references to external
objects.

**Import:**

```bash
agentis lib import bundle.alib
agentis lib import bundle.alib --skip-duplicates
```

Import reads each entry, computes its hash, stores it in the library.
`--skip-duplicates` (default: on) skips entries whose source_hash
already exists in the library.

## Milestones

- [ ] M39: Persistent Population Library
- [ ] M40: Smart Seeding & Warm-start
- [ ] M41: Per-lineage Budget Caps + Adaptive Allocation
- [ ] M42: Event Hooks
- [ ] M43: Portable Library Export / Import

### M39: Persistent Population Library

Deliverable: new `src/library.rs`, changes to `main.rs`

Foundation: content-addressed library storage with serialization and CLI.

**New `src/library.rs`:**

```rust
pub struct LibraryStore { ... }

impl LibraryStore {
    pub fn new(agentis_root: &Path) -> Self;       // .agentis/library/
    pub fn store(entry: &LibraryEntry) -> Result<String, ...>;
    pub fn load(hash: &str) -> Result<LibraryEntry, ...>;
    pub fn list() -> Result<Vec<String>, ...>;      // all hashes from index
    pub fn set_tag(name: &str, hash: &str) -> Result<(), ...>;
    pub fn resolve_tag(name: &str) -> Result<Option<String>, ...>;
    pub fn list_tags() -> Result<Vec<(String, String)>, ...>;
    pub fn remove(hash: &str) -> Result<(), ...>;
    pub fn search(query: &str) -> Result<Vec<(String, LibraryEntry)>, ...>;
    pub fn exists(hash: &str) -> bool;
    pub fn rebuild_index() -> Result<usize, ...>;   // scan objects/, rebuild index
}
```

- SHA-256 hashing via existing `sha2` crate
- Fan-out storage: `.agentis/library/objects/{prefix:2}/{rest}`
- Binary encode/decode for `LibraryEntry`
- Index file: `.agentis/library/index` (one hash per line)
- Tags: `.agentis/library/tags/{name}` (single line, entry hash)
- `search`: substring match on description and tags, plus simple fuzzy
  match on tags (Levenshtein distance ≤ 2) to tolerate typos

**CLI:**

```bash
agentis lib add <file.ag> [--tag <name>] [--description "..."] [--desc-from-file <path>] [--no-desc]
agentis lib list [--tag <name>]
agentis lib show <hash-or-tag>
agentis lib search "keyword"
agentis lib remove <hash-or-tag>
agentis lib tags
agentis lib tag <hash> <name>
```

**`lib add` behavior:**

1. Read source file, compute hash.
2. Resolve description:
   - `--description "..."` — use provided text directly.
   - `--desc-from-file <path>` — read description from file.
   - `--no-desc` — store empty description (no LLM call).
   - (default) — generate via LLM backend:
     prompt = "Summarize what this Agentis program does in 1–2 sentences."
     Falls back to "(no description)" with mock backend.
3. Evaluate source to get fitness metrics (single run, quiet).
4. Build `LibraryEntry`, store, update index.
5. Apply tag if `--tag` specified.

**`lib list` output:**

```
Library: 12 entries

HASH          SCORE   TAGS              DESCRIPTION
ab3f1234...   0.935   email-parser      Classifies emails by urgency using...
cd910987...   0.890   email-parser      Validates email headers with struct...
ef121234...   0.815                     Simple sentiment classifier for...
```

**Auto-add from evolve:** When an evolution run completes (or hits early
stop), the best-ever variant is automatically added to the library if
its score exceeds `lib.min_auto_score` (default 0.8, configurable).
This is opt-out via `--no-lib-add`. For very long runs,
`--lib-add-interval N` adds the current best to the library every N
generations (if it exceeds the threshold), capturing good intermediate
results that might be lost if the run later degrades.

**Tests:** encode/decode roundtrip, store/load, index rebuild, tag CRUD,
search matching, duplicate detection.

### M40: Smart Seeding & Warm-start

Deliverable: changes to `main.rs` (cmd_evolve), `library.rs`

Integrate library-based seeding into the evolution loop.

**CLI flags:**

```bash
agentis evolve seed.ag -g 10 -n 8 --seed-from-lib "tag:email-parser"
agentis evolve seed.ag -g 10 -n 8 --seed-from-lib "tag:email-parser" --seed-top-k 5
agentis evolve seed.ag -g 10 -n 8 --warm-start-prob 0.4
agentis evolve seed.ag -g 20 -n 8 --warm-start-prob 0.7 --warm-start-decay 0.1
```

**Implementation:**

```rust
// In cmd_evolve, before the loop:
let library_seeds: Vec<(String, String)> = if let Some(ref lib_query) = seed_from_lib {
    let lib = LibraryStore::new(&root);
    let matched = lib.search_by_tag_or_query(lib_query)?;
    let top_k = seed_top_k.unwrap_or(3);
    matched.into_iter()
        .take(top_k)
        .map(|(_, entry)| (entry.source, entry.source_hash))
        .collect()
} else {
    vec![]
};

// Merge with seed: library entries + seed file
let initial_parents = if library_seeds.is_empty() {
    vec![(seed_source.clone(), seed_hash.clone())]
} else {
    let mut p = library_seeds;
    p.push((seed_source.clone(), seed_hash.clone()));
    p
};
```

**Warm-start in loop:**

```rust
// Compute effective warm-start probability for this generation:
let effective_prob = if let Some(decay_end) = warm_start_decay {
    let progress = (g - start_gen) as f64 / (end_gen - start_gen).max(1) as f64;
    warm_start_prob + (decay_end - warm_start_prob) * progress
} else {
    warm_start_prob
};

// In variant generation, for each slot:
let parent = if effective_prob > 0.0
    && !library_seeds.is_empty()
    && simple_rng() < effective_prob
{
    &library_seeds[simple_rng_index() % library_seeds.len()]
} else {
    &parents[tournament_select()]
};
```

**RNG:** Simple deterministic PRNG seeded from system time at run start
(no `rand` crate). Used only for warm-start selection — not security-
critical.

**Edge cases:**
- `--seed-from-lib` with empty library: warn, proceed with seed file
  only.
- `--seed-from-lib` with no matching tag: error with suggestion to
  `agentis lib tags`.
- `--warm-start-prob 0.0`: same as not specifying (no warm-start).
- `--warm-start-prob 1.0`: every variant mutated from library (no
  population continuity — probably undesirable, but allowed).

**Tests:** warm-start selection probability, library seed integration,
empty library fallback, initial population merging.

### M41: Per-lineage Budget Caps + Adaptive Allocation

Deliverable: new types in `evolve.rs`, changes to `main.rs`

Dynamic budget allocation across lineages.

**New types in `evolve.rs`:**

```rust
pub struct LineageBudget {
    pub seed_hash: String,
    pub allocated_fraction: f64,
    pub cumulative_cb: u64,
    pub recent_scores: VecDeque<f64>,  // last N best scores (use Vec, no VecDeque — std only)
    pub stall_count: u32,
    pub active: bool,
}

pub struct AdaptiveBudgetManager {
    lineages: Vec<LineageBudget>,
    window_size: usize,           // how many gens to look back (default 5)
    max_fraction: f64,            // max single lineage share (default 0.5)
    min_improvement: f64,         // Δscore threshold (default 0.01)
}

impl AdaptiveBudgetManager {
    pub fn new(config: AdaptiveBudgetConfig) -> Self;
    pub fn register_lineage(seed_hash: &str);
    pub fn update(seed_hash: &str, gen_best: f64, cb_spent: u64);
    pub fn allocate_slots(total_slots: usize) -> Vec<(String, usize)>;
    pub fn is_active(seed_hash: &str) -> bool;
    pub fn report() -> String;  // human-readable allocation table
}
```

**CLI:**

```bash
agentis evolve seed.ag -g 20 -n 8 --adaptive-budget
agentis evolve seed.ag -g 20 -n 8 --adaptive-budget --max-lineage-fraction 0.6
agentis evolve seed.ag -g 20 -n 8 --adaptive-budget --lineage-stall-window 3
```

**Integration with evolution loop:**

```rust
// At end of each generation, if adaptive_budget enabled:
budget_mgr.update(&lineage_hash, gen_best, cb_spent);
let slot_allocation = budget_mgr.allocate_slots(population);

// Print allocation in generation summary:
// Gen  5: best=0.850  avg=0.720  budget=[seed-a: 75%, lib-b: 25%]
```

**Generation summary output with adaptive budget:**

```
Gen  5: best=0.850  avg=0.720  prompts=2.1  ckpt=ab3f...  (8 variants)
        budget: seed-a 6/8 (+2), lib-entry-b 2/8 (-1)
```

**Lineage termination:** When a lineage is terminated (stall > hard
cap), its slots are redistributed. The termination is logged:

```
  Lineage lib-entry-b terminated (stalled 8 gens at 0.720)
  Reallocated 2 slots to seed-a
```

**Tests:** allocation with single lineage, two lineages growing/stalling,
termination + redistribution, slot rounding, normalization.

### M42: Event Hooks

Deliverable: new hook parsing in `config.rs` or `evolve.rs`, changes
to `main.rs`

Config-driven reactions to evolution events.

**Config syntax (in `.agentis/config`):**

```
hooks.on_stagnation = reduce_budget 0.3
hooks.on_new_best = checkpoint tag=improved
hooks.on_validation_fail = retry 2 temp+0.2
hooks.on_validation_fail_fallback = switch_model claude-haiku-4-5-20251001
hooks.on_crash = checkpoint
```

**Parsing:**

```rust
pub enum HookEvent {
    Stagnation,
    NewBest,
    ValidationFail,
    Crash,
}

pub enum HookAction {
    ReduceBudget(f64),
    InjectLibrary(usize),
    Checkpoint,
    Tag(String),
    LibAdd,
    Retry { count: usize, temp_delta: f64 },
    SwitchModel(String),
    Log(String),
    Skip,
}

pub struct Hook {
    pub event: HookEvent,
    pub actions: Vec<HookAction>,
}

pub fn parse_hooks(cfg: &Config) -> Vec<Hook>;
```

**Hook execution points in the evolution loop:**

| Hook | Check location |
|------|---------------|
| `on_stagnation` | After stall_count update, before early-stop check |
| `on_new_best` | After best_ever update |
| `on_validation_fail` | Inside arena evaluation, on validate block failure |
| `on_crash` | Inside arena evaluation, on evaluation error |

**`retry` semantics:** When a variant fails validation, re-run it up to
`count` times. Each retry increases the LLM temperature hint by
`temp_delta` (passed to the backend via a new optional `temperature`
field in prompt evaluation context). If all retries fail, score = 0.0.

**`inject_library` semantics:** Replace the `count` lowest-scoring
variants in the current generation with top library entries (if
available). This is a one-time injection triggered by stagnation.

**Safety:** Hooks cannot execute arbitrary shell commands. Only the
predefined action vocabulary is supported. Invalid hook syntax is
reported at startup (before evolution begins).

**Tests:** hook parsing, each action type, invalid syntax rejection,
hook execution ordering.

### M43: Portable Library Export / Import

Deliverable: changes to `library.rs`, `main.rs`

Share library entries across machines.

**CLI:**

```bash
agentis lib export --tag "email-v3" --out bundle.alib
agentis lib export --top 5 --out best-5.alib
agentis lib export --all --out full.alib
agentis lib import bundle.alib
agentis lib import bundle.alib --skip-duplicates
```

**Export implementation:**

1. Select entries (by tag, top-K, or all).
2. Serialize each entry.
3. Write bundle: magic + version + count + length-prefixed entries.

**Import implementation:**

1. Read bundle header, validate magic and version.
2. For each entry: deserialize, compute hash, check for duplicate.
3. Store new entries, update index.
4. Report: N imported, M skipped (duplicates).

**Bundle format:**

```
Header:
  [4 bytes magic: "ALIb"]
  [u8 version: 1]
  [u32 entry_count]

Entries:
  per entry:
    [u32 blob_len][serialized LibraryEntry bytes]
```

**Edge cases:**
- Import from corrupted bundle: stop at first corrupt entry, report
  how many were successfully imported.
- Import with `--skip-duplicates` (default): check source_hash, skip
  if already present.
- Export with no matching entries: error, don't create empty bundle.

**Tests:** export/import roundtrip, duplicate handling, corrupt bundle
recovery, empty selection error.

## Files Changed

| File | Change |
|------|--------|
| `src/library.rs` | **New.** LibraryStore, LibraryEntry, binary encode/decode, index management, tag CRUD, search, export/import |
| `src/evolve.rs` | `LineageBudget`, `AdaptiveBudgetManager`, warm-start RNG, hook types and parsing |
| `src/main.rs` | `lib` subcommands (add, list, show, search, remove, tags, tag, export, import). `evolve` flags: `--seed-from-lib`, `--seed-top-k`, `--warm-start-prob`, `--warm-start-decay`, `--adaptive-budget`, `--max-lineage-fraction`, `--lineage-stall-window`, `--no-lib-add`, `--lib-add-interval`. Help text |
| `src/config.rs` | Hook parsing utilities (optional, may live in evolve.rs instead) |
| `CLAUDE.md` | Phase 10 docs |
| `docs/phase10-plan.md` | This document |

## Constraints

- **Zero new crate dependencies.** SHA-256 via existing `sha2`. Binary
  serialization by hand. File I/O via `std::fs`. PRNG hand-rolled.
- **Zero new syntax or builtins.** Library and hooks are CLI/config
  infrastructure.
- **Zero async/Tokio.** All I/O is blocking.
- **No serde.** Binary format is hand-rolled.
- **All existing tests pass.** Target ~680+ with Phase 10 tests.
- **Library format is internal.** No stability guarantees across
  versions. Bundle format (`.alib`) has a version byte for future
  compatibility but no formal versioning policy.

## What Phase 10 Does NOT Include

- **No deterministic replay.** LLM outputs are inherently non-
  deterministic (even at temperature=0). Warm-start improves starting
  points but doesn't reproduce results.
- **No language-level hooks.** Hooks are config entries, not `on_crash`
  blocks in `.ag` code. Language extensions are a separate effort.
- **No distributed library sync.** Libraries are local (or shared via
  export/import bundles). Real-time library replication across colony
  nodes is out of scope.
- **No automatic hyperparameter tuning.** Adaptive budget adjusts slot
  allocation, not mutation parameters (temperature, prompt templates).
  Meta-evolution is a future topic.
- **No semantic search via embeddings.** `lib search` uses substring
  matching on descriptions and simple fuzzy matching on tags
  (Levenshtein ≤ 2). Embedding-based similarity search would require
  a vector store — out of scope.
- **No cross-run lineage merging.** Lineages within a run are tracked
  (Phase 7). Lineages across runs (via library) are linked by
  provenance fields but not formally merged into a unified graph.

## Relationship to Existing Features

| Feature | Phase | Scope | Phase 10 interaction |
|---------|-------|-------|----------------------|
| Fitness scoring | 7 | Composite fitness F ∈ [0,1] | Library stores fitness metrics per entry |
| Mutation engine | 7 | Source-level agent prompt mutation | Warm-start mutates from library entries |
| Arena runner | 7–8 | Side-by-side variant evaluation | Unchanged — library feeds initial population |
| Evolution loop | 7 | Generational mutate→arena→select | Extended with warm-start, adaptive budget, hooks |
| Checkpoints | 9 | Per-generation evolution snapshots | Hooks can trigger checkpoints; library references run hashes |
| Colony workers | 8 | Distributed evaluation | Unchanged — adaptive budget is coordinator-side |
| Config system | 3 | key=value config file | Extended with hook entries |

## Success Criteria

Phase 10 is complete when:
1. `agentis lib add/list/show/search/remove` manages elite variants
2. `agentis evolve --seed-from-lib` starts from library entries
3. `agentis evolve --warm-start-prob 0.4` injects library variants
4. `agentis evolve --adaptive-budget` reallocates slots per lineage
5. Config hooks fire on stagnation, new best, validation fail, crash
6. `agentis lib export/import` creates and reads `.alib` bundles
7. Auto-add to library on successful evolution runs (score > threshold)
8. Zero new deps, zero async, all existing tests pass
