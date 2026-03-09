# Agentis Phase 2: Cloud-Native Runtime

## Vision

Taking Agentis from a local sandbox to a zero-dependency, cloud-native runtime.
Compile AST directly to WebAssembly (WASM), implement Capability-Based Security
(OCap) for all side-effects, and introduce Orthogonal Persistence (State-as-Memory).

## Tech Stack Rules

- Language: Rust
- Dependencies: `sha2` (existing) + `wasm-encoder` (WASM binary generation)
- No heavy runtimes (no wasmtime, no wasmer, no serde)
- Storage: Content-addressed `.agentis/objects/` (expanding to memory snapshots)

## Milestones

- [x] M8: WASM Compiler Backend (DAG-to-WASM & CB Injection)
- [x] M7: Raw TCP Peer-to-Peer Sync
- [x] M9: Capability-Based Security (OCap)
- [x] M10: Orthogonal Persistence (State-as-Memory)

### M7: Raw TCP Peer-to-Peer Sync

Binary sync protocol: "Which hashes do you have? I need these."
Deliverable: `network.rs`

### M8: WASM Compiler Backend

Compile content-addressed AST nodes into WebAssembly binary format.

**DAG Mapping:** Each unique AST hash → distinct WASM function.
Preserves structural deduplication.

**Cognitive Budget Injection:** CB checks injected directly into generated WASM
code (decrementing a mutable i64 global). No host calls for basic checks.

Cost table:
| Operation              | Cost |
|------------------------|------|
| Math / literal / var   | 1    |
| Function call          | 5    |
| Loop iteration         | 1    |
| Memory allocation      | dynamic |
| Default                | 1    |

WASM Module exports:
- `run` function: () -> i32 (0 = success, trap = CB exceeded)
- `cb_remaining` global: mutable i64

### M9: Capability-Based Security (OCap)

Remove all implicit access to I/O, network, or filesystem.
Strict cryptographic Capability (Handle) system via WASM host imports.
Deliverable: `capabilities.rs`

### M10: Orthogonal Persistence (State-as-Memory)

Continuous memory snapshotting of WASM linear memory.
**Transaction Boundary rule:** Snapshots ONLY occur when call stack is completely
empty (after top-level operation or explore block returns).
Deliverable: `snapshot.rs`
