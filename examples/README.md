# Agentis Examples

## Quick start

```bash
agentis init                          # creates .agentis/ with config
# Edit .agentis/config — uncomment your LLM backend (claude, ollama, API)
agentis go examples/fast-demo.ag      # first run — output in 3-8 seconds
```

## Examples (ordered by complexity)

| # | File | Prompts | What it shows |
|---|------|---------|---------------|
| 1 | `fast-demo.ag` | 1 | Minimal program, instant gratification |
| 2 | `hello.ag` | 1 | Even hello world is a prompt |
| 3 | `functions.ag` | 0 | Pure functions, `if/else`, recursion |
| 4 | `collections.ag` | 0 | Lists, maps, `push`/`get`/`len` |
| 5 | `budget.ag` | 1 | Cognitive Budget — resource awareness |
| 6 | `classify.ag` | 1 | Typed struct output + validation |
| 7 | `io-sandbox.ag` | 1 | File read/write in the sandbox |
| 8 | `pipeline.ag` | 2 | Data pipeline — LLM as data processor |
| 9 | `parallel.ag` | 3 | Multi-agent spawn/await |
| 10 | `explore.ag` | 2 | Evolutionary branching — survive or die |
| 11 | `test-suite.ag` | 0 | Using validate/explore as tests |
| 12 | `pii-guard.ag` | 2 | PII protection — blocked vs allowed |
| 13 | `evolve-seed.ag` | 2 | Seed program for the evolution pipeline |

## Notes

- `pipeline.ag` reads `data.txt` via `file_read()`. Copy `examples/data.txt`
  to `.agentis/sandbox/data.txt` before running.
- `io-sandbox.ag` creates files in `.agentis/sandbox/`.
- `explore.ag` creates VCS branches. Run `agentis branch` after to see which
  approaches survived validation.
- `pii-guard.ag` demonstrates PII blocking. Run with `--grant-pii` to allow.
- `test-suite.ag` is meant for the test runner: `agentis test examples/test-suite.ag`
- Use `--trace` for verbose output: `agentis go examples/classify.ag --trace`

## VCS workflow

```bash
agentis init                          # initialize with genesis branch
agentis commit examples/classify.ag   # parse, store AST, update branch
agentis run genesis                   # execute from branch
agentis branch                        # list branches
agentis branch experiment             # create new branch
agentis switch experiment             # switch to it
agentis log                           # show commit log
```

## Config — switching LLM backends

Edit `.agentis/config`:

```
# Mock (default, no LLM needed)
llm.backend = mock

# Claude CLI (flat-rate, recommended)
llm.backend = cli
llm.command = claude
llm.args = -p --output-format text

# Ollama (local, free)
llm.backend = cli
llm.command = ollama
llm.args = run llama3

# Anthropic API (per-token)
llm.backend = http
llm.endpoint = https://api.anthropic.com/v1/messages
llm.model = claude-sonnet-4-20250514
llm.api_key_env = ANTHROPIC_API_KEY
```

## Testing

```bash
agentis test examples/test-suite.ag              # run tests
agentis test examples/test-suite.ag --verbose    # verbose output
agentis test examples/ --fail-fast               # stop on first failure
agentis go examples/classify.ag --fitness        # single-file fitness report
```

## Evolution pipeline

The evolution engine mutates agent prompt instructions to find higher-scoring
variants. Fitness = CB efficiency + validate pass rate + explore survival rate.

```bash
# 1. Inspect agents in a program
agentis mutate examples/evolve-seed.ag --list-agents

# 2. Preview mutations without writing files
agentis mutate examples/evolve-seed.ag --dry-run

# 3. Generate mutated variants
agentis mutate examples/evolve-seed.ag --count 5 --out variants/

# 4. Compare variants in the arena
agentis arena variants/                           # rank by fitness
agentis arena variants/ --rounds 3 --top 3        # 3 rounds, top 3
agentis arena variants/ --json                    # JSON output

# 5. Full evolution loop
agentis evolve examples/evolve-seed.ag -g 10 -n 8
agentis evolve examples/evolve-seed.ag -g 10 -n 8 --show-lineage
agentis evolve examples/evolve-seed.ag -g 20 -n 8 --stop-on-stall 5

# 6. Trace lineage of the best variant
agentis lineage evolved/evolve-seed-best.ag
```

## Library management

Elite variants are stored in a persistent library that survives across runs.

```bash
agentis lib add evolved/evolve-seed-best.ag --tag "v1"
agentis lib list
agentis lib search "sentiment"
agentis lib show v1
agentis lib tags

# Export/import for sharing across machines
agentis lib export --out bundle.alib --all
agentis lib import bundle.alib --skip-duplicates
```

## Evolution with library warm-start

```bash
# Seed from library entries
agentis evolve seed.ag -g 10 -n 8 --seed-from-lib "sentiment"
agentis evolve seed.ag -g 10 -n 8 --seed-from-lib "tag:v1" --seed-top-k 3

# Warm-start: inject library variants with probability
agentis evolve seed.ag -g 20 -n 8 --seed-from-lib "tag:v1" --warm-start-prob 0.5
agentis evolve seed.ag -g 20 -n 8 --warm-start-prob 0.7 --warm-start-decay 0.1

# Adaptive budget allocation across lineages
agentis evolve seed.ag -g 20 -n 8 --adaptive-budget
```

## Checkpoints and resume

```bash
# Evolution auto-checkpoints every generation
agentis evolve seed.ag -g 50 -n 8 --tag "experiment-1"

# Resume from checkpoint
agentis evolve seed.ag -g 50 -n 8 --resume experiment-1

# Inspect checkpoint history
agentis colony tags
agentis colony history
agentis colony best --min-score 0.8
```

## Distributed colony

```bash
# Start worker nodes on remote machines
agentis worker 0.0.0.0:9462 --secret mykey

# Run arena across workers
agentis arena variants/ --workers host1:9462,host2:9462 --secret mykey

# Evolve across colony
agentis evolve seed.ag -g 20 -n 8 --workers host1:9462,host2:9462 --secret mykey

# Health check
agentis colony status --workers host1:9462,host2:9462
agentis colony ping host1:9462
```

## PII protection and audit

```bash
agentis init --secure                    # locked-down: PII denied, audit on
agentis go program.ag --grant-pii        # explicitly allow PII in prompts
agentis audit                            # show audit log
agentis audit --last 10 --pii-only       # recent PII events
agentis audit --agent analyzer --blocked # blocked prompts for an agent
```

## Event hooks

Configure in `.agentis/config`:

```
hooks.on_new_best = checkpoint tag=improved lib_add
hooks.on_stagnation = reduce_budget 0.3
hooks.on_crash = log variant crashed
hooks.on_validation_fail = skip
```

Actions: `checkpoint`, `tag=<name>`, `lib_add`, `log <msg>`,
`reduce_budget <frac>`, `inject_library <count>`, `skip`.
