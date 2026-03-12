# Phase 8: Distributed Colony

## Vision

Phases 1--7 built a complete single-instance evolution pipeline: fitness
scoring, mutation, arena competition, and generational selection. Phase 8
makes it **fast**. Agentis agents evolve across a colony of worker nodes,
turning sequential arena evaluation into a parallel, distributed process.

No new builtins. No new language syntax. No new crate dependencies. The
colony is a CLI-level coordination layer that distributes existing
evaluation work across threads and TCP-connected workers.

## Design Principle: Source Travels, State Stays

Workers are stateless evaluators. They receive `.ag` source code, run it,
and return a fitness report. No VCS state, no lineage tracking, no
evolution logic lives on workers. The coordinator (the machine running
`agentis evolve` or `agentis arena`) owns all state: object store,
fitness registry, lineage files, generation management.

This is deliberately simple. Workers don't need `.agentis/` directories,
don't need to agree on branch state, and can be added or removed between
generations without protocol negotiation. A worker is just an evaluation
endpoint.

Workers use their own LLM configuration. This enables heterogeneous
colonies: one worker runs a local Ollama instance, another hits an API
endpoint, a third uses mock backend for testing. The coordinator doesn't
know or care which backend a worker uses — it only sees fitness scores.

## Authentication

Shared secret authentication using `SHA-256(secret.as_bytes())`. The
secret is configured via `--secret <hex>` on the CLI or `colony.secret`
in `.agentis/config`.

Auth flow: connect -> if secret configured: send AUTH -> read
AUTH_OK/AUTH_FAIL -> proceed or abort. If no secret is configured on
either side: skip AUTH entirely. This keeps local development
frictionless while allowing production colonies to gate access.

No TLS, no certificate management, no key exchange. The shared secret
prevents unauthorized workers from joining but does not encrypt traffic.
Encrypted transport is out of scope — colonies are expected to run on
trusted networks or behind VPNs.

## Payload Limits

Practical limits to prevent abuse and memory exhaustion:

- **Source:** <= 1 MB. Workers reject oversized payloads before reading
  the source body. Coordinator rejects source files > 1 MB before
  sending.
- **Output/error:** Truncated to 4 KB **on the worker before encoding**.
  This ensures the RESULT message stays bounded regardless of how verbose
  the program's output is.

## Distribution Strategy

Round-robin assignment of variants to workers. Simple, fair, predictable.
If a colony has 3 workers and 9 variants, each worker gets 3 evaluations.

No work stealing, no load-aware scheduling, no task queues. Smart
scheduling adds complexity that isn't justified until colonies are large
enough to have heterogeneous performance characteristics. Round-robin
is correct for the common case: a handful of similar machines.

## Graceful Fallback

Worker failure is not fatal. When a worker fails, the evaluation falls
back to local execution with a warning that includes the reason:

```
Warning: Worker 10.0.0.2:9462 unreachable (connection refused), falling back to local
Warning: Worker 10.0.0.3:9462 auth failed, falling back to local
Warning: Worker 10.0.0.4:9462 timed out after 120s, falling back to local
Warning: Worker 10.0.0.5:9462 protocol error (unexpected msg type 0x03), falling back to local
```

This means a colony run always produces results — degraded to local
speed in the worst case, but never failing due to worker issues.

## Protocol Extension

The colony protocol extends the existing sync protocol from `network.rs`.
Same framing: `[u8 msg_type][u32LE payload_len][payload...]`.

New message types:

```
EVAL(0x05):    [u32 request_id][u32 source_len][source <=1MB][u64 budget]
               [u32 weights_len][weights_str][u32 filename_len][filename]
               [u8 grant_pii][u64 timeout_ms]

RESULT(0x06):  [u32 request_id][u8 status][f64 score][f64 cb_eff]
               [f64 val_rate][f64 exp_rate][u32 prompt_count]
               [u32 output_len][output <=4KB][u32 error_len][error <=4KB]
               [u64 eval_time_ms]
               All f64: IEEE 754 LE via f64::to_le_bytes()/from_le_bytes()

PING(0x07):    [u64 timestamp_ms]
PONG(0x08):    [u64 echo_timestamp_ms][u32 evals_completed][u32 evals_failed]
               [u64 avg_eval_ms][u8 busy]
               [u32 backend_len][backend_name]

AUTH(0x09):    [32 bytes SHA-256(secret.as_bytes())]
AUTH_OK(0x0A): empty
AUTH_FAIL(0x0B): empty -> close connection
```

The `request_id` field ties EVAL requests to RESULT responses, enabling
pipelined evaluation on workers that support `--max-concurrent`.

The PONG message includes `backend_name` (e.g., "mock", "cli", "http")
so the coordinator can display which LLM backend each worker uses —
essential for understanding heterogeneous colony behavior.

The `status` byte in RESULT: `0x00` = success, `0x01` = error (fitness
0.0, error message in error field), `0x02` = rejected (payload too
large, resource limit).

## Milestones

- [x] M31: Parallel Arena (local threads)
- [x] M32: Worker Node
- [ ] M33: Colony Arena
- [ ] M34: Colony Observability

### M31: Parallel Arena (local threads)

Deliverable: new `colony.rs`, changes to `arena.rs`, `llm.rs`, `main.rs`, `network.rs`

Run arena variants in parallel using local threads. This is the
foundation for both local speedup and remote distribution.

**Prerequisites:**

The `LlmBackend` trait must become `Send + Sync`. Currently, backends
are constructed once and used sequentially. For thread pool usage, the
trait object must be shareable across threads via `Arc<dyn LlmBackend>`.
MockBackend is already stateless. CliBackend spawns a subprocess per
call (no shared state). HttpBackend uses `ureq` which is sync and
thread-safe. All three backends should require minimal changes.

The `write_msg` and `read_msg` functions in `network.rs` must become
`pub` (currently crate-private) so `colony.rs` can use them for
protocol encoding/decoding.

**CLI:**

```bash
agentis arena variants/ --parallel              # auto-detect thread count
agentis arena variants/ --threads 4             # explicit thread count
agentis evolve file.ag -g 10 -n 8 --threads 4  # parallel arena within evolution
```

**Implementation:**

New `colony.rs` containing the thread pool:

```rust
pub struct LocalPool {
    threads: usize,
}

impl LocalPool {
    pub fn new(threads: usize) -> Self { ... }
    pub fn auto() -> Self {
        let n = std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(4);
        Self::new(n)
    }
    pub fn run_all(&self, tasks: Vec<EvalTask>) -> Vec<EvalResult> { ... }
}
```

The pool uses `std::sync::mpsc` channels: the coordinator sends
`EvalTask` values to a channel, N worker threads pull tasks, evaluate
them (parse -> commit -> evaluator -> fitness report), and send
`EvalResult` values back.

`EvalTask` and `EvalResult` structs:

```rust
pub struct EvalTask {
    pub id: u32,
    pub source: String,
    pub filename: String,
    pub budget: u64,
    pub weights: FitnessWeights,
    pub grant_pii: bool,
}

pub struct EvalResult {
    pub id: u32,
    pub score: f64,
    pub cb_efficiency: f64,
    pub validate_rate: f64,
    pub explore_rate: f64,
    pub prompt_count: u32,
    pub output: String,
    pub error: Option<String>,
    pub eval_time_ms: u64,
}
```

**Thread count auto-detection:**

`std::thread::available_parallelism()` returns the number of available
CPU cores. Fallback to 4 if the call fails (e.g., in containers with no
cgroup info). The `--threads N` flag overrides auto-detection.

**Determinism guarantee:**

With `--parallel`, the order of evaluation is non-deterministic, but
the final ranking is deterministic: sort by score descending, then by
filename for ties. The arena table output is identical whether variants
ran sequentially or in parallel.

**What parallel arena is NOT:**

- Not distributed (local threads only — remote workers are M33).
- Not async (uses `std::thread` + `mpsc`, no Tokio).

### M32: Worker Node

Deliverable: changes to `colony.rs`, `main.rs`

A standalone TCP server that accepts evaluation requests, runs `.ag`
programs, and returns fitness results.

**CLI:**

```bash
agentis worker                                   # default 0.0.0.0:9462
agentis worker 0.0.0.0:9500                      # custom bind address
agentis worker --secret abc123                    # require auth
agentis worker --max-concurrent 4                 # pipeline parallelism
agentis worker --max-connections 4                # global connection limit
```

**Output (stderr):**

```
Worker listening on 0.0.0.0:9462 (max-concurrent: 4, max-connections: 4)
[2026-03-12 14:30:01] EVAL #1 classify-m1.ag -> 0.815 (142ms)
[2026-03-12 14:30:01] EVAL #2 classify-m2.ag -> 0.000 (error: CognitiveOverload) (89ms)
[2026-03-12 14:30:02] EVAL #3 classify-m3.ag -> 0.910 (201ms)
```

**Implementation:**

The worker server loop:

1. Bind TCP listener on the configured address.
2. Accept connections (up to `--max-connections`, default 4).
3. Per connection: spawn a handler thread.
4. Handler: if secret configured, perform AUTH handshake. On AUTH_FAIL,
   close connection.
5. Read messages in a loop. Handle EVAL, PING. Ignore unknown types.
6. On EVAL: validate payload (source <= 1 MB). Parse source, create
   evaluator with the worker's own LLM config, run, collect fitness.
   Truncate output and error to 4 KB. Send RESULT.
7. On PING: reply with PONG including stats (evals completed, failed,
   average eval time, busy flag, backend name).

**Pipeline parallelism:**

With `--max-concurrent N`, the worker reuses the `LocalPool` from M31
to run multiple evaluations in parallel within a single connection.
Without `--max-concurrent`, evaluations on a connection run sequentially.

**Payload validation:**

Source payloads larger than 1 MB are rejected immediately: the worker
reads the EVAL header, checks `source_len`, and if > 1 MB, sends a
RESULT with status `0x02` (rejected) without reading the source body.
This prevents memory exhaustion from malicious or buggy coordinators.

**Worker LLM config:**

Workers read their own `.agentis/config` (or use defaults). They do not
receive LLM configuration from the coordinator. This is intentional:
workers may have local models, different API keys, or different rate
limits. The coordinator only sees the fitness score — it doesn't need to
know how the score was computed.

**Graceful shutdown:**

Ctrl+C (SIGINT) sets a shutdown flag. The worker finishes in-progress
evaluations, stops accepting new connections, and exits cleanly.

### M33: Colony Arena

Deliverable: changes to `colony.rs`, `arena.rs`, `main.rs`

Distribute arena evaluations across remote workers. The coordinator
sends EVAL requests, collects RESULT responses, and ranks variants
as if they ran locally.

**CLI:**

```bash
agentis arena variants/ --workers 10.0.0.2:9462,10.0.0.3:9462
agentis arena variants/ --workers workers.txt     # file with one addr:port per line
agentis arena variants/ --workers w1,w2 --secret abc123
agentis evolve file.ag -g 10 -n 8 --workers w1,w2
```

**Configuration:**

Workers can be specified via:
- `--workers addr1:port,addr2:port` on the CLI
- `--workers path/to/workers.txt` (if the path exists as a file, read
  it — one `addr:port` per line, blank lines and `#` comments ignored)
- `colony.workers = addr1:port,addr2:port` in `.agentis/config`

The `--secret` flag or `colony.secret` config provides the auth secret.

**Timeouts:**

- Connect timeout: 5 seconds (configurable via `colony.connect_timeout`
  in config).
- Read/write timeout: 120 seconds (configurable via `colony.eval_timeout`
  in config).

**Distribution:**

Round-robin assignment: variant 0 goes to worker 0, variant 1 to
worker 1, etc., wrapping around. The coordinator opens persistent TCP
connections to all workers at the start of an arena run and reuses them
across evaluations.

**Fallback with reason:**

When a worker fails, the coordinator logs a warning with the specific
failure reason and evaluates that variant locally:

```
Warning: Worker 10.0.0.2:9462 unreachable (connection refused), falling back to local
```

Failure reasons:
- "connection refused" — TCP connect failed
- "connection timed out" — connect timeout exceeded
- "auth failed" — AUTH_FAIL received
- "timed out after {N}s" — read/write timeout exceeded
- "protocol error ({detail})" — unexpected message type or malformed response

The fallback is per-variant, not per-worker. If worker A fails on
variant 3, only variant 3 falls back to local. Subsequent variants
assigned to worker A will attempt the connection again (the worker may
have recovered).

**JSON output with colony info:**

When `--json` is used with `--workers`, each entry includes the worker
address:

```json
[{"rank":1,"file":"variant-c.ag","score":0.915,"cb_eff":0.98,"val_rate":1.0,"exp_rate":0.67,"prompt_count":3,"error":null,"worker":"10.0.0.2:9462","eval_time_ms":142},
 {"rank":2,"file":"variant-a.ag","score":0.815,"cb_eff":0.95,"val_rate":1.0,"exp_rate":0.50,"prompt_count":4,"error":null,"worker":"local","eval_time_ms":89}]
```

Variants that fell back to local show `"worker":"local"`.

**Colony stats line:**

After the arena table, a summary line is printed:

```
Colony: 3 workers, 2 local fallbacks, avg eval 156ms
```

### M34: Colony Observability

Deliverable: changes to `colony.rs`, `main.rs`

Diagnostic tools for inspecting colony health and worker status.

**CLI:**

```bash
agentis colony status --workers 10.0.0.2:9462,10.0.0.3:9462
agentis colony status --workers workers.txt --json
agentis colony ping 10.0.0.2:9462
```

**`agentis colony status` output:**

```
Colony Status: 3 workers

WORKER              STATUS   EVALS  FAILED  AVG_MS  BUSY  BACKEND
10.0.0.2:9462       online     142       3    156ms  no    http
10.0.0.3:9462       online      98       1    201ms  yes   cli
10.0.0.4:9462       offline     --      --      --   --    --

Summary: 2/3 online, 240 evals completed, 4 failed, avg 175ms
```

**Implementation:**

For each worker, the coordinator:
1. Opens a TCP connection (with connect timeout).
2. Performs AUTH handshake if secret configured.
3. Sends PING with current timestamp.
4. Reads PONG response.
5. Displays worker stats in table format.

Workers that fail to connect show status "offline" with dashes for all
metrics. Auth failures show status "auth-fail".

**`agentis colony ping` output:**

```
Pinging 10.0.0.2:9462...
  Status:     online
  Evals:      142 completed, 3 failed
  Avg eval:   156ms
  Busy:       no
  Backend:    http
  Latency:    12ms
```

The latency is measured as round-trip time of the PING/PONG exchange.

**JSON output:**

`--json` on `colony status` outputs machine-readable JSON:

```json
[{"worker":"10.0.0.2:9462","status":"online","evals_completed":142,"evals_failed":3,"avg_eval_ms":156,"busy":false,"backend":"http","latency_ms":12},
 {"worker":"10.0.0.4:9462","status":"offline","evals_completed":null,"evals_failed":null,"avg_eval_ms":null,"busy":null,"backend":null,"latency_ms":null}]
```

**Colony summary in evolve:**

When `agentis evolve` runs with `--workers`, a colony summary is
appended to the final evolution output:

```
Evolution: classify.ag
  Population: 8, Generations: 10, Workers: 3

Gen  1: best=0.815  avg=0.542  prompts=3.5  (8 variants, 2 local fallbacks)
...
Gen 10: best=0.935  avg=0.812  prompts=2.4  (8 variants)

Best agent: evolved/classify-g10-best.ag (score: 0.935)

Colony summary: 3 workers, 80 evals distributed, 6 local fallbacks, avg eval 148ms
```

## Files Changed

| File | Change |
|------|--------|
| `src/colony.rs` | **New.** Thread pool (`LocalPool`), `EvalTask`/`EvalResult` types, worker server loop, colony coordinator (round-robin distribution, fallback logic), protocol encoding/decoding for EVAL/RESULT/PING/PONG/AUTH messages, colony status/ping commands |
| `src/network.rs` | New MSG constants `MSG_EVAL(0x05)`, `MSG_RESULT(0x06)`, `MSG_PING(0x07)`, `MSG_PONG(0x08)`, `MSG_AUTH(0x09)`, `MSG_AUTH_OK(0x0A)`, `MSG_AUTH_FAIL(0x0B)`. Make `write_msg` and `read_msg` `pub` |
| `src/main.rs` | New `worker` command, new `colony` command (with `status` and `ping` subcommands). New flags: `--parallel`, `--threads N`, `--workers`, `--secret`. Wire flags into `arena` and `evolve` commands |
| `src/arena.rs` | `EvalResult` to `ArenaEntry` conversion. Colony info fields (`worker` address, `eval_time_ms`) in arena entries. Colony stats line formatter. JSON entries include `worker` and `eval_time_ms` when running in colony mode |
| `src/llm.rs` | Add `Send + Sync` bounds to `LlmBackend` trait: `pub trait LlmBackend: Send + Sync` |
| `docs/phase8-plan.md` | **New.** This document |

## Implementation Order

1. **M31** -- parallel arena (thread pool foundation)
2. **M32** -- worker node (server before client)
3. **M33** -- colony arena (distributed evaluation)
4. **M34** -- colony observability (needs M32 + M33 to be meaningful)

M31 first: the `LocalPool` is reused by M32 for `--max-concurrent`
pipeline parallelism on workers. M32 before M33: the worker server must
exist before the coordinator can connect to it. M34 last: diagnostics
are only useful once the colony infrastructure exists.

## Constraints

- **Zero new crate dependencies.** `std::thread`, `std::sync::mpsc`,
  `std::net` are sufficient. No thread pool crates, no async runtimes.
- **Zero new syntax or builtins.** Colony is infrastructure, not language.
- **Zero async/Tokio.** `std::thread` + blocking I/O throughout.
- **No serde.** Protocol is hand-rolled binary, JSON output uses `json.rs`.
- **All 529 existing tests pass.** Target ~570+ with colony tests.

## What Phase 8 Does NOT Include

- **No automatic worker discovery.** Workers are manually registered via
  `--workers` or config. No multicast, no service mesh, no DNS-based
  discovery.
- **No work stealing or smart scheduling.** Round-robin only. Load-aware
  scheduling requires profiling worker performance over time — deferred.
- **No encrypted transport.** Shared secret auth only, no TLS. Colonies
  run on trusted networks.
- **No persistent worker state.** Workers are stateless. No caching of
  previously evaluated variants, no local object stores.
- **No cross-colony evolution.** Single coordinator, single evolution
  run. Federating multiple coordinators is a different problem.
- **No WASM sandboxing on workers.** Workers run `.ag` source with the
  same trust model as local execution. Untrusted code requires external
  sandboxing (containers, VMs).

## Success Criteria

Phase 8 is complete when:
1. `agentis arena dir/ --parallel` runs variants in parallel locally
   with identical rankings to sequential
2. `agentis worker` serves evaluation requests over TCP
3. `agentis arena dir/ --workers w1,w2` distributes work across colony
4. `agentis evolve ... --workers w1,w2` runs distributed evolution
5. Worker failure falls back to local evaluation with warning
6. `agentis colony status --workers w1,w2` shows worker health
7. Auth handshake prevents unauthorized workers
8. Zero new deps, zero async, all existing tests pass
