// Portable agent bundle format (Phase 12, M50-M51).
//
// Wire format:
//   [4B "AGBu"][1B version=1]
//   [1B type][4B len][data...]   ← tagged sections
//     0x01 IDENTITY
//     0x02 SEED
//     0x03 CHECKPOINT
//     0x04 MEMO
//     0x05 LINEAGE
//     0xFF ROOT_HASH
//
// Unknown section types are skipped (forward compat).

use sha2::{Digest, Sha256};
use std::path::Path;

use crate::checkpoint::{CheckpointStore, GenerationCheckpoint};
use crate::error::AgentisError;

// --- Constants ---

const MAGIC: &[u8; 4] = b"AGBu";
const VERSION: u8 = 1;

const SECTION_IDENTITY: u8 = 0x01;
const SECTION_SEED: u8 = 0x02;
const SECTION_CHECKPOINT: u8 = 0x03;
const SECTION_MEMO: u8 = 0x04;
const SECTION_LINEAGE: u8 = 0x05;
const SECTION_PROMPT_STATS: u8 = 0x06;
const SECTION_ROOT_HASH: u8 = 0xFF;

// --- Data types ---

#[derive(Debug, Clone)]
pub struct BundleIdentity {
    pub seed_hash: String,
    pub generation: u32,
    pub identity_hash: String,
    pub version: String,
    pub tags: Vec<String>,
    pub timestamp: u64,
}

#[derive(Debug, Clone)]
pub struct MemoEntry {
    pub key: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct LineageEntry {
    pub filename: String,
    pub content: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct BundleContents {
    pub identity: BundleIdentity,
    pub seed_source: String,
    pub checkpoint_data: Option<Vec<u8>>,
    pub memos: Vec<MemoEntry>,
    pub lineage_data: Vec<LineageEntry>,
    pub prompt_stats: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct ImportResult {
    pub checkpoint_hash: Option<String>,
    pub memo_keys_restored: usize,
    pub lineage_files_restored: usize,
}

#[derive(Debug, Clone, Copy)]
pub enum MemoConflict {
    Skip,
    Append,
    Replace,
}

#[derive(Debug, Clone)]
pub struct VerifyReport {
    pub root_hash_ok: bool,
    pub identity_ok: bool,
    pub computed_root: String,
    pub stored_root: String,
    pub identity_hash: String,
    pub stored_identity: String,
}

// --- Binary helpers ---

fn write_u32(buf: &mut Vec<u8>, val: u32) {
    buf.extend_from_slice(&val.to_le_bytes());
}

fn write_u64(buf: &mut Vec<u8>, val: u64) {
    buf.extend_from_slice(&val.to_le_bytes());
}

fn write_string(buf: &mut Vec<u8>, s: &str) {
    write_u32(buf, s.len() as u32);
    buf.extend_from_slice(s.as_bytes());
}

fn read_u32(data: &[u8], pos: &mut usize) -> Result<u32, AgentisError> {
    if *pos + 4 > data.len() {
        return Err(AgentisError::General(
            "bundle: unexpected end reading u32".into(),
        ));
    }
    let val = u32::from_le_bytes(data[*pos..*pos + 4].try_into().unwrap());
    *pos += 4;
    Ok(val)
}

fn read_u64(data: &[u8], pos: &mut usize) -> Result<u64, AgentisError> {
    if *pos + 8 > data.len() {
        return Err(AgentisError::General(
            "bundle: unexpected end reading u64".into(),
        ));
    }
    let val = u64::from_le_bytes(data[*pos..*pos + 8].try_into().unwrap());
    *pos += 8;
    Ok(val)
}

fn read_u8(data: &[u8], pos: &mut usize) -> Result<u8, AgentisError> {
    if *pos >= data.len() {
        return Err(AgentisError::General(
            "bundle: unexpected end reading u8".into(),
        ));
    }
    let val = data[*pos];
    *pos += 1;
    Ok(val)
}

fn read_string(data: &[u8], pos: &mut usize) -> Result<String, AgentisError> {
    let len = read_u32(data, pos)? as usize;
    if *pos + len > data.len() {
        return Err(AgentisError::General(
            "bundle: unexpected end reading string".into(),
        ));
    }
    let s = String::from_utf8(data[*pos..*pos + len].to_vec())
        .map_err(|_| AgentisError::General("bundle: invalid UTF-8".into()))?;
    *pos += len;
    Ok(s)
}

fn read_bytes(data: &[u8], pos: &mut usize, len: usize) -> Result<Vec<u8>, AgentisError> {
    if *pos + len > data.len() {
        return Err(AgentisError::General(
            "bundle: unexpected end reading bytes".into(),
        ));
    }
    let bytes = data[*pos..*pos + len].to_vec();
    *pos += len;
    Ok(bytes)
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    result.iter().map(|b| format!("{b:02x}")).collect()
}

// --- Section serialization ---

fn write_section(buf: &mut Vec<u8>, section_type: u8, data: &[u8]) {
    buf.push(section_type);
    write_u32(buf, data.len() as u32);
    buf.extend_from_slice(data);
}

fn serialize_identity(id: &BundleIdentity) -> Vec<u8> {
    let mut buf = Vec::new();
    write_string(&mut buf, &id.seed_hash);
    write_u32(&mut buf, id.generation);
    write_string(&mut buf, &id.identity_hash);
    write_string(&mut buf, &id.version);
    write_u32(&mut buf, id.tags.len() as u32);
    for tag in &id.tags {
        write_string(&mut buf, tag);
    }
    write_u64(&mut buf, id.timestamp);
    buf
}

fn deserialize_identity(data: &[u8]) -> Result<BundleIdentity, AgentisError> {
    let mut pos = 0;
    let seed_hash = read_string(data, &mut pos)?;
    let generation = read_u32(data, &mut pos)?;
    let identity_hash = read_string(data, &mut pos)?;
    let version = read_string(data, &mut pos)?;
    let tag_count = read_u32(data, &mut pos)?;
    let mut tags = Vec::with_capacity(tag_count as usize);
    for _ in 0..tag_count {
        tags.push(read_string(data, &mut pos)?);
    }
    let timestamp = read_u64(data, &mut pos)?;
    Ok(BundleIdentity {
        seed_hash,
        generation,
        identity_hash,
        version,
        tags,
        timestamp,
    })
}

fn serialize_memos(memos: &[MemoEntry]) -> Vec<u8> {
    let mut buf = Vec::new();
    write_u32(&mut buf, memos.len() as u32);
    for m in memos {
        write_string(&mut buf, &m.key);
        write_string(&mut buf, &m.content);
    }
    buf
}

fn deserialize_memos(data: &[u8]) -> Result<Vec<MemoEntry>, AgentisError> {
    let mut pos = 0;
    let count = read_u32(data, &mut pos)?;
    let mut memos = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let key = read_string(data, &mut pos)?;
        let content = read_string(data, &mut pos)?;
        memos.push(MemoEntry { key, content });
    }
    Ok(memos)
}

fn serialize_lineage(entries: &[LineageEntry]) -> Vec<u8> {
    let mut buf = Vec::new();
    write_u32(&mut buf, entries.len() as u32);
    for e in entries {
        write_string(&mut buf, &e.filename);
        write_string(&mut buf, &e.content);
    }
    buf
}

fn deserialize_lineage(data: &[u8]) -> Result<Vec<LineageEntry>, AgentisError> {
    let mut pos = 0;
    let count = read_u32(data, &mut pos)?;
    let mut entries = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let filename = read_string(data, &mut pos)?;
        let content = read_string(data, &mut pos)?;
        entries.push(LineageEntry { filename, content });
    }
    Ok(entries)
}

// --- Public API ---

/// Write a complete .agb bundle to disk.
#[allow(clippy::too_many_arguments)]
pub fn write_bundle(
    path: &str,
    identity: &BundleIdentity,
    seed_source: &str,
    checkpoint_data: Option<&[u8]>,
    memos: &[MemoEntry],
    lineage: &[LineageEntry],
) -> Result<(), AgentisError> {
    write_bundle_with_stats(
        path,
        identity,
        seed_source,
        checkpoint_data,
        memos,
        lineage,
        None,
    )
}

/// Write a complete .agb bundle to disk, optionally including prompt stats.
#[allow(clippy::too_many_arguments)]
pub fn write_bundle_with_stats(
    path: &str,
    identity: &BundleIdentity,
    seed_source: &str,
    checkpoint_data: Option<&[u8]>,
    memos: &[MemoEntry],
    lineage: &[LineageEntry],
    prompt_stats: Option<&[u8]>,
) -> Result<(), AgentisError> {
    let mut buf = Vec::new();

    // Header
    buf.extend_from_slice(MAGIC);
    buf.push(VERSION);

    // Identity section
    let id_data = serialize_identity(identity);
    write_section(&mut buf, SECTION_IDENTITY, &id_data);

    // Seed section
    write_section(&mut buf, SECTION_SEED, seed_source.as_bytes());

    // Checkpoint section (optional)
    if let Some(ckpt) = checkpoint_data {
        write_section(&mut buf, SECTION_CHECKPOINT, ckpt);
    }

    // Memo section (optional)
    if !memos.is_empty() {
        let memo_data = serialize_memos(memos);
        write_section(&mut buf, SECTION_MEMO, &memo_data);
    }

    // Lineage section (optional)
    if !lineage.is_empty() {
        let lineage_data = serialize_lineage(lineage);
        write_section(&mut buf, SECTION_LINEAGE, &lineage_data);
    }

    // Prompt stats section (optional, Phase 13)
    if let Some(stats) = prompt_stats {
        write_section(&mut buf, SECTION_PROMPT_STATS, stats);
    }

    // Root hash — SHA-256 of everything preceding
    let root_hash = sha256_hex(&buf);
    let mut hash_section = Vec::new();
    write_string(&mut hash_section, &root_hash);
    write_section(&mut buf, SECTION_ROOT_HASH, &hash_section);

    std::fs::write(path, &buf)?;
    Ok(())
}

/// Read and validate a .agb bundle from disk.
pub fn read_bundle(path: &str) -> Result<BundleContents, AgentisError> {
    let data = std::fs::read(path)?;
    parse_bundle(&data)
}

/// Parse bundle bytes (used by both read_bundle and tests).
fn parse_bundle(data: &[u8]) -> Result<BundleContents, AgentisError> {
    if data.len() < 5 {
        return Err(AgentisError::General("bundle: file too short".into()));
    }
    if &data[0..4] != MAGIC {
        return Err(AgentisError::General("bundle: bad magic bytes".into()));
    }
    if data[4] != VERSION {
        return Err(AgentisError::General(format!(
            "bundle: unsupported version {}",
            data[4]
        )));
    }

    let mut pos = 5;
    let mut identity: Option<BundleIdentity> = None;
    let mut seed_source: Option<String> = None;
    let mut checkpoint_data: Option<Vec<u8>> = None;
    let mut memos: Vec<MemoEntry> = Vec::new();
    let mut lineage_data: Vec<LineageEntry> = Vec::new();
    let mut prompt_stats: Option<Vec<u8>> = None;
    let mut stored_root: Option<String> = None;
    let mut root_hash_start = 0usize;

    while pos < data.len() {
        let section_type = read_u8(data, &mut pos)?;
        let section_len = read_u32(data, &mut pos)? as usize;

        if section_type == SECTION_ROOT_HASH {
            // Everything before this section is hashed
            // root_hash_start was set before we read this section type byte
            root_hash_start = pos - 5; // back to before type byte
            let section_data = read_bytes(data, &mut pos, section_len)?;
            let mut spos = 0;
            stored_root = Some(read_string(&section_data, &mut spos)?);
        } else {
            let section_data = read_bytes(data, &mut pos, section_len)?;
            match section_type {
                SECTION_IDENTITY => {
                    identity = Some(deserialize_identity(&section_data)?);
                }
                SECTION_SEED => {
                    seed_source =
                        Some(String::from_utf8(section_data).map_err(|_| {
                            AgentisError::General("bundle: invalid seed UTF-8".into())
                        })?);
                }
                SECTION_CHECKPOINT => {
                    checkpoint_data = Some(section_data);
                }
                SECTION_MEMO => {
                    memos = deserialize_memos(&section_data)?;
                }
                SECTION_LINEAGE => {
                    lineage_data = deserialize_lineage(&section_data)?;
                }
                SECTION_PROMPT_STATS => {
                    prompt_stats = Some(section_data);
                }
                _ => {
                    // Unknown section — skip (forward compat)
                }
            }
        }
    }

    // Validate root hash
    if let Some(ref stored) = stored_root {
        let computed = sha256_hex(&data[..root_hash_start]);
        if computed != *stored {
            return Err(AgentisError::General(format!(
                "bundle: integrity check failed (expected {}, got {})",
                &stored[..16],
                &computed[..16]
            )));
        }
    }

    let id =
        identity.ok_or_else(|| AgentisError::General("bundle: missing IDENTITY section".into()))?;
    let seed =
        seed_source.ok_or_else(|| AgentisError::General("bundle: missing SEED section".into()))?;

    Ok(BundleContents {
        identity: id,
        seed_source: seed,
        checkpoint_data,
        memos,
        lineage_data,
        prompt_stats,
    })
}

/// Verify bundle integrity without importing. Returns a report.
pub fn verify_bundle(path: &str) -> Result<VerifyReport, AgentisError> {
    let data = std::fs::read(path)?;

    if data.len() < 5 {
        return Err(AgentisError::General("bundle: file too short".into()));
    }
    if &data[0..4] != MAGIC {
        return Err(AgentisError::General("bundle: bad magic bytes".into()));
    }

    let mut pos = 5;
    let mut identity: Option<BundleIdentity> = None;
    let mut stored_root = String::new();
    let mut root_hash_start = 0usize;

    while pos < data.len() {
        let section_type = read_u8(&data, &mut pos)?;
        let section_len = read_u32(&data, &mut pos)? as usize;

        if section_type == SECTION_ROOT_HASH {
            root_hash_start = pos - 5;
            let section_data = read_bytes(&data, &mut pos, section_len)?;
            let mut spos = 0;
            stored_root = read_string(&section_data, &mut spos)?;
        } else {
            let section_data = read_bytes(&data, &mut pos, section_len)?;
            if section_type == SECTION_IDENTITY {
                identity = Some(deserialize_identity(&section_data)?);
            }
        }
    }

    let computed_root = sha256_hex(&data[..root_hash_start]);
    let root_hash_ok = computed_root == stored_root;

    let id =
        identity.ok_or_else(|| AgentisError::General("bundle: missing IDENTITY section".into()))?;

    // Identity verification: verify the stored identity_hash is a valid SHA-256 hex string.
    // Full recomputation requires the checkpoint chain which the bundle may not contain.
    let identity_ok = !id.identity_hash.is_empty() && id.identity_hash.len() == 64;

    Ok(VerifyReport {
        root_hash_ok,
        identity_ok,
        computed_root,
        stored_root,
        identity_hash: id.identity_hash.clone(),
        stored_identity: id.identity_hash,
    })
}

/// Collect memo files from .agentis/memo/ directory.
pub fn collect_memos(memo_dir: &Path) -> Result<Vec<MemoEntry>, AgentisError> {
    let mut memos = Vec::new();
    if !memo_dir.exists() {
        return Ok(memos);
    }
    let mut entries: Vec<_> = std::fs::read_dir(memo_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let key = entry
            .path()
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let content = std::fs::read_to_string(entry.path())?;
        if !content.is_empty() {
            memos.push(MemoEntry { key, content });
        }
    }
    Ok(memos)
}

/// Collect lineage JSONL files from .agentis/fitness/ directory.
pub fn collect_lineage(
    fitness_dir: &Path,
    depth: Option<usize>,
) -> Result<Vec<LineageEntry>, AgentisError> {
    let mut entries = Vec::new();
    if !fitness_dir.exists() {
        return Ok(entries);
    }
    let mut files: Vec<_> = std::fs::read_dir(fitness_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    files.sort_by_key(|e| e.file_name());

    // If depth specified, take only the last N files
    if let Some(d) = depth {
        let skip = files.len().saturating_sub(d);
        files = files.into_iter().skip(skip).collect();
    }

    for file in files {
        let filename = file.file_name().to_string_lossy().to_string();
        let content = std::fs::read_to_string(file.path())?;
        if !content.is_empty() {
            entries.push(LineageEntry { filename, content });
        }
    }
    Ok(entries)
}

/// Import bundle contents into .agentis/ directory.
pub fn import_to_store(
    contents: &BundleContents,
    agentis_root: &Path,
    memo_conflict: MemoConflict,
) -> Result<ImportResult, AgentisError> {
    let mut result = ImportResult {
        checkpoint_hash: None,
        memo_keys_restored: 0,
        lineage_files_restored: 0,
    };

    // Restore checkpoint
    if let Some(ref ckpt_data) = contents.checkpoint_data {
        let ckpt = GenerationCheckpoint::from_bytes(ckpt_data)
            .map_err(|e| AgentisError::General(format!("bundle: invalid checkpoint: {e}")))?;
        let store = CheckpointStore::new(agentis_root);
        let hash = store
            .store(&ckpt)
            .map_err(|e| AgentisError::General(format!("bundle: checkpoint store: {e}")))?;
        store
            .set_head(&hash)
            .map_err(|e| AgentisError::General(format!("bundle: set HEAD: {e}")))?;
        result.checkpoint_hash = Some(hash);
    }

    // Restore memos
    if !contents.memos.is_empty() {
        let memo_dir = agentis_root.join("memo");
        std::fs::create_dir_all(&memo_dir)?;

        for memo in &contents.memos {
            let path = memo_dir.join(format!("{}.jsonl", &memo.key));
            match memo_conflict {
                MemoConflict::Skip => {
                    if !path.exists() {
                        std::fs::write(&path, &memo.content)?;
                        result.memo_keys_restored += 1;
                    }
                }
                MemoConflict::Append => {
                    if path.exists() {
                        let existing = std::fs::read_to_string(&path)?;
                        let merged = if existing.ends_with('\n') {
                            format!("{}{}", existing, memo.content)
                        } else {
                            format!("{}\n{}", existing, memo.content)
                        };
                        std::fs::write(&path, merged)?;
                    } else {
                        std::fs::write(&path, &memo.content)?;
                    }
                    result.memo_keys_restored += 1;
                }
                MemoConflict::Replace => {
                    std::fs::write(&path, &memo.content)?;
                    result.memo_keys_restored += 1;
                }
            }
        }
    }

    // Restore lineage
    if !contents.lineage_data.is_empty() {
        let fitness_dir = agentis_root.join("fitness");
        std::fs::create_dir_all(&fitness_dir)?;

        for entry in &contents.lineage_data {
            let path = fitness_dir.join(&entry.filename);
            std::fs::write(&path, &entry.content)?;
            result.lineage_files_restored += 1;
        }
    }

    // Restore prompt stats (Phase 13) with deduplication
    if let Some(ref stats_data) = contents.prompt_stats {
        let imported = crate::prediction::parse_stats_bytes(stats_data);
        let mut local = crate::prediction::PromptCostHistory::load(agentis_root);
        crate::prediction::merge_stats(&mut local, &imported);
        let _ = local.save(agentis_root);
    }

    Ok(result)
}

/// Write a backup bundle from the evolution loop context.
#[allow(clippy::too_many_arguments)]
pub fn write_evolve_backup(
    dir: &str,
    generation: u32,
    seed_hash: &str,
    best_source: &str,
    ckpt_data: Option<&[u8]>,
    agentis_root: &Path,
    identity_hash: &str,
    tags: &[String],
) -> Result<std::path::PathBuf, AgentisError> {
    std::fs::create_dir_all(dir)?;

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let bundle_identity = BundleIdentity {
        seed_hash: seed_hash.to_string(),
        generation,
        identity_hash: identity_hash.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        tags: tags.to_vec(),
        timestamp: ts,
    };

    // Collect memos (always include in backups)
    let memo_dir = agentis_root.join("memo");
    let memos = collect_memos(&memo_dir).unwrap_or_default();

    // Collect lineage (last 10 generations)
    let fitness_dir = agentis_root.join("fitness");
    let lineage = collect_lineage(&fitness_dir, Some(10)).unwrap_or_default();

    let gen_path = std::path::Path::new(dir).join(format!("g{generation:02}-best.agb"));
    write_bundle(
        &gen_path.to_string_lossy(),
        &bundle_identity,
        best_source,
        ckpt_data,
        &memos,
        &lineage,
    )?;

    // Also write latest.agb
    let latest_path = std::path::Path::new(dir).join("latest.agb");
    std::fs::copy(&gen_path, &latest_path)?;

    Ok(gen_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity;

    fn make_test_identity() -> BundleIdentity {
        BundleIdentity {
            seed_hash: "abcdef1234567890".to_string(),
            generation: 5,
            identity_hash: identity::compute_identity_hash("abcdef1234567890", 5, &[]),
            version: "0.8.0".to_string(),
            tags: vec!["stable".to_string()],
            timestamp: 1234567890,
        }
    }

    #[test]
    fn roundtrip_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.agb");
        let path_str = path.to_string_lossy().to_string();

        let id = make_test_identity();
        let seed = "let x = 42;\nprint(x);";
        let ckpt = b"AGCK fake checkpoint data";
        let memos = vec![MemoEntry {
            key: "strategy".to_string(),
            content: "{\"msg\":\"be careful\"}\n".to_string(),
        }];
        let lineage = vec![LineageEntry {
            filename: "g01.jsonl".to_string(),
            content: "{\"gen\":1}\n".to_string(),
        }];

        write_bundle(&path_str, &id, seed, Some(ckpt), &memos, &lineage).unwrap();
        let contents = read_bundle(&path_str).unwrap();

        assert_eq!(contents.identity.seed_hash, id.seed_hash);
        assert_eq!(contents.identity.generation, 5);
        assert_eq!(contents.identity.identity_hash, id.identity_hash);
        assert_eq!(contents.identity.tags, vec!["stable"]);
        assert_eq!(contents.seed_source, seed);
        assert_eq!(contents.checkpoint_data, Some(ckpt.to_vec()));
        assert_eq!(contents.memos.len(), 1);
        assert_eq!(contents.memos[0].key, "strategy");
        assert_eq!(contents.lineage_data.len(), 1);
        assert_eq!(contents.lineage_data[0].filename, "g01.jsonl");
    }

    #[test]
    fn integrity_check_corrupt_byte() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("corrupt.agb");
        let path_str = path.to_string_lossy().to_string();

        let id = make_test_identity();
        write_bundle(&path_str, &id, "let x = 1;", None, &[], &[]).unwrap();

        // Corrupt a byte in the middle of the file
        let mut data = std::fs::read(&path).unwrap();
        let mid = data.len() / 2;
        data[mid] ^= 0xFF;
        std::fs::write(&path, &data).unwrap();

        let result = read_bundle(&path_str);
        assert!(result.is_err());
    }

    #[test]
    fn skip_unknown_section() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("future.agb");
        let path_str = path.to_string_lossy().to_string();

        // Build a bundle with an unknown section (type 0x42)
        let mut buf = Vec::new();
        buf.extend_from_slice(MAGIC);
        buf.push(VERSION);

        // Identity section
        let id = make_test_identity();
        let id_data = serialize_identity(&id);
        write_section(&mut buf, SECTION_IDENTITY, &id_data);

        // Seed section
        write_section(&mut buf, SECTION_SEED, b"let x = 1;");

        // Unknown section
        write_section(&mut buf, 0x42, b"future data");

        // Root hash
        let root_hash = sha256_hex(&buf);
        let mut hash_section = Vec::new();
        write_string(&mut hash_section, &root_hash);
        write_section(&mut buf, SECTION_ROOT_HASH, &hash_section);

        std::fs::write(&path, &buf).unwrap();

        let contents = read_bundle(&path_str).unwrap();
        assert_eq!(contents.identity.seed_hash, id.seed_hash);
        assert_eq!(contents.seed_source, "let x = 1;");
    }

    #[test]
    fn bundle_without_memos() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nomemo.agb");
        let path_str = path.to_string_lossy().to_string();

        let id = make_test_identity();
        write_bundle(&path_str, &id, "seed code", None, &[], &[]).unwrap();

        let contents = read_bundle(&path_str).unwrap();
        assert!(contents.memos.is_empty());
        assert!(contents.checkpoint_data.is_none());
    }

    #[test]
    fn bundle_without_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nockpt.agb");
        let path_str = path.to_string_lossy().to_string();

        let id = make_test_identity();
        let memos = vec![MemoEntry {
            key: "test".to_string(),
            content: "data\n".to_string(),
        }];
        write_bundle(&path_str, &id, "code", None, &memos, &[]).unwrap();

        let contents = read_bundle(&path_str).unwrap();
        assert!(contents.checkpoint_data.is_none());
        assert_eq!(contents.memos.len(), 1);
    }

    #[test]
    fn identity_section_roundtrip() {
        let id = make_test_identity();
        let data = serialize_identity(&id);
        let parsed = deserialize_identity(&data).unwrap();
        assert_eq!(parsed.seed_hash, id.seed_hash);
        assert_eq!(parsed.generation, id.generation);
        assert_eq!(parsed.identity_hash, id.identity_hash);
        assert_eq!(parsed.version, id.version);
        assert_eq!(parsed.tags, id.tags);
        assert_eq!(parsed.timestamp, id.timestamp);
    }

    #[test]
    fn import_restores_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let agentis_root = dir.path();

        // Create a real checkpoint
        use crate::checkpoint::{GenerationCheckpoint, ParentEntry};
        let ckpt = GenerationCheckpoint {
            generation: 3,
            parent: None,
            seed_hash: "seed123".to_string(),
            parents: vec![ParentEntry {
                source: "let x = 1;".to_string(),
                source_hash: "sh1".to_string(),
            }],
            best_ever_score: 0.9,
            best_ever_source: "let x = 1;".to_string(),
            best_ever_hash: "sh1".to_string(),
            stall_count: 0,
            cumulative_cb: 500,
            first_gen_avg_prompts: 2.0,
            gen_best_score: 0.9,
            gen_avg_score: 0.7,
            gen_avg_prompts: 2.0,
            variant_count: 4,
            timestamp: 1000,
            tag: None,
            ancestor_failures: vec![],
            ancestor_successes: vec![],
        };
        let ckpt_bytes = ckpt.to_bytes();

        let contents = BundleContents {
            identity: make_test_identity(),
            seed_source: "let x = 1;".to_string(),
            checkpoint_data: Some(ckpt_bytes),
            memos: vec![],
            lineage_data: vec![],
            prompt_stats: None,
        };

        let result = import_to_store(&contents, agentis_root, MemoConflict::Append).unwrap();
        assert!(result.checkpoint_hash.is_some());

        // Verify HEAD was set
        let store = CheckpointStore::new(agentis_root);
        let head = store.head().unwrap();
        assert_eq!(head, result.checkpoint_hash);
    }

    #[test]
    fn import_restores_memos() {
        let dir = tempfile::tempdir().unwrap();
        let agentis_root = dir.path();

        let contents = BundleContents {
            identity: make_test_identity(),
            seed_source: "code".to_string(),
            checkpoint_data: None,
            memos: vec![
                MemoEntry {
                    key: "strategy".to_string(),
                    content: "{\"msg\":\"think\"}\n".to_string(),
                },
                MemoEntry {
                    key: "history".to_string(),
                    content: "{\"gen\":1}\n".to_string(),
                },
            ],
            lineage_data: vec![],
            prompt_stats: None,
        };

        let result = import_to_store(&contents, agentis_root, MemoConflict::Append).unwrap();
        assert_eq!(result.memo_keys_restored, 2);

        // Verify files exist
        let memo_dir = agentis_root.join("memo");
        assert!(memo_dir.join("strategy.jsonl").exists());
        assert!(memo_dir.join("history.jsonl").exists());
    }

    #[test]
    fn import_restores_lineage() {
        let dir = tempfile::tempdir().unwrap();
        let agentis_root = dir.path();

        let contents = BundleContents {
            identity: make_test_identity(),
            seed_source: "code".to_string(),
            checkpoint_data: None,
            memos: vec![],
            lineage_data: vec![LineageEntry {
                filename: "g01.jsonl".to_string(),
                content: "{\"gen\":1}\n".to_string(),
            }],
            prompt_stats: None,
        };

        let result = import_to_store(&contents, agentis_root, MemoConflict::Append).unwrap();
        assert_eq!(result.lineage_files_restored, 1);
        assert!(agentis_root.join("fitness").join("g01.jsonl").exists());
    }

    #[test]
    fn import_memo_append_existing() {
        let dir = tempfile::tempdir().unwrap();
        let agentis_root = dir.path();
        let memo_dir = agentis_root.join("memo");
        std::fs::create_dir_all(&memo_dir).unwrap();

        // Pre-existing memo
        std::fs::write(memo_dir.join("strategy.jsonl"), "{\"old\":true}\n").unwrap();

        let contents = BundleContents {
            identity: make_test_identity(),
            seed_source: "code".to_string(),
            checkpoint_data: None,
            memos: vec![MemoEntry {
                key: "strategy".to_string(),
                content: "{\"new\":true}\n".to_string(),
            }],
            lineage_data: vec![],
            prompt_stats: None,
        };

        import_to_store(&contents, agentis_root, MemoConflict::Append).unwrap();

        let content = std::fs::read_to_string(memo_dir.join("strategy.jsonl")).unwrap();
        assert!(content.contains("old"));
        assert!(content.contains("new"));
    }

    #[test]
    fn import_as_tag() {
        let dir = tempfile::tempdir().unwrap();
        let agentis_root = dir.path();

        use crate::checkpoint::{GenerationCheckpoint, ParentEntry};
        let ckpt = GenerationCheckpoint {
            generation: 1,
            parent: None,
            seed_hash: "s".to_string(),
            parents: vec![ParentEntry {
                source: "x".to_string(),
                source_hash: "h".to_string(),
            }],
            best_ever_score: 0.5,
            best_ever_source: "x".to_string(),
            best_ever_hash: "h".to_string(),
            stall_count: 0,
            cumulative_cb: 0,
            first_gen_avg_prompts: 0.0,
            gen_best_score: 0.5,
            gen_avg_score: 0.5,
            gen_avg_prompts: 0.0,
            variant_count: 1,
            timestamp: 0,
            tag: None,
            ancestor_failures: vec![],
            ancestor_successes: vec![],
        };

        let contents = BundleContents {
            identity: make_test_identity(),
            seed_source: "x".to_string(),
            checkpoint_data: Some(ckpt.to_bytes()),
            memos: vec![],
            lineage_data: vec![],
            prompt_stats: None,
        };

        let result = import_to_store(&contents, agentis_root, MemoConflict::Append).unwrap();

        // Tag it
        let store = CheckpointStore::new(agentis_root);
        if let Some(ref hash) = result.checkpoint_hash {
            store.set_tag("imported", hash).unwrap();
            let resolved = store.resolve_tag("imported").unwrap();
            assert_eq!(resolved, Some(hash.clone()));
        }
    }

    #[test]
    fn verify_valid_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("valid.agb");
        let path_str = path.to_string_lossy().to_string();

        let id = make_test_identity();
        write_bundle(&path_str, &id, "code", None, &[], &[]).unwrap();

        let report = verify_bundle(&path_str).unwrap();
        assert!(report.root_hash_ok);
        assert!(report.identity_ok);
    }

    #[test]
    fn verify_corrupted_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.agb");
        let path_str = path.to_string_lossy().to_string();

        let id = make_test_identity();
        write_bundle(&path_str, &id, "code", None, &[], &[]).unwrap();

        // Corrupt
        let mut data = std::fs::read(&path).unwrap();
        data[10] ^= 0xFF;
        std::fs::write(&path, &data).unwrap();

        // verify_bundle should either fail or report root_hash_ok = false
        // depending on where the corruption is
        let result = verify_bundle(&path_str);
        if let Ok(report) = result {
            assert!(!report.root_hash_ok);
        }
        // If it fails to parse at all, that's also acceptable
    }

    #[test]
    fn diff_same_seed() {
        let id_a = BundleIdentity {
            seed_hash: "same_seed".to_string(),
            generation: 5,
            identity_hash: identity::compute_identity_hash("same_seed", 5, &[]),
            version: "0.8.0".to_string(),
            tags: vec![],
            timestamp: 100,
        };
        let id_b = BundleIdentity {
            seed_hash: "same_seed".to_string(),
            generation: 10,
            identity_hash: identity::compute_identity_hash("same_seed", 10, &[]),
            version: "0.8.0".to_string(),
            tags: vec![],
            timestamp: 200,
        };
        assert_eq!(id_a.seed_hash, id_b.seed_hash);
        assert_ne!(id_a.identity_hash, id_b.identity_hash);
    }

    #[test]
    fn diff_different_seed() {
        let id_a = BundleIdentity {
            seed_hash: "seed_a".to_string(),
            generation: 5,
            identity_hash: identity::compute_identity_hash("seed_a", 5, &[]),
            version: "0.8.0".to_string(),
            tags: vec![],
            timestamp: 100,
        };
        let id_b = BundleIdentity {
            seed_hash: "seed_b".to_string(),
            generation: 5,
            identity_hash: identity::compute_identity_hash("seed_b", 5, &[]),
            version: "0.8.0".to_string(),
            tags: vec![],
            timestamp: 100,
        };
        assert_ne!(id_a.seed_hash, id_b.seed_hash);
        assert_ne!(id_a.identity_hash, id_b.identity_hash);
    }

    // --- M51: full resume round-trip ---

    #[test]
    fn full_resume_round_trip() {
        // Export a bundle with checkpoint, import it into a fresh store,
        // then verify the checkpoint can be loaded and HEAD is set.
        use crate::checkpoint::{CheckpointStore, GenerationCheckpoint, ParentEntry};

        let dir = tempfile::tempdir().unwrap();
        let export_root = dir.path().join("export");
        std::fs::create_dir_all(&export_root).unwrap();

        // Create a checkpoint in the "export" store
        let ckpt_store = CheckpointStore::new(&export_root);
        let ckpt = GenerationCheckpoint {
            generation: 5,
            parent: None,
            seed_hash: "resume_seed".to_string(),
            parents: vec![ParentEntry {
                source: "let x = 42;".to_string(),
                source_hash: "src42".to_string(),
            }],
            best_ever_score: 0.85,
            best_ever_source: "let x = 42;".to_string(),
            best_ever_hash: "src42".to_string(),
            stall_count: 1,
            cumulative_cb: 999,
            first_gen_avg_prompts: 2.0,
            gen_best_score: 0.85,
            gen_avg_score: 0.7,
            gen_avg_prompts: 2.0,
            variant_count: 4,
            timestamp: 5000,
            tag: None,
            ancestor_failures: vec![],
            ancestor_successes: vec![],
        };
        let ckpt_bytes = ckpt.to_bytes();

        // Write bundle
        let bundle_path = dir.path().join("resume.agb");
        let bundle_path_str = bundle_path.to_string_lossy().to_string();
        let id = BundleIdentity {
            seed_hash: "resume_seed".to_string(),
            generation: 5,
            identity_hash: identity::compute_identity_hash("resume_seed", 5, &[]),
            version: "0.8.0".to_string(),
            tags: vec!["checkpoint-v5".to_string()],
            timestamp: 5000,
        };
        let memos = vec![MemoEntry {
            key: "strategy".to_string(),
            content: "{\"learned\":\"be careful\"}\n".to_string(),
        }];
        let lineage = vec![LineageEntry {
            filename: "g05.jsonl".to_string(),
            content: "{\"gen\":5,\"best\":0.85}\n".to_string(),
        }];
        write_bundle(
            &bundle_path_str,
            &id,
            "let x = 42;",
            Some(&ckpt_bytes),
            &memos,
            &lineage,
        )
        .unwrap();

        // Import into a completely fresh store
        let import_root = dir.path().join("import");
        std::fs::create_dir_all(&import_root).unwrap();
        let contents = read_bundle(&bundle_path_str).unwrap();
        let result = import_to_store(&contents, &import_root, MemoConflict::Append).unwrap();

        // Verify checkpoint was restored and HEAD set
        assert!(result.checkpoint_hash.is_some());
        let import_ckpt_store = CheckpointStore::new(&import_root);
        let head = import_ckpt_store.head().unwrap();
        assert_eq!(head, result.checkpoint_hash);

        // Verify checkpoint data is intact
        let loaded = import_ckpt_store.load(&head.unwrap()).unwrap();
        assert_eq!(loaded.generation, 5);
        assert_eq!(loaded.seed_hash, "resume_seed");
        assert_eq!(loaded.best_ever_score, 0.85);
        assert_eq!(loaded.best_ever_source, "let x = 42;");
        assert_eq!(loaded.stall_count, 1);
        assert_eq!(loaded.cumulative_cb, 999);

        // Verify memos restored
        assert_eq!(result.memo_keys_restored, 1);
        let memo_content =
            std::fs::read_to_string(import_root.join("memo").join("strategy.jsonl")).unwrap();
        assert!(memo_content.contains("be careful"));

        // Verify lineage restored
        assert_eq!(result.lineage_files_restored, 1);
        assert!(import_root.join("fitness").join("g05.jsonl").exists());
    }

    // --- M52: backup tests ---

    #[test]
    fn backup_creates_bundle_files() {
        let dir = tempfile::tempdir().unwrap();
        let agentis_root = dir.path().join("agentis");
        std::fs::create_dir_all(&agentis_root).unwrap();
        let backup_dir = dir.path().join("backups");

        let result = write_evolve_backup(
            &backup_dir.to_string_lossy(),
            3,
            "seed_abc",
            "let x = 1;",
            None,
            &agentis_root,
            "idhash64charsaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            &[],
        )
        .unwrap();

        // gen file created
        assert!(result.exists());
        assert_eq!(result.file_name().unwrap(), "g03-best.agb");

        // latest.agb also created
        assert!(backup_dir.join("latest.agb").exists());
    }

    #[test]
    fn backup_bundle_is_valid() {
        let dir = tempfile::tempdir().unwrap();
        let agentis_root = dir.path().join("agentis");
        std::fs::create_dir_all(&agentis_root).unwrap();
        let backup_dir = dir.path().join("backups");

        let result = write_evolve_backup(
            &backup_dir.to_string_lossy(),
            7,
            "seed_xyz",
            "let y = 2;",
            None,
            &agentis_root,
            "idhash64charsbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            &["tag-a".to_string()],
        )
        .unwrap();

        // Read it back — must parse without error
        let contents = read_bundle(&result.to_string_lossy()).unwrap();
        assert_eq!(contents.identity.seed_hash, "seed_xyz");
        assert_eq!(contents.identity.generation, 7);
        assert_eq!(contents.identity.tags, vec!["tag-a"]);

        // Verify integrity
        let report = verify_bundle(&result.to_string_lossy()).unwrap();
        assert!(report.root_hash_ok);
    }

    #[test]
    fn backup_overwrites_latest_on_new_best() {
        let dir = tempfile::tempdir().unwrap();
        let agentis_root = dir.path().join("agentis");
        std::fs::create_dir_all(&agentis_root).unwrap();
        let backup_dir = dir.path().join("backups");
        let backup_str = backup_dir.to_string_lossy().to_string();

        // First backup at gen 2
        write_evolve_backup(
            &backup_str,
            2,
            "seed",
            "let a = 1;",
            None,
            &agentis_root,
            "id_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            &[],
        )
        .unwrap();

        let latest_size_1 = std::fs::metadata(backup_dir.join("latest.agb"))
            .unwrap()
            .len();

        // Second backup at gen 5 with different (longer) source
        write_evolve_backup(
            &backup_str,
            5,
            "seed",
            "let a = 1;\nlet b = 2;\nlet c = 3;\nprint(a + b + c);",
            None,
            &agentis_root,
            "id_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            &[],
        )
        .unwrap();

        let latest_size_2 = std::fs::metadata(backup_dir.join("latest.agb"))
            .unwrap()
            .len();

        // latest.agb should have been overwritten (different size due to longer source)
        assert_ne!(latest_size_1, latest_size_2);

        // Both gen files should exist
        assert!(backup_dir.join("g02-best.agb").exists());
        assert!(backup_dir.join("g05-best.agb").exists());

        // latest.agb should match the newer one
        let latest_data = std::fs::read(backup_dir.join("latest.agb")).unwrap();
        let gen5_data = std::fs::read(backup_dir.join("g05-best.agb")).unwrap();
        assert_eq!(latest_data, gen5_data);
    }

    // --- Phase 13: bundle prompt stats ---

    #[test]
    fn bundle_roundtrip_includes_prompt_stats() {
        let dir = tempfile::tempdir().unwrap();
        let bundle_path = dir.path().join("with_stats.agb");
        let bundle_path_str = bundle_path.to_string_lossy().to_string();

        let id = BundleIdentity {
            seed_hash: "stats_seed".into(),
            generation: 1,
            identity_hash: "stats_id".into(),
            version: "0.9.0".into(),
            tags: vec![],
            timestamp: 0,
        };

        let stats_jsonl = concat!(
            r#"{"instruction_hash":"abc","input_len":42,"cb_cost":50,"prompt_count":1,"backend":"mock"}"#,
            "\n",
        );
        let stats_bytes = stats_jsonl.as_bytes();

        write_bundle_with_stats(
            &bundle_path_str,
            &id,
            "let x = 1;",
            None,
            &[],
            &[],
            Some(stats_bytes),
        )
        .unwrap();

        // Read back and verify section present
        let contents = read_bundle(&bundle_path_str).unwrap();
        assert!(contents.prompt_stats.is_some());
        let parsed = crate::prediction::parse_stats_bytes(contents.prompt_stats.as_ref().unwrap());
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed.records()[0].instruction_hash, "abc");
        assert_eq!(parsed.records()[0].cb_cost, 50);
    }

    #[test]
    fn bundle_import_restores_stats_with_dedup() {
        let dir = tempfile::tempdir().unwrap();
        let bundle_path = dir.path().join("dedup.agb");
        let bundle_path_str = bundle_path.to_string_lossy().to_string();

        let id = BundleIdentity {
            seed_hash: "dedup_seed".into(),
            generation: 1,
            identity_hash: "dedup_id".into(),
            version: "0.9.0".into(),
            tags: vec![],
            timestamp: 0,
        };

        let stats_jsonl = concat!(
            r#"{"instruction_hash":"xyz","input_len":100,"cb_cost":50,"prompt_count":1,"backend":"mock"}"#,
            "\n",
        );

        write_bundle_with_stats(
            &bundle_path_str,
            &id,
            "code",
            None,
            &[],
            &[],
            Some(stats_jsonl.as_bytes()),
        )
        .unwrap();

        // Create a local store with existing stats
        let import_root = dir.path().join("import");
        std::fs::create_dir_all(&import_root).unwrap();

        // Pre-populate local stats with same record (should dedup)
        let mut local = crate::prediction::PromptCostHistory::new();
        local.record(crate::prediction::PromptRecord {
            instruction_hash: "xyz".into(),
            input_len: 100,
            cb_cost: 50,
            prompt_count: 1,
            backend: "mock".into(),
        });
        local.save(&import_root).unwrap();

        // Import bundle
        let contents = read_bundle(&bundle_path_str).unwrap();
        import_to_store(&contents, &import_root, MemoConflict::Skip).unwrap();

        // Verify no duplicates
        let loaded = crate::prediction::PromptCostHistory::load(&import_root);
        assert_eq!(loaded.len(), 1, "dedup should prevent duplicate");
    }
}
