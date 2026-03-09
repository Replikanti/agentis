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
| 3 | `classify.ag` | 1 | Typed struct output + validation |
| 4 | `pipeline.ag` | 2 | Data pipeline — LLM as data processor |
| 5 | `parallel.ag` | 3 | Multi-agent spawn/await |
| 6 | `explore.ag` | 2 | Evolutionary branching — survive or die |

## Notes

- `pipeline.ag` reads `data.txt` via `file_read()`. Copy `examples/data.txt`
  to `.agentis/sandbox/data.txt` before running.
- `explore.ag` creates VCS branches. Run `agentis branch` after to see which
  approaches survived validation.
- Use `--trace` for verbose output: `agentis go examples/classify.ag --trace`
