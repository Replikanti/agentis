// Identity hash computation (Phase 12, M49).
//
// Deterministic identity fingerprint from (seed_hash, generation, lineage chain).
// Uses SHA-256 with `b"AGID"` salt to avoid collision with other hashes in the codebase.

use sha2::{Digest, Sha256};

use crate::checkpoint::CheckpointStore;

/// Compute a deterministic identity hash from seed, generation, and lineage chain.
///
/// Formula: SHA-256(b"AGID" || seed_hash_bytes || generation_u32_le || chain_hash_0 || ...)
/// Empty chain falls back to `[seed_hash]`.
pub fn compute_identity_hash(seed_hash: &str, generation: u32, chain: &[String]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"AGID");
    hasher.update(seed_hash.as_bytes());
    hasher.update(generation.to_le_bytes());

    let effective_chain: Vec<String>;
    let chain_ref = if chain.is_empty() {
        effective_chain = vec![seed_hash.to_string()];
        &effective_chain
    } else {
        chain
    };

    for hash in chain_ref {
        hasher.update(hash.as_bytes());
    }

    let result = hasher.finalize();
    result.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compute identity from a checkpoint by walking its parent chain.
///
/// Collects checkpoint hashes from the chain (newest first), then computes
/// identity from seed_hash + generation + chain hashes.
/// Fallback for old checkpoints without a parent chain: treat as gen 0 seed-only.
pub fn identity_from_checkpoint(
    ckpt_hash: &str,
    store: &CheckpointStore,
) -> Result<String, String> {
    let ckpt = store
        .load(ckpt_hash)
        .map_err(|e| format!("failed to load checkpoint: {e}"))?;

    let chain_result = store.walk_chain(ckpt_hash, None);
    let chain_hashes: Vec<String> = match chain_result {
        Ok(chain) => chain.into_iter().map(|(hash, _)| hash).collect(),
        Err(_) => vec![ckpt_hash.to_string()],
    };

    Ok(compute_identity_hash(
        &ckpt.seed_hash,
        ckpt.generation,
        &chain_hashes,
    ))
}

/// Convenience for gen 0 seed-only identity.
pub fn identity_from_seed(seed_hash: &str) -> String {
    compute_identity_hash(seed_hash, 0, &[])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_identity() {
        let h1 = compute_identity_hash("abc123", 5, &["h1".into(), "h2".into()]);
        let h2 = compute_identity_hash("abc123", 5, &["h1".into(), "h2".into()]);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex
    }

    #[test]
    fn different_generation_changes_hash() {
        let h1 = compute_identity_hash("seed", 1, &["c1".into()]);
        let h2 = compute_identity_hash("seed", 2, &["c1".into()]);
        assert_ne!(h1, h2);
    }

    #[test]
    fn different_chain_changes_hash() {
        let h1 = compute_identity_hash("seed", 1, &["c1".into()]);
        let h2 = compute_identity_hash("seed", 1, &["c1".into(), "c2".into()]);
        assert_ne!(h1, h2);
    }

    #[test]
    fn seed_only_identity() {
        let h = identity_from_seed("deadbeef");
        assert_eq!(h.len(), 64);
        // Empty chain falls back to [seed_hash], so this should be deterministic
        let h2 = compute_identity_hash("deadbeef", 0, &[]);
        assert_eq!(h, h2);
    }

    #[test]
    fn from_checkpoint_with_store() {
        use crate::checkpoint::{CheckpointStore, GenerationCheckpoint, ParentEntry};

        let dir = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(dir.path());

        let ckpt1 = GenerationCheckpoint {
            generation: 1,
            parent: None,
            seed_hash: "seed42".to_string(),
            parents: vec![ParentEntry {
                source: "let x = 1;".to_string(),
                source_hash: "sh1".to_string(),
            }],
            best_ever_score: 0.5,
            best_ever_source: "let x = 1;".to_string(),
            best_ever_hash: "sh1".to_string(),
            stall_count: 0,
            cumulative_cb: 100,
            first_gen_avg_prompts: 1.0,
            gen_best_score: 0.5,
            gen_avg_score: 0.4,
            gen_avg_prompts: 1.0,
            variant_count: 4,
            timestamp: 1000,
            tag: None,
            ancestor_failures: vec![],
            ancestor_successes: vec![],
        };
        let h1 = store.store(&ckpt1).unwrap();

        let ckpt2 = GenerationCheckpoint {
            generation: 2,
            parent: Some(h1.clone()),
            seed_hash: "seed42".to_string(),
            parents: vec![ParentEntry {
                source: "let x = 2;".to_string(),
                source_hash: "sh2".to_string(),
            }],
            best_ever_score: 0.7,
            best_ever_source: "let x = 2;".to_string(),
            best_ever_hash: "sh2".to_string(),
            stall_count: 0,
            cumulative_cb: 200,
            first_gen_avg_prompts: 1.0,
            gen_best_score: 0.7,
            gen_avg_score: 0.5,
            gen_avg_prompts: 1.0,
            variant_count: 4,
            timestamp: 2000,
            tag: None,
            ancestor_failures: vec![],
            ancestor_successes: vec![],
        };
        let h2 = store.store(&ckpt2).unwrap();

        let identity = identity_from_checkpoint(&h2, &store).unwrap();
        assert_eq!(identity.len(), 64);

        // Different from seed-only
        let seed_id = identity_from_seed("seed42");
        assert_ne!(identity, seed_id);
    }

    #[test]
    fn introspect_identity_accessible() {
        // This tests that identity_from_seed produces a non-empty string
        // that can be injected into IntrospectContext
        let id = identity_from_seed("test_seed_hash");
        assert!(!id.is_empty());
        assert_eq!(id.len(), 64);
    }
}
