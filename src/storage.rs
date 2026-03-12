use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

use crate::ast::Serialize;

/// A SHA-256 hash represented as a hex string.
pub type Hash = String;

#[derive(Debug)]
pub enum StorageError {
    Io(std::io::Error),
    IntegrityError { expected: Hash, actual: Hash },
    NotFound(Hash),
    DeserializeError(String),
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageError::Io(e) => write!(f, "storage I/O error: {e}"),
            StorageError::IntegrityError { expected, actual } => {
                write!(
                    f,
                    "integrity check failed: expected {expected}, got {actual}"
                )
            }
            StorageError::NotFound(hash) => write!(f, "object not found: {hash}"),
            StorageError::DeserializeError(msg) => write!(f, "deserialization error: {msg}"),
        }
    }
}

impl From<std::io::Error> for StorageError {
    fn from(e: std::io::Error) -> Self {
        StorageError::Io(e)
    }
}

/// Content-addressed object store.
///
/// Objects are stored in `.agentis/objects/<first-2-chars>/<rest-of-hash>`
/// following Git-style fanout for filesystem efficiency.
pub struct ObjectStore {
    root: PathBuf,
}

impl ObjectStore {
    /// Create a new ObjectStore rooted at the given directory.
    /// The `root` should be the `.agentis` directory.
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    fn objects_dir(&self) -> PathBuf {
        self.root.join("objects")
    }

    fn object_path(&self, hash: &str) -> PathBuf {
        let (prefix, rest) = hash.split_at(2);
        self.objects_dir().join(prefix).join(rest)
    }

    /// Compute SHA-256 hash of raw bytes, returned as hex string.
    pub fn hash_bytes(data: &[u8]) -> Hash {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let result = hasher.finalize();
        hex_encode(&result)
    }

    /// Save a serializable AST node to the object store.
    /// Returns the SHA-256 hash of the serialized bytes.
    pub fn save<T: Serialize>(&self, node: &T) -> Result<Hash, StorageError> {
        let bytes = node.to_bytes();
        self.save_raw(&bytes)
    }

    /// Save raw bytes to the object store.
    /// Returns the SHA-256 hash.
    pub fn save_raw(&self, data: &[u8]) -> Result<Hash, StorageError> {
        let hash = Self::hash_bytes(data);
        let path = self.object_path(&hash);

        // Skip if already exists (content-addressed = idempotent)
        if path.exists() {
            return Ok(hash);
        }

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, data)?;
        Ok(hash)
    }

    /// Load raw bytes from the object store by hash.
    /// Verifies integrity on read.
    pub fn load_raw(&self, hash: &str) -> Result<Vec<u8>, StorageError> {
        let path = self.object_path(hash);
        if !path.exists() {
            return Err(StorageError::NotFound(hash.to_string()));
        }

        let data = fs::read(&path)?;

        // Verify integrity
        let actual_hash = Self::hash_bytes(&data);
        if actual_hash != hash {
            return Err(StorageError::IntegrityError {
                expected: hash.to_string(),
                actual: actual_hash,
            });
        }

        Ok(data)
    }

    /// Load and deserialize an AST node from the object store by hash.
    pub fn load<T: Serialize>(&self, hash: &str) -> Result<T, StorageError> {
        let data = self.load_raw(hash)?;
        T::from_bytes(&data).map_err(|e| StorageError::DeserializeError(e.0))
    }

    #[allow(dead_code)]
    pub fn exists(&self, hash: &str) -> bool {
        self.object_path(hash).exists()
    }

    /// List all object hashes in the store.
    pub fn list_objects(&self) -> Result<Vec<Hash>, StorageError> {
        let objects_dir = self.objects_dir();
        if !objects_dir.exists() {
            return Ok(Vec::new());
        }

        let mut hashes = Vec::new();
        for prefix_entry in fs::read_dir(&objects_dir)? {
            let prefix_entry = prefix_entry?;
            if !prefix_entry.file_type()?.is_dir() {
                continue;
            }
            let prefix = prefix_entry.file_name().to_string_lossy().to_string();
            if prefix.len() != 2 {
                continue;
            }
            for obj_entry in fs::read_dir(prefix_entry.path())? {
                let obj_entry = obj_entry?;
                if obj_entry.file_type()?.is_file() {
                    let rest = obj_entry.file_name().to_string_lossy().to_string();
                    hashes.push(format!("{prefix}{rest}"));
                }
            }
        }
        hashes.sort();
        Ok(hashes)
    }

    /// Initialize the .agentis directory structure.
    pub fn init(root: &Path) -> Result<Self, StorageError> {
        fs::create_dir_all(root.join("objects"))?;
        fs::create_dir_all(root.join("refs").join("heads"))?;
        Ok(Self::new(root))
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::*;
    use std::fs;

    fn temp_store() -> (ObjectStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = ObjectStore::init(dir.path()).unwrap();
        (store, dir)
    }

    // --- Basic operations ---

    #[test]
    fn save_and_load_raw() {
        let (store, _dir) = temp_store();
        let data = b"hello agentis";
        let hash = store.save_raw(data).unwrap();

        assert!(store.exists(&hash));
        let loaded = store.load_raw(&hash).unwrap();
        assert_eq!(loaded, data);
    }

    #[test]
    fn save_is_idempotent() {
        let (store, _dir) = temp_store();
        let data = b"same content";
        let hash1 = store.save_raw(data).unwrap();
        let hash2 = store.save_raw(data).unwrap();
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn deterministic_hashing() {
        let data = b"deterministic";
        let hash1 = ObjectStore::hash_bytes(data);
        let hash2 = ObjectStore::hash_bytes(data);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn different_content_different_hash() {
        let hash1 = ObjectStore::hash_bytes(b"aaa");
        let hash2 = ObjectStore::hash_bytes(b"bbb");
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn load_nonexistent() {
        let (store, _dir) = temp_store();
        let result =
            store.load_raw("deadbeef00000000000000000000000000000000000000000000000000000000");
        assert!(matches!(result, Err(StorageError::NotFound(_))));
    }

    #[test]
    fn integrity_check() {
        let (store, _dir) = temp_store();
        let data = b"original content";
        let hash = store.save_raw(data).unwrap();

        // Corrupt the file
        let path = store.object_path(&hash);
        fs::write(&path, b"corrupted!").unwrap();

        let result = store.load_raw(&hash);
        assert!(matches!(result, Err(StorageError::IntegrityError { .. })));
    }

    // --- AST node storage ---

    #[test]
    fn save_and_load_expr() {
        let (store, _dir) = temp_store();
        let expr = Expr::Binary(Box::new(BinaryExpr {
            op: BinaryOp::Add,
            left: Expr::IntLiteral(1),
            right: Expr::IntLiteral(2),
        }));

        let hash = store.save(&expr).unwrap();
        let loaded: Expr = store.load(&hash).unwrap();
        assert_eq!(expr, loaded);
    }

    #[test]
    fn save_and_load_program() {
        let (store, _dir) = temp_store();
        let program = Program {
            declarations: vec![Declaration::Function(FnDecl {
                name: "add".into(),
                params: vec![
                    Param {
                        name: "a".into(),
                        type_annotation: TypeAnnotation::Named("int".into()),
                    },
                    Param {
                        name: "b".into(),
                        type_annotation: TypeAnnotation::Named("int".into()),
                    },
                ],
                return_type: Some(TypeAnnotation::Named("int".into())),
                body: Block {
                    statements: vec![Statement::Return(ReturnStmt {
                        value: Some(Expr::Binary(Box::new(BinaryExpr {
                            op: BinaryOp::Add,
                            left: Expr::Identifier("a".into()),
                            right: Expr::Identifier("b".into()),
                        }))),
                    })],
                },
            })],
        };

        let hash = store.save(&program).unwrap();
        let loaded: Program = store.load(&hash).unwrap();
        assert_eq!(program, loaded);
    }

    #[test]
    fn same_ast_same_hash() {
        let (store, _dir) = temp_store();
        let expr1 = Expr::IntLiteral(42);
        let expr2 = Expr::IntLiteral(42);

        let hash1 = store.save(&expr1).unwrap();
        let hash2 = store.save(&expr2).unwrap();
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn different_ast_different_hash() {
        let (store, _dir) = temp_store();
        let hash1 = store.save(&Expr::IntLiteral(1)).unwrap();
        let hash2 = store.save(&Expr::IntLiteral(2)).unwrap();
        assert_ne!(hash1, hash2);
    }

    // --- Agent with prompt ---

    #[test]
    fn save_and_load_agent_with_prompt() {
        let (store, _dir) = temp_store();
        let agent = Declaration::Agent(AgentDecl {
            name: "analyzer".into(),
            params: vec![Param {
                name: "data".into(),
                type_annotation: TypeAnnotation::Named("string".into()),
            }],
            return_type: Some(TypeAnnotation::Named("Report".into())),
            body: Block {
                statements: vec![
                    Statement::Cb(CbStmt { budget: 500 }),
                    Statement::Let(LetStmt {
                        name: "result".into(),
                        type_annotation: None,
                        value: Expr::Prompt(Box::new(PromptExpr {
                            instruction: "Analyze this".into(),
                            input: Expr::Identifier("data".into()),
                            return_type: TypeAnnotation::Named("Report".into()),
                        })),
                    }),
                    Statement::Return(ReturnStmt {
                        value: Some(Expr::Identifier("result".into())),
                    }),
                ],
            },
        });

        let hash = store.save(&agent).unwrap();
        let loaded: Declaration = store.load(&hash).unwrap();
        assert_eq!(agent, loaded);
    }

    // --- List objects ---

    #[test]
    fn list_objects_empty() {
        let (store, _dir) = temp_store();
        let objects = store.list_objects().unwrap();
        assert!(objects.is_empty());
    }

    #[test]
    fn list_objects_after_saves() {
        let (store, _dir) = temp_store();
        let h1 = store.save(&Expr::IntLiteral(1)).unwrap();
        let h2 = store.save(&Expr::IntLiteral(2)).unwrap();
        let h3 = store.save(&Expr::IntLiteral(3)).unwrap();

        let objects = store.list_objects().unwrap();
        assert_eq!(objects.len(), 3);
        assert!(objects.contains(&h1));
        assert!(objects.contains(&h2));
        assert!(objects.contains(&h3));
    }

    // --- Init ---

    #[test]
    fn init_creates_structure() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".agentis");
        ObjectStore::init(&root).unwrap();

        assert!(root.join("objects").exists());
        assert!(root.join("refs").join("heads").exists());
    }

    // --- Fanout structure ---

    #[test]
    fn fanout_directory_structure() {
        let (store, _dir) = temp_store();
        let hash = store.save_raw(b"test fanout").unwrap();

        let prefix = &hash[..2];
        let rest = &hash[2..];
        let expected_path = store.objects_dir().join(prefix).join(rest);
        assert!(expected_path.exists());
    }

    // --- Hash format ---

    #[test]
    fn hash_is_64_char_hex() {
        let hash = ObjectStore::hash_bytes(b"test");
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }
}

pub(crate) mod tempfile {
    use std::path::{Path, PathBuf};

    #[allow(dead_code)]
    pub struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        #[allow(dead_code)]
        pub fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[allow(dead_code)]
    pub fn tempdir() -> Result<TempDir, std::io::Error> {
        let mut path = std::env::temp_dir();
        path.push(format!("agentis-test-{}", std::process::id()));
        path.push(format!("{}", rand_u64()));
        std::fs::create_dir_all(&path)?;
        Ok(TempDir { path })
    }

    #[allow(dead_code)]
    fn rand_u64() -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        use std::time::SystemTime;

        let mut hasher = DefaultHasher::new();
        SystemTime::now().hash(&mut hasher);
        std::thread::current().id().hash(&mut hasher);
        hasher.finish()
    }
}
