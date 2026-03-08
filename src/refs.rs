use std::fs;
use std::path::{Path, PathBuf};

use crate::storage::{Hash, ObjectStore};

#[derive(Debug)]
pub enum RefsError {
    Io(std::io::Error),
    BranchNotFound(String),
    BranchAlreadyExists(String),
    NoCurrentBranch,
    Storage(crate::storage::StorageError),
}

impl std::fmt::Display for RefsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RefsError::Io(e) => write!(f, "refs I/O error: {e}"),
            RefsError::BranchNotFound(name) => write!(f, "branch not found: {name}"),
            RefsError::BranchAlreadyExists(name) => write!(f, "branch already exists: {name}"),
            RefsError::NoCurrentBranch => write!(f, "no current branch (HEAD not set)"),
            RefsError::Storage(e) => write!(f, "storage error: {e}"),
        }
    }
}

impl From<std::io::Error> for RefsError {
    fn from(e: std::io::Error) -> Self {
        RefsError::Io(e)
    }
}

impl From<crate::storage::StorageError> for RefsError {
    fn from(e: crate::storage::StorageError) -> Self {
        RefsError::Storage(e)
    }
}

/// A commit object stored in the object store.
/// Contains the tree hash (program AST), parent commit hash, and timestamp.
#[derive(Debug, Clone, PartialEq)]
pub struct Commit {
    pub tree_hash: Hash,
    pub parent: Option<Hash>,
    pub timestamp: u64,
}

impl Commit {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        // tree hash (length-prefixed)
        let tree_bytes = self.tree_hash.as_bytes();
        buf.extend_from_slice(&(tree_bytes.len() as u32).to_le_bytes());
        buf.extend_from_slice(tree_bytes);
        // parent (option: 0 = none, 1 = some + length-prefixed)
        match &self.parent {
            None => buf.push(0),
            Some(parent) => {
                buf.push(1);
                let parent_bytes = parent.as_bytes();
                buf.extend_from_slice(&(parent_bytes.len() as u32).to_le_bytes());
                buf.extend_from_slice(parent_bytes);
            }
        }
        // timestamp
        buf.extend_from_slice(&self.timestamp.to_le_bytes());
        buf
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self, String> {
        let mut pos = 0;

        if data.len() < 4 {
            return Err("truncated commit data".into());
        }
        let tree_len = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        if data.len() < pos + tree_len {
            return Err("truncated tree hash".into());
        }
        let tree_hash = std::str::from_utf8(&data[pos..pos + tree_len])
            .map_err(|e| format!("invalid tree hash: {e}"))?
            .to_string();
        pos += tree_len;

        if pos >= data.len() {
            return Err("truncated parent flag".into());
        }
        let has_parent = data[pos];
        pos += 1;
        let parent = if has_parent != 0 {
            if data.len() < pos + 4 {
                return Err("truncated parent length".into());
            }
            let parent_len = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 4;
            if data.len() < pos + parent_len {
                return Err("truncated parent hash".into());
            }
            let parent_hash = std::str::from_utf8(&data[pos..pos + parent_len])
                .map_err(|e| format!("invalid parent hash: {e}"))?
                .to_string();
            pos += parent_len;
            Some(parent_hash)
        } else {
            None
        };

        if data.len() < pos + 8 {
            return Err("truncated timestamp".into());
        }
        let timestamp = u64::from_le_bytes(data[pos..pos + 8].try_into().unwrap());

        Ok(Commit { tree_hash, parent, timestamp })
    }
}

/// Reference manager for Agentis branches.
///
/// Layout:
///   .agentis/refs/heads/<branch_name>  — file containing commit hash
///   .agentis/HEAD                       — file containing current branch name
pub struct Refs {
    root: PathBuf,
}

impl Refs {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    fn heads_dir(&self) -> PathBuf {
        self.root.join("refs").join("heads")
    }

    fn head_file(&self) -> PathBuf {
        self.root.join("HEAD")
    }

    fn branch_file(&self, name: &str) -> PathBuf {
        self.heads_dir().join(name)
    }

    /// Initialize refs: create genesis branch (empty) and set HEAD.
    pub fn init(&self) -> Result<(), RefsError> {
        fs::create_dir_all(self.heads_dir())?;
        fs::write(self.branch_file("genesis"), "")?;
        fs::write(self.head_file(), "genesis")?;
        Ok(())
    }

    /// Get the current branch name from HEAD.
    pub fn current_branch(&self) -> Result<String, RefsError> {
        let head = self.head_file();
        if !head.exists() {
            return Err(RefsError::NoCurrentBranch);
        }
        let name = fs::read_to_string(&head)?.trim().to_string();
        if name.is_empty() {
            return Err(RefsError::NoCurrentBranch);
        }
        Ok(name)
    }

    /// Get the commit hash for a branch. Returns None if branch has no commits.
    pub fn get_branch_hash(&self, name: &str) -> Result<Option<Hash>, RefsError> {
        let path = self.branch_file(name);
        if !path.exists() {
            return Err(RefsError::BranchNotFound(name.to_string()));
        }
        let hash = fs::read_to_string(&path)?.trim().to_string();
        if hash.is_empty() {
            Ok(None)
        } else {
            Ok(Some(hash))
        }
    }

    /// Update a branch to point to a new commit hash.
    pub fn update_branch(&self, name: &str, hash: &str) -> Result<(), RefsError> {
        let path = self.branch_file(name);
        if !path.exists() {
            return Err(RefsError::BranchNotFound(name.to_string()));
        }
        fs::write(&path, hash)?;
        Ok(())
    }

    /// Create a commit: store commit object, update current branch.
    /// Returns (branch_name, commit_hash).
    pub fn commit(
        &self,
        tree_hash: &str,
        store: &ObjectStore,
    ) -> Result<(String, Hash), RefsError> {
        let branch = self.current_branch()?;
        let parent = self.get_branch_hash(&branch)?;

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let commit = Commit {
            tree_hash: tree_hash.to_string(),
            parent,
            timestamp,
        };

        let commit_hash = store.save_raw(&commit.to_bytes())?;
        self.update_branch(&branch, &commit_hash)?;

        Ok((branch, commit_hash))
    }

    /// Create a new branch. If from_hash is provided, it starts at that commit.
    /// Otherwise inherits the current branch's commit hash.
    pub fn create_branch(&self, name: &str, from_hash: Option<&str>) -> Result<(), RefsError> {
        let path = self.branch_file(name);
        if path.exists() {
            return Err(RefsError::BranchAlreadyExists(name.to_string()));
        }
        let hash = match from_hash {
            Some(h) => h.to_string(),
            None => {
                let current = self.current_branch()?;
                self.get_branch_hash(&current)?
                    .unwrap_or_default()
            }
        };
        fs::write(&path, &hash)?;
        Ok(())
    }

    /// Switch HEAD to a different branch.
    pub fn switch_branch(&self, name: &str) -> Result<(), RefsError> {
        if !self.branch_file(name).exists() {
            return Err(RefsError::BranchNotFound(name.to_string()));
        }
        fs::write(self.head_file(), name)?;
        Ok(())
    }

    /// List all branches. Current branch is marked.
    pub fn list_branches(&self) -> Result<Vec<(String, bool)>, RefsError> {
        let heads_dir = self.heads_dir();
        if !heads_dir.exists() {
            return Ok(Vec::new());
        }
        let current = self.current_branch().unwrap_or_default();
        let mut branches = Vec::new();
        for entry in fs::read_dir(&heads_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                let name = entry.file_name().to_string_lossy().to_string();
                let is_current = name == current;
                branches.push((name, is_current));
            }
        }
        branches.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(branches)
    }

    /// Walk the commit chain for a branch.
    /// Returns commits from newest to oldest.
    pub fn log(&self, name: &str, store: &ObjectStore) -> Result<Vec<Commit>, RefsError> {
        let mut commits = Vec::new();
        let mut current_hash = match self.get_branch_hash(name)? {
            Some(h) => h,
            None => return Ok(commits),
        };

        loop {
            let data = store.load_raw(&current_hash)?;
            let commit = Commit::from_bytes(&data)
                .map_err(|e| RefsError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))?;
            let parent = commit.parent.clone();
            commits.push(commit);
            match parent {
                Some(p) => current_hash = p,
                None => break,
            }
        }

        Ok(commits)
    }

    /// Resolve a branch to its tree (program) hash by reading the latest commit.
    pub fn resolve_tree(&self, name: &str, store: &ObjectStore) -> Result<Option<Hash>, RefsError> {
        let commit_hash = match self.get_branch_hash(name)? {
            Some(h) => h,
            None => return Ok(None),
        };
        let data = store.load_raw(&commit_hash)?;
        let commit = Commit::from_bytes(&data)
            .map_err(|e| RefsError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e)))?;
        Ok(Some(commit.tree_hash))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_env() -> (Refs, ObjectStore, TempDir) {
        let dir = TempDir::new();
        let root = dir.path().join(".agentis");
        let store = ObjectStore::init(&root).unwrap();
        let refs = Refs::new(&root);
        refs.init().unwrap();
        (refs, store, dir)
    }

    #[test]
    fn init_creates_genesis() {
        let (refs, _, _dir) = temp_env();
        assert_eq!(refs.current_branch().unwrap(), "genesis");
        assert_eq!(refs.get_branch_hash("genesis").unwrap(), None);
    }

    #[test]
    fn commit_creates_commit_object() {
        let (refs, store, _dir) = temp_env();
        let tree_hash = store.save_raw(b"fake program").unwrap();
        let (branch, commit_hash) = refs.commit(&tree_hash, &store).unwrap();

        assert_eq!(branch, "genesis");
        assert!(!commit_hash.is_empty());

        // Read back the commit
        let data = store.load_raw(&commit_hash).unwrap();
        let commit = Commit::from_bytes(&data).unwrap();
        assert_eq!(commit.tree_hash, tree_hash);
        assert!(commit.parent.is_none());
        assert!(commit.timestamp > 0);
    }

    #[test]
    fn commit_chain() {
        let (refs, store, _dir) = temp_env();

        let tree1 = store.save_raw(b"program v1").unwrap();
        let (_, hash1) = refs.commit(&tree1, &store).unwrap();

        let tree2 = store.save_raw(b"program v2").unwrap();
        let (_, hash2) = refs.commit(&tree2, &store).unwrap();

        let tree3 = store.save_raw(b"program v3").unwrap();
        let (_, hash3) = refs.commit(&tree3, &store).unwrap();

        // Verify chain
        let data3 = store.load_raw(&hash3).unwrap();
        let commit3 = Commit::from_bytes(&data3).unwrap();
        assert_eq!(commit3.tree_hash, tree3);
        assert_eq!(commit3.parent, Some(hash2.clone()));

        let data2 = store.load_raw(&hash2).unwrap();
        let commit2 = Commit::from_bytes(&data2).unwrap();
        assert_eq!(commit2.tree_hash, tree2);
        assert_eq!(commit2.parent, Some(hash1.clone()));

        let data1 = store.load_raw(&hash1).unwrap();
        let commit1 = Commit::from_bytes(&data1).unwrap();
        assert_eq!(commit1.tree_hash, tree1);
        assert!(commit1.parent.is_none());
    }

    #[test]
    fn log_returns_chain() {
        let (refs, store, _dir) = temp_env();

        let t1 = store.save_raw(b"v1").unwrap();
        refs.commit(&t1, &store).unwrap();
        let t2 = store.save_raw(b"v2").unwrap();
        refs.commit(&t2, &store).unwrap();
        let t3 = store.save_raw(b"v3").unwrap();
        refs.commit(&t3, &store).unwrap();

        let log = refs.log("genesis", &store).unwrap();
        assert_eq!(log.len(), 3);
        assert_eq!(log[0].tree_hash, t3); // newest first
        assert_eq!(log[1].tree_hash, t2);
        assert_eq!(log[2].tree_hash, t1);
        assert!(log[2].parent.is_none()); // oldest has no parent
    }

    #[test]
    fn log_empty_branch() {
        let (refs, store, _dir) = temp_env();
        let log = refs.log("genesis", &store).unwrap();
        assert!(log.is_empty());
    }

    #[test]
    fn resolve_tree() {
        let (refs, store, _dir) = temp_env();
        let tree_hash = store.save_raw(b"my program").unwrap();
        refs.commit(&tree_hash, &store).unwrap();

        let resolved = refs.resolve_tree("genesis", &store).unwrap();
        assert_eq!(resolved, Some(tree_hash));
    }

    #[test]
    fn resolve_tree_empty_branch() {
        let (refs, store, _dir) = temp_env();
        let resolved = refs.resolve_tree("genesis", &store).unwrap();
        assert!(resolved.is_none());
    }

    #[test]
    fn create_and_switch_branch() {
        let (refs, store, _dir) = temp_env();
        let tree = store.save_raw(b"prog").unwrap();
        refs.commit(&tree, &store).unwrap();

        refs.create_branch("feature", None).unwrap();
        refs.switch_branch("feature").unwrap();
        assert_eq!(refs.current_branch().unwrap(), "feature");

        // Feature branch should have same commit as genesis
        let genesis_hash = refs.get_branch_hash("genesis").unwrap();
        let feature_hash = refs.get_branch_hash("feature").unwrap();
        assert_eq!(genesis_hash, feature_hash);
    }

    #[test]
    fn create_branch_with_hash() {
        let (refs, _, _dir) = temp_env();
        refs.create_branch("custom", Some("custom_hash")).unwrap();
        assert_eq!(refs.get_branch_hash("custom").unwrap(), Some("custom_hash".to_string()));
    }

    #[test]
    fn create_duplicate_branch_fails() {
        let (refs, _, _dir) = temp_env();
        assert!(matches!(
            refs.create_branch("genesis", None),
            Err(RefsError::BranchAlreadyExists(_))
        ));
    }

    #[test]
    fn switch_nonexistent_branch_fails() {
        let (refs, _, _dir) = temp_env();
        assert!(matches!(
            refs.switch_branch("nonexistent"),
            Err(RefsError::BranchNotFound(_))
        ));
    }

    #[test]
    fn get_nonexistent_branch_fails() {
        let (refs, _, _dir) = temp_env();
        assert!(matches!(
            refs.get_branch_hash("nonexistent"),
            Err(RefsError::BranchNotFound(_))
        ));
    }

    #[test]
    fn list_branches() {
        let (refs, _, _dir) = temp_env();
        refs.create_branch("alpha", Some("")).unwrap();
        refs.create_branch("beta", Some("")).unwrap();

        let branches = refs.list_branches().unwrap();
        let names: Vec<&str> = branches.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"genesis"));
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));

        let current: Vec<&str> = branches.iter()
            .filter(|(_, is_current)| *is_current)
            .map(|(n, _)| n.as_str())
            .collect();
        assert_eq!(current, vec!["genesis"]);
    }

    #[test]
    fn commit_serialization_roundtrip() {
        let commit = Commit {
            tree_hash: "abc123".to_string(),
            parent: Some("def456".to_string()),
            timestamp: 1234567890,
        };
        let bytes = commit.to_bytes();
        let recovered = Commit::from_bytes(&bytes).unwrap();
        assert_eq!(commit, recovered);
    }

    #[test]
    fn commit_serialization_no_parent() {
        let commit = Commit {
            tree_hash: "abc123".to_string(),
            parent: None,
            timestamp: 1234567890,
        };
        let bytes = commit.to_bytes();
        let recovered = Commit::from_bytes(&bytes).unwrap();
        assert_eq!(commit, recovered);
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            use std::time::SystemTime;

            let mut hasher = DefaultHasher::new();
            SystemTime::now().hash(&mut hasher);
            std::thread::current().id().hash(&mut hasher);
            let id = hasher.finish();

            let mut path = std::env::temp_dir();
            path.push(format!("agentis-refs-test-{id}"));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
