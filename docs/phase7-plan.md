# Phase 7: Agent Evolution

## Vision

Phases 1–6 built a safe, observable, developer-friendly runtime. Phase 7
makes agents **compete**. Agentis agents evolve through mutation,
selection, and fitness pressure — the CB system and validate predicates
become a natural selection environment.

No new builtins. No new language syntax. Evolution is a CLI tool that
orchestrates existing primitives: run programs, collect metrics, mutate
prompts (via the LLM itself), select winners.

## Design Principle: Evolution IS Prompt

Mutation in Agentis is not random bit-flipping. Since "everything is
prompt," mutation = rephrasing agent instructions via the configured LLM
backend. The LLM generates variations of an agent's prompt instruction.
With mock backend, deterministic perturbations are applied instead.

This is beautifully recursive: agents evolve through the same mechanism
they use to think. The LLM that runs agents also breeds them.

## Fitness Model

A single composite fitness score `F ∈ [0.0, 1.0]` computed from:

```
F = w_cb * CB_efficiency + w_val * validate_rate + w_exp * explore_rate

where:
  CB_efficiency  = budget_remaining / initial_budget    (higher = leaner)
  validate_rate  = validates_passed / validates_total   (1.0 if no validates)
  explore_rate   = explores_passed / explores_total     (1.0 if no explores)

default weights: w_cb = 0.3, w_val = 0.5, w_exp = 0.2
```

**Configurable weights:** Override via CLI `--weights 0.4,0.4,0.2` (cb,val,exp)
or config `fitness.weights = 0.4,0.4,0.2`. Must sum to 1.0.

**Dynamic weight redistribution:** When a program has no `explore` blocks
(explores_total = 0), the explore weight is redistributed proportionally
to the other two components:

```
effective_w_cb  = w_cb / (w_cb + w_val)
effective_w_val = w_val / (w_cb + w_val)
effective_w_exp = 0.0
```

Same logic applies if validates_total = 0 (redistribute to cb + exp).

**Edge case — no validates AND no explores:** If both are absent,
all weight concentrates on CB_efficiency: `F = CB_efficiency`. The
report prints a warning: "No validate/explore blocks — fitness = CB
efficiency only." In `evolve` runs, this warning is printed once per
evolution (not per variant) to avoid spam.

Agents that pass all validations with minimal CB usage score highest.
Agents that burn through budget or fail validates score lowest.
An agent that dies (error) gets fitness 0.0.

## Milestones

- [x] M27: Fitness Metrics
- [ ] M28: Arena Runner
- [ ] M29: Mutation Engine
- [ ] M30: Evolution Loop

### M27: Fitness Metrics

Deliverable: new `fitness.rs`, changes to `evaluator.rs`, `main.rs`

Collect execution metrics and compute fitness scores. Every program
execution can optionally produce a `FitnessReport`.

**CLI:**

```bash
agentis go examples/classify.ag --fitness    # run + print fitness report
```

**Output:**

```
[genesis] a1b2c3d4e5f6
Mood of the day: mock

Fitness Report:
  CB efficiency:   0.95 (9500/10000)
  Validate rate:   1.00 (3/3 passed)
  Explore rate:    0.50 (1/2 passed)
  Prompt calls:    4
  Fitness score:   0.815
```

**Implementation:**

New struct `FitnessReport` in `fitness.rs`:

```rust
pub struct FitnessWeights {
    pub w_cb: f64,
    pub w_val: f64,
    pub w_exp: f64,
}

impl FitnessWeights {
    pub fn default() -> Self { Self { w_cb: 0.3, w_val: 0.5, w_exp: 0.2 } }
    pub fn parse(s: &str) -> Result<Self, String> { ... } // "0.4,0.4,0.2"
}

pub struct FitnessReport {
    pub cb_initial: u64,
    pub cb_remaining: u64,
    pub validates_passed: usize,
    pub validates_total: usize,
    pub explores_passed: usize,
    pub explores_total: usize,
    pub prompt_count: usize,  // not in composite score — reported for diagnostics
    pub error: bool,
}

impl FitnessReport {
    pub fn score(&self) -> f64 { self.score_with(&FitnessWeights::default()) }
    pub fn score_with(&self, w: &FitnessWeights) -> f64 { ... } // dynamic redistribution
    pub fn cb_efficiency(&self) -> f64 { ... }
    pub fn validate_rate(&self) -> f64 { ... }
    pub fn explore_rate(&self) -> f64 { ... }
}
```

`score_with()` implements dynamic weight redistribution: if
`explores_total == 0`, redistributes `w_exp` proportionally to `w_cb`
and `w_val`. Same for missing validates.

Changes to `Evaluator`:
- Add `prompt_count: usize` field, increment on each `eval_prompt` call.
- Add `validates_passed` and `validates_total` counters.
- Add `explores_passed` and `explores_total` counters.
- New method `fitness_report() -> FitnessReport` that snapshots current
  metrics.
- Counters work regardless of test mode — always tracked.

Fitness reports are stored content-addressed in `.agentis/objects/`
using the existing `json.rs` serialization. A fitness registry
(`.agentis/fitness.jsonl`) appends one entry per scored run:

```json
{"ts": 1710000000, "source_hash": "abc123...", "score": 0.815, "cb_eff": 0.95, "val_rate": 1.0, "exp_rate": 0.5, "prompt_count": 4, "weights": "0.3,0.5,0.2"}
```

The `weights` field records which weights were used, ensuring
reproducibility even when default weights change in config.

### M28: Arena Runner

Deliverable: changes to `main.rs`, new `arena.rs`

Run multiple program variants side by side, rank by fitness, report
standings. The arena is the selection environment.

**CLI:**

```bash
agentis arena variant1.ag variant2.ag variant3.ag
agentis arena variants/                    # all .ag files in directory
agentis arena variants/ --rounds 5         # run each variant 5 times, average fitness
agentis arena variants/ --top 3            # show only top 3
agentis arena variants/ --json             # machine-readable JSON output
```

**Output:**

```
Arena: 4 variants, 1 round each

RANK  FILE                SCORE   CB_EFF  VAL    EXP
  1   variant-c.ag        0.915   0.98    1.00   0.67
  2   variant-a.ag        0.815   0.95    1.00   0.50
  3   variant-d.ag        0.700   0.80    0.75   1.00
  4   variant-b.ag        0.000   —       —      — (error: CognitiveOverload)

Winner: variant-c.ag (score: 0.915)
```

**Implementation:**

- Reuse `cmd_go` logic (parse, commit, create evaluator).
- Run each variant with fitness collection enabled.
- With `--rounds N`: run each variant N times, average fitness.
  Non-determinism from real LLM backends means multiple rounds give
  more reliable rankings.
- Sort by fitness score descending.
- Report table with rank, file, score, component rates.
- `--json`: output results as JSON array for downstream analysis:
  ```json
  [{"rank":1,"file":"variant-c.ag","score":0.915,"cb_eff":0.98,"val_rate":1.0,"exp_rate":0.67,"prompt_count":3,"error":null},
   {"rank":4,"file":"variant-b.ag","score":0.0,"cb_eff":null,"val_rate":null,"exp_rate":null,"prompt_count":0,"error":"CognitiveOverload"}]
  ```
  When called from `evolve`, entries include `"gen": 3, "round": 2`
  for post-processing across generations. Error strings are truncated
  to 80 chars to keep JSON compact. When `--rounds > 1`, entries
  include `"rounds": 5, "rounds_avg": true` to distinguish averaged
  scores from single-run scores.
- Exit code 0 if at least one variant succeeds.

**What the arena is NOT:**

- Not head-to-head (variants don't compete directly — they're scored
  independently against the same task).
- Not parallel (variants run sequentially — parallel is Phase 8 colony).

### M29: Mutation Engine

Deliverable: new `mutation.rs`, changes to `main.rs`, `parser.rs`

Generate agent variants by mutating prompt instructions. The mutation
engine uses the configured LLM backend — evolution is itself a prompt.

**CLI:**

```bash
agentis mutate examples/classify.ag --count 5          # generate 5 variants
agentis mutate examples/classify.ag --out variants/    # save to directory
agentis mutate examples/classify.ag --agent classifier # mutate only "classifier" agent
agentis mutate examples/classify.ag --mutate-prompt mutation_prompt.txt  # custom mutation prompt
agentis mutate examples/classify.ag --dry-run          # show what would be mutated, don't write
agentis mutate examples/classify.ag --list-agents      # list agent names in file (for --agent)
```

**Output:**

```
Mutating examples/classify.ag (2 agents found)

Generated 5 variants:
  variants/classify-m1.ag   mutated: classifier (instruction rephrased)
  variants/classify-m2.ag   mutated: classifier (instruction rephrased)
  variants/classify-m3.ag   mutated: sanitize (instruction rephrased)
  variants/classify-m4.ag   mutated: classifier, sanitize (both mutated)
  variants/classify-m5.ag   mutated: classifier (instruction rephrased)
```

**Mutation strategy:**

1. Parse the source file, extract all `agent` declarations.
2. If `--agent <name>` is given, filter to only that agent. Otherwise
   pick a random agent for each mutation.
3. Mutation = LLM call with the default mutation prompt:
   `"Rephrase this instruction differently while preserving its intent:
   <original instruction>"`.
4. If `--mutate-prompt <file>` is given, use the file contents as the
   mutation prompt template instead. The template must contain `{instruction}`
   as a placeholder for the original instruction text.
5. With mock backend: cycle through 8 deterministic perturbations to
   ensure variety across mutations: prepend "Carefully ", append
   " Be precise.", prepend "Step by step: ", append " Think twice.",
   prepend "As an expert, ", append " Be thorough.", prepend
   "Concisely ", append " Double-check your work.". Selection is
   `perturbations[mutation_index % 8]`.
6. Reconstruct the source with the mutated instruction.
7. Write to output file (or print diff if `--dry-run`).

**`--dry-run`:** Shows which agents would be mutated and what the new
instructions would look like, without writing any files. Output uses
diff-style format for easy comparison:

```
Variant 1/5:
  Agent: classifier
  Old:  "Classify the text as positive or negative."
  New:  "Step by step: Classify the text as positive or negative."

Variant 2/5:
  Agent: sanitize
  Old:  "Remove all PII from the input."
  New:  "Remove all PII from the input. Be precise."
```

If `--mutate-prompt` is used, the header shows: "Using custom prompt
from mutation_prompt.txt" (or first 80 chars if inline).

Useful for previewing mutations before committing to a batch.

**Source reconstruction:**

The mutation engine works at the source text level, not the AST level.
This avoids needing an AST-to-source printer (which doesn't exist and
would be complex).

The approach uses **lexer token positions** (not regex) for robustness:
1. Lex the source file, collecting all tokens with byte offsets.
2. Parse to identify agent blocks and their `prompt()` calls.
3. For each target prompt, find the string literal token (the first
   argument to `prompt()`). The lexer already records `(start, end)`
   byte positions for every token.
4. Replace `source[start..end]` with the new quoted string literal.
5. Rebuild the full source by concatenating unchanged slices with
   replaced slices.

This handles escaped quotes, multiline strings, and unusual formatting
correctly — the lexer already solved these problems. No regex needed.

**What mutation is NOT:**

- Not genetic crossover (no combining two agents — that's complex and
  low-value for single-agent evolution).
- Not random noise (mutations are semantically meaningful — LLM-guided).
- Not structural (doesn't add/remove parameters, change types, or alter
  control flow — only prompt instruction text changes).

### M30: Evolution Loop

Deliverable: changes to `main.rs`

Tie everything together: mutate → arena → select → repeat. The full
evolutionary loop runs for G generations with population size N.

**CLI:**

```bash
agentis evolve examples/classify.ag --generations 10 --population 8
agentis evolve examples/classify.ag -g 10 -n 8 --out evolved/
agentis evolve examples/classify.ag -g 5 -n 4 --show-lineage
agentis evolve examples/classify.ag -g 10 -n 8 --weights 0.4,0.4,0.2
agentis evolve examples/classify.ag -g 10 -n 8 --agent classifier
agentis evolve examples/classify.ag -g 10 -n 8 --budget-cap 500000
agentis evolve examples/classify.ag -g 20 -n 8 --stop-on-stall 5
agentis evolve examples/classify.ag --dry-run -g 10 -n 8    # estimate cost, don't run
agentis lineage evolved/classify-g10-best.ag    # trace ancestry to seed
```

**Output:**

```
Evolution: classify.ag
  Population: 8, Generations: 10

Gen  1: best=0.815  avg=0.542  prompts=3.5  (8 variants)
Gen  2: best=0.870  avg=0.621  prompts=3.1  (8 variants)
Gen  3: best=0.870  avg=0.690  prompts=2.8  (8 variants)
...
Gen 10: best=0.935  avg=0.812  prompts=2.4  (8 variants)

Best agent: evolved/classify-g10-best.ag (score: 0.935)
  Lineage: classify.ag → g1-m3 → g4-m1 → g7-m2 → g10-best
  Efficiency: prompt calls -31% (3.5 → 2.4 avg)
```

Efficiency line adapts: "-31%" when reduced, "+12%" when increased
(neutral — more prompts isn't necessarily bad), omitted if unchanged.

**Algorithm:**

```
1. Parse original program → generation 0 (the "seed").
2. For each generation g = 1..G:
   a. Mutate: from top K parents, generate N variants (mutation engine).
   b. Arena: run all N variants, collect fitness scores.
   c. Select: keep top K variants as parents for next generation.
      K = N/2 (tournament selection, top half survives).
   d. Record: store generation results in per-generation fitness file.
   e. Save intermediate best: write <out>/g{gen:02}-best.ag.
3. Output best-of-run agent source file.
```

**`--budget-cap <N>`:** Total CB budget across the entire evolution run.
Tracks cumulative CB spent across all arena evaluations. If exceeded,
evolution stops early with: "Budget cap reached at generation {g}
({spent}/{cap} CB spent)". Prevents runaway cost with real LLM backends.

**`--dry-run`:** Estimates the evolution run without executing:
- Number of mutations per generation (N * G total)
- Number of arena evaluations (N * G * rounds)
- Estimated prompt calls (based on seed program's prompt count * evaluations)
- Cost estimate per backend:
  - **MockBackend:** "Estimated cost: $0 (mock mode)"
  - **CliBackend/Ollama:** "Local inference — cost $0, estimated time:
    ~X min" (based on avg prompt length × evaluations, assuming ~30 tok/s;
    override via config `ollama_tokens_per_second = 50.0`)
  - **HttpBackend:** "Estimated cost: ~$X.XX" — approximation: count
    prompt instruction string lengths in seed (1 token ≈ 4 chars),
    multiply by evaluations

**Intermediate bests:** After each generation, the best variant is saved
to `<out>/g{gen:02}-best.ag` (e.g., `evolved/g03-best.ag`). This allows
the user to inspect or use intermediate winners without waiting for the
full evolution run to complete. The final best is also copied to
`<out>/<name>-best.ag` for convenience.

**Lineage tracking:**

Each variant is identified by `(source_hash, generation, parent_hash)`.
Per-generation fitness files are stored in `.agentis/fitness/g{gen:02}.jsonl`,
one JSONL entry per variant scored in that generation:

```json
{"ts": 1710000005, "gen": 3, "source_hash": "def789...", "parent_hash": "abc123...", "score": 0.870, "prompt_count": 3, "mutations": ["classifier"], "weights": "0.3,0.5,0.2"}
```

`--show-lineage` traces the best agent's ancestry back to the seed.

**Storage cleanup:** Over many evolution runs, `.agentis/fitness/` can
accumulate many files. `agentis evolve --clean` removes per-generation
files from previous runs (keeps only the latest run's files). Not
automatic — user decides when to clean.

**`agentis lineage <file>`:** Standalone command that reads the fitness
registry and traces the given variant's ancestry. Takes the source hash
from the file (by hashing its content) and walks `parent_hash` links
back to the seed.

Implementation: load all per-generation JSONL files into an in-memory
`HashMap<source_hash, (gen, parent_hash, score)>`, then walk the chain
from the target hash until `parent_hash` is absent (= seed). If a
parent's score is missing (e.g., old/cleaned run), show "score unknown"
in the chain. This is fast — even 100 generations × 20 variants = 2000
entries in memory.

Output:

```
classify.ag (seed) → g1-m3 (0.72) → g4-m1 (0.81) → g7-m2 (0.87) → g10-best (0.935)
```

**Convergence detection:**

If the best fitness doesn't improve for 3 consecutive generations,
print a warning: "Evolution stalled at generation {g} (score: {s})".
By default, evolution continues — the user controls the generation count.

`--stop-on-stall <N>`: Automatically stop evolution if the best fitness
doesn't improve for N consecutive generations. Default: off (warn only).
Useful for unattended runs where continuing past convergence wastes budget.

**What evolution is NOT:**

- Not automatic (user decides when to evolve and how many generations).
- Not continuous (no daemon — runs once and exits).
- Not multi-objective (single composite fitness score).
- Not genetic programming (doesn't modify code structure — only prompt
  instruction text within agents).

## Implementation Order

1. **M27** — fitness metrics (foundation for everything)
2. **M29** — mutation engine (visible "wow" effect — mutated agents immediately)
3. **M28** — arena runner (now mutated variants can be compared)
4. **M30** — evolution loop (ties M27 + M28 + M29 together)

M29 before M28: mutation is the most demonstrable feature. After M27+M29,
a user can `agentis go file.ag --fitness` then `agentis mutate file.ag`
and immediately see evolved variants — even without the arena. The arena
adds ranking, the loop adds automation.

## Future Extensions (post-Phase 7)

These are explicitly **out of scope** for Phase 7 but noted for future
consideration:

- **Selection strategies:** `--selection {tournament|roulette|elitism}`.
  Tournament (top K/2) is the Phase 7 default. Roulette and elitism
  may help with diversity vs convergence tradeoffs.
- **Diversity bonus:** Small fitness bonus for variants with dissimilar
  instructions (e.g., cosine distance on token embeddings). Prevents
  premature convergence in long runs.
- **Auto-commit to VCS:** Commit best-of-gen to a branch
  (e.g., `evolve/classify/g03-best`) leveraging the existing VCS.
- **Parallel arena:** Run arena variants in parallel threads (Phase 8
  colony territory).

## Success Criteria

Phase 7 is complete when:
1. `agentis go file.ag --fitness` reports CB efficiency, validate/explore
   rates, and composite fitness score with configurable weights
2. `agentis arena dir/` ranks multiple variants by fitness with table output
3. `agentis mutate file.ag --count N` generates N semantically meaningful
   variants by rephrasing prompt instructions, with `--agent` filter and
   `--dry-run` preview
4. `agentis evolve file.ag -g G -n N` runs the full evolutionary loop
   with lineage tracking, intermediate bests, and best-of-run output
5. `agentis lineage <file>` traces a variant's ancestry back to the seed
6. Evolution uses the LLM itself for mutation — recursive self-improvement
7. Zero new builtins. Zero new language syntax. Zero async.

## What Phase 7 Does NOT Include

- **No `mutate()` builtin.** Mutation is a CLI tool, not a language feature.
- **No `fitness()` builtin.** Fitness is computed by the runtime, not by
  agent code. Agents don't know they're being evolved.
- **No crossover.** Single-parent mutation only. Crossover is complex and
  low-value for prompt-level evolution.
- **No distributed evolution.** Single-instance. Distributed is Phase 8.
- **No real-time evolution.** Batch process, not continuous daemon.
- **No multi-objective optimization.** Single composite fitness score.
