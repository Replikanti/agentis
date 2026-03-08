use std::fs;
use std::path::{Path, PathBuf};

use crate::storage::Hash;

#[derive(Debug)]
pub enum RefsError {
    Io(std::io::Error),
    BranchNotFound(String),
    BranchAlreadyExists(String),
    NoCurrentBranch,
}

impl std::fmt::Display for RefsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RefsError::Io(e) => write!(f, "refs I/O error: {e}"),
            RefsError::BranchNotFound(name) => write!(f, "branch not found: {name}"),
            RefsError::BranchAlreadyExists(name) => write!(f, "branch already exists: {name}"),
            RefsError::NoCurrentBranch => write!(f, "no current branch (HEAD not set)"),
        }
    }
}

impl From<std::io::Error> for RefsError {
    fn from(e: std::io::Error) -> Self {
        RefsError::Io(e)
    }
}

/// Reference manager for Agentis branches.
///
/// Layout:
///   .agentis/refs/heads/<branch_name>  — file containing root hash
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
        // Create empty genesis branch
        fs::write(self.branch_file("genesis"), "")?;
        // Set HEAD to genesis
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

    /// Get the root hash for a branch. Returns None if branch has no commits.
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

    /// Update a branch to point to a new root hash.
    pub fn update_branch(&self, name: &str, hash: &str) -> Result<(), RefsError> {
        let path = self.branch_file(name);
        if !path.exists() {
            return Err(RefsError::BranchNotFound(name.to_string()));
        }
        fs::write(&path, hash)?;
        Ok(())
    }

    /// Update the current branch (HEAD) to point to a new root hash.
    pub fn commit(&self, hash: &str) -> Result<String, RefsError> {
        let branch = self.current_branch()?;
        self.update_branch(&branch, hash)?;
        Ok(branch)
    }

    /// Create a new branch. If from_hash is provided, it starts at that hash.
    /// Otherwise inherits the current branch's hash.
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

    /// Get commit log for a branch (list of hashes).
    /// In Phase 1, each branch just points to one hash (no chain yet).
    pub fn log(&self, name: &str) -> Result<Vec<Hash>, RefsError> {
        match self.get_branch_hash(name)? {
            Some(hash) => Ok(vec![hash]),
            None => Ok(Vec::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_refs() -> (Refs, TempDir) {
        let dir = TempDir::new();
        let root = dir.path().join(".agentis");
        let refs = Refs::new(&root);
        refs.init().unwrap();
        (refs, dir)
    }

    #[test]
    fn init_creates_genesis() {
        let (refs, _dir) = temp_refs();
        assert_eq!(refs.current_branch().unwrap(), "genesis");
        assert_eq!(refs.get_branch_hash("genesis").unwrap(), None);
    }

    #[test]
    fn commit_updates_branch() {
        let (refs, _dir) = temp_refs();
        let hash = "abc123def456";
        let branch = refs.commit(hash).unwrap();
        assert_eq!(branch, "genesis");
        assert_eq!(refs.get_branch_hash("genesis").unwrap(), Some(hash.to_string()));
    }

    #[test]
    fn create_and_switch_branch() {
        let (refs, _dir) = temp_refs();
        refs.commit("hash1").unwrap();
        refs.create_branch("feature", None).unwrap();
        assert_eq!(refs.get_branch_hash("feature").unwrap(), Some("hash1".to_string()));

        refs.switch_branch("feature").unwrap();
        assert_eq!(refs.current_branch().unwrap(), "feature");
    }

    #[test]
    fn create_branch_with_hash() {
        let (refs, _dir) = temp_refs();
        refs.create_branch("custom", Some("custom_hash")).unwrap();
        assert_eq!(refs.get_branch_hash("custom").unwrap(), Some("custom_hash".to_string()));
    }

    #[test]
    fn create_duplicate_branch_fails() {
        let (refs, _dir) = temp_refs();
        assert!(matches!(
            refs.create_branch("genesis", None),
            Err(RefsError::BranchAlreadyExists(_))
        ));
    }

    #[test]
    fn switch_nonexistent_branch_fails() {
        let (refs, _dir) = temp_refs();
        assert!(matches!(
            refs.switch_branch("nonexistent"),
            Err(RefsError::BranchNotFound(_))
        ));
    }

    #[test]
    fn get_nonexistent_branch_fails() {
        let (refs, _dir) = temp_refs();
        assert!(matches!(
            refs.get_branch_hash("nonexistent"),
            Err(RefsError::BranchNotFound(_))
        ));
    }

    #[test]
    fn list_branches() {
        let (refs, _dir) = temp_refs();
        refs.create_branch("alpha", Some("")).unwrap();
        refs.create_branch("beta", Some("")).unwrap();

        let branches = refs.list_branches().unwrap();
        let names: Vec<&str> = branches.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"genesis"));
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));

        // genesis should be current
        let current: Vec<&str> = branches.iter()
            .filter(|(_, is_current)| *is_current)
            .map(|(n, _)| n.as_str())
            .collect();
        assert_eq!(current, vec!["genesis"]);
    }

    #[test]
    fn log_empty_branch() {
        let (refs, _dir) = temp_refs();
        assert!(refs.log("genesis").unwrap().is_empty());
    }

    #[test]
    fn log_with_commit() {
        let (refs, _dir) = temp_refs();
        refs.commit("hash123").unwrap();
        let log = refs.log("genesis").unwrap();
        assert_eq!(log, vec!["hash123"]);
    }

    #[test]
    fn update_nonexistent_branch_fails() {
        let (refs, _dir) = temp_refs();
        assert!(matches!(
            refs.update_branch("nope", "hash"),
            Err(RefsError::BranchNotFound(_))
        ));
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
