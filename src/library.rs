// Persistent Population Library for elite variants (Phase 10, M39).
//
// Content-addressed store in `.agentis/library/` — same SHA-256
// scheme as ObjectStore and CheckpointStore but independent namespace.
// Stores LibraryEntry objects with source, fitness, provenance, and
// metadata. Supports tags, search (substring + fuzzy), and index
// rebuild.

use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

// --- Magic / version ---

const MAGIC: &[u8; 4] = b"AGlb";
const VERSION: u8 = 1;

// --- Error type ---

#[derive(Debug, Clone, PartialEq)]
pub enum LibraryError {
    Io(String),
    NotFound(String),
    IntegrityError { expected: String, actual: String },
    InvalidFormat(String),
    Duplicate(String),
}

impl std::fmt::Display for LibraryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LibraryError::Io(msg) => write!(f, "library I/O error: {msg}"),
            LibraryError::NotFound(hash) => write!(f, "library entry not found: {hash}"),
            LibraryError::IntegrityError { expected, actual } => {
                write!(
                    f,
                    "library integrity error: expected {expected}, got {actual}"
                )
            }
            LibraryError::InvalidFormat(msg) => {
                write!(f, "invalid library format: {msg}")
            }
            LibraryError::Duplicate(hash) => {
                write!(f, "duplicate source already in library: {hash}")
            }
        }
    }
}

impl From<std::io::Error> for LibraryError {
    fn from(e: std::io::Error) -> Self {
        LibraryError::Io(e.to_string())
    }
}

// --- Data types ---

#[derive(Debug, Clone, PartialEq)]
pub struct LibraryEntry {
    // Identity
    pub source: String,
    pub source_hash: String,

    // Provenance
    pub seed_hash: String,
    pub generation: u32,
    pub evolution_run: Option<String>, // checkpoint hash of the run

    // Fitness
    pub fitness_score: f64,
    pub cb_efficiency: f64,
    pub validate_rate: f64,
    pub explore_rate: f64,
    pub prompt_count: u32,

    // Metadata
    pub description: String,
    pub tags: Vec<String>,
    pub timestamp: u64, // Unix millis
}

// --- Binary serialization ---

impl LibraryEntry {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();

        // Header
        buf.extend_from_slice(MAGIC);
        buf.push(VERSION);

        // Identity
        write_string(&mut buf, &self.source);
        write_string(&mut buf, &self.source_hash);

        // Provenance
        write_string(&mut buf, &self.seed_hash);
        write_u32(&mut buf, self.generation);
        match &self.evolution_run {
            Some(h) => {
                buf.push(1);
                write_string(&mut buf, h);
            }
            None => buf.push(0),
        }

        // Fitness
        buf.extend_from_slice(&self.fitness_score.to_le_bytes());
        buf.extend_from_slice(&self.cb_efficiency.to_le_bytes());
        buf.extend_from_slice(&self.validate_rate.to_le_bytes());
        buf.extend_from_slice(&self.explore_rate.to_le_bytes());
        write_u32(&mut buf, self.prompt_count);

        // Metadata
        write_string(&mut buf, &self.description);
        write_u32(&mut buf, self.tags.len() as u32);
        for tag in &self.tags {
            write_string(&mut buf, tag);
        }
        buf.extend_from_slice(&self.timestamp.to_le_bytes());

        buf
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self, LibraryError> {
        let mut pos = 0;

        // Header
        if data.len() < 5 {
            return Err(LibraryError::InvalidFormat("too short".to_string()));
        }
        if &data[0..4] != MAGIC {
            return Err(LibraryError::InvalidFormat("bad magic bytes".to_string()));
        }
        pos += 4;
        let version = data[pos];
        if version != VERSION {
            return Err(LibraryError::InvalidFormat(format!(
                "unsupported version: {version}"
            )));
        }
        pos += 1;

        // Identity
        let source = read_string(data, &mut pos)?;
        let source_hash = read_string(data, &mut pos)?;

        // Provenance
        let seed_hash = read_string(data, &mut pos)?;
        let generation = read_u32(data, &mut pos)?;
        let has_run = read_u8(data, &mut pos)?;
        let evolution_run = if has_run == 1 {
            Some(read_string(data, &mut pos)?)
        } else {
            None
        };

        // Fitness
        let fitness_score = read_f64(data, &mut pos)?;
        let cb_efficiency = read_f64(data, &mut pos)?;
        let validate_rate = read_f64(data, &mut pos)?;
        let explore_rate = read_f64(data, &mut pos)?;
        let prompt_count = read_u32(data, &mut pos)?;

        // Metadata
        let description = read_string(data, &mut pos)?;
        let tag_count = read_u32(data, &mut pos)?;
        let mut tags = Vec::with_capacity(tag_count as usize);
        for _ in 0..tag_count {
            tags.push(read_string(data, &mut pos)?);
        }
        let timestamp = read_u64(data, &mut pos)?;

        Ok(LibraryEntry {
            source,
            source_hash,
            seed_hash,
            generation,
            evolution_run,
            fitness_score,
            cb_efficiency,
            validate_rate,
            explore_rate,
            prompt_count,
            description,
            tags,
            timestamp,
        })
    }
}

// --- Library Store ---

pub struct LibraryStore {
    root: PathBuf, // .agentis/library
}

impl LibraryStore {
    pub fn new(agentis_root: &Path) -> Self {
        Self {
            root: agentis_root.join("library"),
        }
    }

    /// Ensure the library directory structure exists.
    pub fn init(&self) -> Result<(), LibraryError> {
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

    fn index_path(&self) -> PathBuf {
        self.root.join("index")
    }

    fn object_path(&self, hash: &str) -> PathBuf {
        let (prefix, rest) = hash.split_at(2);
        self.objects_dir().join(prefix).join(rest)
    }

    fn tag_path(&self, name: &str) -> PathBuf {
        self.tags_dir().join(name)
    }

    /// Store a library entry. Returns its SHA-256 hash.
    pub fn store(&self, entry: &LibraryEntry) -> Result<String, LibraryError> {
        self.init()?;
        let bytes = entry.to_bytes();
        let hash = hash_bytes(&bytes);
        let path = self.object_path(&hash);

        if path.exists() {
            return Ok(hash);
        }

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, bytes)?;

        // Append to index
        self.append_index(&hash)?;

        Ok(hash)
    }

    /// Load a library entry by exact hash.
    pub fn load(&self, hash: &str) -> Result<LibraryEntry, LibraryError> {
        let path = self.object_path(hash);
        if !path.exists() {
            return Err(LibraryError::NotFound(hash.to_string()));
        }

        let data = fs::read(&path)?;

        // Verify integrity
        let actual = hash_bytes(&data);
        if actual != hash {
            return Err(LibraryError::IntegrityError {
                expected: hash.to_string(),
                actual,
            });
        }

        LibraryEntry::from_bytes(&data)
    }

    /// List all entry hashes from the index.
    pub fn list(&self) -> Result<Vec<String>, LibraryError> {
        let path = self.index_path();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&path)?;
        Ok(content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.trim().to_string())
            .collect())
    }

    /// Append a hash to the index file.
    fn append_index(&self, hash: &str) -> Result<(), LibraryError> {
        use std::io::Write;
        let path = self.index_path();
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        writeln!(file, "{hash}").map_err(|e| LibraryError::Io(e.to_string()))?;
        Ok(())
    }

    /// Create or update a named tag pointing to an entry hash.
    pub fn set_tag(&self, name: &str, hash: &str) -> Result<(), LibraryError> {
        self.init()?;
        if !self.object_path(hash).exists() {
            return Err(LibraryError::NotFound(hash.to_string()));
        }
        fs::write(self.tag_path(name), format!("{hash}\n"))?;
        Ok(())
    }

    /// Resolve a tag name to an entry hash.
    pub fn resolve_tag(&self, name: &str) -> Result<Option<String>, LibraryError> {
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
    pub fn list_tags(&self) -> Result<Vec<(String, String)>, LibraryError> {
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

    /// Remove an entry by hash. Returns true if it existed.
    pub fn remove(&self, hash: &str) -> Result<bool, LibraryError> {
        let path = self.object_path(hash);
        if !path.exists() {
            return Ok(false);
        }
        fs::remove_file(&path)?;
        // Remove empty prefix directory
        if let Some(parent) = path.parent() {
            let _ = fs::remove_dir(parent);
        }
        // Rebuild index without this hash
        self.remove_from_index(hash)?;
        // Remove any tags pointing to this hash
        if let Ok(tags) = self.list_tags() {
            for (name, th) in &tags {
                if th == hash {
                    let _ = fs::remove_file(self.tag_path(name));
                }
            }
        }
        Ok(true)
    }

    /// Remove a hash from the index file.
    fn remove_from_index(&self, hash: &str) -> Result<(), LibraryError> {
        let path = self.index_path();
        if !path.exists() {
            return Ok(());
        }
        let content = fs::read_to_string(&path)?;
        let filtered: Vec<&str> = content.lines().filter(|l| l.trim() != hash).collect();
        fs::write(&path, filtered.join("\n") + "\n")?;
        Ok(())
    }

    /// Search entries by query. Matches substring on description and tags,
    /// plus fuzzy match on tags (Levenshtein distance ≤ 2).
    /// Results are sorted by fitness score descending.
    pub fn search(&self, query: &str) -> Result<Vec<(String, LibraryEntry)>, LibraryError> {
        let hashes = self.list()?;
        let query_lower = query.to_lowercase();
        let mut results = Vec::new();

        for hash in hashes {
            let entry = match self.load(&hash) {
                Ok(e) => e,
                Err(_) => continue, // skip corrupt entries
            };

            let desc_match = entry.description.to_lowercase().contains(&query_lower);
            let tag_match = entry
                .tags
                .iter()
                .any(|t| t.to_lowercase().contains(&query_lower));
            let fuzzy_match = entry
                .tags
                .iter()
                .any(|t| levenshtein(&t.to_lowercase(), &query_lower) <= 2);

            if desc_match || tag_match || fuzzy_match {
                results.push((hash, entry));
            }
        }

        // Sort by fitness score descending
        results.sort_by(|a, b| {
            b.1.fitness_score
                .partial_cmp(&a.1.fitness_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(results)
    }

    /// Check if an entry exists by exact hash.
    pub fn exists(&self, hash: &str) -> bool {
        self.object_path(hash).exists()
    }

    /// Check if a source hash already exists in the library.
    pub fn has_source(&self, source_hash: &str) -> Result<bool, LibraryError> {
        let hashes = self.list()?;
        for hash in hashes {
            if let Ok(entry) = self.load(&hash)
                && entry.source_hash == source_hash
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Rebuild the index from objects directory.
    pub fn rebuild_index(&self) -> Result<usize, LibraryError> {
        let objects_dir = self.objects_dir();
        if !objects_dir.exists() {
            fs::write(self.index_path(), "")?;
            return Ok(0);
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
        hashes.sort();

        let content = hashes.join("\n") + if hashes.is_empty() { "" } else { "\n" };
        fs::write(self.index_path(), content)?;
        Ok(hashes.len())
    }

    /// Resolve a hash-or-tag reference to a full entry hash.
    pub fn resolve(&self, reference: &str) -> Result<String, LibraryError> {
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
                    return Err(LibraryError::InvalidFormat(format!(
                        "ambiguous prefix '{reference}': {} matches",
                        matches.len()
                    )));
                }
            }
        }

        Err(LibraryError::NotFound(reference.to_string()))
    }

    /// Find all entry hashes matching a prefix.
    fn prefix_match(&self, prefix: &str) -> Result<Vec<String>, LibraryError> {
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
}

// --- Formatting ---

/// Format the library list table.
pub fn format_list(entries: &[(String, LibraryEntry)]) -> String {
    if entries.is_empty() {
        return "Library is empty.\n".to_string();
    }

    let mut out = String::new();
    out.push_str(&format!("Library: {} entries\n\n", entries.len()));
    out.push_str("HASH          SCORE   TAGS              DESCRIPTION\n");

    for (hash, entry) in entries {
        let tags_str = if entry.tags.is_empty() {
            String::new()
        } else {
            entry.tags.join(", ")
        };
        let desc_short = if entry.description.len() > 40 {
            format!("{}...", &entry.description[..37])
        } else {
            entry.description.clone()
        };
        out.push_str(&format!(
            "{}   {:.3}   {:<18}{}\n",
            &hash[..12.min(hash.len())],
            entry.fitness_score,
            tags_str,
            desc_short,
        ));
    }

    out
}

/// Format detailed view of a single library entry.
pub fn format_show(hash: &str, entry: &LibraryEntry) -> String {
    let mut out = String::new();
    out.push_str(&format!("Library Entry: {hash}\n"));
    out.push_str(&format!("  Source hash:    {}\n", entry.source_hash));
    out.push_str(&format!(
        "  Seed hash:      {}...\n",
        &entry.seed_hash[..8.min(entry.seed_hash.len())]
    ));
    out.push_str(&format!("  Generation:     {}\n", entry.generation));
    match &entry.evolution_run {
        Some(run) => out.push_str(&format!(
            "  Evolution run:  {}...\n",
            &run[..12.min(run.len())]
        )),
        None => out.push_str("  Evolution run:  (none)\n"),
    }
    out.push_str(&format!("  Fitness score:  {:.3}\n", entry.fitness_score));
    out.push_str(&format!("  CB efficiency:  {:.3}\n", entry.cb_efficiency));
    out.push_str(&format!("  Validate rate:  {:.3}\n", entry.validate_rate));
    out.push_str(&format!("  Explore rate:   {:.3}\n", entry.explore_rate));
    out.push_str(&format!("  Prompt count:   {}\n", entry.prompt_count));
    if !entry.tags.is_empty() {
        out.push_str(&format!("  Tags:           {}\n", entry.tags.join(", ")));
    }
    out.push_str(&format!("  Description:    {}\n", entry.description));
    out.push_str(&format!(
        "  Date:           {}\n",
        crate::checkpoint::format_timestamp(entry.timestamp)
    ));
    out.push_str(&format!("  Source:         {} bytes\n", entry.source.len()));
    out
}

// --- Levenshtein distance ---

/// Compute Levenshtein edit distance between two strings.
fn levenshtein(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let m = a_chars.len();
    let n = b_chars.len();

    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }

    // Use two rows instead of full matrix
    let mut prev = vec![0usize; n + 1];
    let mut curr = vec![0usize; n + 1];

    for (j, slot) in prev.iter_mut().enumerate() {
        *slot = j;
    }

    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[n]
}

// --- Serialization helpers ---

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

fn read_u8(data: &[u8], pos: &mut usize) -> Result<u8, LibraryError> {
    if *pos >= data.len() {
        return Err(LibraryError::InvalidFormat("truncated u8".to_string()));
    }
    let val = data[*pos];
    *pos += 1;
    Ok(val)
}

fn read_u32(data: &[u8], pos: &mut usize) -> Result<u32, LibraryError> {
    if *pos + 4 > data.len() {
        return Err(LibraryError::InvalidFormat("truncated u32".to_string()));
    }
    let val = u32::from_le_bytes(data[*pos..*pos + 4].try_into().unwrap());
    *pos += 4;
    Ok(val)
}

fn read_u64(data: &[u8], pos: &mut usize) -> Result<u64, LibraryError> {
    if *pos + 8 > data.len() {
        return Err(LibraryError::InvalidFormat("truncated u64".to_string()));
    }
    let val = u64::from_le_bytes(data[*pos..*pos + 8].try_into().unwrap());
    *pos += 8;
    Ok(val)
}

fn read_f64(data: &[u8], pos: &mut usize) -> Result<f64, LibraryError> {
    if *pos + 8 > data.len() {
        return Err(LibraryError::InvalidFormat("truncated f64".to_string()));
    }
    let val = f64::from_le_bytes(data[*pos..*pos + 8].try_into().unwrap());
    *pos += 8;
    Ok(val)
}

fn read_string(data: &[u8], pos: &mut usize) -> Result<String, LibraryError> {
    let len = read_u32(data, pos)? as usize;
    if *pos + len > data.len() {
        return Err(LibraryError::InvalidFormat("truncated string".to_string()));
    }
    let val = String::from_utf8(data[*pos..*pos + len].to_vec())
        .map_err(|e| LibraryError::InvalidFormat(format!("invalid UTF-8 in string: {e}")))?;
    *pos += len;
    Ok(val)
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(score: f64, tags: Vec<&str>, desc: &str) -> LibraryEntry {
        LibraryEntry {
            source: "agent test(x: string): string { cb 100; return x; }".to_string(),
            source_hash: "abc123".to_string(),
            seed_hash: "seed456".to_string(),
            generation: 5,
            evolution_run: Some("run789".to_string()),
            fitness_score: score,
            cb_efficiency: 0.95,
            validate_rate: 1.0,
            explore_rate: 0.5,
            prompt_count: 2,
            description: desc.to_string(),
            tags: tags.into_iter().map(|s| s.to_string()).collect(),
            timestamp: 1773357242000,
        }
    }

    fn make_entry_with_source(source: &str, score: f64) -> LibraryEntry {
        LibraryEntry {
            source: source.to_string(),
            source_hash: format!("hash_{source}"),
            seed_hash: "seed456".to_string(),
            generation: 1,
            evolution_run: None,
            fitness_score: score,
            cb_efficiency: 0.9,
            validate_rate: 1.0,
            explore_rate: 0.0,
            prompt_count: 1,
            description: format!("Program: {source}"),
            tags: vec![],
            timestamp: 1773357242000,
        }
    }

    fn temp_store() -> (LibraryStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = LibraryStore::new(dir.path());
        store.init().unwrap();
        (store, dir)
    }

    // --- Serialization ---

    #[test]
    fn encode_decode_roundtrip() {
        let entry = make_entry(0.935, vec!["email-parser", "v2"], "Parses emails");
        let bytes = entry.to_bytes();
        let decoded = LibraryEntry::from_bytes(&bytes).unwrap();
        assert_eq!(entry, decoded);
    }

    #[test]
    fn encode_decode_no_run() {
        let mut entry = make_entry(0.8, vec![], "Simple test");
        entry.evolution_run = None;
        let bytes = entry.to_bytes();
        let decoded = LibraryEntry::from_bytes(&bytes).unwrap();
        assert_eq!(entry, decoded);
    }

    #[test]
    fn encode_decode_empty_tags() {
        let entry = make_entry(0.7, vec![], "No tags");
        let bytes = entry.to_bytes();
        let decoded = LibraryEntry::from_bytes(&bytes).unwrap();
        assert_eq!(entry.tags, decoded.tags);
    }

    #[test]
    fn encode_decode_many_tags() {
        let entry = make_entry(0.9, vec!["a", "b", "c", "d", "e"], "Many tags");
        let bytes = entry.to_bytes();
        let decoded = LibraryEntry::from_bytes(&bytes).unwrap();
        assert_eq!(entry.tags, decoded.tags);
    }

    #[test]
    fn decode_bad_magic() {
        let mut bytes = make_entry(0.5, vec![], "test").to_bytes();
        bytes[0] = b'X';
        assert!(matches!(
            LibraryEntry::from_bytes(&bytes),
            Err(LibraryError::InvalidFormat(_))
        ));
    }

    #[test]
    fn decode_bad_version() {
        let mut bytes = make_entry(0.5, vec![], "test").to_bytes();
        bytes[4] = 99;
        assert!(matches!(
            LibraryEntry::from_bytes(&bytes),
            Err(LibraryError::InvalidFormat(_))
        ));
    }

    #[test]
    fn decode_truncated() {
        let bytes = make_entry(0.5, vec![], "test").to_bytes();
        assert!(LibraryEntry::from_bytes(&bytes[..10]).is_err());
    }

    #[test]
    fn decode_too_short() {
        assert!(matches!(
            LibraryEntry::from_bytes(&[1, 2]),
            Err(LibraryError::InvalidFormat(_))
        ));
    }

    // --- Store / Load ---

    #[test]
    fn store_and_load() {
        let (store, _dir) = temp_store();
        let entry = make_entry(0.9, vec!["test"], "Test entry");
        let hash = store.store(&entry).unwrap();
        let loaded = store.load(&hash).unwrap();
        assert_eq!(entry, loaded);
    }

    #[test]
    fn store_idempotent() {
        let (store, _dir) = temp_store();
        let entry = make_entry(0.9, vec![], "Same entry");
        let h1 = store.store(&entry).unwrap();
        let h2 = store.store(&entry).unwrap();
        assert_eq!(h1, h2);
    }

    #[test]
    fn load_nonexistent() {
        let (store, _dir) = temp_store();
        assert!(matches!(
            store.load("deadbeef00000000000000000000000000000000000000000000000000000000"),
            Err(LibraryError::NotFound(_))
        ));
    }

    #[test]
    fn integrity_check() {
        let (store, _dir) = temp_store();
        let entry = make_entry(0.9, vec![], "Integrity");
        let hash = store.store(&entry).unwrap();
        // Corrupt
        let path = store.object_path(&hash);
        fs::write(&path, b"corrupted!").unwrap();
        assert!(matches!(
            store.load(&hash),
            Err(LibraryError::IntegrityError { .. })
        ));
    }

    #[test]
    fn exists_check() {
        let (store, _dir) = temp_store();
        let entry = make_entry(0.9, vec![], "Exists");
        let hash = store.store(&entry).unwrap();
        assert!(store.exists(&hash));
        assert!(!store.exists("nonexistent"));
    }

    // --- Index ---

    #[test]
    fn list_empty() {
        let (store, _dir) = temp_store();
        assert_eq!(store.list().unwrap(), Vec::<String>::new());
    }

    #[test]
    fn list_after_store() {
        let (store, _dir) = temp_store();
        let e1 = make_entry_with_source("prog1", 0.9);
        let e2 = make_entry_with_source("prog2", 0.8);
        let h1 = store.store(&e1).unwrap();
        let h2 = store.store(&e2).unwrap();
        let list = store.list().unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.contains(&h1));
        assert!(list.contains(&h2));
    }

    #[test]
    fn rebuild_index() {
        let (store, _dir) = temp_store();
        let e1 = make_entry_with_source("prog1", 0.9);
        let e2 = make_entry_with_source("prog2", 0.8);
        store.store(&e1).unwrap();
        store.store(&e2).unwrap();

        // Corrupt index
        fs::write(store.index_path(), "").unwrap();
        assert_eq!(store.list().unwrap().len(), 0);

        // Rebuild
        let count = store.rebuild_index().unwrap();
        assert_eq!(count, 2);
        assert_eq!(store.list().unwrap().len(), 2);
    }

    #[test]
    fn rebuild_index_empty() {
        let (store, _dir) = temp_store();
        let count = store.rebuild_index().unwrap();
        assert_eq!(count, 0);
    }

    // --- Tags ---

    #[test]
    fn tag_crud() {
        let (store, _dir) = temp_store();
        let entry = make_entry(0.9, vec![], "Tag test");
        let hash = store.store(&entry).unwrap();

        store.set_tag("my-tag", &hash).unwrap();
        assert_eq!(store.resolve_tag("my-tag").unwrap(), Some(hash.clone()));
        assert_eq!(store.resolve_tag("nonexistent").unwrap(), None);

        let tags = store.list_tags().unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0], ("my-tag".to_string(), hash));
    }

    #[test]
    fn tag_nonexistent_entry() {
        let (store, _dir) = temp_store();
        assert!(matches!(
            store.set_tag(
                "tag",
                "deadbeef00000000000000000000000000000000000000000000000000000000"
            ),
            Err(LibraryError::NotFound(_))
        ));
    }

    #[test]
    fn multiple_tags() {
        let (store, _dir) = temp_store();
        let e1 = make_entry_with_source("prog1", 0.9);
        let e2 = make_entry_with_source("prog2", 0.8);
        let h1 = store.store(&e1).unwrap();
        let h2 = store.store(&e2).unwrap();

        store.set_tag("alpha", &h1).unwrap();
        store.set_tag("beta", &h2).unwrap();

        let tags = store.list_tags().unwrap();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].0, "alpha");
        assert_eq!(tags[1].0, "beta");
    }

    // --- Remove ---

    #[test]
    fn remove_entry() {
        let (store, _dir) = temp_store();
        let entry = make_entry(0.9, vec![], "To remove");
        let hash = store.store(&entry).unwrap();

        assert!(store.exists(&hash));
        assert!(store.remove(&hash).unwrap());
        assert!(!store.exists(&hash));
        assert!(!store.list().unwrap().contains(&hash));
    }

    #[test]
    fn remove_nonexistent() {
        let (store, _dir) = temp_store();
        assert!(!store.remove("nonexistent").unwrap());
    }

    #[test]
    fn remove_cleans_tags() {
        let (store, _dir) = temp_store();
        let entry = make_entry(0.9, vec![], "Tag remove");
        let hash = store.store(&entry).unwrap();
        store.set_tag("my-tag", &hash).unwrap();

        store.remove(&hash).unwrap();
        assert_eq!(store.resolve_tag("my-tag").unwrap(), None);
    }

    // --- Search ---

    #[test]
    fn search_by_description() {
        let (store, _dir) = temp_store();
        let e1 = make_entry(0.9, vec![], "Classifies emails by urgency");
        let e2 = make_entry(0.8, vec![], "Parses JSON data");
        store.store(&e1).unwrap();
        store.store(&e2).unwrap();

        let results = store.search("email").unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].1.description.contains("email"));
    }

    #[test]
    fn search_by_tag() {
        let (store, _dir) = temp_store();
        let e1 = make_entry(0.9, vec!["email-parser"], "Some program");
        let e2 = make_entry(0.8, vec!["json-tool"], "Another program");
        store.store(&e1).unwrap();
        store.store(&e2).unwrap();

        let results = store.search("email").unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].1.tags.contains(&"email-parser".to_string()));
    }

    #[test]
    fn search_fuzzy_tag() {
        let (store, _dir) = temp_store();
        let e1 = make_entry(0.9, vec!["email"], "Some program");
        store.store(&e1).unwrap();

        // "emal" is Levenshtein distance 1 from "email"
        let results = store.search("emal").unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn search_no_results() {
        let (store, _dir) = temp_store();
        let e1 = make_entry(0.9, vec!["parser"], "Parses things");
        store.store(&e1).unwrap();

        let results = store.search("zzzzz").unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn search_sorted_by_score() {
        let (store, _dir) = temp_store();
        let e1 = make_entry(0.7, vec!["test"], "Test low");
        let e2 = make_entry(0.95, vec!["test"], "Test high");
        let e3 = make_entry(0.85, vec!["test"], "Test mid");
        store.store(&e1).unwrap();
        store.store(&e2).unwrap();
        store.store(&e3).unwrap();

        let results = store.search("test").unwrap();
        assert_eq!(results.len(), 3);
        assert!((results[0].1.fitness_score - 0.95).abs() < 0.001);
        assert!((results[1].1.fitness_score - 0.85).abs() < 0.001);
        assert!((results[2].1.fitness_score - 0.7).abs() < 0.001);
    }

    #[test]
    fn search_case_insensitive() {
        let (store, _dir) = temp_store();
        let e1 = make_entry(0.9, vec!["Email-Parser"], "Classifies EMAILS");
        store.store(&e1).unwrap();

        let results = store.search("email").unwrap();
        assert_eq!(results.len(), 1);
    }

    // --- Resolve ---

    #[test]
    fn resolve_by_tag() {
        let (store, _dir) = temp_store();
        let entry = make_entry(0.9, vec![], "Resolve tag");
        let hash = store.store(&entry).unwrap();
        store.set_tag("my-tag", &hash).unwrap();

        let resolved = store.resolve("my-tag").unwrap();
        assert_eq!(resolved, hash);
    }

    #[test]
    fn resolve_by_exact_hash() {
        let (store, _dir) = temp_store();
        let entry = make_entry(0.9, vec![], "Resolve hash");
        let hash = store.store(&entry).unwrap();

        let resolved = store.resolve(&hash).unwrap();
        assert_eq!(resolved, hash);
    }

    #[test]
    fn resolve_by_prefix() {
        let (store, _dir) = temp_store();
        let entry = make_entry(0.9, vec![], "Resolve prefix");
        let hash = store.store(&entry).unwrap();

        let prefix = &hash[..8];
        let resolved = store.resolve(prefix).unwrap();
        assert_eq!(resolved, hash);
    }

    #[test]
    fn resolve_not_found() {
        let (store, _dir) = temp_store();
        assert!(matches!(
            store.resolve("nonexistent"),
            Err(LibraryError::NotFound(_))
        ));
    }

    // --- has_source ---

    #[test]
    fn has_source_check() {
        let (store, _dir) = temp_store();
        let entry = make_entry(0.9, vec![], "Source check");
        store.store(&entry).unwrap();

        assert!(store.has_source("abc123").unwrap());
        assert!(!store.has_source("nonexistent").unwrap());
    }

    // --- Levenshtein ---

    #[test]
    fn levenshtein_identical() {
        assert_eq!(levenshtein("hello", "hello"), 0);
    }

    #[test]
    fn levenshtein_one_edit() {
        assert_eq!(levenshtein("email", "emal"), 1); // deletion
        assert_eq!(levenshtein("email", "emailx"), 1); // insertion
        assert_eq!(levenshtein("email", "emeil"), 1); // substitution
    }

    #[test]
    fn levenshtein_two_edits() {
        assert_eq!(levenshtein("email", "eml"), 2);
    }

    #[test]
    fn levenshtein_empty() {
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("abc", ""), 3);
        assert_eq!(levenshtein("", ""), 0);
    }

    // --- Formatting ---

    #[test]
    fn format_list_empty() {
        let out = format_list(&[]);
        assert!(out.contains("empty"));
    }

    #[test]
    fn format_list_entries() {
        let e1 = make_entry(0.935, vec!["email"], "Classifies emails by urgency");
        let entries = vec![("abcdef123456".to_string(), e1)];
        let out = format_list(&entries);
        assert!(out.contains("Library: 1 entries"));
        assert!(out.contains("HASH"));
        assert!(out.contains("0.935"));
        assert!(out.contains("email"));
    }

    #[test]
    fn format_show_entry() {
        let entry = make_entry(0.935, vec!["email", "v2"], "Email classifier");
        let out = format_show("fullhash1234567890", &entry);
        assert!(out.contains("Library Entry: fullhash1234567890"));
        assert!(out.contains("Fitness score:  0.935"));
        assert!(out.contains("Tags:           email, v2"));
        assert!(out.contains("Generation:     5"));
        assert!(out.contains("Description:    Email classifier"));
    }

    #[test]
    fn format_show_no_run() {
        let mut entry = make_entry(0.8, vec![], "No run");
        entry.evolution_run = None;
        let out = format_show("hash123", &entry);
        assert!(out.contains("Evolution run:  (none)"));
    }
}
