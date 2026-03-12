// Checkpoint store for evolution state (Phase 9, M35).
//
// Content-addressed store in `.agentis/colony/` — same SHA-256
// scheme as ObjectStore but separate namespace.  Stores
// GenerationCheckpoint nodes that form a chain (each points to
// the previous generation's checkpoint).

use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

// --- Magic / version ---

const MAGIC: &[u8; 4] = b"AGCK";
const VERSION: u8 = 1;

// --- Error type ---

#[derive(Debug, Clone, PartialEq)]
pub enum CheckpointError {
    Io(String),
    NotFound(String),
    IntegrityError { expected: String, actual: String },
    InvalidFormat(String),
}

impl std::fmt::Display for CheckpointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckpointError::Io(msg) => write!(f, "checkpoint I/O error: {msg}"),
            CheckpointError::NotFound(hash) => write!(f, "checkpoint not found: {hash}"),
            CheckpointError::IntegrityError { expected, actual } => {
                write!(
                    f,
                    "checkpoint integrity error: expected {expected}, got {actual}"
                )
            }
            CheckpointError::InvalidFormat(msg) => {
                write!(f, "invalid checkpoint format: {msg}")
            }
        }
    }
}

impl From<std::io::Error> for CheckpointError {
    fn from(e: std::io::Error) -> Self {
        CheckpointError::Io(e.to_string())
    }
}

// --- Data types ---

#[derive(Debug, Clone, PartialEq)]
pub struct ParentEntry {
    pub source: String,
    pub source_hash: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GenerationCheckpoint {
    // Chain
    pub generation: u32,
    pub parent: Option<String>, // hash of previous checkpoint

    // Identity
    pub seed_hash: String,

    // Evolution state (needed for resume)
    pub parents: Vec<ParentEntry>,
    pub best_ever_score: f64,
    pub best_ever_source: String,
    pub best_ever_hash: String,
    pub stall_count: u32,
    pub cumulative_cb: u64,
    pub first_gen_avg_prompts: f64,

    // This generation's results
    pub gen_best_score: f64,
    pub gen_avg_score: f64,
    pub gen_avg_prompts: f64,
    pub variant_count: u32,

    // Metadata
    pub timestamp: u64,
    pub tag: Option<String>,
}

// --- Binary serialization ---

impl GenerationCheckpoint {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();

        // Header
        buf.extend_from_slice(MAGIC);
        buf.push(VERSION);

        // Chain
        write_u32(&mut buf, self.generation);
        match &self.parent {
            Some(h) => {
                buf.push(1);
                write_string(&mut buf, h);
            }
            None => buf.push(0),
        }
        write_string(&mut buf, &self.seed_hash);

        // Evolution state — parents
        write_u32(&mut buf, self.parents.len() as u32);
        for p in &self.parents {
            write_string(&mut buf, &p.source);
            write_string(&mut buf, &p.source_hash);
        }

        // Evolution state — scalars
        buf.extend_from_slice(&self.best_ever_score.to_le_bytes());
        write_string(&mut buf, &self.best_ever_source);
        write_string(&mut buf, &self.best_ever_hash);
        write_u32(&mut buf, self.stall_count);
        buf.extend_from_slice(&self.cumulative_cb.to_le_bytes());
        buf.extend_from_slice(&self.first_gen_avg_prompts.to_le_bytes());

        // Generation results
        buf.extend_from_slice(&self.gen_best_score.to_le_bytes());
        buf.extend_from_slice(&self.gen_avg_score.to_le_bytes());
        buf.extend_from_slice(&self.gen_avg_prompts.to_le_bytes());
        write_u32(&mut buf, self.variant_count);

        // Metadata
        buf.extend_from_slice(&self.timestamp.to_le_bytes());
        match &self.tag {
            Some(t) => {
                buf.push(1);
                write_string(&mut buf, t);
            }
            None => buf.push(0),
        }

        buf
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self, CheckpointError> {
        let mut pos = 0;

        // Header
        if data.len() < 5 {
            return Err(CheckpointError::InvalidFormat("too short".to_string()));
        }
        if &data[0..4] != MAGIC {
            return Err(CheckpointError::InvalidFormat(
                "bad magic bytes".to_string(),
            ));
        }
        pos += 4;
        let version = data[pos];
        if version != VERSION {
            return Err(CheckpointError::InvalidFormat(format!(
                "unsupported version: {version}"
            )));
        }
        pos += 1;

        // Chain
        let generation = read_u32(data, &mut pos)?;
        let has_parent = read_u8(data, &mut pos)?;
        let parent = if has_parent == 1 {
            Some(read_string(data, &mut pos)?)
        } else {
            None
        };
        let seed_hash = read_string(data, &mut pos)?;

        // Parents
        let parent_count = read_u32(data, &mut pos)?;
        let mut parents = Vec::with_capacity(parent_count as usize);
        for _ in 0..parent_count {
            let source = read_string(data, &mut pos)?;
            let source_hash = read_string(data, &mut pos)?;
            parents.push(ParentEntry {
                source,
                source_hash,
            });
        }

        // Scalars
        let best_ever_score = read_f64(data, &mut pos)?;
        let best_ever_source = read_string(data, &mut pos)?;
        let best_ever_hash = read_string(data, &mut pos)?;
        let stall_count = read_u32(data, &mut pos)?;
        let cumulative_cb = read_u64(data, &mut pos)?;
        let first_gen_avg_prompts = read_f64(data, &mut pos)?;

        // Generation results
        let gen_best_score = read_f64(data, &mut pos)?;
        let gen_avg_score = read_f64(data, &mut pos)?;
        let gen_avg_prompts = read_f64(data, &mut pos)?;
        let variant_count = read_u32(data, &mut pos)?;

        // Metadata
        let timestamp = read_u64(data, &mut pos)?;
        let has_tag = read_u8(data, &mut pos)?;
        let tag = if has_tag == 1 {
            Some(read_string(data, &mut pos)?)
        } else {
            None
        };

        Ok(GenerationCheckpoint {
            generation,
            parent,
            seed_hash,
            parents,
            best_ever_score,
            best_ever_source,
            best_ever_hash,
            stall_count,
            cumulative_cb,
            first_gen_avg_prompts,
            gen_best_score,
            gen_avg_score,
            gen_avg_prompts,
            variant_count,
            timestamp,
            tag,
        })
    }
}

// --- Checkpoint Store ---

pub struct CheckpointStore {
    root: PathBuf, // .agentis/colony
}

impl CheckpointStore {
    pub fn new(agentis_root: &Path) -> Self {
        Self {
            root: agentis_root.join("colony"),
        }
    }

    /// Ensure the colony directory structure exists.
    pub fn init(&self) -> Result<(), CheckpointError> {
        fs::create_dir_all(self.objects_dir())?;
        fs::create_dir_all(self.tags_dir())?;
        Ok(())
    }

    fn objects_dir(&self) -> PathBuf {
        self.root.join("objects")
    }

    fn tags_dir(&self) -> PathBuf {
        self.root.join("tags")
    }

    fn head_path(&self) -> PathBuf {
        self.root.join("HEAD")
    }

    fn object_path(&self, hash: &str) -> PathBuf {
        let (prefix, rest) = hash.split_at(2);
        self.objects_dir().join(prefix).join(rest)
    }

    fn tag_path(&self, name: &str) -> PathBuf {
        self.tags_dir().join(name)
    }

    /// Store a checkpoint. Returns its SHA-256 hash.
    pub fn store(&self, checkpoint: &GenerationCheckpoint) -> Result<String, CheckpointError> {
        self.init()?;
        let bytes = checkpoint.to_bytes();
        let hash = hash_bytes(&bytes);
        let path = self.object_path(&hash);

        if path.exists() {
            return Ok(hash);
        }

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, bytes)?;
        Ok(hash)
    }

    /// Load a checkpoint by exact hash.
    pub fn load(&self, hash: &str) -> Result<GenerationCheckpoint, CheckpointError> {
        let path = self.object_path(hash);
        if !path.exists() {
            return Err(CheckpointError::NotFound(hash.to_string()));
        }

        let data = fs::read(&path)?;

        // Verify integrity
        let actual = hash_bytes(&data);
        if actual != hash {
            return Err(CheckpointError::IntegrityError {
                expected: hash.to_string(),
                actual,
            });
        }

        GenerationCheckpoint::from_bytes(&data)
    }

    /// Get the current HEAD hash, if any.
    pub fn head(&self) -> Result<Option<String>, CheckpointError> {
        let path = self.head_path();
        if !path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(&path)?;
        let trimmed = content.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        Ok(Some(trimmed.to_string()))
    }

    /// Set the HEAD pointer to a checkpoint hash.
    pub fn set_head(&self, hash: &str) -> Result<(), CheckpointError> {
        self.init()?;
        fs::write(self.head_path(), format!("{hash}\n"))?;
        Ok(())
    }

    /// Create or update a named tag pointing to a checkpoint hash.
    pub fn set_tag(&self, name: &str, hash: &str) -> Result<(), CheckpointError> {
        self.init()?;
        // Verify the checkpoint exists
        if !self.object_path(hash).exists() {
            return Err(CheckpointError::NotFound(hash.to_string()));
        }
        fs::write(self.tag_path(name), format!("{hash}\n"))?;
        Ok(())
    }

    /// Resolve a tag name to a checkpoint hash.
    pub fn resolve_tag(&self, name: &str) -> Result<Option<String>, CheckpointError> {
        let path = self.tag_path(name);
        if !path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(&path)?;
        let trimmed = content.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        Ok(Some(trimmed.to_string()))
    }

    /// List all tags as (name, hash) pairs, sorted by name.
    pub fn list_tags(&self) -> Result<Vec<(String, String)>, CheckpointError> {
        let dir = self.tags_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut tags = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                let name = entry.file_name().to_string_lossy().to_string();
                let content = fs::read_to_string(entry.path())?;
                let hash = content.trim().to_string();
                if !hash.is_empty() {
                    tags.push((name, hash));
                }
            }
        }
        tags.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(tags)
    }

    /// Delete a tag by name. Returns true if it existed.
    pub fn delete_tag(&self, name: &str) -> Result<bool, CheckpointError> {
        let path = self.tag_path(name);
        if path.exists() {
            fs::remove_file(&path)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Resolve a hash-or-tag reference to a full checkpoint hash.
    /// Checks tags first, then tries prefix matching against stored objects.
    pub fn resolve(&self, reference: &str) -> Result<String, CheckpointError> {
        // Try as tag first
        if let Some(hash) = self.resolve_tag(reference)? {
            return Ok(hash);
        }

        // Try as exact hash
        if self.object_path(reference).exists() {
            return Ok(reference.to_string());
        }

        // Try prefix match
        if reference.len() >= 4 {
            let matches = self.prefix_match(reference)?;
            match matches.len() {
                0 => {}
                1 => return Ok(matches[0].clone()),
                _ => {
                    return Err(CheckpointError::InvalidFormat(format!(
                        "ambiguous prefix '{reference}': {} matches",
                        matches.len()
                    )));
                }
            }
        }

        Err(CheckpointError::NotFound(reference.to_string()))
    }

    /// Find all checkpoint hashes matching a prefix.
    fn prefix_match(&self, prefix: &str) -> Result<Vec<String>, CheckpointError> {
        let objects_dir = self.objects_dir();
        if !objects_dir.exists() {
            return Ok(Vec::new());
        }

        let mut matches = Vec::new();
        let dir_prefix = &prefix[..2.min(prefix.len())];

        let subdir = objects_dir.join(dir_prefix);
        if !subdir.exists() {
            return Ok(Vec::new());
        }

        let rest_prefix = if prefix.len() > 2 { &prefix[2..] } else { "" };

        for entry in fs::read_dir(&subdir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(rest_prefix) {
                matches.push(format!("{dir_prefix}{name}"));
            }
        }

        Ok(matches)
    }

    /// Check if a checkpoint exists by exact hash.
    pub fn exists(&self, hash: &str) -> bool {
        self.object_path(hash).exists()
    }

    /// List all checkpoint hashes in the store.
    pub fn list_all(&self) -> Result<Vec<String>, CheckpointError> {
        let objects_dir = self.objects_dir();
        if !objects_dir.exists() {
            return Ok(Vec::new());
        }

        let mut hashes = Vec::new();
        for dir_entry in fs::read_dir(&objects_dir)? {
            let dir_entry = dir_entry?;
            if dir_entry.file_type()?.is_dir() {
                let prefix = dir_entry.file_name().to_string_lossy().to_string();
                for file_entry in fs::read_dir(dir_entry.path())? {
                    let file_entry = file_entry?;
                    if file_entry.file_type()?.is_file() {
                        let rest = file_entry.file_name().to_string_lossy().to_string();
                        hashes.push(format!("{prefix}{rest}"));
                    }
                }
            }
        }
        Ok(hashes)
    }

    /// Walk the checkpoint chain from a given hash backwards.
    /// Returns checkpoints in order from newest to oldest.
    pub fn walk_chain(
        &self,
        start_hash: &str,
        limit: Option<usize>,
    ) -> Result<Vec<(String, GenerationCheckpoint)>, CheckpointError> {
        let mut chain = Vec::new();
        let mut current = Some(start_hash.to_string());

        while let Some(hash) = current {
            if let Some(max) = limit
                && chain.len() >= max
            {
                break;
            }
            let ckpt = self.load(&hash)?;
            let parent = ckpt.parent.clone();
            chain.push((hash, ckpt));
            current = parent;
        }

        Ok(chain)
    }
}

// --- Helpers ---

fn hash_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    result.iter().map(|b| format!("{b:02x}")).collect()
}

fn write_u32(buf: &mut Vec<u8>, val: u32) {
    buf.extend_from_slice(&val.to_le_bytes());
}

fn write_string(buf: &mut Vec<u8>, s: &str) {
    write_u32(buf, s.len() as u32);
    buf.extend_from_slice(s.as_bytes());
}

fn read_u8(data: &[u8], pos: &mut usize) -> Result<u8, CheckpointError> {
    if *pos >= data.len() {
        return Err(CheckpointError::InvalidFormat("truncated u8".to_string()));
    }
    let val = data[*pos];
    *pos += 1;
    Ok(val)
}

fn read_u32(data: &[u8], pos: &mut usize) -> Result<u32, CheckpointError> {
    if *pos + 4 > data.len() {
        return Err(CheckpointError::InvalidFormat("truncated u32".to_string()));
    }
    let val = u32::from_le_bytes(data[*pos..*pos + 4].try_into().unwrap());
    *pos += 4;
    Ok(val)
}

fn read_u64(data: &[u8], pos: &mut usize) -> Result<u64, CheckpointError> {
    if *pos + 8 > data.len() {
        return Err(CheckpointError::InvalidFormat("truncated u64".to_string()));
    }
    let val = u64::from_le_bytes(data[*pos..*pos + 8].try_into().unwrap());
    *pos += 8;
    Ok(val)
}

fn read_f64(data: &[u8], pos: &mut usize) -> Result<f64, CheckpointError> {
    if *pos + 8 > data.len() {
        return Err(CheckpointError::InvalidFormat("truncated f64".to_string()));
    }
    let val = f64::from_le_bytes(data[*pos..*pos + 8].try_into().unwrap());
    *pos += 8;
    Ok(val)
}

fn read_string(data: &[u8], pos: &mut usize) -> Result<String, CheckpointError> {
    let len = read_u32(data, pos)? as usize;
    if *pos + len > data.len() {
        return Err(CheckpointError::InvalidFormat(
            "truncated string".to_string(),
        ));
    }
    let val = String::from_utf8(data[*pos..*pos + len].to_vec())
        .map_err(|e| CheckpointError::InvalidFormat(format!("invalid UTF-8 in string: {e}")))?;
    *pos += len;
    Ok(val)
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    fn make_checkpoint(
        generation: u32,
        parent: Option<&str>,
        tag: Option<&str>,
    ) -> GenerationCheckpoint {
        GenerationCheckpoint {
            generation,
            parent: parent.map(|s| s.to_string()),
            seed_hash: "abcd1234".to_string(),
            parents: vec![
                ParentEntry {
                    source: "agent foo() { prompt(\"test\") }".to_string(),
                    source_hash: "hash1".to_string(),
                },
                ParentEntry {
                    source: "agent bar() { prompt(\"test2\") }".to_string(),
                    source_hash: "hash2".to_string(),
                },
            ],
            best_ever_score: 0.915,
            best_ever_source: "agent foo() { prompt(\"best\") }".to_string(),
            best_ever_hash: "besthash".to_string(),
            stall_count: 2,
            cumulative_cb: 48000,
            first_gen_avg_prompts: 3.5,
            gen_best_score: 0.890,
            gen_avg_score: 0.720,
            gen_avg_prompts: 2.8,
            variant_count: 8,
            timestamp: 1710280442000,
            tag: tag.map(|s| s.to_string()),
        }
    }

    // --- Serialization tests ---

    #[test]
    fn roundtrip_no_parent_no_tag() {
        let ckpt = make_checkpoint(1, None, None);
        let bytes = ckpt.to_bytes();
        let decoded = GenerationCheckpoint::from_bytes(&bytes).unwrap();
        assert_eq!(ckpt, decoded);
    }

    #[test]
    fn roundtrip_with_parent_and_tag() {
        let ckpt = make_checkpoint(5, Some("deadbeef1234"), Some("nightly-03-12"));
        let bytes = ckpt.to_bytes();
        let decoded = GenerationCheckpoint::from_bytes(&bytes).unwrap();
        assert_eq!(ckpt, decoded);
    }

    #[test]
    fn roundtrip_empty_parents() {
        let mut ckpt = make_checkpoint(1, None, None);
        ckpt.parents = Vec::new();
        let bytes = ckpt.to_bytes();
        let decoded = GenerationCheckpoint::from_bytes(&bytes).unwrap();
        assert_eq!(ckpt, decoded);
    }

    #[test]
    fn roundtrip_preserves_floats() {
        let mut ckpt = make_checkpoint(1, None, None);
        ckpt.best_ever_score = std::f64::consts::PI;
        ckpt.gen_avg_prompts = 0.0;
        ckpt.cumulative_cb = u64::MAX;
        let bytes = ckpt.to_bytes();
        let decoded = GenerationCheckpoint::from_bytes(&bytes).unwrap();
        assert_eq!(ckpt.best_ever_score, decoded.best_ever_score);
        assert_eq!(ckpt.gen_avg_prompts, decoded.gen_avg_prompts);
        assert_eq!(ckpt.cumulative_cb, decoded.cumulative_cb);
    }

    #[test]
    fn magic_bytes_present() {
        let ckpt = make_checkpoint(1, None, None);
        let bytes = ckpt.to_bytes();
        assert_eq!(&bytes[0..4], b"AGCK");
        assert_eq!(bytes[4], 1); // version
    }

    #[test]
    fn bad_magic_rejected() {
        let mut bytes = make_checkpoint(1, None, None).to_bytes();
        bytes[0] = b'X';
        let err = GenerationCheckpoint::from_bytes(&bytes).unwrap_err();
        assert!(matches!(err, CheckpointError::InvalidFormat(_)));
    }

    #[test]
    fn bad_version_rejected() {
        let mut bytes = make_checkpoint(1, None, None).to_bytes();
        bytes[4] = 99;
        let err = GenerationCheckpoint::from_bytes(&bytes).unwrap_err();
        assert!(matches!(err, CheckpointError::InvalidFormat(_)));
    }

    #[test]
    fn truncated_data_rejected() {
        let bytes = make_checkpoint(1, None, None).to_bytes();
        let err = GenerationCheckpoint::from_bytes(&bytes[..10]).unwrap_err();
        assert!(matches!(err, CheckpointError::InvalidFormat(_)));
    }

    #[test]
    fn empty_data_rejected() {
        let err = GenerationCheckpoint::from_bytes(&[]).unwrap_err();
        assert!(matches!(err, CheckpointError::InvalidFormat(_)));
    }

    // --- Store tests ---

    #[test]
    fn store_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(dir.path());
        let ckpt = make_checkpoint(3, None, Some("test-tag"));

        let hash = store.store(&ckpt).unwrap();
        assert_eq!(hash.len(), 64); // SHA-256 hex

        let loaded = store.load(&hash).unwrap();
        assert_eq!(ckpt, loaded);
    }

    #[test]
    fn store_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(dir.path());
        let ckpt = make_checkpoint(1, None, None);

        let h1 = store.store(&ckpt).unwrap();
        let h2 = store.store(&ckpt).unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn load_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(dir.path());
        store.init().unwrap();

        let err = store
            .load("0000000000000000000000000000000000000000000000000000000000000000")
            .unwrap_err();
        assert!(matches!(err, CheckpointError::NotFound(_)));
    }

    #[test]
    fn head_empty_initially() {
        let dir = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(dir.path());
        store.init().unwrap();
        assert_eq!(store.head().unwrap(), None);
    }

    #[test]
    fn head_set_and_get() {
        let dir = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(dir.path());
        let ckpt = make_checkpoint(1, None, None);
        let hash = store.store(&ckpt).unwrap();

        store.set_head(&hash).unwrap();
        assert_eq!(store.head().unwrap(), Some(hash));
    }

    #[test]
    fn tag_set_resolve_list_delete() {
        let dir = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(dir.path());
        let ckpt = make_checkpoint(1, None, None);
        let hash = store.store(&ckpt).unwrap();

        // Set tag
        store.set_tag("my-tag", &hash).unwrap();

        // Resolve
        assert_eq!(store.resolve_tag("my-tag").unwrap(), Some(hash.clone()));
        assert_eq!(store.resolve_tag("nonexistent").unwrap(), None);

        // List
        let tags = store.list_tags().unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].0, "my-tag");
        assert_eq!(tags[0].1, hash);

        // Delete
        assert!(store.delete_tag("my-tag").unwrap());
        assert!(!store.delete_tag("my-tag").unwrap()); // already gone
        assert_eq!(store.resolve_tag("my-tag").unwrap(), None);
    }

    #[test]
    fn tag_requires_existing_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(dir.path());
        store.init().unwrap();

        let err = store.set_tag("tag", "nonexistent_hash").unwrap_err();
        assert!(matches!(err, CheckpointError::NotFound(_)));
    }

    #[test]
    fn resolve_by_tag() {
        let dir = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(dir.path());
        let ckpt = make_checkpoint(1, None, None);
        let hash = store.store(&ckpt).unwrap();
        store.set_tag("v1", &hash).unwrap();

        let resolved = store.resolve("v1").unwrap();
        assert_eq!(resolved, hash);
    }

    #[test]
    fn resolve_by_exact_hash() {
        let dir = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(dir.path());
        let ckpt = make_checkpoint(1, None, None);
        let hash = store.store(&ckpt).unwrap();

        let resolved = store.resolve(&hash).unwrap();
        assert_eq!(resolved, hash);
    }

    #[test]
    fn resolve_by_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(dir.path());
        let ckpt = make_checkpoint(1, None, None);
        let hash = store.store(&ckpt).unwrap();

        // Use first 8 chars as prefix
        let prefix = &hash[..8];
        let resolved = store.resolve(prefix).unwrap();
        assert_eq!(resolved, hash);
    }

    #[test]
    fn resolve_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(dir.path());
        store.init().unwrap();

        let err = store.resolve("nonexistent").unwrap_err();
        assert!(matches!(err, CheckpointError::NotFound(_)));
    }

    #[test]
    fn exists_check() {
        let dir = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(dir.path());
        let ckpt = make_checkpoint(1, None, None);
        let hash = store.store(&ckpt).unwrap();

        assert!(store.exists(&hash));
        assert!(!store.exists("0000000000000000000000000000000000000000000000000000000000000000"));
    }

    #[test]
    fn list_all_checkpoints() {
        let dir = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(dir.path());

        let h1 = store.store(&make_checkpoint(1, None, None)).unwrap();
        let h2 = store.store(&make_checkpoint(2, Some(&h1), None)).unwrap();

        let mut all = store.list_all().unwrap();
        all.sort();
        let mut expected = vec![h1, h2];
        expected.sort();
        assert_eq!(all, expected);
    }

    #[test]
    fn walk_chain_full() {
        let dir = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(dir.path());

        let h1 = store.store(&make_checkpoint(1, None, None)).unwrap();
        let h2 = store.store(&make_checkpoint(2, Some(&h1), None)).unwrap();
        let h3 = store
            .store(&make_checkpoint(3, Some(&h2), Some("latest")))
            .unwrap();

        let chain = store.walk_chain(&h3, None).unwrap();
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0].1.generation, 3);
        assert_eq!(chain[1].1.generation, 2);
        assert_eq!(chain[2].1.generation, 1);
    }

    #[test]
    fn walk_chain_with_limit() {
        let dir = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(dir.path());

        let h1 = store.store(&make_checkpoint(1, None, None)).unwrap();
        let h2 = store.store(&make_checkpoint(2, Some(&h1), None)).unwrap();
        let h3 = store.store(&make_checkpoint(3, Some(&h2), None)).unwrap();

        let chain = store.walk_chain(&h3, Some(2)).unwrap();
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].1.generation, 3);
        assert_eq!(chain[1].1.generation, 2);
    }

    #[test]
    fn walk_chain_single() {
        let dir = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(dir.path());

        let h1 = store.store(&make_checkpoint(1, None, None)).unwrap();

        let chain = store.walk_chain(&h1, None).unwrap();
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].1.generation, 1);
        assert!(chain[0].1.parent.is_none());
    }

    #[test]
    fn integrity_error_on_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(dir.path());
        let ckpt = make_checkpoint(1, None, None);
        let hash = store.store(&ckpt).unwrap();

        // Corrupt the file
        let path = store.object_path(&hash);
        let mut data = fs::read(&path).unwrap();
        data[10] ^= 0xFF;
        fs::write(&path, &data).unwrap();

        let err = store.load(&hash).unwrap_err();
        assert!(matches!(err, CheckpointError::IntegrityError { .. }));
    }

    // --- M36: Auto-checkpoint + resume tests ---

    #[test]
    fn checkpoint_chain_simulates_evolution() {
        // Simulate 3 generations of evolution with checkpointing
        let dir = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(dir.path());
        let seed_hash = "seed1234abcd".to_string();

        let mut prev_hash: Option<String> = None;
        for g in 1..=3 {
            let ckpt = GenerationCheckpoint {
                generation: g,
                parent: prev_hash.clone(),
                seed_hash: seed_hash.clone(),
                parents: vec![ParentEntry {
                    source: format!("agent v{g}() {{ prompt(\"gen{g}\") }}"),
                    source_hash: format!("hash_g{g}"),
                }],
                best_ever_score: 0.5 + g as f64 * 0.1,
                best_ever_source: format!("best at gen {g}"),
                best_ever_hash: format!("best_hash_g{g}"),
                stall_count: 0,
                cumulative_cb: g as u64 * 1000,
                first_gen_avg_prompts: 2.0,
                gen_best_score: 0.5 + g as f64 * 0.1,
                gen_avg_score: 0.4 + g as f64 * 0.08,
                gen_avg_prompts: 2.0 - g as f64 * 0.1,
                variant_count: 4,
                timestamp: 1710280440000 + g as u64 * 60000,
                tag: None,
            };
            let hash = store.store(&ckpt).unwrap();
            store.set_head(&hash).unwrap();
            prev_hash = Some(hash);
        }

        // Verify HEAD points to gen 3
        let head = store.head().unwrap().unwrap();
        let head_ckpt = store.load(&head).unwrap();
        assert_eq!(head_ckpt.generation, 3);

        // Walk chain from HEAD — should get 3 → 2 → 1
        let chain = store.walk_chain(&head, None).unwrap();
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0].1.generation, 3);
        assert_eq!(chain[1].1.generation, 2);
        assert_eq!(chain[2].1.generation, 1);
        assert!(chain[2].1.parent.is_none());
    }

    #[test]
    fn resume_restores_state() {
        // Store a checkpoint, then load it and verify all resume-relevant fields
        let dir = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(dir.path());

        let ckpt = GenerationCheckpoint {
            generation: 5,
            parent: Some("prevhash".to_string()),
            seed_hash: "seedabc".to_string(),
            parents: vec![
                ParentEntry {
                    source: "agent a() { prompt(\"x\") }".to_string(),
                    source_hash: "ha".to_string(),
                },
                ParentEntry {
                    source: "agent b() { prompt(\"y\") }".to_string(),
                    source_hash: "hb".to_string(),
                },
            ],
            best_ever_score: 0.935,
            best_ever_source: "agent best() { prompt(\"z\") }".to_string(),
            best_ever_hash: "hbest".to_string(),
            stall_count: 2,
            cumulative_cb: 25000,
            first_gen_avg_prompts: 3.5,
            gen_best_score: 0.890,
            gen_avg_score: 0.720,
            gen_avg_prompts: 2.8,
            variant_count: 8,
            timestamp: 1710280442000,
            tag: None,
        };

        let hash = store.store(&ckpt).unwrap();
        store.set_head(&hash).unwrap();

        // Simulate resume: resolve, load, extract state
        let resolved = store.resolve(&hash[..8]).unwrap();
        let loaded = store.load(&resolved).unwrap();

        // Verify all resume-critical fields
        assert_eq!(loaded.generation, 5); // next gen = 6
        assert_eq!(loaded.parents.len(), 2);
        assert_eq!(loaded.parents[0].source, "agent a() { prompt(\"x\") }");
        assert_eq!(loaded.parents[1].source_hash, "hb");
        assert_eq!(loaded.best_ever_score, 0.935);
        assert_eq!(loaded.best_ever_source, "agent best() { prompt(\"z\") }");
        assert_eq!(loaded.best_ever_hash, "hbest");
        assert_eq!(loaded.stall_count, 2);
        assert_eq!(loaded.cumulative_cb, 25000);
        assert_eq!(loaded.first_gen_avg_prompts, 3.5);
    }

    #[test]
    fn tag_final_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(dir.path());

        // Simulate evolution with tagging at the end
        let ckpt = make_checkpoint(10, None, None);
        let hash = store.store(&ckpt).unwrap();
        store.set_head(&hash).unwrap();
        store.set_tag("experiment-a", &hash).unwrap();

        // Resolve by tag
        let resolved = store.resolve("experiment-a").unwrap();
        assert_eq!(resolved, hash);

        let loaded = store.load(&resolved).unwrap();
        assert_eq!(loaded.generation, 10);
    }

    #[test]
    fn checkpoint_interval_logic() {
        // Test the interval check: g % interval == 0 || is_last
        let interval = 3;

        let should =
            |g: usize, is_last: bool| -> bool { interval > 0 && (g % interval == 0 || is_last) };

        // Gen 1: not a multiple of 3, not last
        assert!(!should(1, false));
        // Gen 3: multiple of 3
        assert!(should(3, false));
        // Gen 6: multiple of 3
        assert!(should(6, false));
        // Gen 7: not multiple, not last
        assert!(!should(7, false));
        // Gen 10: last gen
        assert!(should(10, true));
        // Gen 8: early stop (is_last = true)
        assert!(should(8, true));
    }

    #[test]
    fn checkpoint_interval_zero_disables() {
        let interval = 0;
        let should =
            |g: usize, is_last: bool| -> bool { interval > 0 && (g % interval == 0 || is_last) };
        // Never checkpoint when interval is 0
        assert!(!should(1, false));
        assert!(!should(5, true));
        assert!(!should(10, true));
    }

    #[test]
    fn resume_from_tagged_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let store = CheckpointStore::new(dir.path());

        let h1 = store.store(&make_checkpoint(1, None, None)).unwrap();
        let h2 = store.store(&make_checkpoint(2, Some(&h1), None)).unwrap();
        store.set_tag("midpoint", &h2).unwrap();

        // Resume by tag
        let resolved = store.resolve("midpoint").unwrap();
        assert_eq!(resolved, h2);

        let ckpt = store.load(&resolved).unwrap();
        assert_eq!(ckpt.generation, 2);
        // Next gen would be 3
    }
}
