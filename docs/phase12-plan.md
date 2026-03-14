# Phase 12 — Portable Agent Identity

**Codename:** The Vessel
**Version target:** v0.8.0
**Theme:** Agents that survive across machines, restarts, and upgrades.

---

## Motivation

After Phase 11, agents have memory (memo store), introspection, and lineage
awareness — but all tied to a single `.agentis/` directory. There's no way to:

- Package an agent's complete identity (code + memos + checkpoint + lineage)
  into a portable bundle.
- Resume evolution on a different machine.
- Verify bundle integrity after transfer.
- Auto-backup elite variants as portable bundles.

Phase 12 creates a unified **identity hash** and **portable bundle format**
(`.agb`) that ties all existing persistence into a single transferable artifact.

---

## Milestones

- [x] M49: Identity Hash + `agentis identity hash`
- [x] M50: Bundle Format + `agentis export`
- [x] M51: `agentis import` + `--resume-from`
- [x] M52: Auto-Backup Hook + `--backup-to`
- [x] M53: Identity CLI Polish + Docs

### M49: Identity Hash + `agentis identity hash`

**Goal:** Deterministic identity fingerprint from (seed_hash, generation,
lineage chain).

**New file: `src/identity.rs`**
- `compute_identity_hash(seed_hash, generation, chain: &[String]) -> String`
  - `SHA-256(b"AGID" || seed_hash_bytes || generation_u32_le || chain_hash_0 || ...)`
  - Salt prefix `b"AGID"` prevents collision with other SHA-256 uses
  - Empty chain → uses `[seed_hash]`
- `identity_from_checkpoint(ckpt_hash, store) -> Result<String>` — walks
  parent chain, collects hashes. Fallback for old checkpoints without lineage.
- `identity_from_seed(seed_hash) -> String` — convenience for gen 0.

**Evaluator changes (`src/evaluator.rs`):**
- `identity_hash: String` field added to `IntrospectContext` (default `""`)
- `inject_introspect()`: inserted as `Value::String` — free access (0 CB)

**CLI (`src/main.rs`):**
- `agentis identity hash` — compute from HEAD checkpoint
- `agentis identity hash <file.ag>` — compute seed-only identity

**Tests (6):** deterministic, different-gen changes hash, different-chain
changes hash, seed-only, from-checkpoint, introspect access in .ag code.

---

### M50: Bundle Format + `agentis export`

**Goal:** Define `AGBu` binary bundle format and implement `agentis export`.

**New file: `src/bundle.rs`**

Wire format:
```
[4B "AGBu"][1B version=1]
[1B type][4B len][data...]   ← tagged sections, extensible
  0x01 IDENTITY   — seed_hash, generation, identity_hash, version, tags, timestamp
  0x02 SEED       — original .ag source code
  0x03 CHECKPOINT — raw AGCK binary (optional)
  0x04 MEMO       — key_count + per key: name + jsonl content (optional)
  0x05 LINEAGE    — file_count + per file: name + content (optional)
  0xFF ROOT_HASH  — SHA-256 of all preceding bytes (integrity)
```

Unknown section types are skipped (forward compat).

Functions:
- `write_bundle(path, identity, seed, ckpt_data, memos, lineage) -> Result`
- `read_bundle(path) -> Result<BundleContents>` — validates root hash
- `BundleContents { identity, seed_source, checkpoint_data, memos, lineage_data }`

**CLI (`src/main.rs`):**
- `agentis export --out agent.agb [--include-memos] [--tag stable] [--lineage-depth N]`
- Reads HEAD checkpoint → seed source from `best_ever_source` field
- Optionally collects `.agentis/memo/*.jsonl`
- Collects `.agentis/fitness/g*.jsonl` (optionally limited via `--lineage-depth`)
- Prints: identity hash, generation, file size

**Tests (6):** roundtrip, integrity-check (corrupt byte → fail), skip unknown
section, without memos, without checkpoint, identity section roundtrip.

---

### M51: `agentis import` + `--resume-from`

**Goal:** Restore state from a bundle and resume evolution from it.

**Bundle import (`src/bundle.rs`):**
- `import_to_store(contents, agentis_root) -> Result<ImportResult>`
  - Writes checkpoint to CheckpointStore, sets HEAD
  - Restores memos to `.agentis/memo/` (appends to existing)
  - Restores lineage JSONL to `.agentis/fitness/`
  - Returns `{ checkpoint_hash, memo_keys_restored, lineage_files_restored }`

**CLI (`src/main.rs`):**
- `agentis import agent.agb [--as run-name] [--memo-conflict skip|append|replace]`
  - `--memo-conflict` controls handling of existing memo keys
- `agentis evolve ... --resume-from agent.agb`
  - Imports bundle, then proceeds as `--resume <hash>`
  - Error if both `--resume` and `--resume-from` specified

**Tests (6):** restores checkpoint, restores memos, restores lineage, appends
existing memos, --as tag, full resume round-trip.

---

### M52: Auto-Backup Hook + `--backup-to`

**Goal:** Auto-export bundle on each new best during evolution.

**Hook action (`src/evolve.rs`):**
- `Backup(String)` variant added to `HookAction` enum
- `"backup /path"` parsed in `parse_hook_actions`

**CLI flag (`src/main.rs`):**
- `agentis evolve ... --backup-to <dir>`
- On `is_new_best`: writes `<dir>/g{gen:02}-best.agb` + `<dir>/latest.agb`
- Print: `  backup → <dir>/g05-best.agb (24 KB)`

**Config:**
- `hooks.on_new_best = backup /backups/my-agent`

**Shared helper:**
- `write_evolve_backup(dir, gen, seed_hash, best_source, ckpt_data, root, identity_hash, tags)`

**Tests (5):** parse hook, backup creates bundle, bundle is valid, overwrites
on new best, hook-triggered backup.

---

### M53: Identity CLI Polish + Docs

**Goal:** Complete CLI surface and update docs.

**CLI (`src/main.rs`):**
- `agentis identity show` — formatted identity card (hash, seed, generation,
  tags, drift risk hint)
- `agentis identity verify <file.agb>` — recompute root hash + identity,
  report PASS/FAIL
- `agentis identity diff <a.agb> <b.agb>` — compare: same seed?, generation
  delta, common ancestor

**Additional (`src/bundle.rs`):**
- `verify_bundle(path) -> Result<VerifyReport>`

**Docs:**
- Updated `CLAUDE.md` with Phase 12 features
- Updated `print_usage()` with new commands

**Tests (5):** show format, verify valid, verify corrupted, diff same seed,
diff different seed.

---

## File changes

```
src/identity.rs  — NEW: identity hash computation               ~80 lines
src/bundle.rs    — NEW: AGBu format, write/read/import/verify  ~410 lines
src/evaluator.rs — IntrospectContext.identity_hash               ~10 lines
src/evolve.rs    — HookAction::Backup, parse                    ~15 lines
src/main.rs      — identity/export/import CLI, --resume-from,
                   --backup-to, identity show/verify/diff       ~400 lines
CLAUDE.md        — Phase 12 documentation                          —
docs/phase12-plan.md — this document                               —
```

**Total:** ~1130 lines of Rust.

---

## Sequencing

```
M49 (identity hash) → M50 (bundle + export) → M51 (import + resume) → M52 (auto-backup) → M53 (polish)
     |                      |                       |                       |                   |
   1-2 hrs               3-4 hrs                 2-3 hrs                 2-3 hrs             1-2 hrs
```

---

## Non-goals

- **IPFS/Arweave** — external transports, not core. Achievable via hooks.
- **Budget Prediction / Confidence** — orthogonal features, separate phase.
- **New language primitives** — no `backup` keyword. Bundle is CLI concern.
- **Bundle encryption** — future extension (add encrypted section type).
- **Remote sync of bundles** — use existing P2P sync or HTTP hooks.

---

## Success Criteria

Phase 12 is done when:

1. `agentis identity hash` prints deterministic identity fingerprint.
2. `introspect.identity_hash` accessible in .ag code (0 CB).
3. `agentis export` produces a portable `.agb` bundle with integrity hash.
4. `agentis import` restores checkpoint + memos + lineage from bundle.
5. `agentis evolve --resume-from agent.agb` continues evolution from bundle.
6. `--backup-to <dir>` writes bundle on each new best.
7. `hooks.on_new_best = backup <dir>` works.
8. `agentis identity verify` detects corruption.
9. All existing tests still pass.
