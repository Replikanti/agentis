# Agentis VCS Model

## Code as DAG

In Agentis, code is not stored as text files. Source `.ag` files are a
**transient input format** — they exist only long enough to be parsed and
committed. The canonical representation is a binary AST stored as a
content-addressed DAG.

```
source.ag → lexer → parser → AST → binary serialization → SHA-256 → object store
```

After `agentis commit`, the `.ag` file is irrelevant. The code lives in
`.agentis/objects/` as hashed binary blobs.

## Content-Addressed Storage

Every AST node is serialized to a stable binary format and stored under its
SHA-256 hash. Objects live in `.agentis/objects/<first-2-chars>/<rest>` (Git-style
fanout for filesystem efficiency).

Properties:
- **Deterministic.** Same AST → same bytes → same hash. Always.
- **Idempotent.** Storing the same content twice is a no-op (file already exists).
- **Integrity-verified.** Every read recomputes the hash and compares. Corruption
  is detected immediately.
- **Deduplication.** Identical subtrees share storage automatically.

```
.agentis/
  objects/
    ab/cdef1234...    # binary AST blob
    f0/9e8d7c6b...    # another blob
  refs/
    heads/
      genesis         # file containing commit hash
      feature-x       # another branch
  HEAD                # file containing current branch name
  config              # LLM backend + trace settings
  sandbox/            # sandboxed I/O directory
```

## Commits

A commit is a binary object containing three fields:

| Field | Type | Description |
|-------|------|-------------|
| `tree_hash` | SHA-256 hex | Hash of the program AST |
| `parent` | Option<SHA-256 hex> | Hash of the parent commit (None for first) |
| `timestamp` | u64 | Unix seconds |

Commits form a singly-linked chain (newest → oldest). The hash of the commit
object itself is what branches point to.

```
commit C3 → commit C2 → commit C1 → ∅
  │              │            │
  tree_hash      tree_hash    tree_hash
  │              │            │
  program v3     program v2   program v1
```

## Branches

A branch is a file in `.agentis/refs/heads/<name>` containing a commit hash.
HEAD is a file containing the current branch name (not a hash — it's a
symbolic reference).

- **Genesis** is the default branch, created by `agentis init`. There is no
  `main` or `master`.
- `agentis branch <name>` creates a new branch at the current commit.
- `agentis switch <name>` changes HEAD.
- `agentis log` walks the commit chain from the current branch's tip.

## Explore Creates Branches

The `explore "name" { ... }` construct is the only way code creates branches
at runtime:

1. Save the entire evaluator state (environment, budget, output).
2. Execute the body in isolation.
3. **Success:** Create branch `name` at the current commit. State is committed.
4. **Failure:** Restore the saved state. No branch, no side effects.

This is natural selection — multiple explore blocks propose solutions, only
those that pass validation survive as branches. Check survivors with
`agentis branch`.

## Import by Hash

```
import "abc123def456...";
```

`import` loads a program by its content hash from the object store. This is
the module system — there are no filenames, no paths, no package registries.
You import a specific, immutable, verified piece of code.

Import variants:
- `import "hash";` — import all declarations
- `import "hash" as utils;` — namespaced (`utils.func_name`)
- `import "hash" { func1, Type1 };` — selective import

Cycle detection prevents infinite import loops.

## Why No Merge Conflicts

Traditional VCS operates on text lines. Merging two edits to the same line
creates a conflict.

Agentis operates on AST nodes. Each node is a self-contained, hashed object.
Two programs that define the same function with different implementations
simply have different hashes — they are different objects in the store. There
is no concept of "the same line changed in two branches."

The unit of identity is the content hash, not the file position.

## P2P Sync

Repositories can sync over TCP using a HAVE/WANT/DATA/DONE protocol:

1. **HAVE** — sender lists all object hashes it has.
2. **WANT** — receiver requests hashes it's missing.
3. **DATA** — sender transmits the requested objects.
4. **DONE** — sync complete.

All messages are length-prefixed binary. Hash lists use 64-byte hex strings.
Object data includes hash + length + raw bytes for integrity verification on
the receiving end.

## CLI Commands

```bash
agentis init                  # Create .agentis/ with genesis branch
agentis commit <file.ag>      # Parse, store AST, update current branch
agentis run <branch>          # Execute code from a branch
agentis go <file.ag>          # commit + run in one step (demo command)
agentis go <file.ag> --trace  # same, with verbose trace output
agentis branch                # List all branches (* = current)
agentis branch <name>         # Create new branch
agentis switch <name>         # Switch to a branch
agentis log                   # Show commit history
agentis doctor                # Pre-flight environment check
```
