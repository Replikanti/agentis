# Phase 9: Evolution Checkpoints

## Vision

Phases 7 and 8 built a complete evolution pipeline — local and distributed.
But evolution runs are fragile: a crash, a network hiccup, or a simple
Ctrl+C after 6 hours of computation loses all progress. Phase 9 makes
evolution runs **resumable**.

Every generation produces a checkpoint — a content-addressed snapshot of
the evolution state stored in the DAG. Checkpoints form a chain: each
points to the previous. You can resume from any checkpoint, inspect the
history of any evolution run, find the best-scoring generation, and
garbage-collect old runs you no longer need.

No new builtins. No new language syntax. No new crate dependencies.
Checkpoints are a CLI-level persistence layer for the evolution loop.

## Design Principle: State in the DAG, Not in Files

Agentis already stores code as content-addressed objects. Evolution state
deserves the same treatment. Instead of `.checkpoints/` directories with
JSON files, checkpoints are hashed, immutable DAG nodes stored alongside
(but separate from) code objects.

This means:
- Checkpoints are immutable and tamper-evident (SHA-256 verified).
- Duplicate state is automatically deduplicated.
- The checkpoint chain is a first-class data structure, not a pile of
  files with naming conventions.

## Checkpoint Store

Checkpoints live in `.agentis/colony/`, separate from `.agentis/objects/`.
Same SHA-256 content-addressed scheme, same fan-out directory layout
(`{prefix:2}/{rest}`), but independent namespace. This keeps `sync` and
`compile` clean — they never see checkpoint data.

```
.agentis/colony/
  objects/          # content-addressed checkpoint blobs
    ab/
      cd1234...     # SHA-256 hash of serialized GenerationCheckpoint
  HEAD              # hash of the latest checkpoint (any run)
  tags/
    nightly-03-12   # file containing hash → named checkpoint
    best-so-far     # file containing hash → named checkpoint
```

**HEAD** points to the most recent checkpoint written by any `evolve` run.
Updated atomically after each generation.

**Tags** are human-readable names for specific checkpoints. Created via
`--tag` on evolve or `colony tag <hash> <name>` command. A tag file
contains exactly one line: the checkpoint hash.

## GenerationCheckpoint

The fundamental data structure. One per completed generation.

```rust
pub struct GenerationCheckpoint {
    // Chain
    pub generation: u32,
    pub parent: Option<String>,       // hash of previous checkpoint (None for gen 1)

    // Identity
    pub seed_hash: String,            // hash of the original seed source

    // Evolution state (needed for resume)
    pub parents: Vec<ParentEntry>,    // surviving population for next generation
    pub best_ever_score: f64,
    pub best_ever_source: String,
    pub best_ever_hash: String,
    pub stall_count: u32,
    pub cumulative_cb: u64,
    pub first_gen_avg_prompts: f64,

    // This generation's results
    pub gen_best_score: f64,
    pub gen_avg_score: f64,
    pub gen_avg_prompts: f64,
    pub variant_count: u32,

    // Metadata
    pub timestamp: u64,               // Unix millis
    pub tag: Option<String>,
}

pub struct ParentEntry {
    pub source: String,
    pub source_hash: String,
}
```

**Why store parents inline?** Population is typically 4–16 variants, each
1–2 KB of source. A checkpoint is ~20–50 KB — acceptable. Storing sources
as separate objects would add complexity (two-phase lookups, orphan risk)
for negligible space savings.

**Why not store full arena results?** Arena results for each generation are
already in `.agentis/fitness/g{gen}.jsonl` (Phase 7 lineage). Duplicating
them in checkpoints wastes space. The checkpoint stores only the summary
stats needed for resume (`gen_best_score`, `gen_avg_score`, etc.) and
points to the lineage files for details.

## Binary Serialization

Same approach as AST serialization in `ast.rs` — hand-rolled, no serde.

```
Header:
  [4 bytes magic: "AGCK"]            # Agentis Checkpoint
  [u8 version: 1]

Chain:
  [u32 generation]
  [u8 has_parent][32 bytes parent_hash if has_parent]
  [u32 seed_hash_len][seed_hash]

Evolution state:
  [u32 parent_count]
    per parent:
      [u32 source_len][source bytes]
      [u32 hash_len][hash bytes]
  [f64 best_ever_score]
  [u32 best_ever_source_len][best_ever_source]
  [u32 best_ever_hash_len][best_ever_hash]
  [u32 stall_count]
  [u64 cumulative_cb]
  [f64 first_gen_avg_prompts]

Generation results:
  [f64 gen_best_score]
  [f64 gen_avg_score]
  [f64 gen_avg_prompts]
  [u32 variant_count]

Metadata:
  [u64 timestamp_ms]
  [u8 has_tag][u32 tag_len][tag bytes if has_tag]
```

All multi-byte integers are little-endian. All f64 values are IEEE 754 LE
via `f64::to_le_bytes()`/`from_le_bytes()`. Strings are UTF-8.

## Resume Flow

```
agentis evolve seed.ag --resume <hash-or-tag> -g 20 -n 8
```

1. Resolve `<hash-or-tag>`: check tags first, then prefix-match against
   checkpoint hashes.
2. Load `GenerationCheckpoint` from colony store.
3. Restore state: `parents`, `best_ever_*`, `stall_count`, `cumulative_cb`,
   `first_gen_avg_prompts`.
4. Start loop from `generation + 1` with the target generation count being
   `checkpoint.generation + cli_generations`.
5. Lineage files in `.agentis/fitness/` already exist for completed
   generations — the loop appends new ones.
6. Output directory: if `--out` specified, use it; otherwise continue
   writing to the same directory structure.

**Config on resume:** The user can change some parameters when resuming:
- `-g N` — how many *additional* generations to run (not total)
- `-n N` — population size (can differ from original run)
- `--weights` — fitness weights (can adjust mid-run)
- `--stop-on-stall`, `--budget-cap` — can tighten or relax
- `--workers`, `--threads` — can change parallelism

The checkpoint does *not* store config. Config comes from CLI flags and
`.agentis/config` at resume time. This is intentional: you should be able
to resume with different parameters (more generations, different weights,
more workers).

## Milestones

- [ ] M35: Checkpoint Store
- [ ] M36: Auto-Checkpoint + Resume
- [ ] M37: Colony History CLI
- [ ] M38: Garbage Collection

### M35: Checkpoint Store

Deliverable: new `src/checkpoint.rs`, changes to `main.rs`

Foundation: content-addressed checkpoint storage with serialization.

**New `src/checkpoint.rs`:**

```rust
pub struct CheckpointStore { ... }

impl CheckpointStore {
    pub fn new(agentis_root: &Path) -> Self;     // .agentis/colony/
    pub fn store(checkpoint: &GenerationCheckpoint) -> Result<String, ...>;
    pub fn load(hash: &str) -> Result<GenerationCheckpoint, ...>;
    pub fn head() -> Result<Option<String>, ...>;
    pub fn set_head(hash: &str) -> Result<(), ...>;
    pub fn set_tag(name: &str, hash: &str) -> Result<(), ...>;
    pub fn resolve_tag(name: &str) -> Result<Option<String>, ...>;
    pub fn list_tags() -> Result<Vec<(String, String)>, ...>;
    pub fn resolve(hash_or_tag: &str) -> Result<String, ...>;  // tag lookup + prefix match
    pub fn exists(hash: &str) -> bool;
}
```

- SHA-256 hashing via existing `sha2` crate (same as `ObjectStore`)
- Fan-out storage: `.agentis/colony/objects/{prefix:2}/{rest}`
- Binary encode/decode for `GenerationCheckpoint`
- HEAD file: `.agentis/colony/HEAD` (single line, checkpoint hash)
- Tags: `.agentis/colony/tags/{name}` (single line, checkpoint hash)
- Prefix matching for hash resolution (like snapshot prefix matching)

**CLI (minimal, for testing):**

```bash
agentis colony tags                    # list all tags
agentis colony tag <hash> <name>       # create named tag
```

**Tests:** encode/decode roundtrip, store/load, HEAD read/write, tag
CRUD, prefix resolution, invalid hash handling.

### M36: Auto-Checkpoint + Resume

Deliverable: changes to `main.rs` (cmd_evolve), `checkpoint.rs`

Integrate checkpointing into the evolution loop.

**Auto-checkpoint:**

After each generation (or every N generations with `--checkpoint-interval`):
1. Build `GenerationCheckpoint` from current state.
2. Store in checkpoint store.
3. Update HEAD.
4. Print checkpoint hash in generation summary line.

```
Gen  1: best=0.815  avg=0.542  prompts=3.5  ckpt=ab3f...
Gen  2: best=0.850  avg=0.612  prompts=3.1  ckpt=cd91...
```

**CLI:**

```bash
# Normal run with auto-checkpointing (every generation by default)
agentis evolve seed.ag -g 10 -n 8

# Checkpoint every 5 generations (less I/O)
agentis evolve seed.ag -g 100 -n 8 --checkpoint-interval 5

# Tag the final checkpoint
agentis evolve seed.ag -g 10 -n 8 --tag "experiment-a"

# Resume from checkpoint
agentis evolve seed.ag --resume ab3f -g 10 -n 8
agentis evolve seed.ag --resume "experiment-a" -g 20 -n 8
```

**Resume implementation:**

```rust
// In cmd_evolve, before the loop:
let (start_gen, mut parents, mut best_ever_score, ...) = if let Some(resume_ref) = resume {
    let store = CheckpointStore::new(&agentis_root);
    let hash = store.resolve(resume_ref)?;
    let ckpt = store.load(&hash)?;
    (ckpt.generation + 1, ckpt.parents_as_tuples(), ckpt.best_ever_score, ...)
} else {
    (1, vec![(seed_source, seed_hash)], 0.0, ...)
};

for g in start_gen..=(start_gen + generations - 1) {
    // ... existing loop body, unchanged ...
    // ... at end of loop: store checkpoint ...
}
```

**Edge cases:**
- Resume with `--resume` but checkpoint's seed_hash doesn't match the
  provided seed.ag: warn but allow (user may have modified the seed).
- Resume from a tagged checkpoint that no longer exists (deleted by GC):
  error with clear message.
- `--checkpoint-interval 0`: disable auto-checkpointing entirely.
- Checkpoint interval > 1: still checkpoint on the very last generation.

**Checkpoint directory auto-creation:**

`agentis evolve` creates `.agentis/colony/objects/` and
`.agentis/colony/tags/` on first use, similar to how `agentis init`
creates `.agentis/objects/`.

### M37: Colony History CLI

Deliverable: changes to `main.rs`, `checkpoint.rs`

Commands for inspecting evolution history.

**`agentis colony history`:**

Walk the checkpoint chain from HEAD backwards.

```bash
agentis colony history              # all checkpoints from HEAD
agentis colony history --limit 10   # last 10 only
```

Output:
```
Evolution History (from HEAD)

GEN   HASH      BEST    AVG     CB_SPENT  TAG                 DATE
 10   ab3f...   0.935   0.812   48200     experiment-a        2026-03-12 23:14
  9   cd91...   0.920   0.798   43500                         2026-03-12 23:08
  8   ef12...   0.920   0.785   38900                         2026-03-12 23:02
  ...
  1   1234...   0.720   0.450    4800                         2026-03-12 22:30

10 checkpoints, best: gen 10 (0.935), seed: 8a7b...
```

**`agentis colony trace <hash-or-tag>`:**

Show details of a single checkpoint.

```bash
agentis colony trace ab3f
agentis colony trace experiment-a
```

Output:
```
Checkpoint: ab3f1234567890...
  Generation:     10
  Tag:            experiment-a
  Date:           2026-03-12 23:14:02
  Seed:           8a7b... (classify.ag)
  Best score:     0.935 (ever: 0.935)
  Avg score:      0.812
  Avg prompts:    2.4
  Stall count:    0
  Cumulative CB:  48200
  Parents:        4 survivors
  Previous:       cd91... (gen 9)
```

**`agentis colony best`:**

Find the checkpoint with the highest `best_ever_score`.

```bash
agentis colony best                   # best overall
agentis colony best --min-score 0.9   # filter
```

Output:
```
Best checkpoint: ab3f... (gen 10, score: 0.935, tag: experiment-a)
```

### M38: Garbage Collection

Deliverable: changes to `checkpoint.rs`, `main.rs`

Content-addressed stores grow without bound. GC removes unreachable
checkpoint objects.

**CLI:**

```bash
agentis colony gc                               # default: keep HEAD chain + tagged
agentis colony gc --older-than 30d              # also remove old checkpoints from live chain
agentis colony gc --older-than 7d --except-tagged  # keep tagged regardless of age
agentis colony gc --dry-run                     # show what would be deleted
```

**Algorithm:**

1. **Mark phase:** Start from HEAD, walk parent chain — mark all reachable.
   Also mark all checkpoints referenced by tags.
2. **Age filter:** If `--older-than` specified, unmark checkpoints older
   than the threshold (but respect `--except-tagged`).
3. **Sweep phase:** Delete all unmarked objects from
   `.agentis/colony/objects/`.
4. **Report:** Print count of deleted objects and freed bytes.

```
Colony GC: 45 checkpoints scanned
  Kept:    12 (HEAD chain) + 3 (tagged)
  Removed: 30 checkpoints (1.2 MB freed)
```

**Safety:**
- HEAD is never deleted.
- Tagged checkpoints are never deleted unless `--force` is used.
- `--dry-run` lists what would be deleted without deleting.
- If HEAD points to a deleted checkpoint (shouldn't happen), error
  with recovery instructions.

**Edge case: orphaned chains.** If HEAD was updated to a different run's
checkpoint, the old run's chain becomes orphaned (unreachable from HEAD).
GC removes these unless they're tagged. This is the desired behavior:
tag what you want to keep.

## Files Changed

| File | Change |
|------|--------|
| `src/checkpoint.rs` | **New.** CheckpointStore, GenerationCheckpoint, ParentEntry, binary encode/decode, HEAD/tag management, history walking, GC |
| `src/main.rs` | `evolve` command: `--resume`, `--checkpoint-interval`, `--tag` flags. `colony` subcommands: `history`, `trace`, `best`, `tags`, `tag`, `gc`. Help text |
| `src/evolve.rs` | Minor: extract checkpoint-building helper from generation state |
| `CLAUDE.md` | Phase 9 docs |
| `docs/phase9-plan.md` | This document |

## Constraints

- **Zero new crate dependencies.** SHA-256 via existing `sha2`. Binary
  serialization by hand. File I/O via `std::fs`.
- **Zero new syntax or builtins.** Checkpoints are CLI infrastructure.
- **Zero async/Tokio.** All I/O is blocking.
- **No serde.** Binary format is hand-rolled.
- **All existing tests pass.** Target ~610+ with checkpoint tests.
- **Checkpoint format is internal.** No stability guarantees across
  versions (unlike the sync protocol). Checkpoints are local-only.

## What Phase 9 Does NOT Include

- **No per-prompt tracing in checkpoints.** Granularity is per-generation.
  Individual prompt traces are in the audit log (Phase 5).
- **No hooks (`on_crash`, `on_validation_fail`).** These are language/config
  features, not persistence.
- **No per-branch CB caps.** That's an evolution engine enhancement.
- **No portable bundle export.** Checkpoints are local. Sharing evolution
  state across machines requires a different protocol.
- **No deterministic replay.** Resume continues from saved state; it does
  not reproduce the exact same LLM outputs.
- **No checkpoint sync.** `.agentis/colony/` is not included in P2P sync.
  Evolution history is local to the machine that ran it.
- **No integration with existing snapshots/fitness JSONL.** Phase 9
  checkpoints complement (not replace) Phase 6 snapshots and Phase 7
  lineage JSONL. Lineage files remain the source of truth for per-variant
  details; checkpoints store resumable state.

## Relationship to Existing Features

| Feature | Phase | Scope | Phase 9 interaction |
|---------|-------|-------|---------------------|
| Snapshots | 6 | Evaluator memory state (REPL resume) | Independent — different scope |
| Fitness JSONL | 7 | Per-variant scores per generation | Checkpoints reference, don't duplicate |
| Lineage tracking | 7 | Variant ancestry (parent hashes) | Checkpoints store summary, lineage has details |
| Audit log | 5 | Per-prompt call log | Independent — different scope |

## Success Criteria

Phase 9 is complete when:
1. `agentis evolve` auto-checkpoints after each generation
2. `agentis evolve --resume <hash>` continues from checkpoint
3. `agentis evolve --resume <tag>` resolves tag to checkpoint
4. `agentis colony history` shows checkpoint chain
5. `agentis colony trace <hash>` shows checkpoint details
6. `agentis colony best` finds highest-scoring checkpoint
7. `agentis colony gc` removes unreachable checkpoints
8. Crash mid-evolution → resume loses at most one generation of work
9. Zero new deps, zero async, all existing tests pass
