# Phase 5: Data Guardians

## Vision

Phases 1–4 built the engine and made it visible. Phase 5 makes it **safe**.
Agentis handles real data — personal records, financial transactions, medical
notes. The runtime must prevent accidental PII leakage to external LLMs without
adding stdlib functions or breaking "everything is prompt."

No new builtins. No new language syntax. PII protection is implemented as:
- A new capability (`PiiTransmit`) in the existing capability system
- An internal guard in `eval_prompt` (never exposed to user code)
- An audit trail for compliance and debugging
- A secure config template

## Design Principle: Guards, Not Functions

`redact_pii()` is stdlib under a different name. If we add it, next comes
`mask_iban()`, then `anonymize_medical()`, and we have a stdlib. The guard
approach is different: the runtime **blocks** PII from reaching the LLM unless
explicitly permitted. The agent never calls a sanitizer builtin — the runtime
enforces the boundary invisibly.

If an agent needs to sanitize data before prompting, it uses another agent
with a prompt. Everything is still a prompt.

```
// The Agentis way: sanitize via prompt, not via builtin
agent sanitize(raw: string) -> string {
    cb 15;
    return prompt("Remove all personal identifiers, preserve meaning", raw) -> string;
}

agent analyzer(data: string) -> Report {
    cb 300;
    let clean = sanitize(data);
    return prompt("Analyze this text", clean) -> Report;
}
```

The sanitizer agent runs against whichever LLM backend is configured. For
production with sensitive data, configure a local model (Ollama). For dev/test,
mock backend works. No special syntax, no config overrides in agent bodies.

## Milestones

- [x] M19: PiiTransmit Capability + Internal Guard
- [x] M20: Audit Log
- [x] M21: `agentis audit` CLI
- [ ] M22: `agentis init --secure`

### M19: PiiTransmit Capability + Internal Guard

Deliverable: changes to `capabilities.rs`, `evaluator.rs`, new `pii.rs`

**Part A — New capability: `PiiTransmit`**

Add `PiiTransmit` to `CapKind`. Defaulting behavior:

- `evaluator.grant_all()` does NOT include `PiiTransmit` — it must be
  explicitly granted. This is the only capability excluded from `grant_all`.
- CLI flag: `agentis go file.ag --grant-pii` explicitly grants it.
- Config: `pii_transmit = allow` in `.agentis/config` grants it for all runs.

**Part B — Internal PII scanner (`pii.rs`)**

A Rust module with a single public function:

```rust
pub fn scan(text: &str) -> PiiScanResult {
    // Returns list of detected PII types (email, phone, card, etc.)
}
```

Pattern matching (not exposed to user code):

| Pattern | Regex | Example |
|---------|-------|---------|
| Email | `[\w.+-]+@[\w.-]+\.\w{2,}` | `user@example.com` |
| Phone (intl) | `\+?\d[\d\s\-]{7,14}\d` | `+420 123 456 789` |
| Credit card | `\b\d{4}[\s-]?\d{4}[\s-]?\d{4}[\s-]?\d{4}\b` | `4111-1111-1111-1111` |
| Czech rodné číslo | `\b\d{2}[0-7]\d[0-3]\d/?\d{3,4}\b` | `900101/1234` |
| IBAN | `\b[A-Z]{2}\d{2}\s?[\dA-Z]{4}[\s\d A-Z]{6,30}\b` | `CZ65 0800 0000 ...` |
| IPv4 | `\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b` | `192.168.1.1` |
| SSN (US) | `\b\d{3}-\d{2}-\d{4}\b` | `123-45-6789` |

This is intentionally simple. It will have false positives (a phone-like
number that isn't a phone). That's acceptable — the guard errs on the side
of caution. The user can grant `PiiTransmit` to bypass.

**Part C — Guard in `eval_prompt`**

Before calling the LLM backend:

1. Run `pii::scan()` on the input string.
2. If PII detected AND agent lacks `PiiTransmit` capability:
   → `EvalError::CapabilityDenied("potential PII detected: email, phone")`
3. If PII detected AND agent HAS `PiiTransmit`:
   → proceed, log to audit (M20).
4. If no PII detected → proceed normally.

**CB cost:** Zero for the scan. It's a guard, not computation.

**Trace output:**

```
[pii] scan: 2 patterns detected (email, phone)
[pii] PiiTransmit granted — proceeding
```

or:

```
[pii] scan: 1 pattern detected (credit_card)
[pii] BLOCKED — PiiTransmit not granted
```

### M20: Audit Log

Deliverable: new `audit.rs`, changes to `evaluator.rs`, `main.rs`

Every `prompt()` call writes a JSONL entry to `.agentis/audit/prompts.jsonl`:

```json
{
  "ts": 1710000000,
  "agent": "analyzer",
  "instruction_hash": "abc123...",
  "input_hash": "def456...",
  "input_len": 1524,
  "pii_scan": "clean",
  "pii_types": [],
  "pii_transmit_granted": false,
  "backend": "cli",
  "model": "claude"
}
```

For PII-detected prompts:

```json
{
  "ts": 1710000001,
  "agent": "sanitize",
  "instruction_hash": "789abc...",
  "input_hash": "012def...",
  "input_len": 842,
  "pii_scan": "detected",
  "pii_types": ["email", "phone"],
  "pii_transmit_granted": true,
  "backend": "cli",
  "model": "claude"
}
```

**Implementation:**
- `Audit` struct holds a file handle to the JSONL file.
- Passed to `Evaluator` like `Tracer` (optional `&'a Audit`).
- Uses our existing `json.rs` module for serialization. No new dependencies.
- Audit is opt-in: enabled when `.agentis/audit/` directory exists.
  `agentis init --secure` creates it. Regular `init` does not.

### M21: `agentis audit` CLI

Deliverable: changes to `main.rs`, new `cmd_audit()`

```bash
agentis audit                     # show last 50 entries
agentis audit --last 100          # show last N entries
agentis audit --pii-only          # only entries with PII detected
agentis audit --agent scanner     # filter by agent name
agentis audit --blocked           # only entries where PiiTransmit was denied
```

Output format (human-readable table):

```
TIME        AGENT       PII          STATUS    BACKEND
19:04:32    sanitize    email,phone  GRANTED   cli/claude
19:04:33    analyzer    clean        —         cli/claude
19:04:35    scanner     credit_card  BLOCKED   —
```

No new dependencies. Plain `println!` formatting. Reads JSONL line by line,
parses with `json.rs`, filters, formats.

### M22: `agentis init --secure`

Deliverable: changes to `main.rs`

`agentis init --secure` creates the standard `.agentis/` structure plus:

1. **Config with security defaults:**
```
llm.backend = mock

# PII Protection (Phase 5: Data Guardians)
# PiiTransmit is DENIED by default. To allow PII in prompts:
# pii_transmit = allow
pii_transmit = deny

# Audit logging (enabled — all prompts are logged)
audit = on

trace.level = normal
```

2. **Creates `.agentis/audit/` directory** (enables audit logging).

3. **Prints security summary:**
```
Initialized secure Agentis repository with genesis branch.
  PII guard:  ON (PiiTransmit denied by default)
  Audit log:  ON (.agentis/audit/)
  LLM:        mock (configure in .agentis/config)
```

## Implementation Order

1. **M19** — capability + scanner + guard (core protection mechanism)
2. **M20** — audit log (needs M19 PII scan results)
3. **M21** — CLI for audit (needs M20 log format)
4. **M22** — secure init template (needs M19+M20 config keys)

## Success Criteria

Phase 5 is complete when:
1. `prompt()` with PII in input is blocked by default (no `PiiTransmit`)
2. `--grant-pii` flag explicitly allows PII transmission
3. Every prompt call is logged to JSONL audit trail
4. `agentis audit --pii-only` shows all PII-related prompt calls
5. `agentis init --secure` creates a locked-down configuration
6. Zero new builtins. Zero new language syntax. "Everything is prompt" intact.

## What Phase 5 Does NOT Include

- **No `redact_pii()` builtin.** Use a prompt.
- **No per-agent LLM backend override.** Use the global config.
- **No `grant` statement in agent bodies.** Capabilities are granted by the
  runtime (CLI flags, config), not by agent code.
- **No PII detection in LLM output.** Phase 5 guards the input only. Output
  scanning is a future consideration.
